//! Monta o src/web do kernel no PC (e a cripto + TLS para baixar paginas
//! de verdade nos testes).

extern crate alloc;

#[path = "../../../src/web/mod.rs"]
pub mod web;

#[path = "../../../src/cripto/mod.rs"]
pub mod cripto;

#[path = "../../../src/rede/tls.rs"]
pub mod tls;

#[path = "../../../src/json.rs"]
pub mod json;

#[path = "../../../src/gui/realce.rs"]
pub mod realce;

pub mod rede;
pub mod png;

/// As fontes do MXOS (as mesmas do pendrive).
pub fn fontes() -> web::fonte::Fontes {
    use web::css::estilo::Familia;
    let raiz = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut f = web::fonte::Fontes::novo();
    let lista = [
        ("ferramentas/fontes/Inter-Regular.ttf", Familia::SemSerif, false),
        ("ferramentas/fontes/Inter-Bold.ttf", Familia::SemSerif, true),
        ("saida/pendrive/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf", Familia::Serif, false),
        ("saida/pendrive/usr/share/fonts/truetype/dejavu/DejaVuSerif-Bold.ttf", Familia::Serif, true),
        ("ferramentas/fontes/JetBrainsMono-Regular.ttf", Familia::Mono, false),
        ("saida/pendrive/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", Familia::SemSerif, false),
        ("saida/pendrive/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf", Familia::SemSerif, true),
    ];
    for (c, fam, b) in lista {
        if let Ok(d) = std::fs::read(raiz.join(c)) {
            assert!(f.adicionar(d, fam, b, false), "fonte {c} nao abriu");
        }
    }
    f
}
pub mod render;
