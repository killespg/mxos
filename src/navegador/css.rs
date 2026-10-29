//! CSS leve: le folhas de estilo (<style>, style="..." e <link>), casa os
//! seletores com a pilha de elementos abertos e calcula o estilo de cada
//! elemento (cascata por especificidade e ordem, com !important).
//!
//! Seletores: tipo (p), .classe, #id, *, descendente (div p), filho
//! (ul > li), lista com virgula e a:link/a:visited (viram a). Qualquer
//! outra coisa (atributo, :hover, ::before, + e ~) descarta a regra.
//!
//! Propriedades: color, background(-color), font-weight, font-style,
//! font-size, font-family (so monospace ou nao), font (atalho),
//! text-align, text-decoration, line-height, margin, padding, border,
//! width, max-width, display (none, block, inline) e list-style.
//! @media, @font-face e cia sao pulados inteiros.

use alloc::string::String;
use alloc::vec::Vec;

/// 0x00RRGGBB, como gui::grafico::Cor.
pub type Cor = u32;

/// Regras demais (paginas enormes) custam caro para casar: o resto e
/// ignorado.
const MAX_REGRAS: usize = 3000;
/// Tamanho da letra do texto normal (o "medium" da pagina).
pub const PX_NORMAL: i32 = 13;

// ---------------------------------------------------------------------
// Valores

/// Medida ainda sem resolver: em e % dependem da letra ou da largura de
/// quem contem. Em milesimos (1.5em = Em(1500)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Medida {
    Px(i32),
    Em(i32),
    Porcento(i32),
    /// Numero sem unidade (line-height: 1.5).
    Numero(i32),
    Auto,
}

impl Medida {
    /// Em px, com `letra` px por em e `base` px valendo 100%.
    pub fn px(self, letra: i32, base: i32) -> Option<i32> {
        match self {
            Medida::Px(v) => Some(v),
            Medida::Em(m) => Some(letra * m / 1000),
            Medida::Porcento(m) => Some(base * m / 100_000),
            Medida::Numero(m) => Some(letra * m / 1000),
            Medida::Auto => None,
        }
    }
}

/// Lados na ordem do CSS: cima, direita, baixo, esquerda.
pub const CIMA: usize = 0;
pub const DIREITA: usize = 1;
pub const BAIXO: usize = 2;
pub const ESQUERDA: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exibir {
    /// O que a tag ja faz (p e bloco, span e linha...).
    Padrao,
    Bloco,
    Linha,
    Nenhum,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Prop {
    Cor,
    Fundo,
    Negrito,
    Italico,
    Tamanho,
    Mono,
    Alinhar,
    Sublinhado,
    AlturaLinha,
    Margem(usize),
    Recheio(usize),
    BordaLargura(usize),
    BordaEstilo(usize),
    BordaCor(usize),
    Largura,
    LarguraMax,
    Exibir,
    SemMarcador,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Valor {
    /// None = transparente.
    Cor(Option<Cor>),
    Sim(bool),
    Medida(Medida),
    /// text-align: 0 esquerda, 1 centro, 2 direita.
    Alinhar(u8),
    Exibir(Exibir),
}

#[derive(Clone, Copy, Debug)]
struct Declaracao {
    prop: Prop,
    valor: Valor,
    importante: bool,
}

// ---------------------------------------------------------------------
// Seletores

/// Um seletor simples composto: "div.a.b#c".
#[derive(Clone, Debug, Default)]
struct Composto {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Combinador {
    Descendente,
    Filho,
}

#[derive(Clone, Debug)]
struct Seletor {
    /// Da esquerda para a direita; o combinador liga a parte a anterior.
    partes: Vec<(Composto, Combinador)>,
    especificidade: u32,
}

struct Regra {
    seletor: Seletor,
    declaracoes: Vec<Declaracao>,
    ordem: u32,
}

/// O que um elemento aberto tem para casar com os seletores.
#[derive(Clone, Debug, Default)]
pub struct Marca {
    pub tag: String,
    pub id: String,
    pub classes: Vec<String>,
}

impl Composto {
    fn casa(&self, e: &Marca) -> bool {
        if self.tag.as_deref().is_some_and(|t| t != e.tag) {
            return false;
        }
        if self.id.as_deref().is_some_and(|i| i != e.id) {
            return false;
        }
        self.classes.iter().all(|c| e.classes.iter().any(|x| x == c))
    }
}

/// `partes` casa com `pilha`, com a ultima parte no ultimo elemento.
fn casa(partes: &[(Composto, Combinador)], pilha: &[Marca]) -> bool {
    let (Some((ultima, combinador)), Some(elemento)) = (partes.last(), pilha.last()) else {
        return false;
    };
    if !ultima.casa(elemento) {
        return false;
    }
    let resto = &partes[..partes.len() - 1];
    if resto.is_empty() {
        return true;
    }
    let acima = &pilha[..pilha.len() - 1];
    match combinador {
        Combinador::Filho => casa(resto, acima),
        Combinador::Descendente => (1..=acima.len()).rev().any(|n| casa(resto, &acima[..n])),
    }
}

// ---------------------------------------------------------------------
// Folha de estilos

#[derive(Default)]
pub struct Folha {
    regras: Vec<Regra>,
    ordem: u32,
}

impl Folha {
    pub fn nova() -> Folha {
        Folha::default()
    }

    pub fn vazia(&self) -> bool {
        self.regras.is_empty()
    }

    /// Acrescenta as regras de um texto CSS (as de depois ganham empates).
    pub fn ler(&mut self, css: &str) {
        let texto = sem_comentarios(css);
        let b = texto.as_bytes();
        let mut i = 0;
        while i < b.len() && self.regras.len() < MAX_REGRAS {
            while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b';' || b[i] == b'}') {
                i += 1;
            }
            if i >= b.len() {
                break;
            }
            if b[i] == b'@' {
                i = pular_regra_arroba(b, i);
                continue;
            }
            let Some(abre) = texto[i..].find('{').map(|p| i + p) else { break };
            let fecha = fim_do_bloco(b, abre);
            let seletores = &texto[i..abre];
            let corpo = &texto[abre + 1..fecha.min(texto.len())];
            i = fecha + 1;
            let declaracoes = ler_declaracoes(corpo);
            if declaracoes.is_empty() {
                continue;
            }
            for seletor in seletores.split(',').filter_map(ler_seletor) {
                self.ordem += 1;
                let declaracoes = declaracoes.clone();
                self.regras.push(Regra { seletor, declaracoes, ordem: self.ordem });
            }
        }
    }

    /// As declaracoes que valem para o ultimo elemento da `pilha`, ja na
    /// ordem de aplicar (a mais forte por ultimo).
    fn casadas(&self, pilha: &[Marca], inline: &[Declaracao]) -> Vec<(u64, Declaracao)> {
        let mut lista: Vec<(u64, Declaracao)> = Vec::new();
        for r in &self.regras {
            if !casa(&r.seletor.partes, pilha) {
                continue;
            }
            for d in &r.declaracoes {
                lista.push((peso(d.importante, r.seletor.especificidade, r.ordem), *d));
            }
        }
        // style="..." ganha de qualquer seletor (menos de !important)
        for d in inline {
            lista.push((peso(d.importante, 1_000_000, u32::MAX), *d));
        }
        lista.sort_by_key(|&(p, _)| p);
        lista
    }
}

/// Ordem da cascata: !important, especificidade e por fim a ordem no texto.
fn peso(importante: bool, especificidade: u32, ordem: u32) -> u64 {
    ((importante as u64) << 63) | ((especificidade as u64) << 32) | ordem as u64
}

/// Tira /* comentarios */ (e o <!-- --> antigo de dentro do <style>).
fn sem_comentarios(css: &str) -> String {
    let mut saida = String::with_capacity(css.len());
    let mut resto = css;
    while let Some(i) = resto.find("/*") {
        saida.push_str(&resto[..i]);
        resto = resto[i + 2..].find("*/").map_or("", |f| &resto[i + 2 + f + 2..]);
        saida.push(' ');
    }
    saida.push_str(resto);
    saida.replace("<!--", " ").replace("-->", " ")
}

/// Posicao do '}' que fecha o '{' em `abre` (ou o fim do texto), pulando
/// textos entre aspas e blocos de dentro.
fn fim_do_bloco(b: &[u8], abre: usize) -> usize {
    let mut nivel = 0;
    let mut aspa = 0u8;
    for (i, &c) in b.iter().enumerate().skip(abre) {
        if aspa != 0 {
            if c == aspa {
                aspa = 0;
            }
            continue;
        }
        match c {
            b'"' | b'\'' => aspa = c,
            b'{' => nivel += 1,
            b'}' => {
                nivel -= 1;
                if nivel == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    b.len()
}

/// "@import ...;" ou "@media ... { ... }": devolve onde a regra acaba.
fn pular_regra_arroba(b: &[u8], inicio: usize) -> usize {
    for (i, &c) in b.iter().enumerate().skip(inicio) {
        match c {
            b';' => return i + 1,
            b'{' => return fim_do_bloco(b, i) + 1,
            _ => {}
        }
    }
    b.len()
}

/// "div.a > p, ..." (um so, sem a virgula). None se tiver algo que nao
/// sabemos casar.
fn ler_seletor(texto: &str) -> Option<Seletor> {
    let texto = texto.trim();
    if texto.is_empty() {
        return None;
    }
    let mut partes: Vec<(Composto, Combinador)> = Vec::new();
    let mut proximo = Combinador::Descendente;
    let (mut ids, mut classes, mut tags) = (0u32, 0u32, 0u32);
    for pedaco in separar_seletor(texto) {
        if pedaco == ">" {
            proximo = Combinador::Filho;
            continue;
        }
        let composto = ler_composto(pedaco)?;
        ids += composto.id.is_some() as u32;
        classes += composto.classes.len() as u32;
        tags += composto.tag.is_some() as u32;
        partes.push((composto, proximo));
        proximo = Combinador::Descendente;
    }
    if partes.is_empty() || proximo == Combinador::Filho {
        return None;
    }
    Some(Seletor { partes, especificidade: ids * 10_000 + classes * 100 + tags })
}

/// Separa "div>p  .a" em ["div", ">", "p", ".a"].
fn separar_seletor(texto: &str) -> Vec<&str> {
    let mut pedacos = Vec::new();
    let mut inicio = None;
    for (i, c) in texto.char_indices() {
        if c.is_ascii_whitespace() || c == '>' {
            if let Some(s) = inicio.take() {
                pedacos.push(&texto[s..i]);
            }
            if c == '>' {
                pedacos.push(">");
            }
        } else if inicio.is_none() {
            inicio = Some(i);
        }
    }
    if let Some(s) = inicio {
        pedacos.push(&texto[s..]);
    }
    pedacos
}

/// "a.b#c", "*", "a:link". None se tiver atributo, pseudo-classe que nao
/// e :link/:visited, pseudo-elemento ou combinador + e ~.
fn ler_composto(texto: &str) -> Option<Composto> {
    let mut c = Composto::default();
    let mut resto = texto;
    let nome = |s: &str| s.find(['.', '#', ':', '[']).unwrap_or(s.len());
    let fim = nome(resto);
    match &resto[..fim] {
        "" | "*" => {}
        tag if valido(tag) => c.tag = Some(tag.to_ascii_lowercase()),
        _ => return None,
    }
    resto = &resto[fim..];
    while let Some(primeiro) = resto.chars().next() {
        let corpo = &resto[1..];
        let fim = nome(corpo);
        let valor = &corpo[..fim];
        match primeiro {
            '.' if valido(valor) => c.classes.push(valor.into()),
            '#' if valido(valor) => c.id = Some(valor.into()),
            ':' if matches!(valor, "link" | "visited") && c.tag.as_deref().is_none_or(|t| t == "a") => {
                c.tag = Some("a".into());
            }
            _ => return None,
        }
        resto = &corpo[fim..];
    }
    Some(c)
}

fn valido(nome: &str) -> bool {
    !nome.is_empty() && nome.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c >= 0x80)
}

// ---------------------------------------------------------------------
// Declaracoes

/// "color: red; margin: 0 auto !important" -> declaracoes (atalhos ja
/// abertos em propriedades simples).
pub fn ler_inline(texto: &str) -> Vec<DeclaracaoInline> {
    ler_declaracoes(&sem_comentarios(texto)).into_iter().map(DeclaracaoInline).collect()
}

/// Declaracoes de um style="..." (opacas para quem esta fora).
#[derive(Clone, Copy, Debug)]
pub struct DeclaracaoInline(Declaracao);

fn ler_declaracoes(corpo: &str) -> Vec<Declaracao> {
    let mut saida = Vec::new();
    for item in separar_declaracoes(corpo) {
        let Some((nome, valor)) = item.split_once(':') else { continue };
        let nome = nome.trim().to_ascii_lowercase();
        let mut valor = valor.trim();
        let mut importante = false;
        if let Some(i) = valor.to_ascii_lowercase().rfind("!important") {
            importante = true;
            valor = valor[..i].trim();
        }
        let valor = valor.to_ascii_lowercase();
        let mut empurrar = |prop: Prop, v: Valor| saida.push(Declaracao { prop, valor: v, importante });
        propriedade(&nome, &valor, &mut empurrar);
    }
    saida
}

/// Separa por ';' fora de aspas e parenteses (url(data:...;base64,...)).
fn separar_declaracoes(corpo: &str) -> Vec<&str> {
    let mut partes = Vec::new();
    let (mut inicio, mut parenteses, mut aspa) = (0, 0i32, 0u8);
    for (i, &c) in corpo.as_bytes().iter().enumerate() {
        if aspa != 0 {
            if c == aspa {
                aspa = 0;
            }
            continue;
        }
        match c {
            b'"' | b'\'' => aspa = c,
            b'(' => parenteses += 1,
            b')' => parenteses -= 1,
            b';' if parenteses <= 0 => {
                partes.push(&corpo[inicio..i]);
                inicio = i + 1;
            }
            _ => {}
        }
    }
    partes.push(&corpo[inicio..]);
    partes
}

/// Uma propriedade CSS -> declaracoes simples.
fn propriedade(nome: &str, valor: &str, empurrar: &mut dyn FnMut(Prop, Valor)) {
    let palavras: Vec<&str> = valor.split_ascii_whitespace().collect();
    match nome {
        "color" => {
            if let Some(c) = ler_cor(valor) {
                empurrar(Prop::Cor, Valor::Cor(c));
            }
        }
        "background-color" | "background" => {
            // no atalho a cor e qualquer palavra que seja cor
            let cor = if nome == "background-color" { ler_cor(valor) } else { cor_do_atalho(valor) };
            if let Some(c) = cor {
                empurrar(Prop::Fundo, Valor::Cor(c));
            }
        }
        "font-weight" => {
            if let Some(n) = negrito(valor) {
                empurrar(Prop::Negrito, Valor::Sim(n));
            }
        }
        "font-style" => empurrar(Prop::Italico, Valor::Sim(valor != "normal")),
        "font-size" => {
            if let Some(m) = tamanho_da_letra(valor) {
                empurrar(Prop::Tamanho, Valor::Medida(m));
            }
        }
        "font-family" => empurrar(Prop::Mono, Valor::Sim(e_mono(valor))),
        "font" => atalho_font(&palavras, valor, empurrar),
        "text-align" => {
            let lado = match valor {
                "center" => 1,
                "right" | "end" => 2,
                _ => 0,
            };
            empurrar(Prop::Alinhar, Valor::Alinhar(lado));
        }
        "text-decoration" | "text-decoration-line" => {
            empurrar(Prop::Sublinhado, Valor::Sim(palavras.contains(&"underline")));
        }
        "line-height" => {
            let m = if valor == "normal" { Some(Medida::Auto) } else { ler_medida(valor) };
            if let Some(m) = m {
                empurrar(Prop::AlturaLinha, Valor::Medida(m));
            }
        }
        "margin" | "padding" => {
            let lados = |i| if nome == "margin" { Prop::Margem(i) } else { Prop::Recheio(i) };
            if let Some(m) = quatro(&palavras, ler_medida) {
                for (i, v) in m.into_iter().enumerate() {
                    empurrar(lados(i), Valor::Medida(v));
                }
            }
        }
        "width" | "max-width" => {
            let m = if valor == "none" { Some(Medida::Auto) } else { ler_medida(valor) };
            if let Some(m) = m {
                empurrar(if nome == "width" { Prop::Largura } else { Prop::LarguraMax }, Valor::Medida(m));
            }
        }
        "display" => {
            let e = match valor {
                "none" => Exibir::Nenhum,
                "inline" | "inline-block" | "inline-flex" => Exibir::Linha,
                "block" | "flex" | "grid" | "list-item" | "table" => Exibir::Bloco,
                _ => return,
            };
            empurrar(Prop::Exibir, Valor::Exibir(e));
        }
        "list-style" | "list-style-type" => empurrar(Prop::SemMarcador, Valor::Sim(palavras.contains(&"none"))),
        _ => bordas(nome, &palavras, empurrar),
    }
}

/// border, border-top..., border-width/style/color e border-top-width...
fn bordas(nome: &str, palavras: &[&str], empurrar: &mut dyn FnMut(Prop, Valor)) {
    let Some(resto) = nome.strip_prefix("border") else { return };
    let (lados, parte): (&[usize], &str) = match resto {
        "" => (&[CIMA, DIREITA, BAIXO, ESQUERDA], ""),
        "-width" => (&[], "width"),
        "-style" => (&[], "style"),
        "-color" => (&[], "color"),
        _ => {
            let resto = &resto[1..];
            let (lado, parte) = resto.split_once('-').unwrap_or((resto, ""));
            let i = match lado {
                "top" => CIMA,
                "right" => DIREITA,
                "bottom" => BAIXO,
                "left" => ESQUERDA,
                _ => return,
            };
            (&[CIMA, DIREITA, BAIXO, ESQUERDA][i..=i], parte)
        }
    };
    if lados.is_empty() {
        // border-width: 1px 2px (de 1 a 4 valores, um por lado)
        match parte {
            "width" => {
                if let Some(m) = quatro(palavras, largura_de_borda) {
                    for (i, v) in m.into_iter().enumerate() {
                        empurrar(Prop::BordaLargura(i), Valor::Medida(v));
                    }
                }
            }
            "style" => {
                if let Some(m) = quatro(palavras, |p| Some(estilo_de_borda(p))) {
                    for (i, v) in m.into_iter().enumerate() {
                        empurrar(Prop::BordaEstilo(i), Valor::Sim(v));
                    }
                }
            }
            _ => {
                if let Some(m) = quatro(palavras, ler_cor) {
                    for (i, v) in m.into_iter().enumerate() {
                        empurrar(Prop::BordaCor(i), Valor::Cor(v));
                    }
                }
            }
        }
        return;
    }
    for &lado in lados {
        match parte {
            "width" => {
                if let Some(m) = palavras.first().and_then(|p| largura_de_borda(p)) {
                    empurrar(Prop::BordaLargura(lado), Valor::Medida(m));
                }
            }
            "style" => empurrar(Prop::BordaEstilo(lado), Valor::Sim(palavras.first().is_some_and(|p| estilo_de_borda(p)))),
            "color" => {
                if let Some(c) = palavras.first().and_then(|p| ler_cor(p)) {
                    empurrar(Prop::BordaCor(lado), Valor::Cor(c));
                }
            }
            "" => atalho_borda(lado, palavras, empurrar),
            _ => {}
        }
    }
}

/// "1px solid #ccc" (em qualquer ordem; o que faltar volta ao padrao).
fn atalho_borda(lado: usize, palavras: &[&str], empurrar: &mut dyn FnMut(Prop, Valor)) {
    let (mut largura, mut estilo, mut cor) = (Medida::Px(3), false, None);
    for p in palavras {
        if let Some(m) = largura_de_borda(p) {
            largura = m;
        } else if let Some(c) = ler_cor(p) {
            cor = c;
        } else {
            estilo = estilo_de_borda(p);
        }
    }
    empurrar(Prop::BordaLargura(lado), Valor::Medida(largura));
    empurrar(Prop::BordaEstilo(lado), Valor::Sim(estilo));
    // sem cor: a cor do texto (resolvida no desenho)
    empurrar(Prop::BordaCor(lado), Valor::Cor(cor));
}

fn largura_de_borda(p: &str) -> Option<Medida> {
    match p {
        "thin" => Some(Medida::Px(1)),
        "medium" => Some(Medida::Px(3)),
        "thick" => Some(Medida::Px(5)),
        _ => ler_medida(p).filter(|m| !matches!(m, Medida::Auto | Medida::Porcento(_))),
    }
}

fn estilo_de_borda(p: &str) -> bool {
    matches!(p, "solid" | "dashed" | "dotted" | "double" | "groove" | "ridge" | "inset" | "outset")
}

/// Atalho de 1 a 4 valores (cima, direita, baixo, esquerda).
fn quatro<T: Copy>(palavras: &[&str], ler: impl Fn(&str) -> Option<T>) -> Option<[T; 4]> {
    let v: Vec<T> = palavras.iter().map(|p| ler(p)).collect::<Option<Vec<T>>>()?;
    Some(match v.len() {
        1 => [v[0]; 4],
        2 => [v[0], v[1], v[0], v[1]],
        3 => [v[0], v[1], v[2], v[1]],
        4 => [v[0], v[1], v[2], v[3]],
        _ => return None,
    })
}

/// "font: italic bold 14px/1.5 Georgia, serif".
fn atalho_font(palavras: &[&str], valor: &str, empurrar: &mut dyn FnMut(Prop, Valor)) {
    let mut negrito_achado = false;
    for (i, p) in palavras.iter().enumerate() {
        if matches!(*p, "italic" | "oblique") {
            empurrar(Prop::Italico, Valor::Sim(true));
        } else if let Some(n) = negrito(p).filter(|_| *p != "normal") {
            empurrar(Prop::Negrito, Valor::Sim(n));
            negrito_achado = true;
        } else if let Some(m) = p.split('/').next().and_then(tamanho_da_letra) {
            empurrar(Prop::Tamanho, Valor::Medida(m));
            if let Some(altura) = p.split_once('/').and_then(|(_, a)| ler_medida(a)) {
                empurrar(Prop::AlturaLinha, Valor::Medida(altura));
            }
            // depois do tamanho vem a familia
            let familia = palavras[i + 1..].join(" ");
            empurrar(Prop::Mono, Valor::Sim(e_mono(&familia)));
            break;
        }
    }
    if !negrito_achado && !valor.is_empty() {
        empurrar(Prop::Negrito, Valor::Sim(false));
    }
}

fn negrito(valor: &str) -> Option<bool> {
    match valor {
        "bold" | "bolder" => Some(true),
        "normal" | "lighter" => Some(false),
        n => n.parse::<u32>().ok().map(|n| n >= 600),
    }
}

fn e_mono(familia: &str) -> bool {
    ["monospace", "mono", "courier", "consolas", "menlo", "monaco"].iter().any(|m| familia.contains(m))
}

fn tamanho_da_letra(valor: &str) -> Option<Medida> {
    let px = match valor {
        "xx-small" | "x-small" | "small" | "smaller" | "medium" => PX_NORMAL,
        "large" | "larger" => 18,
        "x-large" => 22,
        "xx-large" | "xxx-large" => 32,
        _ => return ler_medida(valor).filter(|m| *m != Medida::Auto),
    };
    Some(Medida::Px(px))
}

/// "12px", "1.5em", "2rem", "50%", "0", "auto", "1.4", "12pt".
fn ler_medida(valor: &str) -> Option<Medida> {
    let valor = valor.trim();
    if valor == "auto" {
        return Some(Medida::Auto);
    }
    let fim = valor.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+')).unwrap_or(valor.len());
    let milesimos = ler_milesimos(&valor[..fim])?;
    let px = |m: i32| Some(Medida::Px(m / 1000));
    match &valor[fim..] {
        "px" => px(milesimos),
        "pt" => px(milesimos * 4 / 3),
        "em" | "rem" => Some(Medida::Em(milesimos)),
        "%" => Some(Medida::Porcento(milesimos)),
        "" if milesimos == 0 => Some(Medida::Px(0)),
        "" => Some(Medida::Numero(milesimos)),
        // vw, vh, ch...: uma conta aproximada vale mais que nada
        "ch" | "ex" => Some(Medida::Em(milesimos / 2)),
        // a area da pagina tem uns 800 x 500 px
        "vw" => px(milesimos * 8),
        "vh" => px(milesimos * 5),
        _ => None,
    }
}

/// "1.25" -> 1250 (tres casas, sem ponto flutuante).
fn ler_milesimos(texto: &str) -> Option<i32> {
    let (negativo, texto) = match texto.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, texto.strip_prefix('+').unwrap_or(texto)),
    };
    let (inteiro, fracao) = texto.split_once('.').unwrap_or((texto, ""));
    if inteiro.is_empty() && fracao.is_empty() {
        return None;
    }
    let mut v: i64 = if inteiro.is_empty() { 0 } else { inteiro.parse::<i64>().ok()? } * 1000;
    let mut casa = 100;
    for c in fracao.bytes().take(3) {
        if !c.is_ascii_digit() {
            return None;
        }
        v += (c - b'0') as i64 * casa;
        casa /= 10;
    }
    let v = v.min(10_000_000) as i32;
    Some(if negativo { -v } else { v })
}

// ---------------------------------------------------------------------
// Cores

const NOMES: &[(&str, Cor)] = &[
    ("black", 0x000000),
    ("white", 0xFFFFFF),
    ("red", 0xFF0000),
    ("green", 0x008000),
    ("blue", 0x0000FF),
    ("yellow", 0xFFFF00),
    ("orange", 0xFFA500),
    ("purple", 0x800080),
    ("gray", 0x808080),
    ("grey", 0x808080),
    ("silver", 0xC0C0C0),
    ("maroon", 0x800000),
    ("navy", 0x000080),
    ("teal", 0x008080),
    ("olive", 0x808000),
    ("lime", 0x00FF00),
    ("aqua", 0x00FFFF),
    ("cyan", 0x00FFFF),
    ("fuchsia", 0xFF00FF),
    ("magenta", 0xFF00FF),
    ("pink", 0xFFC0CB),
    ("brown", 0xA52A2A),
    ("gold", 0xFFD700),
    ("crimson", 0xDC143C),
    ("coral", 0xFF7F50),
    ("tomato", 0xFF6347),
    ("salmon", 0xFA8072),
    ("indigo", 0x4B0082),
    ("violet", 0xEE82EE),
    ("beige", 0xF5F5DC),
    ("ivory", 0xFFFFF0),
    ("khaki", 0xF0E68C),
    ("lavender", 0xE6E6FA),
    ("tan", 0xD2B48C),
    ("darkred", 0x8B0000),
    ("darkgreen", 0x006400),
    ("darkblue", 0x00008B),
    ("darkgray", 0xA9A9A9),
    ("darkgrey", 0xA9A9A9),
    ("darkorange", 0xFF8C00),
    ("lightgray", 0xD3D3D3),
    ("lightgrey", 0xD3D3D3),
    ("lightblue", 0xADD8E6),
    ("lightgreen", 0x90EE90),
    ("lightyellow", 0xFFFFE0),
    ("whitesmoke", 0xF5F5F5),
    ("gainsboro", 0xDCDCDC),
    ("dimgray", 0x696969),
    ("dimgrey", 0x696969),
    ("steelblue", 0x4682B4),
    ("royalblue", 0x4169E1),
    ("skyblue", 0x87CEEB),
    ("dodgerblue", 0x1E90FF),
    ("seagreen", 0x2E8B57),
    ("forestgreen", 0x228B22),
    ("slategray", 0x708090),
    ("slategrey", 0x708090),
    ("aliceblue", 0xF0F8FF),
    ("honeydew", 0xF0FFF0),
    ("mintcream", 0xF5FFFA),
    ("snow", 0xFFFAFA),
    ("linen", 0xFAF0E6),
    ("wheat", 0xF5DEB3),
    ("chocolate", 0xD2691E),
    ("firebrick", 0xB22222),
    ("goldenrod", 0xDAA520),
    ("orchid", 0xDA70D6),
    ("plum", 0xDDA0DD),
    ("turquoise", 0x40E0D0),
    ("midnightblue", 0x191970),
    ("darkslategray", 0x2F4F4F),
    ("darkslategrey", 0x2F4F4F),
    ("rebeccapurple", 0x663399),
];

/// Uma cor CSS. Some(None) = transparente; None = nao e cor.
pub fn ler_cor(valor: &str) -> Option<Option<Cor>> {
    let valor = valor.trim();
    if valor == "transparent" {
        return Some(None);
    }
    // light-dark(clara, escura): o MXOS so tem o tema claro nas paginas
    if let Some(dentro) = valor.strip_prefix("light-dark(") {
        return ler_cor(dentro.split(',').next()?);
    }
    if let Some(hexa) = valor.strip_prefix('#') {
        return hexadecimal(hexa).map(Some);
    }
    if let Some(dentro) = valor.strip_prefix("rgba(").or_else(|| valor.strip_prefix("rgb(")) {
        return rgb(dentro.strip_suffix(')')?);
    }
    NOMES.iter().find(|(n, _)| *n == valor).map(|&(_, c)| Some(c))
}

/// "fff", "ffffff" (e "ffffffaa", sem o alfa).
fn hexadecimal(h: &str) -> Option<Cor> {
    if !h.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    match h.len() {
        3 | 4 => {
            let v = if h.len() == 4 { v >> 4 } else { v };
            let (r, g, b) = ((v >> 8) & 0xF, (v >> 4) & 0xF, v & 0xF);
            Some((r * 0x11) << 16 | (g * 0x11) << 8 | b * 0x11)
        }
        6 => Some(v),
        8 => Some(v >> 8),
        _ => None,
    }
}

/// "10, 20, 30" ou "10 20 30 / 0.5" ou "100%, 0%, 0%". Alfa 0 = transparente.
fn rgb(dentro: &str) -> Option<Option<Cor>> {
    let partes: Vec<&str> = dentro.split([',', ' ', '/']).filter(|p| !p.is_empty()).collect();
    if partes.len() < 3 {
        return None;
    }
    let canal = |p: &str| -> Option<u32> {
        let v = match p.strip_suffix('%') {
            Some(n) => ler_milesimos(n)? * 255 / 100_000,
            None => ler_milesimos(p)? / 1000,
        };
        Some(v.clamp(0, 255) as u32)
    };
    if let Some(alfa) = partes.get(3) {
        let a = match alfa.strip_suffix('%') {
            Some(n) => ler_milesimos(n)? / 100,
            None => ler_milesimos(alfa)?,
        };
        if a <= 0 {
            return Some(None);
        }
    }
    Some(Some(canal(partes[0])? << 16 | canal(partes[1])? << 8 | canal(partes[2])?))
}

/// A cor de um "background: ..." (url, repeat e posicao sao ignorados).
fn cor_do_atalho(valor: &str) -> Option<Option<Cor>> {
    let mut resto = valor;
    // rgb(...) tem espacos dentro: tenta antes de separar as palavras
    if let Some(i) = resto.find("rgb") {
        let fim = resto[i..].find(')').map_or(resto.len(), |f| i + f + 1);
        return ler_cor(&resto[i..fim]);
    }
    if valor == "none" {
        return Some(None);
    }
    while let Some(p) = resto.split_ascii_whitespace().next() {
        if let Some(c) = ler_cor(p) {
            return Some(c);
        }
        resto = resto.trim_start()[p.len()..].trim_start();
    }
    None
}

// ---------------------------------------------------------------------
// Estilo calculado

/// O estilo de um elemento depois da cascata. Os campos Option sao None
/// quando nada no CSS falou deles (vale o que a tag ja faz).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Estilo {
    // herdados
    pub cor: Option<Cor>,
    pub negrito: Option<bool>,
    pub italico: Option<bool>,
    pub mono: Option<bool>,
    pub sublinhado: Option<bool>,
    /// Tamanho da letra pedido pelo CSS (px).
    pub tamanho: Option<i32>,
    /// Tamanho de verdade (com o da tag), base do em.
    pub px: i32,
    pub alinhar: Option<u8>,
    pub altura_linha: Option<Medida>,
    pub sem_marcador: bool,
    pub oculto: bool,
    // nao herdados
    pub fundo: Option<Cor>,
    pub margem: [Option<Medida>; 4],
    pub recheio: [Medida; 4],
    pub borda: [i32; 4],
    pub borda_visivel: [bool; 4],
    pub borda_cor: [Option<Cor>; 4],
    pub largura: Option<Medida>,
    pub largura_max: Option<Medida>,
    pub exibir: Exibir,
}

impl Estilo {
    pub const RAIZ: Estilo = Estilo {
        cor: None,
        negrito: None,
        italico: None,
        mono: None,
        sublinhado: None,
        tamanho: None,
        px: PX_NORMAL,
        alinhar: None,
        altura_linha: None,
        sem_marcador: false,
        oculto: false,
        fundo: None,
        margem: [None; 4],
        recheio: [Medida::Px(0); 4],
        borda: [0; 4],
        borda_visivel: [false; 4],
        borda_cor: [None; 4],
        largura: None,
        largura_max: None,
        exibir: Exibir::Padrao,
    };

    /// O que um filho recebe: so as propriedades herdadas.
    pub fn herdado(&self) -> Estilo {
        Estilo {
            cor: self.cor,
            negrito: self.negrito,
            italico: self.italico,
            mono: self.mono,
            sublinhado: self.sublinhado,
            tamanho: self.tamanho,
            px: self.px,
            alinhar: self.alinhar,
            altura_linha: self.altura_linha,
            sem_marcador: self.sem_marcador,
            oculto: self.oculto,
            ..Estilo::RAIZ
        }
    }

    /// Tem margem, recheio, borda, fundo ou largura: precisa de caixa.
    pub fn tem_caixa(&self) -> bool {
        self.fundo.is_some()
            || self.margem.iter().any(|m| m.is_some_and(|m| m != Medida::Px(0)))
            || self.recheio.iter().any(|&m| m != Medida::Px(0))
            || (0..4).any(|i| self.borda_visivel[i] && self.borda[i] > 0)
            || self.largura.is_some_and(|m| m != Medida::Auto)
            || self.largura_max.is_some_and(|m| m != Medida::Auto)
    }

    /// Alguma margem de cima ou de baixo foi dita no CSS.
    pub fn margem_vertical(&self) -> bool {
        self.margem[CIMA].is_some() || self.margem[BAIXO].is_some()
    }

    fn aplicar(&mut self, d: &Declaracao, px_pai: i32) {
        match (d.prop, d.valor) {
            (Prop::Cor, Valor::Cor(c)) => self.cor = c.or(self.cor),
            (Prop::Fundo, Valor::Cor(c)) => self.fundo = c,
            (Prop::Negrito, Valor::Sim(v)) => self.negrito = Some(v),
            (Prop::Italico, Valor::Sim(v)) => self.italico = Some(v),
            (Prop::Mono, Valor::Sim(v)) => self.mono = Some(v),
            (Prop::Sublinhado, Valor::Sim(v)) => self.sublinhado = Some(v),
            (Prop::Tamanho, Valor::Medida(m)) => {
                if let Some(px) = m.px(px_pai, px_pai).filter(|&px| px > 0) {
                    self.tamanho = Some(px);
                    self.px = px;
                }
            }
            (Prop::Alinhar, Valor::Alinhar(a)) => self.alinhar = Some(a),
            (Prop::AlturaLinha, Valor::Medida(m)) => {
                // em e px viram px ja aqui; numero fica (vale para a letra de cada filho)
                self.altura_linha = match m {
                    Medida::Auto => None,
                    Medida::Numero(_) => Some(m),
                    _ => m.px(self.px, self.px).map(Medida::Px),
                };
            }
            (Prop::Margem(i), Valor::Medida(m)) => self.margem[i] = Some(m),
            (Prop::Recheio(i), Valor::Medida(m)) => self.recheio[i] = m,
            (Prop::BordaLargura(i), Valor::Medida(m)) => self.borda[i] = m.px(self.px, 0).unwrap_or(0).clamp(0, 50),
            (Prop::BordaEstilo(i), Valor::Sim(v)) => self.borda_visivel[i] = v,
            (Prop::BordaCor(i), Valor::Cor(c)) => self.borda_cor[i] = c,
            (Prop::Largura, Valor::Medida(m)) => self.largura = Some(m),
            (Prop::LarguraMax, Valor::Medida(m)) => self.largura_max = Some(m),
            (Prop::Exibir, Valor::Exibir(e)) => {
                self.exibir = e;
                self.oculto |= e == Exibir::Nenhum;
            }
            (Prop::SemMarcador, Valor::Sim(v)) => self.sem_marcador = v,
            _ => {}
        }
    }
}

/// O estilo do ultimo elemento da `pilha`: comeca de `base` (o herdado do
/// pai, ja com o que a tag faz) e aplica as regras na ordem da cascata.
pub fn calcular(folha: &Folha, pilha: &[Marca], inline: &[DeclaracaoInline], base: Estilo, px_pai: i32) -> Estilo {
    let mut estilo = base;
    let inline: Vec<Declaracao> = inline.iter().map(|d| d.0).collect();
    if folha.vazia() && inline.is_empty() {
        return estilo;
    }
    for (_, d) in folha.casadas(pilha, &inline) {
        estilo.aplicar(&d, px_pai);
    }
    estilo
}
