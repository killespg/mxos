//! Rasterizador de contornos com antisserrilhamento por area exata: cada
//! segmento soma, em cada pixel que cruza, a area coberta (com sinal);
//! uma soma acumulada por linha da a cobertura final. E o metodo do
//! font-rs/stb_truetype 2, reescrito aqui. As curvas quadraticas viram
//! retas (o numero de pedacos depende do tamanho da curva).

use alloc::vec;
use alloc::vec::Vec;

use super::ttf::Contornos;

pub struct Raster {
    l: usize,
    a: usize,
    acc: Vec<f32>,
}

impl Raster {
    pub fn novo(largura: usize, altura: usize) -> Raster {
        Raster { l: largura, a: altura, acc: vec![0.0; largura * altura + 4] }
    }

    pub fn linha(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) {
        if (y0 - y1).abs() <= 1e-6 {
            return;
        }
        let (dir, (x0, y0), (x1, y1)) = if y0 < y1 { (1.0, (x0, y0), (x1, y1)) } else { (-1.0, (x1, y1), (x0, y0)) };
        let dxdy = (x1 - x0) / (y1 - y0);
        let mut x = x0;
        let y_ini = if y0 < 0.0 {
            x -= y0 * dxdy;
            0
        } else {
            y0 as usize
        };
        let y_fim = (self.a as f32).min(y1.ceil()) as usize;
        let w = self.l;
        for y in y_ini..y_fim {
            let base = y * w;
            let dy = ((y + 1) as f32).min(y1) - (y as f32).max(y0);
            let xprox = x + dxdy * dy;
            let d = dy * dir;
            let (xa, xb) = if x < xprox { (x, xprox) } else { (xprox, x) };
            let xa = xa.max(0.0);
            let xb = xb.max(0.0).min(w as f32 - 0.001);
            let xa_chao = xa.floor();
            let xa_i = xa_chao as usize;
            let xb_teto = xb.ceil();
            let xb_i = xb_teto as usize;
            if xb_i <= xa_i + 1 {
                let xm = 0.5 * (x + xprox) - xa_chao;
                self.acc[base + xa_i] += d - d * xm;
                self.acc[base + xa_i + 1] += d * xm;
            } else {
                let s = 1.0 / (xb - xa);
                let xa_f = xa - xa_chao;
                let a0 = 0.5 * s * (1.0 - xa_f) * (1.0 - xa_f);
                let xb_f = xb - xb_teto + 1.0;
                let am = 0.5 * s * xb_f * xb_f;
                self.acc[base + xa_i] += d * a0;
                if xb_i == xa_i + 2 {
                    self.acc[base + xa_i + 1] += d * (1.0 - a0 - am);
                } else {
                    let a1 = s * (1.5 - xa_f);
                    self.acc[base + xa_i + 1] += d * (a1 - a0);
                    for xi in xa_i + 2..xb_i - 1 {
                        self.acc[base + xi] += d * s;
                    }
                    let a2 = a1 + (xb_i - xa_i - 3) as f32 * s;
                    self.acc[base + xb_i - 1] += d * (1.0 - a2 - am);
                }
                self.acc[base + xb_i] += d * am;
            }
            x = xprox;
        }
    }

    pub fn quadratica(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, x2: f32, y2: f32) {
        let dx = x0 - 2.0 * x1 + x2;
        let dy = y0 - 2.0 * y1 + y2;
        let dd = dx * dx + dy * dy;
        let n = 1 + (3.0 * dd).sqrt().sqrt() as usize;
        let n = n.min(32);
        let mut px = x0;
        let mut py = y0;
        let passo = 1.0 / n as f32;
        for k in 1..=n {
            let t = k as f32 * passo;
            let u = 1.0 - t;
            let nx = u * u * x0 + 2.0 * u * t * x1 + t * t * x2;
            let ny = u * u * y0 + 2.0 * u * t * y1 + t * t * y2;
            self.linha(px, py, nx, ny);
            px = nx;
            py = ny;
        }
    }

    /// Cobertura 0..255 por pixel (linha a linha).
    pub fn cobertura(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(self.l * self.a);
        for y in 0..self.a {
            let mut soma = 0.0f32;
            for x in 0..self.l {
                soma += self.acc[y * self.l + x];
                let c = soma.abs().min(1.0);
                v.push((c * 255.0 + 0.5) as u8);
            }
        }
        v
    }
}

/// Um glifo desenhado: cobertura e onde fica em relacao a origem da
/// linha de base (x para a direita, y para baixo).
#[derive(Clone, Debug, Default)]
pub struct Bitmap {
    pub largura: u16,
    pub altura: u16,
    pub x: i16,
    pub y: i16,
    pub alfa: Vec<u8>,
}

/// Desenha os contornos na escala dada (pixels por unidade), com um
/// deslocamento horizontal fracionario (para posicoes sub-pixel) e
/// inclinacao (italico sintetico) e engrossamento (negrito sintetico).
pub fn desenhar(c: &Contornos, escala: f32, desloc_x: f32, inclinar: f32, engrossar: f32) -> Bitmap {
    if c.is_empty() {
        return Bitmap::default();
    }
    let (mut x_min, mut y_min, mut x_max, mut y_max) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in c.iter().flatten() {
        let x = p.x * escala + p.y * escala * inclinar;
        let y = -p.y * escala;
        x_min = x_min.min(x);
        x_max = x_max.max(x);
        y_min = y_min.min(y);
        y_max = y_max.max(y);
    }
    let x0 = (x_min + desloc_x - engrossar).floor();
    let y0 = y_min.floor();
    let largura = ((x_max + desloc_x + engrossar).ceil() - x0) as usize + 2;
    let altura = (y_max.ceil() - y0) as usize + 1;
    if largura > 2048 || altura > 2048 {
        return Bitmap::default();
    }
    let mut r = Raster::novo(largura, altura);
    let tr = |x: f32, y: f32| (x * escala + y * escala * inclinar + desloc_x - x0, -y * escala - y0);
    let passadas: &[f32] = if engrossar > 0.0 { &[0.0, 1.0] } else { &[0.0] };
    for &k in passadas {
        let dx = k * engrossar;
        for cont in c {
            let n = cont.len();
            if n < 2 {
                continue;
            }
            // primeiro ponto na curva (ou o meio entre dois de controle)
            let inicio = match cont.iter().position(|p| p.na_curva) {
                Some(i) => i,
                None => n, // todos fora: comeca no meio do 0 e 1
            };
            let (mut sx, mut sy) = if inicio < n {
                tr(cont[inicio].x, cont[inicio].y)
            } else {
                let (ax, ay) = tr(cont[0].x, cont[0].y);
                let (bx, by) = tr(cont[1].x, cont[1].y);
                ((ax + bx) / 2.0, (ay + by) / 2.0)
            };
            sx += dx;
            let (px0, py0) = (sx, sy);
            let mut controle: Option<(f32, f32)> = None;
            let base = if inicio < n { inicio } else { 0 };
            for k2 in 1..=n {
                let p = cont[(base + k2) % n];
                let (mut x, y) = tr(p.x, p.y);
                x += dx;
                if p.na_curva {
                    match controle.take() {
                        Some((cx, cy)) => r.quadratica(sx, sy, cx, cy, x, y),
                        None => r.linha(sx, sy, x, y),
                    }
                    sx = x;
                    sy = y;
                } else {
                    if let Some((cx, cy)) = controle {
                        let mx = (cx + x) / 2.0;
                        let my = (cy + y) / 2.0;
                        r.quadratica(sx, sy, cx, cy, mx, my);
                        sx = mx;
                        sy = my;
                    }
                    controle = Some((x, y));
                }
            }
            match controle {
                Some((cx, cy)) => r.quadratica(sx, sy, cx, cy, px0, py0),
                None => r.linha(sx, sy, px0, py0),
            }
            let _ = sy;
        }
    }
    let mut alfa = r.cobertura();
    if engrossar > 0.0 {
        // duas passadas somam mais que 1 onde se sobrepoem: ja limitado
        for a in alfa.iter_mut() {
            *a = (*a).min(255);
        }
    }
    Bitmap { largura: largura as u16, altura: altura as u16, x: x0 as i16, y: y0 as i16, alfa }
}
