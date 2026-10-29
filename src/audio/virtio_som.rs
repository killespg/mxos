//! Driver do virtio-sound (spec virtio 1.2, secao 5.14) sobre virtio-mmio.
//!
//! Quatro filas: 0 controle, 1 eventos, 2 tx (tocar), 3 rx (gravar).
//! Toda mensagem de controle e uma cadeia de 2 descritores: pedido (o
//! dispositivo le) e resposta (o dispositivo escreve), e a resposta comeca
//! com um codigo u32 (0x8000 = OK).
//!
//! Para tocar: PCM_INFO, SET_PARAMS, PREPARE, encher a tx (pre-buffer),
//! START, ir devolvendo periodos conforme o dispositivo consome, STOP e
//! RELEASE. Use essa sequencia uma vez so por stream: no QEMU depois do
//! 11.1 uma transicao invalida (ex.: STOP sem START) poe o dispositivo em
//! NEEDS_RESET, e um novo PREPARE reabre (e trunca) o arquivo do -audiodev wav.
//!
//! Cada periodo vai na tx como cadeia de 3 descritores:
//!   {stream_id u32} (le) -> PCM intercalado S16 LE (le) -> {status u32, latencia u32} (escreve)
//!
//! Tudo por polling: nao precisa da interrupcao (INTID 48+slot no QEMU virt).
//! Se ela for ligada no GIC, chame `confirmar_interrupcao` no tratador.
//!
//! Memoria: o chamador entrega uma regiao de RAM (alinhada em 16, de
//! preferencia em 4096) com pelo menos `memoria_necessaria` bytes:
//!   0x0000..0x2000  as 4 filas (0x800 cada)
//!   0x2000..0x3000  pedido, resposta e buffers de evento
//!   0x3000..        os "slots" de tx, um periodo cada

use core::hint::spin_loop;
use core::ptr::{read_volatile, write_volatile};

use super::gerador::Fonte;
use crate::virtio::virtio_mmio::{Erro, VirtioMmio};
use crate::virtio::virtqueue::{self, Parte, Virtqueue};

// Filas
const FILA_CONTROLE: u32 = 0;
const FILA_EVENTO: u32 = 1;
const FILA_TX: u32 = 2;
const FILA_RX: u32 = 3;
const FILAS: u32 = 4;

// Codigos dos pedidos de controle
const PCM_INFO: u32 = 0x0100;
const PCM_SET_PARAMS: u32 = 0x0101;
const PCM_PREPARE: u32 = 0x0102;
const PCM_RELEASE: u32 = 0x0103;
const PCM_START: u32 = 0x0104;
const PCM_STOP: u32 = 0x0105;

// Codigos de resposta
pub const S_OK: u32 = 0x8000;
pub const S_BAD_MSG: u32 = 0x8001;
pub const S_NOT_SUPP: u32 = 0x8002;
pub const S_IO_ERR: u32 = 0x8003;

// Eventos
pub const EVT_PCM_PERIODO: u32 = 0x1100;
pub const EVT_PCM_XRUN: u32 = 0x1101;

/// Formato S16 (bit 5 do bitmap de formatos).
const FORMATO_S16: u8 = 5;
/// Tamanho da struct virtio_snd_pcm_info.
const TAMANHO_INFO: u32 = 32;
/// Direcao do stream: 0 = saida (tocar), 1 = entrada (gravar).
pub const DIRECAO_SAIDA: u8 = 0;

/// Taxa de amostragem de cada bit do bitmap de taxas (spec 5.14.6.6.1).
pub const TAXAS_HZ: [u32; 14] = [
    5512, 8000, 11025, 16000, 22050, 32000, 44100, 48000, 64000, 88200, 96000, 176400, 192000, 384000,
];

/// Tamanho de fila que pedimos (o QEMU aceita no maximo 64).
const TAMANHO_FILA: u32 = 64;
const REGIAO_FILA: usize = 0x800;
const _: () = assert!(virtqueue::memoria_necessaria(TAMANHO_FILA as u16) <= REGIAO_FILA);

// Pagina de controle
const AREA_CONTROLE: usize = 0x2000;
const PEDIDO: usize = 0x000;
const RESPOSTA: usize = 0x100;
const TAMANHO_RESPOSTA: u32 = 0x100;
const EVENTOS: usize = 0x400;
const QUANTOS_EVENTOS: usize = 4;
const TAMANHO_EVENTO: u32 = 8;

/// Inicio dos slots de tx; tudo antes disso e fixo.
pub const MEMORIA_FIXA: usize = 0x3000;
/// Cada slot: {stream_id} em +0, {status, latencia} em +16, PCM em +32.
const SLOT_STATUS: usize = 16;
const SLOT_PCM: usize = 32;
/// Slots de tx no maximo (cada um usa 3 descritores).
pub const MAX_SLOTS: usize = 16;
/// Canais no maximo por quadro.
pub const MAX_CANAIS: usize = 8;

/// Quantas voltas esperar a resposta de um pedido de controle.
const LIMITE_ESPERA: u32 = 50_000_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ErroSom {
    /// Erro do transporte ou das filas.
    Virtio(Erro),
    /// Regiao de memoria pequena demais ou desalinhada.
    SemMemoria,
    /// O dispositivo nao respondeu a tempo.
    SemResposta,
    /// O dispositivo respondeu com um codigo diferente de S_OK.
    Recusado(u32),
    /// O dispositivo entrou em NEEDS_RESET.
    PrecisaReset,
    /// Nao ha stream de saida.
    SemStreamDeSaida,
    /// Nem S16 nem uma taxa conhecida, ou canais demais.
    FormatoNaoSuportado,
    /// Faltou descritor livre (nao deveria acontecer).
    FilaCheia,
    /// Tocar antes de `configurar`.
    NaoConfigurado,
}

impl ErroSom {
    pub fn descricao(&self) -> &'static str {
        match self {
            ErroSom::Virtio(e) => e.descricao(),
            ErroSom::SemMemoria => "memória de DMA pequena ou desalinhada",
            ErroSom::SemResposta => "o dispositivo não respondeu",
            ErroSom::Recusado(_) => "o dispositivo recusou o pedido",
            ErroSom::PrecisaReset => "o dispositivo pediu reset (NEEDS_RESET)",
            ErroSom::SemStreamDeSaida => "nenhum stream de saída",
            ErroSom::FormatoNaoSuportado => "formato não suportado (precisa S16)",
            ErroSom::FilaCheia => "fila sem descritor livre",
            ErroSom::NaoConfigurado => "stream não configurado",
        }
    }
}

impl From<Erro> for ErroSom {
    fn from(e: Erro) -> ErroSom {
        ErroSom::Virtio(e)
    }
}

/// Resposta do PCM_INFO para um stream.
#[derive(Clone, Copy)]
pub struct InfoPcm {
    pub features: u32,
    /// Bit n ligado = formato n suportado (S16 = bit 5).
    pub formatos: u64,
    /// Bit n ligado = TAXAS_HZ[n] suportada.
    pub taxas: u64,
    pub direcao: u8,
    pub canais_min: u8,
    pub canais_max: u8,
}

impl InfoPcm {
    pub fn suporta_s16(&self) -> bool {
        self.formatos & (1 << FORMATO_S16) != 0
    }

    pub fn suporta_taxa(&self, taxa_hz: u32) -> bool {
        codigo_taxa(taxa_hz).is_some_and(|c| self.taxas & (1 << c) != 0)
    }
}

/// O que o driver vai pedir no SET_PARAMS (sempre S16).
#[derive(Clone, Copy)]
pub struct Parametros {
    pub taxa_hz: u32,
    pub canais: u8,
    /// Quadros por periodo (um periodo = um buffer na tx).
    pub quadros_periodo: u32,
    /// Periodos no buffer do dispositivo (= slots de tx, ate MAX_SLOTS).
    pub periodos: u32,
}

#[derive(Clone, Copy, Default)]
pub struct Estatisticas {
    /// Periodos e quadros entregues ao dispositivo.
    pub periodos: u32,
    pub quadros: u64,
    /// Periodos devolvidos com status diferente de S_OK.
    pub erros_tx: u32,
    pub ultimo_status: u32,
    /// Bytes ainda na fila do dispositivo, segundo a ultima devolucao.
    pub latencia_bytes: u32,
    pub eventos: u32,
    pub xruns: u32,
}

/// Bit do bitmap de taxas para `taxa_hz`.
pub fn codigo_taxa(taxa_hz: u32) -> Option<u8> {
    TAXAS_HZ.iter().position(|&t| t == taxa_hz).map(|i| i as u8)
}

/// Escolhe S16, a primeira taxa de `preferidas` que o stream aceita (senao a
/// maior que ele aceita), estereo se der, e periodos de `periodo_ms`.
pub fn escolher_parametros(info: &InfoPcm, preferidas: &[u32], periodo_ms: u32, periodos: u32) -> Result<Parametros, ErroSom> {
    if !info.suporta_s16() {
        return Err(ErroSom::FormatoNaoSuportado);
    }
    let taxa_hz = preferidas
        .iter()
        .copied()
        .find(|&t| info.suporta_taxa(t))
        .or_else(|| TAXAS_HZ.iter().rev().copied().find(|&t| info.suporta_taxa(t)))
        .ok_or(ErroSom::FormatoNaoSuportado)?;
    let minimo = info.canais_min.max(1);
    let canais = 2u8.clamp(minimo, info.canais_max.max(minimo));
    if canais as usize > MAX_CANAIS {
        return Err(ErroSom::FormatoNaoSuportado);
    }
    let quadros_periodo = ((taxa_hz as u64 * periodo_ms as u64) / 1000).max(1) as u32;
    Ok(Parametros { taxa_hz, canais, quadros_periodo, periodos })
}

/// Bytes que um slot de tx ocupa (alinhado em 64).
const fn tamanho_slot(bytes_periodo: usize) -> usize {
    (SLOT_PCM + bytes_periodo + 63) & !63
}

/// Memoria total para `slots` periodos de `bytes_periodo` bytes.
pub const fn memoria_necessaria(bytes_periodo: usize, slots: usize) -> usize {
    MEMORIA_FIXA + slots * tamanho_slot(bytes_periodo)
}

fn ler32(endereco: usize) -> u32 {
    unsafe { read_volatile(endereco as *const u32) }
}

fn escrever32(endereco: usize, valor: u32) {
    unsafe { write_volatile(endereco as *mut u32, valor) }
}

fn escrever16(endereco: usize, valor: u16) {
    unsafe { write_volatile(endereco as *mut u16, valor) }
}

/// Maior potencia de 2 <= min(maximo, TAMANHO_FILA).
fn tamanho_de_fila(maximo: u32) -> Result<u16, ErroSom> {
    let t = maximo.min(TAMANHO_FILA);
    if t == 0 {
        return Err(ErroSom::Virtio(Erro::FilaInexistente(0)));
    }
    Ok((1u32 << (31 - t.leading_zeros())) as u16)
}

pub struct VirtioSom {
    disp: VirtioMmio,
    controle: Virtqueue,
    evento: Virtqueue,
    tx: Virtqueue,
    _rx: Virtqueue,
    memoria: usize,
    tamanho_memoria: usize,
    features: u64,
    jacks: u32,
    streams: u32,
    chmaps: u32,
    cabecas_evento: [u16; QUANTOS_EVENTOS],
    // estado da reproducao (depois de `configurar`)
    configurado: bool,
    stream: u32,
    canais: u8,
    quadros_periodo: u32,
    slots: usize,
    /// Descritor cabeca de cada slot que esta com o dispositivo.
    cabecas: [Option<u16>; MAX_SLOTS],
    fonte_acabou: bool,
    estatisticas: Estatisticas,
}

impl VirtioSom {
    /// Inicializa o dispositivo: features, as 4 filas, DRIVER_OK, leitura
    /// da configuracao e buffers de evento.
    ///
    /// # Seguranca
    /// `memoria..memoria+tamanho` tem de ser RAM exclusiva deste driver
    /// enquanto ele viver (o dispositivo faz DMA nela).
    pub unsafe fn iniciar(disp: VirtioMmio, memoria: usize, tamanho: usize) -> Result<VirtioSom, ErroSom> {
        if memoria & 0xF != 0 || tamanho < MEMORIA_FIXA {
            return Err(ErroSom::SemMemoria);
        }
        let features = disp.iniciar(0)?;
        let filas = match unsafe { Self::montar_filas(&disp, memoria) } {
            Ok(f) => f,
            Err(e) => {
                disp.falhar();
                return Err(e);
            }
        };
        disp.ativar();
        let [controle, evento, tx, rx] = filas;
        let mut som = VirtioSom {
            jacks: disp.ler_config_u32(0),
            streams: disp.ler_config_u32(4),
            chmaps: disp.ler_config_u32(8),
            disp,
            controle,
            evento,
            tx,
            _rx: rx,
            memoria,
            tamanho_memoria: tamanho,
            features,
            cabecas_evento: [0; QUANTOS_EVENTOS],
            configurado: false,
            stream: 0,
            canais: 0,
            quadros_periodo: 0,
            slots: 0,
            cabecas: [None; MAX_SLOTS],
            fonte_acabou: false,
            estatisticas: Estatisticas::default(),
        };
        som.postar_eventos()?;
        Ok(som)
    }

    /// Cria e instala as filas 0..4, cada uma em REGIAO_FILA bytes.
    unsafe fn montar_filas(disp: &VirtioMmio, memoria: usize) -> Result<[Virtqueue; 4], ErroSom> {
        let montar = |q: u32| -> Result<Virtqueue, ErroSom> {
            let tamanho = tamanho_de_fila(disp.tamanho_max_fila(q))?;
            let fila = unsafe { Virtqueue::nova(q, tamanho, memoria + q as usize * REGIAO_FILA)? };
            fila.instalar(disp)?;
            Ok(fila)
        };
        const _: () = assert!(FILAS == 4);
        Ok([montar(FILA_CONTROLE)?, montar(FILA_EVENTO)?, montar(FILA_TX)?, montar(FILA_RX)?])
    }

    pub fn dispositivo(&self) -> &VirtioMmio {
        &self.disp
    }

    pub fn features(&self) -> u64 {
        self.features
    }

    pub fn jacks(&self) -> u32 {
        self.jacks
    }

    pub fn streams(&self) -> u32 {
        self.streams
    }

    pub fn chmaps(&self) -> u32 {
        self.chmaps
    }

    pub fn estatisticas(&self) -> Estatisticas {
        self.estatisticas
    }

    fn controle(&self, offset: usize) -> usize {
        self.memoria + AREA_CONTROLE + offset
    }

    // ---------------- eventos ----------------

    /// Coloca os buffers de 8 bytes da fila de eventos (a spec pede).
    fn postar_eventos(&mut self) -> Result<(), ErroSom> {
        for i in 0..QUANTOS_EVENTOS {
            self.postar_evento(i)?;
        }
        self.disp.notificar(FILA_EVENTO);
        Ok(())
    }

    fn postar_evento(&mut self, i: usize) -> Result<(), ErroSom> {
        let endereco = self.controle(EVENTOS + 16 * i);
        let parte = Parte { endereco, tamanho: TAMANHO_EVENTO, dispositivo_escreve: true };
        self.cabecas_evento[i] = self.evento.adicionar(&[parte]).ok_or(ErroSom::FilaCheia)?;
        Ok(())
    }

    /// Le os eventos que chegaram (conta xruns) e devolve os buffers.
    fn recolher_eventos(&mut self) -> Result<(), ErroSom> {
        let mut algum = false;
        while let Some(usado) = self.evento.colher() {
            let Some(i) = self.cabecas_evento.iter().position(|&c| c == usado.cabeca) else { continue };
            virtqueue::do_dispositivo(self.controle(EVENTOS + 16 * i), 8);
            self.estatisticas.eventos += 1;
            if ler32(self.controle(EVENTOS + 16 * i)) == EVT_PCM_XRUN {
                self.estatisticas.xruns += 1;
            }
            self.postar_evento(i)?;
            algum = true;
        }
        if algum {
            self.disp.notificar(FILA_EVENTO);
        }
        Ok(())
    }

    // ---------------- controle ----------------

    /// Manda o pedido ja escrito em PEDIDO e espera a resposta em RESPOSTA.
    /// Devolve quantos bytes o dispositivo escreveu.
    fn pedir(&mut self, tamanho_pedido: u32, tamanho_resposta: u32) -> Result<u32, ErroSom> {
        let (pedido, resposta) = (self.controle(PEDIDO), self.controle(RESPOSTA));
        escrever32(resposta, 0);
        virtqueue::para_dispositivo(pedido, RESPOSTA + TAMANHO_RESPOSTA as usize);
        let partes = [
            Parte { endereco: pedido, tamanho: tamanho_pedido, dispositivo_escreve: false },
            Parte { endereco: resposta, tamanho: tamanho_resposta, dispositivo_escreve: true },
        ];
        let cabeca = self.controle.adicionar(&partes).ok_or(ErroSom::FilaCheia)?;
        self.disp.notificar(FILA_CONTROLE);
        let escritos = self.esperar_controle(cabeca)?;
        virtqueue::do_dispositivo(resposta, tamanho_resposta as usize);
        let codigo = ler32(resposta);
        if codigo != S_OK {
            return Err(ErroSom::Recusado(codigo));
        }
        Ok(escritos)
    }

    /// Polling do used ring da fila de controle ate voltar a cadeia `cabeca`.
    fn esperar_controle(&mut self, cabeca: u16) -> Result<u32, ErroSom> {
        for _ in 0..LIMITE_ESPERA {
            if let Some(usado) = self.controle.colher() {
                if usado.cabeca == cabeca {
                    return Ok(usado.tamanho);
                }
            }
            if self.disp.precisa_reset() {
                return Err(ErroSom::PrecisaReset);
            }
            spin_loop();
        }
        Err(ErroSom::SemResposta)
    }

    /// Pedido que so leva {codigo, stream_id} (PREPARE, START, STOP, RELEASE).
    fn pedido_simples(&mut self, codigo: u32, stream: u32) -> Result<(), ErroSom> {
        escrever32(self.controle(PEDIDO), codigo);
        escrever32(self.controle(PEDIDO + 4), stream);
        self.pedir(8, 4).map(|_| ())
    }

    /// PCM_INFO de um stream.
    pub fn info_pcm(&mut self, stream: u32) -> Result<InfoPcm, ErroSom> {
        let p = self.controle(PEDIDO);
        escrever32(p, PCM_INFO);
        escrever32(p + 4, stream); // start_id
        escrever32(p + 8, 1); // count
        escrever32(p + 12, TAMANHO_INFO); // size
        let escritos = self.pedir(16, 4 + TAMANHO_INFO)?;
        if escritos < 4 + TAMANHO_INFO {
            return Err(ErroSom::Recusado(S_BAD_MSG));
        }
        Ok(self.ler_info(self.controle(RESPOSTA + 4)))
    }

    /// Decodifica virtio_snd_pcm_info (so leituras de 32 bits alinhadas).
    fn ler_info(&self, i: usize) -> InfoPcm {
        let byte = |offset: usize, n: u32| (ler32(i + offset) >> (8 * n)) as u8;
        InfoPcm {
            features: ler32(i + 4),
            formatos: ler32(i + 8) as u64 | (ler32(i + 12) as u64) << 32,
            taxas: ler32(i + 16) as u64 | (ler32(i + 20) as u64) << 32,
            direcao: byte(24, 0),
            canais_min: byte(24, 1),
            canais_max: byte(24, 2),
        }
    }

    /// Primeiro stream de saida e o PCM_INFO dele.
    pub fn primeiro_stream_de_saida(&mut self) -> Result<(u32, InfoPcm), ErroSom> {
        for stream in 0..self.streams {
            let info = self.info_pcm(stream)?;
            if info.direcao == DIRECAO_SAIDA {
                return Ok((stream, info));
            }
        }
        Err(ErroSom::SemStreamDeSaida)
    }

    /// Quantos slots cabem na memoria, na fila tx e no pedido.
    fn slots_possiveis(&self, bytes_periodo: usize, pedidos: u32) -> usize {
        let por_memoria = (self.tamanho_memoria - MEMORIA_FIXA) / tamanho_slot(bytes_periodo);
        let por_fila = self.tx.tamanho() as usize / 3;
        (pedidos as usize).min(MAX_SLOTS).min(por_memoria).min(por_fila)
    }

    /// SET_PARAMS e PREPARE. Chame uma vez so por stream.
    pub fn configurar(&mut self, stream: u32, p: &Parametros) -> Result<(), ErroSom> {
        let codigo = codigo_taxa(p.taxa_hz).ok_or(ErroSom::FormatoNaoSuportado)?;
        if p.canais == 0 || p.canais as usize > MAX_CANAIS || p.quadros_periodo == 0 {
            return Err(ErroSom::FormatoNaoSuportado);
        }
        let bytes_periodo = p.quadros_periodo as usize * p.canais as usize * 2;
        let slots = self.slots_possiveis(bytes_periodo, p.periodos);
        if slots == 0 {
            return Err(ErroSom::SemMemoria);
        }
        let r = self.controle(PEDIDO);
        escrever32(r, PCM_SET_PARAMS);
        escrever32(r + 4, stream);
        escrever32(r + 8, (slots * bytes_periodo) as u32); // buffer_bytes
        escrever32(r + 12, bytes_periodo as u32); // period_bytes
        escrever32(r + 16, 0); // features
        // channels u8, format u8, rate u8, padding u8
        escrever32(r + 20, p.canais as u32 | (FORMATO_S16 as u32) << 8 | (codigo as u32) << 16);
        self.pedir(24, 4)?;
        self.pedido_simples(PCM_PREPARE, stream)?;
        self.stream = stream;
        self.canais = p.canais;
        self.quadros_periodo = p.quadros_periodo;
        self.slots = slots;
        self.cabecas = [None; MAX_SLOTS];
        self.fonte_acabou = false;
        self.estatisticas = Estatisticas::default();
        self.configurado = true;
        Ok(())
    }

    /// Slots de tx que `configurar` conseguiu reservar.
    pub fn slots(&self) -> usize {
        self.slots
    }

    // ---------------- tx ----------------

    fn endereco_slot(&self, slot: usize) -> usize {
        let bytes_periodo = self.quadros_periodo as usize * self.canais as usize * 2;
        self.memoria + MEMORIA_FIXA + slot * tamanho_slot(bytes_periodo)
    }

    /// Enche o slot com ate um periodo da fonte. Devolve quantos quadros.
    fn encher_slot(&mut self, slot: usize, fonte: &mut impl Fonte) -> u32 {
        let base = self.endereco_slot(slot);
        escrever32(base, self.stream); // virtio_snd_pcm_xfer
        escrever32(base + SLOT_STATUS, 0); // status, o dispositivo sobrescreve
        let canais = self.canais as usize;
        let mut quadro = [0i16; MAX_CANAIS];
        let mut n = 0;
        while n < self.quadros_periodo {
            if !fonte.proximo_quadro(&mut quadro[..canais]) {
                self.fonte_acabou = true;
                break;
            }
            let destino = base + SLOT_PCM + n as usize * canais * 2;
            for (c, &amostra) in quadro[..canais].iter().enumerate() {
                escrever16(destino + 2 * c, amostra as u16);
            }
            n += 1;
        }
        n
    }

    /// Publica o slot na tx: {stream_id} -> PCM -> {status}.
    fn enviar_slot(&mut self, slot: usize, quadros: u32) -> Result<(), ErroSom> {
        let base = self.endereco_slot(slot);
        let bytes = quadros * self.canais as u32 * 2;
        virtqueue::para_dispositivo(base, SLOT_PCM + bytes as usize);
        let partes = [
            Parte { endereco: base, tamanho: 4, dispositivo_escreve: false },
            Parte { endereco: base + SLOT_PCM, tamanho: bytes, dispositivo_escreve: false },
            Parte { endereco: base + SLOT_STATUS, tamanho: 8, dispositivo_escreve: true },
        ];
        self.cabecas[slot] = Some(self.tx.adicionar(&partes).ok_or(ErroSom::FilaCheia)?);
        self.estatisticas.periodos += 1;
        self.estatisticas.quadros += quadros as u64;
        Ok(())
    }

    /// Enche e publica todos os slots livres. Devolve se publicou algum.
    fn reabastecer(&mut self, fonte: &mut impl Fonte) -> Result<bool, ErroSom> {
        let mut publicou = false;
        for slot in 0..self.slots {
            if self.fonte_acabou {
                break;
            }
            if self.cabecas[slot].is_some() {
                continue;
            }
            let quadros = self.encher_slot(slot, fonte);
            if quadros > 0 {
                self.enviar_slot(slot, quadros)?;
                publicou = true;
            }
        }
        Ok(publicou)
    }

    /// Pega os periodos que o dispositivo ja consumiu e libera os slots.
    fn recolher_tx(&mut self) {
        while let Some(usado) = self.tx.colher() {
            let Some(slot) = self.cabecas.iter().position(|&c| c == Some(usado.cabeca)) else { continue };
            let status = self.endereco_slot(slot) + SLOT_STATUS;
            virtqueue::do_dispositivo(status, 8);
            let codigo = ler32(status);
            self.estatisticas.ultimo_status = codigo;
            self.estatisticas.latencia_bytes = ler32(status + 4);
            if codigo != S_OK {
                self.estatisticas.erros_tx += 1;
            }
            self.cabecas[slot] = None;
        }
    }

    /// Pre-buffer (todos os slots, ainda no estado PREPARE) e START.
    pub fn comecar(&mut self, fonte: &mut impl Fonte) -> Result<(), ErroSom> {
        if !self.configurado {
            return Err(ErroSom::NaoConfigurado);
        }
        if self.reabastecer(fonte)? {
            self.disp.notificar(FILA_TX);
        }
        self.pedido_simples(PCM_START, self.stream)
    }

    /// Um passo da reproducao, sem bloquear: recolhe o que voltou e reenche.
    /// Devolve true quando a fonte acabou e o dispositivo devolveu tudo.
    pub fn bombear(&mut self, fonte: &mut impl Fonte) -> Result<bool, ErroSom> {
        if !self.configurado {
            return Err(ErroSom::NaoConfigurado);
        }
        self.recolher_tx();
        self.recolher_eventos()?;
        if self.reabastecer(fonte)? {
            self.disp.notificar(FILA_TX);
        }
        if self.disp.precisa_reset() {
            return Err(ErroSom::PrecisaReset);
        }
        Ok(self.fonte_acabou && self.cabecas.iter().all(|c| c.is_none()))
    }

    /// Toca a fonte inteira: comecar e bombear ate acabar, chamando
    /// `ocioso` entre um passo e outro (spin_loop, wfi...). Nao manda STOP:
    /// o dispositivo ainda tem a cauda no mixer, espere uns 100 a 300 ms
    /// e chame `parar` e `liberar`.
    pub fn tocar(&mut self, fonte: &mut impl Fonte, mut ocioso: impl FnMut()) -> Result<Estatisticas, ErroSom> {
        self.comecar(fonte)?;
        while !self.bombear(fonte)? {
            ocioso();
        }
        Ok(self.estatisticas)
    }

    /// PCM_STOP.
    pub fn parar(&mut self) -> Result<(), ErroSom> {
        self.pedido_simples(PCM_STOP, self.stream)
    }

    /// PCM_RELEASE: o dispositivo devolve o que sobrou na tx.
    pub fn liberar(&mut self) -> Result<(), ErroSom> {
        self.pedido_simples(PCM_RELEASE, self.stream)?;
        self.recolher_tx();
        self.configurado = false;
        Ok(())
    }

    /// Reset do dispositivo: ele para de usar a memoria de DMA.
    pub fn encerrar(self) {
        self.disp.resetar();
    }
}
