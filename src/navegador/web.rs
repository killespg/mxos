//! O motor web novo (src/web) dentro do MXOS: busca a pagina, as folhas de
//! estilo e as imagens (disco ou rede, com HTTPS proprio), monta o layout
//! e pinta na Superficie da janela. Substitui o motor antigo no app.

use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::{RefCell, UnsafeCell};

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
use crate::web::script::{PedidoRede, Resposta, Scripts};
use crate::web::url;

/// Tempo maximo de cada pedaco (CSS, imagem).
const TEMPO_RECURSO_MS: u64 = 8000;
const TEMPO_PAGINA_MS: u64 = 20000;
const MAX_FOLHAS: usize = 24;
const MAX_IMAGENS: usize = 40;
const MAX_FUNDOS: usize = 20;
const MAX_BYTES_IMAGEM: usize = 6 << 20;
const MAX_BYTES_SCRIPT: usize = 8 << 20;
const MAX_LADO_IMAGEM: i32 = 1280;
/// Pixels de imagem por pagina (16 M = 64 MB): passou, as outras ficam sem.
const MAX_PIXELS_PAGINA: usize = 16 << 20;

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
    tentadas: alloc::collections::BTreeSet<Id>,
    /// Soma dos pixels guardados (limite MAX_PIXELS_PAGINA).
    pixels: usize,
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
    scripts: Option<Scripts>,
    /// agora_ms() quando abriu (o relogio dos timers comeca em 0).
    inicio: f64,
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
    // a janela tem menos de 1280 px: maior que isso so gasta memoria
    imagem::decodificar_max(&bytes, MAX_LADO_IMAGEM, MAX_LADO_IMAGEM)
}

fn src_de(e: &crate::web::dom::Elemento) -> Option<String> {
    let s = e.atributo(a::SRC).filter(|s| !s.trim().is_empty() && !s.starts_with("data:image/gif"));
    let tardio = e.atributo(Atomo::de("data-src"));
    let srcset = e.atributo(a::SRCSET).and_then(|s| s.split(',').next()).and_then(|c| c.split_whitespace().next());
    s.or(tardio).or(srcset).map(String::from)
}

/// Milissegundos desde o boot (timers da pagina).
fn agora_ms() -> f64 {
    let freq = crate::cpu::frequencia_timer().max(1);
    crate::cpu::contador() as f64 * 1000.0 / freq as f64
}

/// Milissegundos desde 1970 (Date.now do JavaScript).
fn relogio_unix() -> f64 {
    crate::processo::rtc_no_boot() as f64 * 1000.0 + agora_ms()
}

/// Texto de um script/modulo/fetch (so GET).
fn buscar_texto(u: &str) -> Option<String> {
    let (b, t, _) = buscar(u, TEMPO_RECURSO_MS).ok()?;
    if b.len() > MAX_BYTES_SCRIPT {
        return None;
    }
    Some(codificacao::texto(&b, t.as_deref()))
}

/// Pedidos de fetch/XMLHttpRequest dos scripts (so GET por enquanto).
fn responder(p: &PedidoRede) -> Option<Resposta> {
    if p.metodo != "GET" {
        return None;
    }
    let r = crate::rede::http_get(p.url, TEMPO_RECURSO_MS).ok()?;
    let cabecalhos = r.cabecalho("content-type").map(|t| alloc::vec![(String::from("content-type"), String::from(t))]).unwrap_or_default();
    let tipo = r.cabecalho("content-type").map(String::from);
    Some(Resposta { status: r.status, corpo: codificacao::texto(&r.corpo, tipo.as_deref()), cabecalhos, url: r.url_final })
}

/// Busca as imagens de <img> ainda sem tentativa (ate o limite).
fn buscar_imagens(doc: &crate::web::dom::Documento, base: &str, imgs: &mut Imgs) {
    for id in doc.descendentes(0) {
        if imgs.tentadas.len() >= MAX_IMAGENS {
            break;
        }
        let Some(e) = doc.elemento(id) else { continue };
        if !e.eh(a::IMG) || imgs.tentadas.contains(&id) {
            continue;
        }
        if let Some(s) = src_de(e) {
            imgs.tentadas.insert(id);
            if imgs.pixels >= MAX_PIXELS_PAGINA {
                continue;
            }
            if let Some(i) = imagem_de(&url::resolver(base, &s)) {
                imgs.pixels += i.px.len();
                imgs.por_no.insert(id, i);
            }
        }
    }
}

/// Abre um endereco (nunca falha: erro vira uma pagina explicando).
pub fn abrir(endereco: &str, largura: i32, altura: i32) -> PaginaWeb {
    unsafe { crate::web::js::embutidos::RELOGIO = Some(relogio_unix) };
    let (html_bytes, tipo, final_, erro) = match buscar(endereco, TEMPO_PAGINA_MS) {
        Ok((b, t, f)) => (b, t, f, None),
        Err(motivo) => (super::pagina_de_erro(endereco, motivo).into_bytes(), None, String::from(endereco), Some(motivo)),
    };
    let doc = Rc::new(RefCell::new(html::analisar_bytes(&html_bytes, tipo.as_deref())));
    let base = final_.clone();
    // scripts primeiro: eles mudam o DOM e poem estilos
    let inicio = agora_ms();
    let mut scripts = if erro.is_none() && javascript_ligado() {
        let mut s = Scripts::novo(doc.clone(), &final_, largura, altura);
        s.rodar_todos(&mut |u| buscar_texto(u));
        s.carregou();
        s.tique(0.0, &mut responder);
        Some(s)
    } else {
        None
    };
    let m = Midia { largura: largura as f32, altura: altura as f32 };
    let mut folhas = Vec::new();
    for f in fontes_css(&doc.borrow()).into_iter().take(MAX_FOLHAS) {
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
    buscar_imagens(&doc.borrow(), &base, &mut imgs);
    let tamanhos: BTreeMap<Id, (i32, i32)> = imgs.por_no.iter().map(|(k, v)| (*k, (v.largura, v.altura))).collect();
    let pag = Pagina::montar_rc(doc, folhas, largura, altura, fontes(), &|id| tamanhos.get(&id).copied());
    if let Some(s) = &mut scripts {
        s.geometria(pag.geometria());
        let _ = s.sujo();
    }
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
        if imgs.pixels >= MAX_PIXELS_PAGINA {
            break;
        }
        if let Some(i) = imagem_de(&url::resolver(&base, &u)) {
            imgs.pixels += i.px.len();
            imgs.por_url.insert(u, i);
        }
    }
    let titulo = pag.titulo.clone();
    let altura_total = pag.altura;
    PaginaWeb { pag, imgs, url: final_, titulo, altura: altura_total, erro, scripts, inicio }
}

/// JavaScript ligado? (configuracao "javascript", padrao sim)
fn javascript_ligado() -> bool {
    crate::config::Config::carregar().obter("javascript").is_none_or(|v| v != "nao" && v != "0" && v != "false")
}

/// Refaz estilo e layout se os scripts mexeram no DOM. true = mudou.
fn refazer_se_preciso(p: &mut PaginaWeb) -> bool {
    let Some(s) = &mut p.scripts else { return false };
    if !s.sujo() {
        return false;
    }
    buscar_imagens(&p.pag.doc.borrow(), &p.url, &mut p.imgs);
    let tamanhos: BTreeMap<Id, (i32, i32)> = p.imgs.por_no.iter().map(|(k, v)| (*k, (v.largura, v.altura))).collect();
    p.pag.refazer(fontes(), &|id| tamanhos.get(&id).copied());
    s.geometria(p.pag.geometria());
    p.titulo = p.pag.titulo.clone();
    p.altura = p.pag.altura;
    true
}

/// Timers e rede dos scripts. true = a pagina mudou (redesenhar).
pub fn tique(p: &mut PaginaWeb) -> bool {
    let Some(s) = &mut p.scripts else { return false };
    let agora = agora_ms() - p.inicio;
    match s.proximo_timer() {
        Some(t) if t <= agora => {}
        _ if !s.tem_pedidos() => return false,
        _ => {}
    }
    s.tique(agora, &mut responder);
    refazer_se_preciso(p)
}

/// Algum script pediu outra pagina (location.href = ..., form enviado).
pub fn navegacao(p: &mut PaginaWeb) -> Option<String> {
    p.scripts.as_mut()?.navegacao()
}

/// O que um clique fez.
pub enum Clique {
    Nada,
    /// O DOM mudou: redesenhar.
    Mudou,
    Navegar(String),
}

/// Clique no ponto (x, y) da area da pagina: eventos para os scripts e,
/// se ninguem impedir, segue o link.
pub fn clicar(p: &mut PaginaWeb, x: i32, y: i32, rolagem: i32) -> Clique {
    let link = link_em(p, x, y, rolagem);
    let no = p.pag.no_em(x, y, rolagem);
    let mut seguir = true;
    if let (Some(s), Some(no)) = (&mut p.scripts, no) {
        s.rolagem(rolagem);
        seguir = s.clique(no, x, y);
    }
    if let Some(u) = navegacao(p) {
        return Clique::Navegar(u);
    }
    if seguir {
        if let Some(l) = link {
            return Clique::Navegar(l);
        }
    }
    if refazer_se_preciso(p) { Clique::Mudou } else { Clique::Nada }
}

/// Tecla para o elemento com foco (campos de formulario, atalhos da pagina).
/// true = os scripts trataram (preventDefault) ou o DOM mudou.
pub fn tecla(p: &mut PaginaWeb, nome: &str, c: Option<char>) -> bool {
    let Some(s) = &mut p.scripts else { return false };
    let tratada = !s.tecla(nome, c);
    refazer_se_preciso(p) || tratada
}

/// Ha um campo de texto da pagina com foco (as teclas vao para ele)?
pub fn campo_focado(p: &mut PaginaWeb) -> bool {
    p.scripts.as_mut().is_some_and(|s| s.foco_editavel())
}

/// Mensagens do console dos scripts (para o log do sistema).
pub fn console(p: &mut PaginaWeb) -> String {
    p.scripts.as_mut().map(|s| s.console()).unwrap_or_default()
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
