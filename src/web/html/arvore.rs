//! Construcao da arvore do HTML5 (WHATWG 13.2.6): os modos de insercao,
//! a pilha de elementos abertos, a lista de formatacao ativa, o algoritmo
//! de adocao (o que conserta "<b><i></b></i>"), o "foster parenting" das
//! tabelas e o conteudo estrangeiro (SVG e MathML). E o que faz toda pagina
//! virar a MESMA arvore que nos outros navegadores, por pior que seja o
//! HTML.
//!
//! Simplificacoes: <template> vira um elemento comum (o CSS padrao esconde)
//! e o documento e sempre tratado com scripts ligados (<noscript> e texto).

use alloc::string::String;
use alloc::vec::Vec;

use super::token::{Estado, Token, Tokenizador};
use crate::web::atomo::{Atomo, a};
use crate::web::dom::{Atributo, Documento, Espaco, Id, Modo as ModoDoc, NENHUM, RAIZ, Tipo};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Modo {
    Inicial,
    AntesHtml,
    AntesHead,
    NoHead,
    DepoisHead,
    NoBody,
    Texto,
    NaTabela,
    TextoDeTabela,
    NaLegenda,
    NoGrupoColunas,
    NoCorpoTabela,
    NaLinha,
    NaCelula,
    NoSelect,
    NoSelectNaTabela,
    DepoisBody,
    NoFrameset,
    DepoisFrameset,
    DepoisDepoisBody,
    DepoisDepoisFrameset,
}

#[derive(Clone)]
enum Formatacao {
    Marcador,
    Elemento { id: Id, nome: Atomo, atributos: Vec<Atributo> },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Escopo {
    Normal,
    Lista,
    Botao,
    Tabela,
    Select,
}

struct Construtor {
    doc: Documento,
    tk: Tokenizador,
    modo: Modo,
    modo_original: Modo,
    pilha: Vec<Id>,
    formatacao: Vec<Formatacao>,
    cabeca: Id,
    formulario: Id,
    frameset_ok: bool,
    adotivo: bool,
    pular_lf: bool,
    texto_tabela: String,
    parar: bool,
}

const H: [Atomo; 6] = [a::H1, a::H2, a::H3, a::H4, a::H5, a::H6];

fn eh_espaco(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0C' | '\r')
}

fn so_espacos(s: &str) -> bool {
    s.chars().all(eh_espaco)
}

/// (espacos do comeco, resto)
fn separar_espacos(s: &str) -> (&str, &str) {
    let i = s.find(|c: char| !eh_espaco(c)).unwrap_or(s.len());
    s.split_at(i)
}

fn especial_html(n: Atomo) -> bool {
    use a::*;
    [
        ADDRESS, APPLET, AREA, ARTICLE, ASIDE, BASE, BASEFONT, BGSOUND, BLOCKQUOTE, BODY, BR, BUTTON, CAPTION,
        CENTER, COL, COLGROUP, DD, DETAILS, DIR, DIV, DL, DT, EMBED, FIELDSET, FIGCAPTION, FIGURE, FOOTER, FORM,
        FRAME, FRAMESET, H1, H2, H3, H4, H5, H6, HEAD, HEADER, HGROUP, HR, HTML, IFRAME, IMG, INPUT, KEYGEN, LI,
        LINK, LISTING, MAIN, MARQUEE, MENU, META, NAV, NOEMBED, NOFRAMES, NOSCRIPT, OBJECT, OL, P, PARAM,
        PLAINTEXT, PRE, SCRIPT, SEARCH, SECTION, SELECT, SOURCE, STYLE, SUMMARY, TABLE, TBODY, TD, TEMPLATE,
        TEXTAREA, TFOOT, TH, THEAD, TITLE, TR, TRACK, UL, WBR, XMP,
    ]
    .contains(&n)
}

fn formatacao_nome(n: Atomo) -> bool {
    use a::*;
    [A, B, BIG, CODE, EM, FONT, I, NOBR, S, SMALL, STRIKE, STRONG, TT, U].contains(&n)
}

/// Fins implicitos ("generate implied end tags").
fn fim_implicito(n: Atomo) -> bool {
    use a::*;
    [DD, DT, LI, OPTGROUP, OPTION, P, RB, RP, RT, RTC].contains(&n)
}

fn fim_implicito_total(n: Atomo) -> bool {
    use a::*;
    fim_implicito(n) || [CAPTION, COLGROUP, TBODY, TD, TFOOT, TH, THEAD, TR].contains(&n)
}

/// Nomes de tags SVG com maiusculas (o tokenizador poe tudo em minusculo).
fn ajustar_svg(nome: Atomo) -> Atomo {
    const T: &[(&str, &str)] = &[
        ("altglyph", "altGlyph"), ("altglyphdef", "altGlyphDef"), ("altglyphitem", "altGlyphItem"),
        ("animatecolor", "animateColor"), ("animatemotion", "animateMotion"),
        ("animatetransform", "animateTransform"), ("clippath", "clipPath"), ("feblend", "feBlend"),
        ("fecolormatrix", "feColorMatrix"), ("fecomponenttransfer", "feComponentTransfer"),
        ("fecomposite", "feComposite"), ("feconvolvematrix", "feConvolveMatrix"),
        ("fediffuselighting", "feDiffuseLighting"), ("fedisplacementmap", "feDisplacementMap"),
        ("fedistantlight", "feDistantLight"), ("fedropshadow", "feDropShadow"), ("feflood", "feFlood"),
        ("fefunca", "feFuncA"), ("fefuncb", "feFuncB"), ("fefuncg", "feFuncG"), ("fefuncr", "feFuncR"),
        ("fegaussianblur", "feGaussianBlur"), ("feimage", "feImage"), ("femerge", "feMerge"),
        ("femergenode", "feMergeNode"), ("femorphology", "feMorphology"), ("feoffset", "feOffset"),
        ("fepointlight", "fePointLight"), ("fespecularlighting", "feSpecularLighting"),
        ("fespotlight", "feSpotLight"), ("fetile", "feTile"), ("feturbulence", "feTurbulence"),
        ("foreignobject", "foreignObject"), ("glyphref", "glyphRef"), ("lineargradient", "linearGradient"),
        ("radialgradient", "radialGradient"), ("textpath", "textPath"),
    ];
    let t = nome.texto();
    match T.iter().find(|(x, _)| *x == t) {
        Some((_, certo)) => Atomo::de(certo),
        None => nome,
    }
}

fn ajustar_atributos_svg(atributos: &mut [Atributo]) {
    const T: &[(&str, &str)] = &[
        ("attributename", "attributeName"), ("attributetype", "attributeType"), ("basefrequency", "baseFrequency"),
        ("baseprofile", "baseProfile"), ("calcmode", "calcMode"), ("clippathunits", "clipPathUnits"),
        ("diffuseconstant", "diffuseConstant"), ("edgemode", "edgeMode"), ("filterunits", "filterUnits"),
        ("glyphref", "glyphRef"), ("gradienttransform", "gradientTransform"), ("gradientunits", "gradientUnits"),
        ("kernelmatrix", "kernelMatrix"), ("kernelunitlength", "kernelUnitLength"), ("keypoints", "keyPoints"),
        ("keysplines", "keySplines"), ("keytimes", "keyTimes"), ("lengthadjust", "lengthAdjust"),
        ("limitingconeangle", "limitingConeAngle"), ("markerheight", "markerHeight"), ("markerunits", "markerUnits"),
        ("markerwidth", "markerWidth"), ("maskcontentunits", "maskContentUnits"), ("maskunits", "maskUnits"),
        ("numoctaves", "numOctaves"), ("pathlength", "pathLength"), ("patterncontentunits", "patternContentUnits"),
        ("patterntransform", "patternTransform"), ("patternunits", "patternUnits"), ("pointsatx", "pointsAtX"),
        ("pointsaty", "pointsAtY"), ("pointsatz", "pointsAtZ"), ("preservealpha", "preserveAlpha"),
        ("preserveaspectratio", "preserveAspectRatio"), ("primitiveunits", "primitiveUnits"), ("refx", "refX"),
        ("refy", "refY"), ("repeatcount", "repeatCount"), ("repeatdur", "repeatDur"),
        ("requiredextensions", "requiredExtensions"), ("requiredfeatures", "requiredFeatures"),
        ("specularconstant", "specularConstant"), ("specularexponent", "specularExponent"),
        ("spreadmethod", "spreadMethod"), ("startoffset", "startOffset"), ("stddeviation", "stdDeviation"),
        ("stitchtiles", "stitchTiles"), ("surfacescale", "surfaceScale"), ("systemlanguage", "systemLanguage"),
        ("tablevalues", "tableValues"), ("targetx", "targetX"), ("targety", "targetY"), ("textlength", "textLength"),
        ("viewbox", "viewBox"), ("viewtarget", "viewTarget"), ("xchannelselector", "xChannelSelector"),
        ("ychannelselector", "yChannelSelector"), ("zoomandpan", "zoomAndPan"),
    ];
    for at in atributos.iter_mut() {
        let t = at.nome.texto();
        if let Some((_, certo)) = T.iter().find(|(x, _)| *x == t) {
            at.nome = Atomo::de(certo);
        }
    }
}

/// Doctypes que ligam o modo "quirks" (os mais comuns da lista da spec).
fn modo_do_doctype(nome: &str, publico: Option<&str>, sistema: Option<&str>, quirks: bool) -> ModoDoc {
    if quirks || nome != "html" {
        return ModoDoc::Quirks;
    }
    let p = publico.map(|p| p.to_ascii_lowercase());
    let s = sistema.map(|s| s.to_ascii_lowercase());
    if let Some(p) = &p {
        const QUIRKS: &[&str] = &[
            "+//silmaril//dtd html pro v0r11 19970101//", "-//as//dtd html 3.0 aswedit + extensions//",
            "-//advasoft ltd//dtd html 3.0 aswedit + extensions//", "-//ietf//dtd html 2.0", "-//ietf//dtd html 3",
            "-//ietf//dtd html level", "-//ietf//dtd html strict", "-//ietf//dtd html//", "-//metrius//",
            "-//microsoft//dtd internet explorer", "-//netscape comm. corp.//", "-//o'reilly and associates//",
            "-//softquad", "-//spyglass//", "-//sq//", "-//sun microsystems corp.//", "-//w3c//dtd html 3",
            "-//w3c//dtd html 4.0 frameset//", "-//w3c//dtd html 4.0 transitional//",
            "-//w3c//dtd html experimental", "-//w3c//dtd w3 html//", "-//w3o//", "-//webtechs//",
        ];
        if p == "-//w3o//dtd w3 html strict 3.0//en//" || p == "-/w3c/dtd html 4.0 transitional/en" || p == "html" {
            return ModoDoc::Quirks;
        }
        if QUIRKS.iter().any(|q| p.starts_with(q)) {
            return ModoDoc::Quirks;
        }
        let html401 = p.starts_with("-//w3c//dtd html 4.01 frameset//") || p.starts_with("-//w3c//dtd html 4.01 transitional//");
        if html401 && s.is_none() {
            return ModoDoc::Quirks;
        }
        if p.starts_with("-//w3c//dtd xhtml 1.0 frameset//") || p.starts_with("-//w3c//dtd xhtml 1.0 transitional//") || html401 {
            return ModoDoc::QuaseNormal;
        }
    }
    if s.as_deref() == Some("http://www.ibm.com/data/dtd/v11/ibmxhtml1-transitional.dtd") {
        return ModoDoc::Quirks;
    }
    ModoDoc::Normal
}

/// Monta a arvore de um documento HTML inteiro.
pub fn analisar(texto: &str) -> Documento {
    let mut c = Construtor {
        doc: Documento::novo(),
        tk: Tokenizador::novo(texto),
        modo: Modo::Inicial,
        modo_original: Modo::Inicial,
        pilha: Vec::with_capacity(64),
        formatacao: Vec::new(),
        cabeca: NENHUM,
        formulario: NENHUM,
        frameset_ok: true,
        adotivo: false,
        pular_lf: false,
        texto_tabela: String::new(),
        parar: false,
    };
    loop {
        let t = c.tk.proximo();
        let fim = t == Token::FimDoArquivo;
        let t = match t {
            Token::Texto(mut s) if c.pular_lf => {
                c.pular_lf = false;
                if s.starts_with('\n') {
                    s.remove(0);
                }
                if s.is_empty() {
                    continue;
                }
                Token::Texto(s)
            }
            t => {
                c.pular_lf = false;
                t
            }
        };
        c.despachar(t);
        if fim || c.parar {
            break;
        }
    }
    c.doc
}

impl Construtor {
    // ------------------------------------------------------- utilidades

    fn atual(&self) -> Id {
        self.pilha.last().copied().unwrap_or(NENHUM)
    }

    fn nome(&self, id: Id) -> Atomo {
        self.doc.nome(id)
    }

    fn atual_eh(&self, n: Atomo) -> bool {
        self.nome(self.atual()) == n
    }

    fn espaco(&self, id: Id) -> Espaco {
        self.doc.elemento(id).map_or(Espaco::Html, |e| e.espaco)
    }

    fn especial(&self, id: Id) -> bool {
        match self.doc.elemento(id) {
            Some(e) if e.espaco == Espaco::Html => especial_html(e.nome),
            Some(e) if e.espaco == Espaco::MathMl => [a::MI, a::MO, a::MN, a::MS, a::MTEXT, a::ANNOTATION_XML].contains(&e.nome),
            Some(e) => [a::FOREIGN_OBJECT, a::DESC, a::TITLE].contains(&e.nome),
            None => false,
        }
    }

    fn no_escopo(&self, alvo: Atomo, escopo: Escopo) -> bool {
        self.no_escopo_algum(&[alvo], escopo)
    }

    fn no_escopo_algum(&self, alvos: &[Atomo], escopo: Escopo) -> bool {
        for &id in self.pilha.iter().rev() {
            let e = match self.doc.elemento(id) {
                Some(e) => e,
                None => continue,
            };
            if e.espaco == Espaco::Html && alvos.contains(&e.nome) {
                return true;
            }
            let barreira = match escopo {
                Escopo::Select => !(e.espaco == Espaco::Html && (e.nome == a::OPTGROUP || e.nome == a::OPTION)),
                Escopo::Tabela => e.espaco == Espaco::Html && [a::HTML, a::TABLE, a::TEMPLATE].contains(&e.nome),
                _ => {
                    let base = match e.espaco {
                        Espaco::Html => [a::APPLET, a::CAPTION, a::HTML, a::TABLE, a::TD, a::TH, a::MARQUEE, a::OBJECT, a::TEMPLATE]
                            .contains(&e.nome),
                        Espaco::MathMl => [a::MI, a::MO, a::MN, a::MS, a::MTEXT, a::ANNOTATION_XML].contains(&e.nome),
                        Espaco::Svg => [a::FOREIGN_OBJECT, a::DESC, a::TITLE].contains(&e.nome),
                    };
                    base || (e.espaco == Espaco::Html
                        && match escopo {
                            Escopo::Lista => e.nome == a::OL || e.nome == a::UL,
                            Escopo::Botao => e.nome == a::BUTTON,
                            _ => false,
                        })
                }
            };
            if barreira {
                return false;
            }
        }
        false
    }

    fn na_pilha(&self, id: Id) -> bool {
        self.pilha.contains(&id)
    }

    fn tirar_da_pilha(&mut self, id: Id) {
        if let Some(i) = self.pilha.iter().rposition(|&x| x == id) {
            self.pilha.remove(i);
        }
    }

    /// Desempilha ate tirar um elemento HTML com esse nome.
    fn desempilhar_ate(&mut self, nome: Atomo) {
        while let Some(id) = self.pilha.pop() {
            if self.nome(id) == nome {
                break;
            }
        }
    }

    fn desempilhar_ate_algum(&mut self, nomes: &[Atomo]) {
        while let Some(id) = self.pilha.pop() {
            if nomes.contains(&self.nome(id)) {
                break;
            }
        }
    }

    fn gerar_fins(&mut self, exceto: Atomo) {
        loop {
            let n = self.nome(self.atual());
            if n != exceto && fim_implicito(n) && self.pilha.len() > 1 {
                self.pilha.pop();
            } else {
                break;
            }
        }
    }

    fn gerar_fins_total(&mut self) {
        while self.pilha.len() > 1 && fim_implicito_total(self.nome(self.atual())) {
            self.pilha.pop();
        }
    }

    fn fechar_p(&mut self) {
        if self.no_escopo(a::P, Escopo::Botao) {
            self.gerar_fins(a::P);
            self.desempilhar_ate(a::P);
        }
    }

    // ---------------------------------------------------------- insercao

    /// Onde inserir (pai, antes de quem), com o "foster parenting".
    fn lugar(&self, alvo: Id) -> (Id, Id) {
        let n = self.nome(alvo);
        if self.adotivo && [a::TABLE, a::TBODY, a::TFOOT, a::THEAD, a::TR].contains(&n) {
            if let Some(k) = self.pilha.iter().rposition(|&x| self.nome(x) == a::TABLE) {
                let tabela = self.pilha[k];
                let pai = self.doc.pai(tabela);
                if pai != NENHUM {
                    return (pai, tabela);
                }
                return (self.pilha[k.saturating_sub(1)], NENHUM);
            }
            return (self.pilha[0], NENHUM);
        }
        (alvo, NENHUM)
    }

    fn inserir_no(&mut self, no: Id) {
        let (pai, antes) = self.lugar(self.atual());
        self.doc.inserir_antes(pai, no, antes);
    }

    fn criar(&mut self, nome: Atomo, espaco: Espaco, atributos: Vec<Atributo>) -> Id {
        self.doc.criar_elemento(nome, espaco, atributos)
    }

    fn inserir(&mut self, nome: Atomo, atributos: Vec<Atributo>) -> Id {
        let id = self.criar(nome, Espaco::Html, atributos);
        self.inserir_no(id);
        self.pilha.push(id);
        id
    }

    fn inserir_estrangeiro(&mut self, nome: Atomo, mut atributos: Vec<Atributo>, espaco: Espaco) -> Id {
        let nome = if espaco == Espaco::Svg {
            ajustar_atributos_svg(&mut atributos);
            ajustar_svg(nome)
        } else {
            nome
        };
        let id = self.criar(nome, espaco, atributos);
        self.inserir_no(id);
        self.pilha.push(id);
        id
    }

    fn inserir_texto(&mut self, texto: &str) {
        if texto.is_empty() {
            return;
        }
        let atual = self.atual();
        if atual == NENHUM {
            return;
        }
        let (pai, antes) = self.lugar(atual);
        if pai == RAIZ {
            return; // o documento nao tem texto
        }
        self.doc.inserir_texto_antes(pai, texto, antes);
    }

    fn inserir_comentario(&mut self, texto: String, pai: Option<Id>) {
        let id = self.doc.criar(Tipo::Comentario(texto));
        match pai {
            Some(p) => self.doc.anexar(p, id),
            None => self.inserir_no(id),
        }
    }

    // -------------------------------------------------------- formatacao

    fn empurrar_formatacao(&mut self, id: Id, nome: Atomo, atributos: Vec<Atributo>) {
        // "arca de Noe": no maximo 3 iguais depois do ultimo marcador
        let mut iguais = Vec::new();
        for (k, f) in self.formatacao.iter().enumerate().rev() {
            match f {
                Formatacao::Marcador => break,
                Formatacao::Elemento { nome: n, atributos: at, .. } => {
                    let mesmo = *n == nome
                        && at.len() == atributos.len()
                        && at.iter().all(|x| atributos.iter().any(|y| y.nome == x.nome && y.valor == x.valor));
                    if mesmo {
                        iguais.push(k);
                    }
                }
            }
        }
        if iguais.len() >= 3 {
            if let Some(&k) = iguais.last() {
                self.formatacao.remove(k);
            }
        }
        self.formatacao.push(Formatacao::Elemento { id, nome, atributos });
    }

    fn posicao_formatacao(&self, id: Id) -> Option<usize> {
        self.formatacao.iter().position(|f| matches!(f, Formatacao::Elemento { id: x, .. } if *x == id))
    }

    fn reconstruir(&mut self) {
        let n = self.formatacao.len();
        if n == 0 {
            return;
        }
        let aberto = |c: &Self, f: &Formatacao| match f {
            Formatacao::Marcador => true,
            Formatacao::Elemento { id, .. } => c.na_pilha(*id),
        };
        if aberto(self, &self.formatacao[n - 1]) {
            return;
        }
        let mut k = n - 1;
        while k > 0 && !aberto(self, &self.formatacao[k - 1]) {
            k -= 1;
        }
        for i in k..n {
            if let Formatacao::Elemento { nome, atributos, .. } = self.formatacao[i].clone() {
                let novo = self.inserir(nome, atributos.clone());
                self.formatacao[i] = Formatacao::Elemento { id: novo, nome, atributos };
            }
        }
    }

    fn limpar_ate_marcador(&mut self) {
        while let Some(f) = self.formatacao.pop() {
            if matches!(f, Formatacao::Marcador) {
                break;
            }
        }
    }

    /// Algoritmo de adocao. false = trate como "qualquer outra tag de fim".
    fn adocao(&mut self, nome: Atomo) -> bool {
        let atual = self.atual();
        if self.nome(atual) == nome && self.posicao_formatacao(atual).is_none() {
            self.pilha.pop();
            return true;
        }
        for _ in 0..8 {
            // o elemento de formatacao: o ultimo com esse nome depois do marcador
            let mut fe = None;
            for (k, f) in self.formatacao.iter().enumerate().rev() {
                match f {
                    Formatacao::Marcador => break,
                    Formatacao::Elemento { id, nome: n, .. } if *n == nome => {
                        fe = Some((k, *id));
                        break;
                    }
                    _ => {}
                }
            }
            let Some((_, fe_id)) = fe else { return false };
            let Some(fe_pilha) = self.pilha.iter().position(|&x| x == fe_id) else {
                if let Some(k) = self.posicao_formatacao(fe_id) {
                    self.formatacao.remove(k);
                }
                return true;
            };
            if !self.no_escopo(nome, Escopo::Normal) {
                return true;
            }
            let bloco = (fe_pilha + 1..self.pilha.len()).find(|&k| self.especial(self.pilha[k]));
            let Some(bloco_k) = bloco else {
                self.pilha.truncate(fe_pilha);
                if let Some(k) = self.posicao_formatacao(fe_id) {
                    self.formatacao.remove(k);
                }
                return true;
            };
            let bloco_id = self.pilha[bloco_k];
            let ancestral = self.pilha[fe_pilha - 1];
            let mut marca = self.posicao_formatacao(fe_id).unwrap_or(0);
            let mut no_k = bloco_k;
            let mut ultimo = bloco_id;
            let mut contador = 0;
            loop {
                contador += 1;
                no_k -= 1;
                let no = self.pilha[no_k];
                if no == fe_id {
                    break;
                }
                let pos = self.posicao_formatacao(no);
                if contador > 3 {
                    if let Some(p) = pos {
                        self.formatacao.remove(p);
                        if p < marca {
                            marca -= 1;
                        }
                    }
                }
                let Some(p) = self.posicao_formatacao(no) else {
                    self.pilha.remove(no_k);
                    continue;
                };
                let (n2, at2) = match &self.formatacao[p] {
                    Formatacao::Elemento { nome, atributos, .. } => (*nome, atributos.clone()),
                    Formatacao::Marcador => unreachable!(),
                };
                let novo = self.criar(n2, Espaco::Html, at2.clone());
                self.formatacao[p] = Formatacao::Elemento { id: novo, nome: n2, atributos: at2 };
                self.pilha[no_k] = novo;
                if ultimo == bloco_id {
                    marca = p + 1;
                }
                self.doc.anexar(novo, ultimo);
                ultimo = novo;
            }
            // o ultimo vai para o ancestral comum (com foster parenting)
            self.doc.soltar(ultimo);
            let (pai, antes) = self.lugar(ancestral);
            self.doc.inserir_antes(pai, ultimo, antes);
            // elemento novo com os filhos do bloco
            let (n3, at3) = match self.posicao_formatacao(fe_id).map(|k| self.formatacao[k].clone()) {
                Some(Formatacao::Elemento { nome, atributos, .. }) => (nome, atributos),
                _ => (nome, Vec::new()),
            };
            let novo = self.criar(n3, Espaco::Html, at3.clone());
            while let Some(f) = self.doc.filhos(bloco_id).next() {
                self.doc.anexar(novo, f);
            }
            self.doc.anexar(bloco_id, novo);
            if let Some(k) = self.posicao_formatacao(fe_id) {
                self.formatacao.remove(k);
                if k < marca {
                    marca -= 1;
                }
            }
            let marca = marca.min(self.formatacao.len());
            self.formatacao.insert(marca, Formatacao::Elemento { id: novo, nome: n3, atributos: at3 });
            self.tirar_da_pilha(fe_id);
            let k = self.pilha.iter().position(|&x| x == bloco_id).map_or(self.pilha.len(), |k| k + 1);
            self.pilha.insert(k, novo);
        }
        true
    }

    // ------------------------------------------------------------- modos

    fn redefinir_modo(&mut self) {
        for (k, &id) in self.pilha.iter().enumerate().rev() {
            let ultimo = k == 0;
            let n = self.nome(id);
            self.modo = match n {
                n if n == a::SELECT => {
                    let em_tabela = self.pilha[..k].iter().any(|&x| self.nome(x) == a::TABLE);
                    if em_tabela { Modo::NoSelectNaTabela } else { Modo::NoSelect }
                }
                n if (n == a::TD || n == a::TH) && !ultimo => Modo::NaCelula,
                n if n == a::TR => Modo::NaLinha,
                n if n == a::TBODY || n == a::THEAD || n == a::TFOOT => Modo::NoCorpoTabela,
                n if n == a::CAPTION => Modo::NaLegenda,
                n if n == a::COLGROUP => Modo::NoGrupoColunas,
                n if n == a::TABLE => Modo::NaTabela,
                n if n == a::HEAD && !ultimo => Modo::NoHead,
                n if n == a::BODY => Modo::NoBody,
                n if n == a::FRAMESET => Modo::NoFrameset,
                n if n == a::HTML => {
                    if self.cabeca == NENHUM { Modo::AntesHead } else { Modo::DepoisHead }
                }
                _ if ultimo => Modo::NoBody,
                _ => continue,
            };
            return;
        }
        self.modo = Modo::NoBody;
    }

    /// O no "atual ajustado" esta em SVG/MathML e o token nao e excecao?
    fn estrangeiro(&self, t: &Token) -> bool {
        let atual = self.atual();
        let Some(e) = self.doc.elemento(atual) else { return false };
        if e.espaco == Espaco::Html || matches!(t, Token::FimDoArquivo) {
            return false;
        }
        let integracao_texto = e.espaco == Espaco::MathMl && [a::MI, a::MO, a::MN, a::MS, a::MTEXT].contains(&e.nome);
        let integracao_html = (e.espaco == Espaco::Svg && [a::FOREIGN_OBJECT, a::DESC, a::TITLE].contains(&e.nome))
            || (e.espaco == Espaco::MathMl
                && e.nome == a::ANNOTATION_XML
                && e.atributo(a::ENCODING).is_some_and(|v| v.eq_ignore_ascii_case("text/html") || v.eq_ignore_ascii_case("application/xhtml+xml")));
        match t {
            Token::Inicio { nome, .. } => {
                if integracao_texto && nome.texto() != "mglyph" && nome.texto() != "malignmark" {
                    return false;
                }
                if e.espaco == Espaco::MathMl && e.nome == a::ANNOTATION_XML && *nome == a::SVG {
                    return false;
                }
                !integracao_html
            }
            Token::Texto(_) => !(integracao_texto || integracao_html),
            _ => true,
        }
    }

    fn despachar(&mut self, t: Token) {
        self.tk.cdata = self.doc.elemento(self.atual()).is_some_and(|e| e.espaco != Espaco::Html);
        if self.estrangeiro(&t) {
            self.conteudo_estrangeiro(t);
        } else {
            self.processar(self.modo, t);
        }
    }

    fn conteudo_estrangeiro(&mut self, t: Token) {
        match t {
            Token::Texto(s) => {
                let s = s.replace('\0', "\u{FFFD}");
                if !so_espacos(&s) {
                    self.frameset_ok = false;
                }
                self.inserir_texto(&s);
            }
            Token::Comentario(c) => self.inserir_comentario(c, None),
            Token::Doctype { .. } => {}
            Token::Inicio { nome, atributos, auto_fecha } => {
                use a::*;
                let fuga = [
                    B, BIG, BLOCKQUOTE, BODY, BR, CENTER, CODE, DD, DIV, DL, DT, EM, EMBED, H1, H2, H3, H4, H5, H6, HEAD,
                    HR, I, IMG, LI, LISTING, MENU, META, NOBR, OL, P, PRE, RUBY, S, SMALL, SPAN, STRONG, STRIKE, SUB,
                    SUP, TABLE, TT, U, UL, VAR,
                ]
                .contains(&nome)
                    || (nome == FONT && atributos.iter().any(|x| [COLOR, FACE, SIZE].contains(&x.nome)));
                if fuga {
                    while self.pilha.len() > 1 {
                        let at = self.atual();
                        let e = self.doc.elemento(at);
                        let fora = e.is_none_or(|e| e.espaco == Espaco::Html)
                            || e.is_some_and(|e| {
                                (e.espaco == Espaco::Svg && [FOREIGN_OBJECT, DESC, TITLE].contains(&e.nome))
                                    || (e.espaco == Espaco::MathMl && [MI, MO, MN, MS, MTEXT].contains(&e.nome))
                            });
                        if fora {
                            break;
                        }
                        self.pilha.pop();
                    }
                    self.processar(self.modo, Token::Inicio { nome, atributos, auto_fecha });
                    return;
                }
                let espaco = self.espaco(self.atual());
                self.inserir_estrangeiro(nome, atributos, espaco);
                if auto_fecha {
                    self.pilha.pop();
                }
            }
            Token::Fim { nome } => {
                let mut k = self.pilha.len();
                while k > 1 {
                    k -= 1;
                    let id = self.pilha[k];
                    let e = match self.doc.elemento(id) {
                        Some(e) => e,
                        None => continue,
                    };
                    if e.espaco == Espaco::Html {
                        self.processar(self.modo, Token::Fim { nome });
                        return;
                    }
                    if e.nome.texto().eq_ignore_ascii_case(nome.texto()) {
                        self.pilha.truncate(k);
                        return;
                    }
                }
            }
            Token::FimDoArquivo => {}
        }
    }

    fn processar(&mut self, modo: Modo, t: Token) {
        match modo {
            Modo::Inicial => self.inicial(t),
            Modo::AntesHtml => self.antes_html(t),
            Modo::AntesHead => self.antes_head(t),
            Modo::NoHead => self.no_head(t),
            Modo::DepoisHead => self.depois_head(t),
            Modo::NoBody => self.no_body(t),
            Modo::Texto => self.texto(t),
            Modo::NaTabela => self.na_tabela(t),
            Modo::TextoDeTabela => self.texto_de_tabela(t),
            Modo::NaLegenda => self.na_legenda(t),
            Modo::NoGrupoColunas => self.no_grupo_colunas(t),
            Modo::NoCorpoTabela => self.no_corpo_tabela(t),
            Modo::NaLinha => self.na_linha(t),
            Modo::NaCelula => self.na_celula(t),
            Modo::NoSelect => self.no_select(t),
            Modo::NoSelectNaTabela => self.no_select_na_tabela(t),
            Modo::DepoisBody => self.depois_body(t),
            Modo::NoFrameset | Modo::DepoisFrameset => self.frameset(t),
            Modo::DepoisDepoisBody | Modo::DepoisDepoisFrameset => self.depois_depois(t),
        }
    }

    /// Texto com espacos na frente: os espacos vao para `f_espacos`, o
    /// resto para `f_resto`. Devolve o resto (se houver).
    fn dividir_texto(&mut self, s: String) -> (String, String) {
        let (e, r) = separar_espacos(&s);
        (String::from(e), String::from(r))
    }

    fn inicial(&mut self, t: Token) {
        match t {
            Token::Texto(s) => {
                let (_, r) = self.dividir_texto(s);
                if !r.is_empty() {
                    self.doc.modo = ModoDoc::Quirks;
                    self.modo = Modo::AntesHtml;
                    self.processar(Modo::AntesHtml, Token::Texto(r));
                }
            }
            Token::Comentario(c) => self.inserir_comentario(c, Some(RAIZ)),
            Token::Doctype { nome, publico, sistema, quirks } => {
                self.doc.modo = modo_do_doctype(&nome, publico.as_deref(), sistema.as_deref(), quirks);
                let id = self.doc.criar(Tipo::Doctype(nome));
                self.doc.anexar(RAIZ, id);
                self.modo = Modo::AntesHtml;
            }
            t => {
                self.doc.modo = ModoDoc::Quirks;
                self.modo = Modo::AntesHtml;
                self.processar(Modo::AntesHtml, t);
            }
        }
    }

    fn criar_html(&mut self, atributos: Vec<Atributo>) {
        let id = self.criar(a::HTML, Espaco::Html, atributos);
        self.doc.anexar(RAIZ, id);
        self.pilha.push(id);
        self.modo = Modo::AntesHead;
    }

    fn antes_html(&mut self, t: Token) {
        match t {
            Token::Doctype { .. } => {}
            Token::Comentario(c) => self.inserir_comentario(c, Some(RAIZ)),
            Token::Texto(s) => {
                let (_, r) = self.dividir_texto(s);
                if !r.is_empty() {
                    self.criar_html(Vec::new());
                    self.processar(Modo::AntesHead, Token::Texto(r));
                }
            }
            Token::Inicio { nome, atributos, .. } if nome == a::HTML => self.criar_html(atributos),
            Token::Fim { nome } if ![a::HEAD, a::BODY, a::HTML, a::BR].contains(&nome) => {}
            t => {
                self.criar_html(Vec::new());
                self.processar(Modo::AntesHead, t);
            }
        }
    }

    fn antes_head(&mut self, t: Token) {
        match t {
            Token::Texto(s) => {
                let (_, r) = self.dividir_texto(s);
                if !r.is_empty() {
                    self.cabeca = self.inserir(a::HEAD, Vec::new());
                    self.modo = Modo::NoHead;
                    self.processar(Modo::NoHead, Token::Texto(r));
                }
            }
            Token::Comentario(c) => self.inserir_comentario(c, None),
            Token::Doctype { .. } => {}
            Token::Inicio { nome, .. } if nome == a::HTML => self.no_body(t),
            Token::Inicio { nome, atributos, .. } if nome == a::HEAD => {
                self.cabeca = self.inserir(a::HEAD, atributos);
                self.modo = Modo::NoHead;
            }
            Token::Fim { nome } if ![a::HEAD, a::BODY, a::HTML, a::BR].contains(&nome) => {}
            t => {
                self.cabeca = self.inserir(a::HEAD, Vec::new());
                self.modo = Modo::NoHead;
                self.processar(Modo::NoHead, t);
            }
        }
    }

    fn texto_generico(&mut self, nome: Atomo, atributos: Vec<Atributo>, estado: Estado) {
        self.inserir(nome, atributos);
        self.tk.estado = estado;
        self.modo_original = self.modo;
        self.modo = Modo::Texto;
    }

    fn no_head(&mut self, t: Token) {
        match t {
            Token::Texto(s) => {
                let (e, r) = self.dividir_texto(s);
                self.inserir_texto(&e);
                if !r.is_empty() {
                    self.pilha.pop();
                    self.modo = Modo::DepoisHead;
                    self.processar(Modo::DepoisHead, Token::Texto(r));
                }
            }
            Token::Comentario(c) => self.inserir_comentario(c, None),
            Token::Doctype { .. } => {}
            Token::Inicio { nome, .. } if nome == a::HTML => self.no_body(t),
            Token::Inicio { nome, atributos, .. } if [a::BASE, a::BASEFONT, a::BGSOUND, a::LINK, a::META].contains(&nome) => {
                self.inserir(nome, atributos);
                self.pilha.pop();
            }
            Token::Inicio { nome, atributos, .. } if nome == a::TITLE => self.texto_generico(nome, atributos, Estado::Rcdata),
            Token::Inicio { nome, atributos, .. } if [a::NOSCRIPT, a::NOFRAMES, a::STYLE].contains(&nome) => {
                self.texto_generico(nome, atributos, Estado::Rawtext)
            }
            Token::Inicio { nome, atributos, .. } if nome == a::SCRIPT => self.texto_generico(nome, atributos, Estado::Script),
            Token::Inicio { nome, atributos, .. } if nome == a::TEMPLATE => {
                self.inserir(nome, atributos);
                self.formatacao.push(Formatacao::Marcador);
                self.frameset_ok = false;
            }
            Token::Fim { nome } if nome == a::TEMPLATE => {
                if self.pilha.iter().any(|&x| self.nome(x) == a::TEMPLATE) {
                    self.gerar_fins_total();
                    self.desempilhar_ate(a::TEMPLATE);
                    self.limpar_ate_marcador();
                    self.redefinir_modo();
                }
            }
            Token::Fim { nome } if nome == a::HEAD => {
                self.pilha.pop();
                self.modo = Modo::DepoisHead;
            }
            Token::Inicio { nome, .. } if nome == a::HEAD => {}
            Token::Fim { nome } if ![a::BODY, a::HTML, a::BR].contains(&nome) => {}
            t => {
                self.pilha.pop();
                self.modo = Modo::DepoisHead;
                self.processar(Modo::DepoisHead, t);
            }
        }
    }

    fn depois_head(&mut self, t: Token) {
        match t {
            Token::Texto(s) => {
                let (e, r) = self.dividir_texto(s);
                self.inserir_texto(&e);
                if !r.is_empty() {
                    self.inserir(a::BODY, Vec::new());
                    self.modo = Modo::NoBody;
                    self.processar(Modo::NoBody, Token::Texto(r));
                }
            }
            Token::Comentario(c) => self.inserir_comentario(c, None),
            Token::Doctype { .. } => {}
            Token::Inicio { nome, .. } if nome == a::HTML => self.no_body(t),
            Token::Inicio { nome, atributos, .. } if nome == a::BODY => {
                self.inserir(a::BODY, atributos);
                self.frameset_ok = false;
                self.modo = Modo::NoBody;
            }
            Token::Inicio { nome, atributos, .. } if nome == a::FRAMESET => {
                self.inserir(a::FRAMESET, atributos);
                self.modo = Modo::NoFrameset;
            }
            Token::Inicio { nome, .. }
                if [a::BASE, a::BASEFONT, a::BGSOUND, a::LINK, a::META, a::NOFRAMES, a::SCRIPT, a::STYLE, a::TEMPLATE, a::TITLE]
                    .contains(&nome) =>
            {
                let cabeca = self.cabeca;
                if cabeca != NENHUM {
                    self.pilha.push(cabeca);
                }
                self.no_head(t);
                if cabeca != NENHUM {
                    self.tirar_da_pilha(cabeca);
                }
            }
            Token::Fim { nome } if nome == a::TEMPLATE => self.no_head(t),
            Token::Inicio { nome, .. } if nome == a::HEAD => {}
            Token::Fim { nome } if ![a::BODY, a::HTML, a::BR].contains(&nome) => {}
            t => {
                self.inserir(a::BODY, Vec::new());
                self.modo = Modo::NoBody;
                self.processar(Modo::NoBody, t);
            }
        }
    }

    fn texto(&mut self, t: Token) {
        match t {
            Token::Texto(s) => self.inserir_texto(&s),
            Token::FimDoArquivo => {
                self.pilha.pop();
                self.modo = self.modo_original;
                self.tk.estado = Estado::Dados;
                self.processar(self.modo, Token::FimDoArquivo);
            }
            _ => {
                // so pode ser o fim da tag (o tokenizador garante)
                self.pilha.pop();
                self.modo = self.modo_original;
                self.tk.estado = Estado::Dados;
            }
        }
    }

    fn juntar_atributos(&mut self, alvo: Id, atributos: Vec<Atributo>) {
        if let Some(e) = self.doc.elemento_mut(alvo) {
            for at in atributos {
                if !e.tem(at.nome) {
                    e.atributos.push(at);
                }
            }
            e.atualizar_cache();
        }
    }

    fn no_body(&mut self, t: Token) {
        use a::*;
        match t {
            Token::Texto(s) => {
                if s.is_empty() {
                    return;
                }
                self.reconstruir();
                if !so_espacos(&s) {
                    self.frameset_ok = false;
                }
                self.inserir_texto(&s);
            }
            Token::Comentario(c) => self.inserir_comentario(c, None),
            Token::Doctype { .. } => {}
            Token::FimDoArquivo => self.parar = true,
            Token::Inicio { nome, atributos, auto_fecha } => self.no_body_inicio(nome, atributos, auto_fecha),
            Token::Fim { nome } => {
                if nome == TEMPLATE {
                    return self.no_head(Token::Fim { nome });
                }
                if nome == BODY || nome == HTML {
                    if !self.no_escopo(BODY, Escopo::Normal) {
                        return;
                    }
                    self.modo = Modo::DepoisBody;
                    if nome == HTML {
                        self.depois_body(Token::Fim { nome });
                    }
                    return;
                }
                let blocos = [
                    ADDRESS, ARTICLE, ASIDE, BLOCKQUOTE, BUTTON, CENTER, DETAILS, DIALOG, DIR, DIV, DL, FIELDSET,
                    FIGCAPTION, FIGURE, FOOTER, HEADER, HGROUP, LISTING, MAIN, MENU, NAV, OL, PRE, SEARCH, SECTION,
                    SUMMARY, UL,
                ];
                if blocos.contains(&nome) {
                    if self.no_escopo(nome, Escopo::Normal) {
                        self.gerar_fins(a::VAZIO);
                        self.desempilhar_ate(nome);
                    }
                    return;
                }
                if nome == FORM {
                    let f = self.formulario;
                    self.formulario = NENHUM;
                    if f == NENHUM || !self.na_pilha(f) || !self.no_escopo(FORM, Escopo::Normal) {
                        return;
                    }
                    self.gerar_fins(a::VAZIO);
                    self.tirar_da_pilha(f);
                    return;
                }
                if nome == P {
                    if !self.no_escopo(P, Escopo::Botao) {
                        self.inserir(P, Vec::new());
                    }
                    self.fechar_p();
                    return;
                }
                if nome == LI {
                    if self.no_escopo(LI, Escopo::Lista) {
                        self.gerar_fins(LI);
                        self.desempilhar_ate(LI);
                    }
                    return;
                }
                if nome == DD || nome == DT {
                    if self.no_escopo(nome, Escopo::Normal) {
                        self.gerar_fins(nome);
                        self.desempilhar_ate(nome);
                    }
                    return;
                }
                if H.contains(&nome) {
                    if self.no_escopo_algum(&H, Escopo::Normal) {
                        self.gerar_fins(a::VAZIO);
                        self.desempilhar_ate_algum(&H);
                    }
                    return;
                }
                if formatacao_nome(nome) {
                    if self.adocao(nome) {
                        return;
                    }
                } else if nome == APPLET || nome == MARQUEE || nome == OBJECT {
                    if self.no_escopo(nome, Escopo::Normal) {
                        self.gerar_fins(a::VAZIO);
                        self.desempilhar_ate(nome);
                        self.limpar_ate_marcador();
                    }
                    return;
                } else if nome == BR {
                    return self.no_body_inicio(BR, Vec::new(), false);
                }
                self.qualquer_fim(nome);
            }
        }
    }

    fn qualquer_fim(&mut self, nome: Atomo) {
        for k in (0..self.pilha.len()).rev() {
            let id = self.pilha[k];
            if self.nome(id) == nome {
                self.gerar_fins(nome);
                self.pilha.truncate(k);
                return;
            }
            if self.especial(id) {
                return;
            }
        }
    }

    fn no_body_inicio(&mut self, nome: Atomo, atributos: Vec<Atributo>, auto_fecha: bool) {
        use a::*;
        let t = || Token::Inicio { nome, atributos: Vec::new(), auto_fecha };
        if nome == HTML {
            let html = self.pilha.first().copied().unwrap_or(NENHUM);
            if html != NENHUM {
                self.juntar_atributos(html, atributos);
            }
            return;
        }
        if [BASE, BASEFONT, BGSOUND, LINK, META, NOFRAMES, SCRIPT, STYLE, TEMPLATE, TITLE].contains(&nome) {
            return self.no_head(Token::Inicio { nome, atributos, auto_fecha });
        }
        if nome == BODY {
            if let Some(&b) = self.pilha.get(1) {
                if self.nome(b) == BODY {
                    self.frameset_ok = false;
                    self.juntar_atributos(b, atributos);
                }
            }
            return;
        }
        if nome == FRAMESET {
            if !self.frameset_ok || self.pilha.get(1).is_none_or(|&b| self.nome(b) != BODY) {
                return;
            }
            let b = self.pilha[1];
            self.doc.soltar(b);
            self.pilha.truncate(1);
            self.inserir(FRAMESET, atributos);
            self.modo = Modo::NoFrameset;
            return;
        }
        let blocos = [
            ADDRESS, ARTICLE, ASIDE, BLOCKQUOTE, CENTER, DETAILS, DIALOG, DIR, DIV, DL, FIELDSET, FIGCAPTION, FIGURE,
            FOOTER, HEADER, HGROUP, MAIN, MENU, NAV, OL, P, SEARCH, SECTION, SUMMARY, UL,
        ];
        if blocos.contains(&nome) {
            self.fechar_p();
            self.inserir(nome, atributos);
            return;
        }
        if H.contains(&nome) {
            self.fechar_p();
            if H.contains(&self.nome(self.atual())) {
                self.pilha.pop();
            }
            self.inserir(nome, atributos);
            return;
        }
        if nome == PRE || nome == LISTING {
            self.fechar_p();
            self.inserir(nome, atributos);
            self.pular_lf = true;
            self.frameset_ok = false;
            return;
        }
        if nome == FORM {
            if self.formulario != NENHUM {
                return;
            }
            self.fechar_p();
            self.formulario = self.inserir(nome, atributos);
            return;
        }
        if nome == LI || nome == DD || nome == DT {
            self.frameset_ok = false;
            let alvos: &[Atomo] = if nome == LI { &[LI] } else { &[DD, DT] };
            for k in (0..self.pilha.len()).rev() {
                let id = self.pilha[k];
                let n = self.nome(id);
                if alvos.contains(&n) {
                    self.gerar_fins(n);
                    self.desempilhar_ate(n);
                    break;
                }
                if self.especial(id) && ![ADDRESS, DIV, P].contains(&n) {
                    break;
                }
            }
            self.fechar_p();
            self.inserir(nome, atributos);
            return;
        }
        if nome == PLAINTEXT {
            self.fechar_p();
            self.inserir(nome, atributos);
            self.tk.estado = Estado::Plaintext;
            return;
        }
        if nome == BUTTON {
            if self.no_escopo(BUTTON, Escopo::Normal) {
                self.gerar_fins(a::VAZIO);
                self.desempilhar_ate(BUTTON);
            }
            self.reconstruir();
            self.inserir(nome, atributos);
            self.frameset_ok = false;
            return;
        }
        if nome == A {
            let mut achou = None;
            for f in self.formatacao.iter().rev() {
                match f {
                    Formatacao::Marcador => break,
                    Formatacao::Elemento { id, nome: n, .. } if *n == A => {
                        achou = Some(*id);
                        break;
                    }
                    _ => {}
                }
            }
            if let Some(velho) = achou {
                self.adocao(A);
                if let Some(k) = self.posicao_formatacao(velho) {
                    self.formatacao.remove(k);
                }
                self.tirar_da_pilha(velho);
            }
            self.reconstruir();
            let id = self.inserir(nome, atributos.clone());
            self.empurrar_formatacao(id, nome, atributos);
            return;
        }
        if [B, BIG, CODE, EM, FONT, I, S, SMALL, STRIKE, STRONG, TT, U].contains(&nome) {
            self.reconstruir();
            let id = self.inserir(nome, atributos.clone());
            self.empurrar_formatacao(id, nome, atributos);
            return;
        }
        if nome == NOBR {
            self.reconstruir();
            if self.no_escopo(NOBR, Escopo::Normal) {
                self.adocao(NOBR);
                self.reconstruir();
            }
            let id = self.inserir(nome, atributos.clone());
            self.empurrar_formatacao(id, nome, atributos);
            return;
        }
        if nome == APPLET || nome == MARQUEE || nome == OBJECT {
            self.reconstruir();
            self.inserir(nome, atributos);
            self.formatacao.push(Formatacao::Marcador);
            self.frameset_ok = false;
            return;
        }
        if nome == TABLE {
            if self.doc.modo != ModoDoc::Quirks {
                self.fechar_p();
            }
            self.inserir(nome, atributos);
            self.frameset_ok = false;
            self.modo = Modo::NaTabela;
            return;
        }
        if [AREA, BR, EMBED, IMG, KEYGEN, WBR, IMAGE].contains(&nome) {
            self.reconstruir();
            let n = if nome == IMAGE { IMG } else { nome };
            self.inserir(n, atributos);
            self.pilha.pop();
            self.frameset_ok = false;
            return;
        }
        if nome == INPUT {
            let escondido = atributos.iter().any(|x| x.nome == TYPE && x.valor.eq_ignore_ascii_case("hidden"));
            self.reconstruir();
            self.inserir(nome, atributos);
            self.pilha.pop();
            if !escondido {
                self.frameset_ok = false;
            }
            return;
        }
        if nome == PARAM || nome == SOURCE || nome == TRACK {
            self.inserir(nome, atributos);
            self.pilha.pop();
            return;
        }
        if nome == HR {
            self.fechar_p();
            self.inserir(nome, atributos);
            self.pilha.pop();
            self.frameset_ok = false;
            return;
        }
        if nome == TEXTAREA {
            self.inserir(nome, atributos);
            self.pular_lf = true;
            self.tk.estado = Estado::Rcdata;
            self.modo_original = self.modo;
            self.frameset_ok = false;
            self.modo = Modo::Texto;
            return;
        }
        if nome == XMP {
            self.fechar_p();
            self.reconstruir();
            self.frameset_ok = false;
            return self.texto_generico(nome, atributos, Estado::Rawtext);
        }
        if nome == IFRAME {
            self.frameset_ok = false;
            return self.texto_generico(nome, atributos, Estado::Rawtext);
        }
        if nome == NOEMBED || nome == NOSCRIPT {
            return self.texto_generico(nome, atributos, Estado::Rawtext);
        }
        if nome == SELECT {
            self.reconstruir();
            self.inserir(nome, atributos);
            self.frameset_ok = false;
            self.modo = if matches!(
                self.modo,
                Modo::NaTabela | Modo::NaLegenda | Modo::NoCorpoTabela | Modo::NaLinha | Modo::NaCelula
            ) {
                Modo::NoSelectNaTabela
            } else {
                Modo::NoSelect
            };
            return;
        }
        if nome == OPTGROUP || nome == OPTION {
            if self.atual_eh(OPTION) {
                self.pilha.pop();
            }
            self.reconstruir();
            self.inserir(nome, atributos);
            return;
        }
        if nome == RB || nome == RTC {
            if self.no_escopo(RUBY, Escopo::Normal) {
                self.gerar_fins(a::VAZIO);
            }
            self.inserir(nome, atributos);
            return;
        }
        if nome == RP || nome == RT {
            if self.no_escopo(RUBY, Escopo::Normal) {
                self.gerar_fins(RTC);
            }
            self.inserir(nome, atributos);
            return;
        }
        if nome == MATH || nome == SVG {
            self.reconstruir();
            let espaco = if nome == MATH { Espaco::MathMl } else { Espaco::Svg };
            self.inserir_estrangeiro(nome, atributos, espaco);
            if auto_fecha {
                self.pilha.pop();
            }
            return;
        }
        if [CAPTION, COL, COLGROUP, FRAME, HEAD, TBODY, TD, TFOOT, TH, THEAD, TR].contains(&nome) {
            let _ = t;
            return;
        }
        self.reconstruir();
        self.inserir(nome, atributos);
    }

    // ----------------------------------------------------------- tabelas

    fn limpar_contexto(&mut self, nomes: &[Atomo]) {
        while self.pilha.len() > 1 && !nomes.contains(&self.nome(self.atual())) {
            self.pilha.pop();
        }
    }

    fn na_tabela(&mut self, t: Token) {
        use a::*;
        match t {
            Token::Texto(s) if [TABLE, TBODY, TEMPLATE, TFOOT, THEAD, TR].contains(&self.nome(self.atual())) => {
                self.texto_tabela.clear();
                self.modo_original = self.modo;
                self.modo = Modo::TextoDeTabela;
                self.texto_de_tabela(Token::Texto(s));
            }
            Token::Comentario(c) => self.inserir_comentario(c, None),
            Token::Doctype { .. } => {}
            Token::Inicio { nome, atributos, .. } if nome == CAPTION => {
                self.limpar_contexto(&[TABLE, TEMPLATE, HTML]);
                self.formatacao.push(Formatacao::Marcador);
                self.inserir(nome, atributos);
                self.modo = Modo::NaLegenda;
            }
            Token::Inicio { nome, atributos, .. } if nome == COLGROUP => {
                self.limpar_contexto(&[TABLE, TEMPLATE, HTML]);
                self.inserir(nome, atributos);
                self.modo = Modo::NoGrupoColunas;
            }
            Token::Inicio { nome, .. } if nome == COL => {
                self.limpar_contexto(&[TABLE, TEMPLATE, HTML]);
                self.inserir(COLGROUP, Vec::new());
                self.modo = Modo::NoGrupoColunas;
                self.no_grupo_colunas(t);
            }
            Token::Inicio { nome, atributos, .. } if [TBODY, TFOOT, THEAD].contains(&nome) => {
                self.limpar_contexto(&[TABLE, TEMPLATE, HTML]);
                self.inserir(nome, atributos);
                self.modo = Modo::NoCorpoTabela;
            }
            Token::Inicio { nome, .. } if [TD, TH, TR].contains(&nome) => {
                self.limpar_contexto(&[TABLE, TEMPLATE, HTML]);
                self.inserir(TBODY, Vec::new());
                self.modo = Modo::NoCorpoTabela;
                self.no_corpo_tabela(t);
            }
            Token::Inicio { nome, .. } if nome == TABLE => {
                if self.no_escopo(TABLE, Escopo::Tabela) {
                    self.desempilhar_ate(TABLE);
                    self.redefinir_modo();
                    self.processar(self.modo, t);
                }
            }
            Token::Fim { nome } if nome == TABLE => {
                if self.no_escopo(TABLE, Escopo::Tabela) {
                    self.desempilhar_ate(TABLE);
                    self.redefinir_modo();
                }
            }
            Token::Fim { nome } if [BODY, CAPTION, COL, COLGROUP, HTML, TBODY, TD, TFOOT, TH, THEAD, TR].contains(&nome) => {}
            Token::Inicio { nome, .. } if [STYLE, SCRIPT, TEMPLATE].contains(&nome) => self.no_head(t),
            Token::Fim { nome } if nome == TEMPLATE => self.no_head(t),
            Token::Inicio { nome, ref atributos, .. }
                if nome == INPUT && atributos.iter().any(|x| x.nome == TYPE && x.valor.eq_ignore_ascii_case("hidden")) =>
            {
                if let Token::Inicio { atributos, .. } = t {
                    self.inserir(INPUT, atributos);
                    self.pilha.pop();
                }
            }
            Token::Inicio { nome, atributos, .. } if nome == FORM => {
                if self.formulario == NENHUM {
                    self.formulario = self.inserir(FORM, atributos);
                    self.pilha.pop();
                }
            }
            Token::FimDoArquivo => self.no_body(t),
            t => {
                self.adotivo = true;
                self.no_body(t);
                self.adotivo = false;
            }
        }
    }

    fn texto_de_tabela(&mut self, t: Token) {
        match t {
            Token::Texto(s) => self.texto_tabela.push_str(&s),
            t => {
                let texto = core::mem::take(&mut self.texto_tabela);
                if !so_espacos(&texto) {
                    self.adotivo = true;
                    self.no_body(Token::Texto(texto));
                    self.adotivo = false;
                } else {
                    self.inserir_texto(&texto);
                }
                self.modo = self.modo_original;
                self.processar(self.modo, t);
            }
        }
    }

    fn na_legenda(&mut self, t: Token) {
        use a::*;
        let fechar = match &t {
            Token::Fim { nome } if *nome == CAPTION => true,
            Token::Inicio { nome, .. } if [CAPTION, COL, COLGROUP, TBODY, TD, TFOOT, TH, THEAD, TR].contains(nome) => true,
            Token::Fim { nome } if *nome == TABLE => true,
            Token::Fim { nome } if [BODY, COL, COLGROUP, HTML, TBODY, TD, TFOOT, TH, THEAD, TR].contains(nome) => return,
            _ => false,
        };
        if fechar {
            if !self.no_escopo(CAPTION, Escopo::Tabela) {
                return;
            }
            self.gerar_fins(a::VAZIO);
            self.desempilhar_ate(CAPTION);
            self.limpar_ate_marcador();
            self.modo = Modo::NaTabela;
            if !matches!(&t, Token::Fim { nome } if *nome == CAPTION) {
                self.na_tabela(t);
            }
            return;
        }
        self.no_body(t);
    }

    fn no_grupo_colunas(&mut self, t: Token) {
        use a::*;
        match t {
            Token::Texto(s) => {
                let (e, r) = self.dividir_texto(s);
                self.inserir_texto(&e);
                if !r.is_empty() {
                    if self.atual_eh(COLGROUP) {
                        self.pilha.pop();
                        self.modo = Modo::NaTabela;
                        self.na_tabela(Token::Texto(r));
                    }
                }
            }
            Token::Comentario(c) => self.inserir_comentario(c, None),
            Token::Inicio { nome, atributos, .. } if nome == COL => {
                self.inserir(COL, atributos);
                self.pilha.pop();
            }
            Token::Fim { nome } if nome == COLGROUP => {
                if self.atual_eh(COLGROUP) {
                    self.pilha.pop();
                    self.modo = Modo::NaTabela;
                }
            }
            Token::Fim { nome } if nome == COL => {}
            Token::FimDoArquivo => self.no_body(t),
            t => {
                if self.atual_eh(COLGROUP) {
                    self.pilha.pop();
                    self.modo = Modo::NaTabela;
                    self.na_tabela(t);
                }
            }
        }
    }

    fn no_corpo_tabela(&mut self, t: Token) {
        use a::*;
        match t {
            Token::Inicio { nome, atributos, .. } if nome == TR => {
                self.limpar_contexto(&[TBODY, TFOOT, THEAD, TEMPLATE, HTML]);
                self.inserir(TR, atributos);
                self.modo = Modo::NaLinha;
            }
            Token::Inicio { nome, .. } if nome == TH || nome == TD => {
                self.limpar_contexto(&[TBODY, TFOOT, THEAD, TEMPLATE, HTML]);
                self.inserir(TR, Vec::new());
                self.modo = Modo::NaLinha;
                self.na_linha(t);
            }
            Token::Fim { nome } if [TBODY, TFOOT, THEAD].contains(&nome) => {
                if self.no_escopo(nome, Escopo::Tabela) {
                    self.limpar_contexto(&[TBODY, TFOOT, THEAD, TEMPLATE, HTML]);
                    self.pilha.pop();
                    self.modo = Modo::NaTabela;
                }
            }
            Token::Inicio { nome, .. } if [CAPTION, COL, COLGROUP, TBODY, TFOOT, THEAD].contains(&nome) => self.sair_corpo(t),
            Token::Fim { nome } if nome == TABLE => self.sair_corpo(t),
            Token::Fim { nome } if [BODY, CAPTION, COL, COLGROUP, HTML, TD, TH, TR].contains(&nome) => {}
            t => self.na_tabela(t),
        }
    }

    fn sair_corpo(&mut self, t: Token) {
        use a::*;
        if !self.no_escopo_algum(&[TBODY, THEAD, TFOOT], Escopo::Tabela) {
            return;
        }
        self.limpar_contexto(&[TBODY, TFOOT, THEAD, TEMPLATE, HTML]);
        self.pilha.pop();
        self.modo = Modo::NaTabela;
        self.na_tabela(t);
    }

    fn na_linha(&mut self, t: Token) {
        use a::*;
        match t {
            Token::Inicio { nome, atributos, .. } if nome == TH || nome == TD => {
                self.limpar_contexto(&[TR, TEMPLATE, HTML]);
                self.inserir(nome, atributos);
                self.modo = Modo::NaCelula;
                self.formatacao.push(Formatacao::Marcador);
            }
            Token::Fim { nome } if nome == TR => {
                if self.no_escopo(TR, Escopo::Tabela) {
                    self.limpar_contexto(&[TR, TEMPLATE, HTML]);
                    self.pilha.pop();
                    self.modo = Modo::NoCorpoTabela;
                }
            }
            Token::Inicio { nome, .. } if [CAPTION, COL, COLGROUP, TBODY, TFOOT, THEAD, TR].contains(&nome) => self.sair_linha(t),
            Token::Fim { nome } if nome == TABLE => self.sair_linha(t),
            Token::Fim { nome } if [TBODY, TFOOT, THEAD].contains(&nome) => {
                if self.no_escopo(nome, Escopo::Tabela) && self.no_escopo(TR, Escopo::Tabela) {
                    self.sair_linha(t);
                }
            }
            Token::Fim { nome } if [BODY, CAPTION, COL, COLGROUP, HTML, TD, TH].contains(&nome) => {}
            t => self.na_tabela(t),
        }
    }

    fn sair_linha(&mut self, t: Token) {
        use a::*;
        if !self.no_escopo(TR, Escopo::Tabela) {
            return;
        }
        self.limpar_contexto(&[TR, TEMPLATE, HTML]);
        self.pilha.pop();
        self.modo = Modo::NoCorpoTabela;
        self.no_corpo_tabela(t);
    }

    fn fechar_celula(&mut self) {
        self.gerar_fins(a::VAZIO);
        self.desempilhar_ate_algum(&[a::TD, a::TH]);
        self.limpar_ate_marcador();
        self.modo = Modo::NaLinha;
    }

    fn na_celula(&mut self, t: Token) {
        use a::*;
        match t {
            Token::Fim { nome } if nome == TD || nome == TH => {
                if self.no_escopo(nome, Escopo::Tabela) {
                    self.gerar_fins(a::VAZIO);
                    self.desempilhar_ate(nome);
                    self.limpar_ate_marcador();
                    self.modo = Modo::NaLinha;
                }
            }
            Token::Inicio { nome, .. } if [CAPTION, COL, COLGROUP, TBODY, TD, TFOOT, TH, THEAD, TR].contains(&nome) => {
                if self.no_escopo_algum(&[TD, TH], Escopo::Tabela) {
                    self.fechar_celula();
                    self.na_linha(t);
                }
            }
            Token::Fim { nome } if [BODY, CAPTION, COL, COLGROUP, HTML].contains(&nome) => {}
            Token::Fim { nome } if [TABLE, TBODY, TFOOT, THEAD, TR].contains(&nome) => {
                if self.no_escopo(nome, Escopo::Tabela) {
                    self.fechar_celula();
                    self.na_linha(t);
                }
            }
            t => self.no_body(t),
        }
    }

    fn no_select(&mut self, t: Token) {
        use a::*;
        match t {
            Token::Texto(s) => self.inserir_texto(&s),
            Token::Comentario(c) => self.inserir_comentario(c, None),
            Token::Inicio { nome, .. } if nome == HTML => self.no_body(t),
            Token::Inicio { nome, atributos, .. } if nome == OPTION => {
                if self.atual_eh(OPTION) {
                    self.pilha.pop();
                }
                self.inserir(nome, atributos);
            }
            Token::Inicio { nome, atributos, .. } if nome == OPTGROUP => {
                if self.atual_eh(OPTION) {
                    self.pilha.pop();
                }
                if self.atual_eh(OPTGROUP) {
                    self.pilha.pop();
                }
                self.inserir(nome, atributos);
            }
            Token::Inicio { nome, atributos, .. } if nome == HR => {
                if self.atual_eh(OPTION) {
                    self.pilha.pop();
                }
                if self.atual_eh(OPTGROUP) {
                    self.pilha.pop();
                }
                self.inserir(nome, atributos);
                self.pilha.pop();
            }
            Token::Fim { nome } if nome == OPTGROUP => {
                let n = self.pilha.len();
                if self.atual_eh(OPTION) && n >= 2 && self.nome(self.pilha[n - 2]) == OPTGROUP {
                    self.pilha.pop();
                }
                if self.atual_eh(OPTGROUP) {
                    self.pilha.pop();
                }
            }
            Token::Fim { nome } if nome == OPTION => {
                if self.atual_eh(OPTION) {
                    self.pilha.pop();
                }
            }
            Token::Fim { nome } if nome == SELECT => {
                if self.no_escopo(SELECT, Escopo::Select) {
                    self.desempilhar_ate(SELECT);
                    self.redefinir_modo();
                }
            }
            Token::Inicio { nome, .. } if nome == SELECT => {
                if self.no_escopo(SELECT, Escopo::Select) {
                    self.desempilhar_ate(SELECT);
                    self.redefinir_modo();
                }
            }
            Token::Inicio { nome, .. } if [INPUT, KEYGEN, TEXTAREA].contains(&nome) => {
                if self.no_escopo(SELECT, Escopo::Select) {
                    self.desempilhar_ate(SELECT);
                    self.redefinir_modo();
                    self.processar(self.modo, t);
                }
            }
            Token::Inicio { nome, .. } if nome == SCRIPT || nome == TEMPLATE => self.no_head(t),
            Token::Fim { nome } if nome == TEMPLATE => self.no_head(t),
            Token::FimDoArquivo => self.no_body(t),
            _ => {}
        }
    }

    fn no_select_na_tabela(&mut self, t: Token) {
        use a::*;
        let tabela = [CAPTION, TABLE, TBODY, TFOOT, THEAD, TR, TD, TH];
        match &t {
            Token::Inicio { nome, .. } if tabela.contains(nome) => {
                self.desempilhar_ate(SELECT);
                self.redefinir_modo();
                self.processar(self.modo, t);
            }
            Token::Fim { nome } if tabela.contains(nome) => {
                if self.no_escopo(*nome, Escopo::Tabela) {
                    self.desempilhar_ate(SELECT);
                    self.redefinir_modo();
                    self.processar(self.modo, t);
                }
            }
            _ => self.no_select(t),
        }
    }

    fn depois_body(&mut self, t: Token) {
        match t {
            Token::Texto(ref s) if so_espacos(s) => self.no_body(t),
            Token::Comentario(c) => {
                let html = self.pilha.first().copied();
                self.inserir_comentario(c, html);
            }
            Token::Doctype { .. } => {}
            Token::Inicio { nome, .. } if nome == a::HTML => self.no_body(t),
            Token::Fim { nome } if nome == a::HTML => self.modo = Modo::DepoisDepoisBody,
            Token::FimDoArquivo => self.parar = true,
            t => {
                self.modo = Modo::NoBody;
                self.no_body(t);
            }
        }
    }

    fn frameset(&mut self, t: Token) {
        match t {
            Token::Texto(s) => {
                let espacos: String = s.chars().filter(|&c| eh_espaco(c)).collect();
                self.inserir_texto(&espacos);
            }
            Token::Comentario(c) => self.inserir_comentario(c, None),
            Token::Inicio { nome, atributos, .. } if nome == a::FRAMESET && self.modo == Modo::NoFrameset => {
                self.inserir(nome, atributos);
            }
            Token::Fim { nome } if nome == a::FRAMESET && self.modo == Modo::NoFrameset => {
                if self.pilha.len() > 1 {
                    self.pilha.pop();
                }
                if !self.atual_eh(a::FRAMESET) {
                    self.modo = Modo::DepoisFrameset;
                }
            }
            Token::Inicio { nome, atributos, .. } if nome == a::FRAME => {
                self.inserir(nome, atributos);
                self.pilha.pop();
            }
            Token::Inicio { nome, .. } if nome == a::NOFRAMES => self.no_head(t),
            Token::Fim { nome } if nome == a::HTML => self.modo = Modo::DepoisDepoisFrameset,
            Token::FimDoArquivo => self.parar = true,
            _ => {}
        }
    }

    fn depois_depois(&mut self, t: Token) {
        match t {
            Token::Comentario(c) => self.inserir_comentario(c, Some(RAIZ)),
            Token::Doctype { .. } => {}
            Token::Texto(ref s) if so_espacos(s) => self.no_body(t),
            Token::Inicio { nome, .. } if nome == a::HTML => self.no_body(t),
            Token::FimDoArquivo => self.parar = true,
            t if self.modo == Modo::DepoisDepoisFrameset => {
                if let Token::Inicio { nome, .. } = &t {
                    if *nome == a::NOFRAMES {
                        self.no_head(t);
                    }
                }
            }
            t => {
                self.modo = Modo::NoBody;
                self.no_body(t);
            }
        }
    }
}
