//! Transporte virtio-mmio moderno (versao 2), o jeito como o QEMU virt liga
//! dispositivos virtio (som, gpu, rede...) ao processador.
//!
//! No QEMU virt sao 32 "slots" de 0x200 bytes a partir de 0x0a000000. O slot
//! i usa a SPI 16+i (INTID 48+i, sensivel a nivel). O primeiro -device da
//! linha de comando cai no slot de MAIOR endereco, entao nunca fixamos o
//! endereco: varremos os slots procurando o tipo de dispositivo.
//!
//! Atencao: o QEMU virt usa o layout legado (versao 1) por padrao. Para ter a
//! versao 2 e preciso "-global virtio-mmio.force-legacy=false".
//!
//! Todos os registradores sao de 32 bits, acesso alinhado e volatil.

use core::arch::asm;
use core::ptr::{read_volatile, write_volatile};

/// Slots do virtio-mmio no QEMU virt.
pub const BASE_QEMU_VIRT: usize = 0x0a00_0000;
pub const PASSO_QEMU_VIRT: usize = 0x200;
/// Na TV box nao ha virtio: nenhum slot (a busca nao le endereco nenhum).
#[cfg(not(feature = "mxq"))]
pub const SLOTS_QEMU_VIRT: usize = 32;
#[cfg(feature = "mxq")]
pub const SLOTS_QEMU_VIRT: usize = 0;
/// INTID do GIC do slot 0 (SPI 16 = 32 + 16).
pub const INTID_PRIMEIRO_SLOT: u32 = 48;

/// Tipos de dispositivo (campo DeviceID).
pub const TIPO_REDE: u32 = 1;
pub const TIPO_BLOCO: u32 = 2;
pub const TIPO_GPU: u32 = 16;
pub const TIPO_SOM: u32 = 25;

/// "virt" em little endian.
const MAGICO: u32 = 0x7472_6976;

// Registradores (offset a partir da base do slot).
const R_MAGICO: usize = 0x000;
const R_VERSAO: usize = 0x004;
const R_TIPO: usize = 0x008;
const R_FABRICANTE: usize = 0x00c;
const R_FEATURES_DISP: usize = 0x010;
const R_FEATURES_DISP_SEL: usize = 0x014;
const R_FEATURES_DRV: usize = 0x020;
const R_FEATURES_DRV_SEL: usize = 0x024;
const R_FILA_SEL: usize = 0x030;
const R_FILA_MAX: usize = 0x034;
const R_FILA_TAMANHO: usize = 0x038;
const R_FILA_PRONTA: usize = 0x044;
const R_FILA_AVISO: usize = 0x050;
const R_INT_ESTADO: usize = 0x060;
const R_INT_CONFIRMA: usize = 0x064;
const R_ESTADO: usize = 0x070;
const R_FILA_DESC: usize = 0x080; // low em +0, high em +4
const R_FILA_AVAIL: usize = 0x090; // "driver area"
const R_FILA_USED: usize = 0x0a0; // "device area"
const R_GERACAO_CONFIG: usize = 0x0fc;
const R_CONFIG: usize = 0x100;

// Bits do registrador de estado.
pub const ESTADO_ACK: u32 = 1;
pub const ESTADO_DRIVER: u32 = 2;
pub const ESTADO_DRIVER_OK: u32 = 4;
pub const ESTADO_FEATURES_OK: u32 = 8;
pub const ESTADO_PRECISA_RESET: u32 = 64;
pub const ESTADO_FALHOU: u32 = 128;

/// Feature obrigatoria do modo moderno (bit 32).
pub const FEATURE_VERSION_1: u64 = 1 << 32;

/// Erros do transporte e das filas.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Erro {
    /// O dispositivo usa o layout legado (versao 1).
    VersaoLegada(u32),
    /// O dispositivo nao oferece VIRTIO_F_VERSION_1.
    SemVersion1,
    /// O dispositivo recusou as features (FEATURES_OK nao ficou ligado).
    FeaturesRecusadas,
    /// A fila nao existe (QueueNumMax = 0).
    FilaInexistente(u32),
    /// A fila ja estava pronta (QueueReady = 1) antes de configurarmos.
    FilaOcupada(u32),
    /// Tamanho de fila zero, maior que o maximo ou nao potencia de 2.
    TamanhoDeFilaInvalido(u32),
    /// Memoria da fila fora do alinhamento de 16 bytes.
    MemoriaDesalinhada,
}

impl Erro {
    pub fn descricao(&self) -> &'static str {
        match self {
            Erro::VersaoLegada(_) => {
                "virtio-mmio legado (versao 1): use -global virtio-mmio.force-legacy=false"
            }
            Erro::SemVersion1 => "dispositivo sem VIRTIO_F_VERSION_1",
            Erro::FeaturesRecusadas => "dispositivo recusou as features",
            Erro::FilaInexistente(_) => "fila inexistente",
            Erro::FilaOcupada(_) => "fila ja estava pronta",
            Erro::TamanhoDeFilaInvalido(_) => "tamanho de fila invalido",
            Erro::MemoriaDesalinhada => "memoria da fila desalinhada",
        }
    }
}

/// Barreira completa: tudo que escrevemos na RAM fica visivel ao dispositivo
/// antes do proximo acesso. Tambem impede o compilador de reordenar.
pub fn barreira() {
    unsafe { asm!("dsb sy", options(nostack, preserves_flags)) };
}

/// Um dispositivo virtio-mmio (um slot).
pub struct VirtioMmio {
    base: usize,
}

impl VirtioMmio {
    /// # Seguranca
    /// `base` tem de ser um slot virtio-mmio mapeado como dispositivo.
    pub const unsafe fn em(base: usize) -> VirtioMmio {
        VirtioMmio { base }
    }

    /// Procura nos slots do QEMU virt o primeiro dispositivo do `tipo` dado.
    /// So chame no QEMU: na TV box esses enderecos nao existem.
    pub fn procurar(tipo: u32) -> Option<VirtioMmio> {
        Self::procurar_em(BASE_QEMU_VIRT, PASSO_QEMU_VIRT, SLOTS_QEMU_VIRT, tipo)
    }

    /// Varre `slots` slots de `passo` bytes a partir de `base`.
    /// Slot com DeviceID 0 esta vazio e e ignorado.
    pub fn procurar_em(base: usize, passo: usize, slots: usize, tipo: u32) -> Option<VirtioMmio> {
        (0..slots)
            .map(|i| VirtioMmio { base: base + i * passo })
            .find(|d| d.ler(R_MAGICO) == MAGICO && d.tipo() == tipo)
    }

    fn ler(&self, offset: usize) -> u32 {
        unsafe { read_volatile((self.base + offset) as *const u32) }
    }

    fn escrever(&self, offset: usize, valor: u32) {
        unsafe { write_volatile((self.base + offset) as *mut u32, valor) }
    }

    /// Escreve um endereco de 64 bits num par de registradores low/high.
    fn escrever_endereco(&self, offset: usize, endereco: usize) {
        self.escrever(offset, endereco as u32);
        self.escrever(offset + 4, (endereco as u64 >> 32) as u32);
    }

    pub fn base(&self) -> usize {
        self.base
    }

    pub fn versao(&self) -> u32 {
        self.ler(R_VERSAO)
    }

    pub fn tipo(&self) -> u32 {
        self.ler(R_TIPO)
    }

    pub fn fabricante(&self) -> u32 {
        self.ler(R_FABRICANTE)
    }

    /// Numero do slot no QEMU virt (None se a base nao for de la).
    pub fn slot_qemu_virt(&self) -> Option<usize> {
        let fim = BASE_QEMU_VIRT + PASSO_QEMU_VIRT * SLOTS_QEMU_VIRT;
        if self.base < BASE_QEMU_VIRT || self.base >= fim {
            return None;
        }
        Some((self.base - BASE_QEMU_VIRT) / PASSO_QEMU_VIRT)
    }

    /// INTID do GIC deste slot no QEMU virt (sensivel a nivel).
    pub fn intid_qemu_virt(&self) -> Option<u32> {
        self.slot_qemu_virt().map(|s| INTID_PRIMEIRO_SLOT + s as u32)
    }

    pub fn estado(&self) -> u32 {
        self.ler(R_ESTADO)
    }

    fn ligar_estado(&self, bits: u32) {
        self.escrever(R_ESTADO, self.estado() | bits);
    }

    /// Estado 0: o dispositivo esquece tudo (filas, features, streams).
    pub fn resetar(&self) {
        self.escrever(R_ESTADO, 0);
    }

    /// Avisa o dispositivo que o driver desistiu dele.
    pub fn falhar(&self) {
        self.ligar_estado(ESTADO_FALHOU);
    }

    /// O dispositivo achou um erro grave e so volta com reset.
    pub fn precisa_reset(&self) -> bool {
        self.estado() & ESTADO_PRECISA_RESET != 0
    }

    /// As 64 features oferecidas pelo dispositivo.
    pub fn features_oferecidas(&self) -> u64 {
        self.escrever(R_FEATURES_DISP_SEL, 0);
        let baixo = self.ler(R_FEATURES_DISP) as u64;
        self.escrever(R_FEATURES_DISP_SEL, 1);
        let alto = self.ler(R_FEATURES_DISP) as u64;
        alto << 32 | baixo
    }

    fn escrever_features(&self, features: u64) {
        self.escrever(R_FEATURES_DRV_SEL, 0);
        self.escrever(R_FEATURES_DRV, features as u32);
        self.escrever(R_FEATURES_DRV_SEL, 1);
        self.escrever(R_FEATURES_DRV, (features >> 32) as u32);
    }

    /// Passos 1 a 6 da inicializacao (spec 3.1.1): reset, ACK, DRIVER,
    /// negociacao e FEATURES_OK. `aceitas` sao as features que o driver
    /// entende alem de VERSION_1. Devolve as features combinadas.
    /// Depois disto: configurar as filas e chamar `ativar`.
    pub fn iniciar(&self, aceitas: u64) -> Result<u64, Erro> {
        let versao = self.versao();
        if versao != 2 {
            return Err(Erro::VersaoLegada(versao));
        }
        self.resetar();
        self.ligar_estado(ESTADO_ACK);
        self.ligar_estado(ESTADO_DRIVER);
        let oferecidas = self.features_oferecidas();
        if oferecidas & FEATURE_VERSION_1 == 0 {
            self.falhar();
            return Err(Erro::SemVersion1);
        }
        let combinadas = oferecidas & (aceitas | FEATURE_VERSION_1);
        self.escrever_features(combinadas);
        // o QEMU so aplica as features do driver quando recebe FEATURES_OK
        self.ligar_estado(ESTADO_FEATURES_OK);
        if self.estado() & ESTADO_FEATURES_OK == 0 {
            self.falhar();
            return Err(Erro::FeaturesRecusadas);
        }
        Ok(combinadas)
    }

    /// Maior tamanho aceito pela fila (0 = a fila nao existe).
    pub fn tamanho_max_fila(&self, fila: u32) -> u32 {
        self.escrever(R_FILA_SEL, fila);
        self.ler(R_FILA_MAX)
    }

    /// Entrega ao dispositivo os tres pedacos de uma split virtqueue.
    /// QueueReady=1 aplica tudo, entao ele e escrito por ultimo.
    pub fn configurar_fila(&self, fila: u32, tamanho: u32, desc: usize, avail: usize, used: usize) -> Result<(), Erro> {
        self.escrever(R_FILA_SEL, fila);
        if self.ler(R_FILA_PRONTA) != 0 {
            return Err(Erro::FilaOcupada(fila));
        }
        let maximo = self.ler(R_FILA_MAX);
        if maximo == 0 {
            return Err(Erro::FilaInexistente(fila));
        }
        if tamanho == 0 || tamanho > maximo || !tamanho.is_power_of_two() {
            return Err(Erro::TamanhoDeFilaInvalido(tamanho));
        }
        if (desc | avail | used) & 0xF != 0 {
            return Err(Erro::MemoriaDesalinhada);
        }
        self.escrever(R_FILA_TAMANHO, tamanho);
        self.escrever_endereco(R_FILA_DESC, desc);
        self.escrever_endereco(R_FILA_AVAIL, avail);
        self.escrever_endereco(R_FILA_USED, used);
        barreira();
        self.escrever(R_FILA_PRONTA, 1);
        Ok(())
    }

    /// Passo 8 da inicializacao: o driver esta pronto (DRIVER_OK).
    /// Nenhum aviso de fila pode ser mandado antes disto.
    pub fn ativar(&self) {
        barreira();
        self.ligar_estado(ESTADO_DRIVER_OK);
    }

    /// Avisa que ha buffers novos na fila. A barreira garante que os
    /// descritores e o avail.idx ja estao na RAM.
    pub fn notificar(&self, fila: u32) {
        barreira();
        self.escrever(R_FILA_AVISO, fila);
    }

    /// Bits pendentes de interrupcao (bit 0 = buffer usado, bit 1 = config).
    pub fn estado_interrupcao(&self) -> u32 {
        self.ler(R_INT_ESTADO)
    }

    /// Confirma as interrupcoes pendentes e devolve quais eram. Obrigatorio
    /// se o INTID estiver ligado no GIC: o sinal e de nivel e fica preso.
    pub fn confirmar_interrupcao(&self) -> u32 {
        let pendentes = self.estado_interrupcao();
        self.escrever(R_INT_CONFIRMA, pendentes);
        pendentes
    }

    /// Le um u32 do espaco de configuracao do dispositivo. Repete se o
    /// dispositivo mudar a configuracao no meio da leitura.
    pub fn ler_config_u32(&self, offset: usize) -> u32 {
        loop {
            let antes = self.ler(R_GERACAO_CONFIG);
            let valor = self.ler(R_CONFIG + offset);
            if self.ler(R_GERACAO_CONFIG) == antes {
                return valor;
            }
        }
    }
}
