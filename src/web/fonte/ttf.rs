//! Leitor de fontes TrueType/OpenType (contornos 'glyf'): tabelas head,
//! hhea, hmtx, maxp, cmap (formatos 4 e 12), loca, glyf (simples e
//! compostos), kern (formato 0) e OS/2. Tudo lido com from_be_bytes sobre
//! fatias (nada de ponteiro desalinhado no ARMv7).

use alloc::vec::Vec;

fn u16_em(d: &[u8], o: usize) -> u16 {
    match d.get(o..o + 2) {
        Some(b) => u16::from_be_bytes([b[0], b[1]]),
        None => 0,
    }
}

fn i16_em(d: &[u8], o: usize) -> i16 {
    u16_em(d, o) as i16
}

fn u32_em(d: &[u8], o: usize) -> u32 {
    match d.get(o..o + 4) {
        Some(b) => u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
        None => 0,
    }
}

/// Um ponto do contorno (unidades da fonte) e se esta na curva.
#[derive(Clone, Copy, Debug)]
pub struct Ponto {
    pub x: f32,
    pub y: f32,
    pub na_curva: bool,
}

/// Contornos de um glifo: cada um e uma lista fechada de pontos.
pub type Contornos = Vec<Vec<Ponto>>;

pub struct Fonte {
    dados: Vec<u8>,
    pub unidades: u16,
    pub ascendente: i16,
    pub descendente: i16,
    pub entrelinha: i16,
    pub altura_x: i16,
    num_metricas: u16,
    num_glifos: u16,
    loca_longo: bool,
    hmtx: usize,
    loca: usize,
    glyf: usize,
    cmap: usize,
    cmap_formato: u16,
    kern: usize,
    kern_pares: usize,
}

impl Fonte {
    pub fn ler(dados: Vec<u8>) -> Option<Fonte> {
        let d = &dados;
        let versao = u32_em(d, 0);
        if versao != 0x0001_0000 && versao != 0x7472_7565 {
            return None; // so contornos TrueType (OTF com CFF fica de fora)
        }
        let n = u16_em(d, 4) as usize;
        let tabela = |tag: &[u8; 4]| -> Option<usize> {
            for k in 0..n {
                let r = 12 + 16 * k;
                if d.get(r..r + 4)? == tag {
                    return Some(u32_em(d, r + 8) as usize);
                }
            }
            None
        };
        let head = tabela(b"head")?;
        let hhea = tabela(b"hhea")?;
        let maxp = tabela(b"maxp")?;
        let hmtx = tabela(b"hmtx")?;
        let loca = tabela(b"loca")?;
        let glyf = tabela(b"glyf")?;
        let cmap = tabela(b"cmap")?;
        let os2 = tabela(b"OS/2");
        let kern = tabela(b"kern").unwrap_or(0);

        // melhor subtabela do cmap: (3,10) formato 12, senao (3,1)/(0,x) formato 4
        let num = u16_em(d, cmap + 2) as usize;
        let mut escolhida = 0;
        let mut formato = 0;
        for k in 0..num {
            let r = cmap + 4 + 8 * k;
            let plataforma = u16_em(d, r);
            let codificacao = u16_em(d, r + 2);
            let off = cmap + u32_em(d, r + 4) as usize;
            let f = u16_em(d, off);
            let boa = match (plataforma, codificacao, f) {
                (3, 10, 12) | (0, 4, 12) | (0, 6, 12) => 2,
                (3, 1, 4) | (0, _, 4) => 1,
                _ => 0,
            };
            if boa > formato_peso(formato) {
                escolhida = off;
                formato = f;
            }
        }
        if escolhida == 0 {
            return None;
        }
        let (altura_x, _) = match os2 {
            Some(o) if u16_em(d, o) >= 2 => (i16_em(d, o + 86), i16_em(d, o + 88)),
            _ => (0, 0),
        };
        let mut kern_pares = 0;
        let mut kern_off = 0;
        if kern != 0 && u16_em(d, kern) == 0 && u16_em(d, kern + 2) > 0 {
            // primeira subtabela, formato 0 horizontal
            let sub = kern + 4;
            if u16_em(d, sub + 4) & 0xFF00 == 0 && u16_em(d, sub + 4) & 1 == 1 {
                kern_pares = u16_em(d, sub + 6) as usize;
                kern_off = sub + 14;
            }
        }
        let unidades = u16_em(d, head + 18).max(16);
        Some(Fonte {
            unidades,
            ascendente: i16_em(d, hhea + 4),
            descendente: i16_em(d, hhea + 6),
            entrelinha: i16_em(d, hhea + 8),
            altura_x: if altura_x > 0 { altura_x } else { (unidades as i32 / 2) as i16 },
            num_metricas: u16_em(d, hhea + 34).max(1),
            num_glifos: u16_em(d, maxp + 4),
            loca_longo: i16_em(d, head + 50) != 0,
            hmtx,
            loca,
            glyf,
            cmap: escolhida,
            cmap_formato: formato,
            kern: kern_off,
            kern_pares,
            dados,
        })
    }

    /// Glifo de um caractere (0 = .notdef, nao tem).
    pub fn glifo(&self, c: char) -> u16 {
        let d = &self.dados;
        let c = c as u32;
        let t = self.cmap;
        match self.cmap_formato {
            4 => {
                if c > 0xFFFF {
                    return 0;
                }
                let segs = u16_em(d, t + 6) as usize / 2;
                let fins = t + 14;
                let inicios = fins + 2 * segs + 2;
                let deltas = inicios + 2 * segs;
                let offsets = deltas + 2 * segs;
                // busca binaria no primeiro fim >= c
                let (mut lo, mut hi) = (0usize, segs);
                while lo < hi {
                    let meio = (lo + hi) / 2;
                    if (u16_em(d, fins + 2 * meio) as u32) < c {
                        lo = meio + 1;
                    } else {
                        hi = meio;
                    }
                }
                if lo >= segs {
                    return 0;
                }
                let inicio = u16_em(d, inicios + 2 * lo) as u32;
                if c < inicio {
                    return 0;
                }
                let delta = u16_em(d, deltas + 2 * lo);
                let off = u16_em(d, offsets + 2 * lo) as usize;
                if off == 0 {
                    return (c as u16).wrapping_add(delta);
                }
                let g = u16_em(d, offsets + 2 * lo + off + 2 * (c - inicio) as usize);
                if g == 0 { 0 } else { g.wrapping_add(delta) }
            }
            12 => {
                let grupos = u32_em(d, t + 12) as usize;
                let (mut lo, mut hi) = (0usize, grupos);
                while lo < hi {
                    let meio = (lo + hi) / 2;
                    let g = t + 16 + 12 * meio;
                    let ini = u32_em(d, g);
                    let fim = u32_em(d, g + 4);
                    if c < ini {
                        hi = meio;
                    } else if c > fim {
                        lo = meio + 1;
                    } else {
                        return (u32_em(d, g + 8) + (c - ini)) as u16;
                    }
                }
                0
            }
            _ => 0,
        }
    }

    /// Avanco horizontal (unidades da fonte).
    pub fn avanco(&self, g: u16) -> u16 {
        let k = (g.min(self.num_metricas - 1)) as usize;
        u16_em(&self.dados, self.hmtx + 4 * k)
    }

    /// Ajuste de par (kerning) entre dois glifos, em unidades.
    pub fn kerning(&self, a: u16, b: u16) -> i16 {
        if self.kern_pares == 0 {
            return 0;
        }
        let alvo = ((a as u32) << 16) | b as u32;
        let (mut lo, mut hi) = (0usize, self.kern_pares);
        while lo < hi {
            let meio = (lo + hi) / 2;
            let r = self.kern + 6 * meio;
            let par = u32_em(&self.dados, r);
            if par < alvo {
                lo = meio + 1;
            } else if par > alvo {
                hi = meio;
            } else {
                return i16_em(&self.dados, r + 4);
            }
        }
        0
    }

    fn faixa_glifo(&self, g: u16) -> Option<(usize, usize)> {
        if g >= self.num_glifos {
            return None;
        }
        let d = &self.dados;
        let (a, b) = if self.loca_longo {
            (u32_em(d, self.loca + 4 * g as usize) as usize, u32_em(d, self.loca + 4 * g as usize + 4) as usize)
        } else {
            (2 * u16_em(d, self.loca + 2 * g as usize) as usize, 2 * u16_em(d, self.loca + 2 * g as usize + 2) as usize)
        };
        if b <= a {
            return None; // glifo vazio (espaco)
        }
        Some((self.glyf + a, self.glyf + b))
    }

    /// Os contornos de um glifo (com os compostos ja montados).
    pub fn contornos(&self, g: u16) -> Contornos {
        let mut v = Vec::new();
        self.montar(g, &mut v, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], 0);
        v
    }

    fn montar(&self, g: u16, saida: &mut Contornos, m: [f32; 6], prof: u32) {
        if prof > 8 {
            return;
        }
        let Some((ini, fim)) = self.faixa_glifo(g) else { return };
        let d = &self.dados[..fim.min(self.dados.len())];
        let ncont = i16_em(d, ini);
        if ncont >= 0 {
            self.simples(d, ini, ncont as usize, saida, m);
            return;
        }
        // composto
        let mut p = ini + 10;
        loop {
            let flags = u16_em(d, p);
            let filho = u16_em(d, p + 2);
            p += 4;
            let (dx, dy) = if flags & 1 != 0 {
                let r = (i16_em(d, p) as f32, i16_em(d, p + 2) as f32);
                p += 4;
                r
            } else {
                let r = (d.get(p).copied().unwrap_or(0) as i8 as f32, d.get(p + 1).copied().unwrap_or(0) as i8 as f32);
                p += 2;
                r
            };
            let f2 = |o: usize| i16_em(d, o) as f32 / 16384.0;
            let (a, b, c, e) = if flags & 0x08 != 0 {
                let s = f2(p);
                p += 2;
                (s, 0.0, 0.0, s)
            } else if flags & 0x40 != 0 {
                let r = (f2(p), 0.0, 0.0, f2(p + 2));
                p += 4;
                r
            } else if flags & 0x80 != 0 {
                let r = (f2(p), f2(p + 2), f2(p + 4), f2(p + 6));
                p += 8;
                r
            } else {
                (1.0, 0.0, 0.0, 1.0)
            };
            // compoe com a matriz de fora
            let mm = [
                m[0] * a + m[2] * b,
                m[1] * a + m[3] * b,
                m[0] * c + m[2] * e,
                m[1] * c + m[3] * e,
                m[0] * dx + m[2] * dy + m[4],
                m[1] * dx + m[3] * dy + m[5],
            ];
            if flags & 2 != 0 {
                self.montar(filho, saida, mm, prof + 1);
            }
            if flags & 0x20 == 0 || p >= d.len() {
                break;
            }
        }
    }

    fn simples(&self, d: &[u8], ini: usize, ncont: usize, saida: &mut Contornos, m: [f32; 6]) {
        let fins: Vec<usize> = (0..ncont).map(|k| u16_em(d, ini + 10 + 2 * k) as usize).collect();
        let npontos = fins.last().map_or(0, |&f| f + 1);
        if npontos == 0 || npontos > 10000 {
            return;
        }
        let ninstr = u16_em(d, ini + 10 + 2 * ncont) as usize;
        let mut p = ini + 12 + 2 * ncont + ninstr;
        let mut flags = Vec::with_capacity(npontos);
        while flags.len() < npontos {
            let f = d.get(p).copied().unwrap_or(0);
            p += 1;
            flags.push(f);
            if f & 8 != 0 {
                let rep = d.get(p).copied().unwrap_or(0);
                p += 1;
                for _ in 0..rep {
                    if flags.len() < npontos {
                        flags.push(f);
                    }
                }
            }
        }
        let mut xs = Vec::with_capacity(npontos);
        let mut v = 0i32;
        for &f in &flags {
            if f & 2 != 0 {
                let b = d.get(p).copied().unwrap_or(0) as i32;
                p += 1;
                v += if f & 16 != 0 { b } else { -b };
            } else if f & 16 == 0 {
                v += i16_em(d, p) as i32;
                p += 2;
            }
            xs.push(v);
        }
        let mut ys = Vec::with_capacity(npontos);
        v = 0;
        for &f in &flags {
            if f & 4 != 0 {
                let b = d.get(p).copied().unwrap_or(0) as i32;
                p += 1;
                v += if f & 32 != 0 { b } else { -b };
            } else if f & 32 == 0 {
                v += i16_em(d, p) as i32;
                p += 2;
            }
            ys.push(v);
        }
        let mut inicio = 0;
        for &fim in &fins {
            if fim < inicio || fim >= npontos {
                break;
            }
            let c: Vec<Ponto> = (inicio..=fim)
                .map(|k| {
                    let (x, y) = (xs[k] as f32, ys[k] as f32);
                    Ponto { x: m[0] * x + m[2] * y + m[4], y: m[1] * x + m[3] * y + m[5], na_curva: flags[k] & 1 != 0 }
                })
                .collect();
            saida.push(c);
            inicio = fim + 1;
        }
    }
}

fn formato_peso(f: u16) -> u8 {
    match f {
        12 => 2,
        4 => 1,
        _ => 0,
    }
}
