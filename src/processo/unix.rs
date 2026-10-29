//! AF_UNIX: soquetes locais entre processos (e com o kernel).
//!
//!   socketpair                 dois soquetes ja ligados
//!   bind + listen / connect    por caminho ("/tmp/.X11-unix/X0") ou nome
//!                              abstrato ("\0nome"); o caminho nao vira
//!                              arquivo no disco (o FAT32 nao guarda)
//!   SCM_RIGHTS                 descritores mandados junto com os bytes
//!
//! So SOCK_STREAM de verdade: SOCK_DGRAM e SOCK_SEQPACKET caem no mesmo
//! canal (os limites das mensagens se perdem, o que os programas comuns
//! aguentam). O kernel tambem escuta nomes (`servir`): o servidor X11 do
//! MXOS mora assim, lendo e escrevendo os canais direto.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::rc::{Rc, Weak};
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::{RefCell, UnsafeCell};
use core::sync::atomic::{AtomicBool, Ordering};

use super::arquivos::Aberto;

/// Bytes que um canal guarda antes de quem escreve ter de esperar.
pub const TAMANHO_CANAL: usize = 1 << 20;

/// Um sentido da conversa: bytes e os descritores que vieram com eles.
#[derive(Default)]
pub struct Canal {
    pub dados: VecDeque<u8>,
    /// (posicao no fluxo, descritores): entregues a quem le aquele byte.
    pub fds: VecDeque<(u64, Vec<Aberto>)>,
    /// Bytes ja tirados do canal desde o comeco (posicao do primeiro).
    pub lidos: u64,
    /// Quem escreve fechou: depois dos dados vem o fim (read = 0).
    pub fechado: bool,
    /// Quem le fechou: escrever da EPIPE.
    pub sem_leitor: bool,
}

impl Canal {
    pub fn escrever(&mut self, bytes: &[u8]) -> usize {
        let n = bytes.len().min(TAMANHO_CANAL.saturating_sub(self.dados.len()));
        self.dados.extend(&bytes[..n]);
        avisar();
        n
    }

    /// Tira ate `maximo` bytes e os descritores que vieram com eles.
    pub fn ler(&mut self, maximo: usize) -> (Vec<u8>, Vec<Aberto>) {
        let n = maximo.min(self.dados.len());
        let dados: Vec<u8> = self.dados.drain(..n).collect();
        let fim = self.lidos + n as u64;
        let mut fds = Vec::new();
        while self.fds.front().is_some_and(|(pos, _)| *pos < fim) {
            fds.extend(self.fds.pop_front().unwrap().1);
        }
        self.lidos = fim;
        if n > 0 {
            avisar();
        }
        (dados, fds)
    }

    /// Posicao (no fluxo) do proximo byte escrito.
    pub fn fim(&self) -> u64 {
        self.lidos + self.dados.len() as u64
    }
}

pub type Lado = Rc<RefCell<Canal>>;

/// Uma ponta de uma conexao: le de `entrada`, escreve em `saida`.
pub struct Ponta {
    pub entrada: Lado,
    pub saida: Lado,
    /// Na ponta de quem aceitou: o nome do programa que conectou (o
    /// servidor X11 usa como titulo quando a janela nao tem um).
    pub quem: String,
}

impl Ponta {
    /// Um par ligado: (uma ponta, a outra).
    pub fn par() -> (Ponta, Ponta) {
        let a: Lado = Rc::new(RefCell::new(Canal::default()));
        let b: Lado = Rc::new(RefCell::new(Canal::default()));
        (Ponta { entrada: a.clone(), saida: b.clone(), quem: String::new() }, Ponta { entrada: b, saida: a, quem: String::new() })
    }

    pub fn legivel(&self) -> bool {
        let e = self.entrada.borrow();
        !e.dados.is_empty() || e.fechado
    }

    pub fn gravavel(&self) -> bool {
        let s = self.saida.borrow();
        s.dados.len() < TAMANHO_CANAL || s.sem_leitor
    }

    /// O outro lado foi embora.
    pub fn desligada(&self) -> bool {
        self.entrada.borrow().fechado
    }
}

impl Drop for Ponta {
    fn drop(&mut self) {
        self.saida.borrow_mut().fechado = true;
        let mut e = self.entrada.borrow_mut();
        e.sem_leitor = true;
        // descritores que ninguem vai ler: soltos ja (um soquete preso
        // num canal morto nunca fecharia)
        e.fds.clear();
        avisar();
    }
}

/// Fila de quem conectou e ainda nao foi aceito.
#[derive(Default)]
pub struct Escuta {
    pub fila: VecDeque<Ponta>,
}

pub enum Unix {
    /// Recem criado (com o nome do bind, se houve).
    Novo { nome: Option<String> },
    Escutando { nome: String, escuta: Rc<RefCell<Escuta>> },
    Conectado { ponta: Ponta, outro: Option<String> },
}

impl Drop for Unix {
    fn drop(&mut self) {
        if let Unix::Escutando { nome, .. } = self {
            nomes().remove(nome.as_str());
        }
    }
}

// ---------------------------------------------------------------------
// Nomes

/// Nome -> fila de quem escuta. So o laco principal mexe (como a tabela
/// de processos), entao basta um UnsafeCell.
struct Nomes(UnsafeCell<BTreeMap<String, Weak<RefCell<Escuta>>>>);

unsafe impl Sync for Nomes {}

static NOMES: Nomes = Nomes(UnsafeCell::new(BTreeMap::new()));

fn nomes() -> &'static mut BTreeMap<String, Weak<RefCell<Escuta>>> {
    unsafe { &mut *NOMES.0.get() }
}

/// Algo mudou num canal: quem espera deve tentar de novo.
static MEXEU: AtomicBool = AtomicBool::new(false);

pub fn avisar() {
    MEXEU.store(true, Ordering::Relaxed);
}

/// Houve mudanca desde a ultima pergunta?
pub fn mexeu() -> bool {
    MEXEU.swap(false, Ordering::Relaxed)
}

/// Passa a escutar `nome`. Err se alguem ja escuta.
pub fn escutar(nome: &str) -> Result<Rc<RefCell<Escuta>>, ()> {
    if nomes().get(nome).is_some_and(|w| w.strong_count() > 0) {
        return Err(());
    }
    let escuta = Rc::new(RefCell::new(Escuta::default()));
    nomes().insert(String::from(nome), Rc::downgrade(&escuta));
    Ok(escuta)
}

/// O kernel passa a atender `nome` (para sempre).
pub fn servir(nome: &str) -> Rc<RefCell<Escuta>> {
    escutar(nome).unwrap_or_else(|_| Rc::new(RefCell::new(Escuta::default())))
}

/// Conecta em `nome`: a ponta do cliente (a outra fica na fila de quem
/// escuta). None = ninguem escuta esse nome.
pub fn conectar(nome: &str, quem: &str) -> Option<Ponta> {
    let escuta = nomes().get(nome)?.upgrade()?;
    let (cliente, mut servidor) = Ponta::par();
    servidor.quem = String::from(quem);
    escuta.borrow_mut().fila.push_back(servidor);
    avisar();
    Some(cliente)
}

/// struct sockaddr_un -> nome ("@..." para os abstratos, como o ss mostra).
pub fn nome_do_endereco(b: &[u8]) -> Option<String> {
    let caminho = b.get(2..)?;
    if caminho.first() == Some(&0) {
        let resto = &caminho[1..];
        return Some(alloc::format!("@{}", String::from_utf8_lossy(resto)));
    }
    let fim = caminho.iter().position(|&c| c == 0).unwrap_or(caminho.len());
    if fim == 0 {
        return None;
    }
    Some(String::from_utf8_lossy(&caminho[..fim]).into_owned())
}

/// O contrario: struct sockaddr_un para `nome` (vazio = sem nome).
pub fn endereco_do_nome(nome: Option<&str>) -> Vec<u8> {
    let mut s = Vec::from(1u16.to_le_bytes());
    match nome {
        Some(n) if n.starts_with('@') => {
            s.push(0);
            s.extend_from_slice(&n.as_bytes()[1..]);
        }
        Some(n) => {
            s.extend_from_slice(n.as_bytes());
            s.push(0);
        }
        None => {}
    }
    s
}
