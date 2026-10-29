//! MXOS - kernel ARM64 escrito do zero em Rust.
//!
//! Sem std, sem libc, sem kernel pronto: so o `core` e o `alloc` do Rust.

#![no_std]
#![no_main]

extern crate alloc;

#[cfg(not(feature = "mxq"))]
mod armazenamento;
#[cfg(not(feature = "mxq"))]
mod audio;
#[cfg(not(feature = "mxq"))]
mod config;
mod cpu;
#[cfg(not(feature = "mxq"))]
mod drivers;
#[cfg(not(feature = "mxq"))]
mod entrada;
mod excecao;
mod fonte;
mod gic;
#[cfg(not(feature = "mxq"))]
mod gui;
mod heap;
mod irq;
mod mapa;
mod mmu;
#[cfg(not(feature = "mxq"))]
mod navegador;
mod plataforma;
mod psci;
#[cfg(not(feature = "mxq"))]
mod processo;
#[cfg(not(feature = "mxq"))]
mod rede;
mod shell;
#[cfg(not(feature = "mxq"))]
mod som;
mod terminal;
mod timer;
mod uart;
#[cfg(not(feature = "mxq"))]
mod virtio;
mod video;

use core::arch::global_asm;
use core::fmt::Write;
use core::panic::PanicInfo;

use shell::Shell;
use terminal::Terminal;

global_asm!(include_str!("boot.s"));
global_asm!(include_str!("vetores.s"));

/// Chamado pelo boot.s, ja em EL1, com pilhas e BSS prontos.
#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(el_boot: u64) -> ! {
    let mut terminal = Terminal::new();
    mmu::ligar();
    heap::iniciar(mapa::kernel().fim, plataforma::RAM_FIM);
    irq::iniciar(&mut terminal.serial);
    // sem ramfb (ou na TV box, por enquanto) o terminal fica so na serial
    match terminal.ligar_video() {
        Ok(v) => diga(&mut terminal, format_args!("vídeo   : {}x{}, {}", v.largura, v.altura, v.nome)),
        Err(motivo) => diga(&mut terminal, format_args!("vídeo   : desligado ({motivo})")),
    }
    #[cfg(not(feature = "mxq"))]
    let terminal = qemu::iniciar(terminal, el_boot);
    // sem area de trabalho: o shell le a propria serial
    Shell::nova(terminal, el_boot).rodar()
}

/// Uma linha de estado do boot (serial e, se ligado, o console da tela).
fn diga(terminal: &mut Terminal, linha: core::fmt::Arguments) {
    let _ = writeln!(terminal, "{linha}");
}

/// Dispositivos que so existem no QEMU virt: disco, rede, teclado e mouse.
/// Com video e entrada a area de trabalho assume e esta funcao nao volta.
#[cfg(not(feature = "mxq"))]
mod qemu {
    use core::fmt;

    use super::diga;
    use crate::entrada::Entrada;
    use crate::shell::Shell;
    use crate::terminal::Terminal;
    use crate::{armazenamento, gui, rede};

    /// Endereco IPv4 no formato a.b.c.d.
    struct Ip([u8; 4]);

    impl fmt::Display for Ip {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            let [a, b, c, d] = self.0;
            write!(f, "{a}.{b}.{c}.{d}")
        }
    }

    pub fn iniciar(mut terminal: Terminal, el_boot: u64) -> Terminal {
        iniciar_disco(&mut terminal);
        iniciar_rede(&mut terminal);
        let (Ok(video), Some(framebuffer)) = (terminal.video(), terminal.tela_do_framebuffer()) else {
            diga(&mut terminal, format_args!("gui     : sem vídeo, fica só o shell na serial"));
            return terminal;
        };
        let entrada = match Entrada::iniciar(video.largura as u32, video.altura as u32) {
            Ok(entrada) => entrada,
            Err(motivo) => {
                diga(&mut terminal, format_args!("entrada : nenhuma ({motivo}), fica só o shell na serial"));
                return terminal;
            }
        };
        let nomes = entrada.nomes();
        diga(&mut terminal, format_args!("entrada : {} dispositivo(s): {}", nomes.len(), nomes.join(", ")));
        diga(&mut terminal, format_args!("gui     : área de trabalho {}x{}; a serial continua no terminal", video.largura, video.altura));
        gui::rodar(Shell::nova(terminal, el_boot), entrada, framebuffer)
    }

    fn iniciar_disco(terminal: &mut Terminal) {
        match armazenamento::iniciar() {
            Ok(d) => diga(
                terminal,
                format_args!(
                    // tamanho do disco inteiro (como no "discos"); o livre e o do FAT32
                    "disco   : {} com {} MB ({} MB livres), {} \"{}\"",
                    d.nome,
                    (d.setores * 512) >> 20,
                    d.livre >> 20,
                    d.sistema,
                    d.rotulo
                ),
            ),
            Err(armazenamento::Erro::SemDisco) => diga(terminal, format_args!("disco   : nenhum (sem virtio-blk)")),
            Err(e) => diga(terminal, format_args!("disco   : não montado ({})", e.descricao())),
        }
    }

    fn iniciar_rede(terminal: &mut Terminal) {
        match rede::iniciar() {
            Ok(r) => diga(
                terminal,
                format_args!("rede    : ip {}, gateway {}, dns {} (DHCP)", Ip(r.ip), Ip(r.gateway), Ip(r.dns)),
            ),
            Err(rede::ErroRede::SemRede) => diga(terminal, format_args!("rede    : nenhuma (sem virtio-net)")),
            Err(e) => diga(terminal, format_args!("rede    : desligada ({})", e.descricao())),
        }
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    let mut console = uart::Uart::new();
    let _ = writeln!(console, "\n*** PÂNICO no kernel: {info}");
    // com a area de trabalho ligada o erro tambem aparece na tela
    excecao::tela_de_erro("PÂNICO NO KERNEL", |painel| write!(painel, "{info}"));
    cpu::parar()
}
