//! Bytes da rede para texto: descobre a codificacao (BOM, cabecalho
//! Content-Type, <meta charset> nos primeiros 1024 bytes) e converte para
//! UTF-8. Fora o UTF-8, a web usa quase so o windows-1252 (que o HTML
//! manda usar no lugar do ISO-8859-1 e do ASCII).

use alloc::string::String;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Codificacao {
    Utf8,
    Windows1252,
}

/// 0x80..0x9F do windows-1252 (o resto e igual ao Latin-1).
pub const WIN1252: [u16; 32] = [
    0x20AC, 0x81, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x8D,
    0x017D, 0x8F, 0x90, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A,
    0x0153, 0x9D, 0x017E, 0x0178,
];

fn pelo_nome(nome: &str) -> Option<Codificacao> {
    let n = nome.trim().trim_matches(|c| c == '"' || c == '\'').to_ascii_lowercase();
    match n.as_str() {
        "utf-8" | "utf8" | "unicode-1-1-utf-8" => Some(Codificacao::Utf8),
        "iso-8859-1" | "latin1" | "windows-1252" | "cp1252" | "us-ascii" | "ascii" | "iso8859-1" | "l1"
        | "iso-8859-15" | "latin-1" | "x-cp1252" => Some(Codificacao::Windows1252),
        // utf-16 num <meta> e mentira (o texto ja esta em bytes ASCII)
        "utf-16" | "utf-16le" | "utf-16be" => Some(Codificacao::Utf8),
        _ => None,
    }
}

/// "text/html; charset=ISO-8859-1" -> Windows1252.
pub fn do_content_type(valor: &str) -> Option<Codificacao> {
    let i = valor.to_ascii_lowercase().find("charset=")?;
    let resto = &valor[i + 8..];
    pelo_nome(resto.split([';', ' ']).next().unwrap_or(""))
}

/// Procura <meta charset="..."> ou <meta http-equiv content="...charset=">
/// no comeco do documento.
fn prever(bytes: &[u8]) -> Option<Codificacao> {
    let inicio = &bytes[..bytes.len().min(1024)];
    let texto: String = inicio.iter().map(|&b| (b as char).to_ascii_lowercase()).collect();
    let mut resto = texto.as_str();
    while let Some(i) = resto.find("<meta") {
        let fim = resto[i..].find('>').map_or(resto.len(), |f| i + f);
        let tag = &resto[i..fim];
        if let Some(j) = tag.find("charset") {
            let depois = tag[j + 7..].trim_start();
            if let Some(valor) = depois.strip_prefix('=') {
                let valor = valor.trim_start().trim_start_matches(['"', '\'']);
                let nome: String = valor.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
                if let Some(c) = pelo_nome(&nome) {
                    return Some(c);
                }
            }
        }
        resto = &resto[fim.min(resto.len())..];
        if resto.is_empty() {
            break;
        }
    }
    None
}

/// Descobre a codificacao: BOM > cabecalho HTTP > <meta> > UTF-8 se for
/// UTF-8 valido > windows-1252.
pub fn descobrir(bytes: &[u8], content_type: Option<&str>) -> Codificacao {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return Codificacao::Utf8;
    }
    if let Some(c) = content_type.and_then(do_content_type) {
        return c;
    }
    if let Some(c) = prever(bytes) {
        return c;
    }
    if core::str::from_utf8(bytes).is_ok() { Codificacao::Utf8 } else { Codificacao::Windows1252 }
}

/// Converte para String (UTF-8 invalido vira U+FFFD).
pub fn decodificar(bytes: &[u8], c: Codificacao) -> String {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match c {
        Codificacao::Utf8 => match core::str::from_utf8(bytes) {
            Ok(s) => String::from(s),
            Err(_) => String::from_utf8_lossy(bytes).into_owned(),
        },
        Codificacao::Windows1252 => {
            let mut s = String::with_capacity(bytes.len() + bytes.len() / 8);
            for &b in bytes {
                let c = match b {
                    0x80..=0x9F => char::from_u32(WIN1252[(b - 0x80) as usize] as u32).unwrap_or('\u{FFFD}'),
                    _ => b as char,
                };
                s.push(c);
            }
            s
        }
    }
}

/// Tudo de uma vez.
pub fn texto(bytes: &[u8], content_type: Option<&str>) -> String {
    decodificar(bytes, descobrir(bytes, content_type))
}
