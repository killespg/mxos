//! A cascata: para cada elemento, junta as regras que casam (achadas por
//! baldes de id/classe/tag, como nos navegadores grandes), ordena por
//! origem, !important, especificidade e ordem, e aplica. Primeiro as
//! variaveis (--x) e a fonte (de que dependem em/rem), depois o resto.
//! Elementos irmaos com a mesma assinatura (tag, classes, atributos, pai)
//! dividem o mesmo estilo (Rc): pagina grande = muitos <li> iguais.

use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::vec::Vec;

use super::estilo::{self, Calculo, Estilo, Exibir, Familia, Posicao};
use super::folha::{self, Declaracao, Folha, Midia};
use super::seletor::{self as sel, Bloom, Casador, Chave, Estado, PseudoElemento, Seletor};
use super::token::{self, Token};
use super::valores::{Contexto, Cor, Leitor, ler_cor};
use crate::web::atomo::{Atomo, a};
use crate::web::dom::{Documento, Elemento, Id, Modo, NENHUM, RAIZ, Tipo};

static UA: &str = include_str!("ua.css");

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Origem {
    Navegador = 0,
    Dica = 1, // atributos de apresentacao (bgcolor, width...)
    Autor = 2,
    Inline = 3,
}

struct Referencia {
    folha: u16,
    regra: u32,
    seletor: u16,
}

/// Todas as folhas, ja indexadas.
pub struct Indice {
    folhas: Vec<(Origem, Folha)>,
    por_id: BTreeMap<Atomo, Vec<Referencia>>,
    por_classe: BTreeMap<Atomo, Vec<Referencia>>,
    por_tag: BTreeMap<Atomo, Vec<Referencia>>,
    /// Seletores que so pedem um atributo ([data-x=y]) ou :root.
    por_atributo: BTreeMap<Atomo, Vec<Referencia>>,
    raiz: Vec<Referencia>,
    universais: Vec<Referencia>,
}

impl Indice {
    /// A folha do navegador mais as da pagina (na ordem do documento).
    pub fn novo(folhas_autor: Vec<Folha>, m: &Midia) -> Indice {
        let mut folhas = alloc::vec![(Origem::Navegador, Folha::ler(UA, m))];
        folhas.extend(folhas_autor.into_iter().map(|f| (Origem::Autor, f)));
        let mut ind = Indice {
            folhas: Vec::new(),
            por_id: BTreeMap::new(),
            por_classe: BTreeMap::new(),
            por_tag: BTreeMap::new(),
            por_atributo: BTreeMap::new(),
            raiz: Vec::new(),
            universais: Vec::new(),
        };
        for (fi, (_, f)) in folhas.iter().enumerate() {
            for (ri, r) in f.regras.iter().enumerate() {
                for (si, s) in r.seletores.iter().enumerate() {
                    let rf = Referencia { folha: fi as u16, regra: ri as u32, seletor: si as u16 };
                    match s.chave() {
                        Chave::Id(i) => ind.por_id.entry(i).or_default().push(rf),
                        Chave::Classe(c) => ind.por_classe.entry(c).or_default().push(rf),
                        Chave::Tag(t) => ind.por_tag.entry(t).or_default().push(rf),
                        Chave::Universal => {
                            let c = &s.partes[0];
                            if let Some(f) = c.atributos.first() {
                                ind.por_atributo.entry(f.nome).or_default().push(rf);
                            } else if c.pseudos.contains(&super::seletor::Pseudo::Raiz) {
                                ind.raiz.push(rf);
                            } else {
                                ind.universais.push(rf);
                            }
                        }
                    }
                }
            }
        }
        ind.folhas = folhas;
        ind
    }

    fn seletor(&self, r: &Referencia) -> &Seletor {
        &self.folhas[r.folha as usize].1.regras[r.regra as usize].seletores[r.seletor as usize]
    }

    fn declaracoes(&self, r: &Referencia) -> &Rc<Vec<Declaracao>> {
        &self.folhas[r.folha as usize].1.regras[r.regra as usize].declaracoes
    }
}

/// Os estilos de um documento inteiro.
pub struct Estilos {
    /// Por no (elementos; texto usa o do pai). None = sem caixa nenhuma.
    pub por_no: Vec<Option<Rc<Estilo>>>,
    pub antes: BTreeMap<Id, Rc<Estilo>>,
    pub depois: BTreeMap<Id, Rc<Estilo>>,
    pub marcador: BTreeMap<Id, Rc<Estilo>>,
}

impl Estilos {
    pub fn de(&self, id: Id) -> Option<&Rc<Estilo>> {
        self.por_no.get(id as usize).and_then(|x| x.as_ref())
    }
}

struct Achado<'a> {
    importante: bool,
    origem: Origem,
    especificidade: u32,
    ordem: u32,
    decl: &'a Declaracao,
}

fn chave_ordem(x: &Achado) -> (u8, u32, u32) {
    // normais: navegador < dica < autor < inline; importantes: invertido
    let nivel = match (x.importante, x.origem) {
        (false, o) => o as u8,
        (true, Origem::Inline) => 6,
        (true, Origem::Autor) | (true, Origem::Dica) => 5,
        (true, Origem::Navegador) => 7,
    };
    (nivel, x.especificidade, x.ordem)
}

/// Troca var(--x, reserva) pelos tokens da variavel (recursivo, com limite).
fn substituir(t: &[Token], e: &Estilo, prof: u32) -> Option<Vec<Token>> {
    if prof > 16 {
        return None;
    }
    let mut v = Vec::with_capacity(t.len());
    let mut i = 0;
    while i < t.len() {
        if let Token::Funcao(f) = &t[i] {
            if f == "var" || f == "env" {
                // argumentos ate o ')'
                let mut nivel = 1;
                let mut j = i + 1;
                while j < t.len() {
                    match t[j] {
                        Token::Funcao(_) | Token::AbreParen => nivel += 1,
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
                let args = &t[i + 1..j.min(t.len())];
                let virgula = args.iter().position(|x| *x == Token::Virgula);
                let nome = args[..virgula.unwrap_or(args.len())].iter().find_map(|x| match x {
                    Token::Ident(n) => Some(n.as_str()),
                    _ => None,
                });
                let valor = nome.and_then(|n| Atomo::se_existe(n)).and_then(|n| e.variavel(n)).filter(|v| !v.is_empty());
                match (valor, virgula) {
                    (Some(val), _) => v.extend(substituir(val, e, prof + 1)?),
                    (None, Some(k)) => v.extend(substituir(&args[k + 1..], e, prof + 1)?),
                    (None, None) => return None,
                }
                i = j + 1;
                continue;
            }
        }
        v.push(t[i].clone());
        i += 1;
    }
    Some(v)
}

fn tem_var(t: &[Token]) -> bool {
    t.iter().any(|x| matches!(x, Token::Funcao(f) if f == "var" || f == "env"))
}

fn precoce(nome: &str) -> bool {
    matches!(nome, "font-size" | "font" | "color" | "font-family" | "line-height")
}

/// Aplica uma declaracao (com inherit/initial/unset e var()).
fn aplicar(e: &mut Estilo, pai: &Estilo, d: &Declaracao, calc: &Calculo) {
    if d.nome.starts_with("--") {
        let v: Rc<[Token]> = Rc::from(d.valor.as_slice());
        // a primeira deste elemento abre a camada dele (sem copiar as do pai)
        if Rc::ptr_eq(&e.variaveis, &pai.variaveis) {
            e.variaveis = Rc::new(super::estilo::Variaveis::sobre(&pai.variaveis));
        }
        Rc::make_mut(&mut e.variaveis).definir(Atomo::de(&d.nome), v);
        return;
    }
    if let [Token::Ident(p)] = d.valor.as_slice() {
        match p.to_ascii_lowercase().as_str() {
            "inherit" => return estilo::herdar_prop(e, pai, &d.nome),
            "initial" => return estilo::inicial_prop(e, &d.nome),
            "unset" | "revert" | "revert-layer" => {
                if estilo::eh_herdada(&d.nome) {
                    estilo::herdar_prop(e, pai, &d.nome);
                } else {
                    estilo::inicial_prop(e, &d.nome);
                }
                return;
            }
            _ => {}
        }
    }
    if tem_var(&d.valor) {
        if let Some(t) = substituir(&d.valor, e, 0) {
            e.aplicar(&d.nome, &t, calc);
        }
    } else {
        e.aplicar(&d.nome, &d.valor, calc);
    }
}

/// Dicas de apresentacao dos atributos do HTML (bgcolor, width, align...),
/// como CSS de especificidade zero.
fn dicas(el: &Elemento) -> Vec<Declaracao> {
    let mut v = Vec::new();
    let mut d = |nome: &str, valor: &str| {
        v.push(Declaracao { nome: nome.into(), valor: token::tokens(valor), importante: false });
    };
    let medida_html = |s: &str| -> Option<alloc::string::String> {
        let s = s.trim();
        let num: alloc::string::String = s.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        if num.is_empty() {
            return None;
        }
        Some(if s[num.len()..].starts_with('%') { alloc::format!("{num}%") } else { alloc::format!("{num}px") })
    };
    let n = el.nome;
    if let Some(c) = el.atributo(a::BGCOLOR) {
        d("background-color", c);
    }
    if [a::IMG, a::TABLE, a::TD, a::TH, a::IFRAME, a::VIDEO, a::CANVAS, a::EMBED, a::OBJECT, a::COL, a::HR, a::INPUT].contains(&n) {
        if let Some(w) = el.atributo(a::WIDTH).and_then(medida_html) {
            d("width", &w);
        }
        if let Some(h) = el.atributo(a::HEIGHT).and_then(medida_html) {
            if n != a::TD && n != a::TH || !h.ends_with('%') {
                d("height", &h);
            }
        }
    }
    if n == a::TABLE {
        if let Some(b) = el.atributo(a::BORDER) {
            let b = if b.trim().is_empty() { "1" } else { b };
            if let Some(px) = medida_html(b) {
                if px != "0px" {
                    d("border", &alloc::format!("{px} outset gray"));
                }
            }
        }
        if let Some(c) = el.atributo(a::CELLSPACING).and_then(medida_html) {
            d("border-spacing", &c);
        }
        if el.atributo(a::ALIGN).is_some_and(|x| x.eq_ignore_ascii_case("center")) {
            d("margin-left", "auto");
            d("margin-right", "auto");
        }
    }
    if n == a::TD || n == a::TH || n == a::TR || n == a::DIV || n == a::P || n.texto().starts_with('h') && n.texto().len() == 2 {
        if let Some(al) = el.atributo(a::ALIGN) {
            match al.to_ascii_lowercase().as_str() {
                "center" | "middle" => d("text-align", "center"),
                "right" => d("text-align", "right"),
                "left" => d("text-align", "left"),
                "justify" => d("text-align", "justify"),
                _ => {}
            }
        }
        if let Some(va) = el.atributo(a::VALIGN) {
            d("vertical-align", va);
        }
        if el.tem(a::NOWRAP) {
            d("white-space", "nowrap");
        }
    }
    if n == a::IMG || n == a::OBJECT {
        if let Some(al) = el.atributo(a::ALIGN) {
            match al.to_ascii_lowercase().as_str() {
                "left" => d("float", "left"),
                "right" => d("float", "right"),
                "middle" | "center" => d("vertical-align", "middle"),
                "top" => d("vertical-align", "top"),
                _ => {}
            }
        }
    }
    if n == a::FONT {
        if let Some(c) = el.atributo(a::COLOR) {
            d("color", c);
        }
        if let Some(f) = el.atributo(a::FACE) {
            d("font-family", f);
        }
        if let Some(s) = el.atributo(a::SIZE) {
            let s = s.trim();
            let base: i32 = if s.starts_with('+') || s.starts_with('-') { 3 } else { 0 };
            let k = base + s.trim_start_matches('+').parse::<i32>().unwrap_or(3);
            let nome = match k.clamp(1, 7) {
                1 => "x-small",
                2 => "small",
                3 => "medium",
                4 => "large",
                5 => "x-large",
                6 => "xx-large",
                _ => "xxx-large",
            };
            d("font-size", nome);
        }
    }
    if n == a::BODY {
        if let Some(c) = el.atributo(a::TEXT) {
            d("color", c);
        }
        if let Some(b) = el.atributo(a::BACKGROUND) {
            d("background-image", &alloc::format!("url(\"{b}\")"));
        }
    }
    if n == a::OL {
        if let Some(t) = el.atributo(a::TYPE) {
            let tipo = match t {
                "a" => "lower-alpha",
                "A" => "upper-alpha",
                "i" => "lower-roman",
                "I" => "upper-roman",
                _ => "decimal",
            };
            d("list-style-type", tipo);
        }
    }
    if n == a::HR {
        if el.tem(a::NOSCRIPT) || el.atributo(Atomo::de("noshade")).is_some() {
            d("border-style", "solid");
        }
        if let Some(c) = el.atributo(a::COLOR) {
            d("background-color", c);
            d("border-color", c);
        }
    }
    v
}

/// Onde a pagina guarda `style="..."` ja lido (para nao reler a cada vez).
fn inline(el: &Elemento) -> Vec<Declaracao> {
    match el.atributo(Atomo::de("style")) {
        Some(s) if !s.trim().is_empty() => folha::ler_declaracoes(&token::tokens(s)),
        _ => Vec::new(),
    }
}

/// Calcula o estilo de todos os elementos.
pub fn calcular(doc: &Documento, ind: &Indice, m: &Midia, estado: Estado) -> Estilos {
    let casador = Casador::novo(doc, estado);
    let mut est = Estilos {
        por_no: alloc::vec![None; doc.nos.len()],
        antes: BTreeMap::new(),
        depois: BTreeMap::new(),
        marcador: BTreeMap::new(),
    };
    let quirks = doc.modo == Modo::Quirks;
    let raiz_base = Rc::new(Estilo::inicial());
    let inicial = Estilo::inicial();
    let mut fonte_raiz = 16.0f32;
    let mut candidatos: Vec<&Referencia> = Vec::with_capacity(64);
    // cache de estilos: mesmo estilo do pai (o mesmo Rc) + mesma tag + as
    // mesmas regras casadas + mesmas dicas/style = o mesmo estilo final.
    // (pense em mil <li> de uma lista: um estilo so)
    let mut cache: BTreeMap<(usize, Atomo, Vec<u64>, u64), [Option<Rc<Estilo>>; 4]> = BTreeMap::new();

    // ancestrais do elemento atual no filtro de Bloom: (id, tracos)
    let mut bloom = Bloom::default();
    let mut pilha: Vec<(Id, Vec<u32>)> = Vec::with_capacity(64);
    // pre-ordem: pai sempre antes dos filhos
    let ordem: Vec<Id> = doc.descendentes(RAIZ).collect();
    for id in ordem {
        let Tipo::Elemento(el) = &doc.no(id).tipo else { continue };
        let pai_id = doc.pai(id);
        let pai: Rc<Estilo> = if pai_id == RAIZ || pai_id == NENHUM {
            raiz_base.clone()
        } else {
            match &est.por_no[pai_id as usize] {
                Some(p) => p.clone(),
                None => continue, // dentro de display:none
            }
        };
        if pai.exibir == Exibir::Nenhum {
            est.por_no[id as usize] = Some(pai.clone()); // filhos tambem somem
            continue;
        }

        // desce/sobe a pilha de ancestrais ate o pai
        while let Some((topo, _)) = pilha.last() {
            if *topo == pai_id {
                break;
            }
            if let Some((_, tr)) = pilha.pop() {
                for h in tr {
                    bloom.remover(h);
                }
            }
        }
        let mut meus = Vec::with_capacity(4);
        sel::tracos(el, &mut meus);

        // candidatos dos baldes
        candidatos.clear();
        if let Some(i) = el.id {
            if let Some(v) = ind.por_id.get(&i) {
                candidatos.extend(v.iter());
            }
        }
        for c in &el.classes {
            if let Some(v) = ind.por_classe.get(c) {
                candidatos.extend(v.iter());
            }
        }
        if let Some(v) = ind.por_tag.get(&el.nome) {
            candidatos.extend(v.iter());
        }
        for at in &el.atributos {
            if let Some(v) = ind.por_atributo.get(&at.nome) {
                candidatos.extend(v.iter());
            }
        }
        if pai_id == RAIZ {
            candidatos.extend(ind.raiz.iter());
        }
        candidatos.extend(ind.universais.iter());

        let mut achados: Vec<Achado> = Vec::with_capacity(32);
        let mut casados: Vec<usize> = Vec::new();
        let mut pseudos: Vec<(PseudoElemento, &Rc<Vec<Declaracao>>, u32, u32, Origem)> = Vec::new();
        for (k, r) in candidatos.iter().enumerate() {
            let s = ind.seletor(r);
            if !casador.casa_com_filtro(s, id, &bloom) {
                continue;
            }
            casados.push(k);
            let origem = ind.folhas[r.folha as usize].0;
            let ordem = ((r.folha as u32) << 20) | r.regra;
            let decls = ind.declaracoes(r);
            if let Some(pe) = s.pseudo_elemento {
                pseudos.push((pe, decls, s.especificidade, ordem, origem));
                continue;
            }
            for d in decls.iter() {
                achados.push(Achado { importante: d.importante, origem, especificidade: s.especificidade, ordem, decl: d });
            }
        }

        for &h in &meus {
            bloom.inserir(h);
        }
        pilha.push((id, meus));
        let dicas_v = dicas(el);
        let inline_v = inline(el);
        let chave_cache = if pai_id != RAIZ {
            // as dicas e o style= entram por um hash (antes: um texto com
            // format! de cada declaracao, montado para cada elemento)
            let mut extra = HASH_INICIO;
            for d in dicas_v.iter().chain(inline_v.iter()) {
                hash_bytes(&mut extra, d.nome.as_bytes());
                for t in &d.valor {
                    hash_token(&mut extra, t);
                }
                hash_bytes(&mut extra, if d.importante { b"!" } else { b";" });
            }
            let mut regras = casados_chave(&candidatos, &casados);
            regras.sort_unstable();
            Some((Rc::as_ptr(&pai) as usize, el.nome, regras, extra))
        } else {
            None
        };
        if let Some(k) = &chave_cache {
            if let Some([e2, antes, depois, marcador]) = cache.get(k) {
                est.por_no[id as usize] = e2.clone();
                if let Some(x) = antes {
                    est.antes.insert(id, x.clone());
                }
                if let Some(x) = depois {
                    est.depois.insert(id, x.clone());
                }
                if let Some(x) = marcador {
                    est.marcador.insert(id, x.clone());
                }
                continue;
            }
        }

        for d in &dicas_v {
            achados.push(Achado { importante: false, origem: Origem::Dica, especificidade: 0, ordem: 0, decl: d });
        }
        for d in &inline_v {
            achados.push(Achado { importante: d.importante, origem: Origem::Inline, especificidade: u32::MAX, ordem: u32::MAX, decl: d });
        }
        achados.sort_by_key(chave_ordem);

        let mut e = Estilo::herdar_de(&inicial, &pai);
        let ctx = Contexto { fonte: pai.tamanho_fonte, fonte_raiz, largura_tela: m.largura, altura_tela: m.altura };
        let mut calc = Calculo { ctx, tamanho_pai: pai.tamanho_fonte, quirks };
        // 1: variaveis e fonte
        for x in &achados {
            if x.decl.nome.starts_with("--") || precoce(&x.decl.nome) {
                aplicar(&mut e, &pai, x.decl, &calc);
            }
        }
        calc.ctx.fonte = e.tamanho_fonte;
        if pai_id == RAIZ {
            fonte_raiz = e.tamanho_fonte;
            calc.ctx.fonte_raiz = fonte_raiz;
        }
        // 2: o resto
        for x in &achados {
            if !(x.decl.nome.starts_with("--") || precoce(&x.decl.nome)) {
                aplicar(&mut e, &pai, x.decl, &calc);
            }
        }
        ajustes(&mut e, &pai, el, pai_id == RAIZ);

        // ::before / ::after / ::marker
        for pe in [PseudoElemento::Antes, PseudoElemento::Depois, PseudoElemento::Marcador] {
            let mut lista: Vec<(u32, u32, Origem, &Rc<Vec<Declaracao>>)> =
                pseudos.iter().filter(|p| p.0 == pe).map(|p| (p.2, p.3, p.4, p.1)).collect();
            if lista.is_empty() {
                continue;
            }
            lista.sort_by_key(|x| (x.2 as u8, x.0, x.1));
            let mut pe_estilo = Estilo::herdar(&e);
            let c2 = Calculo { ctx: Contexto { fonte: e.tamanho_fonte, ..calc.ctx }, tamanho_pai: e.tamanho_fonte, quirks };
            for imp in [false, true] {
                for (_, _, _, decls) in &lista {
                    for d in decls.iter().filter(|d| d.importante == imp) {
                        aplicar(&mut pe_estilo, &e, d, &c2);
                    }
                }
            }
            if pe == PseudoElemento::Marcador {
                est.marcador.insert(id, Rc::new(pe_estilo));
                continue;
            }
            if pe_estilo.conteudo.is_some() && pe_estilo.exibir != Exibir::Nenhum {
                if pe_estilo.posicao == Posicao::Absoluta || pe_estilo.posicao == Posicao::Fixa || pe_estilo.flutuar != estilo::Lado::Nenhum {
                    pe_estilo.exibir = pe_estilo.exibir.bloquificar();
                }
                let rc = Rc::new(pe_estilo);
                if pe == PseudoElemento::Antes {
                    est.antes.insert(id, rc);
                } else {
                    est.depois.insert(id, rc);
                }
            }
        }

        let rc = Rc::new(e);
        if let Some(k) = chave_cache {
            let pe = |m: &BTreeMap<Id, Rc<Estilo>>| m.get(&id).cloned();
            cache.insert(k, [Some(rc.clone()), pe(&est.antes), pe(&est.depois), pe(&est.marcador)]);
        }
        est.por_no[id as usize] = Some(rc);
    }
    est
}

const HASH_INICIO: u64 = 0xCBF2_9CE4_8422_2325;

/// FNV-1a de 64 bits.
fn hash_bytes(h: &mut u64, b: &[u8]) {
    for &x in b {
        *h = (*h ^ x as u64).wrapping_mul(0x0100_0000_01B3);
    }
}

/// Um token no hash (o tipo, o texto e os numeros pelos bits).
fn hash_token(h: &mut u64, t: &Token) {
    let (tipo, texto, numero): (u8, &str, f32) = match t {
        Token::Ident(s) => (1, s, 0.0),
        Token::Funcao(s) => (2, s, 0.0),
        Token::Arroba(s) => (3, s, 0.0),
        Token::Hash(s, v) => (4, s, *v as u8 as f32),
        Token::Texto(s) => (5, s, 0.0),
        Token::Url(s) => (6, s, 0.0),
        Token::Delim(c) => (7, "", *c as u32 as f32),
        Token::Numero(n, i) => (8, "", if *i { -*n - 0.5 } else { *n }),
        Token::Porcentagem(n) => (9, "", *n),
        Token::Dimensao(n, s) => (10, s, *n),
        // o resto: so o tipo (Debug do enum, sem campos)
        outro => (11, "", discriminante(outro)),
    };
    hash_bytes(h, &[tipo]);
    hash_bytes(h, texto.as_bytes());
    hash_bytes(h, &numero.to_bits().to_le_bytes());
}

/// Um numero diferente para cada token sem campos.
fn discriminante(t: &Token) -> f32 {
    match t {
        Token::TextoRuim => 1.0,
        Token::UrlRuim => 2.0,
        Token::Espaco => 3.0,
        Token::Cdo => 4.0,
        Token::Cdc => 5.0,
        Token::DoisPontos => 6.0,
        Token::PontoEVirgula => 7.0,
        Token::Virgula => 8.0,
        Token::AbreColchete => 9.0,
        Token::FechaColchete => 10.0,
        Token::AbreParen => 11.0,
        Token::FechaParen => 12.0,
        Token::AbreChave => 13.0,
        Token::FechaChave => 14.0,
        // os com campos ja foram tratados em hash_token
        Token::Ident(_) | Token::Funcao(_) | Token::Arroba(_) | Token::Hash(..) | Token::Texto(_) | Token::Url(_) | Token::Delim(_) | Token::Numero(..) | Token::Porcentagem(_) | Token::Dimensao(..) => 0.0,
    }
}

/// Identifica o conjunto de regras casadas (para o cache de irmaos).
fn casados_chave(candidatos: &[&Referencia], casados: &[usize]) -> Vec<u64> {
    casados.iter().map(|&k| ((candidatos[k].folha as u64) << 40) | ((candidatos[k].regra as u64) << 8) | candidatos[k].seletor as u64).collect()
}

/// Regras que nao sao de propriedade nenhuma (CSS 2.1, 9.7 e afins).
fn ajustes(e: &mut Estilo, pai: &Estilo, el: &Elemento, raiz: bool) {
    // float e position absoluta viram bloco
    if e.exibir != Exibir::Nenhum && (e.flutuar != estilo::Lado::Nenhum || e.fora_do_fluxo() || raiz) {
        e.exibir = e.exibir.bloquificar();
        if raiz && e.exibir == Exibir::Conteudo {
            e.exibir = Exibir::Bloco;
        }
    }
    if e.fora_do_fluxo() {
        e.flutuar = estilo::Lado::Nenhum;
    }
    // filhos de flex/grid viram bloco
    if matches!(pai.exibir, Exibir::Flex | Exibir::InlineFlex | Exibir::Grid | Exibir::InlineGrid) && e.exibir != Exibir::Nenhum && e.exibir != Exibir::Conteudo {
        e.exibir = e.exibir.bloquificar();
    }
    // borda sem cor: currentcolor (ja resolvido na leitura); estilo none zera
    if el.eh(a::BR) {
        e.exibir = Exibir::Inline;
    }
    if e.familia == Familia::Mono && e.tamanho_fonte == 16.0 && pai.familia != Familia::Mono {
        // como os navegadores: monospace padrao e 13px
        e.tamanho_fonte = 13.0;
    }
}

/// Le "color" de um texto (para atributos como <font color>), util fora.
pub fn cor_de(texto: &str) -> Option<Cor> {
    let t = token::tokens(texto);
    let mut l = Leitor::novo(&t);
    ler_cor(&mut l, None)
}

impl Indice {
    pub fn quantas_regras(&self) -> usize {
        self.folhas.iter().map(|(_, f)| f.regras.len()).sum()
    }
}

#[allow(unused)]
fn _nao_usado(_: &Seletor) {}
