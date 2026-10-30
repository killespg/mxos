//! Som pelo HDMI na TV box RK322x: I2S0 (0x100C0000) -> controlador HDMI
//! (DW-HDMI em 0x200A0000). Valores copiados do Android da box tocando
//! um seno pela placa "RK-HDMI-I2S" (adb, tinyplay + devmem):
//!
//!   - MCLK 12,288 MHz: GPLL 1200 MHz x 32/3125 (CLKSEL9 = fracionario,
//!     CLKSEL8 = 0x00200C35); sclk = MCLK/2, LRCK = sclk/128 = 48 kHz
//!   - I2S: TXCR 0x0F (16 bits, I2S), CKR 0x17F7F, mestre
//!   - HDMI: AUD_CONF0 0x21 (I2S, entrada 0), AUD_CONF1 0x10 (16 bits),
//!     N = 6144 e CTS = relogio TMDS em kHz (148500 no 1080p), infoframe
//!     de 2 canais, controlador em modo HDMI
//!
//! Sem DMA: a interrupcao do I2S (SPI 26) enche a FIFO de 32 palavras com
//! o que esta no anel (silencio se ele esvaziar). Cada palavra = um
//! quadro estereo (esquerda nos 16 bits de baixo). ~3000 IRQs/s.

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

const I2S: usize = 0x100C_0000;
const CRU: usize = 0x110E_0000;
const HDMI: usize = 0x200A_0000;
pub const IRQ: u32 = 32 + 26;
pub const TAXA: u32 = 48_000;

const TXCR: usize = 0x00;
const CKR: usize = 0x08;
const FIFOLR: usize = 0x0C;
const DMACR: usize = 0x10;
const INTCR: usize = 0x14;
const XFER: usize = 0x1C;
const CLR: usize = 0x20;
const TXDR: usize = 0x24;

/// Quadros no anel (potencia de 2): ~0,68 s.
const ANEL: usize = 32 * 1024;
static mut BUF: [u32; ANEL] = [0; ANEL];
/// Escreve o laco principal (cabeca) e le a IRQ (cauda): sem trava.
static CABECA: AtomicUsize = AtomicUsize::new(0);
static CAUDA: AtomicUsize = AtomicUsize::new(0);
static LIGADO: AtomicBool = AtomicBool::new(false);
/// Pausado: a IRQ manda silencio e o anel fica parado.
static PAUSADO: AtomicBool = AtomicBool::new(false);
/// Quadros que ja sairam pela FIFO (o relogio do som) e vezes que o anel
/// esvaziou com o som tocando.
static TOCADOS: AtomicU32 = AtomicU32::new(0);
static FALTAS: AtomicU32 = AtomicU32::new(0);
static IRQS: AtomicU32 = AtomicU32::new(0);

fn i2s(r: usize) -> u32 {
    unsafe { read_volatile((I2S + r) as *const u32) }
}
fn i2s_w(r: usize, v: u32) {
    unsafe { write_volatile((I2S + r) as *mut u32, v) }
}
fn cru(r: usize, mascara: u32, valor: u32) {
    unsafe { write_volatile((CRU + r) as *mut u32, mascara << 16 | valor) }
}
/// Registrador de 8 bits do DW-HDMI (um por palavra).
fn hdmi(r: usize) -> u8 {
    unsafe { read_volatile((HDMI + r * 4) as *const u32) as u8 }
}
fn hdmi_w(r: usize, v: u8) {
    unsafe { write_volatile((HDMI + r * 4) as *mut u32, v as u32) }
}

pub fn ligado() -> bool {
    LIGADO.load(Ordering::Relaxed)
}

/// GPLL em kHz (o U-Boot deixa 600 MHz; o Android usa 1200).
pub fn gpll_khz() -> u32 {
    let (c0, c1) = unsafe { (read_volatile((CRU + 0x24) as *const u32), read_volatile((CRU + 0x28) as *const u32)) };
    let (fb, pd1) = (c0 & 0xFFF, (c0 >> 12 & 7).max(1));
    let (refdiv, pd2) = ((c1 & 0x3F).max(1), (c1 >> 6 & 7).max(1));
    24_000 * fb / (refdiv * pd1 * pd2)
}

/// Fracao (numerador, denominador) que leva o GPLL a 12,288 MHz.
fn fracao_mclk() -> (u32, u32) {
    let (mut n, mut d) = (12_288u64, gpll_khz().max(1) as u64);
    let mdc = {
        let (mut a, mut b) = (n, d);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a
    };
    n /= mdc;
    d /= mdc;
    while d > 0xFFFF || n > 0xFFFF {
        n = (n + 1) / 2;
        d = (d + 1) / 2;
    }
    (n as u32, d as u32)
}

/// Relogio TMDS em kHz pela resolucao que o HDMI esta mandando.
fn tmds_khz() -> u32 {
    let linhas = (hdmi(0x1006) as u32) << 8 | hdmi(0x1005) as u32;
    let colunas = (hdmi(0x1002) as u32) << 8 | hdmi(0x1001) as u32;
    match (colunas, linhas) {
        (3840, 2160) => 297_000,
        (1920, 1080) => 148_500,
        (1280, 720) => 74_250,
        (720, 576) | (720, 480) => 27_000,
        _ => 148_500,
    }
}

/// Liga relogios, I2S e o audio do HDMI. true = pronto.
pub fn iniciar() -> bool {
    if ligado() {
        return true;
    }
    // MCLK: i2s0_src = GPLL/1, i2s0_pre = fracionario (32/3125 com o GPLL
    // de 1200 MHz do Android; calculado pelo GPLL de verdade); portoes
    // (GATE0 bits 3..5) e hclk_i2s0 (GATE8 bit 7)
    cru(0x068, 1 << 15 | 3 << 8 | 0x7F, 1 << 15 | 1 << 8);
    let (num, den) = fracao_mclk();
    unsafe { write_volatile((CRU + 0x064) as *mut u32, num << 16 | den) };
    cru(0x0D0, 7 << 3, 0);
    cru(0x0F0, 1 << 7, 0);
    crate::cpu::barreira();

    // I2S parado e limpo, configurado como o Android, sem DMA
    i2s_w(XFER, 0);
    i2s_w(CLR, 3);
    for _ in 0..1000 {
        if i2s(CLR) & 3 == 0 {
            break;
        }
        core::hint::spin_loop();
    }
    i2s_w(TXCR, 0x0F);
    i2s_w(CKR, 0x0001_7F7F);
    i2s_w(DMACR, 0);
    // FIFO cheia de silencio antes de ligar
    for _ in 0..32 {
        i2s_w(TXDR, 0);
    }

    // HDMI: relogio do audio ligado, I2S na entrada 0, 16 bits, N/CTS
    let clkdis = hdmi(0x4001);
    hdmi_w(0x4001, clkdis & !0x08);
    let n: u32 = 6144;
    let cts = tmds_khz();
    hdmi_w(0x3205, 0x10 | (cts >> 16 & 0x0F) as u8);
    hdmi_w(0x3204, (cts >> 8) as u8);
    hdmi_w(0x3203, cts as u8);
    hdmi_w(0x3202, 0x80 | (n >> 16 & 0x0F) as u8);
    hdmi_w(0x3201, (n >> 8) as u8);
    hdmi_w(0x3200, n as u8);
    hdmi_w(0x3100, 0x21);
    hdmi_w(0x3101, 0x10);
    hdmi_w(0x3102, 0x0C);
    hdmi_w(0x3103, 0);
    hdmi_w(0x3104, 0x10);
    hdmi_w(0x3206, 0);
    // infoframe de audio: 2 canais
    hdmi_w(0x1025, 0x10);
    hdmi_w(0x1026, 0);
    hdmi_w(0x1027, 0);
    hdmi_w(0x1028, 0);
    // modo HDMI (no DVI o som nao passa)
    let inv = hdmi(0x1000);
    if inv & 0x08 == 0 {
        hdmi_w(0x1000, inv | 0x08);
    }
    // reset da FIFO de audio do HDMI (o bit volta sozinho)
    hdmi_w(0x4002, !0x08u8);
    crate::cpu::barreira();

    CABECA.store(0, Ordering::Relaxed);
    CAUDA.store(0, Ordering::Relaxed);
    LIGADO.store(true, Ordering::Relaxed);
    // interrupcao com a FIFO em 16 ou menos; liga a transmissao
    i2s_w(INTCR, 16 << 4 | 1);
    crate::gic::habilitar(IRQ, false);
    i2s_w(XFER, 1);
    true
}

/// Da IRQ do I2S: enche a FIFO.
pub fn atender() {
    IRQS.fetch_add(1, Ordering::Relaxed);
    let nivel = (i2s(FIFOLR) & 0x3F) as usize;
    let cabeca = CABECA.load(Ordering::Acquire);
    let mut cauda = CAUDA.load(Ordering::Relaxed);
    let buf = &raw const BUF;
    let mut faltou = false;
    let pausado = PAUSADO.load(Ordering::Relaxed);
    for _ in nivel..32 {
        let v = if pausado {
            0
        } else if cauda != cabeca {
            let v = unsafe { (*buf)[cauda & (ANEL - 1)] };
            cauda = cauda.wrapping_add(1);
            v
        } else {
            faltou = true;
            0
        };
        i2s_w(TXDR, v);
    }
    let antes = CAUDA.swap(cauda, Ordering::Release);
    TOCADOS.fetch_add(cauda.wrapping_sub(antes) as u32, Ordering::Relaxed);
    if faltou && cabeca != antes {
        FALTAS.fetch_add(1, Ordering::Relaxed);
    }
}

/// Quadros que cabem no anel agora.
pub fn espaco() -> usize {
    ANEL - 1 - CABECA.load(Ordering::Relaxed).wrapping_sub(CAUDA.load(Ordering::Acquire))
}

/// Quadros no anel ainda nao tocados (para sincronizar o video).
pub fn na_fila() -> usize {
    CABECA.load(Ordering::Relaxed).wrapping_sub(CAUDA.load(Ordering::Acquire))
}

/// Quadros que ja sairam pelo HDMI desde que ligou.
pub fn tocados() -> u32 {
    TOCADOS.load(Ordering::Relaxed)
}

/// Poe amostras estereo S16 intercaladas (48 kHz). Devolve quantos
/// quadros couberam.
pub fn escrever(amostras: &[i16]) -> usize {
    if !ligado() {
        return 0;
    }
    let n = (amostras.len() / 2).min(espaco());
    let mut cabeca = CABECA.load(Ordering::Relaxed);
    let buf = &raw mut BUF;
    for k in 0..n {
        let (e, d) = (amostras[2 * k] as u16 as u32, amostras[2 * k + 1] as u16 as u32);
        unsafe { (*buf)[cabeca & (ANEL - 1)] = e | d << 16 };
        cabeca = cabeca.wrapping_add(1);
    }
    CABECA.store(cabeca, Ordering::Release);
    n
}

pub fn pausar(sim: bool) {
    PAUSADO.store(sim, Ordering::Relaxed);
}

/// Joga fora o que esta na fila (pulo, troca de video).
pub fn esvaziar() {
    CABECA.store(CAUDA.load(Ordering::Acquire), Ordering::Release);
}

/// Para o "som" do shell e o INFO.
pub fn relatorio() -> alloc::string::String {
    alloc::format!(
        "hdmi som {}: {} quadros tocados, {} na fila, {} faltas, {} irqs, fifo {}, tmds {} kHz, gpll {} kHz, HDMI conf0 {:02x} n {:02x}{:02x} modo {:02x}",
        if ligado() { "ligado" } else { "desligado" },
        tocados(),
        na_fila(),
        FALTAS.load(Ordering::Relaxed),
        IRQS.load(Ordering::Relaxed),
        i2s(FIFOLR) & 0x3F,
        tmds_khz(),
        gpll_khz(),
        hdmi(0x3100),
        hdmi(0x3201),
        hdmi(0x3200),
        hdmi(0x1000)
    )
}
