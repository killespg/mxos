//! SHA-256, SHA-384 e SHA-512 (FIPS 180-4). Leituras de palavras sempre por
//! from_be_bytes sobre fatias de bytes: nada de converter ponteiro de u8 em
//! ponteiro de u32/u64, que no ARMv7 daria falha de alinhamento.

/// Uma funcao de resumo: o HMAC e o HKDF funcionam com qualquer uma.
pub trait Resumo: Clone {
    /// Tamanho do bloco interno (64 ou 128 bytes).
    const BLOCO: usize;
    /// Tamanho do resumo (32, 48 ou 64 bytes).
    const TAMANHO: usize;
    fn novo() -> Self;
    fn atualizar(&mut self, dados: &[u8]);
    /// Escreve os TAMANHO bytes do resumo no comeco de `saida`.
    fn finalizar(self, saida: &mut [u8]);
}

/// Resumo de uma vez so.
pub fn resumir<R: Resumo>(dados: &[u8], saida: &mut [u8]) {
    let mut r = R::novo();
    r.atualizar(dados);
    r.finalizar(saida);
}

// constantes do SHA-512; as do SHA-256 sao os 32 bits de cima de cada uma
const K512: [u64; 80] = [
    0x428a2f98d728ae22, 0x7137449123ef65cd, 0xb5c0fbcfec4d3b2f, 0xe9b5dba58189dbbc,
    0x3956c25bf348b538, 0x59f111f1b605d019, 0x923f82a4af194f9b, 0xab1c5ed5da6d8118,
    0xd807aa98a3030242, 0x12835b0145706fbe, 0x243185be4ee4b28c, 0x550c7dc3d5ffb4e2,
    0x72be5d74f27b896f, 0x80deb1fe3b1696b1, 0x9bdc06a725c71235, 0xc19bf174cf692694,
    0xe49b69c19ef14ad2, 0xefbe4786384f25e3, 0x0fc19dc68b8cd5b5, 0x240ca1cc77ac9c65,
    0x2de92c6f592b0275, 0x4a7484aa6ea6e483, 0x5cb0a9dcbd41fbd4, 0x76f988da831153b5,
    0x983e5152ee66dfab, 0xa831c66d2db43210, 0xb00327c898fb213f, 0xbf597fc7beef0ee4,
    0xc6e00bf33da88fc2, 0xd5a79147930aa725, 0x06ca6351e003826f, 0x142929670a0e6e70,
    0x27b70a8546d22ffc, 0x2e1b21385c26c926, 0x4d2c6dfc5ac42aed, 0x53380d139d95b3df,
    0x650a73548baf63de, 0x766a0abb3c77b2a8, 0x81c2c92e47edaee6, 0x92722c851482353b,
    0xa2bfe8a14cf10364, 0xa81a664bbc423001, 0xc24b8b70d0f89791, 0xc76c51a30654be30,
    0xd192e819d6ef5218, 0xd69906245565a910, 0xf40e35855771202a, 0x106aa07032bbd1b8,
    0x19a4c116b8d2d0c8, 0x1e376c085141ab53, 0x2748774cdf8eeb99, 0x34b0bcb5e19b48a8,
    0x391c0cb3c5c95a63, 0x4ed8aa4ae3418acb, 0x5b9cca4f7763e373, 0x682e6ff3d6b2b8a3,
    0x748f82ee5defb2fc, 0x78a5636f43172f60, 0x84c87814a1f0ab72, 0x8cc702081a6439ec,
    0x90befffa23631e28, 0xa4506cebde82bde9, 0xbef9a3f7b2c67915, 0xc67178f2e372532b,
    0xca273eceea26619c, 0xd186b8c721c0c207, 0xeada7dd6cde0eb1e, 0xf57d4f7fee6ed178,
    0x06f067aa72176fba, 0x0a637dc5a2c898a6, 0x113f9804bef90dae, 0x1b710b35131c471b,
    0x28db77f523047d84, 0x32caab7b40c72493, 0x3c9ebe0a15c9bebc, 0x431d67c49c100d4c,
    0x4cc5d4becb3e42b6, 0x597f299cfc657e2a, 0x5fcb6fab3ad6faec, 0x6c44198c4a475817,
];

// ------------------------------------------------------------------ SHA-256

#[derive(Clone)]
pub struct Sha256 {
    h: [u32; 8],
    bloco: [u8; 64],
    usado: usize,
    total: u64,
}

fn comprimir256(h: &mut [u32; 8], bloco: &[u8]) {
    let mut w = [0u32; 64];
    for i in 0..16 {
        w[i] = u32::from_be_bytes([bloco[4 * i], bloco[4 * i + 1], bloco[4 * i + 2], bloco[4 * i + 3]]);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ (!e & g);
        let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add((K512[i] >> 32) as u32).wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        hh = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
        *x = x.wrapping_add(y);
    }
}

impl Resumo for Sha256 {
    const BLOCO: usize = 64;
    const TAMANHO: usize = 32;

    fn novo() -> Self {
        Sha256 {
            h: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
            ],
            bloco: [0; 64],
            usado: 0,
            total: 0,
        }
    }

    fn atualizar(&mut self, mut dados: &[u8]) {
        self.total += dados.len() as u64;
        while !dados.is_empty() {
            if self.usado == 0 && dados.len() >= 64 {
                comprimir256(&mut self.h, &dados[..64]);
                dados = &dados[64..];
                continue;
            }
            let n = (64 - self.usado).min(dados.len());
            self.bloco[self.usado..self.usado + n].copy_from_slice(&dados[..n]);
            self.usado += n;
            dados = &dados[n..];
            if self.usado == 64 {
                let bloco = self.bloco;
                comprimir256(&mut self.h, &bloco);
                self.usado = 0;
            }
        }
    }

    fn finalizar(mut self, saida: &mut [u8]) {
        let bits = self.total * 8;
        self.atualizar(&[0x80]);
        while self.usado != 56 {
            self.atualizar(&[0]);
        }
        self.atualizar(&bits.to_be_bytes());
        for (i, p) in self.h.iter().enumerate() {
            saida[4 * i..4 * i + 4].copy_from_slice(&p.to_be_bytes());
        }
    }
}

// ------------------------------------------------------- SHA-512 e SHA-384

#[derive(Clone)]
struct Estado512 {
    h: [u64; 8],
    bloco: [u8; 128],
    usado: usize,
    total: u64,
}

fn comprimir512(h: &mut [u64; 8], bloco: &[u8]) {
    let mut w = [0u64; 80];
    for i in 0..16 {
        let mut b = [0u8; 8];
        b.copy_from_slice(&bloco[8 * i..8 * i + 8]);
        w[i] = u64::from_be_bytes(b);
    }
    for i in 16..80 {
        let s0 = w[i - 15].rotate_right(1) ^ w[i - 15].rotate_right(8) ^ (w[i - 15] >> 7);
        let s1 = w[i - 2].rotate_right(19) ^ w[i - 2].rotate_right(61) ^ (w[i - 2] >> 6);
        w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
    for i in 0..80 {
        let s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
        let ch = (e & f) ^ (!e & g);
        let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K512[i]).wrapping_add(w[i]);
        let s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        hh = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
        *x = x.wrapping_add(y);
    }
}

impl Estado512 {
    fn novo(h: [u64; 8]) -> Self {
        Estado512 { h, bloco: [0; 128], usado: 0, total: 0 }
    }

    fn atualizar(&mut self, mut dados: &[u8]) {
        self.total += dados.len() as u64;
        while !dados.is_empty() {
            if self.usado == 0 && dados.len() >= 128 {
                comprimir512(&mut self.h, &dados[..128]);
                dados = &dados[128..];
                continue;
            }
            let n = (128 - self.usado).min(dados.len());
            self.bloco[self.usado..self.usado + n].copy_from_slice(&dados[..n]);
            self.usado += n;
            dados = &dados[n..];
            if self.usado == 128 {
                let bloco = self.bloco;
                comprimir512(&mut self.h, &bloco);
                self.usado = 0;
            }
        }
    }

    fn finalizar(mut self, saida: &mut [u8], palavras: usize) {
        let bits = (self.total as u128) * 8;
        self.atualizar(&[0x80]);
        while self.usado != 112 {
            self.atualizar(&[0]);
        }
        self.atualizar(&bits.to_be_bytes());
        for (i, p) in self.h.iter().take(palavras).enumerate() {
            saida[8 * i..8 * i + 8].copy_from_slice(&p.to_be_bytes());
        }
    }
}

#[derive(Clone)]
pub struct Sha512(Estado512);

impl Resumo for Sha512 {
    const BLOCO: usize = 128;
    const TAMANHO: usize = 64;

    fn novo() -> Self {
        Sha512(Estado512::novo([
            0x6a09e667f3bcc908, 0xbb67ae8584caa73b, 0x3c6ef372fe94f82b, 0xa54ff53a5f1d36f1,
            0x510e527fade682d1, 0x9b05688c2b3e6c1f, 0x1f83d9abfb41bd6b, 0x5be0cd19137e2179,
        ]))
    }

    fn atualizar(&mut self, dados: &[u8]) {
        self.0.atualizar(dados)
    }

    fn finalizar(self, saida: &mut [u8]) {
        self.0.finalizar(saida, 8)
    }
}

#[derive(Clone)]
pub struct Sha384(Estado512);

impl Resumo for Sha384 {
    const BLOCO: usize = 128;
    const TAMANHO: usize = 48;

    fn novo() -> Self {
        Sha384(Estado512::novo([
            0xcbbb9d5dc1059ed8, 0x629a292a367cd507, 0x9159015a3070dd17, 0x152fecd8f70e5939,
            0x67332667ffc00b31, 0x8eb44a8768581511, 0xdb0c2e0d64f98fa7, 0x47b5481dbefa4fa4,
        ]))
    }

    fn atualizar(&mut self, dados: &[u8]) {
        self.0.atualizar(dados)
    }

    fn finalizar(self, saida: &mut [u8]) {
        self.0.finalizar(saida, 6)
    }
}
