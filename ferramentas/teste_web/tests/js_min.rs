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
