//! Driver do virtio-input (spec virtio 1.2, secao 5.8) sobre virtio-mmio.
//!
//! Um dispositivo por slot: no QEMU o teclado (virtio-keyboard-device) e o
//! tablet (virtio-tablet-device) sao dispositivos separados, os dois com
//! DeviceID 18. Este arquivo cuida de UM dispositivo; quem varre os slots e
//! junta tudo e o mod.rs.
//!
//! Duas filas: 0 eventq (dispositivo -> driver), 1 statusq (driver ->
//! dispositivo, LEDs). So usamos a eventq: a statusq nem e instalada.
//! Cada buffer da eventq recebe um evento de 8 bytes, igual ao do evdev:
//!   {type u16, code u16, value u32} little endian
//! O QEMU so entrega um grupo de eventos (ate o EV_SYN) se houver buffers
//! livres para todos, entao os buffers voltam para a fila logo depois de lidos.
//!
//! Configuracao (offset 0x100 do slot): select u8, subsel u8, size u8,
//! 5 reservados e a uniao de 128 bytes em +8. Escreve-se select/subsel e o
//! dispositivo responde em size e na uniao.
//!
//! Tudo por polling: a interrupcao (INTID 48+slot) nao e ligada no GIC.
//!
//! Memoria de DMA: uma pagina do heap, so devolvida depois do reset:
//!   0x000..0x800  a eventq
//!   0x800..       os buffers de evento, 8 bytes cada

use alloc::alloc::{Layout as LayoutMemoria, alloc_zeroed, dealloc};
use alloc::string::String;
use core::ptr::{read_volatile, write_volatile};

use crate::virtio::virtio_mmio::{Erro, VirtioMmio};
use crate::virtio::virtqueue::{self, Parte, Virtqueue};

/// DeviceID do virtio-input.
pub const TIPO_ENTRADA: u32 = 18;

// Tipos de evento (iguais aos do Linux).
pub const EV_SYN: u16 = 0;
pub const EV_KEY: u16 = 1;
pub const EV_REL: u16 = 2;
pub const EV_ABS: u16 = 3;

// Codigos usados pelo ponteiro.
pub const SYN_REPORT: u16 = 0;
pub const REL_X: u16 = 0;
pub const REL_Y: u16 = 1;
pub const REL_WHEEL: u16 = 8;
pub const ABS_X: u16 = 0;
pub const ABS_Y: u16 = 1;
pub const BTN_LEFT: u16 = 0x110;
pub const BTN_RIGHT: u16 = 0x111;
pub const BTN_MIDDLE: u16 = 0x112;
/// QEMU antigo mandava a roda como botoes "gear"; o novo manda REL_WHEEL na
/// descida e o gear so na subida (valor 0, que e ignorado).
pub const BTN_GEAR_DOWN: u16 = 0x150;
pub const BTN_GEAR_UP: u16 = 0x151;
/// Tipo dado a um evento que veio incompleto (ignorado por todos).
pub const TIPO_INVALIDO: u16 = u16::MAX;
/// Faixa dos botoes (BTN_MISC..=0x15f); o resto do EV_KEY sao teclas
/// (inclusive as de multimidia, de 0x160 para cima).
pub const PRIMEIRO_BOTAO: u16 = 0x100;
pub const ULTIMO_BOTAO: u16 = 0x15f;

// Selecoes do espaco de configuracao.
const CFG_ID_NAME: u8 = 0x01;
const CFG_EV_BITS: u8 = 0x11;
const CFG_ABS_INFO: u8 = 0x12;

// Offsets dentro do espaco de configuracao.
const R_CONFIG: usize = 0x100;
const CFG_SELECT: usize = 0;
const CFG_SUBSEL: usize = 1;
const CFG_SIZE: usize = 2;
const CFG_UNIAO: usize = 8;
/// virtio_input_absinfo: {min, max, fuzz, flat, res}, u32 cada.
const ABS_MIN: usize = CFG_UNIAO;
const ABS_MAX: usize = CFG_UNIAO + 4;

/// Faixa que o QEMU usa no tablet quando o ABS_INFO nao responde.
const ABS_MAX_QEMU: i32 = 0x7fff;

const FILA_EVENTOS: u32 = 0;
/// Tamanho de fila que pedimos (o QEMU usa 64 no virtio-input).
const TAMANHO_FILA: u32 = 64;
const REGIAO_FILA: usize = 0x800;
const _: () = assert!(virtqueue::memoria_necessaria(TAMANHO_FILA as u16) <= REGIAO_FILA);
const BUFFERS: usize = REGIAO_FILA;
const TAMANHO_EVENTO: usize = 8;
const TAMANHO_DMA: usize = 4096;
const _: () = assert!(BUFFERS + TAMANHO_FILA as usize * TAMANHO_EVENTO <= TAMANHO_DMA);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ErroEntrada {
    /// Erro do transporte ou das filas.
    Virtio(Erro),
    /// O heap nao deu a pagina de DMA.
    SemMemoria,
    /// Faltou descritor livre ao entregar os buffers (nao deveria acontecer).
    FilaCheia,
}

impl ErroEntrada {
    pub fn descricao(&self) -> &'static str {
        match self {
            ErroEntrada::Virtio(e) => e.descricao(),
            ErroEntrada::SemMemoria => "sem memória para a fila de eventos",
            ErroEntrada::FilaCheia => "fila de eventos sem descritor livre",
        }
    }
}

impl From<Erro> for ErroEntrada {
    fn from(e: Erro) -> ErroEntrada {
        ErroEntrada::Virtio(e)
    }
}

/// Que tipo de aparelho e, deduzido dos EV_BITS.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tipo {
    Teclado,
    /// Ponteiro absoluto (virtio-tablet).
    Tablet,
    /// Ponteiro relativo (virtio-mouse).
    Mouse,
}

impl Tipo {
    pub fn descricao(&self) -> &'static str {
        match self {
            Tipo::Teclado => "teclado",
            Tipo::Tablet => "tablet",
            Tipo::Mouse => "mouse",
        }
    }
}

/// Faixa de um eixo absoluto (ABS_INFO).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Faixa {
    pub min: i32,
    pub max: i32,
}

impl Faixa {
    /// Converte `valor` para 0..=tamanho-1 (pixels da tela).
    pub fn escalar(&self, valor: i32, tamanho: u32) -> i32 {
        let ultimo = tamanho.max(1) as i64 - 1;
        let largura = (self.max as i64 - self.min as i64).max(1);
        let v = (valor as i64).clamp(self.min as i64, self.max as i64) - self.min as i64;
        (v * ultimo / largura) as i32
    }
}

/// Um evento cru do dispositivo.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EventoCru {
    pub tipo: u16,
    pub codigo: u16,
    pub valor: u32,
}

fn ler8(endereco: usize) -> u8 {
    unsafe { read_volatile(endereco as *const u8) }
}

fn escrever8(endereco: usize, valor: u8) {
    unsafe { write_volatile(endereco as *mut u8, valor) }
}

fn ler16(endereco: usize) -> u16 {
    unsafe { read_volatile(endereco as *const u16) }
}

fn ler32(endereco: usize) -> u32 {
    unsafe { read_volatile(endereco as *const u32) }
}

/// Maior potencia de 2 <= min(maximo, TAMANHO_FILA).
fn tamanho_de_fila(maximo: u32) -> Result<u16, ErroEntrada> {
    let t = maximo.min(TAMANHO_FILA);
    if t == 0 {
        return Err(ErroEntrada::Virtio(Erro::FilaInexistente(FILA_EVENTOS)));
    }
    Ok((1u32 << (31 - t.leading_zeros())) as u16)
}

/// Espaco de configuracao especifico do virtio-input.
struct Config {
    base: usize,
}

impl Config {
    /// Escolhe o que ler e devolve quantos bytes validos a uniao tem.
    fn selecionar(&self, select: u8, subsel: u8) -> usize {
        escrever8(self.base + CFG_SELECT, select);
        escrever8(self.base + CFG_SUBSEL, subsel);
        (ler8(self.base + CFG_SIZE) as usize).min(128)
    }

    fn byte(&self, i: usize) -> u8 {
        ler8(self.base + CFG_UNIAO + i)
    }

    /// Nome do dispositivo (ID_NAME), so ASCII imprimivel.
    fn nome(&self) -> String {
        let tamanho = self.selecionar(CFG_ID_NAME, 0);
        let nome: String = (0..tamanho)
            .map(|i| self.byte(i))
            .take_while(|&b| b != 0)
            .map(|b| if (32..=126).contains(&b) { b as char } else { '?' })
            .collect();
        if nome.is_empty() { String::from("virtio-input") } else { nome }
    }

    /// O dispositivo gera o codigo `codigo` do tipo de evento `tipo`?
    fn tem_bit(&self, tipo: u16, codigo: u16) -> bool {
        let tamanho = self.selecionar(CFG_EV_BITS, tipo as u8);
        let byte = codigo as usize / 8;
        byte < tamanho && self.byte(byte) & (1 << (codigo % 8)) != 0
    }

    fn tipo(&self) -> Tipo {
        if self.tem_bit(EV_ABS, ABS_X) {
            Tipo::Tablet
        } else if self.tem_bit(EV_REL, REL_X) {
            Tipo::Mouse
        } else {
            Tipo::Teclado
        }
    }

    /// Faixa do eixo `eixo`; sem resposta, a faixa padrao do QEMU.
    fn faixa(&self, disp: &VirtioMmio, eixo: u16) -> Faixa {
        if self.selecionar(CFG_ABS_INFO, eixo as u8) < 8 {
            return Faixa { min: 0, max: ABS_MAX_QEMU };
        }
        let min = disp.ler_config_u32(ABS_MIN) as i32;
        let max = disp.ler_config_u32(ABS_MAX) as i32;
        if max > min { Faixa { min, max } } else { Faixa { min: 0, max: ABS_MAX_QEMU } }
    }
}

/// Um virtio-input inicializado, com a eventq cheia de buffers.
pub struct DispositivoEntrada {
    disp: VirtioMmio,
    fila: Virtqueue,
    memoria: usize,
    /// Buffer que esta em cada descritor cabeca.
    buffer_da_cabeca: [usize; TAMANHO_FILA as usize],
    /// Devolvemos buffers e ainda nao avisamos o dispositivo.
    precisa_aviso: bool,
    nome: String,
    tipo: Tipo,
    pub faixa_x: Faixa,
    pub faixa_y: Faixa,
}

impl DispositivoEntrada {
    /// Inicializa o dispositivo do slot: le a configuracao, monta a eventq,
    /// DRIVER_OK e entrega todos os buffers. Em erro o dispositivo fica
    /// resetado e a memoria volta para o heap.
    pub fn iniciar(disp: VirtioMmio) -> Result<DispositivoEntrada, ErroEntrada> {
        let layout = LayoutMemoria::from_size_align(TAMANHO_DMA, 4096).map_err(|_| ErroEntrada::SemMemoria)?;
        let ponteiro = unsafe { alloc_zeroed(layout) };
        if ponteiro.is_null() {
            return Err(ErroEntrada::SemMemoria);
        }
        let base = disp.base();
        match unsafe { Self::montar(disp, ponteiro as usize) } {
            Ok(d) => Ok(d),
            Err(e) => {
                // reset antes de devolver a memoria: nada de DMA nela depois
                let disp = unsafe { VirtioMmio::em(base) };
                disp.falhar();
                disp.resetar();
                unsafe { dealloc(ponteiro, layout) };
                Err(e)
            }
        }
    }

    /// # Seguranca
    /// `memoria` e uma pagina de RAM exclusiva deste dispositivo.
    unsafe fn montar(disp: VirtioMmio, memoria: usize) -> Result<DispositivoEntrada, ErroEntrada> {
        disp.iniciar(0)?;
        let config = Config { base: disp.base() + R_CONFIG };
        let nome = config.nome();
        let tipo = config.tipo();
        let faixa_x = config.faixa(&disp, ABS_X);
        let faixa_y = config.faixa(&disp, ABS_Y);
        let tamanho = tamanho_de_fila(disp.tamanho_max_fila(FILA_EVENTOS))?;
        let fila = unsafe { Virtqueue::nova(FILA_EVENTOS, tamanho, memoria)? };
        fila.instalar(&disp)?;
        disp.ativar();
        let mut d = DispositivoEntrada {
            disp,
            fila,
            memoria,
            buffer_da_cabeca: [0; TAMANHO_FILA as usize],
            precisa_aviso: false,
            nome,
            tipo,
            faixa_x,
            faixa_y,
        };
        d.entregar_buffers(tamanho as usize);
        Ok(d)
    }

    /// Poe os `n` buffers de evento na eventq e avisa. Nao falha: a fila
    /// tem exatamente `n` descritores livres.
    fn entregar_buffers(&mut self, n: usize) {
        for i in 0..n {
            let _ = self.devolver(self.memoria + BUFFERS + i * TAMANHO_EVENTO);
        }
        self.avisar();
    }

    /// Um buffer de evento volta para a fila (o aviso fica para depois).
    fn devolver(&mut self, endereco: usize) -> Result<(), ErroEntrada> {
        let parte = Parte { endereco, tamanho: TAMANHO_EVENTO as u32, dispositivo_escreve: true };
        virtqueue::para_dispositivo(endereco, TAMANHO_EVENTO);
        let cabeca = self.fila.adicionar(&[parte]).ok_or(ErroEntrada::FilaCheia)?;
        self.buffer_da_cabeca[cabeca as usize] = endereco;
        self.precisa_aviso = true;
        Ok(())
    }

    /// Avisa o dispositivo se algum buffer voltou para a fila. Um aviso so
    /// por lote: cada escrita no registrador custa uma saida da VM.
    pub fn avisar(&mut self) {
        if self.precisa_aviso {
            self.precisa_aviso = false;
            self.disp.notificar(FILA_EVENTOS);
        }
    }

    /// Proximo evento cru, se houver (nao bloqueia). O buffer volta na hora
    /// para a fila; chame `avisar` depois do lote.
    pub fn colher(&mut self) -> Option<EventoCru> {
        let usado = self.fila.colher()?;
        let endereco = self.buffer_da_cabeca[usado.cabeca as usize];
        virtqueue::do_dispositivo(endereco, TAMANHO_EVENTO);
        let mut evento = EventoCru { tipo: ler16(endereco), codigo: ler16(endereco + 2), valor: ler32(endereco + 4) };
        if usado.tamanho < TAMANHO_EVENTO as u32 {
            // buffer curto: evento invalido, vira um tipo que ninguem trata
            evento.tipo = TIPO_INVALIDO;
        }
        // o descritor deste buffer acabou de ficar livre: nao falha
        let _ = self.devolver(endereco);
        Some(evento)
    }

    /// Limpa a interrupcao pendente (o sinal e de nivel; sem isso ele fica
    /// preso se alguem ligar o INTID no GIC).
    pub fn confirmar_interrupcao(&self) {
        self.disp.confirmar_interrupcao();
    }

    pub fn nome(&self) -> &str {
        &self.nome
    }

    pub fn tipo(&self) -> Tipo {
        self.tipo
    }

    pub fn dispositivo(&self) -> &VirtioMmio {
        &self.disp
    }
}

impl Drop for DispositivoEntrada {
    /// Reset primeiro, para o dispositivo nunca escrever na pagina devolvida.
    fn drop(&mut self) {
        self.disp.resetar();
        if let Ok(layout) = LayoutMemoria::from_size_align(TAMANHO_DMA, 4096) {
            unsafe { dealloc(self.memoria as *mut u8, layout) };
        }
    }
}
