//! MP4 (ISO BMFF): acha as trilhas, o avcC/esds e onde esta cada amostra
//! no arquivo. So o indice vai para a memoria; as amostras sao lidas da
//! `Fonte` quando o player pede (o arquivo pode ter gigas).
//!
//! Caixas usadas: moov/trak/mdia (mdhd, hdlr), minf/stbl (stsd, stts,
//! ctts, stss, stsz/stz2, stsc, stco/co64) e tkhd para o tamanho. MP4
//! fragmentado (moof) ainda nao.

use alloc::vec;
use alloc::vec::Vec;

use super::Fonte;

/// Uma amostra (um quadro de video ou um pacote de audio).
#[derive(Clone, Copy, Debug)]
pub struct Amostra {
    pub pos: u64,
    pub tamanho: u32,
    /// Instante de decodificacao (na escala da trilha).
    pub dts: u64,
    /// Instante de apresentacao = dts + desvio (ctts).
    pub desvio: i32,
    pub chave: bool,
}

#[derive(Default)]
pub struct Trilha {
    /// "vide", "soun"...
    pub tipo: [u8; 4],
    /// "avc1", "mp4a"...
    pub codec: [u8; 4],
    /// Unidades por segundo dos tempos das amostras.
    pub escala: u32,
    pub largura: u32,
    pub altura: u32,
    /// avcC (H.264) ou esds (AAC), sem o cabecalho da caixa.
    pub config: Vec<u8>,
    pub canais: u16,
    pub taxa: u32,
    pub amostras: Vec<Amostra>,
}

impl Trilha {
    /// Duracao em milissegundos.
    pub fn duracao_ms(&self) -> u64 {
        let fim = self.amostras.last().map(|a| a.dts).unwrap_or(0);
        let passo = match self.amostras.len() {
            0 | 1 => 0,
            n => self.amostras[n - 1].dts - self.amostras[n - 2].dts,
        };
        (fim + passo) * 1000 / self.escala.max(1) as u64
    }
}

pub struct Mp4 {
    pub trilhas: Vec<Trilha>,
}

type R<T> = Result<T, &'static str>;

fn u16_(b: &[u8], i: usize) -> u16 {
    u16::from_be_bytes([b[i], b[i + 1]])
}

fn u32_(b: &[u8], i: usize) -> u32 {
    u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

fn u64_(b: &[u8], i: usize) -> u64 {
    (u32_(b, i) as u64) << 32 | u32_(b, i + 4) as u64
}

/// Caixas de um pedaco ja na memoria: (tipo, conteudo).
struct Caixas<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Iterator for Caixas<'a> {
    type Item = ([u8; 4], &'a [u8]);
    fn next(&mut self) -> Option<Self::Item> {
        let b = self.b;
        if self.i + 8 > b.len() {
            return None;
        }
        let mut tamanho = u32_(b, self.i) as usize;
        let tipo = [b[self.i + 4], b[self.i + 5], b[self.i + 6], b[self.i + 7]];
        let mut cab = 8;
        if tamanho == 1 {
            if self.i + 16 > b.len() {
                return None;
            }
            tamanho = u64_(b, self.i + 8) as usize;
            cab = 16;
        } else if tamanho == 0 {
            tamanho = b.len() - self.i;
        }
        if tamanho < cab || self.i + tamanho > b.len() {
            return None;
        }
        let conteudo = &b[self.i + cab..self.i + tamanho];
        self.i += tamanho;
        Some((tipo, conteudo))
    }
}

fn caixas(b: &[u8]) -> Caixas<'_> {
    Caixas { b, i: 0 }
}

fn achar<'a>(b: &'a [u8], tipo: &[u8; 4]) -> Option<&'a [u8]> {
    caixas(b).find(|(t, _)| t == tipo).map(|(_, c)| c)
}

impl Mp4 {
    /// Le o indice (a caixa moov) do arquivo.
    pub fn abrir(f: &mut dyn Fonte) -> R<Mp4> {
        let total = f.tamanho() as u64;
        let mut pos = 0u64;
        let mut cab = [0u8; 16];
        loop {
            if pos + 8 > total {
                return Err("MP4 sem moov");
            }
            if f.ler(pos as usize, &mut cab) < 8 {
                return Err("MP4 cortado");
            }
            let mut tamanho = u32_(&cab, 0) as u64;
            let tipo = &cab[4..8];
            let mut c = 8;
            if tamanho == 1 {
                tamanho = u64_(&cab, 8);
                c = 16;
            } else if tamanho == 0 {
                tamanho = total - pos;
            }
            if tamanho < c {
                return Err("caixa MP4 invalida");
            }
            if tipo == b"moov" {
                if tamanho > 64 << 20 {
                    return Err("moov grande demais");
                }
                let mut moov = vec![0u8; (tamanho - c) as usize];
                if f.ler((pos + c) as usize, &mut moov) < moov.len() {
                    return Err("moov cortado");
                }
                return Mp4::do_moov(&moov);
            }
            if tipo == b"moof" {
                return Err("MP4 fragmentado ainda nao");
            }
            pos += tamanho;
        }
    }

    fn do_moov(moov: &[u8]) -> R<Mp4> {
        let mut trilhas = Vec::new();
        for (t, trak) in caixas(moov) {
            if &t != b"trak" {
                continue;
            }
            if let Some(tr) = trilha(trak)? {
                trilhas.push(tr);
            }
        }
        if trilhas.is_empty() {
            return Err("MP4 sem trilhas");
        }
        Ok(Mp4 { trilhas })
    }

    pub fn video(&self) -> Option<&Trilha> {
        self.trilhas.iter().find(|t| &t.tipo == b"vide")
    }

    pub fn audio(&self) -> Option<&Trilha> {
        self.trilhas.iter().find(|t| &t.tipo == b"soun")
    }
}

fn trilha(trak: &[u8]) -> R<Option<Trilha>> {
    let mut tr = Trilha::default();
    if let Some(tkhd) = achar(trak, b"tkhd") {
        // largura e altura em 16.16 no fim da caixa
        if tkhd.len() >= 8 {
            let n = tkhd.len();
            tr.largura = u32_(tkhd, n - 8) >> 16;
            tr.altura = u32_(tkhd, n - 4) >> 16;
        }
    }
    let Some(mdia) = achar(trak, b"mdia") else { return Ok(None) };
    if let Some(mdhd) = achar(mdia, b"mdhd") {
        tr.escala = if mdhd.first() == Some(&1) && mdhd.len() >= 24 { u32_(mdhd, 20) } else if mdhd.len() >= 16 { u32_(mdhd, 12) } else { 0 };
    }
    if let Some(hdlr) = achar(mdia, b"hdlr") {
        if hdlr.len() >= 12 {
            tr.tipo.copy_from_slice(&hdlr[8..12]);
        }
    }
    let Some(stbl) = achar(mdia, b"minf").and_then(|m| achar(m, b"stbl")) else { return Ok(None) };
    if let Some(stsd) = achar(stbl, b"stsd") {
        descricao(&mut tr, stsd);
    }
    tr.amostras = tabela(stbl)?;
    if tr.escala == 0 {
        return Ok(None);
    }
    Ok(Some(tr))
}

/// stsd: o codec da primeira entrada e a configuracao dele.
fn descricao(tr: &mut Trilha, stsd: &[u8]) {
    if stsd.len() < 16 {
        return;
    }
    let entrada = &stsd[8..];
    let Some((codec, corpo)) = caixas(entrada).next() else { return };
    tr.codec = codec;
    if &tr.tipo == b"vide" && corpo.len() >= 78 {
        tr.largura = u16_(corpo, 24) as u32;
        tr.altura = u16_(corpo, 26) as u32;
        if let Some(c) = achar(&corpo[78..], b"avcC") {
            tr.config = c.to_vec();
        }
    } else if &tr.tipo == b"soun" && corpo.len() >= 28 {
        tr.canais = u16_(corpo, 16);
        tr.taxa = u32_(corpo, 24) >> 16;
        if let Some(c) = achar(&corpo[28..], b"esds") {
            tr.config = c.to_vec();
        }
    }
}

/// Monta a lista de amostras com as tabelas do stbl.
fn tabela(stbl: &[u8]) -> R<Vec<Amostra>> {
    // tamanhos
    let mut tamanhos: Vec<u32> = Vec::new();
    if let Some(stsz) = achar(stbl, b"stsz") {
        if stsz.len() < 12 {
            return Err("stsz curto");
        }
        let (fixo, n) = (u32_(stsz, 4), u32_(stsz, 8) as usize);
        if fixo != 0 {
            tamanhos = vec![fixo; n];
        } else {
            if stsz.len() < 12 + 4 * n {
                return Err("stsz cortado");
            }
            tamanhos = (0..n).map(|i| u32_(stsz, 12 + 4 * i)).collect();
        }
    } else if let Some(stz2) = achar(stbl, b"stz2") {
        if stz2.len() < 12 {
            return Err("stz2 curto");
        }
        let (bits, n) = (stz2[7], u32_(stz2, 8) as usize);
        let d = &stz2[12..];
        for i in 0..n {
            let v = match bits {
                4 => d.get(i / 2).map(|&b| if i % 2 == 0 { b >> 4 } else { b & 15 } as u32),
                8 => d.get(i).map(|&b| b as u32),
                16 => (2 * i + 2 <= d.len()).then(|| u16_(d, 2 * i) as u32),
                _ => None,
            };
            tamanhos.push(v.ok_or("stz2 cortado")?);
        }
    }
    let n = tamanhos.len();
    // pedacos (chunks): onde comecam no arquivo
    let mut pedacos: Vec<u64> = Vec::new();
    if let Some(stco) = achar(stbl, b"stco") {
        let k = if stco.len() >= 8 { u32_(stco, 4) as usize } else { 0 };
        if stco.len() < 8 + 4 * k {
            return Err("stco cortado");
        }
        pedacos = (0..k).map(|i| u32_(stco, 8 + 4 * i) as u64).collect();
    } else if let Some(co64) = achar(stbl, b"co64") {
        let k = if co64.len() >= 8 { u32_(co64, 4) as usize } else { 0 };
        if co64.len() < 8 + 8 * k {
            return Err("co64 cortado");
        }
        pedacos = (0..k).map(|i| u64_(co64, 8 + 8 * i)).collect();
    }
    // amostras por pedaco (stsc): (primeiro pedaco, quantas)
    let mut stsc: Vec<(u32, u32)> = Vec::new();
    if let Some(s) = achar(stbl, b"stsc") {
        let k = if s.len() >= 8 { u32_(s, 4) as usize } else { 0 };
        if s.len() < 8 + 12 * k {
            return Err("stsc cortado");
        }
        stsc = (0..k).map(|i| (u32_(s, 8 + 12 * i), u32_(s, 12 + 12 * i))).collect();
    }
    let mut amostras: Vec<Amostra> = Vec::with_capacity(n);
    let mut a = 0usize;
    for (pi, &inicio) in pedacos.iter().enumerate() {
        // a entrada do stsc que vale para este pedaco (numerados de 1)
        let num = pi as u32 + 1;
        let por = stsc.iter().rev().find(|e| e.0 <= num).map(|e| e.1).unwrap_or(0);
        let mut p = inicio;
        for _ in 0..por {
            if a >= n {
                break;
            }
            amostras.push(Amostra { pos: p, tamanho: tamanhos[a], dts: 0, desvio: 0, chave: true });
            p += tamanhos[a] as u64;
            a += 1;
        }
    }
    // tempos (stts): (quantas, duracao)
    if let Some(s) = achar(stbl, b"stts") {
        let k = if s.len() >= 8 { u32_(s, 4) as usize } else { 0 };
        let (mut t, mut i) = (0u64, 0usize);
        for e in 0..k {
            if s.len() < 16 + 8 * e {
                break;
            }
            let (qtd, dur) = (u32_(s, 8 + 8 * e), u32_(s, 12 + 8 * e));
            for _ in 0..qtd {
                if let Some(am) = amostras.get_mut(i) {
                    am.dts = t;
                }
                t += dur as u64;
                i += 1;
            }
        }
    }
    // desvio de apresentacao (ctts), para os quadros B
    if let Some(s) = achar(stbl, b"ctts") {
        let k = if s.len() >= 8 { u32_(s, 4) as usize } else { 0 };
        let mut i = 0usize;
        for e in 0..k {
            if s.len() < 16 + 8 * e {
                break;
            }
            let (qtd, desvio) = (u32_(s, 8 + 8 * e), u32_(s, 12 + 8 * e) as i32);
            for _ in 0..qtd {
                if let Some(am) = amostras.get_mut(i) {
                    am.desvio = desvio;
                }
                i += 1;
            }
        }
    }
    // quadros-chave (stss); sem a caixa, todos sao
    if let Some(s) = achar(stbl, b"stss") {
        for am in amostras.iter_mut() {
            am.chave = false;
        }
        let k = if s.len() >= 8 { u32_(s, 4) as usize } else { 0 };
        for e in 0..k {
            if s.len() < 12 + 4 * e {
                break;
            }
            let i = u32_(s, 8 + 4 * e) as usize;
            if let Some(am) = i.checked_sub(1).and_then(|i| amostras.get_mut(i)) {
                am.chave = true;
            }
        }
    }
    Ok(amostras)
}

/// avcC: (bytes do tamanho de cada NAL, SPSs, PPSs).
pub fn avcc(c: &[u8]) -> Option<(usize, Vec<&[u8]>, Vec<&[u8]>)> {
    if c.len() < 7 || c[0] != 1 {
        return None;
    }
    let tam = (c[4] & 3) as usize + 1;
    let mut i = 5;
    let lista = |n: usize, i: &mut usize| -> Option<Vec<&[u8]>> {
        let mut v = Vec::new();
        for _ in 0..n {
            let l = u16_(c.get(*i..*i + 2)?, 0) as usize;
            v.push(c.get(*i + 2..*i + 2 + l)?);
            *i += 2 + l;
        }
        Some(v)
    };
    let nsps = (c[i] & 0x1F) as usize;
    i += 1;
    let sps = lista(nsps, &mut i)?;
    let npps = *c.get(i)? as usize;
    i += 1;
    let pps = lista(npps, &mut i)?;
    Some((tam, sps, pps))
}
