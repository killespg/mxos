//! Uma janela X11 (de um programa de Linux) dentro de uma janela do MXOS.
//! O conteudo vem do servidor (crate::x11); mouse e teclado vao para ele.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::entrada::Botao;
use crate::gui::grafico::Superficie;
use crate::gui::janelas::{App, EventoApp, Pedido};
use crate::x11;

pub struct JanelaX {
    id: u32,
    titulo: String,
    largura: usize,
    altura: usize,
    /// O programa fechou a janela: a do MXOS fecha tambem.
    sumiu: bool,
}

impl JanelaX {
    pub fn nova(id: u32) -> JanelaX {
        let (largura, altura) = x11::tamanho(id);
        let mut j = JanelaX { id, titulo: String::new(), largura, altura, sumiu: false };
        j.atualizar_titulo();
        j
    }

    pub fn tamanho_inicial(&self) -> (usize, usize) {
        (self.largura, self.altura)
    }

    fn atualizar_titulo(&mut self) {
        let titulo = x11::titulo(self.id);
        self.titulo = if titulo.is_empty() { String::from("Programa X11") } else { titulo };
    }
}

impl App for JanelaX {
    fn titulo(&self) -> &str {
        &self.titulo
    }

    fn tamanho(&self) -> (usize, usize) {
        (self.largura, self.altura)
    }

    fn desenhar(&mut self, conteudo: &mut Superficie) {
        let (l, a) = (conteudo.largura(), conteudo.altura());
        x11::copiar(self.id, conteudo.pixels_mut(), l, a);
    }

    fn evento(&mut self, e: EventoApp) -> bool {
        match e {
            EventoApp::Processos | EventoApp::Tique => {
                if !x11::viva(self.id) {
                    self.sumiu = true;
                    return false;
                }
                let suja = x11::tirar_sujeira(self.id);
                if suja {
                    self.atualizar_titulo();
                }
                suja
            }
            EventoApp::Mouse { x, y, .. } => {
                x11::mouse(self.id, x, y);
                false
            }
            EventoApp::Botao { x, y, botao, pressionado } => {
                if pressionado {
                    x11::foco(self.id, true);
                }
                let b = match botao {
                    Botao::Esquerdo => 1,
                    Botao::Meio => 2,
                    Botao::Direito => 3,
                };
                x11::botao(self.id, x, y, b, pressionado);
                false
            }
            EventoApp::Roda(passos) => {
                // roda para cima = botao 4, para baixo = 5 (aperta e solta)
                let b = if passos > 0 { 4 } else { 5 };
                for _ in 0..passos.unsigned_abs().min(10) {
                    x11::botao(self.id, -1, -1, b, true);
                    x11::botao(self.id, -1, -1, b, false);
                }
                false
            }
            EventoApp::Tecla { codigo, pressionada } => {
                x11::tecla(self.id, codigo, pressionada);
                false
            }
            EventoApp::Caractere(_) => false,
        }
    }

    fn fechar(&mut self) {
        if !self.sumiu {
            x11::fechar(self.id);
        }
    }

    fn pedidos(&mut self) -> Vec<Pedido> {
        if self.sumiu { vec![Pedido::FecharEsta] } else { Vec::new() }
    }
}
