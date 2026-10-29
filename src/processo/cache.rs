//! Cache de paginas dos arquivos do disco: cada arquivo lido vira quadros
//! de 4 KB, trazidos do disco so quando alguem precisa daquele pedaco, e
//! divididos por todo mundo que le o mesmo arquivo:
//!
//!   - mmap de arquivo (o ld-linux carregando a libc, por exemplo) poe os
//!     proprios quadros do cache no espaco do programa, so de leitura; quem
//!     escreve ganha a copia dele (copy-on-write, memoria.rs). Dez programas
//!     usando a libc tem uma libc so na RAM.
//!   - read/pread de um arquivo aberto so para leitura copia do cache, sem
//!     trazer o arquivo inteiro para a memoria do processo.
//!
//! Escrever, apagar ou renomear um arquivo (armazenamento) tira ele do
//! cache; quem ja tinha os quadros mapeados fica com a versao antiga, como
//! no Linux com um arquivo trocado inteiro. Passando do LIMITE, os arquivos
//! usados ha mais tempo saem do cache (os quadros que algum processo ainda
//! mapeia continuam vivos com ele).

use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::{RefCell, UnsafeCell};

use super::arquivos::{errno, EIO};
use super::memoria::{self, Quadro, PAGINA};
use crate::armazenamento;

/// Quadros que o cache guarda antes de soltar os mais velhos (96 MB).
const LIMITE: usize = 96 << 20 >> 12;
/// Quantas paginas um read pede ao disco de uma vez (leitura adiantada).
const ADIANTAR: usize = 16;

pub struct Arquivo {
    pub tamanho: usize,
    cadeia: Vec<u32>,
    quadros: RefCell<Vec<Option<Rc<Box<Quadro>>>>>,
    usado: RefCell<u64>,
}

impl Arquivo {
    /// Os quadros das paginas [primeira, primeira + quantas), lendo do
    /// disco o que faltar (de uma vez, os seguidos).
    pub fn paginas(&self, primeira: usize, quantas: usize) -> Result<Vec<Rc<Box<Quadro>>>, i64> {
        let total = self.tamanho.div_ceil(PAGINA);
        let fim = (primeira + quantas).min(total);
        *self.usado.borrow_mut() = proximo_uso();
        let mut k = primeira;
        while k < fim {
            if self.quadros.borrow()[k].is_some() {
                k += 1;
                continue;
            }
            // um trecho de paginas que faltam: um pedido ao disco
            let mut n = 1;
            while k + n < fim && self.quadros.borrow()[k + n].is_none() {
                n += 1;
            }
            let mut dados = alloc::vec![0u8; n * PAGINA];
            let valido = (self.tamanho - k * PAGINA).min(n * PAGINA);
            armazenamento::ler_trecho(&self.cadeia, k * PAGINA, &mut dados[..valido]).map_err(errno)?;
            for (j, pedaco) in dados.chunks_exact(PAGINA).enumerate() {
                let quadro = memoria::quadro_novo();
                // quadro novo: ninguem mais o tem
                let mut q = quadro;
                Rc::get_mut(&mut q).ok_or(EIO)?.0.copy_from_slice(pedaco);
                self.quadros.borrow_mut()[k + j] = Some(q);
                carregados_mais(1);
            }
            k += n;
        }
        let quadros = self.quadros.borrow();
        Ok((primeira..fim).map(|k| quadros[k].clone().unwrap()).collect())
    }

    /// Le ate `destino.len()` bytes a partir de `posicao`. Devolve quantos.
    pub fn ler(&self, posicao: usize, destino: &mut [u8]) -> Result<usize, i64> {
        if posicao >= self.tamanho {
            return Ok(0);
        }
        let n = destino.len().min(self.tamanho - posicao);
        let primeira = posicao / PAGINA;
        let ultima = (posicao + n).div_ceil(PAGINA);
        // le um pouco alem do pedido: o proximo read ja acha no cache
        let quadros = self.paginas(primeira, (ultima - primeira).max(ADIANTAR))?;
        let mut feito = 0;
        while feito < n {
            let atual = posicao + feito;
            let dentro = atual % PAGINA;
            let parte = (PAGINA - dentro).min(n - feito);
            destino[feito..feito + parte].copy_from_slice(&quadros[atual / PAGINA - primeira].0[dentro..dentro + parte]);
            feito += parte;
        }
        Ok(n)
    }

    /// O arquivo inteiro (para quem precisa de tudo, como o carregador de ELF).
    pub fn tudo(&self) -> Result<Vec<u8>, i64> {
        let mut dados = alloc::vec![0u8; self.tamanho];
        self.ler(0, &mut dados)?;
        Ok(dados)
    }

    fn carregados(&self) -> usize {
        self.quadros.borrow().iter().filter(|q| q.is_some()).count()
    }
}

struct Cache {
    arquivos: BTreeMap<String, Rc<Arquivo>>,
    relogio: u64,
    carregados: usize,
}

struct Global(UnsafeCell<Cache>);

// So o laco principal mexe (como a tabela de processos).
unsafe impl Sync for Global {}

static CACHE: Global = Global(UnsafeCell::new(Cache { arquivos: BTreeMap::new(), relogio: 0, carregados: 0 }));

fn cache() -> &'static mut Cache {
    unsafe { &mut *CACHE.0.get() }
}

fn proximo_uso() -> u64 {
    let c = cache();
    c.relogio += 1;
    c.relogio
}

fn carregados_mais(n: usize) {
    let c = cache();
    c.carregados += n;
    if c.carregados > LIMITE {
        soltar_velhos();
    }
}

/// Tira do cache os arquivos usados ha mais tempo ate ficar abaixo de 3/4
/// do limite.
fn soltar_velhos() {
    let c = cache();
    while c.carregados > LIMITE * 3 / 4 && c.arquivos.len() > 1 {
        let Some(velho) = c.arquivos.iter().min_by_key(|(_, a)| *a.usado.borrow()).map(|(k, _)| k.clone()) else { break };
        if let Some(a) = c.arquivos.remove(&velho) {
            c.carregados -= a.carregados();
        }
    }
}

fn chave(caminho: &str) -> String {
    // o FAT32 nao diferencia maiusculas
    caminho.to_lowercase()
}

/// O arquivo no cache (sem ler nada ainda alem da lista de clusters).
pub fn abrir(caminho: &str) -> Result<Rc<Arquivo>, i64> {
    let k = chave(caminho);
    if let Some(a) = cache().arquivos.get(&k) {
        return Ok(a.clone());
    }
    let (tamanho, cadeia) = armazenamento::abrir_leitura(caminho).map_err(errno)?;
    let paginas = tamanho.div_ceil(PAGINA);
    let a = Rc::new(Arquivo {
        tamanho,
        cadeia,
        quadros: RefCell::new(alloc::vec![None; paginas]),
        usado: RefCell::new(proximo_uso()),
    });
    cache().arquivos.insert(k, a.clone());
    Ok(a)
}

/// O arquivo mudou no disco: sai do cache.
pub fn esquecer(caminho: &str) {
    let c = cache();
    if let Some(a) = c.arquivos.remove(&chave(caminho)) {
        c.carregados -= a.carregados();
    }
}

/// (arquivos, KB na memoria) para o comando "memoria".
pub fn resumo() -> (usize, usize) {
    let c = cache();
    (c.arquivos.len(), c.carregados * PAGINA / 1024)
}
