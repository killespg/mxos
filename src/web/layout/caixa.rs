//! A arvore de caixas: o DOM com estilo vira caixas (CSS 2.1, cap. 9).
//! - display:none some; display:contents poe os filhos no lugar;
//! - texto vira caixa de texto com os espacos ja tratados (white-space);
//! - ::before/::after e o marcador de lista viram caixas tambem;
//! - um bloco com filhos de bloco E inline embrulha os inline numa caixa
//!   anonima (para cada filho de um bloco ser so bloco ou so inline);
//! - celulas/linhas soltas ganham a tabela anonima em volta.

use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

use crate::web::atomo::a;
use crate::web::css::cascata::Estilos;
use crate::web::css::estilo::{Brancos, Estilo, Exibir, ParteConteudo, TipoLista, Transformacao};
use crate::web::css::valores::Fixo;
use crate::web::dom::{Documento, Id, NENHUM, RAIZ, Tipo as TipoNo};

/// Elementos que tem conteudo proprio (nao filhos de texto).
#[derive(Clone, Debug, PartialEq)]
pub enum Substituido {
    Imagem { alt: String },
    Entrada { texto: String, senha: bool, marcavel: bool, marcado: bool },
    Botao,
    Selecao { texto: String },
    AreaTexto { texto: String },
    Quadro,
    Video,
    Svg,
    Canvas,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tipo {
    /// Bloco (ou o que vira bloco: item de lista, flow-root...).
    Bloco,
    Inline,
    InlineBloco,
    Flex,
    Grid,
    Tabela,
    GrupoLinhas,
    LinhaTabela,
    Celula,
    Legenda,
    Texto(Rc<str>),
    Quebra,
    Substituido(Substituido),
}

#[derive(Clone, Debug)]
pub struct Caixa {
    pub no: Id,
    pub estilo: Rc<Estilo>,
    pub tipo: Tipo,
    pub pai: u32,
    pub filhos: Vec<u32>,
    // ---- resultado do layout (relativo a pagina, borda externa)
    pub x: Fixo,
    pub y: Fixo,
    pub l: Fixo,
    pub a: Fixo,
    pub margem: [Fixo; 4],
    pub borda: [Fixo; 4],
    pub preench: [Fixo; 4],
    /// Distancia do topo da borda ate a primeira linha de base.
    pub base: Option<Fixo>,
    /// Linhas (so em quem tem contexto inline).
    pub linhas: Vec<super::inline::Linha>,
    /// Filho de uma caixa inline que ficou num contexto inline (posicao
    /// calculada pelo dono das linhas).
    pub em_linha: bool,
}

impl Caixa {
    fn nova(no: Id, estilo: Rc<Estilo>, tipo: Tipo, pai: u32) -> Caixa {
        Caixa {
            no,
            estilo,
            tipo,
            pai,
            filhos: Vec::new(),
            x: 0,
            y: 0,
            l: 0,
            a: 0,
            margem: [0; 4],
            borda: [0; 4],
            preench: [0; 4],
            base: None,
            linhas: Vec::new(),
            em_linha: false,
        }
    }

    /// Caixas que entram numa linha de texto.
    pub fn eh_inline(&self) -> bool {
        match &self.tipo {
            Tipo::Inline | Tipo::InlineBloco | Tipo::Texto(_) | Tipo::Quebra | Tipo::Substituido(_) => {
                !(self.estilo.fora_do_fluxo() || self.estilo.flutuar != crate::web::css::estilo::Lado::Nenhum)
                    || matches!(self.tipo, Tipo::Texto(_))
            }
            Tipo::Flex | Tipo::Grid | Tipo::Tabela => self.estilo.exibir.eh_inline() && !self.estilo.fora_do_fluxo(),
            _ => false,
        }
    }

    /// Caixa atomica dentro da linha (nao se quebra).
    pub fn atomica(&self) -> bool {
        matches!(self.tipo, Tipo::InlineBloco | Tipo::Substituido(_))
            || (matches!(self.tipo, Tipo::Flex | Tipo::Grid | Tipo::Tabela) && self.estilo.exibir.eh_inline())
    }

    pub fn conteudo_x(&self) -> Fixo {
        self.x + self.borda[3] + self.preench[3]
    }
    pub fn conteudo_y(&self) -> Fixo {
        self.y + self.borda[0] + self.preench[0]
    }
    pub fn conteudo_l(&self) -> Fixo {
        (self.l - self.borda[1] - self.borda[3] - self.preench[1] - self.preench[3]).max(0)
    }
    pub fn conteudo_a(&self) -> Fixo {
        (self.a - self.borda[0] - self.borda[2] - self.preench[0] - self.preench[2]).max(0)
    }
}

pub struct Arvore {
    pub caixas: Vec<Caixa>,
}

/// Conteudo de texto de ::before/::after.
fn texto_conteudo(partes: &[ParteConteudo], doc: &Documento, dono: Id, contadores: &[(crate::web::atomo::Atomo, i32)]) -> String {
    let mut s = String::new();
    for p in partes {
        match p {
            ParteConteudo::Texto(t) => s.push_str(t),
            ParteConteudo::Atributo(n) => {
                if let Some(v) = doc.elemento(dono).and_then(|e| e.atributo(*n)) {
                    s.push_str(v);
                }
            }
            ParteConteudo::Contador(n, tipo) => {
                let v = contadores.iter().rev().find(|(x, _)| x == n).map_or(0, |c| c.1);
                s.push_str(&marcador_texto(*tipo, v).trim_end_matches(". "));
            }
            ParteConteudo::AbreAspas => s.push('\u{201C}'),
            ParteConteudo::FechaAspas => s.push('\u{201D}'),
        }
    }
    s
}

fn romano(mut n: i32, maiusculo: bool) -> String {
    if !(1..4000).contains(&n) {
        return alloc::format!("{n}");
    }
    let tabela = [(1000, "m"), (900, "cm"), (500, "d"), (400, "cd"), (100, "c"), (90, "xc"), (50, "l"), (40, "xl"), (10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i")];
    let mut s = String::new();
    for (v, t) in tabela {
        while n >= v {
            s.push_str(t);
            n -= v;
        }
    }
    if maiusculo { s.to_ascii_uppercase() } else { s }
}

/// Texto do marcador de um item de lista ("• ", "3. ", "iv. ").
pub fn marcador_texto(tipo: TipoLista, n: i32) -> String {
    match tipo {
        TipoLista::Nenhum => String::new(),
        TipoLista::Disco => "\u{2022} ".into(),
        TipoLista::Circulo => "\u{25E6} ".into(),
        TipoLista::Quadrado => "\u{25AA} ".into(),
        TipoLista::Decimal => alloc::format!("{n}. "),
        TipoLista::DecimalZero => alloc::format!("{n:02}. "),
        TipoLista::LetraMin | TipoLista::LetraMai => {
            let mut s = String::new();
            let mut k = n.max(1);
            while k > 0 {
                k -= 1;
                s.insert(0, (b'a' + (k % 26) as u8) as char);
                k /= 26;
            }
            if tipo == TipoLista::LetraMai {
                s = s.to_ascii_uppercase();
            }
            alloc::format!("{s}. ")
        }
        TipoLista::RomanoMin => alloc::format!("{}. ", romano(n, false)),
        TipoLista::RomanoMai => alloc::format!("{}. ", romano(n, true)),
    }
}

/// Trata os espacos de um texto conforme white-space e text-transform.
/// `espaco_antes`: o texto anterior (no mesmo contexto) terminou em espaco.
fn tratar_texto(t: &str, e: &Estilo, espaco_antes: &mut bool) -> String {
    let mut s = String::with_capacity(t.len());
    match e.brancos {
        Brancos::Pre | Brancos::PreQuebra => {
            for c in t.chars() {
                s.push(if c == '\t' { ' ' } else { c });
            }
            *espaco_antes = false;
        }
        Brancos::PreLinha => {
            // quebras ficam, espacos colapsam
            for c in t.chars() {
                if c == '\n' {
                    while s.ends_with(' ') {
                        s.pop();
                    }
                    s.push('\n');
                    *espaco_antes = true;
                } else if c.is_ascii_whitespace() {
                    if !*espaco_antes {
                        s.push(' ');
                        *espaco_antes = true;
                    }
                } else {
                    s.push(c);
                    *espaco_antes = false;
                }
            }
        }
        _ => {
            for c in t.chars() {
                if matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0C') {
                    if !*espaco_antes {
                        s.push(' ');
                        *espaco_antes = true;
                    }
                } else {
                    s.push(c);
                    *espaco_antes = false;
                }
            }
        }
    }
    match e.transformacao {
        Transformacao::Maiusculas => s = s.to_uppercase(),
        Transformacao::Minusculas => s = s.to_lowercase(),
        Transformacao::Capitalizar => {
            let mut r = String::with_capacity(s.len());
            let mut inicio = true;
            for c in s.chars() {
                if inicio && c.is_alphabetic() {
                    r.extend(c.to_uppercase());
                    inicio = false;
                } else {
                    if c.is_whitespace() {
                        inicio = true;
                    } else if c.is_alphanumeric() {
                        inicio = false;
                    }
                    r.push(c);
                }
            }
            s = r;
        }
        Transformacao::Nenhuma => {}
    }
    s
}

fn tipo_de(e: &Estilo) -> Tipo {
    match e.exibir {
        Exibir::Inline => Tipo::Inline,
        Exibir::InlineBloco => Tipo::InlineBloco,
        Exibir::Flex | Exibir::InlineFlex => Tipo::Flex,
        Exibir::Grid | Exibir::InlineGrid => Tipo::Grid,
        Exibir::Tabela | Exibir::InlineTabela => Tipo::Tabela,
        Exibir::LinhaTabela => Tipo::LinhaTabela,
        Exibir::CelulaTabela => Tipo::Celula,
        Exibir::GrupoLinhas | Exibir::GrupoCabecalho | Exibir::GrupoRodape => Tipo::GrupoLinhas,
        Exibir::Legenda => Tipo::Legenda,
        Exibir::GrupoColunas | Exibir::Coluna => Tipo::Bloco,
        _ => Tipo::Bloco,
    }
}

fn substituido(doc: &Documento, id: Id) -> Option<Substituido> {
    let e = doc.elemento(id)?;
    if e.espaco != crate::web::dom::Espaco::Html {
        return if e.nome == a::SVG { Some(Substituido::Svg) } else { None };
    }
    let n = e.nome;
    Some(if n == a::IMG {
        Substituido::Imagem { alt: e.atributo(a::ALT).unwrap_or("").into() }
    } else if n == a::INPUT {
        let tipo = e.atributo(a::TYPE).unwrap_or("text").to_ascii_lowercase();
        match tipo.as_str() {
            "hidden" => return None,
            "submit" | "button" | "reset" => Substituido::Entrada {
                texto: e.atributo(a::VALUE).map(String::from).unwrap_or_else(|| if tipo == "reset" { "Limpar".into() } else { "Enviar".into() }),
                senha: false,
                marcavel: false,
                marcado: false,
            },
            "checkbox" | "radio" => Substituido::Entrada { texto: String::new(), senha: false, marcavel: true, marcado: e.tem(a::CHECKED) },
            _ => {
                let v = e.atributo(a::VALUE).unwrap_or("");
                let texto = if v.is_empty() { e.atributo(a::PLACEHOLDER).unwrap_or("") } else { v };
                Substituido::Entrada { texto: texto.into(), senha: tipo == "password", marcavel: false, marcado: false }
            }
        }
    } else if n == a::SELECT {
        let primeira = doc
            .descendentes(id)
            .filter(|&o| doc.eh(o, a::OPTION))
            .find(|&o| doc.elemento(o).is_some_and(|x| x.tem(a::SELECTED)))
            .or_else(|| doc.descendentes(id).find(|&o| doc.eh(o, a::OPTION)));
        let texto = primeira.map(|o| doc.texto(o).trim().into()).unwrap_or_default();
        Substituido::Selecao { texto }
    } else if n == a::TEXTAREA {
        Substituido::AreaTexto { texto: doc.texto(id) }
    } else if n == a::IFRAME || n == a::EMBED || n == a::OBJECT {
        Substituido::Quadro
    } else if n == a::VIDEO {
        Substituido::Video
    } else if n == a::CANVAS {
        Substituido::Canvas
    } else if n == a::SVG {
        Substituido::Svg
    } else {
        return None;
    })
}

struct Construtor<'a> {
    doc: &'a Documento,
    est: &'a Estilos,
    caixas: Vec<Caixa>,
    contadores: Vec<(crate::web::atomo::Atomo, i32)>,
    /// Ultimo texto terminou em espaco (colapso entre caixas inline).
    espaco_antes: bool,
}

impl Construtor<'_> {
    fn nova(&mut self, no: Id, estilo: Rc<Estilo>, tipo: Tipo, pai: u32) -> u32 {
        let k = self.caixas.len() as u32;
        self.caixas.push(Caixa::nova(no, estilo, tipo, pai));
        if pai != u32::MAX {
            self.caixas[pai as usize].filhos.push(k);
        }
        k
    }

    fn texto(&mut self, pai: u32, t: &str, estilo: &Rc<Estilo>) {
        let s = tratar_texto(t, estilo, &mut self.espaco_antes);
        if s.is_empty() {
            return;
        }
        self.nova(NENHUM, estilo.clone(), Tipo::Texto(Rc::from(s.as_str())), pai);
    }

    /// Filhos (DOM) de `id` como caixas filhas de `pai`.
    fn filhos(&mut self, id: Id, pai: u32, estilo_pai: &Rc<Estilo>) {
        // ::before
        if let Some(pe) = self.est.antes.get(&id) {
            self.pseudo(id, pai, pe.clone());
        }
        let filhos: Vec<Id> = self.doc.filhos(id).collect();
        for f in filhos {
            match &self.doc.no(f).tipo {
                TipoNo::Texto(t) => {
                    let t = t.clone();
                    self.texto(pai, &t, estilo_pai);
                }
                TipoNo::Elemento(_) => self.elemento(f, pai),
                _ => {}
            }
        }
        if let Some(pe) = self.est.depois.get(&id) {
            self.pseudo(id, pai, pe.clone());
        }
    }

    fn pseudo(&mut self, dono: Id, pai: u32, e: Rc<Estilo>) {
        let Some(partes) = e.conteudo.clone() else { return };
        let t = texto_conteudo(&partes, self.doc, dono, &self.contadores);
        let tipo = tipo_de(&e);
        let k = self.nova(NENHUM, e.clone(), tipo, pai);
        if !t.is_empty() {
            self.texto(k, &t, &e);
        }
    }

    fn elemento(&mut self, id: Id, pai: u32) {
        let Some(e) = self.est.de(id).cloned() else { return };
        if e.exibir == Exibir::Nenhum {
            return;
        }
        // contadores (counter-reset / counter-increment / itens de lista)
        let marca_contadores = self.contadores.len();
        if let Some((n, v)) = e.zera_contador {
            self.contadores.push((n, v));
        }
        if let Some((n, v)) = e.incrementa_contador {
            match self.contadores.iter_mut().rev().find(|(x, _)| *x == n) {
                Some(c) => c.1 += v,
                None => self.contadores.push((n, v)),
            }
        }
        if self.doc.eh(id, a::OL) || self.doc.eh(id, a::UL) || self.doc.eh(id, a::MENU) {
            let inicio = self.doc.elemento(id).and_then(|el| el.atributo(a::START)).and_then(|s| s.trim().parse::<i32>().ok()).unwrap_or(1);
            self.contadores.push((crate::web::atomo::Atomo::de("list-item"), inicio - 1));
        }
        if e.exibir == Exibir::Conteudo {
            self.filhos(id, pai, &e);
            self.contadores.truncate(marca_contadores.max(self.contadores.len().min(marca_contadores)));
            return;
        }
        if self.doc.eh(id, a::BR) {
            self.nova(id, e, Tipo::Quebra, pai);
            self.espaco_antes = true;
            return;
        }
        if let Some(s) = substituido(self.doc, id) {
            let tipo = if e.exibir.eh_inline() || e.exibir == Exibir::Inline { Tipo::Substituido(s) } else { Tipo::Substituido(s) };
            self.nova(id, e, tipo, pai);
            self.espaco_antes = false;
            return;
        }
        let tipo = tipo_de(&e);
        let bloco = !matches!(tipo, Tipo::Inline);
        if bloco {
            self.espaco_antes = true; // espaco no comeco de bloco some
        }
        let k = self.nova(id, e.clone(), tipo, pai);
        // marcador de item de lista
        if e.exibir == Exibir::ItemLista {
            let nome = crate::web::atomo::Atomo::de("list-item");
            let n = match self.contadores.iter_mut().rev().find(|(x, _)| *x == nome) {
                Some(c) => {
                    let valor = self.doc.elemento(id).and_then(|el| el.atributo(a::VALUE)).and_then(|v| v.trim().parse::<i32>().ok());
                    c.1 = valor.unwrap_or(c.1 + 1);
                    c.1
                }
                None => 1,
            };
            let texto = marcador_texto(e.tipo_lista, n);
            if !texto.is_empty() && e.imagem_lista.is_none() {
                let mut em = Estilo::herdar(&e);
                if let Some(m) = self.est.marcador.get(&id) {
                    em = (**m).clone();
                }
                em.exibir = Exibir::Inline;
                let em = Rc::new(em);
                let mk = self.nova(NENHUM, em.clone(), Tipo::Inline, k);
                let s = Rc::from(texto.as_str());
                let t = self.nova(NENHUM, em, Tipo::Texto(s), mk);
                let _ = t;
                if !e.lista_dentro {
                    // marcador "de fora": o layout puxa para a esquerda
                    self.caixas[mk as usize].tipo = Tipo::Inline;
                    self.caixas[mk as usize].no = u32::MAX - 1; // marca de marcador externo
                }
            }
        }
        self.filhos(id, k, &e);
        if bloco {
            // depois de um inline-block o texto segue normal (o espaco conta)
            self.espaco_antes = !e.exibir.eh_inline();
        }
        if self.doc.eh(id, a::OL) || self.doc.eh(id, a::UL) || self.doc.eh(id, a::MENU) || e.zera_contador.is_some() {
            self.contadores.truncate(marca_contadores);
        }
    }
}

/// Marca usada no `no` da caixa do marcador externo de lista.
pub const MARCADOR_EXTERNO: Id = u32::MAX - 1;

/// Constroi a arvore de caixas do documento. A caixa 0 e a do elemento
/// raiz (<html>), dentro de um bloco "inicial" que e a tela.
pub fn construir(doc: &Documento, est: &Estilos) -> Arvore {
    let mut c = Construtor { doc, est, caixas: Vec::with_capacity(doc.nos.len()), contadores: Vec::new(), espaco_antes: true };
    let raiz_estilo = Rc::new(Estilo { exibir: Exibir::Bloco, ..Estilo::inicial() });
    let r = c.nova(NENHUM, raiz_estilo.clone(), Tipo::Bloco, u32::MAX);
    if let Some(html) = doc.html() {
        c.elemento(html, r);
    } else {
        c.filhos(RAIZ, r, &raiz_estilo);
    }
    let mut arv = Arvore { caixas: c.caixas };
    arv.consertar(0);
    arv
}

impl Arvore {
    /// Caixas anonimas: um bloco nao pode misturar filhos bloco e inline;
    /// partes de tabela soltas ganham a tabela que falta.
    fn consertar(&mut self, k: u32) {
        let filhos = self.caixas[k as usize].filhos.clone();
        for &f in &filhos {
            self.consertar(f);
        }
        let tipo = self.caixas[k as usize].tipo.clone();
        let eh_container_bloco = matches!(tipo, Tipo::Bloco | Tipo::InlineBloco | Tipo::Celula | Tipo::Legenda);
        if eh_container_bloco {
            let tem_bloco = filhos.iter().any(|&f| self.eh_nivel_bloco(f));
            let tem_inline = filhos.iter().any(|&f| self.caixas[f as usize].eh_inline() && !self.so_espaco(f));
            if tem_bloco && tem_inline {
                self.embrulhar_inline(k);
            } else if tem_bloco {
                // sem inline de verdade: textos so de espaco somem
                let lista: Vec<u32> = self.caixas[k as usize].filhos.iter().copied().filter(|&f| !self.so_espaco(f)).collect();
                self.caixas[k as usize].filhos = lista;
            }
        }
        if matches!(tipo, Tipo::Flex | Tipo::Grid) {
            // cada filho em fluxo vira item; texto solto vira bloco anonimo
            self.embrulhar_texto_em_itens(k);
        }
        if matches!(tipo, Tipo::Tabela | Tipo::GrupoLinhas | Tipo::LinhaTabela) {
            self.consertar_tabela(k);
        } else {
            // celulas/linhas soltas fora de tabela: viram bloco simples
            for &f in &self.caixas[k as usize].filhos.clone() {
                if matches!(self.caixas[f as usize].tipo, Tipo::LinhaTabela | Tipo::GrupoLinhas) {
                    self.caixas[f as usize].tipo = Tipo::Bloco;
                } else if matches!(self.caixas[f as usize].tipo, Tipo::Celula) && !matches!(tipo, Tipo::LinhaTabela) {
                    self.caixas[f as usize].tipo = Tipo::InlineBloco;
                }
            }
        }
    }

    fn eh_nivel_bloco(&self, f: u32) -> bool {
        let c = &self.caixas[f as usize];
        !c.eh_inline() && !c.estilo.fora_do_fluxo() && c.estilo.flutuar == crate::web::css::estilo::Lado::Nenhum
    }

    fn so_espaco(&self, f: u32) -> bool {
        match &self.caixas[f as usize].tipo {
            Tipo::Texto(t) => t.chars().all(|c| c == ' ') && !self.caixas[f as usize].estilo.brancos.mantem_espacos(),
            _ => false,
        }
    }

    fn embrulhar_inline(&mut self, k: u32) {
        let filhos = core::mem::take(&mut self.caixas[k as usize].filhos);
        let estilo_anon = Rc::new(Estilo { exibir: Exibir::Bloco, ..Estilo::herdar(&self.caixas[k as usize].estilo) });
        let mut novos = Vec::new();
        let mut atual: Option<u32> = None;
        for f in filhos {
            let fora = {
                let c = &self.caixas[f as usize];
                c.estilo.fora_do_fluxo() || c.estilo.flutuar != crate::web::css::estilo::Lado::Nenhum
            };
            if self.caixas[f as usize].eh_inline() || (fora && atual.is_some()) {
                if atual.is_none() && self.so_espaco(f) {
                    continue;
                }
                let anon = *atual.get_or_insert_with(|| {
                    let n = self.caixas.len() as u32;
                    self.caixas.push(Caixa::nova(NENHUM, estilo_anon.clone(), Tipo::Bloco, k));
                    novos.push(n);
                    n
                });
                self.caixas[f as usize].pai = anon;
                self.caixas[anon as usize].filhos.push(f);
            } else {
                atual = None;
                novos.push(f);
            }
        }
        // anonimos so de espaco somem
        novos.retain(|&n| {
            let c = &self.caixas[n as usize];
            !(c.no == NENHUM && matches!(c.tipo, Tipo::Bloco) && !c.filhos.is_empty() && c.filhos.iter().all(|&f| self.so_espaco(f)))
        });
        self.caixas[k as usize].filhos = novos;
    }

    fn embrulhar_texto_em_itens(&mut self, k: u32) {
        let filhos = core::mem::take(&mut self.caixas[k as usize].filhos);
        let estilo_anon = Rc::new(Estilo { exibir: Exibir::Bloco, ..Estilo::herdar(&self.caixas[k as usize].estilo) });
        let mut novos = Vec::new();
        let mut atual: Option<u32> = None;
        for f in filhos {
            let c = &self.caixas[f as usize];
            let solto = matches!(c.tipo, Tipo::Texto(_) | Tipo::Quebra) || (matches!(c.tipo, Tipo::Inline) && false);
            if solto {
                if self.so_espaco(f) {
                    continue;
                }
                let anon = *atual.get_or_insert_with(|| {
                    let n = self.caixas.len() as u32;
                    self.caixas.push(Caixa::nova(NENHUM, estilo_anon.clone(), Tipo::Bloco, k));
                    novos.push(n);
                    n
                });
                self.caixas[f as usize].pai = anon;
                self.caixas[anon as usize].filhos.push(f);
            } else {
                atual = None;
                // inline vira bloco quando e item de flex/grid
                if matches!(self.caixas[f as usize].tipo, Tipo::Inline) {
                    self.caixas[f as usize].tipo = Tipo::Bloco;
                    self.caixas[f as usize].estilo = Rc::new(Estilo { exibir: Exibir::Bloco, ..(*self.caixas[f as usize].estilo).clone() });
                    self.consertar(f);
                }
                novos.push(f);
            }
        }
        self.caixas[k as usize].filhos = novos;
    }

    fn consertar_tabela(&mut self, k: u32) {
        let tipo = self.caixas[k as usize].tipo.clone();
        let filhos = core::mem::take(&mut self.caixas[k as usize].filhos);
        let estilo = self.caixas[k as usize].estilo.clone();
        let mut novos = Vec::new();
        let mut anon: Option<u32> = None;
        // o que cada nivel aceita direto
        let aceita = |t: &Tipo| match tipo {
            Tipo::Tabela => matches!(t, Tipo::GrupoLinhas | Tipo::Legenda),
            Tipo::GrupoLinhas => matches!(t, Tipo::LinhaTabela),
            _ => matches!(t, Tipo::Celula),
        };
        let filho_tipo = match tipo {
            Tipo::Tabela => Tipo::GrupoLinhas,
            Tipo::GrupoLinhas => Tipo::LinhaTabela,
            _ => Tipo::Celula,
        };
        for f in filhos {
            let tf = self.caixas[f as usize].tipo.clone();
            if aceita(&tf) {
                anon = None;
                novos.push(f);
                continue;
            }
            if self.so_espaco(f) {
                continue;
            }
            let a_k = *anon.get_or_insert_with(|| {
                let n = self.caixas.len() as u32;
                let ex = match filho_tipo {
                    Tipo::GrupoLinhas => Exibir::GrupoLinhas,
                    Tipo::LinhaTabela => Exibir::LinhaTabela,
                    _ => Exibir::CelulaTabela,
                };
                let est = Rc::new(Estilo { exibir: ex, ..Estilo::herdar(&estilo) });
                self.caixas.push(Caixa::nova(NENHUM, est, filho_tipo.clone(), k));
                novos.push(n);
                n
            });
            self.caixas[f as usize].pai = a_k;
            self.caixas[a_k as usize].filhos.push(f);
        }
        self.caixas[k as usize].filhos = novos.clone();
        for n in novos {
            if self.caixas[n as usize].no == NENHUM && matches!(self.caixas[n as usize].tipo, Tipo::GrupoLinhas | Tipo::LinhaTabela) {
                self.consertar_tabela(n);
            }
            if self.caixas[n as usize].no == NENHUM && matches!(self.caixas[n as usize].tipo, Tipo::Celula) {
                let filhos = self.caixas[n as usize].filhos.clone();
                let tem_bloco = filhos.iter().any(|&f| self.eh_nivel_bloco(f));
                let tem_inline = filhos.iter().any(|&f| self.caixas[f as usize].eh_inline());
                if tem_bloco && tem_inline {
                    self.embrulhar_inline(n);
                }
            }
        }
    }
}
