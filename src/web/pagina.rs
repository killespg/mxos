//! Uma pagina pronta: documento, estilos, caixas com layout. Junta as
//! etapas do motor (HTML -> CSS -> caixas -> layout) e pinta. O documento
//! fica em Rc<RefCell> porque o JavaScript (web::script) mexe nele; depois
//! de uma mudanca, `refazer` recalcula estilo e layout.

use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::web::css::cascata::{self, Estilos, Indice};
use crate::web::css::folha::{Folha, Midia};
use crate::web::css::seletor::Estado;
use crate::web::css::valores::{Fixo, UM};
use crate::web::dom::{Documento, Id};
use crate::web::fonte::Fontes;
use crate::web::layout::caixa::{self, Arvore};
use crate::web::layout::{Contexto, Motor};
use crate::web::pintura::desenho::{Imagens, Pintor};
use crate::web::pintura::quadro::Quadro;

pub struct Pagina {
    pub doc: Rc<RefCell<Documento>>,
    pub indice: Indice,
    pub estilos: Estilos,
    pub arvore: Arvore,
    pub largura: i32,
    pub altura_tela: i32,
    /// Altura total do conteudo (px).
    pub altura: i32,
    pub titulo: String,
    pub cor_fundo: u32,
}

impl Pagina {
    /// Monta a pagina: estilos, caixas e layout para a largura dada (px).
    pub fn montar(
        doc: Documento,
        folhas: Vec<Folha>,
        largura: i32,
        altura_tela: i32,
        fontes: &Fontes,
        tamanhos: &dyn Fn(Id) -> Option<(i32, i32)>,
    ) -> Pagina {
        Self::montar_rc(Rc::new(RefCell::new(doc)), folhas, largura, altura_tela, fontes, tamanhos)
    }

    pub fn montar_rc(
        doc: Rc<RefCell<Documento>>,
        folhas: Vec<Folha>,
        largura: i32,
        altura_tela: i32,
        fontes: &Fontes,
        tamanhos: &dyn Fn(Id) -> Option<(i32, i32)>,
    ) -> Pagina {
        let m = Midia { largura: largura as f32, altura: altura_tela as f32 };
        let indice = Indice::novo(folhas, &m);
        let (estilos, arvore) = calcular(&doc.borrow(), &indice, largura, altura_tela, fontes, tamanhos);
        let mut p = Pagina { doc, indice, estilos, arvore, largura, altura_tela, altura: 0, titulo: String::new(), cor_fundo: 0xFFFFFF };
        p.resumir();
        p
    }

    /// Recalcula estilos, caixas e layout (depois de o DOM mudar).
    pub fn refazer(&mut self, fontes: &Fontes, tamanhos: &dyn Fn(Id) -> Option<(i32, i32)>) {
        let (estilos, arvore) = calcular(&self.doc.borrow(), &self.indice, self.largura, self.altura_tela, fontes, tamanhos);
        self.estilos = estilos;
        self.arvore = arvore;
        self.resumir();
    }

    /// Altura, titulo e cor de fundo a partir do layout.
    fn resumir(&mut self) {
        let doc = self.doc.borrow();
        self.altura = self.arvore.caixas.iter().map(|c| c.y + c.a).max().unwrap_or(0).div_euclid(UM);
        self.titulo = doc.titulo();
        // cor de fundo da tela: a do <html>, senao a do <body> (CSS 2.1 14.2)
        let fundo_de = |id: Option<Id>| id.and_then(|i| self.estilos.de(i)).map(|e| e.fundo).filter(|c| c.visivel());
        self.cor_fundo = fundo_de(doc.html()).or_else(|| fundo_de(doc.corpo())).map_or(0xFFFFFF, |c| c.0 & 0xFF_FFFF);
    }

    /// Pinta a parte visivel (rolagem em px) no quadro.
    pub fn pintar(&self, fontes: &mut Fontes, q: &mut Quadro, rolagem: i32, imagens: &dyn Imagens) {
        q.px.fill(self.cor_fundo);
        let mut p = Pintor { arv: &self.arvore, fontes, imagens, q, rolagem: rolagem as Fixo * UM };
        p.pintar();
    }

    /// O <a href> no ponto da tela (x, y) com a rolagem dada.
    pub fn link_em(&self, x: i32, y: i32, rolagem: i32) -> Option<Id> {
        crate::web::pintura::desenho::link_em(&self.arvore, &self.doc.borrow(), x * UM, (y + rolagem) * UM)
    }

    /// O elemento mais fundo sob o ponto (para eventos de clique).
    pub fn no_em(&self, x: i32, y: i32, rolagem: i32) -> Option<Id> {
        let (x, y) = (x * UM, (y + rolagem) * UM);
        let doc = self.doc.borrow();
        let mut achado = None;
        for c in &self.arvore.caixas {
            if doc.elemento(c.no).is_none() {
                continue;
            }
            let (bx, by, bl, ba) = caixa_de_borda(c);
            if x >= bx && x < bx + bl && y >= by && y < by + ba {
                // caixas vem em ordem de documento: a ultima que contem e a mais funda
                achado = Some(c.no);
            }
        }
        achado
    }

    /// Retangulo de borda (px) de cada elemento com caixa: para
    /// getBoundingClientRect, offsetWidth...
    pub fn geometria(&self) -> BTreeMap<Id, (f32, f32, f32, f32)> {
        let mut g = BTreeMap::new();
        for c in &self.arvore.caixas {
            let (x, y, l, a) = caixa_de_borda(c);
            let um = UM as f32;
            g.entry(c.no).or_insert((x as f32 / um, y as f32 / um, l as f32 / um, a as f32 / um));
        }
        g
    }
}

/// (x, y, largura, altura) da borda de uma caixa, em Fixo.
fn caixa_de_borda(c: &caixa::Caixa) -> (Fixo, Fixo, Fixo, Fixo) {
    (c.x, c.y, c.l, c.a)
}

fn calcular(
    doc: &Documento,
    indice: &Indice,
    largura: i32,
    altura_tela: i32,
    fontes: &Fontes,
    tamanhos: &dyn Fn(Id) -> Option<(i32, i32)>,
) -> (Estilos, Arvore) {
    let m = Midia { largura: largura as f32, altura: altura_tela as f32 };
    let estilos = cascata::calcular(doc, indice, &m, Estado::default());
    let mut arvore = caixa::construir(doc, &estilos);
    let ctx = Contexto { fontes, tela_l: largura * UM, tela_a: altura_tela * UM, imagens: tamanhos, doc: Some(doc) };
    Motor::novo(&mut arvore, &ctx).fazer();
    (estilos, arvore)
}
