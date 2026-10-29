//! Folhas de estilo: regras, declaracoes e at-rules (@media, @supports,
//! @import, @layer, @font-face). As condicoes de @media sao avaliadas na
//! leitura (para o tamanho de tela atual): o que nao vale nem entra.

use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

use super::seletor::{self, Seletor};
use super::token::{self, Token};
use super::valores::{Leitor, ler_comprimento, Contexto};

#[derive(Clone, Debug)]
pub struct Declaracao {
    pub nome: String,
    pub valor: Vec<Token>,
    pub importante: bool,
}

#[derive(Clone, Debug)]
pub struct Regra {
    pub seletores: Vec<Seletor>,
    pub declaracoes: Rc<Vec<Declaracao>>,
}

#[derive(Clone, Debug, Default)]
pub struct FonteWeb {
    pub familia: String,
    pub urls: Vec<String>,
    pub peso: u16,
    pub italico: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Folha {
    pub regras: Vec<Regra>,
    pub importar: Vec<String>,
    pub fontes: Vec<FonteWeb>,
}

/// O que as media queries consultam.
#[derive(Clone, Copy, Debug)]
pub struct Midia {
    pub largura: f32,
    pub altura: f32,
}

impl Folha {
    pub fn ler(texto: &str, m: &Midia) -> Folha {
        let t = token::tokens(texto);
        let mut f = Folha::default();
        ler_regras(&t, m, &mut f, true);
        f
    }
}

/// Le declaracoes "a: b; c: d !important" (atributo style e blocos).
pub fn ler_declaracoes(t: &[Token]) -> Vec<Declaracao> {
    let mut v = Vec::new();
    let mut i = 0;
    while i < t.len() {
        // um pedaco ate ';' no nivel 0 (pulando blocos aninhados)
        let inicio = i;
        let mut nivel = 0;
        let mut bloco = false;
        while i < t.len() {
            match &t[i] {
                Token::AbreChave | Token::AbreParen | Token::AbreColchete | Token::Funcao(_) => {
                    if t[i] == Token::AbreChave && nivel == 0 {
                        bloco = true;
                    }
                    nivel += 1;
                }
                Token::FechaChave | Token::FechaParen | Token::FechaColchete => {
                    nivel -= 1;
                    if nivel == 0 && bloco && t[i] == Token::FechaChave {
                        i += 1;
                        break; // regra aninhada (CSS nesting): ignorada
                    }
                }
                Token::PontoEVirgula if nivel == 0 => break,
                _ => {}
            }
            i += 1;
        }
        let pedaco = &t[inicio..i.min(t.len())];
        i += 1;
        if bloco {
            continue;
        }
        if let Some(d) = ler_declaracao(pedaco) {
            v.push(d);
        }
    }
    v
}

fn ler_declaracao(t: &[Token]) -> Option<Declaracao> {
    let mut k = 0;
    while t.get(k) == Some(&Token::Espaco) {
        k += 1;
    }
    let nome = match t.get(k)? {
        Token::Ident(n) => n.clone(),
        _ => return None,
    };
    k += 1;
    while t.get(k) == Some(&Token::Espaco) {
        k += 1;
    }
    if t.get(k)? != &Token::DoisPontos {
        return None;
    }
    let mut valor: Vec<Token> = t[k + 1..].to_vec();
    while valor.last() == Some(&Token::Espaco) {
        valor.pop();
    }
    let mut importante = false;
    if matches!(valor.last(), Some(Token::Ident(s)) if s.eq_ignore_ascii_case("important")) {
        let mut j = valor.len() - 1;
        while j > 0 && valor[j - 1] == Token::Espaco {
            j -= 1;
        }
        if j > 0 && valor[j - 1] == Token::Delim('!') {
            importante = true;
            valor.truncate(j - 1);
            while valor.last() == Some(&Token::Espaco) {
                valor.pop();
            }
        }
    }
    while valor.first() == Some(&Token::Espaco) {
        valor.remove(0);
    }
    let nome = if nome.starts_with("--") { nome } else { nome.to_ascii_lowercase() };
    if valor.is_empty() && !nome.starts_with("--") {
        return None;
    }
    Some(Declaracao { nome, valor, importante })
}

/// Fim do bloco que abre em t[i] ('{'): indice do '}' correspondente.
fn fim_bloco(t: &[Token], i: usize) -> usize {
    let mut nivel = 0;
    let mut j = i;
    while j < t.len() {
        match t[j] {
            Token::AbreChave => nivel += 1,
            Token::FechaChave => {
                nivel -= 1;
                if nivel == 0 {
                    return j;
                }
            }
            _ => {}
        }
        j += 1;
    }
    t.len()
}

fn ler_regras(t: &[Token], m: &Midia, f: &mut Folha, topo: bool) {
    let mut i = 0;
    while i < t.len() {
        match &t[i] {
            Token::Espaco | Token::Cdo | Token::Cdc | Token::PontoEVirgula => i += 1,
            Token::Arroba(nome) => {
                let nome = nome.clone();
                // preludio ate ';' ou '{'
                let inicio = i + 1;
                let mut j = inicio;
                while j < t.len() && t[j] != Token::PontoEVirgula && t[j] != Token::AbreChave {
                    j += 1;
                }
                let preludio = &t[inicio..j.min(t.len())];
                if j < t.len() && t[j] == Token::AbreChave {
                    let fim = fim_bloco(t, j);
                    let bloco = &t[j + 1..fim.min(t.len())];
                    match nome.as_str() {
                        "media" if avaliar_midia(preludio, m) => ler_regras(bloco, m, f, false),
                        "supports" if avaliar_suporte(preludio) => ler_regras(bloco, m, f, false),
                        "layer" | "container" | "scope" | "document" | "-moz-document" | "starting-style" => {
                            ler_regras(bloco, m, f, false)
                        }
                        "font-face" => f.fontes.push(fonte_web(bloco)),
                        _ => {}
                    }
                    i = fim + 1;
                } else {
                    if nome == "import" && topo {
                        let mut l = Leitor::novo(preludio);
                        let url = match l.proximo() {
                            Some(Token::Url(u)) | Some(Token::Texto(u)) => Some(u.clone()),
                            Some(Token::Funcao(fu)) if fu == "url" => l.argumentos().and_then(|a| {
                                a.iter().find_map(|x| if let Token::Texto(s) = x { Some(s.clone()) } else { None })
                            }),
                            _ => None,
                        };
                        let resto: Vec<Token> = l.resto().to_vec();
                        if let Some(u) = url {
                            let cond: Vec<Token> =
                                resto.into_iter().filter(|x| !matches!(x, Token::Funcao(_))).collect();
                            if cond.iter().all(|x| *x == Token::Espaco) || avaliar_midia(&cond, m) {
                                f.importar.push(u);
                            }
                        }
                    }
                    i = j + 1;
                }
            }
            _ => {
                // regra comum: seletores { declaracoes }
                let inicio = i;
                let mut j = i;
                while j < t.len() && t[j] != Token::AbreChave {
                    j += 1;
                }
                if j >= t.len() {
                    return;
                }
                let fim = fim_bloco(t, j);
                let prel = &t[inicio..j];
                let bloco = &t[j + 1..fim.min(t.len())];
                if let Some(seletores) = seletor::ler_lista(prel) {
                    let declaracoes = ler_declaracoes(bloco);
                    if !declaracoes.is_empty() {
                        f.regras.push(Regra { seletores, declaracoes: Rc::new(declaracoes) });
                    }
                }
                i = fim + 1;
            }
        }
    }
}

fn fonte_web(bloco: &[Token]) -> FonteWeb {
    let mut fw = FonteWeb { peso: 400, ..Default::default() };
    for d in ler_declaracoes(bloco) {
        match d.nome.as_str() {
            "font-family" => {
                fw.familia = d.valor.iter().filter_map(|x| match x {
                    Token::Texto(s) | Token::Ident(s) => Some(s.clone()),
                    _ => None,
                }).collect::<Vec<_>>().join(" ");
            }
            "src" => {
                for x in &d.valor {
                    if let Token::Url(u) | Token::Texto(u) = x {
                        fw.urls.push(u.clone());
                    }
                }
            }
            "font-weight" => {
                if let Some(Token::Numero(v, _)) = d.valor.first() {
                    fw.peso = *v as u16;
                } else if matches!(d.valor.first(), Some(Token::Ident(s)) if s == "bold") {
                    fw.peso = 700;
                }
            }
            "font-style" => fw.italico = matches!(d.valor.first(), Some(Token::Ident(s)) if s == "italic"),
            _ => {}
        }
    }
    fw
}

// ------------------------------------------------------------- @media

/// "screen and (min-width: 600px), print"
pub fn avaliar_midia(t: &[Token], m: &Midia) -> bool {
    if t.iter().all(|x| *x == Token::Espaco) {
        return true;
    }
    t.split(|x| *x == Token::Virgula).any(|consulta| avaliar_consulta(consulta, m))
}

fn avaliar_consulta(t: &[Token], m: &Midia) -> bool {
    let mut negar = false;
    let mut ok = true;
    let mut i = 0;
    while i < t.len() {
        match &t[i] {
            Token::Espaco => {}
            Token::Ident(s) => match s.to_ascii_lowercase().as_str() {
                "not" => negar = true,
                "only" | "and" => {}
                "or" => {
                    if ok {
                        return !negar;
                    }
                    ok = true;
                }
                "screen" | "all" => {}
                _ => ok = false, // print, speech, tv...
            },
            Token::AbreParen => {
                let inicio = i + 1;
                let mut nivel = 1;
                let mut j = inicio;
                while j < t.len() {
                    match t[j] {
                        Token::AbreParen | Token::Funcao(_) => nivel += 1,
                        Token::FechaParen => {
                            nivel -= 1;
                            if nivel == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
                ok &= avaliar_recurso(&t[inicio..j.min(t.len())], m);
                i = j;
            }
            _ => {}
        }
        i += 1;
    }
    ok != negar
}

fn px_de(t: &[Token], m: &Midia) -> Option<f32> {
    let mut l = Leitor::novo(t);
    let c = ler_comprimento(&mut l, false)?;
    let ctx = Contexto { fonte: 16.0, fonte_raiz: 16.0, largura_tela: m.largura, altura_tela: m.altura };
    Some(c.calcular(&ctx).px as f32 / 64.0)
}

fn avaliar_recurso(t: &[Token], m: &Midia) -> bool {
    // (not (...)) e (a) and (b) aninhados
    if t.iter().any(|x| *x == Token::AbreParen) {
        return avaliar_consulta(t, m);
    }
    // forma de faixa: (width >= 600px), (400px < width <= 800px)
    let ops: Vec<usize> = t.iter().enumerate().filter(|(_, x)| matches!(x, Token::Delim('<' | '>' | '='))).map(|(k, _)| k).collect();
    if !ops.is_empty() {
        return avaliar_faixa(t, m);
    }
    let mut l = Leitor::novo(t);
    let nome = match l.ident() {
        Some(n) => n,
        None => return false,
    };
    let valor = if l.espiar() == Some(&Token::DoisPontos) {
        l.proximo();
        Some(l.resto())
    } else {
        None
    };
    let largura = m.largura;
    let altura = m.altura;
    let palavra = valor.and_then(|v| Leitor::novo(v).ident());
    match nome.as_str() {
        "min-width" => valor.and_then(|v| px_de(v, m)).is_some_and(|v| largura >= v),
        "max-width" => valor.and_then(|v| px_de(v, m)).is_some_and(|v| largura <= v),
        "width" => valor.and_then(|v| px_de(v, m)).is_some_and(|v| (largura - v).abs() < 0.5),
        "min-height" => valor.and_then(|v| px_de(v, m)).is_some_and(|v| altura >= v),
        "max-height" => valor.and_then(|v| px_de(v, m)).is_some_and(|v| altura <= v),
        "min-device-width" => valor.and_then(|v| px_de(v, m)).is_some_and(|v| largura >= v),
        "max-device-width" => valor.and_then(|v| px_de(v, m)).is_some_and(|v| largura <= v),
        "orientation" => match palavra.as_deref() {
            Some("landscape") => largura >= altura,
            Some("portrait") => largura < altura,
            _ => false,
        },
        "prefers-color-scheme" => palavra.as_deref() == Some("light"),
        "prefers-reduced-motion" => palavra.as_deref() == Some("reduce"),
        "prefers-contrast" | "forced-colors" | "inverted-colors" => palavra.as_deref() == Some("no-preference") || palavra.as_deref() == Some("none"),
        "hover" | "any-hover" => palavra.as_deref() == Some("hover") || valor.is_none(),
        "pointer" | "any-pointer" => palavra.as_deref() == Some("fine") || valor.is_none(),
        "color" => true,
        "monochrome" | "grid" => false,
        "min-resolution" | "-webkit-min-device-pixel-ratio" | "min--moz-device-pixel-ratio" => {
            valor.is_some_and(|v| match Leitor::novo(v).proximo() {
                Some(Token::Numero(n, _)) => *n <= 1.0,
                Some(Token::Dimensao(n, u)) => (u == "dppx" || u == "x") && *n <= 1.0 || u == "dpi" && *n <= 96.0,
                _ => false,
            })
        }
        "max-resolution" | "-webkit-max-device-pixel-ratio" => true,
        "display-mode" => palavra.as_deref() == Some("browser"),
        "scripting" => palavra.as_deref() == Some("enabled"),
        "update" => palavra.as_deref() == Some("fast"),
        _ => false,
    }
}

fn avaliar_faixa(t: &[Token], m: &Midia) -> bool {
    // junta os tokens em lados e operadores
    let mut lados: Vec<Vec<Token>> = alloc::vec![Vec::new()];
    let mut ops: Vec<String> = Vec::new();
    let mut k = 0;
    while k < t.len() {
        match &t[k] {
            Token::Delim(c @ ('<' | '>' | '=')) => {
                let mut op = String::new();
                op.push(*c);
                if t.get(k + 1) == Some(&Token::Delim('=')) {
                    op.push('=');
                    k += 1;
                }
                ops.push(op);
                lados.push(Vec::new());
            }
            x => {
                if let Some(ultimo) = lados.last_mut() {
                    ultimo.push(x.clone());
                }
            }
        }
        k += 1;
    }
    let valor = |lado: &[Token]| -> Option<f32> {
        match Leitor::novo(lado).ident().as_deref() {
            Some("width") => Some(m.largura),
            Some("height") => Some(m.altura),
            Some("aspect-ratio") => Some(m.largura / m.altura.max(1.0)),
            _ => px_de(lado, m),
        }
    };
    for (i, op) in ops.iter().enumerate() {
        let (Some(a), Some(b)) = (valor(&lados[i]), valor(&lados[i + 1])) else { return false };
        let ok = match op.as_str() {
            "<" => a < b,
            "<=" => a <= b,
            ">" => a > b,
            ">=" => a >= b,
            "=" => (a - b).abs() < 0.5,
            _ => false,
        };
        if !ok {
            return false;
        }
    }
    true
}

/// @supports: aceita quase tudo que tiver a forma certa (o motor ignora o
/// que nao entende), recusa so "not (...)" de coisas que temos.
fn avaliar_suporte(t: &[Token]) -> bool {
    let primeiro = t.iter().find(|x| **x != Token::Espaco);
    match primeiro {
        Some(Token::Ident(s)) if s.eq_ignore_ascii_case("not") => {
            // not (display: grid) etc: nos suportamos, entao falso
            let texto: String = t.iter().filter_map(|x| if let Token::Ident(s) = x { Some(s.as_str()) } else { None }).collect::<Vec<_>>().join(" ");
            !(texto.contains("grid") || texto.contains("flex") || texto.contains("gap") || texto.contains("var") || texto.contains("sticky"))
        }
        _ => {
            let texto: String = t.iter().filter_map(|x| if let Token::Ident(s) = x { Some(s.as_str()) } else { None }).collect::<Vec<_>>().join(" ");
            // coisas que nao temos: backdrop-filter, container queries de estilo, :has em alguns
            !(texto.contains("backdrop-filter") || texto.contains("-webkit-touch-callout") || texto.contains("hanging-punctuation"))
        }
    }
}
