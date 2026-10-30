//! MMU do ARMv7 (Cortex-A7 do RK322x): tabela de primeiro nivel com
//! secoes de 1 MB, mapa identidade (virtual = fisico), como o mmu.rs faz no
//! AArch64:
//!   - RAM: normal, com cache (write-back), compartilhavel
//!   - perifericos: device
//!   - o resto: sem mapeamento (ponteiro nulo da erro)
//!
//! Sem permissao por pagina no kernel e sem processos por enquanto: a parte
//! "de processos" da API (USUARIO_*, raiz_kernel...) existe so para o resto
//! do kernel compilar igual nas duas arquiteturas.

#![allow(dead_code)]

use core::arch::asm;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::plataforma;

const SECAO: usize = 1 << 20;
pub const BLOCO: usize = 2 * 1024 * 1024;
pub const ENTRADAS_TABELA: usize = 512;
pub const MAX_TABELAS: usize = 1;
pub const MAIR_DISPOSITIVO: u8 = 0x04;
pub const MAIR_NORMAL: u8 = 0xFF;

// descritor de secao (short-descriptor)
const TIPO_SECAO: u32 = 0b10;
const B: u32 = 1 << 2;
const C: u32 = 1 << 3;
const AP_TOTAL: u32 = 0b11 << 10;
const TEX1: u32 = 1 << 12;
const COMPARTILHADA: u32 = 1 << 16;
const XN: u32 = 1 << 4;
/// RAM: TEX=001, C=1, B=1 (write-back, write-allocate), compartilhavel.
const RAM: u32 = TIPO_SECAO | AP_TOTAL | TEX1 | C | B | COMPARTILHADA;
/// Periferico: device compartilhado (TEX=000, C=0, B=1), sem executar.
const DISPOSITIVO: u32 = TIPO_SECAO | AP_TOTAL | B | XN;

// valores da API do AArch64 (usados so pelos processos, que ainda nao rodam aqui)
pub const USUARIO_CODIGO: u64 = 0;
pub const USUARIO_DADOS: u64 = 0;
pub const USUARIO_LEITURA: u64 = 0;
pub const USUARIO_TUDO: u64 = 0;
pub const DESCRITOR_TABELA: u64 = 0;
pub const DESCRITOR_PAGINA: u64 = 0;

#[repr(C, align(16384))]
struct Tabela([u32; 4096]);

struct Global(UnsafeCell<Tabela>);

unsafe impl Sync for Global {}

static TABELA: Global = Global(UnsafeCell::new(Tabela([0; 4096])));
static RAIZ: AtomicUsize = AtomicUsize::new(0);

fn mapear(t: &mut Tabela, inicio: usize, fim: usize, atributos: u32) {
    let mut e = inicio & !(SECAO - 1);
    while e < fim {
        t.0[e >> 20] = e as u32 | atributos;
        e = match e.checked_add(SECAO) {
            Some(p) => p,
            None => break,
        };
    }
}

/// Monta o mapa identidade e liga MMU e caches (com o bit SMP do Cortex-A7,
/// que precisa vir antes do cache).
pub fn ligar() {
    let t = unsafe { &mut *TABELA.0.get() };
    for &(inicio, fim) in plataforma::DISPOSITIVOS {
        mapear(t, inicio, fim, DISPOSITIVO);
    }
    mapear(t, plataforma::RAM_INICIO, plataforma::RAM_FIM, RAM);
    for &(inicio, fim) in plataforma::RAM_EXTRA {
        mapear(t, inicio, fim, RAM);
    }
    // a secao de guarda abaixo da pilha fica sem mapeamento: estouro = excecao
    t.0[crate::mapa::kernel().guarda >> 20] = 0;
    let raiz = t as *const Tabela as usize;
    RAIZ.store(raiz, Ordering::Relaxed);
    limpar_cache(raiz, core::mem::size_of::<Tabela>());
    unsafe {
        asm!(
            // ACTLR.SMP: o cache entra na coerencia entre nucleos
            "mrc p15, 0, {t}, c1, c0, 1",
            "orr {t}, {t}, #0x40",
            "mcr p15, 0, {t}, c1, c0, 1",
            // so TTBR0, dominio 0 = cliente (vale o AP de cada secao)
            "mov {t}, #0",
            "mcr p15, 0, {t}, c2, c0, 2",
            "mov {t}, #1",
            "mcr p15, 0, {t}, c3, c0, 0",
            // tabela com cache (IRGN/RGN write-back) e compartilhavel
            "orr {t}, {raiz}, #0x4A",
            "mcr p15, 0, {t}, c2, c0, 0",
            "mov {t}, #0",
            "mcr p15, 0, {t}, c8, c7, 0", // TLBIALL
            "mcr p15, 0, {t}, c7, c5, 0", // ICIALLU
            "dsb",
            "isb",
            "mrc p15, 0, {t}, c1, c0, 0",
            "orr {t}, {t}, #0x1",   // M
            "orr {t}, {t}, #0x4",   // C
            "orr {t}, {t}, #0x800", // Z (previsao de saltos)
            "orr {t}, {t}, #0x1000", // I
            "mcr p15, 0, {t}, c1, c0, 0",
            "isb",
            t = out(reg) _,
            raiz = in(reg) raiz,
            options(nostack),
        );
    }
}

/// Limpa e invalida as linhas de cache de [inicio, inicio + tamanho)
/// (DCCIMVAC) ate a RAM.
pub fn limpar_cache(inicio: usize, tamanho: usize) {
    let linha = crate::cpu::tamanho_linha_cache();
    let mut e = inicio & !(linha - 1);
    while e < inicio + tamanho {
        unsafe { asm!("mcr p15, 0, {0}, c7, c14, 1", in(reg) e, options(nostack)) };
        e += linha;
    }
    unsafe { asm!("dsb", options(nostack)) };
}

pub fn sincronizar_codigo(inicio: usize, tamanho: usize) {
    limpar_cache(inicio, tamanho);
    unsafe { asm!("mcr p15, 0, {0}, c7, c5, 0", "dsb", "isb", in(reg) 0usize, options(nostack)) };
}

pub fn registradores() -> (u64, u64, u64) {
    (0, 0, RAIZ.load(Ordering::Relaxed) as u64)
}

pub struct Estado {
    pub mmu: bool,
    pub cache_dados: bool,
    pub cache_instrucoes: bool,
    pub bits_virtuais: u64,
    pub bits_fisicos: u64,
    pub raiz: u64,
    pub tabelas: usize,
}

pub fn estado() -> Estado {
    let sctlr: u32;
    unsafe { asm!("mrc p15, 0, {0}, c1, c0, 0", out(reg) sctlr, options(nomem, nostack)) };
    Estado {
        mmu: sctlr & 1 != 0,
        cache_dados: sctlr & 4 != 0,
        cache_instrucoes: sctlr & 0x1000 != 0,
        bits_virtuais: 32,
        bits_fisicos: 32,
        raiz: RAIZ.load(Ordering::Relaxed) as u64,
        tabelas: 1,
    }
}

pub struct Traducao {
    pub fisico: u64,
    pub tipo_memoria: u8,
    pub escrita: bool,
}

pub fn traduzir(virtual_: u64) -> Option<Traducao> {
    let t = unsafe { &*TABELA.0.get() };
    let e = t.0.get((virtual_ >> 20) as usize).copied()?;
    if e & 3 != TIPO_SECAO {
        return None;
    }
    let tipo = if e & C != 0 { MAIR_NORMAL } else { MAIR_DISPOSITIVO };
    Some(Traducao { fisico: (e & 0xFFF0_0000) as u64 | virtual_ & 0xF_FFFF, tipo_memoria: tipo, escrita: true })
}

// ------------------------------------------------------------ processos
// (ainda nao rodam no ARMv7: so para compilar)

pub fn raiz_kernel() -> [u64; ENTRADAS_TABELA] {
    [0; ENTRADAS_TABELA]
}

pub fn l2_kernel(_indice: usize) -> [u64; ENTRADAS_TABELA] {
    [0; ENTRADAS_TABELA]
}

pub fn usar_espaco(_raiz: u64, _asid: u16) {}

pub fn usar_espaco_do_kernel() {}

pub fn esquecer_asid(_asid: u16) {
    unsafe { asm!("mcr p15, 0, {0}, c8, c3, 0", "dsb", "isb", in(reg) 0usize, options(nostack)) };
}
