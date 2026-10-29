//! Monta o src/cripto e o src/rede/tls.rs do kernel no PC.

extern crate alloc;

#[path = "../../../src/cripto/mod.rs"]
pub mod cripto;

#[path = "../../../src/rede/tls.rs"]
pub mod tls;
