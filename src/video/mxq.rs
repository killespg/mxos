//! Video na TV box MXQ PRO 4K (Amlogic S905W, familia GXL).
//!
//! AINDA NAO TESTADO NO HARDWARE. Compila no QEMU, mas so as funcoes puras
//! (decodificar/codificar registradores, autoteste_decodificacao) rodam
//! la; o resto acessa enderecos que so existem na caixa.
//!
//! Ideia: nao escrever um driver HDMI. O U-Boot da Amlogic deixa o video
//! inteiro ligado (VPU, VENC, PLL, PHY e controlador HDMI) quando chama o
//! kernel com "booti". O logo dele fica num framebuffer na RAM que o OSD
//! (camada grafica do VPU) le sozinho a cada quadro. Entao:
//!   1. descobrir(): le os registradores e acha qual OSD esta ligado, o
//!      formato, a janela e (experimental) o endereco do canvas
//!   2. usar_como_esta(): desenha direto no framebuffer do U-Boot
//!      (RGB565 no env padrao: feio, mas nao muda nada no hardware)
//!   3. tomar_posse(): aponta o MESMO canvas para um buffer nosso de
//!      32 bpp e troca so o formato no OSD (como o meson_genfb do NetBSD)
//!
//! Fontes: Linux drivers/gpu/drm/meson (meson_registers.h, meson_viu.h,
//! meson_dw_hdmi.c), drivers/soc/amlogic/meson-canvas.c, U-Boot da Amlogic
//! 2015.01 (fork da Khadas, board gxl_p281: osd_hw.c, osd_fb.c, canvas.h)
//! e NetBSD meson_genfb.c.
//!
//! Memoria: o VPU le a DRAM sem passar pelo cache da CPU. O framebuffer
//! precisa ser mapeado como Normal Non-cacheable (MAIR 0x44) ou cada
//! desenho precisa de "dc cvac"; as Telas daqui ligam a limpeza de cache.
//! Com a MMU desligada a RAM vira Device: so acessos alinhados (a Tela ja
//! faz assim).

#![allow(dead_code)]

use core::fmt;
use core::ptr::{read_volatile, write_volatile};

use super::tela::{self, Formato, Tela};

// ---------------------------------------------------------------------
// Enderecos fisicos (todos dentro de 0xC000_0000..0x1_0000_0000, que o
// plataforma.rs da mxq ja mapeia como dispositivo).
// ---------------------------------------------------------------------

/// VPU (vcbus): registrador n fica em BASE_VPU + n * 4.
const BASE_VPU: usize = 0xD010_0000;
/// Canvas (dentro do DMC).
const CANVAS_DATAL: usize = 0xC883_8048;
const CANVAS_DATAH: usize = 0xC883_804C;
const CANVAS_ADDR: usize = 0xC883_8050;
/// So aparecem no regs.h do vendor; nenhum driver conhecido usa.
const CANVAS_RDATAL: usize = 0xC883_8054;
const CANVAS_RDATAH: usize = 0xC883_8058;
const CANVAS_ESCREVER: u32 = 1 << 9;
const CANVAS_LER: u32 = 1 << 8;

/// HDMI TX: acesso indireto (endereco duas vezes, dado).
const HDMI_TOP_ENDERECO: usize = 0xC883_A000;
const HDMI_TOP_DADO: usize = 0xC883_A004;
const HDMI_DWC_ENDERECO: usize = 0xC883_A010;
const HDMI_DWC_DADO: usize = 0xC883_A014;

/// Onde o U-Boot de referencia (p281, 1 GB) poe o logo: env fb_addr.
pub const FB_UBOOT: usize = 0x3D80_0000;
/// Regiao sugerida para o framebuffer proprio (mesma do U-Boot, 16 MB,
/// alinhada em 2 MB, fora da RAM do heap do MXOS 0x0100_0000..0x0500_0000).
pub const FB_REGIAO_INICIO: usize = 0x3D80_0000;
pub const FB_REGIAO_FIM: usize = 0x3E80_0000;
/// Um endereco de canvas so e aceito se cair na RAM (caixa de 1 GB).
const RAM_FIM: usize = 0x4000_0000;

const fn vcbus(indice: usize) -> usize {
    BASE_VPU + (indice << 2)
}

// Registradores do VIU/VPP/VENC (indices do vcbus).
const VPP_MISC: usize = vcbus(0x1D26);
const VPP_POSTBLEND_CURRENT_XY: usize = vcbus(0x1D25);
const VPP_OSD_SC_CTRL0: usize = vcbus(0x1DC8);
const ENCI_VIDEO_EN: usize = vcbus(0x1B57);
const ENCP_VIDEO_EN: usize = vcbus(0x1B80);
const ENCP_VIDEO_HAVON_END: usize = vcbus(0x1BA3);
const ENCP_VIDEO_HAVON_BEGIN: usize = vcbus(0x1BA4);
const ENCP_VIDEO_VAVON_BLINE: usize = vcbus(0x1BA6);
const ENCP_VIDEO_VAVON_ELINE: usize = vcbus(0x1BAF);

// Registradores do HDMI (controlador DesignWare e "TOP" da Amlogic).
const DWC_DESIGN_ID: u32 = 0x0000;
const DWC_REVISION_ID: u32 = 0x0001;
const DWC_FC_INVIDCONF: u32 = 0x1000;
const DWC_FC_INHACTV0: u32 = 0x1001;
const DWC_FC_INHACTV1: u32 = 0x1002;
const DWC_FC_INVACTV0: u32 = 0x1005;
const DWC_FC_INVACTV1: u32 = 0x1006;
const DWC_FC_AVIVID: u32 = 0x101C;
const TOP_STAT0: u32 = 0x00E;

// Bits.
const MISC_OSD1_POSTBLEND: u32 = 1 << 12;
const MISC_OSD2_POSTBLEND: u32 = 1 << 13;
const SC_CAMINHO_LIGADO: u32 = 1 << 3;
const W0_LITTLE_ENDIAN: u32 = 1 << 15;
const STAT2_TROCAR_ALFA: u32 = 1 << 14;
const STAT2_ALFA_OPACO: u32 = 0xFF << 6;
const BLK_MODO_32BPP: u32 = 5;
const MATRIZ_ARGB: u32 = 1;

/// Qual camada OSD de hardware. Atencao: com o env padrao
/// display_layer=osd1 o logo vai para o OSD2 (o U-Boot conta de 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Osd {
    Osd1,
    Osd2,
}

/// Registradores de uma camada OSD.
struct RegistrosOsd {
    ctrl_stat2: usize,
    w0: usize,
    w1: usize,
    w2: usize,
}

const REGS_OSD1: RegistrosOsd = RegistrosOsd {
    ctrl_stat2: vcbus(0x1A2D),
    w0: vcbus(0x1A1B),
    w1: vcbus(0x1A1C),
    w2: vcbus(0x1A1D),
};

const REGS_OSD2: RegistrosOsd = RegistrosOsd {
    ctrl_stat2: vcbus(0x1A4D),
    w0: vcbus(0x1A3B),
    w1: vcbus(0x1A3C),
    w2: vcbus(0x1A3D),
};

impl Osd {
    fn registros(self) -> &'static RegistrosOsd {
        match self {
            Osd::Osd1 => &REGS_OSD1,
            Osd::Osd2 => &REGS_OSD2,
        }
    }

    pub fn nome(self) -> &'static str {
        match self {
            Osd::Osd1 => "osd1",
            Osd::Osd2 => "osd2",
        }
    }
}

/// Formato do pixel no OSD (blk_mode e matriz de cor do BLK0_CFG_W0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatoOsd {
    Rgb565,
    /// 3 bytes B, G, R.
    Rgb888,
    /// u32 0xAARRGGBB.
    Argb8888,
    Abgr8888,
    Bgra8888,
    Rgba8888,
}

impl FormatoOsd {
    pub fn nome(self) -> &'static str {
        match self {
            FormatoOsd::Rgb565 => "rgb565",
            FormatoOsd::Rgb888 => "rgb888",
            FormatoOsd::Argb8888 => "argb8888",
            FormatoOsd::Abgr8888 => "abgr8888",
            FormatoOsd::Bgra8888 => "bgra8888",
            FormatoOsd::Rgba8888 => "rgba8888",
        }
    }

    pub fn bytes_por_pixel(self) -> usize {
        match self {
            FormatoOsd::Rgb565 => 2,
            FormatoOsd::Rgb888 => 3,
            _ => 4,
        }
    }

    /// Formato equivalente da Tela, se houver. ARGB vira XRGB: o alfa
    /// tem que ser ignorado pelo OSD (ligar_alfa_fixo).
    pub fn formato_tela(self) -> Option<Formato> {
        match self {
            FormatoOsd::Rgb565 => Some(Formato::Rgb565),
            FormatoOsd::Rgb888 => Some(Formato::Rgb888),
            FormatoOsd::Argb8888 => Some(Formato::Xrgb8888),
            _ => None,
        }
    }
}

/// Uma entrada da tabela de canvas: onde o OSD le os pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Canvas {
    pub endereco: usize,
    /// Bytes por linha (multiplo de 8).
    pub passo: usize,
    pub altura: usize,
}

/// Janela de origem do OSD (dentro do canvas). Fins inclusivos no hardware.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Janela {
    pub x_inicio: usize,
    pub y_inicio: usize,
    pub largura: usize,
    pub altura: usize,
}

// ---------------------------------------------------------------------
// Funcoes puras: so contas com valores de registradores. Testaveis no QEMU.
// ---------------------------------------------------------------------

/// Arredonda para cima ao multiplo de 64 (CANVAS_ALIGNED do U-Boot).
pub const fn alinhar64(valor: usize) -> usize {
    (valor + 63) & !63
}

/// Indice do canvas usado pelo OSD: W0 bits [23:16].
pub const fn canvas_do_w0(w0: u32) -> u8 {
    (w0 >> 16 & 0xFF) as u8
}

/// Formato pelo W0: blk_mode [11:8] e matriz de cor [5:2].
pub fn decodificar_formato(w0: u32) -> Option<FormatoOsd> {
    let modo = w0 >> 8 & 0xF;
    let matriz = w0 >> 2 & 0xF;
    match (modo, matriz) {
        (4, 4) => Some(FormatoOsd::Rgb565),
        (7, 0) => Some(FormatoOsd::Rgb888),
        (5, 0) => Some(FormatoOsd::Rgba8888),
        (5, 1) => Some(FormatoOsd::Argb8888),
        (5, 2) => Some(FormatoOsd::Abgr8888),
        (5, 3) => Some(FormatoOsd::Bgra8888),
        _ => None,
    }
}

/// W0 trocado para ARGB 32 bpp little endian, preservando o resto
/// (indice do canvas, bit 7, entrelacamento).
pub const fn w0_para_argb8888(w0: u32) -> u32 {
    let limpo = w0 & !(0xF << 8) & !(0xF << 2);
    limpo | BLK_MODO_32BPP << 8 | MATRIZ_ARGB << 2 | W0_LITTLE_ENDIAN
}

/// Janela de origem pelos W1 (x) e W2 (y): inicio [12:0], fim [28:16].
pub const fn decodificar_janela(w1: u32, w2: u32) -> Janela {
    let x_inicio = (w1 & 0x1FFF) as usize;
    let x_fim = (w1 >> 16 & 0x1FFF) as usize;
    let y_inicio = (w2 & 0x1FFF) as usize;
    let y_fim = (w2 >> 16 & 0x1FFF) as usize;
    Janela {
        x_inicio,
        y_inicio,
        largura: x_fim.saturating_sub(x_inicio) + 1,
        altura: y_fim.saturating_sub(y_inicio) + 1,
    }
}

/// Qual OSD chega na tela. O free scaler manda (bit 3 ligado = fonte nos
/// bits [1:0]); senao, os bits de postblend do VPP_MISC. Com o scaler o
/// vendor liga o bit 12 mesmo vindo do OSD2, por isso ele vem primeiro.
pub const fn escolher_osd(sc_ctrl0: u32, vpp_misc: u32) -> Option<Osd> {
    if sc_ctrl0 & SC_CAMINHO_LIGADO != 0 {
        return match sc_ctrl0 & 3 {
            0 => Some(Osd::Osd1),
            1 => Some(Osd::Osd2),
            _ => None,
        };
    }
    if vpp_misc & MISC_OSD2_POSTBLEND != 0 {
        Some(Osd::Osd2)
    } else if vpp_misc & MISC_OSD1_POSTBLEND != 0 {
        Some(Osd::Osd1)
    } else {
        None
    }
}

/// Canvas -> (DATAL, DATAH), linear, sem wrap, endian 0 (meson-canvas.c).
pub const fn codificar_canvas(canvas: &Canvas) -> (u32, u32) {
    let endereco = ((canvas.endereco + 7) >> 3) as u32;
    let passo = ((canvas.passo + 7) >> 3) as u32;
    let datal = endereco & 0x1FFF_FFFF | (passo & 7) << 29;
    let datah = (passo >> 3) & 0x1FF | (canvas.altura as u32 & 0x1FFF) << 9;
    (datal, datah)
}

/// (DATAL, DATAH) -> Canvas. Inverso de codificar_canvas.
pub const fn decodificar_canvas(datal: u32, datah: u32) -> Canvas {
    let endereco = ((datal & 0x1FFF_FFFF) as usize) << 3;
    let passo8 = ((datah & 0x1FF) << 3 | datal >> 29) as usize;
    Canvas { endereco, passo: passo8 << 3, altura: (datah >> 9 & 0x1FFF) as usize }
}

/// Confere as funcoes puras com os valores do U-Boot p281 (fb_addr
/// 0x3d800000, 1920 RGB565 = passo 3840, canvas de 2160 linhas).
pub fn autoteste_decodificacao() -> Result<(), &'static str> {
    let canvas = Canvas { endereco: 0x3D80_0000, passo: 3840, altura: 2160 };
    if codificar_canvas(&canvas) != (0x07B0_0000, 0x0010_E03C) {
        return Err("codificar_canvas");
    }
    if decodificar_canvas(0x07B0_0000, 0x0010_E03C) != canvas {
        return Err("decodificar_canvas");
    }
    // passo com bits baixos em DATAL (7680 = 960 * 8: 960 & 7 = 0; 7688 nao)
    let torto = Canvas { endereco: 0x0100_0000, passo: 7688, altura: 1080 };
    let (l, h) = codificar_canvas(&torto);
    if decodificar_canvas(l, h) != torto {
        return Err("canvas passo impar");
    }
    // W0 do vendor para o OSD2: canvas 0x43, RGB565, little endian, bit 7
    let w0 = 0x43 << 16 | W0_LITTLE_ENDIAN | 4 << 8 | 1 << 7 | 4 << 2;
    if decodificar_formato(w0) != Some(FormatoOsd::Rgb565) || canvas_do_w0(w0) != 0x43 {
        return Err("decodificar_formato 565");
    }
    let novo = w0_para_argb8888(w0);
    let preservado = !(0xF << 8 | 0xF << 2 | W0_LITTLE_ENDIAN);
    if decodificar_formato(novo) != Some(FormatoOsd::Argb8888) || novo & preservado != w0 & preservado {
        return Err("w0_para_argb8888");
    }
    if decodificar_formato(5 << 8 | 7 << 2).is_some() {
        return Err("formato invalido aceito");
    }
    let janela = decodificar_janela(1919 << 16, 1079 << 16);
    if janela != (Janela { x_inicio: 0, y_inicio: 0, largura: 1920, altura: 1080 }) {
        return Err("decodificar_janela");
    }
    if escolher_osd(1 | SC_CAMINHO_LIGADO | 1 << 2, MISC_OSD1_POSTBLEND) != Some(Osd::Osd2) {
        return Err("escolher_osd scaler");
    }
    if escolher_osd(0, MISC_OSD2_POSTBLEND) != Some(Osd::Osd2) || escolher_osd(0, MISC_OSD1_POSTBLEND) != Some(Osd::Osd1) {
        return Err("escolher_osd misc");
    }
    if escolher_osd(0, 0).is_some() || alinhar64(1920 * 3) != 5760 || alinhar64(1366 * 2) != 2752 {
        return Err("escolher_osd/alinhar64");
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Acesso ao hardware (so na caixa).
// ---------------------------------------------------------------------

fn ler(endereco: usize) -> u32 {
    unsafe { read_volatile(endereco as *const u32) }
}

fn escrever(endereco: usize, valor: u32) {
    unsafe { write_volatile(endereco as *mut u32, valor) }
}

/// Le um registrador do controlador HDMI DesignWare (endereco escrito
/// duas vezes, dado lido duas vezes, como o meson_dw_hdmi.c do Linux).
fn ler_dwc(registrador: u32) -> u32 {
    escrever(HDMI_DWC_ENDERECO, registrador);
    escrever(HDMI_DWC_ENDERECO, registrador);
    let _ = ler(HDMI_DWC_DADO);
    ler(HDMI_DWC_DADO)
}

/// Mesmo protocolo para os registradores "TOP" da Amlogic.
fn ler_top(registrador: u32) -> u32 {
    escrever(HDMI_TOP_ENDERECO, registrador);
    escrever(HDMI_TOP_ENDERECO, registrador);
    let _ = ler(HDMI_TOP_DADO);
    ler(HDMI_TOP_DADO)
}

/// Le uma entrada da tabela de canvas. EXPERIMENTAL: RD_EN + RDATAL/H
/// so existem no regs.h do vendor. Descarta resultado fora da RAM.
fn ler_canvas(indice: u8) -> Option<Canvas> {
    escrever(CANVAS_ADDR, CANVAS_LER | indice as u32);
    let canvas = decodificar_canvas(ler(CANVAS_RDATAL), ler(CANVAS_RDATAH));
    let valido = canvas.endereco != 0 && canvas.endereco < RAM_FIM && canvas.passo != 0 && canvas.altura != 0;
    valido.then_some(canvas)
}

/// Grava uma entrada da tabela de canvas (meson_canvas_config do Linux).
fn escrever_canvas(indice: u8, canvas: &Canvas) {
    let (datal, datah) = codificar_canvas(canvas);
    escrever(CANVAS_DATAL, datal);
    escrever(CANVAS_DATAH, datah);
    escrever(CANVAS_ADDR, CANVAS_ESCREVER | indice as u32);
    // uma leitura garante que a escrita chegou antes de seguir
    let _ = ler(CANVAS_DATAH);
}

/// O VPP esta varrendo a tela? A posicao atual muda entre leituras.
fn vpp_varrendo() -> bool {
    let primeira = ler(VPP_POSTBLEND_CURRENT_XY);
    (0..100_000).any(|_| ler(VPP_POSTBLEND_CURRENT_XY) != primeira)
}

/// Resolucao que o encoder progressivo (HDMI) esta mandando.
fn resolucao_encp() -> (usize, usize) {
    let largura = ler(ENCP_VIDEO_HAVON_END).wrapping_sub(ler(ENCP_VIDEO_HAVON_BEGIN)).wrapping_add(1);
    let altura = ler(ENCP_VIDEO_VAVON_ELINE).wrapping_sub(ler(ENCP_VIDEO_VAVON_BLINE)).wrapping_add(1);
    (largura as usize & 0x1FFF, altura as usize & 0x1FFF)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Erro {
    /// Nenhum encoder ligado e o VPP parado: o U-Boot nao ligou o video.
    VpuDesligado,
    /// Nenhum OSD chega na saida (o U-Boot nao abriu o logo).
    SemOsd,
    /// Formato do OSD que a Tela nao sabe desenhar.
    FormatoNaoSuportado,
}

impl Erro {
    pub fn descricao(self) -> &'static str {
        match self {
            Erro::VpuDesligado => "vídeo: VPU desligado",
            Erro::SemOsd => "vídeo: nenhum OSD ligado",
            Erro::FormatoNaoSuportado => "vídeo: formato do OSD não suportado",
        }
    }
}

/// Tudo que descobrir() leu dos registradores.
#[derive(Clone, Copy, Debug)]
pub struct InfoVideo {
    pub encp: bool,
    pub enci: bool,
    pub varrendo: bool,
    /// Resolucao de saida do ENCP (0 se desligado).
    pub saida: (usize, usize),
    pub osd: Osd,
    pub w0: u32,
    pub canvas_indice: u8,
    pub formato: Option<FormatoOsd>,
    pub janela: Janela,
    /// Canvas lido do hardware (experimental; None se a leitura falhou).
    pub canvas_lido: Option<Canvas>,
}

impl InfoVideo {
    /// Endereco e passo do framebuffer do U-Boot: o canvas lido, ou o
    /// fb_addr padrao com passo alinhar64(largura * bytes).
    pub fn framebuffer_uboot(&self) -> (usize, usize) {
        if let Some(canvas) = self.canvas_lido {
            return (canvas.endereco, canvas.passo);
        }
        let bytes = self.formato.map_or(2, FormatoOsd::bytes_por_pixel);
        (FB_UBOOT, alinhar64(self.janela.largura * bytes))
    }
}

/// Le o estado que o U-Boot deixou (passo 1 a 6 da pesquisa). Nao escreve
/// nada, exceto o pedido de leitura do canvas.
pub fn descobrir() -> Result<InfoVideo, Erro> {
    let encp = ler(ENCP_VIDEO_EN) & 1 != 0;
    let enci = ler(ENCI_VIDEO_EN) & 1 != 0;
    let varrendo = vpp_varrendo();
    if !encp && !enci && !varrendo {
        return Err(Erro::VpuDesligado);
    }
    let osd = escolher_osd(ler(VPP_OSD_SC_CTRL0), ler(VPP_MISC)).ok_or(Erro::SemOsd)?;
    let regs = osd.registros();
    let w0 = ler(regs.w0);
    let canvas_indice = canvas_do_w0(w0);
    Ok(InfoVideo {
        encp,
        enci,
        varrendo,
        saida: if encp { resolucao_encp() } else { (0, 0) },
        osd,
        w0,
        canvas_indice,
        formato: decodificar_formato(w0),
        janela: decodificar_janela(ler(regs.w1), ler(regs.w2)),
        canvas_lido: ler_canvas(canvas_indice),
    })
}

/// Faz o OSD ignorar o byte de alfa (todo pixel opaco).
fn ligar_alfa_fixo(osd: Osd) {
    let registrador = osd.registros().ctrl_stat2;
    let valor = ler(registrador) & !(0xFF << 6);
    escrever(registrador, valor | STAT2_TROCAR_ALFA | STAT2_ALFA_OPACO);
}

/// Desenha direto no framebuffer do U-Boot, no formato que ele usa.
///
/// # Seguranca
/// O endereco vem do hardware (ou do padrao 0x3d800000) e precisa estar
/// mapeado, de preferencia Non-cacheable, e reservado so para isso.
pub unsafe fn usar_como_esta(info: &InfoVideo) -> Result<Tela, Erro> {
    let formato = info.formato.and_then(FormatoOsd::formato_tela).ok_or(Erro::FormatoNaoSuportado)?;
    if formato == Formato::Xrgb8888 {
        ligar_alfa_fixo(info.osd);
    }
    let (base, passo) = info.framebuffer_uboot();
    let base = base + info.janela.y_inicio * passo + info.janela.x_inicio * formato.bytes_por_pixel();
    let tela = unsafe { Tela::new(base, info.janela.largura, info.janela.altura, passo, formato) };
    Ok(tela.com_limpeza_de_cache(true))
}

/// Bytes que tomar_posse() usa a partir de `base`.
pub fn bytes_para_posse(info: &InfoVideo) -> usize {
    alinhar64(info.janela.largura * 4) * (info.janela.y_inicio + info.janela.altura)
}

/// Aponta o canvas do OSD para um buffer nosso em XRGB 32 bpp. Mesma
/// janela, mesmo scaler: so muda endereco, passo e formato.
/// Sem esperar o vsync pode aparecer um quadro rasgado.
///
/// # Seguranca
/// [base, base + bytes_para_posse(info)) precisa ser RAM mapeada (de
/// preferencia Non-cacheable), alinhada em 64 bytes e so para o video.
pub unsafe fn tomar_posse(info: &InfoVideo, base: usize) -> Tela {
    let passo = alinhar64(info.janela.largura * 4);
    let inicio_janela = base + info.janela.y_inicio * passo + info.janela.x_inicio * 4;
    let tela = unsafe { Tela::new(inicio_janela, info.janela.largura, info.janela.altura, passo, Formato::Xrgb8888) };
    let tela = tela.com_limpeza_de_cache(true);
    // primeiro um buffer limpo, depois o OSD passa a ler dele
    tela.limpar(tela::PRETO);
    let canvas = Canvas { endereco: base, passo, altura: info.janela.y_inicio + info.janela.altura };
    escrever_canvas(info.canvas_indice, &canvas);
    let regs = info.osd.registros();
    escrever(regs.w0, w0_para_argb8888(ler(regs.w0)));
    ligar_alfa_fixo(info.osd);
    tela
}

/// Estado do HDMI, so para diagnostico.
#[derive(Clone, Copy, Debug)]
pub struct InfoHdmi {
    pub design_id: u32,
    pub revisao: u32,
    pub vic: u32,
    /// false = DVI (sem audio).
    pub modo_hdmi: bool,
    pub hpd: bool,
    pub ativo: (u32, u32),
}

pub fn ler_hdmi() -> InfoHdmi {
    let largura = ler_dwc(DWC_FC_INHACTV0) & 0xFF | (ler_dwc(DWC_FC_INHACTV1) & 0x1F) << 8;
    let altura = ler_dwc(DWC_FC_INVACTV0) & 0xFF | (ler_dwc(DWC_FC_INVACTV1) & 0x1F) << 8;
    InfoHdmi {
        design_id: ler_dwc(DWC_DESIGN_ID) & 0xFF,
        revisao: ler_dwc(DWC_REVISION_ID) & 0xFF,
        vic: ler_dwc(DWC_FC_AVIVID) & 0xFF,
        modo_hdmi: ler_dwc(DWC_FC_INVIDCONF) & (1 << 3) != 0,
        hpd: ler_top(TOP_STAT0) != 0,
        ativo: (largura, altura),
    }
}

/// Linhas ASCII para a serial (o rodar.py pode procurar por elas).
pub fn relatorio(info: &InfoVideo, hdmi: &InfoHdmi, saida: &mut impl fmt::Write) -> fmt::Result {
    let (base, passo) = info.framebuffer_uboot();
    let formato = info.formato.map_or("desconhecido", FormatoOsd::nome);
    writeln!(
        saida,
        "video: encp={} enci={} xy_mudou={} saida={}x{} osd={} canvas={:#x} fmt={} {}x{} y0={} base={:#x} stride={} canvas_lido={}",
        info.encp as u8,
        info.enci as u8,
        info.varrendo as u8,
        info.saida.0,
        info.saida.1,
        info.osd.nome(),
        info.canvas_indice,
        formato,
        info.janela.largura,
        info.janela.altura,
        info.janela.y_inicio,
        base,
        passo,
        info.canvas_lido.is_some() as u8,
    )?;
    writeln!(
        saida,
        "hdmi: id={:#x} rev={:#x} vic={} modo={} hpd={} ativo={}x{}",
        hdmi.design_id,
        hdmi.revisao,
        hdmi.vic,
        if hdmi.modo_hdmi { "hdmi" } else { "dvi" },
        hdmi.hpd as u8,
        hdmi.ativo.0,
        hdmi.ativo.1,
    )
}
