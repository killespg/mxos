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
//! Chamada que precisa esperar (ler do teclado ou de um tubo, esperar um
//! filho) volta o PC para o SVC e marca o processo como esperando: a cada
//! volta do laco ele roda de novo e repete a chamada, ate ela andar. Quem
//! dorme (nanosleep) so volta quando passa a hora.
//!
//! Os programas sao de Linux aarch64 (chamadas.rs, elf.rs): estaticos,
//! lidos do disco FAT32 pelo comando "executar".

pub mod arquivos;
pub mod cache;
pub mod chamadas;
pub mod elf;
pub mod espera;
pub mod memoria;
pub mod proc;
pub mod shm;
pub mod sinais;
pub mod soquetes;
pub mod unix;

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::arch::global_asm;
use core::cell::UnsafeCell;
use core::fmt::Write;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::{armazenamento, cpu, mmu, timer};
use memoria::{Espaco, PAGINA, Permissao};

/// Onde o programa de teste embutido e carregado.
const BASE_TESTE: usize = 0x40_0000;
/// Pilha dos programas: o topo fica no fim da area do programa.
pub const TOPO_PILHA: usize = memoria::FIM_USUARIO - PAGINA;
pub const TAMANHO_PILHA: usize = 1024 * 1024;
/// Saida guardada enquanto ninguem le (o terminal fechado, por exemplo).
const MAX_SAIDA: usize = 64 * 1024;
const MAX_PROCESSOS: usize = 64;
/// ASIDs de 8 bits; o 0 e do kernel.
const MAX_ASID: u16 = 255;
/// Ambiente dos programas que o shell do MXOS roda.
const AMBIENTE: [&str; 7] = ["PATH=/bin:/usr/bin:/", "HOME=/", "TERM=vt100", "USER=root", "LANG=C.UTF-8", "PS1=\\w # ", "DISPLAY=:0"];

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

    /// Copia para um filho (fork).
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

/// O que as threads de um processo dividem (a memoria, os descritores).
/// So o laco principal mexe na tabela, uma chamada por vez, entao um Rc
/// com UnsafeCell basta.
pub struct Dividido<T>(Rc<UnsafeCell<T>>);

impl<T> Dividido<T> {
    pub fn novo(valor: T) -> Dividido<T> {
        Dividido(Rc::new(UnsafeCell::new(valor)))
    }

    /// Mais um dono do mesmo valor (uma thread nova).
    pub fn dividir(&self) -> Dividido<T> {
        Dividido(self.0.clone())
    }

    pub fn mesmo(&self, outro: &Dividido<T>) -> bool {
        Rc::ptr_eq(&self.0, &outro.0)
    }
}

impl<T> Deref for Dividido<T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.0.get() }
    }
}

impl<T> DerefMut for Dividido<T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.0.get() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Estado {
    Pronto,
    /// Dorme ate o contador chegar neste valor.
    Dormindo(u64),
    /// Uma chamada que espera algo: e repetida a cada volta do laco.
    Esperando(&'static str),
    /// Parado num vfork ate o filho trocar de programa ou sair.
    Vfork,
    /// Ja terminou (status no formato do wait4); espera o pai ler.
    Terminado(i32),
}

impl Estado {
    pub fn nome(&self) -> &'static str {
        match self {
            Estado::Pronto => "rodando",
            Estado::Dormindo(_) => "dormindo",
            Estado::Esperando(motivo) => motivo,
            Estado::Vfork => "esperando o filho",
            Estado::Terminado(_) => "terminado",
        }
    }
}

/// Status do wait4 -> codigo que o shell mostra (128 + sinal se morreu).
pub fn codigo_de_saida(status: i32) -> i32 {
    if status & 0x7F == 0 { status >> 8 & 0xFF } else { 128 + (status & 0x7F) }
}

pub struct Processo {
    /// O id da thread (o "tid"); na primeira thread e igual ao tgid.
    pub pid: u32,
    /// O processo (grupo de threads) a que esta thread pertence: o que
    /// o getpid devolve.
    pub tgid: u32,
    /// Onde escrever 0 (e acordar o futex) quando a thread sai
    /// (set_tid_address, CLONE_CHILD_CLEARTID).
    pub limpar_tid: usize,
    /// 0 = criado pelo kernel (shell).
    pub pai: u32,
    pub nome: String,
    pub estado: Estado,
    pub contexto: Box<Contexto>,
    pub espaco: Dividido<Espaco>,
    /// Ticks do contador rodando em EL0.
    pub tempo_cpu: u64,
    /// Estado das chamadas de sistema do Linux (arquivos, brk...).
    pub linux: chamadas::Linux,
    /// Caminho do programa (o /proc/self/exe).
    pub exe: String,
    /// Argumentos separados por 0 (o /proc/PID/cmdline).
    pub cmdline: Vec<u8>,
    /// cpu::contador quando o processo nasceu.
    pub inicio: u64,
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

// Bits do termios (c_iflag e c_lflag).
const ICRNL: u32 = 0o400;
const ISIG: u32 = 0o1;
const ICANON: u32 = 0o2;
const ECHO: u32 = 0o10;

/// Disciplina de linha do terminal: no modo canonico (o padrao) junta a
/// linha com eco e Backspace e so entrega no Enter; no modo cru (o
/// programa tirou o ICANON pelo ioctl) cada tecla vai direto.
pub struct Tty {
    linha: Vec<u8>,
    pub prontos: VecDeque<u8>,
    pub canonico: bool,
    pub eco: bool,
    pub sinais: bool,
    pub cr_vira_nl: bool,
    /// Ctrl+D numa linha vazia: o proximo read devolve 0.
    pub fim: bool,
    /// O struct termios como o programa deixou (TCGETS devolve igual).
    pub termios: [u8; 36],
    /// Grupo de processos que recebe o teclado (e o Ctrl+C).
    pub grupo: u32,
}

/// termios padrao: ICRNL|IXON, OPOST|ONLCR, B38400|CS8|CREAD,
/// ISIG|ICANON|ECHO|ECHOE|ECHOK|IEXTEN e os caracteres de controle.
const TERMIOS_PADRAO: [u8; 36] = {
    let mut t = [0u8; 36];
    let campos: [u32; 4] = [0o2400, 0o5, 0o277, 0o105073];
    let mut k = 0;
    while k < 4 {
        let b = campos[k].to_le_bytes();
        t[k * 4] = b[0];
        t[k * 4 + 1] = b[1];
        t[k * 4 + 2] = b[2];
        t[k * 4 + 3] = b[3];
        k += 1;
    }
    // c_line (16) e c_cc[19] a partir de 17: INTR QUIT ERASE KILL EOF TIME MIN
    let cc: [u8; 7] = [3, 0x1C, 0x7F, 0x15, 4, 0, 1];
    let mut j = 0;
    while j < 7 {
        t[17 + j] = cc[j];
        j += 1;
    }
    t
};

impl Tty {
    const fn novo() -> Tty {
        Tty {
            linha: Vec::new(),
            prontos: VecDeque::new(),
            canonico: true,
            eco: true,
            sinais: true,
            cr_vira_nl: true,
            fim: false,
            termios: TERMIOS_PADRAO,
            grupo: 0,
        }
    }

    /// TCSETS: guarda o termios e liga ou desliga o que o MXOS sabe fazer.
    pub fn definir_termios(&mut self, novo: &[u8]) {
        let n = novo.len().min(36);
        self.termios[..n].copy_from_slice(&novo[..n]);
        let campo = |k: usize| u32::from_le_bytes([self.termios[k], self.termios[k + 1], self.termios[k + 2], self.termios[k + 3]]);
        let (iflag, lflag) = (campo(0), campo(12));
        self.cr_vira_nl = iflag & ICRNL != 0;
        self.sinais = lflag & ISIG != 0;
        self.eco = lflag & ECHO != 0;
        let canonico = lflag & ICANON != 0;
        if self.canonico && !canonico {
            // o que estava sendo digitado vai junto
            let linha: Vec<u8> = self.linha.drain(..).collect();
            self.prontos.extend(linha);
        }
        self.canonico = canonico;
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
    /// Processo que o shell do MXOS espera (o prompt volta quando ele sai).
    primeiro_plano: Option<u32>,
    /// Processos do shell que ja terminaram: (pid, status do wait4).
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

/// Segundos desde 1970 quando o contador estava em 0 (RTC lido uma vez).
static RTC_NO_BOOT: AtomicU64 = AtomicU64::new(0);

pub fn rtc_no_boot() -> u64 {
    if RTC_NO_BOOT.load(Ordering::Relaxed) == 0 {
        let desde_o_boot = cpu::contador() / cpu::frequencia_timer().max(1);
        RTC_NO_BOOT.store(chamadas::segundos_do_rtc().saturating_sub(desde_o_boot), Ordering::Relaxed);
    }
    RTC_NO_BOOT.load(Ordering::Relaxed)
}

/// Libera o que o programa precisa ler em EL0: CTR_EL0 (tamanho da linha
/// de cache), DC ZVA, limpeza de cache para codigo novo (UCT, DZE, UCI no
/// SCTLR) e o contador virtual (CNTKCTL_EL1.EL0VCTEN). Uma vez so.
fn liberar_registradores_el0() {
    static FEITO: AtomicBool = AtomicBool::new(false);
    if FEITO.swap(true, Ordering::Relaxed) {
        return;
    }
    unsafe {
        core::arch::asm!(
            "mrs {t}, sctlr_el1",
            "orr {t}, {t}, #(1 << 14)",
            "orr {t}, {t}, #(1 << 15)",
            "orr {t}, {t}, #(1 << 26)",
            "msr sctlr_el1, {t}",
            "mrs {t}, cntkctl_el1",
            "orr {t}, {t}, #2",
            "msr cntkctl_el1, {t}",
            "isb",
            t = out(reg) _,
            options(nostack)
        );
    }
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

    /// Um ASID livre para um processo novo (None: processos demais).
    pub fn asid_para_novo(&self) -> Option<u16> {
        if self.processos.len() >= MAX_PROCESSOS {
            return None;
        }
        (1..=MAX_ASID).find(|&a| self.processos.iter().all(|p| p.espaco.asid != a))
    }

    /// Poe um processo novo na tabela, com o terminal em 0, 1 e 2.
    /// Devolve o pid.
    pub fn acrescentar(&mut self, pai: u32, nome: String, contexto: Box<Contexto>, espaco: Espaco) -> u32 {
        let pid = self.proximo_pid;
        self.proximo_pid += 1;
        let mut linux = chamadas::Linux::com_terminal();
        linux.grupo = pid;
        let p = Processo {
            pid,
            tgid: pid,
            limpar_tid: 0,
            pai,
            nome,
            estado: Estado::Pronto,
            contexto,
            espaco: Dividido::novo(espaco),
            tempo_cpu: 0,
            linux,
            exe: String::new(),
            cmdline: Vec::new(),
            inicio: cpu::contador(),
        };
        self.processos.push(Box::new(p));
        pid
    }

    /// O processo `i` terminou com `status` (formato do wait4): avisa o
    /// pai, entrega os filhos ao kernel e devolve a memoria.
    pub fn terminar(&mut self, i: usize, status: i32) {
        let (pid, tgid) = (self.processos[i].pid, self.processos[i].tgid);
        if pid != tgid {
            // uma thread morreu por sinal: o processo inteiro vai junto
            if let Some(lider) = self.indice(tgid) {
                return self.terminar(lider, status);
            }
            self.processos.remove(i);
            return;
        }
        // as outras threads do processo somem junto
        self.processos.retain(|q| !(q.tgid == tgid && q.pid != tgid));
        let i = self.indice(pid).unwrap_or(i);
        let pai = self.processos[i].pai;
        if let Some(pai_vfork) = self.processos[i].linux.pai_vfork.take() {
            self.acordar_vfork(pai_vfork);
        }
        for filho in self.processos.iter_mut().filter(|p| p.pai == pid) {
            filho.pai = 0;
        }
        if self.primeiro_plano == Some(pid) {
            self.primeiro_plano = None;
            // o terminal volta ao normal para o shell
            self.tty = Tty::novo();
        }
        // filhos que ja tinham terminado e ninguem vai esperar: somem
        self.processos.retain(|p| !(p.pai == 0 && matches!(p.estado, Estado::Terminado(_)) && p.pid != pid));
        let i = self.indice(pid).unwrap_or(i);
        // pai que ignora SIGCHLD (ou pediu SA_NOCLDWAIT) nao faz wait4
        let pai_ignora = self.indice(pai).is_some_and(|j| {
            let acao = self.processos[j].linux.acoes[17];
            acao.tratador == sinais::SIG_IGN || acao.flags & 1 != 0
        });
        if pai == 0 || pai_ignora {
            // ninguem vai fazer wait4: sai da tabela ja
            if pai == 0 {
                self.terminados.push((pid, status));
            }
            self.processos.remove(i);
            return;
        }
        let p = &mut self.processos[i];
        p.estado = Estado::Terminado(status);
        // a memoria e os arquivos vao embora logo; so fica o registro para o pai
        p.espaco = Dividido::novo(Espaco::novo(p.espaco.asid));
        p.linux = chamadas::Linux::novo();
        self.sinal(pai, 17); // SIGCHLD
    }

    /// Uma thread (nao a primeira) saiu com exit: limpa o tid dela,
    /// acorda quem espera no futex (pthread_join) e sai da tabela.
    pub fn sair_thread(&mut self, i: usize) {
        let endereco = self.processos[i].limpar_tid;
        if endereco != 0 && self.processos[i].espaco.escrever(endereco, &0u32.to_le_bytes()).is_ok() {
            chamadas::acordar_futex(self, i, endereco, usize::MAX);
        }
        self.processos.remove(i);
    }

    pub fn acordar_vfork(&mut self, pid: u32) {
        if let Some(j) = self.indice(pid) {
            if self.processos[j].estado == Estado::Vfork {
                self.processos[j].estado = Estado::Pronto;
            }
        }
    }

    /// Manda um sinal. Sem tratador vale o padrao (a maioria termina; os
    /// que so param ou avisam nao fazem nada); com tratador o sinal fica
    /// pendente e `sinais::entregar` desvia o programa para ele antes de
    /// rodar de novo. false se o processo nao existe.
    pub fn sinal(&mut self, pid: u32, sinal: usize) -> bool {
        let Some(i) = self.indice(pid) else { return false };
        if matches!(self.processos[i].estado, Estado::Terminado(_)) {
            return false;
        }
        if sinal == 0 || sinal >= chamadas::SINAIS {
            return sinal == 0;
        }
        let acao = self.processos[i].linux.acoes[sinal];
        let bloqueado = self.processos[i].linux.mascara & sinais::bit(sinal) != 0;
        if sinal == 9 || (!bloqueado && acao.tratador == sinais::SIG_DFL && sinais::padrao_termina(sinal)) {
            self.terminar(i, sinal as i32);
        } else if acao.tem_tratador() || (bloqueado && acao.tratador == sinais::SIG_DFL) {
            let p = &mut self.processos[i];
            p.linux.pendentes |= sinais::bit(sinal);
            // quem espera ou dorme acorda: a chamada volta com EINTR
            if !bloqueado && matches!(p.estado, Estado::Dormindo(_) | Estado::Esperando(_)) {
                p.estado = Estado::Pronto;
            }
        }
        true
    }

    /// Mata um processo pelo pid (SIGKILL).
    pub fn matar(&mut self, pid: u32) -> bool {
        self.sinal(pid, 9)
    }

    /// Acorda quem dorme e ja passou da hora. Com `tentar`, quem espera
    /// tambem fica pronto para repetir a chamada uma vez.
    fn acordar(&mut self, tentar: bool) {
        let agora = cpu::contador();
        // alarmes (setitimer) que venceram: SIGALRM
        let mut disparados = Vec::new();
        for p in self.processos.iter_mut() {
            if let Some((alvo, intervalo)) = p.linux.alarme {
                if agora >= alvo {
                    p.linux.alarme = (intervalo != 0).then(|| (agora + intervalo, intervalo));
                    disparados.push(p.pid);
                }
            }
        }
        for pid in disparados {
            self.sinal(pid, 14);
        }
        for p in self.processos.iter_mut() {
            match p.estado {
                Estado::Dormindo(alvo) if agora >= alvo => p.estado = Estado::Pronto,
                Estado::Esperando(_) if tentar => p.estado = Estado::Pronto,
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
        t.acordar(true);
        while let Some(i) = t.proximo_pronto() {
            if sinais::entregar(t, i) {
                novidade = true;
                continue;
            }
            let motivo = fatia(&mut t.processos[i]);
            if motivo == SINCRONA {
                novidade |= tratar_excecao(t, i);
            } else if motivo != IRQ {
                // FIQ ou SError vindos do programa: nao deviam acontecer
                let pid = t.processos[i].pid;
                let _ = writeln!(Saida(t), "\n[processo {pid}: erro assíncrono da CPU]");
                t.terminar(i, 7);
                novidade = true;
            }
            if unix::mexeu() {
                // um soquete local mudou: o servidor X11 atende na hora e
                // quem espera por um soquete tenta de novo, sem esperar o
                // proximo tique
                crate::x11::atender();
                t.acordar(true);
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
        t.acordar(false);
        t.processos.iter().any(|p| p.estado == Estado::Pronto)
    })
}

/// Roda o processo ate ele voltar para o kernel. Devolve o motivo.
fn fatia(p: &mut Processo) -> u64 {
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
    match classe {
        0x15 => return chamadas::atender(t, i),
        // WFI/WFE em EL0: so um "dar a vez"
        0x01 => {
            t.processos[i].contexto.pc += 4;
            return false;
        }
        // escrita numa pagina dividida pelo fork (copy-on-write): copia e
        // o programa repete a instrucao. ISS: WnR (bit 6) e falha de
        // permissao (DFSC 0b0011xx).
        0x24 if esr & 0x40 != 0 && esr & 0x3C == 0x0C => {
            let far = t.processos[i].contexto.far as usize;
            if t.processos[i].espaco.resolver_cow(far) {
                return false;
            }
        }
        _ => {}
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
        0x18 => String::from("acesso a registrador do sistema proibido"),
        0x3C => String::from("ponto de parada (brk)"),
        _ => format!("exceção da classe 0x{classe:02x}"),
    };
    let _ = writeln!(Saida(t), "\n[processo {pid} ({nome}) morreu: {motivo}, no PC 0x{pc:x}]");
    // SIGSEGV (11) ou SIGILL (4), como o shell do Linux mostraria
    let sinal = if matches!(classe, 0x24 | 0x20) { 11 } else { 4 };
    t.terminar(i, sinal);
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

/// Um aviso do sistema na saida do terminal (como se um processo escrevesse).
pub fn avisar_no_terminal(texto: &str) {
    com(|t| t.escrever(texto.as_bytes()));
}

/// Tira o que os processos escreveram (o shell manda para a tela).
pub fn tirar_saida() -> Vec<u8> {
    com(|t| t.saida.drain(..).collect())
}

/// Codigo de saida de um processo do shell, se ja terminou.
pub fn terminou(pid: u32) -> Option<i32> {
    com(|t| {
        let i = t.terminados.iter().position(|&(p, _)| p == pid)?;
        Some(codigo_de_saida(t.terminados.remove(i).1))
    })
}

/// Uma tecla do terminal para os processos (o shell do MXOS so manda
/// enquanto espera um processo em primeiro plano).
pub fn tecla(byte: u8) {
    com(|t| {
        let tty = &mut t.tty;
        let byte = if byte == b'\r' && tty.cr_vira_nl { b'\n' } else { byte };
        if byte == 3 && tty.sinais {
            // Ctrl+C: SIGINT para o grupo do terminal. Quem tem tratador
            // (um shell) nao morre: o tratador dele roda.
            tty.linha.clear();
            let grupo = tty.grupo;
            let alvos: Vec<u32> =
                t.processos.iter().filter(|p| p.linux.grupo == grupo && p.pid == p.tgid).map(|p| p.pid).collect();
            let mut alguem_morreu = false;
            for pid in alvos {
                let antes = t.indice(pid).map(|i| t.processos[i].estado);
                t.sinal(pid, 2);
                alguem_morreu |= t.indice(pid).map(|i| t.processos[i].estado) != antes;
            }
            t.escrever(if alguem_morreu { b"^C\n" } else { b"^C" });
            return;
        }
        if !tty.canonico {
            tty.prontos.push_back(byte);
            if tty.eco {
                t.escrever(&[byte]);
            }
            return;
        }
        match byte {
            // Ctrl+D: fim de arquivo (ou entrega a linha sem o Enter)
            4 => {
                if tty.linha.is_empty() {
                    tty.fim = true;
                } else {
                    let linha: Vec<u8> = tty.linha.drain(..).collect();
                    tty.prontos.extend(linha);
                }
            }
            b'\n' | b'\r' => {
                let mut linha: Vec<u8> = tty.linha.drain(..).collect();
                linha.push(b'\n');
                tty.prontos.extend(linha);
                if tty.eco {
                    t.escrever(b"\n");
                }
            }
            8 | 127 => {
                // apaga o caractere inteiro (UTF-8)
                let mut apagou = false;
                while let Some(b) = tty.linha.pop() {
                    apagou = true;
                    if b & 0xC0 != 0x80 {
                        break;
                    }
                }
                if apagou && tty.eco {
                    t.escrever(b"\x08 \x08");
                }
            }
            _ => {
                tty.linha.push(byte);
                if tty.eco {
                    t.escrever(&[byte]);
                }
            }
        }
    })
}

// ---------------------------------------------------------------------
// Lista, matar, executar

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
    com(|t| t.matar(pid))
}

/// Roda um programa do disco (ELF de Linux aarch64 estatico). O nome
/// sem "/" e procurado em /bin. `primeiro_plano`: fica com o teclado do
/// terminal ate terminar.
pub fn executar(comando: &str, argumentos: &[&str], primeiro_plano: bool) -> Result<u32, String> {
    let caminho = if comando.contains('/') { arquivos::absoluto("/", comando) } else { format!("/bin/{comando}") };
    let mut args: Vec<Vec<u8>> = Vec::with_capacity(argumentos.len() + 1);
    args.push(comando.as_bytes().to_vec());
    args.extend(argumentos.iter().map(|a| a.as_bytes().to_vec()));
    let ambiente: Vec<Vec<u8>> = AMBIENTE.iter().map(|e| e.as_bytes().to_vec()).collect();
    liberar_registradores_el0();
    rtc_no_boot();
    com(|t| {
        let asid = t.asid_para_novo().ok_or_else(|| String::from("processos demais"))?;
        // um programa vazio que o trocar_programa (execve) enche
        let pid = t.acrescentar(0, String::from(comando), Contexto::novo(0, 0), Espaco::novo(asid));
        let i = t.processos.len() - 1;
        if let Err(e) = chamadas::trocar_programa(t, i, &caminho, args, ambiente) {
            t.processos.remove(i);
            return Err(motivo_de_erro(e, &caminho));
        }
        if primeiro_plano {
            t.primeiro_plano = Some(pid);
            t.tty = Tty::novo();
            t.tty.grupo = pid;
        }
        Ok(pid)
    })
}

/// Explica um errno do executar.
fn motivo_de_erro(errno: i64, caminho: &str) -> String {
    match errno {
        arquivos::ENOENT => format!("{caminho}: arquivo não encontrado"),
        arquivos::ENOEXEC => match armazenamento::ler(caminho) {
            Ok(dados) => match elf::conferir(&dados) {
                Err(motivo) => format!("{caminho}: {motivo}"),
                Ok(()) => {
                    let mut e = Espaco::novo(0);
                    match elf::carregar(&dados, &mut e) {
                        Err(motivo) => format!("{caminho}: {motivo}"),
                        Ok(_) => format!("{caminho}: não deu para carregar"),
                    }
                }
            },
            Err(e) => format!("{caminho}: {}", e.descricao()),
        },
        arquivos::EISDIR => format!("{caminho}: é uma pasta"),
        arquivos::ENOMEM => format!("{caminho}: sem memória"),
        arquivos::ELIBBAD => format!("{caminho}: programa dinâmico, e o interpretador (ld-linux) falta ou não carregou"),
        e => format!("{caminho}: erro {e}"),
    }
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
        let asid = t.asid_para_novo().ok_or("processos demais")?;
        let mut espaco = Espaco::novo(asid);
        espaco.mapear_faixa(BASE_TESTE, BASE_TESTE + codigo.len(), Permissao::Codigo).map_err(|_| "sem memória")?;
        espaco.escrever_forcado(BASE_TESTE, codigo).map_err(|_| "sem memória")?;
        espaco.mapear_faixa(TOPO_PILHA - TAMANHO_PILHA, TOPO_PILHA, Permissao::Dados).map_err(|_| "sem memória")?;
        let mut contexto = Contexto::novo(BASE_TESTE, TOPO_PILHA);
        contexto.x[0] = numero as u64;
        let pid = t.acrescentar(0, format!("teste-{modo}"), contexto, espaco);
        if primeiro_plano {
            t.primeiro_plano = Some(pid);
            t.tty = Tty::novo();
            t.tty.grupo = pid;
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
                let _ = writeln!(saida, "  chamada {numero:>4} ({}): {vezes} vez(es), devolveu -ENOSYS", nome_da_chamada(numero));
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
    let _ = writeln!(saida, "  PID  PAI  ESTADO              CPU (ms)  MEMÓRIA  NOME");
    for p in lista {
        let ms = p.tempo_cpu * 1000 / freq;
        let _ = writeln!(saida, "{:>5} {:>4}  {:<18} {:>9}  {:>5} KB  {}", p.pid, p.pai, p.estado, ms, p.memoria / 1024, p.nome);
    }
}

/// Nome de algumas chamadas do Linux aarch64 (para o comando "syscalls").
fn nome_da_chamada(numero: u64) -> &'static str {
    match numero {
        19 => "eventfd2",
        20 => "epoll_create1",
        21 => "epoll_ctl",
        22 => "epoll_pwait",
        26 => "inotify_init1",
        32 => "flock",
        33 => "mknodat",
        36 => "symlinkat",
        37 => "linkat",
        40 => "mount",
        47 => "fallocate",
        51 => "chroot",
        71 => "sendfile",
        72 => "pselect6",
        74 => "signalfd4",
        76 => "splice",
        117 => "ptrace",
        118 => "sched_setparam",
        122 => "sched_setaffinity",
        123 => "sched_getaffinity",
        165 => "getrusage",
        198 => "socket",
        203 => "connect",
        216 => "mremap",
        220 => "clone (thread)",
        242 => "accept4",
        262 => "fanotify_init",
        267 => "syncfs",
        271 => "process_vm_readv",
        277 => "seccomp",
        279 => "memfd_create",
        281 => "execveat",
        285 => "copy_file_range",
        291 => "statx",
        293 => "rseq",
        424 => "pidfd_send_signal",
        434 => "pidfd_open",
        435 => "clone3",
        437 => "openat2",
        _ => "?",
    }
}
