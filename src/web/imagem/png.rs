//! PNG: todos os tipos de cor (cinza, RGB, paleta, com alfa), profundidades
//! de 1 a 16 bits, os 5 filtros, entrelacamento Adam7 e tRNS. Saida em
//! 0xAARRGGBB (sem pre-multiplicar).

use alloc::vec;
use alloc::vec::Vec;

use super::inflate;
use super::Imagem;

fn u32be(d: &[u8], i: usize) -> u32 {
    u32::from_be_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]])
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let pa = (p - a as i16).abs();
    let pb = (p - b as i16).abs();
    let pc = (p - c as i16).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

fn desfiltrar(dados: &mut [u8], linhas: usize, bytes_linha: usize, bpp: usize) -> Option<()> {
    let passo = bytes_linha + 1;
    if dados.len() < linhas * passo {
        return None;
    }
    for y in 0..linhas {
        let filtro = dados[y * passo];
        let (antes, depois) = dados.split_at_mut(y * passo + 1);
        let atual = &mut depois[..bytes_linha];
        let anterior: &[u8] = if y > 0 { &antes[(y - 1) * passo + 1..(y - 1) * passo + 1 + bytes_linha] } else { &[] };
        let cima = |i: usize| if y > 0 { anterior[i] } else { 0 };
        match filtro {
            0 => {}
            1 => {
                for i in bpp..bytes_linha {
                    atual[i] = atual[i].wrapping_add(atual[i - bpp]);
                }
            }
            2 => {
                if y > 0 {
                    for i in 0..bytes_linha {
                        atual[i] = atual[i].wrapping_add(anterior[i]);
                    }
                }
            }
            3 => {
                for i in 0..bytes_linha {
                    let a = if i >= bpp { atual[i - bpp] as u16 } else { 0 };
                    atual[i] = atual[i].wrapping_add(((a + cima(i) as u16) / 2) as u8);
                }
            }
            4 => {
                for i in 0..bytes_linha {
                    let a = if i >= bpp { atual[i - bpp] } else { 0 };
                    let c = if i >= bpp && y > 0 { anterior[i - bpp] } else { 0 };
                    atual[i] = atual[i].wrapping_add(paeth(a, cima(i), c));
                }
            }
            _ => return None,
        }
    }
    Some(())
}

pub fn decodificar(d: &[u8]) -> Option<Imagem> {
    if d.len() < 33 || &d[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let mut i = 8;
    let (mut l, mut a, mut prof, mut tipo, mut entrelacado) = (0usize, 0usize, 0u8, 0u8, false);
    let mut paleta: Vec<u32> = Vec::new();
    let mut trns: Vec<u8> = Vec::new();
    let mut idat: Vec<u8> = Vec::new();
    while i + 8 <= d.len() {
        let n = u32be(d, i) as usize;
        let tag = &d[i + 4..i + 8];
        let corpo = d.get(i + 8..i + 8 + n)?;
        match tag {
            b"IHDR" => {
                l = u32be(corpo, 0) as usize;
                a = u32be(corpo, 4) as usize;
                prof = corpo[8];
                tipo = corpo[9];
                entrelacado = corpo[12] == 1;
            }
            b"PLTE" => {
                paleta = corpo.chunks(3).map(|c| 0xFF00_0000 | (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32).collect();
            }
            b"tRNS" => trns = corpo.to_vec(),
            b"IDAT" => idat.extend_from_slice(corpo),
            b"IEND" => break,
            _ => {}
        }
        i += 12 + n;
    }
    if l == 0 || a == 0 || l > 16384 || a > 16384 || l * a > 40_000_000 {
        return None;
    }
    let canais = match tipo {
        0 => 1,
        2 => 3,
        3 => 1,
        4 => 2,
        6 => 4,
        _ => return None,
    };
    let bits_px = canais * prof as usize;
    let bpp = bits_px.div_ceil(8).max(1);
    let bytes_linha = |w: usize| (w * bits_px).div_ceil(8);
    // tamanho maximo descomprimido
    let maximo = (bytes_linha(l) + 1) * a + a * 8 + 1024;
    let mut cru = inflate::zlib(&idat, maximo)?;
    let mut px = vec![0u32; l * a];
    let maximo_amostra = ((1u32 << prof.min(16)) - 1).max(1);
    let trns_cinza = if trns.len() >= 2 { Some(u16::from_be_bytes([trns[0], trns[1]]) as u32) } else { None };
    let trns_rgb = if trns.len() >= 6 {
        Some((u16::from_be_bytes([trns[0], trns[1]]) as u32, u16::from_be_bytes([trns[2], trns[3]]) as u32, u16::from_be_bytes([trns[4], trns[5]]) as u32))
    } else {
        None
    };
    let mut converter = |linha: &[u8], w: usize, destino: &mut dyn FnMut(usize, u32)| {
        let amostra = |idx: usize| -> u32 {
            // idx = numero da amostra na linha
            match prof {
                8 => linha[idx] as u32,
                16 => u16::from_be_bytes([linha[2 * idx], linha[2 * idx + 1]]) as u32,
                p => {
                    let bit = idx * p as usize;
                    let byte = linha[bit / 8];
                    ((byte >> (8 - p as usize - bit % 8)) & ((1 << p) - 1)) as u32
                }
            }
        };
        let para8 = |v: u32| -> u32 { if prof == 8 { v } else { v * 255 / maximo_amostra } };
        for x in 0..w {
            let cor = match tipo {
                0 => {
                    let v = amostra(x);
                    let g = para8(v);
                    let alfa = if trns_cinza == Some(v) { 0 } else { 255 };
                    (alfa << 24) | g << 16 | g << 8 | g
                }
                2 => {
                    let (r, g, b) = (amostra(3 * x), amostra(3 * x + 1), amostra(3 * x + 2));
                    let alfa = if trns_rgb == Some((r, g, b)) { 0 } else { 255 };
                    (alfa << 24) | para8(r) << 16 | para8(g) << 8 | para8(b)
                }
                3 => {
                    let k = amostra(x) as usize;
                    let c = paleta.get(k).copied().unwrap_or(0xFF00_0000);
                    let alfa = trns.get(k).copied().unwrap_or(255) as u32;
                    (c & 0xFF_FFFF) | (alfa << 24)
                }
                4 => {
                    let g = para8(amostra(2 * x));
                    let al = para8(amostra(2 * x + 1));
                    (al << 24) | g << 16 | g << 8 | g
                }
                _ => {
                    let (r, g, b, al) = (amostra(4 * x), amostra(4 * x + 1), amostra(4 * x + 2), amostra(4 * x + 3));
                    para8(al) << 24 | para8(r) << 16 | para8(g) << 8 | para8(b)
                }
            };
            destino(x, cor);
        }
    };
    if !entrelacado {
        let bl = bytes_linha(l);
        desfiltrar(&mut cru, a, bl, bpp)?;
        for y in 0..a {
            let linha = &cru[y * (bl + 1) + 1..(y + 1) * (bl + 1)];
            let base = y * l;
            converter(linha, l, &mut |x, c| px[base + x] = c);
        }
    } else {
        // Adam7: 7 passadas, cada uma uma imagem menor
        const P: [(usize, usize, usize, usize); 7] = [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)];
        let mut pos = 0;
        for (x0, y0, dx, dy) in P {
            let w = (l + dx - 1 - x0) / dx;
            let h = (a + dy - 1 - y0) / dy;
            if w == 0 || h == 0 {
                continue;
            }
            let bl = bytes_linha(w);
            let tamanho = h * (bl + 1);
            let pedaco = cru.get_mut(pos..pos + tamanho)?;
            desfiltrar(pedaco, h, bl, bpp)?;
            let pedaco = pedaco.to_vec();
            for yy in 0..h {
                let linha = &pedaco[yy * (bl + 1) + 1..(yy + 1) * (bl + 1)];
                let y = y0 + yy * dy;
                converter(linha, w, &mut |xx, c| {
                    let x = x0 + xx * dx;
                    px[y * l + x] = c;
                });
            }
            pos += tamanho;
        }
    }
    Some(Imagem { largura: l as i32, altura: a as i32, px })
}
