//! Numeros grandes para o RSA e as curvas elipticas: limbs de 32 bits
//! (little-endian: limb 0 e o menos significativo), multiplicacao de
//! Montgomery (CIOS), que no ARMv7 vira UMULL/UMAAL. Sem alocacao: tudo em
//! fatias e arrays na pilha (ate MAX_LIMBS = 4096 bits).

pub const MAX_LIMBS: usize = 128;

/// a >= b (mesmo tamanho)?
pub fn maior_igual(a: &[u32], b: &[u32]) -> bool {
    for i in (0..a.len()).rev() {
        if a[i] != b[i] {
            return a[i] > b[i];
        }
    }
    true
}

pub fn zero(a: &[u32]) -> bool {
    a.iter().all(|&x| x == 0)
}

/// a += b; devolve o vai-um.
pub fn somar(a: &mut [u32], b: &[u32]) -> u32 {
    let mut c = 0u64;
    for i in 0..a.len() {
        c += a[i] as u64 + b[i] as u64;
        a[i] = c as u32;
        c >>= 32;
    }
    c as u32
}

/// a -= b; devolve o emprestimo (1 se b > a).
pub fn subtrair(a: &mut [u32], b: &[u32]) -> u32 {
    let mut e = 0i64;
    for i in 0..a.len() {
        e += a[i] as i64 - b[i] as i64;
        a[i] = e as u32;
        e >>= 32;
    }
    (e & 1) as u32
}

/// Bytes big-endian (como vem no DER/TLS) para limbs. Os bytes a mais a
/// esquerda precisam ser zero; devolve false se nao couber.
pub fn de_bytes(bytes: &[u8], saida: &mut [u32]) -> bool {
    saida.fill(0);
    for (k, &b) in bytes.iter().rev().enumerate() {
        if k / 4 >= saida.len() {
            if b != 0 {
                return false;
            }
            continue;
        }
        saida[k / 4] |= (b as u32) << (8 * (k % 4));
    }
    true
}

/// Limbs para bytes big-endian (saida.len() bytes, cortando a esquerda).
pub fn para_bytes(a: &[u32], saida: &mut [u8]) {
    let n = saida.len();
    for (k, b) in saida.iter_mut().rev().enumerate() {
        *b = if k / 4 < a.len() { (a[k / 4] >> (8 * (k % 4))) as u8 } else { 0 };
    }
    let _ = n;
}

/// Bits significativos.
pub fn bits(a: &[u32]) -> usize {
    for i in (0..a.len()).rev() {
        if a[i] != 0 {
            return 32 * i + 32 - a[i].leading_zeros() as usize;
        }
    }
    0
}

/// Aritmetica modulo um numero impar m, na forma de Montgomery (x*R mod m,
/// R = 2^(32n)).
#[derive(Clone)]
pub struct Montgomery {
    pub n: usize,
    pub m: [u32; MAX_LIMBS],
    /// -m^-1 mod 2^32
    m0: u32,
    /// R^2 mod m (para entrar na forma de Montgomery)
    r2: [u32; MAX_LIMBS],
    /// R mod m (o 1 na forma de Montgomery)
    pub um: [u32; MAX_LIMBS],
}

impl Montgomery {
    /// `m` impar, com o limb mais alto diferente de zero.
    pub fn novo(m: &[u32]) -> Option<Self> {
        let n = m.len();
        if n == 0 || n > MAX_LIMBS || m[0] & 1 == 0 || m[n - 1] == 0 {
            return None;
        }
        let mut inv = 1u32;
        for _ in 0..5 {
            inv = inv.wrapping_mul(2u32.wrapping_sub(m[0].wrapping_mul(inv)));
        }
        let mut mm = [0u32; MAX_LIMBS];
        mm[..n].copy_from_slice(m);
        let mut s = Montgomery { n, m: mm, m0: inv.wrapping_neg(), r2: [0; MAX_LIMBS], um: [0; MAX_LIMBS] };
        // R mod m: 1 dobrado 32n vezes; R^2: mais 32n dobras
        let mut x = [0u32; MAX_LIMBS];
        x[0] = 1;
        for k in 0..64 * n {
            s.dobrar(&mut x[..n]);
            if k + 1 == 32 * n {
                s.um[..n].copy_from_slice(&x[..n]);
            }
        }
        s.r2[..n].copy_from_slice(&x[..n]);
        Some(s)
    }

    fn modulo(&self) -> &[u32] {
        &self.m[..self.n]
    }

    /// x = 2x mod m (x < m)
    fn dobrar(&self, x: &mut [u32]) {
        let mut c = 0u32;
        for v in x.iter_mut() {
            let novo = (*v << 1) | c;
            c = *v >> 31;
            *v = novo;
        }
        if c != 0 || maior_igual(x, self.modulo()) {
            subtrair(x, self.modulo());
        }
    }

    /// saida = a*b/R mod m. a, b < m.
    pub fn mul(&self, a: &[u32], b: &[u32], saida: &mut [u32]) {
        let n = self.n;
        let m = self.modulo();
        let mut t = [0u32; MAX_LIMBS + 2];
        for i in 0..n {
            let bi = b[i] as u64;
            let mut c = 0u64;
            for j in 0..n {
                let x = t[j] as u64 + a[j] as u64 * bi + c;
                t[j] = x as u32;
                c = x >> 32;
            }
            let x = t[n] as u64 + c;
            t[n] = x as u32;
            t[n + 1] = (x >> 32) as u32;

            let q = t[0].wrapping_mul(self.m0) as u64;
            let mut c = (t[0] as u64 + q * m[0] as u64) >> 32;
            for j in 1..n {
                let x = t[j] as u64 + q * m[j] as u64 + c;
                t[j - 1] = x as u32;
                c = x >> 32;
            }
            let x = t[n] as u64 + c;
            t[n - 1] = x as u32;
            t[n] = t[n + 1] + (x >> 32) as u32;
        }
        if t[n] != 0 || maior_igual(&t[..n], m) {
            subtrair(&mut t[..n], m);
        }
        saida[..n].copy_from_slice(&t[..n]);
    }

    /// Para a forma de Montgomery (a < m).
    pub fn entrar(&self, a: &[u32], saida: &mut [u32]) {
        let r2 = self.r2;
        self.mul(a, &r2[..self.n], saida);
    }

    /// Da forma de Montgomery para o numero normal.
    pub fn sair(&self, a: &[u32], saida: &mut [u32]) {
        let mut um = [0u32; MAX_LIMBS];
        um[0] = 1;
        self.mul(a, &um[..self.n], saida);
    }

    /// Reduz um numero qualquer de ate n limbs (a < 2^(32n)) para < m.
    /// Serve quando a < 2m (caso das curvas e do hash no ECDSA).
    pub fn reduzir_uma_vez(&self, a: &mut [u32]) {
        if maior_igual(a, self.modulo()) {
            subtrair(a, self.modulo());
        }
    }

    pub fn somar(&self, a: &[u32], b: &[u32], saida: &mut [u32]) {
        let n = self.n;
        saida[..n].copy_from_slice(&a[..n]);
        let c = somar(&mut saida[..n], &b[..n]);
        if c != 0 || maior_igual(&saida[..n], self.modulo()) {
            subtrair(&mut saida[..n], self.modulo());
        }
    }

    pub fn subtrair(&self, a: &[u32], b: &[u32], saida: &mut [u32]) {
        let n = self.n;
        saida[..n].copy_from_slice(&a[..n]);
        if subtrair(&mut saida[..n], &b[..n]) != 0 {
            somar(&mut saida[..n], self.modulo());
        }
    }

    /// base^exp (base na forma de Montgomery, resultado tambem). O
    /// expoente vem em bytes big-endian. Janela de 4 bits.
    pub fn potencia(&self, base: &[u32], exp: &[u8], saida: &mut [u32]) {
        let n = self.n;
        let mut tabela = [[0u32; MAX_LIMBS]; 16];
        tabela[0][..n].copy_from_slice(&self.um[..n]);
        tabela[1][..n].copy_from_slice(&base[..n]);
        for k in 2..16 {
            let (antes, depois) = tabela.split_at_mut(k);
            self.mul(&antes[k - 1][..n], &base[..n], &mut depois[0][..n]);
        }
        let mut r = [0u32; MAX_LIMBS];
        r[..n].copy_from_slice(&self.um[..n]);
        let mut comecou = false;
        for &byte in exp {
            for meio in [byte >> 4, byte & 15] {
                if comecou {
                    for _ in 0..4 {
                        let c = r;
                        self.mul(&c[..n], &c[..n], &mut r[..n]);
                    }
                }
                if meio != 0 {
                    let c = r;
                    self.mul(&c[..n], &tabela[meio as usize][..n], &mut r[..n]);
                    comecou = true;
                }
            }
        }
        saida[..n].copy_from_slice(&r[..n]);
    }
}
