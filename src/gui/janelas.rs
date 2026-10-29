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
use crate::video::tela::Tela;
use crate::entrada::{Botao, Evento, Layout};
use crate::fonte::{self, INTER, INTER_NEGRITO};

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
    /// Os processos escreveram algo ou terminaram (so o terminal liga).
    Processos,
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
    /// Programa de Linux para rodar no lugar de abrir a janela (argv):
    /// a janela vem depois, pelo X11 (apps/linux.rs).
    fn lancar(&self) -> Option<Vec<String>> {
        None
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
const LADO_BOTAO: i32 = 20;
/// Raio dos cantos das janelas.
const RAIO_JANELA: i32 = 8;
/// Onde abrem as janelas do menu: uma escada a partir daqui.
const CASCATA_X: i32 = 48;
const CASCATA_Y: i32 = 32;
const PASSO_CASCATA: i32 = 28;
const DEGRAUS_CASCATA: i32 = 8;
/// Areas sujas guardadas separadas antes de juntar tudo numa so.
const MAX_SUJAS: usize = 4;
/// Espessura do contorno da janela arrastada.
const LINHA_CONTORNO: i32 = 2;

/// As quatro faixas da borda de `r` (cima, baixo, esquerda, direita).
fn faixas_contorno(r: Retangulo) -> [Retangulo; 4] {
    faixas(r, LINHA_CONTORNO)
}

/// O contorno e uma linha dupla: escura por fora e clara por dentro, para
/// aparecer em cima de qualquer fundo. (faixa, cor) recortadas em `dentro`.
fn pedacos_contorno(r: Retangulo, dentro: Retangulo) -> impl Iterator<Item = (Retangulo, Cor)> {
    let fora = faixas(r, 1).map(|f| (f, 0x0010_1010));
    let miolo = faixas(Retangulo::novo(r.x + 1, r.y + 1, r.largura - 2, r.altura - 2), 1).map(|f| (f, 0x00F0_F0F0));
    fora.into_iter().chain(miolo).map(move |(f, cor)| (f.intersecao(&dentro), cor)).filter(|(f, _)| !f.vazio())
}

/// As quatro faixas de espessura `e` na borda de `r`.
fn faixas(r: Retangulo, e: i32) -> [Retangulo; 4] {
    [
        Retangulo::novo(r.x, r.y, r.largura, e),
        Retangulo::novo(r.x, r.baixo() - e, r.largura, e),
        Retangulo::novo(r.x, r.y + e, e, r.altura - 2 * e),
        Retangulo::novo(r.direita() - e, r.y + e, e, r.altura - 2 * e),
    ]
}

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

    /// Desenha a barra de titulo no cache: degrade leve, titulo em
    /// seminegrito e os botoes [_] e [x] arredondados. A janela que nao
    /// esta em foco fica apagada (titulo e botoes mais fracos).
    fn desenhar_barra(&mut self, focada: bool, t: &Tema) {
        let s = &mut self.barra;
        let area = s.area();
        let base = if focada { t.titulo_ativo } else { t.titulo_inativo };
        s.gradiente_vertical(area, area::clarear(base, 18), base);
        // brilho de 1 px no alto, como luz batendo na borda
        s.retangulo(Retangulo::novo(0, 0, area.largura, 1), area::clarear(base, 60));

        let espaco = area.largura - 2 * (LADO_BOTAO + 6) - 24;
        let texto = fonte::encurtar(&self.titulo, espaco, &INTER_NEGRITO);
        let linha = INTER_NEGRITO.base_centralizada(0, ALTURA_TITULO);
        let cor_texto = if focada { t.titulo_texto } else { grafico::misturar(t.titulo_texto, base, 80) };
        s.texto_com(12, linha, &texto, cor_texto, &INTER_NEGRITO);

        let simbolo = if focada { 0x00FF_FFFF } else { grafico::misturar(0x00FF_FFFF, base, 90) };
        let m = botao_minimizar_local(area.largura);
        s.retangulo_arredondado(m, 4, area::clarear(base, 28));
        s.retangulo(Retangulo::novo(m.x + 6, m.y + m.altura / 2, m.largura - 12, 2), simbolo);

        let f = botao_fechar_local(area.largura);
        let vermelho = if focada { t.fechar } else { grafico::misturar(t.fechar, base, 110) };
        s.retangulo_arredondado(f, 4, vermelho);
        // X de 2 px, um pouco menor que o botao
        for d in 0..2 {
            s.linha(f.x + 6 + d, f.y + 6, f.direita() - 7 + d, f.baixo() - 7, simbolo);
            s.linha(f.direita() - 7 + d, f.y + 6, f.x + 6 + d, f.baixo() - 7, simbolo);
        }
        self.barra_suja = false;
    }

    /// Pinta a janela (sombra, borda, titulo, conteudo) na parte `area` da
    /// cena, com os cantos arredondados: o que estava embaixo dos quatro
    /// cantos e guardado antes e volta, suavizado, depois.
    fn pintar(&self, cena: &mut Superficie, area: Retangulo, t: &Tema) {
        let m = self.moldura();
        pintar_sombra(cena, m, area, t.sombra);
        let cantos = guardar_cantos(cena, m, area);
        self.pintar_retangular(cena, area, t);
        arredondar_cantos(cena, m, area, &cantos, t.borda);
    }

    fn pintar_retangular(&self, cena: &mut Superficie, area: Retangulo, t: &Tema) {
        let m = self.moldura();
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

/// Os quatro cantos de RAIO_JANELA x RAIO_JANELA da moldura `m`
/// (cima-esquerda, cima-direita, baixo-esquerda, baixo-direita).
fn cantos(m: Retangulo) -> [(i32, i32); 4] {
    let r = RAIO_JANELA;
    [(m.x, m.y), (m.direita() - r, m.y), (m.x, m.baixo() - r), (m.direita() - r, m.baixo() - r)]
}

const PIXELS_CANTO: usize = (RAIO_JANELA * RAIO_JANELA) as usize;

/// O que esta na cena embaixo dos cantos da moldura (antes da janela).
fn guardar_cantos(cena: &Superficie, m: Retangulo, area: Retangulo) -> [[Cor; PIXELS_CANTO]; 4] {
    let mut guardado = [[0; PIXELS_CANTO]; 4];
    for (k, (x0, y0)) in cantos(m).into_iter().enumerate() {
        for dy in 0..RAIO_JANELA {
            for dx in 0..RAIO_JANELA {
                let (x, y) = (x0 + dx, y0 + dy);
                if area.contem(x, y) {
                    guardado[k][(dy * RAIO_JANELA + dx) as usize] = cena.ler_pixel(x, y).unwrap_or(0);
                }
            }
        }
    }
    guardado
}

/// Arredonda os cantos: fora da curva volta o fundo guardado, na curva
/// passa a linha da borda (BORDA px) e dentro fica a janela. Cada pixel
/// e medido com 4 x 4 amostras, para a curva sair lisa.
fn arredondar_cantos(cena: &mut Superficie, m: Retangulo, area: Retangulo, guardado: &[[Cor; PIXELS_CANTO]; 4], borda: Cor) {
    let r = RAIO_JANELA;
    // raios em quartos de pixel, ao quadrado: fora da borda e dentro dela
    let fora = (r * 4) * (r * 4);
    let dentro = ((r - BORDA) * 4) * ((r - BORDA) * 4);
    for (k, (x0, y0)) in cantos(m).into_iter().enumerate() {
        // centro do circulo, em quartos de pixel
        let cx = if k % 2 == 0 { (x0 + r) * 4 } else { x0 * 4 };
        let cy = if k < 2 { (y0 + r) * 4 } else { y0 * 4 };
        for dy in 0..r {
            for dx in 0..r {
                let (x, y) = (x0 + dx, y0 + dy);
                if !area.contem(x, y) {
                    continue;
                }
                let (mut no_contorno, mut na_janela) = (0u32, 0u32);
                for sy in 0..4 {
                    for sx in 0..4 {
                        // 4 x 4 pontos dentro do pixel, em quartos de pixel
                        let (qx, qy) = (x * 4 + sx - cx, y * 4 + sy - cy);
                        let d = qx * qx + qy * qy;
                        if d <= dentro {
                            na_janela += 1;
                        } else if d <= fora {
                            no_contorno += 1;
                        }
                    }
                }
                if na_janela == 16 {
                    continue;
                }
                let fundo = guardado[k][(dy * r + dx) as usize];
                let janela = cena.ler_pixel(x, y).unwrap_or(fundo);
                // fundo -> borda pela parte que cai no contorno, depois -> janela
                let com_borda = grafico::misturar(fundo, borda, (no_contorno + na_janela) * 16);
                let total = (no_contorno + na_janela).max(1);
                let cor = grafico::misturar(com_borda, janela, na_janela * 255 / total);
                cena.pixel(x, y, cor);
            }
        }
    }
}

/// Botao [x] relativo a barra de titulo de largura `largura`.
fn botao_fechar_local(largura: i32) -> Retangulo {
    let margem = (ALTURA_TITULO - LADO_BOTAO) / 2;
    Retangulo::novo(largura - LADO_BOTAO - margem, margem, LADO_BOTAO, LADO_BOTAO)
}

/// Botao [_], a esquerda do [x].
fn botao_minimizar_local(largura: i32) -> Retangulo {
    botao_fechar_local(largura).deslocado(-(LADO_BOTAO + 6), 0)
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

/// O que o relogio da barra mostra agora: hora (com ou sem segundos) e data.
fn hora_e_data() -> (String, String) {
    let agora = area::agora_local();
    let hora = if tema::aparencia().relogio_segundos { area::relogio(agora) } else { area::relogio_curto(agora) };
    (hora, area::data(agora))
}

/// Retangulo do cursor com a ponta em (x, y).
fn retangulo_cursor((x, y): (i32, i32)) -> Retangulo {
    Retangulo::novo(x, y, cursor::LARGURA, cursor::ALTURA)
}

/// Menu do clique direito na area de trabalho.
const ITENS_CONTEXTO: [&str; 2] = ["Alterar plano de fundo…", "Próximo plano de fundo"];
const ALTURA_ITEM_CONTEXTO: i32 = 30;
const LARGURA_CONTEXTO: i32 = 220;

#[derive(Clone, Copy, PartialEq)]
struct MenuContexto {
    r: Retangulo,
    destacado: Option<usize>,
}

impl MenuContexto {
    /// Aberto com o canto em (x, y), sem sair da area de trabalho.
    fn novo(x: i32, y: i32, largura_tela: i32, altura_area: i32) -> MenuContexto {
        let altura = ITENS_CONTEXTO.len() as i32 * ALTURA_ITEM_CONTEXTO + 8;
        let x = x.min(largura_tela - LARGURA_CONTEXTO - 2).max(0);
        let y = y.min(altura_area - altura - 2).max(0);
        MenuContexto { r: Retangulo::novo(x, y, LARGURA_CONTEXTO, altura), destacado: None }
    }

    fn item(&self, i: usize) -> Retangulo {
        Retangulo::novo(self.r.x + 4, self.r.y + 4 + i as i32 * ALTURA_ITEM_CONTEXTO, self.r.largura - 8, ALTURA_ITEM_CONTEXTO)
    }

    fn item_em(&self, x: i32, y: i32) -> Option<usize> {
        (0..ITENS_CONTEXTO.len()).find(|&i| self.item(i).contem(x, y))
    }

    /// Com sombra um pouco maior que o retangulo.
    fn limites(&self) -> Retangulo {
        Retangulo::novo(self.r.x, self.r.y, self.r.largura + SOMBRA, self.r.altura + SOMBRA)
    }

    /// Desenhado direto na cena (e pequeno; nao precisa de cache).
    fn pintar(&self, cena: &mut Superficie, area: Retangulo, t: &Tema) {
        if self.limites().intersecao(&area).vazio() {
            return;
        }
        let mut pedaco = Superficie::nova(self.r.largura as usize, self.r.altura as usize, 0);
        cena.copiar_regiao_para(self.r, &mut pedaco, 0, 0);
        let fundo = pedaco.area();
        pedaco.retangulo_arredondado(fundo, 7, t.borda);
        pedaco.retangulo_arredondado(Retangulo::novo(1, 1, fundo.largura - 2, fundo.altura - 2), 6, t.fundo_janela);
        for (i, texto) in ITENS_CONTEXTO.iter().enumerate() {
            let r = self.item(i).deslocado(-self.r.x, -self.r.y);
            let destacado = self.destacado == Some(i);
            if destacado {
                pedaco.retangulo_arredondado(r, 5, t.destaque);
            }
            let cor = if destacado { 0x00FF_FFFF } else { t.texto };
            let linha = INTER.base_centralizada(r.y, r.altura);
            pedaco.texto_com(r.x + 12, linha, texto, cor, &INTER);
        }
        pintar_sombra(cena, self.r, area, t.sombra);
        area::copiar_recortado(&pedaco, self.r.x, self.r.y, area, cena);
    }
}

/// Selecao em area na area de trabalho: de onde o arrasto comecou e o
/// retangulo atual (vazio enquanto o mouse nao andou).
#[derive(Clone, Copy)]
struct Selecao {
    inicio: (i32, i32),
    r: Retangulo,
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
    /// Onde o cursor esta desenhado na tela (None = ainda nao foi).
    cursor_desenhado: Option<(i32, i32)>,
    /// Contorno da janela sendo arrastada (com arrastar_contorno) e onde
    /// ele esta desenhado na tela.
    contorno: Option<Retangulo>,
    contorno_desenhado: Option<Retangulo>,
    /// Menu do clique direito na area de trabalho (None = fechado).
    contexto: Option<MenuContexto>,
    /// Retangulo de selecao na area de trabalho (arrastando no papel).
    selecao: Option<Selecao>,
    /// Pedacos sujos que nao sao juntados com os outros: as faixas finas
    /// que mudam quando a selecao cresce ou encolhe (juntas virariam o
    /// retangulo inteiro).
    soltas: Vec<Retangulo>,
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
            contorno: None,
            contorno_desenhado: None,
            contexto: None,
            selecao: None,
            soltas: Vec::new(),
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
        if let Some(argv) = app.lancar() {
            let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
            if let Err(e) = crate::processo::executar(&argv[0], &args, false) {
                crate::processo::avisar_no_terminal(&alloc::format!("\nnão consegui abrir {}: {e}\n", argv[0]));
            }
            return 0;
        }
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

    /// (x, y) ajustado para a moldura da janela `i` nao sair da tela nem
    /// entrar na barra de tarefas.
    fn posicao_valida(&self, i: usize, x: i32, y: i32) -> (i32, i32) {
        let janela = &self.janelas[i];
        let x = x.clamp(0, (self.largura - janela.largura()).max(0));
        let y = y.clamp(0, (self.altura_area() - janela.altura()).max(0));
        (x, y)
    }

    /// Poe a moldura em (x, y), sem sair da tela nem entrar na barra.
    fn mover_janela(&mut self, id: usize, x: i32, y: i32) {
        let Some(i) = self.indice(id) else { return };
        let (x, y) = self.posicao_valida(i, x, y);
        let janela = &mut self.janelas[i];
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
            let Some(i) = self.indice(a.id) else { return };
            if tema::aparencia().arrastar_contorno {
                // so o contorno anda; a janela vai para o lugar ao soltar
                let (cx, cy) = self.posicao_valida(i, x - a.dx, y - a.dy);
                let m = self.janelas[i].moldura();
                self.contorno = Some(Retangulo::novo(cx, cy, m.largura, m.altura));
            } else {
                self.mover_janela(a.id, x - a.dx, y - a.dy);
            }
            return;
        }
        if self.selecao.is_some() {
            self.esticar_selecao(x, y);
            return;
        }
        if let Some(mut mc) = self.contexto {
            let destacado = mc.item_em(x, y);
            if destacado != mc.destacado {
                mc.destacado = destacado;
                self.contexto = Some(mc);
                self.sujar(mc.r);
            }
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
        for r in self.atalhos.escolher(alvo) {
            self.sujar(r);
        }
        if alvo.is_none() {
            // no papel de parede: comeca a selecao em area
            self.selecao = Some(Selecao { inicio: (x, y), r: Retangulo::default() });
        }
        if let Some(criar) = alvo.and_then(|i| self.atalhos.clicar(i)) {
            self.abrir_em_cascata(criar());
        }
    }

    /// O mouse andou com a selecao em area aberta: o retangulo vai ate ele,
    /// os atalhos que ele toca ficam escolhidos, e so as faixas que mudaram
    /// sao redesenhadas.
    fn esticar_selecao(&mut self, x: i32, y: i32) {
        let Some(sel) = self.selecao else { return };
        let limite = Retangulo::novo(0, 0, self.largura, self.altura_area());
        let novo = Retangulo::entre(sel.inicio, (x, y)).intersecao(&limite);
        if novo == sel.r {
            return;
        }
        let velho = sel.r;
        self.selecao = Some(Selecao { r: novo, ..sel });
        let mut faixas_mudadas: Vec<Retangulo> = Vec::with_capacity(16);
        faixas_mudadas.extend(velho.menos(&novo));
        faixas_mudadas.extend(novo.menos(&velho));
        if !velho.vazio() {
            faixas_mudadas.extend(faixas(velho, 1));
        }
        faixas_mudadas.extend(faixas(novo, 1));
        for f in faixas_mudadas {
            let f = f.intersecao(&self.tela());
            if !f.vazio() {
                self.soltas.push(f);
            }
        }
        if tema::aparencia().icones {
            for r in self.atalhos.escolher_em(novo) {
                self.sujar(r);
            }
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
        if let Some(mc) = self.contexto.take() {
            // qualquer clique fecha o menu; em cima de um item, faz o item
            self.sujar(mc.limites());
            if let Some(i) = mc.item_em(x, y) {
                self.fazer_contexto(i);
                return;
            }
        }
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
            if botao == Botao::Direito && !menu_aberto {
                let mc = MenuContexto::novo(x, y, self.largura, self.altura_area());
                self.sujar(mc.limites());
                self.contexto = Some(mc);
            }
            return;
        };
        let id = self.janelas[i].id;
        self.focar(id);
        self.clicar_janela(id, botao, x, y);
    }

    /// Um item do menu do clique direito.
    fn fazer_contexto(&mut self, item: usize) {
        match item {
            0 => {
                self.abrir_em_cascata(apps::configuracoes::criar_personalizar());
            }
            _ => {
                // o proximo padrao de papel de parede, na hora (Salvar nas
                // Configuracoes guarda no disco)
                let mut a = tema::aparencia();
                let todos = tema::PapelDeParede::TODOS;
                let i = todos.iter().position(|&p| p == a.papel).unwrap_or(0);
                a.papel = todos[(i + 1) % todos.len()];
                tema::definir_aparencia(a);
                self.redesenhar_tudo();
            }
        }
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
            if let Some(sel) = self.selecao.take() {
                self.sujar(sel.r); // some o retangulo; os escolhidos ficam
            }
            if let (Some(a), Some(c)) = (self.arrasto, self.contorno.take()) {
                self.mover_janela(a.id, c.x, c.y);
            }
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
                for criar in self.atalhos.criadores_escolhidos() {
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
        let segundos = area::agora_local();
        let relogio = if tema::aparencia().relogio_segundos { segundos } else { segundos / 60 };
        if relogio != self.relogio {
            self.relogio = relogio;
            if self.barra.suja() {
                return; // a barra inteira ja vai ser refeita
            }
            // so o relogio muda: nada de redesenhar a barra toda
            let (hora, data) = hora_e_data();
            let r = self.barra.atualizar_relogio(&hora, &data, &tema::atual());
            self.sujar(r);
        }
    }

    /// Monta na cena o que mudou e manda cada pedaco sujo para a tela (um
    /// por vez: o relogio e o mouse longe um do outro nao viram um
    /// retangulo enorme). O contorno do arrasto e o cursor vao direto na
    /// tela, por cima, e saem copiando de volta o que a cena tem embaixo.
    pub fn compor(&mut self, tela: &Tela) {
        let (t, ap) = (tema::atual(), tema::aparencia());
        self.atualizar_caches(&t, &ap);
        let mut sujas = core::mem::take(&mut self.sujas);
        sujas.append(&mut self.soltas);
        for &area in &sujas {
            self.pintar_cena(area, &t, &ap);
            grafico::apresentar(&self.cena, tela, area);
        }
        let tocou = |r: Retangulo| sujas.iter().any(|a| !a.intersecao(&r).vazio());

        // contorno: refeito se andou ou se uma area suja passou por ele
        let mut refeito = Retangulo::default();
        if self.contorno != self.contorno_desenhado || self.contorno_desenhado.is_some_and(tocou) {
            if let Some(velho) = self.contorno_desenhado.take() {
                for faixa in faixas_contorno(velho) {
                    grafico::apresentar(&self.cena, tela, faixa);
                }
                refeito = velho;
            }
            if let Some(novo) = self.contorno {
                for (f, cor) in pedacos_contorno(novo, self.tela()) {
                    tela.retangulo(f.x as usize, f.y as usize, f.largura as usize, f.altura as usize, cor);
                }
                refeito = refeito.uniao(&novo);
            }
            self.contorno_desenhado = self.contorno;
        }

        // o cursor parado so volta se algo foi desenhado por cima dele (o
        // relogio mudando nao arrasta o cursor para a conta)
        let apagado = self.cursor_desenhado.is_none_or(|c| {
            let r = retangulo_cursor(c);
            tocou(r) || !refeito.intersecao(&r).vazio()
        });
        self.sujas = sujas;
        self.sujas.clear(); // devolve o Vec (sem realocar a cada quadro)
        if apagado || self.cursor_desenhado != Some(self.cursor) {
            if let Some(velho) = self.cursor_desenhado {
                grafico::apresentar(&self.cena, tela, retangulo_cursor(velho));
                if let Some(c) = self.contorno_desenhado {
                    // apagar o cursor velho pode ter levado um pedaco do contorno
                    let embaixo = retangulo_cursor(velho).intersecao(&self.tela());
                    for (f, cor) in pedacos_contorno(c, embaixo) {
                        tela.retangulo(f.x as usize, f.y as usize, f.largura as usize, f.altura as usize, cor);
                    }
                }
            }
            // o fundo da seta vem da cena; se o contorno passa ali, vai junto
            let r = retangulo_cursor(self.cursor);
            match self.contorno_desenhado {
                Some(c) if faixas_contorno(c).iter().any(|f| !f.intersecao(&r).vazio()) => {
                    self.desenhar_cursor_sobre_contorno(tela, c);
                }
                _ => cursor::desenhar_na_tela(&self.cena, tela, self.cursor.0, self.cursor.1),
            }
            self.cursor_desenhado = Some(self.cursor);
        }
    }

    /// O cursor em cima de um pedaco do contorno: monta o pedaco com o
    /// contorno e a seta e manda junto.
    fn desenhar_cursor_sobre_contorno(&self, tela: &Tela, contorno: Retangulo) {
        let (x, y) = self.cursor;
        let r = retangulo_cursor(self.cursor);
        let mut pedaco = Superficie::nova(cursor::LARGURA as usize, cursor::ALTURA as usize, 0);
        self.cena.copiar_regiao_para(r, &mut pedaco, 0, 0);
        for (f, cor) in pedacos_contorno(contorno, r) {
            pedaco.retangulo(f.deslocado(-x, -y), cor);
        }
        cursor::desenhar(&mut pedaco, 0, 0);
        grafico::apresentar_em(&pedaco, tela, x, y);
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
        let (hora, data) = hora_e_data();
        self.barra.desenhar(&itens, &hora, &data, self.menu.aberto(), t);
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
        if let Some(mc) = self.contexto {
            mc.pintar(&mut self.cena, area, t);
        }
        if let Some(sel) = self.selecao.filter(|s| !s.r.vazio()) {
            // azul translucido com borda, so na parte que cai em `area`
            area::tingir(&mut self.cena, sel.r.intersecao(&area), t.destaque, 60);
            for faixa in faixas(sel.r, 1) {
                area::preencher(&mut self.cena, faixa, area, area::clarear(t.destaque, 40));
            }
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
