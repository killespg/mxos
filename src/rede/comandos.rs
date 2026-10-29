//! Comandos de rede do shell: ip, ping, dns e http.
//! O corpo do http e mostrado como UTF-8 (o que nao for vira '?').

use core::fmt::Write;

use super::{ErroRede, Ip, Mac, http_get, info, ping, resolver};

/// Pings que o comando manda e o prazo de cada um.
const PINGS: u16 = 3;
const PRAZO_PING_MS: u64 = 1000;
const PRAZO_HTTP_MS: u64 = 5000;
/// Linhas do corpo mostradas pelo "http" e largura maxima delas.
const LINHAS_CORPO: usize = 12;
const COLUNAS: usize = 78;

macro_rules! diga {
    ($saida:expr, $($arg:tt)*) => {{ let _ = writeln!($saida, $($arg)*); }};
}

/// Trata os comandos de rede. false = nao e comando daqui.
pub fn comando(nome: &str, args: &str, saida: &mut dyn Write) -> bool {
    let args = args.trim();
    match nome {
        "ip" => mostrar_ip(saida),
        "ping" => comando_ping(args, saida),
        "dns" => comando_dns(args, saida),
        "http" => comando_http(args, saida),
        _ => return false,
    }
    true
}

fn mostrar_ip(saida: &mut dyn Write) {
    let Some(i) = info() else {
        diga!(saida, "Rede: {}", ErroRede::SemRede.descricao());
        return;
    };
    diga!(saida, "Rede: virtio-net  MAC {}", Mac(i.mac));
    diga!(saida, "  IP       {}", Ip(i.ip));
    diga!(saida, "  máscara  {}", Ip(i.mascara));
    diga!(saida, "  gateway  {}", Ip(i.gateway));
    diga!(saida, "  DNS      {}", Ip(i.dns));
    diga!(saida, "  pacotes  {} recebidos, {} enviados", i.pacotes_rx, i.pacotes_tx);
}

fn comando_ping(host: &str, saida: &mut dyn Write) {
    if host.is_empty() {
        diga!(saida, "uso: ping <host>   (ex.: ping 10.0.2.2)");
        return;
    }
    if info().is_none() {
        diga!(saida, "ping: {}", ErroRede::SemRede.descricao());
        return;
    }
    let ip = match resolver(host) {
        Ok(ip) => ip,
        Err(e) => {
            diga!(saida, "ping: {host}: {}", e.descricao());
            return;
        }
    };
    diga!(saida, "PING {host} ({})", Ip(ip));
    let mut recebidos = 0;
    for seq in 1..=PINGS {
        match ping(ip, PRAZO_PING_MS) {
            Ok(ms) => {
                recebidos += 1;
                diga!(saida, "  resposta de {}: seq={seq} tempo={ms} ms", Ip(ip));
            }
            Err(e) => diga!(saida, "  seq={seq}: {}", e.descricao()),
        }
    }
    diga!(saida, "{PINGS} enviados, {recebidos} recebidos");
}

fn comando_dns(nome: &str, saida: &mut dyn Write) {
    if nome.is_empty() {
        diga!(saida, "uso: dns <nome>   (ex.: dns example.com)");
        return;
    }
    match resolver(nome) {
        Ok(ip) => diga!(saida, "{nome} -> {}", Ip(ip)),
        Err(e) => diga!(saida, "dns: {nome}: {}", e.descricao()),
    }
}

fn comando_http(url: &str, saida: &mut dyn Write) {
    if url.is_empty() {
        diga!(saida, "uso: http <url>   (ex.: http http://10.0.2.2:8000/)");
        return;
    }
    let resposta = match http_get(url, PRAZO_HTTP_MS) {
        Ok(r) => r,
        Err(e) => {
            diga!(saida, "http: {}", e.descricao());
            return;
        }
    };
    diga!(saida, "HTTP {}", resposta.status);
    for (nome, valor) in &resposta.cabecalhos {
        diga!(saida, "  {}: {}", cortar(nome), cortar(valor));
    }
    diga!(saida, "--- corpo: {} bytes ---", resposta.corpo.len());
    let texto: alloc::string::String = alloc::string::String::from_utf8_lossy(&resposta.corpo).chars().map(imprimivel).collect();
    let linhas: alloc::vec::Vec<&str> = texto.lines().collect();
    for linha in linhas.iter().take(LINHAS_CORPO) {
        diga!(saida, "{}", cortar(linha.trim_end()));
    }
    if linhas.len() > LINHAS_CORPO {
        diga!(saida, "… (mais {} linhas)", linhas.len() - LINHAS_CORPO);
    }
}

/// Tab vira espaco, quebra de linha fica, outros controles e UTF-8
/// quebrado (U+FFFD) viram '?'.
fn imprimivel(c: char) -> char {
    match c {
        '\n' | '\r' => c,
        '\t' => ' ',
        '\u{FFFD}' => '?',
        c if c.is_control() => '?',
        c => c,
    }
}

/// No maximo COLUNAS caracteres (nao bytes: acentos ocupam dois).
fn cortar(texto: &str) -> &str {
    match texto.char_indices().nth(COLUNAS) {
        Some((fim, _)) => &texto[..fim],
        None => texto,
    }
}
