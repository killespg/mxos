//! Tabelas (CSS 2.1 cap. 17, layout automatico e fixo): grade de celulas
//! com colspan/rowspan, larguras de coluna pelas larguras intrinsecas das
//! celulas, border-spacing, alturas de linha e vertical-align nas celulas.

use alloc::vec::Vec;

use super::Motor;
use super::caixa::Tipo;
use crate::web::atomo::a;
use crate::web::css::estilo::{AlinhamentoVertical, Tam};
use crate::web::css::valores::Fixo;

struct Celula {
    k: u32,
    lin: usize,
    col: usize,
    nl: usize,
    nc: usize,
}

impl Motor<'_, '_> {
    fn span(&self, k: u32, nome: crate::web::atomo::Atomo) -> usize {
        let no = self.arv.caixas[k as usize].no;
        self.ctx
            .doc
            .and_then(|d| d.elemento(no))
            .and_then(|e| e.atributo(nome))
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(1)
            .clamp(1, 1000)
    }

    /// Linhas da tabela (em ordem: cabecalho, corpo, rodape) e a legenda.
    fn linhas_tabela(&self, k: u32) -> (Vec<u32>, Vec<u32>) {
        let mut cab = Vec::new();
        let mut corpo = Vec::new();
        let mut rod = Vec::new();
        let mut legendas = Vec::new();
        for &g in &self.arv.caixas[k as usize].filhos {
            let c = &self.arv.caixas[g as usize];
            match c.tipo {
                Tipo::Legenda => legendas.push(g),
                Tipo::GrupoLinhas => {
                    let destino = match c.estilo.exibir {
                        crate::web::css::estilo::Exibir::GrupoCabecalho => &mut cab,
                        crate::web::css::estilo::Exibir::GrupoRodape => &mut rod,
                        _ => &mut corpo,
                    };
                    for &l in &c.filhos {
                        if matches!(self.arv.caixas[l as usize].tipo, Tipo::LinhaTabela) {
                            destino.push(l);
                        }
                    }
                }
                Tipo::LinhaTabela => corpo.push(g),
                _ => {}
            }
        }
        cab.extend(corpo);
        cab.extend(rod);
        (cab, legendas)
    }

    fn grade_celulas(&self, linhas: &[u32]) -> (Vec<Celula>, usize) {
        let mut cel = Vec::new();
        let mut ocupado: Vec<Vec<bool>> = Vec::new();
        let mut ncol = 0;
        for (li, &l) in linhas.iter().enumerate() {
            while ocupado.len() <= li {
                ocupado.push(Vec::new());
            }
            let mut c = 0;
            for &k in &self.arv.caixas[l as usize].filhos {
                if !matches!(self.arv.caixas[k as usize].tipo, Tipo::Celula) {
                    continue;
                }
                while ocupado[li].get(c).copied().unwrap_or(false) {
                    c += 1;
                }
                let nc = self.span(k, a::COLSPAN);
                let nl = self.span(k, a::ROWSPAN).min(linhas.len() - li).max(1);
                for r in li..li + nl {
                    while ocupado.len() <= r {
                        ocupado.push(Vec::new());
                    }
                    if ocupado[r].len() < c + nc {
                        ocupado[r].resize(c + nc, false);
                    }
                    for cc in c..c + nc {
                        ocupado[r][cc] = true;
                    }
                }
                cel.push(Celula { k, lin: li, col: c, nl, nc });
                c += nc;
                ncol = ncol.max(c);
            }
        }
        (cel, ncol)
    }

    pub fn intrinseca_tabela(&mut self, k: u32) -> (Fixo, Fixo) {
        let (linhas, _) = self.linhas_tabela(k);
        let (cel, ncol) = self.grade_celulas(&linhas);
        let esp = if self.estilo(k).borda_colapsada { 0 } else { self.estilo(k).espaco_borda };
        let (mn, mx) = self.colunas(&cel, ncol);
        let extra = esp * (ncol as Fixo + 1);
        (mn.iter().sum::<Fixo>() + extra, mx.iter().sum::<Fixo>() + extra)
    }

    /// (minimo, maximo) de cada coluna.
    fn colunas(&mut self, cel: &[Celula], ncol: usize) -> (Vec<Fixo>, Vec<Fixo>) {
        let mut mn = alloc::vec![0 as Fixo; ncol];
        let mut mx = alloc::vec![0 as Fixo; ncol];
        let mut ordem: Vec<&Celula> = cel.iter().collect();
        ordem.sort_by_key(|c| c.nc);
        for c in ordem {
            let (a, mut b) = self.intrinseca(c.k);
            // largura fixa na celula vale como minimo e maximo
            if let Tam::M(m) = self.estilo(c.k).largura {
                if !m.tem_pct() {
                    b = b.max(a);
                }
            }
            if c.nc == 1 {
                mn[c.col] = mn[c.col].max(a);
                mx[c.col] = mx[c.col].max(b);
            } else {
                let fim = (c.col + c.nc).min(ncol);
                let soma_mn: Fixo = mn[c.col..fim].iter().sum();
                let soma_mx: Fixo = mx[c.col..fim].iter().sum();
                let n = (fim - c.col) as Fixo;
                if a > soma_mn {
                    for v in &mut mn[c.col..fim] {
                        *v += (a - soma_mn) / n;
                    }
                }
                if b > soma_mx {
                    for v in &mut mx[c.col..fim] {
                        *v += (b - soma_mx) / n;
                    }
                }
            }
        }
        for i in 0..ncol {
            mx[i] = mx[i].max(mn[i]);
        }
        (mn, mx)
    }

    pub fn tabela(&mut self, k: u32, x: Fixo, y: Fixo, l: Fixo, _cb_a: Option<Fixo>) -> Fixo {
        let (linhas, legendas) = self.linhas_tabela(k);
        let (cel, ncol) = self.grade_celulas(&linhas);
        let e = self.arv.caixas[k as usize].estilo.clone();
        let esp = if e.borda_colapsada { 0 } else { e.espaco_borda };
        let mut cy = y;
        // legenda em cima
        for &lg in &legendas {
            self.bloco_fixo(lg, x, cy, l, None, l, None);
            cy += self.arv.caixas[lg as usize].a;
        }
        if ncol == 0 {
            return cy - y;
        }
        let (mn, mx) = self.colunas(&cel, ncol);
        let disponivel = l - esp * (ncol as Fixo + 1);
        let soma_mn: Fixo = mn.iter().sum();
        let soma_mx: Fixo = mx.iter().sum();
        let largura_definida = !e.largura.eh_auto();
        let mut larg: Vec<Fixo> = if e.layout_fixo_tabela && largura_definida {
            alloc::vec![disponivel / ncol as Fixo; ncol]
        } else if soma_mx <= disponivel {
            if largura_definida && soma_mx > 0 {
                // estica proporcional ao maximo
                mx.iter().map(|&v| v + (disponivel - soma_mx) * v / soma_mx.max(1)).collect()
            } else if largura_definida {
                alloc::vec![disponivel / ncol as Fixo; ncol]
            } else {
                mx.clone()
            }
        } else if soma_mn >= disponivel {
            mn.clone()
        } else {
            // entre o minimo e o maximo, proporcional a folga de cada coluna
            let folga_total = (soma_mx - soma_mn).max(1);
            let extra = disponivel - soma_mn;
            (0..ncol).map(|i| mn[i] + ((mx[i] - mn[i]) as i64 * extra as i64 / folga_total as i64) as Fixo).collect()
        };
        // colunas com largura em % (td width=25%): fatia fixa da tabela
        let mut pct = alloc::vec![None::<Fixo>; ncol];
        for c in &cel {
            if c.nc == 1 {
                if let Tam::M(m) = self.estilo(c.k).largura {
                    if m.tem_pct() {
                        pct[c.col] = Some(m.resolver(disponivel).max(mn[c.col]));
                    }
                }
            }
        }
        if pct.iter().any(|p| p.is_some()) {
            let fixo_pct: Fixo = pct.iter().flatten().sum();
            let resto: Vec<usize> = (0..ncol).filter(|&i| pct[i].is_none()).collect();
            let soma_resto: Fixo = resto.iter().map(|&i| larg[i]).sum();
            let alvo_resto = if largura_definida || soma_resto + fixo_pct > disponivel { (disponivel - fixo_pct).max(0) } else { soma_resto };
            for i in 0..ncol {
                if let Some(p) = pct[i] {
                    larg[i] = p;
                }
            }
            if soma_resto > 0 {
                for &i in &resto {
                    larg[i] = (larg[i] as i64 * alvo_resto as i64 / soma_resto as i64) as Fixo;
                }
            } else if !resto.is_empty() {
                for &i in &resto {
                    larg[i] = alvo_resto / resto.len() as Fixo;
                }
            }
        }
        for v in larg.iter_mut() {
            *v = (*v).max(0);
        }
        let mut xs = alloc::vec![0 as Fixo; ncol + 1];
        for i in 0..ncol {
            xs[i + 1] = xs[i] + larg[i] + esp;
        }
        // altura das celulas
        let nlin = linhas.len();
        let mut alt = alloc::vec![0 as Fixo; nlin];
        for (li, &lk) in linhas.iter().enumerate() {
            if let Some(h) = self.altura_especificada(lk, None) {
                alt[li] = h;
            }
        }
        for c in &cel {
            let w = xs[(c.col + c.nc).min(ncol)] - xs[c.col] - esp;
            self.bloco_fixo(c.k, 0, 0, w, None, l, None);
            let h = self.arv.caixas[c.k as usize].a;
            if c.nl == 1 {
                alt[c.lin] = alt[c.lin].max(h);
            }
        }
        for c in &cel {
            if c.nl > 1 {
                let atual: Fixo = alt[c.lin..c.lin + c.nl].iter().sum::<Fixo>() + esp * (c.nl as Fixo - 1);
                let h = self.arv.caixas[c.k as usize].a;
                if h > atual {
                    alt[c.lin + c.nl - 1] += h - atual;
                }
            }
        }
        let mut ys = alloc::vec![0 as Fixo; nlin + 1];
        for i in 0..nlin {
            ys[i + 1] = ys[i] + alt[i] + esp;
        }
        let topo = cy + esp;
        // linhas e grupos ocupam a faixa toda
        for (li, &lk) in linhas.iter().enumerate() {
            self.lados(lk, l);
            let c = &mut self.arv.caixas[lk as usize];
            c.x = x;
            c.y = topo + ys[li];
            c.l = l;
            c.a = alt[li];
        }
        for &g in &self.arv.caixas[k as usize].filhos.clone() {
            if matches!(self.arv.caixas[g as usize].tipo, Tipo::GrupoLinhas) {
                let filhos = self.arv.caixas[g as usize].filhos.clone();
                let (mut y0, mut y1) = (Fixo::MAX, Fixo::MIN);
                for f in filhos {
                    let c = &self.arv.caixas[f as usize];
                    y0 = y0.min(c.y);
                    y1 = y1.max(c.y + c.a);
                }
                let c = &mut self.arv.caixas[g as usize];
                c.x = x;
                c.l = l;
                if y0 <= y1 {
                    c.y = y0;
                    c.a = y1 - y0;
                }
            }
        }
        // celulas no lugar, com altura da linha e vertical-align
        for c in &cel {
            let w = xs[(c.col + c.nc).min(ncol)] - xs[c.col] - esp;
            let h = ys[(c.lin + c.nl).min(nlin)] - ys[c.lin] - esp;
            let cx = x + esp + xs[c.col];
            let cyy = topo + ys[c.lin];
            let h_conteudo = self.arv.caixas[c.k as usize].a;
            self.bloco_fixo(c.k, cx, cyy, w, Some(h), l, None);
            let va = self.estilo(c.k).alinhamento_vertical;
            let folga = h - h_conteudo;
            let dy = match va {
                AlinhamentoVertical::Meio => folga / 2,
                AlinhamentoVertical::Baixo => folga,
                _ => 0,
            };
            if dy > 0 {
                for f in self.arv.caixas[c.k as usize].filhos.clone() {
                    self.deslocar(f, 0, dy);
                }
                let linhas_c = &mut self.arv.caixas[c.k as usize].linhas;
                for ln in linhas_c.iter_mut() {
                    ln.deslocar(0, dy);
                }
            }
        }
        cy - y + ys[nlin] + esp
    }
}
