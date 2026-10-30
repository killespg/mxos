//! Decomposicao do custo do laco (diagnostico do interpretador).

use std::time::Instant;

use teste_web::web::js::{self, vm::Vm};

fn medir(nome: &str, fonte: &str) {
    let mut vm = Vm::novo();
    let t = Instant::now();
    let r = js::avaliar(&mut vm, fonte);
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    println!("{nome:28} {ms:8.1} ms  {}", r.is_ok());
}

#[test]
fn decompor() {
    medir("vazio top-level", "for (let i = 0; i < 3000000; i++) {}");
    medir("vazio em funcao", "(function () { for (let i = 0; i < 3000000; i++) {} })()");
    medir("var em funcao", "(function () { for (var i = 0; i < 3000000; i++) {} })()");
    medir("while i+=1 em funcao", "(function () { let i = 0; while (i < 3000000) i += 1; })()");
    medir("soma em funcao", "(function () { let s = 0; for (let i = 0; i < 3000000; i++) s = s + i; return s; })()");
    medir("soma top-level", "let s = 0; for (let i = 0; i < 3000000; i++) s = s + i;");
}
