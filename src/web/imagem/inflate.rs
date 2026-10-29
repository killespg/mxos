//! DEFLATE (RFC 1951), zlib (RFC 1950) e gzip (RFC 1952): o que o PNG usa
//! por dentro e o que os servidores mandam com "Content-Encoding: gzip".
//! Huffman com tabela de busca direta de 9 bits (o caso comum resolve com
//! um acesso), o resto por arvore canonica.

use alloc::vec;
use alloc::vec::Vec;

const BASE_COMP: [u16; 29] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const EXTRA_COMP: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const BASE_DIST: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const EXTRA_DIST: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];
const ORDEM_CL: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

const RAPIDO: usize = 9;

struct Bits<'a> {
    d: &'a [u8],
    i: usize,
    buf: u64,
    n: u32,
}

impl<'a> Bits<'a> {
    fn encher(&mut self) {
        while self.n <= 56 {
            let b = if self.i < self.d.len() { self.d[self.i] } else { 0 };
            self.i += 1;
            self.buf |= (b as u64) << self.n;
            self.n += 8;
        }
    }
    fn pegar(&mut self, k: u32) -> u32 {
        if k == 0 {
            return 0;
        }
        if self.n < k {
            self.encher();
        }
        let v = (self.buf & ((1u64 << k) - 1)) as u32;
        self.buf >>= k;
        self.n -= k;
        v
    }
    fn espiar(&mut self, k: u32) -> u32 {
        if self.n < k {
            self.encher();
        }
        (self.buf & ((1u64 << k) - 1)) as u32
    }
    fn alinhar(&mut self) {
        let r = self.n % 8;
        self.pegar(r);
    }
    fn acabou(&self) -> bool {
        self.i > self.d.len() + 8
    }
}

struct Huffman {
    /// (simbolo << 4) | comprimento, indexado pelos 9 bits seguintes
    rapido: Vec<u16>,
    contagem: [u16; 16],
    simbolos: Vec<u16>,
}

impl Huffman {
    fn novo(comprimentos: &[u8]) -> Option<Huffman> {
        let mut contagem = [0u16; 16];
        for &c in comprimentos {
            contagem[c as usize] += 1;
        }
        contagem[0] = 0;
        let mut inicio = [0u16; 16];
        for i in 1..16 {
            inicio[i] = inicio[i - 1] + contagem[i - 1];
        }
        let mut simbolos = vec![0u16; comprimentos.len()];
        let mut pos = inicio;
        for (s, &c) in comprimentos.iter().enumerate() {
            if c != 0 {
                simbolos[pos[c as usize] as usize] = s as u16;
                pos[c as usize] += 1;
            }
        }
        // tabela rapida: codigos canonicos ate 9 bits, invertidos
        let mut rapido = vec![0u16; 1 << RAPIDO];
        let mut codigo = 0u32;
        let mut k = 0usize;
        for comp in 1..=15u32 {
            for _ in 0..contagem[comp as usize] {
                if comp as usize <= RAPIDO {
                    let s = simbolos[k];
                    let inv = (codigo.reverse_bits() >> (32 - comp)) as usize;
                    let passo = 1 << comp;
                    let mut j = inv;
                    while j < (1 << RAPIDO) {
                        rapido[j] = (s << 4) | comp as u16;
                        j += passo;
                    }
                }
                codigo += 1;
                k += 1;
            }
            codigo <<= 1;
        }
        Some(Huffman { rapido, contagem, simbolos })
    }

    fn ler(&self, b: &mut Bits) -> Option<u16> {
        let v = self.rapido[b.espiar(RAPIDO as u32) as usize];
        if v & 15 != 0 {
            b.pegar((v & 15) as u32);
            return Some(v >> 4);
        }
        // lento: bit a bit (codigos de mais de 9 bits)
        let mut codigo: i32 = 0;
        let mut primeiro: i32 = 0;
        let mut indice: i32 = 0;
        for comp in 1..16 {
            codigo |= b.pegar(1) as i32;
            let n = self.contagem[comp] as i32;
            if codigo - n < primeiro {
                return self.simbolos.get((indice + codigo - primeiro) as usize).copied();
            }
            indice += n;
            primeiro += n;
            primeiro <<= 1;
            codigo <<= 1;
        }
        None
    }
}

fn fixos() -> (Huffman, Huffman) {
    let mut c = [0u8; 288];
    c[..144].fill(8);
    c[144..256].fill(9);
    c[256..280].fill(7);
    c[280..].fill(8);
    (Huffman::novo(&c).unwrap_or_else(|| unreachable!()), Huffman::novo(&[5u8; 30]).unwrap_or_else(|| unreachable!()))
}

/// Descomprime DEFLATE cru. `limite`: tamanho maximo da saida.
pub fn inflar(d: &[u8], limite: usize) -> Option<Vec<u8>> {
    let mut b = Bits { d, i: 0, buf: 0, n: 0 };
    let mut s: Vec<u8> = Vec::with_capacity((d.len() * 4).min(limite));
    loop {
        let ultimo = b.pegar(1);
        let tipo = b.pegar(2);
        match tipo {
            0 => {
                b.alinhar();
                let len = b.pegar(16) as usize;
                let _nlen = b.pegar(16);
                // os bytes estao no fluxo: esvazia o buffer de bits primeiro
                for _ in 0..len {
                    s.push(b.pegar(8) as u8);
                }
            }
            1 | 2 => {
                let (lit, dist) = if tipo == 1 {
                    fixos()
                } else {
                    let hlit = b.pegar(5) as usize + 257;
                    let hdist = b.pegar(5) as usize + 1;
                    let hclen = b.pegar(4) as usize + 4;
                    let mut cl = [0u8; 19];
                    for &o in ORDEM_CL.iter().take(hclen) {
                        cl[o] = b.pegar(3) as u8;
                    }
                    let hcl = Huffman::novo(&cl)?;
                    let mut comps = vec![0u8; hlit + hdist];
                    let mut i = 0;
                    while i < hlit + hdist {
                        let sym = hcl.ler(&mut b)?;
                        match sym {
                            0..=15 => {
                                comps[i] = sym as u8;
                                i += 1;
                            }
                            16 => {
                                let ant = *comps.get(i.checked_sub(1)?)?;
                                for _ in 0..3 + b.pegar(2) {
                                    if i < comps.len() {
                                        comps[i] = ant;
                                        i += 1;
                                    }
                                }
                            }
                            17 => i += 3 + b.pegar(3) as usize,
                            18 => i += 11 + b.pegar(7) as usize,
                            _ => return None,
                        }
                    }
                    comps.truncate(hlit + hdist);
                    (Huffman::novo(&comps[..hlit])?, Huffman::novo(&comps[hlit..])?)
                };
                loop {
                    let sym = lit.ler(&mut b)? as usize;
                    if sym < 256 {
                        s.push(sym as u8);
                    } else if sym == 256 {
                        break;
                    } else {
                        let k = sym - 257;
                        if k >= 29 {
                            return None;
                        }
                        let len = BASE_COMP[k] as usize + b.pegar(EXTRA_COMP[k] as u32) as usize;
                        let dk = dist.ler(&mut b)? as usize;
                        if dk >= 30 {
                            return None;
                        }
                        let distancia = BASE_DIST[dk] as usize + b.pegar(EXTRA_DIST[dk] as u32) as usize;
                        if distancia > s.len() {
                            return None;
                        }
                        let ini = s.len() - distancia;
                        if distancia >= len {
                            s.extend_from_within(ini..ini + len);
                        } else {
                            for j in 0..len {
                                let v = s[ini + j];
                                s.push(v);
                            }
                        }
                    }
                    if s.len() > limite || b.acabou() {
                        return None;
                    }
                }
            }
            _ => return None,
        }
        if ultimo == 1 || b.acabou() {
            break;
        }
    }
    Some(s)
}

/// zlib: 2 bytes de cabecalho + deflate + adler32 (nao conferido).
pub fn zlib(d: &[u8], limite: usize) -> Option<Vec<u8>> {
    if d.len() < 2 || d[0] & 0x0F != 8 || ((d[0] as u16) << 8 | d[1] as u16) % 31 != 0 {
        return None;
    }
    inflar(&d[2..], limite)
}

/// gzip: cabecalho com campos opcionais + deflate.
pub fn gzip(d: &[u8], limite: usize) -> Option<Vec<u8>> {
    if d.len() < 18 || d[0] != 0x1f || d[1] != 0x8b || d[2] != 8 {
        return None;
    }
    let flg = d[3];
    let mut i = 10;
    if flg & 4 != 0 {
        let n = u16::from_le_bytes([*d.get(i)?, *d.get(i + 1)?]) as usize;
        i += 2 + n;
    }
    if flg & 8 != 0 {
        while *d.get(i)? != 0 {
            i += 1;
        }
        i += 1;
    }
    if flg & 16 != 0 {
        while *d.get(i)? != 0 {
            i += 1;
        }
        i += 1;
    }
    if flg & 2 != 0 {
        i += 2;
    }
    inflar(d.get(i..)?, limite)
}
