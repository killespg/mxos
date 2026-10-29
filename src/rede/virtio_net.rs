//! Driver do virtio-net (spec virtio 1.2, secao 5.1) sobre virtio-mmio.
//!
//! So negociamos VIRTIO_F_VERSION_1 e VIRTIO_NET_F_MAC: sem checksum por
//! hardware, sem TSO, sem buffers mesclados. Duas filas:
//!   fila 0 (rx): buffers de 2 KB que o dispositivo preenche
//!   fila 1 (tx): um buffer por quadro que mandamos
//!
//! Todo pacote, nos dois sentidos, comeca com o virtio_net_hdr de 12 bytes
//! (com VERSION_1 o campo num_buffers sempre existe). Para enviar, tudo zero.
//! Cabecalho e quadro vao juntos num descritor so (VERSION_1 aceita
//! qualquer divisao dos buffers).
//!
//! Tudo por polling, sem interrupcao. Memoria de DMA (alinhada em 4096):
//!   0x0000  fila rx
//!   0x1000  fila tx
//!   0x2000  buffers rx (QUANTOS_RX x 2 KB)
//!   ...     buffers tx (QUANTOS_TX x 2 KB)

use alloc::alloc::{Layout, alloc_zeroed, dealloc};
use core::ptr::{copy_nonoverlapping, read_volatile};

use super::ErroRede;
use crate::virtio::virtio_mmio::{TIPO_REDE, VirtioMmio};
use crate::virtio::virtqueue::{self, Parte, Virtqueue};

const FILA_RX: u32 = 0;
const FILA_TX: u32 = 1;

/// O dispositivo informa o MAC no espaco de configuracao (offset 0).
const FEATURE_MAC: u64 = 1 << 5;

/// virtio_net_hdr com num_buffers.
pub const TAMANHO_CABECALHO: usize = 12;
/// Maior quadro Ethernet sem FCS (1500 de dados + 14 de cabecalho).
pub const MAX_QUADRO: usize = 1514;

const TAMANHO_BUFFER: usize = 2048;
const QUANTOS_RX: usize = 32;
const QUANTOS_TX: usize = 16;

const REGIAO_FILA: usize = 0x1000;
const BUFFERS_RX: usize = 2 * REGIAO_FILA;
const BUFFERS_TX: usize = BUFFERS_RX + QUANTOS_RX * TAMANHO_BUFFER;
const MEMORIA: usize = BUFFERS_TX + QUANTOS_TX * TAMANHO_BUFFER;

const _: () = assert!(virtqueue::memoria_necessaria(QUANTOS_RX as u16) <= REGIAO_FILA);
const _: () = assert!(TAMANHO_CABECALHO + MAX_QUADRO <= TAMANHO_BUFFER);

/// Offset do espaco de configuracao no slot virtio-mmio.
const R_CONFIG: usize = 0x100;

/// MAC usado se o dispositivo nao informar um (o padrao do QEMU).
const MAC_PADRAO: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];

pub struct VirtioNet {
    disp: VirtioMmio,
    rx: Virtqueue,
    tx: Virtqueue,
    memoria: usize,
    mac: [u8; 6],
    /// Descritor cabeca de cada buffer rx (todos ficam com o dispositivo).
    cabecas_rx: [u16; QUANTOS_RX],
    /// Descritor cabeca de cada buffer tx em uso (None = livre).
    cabecas_tx: [Option<u16>; QUANTOS_TX],
    /// Devolvemos buffers rx e ainda nao avisamos o dispositivo.
    avisar_rx: bool,
    /// Quadros perdidos por falta de buffer tx.
    pub descartados_tx: u64,
}

/// Le N bytes do espaco de configuracao, um a um (o MAC nao e
/// alinhado em 4 e pode terminar no fim do espaco).
fn ler_config_bytes<const N: usize>(disp: &VirtioMmio, offset: usize) -> [u8; N] {
    let mut bytes = [0u8; N];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = unsafe { read_volatile((disp.base() + R_CONFIG + offset + i) as *const u8) };
    }
    bytes
}

/// Maior potencia de 2 <= min(maximo, desejado).
fn tamanho_de_fila(maximo: u32, desejado: usize) -> Option<u16> {
    let t = maximo.min(desejado as u32);
    (t != 0).then(|| (1u32 << (31 - t.leading_zeros())) as u16)
}

impl VirtioNet {
    /// Acha o virtio-net nos slots do QEMU, negocia, monta as filas e
    /// entrega todos os buffers rx.
    pub fn iniciar() -> Result<VirtioNet, ErroRede> {
        let disp = VirtioMmio::procurar(TIPO_REDE).ok_or(ErroRede::SemRede)?;
        let features = disp.iniciar(FEATURE_MAC).map_err(|e| ErroRede::Protocolo(e.descricao()))?;
        let layout = Layout::from_size_align(MEMORIA, 4096).map_err(|_| ErroRede::Protocolo("layout de DMA"))?;
        let memoria = unsafe { alloc_zeroed(layout) } as usize;
        if memoria == 0 {
            disp.falhar();
            return Err(ErroRede::Protocolo("sem memória para o virtio-net"));
        }
        let (rx, tx) = match unsafe { Self::montar_filas(&disp, memoria) } {
            Ok(filas) => filas,
            Err(e) => {
                // o dispositivo pode ter visto a fila: reset antes de liberar
                disp.falhar();
                disp.resetar();
                unsafe { dealloc(memoria as *mut u8, layout) };
                return Err(e);
            }
        };
        disp.ativar();
        let mac = if features & FEATURE_MAC != 0 { ler_config_bytes::<6>(&disp, 0) } else { MAC_PADRAO };
        let mut nic = VirtioNet {
            disp,
            rx,
            tx,
            memoria,
            mac,
            cabecas_rx: [0; QUANTOS_RX],
            cabecas_tx: [None; QUANTOS_TX],
            avisar_rx: false,
            descartados_tx: 0,
        };
        nic.postar_todos_rx()?;
        Ok(nic)
    }

    /// Cria e instala as filas rx e tx.
    unsafe fn montar_filas(disp: &VirtioMmio, memoria: usize) -> Result<(Virtqueue, Virtqueue), ErroRede> {
        let montar = |fila: u32, desejado: usize| -> Result<Virtqueue, ErroRede> {
            let tamanho = tamanho_de_fila(disp.tamanho_max_fila(fila), desejado).ok_or(ErroRede::Protocolo("fila de rede inexistente"))?;
            let regiao = memoria + fila as usize * REGIAO_FILA;
            let q = unsafe { Virtqueue::nova(fila, tamanho, regiao) }.map_err(|e| ErroRede::Protocolo(e.descricao()))?;
            q.instalar(disp).map_err(|e| ErroRede::Protocolo(e.descricao()))?;
            Ok(q)
        };
        Ok((montar(FILA_RX, QUANTOS_RX)?, montar(FILA_TX, QUANTOS_TX)?))
    }

    pub fn mac(&self) -> [u8; 6] {
        self.mac
    }

    fn buffer_rx(&self, i: usize) -> usize {
        self.memoria + BUFFERS_RX + i * TAMANHO_BUFFER
    }

    fn buffer_tx(&self, i: usize) -> usize {
        self.memoria + BUFFERS_TX + i * TAMANHO_BUFFER
    }

    // ---------------- recebimento ----------------

    /// Entrega todos os buffers rx que couberem na fila.
    fn postar_todos_rx(&mut self) -> Result<(), ErroRede> {
        let quantos = QUANTOS_RX.min(self.rx.tamanho() as usize);
        for i in 0..quantos {
            self.postar_rx(i)?;
        }
        self.disp.notificar(FILA_RX);
        Ok(())
    }

    fn postar_rx(&mut self, i: usize) -> Result<(), ErroRede> {
        let parte = Parte { endereco: self.buffer_rx(i), tamanho: TAMANHO_BUFFER as u32, dispositivo_escreve: true };
        self.cabecas_rx[i] = self.rx.adicionar(&[parte]).ok_or(ErroRede::Protocolo("fila rx cheia"))?;
        Ok(())
    }

    /// Copia o proximo quadro recebido (sem o cabecalho virtio) para
    /// `destino` e devolve o tamanho. None = nada chegou. O buffer volta
    /// na hora para o dispositivo.
    pub fn receber(&mut self, destino: &mut [u8; MAX_QUADRO]) -> Option<usize> {
        loop {
            let Some(usado) = self.rx.colher() else {
                if self.avisar_rx {
                    self.avisar_rx = false;
                    self.disp.notificar(FILA_RX);
                }
                return None;
            };
            let Some(i) = self.cabecas_rx.iter().position(|&c| c == usado.cabeca) else { continue };
            let endereco = self.buffer_rx(i);
            virtqueue::do_dispositivo(endereco, TAMANHO_BUFFER);
            let tamanho = (usado.tamanho as usize).saturating_sub(TAMANHO_CABECALHO).min(MAX_QUADRO);
            unsafe { copy_nonoverlapping((endereco + TAMANHO_CABECALHO) as *const u8, destino.as_mut_ptr(), tamanho) };
            // o descritor acabou de voltar para a lista de livres: nunca falha
            let _ = self.postar_rx(i);
            self.avisar_rx = true;
            if tamanho > 0 {
                return Some(tamanho);
            }
        }
    }

    // ---------------- envio ----------------

    /// Libera os buffers tx que o dispositivo ja mandou.
    fn recolher_tx(&mut self) {
        while let Some(usado) = self.tx.colher() {
            if let Some(slot) = self.cabecas_tx.iter_mut().find(|c| **c == Some(usado.cabeca)) {
                *slot = None;
            }
        }
    }

    /// Manda um quadro feito dos `partes` coladas (cabecalho Ethernet,
    /// cabecalho IP, dados...). false = sem buffer livre ou grande demais.
    pub fn enviar(&mut self, partes: &[&[u8]]) -> bool {
        self.recolher_tx();
        let tamanho: usize = partes.iter().map(|p| p.len()).sum();
        let Some(i) = self.cabecas_tx.iter().position(|c| c.is_none()) else {
            self.descartados_tx += 1;
            return false;
        };
        if tamanho > MAX_QUADRO {
            self.descartados_tx += 1;
            return false;
        }
        let base = self.buffer_tx(i);
        let mut destino = base + TAMANHO_CABECALHO;
        unsafe {
            core::ptr::write_bytes(base as *mut u8, 0, TAMANHO_CABECALHO);
            for parte in partes {
                copy_nonoverlapping(parte.as_ptr(), destino as *mut u8, parte.len());
                destino += parte.len();
            }
        }
        virtqueue::para_dispositivo(base, TAMANHO_CABECALHO + tamanho);
        let parte = Parte { endereco: base, tamanho: (TAMANHO_CABECALHO + tamanho) as u32, dispositivo_escreve: false };
        let Some(cabeca) = self.tx.adicionar(&[parte]) else {
            self.descartados_tx += 1;
            return false;
        };
        self.cabecas_tx[i] = Some(cabeca);
        self.disp.notificar(FILA_TX);
        true
    }

    /// Reset do dispositivo (ele para de usar a memoria) e libera o DMA.
    pub fn encerrar(self) {
        self.disp.resetar();
        if let Ok(layout) = Layout::from_size_align(MEMORIA, 4096) {
            unsafe { dealloc(self.memoria as *mut u8, layout) };
        }
    }
}
