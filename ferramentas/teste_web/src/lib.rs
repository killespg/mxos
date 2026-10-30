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

#[path = "../../../src/midia/mod.rs"]
pub mod midia;

#[path = "../../../src/chip8.rs"]
pub mod chip8;

pub mod rede;
pub mod png;
pub mod perfil;

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

// ------------------------------------------------ pacotes .mxa (pacotes.rs)

#[allow(dead_code)]
#[path = "../../../src/pacotes.rs"]
pub mod pacotes;

/// So o que o pacotes.rs usa do disco: sem disco nenhum.
pub mod armazenamento {
    use alloc::vec::Vec;

    #[derive(Debug)]
    pub struct Erro;

    impl Erro {
        pub fn descricao(&self) -> &'static str {
            "sem disco no teste"
        }
    }

    pub struct ItemPasta {
        pub nome: alloc::string::String,
        pub pasta: bool,
    }

    pub fn montado() -> bool {
        false
    }
    pub fn ler(_: &str) -> Result<Vec<u8>, Erro> {
        Err(Erro)
    }
    pub fn escrever(_: &str, _: &[u8]) -> Result<(), Erro> {
        Err(Erro)
    }
    pub fn listar(_: &str) -> Result<Vec<ItemPasta>, Erro> {
        Err(Erro)
    }
    pub fn criar_pasta(_: &str) -> Result<(), Erro> {
        Err(Erro)
    }
    pub fn apagar(_: &str) -> Result<(), Erro> {
        Err(Erro)
    }
}

pub mod gui {
    pub mod widgets {
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub enum Icone {
            Pasta, Arquivo, Texto, Html, Disco, Rede, Terminal, Navegador, Engrenagem, Tarefas, Chip, Pincel,
            Calculadora, Info, Teclado, Monitor, Som,
        }
    }
}

pub mod processo {
    pub fn abrir_app(_: &str, _: &str, _: &str) -> Result<u32, alloc::string::String> {
        Err(alloc::string::String::from("sem processos no teste"))
    }
}
