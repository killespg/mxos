//! Split virtqueue (spec virtio 1.2, secao 2.7) sobre memoria do chamador.
//!
//! Tres pedacos na mesma regiao, um atras do outro:
//!   - tabela de descritores: 16 bytes por descritor {addr u64, len u32, flags u16, next u16}
//!   - avail (driver -> dispositivo): {flags u16, idx u16, ring[N] u16}
//!   - used (dispositivo -> driver): {flags u16, idx u16, ring[N] {id u32, len u32}}
//!
//! Sem EVENT_IDX (nao negociado), entao os campos used_event/avail_event
//! nao existem e o "polling" e so comparar used.idx com o ultimo visto.
//!
//! Os descritores livres formam uma lista encadeada pelo proprio campo
//! `next` (o mesmo truque do Linux): nao precisa de memoria extra.
//!
//! Cache: no QEMU (e em virtio de verdade) o dispositivo e coerente com o
//! cache, entao basta a barreira. Num dispositivo DMA nao coerente seria
//! preciso "dc cvac" nos buffers antes de entregar e "dc ivac" antes de ler
//! a resposta: os ganchos sao `para_dispositivo` e `do_dispositivo`.

use core::ptr::{read_volatile, write_volatile};

use super::virtio_mmio::{Erro, VirtioMmio, barreira};

const DESC_PROXIMO: u16 = 1;
const DESC_ESCRITA: u16 = 2;

const TAMANHO_DESC: usize = 16;

/// Um pedaco de uma cadeia de buffers.
#[derive(Clone, Copy)]
pub struct Parte {
    pub endereco: usize,
    pub tamanho: u32,
    /// true = o dispositivo escreve aqui (resposta); false = so le (pedido).
    pub dispositivo_escreve: bool,
}

/// Uma cadeia que o dispositivo terminou de usar.
#[derive(Clone, Copy)]
pub struct Usado {
    /// Indice do primeiro descritor da cadeia (o que `adicionar` devolveu).
    pub cabeca: u16,
    /// Quantos bytes o dispositivo escreveu nas partes de escrita.
    pub tamanho: u32,
}

/// Offset do avail dentro da regiao (logo depois dos descritores).
const fn offset_avail(tamanho: u16) -> usize {
    TAMANHO_DESC * tamanho as usize
}

/// Offset do used: depois do avail, alinhado em 16 (a spec pede 4).
const fn offset_used(tamanho: u16) -> usize {
    let fim_avail = offset_avail(tamanho) + 6 + 2 * tamanho as usize;
    (fim_avail + 15) & !15
}

/// Bytes de memoria que uma fila de `tamanho` descritores precisa.
pub const fn memoria_necessaria(tamanho: u16) -> usize {
    offset_used(tamanho) + 6 + 8 * tamanho as usize
}

/// Gancho de cache antes de entregar um buffer ao dispositivo.
/// Hoje so barreira: virtio e coerente. Com DMA nao coerente: "dc cvac".
pub fn para_dispositivo(_endereco: usize, _tamanho: usize) {
    barreira();
}

/// Gancho de cache antes de ler o que o dispositivo escreveu.
/// Hoje so barreira: virtio e coerente. Com DMA nao coerente: "dc ivac".
pub fn do_dispositivo(_endereco: usize, _tamanho: usize) {
    barreira();
}

fn ler16(endereco: usize) -> u16 {
    unsafe { read_volatile(endereco as *const u16) }
}

fn escrever16(endereco: usize, valor: u16) {
    unsafe { write_volatile(endereco as *mut u16, valor) }
}

fn ler32(endereco: usize) -> u32 {
    unsafe { read_volatile(endereco as *const u32) }
}

fn escrever32(endereco: usize, valor: u32) {
    unsafe { write_volatile(endereco as *mut u32, valor) }
}

fn escrever64(endereco: usize, valor: u64) {
    unsafe { write_volatile(endereco as *mut u64, valor) }
}

/// Zera `tamanho` bytes (multiplo de 4) com escritas volateis alinhadas.
fn zerar(endereco: usize, tamanho: usize) {
    for i in (0..tamanho).step_by(4) {
        escrever32(endereco + i, 0);
    }
}

pub struct Virtqueue {
    indice: u32,
    tamanho: u16,
    desc: usize,
    avail: usize,
    used: usize,
    /// Primeiro descritor livre e quantos estao livres.
    livre: u16,
    livres: u16,
    /// Proximo avail.idx a publicar e ultimo used.idx consumido.
    prox_avail: u16,
    ult_used: u16,
}

impl Virtqueue {
    /// Monta a fila numero `indice` em `memoria` (alinhada em 16, com pelo
    /// menos `memoria_necessaria(tamanho)` bytes, de uso exclusivo da fila).
    ///
    /// # Seguranca
    /// A memoria tem de ser RAM valida que ninguem mais usa enquanto a fila viver.
    pub unsafe fn nova(indice: u32, tamanho: u16, memoria: usize) -> Result<Virtqueue, Erro> {
        if tamanho == 0 || !tamanho.is_power_of_two() {
            return Err(Erro::TamanhoDeFilaInvalido(tamanho as u32));
        }
        if memoria & 0xF != 0 {
            return Err(Erro::MemoriaDesalinhada);
        }
        zerar(memoria, (memoria_necessaria(tamanho) + 3) & !3);
        let fila = Virtqueue {
            indice,
            tamanho,
            desc: memoria,
            avail: memoria + offset_avail(tamanho),
            used: memoria + offset_used(tamanho),
            livre: 0,
            livres: tamanho,
            prox_avail: 0,
            ult_used: 0,
        };
        fila.encadear_livres();
        Ok(fila)
    }

    /// Lista de livres inicial: descritor i aponta para i+1.
    fn encadear_livres(&self) {
        for i in 0..self.tamanho {
            self.escrever_proximo(i, i.wrapping_add(1) % self.tamanho);
        }
    }

    /// Entrega a fila ao dispositivo (antes do DRIVER_OK).
    pub fn instalar(&self, dispositivo: &VirtioMmio) -> Result<(), Erro> {
        dispositivo.configurar_fila(self.indice, self.tamanho as u32, self.desc, self.avail, self.used)
    }

    pub fn indice(&self) -> u32 {
        self.indice
    }

    pub fn tamanho(&self) -> u16 {
        self.tamanho
    }

    /// Descritores livres agora.
    pub fn livres(&self) -> u16 {
        self.livres
    }

    fn endereco_desc(&self, i: u16) -> usize {
        self.desc + TAMANHO_DESC * i as usize
    }

    fn flags(&self, i: u16) -> u16 {
        ler16(self.endereco_desc(i) + 12)
    }

    fn proximo(&self, i: u16) -> u16 {
        ler16(self.endereco_desc(i) + 14)
    }

    fn escrever_proximo(&self, i: u16, proximo: u16) {
        escrever16(self.endereco_desc(i) + 14, proximo);
    }

    /// Preenche endereco, tamanho e flags de um descritor (o next fica como esta).
    fn escrever_desc(&self, i: u16, parte: &Parte, continua: bool) {
        let d = self.endereco_desc(i);
        let mut flags = if continua { DESC_PROXIMO } else { 0 };
        if parte.dispositivo_escreve {
            flags |= DESC_ESCRITA;
        }
        escrever64(d, parte.endereco as u64);
        escrever32(d + 8, parte.tamanho);
        escrever16(d + 12, flags);
    }

    /// Coloca a cadeia no avail ring. Nao avisa o dispositivo: chame
    /// `VirtioMmio::notificar(fila.indice())` depois de uma ou mais adicoes.
    /// Devolve o descritor cabeca, ou None se faltar descritor livre.
    pub fn adicionar(&mut self, partes: &[Parte]) -> Option<u16> {
        let n = partes.len();
        if n == 0 || n > self.livres as usize {
            return None;
        }
        let cabeca = self.livre;
        let mut atual = cabeca;
        for (k, parte) in partes.iter().enumerate() {
            let continua = k + 1 < n;
            self.escrever_desc(atual, parte, continua);
            // o next de um livre ja aponta para o proximo livre: a cadeia
            // nova segue a lista de livres sem precisar reescrever o next
            atual = self.proximo(atual);
        }
        self.livre = atual;
        self.livres -= n as u16;
        self.publicar(cabeca);
        Some(cabeca)
    }

    /// avail.ring[idx % N] = cabeca, barreira, avail.idx += 1.
    fn publicar(&mut self, cabeca: u16) {
        let posicao = (self.prox_avail % self.tamanho) as usize;
        escrever16(self.avail + 4 + 2 * posicao, cabeca);
        barreira();
        self.prox_avail = self.prox_avail.wrapping_add(1);
        escrever16(self.avail + 2, self.prox_avail);
    }

    /// Proxima cadeia devolvida pelo dispositivo, se houver. Os descritores
    /// dela voltam para a lista de livres.
    pub fn colher(&mut self) -> Option<Usado> {
        let idx = ler16(self.used + 2);
        if idx == self.ult_used {
            return None;
        }
        // le o idx antes do elemento do ring (ordem de leitura)
        barreira();
        let posicao = (self.ult_used % self.tamanho) as usize;
        let elemento = self.used + 4 + 8 * posicao;
        let cabeca = ler32(elemento) as u16;
        let tamanho = ler32(elemento + 4);
        self.ult_used = self.ult_used.wrapping_add(1);
        self.liberar_cadeia(cabeca);
        Some(Usado { cabeca, tamanho })
    }

    /// Devolve a cadeia que comeca em `cabeca` para a lista de livres.
    fn liberar_cadeia(&mut self, cabeca: u16) {
        let mut ultimo = cabeca;
        let mut n = 1;
        while self.flags(ultimo) & DESC_PROXIMO != 0 && n < self.tamanho {
            ultimo = self.proximo(ultimo);
            n += 1;
        }
        self.escrever_proximo(ultimo, self.livre);
        self.livre = cabeca;
        self.livres += n;
    }

    /// Cadeias publicadas que o dispositivo ainda nao devolveu.
    pub fn pendentes(&self) -> u16 {
        self.prox_avail.wrapping_sub(self.ult_used)
    }
}
