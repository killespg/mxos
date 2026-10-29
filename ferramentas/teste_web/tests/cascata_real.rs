//! Cascata nas paginas reais (com o CSS externo delas).

use std::time::Instant;

use teste_web::rede;
use teste_web::web::css::cascata::{self, Indice};
use teste_web::web::css::folha::{Folha, Midia};
use teste_web::web::css::seletor::Estado;
use teste_web::web::css::{FonteCss, fontes_css};
use teste_web::web::html::{self, codificacao};
use teste_web::web::url;

#[path = "paginas.rs"]
#[allow(dead_code)]
mod paginas;

#[test]
fn cascata_em_paginas_reais() {
    let m = Midia { largura: 1280.0, altura: 720.0 };
    for site in paginas::SITES {
        let Ok((tipo, bytes)) = rede::pagina(site) else { continue };
        let doc = html::analisar_bytes(&bytes, Some(&tipo));
        let mut textos = Vec::new();
        for f in fontes_css(&doc) {
            match f {
                FonteCss::Embutida(t) => textos.push(t),
                FonteCss::Externa(h) => {
                    let u = url::resolver(site, &h);
                    if let Ok((t, b)) = rede::pagina(&u) {
                        textos.push(codificacao::texto(&b, Some(&t)));
                    }
                }
            }
        }
        let bytes_css: usize = textos.iter().map(|t| t.len()).sum();
        let t0 = Instant::now();
        let folhas: Vec<Folha> = textos.iter().map(|t| Folha::ler(t, &m)).collect();
        let t1 = Instant::now();
        let ind = Indice::novo(folhas, &m);
        let est = cascata::calcular(&doc, &ind, &m, Estado::default());
        let t2 = Instant::now();
        let com_estilo = est.por_no.iter().filter(|x| x.is_some()).count();
        let unicos = {
            let mut v: Vec<*const _> = est.por_no.iter().flatten().map(|r| std::rc::Rc::as_ptr(r)).collect();
            v.sort();
            v.dedup();
            v.len()
        };
        println!(
            "{site:42} css {:8} bytes {:5} regras  ler {:6.1} ms  cascata {:6.1} ms  {com_estilo} elementos, {unicos} estilos distintos",
            bytes_css,
            ind.quantas_regras(),
            (t1 - t0).as_secs_f64() * 1000.0,
            (t2 - t1).as_secs_f64() * 1000.0
        );
    }
}
