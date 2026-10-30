//! A arvore do documento (DOM) numa arena: todos os nos num Vec e as
//! ligacoes (pai, filhos, irmaos) como indices u32. Sem Rc, sem ponteiros:
//! percorrer a arvore e andar num vetor contiguo (bom para o cache do
//! Cortex-A7), e o documento inteiro sai da memoria de uma vez.

use alloc::string::String;
use alloc::vec::Vec;

use super::atomo::{Atomo, a};

pub type Id = u32;
pub const NENHUM: Id = u32::MAX;
/// O no do documento e sempre o 0.
pub const RAIZ: Id = 0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Espaco {
    Html,
    Svg,
    MathMl,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Atributo {
    pub nome: Atomo,
    pub valor: String,
}

#[derive(Clone, Debug)]
pub struct Elemento {
    pub nome: Atomo,
    pub espaco: Espaco,
    pub atributos: Vec<Atributo>,
    /// Classes ja separadas (o seletor ".x" so compara atomos).
    pub classes: Vec<Atomo>,
    pub id: Option<Atomo>,
}

#[derive(Clone, Debug)]
pub enum Tipo {
    Documento,
    Doctype(String),
    Elemento(Elemento),
    Texto(String),
    Comentario(String),
}

#[derive(Clone, Debug)]
pub struct No {
    pub tipo: Tipo,
    pub pai: Id,
    pub primeiro: Id,
    pub ultimo: Id,
    pub anterior: Id,
    pub proximo: Id,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Modo {
    Normal,
    Quirks,
    QuaseNormal,
}

pub struct Documento {
    pub nos: Vec<No>,
    pub modo: Modo,
}

impl Default for Documento {
    fn default() -> Self {
        Self::novo()
    }
}

impl Elemento {
    pub fn novo(nome: Atomo, espaco: Espaco, atributos: Vec<Atributo>) -> Elemento {
        let mut e = Elemento { nome, espaco, atributos, classes: Vec::new(), id: None };
        e.atualizar_cache();
        e
    }

    /// Recalcula classes e id depois de mudar atributos.
    pub fn atualizar_cache(&mut self) {
        self.classes.clear();
        self.id = None;
        for at in &self.atributos {
            if at.nome == a::CLASS {
                self.classes.extend(at.valor.split_ascii_whitespace().map(Atomo::de));
            } else if at.nome == a::ID && !at.valor.is_empty() {
                self.id = Some(Atomo::de(&at.valor));
            }
        }
    }

    pub fn atributo(&self, nome: Atomo) -> Option<&str> {
        self.atributos.iter().find(|x| x.nome == nome).map(|x| x.valor.as_str())
    }

    pub fn tem(&self, nome: Atomo) -> bool {
        self.atributos.iter().any(|x| x.nome == nome)
    }

    pub fn eh(&self, nome: Atomo) -> bool {
        self.nome == nome && self.espaco == Espaco::Html
    }

    pub fn definir(&mut self, nome: Atomo, valor: &str) {
        match self.atributos.iter_mut().find(|x| x.nome == nome) {
            Some(x) => x.valor = String::from(valor),
            None => self.atributos.push(Atributo { nome, valor: String::from(valor) }),
        }
        if nome == a::CLASS || nome == a::ID {
            self.atualizar_cache();
        }
    }

    pub fn tirar(&mut self, nome: Atomo) {
        self.atributos.retain(|x| x.nome != nome);
        if nome == a::CLASS || nome == a::ID {
            self.atualizar_cache();
        }
    }
}

impl Documento {
    pub fn novo() -> Documento {
        let raiz = No { tipo: Tipo::Documento, pai: NENHUM, primeiro: NENHUM, ultimo: NENHUM, anterior: NENHUM, proximo: NENHUM };
        let mut nos = Vec::with_capacity(1024);
        nos.push(raiz);
        Documento { nos, modo: Modo::Normal }
    }

    pub fn criar(&mut self, tipo: Tipo) -> Id {
        let id = self.nos.len() as Id;
        self.nos.push(No { tipo, pai: NENHUM, primeiro: NENHUM, ultimo: NENHUM, anterior: NENHUM, proximo: NENHUM });
        id
    }

    pub fn criar_elemento(&mut self, nome: Atomo, espaco: Espaco, atributos: Vec<Atributo>) -> Id {
        self.criar(Tipo::Elemento(Elemento::novo(nome, espaco, atributos)))
    }

    pub fn no(&self, id: Id) -> &No {
        &self.nos[id as usize]
    }

    pub fn elemento(&self, id: Id) -> Option<&Elemento> {
        match &self.nos.get(id as usize)?.tipo {
            Tipo::Elemento(e) => Some(e),
            _ => None,
        }
    }

    pub fn elemento_mut(&mut self, id: Id) -> Option<&mut Elemento> {
        match &mut self.nos.get_mut(id as usize)?.tipo {
            Tipo::Elemento(e) => Some(e),
            _ => None,
        }
    }

    /// Nome (atomo) se for elemento HTML; NENHUM nos outros casos.
    pub fn nome(&self, id: Id) -> Atomo {
        match self.elemento(id) {
            Some(e) if e.espaco == Espaco::Html => e.nome,
            _ => a::VAZIO,
        }
    }

    pub fn eh(&self, id: Id, nome: Atomo) -> bool {
        self.elemento(id).is_some_and(|e| e.eh(nome))
    }

    pub fn pai(&self, id: Id) -> Id {
        self.nos[id as usize].pai
    }

    /// Tira o no de onde ele esta (continua na arena, solto).
    pub fn soltar(&mut self, id: Id) {
        let (pai, ant, prox) = {
            let n = &self.nos[id as usize];
            (n.pai, n.anterior, n.proximo)
        };
        if pai == NENHUM {
            return;
        }
        if ant != NENHUM {
            self.nos[ant as usize].proximo = prox;
        } else {
            self.nos[pai as usize].primeiro = prox;
        }
        if prox != NENHUM {
            self.nos[prox as usize].anterior = ant;
        } else {
            self.nos[pai as usize].ultimo = ant;
        }
        let n = &mut self.nos[id as usize];
        n.pai = NENHUM;
        n.anterior = NENHUM;
        n.proximo = NENHUM;
    }

    /// `filho` e `pai` ou um ancestral dele? (por o no dentro de si mesmo
    /// faria um ciclo na arvore: o DOM recusa com HierarchyRequestError)
    fn contem(&self, filho: Id, pai: Id) -> bool {
        let mut x = pai;
        while x != NENHUM {
            if x == filho {
                return true;
            }
            x = self.nos[x as usize].pai;
        }
        false
    }

    /// Poe `filho` no fim de `pai`.
    pub fn anexar(&mut self, pai: Id, filho: Id) {
        if self.contem(filho, pai) {
            return;
        }
        self.soltar(filho);
        let ultimo = self.nos[pai as usize].ultimo;
        {
            let f = &mut self.nos[filho as usize];
            f.pai = pai;
            f.anterior = ultimo;
            f.proximo = NENHUM;
        }
        if ultimo != NENHUM {
            self.nos[ultimo as usize].proximo = filho;
        } else {
            self.nos[pai as usize].primeiro = filho;
        }
        self.nos[pai as usize].ultimo = filho;
    }

    /// Poe `filho` antes de `referencia` (NENHUM = no fim).
    pub fn inserir_antes(&mut self, pai: Id, filho: Id, mut referencia: Id) {
        // antes de si mesmo = antes do proximo (como no DOM). Sem isso o
        // B.prepend(p) do example.com, com p ja primeiro filho, ligava p a
        // ele mesmo e quem descia a arvore rodava para sempre
        if referencia == filho {
            referencia = self.nos[filho as usize].proximo;
        }
        if referencia == NENHUM {
            return self.anexar(pai, filho);
        }
        // referencia de outro pai (NotFoundError no DOM) ou ciclo: recusa
        if self.nos[referencia as usize].pai != pai || self.contem(filho, pai) {
            return;
        }
        self.soltar(filho);
        let ant = self.nos[referencia as usize].anterior;
        {
            let f = &mut self.nos[filho as usize];
            f.pai = pai;
            f.anterior = ant;
            f.proximo = referencia;
        }
        self.nos[referencia as usize].anterior = filho;
        if ant != NENHUM {
            self.nos[ant as usize].proximo = filho;
        } else {
            self.nos[pai as usize].primeiro = filho;
        }
    }

    /// Texto no fim de `pai` (junta com o texto que ja estiver la).
    pub fn anexar_texto(&mut self, pai: Id, texto: &str) {
        let ultimo = self.nos[pai as usize].ultimo;
        if ultimo != NENHUM {
            if let Tipo::Texto(t) = &mut self.nos[ultimo as usize].tipo {
                t.push_str(texto);
                return;
            }
        }
        let id = self.criar(Tipo::Texto(String::from(texto)));
        self.anexar(pai, id);
    }

    /// Texto antes de `referencia` (juntando com o texto anterior).
    pub fn inserir_texto_antes(&mut self, pai: Id, texto: &str, referencia: Id) {
        if referencia == NENHUM {
            return self.anexar_texto(pai, texto);
        }
        let ant = self.nos[referencia as usize].anterior;
        if ant != NENHUM {
            if let Tipo::Texto(t) = &mut self.nos[ant as usize].tipo {
                t.push_str(texto);
                return;
            }
        }
        let id = self.criar(Tipo::Texto(String::from(texto)));
        self.inserir_antes(pai, id, referencia);
    }

    /// Os filhos de um no, em ordem.
    pub fn filhos(&self, id: Id) -> Filhos<'_> {
        Filhos { doc: self, atual: self.nos[id as usize].primeiro }
    }

    /// Todos os descendentes em ordem de documento (pre-ordem), sem o
    /// proprio no.
    pub fn descendentes(&self, id: Id) -> Descendentes<'_> {
        Descendentes { doc: self, raiz: id, atual: self.nos[id as usize].primeiro }
    }

    /// O texto de todos os descendentes (textContent).
    pub fn texto(&self, id: Id) -> String {
        let mut s = String::new();
        if let Tipo::Texto(t) = &self.nos[id as usize].tipo {
            s.push_str(t);
        }
        for d in self.descendentes(id) {
            if let Tipo::Texto(t) = &self.nos[d as usize].tipo {
                s.push_str(t);
            }
        }
        s
    }

    /// Primeiro descendente elemento com esse nome.
    pub fn achar(&self, id: Id, nome: Atomo) -> Option<Id> {
        self.descendentes(id).find(|&d| self.eh(d, nome))
    }

    pub fn por_id(&self, id: &str) -> Option<Id> {
        let alvo = Atomo::se_existe(id)?;
        self.descendentes(RAIZ).find(|&d| self.elemento(d).is_some_and(|e| e.id == Some(alvo)))
    }

    pub fn html(&self) -> Option<Id> {
        self.filhos(RAIZ).find(|&f| self.eh(f, a::HTML))
    }

    pub fn corpo(&self) -> Option<Id> {
        let html = self.html()?;
        self.filhos(html).find(|&f| self.eh(f, a::BODY) || self.eh(f, a::FRAMESET))
    }

    pub fn cabeca(&self) -> Option<Id> {
        let html = self.html()?;
        self.filhos(html).find(|&f| self.eh(f, a::HEAD))
    }

    pub fn titulo(&self) -> String {
        match self.cabeca().and_then(|h| self.achar(h, a::TITLE)).or_else(|| self.achar(RAIZ, a::TITLE)) {
            Some(t) => {
                let mut s = String::new();
                for p in self.texto(t).split_ascii_whitespace() {
                    if !s.is_empty() {
                        s.push(' ');
                    }
                    s.push_str(p);
                }
                s
            }
            None => String::new(),
        }
    }
}

pub struct Filhos<'a> {
    doc: &'a Documento,
    atual: Id,
}

impl Iterator for Filhos<'_> {
    type Item = Id;
    fn next(&mut self) -> Option<Id> {
        if self.atual == NENHUM {
            return None;
        }
        let id = self.atual;
        self.atual = self.doc.nos[id as usize].proximo;
        Some(id)
    }
}

pub struct Descendentes<'a> {
    doc: &'a Documento,
    raiz: Id,
    atual: Id,
}

impl Iterator for Descendentes<'_> {
    type Item = Id;
    fn next(&mut self) -> Option<Id> {
        if self.atual == NENHUM {
            return None;
        }
        let id = self.atual;
        let n = &self.doc.nos[id as usize];
        self.atual = if n.primeiro != NENHUM {
            n.primeiro
        } else {
            let mut x = id;
            loop {
                if x == self.raiz {
                    break NENHUM;
                }
                let nx = &self.doc.nos[x as usize];
                if nx.proximo != NENHUM {
                    break nx.proximo;
                }
                x = nx.pai;
                if x == NENHUM || x == self.raiz {
                    break NENHUM;
                }
            }
        };
        Some(id)
    }
}

// ------------------------------------------------- serializar e copiar

fn escapar(s: &str, atributo: bool, saida: &mut String) {
    for c in s.chars() {
        match c {
            '&' => saida.push_str("&amp;"),
            '\u{A0}' => saida.push_str("&nbsp;"),
            '"' if atributo => saida.push_str("&quot;"),
            '<' if !atributo => saida.push_str("&lt;"),
            '>' if !atributo => saida.push_str("&gt;"),
            c => saida.push(c),
        }
    }
}

const VAZIOS: &[&str] = &["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr"];
const CRUS: &[&str] = &["script", "style", "xmp", "iframe", "noembed", "noframes", "plaintext", "noscript"];

impl Documento {
    /// HTML de um no (outerHTML) ou so dos filhos (innerHTML).
    pub fn serializar(&self, id: Id, proprio: bool, saida: &mut String) {
        match &self.no(id).tipo {
            Tipo::Texto(t) => {
                let pai_cru = self.elemento(self.pai(id)).is_some_and(|e| CRUS.contains(&e.nome.texto()));
                if pai_cru { saida.push_str(t) } else { escapar(t, false, saida) }
            }
            Tipo::Comentario(c) => {
                saida.push_str("<!--");
                saida.push_str(c);
                saida.push_str("-->");
            }
            Tipo::Doctype(n) => {
                saida.push_str("<!DOCTYPE ");
                saida.push_str(n);
                saida.push('>');
            }
            Tipo::Documento => {
                for f in self.filhos(id) {
                    self.serializar(f, true, saida);
                }
            }
            Tipo::Elemento(e) => {
                let nome = e.nome.texto();
                if proprio {
                    saida.push('<');
                    saida.push_str(nome);
                    for at in &e.atributos {
                        saida.push(' ');
                        saida.push_str(at.nome.texto());
                        saida.push_str("=\"");
                        escapar(&at.valor, true, saida);
                        saida.push('"');
                    }
                    saida.push('>');
                    if VAZIOS.contains(&nome) {
                        return;
                    }
                }
                for f in self.filhos(id) {
                    self.serializar(f, true, saida);
                }
                if proprio {
                    saida.push_str("</");
                    saida.push_str(nome);
                    saida.push('>');
                }
            }
        }
    }

    /// Copia um no (e os filhos, se `profundo`) para um no solto novo.
    pub fn clonar(&mut self, id: Id, profundo: bool) -> Id {
        let tipo = self.no(id).tipo.clone();
        let novo = self.criar(tipo);
        if profundo {
            let filhos: Vec<Id> = self.filhos(id).collect();
            for f in filhos {
                let c = self.clonar(f, true);
                self.anexar(novo, c);
            }
        }
        novo
    }

    /// Traz para ca um no de outro documento (copia).
    pub fn importar(&mut self, outro: &Documento, id: Id) -> Id {
        let novo = self.criar(outro.no(id).tipo.clone());
        let filhos: Vec<Id> = outro.filhos(id).collect();
        for f in filhos {
            let c = self.importar(outro, f);
            self.anexar(novo, c);
        }
        novo
    }

    /// Le um pedaco de HTML (contexto: dentro do body) e devolve os nos
    /// soltos, prontos para anexar.
    pub fn fragmento(&mut self, html: &str) -> Vec<Id> {
        let outro = super::html::analisar(&alloc::format!("<!DOCTYPE html><body>{html}"));
        let mut v = Vec::new();
        // <head> pode ter ganho <style>/<meta> do fragmento; vao junto
        if let Some(h) = outro.cabeca() {
            for f in outro.filhos(h).collect::<Vec<_>>() {
                v.push(self.importar(&outro, f));
            }
        }
        if let Some(b) = outro.corpo() {
            for f in outro.filhos(b).collect::<Vec<_>>() {
                v.push(self.importar(&outro, f));
            }
        }
        v
    }

    /// O no esta dentro de `ancestral` (ou e ele)?
    pub fn dentro_de(&self, mut id: Id, ancestral: Id) -> bool {
        while id != NENHUM {
            if id == ancestral {
                return true;
            }
            id = self.pai(id);
        }
        false
    }
}
