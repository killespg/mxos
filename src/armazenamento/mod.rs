//! Armazenamento: discos virtio-blk com FAT32 e ext4.
//!   - virtio_blk: driver do disco (setores de 512 bytes, por polling)
//!   - fat32:      sistema de arquivos (nomes longos, duas FATs, FSInfo)
//!   - ext4:       ext2/3/4 so leitura (discos e pendrives do Linux)
//!   - comandos:   ls, cat, escrever, mkdir, rm, mv, df, discos, montar
//!
//! O primeiro disco com FAT32 e montado em "/"; cada ext4 em /mnt/<disco>
//! (sozinho ou na primeira particao Linux do MBR). Os outros discos ficam
//! crus, para os programas em /dev/<disco> (mke2fs, dd). Tudo fica num estado global
//! protegido por uma trava de giro (como o heap). Nunca usar de dentro de
//! uma interrupcao: o driver dorme (irq::cochilar) esperando o disco.
//!
//! Caminhos: "/", "/paginas/index.html". Maiusculas e minusculas sao
//! iguais na busca; "." e ".." funcionam; sem pasta atual (tudo a partir
//! da raiz).
//!
//! So existe no QEMU (virtio-mmio).

mod comandos;
mod ext4;
mod fat32;
#[cfg(feature = "mxq")]
mod amlogic;
#[cfg(feature = "mxq")]
mod meson_sd;
mod virtio_blk;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::hint::spin_loop;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::virtio::virtio_mmio::{self, VirtioMmio};
use ext4::Ext4;
use fat32::Fat32;
use virtio_blk::VirtioBlk;

pub use comandos::comando;

/// Um disco de setores de 512 bytes. Buffers sempre multiplos de 512.
pub trait Disco {
    fn nome(&self) -> &str;
    fn setores(&self) -> u64;
    fn ler(&mut self, setor: u64, buffer: &mut [u8]) -> Result<(), Erro>;
    fn escrever(&mut self, setor: u64, dados: &[u8]) -> Result<(), Erro>;
    /// Garante que o que foi escrito chegou ao meio (flush). Chamado ao
    /// fim de cada operacao que escreve.
    fn sincronizar(&mut self) -> Result<(), Erro> {
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Erro {
    /// Nenhum disco (ou nenhum volume montado).
    SemDisco,
    /// O dispositivo falhou ou recusou a leitura/escrita.
    Disco,
    NaoAchado,
    JaExiste,
    NaoEPasta,
    EPasta,
    PastaNaoVazia,
    Cheio,
    NomeInvalido,
    /// O que esta gravado no disco nao faz sentido.
    Corrompido(&'static str),
    /// O volume e so de leitura (ext4).
    SoLeitura,
}

impl Erro {
    pub fn descricao(&self) -> &'static str {
        match self {
            Erro::SemDisco => "nenhum disco montado",
            Erro::Disco => "erro de leitura ou escrita no disco",
            Erro::NaoAchado => "arquivo ou pasta não encontrado",
            Erro::JaExiste => "já existe",
            Erro::NaoEPasta => "não é uma pasta",
            Erro::EPasta => "é uma pasta",
            Erro::PastaNaoVazia => "a pasta não está vazia",
            Erro::Cheio => "disco cheio",
            Erro::NomeInvalido => "nome inválido",
            Erro::Corrompido(motivo) => motivo,
            Erro::SoLeitura => "o disco é só de leitura",
        }
    }
}

/// Um item de `listar`. Pastas tem tamanho 0.
#[derive(Clone, Debug)]
pub struct ItemPasta {
    pub nome: String,
    pub pasta: bool,
    pub tamanho: u64,
}

/// Um disco. Com FAT32: `tamanho` e a area de dados do volume (o que cabe
/// em arquivos) e `livre` o que sobra nela. Sem: `tamanho` e o disco cru.
#[derive(Clone, Debug)]
pub struct InfoDisco {
    pub nome: String,
    pub tamanho: u64,
    pub livre: u64,
    pub sistema: &'static str,
    pub rotulo: String,
    /// Setores de 512 bytes do disco inteiro.
    pub setores: u64,
    /// true = e o volume montado em "/".
    pub montado: bool,
}

struct Estado {
    procurou: bool,
    volume: Option<Fat32>,
    /// ext4 montados: (nome do disco, volume), em /mnt/<nome>.
    montados: Vec<(String, Ext4)>,
    /// Discos sem volume montado (para /dev/<nome>).
    crus: Vec<Box<dyn Disco>>,
    /// Discos achados que nao estao montados.
    outros: Vec<InfoDisco>,
    /// Por que o primeiro disco nao montou.
    erro_montagem: Option<Erro>,
}

/// Estado global com trava de giro (so o laco principal usa).
struct Global {
    trava: AtomicBool,
    estado: UnsafeCell<Estado>,
}

unsafe impl Sync for Global {}

static GLOBAL: Global = Global {
    trava: AtomicBool::new(false),
    estado: UnsafeCell::new(Estado { procurou: false, volume: None, montados: Vec::new(), crus: Vec::new(), outros: Vec::new(), erro_montagem: None }),
};

/// Roda `acao` com a trava pega. Nao chame uma funcao publica deste modulo
/// de dentro da acao (a trava nao e reentrante).
fn com<R>(acao: impl FnOnce(&mut Estado) -> R) -> R {
    while GLOBAL.trava.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        spin_loop();
    }
    let resultado = acao(unsafe { &mut *GLOBAL.estado.get() });
    GLOBAL.trava.store(false, Ordering::Release);
    resultado
}

fn info_volume(fs: &Fat32, montado: bool) -> InfoDisco {
    InfoDisco {
        nome: String::from(fs.nome_disco()),
        tamanho: fs.bytes_totais(),
        livre: fs.bytes_livres(),
        sistema: "FAT32",
        rotulo: String::from(fs.rotulo()),
        setores: fs.setores_disco(),
        montado,
    }
}

/// Procura os virtio-blk (DeviceID 2) e monta o primeiro FAT32 em "/".
/// Chamar uma vez no boot (depois do irq::iniciar); chamar de novo so
/// devolve o que ja foi achado.
pub fn iniciar() -> Result<InfoDisco, Erro> {
    com(|estado| {
        if !estado.procurou {
            estado.procurou = true;
            procurar_discos(estado);
        }
        match &estado.volume {
            Some(fs) => Ok(info_volume(fs, true)),
            None => Err(estado.erro_montagem.unwrap_or(Erro::SemDisco)),
        }
    })
}

/// Varre os slots do maior para o menor: assim "vda" e o primeiro
/// -device da linha de comando do QEMU.
fn procurar_discos(estado: &mut Estado) {
    // TV box: o pendrive primeiro (vira "/"), depois o cartao SD e a eMMC
    #[cfg(feature = "mxq")]
    for disco in crate::usb::discos().into_iter().chain(meson_sd::achar()) {
        montar(estado, disco);
    }
    let mut letra = b'a';
    for slot in (0..virtio_mmio::SLOTS_QEMU_VIRT).rev() {
        let base = virtio_mmio::BASE_QEMU_VIRT + slot * virtio_mmio::PASSO_QEMU_VIRT;
        let Some(dispositivo) = VirtioMmio::procurar_em(base, virtio_mmio::PASSO_QEMU_VIRT, 1, virtio_mmio::TIPO_BLOCO) else {
            continue;
        };
        let nome = format!("vd{}", letra as char);
        letra = letra.saturating_add(1).min(b'z');
        match VirtioBlk::iniciar(dispositivo, nome.clone()) {
            Ok(disco) => montar(estado, Box::new(disco)),
            Err(e) => {
                estado.erro_montagem.get_or_insert(e);
                estado.outros.push(InfoDisco {
                    nome,
                    tamanho: 0,
                    livre: 0,
                    sistema: "com defeito",
                    rotulo: String::new(),
                    setores: 0,
                    montado: false,
                });
            }
        }
    }
}

/// Monta o disco: ext4 em /mnt/<nome>; FAT32 em "/" se ainda nao ha
/// volume. O resto fica cru (/dev/<nome>).
fn montar(estado: &mut Estado, disco: Box<dyn Disco>) {
    #[cfg(feature = "mxq")]
    let disco = match montar_emmc(estado, disco) {
        Some(d) => d,
        None => return,
    };
    let disco = match tentar_ext4(disco) {
        Ok(fs) => {
            let nome = String::from(fs.nome_disco());
            estado.montados.push((nome, fs));
            return;
        }
        Err(d) => d,
    };
    let (nome, setores) = (String::from(disco.nome()), disco.setores());
    if estado.volume.is_some() {
        // segundo FAT32 (ou desconhecido): cru, para os programas
        estado.outros.push(InfoDisco { nome, tamanho: setores * 512, livre: 0, sistema: "cru", rotulo: String::new(), setores, montado: false });
        estado.crus.push(disco);
        return;
    }
    match Fat32::montar(disco, relogio) {
        Ok(fs) => estado.volume = Some(fs),
        Err(e) => {
            estado.erro_montagem.get_or_insert(e);
            estado.outros.push(InfoDisco {
                nome,
                tamanho: setores * 512,
                livre: 0,
                sistema: "desconhecido",
                rotulo: String::new(),
                setores,
                montado: false,
            });
        }
    }
}

/// A eMMC da box: se o MXOS foi instalado nela (particao data em FAT32) e
/// nao ligou de um pendrive, ela vira "/" (com escrita so na data). Senao
/// fica crua (devolve o disco).
#[cfg(feature = "mxq")]
fn montar_emmc(estado: &mut Estado, mut disco: Box<dyn Disco>) -> Option<Box<dyn Disco>> {
    if disco.nome() != "emmc" || estado.volume.is_some() {
        if disco.nome() == "emmc" {
            estado.crus.push(disco);
            return None;
        }
        return Some(disco);
    }
    let tabela = amlogic::ler_mpt(&mut *disco).unwrap_or_default();
    let total = disco.setores();
    let Some(dados) = amlogic::achar(&tabela, "data", total).cloned() else {
        estado.crus.push(disco);
        return None;
    };
    if !amlogic::tem_fat32(&mut *disco, &dados) {
        estado.crus.push(disco);
        return None;
    }
    meson_sd::liberar_escrita(dados.inicio, dados.inicio + dados.setores);
    let particao = Box::new(Particao { disco, inicio: dados.inicio, setores: dados.setores });
    if let Ok(fs) = Fat32::montar(particao, relogio) {
        estado.volume = Some(fs);
    }
    None
}

/// Troca o Android da eMMC pelo MXOS (comando "instalar-emmc"): ver amlogic.rs.
#[cfg(feature = "mxq")]
pub fn instalar_emmc(saida: &mut dyn core::fmt::Write) -> Result<(), Erro> {
    com(|estado| {
        let Estado { volume, crus, .. } = estado;
        let pendrive = volume.as_mut().ok_or(Erro::SemDisco)?;
        let emmc = crus.iter_mut().find(|d| d.nome() == "emmc").ok_or(Erro::Corrompido("eMMC não achada (ou o MXOS ligou dela)"))?;
        amlogic::instalar(&mut **emmc, pendrive, saida)
    })
}

/// Volta a particao boot do Android (da copia no pendrive).
#[cfg(feature = "mxq")]
pub fn restaurar_boot() -> Result<(), Erro> {
    com(|estado| {
        let Estado { volume, crus, .. } = estado;
        let pendrive = volume.as_mut().ok_or(Erro::SemDisco)?;
        let emmc = crus.iter_mut().find(|d| d.nome() == "emmc").ok_or(Erro::Corrompido("eMMC não achada"))?;
        amlogic::restaurar_boot(&mut **emmc, pendrive)
    })
}

/// O disco inteiro e ext4, ou a primeira particao Linux (0x83) do MBR.
fn tentar_ext4(disco: Box<dyn Disco>) -> Result<Ext4, Box<dyn Disco>> {
    let mut disco = match Ext4::montar(disco) {
        Ok(fs) => return Ok(fs),
        Err((_, d)) => d,
    };
    let mut mbr = [0u8; 512];
    if disco.ler(0, &mut mbr).is_err() || mbr[510..512] != [0x55, 0xAA] {
        return Err(disco);
    }
    for k in 0..4 {
        let e = &mbr[446 + k * 16..462 + k * 16];
        if e[4] == 0x83 {
            let inicio = u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as u64;
            let setores = u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as u64;
            // so embrulha se a particao tem mesmo o numero magico do ext4
            let mut sb = [0u8; 512];
            if disco.ler(inicio + 2, &mut sb).is_err() || u16::from_le_bytes([sb[56], sb[57]]) != 0xEF53 {
                return Err(disco);
            }
            let particao = Box::new(Particao { disco, inicio, setores });
            return Ext4::montar(particao).map_err(|(_, p)| p);
        }
    }
    Err(disco)
}

/// Um pedaco do disco (uma particao) visto como disco inteiro.
struct Particao {
    disco: Box<dyn Disco>,
    inicio: u64,
    setores: u64,
}

impl Disco for Particao {
    fn nome(&self) -> &str {
        self.disco.nome()
    }
    fn setores(&self) -> u64 {
        self.setores
    }
    fn ler(&mut self, setor: u64, buffer: &mut [u8]) -> Result<(), Erro> {
        self.disco.ler(self.inicio + setor, buffer)
    }
    fn escrever(&mut self, setor: u64, dados: &[u8]) -> Result<(), Erro> {
        self.disco.escrever(self.inicio + setor, dados)
    }
}

/// O volume montado primeiro, depois os outros discos achados.
pub fn discos() -> Vec<InfoDisco> {
    com(|estado| {
        let mut lista: Vec<InfoDisco> = estado.volume.iter().map(|fs| info_volume(fs, true)).collect();
        for (nome, fs) in &estado.montados {
            lista.push(InfoDisco {
                nome: format!("{nome} (/mnt/{nome})"),
                tamanho: fs.bytes_totais(),
                livre: fs.bytes_livres(),
                sistema: "ext4",
                rotulo: String::from(fs.rotulo()),
                setores: fs.bytes_totais() / 512,
                montado: true,
            });
        }
        lista.extend(estado.outros.iter().cloned());
        lista
    })
}

/// Onde um caminho mora: no FAT32 de "/", num ext4 de /mnt ou na pasta
/// /mnt (que so existe com algum ext4 montado).
enum Onde {
    Raiz,
    Mnt,
    Ext4(usize, String),
}

fn onde(estado: &Estado, caminho: &str) -> Onde {
    if estado.montados.is_empty() {
        return Onde::Raiz;
    }
    let minusculo = caminho.to_lowercase();
    if minusculo == "/mnt" || minusculo == "/mnt/" {
        return Onde::Mnt;
    }
    for (k, (nome, _)) in estado.montados.iter().enumerate() {
        let ponto = format!("/mnt/{nome}");
        if minusculo == ponto || minusculo.starts_with(&format!("{ponto}/")) {
            return Onde::Ext4(k, String::from(&caminho[ponto.len()..]));
        }
    }
    Onde::Raiz
}

/// O FAT32 de "/" ou o ext4 do caminho.
fn com_caminho<R>(caminho: &str, fat: impl FnOnce(&mut Fat32) -> Result<R, Erro>, ext: impl FnOnce(&mut Ext4, &str) -> Result<R, Erro>) -> Result<R, Erro> {
    com(|estado| match onde(estado, caminho) {
        Onde::Raiz => match &mut estado.volume {
            Some(v) => fat(v),
            None => Err(Erro::SemDisco),
        },
        Onde::Ext4(k, resto) => ext(&mut estado.montados[k].1, &resto),
        Onde::Mnt => Err(Erro::EPasta),
    })
}

/// Itens de uma pasta, na ordem em que estao no disco.
pub fn listar(caminho: &str) -> Result<Vec<ItemPasta>, Erro> {
    let especial = com(|estado| match onde(estado, caminho) {
        Onde::Mnt => Some(estado.montados.iter().map(|(n, _)| ItemPasta { nome: n.clone(), pasta: true, tamanho: 0 }).collect()),
        _ => None,
    });
    if let Some(lista) = especial {
        return Ok(lista);
    }
    let mut itens = com_caminho(caminho, |fs| fs.listar(caminho), |fs, resto| fs.listar(resto))?;
    // a raiz mostra a pasta /mnt quando ha disco montado nela
    let tem_mnt = com(|e| !e.montados.is_empty());
    if caminho == "/" && tem_mnt && !itens.iter().any(|i| i.nome.eq_ignore_ascii_case("mnt")) {
        itens.push(ItemPasta { nome: String::from("mnt"), pasta: true, tamanho: 0 });
    }
    Ok(itens)
}

pub fn ler(caminho: &str) -> Result<Vec<u8>, Erro> {
    com_caminho(caminho, |fs| fs.ler(caminho), |fs, resto| fs.ler(resto))
}

/// Um arquivo aberto para ler aos pedacos (cache de paginas).
pub enum Abertura {
    Fat(Vec<u32>),
    Ext4(String, Vec<u64>),
}

/// Tamanho e onde estao os dados de um arquivo, para ler so os pedacos
/// que precisa.
pub fn abrir_leitura(caminho: &str) -> Result<(usize, Abertura), Erro> {
    com(|estado| match onde(estado, caminho) {
        Onde::Raiz => match &mut estado.volume {
            Some(v) => v.abrir_leitura(caminho).map(|(t, c)| (t, Abertura::Fat(c))),
            None => Err(Erro::SemDisco),
        },
        Onde::Ext4(k, resto) => {
            let (nome, fs) = &mut estado.montados[k];
            fs.abrir_leitura(&resto).map(|(t, m)| (t, Abertura::Ext4(nome.clone(), m)))
        }
        Onde::Mnt => Err(Erro::EPasta),
    })
}

/// Um pedaco de um arquivo aberto com `abrir_leitura`.
pub fn ler_trecho(abertura: &Abertura, inicio: usize, destino: &mut [u8]) -> Result<(), Erro> {
    com(|estado| match abertura {
        Abertura::Fat(cadeia) => match &mut estado.volume {
            Some(v) => v.ler_trecho(cadeia, inicio, destino),
            None => Err(Erro::SemDisco),
        },
        Abertura::Ext4(nome, mapa) => match estado.montados.iter_mut().find(|(n, _)| n == nome) {
            Some((_, fs)) => fs.ler_trecho(mapa, inicio, destino),
            None => Err(Erro::SemDisco),
        },
    })
}

/// Cria ou substitui o arquivo. A pasta tem de existir.
pub fn escrever(caminho: &str, dados: &[u8]) -> Result<(), Erro> {
    crate::processo::cache::esquecer(caminho);
    com_caminho(caminho, |fs| fs.escrever(caminho, dados), |_, _| Err(Erro::SoLeitura))
}

pub fn criar_pasta(caminho: &str) -> Result<(), Erro> {
    com_caminho(caminho, |fs| fs.criar_pasta(caminho), |_, _| Err(Erro::SoLeitura))
}

/// Apaga um arquivo ou uma pasta vazia.
pub fn apagar(caminho: &str) -> Result<(), Erro> {
    crate::processo::cache::esquecer(caminho);
    com_caminho(caminho, |fs| fs.apagar(caminho), |_, _| Err(Erro::SoLeitura))
}

/// Renomeia ou move: `para` e o caminho novo completo e nao pode existir.
pub fn renomear(de: &str, para: &str) -> Result<(), Erro> {
    crate::processo::cache::esquecer(de);
    crate::processo::cache::esquecer(para);
    com_caminho(de, |fs| fs.renomear(de, para), |_, _| Err(Erro::SoLeitura))
}

// ---------------------------------------------------------------- discos crus

/// Os discos sem volume montado: (nome, setores).
pub fn crus() -> Vec<(String, u64)> {
    com(|estado| estado.crus.iter().map(|d| (String::from(d.nome()), d.setores())).collect())
}

/// Le setores de um disco cru (/dev/<nome>).
pub fn ler_cru(nome: &str, setor: u64, buffer: &mut [u8]) -> Result<(), Erro> {
    com(|estado| match estado.crus.iter_mut().find(|d| d.nome() == nome) {
        Some(d) => d.ler(setor, buffer),
        None => Err(Erro::SemDisco),
    })
}

pub fn escrever_cru(nome: &str, setor: u64, dados: &[u8]) -> Result<(), Erro> {
    com(|estado| match estado.crus.iter_mut().find(|d| d.nome() == nome) {
        Some(d) => {
            d.escrever(setor, dados)?;
            d.sincronizar()
        }
        None => Err(Erro::SemDisco),
    })
}

/// Monta o ext4 de um disco cru em /mnt/<nome>.
pub fn montar_disco(nome: &str) -> Result<(), Erro> {
    com(|estado| {
        let i = estado.crus.iter().position(|d| d.nome() == nome).ok_or(Erro::NaoAchado)?;
        let disco = estado.crus.remove(i);
        match tentar_ext4(disco) {
            Ok(fs) => {
                estado.outros.retain(|o| o.nome != nome);
                estado.montados.push((String::from(nome), fs));
                Ok(())
            }
            Err(d) => {
                estado.crus.insert(i, d);
                Err(Erro::Corrompido("o disco não tem ext4"))
            }
        }
    })
}

/// Desmonta o ext4 de /mnt/<nome> (o disco volta a ser cru).
pub fn desmontar_disco(nome: &str) -> Result<(), Erro> {
    com(|estado| {
        let i = estado.montados.iter().position(|(n, _)| n == nome).ok_or(Erro::NaoAchado)?;
        let (_, fs) = estado.montados.remove(i);
        let disco = fs.desmontar();
        let setores = disco.setores();
        estado.outros.push(InfoDisco { nome: String::from(nome), tamanho: setores * 512, livre: 0, sistema: "cru", rotulo: String::new(), setores, montado: false });
        estado.crus.push(disco);
        Ok(())
    })
}

/// Ha um volume montado em "/"? (para os apps da interface)
#[allow(dead_code)]
pub fn montado() -> bool {
    com(|estado| estado.volume.is_some())
}

// ---------------------------------------------------------------- relogio

/// Sem relogio: 1 de janeiro de 2026, 00:00.
const DATA_PADRAO: (u16, u16) = ((46 << 9) | (1 << 5) | 1, 0);

/// Data e hora atuais no formato da FAT (para as entradas de pasta).
pub(super) fn relogio() -> (u16, u16) {
    let segundos = crate::plataforma::segundos_rtc();
    let dias = segundos / 86_400;
    let (ano, mes, dia) = data_civil(dias);
    if !(1980..=2107).contains(&ano) {
        return DATA_PADRAO;
    }
    let resto = segundos % 86_400;
    let (hora, minuto, segundo) = (resto / 3600, resto % 3600 / 60, resto % 60);
    let data = ((ano - 1980) << 9) as u16 | (mes << 5) as u16 | dia as u16;
    let hora = (hora << 11) as u16 | (minuto << 5) as u16 | (segundo / 2) as u16;
    (data, hora)
}

/// Dias desde 1970-01-01 -> (ano, mes, dia), calendario gregoriano.
/// Algoritmo "days_from_civil" ao contrario, de Howard Hinnant.
fn data_civil(dias: u64) -> (u64, u64, u64) {
    let z = dias + 719_468;
    let era = z / 146_097;
    let dia_da_era = z % 146_097;
    let ano_da_era = (dia_da_era - dia_da_era / 1460 + dia_da_era / 36_524 - dia_da_era / 146_096) / 365;
    let dia_do_ano = dia_da_era - (365 * ano_da_era + ano_da_era / 4 - ano_da_era / 100);
    let mp = (5 * dia_do_ano + 2) / 153;
    let dia = dia_do_ano - (153 * mp + 2) / 5 + 1;
    let mes = if mp < 10 { mp + 3 } else { mp - 9 };
    let ano = ano_da_era + era * 400 + if mes <= 2 { 1 } else { 0 };
    (ano, mes, dia)
}
