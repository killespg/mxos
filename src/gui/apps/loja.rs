//! Loja: instala, atualiza, abre e remove os apps em pacotes .mxa.
//!
//! O catalogo e o indice.json do repositorio mxos-apps no GitHub (a chave
//! `loja` do /config/mxos.cfg troca o endereco):
//!
//!   {"apps": [{"id": "calculadora", "nome": "Calculadora", "versao": "1.0.0",
//!              "descricao": "...", "autor": "...", "licenca": "...",
//!              "icone": "Calculadora", "tamanho": 123456,
//!              "pacote": "pacotes/calculadora.mxa"}]}
//!
//! `pacote` e relativo ao endereco do indice. Adicionar um app ao
//! repositorio (o pacote e a linha no indice) faz ele aparecer aqui. Um
//! .mxa aberto no gerenciador de arquivos tambem vem para ca (instalar
//! do disco). Baixar e instalar rodam no Tique seguinte, depois de um
//! quadro com o aviso (a area de trabalho espera enquanto isso).

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::fonte::{self, INTER, INTER_NEGRITO, INTER_TITULO};
use crate::gui::barra::{ALTURA_BARRA, MARGEM, barra_estado, cor_barra, desenhar_aviso, pintar_barra, tamanho_texto, texto_sobre};
use crate::gui::grafico::{self, Retangulo, Superficie};
use crate::gui::icones;
use crate::gui::janelas::{App, EventoApp, Pedido};
use crate::gui::tema::{self, Tema};
use crate::gui::widgets::{self, ALTURA_BOTAO, cor_campo, cor_linha, cor_texto_fraco};
use crate::entrada::Botao;
use crate::json::{self, Json};
use crate::pacotes::{self, Manifesto};

/// Catalogo de fabrica.
pub const INDICE_PADRAO: &str = "https://raw.githubusercontent.com/killespg/mxos-apps/main/indice.json";

const LARGURA: i32 = 760;
const ALTURA: i32 = 520;
const ALTURA_ESTADO: i32 = 24;
const ALTURA_CARTAO: i32 = 84;
const ESPACO: i32 = 10;
const LADO_ICONE: i32 = 48;
const LARGURA_ACAO: i32 = 96;

/// Um app do catalogo.
#[derive(Clone)]
struct Oferta {
    id: String,
    nome: String,
    versao: String,
    descricao: String,
    autor: String,
    licenca: String,
    icone: String,
    tamanho: u64,
    pacote: String,
}

/// Uma linha da lista: o que o catalogo oferece e/ou o que esta instalado.
struct Linha {
    oferta: Option<Oferta>,
    instalado: Option<Manifesto>,
}

impl Linha {
    fn id(&self) -> &str {
        self.oferta.as_ref().map(|o| o.id.as_str()).or(self.instalado.as_ref().map(|m| m.id.as_str())).unwrap_or("")
    }

    fn tem_atualizacao(&self) -> bool {
        matches!((&self.oferta, &self.instalado), (Some(o), Some(m)) if pacotes::versao_maior(&o.versao, &m.versao))
    }
}

#[derive(Clone, PartialEq, Eq)]
enum Acao {
    Recarregar,
    Instalar(String),
    Abrir(String),
    Remover(String),
    InstalarArquivo,
}

/// Trabalho demorado para o proximo Tique (o aviso aparece antes).
enum Tarefa {
    Catalogo,
    Instalar(String),
    Remover(String),
    InstalarArquivo(String),
}

pub struct Loja {
    url: String,
    ofertas: Vec<Oferta>,
    linhas: Vec<Linha>,
    estado: String,
    aviso: Option<String>,
    tarefa: Option<Tarefa>,
    /// Pacote do disco esperando confirmacao.
    arquivo: Option<(String, Manifesto)>,
    rolagem: i32,
    botoes: Vec<(Retangulo, Acao)>,
    apertado: Option<Acao>,
    pedidos: Vec<Pedido>,
}

pub fn criar() -> Box<dyn App> {
    Box::new(Loja::nova())
}

/// A Loja oferecendo instalar o .mxa `caminho` do disco.
pub fn abrir_arquivo(caminho: &str) -> Box<dyn App> {
    let mut loja = Loja::nova();
    match crate::armazenamento::ler(caminho).map_err(|e| e.descricao().to_string()).and_then(|d| pacotes::manifesto_do_mxa(&d)) {
        Ok(m) => {
            loja.estado = format!("Pacote {} {} pronto para instalar", m.nome, m.versao);
            loja.arquivo = Some((String::from(caminho), m));
        }
        Err(e) => loja.estado = format!("{caminho}: {e}"),
    }
    Box::new(loja)
}

impl Loja {
    pub fn nova() -> Loja {
        let url = crate::config::Config::carregar().obter("loja").filter(|u| !u.is_empty()).unwrap_or(INDICE_PADRAO).to_string();
        let mut loja = Loja {
            url,
            ofertas: Vec::new(),
            linhas: Vec::new(),
            estado: String::new(),
            aviso: Some(String::from("Buscando o catálogo ...")),
            tarefa: Some(Tarefa::Catalogo),
            arquivo: None,
            rolagem: 0,
            botoes: Vec::new(),
            apertado: None,
            pedidos: Vec::new(),
        };
        loja.montar_linhas();
        loja
    }

    /// Junta catalogo e instalados (os instalados que sairam do catalogo
    /// continuam na lista, para abrir e remover).
    fn montar_linhas(&mut self) {
        let mut instalados = pacotes::instalados();
        let mut linhas: Vec<Linha> = self
            .ofertas
            .iter()
            .map(|o| {
                let i = instalados.iter().position(|m| m.id == o.id);
                Linha { oferta: Some(o.clone()), instalado: i.map(|i| instalados.remove(i)) }
            })
            .collect();
        linhas.extend(instalados.into_iter().map(|m| Linha { oferta: None, instalado: Some(m) }));
        self.linhas = linhas;
    }

    fn base_url(&self) -> &str {
        self.url.rsplit_once('/').map(|(b, _)| b).unwrap_or(&self.url)
    }

    fn baixar_catalogo(&mut self) {
        match crate::rede::http_get(&self.url, 30_000) {
            Ok(r) if r.status == 200 => match json::ler(&r.corpo) {
                Some(j) => {
                    let campo = |o: &Json, n: &str| o.campo(n).and_then(Json::texto).unwrap_or("").to_string();
                    self.ofertas = j
                        .campo("apps")
                        .map(|a| a.lista())
                        .unwrap_or(&[])
                        .iter()
                        .filter(|o| !campo(o, "id").is_empty() && !campo(o, "pacote").is_empty())
                        .map(|o| Oferta {
                            id: campo(o, "id"),
                            nome: campo(o, "nome"),
                            versao: campo(o, "versao"),
                            descricao: campo(o, "descricao"),
                            autor: campo(o, "autor"),
                            licenca: campo(o, "licenca"),
                            icone: campo(o, "icone"),
                            tamanho: match o.campo("tamanho") {
                                Some(Json::Num(n)) => *n as u64,
                                _ => 0,
                            },
                            pacote: campo(o, "pacote"),
                        })
                        .collect();
                    self.estado = format!("{} apps no catálogo", self.ofertas.len());
                }
                None => self.estado = String::from("O catálogo veio estragado (não é JSON)"),
            },
            Ok(r) => self.estado = format!("O catálogo respondeu HTTP {}", r.status),
            Err(e) => self.estado = format!("Sem catálogo: {}", e.descricao()),
        }
        self.montar_linhas();
    }

    fn instalar(&mut self, id: &str) {
        let Some(o) = self.ofertas.iter().find(|o| o.id == id).cloned() else { return };
        let url = if o.pacote.starts_with("http") { o.pacote.clone() } else { format!("{}/{}", self.base_url(), o.pacote) };
        let dados = match crate::rede::http_get(&url, 120_000) {
            Ok(r) if r.status == 200 => r.corpo,
            Ok(r) => {
                self.estado = format!("{}: o servidor respondeu HTTP {}", o.nome, r.status);
                return;
            }
            Err(e) => {
                self.estado = format!("{}: {}", o.nome, e.descricao());
                return;
            }
        };
        self.instalar_dados(&dados);
    }

    fn instalar_dados(&mut self, dados: &[u8]) {
        match pacotes::instalar(dados) {
            Ok(m) => {
                self.estado = format!("{} {} instalado", m.nome, m.versao);
                self.pedidos.push(Pedido::PacotesMudaram);
            }
            Err(e) => self.estado = format!("Não instalou: {e}"),
        }
        self.montar_linhas();
    }

    fn fazer(&mut self, acao: Acao) {
        match acao {
            Acao::Recarregar => {
                self.aviso = Some(String::from("Buscando o catálogo ..."));
                self.tarefa = Some(Tarefa::Catalogo);
            }
            Acao::Instalar(id) => {
                let nome = self.linhas.iter().find(|l| l.id() == id).and_then(|l| l.oferta.as_ref()).map(|o| o.nome.clone()).unwrap_or_default();
                self.aviso = Some(format!("Baixando {nome} ..."));
                self.tarefa = Some(Tarefa::Instalar(id));
            }
            Acao::Abrir(id) => match pacotes::abrir(&id, "") {
                Ok(_) => self.estado = String::new(),
                Err(e) => self.estado = format!("Não abriu: {e}"),
            },
            Acao::Remover(id) => {
                self.aviso = Some(String::from("Removendo ..."));
                self.tarefa = Some(Tarefa::Remover(id));
            }
            Acao::InstalarArquivo => {
                if let Some((caminho, m)) = self.arquivo.take() {
                    self.aviso = Some(format!("Instalando {} ...", m.nome));
                    self.tarefa = Some(Tarefa::InstalarArquivo(caminho));
                }
            }
        }
    }

    fn rodar_tarefa(&mut self) {
        let Some(tarefa) = self.tarefa.take() else { return };
        match tarefa {
            Tarefa::Catalogo => self.baixar_catalogo(),
            Tarefa::Instalar(id) => self.instalar(&id),
            Tarefa::InstalarArquivo(caminho) => match crate::armazenamento::ler(&caminho) {
                Ok(d) => self.instalar_dados(&d),
                Err(e) => self.estado = format!("{caminho}: {}", e.descricao()),
            },
            Tarefa::Remover(id) => {
                match pacotes::desinstalar(&id) {
                    Ok(()) => {
                        self.estado = String::from("App removido");
                        self.pedidos.push(Pedido::PacotesMudaram);
                    }
                    Err(e) => self.estado = format!("Não removeu: {e}"),
                }
                self.montar_linhas();
            }
        }
        self.aviso = None;
    }

    fn area_lista() -> Retangulo {
        Retangulo::novo(0, ALTURA_BARRA, LARGURA, ALTURA - ALTURA_BARRA - ALTURA_ESTADO)
    }

    fn altura_conteudo(&self) -> i32 {
        let extra = if self.arquivo.is_some() { ALTURA_CARTAO + ESPACO } else { 0 };
        extra + self.linhas.len() as i32 * (ALTURA_CARTAO + ESPACO) + ESPACO
    }

    fn rolar(&mut self, d: i32) -> bool {
        let maximo = (self.altura_conteudo() - Self::area_lista().altura).max(0);
        let nova = (self.rolagem + d).clamp(0, maximo);
        let mudou = nova != self.rolagem;
        self.rolagem = nova;
        mudou
    }

    fn botao(&mut self, s: &mut Superficie, r: Retangulo, texto: &str, principal: bool, acao: Acao, recorte: Retangulo) {
        if r.intersecao(&recorte) != r {
            return; // so botoes inteiros (o clique tem de cair no que se ve)
        }
        widgets::botao(s, r, texto, principal);
        if self.apertado.as_ref() == Some(&acao) {
            crate::gui::area::tingir(s, r, 0, 40);
        }
        self.botoes.push((r, acao));
    }

    fn desenhar_cartao(&mut self, s: &mut Superficie, r: Retangulo, i: usize, recorte: Retangulo, t: &Tema) {
        let l = &self.linhas[i];
        let (nome, versao, descricao, autor, licenca, icone, tamanho) = match (&l.oferta, &l.instalado) {
            (Some(o), _) => (o.nome.clone(), o.versao.clone(), o.descricao.clone(), o.autor.clone(), o.licenca.clone(), o.icone.clone(), o.tamanho),
            (None, Some(m)) => (m.nome.clone(), m.versao.clone(), m.descricao.clone(), m.autor.clone(), m.licenca.clone(), m.icone.clone(), 0),
            _ => return,
        };
        let id = l.id().to_string();
        let instalado = l.instalado.as_ref().map(|m| m.versao.clone());
        let atualizar = l.tem_atualizacao();
        let no_catalogo = l.oferta.is_some();
        s.retangulo_arredondado(r, 10, cor_campo(t));
        let fundo = cor_campo(t);
        let forte = texto_sobre(fundo, t.texto);
        let fraco = grafico::misturar(forte, fundo, 110);
        icones::icone_app(s, r.x + 16, r.y + (r.altura - LADO_ICONE) / 2, pacotes::icone(&icone), LADO_ICONE);
        let x = r.x + 16 + LADO_ICONE + 16;
        let direita = r.direita() - 16 - 2 * LARGURA_ACAO - 8;
        s.texto_com(x, r.y + 26, fonte::cortar(&nome, direita - x, &INTER_TITULO), forte, &INTER_TITULO);
        let mut detalhe = format!("versão {versao}");
        if let Some(v) = &instalado {
            if atualizar {
                detalhe = format!("instalada {v} · nova {versao}");
            } else {
                detalhe += " · instalado";
            }
        }
        if !autor.is_empty() {
            detalhe += &format!(" · {autor}");
        }
        if !licenca.is_empty() {
            detalhe += &format!(" · {licenca}");
        }
        if tamanho > 0 {
            detalhe += &format!(" · {}", tamanho_texto(tamanho));
        }
        s.texto_com(x, r.y + 46, fonte::cortar(&detalhe, direita - x, &INTER), fraco, &INTER);
        s.texto_com(x, r.y + 66, fonte::cortar(&descricao, direita - x, &INTER), forte, &INTER);
        let y = r.y + (r.altura - ALTURA_BOTAO) / 2;
        let a1 = Retangulo::novo(r.direita() - 16 - LARGURA_ACAO, y, LARGURA_ACAO, ALTURA_BOTAO);
        let a2 = Retangulo::novo(a1.x - 8 - LARGURA_ACAO, y, LARGURA_ACAO, ALTURA_BOTAO);
        match instalado {
            None => self.botao(s, a1, "Instalar", true, Acao::Instalar(id), recorte),
            Some(_) => {
                if atualizar && no_catalogo {
                    self.botao(s, a1, "Atualizar", true, Acao::Instalar(id.clone()), recorte);
                } else {
                    self.botao(s, a1, "Abrir", true, Acao::Abrir(id.clone()), recorte);
                }
                self.botao(s, a2, "Remover", false, Acao::Remover(id), recorte);
            }
        }
    }
}

impl App for Loja {
    fn titulo(&self) -> &str {
        "Loja"
    }

    fn tamanho(&self) -> (usize, usize) {
        (LARGURA as usize, ALTURA as usize)
    }

    fn desenhar(&mut self, s: &mut Superficie) {
        let t = tema::atual();
        s.limpar(t.fundo_janela);
        self.botoes.clear();
        // barra de cima: titulo e recarregar
        let barra = Retangulo::novo(0, 0, LARGURA, ALTURA_BARRA);
        pintar_barra(s, barra, cor_barra(&t), &t);
        let fundo_barra = cor_barra(&t);
        s.texto_com(MARGEM + 6, (ALTURA_BARRA + 10) / 2 + 2, "Apps para o MXOS", texto_sobre(fundo_barra, t.texto), &INTER_NEGRITO);
        let r = Retangulo::novo(LARGURA - MARGEM - 110, MARGEM, 110, ALTURA_BOTAO);
        self.botao(s, r, "Recarregar", false, Acao::Recarregar, barra);

        let area = Self::area_lista();
        let mut y = area.y + ESPACO - self.rolagem;
        let largura = LARGURA - 2 * ESPACO;
        if let Some((caminho, m)) = self.arquivo.clone() {
            let r = Retangulo::novo(ESPACO, y, largura, ALTURA_CARTAO);
            if !r.intersecao(&area).vazio() {
                s.retangulo_arredondado(r, 10, grafico::misturar(cor_campo(&t), t.destaque, 40));
                let forte = texto_sobre(cor_campo(&t), t.texto);
                icones::icone_app(s, r.x + 16, r.y + (r.altura - LADO_ICONE) / 2, pacotes::icone(&m.icone), LADO_ICONE);
                let x = r.x + 80;
                s.texto_com(x, r.y + 30, &format!("Instalar {} {}?", m.nome, m.versao), forte, &INTER_TITULO);
                s.texto_com(x, r.y + 54, fonte::cortar(&caminho, r.largura - 220, &INTER), cor_texto_fraco(&t), &INTER);
                let b = Retangulo::novo(r.direita() - 16 - LARGURA_ACAO, r.y + (r.altura - ALTURA_BOTAO) / 2, LARGURA_ACAO, ALTURA_BOTAO);
                self.botao(s, b, "Instalar", true, Acao::InstalarArquivo, area);
            }
            y += ALTURA_CARTAO + ESPACO;
        }
        for i in 0..self.linhas.len() {
            let r = Retangulo::novo(ESPACO, y, largura, ALTURA_CARTAO);
            if !r.intersecao(&area).vazio() {
                self.desenhar_cartao(s, r, i, area, &t);
            }
            y += ALTURA_CARTAO + ESPACO;
        }
        if self.linhas.is_empty() && self.arquivo.is_none() && self.aviso.is_none() {
            let texto = "Nenhum app por aqui ainda. Confira a internet e clique em Recarregar.";
            let largura_texto = Superficie::largura_texto(texto, 1);
            s.texto(area.x + (area.largura - largura_texto) / 2, area.y + area.altura / 2 - 4, texto, cor_texto_fraco(&t), 1);
        }
        // o que passou por cima da barra e do rodape
        pintar_barra(s, barra, fundo_barra, &t);
        s.texto_com(MARGEM + 6, (ALTURA_BARRA + 10) / 2 + 2, "Apps para o MXOS", texto_sobre(fundo_barra, t.texto), &INTER_NEGRITO);
        let r = Retangulo::novo(LARGURA - MARGEM - 110, MARGEM, 110, ALTURA_BOTAO);
        widgets::botao(s, r, "Recarregar", false);
        let y_estado = barra_estado(s, &t);
        let estado = if self.estado.is_empty() { format!("Catálogo: {}", self.url) } else { self.estado.clone() };
        s.texto(MARGEM + 4, y_estado, fonte::cortar(&estado, LARGURA - 2 * MARGEM - 8, &INTER), cor_texto_fraco(&t), 1);
        s.retangulo(Retangulo::novo(0, area.y, LARGURA, 1), cor_linha(&t));
        if let Some(aviso) = &self.aviso {
            desenhar_aviso(s, area, aviso, &t);
        }
    }

    fn evento(&mut self, e: EventoApp) -> bool {
        match e {
            EventoApp::Tique => {
                if self.tarefa.is_some() && self.aviso.is_some() {
                    self.rodar_tarefa();
                    return true;
                }
                false
            }
            EventoApp::Roda(d) => self.rolar(-d * 40),
            EventoApp::Botao { x, y, botao: Botao::Esquerdo, pressionado: true } => {
                if self.aviso.is_some() {
                    return false;
                }
                let acao = self.botoes.iter().find(|(r, _)| r.contem(x, y)).map(|(_, a)| a.clone());
                let mudou = acao != self.apertado;
                self.apertado = acao;
                mudou
            }
            EventoApp::Botao { x, y, botao: Botao::Esquerdo, pressionado: false } => {
                let Some(acao) = self.apertado.take() else { return false };
                if self.botoes.iter().any(|(r, a)| *a == acao && r.contem(x, y)) {
                    self.fazer(acao);
                }
                true
            }
            EventoApp::Tecla { codigo, pressionada: true } => match codigo {
                crate::entrada::teclado::KEY_PAGEDOWN => self.rolar(Self::area_lista().altura - 40),
                crate::entrada::teclado::KEY_PAGEUP => self.rolar(-(Self::area_lista().altura - 40)),
                crate::entrada::teclado::KEY_DOWN => self.rolar(40),
                crate::entrada::teclado::KEY_UP => self.rolar(-40),
                _ => false,
            },
            _ => false,
        }
    }

    fn pedidos(&mut self) -> Vec<Pedido> {
        core::mem::take(&mut self.pedidos)
    }
}
