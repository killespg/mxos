//! Desenha uma pagina inteira (HTML + CSS + imagens) num PNG em
//! target/saidas, baixando tudo pelo TLS do MXOS.

use std::collections::HashMap;
use std::time::Instant;

use crate::web::atomo::a;
use crate::web::css::folha::{Folha, Midia};
use crate::web::css::valores::Imagem as ImagemCss;
use crate::web::css::{FonteCss, fontes_css};
use crate::web::dom::Id;
use crate::web::html::{self, codificacao};
use crate::web::imagem::{self, Imagem};
use crate::web::pagina::Pagina;
use crate::web::pintura::desenho::Imagens;
use crate::web::pintura::quadro::Quadro;
use crate::web::url;

#[derive(Default)]
pub struct Imgs {
    pub por_no: HashMap<Id, Imagem>,
    pub por_url: HashMap<String, Imagem>,
}

impl Imagens for Imgs {
    fn do_elemento(&self, id: Id) -> Option<&Imagem> {
        self.por_no.get(&id)
    }
    fn da_url(&self, u: &str) -> Option<&Imagem> {
        self.por_url.get(u)
    }
}

fn baixar_imagem(u: &str) -> Option<Imagem> {
    let bytes = if u.starts_with("data:") {
        url::dados(u)?
    } else if u.starts_with("https://") {
        crate::rede::pagina(u).ok()?.1
    } else {
        return None;
    };
    Some(imagem::decodificar(&bytes)?.reduzir(2048, 2048))
}

/// A melhor URL de um <img>: src, data-src ou o primeiro do srcset.
fn src_de(e: &crate::web::dom::Elemento) -> Option<String> {
    let s = e.atributo(a::SRC).filter(|s| !s.trim().is_empty() && !s.starts_with("data:image/gif"));
    let alt = e.atributo(crate::web::atomo::Atomo::de("data-src"));
    let srcset = e.atributo(a::SRCSET).and_then(|s| s.split(',').next()).and_then(|c| c.split_whitespace().next());
    s.or(alt).or(srcset).map(String::from)
}

pub struct Carregada {
    pub pag: Pagina,
    pub imgs: Imgs,
    pub tempos: [f64; 5],
}

/// HTML + CSS + imagens -> pagina montada.
pub fn carregar(bytes: &[u8], tipo: Option<&str>, base: &str, largura: i32) -> Carregada {
    let fontes = crate::fontes();
    let ms = |a: Instant, b: Instant| (b - a).as_secs_f64() * 1000.0;
    let t0 = Instant::now();
    let doc = html::analisar_bytes(bytes, tipo);
    let t1 = Instant::now();
    let m = Midia { largura: largura as f32, altura: 800.0 };
    let mut folhas = Vec::new();
    for f in fontes_css(&doc) {
        match f {
            FonteCss::Embutida(t) => folhas.push(Folha::ler_com_base(&t, &m, base)),
            FonteCss::Externa(h) if !base.is_empty() => {
                let u = url::resolver(base, &h);
                if let Ok((t, b)) = crate::rede::pagina(&u) {
                    folhas.push(Folha::ler_com_base(&codificacao::texto(&b, Some(&t)), &m, &u));
                }
            }
            _ => {}
        }
    }
    let t2 = Instant::now();
    // imagens dos <img>
    let mut imgs = Imgs::default();
    if !base.is_empty() {
        for id in doc.descendentes(0) {
            let Some(e) = doc.elemento(id) else { continue };
            if !e.eh(a::IMG) {
                continue;
            }
            if let Some(s) = src_de(e) {
                if let Some(i) = baixar_imagem(&url::resolver(base, &s)) {
                    imgs.por_no.insert(id, i);
                }
            }
        }
    }
    let t3 = Instant::now();
    let tamanhos: HashMap<Id, (i32, i32)> = imgs.por_no.iter().map(|(k, v)| (*k, (v.largura, v.altura))).collect();
    let pag = Pagina::montar(doc, folhas, largura, 800, &fontes, &|id| tamanhos.get(&id).copied());
    let t4 = Instant::now();
    // fundos em CSS (so as visiveis no comeco da pagina, para nao baixar tudo)
    if !base.is_empty() {
        let mut urls: Vec<String> = Vec::new();
        for c in &pag.arvore.caixas {
            if c.y > 3000 * 64 {
                continue;
            }
            if let Some(i) = &c.estilo.imagem_fundo {
                if let ImagemCss::Url(u) = &**i {
                    if !urls.contains(u) {
                        urls.push(u.clone());
                    }
                }
            }
        }
        for u in urls.into_iter().take(40) {
            if let Some(i) = baixar_imagem(&url::resolver(base, &u)) {
                imgs.por_url.insert(u, i);
            }
        }
    }
    Carregada { pag, imgs, tempos: [ms(t0, t1), ms(t1, t2), ms(t2, t3), ms(t3, t4), 0.0] }
}

/// Monta e desenha. Devolve (altura, tempos em ms: html, css, imagens,
/// layout, pintura).
pub fn renderizar(nome: &str, bytes: &[u8], tipo: Option<&str>, base: &str, largura: i32, altura_max: i32) -> (i32, [f64; 5]) {
    let mut c = carregar(bytes, tipo, base, largura);
    let mut fontes = crate::fontes();
    let altura = c.pag.altura.clamp(100, altura_max);
    let mut q = Quadro::novo(largura, altura, 0xFFFFFF);
    let t = Instant::now();
    c.pag.pintar(&mut fontes, &mut q, 0, &c.imgs);
    c.tempos[4] = t.elapsed().as_secs_f64() * 1000.0;
    crate::png::gravar(&crate::png::pasta().join(format!("{nome}.png")), largura as usize, altura as usize, &q.px);
    (c.pag.altura, c.tempos)
}

/// A arvore de caixas em texto (tag.classe tipo x y l a), ate `prof` niveis.
pub fn despejar(pag: &Pagina, prof: usize) -> String {
    use crate::web::css::valores::UM;
    use crate::web::layout::caixa::Tipo;
    let mut s = String::new();
    fn um(pag: &Pagina, k: u32, n: usize, prof: usize, s: &mut String) {
        if n > prof {
            return;
        }
        let c = &pag.arvore.caixas[k as usize];
        let nome = match pag.doc.borrow().elemento(c.no) {
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
                Tipo::Texto(t) => format!("\"{}\"", t.chars().take(30).collect::<String>()),
                _ => "(anon)".into(),
            },
        };
        let tipo = match &c.tipo {
            Tipo::Texto(_) => "Texto".to_string(),
            t => format!("{t:?}").chars().take(30).collect(),
        };
        s.push_str(&format!("{}{} {} {:?} x={} y={} l={} a={}\n", "  ".repeat(n), nome, tipo, c.estilo.exibir, c.x / UM, c.y / UM, c.l / UM, c.a / UM));
        for &f in &c.filhos {
            um(pag, f, n + 1, prof, s);
        }
    }
    um(pag, 0, 0, prof, &mut s);
    s
}

/// Monta a pagina de uma URL (com CSS e imagens) sem desenhar.
pub fn montar(u: &str, largura: i32) -> Pagina {
    let (tipo, bytes) = crate::rede::pagina(u).unwrap();
    carregar(&bytes, Some(&tipo), u, largura).pag
}
