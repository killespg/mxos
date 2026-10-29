//! GIF (87a/89a): paletas global/local, transparencia (Graphic Control),
//! LZW de tamanho variavel e entrelacamento. So o primeiro quadro.

use alloc::vec;
use alloc::vec::Vec;

use super::Imagem;

fn paleta(d: &[u8], i: usize, n: usize) -> Option<Vec<u32>> {
    let t = d.get(i..i + 3 * n)?;
    Some(t.chunks(3).map(|c| 0xFF00_0000 | (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32).collect())
}

fn lzw(dados: &[u8], min: u8, saida: &mut Vec<u8>, limite: usize) {
    let min = min.clamp(2, 11) as u32;
    let limpar = 1u32 << min;
    let fim = limpar + 1;
    let mut prefixo = vec![0u16; 4096];
    let mut sufixo = vec![0u8; 4096];
    let mut tamanho = vec![0u16; 4096];
    for k in 0..limpar as usize {
        sufixo[k] = k as u8;
        tamanho[k] = 1;
    }
    let mut prox = fim + 1;
    let mut largura = min + 1;
    let mut anterior: Option<u32> = None;
    let mut buf = 0u32;
    let mut n = 0u32;
    let mut pilha: Vec<u8> = Vec::with_capacity(4096);
    for &b in dados {
        buf |= (b as u32) << n;
        n += 8;
        while n >= largura {
            let codigo = buf & ((1 << largura) - 1);
            buf >>= largura;
            n -= largura;
            if codigo == limpar {
                prox = fim + 1;
                largura = min + 1;
                anterior = None;
                continue;
            }
            if codigo == fim {
                return;
            }
            let atual = if codigo < prox {
                codigo
            } else if let Some(a) = anterior {
                // codigo ainda nao existe: anterior + primeira letra do anterior
                let _ = a;
                codigo
            } else {
                return;
            };
            // escreve a sequencia do codigo
            let escrever = |c: u32, pilha: &mut Vec<u8>| {
                pilha.clear();
                let mut x = c as usize;
                let mut guarda = 0;
                while x >= limpar as usize && guarda < 4096 {
                    pilha.push(sufixo[x]);
                    x = prefixo[x] as usize;
                    guarda += 1;
                }
                pilha.push(x as u8);
            };
            if atual < prox {
                escrever(atual, &mut pilha);
                let primeira = *pilha.last().unwrap_or(&0);
                saida.extend(pilha.iter().rev());
                if let Some(a) = anterior {
                    if prox < 4096 {
                        prefixo[prox as usize] = a as u16;
                        sufixo[prox as usize] = primeira;
                        tamanho[prox as usize] = tamanho[a as usize] + 1;
                        prox += 1;
                    }
                }
            } else if let Some(a) = anterior {
                escrever(a, &mut pilha);
                let primeira = *pilha.last().unwrap_or(&0);
                saida.extend(pilha.iter().rev());
                saida.push(primeira);
                if prox < 4096 {
                    prefixo[prox as usize] = a as u16;
                    sufixo[prox as usize] = primeira;
                    tamanho[prox as usize] = tamanho[a as usize] + 1;
                    prox += 1;
                }
            }
            anterior = Some(codigo);
            if prox == (1 << largura) && largura < 12 {
                largura += 1;
            }
            if saida.len() > limite {
                return;
            }
        }
    }
}

pub fn decodificar(d: &[u8]) -> Option<Imagem> {
    if d.len() < 13 || &d[..3] != b"GIF" {
        return None;
    }
    let l = u16::from_le_bytes([d[6], d[7]]) as usize;
    let a = u16::from_le_bytes([d[8], d[9]]) as usize;
    if l == 0 || a == 0 || l * a > 40_000_000 {
        return None;
    }
    let flags = d[10];
    let mut i = 13;
    let mut global = Vec::new();
    if flags & 0x80 != 0 {
        let n = 1 << ((flags & 7) + 1);
        global = paleta(d, i, n)?;
        i += 3 * n;
    }
    let mut transparente: Option<u8> = None;
    while i < d.len() {
        match d[i] {
            0x21 => {
                let rotulo = *d.get(i + 1)?;
                i += 2;
                if rotulo == 0xF9 && d.get(i).copied() == Some(4) {
                    let f = *d.get(i + 1)?;
                    if f & 1 != 0 {
                        transparente = Some(*d.get(i + 4)?);
                    }
                }
                // pula os sub-blocos
                while let Some(&n) = d.get(i) {
                    i += 1 + n as usize;
                    if n == 0 {
                        break;
                    }
                }
            }
            0x2C => {
                let x0 = u16::from_le_bytes([*d.get(i + 1)?, *d.get(i + 2)?]) as usize;
                let y0 = u16::from_le_bytes([*d.get(i + 3)?, *d.get(i + 4)?]) as usize;
                let w = u16::from_le_bytes([*d.get(i + 5)?, *d.get(i + 6)?]) as usize;
                let h = u16::from_le_bytes([*d.get(i + 7)?, *d.get(i + 8)?]) as usize;
                let f = *d.get(i + 9)?;
                i += 10;
                let mut tabela = global.clone();
                if f & 0x80 != 0 {
                    let n = 1 << ((f & 7) + 1);
                    tabela = paleta(d, i, n)?;
                    i += 3 * n;
                }
                let entrelacado = f & 0x40 != 0;
                let min = *d.get(i)?;
                i += 1;
                let mut dados = Vec::new();
                while let Some(&n) = d.get(i) {
                    i += 1;
                    if n == 0 {
                        break;
                    }
                    dados.extend_from_slice(d.get(i..i + n as usize)?);
                    i += n as usize;
                }
                let mut indices = Vec::with_capacity(w * h);
                lzw(&dados, min, &mut indices, w * h + 4096);
                let mut px = vec![0u32; l * a];
                // linhas na ordem do entrelacamento
                let ordem: Vec<usize> = if entrelacado {
                    let mut v = Vec::with_capacity(h);
                    for (ini, passo) in [(0, 8), (4, 8), (2, 4), (1, 2)] {
                        let mut y = ini;
                        while y < h {
                            v.push(y);
                            y += passo;
                        }
                    }
                    v
                } else {
                    (0..h).collect()
                };
                for (k, &yy) in ordem.iter().enumerate() {
                    for xx in 0..w {
                        let Some(&ix) = indices.get(k * w + xx) else { break };
                        let (x, y) = (x0 + xx, y0 + yy);
                        if x >= l || y >= a || Some(ix) == transparente {
                            continue;
                        }
                        px[y * l + x] = tabela.get(ix as usize).copied().unwrap_or(0xFF00_0000);
                    }
                }
                return Some(Imagem { largura: l as i32, altura: a as i32, px });
            }
            0x3B => break,
            _ => i += 1,
        }
    }
    None
}
