//! Micro-benchmark do interpretador JS: o que os sites mais fazem.
//!   cargo +stable-x86_64-pc-windows-gnu test --release --test js_bench -- --nocapture

use std::time::Instant;

use teste_web::web::js::{self, vm::Vm};

fn medir(nome: &str, fonte: &str) {
    let mut vm = Vm::novo();
    let t = Instant::now();
    let r = js::avaliar(&mut vm, fonte);
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    let v = match r {
        Ok(v) => vm.para_texto(&v).map(|s| s.to_string()).unwrap_or_default(),
        Err(e) => format!("ERRO {}", js::descrever_erro(&mut vm, &e)),
    };
    println!("{nome:14} {ms:8.1} ms   ({v})");
}

#[test]
fn bench() {
    medir("laco_conta", "let s = 0; for (let i = 0; i < 3000000; i++) { s = (s + i * 3) % 1000003; } s");
    medir("propriedades", "const o = {a: 1, b: 2, c: 3}; let s = 0; for (let i = 0; i < 1000000; i++) { o.a = o.b + o.c; s += o.a; } s");
    medir("arrays", "const a = []; for (let i = 0; i < 300000; i++) a.push(i * 2); let s = 0; for (let i = 0; i < a.length; i++) s += a[i]; s");
    medir("chamadas", "function f(x, y) { return x + y; } let s = 0; for (let i = 0; i < 1000000; i++) s = f(s, i) % 1000003; s");
    medir("closures", "function mk(k) { return x => x + k; } const fs = []; for (let i = 0; i < 100; i++) fs.push(mk(i)); let s = 0; for (let j = 0; j < 10000; j++) for (const f of fs) s = f(s) % 99991; s");
    medir("strings", "let s = ''; for (let i = 0; i < 100000; i++) s += String.fromCharCode(97 + i % 26); s.length + s.indexOf('xyz')");
    medir("objetos_novos", "let s = 0; for (let i = 0; i < 200000; i++) { const p = {x: i, y: i + 1, z: [i]}; s += p.x + p.z[0]; } s");
    medir("metodos", "class P { constructor(x) { this.x = x; } soma(y) { return this.x + y; } } const p = new P(3); let s = 0; for (let i = 0; i < 500000; i++) s = p.soma(s) % 100003; s");
    medir("json", "const o = {a: [1, 2, 3], b: {c: 'texto', d: true}}; let n = 0; for (let i = 0; i < 20000; i++) n += JSON.stringify(JSON.parse(JSON.stringify(o))).length; n");
    medir("regex", "const r = /([a-z]+)-(\\d+)/; let n = 0; for (let i = 0; i < 50000; i++) { const m = r.exec('item-' + i); if (m) n += m[2].length; } n");
}
