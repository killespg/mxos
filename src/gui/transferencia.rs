//! Area de transferencia de texto do MXOS: Ctrl+C e Ctrl+V nos campos de
//! texto e no editor, e a selecao CLIPBOARD dos programas X11 (o servidor
//! X le e escreve aqui: crate::x11).

use alloc::string::String;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};

struct Global(UnsafeCell<String>);

// So o laco principal mexe (com a trava do kernel).
unsafe impl Sync for Global {}

static TEXTO: Global = Global(UnsafeCell::new(String::new()));
/// Muda a cada copia feita no MXOS (o servidor X ve e toma a selecao).
static VERSAO: AtomicU64 = AtomicU64::new(0);

/// Codigos evdev de C e V (com Ctrl).
pub const TECLA_C: u16 = 46;
pub const TECLA_V: u16 = 47;

pub fn texto() -> String {
    unsafe { (*TEXTO.0.get()).clone() }
}

/// Copia no MXOS: os programas X passam a colar isto.
pub fn copiar(texto: &str) {
    guardar(texto);
    VERSAO.fetch_add(1, Ordering::Relaxed);
}

/// Guarda sem mudar a versao (o texto veio de um programa X).
pub fn guardar(texto: &str) {
    unsafe { *TEXTO.0.get() = String::from(texto) };
}

pub fn versao() -> u64 {
    VERSAO.load(Ordering::Relaxed)
}
