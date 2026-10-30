//! Console de boot da TV box: as mensagens do boot na tela, como o Linux
//! faz, desde a primeira instrucao em Rust ate o video do MXOS assumir.
//!
//! Escreve direto no framebuffer que o U-Boot deixou no HDMI (o do logo):
//! le no VOP qual janela esta ligada, o endereco, o formato e o passo da
//! linha. Nao usa heap nem MMU (funciona antes das duas), e o panico e as
//! excecoes tambem escrevem aqui: se o boot travar ou cair, a ultima linha
//! na tela diz onde. Quando o video de verdade liga, este se desliga.
//!
//! Fora da TV box tudo aqui e vazio.

use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::fonte::{MONO, MONO_ALTURA, MONO_BASE, MONO_LARGURA};

static LIGADO: AtomicBool = AtomicBool::new(false);
static BASE: AtomicUsize = AtomicUsize::new(0);
static PASSO: AtomicUsize = AtomicUsize::new(0);
static LARGURA: AtomicUsize = AtomicUsize::new(0);
static ALTURA: AtomicUsize = AtomicUsize::new(0);
/// Bytes por pixel: 4 (XRGB), 3 (RGB) ou 2 (RGB565).
static BPP: AtomicUsize = AtomicUsize::new(0);
static COLUNA: AtomicUsize = AtomicUsize::new(0);
static LINHA: AtomicUsize = AtomicUsize::new(0);

const MARGEM: usize = 8;
const COR: u32 = 0x00D0_D0D0;
const COR_ERRO: u32 = 0x00FF_6060;

#[cfg(feature = "rk322x")]
fn janela_do_uboot() -> Option<(usize, usize, usize, usize, usize)> {
    use core::ptr::read_volatile;
    const VOP: usize = 0x2005_0000;
    let ler = |r: usize| unsafe { read_volatile((VOP + r) as *const u32) };
    // janelas 0 e 1 do VOP (deslocamentos do rk3288_vop_win_data do Linux)
    for janela in [0x30usize, 0x70] {
        let ctrl = ler(janela);
        if ctrl & 1 == 0 {
            continue;
        }
        let bpp = match ctrl >> 1 & 7 {
            0 => 4,
            1 => 3,
            2 => 2,
            _ => continue,
        };
        let passo = (ler(janela + 0x0C) & 0x3FFF) as usize * 4;
        let base = ler(janela + 0x10) as usize;
        let act = ler(janela + 0x18);
        let (largura, altura) = ((act & 0x1FFF) as usize + 1, (act >> 16 & 0x1FFF) as usize + 1);
        let sane = (320..=4096).contains(&largura) && (200..=2304).contains(&altura) && passo >= largura * bpp;
        let na_ram = base >= crate::plataforma::RAM_INICIO && base + passo * altura <= crate::plataforma::RAM_FIM;
        if sane && na_ram {
            return Some((base, passo, largura, altura, bpp));
        }
    }
    None
}

#[cfg(not(feature = "rk322x"))]
fn janela_do_uboot() -> Option<(usize, usize, usize, usize, usize)> {
    None
}

/// Primeira coisa do kernel_main: acha o framebuffer do U-Boot e limpa.
pub fn iniciar() -> bool {
    let Some((base, passo, largura, altura, bpp)) = janela_do_uboot() else { return false };
    BASE.store(base, Ordering::Relaxed);
    PASSO.store(passo, Ordering::Relaxed);
    LARGURA.store(largura, Ordering::Relaxed);
    ALTURA.store(altura, Ordering::Relaxed);
    BPP.store(bpp, Ordering::Relaxed);
    unsafe { core::ptr::write_bytes(base as *mut u8, 0, passo * altura) };
    crate::mmu::limpar_cache(base, passo * altura);
    LIGADO.store(true, Ordering::Release);
    true
}

/// O video de verdade assumiu (o VOP aponta para outro lugar).
#[cfg_attr(not(feature = "rk322x"), allow(dead_code))]
pub fn desligar() {
    LIGADO.store(false, Ordering::Release);
}

pub fn ligado() -> bool {
    LIGADO.load(Ordering::Acquire)
}

/// Memoria do framebuffer do U-Boot, que o heap nao pode usar enquanto o
/// console estiver ligado.
pub fn faixa() -> Option<(usize, usize)> {
    ligado().then(|| {
        let base = BASE.load(Ordering::Relaxed);
        (base, base + PASSO.load(Ordering::Relaxed) * ALTURA.load(Ordering::Relaxed))
    })
}

fn pixel(endereco: usize, bpp: usize, cor: u32) {
    unsafe {
        match bpp {
            4 => core::ptr::write_volatile(endereco as *mut u32, cor),
            3 => {
                let p = endereco as *mut u8;
                p.write_volatile(cor as u8);
                p.add(1).write_volatile((cor >> 8) as u8);
                p.add(2).write_volatile((cor >> 16) as u8);
            }
            _ => {
                let (r, g, b) = (cor >> 19 & 0x1F, cor >> 10 & 0x3F, cor >> 3 & 0x1F);
                core::ptr::write_volatile(endereco as *mut u16, (r << 11 | g << 5 | b) as u16);
            }
        }
    }
}

/// Cor `cor` escurecida pelo alfa do glifo (fundo preto).
fn misturar(cor: u32, alfa: u8) -> u32 {
    let a = alfa as u32;
    ((cor >> 16 & 0xFF) * a / 255) << 16 | ((cor >> 8 & 0xFF) * a / 255) << 8 | (cor & 0xFF) * a / 255
}

fn limpar_linha(linha: usize) {
    let (base, passo) = (BASE.load(Ordering::Relaxed), PASSO.load(Ordering::Relaxed));
    let inicio = base + (MARGEM + linha * MONO_ALTURA as usize) * passo;
    unsafe { core::ptr::write_bytes(inicio as *mut u8, 0, passo * MONO_ALTURA as usize) };
    crate::mmu::limpar_cache(inicio, passo * MONO_ALTURA as usize);
}

fn caractere(c: char, coluna: usize, linha: usize, cor: u32) {
    let (base, passo, bpp) = (BASE.load(Ordering::Relaxed), PASSO.load(Ordering::Relaxed), BPP.load(Ordering::Relaxed));
    let g = MONO.glifo(c);
    let x0 = (MARGEM + coluna * MONO_LARGURA as usize) as i32 + g.dx;
    let y0 = (MARGEM + linha * MONO_ALTURA as usize) as i32 + MONO_BASE - g.dy;
    for y in 0..g.altura {
        for x in 0..g.largura {
            let a = g.alfa(x, y);
            let (px, py) = (x0 + x, y0 + y);
            if a != 0 && px >= 0 && py >= 0 {
                pixel(base + py as usize * passo + px as usize * bpp, bpp, misturar(cor, a));
            }
        }
    }
}

fn nova_linha() {
    let linhas = (ALTURA.load(Ordering::Relaxed) - 2 * MARGEM) / MONO_ALTURA as usize;
    let (base, passo) = (BASE.load(Ordering::Relaxed), PASSO.load(Ordering::Relaxed));
    let linha = LINHA.load(Ordering::Relaxed);
    let topo = base + (MARGEM + linha * MONO_ALTURA as usize) * passo;
    crate::mmu::limpar_cache(topo, passo * MONO_ALTURA as usize);
    // tela cheia: volta para cima, apagando sempre a linha seguinte
    let proxima = (linha + 1) % linhas.max(1);
    limpar_linha(proxima);
    limpar_linha((proxima + 1) % linhas.max(1));
    LINHA.store(proxima, Ordering::Relaxed);
    COLUNA.store(0, Ordering::Relaxed);
}

fn escrever_com(texto: &str, cor: u32) {
    if !ligado() {
        return;
    }
    let colunas = (LARGURA.load(Ordering::Relaxed) - 2 * MARGEM) / MONO_LARGURA as usize;
    for c in texto.chars() {
        match c {
            '\n' => nova_linha(),
            '\r' => COLUNA.store(0, Ordering::Relaxed),
            '\x1b' => {}
            c => {
                if COLUNA.load(Ordering::Relaxed) >= colunas {
                    nova_linha();
                }
                let coluna = COLUNA.fetch_add(1, Ordering::Relaxed);
                caractere(c, coluna, LINHA.load(Ordering::Relaxed), cor);
            }
        }
    }
    // a linha atual tambem tem de chegar a memoria para o VOP ver
    let (base, passo) = (BASE.load(Ordering::Relaxed), PASSO.load(Ordering::Relaxed));
    let topo = base + (MARGEM + LINHA.load(Ordering::Relaxed) * MONO_ALTURA as usize) * passo;
    crate::mmu::limpar_cache(topo, passo * MONO_ALTURA as usize);
}

pub fn escrever(texto: &str) {
    escrever_com(texto, COR);
}

/// `write!` no console de boot (cinza) ou, com `Erro`, em vermelho.
pub struct Boot;
pub struct Erro;

impl fmt::Write for Boot {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        escrever_com(s, COR);
        Ok(())
    }
}

impl fmt::Write for Erro {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        escrever_com(s, COR_ERRO);
        Ok(())
    }
}
