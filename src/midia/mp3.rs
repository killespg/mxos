//! MP3 (MPEG-1, 2 e 2.5 Layer III), decodificado aqui: cabecalho, side
//! info, reservatorio de bits, fatores de escala, Huffman (tabelas do
//! padrao em mp3_tabelas.rs), requantizacao, estereo M/S e de intensidade,
//! reordenacao, anti-aliasing, IMDCT com janelas longas/curtas e a sintese
//! polifasica. O arquivo e lido aos quadros (~400 bytes cada): a memoria
//! fica em umas dezenas de KB, nao importa o tamanho da musica.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::mp3_tabelas as t;
use super::{Audio, Fonte};
use crate::web::js::mat64;

const BITRATE_1: [u32; 15] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320];
const BITRATE_2: [u32; 15] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];
const TAXAS: [u32; 3] = [44100, 48000, 32000];

/// Limites das bandas dos fatores de escala (longas, curtas), por taxa:
/// 44.1, 48, 32 (MPEG-1); 22.05, 24, 16 (MPEG-2); 11.025, 12, 8 (2.5).
const SFB_LONGA: [[u16; 23]; 9] = [
    [0, 4, 8, 12, 16, 20, 24, 30, 36, 44, 52, 62, 74, 90, 110, 134, 162, 196, 238, 288, 342, 418, 576],
    [0, 4, 8, 12, 16, 20, 24, 30, 36, 42, 50, 60, 72, 88, 106, 128, 156, 190, 230, 276, 330, 384, 576],
    [0, 4, 8, 12, 16, 20, 24, 30, 36, 44, 54, 66, 82, 102, 126, 156, 194, 240, 296, 364, 448, 550, 576],
    [0, 6, 12, 18, 24, 30, 36, 44, 54, 66, 80, 96, 116, 140, 168, 200, 238, 284, 336, 396, 464, 522, 576],
    [0, 6, 12, 18, 24, 30, 36, 44, 54, 66, 80, 96, 114, 136, 162, 194, 232, 278, 332, 394, 464, 540, 576],
    [0, 6, 12, 18, 24, 30, 36, 44, 54, 66, 80, 96, 116, 140, 168, 200, 238, 284, 336, 396, 464, 522, 576],
    [0, 6, 12, 18, 24, 30, 36, 44, 54, 66, 80, 96, 116, 140, 168, 200, 238, 284, 336, 396, 464, 522, 576],
    [0, 6, 12, 18, 24, 30, 36, 44, 54, 66, 80, 96, 116, 140, 168, 200, 238, 284, 336, 396, 464, 522, 576],
    [0, 12, 24, 36, 48, 60, 72, 88, 108, 132, 160, 192, 232, 280, 336, 400, 476, 566, 568, 570, 572, 574, 576],
];
const SFB_CURTA: [[u16; 14]; 9] = [
    [0, 4, 8, 12, 16, 22, 30, 40, 52, 66, 84, 106, 136, 192],
    [0, 4, 8, 12, 16, 22, 28, 38, 50, 64, 80, 100, 126, 192],
    [0, 4, 8, 12, 16, 22, 30, 42, 58, 78, 104, 138, 180, 192],
    [0, 4, 8, 12, 18, 24, 32, 42, 56, 74, 100, 132, 174, 192],
    [0, 4, 8, 12, 18, 26, 36, 48, 62, 80, 104, 136, 180, 192],
    [0, 4, 8, 12, 18, 26, 36, 48, 62, 80, 104, 134, 174, 192],
    [0, 4, 8, 12, 18, 26, 36, 48, 62, 80, 104, 134, 174, 192],
    [0, 4, 8, 12, 18, 26, 36, 48, 62, 80, 104, 134, 174, 192],
    [0, 8, 16, 24, 36, 52, 72, 96, 124, 160, 162, 164, 166, 192],
];
const SLEN: [(u32, u32); 16] =
    [(0, 0), (0, 1), (0, 2), (0, 3), (3, 0), (1, 1), (1, 2), (1, 3), (2, 1), (2, 2), (2, 3), (3, 1), (3, 2), (3, 3), (4, 2), (4, 3)];
const PRETAB: [i32; 22] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 3, 3, 3, 2, 0];
/// Quantos fatores de escala em cada uma das 4 partes (MPEG-2):
/// [tabela][bloco: 0 longo, 1 curto, 2 misto][parte].
const NR_SFB: [[[u8; 4]; 3]; 6] = [
    [[6, 5, 5, 5], [9, 9, 9, 9], [6, 9, 9, 9]],
    [[6, 5, 7, 3], [9, 9, 12, 6], [6, 9, 12, 6]],
    [[11, 10, 0, 0], [18, 18, 0, 0], [15, 18, 0, 0]],
    [[7, 7, 7, 0], [12, 12, 12, 0], [6, 15, 12, 0]],
    [[6, 6, 6, 3], [12, 9, 9, 6], [6, 12, 9, 6]],
    [[8, 8, 5, 0], [15, 12, 9, 0], [6, 18, 9, 0]],
];
const C_ALIAS: [f64; 8] = [-0.6, -0.535, -0.33, -0.185, -0.095, -0.041, -0.0142, -0.0037];

#[derive(Clone, Copy)]
struct Cabecalho {
    /// 0 = MPEG-1, 1 = MPEG-2, 2 = MPEG-2.5
    versao: usize,
    crc: bool,
    bitrate: u32,
    taxa: u32,
    indice_taxa: usize,
    #[allow(dead_code)]
    padding: bool,
    modo: u32,
    modo_ext: u32,
    tamanho: usize,
}

impl Cabecalho {
    fn ler(b: &[u8]) -> Option<Cabecalho> {
        if b.len() < 4 || b[0] != 0xFF || b[1] & 0xE0 != 0xE0 {
            return None;
        }
        let versao = match (b[1] >> 3) & 3 {
            3 => 0,
            2 => 1,
            0 => 2,
            _ => return None,
        };
        if (b[1] >> 1) & 3 != 1 {
            return None; // so Layer III
        }
        let ib = (b[2] >> 4) as usize;
        let it = ((b[2] >> 2) & 3) as usize;
        if ib == 0 || ib == 15 || it == 3 {
            return None;
        }
        let bitrate = if versao == 0 { BITRATE_1[ib] } else { BITRATE_2[ib] };
        let taxa = TAXAS[it] >> versao;
        let padding = (b[2] >> 1) & 1 == 1;
        let fator = if versao == 0 { 144 } else { 72 };
        let tamanho = (fator * bitrate * 1000 / taxa) as usize + padding as usize;
        Some(Cabecalho {
            versao,
            crc: b[1] & 1 == 0,
            bitrate,
            taxa,
            indice_taxa: versao * 3 + it,
            padding,
            modo: (b[3] >> 6) as u32,
            modo_ext: ((b[3] >> 4) & 3) as u32,
            tamanho,
        })
    }

    fn canais(&self) -> usize {
        if self.modo == 3 { 1 } else { 2 }
    }

    fn granulos(&self) -> usize {
        if self.versao == 0 { 2 } else { 1 }
    }

    fn tam_side(&self) -> usize {
        match (self.versao == 0, self.canais()) {
            (true, 1) => 17,
            (true, _) => 32,
            (false, 1) => 9,
            (false, _) => 17,
        }
    }
}

struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn bit(&mut self) -> u32 {
        let b = self.d.get(self.pos >> 3).map_or(0, |x| (x >> (7 - (self.pos & 7))) & 1);
        self.pos += 1;
        b as u32
    }
    fn ler(&mut self, n: u32) -> u32 {
        let mut v = 0;
        for _ in 0..n {
            v = (v << 1) | self.bit();
        }
        v
    }
}

/// Arvore de um codigo de Huffman: no >= 0 e filho, < 0 e folha -(valor+1).
struct Arvore(Vec<[i32; 2]>);

impl Arvore {
    fn nova(codigos: &[u16], tamanhos: &[u8]) -> Arvore {
        let mut nos: Vec<[i32; 2]> = alloc::vec![[0, 0]];
        for (v, (&c, &l)) in codigos.iter().zip(tamanhos).enumerate() {
            let mut no = 0usize;
            for k in (0..l).rev() {
                let b = ((c >> k) & 1) as usize;
                if k == 0 {
                    nos[no][b] = -(v as i32 + 1);
                } else {
                    if nos[no][b] <= 0 {
                        nos.push([0, 0]);
                        let novo = nos.len() as i32 - 1;
                        nos[no][b] = novo;
                    }
                    no = nos[no][b] as usize;
                }
            }
        }
        Arvore(nos)
    }

    fn decodificar(&self, b: &mut Bits) -> usize {
        let mut no = 0usize;
        for _ in 0..32 {
            let prox = self.0[no][b.bit() as usize];
            if prox < 0 {
                return (-prox - 1) as usize;
            }
            if prox == 0 {
                return 0; // codigo invalido
            }
            no = prox as usize;
        }
        0
    }
}

#[derive(Clone, Copy, Default)]
struct Granulo {
    p23: usize,
    big: usize,
    ganho: i32,
    sfc: u32,
    ws: bool,
    bt: u8,
    misto: bool,
    tabela: [usize; 3],
    subganho: [i32; 3],
    r0: usize,
    r1: usize,
    preflag: bool,
    sf_escala: bool,
    c1: bool,
}

#[derive(Clone, Copy, Default)]
struct Fatores {
    l: [i32; 22],
    s: [[i32; 3]; 13],
}

pub struct Mp3 {
    fonte: Box<dyn Fonte>,
    tamanho: usize,
    inicio: usize,
    pos: usize,
    taxa: u32,
    canais_arquivo: usize,
    bitrate: u32,
    duracao: Option<u64>,
    reservatorio: Vec<u8>,
    arvores: Vec<Option<Arvore>>,
    pow43: Vec<f32>,
    cos_longa: Vec<f32>,
    cos_curta: Vec<f32>,
    janelas: [[f32; 36]; 4],
    sintese_n: Vec<f32>,
    sobra: [[f32; 576]; 2],
    v: [[f32; 1024]; 2],
    v_pos: [usize; 2],
    fatores: [Fatores; 2],
    saida: Vec<i16>,
    lidos: usize,
    quadros: u64,
    amostras: u64,
    /// Para descricao: versao e modo do primeiro quadro.
    primeiro: Option<Cabecalho>,
    buf: Vec<u8>,
    buf_ini: usize,
}

fn cosf(x: f64) -> f32 {
    mat64::cos(x) as f32
}

impl Mp3 {
    pub fn novo(mut fonte: Box<dyn Fonte>) -> Result<Mp3, &'static str> {
        let tamanho = fonte.tamanho();
        // tag ID3v2 no comeco
        let mut c = [0u8; 10];
        fonte.ler(0, &mut c);
        let mut inicio = 0;
        if &c[0..3] == b"ID3" {
            let n = ((c[6] as usize & 0x7F) << 21) | ((c[7] as usize & 0x7F) << 14) | ((c[8] as usize & 0x7F) << 7) | (c[9] as usize & 0x7F);
            inicio = 10 + n + if c[5] & 0x10 != 0 { 10 } else { 0 };
        }
        let mut arvores = Vec::with_capacity(34);
        for n in 0..34 {
            arvores.push(t::tabela(n).map(|(_, cod, tam)| Arvore::nova(cod, tam)));
        }
        let pow43 = (0..8207).map(|i| if i == 0 { 0.0 } else { mat64::pow(i as f64, 4.0 / 3.0) as f32 }).collect();
        let pi = core::f64::consts::PI;
        let mut cos_longa = alloc::vec![0f32; 36 * 18];
        for i in 0..36 {
            for k in 0..18 {
                cos_longa[i * 18 + k] = cosf(pi / 72.0 * (2 * i + 1 + 18) as f64 * (2 * k + 1) as f64);
            }
        }
        let mut cos_curta = alloc::vec![0f32; 12 * 6];
        for i in 0..12 {
            for k in 0..6 {
                cos_curta[i * 6 + k] = cosf(pi / 24.0 * (2 * i + 1 + 6) as f64 * (2 * k + 1) as f64);
            }
        }
        let sen = |x: f64| mat64::sin(x) as f32;
        let mut janelas = [[0f32; 36]; 4];
        for i in 0..36 {
            janelas[0][i] = sen(pi / 36.0 * (i as f64 + 0.5));
        }
        for i in 0..36 {
            janelas[1][i] = match i {
                0..=17 => sen(pi / 36.0 * (i as f64 + 0.5)),
                18..=23 => 1.0,
                24..=29 => sen(pi / 12.0 * (i as f64 - 18.0 + 0.5)),
                _ => 0.0,
            };
            janelas[3][i] = match i {
                0..=5 => 0.0,
                6..=11 => sen(pi / 12.0 * (i as f64 - 6.0 + 0.5)),
                12..=17 => 1.0,
                _ => sen(pi / 36.0 * (i as f64 + 0.5)),
            };
        }
        for i in 0..12 {
            janelas[2][i] = sen(pi / 12.0 * (i as f64 + 0.5));
        }
        let mut sintese_n = alloc::vec![0f32; 64 * 32];
        for i in 0..64 {
            for k in 0..32 {
                sintese_n[i * 32 + k] = cosf(((16 + i) * (2 * k + 1)) as f64 * pi / 64.0);
            }
        }
        let mut m = Mp3 {
            fonte,
            tamanho,
            inicio,
            pos: inicio,
            taxa: 44100,
            canais_arquivo: 2,
            bitrate: 128,
            duracao: None,
            reservatorio: Vec::new(),
            arvores,
            pow43,
            cos_longa,
            cos_curta,
            janelas,
            sintese_n,
            sobra: [[0.0; 576]; 2],
            v: [[0.0; 1024]; 2],
            v_pos: [0; 2],
            fatores: [Fatores::default(); 2],
            saida: Vec::new(),
            lidos: 0,
            quadros: 0,
            amostras: 0,
            primeiro: None,
            buf: Vec::new(),
            buf_ini: 0,
        };
        let (pos, cab) = m.achar_quadro(inicio, 64 * 1024).ok_or("não achei quadros de MP3")?;
        m.inicio = pos;
        m.pos = pos;
        m.taxa = cab.taxa;
        m.canais_arquivo = cab.canais();
        m.bitrate = cab.bitrate;
        m.primeiro = Some(cab);
        // cabecalho Xing/Info (VBR): numero de quadros, duracao exata; o
        // quadro dele e silencio e fica de fora
        let mut q = alloc::vec![0u8; cab.tamanho.min(400)];
        m.ler_em(pos, &mut q);
        let x = 4 + cab.tam_side();
        if q.len() >= x + 12 && (&q[x..x + 4] == b"Xing" || &q[x..x + 4] == b"Info") {
            let flags = u32::from_be_bytes([q[x + 4], q[x + 5], q[x + 6], q[x + 7]]);
            if flags & 1 != 0 {
                let n = u32::from_be_bytes([q[x + 8], q[x + 9], q[x + 10], q[x + 11]]) as u64;
                let spf = if cab.versao == 0 { 1152 } else { 576 };
                m.duracao = Some(n * spf * 1000 / cab.taxa as u64);
            }
            m.pos += cab.tamanho;
            m.inicio = m.pos;
        }
        if m.duracao.is_none() {
            m.duracao = Some((tamanho - m.inicio) as u64 * 8 / cab.bitrate.max(1) as u64);
        }
        Ok(m)
    }

    /// Le do arquivo com um cache de 16 KB (os quadros sao pequenos).
    fn ler_em(&mut self, pos: usize, destino: &mut [u8]) -> usize {
        let n = destino.len();
        if pos < self.buf_ini || pos + n > self.buf_ini + self.buf.len() {
            let quanto = (16 * 1024).max(n);
            self.buf.resize(quanto, 0);
            let lido = self.fonte.ler(pos, &mut self.buf);
            self.buf.truncate(lido);
            self.buf_ini = pos;
        }
        let o = pos - self.buf_ini;
        let k = n.min(self.buf.len().saturating_sub(o));
        destino[..k].copy_from_slice(&self.buf[o..o + k]);
        k
    }

    /// Proximo cabecalho valido a partir de `pos` (procura ate `limite` bytes).
    fn achar_quadro(&mut self, mut pos: usize, limite: usize) -> Option<(usize, Cabecalho)> {
        let fim = (pos + limite).min(self.tamanho);
        let mut b = [0u8; 4];
        while pos + 4 <= fim {
            self.ler_em(pos, &mut b);
            if let Some(c) = Cabecalho::ler(&b) {
                // confere o seguinte (evita falso sincronismo dentro dos dados)
                let mut b2 = [0u8; 4];
                if pos + c.tamanho + 4 > self.tamanho
                    || (self.ler_em(pos + c.tamanho, &mut b2) == 4
                        && Cabecalho::ler(&b2).is_some_and(|d| d.versao == c.versao && d.taxa == c.taxa))
                {
                    return Some((pos, c));
                }
            }
            pos += 1;
        }
        None
    }

    /// Decodifica o proximo quadro para self.saida. false = acabou.
    fn quadro(&mut self) -> bool {
        let Some((pos, cab)) = self.achar_quadro(self.pos, 8192) else { return false };
        let mut q = alloc::vec![0u8; cab.tamanho];
        if self.ler_em(pos, &mut q) < cab.tamanho {
            return false;
        }
        self.pos = pos + cab.tamanho;
        let ini_side = 4 + if cab.crc { 2 } else { 0 };
        let side = &q[ini_side..(ini_side + cab.tam_side()).min(q.len())];
        let nch = cab.canais();
        let ngr = cab.granulos();
        let mut b = Bits { d: side, pos: 0 };
        let mpeg1 = cab.versao == 0;
        let comeco = b.ler(if mpeg1 { 9 } else { 8 }) as usize;
        b.pos += match (mpeg1, nch) {
            (true, 1) => 5,
            (true, _) => 3,
            (false, 1) => 1,
            _ => 2,
        };
        let mut scfsi = [[false; 4]; 2];
        if mpeg1 {
            for s in scfsi.iter_mut().take(nch) {
                for k in s.iter_mut() {
                    *k = b.bit() == 1;
                }
            }
        }
        let mut gr = [[Granulo::default(); 2]; 2];
        for g in gr.iter_mut().take(ngr) {
            for c in g.iter_mut().take(nch) {
                c.p23 = b.ler(12) as usize;
                c.big = (b.ler(9) as usize).min(288);
                c.ganho = b.ler(8) as i32;
                c.sfc = b.ler(if mpeg1 { 4 } else { 9 });
                c.ws = b.bit() == 1;
                if c.ws {
                    c.bt = b.ler(2) as u8;
                    c.misto = b.bit() == 1;
                    c.tabela[0] = b.ler(5) as usize;
                    c.tabela[1] = b.ler(5) as usize;
                    for k in 0..3 {
                        c.subganho[k] = b.ler(3) as i32;
                    }
                    c.r0 = if c.bt == 2 && !c.misto { 8 } else { 7 };
                    c.r1 = 20 - c.r0;
                } else {
                    for k in 0..3 {
                        c.tabela[k] = b.ler(5) as usize;
                    }
                    c.r0 = b.ler(4) as usize;
                    c.r1 = b.ler(3) as usize;
                }
                if mpeg1 {
                    c.preflag = b.bit() == 1;
                }
                c.sf_escala = b.bit() == 1;
                c.c1 = b.bit() == 1;
            }
        }
        // dados principais: o que sobrou dos quadros anteriores + este
        let dados = &q[(ini_side + cab.tam_side()).min(q.len())..];
        let antes = self.reservatorio.len();
        self.reservatorio.extend_from_slice(dados);
        let pronto = comeco <= antes;
        let base = antes.saturating_sub(comeco);
        let spf = 576 * ngr;
        if self.primeiro.is_none() {
            self.primeiro = Some(cab);
        }
        let mut pcm = alloc::vec![[0f32; 1152]; 2];
        if pronto {
            let reservatorio = core::mem::take(&mut self.reservatorio);
            let mut bits = Bits { d: &reservatorio[base..], pos: 0 };
            for g in 0..ngr {
                let mut xr = [[0f32; 576]; 2];
                let mut nz = [0usize; 2];
                for ch in 0..nch {
                    let gi = gr[g][ch];
                    let ini = bits.pos;
                    let ant = self.fatores[ch];
                    let f = if mpeg1 {
                        self.fatores_mpeg1(&mut bits, &gi, g, &scfsi[ch], &ant)
                    } else {
                        let is_dir = ch == 1 && cab.modo == 1 && cab.modo_ext & 1 != 0;
                        let (f, preflag) = self.fatores_mpeg2(&mut bits, &gi, is_dir);
                        gr[g][ch].preflag = preflag;
                        f
                    };
                    self.fatores[ch] = f;
                    let mut ix = [0i32; 576];
                    nz[ch] = self.huffman(&mut bits, &gr[g][ch], ini + gi.p23, cab.indice_taxa, &mut ix);
                    bits.pos = ini + gi.p23;
                    self.requantizar(&gr[g][ch], &f, &ix, cab.indice_taxa, &mut xr[ch]);
                }
                if nch == 2 && cab.modo == 1 {
                    self.estereo(&cab, &gr[g], &mut xr, nz);
                }
                for ch in 0..nch {
                    let gi = gr[g][ch];
                    if gi.ws && gi.bt == 2 {
                        reordenar(&mut xr[ch], &SFB_CURTA[cab.indice_taxa], gi.misto);
                    }
                    antialias(&mut xr[ch], &gi);
                    let mut sub = [0f32; 576];
                    self.imdct(ch, &gi, &xr[ch], &mut sub);
                    // inversao de frequencia
                    for sb in (1..32).step_by(2) {
                        for i in (1..18).step_by(2) {
                            sub[sb * 18 + i] = -sub[sb * 18 + i];
                        }
                    }
                    let destino: &mut [f32] = &mut pcm[ch][g * 576..g * 576 + 576];
                    self.sintese(ch, &sub, destino);
                }
            }
            // guarda so o fim do reservatorio (o maior main_data_begin e 511)
            let mut r = reservatorio;
            let corte = r.len().saturating_sub(4096);
            r.drain(..corte);
            self.reservatorio = r;
        } else {
            let corte = self.reservatorio.len().saturating_sub(4096);
            self.reservatorio.drain(..corte);
        }
        self.saida.clear();
        self.lidos = 0;
        for i in 0..spf {
            let e = pcm[0][i];
            let d = if nch == 2 { pcm[1][i] } else { e };
            self.saida.push((e * 32767.0).clamp(-32768.0, 32767.0) as i16);
            self.saida.push((d * 32767.0).clamp(-32768.0, 32767.0) as i16);
        }
        self.taxa = cab.taxa;
        self.quadros += 1;
        true
    }

    fn fatores_mpeg1(&self, b: &mut Bits, g: &Granulo, ngr: usize, scfsi: &[bool; 4], ant: &Fatores) -> Fatores {
        let (s1, s2) = SLEN[g.sfc as usize & 15];
        let mut f = Fatores::default();
        if g.ws && g.bt == 2 {
            let mut sfb0 = 0;
            if g.misto {
                for sfb in 0..8 {
                    f.l[sfb] = b.ler(s1) as i32;
                }
                sfb0 = 3;
            }
            for sfb in sfb0..12 {
                for w in 0..3 {
                    f.s[sfb][w] = b.ler(if sfb < 6 { s1 } else { s2 }) as i32;
                }
            }
        } else {
            const PARTES: [(usize, usize); 4] = [(0, 6), (6, 11), (11, 16), (16, 21)];
            for (k, &(a, z)) in PARTES.iter().enumerate() {
                for sfb in a..z {
                    f.l[sfb] = if ngr == 1 && scfsi[k] { ant.l[sfb] } else { b.ler(if k < 2 { s1 } else { s2 }) as i32 };
                }
            }
        }
        f
    }

    fn fatores_mpeg2(&self, b: &mut Bits, g: &Granulo, is_direito: bool) -> (Fatores, bool) {
        let mut sfc = g.sfc;
        let (slen, tabela, preflag): ([u32; 4], usize, bool) = if !is_direito {
            if sfc < 400 {
                ([(sfc >> 4) / 5, (sfc >> 4) % 5, (sfc & 15) >> 2, sfc & 3], 0, false)
            } else if sfc < 500 {
                sfc -= 400;
                ([(sfc >> 2) / 5, (sfc >> 2) % 5, sfc & 3, 0], 1, false)
            } else {
                sfc -= 500;
                ([sfc / 3, sfc % 3, 0, 0], 2, true)
            }
        } else {
            let mut i = sfc >> 1;
            if i < 180 {
                ([i / 36, (i % 36) / 6, (i % 36) % 6, 0], 3, false)
            } else if i < 244 {
                i -= 180;
                ([(i & 63) >> 4, (i & 15) >> 2, i & 3, 0], 4, false)
            } else {
                i -= 244;
                ([i / 3, i % 3, 0, 0], 5, false)
            }
        };
        let bloco = if g.ws && g.bt == 2 { if g.misto { 2 } else { 1 } } else { 0 };
        let mut valores = Vec::with_capacity(45);
        for k in 0..4 {
            for _ in 0..NR_SFB[tabela][bloco][k] {
                valores.push(b.ler(slen[k]) as i32);
            }
        }
        let mut f = Fatores::default();
        let mut it = valores.into_iter();
        match bloco {
            0 => {
                for sfb in 0..21 {
                    f.l[sfb] = it.next().unwrap_or(0);
                }
            }
            1 => {
                for sfb in 0..12 {
                    for w in 0..3 {
                        f.s[sfb][w] = it.next().unwrap_or(0);
                    }
                }
            }
            _ => {
                for sfb in 0..6 {
                    f.l[sfb] = it.next().unwrap_or(0);
                }
                for sfb in 3..12 {
                    for w in 0..3 {
                        f.s[sfb][w] = it.next().unwrap_or(0);
                    }
                }
            }
        }
        (f, preflag)
    }

    /// Le os valores quantizados; devolve ate onde ha valores (os de cima sao zero).
    fn huffman(&self, b: &mut Bits, g: &Granulo, fim: usize, it: usize, ix: &mut [i32; 576]) -> usize {
        let longa = &SFB_LONGA[it];
        let big = (g.big * 2).min(576);
        let (r1, r2) = if g.ws {
            (if g.bt == 2 { SFB_CURTA[it][3] as usize * 3 } else { longa[8] as usize }, 576)
        } else {
            (longa[(g.r0 + 1).min(22)] as usize, longa[(g.r0 + g.r1 + 2).min(22)] as usize)
        };
        let mut i = 0;
        while i < big {
            let reg = if i < r1 { 0 } else if i < r2 { 1 } else { 2 };
            let tab = g.tabela[reg];
            let (mut x, mut y) = (0i32, 0i32);
            if tab != 0 {
                if let (Some(arv), Some((dim, _, _))) = (self.arvores.get(tab).and_then(|a| a.as_ref()), t::tabela(tab)) {
                    let v = arv.decodificar(b);
                    x = (v / dim) as i32;
                    y = (v % dim) as i32;
                    let lin = t::LINBITS[tab] as u32;
                    if lin > 0 && x == 15 {
                        x += b.ler(lin) as i32;
                    }
                    if x != 0 && b.bit() == 1 {
                        x = -x;
                    }
                    if lin > 0 && y == 15 {
                        y += b.ler(lin) as i32;
                    }
                    if y != 0 && b.bit() == 1 {
                        y = -y;
                    }
                }
            }
            ix[i] = x;
            ix[i + 1] = y;
            i += 2;
        }
        // regiao count1: quadras de -1, 0, 1
        let arv = self.arvores[if g.c1 { 33 } else { 32 }].as_ref();
        while i + 4 <= 576 && b.pos < fim {
            let Some(arv) = arv else { break };
            let v = arv.decodificar(b);
            let mut q = [((v >> 3) & 1) as i32, ((v >> 2) & 1) as i32, ((v >> 1) & 1) as i32, (v & 1) as i32];
            for x in q.iter_mut() {
                if *x != 0 && b.bit() == 1 {
                    *x = -1;
                }
            }
            if b.pos > fim {
                break; // a ultima quadra passou do fim: descarta
            }
            ix[i..i + 4].copy_from_slice(&q);
            i += 4;
        }
        i
    }

    fn requantizar(&self, g: &Granulo, f: &Fatores, ix: &[i32; 576], it: usize, xr: &mut [f32; 576]) {
        let mult = if g.sf_escala { 1.0 } else { 0.5 };
        let base = 0.25 * (g.ganho - 210) as f64;
        let valor = |q: i32, fator: f32| -> f32 {
            let a = q.unsigned_abs() as usize;
            let v = if a < self.pow43.len() { self.pow43[a] } else { mat64::pow(a as f64, 4.0 / 3.0) as f32 } * fator;
            if q < 0 { -v } else { v }
        };
        let pot2 = |e: f64| mat64::pow(2.0, e) as f32;
        let longa = &SFB_LONGA[it];
        let curta = &SFB_CURTA[it];
        let pf = if g.preflag { 1 } else { 0 };
        let curto = g.ws && g.bt == 2;
        // parte longa: tudo, ou os primeiros 36 de um bloco misto
        let fim_longa = if !curto { 576 } else if g.misto { 36 } else { 0 };
        let mut sfb = 0;
        while sfb < 22 && (longa[sfb] as usize) < fim_longa {
            let fator = pot2(base - mult * (f.l[sfb] + pf * PRETAB[sfb]) as f64);
            for i in longa[sfb] as usize..(longa[sfb + 1] as usize).min(fim_longa) {
                xr[i] = valor(ix[i], fator);
            }
            sfb += 1;
        }
        if curto {
            let sfb0 = if g.misto { 3 } else { 0 };
            for sfb in sfb0..13 {
                let largura = (curta[sfb + 1] - curta[sfb]) as usize;
                for w in 0..3 {
                    let sf = if sfb < 12 { f.s[sfb][w] } else { 0 };
                    let fator = pot2(base - 2.0 * g.subganho[w] as f64 - mult * sf as f64);
                    let ini = curta[sfb] as usize * 3 + w * largura;
                    for i in ini..ini + largura {
                        xr[i] = valor(ix[i], fator);
                    }
                }
            }
        }
    }

    /// Estereo conjunto: M/S e intensidade (MPEG-1).
    fn estereo(&self, cab: &Cabecalho, g: &[Granulo; 2], xr: &mut [[f32; 576]; 2], nz: [usize; 2]) {
        let ms = cab.modo_ext & 2 != 0;
        let intensidade = cab.modo_ext & 1 != 0 && cab.versao == 0;
        let mut limite_ms = 576;
        if intensidade {
            // bandas (longas) acima do ultimo valor do canal direito
            let g1 = &g[1];
            let longa = &SFB_LONGA[cab.indice_taxa];
            let curto = g1.ws && g1.bt == 2;
            if !curto {
                let mut sfb = 0;
                while sfb < 21 && (longa[sfb + 1] as usize) <= nz[1] {
                    sfb += 1;
                }
                if nz[1] > 0 && (longa[sfb] as usize) < nz[1] {
                    sfb += 1;
                }
                limite_ms = longa[sfb] as usize;
                let f = &self.fatores[1];
                for b in sfb..22 {
                    let pos = if b < 21 { f.l[b] } else { f.l[20] };
                    let (a, z) = (longa[b] as usize, longa[b + 1] as usize);
                    if pos >= 7 {
                        // posicao ilegal: fica M/S (ou normal)
                        if ms {
                            for i in a..z {
                                let (m, s) = (xr[0][i], xr[1][i]);
                                xr[0][i] = (m + s) * core::f32::consts::FRAC_1_SQRT_2;
                                xr[1][i] = (m - s) * core::f32::consts::FRAC_1_SQRT_2;
                            }
                        }
                        continue;
                    }
                    let (kl, kr) = razao_intensidade(pos);
                    for i in a..z {
                        let v = xr[0][i];
                        xr[0][i] = v * kl;
                        xr[1][i] = v * kr;
                    }
                }
            }
        }
        if ms {
            for i in 0..limite_ms {
                let (m, s) = (xr[0][i], xr[1][i]);
                xr[0][i] = (m + s) * core::f32::consts::FRAC_1_SQRT_2;
                xr[1][i] = (m - s) * core::f32::consts::FRAC_1_SQRT_2;
            }
        }
    }

    fn imdct(&mut self, ch: usize, g: &Granulo, xr: &[f32; 576], saida: &mut [f32; 576]) {
        for sb in 0..32 {
            let bt = if g.ws && g.misto && sb < 2 { 0 } else if g.ws { g.bt as usize } else { 0 };
            let x = &xr[sb * 18..sb * 18 + 18];
            let mut z = [0f32; 36];
            if bt == 2 {
                for w in 0..3 {
                    for i in 0..12 {
                        let mut s = 0.0;
                        for k in 0..6 {
                            s += x[w + 3 * k] * self.cos_curta[i * 6 + k];
                        }
                        z[6 + 6 * w + i] += s * self.janelas[2][i];
                    }
                }
            } else {
                for i in 0..36 {
                    let mut s = 0.0;
                    for k in 0..18 {
                        s += x[k] * self.cos_longa[i * 18 + k];
                    }
                    z[i] = s * self.janelas[bt][i];
                }
            }
            let sobra = &mut self.sobra[ch][sb * 18..sb * 18 + 18];
            for i in 0..18 {
                saida[sb * 18 + i] = z[i] + sobra[i];
                sobra[i] = z[i + 18];
            }
        }
    }

    fn sintese(&mut self, ch: usize, sub: &[f32; 576], saida: &mut [f32]) {
        let v = &mut self.v[ch];
        for tempo in 0..18 {
            self.v_pos[ch] = (self.v_pos[ch] + 1024 - 64) & 1023;
            let off = self.v_pos[ch];
            let mut s = [0f32; 32];
            for (sb, x) in s.iter_mut().enumerate() {
                *x = sub[sb * 18 + tempo];
            }
            for i in 0..64 {
                let n = &self.sintese_n[i * 32..i * 32 + 32];
                let mut acc = 0.0;
                for k in 0..32 {
                    acc += n[k] * s[k];
                }
                v[(off + i) & 1023] = acc;
            }
            for j in 0..32 {
                let mut acc = 0.0;
                for i in 0..16 {
                    let vi = 128 * (i / 2) + j + if i % 2 == 1 { 96 } else { 0 };
                    acc += t::JANELA[j + 32 * i] * v[(off + vi) & 1023];
                }
                saida[tempo * 32 + j] = acc;
            }
        }
    }
}

fn razao_intensidade(pos: i32) -> (f32, f32) {
    if pos == 6 {
        return (1.0, 0.0);
    }
    let a = pos as f64 * core::f64::consts::PI / 12.0;
    let r = mat64::sin(a) / mat64::cos(a);
    ((r / (1.0 + r)) as f32, (1.0 / (1.0 + r)) as f32)
}

/// Blocos curtos: de banda/janela/linha para linha intercalada por janela.
fn reordenar(xr: &mut [f32; 576], curta: &[u16; 14], misto: bool) {
    let mut tmp = [0f32; 576];
    let sfb0 = if misto { 3 } else { 0 };
    let ini = curta[sfb0] as usize * 3;
    for sfb in sfb0..13 {
        let a = curta[sfb] as usize;
        let largura = (curta[sfb + 1] - curta[sfb]) as usize;
        for w in 0..3 {
            for k in 0..largura {
                tmp[a * 3 + k * 3 + w] = xr[a * 3 + w * largura + k];
            }
        }
    }
    xr[ini..].copy_from_slice(&tmp[ini..]);
}

fn antialias(xr: &mut [f32; 576], g: &Granulo) {
    if g.ws && g.bt == 2 && !g.misto {
        return;
    }
    let ate = if g.ws && g.bt == 2 && g.misto { 2 } else { 32 };
    for sb in 1..ate {
        for i in 0..8 {
            let c = C_ALIAS[i];
            let raiz = mat64::sqrt(1.0 + c * c);
            let (cs, ca) = ((1.0 / raiz) as f32, (c / raiz) as f32);
            let a = sb * 18 - 1 - i;
            let b = sb * 18 + i;
            let (x, y) = (xr[a], xr[b]);
            xr[a] = x * cs - y * ca;
            xr[b] = y * cs + x * ca;
        }
    }
}

impl Audio for Mp3 {
    fn taxa(&self) -> u32 {
        self.taxa
    }

    fn ler(&mut self, saida: &mut [i16]) -> usize {
        let mut k = 0;
        while k < saida.len() {
            if self.lidos >= self.saida.len() && !self.quadro() {
                break;
            }
            let n = (saida.len() - k).min(self.saida.len() - self.lidos);
            saida[k..k + n].copy_from_slice(&self.saida[self.lidos..self.lidos + n]);
            self.lidos += n;
            k += n;
        }
        self.amostras += (k / 2) as u64;
        k
    }

    fn duracao_ms(&self) -> Option<u64> {
        self.duracao
    }

    fn posicao_ms(&self) -> u64 {
        self.amostras * 1000 / self.taxa.max(1) as u64
    }

    fn descricao(&self) -> String {
        let (v, modo) = match &self.primeiro {
            Some(c) => (["1", "2", "2.5"][c.versao], ["estéreo", "estéreo conjunto", "dois canais", "mono"][c.modo as usize]),
            None => ("?", "?"),
        };
        format!("MP3 (MPEG-{v} Layer III), {} kbps, {} Hz, {modo}", self.bitrate, self.taxa)
    }

    fn ir_para(&mut self, ms: u64) -> bool {
        // posicao aproximada pelo bitrate; o reservatorio recomeca vazio
        let alvo = self.inicio + (ms * self.bitrate as u64 / 8) as usize;
        self.pos = alvo.min(self.tamanho);
        self.reservatorio.clear();
        self.saida.clear();
        self.lidos = 0;
        self.amostras = ms * self.taxa as u64 / 1000;
        true
    }
}
