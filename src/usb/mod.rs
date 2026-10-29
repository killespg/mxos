//! USB da TV box. Os controladores falam pelo trait Controlador:
//!   - Amlogic (mxq): xHCI do DWC3 em modo host
//!   - Rockchip RK322x: EHCI (high speed) + OHCI (low/full speed) nas tres
//!     portas host (cada EHCI passa para o OHCI o que nao e high speed)
//! Os dispositivos ligados direto nas portas:
//!
//!   - pendrive (armazenamento em massa, SCSI por bulk-only): vira um Disco
//!     (armazenamento/mod.rs monta o FAT32 dele em "/" ou o ext4 em /mnt)
//!   - teclado e mouse (HID, protocolo de boot): viram eventos de entrada
//!     no formato do evdev (entrada/mod.rs)
//!
//! Tudo por polling, sem interrupcao; sem hubs (um receptor sem fio de
//! teclado+mouse e um dispositivo so, com duas interfaces). O DMA da Amlogic
//! nao ve o cache da CPU: aneis, contextos e buffers sao limpos (dc civac)
//! antes e depois. NUNCA TESTADO NO HARDWARE.
//!
//! O U-Boot ja ligou o PHY USB (na Amlogic ele leu o MXOS do pendrive);
//! aqui os controladores so sao reiniciados.

#![allow(dead_code)]

#[cfg(feature = "rk322x")]
mod ehci;
#[cfg(feature = "rk322x")]
mod ohci;
#[cfg(not(feature = "rk322x"))]
mod xhci;

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::UnsafeCell;

use crate::armazenamento::{Disco, Erro};
use crate::entrada::virtio_input::{EventoCru, EV_KEY, EV_REL, EV_SYN, REL_WHEEL, REL_X, REL_Y, SYN_REPORT, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};
/// DWC3 do GXL: o xHCI comeca no inicio da regiao; GCTL diz host/device.
#[cfg(not(feature = "rk322x"))]
const BASE_XHCI: usize = 0xC900_0000;
#[cfg(not(feature = "rk322x"))]
const DWC3_GCTL: usize = 0xC900_C110;
/// RK322x: (EHCI, OHCI) das tres portas host (rk322x.dtsi).
#[cfg(feature = "rk322x")]
const HOSTS_RK: [(usize, usize); 3] = [(0x3008_0000, 0x300A_0000), (0x300C_0000, 0x300E_0000), (0x3010_0000, 0x3012_0000)];

/// Um endpoint aberto num controlador (o numero e dele).
#[derive(Clone, Copy)]
pub struct Ep {
    pub id: u32,
    pub entrada: bool,
}

/// O que o mod.rs precisa de um controlador host. `disp` e o numero que o
/// controlador deu ao dispositivo (slot no xHCI, endereco USB no EHCI/OHCI);
/// velocidade: 1 full, 2 low, 3 high, 4+ super.
pub trait Controlador {
    fn portas(&self) -> u8;
    /// Reinicia a porta e da endereco ao que estiver nela: (disp, velocidade).
    fn ligar_porta(&mut self, porta: u8) -> Option<(u8, u8)>;
    fn ajustar_ep0(&mut self, disp: u8, pacote: u16) -> Result<(), &'static str>;
    fn controle(&mut self, disp: u8, tipo: u8, pedido: u8, valor: u16, indice: u16, dados: &mut [u8]) -> Result<usize, &'static str>;
    fn abrir_endpoint(&mut self, disp: u8, velocidade: u8, endereco: u8, tipo: u8, pacote: u16, intervalo: u8) -> Result<Ep, &'static str>;
    fn bulk(&mut self, disp: u8, ep: &Ep, dados: &mut [u8]) -> Result<usize, &'static str>;
    fn pedir(&mut self, disp: u8, ep: &Ep, tamanho: usize) -> Result<(), &'static str>;
    fn colher(&mut self, disp: u8, ep: &Ep, destino: &mut [u8]) -> Option<usize>;
}

/// Memoria para DMA (zerada, alinhada).
pub(crate) fn dma(tamanho: usize, alinhamento: usize) -> usize {
    let layout = core::alloc::Layout::from_size_align(tamanho.max(1), alinhamento).unwrap();
    let p = unsafe { alloc::alloc::alloc_zeroed(layout) };
    assert!(!p.is_null(), "usb: sem memória");
    p as usize
}

pub(crate) fn liberar_dma(endereco: usize, tamanho: usize, alinhamento: usize) {
    if endereco != 0 {
        let layout = core::alloc::Layout::from_size_align(tamanho.max(1), alinhamento).unwrap();
        unsafe { alloc::alloc::dealloc(endereco as *mut u8, layout) };
    }
}

/// O DMA nao ve a cache: limpa e invalida antes e depois.
pub(crate) fn limpar(endereco: usize, tamanho: usize) {
    crate::mmu::limpar_cache(endereco, tamanho);
}

/// Espera `cond` por ate `ms` milissegundos.
pub(crate) fn esperar(mut cond: impl FnMut() -> bool, ms: u64) -> bool {
    let prazo = crate::cpu::contador() + crate::cpu::frequencia_timer() * ms / 1000;
    loop {
        if cond() {
            return true;
        }
        if crate::cpu::contador() > prazo {
            return false;
        }
    }
}

pub(crate) fn pausa_ms(ms: u64) {
    let alvo = crate::cpu::contador() + crate::cpu::frequencia_timer() * ms / 1000;
    while crate::cpu::contador() < alvo {}
}

struct Teclado {
    hc: usize,
    slot: u8,
    ep: Ep,
    anterior: [u8; 8],
}

struct Mouse {
    hc: usize,
    slot: u8,
    ep: Ep,
    botoes: u8,
}

pub struct Armazenamento {
    hc: usize,
    slot: u8,
    entrada: Ep,
    saida: Ep,
    setores: u64,
    tag: u32,
    nome: String,
}

struct Usb {
    hcs: Vec<Box<dyn Controlador>>,
    teclados: Vec<Teclado>,
    mouses: Vec<Mouse>,
    discos: Vec<Option<Armazenamento>>,
    eventos: VecDeque<EventoCru>,
}

struct Global(UnsafeCell<Option<Usb>>);

// So o laco principal mexe (com a trava do kernel).
unsafe impl Sync for Global {}

static USB: Global = Global(UnsafeCell::new(None));

fn usb() -> Option<&'static mut Usb> {
    unsafe { (*USB.0.get()).as_mut() }
}

/// Liga o controlador e configura o que estiver plugado. Devolve uma linha
/// para o boot (quantos teclados, mouses e discos).
pub fn iniciar() -> Result<String, &'static str> {
    if usb().is_some() {
        return Ok(String::from("ja ligado"));
    }
    let mut u = Usb { hcs: Vec::new(), teclados: Vec::new(), mouses: Vec::new(), discos: Vec::new(), eventos: VecDeque::new() };
    #[cfg(not(feature = "rk322x"))]
    {
        // DWC3: PRTCAPDIR (bits 13:12) = 1, host
        unsafe {
            let g = core::ptr::read_volatile(DWC3_GCTL as *const u32);
            core::ptr::write_volatile(DWC3_GCTL as *mut u32, (g & !(3 << 12)) | 1 << 12);
        }
        u.hcs.push(Box::new(xhci::Xhci::iniciar(BASE_XHCI)?));
        ligar_portas(&mut u, 0);
    }
    #[cfg(feature = "rk322x")]
    {
        // primeiro os EHCI (high speed); o que eles largam aparece no OHCI
        let mut erros = 0;
        for &(e, _) in &HOSTS_RK {
            match ehci::Ehci::iniciar(e) {
                Ok(hc) => {
                    u.hcs.push(Box::new(hc));
                    let k = u.hcs.len() - 1;
                    ligar_portas(&mut u, k);
                }
                Err(_) => erros += 1,
            }
        }
        pausa_ms(100);
        for (k, &(_, o)) in HOSTS_RK.iter().enumerate() {
            // enderecos diferentes em cada OHCI so para o log ficar claro
            match ohci::Ohci::iniciar(o, 1 + 20 * k as u8) {
                Ok(hc) => {
                    u.hcs.push(Box::new(hc));
                    let k = u.hcs.len() - 1;
                    ligar_portas(&mut u, k);
                }
                Err(_) => erros += 1,
            }
        }
        if u.hcs.is_empty() {
            return Err(if erros > 0 { "nenhum controlador USB respondeu" } else { "sem controladores" });
        }
    }
    let linha = alloc::format!("{} teclado(s), {} mouse(s), {} disco(s)", u.teclados.len(), u.mouses.len(), u.discos.len());
    unsafe { *USB.0.get() = Some(u) };
    Ok(linha)
}

fn ligar_portas(u: &mut Usb, hc: usize) {
    for porta in 1..=u.hcs[hc].portas() {
        if let Some((slot, velocidade)) = u.hcs[hc].ligar_porta(porta) {
            let _ = configurar(u, hc, slot, velocidade, porta);
        }
    }
}

/// Le os descritores, escolhe a configuracao e liga as interfaces que o
/// MXOS conhece.
fn configurar(u: &mut Usb, hc: usize, slot: u8, velocidade: u8, porta: u8) -> Result<(), &'static str> {
    let mut d = vec![0u8; 18];
    u.hcs[hc].controle(slot, 0x80, 6, 0x0100, 0, &mut d)?;
    // o pacote maximo do endpoint 0 de verdade (full speed pode ser 8 a 64)
    u.hcs[hc].ajustar_ep0(slot, d[7] as u16)?;
    let mut c = vec![0u8; 9];
    u.hcs[hc].controle(slot, 0x80, 6, 0x0200, 0, &mut c)?;
    let total = u16::from_le_bytes([c[2], c[3]]) as usize;
    let mut c = vec![0u8; total.clamp(9, 1024)];
    u.hcs[hc].controle(slot, 0x80, 6, 0x0200, 0, &mut c)?;
    let configuracao = c[5];
    u.hcs[hc].controle(slot, 0x00, 9, configuracao as u16, 0, &mut [])?;

    // interfaces e endpoints
    let mut k = 0;
    let mut interface: Option<(u8, u8, u8, u8)> = None; // numero, classe, subclasse, protocolo
    let mut eps: Vec<(u8, u8, u8, u16, u8)> = Vec::new(); // interface, endereco, tipo, pacote, intervalo
    while k + 2 <= c.len() {
        let tamanho = c[k] as usize;
        if tamanho < 2 || k + tamanho > c.len() {
            break;
        }
        match c[k + 1] {
            4 => interface = Some((c[k + 2], c[k + 5], c[k + 6], c[k + 7])),
            5 => {
                if let Some((n, ..)) = interface {
                    eps.push((n, c[k + 2], c[k + 3] & 3, u16::from_le_bytes([c[k + 4], c[k + 5]]) & 0x7FF, c[k + 6]));
                }
            }
            _ => {}
        }
        k += tamanho;
    }
    // as interfaces de novo, agora com os endpoints na mao
    let mut k = 0;
    while k + 2 <= c.len() {
        let tamanho = c[k] as usize;
        if tamanho < 2 || k + tamanho > c.len() {
            break;
        }
        if c[k + 1] == 4 {
            let (n, classe, sub, proto) = (c[k + 2], c[k + 5], c[k + 6], c[k + 7]);
            let meus: Vec<_> = eps.iter().filter(|e| e.0 == n).copied().collect();
            match (classe, sub, proto) {
                (3, 1, 1) | (3, 1, 2) => {
                    if let Some(&(_, end, _, pacote, intervalo)) = meus.iter().find(|e| e.2 == 3 && e.1 & 0x80 != 0) {
                        // protocolo de boot e sem repeticao automatica
                        let _ = u.hcs[hc].controle(slot, 0x21, 0x0B, 0, n as u16, &mut []);
                        let _ = u.hcs[hc].controle(slot, 0x21, 0x0A, 0, n as u16, &mut []);
                        let ep = u.hcs[hc].abrir_endpoint(slot, velocidade, end, 3, pacote, intervalo)?;
                        u.hcs[hc].pedir(slot, &ep, 8)?;
                        if proto == 1 {
                            u.teclados.push(Teclado { hc, slot, ep, anterior: [0; 8] });
                        } else {
                            u.mouses.push(Mouse { hc, slot, ep, botoes: 0 });
                        }
                    }
                }
                (8, 6, 0x50) => {
                    let entrada = meus.iter().find(|e| e.2 == 2 && e.1 & 0x80 != 0).copied();
                    let saida = meus.iter().find(|e| e.2 == 2 && e.1 & 0x80 == 0).copied();
                    if let (Some(i), Some(o)) = (entrada, saida) {
                        let ep_in = u.hcs[hc].abrir_endpoint(slot, velocidade, i.1, 2, i.3, 0)?;
                        let ep_out = u.hcs[hc].abrir_endpoint(slot, velocidade, o.1, 2, o.3, 0)?;
                        let nome = if u.discos.is_empty() { String::from("usb") } else { alloc::format!("usb{}", u.discos.len()) };
                        let mut disco = Armazenamento { hc, slot, entrada: ep_in, saida: ep_out, setores: 0, tag: 1, nome };
                        if preparar_disco(&mut *u.hcs[hc], &mut disco).is_ok() {
                            u.discos.push(Some(disco));
                        }
                    }
                }
                _ => {}
            }
        }
        k += tamanho;
    }
    let _ = porta;
    Ok(())
}

// ---------------------------------------------------------------------
// Teclado e mouse

/// HID (uso do teclado) -> codigo evdev.
fn tecla_evdev(uso: u8) -> u16 {
    const LETRAS: [u16; 26] = [30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44];
    match uso {
        0x04..=0x1D => LETRAS[(uso - 0x04) as usize],
        0x1E..=0x26 => (uso - 0x1E + 2) as u16,
        0x27 => 11,
        0x28 => 28,
        0x29 => 1,
        0x2A => 14,
        0x2B => 15,
        0x2C => 57,
        0x2D => 12,
        0x2E => 13,
        0x2F => 26,
        0x30 => 27,
        0x31 | 0x32 => 43,
        0x33 => 39,
        0x34 => 40,
        0x35 => 41,
        0x36 => 51,
        0x37 => 52,
        0x38 => 53,
        0x39 => 58,
        0x3A..=0x43 => (uso - 0x3A + 59) as u16,
        0x44 => 87,
        0x45 => 88,
        0x46 => 99,
        0x47 => 70,
        0x48 => 119,
        0x49 => 110,
        0x4A => 102,
        0x4B => 104,
        0x4C => 111,
        0x4D => 107,
        0x4E => 109,
        0x4F => 106,
        0x50 => 105,
        0x51 => 108,
        0x52 => 103,
        0x53 => 69,
        0x54 => 98,
        0x55 => 55,
        0x56 => 74,
        0x57 => 78,
        0x58 => 96,
        0x59..=0x61 => [79, 80, 81, 75, 76, 77, 71, 72, 73][(uso - 0x59) as usize],
        0x62 => 82,
        0x63 => 83,
        0x64 => 86,
        0x65 => 127,
        0x87 => 89,
        _ => 0,
    }
}

const MODIFICADORES: [u16; 8] = [29, 42, 56, 125, 97, 54, 100, 126];

fn tecla(eventos: &mut VecDeque<EventoCru>, codigo: u16, apertada: bool) {
    if codigo != 0 {
        eventos.push_back(EventoCru { tipo: EV_KEY, codigo, valor: apertada as u32 });
        eventos.push_back(EventoCru { tipo: EV_SYN, codigo: SYN_REPORT, valor: 0 });
    }
}

/// Compara o relatorio novo do teclado com o anterior.
fn relatorio_teclado(eventos: &mut VecDeque<EventoCru>, antes: &[u8; 8], agora: &[u8; 8]) {
    for bit in 0..8 {
        let (a, b) = (antes[0] >> bit & 1, agora[0] >> bit & 1);
        if a != b {
            tecla(eventos, MODIFICADORES[bit], b == 1);
        }
    }
    for &uso in &antes[2..8] {
        if uso > 1 && !agora[2..8].contains(&uso) {
            tecla(eventos, tecla_evdev(uso), false);
        }
    }
    for &uso in &agora[2..8] {
        if uso > 1 && !antes[2..8].contains(&uso) {
            tecla(eventos, tecla_evdev(uso), true);
        }
    }
}

/// Le o que chegou dos teclados e mouses. O entrada/mod.rs chama sempre.
pub fn colher_entrada() -> Vec<EventoCru> {
    let Some(u) = usb() else { return Vec::new() };
    let mut dados = [0u8; 8];
    for k in 0..u.teclados.len() {
        let (hc, slot, ep) = (u.teclados[k].hc, u.teclados[k].slot, u.teclados[k].ep);
        if let Some(n) = u.hcs[hc].colher(slot, &ep, &mut dados) {
            if n >= 8 {
                let antes = u.teclados[k].anterior;
                relatorio_teclado(&mut u.eventos, &antes, &dados);
                u.teclados[k].anterior = dados;
            }
            let _ = u.hcs[hc].pedir(slot, &ep, 8);
        }
    }
    for k in 0..u.mouses.len() {
        let (hc, slot, ep) = (u.mouses[k].hc, u.mouses[k].slot, u.mouses[k].ep);
        if let Some(n) = u.hcs[hc].colher(slot, &ep, &mut dados) {
            if n >= 3 {
                let botoes = dados[0];
                let antes = u.mouses[k].botoes;
                for (bit, codigo) in [(0, BTN_LEFT), (1, BTN_RIGHT), (2, BTN_MIDDLE)] {
                    if (antes ^ botoes) >> bit & 1 != 0 {
                        u.eventos.push_back(EventoCru { tipo: EV_KEY, codigo, valor: (botoes >> bit & 1) as u32 });
                    }
                }
                u.mouses[k].botoes = botoes;
                let (dx, dy) = (dados[1] as i8 as i32, dados[2] as i8 as i32);
                if dx != 0 {
                    u.eventos.push_back(EventoCru { tipo: EV_REL, codigo: REL_X, valor: dx as u32 });
                }
                if dy != 0 {
                    u.eventos.push_back(EventoCru { tipo: EV_REL, codigo: REL_Y, valor: dy as u32 });
                }
                if n >= 4 && dados[3] != 0 {
                    u.eventos.push_back(EventoCru { tipo: EV_REL, codigo: REL_WHEEL, valor: dados[3] as i8 as i32 as u32 });
                }
                u.eventos.push_back(EventoCru { tipo: EV_SYN, codigo: SYN_REPORT, valor: 0 });
            }
            let _ = u.hcs[hc].pedir(slot, &ep, 8);
        }
    }
    u.eventos.drain(..).collect()
}

/// Ha teclado ou mouse USB?
pub fn tem_entrada() -> bool {
    usb().is_some_and(|u| !u.teclados.is_empty() || !u.mouses.is_empty())
}

// ---------------------------------------------------------------------
// Pendrive: SCSI por bulk-only (CBW, dados, CSW)

fn scsi(hc: &mut dyn Controlador, d: &mut Armazenamento, comando: &[u8], dados: &mut [u8], escrita: bool) -> Result<(), Erro> {
    let mut cbw = [0u8; 31];
    cbw[0..4].copy_from_slice(&0x4342_5355u32.to_le_bytes());
    cbw[4..8].copy_from_slice(&d.tag.to_le_bytes());
    d.tag = d.tag.wrapping_add(1);
    cbw[8..12].copy_from_slice(&(dados.len() as u32).to_le_bytes());
    cbw[12] = if escrita { 0 } else { 0x80 };
    cbw[14] = comando.len() as u8;
    cbw[15..15 + comando.len()].copy_from_slice(comando);
    hc.bulk(d.slot, &d.saida, &mut cbw).map_err(|_| Erro::Disco)?;
    if !dados.is_empty() {
        let ep = if escrita { d.saida } else { d.entrada };
        hc.bulk(d.slot, &ep, dados).map_err(|_| Erro::Disco)?;
    }
    let mut csw = [0u8; 13];
    hc.bulk(d.slot, &d.entrada, &mut csw).map_err(|_| Erro::Disco)?;
    if csw[0..4] != 0x5342_5355u32.to_le_bytes() || csw[12] != 0 {
        return Err(Erro::Disco);
    }
    Ok(())
}

fn preparar_disco(hc: &mut dyn Controlador, d: &mut Armazenamento) -> Result<(), Erro> {
    let mut inq = [0u8; 36];
    scsi(hc, d, &[0x12, 0, 0, 0, 36, 0], &mut inq, false)?;
    // o pendrive pode demorar a ficar pronto
    for _ in 0..20 {
        if scsi(hc, d, &[0; 6], &mut [], false).is_ok() {
            break;
        }
        let alvo = crate::cpu::contador() + crate::cpu::frequencia_timer() / 10;
        while crate::cpu::contador() < alvo {}
    }
    let mut cap = [0u8; 8];
    scsi(hc, d, &[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0], &mut cap, false)?;
    let ultimo = u32::from_be_bytes([cap[0], cap[1], cap[2], cap[3]]) as u64;
    let bloco = u32::from_be_bytes([cap[4], cap[5], cap[6], cap[7]]);
    if bloco != 512 {
        return Err(Erro::Corrompido("pendrive com bloco diferente de 512"));
    }
    d.setores = ultimo + 1;
    Ok(())
}

/// Um pendrive visto como Disco (o armazenamento so fala com ele por aqui).
pub struct DiscoUsb {
    indice: usize,
    nome: String,
    setores: u64,
}

/// Os pendrives achados no iniciar (cada um sai uma vez so).
pub fn discos() -> Vec<Box<dyn Disco>> {
    let Some(u) = usb() else { return Vec::new() };
    let mut lista: Vec<Box<dyn Disco>> = Vec::new();
    for (indice, d) in u.discos.iter().enumerate() {
        if let Some(d) = d {
            lista.push(Box::new(DiscoUsb { indice, nome: d.nome.clone(), setores: d.setores }));
        }
    }
    lista
}

fn com_disco<R>(indice: usize, f: impl FnOnce(&mut dyn Controlador, &mut Armazenamento) -> Result<R, Erro>) -> Result<R, Erro> {
    let u = usb().ok_or(Erro::SemDisco)?;
    let Usb { hcs, discos, .. } = u;
    let d = discos.get_mut(indice).and_then(Option::as_mut).ok_or(Erro::SemDisco)?;
    let hc = hcs.get_mut(d.hc).ok_or(Erro::SemDisco)?;
    f(&mut **hc, d)
}

/// Blocos por comando READ(10)/WRITE(10).
const BLOCOS_POR_VEZ: usize = 64;

impl Disco for DiscoUsb {
    fn nome(&self) -> &str {
        &self.nome
    }

    fn setores(&self) -> u64 {
        self.setores
    }

    fn ler(&mut self, setor: u64, buffer: &mut [u8]) -> Result<(), Erro> {
        com_disco(self.indice, |hc, d| {
            let mut feito = 0;
            while feito < buffer.len() {
                let blocos = ((buffer.len() - feito) / 512).min(BLOCOS_POR_VEZ);
                let lba = (setor + (feito / 512) as u64) as u32;
                let l = lba.to_be_bytes();
                let cdb = [0x28, 0, l[0], l[1], l[2], l[3], 0, (blocos >> 8) as u8, blocos as u8, 0];
                scsi(hc, d, &cdb, &mut buffer[feito..feito + blocos * 512], false)?;
                feito += blocos * 512;
            }
            Ok(())
        })
    }

    fn escrever(&mut self, setor: u64, dados: &[u8]) -> Result<(), Erro> {
        com_disco(self.indice, |hc, d| {
            let mut feito = 0;
            while feito < dados.len() {
                let blocos = ((dados.len() - feito) / 512).min(BLOCOS_POR_VEZ);
                let lba = (setor + (feito / 512) as u64) as u32;
                let l = lba.to_be_bytes();
                let cdb = [0x2A, 0, l[0], l[1], l[2], l[3], 0, (blocos >> 8) as u8, blocos as u8, 0];
                let mut pedaco = dados[feito..feito + blocos * 512].to_vec();
                scsi(hc, d, &cdb, &mut pedaco, true)?;
                feito += blocos * 512;
            }
            Ok(())
        })
    }
}
