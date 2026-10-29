//! Rede cabeada da TV box: o Ethernet da Amlogic GX (Synopsys DWMAC 3.70,
//! o "stmmac" do Linux) com o PHY interno de 10/100 do S905W. NUNCA TESTADO
//! NO HARDWARE.
//!
//! O MAC vem do que o U-Boot deixou nos registradores (ou um local fixo).
//! Aneis de 32 descritores "enhanced" para receber e 32 para enviar, com
//! buffers de 1536 bytes no heap. O DMA da Amlogic nao ve o cache da CPU:
//! cada descritor e buffer e limpo (dc civac) antes e depois do DMA.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};

use super::virtio_net::MAX_QUADRO;
use super::{ErroRede, Placa};
use crate::{cpu, mmu};

const BASE: usize = 0xC941_0000;
/// Liga o relogio do Ethernet (HHI_GCLK_MPEG1, bit 3).
const HHI_GCLK_MPEG1: usize = 0xC883_C144;
/// Seletor do MDIO do GXL: este valor escolhe o PHY interno.
const MDIO_MUX: usize = 0xC883_4558;
const MDIO_INTERNO: u32 = 0xE409_08FF;
const PHY_INTERNO: u32 = 8;

const MAC_CONFIG: usize = 0x0000;
const MAC_FILTRO: usize = 0x0004;
const MII_ENDERECO: usize = 0x0010;
const MII_DADO: usize = 0x0014;
const MAC_ALTO: usize = 0x0040;
const MAC_BAIXO: usize = 0x0044;
const DMA_BUS: usize = 0x1000;
const DMA_TX_POLL: usize = 0x1004;
const DMA_RX_POLL: usize = 0x1008;
const DMA_RX_LISTA: usize = 0x100C;
const DMA_TX_LISTA: usize = 0x1010;
const DMA_STATUS: usize = 0x1014;
const DMA_MODO: usize = 0x1018;
const DMA_IRQ: usize = 0x101C;

const DONO: u32 = 1 << 31;
const DESCRITORES: usize = 32;
const BUFFER: usize = 1536;

#[repr(C, align(16))]
#[derive(Clone, Copy, Default)]
struct Descritor {
    d0: u32,
    d1: u32,
    buffer: u32,
    proximo: u32,
}

pub struct MesonEth {
    mac: [u8; 6],
    rx: Box<[Descritor; DESCRITORES]>,
    tx: Box<[Descritor; DESCRITORES]>,
    buffers_rx: Vec<Box<[u8; BUFFER]>>,
    buffers_tx: Vec<Box<[u8; BUFFER]>>,
    proximo_rx: usize,
    proximo_tx: usize,
}

fn ler(r: usize) -> u32 {
    unsafe { read_volatile((BASE + r) as *const u32) }
}

fn escrever(r: usize, v: u32) {
    unsafe { write_volatile((BASE + r) as *mut u32, v) }
}

fn esperar_us(us: u64) {
    let alvo = cpu::contador() + cpu::frequencia_timer() * us / 1_000_000;
    while cpu::contador() < alvo {
        core::hint::spin_loop();
    }
}

fn limpar<T>(x: &T) {
    mmu::limpar_cache(x as *const T as usize, core::mem::size_of::<T>());
}

/// Le um registrador do PHY pelo MDIO.
fn mdio_ler(phy: u32, reg: u32) -> u32 {
    escrever(MII_ENDERECO, phy << 11 | reg << 6 | 4 << 2 | 1);
    for _ in 0..10_000 {
        if ler(MII_ENDERECO) & 1 == 0 {
            break;
        }
        esperar_us(10);
    }
    ler(MII_DADO) & 0xFFFF
}

fn mdio_escrever(phy: u32, reg: u32, valor: u32) {
    escrever(MII_DADO, valor);
    escrever(MII_ENDERECO, phy << 11 | reg << 6 | 4 << 2 | 2 | 1);
    for _ in 0..10_000 {
        if ler(MII_ENDERECO) & 1 == 0 {
            break;
        }
        esperar_us(10);
    }
}

impl MesonEth {
    pub fn iniciar() -> Result<MesonEth, ErroRede> {
        unsafe {
            let g = read_volatile(HHI_GCLK_MPEG1 as *const u32);
            write_volatile(HHI_GCLK_MPEG1 as *mut u32, g | 1 << 3);
            write_volatile(MDIO_MUX as *mut u32, MDIO_INTERNO);
        }
        // o MAC que o U-Boot programou (antes do reset apagar)
        let (alto, baixo) = (ler(MAC_ALTO), ler(MAC_BAIXO));
        let mut mac = [baixo as u8, (baixo >> 8) as u8, (baixo >> 16) as u8, (baixo >> 24) as u8, alto as u8, (alto >> 8) as u8];
        if mac == [0; 6] || mac == [0xFF; 6] {
            mac = [0x02, 0x4D, 0x58, 0x4F, 0x53, 0x01]; // local: "MXOS"
        }
        // reset do DMA
        escrever(DMA_BUS, 1);
        for _ in 0..100_000 {
            if ler(DMA_BUS) & 1 == 0 {
                break;
            }
            esperar_us(10);
        }
        if ler(DMA_BUS) & 1 != 0 {
            return Err(ErroRede::SemRede);
        }
        // PHY: reinicia a negociacao e espera o link (ate 3 s)
        mdio_escrever(PHY_INTERNO, 0, 1 << 12 | 1 << 9);
        let mut link = false;
        for _ in 0..300 {
            if mdio_ler(PHY_INTERNO, 1) & (1 << 2) != 0 {
                link = true;
                break;
            }
            esperar_us(10_000);
        }
        if !link {
            return Err(ErroRede::SemRede);
        }
        let parceiro = mdio_ler(PHY_INTERNO, 5);
        let cem = parceiro & (1 << 8 | 1 << 7) != 0;
        let duplex = parceiro & (1 << 8 | 1 << 6) != 0;

        let mut placa = MesonEth {
            mac,
            rx: Box::new([Descritor::default(); DESCRITORES]),
            tx: Box::new([Descritor::default(); DESCRITORES]),
            buffers_rx: (0..DESCRITORES).map(|_| Box::new([0u8; BUFFER])).collect(),
            buffers_tx: (0..DESCRITORES).map(|_| Box::new([0u8; BUFFER])).collect(),
            proximo_rx: 0,
            proximo_tx: 0,
        };
        for k in 0..DESCRITORES {
            let fim = if k == DESCRITORES - 1 { 1 << 15 } else { 0 }; // RER
            placa.rx[k] = Descritor { d0: DONO, d1: fim | BUFFER as u32, buffer: placa.buffers_rx[k].as_ptr() as u32, proximo: 0 };
            let fim_tx = if k == DESCRITORES - 1 { 1 << 21 } else { 0 }; // TER
            placa.tx[k] = Descritor { d0: fim_tx, d1: 0, buffer: placa.buffers_tx[k].as_ptr() as u32, proximo: 0 };
            mmu::limpar_cache(placa.buffers_rx[k].as_ptr() as usize, BUFFER);
        }
        limpar(&*placa.rx);
        limpar(&*placa.tx);
        escrever(MAC_ALTO, mac[4] as u32 | (mac[5] as u32) << 8);
        escrever(MAC_BAIXO, u32::from_le_bytes([mac[0], mac[1], mac[2], mac[3]]));
        escrever(MAC_FILTRO, 1 << 31 | 1); // recebe tudo (o IP filtra)
        escrever(DMA_RX_LISTA, &*placa.rx as *const _ as u32);
        escrever(DMA_TX_LISTA, &*placa.tx as *const _ as u32);
        escrever(DMA_IRQ, 0);
        // descritores de 8 palavras nao; rajada de 8; enhanced (ATDS desligado)
        escrever(DMA_BUS, 8 << 8);
        escrever(DMA_MODO, 1 << 25 | 1 << 21 | 1 << 13 | 1 << 1); // RSF, TSF, ST, SR
        let velocidade = if cem { 1 << 14 } else { 0 };
        let duplo = if duplex { 1 << 11 } else { 0 };
        // MII (PS), velocidade, duplex, transmissor e receptor
        escrever(MAC_CONFIG, 1 << 15 | velocidade | duplo | 1 << 3 | 1 << 2);
        Ok(placa)
    }
}

impl Placa for MesonEth {
    fn mac(&self) -> [u8; 6] {
        self.mac
    }

    fn receber(&mut self, destino: &mut [u8; MAX_QUADRO]) -> Option<usize> {
        let k = self.proximo_rx;
        limpar(&self.rx[k]);
        let d = unsafe { read_volatile(&self.rx[k]) };
        if d.d0 & DONO != 0 {
            return None;
        }
        let tamanho = ((d.d0 >> 16) & 0x3FFF) as usize;
        let ok = d.d0 & (1 << 15) == 0 && tamanho >= 4;
        // sem o CRC (4 bytes) no fim
        let n = tamanho.saturating_sub(4).min(MAX_QUADRO);
        if ok {
            mmu::limpar_cache(self.buffers_rx[k].as_ptr() as usize, BUFFER);
            destino[..n].copy_from_slice(&self.buffers_rx[k][..n]);
        }
        // devolve o descritor para a placa
        self.rx[k].d0 = DONO;
        limpar(&self.rx[k]);
        escrever(DMA_RX_POLL, 1);
        self.proximo_rx = (k + 1) % DESCRITORES;
        if ok { Some(n) } else { self.receber(destino) }
    }

    fn enviar(&mut self, partes: &[&[u8]]) -> bool {
        let k = self.proximo_tx;
        limpar(&self.tx[k]);
        if unsafe { read_volatile(&self.tx[k]) }.d0 & DONO != 0 {
            return false; // anel cheio
        }
        let mut n = 0;
        for p in partes {
            if n + p.len() > BUFFER {
                return false;
            }
            self.buffers_tx[k][n..n + p.len()].copy_from_slice(p);
            n += p.len();
        }
        let n = n.max(60);
        mmu::limpar_cache(self.buffers_tx[k].as_ptr() as usize, BUFFER);
        let fim = if k == DESCRITORES - 1 { 1 << 21 } else { 0 };
        self.tx[k].d1 = n as u32;
        // dono, primeiro e ultimo pedaco do quadro
        self.tx[k].d0 = DONO | 1 << 29 | 1 << 28 | fim;
        limpar(&self.tx[k]);
        escrever(DMA_TX_POLL, 1);
        let _ = ler(DMA_STATUS);
        self.proximo_tx = (k + 1) % DESCRITORES;
        true
    }

    fn encerrar(self: Box<Self>) {
        escrever(DMA_MODO, 0);
        escrever(MAC_CONFIG, 0);
        // os buffers so voltam ao heap depois do DMA parado (aqui, no drop)
    }
}
