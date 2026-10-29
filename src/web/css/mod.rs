//! CSS: tokenizador, valores, seletores, folhas, estilo calculado e cascata.

pub mod cascata;
pub mod estilo;
pub mod folha;
pub mod seletor;
pub mod token;
pub mod valores;

use alloc::string::String;
use alloc::vec::Vec;

use crate::web::atomo::{Atomo, a};
use crate::web::dom::{Documento, RAIZ};

/// De onde vem cada folha de estilo da pagina, na ordem do documento.
#[derive(Clone, Debug, PartialEq)]
pub enum FonteCss {
    /// Conteudo de <style>.
    Embutida(String),
    /// href de <link rel=stylesheet> (ainda relativo).
    Externa(String),
}

pub fn fontes_css(doc: &Documento) -> Vec<FonteCss> {
    let mut v = Vec::new();
    for id in doc.descendentes(RAIZ) {
        let Some(e) = doc.elemento(id) else { continue };
        if e.eh(a::STYLE) {
            let midia_ok = e.atributo(a::MEDIA).is_none_or(|m| {
                let m = m.trim().to_ascii_lowercase();
                m.is_empty() || m.contains("screen") || m.contains("all") || m.starts_with('(')
            });
            if midia_ok {
                v.push(FonteCss::Embutida(doc.texto(id)));
            }
        } else if e.eh(a::LINK) {
            let rel = e.atributo(a::REL).unwrap_or("").to_ascii_lowercase();
            let eh_folha = rel.split_ascii_whitespace().any(|r| r == "stylesheet") && !rel.contains("alternate");
            let midia_ok = e.atributo(a::MEDIA).is_none_or(|m| !m.trim().eq_ignore_ascii_case("print"));
            let desligada = e.tem(a::DISABLED);
            if eh_folha && midia_ok && !desligada {
                if let Some(h) = e.atributo(a::HREF) {
                    v.push(FonteCss::Externa(h.into()));
                }
            }
        }
        let _ = Atomo::default();
    }
    v
}
