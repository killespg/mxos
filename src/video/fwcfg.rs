//! fw_cfg do QEMU: uma lista de "arquivos" que a maquina virtual mostra
//! para o convidado. O MXOS usa para achar e escrever "etc/ramfb".
//!
//! Registradores no QEMU virt (MMIO, todos big endian):
//!   +0x00 dados   : cada leitura devolve o proximo byte do item escolhido
//!   +0x08 seletor : 16 bits, so escrita, escolhe o item (e volta ao byte 0)
//!   +0x10 DMA     : 64 bits, endereco fisico de um AcessoDma; escrever inicia
//!
//! Toda estrutura do fw_cfg e big endian: sempre .to_be() / from_be().
//! So existe no QEMU; a TV box nao tem fw_cfg.

#![allow(dead_code)]

use core::arch::asm;
use core::ptr::{read_volatile, write_volatile};

/// Endereco no QEMU virt (hw/arm/virt.c, VIRT_FW_CFG).
pub const BASE_QEMU_VIRT: usize = 0x0902_0000;

// Deslocamentos dos registradores.
const DADOS: usize = 0x00;
const SELETOR: usize = 0x08;
const DMA: usize = 0x10;

// Chaves fixas (qemu_fw_cfg.h).
pub const CHAVE_ASSINATURA: u16 = 0x00;
pub const CHAVE_ID: u16 = 0x01;
pub const CHAVE_DIRETORIO: u16 = 0x19;

/// "QEMU" lido em big endian na chave 0.
const ASSINATURA: u32 = 0x5145_4D55;
/// "QEMU CFG": o que o registrador de DMA devolve numa leitura.
const ASSINATURA_DMA: u64 = 0x5145_4D55_2043_4647;
/// Bit 1 da chave ID: interface DMA presente.
const ID_DMA: u32 = 1 << 1;
/// Tamanho fixo do nome no diretorio (FW_CFG_MAX_FILE_PATH).
pub const TAMANHO_NOME: usize = 56;

// Bits do campo controle do AcessoDma.
const DMA_ERRO: u32 = 1 << 0;
const DMA_LER: u32 = 1 << 1;
const DMA_SELECIONAR: u32 = 1 << 3;
const DMA_ESCREVER: u32 = 1 << 4;

/// O QEMU termina o DMA dentro da propria escrita no registrador (hoje
/// nunca passa de 0 voltas), mas a spec permite terminar depois.
const MAX_VOLTAS: u32 = 1_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Erro {
    /// A assinatura "QEMU" nao apareceu: nao ha fw_cfg neste endereco.
    Ausente,
    /// fw_cfg sem a interface DMA (escrever exige DMA).
    SemDma,
    /// Nenhum arquivo com esse nome no diretorio.
    NaoAchado,
    /// O QEMU ligou o bit de erro (item so leitura, tamanho errado...).
    Dma,
    /// O DMA nao terminou dentro do limite de voltas.
    TempoEsgotado,
}

impl Erro {
    pub fn descricao(self) -> &'static str {
        match self {
            Erro::Ausente => "fw_cfg ausente",
            Erro::SemDma => "fw_cfg sem DMA",
            Erro::NaoAchado => "arquivo não achado",
            Erro::Dma => "erro no DMA",
            Erro::TempoEsgotado => "DMA não terminou",
        }
    }
}

/// Uma entrada do diretorio de arquivos.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arquivo {
    /// Chave que seleciona o arquivo. Muda conforme os dispositivos
    /// presentes (o diretorio e ordenado por nome): nunca fixe no codigo.
    pub seletor: u16,
    pub tamanho: u32,
}

/// Pedido de DMA, lido pelo QEMU direto da RAM (FWCfgDmaAccess, big endian).
#[repr(C, align(16))]
struct AcessoDma {
    controle: u32,
    tamanho: u32,
    endereco: u64,
}

pub struct FwCfg {
    base: usize,
}

impl FwCfg {
    /// fw_cfg do QEMU virt, no endereco fixo (como a PL011 do uart.rs).
    pub const fn qemu_virt() -> Self {
        FwCfg { base: BASE_QEMU_VIRT }
    }

    /// # Seguranca
    /// `base` precisa ser o fw_cfg MMIO, mapeado como dispositivo.
    pub const unsafe fn em(base: usize) -> Self {
        FwCfg { base }
    }

    fn selecionar(&self, chave: u16) {
        // registrador big endian: o QEMU troca os bytes de volta
        unsafe { write_volatile((self.base + SELETOR) as *mut u16, chave.to_be()) }
    }

    fn ler_byte(&self) -> u8 {
        unsafe { read_volatile((self.base + DADOS) as *const u8) }
    }

    /// Le os proximos bytes do item selecionado (interface sem DMA).
    fn ler_bytes(&self, destino: &mut [u8]) {
        for byte in destino.iter_mut() {
            *byte = self.ler_byte();
        }
    }

    fn ler_be16(&self) -> u16 {
        let mut b = [0u8; 2];
        self.ler_bytes(&mut b);
        u16::from_be_bytes(b)
    }

    fn ler_be32(&self) -> u32 {
        let mut b = [0u8; 4];
        self.ler_bytes(&mut b);
        u32::from_be_bytes(b)
    }

    /// A chave 0 traz "QEMU" quando o fw_cfg existe.
    pub fn presente(&self) -> bool {
        self.selecionar(CHAVE_ASSINATURA);
        self.ler_be32() == ASSINATURA
    }

    /// Chave ID (little endian): bit 0 interface classica, bit 1 DMA.
    pub fn recursos(&self) -> u32 {
        self.selecionar(CHAVE_ID);
        let mut b = [0u8; 4];
        self.ler_bytes(&mut b);
        u32::from_le_bytes(b)
    }

    /// DMA anunciado na chave ID e assinatura "QEMU CFG" no registrador.
    pub fn tem_dma(&self) -> bool {
        let lido = unsafe { read_volatile((self.base + DMA) as *const u64) };
        self.recursos() & ID_DMA != 0 && u64::from_be(lido) == ASSINATURA_DMA
    }

    /// Procura `nome` no diretorio (chave 0x19). Cada entrada tem 64 bytes:
    /// tamanho u32, seletor u16, reservado u16, nome[56] terminado em zero.
    pub fn achar(&self, nome: &[u8]) -> Result<Arquivo, Erro> {
        if !self.presente() {
            return Err(Erro::Ausente);
        }
        self.selecionar(CHAVE_DIRETORIO);
        let quantos = self.ler_be32();
        for _ in 0..quantos {
            let tamanho = self.ler_be32();
            let seletor = self.ler_be16();
            let _reservado = self.ler_be16();
            let mut lido = [0u8; TAMANHO_NOME];
            self.ler_bytes(&mut lido);
            if nome_igual(&lido, nome) {
                return Ok(Arquivo { seletor, tamanho });
            }
        }
        Err(Erro::NaoAchado)
    }

    /// Le o comeco de um arquivo pela interface classica (sem DMA).
    pub fn ler(&self, arquivo: Arquivo, destino: &mut [u8]) {
        self.selecionar(arquivo.seletor);
        self.ler_bytes(destino);
    }

    /// Escreve `dados` no arquivo, a partir do byte 0, por DMA.
    /// O QEMU so aceita se o arquivo for gravavel e o tamanho couber.
    pub fn escrever(&self, arquivo: Arquivo, dados: &[u8]) -> Result<(), Erro> {
        let controle = (arquivo.seletor as u32) << 16 | DMA_SELECIONAR | DMA_ESCREVER;
        self.transferir(controle, dados.as_ptr() as u64, dados.len() as u32)
    }

    /// Le o comeco do arquivo por DMA.
    pub fn ler_dma(&self, arquivo: Arquivo, destino: &mut [u8]) -> Result<(), Erro> {
        let controle = (arquivo.seletor as u32) << 16 | DMA_SELECIONAR | DMA_LER;
        self.transferir(controle, destino.as_mut_ptr() as u64, destino.len() as u32)
    }

    /// Monta o AcessoDma na pilha (a pilha e RAM; o mapa e identidade, entao
    /// o endereco virtual serve como fisico), entrega ao QEMU e espera.
    fn transferir(&self, controle: u32, endereco: u64, tamanho: u32) -> Result<(), Erro> {
        if !self.tem_dma() {
            return Err(Erro::SemDma);
        }
        let mut acesso = AcessoDma { controle: 0, tamanho: 0, endereco: 0 };
        let pedido = &raw mut acesso;
        let conteudo = AcessoDma {
            controle: controle.to_be(),
            tamanho: tamanho.to_be(),
            endereco: endereco.to_be(),
        };
        unsafe { write_volatile(pedido, conteudo) };
        // Com cache ligado num hardware real (ou KVM) aqui seria preciso
        // "dc cvac" no pedido e nos dados. O QEMU TCG nao modela cache:
        // basta a barreira para tudo estar na memoria antes do QEMU ler.
        barreira();
        unsafe { write_volatile((self.base + DMA) as *mut u64, (pedido as u64).to_be()) };
        let resultado = esperar(pedido);
        // os dados lidos por DMA ficam visiveis depois desta barreira
        barreira();
        resultado
    }
}

/// Espera o QEMU zerar o controle (sucesso) ou ligar o bit de erro.
fn esperar(pedido: *const AcessoDma) -> Result<(), Erro> {
    for _ in 0..MAX_VOLTAS {
        let controle = u32::from_be(unsafe { read_volatile(&raw const (*pedido).controle) });
        if controle & DMA_ERRO != 0 {
            return Err(Erro::Dma);
        }
        if controle == 0 {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err(Erro::TempoEsgotado)
}

/// Compara o nome do diretorio (terminado em zero) com `nome`.
fn nome_igual(lido: &[u8; TAMANHO_NOME], nome: &[u8]) -> bool {
    let fim = lido.iter().position(|&b| b == 0).unwrap_or(TAMANHO_NOME);
    &lido[..fim] == nome
}

/// "dsb sy": termina todos os acessos a memoria antes de seguir. Sem
/// "nomem", tambem impede o compilador de mudar acessos de lugar.
fn barreira() {
    unsafe { asm!("dsb sy", options(nostack, preserves_flags)) }
}
