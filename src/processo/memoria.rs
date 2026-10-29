#![allow(dead_code)] // partes usadas pelo carregador de ELF e pelo Linux (passos 3 e 4)
//! Espaco de enderecos de um processo: as tabelas de paginas dele e as
//! paginas de RAM (quadros) que o programa usa.
//!
//! A raiz (L1) e copia da do kernel; so a entrada 0 (o primeiro GB) troca
//! por uma L2 propria, que repete os perifericos do kernel (so EL1) e
//! ganha as tabelas L3 com as paginas do programa, de 0x1000 ate
//! FIM_USUARIO. O resto da memoria continua sendo so do kernel.
//!
//! Os quadros vem do heap (mapa identidade: o endereco do quadro no
//! kernel e o fisico). O kernel le e escreve a memoria do programa pelos
//! quadros, entao nao precisa estar com as tabelas do processo ligadas.
//!
//! Copy-on-write: no fork o filho divide os quadros com o pai (Rc). As
//! paginas que so se le (o codigo) ficam divididas para sempre; as que se
//! escreve ficam so de leitura nas tabelas dos dois, e a primeira escrita
//! de um deles (falha de permissao, `resolver_cow`) ganha uma copia so
//! dele. Um `sh` que faz fork para rodar um comando nao copia mais os
//! megabytes do programa.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::rc::Rc;

use crate::mmu;

pub const PAGINA: usize = 4096;
/// O programa mora abaixo dos perifericos (0x08000000): 128 MB.
pub const FIM_USUARIO: usize = 0x0800_0000;
/// A pagina 0 nunca e mapeada: ponteiro nulo da erro.
pub const INICIO_USUARIO: usize = PAGINA;
/// Cada tabela L3 cobre 2 MB.
const BLOCO: usize = 2 * 1024 * 1024;

#[repr(C, align(4096))]
struct Tabela([u64; mmu::ENTRADAS_TABELA]);

#[repr(C, align(4096))]
pub struct Quadro(pub [u8; PAGINA]);

/// Uma pagina do programa: o quadro (talvez dividido com outro processo,
/// depois de um fork) e os atributos que estao na tabela agora.
struct Pagina {
    /// Rc de Box: o Rc fica pequeno e o quadro continua alinhado em 4 KB.
    quadro: Rc<Box<Quadro>>,
    atributos: u64,
}

impl Pagina {
    fn fisico(&self) -> u64 {
        &**self.quadro as *const Quadro as u64
    }

    fn dividida(&self) -> bool {
        Rc::strong_count(&self.quadro) > 1
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permissao {
    /// Le e executa.
    Codigo,
    /// Le e escreve.
    Dados,
    /// So le.
    Leitura,
    /// Le, escreve e executa.
    Tudo,
}

impl Permissao {
    fn atributos(self) -> u64 {
        match self {
            Permissao::Codigo => mmu::USUARIO_CODIGO,
            Permissao::Dados => mmu::USUARIO_DADOS,
            Permissao::Leitura => mmu::USUARIO_LEITURA,
            Permissao::Tudo => mmu::USUARIO_TUDO,
        }
    }

    fn escreve(self) -> bool {
        matches!(self, Permissao::Dados | Permissao::Tudo)
    }

    /// Os atributos de quando a pagina esta dividida (copy-on-write): a
    /// mesma coisa, sem escrita.
    fn atributos_divididos(self) -> u64 {
        match self {
            Permissao::Dados => mmu::USUARIO_LEITURA,
            Permissao::Tudo => mmu::USUARIO_CODIGO,
            p => p.atributos(),
        }
    }

    /// Bits de protecao do Linux (PROT_READ 1, PROT_WRITE 2, PROT_EXEC 4).
    pub fn de_prot(prot: u64) -> Permissao {
        match (prot & 2 != 0, prot & 4 != 0) {
            (true, true) => Permissao::Tudo,
            (true, false) => Permissao::Dados,
            (false, true) => Permissao::Codigo,
            (false, false) => Permissao::Leitura,
        }
    }
}

pub struct Espaco {
    raiz: Box<Tabela>,
    l2: Box<Tabela>,
    /// Tabelas L3 por indice na L2 (uma a cada 2 MB usados).
    l3: BTreeMap<usize, Box<Tabela>>,
    /// Paginas por numero (endereco / 4096).
    paginas: BTreeMap<usize, Pagina>,
    permissoes: BTreeMap<usize, Permissao>,
    pub asid: u16,
    /// Comeco e fim do heap do brk (as threads dividem com a memoria).
    pub brk_inicio: usize,
    pub brk: usize,
}

/// Endereco fora da area do programa ou sem pagina.
#[derive(Debug)]
pub struct Fora;

impl Espaco {
    pub fn novo(asid: u16) -> Espaco {
        let mut raiz = Box::new(Tabela(mmu::raiz_kernel()));
        let l2 = Box::new(Tabela(mmu::l2_kernel(0)));
        raiz.0[0] = &*l2 as *const Tabela as u64 | mmu::DESCRITOR_TABELA;
        Espaco { raiz, l2, l3: BTreeMap::new(), paginas: BTreeMap::new(), permissoes: BTreeMap::new(), asid, brk_inicio: 0, brk: 0 }
    }

    /// Endereco da raiz (vai no TTBR0).
    pub fn raiz(&self) -> u64 {
        &*self.raiz as *const Tabela as u64
    }

    /// Bytes de RAM que o programa usa (paginas e tabelas).
    pub fn bytes(&self) -> usize {
        (self.paginas.len() + self.l3.len() + 2) * PAGINA
    }

    pub fn mapeada(&self, endereco: usize) -> bool {
        self.paginas.contains_key(&(endereco / PAGINA))
    }

    /// Poe uma pagina zerada em `endereco` (ou so troca a permissao, se
    /// ja existe).
    pub fn mapear(&mut self, endereco: usize, permissao: Permissao) -> Result<(), Fora> {
        let endereco = endereco & !(PAGINA - 1);
        if !(INICIO_USUARIO..FIM_USUARIO).contains(&endereco) {
            return Err(Fora);
        }
        let numero = endereco / PAGINA;
        if self.paginas.contains_key(&numero) {
            self.proteger(endereco, permissao);
            return Ok(());
        }
        let quadro: Box<Quadro> = unsafe { Box::new_zeroed().assume_init() };
        let pagina = Pagina { quadro: Rc::new(quadro), atributos: permissao.atributos() };
        *self.entrada(endereco) = pagina.fisico() | pagina.atributos | mmu::DESCRITOR_PAGINA;
        self.paginas.insert(numero, pagina);
        self.permissoes.insert(numero, permissao);
        Ok(())
    }

    /// Mapeia todas as paginas de [inicio, fim).
    pub fn mapear_faixa(&mut self, inicio: usize, fim: usize, permissao: Permissao) -> Result<(), Fora> {
        let mut endereco = inicio & !(PAGINA - 1);
        while endereco < fim {
            self.mapear(endereco, permissao)?;
            endereco += PAGINA;
        }
        Ok(())
    }

    /// Tira a pagina (o quadro volta para o heap).
    pub fn desmapear(&mut self, endereco: usize) {
        let numero = endereco / PAGINA;
        if self.paginas.remove(&numero).is_some() {
            self.permissoes.remove(&numero);
            *self.entrada(endereco & !(PAGINA - 1)) = 0;
            mmu::esquecer_asid(self.asid);
        }
    }

    /// Troca a permissao de uma pagina que ja existe.
    pub fn proteger(&mut self, endereco: usize, permissao: Permissao) {
        let numero = endereco / PAGINA;
        let Some(pagina) = self.paginas.get_mut(&numero) else { return };
        self.permissoes.insert(numero, permissao);
        // dividida com outro processo: a escrita espera a falha e a copia
        let atributos = if pagina.dividida() { permissao.atributos_divididos() } else { permissao.atributos() };
        if pagina.atributos == atributos {
            return;
        }
        pagina.atributos = atributos;
        let fisico = pagina.fisico();
        *self.entrada(endereco & !(PAGINA - 1)) = fisico | atributos | mmu::DESCRITOR_PAGINA;
        mmu::esquecer_asid(self.asid);
    }

    /// A pagina `numero` passa a ter um quadro so dela (copia se estava
    /// dividida) e os atributos da permissao de verdade.
    fn privatizar(&mut self, numero: usize) {
        let Some(&permissao) = self.permissoes.get(&numero) else { return };
        let Some(pagina) = self.paginas.get_mut(&numero) else { return };
        let mut mudou = false;
        if pagina.dividida() {
            let mut copia: Box<Quadro> = unsafe { Box::new_zeroed().assume_init() };
            copia.0.copy_from_slice(&pagina.quadro.0);
            if matches!(permissao, Permissao::Codigo | Permissao::Tudo) {
                mmu::sincronizar_codigo(copia.0.as_ptr() as usize, PAGINA);
            }
            pagina.quadro = Rc::new(copia);
            mudou = true;
        }
        let atributos = permissao.atributos();
        if mudou || pagina.atributos != atributos {
            pagina.atributos = atributos;
            let valor = pagina.fisico() | atributos | mmu::DESCRITOR_PAGINA;
            *self.entrada(numero * PAGINA) = valor;
            mmu::esquecer_asid(self.asid);
        }
    }

    /// Falha de escrita em `endereco`: se a pagina e de escrita e so esta
    /// protegida pelo copy-on-write, ganha a copia propria e o programa
    /// repete a instrucao (true). false = a falha e de verdade.
    pub fn resolver_cow(&mut self, endereco: usize) -> bool {
        let numero = endereco / PAGINA;
        let Some(&permissao) = self.permissoes.get(&numero) else { return false };
        let Some(pagina) = self.paginas.get(&numero) else { return false };
        if !permissao.escreve() || (!pagina.dividida() && pagina.atributos == permissao.atributos()) {
            return false;
        }
        self.privatizar(numero);
        true
    }

    /// A entrada L3 de `endereco` (cria a tabela L3 se precisar).
    fn entrada(&mut self, endereco: usize) -> &mut u64 {
        let i2 = endereco / BLOCO;
        let l3 = self.l3.entry(i2).or_insert_with(|| Box::new(Tabela([0; mmu::ENTRADAS_TABELA])));
        let descritor = &**l3 as *const Tabela as u64 | mmu::DESCRITOR_TABELA;
        self.l2.0[i2] = descritor;
        &mut l3.0[endereco / PAGINA % mmu::ENTRADAS_TABELA]
    }

    /// Le `destino.len()` bytes do programa a partir de `endereco`.
    pub fn ler(&self, endereco: usize, destino: &mut [u8]) -> Result<(), Fora> {
        let mut feito = 0;
        while feito < destino.len() {
            let atual = endereco.checked_add(feito).ok_or(Fora)?;
            let pagina = self.paginas.get(&(atual / PAGINA)).ok_or(Fora)?;
            let dentro = atual % PAGINA;
            let n = (PAGINA - dentro).min(destino.len() - feito);
            destino[feito..feito + n].copy_from_slice(&pagina.quadro.0[dentro..dentro + n]);
            feito += n;
        }
        Ok(())
    }

    /// Escreve no programa, so em paginas que ele mesmo pode escrever.
    pub fn escrever(&mut self, endereco: usize, dados: &[u8]) -> Result<(), Fora> {
        self.escrever_com(endereco, dados, false)
    }

    /// Escreve ate em paginas so de leitura (o carregador poe o codigo).
    pub fn escrever_forcado(&mut self, endereco: usize, dados: &[u8]) -> Result<(), Fora> {
        self.escrever_com(endereco, dados, true)
    }

    fn escrever_com(&mut self, endereco: usize, dados: &[u8], forcado: bool) -> Result<(), Fora> {
        // confere tudo antes, para nao escrever pela metade
        let mut atual = endereco & !(PAGINA - 1);
        let fim = endereco.checked_add(dados.len()).ok_or(Fora)?;
        while atual < fim {
            let permissao = self.permissoes.get(&(atual / PAGINA)).ok_or(Fora)?;
            if !forcado && !permissao.escreve() {
                return Err(Fora);
            }
            atual += PAGINA;
        }
        let mut feito = 0;
        while feito < dados.len() {
            let atual = endereco + feito;
            let numero = atual / PAGINA;
            // o kernel escreve pelo quadro: ele nao pode ser o do outro processo
            if self.paginas.get(&numero).ok_or(Fora)?.dividida() {
                self.privatizar(numero);
            }
            let pagina = self.paginas.get_mut(&numero).ok_or(Fora)?;
            let quadro = Rc::get_mut(&mut pagina.quadro).ok_or(Fora)?;
            let dentro = atual % PAGINA;
            let n = (PAGINA - dentro).min(dados.len() - feito);
            quadro.0[dentro..dentro + n].copy_from_slice(&dados[feito..feito + n]);
            if forcado && matches!(self.permissoes.get(&numero), Some(Permissao::Codigo | Permissao::Tudo)) {
                let quadro = &pagina.quadro.0[dentro] as *const u8 as usize;
                mmu::sincronizar_codigo(quadro, n);
            }
            feito += n;
        }
        Ok(())
    }

    /// Le um texto terminado em 0 (no maximo `maximo` bytes).
    pub fn ler_texto(&self, endereco: usize, maximo: usize) -> Result<alloc::vec::Vec<u8>, Fora> {
        let mut texto = alloc::vec::Vec::new();
        let mut byte = [0u8];
        while texto.len() < maximo {
            self.ler(endereco + texto.len(), &mut byte)?;
            if byte[0] == 0 {
                return Ok(texto);
            }
            texto.push(byte[0]);
        }
        Err(Fora)
    }

    /// A primeira faixa livre de `tamanho` bytes a partir de `desde`.
    pub fn achar_livre(&self, desde: usize, tamanho: usize) -> Option<usize> {
        let paginas = tamanho.div_ceil(PAGINA);
        let mut inicio = desde.max(INICIO_USUARIO) / PAGINA;
        let fim = FIM_USUARIO / PAGINA;
        while inicio + paginas <= fim {
            match (inicio..inicio + paginas).find(|n| self.paginas.contains_key(n)) {
                None => return Some(inicio * PAGINA),
                Some(ocupada) => inicio = ocupada + 1,
            }
        }
        None
    }

    /// O espaco do filho num fork: os mesmos quadros (copy-on-write). As
    /// paginas de escrita ficam so de leitura no pai e no filho ate um deles
    /// escrever.
    pub fn duplicar(&mut self, asid: u16) -> Espaco {
        let mut novo = Espaco::novo(asid);
        novo.brk_inicio = self.brk_inicio;
        novo.brk = self.brk;
        let numeros: alloc::vec::Vec<usize> = self.paginas.keys().copied().collect();
        let mut pai_mudou = false;
        for numero in numeros {
            let permissao = self.permissoes[&numero];
            let endereco = numero * PAGINA;
            let atributos = permissao.atributos_divididos();
            let pagina = self.paginas.get_mut(&numero).expect("pagina sem quadro");
            let quadro = pagina.quadro.clone();
            if pagina.atributos != atributos {
                pagina.atributos = atributos;
                let valor = pagina.fisico() | atributos | mmu::DESCRITOR_PAGINA;
                *self.entrada(endereco) = valor;
                pai_mudou = true;
            }
            let copia = Pagina { quadro, atributos };
            *novo.entrada(endereco) = copia.fisico() | atributos | mmu::DESCRITOR_PAGINA;
            novo.paginas.insert(numero, copia);
            novo.permissoes.insert(numero, permissao);
        }
        if pai_mudou {
            // o TLB do pai ainda pode achar que as paginas sao de escrita
            mmu::esquecer_asid(self.asid);
        }
        novo
    }
}

impl Drop for Espaco {
    /// O TLB nao pode guardar traducoes para quadros que voltam ao heap.
    fn drop(&mut self) {
        mmu::esquecer_asid(self.asid);
    }
}
