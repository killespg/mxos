//! Rede cabeada das TV boxes: o Synopsys DWMAC 3.x (o "stmmac" do Linux),
//! o mesmo IP na Amlogic GX e na Rockchip RK322x, os dois com um PHY
//! interno de 10/100. So muda a preparacao (relogios, seletor do PHY):
//!   - Amlogic GXL: liga o relogio e escolhe o PHY interno no MDIO
//!   - RK322x: MAC em RMII com 50 MHz tirados de um PLL (o divisor e
//!     calculado lendo o PLL), PHY integrado ligado pelo GRF e o ajuste
//!     analogico que o PHY da Rockchip pede
//! NUNCA TESTADO NO HARDWARE.
//!
//! O MAC vem do que o U-Boot deixou nos registradores (ou um local fixo).
//! Aneis de 32 descritores "enhanced" para receber e 32 para enviar, com
//! buffers de 1536 bytes no heap. O DMA nao ve a cache da CPU: cada
//! descritor ocupa uma linha de cache inteira (DSL pula o resto), entao
//! limpar um nunca escreve por cima do que o DMA acabou de por no vizinho.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};

use super::virtio_net::MAX_QUADRO;
use super::{ErroRede, Placa};
use crate::{cpu, mmu};

#[cfg(feature = "mxq")]
const BASE: usize = 0xC941_0000;
#[cfg(feature = "rk322x")]
const BASE: usize = 0x3020_0000;
#[cfg(not(any(feature = "mxq", feature = "rk322x")))]
const BASE: usize = 0;

#[cfg(feature = "mxq")]
const PHY: u32 = 8;
#[cfg(not(feature = "mxq"))]
const PHY: u32 = 0;

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
/// Palavras puladas depois de cada descritor (16 bytes + 48 = 64, uma linha).
const DSL: u32 = 12;

#[repr(C, align(64))]
#[derive(Clone, Copy, Default)]
struct Descritor {
    d0: u32,
    d1: u32,
    buffer: u32,
    proximo: u32,
    _resto: [u32; 12],
}

/// Buffer de quadro alinhado na linha de cache (nada mais divide a linha).
#[repr(C, align(64))]
struct Buf([u8; BUFFER]);

pub struct Dwmac {
    mac: [u8; 6],
    rx: Box<[Descritor; DESCRITORES]>,
    tx: Box<[Descritor; DESCRITORES]>,
    buffers_rx: Vec<Box<Buf>>,
    buffers_tx: Vec<Box<Buf>>,
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

#[cfg(feature = "mxq")]
mod plataforma {
    use core::ptr::{read_volatile, write_volatile};

    /// Liga o relogio do Ethernet (HHI_GCLK_MPEG1, bit 3).
    const HHI_GCLK_MPEG1: usize = 0xC883_C144;
    /// Seletor do MDIO do GXL: este valor escolhe o PHY interno.
    const MDIO_MUX: usize = 0xC883_4558;
    const MDIO_INTERNO: u32 = 0xE409_08FF;

    pub fn preparar() -> Result<(), &'static str> {
        unsafe {
            let g = read_volatile(HHI_GCLK_MPEG1 as *const u32);
            write_volatile(HHI_GCLK_MPEG1 as *mut u32, g | 1 << 3);
            write_volatile(MDIO_MUX as *mut u32, MDIO_INTERNO);
        }
        Ok(())
    }

    pub fn depois_do_link(_cem: bool) {}

    pub fn ajustar_phy() {}
}

#[cfg(feature = "rk322x")]
mod plataforma {
    use core::ptr::{read_volatile, write_volatile};

    use super::{PHY, esperar_us, mdio_escrever, mdio_ler};

    const CRU: usize = 0x110E_0000;
    const GRF: usize = 0x1100_0000;
    const MODO_PLL: usize = 0x40;
    /// CLKSEL_CON(5): sclk_gmac_src (mux bit 7: cpll/gpll; divisor 4:0) e
    /// sclk_gmac_pre (bit 5: 0 = interno).
    const CLKSEL5: usize = 0x44 + 5 * 4;
    /// CLKSEL_CON(29): sclk_macphy (mux bit 12: sclk_gmac_src; divisor 9:8).
    const CLKSEL29: usize = 0x44 + 29 * 4;
    const GATE1: usize = 0xD0 + 4;
    const GATE5: usize = 0xD0 + 5 * 4;
    const GATE11: usize = 0xD0 + 11 * 4;
    /// SOFTRST_CON(3): bit 8 = GMAC (56), bit 15 = MACPHY (63).
    const RESET3: usize = 0x110 + 3 * 4;
    const GRF_MUX: usize = 0x50;
    const GRF_MAC_CON1: usize = 0x904;
    const GRF_MACPHY: usize = 0xB00;
    const ALVO_HZ: u64 = 50_000_000;

    fn ler(e: usize) -> u32 {
        unsafe { read_volatile(e as *const u32) }
    }

    /// Os registradores da Rockchip tem mascara de escrita nos 16 bits altos.
    fn por(e: usize, mascara: u32, valor: u32) {
        unsafe { write_volatile(e as *mut u32, mascara << 16 | (valor & mascara)) }
    }

    /// Frequencia de um PLL do tipo rk3036 (CON0..2), ou 24 MHz se o PLL
    /// esta em modo lento.
    fn freq_pll(con: usize, bit_modo: u32) -> u64 {
        if ler(CRU + MODO_PLL) >> bit_modo & 1 == 0 {
            return 24_000_000;
        }
        let (c0, c1, c2) = (ler(CRU + con), ler(CRU + con + 4), ler(CRU + con + 8));
        let fb = (c0 & 0xFFF) as u64;
        let pd1 = ((c0 >> 12) & 7).max(1) as u64;
        let refdiv = (c1 & 0x3F).max(1) as u64;
        let pd2 = ((c1 >> 6) & 7).max(1) as u64;
        let frac = if c1 >> 12 & 1 == 0 { (c2 & 0xFF_FFFF) as u64 } else { 0 };
        (24_000_000 * fb + ((24_000_000 * frac) >> 24)) / (refdiv * pd1 * pd2)
    }

    pub fn preparar() -> Result<(), &'static str> {
        // 50 MHz para o RMII: o PLL (cpll ou gpll) e o divisor mais certos
        let plls = [(0usize, freq_pll(6 * 4, 8)), (1, freq_pll(9 * 4, 12))];
        let mut melhor: Option<(u32, u32, u64)> = None;
        for (mux, f) in plls {
            for div in 1..=32u64 {
                let erro = (f / div).abs_diff(ALVO_HZ);
                if melhor.is_none_or(|m| erro < m.2) {
                    melhor = Some((mux as u32, div as u32, erro));
                }
            }
        }
        let (mux, div, erro) = melhor.ok_or("sem PLL")?;
        if erro > ALVO_HZ / 50 {
            return Err("nenhum PLL dá 50 MHz para a rede");
        }
        por(CRU + CLKSEL5, 1 << 7 | 1 << 5 | 0x1F, mux << 7 | (div - 1));
        por(CRU + CLKSEL29, 1 << 12 | 3 << 8, 0);
        // abre os relogios (bit 1 = desligado)
        por(CRU + GATE1, 1 << 7, 0);
        por(CRU + GATE5, 0x1F << 3, 0);
        por(CRU + GATE11, 3 << 4, 0);
        // MAC em RMII (interface 4, modo RMII) e o PHY integrado
        por(GRF + GRF_MAC_CON1, 7 << 4 | 1 << 10 | 1 << 11, 4 << 4 | 1 << 10 | 1 << 11);
        por(GRF + GRF_MUX, 1 << 15, 1 << 15);
        por(GRF + GRF_MACPHY, 1 << 14, 1 << 14); // relogio de 50 MHz
        por(GRF + GRF_MACPHY, 3 << 6, 1 << 6); // GMAC <-> PHY em RMII
        por(GRF + GRF_MACPHY + 8, 0xFFFF, 0x1234); // ID do PHY
        por(GRF + GRF_MACPHY + 12, 0x3F, 0x35);
        // reinicia o PHY desligado, depois liga
        por(GRF + GRF_MACPHY, 1, 0);
        por(CRU + RESET3, 1 << 15 | 1 << 8, 1 << 15 | 1 << 8);
        esperar_us(20);
        por(CRU + RESET3, 1 << 15 | 1 << 8, 0);
        esperar_us(20);
        por(GRF + GRF_MACPHY, 1, 1);
        esperar_us(30_000);
        Ok(())
    }

    /// O PHY integrado: sem auto MDIX (falha em algumas placas) e com a
    /// amplitude de transmissao ajustada (registradores de teste).
    pub fn ajustar_phy() {
        let v = mdio_ler(PHY, 17);
        mdio_escrever(PHY, 17, v & !(1 << 7));
        analogico();
    }

    fn analogico() {
        mdio_escrever(PHY, 20, 0x400);
        mdio_escrever(PHY, 20, 0);
        mdio_escrever(PHY, 20, 0x400);
        mdio_escrever(PHY, 23, 0xB);
        mdio_escrever(PHY, 20, 1 << 14 | 1 << 10 | 0x18);
        mdio_escrever(PHY, 20, 0);
    }

    /// Velocidade do RMII no GRF (o ajuste analogico se perde ao mudar para 100).
    pub fn depois_do_link(cem: bool) {
        por(GRF + GRF_MAC_CON1, 1 << 7 | 1 << 2, if cem { 1 << 7 | 1 << 2 } else { 0 });
        if cem {
            analogico();
        }
    }
}

#[cfg(not(any(feature = "mxq", feature = "rk322x")))]
mod plataforma {
    pub fn preparar() -> Result<(), &'static str> {
        Err("sem DWMAC")
    }
    pub fn depois_do_link(_cem: bool) {}
    pub fn ajustar_phy() {}
}

impl Dwmac {
    pub fn iniciar() -> Result<Dwmac, ErroRede> {
        plataforma::preparar().map_err(ErroRede::Protocolo)?;
        // o MAC que o U-Boot programou (antes do reset apagar)
        let (alto, baixo) = (ler(MAC_ALTO), ler(MAC_BAIXO));
        let mut mac = [baixo as u8, (baixo >> 8) as u8, (baixo >> 16) as u8, (baixo >> 24) as u8, alto as u8, (alto >> 8) as u8];
        if mac == [0; 6] || mac == [0xFF; 6] {
            // local e unico o bastante: "MX" + bits do relogio
            let c = cpu::contador();
            mac = [0x02, 0x4D, 0x58, (c >> 16) as u8, (c >> 8) as u8, c as u8 | 1];
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
            return Err(ErroRede::Protocolo("o DMA da placa de rede não saiu do reset (relógio do MAC desligado?)"));
        }
        // o PHY responde no MDIO? (0 ou 0xFFFF = ninguem do outro lado)
        let id = mdio_ler(PHY, 2);
        if id == 0 || id == 0xFFFF {
            return Err(ErroRede::Protocolo("o PHY da porta de rede não responde (MDIO)"));
        }
        plataforma::ajustar_phy();
        // PHY: reinicia a negociacao e espera o link (ate 8 s: depois de um
        // reset do PHY a negociacao passa facil de 3 s)
        mdio_escrever(PHY, 0, 1 << 12 | 1 << 9);
        let mut link = false;
        for _ in 0..800 {
            if mdio_ler(PHY, 1) & (1 << 2) != 0 {
                link = true;
                break;
            }
            esperar_us(10_000);
        }
        if !link {
            return Err(ErroRede::Protocolo("sem link no cabo de rede (cabo desligado, ou o PHY não negociou)"));
        }
        let parceiro = mdio_ler(PHY, 5);
        let cem = parceiro & (1 << 8 | 1 << 7) != 0;
        let duplex = parceiro & (1 << 8 | 1 << 6) != 0;
        plataforma::depois_do_link(cem);

        let mut placa = Dwmac {
            mac,
            rx: Box::new([Descritor::default(); DESCRITORES]),
            tx: Box::new([Descritor::default(); DESCRITORES]),
            buffers_rx: (0..DESCRITORES).map(|_| Box::new(Buf([0u8; BUFFER]))).collect(),
            buffers_tx: (0..DESCRITORES).map(|_| Box::new(Buf([0u8; BUFFER]))).collect(),
            proximo_rx: 0,
            proximo_tx: 0,
        };
        for k in 0..DESCRITORES {
            let fim = if k == DESCRITORES - 1 { 1 << 15 } else { 0 }; // RER
            placa.rx[k] = Descritor { d0: DONO, d1: fim | BUFFER as u32, buffer: placa.buffers_rx[k].0.as_ptr() as u32, ..Default::default() };
            let fim_tx = if k == DESCRITORES - 1 { 1 << 21 } else { 0 }; // TER
            placa.tx[k] = Descritor { d0: fim_tx, buffer: placa.buffers_tx[k].0.as_ptr() as u32, ..Default::default() };
            mmu::limpar_cache(placa.buffers_rx[k].0.as_ptr() as usize, BUFFER);
        }
        limpar(&*placa.rx);
        limpar(&*placa.tx);
        escrever(MAC_ALTO, mac[4] as u32 | (mac[5] as u32) << 8);
        escrever(MAC_BAIXO, u32::from_le_bytes([mac[0], mac[1], mac[2], mac[3]]));
        escrever(MAC_FILTRO, 1 << 31 | 1); // recebe tudo (o IP filtra)
        escrever(DMA_RX_LISTA, &*placa.rx as *const _ as usize as u32);
        escrever(DMA_TX_LISTA, &*placa.tx as *const _ as usize as u32);
        escrever(DMA_IRQ, 0);
        // rajada de 8; descritores "enhanced" de 4 palavras + DSL pulando o resto da linha
        escrever(DMA_BUS, 8 << 8 | DSL << 2);
        escrever(DMA_MODO, 1 << 25 | 1 << 21 | 1 << 13 | 1 << 1); // RSF, TSF, ST, SR
        let velocidade = if cem { 1 << 14 } else { 0 };
        let duplo = if duplex { 1 << 11 } else { 0 };
        // MII (PS), velocidade, duplex, transmissor e receptor
        escrever(MAC_CONFIG, 1 << 15 | velocidade | duplo | 1 << 3 | 1 << 2);
        Ok(placa)
    }
}

impl Placa for Dwmac {
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
            mmu::limpar_cache(self.buffers_rx[k].0.as_ptr() as usize, BUFFER);
            destino[..n].copy_from_slice(&self.buffers_rx[k].0[..n]);
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
            self.buffers_tx[k].0[n..n + p.len()].copy_from_slice(p);
            n += p.len();
        }
        let n = n.max(60);
        mmu::limpar_cache(self.buffers_tx[k].0.as_ptr() as usize, BUFFER);
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
