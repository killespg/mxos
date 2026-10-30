//! Filme MP4 (H.264) na TV box: o RKVDEC decodifica, o DPB da ordem de
//! exibicao, o relogio escolhe o quadro e o RGA converte NV12 -> RGB ja
//! escalado na janela. A CPU nao toca em pixel nenhum.
//!
//! As amostras vem de uma `Origem`: um MP4 inteiro (disco, ou a rede aos
//! pedacos) ou um MP4 fragmentado (DASH do YouTube), que baixa o proximo
//! segmento em paralelo enquanto o video toca.

use alloc::boxed::Box;
use alloc::collections::{BinaryHeap, VecDeque};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Reverse;

use super::visualizador::FonteDisco;
use crate::gui::grafico::{Retangulo, Superficie};
use crate::midia::Fonte;
use crate::midia::aac::{self, Aac};
use crate::midia::mp4::{self, Amostra, Mp4, Segmento};
use crate::rede::fluxo::Baixador;
use crate::video::rga;
use crate::video::rkvdec::{Decodificador, QuadroPronto};

/// A proxima amostra de video.
pub enum Proxima {
    /// Bytes e tempo de apresentacao (na escala da trilha).
    Amostra(Vec<u8>, i64),
    /// Ainda baixando: tentar no proximo tique.
    Esperando,
    Fim,
}

/// De onde vem as amostras.
pub trait Origem {
    /// avcC.
    fn config(&self) -> &[u8];
    fn escala(&self) -> u64;
    fn duracao_ms(&self) -> u64;
    /// Rede em segundo plano (sem esperar).
    fn andar(&mut self) -> Result<(), &'static str> {
        Ok(())
    }
    fn proxima(&mut self) -> Result<Proxima, &'static str>;
}

/// Um MP4 com o indice no moov: o arquivo do disco ou da rede.
struct OrigemArquivo {
    fonte: Box<dyn Fonte>,
    amostras: Vec<Amostra>,
    i: usize,
    config: Vec<u8>,
    escala: u64,
    duracao_ms: u64,
}

impl Origem for OrigemArquivo {
    fn config(&self) -> &[u8] {
        &self.config
    }
    fn escala(&self) -> u64 {
        self.escala
    }
    fn duracao_ms(&self) -> u64 {
        self.duracao_ms
    }
    fn proxima(&mut self) -> Result<Proxima, &'static str> {
        let Some(a) = self.amostras.get(self.i).copied() else { return Ok(Proxima::Fim) };
        self.i += 1;
        let mut b = alloc::vec![0u8; a.tamanho as usize];
        if self.fonte.ler(a.pos as usize, &mut b) < b.len() {
            return Err("o arquivo acabou antes do fim do vídeo");
        }
        Ok(Proxima::Amostra(b, a.dts as i64 + a.desvio as i64))
    }
}

/// MP4 fragmentado pela rede (DASH): init + indice no comeco, depois os
/// segmentos um atras do outro, sempre um ou dois na frente.
pub struct OrigemDash {
    dl: Baixador,
    segs: Vec<Segmento>,
    /// Proximo segmento a pedir.
    pedir: usize,
    prontos: VecDeque<Vec<u8>>,
    amostras: VecDeque<(Vec<u8>, i64)>,
    padrao: (u32, u32, u32),
    config: Vec<u8>,
    escala: u64,
    duracao_ms: u64,
}

/// Quantos segmentos baixados ficam esperando (cada um tem alguns segundos).
const SEGMENTOS_NA_FRENTE: usize = 2;

impl OrigemDash {
    /// `fim_indice`: o ultimo byte do sidx (indexRange.end da API). `audio`:
    /// a trilha de som em vez da de video (config = o esds).
    pub fn abrir(url: &str, fim_indice: u64, audio: bool) -> Result<OrigemDash, &'static str> {
        let mut dl = Baixador::novo(url)?;
        let init = dl.baixar(0, fim_indice)?;
        let mut f = init.clone();
        let mp4 = Mp4::abrir(&mut f)?;
        let v = if audio { mp4.audio().ok_or("o fluxo não tem som")? } else { mp4.video().ok_or("o fluxo não tem vídeo")? };
        let padrao = mp4::padroes(mp4::moov_de(&init).ok_or("init sem moov")?);
        let segs = mp4::indice(&init, 0);
        if segs.is_empty() {
            return Err("o fluxo não tem índice de segmentos");
        }
        let escala = v.escala.max(1) as u64;
        let total: u64 = segs.iter().map(|s| s.duracao as u64).sum();
        Ok(OrigemDash { dl, segs, pedir: 0, prontos: VecDeque::new(), amostras: VecDeque::new(), padrao, config: v.config.clone(), escala, duracao_ms: total * 1000 / escala })
    }
}

impl Origem for OrigemDash {
    fn config(&self) -> &[u8] {
        &self.config
    }
    fn escala(&self) -> u64 {
        self.escala
    }
    fn duracao_ms(&self) -> u64 {
        self.duracao_ms
    }
    fn andar(&mut self) -> Result<(), &'static str> {
        if let Some(r) = self.dl.andar() {
            self.prontos.push_back(r?);
        }
        if !self.dl.ocupado() && self.prontos.len() < SEGMENTOS_NA_FRENTE && self.pedir < self.segs.len() {
            let s = self.segs[self.pedir];
            self.pedir += 1;
            self.dl.pedir(s.pos, s.pos + s.tamanho as u64 - 1)?;
        }
        Ok(())
    }
    fn proxima(&mut self) -> Result<Proxima, &'static str> {
        if self.amostras.is_empty() {
            if let Some(seg) = self.prontos.pop_front() {
                for a in mp4::fragmento(&seg, self.padrao) {
                    let b = seg[a.inicio..a.inicio + a.tamanho as usize].to_vec();
                    self.amostras.push_back((b, a.dts as i64 + a.desvio as i64));
                }
            }
        }
        if let Some((b, t)) = self.amostras.pop_front() {
            return Ok(Proxima::Amostra(b, t));
        }
        if self.pedir >= self.segs.len() && !self.dl.ocupado() && self.prontos.is_empty() {
            return Ok(Proxima::Fim);
        }
        Ok(Proxima::Esperando)
    }
}

/// O som do filme: AAC decodificado, convertido para 48 kHz e posto no
/// anel do HDMI. Os quadros que ja sairam pelo HDMI sao o relogio.
struct Som {
    origem: OrigemDash,
    aac: Aac,
    /// Estereo na taxa do AAC, ainda nao convertido.
    entrada: VecDeque<[i16; 2]>,
    /// Posicao (16.16) entre entrada[0] e entrada[1].
    fase: u32,
    passo: u32,
    /// Tempo (us) da primeira amostra e o contador do HDMI naquela hora.
    inicio_us: Option<i64>,
    tocados_inicio: u32,
    acabou: bool,
}

impl Som {
    fn abrir(url: &str, fim: u64) -> Result<Som, &'static str> {
        let origem = OrigemDash::abrir(url, fim, true)?;
        let asc = aac::asc_do_esds(origem.config()).ok_or("som sem configuração AAC")?;
        let aac = Aac::novo(asc)?;
        if !crate::audio::hdmi::iniciar() {
            return Err("o som pelo HDMI não ligou");
        }
        crate::audio::hdmi::esvaziar();
        let passo = ((aac.taxa as u64) << 16) / crate::audio::hdmi::TAXA as u64;
        Ok(Som { origem, aac, entrada: VecDeque::new(), fase: 0, passo: passo as u32, inicio_us: None, tocados_inicio: 0, acabou: false })
    }

    /// Baixa, decodifica e enche o anel do HDMI (sem esperar).
    fn alimentar(&mut self) -> Result<(), &'static str> {
        use crate::audio::hdmi;
        self.origem.andar()?;
        let mut saida: Vec<i16> = Vec::new();
        while hdmi::espaco() > 2048 && !self.acabou {
            // entrada suficiente para 1024 quadros de saida
            while self.entrada.len() < 1200 {
                match self.origem.proxima()? {
                    Proxima::Amostra(b, t) => {
                        let mut pcm = Vec::new();
                        if self.aac.decodificar(&b, &mut pcm).is_err() {
                            pcm = alloc::vec![0; 2048];
                        }
                        if self.inicio_us.is_none() {
                            self.inicio_us = Some(t * 1_000_000 / self.origem.escala() as i64);
                            self.tocados_inicio = hdmi::tocados().wrapping_add(hdmi::na_fila() as u32);
                        }
                        self.entrada.extend(pcm.chunks_exact(2).map(|c| [c[0], c[1]]));
                    }
                    Proxima::Esperando => break,
                    Proxima::Fim => {
                        self.acabou = true;
                        break;
                    }
                }
            }
            if self.entrada.len() < 2 {
                break;
            }
            // interpolacao linear para 48 kHz
            saida.clear();
            let quantos = (hdmi::espaco() - 1).min(4096);
            while saida.len() / 2 < quantos && self.entrada.len() >= 2 {
                let (a, b) = (self.entrada[0], self.entrada[1]);
                let f = self.fase as i32;
                for c in 0..2 {
                    saida.push((a[c] as i32 + ((b[c] as i32 - a[c] as i32) * f >> 16)) as i16);
                }
                self.fase += self.passo;
                while self.fase >= 1 << 16 && self.entrada.len() >= 2 {
                    self.fase -= 1 << 16;
                    self.entrada.pop_front();
                }
            }
            if saida.is_empty() {
                break;
            }
            hdmi::escrever(&saida);
        }
        Ok(())
    }

    /// Tempo do som que esta saindo agora (us), se ja comecou.
    fn agora_us(&self) -> Option<i64> {
        let ini = self.inicio_us?;
        let tocados = crate::audio::hdmi::tocados().wrapping_sub(self.tocados_inicio) as i32;
        if tocados <= 0 {
            return None;
        }
        Some(ini + tocados as i64 * 1_000_000 / crate::audio::hdmi::TAXA as i64)
    }
}

pub struct Filme {
    som: Option<Som>,
    origem: Box<dyn Origem>,
    escala: u64,
    dec: Decodificador,
    /// Tempos de apresentacao dos quadros decodificados e ainda nao
    /// mostrados: o DPB entrega na ordem de exibicao = ordem desses tempos.
    tempos: BinaryHeap<Reverse<i64>>,
    /// O quadro na tela e o proximo (com o tempo de cada um).
    atual: Option<(QuadroPronto, i64)>,
    seguinte: Option<(QuadroPronto, i64)>,
    /// Quando (em us do relogio) o tempo 0 do video passou.
    inicio: i64,
    pausado_em: Option<u64>,
    /// Sem dados no ultimo tique (baixando): o relogio para.
    esperando: bool,
    ultimo_tique: u64,
    acabou: bool,
    pub mostrados: u32,
    pub erro: Option<&'static str>,
    duracao_ms: u64,
    largura: u32,
    altura: u32,
}

/// Estado do ultimo filme (para o INFO da depuracao): posicao em ms,
/// quadros mostrados, tiques esperando a rede.
pub static POSICAO_MS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
pub static MOSTRADOS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
pub static ESPERAS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

fn agora_us() -> u64 {
    crate::cpu::contador() * 1_000_000 / crate::cpu::frequencia_timer().max(1)
}

impl Filme {
    pub fn abrir(caminho: &str) -> Result<Filme, &'static str> {
        Filme::de_fonte(Box::new(FonteDisco::abrir(caminho)?))
    }

    /// Um MP4 de qualquer lugar (disco, rede).
    pub fn de_fonte(mut fonte: Box<dyn Fonte>) -> Result<Filme, &'static str> {
        let mp4 = Mp4::abrir(&mut *fonte)?;
        let v = mp4.video().ok_or("MP4 sem vídeo")?;
        if &v.codec != b"avc1" && &v.codec != b"avc3" {
            return Err("só vídeo H.264 por enquanto");
        }
        let origem = OrigemArquivo { amostras: v.amostras.clone(), i: 0, config: v.config.clone(), escala: v.escala.max(1) as u64, duracao_ms: v.duracao_ms(), fonte };
        Filme::de_origem(Box::new(origem))
    }

    pub fn de_origem(origem: Box<dyn Origem>) -> Result<Filme, &'static str> {
        if !rga::garantir() {
            return Err("o RGA (conversão de cor) não respondeu");
        }
        let dec = Decodificador::novo(origem.config())?;
        let (largura, altura) = dec.tamanho();
        Ok(Filme {
            som: None,
            escala: origem.escala(),
            duracao_ms: origem.duracao_ms(),
            origem,
            dec,
            tempos: BinaryHeap::new(),
            atual: None,
            seguinte: None,
            inicio: agora_us() as i64,
            pausado_em: None,
            esperando: false,
            ultimo_tique: agora_us(),
            acabou: false,
            mostrados: 0,
            erro: None,
            largura,
            altura,
        })
    }

    /// Liga o som (DASH AAC): o relogio do video passa a ser o do som.
    pub fn ligar_som(&mut self, url: &str, fim_indice: u64) -> Result<(), &'static str> {
        self.som = Some(Som::abrir(url, fim_indice)?);
        Ok(())
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

    /// Esperando a rede (para mostrar "carregando").
    pub fn esperando(&self) -> bool {
        self.esperando
    }

    pub fn pausar(&mut self) {
        match self.pausado_em.take() {
            Some(p) => self.inicio += (agora_us() - p) as i64,
            None => self.pausado_em = Some(agora_us()),
        }
        if self.som.is_some() {
            crate::audio::hdmi::pausar(self.pausado_em.is_some());
        }
    }

    /// Decodifica ate sair um quadro. None = acabou ou esperando a rede.
    fn puxar(&mut self) -> Option<(QuadroPronto, i64)> {
        loop {
            if let Some(q) = self.dec.proximo() {
                let t = self.tempos.pop().map(|Reverse(t)| t).unwrap_or(0);
                return Some((q, t));
            }
            if self.acabou {
                return None;
            }
            match self.origem.proxima() {
                Ok(Proxima::Amostra(b, t)) => {
                    // com defeito ou nao, o tempo entra (um por quadro)
                    if let Err(e) = self.dec.decodificar(&b) {
                        self.erro = Some(e);
                    }
                    self.tempos.push(Reverse(t));
                }
                Ok(Proxima::Esperando) => {
                    self.esperando = true;
                    return None;
                }
                Ok(Proxima::Fim) => {
                    self.dec.esvaziar();
                    self.acabou = true;
                }
                Err(e) => {
                    self.erro = Some(e);
                    self.dec.esvaziar();
                    self.acabou = true;
                }
            }
        }
    }

    /// Anda o relogio. true = trocou o quadro (redesenhar).
    pub fn tique(&mut self) -> bool {
        if let Err(e) = self.origem.andar() {
            self.erro = Some(e);
        }
        if let Some(s) = &mut self.som {
            if let Err(e) = s.alimentar() {
                // sem som daqui em diante; o video segue pelo relogio
                self.erro = Some(e);
                self.som = None;
            }
        }
        let agora_real = agora_us();
        // esperando a rede no tique anterior: o video nao andou nesse tempo
        if self.esperando {
            self.inicio += (agora_real - self.ultimo_tique) as i64;
        }
        self.ultimo_tique = agora_real;
        self.esperando = false;
        if self.pausado_em.is_some() {
            return false;
        }
        let mut agora = (agora_real as i64 - self.inicio) * self.escala as i64 / 1_000_000;
        // com som, o relogio e o som (o que ja saiu pelo HDMI); antes de ele
        // comecar, o video fica no primeiro quadro
        if let Some(s) = &self.som {
            match s.agora_us() {
                Some(us) => {
                    agora = us * self.escala as i64 / 1_000_000;
                    // o relogio de parede acompanha (se o som sumir, segue dali)
                    self.inicio = agora_real as i64 - us;
                }
                None if self.atual.is_some() => agora = i64::MIN,
                None => {}
            }
        }
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
        // o proximo quadro ja passou da hora e nao chegou: esperando a rede
        if self.seguinte.is_none() && !self.acabou {
            self.esperando = true;
            ESPERAS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }
        POSICAO_MS.store(self.posicao_ms() as u32, core::sync::atomic::Ordering::Relaxed);
        MOSTRADOS.store(self.mostrados, core::sync::atomic::Ordering::Relaxed);
        mudou
    }

    /// O quadro atual em `area` da janela, mantendo a proporcao.
    pub fn desenhar(&self, s: &mut Superficie, area: Retangulo) {
        self.desenhar_recortado(s, area, area);
    }

    /// Como `desenhar`, mas so a parte dentro de `visivel` (o resto da
    /// area esta fora da tela: a origem e cortada na mesma proporcao).
    pub fn desenhar_recortado(&self, s: &mut Superficie, area: Retangulo, visivel: Retangulo) {
        let (sl, sa) = (s.largura(), s.altura());
        let destino = rga::Imagem::xrgb(s.pixels_mut().as_mut_ptr() as usize, sl * 4, sl, sa);
        self.desenhar_em(&destino, area, visivel);
    }

    /// O quadro atual numa imagem XRGB qualquer (a janela ou a tela toda).
    pub fn desenhar_em(&self, destino: &rga::Imagem, area: Retangulo, visivel: Retangulo) {
        let Some((q, _)) = &self.atual else { return };
        let (ql, qa) = (q.largura.max(2) as i64, q.altura.max(2) as i64);
        let (al, aa) = (area.largura as i64, area.altura as i64);
        // o maior retangulo com a proporcao do video que cabe
        let (mut l, mut a) = if al * qa <= aa * ql { (al, al * qa / ql) } else { (aa * ql / qa, aa) };
        l &= !1;
        a &= !1;
        let x = area.x as i64 + (al - l) / 2;
        let y = area.y as i64 + (aa - a) / 2;
        // a parte visivel do retangulo do video, e o pedaco da origem que cai nela
        let limite = Retangulo::novo(0, 0, destino.largura as i32, destino.altura as i32);
        let v = Retangulo::novo(x as i32, y as i32, l as i32, a as i32).intersecao(&visivel).intersecao(&limite);
        if v.vazio() || v.largura < 2 || v.altura < 2 {
            return;
        }
        let (dx, dy) = (v.x as i64 - x, v.y as i64 - y);
        let sx = (q.x as i64 + dx * ql / l) as usize & !1;
        let sy = (q.y0 as i64 + dy * qa / a) as usize & !1;
        let sl_ = ((v.largura as i64 * ql / l) as usize & !1).max(2);
        let sa_ = ((v.altura as i64 * qa / a) as usize & !1).max(2);
        let origem = rga::Imagem { endereco: q.y, passo: q.passo, largura: q.x + q.largura, altura: q.y0 + q.altura, formato: rga::Formato::Nv12 { uv: q.uv, bt709: q.bt709 } };
        let sl_ = sl_.min(q.x + q.largura - sx);
        let sa_ = sa_.min(q.y0 + q.altura - sy);
        rga::copiar(&origem, (sx, sy, sl_, sa_), destino, (v.x as usize, v.y as usize, v.largura as usize, v.altura as usize));
    }
}

impl Drop for Filme {
    fn drop(&mut self) {
        if self.som.is_some() {
            crate::audio::hdmi::pausar(false);
            crate::audio::hdmi::esvaziar();
        }
        if let Some((q, _)) = self.atual.take() {
            self.dec.soltar(&q);
        }
        if let Some((q, _)) = self.seguinte.take() {
            self.dec.soltar(&q);
        }
    }
}
