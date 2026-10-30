//! Interrupcoes: em vez de ficar perguntando a serial sem parar, a CPU
//! dorme (WFI) e o hardware acorda ela.
//!   - timer: 100 vezes por segundo (tique)
//!   - serial: chegou byte no teclado
//!
//! O tratador roda no meio de qualquer codigo do kernel, inclusive com a
//! trava do heap pega: por isso ele nao aloca e nao imprime nada.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::cpu;
use crate::gic;
use crate::plataforma::{IRQ_SERIAL, IRQ_SERIAL_BORDA, IRQ_TIMER};
use crate::timer;
use crate::uart::Uart;

static TIMER: AtomicU64 = AtomicU64::new(0);
static SERIAL: AtomicU64 = AtomicU64::new(0);
static BYTES_SERIAL: AtomicU64 = AtomicU64::new(0);
static OUTRAS: AtomicU64 = AtomicU64::new(0);
static ESPURIAS: AtomicU64 = AtomicU64::new(0);
static OCIOSO: AtomicU64 = AtomicU64::new(0);
static INICIO: AtomicU64 = AtomicU64::new(0);

/// Fotografia dos contadores. `ocioso` e `desde_inicio` em ticks do contador.
pub struct Contadores {
    pub timer: u64,
    pub serial: u64,
    pub bytes_serial: u64,
    pub outras: u64,
    pub espurias: u64,
    pub ocioso: u64,
    pub desde_inicio: u64,
}

fn somar(contador: &AtomicU64, valor: u64) {
    contador.fetch_add(valor, Ordering::Relaxed);
}

/// Liga GIC, timer e RX da serial. Chamar depois do heap e antes do shell.
pub fn iniciar(console: &mut Uart) {
    cpu::ligar_erros_assincronos(); // erro no GIC/serial aparece na tela
    gic::iniciar();
    gic::habilitar(IRQ_TIMER, false);
    // TV box: a UART2 e do firmware e ninguem liga cabo serial; uma
    // interrupcao presa nela travaria tudo. Nas outras, entrada pela serial.
    if !cfg!(feature = "rk322x") {
        gic::habilitar(IRQ_SERIAL, IRQ_SERIAL_BORDA);
        console.ligar_interrupcao_rx();
    }
    timer::iniciar();
    INICIO.store(cpu::contador(), Ordering::Relaxed);
    cpu::ligar_interrupcoes();
}

/// Chamado pelo vetores.s (irq_entrada) com os registradores ja salvos.
#[unsafe(no_mangle)]
extern "C" fn tratar_irq() {
    let mut atendidas = 0;
    // atende tudo que estiver pendente antes de voltar
    loop {
        let iar = gic::confirmar();
        let id = iar & 0x3FF;
        if id >= gic::ESPURIA {
            // so e espuria se acordou e nao tinha nada mesmo
            if atendidas == 0 {
                somar(&ESPURIAS, 1);
            }
            break;
        }
        atender(id);
        gic::finalizar(iar);
        atendidas += 1;
    }
}

fn atender(id: u32) {
    match id {
        // sem disparar de verdade e repeticao da linha que ainda nao desceu
        IRQ_TIMER if !timer::disparou() => somar(&ESPURIAS, 1),
        // os outros nucleos: so a troca de programa, sem contar tique
        IRQ_TIMER if cpu::mpidr() & 0xFF != 0 => timer::proximo_do_nucleo(),
        IRQ_TIMER => {
            somar(&TIMER, 1);
            timer::tique();
            // TV box: temperatura e travamento, mesmo com o laco principal preso
            crate::frequencia::vigiar();
            crate::console_boot::vigiar_travamento();
        }
        IRQ_SERIAL => {
            somar(&SERIAL, 1);
            let lidos = Uart::new().atender_interrupcao();
            somar(&BYTES_SERIAL, lidos as u64);
        }
        _ => {
            // ninguem pediu esta: desliga para contar uma vez e nao travar
            somar(&OUTRAS, 1);
            gic::desabilitar(id);
        }
    }
}

/// Dorme ate `pronto` devolver algo. Testa com interrupcoes desligadas para
/// nao perder a que chegar entre o teste e o WFI: o WFI acorda mesmo com
/// elas mascaradas, e a pendente e atendida assim que religamos.
/// Sempre devolve com as interrupcoes ligadas.
pub fn esperar<T>(mut pronto: impl FnMut() -> Option<T>) -> T {
    loop {
        cpu::desligar_interrupcoes();
        if let Some(valor) = pronto() {
            cpu::ligar_interrupcoes();
            return valor;
        }
        dormir();
    }
}

/// Dorme ate a proxima interrupcao, contando o tempo como ocioso. Para quem
/// faz polling num laco proprio (o driver de som, por exemplo).
#[cfg_attr(feature = "mxq", allow(dead_code))] // na TV box ainda nao ha quem faca polling
pub fn cochilar() {
    cpu::desligar_interrupcoes();
    dormir();
}

/// WFI com as interrupcoes ja desligadas; liga de novo no fim.
fn dormir() {
    let antes = cpu::contador();
    cpu::esperar_interrupcao();
    somar(&OCIOSO, cpu::contador().wrapping_sub(antes));
    cpu::ligar_interrupcoes(); // a interrupcao pendente e atendida aqui
}

pub fn contadores() -> Contadores {
    Contadores {
        timer: TIMER.load(Ordering::Relaxed),
        serial: SERIAL.load(Ordering::Relaxed),
        bytes_serial: BYTES_SERIAL.load(Ordering::Relaxed),
        outras: OUTRAS.load(Ordering::Relaxed),
        espurias: ESPURIAS.load(Ordering::Relaxed),
        ocioso: OCIOSO.load(Ordering::Relaxed),
        desde_inicio: cpu::contador().wrapping_sub(INICIO.load(Ordering::Relaxed)),
    }
}
