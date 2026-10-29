//! Criptografia do MXOS, escrita do zero para o TLS 1.3 do navegador:
//! resumos (SHA-2), HMAC/HKDF, cifras autenticadas (AES-GCM e
//! ChaCha20-Poly1305) e troca de chaves (X25519). Conferida no PC com os
//! vetores oficiais (ferramentas/teste_cripto).

#![allow(dead_code)]

pub mod aes;
pub mod chacha;
pub mod hmac;
pub mod sha2;
pub mod x25519;
