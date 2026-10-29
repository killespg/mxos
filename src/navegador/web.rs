//! O motor web novo (src/web) dentro do MXOS: busca a pagina, as folhas de
//! estilo e as imagens (disco ou rede, com HTTPS proprio), monta o layout
//! e pinta na Superficie da janela. Substitui o motor antigo no app.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::UnsafeCell;

use crate::gui::grafico::{Retangulo, Superficie};
use crate::web::atomo::{Atomo, a};
use crate::web::css::estilo::Familia;
use crate::web::css::folha::{Folha, Midia};
use crate::web::css::valores::Imagem as ImagemCss;
use crate::web::css::{FonteCss, fontes_css};
use crate::web::dom::Id;
use crate::web::fonte::Fontes;
use crate::web::html::{self, codificacao};
use crate::web::imagem::{self, Imagem};
use crate::web::pagina::Pagina;
use crate::web::pintura::desenho::Imagens;
use crate::web::pintura::quadro::Quadro;
use crate::web::url;

/// Tempo maximo de cada pedaco (CSS, imagem).
const TEMPO_RECURSO_MS: u64 = 8000;
const TEMPO_PAGINA_MS: u64 = 20000;
const MAX_FOLHAS: usize = 24;
const MAX_IMAGENS: usize = 40;
const MAX_FUNDOS: usize = 20;
const MAX_BYTES_IMAGEM: usize = 6 << 20;

static INTER: &[u8] = include_bytes!("../../ferramentas/fontes/Inter-Regular.ttf");
static INTER_NEGRITO: &[u8] = include_bytes!("../../ferramentas/fontes/Inter-Bold.ttf");
static MONO: &[u8] = include_bytes!("../../ferramentas/fontes/JetBrainsMono-Regular.ttf");

struct Global(UnsafeCell<Option<Fontes>>);
unsafe impl Sync for Global {}
static FONTES: Global = Global(UnsafeCell::new(None));

/// As fontes do motor (carregadas na primeira vez). So a GUI usa.
fn fontes() -> &'static mut Fontes {
    let f = unsafe { &mut *FONTES.0.get() };
    f.get_or_insert_with(|| {
        let mut f = Fontes::novo();
        f.adicionar(INTER.to_vec(), Familia::SemSerif, false, false);
        f.adicionar(INTER_NEGRITO.to_vec(), Familia::SemSerif, true, false);
        f.adicionar(MONO.to_vec(), Familia::Mono, false, false);
        // serifadas e a reserva de caracteres (grego, cirilico, simbolos): do disco
        let dejavu = "/usr/share/fonts/truetype/dejavu/";
        for (nome, fam, negrito) in [
            ("DejaVuSerif.ttf", Familia::Serif, false),
            ("DejaVuSerif-Bold.ttf", Familia::Serif, true),
            ("DejaVuSans.ttf", Familia::SemSerif, false),
        ] {
            if let Ok(d) = crate::armazenamento::ler(&alloc::format!("{dejavu}{nome}")) {
                f.adicionar(d, fam, negrito, false);
            }
        }
        f
    })
}

#[derive(Default)]
struct Imgs {
    por_no: BTreeMap<Id, Imagem>,
    por_url: BTreeMap<String, Imagem>,
}

impl Imagens for Imgs {
    fn do_elemento(&self, id: Id) -> Option<&Imagem> {
        self.por_no.get(&id)
    }
    fn da_url(&self, u: &str) -> Option<&Imagem> {
        self.por_url.get(u)
    }
}

/// Uma pagina aberta e pronta para desenhar.
pub struct PaginaWeb {
    pag: Pagina,
    imgs: Imgs,
    pub url: String,
    pub titulo: String,
    pub altura: i32,
    /// Por que virou pagina de erro (None = deu certo).
    pub erro: Option<&'static str>,
}

/// (bytes, content-type, url final) de um endereco do disco ou da rede.
fn buscar(endereco: &str, tempo: u64) -> Result<(Vec<u8>, Option<String>, String), &'static str> {
    if let Some(caminho) = endereco.strip_prefix("disco:") {
        let caminho = caminho.split(['#', '?']).next().unwrap_or("");
        let bytes = crate::armazenamento::ler(caminho).map_err(|e| e.descricao())?;
        return Ok((bytes, None, String::from(endereco)));
    }
    if endereco.starts_with("data:") {
        return url::dados(endereco).map(|d| (d, None, String::from(endereco))).ok_or("data: inválido");
    }
    let r = crate::rede::http_get(endereco, tempo).map_err(|e| e.descricao())?;
    if r.status >= 400 && r.corpo.is_empty() {
        return Err("o servidor respondeu com erro");
    }
    let tipo = r.cabecalho("content-type").map(String::from);
    Ok((r.corpo, tipo, r.url_final))
}

fn imagem_de(u: &str) -> Option<Imagem> {
    let (bytes, _, _) = buscar(u, TEMPO_RECURSO_MS).ok()?;
    if bytes.len() > MAX_BYTES_IMAGEM {
        return None;
    }
    Some(imagem::decodificar(&bytes)?.reduzir(1920, 1920))
}

fn src_de(e: &crate::web::dom::Elemento) -> Option<String> {
    let s = e.atributo(a::SRC).filter(|s| !s.trim().is_empty() && !s.starts_with("data:image/gif"));
    let tardio = e.atributo(Atomo::de("data-src"));
    let srcset = e.atributo(a::SRCSET).and_then(|s| s.split(',').next()).and_then(|c| c.split_whitespace().next());
    s.or(tardio).or(srcset).map(String::from)
}

/// Abre um endereco (nunca falha: erro vira uma pagina explicando).
pub fn abrir(endereco: &str, largura: i32, altura: i32) -> PaginaWeb {
    let (html_bytes, tipo, final_, erro) = match buscar(endereco, TEMPO_PAGINA_MS) {
        Ok((b, t, f)) => (b, t, f, None),
        Err(motivo) => (super::pagina_de_erro(endereco, motivo).into_bytes(), None, String::from(endereco), Some(motivo)),
    };
    let doc = html::analisar_bytes(&html_bytes, tipo.as_deref());
    let base = final_.clone();
    let m = Midia { largura: largura as f32, altura: altura as f32 };
    let mut folhas = Vec::new();
    for f in fontes_css(&doc).into_iter().take(MAX_FOLHAS) {
        match f {
            FonteCss::Embutida(t) => folhas.push(Folha::ler_com_base(&t, &m, &base)),
            FonteCss::Externa(h) => {
                let u = url::resolver(&base, &h);
                if let Ok((b, t, _)) = buscar(&u, TEMPO_RECURSO_MS) {
                    folhas.push(Folha::ler_com_base(&codificacao::texto(&b, t.as_deref()), &m, &u));
                }
            }
        }
    }
    let mut imgs = Imgs::default();
    let mut n = 0;
    for id in doc.descendentes(0) {
        if n >= MAX_IMAGENS {
            break;
        }
        let Some(e) = doc.elemento(id) else { continue };
        if !e.eh(a::IMG) {
            continue;
        }
        if let Some(s) = src_de(e) {
            n += 1;
            if let Some(i) = imagem_de(&url::resolver(&base, &s)) {
                imgs.por_no.insert(id, i);
            }
        }
    }
    let tamanhos: BTreeMap<Id, (i32, i32)> = imgs.por_no.iter().map(|(k, v)| (*k, (v.largura, v.altura))).collect();
    let pag = Pagina::montar(doc, folhas, largura, altura, fontes(), &|id| tamanhos.get(&id).copied());
    // fundos do comeco da pagina
    let mut urls: Vec<String> = Vec::new();
    for c in &pag.arvore.caixas {
        if c.y > 2 * altura * 64 || urls.len() >= MAX_FUNDOS {
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
    for u in urls {
        if let Some(i) = imagem_de(&url::resolver(&base, &u)) {
            imgs.por_url.insert(u, i);
        }
    }
    let titulo = pag.titulo.clone();
    let altura_total = pag.altura;
    PaginaWeb { pag, imgs, url: final_, titulo, altura: altura_total, erro }
}

/// Pinta a pagina na area da superficie, com `rolagem` px escondidos em cima.
pub fn desenhar(p: &PaginaWeb, s: &mut Superficie, area: Retangulo, rolagem: i32) {
    if area.vazio() {
        return;
    }
    let mut q = Quadro::novo(area.largura, area.altura, p.pag.cor_fundo);
    p.pag.pintar(fontes(), &mut q, rolagem, &p.imgs);
    let largura_s = s.largura() as i32;
    let altura_s = s.altura() as i32;
    let destino = s.pixels_mut();
    for y in 0..area.altura {
        let yy = area.y + y;
        if yy < 0 || yy >= altura_s {
            continue;
        }
        let x0 = area.x.max(0);
        let x1 = (area.x + area.largura).min(largura_s);
        if x1 <= x0 {
            continue;
        }
        let o = (y * area.largura + (x0 - area.x)) as usize;
        let d = (yy * largura_s + x0) as usize;
        let n = (x1 - x0) as usize;
        destino[d..d + n].copy_from_slice(&q.px[o..o + n]);
    }
}

/// O endereco do link no ponto (x, y) da area da pagina.
pub fn link_em(p: &PaginaWeb, x: i32, y: i32, rolagem: i32) -> Option<String> {
    let id = p.pag.link_em(x, y, rolagem)?;
    let doc = p.pag.doc.borrow();
    let href = doc.elemento(id)?.atributo(a::HREF)?;
    if href.trim_start().starts_with("javascript:") {
        return None;
    }
    Some(url::resolver(&p.url, href))
}
