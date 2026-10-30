//! Janela de um app nativo (processo com a ABI do MXOS, processo/abi.rs).
//! O app manda os pixels pelo JANELA_MOSTRAR (copiados para cá) e le os
//! eventos daqui; o gerenciador ve uma App comum.
//!
//! A tabela de janelas e global: as chamadas de sistema (que rodam com a
//! trava do kernel) criam e enchem; o laco da GUI abre as novas
//! (`para_abrir`) e as apresenta.

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::UnsafeCell;

use crate::entrada::Botao;
use crate::gui::grafico::Superficie;
use crate::gui::janelas::{App, EventoApp};

/// Tipos de evento que o app recebe ([tipo, a, b, c] em u32).
pub const EV_TECLA: u32 = 1;
pub const EV_CARACTERE: u32 = 2;
pub const EV_MOUSE: u32 = 3;
pub const EV_BOTAO: u32 = 4;
pub const EV_RODA: u32 = 5;
pub const EV_FECHAR: u32 = 6;

/// Eventos guardados no maximo por janela (o app que nao le perde os velhos).
const MAX_EVENTOS: usize = 256;

struct Janela {
    pid: u32,
    largura: usize,
    altura: usize,
    titulo: String,
    pixels: Vec<u32>,
    /// O app mandou pixels novos que a tela ainda nao viu.
    sujo: bool,
    eventos: VecDeque<[u32; 4]>,
    /// O app fechou (a App some no proximo quadro).
    fechada: bool,
}

#[derive(Default)]
struct Tabela {
    janelas: BTreeMap<u32, Janela>,
    proximo: u32,
    abrir: Vec<u32>,
}

struct Global(UnsafeCell<Option<Tabela>>);
// So com a trava do kernel (laco principal ou chamada de sistema).
unsafe impl Sync for Global {}
static TABELA: Global = Global(UnsafeCell::new(None));

fn tabela() -> &'static mut Tabela {
    unsafe { (*TABELA.0.get()).get_or_insert_with(Tabela::default) }
}

/// Nova janela do app `pid`. O laco da GUI abre na proxima volta.
pub fn criar(pid: u32, largura: usize, altura: usize, titulo: &str) -> u32 {
    let t = tabela();
    t.proximo += 1;
    let id = t.proximo;
    let fundo = crate::gui::tema::atual().fundo_janela;
    t.janelas.insert(id, Janela { pid, largura, altura, titulo: String::from(titulo), pixels: vec![fundo; largura * altura], sujo: true, eventos: VecDeque::new(), fechada: false });
    t.abrir.push(id);
    id
}

/// Mexe nos pixels da janela (largura, altura). None = nao existe.
pub fn com_pixels<R>(id: u32, f: impl FnOnce(&mut [u32], usize, usize) -> R) -> Option<R> {
    let j = tabela().janelas.get_mut(&id)?;
    j.sujo = true;
    let (l, a) = (j.largura, j.altura);
    Some(f(&mut j.pixels, l, a))
}

pub fn titulo(id: u32, texto: &str) {
    if let Some(j) = tabela().janelas.get_mut(&id) {
        j.titulo = String::from(texto);
    }
}

pub fn fechar(id: u32) {
    if let Some(j) = tabela().janelas.get_mut(&id) {
        j.fechada = true;
    }
}

pub fn tirar_evento(id: u32) -> Option<[u32; 4]> {
    tabela().janelas.get_mut(&id)?.eventos.pop_front()
}

pub fn tem_evento(id: u32) -> bool {
    tabela().janelas.get(&id).is_some_and(|j| !j.eventos.is_empty())
}

/// As janelas criadas desde a ultima volta (o laco da GUI abre).
pub fn para_abrir() -> Vec<Box<dyn App>> {
    core::mem::take(&mut tabela().abrir).into_iter().map(|id| Box::new(JanelaApp { id, titulo: String::new() }) as Box<dyn App>).collect()
}

/// Um app saiu (ou morreu): as janelas dele fecham.
pub fn processo_saiu(pid: u32) {
    for j in tabela().janelas.values_mut() {
        if j.pid == pid {
            j.fechada = true;
        }
    }
}

/// A App que o gerenciador ve.
pub struct JanelaApp {
    id: u32,
    titulo: String,
}

impl JanelaApp {
    fn empurrar(&self, e: [u32; 4]) {
        if let Some(j) = tabela().janelas.get_mut(&self.id) {
            if j.eventos.len() >= MAX_EVENTOS {
                j.eventos.pop_front();
            }
            j.eventos.push_back(e);
        }
    }
}

impl App for JanelaApp {
    fn titulo(&self) -> &str {
        &self.titulo
    }

    fn tamanho(&self) -> (usize, usize) {
        tabela().janelas.get(&self.id).map(|j| (j.largura, j.altura)).unwrap_or((16, 16))
    }

    fn desenhar(&mut self, s: &mut Superficie) {
        let Some(j) = tabela().janelas.get_mut(&self.id) else { return };
        j.sujo = false;
        let largura = s.largura().min(j.largura);
        let altura = s.altura().min(j.altura);
        let passo = s.largura();
        let destino = s.pixels_mut();
        for y in 0..altura {
            destino[y * passo..y * passo + largura].copy_from_slice(&j.pixels[y * j.largura..y * j.largura + largura]);
        }
    }

    fn evento(&mut self, e: EventoApp) -> bool {
        match e {
            EventoApp::Tecla { codigo, pressionada } => self.empurrar([EV_TECLA, codigo as u32, pressionada as u32, 0]),
            EventoApp::Caractere(c) => self.empurrar([EV_CARACTERE, c as u32, 0, 0]),
            EventoApp::Mouse { x, y, botoes } => self.empurrar([EV_MOUSE, x as u32, y as u32, botoes as u32]),
            EventoApp::Botao { x, y, botao, pressionado } => {
                let b = match botao {
                    Botao::Esquerdo => 0,
                    Botao::Direito => 1,
                    _ => 2,
                };
                self.empurrar([EV_BOTAO, x as u32, y as u32, b | (pressionado as u32) << 8])
            }
            EventoApp::Roda(d) => self.empurrar([EV_RODA, d as u32, 0, 0]),
            EventoApp::Quadro | EventoApp::Tique => {}
            _ => {}
        }
        // o titulo pode ter mudado; os pixels novos pedem redesenho
        match tabela().janelas.get(&self.id) {
            Some(j) => {
                if j.titulo != self.titulo {
                    self.titulo = j.titulo.clone();
                }
                j.sujo
            }
            None => false,
        }
    }

    fn fechar(&mut self) {
        // o app recebe o pedido de fechar (e decide: salvar, sair...)
        if let Some(j) = tabela().janelas.get_mut(&self.id) {
            j.eventos.push_back([EV_FECHAR, 0, 0, 0]);
            j.fechada = true;
        }
    }

    /// O app fechou a janela (ou saiu): ela some da tela.
    fn pedidos(&mut self) -> Vec<crate::gui::janelas::Pedido> {
        let fechada = tabela().janelas.get(&self.id).is_none_or(|j| j.fechada);
        if fechada {
            vec![crate::gui::janelas::Pedido::FecharEsta]
        } else {
            Vec::new()
        }
    }

    fn quer_quadros(&self) -> bool {
        true
    }
}
