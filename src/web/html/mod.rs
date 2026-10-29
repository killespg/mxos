//! HTML5: codificacao, tokenizador e construcao da arvore.

pub mod arvore;
pub mod codificacao;
pub mod entidades;
pub mod token;

use alloc::string::String;
use core::fmt::Write;

use super::dom::{Documento, Espaco, Id, RAIZ, Tipo};

pub use arvore::analisar;

/// Bytes da rede (com o Content-Type, se veio) para a arvore.
pub fn analisar_bytes(bytes: &[u8], content_type: Option<&str>) -> Documento {
    analisar(&codificacao::texto(bytes, content_type))
}

/// A arvore no formato dos testes do html5lib ("| <html>", com dois
/// espacos por nivel): para depurar e para os testes.
pub fn depurar(doc: &Documento) -> String {
    let mut s = String::new();
    fn no(doc: &Documento, id: Id, nivel: usize, s: &mut String) {
        let recuo = "  ".repeat(nivel);
        match &doc.no(id).tipo {
            Tipo::Documento => {}
            Tipo::Doctype(n) => {
                let _ = writeln!(s, "| {recuo}<!DOCTYPE {n}>");
            }
            Tipo::Comentario(c) => {
                let _ = writeln!(s, "| {recuo}<!-- {c} -->");
            }
            Tipo::Texto(t) => {
                let _ = writeln!(s, "| {recuo}\"{t}\"");
            }
            Tipo::Elemento(e) => {
                let prefixo = match e.espaco {
                    Espaco::Html => "",
                    Espaco::Svg => "svg ",
                    Espaco::MathMl => "math ",
                };
                let _ = writeln!(s, "| {recuo}<{prefixo}{}>", e.nome.texto());
                let mut ats: alloc::vec::Vec<_> = e.atributos.iter().collect();
                ats.sort_by(|x, y| x.nome.texto().cmp(y.nome.texto()));
                for at in ats {
                    let _ = writeln!(s, "| {recuo}  {}=\"{}\"", at.nome.texto(), at.valor);
                }
            }
        }
        let filho_nivel = if id == RAIZ { nivel } else { nivel + 1 };
        for f in doc.filhos(id) {
            no(doc, f, filho_nivel, s);
        }
    }
    no(doc, RAIZ, 0, &mut s);
    s
}
