//! Som: driver virtio-sound (sobre o transporte em crate::virtio) e gerador
//! de tons so com inteiros. Por enquanto so no QEMU virt (na TV box o som
//! sai pelo HDMI/I2S da Amlogic, outro driver).

// Nem todo item publico e usado por quem inclui o modulo.
#![allow(dead_code)]

pub mod gerador;
pub mod virtio_som;
