//! Filme MP4 (H.264) na TV box: o RKVDEC decodifica, o DPB da ordem de
//! exibicao, o relogio escolhe o quadro e o RGA converte NV12 -> RGB ja
//! escalado na janela. A CPU nao toca em pixel nenhum.
//!
//! Por enquanto sem som (o AAC ainda nao tem decodificador).

use alloc::collections::BinaryHeap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Reverse;

use super::visualizador::FonteDisco;
use crate::gui::grafico::{Retangulo, Superficie};
use crate::midia::Fonte;
use crate::midia::mp4::{Amostra, Mp4};
use crate::video::rga;
use crate::video::rkvdec::{Decodificador, QuadroPronto};

pub struct Filme {
    fonte: FonteDisco,
    amostras: Vec<Amostra>,
    escala: u64,
    dec: Decodificador,
    /// Proxima amostra a decodificar.
    proxima: usize,
    /// Tempos de apresentacao dos quadros decodificados e ainda nao
    /// mostrados: o DPB entrega na ordem de exibicao = ordem desses tempos.
    tempos: BinaryHeap<Reverse<i64>>,
    /// O quadro na tela e o proximo (com o tempo de cada um).
    atual: Option<(QuadroPronto, i64)>,
    seguinte: Option<(QuadroPronto, i64)>,
    /// Quando (em us do relogio) o tempo 0 do video passou.
    inicio: i64,
    pausado_em: Option<u64>,
    acabou: bool,
    buf: Vec<u8>,
    pub mostrados: u32,
    pub erro: Option<&'static str>,
    duracao_ms: u64,
    largura: u32,
    altura: u32,
}

fn agora_us() -> u64 {
    crate::cpu::contador() * 1_000_000 / crate::cpu::frequencia_timer().max(1)
}

impl Filme {
    pub fn abrir(caminho: &str) -> Result<Filme, &'static str> {
        if !rga::disponivel() {
            return Err("o RGA (conversão de cor) não está ligado");
        }
        let mut fonte = FonteDisco::abrir(caminho)?;
        let mp4 = Mp4::abrir(&mut fonte)?;
        let v = mp4.video().ok_or("MP4 sem vídeo")?;
        if &v.codec != b"avc1" && &v.codec != b"avc3" {
            return Err("só vídeo H.264 por enquanto");
        }
        let dec = Decodificador::novo(&v.config)?;
        let (largura, altura) = dec.tamanho();
        Ok(Filme {
            fonte,
            amostras: v.amostras.clone(),
            escala: v.escala.max(1) as u64,
            dec,
            proxima: 0,
            tempos: BinaryHeap::new(),
            atual: None,
            seguinte: None,
            inicio: agora_us() as i64,
            pausado_em: None,
            acabou: false,
            buf: Vec::new(),
            mostrados: 0,
            erro: None,
            duracao_ms: v.duracao_ms(),
            largura,
            altura,
        })
    }

    pub fn descricao(&self) -> String {
        format!("H.264 {}x{} por hardware (RKVDEC + RGA)", self.largura, self.altura)
    }

    pub fn duracao_ms(&self) -> u64 {
        self.duracao_ms
    }

    pub fn posicao_ms(&self) -> u64 {
        self.atual.map(|(_, t)| t.max(0) as u64 * 1000 / self.escala).unwrap_or(0)
    }

    pub fn pausado(&self) -> bool {
        self.pausado_em.is_some()
    }

    pub fn acabou(&self) -> bool {
        self.acabou
    }

    pub fn pausar(&mut self) {
        match self.pausado_em.take() {
            Some(p) => self.inicio += (agora_us() - p) as i64,
            None => self.pausado_em = Some(agora_us()),
        }
    }

    /// Decodifica ate sair um quadro (ou o arquivo acabar).
    fn puxar(&mut self) -> Option<(QuadroPronto, i64)> {
        loop {
            if let Some(q) = self.dec.proximo() {
                let t = self.tempos.pop().map(|Reverse(t)| t).unwrap_or(0);
                return Some((q, t));
            }
            if self.proxima >= self.amostras.len() {
                if self.acabou {
                    return None;
                }
                self.dec.esvaziar();
                self.acabou = true;
                continue;
            }
            let a = self.amostras[self.proxima];
            self.proxima += 1;
            self.buf.resize(a.tamanho as usize, 0);
            if self.fonte.ler(a.pos as usize, &mut self.buf) < self.buf.len() {
                self.erro = Some("o arquivo acabou antes do fim do vídeo");
                continue;
            }
            match self.dec.decodificar(&self.buf) {
                Ok(()) => self.tempos.push(Reverse(a.dts as i64 + a.desvio as i64)),
                Err(e) => {
                    // o quadro (com defeito) entra assim mesmo: mantem a
                    // contagem de tempos igual a de quadros
                    self.tempos.push(Reverse(a.dts as i64 + a.desvio as i64));
                    self.erro = Some(e);
                }
            }
        }
    }

    /// Anda o relogio. true = trocou o quadro (redesenhar).
    pub fn tique(&mut self) -> bool {
        if self.pausado_em.is_some() {
            return false;
        }
        let agora = (agora_us() as i64 - self.inicio) * self.escala as i64 / 1_000_000;
        let mut mudou = false;
        loop {
            if self.seguinte.is_none() {
                self.seguinte = self.puxar();
                if self.seguinte.is_none() {
                    break;
                }
            }
            let (_, t) = self.seguinte.unwrap();
            if self.atual.is_some() && t > agora {
                break;
            }
            if self.atual.is_none() {
                // primeiro quadro: o relogio comeca nele (os tempos do MP4
                // podem comecar depois de 0 por causa do ctts)
                self.inicio = agora_us() as i64 - t * 1_000_000 / self.escala as i64;
            }
            if let Some((velho, _)) = self.atual.take() {
                self.dec.soltar(&velho);
            }
            self.atual = self.seguinte.take();
            self.mostrados += 1;
            mudou = true;
        }
        mudou
    }

    /// O quadro atual em `area` da janela, mantendo a proporcao.
    pub fn desenhar(&self, s: &mut Superficie, area: Retangulo) {
        let Some((q, _)) = &self.atual else { return };
        let (ql, qa) = (q.largura.max(2) as i64, q.altura.max(2) as i64);
        let (al, aa) = (area.largura as i64, area.altura as i64);
        // o maior retangulo com a proporcao do video que cabe
        let (mut l, mut a) = if al * qa <= aa * ql { (al, al * qa / ql) } else { (aa * ql / qa, aa) };
        l &= !1;
        a &= !1;
        let x = area.x as i64 + (al - l) / 2;
        let y = area.y as i64 + (aa - a) / 2;
        let origem = rga::Imagem { endereco: q.y, passo: q.passo, largura: q.x + q.largura, altura: q.y0 + q.altura, formato: rga::Formato::Nv12 { uv: q.uv, bt709: q.bt709 } };
        let (sl, sa) = (s.largura(), s.altura());
        let destino = rga::Imagem::xrgb(s.pixels_mut().as_mut_ptr() as usize, sl * 4, sl, sa);
        rga::copiar(&origem, (q.x & !1, q.y0 & !1, q.largura & !1, q.altura & !1), &destino, (x as usize, y as usize, l as usize, a as usize));
    }
}

impl Drop for Filme {
    fn drop(&mut self) {
        if let Some((q, _)) = self.atual.take() {
            self.dec.soltar(&q);
        }
        if let Some((q, _)) = self.seguinte.take() {
            self.dec.soltar(&q);
        }
    }
}
