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

/// H vezes cada valor de 4 bits em GF(2^128) (ordem de bits do GCM), em
/// metades de 64 bits: o GHASH anda de 4 em 4 bits com uma consulta e um
/// deslocamento, em vez de 128 voltas por bloco (que em 32 bits, com u128
/// emulado, pesavam meio segundo por megabyte baixado).
#[derive(Clone)]
struct TabelaH {
    alto: [u64; 16],
    baixo: [u64; 16],
}

/// A reducao dos 4 bits que saem pela direita a cada passo.
const RESTO4: [u64; 16] = [
    0x0000, 0x1C20, 0x3840, 0x2460, 0x7080, 0x6CA0, 0x48C0, 0x54E0, 0xE100, 0xFD20, 0xD940, 0xC560, 0x9180, 0x8DA0, 0xA9C0,
    0xB5E0,
];

impl TabelaH {
    fn nova(h: &[u8; 16]) -> TabelaH {
        let mut t = TabelaH { alto: [0; 16], baixo: [0; 16] };
        let mut vh = u64::from_be_bytes(h[..8].try_into().unwrap());
        let mut vl = u64::from_be_bytes(h[8..].try_into().unwrap());
        t.alto[8] = vh;
        t.baixo[8] = vl;
        // 4, 2, 1: H vezes x, x^2, x^3 (cada um, metade do anterior com reducao)
        let mut i = 4;
        while i > 0 {
            let reduz = if vl & 1 != 0 { 0xE100_0000_0000_0000u64 } else { 0 };
            vl = (vh << 63) | (vl >> 1);
            vh = (vh >> 1) ^ reduz;
            t.alto[i] = vh;
            t.baixo[i] = vl;
            i >>= 1;
        }
        // o resto e soma (xor) das potencias
        for i in [2usize, 4, 8] {
            let (ah, al) = (t.alto[i], t.baixo[i]);
            for j in 1..i {
                t.alto[i + j] = ah ^ t.alto[j];
                t.baixo[i + j] = al ^ t.baixo[j];
            }
        }
        t
    }

    /// x vezes H.
    fn multiplicar(&self, x: &[u8; 16]) -> [u8; 16] {
        let lo = (x[15] & 0xF) as usize;
        let (mut zh, mut zl) = (self.alto[lo], self.baixo[lo]);
        for i in (0..16).rev() {
            let lo = (x[i] & 0xF) as usize;
            let hi = (x[i] >> 4) as usize;
            if i != 15 {
                let resto = (zl & 0xF) as usize;
                zl = (zh << 60) | (zl >> 4);
                zh = (zh >> 4) ^ (RESTO4[resto] << 48);
                zh ^= self.alto[lo];
                zl ^= self.baixo[lo];
            }
            let resto = (zl & 0xF) as usize;
            zl = (zh << 60) | (zl >> 4);
            zh = (zh >> 4) ^ (RESTO4[resto] << 48);
            zh ^= self.alto[hi];
            zl ^= self.baixo[hi];
        }
        let mut z = [0u8; 16];
        z[..8].copy_from_slice(&zh.to_be_bytes());
        z[8..].copy_from_slice(&zl.to_be_bytes());
        z
    }
}

struct Ghash<'a> {
    h: &'a TabelaH,
    y: [u8; 16],
}

impl Ghash<'_> {
    fn absorver(&mut self, dados: &[u8]) {
        for pedaco in dados.chunks(16) {
            for (y, b) in self.y.iter_mut().zip(pedaco) {
                *y ^= b;
            }
            self.y = self.h.multiplicar(&self.y);
        }
    }
}

#[derive(Clone)]
pub struct AesGcm {
    aes: Aes,
    h: TabelaH,
}

impl AesGcm {
    pub fn novo(chave: &[u8]) -> Option<Self> {
        let aes = Aes::novo(chave)?;
        let mut h = [0u8; 16];
        aes.cifrar(&mut h);
        Some(AesGcm { aes, h: TabelaH::nova(&h) })
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
        let mut g = Ghash { h: &self.h, y: [0; 16] };
        g.absorver(aad);
        g.absorver(cifrado);
        let tamanhos = ((aad.len() as u128 * 8) << 64) | (cifrado.len() as u128 * 8);
        g.absorver(&tamanhos.to_be_bytes());
        let mut j0 = [0u8; 16];
        j0[..12].copy_from_slice(nonce);
        j0[15] = 1;
        self.aes.cifrar(&mut j0);
        let mut t = g.y;
        for (a, b) in t.iter_mut().zip(j0) {
            *a ^= b;
        }
        t
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
