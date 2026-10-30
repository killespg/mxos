//! RKVDEC da RK322x (0x20030000): o decodificador de video por hardware
//! (H.264 ate 4K; o mesmo bloco faz H.265 e VP9). E o que o Android usa
//! para o YouTube: a CPU so prepara os parametros, o hardware le a fatia
//! (cabecalho e CABAC) e escreve o quadro NV12 na RAM.
//!
//! Registradores, tabela de SPS/PPS e RPS conferidos no driver rkvdec do
//! Linux (variante rk3399, que o rk3228 segue) e na box: ID 0x6876 com os
//! relogios ligados. Sem a IOMMU (desligada no boot): enderecos fisicos.
//!
//! O que fica na CPU (POC, DPB, listas) esta em midia::h264. Aqui so o
//! hardware: buffers, tabelas, registradores e a espera pelo fim.

use alloc::vec::Vec;
use core::alloc::Layout;
use core::ptr::{read_volatile, write_volatile};

use crate::midia::h264::{self, Dpb, Fatia, Pps, Quadro, Ref, Sps};

const BASE: usize = 0x2003_0000;
const CRU: usize = 0x110E_0000;

const REG_INT: usize = 1;
const INT_DEC_E: u32 = 1 << 0;
const INT_CLKGATE_E: u32 = 1 << 1;
const INT_TIMEOUT_E: u32 = 1 << 5;
const INT_BUF_EMPTY_E: u32 = 1 << 6;
const INT_RDY: u32 = 1 << 12;
const INT_BUS: u32 = 1 << 13;
const INT_ERRO: u32 = 1 << 14;
const INT_TEMPO: u32 = 1 << 15;
const INT_VAZIO: u32 = 1 << 16;
const INT_COLMV: u32 = 1 << 17;

fn ler(r: usize) -> u32 {
    unsafe { read_volatile((BASE + 4 * r) as *const u32) }
}

fn escrever(r: usize, v: u32) {
    unsafe { write_volatile((BASE + 4 * r) as *mut u32, v) }
}

fn cru(r: usize, mascara: u32, valor: u32) {
    unsafe { write_volatile((CRU + r) as *mut u32, mascara << 16 | valor) }
}

/// Liga relogios (300 MHz do GPLL) e solta os resets. true = o ID bate.
pub fn ligar() -> bool {
    // divisores: aclk_rkvdec_pre e sclk_vdec_cabac (CLKSEL28) e
    // sclk_vdec_core (CLKSEL34), mux 1 = GPLL, para ~300 MHz (o U-Boot
    // deixa o GPLL em 600 MHz, o Android em 1200)
    let div = crate::audio::hdmi::gpll_khz().div_ceil(300_000).clamp(1, 32) - 1;
    cru(0x0B4, 0xDFDF, 1 << 14 | div << 8 | 1 << 6 | div);
    cru(0x0CC, 0x7F00, 1 << 13 | div << 8);
    // portoes (1 = desligado): pre-relogios (GATE3 2..4, GATE4 5) e
    // aclk/hclk do bloco e dos NOCs (GATE15 2, 3, 6, 7)
    cru(0x0DC, 0x1C, 0);
    cru(0x0E0, 1 << 5, 0);
    cru(0x10C, 1 << 2 | 1 << 3 | 1 << 6 | 1 << 7, 0);
    // resets do RKVDEC (SOFTRST_CON7 bits 6..11) soltos
    cru(0x12C, 0x0FC0, 0);
    crate::cpu::barreira();
    ler(0) >> 16 == 0x6876
}

/// O ID do bloco (0 = desligado), para o "info".
pub fn id() -> u32 {
    ler(0)
}

/// Memoria para o hardware: alinhada, fora do cache da CPU na entrega.
struct Dma {
    ptr: *mut u8,
    tamanho: usize,
}

impl Dma {
    fn novo(tamanho: usize) -> Option<Dma> {
        let l = Layout::from_size_align(tamanho.max(64), 4096).ok()?;
        let ptr = unsafe { alloc::alloc::alloc_zeroed(l) };
        if ptr.is_null() {
            return None;
        }
        // nada do cache sujo pode cair por cima do que o hardware escrever
        crate::mmu::limpar_cache(ptr as usize, tamanho);
        Some(Dma { ptr, tamanho })
    }

    fn endereco(&self) -> u32 {
        self.ptr as u32
    }
}

impl Drop for Dma {
    fn drop(&mut self) {
        let l = Layout::from_size_align(self.tamanho.max(64), 4096).unwrap();
        unsafe { alloc::alloc::dealloc(self.ptr, l) };
    }
}

/// As tabelas que o hardware le (mesmo layout do Linux).
#[repr(C)]
struct Tabelas {
    cabac: [[[i8; 2]; 464]; 4],
    escala4: [[u8; 16]; 6],
    escala8: [[u8; 64]; 6],
    escala_folga: [u8; 128],
    rps: [u32; 32],
    parametros: [[u32; 8]; 256],
    erros: [u8; 256 * 144 * 4],
}

/// Escreve `valor` em `largura` bits a partir do bit `bit` (campos do
/// pacote de SPS/PPS e do RPS).
fn campo(buf: &mut [u32], bit: usize, largura: u32, valor: u32) {
    let (palavra, b) = (bit / 32, (bit % 32) as u32);
    let mascara = if largura == 32 { u64::MAX >> 32 } else { (1u64 << largura) - 1 } << b;
    let v = ((valor as u64) << b) & mascara;
    buf[palavra] = (buf[palavra] & !(mascara as u32)) | v as u32;
    if b + largura > 32 {
        buf[palavra + 1] = (buf[palavra + 1] & !((mascara >> 32) as u32)) | (v >> 32) as u32;
    }
}

/// Um quadro pronto para mostrar (NV12).
#[derive(Clone, Copy)]
pub struct QuadroPronto {
    /// Qual buffer (para `soltar`).
    pub buffer: usize,
    pub y: usize,
    pub uv: usize,
    /// Bytes por linha (Y e UV).
    pub passo: usize,
    /// Parte visivel: canto e tamanho.
    pub x: usize,
    pub y0: usize,
    pub largura: usize,
    pub altura: usize,
    pub bt709: bool,
}

pub struct Decodificador {
    spss: Vec<Option<Sps>>,
    ppss: Vec<Option<Pps>>,
    sps: Sps,
    dpb: Dpb,
    tabelas: Dma,
    quadros: Vec<Dma>,
    fluxo: Dma,
    passo: usize,
    altura16: usize,
    /// Bytes do tamanho de cada NAL nas amostras (avcC).
    bytes_tamanho: usize,
    pub erros: u32,
    /// Ultimo estado do registrador de interrupcao (diagnostico).
    pub ultimo_int: u32,
}

type R<T> = Result<T, &'static str>;

impl Decodificador {
    /// Com o avcC do MP4 (SPS e PPS dentro).
    pub fn novo(avcc: &[u8]) -> R<Decodificador> {
        let (bytes_tamanho, spss_b, ppss_b) = crate::midia::mp4::avcc(avcc).ok_or("avcC invalido")?;
        let mut spss: Vec<Option<Sps>> = alloc::vec![None; 32];
        let mut ppss: Vec<Option<Pps>> = alloc::vec![None; 256];
        for s in spss_b {
            let sps = h264::ler_sps(s)?;
            let id = sps.id as usize;
            spss[id] = Some(sps);
        }
        for p in ppss_b {
            let pps = h264::ler_pps(p, &spss)?;
            let id = pps.id as usize;
            ppss[id] = Some(pps);
        }
        let sps = spss.iter().flatten().next().ok_or("avcC sem SPS")?.clone();
        if sps.croma != 1 || sps.prof_luma_menos8 != 0 {
            return Err("so H.264 4:2:0 de 8 bits");
        }
        if !ligar() {
            return Err("RKVDEC nao respondeu");
        }
        let passo = (sps.largura() as usize).next_multiple_of(64);
        let altura16 = (sps.altura() as usize).next_multiple_of(16);
        let mbs = sps.largura_mbs as usize * (altura16 / 16);
        let tamanho = passo * altura16 * 3 / 2 + 128 * mbs;
        let n = Dpb::buffers_para(&sps);
        let mut quadros = Vec::with_capacity(n);
        for _ in 0..n {
            quadros.push(Dma::novo(tamanho).ok_or("sem memoria para os quadros")?);
        }
        let tabelas = Dma::novo(core::mem::size_of::<Tabelas>()).ok_or("sem memoria")?;
        unsafe { (*(tabelas.ptr as *mut Tabelas)).cabac = crate::midia::h264_cabac::TABELA };
        let fluxo = Dma::novo(1 << 20).ok_or("sem memoria")?;
        let dpb = Dpb::novo(&sps, n);
        Ok(Decodificador { spss, ppss, sps, dpb, tabelas, quadros, fluxo, passo, altura16, bytes_tamanho, erros: 0, ultimo_int: 0 })
    }

    pub fn tamanho(&self) -> (u32, u32) {
        self.sps.visivel()
    }

    /// Decodifica uma amostra do MP4 (um quadro). Os quadros prontos saem
    /// por `proximo`.
    pub fn decodificar(&mut self, amostra: &[u8]) -> R<()> {
        let nals = h264::nals_com_tamanho(amostra, self.bytes_tamanho);
        let mut fatias: Vec<&[u8]> = Vec::new();
        let mut cabecalho: Option<Fatia> = None;
        for n in nals {
            match n.first().map(|b| b & 0x1F) {
                Some(7) => {
                    if let Ok(s) = h264::ler_sps(n) {
                        let id = s.id as usize;
                        self.spss[id] = Some(s);
                    }
                }
                Some(8) => {
                    if let Ok(p) = h264::ler_pps(n, &self.spss) {
                        let id = p.id as usize;
                        self.ppss[id] = Some(p);
                    }
                }
                Some(1) | Some(5) => {
                    if cabecalho.is_none() {
                        cabecalho = Some(h264::ler_fatia(n, &self.spss, &self.ppss)?);
                    }
                    fatias.push(n);
                }
                _ => {}
            }
        }
        let Some(f) = cabecalho else { return Ok(()) };
        let q = self.dpb.comecar(&self.sps, &f)?;
        let r = self.rodar(&f, &q, &fatias);
        match r {
            Ok(()) => {
                self.dpb.terminar(&self.sps, &f, &q);
                Ok(())
            }
            Err(e) => {
                // o quadro fica no DPB mesmo assim (as referencias seguintes
                // dependem dele), so com o que o hardware conseguiu
                self.erros += 1;
                self.dpb.terminar(&self.sps, &f, &q);
                Err(e)
            }
        }
    }

    /// Fim do video: o que falta mostrar vai para a saida.
    pub fn esvaziar(&mut self) {
        self.dpb.esvaziar();
    }

    /// O proximo quadro na ordem de exibicao. O buffer fica reservado ate
    /// `soltar` (o player pode segurar dois: o da tela e o seguinte).
    pub fn proximo(&mut self) -> Option<QuadroPronto> {
        let (b, _poc) = self.dpb.saida.pop_front()?;
        self.dpb.segurar(b);
        let y = self.quadros[b].ptr as usize;
        let (l, a) = self.sps.visivel();
        Some(QuadroPronto {
            buffer: b,
            y,
            uv: y + self.passo * self.altura16,
            passo: self.passo,
            x: 2 * self.sps.corte[0] as usize,
            y0: 2 * self.sps.corte[2] as usize,
            largura: l as usize,
            altura: a as usize,
            bt709: self.sps.bt709,
        })
    }

    pub fn soltar(&mut self, q: &QuadroPronto) {
        self.dpb.devolver(q.buffer);
    }

    /// Joga fora o que esta na fila (pulo no video).
    pub fn recomecar(&mut self) {
        let n = self.quadros.len();
        self.dpb = Dpb::novo(&self.sps, n);
    }

    fn rodar(&mut self, f: &Fatia, q: &Quadro, fatias: &[&[u8]]) -> R<()> {
        let pps = self.ppss[f.pps_id as usize].clone().ok_or("PPS sumiu")?;
        let sps = self.spss[pps.sps_id as usize].clone().ok_or("SPS sumiu")?;
        // fluxo: as fatias com 00 00 01 na frente, e zeros no fim
        let total: usize = fatias.iter().map(|n| n.len() + 3).sum::<usize>() + 64;
        if total > self.fluxo.tamanho {
            self.fluxo = Dma::novo(total.next_power_of_two()).ok_or("sem memoria para o fluxo")?;
        }
        let destino = unsafe { core::slice::from_raw_parts_mut(self.fluxo.ptr, self.fluxo.tamanho) };
        let mut p = 0;
        for n in fatias {
            destino[p..p + 3].copy_from_slice(&[0, 0, 1]);
            destino[p + 3..p + 3 + n.len()].copy_from_slice(n);
            p += 3 + n.len();
        }
        let comprimento = p;
        destino[p..p + 64].fill(0);
        crate::mmu::limpar_cache(self.fluxo.ptr as usize, total);

        let t = unsafe { &mut *(self.tabelas.ptr as *mut Tabelas) };
        self.montar_parametros(t, &sps, &pps, q);
        self.montar_rps(t, q);
        crate::mmu::limpar_cache(self.tabelas.ptr as usize, core::mem::size_of::<Tabelas>() - t.erros.len());

        let base_tab = self.tabelas.endereco();
        let off = |campo: *const u8| campo as usize as u32 - base_tab;
        let atual = self.quadros[q.buffer].endereco();
        let y_tam = (self.passo * self.altura16) as u32;
        let mut r = [0u32; 78];
        r[2] = 1 << 20; // dec_mode = H.264
        let p16 = (self.passo / 16) as u32;
        r[3] = p16 | 1 << 11 | p16 << 12 | 0x7FF << 21;
        r[4] = self.fluxo.endereco();
        r[5] = comprimento as u32;
        r[6] = base_tab + off(t.cabac.as_ptr() as *const u8);
        r[7] = atual;
        r[8] = y_tam / 16;
        r[9] = y_tam * 3 / 2 / 16;
        for i in 0..16 {
            let l = &q.dpb[i];
            let ativo = l.ocupado && l.referencia != Ref::Nao;
            let endereco = if ativo { self.quadros[l.buffer].endereco() } else { atual };
            // quadro (nao campo): os dois campos usados
            let flags = if ativo { 1 << 1 | 1 << 2 | 1 << 3 } else { 0 };
            let base = (endereco & !0xF) | flags;
            if i < 15 {
                r[10 + i] = base;
            } else {
                r[48] = base;
            }
            let (topo, baixo) = if ativo { (l.poc.0 as u32, l.poc.1 as u32) } else { (0, 0) };
            // POCs: 0..14 alternam topo/baixo, 15..29 idem com outro inicio,
            // 30 e 31 separados (como no Linux)
            match i {
                0..=7 => r[25 + i * 2] = topo,
                8..=14 => r[49 + (i - 8) * 2 + 1] = topo,
                _ => r[72] = topo,
            }
            match i {
                0..=6 => r[25 + i * 2 + 1] = baixo,
                7..=14 => r[49 + (i - 7) * 2] = baixo,
                _ => r[73] = baixo,
            }
        }
        r[40] = q.poc.0 as u32;
        r[74] = q.poc.1 as u32;
        r[41] = self.fluxo.endereco();
        r[42] = base_tab + off(t.parametros.as_ptr() as *const u8);
        r[43] = base_tab + off(t.rps.as_ptr() as *const u8);
        r[75] = base_tab + off(t.erros.as_ptr() as *const u8);

        for (i, &v) in r.iter().enumerate().skip(2) {
            escrever(i, v);
        }
        // swreg_block_gating_e (o Linux desliga no rk3328, irmao do rk3228)
        let qos = ler(0x18C / 4);
        escrever(0x18C / 4, (qos & 0xFFFF_0000) | 0xEFFF);
        escrever(0x410 / 4, 1);
        escrever(0x450 / 4, 1);
        crate::cpu::barreira();
        escrever(REG_INT, INT_DEC_E | INT_CLKGATE_E | INT_TIMEOUT_E | INT_BUF_EMPTY_E);

        let freq = crate::cpu::frequencia_timer().max(1);
        let fim = crate::cpu::contador() + freq / 2;
        loop {
            let s = ler(REG_INT);
            if s & (INT_RDY | INT_BUS | INT_ERRO | INT_TEMPO | INT_VAZIO | INT_COLMV) != 0 {
                self.ultimo_int = s;
                escrever(REG_INT, 0);
                return if s & INT_RDY != 0 && s & (INT_ERRO | INT_TEMPO | INT_BUS) == 0 { Ok(()) } else { Err("o RKVDEC acusou erro no quadro") };
            }
            if crate::cpu::contador() > fim {
                self.ultimo_int = s;
                // reset do bloco pela CRU e segue
                cru(0x12C, 0x0FC0, 0x0FC0);
                crate::cpu::barreira();
                cru(0x12C, 0x0FC0, 0);
                return Err("o RKVDEC nao terminou o quadro");
            }
            core::hint::spin_loop();
        }
    }

    /// O pacote de SPS/PPS (256 bits) no lugar do PPS id, e as listas de
    /// escala.
    fn montar_parametros(&self, t: &mut Tabelas, sps: &Sps, pps: &Pps, q: &Quadro) {
        let escala = pps.escala.as_ref().or(sps.escala.as_ref());
        if let Some(e) = escala {
            t.escala4 = e.l4;
            t.escala8 = e.l8;
        }
        let endereco_escala = self.tabelas.endereco() + (t.escala4.as_ptr() as usize - self.tabelas.ptr as usize) as u32;
        let hw = &mut t.parametros[pps.id as usize];
        *hw = [0; 8];
        let mut w = |bit: usize, largura: u32, v: u32| campo(hw, bit, largura, v);
        w(0, 4, sps.id);
        w(4, 8, sps.perfil);
        w(12, 1, sps.restricoes >> 4 & 1);
        w(13, 2, sps.croma);
        w(15, 3, sps.prof_luma_menos8);
        w(18, 3, sps.prof_croma_menos8);
        w(21, 1, sps.bypass_qp0 as u32);
        w(22, 4, sps.log2_max_frame_num_menos4);
        w(26, 5, sps.max_refs);
        w(31, 2, sps.tipo_poc);
        w(33, 4, sps.log2_max_poc_lsb_menos4);
        w(37, 1, sps.delta_poc_sempre_zero as u32);
        w(38, 9, sps.largura_mbs);
        w(47, 9, sps.altura_mbs);
        w(56, 1, sps.so_quadros as u32);
        w(57, 1, sps.mbaff as u32);
        w(58, 1, sps.direto_8x8 as u32);
        w(128, 8, pps.id);
        w(136, 5, pps.sps_id);
        w(141, 1, pps.cabac as u32);
        w(142, 1, pps.poc_campo_baixo as u32);
        w(143, 5, pps.refs_l0_menos1);
        w(148, 5, pps.refs_l1_menos1);
        w(153, 1, pps.predicao_pesos as u32);
        w(154, 2, pps.bipred_pesos);
        w(156, 7, pps.qp_inicial_menos26 as u32);
        w(163, 6, pps.qs_inicial_menos26 as u32);
        w(169, 5, pps.desvio_qp_croma as u32);
        w(174, 1, pps.controle_desbloqueio as u32);
        w(175, 1, pps.intra_restrito as u32);
        w(176, 1, pps.redundante as u32);
        w(177, 1, pps.transformada_8x8 as u32);
        w(178, 5, pps.desvio_qp_croma2 as u32);
        w(183, 1, escala.is_some() as u32);
        w(184, 32, endereco_escala);
        for i in 0..16 {
            w(216 + i, 1, (q.dpb[i].ocupado && q.dpb[i].referencia == Ref::Longa) as u32);
        }
    }

    /// RPS: o frame_num de cada referencia e as tres listas iniciais.
    fn montar_rps(&self, t: &mut Tabelas, q: &Quadro) {
        let rps = &mut t.rps;
        *rps = [0; 32];
        for i in 0..16 {
            let l = &q.dpb[i];
            if !l.ocupado || l.referencia == Ref::Nao {
                continue;
            }
            let n = if l.referencia == Ref::Longa { l.indice_longa } else { l.frame_num_wrap as u32 };
            campo(rps, i * 16, 16, n);
        }
        for (j, lista) in [&q.lista_p, &q.lista_b0, &q.lista_b1].into_iter().enumerate() {
            for (e, &idx) in lista.iter().enumerate().take(32) {
                let bit = 288 + j * 7 * 32 + e * 7;
                campo(rps, bit, 5, idx as u32 | 1 << 4);
            }
        }
    }
}

