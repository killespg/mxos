//! Painel de drivers: cada dispositivo que o MXOS achou, o driver que
//! cuida dele e o estado (drivers::listar), agrupados por categoria. Embaixo
//! ficam os detalhes do selecionado (drivers::detalhes).
//!
//! "Atualizar" (ou F5) sonda tudo de novo. A sondagem so le registradores,
//! nunca mexe num dispositivo, entao pode ser repetida a vontade.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::drivers::{self, Categoria, InfoDriver};
use crate::entrada::Botao;
use crate::fonte::{self, INTER, INTER_NEGRITO, INTER_TITULO};
use crate::gui::grafico::{self, Cor, Retangulo, Superficie};
use crate::gui::janelas::{App, EventoApp};
use crate::gui::tema::{self, Tema};
use crate::gui::widgets::{self, ALTURA_BOTAO, AcaoLista, Icone, ItemLista, Lista};

pub const TITULO: &str = "Drivers";
const LARGURA: i32 = 720;
const ALTURA: i32 = 476;
const MARGEM: i32 = 8;
const ALTURA_CABECALHO: i32 = 52;
const LARGURA_ATUALIZAR: i32 = 104;
const ALTURA_DETALHES: i32 = 160;
const COLUNAS: [&str; 5] = ["Dispositivo", "Driver", "Endereço", "IRQ", "Estado"];
/// A Lista poe o icone na primeira coluna e deixa 6 + 4 px de folga em
/// cada uma: "QEMU Virtio Keyboard" e "virtio_input" cabem inteiros.
const LARGURAS: [i32; 5] = [196, 108, 96, 40, 240];
/// Codigo evdev da F5.
const TECLA_F5: u16 = 63;
/// Icones grandes (escala 2) e o texto que vem ao lado deles.
const LADO_ICONE: i32 = 32;
const DEPOIS_DO_ICONE: i32 = LADO_ICONE + 10;
/// Margem interna do painel de detalhes e distancia entre as linhas
/// (Inter 13 px precisa de uns 17 px por linha).
const DENTRO: i32 = 12;
const PASSO_DETALHE: i32 = 17;
/// Onde o valor comeca na linha "Estado".
const COLUNA_VALOR: i32 = 64;
/// Quanto o texto secundario puxa para a cor do fundo (0 a 255).
const APAGADO: u32 = 90;

/// Para o menu iniciar.
pub fn criar() -> Box<dyn App> {
    Box::new(Drivers::novo())
}

pub struct Drivers {
    lista: Lista,
    drivers: Vec<InfoDriver>,
    /// Linhas extras do selecionado, feitas so quando a selecao muda.
    detalhes: Vec<String>,
}

fn botao_atualizar() -> Retangulo {
    let y = (ALTURA_CABECALHO - ALTURA_BOTAO) / 2;
    Retangulo::novo(LARGURA - MARGEM - LARGURA_ATUALIZAR, y, LARGURA_ATUALIZAR, ALTURA_BOTAO)
}

fn area_lista() -> Retangulo {
    let altura = ALTURA - ALTURA_CABECALHO - ALTURA_DETALHES - 2 * MARGEM;
    Retangulo::novo(MARGEM, ALTURA_CABECALHO, LARGURA - 2 * MARGEM, altura)
}

fn area_detalhes() -> Retangulo {
    Retangulo::novo(MARGEM, ALTURA - MARGEM - ALTURA_DETALHES, LARGURA - 2 * MARGEM, ALTURA_DETALHES)
}

fn icone(categoria: Categoria) -> Icone {
    match categoria {
        Categoria::Processador | Categoria::Interrupcoes | Categoria::Outro => Icone::Chip,
        Categoria::Relogio => Icone::Engrenagem,
        Categoria::Serial => Icone::Terminal,
        Categoria::Video => Icone::Monitor,
        Categoria::Som => Icone::Som,
        Categoria::Entrada => Icone::Teclado,
        Categoria::Disco => Icone::Disco,
        Categoria::Rede => Icone::Rede,
    }
}

fn item(d: &InfoDriver) -> ItemLista {
    ItemLista {
        icone: Some(icone(d.categoria)),
        colunas: vec![
            d.dispositivo.clone(),
            String::from(d.driver),
            drivers::texto_endereco(d.endereco),
            drivers::texto_irq(d.irq),
            d.estado.clone(),
        ],
    }
}

/// Cor do texto secundario (legendas, nomes dos campos).
fn apagado(t: &Tema) -> Cor {
    grafico::misturar(t.texto, t.fundo_janela, APAGADO)
}

impl Drivers {
    pub fn novo() -> Drivers {
        let mut lista = Lista::nova();
        lista.definir_colunas(&COLUNAS, &LARGURAS);
        let mut app = Drivers { lista, drivers: Vec::new(), detalhes: Vec::new() };
        app.atualizar();
        app
    }

    fn selecionado(&self) -> Option<&InfoDriver> {
        self.lista.selecionado().and_then(|i| self.drivers.get(i))
    }

    /// Sonda de novo, deixando selecionado o mesmo dispositivo (ou o primeiro).
    fn atualizar(&mut self) {
        let antes = self.selecionado().map(|d| (d.endereco, d.dispositivo.clone()));
        self.drivers = drivers::listar();
        self.lista.definir_itens(self.drivers.iter().map(item).collect());
        let mesmo = antes.and_then(|(endereco, nome)| {
            self.drivers.iter().position(|d| d.endereco == endereco && d.dispositivo == nome)
        });
        let primeiro = (!self.drivers.is_empty()).then_some(0);
        self.lista.selecionar(mesmo.or(primeiro));
        self.preparar_detalhes();
    }

    fn preparar_detalhes(&mut self) {
        self.detalhes = self.selecionado().map(drivers::detalhes).unwrap_or_default();
    }

    fn evento_lista(&mut self, e: EventoApp) -> bool {
        match self.lista.evento(e, area_lista()) {
            AcaoLista::Nada => false,
            AcaoLista::Selecionou(_) | AcaoLista::Abriu(_) => {
                self.preparar_detalhes();
                true
            }
            // Rolou (e o que mais a lista avisar): so redesenha
            _ => true,
        }
    }

    /// Icone, titulo, quantos dispositivos e o botao Atualizar.
    fn desenhar_cabecalho(&self, s: &mut Superficie, t: &Tema) {
        widgets::icone(s, MARGEM, (ALTURA_CABECALHO - LADO_ICONE) / 2, Icone::Chip, 2);
        let x = MARGEM + DEPOIS_DO_ICONE;
        s.texto_com(x, 24, "Drivers e dispositivos", t.texto, &INTER_TITULO);
        let ativos = self.drivers.iter().filter(|d| d.ativo()).count();
        let resumo = format!("{} dispositivos, {} com driver ativo", self.drivers.len(), ativos);
        s.texto_com(x, 42, &resumo, apagado(t), &INTER);
        widgets::botao(s, botao_atualizar(), "Atualizar", false);
    }

    /// Painel de baixo: o selecionado em letra grande, os campos dele e
    /// as linhas de drivers::detalhes.
    fn desenhar_detalhes(&self, s: &mut Superficie, t: &Tema) {
        let r = area_detalhes();
        widgets::painel(s, r);
        let x = r.x + DENTRO;
        let Some(d) = self.selecionado() else {
            s.texto_com(x, r.y + 24, "Selecione um dispositivo para ver os detalhes.", apagado(t), &INTER);
            return;
        };
        widgets::icone(s, x, r.y + 12, icone(d.categoria), 2);
        let largura = r.largura - 2 * DENTRO - DEPOIS_DO_ICONE;
        let nome = fonte::cortar(&d.dispositivo, largura, &INTER_TITULO);
        s.texto_com(x + DEPOIS_DO_ICONE, r.y + 28, nome, t.texto, &INTER_TITULO);
        let campos = format!(
            "{}  ·  driver {}  ·  endereço {}  ·  IRQ {}",
            d.categoria.nome(),
            d.driver,
            drivers::texto_endereco(d.endereco),
            drivers::texto_irq(d.irq)
        );
        s.texto_com(x + DEPOIS_DO_ICONE, r.y + 46, fonte::cortar(&campos, largura, &INTER), apagado(t), &INTER);

        let largura = r.largura - 2 * DENTRO;
        let mut base = r.y + 72;
        s.texto_com(x, base, "Estado", t.texto, &INTER_NEGRITO);
        let estado = fonte::cortar(&d.estado, largura - COLUNA_VALOR, &INTER);
        s.texto_com(x + COLUNA_VALOR, base, estado, t.texto, &INTER);
        for linha in &self.detalhes {
            base += PASSO_DETALHE;
            if base + 4 > r.baixo() - DENTRO / 2 {
                break; // o resto nao cabe no painel
            }
            s.texto_com(x, base, fonte::cortar(linha, largura, &INTER), t.texto, &INTER);
        }
    }
}

impl App for Drivers {
    fn titulo(&self) -> &str {
        TITULO
    }

    fn tamanho(&self) -> (usize, usize) {
        (LARGURA as usize, ALTURA as usize)
    }

    fn desenhar(&mut self, s: &mut Superficie) {
        let t = tema::atual();
        s.limpar(t.fundo_janela);
        self.desenhar_cabecalho(s, &t);
        self.lista.desenhar(s, area_lista());
        self.desenhar_detalhes(s, &t);
    }

    fn evento(&mut self, e: EventoApp) -> bool {
        match e {
            EventoApp::Botao { x, y, botao: Botao::Esquerdo, pressionado: true } if botao_atualizar().contem(x, y) => {
                self.atualizar();
                true
            }
            EventoApp::Tecla { codigo: TECLA_F5, pressionada: true } => {
                self.atualizar();
                true
            }
            EventoApp::Tique => false,
            _ => self.evento_lista(e),
        }
    }
}
