//! Kit de componentes da interface, com as cores do tema atual: botao,
//! painel, rotulo, barra de progresso, grafico de linhas, icones, campo de
//! texto, lista com colunas e abas.
//!
//! Tudo desenha numa Superficie (o conteudo de uma janela) dentro do
//! Retangulo que o app passa, sem pintar fora dele. Os componentes com
//! estado (CampoTexto, Lista, Abas) recebem os EventoApp do app junto com o
//! MESMO Retangulo usado para desenhar e dizem o que aconteceu. Qualquer
//! resposta diferente de Nada (ou false) pede um redesenho.
//!
//! Foco: CampoTexto e Lista ganham o foco com um clique dentro da propria
//! area e perdem com um clique fora dela. So quem tem o foco atende o
//! teclado, entao o app pode passar todos os eventos para todos os
//! componentes. O campo comeca sem foco; a lista comeca com foco.
//!
//! Clique duplo: dois apertos no mesmo alvo em menos de 400 ms, medidos no
//! contador da CPU.

use alloc::string::String;
use alloc::vec::Vec;

use super::area;
use super::grafico::{self, Cor, Retangulo, Superficie};
use super::janelas::EventoApp;
use super::tema::{self, Tema};
use crate::cpu;
use crate::entrada::Botao;
use crate::fonte::{self, Fonte, INTER, INTER_NEGRITO};
use crate::entrada::teclado::{
    KEY_DELETE, KEY_DOWN, KEY_END, KEY_HOME, KEY_LEFT, KEY_PAGEDOWN, KEY_PAGEUP, KEY_RIGHT, KEY_UP,
};

pub const ALTURA_BOTAO: i32 = 24;
/// Altura de uma linha da Lista (e do cabecalho dela).
pub const ALTURA_LINHA: i32 = 20;
/// Altura sugerida para a faixa das Abas.
pub const ALTURA_ABAS: i32 = 28;

const BRANCO: Cor = 0x00FF_FFFF;
const MS_CLIQUE_DUPLO: u64 = 400;
/// Espaco entre a borda e o texto de campos, colunas e botoes.
const MARGEM_TEXTO: i32 = 6;
const MAX_CAMPO: usize = 1024;
const LARGURA_ROLAGEM: i32 = 12;
const MIN_POLEGAR: i32 = 16;
const LINHAS_POR_PASSO_DA_RODA: usize = 3;
const MARGEM_ABA: i32 = 14;

// ------------------------------------------------------------------ cores

/// Brilho percebido de uma cor (0 a 255).
fn brilho(cor: Cor) -> u32 {
    let (r, g, b) = (cor >> 16 & 0xFF, cor >> 8 & 0xFF, cor & 0xFF);
    (r * 299 + g * 587 + b * 114) / 1000
}

/// O tema e escuro (texto claro em janela escura)?
pub fn tema_escuro(t: &Tema) -> bool {
    brilho(t.fundo_janela) < 128
}

/// Fundo de campos, listas e graficos: branco nos temas claros, um pouco
/// mais escuro que a janela nos escuros.
pub fn cor_campo(t: &Tema) -> Cor {
    if tema_escuro(t) { area::escurecer(t.fundo_janela, 50) } else { BRANCO }
}

/// Linha fina de contorno e separacao, mais suave que t.borda.
pub fn cor_linha(t: &Tema) -> Cor {
    grafico::misturar(t.fundo_janela, t.borda, 100)
}

/// Texto apagado (dicas, abas inativas, valores secundarios).
pub fn cor_texto_fraco(t: &Tema) -> Cor {
    grafico::misturar(t.texto, t.fundo_janela, 110)
}

// ---------------------------------------------------------- desenho simples

/// Botao chapado com o texto centralizado. `destaque` = acao principal ou
/// opcao escolhida (fundo na cor de destaque, texto branco).
pub fn botao(s: &mut Superficie, r: Retangulo, texto: &str, destaque: bool) {
    if r.vazio() {
        return;
    }
    let t = tema::atual();
    let (fundo, frente) = if destaque { (t.destaque, BRANCO) } else { (t.botao, t.botao_texto) };
    s.retangulo(r, fundo);
    // a linha de baixo um pouco mais escura da volume sem sair do chapado
    s.retangulo(Retangulo::novo(r.x + 1, r.baixo() - 2, r.largura - 2, 1), area::escurecer(fundo, 22));
    s.contorno(r, grafico::misturar(fundo, t.borda, 110), 1);
    let texto = fonte::encurtar(texto, r.largura - 8, &INTER);
    texto_centrado(s, r, &texto, frente, &INTER);
}

/// Texto no meio de `r` (na horizontal e na vertical), sem cortar.
pub fn texto_centrado(s: &mut Superficie, r: Retangulo, texto: &str, cor: Cor, f: &Fonte) {
    let x = r.x + (r.largura - f.largura(texto)) / 2;
    s.texto_com(x, f.base_centralizada(r.y, r.altura), texto, cor, f);
}

/// Texto a partir de `x`, centralizado na vertical na faixa de `r`.
pub fn texto_na_faixa(s: &mut Superficie, x: i32, r: Retangulo, texto: &str, cor: Cor, f: &Fonte) {
    s.texto_com(x, f.base_centralizada(r.y, r.altura), texto, cor, f);
}

/// Cartao para agrupar coisas: fundo um tom acima da janela, contorno fino.
pub fn painel(s: &mut Superficie, r: Retangulo) {
    if r.vazio() {
        return;
    }
    let t = tema::atual();
    let fundo = if tema_escuro(&t) { area::clarear(t.fundo_janela, 10) } else { BRANCO };
    s.retangulo(r, fundo);
    s.contorno(r, cor_linha(&t), 1);
}

/// Texto simples (Inter 13 px). As maiusculas ocupam a faixa de 8 px a
/// partir de y (como a fonte antiga); acentos sobem uns 3 px e as letras
/// com perna (g, p, ç) descem uns 4 px.
pub fn rotulo(s: &mut Superficie, x: i32, y: i32, texto: &str, cor: Cor) {
    s.texto(x, y, texto, cor, 1);
}

/// Como `rotulo`, em seminegrito (cabecalhos, valores em destaque).
#[allow(dead_code)] // para os apps
pub fn rotulo_negrito(s: &mut Superficie, x: i32, y: i32, texto: &str, cor: Cor) {
    s.texto_com(x, INTER_NEGRITO.base_centralizada(y, 8), texto, cor, &INTER_NEGRITO);
}

/// Barra horizontal cheia ate `milesimos` (0 a 1000) na cor de destaque.
pub fn barra_progresso(s: &mut Superficie, r: Retangulo, milesimos: u32) {
    if r.vazio() {
        return;
    }
    let t = tema::atual();
    s.retangulo(r, grafico::misturar(cor_campo(&t), t.texto, 24));
    let dentro = Retangulo::novo(r.x + 1, r.y + 1, r.largura - 2, r.altura - 2);
    let cheio = (dentro.largura.max(0) as i64 * milesimos.min(1000) as i64 / 1000) as i32;
    s.retangulo(Retangulo::novo(dentro.x, dentro.y, cheio, dentro.altura), t.destaque);
    s.contorno(r, cor_linha(&t), 1);
}

/// Grafico de linha com a area embaixo pintada: `valores` vao da esquerda
/// (mais antigo) para a direita, de 0 (pe) a `maximo` (topo; acima disso
/// fica no topo). Grade de fundo em 25%, 50% e 75%.
pub fn grafico_linhas(s: &mut Superficie, r: Retangulo, valores: &[u32], maximo: u32, cor: Cor) {
    if r.largura < 4 || r.altura < 4 {
        return;
    }
    let t = tema::atual();
    let fundo = cor_campo(&t);
    s.retangulo(r, fundo);
    let grade = grafico::misturar(fundo, t.texto, 22);
    for quarto in 1..4 {
        s.retangulo(Retangulo::novo(r.x + 1, r.y + r.altura * quarto / 4, r.largura - 2, 1), grade);
    }
    let dentro = Retangulo::novo(r.x + 1, r.y + 1, r.largura - 2, r.altura - 2);
    let maximo = maximo.max(1) as i64;
    let altura_de = |v: u32| (v.min(maximo as u32) as i64 * (dentro.altura - 1) as i64 / maximo) as i32;
    let y_de = |v: u32| dentro.baixo() - 1 - altura_de(v);
    if !valores.is_empty() {
        preencher_grafico(s, dentro, valores, &y_de, grafico::misturar(fundo, cor, 70));
        tracar_grafico(s, dentro, valores, &y_de, cor);
    }
    s.contorno(r, cor_linha(&t), 1);
}

/// x do ponto `i` de `n` espalhados na largura de `dentro`.
fn x_do_ponto(dentro: Retangulo, i: usize, n: usize) -> i32 {
    if n < 2 {
        return dentro.x;
    }
    dentro.x + (i as i64 * (dentro.largura - 1) as i64 / (n - 1) as i64) as i32
}

/// Area embaixo da linha, uma coluna de cada vez (interpolando os pontos).
fn preencher_grafico(s: &mut Superficie, dentro: Retangulo, valores: &[u32], y_de: &dyn Fn(u32) -> i32, cor: Cor) {
    let n = valores.len();
    let colunas = dentro.largura.max(1) as i64;
    for coluna in 0..colunas {
        let y = if n == 1 || colunas == 1 {
            y_de(valores[0])
        } else {
            // posicao da coluna na sequencia, em 1/(colunas - 1) de ponto
            let passo = coluna * (n as i64 - 1);
            let (i, resto) = ((passo / (colunas - 1)) as usize, passo % (colunas - 1));
            let (a, b) = (y_de(valores[i]), y_de(valores[(i + 1).min(n - 1)]));
            a + ((b - a) as i64 * resto / (colunas - 1)) as i32
        };
        let x = dentro.x + coluna as i32;
        s.retangulo(Retangulo::novo(x, y, 1, dentro.baixo() - y), cor);
    }
}

/// A linha em si, com 2 px de espessura.
fn tracar_grafico(s: &mut Superficie, dentro: Retangulo, valores: &[u32], y_de: &dyn Fn(u32) -> i32, cor: Cor) {
    let n = valores.len();
    if n == 1 {
        let y = y_de(valores[0]);
        s.retangulo(Retangulo::novo(dentro.x, y - 1, dentro.largura, 2), cor);
        return;
    }
    for i in 0..n - 1 {
        let (x0, x1) = (x_do_ponto(dentro, i, n), x_do_ponto(dentro, i + 1, n));
        let (y0, y1) = (y_de(valores[i]), y_de(valores[i + 1]));
        s.linha(x0, y0, x1, y1, cor);
        s.linha(x0, y0 - 1, x1, y1 - 1, cor);
    }
}

// ------------------------------------------------------------------ icones

/// Pictogramas 16x16 desenhados com retangulos, linhas e discos.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icone {
    Pasta,
    Arquivo,
    Texto,
    Html,
    Disco,
    Rede,
    Terminal,
    Navegador,
    Engrenagem,
    Tarefas,
    Chip,
    Pincel,
    Calculadora,
    Info,
    Teclado,
    Monitor,
    Som,
    Voltar,
    Avancar,
    Recarregar,
    Casa,
    Cima,
}

// Cores fixas dos icones coloridos (os de navegacao usam o tema).
const AMARELO: Cor = 0x00F5_C04A;
const AMARELO_ESCURO: Cor = 0x00D9_962B;
const AMARELO_CLARO: Cor = 0x00F9_D67F;
const DOBRA: Cor = 0x00D5_DBE4;
const CONTORNO: Cor = 0x0078_8394;
const CINZA: Cor = 0x00A3_ACBA;
const CINZA_CLARO: Cor = 0x00D3_D9E1;
const CINZA_ESCURO: Cor = 0x004A_5263;
/// Metal da engrenagem: claro o bastante para os papeis de parede escuros.
const ACO: Cor = 0x0086_91A3;
const GRAFITE: Cor = 0x0025_2C3A;
/// Contorno dos icones escuros, para aparecerem no tema escuro.
const BORDA_GRAFITE: Cor = 0x005B_6576;
const AZUL: Cor = 0x0030_7FDD;
const AZUL_CLARO: Cor = 0x0078_B4F0;
const VERDE: Cor = 0x0034_B864;
const VERDE_VISOR: Cor = 0x00BF_E8C4;
const LARANJA: Cor = 0x00EF_7D2B;
const MADEIRA: Cor = 0x00C0_8040;

/// Desenha no quadriculado 16x16 do icone: cada quadradinho vira um bloco
/// de e x e pixels na Superficie (o icone cresce sem borrar).
struct Pincel<'a> {
    s: &'a mut Superficie,
    x: i32,
    y: i32,
    e: i32,
}

impl Pincel<'_> {
    fn r(&mut self, x: i32, y: i32, largura: i32, altura: i32, cor: Cor) {
        let (e, x0, y0) = (self.e, self.x, self.y);
        self.s.retangulo(Retangulo::novo(x0 + x * e, y0 + y * e, largura * e, altura * e), cor);
    }

    fn p(&mut self, x: i32, y: i32, cor: Cor) {
        self.r(x, y, 1, 1, cor);
    }

    /// Linha de um quadradinho de largura (Bresenham no quadriculado).
    fn l(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, cor: Cor) {
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (passo_x, passo_y) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let (mut x, mut y, mut erro) = (x0, y0, dx + dy);
        loop {
            self.p(x, y, cor);
            if x == x1 && y == y1 {
                return;
            }
            let dobro = 2 * erro;
            if dobro >= dy {
                erro += dy;
                x += passo_x;
            }
            if dobro <= dx {
                erro += dx;
                y += passo_y;
            }
        }
    }

    /// Disco centrado no meio do icone (7,5; 7,5).
    fn redondo(&mut self, raio: i32, cor: Cor) {
        for y in 0..16 {
            let dy = 2 * y - 15;
            let limite = 4 * raio * raio + 2 - dy * dy;
            if limite < 0 {
                continue;
            }
            let meia = raiz(limite); // em meios quadradinhos
            let (x0, x1) = ((16 - meia) / 2, (15 + meia) / 2);
            self.r(x0, y, x1 - x0 + 1, 1, cor);
        }
    }

    /// Anel centrado no meio do icone, sem os quadradinhos de `pular`.
    fn anel(&mut self, externo: i32, interno: i32, cor: Cor, pular: impl Fn(i32, i32) -> bool) {
        for y in 0..16 {
            for x in 0..16 {
                let (dx, dy) = (2 * x - 15, 2 * y - 15);
                let d = dx * dx + dy * dy;
                if d <= 4 * externo * externo + 2 && d > 4 * interno * interno + 2 && !pular(x, y) {
                    self.p(x, y, cor);
                }
            }
        }
    }

    /// Moldura de um quadradinho em volta de (x, y, largura, altura).
    fn moldura(&mut self, x: i32, y: i32, largura: i32, altura: i32, cor: Cor) {
        let (direita, baixo) = (x + largura - 1, y + altura - 1);
        self.l(x, y, direita, y, cor);
        self.l(x, baixo, direita, baixo, cor);
        self.l(x, y, x, baixo, cor);
        self.l(direita, y, direita, baixo, cor);
    }
}

/// Maior n com n * n <= valor (valores pequenos).
fn raiz(valor: i32) -> i32 {
    let mut n = 0;
    while (n + 1) * (n + 1) <= valor {
        n += 1;
    }
    n
}

/// Desenha o icone com o canto em (x, y): 16 x 16 pixels vezes `escala`.
/// Os de navegacao (Voltar, Avancar, Recarregar, Casa, Cima) usam a cor do
/// texto dos botoes do tema; os outros tem cores proprias.
pub fn icone(s: &mut Superficie, x: i32, y: i32, tipo: Icone, escala: i32) {
    let frente = tema::atual().botao_texto;
    let mut p = Pincel { s, x, y, e: escala.max(1) };
    match tipo {
        Icone::Pasta => pasta(&mut p),
        Icone::Arquivo => pagina(&mut p),
        Icone::Texto => folha_de_texto(&mut p),
        Icone::Html => folha_html(&mut p),
        Icone::Disco => disco(&mut p),
        Icone::Rede => rede(&mut p),
        Icone::Terminal => terminal(&mut p),
        Icone::Navegador => globo(&mut p),
        Icone::Engrenagem => engrenagem(&mut p),
        Icone::Tarefas => tarefas(&mut p),
        Icone::Chip => chip(&mut p),
        Icone::Pincel => pincel(&mut p),
        Icone::Calculadora => calculadora(&mut p),
        Icone::Info => info(&mut p),
        Icone::Teclado => teclado(&mut p),
        Icone::Monitor => monitor(&mut p),
        Icone::Som => som(&mut p),
        Icone::Voltar => voltar(&mut p, frente),
        Icone::Avancar => avancar(&mut p, frente),
        Icone::Recarregar => recarregar(&mut p, frente),
        Icone::Casa => casa(&mut p, frente),
        Icone::Cima => cima(&mut p, frente),
    }
}

fn pasta(p: &mut Pincel) {
    p.r(1, 2, 6, 2, AMARELO_ESCURO); // aba
    p.r(1, 3, 14, 11, AMARELO_ESCURO); // fundo
    p.r(1, 5, 14, 9, AMARELO); // frente
    p.r(1, 5, 14, 1, AMARELO_CLARO);
    p.r(1, 13, 14, 1, AMARELO_ESCURO);
}

/// Folha com o canto de cima dobrado (base de Arquivo, Texto e Html).
fn pagina(p: &mut Pincel) {
    p.r(3, 1, 6, 14, BRANCO);
    p.r(9, 5, 4, 10, BRANCO);
    // dobra: o triangulo embaixo da diagonal (9,1)-(12,4)
    p.r(9, 1, 1, 4, DOBRA);
    p.r(10, 2, 1, 3, DOBRA);
    p.r(11, 3, 1, 2, DOBRA);
    p.p(12, 4, DOBRA);
    p.l(9, 2, 9, 4, CINZA);
    p.l(10, 4, 11, 4, CINZA);
    p.l(3, 1, 9, 1, CONTORNO);
    p.l(9, 1, 12, 4, CONTORNO);
    p.l(12, 4, 12, 14, CONTORNO);
    p.l(3, 14, 12, 14, CONTORNO);
    p.l(3, 1, 3, 14, CONTORNO);
}

fn folha_de_texto(p: &mut Pincel) {
    pagina(p);
    p.r(5, 3, 3, 1, CINZA);
    for linha in 0..4 {
        let largura = if linha == 3 { 4 } else { 6 };
        p.r(5, 6 + 2 * linha, largura, 1, CINZA);
    }
}

fn folha_html(p: &mut Pincel) {
    pagina(p);
    p.l(7, 6, 5, 8, LARANJA);
    p.l(5, 8, 7, 10, LARANJA);
    p.l(9, 6, 11, 8, LARANJA);
    p.l(11, 8, 9, 10, LARANJA);
    p.r(5, 12, 6, 1, AZUL);
}

fn disco(p: &mut Pincel) {
    p.r(1, 4, 14, 9, CINZA_CLARO);
    p.r(1, 10, 14, 3, CINZA);
    p.r(3, 7, 7, 1, CINZA);
    p.r(11, 11, 2, 1, VERDE); // luz de atividade
    p.moldura(1, 4, 14, 9, CONTORNO);
}

fn rede(p: &mut Pincel) {
    // tres computadores ligados por fios
    p.r(5, 1, 6, 4, AZUL);
    p.r(6, 2, 4, 2, AZUL_CLARO);
    p.r(7, 5, 2, 2, CINZA_ESCURO);
    p.r(2, 7, 12, 1, CINZA_ESCURO);
    p.r(2, 8, 2, 2, CINZA_ESCURO);
    p.r(12, 8, 2, 2, CINZA_ESCURO);
    p.r(0, 10, 6, 4, AZUL);
    p.r(1, 11, 4, 2, AZUL_CLARO);
    p.r(10, 10, 6, 4, AZUL);
    p.r(11, 11, 4, 2, AZUL_CLARO);
}

fn terminal(p: &mut Pincel) {
    p.r(1, 2, 14, 12, GRAFITE);
    p.r(1, 2, 14, 2, CINZA_ESCURO);
    p.l(3, 6, 5, 8, VERDE); // >
    p.l(5, 8, 3, 10, VERDE);
    p.r(7, 10, 4, 1, VERDE); // _
    p.moldura(1, 2, 14, 12, BORDA_GRAFITE);
}

fn globo(p: &mut Pincel) {
    p.redondo(6, AZUL);
    // continentes
    p.r(5, 4, 3, 2, VERDE);
    p.r(4, 6, 4, 2, VERDE);
    p.r(6, 8, 2, 1, VERDE);
    p.r(6, 9, 1, 2, VERDE);
    p.r(10, 8, 3, 2, VERDE);
    p.r(9, 10, 3, 2, VERDE);
    p.p(10, 12, VERDE);
    // brilho
    p.r(10, 4, 2, 1, AZUL_CLARO);
    p.p(12, 5, AZUL_CLARO);
}

fn engrenagem(p: &mut Pincel) {
    // dentes: quatro retos e quatro nas diagonais
    for (x, y, l, a) in [(6, 0, 4, 3), (6, 13, 4, 3), (0, 6, 3, 4), (13, 6, 3, 4)] {
        p.r(x, y, l, a, ACO);
    }
    for (x, y) in [(2, 2), (11, 2), (2, 11), (11, 11)] {
        p.r(x, y, 3, 3, ACO);
    }
    p.redondo(5, ACO);
    p.redondo(3, CINZA_ESCURO);
    p.redondo(2, CINZA_CLARO); // eixo
}

fn tarefas(p: &mut Pincel) {
    p.r(1, 2, 14, 12, GRAFITE);
    p.r(2, 8, 12, 1, CINZA_ESCURO);
    // batimento
    p.l(2, 9, 5, 9, VERDE);
    p.l(5, 9, 7, 4, VERDE);
    p.l(7, 4, 9, 12, VERDE);
    p.l(9, 12, 11, 7, VERDE);
    p.l(11, 7, 13, 7, VERDE);
    p.moldura(1, 2, 14, 12, BORDA_GRAFITE);
}

fn chip(p: &mut Pincel) {
    for c in [4, 6, 9, 11] {
        p.r(c, 1, 1, 2, CINZA);
        p.r(c, 13, 1, 2, CINZA);
        p.r(1, c, 2, 1, CINZA);
        p.r(13, c, 2, 1, CINZA);
    }
    p.r(3, 3, 10, 10, GRAFITE);
    p.moldura(3, 3, 10, 10, BORDA_GRAFITE);
    p.r(5, 5, 2, 2, CINZA);
}

fn pincel(p: &mut Pincel) {
    // cabo na diagonal, com 2 de largura
    p.l(14, 1, 10, 5, MADEIRA);
    p.l(14, 2, 10, 6, MADEIRA);
    // aro de metal
    p.l(9, 6, 8, 7, CINZA);
    p.l(9, 7, 8, 8, CINZA);
    // ponta com tinta
    p.r(5, 8, 3, 3, LARANJA);
    p.r(3, 10, 4, 3, LARANJA);
    p.r(2, 13, 2, 1, LARANJA);
}

fn calculadora(p: &mut Pincel) {
    p.r(3, 1, 10, 14, CINZA_ESCURO);
    p.r(4, 2, 8, 3, VERDE_VISOR);
    for linha in 0..3 {
        for coluna in 0..3 {
            p.r(4 + 3 * coluna, 6 + 3 * linha, 2, 2, CINZA_CLARO);
        }
    }
    p.r(10, 12, 2, 2, LARANJA); // tecla de igual
}

fn info(p: &mut Pincel) {
    p.redondo(7, AZUL);
    p.r(7, 3, 2, 2, BRANCO);
    p.r(7, 6, 2, 5, BRANCO);
    p.r(6, 11, 4, 1, BRANCO);
}

fn teclado(p: &mut Pincel) {
    p.r(0, 4, 16, 9, CINZA_ESCURO);
    for i in 0..7 {
        p.p(1 + 2 * i, 6, CINZA_CLARO);
    }
    for i in 0..6 {
        p.p(2 + 2 * i, 8, CINZA_CLARO);
    }
    p.r(4, 10, 8, 1, CINZA_CLARO); // espaco
}

fn monitor(p: &mut Pincel) {
    p.r(1, 2, 14, 10, GRAFITE);
    p.r(2, 3, 12, 8, AZUL);
    p.r(2, 3, 12, 2, AZUL_CLARO);
    p.r(7, 12, 2, 2, CINZA_ESCURO);
    p.r(4, 14, 8, 1, CINZA_ESCURO);
}

fn som(p: &mut Pincel) {
    p.r(2, 6, 3, 4, CINZA_ESCURO);
    for i in 0..5 {
        p.r(5 + i, 5 - i, 1, 6 + 2 * i, CINZA_ESCURO); // cone
    }
    // ondas
    p.p(11, 6, AZUL);
    p.r(12, 7, 1, 2, AZUL);
    p.p(11, 9, AZUL);
    p.p(12, 3, AZUL);
    p.p(13, 4, AZUL);
    p.r(14, 5, 1, 6, AZUL);
    p.p(13, 11, AZUL);
    p.p(12, 12, AZUL);
}

fn voltar(p: &mut Pincel, cor: Cor) {
    p.r(3, 7, 11, 2, cor);
    p.l(2, 7, 6, 3, cor);
    p.l(3, 7, 7, 3, cor);
    p.l(2, 8, 6, 12, cor);
    p.l(3, 8, 7, 12, cor);
}

fn avancar(p: &mut Pincel, cor: Cor) {
    p.r(2, 7, 11, 2, cor);
    p.l(13, 7, 9, 3, cor);
    p.l(12, 7, 8, 3, cor);
    p.l(13, 8, 9, 12, cor);
    p.l(12, 8, 8, 12, cor);
}

fn cima(p: &mut Pincel, cor: Cor) {
    p.r(7, 3, 2, 11, cor);
    p.l(7, 2, 3, 6, cor);
    p.l(7, 3, 3, 7, cor);
    p.l(8, 2, 12, 6, cor);
    p.l(8, 3, 12, 7, cor);
}

fn recarregar(p: &mut Pincel, cor: Cor) {
    // tres quartos de anel (falta o quarto de cima a direita) e a ponta
    p.anel(6, 4, cor, |x, y| x >= 8 && y <= 7);
    p.r(8, 0, 1, 6, cor);
    p.r(9, 1, 1, 4, cor);
    p.r(10, 2, 1, 2, cor);
}

fn casa(p: &mut Pincel, cor: Cor) {
    for i in 0..7 {
        p.r(7 - i, 1 + i, 2 + 2 * i, 1, cor); // telhado
    }
    p.r(3, 8, 4, 7, cor);
    p.r(9, 8, 4, 7, cor);
    p.r(7, 8, 2, 2, cor); // em cima da porta
}

// ------------------------------------------------------------ clique duplo

/// Reconhece dois apertos seguidos no mesmo alvo (linha, icone...).
#[derive(Clone, Copy, Debug, Default)]
pub struct DuploClique {
    /// (contador da CPU, alvo) do ultimo aperto.
    ultimo: Option<(u64, usize)>,
}

impl DuploClique {
    pub const fn novo() -> DuploClique {
        DuploClique { ultimo: None }
    }

    /// Conta um aperto em `alvo`. true = fechou um clique duplo (o
    /// proximo aperto comeca de novo).
    pub fn apertar(&mut self, alvo: usize) -> bool {
        let agora = cpu::contador();
        let limite = cpu::frequencia_timer() * MS_CLIQUE_DUPLO / 1000;
        let duplo = self.ultimo.is_some_and(|(quando, a)| a == alvo && agora.wrapping_sub(quando) < limite);
        self.ultimo = if duplo { None } else { Some((agora, alvo)) };
        duplo
    }

    pub fn esquecer(&mut self) {
        self.ultimo = None;
    }
}

// ------------------------------------------------------------- CampoTexto

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AcaoCampo {
    Nada,
    /// O texto, o cursor ou o foco mudaram: redesenhar.
    Mudou,
    /// Enter com o campo focado.
    Confirmou,
    /// Esc com o campo focado.
    Cancelou,
}

/// Campo de uma linha so (texto Unicode, sem caracteres de controle). Com
/// foco: caracteres, 8 (Backspace), 13 (Enter = Confirmou), 27 (Esc =
/// Cancelou) e as teclas Esquerda, Direita, Home, End e Delete. O cursor
/// anda de caractere em caractere (um "ç" e um passo so) e o texto rola
/// para o cursor sempre aparecer. Quem passar o Tique ao campo ganha o
/// cursor piscando (Mudou a cada meio segundo, so com foco).
pub struct CampoTexto {
    texto: String,
    /// Byte do cursor (antes do caractere que comeca nele).
    cursor: usize,
    /// Byte do primeiro caractere visivel.
    rolagem: usize,
    focado: bool,
    /// Cursor apagado pelo pisca-pisca.
    apagado: bool,
    /// Tiques desde a ultima piscada.
    tiques: u8,
}

/// Largura em pixels onde o texto do campo aparece.
fn largura_do_campo(area: Retangulo) -> i32 {
    (area.largura - 2 * MARGEM_TEXTO).max(1)
}

/// Byte do caractere antes de `i` (0 se `i` ja e o comeco).
fn anterior(texto: &str, i: usize) -> usize {
    texto[..i].char_indices().next_back().map_or(0, |(j, _)| j)
}

/// Byte do caractere depois do que comeca em `i` (o fim se nao houver).
fn seguinte(texto: &str, i: usize) -> usize {
    texto[i..].chars().next().map_or(texto.len(), |c| i + c.len_utf8())
}

/// O campo aceita este caractere no texto?
fn imprimivel(c: char) -> bool {
    !c.is_control()
}

/// Tiques (de 100 ms) entre duas piscadas do cursor.
const TIQUES_PISCADA: u8 = 5;

impl CampoTexto {
    pub fn novo(texto: &str) -> Self {
        let mut campo = CampoTexto { texto: String::new(), cursor: 0, rolagem: 0, focado: false, apagado: false, tiques: 0 };
        campo.definir(texto);
        campo
    }

    pub fn texto(&self) -> &str {
        &self.texto
    }

    /// Troca o texto todo (os caracteres de controle ficam de fora) e poe
    /// o cursor no fim.
    pub fn definir(&mut self, texto: &str) {
        self.texto = texto.chars().filter(|&c| imprimivel(c)).take(MAX_CAMPO).collect();
        self.cursor = self.texto.len();
        self.rolagem = 0;
    }

    pub fn focado(&self) -> bool {
        self.focado
    }

    pub fn focar(&mut self, sim: bool) {
        self.focado = sim;
        self.acender();
    }

    pub fn evento(&mut self, e: EventoApp, area: Retangulo) -> AcaoCampo {
        let acao = match e {
            EventoApp::Botao { x, y, botao: Botao::Esquerdo, pressionado: true } => self.clique(x, y, area),
            EventoApp::Caractere(c) if self.focado => self.caractere(c),
            EventoApp::Tecla { codigo, pressionada: true } if self.focado => self.tecla(codigo),
            EventoApp::Tique if self.focado => return self.piscar(),
            _ => AcaoCampo::Nada,
        };
        if acao == AcaoCampo::Mudou {
            self.rolagem = self.inicio_visivel(area);
            self.acender();
        }
        acao
    }

    /// Cursor aceso de novo e a contagem da piscada recomecada (depois de
    /// qualquer mudanca o cursor aparece na hora).
    fn acender(&mut self) {
        self.apagado = false;
        self.tiques = 0;
    }

    fn piscar(&mut self) -> AcaoCampo {
        self.tiques += 1;
        if self.tiques < TIQUES_PISCADA {
            return AcaoCampo::Nada;
        }
        self.tiques = 0;
        self.apagado = !self.apagado;
        AcaoCampo::Mudou
    }

    /// Clique dentro: foca e poe o cursor entre os caracteres mais perto
    /// do ponteiro (do jeito que estava na tela); fora: perde o foco.
    fn clique(&mut self, x: i32, y: i32, area: Retangulo) -> AcaoCampo {
        if !area.contem(x, y) {
            if !self.focado {
                return AcaoCampo::Nada;
            }
            self.focado = false;
            return AcaoCampo::Mudou;
        }
        let inicio = self.inicio_visivel(area);
        let cursor = inicio + fonte::indice_no_x(&self.texto[inicio..], x - area.x - MARGEM_TEXTO, &INTER);
        let mudou = !self.focado || cursor != self.cursor;
        self.focado = true;
        self.rolagem = inicio; // o texto fica onde estava
        self.cursor = cursor;
        if mudou { AcaoCampo::Mudou } else { AcaoCampo::Nada }
    }

    fn caractere(&mut self, c: char) -> AcaoCampo {
        match c {
            '\r' => AcaoCampo::Confirmou,
            '\x1b' => AcaoCampo::Cancelou,
            '\x08' if self.cursor > 0 => {
                self.cursor = anterior(&self.texto, self.cursor);
                self.texto.remove(self.cursor);
                AcaoCampo::Mudou
            }
            c if imprimivel(c) && self.texto.chars().count() < MAX_CAMPO => {
                self.texto.insert(self.cursor, c);
                self.cursor += c.len_utf8();
                AcaoCampo::Mudou
            }
            _ => AcaoCampo::Nada,
        }
    }

    fn tecla(&mut self, codigo: u16) -> AcaoCampo {
        let antes = self.cursor;
        match codigo {
            KEY_LEFT => self.cursor = anterior(&self.texto, self.cursor),
            KEY_RIGHT => self.cursor = seguinte(&self.texto, self.cursor),
            KEY_HOME => self.cursor = 0,
            KEY_END => self.cursor = self.texto.len(),
            KEY_DELETE if self.cursor < self.texto.len() => {
                self.texto.remove(self.cursor);
                return AcaoCampo::Mudou;
            }
            _ => return AcaoCampo::Nada,
        }
        if self.cursor != antes { AcaoCampo::Mudou } else { AcaoCampo::Nada }
    }

    /// Byte do primeiro caractere a mostrar. Sem foco, o comeco do texto;
    /// com foco, a rolagem guardada, mexida so o necessario para o cursor
    /// aparecer (e sem sobrar espaco no fim).
    fn inicio_visivel(&self, area: Retangulo) -> usize {
        if !self.focado {
            return 0;
        }
        let cabe = largura_do_campo(area) - 1; // 1 px para a barrinha do cursor
        let mut inicio = self.rolagem.min(self.cursor);
        while inicio < self.cursor && INTER.largura(&self.texto[inicio..self.cursor]) > cabe {
            inicio = seguinte(&self.texto, inicio);
        }
        while inicio > 0 && INTER.largura(&self.texto[anterior(&self.texto, inicio)..]) <= cabe {
            inicio = anterior(&self.texto, inicio);
        }
        inicio
    }

    pub fn desenhar(&self, s: &mut Superficie, area: Retangulo) {
        if area.vazio() {
            return;
        }
        let t = tema::atual();
        s.retangulo(area, cor_campo(&t));
        if self.focado {
            s.contorno(area, t.destaque, 2);
        } else {
            s.contorno(area, cor_linha(&t), 1);
        }
        let inicio = self.inicio_visivel(area);
        let visivel = fonte::cortar(&self.texto[inicio..], largura_do_campo(area), &INTER);
        let x = area.x + MARGEM_TEXTO;
        texto_na_faixa(s, x, area, visivel, t.texto, &INTER);
        if self.focado && !self.apagado {
            // barrinha vertical logo depois do caractere anterior
            let x = x + INTER.largura(&self.texto[inicio..self.cursor]);
            let altura = (area.altura - 8).clamp(8, 16);
            s.retangulo(Retangulo::novo(x, area.y + (area.altura - altura) / 2, 1, altura), t.texto);
        }
    }
}

// ------------------------------------------------------------------ Lista

/// Uma linha da Lista: icone opcional (na primeira coluna) e o texto de
/// cada coluna.
pub struct ItemLista {
    pub icone: Option<Icone>,
    pub colunas: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AcaoLista {
    Nada,
    /// Clique numa linha ou setas: a selecao e esta.
    Selecionou(usize),
    /// Clique duplo numa linha ou Enter com uma linha escolhida.
    Abriu(usize),
    /// So a aparencia mudou (rolagem, foco, selecao desfeita): redesenhar.
    Rolou,
}

/// Lista com cabecalho, colunas, selecao e barra de rolagem.
///   - mouse: clique seleciona, clique duplo abre, clique no vazio tira a
///     selecao, roda rola, barra de rolagem com polegar arrastavel e
///     clique acima/abaixo dele
///   - teclado (com foco): Cima, Baixo, PageUp, PageDown, Home, End, Enter
pub struct Lista {
    titulos: Vec<String>,
    larguras: Vec<i32>,
    itens: Vec<ItemLista>,
    selecionado: Option<usize>,
    /// Primeira linha visivel.
    rolagem: usize,
    /// selecionar() pediu para a selecao aparecer (vale no proximo evento).
    mostrar_selecao: bool,
    focada: bool,
    duplo: DuploClique,
    /// Arrastando o polegar: distancia do ponteiro ao topo dele.
    arrasto: Option<i32>,
}

/// Onde fica cada parte da lista dentro da area.
struct Partes {
    cabecalho: Retangulo,
    linhas: Retangulo,
    barra: Option<Retangulo>,
    /// Linhas inteiras que cabem.
    visiveis: usize,
}

/// Rolagem que mostra `linha`, mexendo o minimo na `rolagem` atual.
fn rolagem_para_mostrar(rolagem: usize, linha: usize, visiveis: usize) -> usize {
    let visiveis = visiveis.max(1);
    if linha < rolagem {
        linha
    } else if linha >= rolagem + visiveis {
        linha + 1 - visiveis
    } else {
        rolagem
    }
}

impl Lista {
    pub fn nova() -> Self {
        Lista {
            titulos: Vec::new(),
            larguras: Vec::new(),
            itens: Vec::new(),
            selecionado: None,
            rolagem: 0,
            mostrar_selecao: false,
            focada: true,
            duplo: DuploClique::novo(),
            arrasto: None,
        }
    }

    /// Titulos do cabecalho e largura de cada coluna em pixels (a ultima
    /// coluna vai ate a borda). Sem titulos a lista nao tem cabecalho.
    pub fn definir_colunas(&mut self, titulos: &[&str], larguras: &[i32]) {
        self.titulos = titulos.iter().map(|&t| String::from(t)).collect();
        self.larguras = larguras.to_vec();
    }

    /// Troca as linhas. Com a mesma quantidade de antes (uma atualizacao)
    /// a selecao e a rolagem ficam; com outra quantidade a selecao some e a
    /// lista volta para o topo.
    pub fn definir_itens(&mut self, itens: Vec<ItemLista>) {
        if itens.len() != self.itens.len() {
            self.selecionado = None;
            self.rolagem = 0;
            self.mostrar_selecao = false;
            self.duplo.esquecer();
        }
        self.itens = itens;
    }

    #[allow(dead_code)] // para os apps lerem as linhas de volta
    pub fn itens(&self) -> &[ItemLista] {
        &self.itens
    }

    pub fn selecionado(&self) -> Option<usize> {
        self.selecionado
    }

    /// Escolhe a linha (None = nenhuma); a lista rola para ela aparecer.
    pub fn selecionar(&mut self, i: Option<usize>) {
        let nova = i.filter(|&i| i < self.itens.len());
        if nova != self.selecionado {
            self.duplo.esquecer();
        }
        self.selecionado = nova;
        self.mostrar_selecao = nova.is_some();
    }

    /// A lista atende o teclado? (comeca com foco; clique fora tira)
    pub fn focada(&self) -> bool {
        self.focada
    }

    pub fn focar(&mut self, sim: bool) {
        self.focada = sim;
    }

    fn partes(&self, area: Retangulo) -> Partes {
        let dentro = Retangulo::novo(area.x + 1, area.y + 1, area.largura - 2, area.altura - 2);
        let alto_cabecalho = if self.titulos.is_empty() { 0 } else { ALTURA_LINHA.min(dentro.altura.max(0)) };
        let cabecalho = Retangulo::novo(dentro.x, dentro.y, dentro.largura, alto_cabecalho);
        let corpo = Retangulo::novo(dentro.x, dentro.y + alto_cabecalho, dentro.largura, dentro.altura - alto_cabecalho);
        let visiveis = (corpo.altura / ALTURA_LINHA).max(0) as usize;
        let precisa_barra = self.itens.len() > visiveis && corpo.largura > 3 * LARGURA_ROLAGEM;
        let barra = precisa_barra
            .then(|| Retangulo::novo(corpo.direita() - LARGURA_ROLAGEM, corpo.y, LARGURA_ROLAGEM, corpo.altura));
        let largura_linhas = corpo.largura - if precisa_barra { LARGURA_ROLAGEM } else { 0 };
        let linhas = Retangulo::novo(corpo.x, corpo.y, largura_linhas, corpo.altura);
        Partes { cabecalho, linhas, barra, visiveis }
    }

    fn maximo_rolagem(&self, visiveis: usize) -> usize {
        self.itens.len().saturating_sub(visiveis.max(1))
    }

    /// Rolagem para desenhar e para achar a linha de um clique: a guardada,
    /// presa ao maximo e, se pedido, mexida para a selecao aparecer.
    fn rolagem_efetiva(&self, visiveis: usize) -> usize {
        let mut rolagem = self.rolagem;
        if let Some(linha) = self.selecionado.filter(|_| self.mostrar_selecao) {
            rolagem = rolagem_para_mostrar(rolagem, linha, visiveis);
        }
        rolagem.min(self.maximo_rolagem(visiveis))
    }

    pub fn evento(&mut self, e: EventoApp, area: Retangulo) -> AcaoLista {
        let p = self.partes(area);
        self.rolagem = self.rolagem_efetiva(p.visiveis);
        self.mostrar_selecao = false;
        match e {
            EventoApp::Mouse { y, botoes, .. } => self.mover(y, botoes, &p),
            EventoApp::Botao { x, y, botao: Botao::Esquerdo, pressionado: true } => self.apertar(x, y, area, &p),
            EventoApp::Botao { botao: Botao::Esquerdo, pressionado: false, .. } => match self.arrasto.take() {
                Some(_) => AcaoLista::Rolou,
                None => AcaoLista::Nada,
            },
            EventoApp::Roda(passos) => self.rodar(passos, &p),
            EventoApp::Tecla { codigo, pressionada: true } if self.focada => self.tecla(codigo, &p),
            EventoApp::Caractere('\r') if self.focada => self.selecionado.map_or(AcaoLista::Nada, AcaoLista::Abriu),
            _ => AcaoLista::Nada,
        }
    }

    fn apertar(&mut self, x: i32, y: i32, area: Retangulo, p: &Partes) -> AcaoLista {
        if !area.contem(x, y) {
            // perdeu o foco: so a cor da selecao muda
            let tinha = core::mem::replace(&mut self.focada, false);
            return if tinha { AcaoLista::Rolou } else { AcaoLista::Nada };
        }
        let ganhou_foco = !core::mem::replace(&mut self.focada, true);
        if let Some(barra) = p.barra.filter(|b| b.contem(x, y)) {
            return self.clique_na_barra(y, barra, p.visiveis);
        }
        let fileira = (y - p.linhas.y) / ALTURA_LINHA;
        let linha = self.rolagem + fileira.max(0) as usize;
        let na_linha = p.linhas.contem(x, y) && (fileira as usize) < p.visiveis && linha < self.itens.len();
        if !na_linha {
            if !p.linhas.contem(x, y) {
                // cabecalho ou contorno
                return if ganhou_foco { AcaoLista::Rolou } else { AcaoLista::Nada };
            }
            // espaco vazio embaixo das linhas: tira a selecao
            self.duplo.esquecer();
            let tinha = self.selecionado.take().is_some();
            return if tinha || ganhou_foco { AcaoLista::Rolou } else { AcaoLista::Nada };
        }
        self.selecionado = Some(linha);
        if self.duplo.apertar(linha) { AcaoLista::Abriu(linha) } else { AcaoLista::Selecionou(linha) }
    }

    /// O polegar da barra: altura proporcional ao que aparece, posicao pela
    /// rolagem.
    fn polegar(&self, barra: Retangulo, visiveis: usize, rolagem: usize) -> Retangulo {
        let total = self.itens.len().max(1) as i64;
        let proporcional = (barra.altura as i64 * visiveis as i64 / total) as i32;
        let altura = proporcional.clamp(MIN_POLEGAR.min(barra.altura), barra.altura);
        let maximo = self.maximo_rolagem(visiveis) as i64;
        let curso = (barra.altura - altura) as i64;
        let deslocamento = if maximo == 0 { 0 } else { (curso * rolagem.min(maximo as usize) as i64 / maximo) as i32 };
        Retangulo::novo(barra.x + 2, barra.y + deslocamento, barra.largura - 4, altura)
    }

    /// Clique na barra: no polegar comeca a arrastar; acima ou abaixo dele
    /// rola uma pagina.
    fn clique_na_barra(&mut self, y: i32, barra: Retangulo, visiveis: usize) -> AcaoLista {
        let polegar = self.polegar(barra, visiveis, self.rolagem);
        let pagina = visiveis.max(1);
        if y >= polegar.y && y < polegar.baixo() {
            self.arrasto = Some(y - polegar.y);
        } else if y < polegar.y {
            self.rolagem = self.rolagem.saturating_sub(pagina);
        } else {
            self.rolagem = (self.rolagem + pagina).min(self.maximo_rolagem(visiveis));
        }
        AcaoLista::Rolou
    }

    /// Ponteiro andou: arrastando o polegar, rola.
    fn mover(&mut self, y: i32, botoes: u8, p: &Partes) -> AcaoLista {
        let Some(dy) = self.arrasto else { return AcaoLista::Nada };
        let barra = match p.barra {
            Some(barra) if botoes & 1 != 0 => barra,
            _ => {
                self.arrasto = None; // soltou fora da janela (ou a barra sumiu)
                return AcaoLista::Rolou;
            }
        };
        let polegar = self.polegar(barra, p.visiveis, self.rolagem);
        let curso = (barra.altura - polegar.altura).max(1) as i64;
        let topo = ((y - dy - barra.y) as i64).clamp(0, curso);
        let maximo = self.maximo_rolagem(p.visiveis) as i64;
        let nova = ((topo * maximo + curso / 2) / curso) as usize;
        if nova == self.rolagem {
            return AcaoLista::Nada;
        }
        self.rolagem = nova;
        AcaoLista::Rolou
    }

    /// Roda: 3 linhas por passo (o gerenciador so manda a roda para a
    /// janela embaixo do ponteiro).
    fn rodar(&mut self, passos: i32, p: &Partes) -> AcaoLista {
        let delta = passos.unsigned_abs() as usize * LINHAS_POR_PASSO_DA_RODA;
        let nova = if passos > 0 {
            self.rolagem.saturating_sub(delta)
        } else {
            (self.rolagem + delta).min(self.maximo_rolagem(p.visiveis))
        };
        if nova == self.rolagem {
            return AcaoLista::Nada;
        }
        self.rolagem = nova;
        AcaoLista::Rolou
    }

    fn tecla(&mut self, codigo: u16, p: &Partes) -> AcaoLista {
        let n = self.itens.len();
        if n == 0 {
            return AcaoLista::Nada;
        }
        let pagina = p.visiveis.max(1);
        let atual = self.selecionado;
        let nova = match codigo {
            KEY_UP => atual.map_or(0, |s| s.saturating_sub(1)),
            KEY_DOWN => atual.map_or(0, |s| (s + 1).min(n - 1)),
            KEY_PAGEUP => atual.map_or(0, |s| s.saturating_sub(pagina)),
            KEY_PAGEDOWN => atual.map_or(0, |s| (s + pagina).min(n - 1)),
            KEY_HOME => 0,
            KEY_END => n - 1,
            _ => return AcaoLista::Nada,
        };
        let antes = self.rolagem;
        self.rolagem = rolagem_para_mostrar(self.rolagem, nova, p.visiveis).min(self.maximo_rolagem(p.visiveis));
        if atual == Some(nova) {
            return if self.rolagem != antes { AcaoLista::Rolou } else { AcaoLista::Nada };
        }
        self.selecionado = Some(nova);
        self.duplo.esquecer();
        AcaoLista::Selecionou(nova)
    }

    /// Coluna `c` dentro de `linha` (a ultima vai ate o fim da linha).
    fn coluna(&self, c: usize, linha: Retangulo) -> Retangulo {
        if self.larguras.is_empty() {
            return linha;
        }
        let x = linha.x + self.larguras[..c.min(self.larguras.len())].iter().sum::<i32>();
        let largura = match self.larguras.get(c) {
            Some(&l) if c + 1 < self.larguras.len() => l.min(linha.direita() - x),
            _ => linha.direita() - x,
        };
        Retangulo::novo(x, linha.y, largura, linha.altura)
    }

    pub fn desenhar(&self, s: &mut Superficie, area: Retangulo) {
        if area.largura < 4 || area.altura < 4 {
            return;
        }
        let t = tema::atual();
        let p = self.partes(area);
        let fundo = cor_campo(&t);
        s.retangulo(area, fundo);
        s.contorno(area, cor_linha(&t), 1);
        self.desenhar_cabecalho(s, &p, &t);
        let rolagem = self.rolagem_efetiva(p.visiveis);
        for fileira in 0..p.visiveis {
            let indice = rolagem + fileira;
            if indice >= self.itens.len() {
                break;
            }
            let y = p.linhas.y + fileira as i32 * ALTURA_LINHA;
            let r = Retangulo::novo(p.linhas.x, y, p.linhas.largura, ALTURA_LINHA);
            self.desenhar_linha(s, r, indice, &t, fundo);
        }
        if let Some(barra) = p.barra {
            s.retangulo(barra, grafico::misturar(fundo, t.texto, 14));
            let cor = if self.arrasto.is_some() { t.destaque } else { grafico::misturar(fundo, t.texto, 80) };
            s.retangulo(self.polegar(barra, p.visiveis, rolagem), cor);
        }
    }

    fn desenhar_cabecalho(&self, s: &mut Superficie, p: &Partes, t: &Tema) {
        let r = p.cabecalho;
        if r.altura < ALTURA_LINHA {
            return;
        }
        s.retangulo(r, t.botao);
        s.retangulo(Retangulo::novo(r.x, r.baixo() - 1, r.largura, 1), cor_linha(t));
        for (c, titulo) in self.titulos.iter().enumerate() {
            let coluna = self.coluna(c, r);
            if coluna.largura <= 0 {
                break;
            }
            if c > 0 {
                s.retangulo(Retangulo::novo(coluna.x, coluna.y + 4, 1, coluna.altura - 8), cor_linha(t));
            }
            let texto = fonte::encurtar(titulo, coluna.largura - MARGEM_TEXTO - 4, &INTER_NEGRITO);
            texto_na_faixa(s, coluna.x + MARGEM_TEXTO, coluna, &texto, t.botao_texto, &INTER_NEGRITO);
        }
    }

    fn desenhar_linha(&self, s: &mut Superficie, r: Retangulo, indice: usize, t: &Tema, fundo: Cor) {
        let item = &self.itens[indice];
        let (cor_fundo, cor_texto) = match (self.selecionado == Some(indice), self.focada) {
            (true, true) => (t.destaque, BRANCO),
            (true, false) => (grafico::misturar(fundo, t.destaque, 80), t.texto),
            // zebra bem leve nas linhas impares
            (false, _) if indice % 2 == 1 => (grafico::misturar(fundo, t.texto, 9), t.texto),
            (false, _) => (fundo, t.texto),
        };
        s.retangulo(r, cor_fundo);
        for c in 0..self.larguras.len().max(1) {
            let Some(texto) = item.colunas.get(c) else { break };
            let coluna = self.coluna(c, r);
            let mut x = coluna.x + MARGEM_TEXTO;
            if let Some(tipo) = item.icone.filter(|_| c == 0) {
                icone(s, coluna.x + 4, r.y + (ALTURA_LINHA - 16) / 2, tipo, 1);
                x = coluna.x + 24;
            }
            let largura = coluna.direita() - x - 4;
            if largura > 0 {
                texto_na_faixa(s, x, r, &fonte::encurtar(texto, largura, &INTER), cor_texto, &INTER);
            }
        }
    }
}

// ------------------------------------------------------------------- Abas

/// Fileira de abas: a escolhida tem o texto forte e um traco de destaque
/// embaixo. `area` e a faixa das abas (altura sugerida: ALTURA_ABAS).
pub struct Abas {
    nomes: Vec<&'static str>,
    atual: usize,
}

impl Abas {
    pub fn novas(nomes: &[&'static str]) -> Self {
        Abas { nomes: nomes.to_vec(), atual: 0 }
    }

    pub fn atual(&self) -> usize {
        self.atual
    }

    pub fn definir(&mut self, i: usize) {
        if i < self.nomes.len() {
            self.atual = i;
        }
    }

    /// Retangulo da aba `i`: largura do nome (em seminegrito, como fica
    /// quando escolhida); se nao couberem todas, todas do mesmo tamanho.
    fn retangulo(&self, i: usize, area: Retangulo) -> Retangulo {
        let natural = |nome: &str| INTER_NEGRITO.largura(nome) + 2 * MARGEM_ABA;
        let n = self.nomes.len().max(1) as i32;
        let disponivel = area.largura - 8;
        let total: i32 = self.nomes.iter().map(|nome| natural(nome)).sum();
        if total <= disponivel {
            let x = area.x + 4 + self.nomes[..i].iter().map(|nome| natural(nome)).sum::<i32>();
            return Retangulo::novo(x, area.y, natural(self.nomes[i]), area.altura);
        }
        let largura = disponivel / n;
        Retangulo::novo(area.x + 4 + i as i32 * largura, area.y, largura, area.altura)
    }

    /// true = trocou de aba (clique numa aba que nao era a atual).
    pub fn evento(&mut self, e: EventoApp, area: Retangulo) -> bool {
        let EventoApp::Botao { x, y, botao: Botao::Esquerdo, pressionado: true } = e else { return false };
        let Some(i) = (0..self.nomes.len()).find(|&i| self.retangulo(i, area).contem(x, y)) else { return false };
        if i == self.atual {
            return false;
        }
        self.atual = i;
        true
    }

    pub fn desenhar(&self, s: &mut Superficie, area: Retangulo) {
        if area.vazio() {
            return;
        }
        let t = tema::atual();
        s.retangulo(area, t.fundo_janela);
        s.retangulo(Retangulo::novo(area.x, area.baixo() - 1, area.largura, 1), cor_linha(&t));
        for (i, nome) in self.nomes.iter().enumerate() {
            let r = self.retangulo(i, area);
            let ativa = i == self.atual;
            let (cor, f) = if ativa { (t.texto, &INTER_NEGRITO) } else { (cor_texto_fraco(&t), &INTER) };
            let texto = fonte::encurtar(nome, r.largura - 8, f);
            // 1 px acima do meio: o traco de destaque ocupa o pe da aba
            texto_centrado(s, Retangulo::novo(r.x, r.y - 1, r.largura, r.altura), &texto, cor, f);
            if ativa {
                s.retangulo(Retangulo::novo(r.x + 2, r.baixo() - 3, r.largura - 4, 3), t.destaque);
            }
        }
    }
}
