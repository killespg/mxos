//! Arquivos abertos dos processos: o que um descritor (fd) aponta.
//!
//!   Terminal          stdin/stdout/stderr, /dev/tty e /dev/console
//!   Nulo, Zero, Aleatorio   /dev/null, /dev/zero, /dev/urandom
//!   Disco             arquivo do FAT32: lido inteiro no open, gravado
//!                     inteiro quando o ultimo descritor fecha (ou no fsync)
//!   Pasta             a listagem, para o getdents64
//!   Tubo              pipe: um buffer com contagem de quem le e escreve
//!   Soquete           socket de rede (soquetes.rs)
//!
//! Um open cria uma Descricao (com a posicao); dup, fork e herancas so
//! copiam o Rc, como no Linux (a posicao e dividida).

use alloc::collections::VecDeque;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

use super::soquetes::Soquete;
use crate::armazenamento::{self, Erro};

// errno do Linux usados aqui e em chamadas.rs.
pub const ENOENT: i64 = 2;
pub const ESRCH: i64 = 3;
pub const EINTR: i64 = 4;
pub const EIO: i64 = 5;
pub const ENOEXEC: i64 = 8;
pub const EBADF: i64 = 9;
pub const ECHILD: i64 = 10;
pub const EAGAIN: i64 = 11;
pub const ENOMEM: i64 = 12;
pub const EACCES: i64 = 13;
pub const EFAULT: i64 = 14;
pub const EEXIST: i64 = 17;
pub const ENOTDIR: i64 = 20;
pub const EISDIR: i64 = 21;
pub const EINVAL: i64 = 22;
pub const EMFILE: i64 = 24;
pub const ENOTTY: i64 = 25;
pub const ENOSPC: i64 = 28;
pub const ESPIPE: i64 = 29;
pub const EPIPE: i64 = 32;
pub const ERANGE: i64 = 34;
pub const ENAMETOOLONG: i64 = 36;
pub const ENOSYS: i64 = 38;
pub const ENOTEMPTY: i64 = 39;
/// Biblioteca (ou interpretador) que falta ou esta estragada.
pub const ELIBBAD: i64 = 80;

pub fn errno(e: Erro) -> i64 {
    match e {
        Erro::NaoAchado | Erro::SemDisco => ENOENT,
        Erro::JaExiste => EEXIST,
        Erro::NaoEPasta => ENOTDIR,
        Erro::EPasta => EISDIR,
        Erro::PastaNaoVazia => ENOTEMPTY,
        Erro::Cheio => ENOSPC,
        Erro::NomeInvalido => EINVAL,
        Erro::Disco | Erro::Corrompido(_) => EIO,
    }
}

pub struct Tubo {
    pub buffer: VecDeque<u8>,
    pub leitores: usize,
    pub escritores: usize,
}

/// Bytes que um tubo guarda antes de quem escreve ter de esperar.
pub const TAMANHO_TUBO: usize = 64 * 1024;

pub struct ArquivoDisco {
    pub caminho: String,
    pub dados: Vec<u8>,
    pub sujo: bool,
}

pub struct ItemDePasta {
    pub nome: String,
    pub pasta: bool,
}

pub enum Tipo {
    Terminal,
    Nulo,
    Zero,
    Aleatorio,
    Disco(ArquivoDisco),
    Pasta { caminho: String, itens: Vec<ItemDePasta> },
    /// O tubo e o lado (true = escrita).
    Tubo(Rc<RefCell<Tubo>>, bool),
    /// Arquivo feito na hora pelo kernel (/proc): so leitura.
    Gerado(Vec<u8>),
    Soquete(Soquete),
}

pub struct Descricao {
    pub tipo: Tipo,
    pub posicao: usize,
    /// Flags do open (O_APPEND, O_NONBLOCK, modo de acesso).
    pub flags: u32,
}

pub type Aberto = Rc<RefCell<Descricao>>;

pub const O_ACESSO: u32 = 3;
pub const O_RDONLY: u32 = 0;
pub const O_RDWR: u32 = 2;
pub const O_CREAT: u32 = 0o100;
pub const O_EXCL: u32 = 0o200;
pub const O_TRUNC: u32 = 0o1000;
pub const O_APPEND: u32 = 0o2000;
pub const O_NONBLOCK: u32 = 0o4000;
pub const O_DIRECTORY: u32 = 0o40000;
pub const O_CLOEXEC: u32 = 0o2000000;

impl Descricao {
    pub fn nova(tipo: Tipo, flags: u32) -> Aberto {
        Rc::new(RefCell::new(Descricao { tipo, posicao: 0, flags }))
    }

    /// Grava no disco o que mudou (fsync, ou o ultimo close).
    pub fn sincronizar(&mut self) -> Result<(), i64> {
        if let Tipo::Disco(a) = &mut self.tipo {
            if a.sujo {
                armazenamento::escrever(&a.caminho, &a.dados).map_err(errno)?;
                a.sujo = false;
            }
        }
        Ok(())
    }

    pub fn caminho(&self) -> Option<&str> {
        match &self.tipo {
            Tipo::Disco(a) => Some(&a.caminho),
            Tipo::Pasta { caminho, .. } => Some(caminho),
            _ => None,
        }
    }
}

impl Drop for Descricao {
    fn drop(&mut self) {
        let _ = self.sincronizar();
        if let Tipo::Tubo(tubo, escrita) = &self.tipo {
            let mut t = tubo.borrow_mut();
            if *escrita {
                t.escritores -= 1;
            } else {
                t.leitores -= 1;
            }
        }
    }
}

/// Um tubo novo: (lado de leitura, lado de escrita).
pub fn tubo(flags: u32) -> (Aberto, Aberto) {
    let t = Rc::new(RefCell::new(Tubo { buffer: VecDeque::new(), leitores: 1, escritores: 1 }));
    (Descricao::nova(Tipo::Tubo(t.clone(), false), flags), Descricao::nova(Tipo::Tubo(t, true), flags | 1))
}

// ---------------------------------------------------------------------
// Caminhos

/// Junta `caminho` a pasta atual e resolve "." e "..". Sempre absoluto,
/// sem barra no fim (a raiz e "/").
pub fn absoluto(atual: &str, caminho: &str) -> String {
    let mut partes: Vec<&str> = Vec::new();
    let base = if caminho.starts_with('/') { "" } else { atual };
    for parte in base.split('/').chain(caminho.split('/')) {
        match parte {
            "" | "." => {}
            ".." => {
                partes.pop();
            }
            p => partes.push(p),
        }
    }
    let mut saida = String::new();
    for p in partes {
        saida.push('/');
        saida.push_str(p);
    }
    if saida.is_empty() {
        saida.push('/');
    }
    saida
}

/// Arquivos de dispositivo que nao estao no disco.
pub fn dispositivo(caminho: &str) -> Option<Tipo> {
    Some(match caminho {
        "/dev/null" => Tipo::Nulo,
        "/dev/zero" => Tipo::Zero,
        "/dev/urandom" | "/dev/random" => Tipo::Aleatorio,
        "/dev/tty" | "/dev/console" | "/dev/tty0" | "/dev/ttyS0" => Tipo::Terminal,
        _ => return None,
    })
}

/// Pastas que existem mesmo sem estar no disco.
fn pasta_virtual(caminho: &str) -> bool {
    matches!(caminho, "/" | "/dev" | "/proc")
}

/// O que ha em `caminho`: Some(pasta, tamanho). None = nao existe.
pub fn info(caminho: &str) -> Result<(bool, u64), i64> {
    if pasta_virtual(caminho) {
        return Ok((true, 0));
    }
    if dispositivo(caminho).is_some() {
        return Ok((false, 0));
    }
    let (pai, nome) = match caminho.rfind('/') {
        Some(0) => ("/", &caminho[1..]),
        Some(i) => (&caminho[..i], &caminho[i + 1..]),
        None => return Err(ENOENT),
    };
    let itens = armazenamento::listar(pai).map_err(errno)?;
    let minusculo = nome.to_lowercase();
    itens
        .iter()
        .find(|i| i.nome.to_lowercase() == minusculo)
        .map(|i| (i.pasta, i.tamanho))
        .ok_or(ENOENT)
}

/// Abre `caminho` (ja absoluto). `flags` como no Linux.
pub fn abrir(caminho: &str, flags: u32) -> Result<Aberto, i64> {
    if let Some(tipo) = dispositivo(caminho) {
        return Ok(Descricao::nova(tipo, flags));
    }
    let escreve = flags & O_ACESSO != O_RDONLY;
    match info(caminho) {
        Ok((true, _)) => {
            if escreve {
                return Err(EISDIR);
            }
            Ok(Descricao::nova(Tipo::Pasta { caminho: caminho.into(), itens: listar(caminho)? }, flags))
        }
        Ok((false, _)) => {
            if flags & O_CREAT != 0 && flags & O_EXCL != 0 {
                return Err(EEXIST);
            }
            if flags & O_DIRECTORY != 0 {
                return Err(ENOTDIR);
            }
            let truncar = flags & O_TRUNC != 0 && escreve;
            let dados = if truncar { Vec::new() } else { armazenamento::ler(caminho).map_err(errno)? };
            let arquivo = ArquivoDisco { caminho: caminho.into(), dados, sujo: truncar };
            Ok(Descricao::nova(Tipo::Disco(arquivo), flags))
        }
        Err(ENOENT) if flags & O_CREAT != 0 => {
            // cria ja vazio, para o arquivo existir mesmo antes do close
            armazenamento::escrever(caminho, &[]).map_err(errno)?;
            let arquivo = ArquivoDisco { caminho: caminho.into(), dados: Vec::new(), sujo: false };
            Ok(Descricao::nova(Tipo::Disco(arquivo), flags))
        }
        Err(e) => Err(e),
    }
}

fn listar(caminho: &str) -> Result<Vec<ItemDePasta>, i64> {
    let mut itens = alloc::vec![
        ItemDePasta { nome: ".".into(), pasta: true },
        ItemDePasta { nome: "..".into(), pasta: true },
    ];
    if caminho == "/dev" {
        for nome in ["null", "zero", "urandom", "random", "tty", "console"] {
            itens.push(ItemDePasta { nome: nome.into(), pasta: false });
        }
        return Ok(itens);
    }
    let lista = armazenamento::listar(caminho).map_err(errno)?;
    itens.extend(lista.into_iter().map(|i| ItemDePasta { nome: i.nome, pasta: i.pasta }));
    for virtual_ in ["dev", "proc"] {
        if caminho == "/" && !itens.iter().any(|i| i.nome.eq_ignore_ascii_case(virtual_)) {
            itens.push(ItemDePasta { nome: virtual_.into(), pasta: true });
        }
    }
    Ok(itens)
}

/// Numero de "inode" para um nome (o FAT32 nao tem): um hash que nunca e 0.
pub fn inode(caminho: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in caminho.to_lowercase().bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
    }
    h | 1
}
