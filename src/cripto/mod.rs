//! Criptografia do MXOS, escrita do zero para o TLS 1.3 do navegador:
//! resumos (SHA-2), HMAC/HKDF, cifras autenticadas (AES-GCM e
//! ChaCha20-Poly1305), troca de chaves (X25519, P-256/P-384), assinaturas
//! (RSA, ECDSA), certificados X.509 e o gerador de numeros aleatorios.
//! Conferida no PC com os vetores oficiais (ferramentas/teste_cripto).

#![allow(dead_code)]

use alloc::vec::Vec;

pub mod acaso;
pub mod aes;
pub mod chacha;
pub mod der;
pub mod ec;
pub mod grande;
pub mod hmac;
pub mod rsa;
pub mod sha2;
pub mod x25519;
pub mod x509;

use sha2::{Sha256, Sha384, Sha512};

/// Os resumos escolhidos em tempo de execucao (pelo certificado ou pela
/// suite do TLS).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hash {
    Sha256,
    Sha384,
    Sha512,
}

impl Hash {
    pub fn tamanho(self) -> usize {
        match self {
            Hash::Sha256 => 32,
            Hash::Sha384 => 48,
            Hash::Sha512 => 64,
        }
    }

    pub fn resumir(self, dados: &[u8]) -> Vec<u8> {
        let mut s = alloc::vec![0u8; self.tamanho()];
        match self {
            Hash::Sha256 => sha2::resumir::<Sha256>(dados, &mut s),
            Hash::Sha384 => sha2::resumir::<Sha384>(dados, &mut s),
            Hash::Sha512 => sha2::resumir::<Sha512>(dados, &mut s),
        }
        s
    }

    pub fn hmac(self, chave: &[u8], dados: &[u8]) -> Vec<u8> {
        let mut s = alloc::vec![0u8; self.tamanho()];
        match self {
            Hash::Sha256 => hmac::hmac::<Sha256>(chave, dados, &mut s),
            Hash::Sha384 => hmac::hmac::<Sha384>(chave, dados, &mut s),
            Hash::Sha512 => hmac::hmac::<Sha512>(chave, dados, &mut s),
        }
        s
    }

    pub fn extrair(self, sal: &[u8], material: &[u8]) -> Vec<u8> {
        match self {
            Hash::Sha256 => hmac::extrair::<Sha256>(sal, material),
            Hash::Sha384 => hmac::extrair::<Sha384>(sal, material),
            Hash::Sha512 => hmac::extrair::<Sha512>(sal, material),
        }
    }

    pub fn expandir(self, prk: &[u8], info: &[u8], tamanho: usize) -> Vec<u8> {
        match self {
            Hash::Sha256 => hmac::expandir::<Sha256>(prk, info, tamanho),
            Hash::Sha384 => hmac::expandir::<Sha384>(prk, info, tamanho),
            Hash::Sha512 => hmac::expandir::<Sha512>(prk, info, tamanho),
        }
    }
}
