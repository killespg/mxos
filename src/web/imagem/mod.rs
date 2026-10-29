//! Imagens: decodificadores feitos aqui (PNG, JPEG, GIF) e o DEFLATE que
//! o PNG e o gzip do HTTP usam.

pub mod gif;
pub mod inflate;
pub mod jpeg;
pub mod png;

use alloc::vec::Vec;

/// Uma imagem decodificada: 0xAARRGGBB por pixel, linha a linha.
pub struct Imagem {
    pub largura: i32,
    pub altura: i32,
    pub px: Vec<u32>,
}

/// Como `decodificar`, mas ja perto de max_l x max_a quando o formato deixa
/// (JPEG reduz por 2, 4 ou 8 na decodificacao) e reduzido no fim.
pub fn decodificar_max(d: &[u8], max_l: i32, max_a: i32) -> Option<Imagem> {
    let img = if d.starts_with(&[0xFF, 0xD8]) { jpeg::decodificar_max(d, max_l.max(1) as usize, max_a.max(1) as usize)? } else { decodificar(d)? };
    Some(img.reduzir(max_l, max_a))
}

/// Decodifica pelo cabecalho (nao confia no Content-Type).
pub fn decodificar(d: &[u8]) -> Option<Imagem> {
    if d.starts_with(b"\x89PNG") {
        png::decodificar(d)
    } else if d.starts_with(&[0xFF, 0xD8]) {
        jpeg::decodificar(d)
    } else if d.starts_with(b"GIF8") {
        gif::decodificar(d)
    } else {
        None
    }
}

impl Imagem {
    /// Reduz (caixa media) para caber em l x a: imagens enormes nao
    /// ocupam memoria a toa na TV box.
    pub fn reduzir(self, l: i32, a: i32) -> Imagem {
        if self.largura <= l && self.altura <= a || l <= 0 || a <= 0 {
            return self;
        }
        let fx = (self.largura + l - 1) / l;
        let fy = (self.altura + a - 1) / a;
        let f = fx.max(fy).max(1);
        let nl = self.largura / f;
        let na = self.altura / f;
        let mut px = Vec::with_capacity((nl * na) as usize);
        for y in 0..na {
            for x in 0..nl {
                let (mut sa, mut sr, mut sg, mut sb) = (0u32, 0u32, 0u32, 0u32);
                for dy in 0..f {
                    for dx in 0..f {
                        let p = self.px[((y * f + dy) * self.largura + x * f + dx) as usize];
                        sa += p >> 24;
                        sr += (p >> 16) & 255;
                        sg += (p >> 8) & 255;
                        sb += p & 255;
                    }
                }
                let n = (f * f) as u32;
                px.push((sa / n) << 24 | (sr / n) << 16 | (sg / n) << 8 | sb / n);
            }
        }
        Imagem { largura: nl, altura: na, px }
    }
}
