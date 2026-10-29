//! Verificacao de assinaturas RSA (RFC 8017): PKCS#1 v1.5 (certificados) e
//! PSS (o CertificateVerify do TLS 1.3). So a chave publica: com e = 65537
//! sao 17 multiplicacoes de Montgomery.

use alloc::vec;
use alloc::vec::Vec;

use super::Hash;
use super::grande::{self, MAX_LIMBS, Montgomery};

pub struct ChavePublica<'a> {
    /// Modulo e expoente, bytes big-endian (com ou sem zeros a esquerda).
    pub n: &'a [u8],
    pub e: &'a [u8],
}

fn sem_zeros(b: &[u8]) -> &[u8] {
    let i = b.iter().position(|&x| x != 0).unwrap_or(b.len());
    &b[i..]
}

/// s^e mod n, em `k` bytes (k = tamanho do modulo).
fn abrir(chave: &ChavePublica, assinatura: &[u8]) -> Option<Vec<u8>> {
    let n = sem_zeros(chave.n);
    let k = n.len();
    if !(128..=512).contains(&k) || assinatura.len() != k {
        return None;
    }
    let limbs = k.div_ceil(4);
    let mut m = [0u32; MAX_LIMBS];
    let mut s = [0u32; MAX_LIMBS];
    grande::de_bytes(n, &mut m[..limbs]);
    grande::de_bytes(assinatura, &mut s[..limbs]);
    if grande::maior_igual(&s[..limbs], &m[..limbs]) {
        return None;
    }
    let mont = Montgomery::novo(&m[..limbs])?;
    let mut sm = [0u32; MAX_LIMBS];
    mont.entrar(&s[..limbs], &mut sm[..limbs]);
    let mut r = [0u32; MAX_LIMBS];
    mont.potencia(&sm[..limbs], sem_zeros(chave.e), &mut r[..limbs]);
    let mut em = [0u32; MAX_LIMBS];
    mont.sair(&r[..limbs], &mut em[..limbs]);
    let mut saida = vec![0u8; k];
    grande::para_bytes(&em[..limbs], &mut saida);
    Some(saida)
}

fn prefixo_digest_info(h: Hash) -> &'static [u8] {
    match h {
        Hash::Sha256 => &[0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05, 0x00, 0x04, 0x20],
        Hash::Sha384 => &[0x30, 0x41, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x02, 0x05, 0x00, 0x04, 0x30],
        Hash::Sha512 => &[0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03, 0x05, 0x00, 0x04, 0x40],
    }
}

/// PKCS#1 v1.5: EM = 00 01 FF..FF 00 DigestInfo(hash da mensagem).
pub fn verificar_pkcs1(chave: &ChavePublica, h: Hash, mensagem: &[u8], assinatura: &[u8]) -> bool {
    let Some(em) = abrir(chave, assinatura) else { return false };
    let resumo = h.resumir(mensagem);
    let prefixo = prefixo_digest_info(h);
    let t = prefixo.len() + resumo.len();
    let k = em.len();
    if k < t + 11 {
        return false;
    }
    em[0] == 0
        && em[1] == 1
        && em[2..k - t - 1].iter().all(|&b| b == 0xFF)
        && em[k - t - 1] == 0
        && &em[k - t..k - resumo.len()] == prefixo
        && em[k - resumo.len()..] == resumo[..]
}

fn mgf1(h: Hash, semente: &[u8], tamanho: usize) -> Vec<u8> {
    let mut saida = Vec::with_capacity(tamanho + 64);
    let mut c = 0u32;
    while saida.len() < tamanho {
        let mut d = Vec::with_capacity(semente.len() + 4);
        d.extend_from_slice(semente);
        d.extend_from_slice(&c.to_be_bytes());
        saida.extend_from_slice(&h.resumir(&d));
        c += 1;
    }
    saida.truncate(tamanho);
    saida
}

/// RSASSA-PSS com MGF1 do mesmo hash e sal do tamanho do hash.
pub fn verificar_pss(chave: &ChavePublica, h: Hash, mensagem: &[u8], assinatura: &[u8]) -> bool {
    let Some(em_k) = abrir(chave, assinatura) else { return false };
    let mut nb = [0u32; MAX_LIMBS];
    let n = sem_zeros(chave.n);
    grande::de_bytes(n, &mut nb[..n.len().div_ceil(4)]);
    let mod_bits = grande::bits(&nb[..n.len().div_ceil(4)]);
    let em_bits = mod_bits - 1;
    let em_len = em_bits.div_ceil(8);
    let k = em_k.len();
    // se emLen < k, o byte da frente sobra e tem que ser zero
    if k > em_len && em_k[..k - em_len].iter().any(|&b| b != 0) {
        return false;
    }
    let em = &em_k[k - em_len..];
    let h_len = h.tamanho();
    let s_len = h_len;
    if em_len < h_len + s_len + 2 || em[em_len - 1] != 0xbc {
        return false;
    }
    let mascarado = &em[..em_len - h_len - 1];
    let hh = &em[em_len - h_len - 1..em_len - 1];
    let bits_altos = 8 * em_len - em_bits;
    let mascara_alta = if bits_altos == 0 { 0 } else { 0xFFu8 << (8 - bits_altos) };
    if mascarado[0] & mascara_alta != 0 {
        return false;
    }
    let mut db = mgf1(h, hh, mascarado.len());
    for (d, m) in db.iter_mut().zip(mascarado) {
        *d ^= m;
    }
    db[0] &= !mascara_alta;
    let ps = db.len() - s_len - 1;
    if db[..ps].iter().any(|&b| b != 0) || db[ps] != 1 {
        return false;
    }
    let sal = &db[ps + 1..];
    let mut m2 = vec![0u8; 8];
    m2.extend_from_slice(&h.resumir(mensagem));
    m2.extend_from_slice(sal);
    h.resumir(&m2)[..] == *hh
}
