//! Acesso direto a registradores do processador ARM64.

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

pub fn nome_cpu(midr: u64) -> &'static str {
    let fabricante = (midr >> 24) & 0xFF;
    let modelo = (midr >> 4) & 0xFFF;
    match (fabricante, modelo) {
        (0x41, 0xD03) => "ARM Cortex-A53",
        (0x41, 0xD04) => "ARM Cortex-A35",
        (0x41, 0xD05) => "ARM Cortex-A55",
        (0x41, 0xD07) => "ARM Cortex-A57",
        (0x41, 0xD08) => "ARM Cortex-A72",
        (0x41, _) => "ARM (modelo desconhecido)",
        _ => "desconhecido",
    }
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
