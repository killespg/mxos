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
/// Janelas 0 e 1 (rk3288_vop_win_data do Linux) e os registradores de cada.
const JANELAS: [usize; 2] = [0x0030, 0x0070];
const CTRL0: usize = 0x00;
const VIR: usize = 0x0C;
const YRGB_MST: usize = 0x10;
const ACT_INFO: usize = 0x18;
const DSP_INFO: usize = 0x1C;
const CTRL1: usize = 0x04;
const SCL_FACTOR: usize = 0x14;
/// CTRL0 bits 7:5: buffer de linha (rk3288_vop: LB_RGB_1920X5 = 4).
const LB_1920: u32 = 4;

fn ler(r: usize) -> u32 {
    unsafe { read_volatile((VOP + r) as *const u32) }
}

fn escrever(r: usize, v: u32) {
    unsafe { write_volatile((VOP + r) as *mut u32, v) }
}

/// A primeira janela ligada pelo U-Boot (base dos registradores dela).
fn janela() -> Result<usize, &'static str> {
    JANELAS.into_iter().find(|&j| ler(j + CTRL0) & 1 != 0).ok_or("o U-Boot deixou o vídeo desligado (VOP sem janela)")
}

/// (largura, altura) da janela que o U-Boot ligou.
pub fn descobrir() -> Result<(usize, usize), &'static str> {
    let j = janela()?;
    // o tamanho da imagem (ACT_INFO): o VOP do U-Boot amplia para a TV
    // (1280x720 -> 1920x1080 na box) e o escalador continua configurado
    let info = match ler(j + ACT_INFO) {
        0 => ler(j + DSP_INFO),
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
/// janela 0 do VOP. Devolve a Tela e o endereco. `nativa`: a imagem do
/// tamanho da saida (1920x1080 na box), sem o escalador; senao o tamanho
/// que o U-Boot usava (1280x720), que o VOP amplia.
pub fn tomar_posse(nativa: bool) -> Result<(Tela, usize), &'static str> {
    let j = janela()?;
    let (largura, altura) = if nativa {
        let info = ler(j + DSP_INFO);
        ((info & 0x1FFF) as usize + 1, (info >> 16 & 0x1FFF) as usize + 1)
    } else {
        descobrir()?
    };
    if !(320..=4096).contains(&largura) || !(200..=2304).contains(&altura) {
        return Err("tamanho de tela estranho no VOP");
    }
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
    // formato ARGB8888 e sem o espelho vertical (bit 22) e a troca de
    // vermelho com azul (bit 12) que o U-Boot usa para mostrar o BMP
    let mut ctrl = ler(j + CTRL0) & !(0x7 << 1) & !(1 << 22) & !(1 << 12);
    if nativa {
        // 1:1: fator 0x1000 nos dois eixos, escalador sem modo e o buffer
        // de linha das imagens de 1920
        escrever(j + SCL_FACTOR, 0x1000_1000);
        escrever(j + CTRL1, ler(j + CTRL1) & !(0xF << 12));
        ctrl = ctrl & !(0x7 << 5) | LB_1920 << 5;
    }
    escrever(j + VIR, largura as u32);
    escrever(j + ACT_INFO, ((altura as u32 - 1) << 16) | (largura as u32 - 1));
    escrever(j + YRGB_MST, base as u32);
    escrever(j + CTRL0, ctrl | 1);
    escrever(CFG_DONE, 1);
    // o VOP agora mostra o framebuffer do MXOS: o console de boot sai
    crate::console_boot::desligar();
    Ok((tela, base))
}
