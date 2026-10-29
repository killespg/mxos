//! Porta serial: a "tela" e o "teclado" do MXOS por enquanto.
//!
//! Nao configura velocidade: no QEMU a PL011 ja funciona, e na TV box o
//! U-Boot deixa a UART pronta em 115200 baud.
//!
//! Leitura: a interrupcao de RX joga os bytes numa fila (ENTRADA) e o laco
//! principal tira de la. Se a interrupcao nao vier (registrador errado na
//! TV box, por exemplo), o tique de 100 Hz acorda a CPU e lemos direto.

use core::cell::UnsafeCell;
use core::fmt;
use core::hint::spin_loop;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::irq;

#[cfg(not(any(feature = "mxq", feature = "rk322x")))]
mod regs {
    pub const INVERTER: u32 = 0;
    pub const NOME: &str = "PL011 em 0x09000000 (QEMU virt)";
    pub const BASE: usize = 0x0900_0000;
    pub const TX: usize = 0x00; // UARTDR
    pub const RX: usize = 0x00; // UARTDR
    pub const STATUS: usize = 0x18; // UARTFR
    pub const TX_CHEIO: u32 = 1 << 5;
    pub const RX_VAZIO: u32 = 1 << 4;
    pub const MASCARA_IRQ: usize = 0x38; // UARTIMSC
    pub const LIMPAR_IRQ: usize = 0x44; // UARTICR
    /// RXIM (chegou byte) e RTIM (byte parado na FIFO, timeout).
    pub const IRQ_RX: u32 = (1 << 4) | (1 << 6);
}

// Registradores do driver meson_uart do Linux (familia GXL).
// Ainda nao testado no hardware.
/// UART2 do RK322x (16550 DesignWare, registradores de 4 em 4 bytes), a
/// do console do U-Boot da Rockchip (115200 baud, ja configurada).
#[cfg(feature = "rk322x")]
mod regs {
    pub const NOME: &str = "UART2 em 0x11030000 (Rockchip RK322x)";
    pub const BASE: usize = 0x1103_0000;
    pub const TX: usize = 0x00; // THR
    pub const RX: usize = 0x00; // RBR
    pub const IER: usize = 0x04;
    pub const STATUS: usize = 0x14; // LSR
    /// LSR: THRE (bit 5) = 1 quando cabe mais; DR (bit 0) = 1 quando chegou.
    /// Ao contrario dos outros: o INVERTER vira os dois.
    pub const TX_CHEIO: u32 = 1 << 5;
    pub const RX_VAZIO: u32 = 1 << 0;
    pub const INVERTER: u32 = (1 << 5) | (1 << 0);
}

#[cfg(feature = "mxq")]
mod regs {
    pub const INVERTER: u32 = 0;
    pub const NOME: &str = "UART_AO_A em 0xC81004C0 (Amlogic S905W)";
    pub const BASE: usize = 0xC810_04C0;
    pub const TX: usize = 0x00; // AML_UART_WFIFO
    pub const RX: usize = 0x04; // AML_UART_RFIFO
    pub const STATUS: usize = 0x0C; // AML_UART_STATUS
    pub const TX_CHEIO: u32 = 1 << 21;
    pub const RX_VAZIO: u32 = 1 << 20;
    pub const CONTROLE: usize = 0x08; // AML_UART_CONTROL
    pub const MISC: usize = 0x10; // AML_UART_MISC
    pub const RX_IRQ_LIGADO: u32 = 1 << 27; // AML_UART_RX_INT_EN
    /// AML_UART_RECV_IRQ(1): avisa a cada byte recebido (byte baixo do MISC).
    pub const RX_IRQ_LIMITE: u32 = 1;
}

pub use regs::NOME;

const TAMANHO_FILA: usize = 256;

/// Fila circular sem trava: um produtor (a interrupcao) e um consumidor
/// (o laco principal, com interrupcoes desligadas). Cada lado so escreve
/// no proprio indice. Os indices so crescem; a posicao e indice % tamanho.
pub struct Fila {
    bytes: UnsafeCell<[u8; TAMANHO_FILA]>,
    escrita: AtomicUsize,
    leitura: AtomicUsize,
}

unsafe impl Sync for Fila {}

static ENTRADA: Fila = Fila::new();

impl Fila {
    pub const fn new() -> Self {
        Fila {
            bytes: UnsafeCell::new([0; TAMANHO_FILA]),
            escrita: AtomicUsize::new(0),
            leitura: AtomicUsize::new(0),
        }
    }

    fn posicao(&self, indice: usize) -> *mut u8 {
        // ponteiro cru: nunca criamos referencia para o vetor inteiro
        unsafe { self.bytes.get().cast::<u8>().add(indice % TAMANHO_FILA) }
    }

    /// So o produtor chama. Devolve false se estava cheia (o byte se perde).
    pub fn colocar(&self, byte: u8) -> bool {
        let escrita = self.escrita.load(Ordering::Relaxed);
        // Acquire: o consumidor ja terminou de ler a posicao que vamos reusar
        let leitura = self.leitura.load(Ordering::Acquire);
        if escrita.wrapping_sub(leitura) >= TAMANHO_FILA {
            return false;
        }
        unsafe { self.posicao(escrita).write(byte) };
        // Release: o byte fica visivel antes do novo indice
        self.escrita.store(escrita.wrapping_add(1), Ordering::Release);
        true
    }

    /// So o consumidor chama.
    pub fn tirar(&self) -> Option<u8> {
        let leitura = self.leitura.load(Ordering::Relaxed);
        // Acquire: se vemos o indice novo, vemos o byte escrito antes dele
        let escrita = self.escrita.load(Ordering::Acquire);
        if leitura == escrita {
            return None;
        }
        let byte = unsafe { self.posicao(leitura).read() };
        // Release: so libera a posicao depois de ter lido o byte
        self.leitura.store(leitura.wrapping_add(1), Ordering::Release);
        Some(byte)
    }
}

pub struct Uart;

impl Uart {
    pub const fn new() -> Self {
        Uart
    }

    fn ler(&self, offset: usize) -> u32 {
        // volatile: o valor muda sozinho (e hardware), o compilador nao pode cachear
        unsafe { read_volatile((regs::BASE + offset) as *const u32) }
    }

    fn escrever(&self, offset: usize, valor: u32) {
        unsafe { write_volatile((regs::BASE + offset) as *mut u32, valor) }
    }

    pub fn enviar(&mut self, byte: u8) {
        while (self.ler(regs::STATUS) ^ regs::INVERTER) & regs::TX_CHEIO != 0 {
            spin_loop();
        }
        self.escrever(regs::TX, byte as u32);
    }

    pub fn receber(&mut self) -> Option<u8> {
        if (self.ler(regs::STATUS) ^ regs::INVERTER) & regs::RX_VAZIO != 0 {
            None
        } else {
            Some(self.ler(regs::RX) as u8)
        }
    }

    /// Byte recebido, sem dormir. Primeiro o que a interrupcao guardou (mais
    /// antigo), depois a FIFO direto, caso a interrupcao de RX nao funcione.
    /// Com a interrupcao bloqueada: se ela esvaziasse a FIFO no meio, um byte
    /// novo passaria na frente dos guardados (ou a FIFO vazia repetiria o
    /// ultimo byte). Pode ser chamada com as interrupcoes ligadas ou nao.
    pub fn tentar_receber(&mut self) -> Option<u8> {
        sem_interrupcoes(|| ENTRADA.tirar().or_else(|| self.receber()))
    }

    /// Dorme ate chegar um byte.
    #[allow(dead_code)]
    pub fn esperar_byte(&mut self) -> u8 {
        irq::esperar(|| self.tentar_receber())
    }

    /// Chamado de dentro da interrupcao: esvazia a FIFO na fila ENTRADA.
    /// Devolve quantos bytes leu. Nao aloca e nao imprime.
    pub fn atender_interrupcao(&mut self) -> usize {
        // limpa antes de esvaziar: um byte que chegue depois da limpeza
        // levanta a interrupcao de novo, entao nada fica esquecido na FIFO
        self.limpar_interrupcao();
        let mut lidos = 0;
        // limite para nao ficar preso aqui se a serial nao parar de mandar
        while lidos < TAMANHO_FILA {
            let Some(byte) = self.receber() else { break };
            // fila cheia: descarta, senao a FIFO cheia repete a interrupcao sem parar
            ENTRADA.colocar(byte);
            lidos += 1;
        }
        lidos
    }
}

#[cfg(not(any(feature = "mxq", feature = "rk322x")))]
impl Uart {
    pub fn ligar_interrupcao_rx(&mut self) {
        let mascara = self.ler(regs::MASCARA_IRQ);
        self.escrever(regs::MASCARA_IRQ, mascara | regs::IRQ_RX);
    }

    fn limpar_interrupcao(&mut self) {
        self.escrever(regs::LIMPAR_IRQ, regs::IRQ_RX);
    }
}

// Igual ao meson_uart_startup do Linux, sem a interrupcao de TX.
// Ainda nao testado no hardware.
#[cfg(feature = "mxq")]
impl Uart {
    pub fn ligar_interrupcao_rx(&mut self) {
        let controle = self.ler(regs::CONTROLE);
        self.escrever(regs::CONTROLE, controle | regs::RX_IRQ_LIGADO);
        let misc = self.ler(regs::MISC);
        self.escrever(regs::MISC, (misc & !0xFF) | regs::RX_IRQ_LIMITE);
    }

    /// Interrupcao de borda: nao ha nada para limpar na UART.
    fn limpar_interrupcao(&mut self) {}
}

// 16550 (DesignWare): IER bit 0 liga a interrupcao de dado recebido; ler
// o RBR ja limpa.
#[cfg(feature = "rk322x")]
impl Uart {
    pub fn ligar_interrupcao_rx(&mut self) {
        self.escrever(regs::IER, 1);
    }

    fn limpar_interrupcao(&mut self) {}
}

/// Roda `f` com IRQ bloqueada e depois volta o DAIF como estava (quem ja
/// estava com elas desligadas, como o irq::esperar, continua assim).
/// Sem `nomem`: o asm e barreira do compilador para os acessos de `f`.
fn sem_interrupcoes<T>(f: impl FnOnce() -> T) -> T {
    let estado = crate::cpu::guardar_e_desligar_irq();
    let valor = f();
    crate::cpu::restaurar_irq(estado);
    valor
}

impl fmt::Write for Uart {
    fn write_str(&mut self, texto: &str) -> fmt::Result {
        for byte in texto.bytes() {
            // terminais seriais precisam de \r\n para voltar ao inicio da linha
            if byte == b'\n' {
                self.enviar(b'\r');
            }
            self.enviar(byte);
        }
        Ok(())
    }
}
