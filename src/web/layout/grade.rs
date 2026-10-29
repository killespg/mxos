//! CSS Grid: faixas explicitas (px, %, fr, auto, min/max-content, minmax,
//! repeat com auto-fill/auto-fit), areas nomeadas, linhas numeradas,
//! spans, colocacao automatica (por linha ou coluna, densa), faixas
//! implicitas, lacunas e alinhamento dos itens.

use alloc::vec::Vec;

use super::Motor;
use crate::web::css::estilo::{Alinhar, Faixa, LinhaGrade};
use crate::web::css::valores::Fixo;

#[derive(Clone, Copy)]
struct Lugar {
    k: u32,
    lin: usize,
    col: usize,
    nl: usize,
    nc: usize,
}

/// Tamanho minimo/maximo de uma faixa (None = depende do conteudo).
fn limites(f: &Faixa, base: Option<Fixo>) -> (Option<Fixo>, Option<Fixo>, f32) {
    match f {
        Faixa::M(m) => {
            let v = if m.tem_pct() { base.map(|b| m.resolver(b)) } else { Some(m.px) };
            (v, v, 0.0)
        }
        Faixa::Fr(fr) => (Some(0), None, *fr),
        Faixa::MinMax(a, b) => {
            let (mn, _, _) = limites(a, base);
            let (_, mx, fr) = limites(b, base);
            let fr = if let Faixa::Fr(v) = **b { v } else { fr };
            (mn, mx, fr)
        }
        _ => (None, None, 0.0),
    }
}

/// Expande repeat(auto-fill/auto-fit) para o espaco disponivel.
fn expandir(lista: &[Faixa], disponivel: Option<Fixo>, gap: Fixo) -> Vec<Faixa> {
    let mut v = Vec::new();
    for f in lista {
        match f {
            Faixa::Repetir(_, faixas) => {
                let fixo: Fixo = faixas
                    .iter()
                    .map(|x| {
                        let (mn, mx, _) = limites(x, disponivel);
                        mn.or(mx).unwrap_or(0).max(1)
                    })
                    .sum();
                let n = match disponivel {
                    Some(d) if fixo > 0 => ((d + gap) / (fixo + gap * faixas.len() as Fixo)).max(1) as usize,
                    _ => 1,
                };
                for _ in 0..n.min(200) {
                    v.extend(faixas.iter().cloned());
                }
            }
            f => v.push(f.clone()),
        }
    }
    v
}

impl Motor<'_, '_> {
    pub fn grade(&mut self, k: u32, x: Fixo, y: Fixo, l: Fixo, cb_a: Option<Fixo>) -> Fixo {
        let e = self.arv.caixas[k as usize].estilo.clone();
        let gap_c = e.lacuna.1.resolver(l);
        let gap_l = e.lacuna.0.resolver(cb_a.unwrap_or(0));
        let mut colunas = expandir(&e.colunas_grade, Some(l), gap_c);
        let mut linhas_def = expandir(&e.linhas_grade, cb_a, gap_l);
        let areas = e.areas_grade.clone();
        if colunas.is_empty() {
            let n = areas.first().map_or(1, |l| l.len().max(1));
            for _ in 0..n {
                colunas.push(e.auto_colunas.first().cloned().unwrap_or(Faixa::Auto));
            }
        }
        if linhas_def.len() < areas.len() {
            linhas_def.resize(areas.len(), Faixa::Auto);
        }
        let ncol = colunas.len();

        // itens
        let mut filhos: Vec<u32> = Vec::new();
        for f in self.arv.caixas[k as usize].filhos.clone() {
            if self.estilo(f).fora_do_fluxo() {
                self.absolutos_estatico(f, x, y);
                continue;
            }
            filhos.push(f);
        }
        filhos.sort_by_key(|&f| self.estilo(f).ordem);

        // colocacao
        let mut ocupado: Vec<Vec<bool>> = Vec::new();
        let garantir = |o: &mut Vec<Vec<bool>>, n: usize| {
            while o.len() < n {
                o.push(alloc::vec![false; ncol]);
            }
        };
        let resolver = |linha: LinhaGrade, fim: LinhaGrade, total: usize| -> (Option<usize>, usize) {
            // (inicio 0-based, span)
            let num = |n: i32| -> usize {
                if n > 0 { (n - 1) as usize } else if n < 0 { (total as i32 + 1 + n).max(0) as usize } else { 0 }
            };
            match (linha, fim) {
                (LinhaGrade::Num(a), LinhaGrade::Num(b)) => {
                    let (a, b) = (num(a), num(b));
                    let (a, b) = if b < a { (b, a) } else { (a, b) };
                    (Some(a), (b - a).max(1))
                }
                (LinhaGrade::Num(a), LinhaGrade::Vao(s)) => (Some(num(a)), s.max(1) as usize),
                (LinhaGrade::Num(a), LinhaGrade::Auto) => (Some(num(a)), 1),
                (LinhaGrade::Vao(s), LinhaGrade::Num(b)) => {
                    let b = num(b);
                    (Some(b.saturating_sub(s.max(1) as usize)), s.max(1) as usize)
                }
                (LinhaGrade::Vao(s), _) | (_, LinhaGrade::Vao(s)) => (None, s.max(1) as usize),
                (LinhaGrade::Auto, LinhaGrade::Num(b)) => (Some(num(b).saturating_sub(1)), 1),
                _ => (None, 1),
            }
        };
        let mut lugares: Vec<Lugar> = Vec::new();
        let mut cursor = (0usize, 0usize);
        for &f in &filhos {
            let ef = self.arv.caixas[f as usize].estilo.clone();
            let (mut lin, mut nl) = resolver(ef.grade_linha.0, ef.grade_linha.1, linhas_def.len().max(1));
            let (mut col, mut nc) = resolver(ef.grade_coluna.0, ef.grade_coluna.1, ncol);
            if let Some(nome) = ef.area {
                // area nomeada: o retangulo onde o nome aparece
                let mut achou: Option<(usize, usize, usize, usize)> = None;
                for (li, l2) in areas.iter().enumerate() {
                    for (ci, c) in l2.iter().enumerate() {
                        if *c == Some(nome) {
                            achou = Some(match achou {
                                None => (li, ci, li, ci),
                                Some((a, b, c2, d)) => (a.min(li), b.min(ci), c2.max(li), d.max(ci)),
                            });
                        }
                    }
                }
                if let Some((l0, c0, l1, c1)) = achou {
                    lin = Some(l0);
                    col = Some(c0);
                    nl = l1 - l0 + 1;
                    nc = c1 - c0 + 1;
                }
            }
            let nc = nc.min(ncol);
            let (l0, c0) = match (lin, col) {
                (Some(a), Some(b)) => (a, b.min(ncol - nc)),
                (Some(a), None) => {
                    garantir(&mut ocupado, a + nl);
                    let mut c = 0;
                    while c + nc <= ncol && (a..a + nl).any(|r| (c..c + nc).any(|cc| ocupado[r][cc])) {
                        c += 1;
                    }
                    (a, if c + nc > ncol { 0 } else { c })
                }
                (None, col_fixa) => {
                    // automatica: proximo lugar livre a partir do cursor
                    let (mut r, mut c) = if e.fluxo_denso { (0, 0) } else { cursor };
                    if let Some(cf) = col_fixa {
                        c = cf.min(ncol - nc);
                    }
                    loop {
                        garantir(&mut ocupado, r + nl);
                        if c + nc > ncol {
                            r += 1;
                            c = col_fixa.unwrap_or(0);
                            continue;
                        }
                        let livre = (r..r + nl).all(|rr| (c..c + nc).all(|cc| !ocupado[rr][cc]));
                        if livre {
                            break;
                        }
                        if col_fixa.is_some() {
                            r += 1;
                        } else {
                            c += 1;
                        }
                    }
                    if !e.fluxo_denso {
                        cursor = (r, c + nc);
                    }
                    (r, c)
                }
            };
            garantir(&mut ocupado, l0 + nl);
            for r in l0..l0 + nl {
                for c in c0..(c0 + nc).min(ncol) {
                    ocupado[r][c] = true;
                }
            }
            lugares.push(Lugar { k: f, lin: l0, col: c0, nl, nc });
        }
        let nlin = ocupado.len().max(linhas_def.len()).max(1);
        let auto_linha = e.auto_linhas.first().cloned().unwrap_or(Faixa::Auto);
        while linhas_def.len() < nlin {
            linhas_def.push(auto_linha.clone());
        }

        // larguras das colunas
        let mut larg: Vec<Fixo> = alloc::vec![0; ncol];
        let mut cresce: Vec<Option<Fixo>> = alloc::vec![None; ncol];
        let mut frs: Vec<f32> = alloc::vec![0.0; ncol];
        for (i, f) in colunas.iter().enumerate() {
            let (mn, mx, fr) = limites(f, Some(l));
            frs[i] = fr;
            larg[i] = mn.unwrap_or(0);
            cresce[i] = mx;
            if mn.is_none() || (mx.is_none() && fr == 0.0) {
                // conteudo: maior max-content (ou min-content) dos itens de span 1
                let quer_min = matches!(f, Faixa::MinConteudo);
                for lg in lugares.iter().filter(|p| p.col == i && p.nc == 1) {
                    let (a, b) = self.intrinseca(lg.k);
                    let m = self.margens_h_pub(lg.k);
                    let v = if quer_min { a } else { b } + m;
                    if mn.is_none() {
                        larg[i] = larg[i].max(if fr > 0.0 { a + m } else { v });
                    }
                    if mx.is_none() && fr == 0.0 {
                        cresce[i] = Some(cresce[i].unwrap_or(0).max(v));
                    }
                }
            }
        }
        let gaps_c = gap_c * (ncol as Fixo - 1).max(0);
        // auto cresce ate o max-content; depois fr divide o resto
        let mut usado: Fixo = larg.iter().sum::<Fixo>() + gaps_c;
        for i in 0..ncol {
            if frs[i] == 0.0 {
                if let Some(mx) = cresce[i] {
                    if mx > larg[i] && usado < l {
                        let d = (mx - larg[i]).min(l - usado);
                        larg[i] += d;
                        usado += d;
                    }
                }
            }
        }
        let total_fr: f32 = frs.iter().sum();
        if total_fr > 0.0 {
            let sobra = (l - (larg.iter().enumerate().filter(|(i, _)| frs[*i] == 0.0).map(|(_, v)| *v).sum::<Fixo>() + gaps_c)).max(0);
            let por_fr = sobra as f32 / total_fr.max(1.0);
            for i in 0..ncol {
                if frs[i] > 0.0 {
                    larg[i] = larg[i].max((por_fr * frs[i]) as Fixo);
                }
            }
        } else if usado < l && matches!(e.justificar, Alinhar::Normal | Alinhar::Esticar) {
            // colunas auto esticam
            let autos: Vec<usize> = (0..ncol).filter(|&i| matches!(colunas[i], Faixa::Auto)).collect();
            if !autos.is_empty() {
                let d = (l - usado) / autos.len() as Fixo;
                for i in autos {
                    larg[i] += d;
                }
            }
        }
        let mut xs = alloc::vec![0 as Fixo; ncol + 1];
        for i in 0..ncol {
            xs[i + 1] = xs[i] + larg[i] + if i + 1 < ncol { gap_c } else { 0 };
        }
        // deslocamento de justify-content da grade inteira
        let total_l = xs[ncol];
        let desloc_x = match e.justificar {
            Alinhar::Centro => (l - total_l).max(0) / 2,
            Alinhar::Fim | Alinhar::Direita => (l - total_l).max(0),
            _ => 0,
        };

        // alturas: layout dos itens com a largura da area
        let largura_area = |p: &Lugar| xs[(p.col + p.nc).min(ncol)] - xs[p.col] - if p.col + p.nc < ncol { gap_c } else { 0 };
        let mut alt: Vec<Fixo> = alloc::vec![0; nlin];
        for (i, f) in linhas_def.iter().enumerate() {
            let (mn, _, _) = limites(f, cb_a);
            alt[i] = mn.unwrap_or(0);
        }
        let mut alturas_itens = alloc::vec![0 as Fixo; lugares.len()];
        for (n, p) in lugares.iter().enumerate() {
            let c = &self.arv.caixas[p.k as usize];
            let mh = c.margem[1] + c.margem[3];
            let mv = c.margem[0] + c.margem[2];
            let w = (largura_area(p) - mh).max(0);
            let ef = c.estilo.clone();
            let alinhar_h = match ef.justificar_proprio {
                Alinhar::Auto | Alinhar::Normal => e.justificar_itens,
                a => a,
            };
            let w = if matches!(alinhar_h, Alinhar::Normal | Alinhar::Esticar) && ef.largura.eh_auto() {
                w
            } else {
                self.largura_encolhida(p.k, w + mh).min(w.max(0))
            };
            self.bloco_fixo(p.k, 0, 0, w, None, l, None);
            let h = self.arv.caixas[p.k as usize].a + mv;
            alturas_itens[n] = h;
            if p.nl == 1 {
                let fixa = matches!(linhas_def[p.lin], Faixa::M(_));
                if !fixa {
                    alt[p.lin] = alt[p.lin].max(h);
                }
            }
        }
        // itens que ocupam varias linhas: a ultima cresce se faltar
        for (n, p) in lugares.iter().enumerate() {
            if p.nl > 1 {
                let atual: Fixo = alt[p.lin..(p.lin + p.nl).min(nlin)].iter().sum::<Fixo>() + gap_l * (p.nl as Fixo - 1);
                if alturas_itens[n] > atual {
                    let ult = (p.lin + p.nl - 1).min(nlin - 1);
                    alt[ult] += alturas_itens[n] - atual;
                }
            }
        }
        // linhas fr com altura definida
        if let Some(h) = cb_a {
            let frs_l: Vec<f32> = linhas_def.iter().map(|f| limites(f, cb_a).2).collect();
            let total: f32 = frs_l.iter().sum();
            if total > 0.0 {
                let fixo: Fixo = alt.iter().enumerate().filter(|(i, _)| frs_l[*i] == 0.0).map(|(_, v)| *v).sum::<Fixo>()
                    + gap_l * (nlin as Fixo - 1);
                let sobra = (h - fixo).max(0);
                for i in 0..nlin {
                    if frs_l[i] > 0.0 {
                        alt[i] = alt[i].max((sobra as f32 * frs_l[i] / total) as Fixo);
                    }
                }
            }
        }
        let mut ys = alloc::vec![0 as Fixo; nlin + 1];
        for i in 0..nlin {
            ys[i + 1] = ys[i] + alt[i] + if i + 1 < nlin { gap_l } else { 0 };
        }

        // posicao final
        for (n, p) in lugares.iter().enumerate() {
            let c = &self.arv.caixas[p.k as usize];
            let ef = c.estilo.clone();
            let (mt, mr, mb, ml) = (c.margem[0], c.margem[1], c.margem[2], c.margem[3]);
            let ax = x + desloc_x + xs[p.col];
            let ay = y + ys[p.lin];
            let aw = largura_area(p);
            let ah = ys[(p.lin + p.nl).min(nlin)] - ys[p.lin] - if p.lin + p.nl < nlin { gap_l } else { 0 };
            let w_item = self.arv.caixas[p.k as usize].l;
            let h_item = alturas_itens[n] - mt - mb;
            let alinhar_v = match ef.alinhar_proprio {
                Alinhar::Auto | Alinhar::Normal => e.alinhar_itens,
                a => a,
            };
            let alinhar_h = match ef.justificar_proprio {
                Alinhar::Auto | Alinhar::Normal => e.justificar_itens,
                a => a,
            };
            let ix = match alinhar_h {
                Alinhar::Centro => ax + (aw - w_item - ml - mr) / 2 + ml,
                Alinhar::Fim | Alinhar::Direita => ax + aw - w_item - mr,
                _ => ax + ml,
            };
            let esticar = matches!(alinhar_v, Alinhar::Normal | Alinhar::Esticar) && ef.altura.eh_auto();
            let (iy, ih) = match alinhar_v {
                _ if esticar => (ay + mt, Some((ah - mt - mb).max(h_item))),
                Alinhar::Centro => (ay + (ah - h_item - mt - mb) / 2 + mt, None),
                Alinhar::Fim => (ay + ah - h_item - mb, None),
                _ => (ay + mt, None),
            };
            self.bloco_fixo(p.k, ix, iy, w_item, ih, l, cb_a);
        }
        ys[nlin]
    }

    pub fn margens_h_pub(&mut self, k: u32) -> Fixo {
        let c = &self.arv.caixas[k as usize];
        let m = |t: crate::web::css::estilo::Tam| match t {
            crate::web::css::estilo::Tam::M(m) if !m.tem_pct() => m.px,
            _ => 0,
        };
        m(c.estilo.margem[1]) + m(c.estilo.margem[3])
    }
}
