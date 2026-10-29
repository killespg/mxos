//! WAV (RIFF) com PCM de 8, 16, 24 ou 32 bits, mono ou estereo, lido do
//! disco aos pedacos.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::{Audio, Fonte};

const PEDACO: usize = 32 * 1024;

pub struct Wav {
    fonte: Box<dyn Fonte>,
    taxa: u32,
    canais: u16,
    bits: u16,
    inicio: usize,
    tamanho: usize,
    /// Bytes de dados ja entregues.
    pos: usize,
    buf: Vec<u8>,
}

fn le16(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}

fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

impl Wav {
    pub fn novo(mut fonte: Box<dyn Fonte>) -> Result<Wav, &'static str> {
        let total = fonte.tamanho();
        let mut pos = 12;
        let mut formato: Option<(u16, u16, u32, u16)> = None;
        let mut cab = [0u8; 8];
        // os pedacos: "fmt " diz o formato, "data" tem as amostras
        while pos + 8 <= total {
            if fonte.ler(pos, &mut cab) < 8 {
                break;
            }
            let id = [cab[0], cab[1], cab[2], cab[3]];
            let n = le32(&cab[4..]) as usize;
            if &id == b"fmt " {
                let mut f = [0u8; 16];
                fonte.ler(pos + 8, &mut f);
                let tipo = le16(&f[0..]);
                // 1 = PCM; 0xFFFE = extensivel (assume PCM)
                if tipo != 1 && tipo != 0xFFFE {
                    return Err("WAV comprimido não é suportado (só PCM)");
                }
                formato = Some((tipo, le16(&f[2..]), le32(&f[4..]), le16(&f[14..])));
            } else if &id == b"data" {
                let (_, canais, taxa, bits) = formato.ok_or("WAV sem formato")?;
                if canais == 0 || !matches!(bits, 8 | 16 | 24 | 32) {
                    return Err("WAV com formato estranho");
                }
                return Ok(Wav { fonte, taxa, canais, bits, inicio: pos + 8, tamanho: n.min(total - pos - 8), pos: 0, buf: Vec::new() });
            }
            pos += 8 + n + (n & 1);
        }
        Err("WAV sem dados")
    }

    fn quadro_bytes(&self) -> usize {
        self.canais as usize * (self.bits as usize / 8)
    }
}

impl Audio for Wav {
    fn taxa(&self) -> u32 {
        self.taxa
    }

    fn ler(&mut self, saida: &mut [i16]) -> usize {
        let qb = self.quadro_bytes();
        let quadros = (saida.len() / 2).min(PEDACO / qb).min((self.tamanho - self.pos) / qb);
        if quadros == 0 {
            return 0;
        }
        self.buf.resize(quadros * qb, 0);
        let lido = self.fonte.ler(self.inicio + self.pos, &mut self.buf);
        let quadros = lido / qb;
        self.pos += quadros * qb;
        let bps = self.bits as usize / 8;
        let amostra = |b: &[u8]| -> i16 {
            match bps {
                1 => ((b[0] as i16) - 128) << 8,
                2 => i16::from_le_bytes([b[0], b[1]]),
                3 => i16::from_le_bytes([b[1], b[2]]),
                _ => i16::from_le_bytes([b[2], b[3]]),
            }
        };
        for q in 0..quadros {
            let base = q * qb;
            let e = amostra(&self.buf[base..]);
            let d = if self.canais > 1 { amostra(&self.buf[base + bps..]) } else { e };
            saida[2 * q] = e;
            saida[2 * q + 1] = d;
        }
        quadros * 2
    }

    fn duracao_ms(&self) -> Option<u64> {
        Some((self.tamanho / self.quadro_bytes()) as u64 * 1000 / self.taxa.max(1) as u64)
    }

    fn posicao_ms(&self) -> u64 {
        (self.pos / self.quadro_bytes()) as u64 * 1000 / self.taxa.max(1) as u64
    }

    fn descricao(&self) -> String {
        format!("WAV PCM {} bits, {} canal(is), {} Hz", self.bits, self.canais, self.taxa)
    }

    fn ir_para(&mut self, ms: u64) -> bool {
        let qb = self.quadro_bytes();
        let q = (ms * self.taxa as u64 / 1000) as usize;
        self.pos = (q * qb).min(self.tamanho / qb * qb);
        true
    }
}
