//! Ponteiro do mouse: a seta classica, preta por fora e branca por dentro.
//!
//! O desenho e um bitmap de texto: 'P' = preto (borda), 'B' = branco
//! (miolo), ' ' = transparente. A ponta da seta e o pixel (0, 0).
//! Quem desenha guarda o que estava embaixo de LARGURA x ALTURA pixels a
//! partir de (x, y) para poder apagar o cursor depois.

use super::grafico::{Cor, Superficie};

pub const LARGURA: i32 = 12;
pub const ALTURA: i32 = 19;

const BORDA: Cor = 0x0010_1010;
const MIOLO: Cor = 0x00FF_FFFF;

const SETA: [&[u8; LARGURA as usize]; ALTURA as usize] = [
    b"P           ",
    b"PP          ",
    b"PBP         ",
    b"PBBP        ",
    b"PBBBP       ",
    b"PBBBBP      ",
    b"PBBBBBP     ",
    b"PBBBBBBP    ",
    b"PBBBBBBBP   ",
    b"PBBBBBBBBP  ",
    b"PBBBBBBBBBP ",
    b"PBBBBBBPPPPP",
    b"PBBBPBBP    ",
    b"PBBPPBBP    ",
    b"PBP  PBBP   ",
    b"PP   PBBP   ",
    b"P     PBBP  ",
    b"      PBBP  ",
    b"       PP   ",
];

/// Desenha a seta na tela com a ponta em (x, y): o que estava embaixo vem
/// de `cena`, a seta vai por cima num pedaco pequeno e so ele sai.
pub fn desenhar_na_tela(cena: &Superficie, tela: &crate::video::tela::Tela, x: i32, y: i32) {
    let mut pedaco = Superficie::nova(LARGURA as usize, ALTURA as usize, 0);
    cena.copiar_regiao_para(super::grafico::Retangulo::novo(x, y, LARGURA, ALTURA), &mut pedaco, 0, 0);
    desenhar(&mut pedaco, 0, 0);
    super::grafico::apresentar_em(&pedaco, tela, x, y);
}

/// Desenha a seta com a ponta em (x, y). O que sai da superficie e cortado.
pub fn desenhar(s: &mut Superficie, x: i32, y: i32) {
    for (dy, linha) in SETA.iter().enumerate() {
        for (dx, &ponto) in linha.iter().enumerate() {
            let cor = match ponto {
                b'P' => BORDA,
                b'B' => MIOLO,
                _ => continue,
            };
            s.pixel(x + dx as i32, y + dy as i32, cor);
        }
    }
}
