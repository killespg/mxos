//! X25519 (RFC 7748): troca de chaves Diffie-Hellman na curva 25519. Corpo
//! com 16 limbs de 16 bits em i64 (o desenho do TweetNaCl): curto e sem
//! desvios que dependam da chave secreta.

type Gf = [i64; 16];

const A24: Gf = [0xDB41, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

fn carregar(o: &mut Gf) {
    for i in 0..16 {
        o[i] += 1 << 16;
        let c = o[i] >> 16;
        if i < 15 {
            o[i + 1] += c - 1;
        } else {
            o[0] += 38 * (c - 1);
        }
        o[i] -= c << 16;
    }
}

/// Troca p e q se b == 1 (sem desvio).
fn trocar(p: &mut Gf, q: &mut Gf, b: i64) {
    let c = !(b - 1);
    for i in 0..16 {
        let t = c & (p[i] ^ q[i]);
        p[i] ^= t;
        q[i] ^= t;
    }
}

fn empacotar(n: &Gf) -> [u8; 32] {
    let mut t = *n;
    carregar(&mut t);
    carregar(&mut t);
    carregar(&mut t);
    let mut m: Gf = [0; 16];
    for _ in 0..2 {
        m[0] = t[0] - 0xffed;
        for i in 1..15 {
            m[i] = t[i] - 0xffff - ((m[i - 1] >> 16) & 1);
            m[i - 1] &= 0xffff;
        }
        m[15] = t[15] - 0x7fff - ((m[14] >> 16) & 1);
        let b = (m[15] >> 16) & 1;
        m[14] &= 0xffff;
        trocar(&mut t, &mut m, 1 - b);
    }
    let mut o = [0u8; 32];
    for i in 0..16 {
        o[2 * i] = t[i] as u8;
        o[2 * i + 1] = (t[i] >> 8) as u8;
    }
    o
}

fn desempacotar(n: &[u8; 32]) -> Gf {
    let mut o: Gf = [0; 16];
    for i in 0..16 {
        o[i] = n[2 * i] as i64 + ((n[2 * i + 1] as i64) << 8);
    }
    o[15] &= 0x7fff;
    o
}

fn somar(a: &Gf, b: &Gf) -> Gf {
    core::array::from_fn(|i| a[i] + b[i])
}

fn subtrair(a: &Gf, b: &Gf) -> Gf {
    core::array::from_fn(|i| a[i] - b[i])
}

fn multiplicar(a: &Gf, b: &Gf) -> Gf {
    let mut t = [0i64; 31];
    for i in 0..16 {
        for j in 0..16 {
            t[i + j] += a[i] * b[j];
        }
    }
    for i in 0..15 {
        t[i] += 38 * t[i + 16];
    }
    let mut o: Gf = [0; 16];
    o.copy_from_slice(&t[..16]);
    carregar(&mut o);
    carregar(&mut o);
    o
}

fn quadrado(a: &Gf) -> Gf {
    multiplicar(a, a)
}

fn inverter(i: &Gf) -> Gf {
    let mut c = *i;
    for a in (0..=253).rev() {
        c = quadrado(&c);
        if a != 2 && a != 4 {
            c = multiplicar(&c, i);
        }
    }
    c
}

/// escalar * ponto (coordenada u de 32 bytes).
pub fn multiplicar_ponto(escalar: &[u8; 32], ponto: &[u8; 32]) -> [u8; 32] {
    let mut z = *escalar;
    z[31] = (z[31] & 127) | 64;
    z[0] &= 248;
    let x = desempacotar(ponto);
    let mut a: Gf = [0; 16];
    let mut b = x;
    let mut c: Gf = [0; 16];
    let mut d: Gf = [0; 16];
    a[0] = 1;
    d[0] = 1;
    for i in (0..=254).rev() {
        let r = ((z[i >> 3] >> (i & 7)) & 1) as i64;
        trocar(&mut a, &mut b, r);
        trocar(&mut c, &mut d, r);
        let mut e = somar(&a, &c);
        a = subtrair(&a, &c);
        c = somar(&b, &d);
        b = subtrair(&b, &d);
        d = quadrado(&e);
        let f = quadrado(&a);
        a = multiplicar(&c, &a);
        c = multiplicar(&b, &e);
        e = somar(&a, &c);
        a = subtrair(&a, &c);
        b = quadrado(&a);
        c = subtrair(&d, &f);
        a = multiplicar(&c, &A24);
        a = somar(&a, &d);
        c = multiplicar(&c, &a);
        a = multiplicar(&d, &f);
        d = multiplicar(&b, &x);
        b = quadrado(&e);
        trocar(&mut a, &mut b, r);
        trocar(&mut c, &mut d, r);
    }
    let c = inverter(&c);
    empacotar(&multiplicar(&a, &c))
}

/// Chave publica de uma chave secreta (escalar * 9).
pub fn publica(secreta: &[u8; 32]) -> [u8; 32] {
    let mut base = [0u8; 32];
    base[0] = 9;
    multiplicar_ponto(secreta, &base)
}
