//! PSCI: a interface padrao do ARM para pedir ao firmware que desligue,
//! reinicie ou ligue nucleos. No QEMU quem responde e o proprio QEMU;
//! na TV box e o ARM Trusted Firmware da Amlogic.
//!
//! A via (HVC ou SMC) depende de quem entregou o kernel: o Shell::nova
//! descobre no boot e guarda aqui com definir_via. Assim quem nao conhece
//! o shell (a area de trabalho) tambem consegue reiniciar e desligar.

use core::arch::asm;
use core::fmt::Write;
use core::sync::atomic::{AtomicU8, Ordering};

use crate::cpu;
use crate::uart::Uart;

pub const VERSAO: u32 = 0x8400_0000;
pub const DESLIGAR: u32 = 0x8400_0008;
pub const REINICIAR: u32 = 0x8400_0009;

/// Instrucao usada para falar com o firmware.
#[derive(Clone, Copy)]
pub enum Via {
    /// Hypervisor (EL2): o QEMU quando o kernel ja comeca em EL1.
    Hvc,
    /// Secure monitor (EL3): o firmware de verdade.
    Smc,
}

impl Via {
    pub fn nome(self) -> &'static str {
        match self {
            Via::Hvc => "HVC",
            Via::Smc => "SMC",
        }
    }

    /// Numero guardado em VIA (0 fica para "ainda nao definida").
    fn codigo(self) -> u8 {
        match self {
            Via::Hvc => 1,
            Via::Smc => 2,
        }
    }
}

/// Via guardada pelo shell no boot. Atomico: nada de static mut.
static VIA: AtomicU8 = AtomicU8::new(0);

pub fn chamar(via: Via, funcao: u32) -> u64 {
    chamar3(via, funcao, 0, 0, 0)
}

/// Chamada com 3 argumentos (CPU_ON: nucleo, endereco, contexto).
#[cfg(target_arch = "aarch64")]
pub fn chamar3(via: Via, funcao: u32, a1: u64, a2: u64, a3: u64) -> u64 {
    let mut x0 = funcao as u64;
    unsafe {
        match via {
            Via::Hvc => asm!("hvc #0", inout("x0") x0, in("x1") a1, in("x2") a2, in("x3") a3, clobber_abi("C"), options(nostack)),
            Via::Smc => asm!("smc #0", inout("x0") x0, in("x1") a1, in("x2") a2, in("x3") a3, clobber_abi("C"), options(nostack)),
        }
    }
    x0
}

/// No ARMv7 a mesma chamada, com r0-r3 de 32 bits (funcoes SMC32).
#[cfg(target_arch = "arm")]
pub fn chamar3(via: Via, funcao: u32, a1: u64, a2: u64, a3: u64) -> u64 {
    // as funcoes de 64 bits (0xC4...) viram as de 32 (0x84...)
    let mut r0 = funcao & !0x4000_0000;
    unsafe {
        match via {
            Via::Hvc => asm!(".arch_extension virt", "hvc #0", inout("r0") r0, in("r1") a1 as u32, in("r2") a2 as u32, in("r3") a3 as u32, clobber_abi("C"), options(nostack)),
            Via::Smc => asm!(".arch_extension sec", "smc #0", inout("r0") r0, in("r1") a1 as u32, in("r2") a2 as u32, in("r3") a3 as u32, clobber_abi("C"), options(nostack)),
        }
    }
    r0 as i32 as i64 as u64
}

/// Guarda a via que o firmware atende (chamado pelo Shell::nova).
pub fn definir_via(via: Via) {
    VIA.store(via.codigo(), Ordering::Relaxed);
}

/// A via guardada. Sem nenhuma, o mesmo palpite do shell para quem comeca
/// em EL1: SMC na TV box, HVC no QEMU.
#[cfg_attr(feature = "mxq", allow(dead_code))] // na TV box so o shell usa o PSCI
pub fn via() -> Via {
    match VIA.load(Ordering::Relaxed) {
        1 => Via::Hvc,
        2 => Via::Smc,
        // TV boxes: firmware seguro por SMC (sem hipervisor, HVC e instrucao invalida)
        _ if cfg!(any(feature = "mxq", feature = "rk322x")) => Via::Smc,
        _ => Via::Hvc,
    }
}

/// Reinicia a maquina pelo firmware. Se ele nao atender, avisa na serial
/// e para a CPU (a tela fica com o aviso que quem chamou desenhou).
#[cfg_attr(feature = "mxq", allow(dead_code))]
pub fn reiniciar() -> ! {
    chamar(via(), REINICIAR);
    nao_atendeu("reiniciou")
}

/// Desliga a maquina pelo firmware (no QEMU a janela fecha).
#[cfg_attr(feature = "mxq", allow(dead_code))]
pub fn desligar() -> ! {
    chamar(via(), DESLIGAR);
    nao_atendeu("desligou")
}

#[cfg_attr(feature = "mxq", allow(dead_code))]
fn nao_atendeu(acao: &str) -> ! {
    let _ = writeln!(Uart::new(), "\npsci: o firmware não {acao} (via {}); CPU parada.", via().nome());
    cpu::parar()
}
