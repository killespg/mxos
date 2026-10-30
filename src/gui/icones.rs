//! Icones de app no estilo atual: um azulejo de cantos bem redondos com
//! degrade na cor do app e o simbolo em branco por cima, com uma sombra
//! curta embaixo. Desenhados por formas (distancia com sinal) e 4 x 4
//! amostras por pixel, em qualquer tamanho: nada de pixel art ampliada.
//!
//! Os de navegacao (voltar, recarregar...) continuam os de widgets::icone.

use super::grafico::{self, Cor, Superficie};
use super::widgets::Icone;
use crate::web::js::mat64;

/// Uma forma do simbolo, em coordenadas do azulejo (0..1).
#[derive(Clone, Copy)]
enum Forma {
    /// Retangulo arredondado: centro, meia largura, meia altura, raio.
    Caixa(f32, f32, f32, f32, f32),
    /// Disco: centro e raio.
    Disco(f32, f32, f32),
    /// Anel: centro, raio e espessura.
    Anel(f32, f32, f32, f32),
    /// Elipse vazada: centro, raios x e y, espessura.
    Elipse(f32, f32, f32, f32, f32),
    /// Segmento grosso de ponta redonda: a, b, espessura.
    Traco(f32, f32, f32, f32, f32),
}

/// Distancia com sinal (negativa = dentro) do ponto (x, y) a forma.
fn distancia(f: Forma, x: f32, y: f32) -> f32 {
    match f {
        Forma::Caixa(cx, cy, mx, my, r) => {
            let dx = (x - cx).abs() - (mx - r);
            let dy = (y - cy).abs() - (my - r);
            let fora = raiz(dx.max(0.0) * dx.max(0.0) + dy.max(0.0) * dy.max(0.0));
            fora + dx.max(dy).min(0.0) - r
        }
        Forma::Disco(cx, cy, r) => raiz((x - cx) * (x - cx) + (y - cy) * (y - cy)) - r,
        Forma::Anel(cx, cy, r, e) => (raiz((x - cx) * (x - cx) + (y - cy) * (y - cy)) - r).abs() - e / 2.0,
        Forma::Elipse(cx, cy, rx, ry, e) => {
            // aproximacao: distancia radial normalizada vezes o raio medio
            let (nx, ny) = ((x - cx) / rx, (y - cy) / ry);
            let k = raiz(nx * nx + ny * ny);
            ((k - 1.0) * rx.min(ry)).abs() - e / 2.0
        }
        Forma::Traco(ax, ay, bx, by, e) => {
            let (px, py, vx, vy) = (x - ax, y - ay, bx - ax, by - ay);
            let h = ((px * vx + py * vy) / (vx * vx + vy * vy)).clamp(0.0, 1.0);
            let (dx, dy) = (px - vx * h, py - vy * h);
            raiz(dx * dx + dy * dy) - e / 2.0
        }
    }
}

fn raiz(v: f32) -> f32 {
    mat64::sqrt(v as f64) as f32
}

/// Cores do degrade (cima, baixo) e o simbolo de cada icone. Formas em
/// `buracos` sao recortadas do simbolo (o azulejo aparece nelas).
fn desenho(tipo: Icone) -> (Cor, Cor, &'static [Forma], &'static [Forma]) {
    use Forma::*;
    match tipo {
        Icone::Pasta => (0x00FB_BF24, 0x00F5_9E0B, &[Caixa(0.36, 0.33, 0.14, 0.06, 0.04), Caixa(0.5, 0.54, 0.29, 0.20, 0.05)], &[]),
        Icone::Navegador | Icone::Rede => (0x0060_A5FA, 0x0025_63EB, &[Anel(0.5, 0.5, 0.25, 0.055), Elipse(0.5, 0.5, 0.11, 0.25, 0.05), Traco(0.26, 0.5, 0.74, 0.5, 0.05), Traco(0.30, 0.38, 0.70, 0.38, 0.04), Traco(0.30, 0.62, 0.70, 0.62, 0.04)], &[]),
        Icone::Terminal => (0x0047_5569, 0x001E_293B, &[Traco(0.30, 0.38, 0.42, 0.50, 0.065), Traco(0.42, 0.50, 0.30, 0.62, 0.065), Traco(0.49, 0.63, 0.70, 0.63, 0.065)], &[]),
        Icone::Tarefas => (0x004A_DE80, 0x0016_A34A, &[Traco(0.20, 0.54, 0.36, 0.54, 0.06), Traco(0.36, 0.54, 0.44, 0.32, 0.06), Traco(0.44, 0.32, 0.55, 0.72, 0.06), Traco(0.55, 0.72, 0.63, 0.48, 0.06), Traco(0.63, 0.48, 0.80, 0.48, 0.06)], &[]),
        Icone::Engrenagem => (
            0x0094_A3B8,
            0x0047_5569,
            &[
                Disco(0.5, 0.5, 0.22),
                Traco(0.5, 0.21, 0.5, 0.79, 0.13),
                Traco(0.21, 0.5, 0.79, 0.5, 0.13),
                Traco(0.30, 0.30, 0.70, 0.70, 0.13),
                Traco(0.70, 0.30, 0.30, 0.70, 0.13),
            ],
            &[Disco(0.5, 0.5, 0.085)],
        ),
        Icone::Disco => (0x0094_A3B8, 0x0047_5569, &[Caixa(0.5, 0.5, 0.26, 0.18, 0.05)], &[Disco(0.66, 0.5, 0.035), Traco(0.32, 0.5, 0.52, 0.5, 0.04)]),
        Icone::Chip => (0x00A7_8BFA, 0x006D_28D9, &[Caixa(0.5, 0.5, 0.18, 0.18, 0.04), Traco(0.40, 0.22, 0.40, 0.78, 0.04), Traco(0.60, 0.22, 0.60, 0.78, 0.04), Traco(0.22, 0.40, 0.78, 0.40, 0.04), Traco(0.22, 0.60, 0.78, 0.60, 0.04)], &[Caixa(0.5, 0.5, 0.09, 0.09, 0.02)]),
        Icone::Pincel => (0x00F4_72B6, 0x00DB_2777, &[Traco(0.66, 0.30, 0.42, 0.56, 0.08), Disco(0.36, 0.64, 0.10)], &[]),
        Icone::Calculadora => (0x00FB_923C, 0x00EA_580C, &[Caixa(0.5, 0.5, 0.22, 0.28, 0.05)], &[Caixa(0.5, 0.34, 0.15, 0.06, 0.02), Disco(0.40, 0.52, 0.035), Disco(0.60, 0.52, 0.035), Disco(0.40, 0.66, 0.035), Disco(0.60, 0.66, 0.035)]),
        Icone::Info => (0x0060_A5FA, 0x0025_63EB, &[Disco(0.5, 0.33, 0.055), Traco(0.5, 0.46, 0.5, 0.70, 0.08)], &[]),
        Icone::Teclado => (0x0094_A3B8, 0x0047_5569, &[Caixa(0.5, 0.52, 0.30, 0.18, 0.04)], &[Disco(0.34, 0.46, 0.03), Disco(0.46, 0.46, 0.03), Disco(0.58, 0.46, 0.03), Disco(0.70, 0.46, 0.03), Traco(0.36, 0.60, 0.64, 0.60, 0.04)]),
        Icone::Monitor => (0x0060_A5FA, 0x0025_63EB, &[Caixa(0.5, 0.44, 0.28, 0.19, 0.04), Traco(0.5, 0.63, 0.5, 0.72, 0.06), Traco(0.38, 0.73, 0.62, 0.73, 0.06)], &[Caixa(0.5, 0.44, 0.22, 0.13, 0.02)]),
        Icone::Som => (0x00F4_72B6, 0x00C0_2685, &[Caixa(0.36, 0.5, 0.07, 0.10, 0.02), Traco(0.38, 0.5, 0.52, 0.34, 0.08), Traco(0.38, 0.5, 0.52, 0.66, 0.08), Traco(0.52, 0.34, 0.52, 0.66, 0.06), Anel(0.52, 0.5, 0.20, 0.045)], &[Caixa(0.44, 0.5, 0.06, 0.40, 0.0)]),
        Icone::Arquivo | Icone::Texto | Icone::Html => (
            0x0094_A3B8,
            0x0047_5569,
            &[Caixa(0.5, 0.5, 0.20, 0.27, 0.04)],
            &[Traco(0.40, 0.40, 0.60, 0.40, 0.04), Traco(0.40, 0.50, 0.60, 0.50, 0.04), Traco(0.40, 0.60, 0.54, 0.60, 0.04)],
        ),
        // navegacao: nao tem azulejo (ficam os de widgets::icone)
        _ => (0x0094_A3B8, 0x0047_5569, &[], &[]),
    }
}

/// O icone `tipo` em azulejo de `lado` pixels com o canto em (x, y), sobre
/// o que ja esta em `s`.
pub fn icone_app(s: &mut Superficie, x: i32, y: i32, tipo: Icone, lado: i32) {
    let (cima, baixo, simbolo, buracos) = desenho(tipo);
    let l = lado as f32;
    // azulejo: retangulo arredondado com raio de 23% (o "squircle" atual)
    let azulejo = Forma::Caixa(0.5, 0.5, 0.5, 0.5, 0.23);
    // sombra: o azulejo descido 3% e borrado ~6%
    let sombra = Forma::Caixa(0.5, 0.54, 0.5, 0.5, 0.23);
    let px = 1.0 / l;
    for py in (y - 2)..(y + lado + 4) {
        for qx in (x - 2)..(x + lado + 2) {
            let Some(fundo) = s.ler_pixel(qx, py) else { continue };
            let (mut cobre_az, mut cobre_sim) = (0u32, 0u32);
            for sy in 0..4 {
                for sx in 0..4 {
                    let u = ((qx - x) as f32 + (sx as f32 + 0.5) / 4.0) * px;
                    let v = ((py - y) as f32 + (sy as f32 + 0.5) / 4.0) * px;
                    if distancia(azulejo, u, v) <= 0.0 {
                        cobre_az += 1;
                        let no_simbolo = simbolo.iter().any(|&f| distancia(f, u, v) <= 0.0);
                        let no_buraco = buracos.iter().any(|&f| distancia(f, u, v) <= 0.0);
                        if no_simbolo && !no_buraco {
                            cobre_sim += 1;
                        }
                    }
                }
            }
            // a sombra fica embaixo de tudo, mais fraca longe do azulejo
            let (u, v) = (((qx - x) as f32 + 0.5) * px, ((py - y) as f32 + 0.5) * px);
            let ds = distancia(sombra, u, v);
            let alfa_sombra = if ds <= 0.0 { 90.0 } else { (90.0 * (1.0 - ds / 0.07)).max(0.0) };
            let mut cor = grafico::misturar(fundo, 0, alfa_sombra as u32);
            if cobre_az > 0 {
                let t = ((py - y) as f32 / l).clamp(0.0, 1.0);
                let tile = grafico::misturar(cima, baixo, (t * 255.0) as u32);
                cor = grafico::misturar(cor, tile, cobre_az * 255 / 16);
                if cobre_sim > 0 {
                    cor = grafico::misturar(cor, 0x00FF_FFFF, cobre_sim * 255 / 16);
                }
            }
            s.pixel(qx, py, cor);
        }
    }
}
