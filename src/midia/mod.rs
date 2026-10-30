//! Midia do MXOS, feita aqui: audio (WAV, MOD do ProTracker, MP3) e
//! video (AVI com quadros JPEG). Os decodificadores entregam PCM estereo
//! de 16 bits em pedacos, entao o arquivo nao precisa caber na memoria
//! (o WAV e o MP3 sao lidos aos trechos do disco).
//!
//! So core + alloc: o mesmo codigo roda nos testes do PC.

pub mod aac;
pub mod aac_tabelas;
pub mod avi;
pub mod h264;
pub mod h264_cabac;
pub mod mp3;
pub mod mp3_tabelas;
pub mod mp4;
pub mod tracker;
pub mod wav;

use alloc::boxed::Box;
use alloc::string::String;

/// De onde vem os bytes: um arquivo do disco lido aos trechos, ou a
/// memoria (testes, arquivos pequenos).
pub trait Fonte {
    fn tamanho(&self) -> usize;
    /// Le `destino.len()` bytes a partir de `pos` (menos no fim). Devolve
    /// quantos leu.
    fn ler(&mut self, pos: usize, destino: &mut [u8]) -> usize;
}

impl Fonte for alloc::vec::Vec<u8> {
    fn tamanho(&self) -> usize {
        self.len()
    }
    fn ler(&mut self, pos: usize, destino: &mut [u8]) -> usize {
        if pos >= self.len() {
            return 0;
        }
        let n = destino.len().min(self.len() - pos);
        destino[..n].copy_from_slice(&self[pos..pos + n]);
        n
    }
}

/// Um decodificador de audio: PCM estereo i16 intercalado.
pub trait Audio {
    fn taxa(&self) -> u32;
    /// Enche `saida` (pares esquerda/direita). Devolve quantas amostras
    /// escreveu (0 = acabou).
    fn ler(&mut self, saida: &mut [i16]) -> usize;
    /// Duracao total em ms, se o formato sabe.
    fn duracao_ms(&self) -> Option<u64>;
    fn posicao_ms(&self) -> u64;
    /// Nome da musica (MOD) ou descricao do formato.
    fn descricao(&self) -> String;
    /// Pula para `ms` (aproximado). false = nao da.
    fn ir_para(&mut self, _ms: u64) -> bool {
        false
    }
}

/// Abre um audio pelo conteudo (e a extensao para o MOD, que nao tem
/// assinatura no comeco).
pub fn abrir_audio(mut fonte: Box<dyn Fonte>, nome: &str, taxa_saida: u32) -> Result<Box<dyn Audio>, &'static str> {
    let mut cab = [0u8; 12];
    let n = fonte.ler(0, &mut cab);
    let cab = &cab[..n];
    let minusculo = nome.to_ascii_lowercase();
    if cab.starts_with(b"RIFF") && cab.get(8..12) == Some(b"WAVE") {
        return Ok(Box::new(wav::Wav::novo(fonte)?));
    }
    if minusculo.ends_with(".mod") || tracker::parece_mod(&mut *fonte) {
        return Ok(Box::new(tracker::Tracker::novo(&mut *fonte, taxa_saida)?));
    }
    if minusculo.ends_with(".mp3") || cab.starts_with(b"ID3") || (cab.len() > 1 && cab[0] == 0xFF && cab[1] & 0xE0 == 0xE0) {
        return Ok(Box::new(mp3::Mp3::novo(fonte)?));
    }
    Err("formato de áudio desconhecido")
}

/// Converte a taxa de um audio para a da placa (interpolacao linear).
pub struct Reamostrador {
    de: u32,
    para: u32,
    /// Posicao fracionaria entre a amostra anterior e a atual (16.16).
    fase: u32,
    anterior: [i16; 2],
    atual: [i16; 2],
    entrada: alloc::vec::Vec<i16>,
    lido: usize,
    acabou: bool,
}

impl Reamostrador {
    pub fn novo(de: u32, para: u32) -> Reamostrador {
        Reamostrador { de, para, fase: 0, anterior: [0; 2], atual: [0; 2], entrada: alloc::vec::Vec::new(), lido: 0, acabou: false }
    }

    fn proximo_par(&mut self, a: &mut dyn Audio) -> bool {
        if self.lido + 2 > self.entrada.len() {
            self.entrada.resize(4096, 0);
            let n = a.ler(&mut self.entrada);
            self.entrada.truncate(n & !1);
            self.lido = 0;
            if n < 2 {
                self.acabou = true;
                return false;
            }
        }
        self.anterior = self.atual;
        self.atual = [self.entrada[self.lido], self.entrada[self.lido + 1]];
        self.lido += 2;
        true
    }

    /// Enche `saida` na taxa da placa. Devolve as amostras escritas.
    pub fn ler(&mut self, a: &mut dyn Audio, saida: &mut [i16]) -> usize {
        if self.de == self.para {
            return a.ler(saida);
        }
        if self.acabou {
            return 0;
        }
        let passo = ((self.de as u64) << 16) / self.para as u64;
        let mut k = 0;
        while k + 1 < saida.len() {
            while self.fase >= 1 << 16 {
                self.fase -= 1 << 16;
                if !self.proximo_par(a) {
                    return k;
                }
            }
            let f = self.fase as i32;
            for c in 0..2 {
                let x0 = self.anterior[c] as i32;
                let x1 = self.atual[c] as i32;
                saida[k + c] = (x0 + (((x1 - x0) * (f >> 1)) >> 15)) as i16;
            }
            k += 2;
            self.fase += passo as u32;
        }
        k
    }

    #[allow(dead_code)]
    pub fn acabou(&self) -> bool {
        self.acabou
    }
}
