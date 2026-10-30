//! Arquivos do disco pela ABI (ABRIR, LER, ESCREVER, LISTAR), com os mesmos
//! nomes do crate::armazenamento do kernel. Caminho absoluto ("/documentos/
//! a.txt") ou relativo a pasta do app ("dados/config.txt").

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::sys::{self, chamada};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Erro {
    SemDisco,
    Disco,
    NaoAchado,
    JaExiste,
    NaoEPasta,
    EPasta,
    PastaNaoVazia,
    Cheio,
    NomeInvalido,
    Corrompido(&'static str),
    SoLeitura,
    /// A memoria do app acabou.
    Memoria,
}

impl Erro {
    pub fn descricao(&self) -> &'static str {
        match self {
            Erro::SemDisco => "nenhum disco montado",
            Erro::Disco => "erro de leitura ou escrita no disco",
            Erro::NaoAchado => "arquivo ou pasta não encontrado",
            Erro::JaExiste => "já existe",
            Erro::NaoEPasta => "não é uma pasta",
            Erro::EPasta => "é uma pasta",
            Erro::PastaNaoVazia => "a pasta não está vazia",
            Erro::Cheio => "disco cheio",
            Erro::NomeInvalido => "nome inválido",
            Erro::Corrompido(motivo) => motivo,
            Erro::SoLeitura => "o disco é só de leitura",
            Erro::Memoria => "memória insuficiente",
        }
    }

    fn de(codigo: isize) -> Erro {
        match codigo {
            sys::ERRO_NAO_EXISTE => Erro::NaoAchado,
            sys::ERRO_MEMORIA => Erro::Memoria,
            sys::ERRO_INVALIDO => Erro::NomeInvalido,
            _ => Erro::Disco,
        }
    }
}

/// Um item de `listar`. Pastas tem tamanho 0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemPasta {
    pub nome: String,
    pub pasta: bool,
    pub tamanho: u64,
}

/// Um arquivo aberto (fecha sozinho; a escrita vai para o disco ao fechar).
pub struct Arquivo(usize);

pub enum Modo {
    Ler = 0,
    /// Cria ou zera.
    Escrever = 1,
    Acrescentar = 2,
}

impl Arquivo {
    pub fn abrir(caminho: &str, modo: Modo) -> Result<Arquivo, Erro> {
        let r = chamada(sys::ABRIR, [caminho.as_ptr() as usize, caminho.len(), modo as usize, 0, 0, 0]);
        if r < 0 { Err(Erro::de(r)) } else { Ok(Arquivo(r as usize)) }
    }

    /// Le ate encher `buf`. 0 = fim.
    pub fn ler(&mut self, buf: &mut [u8]) -> Result<usize, Erro> {
        let r = chamada(sys::LER, [self.0, buf.as_mut_ptr() as usize, buf.len(), 0, 0, 0]);
        if r < 0 { Err(Erro::de(r)) } else { Ok(r as usize) }
    }

    pub fn escrever(&mut self, dados: &[u8]) -> Result<usize, Erro> {
        let r = chamada(sys::ESCREVER, [self.0, dados.as_ptr() as usize, dados.len(), 0, 0, 0]);
        if r < 0 { Err(Erro::de(r)) } else { Ok(r as usize) }
    }

    pub fn tamanho(&self) -> u64 {
        sys::chamada64(sys::TAMANHO, [self.0, 0, 0, 0, 0, 0])
    }

    pub fn posicionar(&mut self, pos: u64) {
        chamada(sys::POSICIONAR, [self.0, pos as usize, (pos >> 32) as usize, 0, 0, 0]);
    }

    /// Fecha agora e diz se a gravacao deu certo.
    pub fn fechar(self) -> Result<(), Erro> {
        let r = chamada(sys::FECHAR, [self.0, 0, 0, 0, 0, 0]);
        core::mem::forget(self);
        if r < 0 { Err(Erro::de(r)) } else { Ok(()) }
    }
}

impl Drop for Arquivo {
    fn drop(&mut self) {
        chamada(sys::FECHAR, [self.0, 0, 0, 0, 0, 0]);
    }
}

/// O arquivo inteiro.
pub fn ler(caminho: &str) -> Result<Vec<u8>, Erro> {
    let mut a = Arquivo::abrir(caminho, Modo::Ler)?;
    let mut dados = vec![0u8; a.tamanho() as usize];
    let mut feito = 0;
    while feito < dados.len() {
        let n = a.ler(&mut dados[feito..])?;
        if n == 0 {
            break;
        }
        feito += n;
    }
    dados.truncate(feito);
    Ok(dados)
}

/// Cria (ou troca) o arquivo com `dados`.
pub fn escrever(caminho: &str, dados: &[u8]) -> Result<(), Erro> {
    let mut a = Arquivo::abrir(caminho, Modo::Escrever)?;
    a.escrever(dados)?;
    a.fechar()
}

/// O que tem na pasta.
pub fn listar(caminho: &str) -> Result<Vec<ItemPasta>, Erro> {
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let r = chamada(sys::LISTAR, [caminho.as_ptr() as usize, caminho.len(), buf.as_mut_ptr() as usize, buf.len(), 0, 0]);
        if r < 0 {
            return Err(Erro::de(r));
        }
        if r as usize > buf.len() {
            buf.resize(r as usize, 0);
            continue;
        }
        let texto = String::from_utf8_lossy(&buf[..r as usize]);
        let mut itens = Vec::new();
        for linha in texto.lines() {
            if let Some(nome) = linha.strip_prefix("D ") {
                itens.push(ItemPasta { nome: String::from(nome), pasta: true, tamanho: 0 });
            } else if let Some(resto) = linha.strip_prefix("F ") {
                if let Some((tamanho, nome)) = resto.split_once(' ') {
                    itens.push(ItemPasta { nome: String::from(nome), pasta: false, tamanho: tamanho.parse().unwrap_or(0) });
                }
            }
        }
        return Ok(itens);
    }
}
