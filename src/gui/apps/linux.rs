//! Atalhos para programas de Linux que estao no disco (pacotes do
//! Debian, ferramentas/pacotes.py). O "app" so diz o que rodar: o
//! gerenciador comeca o processo em vez de abrir uma janela, e a janela
//! aparece quando o programa abre a dele pelo X11.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::gui::grafico::Superficie;
use crate::gui::janelas::{App, EventoApp};
use crate::gui::tema;
use crate::processo::arquivos;

pub const NETSURF: &str = "/usr/bin/netsurf-fb";

pub struct Programa {
    argv: Vec<String>,
}

impl App for Programa {
    fn titulo(&self) -> &str {
        &self.argv[0]
    }

    fn tamanho(&self) -> (usize, usize) {
        (1, 1)
    }

    fn desenhar(&mut self, _: &mut Superficie) {}

    fn evento(&mut self, _: EventoApp) -> bool {
        false
    }

    fn lancar(&self) -> Option<Vec<String>> {
        Some(self.argv.clone())
    }
}

/// O programa existe no disco?
pub fn existe(caminho: &str) -> bool {
    arquivos::info(caminho).is_ok_and(|(pasta, _)| !pasta)
}

/// O NetSurf do tamanho da tela (menos a moldura e a barra de tarefas).
pub fn criar_netsurf() -> Box<dyn App> {
    let (largura, altura) = tema::tamanho_da_tela();
    let l = (largura - 2 * tema::BORDA - 120).max(320);
    let a = (altura - tema::ALTURA_BARRA_TAREFAS - tema::ALTURA_TITULO - 2 * tema::BORDA - 80).max(240);
    Box::new(Programa {
        argv: vec![
            String::from(NETSURF),
            String::from("-f"),
            String::from("x"),
            String::from("-w"),
            format!("{l}"),
            String::from("-h"),
            format!("{a}"),
        ],
    })
}
