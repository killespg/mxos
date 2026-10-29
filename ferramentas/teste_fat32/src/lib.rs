//! Monta o src/armazenamento/fat32.rs do kernel no PC. Os tipos abaixo sao
//! copia dos de src/armazenamento/mod.rs (mantenha iguais).

extern crate alloc;

use std::cell::RefCell;
use std::rc::Rc;

#[path = "../../../src/armazenamento/fat32.rs"]
pub mod fat32;

pub trait Disco {
    fn nome(&self) -> &str;
    fn setores(&self) -> u64;
    fn ler(&mut self, setor: u64, buffer: &mut [u8]) -> Result<(), Erro>;
    fn escrever(&mut self, setor: u64, dados: &[u8]) -> Result<(), Erro>;
    fn sincronizar(&mut self) -> Result<(), Erro> {
        Ok(())
    }
}

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
}

#[derive(Clone, Debug)]
pub struct ItemPasta {
    pub nome: String,
    pub pasta: bool,
    pub tamanho: u64,
}

/// Contadores de pedidos ao disco.
#[derive(Default, Clone, Copy, Debug)]
pub struct Pedidos {
    pub leituras: u64,
    pub escritas: u64,
    pub setores_lidos: u64,
    pub setores_escritos: u64,
}

/// Disco na memoria; o teste guarda outra referencia para salvar a imagem.
pub struct DiscoMemoria {
    pub dados: Rc<RefCell<Vec<u8>>>,
    pub pedidos: Rc<RefCell<Pedidos>>,
}

impl DiscoMemoria {
    pub fn novo(dados: Rc<RefCell<Vec<u8>>>) -> (Box<dyn Disco>, Rc<RefCell<Pedidos>>) {
        let pedidos = Rc::new(RefCell::new(Pedidos::default()));
        (Box::new(DiscoMemoria { dados, pedidos: pedidos.clone() }), pedidos)
    }
}

impl Disco for DiscoMemoria {
    fn nome(&self) -> &str {
        "memoria"
    }

    fn setores(&self) -> u64 {
        (self.dados.borrow().len() / 512) as u64
    }

    fn ler(&mut self, setor: u64, buffer: &mut [u8]) -> Result<(), Erro> {
        assert!(buffer.len() % 512 == 0 && !buffer.is_empty(), "buffer de {} bytes", buffer.len());
        let inicio = setor as usize * 512;
        let dados = self.dados.borrow();
        if inicio + buffer.len() > dados.len() {
            return Err(Erro::Disco);
        }
        buffer.copy_from_slice(&dados[inicio..inicio + buffer.len()]);
        let mut p = self.pedidos.borrow_mut();
        p.leituras += 1;
        p.setores_lidos += (buffer.len() / 512) as u64;
        Ok(())
    }

    fn escrever(&mut self, setor: u64, buffer: &[u8]) -> Result<(), Erro> {
        assert!(buffer.len() % 512 == 0 && !buffer.is_empty(), "buffer de {} bytes", buffer.len());
        let inicio = setor as usize * 512;
        let mut dados = self.dados.borrow_mut();
        if inicio + buffer.len() > dados.len() {
            return Err(Erro::Disco);
        }
        dados[inicio..inicio + buffer.len()].copy_from_slice(buffer);
        let mut p = self.pedidos.borrow_mut();
        p.escritas += 1;
        p.setores_escritos += (buffer.len() / 512) as u64;
        Ok(())
    }
}
