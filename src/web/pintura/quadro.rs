//! Um quadro de pixels XRGB (0x00RRGGBB, o formato do framebuffer do
//! MXOS) e as primitivas de desenho com mistura alfa e recorte.
//! Coordenadas inteiras em pixels (o layout ja arredondou).

use alloc::vec;
use alloc::vec::Vec;

use crate::web::css::valores::Cor;
use crate::web::fonte::raster::Bitmap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Ret {
    pub x: i32,
    pub y: i32,
    pub l: i32,
    pub a: i32,
}

impl Ret {
    pub fn novo(x: i32, y: i32, l: i32, a: i32) -> Ret {
        Ret { x, y, l, a }
    }
    pub fn vazio(&self) -> bool {
        self.l <= 0 || self.a <= 0
    }
    pub fn intersecao(&self, o: &Ret) -> Ret {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = (self.x + self.l).min(o.x + o.l);
        let y1 = (self.y + self.a).min(o.y + o.a);
        Ret { x: x0, y: y0, l: (x1 - x0).max(0), a: (y1 - y0).max(0) }
    }
    pub fn contem(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.l && y < self.y + self.a
    }
}

pub struct Quadro {
    pub largura: i32,
    pub altura: i32,
    pub px: Vec<u32>,
    pub recorte: Ret,
}

/// a*b/255 arredondado, sem divisao.
#[inline]
fn mul8(a: u32, b: u32) -> u32 {
    let t = a * b + 128;
    (t + (t >> 8)) >> 8
}

#[inline]
fn misturar(fundo: u32, cor: u32, alfa: u32) -> u32 {
    if alfa >= 255 {
        return cor & 0xFF_FFFF;
    }
    if alfa == 0 {
        return fundo;
    }
    // dois canais de uma vez (R e B em 0x00FF00FF, G sozinho)
    let inv = 255 - alfa;
    let rb = ((cor & 0xFF00FF) * alfa + (fundo & 0xFF00FF) * inv + 0x800080) >> 8 & 0xFF00FF;
    let g = ((cor & 0x00FF00) * alfa + (fundo & 0x00FF00) * inv + 0x008000) >> 8 & 0x00FF00;
    rb | g
}

impl Quadro {
    pub fn novo(largura: i32, altura: i32, fundo: u32) -> Quadro {
        Quadro {
            largura,
            altura,
            px: vec![fundo; (largura.max(0) * altura.max(0)) as usize],
            recorte: Ret::novo(0, 0, largura, altura),
        }
    }

    pub fn todo(&self) -> Ret {
        Ret::novo(0, 0, self.largura, self.altura)
    }

    /// Retangulo cheio com a cor (alfa da cor e `opacidade` extra 0..255).
    pub fn retangulo(&mut self, r: Ret, cor: Cor, opacidade: u8) {
        let alfa = mul8(cor.a() as u32, opacidade as u32);
        if alfa == 0 {
            return;
        }
        let r = r.intersecao(&self.recorte);
        if r.vazio() {
            return;
        }
        let c = cor.0 & 0xFF_FFFF;
        let l = self.largura as usize;
        for y in r.y..r.y + r.a {
            let linha = &mut self.px[y as usize * l + r.x as usize..y as usize * l + (r.x + r.l) as usize];
            if alfa >= 255 {
                linha.fill(c);
            } else {
                for p in linha.iter_mut() {
                    *p = misturar(*p, c, alfa);
                }
            }
        }
    }

    /// Um glifo (cobertura) com a origem em (x, y) da linha de base.
    pub fn glifo(&mut self, b: &Bitmap, x: i32, y: i32, cor: Cor, opacidade: u8) {
        let alfa_cor = mul8(cor.a() as u32, opacidade as u32);
        if alfa_cor == 0 || b.largura == 0 {
            return;
        }
        let gx = x + b.x as i32;
        let gy = y + b.y as i32;
        let alvo = Ret::novo(gx, gy, b.largura as i32, b.altura as i32).intersecao(&self.recorte);
        if alvo.vazio() {
            return;
        }
        let c = cor.0 & 0xFF_FFFF;
        let l = self.largura as usize;
        let bl = b.largura as usize;
        for yy in alvo.y..alvo.y + alvo.a {
            let lb = (yy - gy) as usize * bl;
            let lq = yy as usize * l;
            for xx in alvo.x..alvo.x + alvo.l {
                let cob = b.alfa[lb + (xx - gx) as usize] as u32;
                if cob == 0 {
                    continue;
                }
                let k = lq + xx as usize;
                self.px[k] = misturar(self.px[k], c, mul8(cob, alfa_cor));
            }
        }
    }

    /// Copia uma imagem ARGB (com alfa) escalada para o retangulo `r`.
    pub fn imagem(&mut self, origem: &[u32], ol: i32, oa: i32, r: Ret, opacidade: u8) {
        if ol <= 0 || oa <= 0 || r.vazio() {
            return;
        }
        let alvo = r.intersecao(&self.recorte);
        if alvo.vazio() {
            return;
        }
        let l = self.largura as usize;
        // passo em 16.16 para escalar sem divisao por pixel
        let px = ((ol as i64) << 16) / r.l as i64;
        let py = ((oa as i64) << 16) / r.a as i64;
        for yy in alvo.y..alvo.y + alvo.a {
            let sy = (((yy - r.y) as i64 * py) >> 16).min(oa as i64 - 1) as usize;
            let lo = sy * ol as usize;
            let lq = yy as usize * l;
            let mut sx_f = (alvo.x - r.x) as i64 * px;
            for xx in alvo.x..alvo.x + alvo.l {
                let sx = ((sx_f >> 16) as usize).min(ol as usize - 1);
                sx_f += px;
                let s = origem[lo + sx];
                let a = mul8(s >> 24, opacidade as u32);
                let k = lq + xx as usize;
                self.px[k] = misturar(self.px[k], s, a);
            }
        }
    }

    /// Linha horizontal de 1px (sublinhados, bordas finas).
    pub fn linha_h(&mut self, x: i32, y: i32, l: i32, esp: i32, cor: Cor) {
        self.retangulo(Ret::novo(x, y, l, esp.max(1)), cor, 255);
    }
}
