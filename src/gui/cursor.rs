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
