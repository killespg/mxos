//! O motor web do MXOS, escrito do zero: HTML5, CSS, layout, pintura,
//! fontes, imagens e JavaScript. So usa `core` e `alloc` (nada do kernel),
//! entao roda igual no PC (ferramentas/teste_web) e na TV box.
//!
//! Regras de desempenho (a box e um Cortex-A7 de 32 bits):
//! - nomes viram atomos (u32), o DOM e uma arena com indices;
//! - geometria em ponto fixo (1/64 de pixel), sem float no layout;
//! - nada de ler u32 de ponteiros desalinhados (from_le/be_bytes).

#![allow(dead_code)]

pub mod atomo;
pub mod css;
pub mod dom;
pub mod html;
pub mod url;
