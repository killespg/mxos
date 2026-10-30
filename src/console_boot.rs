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
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::fonte::{MONO, MONO_ALTURA, MONO_BASE, MONO_LARGURA};

static LIGADO: AtomicBool = AtomicBool::new(false);
/// Endereco da linha 0 da tela (o YRGB_MST do VOP).
static BASE: AtomicUsize = AtomicUsize::new(0);
/// Espelhado na vertical: a linha y fica y passos ABAIXO da base na
/// memoria (o U-Boot da box mostra o BMP do logo direto, de baixo para cima).
static ESPELHO: AtomicBool = AtomicBool::new(false);
static PASSO: AtomicUsize = AtomicUsize::new(0);
static LARGURA: AtomicUsize = AtomicUsize::new(0);
static ALTURA: AtomicUsize = AtomicUsize::new(0);
/// Bytes por pixel: 4 (XRGB), 3 (RGB) ou 2 (RGB565).
static BPP: AtomicUsize = AtomicUsize::new(0);
static COLUNA: AtomicUsize = AtomicUsize::new(0);
/// Quantas vezes algo foi escrito (vai para o registrador de rastro).
static ESCRITAS: AtomicUsize = AtomicUsize::new(0);
static LINHA: AtomicUsize = AtomicUsize::new(0);

const MARGEM: usize = 8;
const COR: u32 = 0x00D0_D0D0;
const COR_ERRO: u32 = 0x00FF_6060;

#[cfg(feature = "rk322x")]
fn janela_do_uboot() -> Option<(usize, usize, usize, usize, usize, bool)> {
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
        // o U-Boot da box deixa o logo num endereco impar (0x9dea2136): com a
        // MMU desligada, escrita desalinhada e falha (e o U-Boot reinicia)
        let sane = (320..=4096).contains(&largura)
            && (200..=2304).contains(&altura)
            && passo >= largura * bpp
            && base % match bpp { 4 => 4, 2 => 2, _ => 1 } == 0;
        // Medido na box (2026-10-29): MST 0x9dea2136, passo 3840, 1280x720,
        // RGB888 = 0x9dc00000 (logo.bmp) + 54 (cabecalho do BMP) + 719
        // linhas: a imagem esta de baixo para cima e o VOP espelha. Quando
        // para cima nao cabe na RAM e para baixo cabe, e espelho.
        let reto = crate::plataforma::na_ram(base, base + passo * altura);
        let espelho = !reto && base >= (altura - 1) * passo
            && crate::plataforma::na_ram(base - (altura - 1) * passo, base + passo);
        if sane && (reto || espelho) {
            return Some((base, passo, largura, altura, bpp, espelho));
        }
    }
    None
}

#[cfg(not(feature = "rk322x"))]
fn janela_do_uboot() -> Option<(usize, usize, usize, usize, usize, bool)> {
    None
}

/// Rastro para depois de um reset quente: GRF_OS_REG1 e OS_REG2 da RK322x
/// (o U-Boot so mexe no OS_REG0, a marca de boot). Ler no Android com
/// `busybox devmem 0x110005cc 32` e `0x110005d4`. 1 a 3 = etapas do
/// boot_arm.s; 0x100 + n = n-esima escrita no console; 0xDEAD0000 + linha =
/// panico; 0xE0000000 + tipo = excecao (OS_REG2 = instrucao que falhou).
pub fn rastro(valor: u32, endereco: u32) {
    alimentar();
    #[cfg(feature = "rk322x")]
    unsafe {
        core::ptr::write_volatile(0x1100_05CC as *mut u32, valor);
        if endereco != 0 {
            core::ptr::write_volatile(0x1100_05D4 as *mut u32, endereco);
        }
    }
    let _ = (valor, endereco);
}

/// Fim de um erro fatal na TV box: deixa a mensagem uns segundos na tela e
/// reinicia quente pela CRU. O rastro fica no GRF e a box volta sozinha
/// para o Android, onde da para ler o que aconteceu (sem cabo serial).
pub fn reiniciar_depois_de_erro() {
    #[cfg(feature = "rk322x")]
    {
        let fim = crate::cpu::contador() + crate::cpu::frequencia_timer() * 8;
        while crate::cpu::contador() < fim {
            core::hint::spin_loop();
        }
        unsafe { core::ptr::write_volatile(0x110E_01F0 as *mut u32, 0xFDB9) };
    }
}

/// Onde a CPU estava quando a ultima IRQ chegou (vetores_arm.s grava).
#[unsafe(no_mangle)]
pub static ULTIMO_PC_IRQ: AtomicU32 = AtomicU32::new(0);
/// Tique da ultima vez que o laco principal (ou uma linha do boot) deu sinal.
static ULTIMO_SINAL: AtomicU64 = AtomicU64::new(0);
/// Sem sinal por tanto tempo e travamento: grava onde estava e reinicia.
const TRAVADO_S: u64 = 60;

/// Da IRQ do timer: o laco principal parou de dar sinal por 60 s? Entao
/// grava no rastro 0xB0B0_0000 e o endereco da instrucao em que a CPU
/// estava (OS_REG2) e reinicia; o Android le depois. So na TV box.
pub fn vigiar_travamento() {
    if !cfg!(feature = "rk322x") {
        return;
    }
    let ultimo = ULTIMO_SINAL.load(Ordering::Relaxed);
    if ultimo != 0 && crate::timer::tiques() > ultimo + TRAVADO_S * crate::timer::HZ {
        rastro(0xB0B0_0000, ULTIMO_PC_IRQ.load(Ordering::Relaxed));
        #[cfg(feature = "rk322x")]
        unsafe {
            core::ptr::write_volatile(0x110E_01F0 as *mut u32, 0xFDB9);
        }
    }
}

/// Alimenta o watchdog que o boot_arm.s ligou (~28 s): cada linha do boot e
/// cada volta do laco principal. Travou, a placa reinicia sozinha.
pub fn alimentar() {
    ULTIMO_SINAL.store(crate::timer::tiques().max(1), Ordering::Relaxed);
    #[cfg(feature = "rk322x")]
    unsafe {
        core::ptr::write_volatile(0x110A_000C as *mut u32, 0x76);
    }
}

/// Primeira coisa do kernel_main: acha o framebuffer do U-Boot e limpa.
pub fn iniciar() -> bool {
    rastro(0x10, 0); // entrou no Rust
    let Some((base, passo, largura, altura, bpp, espelho)) = janela_do_uboot() else {
        rastro(0x12, 0); // sem tela do U-Boot
        return false;
    };
    rastro(0x11, base as u32);
    BASE.store(base, Ordering::Relaxed);
    PASSO.store(passo, Ordering::Relaxed);
    LARGURA.store(largura, Ordering::Relaxed);
    ALTURA.store(altura, Ordering::Relaxed);
    BPP.store(bpp, Ordering::Relaxed);
    ESPELHO.store(espelho, Ordering::Relaxed);
    let (inicio, fim) = memoria();
    unsafe { core::ptr::write_bytes(inicio as *mut u8, 0, fim - inicio) };
    crate::mmu::limpar_cache(inicio, fim - inicio);
    LIGADO.store(true, Ordering::Release);
    true
}

/// Endereco do comeco da linha `y` da tela.
fn linha_tela(y: usize) -> usize {
    let (base, passo) = (BASE.load(Ordering::Relaxed), PASSO.load(Ordering::Relaxed));
    if ESPELHO.load(Ordering::Relaxed) { base - y * passo } else { base + y * passo }
}

/// [inicio, fim) da memoria da imagem inteira.
fn memoria() -> (usize, usize) {
    let (passo, altura) = (PASSO.load(Ordering::Relaxed), ALTURA.load(Ordering::Relaxed));
    let (a, b) = (linha_tela(0), linha_tela(altura - 1));
    (a.min(b), a.max(b) + passo)
}

/// Limpa do cache as linhas de pixel [y, y + n) da tela.
fn mandar_linhas(y: usize, n: usize) {
    let passo = PASSO.load(Ordering::Relaxed);
    let (a, b) = (linha_tela(y), linha_tela(y + n - 1));
    crate::mmu::limpar_cache(a.min(b), a.max(b) - a.min(b) + passo);
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
    ligado().then(memoria)
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
    let largura = LARGURA.load(Ordering::Relaxed) * BPP.load(Ordering::Relaxed);
    let y0 = MARGEM + linha * MONO_ALTURA as usize;
    for y in y0..y0 + MONO_ALTURA as usize {
        unsafe { core::ptr::write_bytes(linha_tela(y) as *mut u8, 0, largura) };
    }
    mandar_linhas(y0, MONO_ALTURA as usize);
}

fn caractere(c: char, coluna: usize, linha: usize, cor: u32) {
    let bpp = BPP.load(Ordering::Relaxed);
    let g = MONO.glifo(c);
    let x0 = (MARGEM + coluna * MONO_LARGURA as usize) as i32 + g.dx;
    let y0 = (MARGEM + linha * MONO_ALTURA as usize) as i32 + MONO_BASE - g.dy;
    for y in 0..g.altura {
        for x in 0..g.largura {
            let a = g.alfa(x, y);
            let (px, py) = (x0 + x, y0 + y);
            if a != 0 && px >= 0 && py >= 0 {
                pixel(linha_tela(py as usize) + px as usize * bpp, bpp, misturar(cor, a));
            }
        }
    }
}

fn nova_linha() {
    let linhas = (ALTURA.load(Ordering::Relaxed) - 2 * MARGEM) / MONO_ALTURA as usize;
    let linha = LINHA.load(Ordering::Relaxed);
    mandar_linhas(MARGEM + linha * MONO_ALTURA as usize, MONO_ALTURA as usize);
    // tela cheia: volta para cima, apagando sempre a linha seguinte
    let proxima = (linha + 1) % linhas.max(1);
    limpar_linha(proxima);
    limpar_linha((proxima + 1) % linhas.max(1));
    LINHA.store(proxima, Ordering::Relaxed);
    COLUNA.store(0, Ordering::Relaxed);
}

fn escrever_com(texto: &str, cor: u32) {
    // load + store, nunca fetch_add: antes da MMU toda a memoria e
    // "strongly-ordered" e o LDREX/STREX do ARMv7 pode falhar para sempre
    // (so o nucleo 0 roda no boot, entao nao ha corrida)
    let n = ESCRITAS.load(Ordering::Relaxed) + 1;
    ESCRITAS.store(n, Ordering::Relaxed);
    if cor == COR {
        rastro(0x100 + n as u32, 0);
    }
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
                let coluna = COLUNA.load(Ordering::Relaxed);
                COLUNA.store(coluna + 1, Ordering::Relaxed);
                caractere(c, coluna, LINHA.load(Ordering::Relaxed), cor);
            }
        }
    }
    // a linha atual tambem tem de chegar a memoria para o VOP ver
    mandar_linhas(MARGEM + LINHA.load(Ordering::Relaxed) * MONO_ALTURA as usize, MONO_ALTURA as usize);
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
