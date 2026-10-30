//! Os ajudantes de cor e o botao chapado da area de trabalho do sistema
//! (gui/area.rs), que os widgets e os apps usam.

use super::grafico::{self, Cor, Retangulo, Superficie};
use super::widgets;
use crate::fonte;

/// Clareia cada canal de `cor` em `quanto` (satura em 255).
pub fn clarear(cor: Cor, quanto: u32) -> Cor {
    grafico::misturar(cor, 0x00FF_FFFF, quanto)
}

/// Escurece `cor` (quanto: 0 = igual, 255 = preto).
pub fn escurecer(cor: Cor, quanto: u32) -> Cor {
    grafico::misturar(cor, 0, quanto)
}

/// Puxa os pixels de `r` para `cor` (alfa 0 a 255).
pub fn tingir(s: &mut Superficie, r: Retangulo, cor: Cor, alfa: u32) {
    let r = r.intersecao(&s.area());
    let largura = s.largura();
    let pixels = s.pixels_mut();
    for y in r.y..r.baixo() {
        let linha = y as usize * largura;
        for p in &mut pixels[linha + r.x as usize..linha + r.direita() as usize] {
            *p = grafico::misturar(*p, cor, alfa);
        }
    }
}

/// O comeco de `texto` que cabe em `largura` pixels na fonte da `escala`.
pub fn cortar(texto: &str, largura: i32, escala: usize) -> &str {
    fonte::cortar(texto, largura, fonte::por_escala(escala))
}

/// Botao chapado com texto centralizado.
pub fn botao(s: &mut Superficie, r: Retangulo, texto: &str, fundo: Cor, frente: Cor, borda: Cor, escala: usize) {
    s.retangulo(r, fundo);
    s.contorno(r, borda, 1);
    let f = fonte::por_escala(escala);
    widgets::texto_centrado(s, r, cortar(texto, r.largura - 8, escala), frente, f);
}
