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
