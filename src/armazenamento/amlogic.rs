//! A eMMC da TV box, do jeito da Amlogic: a tabela de particoes propria
//! (MPT, em 36 MB) e o instalador que troca o Android pelo MXOS sem tocar
//! no U-Boot. NUNCA TESTADO NO HARDWARE.
//!
//! O instalador (comando "instalar-emmc"):
//!   1. guarda no pendrive /mxos-backup: a particao boot do Android, os
//!      primeiros 4 MB (U-Boot) e a tabela de particoes
//!   2. formata a particao "data" (a maior) em FAT32 e copia o pendrive
//!      inteiro para ela: ela vira o disco "/" do MXOS
//!   3. grava na particao "boot" o MXOS embrulhado numa imagem de boot do
//!      Android: o comando storeboot do U-Boot (o boot normal da box) liga
//!      o MXOS como ligaria o Android
//! O comeco da eMMC (U-Boot, tabela, env, dtb) nunca e escrito: o driver
//! (meson_sd.rs) so deixa escrever nas faixas liberadas aqui.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::Write;

use super::fat32::{self, Fat32};
use super::{meson_sd, relogio, Disco, Erro};

/// Onde a tabela fica (byte 36 MB da eMMC), em setores.
const SETOR_MPT: u64 = 0x2400000 / 512;
const ENDERECO_KERNEL: u32 = 0x0108_0000;
const PAGINA_BOOT: usize = 2048;

#[derive(Clone, Debug)]
pub struct ParticaoMpt {
    pub nome: String,
    pub inicio: u64,
    pub setores: u64,
}

/// Le a tabela de particoes da Amlogic ("MPT").
pub fn ler_mpt(disco: &mut dyn Disco) -> Result<Vec<ParticaoMpt>, Erro> {
    let mut b = vec![0u8; 3 * 512];
    disco.ler(SETOR_MPT, &mut b)?;
    if &b[0..3] != b"MPT" {
        return Err(Erro::Corrompido("a eMMC não tem a tabela de partições da Amlogic"));
    }
    let quantas = (i32::from_le_bytes([b[16], b[17], b[18], b[19]]) as usize).min(32);
    let mut lista = Vec::new();
    for k in 0..quantas {
        let e = &b[24 + k * 40..24 + (k + 1) * 40];
        let fim = e[..16].iter().position(|&c| c == 0).unwrap_or(16);
        let nome = String::from_utf8_lossy(&e[..fim]).into_owned();
        let tamanho = u64::from_le_bytes(e[16..24].try_into().unwrap());
        let posicao = u64::from_le_bytes(e[24..32].try_into().unwrap());
        lista.push(ParticaoMpt { nome, inicio: posicao / 512, setores: tamanho / 512 });
    }
    Ok(lista)
}

/// A particao `nome`, se ela existe e cabe na eMMC.
pub fn achar<'a>(tabela: &'a [ParticaoMpt], nome: &str, setores_disco: u64) -> Option<&'a ParticaoMpt> {
    tabela.iter().find(|p| p.nome == nome && p.setores > 0 && p.inicio + p.setores <= setores_disco && p.inicio > SETOR_MPT)
}

/// A particao tem FAT32? (olha o setor de boot antes de embrulhar)
pub fn tem_fat32(disco: &mut dyn Disco, p: &ParticaoMpt) -> bool {
    let mut b = [0u8; 512];
    disco.ler(p.inicio, &mut b).is_ok() && &b[82..87] == b"FAT32" && b[510..512] == [0x55, 0xAA]
}

/// O MXOS dentro de uma imagem de boot do Android (cabecalho v0, sem
/// ramdisk): o storeboot do U-Boot carrega o "kernel" em 0x1080000.
pub fn imagem_de_boot(kernel: &[u8]) -> Vec<u8> {
    let mut img = vec![0u8; PAGINA_BOOT];
    img[0..8].copy_from_slice(b"ANDROID!");
    let mut campo = |pos: usize, v: u32| img[pos..pos + 4].copy_from_slice(&v.to_le_bytes());
    campo(8, kernel.len() as u32);
    campo(12, ENDERECO_KERNEL);
    campo(16, 0); // ramdisk
    campo(20, 0x0100_0000);
    campo(24, 0); // second
    campo(28, 0x00F0_0000);
    campo(32, 0x0000_0100); // tags
    campo(36, PAGINA_BOOT as u32);
    img[48..52].copy_from_slice(b"mxos");
    img.extend_from_slice(kernel);
    img.resize(img.len().div_ceil(PAGINA_BOOT) * PAGINA_BOOT, 0);
    img
}

/// Um pedaco da eMMC visto como disco, sem ser dono dela (o instalador
/// ainda precisa da eMMC inteira depois). Quem cria garante que a eMMC
/// vive mais que este disco.
pub struct Pedaco {
    disco: *mut dyn Disco,
    inicio: u64,
    setores: u64,
}

impl Pedaco {
    /// # Seguranca
    /// `disco` precisa durar mais que o Pedaco (e quem o usa).
    pub unsafe fn novo(disco: &mut dyn Disco, p: &ParticaoMpt) -> Pedaco {
        let disco: *mut (dyn Disco + '_) = disco;
        Pedaco { disco: unsafe { core::mem::transmute::<*mut (dyn Disco + '_), *mut dyn Disco>(disco) }, inicio: p.inicio, setores: p.setores }
    }
}

impl Disco for Pedaco {
    fn nome(&self) -> &str {
        "emmc (dados)"
    }
    fn setores(&self) -> u64 {
        self.setores
    }
    fn ler(&mut self, setor: u64, buffer: &mut [u8]) -> Result<(), Erro> {
        unsafe { (*self.disco).ler(self.inicio + setor, buffer) }
    }
    fn escrever(&mut self, setor: u64, dados: &[u8]) -> Result<(), Erro> {
        unsafe { (*self.disco).escrever(self.inicio + setor, dados) }
    }
    fn sincronizar(&mut self) -> Result<(), Erro> {
        unsafe { (*self.disco).sincronizar() }
    }
}

/// Copia a arvore `caminho` de um volume para o outro.
fn copiar(origem: &mut Fat32, destino: &mut Fat32, caminho: &str, saida: &mut dyn Write, contagem: &mut usize) -> Result<(), Erro> {
    for item in origem.listar(if caminho.is_empty() { "/" } else { caminho })? {
        if item.nome == "." || item.nome == ".." || item.nome.eq_ignore_ascii_case("mxos-backup") {
            continue;
        }
        let filho = format!("{caminho}/{}", item.nome);
        if item.pasta {
            match destino.criar_pasta(&filho) {
                Ok(()) | Err(Erro::JaExiste) => {}
                Err(e) => return Err(e),
            }
            copiar(origem, destino, &filho, saida, contagem)?;
        } else {
            let dados = origem.ler(&filho)?;
            destino.escrever(&filho, &dados)?;
            *contagem += 1;
            if *contagem % 50 == 0 {
                let _ = writeln!(saida, "  {contagem} arquivos copiados...");
            }
        }
    }
    Ok(())
}

/// Le `setores` a partir de `inicio`, aos pedacos de 1 MB.
fn ler_tudo(disco: &mut dyn Disco, inicio: u64, setores: u64) -> Result<Vec<u8>, Erro> {
    let mut dados = vec![0u8; setores as usize * 512];
    let mut feito = 0u64;
    while feito < setores {
        let n = (setores - feito).min(2048);
        disco.ler(inicio + feito, &mut dados[feito as usize * 512..(feito + n) as usize * 512])?;
        feito += n;
    }
    Ok(dados)
}

/// O instalador inteiro. `pendrive` e o volume "/" (de onde o MXOS ligou).
pub fn instalar(emmc: &mut dyn Disco, pendrive: &mut Fat32, saida: &mut dyn Write) -> Result<(), Erro> {
    let total = emmc.setores();
    let tabela = ler_mpt(emmc)?;
    let _ = writeln!(saida, "partições da eMMC:");
    for p in &tabela {
        let _ = writeln!(saida, "  {:<12} {:>6} MB em {} MB", p.nome, p.setores / 2048, p.inicio / 2048);
    }
    let boot = achar(&tabela, "boot", total).ok_or(Erro::Corrompido("a eMMC não tem a partição boot"))?.clone();
    let dados = achar(&tabela, "data", total).ok_or(Erro::Corrompido("a eMMC não tem a partição data"))?.clone();
    let kernel = pendrive.ler("/mxos-mxq.img")?;
    if imagem_de_boot(&kernel).len() as u64 > boot.setores * 512 {
        return Err(Erro::Corrompido("o MXOS não cabe na partição boot"));
    }

    let _ = writeln!(saida, "1/3 cópia de segurança em /mxos-backup do pendrive...");
    match pendrive.criar_pasta("/mxos-backup") {
        Ok(()) | Err(Erro::JaExiste) => {}
        Err(e) => return Err(e),
    }
    let boot_android = ler_tudo(emmc, boot.inicio, boot.setores)?;
    pendrive.escrever("/mxos-backup/boot.img", &boot_android)?;
    drop(boot_android);
    pendrive.escrever("/mxos-backup/inicio-4MB.bin", &ler_tudo(emmc, 0, 8192)?)?;
    pendrive.escrever("/mxos-backup/tabela.bin", &ler_tudo(emmc, SETOR_MPT, 3)?)?;

    let _ = writeln!(saida, "2/3 formatando a partição data ({} MB) e copiando o MXOS...", dados.setores / 2048);
    meson_sd::liberar_escrita(dados.inicio, dados.inicio + dados.setores);
    {
        let mut pedaco = unsafe { Pedaco::novo(emmc, &dados) };
        fat32::formatar(&mut pedaco, "MXOS")?;
        let mut novo = Fat32::montar(alloc::boxed::Box::new(pedaco), relogio)?;
        let mut contagem = 0;
        copiar(pendrive, &mut novo, "", saida, &mut contagem)?;
        let _ = writeln!(saida, "  {contagem} arquivos copiados");
    }

    let _ = writeln!(saida, "3/3 gravando o MXOS na partição boot...");
    meson_sd::liberar_escrita(boot.inicio, boot.inicio + boot.setores);
    let mut img = imagem_de_boot(&kernel);
    img.resize(img.len().div_ceil(512) * 512, 0);
    emmc.escrever(boot.inicio, &img)?;
    emmc.sincronizar()?;
    let _ = writeln!(saida, "pronto: tire o pendrive e reinicie. Para voltar a parte de boot do Android: restaurar-boot");
    Ok(())
}

/// Grava de volta a particao boot guardada em /mxos-backup/boot.img.
pub fn restaurar_boot(emmc: &mut dyn Disco, pendrive: &mut Fat32) -> Result<(), Erro> {
    let tabela = ler_mpt(emmc)?;
    let boot = achar(&tabela, "boot", emmc.setores()).ok_or(Erro::Corrompido("a eMMC não tem a partição boot"))?.clone();
    let mut img = pendrive.ler("/mxos-backup/boot.img")?;
    if img.len() as u64 > boot.setores * 512 {
        return Err(Erro::Corrompido("cópia maior que a partição boot"));
    }
    img.resize(img.len().div_ceil(512) * 512, 0);
    meson_sd::liberar_escrita(boot.inicio, boot.inicio + boot.setores);
    emmc.escrever(boot.inicio, &img)?;
    emmc.sincronizar()
}
