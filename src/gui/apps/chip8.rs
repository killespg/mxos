//! Janela da maquina CHIP-8 (crate::chip8): abre .ch8/.c8 do disco (ou a
//! demonstracao embutida), roda ~700 instrucoes por segundo com os timers
//! a 60 Hz medidos no relogio, e apita pela fila do som enquanto o timer
//! de som corre. Recebe EventoApp::Quadro (~100 por segundo).
//!
//! Teclado do CHIP-8 no teclado do PC:
//!   1 2 3 C      1 2 3 4
//!   4 5 6 D  ->  Q W E R
//!   7 8 9 E      A S D F
//!   A 0 B F      Z X C V
//! F5 reinicia, P pausa.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::arquivos::nome_do_caminho;
use crate::audio::dsp;
use crate::chip8::{self, Chip8};
use crate::cpu;
use crate::gui::grafico::{Retangulo, Superficie};
use crate::gui::janelas::{App, EventoApp};

const LARGURA: i32 = 640;
const ALTURA: i32 = 360;
const ALTURA_TELA: i32 = 320;
const INSTRUCOES_POR_SEGUNDO: u64 = 700;
const LIGADO: u32 = 0x0060_F0A0;
const DESLIGADO: u32 = 0x0008_140C;
const TEXTO: u32 = 0x00C8_D0C8;
const TECLA_F5: u16 = 63;
const TECLA_P: u16 = 25;

/// Um quadradinho andando na diagonal (o "8" da fonte).
const DEMO: [u8; 30] = [
    0x00, 0xE0, 0x60, 0x00, 0x61, 0x00, 0x64, 0x08, 0xF4, 0x29, 0xD0, 0x15, 0x65, 0x03, 0xF5, 0x15, 0xF5, 0x07, 0x35, 0x00,
    0x12, 0x10, 0xD0, 0x15, 0x70, 0x01, 0x71, 0x01, 0x12, 0x0A,
];

/// Codigo evdev -> tecla do CHIP-8.
fn tecla_chip8(codigo: u16) -> Option<usize> {
    Some(match codigo {
        2 => 0x1,
        3 => 0x2,
        4 => 0x3,
        5 => 0xC,
        16 => 0x4,
        17 => 0x5,
        18 => 0x6,
        19 => 0xD,
        30 => 0x7,
        31 => 0x8,
        32 => 0x9,
        33 => 0xE,
        44 => 0xA,
        45 => 0x0,
        46 => 0xB,
        47 => 0xF,
        _ => return None,
    })
}

pub struct AppChip8 {
    titulo: String,
    rom: Vec<u8>,
    maquina: Option<Chip8>,
    erro: Option<String>,
    /// cpu::contador da ultima volta.
    ultimo: u64,
    /// Restos fracionarios (em ticks do contador) de instrucoes e de timers.
    resto_instr: u64,
    resto_timer: u64,
    pausado: bool,
    placa: bool,
    fase_som: u32,
}

pub fn criar() -> Box<dyn App> {
    Box::new(AppChip8::novo(String::from("demonstração"), DEMO.to_vec()))
}

pub fn abrir(caminho: &str) -> Box<dyn App> {
    match crate::armazenamento::ler(caminho) {
        Ok(rom) => Box::new(AppChip8::novo(String::from(nome_do_caminho(caminho)), rom)),
        Err(e) => {
            let mut a = AppChip8::novo(String::from(nome_do_caminho(caminho)), Vec::new());
            a.maquina = None;
            a.erro = Some(String::from(e.descricao()));
            Box::new(a)
        }
    }
}

impl AppChip8 {
    fn novo(nome: String, rom: Vec<u8>) -> AppChip8 {
        let mut a = AppChip8 {
            titulo: format!("CHIP-8 — {nome}"),
            rom,
            maquina: None,
            erro: None,
            ultimo: cpu::contador(),
            resto_instr: 0,
            resto_timer: 0,
            pausado: false,
            placa: false,
            fase_som: 0,
        };
        a.reiniciar();
        a
    }

    fn reiniciar(&mut self) {
        match Chip8::novo(&self.rom) {
            Ok(m) => {
                self.maquina = Some(m);
                self.erro = None;
            }
            Err(_) => self.erro = Some(String::from("ROM grande demais")),
        }
        self.ultimo = cpu::contador();
    }

    /// Roda o que deu de tempo desde a ultima volta. true = tela mudou.
    fn rodar(&mut self) -> bool {
        let agora = cpu::contador();
        let freq = cpu::frequencia_timer().max(1);
        // no maximo 100 ms por volta (janela parada nao dispara depois)
        let passou = agora.saturating_sub(self.ultimo).min(freq / 10);
        self.ultimo = agora;
        let Some(m) = &mut self.maquina else { return false };
        if self.pausado {
            return false;
        }
        self.resto_instr += passou * INSTRUCOES_POR_SEGUNDO;
        let n = self.resto_instr / freq;
        self.resto_instr %= freq;
        for _ in 0..n {
            if let Err(e) = m.passo() {
                self.erro = Some(match e {
                    chip8::Erro::Instrucao(pc, op) => format!("instrução desconhecida {op:04X} em {pc:03X}"),
                    chip8::Erro::Pilha => String::from("pilha estourou"),
                    chip8::Erro::Grande => String::from("ROM grande demais"),
                });
                self.pausado = true;
                return true;
            }
        }
        self.resto_timer += passou * 60;
        let t = self.resto_timer / freq;
        self.resto_timer %= freq;
        for _ in 0..t {
            m.tique_60hz();
        }
        // apito: onda quadrada baixinha enquanto o timer de som corre
        if m.som > 0 {
            if !self.placa && dsp::abrir() {
                self.placa = true;
                dsp::canais(2);
            }
            if self.placa {
                let taxa = dsp::taxa(48000).max(1);
                let quadros = (passou * taxa as u64 / freq) as usize;
                let mut bytes = Vec::with_capacity(quadros * 4);
                for _ in 0..quadros {
                    self.fase_som = self.fase_som.wrapping_add(440 * 65536 / taxa);
                    let v: i16 = if self.fase_som & 0x8000 != 0 { 3000 } else { -3000 };
                    bytes.extend_from_slice(&v.to_le_bytes());
                    bytes.extend_from_slice(&v.to_le_bytes());
                }
                dsp::escrever(&bytes);
            }
        }
        core::mem::take(&mut m.mudou)
    }
}

impl App for AppChip8 {
    fn titulo(&self) -> &str {
        &self.titulo
    }

    fn tamanho(&self) -> (usize, usize) {
        (LARGURA as usize, ALTURA as usize)
    }

    fn quer_quadros(&self) -> bool {
        true
    }

    fn desenhar(&mut self, s: &mut Superficie) {
        s.limpar(0x0010_1210);
        if let Some(m) = &self.maquina {
            let (l, a) = m.dimensoes();
            let escala = (LARGURA / l as i32).min(ALTURA_TELA / a as i32);
            let x0 = (LARGURA - escala * l as i32) / 2;
            s.retangulo(Retangulo::novo(x0, 0, escala * l as i32, escala * a as i32), DESLIGADO);
            for y in 0..a {
                for x in 0..l {
                    if m.pixel(x, y) {
                        s.retangulo(Retangulo::novo(x0 + x as i32 * escala, y as i32 * escala, escala, escala), LIGADO);
                    }
                }
            }
        }
        let rodape = match &self.erro {
            Some(e) => format!("Erro: {e}   (F5 reinicia)"),
            None if self.pausado => String::from("pausado (P continua)   F5 reinicia"),
            None => String::from("teclas 1234 QWER ASDF ZXCV   P pausa   F5 reinicia"),
        };
        s.texto(10, ALTURA_TELA + 14, &rodape, TEXTO, 1);
    }

    fn evento(&mut self, e: EventoApp) -> bool {
        match e {
            EventoApp::Quadro | EventoApp::Tique => self.rodar(),
            EventoApp::Tecla { codigo: TECLA_F5, pressionada: true } => {
                self.pausado = false;
                self.reiniciar();
                true
            }
            EventoApp::Tecla { codigo: TECLA_P, pressionada: true } => {
                self.pausado = !self.pausado && self.erro.is_none();
                true
            }
            EventoApp::Tecla { codigo, pressionada } => {
                if let (Some(k), Some(m)) = (tecla_chip8(codigo), &mut self.maquina) {
                    m.tecla(k, pressionada);
                }
                false
            }
            _ => false,
        }
    }

    fn fechar(&mut self) {
        if self.placa {
            dsp::fechar();
        }
    }
}
