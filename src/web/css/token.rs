//! Tokenizador do CSS (CSS Syntax Level 3, secao 4). Comentarios somem;
//! numeros guardam o valor em f32 so ate virarem ponto fixo (valores.rs).
//!
//! Anda nos BYTES do texto (UTF-8): tudo que delimita tokens e ASCII, e
//! bytes >= 0x80 contam como letras de nome, como manda a especificacao.
//! Nomes e textos sem escapes saem como uma copia direta do pedaco.

use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    Ident(String),
    Funcao(String),
    Arroba(String),
    Hash(String, bool), // (nome, e um identificador valido: serve de #id)
    Texto(String),
    TextoRuim,
    Url(String),
    UrlRuim,
    Delim(char),
    Numero(f32, bool), // (valor, inteiro)
    Porcentagem(f32),
    Dimensao(f32, String),
    Espaco,
    Cdo,
    Cdc,
    DoisPontos,
    PontoEVirgula,
    Virgula,
    AbreColchete,
    FechaColchete,
    AbreParen,
    FechaParen,
    AbreChave,
    FechaChave,
}

fn inicio_nome(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c >= 0x80
}

fn nome(c: u8) -> bool {
    inicio_nome(c) || c.is_ascii_digit() || c == b'-'
}

fn espaco(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | b'\x0C')
}

pub struct Tokenizador<'a> {
    t: &'a str,
    b: &'a [u8],
    i: usize,
}

impl<'a> Tokenizador<'a> {
    pub fn novo(texto: &'a str) -> Self {
        Tokenizador { t: texto, b: texto.as_bytes(), i: 0 }
    }

    fn espiar(&self, k: usize) -> Option<u8> {
        self.b.get(self.i + k).copied()
    }

    fn escape_valido(&self, k: usize) -> bool {
        self.espiar(k) == Some(b'\\') && self.espiar(k + 1).is_some_and(|c| c != b'\n')
    }

    fn comeca_ident(&self, k: usize) -> bool {
        match self.espiar(k) {
            Some(b'-') => self.espiar(k + 1).is_some_and(|c| inicio_nome(c) || c == b'-') || self.escape_valido(k + 1),
            Some(b'\\') => self.escape_valido(k),
            Some(c) => inicio_nome(c),
            None => false,
        }
    }

    fn comeca_numero(&self, k: usize) -> bool {
        match self.espiar(k) {
            Some(b'+' | b'-') => match self.espiar(k + 1) {
                Some(c) if c.is_ascii_digit() => true,
                Some(b'.') => self.espiar(k + 2).is_some_and(|c| c.is_ascii_digit()),
                _ => false,
            },
            Some(b'.') => self.espiar(k + 1).is_some_and(|c| c.is_ascii_digit()),
            Some(c) => c.is_ascii_digit(),
            None => false,
        }
    }

    /// Escape depois do '\\' (ja consumido).
    fn escape(&mut self) -> char {
        let inicio = self.i;
        while self.i - inicio < 6 && self.espiar(0).is_some_and(|c| c.is_ascii_hexdigit()) {
            self.i += 1;
        }
        if self.i == inicio {
            return match self.t[self.i..].chars().next() {
                Some(c) => {
                    self.i += c.len_utf8();
                    c
                }
                None => '\u{FFFD}',
            };
        }
        let hexa = &self.t[inicio..self.i];
        if self.espiar(0).is_some_and(espaco) {
            self.i += 1;
        }
        match u32::from_str_radix(hexa, 16) {
            Ok(0) | Err(_) => '\u{FFFD}',
            Ok(v) => char::from_u32(v).unwrap_or('\u{FFFD}'),
        }
    }

    fn nome(&mut self) -> String {
        let inicio = self.i;
        while self.espiar(0).is_some_and(nome) {
            self.i += 1;
        }
        if !self.escape_valido(0) {
            return String::from(&self.t[inicio..self.i]);
        }
        let mut s = String::from(&self.t[inicio..self.i]);
        loop {
            match self.espiar(0) {
                Some(c) if nome(c) => {
                    let ini = self.i;
                    while self.espiar(0).is_some_and(nome) {
                        self.i += 1;
                    }
                    s.push_str(&self.t[ini..self.i]);
                }
                Some(b'\\') if self.escape_valido(0) => {
                    self.i += 1;
                    s.push(self.escape());
                }
                _ => return s,
            }
        }
    }

    fn numero(&mut self) -> (f32, bool) {
        let inicio = self.i;
        let mut inteiro = true;
        if matches!(self.espiar(0), Some(b'+' | b'-')) {
            self.i += 1;
        }
        while self.espiar(0).is_some_and(|c| c.is_ascii_digit()) {
            self.i += 1;
        }
        if self.espiar(0) == Some(b'.') && self.espiar(1).is_some_and(|c| c.is_ascii_digit()) {
            inteiro = false;
            self.i += 1;
            while self.espiar(0).is_some_and(|c| c.is_ascii_digit()) {
                self.i += 1;
            }
        }
        if matches!(self.espiar(0), Some(b'e' | b'E')) {
            let k = if matches!(self.espiar(1), Some(b'+' | b'-')) { 2 } else { 1 };
            if self.espiar(k).is_some_and(|c| c.is_ascii_digit()) {
                inteiro = false;
                self.i += k;
                while self.espiar(0).is_some_and(|c| c.is_ascii_digit()) {
                    self.i += 1;
                }
            }
        }
        let texto = &self.t[inicio..self.i];
        // inteiros curtos sem o parse de f32 (o caso comum: "10px", "0")
        if inteiro && texto.len() <= 9 {
            if let Ok(v) = texto.parse::<i32>() {
                return (v as f32, true);
            }
        }
        (texto.parse::<f32>().unwrap_or(0.0), inteiro)
    }

    fn numerico(&mut self) -> Token {
        let (v, inteiro) = self.numero();
        if self.comeca_ident(0) {
            let mut u = self.nome();
            u.make_ascii_lowercase();
            return Token::Dimensao(v, u);
        }
        if self.espiar(0) == Some(b'%') {
            self.i += 1;
            return Token::Porcentagem(v);
        }
        Token::Numero(v, inteiro)
    }

    fn texto(&mut self, fim: u8) -> Token {
        let inicio = self.i;
        while let Some(c) = self.espiar(0) {
            if c == fim {
                let s = String::from(&self.t[inicio..self.i]);
                self.i += 1;
                return Token::Texto(s);
            }
            if c == b'\\' || c == b'\n' {
                break;
            }
            self.i += 1;
        }
        let mut s = String::from(&self.t[inicio..self.i]);
        loop {
            match self.espiar(0) {
                None => return Token::Texto(s),
                Some(c) if c == fim => {
                    self.i += 1;
                    return Token::Texto(s);
                }
                Some(b'\n') => {
                    self.i += 1;
                    return Token::TextoRuim;
                }
                Some(b'\\') => {
                    self.i += 1;
                    match self.espiar(0) {
                        None => {}
                        Some(b'\n') => self.i += 1,
                        Some(_) => s.push(self.escape()),
                    }
                }
                Some(_) => {
                    let ini = self.i;
                    while self.espiar(0).is_some_and(|c| c != fim && c != b'\\' && c != b'\n') {
                        self.i += 1;
                    }
                    s.push_str(&self.t[ini..self.i]);
                }
            }
        }
    }

    fn url(&mut self) -> Token {
        while self.espiar(0).is_some_and(espaco) {
            self.i += 1;
        }
        let mut s = String::new();
        loop {
            match self.espiar(0) {
                None => return Token::Url(s),
                Some(b')') => {
                    self.i += 1;
                    return Token::Url(s);
                }
                Some(c) if espaco(c) => {
                    while self.espiar(0).is_some_and(espaco) {
                        self.i += 1;
                    }
                    return match self.espiar(0) {
                        None => Token::Url(s),
                        Some(b')') => {
                            self.i += 1;
                            Token::Url(s)
                        }
                        _ => self.resto_url_ruim(),
                    };
                }
                Some(b'"' | b'\'' | b'(') => return self.resto_url_ruim(),
                Some(b'\\') => {
                    self.i += 1;
                    s.push(self.escape());
                }
                Some(_) => {
                    let ini = self.i;
                    while self.espiar(0).is_some_and(|c| !espaco(c) && !matches!(c, b')' | b'"' | b'\'' | b'(' | b'\\')) {
                        self.i += 1;
                    }
                    s.push_str(&self.t[ini..self.i]);
                }
            }
        }
    }

    fn resto_url_ruim(&mut self) -> Token {
        loop {
            match self.espiar(0) {
                None => return Token::UrlRuim,
                Some(b')') => {
                    self.i += 1;
                    return Token::UrlRuim;
                }
                Some(b'\\') => self.i = (self.i + 2).min(self.b.len()),
                _ => self.i += 1,
            }
        }
    }

    fn ident_ou_funcao(&mut self) -> Token {
        let mut n = self.nome();
        if self.espiar(0) == Some(b'(') {
            self.i += 1;
            if n.eq_ignore_ascii_case("url") {
                let mut k = 0;
                while self.espiar(k).is_some_and(espaco) {
                    k += 1;
                }
                if !matches!(self.espiar(k), Some(b'"' | b'\'')) {
                    return self.url();
                }
            }
            n.make_ascii_lowercase();
            return Token::Funcao(n);
        }
        Token::Ident(n)
    }

    fn um(&mut self, t: Token) -> Option<Token> {
        self.i += 1;
        Some(t)
    }

    pub fn proximo(&mut self) -> Option<Token> {
        while self.espiar(0) == Some(b'/') && self.espiar(1) == Some(b'*') {
            match self.t[self.i + 2..].find("*/") {
                Some(p) => self.i += 2 + p + 2,
                None => {
                    self.i = self.b.len();
                    return None;
                }
            }
        }
        let c = self.espiar(0)?;
        if espaco(c) {
            while self.espiar(0).is_some_and(espaco) {
                self.i += 1;
            }
            return Some(Token::Espaco);
        }
        match c {
            b'"' | b'\'' => {
                self.i += 1;
                Some(self.texto(c))
            }
            b'#' => {
                self.i += 1;
                if self.espiar(0).is_some_and(nome) || self.escape_valido(0) {
                    let id = self.comeca_ident(0);
                    Some(Token::Hash(self.nome(), id))
                } else {
                    Some(Token::Delim('#'))
                }
            }
            b'(' => self.um(Token::AbreParen),
            b')' => self.um(Token::FechaParen),
            b'[' => self.um(Token::AbreColchete),
            b']' => self.um(Token::FechaColchete),
            b'{' => self.um(Token::AbreChave),
            b'}' => self.um(Token::FechaChave),
            b',' => self.um(Token::Virgula),
            b':' => self.um(Token::DoisPontos),
            b';' => self.um(Token::PontoEVirgula),
            b'+' | b'.' if self.comeca_numero(0) => Some(self.numerico()),
            b'-' => {
                if self.comeca_numero(0) {
                    Some(self.numerico())
                } else if self.espiar(1) == Some(b'-') && self.espiar(2) == Some(b'>') {
                    self.i += 3;
                    Some(Token::Cdc)
                } else if self.comeca_ident(0) {
                    Some(self.ident_ou_funcao())
                } else {
                    self.um(Token::Delim('-'))
                }
            }
            b'<' if self.t[self.i..].starts_with("<!--") => {
                self.i += 4;
                Some(Token::Cdo)
            }
            b'@' => {
                self.i += 1;
                if self.comeca_ident(0) {
                    let mut n = self.nome();
                    n.make_ascii_lowercase();
                    Some(Token::Arroba(n))
                } else {
                    Some(Token::Delim('@'))
                }
            }
            b'\\' if self.escape_valido(0) => Some(self.ident_ou_funcao()),
            c if c.is_ascii_digit() => Some(self.numerico()),
            c if inicio_nome(c) => Some(self.ident_ou_funcao()),
            _ => {
                let ch = self.t[self.i..].chars().next().unwrap_or('\u{FFFD}');
                self.i += ch.len_utf8().max(1);
                Some(Token::Delim(ch))
            }
        }
    }
}

/// O texto inteiro em tokens.
pub fn tokens(texto: &str) -> Vec<Token> {
    let mut t = Tokenizador::novo(texto);
    let mut v = Vec::with_capacity(texto.len() / 4);
    while let Some(x) = t.proximo() {
        v.push(x);
    }
    v
}
