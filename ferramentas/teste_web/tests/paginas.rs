//! Paginas reais: baixadas pelo TLS do MXOS e analisadas pelo HTML5.

use std::time::Instant;

use teste_web::rede;
use teste_web::web::{dom, html};

pub const SITES: &[&str] = &[
    "https://example.com/",
    "https://pt.wikipedia.org/wiki/Brasil",
    "https://github.com/",
    "https://www.uol.com.br/",
    "https://news.ycombinator.com/",
    "https://www.gov.br/pt-br",
    "https://www.google.com/",
    "https://developer.mozilla.org/pt-BR/",
];

#[test]
fn analisar_paginas_reais() {
    for url in SITES {
        let (tipo, bytes) = match rede::pagina(url) {
            Ok(x) => x,
            Err(e) => {
                println!("{url}: nao baixou ({e})");
                continue;
            }
        };
        let inicio = Instant::now();
        let doc = html::analisar_bytes(&bytes, Some(&tipo));
        let t = inicio.elapsed();
        let elementos = doc.nos.iter().filter(|n| matches!(n.tipo, dom::Tipo::Elemento(_))).count();
        println!(
            "{url:45} {:8} bytes {:6} nos {:6} elementos {:5.1} ms  titulo: {}",
            bytes.len(),
            doc.nos.len(),
            elementos,
            t.as_secs_f64() * 1000.0,
            doc.titulo()
        );
        assert!(doc.corpo().is_some(), "{url} sem body");
    }
}
