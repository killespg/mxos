//! MXOS - kernel ARM64 escrito do zero em Rust.
//!
//! Sem std, sem libc, sem kernel pronto: so o `core` e o `alloc` do Rust.

#![no_std]
#![no_main]

extern crate alloc;

mod armazenamento;
mod atualizacao;
mod audio;
mod chip8;
mod config;
mod console_boot;
mod cpu;
mod depuracao;
mod cripto;
mod drivers;
mod entrada;
mod excecao;
mod fonte;
mod frequencia;
mod gic;
mod gui;
mod heap;
mod ia;
mod irq;
mod json;
mod mapa;
mod midia;
#[cfg_attr(target_arch = "arm", path = "mmu_arm.rs")]
mod mmu;
mod nas;
mod navegador;
mod plataforma;
mod psci;
mod processo;
mod rede;
mod shell;
mod smp;
mod som;
mod terminal;
mod timer;
mod uart;
#[cfg(any(feature = "mxq", feature = "rk322x"))]
mod usb;
mod virtio;
mod video;
mod web;
mod x11;

use core::arch::global_asm;
use core::fmt::Write;
use core::panic::PanicInfo;

use shell::Shell;
use terminal::Terminal;

#[cfg(target_arch = "aarch64")]
global_asm!(include_str!("boot.s"));
#[cfg(target_arch = "aarch64")]
global_asm!(include_str!("vetores.s"));
#[cfg(target_arch = "arm")]
global_asm!(include_str!("boot_arm.s"), options(raw));
#[cfg(target_arch = "arm")]
global_asm!(include_str!("vetores_arm.s"), options(raw));

/// Chamado pelo boot.s, ja em EL1, com pilhas e BSS prontos.
#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(el_boot: u64) -> ! {
    // TV box: as mensagens do boot na tela desde ja (no framebuffer do U-Boot)
    if console_boot::iniciar() {
        etapa(format_args!("MXOS {}: kernel no ar (núcleo 0, modo {el_boot})", env!("CARGO_PKG_VERSION")));
    }
    let mut terminal = Terminal::new();
    etapa(format_args!("mmu     : ligando tabelas de páginas e caches"));
    mmu::ligar();
    etapa(format_args!("mmu     : ligada"));
    // o heap pula o framebuffer do U-Boot enquanto o console de boot o usa
    let fb = console_boot::faixa();
    let mut primeiro = true;
    let pedacos = core::iter::once((mapa::kernel().fim, plataforma::RAM_FIM)).chain(plataforma::RAM_EXTRA.iter().copied());
    for (inicio, fim) in pedacos {
        let partes = match fb {
            Some((a, b)) if a > inicio && b < fim => [(inicio, a), (b, fim)],
            _ => [(inicio, fim), (fim, fim)],
        };
        for (a, b) in partes {
            if b > a {
                if core::mem::take(&mut primeiro) { heap::iniciar(a, b) } else { heap::acrescentar(a, b) }
            }
        }
    }
    etapa(format_args!("heap    : {} MB livres", heap::estatisticas().livre >> 20));
    etapa(format_args!("irq     : ligando GIC e timer ({} Hz no contador)", cpu::frequencia_timer()));
    irq::iniciar(&mut terminal.serial);
    etapa(format_args!("irq     : ligadas"));
    // sem ramfb (ou na TV box, por enquanto) o terminal fica so na serial
    diga(&mut terminal, format_args!("vídeo   : procurando a tela..."));
    match terminal.ligar_video() {
        Ok(v) => diga(&mut terminal, format_args!("vídeo   : {}x{}, {}", v.largura, v.altura, v.nome)),
        Err(motivo) => diga(&mut terminal, format_args!("vídeo   : desligado ({motivo})")),
    }
    #[cfg(feature = "rk322x")]
    {
        diga(&mut terminal, format_args!("rga     : ligando o acelerador 2D..."));
        if video::rga::iniciar() {
            diga(&mut terminal, format_args!("rga     : versão {:08x}, copia e escala a tela", video::rga::versao()));
        } else {
            diga(&mut terminal, format_args!("rga     : não respondeu ao teste, a CPU desenha"));
        }
    }
        let terminal = qemu::iniciar(terminal, el_boot);
    // sem area de trabalho: o shell le a propria serial
    Shell::nova(terminal, el_boot).rodar()
}

/// Linha do boot antes do Terminal existir: so o console de boot da tela.
fn etapa(linha: core::fmt::Arguments) {
    let _ = writeln!(console_boot::Boot, "{linha}");
}

/// Uma linha de estado do boot (serial e, se ligado, o console da tela).
fn diga(terminal: &mut Terminal, linha: core::fmt::Arguments) {
    let _ = writeln!(terminal, "{linha}");
}

/// Dispositivos que so existem no QEMU virt: disco, rede, teclado e mouse.
/// Com video e entrada a area de trabalho assume e esta funcao nao volta.
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
        // TV box: o pendrive (disco "/"), teclado e mouse sao USB
        #[cfg(any(feature = "mxq", feature = "rk322x"))]
        diga(&mut terminal, format_args!("usb     : ligando os controladores..."));
        #[cfg(any(feature = "mxq", feature = "rk322x"))]
        match crate::usb::iniciar() {
            Ok(linha) => diga(&mut terminal, format_args!("usb     : {linha}")),
            Err(motivo) => diga(&mut terminal, format_args!("usb     : desligado ({motivo})")),
        }
        diga(&mut terminal, format_args!("disco   : procurando..."));
        iniciar_disco(&mut terminal);
        // TV box: `config video 1080p` desenha direto em 1920x1080 (a config
        // mora no disco, entao o video e religado agora, antes da area de trabalho)
        #[cfg(feature = "rk322x")]
        if crate::config::Config::carregar().obter("video").is_some_and(|v| v.trim() == "1080p") {
            match terminal.video_nativo() {
                Ok(v) => diga(&mut terminal, format_args!("vídeo   : {}x{} nativo", v.largura, v.altura)),
                Err(motivo) => diga(&mut terminal, format_args!("vídeo   : 1080p não deu ({motivo})")),
            }
        }
        // a frequencia escolhida mora na config, que esta no disco
        if let Some(linha) = crate::frequencia::aplicar_config() {
            diga(&mut terminal, format_args!("cpu     : {linha}"));
        }
        diga(&mut terminal, format_args!("rede    : ligando (DHCP)..."));
        iniciar_rede(&mut terminal);
        let (Ok(video), Some(framebuffer)) = (terminal.video(), terminal.tela_do_framebuffer()) else {
            diga(&mut terminal, format_args!("gui     : sem vídeo, fica só o shell na serial"));
            return terminal;
        };
        diga(&mut terminal, format_args!("entrada : procurando teclado e mouse..."));
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
    let linha = info.location().map_or(0, |l| l.line());
    console_boot::rastro(0xDEAD_0000 | (linha & 0xFFFF), 0);
    let _ = writeln!(console_boot::Erro, "\n*** PÂNICO no kernel: {info}");
    // com a area de trabalho ligada o erro tambem aparece na tela
    excecao::tela_de_erro("PÂNICO NO KERNEL", |painel| write!(painel, "{info}"));
    console_boot::reiniciar_depois_de_erro();
    cpu::parar()
}
