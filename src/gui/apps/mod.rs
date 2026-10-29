//! Programas da area de trabalho. Cada um implementa janelas::App e so
//! desenha dentro da propria Superficie de conteudo.
//!
//!   - arquivos:      gerenciador de arquivos e de discos
//!   - calculadora:   inteiros de 64 bits, mouse e teclado
//!   - configuracoes: personalizar, teclado, rede, disco e sistema
//!   - drivers:       painel com os dispositivos e os drivers de cada um
//!   - editor:        editor de texto
//!   - navegador:     o navegador (o motor fica em crate::navegador)
//!   - pintura:       paleta de cores, pincel com o mouse, limpar
//!   - sobre:         versao, cpu, memoria e tempo ligado
//!   - tarefas:       gerenciador de tarefas (aplicativos e desempenho)
//!   - terminal:      o shell numa janela

pub mod arquivos;
pub mod calculadora;
pub mod configuracoes;
pub mod drivers;
pub mod editor;
pub mod navegador;
pub mod pintura;
pub mod sobre;
pub mod tarefas;
pub mod terminal;

use alloc::boxed::Box;
use alloc::format;

use super::janelas::{App, Gerenciador};
use super::widgets::Icone;

/// Poe os apps deste modulo no menu iniciar (o Terminal entra pelo gui::rodar).
pub fn registrar_apps_basicos(g: &mut Gerenciador) {
    g.registrar_app("Arquivos", Icone::Pasta, arquivos::criar);
    g.registrar_app("Discos", Icone::Disco, arquivos::criar_discos);
    g.registrar_app("Navegador", Icone::Navegador, navegador::criar);
    g.registrar_app("Editor de texto", Icone::Texto, editor::criar);
    g.registrar_app("Tarefas", Icone::Tarefas, tarefas::criar);
    g.registrar_app("Drivers", Icone::Chip, drivers::criar);
    g.registrar_app("Configurações", Icone::Engrenagem, configuracoes::criar);
    g.registrar_app("Personalizar", Icone::Monitor, configuracoes::criar_personalizar);
    g.registrar_app("Calculadora", Icone::Calculadora, criar_calculadora);
    g.registrar_app("Pintura", Icone::Pincel, criar_pintura);
    g.registrar_app("Sobre o MXOS", Icone::Info, criar_sobre);
}

/// Icones da coluna da esquerda da area de trabalho.
pub fn registrar_atalhos(g: &mut Gerenciador) {
    g.registrar_atalho("Arquivos", Icone::Pasta, arquivos::criar);
    g.registrar_atalho("Navegador", Icone::Navegador, navegador::criar);
    g.registrar_atalho(terminal::TITULO, Icone::Terminal, terminal::criar);
    g.registrar_atalho("Tarefas", Icone::Tarefas, tarefas::criar);
    g.registrar_atalho("Configurações", Icone::Engrenagem, configuracoes::criar);
}

/// O app certo para um arquivo do disco: paginas (.htm, .html) no
/// navegador, o resto no editor de texto.
pub fn abrir_arquivo(caminho: &str) -> Box<dyn App> {
    let minusculo = caminho.to_ascii_lowercase();
    if minusculo.ends_with(".htm") || minusculo.ends_with(".html") {
        Box::new(navegador::Navegador::novo(&format!("disco:{caminho}")))
    } else {
        Box::new(editor::Editor::abrir(caminho))
    }
}

fn criar_calculadora() -> Box<dyn App> {
    Box::new(calculadora::Calculadora::nova())
}

fn criar_pintura() -> Box<dyn App> {
    Box::new(pintura::Pintura::nova())
}

fn criar_sobre() -> Box<dyn App> {
    Box::new(sobre::Sobre::novo())
}
