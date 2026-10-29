//! JSON pequeno para o kernel (APIs como a do Claude): ler para uma
//! arvore e escrever textos com escape. Numeros ficam f64.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Nulo,
    Bool(bool),
    Num(f64),
    Texto(String),
    Lista(Vec<Json>),
    Objeto(BTreeMap<String, Json>),
}

impl Json {
    pub fn campo(&self, nome: &str) -> Option<&Json> {
        match self {
            Json::Objeto(m) => m.get(nome),
            _ => None,
        }
    }
    pub fn texto(&self) -> Option<&str> {
        match self {
            Json::Texto(s) => Some(s),
            _ => None,
        }
    }
    pub fn lista(&self) -> &[Json] {
        match self {
            Json::Lista(v) => v,
            _ => &[],
        }
    }
}

/// Texto entre aspas com os escapes do JSON.
pub fn texto(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&alloc::format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

struct Leitor<'a> {
    b: &'a [u8],
    i: usize,
    prof: u32,
}

impl Leitor<'_> {
    fn espacos(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\r' | b'\n') {
            self.i += 1;
        }
    }

    fn valor(&mut self) -> Option<Json> {
        self.espacos();
        self.prof += 1;
        if self.prof > 200 {
            return None;
        }
        let v = match *self.b.get(self.i)? {
            b'{' => {
                self.i += 1;
                let mut m = BTreeMap::new();
                self.espacos();
                if self.b.get(self.i) == Some(&b'}') {
                    self.i += 1;
                } else {
                    loop {
                        self.espacos();
                        let k = self.texto_lido()?;
                        self.espacos();
                        (self.b.get(self.i) == Some(&b':')).then_some(())?;
                        self.i += 1;
                        let v = self.valor()?;
                        m.insert(k, v);
                        self.espacos();
                        match self.b.get(self.i)? {
                            b',' => self.i += 1,
                            b'}' => {
                                self.i += 1;
                                break;
                            }
                            _ => return None,
                        }
                    }
                }
                Json::Objeto(m)
            }
            b'[' => {
                self.i += 1;
                let mut v = Vec::new();
                self.espacos();
                if self.b.get(self.i) == Some(&b']') {
                    self.i += 1;
                } else {
                    loop {
                        v.push(self.valor()?);
                        self.espacos();
                        match self.b.get(self.i)? {
                            b',' => self.i += 1,
                            b']' => {
                                self.i += 1;
                                break;
                            }
                            _ => return None,
                        }
                    }
                }
                Json::Lista(v)
            }
            b'"' => Json::Texto(self.texto_lido()?),
            b't' if self.b[self.i..].starts_with(b"true") => {
                self.i += 4;
                Json::Bool(true)
            }
            b'f' if self.b[self.i..].starts_with(b"false") => {
                self.i += 5;
                Json::Bool(false)
            }
            b'n' if self.b[self.i..].starts_with(b"null") => {
                self.i += 4;
                Json::Nulo
            }
            _ => {
                let ini = self.i;
                while self.i < self.b.len() && matches!(self.b[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
                    self.i += 1;
                }
                let s = core::str::from_utf8(&self.b[ini..self.i]).ok()?;
                Json::Num(s.parse().ok()?)
            }
        };
        self.prof -= 1;
        Some(v)
    }

    fn hex4(&mut self) -> Option<u32> {
        let s = core::str::from_utf8(self.b.get(self.i..self.i + 4)?).ok()?;
        self.i += 4;
        u32::from_str_radix(s, 16).ok()
    }

    fn texto_lido(&mut self) -> Option<String> {
        (self.b.get(self.i) == Some(&b'"')).then_some(())?;
        self.i += 1;
        let mut bytes = Vec::new();
        loop {
            let c = *self.b.get(self.i)?;
            self.i += 1;
            match c {
                b'"' => break,
                b'\\' => {
                    let e = *self.b.get(self.i)?;
                    self.i += 1;
                    let ch = match e {
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'u' => {
                            let mut v = self.hex4()?;
                            // par substituto
                            if (0xD800..0xDC00).contains(&v) && self.b.get(self.i..self.i + 2) == Some(b"\\u") {
                                self.i += 2;
                                let baixo = self.hex4()?;
                                v = 0x10000 + ((v - 0xD800) << 10) + (baixo.wrapping_sub(0xDC00) & 0x3FF);
                            }
                            char::from_u32(v).unwrap_or('\u{FFFD}')
                        }
                        outro => outro as char,
                    };
                    let mut buf = [0u8; 4];
                    bytes.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
                _ => bytes.push(c),
            }
        }
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// Le um documento JSON inteiro.
pub fn ler(dados: &[u8]) -> Option<Json> {
    let mut l = Leitor { b: dados, i: 0, prof: 0 };
    let v = l.valor()?;
    l.espacos();
    (l.i == dados.len()).then_some(v)
}
