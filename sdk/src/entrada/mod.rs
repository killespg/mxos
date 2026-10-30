//! Botoes do mouse e codigos de tecla, como no crate::entrada do sistema.

#[allow(dead_code)]
#[path = "../../../src/entrada/teclado.rs"]
pub mod teclado;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Botao {
    Esquerdo,
    Direito,
    Meio,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Layout {
    Us,
    Abnt2,
}

impl Layout {
    pub fn descricao(&self) -> &'static str {
        match self {
            Layout::Us => "US (americano)",
            Layout::Abnt2 => "ABNT2 (brasileiro)",
        }
    }
}
