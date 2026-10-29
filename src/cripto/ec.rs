//! Curvas NIST P-256 e P-384 (y^2 = x^3 - 3x + b): verificacao ECDSA dos
//! certificados e do TLS, e ECDH (para servidores sem X25519). Pontos em
//! coordenadas jacobianas, corpo na forma de Montgomery (grande.rs).
//! Limbs fixos em 12 (o tamanho da P-384); a P-256 usa os 8 primeiros.

use alloc::vec::Vec;

use super::Hash;
use super::grande::{self, MAX_LIMBS, Montgomery};

const L: usize = 12;
type Num = [u32; L];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Curva {
    P256,
    P384,
}

struct Parametros {
    bytes: usize,
    p: &'static str,
    n: &'static str,
    b: &'static str,
    gx: &'static str,
    gy: &'static str,
}

const P256: Parametros = Parametros {
    bytes: 32,
    p: "ffffffff00000001000000000000000000000000ffffffffffffffffffffffff",
    n: "ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551",
    b: "5ac635d8aa3a93e7b3ebbd55769886bc651d06b0cc53b0f63bce3c3e27d2604b",
    gx: "6b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296",
    gy: "4fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5",
};

const P384: Parametros = Parametros {
    bytes: 48,
    p: "fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffeffffffff0000000000000000ffffffff",
    n: "ffffffffffffffffffffffffffffffffffffffffffffffffc7634d81f4372ddf581a0db248b0a77aecec196accc52973",
    b: "b3312fa7e23ee7e4988e056be3f82d19181d9c6efe8141120314088f5013875ac656398d8a2ed19d2a85c8edd3ec2aef",
    gx: "aa87ca22be8b05378eb1c71ef320ad746e1d3b628ba79b9859f741e082542a385502f25dbf55296c3a545e3872760ab7",
    gy: "3617de4a96262c6f5d9e98bf9292dc29f8f41dbd289a147ce9da3113b5f0b8c00a60b1ce1d7e819d7a431d7c90ea0e5f",
};

fn de_hexa(s: &str) -> Num {
    let mut bytes = [0u8; 4 * L];
    let n = s.len() / 2;
    for i in 0..n {
        bytes[4 * L - n + i] = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap_or(0);
    }
    let mut r = [0u32; L];
    grande::de_bytes(&bytes, &mut r);
    r
}

/// Tudo que uma curva precisa, ja na forma de Montgomery.
pub struct Contexto {
    pub curva: Curva,
    pub bytes: usize,
    k: usize,
    fp: Montgomery,
    fnn: Montgomery,
    b: Num,
    g: Ponto,
    tres: Num,
}

#[derive(Clone, Copy)]
struct Ponto {
    x: Num,
    y: Num,
    z: Num,
}

impl Contexto {
    pub fn novo(curva: Curva) -> Contexto {
        let par = match curva {
            Curva::P256 => &P256,
            Curva::P384 => &P384,
        };
        let k = par.bytes / 4;
        let fp = Montgomery::novo(&de_hexa(par.p)[..k]).expect("p");
        let fnn = Montgomery::novo(&de_hexa(par.n)[..k]).expect("n");
        let mut c = Contexto {
            curva,
            bytes: par.bytes,
            k,
            fp,
            fnn,
            b: [0; L],
            g: Ponto { x: [0; L], y: [0; L], z: [0; L] },
            tres: [0; L],
        };
        c.b = c.entrar(&de_hexa(par.b));
        let mut tres = [0u32; L];
        tres[0] = 3;
        c.tres = c.entrar(&tres);
        c.g = Ponto { x: c.entrar(&de_hexa(par.gx)), y: c.entrar(&de_hexa(par.gy)), z: c.um() };
        c
    }

    // ------------------------------------------------------- corpo (mod p)

    fn entrar(&self, a: &Num) -> Num {
        let mut r = [0u32; L];
        self.fp.entrar(&a[..self.k], &mut r[..self.k]);
        r
    }

    fn sair(&self, a: &Num) -> Num {
        let mut r = [0u32; L];
        self.fp.sair(&a[..self.k], &mut r[..self.k]);
        r
    }

    fn um(&self) -> Num {
        let mut r = [0u32; L];
        r[..self.k].copy_from_slice(&self.fp.um[..self.k]);
        r
    }

    fn mul(&self, a: &Num, b: &Num) -> Num {
        let mut r = [0u32; L];
        self.fp.mul(&a[..self.k], &b[..self.k], &mut r[..self.k]);
        r
    }

    fn sq(&self, a: &Num) -> Num {
        self.mul(a, a)
    }

    fn add(&self, a: &Num, b: &Num) -> Num {
        let mut r = [0u32; L];
        self.fp.somar(&a[..self.k], &b[..self.k], &mut r[..self.k]);
        r
    }

    fn sub(&self, a: &Num, b: &Num) -> Num {
        let mut r = [0u32; L];
        self.fp.subtrair(&a[..self.k], &b[..self.k], &mut r[..self.k]);
        r
    }

    fn inv(&self, a: &Num) -> Num {
        // a^(p-2)
        let mut p = [0u32; L];
        p[..self.k].copy_from_slice(&self.fp.m[..self.k]);
        let mut dois = [0u32; L];
        dois[0] = 2;
        grande::subtrair(&mut p[..self.k], &dois[..self.k]);
        let mut e = [0u8; 4 * L];
        grande::para_bytes(&p[..self.k], &mut e[..self.bytes]);
        let mut r = [0u32; L];
        self.fp.potencia(&a[..self.k], &e[..self.bytes], &mut r[..self.k]);
        r
    }

    fn eh_zero(&self, a: &Num) -> bool {
        grande::zero(&a[..self.k])
    }

    // ------------------------------------------------------------ pontos

    fn infinito(&self) -> Ponto {
        Ponto { x: self.um(), y: self.um(), z: [0; L] }
    }

    fn dobrar(&self, p: &Ponto) -> Ponto {
        if self.eh_zero(&p.z) || self.eh_zero(&p.y) {
            return self.infinito();
        }
        // dbl-2001-b (a = -3)
        let delta = self.sq(&p.z);
        let gamma = self.sq(&p.y);
        let beta = self.mul(&p.x, &gamma);
        let alfa = self.mul(&self.tres, &self.mul(&self.sub(&p.x, &delta), &self.add(&p.x, &delta)));
        let beta4 = self.add(&self.add(&beta, &beta), &self.add(&beta, &beta));
        let beta8 = self.add(&beta4, &beta4);
        let x3 = self.sub(&self.sq(&alfa), &beta8);
        let z3 = self.sub(&self.sub(&self.sq(&self.add(&p.y, &p.z)), &gamma), &delta);
        let g2 = self.sq(&gamma);
        let g4 = self.add(&g2, &g2);
        let g8 = self.add(&g4, &g4);
        let y3 = self.sub(&self.mul(&alfa, &self.sub(&beta4, &x3)), &self.add(&g8, &g8));
        Ponto { x: x3, y: y3, z: z3 }
    }

    fn somar(&self, p: &Ponto, q: &Ponto) -> Ponto {
        if self.eh_zero(&p.z) {
            return *q;
        }
        if self.eh_zero(&q.z) {
            return *p;
        }
        // add-2007-bl
        let z1z1 = self.sq(&p.z);
        let z2z2 = self.sq(&q.z);
        let u1 = self.mul(&p.x, &z2z2);
        let u2 = self.mul(&q.x, &z1z1);
        let s1 = self.mul(&self.mul(&p.y, &q.z), &z2z2);
        let s2 = self.mul(&self.mul(&q.y, &p.z), &z1z1);
        let h = self.sub(&u2, &u1);
        let r0 = self.sub(&s2, &s1);
        if self.eh_zero(&h) {
            return if self.eh_zero(&r0) { self.dobrar(p) } else { self.infinito() };
        }
        let h2 = self.add(&h, &h);
        let i = self.sq(&h2);
        let j = self.mul(&h, &i);
        let r = self.add(&r0, &r0);
        let v = self.mul(&u1, &i);
        let x3 = self.sub(&self.sub(&self.sq(&r), &j), &self.add(&v, &v));
        let s1j = self.mul(&s1, &j);
        let y3 = self.sub(&self.mul(&r, &self.sub(&v, &x3)), &self.add(&s1j, &s1j));
        let z3 = self.mul(&self.sub(&self.sub(&self.sq(&self.add(&p.z, &q.z)), &z1z1), &z2z2), &h);
        Ponto { x: x3, y: y3, z: z3 }
    }

    /// (x, y) normais (fora de Montgomery); None no infinito.
    fn afim(&self, p: &Ponto) -> Option<(Num, Num)> {
        if self.eh_zero(&p.z) {
            return None;
        }
        let zi = self.inv(&p.z);
        let zi2 = self.sq(&zi);
        Some((self.sair(&self.mul(&p.x, &zi2)), self.sair(&self.mul(&self.mul(&p.y, &zi2), &zi))))
    }

    /// Le um numero de `bytes` bytes que precisa ser < p.
    fn ler_coordenada(&self, b: &[u8]) -> Option<Num> {
        let mut r = [0u32; L];
        if b.len() != self.bytes || !grande::de_bytes(b, &mut r[..self.k]) {
            return None;
        }
        (!grande::maior_igual(&r[..self.k], &self.fp.m[..self.k])).then_some(r)
    }

    /// Ponto "04 || X || Y" (sem compressao), conferindo que esta na curva.
    fn ler_ponto(&self, b: &[u8]) -> Option<Ponto> {
        if b.len() != 1 + 2 * self.bytes || b[0] != 4 {
            return None;
        }
        let x = self.entrar(&self.ler_coordenada(&b[1..1 + self.bytes])?);
        let y = self.entrar(&self.ler_coordenada(&b[1 + self.bytes..])?);
        // y^2 == x^3 - 3x + b
        let dir = self.add(&self.sub(&self.mul(&self.sq(&x), &x), &self.mul(&self.tres, &x)), &self.b);
        if self.sq(&y)[..self.k] != dir[..self.k] {
            return None;
        }
        Some(Ponto { x, y, z: self.um() })
    }

    /// escalar * P (escalar em bytes big-endian), da esquerda para a direita.
    fn multiplicar(&self, p: &Ponto, escalar: &[u8]) -> Ponto {
        let mut r = self.infinito();
        for &byte in escalar {
            for bit in (0..8).rev() {
                r = self.dobrar(&r);
                if (byte >> bit) & 1 != 0 {
                    r = self.somar(&r, p);
                }
            }
        }
        r
    }

    // ------------------------------------------------------ escalares (mod n)

    /// Hash truncado aos bits de n e reduzido.
    fn escalar_do_hash(&self, h: &[u8]) -> Num {
        let h = &h[..h.len().min(self.bytes)];
        let mut b = [0u8; 4 * L];
        b[self.bytes - h.len()..self.bytes].copy_from_slice(h);
        let mut e = [0u32; L];
        grande::de_bytes(&b[..self.bytes], &mut e[..self.k]);
        self.fnn.reduzir_uma_vez(&mut e[..self.k]);
        e
    }

    // ------------------------------------------------------------- uso

    /// Verifica uma assinatura ECDSA (r, s em bytes big-endian) do hash.
    pub fn verificar(&self, publica: &[u8], resumo: &[u8], r: &[u8], s: &[u8]) -> bool {
        let k = self.k;
        let Some(q) = self.ler_ponto(publica) else { return false };
        let ler = |b: &[u8]| -> Option<Num> {
            let mut x = [0u32; L];
            if !grande::de_bytes(b, &mut x[..k]) || grande::zero(&x[..k]) || grande::maior_igual(&x[..k], &self.fnn.m[..k]) {
                return None;
            }
            Some(x)
        };
        let (Some(rn), Some(sn)) = (ler(r), ler(s)) else { return false };
        let e = self.escalar_do_hash(resumo);
        // w = s^-1 mod n; u1 = e*w; u2 = r*w
        let mut sm = [0u32; L];
        self.fnn.entrar(&sn[..k], &mut sm[..k]);
        let mut n2 = [0u32; L];
        n2[..k].copy_from_slice(&self.fnn.m[..k]);
        let mut dois = [0u32; L];
        dois[0] = 2;
        grande::subtrair(&mut n2[..k], &dois[..k]);
        let mut exp = [0u8; 4 * L];
        grande::para_bytes(&n2[..k], &mut exp[..self.bytes]);
        let mut w = [0u32; L];
        self.fnn.potencia(&sm[..k], &exp[..self.bytes], &mut w[..k]);
        // w esta em Montgomery: e*w/R = e*s^-1 (e e r entram normais)
        let mut u1 = [0u32; L];
        let mut u2 = [0u32; L];
        self.fnn.mul(&e[..k], &w[..k], &mut u1[..k]);
        self.fnn.mul(&rn[..k], &w[..k], &mut u2[..k]);
        let mut b1 = [0u8; 4 * L];
        let mut b2 = [0u8; 4 * L];
        grande::para_bytes(&u1[..k], &mut b1[..self.bytes]);
        grande::para_bytes(&u2[..k], &mut b2[..self.bytes]);

        // u1*G + u2*Q de uma vez (truque de Shamir)
        let gq = self.somar(&self.g, &q);
        let mut acc = self.infinito();
        for i in 0..self.bytes {
            for bit in (0..8).rev() {
                acc = self.dobrar(&acc);
                match ((b1[i] >> bit) & 1, (b2[i] >> bit) & 1) {
                    (1, 1) => acc = self.somar(&acc, &gq),
                    (1, 0) => acc = self.somar(&acc, &self.g),
                    (0, 1) => acc = self.somar(&acc, &q),
                    _ => {}
                }
            }
        }
        let Some((mut x, _)) = self.afim(&acc) else { return false };
        self.fnn.reduzir_uma_vez(&mut x[..k]);
        x[..k] == rn[..k]
    }

    /// Chave publica "04||X||Y" de uma secreta (bytes big-endian, < n).
    pub fn publica(&self, secreta: &[u8]) -> Option<Vec<u8>> {
        let (x, y) = self.afim(&self.multiplicar(&self.g, secreta))?;
        Some(self.codificar(&x, &y))
    }

    fn codificar(&self, x: &Num, y: &Num) -> Vec<u8> {
        let mut v = alloc::vec![0u8; 1 + 2 * self.bytes];
        v[0] = 4;
        grande::para_bytes(&x[..self.k], &mut v[1..1 + self.bytes]);
        grande::para_bytes(&y[..self.k], &mut v[1 + self.bytes..]);
        v
    }

    /// ECDH: X de secreta * publica_do_outro.
    pub fn segredo(&self, secreta: &[u8], publica: &[u8]) -> Option<Vec<u8>> {
        let q = self.ler_ponto(publica)?;
        let (x, _) = self.afim(&self.multiplicar(&q, secreta))?;
        let mut v = alloc::vec![0u8; self.bytes];
        grande::para_bytes(&x[..self.k], &mut v);
        Some(v)
    }

    /// Para os testes: G esta na curva e n*G e o infinito.
    pub fn conferir(&self) -> bool {
        let g = self.codificar(&self.sair(&self.g.x), &self.sair(&self.g.y));
        let mut nb = [0u8; 4 * L];
        grande::para_bytes(&self.fnn.m[..self.k], &mut nb[..self.bytes]);
        self.ler_ponto(&g).is_some() && self.eh_zero(&self.multiplicar(&self.g, &nb[..self.bytes]).z)
    }

    /// Verifica com o hash da mensagem.
    pub fn verificar_mensagem(&self, publica: &[u8], h: Hash, mensagem: &[u8], r: &[u8], s: &[u8]) -> bool {
        self.verificar(publica, &h.resumir(mensagem), r, s)
    }
}

const _: () = assert!(MAX_LIMBS >= L);
