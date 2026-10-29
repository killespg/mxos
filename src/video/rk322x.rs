//! Video na TV box com RK322x: o VOP (controlador de video da Rockchip)
//! que o U-Boot/Android deixou ligado no HDMI. Como no mxq.rs da Amlogic,
//! nada de driver HDMI: o MXOS le a janela 0 do VOP (tamanho, formato,
//! endereco) e aponta ela para um framebuffer proprio de 32 bits, com a
//! mesma resolucao. NUNCA TESTADO NO HARDWARE.
//!
//! Registradores do VOP do RK3228 (os mesmos do RK3288 no Linux,
//! drivers/gpu/drm/rockchip/rockchip_vop_reg.c).

use alloc::alloc::{Layout, alloc_zeroed};
use core::ptr::{read_volatile, write_volatile};

use super::tela::{Formato, Tela};

const VOP: usize = 0x2005_0000;
const CFG_DONE: usize = 0x0000;
const WIN0_CTRL0: usize = 0x0030;
const WIN0_VIR: usize = 0x003C;
const WIN0_YRGB_MST: usize = 0x0040;
const WIN0_ACT_INFO: usize = 0x0048;
const WIN0_DSP_INFO: usize = 0x004C;

fn ler(r: usize) -> u32 {
    unsafe { read_volatile((VOP + r) as *const u32) }
}

fn escrever(r: usize, v: u32) {
    unsafe { write_volatile((VOP + r) as *mut u32, v) }
}

/// (largura, altura) da janela 0, se ela esta ligada.
pub fn descobrir() -> Result<(usize, usize), &'static str> {
    let ctrl = ler(WIN0_CTRL0);
    if ctrl & 1 == 0 {
        return Err("o U-Boot deixou o vídeo desligado (VOP sem janela)");
    }
    // a area que aparece na tela (DSP_INFO), senao a da imagem (ACT_INFO)
    let info = match ler(WIN0_DSP_INFO) {
        0 => ler(WIN0_ACT_INFO),
        v => v,
    };
    let largura = (info & 0x1FFF) as usize + 1;
    let altura = (info >> 16 & 0x1FFF) as usize + 1;
    if !(320..=4096).contains(&largura) || !(200..=2304).contains(&altura) {
        return Err("tamanho de tela estranho no VOP");
    }
    Ok((largura, altura))
}

/// Liga o framebuffer do MXOS (XRGB 32 bits, do heap, nunca devolvido) na
/// janela 0 do VOP. Devolve a Tela e o endereco.
pub fn tomar_posse() -> Result<(Tela, usize), &'static str> {
    let (largura, altura) = descobrir()?;
    let bytes = largura * altura * 4;
    let layout = Layout::from_size_align(bytes, 4096).map_err(|_| "tamanho inválido")?;
    let base = unsafe { alloc_zeroed(layout) } as usize;
    if base == 0 {
        return Err("sem memória para o framebuffer");
    }
    let tela = unsafe { Tela::new(base, largura, altura, largura * 4, Formato::Xrgb8888) }.com_limpeza_de_cache(true);
    tela.limpar(super::tela::PRETO);
    // formato ARGB8888 (0 nos bits 1-3), passo em palavras, endereco novo;
    // a imagem tem o tamanho da tela
    let ctrl = ler(WIN0_CTRL0) & !(0x7 << 1);
    escrever(WIN0_VIR, largura as u32);
    escrever(WIN0_ACT_INFO, ((altura as u32 - 1) << 16) | (largura as u32 - 1));
    escrever(WIN0_YRGB_MST, base as u32);
    escrever(WIN0_CTRL0, ctrl | 1);
    escrever(CFG_DONE, 1);
    Ok((tela, base))
}
