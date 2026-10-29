//! Virtio: o jeito padrao do QEMU expor dispositivos (som, teclado, mouse...).
//!   - virtio_mmio: acha o dispositivo nos slots e faz a inicializacao
//!   - virtqueue:   fila dividida (descritores, anel disponivel, anel usado)
//! Os drivers (audio, entrada) usam os dois. So existe no QEMU.

// Nem todo item publico e usado pelos drivers.
#![allow(dead_code)]

pub mod virtio_mmio;
pub mod virtqueue;
