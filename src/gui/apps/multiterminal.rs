//! Terminal com abas e divisoes (estilo tmux) e um monitor de CPU e RAM.
//!
//! Cada aba tem ate 4 paineis: shells do MXOS (cada um com o proprio
//! console e a propria linha de comando) ou o monitor. As divisoes sao
//! fixas por quantidade: 1 painel ocupa tudo; 2 ficam lado a lado ou um
//! em cima do outro; 3 = um grande a esquerda e dois a direita; 4 = grade.
//!
//! Teclas (Ctrl+B e depois a tecla, como no tmux):
//!   c nova aba, n/p proxima/anterior, 1-9 vai para a aba,
//!   % divide lado a lado, " divide em cima/embaixo, o proximo painel,
//!   x fecha o painel, m poe o monitor num painel novo.
//! O mouse escolhe aba (barra de cima) e painel (clique nele); os botoes
//! da barra fazem o mesmo que as teclas.
//!
//! Os programas de Linux usam um tty so: um painel por vez pode ter um
//! programa em primeiro plano (o shell avisa nos outros).

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::entrada::Botao;
use crate::entrada::teclado::{
    self, KEY_DELETE, KEY_DOWN, KEY_END, KEY_HOME, KEY_LEFT, KEY_LEFTCTRL, KEY_PAGEDOWN, KEY_PAGEUP, KEY_RIGHT,
    KEY_RIGHTCTRL, KEY_UP,
};
use crate::gui::grafico::{Cor, Retangulo, Superficie};
use crate::gui::janelas::{App, EventoApp};
use crate::shell::{PROMPT, Shell};
use crate::video::tela::{Console, Escala};
use crate::{cpu, heap, irq, processo};

pub const TITULO: &str = "Terminal com abas";
const LARGURA: i32 = 960;
const ALTURA: i32 = 600;
const ALTURA_ABAS: i32 = 26;
const MARGEM: i32 = 3;
const MAX_PAINEIS: usize = 4;
const MAX_ABAS: usize = 9;
const AMOSTRAS: usize = 60;

const FRENTE: Cor = 0x00E0_E0E0;
const FUNDO: Cor = 0x0010_1418;
const BARRA: Cor = 0x0020_262E;
const ABA_ATIVA: Cor = 0x0030_3A46;
const DESTAQUE: Cor = 0x0040_A0FF;
const BORDA: Cor = 0x0038_4048;
const APAGADO: Cor = 0x0080_8890;
const VERDE: Cor = 0x0050_D070;
const AMARELO: Cor = 0x00E0_C040;

pub fn criar() -> Box<dyn App> {
    Box::new(MultiTerminal::novo())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Divisao {
    /// Paineis lado a lado.
    Vertical,
    /// Paineis um em cima do outro.
    Horizontal,
}

struct PainelShell {
    shell: Shell,
    /// O console desenha aqui (Box: o endereco nao muda).
    sup: Box<Superficie>,
}

/// Uso da CPU por segundo e a memoria do heap.
struct Monitor {
    cpu: [u32; AMOSTRAS],
    ocioso: u64,
    desde_inicio: u64,
    proxima: u64,
}

enum Painel {
    Shell(PainelShell),
    Monitor(Monitor),
}

struct Aba {
    paineis: Vec<Painel>,
    divisao: Divisao,
    foco: usize,
    /// Numero mostrado na barra.
    numero: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Botao_ {
    NovaAba,
    DividirLado,
    DividirBaixo,
    Monitor,
    Fechar,
}

const BOTOES: [(Botao_, &str); 5] = [
    (Botao_::NovaAba, "+ aba"),
    (Botao_::DividirLado, "| dividir"),
    (Botao_::DividirBaixo, "— dividir"),
    (Botao_::Monitor, "monitor"),
    (Botao_::Fechar, "x painel"),
];

pub struct MultiTerminal {
    abas: Vec<Aba>,
    atual: usize,
    ctrl: bool,
    /// Ctrl+B apertado: a proxima tecla e um comando.
    prefixo: bool,
    proximo_numero: usize,
    aviso: Option<String>,
}

fn area_paineis() -> Retangulo {
    Retangulo::novo(0, ALTURA_ABAS, LARGURA, ALTURA - ALTURA_ABAS)
}

/// Retangulos dos `n` paineis dentro de `r`.
fn layout(r: Retangulo, n: usize, d: Divisao) -> Vec<Retangulo> {
    let (x, y, l, a) = (r.x, r.y, r.largura, r.altura);
    match n {
        0 | 1 => alloc::vec![r],
        2 if d == Divisao::Vertical => alloc::vec![Retangulo::novo(x, y, l / 2, a), Retangulo::novo(x + l / 2, y, l - l / 2, a)],
        2 => alloc::vec![Retangulo::novo(x, y, l, a / 2), Retangulo::novo(x, y + a / 2, l, a - a / 2)],
        3 => alloc::vec![
            Retangulo::novo(x, y, l / 2, a),
            Retangulo::novo(x + l / 2, y, l - l / 2, a / 2),
            Retangulo::novo(x + l / 2, y + a / 2, l - l / 2, a - a / 2),
        ],
        _ => alloc::vec![
            Retangulo::novo(x, y, l / 2, a / 2),
            Retangulo::novo(x + l / 2, y, l - l / 2, a / 2),
            Retangulo::novo(x, y + a / 2, l / 2, a - a / 2),
            Retangulo::novo(x + l / 2, y + a / 2, l - l / 2, a - a / 2),
        ],
    }
}

/// Area de texto dentro de um painel (fora a borda), em celulas de 8x16.
fn area_texto(r: Retangulo) -> (usize, usize) {
    let colunas = ((r.largura - 2 * MARGEM - 2) / 8).max(8) as usize;
    let linhas = ((r.altura - 2 * MARGEM - 2) / 16).max(2) as usize;
    (colunas * 8, linhas * 16)
}

impl PainelShell {
    fn novo(r: Retangulo) -> PainelShell {
        let (l, a) = area_texto(r);
        let mut sup = Box::new(Superficie::nova(l, a, FUNDO));
        let console = Console::new(sup.tela(), Escala::CONSOLE, FRENTE, FUNDO);
        let mut shell = Shell::painel(console);
        shell.banner_curto();
        PainelShell { shell, sup }
    }

    /// O painel mudou de tamanho: console novo (o texto antigo some) com o
    /// prompt e o que ja estava digitado.
    fn redimensionar(&mut self, r: Retangulo) {
        let (l, a) = area_texto(r);
        if self.sup.largura() == l && self.sup.altura() == a {
            return;
        }
        let mut sup = Box::new(Superficie::nova(l, a, FUNDO));
        let mut console = Console::new(sup.tela(), Escala::CONSOLE, FRENTE, FUNDO);
        if !self.shell.ocupado() {
            console.escrever_texto(PROMPT);
            console.escrever_texto(self.shell.linha_atual());
        }
        // o console velho aponta para a superficie velha: sai antes dela
        drop(self.shell.terminal().trocar_console(Some(console)));
        self.sup = sup;
    }

    fn tecla(&mut self, e: EventoApp, ctrl: bool) -> bool {
        match e {
            EventoApp::Caractere(c) => {
                let mut b = [0u8; 4];
                for byte in c.encode_utf8(&mut b).bytes() {
                    self.shell.tecla(byte);
                }
                true
            }
            EventoApp::Tecla { codigo, pressionada: true } if self.shell.ocupado() => {
                let seq: &[u8] = match codigo {
                    KEY_UP => b"\x1b[A",
                    KEY_DOWN => b"\x1b[B",
                    KEY_RIGHT => b"\x1b[C",
                    KEY_LEFT => b"\x1b[D",
                    KEY_HOME => b"\x1b[H",
                    KEY_END => b"\x1b[F",
                    KEY_DELETE => b"\x1b[3~",
                    KEY_PAGEUP => b"\x1b[5~",
                    KEY_PAGEDOWN => b"\x1b[6~",
                    _ => match teclado::letra(codigo).filter(|_| ctrl) {
                        Some(letra) => {
                            self.shell.tecla(letra & 0x1F);
                            return true;
                        }
                        None => return false,
                    },
                };
                for &b in seq {
                    self.shell.tecla(b);
                }
                true
            }
            _ => false,
        }
    }
}

impl Monitor {
    fn novo() -> Monitor {
        let c = irq::contadores();
        Monitor { cpu: [0; AMOSTRAS], ocioso: c.ocioso, desde_inicio: c.desde_inicio, proxima: cpu::contador() + cpu::frequencia_timer() }
    }

    /// Uma amostra por segundo. true = mudou.
    fn amostrar(&mut self) -> bool {
        let agora = cpu::contador();
        if agora < self.proxima {
            return false;
        }
        self.proxima = agora + cpu::frequencia_timer();
        let c = irq::contadores();
        let passou = c.desde_inicio.wrapping_sub(self.desde_inicio).max(1);
        let dormiu = c.ocioso.wrapping_sub(self.ocioso).min(passou);
        self.ocioso = c.ocioso;
        self.desde_inicio = c.desde_inicio;
        self.cpu.rotate_left(1);
        self.cpu[AMOSTRAS - 1] = (1000 - dormiu * 1000 / passou) as u32;
        true
    }

    fn desenhar(&self, s: &mut Superficie, r: Retangulo) {
        let x = r.x + 10;
        let mut y = r.y + 10;
        let uso = self.cpu[AMOSTRAS - 1];
        s.texto(x, y, &format!("CPU  {}.{}%", uso / 10, uso % 10), FRENTE, 1);
        y += 16;
        // grafico da CPU (ultimos 60 s)
        let g = Retangulo::novo(x, y, (r.largura - 20).max(60), ((r.altura - 120) / 2).max(40));
        s.retangulo(g, 0x0018_1E24);
        s.contorno(g, BORDA, 1);
        let passo = (g.largura as f32 / AMOSTRAS as f32).max(1.0);
        for (k, &v) in self.cpu.iter().enumerate() {
            let h = (v as i32 * (g.altura - 2)) / 1000;
            let bx = g.x + 1 + (k as f32 * passo) as i32;
            let bl = (passo as i32).max(1);
            s.retangulo(Retangulo::novo(bx, g.y + g.altura - 1 - h, bl, h), if v > 800 { AMARELO } else { VERDE });
        }
        y = g.y + g.altura + 14;
        // memoria
        let h = heap::estatisticas();
        let usada = h.total - h.livre;
        let pct = if h.total > 0 { usada * 1000 / h.total } else { 0 };
        s.texto(x, y, &format!("RAM  {} de {} ({}.{}%)", mb(usada), mb(h.total), pct / 10, pct % 10), FRENTE, 1);
        y += 16;
        let barra = Retangulo::novo(x, y, g.largura, 14);
        s.retangulo(barra, 0x0018_1E24);
        s.retangulo(Retangulo::novo(barra.x, barra.y, (barra.largura as usize * pct / 1000) as i32, barra.altura), DESTAQUE);
        s.contorno(barra, BORDA, 1);
        y += 26;
        s.texto(x, y, &format!("maior bloco livre {}, {} alocações", mb(h.maior_bloco), h.alocacoes), APAGADO, 1);
        y += 18;
        let procs = processo::lista();
        s.texto(x, y, &format!("processos: {}", procs.len()), FRENTE, 1);
        y += 16;
        for p in procs.iter().take(((r.y + r.altura - y) / 14).max(0) as usize) {
            s.texto(x + 8, y, &format!("{:>5}  {}", p.pid, p.nome), APAGADO, 1);
            y += 14;
        }
    }
}

fn mb(bytes: usize) -> String {
    if bytes >= 1 << 20 { format!("{}.{} MB", bytes >> 20, (bytes & 0xFFFFF) * 10 >> 20) } else { format!("{} KB", bytes >> 10) }
}

impl Aba {
    fn nova(numero: usize) -> Aba {
        let r = area_paineis();
        Aba { paineis: alloc::vec![Painel::Shell(PainelShell::novo(r))], divisao: Divisao::Vertical, foco: 0, numero }
    }

    fn retangulos(&self) -> Vec<Retangulo> {
        layout(area_paineis(), self.paineis.len(), self.divisao)
    }

    /// Depois de mudar a quantidade de paineis: ajusta os consoles.
    fn arrumar(&mut self) {
        let rs = self.retangulos();
        for (p, r) in self.paineis.iter_mut().zip(rs) {
            if let Painel::Shell(ps) = p {
                ps.redimensionar(r);
            }
        }
    }

    fn adicionar(&mut self, painel: impl FnOnce(Retangulo) -> Painel, divisao: Divisao) -> bool {
        if self.paineis.len() >= MAX_PAINEIS {
            return false;
        }
        if self.paineis.len() == 1 {
            self.divisao = divisao;
        }
        let n = self.paineis.len() + 1;
        let r = layout(area_paineis(), n, self.divisao)[n - 1];
        self.paineis.push(painel(r));
        self.foco = n - 1;
        self.arrumar();
        true
    }
}

impl MultiTerminal {
    pub fn novo() -> MultiTerminal {
        MultiTerminal { abas: alloc::vec![Aba::nova(1)], atual: 0, ctrl: false, prefixo: false, proximo_numero: 2, aviso: None }
    }

    fn aba(&mut self) -> &mut Aba {
        &mut self.abas[self.atual]
    }

    fn nova_aba(&mut self) {
        if self.abas.len() >= MAX_ABAS {
            self.aviso = Some(format!("no máximo {MAX_ABAS} abas"));
            return;
        }
        self.abas.push(Aba::nova(self.proximo_numero));
        self.proximo_numero += 1;
        self.atual = self.abas.len() - 1;
    }

    fn dividir(&mut self, d: Divisao) {
        if !self.aba().adicionar(|r| Painel::Shell(PainelShell::novo(r)), d) {
            self.aviso = Some(format!("no máximo {MAX_PAINEIS} painéis por aba"));
        }
    }

    fn monitor(&mut self) {
        if !self.aba().adicionar(|_| Painel::Monitor(Monitor::novo()), Divisao::Vertical) {
            self.aviso = Some(format!("no máximo {MAX_PAINEIS} painéis por aba"));
        }
    }

    /// Fecha o painel com foco; a aba some junto com o ultimo painel.
    fn fechar_painel(&mut self) {
        let aba = self.aba();
        aba.paineis.remove(aba.foco);
        if aba.paineis.is_empty() {
            self.abas.remove(self.atual);
            if self.abas.is_empty() {
                self.abas.push(Aba::nova(self.proximo_numero));
                self.proximo_numero += 1;
            }
            self.atual = self.atual.min(self.abas.len() - 1);
            return;
        }
        aba.foco = aba.foco.min(aba.paineis.len() - 1);
        aba.arrumar();
    }

    fn executar(&mut self, b: Botao_) {
        match b {
            Botao_::NovaAba => self.nova_aba(),
            Botao_::DividirLado => self.dividir(Divisao::Vertical),
            Botao_::DividirBaixo => self.dividir(Divisao::Horizontal),
            Botao_::Monitor => self.monitor(),
            Botao_::Fechar => self.fechar_painel(),
        }
    }

    /// Comando depois do Ctrl+B.
    fn comando(&mut self, c: char) -> bool {
        match c {
            'c' => self.nova_aba(),
            'n' => self.atual = (self.atual + 1) % self.abas.len(),
            'p' => self.atual = (self.atual + self.abas.len() - 1) % self.abas.len(),
            '%' => self.dividir(Divisao::Vertical),
            '"' => self.dividir(Divisao::Horizontal),
            'o' => {
                let aba = self.aba();
                aba.foco = (aba.foco + 1) % aba.paineis.len();
            }
            'x' => self.fechar_painel(),
            'm' => self.monitor(),
            d @ '1'..='9' => {
                let i = d as usize - '1' as usize;
                if i < self.abas.len() {
                    self.atual = i;
                }
            }
            _ => return false,
        }
        true
    }

    /// Retangulos da barra: abas e botoes.
    fn barra(&self) -> (Vec<Retangulo>, Vec<(Botao_, Retangulo)>) {
        let mut x = MARGEM;
        let abas = self
            .abas
            .iter()
            .map(|a| {
                let r = Retangulo::novo(x, 2, Superficie::largura_texto(&format!("{}: shell", a.numero), 1) + 16, ALTURA_ABAS - 4);
                x = r.direita() + 2;
                r
            })
            .collect();
        let mut fim = LARGURA - MARGEM;
        let mut botoes: Vec<(Botao_, Retangulo)> = BOTOES
            .iter()
            .rev()
            .map(|&(b, t)| {
                let l = Superficie::largura_texto(t, 1) + 12;
                let r = Retangulo::novo(fim - l, 2, l, ALTURA_ABAS - 4);
                fim = r.x - 4;
                (b, r)
            })
            .collect();
        botoes.reverse();
        (abas, botoes)
    }

    fn clique(&mut self, x: i32, y: i32) -> bool {
        if y < ALTURA_ABAS {
            let (abas, botoes) = self.barra();
            if let Some(i) = abas.iter().position(|r| r.contem(x, y)) {
                self.atual = i;
                return true;
            }
            if let Some((b, _)) = botoes.iter().find(|(_, r)| r.contem(x, y)) {
                self.executar(*b);
                return true;
            }
            return false;
        }
        let rs = self.aba().retangulos();
        if let Some(i) = rs.iter().position(|r| r.contem(x, y)) {
            self.aba().foco = i;
            return true;
        }
        false
    }

    fn desenhar_barra(&self, s: &mut Superficie) {
        s.retangulo(Retangulo::novo(0, 0, LARGURA, ALTURA_ABAS), BARRA);
        let (abas, botoes) = self.barra();
        for (i, (a, r)) in self.abas.iter().zip(abas).enumerate() {
            let ativa = i == self.atual;
            s.retangulo(r, if ativa { ABA_ATIVA } else { BARRA });
            if ativa {
                s.retangulo(Retangulo::novo(r.x, r.y + r.altura - 2, r.largura, 2), DESTAQUE);
            }
            s.texto(r.x + 8, r.y + 7, &format!("{}: shell", a.numero), if ativa { FRENTE } else { APAGADO }, 1);
        }
        for (b, r) in botoes {
            s.contorno(r, BORDA, 1);
            let t = BOTOES.iter().find(|(x, _)| *x == b).map_or("", |x| x.1);
            s.texto(r.x + 6, r.y + 7, t, FRENTE, 1);
        }
        let info = if self.prefixo {
            String::from("Ctrl+B: c n p % \" o x m 1-9")
        } else if let Some(a) = &self.aviso {
            a.clone()
        } else {
            String::new()
        };
        if !info.is_empty() {
            let fim = abas_fim(&self.barra().0);
            s.texto(fim + 12, 9, &info, AMARELO, 1);
        }
    }
}

fn abas_fim(abas: &[Retangulo]) -> i32 {
    abas.last().map_or(0, |r| r.direita())
}

impl App for MultiTerminal {
    fn titulo(&self) -> &str {
        TITULO
    }

    fn tamanho(&self) -> (usize, usize) {
        (LARGURA as usize, ALTURA as usize)
    }

    fn desenhar(&mut self, s: &mut Superficie) {
        s.limpar(FUNDO);
        self.desenhar_barra(s);
        let aba = &self.abas[self.atual];
        for (i, (p, r)) in aba.paineis.iter().zip(aba.retangulos()).enumerate() {
            let foco = i == aba.foco;
            s.contorno(r, if foco { DESTAQUE } else { BORDA }, 1);
            match p {
                Painel::Shell(ps) => ps.sup.copiar_para(s, r.x + MARGEM + 1, r.y + MARGEM + 1),
                Painel::Monitor(m) => m.desenhar(s, r),
            }
        }
    }

    fn evento(&mut self, e: EventoApp) -> bool {
        match e {
            EventoApp::Tecla { codigo: KEY_LEFTCTRL | KEY_RIGHTCTRL, pressionada } => {
                self.ctrl = pressionada;
                false
            }
            // Ctrl+B: prefixo dos comandos
            EventoApp::Tecla { codigo, pressionada: true } if self.ctrl && teclado::letra(codigo) == Some(b'b') => {
                self.prefixo = true;
                self.aviso = None;
                true
            }
            EventoApp::Caractere(c) if self.prefixo => {
                self.prefixo = false;
                self.comando(c);
                true
            }
            EventoApp::Botao { x, y, botao: Botao::Esquerdo, pressionado: true } => {
                self.aviso = None;
                self.clique(x, y)
            }
            EventoApp::Tique => {
                // a saida dos programas e o monitor
                let mut mudou = false;
                let atual = self.atual;
                for (k, aba) in self.abas.iter_mut().enumerate() {
                    for p in &mut aba.paineis {
                        match p {
                            Painel::Shell(ps) => mudou |= ps.shell.atender_processos() && k == atual,
                            Painel::Monitor(m) => mudou |= m.amostrar() && k == atual,
                        }
                    }
                }
                mudou
            }
            EventoApp::Caractere(_) | EventoApp::Tecla { .. } => {
                let ctrl = self.ctrl;
                let aba = self.aba();
                match aba.paineis.get_mut(aba.foco) {
                    Some(Painel::Shell(ps)) => ps.tecla(e, ctrl),
                    _ => false,
                }
            }
            _ => false,
        }
    }
}
