//! Contexto de formatacao inline: o texto (e as caixas inline, inline-block
//! e imagens) vira linhas. Passos:
//! 1. achatar o conteudo em itens (palavras medidas, aberturas/fechos de
//!    caixas inline, atomicos, quebras forcadas, floats);
//! 2. encher as linhas respeitando a largura livre (floats tiram espaco);
//! 3. em cada linha: alinhamento vertical (linha de base, middle, top...),
//!    altura da linha e text-align (inclusive justify).

use alloc::vec::Vec;

use super::caixa::{MARCADOR_EXTERNO, Tipo};
use super::{Floats, Motor};
use crate::web::css::estilo::{Alinhamento, AlinhamentoVertical, Brancos, Estilo, Lado};
use crate::web::css::valores::{Fixo, UM};
use crate::web::fonte::Escolha;

/// Um pedaco de texto ja colocado (coordenadas da pagina).
#[derive(Clone, Debug)]
pub struct Trecho {
    pub caixa: u32,
    pub ini: u32,
    pub fim: u32,
    pub x: Fixo,
    /// y da linha de base.
    pub base: Fixo,
    pub largura: Fixo,
    pub escolha: Escolha,
    /// Espacos extras por espaco (justify).
    pub extra_espaco: Fixo,
}

/// Fundo/borda de uma caixa inline num pedaco de linha.
#[derive(Clone, Debug)]
pub struct FundoInline {
    pub caixa: u32,
    pub x0: Fixo,
    pub x1: Fixo,
    pub topo: Fixo,
    pub baixo: Fixo,
    pub inicio: bool,
    pub fim: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Linha {
    pub y: Fixo,
    pub altura: Fixo,
    pub base: Fixo,
    pub trechos: Vec<Trecho>,
    pub fundos: Vec<FundoInline>,
    pub atomicos: Vec<u32>,
}

impl Linha {
    pub fn deslocar(&mut self, dx: Fixo, dy: Fixo) {
        self.y += dy;
        self.base += dy;
        for t in self.trechos.iter_mut() {
            t.x += dx;
            t.base += dy;
        }
        for f in self.fundos.iter_mut() {
            f.x0 += dx;
            f.x1 += dx;
            f.topo += dy;
            f.baixo += dy;
        }
    }
}

#[derive(Clone, Debug)]
enum Item {
    /// Palavra de uma caixa de texto: bytes [ini, fim) (sem o espaco);
    /// `espaco` = largura do espaco que vem depois (0 se nao tem).
    Palavra { caixa: u32, ini: u32, fim: u32, l: Fixo, espaco: Fixo, quebra_dentro: bool },
    Abre(u32),
    Fecha(u32),
    Atomico(u32),
    Quebra(u32),
    Float(u32),
    Absoluto(u32),
}

/// Medidas verticais de um estilo de texto: (acima da base, abaixo).
fn vertical(m: &Motor, e: &Estilo, esc: &Escolha) -> (Fixo, Fixo) {
    let (asc, desc) = m.ctx.fontes.metricas(esc);
    let linha = e.linha();
    let meio = (linha - (asc + desc)) / 2;
    (asc + meio, desc + (linha - (asc + desc) - meio))
}

fn eh_cjk(c: char) -> bool {
    matches!(c as u32, 0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF)
}

impl Motor<'_, '_> {
    fn itens(&mut self, k: u32, saida: &mut Vec<Item>, disponivel: Fixo) {
        let filhos = self.arv.caixas[k as usize].filhos.clone();
        for f in filhos {
            let c = &self.arv.caixas[f as usize];
            if c.estilo.fora_do_fluxo() {
                saida.push(Item::Absoluto(f));
                continue;
            }
            if c.estilo.flutuar != Lado::Nenhum && !matches!(c.tipo, Tipo::Texto(_)) {
                saida.push(Item::Float(f));
                continue;
            }
            match &c.tipo {
                Tipo::Texto(t) => {
                    let t = t.clone();
                    let e = c.estilo.clone();
                    self.palavras(f, &t, &e, saida);
                }
                Tipo::Inline => {
                    saida.push(Item::Abre(f));
                    self.itens(f, saida, disponivel);
                    saida.push(Item::Fecha(f));
                }
                Tipo::Quebra => saida.push(Item::Quebra(f)),
                _ => saida.push(Item::Atomico(f)),
            }
        }
    }

    fn palavras(&self, caixa: u32, t: &str, e: &Estilo, saida: &mut Vec<Item>) {
        let esc = self.ctx.escolha(e);
        let fontes = self.ctx.fontes;
        let espaco_l = fontes.medir(&esc, " ", e.espaco_letras) + e.espaco_palavras;
        let quebra = e.brancos.quebra_linha();
        let manter = e.brancos.mantem_espacos();
        let b = t.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'\n' && e.brancos.mantem_quebras() {
                saida.push(Item::Quebra(u32::MAX));
                i += 1;
                continue;
            }
            // palavra: ate espaco (ou CJK, que quebra em todo caractere)
            let ini = i;
            let mut fim = i;
            let resto = &t[i..];
            let mut cjk = false;
            for (off, ch) in resto.char_indices() {
                if ch == ' ' || (ch == '\n' && e.brancos.mantem_quebras()) {
                    break;
                }
                if eh_cjk(ch) && quebra {
                    if off == 0 {
                        fim = i + ch.len_utf8();
                        cjk = true;
                    }
                    break;
                }
                fim = i + off + ch.len_utf8();
                // quebra depois de hifen
                if ch == '-' && off > 0 && quebra {
                    break;
                }
            }
            let _ = cjk;
            if fim == ini {
                fim = ini;
            }
            let palavra = &t[ini..fim];
            let l = if palavra.is_empty() { 0 } else { fontes.medir(&esc, palavra, e.espaco_letras) };
            // espacos depois
            let mut j = fim;
            let mut espacos = 0;
            while j < b.len() && b[j] == b' ' {
                j += 1;
                espacos += 1;
            }
            let espaco = if espacos == 0 {
                0
            } else if manter {
                espaco_l * espacos
            } else {
                espaco_l
            };
            if !palavra.is_empty() || espaco > 0 {
                if !quebra && !saida.is_empty() {
                    // sem quebra: tudo junto numa palavra so (nowrap/pre)
                    if let Some(Item::Palavra { caixa: c2, fim: f2, l: l2, espaco: e2, .. }) = saida.last_mut() {
                        if *c2 == caixa && *f2 as usize <= ini {
                            *l2 += *e2 + l;
                            *f2 = fim as u32;
                            *e2 = espaco;
                            i = j;
                            continue;
                        }
                    }
                }
                saida.push(Item::Palavra {
                    caixa,
                    ini: ini as u32,
                    fim: fim as u32,
                    l,
                    espaco,
                    quebra_dentro: e.quebra_palavra,
                });
            }
            i = j.max(fim).max(i + 1);
        }
    }

    /// Lados (margem+borda+preenchimento) esquerdo e direito de uma caixa inline.
    fn lados_inline(&mut self, k: u32, cb_l: Fixo) -> (Fixo, Fixo) {
        self.lados(k, cb_l);
        let c = &self.arv.caixas[k as usize];
        (c.margem[3] + c.borda[3] + c.preench[3], c.margem[1] + c.borda[1] + c.preench[1])
    }

    /// Faz as linhas do conteudo inline de `k` na faixa (x, y, l).
    /// Devolve a altura usada.
    pub fn linhas(&mut self, k: u32, x: Fixo, y: Fixo, l: Fixo, fl: &mut Floats) -> Fixo {
        let mut itens = Vec::new();
        self.itens(k, &mut itens, l);
        let dono = self.arv.caixas[k as usize].estilo.clone();
        let esc_dono = self.ctx.escolha(&dono);
        let (strut_cima, strut_baixo) = vertical(self, &dono, &esc_dono);
        let recuo = dono.recuo.resolver(l);
        let mut linhas: Vec<Linha> = Vec::new();
        let mut cy = y;

        // estado da linha atual
        struct Pendente {
            itens: Vec<(usize, Fixo)>, // (indice do item, x relativo ao inicio da linha)
            largura: Fixo,
        }
        let mut atual = Pendente { itens: Vec::new(), largura: 0 };
        let mut abertos: Vec<u32> = Vec::new();
        let mut primeira = true;
        let altura_estimada = strut_cima + strut_baixo;
        let (mut le, mut ld) = fl.livre(cy, altura_estimada, x, x + l);
        let mut x_ini = le + if primeira { recuo } else { 0 };
        let mut limite = ld - x_ini;
        let mut so_espaco_no_fim: Fixo = 0;
        // larguras das palavras dependem do item: pre-calcula atomicos
        let mut larguras = alloc::vec![0 as Fixo; itens.len()];
        for (i, it) in itens.iter().enumerate() {
            match *it {
                Item::Atomico(a) => {
                    self.atomico(a, l);
                    let c = &self.arv.caixas[a as usize];
                    larguras[i] = c.l + c.margem[1] + c.margem[3];
                }
                Item::Abre(a) => {
                    larguras[i] = self.lados_inline(a, l).0;
                }
                Item::Fecha(a) => {
                    larguras[i] = self.lados_inline(a, l).1;
                }
                Item::Palavra { l: w, .. } => larguras[i] = w,
                _ => {}
            }
        }

        let mut i = 0;
        let mut quebrar_agora;
        while i < itens.len() {
            quebrar_agora = false;
            let it = itens[i].clone();
            match it {
                Item::Palavra { espaco, quebra_dentro, caixa, ini, fim, .. } => {
                    let w = larguras[i];
                    let vazio = atual.itens.iter().all(|(j, _)| matches!(itens[*j], Item::Abre(_) | Item::Fecha(_)));
                    if !vazio && atual.largura + w > limite + 1 {
                        quebrar_agora = true;
                    } else if vazio && w > limite && quebra_dentro && fim - ini > 1 {
                        // palavra maior que a linha: corta por caracteres
                        let e = self.arv.caixas[caixa as usize].estilo.clone();
                        let esc = self.ctx.escolha(&e);
                        let t = match &self.arv.caixas[caixa as usize].tipo {
                            Tipo::Texto(t) => t.clone(),
                            _ => unreachable!(),
                        };
                        let palavra = &t[ini as usize..fim as usize];
                        let mut acc = 0;
                        let mut corte = ini as usize;
                        for (off, ch) in palavra.char_indices() {
                            let cw = self.ctx.fontes.medir(&esc, &palavra[off..off + ch.len_utf8()], e.espaco_letras);
                            if acc + cw > limite - atual.largura && off > 0 {
                                break;
                            }
                            acc += cw;
                            corte = ini as usize + off + ch.len_utf8();
                        }
                        if corte < fim as usize && corte > ini as usize {
                            let resto_l = self.ctx.fontes.medir(&esc, &t[corte..fim as usize], e.espaco_letras);
                            itens[i] = Item::Palavra { caixa, ini, fim: corte as u32, l: acc, espaco: 0, quebra_dentro };
                            larguras[i] = acc;
                            itens.insert(i + 1, Item::Palavra { caixa, ini: corte as u32, fim, l: resto_l, espaco, quebra_dentro });
                            larguras.insert(i + 1, resto_l);
                            atual.itens.push((i, atual.largura));
                            atual.largura += acc;
                            so_espaco_no_fim = 0;
                            quebrar_agora = true;
                            i += 1;
                        }
                    }
                    if !quebrar_agora {
                        let pula = vazio && w == 0 && !self.arv.caixas[caixa as usize].estilo.brancos.mantem_espacos();
                        if !pula {
                            atual.itens.push((i, atual.largura));
                            atual.largura += w + espaco;
                            so_espaco_no_fim = espaco;
                        }
                        i += 1;
                    }
                }
                Item::Atomico(_) => {
                    let w = larguras[i];
                    let vazio = atual.itens.iter().all(|(j, _)| matches!(itens[*j], Item::Abre(_) | Item::Fecha(_)));
                    if !vazio && atual.largura + w > limite + 1 && dono.brancos.quebra_linha() {
                        quebrar_agora = true;
                    } else {
                        atual.itens.push((i, atual.largura));
                        atual.largura += w;
                        so_espaco_no_fim = 0;
                        i += 1;
                    }
                }
                Item::Abre(a) => {
                    abertos.push(a);
                    atual.itens.push((i, atual.largura));
                    atual.largura += larguras[i];
                    i += 1;
                }
                Item::Fecha(a) => {
                    abertos.retain(|&x| x != a);
                    atual.itens.push((i, atual.largura));
                    atual.largura += larguras[i];
                    i += 1;
                }
                Item::Quebra(_) => {
                    atual.itens.push((i, atual.largura));
                    i += 1;
                    quebrar_agora = true;
                    so_espaco_no_fim = 0;
                }
                Item::Float(f) => {
                    // float no meio do texto: vai para a linha atual (ou a proxima)
                    let topo = cy;
                    self.colocar_float(f, x, topo, l, None, fl);
                    let (e2, d2) = fl.livre(cy, altura_estimada, x, x + l);
                    le = e2;
                    ld = d2;
                    x_ini = le + if primeira { recuo } else { 0 };
                    limite = ld - x_ini;
                    i += 1;
                }
                Item::Absoluto(a) => {
                    atual.itens.push((i, atual.largura));
                    let _ = a;
                    i += 1;
                }
            }
            let fim_itens = i >= itens.len();
            if quebrar_agora || fim_itens {
                let tem_algo = !atual.itens.is_empty();
                if tem_algo || matches!(itens.get(i.wrapping_sub(1)), Some(Item::Quebra(_))) {
                    let ultima = fim_itens || matches!(itens.get(i.wrapping_sub(1)), Some(Item::Quebra(_)));
                    let linha = self.fechar_linha(
                        &itens,
                        &larguras,
                        &atual.itens,
                        x_ini,
                        cy,
                        limite,
                        atual.largura - so_espaco_no_fim,
                        &dono,
                        (strut_cima, strut_baixo),
                        ultima,
                        &abertos,
                    );
                    cy += linha.altura;
                    linhas.push(linha);
                }
                atual = Pendente { itens: Vec::new(), largura: 0 };
                // caixas inline ainda abertas continuam na proxima linha
                for &a in &abertos {
                    let _ = a;
                }
                so_espaco_no_fim = 0;
                primeira = false;
                let (e2, d2) = fl.livre(cy, altura_estimada, x, x + l);
                // linha sem espaco nenhum ao lado de floats: desce
                let (cy2, e3, d3) = if d2 - e2 < 8 * UM { fl.achar_lugar(cy, altura_estimada, 8 * UM, x, x + l) } else { (cy, e2, d2) };
                cy = cy2;
                le = e3;
                ld = d3;
                x_ini = le;
                limite = ld - x_ini;
            }
        }
        // base da caixa: a primeira linha
        if let Some(p) = linhas.first() {
            let c = &mut self.arv.caixas[k as usize];
            let topo_borda = c.y;
            c.base = Some(p.base - topo_borda);
        }
        let altura = cy - y;
        self.arv.caixas[k as usize].linhas = linhas;
        altura
    }

    /// Layout de um atomico (inline-block, imagem, controle) na origem;
    /// a linha move ele depois.
    fn atomico(&mut self, a: u32, cb_l: Fixo) {
        self.lados(a, cb_l);
        let tipo = self.arv.caixas[a as usize].tipo.clone();
        let (ml, mt) = (self.arv.caixas[a as usize].margem[3], self.arv.caixas[a as usize].margem[0]);
        match tipo {
            Tipo::Substituido(_) => {
                let (w, h) = self.tamanho_substituido(a, cb_l, None);
                let c = &mut self.arv.caixas[a as usize];
                c.x = ml;
                c.y = mt;
                c.l = w;
                c.a = h;
                c.base = None;
            }
            _ => {
                let w = self.largura_encolhida(a, cb_l);
                self.bloco_com_largura(a, ml, mt, w, cb_l, None);
            }
        }
        self.arv.caixas[a as usize].em_linha = true;
    }

    #[allow(clippy::too_many_arguments)]
    fn fechar_linha(
        &mut self,
        itens: &[Item],
        larguras: &[Fixo],
        lista: &[(usize, Fixo)],
        x_ini: Fixo,
        y: Fixo,
        limite: Fixo,
        usada: Fixo,
        dono: &Estilo,
        strut: (Fixo, Fixo),
        ultima: bool,
        abertos_no_fim: &[u32],
    ) -> Linha {
        // text-align
        let sobra = (limite - usada).max(0);
        let espacos = lista.iter().filter(|(j, _)| matches!(itens[*j], Item::Palavra { espaco, .. } if espaco > 0)).count();
        let (desloc, extra) = match dono.alinhamento {
            Alinhamento::Esquerda => (0, 0),
            Alinhamento::Direita => (sobra, 0),
            Alinhamento::Centro => (sobra / 2, 0),
            Alinhamento::Justificado if !ultima && espacos > 1 => (0, sobra / (espacos as Fixo - 1).max(1)),
            Alinhamento::Justificado => (0, 0),
        };
        let mut cima = strut.0;
        let mut baixo = strut.1;
        // primeira passada: medidas verticais de cada item (relativas a base)
        struct Colocado {
            j: usize,
            x: Fixo,
            sobe: Fixo, // deslocamento da base do item em relacao a base da linha (positivo = acima)
        }
        let mut colocados: Vec<Colocado> = Vec::with_capacity(lista.len());
        let mut extra_acc = 0;
        let mut abertos: Vec<(u32, Fixo)> = Vec::new();
        // caixas inline abertas na linha anterior continuam aqui
        let mut topo_extra: Vec<(u32, Fixo)> = Vec::new();
        for (j, xr) in lista {
            let x = x_ini + desloc + xr + extra_acc;
            match &itens[*j] {
                Item::Palavra { caixa, espaco, .. } => {
                    let e = self.arv.caixas[*caixa as usize].estilo.clone();
                    let esc = self.ctx.escolha(&e);
                    let (c, b) = vertical(self, &e, &esc);
                    let sobe = self.deslocamento_vertical(&e, &esc, dono);
                    cima = cima.max(c + sobe);
                    baixo = baixo.max(b - sobe);
                    colocados.push(Colocado { j: *j, x, sobe });
                    if *espaco > 0 {
                        extra_acc += extra;
                    }
                }
                Item::Atomico(a) => {
                    let c = &self.arv.caixas[*a as usize];
                    let altura_margem = c.a + c.margem[0] + c.margem[2];
                    let base = c.base.map(|b| b + c.margem[0]).unwrap_or(altura_margem);
                    let e = c.estilo.clone();
                    let (sobe, acima) = match e.alinhamento_vertical {
                        AlinhamentoVertical::Meio => {
                            let esc = self.ctx.escolha(dono);
                            let meio = self.ctx.fontes.altura_x(&esc) / 2;
                            (meio + altura_margem / 2 - base, altura_margem / 2 + meio)
                        }
                        AlinhamentoVertical::Topo | AlinhamentoVertical::TextoTopo => (0, base),
                        _ => {
                            let s = self.deslocamento_vertical(&e, &self.ctx.escolha(&e), dono);
                            (s, base + s)
                        }
                    };
                    cima = cima.max(acima);
                    baixo = baixo.max(altura_margem - base - sobe);
                    colocados.push(Colocado { j: *j, x, sobe });
                }
                Item::Abre(a) => {
                    abertos.push((*a, x + larguras[*j]));
                    colocados.push(Colocado { j: *j, x, sobe: 0 });
                }
                Item::Fecha(a) => {
                    colocados.push(Colocado { j: *j, x, sobe: 0 });
                    if let Some(p) = abertos.iter().position(|(c, _)| c == a) {
                        abertos.remove(p);
                    } else {
                        topo_extra.push((*a, x));
                    }
                }
                Item::Quebra(_) | Item::Absoluto(_) | Item::Float(_) => colocados.push(Colocado { j: *j, x, sobe: 0 }),
            }
        }
        let base = y + cima;
        let altura = cima + baixo;
        let mut linha = Linha { y, altura, base, ..Default::default() };
        // fundos das caixas inline: das aberturas ate os fechos (ou o fim)
        let mut inicio_de: Vec<(u32, Fixo, bool)> = Vec::new();
        // caixas que vieram abertas da linha anterior comecam no inicio
        for &a in abertos_no_fim {
            let fechada_aqui = lista.iter().any(|(j, _)| matches!(itens[*j], Item::Fecha(x) if x == a));
            let aberta_aqui = lista.iter().any(|(j, _)| matches!(itens[*j], Item::Abre(x) if x == a));
            if fechada_aqui && !aberta_aqui {
                inicio_de.push((a, x_ini + desloc, false));
            }
        }
        for c in &colocados {
            match &itens[c.j] {
                Item::Palavra { caixa, ini, fim, l, espaco, .. } => {
                    let e = self.arv.caixas[*caixa as usize].estilo.clone();
                    let esc = self.ctx.escolha(&e);
                    linha.trechos.push(Trecho {
                        caixa: *caixa,
                        ini: *ini,
                        fim: *fim,
                        x: c.x,
                        base: base - c.sobe,
                        largura: *l + if *espaco > 0 { *espaco } else { 0 },
                        escolha: esc,
                        extra_espaco: 0,
                    });
                }
                Item::Atomico(a) => {
                    let (mt, ml, base_c) = {
                        let cx = &self.arv.caixas[*a as usize];
                        let altura_margem = cx.a + cx.margem[0] + cx.margem[2];
                        (cx.margem[0], cx.margem[3], cx.base.map(|b| b + cx.margem[0]).unwrap_or(altura_margem))
                    };
                    let e = self.arv.caixas[*a as usize].estilo.clone();
                    let topo = match e.alinhamento_vertical {
                        AlinhamentoVertical::Topo | AlinhamentoVertical::TextoTopo => y,
                        AlinhamentoVertical::Baixo | AlinhamentoVertical::TextoBaixo => {
                            let cx = &self.arv.caixas[*a as usize];
                            y + altura - (cx.a + cx.margem[0] + cx.margem[2])
                        }
                        _ => base - c.sobe - base_c,
                    };
                    let (ax, ay) = (self.arv.caixas[*a as usize].x, self.arv.caixas[*a as usize].y);
                    self.deslocar(*a, c.x + ml - ax, topo + mt - ay);
                    linha.atomicos.push(*a);
                }
                Item::Abre(a) => inicio_de.push((*a, c.x + self.arv.caixas[*a as usize].margem[3], true)),
                Item::Fecha(a) => {
                    let x1 = c.x + larguras[c.j] - self.arv.caixas[*a as usize].margem[1];
                    if let Some(p) = inicio_de.iter().position(|(x, _, _)| x == a) {
                        let (_, x0, ini) = inicio_de.remove(p);
                        linha.fundos.push(self.fundo(*a, x0, x1, base, ini, true));
                    }
                }
                Item::Absoluto(a) => {
                    // posicao estatica: aqui na linha
                    self.marcar_estatico(*a, c.x, y);
                }
                _ => {}
            }
        }
        // caixas que continuam na proxima linha: fundo ate o fim desta
        let fim_x = x_ini + desloc + usada;
        for (a, x0, ini) in inicio_de {
            linha.fundos.push(self.fundo(a, x0, fim_x, base, ini, false));
        }
        // marcador externo de lista: sai para a esquerda
        for t in linha.trechos.iter_mut() {
            let pai = self.arv.caixas[t.caixa as usize].pai;
            if self.arv.caixas[pai as usize].no == MARCADOR_EXTERNO {
                t.x -= t.largura + 0;
                t.x -= 0;
            }
        }
        let _ = extra;
        // ajuste do marcador externo: as outras palavras voltam
        let marcadores: Vec<Fixo> = linha
            .trechos
            .iter()
            .filter(|t| self.arv.caixas[self.arv.caixas[t.caixa as usize].pai as usize].no == MARCADOR_EXTERNO)
            .map(|t| t.largura)
            .collect();
        if let Some(&w) = marcadores.first() {
            for t in linha.trechos.iter_mut() {
                let pai = self.arv.caixas[t.caixa as usize].pai;
                if self.arv.caixas[pai as usize].no != MARCADOR_EXTERNO {
                    t.x -= w;
                }
            }
            for &a in &linha.atomicos {
                self.deslocar(a, -w, 0);
            }
            for f in linha.fundos.iter_mut() {
                f.x0 -= w;
                f.x1 -= w;
            }
        }
        linha
    }

    fn fundo(&self, a: u32, x0: Fixo, x1: Fixo, base: Fixo, inicio: bool, fim: bool) -> FundoInline {
        let c = &self.arv.caixas[a as usize];
        let esc = self.ctx.escolha(&c.estilo);
        let (asc, desc) = self.ctx.fontes.metricas(&esc);
        FundoInline {
            caixa: a,
            x0,
            x1,
            topo: base - asc - c.preench[0] - c.borda[0],
            baixo: base + desc + c.preench[2] + c.borda[2],
            inicio,
            fim,
        }
    }

    /// Quanto a base de um item sobe (vertical-align sub/super/medida).
    fn deslocamento_vertical(&self, e: &Estilo, esc: &Escolha, dono: &Estilo) -> Fixo {
        match e.alinhamento_vertical {
            AlinhamentoVertical::Sub => -(dono.tamanho_fixo() / 5),
            AlinhamentoVertical::Super => dono.tamanho_fixo() / 3,
            AlinhamentoVertical::M(m) => m.resolver(e.linha()),
            AlinhamentoVertical::Meio => {
                let _ = esc;
                0
            }
            _ => 0,
        }
    }

    fn marcar_estatico(&mut self, a: u32, x: Fixo, y: Fixo) {
        self.absolutos_estatico(a, x, y);
    }
}

#[allow(unused)]
fn _nao(_: Brancos) {}
