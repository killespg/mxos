//! Video do MXOS.
//!
//!   - fwcfg: acesso ao fw_cfg do QEMU (achar e escrever "arquivos")
//!   - ramfb: framebuffer simples do QEMU, configurado pelo fw_cfg
//!   - tela:  desenho generico num buffer de pixels (pixel, retangulo, texto)
//!            e um console de texto com cursor e rolagem (as fontes ficam
//!            em crate::fonte: Inter e JetBrains Mono)
//!   - mxq:   TV box (Amlogic S905W): reaproveita o framebuffer do U-Boot.
//!            Ainda nao testado no hardware.
//!   - rk322x: TV box Rockchip: o framebuffer do MXOS na janela do VOP.
//!
//! Nenhum modulo aloca memoria: quem chama passa o endereco do buffer.
//! O mapa de memoria e identidade (virtual = fisico), entao o endereco de
//! uma variavel serve como endereco fisico para o dispositivo.

pub mod fwcfg;
pub mod mxq;
pub mod ramfb;
#[cfg(feature = "rk322x")]
pub mod rga;
#[cfg(feature = "rk322x")]
pub mod rk322x;
pub mod tela;
