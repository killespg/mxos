//! URLs (o essencial do WHATWG URL): separar e resolver relativos.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Url {
    pub esquema: String,
    /// "host[:porta]" (vazio em disco:, data:, about:...).
    pub autoridade: String,
    /// Caminho com a consulta, sem o fragmento.
    pub caminho: String,
    pub fragmento: String,
}

impl Url {
    pub fn ler(s: &str) -> Option<Url> {
        let s = s.trim();
        let dois = s.find(':')?;
        let esquema = &s[..dois];
        if esquema.is_empty() || !esquema.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'+' || c == b'-' || c == b'.') {
            return None;
        }
        let esquema = esquema.to_ascii_lowercase();
        let resto = &s[dois + 1..];
        let (resto, fragmento) = match resto.find('#') {
            Some(i) => (&resto[..i], &resto[i + 1..]),
            None => (resto, ""),
        };
        if let Some(r) = resto.strip_prefix("//") {
            let fim = r.find(['/', '?']).unwrap_or(r.len());
            let mut autoridade = String::from(&r[..fim]);
            if let Some(arroba) = autoridade.rfind('@') {
                autoridade = String::from(&autoridade[arroba + 1..]);
            }
            let autoridade = autoridade.to_ascii_lowercase();
            let mut caminho = String::from(&r[fim..]);
            if !caminho.starts_with('/') {
                caminho.insert(0, '/');
            }
            Some(Url { esquema, autoridade, caminho: normalizar(&caminho), fragmento: fragmento.into() })
        } else {
            Some(Url { esquema, autoridade: String::new(), caminho: resto.into(), fragmento: fragmento.into() })
        }
    }

    pub fn host(&self) -> &str {
        match self.autoridade.rfind(':') {
            Some(i) if !self.autoridade.ends_with(']') => &self.autoridade[..i],
            _ => &self.autoridade,
        }
    }

    pub fn porta(&self) -> u16 {
        if let Some(i) = self.autoridade.rfind(':') {
            if let Ok(p) = self.autoridade[i + 1..].parse() {
                return p;
            }
        }
        match self.esquema.as_str() {
            "https" | "wss" => 443,
            _ => 80,
        }
    }

    pub fn origem(&self) -> String {
        format!("{}://{}", self.esquema, self.autoridade)
    }

    pub fn texto(&self) -> String {
        let mut s = if self.autoridade.is_empty() && !matches!(self.esquema.as_str(), "http" | "https" | "file") {
            format!("{}:{}", self.esquema, self.caminho)
        } else {
            format!("{}://{}{}", self.esquema, self.autoridade, self.caminho)
        };
        if !self.fragmento.is_empty() {
            s.push('#');
            s.push_str(&self.fragmento);
        }
        s
    }

    /// Resolve `rel` (absoluto, "//host/x", "/x", "x", "?q", "#f") contra esta.
    pub fn juntar(&self, rel: &str) -> Option<Url> {
        let rel = rel.trim();
        if rel.is_empty() {
            let mut u = self.clone();
            u.fragmento.clear();
            return Some(u);
        }
        if let Some(i) = rel.find(':') {
            let e = &rel[..i];
            if !e.is_empty() && e.as_bytes()[0].is_ascii_alphabetic() && e.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'+' || c == b'-' || c == b'.') && !rel[..i].contains('/') {
                return Url::ler(rel);
            }
        }
        if let Some(r) = rel.strip_prefix("//") {
            return Url::ler(&format!("{}://{}", self.esquema, r));
        }
        let (rel, fragmento) = match rel.find('#') {
            Some(i) => (&rel[..i], &rel[i + 1..]),
            None => (rel, ""),
        };
        let caminho = if rel.is_empty() {
            self.caminho.clone()
        } else if rel.starts_with('/') {
            String::from(rel)
        } else if rel.starts_with('?') {
            format!("{}{}", self.caminho.split('?').next().unwrap_or("/"), rel)
        } else {
            let base = self.caminho.split('?').next().unwrap_or("/");
            let pasta = &base[..base.rfind('/').map_or(0, |i| i + 1)];
            format!("{pasta}{rel}")
        };
        Some(Url {
            esquema: self.esquema.clone(),
            autoridade: self.autoridade.clone(),
            caminho: normalizar(&caminho),
            fragmento: fragmento.into(),
        })
    }
}

/// Resolve "." e ".." (a consulta fica igual) e codifica espacos.
fn normalizar(caminho: &str) -> String {
    let (c, consulta) = match caminho.find('?') {
        Some(i) => (&caminho[..i], &caminho[i..]),
        None => (caminho, ""),
    };
    let mut partes: Vec<&str> = Vec::new();
    let pecas: Vec<&str> = c.split('/').collect();
    for (i, p) in pecas.iter().enumerate().skip(1) {
        let ultima = i + 1 == pecas.len();
        match *p {
            "." => {
                if ultima {
                    partes.push("");
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
    let mut s = String::with_capacity(caminho.len());
    for p in &partes {
        s.push('/');
        s.push_str(p);
    }
    if s.is_empty() {
        s.push('/');
    }
    s.push_str(consulta);
    s.replace(' ', "%20")
}

/// Junta duas URLs em texto (atalho).
pub fn resolver(base: &str, rel: &str) -> String {
    match Url::ler(base).and_then(|b| b.juntar(rel)) {
        Some(u) => u.texto(),
        None => String::from(rel),
    }
}

/// Base64 (RFC 4648, aceita sem "=" e com espacos).
pub fn base64(s: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(s.len() * 3 / 4);
    let mut buf = 0u32;
    let mut n = 0;
    for c in s.bytes() {
        let x = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => continue,
        } as u32;
        buf = (buf << 6) | x;
        n += 6;
        if n >= 8 {
            n -= 8;
            v.push((buf >> n) as u8);
        }
    }
    v
}

/// Conteudo de um "data:tipo;base64,...." (ou com %XX).
pub fn dados(url: &str) -> Option<Vec<u8>> {
    let resto = url.strip_prefix("data:")?;
    let virgula = resto.find(',')?;
    let (cab, corpo) = (&resto[..virgula], &resto[virgula + 1..]);
    if cab.ends_with(";base64") {
        return Some(base64(corpo));
    }
    // %XX
    let b = corpo.as_bytes();
    let mut v = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(x) = u8::from_str_radix(&corpo[i + 1..i + 3], 16) {
                v.push(x);
                i += 3;
                continue;
            }
        }
        v.push(b[i]);
        i += 1;
    }
    Some(v)
}
