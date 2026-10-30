//! RGA2 da RK322x (0x20060000): o acelerador 2D da Rockchip. Copia,
//! escala, preenche e converte YUV -> RGB direto na RAM, sem a CPU tocar
//! nos pixels (e o que o Android usa para compor a tela e mostrar video).
//!
//! Registradores e campos conferidos no driver rockchip-rga do Linux e na
//! box (versao 0x04117756 com os relogios ligados). Sem a MMU do RGA: os
//! enderecos sao fisicos, e o mapa do MXOS e identidade.
//!
//! Um trabalho = 32 palavras (0x100..0x17C) num buffer que o RGA le pela
//! RAM (CMD_BASE). A CPU espera o fim olhando o INT (sem IRQ: os trabalhos
//! levam de micro a poucos milissegundos).
//!
//! `iniciar` liga os relogios, le a versao e faz um teste (preenche e
//! copia um bloquinho e confere). Se algo nao bater, o RGA fica desligado
//! e quem chama segue pela CPU: nada aqui e obrigatorio.

use alloc::format;
use alloc::string::String;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU32, AtomicU8, Ordering};

const BASE: usize = 0x2006_0000;
const CRU: usize = 0x110E_0000;

const SYS_CTRL: usize = 0x000;
const CMD_CTRL: usize = 0x004;
const CMD_BASE: usize = 0x008;
const INT: usize = 0x010;
const VERSAO: usize = 0x028;

/// Palavras do trabalho (registrador 0x100 + 4 * i).
const MODO: usize = 0;
const SRC_INFO: usize = 1;
const SRC_Y: usize = 2;
const SRC_CB: usize = 3;
const SRC_CR: usize = 4;
const SRC_VIR: usize = 6;
const SRC_ACT: usize = 7;
const SRC_X_FATOR: usize = 8;
const SRC_Y_FATOR: usize = 9;
const SRC_FG: usize = 11;
const DST_INFO: usize = 14;
const DST_Y: usize = 15;
const DST_VIR: usize = 18;
const DST_ACT: usize = 19;

const MODO_BITBLT: u32 = 0;
const MODO_PREENCHER: u32 = 2;
/// gradient_sat (bit 6): o Linux sempre liga.
const MODO_SATURAR: u32 = 1 << 6;

const FMT_XBGR8888: u32 = 1;
const FMT_YUV420SP: u32 = 10;
const TROCA_RB: u32 = 1;
const TROCA_UV: u32 = 4;
const CSC_BT601: u32 = 1;
const CSC_BT709: u32 = 3;

/// Bit "comando terminado" no INT.
const INT_TERMINOU: u32 = 1 << 2;

/// Maior lado que o RGA aceita (act_width/height de 12 bits no destino).
pub const MAX_LADO: usize = 4096;

const DESCONHECIDO: u8 = 0;
const LIGADO: u8 = 1;
const DESLIGADO: u8 = 2;
static ESTADO: AtomicU8 = AtomicU8::new(DESCONHECIDO);
/// No preenchimento a cor vai com R e B trocados? (o teste descobre)
static COR_TROCADA: AtomicU8 = AtomicU8::new(0);
/// Onde o autoteste parou (para o comando "rga") e o que leu ali.
static PASSO: AtomicU8 = AtomicU8::new(0);
static LIDO: AtomicU32 = AtomicU32::new(0);
static ULTIMO_INT: AtomicU32 = AtomicU32::new(0);
/// Falhas seguidas (3 = desliga) e quantas no total.
static FALHAS: AtomicU8 = AtomicU8::new(0);
static FALHAS_TOTAL: AtomicU32 = AtomicU32::new(0);
/// Tentativas do autoteste que falharam (e onde parou a ultima): passo << 24 | n.
static FALHAS_TESTE: AtomicU32 = AtomicU32::new(0);
/// O ultimo trabalho que nao terminou (para o comando "rga").
static mut TRABALHO_RUIM: [u32; 32] = [0; 32];

const PASSOS: [&str; 7] = ["nao testado", "versao 0 (relogio desligado?)", "preencher nao terminou", "cor errada no preenchimento", "preencheu fora do retangulo", "copiar nao terminou", "copia errada"];

/// O trabalho que o RGA le (alinhado a linha de cache).
#[repr(C, align(64))]
struct Trabalho([u32; 32]);
static mut TRABALHO: Trabalho = Trabalho([0; 32]);

/// Uma imagem na RAM para o RGA (endereco fisico = virtual).
#[derive(Clone, Copy)]
pub struct Imagem {
    pub endereco: usize,
    /// Bytes por linha (multiplo de 4).
    pub passo: usize,
    pub largura: usize,
    pub altura: usize,
    pub formato: Formato,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Formato {
    /// u32 0x00RRGGBB (o pixel do MXOS).
    Xrgb8888,
    /// NV12: plano Y e depois o UV intercalado em `uv` (o que os
    /// decodificadores de video entregam). `bt709` escolhe a matriz.
    Nv12 { uv: usize, bt709: bool },
}

impl Imagem {
    pub fn xrgb(endereco: usize, passo: usize, largura: usize, altura: usize) -> Imagem {
        Imagem { endereco, passo, largura, altura, formato: Formato::Xrgb8888 }
    }
}

fn ler(r: usize) -> u32 {
    unsafe { read_volatile((BASE + r) as *const u32) }
}

fn escrever(r: usize, v: u32) {
    unsafe { write_volatile((BASE + r) as *mut u32, v) }
}

/// Escreve so os bits de `mascara` de um registrador da CRU (os 16 de cima
/// dizem quais dos 16 de baixo mudam).
fn cru(r: usize, mascara: u32, valor: u32) {
    unsafe { write_volatile((CRU + r) as *mut u32, mascara << 16 | valor) }
}

/// O RGA esta pronto para uso?
pub fn disponivel() -> bool {
    ESTADO.load(Ordering::Relaxed) == LIGADO
}

/// Liga os relogios, confere a versao e testa. true = pronto.
pub fn iniciar() -> bool {
    if ESTADO.load(Ordering::Relaxed) != DESCONHECIDO {
        return disponivel();
    }
    // relogios (portao em 1 = desligado): aclk_rga_pre (GATE1 bit 2),
    // sclk_rga (GATE3 bit 6), aclk/hclk_rga e o NOC do RGA (GATE13 bits
    // 0, 1 e 11). Divisores como o Android deixa: aclk_rga_pre = PLL/2
    // (CLKSEL33 bits 8..14) e sclk_rga = PLL/4 (CLKSEL22 bits 0..6)
    cru(0x0C8, 0x7F00, 0x0100);
    cru(0x09C, 0x007F, 0x0003);
    cru(0x0D4, 1 << 2, 0);
    cru(0x0DC, 1 << 6, 0);
    cru(0x104, 1 << 0 | 1 << 1 | 1 << 11, 0);
    crate::cpu::barreira();
    let versao = ler(VERSAO);
    if versao == 0 || versao == u32::MAX {
        PASSO.store(1, Ordering::Relaxed);
        ESTADO.store(DESLIGADO, Ordering::Relaxed);
        return false;
    }
    // o primeiro trabalho depois de ligar os relogios as vezes trava: ate
    // 3 tentativas (cada falha ja leva um reset em `executar`)
    for _ in 0..3 {
        ESTADO.store(LIGADO, Ordering::Relaxed);
        FALHAS.store(0, Ordering::Relaxed);
        if autoteste() {
            return true;
        }
        let n = (FALHAS_TESTE.load(Ordering::Relaxed) & 0xFF_FFFF) + 1;
        FALHAS_TESTE.store((PASSO.load(Ordering::Relaxed) as u32) << 24 | n, Ordering::Relaxed);
    }
    ESTADO.store(DESLIGADO, Ordering::Relaxed);
    false
}

/// Pronto para usar? Se desligou (teste do boot ou falhas), tenta ligar
/// de novo (no maximo uma vez a cada chamada de quem vai usar de verdade).
pub fn garantir() -> bool {
    if ESTADO.load(Ordering::Relaxed) == DESLIGADO {
        ESTADO.store(DESCONHECIDO, Ordering::Relaxed);
        return iniciar();
    }
    disponivel() || iniciar()
}

/// Versao do RGA (0 = desligado), para o "info".
pub fn versao() -> u32 {
    if disponivel() { ler(VERSAO) } else { 0 }
}

/// Preenche 16x16, copia para outro lugar com escala 2x e confere os dois.
/// Tambem descobre a ordem da cor no preenchimento.
fn autoteste() -> bool {
    #[repr(C, align(64))]
    struct Bloco([u32; 32 * 32]);
    static mut A: Bloco = Bloco([0; 32 * 32]);
    static mut B: Bloco = Bloco([0; 32 * 32]);
    let (a, b) = unsafe { (&raw mut A.0, &raw mut B.0) };
    let (pa, pb) = (a as usize, b as usize);
    let cor = 0x0011_2233u32;
    unsafe {
        (*a).fill(0);
        (*b).fill(0);
    }
    let falhou = |p: u8, lido: u32| {
        PASSO.store(p, Ordering::Relaxed);
        LIDO.store(lido, Ordering::Relaxed);
        false
    };
    let ia = Imagem::xrgb(pa, 32 * 4, 16, 16);
    if !preencher(&ia, 0, 0, 16, 16, cor) {
        return falhou(2, ULTIMO_INT.load(Ordering::Relaxed));
    }
    // o byte X (de cima) nao importa para a tela
    let lido = unsafe { read_volatile(&(*a)[5 * 32 + 7]) } & 0x00FF_FFFF;
    if lido != cor {
        let trocada = (cor & 0xFF) << 16 | (cor & 0xFF00) | (cor >> 16 & 0xFF);
        if lido != trocada {
            return falhou(3, lido);
        }
        COR_TROCADA.store(1, Ordering::Relaxed);
    }
    // fora do retangulo nao pode ter mexido
    let fora = unsafe { read_volatile(&(*a)[20]) };
    if fora != 0 {
        return falhou(4, fora);
    }
    unsafe { (*a)[0] = 0x00AB_CDEF };
    let ib = Imagem::xrgb(pb, 32 * 4, 32, 32);
    if !copiar(&ia, (0, 0, 16, 16), &ib, (0, 0, 32, 32)) {
        return falhou(5, ULTIMO_INT.load(Ordering::Relaxed));
    }
    let (canto, meio) = unsafe { (read_volatile(&(*b)[0]) & 0x00FF_FFFF, read_volatile(&(*b)[20 * 32 + 20]) & 0x00FF_FFFF) };
    if canto != 0x00AB_CDEF || meio != cor {
        return falhou(6, canto);
    }
    PASSO.store(0, Ordering::Relaxed);
    true
}

/// Testa de novo (mesmo depois de falhar) e conta o que viu: o comando
/// "rga" do shell.
pub fn relatorio() -> String {
    let falhas = FALHAS_TOTAL.load(Ordering::Relaxed);
    let ruim = unsafe { TRABALHO_RUIM };
    ESTADO.store(DESCONHECIDO, Ordering::Relaxed);
    FALHAS.store(0, Ordering::Relaxed);
    let ok = iniciar();
    let cru_ = |r: usize| unsafe { read_volatile((CRU + r) as *const u32) };
    let mut s = format!(
        "RGA {}: versao {:08x}, sys {:08x}, int {:08x}, status {:08x}
",
        if ok { "ok" } else { "desligado" },
        ler(VERSAO),
        ler(SYS_CTRL),
        ler(INT),
        ler(0x00C)
    );
    s += &format!("  portoes: GATE1 {:04x} GATE3 {:04x} GATE13 {:04x}  CLKSEL22 {:04x} CLKSEL33 {:04x}
", cru_(0x0D4), cru_(0x0DC), cru_(0x104), cru_(0x09C), cru_(0x0C8));
    if !ok {
        let p = PASSO.load(Ordering::Relaxed) as usize;
        s += &format!("  parou em: {} (lido {:08x})
", PASSOS.get(p).unwrap_or(&"?"), LIDO.load(Ordering::Relaxed));
    }
    let teste = FALHAS_TESTE.load(Ordering::Relaxed);
    if teste != 0 {
        s += &format!("  autoteste falhou {} vez(es); a ultima parou em: {}\n", teste & 0xFF_FFFF, PASSOS.get((teste >> 24) as usize).unwrap_or(&"?"));
    }
    if falhas > 0 {
        s += &format!("  {falhas} trabalho(s) nao terminaram; o ultimo:\n ");
        for (i, v) in ruim.iter().enumerate().filter(|(_, v)| **v != 0) {
            s += &format!(" {:03x}={v:08x}", 0x100 + 4 * i);
        }
        s += "\n";
    }
    if ok && COR_TROCADA.load(Ordering::Relaxed) != 0 {
        s += "  (cor do preenchimento com R e B trocados: corrigido)
";
    }
    s
}

/// Reset do RGA pela CRU (SOFTRST_CON6 bits 11, 13 e 14: RGA, aclk, hclk).
/// O do NOC (bit 12) fica de fora: com ele o RGA nao termina mais nada.
fn resetar() {
    unsafe {
        write_volatile((CRU + 0x128) as *mut u32, 0x6800 << 16 | 0x6800);
    }
    crate::cpu::barreira();
    for _ in 0..1000 {
        core::hint::spin_loop();
    }
    unsafe {
        write_volatile((CRU + 0x128) as *mut u32, 0x7800 << 16);
    }
    crate::cpu::barreira();
}

/// Manda o trabalho e espera (ate 100 ms). false = o RGA nao respondeu:
/// leva um reset, e depois de 3 falhas seguidas fica desligado.
fn executar(t: &[u32; 32]) -> bool {
    if !disponivel() {
        return false;
    }
    let p = &raw mut TRABALHO;
    unsafe { (*p).0 = *t };
    let endereco = p as usize;
    crate::mmu::limpar_cache(endereco, 128);
    escrever(CMD_BASE, endereco as u32);
    escrever(SYS_CTRL, 0);
    escrever(SYS_CTRL, 0x22);
    escrever(INT, 0x600);
    escrever(CMD_CTRL, 1);
    let freq = crate::cpu::frequencia_timer().max(1);
    let fim = crate::cpu::contador() + freq / 10;
    loop {
        let int = ler(INT);
        ULTIMO_INT.store(int, Ordering::Relaxed);
        if int & INT_TERMINOU != 0 {
            // limpa os pedidos (bits 4..7 = os mesmos de 0..3)
            escrever(INT, (int & !0xF0) | (int & 0xF) << 4);
            FALHAS.store(0, Ordering::Relaxed);
            return true;
        }
        if crate::cpu::contador() > fim {
            unsafe { TRABALHO_RUIM = *t };
            FALHAS_TOTAL.fetch_add(1, Ordering::Relaxed);
            resetar();
            if FALHAS.fetch_add(1, Ordering::Relaxed) >= 2 {
                ESTADO.store(DESLIGADO, Ordering::Relaxed);
            }
            return false;
        }
        core::hint::spin_loop();
    }
}

/// Fator de escala do RGA: o inverso normalizado em 2^16.
fn fator(origem: usize, destino: usize) -> (u32, u32) {
    // (modo: 0 sem, 1 reduz, 2 amplia; fator para o registrador)
    if origem == destino {
        (0, 0)
    } else if origem > destino {
        (1, (((destino << 16) / origem) as u32 + 1) & 0xFFFF)
    } else {
        (2, ((((origem - 1) << 16) / (destino - 1)) as u32) << 16)
    }
}

fn bytes_por_pixel(f: Formato) -> usize {
    match f {
        Formato::Xrgb8888 => 4,
        Formato::Nv12 { .. } => 1,
    }
}

/// Endereco do pixel (x, y) no plano principal.
fn canto(i: &Imagem, x: usize, y: usize) -> usize {
    i.endereco + y * i.passo + x * bytes_por_pixel(i.formato)
}

/// O destino num retangulo valido? (dentro da imagem e do limite do RGA)
fn cabe(i: &Imagem, x: usize, y: usize, l: usize, a: usize) -> bool {
    l > 0 && a > 0 && l <= MAX_LADO && a <= MAX_LADO && x + l <= i.largura && y + a <= i.altura && i.passo % 4 == 0
}

/// Linhas de cache da area [y, y + a) de uma imagem para a RAM (e fora do
/// cache, se `invalidar`: o RGA vai escrever ali).
fn sincronizar(i: &Imagem, y: usize, a: usize) {
    let inicio = i.endereco + y * i.passo;
    crate::mmu::limpar_cache(inicio, a * i.passo);
    if let Formato::Nv12 { uv, .. } = i.formato {
        crate::mmu::limpar_cache(uv + y / 2 * i.passo, a.div_ceil(2) * i.passo);
    }
}

/// Retangulo de uma cor so (XRGB).
pub fn preencher(destino: &Imagem, x: usize, y: usize, l: usize, a: usize, cor: u32) -> bool {
    if destino.formato != Formato::Xrgb8888 || !cabe(destino, x, y, l, a) {
        return false;
    }
    sincronizar(destino, y, a);
    let cor = if COR_TROCADA.load(Ordering::Relaxed) != 0 {
        (cor & 0xFF) << 16 | (cor & 0xFF00) | (cor >> 16 & 0xFF)
    } else {
        cor
    };
    let mut t = [0u32; 32];
    t[MODO] = MODO_PREENCHER | MODO_SATURAR;
    t[SRC_FG] = cor & 0x00FF_FFFF;
    t[DST_INFO] = FMT_XBGR8888 | TROCA_RB << 4;
    t[DST_Y] = canto(destino, x, y) as u32;
    t[DST_VIR] = (destino.passo / 4) as u32 & 0x7FFF;
    t[DST_ACT] = ((a - 1) << 16 | (l - 1)) as u32;
    executar(&t)
}

/// Copia o retangulo `de` (x, y, l, a) de `origem` para `para` em
/// `destino`, escalando se os tamanhos forem diferentes e convertendo
/// NV12 -> XRGB se a origem for NV12. O destino e sempre XRGB.
pub fn copiar(origem: &Imagem, de: (usize, usize, usize, usize), destino: &Imagem, para: (usize, usize, usize, usize)) -> bool {
    let (sx, sy, sl, sa) = de;
    let (dx, dy, dl, da) = para;
    if destino.formato != Formato::Xrgb8888 || !cabe(destino, dx, dy, dl, da) {
        return false;
    }
    if sl == 0 || sa == 0 || sx + sl > origem.largura || sy + sa > origem.altura || origem.passo % 4 != 0 || sl > 8192 || sa > 8192 {
        return false;
    }
    // escala de no maximo 16x para cada lado (limite do hardware)
    if sl > dl * 16 || dl > sl * 16 || sa > da * 16 || da > sa * 16 {
        return false;
    }
    sincronizar(origem, sy, sa);
    sincronizar(destino, dy, da);
    let (hmodo, xf) = fator(sl, dl);
    let (vmodo, yf) = fator(sa, da);
    let mut t = [0u32; 32];
    t[MODO] = MODO_BITBLT | MODO_SATURAR;
    let (fmt, troca, csc) = match origem.formato {
        Formato::Xrgb8888 => (FMT_XBGR8888, TROCA_RB, 0),
        Formato::Nv12 { bt709, .. } => (FMT_YUV420SP, 0, if bt709 { CSC_BT709 } else { CSC_BT601 }),
    };
    t[SRC_INFO] = fmt | troca << 4 | csc << 8 | hmodo << 14 | vmodo << 16;
    t[SRC_Y] = canto(origem, sx, sy) as u32;
    if let Formato::Nv12 { uv, .. } = origem.formato {
        // UV: meia altura, mesma largura em bytes (x par)
        let c = uv + sy / 2 * origem.passo + (sx & !1);
        t[SRC_CB] = c as u32;
        t[SRC_CR] = c as u32;
    }
    // bits 0..14: o passo em palavras (os de cima sao o passo da mascara)
    t[SRC_VIR] = (origem.passo / 4) as u32 & 0x7FFF;
    t[SRC_ACT] = ((sa - 1) << 16 | (sl - 1)) as u32;
    t[SRC_X_FATOR] = xf;
    t[SRC_Y_FATOR] = yf;
    t[DST_INFO] = FMT_XBGR8888 | TROCA_RB << 4;
    t[DST_Y] = canto(destino, dx, dy) as u32;
    t[DST_VIR] = (destino.passo / 4) as u32 & 0x7FFF;
    t[DST_ACT] = ((da - 1) << 16 | (dl - 1)) as u32;
    let _ = TROCA_UV;
    executar(&t)
}
