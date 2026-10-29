//! Layout (CSS 2.1 cap. 8-10, Flexbox, Grid, tabelas), em ponto fixo:
//! cada caixa recebe x, y, largura e altura da borda externa (coordenadas
//! da pagina), mais margens, bordas e preenchimentos ja resolvidos.
//!
//! Organizacao:
//! - caixa.rs: DOM + estilos -> arvore de caixas;
//! - bloco (aqui): largura/altura de blocos, margens que colapsam, floats,
//!   posicao relativa/absoluta/fixa;
//! - inline.rs: texto em linhas (quebra, alinhamento, linha de base);
//! - intrinseco.rs: larguras minima e maxima de conteudo;
//! - flex.rs, grade.rs, tabela.rs: os outros contextos de formatacao.

pub mod caixa;
pub mod flex;
pub mod grade;
pub mod inline;
pub mod intrinseco;
pub mod tabela;

use alloc::vec::Vec;

use crate::web::css::estilo::{Estilo, Lado, Posicao, Tam, Transbordo};
use crate::web::css::valores::{Fixo, Medida, UM};
use crate::web::dom::Id;
use crate::web::fonte::{Escolha, Fontes};
use caixa::{Arvore, Substituido, Tipo};

/// O que o layout precisa do mundo de fora.
pub struct Contexto<'a> {
    pub fontes: &'a Fontes,
    pub tela_l: Fixo,
    pub tela_a: Fixo,
    /// Tamanho natural (px) de uma imagem/objeto do elemento, se ja chegou.
    pub imagens: &'a dyn Fn(Id) -> Option<(i32, i32)>,
    /// O documento (atributos como colspan/rowspan).
    pub doc: Option<&'a crate::web::dom::Documento>,
}

impl Contexto<'_> {
    pub fn escolha(&self, e: &Estilo) -> Escolha {
        self.fontes.escolher(e.familia, e.peso, e.italico, e.tamanho_fonte)
    }
}

/// Um float ja colocado (borda externa da margem).
#[derive(Clone, Copy, Debug)]
pub struct Float {
    pub esquerda: bool,
    pub x: Fixo,
    pub y: Fixo,
    pub l: Fixo,
    pub a: Fixo,
}

/// Os floats de um contexto de formatacao de bloco.
#[derive(Default, Clone)]
pub struct Floats {
    pub lista: Vec<Float>,
}

impl Floats {
    /// Faixa livre [esq, dir) para uma linha em [y, y+a) dentro de [x0, x1).
    pub fn livre(&self, y: Fixo, a: Fixo, x0: Fixo, x1: Fixo) -> (Fixo, Fixo) {
        let mut e = x0;
        let mut d = x1;
        let a = a.max(1);
        for f in &self.lista {
            if f.y < y + a && f.y + f.a > y {
                if f.esquerda {
                    e = e.max(f.x + f.l);
                } else {
                    d = d.min(f.x);
                }
            }
        }
        (e, d.max(e))
    }

    /// Menor y >= `y` onde cabe `l` de largura (descendo pelos fins dos floats).
    pub fn achar_lugar(&self, y: Fixo, a: Fixo, l: Fixo, x0: Fixo, x1: Fixo) -> (Fixo, Fixo, Fixo) {
        let mut y = y;
        for _ in 0..64 {
            let (e, d) = self.livre(y, a, x0, x1);
            if d - e >= l || !self.lista.iter().any(|f| f.y + f.a > y && f.y < y + a.max(1)) {
                return (y, e, d);
            }
            // desce ate o proximo fim de float
            let prox = self.lista.iter().filter(|f| f.y + f.a > y).map(|f| f.y + f.a).min();
            match prox {
                Some(p) => y = p,
                None => break,
            }
        }
        let (e, d) = self.livre(y, a, x0, x1);
        (y, e, d)
    }

    /// y abaixo dos floats do lado pedido (clear).
    pub fn abaixo(&self, lado: Lado, y: Fixo) -> Fixo {
        let mut r = y;
        for f in &self.lista {
            let pega = match lado {
                Lado::Esquerda => f.esquerda,
                Lado::Direita => !f.esquerda,
                Lado::Ambos => true,
                Lado::Nenhum => false,
            };
            if pega {
                r = r.max(f.y + f.a);
            }
        }
        r
    }

    pub fn fundo(&self) -> Fixo {
        self.lista.iter().map(|f| f.y + f.a).max().unwrap_or(0)
    }
}

/// Combina duas margens que colapsam (maior positiva + menor negativa).
pub fn colapsar(a: Fixo, b: Fixo) -> Fixo {
    let pos = a.max(0).max(b.max(0));
    let neg = a.min(0).min(b.min(0));
    pos + neg
}

fn medida(m: Medida, base: Fixo) -> Fixo {
    m.resolver(base)
}

fn tam(t: Tam, base: Option<Fixo>) -> Option<Fixo> {
    match t {
        Tam::M(m) if m.tem_pct() => base.map(|b| m.resolver(b)),
        Tam::M(m) => Some(m.px),
        _ => None,
    }
}

/// O motor de layout de uma arvore de caixas.
pub struct Motor<'a, 'c> {
    pub arv: &'a mut Arvore,
    pub ctx: &'a Contexto<'c>,
    /// Caixas absolutas/fixas esperando o bloco que as contem terminar:
    /// (caixa, x estatico, y estatico).
    absolutos: Vec<(u32, Fixo, Fixo)>,
    pub intrinsecas: Vec<Option<(Fixo, Fixo)>>,
}

impl<'a, 'c> Motor<'a, 'c> {
    pub fn novo(arv: &'a mut Arvore, ctx: &'a Contexto<'c>) -> Self {
        let n = arv.caixas.len();
        Motor { arv, ctx, absolutos: Vec::new(), intrinsecas: alloc::vec![None; n] }
    }

    pub fn estilo(&self, k: u32) -> &Estilo {
        &self.arv.caixas[k as usize].estilo
    }

    /// Resolve bordas, preenchimentos e margens (auto = 0 por enquanto).
    pub fn lados(&mut self, k: u32, cb_l: Fixo) {
        let c = &mut self.arv.caixas[k as usize];
        let e = &c.estilo;
        for i in 0..4 {
            c.borda[i] = e.borda(i);
            c.preench[i] = medida(e.preenchimento[i], cb_l).max(0);
            c.margem[i] = tam(e.margem[i], Some(cb_l)).unwrap_or(0);
        }
        // celulas de tabela: margem nao existe
        if matches!(c.tipo, Tipo::Celula | Tipo::LinhaTabela | Tipo::GrupoLinhas) {
            c.margem = [0; 4];
        }
    }

    pub fn lados_h(&self, k: u32) -> Fixo {
        let c = &self.arv.caixas[k as usize];
        c.borda[1] + c.borda[3] + c.preench[1] + c.preench[3]
    }

    pub fn lados_v(&self, k: u32) -> Fixo {
        let c = &self.arv.caixas[k as usize];
        c.borda[0] + c.borda[2] + c.preench[0] + c.preench[2]
    }

    /// Largura especificada (borda externa), se houver.
    pub fn largura_especificada(&self, k: u32, cb_l: Option<Fixo>) -> Option<Fixo> {
        let e = self.estilo(k);
        let w = tam(e.largura, cb_l)?;
        Some(if e.caixa_borda { w } else { w + self.lados_h(k) })
    }

    pub fn altura_especificada(&self, k: u32, cb_a: Option<Fixo>) -> Option<Fixo> {
        let e = self.estilo(k);
        let h = tam(e.altura, cb_a)?;
        Some(if e.caixa_borda { h } else { h + self.lados_v(k) })
    }

    /// Aplica min/max-width a uma largura de borda externa.
    pub fn limitar_largura(&self, k: u32, w: Fixo, cb_l: Option<Fixo>) -> Fixo {
        let e = self.estilo(k);
        let extra = if e.caixa_borda { 0 } else { self.lados_h(k) };
        let mut w = w;
        if let Some(mx) = tam(e.max_largura, cb_l) {
            w = w.min(mx + extra);
        }
        if let Some(mn) = tam(e.min_largura, cb_l) {
            w = w.max(mn + extra);
        }
        w.max(self.lados_h(k))
    }

    pub fn limitar_altura(&self, k: u32, h: Fixo, cb_a: Option<Fixo>) -> Fixo {
        let e = self.estilo(k);
        let extra = if e.caixa_borda { 0 } else { self.lados_v(k) };
        let mut h = h;
        if let Some(mx) = tam(e.max_altura, cb_a) {
            h = h.min(mx + extra);
        }
        if let Some(mn) = tam(e.min_altura, cb_a) {
            h = h.max(mn + extra);
        }
        h.max(self.lados_v(k))
    }

    /// Cria contexto de formatacao de bloco proprio?
    pub fn novo_bfc(&self, k: u32) -> bool {
        let c = &self.arv.caixas[k as usize];
        let e = &c.estilo;
        k == 0
            || e.flutuar != Lado::Nenhum
            || e.fora_do_fluxo()
            || !matches!(e.transbordo[0], Transbordo::Visivel | Transbordo::Cortar)
            || !matches!(c.tipo, Tipo::Bloco)
            || matches!(e.exibir, crate::web::css::estilo::Exibir::FlowRoot)
            || e.exibir.eh_inline()
    }

    // ------------------------------------------------------------- inicio

    /// Faz o layout da pagina inteira para a largura da tela.
    pub fn fazer(&mut self) {
        let tela_l = self.ctx.tela_l;
        let tela_a = self.ctx.tela_a;
        let mut fl = Floats::default();
        self.lados(0, tela_l);
        let c = &mut self.arv.caixas[0];
        c.x = 0;
        c.y = 0;
        c.l = tela_l;
        let raiz_filhos = c.filhos.clone();
        let mut y = 0;
        let mut margem = 0;
        for f in raiz_filhos {
            let (h, _) = self.bloco_no_fluxo(f, 0, y, tela_l, Some(tela_a), &mut fl, &mut margem, true);
            y += h;
        }
        y += margem.max(0);
        let fim = y.max(fl.fundo());
        self.arv.caixas[0].a = fim.max(tela_a);
        self.processar_absolutos();
    }

    // ------------------------------------------------------- blocos

    /// Coloca um filho de bloco a partir de (x, y) (y = onde terminou o
    /// irmao anterior, sem a margem pendente). Devolve (avanco em y,
    /// margem de baixo pendente). `margem` entra com a pendente e sai com
    /// a nova.
    #[allow(clippy::too_many_arguments)]
    pub fn bloco_no_fluxo(
        &mut self,
        k: u32,
        x: Fixo,
        y: Fixo,
        cb_l: Fixo,
        cb_a: Option<Fixo>,
        fl: &mut Floats,
        margem: &mut Fixo,
        _primeiro: bool,
    ) -> (Fixo, Fixo) {
        let e = self.arv.caixas[k as usize].estilo.clone();
        if e.fora_do_fluxo() {
            // posicao estatica: onde estaria
            self.absolutos.push((k, x, y + (*margem).max(0)));
            return (0, 0);
        }
        if e.flutuar != Lado::Nenhum {
            self.colocar_float(k, x, y + (*margem).max(0), cb_l, cb_a, fl);
            return (0, 0);
        }
        self.lados(k, cb_l);
        let mt = self.arv.caixas[k as usize].margem[0];
        let mut y_borda = y + colapsar(*margem, mt);
        // clear: abaixo dos floats
        if e.limpar != Lado::Nenhum {
            let abaixo = fl.abaixo(e.limpar, y_borda);
            if abaixo > y_borda {
                y_borda = abaixo;
            }
        }
        let novo_bfc = self.novo_bfc(k);
        let (bx, bl) = if novo_bfc && !fl.lista.is_empty() && !matches!(self.arv.caixas[k as usize].tipo, Tipo::Tabela) || novo_bfc && !fl.lista.is_empty() {
            // BFC ao lado de floats: estreita
            let (e0, d0) = fl.livre(y_borda, UM, x, x + cb_l);
            (e0, d0 - e0)
        } else {
            (x, cb_l)
        };
        self.bloco(k, bx, y_borda, bl, cb_a, fl);
        let c = &self.arv.caixas[k as usize];
        let mb = c.margem[2];
        let h = c.a;
        let y_borda = c.y;
        *margem = mb;
        // posicao relativa (depois de tudo pronto)
        if e.posicao == Posicao::Relativa || e.posicao == Posicao::Grudada {
            self.relativa(k, cb_l, cb_a);
        }
        (y_borda - y + h, mb)
    }

    /// Layout de um bloco com a borda de cima em (x+margem esq, y).
    /// `x`/`cb_l`: a faixa do bloco que contem.
    pub fn bloco(&mut self, k: u32, x: Fixo, y: Fixo, cb_l: Fixo, cb_a: Option<Fixo>, fl_fora: &mut Floats) {
        self.lados(k, cb_l);
        let e = self.arv.caixas[k as usize].estilo.clone();
        // largura
        let ml_auto = e.margem[3].eh_auto();
        let mr_auto = e.margem[1].eh_auto();
        let (ml, mr) = (self.arv.caixas[k as usize].margem[3], self.arv.caixas[k as usize].margem[1]);
        let tipo = self.arv.caixas[k as usize].tipo.clone();
        let especificada = self.largura_especificada(k, Some(cb_l));
        let encolher = matches!(tipo, Tipo::Tabela) || matches!(e.largura, Tam::MinConteudo | Tam::MaxConteudo | Tam::AjustarConteudo);
        let mut w = match especificada {
            Some(w) => w,
            None if encolher => {
                let (mn, mx) = self.intrinseca(k);
                match e.largura {
                    Tam::MinConteudo => mn,
                    Tam::MaxConteudo => mx,
                    _ => mx.min((cb_l - ml - mr).max(mn)),
                }
            }
            None => cb_l - ml - mr,
        };
        w = self.limitar_largura(k, w, Some(cb_l));
        let (ml, mr) = if especificada.is_some() || encolher || w < cb_l - ml - mr {
            let sobra = cb_l - w - ml - mr;
            match (ml_auto, mr_auto) {
                (true, true) => (sobra.max(0) / 2 + ml, sobra.max(0) - sobra.max(0) / 2 + mr),
                (true, false) => (ml + sobra, mr),
                (false, true) => (ml, mr + sobra),
                _ => (ml, mr),
            }
        } else {
            (ml, mr)
        };
        {
            let c = &mut self.arv.caixas[k as usize];
            c.margem[3] = ml;
            c.margem[1] = mr;
            c.x = x + ml;
            c.y = y;
            c.l = w;
        }
        let cb_a_proprio = self.altura_especificada(k, cb_a).map(|h| h - self.lados_v(k));
        let conteudo_x = self.arv.caixas[k as usize].conteudo_x();
        let conteudo_y = self.arv.caixas[k as usize].conteudo_y();
        let conteudo_l = self.arv.caixas[k as usize].conteudo_l();

        let altura_conteudo = match tipo {
            Tipo::Flex => self.flex(k, conteudo_x, conteudo_y, conteudo_l, cb_a_proprio),
            Tipo::Grid => self.grade(k, conteudo_x, conteudo_y, conteudo_l, cb_a_proprio),
            Tipo::Tabela => self.tabela(k, conteudo_x, conteudo_y, conteudo_l, cb_a_proprio),
            Tipo::Substituido(_) => self.substituido_altura(k, conteudo_l),
            _ => {
                let mut proprio = Floats::default();
                let novo = self.novo_bfc(k);
                let fl: &mut Floats = if novo { &mut proprio } else { fl_fora };
                let h = self.conteudo_bloco(k, conteudo_x, conteudo_y, conteudo_l, cb_a_proprio, fl);
                if novo {
                    // BFC inclui os floats na altura
                    h.max(proprio.fundo() - conteudo_y)
                } else {
                    h
                }
            }
        };
        let h = match self.altura_especificada(k, cb_a) {
            Some(h) => h,
            None => {
                let mut h = altura_conteudo + self.lados_v(k);
                if let (Some(r), None) = (e.proporcao, tam(e.altura, cb_a)) {
                    if especificada.is_some() || r > 0.0 {
                        h = h.max(((w as f32) / r) as Fixo);
                    }
                }
                h
            }
        };
        let h = self.limitar_altura(k, h, cb_a);
        self.arv.caixas[k as usize].a = h;
        if self.arv.caixas[k as usize].base.is_none() {
            // sem linhas: base e o fundo da caixa (para inline-block)
        }
        if e.posicao != Posicao::Estatica && !e.fora_do_fluxo() {
            // sou bloco contedor de absolutos: resolve os meus agora
            self.processar_absolutos_de(k);
        }
    }

    /// Conteudo de um bloco comum: so blocos ou so inline.
    fn conteudo_bloco(&mut self, k: u32, x: Fixo, y: Fixo, l: Fixo, cb_a: Option<Fixo>, fl: &mut Floats) -> Fixo {
        let filhos = self.arv.caixas[k as usize].filhos.clone();
        if filhos.is_empty() {
            return 0;
        }
        let tem_inline = filhos.iter().any(|&f| self.arv.caixas[f as usize].eh_inline());
        if tem_inline {
            return self.linhas(k, x, y, l, fl);
        }
        let mut cy = y;
        let mut margem = 0;
        // margem de cima do primeiro filho colapsa com a minha se nao ha
        // borda/preenchimento (e nao sou BFC): aproximacao, ela fica dentro
        let colapsa_topo = self.arv.caixas[k as usize].borda[0] == 0 && self.arv.caixas[k as usize].preench[0] == 0 && !self.novo_bfc(k);
        let mut primeiro = true;
        for f in filhos {
            let (dy, _) = self.bloco_no_fluxo(f, x, cy, l, cb_a, fl, &mut margem, primeiro);
            if primeiro && colapsa_topo && dy > 0 {
                // o primeiro filho em fluxo: sua margem de cima "vaza" para fora
                let mt = self.arv.caixas[f as usize].margem[0];
                if mt > 0 && !self.arv.caixas[f as usize].estilo.fora_do_fluxo() && self.arv.caixas[f as usize].estilo.flutuar == Lado::Nenhum {
                    let minha = self.arv.caixas[k as usize].margem[0];
                    let nova = colapsar(minha, mt);
                    let empurrar = nova - minha;
                    // sobe o filho (a margem dele passa a ser a minha)
                    self.deslocar(f, 0, -mt);
                    self.arv.caixas[k as usize].margem[0] = nova;
                    self.arv.caixas[k as usize].y += empurrar;
                    self.deslocar_filhos_ate(k, f, empurrar);
                    cy += empurrar - mt;
                }
            }
            if dy > 0 || !self.arv.caixas[f as usize].estilo.fora_do_fluxo() && self.arv.caixas[f as usize].estilo.flutuar == Lado::Nenhum {
                primeiro = false;
            }
            cy += dy;
        }
        // margem de baixo do ultimo: fica dentro se tenho borda/preenchimento
        let colapsa_baixo = self.arv.caixas[k as usize].borda[2] == 0
            && self.arv.caixas[k as usize].preench[2] == 0
            && !self.novo_bfc(k)
            && self.estilo(k).altura.eh_auto();
        if colapsa_baixo {
            let minha = self.arv.caixas[k as usize].margem[2];
            self.arv.caixas[k as usize].margem[2] = colapsar(minha, margem);
        } else {
            cy += margem.max(0);
        }
        cy - y
    }

    /// Move uma subarvore inteira.
    pub fn deslocar(&mut self, k: u32, dx: Fixo, dy: Fixo) {
        if dx == 0 && dy == 0 {
            return;
        }
        let c = &mut self.arv.caixas[k as usize];
        c.x += dx;
        c.y += dy;
        for l in c.linhas.iter_mut() {
            l.deslocar(dx, dy);
        }
        let filhos = c.filhos.clone();
        for f in filhos {
            self.deslocar(f, dx, dy);
        }
    }

    /// Ajuste depois de mover o pai: os irmaos anteriores a `ate` (que ja
    /// estavam no lugar) acompanham.
    fn deslocar_filhos_ate(&mut self, k: u32, ate: u32, dy: Fixo) {
        let filhos = self.arv.caixas[k as usize].filhos.clone();
        for f in filhos {
            self.deslocar(f, 0, dy);
            if f == ate {
                break;
            }
        }
    }

    pub fn absolutos_estatico(&mut self, k: u32, x: Fixo, y: Fixo) {
        self.absolutos.push((k, x, y));
    }

    pub fn relativa_pub(&mut self, k: u32, cb_l: Fixo, cb_a: Option<Fixo>) {
        self.relativa(k, cb_l, cb_a)
    }

    fn relativa(&mut self, k: u32, cb_l: Fixo, cb_a: Option<Fixo>) {
        let e = self.estilo(k);
        let d = e.deslocamento;
        let dx = match (tam(d[3], Some(cb_l)), tam(d[1], Some(cb_l))) {
            (Some(l), _) => l,
            (None, Some(r)) => -r,
            _ => 0,
        };
        let dy = match (tam(d[0], cb_a), tam(d[2], cb_a)) {
            (Some(t), _) => t,
            (None, Some(b)) => -b,
            _ => 0,
        };
        if e.posicao == Posicao::Grudada {
            return;
        }
        self.deslocar(k, dx, dy);
    }

    /// Float: largura encolhida, colocado no primeiro lugar livre.
    pub fn colocar_float(&mut self, k: u32, x: Fixo, y: Fixo, cb_l: Fixo, cb_a: Option<Fixo>, fl: &mut Floats) {
        self.lados(k, cb_l);
        let w = self.largura_encolhida(k, cb_l);
        let esquerda = self.estilo(k).flutuar == Lado::Esquerda;
        let lim = self.estilo(k).limpar;
        let c = &self.arv.caixas[k as usize];
        let (ml, mr, mt, mb) = (c.margem[3], c.margem[1], c.margem[0], c.margem[2]);
        let caixa_l = w + ml + mr;
        let y0 = if lim != Lado::Nenhum { fl.abaixo(lim, y) } else { y };
        // floats nao sobem acima de floats anteriores
        let y0 = fl.lista.last().map_or(y0, |f| y0.max(f.y));
        let (fy, e, d) = fl.achar_lugar(y0, UM, caixa_l, x, x + cb_l);
        let fx = if esquerda { e } else { d - caixa_l };
        self.bloco_com_largura(k, fx + ml, fy + mt, w, cb_l, cb_a);
        let h = self.arv.caixas[k as usize].a;
        fl.lista.push(Float { esquerda, x: fx, y: fy, l: caixa_l, a: h + mt + mb });
        if matches!(self.estilo(k).posicao, Posicao::Relativa) {
            self.relativa(k, cb_l, cb_a);
        }
    }

    /// Largura "encolher para caber" (floats, inline-block, absolutos).
    pub fn largura_encolhida(&mut self, k: u32, disponivel: Fixo) -> Fixo {
        if let Some(w) = self.largura_especificada(k, Some(disponivel)) {
            return self.limitar_largura(k, w, Some(disponivel));
        }
        let c = &self.arv.caixas[k as usize];
        let margens = c.margem[1] + c.margem[3];
        let (mn, mx) = self.intrinseca(k);
        let w = mx.min((disponivel - margens).max(mn));
        self.limitar_largura(k, w, Some(disponivel))
    }

    /// Layout de um bloco com largura ja decidida, borda em (x, y).
    pub fn bloco_com_largura(&mut self, k: u32, x: Fixo, y: Fixo, w: Fixo, cb_l: Fixo, cb_a: Option<Fixo>) {
        // truque: finge um bloco contedor do tamanho exato e margens 0
        let salvo = self.arv.caixas[k as usize].estilo.clone();
        let mut e = (*salvo).clone();
        e.largura = Tam::M(Medida::px(w));
        e.caixa_borda = true;
        e.margem = [Tam::ZERO; 4];
        e.min_largura = Tam::Auto;
        e.max_largura = Tam::Nenhum;
        self.arv.caixas[k as usize].estilo = alloc::rc::Rc::new(e);
        let mut fl = Floats::default();
        self.bloco(k, x, y, w, cb_a, &mut fl);
        let margens = {
            // as margens de verdade (para quem perguntar depois)
            let c = &self.arv.caixas[k as usize];
            c.margem
        };
        self.arv.caixas[k as usize].estilo = salvo;
        let _ = (cb_l, margens);
        self.lados_margens(k, cb_l);
    }

    fn lados_margens(&mut self, k: u32, cb_l: Fixo) {
        let c = &mut self.arv.caixas[k as usize];
        for i in 0..4 {
            c.margem[i] = tam(c.estilo.margem[i], Some(cb_l)).unwrap_or(0);
        }
    }

    // ------------------------------------------------- substituidos

    /// Tamanho natural (px de ponto fixo) de um elemento substituido.
    pub fn tamanho_natural(&self, k: u32) -> (Fixo, Fixo) {
        let c = &self.arv.caixas[k as usize];
        let Tipo::Substituido(s) = &c.tipo else { return (0, 0) };
        let e = &c.estilo;
        let linha = e.linha();
        match s {
            Substituido::Imagem { alt } => match (self.ctx.imagens)(c.no) {
                Some((l, a)) => (l * UM, a * UM),
                None => {
                    if alt.is_empty() {
                        (0, 0)
                    } else {
                        let esc = self.ctx.escolha(e);
                        (self.ctx.fontes.medir(&esc, alt, 0), linha)
                    }
                }
            },
            Substituido::Entrada { texto, marcavel, .. } => {
                if *marcavel {
                    (13 * UM, 13 * UM)
                } else {
                    let esc = self.ctx.escolha(e);
                    (self.ctx.fontes.medir(&esc, texto, 0).max(20 * UM), linha)
                }
            }
            Substituido::Botao => (60 * UM, linha),
            Substituido::Selecao { texto } => {
                let esc = self.ctx.escolha(e);
                (self.ctx.fontes.medir(&esc, texto, 0) + 20 * UM, linha)
            }
            Substituido::AreaTexto { .. } => (180 * UM, 36 * UM),
            Substituido::Svg | Substituido::Canvas | Substituido::Quadro | Substituido::Video => {
                match (self.ctx.imagens)(c.no) {
                    Some((l, a)) => (l * UM, a * UM),
                    None => (300 * UM, 150 * UM),
                }
            }
        }
    }

    /// Largura e altura usadas de um substituido (com proporcao).
    pub fn tamanho_substituido(&self, k: u32, cb_l: Fixo, cb_a: Option<Fixo>) -> (Fixo, Fixo) {
        let (nl, na) = self.tamanho_natural(k);
        let e = self.estilo(k);
        let lados_h = self.lados_h(k);
        let lados_v = self.lados_v(k);
        let wl = tam(e.largura, Some(cb_l)).map(|w| if e.caixa_borda { w - lados_h } else { w });
        let ha = tam(e.altura, cb_a).map(|h| if e.caixa_borda { h - lados_v } else { h });
        let proporcao = e.proporcao.or_else(|| if na > 0 { Some(nl as f32 / na as f32) } else { None });
        let (w, h) = match (wl, ha) {
            (Some(w), Some(h)) => (w, h),
            (Some(w), None) => (w, proporcao.map_or(na, |r| (w as f32 / r) as Fixo)),
            (None, Some(h)) => (proporcao.map_or(nl, |r| (h as f32 * r) as Fixo), h),
            (None, None) => (nl, na),
        };
        // max-width: 100% e afins
        let extra_h = if e.caixa_borda { 0 } else { lados_h };
        let mut w2 = w + lados_h;
        let mut h2 = h + lados_v;
        if let Some(mx) = tam(e.max_largura, Some(cb_l)) {
            if w2 > mx + extra_h {
                let novo = mx + extra_h;
                if ha.is_none() {
                    if let Some(r) = proporcao {
                        h2 = ((novo - lados_h) as f32 / r) as Fixo + lados_v;
                    }
                }
                w2 = novo;
            }
        }
        w2 = self.limitar_largura(k, w2, Some(cb_l));
        h2 = self.limitar_altura(k, h2, cb_a);
        (w2.max(lados_h), h2.max(lados_v))
    }

    fn substituido_altura(&mut self, k: u32, conteudo_l: Fixo) -> Fixo {
        let (_, h) = self.tamanho_substituido(k, conteudo_l + self.lados_h(k), None);
        h - self.lados_v(k)
    }

    // ---------------------------------------------- absolutos e fixos

    fn bloco_contedor(&self, k: u32) -> (Fixo, Fixo, Fixo, Fixo) {
        let fixo = self.estilo(k).posicao == Posicao::Fixa;
        if !fixo {
            let mut p = self.arv.caixas[k as usize].pai;
            while p != u32::MAX {
                let c = &self.arv.caixas[p as usize];
                if c.estilo.posicao != Posicao::Estatica || p == 0 {
                    if p == 0 {
                        break;
                    }
                    let x = c.x + c.borda[3];
                    let y = c.y + c.borda[0];
                    return (x, y, c.l - c.borda[1] - c.borda[3], c.a - c.borda[0] - c.borda[2]);
                }
                p = c.pai;
            }
        }
        (0, 0, self.ctx.tela_l, self.ctx.tela_a)
    }

    fn processar_absolutos_de(&mut self, dono: u32) {
        // os absolutos cujo bloco contedor e `dono` (ja pronto)
        let mut i = 0;
        while i < self.absolutos.len() {
            let (k, _, _) = self.absolutos[i];
            let mut p = self.arv.caixas[k as usize].pai;
            let mut achou = false;
            while p != u32::MAX {
                if self.arv.caixas[p as usize].estilo.posicao != Posicao::Estatica {
                    achou = p == dono;
                    break;
                }
                p = self.arv.caixas[p as usize].pai;
            }
            if achou && self.estilo(k).posicao != Posicao::Fixa {
                let (k, sx, sy) = self.absolutos.remove(i);
                self.absoluto(k, sx, sy);
            } else {
                i += 1;
            }
        }
    }

    fn processar_absolutos(&mut self) {
        let mut voltas = 0;
        while !self.absolutos.is_empty() && voltas < 100_000 {
            let (k, sx, sy) = self.absolutos.remove(0);
            self.absoluto(k, sx, sy);
            voltas += 1;
        }
    }

    fn absoluto(&mut self, k: u32, sx: Fixo, sy: Fixo) {
        let (cx, cy, cl, ca) = self.bloco_contedor(k);
        self.lados(k, cl);
        let e = self.arv.caixas[k as usize].estilo.clone();
        let d = e.deslocamento;
        let esq = tam(d[3], Some(cl));
        let dir = tam(d[1], Some(cl));
        let topo = tam(d[0], Some(ca));
        let baixo = tam(d[2], Some(ca));
        let c = &self.arv.caixas[k as usize];
        let (ml, mr, mt, mb) = (c.margem[3], c.margem[1], c.margem[0], c.margem[2]);
        let w = match (self.largura_especificada(k, Some(cl)), esq, dir) {
            (Some(w), _, _) => self.limitar_largura(k, w, Some(cl)),
            (None, Some(l), Some(r)) if !matches!(self.arv.caixas[k as usize].tipo, Tipo::Substituido(_)) => {
                self.limitar_largura(k, cl - l - r - ml - mr, Some(cl))
            }
            _ => {
                if let Tipo::Substituido(_) = self.arv.caixas[k as usize].tipo {
                    self.tamanho_substituido(k, cl, Some(ca)).0
                } else {
                    let disp = match (esq, dir) {
                        (Some(l), _) => cl - l,
                        (_, Some(r)) => cl - r,
                        _ => cl - (sx - cx),
                    };
                    self.largura_encolhida(k, disp.max(0))
                }
            }
        };
        let x = match (esq, dir) {
            (Some(l), _) => {
                if let (Some(r), true, true) = (dir, e.margem[3].eh_auto(), e.margem[1].eh_auto()) {
                    cx + l + (cl - l - r - w).max(0) / 2
                } else {
                    cx + l + ml
                }
            }
            (None, Some(r)) => cx + cl - r - mr - w,
            (None, None) => sx + ml,
        };
        // altura: layout primeiro (no topo provisorio), depois ajusta
        let y_prov = match topo {
            Some(t) => cy + t + mt,
            None => sy + mt,
        };
        let cb_a = Some(ca);
        self.bloco_com_largura(k, x, y_prov, w, cl, cb_a);
        let mut h = self.arv.caixas[k as usize].a;
        if let (Some(t), Some(b), None) = (topo, baixo, tam(e.altura, cb_a)) {
            // top e bottom: estica
            let alvo = ca - t - b - mt - mb;
            if alvo > h {
                h = self.limitar_altura(k, alvo, cb_a);
                self.arv.caixas[k as usize].a = h;
            }
        }
        if let (None, Some(b)) = (topo, baixo) {
            let y = cy + ca - b - mb - h;
            let dy = y - self.arv.caixas[k as usize].y;
            self.deslocar(k, 0, dy);
        } else if let (Some(t), Some(b), true, true) = (topo, baixo, e.margem[0].eh_auto(), e.margem[2].eh_auto()) {
            let y = cy + t + (ca - t - b - h).max(0) / 2;
            let dy = y - self.arv.caixas[k as usize].y;
            self.deslocar(k, 0, dy);
        }
        // os absolutos de dentro dele
        self.processar_absolutos_de(k);
    }
}


