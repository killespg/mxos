//! ChaCha20, Poly1305 e o AEAD ChaCha20-Poly1305 (RFC 8439). So soma, xor e
//! rotacao de 32 bits: e a cifra mais rapida no Cortex-A7, que nao tem
//! instrucoes de AES. O Poly1305 usa limbs de 26 bits (multiplicacoes
//! 32x32->64, boas para o ARMv7).

fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

fn quarto(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(16);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(12);
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(8);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(7);
}

/// Um bloco de 64 bytes do fluxo da ChaCha20.
pub fn bloco(chave: &[u8; 32], contador: u32, nonce: &[u8; 12]) -> [u8; 64] {
    let mut e = [0u32; 16];
    e[0] = 0x61707865;
    e[1] = 0x3320646e;
    e[2] = 0x79622d32;
    e[3] = 0x6b206574;
    for i in 0..8 {
        e[4 + i] = le32(&chave[4 * i..]);
    }
    e[12] = contador;
    for i in 0..3 {
        e[13 + i] = le32(&nonce[4 * i..]);
    }
    let mut s = e;
    for _ in 0..10 {
        quarto(&mut s, 0, 4, 8, 12);
        quarto(&mut s, 1, 5, 9, 13);
        quarto(&mut s, 2, 6, 10, 14);
        quarto(&mut s, 3, 7, 11, 15);
        quarto(&mut s, 0, 5, 10, 15);
        quarto(&mut s, 1, 6, 11, 12);
        quarto(&mut s, 2, 7, 8, 13);
        quarto(&mut s, 3, 4, 9, 14);
    }
    let mut saida = [0u8; 64];
    for i in 0..16 {
        saida[4 * i..4 * i + 4].copy_from_slice(&s[i].wrapping_add(e[i]).to_le_bytes());
    }
    saida
}

/// Cifra (ou decifra) no lugar, a partir do bloco `contador`.
pub fn cifrar(chave: &[u8; 32], mut contador: u32, nonce: &[u8; 12], dados: &mut [u8]) {
    for pedaco in dados.chunks_mut(64) {
        let k = bloco(chave, contador, nonce);
        for (x, y) in pedaco.iter_mut().zip(k) {
            *x ^= y;
        }
        contador = contador.wrapping_add(1);
    }
}

// ---------------------------------------------------------------- Poly1305

pub struct Poly1305 {
    r: [u32; 5],
    s: [u32; 4],
    h: [u32; 5],
    resto: [u8; 16],
    usado: usize,
}

const M26: u32 = 0x3ff_ffff;

impl Poly1305 {
    pub fn novo(chave: &[u8; 32]) -> Self {
        Poly1305 {
            r: [
                le32(&chave[0..]) & 0x3ff_ffff,
                (le32(&chave[3..]) >> 2) & 0x3ff_ff03,
                (le32(&chave[6..]) >> 4) & 0x3ff_c0ff,
                (le32(&chave[9..]) >> 6) & 0x3f0_3fff,
                (le32(&chave[12..]) >> 8) & 0x00f_ffff,
            ],
            s: [le32(&chave[16..]), le32(&chave[20..]), le32(&chave[24..]), le32(&chave[28..])],
            h: [0; 5],
            resto: [0; 16],
            usado: 0,
        }
    }

    fn bloco(&mut self, m: &[u8], bit_alto: u32) {
        let [r0, r1, r2, r3, r4] = self.r.map(|x| x as u64);
        let (s1, s2, s3, s4) = (r1 * 5, r2 * 5, r3 * 5, r4 * 5);
        let h = &mut self.h;
        let h0 = (h[0] + (le32(&m[0..]) & M26)) as u64;
        let h1 = (h[1] + ((le32(&m[3..]) >> 2) & M26)) as u64;
        let h2 = (h[2] + ((le32(&m[6..]) >> 4) & M26)) as u64;
        let h3 = (h[3] + ((le32(&m[9..]) >> 6) & M26)) as u64;
        let h4 = (h[4] + ((le32(&m[12..]) >> 8) | bit_alto)) as u64;

        let d0 = h0 * r0 + h1 * s4 + h2 * s3 + h3 * s2 + h4 * s1;
        let mut d1 = h0 * r1 + h1 * r0 + h2 * s4 + h3 * s3 + h4 * s2;
        let mut d2 = h0 * r2 + h1 * r1 + h2 * r0 + h3 * s4 + h4 * s3;
        let mut d3 = h0 * r3 + h1 * r2 + h2 * r1 + h3 * r0 + h4 * s4;
        let mut d4 = h0 * r4 + h1 * r3 + h2 * r2 + h3 * r1 + h4 * r0;

        let mut c = d0 >> 26;
        h[0] = d0 as u32 & M26;
        d1 += c;
        c = d1 >> 26;
        h[1] = d1 as u32 & M26;
        d2 += c;
        c = d2 >> 26;
        h[2] = d2 as u32 & M26;
        d3 += c;
        c = d3 >> 26;
        h[3] = d3 as u32 & M26;
        d4 += c;
        c = d4 >> 26;
        h[4] = d4 as u32 & M26;
        h[0] += c as u32 * 5;
        let c = h[0] >> 26;
        h[0] &= M26;
        h[1] += c;
    }

    pub fn atualizar(&mut self, mut dados: &[u8]) {
        if self.usado > 0 {
            let n = (16 - self.usado).min(dados.len());
            self.resto[self.usado..self.usado + n].copy_from_slice(&dados[..n]);
            self.usado += n;
            dados = &dados[n..];
            if self.usado < 16 {
                return;
            }
            let b = self.resto;
            self.bloco(&b, 1 << 24);
            self.usado = 0;
        }
        while dados.len() >= 16 {
            self.bloco(&dados[..16], 1 << 24);
            dados = &dados[16..];
        }
        self.resto[..dados.len()].copy_from_slice(dados);
        self.usado = dados.len();
    }

    pub fn finalizar(mut self) -> [u8; 16] {
        if self.usado > 0 {
            let mut b = [0u8; 16];
            b[..self.usado].copy_from_slice(&self.resto[..self.usado]);
            b[self.usado] = 1;
            self.bloco(&b, 0);
        }
        let mut h = self.h;
        let mut c = h[1] >> 26;
        h[1] &= M26;
        for i in 2..5 {
            h[i] += c;
            c = h[i] >> 26;
            h[i] &= M26;
        }
        h[0] += c * 5;
        c = h[0] >> 26;
        h[0] &= M26;
        h[1] += c;

        // g = h + 5 - 2^130: se nao ficou negativo, o resultado e g
        let mut g = [0u32; 5];
        g[0] = h[0] + 5;
        c = g[0] >> 26;
        g[0] &= M26;
        for i in 1..4 {
            g[i] = h[i] + c;
            c = g[i] >> 26;
            g[i] &= M26;
        }
        g[4] = (h[4] + c).wrapping_sub(1 << 26);
        let mascara = (g[4] >> 31).wrapping_sub(1);
        for i in 0..5 {
            h[i] = (h[i] & !mascara) | (g[i] & mascara);
        }

        let p0 = h[0] | (h[1] << 26);
        let p1 = (h[1] >> 6) | (h[2] << 20);
        let p2 = (h[2] >> 12) | (h[3] << 14);
        let p3 = (h[3] >> 18) | (h[4] << 8);
        let mut saida = [0u8; 16];
        let mut f = 0u64;
        for (i, p) in [p0, p1, p2, p3].into_iter().enumerate() {
            f = p as u64 + self.s[i] as u64 + (f >> 32);
            saida[4 * i..4 * i + 4].copy_from_slice(&(f as u32).to_le_bytes());
        }
        saida
    }
}

// ------------------------------------------------------- ChaCha20-Poly1305

fn etiqueta(chave: &[u8; 32], nonce: &[u8; 12], aad: &[u8], cifrado: &[u8]) -> [u8; 16] {
    let mut k = [0u8; 32];
    k.copy_from_slice(&bloco(chave, 0, nonce)[..32]);
    let mut p = Poly1305::novo(&k);
    let zeros = [0u8; 16];
    p.atualizar(aad);
    p.atualizar(&zeros[..(16 - aad.len() % 16) % 16]);
    p.atualizar(cifrado);
    p.atualizar(&zeros[..(16 - cifrado.len() % 16) % 16]);
    p.atualizar(&(aad.len() as u64).to_le_bytes());
    p.atualizar(&(cifrado.len() as u64).to_le_bytes());
    p.finalizar()
}

/// Cifra `dados` no lugar e devolve a etiqueta.
pub fn selar(chave: &[u8; 32], nonce: &[u8; 12], aad: &[u8], dados: &mut [u8]) -> [u8; 16] {
    cifrar(chave, 1, nonce, dados);
    etiqueta(chave, nonce, aad, dados)
}

/// Confere a etiqueta e decifra no lugar.
pub fn abrir(chave: &[u8; 32], nonce: &[u8; 12], aad: &[u8], dados: &mut [u8], marca: &[u8]) -> bool {
    if !super::hmac::iguais(&etiqueta(chave, nonce, aad, dados), marca) {
        return false;
    }
    cifrar(chave, 1, nonce, dados);
    true
}
