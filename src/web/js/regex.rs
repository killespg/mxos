//! Expressoes regulares do JavaScript: compilacao para uma arvore e
//! casamento por retrocesso com continuacoes. Repeticao de um caractere
//! simples (a*, \d+, [^"]*) conta de uma vez e volta sem recursao por
//! caractere.

use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Debug)]
enum No {
    Char(char),
    Qualquer,
    Classe(Vec<(char, char)>, bool),
    Inicio,
    Fim,
    Limite(bool),
    Grupo(Box<Vec<Vec<No>>>, Option<usize>),
    Rep(Box<No>, u32, u32, bool),
    Ref(usize),
    Olhar(Box<Vec<Vec<No>>>, bool, bool),
}

#[derive(Debug)]
pub struct Regex {
    alt: Vec<Vec<No>>,
    pub grupos: usize,
    pub nomes: Vec<(Rc<str>, usize)>,
    pub fonte: Rc<str>,
    pub flags: Rc<str>,
    pub global: bool,
    pub sem_caixa: bool,
    pub multilinha: bool,
    pub ponto_tudo: bool,
    pub grudado: bool,
    pub unicode: bool,
}

const DIGITOS: &[(char, char)] = &[('0', '9')];
const PALAVRA: &[(char, char)] = &[('0', '9'), ('A', 'Z'), ('_', '_'), ('a', 'z')];
const ESPACOS: &[(char, char)] = &[
    ('\t', '\r'),
    (' ', ' '),
    ('\u{A0}', '\u{A0}'),
    ('\u{1680}', '\u{1680}'),
    ('\u{2000}', '\u{200A}'),
    ('\u{2028}', '\u{2029}'),
    ('\u{202F}', '\u{202F}'),
    ('\u{205F}', '\u{205F}'),
    ('\u{3000}', '\u{3000}'),
    ('\u{FEFF}', '\u{FEFF}'),
];

fn negar(f: &[(char, char)]) -> Vec<(char, char)> {
    let mut v = Vec::new();
    let mut ini = '\0' as u32;
    let mut fs: Vec<(char, char)> = f.to_vec();
    fs.sort();
    for (a, b) in fs {
        if (a as u32) > ini {
            if let (Some(x), Some(y)) = (char::from_u32(ini), char::from_u32(a as u32 - 1)) {
                v.push((x, y));
            }
        }
        ini = ini.max(b as u32 + 1);
    }
    if ini <= 0x10FFFF {
        if let Some(x) = char::from_u32(ini) {
            v.push((x, '\u{10FFFF}'));
        }
    }
    v
}

struct Ler<'a> {
    c: Vec<char>,
    i: usize,
    grupos: usize,
    nomes: Vec<(Rc<str>, usize)>,
    _s: &'a str,
}

impl Ler<'_> {
    fn espiar(&self) -> Option<char> {
        self.c.get(self.i).copied()
    }
    fn pegar(&mut self) -> Option<char> {
        let c = self.c.get(self.i).copied();
        self.i += 1;
        c
    }

    fn alternativas(&mut self) -> Result<Vec<Vec<No>>, &'static str> {
        let mut alts = alloc::vec![self.sequencia()?];
        while self.espiar() == Some('|') {
            self.i += 1;
            alts.push(self.sequencia()?);
        }
        Ok(alts)
    }

    fn sequencia(&mut self) -> Result<Vec<No>, &'static str> {
        let mut v = Vec::new();
        while let Some(c) = self.espiar() {
            if c == '|' || c == ')' {
                break;
            }
            let atomo = self.atomo()?;
            let atomo = self.quantificador(atomo)?;
            v.push(atomo);
        }
        Ok(v)
    }

    fn numero(&mut self) -> Option<u32> {
        let ini = self.i;
        while self.espiar().is_some_and(|c| c.is_ascii_digit()) {
            self.i += 1;
        }
        if self.i == ini {
            return None;
        }
        self.c[ini..self.i].iter().collect::<String>().parse().ok()
    }

    fn quantificador(&mut self, a: No) -> Result<No, &'static str> {
        let (min, max) = match self.espiar() {
            Some('*') => (0, u32::MAX),
            Some('+') => (1, u32::MAX),
            Some('?') => (0, 1),
            Some('{') => {
                let salvo = self.i;
                self.i += 1;
                match self.numero() {
                    Some(m) => {
                        let max = if self.espiar() == Some(',') {
                            self.i += 1;
                            self.numero().unwrap_or(u32::MAX)
                        } else {
                            m
                        };
                        if self.espiar() != Some('}') {
                            self.i = salvo;
                            return Ok(a);
                        }
                        self.i -= 0;
                        (m, max)
                    }
                    None => {
                        self.i = salvo;
                        return Ok(a);
                    }
                }
            }
            _ => return Ok(a),
        };
        self.i += 1;
        let guloso = if self.espiar() == Some('?') {
            self.i += 1;
            false
        } else {
            true
        };
        Ok(No::Rep(Box::new(a), min, max, guloso))
    }

    fn escape_char(&mut self, c: char) -> Result<char, &'static str> {
        Ok(match c {
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            'v' => '\u{B}',
            'f' => '\u{C}',
            '0' => '\0',
            'x' => {
                let h: String = self.c.get(self.i..self.i + 2).ok_or("escape")?.iter().collect();
                self.i += 2;
                char::from_u32(u32::from_str_radix(&h, 16).map_err(|_| "escape")?).ok_or("escape")?
            }
            'u' => {
                if self.espiar() == Some('{') {
                    let fim = self.c[self.i..].iter().position(|&x| x == '}').ok_or("escape")?;
                    let h: String = self.c[self.i + 1..self.i + fim].iter().collect();
                    self.i += fim + 1;
                    char::from_u32(u32::from_str_radix(&h, 16).map_err(|_| "escape")?).ok_or("escape")?
                } else {
                    let h: String = self.c.get(self.i..self.i + 4).ok_or("escape")?.iter().collect();
                    self.i += 4;
                    let v = u32::from_str_radix(&h, 16).map_err(|_| "escape")?;
                    // par substituto 😀
                    if (0xD800..0xDC00).contains(&v) && self.c.get(self.i) == Some(&'\\') && self.c.get(self.i + 1) == Some(&'u') {
                        let h2: String = self.c.get(self.i + 2..self.i + 6).unwrap_or(&[]).iter().collect();
                        if let Ok(v2) = u32::from_str_radix(&h2, 16) {
                            if (0xDC00..0xE000).contains(&v2) {
                                self.i += 6;
                                return char::from_u32(0x10000 + ((v - 0xD800) << 10) + (v2 - 0xDC00)).ok_or("escape");
                            }
                        }
                    }
                    char::from_u32(v).unwrap_or('\u{FFFD}')
                }
            }
            'c' => {
                let l = self.pegar().ok_or("escape")?;
                char::from_u32((l as u32) % 32).unwrap_or('\0')
            }
            c => c,
        })
    }

    /// Escape de classe (\d, \w, \s...) como faixas.
    fn classe_escape(c: char) -> Option<Vec<(char, char)>> {
        Some(match c {
            'd' => DIGITOS.to_vec(),
            'D' => negar(DIGITOS),
            'w' => PALAVRA.to_vec(),
            'W' => negar(PALAVRA),
            's' => ESPACOS.to_vec(),
            'S' => negar(ESPACOS),
            _ => return None,
        })
    }

    fn atomo(&mut self) -> Result<No, &'static str> {
        let c = self.pegar().ok_or("regex vazia")?;
        Ok(match c {
            '.' => No::Qualquer,
            '^' => No::Inicio,
            '$' => No::Fim,
            '(' => {
                let mut captura = None;
                if self.espiar() == Some('?') {
                    self.i += 1;
                    match self.pegar() {
                        Some(':') => {}
                        Some('=') | Some('!') => {
                            let neg = self.c[self.i - 1] == '!';
                            let alt = self.alternativas()?;
                            if self.pegar() != Some(')') {
                                return Err("parêntese sem fim");
                            }
                            return Ok(No::Olhar(Box::new(alt), true, neg));
                        }
                        Some('<') if matches!(self.espiar(), Some('=') | Some('!')) => {
                            let neg = self.pegar() == Some('!');
                            let alt = self.alternativas()?;
                            if self.pegar() != Some(')') {
                                return Err("parêntese sem fim");
                            }
                            return Ok(No::Olhar(Box::new(alt), false, neg));
                        }
                        Some('<') => {
                            let fim = self.c[self.i..].iter().position(|&x| x == '>').ok_or("nome de grupo")?;
                            let nome: String = self.c[self.i..self.i + fim].iter().collect();
                            self.i += fim + 1;
                            self.grupos += 1;
                            captura = Some(self.grupos);
                            self.nomes.push((Rc::from(nome.as_str()), self.grupos));
                        }
                        _ => return Err("grupo inválido"),
                    }
                } else {
                    self.grupos += 1;
                    captura = Some(self.grupos);
                }
                let alt = self.alternativas()?;
                if self.pegar() != Some(')') {
                    return Err("parêntese sem fim");
                }
                No::Grupo(Box::new(alt), captura)
            }
            '[' => {
                let mut neg = false;
                if self.espiar() == Some('^') {
                    neg = true;
                    self.i += 1;
                }
                let mut faixas = Vec::new();
                let mut primeiro = true;
                loop {
                    let c = self.pegar().ok_or("classe sem fim")?;
                    if c == ']' && !primeiro {
                        break;
                    }
                    if c == ']' && primeiro {
                        // "[]" casa nada; "[^]" casa tudo
                        break;
                    }
                    primeiro = false;
                    let ini = if c == '\\' {
                        let e = self.pegar().ok_or("classe")?;
                        if let Some(f) = Self::classe_escape(e) {
                            faixas.extend(f);
                            continue;
                        }
                        if e == 'b' { '\u{8}' } else { self.escape_char(e)? }
                    } else {
                        c
                    };
                    if self.espiar() == Some('-') && self.c.get(self.i + 1).is_some_and(|&x| x != ']') {
                        self.i += 1;
                        let f = self.pegar().ok_or("classe")?;
                        let fim = if f == '\\' {
                            let e = self.pegar().ok_or("classe")?;
                            if Self::classe_escape(e).is_some() {
                                faixas.push((ini, ini));
                                faixas.push(('-', '-'));
                                faixas.extend(Self::classe_escape(e).unwrap_or_default());
                                continue;
                            }
                            self.escape_char(e)?
                        } else {
                            f
                        };
                        faixas.push((ini.min(fim), ini.max(fim)));
                    } else {
                        faixas.push((ini, ini));
                    }
                }
                No::Classe(faixas, neg)
            }
            '\\' => {
                let e = self.pegar().ok_or("\\ no fim")?;
                if let Some(f) = Self::classe_escape(e) {
                    return Ok(No::Classe(f, false));
                }
                match e {
                    'b' => No::Limite(true),
                    'B' => No::Limite(false),
                    '1'..='9' => {
                        self.i -= 1;
                        let n = self.numero().unwrap_or(1) as usize;
                        No::Ref(n)
                    }
                    'k' if self.espiar() == Some('<') => {
                        let fim = self.c[self.i..].iter().position(|&x| x == '>').ok_or("\\k")?;
                        let nome: String = self.c[self.i + 1..self.i + fim].iter().collect();
                        self.i += fim + 1;
                        let n = self.nomes.iter().find(|(x, _)| &**x == nome).map_or(0, |x| x.1);
                        No::Ref(n)
                    }
                    e => No::Char(self.escape_char(e)?),
                }
            }
            c => No::Char(c),
        })
    }
}

type Caps = Vec<Option<(usize, usize)>>;

struct Casador<'a> {
    t: &'a [char],
    r: &'a Regex,
    passos: u32,
}

fn minuscula(c: char) -> char {
    if c.is_ascii() { c.to_ascii_lowercase() } else { c.to_lowercase().next().unwrap_or(c) }
}

fn eh_palavra(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

impl Casador<'_> {
    fn um_char(&self, n: &No, c: char) -> bool {
        match n {
            No::Char(x) => *x == c || (self.r.sem_caixa && minuscula(*x) == minuscula(c)),
            No::Qualquer => self.r.ponto_tudo || !matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}'),
            No::Classe(f, neg) => {
                let dentro = f.iter().any(|&(a, b)| (a..=b).contains(&c))
                    || (self.r.sem_caixa && {
                        let l = minuscula(c);
                        let u = c.to_uppercase().next().unwrap_or(c);
                        f.iter().any(|&(a, b)| (a..=b).contains(&l) || (a..=b).contains(&u))
                    });
                dentro != *neg
            }
            _ => false,
        }
    }

    fn simples(n: &No) -> bool {
        matches!(n, No::Char(_) | No::Qualquer | No::Classe(..))
    }

    fn alt(&mut self, alts: &[Vec<No>], pos: usize, caps: &mut Caps, k: &mut dyn FnMut(usize, &mut Caps, &mut Self) -> bool) -> bool {
        for seq in alts {
            if self.seq(seq, pos, caps, k) {
                return true;
            }
        }
        false
    }

    fn seq(&mut self, seq: &[No], pos: usize, caps: &mut Caps, k: &mut dyn FnMut(usize, &mut Caps, &mut Self) -> bool) -> bool {
        self.passos += 1;
        if self.passos > 5_000_000 {
            return false;
        }
        let Some((n, resto)) = seq.split_first() else { return k(pos, caps, self) };
        match n {
            No::Char(_) | No::Qualquer | No::Classe(..) => {
                if pos < self.t.len() && self.um_char(n, self.t[pos]) {
                    self.seq(resto, pos + 1, caps, k)
                } else {
                    false
                }
            }
            No::Inicio => {
                if pos == 0 || (self.r.multilinha && matches!(self.t[pos - 1], '\n' | '\r')) {
                    self.seq(resto, pos, caps, k)
                } else {
                    false
                }
            }
            No::Fim => {
                if pos == self.t.len() || (self.r.multilinha && matches!(self.t[pos], '\n' | '\r')) {
                    self.seq(resto, pos, caps, k)
                } else {
                    false
                }
            }
            No::Limite(sim) => {
                let a = pos > 0 && eh_palavra(self.t[pos - 1]);
                let b = pos < self.t.len() && eh_palavra(self.t[pos]);
                if (a != b) == *sim { self.seq(resto, pos, caps, k) } else { false }
            }
            No::Ref(g) => {
                let alvo = caps.get(*g).copied().flatten();
                match alvo {
                    None => self.seq(resto, pos, caps, k),
                    Some((a, b)) => {
                        let len = b - a;
                        if pos + len > self.t.len() {
                            return false;
                        }
                        for i in 0..len {
                            let (x, y) = (self.t[a + i], self.t[pos + i]);
                            if x != y && !(self.r.sem_caixa && minuscula(x) == minuscula(y)) {
                                return false;
                            }
                        }
                        self.seq(resto, pos + len, caps, k)
                    }
                }
            }
            No::Olhar(alt, adiante, negado) => {
                let mut c2 = caps.clone();
                let ok = if *adiante {
                    self.alt(alt, pos, &mut c2, &mut |_, _, _| true)
                } else {
                    // para tras: tenta todos os inicios que terminam aqui
                    (0..=pos).rev().any(|ini| {
                        let mut c3 = caps.clone();
                        self.alt(alt, ini, &mut c3, &mut |p, _, _| p == pos)
                    })
                };
                if ok != *negado {
                    if ok && !*negado {
                        *caps = c2;
                    }
                    self.seq(resto, pos, caps, k)
                } else {
                    false
                }
            }
            No::Grupo(alt, cap) => {
                let cap = *cap;
                let antigo = cap.and_then(|g| caps[g]);
                let ok = self.alt(alt, pos, caps, &mut |p, cs: &mut Caps, eu: &mut Self| {
                    let salvo = cap.and_then(|g| cs[g]);
                    if let Some(g) = cap {
                        cs[g] = Some((pos, p));
                    }
                    if eu.seq(resto, p, cs, k) {
                        return true;
                    }
                    if let Some(g) = cap {
                        cs[g] = salvo;
                    }
                    false
                });
                if !ok {
                    if let Some(g) = cap {
                        caps[g] = antigo;
                    }
                }
                ok
            }
            No::Rep(item, min, max, guloso) => {
                let (min, max, guloso) = (*min, *max, *guloso);
                if Self::simples(item) {
                    // conta quantos casam de uma vez
                    let mut n = 0u32;
                    while n < max && pos + (n as usize) < self.t.len() && self.um_char(item, self.t[pos + n as usize]) {
                        n += 1;
                    }
                    if n < min {
                        return false;
                    }
                    if guloso {
                        let mut j = n;
                        loop {
                            if self.seq(resto, pos + j as usize, caps, k) {
                                return true;
                            }
                            if j == min {
                                return false;
                            }
                            j -= 1;
                        }
                    } else {
                        for j in min..=n {
                            if self.seq(resto, pos + j as usize, caps, k) {
                                return true;
                            }
                        }
                        return false;
                    }
                }
                self.rep(item, min, max, guloso, pos, 0, caps, resto, k)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn rep(
        &mut self,
        item: &No,
        min: u32,
        max: u32,
        guloso: bool,
        pos: usize,
        feitos: u32,
        caps: &mut Caps,
        resto: &[No],
        k: &mut dyn FnMut(usize, &mut Caps, &mut Self) -> bool,
    ) -> bool {
        let tentar_mais = |eu: &mut Self, caps: &mut Caps, k: &mut dyn FnMut(usize, &mut Caps, &mut Self) -> bool| -> bool {
            if feitos >= max {
                return false;
            }
            let um = core::slice::from_ref(item);
            eu.seq(um, pos, caps, &mut |p, cs: &mut Caps, eu2: &mut Self| {
                if p == pos && feitos >= min {
                    return false; // repeticao vazia: para
                }
                eu2.rep(item, min, max, guloso, p, feitos + 1, cs, resto, k)
            })
        };
        if feitos < min {
            return tentar_mais(self, caps, k);
        }
        if guloso {
            if tentar_mais(self, caps, k) {
                return true;
            }
            self.seq(resto, pos, caps, k)
        } else {
            if self.seq(resto, pos, caps, k) {
                return true;
            }
            tentar_mais(self, caps, k)
        }
    }
}

impl Regex {
    pub fn nova(fonte: &str, flags: &str) -> Result<Regex, &'static str> {
        let mut l = Ler { c: fonte.chars().collect(), i: 0, grupos: 0, nomes: Vec::new(), _s: fonte };
        let alt = l.alternativas()?;
        if l.i < l.c.len() {
            return Err("parêntese a mais");
        }
        Ok(Regex {
            alt,
            grupos: l.grupos,
            nomes: l.nomes,
            fonte: Rc::from(fonte),
            flags: Rc::from(flags),
            global: flags.contains('g'),
            sem_caixa: flags.contains('i'),
            multilinha: flags.contains('m'),
            ponto_tudo: flags.contains('s'),
            grudado: flags.contains('y'),
            unicode: flags.contains('u'),
        })
    }

    /// Procura a partir de `inicio` (em chars). Devolve as capturas
    /// (0 = o casamento todo), em indices de char.
    pub fn procurar(&self, t: &[char], inicio: usize) -> Option<Caps> {
        let mut c = Casador { t, r: self, passos: 0 };
        let mut pos = inicio;
        // otimizacao: comeca com um caractere fixo? pula direto ate ele
        let primeiro = match self.alt.as_slice() {
            [seq] => match seq.first() {
                Some(No::Char(ch)) if !self.sem_caixa => Some(*ch),
                _ => None,
            },
            _ => None,
        };
        while pos <= t.len() {
            if let Some(ch) = primeiro {
                match t[pos..].iter().position(|&x| x == ch) {
                    Some(p) => pos += p,
                    None => return None,
                }
            }
            let mut caps: Caps = alloc::vec![None; self.grupos + 1];
            let mut fim = None;
            let achou = c.alt(&self.alt, pos, &mut caps, &mut |p, _, _| {
                fim = Some(p);
                true
            });
            if achou {
                caps[0] = Some((pos, fim.unwrap_or(pos)));
                return Some(caps);
            }
            if self.grudado || c.passos > 5_000_000 {
                return None;
            }
            pos += 1;
        }
        None
    }
}
