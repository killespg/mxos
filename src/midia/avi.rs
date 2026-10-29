//! AVI com video MJPEG (cada quadro e um JPEG, decodificado pelo
//! web::imagem) e audio PCM opcional. O arquivo e lido aos pedacos: so o
//! indice dos quadros (posicao e tamanho) fica na memoria.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::{Audio, Fonte};
use crate::web::imagem::{self, Imagem};

pub struct Avi {
    fonte: Box<dyn Fonte>,
    pub largura: u32,
    pub altura: u32,
    /// Microssegundos por quadro.
    pub us_por_quadro: u32,
    /// (posicao, tamanho) de cada quadro de video.
    quadros: Vec<(usize, usize)>,
    /// Pedacos de audio PCM: (posicao, tamanho).
    audio: Vec<(usize, usize)>,
    pub audio_taxa: u32,
    pub audio_canais: u16,
    pub audio_bits: u16,
}

fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

fn le16(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}

impl Avi {
    pub fn abrir(mut fonte: Box<dyn Fonte>) -> Result<Avi, &'static str> {
        let total = fonte.tamanho();
        let mut cab = [0u8; 12];
        if fonte.ler(0, &mut cab) < 12 || &cab[0..4] != b"RIFF" || &cab[8..12] != b"AVI " {
            return Err("não é AVI");
        }
        let mut avi = Avi {
            fonte,
            largura: 0,
            altura: 0,
            us_por_quadro: 40_000,
            quadros: Vec::new(),
            audio: Vec::new(),
            audio_taxa: 0,
            audio_canais: 0,
            audio_bits: 0,
        };
        let mut tipo_stream = [0u8; 4];
        let mut video_mjpeg = false;
        // percorre os pedacos; as listas (LIST) sao abertas por dentro
        let mut pos = 12;
        let mut ate: Vec<usize> = alloc::vec![total];
        let mut c = [0u8; 12];
        while pos + 8 <= *ate.last().unwrap_or(&total) {
            if avi.fonte.ler(pos, &mut c[..8]) < 8 {
                break;
            }
            let id = [c[0], c[1], c[2], c[3]];
            let n = le32(&c[4..8]) as usize;
            if &id == b"LIST" || &id == b"RIFF" {
                // entra na lista (o tipo dela sao os 4 bytes seguintes)
                ate.push((pos + 8 + n).min(total));
                pos += 12;
                continue;
            }
            match &id {
                b"avih" => {
                    let mut h = [0u8; 40];
                    avi.fonte.ler(pos + 8, &mut h);
                    avi.us_por_quadro = le32(&h[0..]).max(1);
                    avi.largura = le32(&h[32..]);
                    avi.altura = le32(&h[36..]);
                }
                b"strh" => {
                    let mut h = [0u8; 8];
                    avi.fonte.ler(pos + 8, &mut h);
                    tipo_stream = [h[0], h[1], h[2], h[3]];
                    if &tipo_stream == b"vids" {
                        let codec = [h[4], h[5], h[6], h[7]];
                        video_mjpeg = codec.eq_ignore_ascii_case(b"MJPG") || codec == [0; 4] || codec.eq_ignore_ascii_case(b"jpeg");
                    }
                }
                b"strf" if &tipo_stream == b"auds" => {
                    let mut f = [0u8; 16];
                    avi.fonte.ler(pos + 8, &mut f);
                    if le16(&f[0..]) == 1 {
                        avi.audio_canais = le16(&f[2..]);
                        avi.audio_taxa = le32(&f[4..]);
                        avi.audio_bits = le16(&f[14..]);
                    }
                }
                [_, _, b'd', b'c'] | [_, _, b'd', b'b'] if n > 0 => avi.quadros.push((pos + 8, n)),
                [_, _, b'w', b'b'] if n > 0 => avi.audio.push((pos + 8, n)),
                _ => {}
            }
            pos += 8 + n + (n & 1);
            while ate.len() > 1 && pos >= *ate.last().unwrap_or(&total) {
                ate.pop();
            }
        }
        if !video_mjpeg {
            return Err("AVI sem vídeo MJPEG (só MJPEG é suportado)");
        }
        if avi.quadros.is_empty() {
            return Err("AVI sem quadros");
        }
        Ok(avi)
    }

    pub fn total_quadros(&self) -> usize {
        self.quadros.len()
    }

    pub fn duracao_ms(&self) -> u64 {
        self.quadros.len() as u64 * self.us_por_quadro as u64 / 1000
    }

    /// Decodifica o quadro `i`.
    pub fn quadro(&mut self, i: usize) -> Option<Imagem> {
        let &(pos, n) = self.quadros.get(i)?;
        let mut d = alloc::vec![0u8; n];
        self.fonte.ler(pos, &mut d);
        // MJPEG costuma vir sem a tabela de Huffman (DHT): poe a padrao
        if !tem_dht(&d) {
            d = com_dht_padrao(&d);
        }
        imagem::decodificar(&d)
    }

    pub fn descricao(&self) -> String {
        let fps10 = 10_000_000 / self.us_por_quadro.max(1);
        let audio = if self.audio_taxa > 0 { format!(", áudio PCM {} Hz", self.audio_taxa) } else { String::new() };
        format!("AVI MJPEG {}x{}, {}.{} quadros/s{audio}", self.largura, self.altura, fps10 / 10, fps10 % 10)
    }

    /// O audio do AVI (PCM), se houver. Consome os indices do audio.
    pub fn tirar_audio(&mut self, fonte: Box<dyn Fonte>) -> Option<Box<dyn Audio>> {
        if self.audio_taxa == 0 || self.audio.is_empty() || !matches!(self.audio_bits, 8 | 16) {
            return None;
        }
        Some(Box::new(AudioAvi {
            fonte,
            pedacos: core::mem::take(&mut self.audio),
            atual: 0,
            dentro: 0,
            taxa: self.audio_taxa,
            canais: self.audio_canais.max(1),
            bits: self.audio_bits,
            quadros: 0,
            buf: Vec::new(),
        }))
    }
}

fn tem_dht(d: &[u8]) -> bool {
    let mut i = 2;
    while i + 4 <= d.len() {
        if d[i] != 0xFF {
            return false;
        }
        let m = d[i + 1];
        if m == 0xC4 {
            return true;
        }
        if m == 0xDA {
            return false;
        }
        let n = u16::from_be_bytes([d[i + 2], d[i + 3]]) as usize;
        i += 2 + n;
    }
    false
}

/// As tabelas de Huffman tipicas do JPEG (anexo K.3 da norma), que o MJPEG
/// supoe quando o quadro nao traz DHT.
fn com_dht_padrao(d: &[u8]) -> Vec<u8> {
    const DC_L: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
    const DC_V: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
    const DC_C_L: [u8; 16] = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0];
    const AC_Y_L: [u8; 16] = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7d];
    const AC_C_L: [u8; 16] = [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77];
    const AC_Y_V: [u8; 162] = [
        0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07, 0x22, 0x71, 0x14, 0x32,
        0x81, 0x91, 0xa1, 0x08, 0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16,
        0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45,
        0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
        0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92, 0x93, 0x94,
        0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6,
        0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8,
        0xd9, 0xda, 0xe1, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
        0xf9, 0xfa,
    ];
    const AC_C_V: [u8; 162] = [
        0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71, 0x13, 0x22, 0x32, 0x81,
        0x08, 0x14, 0x42, 0x91, 0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52, 0xf0, 0x15, 0x62, 0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34,
        0xe1, 0x25, 0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44,
        0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68,
        0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92,
        0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4,
        0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6,
        0xd7, 0xd8, 0xd9, 0xda, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
        0xf9, 0xfa,
    ];
    let mut dht = Vec::new();
    for (classe, l, v) in [(0x00u8, &DC_L[..], &DC_V[..]), (0x01, &DC_C_L[..], &DC_V[..]), (0x10, &AC_Y_L[..], &AC_Y_V[..]), (0x11, &AC_C_L[..], &AC_C_V[..])]
    {
        let n = 2 + 1 + 16 + v.len();
        dht.extend_from_slice(&[0xFF, 0xC4, (n >> 8) as u8, n as u8, classe]);
        dht.extend_from_slice(l);
        dht.extend_from_slice(v);
    }
    // depois do SOI
    let mut out = Vec::with_capacity(d.len() + dht.len());
    out.extend_from_slice(&d[..2.min(d.len())]);
    out.extend_from_slice(&dht);
    out.extend_from_slice(d.get(2..).unwrap_or(&[]));
    out
}

struct AudioAvi {
    fonte: Box<dyn Fonte>,
    pedacos: Vec<(usize, usize)>,
    atual: usize,
    dentro: usize,
    taxa: u32,
    canais: u16,
    bits: u16,
    quadros: u64,
    buf: Vec<u8>,
}

impl Audio for AudioAvi {
    fn taxa(&self) -> u32 {
        self.taxa
    }

    fn ler(&mut self, saida: &mut [i16]) -> usize {
        let bps = self.bits as usize / 8;
        let qb = bps * self.canais as usize;
        let mut k = 0;
        while k + 1 < saida.len() {
            let Some(&(pos, n)) = self.pedacos.get(self.atual) else { break };
            let resto = n - self.dentro;
            if resto < qb {
                self.atual += 1;
                self.dentro = 0;
                continue;
            }
            let quadros = ((saida.len() - k) / 2).min(resto / qb);
            self.buf.resize(quadros * qb, 0);
            self.fonte.ler(pos + self.dentro, &mut self.buf);
            for q in 0..quadros {
                let b = &self.buf[q * qb..];
                let am = |o: usize| if bps == 1 { ((b[o] as i16) - 128) << 8 } else { i16::from_le_bytes([b[o], b[o + 1]]) };
                let e = am(0);
                let d = if self.canais > 1 { am(bps) } else { e };
                saida[k] = e;
                saida[k + 1] = d;
                k += 2;
            }
            self.dentro += quadros * qb;
            self.quadros += quadros as u64;
        }
        k
    }

    fn duracao_ms(&self) -> Option<u64> {
        None
    }

    fn posicao_ms(&self) -> u64 {
        self.quadros * 1000 / self.taxa.max(1) as u64
    }

    fn descricao(&self) -> String {
        String::from("áudio do AVI")
    }
}
