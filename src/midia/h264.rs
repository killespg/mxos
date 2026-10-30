//! H.264: a parte do decodificador que fica na CPU quando quem decodifica
//! os macroblocos e o hardware (o RKVDEC da TV box le os cabecalhos das
//! fatias e o CABAC sozinho). Aqui:
//!
//!   - leitura do fluxo (NALs, emulacao 0x000003, Exp-Golomb)
//!   - SPS e PPS (com as listas de escala e o VUI)
//!   - o comeco do cabecalho da fatia, ate a marcacao de referencias
//!   - a ordem de exibicao (POC, os tres tipos da norma, 8.2.1)
//!   - o DPB: quadros de referencia (janela deslizante e MMCO, 8.2.5) e a
//!     saida na ordem de exibicao
//!   - as listas iniciais de referencia P, B0 e B1 (8.2.4.2)
//!
//! So quadros progressivos (frame_mbs_only): e o que YouTube, celulares e
//! quase todo MP4 usam. So core + alloc: os testes do PC rodam o mesmo.

use alloc::vec;
use alloc::vec::Vec;

type R<T> = Result<T, &'static str>;

// ------------------------------------------------------------- bits

/// Tira os bytes de emulacao (00 00 03 -> 00 00) de uma NAL.
pub fn rbsp(nal: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(nal.len());
    let mut zeros = 0;
    for &b in nal {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        v.push(b);
    }
    v
}

pub struct Bits<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    pub fn novo(b: &'a [u8]) -> Bits<'a> {
        Bits { b, pos: 0 }
    }

    pub fn bit(&mut self) -> R<u32> {
        let byte = *self.b.get(self.pos / 8).ok_or("fluxo H.264 acabou")?;
        let v = (byte >> (7 - self.pos % 8)) & 1;
        self.pos += 1;
        Ok(v as u32)
    }

    pub fn flag(&mut self) -> R<bool> {
        Ok(self.bit()? == 1)
    }

    pub fn u(&mut self, n: u32) -> R<u32> {
        let mut v = 0;
        for _ in 0..n {
            v = v << 1 | self.bit()?;
        }
        Ok(v)
    }

    pub fn ue(&mut self) -> R<u32> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return Err("Exp-Golomb invalido");
            }
        }
        Ok(((1u64 << zeros) - 1 + self.u(zeros)? as u64) as u32)
    }

    pub fn se(&mut self) -> R<i32> {
        let k = self.ue()? as i64;
        Ok(if k % 2 == 1 { ((k + 1) / 2) as i32 } else { (-(k / 2)) as i32 })
    }

    /// Ainda ha dados antes do bit de parada (more_rbsp_data).
    pub fn mais_dados(&self) -> bool {
        let total = self.b.len() * 8;
        if self.pos >= total {
            return false;
        }
        // o ultimo bit 1 do fluxo e o de parada
        let Some(ultimo) = (0..self.b.len()).rev().find(|&i| self.b[i] != 0) else { return false };
        let parada = ultimo * 8 + 7 - self.b[ultimo].trailing_zeros() as usize;
        self.pos < parada
    }
}

/// As NALs de um fluxo Annex B (separadas por 00 00 01).
pub fn nals_annex_b(b: &[u8]) -> Vec<&[u8]> {
    let mut inicios = Vec::new();
    let mut i = 0;
    while i + 3 <= b.len() {
        if b[i] == 0 && b[i + 1] == 0 && b[i + 2] == 1 {
            inicios.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut v = Vec::new();
    for (k, &ini) in inicios.iter().enumerate() {
        let mut fim = inicios.get(k + 1).map(|&p| p - 3).unwrap_or(b.len());
        while fim > ini && b[fim - 1] == 0 {
            fim -= 1;
        }
        v.push(&b[ini..fim]);
    }
    v
}

/// As NALs de uma amostra MP4 (cada uma com o tamanho na frente).
pub fn nals_com_tamanho(b: &[u8], bytes_tamanho: usize) -> Vec<&[u8]> {
    let mut v = Vec::new();
    let mut i = 0;
    while i + bytes_tamanho <= b.len() {
        let mut n = 0usize;
        for k in 0..bytes_tamanho {
            n = n << 8 | b[i + k] as usize;
        }
        i += bytes_tamanho;
        if n == 0 || i + n > b.len() {
            break;
        }
        v.push(&b[i..i + n]);
        i += n;
    }
    v
}

// ---------------------------------------------------- listas de escala

/// Ordem zigue-zague -> posicao na matriz (4x4 e 8x8, quadro).
const ZIGUE4: [u8; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];
const ZIGUE8: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22,
    15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];
/// Tabelas 7-3 e 7-4 (em zigue-zague).
const PADRAO4_INTRA: [u8; 16] = [6, 13, 13, 20, 20, 20, 28, 28, 28, 28, 32, 32, 32, 37, 37, 42];
const PADRAO4_INTER: [u8; 16] = [10, 14, 14, 20, 20, 20, 24, 24, 24, 24, 27, 27, 27, 30, 30, 34];
const PADRAO8_INTRA: [u8; 64] = [
    6, 10, 10, 13, 11, 13, 16, 16, 16, 16, 18, 18, 18, 18, 18, 23, 23, 23, 23, 23, 23, 25, 25, 25, 25, 25, 25, 25, 27, 27, 27, 27, 27, 27, 27, 27, 29, 29, 29,
    29, 29, 29, 29, 31, 31, 31, 31, 31, 31, 33, 33, 33, 33, 33, 36, 36, 36, 36, 38, 38, 38, 40, 40, 42,
];
const PADRAO8_INTER: [u8; 64] = [
    9, 13, 13, 15, 13, 15, 17, 17, 17, 17, 19, 19, 19, 19, 19, 21, 21, 21, 21, 21, 21, 22, 22, 22, 22, 22, 22, 22, 24, 24, 24, 24, 24, 24, 24, 24, 25, 25, 25,
    25, 25, 25, 25, 27, 27, 27, 27, 27, 27, 28, 28, 28, 28, 28, 30, 30, 30, 30, 32, 32, 32, 33, 33, 35,
];

/// As 6 listas 4x4 e 6 listas 8x8 ja na ordem de varredura da matriz
/// (linha a linha), como o hardware quer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Escala {
    pub l4: [[u8; 16]; 6],
    pub l8: [[u8; 64]; 6],
}

impl Escala {
    pub fn plana() -> Escala {
        Escala { l4: [[16; 16]; 6], l8: [[16; 64]; 6] }
    }
}

fn desfazer_zigue<const N: usize>(zz: &[u8; N]) -> [u8; N] {
    let mut m = [0u8; N];
    for (i, &v) in zz.iter().enumerate() {
        let p = if N == 16 { ZIGUE4[i] } else { ZIGUE8[i] } as usize;
        m[p] = v;
    }
    m
}

/// scaling_list(): Some(lista em zigue-zague) ou None = usar a padrao.
fn ler_lista<const N: usize>(b: &mut Bits) -> R<Option<[u8; N]>> {
    let mut lista = [0u8; N];
    let (mut ultima, mut proxima) = (8i32, 8i32);
    for j in 0..N {
        if proxima != 0 {
            let delta = b.se()?;
            proxima = (ultima + delta + 256) % 256;
            if j == 0 && proxima == 0 {
                return Ok(None);
            }
        }
        lista[j] = if proxima == 0 { ultima } else { proxima } as u8;
        ultima = lista[j] as i32;
    }
    Ok(Some(lista))
}

/// As listas de um SPS ou PPS. `base` = as do SPS (regra B do PPS) ou
/// None (regra A do SPS). `n8` = quantas listas 8x8 vem no fluxo.
fn ler_escala(b: &mut Bits, n8: usize, base: Option<&Escala>) -> R<Escala> {
    let mut e = Escala::plana();
    for i in 0..6 {
        let presente = b.flag()?;
        let lista = if presente { ler_lista::<16>(b)? } else { None };
        e.l4[i] = match (presente, lista) {
            (true, Some(l)) => desfazer_zigue(&l),
            // usou a padrao
            (true, None) => desfazer_zigue(if i < 3 { &PADRAO4_INTRA } else { &PADRAO4_INTER }),
            // nao veio: a anterior, ou para 0 e 3 a padrao (regra A) ou a do SPS (B)
            (false, _) => match (i, base) {
                (0, None) => desfazer_zigue(&PADRAO4_INTRA),
                (3, None) => desfazer_zigue(&PADRAO4_INTER),
                (0 | 3, Some(s)) => s.l4[i],
                _ => e.l4[i - 1],
            },
        };
    }
    for i in 0..n8 {
        let presente = b.flag()?;
        let lista = if presente { ler_lista::<64>(b)? } else { None };
        // ordem do fluxo: Y intra, Y inter, Cb intra, Cb inter, Cr intra, Cr inter
        e.l8[i] = match (presente, lista) {
            (true, Some(l)) => desfazer_zigue(&l),
            (true, None) => desfazer_zigue(if i % 2 == 0 { &PADRAO8_INTRA } else { &PADRAO8_INTER }),
            (false, _) => match (i, base) {
                (0, None) => desfazer_zigue(&PADRAO8_INTRA),
                (1, None) => desfazer_zigue(&PADRAO8_INTER),
                (0 | 1, Some(s)) => s.l8[i],
                _ => e.l8[i - 2],
            },
        };
    }
    // sem as 8x8 do fluxo (4:2:0 com transform_8x8 so no PPS): as do SPS
    // ou as padrao
    for i in n8..6 {
        e.l8[i] = match base {
            Some(s) => s.l8[i],
            None => desfazer_zigue(if i % 2 == 0 { &PADRAO8_INTRA } else { &PADRAO8_INTER }),
        };
    }
    Ok(e)
}

// ------------------------------------------------------------ SPS e PPS

#[derive(Clone, Debug)]
pub struct Sps {
    pub id: u32,
    pub perfil: u32,
    pub restricoes: u32,
    pub nivel: u32,
    pub croma: u32,
    pub prof_luma_menos8: u32,
    pub prof_croma_menos8: u32,
    pub bypass_qp0: bool,
    /// Listas de escala do SPS (None = planas).
    pub escala: Option<Escala>,
    pub log2_max_frame_num_menos4: u32,
    pub tipo_poc: u32,
    pub log2_max_poc_lsb_menos4: u32,
    pub delta_poc_sempre_zero: bool,
    pub desvio_nao_ref: i32,
    pub desvio_campo_baixo: i32,
    pub desvios_ciclo: Vec<i32>,
    pub max_refs: u32,
    pub largura_mbs: u32,
    pub altura_mbs: u32,
    pub so_quadros: bool,
    pub mbaff: bool,
    pub direto_8x8: bool,
    /// Corte (em pixels): esquerda, direita, cima, baixo.
    pub corte: [u32; 4],
    /// VUI: quantos quadros a saida pode atrasar e o DPB maximo.
    pub reordenar: Option<u32>,
    pub max_dpb: Option<u32>,
    /// Matriz de cor BT.709 (VUI); senao BT.601.
    pub bt709: bool,
}

impl Sps {
    pub fn max_frame_num(&self) -> u32 {
        1 << (self.log2_max_frame_num_menos4 + 4)
    }

    pub fn largura(&self) -> u32 {
        self.largura_mbs * 16
    }

    pub fn altura(&self) -> u32 {
        self.altura_mbs * 16 * if self.so_quadros { 1 } else { 2 }
    }

    /// Tamanho visivel (depois do corte).
    pub fn visivel(&self) -> (u32, u32) {
        let (cx, cy) = if self.croma == 0 { (1, 1) } else { (2, if self.croma == 1 { 2 } else { 1 }) };
        let cy = cy * if self.so_quadros { 1 } else { 2 };
        let l = self.largura().saturating_sub(cx * (self.corte[0] + self.corte[1]));
        let a = self.altura().saturating_sub(cy * (self.corte[2] + self.corte[3]));
        (l, a)
    }

    /// Quantos quadros o DPB guarda (VUI, ou a tabela A-1 pelo nivel).
    pub fn tamanho_dpb(&self) -> usize {
        if let Some(n) = self.max_dpb {
            return (n as usize).clamp(1, 16);
        }
        let max_dpb_mbs: u32 = match self.nivel {
            9 | 10 => 396,
            11 => if self.restricoes & 0x10 != 0 && self.perfil != 100 { 396 } else { 900 },
            12 | 13 | 20 => 2376,
            21 => 4752,
            22 | 30 => 8100,
            31 => 18000,
            32 => 20480,
            40 | 41 => 32768,
            42 => 34816,
            50 => 110400,
            _ => 184320,
        };
        let quadro = (self.largura_mbs * self.altura_mbs * if self.so_quadros { 1 } else { 2 }).max(1);
        ((max_dpb_mbs / quadro) as usize).clamp(self.max_refs.max(1) as usize, 16)
    }
}

pub fn ler_sps(nal: &[u8]) -> R<Sps> {
    let r = rbsp(nal);
    let mut b = Bits::novo(&r);
    if b.u(8)? & 0x1F != 7 {
        return Err("nao e SPS");
    }
    let perfil = b.u(8)?;
    let restricoes = b.u(8)?;
    let nivel = b.u(8)?;
    let id = b.ue()?;
    if id > 31 {
        return Err("SPS id invalido");
    }
    let (mut croma, mut prof_luma_menos8, mut prof_croma_menos8, mut bypass_qp0, mut escala) = (1, 0, 0, false, None);
    if matches!(perfil, 100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135) {
        croma = b.ue()?;
        if croma == 3 {
            b.flag()?; // separate_colour_plane
        }
        prof_luma_menos8 = b.ue()?;
        prof_croma_menos8 = b.ue()?;
        bypass_qp0 = b.flag()?;
        if b.flag()? {
            escala = Some(ler_escala(&mut b, if croma != 3 { 2 } else { 6 }, None)?);
        }
    }
    let log2_max_frame_num_menos4 = b.ue()?;
    let tipo_poc = b.ue()?;
    let (mut log2_max_poc_lsb_menos4, mut delta_poc_sempre_zero, mut desvio_nao_ref, mut desvio_campo_baixo) = (0, false, 0, 0);
    let mut desvios_ciclo = Vec::new();
    match tipo_poc {
        0 => log2_max_poc_lsb_menos4 = b.ue()?,
        1 => {
            delta_poc_sempre_zero = b.flag()?;
            desvio_nao_ref = b.se()?;
            desvio_campo_baixo = b.se()?;
            let n = b.ue()?;
            if n > 255 {
                return Err("ciclo de POC grande demais");
            }
            for _ in 0..n {
                desvios_ciclo.push(b.se()?);
            }
        }
        2 => {}
        _ => return Err("tipo de POC invalido"),
    }
    let max_refs = b.ue()?;
    b.flag()?; // gaps_in_frame_num_value_allowed
    let largura_mbs = b.ue()? + 1;
    let altura_unidades = b.ue()? + 1;
    let so_quadros = b.flag()?;
    let mbaff = if !so_quadros { b.flag()? } else { false };
    let direto_8x8 = b.flag()?;
    let mut corte = [0; 4];
    if b.flag()? {
        for c in corte.iter_mut() {
            *c = b.ue()?;
        }
    }
    let (mut reordenar, mut max_dpb, mut bt709) = (None, None, false);
    if b.flag()? {
        vui(&mut b, &mut reordenar, &mut max_dpb, &mut bt709).ok();
    }
    if largura_mbs > 256 || altura_unidades > 256 {
        return Err("video grande demais");
    }
    Ok(Sps {
        id,
        perfil,
        restricoes,
        nivel,
        croma,
        prof_luma_menos8,
        prof_croma_menos8,
        bypass_qp0,
        escala,
        log2_max_frame_num_menos4,
        tipo_poc,
        log2_max_poc_lsb_menos4,
        delta_poc_sempre_zero,
        desvio_nao_ref,
        desvio_campo_baixo,
        desvios_ciclo,
        max_refs,
        largura_mbs,
        altura_mbs: altura_unidades,
        so_quadros,
        mbaff,
        direto_8x8,
        corte,
        reordenar,
        max_dpb,
        bt709,
    })
}

/// O VUI (E.1.1): so a cor e o bitstream_restriction interessam.
fn vui(b: &mut Bits, reordenar: &mut Option<u32>, max_dpb: &mut Option<u32>, bt709: &mut bool) -> R<()> {
    if b.flag()? {
        // aspect_ratio_info: 255 = Extended_SAR
        if b.u(8)? == 255 {
            b.u(32)?;
        }
    }
    if b.flag()? {
        b.flag()?; // overscan_appropriate
    }
    if b.flag()? {
        b.u(4)?; // video_format, full_range
        if b.flag()? {
            let primarias = b.u(8)?;
            b.u(8)?;
            let matriz = b.u(8)?;
            *bt709 = matriz == 1 || (matriz == 2 && primarias == 1);
        }
    }
    if b.flag()? {
        b.ue()?;
        b.ue()?;
    }
    if b.flag()? {
        b.u(32)?;
        b.u(32)?;
        b.flag()?;
    }
    let nal_hrd = b.flag()?;
    if nal_hrd {
        hrd(b)?;
    }
    let vcl_hrd = b.flag()?;
    if vcl_hrd {
        hrd(b)?;
    }
    if nal_hrd || vcl_hrd {
        b.flag()?; // low_delay_hrd
    }
    b.flag()?; // pic_struct_present
    if b.flag()? {
        b.flag()?;
        b.ue()?;
        b.ue()?;
        b.ue()?;
        b.ue()?;
        *reordenar = Some(b.ue()?);
        *max_dpb = Some(b.ue()?);
    }
    Ok(())
}

fn hrd(b: &mut Bits) -> R<()> {
    let n = b.ue()? + 1;
    if n > 32 {
        return Err("HRD invalido");
    }
    b.u(8)?;
    for _ in 0..n {
        b.ue()?;
        b.ue()?;
        b.flag()?;
    }
    b.u(20)?;
    Ok(())
}

#[derive(Clone, Debug)]
pub struct Pps {
    pub id: u32,
    pub sps_id: u32,
    pub cabac: bool,
    pub poc_campo_baixo: bool,
    pub refs_l0_menos1: u32,
    pub refs_l1_menos1: u32,
    pub predicao_pesos: bool,
    pub bipred_pesos: u32,
    pub qp_inicial_menos26: i32,
    pub qs_inicial_menos26: i32,
    pub desvio_qp_croma: i32,
    pub controle_desbloqueio: bool,
    pub intra_restrito: bool,
    pub redundante: bool,
    pub transformada_8x8: bool,
    /// Listas proprias do PPS (None = as do SPS).
    pub escala: Option<Escala>,
    pub desvio_qp_croma2: i32,
}

/// `spss`: os SPS ja lidos (o PPS com listas de escala depende do SPS).
pub fn ler_pps(nal: &[u8], spss: &[Option<Sps>]) -> R<Pps> {
    let r = rbsp(nal);
    let mut b = Bits::novo(&r);
    if b.u(8)? & 0x1F != 8 {
        return Err("nao e PPS");
    }
    let id = b.ue()?;
    let sps_id = b.ue()?;
    if id > 255 || sps_id > 31 {
        return Err("PPS id invalido");
    }
    let cabac = b.flag()?;
    let poc_campo_baixo = b.flag()?;
    if b.ue()? != 0 {
        return Err("grupos de fatias (FMO) nao suportados");
    }
    let refs_l0_menos1 = b.ue()?;
    let refs_l1_menos1 = b.ue()?;
    let predicao_pesos = b.flag()?;
    let bipred_pesos = b.u(2)?;
    let qp_inicial_menos26 = b.se()?;
    let qs_inicial_menos26 = b.se()?;
    let desvio_qp_croma = b.se()?;
    let controle_desbloqueio = b.flag()?;
    let intra_restrito = b.flag()?;
    let redundante = b.flag()?;
    let (mut transformada_8x8, mut escala, mut desvio_qp_croma2) = (false, None, desvio_qp_croma);
    if b.mais_dados() {
        transformada_8x8 = b.flag()?;
        if b.flag()? {
            let sps = spss.get(sps_id as usize).and_then(|s| s.as_ref());
            let croma = sps.map(|s| s.croma).unwrap_or(1);
            let n8 = if transformada_8x8 { if croma != 3 { 2 } else { 6 } } else { 0 };
            let plana = Escala::plana();
            // regra B: o que falta vem do SPS (ou das padrao, se o SPS nao tem)
            let base = sps.and_then(|s| s.escala.clone()).unwrap_or_else(|| {
                let mut b0 = plana.clone();
                b0.l4[0] = desfazer_zigue(&PADRAO4_INTRA);
                b0.l4[3] = desfazer_zigue(&PADRAO4_INTER);
                b0.l8[0] = desfazer_zigue(&PADRAO8_INTRA);
                b0.l8[1] = desfazer_zigue(&PADRAO8_INTER);
                b0
            });
            escala = Some(ler_escala(&mut b, n8, Some(&base))?);
        }
        desvio_qp_croma2 = b.se()?;
    }
    Ok(Pps {
        id,
        sps_id,
        cabac,
        poc_campo_baixo,
        refs_l0_menos1,
        refs_l1_menos1,
        predicao_pesos,
        bipred_pesos,
        qp_inicial_menos26,
        qs_inicial_menos26,
        desvio_qp_croma,
        controle_desbloqueio,
        intra_restrito,
        redundante,
        transformada_8x8,
        escala,
        desvio_qp_croma2,
    })
}

// ----------------------------------------------------- cabecalho da fatia

/// Uma operacao MMCO (memory_management_control_operation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mmco {
    /// 1: tira a curta com esse PicNum = CurrPicNum - (dif + 1).
    TirarCurta(u32),
    /// 2: tira a longa com esse LongTermPicNum.
    TirarLonga(u32),
    /// 3: curta (dif) vira longa com o indice.
    CurtaParaLonga(u32, u32),
    /// 4: maior indice de longa + 1.
    MaxLonga(u32),
    /// 5: tira todas.
    TirarTodas,
    /// 6: o quadro atual vira longa com o indice.
    AtualLonga(u32),
}

#[derive(Clone, Debug)]
pub struct Fatia {
    pub tipo_nal: u32,
    pub nal_ref: u32,
    pub primeiro_mb: u32,
    /// 0 P, 1 B, 2 I, 3 SP, 4 SI (ja sem o +5).
    pub tipo: u32,
    pub pps_id: u32,
    pub frame_num: u32,
    pub idr_id: u32,
    pub poc_lsb: u32,
    pub delta_poc_baixo: i32,
    pub delta_poc: [i32; 2],
    /// Marcacao de referencias (so se nal_ref != 0).
    pub idr_longa: bool,
    pub mmcos: Vec<Mmco>,
    pub adaptativo: bool,
}

impl Fatia {
    pub fn idr(&self) -> bool {
        self.tipo_nal == 5
    }
}

/// Le o cabecalho de uma fatia (NAL 1 ou 5) ate a marcacao de referencias.
pub fn ler_fatia(nal: &[u8], spss: &[Option<Sps>], ppss: &[Option<Pps>]) -> R<Fatia> {
    // o comeco basta (o resto e a fatia em si); 256 bytes cobrem o cabecalho
    // quase sempre, mas listas de pesos longas pedem mais
    let r = rbsp(&nal[..nal.len().min(2048)]);
    let mut b = Bits::novo(&r);
    let cab = b.u(8)?;
    let (nal_ref, tipo_nal) = (cab >> 5 & 3, cab & 0x1F);
    if tipo_nal != 1 && tipo_nal != 5 {
        return Err("nao e fatia");
    }
    let primeiro_mb = b.ue()?;
    let tipo = b.ue()? % 5;
    let pps_id = b.ue()?;
    let pps = ppss.get(pps_id as usize).and_then(|p| p.as_ref()).ok_or("fatia com PPS desconhecido")?;
    let sps = spss.get(pps.sps_id as usize).and_then(|s| s.as_ref()).ok_or("fatia com SPS desconhecido")?;
    let frame_num = b.u(sps.log2_max_frame_num_menos4 + 4)?;
    if !sps.so_quadros {
        return Err("video entrelacado nao suportado");
    }
    let idr_id = if tipo_nal == 5 { b.ue()? } else { 0 };
    let (mut poc_lsb, mut delta_poc_baixo, mut delta_poc) = (0, 0, [0; 2]);
    if sps.tipo_poc == 0 {
        poc_lsb = b.u(sps.log2_max_poc_lsb_menos4 + 4)?;
        if pps.poc_campo_baixo {
            delta_poc_baixo = b.se()?;
        }
    }
    if sps.tipo_poc == 1 && !sps.delta_poc_sempre_zero {
        delta_poc[0] = b.se()?;
        if pps.poc_campo_baixo {
            delta_poc[1] = b.se()?;
        }
    }
    if pps.redundante {
        b.ue()?;
    }
    if tipo == 1 {
        b.flag()?; // direct_spatial_mv_pred
    }
    let (mut n0, mut n1) = (pps.refs_l0_menos1, pps.refs_l1_menos1);
    if tipo == 0 || tipo == 1 || tipo == 3 {
        if b.flag()? {
            n0 = b.ue()?;
            if tipo == 1 {
                n1 = b.ue()?;
            }
        }
    }
    if n0 > 31 || n1 > 31 {
        return Err("referencias demais");
    }
    // ref_pic_list_modification
    if tipo != 2 && tipo != 4 {
        modificacao_lista(&mut b)?;
    }
    if tipo == 1 {
        modificacao_lista(&mut b)?;
    }
    if (pps.predicao_pesos && (tipo == 0 || tipo == 3)) || (pps.bipred_pesos == 1 && tipo == 1) {
        tabela_pesos(&mut b, sps, tipo, n0, n1)?;
    }
    let (mut idr_longa, mut adaptativo, mut mmcos) = (false, false, Vec::new());
    if nal_ref != 0 {
        if tipo_nal == 5 {
            b.flag()?; // no_output_of_prior_pics
            idr_longa = b.flag()?;
        } else {
            adaptativo = b.flag()?;
            if adaptativo {
                loop {
                    let op = b.ue()?;
                    let m = match op {
                        0 => break,
                        1 => Mmco::TirarCurta(b.ue()?),
                        2 => Mmco::TirarLonga(b.ue()?),
                        3 => {
                            let d = b.ue()?;
                            Mmco::CurtaParaLonga(d, b.ue()?)
                        }
                        4 => Mmco::MaxLonga(b.ue()?),
                        5 => Mmco::TirarTodas,
                        6 => Mmco::AtualLonga(b.ue()?),
                        _ => return Err("MMCO invalida"),
                    };
                    mmcos.push(m);
                    if mmcos.len() > 66 {
                        return Err("MMCOs demais");
                    }
                }
            }
        }
    }
    Ok(Fatia { tipo_nal, nal_ref, primeiro_mb, tipo, pps_id, frame_num, idr_id, poc_lsb, delta_poc_baixo, delta_poc, idr_longa, mmcos, adaptativo })
}

fn modificacao_lista(b: &mut Bits) -> R<()> {
    if b.flag()? {
        for _ in 0..100 {
            match b.ue()? {
                3 => return Ok(()),
                0..=2 | 4 | 5 => {
                    b.ue()?;
                }
                _ => return Err("modificacao de lista invalida"),
            }
        }
        return Err("modificacao de lista sem fim");
    }
    Ok(())
}

fn tabela_pesos(b: &mut Bits, sps: &Sps, tipo: u32, n0: u32, n1: u32) -> R<()> {
    b.ue()?; // luma_log2_weight_denom
    if sps.croma != 0 {
        b.ue()?;
    }
    for lista in 0..if tipo == 1 { 2 } else { 1 } {
        let n = if lista == 0 { n0 } else { n1 };
        for _ in 0..=n {
            if b.flag()? {
                b.se()?;
                b.se()?;
            }
            if sps.croma != 0 && b.flag()? {
                for _ in 0..4 {
                    b.se()?;
                }
            }
        }
    }
    Ok(())
}

// --------------------------------------------------------------- POC

/// O estado entre quadros para a conta do POC (8.2.1).
#[derive(Default, Clone)]
pub struct EstadoPoc {
    msb_ant: i32,
    lsb_ant: i32,
    frame_num_ant: u32,
    desvio_frame_num_ant: i32,
}

impl EstadoPoc {
    /// POC (topo, baixo) do quadro atual. `mmco5_ant` = o quadro de
    /// referencia anterior teve a MMCO 5 (e `poc_topo_ant` o POC dele ja
    /// ajustado).
    pub fn calcular(&mut self, sps: &Sps, f: &Fatia) -> (i32, i32) {
        let max_frame_num = sps.max_frame_num() as i32;
        if f.idr() {
            self.frame_num_ant = 0;
            self.desvio_frame_num_ant = 0;
        }
        let desvio_frame_num = if f.idr() {
            0
        } else if self.frame_num_ant > f.frame_num {
            self.desvio_frame_num_ant + max_frame_num
        } else {
            self.desvio_frame_num_ant
        };
        let (topo, baixo) = match sps.tipo_poc {
            0 => {
                if f.idr() {
                    self.msb_ant = 0;
                    self.lsb_ant = 0;
                }
                let max_lsb = 1i32 << (sps.log2_max_poc_lsb_menos4 + 4);
                let lsb = f.poc_lsb as i32;
                let msb = if lsb < self.lsb_ant && self.lsb_ant - lsb >= max_lsb / 2 {
                    self.msb_ant + max_lsb
                } else if lsb > self.lsb_ant && lsb - self.lsb_ant > max_lsb / 2 {
                    self.msb_ant - max_lsb
                } else {
                    self.msb_ant
                };
                let topo = msb + lsb;
                if f.nal_ref != 0 {
                    self.msb_ant = msb;
                    self.lsb_ant = lsb;
                }
                (topo, topo + f.delta_poc_baixo)
            }
            1 => {
                let n = sps.desvios_ciclo.len() as i32;
                let mut abs = if n != 0 { desvio_frame_num + f.frame_num as i32 } else { 0 };
                if f.nal_ref == 0 && abs > 0 {
                    abs -= 1;
                }
                let mut esperado = 0;
                if abs > 0 {
                    let delta_ciclo: i32 = sps.desvios_ciclo.iter().sum();
                    let ciclo = (abs - 1) / n;
                    let no_ciclo = (abs - 1) % n;
                    esperado = ciclo * delta_ciclo;
                    for i in 0..=no_ciclo as usize {
                        esperado += sps.desvios_ciclo[i];
                    }
                }
                if f.nal_ref == 0 {
                    esperado += sps.desvio_nao_ref;
                }
                let topo = esperado + f.delta_poc[0];
                (topo, topo + sps.desvio_campo_baixo + f.delta_poc[1])
            }
            _ => {
                let t = if f.idr() {
                    0
                } else if f.nal_ref == 0 {
                    2 * (desvio_frame_num + f.frame_num as i32) - 1
                } else {
                    2 * (desvio_frame_num + f.frame_num as i32)
                };
                (t, t)
            }
        };
        self.frame_num_ant = f.frame_num;
        self.desvio_frame_num_ant = desvio_frame_num;
        (topo, baixo)
    }

    /// Depois de um quadro com MMCO 5: ele passa a valer frame_num 0 e POC
    /// relativo a ele mesmo.
    pub fn depois_mmco5(&mut self, poc_topo_ajustado: i32) {
        self.msb_ant = 0;
        self.lsb_ant = poc_topo_ajustado;
        self.frame_num_ant = 0;
        self.desvio_frame_num_ant = 0;
    }
}

// --------------------------------------------------------------- DPB

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ref {
    Nao,
    Curta,
    Longa,
}

/// Um lugar do DPB: o quadro decodificado no buffer `buffer`.
#[derive(Clone, Copy, Debug)]
pub struct Lugar {
    pub ocupado: bool,
    pub buffer: usize,
    pub frame_num: u32,
    /// FrameNumWrap das curtas (conta de novo a cada quadro).
    pub frame_num_wrap: i32,
    pub indice_longa: u32,
    pub poc: (i32, i32),
    pub referencia: Ref,
    pub falta_mostrar: bool,
}

impl Lugar {
    const VAZIO: Lugar = Lugar { ocupado: false, buffer: 0, frame_num: 0, frame_num_wrap: 0, indice_longa: 0, poc: (0, 0), referencia: Ref::Nao, falta_mostrar: false };

    pub fn poc(&self) -> i32 {
        self.poc.0.min(self.poc.1)
    }
}

/// Uma entrada das listas de referencia: o lugar do DPB (0..15).
pub type ListaRef = Vec<u8>;

/// O que o hardware precisa para um quadro (o resto ele le da fatia).
pub struct Quadro {
    pub sps_id: u32,
    pub pps_id: u32,
    pub poc: (i32, i32),
    pub frame_num: u32,
    pub idr: bool,
    pub referencia: bool,
    /// Onde decodificar.
    pub buffer: usize,
    /// O DPB como o hardware ve (16 lugares).
    pub dpb: [Lugar; 16],
    pub lista_p: ListaRef,
    pub lista_b0: ListaRef,
    pub lista_b1: ListaRef,
}

pub struct Dpb {
    /// So as referencias (o DPB que o hardware ve). Uma referencia ainda
    /// nao mostrada fica aqui com `falta_mostrar`.
    pub lugares: [Lugar; 16],
    /// Quadros que nao sao referencia esperando a vez de mostrar: (buffer, POC).
    espera: Vec<(usize, i32)>,
    /// Tamanho do DPB pelo SPS (informativo: o limite real sao as referencias).
    pub tamanho: usize,
    /// A saida atrasa no maximo isso (num_reorder_frames do VUI).
    pub reordenar: usize,
    max_longa: Option<u32>,
    poc: EstadoPoc,
    pub buffers: usize,
    /// Buffer pego por `comecar` e ainda nao entregue a `terminar`.
    usados: Vec<bool>,
    /// Quadros prontos para mostrar, na ordem: (buffer, POC). O player tira
    /// daqui e mostra na hora (o buffer volta a ser livre quando sai).
    pub saida: alloc::collections::VecDeque<(usize, i32)>,
}

impl Dpb {
    /// `buffers`: quantos buffers de quadro existem (`buffers_para`).
    pub fn novo(sps: &Sps, buffers: usize) -> Dpb {
        let tamanho = sps.tamanho_dpb();
        Dpb {
            lugares: [Lugar::VAZIO; 16],
            espera: Vec::new(),
            tamanho,
            reordenar: Dpb::atraso(sps),
            max_longa: None,
            poc: EstadoPoc::default(),
            buffers,
            usados: vec![false; buffers],
            saida: Default::default(),
        }
    }

    fn atraso(sps: &Sps) -> usize {
        sps.reordenar.map(|r| r as usize).unwrap_or(sps.tamanho_dpb()).min(16)
    }

    /// Quantos buffers o player deve criar para esse SPS: as referencias,
    /// os que esperam a vez, o atual e um de folga.
    pub fn buffers_para(sps: &Sps) -> usize {
        sps.max_refs.max(1) as usize + Dpb::atraso(sps) + 2
    }

    /// Manda para a saida o de menor POC entre os que esperam.
    fn mostrar_um(&mut self) -> bool {
        let lugar = (0..16).filter(|&i| self.lugares[i].ocupado && self.lugares[i].falta_mostrar).min_by_key(|&i| self.lugares[i].poc());
        let espera = (0..self.espera.len()).min_by_key(|&i| self.espera[i].1);
        let poc_lugar = lugar.map(|i| self.lugares[i].poc());
        let poc_espera = espera.map(|i| self.espera[i].1);
        match (poc_lugar, poc_espera) {
            (Some(pl), pe) if pe.is_none_or(|pe| pl <= pe) => {
                let l = &mut self.lugares[lugar.unwrap()];
                l.falta_mostrar = false;
                self.saida.push_back((l.buffer, pl));
                if l.referencia == Ref::Nao {
                    l.ocupado = false;
                }
                true
            }
            (_, Some(_)) => {
                let e = self.espera.remove(espera.unwrap());
                self.saida.push_back(e);
                true
            }
            _ => false,
        }
    }

    fn esperando(&self) -> usize {
        self.espera.len() + self.lugares.iter().filter(|l| l.ocupado && l.falta_mostrar).count()
    }

    /// Tudo o que falta mostrar vai para a saida (fim do video, IDR).
    pub fn esvaziar(&mut self) {
        while self.mostrar_um() {}
    }

    /// O player largou esse buffer sem passar por `terminar` (erro do
    /// hardware): pode ser reusado.
    pub fn devolver(&mut self, buffer: usize) {
        if buffer < self.usados.len() {
            self.usados[buffer] = false;
        }
    }

    fn pegar_buffer(&mut self) -> Option<usize> {
        // ocupado: referencia, esperando a vez, na saida ou com o hardware
        for b in 0..self.buffers {
            let ocupado = self.usados[b]
                || self.lugares.iter().any(|l| l.ocupado && l.buffer == b)
                || self.espera.iter().any(|e| e.0 == b)
                || self.saida.iter().any(|s| s.0 == b);
            if !ocupado {
                self.usados[b] = true;
                return Some(b);
            }
        }
        None
    }

    /// Prepara o quadro desta fatia (a primeira do quadro): POC, listas,
    /// buffer. Depois de o hardware decodificar, chamar `terminar`.
    pub fn comecar(&mut self, sps: &Sps, f: &Fatia) -> R<Quadro> {
        if f.idr() {
            // IDR: as referencias somem; o que falta mostrar sai antes
            self.esvaziar();
            self.lugares = [Lugar::VAZIO; 16];
            self.max_longa = None;
        }
        let poc = self.poc.calcular(sps, f);
        let max_frame_num = sps.max_frame_num() as i32;
        for l in self.lugares.iter_mut() {
            if l.ocupado && l.referencia == Ref::Curta {
                l.frame_num_wrap = if l.frame_num > f.frame_num { l.frame_num as i32 - max_frame_num } else { l.frame_num as i32 };
            }
        }
        let buffer = self.pegar_buffer().ok_or("sem buffer livre (o player segurou quadros demais)")?;
        let (p, b0, b1) = self.listas(poc);
        Ok(Quadro {
            sps_id: sps.id,
            pps_id: f.pps_id,
            poc,
            frame_num: f.frame_num,
            idr: f.idr(),
            referencia: f.nal_ref != 0,
            buffer,
            dpb: self.lugares,
            lista_p: p,
            lista_b0: b0,
            lista_b1: b1,
        })
    }

    /// Listas iniciais (8.2.4.2.1, .3) para um quadro com POC `poc`.
    fn listas(&self, poc: (i32, i32)) -> (ListaRef, ListaRef, ListaRef) {
        let atual = poc.0.min(poc.1);
        let l = |i: &u8| &self.lugares[*i as usize];
        let refs: Vec<u8> = (0..16u8).filter(|i| l(i).ocupado && l(i).referencia != Ref::Nao).collect();
        let curtas: Vec<u8> = refs.iter().copied().filter(|i| l(i).referencia == Ref::Curta).collect();
        let mut longas: Vec<u8> = refs.iter().copied().filter(|i| l(i).referencia == Ref::Longa).collect();
        longas.sort_by_key(|i| l(i).indice_longa);
        // P: curtas por PicNum descendo, depois longas
        let mut p = curtas.clone();
        p.sort_by_key(|i| core::cmp::Reverse(l(i).frame_num_wrap));
        p.extend_from_slice(&longas);
        // B0: POC menor que o atual (descendo), depois maior (subindo)
        let mut antes: Vec<u8> = curtas.iter().copied().filter(|i| l(i).poc() < atual).collect();
        antes.sort_by_key(|i| core::cmp::Reverse(l(i).poc()));
        let mut depois: Vec<u8> = curtas.iter().copied().filter(|i| l(i).poc() >= atual).collect();
        depois.sort_by_key(|i| l(i).poc());
        let mut b0 = antes.clone();
        b0.extend_from_slice(&depois);
        b0.extend_from_slice(&longas);
        let mut b1 = depois;
        b1.extend_from_slice(&antes);
        b1.extend_from_slice(&longas);
        if b1.len() > 1 && b0 == b1 {
            b1.swap(0, 1);
        }
        (p, b0, b1)
    }

    /// O hardware decodificou o quadro: marca referencias (8.2.5) e guarda.
    /// O que ficou pronto para mostrar vai para `saida`.
    pub fn terminar(&mut self, sps: &Sps, f: &Fatia, q: &Quadro) {
        let max_refs = sps.max_refs.max(1) as usize;
        let mut poc = q.poc;
        let mut frame_num = f.frame_num;
        let mut atual = if f.nal_ref != 0 { Ref::Curta } else { Ref::Nao };
        let mut indice_atual = 0;
        if f.nal_ref != 0 {
            if f.idr() {
                if f.idr_longa {
                    atual = Ref::Longa;
                    self.max_longa = Some(0);
                }
            } else if f.adaptativo {
                self.mmcos(f, &mut poc, &mut frame_num, &mut atual, &mut indice_atual);
            } else {
                // janela deslizante: tira a curta mais antiga se encheu
                let total = self.lugares.iter().filter(|l| l.ocupado && l.referencia != Ref::Nao).count();
                if total >= max_refs {
                    if let Some(i) = (0..16).filter(|&i| self.lugares[i].ocupado && self.lugares[i].referencia == Ref::Curta).min_by_key(|&i| self.lugares[i].frame_num_wrap) {
                        self.lugares[i].referencia = Ref::Nao;
                    }
                }
            }
        }
        // quem deixou de ser referencia sai do DPB do hardware (e, se ainda
        // nao foi mostrado, vai esperar a vez fora dele)
        for l in self.lugares.iter_mut() {
            if l.ocupado && l.referencia == Ref::Nao {
                if l.falta_mostrar {
                    self.espera.push((l.buffer, l.poc()));
                }
                l.ocupado = false;
            }
        }
        self.usados[q.buffer] = false;
        if atual == Ref::Nao {
            self.espera.push((q.buffer, poc.0.min(poc.1)));
        } else if let Some(i) = (0..16).find(|&i| !self.lugares[i].ocupado) {
            self.lugares[i] = Lugar { ocupado: true, buffer: q.buffer, frame_num, frame_num_wrap: frame_num as i32, indice_longa: indice_atual, poc, referencia: atual, falta_mostrar: true };
        } else {
            // mais de 16 referencias: fluxo invalido; mostra e segue
            self.espera.push((q.buffer, poc.0.min(poc.1)));
        }
        // a saida atrasa no maximo `reordenar` quadros
        while self.esperando() > self.reordenar {
            if !self.mostrar_um() {
                break;
            }
        }
    }

    fn mmcos(&mut self, f: &Fatia, poc: &mut (i32, i32), frame_num: &mut u32, atual: &mut Ref, indice_atual: &mut u32) {
        let curr_pic_num = f.frame_num as i32;
        let tirar = |ls: &mut [Lugar; 16], tipo: Ref, ok: &dyn Fn(&Lugar) -> bool| {
            for l in ls.iter_mut() {
                if l.ocupado && l.referencia == tipo && ok(l) {
                    l.referencia = Ref::Nao;
                }
            }
        };
        for m in &f.mmcos {
            match *m {
                Mmco::TirarCurta(d) => {
                    let x = curr_pic_num - (d as i32 + 1);
                    tirar(&mut self.lugares, Ref::Curta, &|l| l.frame_num_wrap == x);
                }
                Mmco::TirarLonga(n) => tirar(&mut self.lugares, Ref::Longa, &|l| l.indice_longa == n),
                Mmco::CurtaParaLonga(d, idx) => {
                    let x = curr_pic_num - (d as i32 + 1);
                    tirar(&mut self.lugares, Ref::Longa, &|l| l.indice_longa == idx);
                    for l in self.lugares.iter_mut() {
                        if l.ocupado && l.referencia == Ref::Curta && l.frame_num_wrap == x {
                            l.referencia = Ref::Longa;
                            l.indice_longa = idx;
                        }
                    }
                }
                Mmco::MaxLonga(n) => {
                    self.max_longa = n.checked_sub(1);
                    let max = self.max_longa;
                    tirar(&mut self.lugares, Ref::Longa, &|l| max.is_none_or(|m| l.indice_longa > m));
                }
                Mmco::TirarTodas => {
                    for l in self.lugares.iter_mut() {
                        l.referencia = Ref::Nao;
                    }
                    self.max_longa = None;
                    // o quadro passa a ser o comeco: POC relativo a ele, e o
                    // que falta mostrar vem antes
                    let t = poc.0.min(poc.1);
                    *poc = (poc.0 - t, poc.1 - t);
                    *frame_num = 0;
                    self.poc.depois_mmco5(poc.0);
                    for l in self.lugares.iter_mut() {
                        if l.ocupado && l.falta_mostrar {
                            self.espera.push((l.buffer, l.poc()));
                        }
                        l.ocupado = false;
                    }
                    self.esvaziar();
                }
                Mmco::AtualLonga(idx) => {
                    tirar(&mut self.lugares, Ref::Longa, &|l| l.indice_longa == idx);
                    *atual = Ref::Longa;
                    *indice_atual = idx;
                }
            }
        }
    }
}
