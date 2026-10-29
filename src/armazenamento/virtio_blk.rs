//! Driver do virtio-blk (spec virtio 1.2, secao 5.2) sobre virtio-mmio.
//!
//! Uma fila so (requestq, numero 0). Cada pedido e uma cadeia de 3
//! descritores:
//!   cabecalho {tipo u32, reservado u32, setor u64} (o dispositivo le)
//!   -> dados (le na escrita, escreve na leitura)
//!   -> status u8 (o dispositivo escreve): 0 ok, 1 erro de E/S, 2 nao suportado
//! O FLUSH nao tem a parte de dados.
//!
//! Um pedido por vez. Os dados vao direto do/para o buffer de quem chamou
//! (mapa identidade: endereco virtual = fisico, e a RAM e coerente para o
//! virtio). Se o dispositivo nao responder, ele e resetado antes de
//! devolvermos o erro: assim nunca escreve num buffer que ja foi liberado.
//!
//! Espera por polling do used ring: um giro curto (o QEMU costuma
//! terminar em dezenas de microssegundos) e depois irq::cochilar ate o
//! proximo tique. A interrupcao do disco nao e ligada no GIC.
//!
//! Memoria (uma pagina de 4 KB do heap, nunca devolvida enquanto o
//! dispositivo vive):
//!   0x000..0x800  a fila
//!   0x800         cabecalho do pedido (16 bytes)
//!   0x810         status (1 byte)

use alloc::alloc::{Layout, alloc_zeroed, dealloc};
use alloc::string::String;
use core::hint::spin_loop;
use core::ptr::{read_volatile, write_volatile};

use super::{Disco, Erro};
use crate::virtio::virtio_mmio::VirtioMmio;
use crate::virtio::virtqueue::{self, Parte, Virtqueue};
use crate::{cpu, irq};

// Tipos de pedido
const T_LER: u32 = 0;
const T_ESCREVER: u32 = 1;
const T_FLUSH: u32 = 4;

// Features que entendemos
const F_SO_LEITURA: u64 = 1 << 5;
const F_FLUSH: u64 = 1 << 9;

const STATUS_OK: u8 = 0;
/// Valor posto no status antes do pedido (o dispositivo sobrescreve).
const STATUS_PENDENTE: u8 = 0xFF;

const FILA: u32 = 0;
/// So um pedido por vez (3 descritores): 16 sobra.
const TAMANHO_FILA: u32 = 16;
const MEMORIA: usize = 4096;
const CABECALHO: usize = 0x800;
const STATUS: usize = 0x810;
const _: () = assert!(virtqueue::memoria_necessaria(TAMANHO_FILA as u16) <= CABECALHO);

const SETOR: usize = 512;
/// Maior pedido de uma vez (pedidos maiores sao divididos).
const MAX_BYTES_PEDIDO: usize = 64 * 1024;
/// Giro antes de dormir e tempo maximo de um pedido.
const GIRO_US: u64 = 300;
const TEMPO_LIMITE_MS: u64 = 5000;

fn escrever32(endereco: usize, valor: u32) {
    unsafe { write_volatile(endereco as *mut u32, valor) }
}

fn escrever64(endereco: usize, valor: u64) {
    unsafe { write_volatile(endereco as *mut u64, valor) }
}

/// Maior potencia de 2 <= min(maximo, TAMANHO_FILA).
fn tamanho_de_fila(maximo: u32) -> Option<u16> {
    let t = maximo.min(TAMANHO_FILA);
    (t != 0).then(|| (1u32 << (31 - t.leading_zeros())) as u16)
}

pub struct VirtioBlk {
    disp: VirtioMmio,
    fila: Virtqueue,
    memoria: usize,
    setores: u64,
    nome: String,
    somente_leitura: bool,
    tem_flush: bool,
    /// O dispositivo foi resetado depois de um erro: nada mais funciona.
    falhou: bool,
}

impl VirtioBlk {
    /// Inicializa o dispositivo: features, fila, DRIVER_OK e capacidade.
    pub fn iniciar(disp: VirtioMmio, nome: String) -> Result<VirtioBlk, Erro> {
        let features = disp.iniciar(F_SO_LEITURA | F_FLUSH).map_err(|_| Erro::Disco)?;
        let layout = Layout::from_size_align(MEMORIA, MEMORIA).map_err(|_| Erro::Disco)?;
        let memoria = unsafe { alloc_zeroed(layout) } as usize;
        if memoria == 0 {
            disp.falhar();
            return Err(Erro::Disco);
        }
        let fila = match Self::montar_fila(&disp, memoria) {
            Some(fila) => fila,
            None => {
                // a fila nao ficou pronta: o dispositivo nunca viu a memoria
                disp.falhar();
                disp.resetar();
                unsafe { dealloc(memoria as *mut u8, layout) };
                return Err(Erro::Disco);
            }
        };
        disp.ativar();
        // capacity: u64 no comeco da configuracao, em setores de 512
        let setores = disp.ler_config_u32(0) as u64 | (disp.ler_config_u32(4) as u64) << 32;
        Ok(VirtioBlk {
            disp,
            fila,
            memoria,
            setores,
            nome,
            somente_leitura: features & F_SO_LEITURA != 0,
            tem_flush: features & F_FLUSH != 0,
            falhou: false,
        })
    }

    fn montar_fila(disp: &VirtioMmio, memoria: usize) -> Option<Virtqueue> {
        let tamanho = tamanho_de_fila(disp.tamanho_max_fila(FILA))?;
        let fila = unsafe { Virtqueue::nova(FILA, tamanho, memoria).ok()? };
        fila.instalar(disp).ok()?;
        Some(fila)
    }

    /// Um pedido completo: cabecalho, dados opcionais, status; espera o fim.
    fn pedido(&mut self, tipo: u32, setor: u64, dados: Option<Parte>) -> Result<(), Erro> {
        if self.falhou {
            return Err(Erro::Disco);
        }
        let (cabecalho, status) = (self.memoria + CABECALHO, self.memoria + STATUS);
        escrever32(cabecalho, tipo);
        escrever32(cabecalho + 4, 0);
        escrever64(cabecalho + 8, setor);
        unsafe { write_volatile(status as *mut u8, STATUS_PENDENTE) };
        virtqueue::para_dispositivo(cabecalho, STATUS + 1 - CABECALHO);
        if let Some(parte) = dados {
            virtqueue::para_dispositivo(parte.endereco, parte.tamanho as usize);
        }
        let cab = Parte { endereco: cabecalho, tamanho: 16, dispositivo_escreve: false };
        let fim = Parte { endereco: status, tamanho: 1, dispositivo_escreve: true };
        let cabeca = match dados {
            Some(parte) => self.fila.adicionar(&[cab, parte, fim]),
            None => self.fila.adicionar(&[cab, fim]),
        }
        .ok_or(Erro::Disco)?;
        self.disp.notificar(FILA);
        self.esperar(cabeca)?;
        if let Some(parte) = dados {
            virtqueue::do_dispositivo(parte.endereco, parte.tamanho as usize);
        }
        virtqueue::do_dispositivo(status, 1);
        // a interrupcao nao esta ligada no GIC, mas o bit fica pendente
        self.disp.confirmar_interrupcao();
        match unsafe { read_volatile(status as *const u8) } {
            STATUS_OK => Ok(()),
            _ => Err(Erro::Disco),
        }
    }

    /// Polling do used ring ate voltar a cadeia `cabeca`.
    fn esperar(&mut self, cabeca: u16) -> Result<(), Erro> {
        let frequencia = cpu::frequencia_timer();
        let giro = frequencia * GIRO_US / 1_000_000;
        let limite = frequencia * TEMPO_LIMITE_MS / 1000;
        let inicio = cpu::contador();
        loop {
            while let Some(usado) = self.fila.colher() {
                if usado.cabeca == cabeca {
                    return Ok(());
                }
            }
            if self.disp.precisa_reset() {
                return self.desistir();
            }
            let passou = cpu::contador().wrapping_sub(inicio);
            if passou > limite {
                return self.desistir();
            }
            if passou < giro {
                spin_loop();
            } else {
                irq::cochilar();
            }
        }
    }

    /// Reset: o dispositivo esquece a fila e para de usar os buffers.
    fn desistir(&mut self) -> Result<(), Erro> {
        self.disp.resetar();
        self.falhou = true;
        Err(Erro::Disco)
    }

    /// Confere alinhamento e limites e divide em pedidos de ate 64 KB.
    fn transferir(&mut self, tipo: u32, setor: u64, endereco: usize, bytes: usize) -> Result<(), Erro> {
        let setores = (bytes / SETOR) as u64;
        if bytes % SETOR != 0 || setor.checked_add(setores).is_none_or(|fim| fim > self.setores) {
            return Err(Erro::Disco);
        }
        let mut feito = 0;
        while feito < bytes {
            let n = (bytes - feito).min(MAX_BYTES_PEDIDO);
            let parte = Parte { endereco: endereco + feito, tamanho: n as u32, dispositivo_escreve: tipo == T_LER };
            self.pedido(tipo, setor + (feito / SETOR) as u64, Some(parte))?;
            feito += n;
        }
        Ok(())
    }
}

impl Disco for VirtioBlk {
    fn nome(&self) -> &str {
        &self.nome
    }

    fn setores(&self) -> u64 {
        self.setores
    }

    fn ler(&mut self, setor: u64, buffer: &mut [u8]) -> Result<(), Erro> {
        self.transferir(T_LER, setor, buffer.as_mut_ptr() as usize, buffer.len())
    }

    fn escrever(&mut self, setor: u64, dados: &[u8]) -> Result<(), Erro> {
        if self.somente_leitura {
            return Err(Erro::Disco);
        }
        // o dispositivo so le este buffer
        self.transferir(T_ESCREVER, setor, dados.as_ptr() as usize, dados.len())
    }

    fn sincronizar(&mut self) -> Result<(), Erro> {
        if !self.tem_flush || self.somente_leitura {
            return Ok(());
        }
        self.pedido(T_FLUSH, 0, None)
    }
}

impl Drop for VirtioBlk {
    /// Reset antes de devolver a memoria: o dispositivo para de usar a fila.
    fn drop(&mut self) {
        self.disp.resetar();
        let layout = Layout::from_size_align(MEMORIA, MEMORIA).expect("layout da fila");
        unsafe { dealloc(self.memoria as *mut u8, layout) };
    }
}
