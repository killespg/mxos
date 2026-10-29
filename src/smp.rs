//! Os outros nucleos (o S905W da TV box e o QEMU com -smp 4 tem 4).
//!
//! Modelo simples, o do Linux 2.0: uma trava unica do kernel. So um nucleo
//! por vez roda codigo do kernel (chamadas de sistema, a interface, a
//! rede, o disco); os programas (EL0) rodam nos 4 ao mesmo tempo. Quem
//! entra no kernel pega a trava; quem vai para o programa ou vai dormir
//! solta. Como o kernel inteiro foi escrito para um nucleo so (globais em
//! UnsafeCell, "so o laco principal mexe"), a trava mantem isso verdade
//! sem mexer em cada modulo.
//!
//! O nucleo 0 faz tudo o que fazia (interface, servidor X, dispositivos) e
//! tambem roda processos; os nucleos 1-3 so rodam processos. As
//! interrupcoes dos dispositivos continuam no nucleo 0; cada nucleo tem o
//! proprio timer (100 Hz) para tirar o programa da CPU na hora da troca.

use alloc::boxed::Box;
use alloc::vec;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::{cpu, gic, mmu, psci, timer};

/// Nucleos que existem (no maximo).
pub const MAXIMO: usize = 4;
const PILHA: usize = 64 * 1024;
const PILHA_EXCECAO: usize = 16 * 1024;
const PSCI_CPU_ON: u32 = 0xC400_0003;

/// Quantos nucleos estao rodando (o 0 conta).
static LIGADOS: AtomicUsize = AtomicUsize::new(1);

/// O numero deste nucleo (Aff0 do MPIDR: 0 a 3).
pub fn nucleo() -> usize {
    (cpu::mpidr() & 0xFF) as usize
}

pub fn ligados() -> usize {
    LIGADOS.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------
// A trava do kernel

/// Dono da trava: 0 = livre, n = nucleo n - 1.
static DONO: AtomicUsize = AtomicUsize::new(0);
/// Quantas vezes o dono pegou (pegar de novo no mesmo nucleo nao trava).
static PROFUNDIDADE: AtomicUsize = AtomicUsize::new(0);

pub fn pegar() {
    let eu = nucleo() + 1;
    if DONO.load(Ordering::Relaxed) == eu {
        PROFUNDIDADE.fetch_add(1, Ordering::Relaxed);
        return;
    }
    while DONO.compare_exchange_weak(0, eu, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    PROFUNDIDADE.store(1, Ordering::Relaxed);
}

pub fn soltar() {
    if PROFUNDIDADE.fetch_sub(1, Ordering::Relaxed) == 1 {
        DONO.store(0, Ordering::Release);
        // quem espera num WFE acorda
        unsafe { core::arch::asm!("sev", options(nomem, nostack)) };
    }
}

/// Roda `f` sem a trava (rodar o programa, dormir) e pega de volta.
pub fn sem_trava<R>(f: impl FnOnce() -> R) -> R {
    let eu = nucleo() + 1;
    if DONO.load(Ordering::Relaxed) != eu {
        return f(); // ainda nao ha trava (antes de ligar os nucleos)
    }
    let profundidade = PROFUNDIDADE.load(Ordering::Relaxed);
    PROFUNDIDADE.store(0, Ordering::Relaxed);
    DONO.store(0, Ordering::Release);
    unsafe { core::arch::asm!("sev", options(nomem, nostack)) };
    let r = f();
    while DONO.compare_exchange_weak(0, eu, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    PROFUNDIDADE.store(profundidade, Ordering::Relaxed);
    r
}

/// O laco principal do nucleo 0 nao tem nada a fazer: dorme sem a trava
/// ate a proxima interrupcao.
pub fn ocioso() {
    sem_trava(crate::irq::cochilar);
}

// ---------------------------------------------------------------------
// Ligar os outros

/// O que o boot.s do nucleo novo le (offsets fixos: ver _start_secundario).
#[repr(C)]
struct Partida {
    pilha_excecao: u64, // 0
    pilha: u64,         // 8
    mair: u64,          // 16
    tcr: u64,           // 24
    ttbr0: u64,         // 32
    numero: u64,        // 40
}

unsafe extern "C" {
    fn _start_secundario();
}

/// Liga os nucleos 1..MAXIMO pelo PSCI. O nucleo 0 pega a trava do kernel
/// antes (e so solta para rodar programas e dormir). Devolve quantos ligaram.
pub fn ligar_nucleos() -> usize {
    pegar();
    // ARMv7 (RK322x): por enquanto so o nucleo 0
    if cfg!(target_arch = "arm") {
        return ligados();
    }
    let (mair, tcr, ttbr0) = mmu::registradores();
    for numero in 1..MAXIMO {
        // pilhas que nunca sao devolvidas (o nucleo roda para sempre)
        let pilha: &'static mut [u8] = Box::leak(vec![0u8; PILHA].into_boxed_slice());
        let excecao: &'static mut [u8] = Box::leak(vec![0u8; PILHA_EXCECAO].into_boxed_slice());
        let partida = Box::leak(Box::new(Partida {
            pilha_excecao: (excecao.as_ptr() as u64 + PILHA_EXCECAO as u64) & !15,
            pilha: (pilha.as_ptr() as u64 + PILHA as u64) & !15,
            mair,
            tcr,
            ttbr0,
            numero: numero as u64,
        }));
        // o nucleo novo comeca com cache desligado: le direto da RAM
        mmu::limpar_cache(partida as *const Partida as usize, core::mem::size_of::<Partida>());
        let antes = LIGADOS.load(Ordering::Relaxed);
        let r = psci::chamar3(psci::via(), PSCI_CPU_ON, numero as u64, _start_secundario as *const () as usize as u64, partida as *const Partida as u64);
        if r != 0 {
            break; // o firmware nao tem esse nucleo (ou nao liga nucleos)
        }
        // espera ele avisar que esta de pe (no maximo 100 ms)
        let prazo = cpu::contador() + cpu::frequencia_timer() / 10;
        while LIGADOS.load(Ordering::Acquire) == antes && cpu::contador() < prazo {
            core::hint::spin_loop();
        }
    }
    ligados()
}

/// Primeiro codigo Rust de um nucleo novo (MMU e caches ja ligados pelo
/// boot.s, pilhas proprias). Nunca volta.
#[unsafe(no_mangle)]
extern "C" fn nucleo_secundario(numero: u64) -> ! {
    let _ = numero;
    gic::iniciar_nucleo();
    timer::iniciar_nucleo();
    crate::processo::configurar_el0();
    LIGADOS.fetch_add(1, Ordering::Release);
    cpu::ligar_interrupcoes();
    loop {
        pegar();
        let rodou = crate::processo::rodar_neste_nucleo();
        soltar();
        if !rodou {
            // nada para rodar: dorme ate o timer (ou um SEV do nucleo 0)
            unsafe { core::arch::asm!("wfe", options(nomem, nostack)) };
        }
    }
}

/// Acorda os nucleos que dormem no WFE (ha processo pronto).
pub fn acordar_todos() {
    unsafe { core::arch::asm!("dsb ish", "sev", options(nomem, nostack)) };
}
