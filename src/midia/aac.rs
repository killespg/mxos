//! AAC-LC (ISO/IEC 14496-3): o som do YouTube (formato 140), de MP4 e de
//! celulares. Feito aqui do zero; as tabelas da norma estao em
//! aac_tabelas.rs.
//!
//! Caminho de um quadro (raw_data_block): elementos SCE/CPE/LFE ->
//! ics_info, secoes, fatores de escala (Huffman), espectro (11 livros
//! Huffman, escape no 11) e pulsos -> requantizacao |q|^(4/3) * 2^(sf/4)
//! -> ruido (PNS), M/S e intensidade -> TNS -> IMDCT (FFT de N/4) com
//! janelas seno/KBD e sobreposicao -> PCM de 16 bits.
//!
//! So core + alloc: o mesmo codigo roda nos testes do PC.

use alloc::vec;
use alloc::vec::Vec;

use super::aac_tabelas as t;
use crate::web::js::mat64;

type R<T> = Result<T, &'static str>;

const SO_LONGA: u8 = 0;
const INICIO_LONGA: u8 = 1;
const OITO_CURTAS: u8 = 2;
const FIM_LONGA: u8 = 3;

const LIVRO_RUIDO: u8 = 13;
const LIVRO_INTENSIDADE2: u8 = 14;
const LIVRO_INTENSIDADE: u8 = 15;

// ------------------------------------------------------------- bits

struct Bits<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    fn bit(&mut self) -> R<u32> {
        let byte = *self.b.get(self.pos >> 3).ok_or("quadro AAC acabou")?;
        let v = (byte >> (7 - (self.pos & 7))) & 1;
        self.pos += 1;
        Ok(v as u32)
    }
    fn u(&mut self, n: u32) -> R<u32> {
        let mut v = 0;
        for _ in 0..n {
            v = v << 1 | self.bit()?;
        }
        Ok(v)
    }
    fn alinhar(&mut self) {
        self.pos = (self.pos + 7) & !7;
    }
}

/// Um dicionario de Huffman: para cada tamanho, (codigo, indice) em ordem.
struct Huffman {
    por_tamanho: Vec<Vec<(u32, u16)>>,
}

impl Huffman {
    fn novo(codigos: &[u32], bits: &[u8]) -> Huffman {
        let max = *bits.iter().max().unwrap_or(&1) as usize;
        let mut por_tamanho = vec![Vec::new(); max + 1];
        for (i, (&c, &b)) in codigos.iter().zip(bits).enumerate() {
            por_tamanho[b as usize].push((c, i as u16));
        }
        for l in por_tamanho.iter_mut() {
            l.sort_unstable();
        }
        Huffman { por_tamanho }
    }

    fn ler(&self, b: &mut Bits) -> R<usize> {
        let mut codigo = 0u32;
        for tam in 1..self.por_tamanho.len() {
            codigo = codigo << 1 | b.bit()?;
            let l = &self.por_tamanho[tam];
            if let Ok(k) = l.binary_search_by_key(&codigo, |e| e.0) {
                return Ok(l[k].1 as usize);
            }
        }
        Err("codigo de Huffman invalido no AAC")
    }
}

// -------------------------------------------------------- IMDCT (FFT)

/// IMDCT de tamanho `n` (2048 ou 256) com uma FFT complexa de n/4 pontos.
struct Imdct {
    n: usize,
    /// cos/sen da rotacao antes e depois da FFT.
    rot: Vec<(f32, f32)>,
    /// Fatores da FFT e a permutacao de bits.
    fft_w: Vec<(f32, f32)>,
    inverte: Vec<u16>,
}

impl Imdct {
    fn novo(n: usize) -> Imdct {
        let n4 = n / 4;
        let pi = core::f64::consts::PI;
        let rot = (0..n4)
            .map(|k| {
                let a = pi * (k as f64 + 0.125) / (n / 2) as f64;
                (mat64::cos(a) as f32, mat64::sin(a) as f32)
            })
            .collect();
        let fft_w = (0..n4 / 2)
            .map(|k| {
                let a = -2.0 * pi * k as f64 / n4 as f64;
                (mat64::cos(a) as f32, mat64::sin(a) as f32)
            })
            .collect();
        let bits = n4.trailing_zeros();
        let inverte = (0..n4).map(|i| ((i as u32).reverse_bits() >> (32 - bits)) as u16).collect();
        Imdct { n, rot, fft_w, inverte }
    }

    fn fft(&self, re: &mut [f32], im: &mut [f32]) {
        let n = re.len();
        for i in 0..n {
            let j = self.inverte[i] as usize;
            if i < j {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut tam = 2;
        while tam <= n {
            let passo = n / tam;
            for ini in (0..n).step_by(tam) {
                for k in 0..tam / 2 {
                    let (c, s) = self.fft_w[k * passo];
                    let (a, b) = (ini + k, ini + k + tam / 2);
                    let tr = re[b] * c - im[b] * s;
                    let ti = re[b] * s + im[b] * c;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            tam *= 2;
        }
    }

    /// `x`: n/2 coeficientes -> `y`: n amostras, ja com o fator 2/n da norma:
    /// y[i] = 2/n * soma x[k] cos(2pi/n (i + n0)(k + 1/2)), n0 = (n/2 + 1)/2.
    ///
    /// Pela DCT-IV de m = n/2 pontos, u[j] = soma x[k] cos(pi/m (j + 1/2)(k + 1/2))
    /// (feita com a FFT de m/2), e as simetrias do cosseno:
    ///   y[i] = u[i + m/2]           (i < m/2)
    ///   y[i] = -u[3m/2 - 1 - i]     (m/2 <= i < 3m/2)
    ///   y[i] = -u[i - 3m/2]         (i >= 3m/2)
    fn calcular(&self, x: &[f32], y: &mut [f32]) {
        let (n, m) = (self.n, self.n / 2);
        let m2 = m / 2;
        let mut re = vec![0f32; m2];
        let mut im = vec![0f32; m2];
        // (x[2k] + j x[m-1-2k]) * e^(-j pi (k + 1/8) / m), e o mesmo giro depois
        for k in 0..m2 {
            let (a, b) = (x[2 * k], x[m - 1 - 2 * k]);
            let (c, s) = self.rot[k];
            re[k] = a * c + b * s;
            im[k] = b * c - a * s;
        }
        self.fft(&mut re, &mut im);
        let mut u = vec![0f32; m];
        let escala = 2.0 / n as f32;
        for j in 0..m2 {
            let (c, s) = self.rot[j];
            let (r, i) = (re[j], im[j]);
            u[2 * j] = (r * c + i * s) * escala;
            u[m - 1 - 2 * j] = -(i * c - r * s) * escala;
        }
        let meio = m / 2;
        for i in 0..meio {
            y[i] = u[i + meio];
        }
        for i in meio..3 * meio {
            y[i] = -u[3 * meio - 1 - i];
        }
        for i in 3 * meio..n {
            y[i] = -u[i - 3 * meio];
        }
    }
}

// ----------------------------------------------------------- janelas

/// Metade que sobe de uma janela de tamanho 2*n.
fn janela_seno(n: usize) -> Vec<f32> {
    (0..n).map(|i| mat64::sin(core::f64::consts::PI * (i as f64 + 0.5) / (2 * n) as f64) as f32).collect()
}

fn janela_kbd(n: usize, alfa: f64) -> Vec<f32> {
    let a2 = 4.0 * (alfa * core::f64::consts::PI / n as f64) * (alfa * core::f64::consts::PI / n as f64);
    let mut soma = 0.0;
    let mut parcial = vec![0f64; n];
    for i in 0..n {
        let tmp = (i * (n - i)) as f64 * a2;
        let mut bessel = 1.0;
        for j in (1..=50).rev() {
            bessel = bessel * tmp / (j * j) as f64 + 1.0;
        }
        soma += bessel;
        parcial[i] = soma;
    }
    soma += 1.0;
    parcial.iter().map(|&p| mat64::sqrt(p / soma) as f32).collect()
}

// ------------------------------------------------------- configuracao

/// AudioSpecificConfig: (tipo de objeto, indice de taxa, canais).
pub fn config(asc: &[u8]) -> R<(u32, usize, usize)> {
    let mut b = Bits { b: asc, pos: 0 };
    let mut tipo = b.u(5)?;
    if tipo == 31 {
        tipo = 32 + b.u(6)?;
    }
    let idx = b.u(4)? as usize;
    if idx == 15 {
        return Err("taxa explicita no AAC nao suportada");
    }
    let canais = b.u(4)? as usize;
    Ok((tipo, idx, canais))
}

/// O AudioSpecificConfig de dentro de uma caixa esds do MP4.
pub fn asc_do_esds(esds: &[u8]) -> Option<&[u8]> {
    // versao/flags, depois descritores (tag, tamanho em 7 bits por byte)
    let mut i = 4;
    while i < esds.len() {
        let tag = esds[i];
        i += 1;
        let mut tam = 0usize;
        for _ in 0..4 {
            let c = *esds.get(i)?;
            i += 1;
            tam = tam << 7 | (c & 0x7F) as usize;
            if c & 0x80 == 0 {
                break;
            }
        }
        match tag {
            0x03 => {
                // ES_ID(2) e flags(1) (+ campos opcionais)
                let flags = *esds.get(i + 2)?;
                i += 3;
                if flags & 0x80 != 0 {
                    i += 2;
                }
                if flags & 0x40 != 0 {
                    i += 1 + *esds.get(i)? as usize;
                }
                if flags & 0x20 != 0 {
                    i += 2;
                }
            }
            0x04 => i += 13,
            0x05 => return esds.get(i..i + tam),
            _ => i += tam,
        }
    }
    None
}

// -------------------------------------------------------- canal (ICS)

#[derive(Clone)]
struct Info {
    sequencia: u8,
    forma: u8,
    max_sfb: usize,
    /// Tamanho de cada grupo de janelas curtas (1 grupo de 1 nas longas).
    grupos: Vec<usize>,
}

impl Info {
    fn curta(&self) -> bool {
        self.sequencia == OITO_CURTAS
    }
}

struct Canal {
    info: Info,
    livros: Vec<u8>,
    fatores: Vec<i32>,
    espectro: [f32; 1024],
}

/// TNS de um canal: por janela, os filtros (inicio da faixa de cima,
/// tamanho, direcao, coeficientes LPC).
struct Tns {
    filtros: Vec<(usize, usize, usize, bool, Vec<f32>)>,
}

const TNS_MAPA: [&[f32]; 4] = [
    &[0.0, -0.43388373, -0.78183150, -0.97492790, 0.98480773, 0.86602539, 0.64278758, 0.34202015],
    &[0.0, -0.20791170, -0.40673664, -0.58778524, -0.74314481, -0.86602539, -0.95105654, -0.99452192, 0.99573416, 0.96182561, 0.89516330, 0.79801720, 0.67369562, 0.52643216, 0.36124167, 0.18374951],
    &[0.0, -0.43388373, 0.64278758, 0.34202015],
    &[0.0, -0.20791170, -0.40673664, -0.58778524, 0.67369562, 0.52643216, 0.36124167, 0.18374951],
];

pub struct Aac {
    pub taxa: u32,
    idx: usize,
    pub canais: usize,
    sf: Huffman,
    livros: Vec<Huffman>,
    pow43: Vec<f32>,
    imdct_longa: Imdct,
    imdct_curta: Imdct,
    /// [forma 0 = seno, 1 = KBD]: metades que sobem.
    longas: [Vec<f32>; 2],
    curtas: [Vec<f32>; 2],
    /// Sobreposicao de cada canal e a forma da janela anterior.
    sobra: [[f32; 1024]; 2],
    forma_ant: [u8; 2],
    semente: u32,
}

impl Aac {
    /// Com o AudioSpecificConfig (do esds do MP4).
    pub fn novo(asc: &[u8]) -> R<Aac> {
        let (tipo, idx, canais) = config(asc)?;
        if tipo != 2 {
            return Err("so AAC-LC (objeto 2)");
        }
        if idx >= 13 || !(1..=2).contains(&canais) {
            return Err("AAC com taxa ou canais nao suportados");
        }
        let livros = t::LIVROS.iter().map(|(c, b)| Huffman::novo(&c.iter().map(|&x| x as u32).collect::<Vec<_>>(), b)).collect();
        let pow43 = (0..8192).map(|i| mat64::pow(i as f64, 4.0 / 3.0) as f32).collect();
        Ok(Aac {
            taxa: t::TAXAS[idx],
            idx,
            canais,
            sf: Huffman::novo(&t::FE_CODIGOS, &t::FE_BITS),
            livros,
            pow43,
            imdct_longa: Imdct::novo(2048),
            imdct_curta: Imdct::novo(256),
            longas: [janela_seno(1024), janela_kbd(1024, 4.0)],
            curtas: [janela_seno(128), janela_kbd(128, 6.0)],
            sobra: [[0.0; 1024]; 2],
            forma_ant: [0; 2],
            semente: 0x1F2E_3D4C,
        })
    }

    /// Volta ao comeco (pulo no audio): sem o que sobrou do quadro anterior.
    pub fn recomecar(&mut self) {
        self.sobra = [[0.0; 1024]; 2];
    }

    /// Um quadro (raw_data_block) -> 1024 quadros estereo em `saida`.
    pub fn decodificar(&mut self, quadro: &[u8], saida: &mut Vec<i16>) -> R<()> {
        let mut b = Bits { b: quadro, pos: 0 };
        let mut canais: Vec<Canal> = Vec::new();
        loop {
            let id = b.u(3)?;
            match id {
                0 | 3 => {
                    b.u(4)?; // tag
                    let c = self.ics(&mut b, None)?;
                    canais.push(c);
                }
                1 => {
                    b.u(4)?;
                    let comum = b.bit()? == 1;
                    let mut info = None;
                    let mut ms: Vec<bool> = Vec::new();
                    let mut ms_tudo = false;
                    if comum {
                        let i = self.ics_info(&mut b)?;
                        let presente = b.u(2)?;
                        match presente {
                            1 => {
                                for _ in 0..i.grupos.len() * i.max_sfb {
                                    ms.push(b.bit()? == 1);
                                }
                            }
                            2 => ms_tudo = true,
                            _ => {}
                        }
                        info = Some(i);
                    }
                    let mut e = self.ics(&mut b, info.clone())?;
                    let mut d = self.ics(&mut b, info)?;
                    self.estereo(&mut e, &mut d, comum, &ms, ms_tudo);
                    canais.push(e);
                    canais.push(d);
                }
                2 => return Err("AAC com CCE nao suportado"),
                4 => {
                    b.u(4)?;
                    let alinhar = b.bit()? == 1;
                    let mut n = b.u(8)?;
                    if n == 255 {
                        n += b.u(8)?;
                    }
                    if alinhar {
                        b.alinhar();
                    }
                    b.pos += 8 * n as usize;
                }
                5 => return Err("AAC com PCE no quadro nao suportado"),
                6 => {
                    let mut n = b.u(4)?;
                    if n == 15 {
                        n += b.u(8)? - 1;
                    }
                    b.pos += 8 * n as usize;
                }
                _ => break,
            }
        }
        if canais.is_empty() {
            return Err("quadro AAC sem canais");
        }
        // banco de filtros de cada canal e saida estereo
        let mut pcm = [[0f32; 1024]; 2];
        for (k, c) in canais.iter().take(2).enumerate() {
            self.filtros(k, c, &mut pcm[k]);
        }
        let dois = canais.len() >= 2;
        saida.reserve(2048);
        for i in 0..1024 {
            let e = pcm[0][i];
            let d = if dois { pcm[1][i] } else { e };
            saida.push(e.clamp(-32768.0, 32767.0) as i16);
            saida.push(d.clamp(-32768.0, 32767.0) as i16);
        }
        Ok(())
    }

    fn ics_info(&self, b: &mut Bits) -> R<Info> {
        b.bit()?; // reservado
        let sequencia = b.u(2)? as u8;
        let forma = b.u(1)? as u8;
        let (max_sfb, grupos) = if sequencia == OITO_CURTAS {
            let max = b.u(4)? as usize;
            let agrup = b.u(7)?;
            let mut g = vec![1usize];
            for i in 0..7 {
                if agrup >> (6 - i) & 1 == 1 {
                    *g.last_mut().unwrap() += 1;
                } else {
                    g.push(1);
                }
            }
            (max, g)
        } else {
            let max = b.u(6)? as usize;
            if b.bit()? == 1 {
                return Err("predicao do AAC Main nao suportada");
            }
            (max, vec![1])
        };
        let limite = if sequencia == OITO_CURTAS { t::NUM_FAIXAS_CURTAS[self.idx] } else { t::NUM_FAIXAS_LONGAS[self.idx] } as usize;
        if max_sfb > limite {
            return Err("max_sfb alem das faixas");
        }
        Ok(Info { sequencia, forma, max_sfb, grupos })
    }

    fn faixas(&self, curta: bool) -> &'static [u16] {
        if curta { t::FAIXAS_CURTAS[self.idx] } else { t::FAIXAS_LONGAS[self.idx] }
    }

    /// individual_channel_stream. `info` = a comum do CPE.
    fn ics(&mut self, b: &mut Bits, info: Option<Info>) -> R<Canal> {
        let ganho = b.u(8)? as i32;
        let info = match info {
            Some(i) => i,
            None => self.ics_info(b)?,
        };
        let curta = info.curta();
        let ngrupos = info.grupos.len();
        let max = info.max_sfb;
        // secoes: o livro de cada faixa
        let mut livros = vec![0u8; ngrupos * max];
        let (bits_sec, esc) = if curta { (3, 7) } else { (5, 31) };
        for g in 0..ngrupos {
            let mut k = 0;
            while k < max {
                let livro = b.u(4)? as u8;
                if livro == 12 {
                    return Err("livro 12 reservado");
                }
                let mut tam = 0;
                loop {
                    let inc = b.u(bits_sec)? as usize;
                    tam += inc;
                    if inc != esc {
                        break;
                    }
                }
                if k + tam > max {
                    return Err("secao alem das faixas");
                }
                for x in &mut livros[g * max + k..g * max + k + tam] {
                    *x = livro;
                }
                k += tam;
            }
        }
        // fatores de escala
        let mut fatores = vec![0i32; ngrupos * max];
        let (mut global, mut ruido, mut intensidade) = (ganho, ganho - 90, 0i32);
        let mut primeiro_ruido = true;
        for (i, &livro) in livros.iter().enumerate() {
            fatores[i] = match livro {
                0 => 0,
                LIVRO_INTENSIDADE | LIVRO_INTENSIDADE2 => {
                    intensidade += self.sf.ler(b)? as i32 - 60;
                    intensidade
                }
                LIVRO_RUIDO => {
                    if primeiro_ruido {
                        primeiro_ruido = false;
                        ruido += b.u(9)? as i32 - 256;
                    } else {
                        ruido += self.sf.ler(b)? as i32 - 60;
                    }
                    ruido
                }
                _ => {
                    global += self.sf.ler(b)? as i32 - 60;
                    if !(0..=255).contains(&global) {
                        return Err("fator de escala fora da faixa");
                    }
                    global
                }
            };
        }
        // pulsos (so janela longa)
        let mut pulsos: Vec<(usize, i32)> = Vec::new();
        if b.bit()? == 1 {
            if curta {
                return Err("pulsos em janela curta");
            }
            let n = b.u(2)? + 1;
            let inicio = b.u(6)? as usize;
            let faixas = self.faixas(false);
            let mut pos = *faixas.get(inicio).ok_or("pulso fora")? as usize;
            for _ in 0..n {
                pos += b.u(5)? as usize;
                let amp = b.u(4)? as i32;
                pulsos.push((pos, amp));
            }
        }
        // TNS
        let tns = if b.bit()? == 1 { Some(self.ler_tns(b, &info)?) } else { None };
        if b.bit()? == 1 {
            return Err("controle de ganho (AAC SSR) nao suportado");
        }
        // espectro
        let mut q = [0i32; 1024];
        let faixas = self.faixas(curta);
        let mut base = 0usize;
        for g in 0..ngrupos {
            let tam_g = info.grupos[g];
            for sfb in 0..max {
                let livro = livros[g * max + sfb];
                let (ini, fim) = (faixas[sfb] as usize, faixas[sfb + 1] as usize);
                if livro == 0 || livro >= LIVRO_RUIDO {
                    continue;
                }
                for w in 0..tam_g {
                    let o = base + w * 128;
                    self.ler_espectro(b, livro, &mut q[o + ini..o + fim])?;
                }
            }
            base += tam_g * 128;
        }
        for (pos, amp) in pulsos {
            if pos < 1024 {
                q[pos] += if q[pos] < 0 { -amp } else { amp };
            }
        }
        // requantizacao (e ruido nas faixas de ruido)
        let mut espectro = [0f32; 1024];
        let mut base = 0usize;
        for g in 0..ngrupos {
            let tam_g = info.grupos[g];
            for sfb in 0..max {
                let i = g * max + sfb;
                let (ini, fim) = (faixas[sfb] as usize, faixas[sfb + 1] as usize);
                for w in 0..tam_g {
                    let o = base + w * 128;
                    match livros[i] {
                        0 | LIVRO_INTENSIDADE | LIVRO_INTENSIDADE2 => {}
                        LIVRO_RUIDO => {
                            let mut energia = 0f32;
                            for x in &mut espectro[o + ini..o + fim] {
                                self.semente = self.semente.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                                *x = (self.semente as i32) as f32;
                                energia += *x * *x;
                            }
                            let escala = ganho_de(fatores[i]) / mat64::sqrt(energia.max(1e-30) as f64) as f32;
                            for x in &mut espectro[o + ini..o + fim] {
                                *x *= escala;
                            }
                        }
                        _ => {
                            let gs = ganho_de(fatores[i] - 100);
                            for k in o + ini..o + fim {
                                let v = q[k];
                                let a = v.unsigned_abs() as usize;
                                let m = if a < 8192 { self.pow43[a] } else { mat64::pow(a as f64, 4.0 / 3.0) as f32 };
                                espectro[k] = if v < 0 { -m * gs } else { m * gs };
                            }
                        }
                    }
                }
            }
            base += tam_g * 128;
        }
        if let Some(tns) = tns {
            aplicar_tns(&tns, &info, &mut espectro);
        }
        Ok(Canal { info, livros, fatores, espectro })
    }

    fn ler_espectro(&self, b: &mut Bits, livro: u8, saida: &mut [i32]) -> R<()> {
        let h = &self.livros[livro as usize - 1];
        let (dim, com_sinal, modulo) = match livro {
            1 | 2 => (4, true, 3),
            3 | 4 => (4, false, 3),
            5 | 6 => (2, true, 9),
            7 | 8 => (2, false, 8),
            9 | 10 => (2, false, 13),
            _ => (2, false, 17),
        };
        let mut k = 0;
        while k < saida.len() {
            let mut idx = h.ler(b)?;
            let mut v = [0i32; 4];
            for j in (0..dim).rev() {
                v[j] = (idx % modulo) as i32;
                idx /= modulo;
            }
            if com_sinal {
                let meio = (modulo / 2) as i32;
                for x in v.iter_mut().take(dim) {
                    *x -= meio;
                }
            } else {
                for x in v.iter_mut().take(dim) {
                    if *x != 0 && b.bit()? == 1 {
                        *x = -*x;
                    }
                }
            }
            if livro == 11 {
                for x in v.iter_mut().take(dim) {
                    if x.abs() == 16 {
                        let mut n = 0;
                        while b.bit()? == 1 {
                            n += 1;
                            if n > 8 {
                                return Err("escape do AAC longo demais");
                            }
                        }
                        let valor = (1i32 << (n + 4)) + b.u(n + 4)? as i32;
                        *x = if *x < 0 { -valor } else { valor };
                    }
                }
            }
            for j in 0..dim {
                if k + j < saida.len() {
                    saida[k + j] = v[j];
                }
            }
            k += dim;
        }
        Ok(())
    }

    fn ler_tns(&self, b: &mut Bits, info: &Info) -> R<Tns> {
        let curta = info.curta();
        let janelas = if curta { 8 } else { 1 };
        let (bits_n, bits_tam, bits_ordem) = if curta { (1, 4, 3) } else { (2, 6, 5) };
        let max_bandas = if curta { t::TNS_MAX_CURTA[self.idx] } else { t::TNS_MAX_LONGA[self.idx] } as usize;
        let faixas = self.faixas(curta);
        let limite = max_bandas.min(info.max_sfb);
        let mut filtros = Vec::new();
        for w in 0..janelas {
            let n = b.u(bits_n)?;
            if n == 0 {
                continue;
            }
            let res = b.bit()?;
            let mut topo = if curta { t::NUM_FAIXAS_CURTAS[self.idx] } else { t::NUM_FAIXAS_LONGAS[self.idx] } as usize;
            for _ in 0..n {
                let tam = b.u(bits_tam)? as usize;
                let ordem = b.u(bits_ordem)? as usize;
                let fundo = topo.saturating_sub(tam);
                if ordem > 0 {
                    let direcao = b.bit()? == 1;
                    let compr = b.bit()?;
                    let bits = res + 3 - compr;
                    let mapa = TNS_MAPA[(2 * compr + res) as usize];
                    let mut refl = Vec::with_capacity(ordem);
                    for _ in 0..ordem {
                        refl.push(*mapa.get(b.u(bits)? as usize).ok_or("coeficiente TNS")?);
                    }
                    // reflexao -> LPC (como o compute_lpc_coefs sem normalizar)
                    let mut lpc = vec![0f32; ordem];
                    for i in 0..ordem {
                        let r = -refl[i];
                        lpc[i] = r;
                        for j in 0..(i + 1) >> 1 {
                            let (f, bb) = (lpc[j], lpc[i - 1 - j]);
                            lpc[j] = f + r * bb;
                            lpc[i - 1 - j] = bb + r * f;
                        }
                    }
                    let ini = faixas[fundo.min(limite)] as usize;
                    let fim = faixas[topo.min(limite)] as usize;
                    if fim > ini {
                        filtros.push((w, ini, fim, direcao, lpc));
                    }
                }
                topo = fundo;
            }
        }
        Ok(Tns { filtros })
    }

    fn estereo(&self, e: &mut Canal, d: &mut Canal, comum: bool, ms: &[bool], ms_tudo: bool) {
        if !comum {
            return;
        }
        let info = e.info.clone();
        let curta = info.curta();
        let faixas = self.faixas(curta);
        let max = info.max_sfb;
        let mut base = 0usize;
        for (g, &tam_g) in info.grupos.iter().enumerate() {
            for sfb in 0..max {
                let i = g * max + sfb;
                let usa_ms = ms_tudo || ms.get(i).copied().unwrap_or(false);
                let (ini, fim) = (faixas[sfb] as usize, faixas[sfb + 1] as usize);
                let (le, ld) = (e.livros[i], d.livros[i]);
                for w in 0..tam_g {
                    let o = base + w * 128;
                    if ld == LIVRO_INTENSIDADE || ld == LIVRO_INTENSIDADE2 {
                        // direita = esquerda * 2^(-pos/4), com o sinal do livro (e do M/S)
                        let mut sinal = if ld == LIVRO_INTENSIDADE { 1.0 } else { -1.0 };
                        if usa_ms {
                            sinal = -sinal;
                        }
                        let escala = sinal * ganho_de(-d.fatores[i]);
                        for k in o + ini..o + fim {
                            d.espectro[k] = e.espectro[k] * escala;
                        }
                    } else if usa_ms && le < LIVRO_RUIDO && ld < LIVRO_RUIDO {
                        for k in o + ini..o + fim {
                            let (m, s) = (e.espectro[k], d.espectro[k]);
                            e.espectro[k] = m + s;
                            d.espectro[k] = m - s;
                        }
                    } else if usa_ms && le == LIVRO_RUIDO && ld == LIVRO_RUIDO {
                        // ruido correlacionado: o mesmo dos dois lados
                        let n = (fim - ini) as f32;
                        let ed: f32 = d.espectro[o + ini..o + fim].iter().map(|x| x * x).sum::<f32>().max(1e-30);
                        let ee: f32 = e.espectro[o + ini..o + fim].iter().map(|x| x * x).sum::<f32>().max(1e-30);
                        let escala = mat64::sqrt((ed / ee) as f64) as f32;
                        let _ = n;
                        for k in o + ini..o + fim {
                            d.espectro[k] = e.espectro[k] * escala;
                        }
                    }
                }
            }
            base += tam_g * 128;
        }
    }

    /// IMDCT, janela e sobreposicao do canal `k`: 1024 amostras em `saida`.
    fn filtros(&mut self, k: usize, c: &Canal, saida: &mut [f32; 1024]) {
        let ant = self.forma_ant[k] as usize;
        let atual = c.info.forma as usize;
        let mut y = [0f32; 2048];
        if c.info.curta() {
            let mut t = [0f32; 256];
            for w in 0..8 {
                self.imdct_curta.calcular(&c.espectro[w * 128..w * 128 + 128], &mut t);
                let sobe = if w == 0 { &self.curtas[ant] } else { &self.curtas[atual] };
                let desce = &self.curtas[atual];
                let o = 448 + 128 * w;
                for i in 0..128 {
                    y[o + i] += t[i] * sobe[i];
                    y[o + 128 + i] += t[128 + i] * desce[127 - i];
                }
            }
        } else {
            let mut t = [0f32; 2048];
            self.imdct_longa.calcular(&c.espectro, &mut t);
            let seq = c.info.sequencia;
            for i in 0..1024 {
                let w = match seq {
                    FIM_LONGA => {
                        if i < 448 {
                            0.0
                        } else if i < 576 {
                            self.curtas[ant][i - 448]
                        } else {
                            1.0
                        }
                    }
                    _ => self.longas[ant][i],
                };
                y[i] = t[i] * w;
            }
            for i in 0..1024 {
                let w = match seq {
                    INICIO_LONGA => {
                        if i < 448 {
                            1.0
                        } else if i < 576 {
                            self.curtas[atual][127 - (i - 448)]
                        } else {
                            0.0
                        }
                    }
                    _ => self.longas[atual][1023 - i],
                };
                y[1024 + i] = t[1024 + i] * w;
            }
            let _ = SO_LONGA;
        }
        for i in 0..1024 {
            saida[i] = self.sobra[k][i] + y[i];
            self.sobra[k][i] = y[1024 + i];
        }
        self.forma_ant[k] = c.info.forma;
    }
}

/// 2^(x/4).
fn ganho_de(x: i32) -> f32 {
    mat64::pow(2.0, x as f64 / 4.0) as f32
}

fn aplicar_tns(tns: &Tns, info: &Info, espectro: &mut [f32; 1024]) {
    for (w, ini, fim, direcao, lpc) in &tns.filtros {
        let o = if info.curta() { w * 128 } else { 0 };
        let tam = fim - ini;
        let ordem = lpc.len();
        let (mut pos, passo): (isize, isize) = if *direcao { ((o + fim - 1) as isize, -1) } else { ((o + ini) as isize, 1) };
        for m in 0..tam {
            for i in 1..=m.min(ordem) {
                let ant = espectro[(pos - i as isize * passo) as usize];
                espectro[pos as usize] -= ant * lpc[i - 1];
            }
            pos += passo;
        }
    }
}

/// So para o teste do PC (a IMDCT contra a formula).
#[doc(hidden)]
pub fn imdct_teste(n: usize, x: &[f32]) -> Vec<f32> {
    let m = Imdct::novo(n);
    let mut y = vec![0f32; n];
    m.calcular(x, &mut y);
    y
}
