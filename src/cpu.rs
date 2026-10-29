//! Acesso direto a registradores do processador: AArch64 (QEMU, Amlogic)
//! ou ARMv7 (Cortex-A7 do RK322x). O resto do kernel so ve estas funcoes.

#[cfg(target_arch = "aarch64")]
mod a64 {
use core::arch::asm;

macro_rules! ler_registrador {
    ($nome:literal) => {{
        let valor: u64;
        unsafe { asm!(concat!("mrs {}, ", $nome), out(reg) valor, options(nomem, nostack, preserves_flags)) };
        valor
    }};
}

/// Nivel de excecao atual (0 = apps, 1 = kernel, 2 = hypervisor, 3 = firmware).
pub fn el_atual() -> u64 {
    (ler_registrador!("CurrentEL") >> 2) & 3
}

/// Identificacao do processador (fabricante, modelo, revisao).
/// MPIDR_EL1: qual nucleo e este (Aff0 nos 8 bits de baixo).
pub fn mpidr() -> u64 {
    let v: u64;
    unsafe { asm!("mrs {}, mpidr_el1", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

pub fn midr() -> u64 {
    ler_registrador!("midr_el1")
}

/// Frequencia do timer do sistema, em Hz.
pub fn frequencia_timer() -> u64 {
    ler_registrador!("cntfrq_el0")
}

/// Ticks do timer desde que a maquina ligou.
pub fn contador() -> u64 {
    ler_registrador!("cntvct_el0")
}


/// Dispara uma excecao de proposito, para testar o tratador.
pub fn ponto_de_parada() {
    unsafe { asm!("brk #0x4d58", options(nomem, nostack)) }
}

// As tres abaixo NAO usam `nomem`: assim o compilador nao move leituras e
// escritas da memoria para antes ou depois delas (barreira do compilador).

/// Deixa as interrupcoes (IRQ) chegarem: limpa o bit I do DAIF.
pub fn ligar_interrupcoes() {
    unsafe { asm!("msr daifclr, #2", options(nostack, preserves_flags)) }
}

/// Bloqueia as interrupcoes (IRQ): liga o bit I do DAIF.
pub fn desligar_interrupcoes() {
    unsafe { asm!("msr daifset, #2", options(nostack, preserves_flags)) }
}

/// Deixa os erros assincronos (SError) chegarem: limpa o bit A do DAIF.
/// Sem isso uma escrita num endereco de hardware errado falharia calada.
pub fn ligar_erros_assincronos() {
    unsafe { asm!("msr daifclr, #4", options(nostack, preserves_flags)) }
}

/// Dorme ate chegar uma interrupcao. Mesmo com IRQ bloqueada no DAIF a CPU
/// acorda; a interrupcao so e atendida quando ligar de novo.
pub fn esperar_interrupcao() {
    unsafe { asm!("wfi", options(nostack, preserves_flags)) }
}

/// Para a CPU para sempre, gastando o minimo de energia.
pub fn parar() -> ! {
    // Bloqueia tudo (D, A, I, F) antes: com IRQ mascarada o WFE nao acorda
    // para atender ninguem, entao a CPU fica parada de verdade.
    unsafe { asm!("msr daifset, #0xf", options(nostack, preserves_flags)) }
    loop {
        unsafe { asm!("wfe", options(nomem, nostack)) }
    }
}

/// Menor linha de cache de dados: CTR_EL0.DminLine = log2 de palavras de 4 bytes.
pub fn tamanho_linha_cache() -> usize {
    let ctr: u64;
    unsafe { asm!("mrs {0}, ctr_el0", out(reg) ctr, options(nomem, nostack, preserves_flags)) };
    4 << (ctr >> 16 & 0xF)
}

/// Limpa uma linha de cache ate a RAM (dc cvac).
pub fn limpar_linha(endereco: usize) {
    unsafe { asm!("dc cvac, {0}", in(reg) endereco, options(nostack, preserves_flags)) };
}

/// Barreira de memoria completa (dsb sy).
pub fn barreira() {
    unsafe { asm!("dsb sy", options(nostack, preserves_flags)) };
}

/// Bloqueia a IRQ e devolve o estado de antes (para `restaurar_irq`).
pub fn guardar_e_desligar_irq() -> usize {
    let daif: u64;
    unsafe { asm!("mrs {0}, daif", "msr daifset, #2", out(reg) daif, options(nostack, preserves_flags)) };
    daif as usize
}

pub fn restaurar_irq(estado: usize) {
    unsafe { asm!("msr daif, {0}", in(reg) estado as u64, options(nostack, preserves_flags)) };
}
}

#[cfg(target_arch = "aarch64")]
pub use a64::*;

/// ARMv7 (Cortex-A7): os mesmos servicos pelos registradores do CP15.
#[cfg(target_arch = "arm")]
mod a32 {
use core::arch::asm;

/// Modo do processador como "nivel": 1 = kernel (SVC), 2 = hypervisor.
pub fn el_atual() -> u64 {
    let cpsr: u32;
    unsafe { asm!("mrs {0}, cpsr", out(reg) cpsr, options(nomem, nostack, preserves_flags)) };
    if cpsr & 0x1F == 0x1A { 2 } else { 1 }
}

pub fn mpidr() -> u64 {
    let v: u32;
    unsafe { asm!("mrc p15, 0, {0}, c0, c0, 5", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v as u64
}

pub fn midr() -> u64 {
    let v: u32;
    unsafe { asm!("mrc p15, 0, {0}, c0, c0, 0", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v as u64
}

pub fn frequencia_timer() -> u64 {
    let v: u32;
    unsafe { asm!("mrc p15, 0, {0}, c14, c0, 0", out(reg) v, options(nomem, nostack, preserves_flags)) };
    // o firmware da Rockchip as vezes nao preenche: 24 MHz e o do RK322x
    if v == 0 { 24_000_000 } else { v as u64 }
}

pub fn contador() -> u64 {
    let (baixo, alto): (u32, u32);
    unsafe { asm!("isb", "mrrc p15, 1, {0}, {1}, c14", out(reg) baixo, out(reg) alto, options(nomem, nostack, preserves_flags)) };
    (alto as u64) << 32 | baixo as u64
}

pub fn ponto_de_parada() {
    unsafe { asm!("bkpt #0x4d", options(nomem, nostack)) }
}

pub fn ligar_interrupcoes() {
    unsafe { asm!("cpsie i", options(nostack, preserves_flags)) }
}

pub fn desligar_interrupcoes() {
    unsafe { asm!("cpsid i", options(nostack, preserves_flags)) }
}

pub fn ligar_erros_assincronos() {
    unsafe { asm!("cpsie a", options(nostack, preserves_flags)) }
}

pub fn esperar_interrupcao() {
    unsafe { asm!("dsb", "wfi", options(nostack, preserves_flags)) }
}

pub fn parar() -> ! {
    unsafe { asm!("cpsid aif", options(nostack, preserves_flags)) }
    loop {
        unsafe { asm!("wfe", options(nomem, nostack)) }
    }
}

/// CTR.DminLine, como no AArch64.
pub fn tamanho_linha_cache() -> usize {
    let ctr: u32;
    unsafe { asm!("mrc p15, 0, {0}, c0, c0, 1", out(reg) ctr, options(nomem, nostack, preserves_flags)) };
    4 << (ctr >> 16 & 0xF)
}

/// DCCMVAC: limpa uma linha ate o ponto de coerencia.
pub fn limpar_linha(endereco: usize) {
    unsafe { asm!("mcr p15, 0, {0}, c7, c10, 1", in(reg) endereco, options(nostack, preserves_flags)) };
}

pub fn barreira() {
    unsafe { asm!("dsb", options(nostack, preserves_flags)) };
}

pub fn guardar_e_desligar_irq() -> usize {
    let cpsr: u32;
    unsafe { asm!("mrs {0}, cpsr", "cpsid i", out(reg) cpsr, options(nostack, preserves_flags)) };
    cpsr as usize
}

pub fn restaurar_irq(estado: usize) {
    if estado & 0x80 == 0 {
        unsafe { asm!("cpsie i", options(nostack, preserves_flags)) };
    }
}
}

#[cfg(target_arch = "arm")]
pub use a32::*;

pub fn nome_cpu(midr: u64) -> &'static str {
    let fabricante = (midr >> 24) & 0xFF;
    let modelo = (midr >> 4) & 0xFFF;
    match (fabricante, modelo) {
        (0x41, 0xC07) => "ARM Cortex-A7",
        (0x41, 0xD03) => "ARM Cortex-A53",
        (0x41, 0xD04) => "ARM Cortex-A35",
        (0x41, 0xD05) => "ARM Cortex-A55",
        (0x41, 0xD07) => "ARM Cortex-A57",
        (0x41, 0xD08) => "ARM Cortex-A72",
        (0x41, _) => "ARM (modelo desconhecido)",
        _ => "desconhecido",
    }
}
