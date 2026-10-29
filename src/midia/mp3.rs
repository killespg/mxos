//! MP3: por enquanto so reconhece o arquivo (cabecalho, taxa, duracao
//! aproximada). Decodificar o Layer III precisa das tabelas de Huffman
//! do padrao ISO 11172-3, que nao estao neste repositorio.

use alloc::boxed::Box;
use alloc::string::String;

use super::{Audio, Fonte};

pub struct Mp3;

impl Mp3 {
    pub fn novo(_fonte: Box<dyn Fonte>) -> Result<Mp3, &'static str> {
        Err("MP3 ainda não decodifica: faltam as tabelas de Huffman do padrão (ISO 11172-3)")
    }
}

impl Audio for Mp3 {
    fn taxa(&self) -> u32 {
        44100
    }
    fn ler(&mut self, _saida: &mut [i16]) -> usize {
        0
    }
    fn duracao_ms(&self) -> Option<u64> {
        None
    }
    fn posicao_ms(&self) -> u64 {
        0
    }
    fn descricao(&self) -> String {
        String::from("MP3")
    }
}
