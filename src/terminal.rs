//! Terminal do MXOS: tudo que o shell escreve vai para a serial e, se o
//! video estiver ligado, tambem para a tela (um console grafico de texto).
//! O console pode ser trocado: na area de trabalho ele passa a ser o da
//! janela do terminal, e o console de tela cheia do boot sai de cena.
//! O teclado do modo serial continua sendo a serial.

use core::fmt;

use crate::uart::Uart;
use crate::video::tela::{Console, Tela};

#[derive(Clone, Copy)]
pub struct Video {
    pub largura: usize,
    pub altura: usize,
    pub base: usize,
    pub nome: &'static str,
}

pub struct Terminal {
    pub serial: Uart,
    tela: Option<Console>,
    video: Option<Video>,
    /// O framebuffer inteiro, para quem desenha sem o console (a GUI).
    framebuffer: Option<Tela>,
    /// Por que o video nao ligou (mostrado pelo shell).
    motivo: &'static str,
}

impl Terminal {
    pub const fn new() -> Self {
        Terminal { serial: Uart::new(), tela: None, video: None, framebuffer: None, motivo: "ainda não foi ligado" }
    }

    /// Byte da serial, se chegou algum. Nao dorme.
    pub fn tentar_receber(&mut self) -> Option<u8> {
        self.serial.tentar_receber()
    }

    /// Poe outro console no lugar do atual (None = so a serial) e devolve
    /// o antigo. O video continua ligado: so muda onde o texto aparece.
    #[cfg_attr(feature = "mxq", allow(dead_code))]
    pub fn trocar_console(&mut self, console: Option<Console>) -> Option<Console> {
        core::mem::replace(&mut self.tela, console)
    }

    /// O framebuffer inteiro (None sem video).
    #[cfg_attr(feature = "mxq", allow(dead_code))]
    pub fn tela_do_framebuffer(&self) -> Option<Tela> {
        self.framebuffer
    }

    /// Eco de uma tecla nos dois lugares.
    pub fn enviar(&mut self, byte: u8) {
        self.serial.enviar(byte);
        if let Some(console) = &mut self.tela {
            console.escrever_byte(byte);
        }
    }

    /// (coluna, linha) do cursor no console da tela, se houver.
    pub fn posicao(&self) -> Option<(usize, usize)> {
        self.tela.as_ref().map(|c| c.posicao())
    }

    pub fn limpar(&mut self) {
        let _ = fmt::Write::write_str(&mut self.serial, "\x1b[2J\x1b[H");
        if let Some(console) = &mut self.tela {
            console.limpar();
        }
    }

    pub fn video(&self) -> Result<Video, &'static str> {
        self.video.ok_or(self.motivo)
    }

    /// Liga o video. Se nao der, o terminal segue so na serial.
    pub fn ligar_video(&mut self) -> Result<Video, &'static str> {
        if self.video.is_none() {
            match video_qemu::ligar() {
                Ok((video, framebuffer, console)) => {
                    self.video = Some(video);
                    self.framebuffer = Some(framebuffer);
                    self.tela = Some(console);
                }
                Err(motivo) => self.motivo = motivo,
            }
        }
        self.video()
    }
}

impl fmt::Write for Terminal {
    fn write_str(&mut self, texto: &str) -> fmt::Result {
        self.serial.write_str(texto)?;
        if let Some(console) = &mut self.tela {
            console.write_str(texto)?;
        }
        Ok(())
    }
}

#[cfg(not(feature = "mxq"))]
mod video_qemu {
    use alloc::alloc::{Layout, alloc_zeroed, dealloc};

    use super::Video;
    use crate::video::fwcfg::FwCfg;
    use crate::video::ramfb;
    use crate::video::tela::{self, Console, Escala, Formato, Tela};

    const LARGURA: usize = 1024;
    const ALTURA: usize = 768;
    /// Faixa de cima com o titulo; o console de texto fica embaixo dela.
    const ALTURA_TITULO: usize = 48;

    /// Framebuffer do heap passado ao ramfb do QEMU. Nunca e liberado
    /// depois de configurado: o QEMU continua lendo dele.
    pub fn ligar() -> Result<(Video, Tela, Console), &'static str> {
        let layout = Layout::from_size_align(LARGURA * ALTURA * 4, 4096).map_err(|_| "tamanho inválido")?;
        let ponteiro = unsafe { alloc_zeroed(layout) };
        if ponteiro.is_null() {
            return Err("sem memória para o framebuffer");
        }
        let base = ponteiro as usize;
        // mapa identidade: o endereco virtual do heap e o fisico
        let modo = ramfb::modo_xrgb(base as u64, LARGURA as u32, ALTURA as u32);
        if let Err(erro) = ramfb::configurar(&FwCfg::qemu_virt(), &modo) {
            // o QEMU nao aceitou o buffer, entao pode voltar para o heap
            unsafe { dealloc(ponteiro, layout) };
            return Err(erro.descricao());
        }

        let tela = unsafe { Tela::new(base, LARGURA, ALTURA, LARGURA * 4, Formato::Xrgb8888) };
        desenhar_titulo(&tela);
        let area = tela.regiao(0, ALTURA_TITULO, LARGURA, ALTURA - ALTURA_TITULO).ok_or("tela pequena demais")?;
        let console = Console::new(area, Escala::CONSOLE, tela::BRANCO, tela::PRETO);
        let video = Video { largura: LARGURA, altura: ALTURA, base, nome: "ramfb do QEMU, XRGB8888" };
        Ok((video, tela, console))
    }

    fn desenhar_titulo(tela: &Tela) {
        const FUNDO: tela::Cor = 0x0010_2040;
        tela.retangulo(0, 0, LARGURA, ALTURA_TITULO, FUNDO);
        tela.texto(12, 12, "MXOS", tela::AMARELO, None, Escala::uniforme(3));
        tela.texto(124, 20, "kernel ARM64 em Rust", tela::CINZA, None, Escala::NORMAL);
        let faixas = [tela::VERMELHO, tela::AMARELO, tela::VERDE, tela::CIANO, tela::AZUL, tela::MAGENTA];
        for (i, cor) in faixas.iter().enumerate() {
            tela.retangulo(LARGURA - 6 * 24 - 12 + i * 24, 12, 20, 24, *cor);
        }
    }
}

// Na TV box o caminho e o framebuffer que o U-Boot deixa ligado
// (src/video/mxq.rs), mapeado pela MMU (plataforma::FRAMEBUFFER). Nunca
// testado no hardware.
#[cfg(feature = "mxq")]
mod video_qemu {
    use super::Video;
    use crate::video::mxq;
    use crate::video::tela::{self, Console, Escala};

    /// Desenha direto no framebuffer que o U-Boot deixou no HDMI, no formato
    /// dele (RGB565 no padrao): nada muda nos registradores do video.
    pub fn ligar() -> Result<(Video, tela::Tela, Console), &'static str> {
        let info = mxq::descobrir().map_err(|e| e.descricao())?;
        let tela = unsafe { mxq::usar_como_esta(&info) }.map_err(|e| e.descricao())?;
        let (largura, altura) = (tela.largura(), tela.altura());
        tela.limpar(tela::PRETO);
        let console = Console::new(tela, Escala::CONSOLE, tela::BRANCO, tela::PRETO);
        let (base, _) = info.framebuffer_uboot();
        let video = Video { largura, altura, base, nome: "framebuffer do U-Boot (HDMI)" };
        Ok((video, tela, console))
    }
}
