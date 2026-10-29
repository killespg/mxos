//! Desenho da Pagina numa Superficie, recortado a uma area e com
//! rolagem. So os itens que aparecem na area sao desenhados: as faixas da
//! Pagina (uma por bloco) sao achadas por busca binaria, entao uma pagina
//! comprida custa o mesmo que uma curta.
//!
//! O texto usa as fontes pre-renderizadas de crate::fonte (Inter e
//! JetBrains Mono, com antialiasing), desenhadas aqui mesmo, direto no Vec
//! de pixels e recortadas ao retangulo: uma linha cortada no alto ou no pe
//! da janela aparece pela metade em vez de vazar para fora. O negrito ja
//! vem da fonte (Inter seminegrito); so o <b> dentro de <code> e desenhado
//! duas vezes. Italico = o glifo inclinado (cada linha anda 1 px para a
//! direita a cada 4 px acima da linha de base).

use super::html::ITALICO;
use super::layout::{Item, Letra, Pagina, ESTILO_CODIGO, ESTILO_NEGRITO, ESTILO_SUBLINHADO};
use crate::fonte::{self, Glifo};
use crate::gui::grafico::{Cor, Retangulo, Superficie};

pub const COR_FUNDO: Cor = 0x00FF_FFFF;
pub const COR_TEXTO: Cor = 0x001F_2328;
pub const COR_LINK: Cor = 0x000B_57D0;
pub const COR_CODIGO: Cor = 0x009A_2C3C;
pub const COR_FUNDO_PRE: Cor = 0x00F3_F4F6;
pub const COR_REGUA: Cor = 0x00D0_D4DA;
pub const COR_CITACAO: Cor = 0x00C8_CDD4;
pub const COR_MARCADOR: Cor = 0x0050_5860;
pub const COR_CABECALHO: Cor = 0x00EC_EFF3;
pub const COR_GRADE: Cor = 0x00C4_CAD2;

/// Desenha a parte visivel da pagina dentro de `area` (coordenadas da
/// superficie). `rolagem` e quantos px da pagina ficaram para cima.
pub fn desenhar(pagina: &Pagina, s: &mut Superficie, area: Retangulo, rolagem: i32) {
    let limites = Retangulo::novo(0, 0, s.largura() as i32, s.altura() as i32);
    let recorte = area.intersecao(&limites);
    if recorte.vazio() {
        return;
    }
    s.retangulo(recorte, COR_FUNDO);
    // deslocamento pagina -> superficie
    let dx = area.x;
    let dy = area.y - rolagem;
    let (cima, baixo) = (recorte.y - dy, recorte.y + recorte.altura - dy);
    for item in itens_entre(pagina, cima, baixo) {
        let (topo, base) = item.faixa();
        if base <= cima || topo >= baixo {
            continue;
        }
        desenhar_item(s, recorte, item, dx, dy);
    }
}

/// Os itens das faixas que podem aparecer entre `cima` e `baixo` (y da
/// pagina), na ordem em que foram diagramados.
fn itens_entre(pagina: &Pagina, cima: i32, baixo: i32) -> &[Item] {
    let faixas = &pagina.faixas;
    if faixas.is_empty() {
        return &pagina.itens;
    }
    // base sobe em ordem: as faixas antes de `primeira` acabam acima de `cima`
    let primeira = faixas.partition_point(|f| f.base <= cima);
    // topo sobe em ordem: de `ultima` em diante tudo comeca abaixo de `baixo`
    let ultima = primeira + faixas[primeira..].partition_point(|f| f.topo < baixo);
    let inicio = faixas.get(primeira).map_or(pagina.itens.len(), |f| f.inicio);
    let fim = faixas.get(ultima).map_or(pagina.itens.len(), |f| f.inicio);
    pagina.itens.get(inicio..fim.max(inicio)).unwrap_or(&[])
}

fn desenhar_item(s: &mut Superficie, recorte: Retangulo, item: &Item, dx: i32, dy: i32) {
    match item {
        Item::Texto { x, base, texto, letra, estilo, link } => {
            let cor = if link.is_some() {
                COR_LINK
            } else if estilo & ESTILO_CODIGO != 0 {
                COR_CODIGO
            } else {
                COR_TEXTO
            };
            let caneta = Caneta { recorte, cor, italico: estilo & ITALICO != 0 };
            let (x, base) = (x + dx, base + dy);
            let largura = caneta.escrever(s, x, base, texto, *letra);
            if *letra == Letra::Mono && estilo & ESTILO_NEGRITO != 0 {
                caneta.escrever(s, x + 1, base, texto, *letra);
            }
            if link.is_some() || estilo & ESTILO_SUBLINHADO != 0 {
                let linha = Retangulo::novo(x, base + 2, largura, 1);
                s.retangulo(linha.intersecao(&recorte), cor);
            }
        }
        Item::Regua { x, y, largura } => {
            let r = Retangulo::novo(x + dx, y + dy, *largura, 1);
            s.retangulo(r.intersecao(&recorte), COR_REGUA);
        }
        Item::Bolinha { cx, cy, raio, cheia } => bolinha(s, recorte, cx + dx, cy + dy, *raio, *cheia),
        Item::FundoPre(r) => s.retangulo(r.deslocado(dx, dy).intersecao(&recorte), COR_FUNDO_PRE),
        Item::Barra(r) => s.retangulo(r.deslocado(dx, dy).intersecao(&recorte), COR_CITACAO),
        Item::FundoCelula(r) => s.retangulo(r.deslocado(dx, dy).intersecao(&recorte), COR_CABECALHO),
        Item::Grade(r) => s.retangulo(r.deslocado(dx, dy).intersecao(&recorte), COR_GRADE),
    }
}

/// Como o texto sai: cor, inclinacao e o retangulo onde pode pintar.
struct Caneta {
    recorte: Retangulo,
    cor: Cor,
    italico: bool,
}

impl Caneta {
    /// Escreve `texto` com a caneta em `x` e a linha de base em `base`.
    /// Devolve a largura (para o sublinhado).
    fn escrever(&self, s: &mut Superficie, x: i32, base: i32, texto: &str, letra: Letra) -> i32 {
        let f = letra.fonte();
        let passo = s.largura();
        let pixels = s.pixels_mut();
        let mut caneta = x;
        for c in texto.chars() {
            let glifo = f.glifo(c);
            // + 4: o italico pode passar um pouco do avanco
            let aparece = caneta < self.recorte.direita() && caneta + glifo.avanco + 4 > self.recorte.x;
            if aparece && glifo.largura > 0 {
                self.glifo(pixels, passo, caneta, base, &glifo);
            }
            caneta += glifo.avanco;
        }
        caneta - x
    }

    /// Um glifo, misturando o alfa de cada pixel com o que ja esta la.
    fn glifo(&self, pixels: &mut [u32], passo: usize, x: i32, base: i32, g: &Glifo) {
        let r = self.recorte;
        for gy in 0..g.altura {
            let y = base + g.dy + gy;
            if y < r.y || y >= r.baixo() {
                continue;
            }
            let inclinacao = if self.italico { (base - y).div_euclid(4) } else { 0 };
            let linha = y as usize * passo;
            for gx in 0..g.largura {
                let alfa = g.alfa(gx, gy);
                let px = x + g.dx + gx + inclinacao;
                if alfa == 0 || px < r.x || px >= r.direita() {
                    continue;
                }
                if let Some(p) = pixels.get_mut(linha + px as usize) {
                    *p = if alfa == 255 { self.cor } else { fonte::misturar(*p, self.cor, alfa) };
                }
            }
        }
    }
}

/// Preenche um retangulo ja recortado direto no Vec de pixels.
fn pintar(pixels: &mut [u32], passo: usize, r: Retangulo, cor: Cor) {
    if r.vazio() {
        return;
    }
    for linha in r.y..r.y + r.altura {
        let inicio = linha as usize * passo + r.x as usize;
        if let Some(faixa) = pixels.get_mut(inicio..inicio + r.largura as usize) {
            faixa.fill(cor);
        }
    }
}

/// Bolinha de lista: cheia ou so o contorno.
fn bolinha(s: &mut Superficie, recorte: Retangulo, cx: i32, cy: i32, raio: i32, cheia: bool) {
    let passo = s.largura();
    let pixels = s.pixels_mut();
    let r2 = raio * raio;
    for dy in -raio..=raio {
        for dx in -raio..=raio {
            let d = dx * dx + dy * dy;
            let dentro = d <= r2 + raio / 2;
            let borda = d > (raio - 1) * (raio - 1);
            if dentro && (cheia || borda) {
                let ponto = Retangulo::novo(cx + dx, cy + dy, 1, 1).intersecao(&recorte);
                pintar(pixels, passo, ponto, COR_MARCADOR);
            }
        }
    }
}
