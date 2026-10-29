//! Processos: programas rodando em EL0, cada um com a propria memoria.
//!
//! O kernel continua com um laco principal so (a area de trabalho ou o
//! shell da serial). A cada volta ele chama `rodar`, que da a CPU aos
//! processos prontos ate o proximo tique do timer (10 ms):
//!
//!   entrar_usuario (vetores.s) salva o kernel e faz eret para o programa;
//!   qualquer excecao vinda dele (SVC, interrupcao, erro) salva o programa
//!   no Contexto e volta para o kernel como se a funcao tivesse retornado.
//!
//! Assim as chamadas de sistema rodam no kernel normal (podem alocar, ler
//! o disco, esperar), e uma interrupcao do timer no meio do programa e a
//! preempcao: o processo volta para a fila e o laco principal segue.
//!
//! Um processo que erra (acesso invalido, instrucao desconhecida) morre
//! sozinho, com uma mensagem no terminal; o kernel continua.
//!
//! Chamada que precisa esperar (ler do teclado, dormir, esperar um filho)
//! volta o PC para o SVC e marca o processo como esperando: quando a
//! condicao muda, ele roda de novo e repete a chamada.

pub mod chamadas;
pub mod memoria;

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::arch::global_asm;
use core::cell::UnsafeCell;
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::{cpu, mmu, timer};
use memoria::{Espaco, PAGINA, Permissao};

/// Onde o programa de teste embutido e carregado.
const BASE_TESTE: usize = 0x40_0000;
/// Pilha dos programas: o topo fica no fim da area do programa.
pub const TOPO_PILHA: usize = memoria::FIM_USUARIO - PAGINA;
pub const TAMANHO_PILHA: usize = 256 * 1024;
/// Saida guardada enquanto ninguem le (o terminal fechado, por exemplo).
const MAX_SAIDA: usize = 64 * 1024;
const MAX_PROCESSOS: usize = 64;
/// ASIDs de 8 bits; o 0 e do kernel.
const MAX_ASID: u16 = 255;

// ---------------------------------------------------------------------
// Contexto (o mesmo layout que o vetores.s usa)

#[repr(C, align(16))]
pub struct Contexto {
    pub x: [u64; 31],
    pub sp: u64,
    pub pc: u64,
    pub spsr: u64,
    pub tpidr: u64,
    pub fpcr: u64,
    pub fpsr: u64,
    kernel: u64,
    pub esr: u64,
    pub far: u64,
    pub q: [u128; 32],
}

#[repr(C)]
struct ContextoKernel {
    x: [u64; 12],
    sp: u64,
    d: [u64; 8],
    fpcr: u64,
}

const _: () = assert!(core::mem::offset_of!(Contexto, sp) == 248);
const _: () = assert!(core::mem::offset_of!(Contexto, kernel) == 296);
const _: () = assert!(core::mem::offset_of!(Contexto, q) == 320);
const _: () = assert!(core::mem::offset_of!(ContextoKernel, fpcr) == 168);

unsafe extern "C" {
    fn entrar_usuario(contexto: *mut Contexto, kernel: *mut ContextoKernel) -> u64;
}

impl Contexto {
    /// Programa novo: comeca em `entrada` com a pilha em `sp`, em EL0
    /// com as interrupcoes liberadas (SPSR = 0).
    pub fn novo(entrada: usize, sp: usize) -> Box<Contexto> {
        let mut c: Box<Contexto> = unsafe { Box::new_zeroed().assume_init() };
        c.pc = entrada as u64;
        c.sp = sp as u64;
        c
    }

    #[allow(dead_code)] // o fork (clone) do passo 4 usa
    pub fn copia(&self) -> Box<Contexto> {
        let mut c: Box<Contexto> = unsafe { Box::new_zeroed().assume_init() };
        c.x = self.x;
        c.sp = self.sp;
        c.pc = self.pc;
        c.spsr = self.spsr;
        c.tpidr = self.tpidr;
        c.fpcr = self.fpcr;
        c.fpsr = self.fpsr;
        c.q = self.q;
        c
    }
}

// ---------------------------------------------------------------------
// Processos

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Estado {
    Pronto,
    /// Dorme ate o contador chegar neste valor.
    Dormindo(u64),
    /// Esperando o terminal mandar uma linha (ou uma tecla).
    Lendo,
    /// Esperando um filho terminar (wait4).
    EsperandoFilho,
    /// Ja terminou, com este codigo; espera o pai ler (wait4).
    Terminado(i32),
}

impl Estado {
    pub fn nome(&self) -> &'static str {
        match self {
            Estado::Pronto => "rodando",
            Estado::Dormindo(_) => "dormindo",
            Estado::Lendo => "lendo o teclado",
            Estado::EsperandoFilho => "esperando filho",
            Estado::Terminado(_) => "terminado",
        }
    }
}

pub struct Processo {
    pub pid: u32,
    /// 0 = criado pelo kernel (shell).
    pub pai: u32,
    pub nome: String,
    pub estado: Estado,
    pub contexto: Box<Contexto>,
    pub espaco: Espaco,
    /// Ticks do contador rodando em EL0.
    pub tempo_cpu: u64,
    /// Estado das chamadas de sistema do Linux (arquivos, brk...).
    pub linux: chamadas::Linux,
}

/// Fotografia de um processo (comando "processos", app Tarefas).
#[derive(Clone)]
pub struct Info {
    pub pid: u32,
    pub pai: u32,
    pub nome: String,
    pub estado: &'static str,
    pub tempo_cpu: u64,
    pub memoria: usize,
}

// ---------------------------------------------------------------------
// Terminal dos processos (stdin)

/// Disciplina de linha do terminal: no modo canonico (o padrao) junta a
/// linha com eco e Backspace e so entrega no Enter; no modo cru (um
/// programa pediu pelo ioctl) cada tecla vai direto.
pub struct Tty {
    linha: Vec<u8>,
    pub prontos: VecDeque<u8>,
    pub canonico: bool,
    pub eco: bool,
    /// Ctrl+D numa linha vazia: o proximo read devolve 0.
    pub fim: bool,
}

impl Tty {
    const fn novo() -> Tty {
        Tty { linha: Vec::new(), prontos: VecDeque::new(), canonico: true, eco: true, fim: false }
    }
}

// ---------------------------------------------------------------------
// Tabela de processos

pub struct Tabela {
    pub processos: Vec<Box<Processo>>,
    proximo_pid: u32,
    /// Proximo da fila (round-robin).
    vez: usize,
    saida: VecDeque<u8>,
    pub tty: Tty,
    /// Processo que recebe o teclado do terminal (o shell espera por ele).
    primeiro_plano: Option<u32>,
    /// Processos do shell que ja terminaram: (pid, codigo).
    terminados: Vec<(u32, i32)>,
    /// Chamadas de sistema que ainda nao existem: numero -> vezes.
    pub desconhecidas: BTreeMap<u64, u32>,
}

struct Global {
    usando: AtomicBool,
    tabela: UnsafeCell<Tabela>,
}

// So o laco principal usa (nunca uma interrupcao); `usando` pega reentrada.
unsafe impl Sync for Global {}

static GLOBAL: Global = Global {
    usando: AtomicBool::new(false),
    tabela: UnsafeCell::new(Tabela {
        processos: Vec::new(),
        proximo_pid: 1,
        vez: 0,
        saida: VecDeque::new(),
        tty: Tty::novo(),
        primeiro_plano: None,
        terminados: Vec::new(),
        desconhecidas: BTreeMap::new(),
    }),
};

fn com<R>(acao: impl FnOnce(&mut Tabela) -> R) -> R {
    assert!(!GLOBAL.usando.swap(true, Ordering::Acquire), "processos: tabela usada de dentro dela mesma");
    let resultado = acao(unsafe { &mut *GLOBAL.tabela.get() });
    GLOBAL.usando.store(false, Ordering::Release);
    resultado
}

impl Tabela {
    /// O que os processos escrevem no terminal.
    pub fn escrever(&mut self, dados: &[u8]) {
        self.saida.extend(dados);
        while self.saida.len() > MAX_SAIDA {
            self.saida.pop_front();
        }
    }

    pub fn indice(&self, pid: u32) -> Option<usize> {
        self.processos.iter().position(|p| p.pid == pid)
    }

    /// Um ASID que nenhum processo vivo usa.
    fn asid_livre(&self) -> Option<u16> {
        (1..=MAX_ASID).find(|&a| self.processos.iter().all(|p| p.espaco.asid != a))
    }

    /// Poe um processo novo na tabela. Devolve o pid.
    pub fn acrescentar(&mut self, pai: u32, nome: String, contexto: Box<Contexto>, espaco: Espaco) -> u32 {
        let pid = self.proximo_pid;
        self.proximo_pid += 1;
        let linux = chamadas::Linux::novo();
        let p = Processo { pid, pai, nome, estado: Estado::Pronto, contexto, espaco, tempo_cpu: 0, linux };
        self.processos.push(Box::new(p));
        pid
    }

    /// Espaco vazio para um processo novo (None: processos demais).
    pub fn novo_espaco(&self) -> Option<Espaco> {
        if self.processos.len() >= MAX_PROCESSOS {
            return None;
        }
        self.asid_livre().map(Espaco::novo)
    }

    /// O processo `i` terminou com `codigo`: avisa o pai, entrega os filhos
    /// ao kernel e devolve a memoria.
    pub fn terminar(&mut self, i: usize, codigo: i32) {
        let pid = self.processos[i].pid;
        let pai = self.processos[i].pai;
        for filho in self.processos.iter_mut().filter(|p| p.pai == pid) {
            filho.pai = 0;
        }
        if self.primeiro_plano == Some(pid) {
            self.primeiro_plano = None;
            // o terminal volta ao normal para o shell
            self.tty = Tty::novo();
        }
        if pai == 0 {
            // ninguem vai fazer wait4: sai da tabela ja
            self.terminados.push((pid, codigo));
            self.processos.remove(i);
            return;
        }
        let p = &mut self.processos[i];
        p.estado = Estado::Terminado(codigo);
        // a memoria vai embora logo; so fica o registro para o pai
        p.espaco = Espaco::novo(p.espaco.asid);
        p.linux = chamadas::Linux::novo();
        if let Some(j) = self.indice(pai) {
            if self.processos[j].estado == Estado::EsperandoFilho {
                self.processos[j].estado = Estado::Pronto;
            }
        }
    }

    /// Mata um processo por um sinal (codigo 128 + sinal, como no shell).
    pub fn matar(&mut self, pid: u32, sinal: i32) -> bool {
        let Some(i) = self.indice(pid) else { return false };
        if matches!(self.processos[i].estado, Estado::Terminado(_)) {
            return false;
        }
        self.terminar(i, 128 + sinal);
        true
    }

    /// Acorda quem dorme e ja passou da hora, e quem le e ja tem entrada.
    fn acordar(&mut self) {
        let agora = cpu::contador();
        let tem_entrada = !self.tty.prontos.is_empty() || self.tty.fim;
        for p in self.processos.iter_mut() {
            match p.estado {
                Estado::Dormindo(alvo) if agora >= alvo => p.estado = Estado::Pronto,
                Estado::Lendo if tem_entrada => p.estado = Estado::Pronto,
                _ => {}
            }
        }
    }

    /// O proximo pronto da fila, a partir da vez.
    fn proximo_pronto(&mut self) -> Option<usize> {
        let n = self.processos.len();
        for k in 0..n {
            let i = (self.vez + k) % n;
            if self.processos[i].estado == Estado::Pronto {
                return Some(i);
            }
        }
        None
    }
}

// ---------------------------------------------------------------------
// Rodar

/// Motivos que o vetores.s devolve.
const SINCRONA: u64 = 0;
const IRQ: u64 = 1;

/// Da a CPU aos processos prontos ate o proximo tique do timer (ou ate
/// nenhum estar pronto). Devolve true se algum processo escreveu, mudou de
/// estado ou terminou (o terminal precisa olhar).
pub fn rodar() -> bool {
    com(|t| {
        let tique = timer::tiques();
        let mut novidade = false;
        t.acordar();
        while let Some(i) = t.proximo_pronto() {
            let motivo = executar(&mut t.processos[i]);
            if motivo == SINCRONA {
                novidade |= tratar_excecao(t, i);
            } else if motivo != IRQ {
                // FIQ ou SError vindos do programa: nao deviam acontecer
                let pid = t.processos[i].pid;
                let _ = writeln!(Saida(t), "\n[processo {pid}: erro assíncrono da CPU]");
                t.terminar(i, 128 + 7);
                novidade = true;
            }
            if timer::tiques() != tique {
                // a fatia acabou: o proximo da fila comeca na proxima volta
                t.vez = t.vez.wrapping_add(1);
                break;
            }
        }
        novidade || !t.saida.is_empty()
    })
}

/// Algum processo quer a CPU agora (o laco principal nao deve dormir).
pub fn tem_prontos() -> bool {
    com(|t| {
        t.acordar();
        t.processos.iter().any(|p| p.estado == Estado::Pronto)
    })
}

/// Roda o processo ate ele voltar para o kernel. Devolve o motivo.
fn executar(p: &mut Processo) -> u64 {
    let mut kernel = ContextoKernel { x: [0; 12], sp: 0, d: [0; 8], fpcr: 0 };
    cpu::desligar_interrupcoes();
    mmu::usar_espaco(p.espaco.raiz(), p.espaco.asid);
    let antes = cpu::contador();
    let motivo = unsafe { entrar_usuario(&mut *p.contexto, &mut kernel) };
    p.tempo_cpu += cpu::contador().wrapping_sub(antes);
    // as tabelas do processo podem sumir: o kernel volta para as dele
    mmu::usar_espaco_do_kernel();
    // a interrupcao que tirou o programa da CPU e atendida aqui
    cpu::ligar_interrupcoes();
    motivo
}

/// Excecao sincrona do programa: chamada de sistema ou erro.
fn tratar_excecao(t: &mut Tabela, i: usize) -> bool {
    let esr = t.processos[i].contexto.esr;
    let classe = esr >> 26 & 0x3F;
    if classe == 0x15 {
        return chamadas::atender(t, i);
    }
    let p = &t.processos[i];
    let (pid, nome, pc, far) = (p.pid, p.nome.clone(), p.contexto.pc, p.contexto.far);
    let motivo = match classe {
        0x24 => {
            let acao = if esr >> 6 & 1 == 1 { "escrever" } else { "ler" };
            format!("falha de segmentação ao {acao} 0x{far:x}")
        }
        0x20 => format!("tentou executar 0x{far:x}, que não é código dele"),
        0x22 => String::from("PC desalinhado"),
        0x26 => String::from("pilha desalinhada"),
        0x00 => String::from("instrução desconhecida"),
        0x3C => String::from("ponto de parada (brk)"),
        _ => format!("exceção da classe 0x{classe:02x}"),
    };
    let _ = writeln!(Saida(t), "\n[processo {pid} ({nome}) morreu: {motivo}, no PC 0x{pc:x}]");
    // SIGSEGV (11) ou SIGILL (4), como o shell do Linux mostraria
    let sinal = if matches!(classe, 0x24 | 0x20) { 11 } else { 4 };
    t.terminar(i, 128 + sinal);
    true
}

/// `write!` na saida dos processos.
pub struct Saida<'a>(pub &'a mut Tabela);

impl core::fmt::Write for Saida<'_> {
    fn write_str(&mut self, texto: &str) -> core::fmt::Result {
        self.0.escrever(texto.as_bytes());
        Ok(())
    }
}

// ---------------------------------------------------------------------
// Terminal

/// Tira o que os processos escreveram (o shell manda para a tela).
pub fn tirar_saida() -> Vec<u8> {
    com(|t| t.saida.drain(..).collect())
}

/// Codigo de saida de um processo do shell, se ja terminou.
pub fn terminou(pid: u32) -> Option<i32> {
    com(|t| {
        let i = t.terminados.iter().position(|&(p, _)| p == pid)?;
        Some(t.terminados.remove(i).1)
    })
}

/// Uma tecla do terminal para o processo em primeiro plano.
pub fn tecla(byte: u8) {
    com(|t| {
        let Some(pid) = t.primeiro_plano else { return };
        match byte {
            // Ctrl+C: SIGINT
            3 => {
                t.escrever(b"^C\n");
                t.matar(pid, 2);
            }
            _ if !t.tty.canonico => {
                t.tty.prontos.push_back(byte);
            }
            // Ctrl+D: fim de arquivo (ou entrega a linha sem o Enter)
            4 => {
                if t.tty.linha.is_empty() {
                    t.tty.fim = true;
                } else {
                    let linha: Vec<u8> = t.tty.linha.drain(..).collect();
                    t.tty.prontos.extend(linha);
                }
            }
            b'\r' | b'\n' => {
                let mut linha: Vec<u8> = t.tty.linha.drain(..).collect();
                linha.push(b'\n');
                t.tty.prontos.extend(linha);
                if t.tty.eco {
                    t.escrever(b"\n");
                }
            }
            8 | 127 => {
                // apaga o caractere inteiro (UTF-8)
                let mut apagou = false;
                while let Some(b) = t.tty.linha.pop() {
                    apagou = true;
                    if b & 0xC0 != 0x80 {
                        break;
                    }
                }
                if apagou && t.tty.eco {
                    t.escrever(b"\x08 \x08");
                }
            }
            _ => {
                t.tty.linha.push(byte);
                if t.tty.eco {
                    t.escrever(&[byte]);
                }
            }
        }
    })
}

// ---------------------------------------------------------------------
// Lista, matar, programa de teste

pub fn lista() -> Vec<Info> {
    com(|t| {
        t.processos
            .iter()
            .map(|p| Info {
                pid: p.pid,
                pai: p.pai,
                nome: p.nome.clone(),
                estado: p.estado.nome(),
                tempo_cpu: p.tempo_cpu,
                memoria: p.espaco.bytes(),
            })
            .collect()
    })
}

/// Mata o processo `pid` (SIGKILL). false se ele nao existe.
pub fn matar(pid: u32) -> bool {
    com(|t| t.matar(pid, 9))
}

// O programa de teste: codigo de maquina AArch64 escrito aqui mesmo,
// independente de posicao. Recebe o modo em x0:
//   0 "ola": escreve 5 linhas, uma a cada meio segundo
//   1 "conta": gasta CPU contando (mostra a preempcao) e escreve pontos
//   2 "falha": escreve no endereco 0x10 (o processo morre, o kernel nao)
//   3 "eco": le linhas do teclado e devolve em maiusculas ate "fim"
global_asm!(
    r#"
.section .rodata.programa_teste, "a"
.balign 16
.global programa_teste_inicio, programa_teste_fim
programa_teste_inicio:
    cmp     x0, #1
    b.eq    2f
    cmp     x0, #2
    b.eq    4f
    cmp     x0, #3
    b.eq    5f
    // ola
    mov     x19, #5
1:  mov     x0, #1
    adr     x1, 9f
    mov     x2, #(10f - 9f)
    mov     x8, #64             // write
    svc     #0
    adr     x0, 11f
    mov     x1, #0
    mov     x8, #101            // nanosleep
    svc     #0
    subs    x19, x19, #1
    b.ne    1b
    mov     x0, #0
    b       8f
    // conta
2:  mov     x19, #20
3:  movz    x20, #0x1000, lsl #16
31: subs    x20, x20, #1
    b.ne    31b
    mov     x0, #1
    adr     x1, 12f
    mov     x2, #1
    mov     x8, #64
    svc     #0
    subs    x19, x19, #1
    b.ne    3b
    mov     x0, #1
    adr     x1, 13f
    mov     x2, #1
    mov     x8, #64
    svc     #0
    mov     x0, #0
    b       8f
    // falha
4:  mov     x1, #0x10
    str     x1, [x1]
    mov     x0, #1
    b       8f
    // eco: le ate 200 bytes na pilha, troca a-z por A-Z e escreve
5:  sub     sp, sp, #256
6:  mov     x0, #0
    mov     x1, sp
    mov     x2, #200
    mov     x8, #63             // read
    svc     #0
    cmp     x0, #0
    b.le    7f
    mov     x21, x0
    // "fim\n" encerra
    cmp     x21, #4
    b.ne    61f
    ldr     w3, [sp]
    movz    w4, #0x6966
    movk    w4, #0x0a6d, lsl #16
    cmp     w3, w4
    b.eq    7f
61: mov     x3, #0
62: ldrb    w4, [sp, x3]
    sub     w5, w4, #97         // 'a'
    cmp     w5, #25
    b.hi    63f
    sub     w4, w4, #32
    strb    w4, [sp, x3]
63: add     x3, x3, #1
    cmp     x3, x21
    b.lo    62b
    mov     x0, #1
    mov     x1, sp
    mov     x2, x21
    mov     x8, #64
    svc     #0
    b       6b
7:  add     sp, sp, #256
    mov     x0, #0
8:  mov     x8, #93             // exit
    svc     #0
    b       8b
9:  .ascii  "Olá! Sou um processo em EL0, falando pela chamada write.\n"
10:
12: .ascii  "."
13: .ascii  "\n"
.balign 8
11: .quad   0, 500000000        // meio segundo
programa_teste_fim:
.text
"#
);

unsafe extern "C" {
    static programa_teste_inicio: u8;
    static programa_teste_fim: u8;
}

/// Os modos do programa de teste, na ordem do x0.
pub const MODOS_TESTE: [&str; 4] = ["ola", "conta", "falha", "eco"];

/// Cria um processo com o programa de teste. `primeiro_plano`: fica com o
/// teclado do terminal ate terminar.
pub fn criar_teste(modo: &str, primeiro_plano: bool) -> Result<u32, &'static str> {
    let numero = MODOS_TESTE.iter().position(|&m| m == modo).ok_or("modo desconhecido")?;
    let inicio = &raw const programa_teste_inicio as usize;
    let fim = &raw const programa_teste_fim as usize;
    let codigo = unsafe { core::slice::from_raw_parts(inicio as *const u8, fim - inicio) };
    com(|t| {
        let mut espaco = t.novo_espaco().ok_or("processos demais")?;
        espaco.mapear_faixa(BASE_TESTE, BASE_TESTE + codigo.len(), Permissao::Codigo).map_err(|_| "sem memória")?;
        espaco.escrever_forcado(BASE_TESTE, codigo).map_err(|_| "sem memória")?;
        espaco.mapear_faixa(TOPO_PILHA - TAMANHO_PILHA, TOPO_PILHA, Permissao::Dados).map_err(|_| "sem memória")?;
        let mut contexto = Contexto::novo(BASE_TESTE, TOPO_PILHA);
        contexto.x[0] = numero as u64;
        let pid = t.acrescentar(0, format!("teste-{modo}"), contexto, espaco);
        if primeiro_plano {
            t.primeiro_plano = Some(pid);
        }
        Ok(pid)
    })
}

/// Comandos de shell dos processos. Devolve false se o nome nao e deles.
pub fn comando(nome: &str, args: &str, saida: &mut dyn Write) -> bool {
    match nome {
        "processos" | "ps" => listar(saida),
        "matar" | "kill" => match args.trim().parse::<u32>() {
            Ok(pid) if matar(pid) => {
                let _ = writeln!(saida, "processo {pid} finalizado");
            }
            Ok(pid) => {
                let _ = writeln!(saida, "matar: não há processo {pid}");
            }
            Err(_) => {
                let _ = writeln!(saida, "uso: matar <pid>   (veja os pids com \"processos\")");
            }
        },
        "syscalls" => {
            let desconhecidas = com(|t| t.desconhecidas.clone());
            if desconhecidas.is_empty() {
                let _ = writeln!(saida, "nenhum programa pediu uma chamada de sistema que o MXOS não tem");
            }
            for (numero, vezes) in desconhecidas {
                let _ = writeln!(saida, "  chamada {numero:>4} ({}): {vezes} vez(es), devolveu -ENOSYS", chamadas::nome(numero));
            }
        }
        _ => return false,
    }
    true
}

fn listar(saida: &mut dyn Write) {
    let lista = lista();
    if lista.is_empty() {
        let _ = writeln!(saida, "nenhum processo rodando (teste com: processo ola)");
        return;
    }
    let freq = cpu::frequencia_timer().max(1);
    let _ = writeln!(saida, "  PID  PAI  ESTADO            CPU (ms)  MEMÓRIA  NOME");
    for p in lista {
        let ms = p.tempo_cpu * 1000 / freq;
        let _ = writeln!(saida, "{:>5} {:>4}  {:<16} {:>9}  {:>5} KB  {}", p.pid, p.pai, p.estado, ms, p.memoria / 1024, p.nome);
    }
}
