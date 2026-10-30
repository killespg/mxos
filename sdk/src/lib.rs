//! Kit de desenvolvimento dos apps nativos do MXOS.
//!
//! Um app e um executavel estatico (ARMv7 para a TV box, AArch64 para o
//! QEMU e a MXQ) que fala com o kernel pela ABI do MXOS (sys.rs). O app
//! implementa o mesmo `App` das janelas do sistema e chama `mxos::app!`:
//!
//! ```ignore
//! #![no_std]
//! #![no_main]
//! extern crate alloc;
//! use mxos::gui::janelas::{App, EventoApp};
//! struct Ola;
//! impl App for Ola { ... }
//! mxos::app!(|| Ola);
//! ```
//!
//! Os modulos `fonte`, `gui::{grafico, tema, widgets, realce, barra}` e
//! `entrada::teclado` sao os mesmos arquivos do sistema (src/), entao um
//! app do sistema vira pacote trocando `crate::` por `mxos::`. `cpu`,
//! `video::tela`, `armazenamento` e o resto de `gui` imitam os do kernel
//! por cima da ABI.

#![no_std]
#![allow(unexpected_cfgs)]

extern crate alloc;

pub mod armazenamento;
pub mod audio;
pub mod cpu;
pub mod entrada;
pub mod gui;
mod heap;
pub mod http;
pub mod ia;
pub mod janela;
pub mod sys;
pub mod video;

#[allow(dead_code)]
#[path = "../../src/fonte/mod.rs"]
pub mod fonte;

pub use janela::rodar;

/// Ponto de entrada do app: `mxos::app!(Calculadora::nova)` (qualquer
/// `fn() -> impl App`).
#[macro_export]
macro_rules! app {
    ($criar:expr) => {
        #[unsafe(no_mangle)]
        #[unsafe(link_section = ".text._start")]
        pub extern "C" fn _start() -> ! {
            $crate::rodar($criar())
        }
    };
}

/// Argumentos que quem abriu o app passou (o caminho de um arquivo, por
/// exemplo). Vazio se nenhum.
pub fn argumentos() -> alloc::string::String {
    let mut buf = alloc::vec![0u8; 4096];
    let n = sys::chamada(sys::ARGUMENTOS, [buf.as_mut_ptr() as usize, buf.len(), 0, 0, 0, 0]);
    buf.truncate(n.clamp(0, 4096) as usize);
    alloc::string::String::from_utf8_lossy(&buf).into_owned()
}

#[panic_handler]
fn panico(info: &core::panic::PanicInfo) -> ! {
    use core::fmt::Write;
    struct Log;
    impl core::fmt::Write for Log {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            sys::log(s);
            Ok(())
        }
    }
    let _ = writeln!(Log, "\n[app] pânico: {info}");
    sys::sair(101)
}
