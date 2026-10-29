//! Janela do terminal: o shell do MXOS dentro da area de trabalho.
//!
//! A janela tem uma Superficie propria com um Console de texto por cima.
//! Ao abrir, esse console entra no lugar do console de tela cheia do boot:
//! o que o shell escreve vai para a serial e para a janela. As teclas vem
//! do teclado virtio (Caractere) ou da serial (o laco da GUI repassa).
//!
//! So existe um shell. Quando a janela fecha ele fica guardado, e o menu
//! (ou o proximo byte da serial) abre a janela de novo com o mesmo shell.

use alloc::boxed::Box;
use core::cell::UnsafeCell;

use crate::gui::grafico::Superficie;
use crate::gui::janelas::{App, EventoApp};
use crate::shell::{PROMPT, Shell};
use crate::video::tela::{Console, Cor, Escala};

pub const TITULO: &str = "Terminal";
const COLUNAS: usize = 80;
const LINHAS: usize = 25;
/// Espaco entre o texto e a borda da janela.
const MARGEM: usize = 4;
/// Tamanho do conteudo em pixels: celulas de 8x16 e a margem em volta.
pub const LARGURA: usize = COLUNAS * 8 + 2 * MARGEM;
pub const ALTURA: usize = LINHAS * 16 + 2 * MARGEM;
const FRENTE: Cor = 0x00E0_E0E0;
const FUNDO: Cor = 0x0010_1418;

/// O shell enquanto a janela esta fechada. So o laco principal mexe.
struct Guardado(UnsafeCell<Option<Shell>>);

// Seguranca: so o laco principal da GUI usa, nunca uma interrupcao.
unsafe impl Sync for Guardado {}

static GUARDADO: Guardado = Guardado(UnsafeCell::new(None));

impl Guardado {
    /// Guarda o shell se o lugar estiver vazio (senao ele se perde).
    fn colocar(&self, shell: Shell) {
        let lugar = unsafe { &mut *self.0.get() };
        if lugar.is_none() {
            *lugar = Some(shell);
        }
    }

    fn tirar(&self) -> Option<Shell> {
        unsafe { (*self.0.get()).take() }
    }
}

/// Para o menu iniciar: reabre o terminal com o shell guardado ou, se ele
/// ja esta numa janela, mostra um aviso.
pub fn criar() -> Box<dyn App> {
    match GUARDADO.tirar() {
        Some(shell) => Box::new(AppTerminal::reaberta(shell)),
        None => Box::new(Aviso),
    }
}

pub struct AppTerminal {
    /// None so depois de fechar (o shell volta para GUARDADO).
    shell: Option<Shell>,
    /// Box: o Console aponta para estes pixels, que nunca mudam de lugar.
    superficie: Box<Superficie>,
}

impl AppTerminal {
    /// Primeira janela: troca o console do boot pelo da janela e mostra o
    /// banner com o primeiro prompt.
    pub fn nova(shell: Shell) -> AppTerminal {
        let mut app = AppTerminal::com_shell(shell, |_| {});
        if let Some(shell) = &mut app.shell {
            shell.banner();
        }
        app
    }

    /// Janela reaberta: o prompt (e o que ja estava digitado) aparece so na
    /// janela, para a serial nao ganhar um prompt a mais.
    fn reaberta(shell: Shell) -> AppTerminal {
        let linha = alloc::string::String::from(shell.linha_atual());
        AppTerminal::com_shell(shell, |console| {
            console.escrever_texto(PROMPT);
            console.escrever_texto(&linha);
        })
    }

    fn com_shell(mut shell: Shell, preparar: impl FnOnce(&mut Console)) -> AppTerminal {
        let mut superficie = Box::new(Superficie::nova(LARGURA, ALTURA, FUNDO));
        let tela = superficie.tela();
        // o texto fica MARGEM pixels para dentro da borda
        let area = tela.regiao(MARGEM, MARGEM, COLUNAS * 8, LINHAS * 16).unwrap_or(tela);
        let mut console = Console::new(area, Escala::CONSOLE, FRENTE, FUNDO);
        preparar(&mut console);
        // o console antigo (tela cheia do boot) e descartado aqui
        let _ = shell.terminal().trocar_console(Some(console));
        AppTerminal { shell: Some(shell), superficie }
    }

    /// Tira o console da janela (os pixels vao embora com ela) e guarda o shell.
    fn guardar_shell(&mut self) {
        if let Some(mut shell) = self.shell.take() {
            let _ = shell.terminal().trocar_console(None);
            GUARDADO.colocar(shell);
        }
    }
}

impl App for AppTerminal {
    fn titulo(&self) -> &str {
        TITULO
    }

    fn tamanho(&self) -> (usize, usize) {
        (LARGURA, ALTURA)
    }

    fn desenhar(&mut self, conteudo: &mut Superficie) {
        self.superficie.copiar_para(conteudo, 0, 0);
    }

    fn evento(&mut self, e: EventoApp) -> bool {
        match (e, &mut self.shell) {
            // o shell desenha direto no console; a janela so precisa copiar
            (EventoApp::Caractere(c), Some(shell)) => {
                // o shell trabalha com os bytes UTF-8 da linha
                let mut bytes = [0u8; 4];
                for byte in c.encode_utf8(&mut bytes).bytes() {
                    shell.tecla(byte);
                }
                true
            }
            (EventoApp::Processos, Some(shell)) => shell.atender_processos(),
            _ => false,
        }
    }

    fn fechar(&mut self) {
        self.guardar_shell();
    }
}

impl Drop for AppTerminal {
    /// Se a janela sumir sem `fechar`, o shell tambem nao se perde.
    fn drop(&mut self) {
        self.guardar_shell();
    }
}

/// Janela pequena para quando o terminal ja esta aberto.
struct Aviso;

impl App for Aviso {
    fn titulo(&self) -> &str {
        "Terminal: aviso"
    }

    fn tamanho(&self) -> (usize, usize) {
        (320, 56)
    }

    fn desenhar(&mut self, conteudo: &mut Superficie) {
        conteudo.limpar(FUNDO);
        conteudo.texto(14, 14, "O terminal já está aberto.", FRENTE, 1);
        conteudo.texto(14, 34, "Só existe um shell no MXOS.", FRENTE, 1);
    }

    fn evento(&mut self, _e: EventoApp) -> bool {
        false
    }
}
