//! Flexbox (CSS Flexible Box Layout 1, secao 9): tamanho base, linhas,
//! crescer/encolher com congelamento nos limites min/max, tamanho
//! cruzado, stretch, justify-content (com margens auto) e align-*.

use alloc::vec::Vec;

use super::Motor;
use super::caixa::Tipo;
use crate::web::css::estilo::{Alinhar, Tam, Transbordo};
use crate::web::css::valores::{Fixo, Medida};

struct Item {
    k: u32,
    base: Fixo,
    hipotetico: Fixo,
    min: Fixo,
    max: Fixo,
    final_: Fixo,
    congelado: bool,
    /// margens no eixo principal (antes, depois) e se sao auto
    m_ini: Fixo,
    m_fim: Fixo,
    auto_ini: bool,
    auto_fim: bool,
    cruzado: Fixo,
}

fn tam_px(t: Tam, base: Option<Fixo>) -> Option<Fixo> {
    match t {
        Tam::M(m) if m.tem_pct() => base.map(|b| m.resolver(b)),
        Tam::M(m) => Some(m.px),
        _ => None,
    }
}

impl Motor<'_, '_> {
    /// Layout de um item com tamanho de borda externa definido (altura
    /// opcional), borda em (x, y).
    pub fn bloco_fixo(&mut self, k: u32, x: Fixo, y: Fixo, w: Fixo, h: Option<Fixo>, cb_l: Fixo, cb_a: Option<Fixo>) {
        if let Tipo::Substituido(_) = self.arv.caixas[k as usize].tipo {
            self.lados(k, cb_l);
            let (_, h0) = self.tamanho_substituido(k, cb_l, cb_a);
            let h_nat = match self.estilo(k).proporcao.or_else(|| {
                let (nl, na) = self.tamanho_natural(k);
                if na > 0 { Some(nl as f32 / na as f32) } else { None }
            }) {
                Some(r) if self.estilo(k).altura.eh_auto() => ((w - self.lados_h(k)) as f32 / r) as Fixo + self.lados_v(k),
                _ => h0,
            };
            let c = &mut self.arv.caixas[k as usize];
            c.x = x;
            c.y = y;
            c.l = w;
            c.a = h.unwrap_or(h_nat);
            return;
        }
        let salvo = self.arv.caixas[k as usize].estilo.clone();
        let mut e = (*salvo).clone();
        e.largura = Tam::M(Medida::px(w));
        e.caixa_borda = true;
        e.margem = [Tam::ZERO; 4];
        e.min_largura = Tam::Auto;
        e.max_largura = Tam::Nenhum;
        if let Some(h) = h {
            e.altura = Tam::M(Medida::px(h));
            e.min_altura = Tam::Auto;
            e.max_altura = Tam::Nenhum;
        }
        self.arv.caixas[k as usize].estilo = alloc::rc::Rc::new(e);
        let mut fl = super::Floats::default();
        self.bloco(k, x, y, w, cb_a, &mut fl);
        self.arv.caixas[k as usize].estilo = salvo;
        self.lados(k, cb_l);
        let c = &mut self.arv.caixas[k as usize];
        c.x = x;
        c.y = y;
    }

    /// Altura (borda externa) que o item teria com essa largura.
    fn altura_com_largura(&mut self, k: u32, w: Fixo, cb_l: Fixo, cb_a: Option<Fixo>) -> Fixo {
        self.bloco_fixo(k, 0, 0, w, None, cb_l, cb_a);
        self.arv.caixas[k as usize].a
    }

    /// Devolve a altura do conteudo do container.
    pub fn flex(&mut self, k: u32, x: Fixo, y: Fixo, l: Fixo, cb_a: Option<Fixo>) -> Fixo {
        let e = self.arv.caixas[k as usize].estilo.clone();
        let linha = e.direcao_flex.eh_linha();
        let inverso = e.direcao_flex.inversa();
        let gap_principal = if linha { e.lacuna.1.resolver(l) } else { e.lacuna.0.resolver(cb_a.unwrap_or(0)) };
        let gap_cruzado = if linha { e.lacuna.0.resolver(cb_a.unwrap_or(0)) } else { e.lacuna.1.resolver(l) };
        let tamanho_principal: Option<Fixo> = if linha { Some(l) } else { cb_a };

        // itens em fluxo, pela propriedade order
        let mut filhos: Vec<u32> = Vec::new();
        for f in self.arv.caixas[k as usize].filhos.clone() {
            if self.estilo(f).fora_do_fluxo() {
                self.absolutos_estatico(f, x, y);
                continue;
            }
            filhos.push(f);
        }
        filhos.sort_by_key(|&f| self.estilo(f).ordem);

        let mut itens: Vec<Item> = Vec::with_capacity(filhos.len());
        for &f in &filhos {
            self.lados(f, l);
            let ef = self.arv.caixas[f as usize].estilo.clone();
            let c = &self.arv.caixas[f as usize];
            let (m_ini, m_fim, auto_ini, auto_fim) = if linha {
                (c.margem[3], c.margem[1], ef.margem[3].eh_auto(), ef.margem[1].eh_auto())
            } else {
                (c.margem[0], c.margem[2], ef.margem[0].eh_auto(), ef.margem[2].eh_auto())
            };
            let lados = if linha { self.lados_h(f) } else { self.lados_v(f) };
            let extra = if ef.caixa_borda { 0 } else { lados };
            // tamanho base
            let base = match ef.base_flex {
                Tam::M(m) if !m.tem_pct() || tamanho_principal.is_some() => {
                    let v = m.resolver(tamanho_principal.unwrap_or(0));
                    if ef.caixa_borda { v } else { v + lados }
                }
                _ => {
                    let prop = if linha { ef.largura } else { ef.altura };
                    let base_pct = if linha { Some(l) } else { cb_a };
                    match tam_px(prop, base_pct) {
                        Some(v) => v + extra,
                        None => {
                            if linha {
                                self.intrinseca(f).1
                            } else {
                                let w = self.largura_item_coluna(f, l, &e);
                                self.altura_com_largura(f, w, l, None)
                            }
                        }
                    }
                }
            };
            let (min, max) = if linha {
                let min = match ef.min_largura {
                    Tam::Auto if matches!(ef.transbordo[0], Transbordo::Visivel) => {
                        let mc = self.intrinseca(f).0;
                        match tam_px(ef.largura, Some(l)) {
                            Some(w) => mc.min(w + extra),
                            None => mc,
                        }
                    }
                    t => tam_px(t, Some(l)).map_or(lados, |v| v + extra),
                };
                let max = tam_px(ef.max_largura, Some(l)).map_or(Fixo::MAX / 4, |v| v + extra);
                (min, max)
            } else {
                let min = tam_px(ef.min_altura, cb_a).map_or(lados, |v| v + extra);
                let max = tam_px(ef.max_altura, cb_a).map_or(Fixo::MAX / 4, |v| v + extra);
                (min, max)
            };
            let max = max.max(min);
            let hip = base.clamp(min, max);
            itens.push(Item {
                k: f,
                base,
                hipotetico: hip,
                min,
                max,
                final_: hip,
                congelado: false,
                m_ini: if auto_ini { 0 } else { m_ini },
                m_fim: if auto_fim { 0 } else { m_fim },
                auto_ini,
                auto_fim,
                cruzado: 0,
            });
        }

        // linhas
        let mut linhas: Vec<Vec<usize>> = Vec::new();
        {
            let mut atual: Vec<usize> = Vec::new();
            let mut soma = 0;
            for (i, it) in itens.iter().enumerate() {
                let ext = it.hipotetico + it.m_ini + it.m_fim;
                let com_gap = if atual.is_empty() { ext } else { ext + gap_principal };
                if e.quebra_flex && !atual.is_empty() && tamanho_principal.is_some_and(|t| soma + com_gap > t) {
                    linhas.push(core::mem::take(&mut atual));
                    soma = ext;
                } else {
                    soma += com_gap;
                }
                atual.push(i);
            }
            if !atual.is_empty() {
                linhas.push(atual);
            }
        }
        if e.quebra_flex_inversa {
            linhas.reverse();
        }

        // tamanhos flexiveis
        for ln in &linhas {
            let disponivel = match tamanho_principal {
                Some(t) => t,
                None => {
                    // principal indefinido (coluna sem altura): cada um no hipotetico
                    for &i in ln {
                        itens[i].final_ = itens[i].hipotetico;
                    }
                    continue;
                }
            };
            let gaps = gap_principal * (ln.len() as Fixo - 1).max(0);
            let soma_hip: Fixo = ln.iter().map(|&i| itens[i].hipotetico + itens[i].m_ini + itens[i].m_fim).sum::<Fixo>() + gaps;
            let crescendo = soma_hip < disponivel;
            for &i in ln {
                let it = &mut itens[i];
                let ef = &self.arv.caixas[it.k as usize].estilo;
                let fator = if crescendo { ef.crescer } else { ef.encolher };
                it.congelado = fator == 0.0 || (crescendo && it.base > it.hipotetico) || (!crescendo && it.base < it.hipotetico);
                it.final_ = it.hipotetico;
            }
            for _ in 0..8 {
                let livres: Vec<usize> = ln.iter().copied().filter(|&i| !itens[i].congelado).collect();
                if livres.is_empty() {
                    break;
                }
                let usado: Fixo = ln
                    .iter()
                    .map(|&i| {
                        let it = &itens[i];
                        (if it.congelado { it.final_ } else { it.base }) + it.m_ini + it.m_fim
                    })
                    .sum::<Fixo>()
                    + gaps;
                let sobra = disponivel - usado;
                let mut violacao_min = 0i64;
                let mut violacao_max = 0i64;
                if crescendo {
                    let total: f32 = livres.iter().map(|&i| self.estilo(itens[i].k).crescer).sum();
                    let total = if total < 1.0 { 1.0f32.max(total) } else { total };
                    let total_real: f32 = livres.iter().map(|&i| self.estilo(itens[i].k).crescer).sum();
                    for &i in &livres {
                        let g = self.estilo(itens[i].k).crescer;
                        let parte = if total_real < 1.0 { sobra as f32 * g } else { sobra as f32 * g / total };
                        let alvo = itens[i].base + parte as Fixo;
                        let c = alvo.clamp(itens[i].min, itens[i].max);
                        if c > alvo {
                            violacao_min += (c - alvo) as i64;
                        } else if c < alvo {
                            violacao_max += (alvo - c) as i64;
                        }
                        itens[i].final_ = c;
                    }
                } else {
                    let total: f32 = livres.iter().map(|&i| self.estilo(itens[i].k).encolher * itens[i].base as f32).sum();
                    for &i in &livres {
                        let pesado = self.estilo(itens[i].k).encolher * itens[i].base as f32;
                        let parte = if total > 0.0 { sobra as f32 * pesado / total } else { 0.0 };
                        let alvo = itens[i].base + parte as Fixo;
                        let c = alvo.clamp(itens[i].min, itens[i].max);
                        if c > alvo {
                            violacao_min += (c - alvo) as i64;
                        } else if c < alvo {
                            violacao_max += (alvo - c) as i64;
                        }
                        itens[i].final_ = c;
                    }
                }
                let total = violacao_min - violacao_max;
                if total == 0 {
                    break;
                }
                for &i in &livres {
                    let it = &mut itens[i];
                    let alvo_min = it.final_ == it.min;
                    let alvo_max = it.final_ == it.max;
                    if (total > 0 && alvo_min) || (total < 0 && alvo_max) {
                        it.congelado = true;
                    }
                }
            }
        }

        // tamanho cruzado de cada item
        let cruzado_container: Option<Fixo> = if linha { cb_a } else { Some(l) };
        for i in 0..itens.len() {
            let f = itens[i].k;
            let w_principal = itens[i].final_;
            if linha {
                let h = self.altura_com_largura(f, w_principal, l, cb_a);
                itens[i].cruzado = h;
            } else {
                let w = self.largura_item_coluna(f, l, &e);
                itens[i].cruzado = w;
            }
        }
        let margens_cruzado = |m: &Motor, k: u32| -> Fixo {
            let c = &m.arv.caixas[k as usize];
            if linha { c.margem[0] + c.margem[2] } else { c.margem[1] + c.margem[3] }
        };
        let mut cruzados_linha: Vec<Fixo> = linhas
            .iter()
            .map(|ln| ln.iter().map(|&i| itens[i].cruzado + margens_cruzado(self, itens[i].k)).max().unwrap_or(0))
            .collect();
        if linhas.len() == 1 {
            if let Some(c) = cruzado_container {
                if !e.quebra_flex || linha {
                    cruzados_linha[0] = if linha { cruzados_linha[0].max(c) } else { c };
                    if linha && cb_a.is_some() {
                        cruzados_linha[0] = c;
                    }
                }
            }
        }
        // align-content: sobra do cruzado entre as linhas
        let total_cruzado: Fixo = cruzados_linha.iter().sum::<Fixo>() + gap_cruzado * (linhas.len() as Fixo - 1).max(0);
        let (mut inicio_cruzado, entre_linhas) = (0, 0);
        if let Some(c) = cruzado_container.filter(|_| linhas.len() > 1 || e.quebra_flex) {
            let sobra = c - total_cruzado;
            if sobra > 0 {
                match e.alinhar_conteudo {
                    Alinhar::Centro => inicio_cruzado = sobra / 2,
                    Alinhar::Fim => inicio_cruzado = sobra,
                    Alinhar::EspacoEntre if linhas.len() > 1 => {
                        let por = sobra / (linhas.len() as Fixo - 1);
                        let _ = por;
                    }
                    Alinhar::Normal | Alinhar::Esticar => {
                        let por = sobra / linhas.len() as Fixo;
                        for c in cruzados_linha.iter_mut() {
                            *c += por;
                        }
                    }
                    _ => {}
                }
            }
        }
        let _ = entre_linhas;

        // posicoes
        let mut pos_cruzado = inicio_cruzado;
        for (li, ln) in linhas.iter().enumerate() {
            let cruz = cruzados_linha[li];
            let ocupado: Fixo =
                ln.iter().map(|&i| itens[i].final_ + itens[i].m_ini + itens[i].m_fim).sum::<Fixo>() + gap_principal * (ln.len() as Fixo - 1).max(0);
            let disponivel = tamanho_principal.unwrap_or(ocupado);
            let sobra = (disponivel - ocupado).max(0);
            let autos = ln.iter().map(|&i| itens[i].auto_ini as Fixo + itens[i].auto_fim as Fixo).sum::<Fixo>();
            let n = ln.len() as Fixo;
            let (mut cursor, entre) = if autos > 0 {
                (0, 0)
            } else {
                match e.justificar {
                    Alinhar::Fim | Alinhar::Direita => (sobra, 0),
                    Alinhar::Centro => (sobra / 2, 0),
                    Alinhar::EspacoEntre if n > 1 => (0, sobra / (n - 1)),
                    Alinhar::EspacoEmVolta => (sobra / (2 * n), sobra / n),
                    Alinhar::EspacoUniforme => (sobra / (n + 1), sobra / (n + 1)),
                    _ => (0, 0),
                }
            };
            let por_auto = if autos > 0 { sobra / autos } else { 0 };
            let ordem: Vec<usize> = if inverso { ln.iter().rev().copied().collect() } else { ln.clone() };
            for &i in &ordem {
                let it = &itens[i];
                let f = it.k;
                let ef = self.arv.caixas[f as usize].estilo.clone();
                cursor += it.m_ini + if it.auto_ini { por_auto } else { 0 };
                let principal = if inverso { disponivel - cursor - it.final_ } else { cursor };
                // alinhamento cruzado
                let alinhar = match ef.alinhar_proprio {
                    Alinhar::Auto | Alinhar::Normal => e.alinhar_itens,
                    a => a,
                };
                let (mc_ini, mc_fim) = {
                    let c = &self.arv.caixas[f as usize];
                    if linha { (c.margem[0], c.margem[2]) } else { (c.margem[3], c.margem[1]) }
                };
                let auto_cruzado = if linha { ef.altura.eh_auto() } else { ef.largura.eh_auto() };
                let esticar = matches!(alinhar, Alinhar::Normal | Alinhar::Esticar) && auto_cruzado;
                let tamanho_c = if esticar { (cruz - mc_ini - mc_fim).max(0) } else { it.cruzado };
                let desloc_c = match alinhar {
                    Alinhar::Centro => (cruz - tamanho_c - mc_ini - mc_fim) / 2 + mc_ini,
                    Alinhar::Fim | Alinhar::Direita => cruz - tamanho_c - mc_fim,
                    _ => mc_ini,
                };
                let (ix, iy, iw, ih) = if linha {
                    (x + principal, y + pos_cruzado + desloc_c, it.final_, Some(tamanho_c))
                } else {
                    (x + pos_cruzado + desloc_c, y + principal, tamanho_c, Some(it.final_))
                };
                let forcar_altura = if linha { if esticar { ih } else { None } } else { ih };
                self.bloco_fixo(f, ix, iy, iw, forcar_altura, l, cb_a);
                if self.estilo(f).posicao == crate::web::css::estilo::Posicao::Relativa {
                    self.relativa_pub(f, l, cb_a);
                }
                cursor += it.final_ + it.m_fim + if it.auto_fim { por_auto } else { 0 } + gap_principal + entre;
            }
            pos_cruzado += cruz + gap_cruzado;
        }
        if linha {
            (pos_cruzado - gap_cruzado).max(0)
        } else {
            match cb_a {
                Some(h) => h,
                None => {
                    // altura = soma dos itens da coluna
                    linhas
                        .iter()
                        .map(|ln| {
                            ln.iter().map(|&i| itens[i].final_ + itens[i].m_ini + itens[i].m_fim).sum::<Fixo>()
                                + gap_principal * (ln.len() as Fixo - 1).max(0)
                        })
                        .max()
                        .unwrap_or(0)
                }
            }
        }
    }

    /// Largura de um item numa coluna flex (esticado ou encolhido).
    fn largura_item_coluna(&mut self, f: u32, l: Fixo, container: &crate::web::css::estilo::Estilo) -> Fixo {
        let ef = self.arv.caixas[f as usize].estilo.clone();
        let margens = {
            let c = &self.arv.caixas[f as usize];
            c.margem[1] + c.margem[3]
        };
        if let Some(w) = self.largura_especificada(f, Some(l)) {
            return self.limitar_largura(f, w, Some(l));
        }
        let alinhar = match ef.alinhar_proprio {
            Alinhar::Auto | Alinhar::Normal => container.alinhar_itens,
            a => a,
        };
        if matches!(alinhar, Alinhar::Normal | Alinhar::Esticar) && !ef.margem[1].eh_auto() && !ef.margem[3].eh_auto() {
            self.limitar_largura(f, l - margens, Some(l))
        } else {
            self.largura_encolhida(f, l)
        }
    }
}
