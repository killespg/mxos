//! Perfil do interpretador num laco simples (amostras em saida/perfil_*.txt).

use teste_web::web::js::{self, vm::Vm};

#[test]
fn perfil_laco() {
    let a = teste_web::perfil::amostrar(|| {
        let mut vm = Vm::novo();
        let _ = js::avaliar(&mut vm, "(function () { let s = 0; for (let i = 0; i < 20000000; i++) s = (s + i * 3) % 1000003; return s; })()");
    });
    println!("{} amostras", a.len());
    teste_web::perfil::gravar("laco", &a);
}

#[test]
fn perfil_objetos() {
    let a = teste_web::perfil::amostrar(|| {
        let mut vm = Vm::novo();
        let _ = js::avaliar(
            &mut vm,
            "class P { constructor(x) { this.x = x; } soma(y) { return this.x + y; } } const p = new P(3); let s = 0; for (let i = 0; i < 3000000; i++) { s = p.soma(s) % 100003; const o = {a: i, b: [i]}; s += o.a - o.b[0]; } s",
        );
    });
    println!("{} amostras", a.len());
    teste_web::perfil::gravar("objetos", &a);
}

#[test]
fn perfil_chamadas() {
    let a = teste_web::perfil::amostrar(|| {
        let mut vm = Vm::novo();
        let _ = js::avaliar(&mut vm, "(function () { function f(x, y) { return x + y; } let s = 0; for (let i = 0; i < 4000000; i++) s = f(s, i) % 1000003; return s; })()");
    });
    println!("{} amostras", a.len());
    teste_web::perfil::gravar("chamadas", &a);
}

#[test]
fn perfil_props() {
    let a = teste_web::perfil::amostrar(|| {
        let mut vm = Vm::novo();
        let _ = js::avaliar(&mut vm, "(function () { const o = {a: 1, b: 2, c: 3, d: 4, e: 5, f: 6}; let s = 0; for (let i = 0; i < 4000000; i++) { o.a = o.b + o.e; s += o.a; } return s; })()");
    });
    println!("{} amostras", a.len());
    teste_web::perfil::gravar("props", &a);
}

#[test]
fn perfil_pagina() {
    use teste_web::web::css::folha::{Folha, Midia};
    use teste_web::web::html::{self, codificacao};
    let url = std::env::var("URL").unwrap_or("https://github.com/".into());
    let (tipo, bytes) = teste_web::rede::pagina(&url).unwrap();
    // os CSS externos, baixados uma vez so (fora da medicao)
    let doc = html::analisar_bytes(&bytes, Some(&tipo));
    let mut css: Vec<(String, String)> = Vec::new();
    for f in teste_web::render::fontes_css_pub(&doc) {
        match f {
            (None, t) => css.push((url.clone(), t)),
            (Some(h), _) => {
                let u = teste_web::web::url::resolver(&url, &h);
                if let Ok((t, b)) = teste_web::rede::pagina(&u) {
                    css.push((u, codificacao::texto(&b, Some(&t))));
                }
            }
        }
    }
    println!("{} folhas", css.len());
    let a = teste_web::perfil::amostrar(move || {
        let fontes = teste_web::fontes();
        for _ in 0..4 {
            let t = std::time::Instant::now();
            let doc = html::analisar_bytes(&bytes, Some(&tipo));
            let m = Midia { largura: 1280.0, altura: 800.0 };
            let folhas: Vec<Folha> = css.iter().map(|(u, t)| Folha::ler_com_base(t, &m, u)).collect();
            let t1 = t.elapsed().as_secs_f64() * 1000.0;
            let pag = teste_web::web::pagina::Pagina::montar(doc, folhas, 1280, 800, &fontes, &|_| None);
            println!("html+css {t1:.0} ms, montar {:.0} ms, altura {}", t.elapsed().as_secs_f64() * 1000.0 - t1, pag.altura);
        }
    });
    println!("{} amostras", a.len());
    teste_web::perfil::gravar("pagina", &a);
}

#[test]
fn perfil_imagens() {
    use teste_web::web::{dom, html, url};
    let pagina = std::env::var("URL").unwrap_or("https://www.uol.com.br/".into());
    let (tipo, bytes) = teste_web::rede::pagina(&pagina).unwrap();
    let doc = html::analisar_bytes(&bytes, Some(&tipo));
    let mut imagens: Vec<Vec<u8>> = Vec::new();
    for id in doc.descendentes(0) {
        let Some(e) = doc.elemento(id) else { continue };
        if !e.eh(teste_web::web::atomo::a::IMG) {
            continue;
        }
        let Some(s) = e.atributo(teste_web::web::atomo::a::SRC) else { continue };
        if let Ok((_, b)) = teste_web::rede::pagina(&url::resolver(&pagina, s)) {
            imagens.push(b);
        }
        if imagens.len() >= 30 {
            break;
        }
    }
    let _ = dom::RAIZ;
    println!("{} imagens, {} KB", imagens.len(), imagens.iter().map(|b| b.len()).sum::<usize>() / 1024);
    let a = teste_web::perfil::amostrar(move || {
        for _ in 0..5 {
            let t = std::time::Instant::now();
            let mut px = 0;
            for b in &imagens {
                if let Some(i) = teste_web::web::imagem::decodificar_max(b, 1280, 1280) {
                    px += i.px.len();
                }
            }
            println!("decodificar {:.0} ms, {} Mpx", t.elapsed().as_secs_f64() * 1000.0, px / 1_000_000);
        }
    });
    teste_web::perfil::gravar("imagens", &a);
}
