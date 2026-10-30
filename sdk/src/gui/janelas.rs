//! O `App` das janelas do sistema (gui/janelas.rs), para o app escrever o
//! mesmo codigo dentro ou fora do kernel. Quem roda o App e mxos::rodar.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use super::grafico::Superficie;
use crate::entrada::{Botao, Layout};

/// Evento entregue a um app. Coordenadas relativas ao canto do conteudo.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EventoApp {
    Caractere(char),
    Tecla { codigo: u16, pressionada: bool },
    /// botoes: bit 0 esquerdo, bit 1 direito, bit 2 meio.
    Mouse { x: i32, y: i32, botoes: u8 },
    Botao { x: i32, y: i32, botao: Botao, pressionado: bool },
    Roda(i32),
    /// Uns 10 por segundo.
    Tique,
    /// Nao chega aos apps (so o terminal do sistema).
    Processos,
    /// ~60 por segundo, so para quem pede em App::quer_quadros.
    Quadro,
}

/// Um programa que mora numa janela.
pub trait App {
    fn titulo(&self) -> &str;
    /// Tamanho do conteudo em pixels (lido ao abrir a janela).
    fn tamanho(&self) -> (usize, usize);
    /// Redesenha o conteudo inteiro.
    fn desenhar(&mut self, conteudo: &mut Superficie);
    /// true = precisa redesenhar.
    fn evento(&mut self, e: EventoApp) -> bool;
    /// Chamado quando a janela fecha (o app sai logo depois).
    fn fechar(&mut self) {}
    /// Pedidos ao sistema, lidos depois de cada evento.
    fn pedidos(&mut self) -> Vec<Pedido> {
        Vec::new()
    }
    /// true = recebe EventoApp::Quadro ~60 vezes por segundo.
    fn quer_quadros(&self) -> bool {
        false
    }
}

/// O que um app pede (ver App::pedidos). Num pacote so FecharEsta e
/// TemaMudou fazem efeito; o resto existe para o codigo dos apps do
/// sistema compilar igual.
pub enum Pedido {
    Abrir { app: Box<dyn App>, x: i32, y: i32 },
    Fechar(usize),
    FecharEsta,
    AbrirArquivo(String),
    DefinirLayout(Layout),
    TemaMudou,
}
