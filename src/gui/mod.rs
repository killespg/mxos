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
//! Tudo e montado numa cena no heap (a tela sem o cursor); so os pedacos
//! que mudaram vao para o framebuffer, cada um separado, e o cursor e o
//! contorno do arrasto sao desenhados direto nele. Entre um quadro e outro
//! a CPU dorme no WFI.
//!
//! Antes do primeiro quadro o laco le as configuracoes (/config/mxos.cfg):
//! tema, papel de parede e layout do teclado voltam como estavam.

pub mod apps;
pub mod realce;
pub mod area;
pub mod cursor;
pub mod grafico;
pub mod janelas;
pub mod tema;
pub mod transferencia;
pub mod widgets;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt::Write;

use crate::config::{self, Config};
use crate::entrada::{Entrada, Layout};
use crate::shell::Shell;
use crate::uart::Uart;
use crate::video::tela::Tela;
use crate::{drivers, excecao, processo, rede, smp, timer, x11};
use apps::janela_x::JanelaX;
use apps::terminal::{self, AppTerminal};
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
    tema::definir_tela(largura as i32, altura as i32);
    let mut gerenciador = Gerenciador::novo(largura as i32, altura as i32);
    apps::registrar_apps_basicos(&mut gerenciador);
    gerenciador.registrar_app(terminal::TITULO, Icone::Terminal, terminal::criar);
    apps::registrar_atalhos(&mut gerenciador);
    // programas de Linux com janela falam X11 em /tmp/.X11-unix/X0
    x11::iniciar(largura, altura);
    // os outros nucleos rodam processos; daqui em diante o nucleo 0 tem a
    // trava do kernel e so solta para rodar programas e dormir
    let nucleos = smp::ligar_nucleos();
    let _ = writeln!(shell.terminal(), "cpu     : {nucleos} núcleo(s) rodando programas");
    let mut janelas_x: Vec<usize> = Vec::new();

    let tela = (largura as i32, altura as i32);
    let (x, y) = posicao_do_terminal(tela);
    let mut id_terminal = gerenciador.abrir(Box::new(AppTerminal::nova(shell)), x, y);
    // modo quiosque: o painel em tela cheia por cima de tudo
    if Config::carregar().obter("quiosque").is_some_and(|v| v.trim() == "sim") {
        gerenciador.abrir(apps::quiosque::criar(), 0, 0);
    }
    // o primeiro quadro sai antes de ler a serial: um comando que chegue
    // junto com o prompt nao deixa a tela parada no console do boot
    gerenciador.compor(&framebuffer);
    let mut proximo_tique = timer::tiques() + TIQUES_POR_TIQUE_APP;
    let mut utf8 = Utf8::default();

    loop {
        id_terminal = repassar_serial(&mut gerenciador, id_terminal, tela, &mut utf8);
        while let Some(evento) = entrada.proximo() {
            gerenciador.evento(evento);
        }
        atender_externos(&mut gerenciador, &mut entrada);
        rede::processar();
        crate::nas::atender();
        if timer::tiques() >= proximo_tique {
            proximo_tique = timer::tiques() + TIQUES_POR_TIQUE_APP;
            gerenciador.tique();
            crate::frequencia::vigiar();
            atender_externos(&mut gerenciador, &mut entrada);
        }
        // os processos rodam ate o proximo tique; o shell da janela mostra a saida
        gerenciador.enviar(id_terminal, EventoApp::Processos);
        gerenciador.quadro();
        atender_x11(&mut gerenciador, &mut janelas_x, (largura as i32, altura as i32));
        crate::audio::dsp::bombear();
        gerenciador.compor(&framebuffer);
        // o tique de 100 Hz acorda; teclado e mouse sao lidos na volta.
        // Com processo pronto a CPU nao dorme: ele usa o tempo ate o tique.
        if processo::tem_prontos() {
            smp::acordar_todos(); // os outros nucleos pegam o que estiver pronto
        } else {
            smp::ocioso();
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

/// O servidor X11: atende os programas, abre uma janela do MXOS para cada
/// janela X nova e avisa as abertas (redesenham se o programa desenhou).
fn atender_x11(gerenciador: &mut Gerenciador, janelas: &mut Vec<usize>, (largura, altura): (i32, i32)) {
    x11::atender();
    for id in x11::janelas_novas() {
        let app = JanelaX::nova(id);
        let (l, a) = app.tamanho_inicial();
        let x = ((largura - l as i32) / 2).max(0);
        let y = ((altura - tema::ALTURA_BARRA_TAREFAS - a as i32 - tema::ALTURA_TITULO) / 2).max(0);
        janelas.push(gerenciador.abrir(Box::new(app), x, y));
    }
    let abertas = gerenciador.janelas();
    janelas.retain(|j| abertas.iter().any(|a| a.id == *j));
    for &j in janelas.iter() {
        gerenciador.enviar(j, EventoApp::Processos);
    }
}

/// Pedidos dos apps que so este laco atende (ele e o dono da Entrada).
fn atender_externos(gerenciador: &mut Gerenciador, entrada: &mut Entrada) {
    for pedido in gerenciador.pedidos_externos() {
        if let Pedido::DefinirLayout(layout) = pedido {
            entrada.definir_layout(layout);
        }
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
