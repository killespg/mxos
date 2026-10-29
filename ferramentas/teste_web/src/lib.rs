//! Monta o src/web do kernel no PC (e a cripto + TLS para baixar paginas
//! de verdade nos testes).

extern crate alloc;

#[path = "../../../src/web/mod.rs"]
pub mod web;

#[path = "../../../src/cripto/mod.rs"]
pub mod cripto;

#[path = "../../../src/rede/tls.rs"]
pub mod tls;

pub mod rede;
