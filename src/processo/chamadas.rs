//! Chamadas de sistema, com os numeros e as regras do Linux aarch64:
//! numero em x8, argumentos em x0-x5, resposta em x0 (erro = -errno).
//! Assim um programa de Linux estatico roda sem mudar nada.
//!
//! Chamada que o MXOS ainda nao tem devolve -ENOSYS e fica anotada (o
//! comando "syscalls" mostra quais faltam).

use super::{Estado, Tabela};
use crate::cpu;

pub const ENOSYS: i64 = 38;
pub const EFAULT: i64 = 14;
pub const EBADF: i64 = 9;
pub const EINVAL: i64 = 22;
pub const ESRCH: i64 = 3;

/// O que cada processo guarda para as chamadas do Linux.
pub struct Linux {
    /// Hora de acordar de um nanosleep em andamento (a chamada e repetida
    /// quando o processo acorda).
    acordar_em: Option<u64>,
}

impl Linux {
    pub fn novo() -> Linux {
        Linux { acordar_em: None }
    }
}

/// O que fazer com o processo depois da chamada.
enum Fim {
    /// Volta para o programa com este valor em x0.
    Valor(i64),
    /// Espera (o estado ja foi trocado): o SVC roda de novo quando acordar.
    Esperar,
    /// O processo nao existe mais (exit).
    Saiu,
}

/// Atende a chamada do processo `i` (que acabou de fazer SVC). Devolve
/// true se algo mudou que o terminal precisa ver.
pub fn atender(t: &mut Tabela, i: usize) -> bool {
    let c = &t.processos[i].contexto;
    let numero = c.x[8];
    let a = [c.x[0], c.x[1], c.x[2], c.x[3], c.x[4], c.x[5]];
    let fim = match numero {
        63 => ler(t, i, a[0], a[1] as usize, a[2] as usize),
        64 => escrever(t, i, a[0], a[1] as usize, a[2] as usize),
        93 | 94 => {
            t.terminar(i, a[0] as i32 & 0xFF);
            Fim::Saiu
        }
        101 => dormir(t, i, a[0] as usize),
        113 => relogio(t, i, a[1] as usize),
        124 => Fim::Valor(0),
        129 => Fim::Valor(if t.matar(a[0] as u32, a[1] as i32) { 0 } else { -ESRCH }),
        172 | 178 => Fim::Valor(t.processos[i].pid as i64),
        173 => Fim::Valor(t.processos[i].pai as i64),
        174..=177 => Fim::Valor(0),
        _ => {
            *t.desconhecidas.entry(numero).or_insert(0) += 1;
            Fim::Valor(-ENOSYS)
        }
    };
    match fim {
        Fim::Valor(v) => {
            t.processos[i].contexto.x[0] = v as u64;
            numero == 64
        }
        Fim::Esperar => {
            // volta para o SVC: quando acordar, repete a chamada
            t.processos[i].contexto.pc -= 4;
            false
        }
        Fim::Saiu => true,
    }
}

/// read: so o terminal (fd 0) por enquanto.
fn ler(t: &mut Tabela, i: usize, fd: u64, endereco: usize, tamanho: usize) -> Fim {
    if fd != 0 {
        return Fim::Valor(-EBADF);
    }
    if t.tty.prontos.is_empty() {
        if core::mem::take(&mut t.tty.fim) {
            return Fim::Valor(0);
        }
        t.processos[i].estado = Estado::Lendo;
        return Fim::Esperar;
    }
    let n = tamanho.min(t.tty.prontos.len());
    let dados: alloc::vec::Vec<u8> = t.tty.prontos.iter().take(n).copied().collect();
    if t.processos[i].espaco.escrever(endereco, &dados).is_err() {
        return Fim::Valor(-EFAULT);
    }
    t.tty.prontos.drain(..n);
    Fim::Valor(n as i64)
}

/// write: stdout e stderr vao para o terminal.
fn escrever(t: &mut Tabela, i: usize, fd: u64, endereco: usize, tamanho: usize) -> Fim {
    if fd != 1 && fd != 2 {
        return Fim::Valor(-EBADF);
    }
    let mut dados = alloc::vec![0u8; tamanho.min(1 << 20)];
    if t.processos[i].espaco.ler(endereco, &mut dados).is_err() {
        return Fim::Valor(-EFAULT);
    }
    t.escrever(&dados);
    Fim::Valor(dados.len() as i64)
}

/// nanosleep(&{segundos, nanossegundos}, ...).
fn dormir(t: &mut Tabela, i: usize, endereco: usize) -> Fim {
    let agora = cpu::contador();
    let p = &mut t.processos[i];
    if let Some(alvo) = p.linux.acordar_em {
        if agora >= alvo {
            p.linux.acordar_em = None;
            return Fim::Valor(0);
        }
        p.estado = Estado::Dormindo(alvo);
        return Fim::Esperar;
    }
    let mut tempo = [0u8; 16];
    if p.espaco.ler(endereco, &mut tempo).is_err() {
        return Fim::Valor(-EFAULT);
    }
    let segundos = u64::from_le_bytes(tempo[..8].try_into().unwrap_or_default());
    let nanos = u64::from_le_bytes(tempo[8..].try_into().unwrap_or_default());
    if nanos >= 1_000_000_000 {
        return Fim::Valor(-EINVAL);
    }
    let freq = cpu::frequencia_timer();
    let ticks = segundos.saturating_mul(freq).saturating_add(nanos * freq / 1_000_000_000);
    let alvo = agora.saturating_add(ticks);
    p.linux.acordar_em = Some(alvo);
    p.estado = Estado::Dormindo(alvo);
    Fim::Esperar
}

/// clock_gettime: o contador desde que a maquina ligou (todos os relogios).
fn relogio(t: &mut Tabela, i: usize, endereco: usize) -> Fim {
    let freq = cpu::frequencia_timer();
    let agora = cpu::contador();
    let segundos = agora / freq;
    let nanos = agora % freq * 1_000_000_000 / freq;
    let mut tempo = [0u8; 16];
    tempo[..8].copy_from_slice(&segundos.to_le_bytes());
    tempo[8..].copy_from_slice(&nanos.to_le_bytes());
    match t.processos[i].espaco.escrever(endereco, &tempo) {
        Ok(()) => Fim::Valor(0),
        Err(_) => Fim::Valor(-EFAULT),
    }
}

/// Nome de algumas chamadas do Linux aarch64 (para o comando "syscalls").
pub fn nome(numero: u64) -> &'static str {
    match numero {
        17 => "getcwd",
        23 => "dup",
        24 => "dup3",
        25 => "fcntl",
        29 => "ioctl",
        34 => "mkdirat",
        35 => "unlinkat",
        38 => "renameat",
        48 => "faccessat",
        49 => "chdir",
        56 => "openat",
        57 => "close",
        59 => "pipe2",
        61 => "getdents64",
        62 => "lseek",
        63 => "read",
        64 => "write",
        65 => "readv",
        66 => "writev",
        73 => "ppoll",
        78 => "readlinkat",
        79 => "newfstatat",
        80 => "fstat",
        93 => "exit",
        94 => "exit_group",
        96 => "set_tid_address",
        98 => "futex",
        99 => "set_robust_list",
        101 => "nanosleep",
        113 => "clock_gettime",
        124 => "sched_yield",
        129 => "kill",
        134 => "rt_sigaction",
        135 => "rt_sigprocmask",
        160 => "uname",
        172 => "getpid",
        173 => "getppid",
        174 => "getuid",
        178 => "gettid",
        214 => "brk",
        215 => "munmap",
        220 => "clone",
        221 => "execve",
        222 => "mmap",
        226 => "mprotect",
        260 => "wait4",
        261 => "prlimit64",
        278 => "getrandom",
        _ => "?",
    }
}
