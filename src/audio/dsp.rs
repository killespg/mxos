//! /dev/dsp: o som dos programas de Linux pela interface OSS (a mais
//! simples: abrir, dizer a taxa e os canais por ioctl, escrever amostras
//! S16). Programas como mpg123, SDL e ffplay sabem usar.
//!
//! O que o programa escreve vai para uma fila; o laco principal chama
//! `bombear`, que passa a fila para o virtio-sound (silencio quando ela
//! esvazia). O dispositivo e iniciado no primeiro open e fica ligado.

use alloc::alloc::{Layout, alloc_zeroed};
use alloc::collections::VecDeque;
use core::cell::UnsafeCell;

use super::gerador::Fonte;
use super::virtio_som::{self, Parametros, VirtioSom};
use crate::virtio::virtio_mmio::{TIPO_SOM, VirtioMmio};

const TAMANHO_DMA: usize = 64 * 1024;
/// Amostras na fila (uns 0,7 s em 48 kHz estereo): passou disso, o write espera.
const MAXIMO_FILA: usize = 64 * 1024;

struct Dsp {
    som: VirtioSom,
    stream: u32,
    info: virtio_som::InfoPcm,
    p: Parametros,
    /// Canais que o programa escreve (1 ou 2).
    canais_programa: u8,
    fila: VecDeque<i16>,
    tocando: bool,
    abertos: usize,
}

struct Global(UnsafeCell<Option<Dsp>>);

// So o laco principal mexe (com a trava do kernel).
unsafe impl Sync for Global {}

static DSP: Global = Global(UnsafeCell::new(None));

fn dsp() -> Option<&'static mut Dsp> {
    unsafe { (*DSP.0.get()).as_mut() }
}

/// Fonte que tira da fila (silencio quando ela acaba).
struct DaFila<'a> {
    fila: &'a mut VecDeque<i16>,
    canais_programa: u8,
}

impl Fonte for DaFila<'_> {
    fn proximo_quadro(&mut self, quadro: &mut [i16]) -> bool {
        if self.canais_programa == 1 {
            let v = self.fila.pop_front().unwrap_or(0);
            quadro.fill(v);
        } else {
            for (k, lugar) in quadro.iter_mut().enumerate() {
                *lugar = if k < 2 { self.fila.pop_front().unwrap_or(0) } else { 0 };
            }
        }
        true
    }
}

/// TV box: o som vai pelo HDMI (audio::hdmi) e nao pelo virtio.
#[cfg(feature = "rk322x")]
static CANAIS_HDMI: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(2);

#[cfg(feature = "rk322x")]
fn hdmi() -> bool {
    super::hdmi::ligado()
}
#[cfg(not(feature = "rk322x"))]
fn hdmi() -> bool {
    false
}

/// open de /dev/dsp. false = nao ha placa de som.
pub fn abrir() -> bool {
    #[cfg(feature = "rk322x")]
    if super::hdmi::iniciar() {
        return true;
    }
    if let Some(d) = dsp() {
        d.abertos += 1;
        return true;
    }
    let Some(disp) = VirtioMmio::procurar(TIPO_SOM) else { return false };
    let Ok(layout) = Layout::from_size_align(TAMANHO_DMA, 4096) else { return false };
    // memoria de DMA para sempre (o dispositivo fica ligado)
    let memoria = unsafe { alloc_zeroed(layout) };
    if memoria.is_null() {
        return false;
    }
    let iniciar = || -> Result<Dsp, virtio_som::ErroSom> {
        let mut som = unsafe { VirtioSom::iniciar(disp, memoria as usize, TAMANHO_DMA)? };
        let (stream, info) = som.primeiro_stream_de_saida()?;
        let p = virtio_som::escolher_parametros(&info, &[48000, 44100], 20, 8)?;
        som.configurar(stream, &p)?;
        Ok(Dsp { som, stream, info, p, canais_programa: 2, fila: VecDeque::new(), tocando: false, abertos: 1 })
    };
    match iniciar() {
        Ok(d) => {
            unsafe { *DSP.0.get() = Some(d) };
            true
        }
        Err(_) => false,
    }
}

pub fn fechar() {
    if let Some(d) = dsp() {
        d.abertos = d.abertos.saturating_sub(1);
    }
}

/// SNDCTL_DSP_SPEED: a taxa pedida, se a placa aceita; senao a dela.
pub fn taxa(pedida: u32) -> u32 {
    #[cfg(feature = "rk322x")]
    if hdmi() {
        return super::hdmi::TAXA;
    }
    let Some(d) = dsp() else { return pedida };
    if pedida != d.p.taxa_hz && d.info.suporta_taxa(pedida) {
        if let Ok(p) = virtio_som::escolher_parametros(&d.info, &[pedida], 20, 8) {
            if d.tocando {
                let _ = d.som.parar();
                let _ = d.som.liberar();
                d.tocando = false;
            }
            if d.som.configurar(d.stream, &p).is_ok() {
                d.p = p;
            }
        }
    }
    d.p.taxa_hz
}

/// SNDCTL_DSP_CHANNELS: 1 ou 2.
pub fn canais(pedidos: u32) -> u32 {
    #[cfg(feature = "rk322x")]
    if hdmi() {
        let c = if pedidos <= 1 { 1 } else { 2 };
        CANAIS_HDMI.store(c, core::sync::atomic::Ordering::Relaxed);
        return c as u32;
    }
    let Some(d) = dsp() else { return pedidos };
    d.canais_programa = if pedidos <= 1 { 1 } else { 2 };
    d.canais_programa as u32
}

/// Quantos bytes cabem na fila agora.
pub fn espaco() -> usize {
    #[cfg(feature = "rk322x")]
    if hdmi() {
        let por_quadro = 2 * CANAIS_HDMI.load(core::sync::atomic::Ordering::Relaxed) as usize;
        return super::hdmi::espaco() * por_quadro;
    }
    dsp().map_or(0, |d| MAXIMO_FILA.saturating_sub(d.fila.len()) * 2)
}

/// write: amostras S16 little-endian. Devolve quantos bytes entraram.
pub fn escrever(dados: &[u8]) -> usize {
    #[cfg(feature = "rk322x")]
    if hdmi() {
        let mono = CANAIS_HDMI.load(core::sync::atomic::Ordering::Relaxed) == 1;
        let amostras: alloc::vec::Vec<i16> = if mono {
            dados.chunks_exact(2).flat_map(|b| {
                let v = i16::from_le_bytes([b[0], b[1]]);
                [v, v]
            }).collect()
        } else {
            dados.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect()
        };
        let quadros = super::hdmi::escrever(&amostras);
        return quadros * if mono { 2 } else { 4 };
    }
    let Some(d) = dsp() else { return dados.len() };
    let cabe = (MAXIMO_FILA.saturating_sub(d.fila.len()) * 2).min(dados.len()) & !1;
    d.fila.extend(dados[..cabe].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])));
    cabe
}

/// Passa a fila para a placa. O laco principal chama sempre.
pub fn bombear() {
    let Some(d) = dsp() else { return };
    if !d.tocando {
        if d.fila.is_empty() {
            return;
        }
        let mut fonte = DaFila { fila: &mut d.fila, canais_programa: d.canais_programa };
        if d.som.comecar(&mut fonte).is_err() {
            return;
        }
        d.tocando = true;
        return;
    }
    let mut fonte = DaFila { fila: &mut d.fila, canais_programa: d.canais_programa };
    if d.som.bombear(&mut fonte).is_err() {
        d.tocando = false;
    }
    // ninguem com o /dev/dsp aberto e nada a tocar: para
    if d.abertos == 0 && d.fila.is_empty() {
        let _ = d.som.parar();
        let _ = d.som.liberar();
        let p = d.p;
        let _ = d.som.configurar(d.stream, &p);
        d.tocando = false;
    }
}
