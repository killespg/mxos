//! HMAC (RFC 2104) e HKDF (RFC 5869) sobre qualquer Resumo.

use alloc::vec::Vec;

use super::sha2::Resumo;

const MAX_BLOCO: usize = 128;
const MAX_RESUMO: usize = 64;

#[derive(Clone)]
pub struct Hmac<R: Resumo> {
    dentro: R,
    fora: R,
}

impl<R: Resumo> Hmac<R> {
    pub fn novo(chave: &[u8]) -> Self {
        let mut k = [0u8; MAX_BLOCO];
        if chave.len() > R::BLOCO {
            let mut r = R::novo();
            r.atualizar(chave);
            r.finalizar(&mut k);
        } else {
            k[..chave.len()].copy_from_slice(chave);
        }
        let mut ipad = [0u8; MAX_BLOCO];
        let mut opad = [0u8; MAX_BLOCO];
        for i in 0..R::BLOCO {
            ipad[i] = k[i] ^ 0x36;
            opad[i] = k[i] ^ 0x5c;
        }
        let mut dentro = R::novo();
        dentro.atualizar(&ipad[..R::BLOCO]);
        let mut fora = R::novo();
        fora.atualizar(&opad[..R::BLOCO]);
        Hmac { dentro, fora }
    }

    pub fn atualizar(&mut self, dados: &[u8]) {
        self.dentro.atualizar(dados);
    }

    /// Escreve os R::TAMANHO bytes da etiqueta no comeco de `saida`.
    pub fn finalizar(self, saida: &mut [u8]) {
        let mut meio = [0u8; MAX_RESUMO];
        self.dentro.finalizar(&mut meio);
        let mut fora = self.fora;
        fora.atualizar(&meio[..R::TAMANHO]);
        fora.finalizar(saida);
    }
}

/// HMAC de uma vez so.
pub fn hmac<R: Resumo>(chave: &[u8], dados: &[u8], saida: &mut [u8]) {
    let mut h = Hmac::<R>::novo(chave);
    h.atualizar(dados);
    h.finalizar(saida);
}

/// HKDF-Extract: devolve a PRK (R::TAMANHO bytes).
pub fn extrair<R: Resumo>(sal: &[u8], material: &[u8]) -> Vec<u8> {
    let mut prk = alloc::vec![0u8; R::TAMANHO];
    let zeros = [0u8; MAX_RESUMO];
    let sal = if sal.is_empty() { &zeros[..R::TAMANHO] } else { sal };
    hmac::<R>(sal, material, &mut prk);
    prk
}

/// HKDF-Expand: `tamanho` bytes a partir da PRK e da informacao.
pub fn expandir<R: Resumo>(prk: &[u8], info: &[u8], tamanho: usize) -> Vec<u8> {
    let mut saida = Vec::with_capacity(tamanho);
    let mut anterior = [0u8; MAX_RESUMO];
    let mut tem_anterior = false;
    let mut contador = 1u8;
    while saida.len() < tamanho {
        let mut h = Hmac::<R>::novo(prk);
        if tem_anterior {
            h.atualizar(&anterior[..R::TAMANHO]);
        }
        h.atualizar(info);
        h.atualizar(&[contador]);
        h.finalizar(&mut anterior);
        tem_anterior = true;
        let n = (tamanho - saida.len()).min(R::TAMANHO);
        saida.extend_from_slice(&anterior[..n]);
        contador = contador.wrapping_add(1);
    }
    saida
}

/// Compara sem parar no primeiro byte diferente (o tempo nao revela onde).
pub fn iguais(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut d = 0u8;
    for (x, y) in a.iter().zip(b) {
        d |= x ^ y;
    }
    d == 0
}
