//! Gerenciador de janelas do MXOS.
//!
//! Camadas, de baixo para cima: papel de parede, janelas (na ordem de
//! empilhamento, a de cima por ultimo), barra de tarefas e menu iniciar.
//! Tudo isso e montado na "cena", um buffer do tamanho da tela SEM o
//! cursor, e so os pedacos que mudaram (areas sujas) sao refeitos.
//!
//! O cursor vai por cima, direto no buffer de destino: mexer o mouse so
//! devolve a cena embaixo do cursor velho e desenha o novo (dois
//! retangulos pequenos), sem refazer nada da cena.
//!
//! Cada janela guarda a propria Superficie de conteudo e o desenho pronto
//! da barra de titulo. O app so redesenha o conteudo quando pede (evento
//! devolveu true); a barra de titulo so muda com o foco ou o titulo.
//!
//! Os apps falam com o gerenciador por Pedidos: depois de cada evento
//! entregue (Tique tambem) o gerenciador le App::pedidos e atende (abrir e
//! fechar janelas, abrir um arquivo, redesenhar tudo...). O que so o laco
//! principal sabe fazer (trocar o layout do teclado) fica guardado para
//! ele em pedidos_externos.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::UnsafeCell;

use super::apps;
use super::area::{self, AlvoBarra, Atalhos, BarraTarefas, Criador, ItemBarra, MenuIniciar};
use super::cursor;
use super::grafico::{self, Cor, Retangulo, Superficie};
use super::tema::{self, ALTURA_BARRA_TAREFAS, ALTURA_TITULO, Aparencia, BORDA, Tema};
use super::widgets::Icone;
use crate::cpu;
use crate::entrada::{Botao, Evento, Layout};
use crate::fonte::{self, INTER_NEGRITO};

/// Evento entregue a um app. Coordenadas relativas ao canto do conteudo.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EventoApp {
    Caractere(char),
    Tecla { codigo: u16, pressionada: bool },
    /// botoes: bit 0 esquerdo, bit 1 direito, bit 2 meio.
    Mouse { x: i32, y: i32, botoes: u8 },
    Botao { x: i32, y: i32, botao: Botao, pressionado: bool },
    Roda(i32),
    /// Uns 10 por segundo.
    Tique,
}

/// Um programa que mora numa janela.
pub trait App {
    fn titulo(&self) -> &str;
    /// Tamanho do conteudo em pixels (fixo por enquanto).
    fn tamanho(&self) -> (usize, usize);
    /// Redesenha o conteudo inteiro.
    fn desenhar(&mut self, conteudo: &mut Superficie);
    /// true = precisa redesenhar.
    fn evento(&mut self, e: EventoApp) -> bool;
    /// Chamado quando a janela fecha.
    fn fechar(&mut self) {}
    /// Pedidos ao gerenciador, lidos depois de cada evento (Tique tambem)
    /// entregue a este app. O padrao e nao pedir nada.
    fn pedidos(&mut self) -> Vec<Pedido> {
        Vec::new()
    }
}

/// O que um app pede ao gerenciador de janelas (ver App::pedidos).
pub enum Pedido {
    /// Abre uma janela com o canto em (x, y); x ou y negativo = na escada.
    Abrir { app: Box<dyn App>, x: i32, y: i32 },
    /// Fecha a janela com este id (gerenciador de tarefas).
    Fechar(usize),
    /// Fecha a janela de quem pediu.
    #[allow(dead_code)] // para dialogos que se fecham sozinhos
    FecharEsta,
    /// Abre um caminho do disco com o app certo (apps::abrir_arquivo).
    AbrirArquivo(String),
    /// Troca o layout do teclado. Quem atende e o gui::rodar, dono da
    /// Entrada (sai em pedidos_externos).
    DefinirLayout(Layout),
    /// O tema ou a aparencia mudaram: redesenha tudo.
    TemaMudou,
}

/// Fotografia de uma janela (gerenciador de tarefas, comando "tarefas").
#[derive(Clone)]
pub struct InfoJanela {
    pub id: usize,
    pub titulo: String,
    /// Ticks do contador gastos no evento + desenhar deste app.
    pub tempo_cpu: u64,
    /// Bytes da Superficie de conteudo.
    pub memoria: usize,
    pub focada: bool,
}

/// Fotografia de todas as janelas, refeita a cada Gerenciador::tique e
/// quando uma janela abre ou fecha. So o laco principal usa (nunca uma
/// interrupcao), entao basta um UnsafeCell sem trava, como o tema.
struct Resumo(UnsafeCell<Vec<InfoJanela>>);

unsafe impl Sync for Resumo {}

static RESUMO: Resumo = Resumo(UnsafeCell::new(Vec::new()));

/// Copia da ultima fotografia das janelas, na ordem em que abriram
/// (vazia sem a area de trabalho).
pub fn resumo() -> Vec<InfoJanela> {
    unsafe { (*RESUMO.0.get()).clone() }
}

fn guardar_resumo(lista: Vec<InfoJanela>) {
    unsafe { *RESUMO.0.get() = lista };
}

/// Deslocamento da sombra (para a direita e para baixo) e largura do degrade.
const SOMBRA: i32 = 6;
/// Opacidade da sombra no miolo (0 a 255).
const SOMBRA_ALFA: u32 = 120;
/// Botoes [_] e [x] da barra de titulo.
const LADO_BOTAO: i32 = 18;
/// Onde abrem as janelas do menu: uma escada a partir daqui.
const CASCATA_X: i32 = 48;
const CASCATA_Y: i32 = 32;
const PASSO_CASCATA: i32 = 28;
const DEGRAUS_CASCATA: i32 = 8;
/// Areas sujas guardadas separadas antes de juntar tudo numa so.
const MAX_SUJAS: usize = 4;
/// Pedacos que um compor pode mudar: as areas sujas e os dois do cursor.
pub const MAX_PARTES: usize = MAX_SUJAS + 2;

/// Enter (Caractere) na area de trabalho: abre o atalho escolhido.
const ENTER: u8 = 13;

// Codigos evdev (KEY_*) dos atalhos.
const TECLA_ESC: u16 = 1;
const TECLA_TAB: u16 = 15;
const TECLA_ALT: u16 = 56;
const TECLA_F4: u16 = 62;
const TECLA_ALT_DIREITO: u16 = 100;
const TECLA_INICIAR: u16 = 125;

const BOTAO_ESQUERDO: u8 = 1 << 0;
const BOTAO_DIREITO: u8 = 1 << 1;
const BOTAO_MEIO: u8 = 1 << 2;

fn bit_do_botao(botao: Botao) -> u8 {
    match botao {
        Botao::Esquerdo => BOTAO_ESQUERDO,
        Botao::Direito => BOTAO_DIREITO,
        Botao::Meio => BOTAO_MEIO,
    }
}

struct Janela {
    id: usize,
    app: Box<dyn App>,
    /// Canto de cima a esquerda da moldura.
    x: i32,
    y: i32,
    conteudo: Superficie,
    /// Barra de titulo ja desenhada (cache).
    barra: Superficie,
    titulo: String,
    barra_suja: bool,
    conteudo_sujo: bool,
    minimizada: bool,
    tempo_cpu: u64,
}

impl Janela {
    fn largura(&self) -> i32 {
        self.conteudo.largura() as i32 + 2 * BORDA
    }

    fn altura(&self) -> i32 {
        self.conteudo.altura() as i32 + ALTURA_TITULO + 2 * BORDA
    }

    fn moldura(&self) -> Retangulo {
        Retangulo::novo(self.x, self.y, self.largura(), self.altura())
    }

    /// Moldura mais a sombra: tudo que a janela pinta na cena.
    fn limites(&self) -> Retangulo {
        Retangulo::novo(self.x, self.y, self.largura() + SOMBRA, self.altura() + SOMBRA)
    }

    fn barra_titulo(&self) -> Retangulo {
        Retangulo::novo(self.x + BORDA, self.y + BORDA, self.largura() - 2 * BORDA, ALTURA_TITULO)
    }

    fn area_conteudo(&self) -> Retangulo {
        let (largura, altura) = (self.conteudo.largura() as i32, self.conteudo.altura() as i32);
        Retangulo::novo(self.x + BORDA, self.y + BORDA + ALTURA_TITULO, largura, altura)
    }

    /// Botao [x], na tela.
    fn botao_fechar(&self) -> Retangulo {
        let b = self.barra_titulo();
        botao_fechar_local(b.largura).deslocado(b.x, b.y)
    }

    /// Botao [_], na tela.
    fn botao_minimizar(&self) -> Retangulo {
        let b = self.barra_titulo();
        botao_minimizar_local(b.largura).deslocado(b.x, b.y)
    }

    /// Entrega o evento ao app contando o tempo gasto. Marca o conteudo
    /// para redesenhar se o app pediu. Devolve true se o titulo mudou (o
    /// app pode trocar o titulo em qualquer evento, redesenhando ou nao).
    fn evento_app(&mut self, e: EventoApp) -> bool {
        let inicio = cpu::contador();
        let redesenhar = self.app.evento(e);
        self.tempo_cpu += cpu::contador().wrapping_sub(inicio);
        if redesenhar {
            self.conteudo_sujo = true;
        }
        self.conferir_titulo()
    }

    /// O app trocou de titulo? (ex.: navegador mudou de pagina)
    fn conferir_titulo(&mut self) -> bool {
        if self.app.titulo() == self.titulo {
            return false;
        }
        self.titulo = String::from(self.app.titulo());
        self.barra_suja = true;
        true
    }

    fn redesenhar_conteudo(&mut self) {
        let inicio = cpu::contador();
        self.app.desenhar(&mut self.conteudo);
        self.tempo_cpu += cpu::contador().wrapping_sub(inicio);
        self.conteudo_sujo = false;
    }

    /// Desenha a barra de titulo no cache: degrade, titulo em seminegrito,
    /// botoes [_] e [x].
    fn desenhar_barra(&mut self, focada: bool, t: &Tema) {
        let s = &mut self.barra;
        let area = s.area();
        let base = if focada { t.titulo_ativo } else { t.titulo_inativo };
        s.gradiente_vertical(area, area::clarear(base, 36), base);

        let espaco = area.largura - 2 * (LADO_BOTAO + 4) - 16;
        let texto = fonte::encurtar(&self.titulo, espaco, &INTER_NEGRITO);
        let base = INTER_NEGRITO.base_centralizada(0, ALTURA_TITULO);
        s.texto_com(9, base, &texto, t.titulo_texto, &INTER_NEGRITO);

        let m = botao_minimizar_local(area.largura);
        s.retangulo(m, t.botao);
        s.contorno(m, t.borda, 1);
        s.retangulo(Retangulo::novo(m.x + 5, m.baixo() - 6, m.largura - 10, 2), t.botao_texto);

        let f = botao_fechar_local(area.largura);
        s.retangulo(f, t.fechar);
        s.contorno(f, t.borda, 1);
        for d in 0..2 {
            // X com traco de 2 px
            s.linha(f.x + 5 + d, f.y + 5, f.direita() - 6 + d, f.baixo() - 6, 0x00FF_FFFF);
            s.linha(f.direita() - 6 + d, f.y + 5, f.x + 5 + d, f.baixo() - 6, 0x00FF_FFFF);
        }
        self.barra_suja = false;
    }

    /// Pinta a janela (sombra, borda, titulo, conteudo) na parte `area` da cena.
    fn pintar(&self, cena: &mut Superficie, area: Retangulo, t: &Tema) {
        let m = self.moldura();
        pintar_sombra(cena, m, area, t.sombra);
        area::preencher(cena, Retangulo::novo(m.x, m.y, m.largura, BORDA), area, t.borda);
        area::preencher(cena, Retangulo::novo(m.x, m.baixo() - BORDA, m.largura, BORDA), area, t.borda);
        area::preencher(cena, Retangulo::novo(m.x, m.y + BORDA, BORDA, m.altura - 2 * BORDA), area, t.borda);
        area::preencher(cena, Retangulo::novo(m.direita() - BORDA, m.y + BORDA, BORDA, m.altura - 2 * BORDA), area, t.borda);
        let b = self.barra_titulo();
        area::copiar_recortado(&self.barra, b.x, b.y, area, cena);
        let c = self.area_conteudo();
        area::copiar_recortado(&self.conteudo, c.x, c.y, area, cena);
    }
}

/// Botao [x] relativo a barra de titulo de largura `largura`.
fn botao_fechar_local(largura: i32) -> Retangulo {
    let margem = (ALTURA_TITULO - LADO_BOTAO) / 2;
    Retangulo::novo(largura - LADO_BOTAO - margem, margem, LADO_BOTAO, LADO_BOTAO)
}

/// Botao [_], a esquerda do [x].
fn botao_minimizar_local(largura: i32) -> Retangulo {
    botao_fechar_local(largura).deslocado(-(LADO_BOTAO + 4), 0)
}

/// Sombra suave embaixo e a direita da moldura: escurece o que ja esta
/// na cena, mais forte no meio e sumindo nos ultimos SOMBRA pixels.
fn pintar_sombra(cena: &mut Superficie, moldura: Retangulo, area: Retangulo, cor: Cor) {
    let sombra = moldura.deslocado(SOMBRA, SOMBRA);
    let alvo = sombra.intersecao(&area).intersecao(&cena.area());
    if alvo.vazio() {
        return;
    }
    let largura = cena.largura();
    let pixels = cena.pixels_mut();
    for y in alvo.y..alvo.baixo() {
        let dy = (y - sombra.y).min(sombra.baixo() - 1 - y);
        let linha = y as usize * largura;
        // ao lado da janela a sombra so comeca depois da moldura (o resto
        // a janela pinta por cima); embaixo dela, a linha toda
        let inicio = if y < moldura.baixo() { alvo.x.max(moldura.direita()) } else { alvo.x };
        for x in inicio..alvo.direita() {
            let dx = (x - sombra.x).min(sombra.direita() - 1 - x);
            let d = dx.min(dy).clamp(0, SOMBRA - 1) as u32;
            let alfa = SOMBRA_ALFA * (d + 1) / SOMBRA as u32;
            let p = &mut pixels[linha + x as usize];
            *p = grafico::misturar(*p, cor, alfa);
        }
    }
}

/// Junta `r` numa lista pequena de areas sujas: grudando numa que ele
/// toca ou, com a lista cheia, na que crescer menos.
fn juntar(sujas: &mut Vec<Retangulo>, r: Retangulo) {
    if r.vazio() {
        return;
    }
    let tocando = |a: &Retangulo| !a.intersecao(&Retangulo::novo(r.x - 1, r.y - 1, r.largura + 2, r.altura + 2)).vazio();
    if let Some(a) = sujas.iter_mut().find(|a| tocando(a)) {
        *a = a.uniao(&r);
        return;
    }
    if sujas.len() < MAX_SUJAS {
        sujas.push(r);
        return;
    }
    let crescimento = |a: &Retangulo| {
        let u = a.uniao(&r);
        u.largura as i64 * u.altura as i64 - a.largura as i64 * a.altura as i64
    };
    if let Some(a) = sujas.iter_mut().min_by_key(|a| crescimento(a)) {
        *a = a.uniao(&r);
    }
}

/// Retangulo do cursor com a ponta em (x, y).
fn retangulo_cursor((x, y): (i32, i32)) -> Retangulo {
    Retangulo::novo(x, y, cursor::LARGURA, cursor::ALTURA)
}

/// Janela sendo arrastada pela barra de titulo.
#[derive(Clone, Copy)]
struct Arrasto {
    id: usize,
    /// Ponteiro em relacao ao canto da moldura no momento do clique.
    dx: i32,
    dy: i32,
}

pub struct Gerenciador {
    largura: i32,
    altura: i32,
    /// Ordem de empilhamento: a ultima fica por cima.
    janelas: Vec<Janela>,
    foco: Option<usize>,
    proximo_id: usize,
    /// A tela montada sem o cursor.
    cena: Superficie,
    sujas: Vec<Retangulo>,
    barra: BarraTarefas,
    menu: MenuIniciar,
    cursor: (i32, i32),
    /// Onde o cursor esta desenhado no destino (None = destino ainda vazio).
    cursor_desenhado: Option<(i32, i32)>,
    botoes: u8,
    arrasto: Option<Arrasto>,
    /// Janela que recebeu o clique no conteudo: fica com o mouse ate soltar.
    captura: Option<usize>,
    alt: bool,
    /// Caractere que vem junto de uma tecla de atalho e nao vai para o app.
    engolir: Option<u8>,
    /// Ultimo valor mostrado no relogio (segundo ou minuto, pela aparencia).
    relogio: u64,
    cascata: i32,
    /// Icones da area de trabalho.
    atalhos: Atalhos,
    /// Pedidos que so o laco principal atende (pedidos_externos).
    externos: Vec<Pedido>,
}

impl Gerenciador {
    pub fn novo(largura: i32, altura: i32) -> Gerenciador {
        let (largura, altura) = (largura.max(1), altura.max(ALTURA_BARRA_TAREFAS + 1));
        let t = tema::atual();
        let mut sujas = Vec::with_capacity(MAX_SUJAS);
        sujas.push(Retangulo::novo(0, 0, largura, altura));
        Gerenciador {
            largura,
            altura,
            janelas: Vec::new(),
            foco: None,
            proximo_id: 1,
            cena: Superficie::nova(largura as usize, altura as usize, t.area_cima),
            sujas,
            barra: BarraTarefas::nova(largura, altura, &t),
            menu: MenuIniciar::novo(altura - ALTURA_BARRA_TAREFAS),
            cursor: (largura / 2, altura / 2),
            cursor_desenhado: None,
            botoes: 0,
            arrasto: None,
            captura: None,
            alt: false,
            engolir: None,
            relogio: u64::MAX,
            cascata: 0,
            atalhos: Atalhos::novos(),
            externos: Vec::new(),
        }
    }

    fn tela(&self) -> Retangulo {
        Retangulo::novo(0, 0, self.largura, self.altura)
    }

    /// Altura livre para janelas (acima da barra de tarefas).
    fn altura_area(&self) -> i32 {
        self.altura - ALTURA_BARRA_TAREFAS
    }

    fn sujar(&mut self, r: Retangulo) {
        let r = r.intersecao(&self.tela());
        juntar(&mut self.sujas, r);
    }

    fn indice(&self, id: usize) -> Option<usize> {
        self.janelas.iter().position(|j| j.id == id)
    }

    /// Janela visivel mais de cima no ponto (x, y).
    fn janela_em(&self, x: i32, y: i32) -> Option<usize> {
        self.janelas.iter().rposition(|j| !j.minimizada && j.moldura().contem(x, y))
    }

    /// Abre uma janela com o canto em (x, y) (ajustado para caber) e foca.
    pub fn abrir(&mut self, app: Box<dyn App>, x: i32, y: i32) -> usize {
        let t = tema::atual();
        let (largura, altura) = app.tamanho();
        // o conteudo nunca passa da tela, nem que o app peca mais
        let largura = largura.clamp(1, (self.largura - 2 * BORDA).max(1) as usize);
        let altura = altura.clamp(1, (self.altura_area() - ALTURA_TITULO - 2 * BORDA).max(1) as usize);
        let id = self.proximo_id;
        self.proximo_id += 1;
        let titulo = String::from(app.titulo());
        self.janelas.push(Janela {
            id,
            app,
            x,
            y,
            conteudo: Superficie::nova(largura, altura, t.fundo_janela),
            barra: Superficie::nova(largura, ALTURA_TITULO as usize, t.titulo_ativo),
            titulo,
            barra_suja: true,
            conteudo_sujo: true,
            minimizada: false,
            tempo_cpu: 0,
        });
        self.mover_janela(id, x, y);
        self.focar(id);
        guardar_resumo(self.janelas()); // o resumo() ja ve a janela nova
        id
    }

    /// Abre na proxima posicao da escada (usado pelo menu iniciar).
    fn abrir_em_cascata(&mut self, app: Box<dyn App>) -> usize {
        let degrau = self.cascata;
        self.cascata = (self.cascata + 1) % DEGRAUS_CASCATA;
        self.abrir(app, CASCATA_X + degrau * PASSO_CASCATA, CASCATA_Y + degrau * PASSO_CASCATA)
    }

    pub fn fechar(&mut self, id: usize) -> bool {
        let Some(i) = self.indice(id) else { return false };
        let mut janela = self.janelas.remove(i);
        janela.app.fechar();
        if !janela.minimizada {
            self.sujar(janela.limites());
        }
        self.esquecer(id);
        if self.foco == Some(id) {
            self.focar_a_de_cima();
        }
        self.barra.sujar();
        guardar_resumo(self.janelas());
        true
    }

    /// Larga arrasto e captura de uma janela que sumiu da tela.
    fn esquecer(&mut self, id: usize) {
        if self.arrasto.is_some_and(|a| a.id == id) {
            self.arrasto = None;
        }
        if self.captura == Some(id) {
            self.captura = None;
        }
    }

    /// Foca e traz para cima (desminimiza se precisar).
    fn focar(&mut self, id: usize) {
        let Some(i) = self.indice(id) else { return };
        let topo = self.janelas.len() - 1;
        if self.foco == Some(id) && i == topo && !self.janelas[i].minimizada {
            return;
        }
        let mut janela = self.janelas.remove(i);
        janela.minimizada = false;
        janela.barra_suja = true;
        let limites = janela.limites();
        self.janelas.push(janela);
        self.sujar(limites);
        if let Some(velha) = self.foco.filter(|&v| v != id).and_then(|v| self.indice(v)) {
            self.janelas[velha].barra_suja = true;
        }
        self.foco = Some(id);
        self.barra.sujar();
    }

    /// Passa o foco para a janela visivel mais de cima (ou ninguem).
    fn focar_a_de_cima(&mut self) {
        self.foco = self.janelas.iter().rev().find(|j| !j.minimizada).map(|j| j.id);
        if let Some(i) = self.foco.and_then(|id| self.indice(id)) {
            self.janelas[i].barra_suja = true;
        }
    }

    fn minimizar(&mut self, id: usize) {
        let Some(i) = self.indice(id) else { return };
        if self.janelas[i].minimizada {
            return;
        }
        self.janelas[i].minimizada = true;
        let limites = self.janelas[i].limites();
        self.sujar(limites);
        self.esquecer(id);
        if self.foco == Some(id) {
            self.focar_a_de_cima();
        }
        self.barra.sujar();
    }

    /// Poe a moldura em (x, y), sem sair da tela nem entrar na barra.
    fn mover_janela(&mut self, id: usize, x: i32, y: i32) {
        let (largura, altura_area) = (self.largura, self.altura_area());
        let Some(i) = self.indice(id) else { return };
        let janela = &mut self.janelas[i];
        let x = x.clamp(0, (largura - janela.largura()).max(0));
        let y = y.clamp(0, (altura_area - janela.altura()).max(0));
        if (x, y) == (janela.x, janela.y) {
            return;
        }
        let antes = janela.limites();
        janela.x = x;
        janela.y = y;
        let depois = janela.limites();
        if !janela.minimizada {
            self.sujar(antes.uniao(&depois));
        }
    }

    /// Entrega um evento a janela `id` (ex.: bytes da serial para o
    /// terminal) e atende os pedidos que o app fizer.
    pub fn enviar(&mut self, id: usize, e: EventoApp) {
        let Some(i) = self.indice(id) else { return };
        if self.janelas[i].evento_app(e) {
            self.barra.sujar(); // titulo mudou: o botao da barra tambem
        }
        for pedido in self.janelas[i].app.pedidos() {
            self.atender(id, pedido);
        }
    }

    /// Faz o que o app da janela `id` pediu.
    fn atender(&mut self, id: usize, pedido: Pedido) {
        match pedido {
            Pedido::Abrir { app, x, y } if x < 0 || y < 0 => {
                self.abrir_em_cascata(app);
            }
            Pedido::Abrir { app, x, y } => {
                self.abrir(app, x, y);
            }
            Pedido::Fechar(alvo) => {
                self.fechar(alvo);
            }
            Pedido::FecharEsta => {
                self.fechar(id);
            }
            Pedido::AbrirArquivo(caminho) => {
                self.abrir_em_cascata(apps::abrir_arquivo(&caminho));
            }
            Pedido::TemaMudou => self.redesenhar_tudo(),
            externo @ Pedido::DefinirLayout(_) => self.externos.push(externo),
        }
    }

    /// Tira os pedidos que o gerenciador nao atende sozinho (hoje so o
    /// DefinirLayout); o laco principal le a cada volta.
    pub fn pedidos_externos(&mut self) -> Vec<Pedido> {
        core::mem::take(&mut self.externos)
    }

    pub fn evento(&mut self, e: Evento) {
        match e {
            Evento::Mouse { x, y } => self.mover_ponteiro(x, y),
            Evento::Botao { botao, pressionado: true } => self.apertar(botao),
            Evento::Botao { botao, pressionado: false } => self.soltar(botao),
            Evento::Tecla { codigo, pressionada } => self.tecla(codigo, pressionada),
            Evento::Caractere(c) => self.caractere(c),
            Evento::Roda(passos) => self.roda(passos),
        }
    }

    fn mover_ponteiro(&mut self, x: i32, y: i32) {
        let (x, y) = (x.clamp(0, self.largura - 1), y.clamp(0, self.altura - 1));
        self.cursor = (x, y);
        if let Some(a) = self.arrasto {
            self.mover_janela(a.id, x - a.dx, y - a.dy);
            return;
        }
        self.passar_atalhos(x, y);
        if self.menu.aberto() && self.captura.is_none() {
            self.menu.passar(x, y);
            if self.menu.retangulo().contem(x, y) {
                return;
            }
        }
        let alvo = self.captura.or_else(|| self.conteudo_em(x, y));
        if let Some(id) = alvo {
            self.mouse_para(id, x, y);
        }
    }

    /// Destaque leve no atalho embaixo do ponteiro, so com o ponteiro no
    /// papel de parede (nada por cima). So suja alguma coisa quando o
    /// ponteiro entra ou sai de um atalho.
    fn passar_atalhos(&mut self, x: i32, y: i32) {
        let livre = self.captura.is_none()
            && !self.barra.retangulo().contem(x, y)
            && !(self.menu.aberto() && self.menu.retangulo().contem(x, y))
            && self.janela_em(x, y).is_none();
        let sob = if livre { self.atalho_em(x, y) } else { None };
        let celulas = self.atalhos.passar(sob);
        self.sujar_celulas(celulas);
    }

    /// Atalho em (x, y) da tela (None se os icones estao escondidos).
    fn atalho_em(&self, x: i32, y: i32) -> Option<usize> {
        if !tema::aparencia().icones {
            return None;
        }
        self.atalhos.indice_em(x, y)
    }

    fn sujar_celulas(&mut self, celulas: [Option<Retangulo>; 2]) {
        for r in celulas.into_iter().flatten() {
            self.sujar(r);
        }
    }

    /// Clique no papel de parede: as janelas perdem o foco (o teclado fica
    /// com a area de trabalho) e o atalho embaixo e escolhido; o segundo
    /// clique rapido no mesmo atalho abre o app.
    fn clicar_area(&mut self, x: i32, y: i32) {
        if let Some(i) = self.foco.take().and_then(|id| self.indice(id)) {
            self.janelas[i].barra_suja = true;
            self.barra.sujar();
        }
        let alvo = self.atalho_em(x, y);
        let celulas = self.atalhos.escolher(alvo);
        self.sujar_celulas(celulas);
        if let Some(criar) = alvo.and_then(|i| self.atalhos.clicar(i)) {
            self.abrir_em_cascata(criar());
        }
    }

    /// Janela cujo conteudo (visivel, por cima de tudo) esta em (x, y).
    fn conteudo_em(&self, x: i32, y: i32) -> Option<usize> {
        if self.barra.retangulo().contem(x, y) {
            return None;
        }
        let i = self.janela_em(x, y)?;
        let janela = &self.janelas[i];
        janela.area_conteudo().contem(x, y).then_some(janela.id)
    }

    /// Ponto da tela -> ponto do conteudo da janela `id`.
    fn relativo(&self, id: usize, x: i32, y: i32) -> Option<(i32, i32)> {
        let c = self.janelas[self.indice(id)?].area_conteudo();
        Some((x - c.x, y - c.y))
    }

    fn mouse_para(&mut self, id: usize, x: i32, y: i32) {
        if let Some((x, y)) = self.relativo(id, x, y) {
            self.enviar(id, EventoApp::Mouse { x, y, botoes: self.botoes });
        }
    }

    fn botao_para(&mut self, id: usize, botao: Botao, pressionado: bool) {
        let (x, y) = self.cursor;
        if let Some((x, y)) = self.relativo(id, x, y) {
            self.enviar(id, EventoApp::Botao { x, y, botao, pressionado });
        }
    }

    fn apertar(&mut self, botao: Botao) {
        self.botoes |= bit_do_botao(botao);
        if let Some(id) = self.captura {
            // outro botao com um ja apertado: vai para a mesma janela
            self.botao_para(id, botao, true);
            return;
        }
        let (x, y) = self.cursor;
        let menu_aberto = self.menu.aberto();
        if menu_aberto && self.clique_com_menu(x, y) {
            return;
        }
        if self.barra.retangulo().contem(x, y) {
            if botao == Botao::Esquerdo {
                self.clicar_barra(x, y);
            }
            return;
        }
        let Some(i) = self.janela_em(x, y) else {
            // o clique que so fechou o menu nao mexe na area de trabalho
            if botao == Botao::Esquerdo && !menu_aberto {
                self.clicar_area(x, y);
            }
            return;
        };
        let id = self.janelas[i].id;
        self.focar(id);
        self.clicar_janela(id, botao, x, y);
    }

    /// Clique com o menu aberto. true = o clique acabou aqui.
    fn clique_com_menu(&mut self, x: i32, y: i32) -> bool {
        let dentro = self.menu.retangulo().contem(x, y);
        let no_iniciar = self.barra.alvo_em(x, y) == Some(AlvoBarra::Iniciar);
        let criar = self.menu.item_em(x, y);
        self.fechar_menu();
        if let Some(criar) = criar {
            self.abrir_em_cascata(criar());
        }
        dentro || no_iniciar
    }

    /// Clique na janela `id` (ja focada): botoes, titulo ou conteudo.
    fn clicar_janela(&mut self, id: usize, botao: Botao, x: i32, y: i32) {
        let Some(i) = self.indice(id) else { return };
        let janela = &self.janelas[i];
        let esquerdo = botao == Botao::Esquerdo;
        if janela.botao_fechar().contem(x, y) {
            if esquerdo {
                self.fechar(id);
            }
        } else if janela.botao_minimizar().contem(x, y) {
            if esquerdo {
                self.minimizar(id);
            }
        } else if janela.barra_titulo().contem(x, y) {
            if esquerdo {
                self.arrasto = Some(Arrasto { id, dx: x - janela.x, dy: y - janela.y });
            }
        } else if janela.area_conteudo().contem(x, y) {
            self.captura = Some(id);
            self.botao_para(id, botao, true);
        }
    }

    /// Botao MXOS abre/fecha o menu; botao de janela foca, minimiza ou restaura.
    fn clicar_barra(&mut self, x: i32, y: i32) {
        match self.barra.alvo_em(x, y) {
            Some(AlvoBarra::Iniciar) => self.alternar_menu(),
            Some(AlvoBarra::Janela(id)) => {
                let Some(i) = self.indice(id) else { return };
                if !self.janelas[i].minimizada && self.foco == Some(id) {
                    self.minimizar(id);
                } else {
                    self.focar(id);
                }
            }
            None => {}
        }
    }

    fn soltar(&mut self, botao: Botao) {
        self.botoes &= !bit_do_botao(botao);
        if botao == Botao::Esquerdo {
            self.arrasto = None;
        }
        if let Some(id) = self.captura {
            self.botao_para(id, botao, false);
            if self.botoes == 0 {
                self.captura = None;
            }
        }
    }

    fn alternar_menu(&mut self) {
        if self.menu.aberto() {
            self.fechar_menu();
        } else {
            self.menu.abrir();
            self.sujar(self.menu.retangulo());
            self.barra.sujar();
        }
    }

    fn fechar_menu(&mut self) {
        if self.menu.aberto() {
            self.menu.fechar();
            self.sujar(self.menu.retangulo());
            self.barra.sujar();
        }
    }

    fn tecla(&mut self, codigo: u16, pressionada: bool) {
        if codigo == TECLA_ALT || codigo == TECLA_ALT_DIREITO {
            self.alt = pressionada;
        }
        if pressionada {
            self.engolir = None;
            if self.atalho(codigo) {
                return;
            }
        } else if self.alt && (codigo == TECLA_F4 || codigo == TECLA_TAB) {
            return; // soltou a tecla de um atalho: o app nao viu o aperto
        }
        self.entregar_ao_foco(EventoApp::Tecla { codigo, pressionada });
    }

    /// Atalhos globais. true = a tecla era um atalho.
    fn atalho(&mut self, codigo: u16) -> bool {
        match codigo {
            TECLA_F4 if self.alt => {
                if let Some(id) = self.foco {
                    self.fechar(id);
                }
            }
            TECLA_TAB if self.alt => {
                // traz a janela de baixo para cima: repetir passa por todas
                if let Some(id) = self.janelas.first().map(|j| j.id) {
                    self.focar(id);
                }
                self.engolir = Some(9);
            }
            TECLA_INICIAR => self.alternar_menu(),
            TECLA_ESC if self.menu.aberto() => {
                self.fechar_menu();
                self.engolir = Some(27);
            }
            _ => return false,
        }
        true
    }

    fn caractere(&mut self, c: char) {
        if self.engolir.take().map(char::from) == Some(c) {
            return;
        }
        self.entregar_ao_foco(EventoApp::Caractere(c));
    }

    /// Roda vai para a janela embaixo do ponteiro; se nao tiver, para a focada.
    fn roda(&mut self, passos: i32) {
        let (x, y) = self.cursor;
        let alvo = self.conteudo_em(x, y).or(self.foco);
        if let Some(id) = alvo {
            self.enviar(id, EventoApp::Roda(passos));
        }
    }

    /// Teclado para a janela focada. Sem nenhuma, o Enter abre o atalho
    /// escolhido na area de trabalho.
    fn entregar_ao_foco(&mut self, e: EventoApp) {
        let alvo = self.foco.and_then(|id| self.indice(id)).filter(|&i| !self.janelas[i].minimizada);
        match alvo {
            Some(i) => {
                let id = self.janelas[i].id;
                self.enviar(id, e);
            }
            None if e == EventoApp::Caractere(ENTER as char) && tema::aparencia().icones => {
                if let Some(criar) = self.atalhos.criador_escolhido() {
                    self.abrir_em_cascata(criar());
                }
            }
            None => {}
        }
    }

    /// Tique para todos os apps (minimizados tambem), a fotografia do
    /// resumo() e o relogio.
    pub fn tique(&mut self) {
        guardar_resumo(self.janelas());
        // os ids antes: um app pode abrir ou fechar janelas no proprio Tique
        let ids: Vec<usize> = self.janelas.iter().map(|j| j.id).collect();
        for id in ids {
            self.enviar(id, EventoApp::Tique);
        }
        let segundos = cpu::contador() / cpu::frequencia_timer().max(1);
        let relogio = if tema::aparencia().relogio_segundos { segundos } else { segundos / 60 };
        if relogio != self.relogio {
            self.relogio = relogio;
            self.barra.sujar();
        }
    }

    /// Monta na cena o que mudou e passa para `destino` (o buffer de tras,
    /// sempre o mesmo, do tamanho da tela), com o cursor por cima.
    /// Devolve a area de `destino` que mudou.
    pub fn compor(&mut self, destino: &mut Superficie) -> Option<Retangulo> {
        let partes = self.compor_partes(destino);
        let uniao = partes.iter().fold(Retangulo::default(), |u, p| u.uniao(p));
        (!uniao.vazio()).then_some(uniao)
    }

    /// Igual a compor, mas devolve cada pedaco que mudou separado (os
    /// vazios sobram no fim). Bom para apresentar so os pedacos quando o
    /// cursor e uma janela mudam longe um do outro.
    pub fn compor_partes(&mut self, destino: &mut Superficie) -> [Retangulo; MAX_PARTES] {
        let (t, ap) = (tema::atual(), tema::aparencia());
        self.atualizar_caches(&t, &ap);
        let mut partes = [Retangulo::default(); MAX_PARTES];
        let mut n = 0;
        let sujas = core::mem::take(&mut self.sujas);
        for &area in &sujas {
            self.pintar_cena(area, &t, &ap);
            self.cena.copiar_regiao_para(area, destino, area.x, area.y);
            partes[n] = area;
            n += 1;
        }
        // o cursor parado so precisa voltar se uma area suja passou por cima
        // dele (o relogio mudando nao arrasta o cursor para a conta)
        let apagado = self.cursor_desenhado.is_none_or(|c| {
            let r = retangulo_cursor(c);
            sujas.iter().any(|a| !a.intersecao(&r).vazio())
        });
        self.sujas = sujas;
        self.sujas.clear(); // devolve o Vec (sem realocar a cada quadro)

        if apagado || self.cursor_desenhado != Some(self.cursor) {
            if let Some(velho) = self.cursor_desenhado {
                let velho = retangulo_cursor(velho).intersecao(&self.tela());
                self.cena.copiar_regiao_para(velho, destino, velho.x, velho.y);
                partes[n] = velho;
                n += 1;
            }
            cursor::desenhar(destino, self.cursor.0, self.cursor.1);
            self.cursor_desenhado = Some(self.cursor);
            partes[n] = retangulo_cursor(self.cursor).intersecao(&self.tela());
        }
        partes
    }

    /// Redesenha o que o app pediu, barras de titulo, barra de tarefas,
    /// menu e atalhos, marcando na cena onde eles ficam.
    fn atualizar_caches(&mut self, t: &Tema, ap: &Aparencia) {
        for i in 0..self.janelas.len() {
            let focada = self.foco == Some(self.janelas[i].id);
            let janela = &mut self.janelas[i];
            if janela.minimizada {
                continue; // fica sujo ate voltar para a tela
            }
            if janela.conteudo_sujo {
                janela.redesenhar_conteudo();
                juntar(&mut self.sujas, janela.area_conteudo().intersecao(&Retangulo::novo(0, 0, self.largura, self.altura)));
            }
            if janela.barra_suja {
                janela.desenhar_barra(focada, t);
                juntar(&mut self.sujas, janela.barra_titulo().intersecao(&Retangulo::novo(0, 0, self.largura, self.altura)));
            }
        }
        if self.barra.suja() {
            self.desenhar_barra_tarefas(t);
            self.sujar(self.barra.retangulo());
        }
        if self.menu.aberto() && self.menu.preparar(t) {
            self.sujar(self.menu.retangulo());
        }
        if ap.icones {
            // quem mudou um atalho ja sujou a celula dele
            self.atalhos.preparar(self.altura_area(), t, ap);
        }
    }

    fn desenhar_barra_tarefas(&mut self, t: &Tema) {
        // botoes na ordem em que as janelas abriram, nao na de empilhamento
        let mut itens: Vec<ItemBarra> = self
            .janelas
            .iter()
            .map(|j| ItemBarra { id: j.id, titulo: &j.titulo, focada: self.foco == Some(j.id), minimizada: j.minimizada })
            .collect();
        itens.sort_unstable_by_key(|item| item.id);
        let segundos = cpu::contador() / cpu::frequencia_timer().max(1);
        let relogio = if tema::aparencia().relogio_segundos { area::relogio(segundos) } else { area::relogio_curto(segundos) };
        self.barra.desenhar(&itens, &relogio, self.menu.aberto(), t);
    }

    /// Refaz a parte `area` da cena, de baixo para cima.
    fn pintar_cena(&mut self, area: Retangulo, t: &Tema, ap: &Aparencia) {
        let altura_area = self.altura_area();
        area::pintar_papel(&mut self.cena, area, (0, 0), altura_area, ap);
        if ap.icones {
            self.atalhos.pintar(&mut self.cena, area);
        }
        for janela in self.janelas.iter().filter(|j| !j.minimizada) {
            if !janela.limites().intersecao(&area).vazio() {
                janela.pintar(&mut self.cena, area, t);
            }
        }
        self.barra.pintar(&mut self.cena, area);
        if self.menu.aberto() {
            self.menu.pintar(&mut self.cena, area);
        }
    }

    /// Fotografia das janelas, na ordem em que abriram.
    pub fn janelas(&self) -> Vec<InfoJanela> {
        let mut lista: Vec<InfoJanela> = self
            .janelas
            .iter()
            .map(|j| InfoJanela {
                id: j.id,
                titulo: j.titulo.clone(),
                tempo_cpu: j.tempo_cpu,
                memoria: j.conteudo.largura() * j.conteudo.altura() * 4,
                focada: self.foco == Some(j.id),
            })
            .collect();
        lista.sort_unstable_by_key(|j| j.id);
        lista
    }

    /// Entrada nova no menu iniciar (sem icone).
    #[allow(dead_code)] // os apps do MXOS entram com icone (registrar_app)
    pub fn registrar_no_menu(&mut self, nome: &'static str, criar: Criador) {
        self.registrar_menu(nome, None, criar);
    }

    /// Entrada nova no menu iniciar, com o icone na frente do nome.
    pub fn registrar_app(&mut self, nome: &'static str, icone: Icone, criar: Criador) {
        self.registrar_menu(nome, Some(icone), criar);
    }

    fn registrar_menu(&mut self, nome: &'static str, icone: Option<Icone>, criar: Criador) {
        self.menu.adicionar(nome, icone, criar);
        if self.menu.aberto() {
            self.fechar_menu(); // o tamanho mudou: fecha para nao sobrar lixo
        }
    }

    /// Atalho novo na coluna de icones da area de trabalho.
    pub fn registrar_atalho(&mut self, nome: &'static str, icone: Icone, criar: Criador) {
        let celula = self.atalhos.adicionar(nome, icone, criar);
        self.sujar(celula);
    }

    /// Redesenha tudo (ex.: depois de trocar o tema ou o papel de parede):
    /// apps, barras, menu, atalhos e a cena inteira.
    pub fn redesenhar_tudo(&mut self) {
        for janela in &mut self.janelas {
            janela.conteudo_sujo = true;
            janela.barra_suja = true;
        }
        self.barra.sujar();
        self.menu.sujar();
        self.atalhos.sujar_todos();
        self.sujas.clear();
        self.sujas.push(self.tela());
    }
}
