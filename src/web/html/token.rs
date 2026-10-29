//! Tokenizador do HTML5 (WHATWG, secao 13.2.5). Anda sobre os bytes de um
//! texto UTF-8 ja com as quebras de linha normalizadas; tags e atributos
//! sao ASCII, entao cortar nos delimitadores nunca parte um caractere.
//! Texto corrido sai num token so (com as referencias de caractere ja
//! trocadas), o que poupa alocacao.
//!
//! O construtor da arvore (arvore.rs) muda o `estado` quando entra em
//! <title>/<textarea> (RCDATA), <style>/<xmp>/<iframe>... (RAWTEXT),
//! <script> e <plaintext>.

use alloc::string::String;
use alloc::vec::Vec;

use super::codificacao::WIN1252;
use super::entidades::ENTIDADES;
use crate::web::atomo::Atomo;
use crate::web::dom::Atributo;

#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    Doctype { nome: String, publico: Option<String>, sistema: Option<String>, quirks: bool },
    Inicio { nome: Atomo, atributos: Vec<Atributo>, auto_fecha: bool },
    Fim { nome: Atomo },
    Texto(String),
    Comentario(String),
    FimDoArquivo,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Estado {
    Dados,
    Rcdata,
    Rawtext,
    Script,
    Plaintext,
}

pub struct Tokenizador {
    s: String,
    i: usize,
    pub estado: Estado,
    /// Nome da ultima tag de inicio (o fim "apropriado" do RCDATA/RAWTEXT).
    ultima: String,
    /// <![CDATA[ vale (so dentro de SVG/MathML).
    pub cdata: bool,
}

fn espaco(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\x0C' | b'\r')
}

impl Tokenizador {
    pub fn novo(texto: &str) -> Tokenizador {
        // CR LF e CR sozinho viram LF
        let s = if texto.contains('\r') { texto.replace("\r\n", "\n").replace('\r', "\n") } else { String::from(texto) };
        Tokenizador { s, i: 0, estado: Estado::Dados, ultima: String::new(), cdata: false }
    }

    fn b(&self) -> &[u8] {
        self.s.as_bytes()
    }

    fn em(&self, k: usize) -> Option<u8> {
        self.b().get(self.i + k).copied()
    }

    /// Os proximos bytes sao `p` (sem diferenciar maiusculas)?
    fn comeca(&self, p: &[u8]) -> bool {
        let b = self.b();
        b.len() >= self.i + p.len() && b[self.i..self.i + p.len()].eq_ignore_ascii_case(p)
    }

    pub fn proximo(&mut self) -> Token {
        if self.i >= self.s.len() {
            return Token::FimDoArquivo;
        }
        match self.estado {
            Estado::Dados => self.dados(),
            Estado::Rcdata => self.texto_ate_fim(true),
            Estado::Rawtext | Estado::Script => self.texto_ate_fim(false),
            Estado::Plaintext => {
                let t = String::from(&self.s[self.i..]);
                self.i = self.s.len();
                Token::Texto(t.replace('\0', "\u{FFFD}"))
            }
        }
    }

    // ---------------------------------------------------------- texto

    fn dados(&mut self) -> Token {
        if self.em(0) == Some(b'<') {
            if let Some(t) = self.tag() {
                return t;
            }
            // "<" que nao abre nada vira texto
        }
        let mut texto = String::new();
        let mut primeiro = true;
        while self.i < self.s.len() {
            let c = self.b()[self.i];
            match c {
                b'<' if !primeiro => break,
                b'<' => {
                    texto.push('<');
                    self.i += 1;
                }
                b'&' => self.referencia(&mut texto, false),
                0 => self.i += 1, // NUL no texto e ignorado
                _ => {
                    let fim = self.b()[self.i..].iter().position(|&x| x == b'<' || x == b'&' || x == 0).map_or(self.s.len(), |p| self.i + p);
                    texto.push_str(&self.s[self.i..fim]);
                    self.i = fim;
                }
            }
            primeiro = false;
        }
        Token::Texto(texto)
    }

    /// Texto ate "</ultima" (RCDATA decodifica as referencias).
    fn texto_ate_fim(&mut self, referencias: bool) -> Token {
        let inicio = self.i;
        let mut j = self.i;
        let b = self.b();
        let n = self.ultima.len();
        let fim = loop {
            match b[j..].iter().position(|&x| x == b'<') {
                None => break b.len(),
                Some(p) => {
                    let k = j + p;
                    let nome_ok = b.len() >= k + 2 + n
                        && b[k + 1] == b'/'
                        && b[k + 2..k + 2 + n].eq_ignore_ascii_case(self.ultima.as_bytes());
                    let depois = b.get(k + 2 + n).copied();
                    if nome_ok && (depois.is_none() || matches!(depois, Some(c) if espaco(c) || c == b'/' || c == b'>')) {
                        break k;
                    }
                    j = k + 1;
                }
            }
        };
        if fim == inicio {
            // estamos no "</nome": volta ao normal e le a tag de fim
            self.estado = Estado::Dados;
            return self.dados();
        }
        self.i = fim;
        let bruto = &self.s[inicio..fim];
        let mut t = String::with_capacity(bruto.len());
        if referencias && bruto.contains('&') {
            let salvo = self.i;
            self.i = inicio;
            while self.i < fim {
                if self.b()[self.i] == b'&' {
                    self.referencia(&mut t, false);
                } else {
                    let prox = self.b()[self.i..fim].iter().position(|&x| x == b'&').map_or(fim, |p| self.i + p);
                    t.push_str(&self.s[self.i..prox]);
                    self.i = prox;
                }
            }
            self.i = salvo;
        } else {
            t.push_str(bruto);
        }
        if t.contains('\0') {
            t = t.replace('\0', "\u{FFFD}");
        }
        Token::Texto(t)
    }

    // ------------------------------------------------- referencias (&...)

    /// Le uma referencia de caractere em self.i (que aponta para '&') e
    /// poe o resultado em `saida`.
    fn referencia(&mut self, saida: &mut String, em_atributo: bool) {
        self.i += 1; // '&'
        match self.em(0) {
            Some(b'#') => {
                let hexa = matches!(self.em(1), Some(b'x' | b'X'));
                let comeco = self.i + if hexa { 2 } else { 1 };
                let b = self.b();
                let mut j = comeco;
                let mut valor: u32 = 0;
                while j < b.len() {
                    let d = match (hexa, b[j]) {
                        (_, c @ b'0'..=b'9') => (c - b'0') as u32,
                        (true, c @ b'a'..=b'f') => (c - b'a' + 10) as u32,
                        (true, c @ b'A'..=b'F') => (c - b'A' + 10) as u32,
                        _ => break,
                    };
                    valor = valor.saturating_mul(if hexa { 16 } else { 10 }).saturating_add(d);
                    j += 1;
                }
                if j == comeco {
                    saida.push('&'); // "&#" sem numero: texto
                    return;
                }
                self.i = j;
                if self.em(0) == Some(b';') {
                    self.i += 1;
                }
                let c = match valor {
                    0 | 0xD800..=0xDFFF => '\u{FFFD}',
                    0x80..=0x9F => char::from_u32(WIN1252[(valor - 0x80) as usize] as u32).unwrap_or('\u{FFFD}'),
                    v => char::from_u32(v).unwrap_or('\u{FFFD}'),
                };
                saida.push(c);
            }
            Some(c) if c.is_ascii_alphanumeric() => {
                let b = self.b();
                let mut j = self.i;
                while j < b.len() && j - self.i < 32 && b[j].is_ascii_alphanumeric() {
                    j += 1;
                }
                if b.get(j) == Some(&b';') {
                    j += 1;
                }
                // o nome mais longo que existe na tabela
                let mut n = j - self.i;
                while n > 0 {
                    let nome = &self.s[self.i..self.i + n];
                    if let Ok(k) = ENTIDADES.binary_search_by(|(e, _)| e.as_bytes().cmp(nome.as_bytes())) {
                        let depois = b.get(self.i + n).copied();
                        let antigo = !nome.ends_with(';');
                        if em_atributo && antigo && matches!(depois, Some(x) if x == b'=' || x.is_ascii_alphanumeric()) {
                            break;
                        }
                        saida.push_str(ENTIDADES[k].1);
                        self.i += n;
                        return;
                    }
                    n -= 1;
                }
                saida.push('&');
            }
            _ => saida.push('&'),
        }
    }

    // -------------------------------------------------------------- tags

    /// Em self.i ha um '<'. None = nao e tag (o '<' e texto).
    fn tag(&mut self) -> Option<Token> {
        let c1 = self.em(1);
        match c1 {
            Some(b'!') => Some(self.declaracao()),
            Some(b'/') => {
                match self.em(2) {
                    Some(c) if c.is_ascii_alphabetic() => {
                        self.i += 2;
                        let nome = self.nome_tag();
                        let _ = self.atributos();
                        Some(Token::Fim { nome })
                    }
                    Some(b'>') => {
                        self.i += 3; // "</>" some
                        Some(Token::Texto(String::new()))
                    }
                    None => None,
                    _ => {
                        self.i += 2;
                        Some(self.comentario_falso())
                    }
                }
            }
            Some(c) if c.is_ascii_alphabetic() => {
                self.i += 1;
                let nome = self.nome_tag();
                let Some((atributos, auto_fecha)) = self.atributos() else {
                    return Some(Token::FimDoArquivo); // o arquivo acabou dentro da tag
                };
                self.ultima = String::from(nome.texto());
                Some(Token::Inicio { nome, atributos, auto_fecha })
            }
            Some(b'?') => {
                self.i += 1;
                Some(self.comentario_falso())
            }
            _ => None,
        }
    }

    fn nome_tag(&mut self) -> Atomo {
        let b = self.b();
        let inicio = self.i;
        let mut j = self.i;
        while j < b.len() && !espaco(b[j]) && b[j] != b'/' && b[j] != b'>' {
            j += 1;
        }
        self.i = j;
        let nome = &self.s[inicio..j];
        if nome.contains('\0') {
            return Atomo::minusculo(&nome.replace('\0', "\u{FFFD}"));
        }
        Atomo::minusculo(nome)
    }

    fn pular_espacos(&mut self) {
        while self.em(0).is_some_and(espaco) {
            self.i += 1;
        }
    }

    /// Atributos ate o '>'. None = o arquivo acabou dentro da tag (a tag
    /// e descartada).
    fn atributos(&mut self) -> Option<(Vec<Atributo>, bool)> {
        let mut lista: Vec<Atributo> = Vec::new();
        loop {
            self.pular_espacos();
            match self.em(0)? {
                b'>' => {
                    self.i += 1;
                    return Some((lista, false));
                }
                b'/' => {
                    self.i += 1;
                    if self.em(0) == Some(b'>') {
                        self.i += 1;
                        return Some((lista, true));
                    }
                    continue;
                }
                _ => {}
            }
            // nome (o primeiro caractere pode ser '=')
            let inicio = self.i;
            self.i += 1;
            while let Some(c) = self.em(0) {
                if espaco(c) || c == b'/' || c == b'>' || c == b'=' {
                    break;
                }
                self.i += 1;
            }
            let nome = Atomo::minusculo(&self.s[inicio..self.i]);
            self.pular_espacos();
            let mut valor = String::new();
            if self.em(0) == Some(b'=') {
                self.i += 1;
                self.pular_espacos();
                match self.em(0)? {
                    q @ (b'"' | b'\'') => {
                        self.i += 1;
                        loop {
                            let c = self.em(0)?;
                            if c == q {
                                self.i += 1;
                                break;
                            }
                            if c == b'&' {
                                self.referencia(&mut valor, true);
                            } else {
                                let b = self.b();
                                let fim = b[self.i..].iter().position(|&x| x == q || x == b'&').map_or(b.len(), |p| self.i + p);
                                valor.push_str(&self.s[self.i..fim]);
                                self.i = fim;
                            }
                        }
                    }
                    b'>' => {}
                    _ => {
                        while let Some(c) = self.em(0) {
                            if espaco(c) || c == b'>' {
                                break;
                            }
                            if c == b'&' {
                                self.referencia(&mut valor, true);
                            } else {
                                let fim = self.i + self.s[self.i..].chars().next().map_or(1, |ch| ch.len_utf8());
                                valor.push_str(&self.s[self.i..fim]);
                                self.i = fim;
                            }
                        }
                    }
                }
            }
            if valor.contains('\0') {
                valor = valor.replace('\0', "\u{FFFD}");
            }
            if !lista.iter().any(|a| a.nome == nome) {
                lista.push(Atributo { nome, valor });
            }
        }
    }

    // ------------------------------------------------ comentarios e afins

    /// "<!..." em self.i.
    fn declaracao(&mut self) -> Token {
        self.i += 2;
        if self.comeca(b"--") {
            self.i += 2;
            return self.comentario();
        }
        if self.comeca(b"doctype") {
            self.i += 7;
            return self.doctype();
        }
        if self.cdata && self.s[self.i..].starts_with("[CDATA[") {
            self.i += 7;
            let fim = self.s[self.i..].find("]]>").map_or(self.s.len(), |p| self.i + p);
            let t = String::from(&self.s[self.i..fim]);
            self.i = (fim + 3).min(self.s.len());
            return Token::Texto(t);
        }
        self.comentario_falso()
    }

    fn comentario(&mut self) -> Token {
        // "<!-->" e "<!--->" sao comentarios vazios
        if self.em(0) == Some(b'>') {
            self.i += 1;
            return Token::Comentario(String::new());
        }
        if self.comeca(b"->") {
            self.i += 2;
            return Token::Comentario(String::new());
        }
        // o primeiro "--" seguido de ">" ou "!>" (uma passada so: procurar
        // "--!>" no resto inteiro a cada comentario era quadratico)
        let resto = &self.s[self.i..];
        let b = resto.as_bytes();
        let mut j = 0;
        let (fim, pulo) = loop {
            match b[j..].iter().position(|&c| c == b'-') {
                None => break (resto.len(), 0),
                Some(p) => {
                    let k = j + p;
                    if b.get(k + 1) == Some(&b'-') {
                        if b.get(k + 2) == Some(&b'>') {
                            break (k, 3);
                        }
                        if b.get(k + 2) == Some(&b'!') && b.get(k + 3) == Some(&b'>') {
                            break (k, 4);
                        }
                    }
                    j = k + 1;
                }
            }
        };
        let t = String::from(&resto[..fim]);
        self.i += fim + pulo;
        Token::Comentario(t)
    }

    fn comentario_falso(&mut self) -> Token {
        let resto = &self.s[self.i..];
        let fim = resto.find('>').unwrap_or(resto.len());
        let t = String::from(&resto[..fim]);
        self.i = (self.i + fim + 1).min(self.s.len());
        Token::Comentario(t)
    }

    fn doctype(&mut self) -> Token {
        let resto = &self.s[self.i..];
        let fim = resto.find('>').unwrap_or(resto.len());
        let dentro = &resto[..fim];
        let fechou = fim < resto.len();
        self.i = (self.i + fim + 1).min(self.s.len());
        let mut partes = dentro.trim_start();
        let nome_fim = partes.find(|c: char| c.is_ascii_whitespace()).unwrap_or(partes.len());
        let nome = partes[..nome_fim].to_ascii_lowercase();
        partes = partes[nome_fim..].trim_start();
        let mut publico = None;
        let mut sistema = None;
        let mut quirks = !fechou || nome.is_empty();
        let citado = |p: &str| -> Option<(String, usize)> {
            let p2 = p.trim_start();
            let pulo = p.len() - p2.len();
            let q = p2.chars().next()?;
            if q != '"' && q != '\'' {
                return None;
            }
            let f = p2[1..].find(q).unwrap_or(p2.len() - 1);
            Some((String::from(&p2[1..1 + f]), pulo + f + 2))
        };
        if partes.len() >= 6 && partes[..6].eq_ignore_ascii_case("public") {
            match citado(&partes[6..]) {
                Some((id, n)) => {
                    publico = Some(id);
                    let resto = &partes[(6 + n).min(partes.len())..];
                    sistema = citado(resto).map(|(s, _)| s);
                }
                None => quirks = true,
            }
        } else if partes.len() >= 6 && partes[..6].eq_ignore_ascii_case("system") {
            match citado(&partes[6..]) {
                Some((id, _)) => sistema = Some(id),
                None => quirks = true,
            }
        }
        Token::Doctype { nome, publico, sistema, quirks }
    }
}
