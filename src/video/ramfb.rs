//! ramfb do QEMU ("-device ramfb"): um framebuffer que mora na RAM do
//! convidado. O kernel reserva o buffer e conta ao QEMU onde ele esta
//! escrevendo 28 bytes no arquivo "etc/ramfb" do fw_cfg (struct RAMFBCfg
//! de hw/display/ramfb.c, toda big endian):
//!   u64 endereco, u32 fourcc, u32 flags, u32 largura, u32 altura, u32 passo
//!
//! Depois disso o QEMU le o buffer direto a cada atualizacao da tela: o que
//! for escrito nele aparece sem configurar de novo. O buffer nunca pode ser
//! liberado nem reaproveitado enquanto o QEMU estiver mostrando.
//!
//! Cuidado: uma configuracao invalida e ignorada em silencio (o DMA da
//! certo e a tela fica como estava). Por isso validar() confere antes.

#![allow(dead_code)]

use super::fwcfg::{self, FwCfg};

pub const NOME_ARQUIVO: &[u8] = b"etc/ramfb";
/// DRM_FORMAT_XRGB8888 ('X','R','2','4'): pixel u32 0x00RRGGBB.
pub const XRGB8888: u32 = 0x3432_5258;
pub const BYTES_POR_PIXEL: u32 = 4;
const TAMANHO_CONFIG: usize = 28;

// Limites do QEMU (ramfb_create_display_surface / bochs-vbe.h).
pub const MIN_LADO: u32 = 16;
pub const MAX_LARGURA: u32 = 16000;
pub const MAX_ALTURA: u32 = 12000;

/// Modo de video pedido ao QEMU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Modo {
    /// Endereco fisico do buffer (na RAM, contiguo).
    pub endereco: u64,
    pub largura: u32,
    pub altura: u32,
    /// Bytes entre o comeco de uma linha e o da proxima.
    pub passo: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Erro {
    FwCfg(fwcfg::Erro),
    /// Largura ou altura fora dos limites do QEMU.
    Tamanho,
    /// Passo menor que a linha, endereco nulo ou desalinhado.
    Buffer,
    /// "etc/ramfb" existe mas nao tem 28 bytes.
    TamanhoArquivo(u32),
    /// A leitura de volta nao bateu com o que foi escrito.
    NaoConfirmou,
}

impl Erro {
    pub fn descricao(self) -> &'static str {
        match self {
            Erro::FwCfg(e) => e.descricao(),
            Erro::Tamanho => "largura/altura fora dos limites",
            Erro::Buffer => "buffer inválido",
            Erro::TamanhoArquivo(_) => "etc/ramfb com tamanho inesperado",
            Erro::NaoConfirmou => "configuração não confirmada",
        }
    }
}

impl From<fwcfg::Erro> for Erro {
    fn from(e: fwcfg::Erro) -> Self {
        Erro::FwCfg(e)
    }
}

/// Modo XRGB8888 sem espaco no fim da linha (passo = largura * 4).
pub const fn modo_xrgb(endereco: u64, largura: u32, altura: u32) -> Modo {
    Modo { endereco, largura, altura, passo: largura * BYTES_POR_PIXEL }
}

/// Quantos bytes o buffer do modo ocupa.
pub const fn bytes_necessarios(modo: &Modo) -> usize {
    modo.passo as usize * modo.altura as usize
}

/// Confere o modo com as mesmas regras do QEMU (que nao avisa se recusar).
pub fn validar(modo: &Modo) -> Result<(), Erro> {
    let largura_ok = (MIN_LADO..=MAX_LARGURA).contains(&modo.largura);
    let altura_ok = (MIN_LADO..=MAX_ALTURA).contains(&modo.altura);
    if !largura_ok || !altura_ok {
        return Err(Erro::Tamanho);
    }
    let linha = modo.largura as u64 * BYTES_POR_PIXEL as u64;
    if modo.endereco == 0 || modo.endereco % 4 != 0 || (modo.passo as u64) < linha || modo.passo % 4 != 0 {
        return Err(Erro::Buffer);
    }
    Ok(())
}

/// Os 28 bytes da RAMFBCfg, em big endian, sem struct packed.
pub fn serializar(modo: &Modo) -> [u8; TAMANHO_CONFIG] {
    let mut bytes = [0u8; TAMANHO_CONFIG];
    bytes[0..8].copy_from_slice(&modo.endereco.to_be_bytes());
    bytes[8..12].copy_from_slice(&XRGB8888.to_be_bytes());
    bytes[12..16].copy_from_slice(&0u32.to_be_bytes()); // flags: sem uso
    bytes[16..20].copy_from_slice(&modo.largura.to_be_bytes());
    bytes[20..24].copy_from_slice(&modo.altura.to_be_bytes());
    bytes[24..28].copy_from_slice(&modo.passo.to_be_bytes());
    bytes
}

/// Liga o ramfb no modo pedido. O buffer ja pode ter conteudo: o QEMU
/// mostra o que estiver la. Devolve o seletor usado (para diagnostico).
pub fn configurar(fw: &FwCfg, modo: &Modo) -> Result<fwcfg::Arquivo, Erro> {
    validar(modo)?;
    let arquivo = fw.achar(NOME_ARQUIVO)?;
    if arquivo.tamanho as usize != TAMANHO_CONFIG {
        return Err(Erro::TamanhoArquivo(arquivo.tamanho));
    }
    let config = serializar(modo);
    fw.escrever(arquivo, &config)?;
    confirmar(fw, arquivo, &config)?;
    Ok(arquivo)
}

/// Le "etc/ramfb" de volta e compara com o que foi escrito.
fn confirmar(fw: &FwCfg, arquivo: fwcfg::Arquivo, config: &[u8; TAMANHO_CONFIG]) -> Result<(), Erro> {
    let mut lido = [0u8; TAMANHO_CONFIG];
    fw.ler_dma(arquivo, &mut lido)?;
    if &lido != config {
        return Err(Erro::NaoConfirmou);
    }
    Ok(())
}
