//! Desenha uma pagina inteira (HTML + CSS) num PNG em target/saidas.

use std::time::Instant;

use crate::web::css::folha::{Folha, Midia};
use crate::web::css::{FonteCss, fontes_css};
use crate::web::html::{self, codificacao};
use crate::web::pagina::Pagina;
use crate::web::pintura::desenho::SemImagens;
use crate::web::pintura::quadro::Quadro;
use crate::web::url;

/// Monta e desenha. `base`: URL da pagina (para baixar o CSS externo; ""
/// = nao baixa). Devolve (altura, tempos em ms: html, css, layout, pintura).
pub fn renderizar(nome: &str, bytes: &[u8], tipo: Option<&str>, base: &str, largura: i32, altura_max: i32) -> (i32, [f64; 4]) {
    let fontes_base = crate::fontes();
    let mut fontes = fontes_base;
    let t0 = Instant::now();
    let doc = html::analisar_bytes(bytes, tipo);
    let t1 = Instant::now();
    let m = Midia { largura: largura as f32, altura: 800.0 };
    let mut folhas = Vec::new();
    for f in fontes_css(&doc) {
        match f {
            FonteCss::Embutida(t) => folhas.push(Folha::ler(&t, &m)),
            FonteCss::Externa(h) if !base.is_empty() => {
                let u = url::resolver(base, &h);
                if let Ok((t, b)) = crate::rede::pagina(&u) {
                    folhas.push(Folha::ler(&codificacao::texto(&b, Some(&t)), &m));
                }
            }
            _ => {}
        }
    }
    let t2 = Instant::now();
    let pag = Pagina::montar(doc, folhas, largura, 800, &fontes, &|_| None);
    let t3 = Instant::now();
    let altura = pag.altura.clamp(100, altura_max);
    let mut q = Quadro::novo(largura, altura, 0xFFFFFF);
    pag.pintar(&mut fontes, &mut q, 0, &SemImagens);
    let t4 = Instant::now();
    crate::png::gravar(&crate::png::pasta().join(format!("{nome}.png")), largura as usize, altura as usize, &q.px);
    let ms = |a: Instant, b: Instant| (b - a).as_secs_f64() * 1000.0;
    (pag.altura, [ms(t0, t1), ms(t1, t2), ms(t2, t3), ms(t3, t4)])
}

/// A arvore de caixas em texto (tag.classe tipo x y l a), ate `prof` niveis.
pub fn despejar(pag: &Pagina, prof: usize) -> String {
    use crate::web::css::valores::UM;
    let mut s = String::new();
    fn um(pag: &Pagina, k: u32, n: usize, prof: usize, s: &mut String) {
        if n > prof {
            return;
        }
        let c = &pag.arvore.caixas[k as usize];
        let nome = match pag.doc.elemento(c.no) {
            Some(e) => {
                let mut x = e.nome.texto().to_string();
                if let Some(i) = e.id {
                    x += &format!("#{}", i.texto());
                }
                for cl in e.classes.iter().take(2) {
                    x += &format!(".{}", cl.texto());
                }
                x
            }
            None => match &c.tipo {
                crate::web::layout::caixa::Tipo::Texto(t) => format!("\"{}\"", t.chars().take(30).collect::<String>()),
                _ => "(anon)".into(),
            },
        };
        s.push_str(&format!(
            "{}{} {:?} {:?} x={} y={} l={} a={}\n",
            "  ".repeat(n),
            nome,
            match &c.tipo { crate::web::layout::caixa::Tipo::Texto(_) => crate::web::layout::caixa::Tipo::Quebra, t => t.clone() },
            c.estilo.exibir,
            c.x / UM,
            c.y / UM,
            c.l / UM,
            c.a / UM
        ));
        for &f in &c.filhos {
            um(pag, f, n + 1, prof, s);
        }
    }
    um(pag, 0, 0, prof, &mut s);
    s
}

/// Monta a pagina de uma URL (com o CSS externo) sem desenhar.
pub fn montar(url: &str, largura: i32) -> Pagina {
    let (tipo, bytes) = crate::rede::pagina(url).unwrap();
    let fontes = crate::fontes();
    let doc = html::analisar_bytes(&bytes, Some(&tipo));
    let m = Midia { largura: largura as f32, altura: 800.0 };
    let mut folhas = Vec::new();
    for f in fontes_css(&doc) {
        match f {
            FonteCss::Embutida(t) => folhas.push(Folha::ler(&t, &m)),
            FonteCss::Externa(h) => {
                if let Ok((t, b)) = crate::rede::pagina(&url::resolver(url, &h)) {
                    folhas.push(Folha::ler(&codificacao::texto(&b, Some(&t)), &m));
                }
            }
        }
    }
    Pagina::montar(doc, folhas, largura, 800, &fontes, &|_| None)
}
