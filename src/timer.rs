//! Timer virtual do ARM (CNTV_*): gera o "tique" de 100 Hz.
//!
//! O timer compara o contador virtual (CNTVCT_EL0) com um alvo (CNTV_CVAL_EL0)
//! e levanta a interrupcao IRQ_TIMER quando o contador passa do alvo.
//! A cada tique o alvo anda um periodo inteiro, contado a partir do alvo
//! anterior (e nao de "agora"), para o relogio nao atrasar com o tempo.

use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::{cpu, irq};

/// Tiques por segundo.
pub const HZ: u64 = 100;

// Bits do CNTV_CTL_EL0.
const LIGADO: u64 = 1 << 0; // ENABLE (bit 1 = IMASK fica 0: interrupcao liberada)
const DISPAROU: u64 = 1 << 2; // ISTATUS: o contador ja passou do alvo

/// Ticks do contador entre dois tiques (frequencia / HZ).
static PERIODO: AtomicU64 = AtomicU64::new(0);
static TIQUES: AtomicU64 = AtomicU64::new(0);

#[cfg(target_arch = "aarch64")]
fn escrever_alvo(valor: u64) {
    unsafe { asm!("msr cntv_cval_el0, {}", "isb", in(reg) valor, options(nostack, preserves_flags)) }
}

#[cfg(target_arch = "aarch64")]
fn ler_alvo() -> u64 {
    let valor: u64;
    unsafe { asm!("mrs {}, cntv_cval_el0", out(reg) valor, options(nomem, nostack, preserves_flags)) };
    valor
}

#[cfg(target_arch = "aarch64")]
fn ler_controle() -> u64 {
    let valor: u64;
    unsafe { asm!("mrs {}, cntv_ctl_el0", out(reg) valor, options(nomem, nostack, preserves_flags)) };
    valor
}

#[cfg(target_arch = "aarch64")]
fn escrever_controle(valor: u64) {
    unsafe { asm!("msr cntv_ctl_el0, {}", "isb", in(reg) valor, options(nostack, preserves_flags)) }
}

// ARMv7: os mesmos registradores do timer virtual pelo CP15
#[cfg(target_arch = "arm")]
fn escrever_alvo(valor: u64) {
    let (baixo, alto) = (valor as u32, (valor >> 32) as u32);
    unsafe { asm!("mcrr p15, 3, {0}, {1}, c14", "isb", in(reg) baixo, in(reg) alto, options(nostack, preserves_flags)) }
}

#[cfg(target_arch = "arm")]
fn ler_alvo() -> u64 {
    let (baixo, alto): (u32, u32);
    unsafe { asm!("mrrc p15, 3, {0}, {1}, c14", out(reg) baixo, out(reg) alto, options(nomem, nostack, preserves_flags)) };
    (alto as u64) << 32 | baixo as u64
}

#[cfg(target_arch = "arm")]
fn ler_controle() -> u64 {
    let valor: u32;
    unsafe { asm!("mrc p15, 0, {0}, c14, c3, 1", out(reg) valor, options(nomem, nostack, preserves_flags)) };
    valor as u64
}

#[cfg(target_arch = "arm")]
fn escrever_controle(valor: u64) {
    unsafe { asm!("mcr p15, 0, {0}, c14, c3, 1", "isb", in(reg) valor as u32, options(nostack, preserves_flags)) }
}

/// Liga o timer: primeiro tique daqui a um periodo.
pub fn iniciar() {
    let periodo = (cpu::frequencia_timer() / HZ).max(1);
    PERIODO.store(periodo, Ordering::Relaxed);
    escrever_alvo(cpu::contador() + periodo);
    escrever_controle(LIGADO);
}

/// O timer de outro nucleo: so tira o programa da CPU a cada periodo
/// (os tiques da conta sao do nucleo 0).
#[cfg_attr(feature = "mxq", allow(dead_code))]
pub fn iniciar_nucleo() {
    escrever_alvo(cpu::contador() + PERIODO.load(Ordering::Relaxed));
    escrever_controle(LIGADO);
}

/// Interrupcao do timer num nucleo que nao e o 0: marca o proximo.
pub fn proximo_do_nucleo() {
    escrever_alvo(cpu::contador() + PERIODO.load(Ordering::Relaxed));
}

/// O timer chegou mesmo no alvo? A linha do timer pode ainda estar alta
/// no GIC logo depois de andar o alvo, e a mesma interrupcao vir de novo.
pub fn disparou() -> bool {
    ler_controle() & DISPAROU != 0
}

/// Chamado pela interrupcao do timer (so quando `disparou`): conta o tique
/// e marca o proximo.
pub fn tique() {
    TIQUES.fetch_add(1, Ordering::Relaxed);
    let periodo = PERIODO.load(Ordering::Relaxed);
    let agora = cpu::contador();
    let mut alvo = ler_alvo() + periodo;
    if alvo <= agora {
        // ficou para tras (IRQ bloqueada muito tempo): recomeca de agora,
        // senao viriam varias interrupcoes seguidas so para alcancar
        alvo = agora + periodo;
    }
    escrever_alvo(alvo);
}

/// Tiques desde `iniciar`.
pub fn tiques() -> u64 {
    TIQUES.load(Ordering::Relaxed)
}

/// Converte milissegundos em ticks do contador sem estourar o u64.
fn ms_para_ticks(ms: u64) -> u64 {
    let freq = cpu::frequencia_timer();
    (ms / 1000).saturating_mul(freq).saturating_add(ms % 1000 * freq / 1000)
}

/// Dorme (WFI) ate passarem `ms` milissegundos.
/// Precisa das interrupcoes ligadas (irq::iniciar), senao nada acorda a CPU.
pub fn esperar_ms(ms: u64) {
    let alvo = cpu::contador().saturating_add(ms_para_ticks(ms));
    irq::esperar(|| (cpu::contador() >= alvo).then_some(()));
}
