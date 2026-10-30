//! Area de transferencia do sistema (Ctrl+C / Ctrl+V), pela ABI.

use alloc::string::String;
use alloc::vec;

use crate::sys::{self, chamada};

/// Codigos evdev de C e V (com Ctrl).
pub const TECLA_C: u16 = 46;
pub const TECLA_V: u16 = 47;

pub fn texto() -> String {
    let mut buf = vec![0u8; 4096];
    loop {
        let n = chamada(sys::COLAR, [buf.as_mut_ptr() as usize, buf.len(), 0, 0, 0, 0]);
        if n < 0 {
            return String::new();
        }
        if n as usize > buf.len() {
            buf.resize(n as usize, 0);
            continue;
        }
        buf.truncate(n as usize);
        return String::from_utf8(buf).unwrap_or_default();
    }
}

pub fn copiar(texto: &str) {
    chamada(sys::COPIAR, [texto.as_ptr() as usize, texto.len(), 0, 0, 0, 0]);
}
