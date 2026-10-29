use teste_web::web::js::{self, vm::Vm};
fn rodar(fonte: &str) -> String {
    let mut vm = Vm::novo();
    match js::avaliar(&mut vm, fonte) {
        Ok(v) => vm.para_texto(&v).map(|s| s.to_string()).unwrap_or_else(|_| "<obj>".into()),
        Err(e) => format!("ERRO: {}", js::descrever_erro(&mut vm, &e)),
    }
}
#[test]
fn minimo() {
    let casos = std::fs::read_to_string("C:/dev/mxos-tmp/casos.txt").unwrap();
    if let Ok(a) = std::fs::read_to_string("C:/dev/mxos-tmp/arquivo.js") { println!("ARQ => {}", rodar(&a)); }
    for f in casos.lines() {
        println!("{} => {}", f, rodar(f));
    }
    for f in [
        "function t(e){return e+1} !function(){ if(function(){let t=1;return t}()==2) return; r = t(1) }(); r",
        "function t(e){return e+1} (function(){ var f = function(){let t=1;return t}; return t(1) })()",
        "function t(e){return e+1} (function(){ return t(1) })()",
        "let e; function t(x){ return x } (function(){ let n = t(3); return n })()",
    ] {
        println!("{} => {}", f, rodar(f));
    }
}
#[test]
fn transformar_modulo() {
    for f in [
        "import padrao, { dobro as d, NOME } from './lib/a.js';\nimport * as tudo from './lib/b.js';\nwindow.r = [padrao, d(21)].join();\nimport('./lib/b.js').then(m => { window.din = m.b; });",
        "export let b = 7; export { dobro } from './a.js'; export * from './c.js';",
    ] {
        let m = teste_web::web::js::modulo::transformar(f, "https://x/y/", &[]).unwrap();
        println!("MOD {}\nDEPS {:?}", m.codigo, m.deps);
    }
}

#[test]
fn velocidade_compilacao() {
    use std::time::Instant;
    let pasta = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/paginas/");
    for nome in ["https___github_githubassets_com_assets_landing_pages_016ffb52746df72b_js", "https___github_githubassets_com_assets_primer_react_911369ee234690ef_js"] {
        let Ok(f) = std::fs::read_to_string(format!("{pasta}{nome}")) else { continue };
        let t = Instant::now();
        let m = teste_web::web::js::modulo::transformar(&f, "https://x/", &[]).unwrap();
        let t1 = t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        let toks = teste_web::web::js::parser::tokenizar(&m.codigo).unwrap();
        let t2 = t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        let ast = teste_web::web::js::parser::analisar(&m.codigo).unwrap();
        let t3 = t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        let p = teste_web::web::js::compilador::compilar_script(&ast, &m.codigo).unwrap();
        let t4 = t.elapsed().as_secs_f64() * 1000.0;
        println!("VEL {} KB: transformar {t1:.1} ms, tokens {t2:.1} ms ({} tokens), analisar(tokens+parse) {t3:.1} ms, compilar {t4:.1} ms", f.len() / 1024, toks.len());
        drop(p);
    }
}

#[test]
fn tamanhos_ast() {
    use teste_web::web::js::ast::*;
    println!("TAM Expr {} Stmt {} Padrao {} Funcao {} Op {}", std::mem::size_of::<Expr>(), std::mem::size_of::<Stmt>(), std::mem::size_of::<Padrao>(), std::mem::size_of::<Funcao>(), std::mem::size_of::<teste_web::web::js::bytecode::Op>());
}
