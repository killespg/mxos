//! Fontes: as faces carregadas (TrueType), a escolha por familia/peso/
//! italico (com negrito e italico sinteticos quando falta a face), a
//! medida de texto (com troca de face por caractere quando a fonte nao
//! tem o glifo) e o cache de glifos ja desenhados.

pub mod raster;
pub mod ttf;

#[allow(unused_imports)]
use crate::web::mat::F32Ext;
use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::vec::Vec;

use crate::web::css::estilo::Familia;
use crate::web::css::valores::{Fixo, fixo};
use raster::Bitmap;
use ttf::Fonte;

pub struct Face {
    pub fonte: Fonte,
    pub familia: Familia,
    pub negrito: bool,
    pub italico: bool,
}

/// Uma fonte escolhida para um pedaco de texto.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Escolha {
    pub face: u16,
    pub tamanho: f32,
    pub negrito_sintetico: bool,
    pub italico_sintetico: bool,
}

#[derive(Default)]
pub struct Fontes {
    pub faces: Vec<Face>,
    cache: BTreeMap<(u16, u16, u32, u8, u8), Rc<Bitmap>>,
}

/// Um glifo posicionado de um texto.
#[derive(Clone, Copy, Debug)]
pub struct Glifo {
    pub face: u16,
    pub id: u16,
    /// Posicao x (ponto fixo) a partir do inicio do texto.
    pub x: Fixo,
}

impl Fontes {
    pub fn novo() -> Fontes {
        Fontes::default()
    }

    pub fn adicionar(&mut self, dados: Vec<u8>, familia: Familia, negrito: bool, italico: bool) -> bool {
        match Fonte::ler(dados) {
            Some(fonte) => {
                self.faces.push(Face { fonte, familia, negrito, italico });
                true
            }
            None => false,
        }
    }

    pub fn escolher(&self, familia: Familia, peso: u16, italico: bool, tamanho: f32) -> Escolha {
        let negrito = peso >= 600;
        let mut melhor = 0usize;
        let mut nota = -1i32;
        for (k, f) in self.faces.iter().enumerate() {
            let mut n = 0;
            if f.familia == familia {
                n += 8;
            } else if familia == Familia::Serif && f.familia == Familia::SemSerif {
                n += 2;
            }
            if f.negrito == negrito {
                n += 4;
            }
            if f.italico == italico {
                n += 2;
            }
            if n > nota {
                nota = n;
                melhor = k;
            }
        }
        let f = self.faces.get(melhor);
        Escolha {
            face: melhor as u16,
            tamanho,
            negrito_sintetico: negrito && f.is_some_and(|f| !f.negrito),
            italico_sintetico: italico && f.is_some_and(|f| !f.italico),
        }
    }

    fn escala(&self, face: u16, tamanho: f32) -> f32 {
        match self.faces.get(face as usize) {
            Some(f) => tamanho / f.fonte.unidades as f32,
            None => 0.0,
        }
    }

    /// (ascendente, descendente) em pixels de ponto fixo (descendente > 0).
    pub fn metricas(&self, e: &Escolha) -> (Fixo, Fixo) {
        let Some(f) = self.faces.get(e.face as usize) else { return (fixo(e.tamanho * 0.8), fixo(e.tamanho * 0.2)) };
        let s = self.escala(e.face, e.tamanho);
        (fixo(f.fonte.ascendente as f32 * s), fixo(-(f.fonte.descendente as f32) * s))
    }

    pub fn altura_x(&self, e: &Escolha) -> Fixo {
        match self.faces.get(e.face as usize) {
            Some(f) => fixo(f.fonte.altura_x as f32 * self.escala(e.face, e.tamanho)),
            None => fixo(e.tamanho * 0.5),
        }
    }

    /// Face e glifo para um caractere (com reserva em outras faces).
    pub fn glifo_de(&self, e: &Escolha, c: char) -> (u16, u16) {
        if let Some(f) = self.faces.get(e.face as usize) {
            let g = f.fonte.glifo(c);
            if g != 0 || c.is_control() {
                return (e.face, g);
            }
        }
        for (k, f) in self.faces.iter().enumerate() {
            let g = f.fonte.glifo(c);
            if g != 0 {
                return (k as u16, g);
            }
        }
        (e.face, 0)
    }

    /// Avanco de um glifo em ponto fixo.
    pub fn avanco(&self, e: &Escolha, face: u16, g: u16) -> Fixo {
        let Some(f) = self.faces.get(face as usize) else { return fixo(e.tamanho * 0.5) };
        let mut a = f.fonte.avanco(g) as f32 * self.escala(face, e.tamanho);
        if e.negrito_sintetico {
            a += e.tamanho / 24.0;
        }
        fixo(a)
    }

    /// Largura de um texto (com kerning e espacamento entre letras).
    pub fn medir(&self, e: &Escolha, texto: &str, espaco_letras: Fixo) -> Fixo {
        let mut x: Fixo = 0;
        let mut anterior: Option<(u16, u16)> = None;
        for c in texto.chars() {
            let (face, g) = self.glifo_de(e, c);
            if let Some((fa, ga)) = anterior {
                if fa == face {
                    if let Some(f) = self.faces.get(face as usize) {
                        x += fixo(f.fonte.kerning(ga, g) as f32 * self.escala(face, e.tamanho));
                    }
                }
            }
            x += self.avanco(e, face, g) + espaco_letras;
            anterior = Some((face, g));
        }
        x
    }

    /// Os glifos posicionados de um texto.
    pub fn glifos(&self, e: &Escolha, texto: &str, espaco_letras: Fixo, saida: &mut Vec<Glifo>) -> Fixo {
        let mut x: Fixo = 0;
        let mut anterior: Option<(u16, u16)> = None;
        for c in texto.chars() {
            let (face, g) = self.glifo_de(e, c);
            if let Some((fa, ga)) = anterior {
                if fa == face {
                    if let Some(f) = self.faces.get(face as usize) {
                        x += fixo(f.fonte.kerning(ga, g) as f32 * self.escala(face, e.tamanho));
                    }
                }
            }
            if !c.is_whitespace() {
                saida.push(Glifo { face, id: g, x });
            }
            x += self.avanco(e, face, g) + espaco_letras;
            anterior = Some((face, g));
        }
        x
    }

    /// O glifo desenhado (do cache). `sub` = fracao de pixel (0..3) da
    /// posicao horizontal, para o texto nao "tremer".
    pub fn bitmap(&mut self, e: &Escolha, face: u16, g: u16, sub: u8) -> Rc<Bitmap> {
        let chave = (face, g, (e.tamanho * 4.0) as u32, sub, (e.negrito_sintetico as u8) | ((e.italico_sintetico as u8) << 1));
        if let Some(b) = self.cache.get(&chave) {
            return b.clone();
        }
        let b = match self.faces.get(face as usize) {
            Some(f) => {
                let contornos = f.fonte.contornos(g);
                let escala = e.tamanho / f.fonte.unidades as f32;
                let inclinar = if e.italico_sintetico { 0.2 } else { 0.0 };
                let engrossar = if e.negrito_sintetico { e.tamanho / 24.0 } else { 0.0 };
                raster::desenhar(&contornos, escala, sub as f32 / 4.0, inclinar, engrossar)
            }
            None => Bitmap::default(),
        };
        let rc = Rc::new(b);
        if self.cache.len() > 8192 {
            self.cache.clear();
        }
        self.cache.insert(chave, rc.clone());
        rc
    }
}
