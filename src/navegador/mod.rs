//! Navegador do MXOS: o motor fica em crate::web (HTML5, CSS, layout,
//! JavaScript), ligado a GUI por web.rs. Aqui ficam os enderecos, as
//! paginas de erro e o modo texto para a serial (comando "navegar").
//!
//! Enderecos aceitos:
//!   disco:/paginas/index.html   arquivo do disco FAT32 (armazenamento)
//!   http[s]://host[:porta]/x    pagina da rede (https pelo TLS 1.3 proprio)
//!

// Nem todo item publico e usado fora daqui (os testes no PC usam).
#![allow(dead_code)]

pub mod web;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::fmt::Write;
use core::hint::spin_loop;
use core::sync::atomic::{AtomicBool, Ordering};


/// Tempo maximo para uma pagina chegar pela rede.
const TEMPO_LIMITE_MS: u64 = 10_000;
/// Largura do texto no comando "navegar".
const COLUNAS_SERIAL: usize = 78;
/// Bytes do comeco de um arquivo olhados para saber se e texto.
const AMOSTRA_BINARIO: usize = 1024;

const SO_TEXTO: &str = "o conteúdo não é texto (imagem, programa…)";

// ---------------------------------------------------------------------
// Carregar

/// (HTML, endereco final) de `url`.
fn buscar(url: &str) -> Result<(String, String), &'static str> {
    let url = url.trim();
    if let Some(caminho) = sem_prefixo(url, "disco:") {
        return carregar_do_disco(caminho).map(|html| (html, String::from(url)));
    }
    if sem_prefixo(url, "http://").is_some() || sem_prefixo(url, "https://").is_some() {
        return carregar_da_rede(url);
    }
    Err("endereço inválido (use disco:/caminho, http:// ou https://)")
}

fn carregar_do_disco(caminho: &str) -> Result<String, &'static str> {
    // "#parte" e "?consulta" nao fazem sentido para arquivo
    let caminho = caminho.split(['#', '?']).next().unwrap_or("");
    let caminho = decodificar_porcento(caminho);
    let caminho = if caminho.is_empty() { "/" } else { caminho.as_str() };
    let mut absoluto = String::new();
    if !caminho.starts_with('/') {
        absoluto.push('/');
    }
    absoluto.push_str(caminho);
    if absoluto.ends_with('/') {
        return Ok(pagina_da_pasta(&absoluto));
    }
    let dados = crate::armazenamento::ler(&absoluto).map_err(|e| e.descricao())?;
    let minusculo = absoluto.to_ascii_lowercase();
    let e_html = minusculo.ends_with(".html") || minusculo.ends_with(".htm") || parece_html(&dados);
    if !e_html && parece_binario(&dados) {
        return Err(SO_TEXTO);
    }
    Ok(como_html(&dados, e_html))
}

/// Texto nao tem byte 0; imagem e programa quase sempre tem logo no comeco.
fn parece_binario(dados: &[u8]) -> bool {
    dados[..dados.len().min(AMOSTRA_BINARIO)].contains(&0)
}

/// "disco:/pasta/": uma pagina com a lista de arquivos, cada um um link.
fn pagina_da_pasta(caminho: &str) -> String {
    let itens = match crate::armazenamento::listar(caminho) {
        Ok(itens) => itens,
        Err(e) => return format!("<h1>Erro</h1><p>{}: {}</p>", escapar(caminho), e.descricao()),
    };
    let mut pagina = format!("<title>{0}</title><h1>Pasta {0}</h1><ul>", escapar(caminho));
    for item in itens {
        let barra = if item.pasta { "/" } else { "" };
        let nome = escapar(&item.nome);
        let _ = write!(pagina, "<li><a href=\"{nome}{barra}\">{nome}{barra}</a>");
        if !item.pasta {
            let _ = write!(pagina, " ({} bytes)", item.tamanho);
        }
    }
    pagina.push_str("</ul>");
    pagina
}

/// (HTML, endereco final depois dos redirecionamentos).
fn carregar_da_rede(url: &str) -> Result<(String, String), &'static str> {
    let resposta = crate::rede::http_get(&codificar_url(url), TEMPO_LIMITE_MS).map_err(|e| e.descricao())?;
    let url_final = if resposta.url_final.is_empty() { String::from(url) } else { resposta.url_final.clone() };
    let tipo = resposta
        .cabecalhos
        .iter()
        .find(|(nome, _)| nome.eq_ignore_ascii_case("content-type"))
        .map(|(_, valor)| valor.to_ascii_lowercase())
        .unwrap_or_default();
    let texto = tipo.starts_with("text/") || tipo.contains("xml") || tipo.contains("json") || tipo.contains("javascript");
    if !tipo.is_empty() && !texto {
        return Err(SO_TEXTO);
    }
    if resposta.corpo.is_empty() && resposta.status >= 400 {
        let html = format!("<title>Erro {0}</title><h1>Erro {0}</h1><p>O servidor não mandou página.</p>", resposta.status);
        return Ok((html, url_final));
    }
    let e_html = tipo.contains("html") || (tipo.is_empty() && parece_html(&resposta.corpo));
    if !e_html && tipo.is_empty() && parece_binario(&resposta.corpo) {
        return Err(SO_TEXTO);
    }
    Ok((como_html(&resposta.corpo, e_html), url_final))
}

/// Endereco para mandar ao servidor: o que nao e ASCII (e o espaco)
/// vira %XX dos bytes UTF-8 ("pagina com acento" -> "p%C3%A1gina").
fn codificar_url(url: &str) -> String {
    let mut saida = String::with_capacity(url.len());
    for &b in url.as_bytes() {
        if b > b' ' && b < 0x7F {
            saida.push(b as char);
        } else {
            let _ = write!(saida, "%{b:02X}");
        }
    }
    saida
}

/// "%C3%A1" -> "á" (link do disco escrito com %XX). Sequencia que nao
/// forma UTF-8 valido fica como estava.
fn decodificar_porcento(texto: &str) -> String {
    let b = texto.as_bytes();
    let mut bytes = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hexa = b.get(i + 1..i + 3).and_then(|h| core::str::from_utf8(h).ok());
        match hexa.and_then(|h| u8::from_str_radix(h, 16).ok()) {
            Some(valor) if b[i] == b'%' => {
                bytes.push(valor);
                i += 3;
            }
            _ => {
                bytes.push(b[i]);
                i += 1;
            }
        }
    }
    String::from_utf8(bytes).unwrap_or_else(|_| texto.into())
}

/// O comeco (sem espacos) e um '<'?
fn parece_html(dados: &[u8]) -> bool {
    dados.iter().find(|c| !c.is_ascii_whitespace()) == Some(&b'<')
}

/// Bytes (UTF-8 ou Windows-1252) -> HTML em UTF-8. Texto puro vira <pre>
/// com <, > e & escapados.
fn como_html(dados: &[u8], e_html: bool) -> String {
    let texto = crate::web::html::codificacao::texto(dados, None);
    if e_html {
        return texto;
    }
    let mut pagina = String::with_capacity(texto.len() + 16);
    pagina.push_str("<pre>");
    pagina.push_str(&escapar(&texto));
    pagina.push_str("</pre>");
    pagina
}

fn escapar(texto: &str) -> String {
    let mut saida = String::with_capacity(texto.len());
    for c in texto.chars() {
        match c {
            '&' => saida.push_str("&amp;"),
            '<' => saida.push_str("&lt;"),
            '>' => saida.push_str("&gt;"),
            '"' => saida.push_str("&quot;"),
            c => saida.push(c),
        }
    }
    saida
}

// ---------------------------------------------------------------------
// Paginas feitas aqui (erros)

/// Pagina que explica por que `url` nao abriu.
pub fn pagina_de_erro(url: &str, motivo: &str) -> String {
    let mut pagina = format!(
        "<title>Erro</title><h1>Não foi possível abrir a página</h1>\
         <p>Endereço: <code>{}</code></p><p>Motivo: <b>{}</b></p><h3>O que tentar</h3><ul>",
        escapar(url),
        escapar(motivo)
    );
    if sem_prefixo(url, "disco:").is_some() {
        let pasta = escapar(&resolver_url(url, "./"));
        let _ = write!(pagina, "<li>Veja o que há na pasta: <a href=\"{pasta}\">{pasta}</a></li>");
    } else {
        pagina.push_str("<li>Confira o endereço e se a rede está ligada (comando <code>ip</code> no terminal).</li>");
        pagina.push_str("<li>No QEMU, o próprio PC responde em <code>http://10.0.2.2/</code>.</li>");
    }
    pagina.push_str("<li>Páginas do disco: <a href=\"disco:/paginas/index.html\">página inicial</a></li></ul>");
    pagina
}

/// `texto` sem o `prefixo` (ignorando maiusculas), se comecar com ele.
fn sem_prefixo<'a>(texto: &'a str, prefixo: &str) -> Option<&'a str> {
    let n = prefixo.len();
    let igual = texto.len() >= n && texto.as_bytes()[..n].eq_ignore_ascii_case(prefixo.as_bytes());
    // prefixo e ASCII, entao n e fronteira de caractere
    if igual { texto.get(n..) } else { None }
}

// ---------------------------------------------------------------------
// Enderecos

/// Separa "http://host:porta/caminho" em ("http://host:porta", "/caminho")
/// e "disco:/caminho" em ("disco:", "/caminho").
fn origem_e_caminho(url: &str) -> (&str, &str) {
    if let Some(i) = url.find("://") {
        let depois = i + 3;
        let fim_host = url[depois..].find(['/', '?', '#']).map_or(url.len(), |p| depois + p);
        return (&url[..fim_host], &url[fim_host..]);
    }
    match url.find(':') {
        Some(i) if url[..i].bytes().all(|c| c.is_ascii_alphabetic()) && i > 0 => (&url[..=i], &url[i + 1..]),
        _ => ("", url),
    }
}

/// O href tem esquema proprio ("http:", "mailto:", "disco:")?
fn tem_esquema(href: &str) -> bool {
    match href.find(':') {
        Some(i) if i > 0 => {
            let esquema = &href[..i];
            esquema.as_bytes()[0].is_ascii_alphabetic()
                && esquema.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'+' || c == b'-' || c == b'.')
        }
        _ => false,
    }
}

/// Junta um link a pagina onde ele esta. "#parte" e ignorado (fica a
/// propria pagina); "../" e "./" sao resolvidos.
pub fn resolver_url(base: &str, href: &str) -> String {
    let href = href.trim();
    let href = href.split('#').next().unwrap_or("");
    let base = base.split('#').next().unwrap_or("");
    if href.is_empty() {
        return base.into();
    }
    if tem_esquema(href) {
        let (origem, caminho) = origem_e_caminho(href);
        if origem.eq_ignore_ascii_case("disco:") && !caminho.starts_with('/') {
            return format!("disco:/{caminho}");
        }
        return href.into();
    }
    let (origem, caminho_base) = origem_e_caminho(base);
    if let Some(resto) = href.strip_prefix("//") {
        // "//host/x": mesmo esquema da base
        let esquema = origem.find("//").map_or("http:", |i| &origem[..i]);
        return format!("{esquema}//{resto}");
    }
    let caminho_base = caminho_base.split('?').next().unwrap_or("");
    let caminho = if href.starts_with('/') {
        String::from(href)
    } else if href.starts_with('?') {
        format!("{caminho_base}{href}")
    } else {
        // relativo: a pasta da base + href
        let pasta = caminho_base.rfind('/').map_or("/", |i| &caminho_base[..=i]);
        let pasta = if pasta.starts_with('/') { String::from(pasta) } else { format!("/{pasta}") };
        format!("{pasta}{href}")
    };
    format!("{origem}{}", normalizar_caminho(&caminho))
}

/// Resolve "." e ".." de um caminho absoluto (a consulta "?..." fica igual).
fn normalizar_caminho(caminho: &str) -> String {
    let (caminho, consulta) = match caminho.find('?') {
        Some(i) => (&caminho[..i], &caminho[i..]),
        None => (caminho, ""),
    };
    let mut partes: Vec<&str> = Vec::new();
    let pecas: Vec<&str> = caminho.split('/').collect();
    for (i, peca) in pecas.iter().enumerate() {
        let ultima = i + 1 == pecas.len();
        match *peca {
            "" | "." => {
                if ultima && !partes.is_empty() {
                    partes.push(""); // termina com '/'
                }
            }
            ".." => {
                partes.pop();
                if ultima {
                    partes.push("");
                }
            }
            p => partes.push(p),
        }
    }
    let mut saida = String::with_capacity(caminho.len() + 1);
    for p in &partes {
        saida.push('/');
        saida.push_str(p);
    }
    if saida.is_empty() {
        saida.push('/');
    }
    saida.push_str(consulta);
    saida
}

/// O que a pessoa digitou -> endereco completo:
/// "/paginas/x.html" -> "disco:/paginas/x.html", "exemplo.com" -> "https://exemplo.com/",
/// "10.0.2.2:8000" e "localhost" -> http:// (servidores locais nao tem certificado).
pub fn normalizar_url(entrada: &str) -> String {
    let entrada = entrada.trim();
    if entrada.starts_with('/') {
        return format!("disco:{entrada}");
    }
    if tem_esquema(entrada) && (entrada.contains("://") || sem_prefixo(entrada, "disco:").is_some()) {
        return resolver_url("", entrada);
    }
    let host = entrada.split(['/', ':']).next().unwrap_or("");
    let local = host.bytes().all(|c| c.is_ascii_digit() || c == b'.') || host.eq_ignore_ascii_case("localhost") || entrada.contains(':');
    let esquema = if local { "http" } else { "https" };
    if entrada.contains('/') { format!("{esquema}://{entrada}") } else { format!("{esquema}://{entrada}/") }
}

// ---------------------------------------------------------------------
// Comando "navegar" (modo texto, pela serial)

/// Links da ultima pagina mostrada, para "navegar N".
struct Historico {
    trava: AtomicBool,
    links: UnsafeCell<Vec<String>>,
}

// So o shell (laco principal) usa; a trava e so por garantia.
unsafe impl Sync for Historico {}

static HISTORICO: Historico = Historico { trava: AtomicBool::new(false), links: UnsafeCell::new(Vec::new()) };

impl Historico {
    fn com<R>(&self, acao: impl FnOnce(&mut Vec<String>) -> R) -> R {
        while self.trava.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            spin_loop();
        }
        let resultado = acao(unsafe { &mut *self.links.get() });
        self.trava.store(false, Ordering::Release);
        resultado
    }
}

/// Comandos do navegador no shell. Devolve false se o nome nao e dele.
pub fn comando(nome: &str, args: &str, saida: &mut dyn Write) -> bool {
    if nome != "navegar" {
        return false;
    }
    let args = args.trim();
    if args.is_empty() {
        let _ = writeln!(saida, "uso: navegar <endereço>   ex.: navegar disco:/paginas/index.html");
        let _ = writeln!(saida, "     navegar http://10.0.2.2:8000/   |   navegar N (segue o link [N] da última página)");
        return true;
    }
    let url = match args.parse::<usize>() {
        Ok(n) => match HISTORICO.com(|links| links.get(n.wrapping_sub(1)).cloned()) {
            Some(url) => url,
            None => {
                let _ = writeln!(saida, "navegar: não há link [{n}] na última página");
                return true;
            }
        },
        Err(_) => normalizar_url(args),
    };
    navegar(&url, saida);
    true
}

/// Carrega, mostra o texto e guarda os links para "navegar N". Os links
/// sao resolvidos contra o endereco final (depois dos redirecionamentos).
fn navegar(url: &str, saida: &mut dyn Write) {
    let _ = writeln!(saida, "Carregando {url}…");
    let (pagina, url_final) = match buscar(url) {
        Ok(p) => p,
        Err(e) => {
            let _ = writeln!(saida, "navegar: {e}");
            return;
        }
    };
    let doc = crate::web::html::analisar(&pagina);
    let (corpo, links) = texto_da_pagina(&doc, COLUNAS_SERIAL);
    let titulo_doc = doc.titulo();
    let titulo = if titulo_doc.trim().is_empty() { url } else { titulo_doc.trim() };
    let _ = writeln!(saida, "{}", "=".repeat(COLUNAS_SERIAL));
    let _ = writeln!(saida, " {titulo}");
    let _ = writeln!(saida, "{}", "=".repeat(COLUNAS_SERIAL));
    for linha in corpo.lines() {
        let _ = writeln!(saida, "{linha}");
    }
    let absolutos: Vec<String> = links.iter().map(|href| resolver_url(&url_final, href)).collect();
    if !absolutos.is_empty() {
        let _ = writeln!(saida, "{}", "-".repeat(COLUNAS_SERIAL));
        let _ = writeln!(saida, "Links:");
        for (i, link) in absolutos.iter().enumerate() {
            let _ = writeln!(saida, "  [{}] {link}", i + 1);
        }
        let _ = writeln!(saida, "(navegar N segue o link [N])");
    }
    HISTORICO.com(|guardados| *guardados = absolutos);
}

/// O texto da pagina quebrado em `colunas`, com [N] depois de cada link, e
/// os hrefs na ordem.
fn texto_da_pagina(doc: &crate::web::dom::Documento, colunas: usize) -> (String, Vec<String>) {
    use crate::web::atomo::a;
    use crate::web::dom::{NENHUM, RAIZ, Tipo};
    const BLOCOS: &[&str] = &[
        "p", "div", "br", "li", "tr", "h1", "h2", "h3", "h4", "h5", "h6", "pre", "blockquote", "section", "article",
        "header", "footer", "nav", "ul", "ol", "table", "dt", "dd", "hr", "main", "aside", "figure",
    ];
    let mut paragrafos: Vec<String> = alloc::vec![String::new()];
    let mut links = Vec::new();
    // percorre na ordem, sem entrar em script/style/head
    let mut pilha = alloc::vec![RAIZ];
    while let Some(id) = pilha.pop() {
        if id == NENHUM {
            continue;
        }
        let n = doc.no(id);
        match &n.tipo {
            Tipo::Texto(t) => {
                let atual = paragrafos.last_mut().unwrap_or_else(|| unreachable!());
                for p in t.split_whitespace() {
                    if !atual.is_empty() && !atual.ends_with(' ') {
                        atual.push(' ');
                    }
                    atual.push_str(p);
                }
                if t.ends_with(char::is_whitespace) && !atual.is_empty() {
                    atual.push(' ');
                }
            }
            Tipo::Elemento(e) => {
                let nome = e.nome.texto();
                if matches!(nome, "script" | "style" | "head" | "noscript" | "template" | "svg") {
                    continue;
                }
                if BLOCOS.contains(&nome) && !paragrafos.last().is_some_and(|p| p.trim().is_empty()) {
                    paragrafos.push(String::new());
                }
                if e.eh(a::A) {
                    if let Some(h) = e.atributo(a::HREF).filter(|h| !h.starts_with('#') && !h.starts_with("javascript:")) {
                        links.push(String::from(h));
                        let atual = paragrafos.last_mut().unwrap_or_else(|| unreachable!());
                        if !atual.is_empty() && !atual.ends_with(' ') {
                            atual.push(' ');
                        }
                        atual.push_str(&format!("[{}]", links.len()));
                    }
                }
                let filhos: Vec<_> = doc.filhos(id).collect();
                for f in filhos.into_iter().rev() {
                    pilha.push(f);
                }
            }
            _ => {
                let filhos: Vec<_> = doc.filhos(id).collect();
                for f in filhos.into_iter().rev() {
                    pilha.push(f);
                }
            }
        }
    }
    let mut saida = String::new();
    for p in paragrafos.iter().map(|p| p.trim()).filter(|p| !p.is_empty()) {
        let mut linha = String::new();
        for palavra in p.split(' ') {
            if !linha.is_empty() && linha.chars().count() + 1 + palavra.chars().count() > colunas {
                saida.push_str(&linha);
                saida.push('\n');
                linha.clear();
            }
            if !linha.is_empty() {
                linha.push(' ');
            }
            linha.push_str(palavra);
        }
        saida.push_str(&linha);
        saida.push_str("\n\n");
    }
    (saida, links)
}
