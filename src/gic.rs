//! GIC (Generic Interrupt Controller) versao 2: o GIC-400 da TV box e o GICv2
//! do QEMU. Ele recebe os sinais de interrupcao dos perifericos e avisa a CPU.
//!   - distribuidor: liga/desliga cada interrupcao, prioridade, qual CPU recebe
//!   - interface da CPU: "qual interrupcao chegou?" (IAR) e "terminei" (EOIR)
//!
//! Numeros (INTID): 0-15 SGI (entre CPUs), 16-31 PPI (de cada CPU, ex.: timer),
//! 32+ SPI (perifericos compartilhados, ex.: serial).

use core::ptr::{read_volatile, write_volatile};

use crate::plataforma::{GIC_CPU, GIC_DISTRIBUIDOR};

// Distribuidor (GICD_*)
const D_CONTROLE: usize = 0x000;
const D_TIPO: usize = 0x004;
const D_LIGAR: usize = 0x100; // ISENABLERn
const D_DESLIGAR: usize = 0x180; // ICENABLERn
const D_DESATIVAR: usize = 0x380; // ICACTIVERn
const D_PRIORIDADE: usize = 0x400; // IPRIORITYRn, 1 byte por interrupcao
const D_DESTINO: usize = 0x800; // ITARGETSRn, 1 byte por interrupcao
const D_CONFIGURACAO: usize = 0xC00; // ICFGRn, 2 bits por interrupcao

// Interface da CPU (GICC_*)
const C_CONTROLE: usize = 0x00;
const C_MASCARA: usize = 0x04; // PMR
const C_CONFIRMAR: usize = 0x0C; // IAR
const C_FINALIZAR: usize = 0x10; // EOIR

/// Prioridade das nossas interrupcoes (menor = mais urgente) e o limite
/// da mascara (so passa quem for mais urgente que ele). Mesmos valores do Linux.
const PRIORIDADE: u8 = 0xA0;
const MASCARA: u32 = 0xF0;

/// A partir deste numero o IAR quer dizer "nada pendente".
pub const ESPURIA: u32 = 1020;

fn ler(base: usize, offset: usize) -> u32 {
    unsafe { read_volatile((base + offset) as *const u32) }
}

fn escrever(base: usize, offset: usize, valor: u32) {
    unsafe { write_volatile((base + offset) as *mut u32, valor) }
}

fn escrever_byte(base: usize, offset: usize, valor: u8) {
    unsafe { write_volatile((base + offset) as *mut u8, valor) }
}

pub fn iniciar() {
    escrever(GIC_DISTRIBUIDOR, D_CONTROLE, 0); // desliga enquanto configura
    limpar();
    escrever(GIC_CPU, C_MASCARA, MASCARA);
    escrever(GIC_CPU, C_CONTROLE, 1);
    escrever(GIC_DISTRIBUIDOR, D_CONTROLE, 1);
}

/// A interface da CPU de outro nucleo (registradores bancados) e o timer
/// dele (PPI, tambem bancada no distribuidor).
#[cfg_attr(feature = "mxq", allow(dead_code))]
pub fn iniciar_nucleo() {
    escrever(GIC_CPU, C_MASCARA, MASCARA);
    escrever(GIC_CPU, C_CONTROLE, 1);
    habilitar(crate::plataforma::IRQ_TIMER, false);
}

/// Desliga e desativa tudo que o firmware (U-Boot) pode ter deixado ligado.
/// Uma interrupcao de nivel esquecida ligada viraria uma tempestade que
/// nunca deixa o tratador sair. Mesma limpeza que o Linux faz.
fn limpar() {
    // registrador 0 e bancado (da propria CPU): desliga so as PPI (16-31),
    // as SGI ficam como estao
    escrever(GIC_DISTRIBUIDOR, D_DESLIGAR, 0xFFFF_0000);
    escrever(GIC_DISTRIBUIDOR, D_DESATIVAR, 0xFFFF_FFFF);
    for n in 1..(linhas() / 32) as usize {
        escrever(GIC_DISTRIBUIDOR, D_DESLIGAR + n * 4, 0xFFFF_FFFF);
        escrever(GIC_DISTRIBUIDOR, D_DESATIVAR + n * 4, 0xFFFF_FFFF);
    }
}

/// Desliga a interrupcao `id`.
pub fn desabilitar(id: u32) {
    let id = id as usize;
    escrever(GIC_DISTRIBUIDOR, D_DESLIGAR + id / 32 * 4, 1 << (id % 32));
}

/// Liga a interrupcao `id` e manda para a CPU 0.
pub fn habilitar(id: u32, borda: bool) {
    // mudar o tipo (ICFGR) com ela ligada e imprevisivel na spec
    desabilitar(id);
    let id = id as usize;
    escrever_byte(GIC_DISTRIBUIDOR, D_PRIORIDADE + id, PRIORIDADE);
    if id >= 32 {
        // so SPI tem destino e tipo configuraveis; PPI ja e da propria CPU
        escrever_byte(GIC_DISTRIBUIDOR, D_DESTINO + id, 1);
        let offset = D_CONFIGURACAO + id / 16 * 4;
        let bit = 1 << (id % 16 * 2 + 1);
        let atual = ler(GIC_DISTRIBUIDOR, offset);
        escrever(GIC_DISTRIBUIDOR, offset, if borda { atual | bit } else { atual & !bit });
    }
    escrever(GIC_DISTRIBUIDOR, D_LIGAR + id / 32 * 4, 1 << (id % 32));
}

/// Pergunta qual interrupcao chegou. Devolve o IAR inteiro (passar para `finalizar`).
pub fn confirmar() -> u32 {
    ler(GIC_CPU, C_CONFIRMAR)
}

pub fn finalizar(iar: u32) {
    escrever(GIC_CPU, C_FINALIZAR, iar);
}

/// Quantas linhas de interrupcao este GIC tem.
pub fn linhas() -> u32 {
    ((ler(GIC_DISTRIBUIDOR, D_TIPO) & 0x1F) + 1) * 32
}
