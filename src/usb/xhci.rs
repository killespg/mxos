//! xHCI (USB 3 / o DWC3 da Amlogic em modo host): um anel de comandos, um
//! de eventos e um anel por endpoint, tudo por polling. So o que o mod.rs
//! precisa: enderecar o dispositivo de cada porta, pedidos de controle,
//! bulk (pendrive) e interrupcao de entrada (teclado e mouse).

use alloc::alloc::{Layout, alloc_zeroed};
use alloc::vec;
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};

use crate::{cpu, mmu};

const TRBS: usize = 256;

// tipos de TRB
const NORMAL: u32 = 1;
const SETUP: u32 = 2;
const DADOS: u32 = 3;
const STATUS: u32 = 4;
const LINK: u32 = 6;
const HABILITAR_SLOT: u32 = 9;
const ENDERECAR: u32 = 11;
const CONFIGURAR_EP: u32 = 12;
const AVALIAR: u32 = 13;
const EVENTO_TRANSFERENCIA: u32 = 32;
const EVENTO_COMANDO: u32 = 33;

const IOC: u32 = 1 << 5;
const IDT: u32 = 1 << 6;
const ISP: u32 = 1 << 2;

#[derive(Clone, Copy, Default)]
#[repr(C)]
struct Trb {
    parametro: u64,
    status: u32,
    controle: u32,
}

/// Memoria para o controlador (nunca devolvida; alinhada em 4 KB).
fn alocar(tamanho: usize) -> usize {
    let layout = Layout::from_size_align(tamanho.max(64), 4096).unwrap();
    let p = unsafe { alloc_zeroed(layout) };
    assert!(!p.is_null(), "usb: sem memória");
    p as usize
}

fn limpar(endereco: usize, tamanho: usize) {
    mmu::limpar_cache(endereco, tamanho);
}

struct Anel {
    base: usize,
    indice: usize,
    ciclo: bool,
}

impl Anel {
    fn novo() -> Anel {
        let base = alocar(TRBS * 16);
        // o ultimo TRB volta para o comeco e troca o ciclo
        let link = Trb { parametro: base as u64, status: 0, controle: LINK << 10 | 1 << 1 };
        unsafe { write_volatile((base + (TRBS - 1) * 16) as *mut Trb, link) };
        limpar(base, TRBS * 16);
        Anel { base, indice: 0, ciclo: true }
    }

    /// Poe um TRB (com o bit de ciclo certo). Devolve o endereco dele.
    fn por(&mut self, mut trb: Trb) -> usize {
        trb.controle = (trb.controle & !1) | self.ciclo as u32;
        let endereco = self.base + self.indice * 16;
        unsafe { write_volatile(endereco as *mut Trb, trb) };
        limpar(endereco, 16);
        self.indice += 1;
        if self.indice == TRBS - 1 {
            let link = self.base + (TRBS - 1) * 16;
            let mut l: Trb = unsafe { read_volatile(link as *const Trb) };
            l.controle = (l.controle & !1) | self.ciclo as u32;
            unsafe { write_volatile(link as *mut Trb, l) };
            limpar(link, 16);
            self.indice = 0;
            self.ciclo = !self.ciclo;
        }
        endereco
    }
}

/// Um endpoint aberto: o indice dele no contexto (DCI).
#[derive(Clone, Copy)]
pub struct Endpoint {
    dci: u8,
    entrada: bool,
    pacote: u16,
}

/// Uma transferencia pedida e ainda nao colhida.
struct Pendente {
    buffer: usize,
    tamanho: usize,
    feito: Option<(u32, usize)>,
}

struct Slot {
    saida: usize,
    entrada: usize,
    aneis: Vec<Option<Anel>>,
    pendentes: Vec<Option<Pendente>>,
    /// Buffer fixo de cada endpoint de interrupcao (64 bytes).
    buffers: Vec<usize>,
    porta: u8,
    velocidade: u8,
}

pub struct Xhci {
    base: usize,
    op: usize,
    portas: u8,
    campainha: usize,
    interruptor: usize,
    dcbaa: usize,
    contexto: usize,
    comandos: Anel,
    eventos: usize,
    evento_indice: usize,
    evento_ciclo: bool,
    slots: Vec<Option<Slot>>,
    completados: Vec<(usize, u32, u8)>,
}

fn ler32(endereco: usize) -> u32 {
    unsafe { read_volatile(endereco as *const u32) }
}

fn escrever32(endereco: usize, v: u32) {
    unsafe { write_volatile(endereco as *mut u32, v) }
}

fn escrever64(endereco: usize, v: u64) {
    escrever32(endereco, v as u32);
    escrever32(endereco + 4, (v >> 32) as u32);
}

fn esperar(mut pronto: impl FnMut() -> bool, ms: u64) -> bool {
    let prazo = cpu::contador() + cpu::frequencia_timer() * ms / 1000;
    while cpu::contador() < prazo {
        if pronto() {
            return true;
        }
        core::hint::spin_loop();
    }
    pronto()
}

impl Xhci {
    pub fn iniciar(base: usize) -> Result<Xhci, &'static str> {
        let tamanho_cap = ler32(base) & 0xFF;
        if tamanho_cap == 0 || tamanho_cap > 0x40 {
            return Err("xHCI não responde");
        }
        let op = base + tamanho_cap as usize;
        let params1 = ler32(base + 0x04);
        let params2 = ler32(base + 0x08);
        let ccparams = ler32(base + 0x10);
        let max_slots = (params1 & 0xFF) as usize;
        let portas = (params1 >> 24) as u8;
        let contexto = if ccparams & 4 != 0 { 64 } else { 32 };
        let campainha = base + (ler32(base + 0x14) & !3) as usize;
        let interruptor = base + (ler32(base + 0x18) & !0x1F) as usize + 0x20;

        // para e reinicia o controlador
        escrever32(op, ler32(op) & !1);
        if !esperar(|| ler32(op + 4) & 1 != 0, 100) {
            return Err("xHCI não para");
        }
        escrever32(op, 1 << 1);
        if !esperar(|| ler32(op) & 2 == 0 && ler32(op + 4) & (1 << 11) == 0, 1000) {
            return Err("xHCI não reinicia");
        }
        escrever32(op + 0x38, max_slots as u32);

        let dcbaa = alocar((max_slots + 1) * 8);
        let rascunhos = (((params2 >> 21) & 0x1F) << 5 | (params2 >> 27) & 0x1F) as usize;
        if rascunhos > 0 {
            let lista = alocar(rascunhos * 8);
            for k in 0..rascunhos {
                unsafe { write_volatile((lista + k * 8) as *mut u64, alocar(4096) as u64) };
            }
            limpar(lista, rascunhos * 8);
            unsafe { write_volatile(dcbaa as *mut u64, lista as u64) };
        }
        limpar(dcbaa, (max_slots + 1) * 8);
        escrever64(op + 0x30, dcbaa as u64);

        let comandos = Anel::novo();
        escrever64(op + 0x18, comandos.base as u64 | 1);

        let eventos = alocar(TRBS * 16);
        let tabela = alocar(64);
        unsafe {
            write_volatile(tabela as *mut u64, eventos as u64);
            write_volatile((tabela + 8) as *mut u32, TRBS as u32);
        }
        limpar(tabela, 64);
        escrever32(interruptor + 0x08, 1);
        escrever64(interruptor + 0x18, eventos as u64);
        escrever64(interruptor + 0x10, tabela as u64);

        escrever32(op, 1); // roda
        if !esperar(|| ler32(op + 4) & 1 == 0, 100) {
            return Err("xHCI não liga");
        }
        let mut slots = Vec::new();
        slots.resize_with(max_slots + 1, || None);
        Ok(Xhci {
            base,
            op,
            portas,
            campainha,
            interruptor,
            dcbaa,
            contexto,
            comandos,
            eventos,
            evento_indice: 0,
            evento_ciclo: true,
            slots,
            completados: Vec::new(),
        })
    }

    pub fn portas(&self) -> u8 {
        self.portas
    }

    fn portsc(&self, porta: u8) -> usize {
        self.op + 0x400 + 0x10 * (porta as usize - 1)
    }

    /// Le os eventos novos e guarda: comandos completados e transferencias.
    fn processar_eventos(&mut self) {
        loop {
            let endereco = self.eventos + self.evento_indice * 16;
            limpar(endereco, 16);
            let trb: Trb = unsafe { read_volatile(endereco as *const Trb) };
            if (trb.controle & 1 == 1) != self.evento_ciclo {
                break;
            }
            let tipo = trb.controle >> 10 & 0x3F;
            let codigo = trb.status >> 24;
            match tipo {
                EVENTO_COMANDO => self.completados.push((trb.parametro as usize, codigo, (trb.controle >> 24) as u8)),
                EVENTO_TRANSFERENCIA => {
                    let slot = (trb.controle >> 24) as usize;
                    let dci = (trb.controle >> 16 & 0x1F) as usize;
                    let resto = (trb.status & 0xFF_FFFF) as usize;
                    if let Some(Some(s)) = self.slots.get_mut(slot) {
                        if let Some(Some(p)) = s.pendentes.get_mut(dci) {
                            p.feito = Some((codigo, p.tamanho.saturating_sub(resto)));
                        }
                    }
                }
                _ => {}
            }
            self.evento_indice += 1;
            if self.evento_indice == TRBS {
                self.evento_indice = 0;
                self.evento_ciclo = !self.evento_ciclo;
            }
            let novo = self.eventos + self.evento_indice * 16;
            // EHB (bit 3) limpo escrevendo 1
            escrever64(self.interruptor + 0x18, novo as u64 | 1 << 3);
        }
    }

    fn comando(&mut self, trb: Trb) -> Result<u8, &'static str> {
        let endereco = self.comandos.por(trb);
        escrever32(self.campainha, 0);
        let prazo = cpu::contador() + cpu::frequencia_timer();
        loop {
            self.processar_eventos();
            if let Some(k) = self.completados.iter().position(|c| c.0 == endereco) {
                let (_, codigo, slot) = self.completados.remove(k);
                return if codigo == 1 { Ok(slot) } else { Err("comando USB falhou") };
            }
            if cpu::contador() > prazo {
                return Err("comando USB sem resposta");
            }
        }
    }

    /// Endereco do contexto `indice` dentro de um contexto de entrada/saida.
    fn ctx(&self, base: usize, indice: usize) -> usize {
        base + indice * self.contexto
    }

    /// Reinicia a porta e da um endereco ao dispositivo dela. Devolve
    /// (slot, velocidade).
    pub fn ligar_porta(&mut self, porta: u8) -> Option<(u8, u8)> {
        let reg = self.portsc(porta);
        let estado = ler32(reg);
        if estado & 1 == 0 {
            return None; // nada plugado
        }
        // reset (sem tocar nos bits que se apagam escrevendo 1)
        let manter = |v: u32| v & !(1 << 1) & !(0x7F << 17);
        escrever32(reg, manter(estado) | 1 << 4);
        if !esperar(|| ler32(reg) & (1 << 21) != 0, 500) {
            return None;
        }
        escrever32(reg, manter(ler32(reg)) | 1 << 21);
        let estado = ler32(reg);
        if estado & 2 == 0 {
            return None;
        }
        let velocidade = (estado >> 10 & 0xF) as u8;
        let slot = self.comando(Trb { controle: HABILITAR_SLOT << 10, ..Default::default() }).ok()?;
        let saida = alocar(32 * self.contexto);
        let entrada = alocar(33 * self.contexto);
        unsafe { write_volatile((self.dcbaa + slot as usize * 8) as *mut u64, saida as u64) };
        limpar(self.dcbaa + slot as usize * 8, 8);
        let mut aneis = Vec::new();
        aneis.resize_with(32, || None);
        let mut pendentes = Vec::new();
        pendentes.resize_with(32, || None);
        aneis[1] = Some(Anel::novo());
        let ep0 = aneis[1].as_ref().unwrap().base;
        self.slots[slot as usize] = Some(Slot { saida, entrada, aneis, pendentes, buffers: vec![0; 32], porta, velocidade });
        // contexto de entrada: slot e endpoint 0
        let pacote: u32 = match velocidade {
            3 => 64,
            4 => 512,
            _ => 8,
        };
        unsafe {
            write_volatile((entrada + 4) as *mut u32, 0b11);
            let s = self.ctx(entrada, 1);
            write_volatile(s as *mut u32, (velocidade as u32) << 20 | 1 << 27);
            write_volatile((s + 4) as *mut u32, (porta as u32) << 16);
            let e = self.ctx(entrada, 2);
            write_volatile((e + 4) as *mut u32, 3 << 1 | 4 << 3 | pacote << 16);
            write_volatile((e + 8) as *mut u64, ep0 as u64 | 1);
            write_volatile((e + 16) as *mut u32, 8);
        }
        limpar(entrada, 33 * self.contexto);
        let trb = Trb { parametro: entrada as u64, status: 0, controle: ENDERECAR << 10 | (slot as u32) << 24 };
        self.comando(trb).ok()?;
        Some((slot, velocidade))
    }

    /// Endpoint 0 com o tamanho de pacote certo (lido no descritor).
    pub fn ajustar_ep0(&mut self, slot: u8, pacote: u16) -> Result<(), &'static str> {
        let entrada = self.slots[slot as usize].as_ref().ok_or("slot")?.entrada;
        unsafe {
            write_volatile(entrada as *mut u32, 0);
            write_volatile((entrada + 4) as *mut u32, 1 << 1);
            let e = self.ctx(entrada, 2);
            let v = read_volatile((e + 4) as *const u32);
            write_volatile((e + 4) as *mut u32, (v & 0xFFFF) | (pacote.max(8) as u32) << 16);
        }
        limpar(entrada, 33 * self.contexto);
        self.comando(Trb { parametro: entrada as u64, status: 0, controle: AVALIAR << 10 | (slot as u32) << 24 }).map(|_| ())
    }

    fn tocar(&self, slot: u8, dci: u8) {
        escrever32(self.campainha + slot as usize * 4, dci as u32);
    }

    /// Espera a transferencia pendente do endpoint (ate 2 s).
    fn esperar_transferencia(&mut self, slot: u8, dci: u8) -> Result<usize, &'static str> {
        let prazo = cpu::contador() + cpu::frequencia_timer() * 2;
        loop {
            self.processar_eventos();
            let s = self.slots[slot as usize].as_mut().ok_or("slot")?;
            if let Some(p) = &s.pendentes[dci as usize] {
                if let Some((codigo, n)) = p.feito {
                    s.pendentes[dci as usize] = None;
                    // 1 = sucesso, 13 = pacote curto (normal)
                    return if codigo == 1 || codigo == 13 { Ok(n) } else { Err("transferência USB falhou") };
                }
            }
            if cpu::contador() > prazo {
                return Err("transferência USB sem resposta");
            }
        }
    }

    /// Pedido de controle no endpoint 0 (setup, dados, status).
    pub fn controle(&mut self, slot: u8, tipo: u8, pedido: u8, valor: u16, indice: u16, dados: &mut [u8]) -> Result<usize, &'static str> {
        let entrando = tipo & 0x80 != 0;
        let mut temporario = alocar_temporario(dados.len());
        if !entrando {
            temporario[..dados.len()].copy_from_slice(dados);
        }
        let buffer = if dados.is_empty() { 0 } else { temporario.as_ptr() as usize };
        if buffer != 0 {
            limpar(buffer, dados.len());
        }
        let s = self.slots[slot as usize].as_mut().ok_or("slot")?;
        let anel = s.aneis[1].as_mut().ok_or("ep0")?;
        let configuracao = tipo as u64 | (pedido as u64) << 8 | (valor as u64) << 16 | (indice as u64) << 32 | (dados.len() as u64) << 48;
        let trt = if dados.is_empty() { 0 } else if entrando { 3 } else { 2 };
        anel.por(Trb { parametro: configuracao, status: 8, controle: SETUP << 10 | IDT | trt << 16 });
        if !dados.is_empty() {
            anel.por(Trb { parametro: buffer as u64, status: dados.len() as u32, controle: DADOS << 10 | (entrando as u32) << 16 });
        }
        let direcao_status = (dados.is_empty() || !entrando) as u32;
        anel.por(Trb { parametro: 0, status: 0, controle: STATUS << 10 | IOC | direcao_status << 16 });
        s.pendentes[1] = Some(Pendente { buffer, tamanho: dados.len(), feito: None });
        self.tocar(slot, 1);
        self.esperar_transferencia(slot, 1)?;
        if entrando && !dados.is_empty() {
            limpar(buffer, dados.len());
            unsafe { core::ptr::copy_nonoverlapping(buffer as *const u8, dados.as_mut_ptr(), dados.len()) };
        }
        Ok(dados.len())
    }

    /// Abre um endpoint bulk (tipo 2) ou de interrupcao (tipo 3).
    pub fn abrir_endpoint(&mut self, slot: u8, velocidade: u8, endereco: u8, tipo: u8, pacote: u16, intervalo: u8) -> Result<Endpoint, &'static str> {
        let entrando = endereco & 0x80 != 0;
        let dci = (endereco & 0xF) * 2 + entrando as u8;
        let contexto = self.contexto;
        let s = self.slots[slot as usize].as_mut().ok_or("slot")?;
        let anel = Anel::novo();
        let base_anel = anel.base;
        s.aneis[dci as usize] = Some(anel);
        if tipo == 3 && s.buffers[dci as usize] == 0 {
            s.buffers[dci as usize] = alocar(64);
        }
        let (entrada, saida) = (s.entrada, s.saida);
        limpar(saida, 32 * contexto);
        let tipo_ep: u32 = match (tipo, entrando) {
            (2, false) => 2,
            (2, true) => 6,
            (_, false) => 3,
            (_, true) => 7,
        };
        // intervalo em 2^n * 125 us: high speed ja vem assim; full/low
        // speed vem em milissegundos (quadros)
        let expoente = if tipo == 3 {
            if velocidade >= 3 { intervalo.saturating_sub(1).min(15) as u32 } else { (intervalo.max(1) as u32 * 8).ilog2().min(15) }
        } else {
            0
        };
        unsafe {
            core::ptr::write_bytes(entrada as *mut u8, 0, 33 * contexto);
            write_volatile((entrada + 4) as *mut u32, 1 | 1 << dci);
            // slot: o mesmo de agora, com mais entradas de contexto
            core::ptr::copy_nonoverlapping(saida as *const u8, (entrada + contexto) as *mut u8, contexto);
            let s0 = read_volatile((entrada + contexto) as *const u32);
            let entradas = ((s0 >> 27) & 0x1F).max(dci as u32);
            write_volatile((entrada + contexto) as *mut u32, (s0 & !(0x1F << 27)) | entradas << 27);
            let e = entrada + (dci as usize + 1) * contexto;
            write_volatile(e as *mut u32, expoente << 16);
            write_volatile((e + 4) as *mut u32, 3 << 1 | tipo_ep << 3 | (pacote as u32) << 16);
            write_volatile((e + 8) as *mut u64, base_anel as u64 | 1);
            let media = if tipo == 3 { pacote as u32 } else { 1024 };
            write_volatile((e + 16) as *mut u32, media | (if tipo == 3 { pacote as u32 } else { 0 }) << 16);
        }
        limpar(entrada, 33 * contexto);
        self.comando(Trb { parametro: entrada as u64, status: 0, controle: CONFIGURAR_EP << 10 | (slot as u32) << 24 })?;
        Ok(Endpoint { dci, entrada: entrando, pacote })
    }

    fn por_normal(&mut self, slot: u8, ep: &Endpoint, buffer: usize, tamanho: usize) -> Result<(), &'static str> {
        let s = self.slots[slot as usize].as_mut().ok_or("slot")?;
        let anel = s.aneis[ep.dci as usize].as_mut().ok_or("endpoint")?;
        anel.por(Trb { parametro: buffer as u64, status: tamanho as u32, controle: NORMAL << 10 | IOC | ISP });
        s.pendentes[ep.dci as usize] = Some(Pendente { buffer, tamanho, feito: None });
        self.tocar(slot, ep.dci);
        Ok(())
    }

    /// Bulk sincrono (pendrive): manda ou recebe `dados` inteiros.
    pub fn bulk(&mut self, slot: u8, ep: &Endpoint, dados: &mut [u8]) -> Result<usize, &'static str> {
        let buffer = alocar_temporario(dados.len());
        if !ep.entrada {
            unsafe { core::ptr::copy_nonoverlapping(dados.as_ptr(), buffer.as_ptr() as *mut u8, dados.len()) };
        }
        limpar(buffer.as_ptr() as usize, dados.len());
        self.por_normal(slot, ep, buffer.as_ptr() as usize, dados.len())?;
        let n = self.esperar_transferencia(slot, ep.dci)?;
        if ep.entrada {
            limpar(buffer.as_ptr() as usize, dados.len());
            let n = n.min(dados.len());
            dados[..n].copy_from_slice(&buffer[..n]);
        }
        Ok(n)
    }

    /// Pede `tamanho` bytes de um endpoint de interrupcao (teclado, mouse);
    /// a resposta sai no `colher`.
    pub fn pedir(&mut self, slot: u8, ep: &Endpoint, tamanho: usize) -> Result<(), &'static str> {
        let buffer = self.slots[slot as usize].as_ref().ok_or("slot")?.buffers[ep.dci as usize];
        if buffer == 0 {
            return Err("endpoint sem buffer");
        }
        limpar(buffer, 64);
        self.por_normal(slot, ep, buffer, tamanho.min(64))
    }

    /// O pedido de interrupcao chegou? Copia e devolve quantos bytes.
    pub fn colher(&mut self, slot: u8, ep: &Endpoint, destino: &mut [u8]) -> Option<usize> {
        self.processar_eventos();
        let s = self.slots[slot as usize].as_mut()?;
        let p = s.pendentes[ep.dci as usize].as_ref()?;
        let (codigo, n) = p.feito?;
        let buffer = p.buffer;
        s.pendentes[ep.dci as usize] = None;
        if codigo != 1 && codigo != 13 {
            return Some(0);
        }
        limpar(buffer, 64);
        let n = n.min(destino.len());
        unsafe { core::ptr::copy_nonoverlapping(buffer as *const u8, destino.as_mut_ptr(), n) };
        Some(n)
    }
}

/// Buffer temporario alinhado para o bulk (volta ao heap no fim).
fn alocar_temporario(tamanho: usize) -> Vec<u8> {
    vec![0u8; tamanho.max(1)]
}
