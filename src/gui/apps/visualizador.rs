//! Visualizador de midia: imagens (PNG, JPEG, GIF), video AVI/MJPEG, MP4
//! H.264 (na TV box, pelo hardware: ver filme.rs) e audio (WAV, MOD). Os arquivos de audio e video sao lidos do disco aos
//! pedacos (crate::midia); o som vai pela fila do /dev/dsp (audio::dsp),
//! que o laco principal passa para a placa.
//!
//! Teclas: espaco pausa/continua, setas pulam 5 s (WAV), Home volta ao
//! comeco; nas imagens, + e - mudam o zoom e 0 ajusta a janela.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::gui::barra::nome_do_caminho;
use crate::armazenamento::{self, Abertura};
use crate::audio::dsp;
use crate::cpu;
use crate::entrada::Botao;
use crate::entrada::teclado::{KEY_HOME, KEY_LEFT, KEY_RIGHT, KEY_SPACE};
use crate::gui::grafico::{Retangulo, Superficie};
use crate::gui::janelas::{App, EventoApp, Pedido};
use crate::midia::{self, Audio, Fonte, Reamostrador};
use crate::web::imagem::{self, Imagem};

const LARGURA: i32 = 800;
const ALTURA: i32 = 560;
const ALTURA_BARRA: i32 = 52;
const FUNDO: u32 = 0x0012_1416;
const TEXTO: u32 = 0x00E8_E8E8;
const APAGADO: u32 = 0x0090_9498;
const DESTAQUE: u32 = 0x0040_A0FF;
/// Maior imagem aberta (bytes do arquivo).
const MAX_IMAGEM: usize = 24 << 20;
/// Amostras guardadas para desenhar a onda.
const ONDA: usize = 512;

/// Um arquivo do disco lido aos pedacos.
pub struct FonteDisco {
    abertura: Abertura,
    tamanho: usize,
}

impl FonteDisco {
    pub fn abrir(caminho: &str) -> Result<FonteDisco, &'static str> {
        let (tamanho, abertura) = armazenamento::abrir_leitura(caminho).map_err(|e| e.descricao())?;
        Ok(FonteDisco { abertura, tamanho })
    }
}

impl Fonte for FonteDisco {
    fn tamanho(&self) -> usize {
        self.tamanho
    }
    fn ler(&mut self, pos: usize, destino: &mut [u8]) -> usize {
        if pos >= self.tamanho {
            return 0;
        }
        let n = destino.len().min(self.tamanho - pos);
        match armazenamento::ler_trecho(&self.abertura, pos, &mut destino[..n]) {
            Ok(()) => n,
            Err(_) => 0,
        }
    }
}

pub fn eh_midia(caminho: &str) -> bool {
    let m = caminho.to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".gif", ".avi", ".mp4", ".m4v", ".wav", ".mod", ".mp3"].iter().any(|e| m.ends_with(e))
}

/// Som tocando: decodificador, conversao de taxa e a fila do dsp.
struct Som {
    audio: Box<dyn Audio>,
    reamostra: Reamostrador,
    tocando: bool,
    acabou: bool,
    /// Ultimas amostras (para a onda na tela).
    onda: Vec<i16>,
    buf: Vec<i16>,
}

impl Som {
    fn novo(audio: Box<dyn Audio>, taxa_placa: u32) -> Som {
        let reamostra = Reamostrador::novo(audio.taxa(), taxa_placa);
        Som { audio, reamostra, tocando: true, acabou: false, onda: alloc::vec![0; ONDA], buf: alloc::vec![0; 4096] }
    }

    /// Enche a fila da placa. true = mudou algo para desenhar.
    fn alimentar(&mut self) -> bool {
        if !self.tocando || self.acabou {
            return false;
        }
        let mut mexeu = false;
        // ate ~0,5 s na fila
        while dsp::espaco() > self.buf.len() * 2 + 32 * 1024 {
            let n = self.reamostra.ler(&mut *self.audio, &mut self.buf);
            if n == 0 {
                self.acabou = true;
                break;
            }
            let mut bytes = Vec::with_capacity(n * 2);
            for s in &self.buf[..n] {
                bytes.extend_from_slice(&s.to_le_bytes());
            }
            dsp::escrever(&bytes);
            let k = n.min(ONDA);
            self.onda.rotate_left(k);
            let fim = ONDA - k;
            self.onda[fim..].copy_from_slice(&self.buf[n - k..n]);
            mexeu = true;
        }
        mexeu
    }
}

enum Conteudo {
    Imagem(Imagem),
    Video { avi: midia::avi::Avi, quadro: Option<Imagem>, indice: usize, inicio: u64, pausado_em: Option<u64> },
    #[cfg(feature = "rk322x")]
    Filme(Box<super::filme::Filme>),
    Audio,
    Erro(String),
}

pub struct Visualizador {
    titulo: String,
    caminho: String,
    conteudo: Conteudo,
    som: Option<Som>,
    descricao: String,
    /// Zoom da imagem em porcento (0 = ajustar a janela).
    zoom: u32,
    placa: bool,
    pedidos: Vec<Pedido>,
}

pub fn abrir(caminho: &str) -> Box<dyn App> {
    Box::new(Visualizador::novo(caminho))
}

fn agora_us() -> u64 {
    cpu::contador() * 1_000_000 / cpu::frequencia_timer().max(1)
}

impl Visualizador {
    pub fn novo(caminho: &str) -> Visualizador {
        let nome = nome_do_caminho(caminho);
        let mut v = Visualizador {
            titulo: format!("Mídia — {nome}"),
            caminho: String::from(caminho),
            conteudo: Conteudo::Erro(String::new()),
            som: None,
            descricao: String::new(),
            zoom: 0,
            placa: false,
            pedidos: Vec::new(),
        };
        v.carregar();
        v
    }

    fn ligar_placa(&mut self) -> u32 {
        if !self.placa && dsp::abrir() {
            self.placa = true;
            dsp::canais(2);
        }
        if self.placa { dsp::taxa(48000) } else { 0 }
    }

    fn carregar(&mut self) {
        let m = self.caminho.to_ascii_lowercase();
        let nome = String::from(nome_do_caminho(&self.caminho));
        if m.ends_with(".avi") {
            match FonteDisco::abrir(&self.caminho).and_then(|f| midia::avi::Avi::abrir(Box::new(f))) {
                Ok(mut avi) => {
                    self.descricao = avi.descricao();
                    let taxa = self.ligar_placa();
                    if taxa > 0 {
                        if let Ok(f) = FonteDisco::abrir(&self.caminho) {
                            if let Some(a) = avi.tirar_audio(Box::new(f)) {
                                self.som = Some(Som::novo(a, taxa));
                            }
                        }
                    }
                    let quadro = avi.quadro(0);
                    self.conteudo = Conteudo::Video { avi, quadro, indice: 0, inicio: agora_us(), pausado_em: None };
                }
                Err(e) => self.conteudo = Conteudo::Erro(String::from(e)),
            }
            return;
        }
        if m.ends_with(".mp4") || m.ends_with(".m4v") {
            #[cfg(feature = "rk322x")]
            match super::filme::Filme::abrir(&self.caminho) {
                Ok(f) => {
                    self.descricao = f.descricao();
                    self.conteudo = Conteudo::Filme(Box::new(f));
                }
                Err(e) => self.conteudo = Conteudo::Erro(String::from(e)),
            }
            #[cfg(not(feature = "rk322x"))]
            {
                self.conteudo = Conteudo::Erro(String::from("vídeo H.264 só na TV box (decodificador por hardware)"));
            }
            return;
        }
        if m.ends_with(".wav") || m.ends_with(".mod") || m.ends_with(".mp3") {
            let taxa = self.ligar_placa();
            let taxa_dec = if taxa > 0 { taxa } else { 48000 };
            match FonteDisco::abrir(&self.caminho).and_then(|f| midia::abrir_audio(Box::new(f), &nome, taxa_dec)) {
                Ok(a) => {
                    self.descricao = a.descricao();
                    if taxa > 0 {
                        self.som = Some(Som::novo(a, taxa));
                        self.conteudo = Conteudo::Audio;
                    } else {
                        self.conteudo = Conteudo::Erro(String::from("Não há placa de som (virtio-sound) neste aparelho"));
                    }
                }
                Err(e) => self.conteudo = Conteudo::Erro(String::from(e)),
            }
            return;
        }
        match armazenamento::ler(&self.caminho) {
            Ok(d) if d.len() > MAX_IMAGEM => self.conteudo = Conteudo::Erro(String::from("imagem grande demais")),
            Ok(d) => match imagem::decodificar_max(&d, 2560, 2560) {
                Some(img) => {
                    self.descricao = format!("{} x {} pixels", img.largura, img.altura);
                    self.conteudo = Conteudo::Imagem(img);
                }
                None => self.conteudo = Conteudo::Erro(String::from("formato de imagem não reconhecido")),
            },
            Err(e) => self.conteudo = Conteudo::Erro(String::from(e.descricao())),
        }
    }

    fn area() -> Retangulo {
        Retangulo::novo(0, 0, LARGURA, ALTURA - ALTURA_BARRA)
    }

    fn pausar(&mut self) {
        if let Some(s) = &mut self.som {
            s.tocando = !s.tocando;
        }
        if let Conteudo::Video { inicio, pausado_em, .. } = &mut self.conteudo {
            match pausado_em.take() {
                Some(p) => *inicio += agora_us() - p,
                None => *pausado_em = Some(agora_us()),
            }
        }
        #[cfg(feature = "rk322x")]
        if let Conteudo::Filme(f) = &mut self.conteudo {
            f.pausar();
        }
    }

    fn pular(&mut self, ms: i64) {
        if let Some(s) = &mut self.som {
            let alvo = (s.audio.posicao_ms() as i64 + ms).max(0) as u64;
            if s.audio.ir_para(alvo) {
                s.acabou = false;
            }
        }
    }

    fn tique(&mut self) -> bool {
        let mut mudou = self.som.as_mut().is_some_and(|s| s.alimentar());
        if let Conteudo::Video { avi, quadro, indice, inicio, pausado_em } = &mut self.conteudo {
            if pausado_em.is_none() {
                let passou = agora_us().saturating_sub(*inicio);
                let alvo = ((passou / avi.us_por_quadro.max(1) as u64) as usize).min(avi.total_quadros() - 1);
                if alvo != *indice {
                    *indice = alvo;
                    if let Some(img) = avi.quadro(alvo) {
                        *quadro = Some(img);
                    }
                    mudou = true;
                }
            }
        }
        #[cfg(feature = "rk322x")]
        if let Conteudo::Filme(f) = &mut self.conteudo {
            mudou |= f.tique();
            if let Some(e) = f.erro.take() {
                self.descricao = format!("{} - {e}", f.descricao());
            }
        }
        mudou
    }

    fn desenhar_imagem(s: &mut Superficie, img: &Imagem, area: Retangulo, zoom: u32) {
        let (il, ia) = (img.largura.max(1) as i64, img.altura.max(1) as i64);
        // escala em 1/1000: ajustada a area (sem aumentar) ou o zoom pedido
        let escala = if zoom == 0 {
            (area.largura as i64 * 1000 / il).min(area.altura as i64 * 1000 / ia).min(1000)
        } else {
            zoom as i64 * 10
        }
        .max(1);
        let (l, a) = ((il * escala / 1000) as i32, (ia * escala / 1000) as i32);
        let x0 = area.x + (area.largura - l) / 2;
        let y0 = area.y + (area.altura - a) / 2;
        let sl = s.largura() as i32;
        let px = s.pixels_mut();
        for y in y0.max(area.y)..(y0 + a).min(area.y + area.altura) {
            let sy = ((y - y0) as i64 * 1000 / escala).min(ia - 1) as usize;
            let linha = &img.px[sy * il as usize..];
            for x in x0.max(area.x)..(x0 + l).min(area.x + area.largura) {
                let sx = ((x - x0) as i64 * 1000 / escala).min(il - 1) as usize;
                px[(y * sl + x) as usize] = linha[sx] & 0x00FF_FFFF;
            }
        }
    }

    fn desenhar_onda(&self, s: &mut Superficie, r: Retangulo) {
        let Some(som) = &self.som else { return };
        let meio = r.y + r.altura / 2;
        s.retangulo(Retangulo::novo(r.x, meio, r.largura, 1), 0x0030_3438);
        let n = som.onda.len() / 2;
        let mut anterior = meio;
        for x in 0..r.largura {
            let i = (x as usize * n / r.largura as usize) * 2;
            let v = (som.onda[i] as i32 + som.onda[i + 1] as i32) / 2;
            let y = meio - v * (r.altura / 2 - 4) / 32768;
            let (a, b) = if y < anterior { (y, anterior) } else { (anterior, y) };
            s.retangulo(Retangulo::novo(r.x + x, a, 1, (b - a).max(1)), DESTAQUE);
            anterior = y;
        }
    }
}

fn tempo(ms: u64) -> String {
    format!("{}:{:02}", ms / 60000, ms / 1000 % 60)
}

impl App for Visualizador {
    fn titulo(&self) -> &str {
        &self.titulo
    }

    fn tamanho(&self) -> (usize, usize) {
        (LARGURA as usize, ALTURA as usize)
    }

    fn desenhar(&mut self, s: &mut Superficie) {
        s.limpar(FUNDO);
        let area = Self::area();
        match &self.conteudo {
            Conteudo::Imagem(img) => Self::desenhar_imagem(s, img, area, self.zoom),
            Conteudo::Video { quadro: Some(img), .. } => Self::desenhar_imagem(s, img, area, 0),
            Conteudo::Video { .. } => {}
            #[cfg(feature = "rk322x")]
            Conteudo::Filme(f) => f.desenhar(s, area),
            Conteudo::Audio => {
                s.texto(24, 24, nome_do_caminho(&self.caminho), TEXTO, 2);
                let r = Retangulo::novo(24, 90, LARGURA - 48, area.altura - 130);
                self.desenhar_onda(s, r);
            }
            Conteudo::Erro(e) => {
                s.texto(24, area.altura / 2 - 10, &format!("Não foi possível abrir: {e}"), 0x00FF_7070, 1);
            }
        }
        // barra de baixo: descricao, tempo e progresso
        let b = Retangulo::novo(0, ALTURA - ALTURA_BARRA, LARGURA, ALTURA_BARRA);
        s.retangulo(b, 0x001C_2024);
        s.texto(12, b.y + 8, &self.descricao, APAGADO, 1);
        let (pos, total, pausado) = match (&self.conteudo, &self.som) {
            (Conteudo::Video { avi, indice, pausado_em, .. }, _) => {
                (*indice as u64 * avi.us_por_quadro as u64 / 1000, Some(avi.duracao_ms()), pausado_em.is_some())
            }
            #[cfg(feature = "rk322x")]
            (Conteudo::Filme(f), _) => (f.posicao_ms(), Some(f.duracao_ms()), f.pausado()),
            (_, Some(som)) => (som.audio.posicao_ms(), som.audio.duracao_ms(), !som.tocando),
            _ => (0, None, false),
        };
        #[cfg(feature = "rk322x")]
        let filme = matches!(self.conteudo, Conteudo::Filme(_));
        #[cfg(not(feature = "rk322x"))]
        let filme = false;
        if filme || matches!(self.conteudo, Conteudo::Video { .. } | Conteudo::Audio) {
            let estado = if pausado { "pausado" } else if self.som.as_ref().is_some_and(|s| s.acabou) { "fim" } else { "tocando" };
            let t = match total {
                Some(t) => format!("{} / {}  ({estado})  espaço pausa, setas pulam", tempo(pos), tempo(t)),
                None => format!("{}  ({estado})  espaço pausa", tempo(pos)),
            };
            s.texto(12, b.y + 28, &t, TEXTO, 1);
            if let Some(t) = total.filter(|&t| t > 0) {
                let barra = Retangulo::novo(LARGURA - 312, b.y + 30, 300, 6);
                s.retangulo(barra, 0x0030_3438);
                s.retangulo(Retangulo::novo(barra.x, barra.y, (barra.largura as u64 * pos.min(t) / t) as i32, barra.altura), DESTAQUE);
            }
        } else if let Conteudo::Imagem(_) = self.conteudo {
            let z = if self.zoom == 0 { String::from("ajustada") } else { format!("{}%", self.zoom) };
            s.texto(12, b.y + 28, &format!("zoom {z}   (+ e - mudam, 0 ajusta, P = papel de parede)"), TEXTO, 1);
        }
    }

    fn evento(&mut self, e: EventoApp) -> bool {
        match e {
            EventoApp::Tique => self.tique(),
            EventoApp::Tecla { codigo: KEY_SPACE, pressionada: true } => {
                self.pausar();
                true
            }
            EventoApp::Tecla { codigo: KEY_LEFT, pressionada: true } => {
                self.pular(-5000);
                true
            }
            EventoApp::Tecla { codigo: KEY_RIGHT, pressionada: true } => {
                self.pular(5000);
                true
            }
            EventoApp::Tecla { codigo: KEY_HOME, pressionada: true } => {
                if let Some(s) = &mut self.som {
                    s.audio.ir_para(0);
                    s.acabou = false;
                }
                if let Conteudo::Video { inicio, .. } = &mut self.conteudo {
                    *inicio = agora_us();
                }
                true
            }
            // P: esta imagem vira o papel de parede (e fica gravado na config)
            EventoApp::Caractere('p' | 'P') if matches!(self.conteudo, Conteudo::Imagem(_)) => {
                self.descricao = match crate::gui::area::definir_imagem_papel(Some(&self.caminho)) {
                    Ok(()) => {
                        let mut cfg = crate::config::Config::carregar();
                        cfg.definir("papel_imagem", &self.caminho);
                        self.pedidos.push(Pedido::TemaMudou);
                        match cfg.salvar() {
                            Ok(()) => String::from("Papel de parede trocado (gravado na config)."),
                            Err(e) => format!("Papel de parede trocado, mas não gravou: {e}"),
                        }
                    }
                    Err(e) => format!("Não deu para usar de papel de parede: {e}"),
                };
                true
            }
            EventoApp::Caractere(c @ ('+' | '=' | '-' | '0')) if matches!(self.conteudo, Conteudo::Imagem(_)) => {
                let atual = if self.zoom == 0 { 100 } else { self.zoom };
                self.zoom = match c {
                    '0' => 0,
                    '-' => (atual * 4 / 5).max(10),
                    _ => (atual * 5 / 4).min(1600),
                };
                true
            }
            EventoApp::Botao { botao: Botao::Esquerdo, pressionado: true, .. } if !matches!(self.conteudo, Conteudo::Imagem(_)) => {
                self.pausar();
                true
            }
            _ => false,
        }
    }

    fn pedidos(&mut self) -> Vec<Pedido> {
        core::mem::take(&mut self.pedidos)
    }

    fn fechar(&mut self) {
        self.som = None;
        if self.placa {
            dsp::fechar();
            self.placa = false;
        }
    }
}
