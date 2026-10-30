//! Janelinha de aviso: um titulo, a mensagem (quebrada em linhas) e OK.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::entrada::Botao;
use crate::fonte::{self, INTER, INTER_TITULO};
use crate::gui::grafico::{Retangulo, Superficie};
use crate::gui::janelas::{App, EventoApp, Pedido};
use crate::gui::tema;
use crate::gui::widgets::{self, ALTURA_BOTAO, cor_texto_fraco};

const LARGURA: i32 = 420;
const MARGEM: i32 = 20;
const ALTURA_LINHA: i32 = 18;

pub struct Aviso {
    titulo: String,
    linhas: Vec<String>,
    fechar: bool,
}

pub fn criar(titulo: &str, mensagem: &str) -> Box<dyn App> {
    Box::new(Aviso { titulo: String::from(titulo), linhas: quebrar(mensagem, LARGURA - 2 * MARGEM), fechar: false })
}

/// Quebra `texto` em linhas de ate `largura` pixels (nas palavras).
fn quebrar(texto: &str, largura: i32) -> Vec<String> {
    let mut linhas = Vec::new();
    for paragrafo in texto.lines() {
        let mut atual = String::new();
        for palavra in paragrafo.split(' ') {
            let tentativa = if atual.is_empty() { String::from(palavra) } else { alloc::format!("{atual} {palavra}") };
            if Superficie::largura_texto(&tentativa, 1) > largura && !atual.is_empty() {
                linhas.push(core::mem::replace(&mut atual, String::from(palavra)));
            } else {
                atual = tentativa;
            }
        }
        linhas.push(atual);
    }
    linhas.truncate(12);
    linhas
}

impl Aviso {
    fn botao_ok(&self) -> Retangulo {
        let (l, a) = self.tamanho();
        Retangulo::novo(l as i32 - MARGEM - 88, a as i32 - MARGEM - ALTURA_BOTAO, 88, ALTURA_BOTAO)
    }
}

impl App for Aviso {
    fn titulo(&self) -> &str {
        &self.titulo
    }

    fn tamanho(&self) -> (usize, usize) {
        let altura = MARGEM + 28 + self.linhas.len() as i32 * ALTURA_LINHA + 16 + ALTURA_BOTAO + MARGEM;
        (LARGURA as usize, altura as usize)
    }

    fn desenhar(&mut self, s: &mut Superficie) {
        let t = tema::atual();
        s.limpar(t.fundo_janela);
        s.texto_com(MARGEM, MARGEM + 18, fonte::cortar(&self.titulo, LARGURA - 2 * MARGEM, &INTER_TITULO), t.texto, &INTER_TITULO);
        for (i, linha) in self.linhas.iter().enumerate() {
            s.texto_com(MARGEM, MARGEM + 28 + 14 + i as i32 * ALTURA_LINHA, linha, cor_texto_fraco(&t), &INTER);
        }
        widgets::botao(s, self.botao_ok(), "OK", true);
    }

    fn evento(&mut self, e: EventoApp) -> bool {
        match e {
            EventoApp::Botao { x, y, botao: Botao::Esquerdo, pressionado: false } if self.botao_ok().contem(x, y) => self.fechar = true,
            EventoApp::Caractere('\r' | '\n' | '\u{1b}') => self.fechar = true,
            _ => {}
        }
        false
    }

    fn pedidos(&mut self) -> Vec<Pedido> {
        if self.fechar { vec![Pedido::FecharEsta] } else { Vec::new() }
    }
}
