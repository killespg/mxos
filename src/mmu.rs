//! MMU: liga a memoria virtual.
//!
//! O mapa e "identidade" (endereco virtual = endereco fisico), mas cada
//! regiao ganha tipo e permissao:
//!   - perifericos: memoria de dispositivo, sem cache, nunca executa
//!   - RAM: memoria normal com cache, le e escreve, nunca executa
//!   - codigo do kernel: le e executa, nao escreve
//!   - constantes: so le
//!   - pagina de guarda e endereco 0: sem mapeamento (qualquer acesso falha)
//!
//! Paginas de 4 KB, enderecos virtuais de 39 bits (3 niveis de tabela):
//!   L1: 1 GB por entrada -> L2: 2 MB por entrada -> L3: 4 KB por entrada

use core::arch::asm;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::{mapa, plataforma};

const ENTRADAS: usize = 512;
const PAGINA: usize = 4096;
pub const BLOCO: usize = 2 * 1024 * 1024;
const BITS_VIRTUAIS: u64 = 39;

// Bits dos descritores das tabelas.
const VALIDO: u64 = 1 << 0;
const TABELA_OU_PAGINA: u64 = 1 << 1; // 0 = bloco de 2 MB
const ATTR_DISPOSITIVO: u64 = 0 << 2; // indice 0 do MAIR
const ATTR_NORMAL: u64 = 1 << 2; // indice 1 do MAIR
const SO_LEITURA: u64 = 1 << 7; // AP[2]
const COMPARTILHADA: u64 = 3 << 8; // inner shareable
const ACESSADA: u64 = 1 << 10; // flag AF (sem ela o primeiro acesso falha)
const NAO_EXECUTA_KERNEL: u64 = 1 << 53; // PXN
const NAO_EXECUTA_USUARIO: u64 = 1 << 54; // UXN
const NAO_EXECUTA: u64 = NAO_EXECUTA_KERNEL | NAO_EXECUTA_USUARIO;
const ENDERECO: u64 = 0x0000_FFFF_FFFF_F000;

// MAIR_EL1: tipos de memoria. 0 = Device-nGnRE, 1 = Normal write-back.
pub const MAIR_DISPOSITIVO: u8 = 0x04;
pub const MAIR_NORMAL: u8 = 0xFF;
const MAIR: u64 = MAIR_DISPOSITIVO as u64 | (MAIR_NORMAL as u64) << 8;

const DISPOSITIVO: u64 = ATTR_DISPOSITIVO | ACESSADA | NAO_EXECUTA;
const RAM: u64 = ATTR_NORMAL | COMPARTILHADA | ACESSADA | NAO_EXECUTA;
const CODIGO: u64 = ATTR_NORMAL | COMPARTILHADA | ACESSADA | SO_LEITURA | NAO_EXECUTA_USUARIO;
const CONSTANTES: u64 = ATTR_NORMAL | COMPARTILHADA | ACESSADA | SO_LEITURA | NAO_EXECUTA;

pub const MAX_TABELAS: usize = 8;

#[repr(C, align(4096))]
struct Tabela([u64; ENTRADAS]);

struct Tabelas(UnsafeCell<[Tabela; MAX_TABELAS]>);
// So o nucleo 0 mexe nas tabelas, uma vez, antes de ligar a MMU.
unsafe impl Sync for Tabelas {}

static TABELAS: Tabelas = Tabelas(UnsafeCell::new([const { Tabela([0; ENTRADAS]) }; MAX_TABELAS]));
static TABELAS_USADAS: AtomicUsize = AtomicUsize::new(0);

struct Mapeador {
    tabelas: &'static mut [Tabela; MAX_TABELAS],
    usadas: usize,
}

impl Mapeador {
    fn nova_tabela(&mut self) -> usize {
        assert!(self.usadas < MAX_TABELAS, "MMU: acabaram as tabelas de páginas");
        self.usadas += 1;
        self.usadas - 1
    }

    fn endereco(&self, indice: usize) -> u64 {
        &self.tabelas[indice] as *const Tabela as u64
    }

    fn indice_de(&self, descritor: u64) -> usize {
        ((descritor & ENDERECO) - self.endereco(0)) as usize / PAGINA
    }

    /// Tabela L2 que cobre `endereco` (cria se ainda nao existe).
    fn l2(&mut self, endereco: usize) -> usize {
        let i1 = endereco >> 30 & 0x1FF;
        let descritor = self.tabelas[0].0[i1];
        if descritor & VALIDO != 0 {
            return self.indice_de(descritor);
        }
        let nova = self.nova_tabela();
        self.tabelas[0].0[i1] = self.endereco(nova) | VALIDO | TABELA_OU_PAGINA;
        nova
    }

    /// Mapeia [inicio, fim) em blocos de 2 MB.
    fn mapear_blocos(&mut self, inicio: usize, fim: usize, atributos: u64) {
        assert!(inicio % BLOCO == 0 && fim % BLOCO == 0, "MMU: região fora do alinhamento de 2 MB");
        for endereco in (inicio..fim).step_by(BLOCO) {
            let l2 = self.l2(endereco);
            self.tabelas[l2].0[endereco >> 21 & 0x1FF] = endereco as u64 | atributos | VALIDO;
        }
    }

    /// Troca um bloco de 2 MB por 512 paginas de 4 KB, cada uma com sua permissao.
    fn mapear_paginas(&mut self, bloco: usize, permissao: impl Fn(usize) -> Option<u64>) {
        let l2 = self.l2(bloco);
        let l3 = self.nova_tabela();
        for i in 0..ENTRADAS {
            let endereco = bloco + i * PAGINA;
            self.tabelas[l3].0[i] = match permissao(endereco) {
                Some(atributos) => endereco as u64 | atributos | VALIDO | TABELA_OU_PAGINA,
                None => 0,
            };
        }
        self.tabelas[l2].0[bloco >> 21 & 0x1FF] = self.endereco(l3) | VALIDO | TABELA_OU_PAGINA;
    }
}

/// Monta as tabelas e liga a MMU e os caches. Chamar uma vez so, no boot.
pub fn ligar() {
    let k = mapa::kernel();
    let mut m = Mapeador { tabelas: unsafe { &mut *TABELAS.0.get() }, usadas: 0 };
    m.nova_tabela(); // tabela 0 = L1 (raiz)

    for &(inicio, fim) in plataforma::DISPOSITIVOS {
        m.mapear_blocos(inicio, fim, DISPOSITIVO);
    }
    m.mapear_blocos(plataforma::RAM_INICIO, plataforma::RAM_FIM, RAM);

    // O bloco de 2 MB onde o kernel mora ganha permissoes por pagina.
    let bloco = k.inicio & !(BLOCO - 1);
    assert!(k.fim <= bloco + BLOCO, "MMU: o kernel passou de 2 MB");
    m.mapear_paginas(bloco, |endereco| {
        if endereco < plataforma::RAM_INICIO || endereco >= plataforma::RAM_FIM || endereco == k.guarda {
            None
        } else if endereco >= k.inicio && endereco < k.codigo_fim {
            Some(CODIGO)
        } else if endereco >= k.codigo_fim && endereco < k.constantes_fim {
            Some(CONSTANTES)
        } else {
            Some(RAM)
        }
    });

    TABELAS_USADAS.store(m.usadas, Ordering::Relaxed);
    let raiz = m.endereco(0);
    limpar_cache(raiz as usize, m.usadas * PAGINA);
    ativar(raiz);
}

/// Garante que as tabelas estao na RAM, e nao presas em algum cache antigo
/// (o U-Boot rodou com cache ligado antes de nos).
fn limpar_cache(inicio: usize, tamanho: usize) {
    let ctr: u64;
    unsafe { asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack)) };
    let linha = 4 << (ctr >> 16 & 0xF);
    for endereco in (inicio..inicio + tamanho).step_by(linha) {
        unsafe { asm!("dc civac, {}", in(reg) endereco, options(nostack)) };
    }
    unsafe { asm!("dsb sy", options(nostack)) };
}

fn ativar(raiz: u64) {
    let mmfr0: u64;
    unsafe { asm!("mrs {}, id_aa64mmfr0_el1", out(reg) mmfr0, options(nomem, nostack)) };
    let bits_fisicos = (mmfr0 & 0xF).min(5); // PARange: o que a CPU suporta

    let tcr = (64 - BITS_VIRTUAIS) // T0SZ
        | 1 << 8                   // IRGN0: tabelas com cache write-back
        | 1 << 10                  // ORGN0: idem
        | 3 << 12                  // SH0: inner shareable
        | 0 << 14                  // TG0: paginas de 4 KB
        | 1 << 23                  // EPD1: sem TTBR1 (metade alta) por enquanto
        | bits_fisicos << 32; // IPS

    let ligar: u64 = 1 << 0 | 1 << 2 | 1 << 12; // SCTLR: M (MMU), C (cache de dados), I (cache de instrucoes)
    unsafe {
        asm!(
            "msr mair_el1, {mair}",
            "msr tcr_el1, {tcr}",
            "msr ttbr0_el1, {raiz}",
            "isb",
            "tlbi vmalle1",
            "ic iallu",
            "dsb ish",
            "isb",
            "mrs {tmp}, sctlr_el1",
            "orr {tmp}, {tmp}, {ligar}",
            "msr sctlr_el1, {tmp}",
            "isb",
            mair = in(reg) MAIR,
            tcr = in(reg) tcr,
            raiz = in(reg) raiz,
            ligar = in(reg) ligar,
            tmp = out(reg) _,
            options(nostack),
        );
    }
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
    let (sctlr, tcr, ttbr0): (u64, u64, u64);
    unsafe {
        asm!(
            "mrs {0}, sctlr_el1",
            "mrs {1}, tcr_el1",
            "mrs {2}, ttbr0_el1",
            out(reg) sctlr, out(reg) tcr, out(reg) ttbr0,
            options(nomem, nostack),
        );
    }
    const BITS_IPS: [u64; 6] = [32, 36, 40, 42, 44, 48];
    Estado {
        mmu: sctlr & 1 != 0,
        cache_dados: sctlr & 1 << 2 != 0,
        cache_instrucoes: sctlr & 1 << 12 != 0,
        bits_virtuais: 64 - (tcr & 0x3F),
        bits_fisicos: BITS_IPS[(tcr >> 32 & 7).min(5) as usize],
        raiz: ttbr0 & ENDERECO,
        tabelas: TABELAS_USADAS.load(Ordering::Relaxed),
    }
}

pub struct Traducao {
    pub fisico: u64,
    pub tipo_memoria: u8,
    pub escrita: bool,
}

/// Pergunta a propria MMU como ela traduz `virtual` (instrucao AT).
pub fn traduzir(virtual_: u64) -> Option<Traducao> {
    let (leitura, escrita): (u64, u64);
    unsafe {
        asm!(
            "at s1e1r, {va}",
            "isb",
            "mrs {r}, par_el1",
            "at s1e1w, {va}",
            "isb",
            "mrs {w}, par_el1",
            va = in(reg) virtual_,
            r = out(reg) leitura,
            w = out(reg) escrita,
            options(nostack),
        );
    }
    if leitura & 1 != 0 {
        return None; // bit F: a traducao falhou
    }
    Some(Traducao {
        fisico: leitura & ENDERECO | virtual_ & 0xFFF,
        tipo_memoria: (leitura >> 56) as u8,
        escrita: escrita & 1 == 0,
    })
}
