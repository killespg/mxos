//! Memoria compartilhada do System V (shmget, shmat, shmdt, shmctl): um
//! segmento sao quadros que todo processo que faz shmat ve no proprio
//! espaco (memoria.rs, paginas compartilhadas). O servidor X11 le os
//! mesmos quadros (MIT-SHM): a imagem do programa chega na tela sem
//! passar pelo soquete.
//!
//! O segmento some quando foi marcado (IPC_RMID) e ninguem mais o prende
//! (o servidor X); os quadros em si duram enquanto algum processo os tem
//! mapeados (Rc).

use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::cell::UnsafeCell;

use super::arquivos::*;
use super::chamadas::escrever_mem;
use super::memoria::{self, Permissao, Quadro, PAGINA};
use super::Tabela;

const IPC_PRIVATE: i64 = 0;
const IPC_CREAT: u64 = 0o1000;
const IPC_EXCL: u64 = 0o2000;
const IPC_RMID: u64 = 0;
const IPC_STAT: u64 = 2;
const SHM_RDONLY: u64 = 0o10000;
/// Maior segmento aceito (uma tela 4K de 32 bits cabe).
const MAXIMO: usize = 64 << 20;

struct Segmento {
    chave: i64,
    tamanho: usize,
    quadros: Vec<Rc<Box<Quadro>>>,
    /// Marcado para sumir (IPC_RMID).
    apagar: bool,
    /// Quantas vezes o kernel (servidor X) o prendeu.
    presos: usize,
}

struct Segmentos(UnsafeCell<(BTreeMap<u32, Segmento>, u32)>);

// So o laco principal mexe.
unsafe impl Sync for Segmentos {}

static SEGMENTOS: Segmentos = Segmentos(UnsafeCell::new((BTreeMap::new(), 1)));

fn tabela() -> &'static mut (BTreeMap<u32, Segmento>, u32) {
    unsafe { &mut *SEGMENTOS.0.get() }
}

pub fn shmget(chave: i64, tamanho: usize, flags: u64) -> Result<i64, i64> {
    let (segmentos, proximo) = tabela();
    if chave != IPC_PRIVATE {
        if let Some((&id, s)) = segmentos.iter().find(|(_, s)| s.chave == chave && !s.apagar) {
            if flags & IPC_CREAT != 0 && flags & IPC_EXCL != 0 {
                return Err(EEXIST);
            }
            if tamanho > s.tamanho {
                return Err(EINVAL);
            }
            return Ok(id as i64);
        }
        if flags & IPC_CREAT == 0 {
            return Err(ENOENT);
        }
    }
    if tamanho == 0 || tamanho > MAXIMO {
        return Err(EINVAL);
    }
    let quadros = (0..tamanho.div_ceil(PAGINA)).map(|_| memoria::quadro_novo()).collect();
    let id = *proximo;
    *proximo += 1;
    segmentos.insert(id, Segmento { chave, tamanho, quadros, apagar: false, presos: 0 });
    Ok(id as i64)
}

pub fn shmat(t: &mut Tabela, i: usize, id: u32, endereco: usize, flags: u64) -> Result<i64, i64> {
    let s = tabela().0.get(&id).ok_or(EINVAL)?;
    let tamanho = s.quadros.len() * PAGINA;
    let espaco = &mut t.processos[i].espaco;
    let endereco = if endereco != 0 {
        endereco & !(PAGINA - 1)
    } else {
        espaco.achar_livre(super::chamadas::BASE_MMAP, tamanho).ok_or(ENOMEM)?
    };
    let permissao = if flags & SHM_RDONLY != 0 { Permissao::Leitura } else { Permissao::Dados };
    espaco.mapear_compartilhada(endereco, &s.quadros, permissao).map_err(|_| EINVAL)?;
    Ok(endereco as i64)
}

/// shmdt: tira as paginas do segmento que comeca em `endereco`.
pub fn shmdt(t: &mut Tabela, i: usize, endereco: usize) -> Result<i64, i64> {
    let espaco = &mut t.processos[i].espaco;
    let primeiro = espaco.quadro_compartilhado(endereco).ok_or(EINVAL)?;
    let s = tabela().0.values().find(|s| s.quadros.first().is_some_and(|q| Rc::ptr_eq(q, &primeiro)));
    let paginas = s.map_or(1, |s| s.quadros.len());
    for k in 0..paginas {
        espaco.desmapear(endereco + k * PAGINA);
    }
    Ok(0)
}

pub fn shmctl(t: &mut Tabela, i: usize, id: u32, comando: u64, buf: usize) -> Result<i64, i64> {
    let (segmentos, _) = tabela();
    let s = segmentos.get_mut(&id).ok_or(EINVAL)?;
    match comando & 0xFF {
        IPC_RMID => {
            s.apagar = true;
            if s.presos == 0 {
                segmentos.remove(&id);
            }
            Ok(0)
        }
        IPC_STAT => {
            // struct shmid_ds: ipc_perm (48 bytes no aarch64) e shm_segsz
            let mut ds = [0u8; 112];
            ds[0..4].copy_from_slice(&(s.chave as i32).to_le_bytes());
            ds[20..24].copy_from_slice(&0o600u32.to_le_bytes());
            ds[48..56].copy_from_slice(&(s.tamanho as u64).to_le_bytes());
            escrever_mem(t, i, buf, &ds)?;
            Ok(0)
        }
        _ => Ok(0),
    }
}

// ---------------------------------------------------------------------
// Para o servidor X11

/// Prende o segmento (ShmAttach): ele fica mesmo depois do IPC_RMID.
pub fn prender(id: u32) -> bool {
    match tabela().0.get_mut(&id) {
        Some(s) => {
            s.presos += 1;
            true
        }
        None => false,
    }
}

pub fn soltar(id: u32) {
    let (segmentos, _) = tabela();
    if let Some(s) = segmentos.get_mut(&id) {
        s.presos = s.presos.saturating_sub(1);
        if s.presos == 0 && s.apagar {
            segmentos.remove(&id);
        }
    }
}

/// Le `destino.len()` bytes do segmento a partir de `deslocamento`.
pub fn ler(id: u32, deslocamento: usize, destino: &mut [u8]) -> bool {
    let Some(s) = tabela().0.get(&id) else { return false };
    if deslocamento + destino.len() > s.quadros.len() * PAGINA {
        return false;
    }
    let mut feito = 0;
    while feito < destino.len() {
        let atual = deslocamento + feito;
        let dentro = atual % PAGINA;
        let n = (PAGINA - dentro).min(destino.len() - feito);
        destino[feito..feito + n].copy_from_slice(&s.quadros[atual / PAGINA].0[dentro..dentro + n]);
        feito += n;
    }
    true
}
