//! Seletores do CSS (Selectors Level 4): leitura, especificidade e
//! casamento da direita para a esquerda (como os navegadores fazem: o
//! ultimo composto filtra quase tudo de cara).

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use super::token::Token;
use crate::web::atomo::{Atomo, a};
use crate::web::dom::{Documento, Espaco, Id, NENHUM, Tipo};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Combinador {
    Descendente,
    Filho,
    Adjacente,
    Irmao,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OpAtributo {
    Existe,
    Igual,
    Palavra,
    Prefixo,
    Comeca,
    Termina,
    Contem,
}

#[derive(Clone, PartialEq, Debug)]
pub struct FiltroAtributo {
    pub nome: Atomo,
    pub op: OpAtributo,
    pub valor: String,
    pub sem_caixa: bool,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Pseudo {
    PrimeiroFilho,
    UltimoFilho,
    UnicoFilho,
    PrimeiroDoTipo,
    UltimoDoTipo,
    UnicoDoTipo,
    Nth { a: i32, b: i32, do_tipo: bool, do_fim: bool, de: Option<Box<Vec<Seletor>>> },
    Nao(Vec<Seletor>),
    E(Vec<Seletor>),
    Tem(Vec<(Combinador, Seletor)>),
    Raiz,
    Vazio,
    Link,
    Visitado,
    Hover,
    Ativo,
    Foco,
    FocoDentro,
    Marcado,
    Desabilitado,
    Habilitado,
    Obrigatorio,
    Opcional,
    SoLeitura,
    Editavel,
    Alvo,
    PlaceholderVisivel,
    Nunca,
    Sempre,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PseudoElemento {
    Antes,
    Depois,
    PrimeiraLinha,
    PrimeiraLetra,
    Marcador,
    Placeholder,
    Selecao,
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct Composto {
    pub tag: Option<Atomo>,
    pub id: Option<Atomo>,
    pub classes: Vec<Atomo>,
    pub atributos: Vec<FiltroAtributo>,
    pub pseudos: Vec<Pseudo>,
}

/// Um seletor complexo, guardado da DIREITA para a esquerda: partes[0] e o
/// composto do proprio elemento; `ligacoes[i]` liga partes[i] a partes[i+1].
#[derive(Clone, PartialEq, Debug)]
pub struct Seletor {
    pub partes: Vec<Composto>,
    pub ligacoes: Vec<Combinador>,
    pub especificidade: u32,
    pub pseudo_elemento: Option<PseudoElemento>,
    /// Tags/ids/classes que algum ANCESTRAL precisa ter (para o filtro de
    /// Bloom rejeitar sem andar na arvore).
    pub ancestrais: Vec<u32>,
}

/// Chave de um traco de elemento no filtro de Bloom.
pub fn traco(tipo: u32, x: Atomo) -> u32 {
    x.0.wrapping_mul(4).wrapping_add(tipo).wrapping_mul(0x9E37_79B1)
}

/// Filtro de Bloom contador dos ancestrais do elemento atual (o que a
/// cascata empilha e desempilha ao descer e subir na arvore).
pub struct Bloom {
    c: Vec<u8>,
}

impl Default for Bloom {
    fn default() -> Self {
        Bloom { c: alloc::vec![0; 4096] }
    }
}

impl Bloom {
    fn indices(h: u32) -> (usize, usize) {
        ((h & 0xFFF) as usize, ((h >> 12) & 0xFFF) as usize)
    }
    pub fn inserir(&mut self, h: u32) {
        let (a, b) = Self::indices(h);
        self.c[a] = self.c[a].saturating_add(1);
        self.c[b] = self.c[b].saturating_add(1);
    }
    pub fn remover(&mut self, h: u32) {
        let (a, b) = Self::indices(h);
        if self.c[a] != 255 {
            self.c[a] = self.c[a].saturating_sub(1);
        }
        if self.c[b] != 255 {
            self.c[b] = self.c[b].saturating_sub(1);
        }
    }
    pub fn pode_ter(&self, h: u32) -> bool {
        let (a, b) = Self::indices(h);
        self.c[a] != 0 && self.c[b] != 0
    }
}

/// Os tracos de um elemento (tag, id, classes).
pub fn tracos(e: &crate::web::dom::Elemento, v: &mut Vec<u32>) {
    v.push(traco(0, e.nome));
    if let Some(i) = e.id {
        v.push(traco(1, i));
    }
    for c in &e.classes {
        v.push(traco(2, *c));
    }
}

/// Estado dinamico para :hover, :focus... (ids de elementos).
#[derive(Clone, Copy, Debug)]
pub struct Estado {
    pub hover: Id,
    pub foco: Id,
    pub ativo: Id,
    pub alvo: Id,
}

impl Default for Estado {
    fn default() -> Self {
        Estado { hover: NENHUM, foco: NENHUM, ativo: NENHUM, alvo: NENHUM }
    }
}

// ----------------------------------------------------------------- leitura

struct Ler<'a> {
    t: &'a [Token],
    i: usize,
}

impl<'a> Ler<'a> {
    fn espiar(&self) -> Option<&'a Token> {
        self.t.get(self.i)
    }
    fn pegar(&mut self) -> Option<&'a Token> {
        let t = self.t.get(self.i);
        self.i += 1;
        t
    }
    fn pular_espacos(&mut self) -> bool {
        let mut pulou = false;
        while self.espiar() == Some(&Token::Espaco) {
            self.i += 1;
            pulou = true;
        }
        pulou
    }
    /// Tokens ate o ')' que fecha (ja passado o Funcao).
    fn argumentos(&mut self) -> Option<&'a [Token]> {
        let inicio = self.i;
        let mut nivel = 1;
        loop {
            match self.pegar()? {
                Token::AbreParen | Token::Funcao(_) => nivel += 1,
                Token::FechaParen => {
                    nivel -= 1;
                    if nivel == 0 {
                        return Some(&self.t[inicio..self.i - 1]);
                    }
                }
                _ => {}
            }
        }
    }
}

/// Le "a, b > c, ..." Devolve None se algum seletor for invalido (a regra
/// inteira e descartada, como manda a especificacao).
pub fn ler_lista(t: &[Token]) -> Option<Vec<Seletor>> {
    let mut v = Vec::new();
    for parte in dividir_virgulas(t) {
        v.push(ler_complexo(parte)?);
    }
    if v.is_empty() { None } else { Some(v) }
}

/// Como ler_lista, mas descarta so os invalidos (:is, :where).
fn ler_lista_tolerante(t: &[Token]) -> Vec<Seletor> {
    dividir_virgulas(t).into_iter().filter_map(ler_complexo).collect()
}

fn dividir_virgulas(t: &[Token]) -> Vec<&[Token]> {
    let mut v = Vec::new();
    let mut nivel = 0;
    let mut inicio = 0;
    for (k, x) in t.iter().enumerate() {
        match x {
            Token::AbreParen | Token::Funcao(_) | Token::AbreColchete => nivel += 1,
            Token::FechaParen | Token::FechaColchete => nivel -= 1,
            Token::Virgula if nivel == 0 => {
                v.push(&t[inicio..k]);
                inicio = k + 1;
            }
            _ => {}
        }
    }
    v.push(&t[inicio..]);
    v
}

fn ler_complexo(t: &[Token]) -> Option<Seletor> {
    let mut l = Ler { t, i: 0 };
    l.pular_espacos();
    let mut partes = Vec::new();
    let mut ligacoes = Vec::new();
    let mut pseudo_elemento = None;
    loop {
        let (c, pe) = ler_composto(&mut l)?;
        if pe.is_some() {
            pseudo_elemento = pe;
        }
        partes.push(c);
        let espaco = l.pular_espacos();
        let comb = match l.espiar() {
            None => break,
            Some(Token::Delim('>')) => Combinador::Filho,
            Some(Token::Delim('+')) => Combinador::Adjacente,
            Some(Token::Delim('~')) => Combinador::Irmao,
            Some(_) if espaco => Combinador::Descendente,
            Some(_) => return None,
        };
        if comb != Combinador::Descendente {
            l.pegar();
            l.pular_espacos();
        }
        if pseudo_elemento.is_some() {
            return None; // nada depois de ::before
        }
        ligacoes.push(comb);
    }
    partes.reverse();
    ligacoes.reverse();
    let mut especificidade: u32 = partes.iter().map(especificidade_composto).sum();
    if pseudo_elemento.is_some() {
        especificidade += 1;
    }
    // tracos obrigatorios dos ancestrais: partes alcancadas so por > e espaco
    let mut ancestrais = Vec::new();
    for k in 1..partes.len() {
        if !matches!(ligacoes[k - 1], Combinador::Descendente | Combinador::Filho) {
            break;
        }
        let c = &partes[k];
        if let Some(t) = c.tag {
            ancestrais.push(traco(0, t));
        }
        if let Some(i) = c.id {
            ancestrais.push(traco(1, i));
        }
        for cl in &c.classes {
            ancestrais.push(traco(2, *cl));
        }
    }
    Some(Seletor { partes, ligacoes, especificidade, pseudo_elemento, ancestrais })
}

fn especificidade_lista(v: &[Seletor]) -> u32 {
    v.iter().map(|s| s.especificidade).max().unwrap_or(0)
}

fn especificidade_composto(c: &Composto) -> u32 {
    let mut e = 0;
    if c.id.is_some() {
        e += 1 << 20;
    }
    e += (c.classes.len() as u32 + c.atributos.len() as u32) << 10;
    if c.tag.is_some() {
        e += 1;
    }
    for p in &c.pseudos {
        e += match p {
            Pseudo::Nao(v) | Pseudo::E(v) => especificidade_lista(v),
            Pseudo::Tem(v) => v.iter().map(|(_, s)| s.especificidade).max().unwrap_or(0),
            Pseudo::Sempre => 0, // :where()
            Pseudo::Nth { de: Some(v), .. } => (1 << 10) + especificidade_lista(v),
            _ => 1 << 10,
        };
    }
    e
}

fn ler_composto(l: &mut Ler) -> Option<(Composto, Option<PseudoElemento>)> {
    let mut c = Composto::default();
    let mut pe = None;
    let mut algum = false;
    match l.espiar() {
        Some(Token::Ident(n)) => {
            c.tag = Some(Atomo::minusculo(n));
            l.pegar();
            algum = true;
        }
        Some(Token::Delim('*')) => {
            l.pegar();
            algum = true;
        }
        _ => {}
    }
    // namespace "ns|tag": ignorado
    if l.espiar() == Some(&Token::Delim('|')) {
        l.pegar();
        match l.pegar()? {
            Token::Ident(n) => c.tag = Some(Atomo::minusculo(n)),
            Token::Delim('*') => c.tag = None,
            _ => return None,
        }
    }
    loop {
        match l.espiar() {
            Some(Token::Hash(h, true)) => {
                c.id = Some(Atomo::de(h));
                l.pegar();
            }
            Some(Token::Delim('.')) => {
                l.pegar();
                match l.pegar()? {
                    Token::Ident(n) => c.classes.push(Atomo::de(n)),
                    _ => return None,
                }
            }
            Some(Token::AbreColchete) => {
                l.pegar();
                c.atributos.push(ler_atributo(l)?);
            }
            Some(Token::DoisPontos) => {
                l.pegar();
                if l.espiar() == Some(&Token::DoisPontos) {
                    l.pegar();
                    pe = Some(ler_pseudo_elemento(l)?);
                } else {
                    match ler_pseudo(l)? {
                        Ok(p) => c.pseudos.push(p),
                        Err(e) => pe = Some(e),
                    }
                }
            }
            _ => break,
        }
        algum = true;
    }
    if algum { Some((c, pe)) } else { None }
}

fn ler_atributo(l: &mut Ler) -> Option<FiltroAtributo> {
    l.pular_espacos();
    let nome = match l.pegar()? {
        Token::Ident(n) => Atomo::minusculo(n),
        _ => return None,
    };
    l.pular_espacos();
    let op = match l.pegar()? {
        Token::FechaColchete => return Some(FiltroAtributo { nome, op: OpAtributo::Existe, valor: String::new(), sem_caixa: false }),
        Token::Delim('=') => OpAtributo::Igual,
        Token::Delim(c) => {
            let op = match c {
                '~' => OpAtributo::Palavra,
                '|' => OpAtributo::Prefixo,
                '^' => OpAtributo::Comeca,
                '$' => OpAtributo::Termina,
                '*' => OpAtributo::Contem,
                _ => return None,
            };
            if l.pegar()? != &Token::Delim('=') {
                return None;
            }
            op
        }
        _ => return None,
    };
    l.pular_espacos();
    let valor = match l.pegar()? {
        Token::Ident(s) | Token::Texto(s) => s.clone(),
        Token::Numero(v, _) => alloc::format!("{v}"),
        _ => return None,
    };
    l.pular_espacos();
    let mut sem_caixa = false;
    if let Some(Token::Ident(f)) = l.espiar() {
        sem_caixa = f.eq_ignore_ascii_case("i");
        l.pegar();
        l.pular_espacos();
    }
    if l.pegar()? != &Token::FechaColchete {
        return None;
    }
    Some(FiltroAtributo { nome, op, valor, sem_caixa })
}

fn ler_pseudo_elemento(l: &mut Ler) -> Option<PseudoElemento> {
    match l.pegar()? {
        Token::Ident(n) => Some(match n.to_ascii_lowercase().as_str() {
            "before" => PseudoElemento::Antes,
            "after" => PseudoElemento::Depois,
            "first-line" => PseudoElemento::PrimeiraLinha,
            "first-letter" => PseudoElemento::PrimeiraLetra,
            "marker" => PseudoElemento::Marcador,
            "placeholder" => PseudoElemento::Placeholder,
            "selection" => PseudoElemento::Selecao,
            _ => return None,
        }),
        _ => None,
    }
}

/// Ok(pseudo-classe) ou Err(pseudo-elemento escrito com um ':' so).
fn ler_pseudo(l: &mut Ler) -> Option<Result<Pseudo, PseudoElemento>> {
    match l.pegar()? {
        Token::Ident(n) => Some(Ok(match n.to_ascii_lowercase().as_str() {
            "first-child" => Pseudo::PrimeiroFilho,
            "last-child" => Pseudo::UltimoFilho,
            "only-child" => Pseudo::UnicoFilho,
            "first-of-type" => Pseudo::PrimeiroDoTipo,
            "last-of-type" => Pseudo::UltimoDoTipo,
            "only-of-type" => Pseudo::UnicoDoTipo,
            "root" => Pseudo::Raiz,
            "empty" => Pseudo::Vazio,
            "link" | "any-link" => Pseudo::Link,
            "visited" => Pseudo::Visitado,
            "hover" => Pseudo::Hover,
            "active" => Pseudo::Ativo,
            "focus" | "focus-visible" => Pseudo::Foco,
            "focus-within" => Pseudo::FocoDentro,
            "checked" => Pseudo::Marcado,
            "disabled" => Pseudo::Desabilitado,
            "enabled" => Pseudo::Habilitado,
            "required" => Pseudo::Obrigatorio,
            "optional" => Pseudo::Opcional,
            "read-only" => Pseudo::SoLeitura,
            "read-write" => Pseudo::Editavel,
            "target" => Pseudo::Alvo,
            "placeholder-shown" => Pseudo::PlaceholderVisivel,
            "scope" => Pseudo::Raiz,
            "defined" => Pseudo::Sempre,
            "before" => return Some(Err(PseudoElemento::Antes)),
            "after" => return Some(Err(PseudoElemento::Depois)),
            "first-line" => return Some(Err(PseudoElemento::PrimeiraLinha)),
            "first-letter" => return Some(Err(PseudoElemento::PrimeiraLetra)),
            // estados que nunca valem aqui (mas o seletor e valido)
            "fullscreen" | "modal" | "picture-in-picture" | "autofill" | "playing" | "paused" | "invalid"
            | "user-invalid" | "indeterminate" | "default" | "in-range" | "out-of-range" | "popover-open" => Pseudo::Nunca,
            "valid" | "user-valid" => Pseudo::Sempre,
            _ => return None,
        })),
        Token::Funcao(f) => {
            let f = f.clone();
            let args = l.argumentos()?;
            Some(Ok(match f.as_str() {
                "not" => Pseudo::Nao(ler_lista(args)?),
                "is" | "matches" | "-webkit-any" => {
                    let v = ler_lista_tolerante(args);
                    if v.is_empty() { Pseudo::Nunca } else { Pseudo::E(v) }
                }
                "where" => {
                    let v = ler_lista_tolerante(args);
                    if v.is_empty() {
                        Pseudo::Nunca
                    } else {
                        // :where() casa como :is() mas nao soma especificidade
                        let v: Vec<Seletor> = v.into_iter().map(|mut s| {
                            s.especificidade = 0;
                            s
                        }).collect();
                        Pseudo::E(v)
                    }
                }
                "has" => {
                    let mut v = Vec::new();
                    for parte in dividir_virgulas(args) {
                        let mut i = 0;
                        while parte.get(i) == Some(&Token::Espaco) {
                            i += 1;
                        }
                        let comb = match parte.get(i) {
                            Some(Token::Delim('>')) => Combinador::Filho,
                            Some(Token::Delim('+')) => Combinador::Adjacente,
                            Some(Token::Delim('~')) => Combinador::Irmao,
                            _ => Combinador::Descendente,
                        };
                        let resto = if comb == Combinador::Descendente { &parte[i..] } else { &parte[i + 1..] };
                        v.push((comb, ler_complexo(resto)?));
                    }
                    Pseudo::Tem(v)
                }
                "nth-child" | "nth-last-child" | "nth-of-type" | "nth-last-of-type" => {
                    let do_tipo = f.ends_with("of-type");
                    let do_fim = f.contains("last");
                    // "an+b [of S]"
                    let corte = args.iter().position(|t| matches!(t, Token::Ident(s) if s.eq_ignore_ascii_case("of")));
                    let (formula, de) = match corte {
                        Some(k) if !do_tipo => (&args[..k], Some(Box::new(ler_lista(&args[k + 1..])?))),
                        _ => (args, None),
                    };
                    let (a, b) = ler_anb(formula)?;
                    Pseudo::Nth { a, b, do_tipo, do_fim, de }
                }
                "lang" | "dir" | "host" | "host-context" | "state" => Pseudo::Nunca,
                _ => return None,
            }))
        }
        _ => None,
    }
}

/// "odd", "even", "3", "2n+1", "-n+3"...
fn ler_anb(t: &[Token]) -> Option<(i32, i32)> {
    let mut s = String::new();
    for x in t {
        match x {
            Token::Espaco => {}
            Token::Ident(i) => s.push_str(i),
            Token::Dimensao(v, u) => {
                s.push_str(&alloc::format!("{}", *v as i32));
                s.push_str(u);
            }
            Token::Numero(v, _) => s.push_str(&alloc::format!("{:+}", *v as i32)),
            Token::Delim(c) => s.push(*c),
            _ => return None,
        }
    }
    let s = s.to_ascii_lowercase();
    match s.as_str() {
        "odd" => return Some((2, 1)),
        "even" => return Some((2, 0)),
        _ => {}
    }
    match s.find('n') {
        None => Some((0, s.trim_start_matches('+').parse().ok()?)),
        Some(k) => {
            let a = match &s[..k] {
                "" | "+" => 1,
                "-" => -1,
                x => x.trim_start_matches('+').parse().ok()?,
            };
            let resto = &s[k + 1..];
            let b = if resto.is_empty() { 0 } else { resto.trim_start_matches('+').parse().ok()? };
            Some((a, b))
        }
    }
}

// ---------------------------------------------------------------- casamento

pub struct Casador<'a> {
    pub doc: &'a Documento,
    pub estado: Estado,
}

fn eh_elemento(doc: &Documento, id: Id) -> bool {
    matches!(doc.no(id).tipo, Tipo::Elemento(_))
}

impl<'a> Casador<'a> {
    pub fn novo(doc: &'a Documento, estado: Estado) -> Self {
        Casador { doc, estado }
    }

    fn pai_elemento(&self, id: Id) -> Id {
        let p = self.doc.pai(id);
        if p != NENHUM && eh_elemento(self.doc, p) { p } else { NENHUM }
    }

    fn irmao_anterior(&self, id: Id) -> Id {
        let mut x = self.doc.no(id).anterior;
        while x != NENHUM && !eh_elemento(self.doc, x) {
            x = self.doc.no(x).anterior;
        }
        x
    }

    fn irmao_seguinte(&self, id: Id) -> Id {
        let mut x = self.doc.no(id).proximo;
        while x != NENHUM && !eh_elemento(self.doc, x) {
            x = self.doc.no(x).proximo;
        }
        x
    }

    /// O seletor casa com o elemento `id`?
    pub fn casa(&self, s: &Seletor, id: Id) -> bool {
        self.casa_desde(s, 0, id)
    }

    /// Como `casa`, mas rejeita logo pelo filtro de Bloom dos ancestrais.
    pub fn casa_com_filtro(&self, s: &Seletor, id: Id, b: &Bloom) -> bool {
        if !s.ancestrais.iter().all(|&h| b.pode_ter(h)) {
            return false;
        }
        self.casa_desde(s, 0, id)
    }

    fn casa_desde(&self, s: &Seletor, k: usize, id: Id) -> bool {
        if !self.composto(&s.partes[k], id) {
            return false;
        }
        if k + 1 == s.partes.len() {
            return true;
        }
        match s.ligacoes[k] {
            Combinador::Filho => {
                let p = self.pai_elemento(id);
                p != NENHUM && self.casa_desde(s, k + 1, p)
            }
            Combinador::Descendente => {
                let mut p = self.pai_elemento(id);
                while p != NENHUM {
                    if self.casa_desde(s, k + 1, p) {
                        return true;
                    }
                    p = self.pai_elemento(p);
                }
                false
            }
            Combinador::Adjacente => {
                let a = self.irmao_anterior(id);
                a != NENHUM && self.casa_desde(s, k + 1, a)
            }
            Combinador::Irmao => {
                let mut a = self.irmao_anterior(id);
                while a != NENHUM {
                    if self.casa_desde(s, k + 1, a) {
                        return true;
                    }
                    a = self.irmao_anterior(a);
                }
                false
            }
        }
    }

    fn composto(&self, c: &Composto, id: Id) -> bool {
        let Some(e) = self.doc.elemento(id) else { return false };
        if let Some(t) = c.tag {
            if e.nome != t && !(e.espaco != Espaco::Html && e.nome.texto().eq_ignore_ascii_case(t.texto())) {
                return false;
            }
        }
        if let Some(i) = c.id {
            if e.id != Some(i) {
                return false;
            }
        }
        for cl in &c.classes {
            if !e.classes.contains(cl) {
                return false;
            }
        }
        for f in &c.atributos {
            let Some(v) = e.atributo(f.nome) else { return false };
            if !casa_atributo(f, v) {
                return false;
            }
        }
        c.pseudos.iter().all(|p| self.pseudo(p, id))
    }

    fn posicao(&self, id: Id, do_tipo: bool, do_fim: bool, de: Option<&[Seletor]>) -> i32 {
        let nome = self.doc.elemento(id).map(|e| e.nome);
        let mut n = 1;
        let mut x = if do_fim { self.irmao_seguinte(id) } else { self.irmao_anterior(id) };
        while x != NENHUM {
            let conta = if do_tipo {
                self.doc.elemento(x).map(|e| e.nome) == nome
            } else if let Some(de) = de {
                de.iter().any(|s| self.casa(s, x))
            } else {
                true
            };
            if conta {
                n += 1;
            }
            x = if do_fim { self.irmao_seguinte(x) } else { self.irmao_anterior(x) };
        }
        n
    }

    fn pseudo(&self, p: &Pseudo, id: Id) -> bool {
        let e = match self.doc.elemento(id) {
            Some(e) => e,
            None => return false,
        };
        match p {
            Pseudo::PrimeiroFilho => self.irmao_anterior(id) == NENHUM,
            Pseudo::UltimoFilho => self.irmao_seguinte(id) == NENHUM,
            Pseudo::UnicoFilho => self.irmao_anterior(id) == NENHUM && self.irmao_seguinte(id) == NENHUM,
            Pseudo::PrimeiroDoTipo => self.posicao(id, true, false, None) == 1,
            Pseudo::UltimoDoTipo => self.posicao(id, true, true, None) == 1,
            Pseudo::UnicoDoTipo => self.posicao(id, true, false, None) == 1 && self.posicao(id, true, true, None) == 1,
            Pseudo::Nth { a, b, do_tipo, do_fim, de } => {
                if let Some(de) = de {
                    if !de.iter().any(|s| self.casa(s, id)) {
                        return false;
                    }
                }
                let n = self.posicao(id, *do_tipo, *do_fim, de.as_deref().map(|v| v.as_slice()));
                if *a == 0 {
                    n == *b
                } else {
                    let d = n - b;
                    d % a == 0 && d / a >= 0
                }
            }
            Pseudo::Nao(v) => !v.iter().any(|s| self.casa(s, id)),
            Pseudo::E(v) => v.iter().any(|s| self.casa(s, id)),
            Pseudo::Tem(v) => v.iter().any(|(comb, s)| match comb {
                Combinador::Descendente => self.doc.descendentes(id).any(|d| eh_elemento(self.doc, d) && self.casa(s, d)),
                Combinador::Filho => self.doc.filhos(id).any(|d| eh_elemento(self.doc, d) && self.casa(s, d)),
                Combinador::Adjacente => {
                    let x = self.irmao_seguinte(id);
                    x != NENHUM && self.casa(s, x)
                }
                Combinador::Irmao => {
                    let mut x = self.irmao_seguinte(id);
                    while x != NENHUM {
                        if self.casa(s, x) {
                            return true;
                        }
                        x = self.irmao_seguinte(x);
                    }
                    false
                }
            }),
            Pseudo::Raiz => self.pai_elemento(id) == NENHUM,
            Pseudo::Vazio => self.doc.filhos(id).all(|f| match &self.doc.no(f).tipo {
                Tipo::Texto(t) => t.is_empty(),
                Tipo::Comentario(_) => true,
                _ => false,
            }),
            Pseudo::Link => (e.eh(a::A) || e.eh(a::AREA)) && e.tem(a::HREF),
            Pseudo::Visitado => false,
            Pseudo::Hover => self.contem(id, self.estado.hover),
            Pseudo::Ativo => self.contem(id, self.estado.ativo),
            Pseudo::Foco => id == self.estado.foco,
            Pseudo::FocoDentro => self.contem(id, self.estado.foco),
            Pseudo::Alvo => id == self.estado.alvo,
            Pseudo::Marcado => {
                (e.eh(a::INPUT) && e.tem(a::CHECKED)) || (e.eh(a::OPTION) && e.tem(a::SELECTED))
            }
            Pseudo::Desabilitado => formulario(e.nome) && e.tem(a::DISABLED),
            Pseudo::Habilitado => formulario(e.nome) && !e.tem(a::DISABLED),
            Pseudo::Obrigatorio => e.tem(Atomo::de("required")),
            Pseudo::Opcional => formulario(e.nome) && !e.tem(Atomo::de("required")),
            Pseudo::SoLeitura => !(e.eh(a::INPUT) || e.eh(a::TEXTAREA)) || e.tem(a::READONLY),
            Pseudo::Editavel => (e.eh(a::INPUT) || e.eh(a::TEXTAREA)) && !e.tem(a::READONLY),
            Pseudo::PlaceholderVisivel => {
                e.tem(a::PLACEHOLDER) && e.atributo(a::VALUE).is_none_or(|v| v.is_empty())
            }
            Pseudo::Nunca => false,
            Pseudo::Sempre => true,
        }
    }

    /// `alvo` e `id` ou esta dentro dele?
    fn contem(&self, id: Id, alvo: Id) -> bool {
        let mut x = alvo;
        while x != NENHUM {
            if x == id {
                return true;
            }
            x = self.doc.pai(x);
        }
        false
    }
}

fn formulario(n: Atomo) -> bool {
    [a::INPUT, a::BUTTON, a::SELECT, a::TEXTAREA, a::OPTION, a::OPTGROUP, a::FIELDSET].contains(&n)
}

fn casa_atributo(f: &FiltroAtributo, v: &str) -> bool {
    let igual = |x: &str, y: &str| if f.sem_caixa { x.eq_ignore_ascii_case(y) } else { x == y };
    let w = f.valor.as_str();
    match f.op {
        OpAtributo::Existe => true,
        OpAtributo::Igual => igual(v, w),
        OpAtributo::Palavra => !w.is_empty() && v.split_ascii_whitespace().any(|p| igual(p, w)),
        OpAtributo::Prefixo => igual(v, w) || (v.len() > w.len() && v.as_bytes()[w.len()] == b'-' && igual(&v[..w.len()], w)),
        OpAtributo::Comeca => !w.is_empty() && v.len() >= w.len() && v.is_char_boundary(w.len()) && igual(&v[..w.len()], w),
        OpAtributo::Termina => {
            !w.is_empty() && v.len() >= w.len() && v.is_char_boundary(v.len() - w.len()) && igual(&v[v.len() - w.len()..], w)
        }
        OpAtributo::Contem => {
            !w.is_empty() && if f.sem_caixa { v.to_ascii_lowercase().contains(&w.to_ascii_lowercase()) } else { v.contains(w) }
        }
    }
}

impl Seletor {
    /// A chave do balde onde esta regra mora (o composto da direita):
    /// id > classe > tag > universal.
    pub fn chave(&self) -> Chave {
        let c = &self.partes[0];
        if let Some(i) = c.id {
            Chave::Id(i)
        } else if let Some(&cl) = c.classes.first() {
            Chave::Classe(cl)
        } else if let Some(t) = c.tag {
            Chave::Tag(t)
        } else {
            Chave::Universal
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Chave {
    Id(Atomo),
    Classe(Atomo),
    Tag(Atomo),
    Universal,
}
