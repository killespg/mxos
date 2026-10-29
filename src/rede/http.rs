//! HTTP/1.1, so GET e so http:// (sem TLS).
//!
//! Pedido: "GET caminho HTTP/1.1" + Host + User-Agent + Connection: close.
//! Resposta: linha de status, cabecalhos ate a linha vazia e o corpo, que
//! termina de um destes jeitos:
//!   - Content-Length: N bytes
//!   - Transfer-Encoding: chunked: pedacos "tamanho em hexa\r\n dados\r\n", ate um de tamanho 0
//!   - nenhum dos dois: ate o servidor fechar a conexao
//!
//! Respostas 301/302/303/307/308 com Location sao seguidas (ate 5).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::tcp::Estado;
use super::{ErroRede, Resposta, com, esperar};

const MAX_REDIRECIONAMENTOS: usize = 5;
/// Maior resposta aceita (o heap e grande, mas uma pagina nao precisa mais).
const MAX_RESPOSTA: usize = 32 << 20;

const INCOMPLETA: ErroRede = ErroRede::Protocolo("resposta HTTP incompleta");

/// URL ja separada.
pub struct Url {
    pub host: String,
    pub porta: u16,
    /// Caminho com a consulta ("/a/b?x=1"), sem o fragmento.
    pub caminho: String,
}

impl Url {
    /// "host" ou "host:porta" (a porta so aparece se nao for 80).
    fn autoridade(&self) -> String {
        if self.porta == 80 { self.host.clone() } else { format!("{}:{}", self.host, self.porta) }
    }

    /// A URL inteira de novo, ja normalizada: "http://host[:porta]/caminho".
    fn completa(&self) -> String {
        format!("http://{}{}", self.autoridade(), self.caminho)
    }
}

/// Separa "http://host:porta/caminho?consulta#fragmento". Sem esquema
/// tambem vale ("exemplo.com/x" = "http://exemplo.com/x").
pub fn ler_url(url: &str) -> Result<Url, ErroRede> {
    let url = url.trim();
    let resto = match url.find("://") {
        Some(i) if url[..i].eq_ignore_ascii_case("http") => &url[i + 3..],
        Some(i) if url[..i].eq_ignore_ascii_case("https") => return Err(ErroRede::SoHttp),
        Some(_) => return Err(ErroRede::UrlInvalida),
        None => url,
    };
    let fim_host = resto.find(['/', '?', '#']).unwrap_or(resto.len());
    let (autoridade, caminho) = resto.split_at(fim_host);
    let (host, porta) = match autoridade.rsplit_once(':') {
        Some((host, porta)) => (host, porta.parse::<u16>().map_err(|_| ErroRede::UrlInvalida)?),
        None => (autoridade, 80),
    };
    let host_valido = host.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-' || c == b'_');
    if host.is_empty() || !host_valido || porta == 0 {
        return Err(ErroRede::UrlInvalida);
    }
    let caminho = caminho.split('#').next().unwrap_or("");
    let mut final_ = String::new();
    if !caminho.starts_with('/') {
        final_.push('/');
    }
    for c in caminho.chars() {
        match c {
            ' ' => final_.push_str("%20"),
            c if c.is_ascii_graphic() => final_.push(c),
            _ => final_.push_str("%3F"),
        }
    }
    Ok(Url { host: host.to_ascii_lowercase(), porta, caminho: final_ })
}

/// URL absoluta de um Location (absoluto, "//host/x", "/x" ou relativo).
fn juntar(base: &Url, local: &str) -> String {
    let local = local.trim();
    if local.contains("://") {
        String::from(local)
    } else if let Some(resto) = local.strip_prefix("//") {
        format!("http://{resto}")
    } else if local.starts_with('/') {
        format!("http://{}{}", base.autoridade(), local)
    } else {
        let sem_consulta = base.caminho.split('?').next().unwrap_or("/");
        let pasta = &sem_consulta[..sem_consulta.rfind('/').map_or(0, |i| i + 1)];
        format!("http://{}{}{}", base.autoridade(), pasta, local)
    }
}

fn pedido(url: &Url) -> String {
    format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: MXOS/0.11\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        url.caminho,
        url.autoridade()
    )
}

/// Posicao da primeira ocorrencia de `agulha`.
fn achar(dados: &[u8], agulha: &[u8]) -> Option<usize> {
    dados.windows(agulha.len()).position(|w| w == agulha)
}

/// Bytes para String trocando o que nao e ASCII (imprimivel, tab ou fim
/// de linha) por '?'.
fn ascii(bytes: &[u8]) -> String {
    let fica = |b: u8| (32..=126).contains(&b) || matches!(b, b'\t' | b'\r' | b'\n');
    bytes.iter().map(|&b| if fica(b) { b as char } else { '?' }).collect()
}

pub fn achar_cabecalho<'a>(cabecalhos: &'a [(String, String)], nome: &str) -> Option<&'a str> {
    cabecalhos.iter().find(|(n, _)| n.eq_ignore_ascii_case(nome)).map(|(_, v)| v.as_str())
}

/// Linha de status e cabecalhos (sem a linha vazia do fim).
fn ler_cabecalho(texto: &[u8]) -> Result<(u16, Vec<(String, String)>), ErroRede> {
    let texto = ascii(texto);
    let mut linhas = texto.split("\r\n");
    let status_linha = linhas.next().unwrap_or("");
    let mut partes = status_linha.split_whitespace();
    let versao = partes.next().unwrap_or("");
    let status = partes.next().and_then(|s| s.parse::<u16>().ok());
    let (true, Some(status)) = (versao.starts_with("HTTP/"), status) else {
        return Err(ErroRede::Protocolo("resposta não é HTTP"));
    };
    let cabecalhos = linhas
        .filter_map(|linha| linha.split_once(':'))
        .map(|(nome, valor)| (String::from(nome.trim()), String::from(valor.trim())))
        .collect();
    Ok((status, cabecalhos))
}

/// Corpo chunked. Ok(None) = ainda faltam pedacos.
fn decodificar_chunked(dados: &[u8]) -> Result<Option<Vec<u8>>, ErroRede> {
    let invalido = ErroRede::Protocolo("corpo chunked inválido");
    let mut corpo = Vec::new();
    let mut i = 0;
    loop {
        let Some(fim_linha) = achar(&dados[i..], b"\r\n") else { return Ok(None) };
        let linha = ascii(&dados[i..i + fim_linha]);
        let hexa = linha.split(';').next().unwrap_or("").trim();
        let tamanho = usize::from_str_radix(hexa, 16).map_err(|_| invalido)?;
        if tamanho > MAX_RESPOSTA {
            return Err(invalido);
        }
        i += fim_linha + 2;
        if tamanho == 0 {
            return Ok(Some(corpo)); // os "trailers" depois dele sao ignorados
        }
        if dados.len() < i + tamanho + 2 {
            return Ok(None);
        }
        corpo.extend_from_slice(&dados[i..i + tamanho]);
        i += tamanho + 2;
    }
}

/// Tenta montar a resposta com o que chegou. Ok(None) = falta chegar mais.
/// `fechou` = o servidor ja fechou a conexao (nao vem mais nada).
fn completa(bruto: &mut Vec<u8>, fechou: bool) -> Result<Option<Resposta>, ErroRede> {
    let Some(fim) = achar(bruto, b"\r\n\r\n") else {
        return if fechou { Err(INCOMPLETA) } else { Ok(None) };
    };
    let (status, cabecalhos) = ler_cabecalho(&bruto[..fim])?;
    if (100..200).contains(&status) {
        // resposta provisoria (100 Continue): descarta e espera a de verdade
        bruto.drain(..fim + 4);
        return completa(bruto, fechou);
    }
    let corpo = &bruto[fim + 4..];
    // url_final e preenchida por `get`, que sabe dos redirecionamentos
    let resposta = |corpo: Vec<u8>| Resposta { status, cabecalhos: cabecalhos.clone(), corpo, url_final: String::new() };
    if status == 204 || status == 304 {
        return Ok(Some(resposta(Vec::new())));
    }
    let chunked = achar_cabecalho(&cabecalhos, "transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked"));
    if chunked {
        return match decodificar_chunked(corpo)? {
            Some(c) => Ok(Some(resposta(c))),
            None if fechou => Err(INCOMPLETA),
            None => Ok(None),
        };
    }
    if let Some(tamanho) = achar_cabecalho(&cabecalhos, "content-length").and_then(|v| v.parse::<usize>().ok()) {
        return match corpo.len() >= tamanho {
            true => Ok(Some(resposta(corpo[..tamanho].to_vec()))),
            false if fechou => Err(INCOMPLETA),
            false => Ok(None),
        };
    }
    Ok(fechou.then(|| resposta(corpo.to_vec())))
}

/// Derruba a conexao e devolve o erro.
fn abortar<T>(erro: ErroRede) -> Result<T, ErroRede> {
    let _ = com(|p| p.tcp_abortar());
    Err(erro)
}

/// Um GET sem seguir redirecionamento.
fn buscar(url: &Url, limite_ms: u64) -> Result<Resposta, ErroRede> {
    let ip = super::resolver(&url.host)?;
    com(|p| p.tcp_conectar(ip, url.porta))?;
    let conectou = esperar(limite_ms, |p| match p.tcp.estado {
        Estado::SynEnviado => None,
        Estado::Fechada => Some(Err(p.tcp.erro.unwrap_or(ErroRede::Recusado))),
        _ => Some(Ok(())),
    });
    match conectou {
        Err(e) => return abortar(e),
        Ok(Err(e)) => return Err(e),
        Ok(Ok(())) => {}
    }
    if !com(|p| p.tcp_enviar(pedido(url).as_bytes()))? {
        return abortar(ErroRede::Protocolo("conexão fechou antes do pedido"));
    }
    let mut bruto = Vec::new();
    loop {
        // espera chegar algo (o prazo recomeca a cada pedaco)
        let passo = esperar(limite_ms, |p| {
            let chegou = p.tcp_ler(&mut bruto);
            let terminou = p.tcp.terminou();
            (chegou > 0 || terminou).then_some((terminou, p.tcp.erro))
        });
        let (fechou, erro) = match passo {
            Ok(r) => r,
            Err(e) => return abortar(e),
        };
        if bruto.len() > MAX_RESPOSTA {
            return abortar(ErroRede::Protocolo("resposta grande demais"));
        }
        match completa(&mut bruto, fechou) {
            Ok(Some(resposta)) => {
                let _ = com(|p| p.tcp_fechar());
                return Ok(resposta);
            }
            Ok(None) => {}
            Err(e) => return abortar(erro.unwrap_or(e)),
        }
    }
}

/// GET seguindo redirecionamentos. A resposta diz em `url_final` quem
/// respondeu de fato.
pub fn get(url: &str, limite_ms: u64) -> Result<Resposta, ErroRede> {
    let mut atual = String::from(url);
    for _ in 0..=MAX_REDIRECIONAMENTOS {
        let u = ler_url(&atual)?;
        let mut resposta = buscar(&u, limite_ms)?;
        let redireciona = matches!(resposta.status, 301 | 302 | 303 | 307 | 308);
        match resposta.cabecalho("location") {
            Some(local) if redireciona => atual = juntar(&u, local),
            _ => {
                resposta.url_final = u.completa();
                return Ok(resposta);
            }
        }
    }
    Err(ErroRede::Protocolo("redirecionamentos demais"))
}
