//! Area de trabalho do MXOS (so no QEMU por enquanto).
//!
//!   - grafico: Retangulo e Superficie (buffer de pixels na RAM) e apresentar
//!   - cursor:  a seta do mouse
//!   - tema:    cores, papel de parede e medidas das janelas
//!   - janelas: o Gerenciador (janelas, atalhos, barra de tarefas, menu)
//!   - area:    papel de parede, atalhos, barra de tarefas e menu iniciar
//!   - widgets: botoes, campos, listas, abas, icones e graficos dos apps
//!   - apps:    os programas que moram nas janelas (arquivos, navegador...)
//!
//! Tudo e desenhado num buffer de tras no heap; so a parte que mudou vai
//! para o framebuffer. Entre um quadro e outro a CPU dorme no WFI.
//!
//! Antes do primeiro quadro o laco le as configuracoes (/config/mxos.cfg):
//! tema, papel de parede e layout do teclado voltam como estavam.

pub mod apps;
pub mod area;
pub mod cursor;
pub mod grafico;
pub mod janelas;
pub mod tema;
pub mod widgets;

use alloc::boxed::Box;
use core::fmt::Write;

use crate::config::{self, Config};
use crate::entrada::{Entrada, Layout};
use crate::shell::Shell;
use crate::uart::Uart;
use crate::video::tela::Tela;
use crate::{drivers, excecao, irq, processo, rede, timer};
use apps::terminal::{self, AppTerminal};
use grafico::Superficie;
use janelas::{EventoApp, Gerenciador, Pedido};
use widgets::Icone;

/// O Tique dos apps: a cada 10 tiques do timer (10 vezes por segundo).
const TIQUES_POR_TIQUE_APP: u64 = timer::HZ / 10;
/// Quantos bytes da serial entram por volta do laco.
const MAX_BYTES_SERIAL: usize = 256;

/// Laco principal da area de trabalho. O shell vai morar na janela do
/// terminal; os comandos que chegam pela serial vao para ela.
pub fn rodar(mut shell: Shell, mut entrada: Entrada, framebuffer: Tela) -> ! {
    // daqui em diante um erro fatal tambem aparece na tela
    excecao::registrar_tela(framebuffer);
    let cfg = Config::carregar();
    config::aplicar_aparencia(&cfg);
    entrada.definir_layout(config::layout(&cfg));
    drivers::definir_entrada(entrada.descricoes());
    anunciar_config(&mut shell, &cfg, entrada.layout());

    let (largura, altura) = (framebuffer.largura(), framebuffer.altura());
    let mut fundo = Superficie::nova(largura, altura, tema::atual().area_cima);
    let mut gerenciador = Gerenciador::novo(largura as i32, altura as i32);
    apps::registrar_apps_basicos(&mut gerenciador);
    gerenciador.registrar_app(terminal::TITULO, Icone::Terminal, terminal::criar);
    apps::registrar_atalhos(&mut gerenciador);

    let tela = (largura as i32, altura as i32);
    let (x, y) = posicao_do_terminal(tela);
    let mut id_terminal = gerenciador.abrir(Box::new(AppTerminal::nova(shell)), x, y);
    // o primeiro quadro sai antes de ler a serial: um comando que chegue
    // junto com o prompt nao deixa a tela parada no console do boot
    mostrar(&mut gerenciador, &mut fundo, &framebuffer);
    let mut proximo_tique = timer::tiques() + TIQUES_POR_TIQUE_APP;
    let mut utf8 = Utf8::default();

    loop {
        id_terminal = repassar_serial(&mut gerenciador, id_terminal, tela, &mut utf8);
        while let Some(evento) = entrada.proximo() {
            gerenciador.evento(evento);
        }
        atender_externos(&mut gerenciador, &mut entrada);
        rede::processar();
        if timer::tiques() >= proximo_tique {
            proximo_tique = timer::tiques() + TIQUES_POR_TIQUE_APP;
            gerenciador.tique();
            atender_externos(&mut gerenciador, &mut entrada);
        }
        // os processos rodam ate o proximo tique; o shell da janela mostra a saida
        gerenciador.enviar(id_terminal, EventoApp::Processos);
        mostrar(&mut gerenciador, &mut fundo, &framebuffer);
        // o tique de 100 Hz acorda; teclado e mouse sao lidos na volta.
        // Com processo pronto a CPU nao dorme: ele usa o tempo ate o tique.
        if !processo::tem_prontos() {
            irq::cochilar();
        }
    }
}

/// Linha de estado do boot (serial e console da tela) com o que veio das
/// configuracoes.
fn anunciar_config(shell: &mut Shell, cfg: &Config, layout: Layout) {
    let origem = if cfg.do_disco() { "lido de /config/mxos.cfg" } else { "padrão, sem /config/mxos.cfg" };
    let _ = writeln!(
        shell.terminal(),
        "config  : tema {}, papel de parede {}, teclado {}; {origem}",
        tema::nome_atual().unwrap_or("?"),
        tema::aparencia().papel.rotulo(),
        layout.descricao()
    );
}

/// Pedidos dos apps que so este laco atende (ele e o dono da Entrada).
fn atender_externos(gerenciador: &mut Gerenciador, entrada: &mut Entrada) {
    for pedido in gerenciador.pedidos_externos() {
        if let Pedido::DefinirLayout(layout) = pedido {
            entrada.definir_layout(layout);
        }
    }
}

/// Monta o que mudou no buffer de tras e passa so essa area para a tela.
fn mostrar(gerenciador: &mut Gerenciador, fundo: &mut Superficie, framebuffer: &Tela) {
    if let Some(area) = gerenciador.compor(fundo) {
        grafico::apresentar(fundo, framebuffer, area);
    }
}

/// Centraliza a janela do terminal acima da barra de tarefas.
fn posicao_do_terminal((largura, altura): (i32, i32)) -> (i32, i32) {
    let (conteudo_l, conteudo_a) = (terminal::LARGURA as i32, terminal::ALTURA as i32);
    let janela_l = conteudo_l + 2 * tema::BORDA;
    let janela_a = conteudo_a + tema::ALTURA_TITULO + 2 * tema::BORDA;
    let livre_a = altura - tema::ALTURA_BARRA_TAREFAS;
    (((largura - janela_l) / 2).max(0), ((livre_a - janela_a) / 2).max(0))
}

/// Entrega os bytes da serial ao terminal como teclas. Se a janela foi
/// fechada, reabre (com o mesmo shell) para o comando nao se perder.
/// Devolve o id da janela que recebeu.
fn repassar_serial(gerenciador: &mut Gerenciador, id: usize, tela: (i32, i32), utf8: &mut Utf8) -> usize {
    let mut uart = Uart::new();
    let Some(primeiro) = uart.tentar_receber() else { return id };
    let id = janela_do_terminal(gerenciador, id, tela);
    let mut proximo = Some(primeiro);
    for _ in 0..MAX_BYTES_SERIAL {
        let Some(byte) = proximo else { break };
        if let Some(c) = utf8.empurrar(byte) {
            gerenciador.enviar(id, EventoApp::Caractere(c));
        }
        proximo = uart.tentar_receber();
    }
    id
}

/// Junta os bytes UTF-8 da serial em caracteres: um caractere acentuado
/// pode chegar partido entre duas voltas do laco.
#[derive(Default)]
struct Utf8 {
    bytes: [u8; 4],
    tem: usize,
}

impl Utf8 {
    /// Devolve o caractere quando ele fica completo. Lixo vira '?'.
    fn empurrar(&mut self, byte: u8) -> Option<char> {
        if byte < 0x80 {
            self.tem = 0;
            return Some(byte as char);
        }
        if byte >= 0xC0 {
            self.bytes[0] = byte;
            self.tem = 1;
            return None;
        }
        if self.tem == 0 || self.tem == 4 {
            return Some('?');
        }
        self.bytes[self.tem] = byte;
        self.tem += 1;
        let precisa = match self.bytes[0] {
            0xC0..=0xDF => 2,
            0xE0..=0xEF => 3,
            _ => 4,
        };
        if self.tem < precisa {
            return None;
        }
        self.tem = 0;
        let texto = core::str::from_utf8(&self.bytes[..precisa]).ok();
        Some(texto.and_then(|t| t.chars().next()).unwrap_or('?'))
    }
}

/// A janela do terminal: a ultima conhecida, outra com o mesmo titulo ou
/// uma nova.
fn janela_do_terminal(gerenciador: &mut Gerenciador, id: usize, tela: (i32, i32)) -> usize {
    let janelas = gerenciador.janelas();
    if janelas.iter().any(|j| j.id == id) {
        return id;
    }
    if let Some(janela) = janelas.iter().find(|j| j.titulo == terminal::TITULO) {
        return janela.id;
    }
    let (x, y) = posicao_do_terminal(tela);
    gerenciador.abrir(terminal::criar(), x, y)
}
