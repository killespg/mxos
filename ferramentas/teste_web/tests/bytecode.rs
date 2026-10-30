//! Mostra o bytecode de uma funcao (diagnostico do compilador).

use teste_web::web::js::{compilador, parser};

#[test]
fn mostrar() {
    let fonte = std::env::var("JS").unwrap_or("function f() { let s = 0; for (let i = 0; i < 100; i++) s = (s + i * 3) % 1000003; return s; } f.x = o.a + o.b;".into());
    let ast = parser::analisar(&fonte).unwrap();
    let p = compilador::compilar_script(&ast, &fonte).unwrap();
    fn mostrar(p: &teste_web::web::js::bytecode::Prototipo, prof: usize) {
        println!("{}== {} ({} ops)", "  ".repeat(prof), p.nome, p.codigo.len());
        for (i, op) in p.codigo.iter().enumerate() {
            println!("{}{i:4} {op:?}", "  ".repeat(prof));
        }
        for f in &p.filhos {
            mostrar(f, prof + 1);
        }
    }
    mostrar(&p, 0);
}
