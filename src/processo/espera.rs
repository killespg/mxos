//! Esperar por varios descritores: ppoll, pselect6, epoll e o eventfd
//! (um contador que serve de sinal entre threads ou para o laco de
//! eventos de bibliotecas como a glib).
//!
//! Tudo pergunta a mesma coisa (`prontidao`): o descritor tem o que ler,
//! aceita escrita, deu erro ou o outro lado fechou? Ninguem pronto e com
//! prazo: a chamada espera (Fim::Esperar) e e repetida ate o prazo, como
//! as outras do chamadas.rs.

use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;

use super::arquivos::*;
use super::chamadas::{escrever_mem, ler_mem, tempo_em_ticks, Fim};
use super::soquetes;
use super::Tabela;
use crate::cpu;

pub const POLLIN: u16 = 1;
pub const POLLOUT: u16 = 4;
pub const POLLERR: u16 = 8;
pub const POLLHUP: u16 = 0x10;
pub const POLLNVAL: u16 = 0x20;

/// O contador do eventfd.
pub struct Contador {
    pub valor: u64,
    pub semaforo: bool,
}

/// Um descritor que o epoll vigia: (fd, eventos pedidos, dado do programa).
#[derive(Clone, Copy)]
pub struct Vigiado {
    fd: i32,
    eventos: u32,
    dado: u64,
}

pub type Lista = Rc<RefCell<Vec<Vigiado>>>;

/// O estado do descritor `fd` em bits do poll.
pub fn prontidao(t: &Tabela, i: usize, fd: i32) -> u16 {
    if fd < 0 {
        return 0;
    }
    let Ok(aberto) = t.processos[i].linux.fd_aberto(fd as u64) else { return POLLNVAL };
    let d = aberto.borrow();
    match &d.tipo {
        Tipo::Terminal => {
            let entrada = !t.tty.prontos.is_empty() || t.tty.fim;
            (if entrada { POLLIN } else { 0 }) | POLLOUT
        }
        Tipo::Tubo(tubo, escrita) => {
            let tb = tubo.borrow();
            if *escrita {
                if tb.leitores == 0 { POLLHUP | POLLERR } else if tb.buffer.len() < TAMANHO_TUBO { POLLOUT } else { 0 }
            } else if !tb.buffer.is_empty() {
                POLLIN
            } else if tb.escritores == 0 {
                POLLHUP
            } else {
                0
            }
        }
        Tipo::Soquete(s) => {
            let (ler, escrever, erro) = soquetes::prontidao(s);
            let fechou = matches!(s, soquetes::Soquete::Unix(super::unix::Unix::Conectado { ponta, .. }) if ponta.desligada());
            (if ler { POLLIN } else { 0 }) | (if escrever { POLLOUT } else { 0 }) | (if erro { POLLERR } else { 0 }) | (if fechou { POLLHUP } else { 0 })
        }
        Tipo::Evento(c) => (if c.borrow().valor > 0 { POLLIN } else { 0 }) | POLLOUT,
        Tipo::Epoll(lista) => {
            // um epoll dentro de outro: pronto se algum vigiado esta
            let vigiados = lista.borrow().clone();
            if vigiados.iter().any(|v| v.fd != fd && prontidao(t, i, v.fd) as u32 & v.eventos != 0) { POLLIN } else { 0 }
        }
        _ => POLLIN | POLLOUT,
    }
}

/// Ninguem pronto: espera ate o prazo (em ticks, None = para sempre).
/// Devolve Some(0) quando o prazo venceu.
fn esperar_prazo(t: &mut Tabela, i: usize, prazo: Option<u64>) -> Option<Fim> {
    let Some(ticks) = prazo else { return None };
    let agora = cpu::contador();
    let alvo = *t.processos[i].linux.acordar_em.get_or_insert(agora.saturating_add(ticks));
    if agora >= alvo {
        t.processos[i].linux.acordar_em = None;
        return Some(Fim::Valor(0));
    }
    None
}

/// Prazo de uma struct timespec (0 = sem prazo).
fn prazo_timespec(t: &Tabela, i: usize, endereco: usize) -> Result<Option<u64>, i64> {
    if endereco == 0 { Ok(None) } else { tempo_em_ticks(t, i, endereco).map(Some) }
}

pub fn ppoll(t: &mut Tabela, i: usize, endereco: usize, quantos: usize, prazo: usize) -> Fim {
    let quantos = quantos.min(256);
    let Ok(mut fds) = ler_mem(t, i, endereco, quantos * 8) else { return Fim::Valor(-EFAULT) };
    let mut prontos = 0;
    for k in 0..quantos {
        let b = &fds[k * 8..k * 8 + 8];
        let fd = i32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let quer = u16::from_le_bytes([b[4], b[5]]);
        let resposta = prontidao(t, i, fd) & (quer | POLLHUP | POLLNVAL | POLLERR);
        fds[k * 8 + 6..k * 8 + 8].copy_from_slice(&resposta.to_le_bytes());
        if resposta != 0 {
            prontos += 1;
        }
    }
    let prazo = match prazo_timespec(t, i, prazo) {
        Ok(p) => p,
        Err(e) => return Fim::Valor(-e),
    };
    if prontos > 0 || prazo == Some(0) {
        t.processos[i].linux.acordar_em = None;
        return match escrever_mem(t, i, endereco, &fds) {
            Ok(()) => Fim::Valor(prontos),
            Err(e) => Fim::Valor(-e),
        };
    }
    if let Some(fim) = esperar_prazo(t, i, prazo) {
        let _ = escrever_mem(t, i, endereco, &fds);
        return fim;
    }
    Fim::Esperar("esperando entrada")
}

/// pselect6: os tres fd_set (ler, escrever, excecao) viram respostas.
pub fn pselect6(t: &mut Tabela, i: usize, quantos: usize, conjuntos: [usize; 3], prazo: usize) -> Fim {
    let quantos = quantos.min(1024);
    let bytes = quantos.div_ceil(64) * 8;
    let mut entrada = [Vec::new(), Vec::new(), Vec::new()];
    for k in 0..3 {
        if conjuntos[k] != 0 && bytes > 0 {
            match ler_mem(t, i, conjuntos[k], bytes) {
                Ok(b) => entrada[k] = b,
                Err(e) => return Fim::Valor(-e),
            }
        }
    }
    let mut saida = [vec_zero(entrada[0].len()), vec_zero(entrada[1].len()), vec_zero(entrada[2].len())];
    let mut prontos = 0;
    for fd in 0..quantos {
        let (byte, bit) = (fd / 8, 1u8 << (fd % 8));
        let quer: [bool; 3] = core::array::from_fn(|k| entrada[k].get(byte).is_some_and(|b| b & bit != 0));
        if !quer.iter().any(|&q| q) {
            continue;
        }
        let estado = prontidao(t, i, fd as i32);
        if estado & POLLNVAL != 0 {
            return Fim::Valor(-EBADF);
        }
        let tem = [estado & (POLLIN | POLLHUP | POLLERR) != 0, estado & (POLLOUT | POLLERR) != 0, false];
        for k in 0..3 {
            if quer[k] && tem[k] {
                saida[k][byte] |= bit;
                prontos += 1;
            }
        }
    }
    let prazo = match prazo_timespec(t, i, prazo) {
        Ok(p) => p,
        Err(e) => return Fim::Valor(-e),
    };
    if prontos == 0 && prazo != Some(0) {
        match esperar_prazo(t, i, prazo) {
            Some(Fim::Valor(_)) => {}
            _ => return Fim::Esperar("esperando entrada (select)"),
        }
    }
    t.processos[i].linux.acordar_em = None;
    for k in 0..3 {
        if conjuntos[k] != 0 && !saida[k].is_empty() {
            if let Err(e) = escrever_mem(t, i, conjuntos[k], &saida[k]) {
                return Fim::Valor(-e);
            }
        }
    }
    Fim::Valor(prontos)
}

fn vec_zero(n: usize) -> Vec<u8> {
    alloc::vec![0; n]
}

// ---------------------------------------------------------------------
// eventfd

pub fn eventfd2(t: &mut Tabela, i: usize, inicial: u64, flags: u64) -> Result<i64, i64> {
    const EFD_SEMAPHORE: u64 = 1;
    let contador = Rc::new(RefCell::new(Contador { valor: inicial, semaforo: flags & EFD_SEMAPHORE != 0 }));
    let abertura = O_RDWR | (flags as u32 & O_NONBLOCK);
    let aberto = Descricao::nova(Tipo::Evento(contador), abertura);
    t.processos[i].linux.novo_descritor(aberto, flags as u32 & O_CLOEXEC != 0).map(|fd| fd as i64)
}

/// read de um eventfd: o valor (ou 1, no modo semaforo) em 8 bytes.
pub fn ler_contador(contador: &RefCell<Contador>, nao_bloqueia: bool, tamanho: usize) -> Result<Option<[u8; 8]>, i64> {
    if tamanho < 8 {
        return Err(EINVAL);
    }
    let mut c = contador.borrow_mut();
    if c.valor == 0 {
        return if nao_bloqueia { Err(EAGAIN) } else { Ok(None) };
    }
    let valor = if c.semaforo { 1 } else { c.valor };
    c.valor -= valor;
    Ok(Some(valor.to_le_bytes()))
}

pub fn escrever_contador(contador: &RefCell<Contador>, dados: &[u8]) -> Result<i64, i64> {
    let bytes: [u8; 8] = dados.get(..8).and_then(|b| b.try_into().ok()).ok_or(EINVAL)?;
    let v = u64::from_le_bytes(bytes);
    if v == u64::MAX {
        return Err(EINVAL);
    }
    let mut c = contador.borrow_mut();
    c.valor = c.valor.saturating_add(v);
    super::unix::avisar();
    Ok(8)
}

// ---------------------------------------------------------------------
// epoll

pub fn epoll_create(t: &mut Tabela, i: usize, flags: u64) -> Result<i64, i64> {
    let lista: Lista = Rc::new(RefCell::new(Vec::new()));
    let aberto = Descricao::nova(Tipo::Epoll(lista), O_RDWR);
    t.processos[i].linux.novo_descritor(aberto, flags as u32 & O_CLOEXEC != 0).map(|fd| fd as i64)
}

fn lista(t: &Tabela, i: usize, epfd: u64) -> Result<Lista, i64> {
    let aberto = t.processos[i].linux.fd_aberto(epfd)?;
    let d = aberto.borrow();
    match &d.tipo {
        Tipo::Epoll(l) => Ok(l.clone()),
        _ => Err(EINVAL),
    }
}

pub fn epoll_ctl(t: &mut Tabela, i: usize, epfd: u64, operacao: u64, fd: u64, evento: usize) -> Result<i64, i64> {
    const ADD: u64 = 1;
    const DEL: u64 = 2;
    const MOD: u64 = 3;
    let l = lista(t, i, epfd)?;
    t.processos[i].linux.fd_aberto(fd)?;
    let (eventos, dado) = if operacao != DEL {
        let b = ler_mem(t, i, evento, 16)?;
        (u32::from_le_bytes(b[0..4].try_into().unwrap()), u64::from_le_bytes(b[8..16].try_into().unwrap()))
    } else {
        (0, 0)
    };
    let mut l = l.borrow_mut();
    let existe = l.iter().position(|v| v.fd == fd as i32);
    match (operacao, existe) {
        (ADD, Some(_)) => Err(EEXIST),
        (ADD, None) => {
            l.push(Vigiado { fd: fd as i32, eventos, dado });
            Ok(0)
        }
        (MOD, Some(k)) => {
            l[k] = Vigiado { fd: fd as i32, eventos, dado };
            Ok(0)
        }
        (DEL, Some(k)) => {
            l.remove(k);
            Ok(0)
        }
        _ => Err(ENOENT),
    }
}

/// epoll_pwait: ate `maximo` eventos prontos (struct epoll_event do arm64:
/// 4 bytes de eventos, 4 de enchimento e 8 do dado).
pub fn epoll_wait(t: &mut Tabela, i: usize, epfd: u64, eventos: usize, maximo: usize, prazo_ms: i64) -> Fim {
    const EPOLLONESHOT: u32 = 1 << 30;
    let l = match lista(t, i, epfd) {
        Ok(l) => l,
        Err(e) => return Fim::Valor(-e),
    };
    if maximo == 0 {
        return Fim::Valor(-EINVAL);
    }
    let vigiados = l.borrow().clone();
    let mut saida = Vec::new();
    let mut quantos = 0;
    for v in vigiados {
        if quantos == maximo.min(256) {
            break;
        }
        let estado = prontidao(t, i, v.fd) as u32;
        if estado & POLLNVAL as u32 != 0 {
            // fechado sem o EPOLL_CTL_DEL: sai da lista, como no Linux
            l.borrow_mut().retain(|x| x.fd != v.fd);
            continue;
        }
        let resposta = estado & (v.eventos | (POLLERR | POLLHUP) as u32);
        if resposta == 0 {
            continue;
        }
        saida.extend_from_slice(&resposta.to_le_bytes());
        saida.extend_from_slice(&[0; 4]);
        saida.extend_from_slice(&v.dado.to_le_bytes());
        quantos += 1;
        if v.eventos & EPOLLONESHOT != 0 {
            if let Some(x) = l.borrow_mut().iter_mut().find(|x| x.fd == v.fd) {
                x.eventos = 0;
            }
        }
    }
    if quantos == 0 && prazo_ms != 0 {
        let prazo = (prazo_ms > 0).then(|| prazo_ms as u64 * cpu::frequencia_timer() / 1000);
        match esperar_prazo(t, i, prazo) {
            Some(fim) => return fim,
            None => return Fim::Esperar("esperando entrada (epoll)"),
        }
    }
    t.processos[i].linux.acordar_em = None;
    match escrever_mem(t, i, eventos, &saida) {
        Ok(()) => Fim::Valor(quantos as i64),
        Err(e) => Fim::Valor(-e),
    }
}
