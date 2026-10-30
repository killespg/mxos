//! Janelas pela ABI e o laco que roda um `App`: cria a janela do tamanho
//! que ele pede, entrega os eventos, chama Tique e Quadro na hora certa e
//! manda os pixels quando ele diz que mudou.

use alloc::string::String;
use alloc::vec;

use crate::entrada::Botao;
use crate::gui::grafico::Superficie;
use crate::gui::janelas::{App, EventoApp, Pedido};
use crate::gui::tema;
use crate::sys::{self, chamada};

pub const EV_TECLA: u32 = 1;
pub const EV_CARACTERE: u32 = 2;
pub const EV_MOUSE: u32 = 3;
pub const EV_BOTAO: u32 = 4;
pub const EV_RODA: u32 = 5;
pub const EV_FECHAR: u32 = 6;

/// Uma janela do app na area de trabalho.
pub struct Janela {
    id: usize,
}

/// O que sai da fila de uma janela.
pub enum Evento {
    App(EventoApp),
    Fechar,
}

impl Janela {
    pub fn criar(largura: usize, altura: usize, titulo: &str) -> Option<Janela> {
        let r = chamada(sys::JANELA_CRIAR, [largura, altura, titulo.as_ptr() as usize, titulo.len(), 0, 0]);
        (r > 0).then_some(Janela { id: r as usize })
    }

    /// Manda a superficie inteira para a tela.
    pub fn mostrar(&self, s: &Superficie) {
        let (l, a) = (s.largura(), s.altura());
        self.mostrar_pixels(s.pixels(), l * 4, 0, 0, l, a);
    }

    /// Manda o retangulo (x, y, l, a) de `pixels` (XRGB, `passo` bytes por linha).
    pub fn mostrar_pixels(&self, pixels: &[u32], passo: usize, x: usize, y: usize, l: usize, a: usize) {
        chamada(sys::JANELA_MOSTRAR, [self.id, pixels.as_ptr() as usize, passo, x | y << 16, l | a << 16, 0]);
    }

    pub fn titulo(&self, texto: &str) {
        chamada(sys::JANELA_TITULO, [self.id, texto.as_ptr() as usize, texto.len(), 0, 0, 0]);
    }

    /// O proximo evento da fila, sem esperar.
    pub fn evento(&self) -> Option<Evento> {
        let mut b = [0u32; 4];
        if chamada(sys::JANELA_EVENTO, [self.id, b.as_mut_ptr() as usize, 0, 0, 0, 0]) != 1 {
            return None;
        }
        let [tipo, a, x, y] = [b[0], b[1], b[2], b[3]];
        Some(match tipo {
            EV_TECLA => Evento::App(EventoApp::Tecla { codigo: a as u16, pressionada: x != 0 }),
            EV_CARACTERE => Evento::App(EventoApp::Caractere(char::from_u32(a).unwrap_or('?'))),
            EV_MOUSE => Evento::App(EventoApp::Mouse { x: a as i32, y: x as i32, botoes: y as u8 }),
            EV_BOTAO => {
                let botao = match y & 0xFF {
                    0 => Botao::Esquerdo,
                    1 => Botao::Direito,
                    _ => Botao::Meio,
                };
                Evento::App(EventoApp::Botao { x: a as i32, y: x as i32, botao, pressionado: y >> 8 != 0 })
            }
            EV_RODA => Evento::App(EventoApp::Roda(a as i32)),
            EV_FECHAR => Evento::Fechar,
            _ => return self.evento(),
        })
    }
}

impl Drop for Janela {
    fn drop(&mut self) {
        chamada(sys::JANELA_FECHAR, [self.id, 0, 0, 0, 0, 0]);
    }
}

/// Espera ate `ms` milissegundos por um evento em qualquer janela do app.
pub fn esperar(ms: u32) -> bool {
    chamada(sys::ESPERAR_EVENTO, [ms as usize, 0, 0, 0, 0, 0]) > 0
}

/// Poe o tema do sistema (e o tamanho da tela) nos modulos do app.
pub fn carregar_tema() {
    let mut buf = [0u8; 64];
    let n = chamada(sys::TEMA, [buf.as_mut_ptr() as usize, buf.len(), 0, 0, 0, 0]);
    if n > 0 {
        let nome = core::str::from_utf8(&buf[..(n as usize).min(buf.len())]).unwrap_or("");
        if let Some(t) = tema::por_nome(nome) {
            tema::definir(t);
        }
    }
    let (l, a) = sys::tela();
    if l > 0 && a > 0 {
        tema::definir_tela(l as i32, a as i32);
    }
}

/// Intervalo entre dois Tiques (como no sistema: uns 10 por segundo).
const TIQUE_US: u64 = 100_000;
/// Intervalo entre dois Quadros (60 por segundo).
const QUADRO_US: u64 = 16_667;

/// Roda o app ate a janela fechar. Nunca volta.
pub fn rodar<A: App>(mut app: A) -> ! {
    carregar_tema();
    let (largura, altura) = app.tamanho();
    let Some(janela) = Janela::criar(largura, altura, app.titulo()) else {
        sys::log("[app] não deu para criar a janela\n");
        sys::sair(1)
    };
    let mut s = Superficie::nova(largura, altura, tema::atual().fundo_janela);
    let mut titulo = String::from(app.titulo());
    app.desenhar(&mut s);
    janela.mostrar(&s);
    let quadros = app.quer_quadros();
    let mut proximo_tique = sys::tempo_us() + TIQUE_US;
    let mut proximo_quadro = sys::tempo_us() + QUADRO_US;
    let mut fila = vec![];
    loop {
        let agora = sys::tempo_us();
        let alvo = if quadros { proximo_quadro.min(proximo_tique) } else { proximo_tique };
        if alvo > agora {
            esperar(((alvo - agora) / 1000).max(1) as u32);
        }
        let mut sujo = false;
        while let Some(e) = janela.evento() {
            fila.push(e);
        }
        for e in fila.drain(..) {
            match e {
                Evento::Fechar => {
                    app.fechar();
                    sys::sair(0);
                }
                Evento::App(e) => sujo |= app.evento(e),
            }
        }
        let agora = sys::tempo_us();
        if quadros && agora >= proximo_quadro {
            sujo |= app.evento(EventoApp::Quadro);
            proximo_quadro = (proximo_quadro + QUADRO_US).max(agora);
        }
        if agora >= proximo_tique {
            sujo |= app.evento(EventoApp::Tique);
            proximo_tique = agora + TIQUE_US;
        }
        for p in app.pedidos() {
            match p {
                Pedido::FecharEsta => {
                    app.fechar();
                    sys::sair(0);
                }
                Pedido::TemaMudou => {
                    carregar_tema();
                    sujo = true;
                }
                _ => {}
            }
        }
        if app.titulo() != titulo {
            titulo = String::from(app.titulo());
            janela.titulo(&titulo);
        }
        if sujo {
            app.desenhar(&mut s);
            janela.mostrar(&s);
        }
    }
}
