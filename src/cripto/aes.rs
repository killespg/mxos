//! AES-128/256 (FIPS 197), so cifrar (o GCM so precisa disso), e AES-GCM
//! (NIST SP 800-38D) com nonce de 12 bytes. A S-box e calculada em tempo de
//! compilacao (inverso em GF(2^8) + transformacao afim), sem tabela digitada.

const fn calcular_sbox() -> [u8; 256] {
    let mut s = [0u8; 256];
    let mut p: u8 = 1;
    let mut q: u8 = 1;
    loop {
        // p *= 3
        p = p ^ (p << 1) ^ (if p & 0x80 != 0 { 0x1B } else { 0 });
        // q /= 3
        q ^= q << 1;
        q ^= q << 2;
        q ^= q << 4;
        if q & 0x80 != 0 {
            q ^= 0x09;
        }
        let x = q ^ q.rotate_left(1) ^ q.rotate_left(2) ^ q.rotate_left(3) ^ q.rotate_left(4);
        s[p as usize] = x ^ 0x63;
        if p == 1 {
            break;
        }
    }
    s[0] = 0x63;
    s
}

static SBOX: [u8; 256] = calcular_sbox();

fn xtime(x: u8) -> u8 {
    (x << 1) ^ (((x >> 7) & 1) * 0x1B)
}

#[derive(Clone)]
pub struct Aes {
    chaves: [[u8; 16]; 15],
    rodadas: usize,
}

impl Aes {
    /// Chave de 16 (AES-128) ou 32 bytes (AES-256).
    pub fn novo(chave: &[u8]) -> Option<Self> {
        let nk = match chave.len() {
            16 => 4,
            32 => 8,
            _ => return None,
        };
        let rodadas = nk + 6;
        let total = 4 * (rodadas + 1);
        let mut w = [[0u8; 4]; 60];
        for i in 0..nk {
            w[i].copy_from_slice(&chave[4 * i..4 * i + 4]);
        }
        let mut rcon = 1u8;
        for i in nk..total {
            let mut t = w[i - 1];
            if i % nk == 0 {
                t = [SBOX[t[1] as usize] ^ rcon, SBOX[t[2] as usize], SBOX[t[3] as usize], SBOX[t[0] as usize]];
                rcon = xtime(rcon);
            } else if nk > 6 && i % nk == 4 {
                t = [SBOX[t[0] as usize], SBOX[t[1] as usize], SBOX[t[2] as usize], SBOX[t[3] as usize]];
            }
            for j in 0..4 {
                w[i][j] = w[i - nk][j] ^ t[j];
            }
        }
        let mut chaves = [[0u8; 16]; 15];
        for r in 0..=rodadas {
            for c in 0..4 {
                chaves[r][4 * c..4 * c + 4].copy_from_slice(&w[4 * r + c]);
            }
        }
        Some(Aes { chaves, rodadas })
    }

    pub fn cifrar(&self, b: &mut [u8; 16]) {
        somar(b, &self.chaves[0]);
        for r in 1..=self.rodadas {
            // SubBytes + ShiftRows (o estado e coluna a coluna: b[4c + linha])
            let mut t = [0u8; 16];
            for c in 0..4 {
                for l in 0..4 {
                    t[4 * c + l] = SBOX[b[4 * ((c + l) % 4) + l] as usize];
                }
            }
            if r != self.rodadas {
                // MixColumns
                for c in 0..4 {
                    let a = [t[4 * c], t[4 * c + 1], t[4 * c + 2], t[4 * c + 3]];
                    let x = a[0] ^ a[1] ^ a[2] ^ a[3];
                    for l in 0..4 {
                        t[4 * c + l] = a[l] ^ x ^ xtime(a[l] ^ a[(l + 1) % 4]);
                    }
                }
            }
            *b = t;
            somar(b, &self.chaves[r]);
        }
    }
}

fn somar(b: &mut [u8; 16], k: &[u8; 16]) {
    for i in 0..16 {
        b[i] ^= k[i];
    }
}

// ----------------------------------------------------------------- AES-GCM

/// Multiplicacao em GF(2^128) com a ordem de bits do GCM.
fn multiplicar(x: u128, y: u128) -> u128 {
    let mut z = 0u128;
    let mut v = y;
    for i in 0..128 {
        if (x >> (127 - i)) & 1 != 0 {
            z ^= v;
        }
        v = if v & 1 != 0 { (v >> 1) ^ (0xE1u128 << 120) } else { v >> 1 };
    }
    z
}

struct Ghash {
    h: u128,
    y: u128,
}

impl Ghash {
    fn absorver(&mut self, dados: &[u8]) {
        for pedaco in dados.chunks(16) {
            let mut b = [0u8; 16];
            b[..pedaco.len()].copy_from_slice(pedaco);
            self.y = multiplicar(self.y ^ u128::from_be_bytes(b), self.h);
        }
    }
}

#[derive(Clone)]
pub struct AesGcm {
    aes: Aes,
    h: u128,
}

impl AesGcm {
    pub fn novo(chave: &[u8]) -> Option<Self> {
        let aes = Aes::novo(chave)?;
        let mut h = [0u8; 16];
        aes.cifrar(&mut h);
        Some(AesGcm { aes, h: u128::from_be_bytes(h) })
    }

    fn ctr(&self, nonce: &[u8; 12], dados: &mut [u8]) {
        let mut contador = 2u32;
        for pedaco in dados.chunks_mut(16) {
            let mut b = [0u8; 16];
            b[..12].copy_from_slice(nonce);
            b[12..].copy_from_slice(&contador.to_be_bytes());
            self.aes.cifrar(&mut b);
            for (x, k) in pedaco.iter_mut().zip(b) {
                *x ^= k;
            }
            contador = contador.wrapping_add(1);
        }
    }

    fn etiqueta(&self, nonce: &[u8; 12], aad: &[u8], cifrado: &[u8]) -> [u8; 16] {
        let mut g = Ghash { h: self.h, y: 0 };
        g.absorver(aad);
        g.absorver(cifrado);
        let tamanhos = ((aad.len() as u128 * 8) << 64) | (cifrado.len() as u128 * 8);
        g.absorver(&tamanhos.to_be_bytes());
        let mut j0 = [0u8; 16];
        j0[..12].copy_from_slice(nonce);
        j0[15] = 1;
        self.aes.cifrar(&mut j0);
        (g.y ^ u128::from_be_bytes(j0)).to_be_bytes()
    }

    /// Cifra `dados` no lugar e devolve a etiqueta.
    pub fn selar(&self, nonce: &[u8; 12], aad: &[u8], dados: &mut [u8]) -> [u8; 16] {
        self.ctr(nonce, dados);
        self.etiqueta(nonce, aad, dados)
    }

    /// Confere a etiqueta e decifra no lugar; false (e nada decifrado) se
    /// os dados foram mexidos.
    pub fn abrir(&self, nonce: &[u8; 12], aad: &[u8], dados: &mut [u8], etiqueta: &[u8]) -> bool {
        if !super::hmac::iguais(&self.etiqueta(nonce, aad, dados), etiqueta) {
            return false;
        }
        self.ctr(nonce, dados);
        true
    }
}
