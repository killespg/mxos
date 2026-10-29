//! Uma pagina pronta: documento, estilos, caixas com layout. Junta as
//! etapas do motor (HTML -> CSS -> caixas -> layout) e pinta.

use alloc::string::String;
use alloc::vec::Vec;

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
    pub doc: Documento,
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
        let m = Midia { largura: largura as f32, altura: altura_tela as f32 };
        let ind = Indice::novo(folhas, &m);
        let estilos = cascata::calcular(&doc, &ind, &m, Estado::default());
        let mut arvore = caixa::construir(&doc, &estilos);
        let ctx = Contexto { fontes, tela_l: largura * UM, tela_a: altura_tela * UM, imagens: tamanhos, doc: Some(&doc) };
        {
            let mut motor = Motor::novo(&mut arvore, &ctx);
            motor.fazer();
        }
        let altura = arvore.caixas.iter().map(|c| c.y + c.a).max().unwrap_or(0).div_euclid(UM);
        let titulo = doc.titulo();
        // cor de fundo da tela: a do <html>, senao a do <body> (CSS 2.1 14.2)
        let fundo_de = |id: Option<Id>| id.and_then(|i| estilos.de(i)).map(|e| e.fundo).filter(|c| c.visivel());
        let cor_fundo = fundo_de(doc.html()).or_else(|| fundo_de(doc.corpo())).map_or(0xFFFFFF, |c| c.0 & 0xFF_FFFF);
        Pagina { doc, estilos, arvore, largura, altura_tela, altura, titulo, cor_fundo }
    }

    /// Pinta a parte visivel (rolagem em px) no quadro.
    pub fn pintar(&self, fontes: &mut Fontes, q: &mut Quadro, rolagem: i32, imagens: &dyn Imagens) {
        q.px.fill(self.cor_fundo);
        let mut p = Pintor { arv: &self.arvore, fontes, imagens, q, rolagem: rolagem as Fixo * UM };
        p.pintar();
    }

    /// O <a href> no ponto da tela (x, y) com a rolagem dada.
    pub fn link_em(&self, x: i32, y: i32, rolagem: i32) -> Option<Id> {
        crate::web::pintura::desenho::link_em(&self.arvore, &self.doc, x * UM, (y + rolagem) * UM)
    }
}
