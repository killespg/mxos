//! Drivers: o que existe na maquina e qual driver do MXOS cuida de cada
//! coisa. Alimenta o app Drivers e os comandos "drivers" e "tarefas".
//!
//! A sondagem so LE registradores que nao mudam nada no dispositivo:
//!   - CPU: MIDR_EL1, CurrentEL e CNTFRQ_EL0
//!   - GIC: GICD_TYPER e GICD_ISENABLERn (quais interrupcoes estao ligadas)
//!   - PL011 e PL031: os registradores de identificacao PrimeCell
//!   - fw_cfg: a assinatura do registrador de DMA
//!   - virtio-mmio: MagicValue, Version, DeviceID, VendorID e Status
//! Nada aqui escreve, reseta ou reconfigura um dispositivo, nem os que um
//! driver usa. Pode ser chamado a qualquer hora, menos numa interrupcao
//! (aloca Strings).
//!
//! Um slot virtio tem driver ativo quando o Status tem DRIVER_OK. O resto
//! (IP, disco montado, nome do teclado) vem dos proprios drivers:
//! rede::info, armazenamento::discos e as descricoes da entrada
//! (definir_entrada, chamado pelo gui::rodar). O video vem do terminal
//! (definir_video, chamado pelo shell).

// Na TV box ninguem chama isto ainda (la nao ha area de trabalho).
#![cfg_attr(feature = "mxq", allow(dead_code))]

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::fmt::Write;
use core::ptr::read_volatile;

use crate::plataforma::{GIC_CPU, GIC_DISTRIBUIDOR, IRQ_SERIAL, IRQ_TIMER};
use crate::terminal::Video;
use crate::{cpu, gic, irq, timer, uart};

macro_rules! diga {
    ($saida:expr, $($arg:tt)*) => {{ let _ = writeln!($saida, $($arg)*); }};
}

/// Driver de quem nao tem driver.
const SEM_DRIVER: &str = "-";
/// GICD_ISENABLERn: um bit por interrupcao ligada (ler nao muda nada).
const GICD_LIGADAS: usize = 0x100;
/// INTID 0-15 sao SGI (entre CPUs), 16-31 PPI (de cada CPU), 32+ SPI.
const PRIMEIRA_PPI: u32 = 16;
const PRIMEIRA_SPI: u32 = 32;
// Colunas do comando "drivers": cabe no terminal de 80 colunas.
const COLUNA_DISPOSITIVO: usize = 21;
const COLUNA_DRIVER: usize = 12;
const COLUNA_ESTADO: usize = 25;
const COLUNA_JANELA: usize = 28;

/// Que tipo de coisa o dispositivo e. A lista sai agrupada nesta ordem.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Categoria {
    Processador,
    Interrupcoes,
    Relogio,
    Serial,
    Video,
    Som,
    Entrada,
    Disco,
    Rede,
    Outro,
}

impl Categoria {
    pub fn nome(self) -> &'static str {
        match self {
            Categoria::Processador => "Processador",
            Categoria::Interrupcoes => "Interrupções",
            Categoria::Relogio => "Relógio",
            Categoria::Serial => "Serial",
            Categoria::Video => "Vídeo",
            Categoria::Som => "Som",
            Categoria::Entrada => "Entrada",
            Categoria::Disco => "Disco",
            Categoria::Rede => "Rede",
            Categoria::Outro => "Outro",
        }
    }
}

/// Um dispositivo e quem cuida dele.
#[derive(Clone)]
pub struct InfoDriver {
    pub categoria: Categoria,
    pub dispositivo: String,
    /// Arquivo do driver no MXOS ("virtio_blk", "gic"...); "-" = nenhum.
    pub driver: &'static str,
    /// Base dos registradores (0 = registradores do sistema, sem endereco).
    pub endereco: usize,
    /// INTID no GIC, se o dispositivo tem interrupcao.
    pub irq: Option<u32>,
    /// Resumo curto ("ativo, 10.0.2.15", "sem driver"...). Comeca com
    /// "ativo" quando um driver do MXOS esta cuidando dele agora.
    pub estado: String,
}

impl InfoDriver {
    pub fn ativo(&self) -> bool {
        self.estado.starts_with("ativo")
    }
}

/// Valor que so o laco principal usa (nunca uma interrupcao): basta um
/// UnsafeCell sem trava.
struct SoLaco<T>(UnsafeCell<T>);

// Seguranca: so o laco principal le e escreve (ver acima).
unsafe impl<T: Send> Sync for SoLaco<T> {}

impl<T> SoLaco<T> {
    fn com<R>(&self, acao: impl FnOnce(&mut T) -> R) -> R {
        acao(unsafe { &mut *self.0.get() })
    }
}

/// Uma linha por teclado ou mouse, como a entrada descreveu no boot.
static ENTRADA: SoLaco<Vec<String>> = SoLaco(UnsafeCell::new(Vec::new()));
/// O video que o terminal ligou (None = desligado).
static VIDEO: SoLaco<Option<Video>> = SoLaco(UnsafeCell::new(None));

/// Entrada::descricoes(), entregue pelo gui::rodar.
pub fn definir_entrada(descricoes: Vec<String>) {
    ENTRADA.com(|e| *e = descricoes);
}

/// O video do terminal, entregue pelo shell.
pub fn definir_video(video: Option<Video>) {
    VIDEO.com(|v| *v = video);
}

/// Le um registrador de 32 bits de um periferico mapeado. So para os
/// registradores que ler nao muda nada (ver o topo).
fn ler32(endereco: usize) -> u32 {
    unsafe { read_volatile(endereco as *const u32) }
}

/// O comeco de `texto` com no maximo `maximo` caracteres.
fn cortar(texto: &str, maximo: usize) -> &str {
    match texto.char_indices().nth(maximo) {
        Some((fim, _)) => &texto[..fim],
        None => texto,
    }
}

/// "0x0a003e00", ou "-" para os registradores do sistema.
pub fn texto_endereco(endereco: usize) -> String {
    if endereco == 0 { String::from("-") } else { format!("0x{endereco:08x}") }
}

/// "76", ou "-" sem interrupcao.
pub fn texto_irq(irq: Option<u32>) -> String {
    match irq {
        Some(id) => format!("{id}"),
        None => String::from("-"),
    }
}

// ---------------------------------------------------------------- sondagem

/// Tudo que achamos, agrupado por categoria (na ordem do enum).
pub fn listar() -> Vec<InfoDriver> {
    let mut lista = Vec::new();
    lista.push(processador());
    lista.push(controlador_de_interrupcoes());
    lista.push(timer_generico());
    lista.push(porta_serial());
    maquina::sondar(&mut lista);
    // estavel: dentro de cada categoria fica a ordem da sondagem
    lista.sort_by_key(|d| d.categoria as u8);
    lista
}

fn processador() -> InfoDriver {
    InfoDriver {
        categoria: Categoria::Processador,
        dispositivo: String::from(cpu::nome_cpu(cpu::midr())),
        driver: "cpu",
        endereco: 0,
        irq: None,
        estado: format!("ativo, EL{}", cpu::el_atual()),
    }
}

fn controlador_de_interrupcoes() -> InfoDriver {
    InfoDriver {
        categoria: Categoria::Interrupcoes,
        dispositivo: String::from("GICv2"),
        driver: "gic",
        endereco: GIC_DISTRIBUIDOR,
        irq: None,
        estado: format!("ativo, {} linhas", gic::linhas()),
    }
}

fn timer_generico() -> InfoDriver {
    InfoDriver {
        categoria: Categoria::Relogio,
        dispositivo: String::from("Timer virtual do ARM"),
        driver: "timer",
        endereco: 0,
        irq: Some(IRQ_TIMER),
        estado: format!("ativo, {} Hz", timer::HZ),
    }
}

fn porta_serial() -> InfoDriver {
    InfoDriver {
        categoria: Categoria::Serial,
        dispositivo: String::from(maquina::NOME_SERIAL),
        driver: "uart",
        endereco: maquina::BASE_SERIAL,
        irq: Some(IRQ_SERIAL),
        estado: String::from("ativo (console)"),
    }
}

/// A interrupcao `id` esta ligada no distribuidor do GIC?
fn irq_ligada(id: u32) -> bool {
    id < gic::linhas() && ler32(GIC_DISTRIBUIDOR + GICD_LIGADAS + id as usize / 32 * 4) >> (id % 32) & 1 != 0
}

/// INTIDs ligados no GIC, das PPI para cima (as SGI o QEMU deixa sempre ligadas).
fn interrupcoes_ligadas() -> Vec<u32> {
    let mut ligadas = Vec::new();
    for registrador in 0..gic::linhas() / 32 {
        let bits = ler32(GIC_DISTRIBUIDOR + GICD_LIGADAS + registrador as usize * 4);
        ligadas.extend((0..32).filter(|b| bits >> b & 1 != 0).map(|b| registrador * 32 + b));
    }
    ligadas.retain(|&id| id >= PRIMEIRA_PPI);
    ligadas
}

// ---------------------------------------------------------------- detalhes

/// Linhas extras sobre o dispositivo, para o painel de detalhes do app.
pub fn detalhes(d: &InfoDriver) -> Vec<String> {
    let mut linhas = Vec::new();
    match d.driver {
        "cpu" => detalhes_cpu(&mut linhas),
        "gic" => detalhes_gic(&mut linhas),
        "timer" => detalhes_timer(&mut linhas),
        "uart" => detalhes_serial(&mut linhas),
        _ => {}
    }
    maquina::detalhes(d, &mut linhas);
    if let Some(id) = d.irq {
        linhas.push(descrever_irq(id));
    }
    linhas
}

fn detalhes_cpu(linhas: &mut Vec<String>) {
    let midr = cpu::midr();
    linhas.push(format!(
        "MIDR 0x{midr:08x}: fabricante 0x{:02x}, peça 0x{:03x}, revisão r{}p{}",
        midr >> 24 & 0xFF,
        midr >> 4 & 0xFFF,
        midr >> 20 & 0xF,
        midr & 0xF
    ));
    linhas.push(format!("Rodando em EL{}; o MXOS usa só o núcleo 0", cpu::el_atual()));
}

fn detalhes_gic(linhas: &mut Vec<String>) {
    linhas.push(format!("Distribuidor em 0x{GIC_DISTRIBUIDOR:08x}, interface da CPU em 0x{GIC_CPU:08x}"));
    let ligadas: Vec<String> = interrupcoes_ligadas().iter().map(|id| format!("{id}")).collect();
    linhas.push(format!("{} linhas; ligadas agora (sem contar as SGI): {}", gic::linhas(), ligadas.join(", ")));
}

fn detalhes_timer(linhas: &mut Vec<String>) {
    let freq = cpu::frequencia_timer();
    linhas.push(format!(
        "Contador a {}.{} MHz; o timer virtual dispara {} vezes por segundo",
        freq / 1_000_000,
        freq / 100_000 % 10,
        timer::HZ
    ));
    linhas.push(format!("{} tiques desde que as interrupções ligaram", timer::tiques()));
}

fn detalhes_serial(linhas: &mut Vec<String>) {
    let c = irq::contadores();
    linhas.push(String::from(uart::NOME));
    linhas.push(format!("{} interrupções de recepção, {} bytes recebidos", c.serial, c.bytes_serial));
}

/// "Interrupcao: INTID 76 (SPI 44), desligada no GIC".
fn descrever_irq(id: u32) -> String {
    let (tipo, numero) = match id {
        0..PRIMEIRA_PPI => ("SGI", id),
        PRIMEIRA_PPI..PRIMEIRA_SPI => ("PPI", id - PRIMEIRA_PPI),
        _ => ("SPI", id - PRIMEIRA_SPI),
    };
    let estado = if irq_ligada(id) { "ligada" } else { "desligada" };
    format!("Interrupção: INTID {id} ({tipo} {numero}), {estado} no GIC")
}

// ---------------------------------------------------------------- comandos

/// Comandos "drivers" e "tarefas" do shell. false = nao e daqui.
pub fn comando(nome: &str, _args: &str, saida: &mut dyn Write) -> bool {
    match nome {
        "drivers" => mostrar_drivers(saida),
        "tarefas" => maquina::mostrar_tarefas(saida),
        _ => return false,
    }
    true
}

/// Tabela agrupada por categoria, com uma linha por dispositivo.
fn mostrar_drivers(saida: &mut dyn Write) {
    let lista = listar();
    let ativos = lista.iter().filter(|d| d.ativo()).count();
    diga!(saida, "Dispositivos: {} achados, {} com driver ativo", lista.len(), ativos);
    linha_driver(saida, "dispositivo", "driver", "endereço", "IRQ", "estado");
    let mut categoria = None;
    for d in &lista {
        if categoria != Some(d.categoria) {
            categoria = Some(d.categoria);
            diga!(saida, "{}", d.categoria.nome());
        }
        let (endereco, irq) = (texto_endereco(d.endereco), texto_irq(d.irq));
        linha_driver(saida, &d.dispositivo, d.driver, &endereco, &irq, &d.estado);
    }
}

fn linha_driver(saida: &mut dyn Write, dispositivo: &str, driver: &str, endereco: &str, irq: &str, estado: &str) {
    diga!(
        saida,
        "  {:<w$} {:<d$} {:<10} {:>4}  {}",
        cortar(dispositivo, COLUNA_DISPOSITIVO),
        driver,
        endereco,
        irq,
        cortar(estado, COLUNA_ESTADO),
        w = COLUNA_DISPOSITIVO,
        d = COLUNA_DRIVER
    );
}

/// O que so existe no QEMU virt: RTC, ramfb, os 32 slots virtio e as janelas.
#[cfg(not(any(feature = "mxq", feature = "rk322x")))]
mod maquina {
    use alloc::format;
    use alloc::string::String;
    use alloc::vec::Vec;
    use core::fmt::Write;
    use core::ptr::read_volatile;

    use super::{COLUNA_JANELA, Categoria, ENTRADA, InfoDriver, SEM_DRIVER, VIDEO, cortar, ler32};
    use crate::armazenamento::{self, InfoDisco};
    use crate::cpu;
    use crate::entrada::virtio_input::TIPO_ENTRADA;
    use crate::rede::{self, Ip, Mac};
    use crate::video::fwcfg;
    use crate::virtio::virtio_mmio::{
        BASE_QEMU_VIRT, ESTADO_ACK, ESTADO_DRIVER, ESTADO_DRIVER_OK, ESTADO_FALHOU, ESTADO_FEATURES_OK,
        ESTADO_PRECISA_RESET, INTID_PRIMEIRO_SLOT, PASSO_QEMU_VIRT, SLOTS_QEMU_VIRT, TIPO_BLOCO, TIPO_GPU, TIPO_REDE,
        TIPO_SOM,
    };

    pub const NOME_SERIAL: &str = "UART PL011";
    pub const BASE_SERIAL: usize = 0x0900_0000;
    /// RTC PL031 do QEMU virt (SPI 2). RTCDR, no offset 0, conta os
    /// segundos desde 1970 (UTC).
    const BASE_RTC: usize = 0x0901_0000;
    const IRQ_RTC: u32 = 34;
    /// Numeros de peca PrimeCell e a assinatura que todo PrimeCell tem.
    const PECA_PL011: u32 = 0x011;
    const PECA_PL031: u32 = 0x031;
    const ID_PRIMECELL: u32 = 0xB105_F00D;
    /// O registrador de DMA do fw_cfg devolve "QEMU CFG" numa leitura.
    const DMA_FWCFG: usize = 0x10;
    const ASSINATURA_DMA: u64 = 0x5145_4D55_2043_4647;
    /// "virt" em little endian.
    const MAGICO: u32 = 0x7472_6976;
    // Registradores de identificacao do virtio-mmio (spec 4.2.2).
    const R_MAGICO: usize = 0x000;
    const R_VERSAO: usize = 0x004;
    const R_TIPO: usize = 0x008;
    const R_FABRICANTE: usize = 0x00c;
    const R_ESTADO: usize = 0x070;

    /// Nome de cada DeviceID do virtio (spec 1.2, secao 5).
    const NOMES_VIRTIO: [(u32, &str); 13] = [
        (TIPO_REDE, "virtio-net"),
        (TIPO_BLOCO, "virtio-blk"),
        (3, "virtio-console"),
        (4, "virtio-rng"),
        (5, "virtio-balloon"),
        (8, "virtio-scsi"),
        (9, "virtio-9p"),
        (TIPO_GPU, "virtio-gpu"),
        (TIPO_ENTRADA, "virtio-input"),
        (19, "virtio-vsock"),
        (20, "virtio-crypto"),
        (TIPO_SOM, "virtio-sound"),
        (26, "virtio-fs"),
    ];

    /// Bits do Status do virtio e o nome de cada um.
    const BITS_ESTADO: [(u32, &str); 6] = [
        (ESTADO_ACK, "ACK"),
        (ESTADO_DRIVER, "DRIVER"),
        (ESTADO_FEATURES_OK, "FEATURES_OK"),
        (ESTADO_DRIVER_OK, "DRIVER_OK"),
        (ESTADO_PRECISA_RESET, "NEEDS_RESET"),
        (ESTADO_FALHOU, "FAILED"),
    ];

    pub fn sondar(lista: &mut Vec<InfoDriver>) {
        lista.extend(rtc());
        lista.push(ramfb());
        lista.extend((0..SLOTS_QEMU_VIRT).filter_map(Slot::ler).map(|slot| slot.info()));
    }

    pub fn detalhes(d: &InfoDriver, linhas: &mut Vec<String>) {
        match d.endereco {
            BASE_SERIAL => detalhes_pl011(linhas),
            BASE_RTC => detalhes_rtc(linhas),
            fwcfg::BASE_QEMU_VIRT => detalhes_ramfb(linhas),
            endereco => {
                if let Some(slot) = Slot::do_endereco(endereco) {
                    slot.detalhes(linhas);
                }
            }
        }
    }

    /// (peca, revisao) dos registradores de identificacao PrimeCell, no fim
    /// da pagina do periferico. None se a assinatura nao confere.
    fn primecell(base: usize) -> Option<(u32, u32)> {
        let byte = |offset: usize| ler32(base + offset) & 0xFF;
        let assinatura = byte(0xFF0) | byte(0xFF4) << 8 | byte(0xFF8) << 16 | byte(0xFFC) << 24;
        (assinatura == ID_PRIMECELL).then(|| (byte(0xFE0) | (byte(0xFE4) & 0xF) << 8, byte(0xFE8) >> 4))
    }

    fn detalhes_pl011(linhas: &mut Vec<String>) {
        if let Some((PECA_PL011, revisao)) = primecell(BASE_SERIAL) {
            linhas.push(format!("PrimeCell PL011 da ARM, revisão {revisao}"));
        }
    }

    // ------------------------------------------------------------ relogio

    fn rtc() -> Option<InfoDriver> {
        let (peca, _) = primecell(BASE_RTC)?;
        (peca == PECA_PL031).then(|| InfoDriver {
            categoria: Categoria::Relogio,
            dispositivo: String::from("RTC PL031"),
            // quem le o RTC e o relogio do armazenamento, para as datas do FAT32
            driver: "fat32",
            endereco: BASE_RTC,
            irq: Some(IRQ_RTC),
            estado: format!("ativo, {} UTC", hora_rtc()),
        })
    }

    /// "22:45:10", a hora (UTC) do RTC.
    fn hora_rtc() -> String {
        let segundos = ler32(BASE_RTC) % 86_400;
        format!("{:02}:{:02}:{:02}", segundos / 3600, segundos / 60 % 60, segundos % 60)
    }

    fn detalhes_rtc(linhas: &mut Vec<String>) {
        linhas.push(format!("PrimeCell PL031: {} segundos desde 1970, {} UTC", ler32(BASE_RTC), hora_rtc()));
        linhas.push(String::from("O FAT32 lê a data e a hora daqui para gravar nos arquivos"));
    }

    // ------------------------------------------------------------ video

    fn ramfb() -> InfoDriver {
        let estado = match VIDEO.com(|v| *v) {
            Some(v) => format!("ativo, {}x{}", v.largura, v.altura),
            None => String::from("desligado"),
        };
        InfoDriver {
            categoria: Categoria::Video,
            dispositivo: String::from("ramfb (fw_cfg)"),
            driver: "ramfb",
            endereco: fwcfg::BASE_QEMU_VIRT,
            irq: None,
            estado,
        }
    }

    fn detalhes_ramfb(linhas: &mut Vec<String>) {
        // a mesma leitura que o fwcfg::tem_dma faz: so devolve a assinatura
        let dma = unsafe { read_volatile((fwcfg::BASE_QEMU_VIRT + DMA_FWCFG) as *const u64) };
        let com_dma = if u64::from_be(dma) == ASSINATURA_DMA { "com DMA" } else { "sem DMA" };
        linhas.push(format!("fw_cfg do QEMU em 0x{:08x}, {com_dma}", fwcfg::BASE_QEMU_VIRT));
        match VIDEO.com(|v| *v) {
            Some(v) => {
                let kb = v.largura * v.altura * 4 / 1024;
                linhas.push(format!("{}: {}x{} em 0x{:08x} ({kb} KB da RAM)", v.nome, v.largura, v.altura, v.base));
                linhas.push(String::from("A área de trabalho desenha na RAM e só copia o que mudou"));
            }
            None => linhas.push(String::from("O terminal não conseguiu ligar o vídeo no boot")),
        }
    }

    // ------------------------------------------------------------ virtio

    /// Os registradores de identificacao de um slot virtio-mmio.
    struct Slot {
        numero: usize,
        versao: u32,
        tipo: u32,
        fabricante: u32,
        estado: u32,
    }

    impl Slot {
        fn base(numero: usize) -> usize {
            BASE_QEMU_VIRT + numero * PASSO_QEMU_VIRT
        }

        /// None se o slot esta vazio (DeviceID 0) ou nao e virtio.
        fn ler(numero: usize) -> Option<Slot> {
            let base = Slot::base(numero);
            if ler32(base + R_MAGICO) != MAGICO {
                return None;
            }
            let tipo = ler32(base + R_TIPO);
            (tipo != 0).then(|| Slot {
                numero,
                versao: ler32(base + R_VERSAO),
                tipo,
                fabricante: ler32(base + R_FABRICANTE),
                estado: ler32(base + R_ESTADO),
            })
        }

        /// O slot cujos registradores comecam em `endereco`.
        fn do_endereco(endereco: usize) -> Option<Slot> {
            let deslocamento = endereco.checked_sub(BASE_QEMU_VIRT)?;
            let numero = deslocamento / PASSO_QEMU_VIRT;
            if deslocamento % PASSO_QEMU_VIRT != 0 || numero >= SLOTS_QEMU_VIRT {
                return None;
            }
            Slot::ler(numero)
        }

        fn com_driver(&self) -> bool {
            self.estado & ESTADO_DRIVER_OK != 0
        }

        fn info(&self) -> InfoDriver {
            InfoDriver {
                categoria: categoria(self.tipo),
                dispositivo: self.nome(),
                driver: driver(self.tipo),
                endereco: Slot::base(self.numero),
                irq: Some(INTID_PRIMEIRO_SLOT + self.numero as u32),
                estado: self.resumo(),
            }
        }

        /// Teclado e mouse tem o nome que a entrada leu; o resto, o do tipo.
        fn nome(&self) -> String {
            let entrada = if self.tipo == TIPO_ENTRADA { entrada_do_slot(self.numero) } else { None };
            match entrada {
                Some((nome, _)) => nome,
                None => nome_virtio(self.tipo),
            }
        }

        /// O estado curto da lista.
        fn resumo(&self) -> String {
            if self.estado & (ESTADO_FALHOU | ESTADO_PRECISA_RESET) != 0 {
                return String::from("com erro (precisa reset)");
            }
            if !self.com_driver() {
                return String::from(match self.tipo {
                    TIPO_SOM => "livre (liga ao tocar)",
                    TIPO_BLOCO => "parado (não montado)",
                    TIPO_REDE => "desligado",
                    _ => "sem driver",
                });
            }
            let resumo = match self.tipo {
                TIPO_REDE => rede::info().map(|i| format!("ativo, {}", Ip(i.ip))),
                TIPO_BLOCO => disco_montado().map(|d| format!("ativo, {} em /", d.nome)),
                TIPO_ENTRADA => entrada_do_slot(self.numero).map(|(_, tipo)| format!("ativo ({tipo})")),
                TIPO_SOM => Some(String::from("ativo (tocando)")),
                _ => None,
            };
            resumo.unwrap_or_else(|| String::from("ativo"))
        }

        fn detalhes(&self, linhas: &mut Vec<String>) {
            let legado = if self.versao == 1 { " (legado)" } else { "" };
            linhas.push(format!(
                "virtio-mmio slot {}, versão {}{legado}, fabricante {}",
                self.numero,
                self.versao,
                fabricante(self.fabricante)
            ));
            linhas.push(format!("Status 0x{:02x}: {}", self.estado, bits_do_estado(self.estado)));
            let ativo = self.com_driver();
            match self.tipo {
                TIPO_REDE if ativo => detalhes_rede(linhas),
                TIPO_BLOCO if ativo => detalhes_disco(linhas),
                TIPO_BLOCO => linhas.push(String::from("Só o primeiro disco com FAT32 é montado em /")),
                TIPO_SOM => linhas.push(String::from("O comando som liga o driver, toca e reseta o dispositivo")),
                TIPO_GPU => linhas.push(String::from("Sem driver: o vídeo do MXOS usa o ramfb")),
                TIPO_REDE | TIPO_ENTRADA => {}
                _ => linhas.push(String::from("O MXOS ainda não tem driver para este tipo de dispositivo")),
            }
            if ativo {
                linhas.push(String::from("Filas atendidas por polling: o driver não liga a interrupção"));
            }
        }
    }

    fn categoria(tipo: u32) -> Categoria {
        match tipo {
            TIPO_REDE => Categoria::Rede,
            TIPO_BLOCO => Categoria::Disco,
            TIPO_GPU => Categoria::Video,
            TIPO_ENTRADA => Categoria::Entrada,
            TIPO_SOM => Categoria::Som,
            _ => Categoria::Outro,
        }
    }

    fn driver(tipo: u32) -> &'static str {
        match tipo {
            TIPO_REDE => "virtio_net",
            TIPO_BLOCO => "virtio_blk",
            TIPO_ENTRADA => "virtio_input",
            TIPO_SOM => "virtio_som",
            _ => SEM_DRIVER,
        }
    }

    fn nome_virtio(tipo: u32) -> String {
        match NOMES_VIRTIO.iter().find(|&&(t, _)| t == tipo) {
            Some(&(_, nome)) => String::from(nome),
            None => format!("virtio tipo {tipo}"),
        }
    }

    /// VendorID: quatro letras ASCII (0x554d4551 = "QEMU") ou so o numero.
    fn fabricante(id: u32) -> String {
        let letras = id.to_le_bytes();
        if !letras.iter().all(u8::is_ascii_alphanumeric) {
            return format!("0x{id:08x}");
        }
        let nome: String = letras.iter().map(|&c| c as char).collect();
        format!("{nome} (0x{id:08x})")
    }

    /// "ACK, DRIVER, FEATURES_OK, DRIVER_OK".
    fn bits_do_estado(estado: u32) -> String {
        let nomes: Vec<&str> =
            BITS_ESTADO.iter().filter(|&&(bit, _)| estado & bit != 0).map(|&(_, nome)| nome).collect();
        if nomes.is_empty() { String::from("zerado (nenhum driver)") } else { nomes.join(", ") }
    }

    /// Nome e tipo do teclado/mouse do slot, das descricoes da entrada.
    fn entrada_do_slot(numero: usize) -> Option<(String, String)> {
        ENTRADA.com(|descricoes| {
            descricoes
                .iter()
                .filter_map(|d| ler_descricao(d))
                .find(|&(_, _, slot)| slot == numero)
                .map(|(nome, tipo, _)| (String::from(nome), String::from(tipo)))
        })
    }

    /// "QEMU Virtio Keyboard (teclado, slot 30)" -> (nome, tipo, slot),
    /// o formato de Entrada::descricoes.
    fn ler_descricao(d: &str) -> Option<(&str, &str, usize)> {
        let (nome, resto) = d.rsplit_once(" (")?;
        let (tipo, slot) = resto.strip_suffix(')')?.split_once(", slot ")?;
        Some((nome, tipo, slot.parse().ok()?))
    }

    fn disco_montado() -> Option<InfoDisco> {
        armazenamento::discos().into_iter().find(|d| d.montado)
    }

    fn detalhes_disco(linhas: &mut Vec<String>) {
        if let Some(d) = disco_montado() {
            linhas.push(format!(
                "{}: {} MB, {} \"{}\" montado em /, {} MB livres",
                d.nome,
                (d.setores * 512) >> 20,
                d.sistema,
                d.rotulo,
                d.livre >> 20
            ));
        }
    }

    fn detalhes_rede(linhas: &mut Vec<String>) {
        let Some(i) = rede::info() else { return };
        linhas.push(format!("MAC {}  IP {}  máscara {}", Mac(i.mac), Ip(i.ip), Ip(i.mascara)));
        linhas.push(format!(
            "Gateway {}  DNS {}  pacotes: {} recebidos, {} enviados",
            Ip(i.gateway),
            Ip(i.dns),
            i.pacotes_rx,
            i.pacotes_tx
        ));
    }

    // ------------------------------------------------------------ janelas

    /// Comando "tarefas": as janelas do gerenciador (janelas::resumo).
    pub fn mostrar_tarefas(saida: &mut dyn Write) {
        let janelas = crate::gui::janelas::resumo();
        if janelas.is_empty() {
            // o gui::rodar entrega as descricoes da entrada assim que comeca
            if ENTRADA.com(|e| e.is_empty()) {
                diga!(saida, "Nenhuma janela: a área de trabalho não está rodando");
            } else {
                diga!(saida, "Nenhuma janela no resumo ainda (o gerenciador atualiza 10 vezes por segundo)");
            }
            return;
        }
        let freq = cpu::frequencia_timer().max(1);
        diga!(saida, "Janelas abertas: {}", janelas.len());
        diga!(saida, "  {:>4}  {:<w$} {:>9} {:>9}", "id", "janela", "CPU", "memória", w = COLUNA_JANELA);
        for j in &janelas {
            let ms = j.tempo_cpu.saturating_mul(1000) / freq;
            let foco = if j.focada { "  (em foco)" } else { "" };
            diga!(
                saida,
                "  {:>4}  {:<w$} {:>6} ms {:>6} KB{foco}",
                j.id,
                cortar(&j.titulo, COLUNA_JANELA),
                ms,
                j.memoria / 1024,
                w = COLUNA_JANELA
            );
        }
        diga!(saida, "CPU: tempo gasto nos eventos e no desenho de cada janela desde que abriu");
    }
}

/// Na TV box (ainda sem testar) so existe o que e de todo ARM: CPU, GIC,
/// timer e serial. Sem virtio, disco, rede nem area de trabalho.
#[cfg(any(feature = "mxq", feature = "rk322x"))]
mod maquina {
    use alloc::string::String;
    use alloc::vec::Vec;
    use core::fmt::Write;

    use super::InfoDriver;

    pub const NOME_SERIAL: &str = "UART_AO_A (Amlogic)";
    pub const BASE_SERIAL: usize = 0xC810_04C0;

    pub fn sondar(_lista: &mut Vec<InfoDriver>) {}

    pub fn detalhes(_d: &InfoDriver, _linhas: &mut Vec<String>) {}

    pub fn mostrar_tarefas(saida: &mut dyn Write) {
        diga!(saida, "tarefas: a TV box ainda não tem área de trabalho");
    }
}
