//! Pinta a arvore de caixas num Quadro, na ordem do CSS (simplificada do
//! apendice E): fundo e borda, linhas de texto, filhos em ordem, e por
//! ultimo os posicionados por z-index. Fixos nao rolam.

#[allow(unused_imports)]
use crate::web::mat::F32Ext;
use alloc::vec::Vec;

use super::quadro::{Quadro, Ret};
use crate::web::css::estilo::{EstiloBorda, Posicao, Repetir, TamanhoFundo, Transbordo};
use crate::web::css::valores::{Cor, Fixo, Gradiente, Imagem, UM};
use crate::web::dom::Id;
use crate::web::fonte::{Fontes, Glifo};
use crate::web::layout::caixa::{Arvore, Substituido, Tipo};

/// Uma imagem decodificada (ARGB com alfa).
pub use crate::web::imagem::Imagem as Bitmap32;

/// Onde a pintura busca imagens: pelo elemento (<img>) ou pela URL
/// (fundos em CSS).
pub trait Imagens {
    fn do_elemento(&self, id: Id) -> Option<&Bitmap32>;
    fn da_url(&self, url: &str) -> Option<&Bitmap32>;
}

pub struct SemImagens;

impl Imagens for SemImagens {
    fn do_elemento(&self, _: Id) -> Option<&Bitmap32> {
        None
    }
    fn da_url(&self, _: &str) -> Option<&Bitmap32> {
        None
    }
}

fn px(v: Fixo) -> i32 {
    (v + 32).div_euclid(UM)
}

fn ret_px(x: Fixo, y: Fixo, l: Fixo, a: Fixo) -> Ret {
    let x0 = px(x);
    let y0 = px(y);
    Ret::novo(x0, y0, px(x + l) - x0, px(y + a) - y0)
}

fn mul_op(a: u8, b: u8) -> u8 {
    ((a as u32 * b as u32 + 127) / 255) as u8
}

pub struct Pintor<'a> {
    pub arv: &'a Arvore,
    pub fontes: &'a mut Fontes,
    pub imagens: &'a dyn Imagens,
    pub q: &'a mut Quadro,
    /// Rolagem vertical da pagina (px de ponto fixo).
    pub rolagem: Fixo,
}

/// Caixa posicionada pintada depois, na ordem do z-index.
struct Adiado {
    z: i32,
    k: u32,
    dy: Fixo,
    /// O recorte dos ancestrais com overflow, se vale para ela.
    recorte: Option<Ret>,
}

impl Pintor<'_> {
    pub fn pintar(&mut self) {
        self.contexto(0, 255, 0);
    }

    /// Pinta `k` como raiz de um contexto de empilhamento.
    fn contexto(&mut self, k: u32, opacidade: u8, dy: Fixo) {
        let mut adiados: Vec<Adiado> = Vec::new();
        self.caixa(k, opacidade, dy, &mut adiados, true);
        adiados.sort_by_key(|x| x.z);
        for d in adiados {
            let op = mul_op(opacidade, self.arv.caixas[d.k as usize].estilo.opacidade);
            let salvo = self.q.recorte;
            if let Some(r) = d.recorte {
                self.q.recorte = self.q.recorte.intersecao(&r);
            }
            self.contexto(d.k, op, d.dy);
            self.q.recorte = salvo;
        }
    }

    fn caixa(&mut self, k: u32, opacidade: u8, dy: Fixo, adiados: &mut Vec<Adiado>, raiz: bool) {
        let c = &self.arv.caixas[k as usize];
        let e = c.estilo.clone();
        let dy = if e.posicao == Posicao::Fixa { 0 } else { dy };
        if !raiz && e.posicao != Posicao::Estatica {
            // relative e sticky continuam presos ao overflow dos ancestrais
            // (o indice da Wikipedia, de itens relative, vazava por cima do
            // artigo); absolute e fixed podem ter o bloco de conter fora
            // deles, entao ficam com o recorte do contexto
            let recorte = matches!(e.posicao, Posicao::Relativa | Posicao::Grudada).then_some(self.q.recorte);
            adiados.push(Adiado { z: e.z.unwrap_or(0), k, dy, recorte });
            return;
        }
        let op = if raiz { opacidade } else { mul_op(opacidade, e.opacidade) };
        if op == 0 {
            return;
        }
        let dy_total = dy - self.rolagem * (e.posicao != Posicao::Fixa) as Fixo;
        let _ = dy_total;
        let desloc = if e.posicao == Posicao::Fixa { 0 } else { -self.rolagem };
        let (x, y, l, a) = (c.x, c.y + desloc, c.l, c.a);
        // fora da tela: pula o que da (mas os filhos podem transbordar)
        let visivel_na_tela = y < self.q.altura as Fixo * UM && y + a > 0;
        let caixa_inline = matches!(c.tipo, Tipo::Inline | Tipo::Texto(_));
        if e.visivel && !caixa_inline && !matches!(c.tipo, Tipo::Quebra) && visivel_na_tela {
            self.fundo_e_borda(k, x, y, l, a, op);
        }
        let salvo = self.q.recorte;
        let corta = !matches!(e.transbordo[0], Transbordo::Visivel) || !matches!(e.transbordo[1], Transbordo::Visivel);
        if corta && !caixa_inline {
            let r = ret_px(x + c.borda[3], y + c.borda[0], l - c.borda[1] - c.borda[3], a - c.borda[0] - c.borda[2]);
            self.q.recorte = self.q.recorte.intersecao(&r);
        }
        if let Tipo::Substituido(s) = &c.tipo {
            if e.visivel && visivel_na_tela {
                self.substituido(k, s.clone(), x, y, op);
            }
        }
        // linhas de texto
        if !c.linhas.is_empty() {
            self.linhas(k, desloc, op);
        }
        for &f in &self.arv.caixas[k as usize].filhos.clone() {
            if self.q.recorte.vazio() {
                break;
            }
            self.caixa(f, op, dy, adiados, false);
        }
        self.q.recorte = salvo;
    }

    fn fundo_e_borda(&mut self, k: u32, x: Fixo, y: Fixo, l: Fixo, a: Fixo, op: u8) {
        let c = &self.arv.caixas[k as usize];
        let e = c.estilo.clone();
        let r = ret_px(x, y, l, a);
        if let Some(s) = e.sombra {
            // borrao aproximado: aneis concentricos com alfa caindo
            let passos = (s.borrao / UM).clamp(0, 12);
            if passos == 0 {
                self.q.retangulo(ret_px(x + s.x, y + s.y, l, a), s.cor, op);
            } else {
                let alfa_anel = (s.cor.a() as u32 / (passos as u32 + 1)).max(1) as u8;
                for i in (0..passos).rev() {
                    let d = (i * UM) as Fixo;
                    let rs = ret_px(x + s.x - d, y + s.y - d, l + 2 * d, a + 2 * d);
                    self.q.retangulo(rs, s.cor.com_alfa(alfa_anel), op);
                }
            }
        }
        if e.fundo.visivel() {
            let raio = e.raio[0].resolver(l.min(a));
            if raio > 2 * UM {
                self.retangulo_arredondado(r, px(raio), e.fundo, op);
            } else {
                self.q.retangulo(r, e.fundo, op);
            }
        }
        if let Some(img) = &e.imagem_fundo {
            let salvo = self.q.recorte;
            self.q.recorte = self.q.recorte.intersecao(&r);
            match &**img {
                Imagem::Gradiente(g) => self.gradiente(r, g, op),
                Imagem::Url(u) => {
                    if let Some(b) = self.imagens.da_url(u) {
                        let (bl, ba) = (b.largura, b.altura);
                        let (il, ia) = match e.tamanho_fundo {
                            TamanhoFundo::Cobrir | TamanhoFundo::Conter => {
                                let sx = r.l as f32 / bl.max(1) as f32;
                                let sy = r.a as f32 / ba.max(1) as f32;
                                let s = if e.tamanho_fundo == TamanhoFundo::Cobrir { sx.max(sy) } else { sx.min(sy) };
                                ((bl as f32 * s) as i32, (ba as f32 * s) as i32)
                            }
                            TamanhoFundo::M(w, h) => {
                                let w = px(w.resolver(l)).max(1);
                                let h = h.map(|h| px(h.resolver(a))).unwrap_or(w * ba / bl.max(1));
                                (w, h.max(1))
                            }
                            TamanhoFundo::Auto => (bl, ba),
                        };
                        let ox = r.x + px(e.posicao_fundo.0.resolver((r.l - il) * UM));
                        let oy = r.y + px(e.posicao_fundo.1.resolver((r.a - ia) * UM));
                        let (rx, ry) = match e.repetir_fundo {
                            Repetir::Repetir => (true, true),
                            Repetir::X => (true, false),
                            Repetir::Y => (false, true),
                            Repetir::Nao => (false, false),
                        };
                        if il > 0 && ia > 0 {
                            let x0 = if rx { ox - ((ox - r.x) / il + 1) * il } else { ox };
                            let y0 = if ry { oy - ((oy - r.y) / ia + 1) * ia } else { oy };
                            let mut yy = y0;
                            while yy < r.y + r.a {
                                let mut xx = x0;
                                while xx < r.x + r.l {
                                    self.q.imagem(&b.px, bl, ba, Ret::novo(xx, yy, il, ia), op);
                                    if !rx {
                                        break;
                                    }
                                    xx += il;
                                }
                                if !ry {
                                    break;
                                }
                                yy += ia;
                            }
                        }
                    }
                }
            }
            self.q.recorte = salvo;
        }
        // bordas
        let b = [e.borda(0), e.borda(1), e.borda(2), e.borda(3)];
        if b.iter().any(|&v| v > 0) {
            let bp: Vec<i32> = b.iter().map(|&v| if v > 0 { px(v).max(1) } else { 0 }).collect();
            let cores = [e.cor_borda(0), e.cor_borda(1), e.cor_borda(2), e.cor_borda(3)];
            let estilos = e.borda_estilo;
            let lado = |q: &mut Quadro, rr: Ret, i: usize| {
                if bp[i] == 0 {
                    return;
                }
                match estilos[i] {
                    EstiloBorda::Tracejado | EstiloBorda::Pontilhado => {
                        let passo = if estilos[i] == EstiloBorda::Pontilhado { bp[i].max(1) } else { (bp[i] * 3).max(3) };
                        let horizontal = rr.l > rr.a;
                        let total = if horizontal { rr.l } else { rr.a };
                        let mut p = 0;
                        while p < total {
                            let n = passo.min(total - p);
                            let pedaco = if horizontal { Ret::novo(rr.x + p, rr.y, n, rr.a) } else { Ret::novo(rr.x, rr.y + p, rr.l, n) };
                            q.retangulo(pedaco, cores[i], op);
                            p += passo * 2;
                        }
                    }
                    _ => q.retangulo(rr, cores[i], op),
                }
            };
            lado(self.q, Ret::novo(r.x, r.y, r.l, bp[0]), 0);
            lado(self.q, Ret::novo(r.x, r.y + r.a - bp[2], r.l, bp[2]), 2);
            lado(self.q, Ret::novo(r.x, r.y + bp[0], bp[3], r.a - bp[0] - bp[2]), 3);
            lado(self.q, Ret::novo(r.x + r.l - bp[1], r.y + bp[0], bp[1], r.a - bp[0] - bp[2]), 1);
        }
    }

    fn retangulo_arredondado(&mut self, r: Ret, raio: i32, cor: Cor, op: u8) {
        let raio = raio.min(r.l / 2).min(r.a / 2).max(0);
        if raio <= 1 {
            self.q.retangulo(r, cor, op);
            return;
        }
        // miolo
        self.q.retangulo(Ret::novo(r.x, r.y + raio, r.l, r.a - 2 * raio), cor, op);
        // faixas de cima e de baixo, linha a linha com antisserrilhado na ponta
        for i in 0..raio {
            let dy = raio as f32 - i as f32 - 0.5;
            let dx = (raio as f32 * raio as f32 - dy * dy).max(0.0).sqrt();
            let recuo = raio as f32 - dx;
            let ri = recuo.floor() as i32;
            let frac = 1.0 - (recuo - ri as f32);
            let a_borda = (frac * 255.0) as u8;
            for (yy, _) in [(r.y + i, 0), (r.y + r.a - 1 - i, 1)] {
                self.q.retangulo(Ret::novo(r.x + ri + 1, yy, r.l - 2 * ri - 2, 1), cor, op);
                self.q.retangulo(Ret::novo(r.x + ri, yy, 1, 1), cor, mul_op(op, a_borda));
                self.q.retangulo(Ret::novo(r.x + r.l - ri - 1, yy, 1, 1), cor, mul_op(op, a_borda));
            }
        }
    }

    fn gradiente(&mut self, r: Ret, g: &Gradiente, op: u8) {
        let n = g.paradas.len();
        if n == 0 || r.vazio() {
            return;
        }
        // posicoes das paradas (as sem posicao ficam igualmente espacadas)
        let mut pos: Vec<f32> = g.paradas.iter().map(|p| p.1.unwrap_or(f32::NAN)).collect();
        if pos[0].is_nan() {
            pos[0] = 0.0;
        }
        if pos[n - 1].is_nan() {
            pos[n - 1] = 1.0;
        }
        let mut i = 0;
        while i < n {
            if pos[i].is_nan() {
                let ini = i - 1;
                let mut fim = i;
                while pos[fim].is_nan() {
                    fim += 1;
                }
                for j in i..fim {
                    pos[j] = pos[ini] + (pos[fim] - pos[ini]) * (j - ini) as f32 / (fim - ini) as f32;
                }
                i = fim;
            }
            i += 1;
        }
        let cor_em = |t: f32| -> Cor {
            if t <= pos[0] {
                return g.paradas[0].0;
            }
            for j in 1..n {
                if t <= pos[j] {
                    let span = (pos[j] - pos[j - 1]).max(1e-6);
                    let f = (t - pos[j - 1]) / span;
                    let (a, b) = (g.paradas[j - 1].0, g.paradas[j].0);
                    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * f) as u8;
                    return Cor::rgba(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()), mix(a.a(), b.a()));
                }
            }
            g.paradas[n - 1].0
        };
        let ang = g.angulo.to_radians();
        let (sx, sy) = (ang.sin(), -ang.cos());
        let comprimento = (r.l as f32 * sx).abs() + (r.a as f32 * sy).abs();
        let (cx, cy) = (r.x as f32 + r.l as f32 / 2.0, r.y as f32 + r.a as f32 / 2.0);
        let alvo = r.intersecao(&self.q.recorte);
        if g.radial {
            let raio = ((r.l * r.l + r.a * r.a) as f32).sqrt() / 2.0;
            for yy in alvo.y..alvo.y + alvo.a {
                for xx in alvo.x..alvo.x + alvo.l {
                    let d = ((xx as f32 - cx).powi(2) + (yy as f32 - cy).powi(2)).sqrt() / raio.max(1.0);
                    self.q.retangulo(Ret::novo(xx, yy, 1, 1), cor_em(d), op);
                }
            }
            return;
        }
        // gradiente vertical/horizontal puro: uma cor por linha/coluna
        if sx.abs() < 1e-3 {
            for yy in alvo.y..alvo.y + alvo.a {
                let t = ((yy as f32 + 0.5 - cy) * sy / comprimento.max(1.0)) + 0.5;
                self.q.retangulo(Ret::novo(alvo.x, yy, alvo.l, 1), cor_em(t), op);
            }
        } else if sy.abs() < 1e-3 {
            for xx in alvo.x..alvo.x + alvo.l {
                let t = ((xx as f32 + 0.5 - cx) * sx / comprimento.max(1.0)) + 0.5;
                self.q.retangulo(Ret::novo(xx, alvo.y, 1, alvo.a), cor_em(t), op);
            }
        } else {
            for yy in alvo.y..alvo.y + alvo.a {
                for xx in alvo.x..alvo.x + alvo.l {
                    let t = (((xx as f32 + 0.5 - cx) * sx + (yy as f32 + 0.5 - cy) * sy) / comprimento.max(1.0)) + 0.5;
                    self.q.retangulo(Ret::novo(xx, yy, 1, 1), cor_em(t), op);
                }
            }
        }
    }

    fn texto(&mut self, x: Fixo, base: Fixo, texto: &str, k_estilo: u32, op: u8) {
        self.texto_sublinhado_ate(x, base, texto, k_estilo, op, None)
    }

    /// `ate`: largura do sublinhado (inclui o espaco ate a proxima palavra).
    fn texto_sublinhado_ate(&mut self, x: Fixo, base: Fixo, texto: &str, k_estilo: u32, op: u8, ate: Option<Fixo>) {
        let e = self.arv.caixas[k_estilo as usize].estilo.clone();
        if !e.visivel {
            return;
        }
        let esc = self.fontes.escolher(e.familia, e.peso, e.italico, e.tamanho_fonte);
        let mut gl: Vec<Glifo> = Vec::with_capacity(texto.len());
        let largura = self.fontes.glifos(&esc, texto, e.espaco_letras, &mut gl);
        let by = px(base);
        if by < -64 || by > self.q.altura + 64 {
            return;
        }
        for g in &gl {
            let gx = x + g.x;
            let sub = ((gx.rem_euclid(UM)) / (UM / 4)) as u8;
            let b = self.fontes.bitmap(&esc, g.face, g.id, sub);
            self.q.glifo(&b, gx.div_euclid(UM), by, e.cor, op);
        }
        if e.decoracao != 0 {
            let cor = e.cor_decoracao.unwrap_or(e.cor);
            let esp = (e.tamanho_fonte / 14.0).max(1.0) as i32;
            let x0 = px(x);
            let w = px(x + ate.unwrap_or(largura).max(largura)) - x0;
            if e.decoracao & 1 != 0 {
                self.q.linha_h(x0, by + (e.tamanho_fonte / 8.0) as i32 + 1, w, esp, cor);
            }
            if e.decoracao & 2 != 0 {
                self.q.linha_h(x0, by - (e.tamanho_fonte * 0.8) as i32, w, esp, cor);
            }
            if e.decoracao & 4 != 0 {
                self.q.linha_h(x0, by - (e.tamanho_fonte * 0.3) as i32, w, esp, cor);
            }
        }
    }

    fn linhas(&mut self, k: u32, desloc: Fixo, op: u8) {
        let c = &self.arv.caixas[k as usize];
        let altura_q = self.q.altura as Fixo * UM;
        for ln in &c.linhas {
            let topo = ln.y + desloc;
            if topo > altura_q || topo + ln.altura < 0 {
                continue;
            }
            // fundos das caixas inline primeiro
            for f in &ln.fundos {
                let cf = &self.arv.caixas[f.caixa as usize];
                let e = cf.estilo.clone();
                if !e.visivel {
                    continue;
                }
                let r = ret_px(f.x0, f.topo + desloc, f.x1 - f.x0, f.baixo - f.topo);
                if e.fundo.visivel() {
                    self.q.retangulo(r, e.fundo, op);
                }
                let b = [e.borda(0), e.borda(1), e.borda(2), e.borda(3)];
                if b.iter().any(|&v| v > 0) {
                    let bp: Vec<i32> = b.iter().map(|&v| if v > 0 { px(v).max(1) } else { 0 }).collect();
                    self.q.retangulo(Ret::novo(r.x, r.y, r.l, bp[0]), e.cor_borda(0), op);
                    self.q.retangulo(Ret::novo(r.x, r.y + r.a - bp[2], r.l, bp[2]), e.cor_borda(2), op);
                    if f.inicio {
                        self.q.retangulo(Ret::novo(r.x, r.y, bp[3], r.a), e.cor_borda(3), op);
                    }
                    if f.fim {
                        self.q.retangulo(Ret::novo(r.x + r.l - bp[1], r.y, bp[1], r.a), e.cor_borda(1), op);
                    }
                }
            }
            for (n, t) in ln.trechos.iter().enumerate() {
                let Tipo::Texto(s) = &self.arv.caixas[t.caixa as usize].tipo else { continue };
                let s = s.clone();
                let pedaco = &s[t.ini as usize..t.fim as usize];
                // sublinhado continua ate a proxima palavra da mesma caixa
                let ate = ln.trechos.get(n + 1).filter(|p| p.caixa == t.caixa).map(|p| p.x - t.x);
                self.texto_sublinhado_ate(t.x, t.base + desloc, pedaco, t.caixa, op, ate);
            }
        }
    }

    fn substituido(&mut self, k: u32, s: Substituido, x: Fixo, y: Fixo, op: u8) {
        let c = &self.arv.caixas[k as usize];
        let e = c.estilo.clone();
        let cx = x + c.borda[3] + c.preench[3];
        let cy = y + c.borda[0] + c.preench[0];
        let cl = c.conteudo_l();
        let ca = c.conteudo_a();
        let r = ret_px(cx, cy, cl, ca);
        match s {
            Substituido::Imagem { alt } => {
                if let Some(b) = self.imagens.do_elemento(c.no) {
                    self.q.imagem(&b.px, b.largura, b.altura, r, op);
                } else if !alt.is_empty() {
                    let esc = self.fontes.escolher(e.familia, e.peso, e.italico, e.tamanho_fonte);
                    let (asc, _) = self.fontes.metricas(&esc);
                    let salvo = self.q.recorte;
                    self.q.recorte = self.q.recorte.intersecao(&r);
                    self.texto(cx, cy + asc, &alt, k, op);
                    self.q.recorte = salvo;
                }
            }
            Substituido::Entrada { texto, senha, marcavel, marcado } => {
                if marcavel {
                    self.q.retangulo(r, Cor::BRANCO, op);
                    let cinza = Cor(0xFF76_7676);
                    self.q.retangulo(Ret::novo(r.x, r.y, r.l, 1), cinza, op);
                    self.q.retangulo(Ret::novo(r.x, r.y + r.a - 1, r.l, 1), cinza, op);
                    self.q.retangulo(Ret::novo(r.x, r.y, 1, r.a), cinza, op);
                    self.q.retangulo(Ret::novo(r.x + r.l - 1, r.y, 1, r.a), cinza, op);
                    if marcado {
                        self.q.retangulo(Ret::novo(r.x + 3, r.y + 3, r.l - 6, r.a - 6), Cor(0xFF00_75FF), op);
                    }
                    return;
                }
                let mostrar: alloc::string::String = if senha { texto.chars().map(|_| '\u{2022}').collect() } else { texto };
                let esc = self.fontes.escolher(e.familia, e.peso, e.italico, e.tamanho_fonte);
                let (asc, desc) = self.fontes.metricas(&esc);
                let base = cy + (ca - asc - desc) / 2 + asc;
                let w = self.fontes.medir(&esc, &mostrar, 0);
                let x_texto = if e.alinhamento == crate::web::css::estilo::Alinhamento::Centro || c.estilo.fundo == Cor(0xFFEF_EFEF) {
                    cx + (cl - w) / 2
                } else {
                    cx
                };
                let salvo = self.q.recorte;
                self.q.recorte = self.q.recorte.intersecao(&r);
                self.texto(x_texto, base, &mostrar, k, op);
                self.q.recorte = salvo;
            }
            Substituido::Selecao { texto } => {
                let esc = self.fontes.escolher(e.familia, e.peso, e.italico, e.tamanho_fonte);
                let (asc, desc) = self.fontes.metricas(&esc);
                let base = cy + (ca - asc - desc) / 2 + asc;
                self.texto(cx + 2 * UM, base, &texto, k, op);
                self.texto(cx + cl - 14 * UM, base, "\u{25BE}", k, op);
            }
            Substituido::AreaTexto { texto } => {
                let esc = self.fontes.escolher(e.familia, e.peso, e.italico, e.tamanho_fonte);
                let (asc, _) = self.fontes.metricas(&esc);
                let salvo = self.q.recorte;
                self.q.recorte = self.q.recorte.intersecao(&r);
                let linha = e.linha();
                for (n, l) in texto.lines().enumerate() {
                    self.texto(cx, cy + asc + linha * n as Fixo, l, k, op);
                }
                self.q.recorte = salvo;
            }
            Substituido::Video | Substituido::Quadro | Substituido::Canvas | Substituido::Svg | Substituido::Botao => {
                if let Some(b) = self.imagens.do_elemento(c.no) {
                    self.q.imagem(&b.px, b.largura, b.altura, r, op);
                } else if matches!(s, Substituido::Video) {
                    self.q.retangulo(r, Cor::PRETO, op);
                }
            }
        }
    }
}

/// O link (elemento <a href>) no ponto (x, y) da pagina (ja com a rolagem).
pub fn link_em(arv: &Arvore, doc: &crate::web::dom::Documento, x: Fixo, y: Fixo) -> Option<Id> {
    // procura o trecho de texto ou caixa sob o ponto, de tras para frente
    let mut achado: Option<Id> = None;
    for (k, c) in arv.caixas.iter().enumerate() {
        for ln in &c.linhas {
            if y < ln.y || y >= ln.y + ln.altura {
                continue;
            }
            for t in &ln.trechos {
                if x >= t.x && x < t.x + t.largura {
                    achado = no_de(arv, t.caixa).or(achado);
                }
            }
        }
        if !matches!(c.tipo, Tipo::Texto(_) | Tipo::Inline) && c.no != crate::web::dom::NENHUM && x >= c.x && x < c.x + c.l && y >= c.y && y < c.y + c.a {
            if achado.is_none() {
                achado = Some(c.no);
            }
        }
        let _ = k;
    }
    // sobe ate um <a href>
    let mut n = achado?;
    while n != crate::web::dom::NENHUM {
        if let Some(e) = doc.elemento(n) {
            if e.eh(crate::web::atomo::a::A) && e.tem(crate::web::atomo::a::HREF) {
                return Some(n);
            }
        }
        n = doc.pai(n);
    }
    None
}

fn no_de(arv: &Arvore, mut k: u32) -> Option<Id> {
    while k != u32::MAX {
        let c = &arv.caixas[k as usize];
        if c.no != crate::web::dom::NENHUM && c.no != crate::web::layout::caixa::MARCADOR_EXTERNO {
            return Some(c.no);
        }
        k = c.pai;
    }
    None
}
