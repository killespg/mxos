//! Analisador lexico do JavaScript (ECMAScript 2022): identificadores
//! (com Unicode), palavras reservadas, numeros (decimal, hexa, octal,
//! binario, separador _), textos com escapes, templates, expressoes
//! regulares (decididas pelo contexto) e pontuacao. Guarda se houve quebra
//! de linha antes de cada token (insercao automatica de ponto e virgula).

use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Ident(Rc<str>),
    /// Palavra reservada (guardada como texto: o parser compara).
    Palavra(&'static str),
    Num(f64),
    BigInt(Rc<str>),
    Texto(Rc<str>),
    /// Pedaco de template: (cozido, cru, termina o template?)
    Template(Option<Rc<str>>, Rc<str>, bool),
    /// Pedaco de template depois de um ${...} (nao e inicio de template).
    TemplateMeio(Option<Rc<str>>, Rc<str>, bool),
    Regex(Rc<str>, Rc<str>),
    Pont(&'static str),
    PrivadoIdent(Rc<str>),
    Fim,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub linha_antes: bool,
    pub pos: u32,
    pub linha: u32,
}

const PALAVRAS: &[&str] = &[
    "await", "break", "case", "catch", "class", "const", "continue", "debugger", "default", "delete", "do", "else", "export",
    "extends", "false", "finally", "for", "function", "if", "import", "in", "instanceof", "new", "null", "return", "super",
    "switch", "this", "throw", "true", "try", "typeof", "var", "void", "while", "with", "yield", "let", "static", "async",
    "of", "get", "set",
];

/// Palavras que tambem podem ser nomes (let, async, of, get, set, static...).
pub fn contextual(p: &str) -> bool {
    matches!(p, "let" | "static" | "async" | "of" | "get" | "set" | "await" | "yield")
}

const PONTUACAO: &[&str] = &[
    ">>>=", "...", "===", "!==", "**=", "<<=", ">>=", ">>>", "&&=", "||=", "??=", "=>", "==", "!=", "<=", ">=", "&&", "||", "??",
    "?.", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "<<", ">>", "**", "{", "}", "(", ")", "[", "]", ";", ",",
    "<", ">", "+", "-", "*", "/", "%", "&", "|", "^", "!", "~", "?", ":", "=", ".", "@", "#",
];

pub struct Lexico<'a> {
    s: &'a str,
    b: &'a [u8],
    pub i: usize,
    linha: u32,
    /// Profundidade de chaves dentro de cada ${ } de template aberto.
    templates: Vec<u32>,
    chaves: u32,
}

#[derive(Debug, Clone)]
pub struct ErroLexico {
    pub msg: &'static str,
    pub linha: u32,
}

fn inicio_ident(c: char) -> bool {
    c.is_alphabetic() || c == '$' || c == '_'
}

fn parte_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '$' || c == '_' || c == '\u{200C}' || c == '\u{200D}'
}

impl<'a> Lexico<'a> {
    pub fn novo(s: &'a str) -> Self {
        Lexico { s, b: s.as_bytes(), i: 0, linha: 1, templates: Vec::new(), chaves: 0 }
    }

    fn espiar(&self, k: usize) -> u8 {
        self.b.get(self.i + k).copied().unwrap_or(0)
    }

    fn char_em(&self) -> Option<char> {
        self.s[self.i..].chars().next()
    }

    /// Pula espacos e comentarios; diz se passou por quebra de linha.
    fn pular(&mut self) -> Result<bool, ErroLexico> {
        let mut quebra = false;
        loop {
            let Some(c) = self.char_em() else { return Ok(quebra) };
            match c {
                '\n' | '\r' | '\u{2028}' | '\u{2029}' => {
                    quebra = true;
                    if c == '\n' {
                        self.linha += 1;
                    }
                    self.i += c.len_utf8();
                }
                ' ' | '\t' | '\x0B' | '\x0C' | '\u{A0}' | '\u{FEFF}' => self.i += c.len_utf8(),
                c if c.is_whitespace() => self.i += c.len_utf8(),
                '/' if self.espiar(1) == b'/' => {
                    while let Some(c) = self.char_em() {
                        if c == '\n' || c == '\r' {
                            break;
                        }
                        self.i += c.len_utf8();
                    }
                }
                '/' if self.espiar(1) == b'*' => {
                    match self.s[self.i + 2..].find("*/") {
                        Some(p) => {
                            let corpo = &self.s[self.i + 2..self.i + 2 + p];
                            let n = corpo.bytes().filter(|&b| b == b'\n').count() as u32;
                            if n > 0 {
                                quebra = true;
                                self.linha += n;
                            }
                            self.i += p + 4;
                        }
                        None => return Err(ErroLexico { msg: "comentário sem fim", linha: self.linha }),
                    }
                }
                // <!-- e --> no comeco de linha (legado de HTML)
                '<' if self.s[self.i..].starts_with("<!--") => {
                    while let Some(c) = self.char_em() {
                        if c == '\n' {
                            break;
                        }
                        self.i += c.len_utf8();
                    }
                }
                '-' if (quebra || self.i == 0) && self.s[self.i..].starts_with("-->") => {
                    while let Some(c) = self.char_em() {
                        if c == '\n' {
                            break;
                        }
                        self.i += c.len_utf8();
                    }
                }
                _ => return Ok(quebra),
            }
        }
    }

    /// Proximo token. `regex_ok`: um '/' aqui comeca uma expressao regular.
    pub fn proximo(&mut self, regex_ok: bool) -> Result<Token, ErroLexico> {
        let quebra = self.pular()?;
        let pos = self.i as u32;
        let linha = self.linha;
        let tok = self.ler(regex_ok)?;
        Ok(Token { tok, linha_antes: quebra, pos, linha })
    }

    fn erro(&self, msg: &'static str) -> ErroLexico {
        ErroLexico { msg, linha: self.linha }
    }

    fn ler(&mut self, regex_ok: bool) -> Result<Tok, ErroLexico> {
        let Some(c) = self.char_em() else { return Ok(Tok::Fim) };
        if inicio_ident(c) || c == '\\' {
            let nome = self.identificador()?;
            if let Some(p) = PALAVRAS.iter().find(|p| **p == &*nome) {
                return Ok(Tok::Palavra(p));
            }
            return Ok(Tok::Ident(nome));
        }
        if c.is_ascii_digit() || (c == '.' && self.espiar(1).is_ascii_digit()) {
            return self.numero();
        }
        match c {
            '"' | '\'' => return self.texto(c as u8),
            '`' => {
                self.i += 1;
                return self.template();
            }
            '}' if self.templates.last() == Some(&self.chaves) => {
                self.templates.pop();
                self.i += 1;
                return match self.template()? {
                    Tok::Template(a, b, c) => Ok(Tok::TemplateMeio(a, b, c)),
                    t => Ok(t),
                };
            }
            '/' if regex_ok => return self.regex(),
            '#' if inicio_ident(self.s[self.i + 1..].chars().next().unwrap_or(' ')) => {
                self.i += 1;
                return Ok(Tok::PrivadoIdent(self.identificador()?));
            }
            _ => {}
        }
        for p in PONTUACAO {
            if self.s[self.i..].starts_with(p) {
                // "?." seguido de digito e o ternario
                if *p == "?." && self.espiar(2).is_ascii_digit() {
                    continue;
                }
                self.i += p.len();
                match *p {
                    "{" => self.chaves += 1,
                    "}" => self.chaves = self.chaves.saturating_sub(1),
                    _ => {}
                }
                return Ok(Tok::Pont(p));
            }
        }
        Err(self.erro("caractere inesperado"))
    }

    fn escape_unicode(&mut self) -> Result<char, ErroLexico> {
        // depois de "\u"
        if self.espiar(0) == b'{' {
            let fim = self.s[self.i..].find('}').ok_or(self.erro("escape inválido"))?;
            let v = u32::from_str_radix(&self.s[self.i + 1..self.i + fim], 16).map_err(|_| self.erro("escape inválido"))?;
            self.i += fim + 1;
            return char::from_u32(v).ok_or(self.erro("escape inválido"));
        }
        let h = self.s.get(self.i..self.i + 4).ok_or(self.erro("escape inválido"))?;
        let v = u32::from_str_radix(h, 16).map_err(|_| self.erro("escape inválido"))?;
        self.i += 4;
        Ok(char::from_u32(v).unwrap_or('\u{FFFD}'))
    }

    fn identificador(&mut self) -> Result<Rc<str>, ErroLexico> {
        let inicio = self.i;
        while let Some(c) = self.char_em() {
            if parte_ident(c) {
                self.i += c.len_utf8();
            } else {
                break;
            }
        }
        if self.char_em() != Some('\\') {
            return Ok(Rc::from(&self.s[inicio..self.i]));
        }
        let mut nome = String::from(&self.s[inicio..self.i]);
        loop {
            match self.char_em() {
                Some('\\') if self.espiar(1) == b'u' => {
                    self.i += 2;
                    nome.push(self.escape_unicode()?);
                }
                Some(c) if parte_ident(c) => {
                    nome.push(c);
                    self.i += c.len_utf8();
                }
                _ => break,
            }
        }
        Ok(Rc::from(nome.as_str()))
    }

    fn numero(&mut self) -> Result<Tok, ErroLexico> {
        let inicio = self.i;
        let base = if self.espiar(0) == b'0' {
            match self.espiar(1) | 0x20 {
                b'x' => 16,
                b'o' => 8,
                b'b' => 2,
                _ => 10,
            }
        } else {
            10
        };
        if base != 10 {
            self.i += 2;
            let ini = self.i;
            while self.espiar(0).is_ascii_alphanumeric() || self.espiar(0) == b'_' {
                self.i += 1;
            }
            let digitos: String = self.s[ini..self.i].chars().filter(|&c| c != '_').collect();
            if let Some(d) = digitos.strip_suffix('n') {
                return Ok(Tok::BigInt(Rc::from(d)));
            }
            let mut v = 0f64;
            for c in digitos.chars() {
                let d = c.to_digit(base).ok_or(self.erro("número inválido"))?;
                v = v * base as f64 + d as f64;
            }
            return Ok(Tok::Num(v));
        }
        // octal legado (0777)
        if self.espiar(0) == b'0' && self.espiar(1).is_ascii_digit() {
            let ini = self.i;
            while self.espiar(0).is_ascii_digit() {
                self.i += 1;
            }
            let t = &self.s[ini..self.i];
            if t.bytes().all(|b| b < b'8') {
                let mut v = 0f64;
                for c in t.bytes() {
                    v = v * 8.0 + (c - b'0') as f64;
                }
                return Ok(Tok::Num(v));
            }
            return Ok(Tok::Num(t.parse().unwrap_or(0.0)));
        }
        while self.espiar(0).is_ascii_digit() || self.espiar(0) == b'_' {
            self.i += 1;
        }
        if self.espiar(0) == b'n' {
            let d: String = self.s[inicio..self.i].chars().filter(|&c| c != '_').collect();
            self.i += 1;
            return Ok(Tok::BigInt(Rc::from(d.as_str())));
        }
        if self.espiar(0) == b'.' {
            self.i += 1;
            while self.espiar(0).is_ascii_digit() || self.espiar(0) == b'_' {
                self.i += 1;
            }
        }
        if self.espiar(0) | 0x20 == b'e' {
            let k = if matches!(self.espiar(1), b'+' | b'-') { 2 } else { 1 };
            if self.espiar(k).is_ascii_digit() {
                self.i += k;
                while self.espiar(0).is_ascii_digit() {
                    self.i += 1;
                }
            }
        }
        let t: String = self.s[inicio..self.i].chars().filter(|&c| c != '_').collect();
        Ok(Tok::Num(t.parse().unwrap_or(f64::NAN)))
    }

    /// Escape de texto depois do '\\' (devolve None para continuacao de linha).
    fn escape(&mut self, s: &mut String) -> Result<(), ErroLexico> {
        let c = self.char_em().ok_or(self.erro("texto sem fim"))?;
        self.i += c.len_utf8();
        match c {
            'n' => s.push('\n'),
            't' => s.push('\t'),
            'r' => s.push('\r'),
            'b' => s.push('\u{8}'),
            'f' => s.push('\u{C}'),
            'v' => s.push('\u{B}'),
            '0' if !self.espiar(0).is_ascii_digit() => s.push('\0'),
            'x' => {
                let h = self.s.get(self.i..self.i + 2).ok_or(self.erro("escape inválido"))?;
                let v = u32::from_str_radix(h, 16).map_err(|_| self.erro("escape inválido"))?;
                self.i += 2;
                s.push(char::from_u32(v).unwrap_or('\u{FFFD}'));
            }
            'u' => {
                let c = self.escape_unicode()?;
                // par substituto 😀
                if (0xD800..0xDC00).contains(&(c as u32)) {
                    s.push(c);
                } else {
                    s.push(c);
                }
            }
            '\r' => {
                if self.espiar(0) == b'\n' {
                    self.i += 1;
                }
                self.linha += 1;
            }
            '\n' | '\u{2028}' | '\u{2029}' => self.linha += 1,
            c if c.is_ascii_digit() => {
                // octal legado
                let mut v = c.to_digit(8).unwrap_or(0);
                for _ in 0..2 {
                    match (self.espiar(0) as char).to_digit(8) {
                        Some(d) if v * 8 + d < 256 => {
                            v = v * 8 + d;
                            self.i += 1;
                        }
                        _ => break,
                    }
                }
                s.push(char::from_u32(v).unwrap_or('\0'));
            }
            c => s.push(c),
        }
        Ok(())
    }

    fn texto(&mut self, q: u8) -> Result<Tok, ErroLexico> {
        self.i += 1;
        let inicio = self.i;
        // caminho rapido: sem escapes
        while self.i < self.b.len() {
            let c = self.b[self.i];
            if c == q {
                let t = Rc::from(&self.s[inicio..self.i]);
                self.i += 1;
                return Ok(Tok::Texto(t));
            }
            if c == b'\\' || c == b'\n' {
                break;
            }
            self.i += 1;
        }
        let mut s = String::from(&self.s[inicio..self.i]);
        loop {
            let c = self.char_em().ok_or(self.erro("texto sem fim"))?;
            if c as u32 == q as u32 {
                self.i += 1;
                return Ok(Tok::Texto(Rc::from(s.as_str())));
            }
            match c {
                '\\' => {
                    self.i += 1;
                    self.escape(&mut s)?;
                }
                '\n' => return Err(self.erro("texto sem fim")),
                c => {
                    s.push(c);
                    self.i += c.len_utf8();
                }
            }
        }
    }

    /// Le um pedaco de template ate ` ou ${.
    fn template(&mut self) -> Result<Tok, ErroLexico> {
        let mut cozido = String::new();
        let mut valido = true;
        let inicio = self.i;
        loop {
            let c = self.char_em().ok_or(self.erro("template sem fim"))?;
            match c {
                '`' => {
                    let cru = Rc::from(&self.s[inicio..self.i]);
                    self.i += 1;
                    return Ok(Tok::Template(valido.then(|| Rc::from(cozido.as_str())), cru, true));
                }
                '$' if self.espiar(1) == b'{' => {
                    let cru = Rc::from(&self.s[inicio..self.i]);
                    self.i += 2;
                    self.templates.push(self.chaves);
                    return Ok(Tok::Template(valido.then(|| Rc::from(cozido.as_str())), cru, false));
                }
                '\\' => {
                    self.i += 1;
                    if self.escape(&mut cozido).is_err() {
                        valido = false;
                    }
                }
                '\r' => {
                    self.i += 1;
                    if self.espiar(0) == b'\n' {
                        self.i += 1;
                    }
                    cozido.push('\n');
                    self.linha += 1;
                }
                c => {
                    if c == '\n' {
                        self.linha += 1;
                    }
                    cozido.push(c);
                    self.i += c.len_utf8();
                }
            }
        }
    }

    fn regex(&mut self) -> Result<Tok, ErroLexico> {
        self.i += 1;
        let inicio = self.i;
        let mut classe = false;
        loop {
            let c = self.char_em().ok_or(self.erro("regex sem fim"))?;
            match c {
                '\\' => {
                    self.i += 1;
                    let c2 = self.char_em().ok_or(self.erro("regex sem fim"))?;
                    self.i += c2.len_utf8();
                    continue;
                }
                '[' => classe = true,
                ']' => classe = false,
                '/' if !classe => break,
                '\n' => return Err(self.erro("regex sem fim")),
                _ => {}
            }
            self.i += c.len_utf8();
        }
        let corpo = Rc::from(&self.s[inicio..self.i]);
        self.i += 1;
        let ini_f = self.i;
        while self.char_em().is_some_and(parte_ident) {
            self.i += 1;
        }
        Ok(Tok::Regex(corpo, Rc::from(&self.s[ini_f..self.i])))
    }

    pub fn linha(&self) -> u32 {
        self.linha
    }
}
