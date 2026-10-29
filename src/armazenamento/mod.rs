//! Armazenamento: disco virtio-blk com sistema de arquivos FAT32.
//!   - virtio_blk: driver do disco (setores de 512 bytes, por polling)
//!   - fat32:      sistema de arquivos (nomes longos, duas FATs, FSInfo)
//!   - comandos:   ls, cat, escrever, mkdir, rm, mv, df, discos
//!
//! O primeiro disco com FAT32 e montado em "/". Ele fica num estado global
//! protegido por uma trava de giro (como o heap). Nunca usar de dentro de
//! uma interrupcao: o driver dorme (irq::cochilar) esperando o disco.
//!
//! Caminhos: "/", "/paginas/index.html". Maiusculas e minusculas sao
//! iguais na busca; "." e ".." funcionam; sem pasta atual (tudo a partir
//! da raiz).
//!
//! So existe no QEMU (virtio-mmio).

mod comandos;
mod fat32;
mod virtio_blk;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::hint::spin_loop;
use core::ptr::read_volatile;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::virtio::virtio_mmio::{self, VirtioMmio};
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
    estado: UnsafeCell::new(Estado { procurou: false, volume: None, outros: Vec::new(), erro_montagem: None }),
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

fn com_volume<R>(acao: impl FnOnce(&mut Fat32) -> Result<R, Erro>) -> Result<R, Erro> {
    com(|estado| match &mut estado.volume {
        Some(volume) => acao(volume),
        None => Err(Erro::SemDisco),
    })
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
    let mut letra = b'a';
    for slot in (0..virtio_mmio::SLOTS_QEMU_VIRT).rev() {
        let base = virtio_mmio::BASE_QEMU_VIRT + slot * virtio_mmio::PASSO_QEMU_VIRT;
        let Some(dispositivo) = VirtioMmio::procurar_em(base, virtio_mmio::PASSO_QEMU_VIRT, 1, virtio_mmio::TIPO_BLOCO) else {
            continue;
        };
        let nome = format!("vd{}", letra as char);
        letra = letra.saturating_add(1).min(b'z');
        match VirtioBlk::iniciar(dispositivo, nome.clone()) {
            Ok(disco) => montar(estado, disco),
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

/// Monta o disco em "/" se ainda nao ha volume; senao so anota as
/// informacoes dele e desliga o dispositivo.
fn montar(estado: &mut Estado, disco: VirtioBlk) {
    let (nome, setores) = (String::from(disco.nome()), disco.setores());
    match Fat32::montar(Box::new(disco), relogio) {
        Ok(fs) if estado.volume.is_none() => estado.volume = Some(fs),
        Ok(fs) => {
            estado.outros.push(info_volume(&fs, false));
            drop(fs.desmontar()); // o Drop do driver reseta o dispositivo
        }
        Err(e) => {
            if estado.volume.is_none() {
                estado.erro_montagem.get_or_insert(e);
            }
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

/// O volume montado primeiro, depois os outros discos achados.
pub fn discos() -> Vec<InfoDisco> {
    com(|estado| {
        let mut lista: Vec<InfoDisco> = estado.volume.iter().map(|fs| info_volume(fs, true)).collect();
        lista.extend(estado.outros.iter().cloned());
        lista
    })
}

/// Itens de uma pasta, na ordem em que estao no disco.
pub fn listar(caminho: &str) -> Result<Vec<ItemPasta>, Erro> {
    com_volume(|fs| fs.listar(caminho))
}

pub fn ler(caminho: &str) -> Result<Vec<u8>, Erro> {
    com_volume(|fs| fs.ler(caminho))
}

/// Cria ou substitui o arquivo. A pasta tem de existir.
pub fn escrever(caminho: &str, dados: &[u8]) -> Result<(), Erro> {
    com_volume(|fs| fs.escrever(caminho, dados))
}

pub fn criar_pasta(caminho: &str) -> Result<(), Erro> {
    com_volume(|fs| fs.criar_pasta(caminho))
}

/// Apaga um arquivo ou uma pasta vazia.
pub fn apagar(caminho: &str) -> Result<(), Erro> {
    com_volume(|fs| fs.apagar(caminho))
}

/// Renomeia ou move: `para` e o caminho novo completo e nao pode existir.
pub fn renomear(de: &str, para: &str) -> Result<(), Erro> {
    com_volume(|fs| fs.renomear(de, para))
}

/// Ha um volume montado em "/"? (para os apps da interface)
#[allow(dead_code)]
pub fn montado() -> bool {
    com(|estado| estado.volume.is_some())
}

// ---------------------------------------------------------------- relogio

/// RTC PL031 do QEMU virt: RTCDR (offset 0) = segundos desde 1970 (UTC).
const RTC_PL031: usize = 0x0901_0000;
/// Sem relogio: 1 de janeiro de 2026, 00:00.
const DATA_PADRAO: (u16, u16) = ((46 << 9) | (1 << 5) | 1, 0);

/// Data e hora atuais no formato da FAT (para as entradas de pasta).
fn relogio() -> (u16, u16) {
    let segundos = unsafe { read_volatile(RTC_PL031 as *const u32) } as u64;
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
