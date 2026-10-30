//! YouTube no navegador do MXOS. A pagina oficial e um app de 13 MB de
//! JavaScript feito para motores com JIT; aqui o navegador monta as
//! paginas (inicio, busca, video) com o conteudo de verdade, pela mesma
//! API JSON que os apps do YouTube usam (youtubei/v1), e o video toca na
//! propria pagina pelo decodificador de hardware da TV box.
//!
//!   youtube.com/                 -> busca e videos em alta
//!   youtube.com/results?search_query=...
//!   youtube.com/watch?v=ID, youtu.be/ID, youtube.com/shorts/ID
//!
//! O video vem do formato 18 (MP4 360p com o som junto) do cliente
//! ANDROID, baixado aos pedacos por `&range=`.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::json::{self, Json};
use crate::midia::Fonte;

/// Qual pagina do YouTube um endereco pede.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rota {
    Inicio,
    Busca(String),
    Assistir(String),
}

/// Id do elemento onde o video e desenhado.
pub const ID_VIDEO: &str = "mxos-video";

const TEMPO_API_MS: u64 = 20_000;
const TEMPO_BLOCO_MS: u64 = 30_000;

/// %xx e + de um parametro de URL.
fn decodificar_url(s: &str) -> String {
    let b = s.as_bytes();
    let mut v = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => v.push(b' '),
            b'%' if i + 2 < b.len() => {
                let h = core::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                match h {
                    Some(x) => {
                        v.push(x);
                        i += 2;
                    }
                    None => v.push(b'%'),
                }
            }
            c => v.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&v).into_owned()
}

fn codificar_url(s: &str) -> String {
    let mut r = String::new();
    for &c in s.as_bytes() {
        if c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'~') {
            r.push(c as char);
        } else if c == b' ' {
            r.push('+');
        } else {
            r.push_str(&format!("%{c:02X}"));
        }
    }
    r
}

fn parametro<'a>(consulta: &'a str, nome: &str) -> Option<&'a str> {
    consulta.split('&').find_map(|p| p.strip_prefix(nome).and_then(|r| r.strip_prefix('=')))
}

fn id_valido(id: &str) -> bool {
    id.len() == 11 && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

/// O endereco e do YouTube? Qual pagina?
pub fn rota(url: &str) -> Option<Rota> {
    let resto = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let (host, caminho) = resto.split_once('/').unwrap_or((resto, ""));
    let host = host.to_ascii_lowercase();
    let (caminho, consulta) = caminho.split_once('?').unwrap_or((caminho, ""));
    let consulta = consulta.split('#').next().unwrap_or("");
    if host == "youtu.be" {
        let id = caminho.split('/').next().unwrap_or("");
        return id_valido(id).then(|| Rota::Assistir(String::from(id)));
    }
    if !matches!(host.as_str(), "youtube.com" | "www.youtube.com" | "m.youtube.com") {
        return None;
    }
    match caminho {
        "" => Some(Rota::Inicio),
        "watch" => parametro(consulta, "v").filter(|id| id_valido(id)).map(|id| Rota::Assistir(String::from(id))),
        "results" => {
            let q = parametro(consulta, "search_query").or_else(|| parametro(consulta, "q")).map(decodificar_url).unwrap_or_default();
            Some(if q.trim().is_empty() { Rota::Inicio } else { Rota::Busca(q) })
        }
        c => {
            let id = c.strip_prefix("shorts/").or_else(|| c.strip_prefix("embed/")).or_else(|| c.strip_prefix("live/"))?;
            id_valido(id).then(|| Rota::Assistir(String::from(id)))
        }
    }
}

// --------------------------------------------------------------- API

fn api(ponto: &str, corpo: &str, cliente: &str, numero: &str, versao: &str, agente: &str) -> Result<Json, &'static str> {
    let url = format!("https://www.youtube.com/youtubei/v1/{ponto}?prettyPrint=false");
    let r = crate::rede::http::post(
        &url,
        &[("Content-Type", "application/json"), ("X-YouTube-Client-Name", numero), ("X-YouTube-Client-Version", versao), ("User-Agent", agente)],
        corpo.as_bytes(),
        TEMPO_API_MS,
    )
    .map_err(|e| e.descricao())?;
    let _ = cliente;
    if r.status != 200 {
        return Err("a API do YouTube respondeu com erro");
    }
    json::ler(&r.corpo).ok_or("resposta da API do YouTube ilegivel")
}

const WEB_VERSAO: &str = "2.20250312.04.00";

fn api_web(ponto: &str, campos: &str) -> Result<Json, &'static str> {
    let corpo = format!("{{\"context\":{{\"client\":{{\"clientName\":\"WEB\",\"clientVersion\":\"{WEB_VERSAO}\",\"hl\":\"pt\",\"gl\":\"BR\"}}}},{campos}}}");
    api(ponto, &corpo, "WEB", "1", WEB_VERSAO, "Mozilla/5.0")
}

/// Todos os valores com essa chave, em qualquer profundidade (na ordem).
fn achar<'a>(j: &'a Json, chave: &str, saida: &mut Vec<&'a Json>) {
    match j {
        Json::Objeto(m) => {
            for (k, v) in m {
                if k == chave {
                    saida.push(v);
                }
                achar(v, chave, saida);
            }
        }
        Json::Lista(l) => {
            for v in l {
                achar(v, chave, saida);
            }
        }
        _ => {}
    }
}

fn todos<'a>(j: &'a Json, chave: &str) -> Vec<&'a Json> {
    let mut v = Vec::new();
    achar(j, chave, &mut v);
    v
}

/// simpleText, runs[].text ou content.
fn texto(j: Option<&Json>) -> String {
    let Some(j) = j else { return String::new() };
    if let Some(t) = j.campo("simpleText").and_then(Json::texto) {
        return String::from(t);
    }
    if let Some(t) = j.campo("content").and_then(Json::texto) {
        return String::from(t);
    }
    if let Some(t) = j.texto() {
        return String::from(t);
    }
    j.campo("runs").map(|r| r.lista().iter().filter_map(|x| x.campo("text").and_then(Json::texto)).collect()).unwrap_or_default()
}

/// Um video numa lista (busca, inicio, relacionados).
struct Item {
    id: String,
    titulo: String,
    canal: String,
    detalhes: String,
    duracao: String,
}

fn itens_video_renderer(j: &Json) -> Vec<Item> {
    let mut v = Vec::new();
    for r in todos(j, "videoRenderer") {
        let Some(id) = r.campo("videoId").and_then(Json::texto) else { continue };
        let visto = texto(r.campo("viewCountText").or_else(|| r.campo("shortViewCountText")));
        let quando = texto(r.campo("publishedTimeText"));
        v.push(Item {
            id: String::from(id),
            titulo: texto(r.campo("title")),
            canal: texto(r.campo("ownerText").or_else(|| r.campo("longBylineText"))),
            detalhes: if quando.is_empty() { visto } else { format!("{visto} • {quando}") },
            duracao: texto(r.campo("lengthText")),
        });
    }
    v
}

/// Os relacionados do "next" vem como lockupViewModel.
fn itens_lockup(j: &Json) -> Vec<Item> {
    let mut v = Vec::new();
    for l in todos(j, "lockupViewModel") {
        let Some(id) = l.campo("contentId").and_then(Json::texto) else { continue };
        if !id_valido(id) {
            continue;
        }
        let meta = l.campo("metadata").and_then(|m| m.campo("lockupMetadataViewModel"));
        let titulo = texto(meta.and_then(|m| m.campo("title")));
        // linhas de metadados: canal, depois visualizacoes e data
        let mut linhas: Vec<String> = Vec::new();
        if let Some(m) = meta {
            for linha in todos(m, "metadataRows").iter().flat_map(|r| r.lista()) {
                let partes: Vec<String> = linha.campo("metadataParts").map(|p| p.lista().iter().map(|x| texto(x.campo("text"))).collect()).unwrap_or_default();
                let s = partes.join(" • ");
                if !s.is_empty() {
                    linhas.push(s);
                }
            }
        }
        let duracao = todos(l, "thumbnailBadgeViewModel").first().map(|b| texto(b.campo("text"))).unwrap_or_default();
        v.push(Item { id: String::from(id), titulo, canal: linhas.first().cloned().unwrap_or_default(), detalhes: linhas.get(1).cloned().unwrap_or_default(), duracao });
    }
    v
}

// -------------------------------------------------------------- HTML

fn escapar(s: &str) -> String {
    let mut r = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => r.push_str("&lt;"),
            '>' => r.push_str("&gt;"),
            '&' => r.push_str("&amp;"),
            '"' => r.push_str("&quot;"),
            c => r.push(c),
        }
    }
    r
}

const ESTILO: &str = "body{margin:0;font-family:sans-serif;background:#fff;color:#0f0f0f}\
a{color:inherit;text-decoration:none}\
.topo{display:flex;align-items:center;height:40px;padding:8px 16px;border-bottom:1px solid #e5e5e5;background:#fff}\
.logo{font-weight:bold;font-size:19px;letter-spacing:-1px;margin-right:24px}\
.logo b{background:#f00;color:#fff;border-radius:7px;padding:1px 7px;margin-right:3px;font-size:13px}\
form{display:flex;flex:1;height:36px;margin:0}\
.q{flex:1;height:36px;box-sizing:border-box;padding:0 14px;border:1px solid #ccc;border-radius:18px 0 0 18px;font-size:14px}\
.ir{height:36px;box-sizing:border-box;padding:0 18px;border:1px solid #ccc;border-left:0;border-radius:0 18px 18px 0;background:#f2f2f2;font-size:14px}\
.titulo-sec{font-size:18px;font-weight:bold;margin:16px 16px 4px}\
.grade{display:flex;flex-wrap:wrap;padding:4px 8px}\
.card{width:224px;margin:8px}\
.mini{position:relative;width:224px;height:126px;border-radius:10px;overflow:hidden;background:#e5e5e5}\
.mini img{width:224px;height:126px;display:block}\
.dur{position:absolute;right:6px;bottom:6px;background:#000;color:#fff;font-size:11px;line-height:14px;font-weight:bold;padding:1px 4px;border-radius:4px;white-space:nowrap}\
.t{font-size:14px;font-weight:bold;margin-top:8px;line-height:18px;max-height:36px;overflow:hidden}\
.c{font-size:12px;color:#606060;margin-top:4px}\
.d{font-size:12px;color:#606060}\
.assistir{padding:8px 16px 16px}\
#mxos-video{width:100%;height:400px;background:#000}\
.vt{font-size:19px;font-weight:bold;margin:12px 0 6px}\
.canal{font-size:15px;font-weight:bold}\
.desc{background:#f2f2f2;border-radius:12px;padding:12px;margin-top:12px;font-size:13px;white-space:pre-wrap}\
.lista .card{display:flex;width:100%;margin:8px 0}\
.lista .mini{width:168px;height:94px;flex:none;margin-right:10px}\
.lista .mini img{width:168px;height:94px}\
.lista .t{margin-top:0}\
.aviso{padding:24px;color:#606060}";

fn cabeca(titulo: &str, busca: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>{}</title><style>{ESTILO}</style></head><body>\
<div class=\"topo\"><a class=\"logo\" href=\"https://www.youtube.com/\"><b>&#9654;</b>YouTube</a>\
<form action=\"https://www.youtube.com/results\" method=\"get\"><input class=\"q\" type=\"text\" name=\"search_query\" value=\"{}\" placeholder=\"Pesquisar\"><button class=\"ir\" type=\"submit\">Buscar</button></form></div>",
        escapar(titulo),
        escapar(busca)
    )
}

fn cartao(i: &Item) -> String {
    let dur = String::new();
    let detalhes = match (i.duracao.is_empty(), i.detalhes.is_empty()) {
        (true, _) => i.detalhes.clone(),
        (false, true) => i.duracao.clone(),
        (false, false) => format!("{} • {}", i.duracao, i.detalhes),
    };
    format!(
        "<a class=\"card\" href=\"https://www.youtube.com/watch?v={id}\"><div class=\"mini\"><img src=\"https://i.ytimg.com/vi/{id}/mqdefault.jpg\" alt=\"\">{dur}</div>\
<div><div class=\"t\">{}</div><div class=\"c\">{}</div><div class=\"d\">{}</div></div></a>",
        escapar(&i.titulo),
        escapar(&i.canal),
        escapar(&detalhes),
        id = i.id
    )
}

fn grade(titulo: &str, itens: &[Item]) -> String {
    let mut s = format!("<div class=\"titulo-sec\">{}</div><div class=\"grade\">", escapar(titulo));
    for i in itens {
        s += &cartao(i);
    }
    s + "</div>"
}

fn busca(q: &str) -> Result<Vec<Item>, &'static str> {
    let j = api_web("search", &format!("\"query\":{}", json::texto(q)))?;
    Ok(itens_video_renderer(&j))
}

/// A pagina (HTML) de um endereco do YouTube. Erros viram pagina tambem.
pub fn pagina(r: &Rota) -> String {
    match r {
        Rota::Inicio => {
            let mut s = cabeca("YouTube", "");
            match busca("em alta hoje brasil") {
                Ok(itens) if !itens.is_empty() => s += &grade("Em alta", &itens),
                Ok(_) => s += "<div class=\"aviso\">Nada para mostrar agora. Use a busca.</div>",
                Err(e) => s += &format!("<div class=\"aviso\">Não deu para falar com o YouTube: {}</div>", escapar(e)),
            }
            s + "</body></html>"
        }
        Rota::Busca(q) => {
            let mut s = cabeca(&format!("{q} - YouTube"), q);
            match busca(q) {
                Ok(itens) if !itens.is_empty() => s += &grade(&format!("Resultados para \"{q}\""), &itens),
                Ok(_) => s += "<div class=\"aviso\">Nenhum vídeo encontrado.</div>",
                Err(e) => s += &format!("<div class=\"aviso\">Não deu para falar com o YouTube: {}</div>", escapar(e)),
            }
            s + "</body></html>"
        }
        Rota::Assistir(id) => {
            let (mut titulo, mut canal, mut info, mut desc) = (String::new(), String::new(), String::new(), String::new());
            let mut relacionados = Vec::new();
            if let Ok(j) = api_web("next", &format!("\"videoId\":\"{id}\"")) {
                if let Some(p) = todos(&j, "videoPrimaryInfoRenderer").first() {
                    titulo = texto(p.campo("title"));
                    let vistas = texto(p.campo("viewCount").and_then(|v| v.campo("videoViewCountRenderer")).and_then(|v| v.campo("viewCount")));
                    let data = texto(p.campo("dateText"));
                    info = format!("{vistas} • {data}");
                }
                if let Some(s) = todos(&j, "videoSecondaryInfoRenderer").first() {
                    canal = texto(s.campo("owner").and_then(|o| o.campo("videoOwnerRenderer")).and_then(|o| o.campo("title")));
                    desc = texto(s.campo("attributedDescription"));
                    if desc.is_empty() {
                        desc = texto(s.campo("description"));
                    }
                }
                relacionados = itens_lockup(&j);
            }
            if titulo.is_empty() {
                titulo = String::from("YouTube");
            }
            let desc: String = desc.chars().take(1500).collect();
            let mut s = cabeca(&format!("{titulo} - YouTube"), "");
            s += &format!(
                "<div class=\"assistir\"><div id=\"{ID_VIDEO}\"></div><div class=\"d\" style=\"margin-top:6px\">Espaço ou clique no vídeo: pausa • F11: tela cheia (Esc sai)</div><div class=\"vt\">{}</div><div class=\"canal\">{}</div><div class=\"d\">{}</div><div class=\"desc\">{}</div>\
<div class=\"titulo-sec\" style=\"margin-left:0\">A seguir</div><div class=\"lista\">",
                escapar(&titulo),
                escapar(&canal),
                escapar(&info),
                escapar(&desc)
            );
            for i in relacionados.iter().take(20) {
                s += &cartao(i);
            }
            s + "</div></div></body></html>"
        }
    }
}

// ------------------------------------------------------------- video

const ANDROID_VERSAO: &str = "20.10.38";

/// Como baixar o video.
pub enum Fluxo {
    /// MP4 inteiro (formato 18: 360p com som junto), lido aos blocos.
    Progressivo { url: String, tamanho: usize },
    /// MP4 fragmentado so de video (DASH): ate o byte `fim_indice` vem o
    /// init e o indice dos segmentos.
    Dash { url: String, fim_indice: u64, altura: u32 },
}

/// Formatos H.264 em ordem de preferencia para a qualidade pedida
/// (config youtube_qualidade: 360, 480, 720 ou 1080; padrao 720).
fn preferidos() -> &'static [u32] {
    let q = crate::config::Config::carregar().obter("youtube_qualidade").and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(720);
    match q {
        0..=360 => &[18],
        361..=480 => &[135, 18],
        481..=720 => &[136, 135, 18],
        _ => &[137, 136, 135, 18],
    }
}

fn numero(j: Option<&Json>) -> Option<u64> {
    match j? {
        Json::Num(n) => Some(*n as u64),
        Json::Texto(t) => t.parse().ok(),
        _ => None,
    }
}

/// O melhor jeito de baixar o video (ver `preferidos`).
pub fn fluxo(id: &str) -> Result<Fluxo, &'static str> {
    let corpo = format!(
        "{{\"context\":{{\"client\":{{\"clientName\":\"ANDROID\",\"clientVersion\":\"{ANDROID_VERSAO}\",\"androidSdkVersion\":30,\"osName\":\"Android\",\"osVersion\":\"11\",\"hl\":\"pt\",\"gl\":\"BR\"}}}},\"videoId\":\"{id}\",\"contentCheckOk\":true,\"racyCheckOk\":true}}"
    );
    let agente = format!("com.google.android.youtube/{ANDROID_VERSAO} (Linux; U; Android 11) gzip");
    let j = api("player", &corpo, "ANDROID", "3", ANDROID_VERSAO, &agente)?;
    let status = j.campo("playabilityStatus").and_then(|s| s.campo("status")).and_then(Json::texto).unwrap_or("");
    if status != "OK" {
        return Err(match status {
            "LOGIN_REQUIRED" => "o YouTube pediu login para este vídeo",
            "UNPLAYABLE" => "o YouTube não deixa tocar este vídeo fora do site",
            "LIVE_STREAM_OFFLINE" => "transmissão ao vivo ainda não começou",
            _ => "o YouTube recusou este vídeo",
        });
    }
    let dados = j.campo("streamingData");
    let formatos = dados.and_then(|s| s.campo("formats")).map(Json::lista).unwrap_or(&[]);
    let adaptativos = dados.and_then(|s| s.campo("adaptiveFormats")).map(Json::lista).unwrap_or(&[]);
    let itag = |f: &&Json| numero(f.campo("itag")).unwrap_or(0) as u32;
    for &quero in preferidos() {
        if quero == 18 {
            if let Some(f) = formatos.iter().find(|f| itag(f) == 18) {
                let url = f.campo("url").and_then(Json::texto).ok_or("o formato veio cifrado")?;
                let tamanho = numero(f.campo("contentLength")).ok_or("o YouTube não disse o tamanho do vídeo")?;
                return Ok(Fluxo::Progressivo { url: String::from(url), tamanho: tamanho as usize });
            }
            continue;
        }
        let Some(f) = adaptativos.iter().find(|f| itag(f) == quero) else { continue };
        let (Some(url), Some(fim)) = (f.campo("url").and_then(Json::texto), numero(f.campo("indexRange").and_then(|r| r.campo("end")))) else { continue };
        let altura = numero(f.campo("height")).unwrap_or(0) as u32;
        return Ok(Fluxo::Dash { url: String::from(url), fim_indice: fim, altura });
    }
    Err("o vídeo não tem formato H.264 que a box toque")
}

/// O video lido pela rede aos blocos (`&range=`), guardando os ultimos.
pub struct FonteRede {
    url: String,
    total: usize,
    blocos: BTreeMap<usize, Vec<u8>>,
    /// Ordem de uso (o mais antigo sai primeiro).
    uso: Vec<usize>,
    pub erro: Option<&'static str>,
}

const BLOCO: usize = 512 * 1024;
const MAX_BLOCOS: usize = 8;

impl FonteRede {
    pub fn nova(url: String, total: usize) -> FonteRede {
        FonteRede { url, total, blocos: BTreeMap::new(), uso: Vec::new(), erro: None }
    }

    fn bloco(&mut self, n: usize) -> Option<&Vec<u8>> {
        if !self.blocos.contains_key(&n) {
            let inicio = n * BLOCO;
            let fim = (inicio + BLOCO).min(self.total) - 1;
            let u = format!("{}&range={inicio}-{fim}", self.url);
            let r = match crate::rede::http_get(&u, TEMPO_BLOCO_MS) {
                Ok(r) if (200..300).contains(&r.status) && !r.corpo.is_empty() => r,
                Ok(_) => {
                    self.erro = Some("o servidor de vídeo recusou o pedaço");
                    return None;
                }
                Err(e) => {
                    self.erro = Some(e.descricao());
                    return None;
                }
            };
            if self.blocos.len() >= MAX_BLOCOS {
                let velho = self.uso.remove(0);
                self.blocos.remove(&velho);
            }
            self.blocos.insert(n, r.corpo);
        }
        self.uso.retain(|&b| b != n);
        self.uso.push(n);
        self.blocos.get(&n)
    }
}

impl Fonte for FonteRede {
    fn tamanho(&self) -> usize {
        self.total
    }

    fn ler(&mut self, pos: usize, destino: &mut [u8]) -> usize {
        let mut feito = 0;
        while feito < destino.len() && pos + feito < self.total {
            let p = pos + feito;
            let (n, desvio) = (p / BLOCO, p % BLOCO);
            let Some(b) = self.bloco(n) else { break };
            if desvio >= b.len() {
                break;
            }
            let k = (b.len() - desvio).min(destino.len() - feito);
            destino[feito..feito + k].copy_from_slice(&b[desvio..desvio + k]);
            feito += k;
        }
        feito
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn rotas() {
        assert_eq!(rota("https://www.youtube.com/"), Some(Rota::Inicio));
        assert_eq!(rota("https://www.youtube.com/watch?v=aqz-KE-bpKQ&t=3"), Some(Rota::Assistir("aqz-KE-bpKQ".into())));
        assert_eq!(rota("https://youtu.be/aqz-KE-bpKQ"), Some(Rota::Assistir("aqz-KE-bpKQ".into())));
        assert_eq!(rota("https://www.youtube.com/results?search_query=bom+dia%21"), Some(Rota::Busca("bom dia!".into())));
        assert_eq!(rota("https://example.com/"), None);
    }
}
