//! JPEG (ITU T.81): baseline e progressivo, Huffman, subamostragem de cor
//! (4:4:4, 4:2:2, 4:2:0...), intervalos de reinicio, tons de cinza, CMYK.
//! Os coeficientes de todos os blocos ficam guardados (o progressivo chega
//! em varias passadas) e a IDCT inteira (aritmetica de 32 bits, sem float)
//! roda no fim.

use alloc::vec;
use alloc::vec::Vec;

use super::Imagem;

const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57,
    50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

#[derive(Clone, Default)]
struct Huff {
    /// (codigo, comprimento) -> simbolo, por busca: tabela de 9 bits + lento
    rapido: Vec<u16>, // (simbolo << 4) | comprimento
    maxcode: [i32; 18],
    valptr: [i32; 17],
    mincode: [i32; 17],
    valores: Vec<u8>,
}

impl Huff {
    fn novo(contagens: &[u8; 16], valores: &[u8]) -> Huff {
        let mut h = Huff { rapido: vec![0; 512], valores: valores.to_vec(), ..Default::default() };
        let mut codigo = 0i32;
        let mut k = 0i32;
        for l in 1..=16 {
            let n = contagens[l - 1] as i32;
            h.valptr[l] = k;
            h.mincode[l] = codigo;
            for _ in 0..n {
                if l <= 9 {
                    let sobra = 9 - l;
                    let base = (codigo << sobra) as usize;
                    for j in 0..(1 << sobra) {
                        if base + j < 512 {
                            h.rapido[base + j] = ((valores.get(k as usize).copied().unwrap_or(0) as u16) << 4) | l as u16;
                        }
                    }
                }
                codigo += 1;
                k += 1;
            }
            h.maxcode[l] = if n > 0 { codigo - 1 } else { -1 };
            codigo <<= 1;
        }
        h.maxcode[17] = i32::MAX;
        h
    }
}

struct Bits<'a> {
    d: &'a [u8],
    i: usize,
    buf: u32,
    n: u32,
    marcador: bool,
}

impl Bits<'_> {
    fn encher(&mut self) {
        while self.n <= 24 {
            let mut b = 0u32;
            if !self.marcador && self.i < self.d.len() {
                let x = self.d[self.i];
                if x == 0xFF {
                    let prox = self.d.get(self.i + 1).copied().unwrap_or(0);
                    if prox == 0 {
                        self.i += 2;
                        b = 0xFF;
                    } else {
                        self.marcador = true;
                    }
                } else {
                    self.i += 1;
                    b = x as u32;
                }
            }
            self.buf |= b << (24 - self.n);
            self.n += 8;
        }
    }
    fn bit(&mut self) -> u32 {
        self.bits(1)
    }
    fn bits(&mut self, k: u32) -> u32 {
        if k == 0 {
            return 0;
        }
        if self.n < k {
            self.encher();
        }
        let v = self.buf >> (32 - k);
        self.buf <<= k;
        self.n -= k;
        v
    }
    fn receber(&mut self, s: u32) -> i32 {
        // valor com sinal de s bits (EXTEND da norma)
        if s == 0 {
            return 0;
        }
        let v = self.bits(s) as i32;
        if v < (1 << (s - 1)) { v - (1 << s) + 1 } else { v }
    }
    fn decodificar(&mut self, h: &Huff) -> u8 {
        if self.n < 16 {
            self.encher();
        }
        let r = h.rapido[(self.buf >> 23) as usize];
        if r & 15 != 0 {
            let l = (r & 15) as u32;
            self.buf <<= l;
            self.n -= l;
            return (r >> 4) as u8;
        }
        let mut codigo = 0i32;
        for l in 1..=16 {
            codigo = (codigo << 1) | self.bit() as i32;
            if codigo <= h.maxcode[l] && h.maxcode[l] >= 0 {
                let k = h.valptr[l] + codigo - h.mincode[l];
                return h.valores.get(k as usize).copied().unwrap_or(0);
            }
        }
        0
    }
    /// Pula para depois de um marcador RSTn.
    fn reiniciar(&mut self) {
        self.buf = 0;
        self.n = 0;
        self.marcador = false;
        while self.i + 1 < self.d.len() {
            if self.d[self.i] == 0xFF && (0xD0..=0xD7).contains(&self.d[self.i + 1]) {
                self.i += 2;
                return;
            }
            self.i += 1;
        }
    }
}

#[derive(Clone, Default)]
struct Comp {
    id: u8,
    h: usize,
    v: usize,
    tq: usize,
    blocos_l: usize,
    blocos_a: usize,
    coef: Vec<[i16; 64]>,
    pred: i32,
    td: usize,
    ta: usize,
}

const A: i32 = 4096;
fn f2f(x: f32) -> i32 {
    (x * A as f32 + 0.5) as i32
}

fn idct(dados: &[i32; 64], saida: &mut [u8; 64]) {
    let mut v = [0i32; 64];
    let c = (f2f(0.5411961), f2f(-1.847759065), f2f(0.765366865));
    let d = (f2f(1.175875602), f2f(0.298631336), f2f(2.053119869), f2f(3.072711026), f2f(1.501321110));
    let e = (f2f(-0.899976223), f2f(-2.562915447), f2f(-1.961570560), f2f(-0.390180644));
    let um = |s: [i32; 8]| -> ([i32; 4], [i32; 4]) {
        let p1 = (s[2] + s[6]) * c.0;
        let t2 = p1 + s[6] * c.1;
        let t3 = p1 + s[2] * c.2;
        let t0 = (s[0] + s[4]) * A;
        let t1 = (s[0] - s[4]) * A;
        let x0 = t0 + t3;
        let x3 = t0 - t3;
        let x1 = t1 + t2;
        let x2 = t1 - t2;
        let (mut t0, mut t1, mut t2, mut t3) = (s[7], s[5], s[3], s[1]);
        let p3 = t0 + t2;
        let p4 = t1 + t3;
        let p1 = t0 + t3;
        let p2 = t1 + t2;
        let p5 = (p3 + p4) * d.0;
        t0 *= d.1;
        t1 *= d.2;
        t2 *= d.3;
        t3 *= d.4;
        let p1 = p5 + p1 * e.0;
        let p2 = p5 + p2 * e.1;
        let p3 = p3 * e.2;
        let p4 = p4 * e.3;
        t3 += p1 + p4;
        t2 += p2 + p3;
        t1 += p2 + p4;
        t0 += p1 + p3;
        ([x0, x1, x2, x3], [t0, t1, t2, t3])
    };
    for i in 0..8 {
        let col = |r: usize| dados[r * 8 + i];
        if (1..8).all(|r| col(r) == 0) {
            let dc = col(0) * 4;
            for r in 0..8 {
                v[r * 8 + i] = dc;
            }
            continue;
        }
        let (x, t) = um([col(0), col(1), col(2), col(3), col(4), col(5), col(6), col(7)]);
        let x: [i32; 4] = [x[0] + 512, x[1] + 512, x[2] + 512, x[3] + 512];
        v[i] = (x[0] + t[3]) >> 10;
        v[56 + i] = (x[0] - t[3]) >> 10;
        v[8 + i] = (x[1] + t[2]) >> 10;
        v[48 + i] = (x[1] - t[2]) >> 10;
        v[16 + i] = (x[2] + t[1]) >> 10;
        v[40 + i] = (x[2] - t[1]) >> 10;
        v[24 + i] = (x[3] + t[0]) >> 10;
        v[32 + i] = (x[3] - t[0]) >> 10;
    }
    let lim = |x: i32| x.clamp(0, 255) as u8;
    for r in 0..8 {
        let s: [i32; 8] = core::array::from_fn(|k| v[r * 8 + k]);
        let (x, t) = um(s);
        let soma = 65536 + (128 << 17);
        let x: [i32; 4] = [x[0] + soma, x[1] + soma, x[2] + soma, x[3] + soma];
        saida[r * 8] = lim((x[0] + t[3]) >> 17);
        saida[r * 8 + 7] = lim((x[0] - t[3]) >> 17);
        saida[r * 8 + 1] = lim((x[1] + t[2]) >> 17);
        saida[r * 8 + 6] = lim((x[1] - t[2]) >> 17);
        saida[r * 8 + 2] = lim((x[2] + t[1]) >> 17);
        saida[r * 8 + 5] = lim((x[2] - t[1]) >> 17);
        saida[r * 8 + 3] = lim((x[3] + t[0]) >> 17);
        saida[r * 8 + 4] = lim((x[3] - t[0]) >> 17);
    }
}

pub fn decodificar(d: &[u8]) -> Option<Imagem> {
    decodificar_max(d, usize::MAX, usize::MAX)
}

/// Decodifica ja reduzido por 2, 4 ou 8 (o maior que ainda deixa a imagem
/// com pelo menos max_l x max_a): foto grande nao passa pela memoria em
/// tamanho cheio (na escala 8 so o DC de cada bloco e usado).
pub fn decodificar_max(d: &[u8], max_l: usize, max_a: usize) -> Option<Imagem> {
    if d.len() < 4 || d[0] != 0xFF || d[1] != 0xD8 {
        return None;
    }
    let mut qt = [[1u16; 64]; 4];
    let mut hdc: [Huff; 4] = Default::default();
    let mut hac: [Huff; 4] = Default::default();
    let mut comps: Vec<Comp> = Vec::new();
    let (mut l, mut a) = (0usize, 0usize);
    let mut progressivo = false;
    let mut reinicio = 0usize;
    let (mut hmax, mut vmax) = (1usize, 1usize);
    let (mut mcux, mut mcuy) = (0usize, 0usize);
    let mut adobe_transformar: Option<u8> = None;
    let mut i = 2;
    while i + 4 <= d.len() {
        if d[i] != 0xFF {
            i += 1;
            continue;
        }
        let m = d[i + 1];
        if m == 0xD8 || m == 0x01 || (0xD0..=0xD7).contains(&m) || m == 0xFF {
            i += if m == 0xFF { 1 } else { 2 };
            continue;
        }
        if m == 0xD9 {
            break;
        }
        let n = u16::from_be_bytes([d[i + 2], d[i + 3]]) as usize;
        let seg = d.get(i + 4..i + 2 + n)?;
        match m {
            0xDB => {
                let mut p = 0;
                while p < seg.len() {
                    let pq = seg[p] >> 4;
                    let tq = (seg[p] & 15) as usize & 3;
                    p += 1;
                    for k in 0..64 {
                        qt[tq][ZIGZAG[k]] = if pq == 0 {
                            *seg.get(p + k)? as u16
                        } else {
                            u16::from_be_bytes([*seg.get(p + 2 * k)?, *seg.get(p + 2 * k + 1)?])
                        };
                    }
                    p += if pq == 0 { 64 } else { 128 };
                }
            }
            0xC0 | 0xC1 | 0xC2 => {
                progressivo = m == 0xC2;
                a = u16::from_be_bytes([seg[1], seg[2]]) as usize;
                l = u16::from_be_bytes([seg[3], seg[4]]) as usize;
                let nc = seg[5] as usize;
                if l == 0 || a == 0 || l > 16384 || a > 16384 || l * a > 40_000_000 {
                    return None;
                }
                for k in 0..nc {
                    let b = 6 + 3 * k;
                    comps.push(Comp {
                        id: seg[b],
                        h: (seg[b + 1] >> 4).max(1) as usize,
                        v: (seg[b + 1] & 15).max(1) as usize,
                        tq: (seg[b + 2] & 3) as usize,
                        ..Default::default()
                    });
                }
                hmax = comps.iter().map(|c| c.h).max().unwrap_or(1);
                vmax = comps.iter().map(|c| c.v).max().unwrap_or(1);
                mcux = l.div_ceil(8 * hmax);
                mcuy = a.div_ceil(8 * vmax);
                for c in comps.iter_mut() {
                    c.blocos_l = mcux * c.h;
                    c.blocos_a = mcuy * c.v;
                    c.coef = vec![[0i16; 64]; c.blocos_l * c.blocos_a];
                }
            }
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => return None, // sem perdas / aritmetico
            0xC4 => {
                let mut p = 0;
                while p + 17 <= seg.len() {
                    let tc = seg[p] >> 4;
                    let th = (seg[p] & 15) as usize & 3;
                    let mut cont = [0u8; 16];
                    cont.copy_from_slice(&seg[p + 1..p + 17]);
                    let total: usize = cont.iter().map(|&x| x as usize).sum();
                    let valores = seg.get(p + 17..p + 17 + total)?;
                    let h = Huff::novo(&cont, valores);
                    if tc == 0 {
                        hdc[th] = h;
                    } else {
                        hac[th] = h;
                    }
                    p += 17 + total;
                }
            }
            0xDD => reinicio = u16::from_be_bytes([seg[0], seg[1]]) as usize,
            0xEE => {
                if seg.len() >= 12 && &seg[..5] == b"Adobe" {
                    adobe_transformar = Some(seg[11]);
                }
            }
            0xDA => {
                let ns = seg[0] as usize;
                let mut na_varredura: Vec<usize> = Vec::new();
                for k in 0..ns {
                    let cid = seg[1 + 2 * k];
                    let t = seg[2 + 2 * k];
                    if let Some(ci) = comps.iter().position(|c| c.id == cid) {
                        comps[ci].td = (t >> 4) as usize & 3;
                        comps[ci].ta = (t & 15) as usize & 3;
                        na_varredura.push(ci);
                    }
                }
                let b = 1 + 2 * ns;
                let ss = seg[b] as usize;
                let se = (seg[b + 1] as usize).min(63);
                let ah = (seg[b + 2] >> 4) as u32;
                let al = (seg[b + 2] & 15) as u32;
                let inicio = i + 2 + n;
                let mut bits = Bits { d, i: inicio, buf: 0, n: 0, marcador: false };
                for &ci in &na_varredura {
                    comps[ci].pred = 0;
                }
                let mut eobrun = 0u32;
                let unica = na_varredura.len() == 1;
                let (unid_l, unid_a) = if unica {
                    let c = &comps[na_varredura[0]];
                    ((l * c.h).div_ceil(8 * hmax), (a * c.v).div_ceil(8 * vmax))
                } else {
                    (mcux, mcuy)
                };
                let total = unid_l * unid_a;
                let mut cont = 0usize;
                for u in 0..total {
                    if reinicio > 0 && cont == reinicio {
                        bits.reiniciar();
                        for &ci in &na_varredura {
                            comps[ci].pred = 0;
                        }
                        eobrun = 0;
                        cont = 0;
                    }
                    cont += 1;
                    let (ux, uy) = (u % unid_l, u / unid_l);
                    for &ci in &na_varredura {
                        let (h, v) = if unica { (1, 1) } else { (comps[ci].h, comps[ci].v) };
                        for by in 0..v {
                            for bx in 0..h {
                                let (x, y) = if unica { (ux, uy) } else { (ux * comps[ci].h + bx, uy * comps[ci].v + by) };
                                let c = &mut comps[ci];
                                let idx = y * c.blocos_l + x;
                                if idx >= c.coef.len() {
                                    continue;
                                }
                                let (td, ta) = (c.td, c.ta);
                                if !progressivo {
                                    let bloco = &mut c.coef[idx];
                                    let s = bits.decodificar(&hdc[td]) as u32;
                                    c.pred += bits.receber(s);
                                    bloco[0] = c.pred as i16;
                                    let mut k = 1;
                                    while k < 64 {
                                        let rs = bits.decodificar(&hac[ta]);
                                        let r = (rs >> 4) as usize;
                                        let s = (rs & 15) as u32;
                                        if s == 0 {
                                            if r == 15 {
                                                k += 16;
                                                continue;
                                            }
                                            break;
                                        }
                                        k += r;
                                        if k > 63 {
                                            break;
                                        }
                                        bloco[ZIGZAG[k]] = bits.receber(s) as i16;
                                        k += 1;
                                    }
                                } else if ss == 0 {
                                    // DC progressivo
                                    if ah == 0 {
                                        let s = bits.decodificar(&hdc[td]) as u32;
                                        c.pred += bits.receber(s);
                                        c.coef[idx][0] = (c.pred << al) as i16;
                                    } else if bits.bit() == 1 {
                                        c.coef[idx][0] |= (1 << al) as i16;
                                    }
                                } else if ah == 0 {
                                    // AC primeira passada
                                    if eobrun > 0 {
                                        eobrun -= 1;
                                        continue;
                                    }
                                    let bloco = &mut c.coef[idx];
                                    let mut k = ss;
                                    while k <= se {
                                        let rs = bits.decodificar(&hac[ta]);
                                        let r = (rs >> 4) as u32;
                                        let s = (rs & 15) as u32;
                                        if s == 0 {
                                            if r < 15 {
                                                eobrun = (1 << r) - 1;
                                                if r > 0 {
                                                    eobrun += bits.bits(r);
                                                }
                                                break;
                                            }
                                            k += 16;
                                            continue;
                                        }
                                        k += r as usize;
                                        if k > 63 {
                                            break;
                                        }
                                        bloco[ZIGZAG[k]] = (bits.receber(s) * (1 << al)) as i16;
                                        k += 1;
                                    }
                                } else {
                                    // AC refinamento
                                    let p1 = 1i16 << al;
                                    let m1 = -1i16 << al;
                                    let bloco = &mut c.coef[idx];
                                    let mut k = ss;
                                    if eobrun == 0 {
                                        while k <= se {
                                            let rs = bits.decodificar(&hac[ta]);
                                            let mut r = (rs >> 4) as i32;
                                            let s = (rs & 15) as u32;
                                            let mut novo = 0i16;
                                            if s != 0 {
                                                novo = if bits.bit() == 1 { p1 } else { m1 };
                                            } else if r != 15 {
                                                eobrun = 1 << r;
                                                if r > 0 {
                                                    eobrun += bits.bits(r as u32);
                                                }
                                                break;
                                            }
                                            while k <= se {
                                                let z = ZIGZAG[k];
                                                if bloco[z] != 0 {
                                                    if bits.bit() == 1 && bloco[z] & p1 == 0 {
                                                        bloco[z] += if bloco[z] >= 0 { p1 } else { m1 };
                                                    }
                                                } else {
                                                    r -= 1;
                                                    if r < 0 {
                                                        break;
                                                    }
                                                }
                                                k += 1;
                                            }
                                            if novo != 0 && k <= 63 {
                                                bloco[ZIGZAG[k]] = novo;
                                            }
                                            k += 1;
                                        }
                                    }
                                    if eobrun > 0 {
                                        while k <= se {
                                            let z = ZIGZAG[k];
                                            if bloco[z] != 0 && bits.bit() == 1 && bloco[z] & p1 == 0 {
                                                bloco[z] += if bloco[z] >= 0 { p1 } else { m1 };
                                            }
                                            k += 1;
                                        }
                                        eobrun -= 1;
                                    }
                                }
                            }
                        }
                    }
                }
                // proximo segmento: procura o marcador seguinte
                let mut j = bits.i.max(inicio);
                while j + 1 < d.len() && !(d[j] == 0xFF && d[j + 1] != 0 && !(0xD0..=0xD7).contains(&d[j + 1])) {
                    j += 1;
                }
                i = j;
                continue;
            }
            _ => {}
        }
        i += 2 + n;
    }
    if comps.is_empty() || l == 0 {
        return None;
    }
    let mut escala = 1;
    while escala < 8 && l / (escala * 2) >= max_l && a / (escala * 2) >= max_a {
        escala *= 2;
    }
    let lado = 8 / escala; // pixels por bloco em cada direcao
    // IDCT de todos os blocos de cada componente
    let mut planos: Vec<(Vec<u8>, usize, usize, usize)> = Vec::new(); // (pixels, largura, h, v)
    for c in &mut comps {
        let pl = c.blocos_l * lado;
        let mut plano = vec![0u8; pl * c.blocos_a * lado];
        let q = &qt[c.tq];
        let mut dq = [0i32; 64];
        let mut saida = [0u8; 64];
        for by in 0..c.blocos_a {
            for bx in 0..c.blocos_l {
                let b = &c.coef[by * c.blocos_l + bx];
                if escala == 8 {
                    // so o DC: a media do bloco
                    plano[by * pl + bx] = ((b[0] as i32 * q[0] as i32 + 4) / 8 + 128).clamp(0, 255) as u8;
                    continue;
                }
                for k in 0..64 {
                    dq[k] = b[k] as i32 * q[k] as i32;
                }
                idct(&dq, &mut saida);
                for r in 0..lado {
                    let o = (by * lado + r) * pl + bx * lado;
                    for x in 0..lado {
                        // media de escala x escala pixels
                        let mut s = 0u32;
                        for dy in 0..escala {
                            for dx in 0..escala {
                                s += saida[(r * escala + dy) * 8 + x * escala + dx] as u32;
                            }
                        }
                        plano[o + x] = (s / (escala * escala) as u32) as u8;
                    }
                }
            }
        }
        // os coeficientes nao servem mais: libera antes de montar a imagem
        c.coef = Vec::new();
        planos.push((plano, pl, c.h, c.v));
    }
    let (l, a) = (l.div_ceil(escala), a.div_ceil(escala));
    let mut px = vec![0u32; l * a];
    let amostra = |p: &(Vec<u8>, usize, usize, usize), x: usize, y: usize| -> i32 {
        let sx = x * p.2 / hmax;
        let sy = y * p.3 / vmax;
        p.0[sy * p.1 + sx] as i32
    };
    for y in 0..a {
        for x in 0..l {
            let cor = match planos.len() {
                1 | 2 => {
                    let g = amostra(&planos[0], x, y) as u32;
                    0xFF00_0000 | g << 16 | g << 8 | g
                }
                3 => {
                    let yy = amostra(&planos[0], x, y);
                    let cb = amostra(&planos[1], x, y) - 128;
                    let cr = amostra(&planos[2], x, y) - 128;
                    if adobe_transformar == Some(0) {
                        // RGB direto
                        0xFF00_0000 | (yy as u32) << 16 | ((cb + 128) as u32) << 8 | (cr + 128) as u32
                    } else {
                        let r = yy + ((91881 * cr + 32768) >> 16);
                        let g = yy - ((22554 * cb + 46802 * cr + 32768) >> 16);
                        let b = yy + ((116130 * cb + 32768) >> 16);
                        0xFF00_0000 | (r.clamp(0, 255) as u32) << 16 | (g.clamp(0, 255) as u32) << 8 | b.clamp(0, 255) as u32
                    }
                }
                _ => {
                    // CMYK (Adobe, invertido) ou YCCK
                    let (mut c0, mut c1, mut c2) = (amostra(&planos[0], x, y), amostra(&planos[1], x, y), amostra(&planos[2], x, y));
                    let k = amostra(&planos[3], x, y);
                    if adobe_transformar == Some(2) {
                        let (yy, cb, cr) = (c0, c1 - 128, c2 - 128);
                        c0 = (yy + ((91881 * cr) >> 16)).clamp(0, 255);
                        c1 = (yy - ((22554 * cb + 46802 * cr) >> 16)).clamp(0, 255);
                        c2 = (yy + ((116130 * cb) >> 16)).clamp(0, 255);
                    }
                    let r = c0 * k / 255;
                    let g = c1 * k / 255;
                    let b = c2 * k / 255;
                    0xFF00_0000 | (r as u32) << 16 | (g as u32) << 8 | b as u32
                }
            };
            px[y * l + x] = cor;
        }
    }
    Some(Imagem { largura: l as i32, altura: a as i32, px })
}
