//! Entrada: teclado e mouse/tablet pelo virtio-input do QEMU.
//!   - virtio_input: driver de um dispositivo (eventq, configuracao)
//!   - teclado:      codigo de tecla + Shift/CapsLock + layout -> caractere
//!
//! O `Entrada` acha todos os virtio-input nos 32 slots do QEMU virt, le os
//! eventos crus deles e devolve eventos prontos para a interface grafica:
//! teclas, caracteres, posicao do ponteiro em pixels, botoes e roda.
//!
//! O ponteiro so e anunciado no EV_SYN (fim de um grupo): varios ABS_X/ABS_Y
//! viram um Evento::Mouse so, e os botoes do mesmo grupo saem DEPOIS da
//! posicao nova, para o clique cair no lugar certo.
//!
//! Tudo por polling (`proximo` nao bloqueia). So existe no QEMU.

// Nem todo item publico e usado pela interface.
#![allow(dead_code)]

pub mod teclado;
pub mod virtio_input;

use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::virtio::virtio_mmio::{BASE_QEMU_VIRT, PASSO_QEMU_VIRT, SLOTS_QEMU_VIRT, VirtioMmio};
use teclado::Teclado;
use virtio_input::{
    ABS_X, ABS_Y, BTN_GEAR_DOWN, BTN_GEAR_UP, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, DispositivoEntrada, EV_ABS, EV_KEY,
    EV_REL, EV_SYN, EventoCru, PRIMEIRO_BOTAO, REL_WHEEL, ULTIMO_BOTAO, REL_X, REL_Y, SYN_REPORT, TIPO_ENTRADA,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Botao {
    Esquerdo,
    Direito,
    Meio,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Evento {
    /// Codigo evdev cru (KEY_*), toda tecla: setas, F1..F12, modificadores...
    Tecla { codigo: u16, pressionada: bool },
    /// Tambem sai no aperto (e na repeticao) das teclas que escrevem algo:
    /// letras (com acento no ABNT2), simbolos, 13 Enter, 8 Backspace,
    /// 9 Tab, 27 Esc.
    Caractere(char),
    /// Posicao absoluta do ponteiro em pixels da tela.
    Mouse { x: i32, y: i32 },
    Botao { botao: Botao, pressionado: bool },
    /// Roda do mouse, positivo = para cima.
    Roda(i32),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Layout {
    Us,
    Abnt2,
}

impl Layout {
    pub fn descricao(&self) -> &'static str {
        match self {
            Layout::Us => "US (americano)",
            Layout::Abnt2 => "ABNT2 (brasileiro)",
        }
    }
}

/// Eventos crus lidos de cada dispositivo por chamada, no maximo (o
/// tamanho da fila: o resto fica para a proxima).
const LOTE: usize = 64;

/// Um dispositivo e o grupo de eventos de ponteiro ainda sem EV_SYN.
struct Fonte {
    /// None = teclado e mouse USB da TV box (crate::usb).
    disp: Option<DispositivoEntrada>,
    /// Houve ABS/REL de posicao desde o ultimo EV_SYN.
    moveu: bool,
    /// Botoes e roda deste grupo, soltos no EV_SYN depois da posicao.
    adiados: Vec<Evento>,
}

pub struct Entrada {
    fontes: Vec<Fonte>,
    teclado: Teclado,
    /// Eventos prontos ainda nao entregues.
    prontos: VecDeque<Evento>,
    largura: u32,
    altura: u32,
    /// Posicao atual do ponteiro (compartilhada por tablet e mouse).
    x: i32,
    y: i32,
}

impl Entrada {
    /// Inicializa todos os virtio-input (DeviceID 18) dos slots do QEMU virt.
    /// Erro se nenhum funcionar.
    pub fn iniciar(largura: u32, altura: u32) -> Result<Entrada, &'static str> {
        let mut fontes = Vec::new();
        let mut ultimo_erro = "nenhum virtio-input (QEMU: -device virtio-keyboard-device -device virtio-tablet-device)";
        for disp in procurar_todos() {
            match DispositivoEntrada::iniciar(disp) {
                Ok(d) => fontes.push(Fonte { disp: Some(d), moveu: false, adiados: Vec::new() }),
                Err(e) => ultimo_erro = e.descricao(),
            }
        }
        #[cfg(feature = "mxq")]
        if crate::usb::tem_entrada() {
            fontes.push(Fonte { disp: None, moveu: false, adiados: Vec::new() });
        }
        if fontes.is_empty() {
            return Err(ultimo_erro);
        }
        Ok(Entrada {
            fontes,
            teclado: Teclado::novo(Layout::Us),
            prontos: VecDeque::new(),
            largura: largura.max(1),
            altura: altura.max(1),
            x: largura as i32 / 2,
            y: altura as i32 / 2,
        })
    }

    /// Proximo evento, se houver. Nao bloqueia; os buffers lidos voltam
    /// para os dispositivos.
    pub fn proximo(&mut self) -> Option<Evento> {
        if self.prontos.is_empty() {
            self.ler_dispositivos();
        }
        self.prontos.pop_front()
    }

    pub fn definir_layout(&mut self, layout: Layout) {
        self.teclado.definir_layout(layout);
    }

    pub fn layout(&self) -> Layout {
        self.teclado.layout()
    }

    /// Nome (ID_NAME) de cada dispositivo.
    pub fn nomes(&self) -> Vec<String> {
        self.fontes.iter().map(|f| f.disp.as_ref().map_or(String::from("teclado/mouse USB"), |d| String::from(d.nome()))).collect()
    }

    /// Uma linha por dispositivo: nome, tipo e slot (painel de drivers).
    pub fn descricoes(&self) -> Vec<String> {
        self.fontes
            .iter()
            .map(|f| match &f.disp {
                Some(d) => {
                    let slot = d.dispositivo().slot_qemu_virt().unwrap_or(0);
                    format!("{} ({}, slot {})", d.nome(), d.tipo().descricao(), slot)
                }
                None => String::from("teclado e mouse USB (HID, protocolo de boot)"),
            })
            .collect()
    }

    /// Posicao atual do ponteiro.
    pub fn posicao(&self) -> (i32, i32) {
        (self.x, self.y)
    }

    /// Modificadores apertados agora: (shift, ctrl, alt).
    pub fn modificadores(&self) -> (bool, bool, bool) {
        (self.teclado.shift(), self.teclado.ctrl(), self.teclado.alt())
    }

    /// Le um lote de cada dispositivo e avisa cada um uma vez so.
    fn ler_dispositivos(&mut self) {
        for i in 0..self.fontes.len() {
            if self.fontes[i].disp.is_none() {
                #[cfg(feature = "mxq")]
                for cru in crate::usb::colher_entrada() {
                    self.traduzir(i, cru);
                }
                continue;
            }
            let mut lidos = 0;
            while lidos < LOTE {
                let Some(cru) = self.fontes[i].disp.as_mut().and_then(|d| d.colher()) else { break };
                self.traduzir(i, cru);
                lidos += 1;
            }
            let Some(disp) = self.fontes[i].disp.as_mut() else { continue };
            if lidos > 0 {
                disp.confirmar_interrupcao();
            }
            disp.avisar();
        }
    }

    /// Evento cru do dispositivo `i` -> eventos prontos.
    fn traduzir(&mut self, i: usize, cru: EventoCru) {
        match cru.tipo {
            EV_KEY if eh_botao(cru.codigo) => self.botao(i, cru.codigo, cru.valor),
            EV_KEY => self.tecla(cru.codigo, cru.valor),
            EV_ABS => self.absoluto(i, cru.codigo, cru.valor as i32),
            EV_REL => self.relativo(i, cru.codigo, cru.valor as i32),
            EV_SYN if cru.codigo == SYN_REPORT => self.sincronizar(i),
            // EV_MSC, EV_LED, SYN_DROPPED...: ignorados
            _ => {}
        }
    }

    /// Tecla: sempre o codigo cru; depois o caractere, se ela tiver um.
    fn tecla(&mut self, codigo: u16, valor: u32) {
        let pressionada = valor != teclado::SOLTOU;
        self.prontos.push_back(Evento::Tecla { codigo, pressionada });
        let prontos = &mut self.prontos;
        self.teclado.processar(codigo, valor, &mut |c| prontos.push_back(Evento::Caractere(c)));
    }

    /// Botao do ponteiro: fica para o EV_SYN. Repeticao (valor 2) ignorada.
    fn botao(&mut self, i: usize, codigo: u16, valor: u32) {
        let pressionado = match valor {
            0 => false,
            1 => true,
            _ => return,
        };
        let evento = match codigo {
            BTN_LEFT => Evento::Botao { botao: Botao::Esquerdo, pressionado },
            BTN_RIGHT => Evento::Botao { botao: Botao::Direito, pressionado },
            BTN_MIDDLE => Evento::Botao { botao: Botao::Meio, pressionado },
            // QEMU antigo: roda como botao (a subida, valor 0, nao conta)
            BTN_GEAR_UP if pressionado => Evento::Roda(1),
            BTN_GEAR_DOWN if pressionado => Evento::Roda(-1),
            _ => return,
        };
        self.fontes[i].adiados.push(evento);
    }

    /// Tablet: posicao absoluta escalada para a tela.
    fn absoluto(&mut self, i: usize, codigo: u16, valor: i32) {
        let fonte = &mut self.fontes[i];
        let Some(disp) = &fonte.disp else { return };
        match codigo {
            ABS_X => self.x = disp.faixa_x.escalar(valor, self.largura),
            ABS_Y => self.y = disp.faixa_y.escalar(valor, self.altura),
            _ => return,
        }
        fonte.moveu = true;
    }

    /// Mouse relativo: anda e para na borda. A roda fica para o EV_SYN.
    fn relativo(&mut self, i: usize, codigo: u16, valor: i32) {
        let fonte = &mut self.fontes[i];
        match codigo {
            REL_X => self.x = self.x.saturating_add(valor).clamp(0, self.largura as i32 - 1),
            REL_Y => self.y = self.y.saturating_add(valor).clamp(0, self.altura as i32 - 1),
            REL_WHEEL if valor != 0 => {
                fonte.adiados.push(Evento::Roda(valor));
                return;
            }
            _ => return,
        }
        fonte.moveu = true;
    }

    /// Fim do grupo: primeiro a posicao nova, depois botoes e roda.
    fn sincronizar(&mut self, i: usize) {
        let fonte = &mut self.fontes[i];
        if fonte.moveu {
            fonte.moveu = false;
            self.prontos.push_back(Evento::Mouse { x: self.x, y: self.y });
        }
        self.prontos.extend(fonte.adiados.drain(..));
    }
}

/// Botoes de mouse/joystick (BTN_MISC..BTN_GEAR_UP); o resto do EV_KEY e tecla.
fn eh_botao(codigo: u16) -> bool {
    (PRIMEIRO_BOTAO..=ULTIMO_BOTAO).contains(&codigo)
}

/// Todos os virtio-input dos slots do QEMU virt, do slot 0 ao 31.
fn procurar_todos() -> Vec<VirtioMmio> {
    (0..SLOTS_QEMU_VIRT)
        .filter_map(|i| VirtioMmio::procurar_em(BASE_QEMU_VIRT + i * PASSO_QEMU_VIRT, PASSO_QEMU_VIRT, 1, TIPO_ENTRADA))
        .collect()
}
