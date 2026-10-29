//! Modo quiosque: um painel do tamanho da tela com uma pagina da web (o
//! motor do navegador) e, ao lado, relogio, tempo (previsao por HTTPS),
//! calendario do mes e notas.
//!
//! Configuracoes (comando `config`):
//!   quiosque_url        pagina mostrada (padrao: a pagina inicial)
//!   quiosque_cidade     cidade da previsao (padrao: Sao Paulo)
//!   quiosque_recarregar minutos entre recargas da pagina (padrao 10)
//!   quiosque            "sim" abre o quiosque sozinho ao ligar
//! As notas sao o arquivo /config/notas.txt (relido a cada minuto).
//!
//! A previsao vem do wttr.in (JSON), pelo HTTPS/TLS do proprio MXOS.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::config::Config;
use crate::gui::area;
use crate::gui::grafico::{Retangulo, Superficie};
use crate::gui::janelas::{App, EventoApp};
use crate::gui::tema;
use crate::json::{self, Json};
use crate::navegador::web;

const LARGURA_LADO: i32 = 340;
const FUNDO: u32 = 0x0010_1820;
const CARTAO: u32 = 0x0018_2430;
const TEXTO: u32 = 0x00EE_F2F6;
const APAGADO: u32 = 0x0090_A0B0;
const DESTAQUE: u32 = 0x0040_A0FF;
const MINUTOS_TEMPO: u64 = 30;

pub fn criar() -> Box<dyn App> {
    Box::new(Quiosque::novo())
}

/// Tamanho do conteudo para ocupar a tela (fora a barra de tarefas e o titulo).
pub fn tamanho_tela_cheia() -> (i32, i32) {
    let (l, a) = tema::tamanho_da_tela();
    ((l - 4).max(640), (a - tema::ALTURA_BARRA_TAREFAS - tema::ALTURA_TITULO - 4).max(400))
}

struct Previsao {
    agora: String,
    dias: Vec<String>,
}

pub struct Quiosque {
    largura: i32,
    altura: i32,
    url: String,
    cidade: String,
    recarregar_min: u64,
    pagina: Option<web::PaginaWeb>,
    rolagem: i32,
    previsao: Option<Result<Previsao, String>>,
    notas: Vec<String>,
    /// Segundos (relogio local) da ultima carga de cada coisa.
    pagina_em: u64,
    previsao_em: u64,
    notas_em: u64,
    relogio: u64,
    /// Primeiro quadro ja saiu (a carga trava: espera o aviso aparecer).
    mostrou: bool,
}

impl Quiosque {
    pub fn novo() -> Quiosque {
        let cfg = Config::carregar();
        let (largura, altura) = tamanho_tela_cheia();
        let url = cfg.obter("quiosque_url").or_else(|| cfg.obter("pagina_inicial")).unwrap_or("disco:/paginas/index.html");
        Quiosque {
            largura,
            altura,
            url: crate::navegador::normalizar_url(url),
            cidade: String::from(cfg.obter("quiosque_cidade").unwrap_or("Sao Paulo")),
            recarregar_min: cfg.obter("quiosque_recarregar").and_then(|v| v.trim().parse().ok()).unwrap_or(10).max(1),
            pagina: None,
            rolagem: 0,
            previsao: None,
            notas: Vec::new(),
            pagina_em: 0,
            previsao_em: 0,
            notas_em: 0,
            relogio: 0,
            mostrou: false,
        }
    }

    fn area_pagina(&self) -> Retangulo {
        Retangulo::novo(0, 0, self.largura - LARGURA_LADO, self.altura)
    }

    fn carregar_pagina(&mut self, agora: u64) {
        let a = self.area_pagina();
        self.pagina = Some(web::abrir(&self.url, a.largura, a.altura));
        self.pagina_em = agora;
        self.rolagem = 0;
    }

    fn carregar_previsao(&mut self, agora: u64) {
        self.previsao_em = agora;
        let cidade: String = self.cidade.chars().map(|c| if c == ' ' { '+' } else { c }).collect();
        let url = format!("https://wttr.in/{cidade}?format=j1&lang=pt");
        self.previsao = Some(match crate::rede::http_get(&url, 15_000) {
            Ok(r) if r.status == 200 => ler_previsao(&r.corpo).ok_or_else(|| String::from("resposta inválida")),
            Ok(r) => Err(format!("HTTP {}", r.status)),
            Err(e) => Err(String::from(e.descricao())),
        });
    }

    fn carregar_notas(&mut self, agora: u64) {
        self.notas_em = agora;
        self.notas = match crate::armazenamento::ler("/config/notas.txt") {
            Ok(d) => String::from_utf8_lossy(&d).lines().map(String::from).collect(),
            Err(_) => alloc::vec![String::from("(crie /config/notas.txt para ver notas aqui)")],
        };
    }

    fn tique(&mut self) -> bool {
        let agora = area::agora_local();
        let mut mudou = false;
        if !self.mostrou {
            return false;
        }
        if self.notas_em == 0 || agora >= self.notas_em + 60 {
            self.carregar_notas(agora);
            mudou = true;
        }
        if self.previsao.is_none() || agora >= self.previsao_em + MINUTOS_TEMPO * 60 {
            self.carregar_previsao(agora);
            mudou = true;
        }
        if self.pagina.is_none() || agora >= self.pagina_em + self.recarregar_min * 60 {
            self.carregar_pagina(agora);
            mudou = true;
        } else if let Some(p) = &mut self.pagina {
            mudou |= web::tique(p);
        }
        if agora != self.relogio {
            self.relogio = agora;
            mudou = true;
        }
        mudou
    }

    fn desenhar_lado(&self, s: &mut Superficie) {
        let x0 = self.largura - LARGURA_LADO;
        s.retangulo(Retangulo::novo(x0, 0, LARGURA_LADO, self.altura), FUNDO);
        let x = x0 + 16;
        let l = LARGURA_LADO - 32;
        // relogio
        let seg = self.relogio;
        s.texto(x, 20, &format!("{:02}:{:02}:{:02}", seg / 3600 % 24, seg / 60 % 60, seg % 60), TEXTO, 4);
        s.texto(x, 92, &area::data(seg), APAGADO, 1);
        // tempo
        let mut y = 120;
        let cartao = |s: &mut Superficie, y: i32, a: i32| s.retangulo(Retangulo::novo(x - 6, y, l + 12, a), CARTAO);
        cartao(s, y, 112);
        s.texto(x, y + 10, &format!("Tempo em {}", self.cidade), DESTAQUE, 1);
        match &self.previsao {
            Some(Ok(p)) => {
                s.texto(x, y + 30, &crate::fonte::cortar(&p.agora, l, &crate::fonte::INTER), TEXTO, 1);
                for (k, d) in p.dias.iter().enumerate() {
                    s.texto(x, y + 54 + k as i32 * 18, d, APAGADO, 1);
                }
            }
            Some(Err(e)) => {
                s.texto(x, y + 30, &format!("sem previsão: {e}"), APAGADO, 1);
            }
            None => {
                s.texto(x, y + 30, "carregando…", APAGADO, 1);
            }
        }
        // calendario
        y += 124;
        cartao(s, y, 170);
        self.desenhar_mes(s, x, y + 10, l);
        // notas
        y += 182;
        let resto = self.altura - y - 12;
        if resto > 40 {
            cartao(s, y, resto);
            s.texto(x, y + 10, "Notas", DESTAQUE, 1);
            for (k, n) in self.notas.iter().take(((resto - 34) / 16).max(0) as usize).enumerate() {
                s.texto(x, y + 30 + k as i32 * 16, crate::fonte::cortar(n, l, &crate::fonte::INTER), TEXTO, 1);
            }
        }
    }

    fn desenhar_mes(&self, s: &mut Superficie, x: i32, y: i32, l: i32) {
        let dias = (self.relogio / 86_400) as i64;
        let (ano, mes, dia) = area::civil(dias);
        const MESES: [&str; 12] = ["janeiro", "fevereiro", "março", "abril", "maio", "junho", "julho", "agosto", "setembro", "outubro", "novembro", "dezembro"];
        s.texto(x, y, &format!("{} de {ano}", MESES[(mes as usize - 1) % 12]), DESTAQUE, 1);
        let coluna = l / 7;
        for (k, d) in ["D", "S", "T", "Q", "Q", "S", "S"].iter().enumerate() {
            s.texto(x + k as i32 * coluna + 8, y + 22, d, APAGADO, 1);
        }
        // dia da semana do dia 1 (1970-01-01 foi quinta = 4)
        let primeiro = dias - (dia as i64 - 1);
        let semana = (primeiro + 4).rem_euclid(7) as i32;
        let no_mes = match mes {
            2 if (ano % 4 == 0 && ano % 100 != 0) || ano % 400 == 0 => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        };
        for d in 1..=no_mes {
            let pos = semana + d - 1;
            let (cx, cy) = (x + (pos % 7) * coluna, y + 40 + (pos / 7) * 20);
            if d == dia as i32 {
                s.retangulo(Retangulo::novo(cx + 2, cy - 4, coluna - 4, 18), DESTAQUE);
            }
            s.texto(cx + 8, cy, &format!("{d}"), TEXTO, 1);
        }
    }
}

/// O JSON do wttr.in: agora e os proximos dias.
fn ler_previsao(corpo: &[u8]) -> Option<Previsao> {
    let d = json::ler(corpo)?;
    let atual = d.campo("current_condition")?.lista().first()?;
    let texto = |j: &Json, k: &str| j.campo(k).and_then(Json::texto).map(String::from).unwrap_or_default();
    let desc = atual
        .campo("lang_pt")
        .or_else(|| atual.campo("weatherDesc"))
        .and_then(|v| v.lista().first())
        .map(|v| texto(v, "value"))
        .unwrap_or_default();
    let agora = format!("{} °C, {}  (umidade {}%)", texto(atual, "temp_C"), desc.trim(), texto(atual, "humidity"));
    let dias = d
        .campo("weather")
        .map(Json::lista)
        .unwrap_or(&[])
        .iter()
        .take(3)
        .map(|w| {
            let data = texto(w, "date");
            let curta = data.get(5..).unwrap_or(&data).replace('-', "/");
            format!("{curta}: mín {} °C, máx {} °C", texto(w, "mintempC"), texto(w, "maxtempC"))
        })
        .collect();
    Some(Previsao { agora, dias })
}

impl App for Quiosque {
    fn titulo(&self) -> &str {
        "Quiosque"
    }

    fn tamanho(&self) -> (usize, usize) {
        (self.largura as usize, self.altura as usize)
    }

    fn desenhar(&mut self, s: &mut Superficie) {
        s.limpar(FUNDO);
        let a = self.area_pagina();
        match &self.pagina {
            Some(p) => web::desenhar(p, s, a, self.rolagem),
            None => {
                s.texto(24, a.altura / 2, &format!("Carregando {}…", self.url), APAGADO, 1);
            }
        }
        self.desenhar_lado(s);
        self.mostrou = true;
    }

    fn evento(&mut self, e: EventoApp) -> bool {
        match e {
            EventoApp::Tique => self.tique(),
            EventoApp::Roda(passos) => {
                let max = self.pagina.as_ref().map_or(0, |p| (p.altura - self.area_pagina().altura).max(0));
                let novo = (self.rolagem - passos * 60).clamp(0, max);
                let mudou = novo != self.rolagem;
                self.rolagem = novo;
                mudou
            }
            // F5 recarrega tudo
            EventoApp::Tecla { codigo: 63, pressionada: true } => {
                self.pagina = None;
                self.previsao = None;
                self.notas_em = 0;
                self.tique();
                true
            }
            _ => false,
        }
    }
}
