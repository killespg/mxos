//! Larguras intrinsecas (min-content e max-content) da borda externa de
//! uma caixa: o que floats, inline-block, absolutos, itens flex e colunas
//! de tabela usam para decidir o proprio tamanho. Guardadas em cache.

use super::Motor;
use super::caixa::Tipo;
use crate::web::css::estilo::{Lado, Tam};
use crate::web::css::valores::Fixo;

impl Motor<'_, '_> {
    /// (min-content, max-content) da borda externa (sem as margens).
    pub fn intrinseca(&mut self, k: u32) -> (Fixo, Fixo) {
        if let Some(Some(v)) = self.intrinsecas.get(k as usize) {
            return *v;
        }
        let v = self.calcular_intrinseca(k);
        if (k as usize) < self.intrinsecas.len() {
            self.intrinsecas[k as usize] = Some(v);
        }
        v
    }

    fn calcular_intrinseca(&mut self, k: u32) -> (Fixo, Fixo) {
        // lados em relacao a 0 (porcentagens nao contam aqui)
        self.lados(k, 0);
        let lados = self.lados_h(k);
        let e = self.arv.caixas[k as usize].estilo.clone();
        if let Tam::M(m) = e.largura {
            if !m.tem_pct() {
                let w = if e.caixa_borda { m.px } else { m.px + lados };
                let w = self.limitar_largura(k, w, None);
                return (w, w);
            }
        }
        let tipo = self.arv.caixas[k as usize].tipo.clone();
        let (mn, mx) = match tipo {
            Tipo::Substituido(_) => {
                let (w, _) = self.tamanho_substituido(k, 0, None);
                let (mn, mx) = (w - lados, w - lados);
                // max-width em % deixa a imagem encolher ate 0
                if matches!(e.max_largura, Tam::M(m) if m.tem_pct()) || matches!(e.largura, Tam::M(m) if m.tem_pct()) {
                    (0, mx)
                } else {
                    (mn, mx)
                }
            }
            Tipo::Texto(_) | Tipo::Inline | Tipo::Quebra => self.intrinseca_inline(&[k]),
            Tipo::Flex => {
                let filhos = self.arv.caixas[k as usize].filhos.clone();
                let linha = e.direcao_flex.eh_linha();
                let mut mn: Fixo = 0;
                let mut mx: Fixo = 0;
                let gap = if linha { e.lacuna.1.px } else { 0 };
                let mut n = 0;
                for f in filhos {
                    if self.arv.caixas[f as usize].estilo.fora_do_fluxo() {
                        continue;
                    }
                    let (a, b) = self.intrinseca(f);
                    let m = self.margens_h(f);
                    if linha {
                        mx += b + m;
                        mn = if e.quebra_flex { mn.max(a + m) } else { mn + a + m };
                    } else {
                        mx = mx.max(b + m);
                        mn = mn.max(a + m);
                    }
                    n += 1;
                }
                if n > 1 {
                    mx += gap * (n - 1);
                    if !e.quebra_flex && linha {
                        mn += gap * (n - 1);
                    }
                }
                (mn, mx)
            }
            Tipo::Tabela => self.intrinseca_tabela(k),
            Tipo::Grid => {
                let filhos = self.arv.caixas[k as usize].filhos.clone();
                let colunas = e.colunas_grade.len().max(1) as Fixo;
                let mut mn: Fixo = 0;
                let mut mx: Fixo = 0;
                for f in filhos {
                    let (a, b) = self.intrinseca(f);
                    let m = self.margens_h(f);
                    mn = mn.max(a + m);
                    mx = mx.max(b + m);
                }
                (mn, mx * colunas + e.lacuna.1.px * (colunas - 1))
            }
            _ => {
                let filhos = self.arv.caixas[k as usize].filhos.clone();
                let tem_inline = filhos.iter().any(|&f| self.arv.caixas[f as usize].eh_inline());
                if tem_inline {
                    self.intrinseca_inline(&filhos)
                } else {
                    let mut mn: Fixo = 0;
                    let mut mx: Fixo = 0;
                    // floats lado a lado somam no max
                    let mut floats_linha: Fixo = 0;
                    for f in filhos {
                        let est = self.arv.caixas[f as usize].estilo.clone();
                        if est.fora_do_fluxo() {
                            continue;
                        }
                        let (a, b) = self.intrinseca(f);
                        let m = self.margens_h(f);
                        mn = mn.max(a + m);
                        if est.flutuar != Lado::Nenhum {
                            floats_linha += b + m;
                            mx = mx.max(floats_linha);
                        } else {
                            floats_linha = 0;
                            mx = mx.max(b + m);
                        }
                    }
                    (mn, mx)
                }
            }
        };
        let mn = mn.max(0) + lados;
        let mx = mx.max(mn - lados) + lados;
        (self.limitar_largura(k, mn, None), self.limitar_largura(k, mx, None))
    }

    fn margens_h(&mut self, k: u32) -> Fixo {
        let e = self.arv.caixas[k as usize].estilo.clone();
        let m = |t: Tam| match t {
            Tam::M(m) if !m.tem_pct() => m.px,
            _ => 0,
        };
        m(e.margem[1]) + m(e.margem[3])
    }

    /// min/max de um pedaco de conteudo inline (uma lista de caixas irmas).
    fn intrinseca_inline(&mut self, caixas: &[u32]) -> (Fixo, Fixo) {
        let mut mn: Fixo = 0;
        let mut linha: Fixo = 0;
        let mut mx: Fixo = 0;
        self.acumular_inline(caixas, &mut mn, &mut linha, &mut mx);
        (mn, mx.max(linha))
    }

    fn acumular_inline(&mut self, caixas: &[u32], mn: &mut Fixo, linha: &mut Fixo, mx: &mut Fixo) {
        for &k in caixas {
            let c = &self.arv.caixas[k as usize];
            if c.estilo.fora_do_fluxo() {
                continue;
            }
            let flutua = c.estilo.flutuar != Lado::Nenhum;
            match c.tipo.clone() {
                Tipo::Texto(t) => {
                    let e = c.estilo.clone();
                    let esc = self.ctx.escolha(&e);
                    let quebra = e.brancos.quebra_linha();
                    let espaco = self.ctx.fontes.medir(&esc, " ", e.espaco_letras);
                    if e.brancos.mantem_quebras() {
                        for (n, parte) in t.split('\n').enumerate() {
                            if n > 0 {
                                *mx = (*mx).max(*linha);
                                *linha = 0;
                            }
                            let w = self.ctx.fontes.medir(&esc, parte, e.espaco_letras);
                            *linha += w;
                            if quebra {
                                for p in parte.split(' ') {
                                    *mn = (*mn).max(self.ctx.fontes.medir(&esc, p, e.espaco_letras));
                                }
                            } else {
                                *mn = (*mn).max(*linha);
                            }
                        }
                    } else {
                        let mut primeira = true;
                        for p in t.split(' ') {
                            let w = self.ctx.fontes.medir(&esc, p, e.espaco_letras);
                            if !primeira {
                                *linha += espaco + e.espaco_palavras;
                            }
                            primeira = false;
                            *linha += w;
                            if quebra {
                                if e.quebra_palavra {
                                    // pode quebrar em qualquer letra: o minimo e uma letra
                                    *mn = (*mn).max(w.min(e.tamanho_fixo()));
                                } else {
                                    *mn = (*mn).max(w);
                                }
                            }
                        }
                        if !quebra {
                            *mn = (*mn).max(*linha);
                        }
                    }
                }
                Tipo::Quebra => {
                    *mx = (*mx).max(*linha);
                    *linha = 0;
                }
                Tipo::Inline => {
                    self.lados(k, 0);
                    let cx = &self.arv.caixas[k as usize];
                    let lados = cx.margem[1] + cx.margem[3] + cx.borda[1] + cx.borda[3] + cx.preench[1] + cx.preench[3];
                    *linha += lados;
                    let filhos = cx.filhos.clone();
                    self.acumular_inline(&filhos, mn, linha, mx);
                }
                _ => {
                    let (a, b) = self.intrinseca(k);
                    let m = self.margens_h(k);
                    *mn = (*mn).max(a + m);
                    if flutua {
                        *mx = (*mx).max(b + m);
                    } else {
                        *linha += b + m;
                    }
                }
            }
        }
    }
}
