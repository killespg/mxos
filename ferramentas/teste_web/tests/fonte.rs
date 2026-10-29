//! Fontes: desenha amostras num PNG (target/saidas/texto.png).

use teste_web::web::css::estilo::Familia;
use teste_web::web::css::valores::{Cor, UM};
use teste_web::web::fonte::Glifo;
use teste_web::web::pintura::quadro::{Quadro, Ret};

#[test]
fn amostra_de_texto() {
    let mut f = teste_web::fontes();
    assert!(f.faces.len() >= 5);
    let mut q = Quadro::novo(900, 560, 0xFFFFFF);
    q.retangulo(Ret::novo(0, 0, 900, 40), Cor(0xFF1E3A8A), 255);
    let mut y = 30;
    let linhas: &[(Familia, u16, bool, f32, &str, u32)] = &[
        (Familia::SemSerif, 700, false, 22.0, "MXOS — navegador feito do zero", 0xFFFFFF),
        (Familia::SemSerif, 400, false, 12.0, "Inter 12px: The quick brown fox jumps over the lazy dog. Ação, coração, pão.", 0),
        (Familia::SemSerif, 400, false, 16.0, "Inter 16px: The quick brown fox jumps over the lazy dog. Ação, coração.", 0),
        (Familia::SemSerif, 700, false, 16.0, "Inter negrito 16px: Olá, mundo! 0123456789 ¿¡ @#$%&*", 0),
        (Familia::SemSerif, 400, true, 16.0, "Italico sintetico 16px: Café com pão, açúcar e maçã", 0),
        (Familia::Serif, 400, false, 18.0, "DejaVu Serif 18px: Wikipédia, a enciclopédia livre", 0x202122),
        (Familia::Serif, 700, false, 24.0, "Serif negrito 24px: Brasil", 0),
        (Familia::Mono, 400, false, 14.0, "JetBrains Mono 14px: fn main() { println!(\"oi\"); }", 0x0000AA),
        (Familia::SemSerif, 400, false, 40.0, "Grande 40px: Ágil ♥ €", 0xC00000),
        (Familia::SemSerif, 400, false, 16.0, "Reserva (DejaVu): Ωμέγα · Привет · ←↑→↓ · ✓ ✗ · ∑∫√", 0),
        (Familia::SemSerif, 400, false, 9.0, "Pequeno 9px: texto miudo de rodape de pagina, ainda legivel?", 0x555555),
    ];
    for &(fam, peso, ital, tam, texto, cor) in linhas {
        let e = f.escolher(fam, peso, ital, tam);
        let (asc, desc) = f.metricas(&e);
        y += (asc + desc) / UM + 8;
        let mut gl: Vec<Glifo> = Vec::new();
        let largura = f.glifos(&e, texto, 0, &mut gl);
        for g in &gl {
            let x = 20 * UM + g.x;
            let b = f.bitmap(&e, g.face, g.id, ((x & 63) / 16) as u8);
            q.glifo(&b, x / UM, y - desc / UM, Cor(0xFF00_0000 | cor), 255);
        }
        assert!(largura > 0);
    }
    teste_web::png::gravar(&teste_web::png::pasta().join("texto.png"), 900, 560, &q.px);
}
