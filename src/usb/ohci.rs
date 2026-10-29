//! OHCI (USB 1.1, o companheiro do EHCI na RK322x): teclado, mouse e o que
//! for low/full speed. Listas de controle, bulk e periodica (interrupcao),
//! cada uma com um ED vazio fixo na cabeca; cada ED tem um TD vazio no fim
//! (o TailP), e uma transferencia preenche esse TD e poe outro vazio
//! depois. Tudo por polling; estruturas alinhadas em 64 bytes e limpas da
//! cache. NUNCA TESTADO NO HARDWARE.

use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};

use super::{Controlador, Ep, dma, esperar, limpar, liberar_dma, pausa_ms};

// registradores
const CONTROLE: usize = 0x04;
const COMANDO: usize = 0x08;
const INT_STATUS: usize = 0x0C;
const INT_DESLIGA: usize = 0x14;
const HCCA: usize = 0x18;
const CABECA_CONTROLE: usize = 0x20;
const CABECA_BULK: usize = 0x28;
const FM_INTERVALO: usize = 0x34;
const INICIO_PERIODICO: usize = 0x40;
const RH_A: usize = 0x48;
const RH_STATUS: usize = 0x50;
const RH_PORTA: usize = 0x54;

const CLF: u32 = 1 << 1;
const BLF: u32 = 1 << 2;
/// Codigo de condicao "nao acessado" no TD novo.
const NAO_ACESSADO: u32 = 0xF << 28;
const DP_SETUP: u32 = 0;
const DP_OUT: u32 = 1;
const DP_IN: u32 = 2;
const MAX_TD: usize = 4096;

fn ler(e: usize) -> u32 {
    unsafe { read_volatile(e as *const u32) }
}

fn escrever(e: usize, v: u32) {
    unsafe { write_volatile(e as *mut u32, v) }
}

fn novo_td() -> usize {
    dma(64, 64)
}

struct Endpoint {
    ed: usize,
    entrada: bool,
    /// Interrupcao: o TD em andamento e o buffer.
    td: usize,
    buf: usize,
    tamanho: usize,
    pendente: bool,
}

pub struct Ohci {
    base: usize,
    portas: u8,
    controle: usize,
    bulk: usize,
    periodico: usize,
    buf: usize,
    setup: usize,
    ep0: Vec<(u8, usize)>,
    eps: Vec<Endpoint>,
    proximo: u8,
}

impl Ohci {
    pub fn iniciar(base: usize, primeiro_endereco: u8) -> Result<Ohci, &'static str> {
        let rev = ler(base) & 0xFF;
        if rev != 0x10 && rev != 0x11 {
            return Err("OHCI não responde");
        }
        let fi = ler(base + FM_INTERVALO) & 0x3FFF;
        let fi = if fi == 0 { 0x2EDF } else { fi };
        escrever(base + COMANDO, 1);
        if !esperar(|| ler(base + COMANDO) & 1 == 0, 10) {
            return Err("OHCI não reiniciou");
        }
        let fs_max = ((fi - 210) * 6) / 7;
        escrever(base + FM_INTERVALO, fi | fs_max << 16 | 1 << 31);
        escrever(base + INICIO_PERIODICO, fi * 9 / 10);
        escrever(base + INT_DESLIGA, 0xC000_007F);
        escrever(base + INT_STATUS, 0x7F);
        // EDs vazios (pulados) na cabeca de cada lista
        let vazio = |tipo_periodico: bool| {
            let ed = dma(64, 64);
            escrever(ed, 1 << 14); // K: pular
            let _ = tipo_periodico;
            limpar(ed, 64);
            ed
        };
        let (controle, bulk, periodico) = (vazio(false), vazio(false), vazio(true));
        let hcca = dma(256, 256);
        for k in 0..32 {
            escrever(hcca + 4 * k, periodico as u32);
        }
        limpar(hcca, 256);
        escrever(base + HCCA, hcca as u32);
        escrever(base + CABECA_CONTROLE, controle as u32);
        escrever(base + CABECA_BULK, bulk as u32);
        // operacional, com as tres listas
        escrever(base + CONTROLE, 3 | 1 << 2 | 1 << 4 | 1 << 5 | 2 << 6);
        // energia nas portas
        let a = ler(base + RH_A);
        escrever(base + RH_STATUS, 1 << 16);
        let portas = (a & 0xFF) as u8;
        for p in 0..portas as usize {
            escrever(base + RH_PORTA + 4 * p, 1 << 8);
        }
        pausa_ms(((a >> 24) * 2).max(20) as u64 + 100);
        Ok(Ohci {
            base,
            portas,
            controle,
            bulk,
            periodico,
            buf: dma(MAX_TD, 4096),
            setup: dma(64, 64),
            ep0: Vec::new(),
            eps: Vec::new(),
            proximo: primeiro_endereco,
        })
    }

    /// ED novo depois da cabeca de uma lista (com um TD vazio no fim).
    fn novo_ed(&mut self, lista: usize, endereco: u8, ep: u8, pacote: u16, lenta: bool) -> usize {
        let ed = dma(64, 64);
        let td = novo_td();
        escrever(ed, endereco as u32 | (ep as u32 & 0xF) << 7 | (lenta as u32) << 13 | (pacote as u32 & 0x7FF) << 16);
        escrever(ed + 4, td as u32);
        escrever(ed + 8, td as u32);
        limpar(lista, 64);
        escrever(ed + 12, ler(lista + 12));
        limpar(ed, 64);
        escrever(lista + 12, ed as u32);
        limpar(lista, 64);
        ed
    }

    /// Preenche o TD do fim do ED e poe um vazio depois. Devolve o TD.
    fn por_td(ed: usize, dp: u32, toggle: Option<bool>, buffer: usize, tamanho: usize) -> usize {
        limpar(ed, 64);
        let td = ler(ed + 4) as usize;
        let novo = novo_td();
        let t = match toggle {
            None => 0,
            Some(b) => 2 | b as u32,
        };
        escrever(td, NAO_ACESSADO | t << 24 | 7 << 21 | dp << 19 | 1 << 18);
        escrever(td + 4, if tamanho == 0 { 0 } else { buffer as u32 });
        escrever(td + 8, novo as u32);
        escrever(td + 12, if tamanho == 0 { 0 } else { (buffer + tamanho - 1) as u32 });
        limpar(td, 64);
        escrever(ed + 4, novo as u32);
        limpar(ed, 64);
        td
    }

    /// Espera o ED esvaziar (ou parar com erro).
    fn esperar_ed(&self, ed: usize, prazo_ms: u64) -> Result<(), &'static str> {
        let ok = esperar(
            || {
                limpar(ed, 64);
                let cabeca = ler(ed + 8);
                cabeca & 1 != 0 || cabeca & !0xF == ler(ed + 4)
            },
            prazo_ms,
        );
        limpar(ed, 64);
        let cabeca = ler(ed + 8);
        if !ok || cabeca & 1 != 0 {
            // tira o halt (guardando o toggle) e descarta o que sobrou
            escrever(ed + 8, ler(ed + 4) | (cabeca & 2));
            limpar(ed, 64);
            return Err(if ok { "transferência USB falhou" } else { "transferência USB sem resposta" });
        }
        Ok(())
    }

    /// Bytes que o TD terminado moveu.
    fn movidos(td: usize, buffer: usize, tamanho: usize) -> usize {
        limpar(td, 64);
        let cbp = ler(td + 4) as usize;
        if cbp == 0 { tamanho } else { cbp - buffer }
    }

    fn controle_em(&mut self, ed: usize, tipo: u8, pedido: u8, valor: u16, indice: u16, dados: &mut [u8]) -> Result<usize, &'static str> {
        let entrando = tipo & 0x80 != 0;
        let n = dados.len().min(MAX_TD);
        let pacote: [u8; 8] = [tipo, pedido, valor as u8, (valor >> 8) as u8, indice as u8, (indice >> 8) as u8, n as u8, (n >> 8) as u8];
        unsafe { core::ptr::copy_nonoverlapping(pacote.as_ptr(), self.setup as *mut u8, 8) };
        limpar(self.setup, 64);
        if !entrando && n > 0 {
            unsafe { core::ptr::copy_nonoverlapping(dados.as_ptr(), self.buf as *mut u8, n) };
        }
        limpar(self.buf, n.max(1));
        let td_setup = Self::por_td(ed, DP_SETUP, Some(false), self.setup, 8);
        let td_dados = if n > 0 { Some(Self::por_td(ed, if entrando { DP_IN } else { DP_OUT }, Some(true), self.buf, n)) } else { None };
        let td_status = Self::por_td(ed, if entrando && n > 0 { DP_OUT } else { DP_IN }, Some(true), 0, 0);
        escrever(self.base + COMANDO, CLF);
        let r = self.esperar_ed(ed, 1000);
        let feito = if r.is_ok() { td_dados.map_or(0, |td| Self::movidos(td, self.buf, n)) } else { 0 };
        // os TDs terminados voltam ao heap (o controlador ja passou deles)
        for td in [Some(td_setup), td_dados, Some(td_status)].into_iter().flatten() {
            liberar_dma(td, 64, 64);
        }
        r?;
        if entrando && feito > 0 {
            limpar(self.buf, feito);
            unsafe { core::ptr::copy_nonoverlapping(self.buf as *const u8, dados.as_mut_ptr(), feito) };
        }
        Ok(feito)
    }

    fn ed_do_ep0(&self, disp: u8) -> Option<usize> {
        self.ep0.iter().find(|(d, _)| *d == disp).map(|x| x.1)
    }
}

impl Controlador for Ohci {
    fn portas(&self) -> u8 {
        self.portas
    }

    fn ligar_porta(&mut self, porta: u8) -> Option<(u8, u8)> {
        let r = self.base + RH_PORTA + 4 * (porta as usize - 1);
        if ler(r) & 1 == 0 {
            return None;
        }
        escrever(r, 1 << 4); // reset
        esperar(|| ler(r) & (1 << 20) != 0, 100);
        escrever(r, 1 << 20 | 1 << 16);
        pausa_ms(20);
        let v = ler(r);
        if v & 2 == 0 {
            return None;
        }
        let lenta = v & (1 << 9) != 0;
        let controle = self.controle;
        let ed = self.novo_ed(controle, 0, 0, 8, lenta);
        let mut d = [0u8; 8];
        self.controle_em(ed, 0x80, 6, 0x0100, 0, &mut d).ok()?;
        let endereco = self.proximo;
        self.proximo += 1;
        self.controle_em(ed, 0x00, 5, endereco as u16, 0, &mut []).ok()?;
        pausa_ms(3);
        limpar(ed, 64);
        escrever(ed, (ler(ed) & !0x7F & !(0x7FF << 16)) | endereco as u32 | (d[7].max(8) as u32) << 16);
        limpar(ed, 64);
        self.ep0.push((endereco, ed));
        Some((endereco, if lenta { 2 } else { 1 }))
    }

    fn ajustar_ep0(&mut self, disp: u8, pacote: u16) -> Result<(), &'static str> {
        let ed = self.ed_do_ep0(disp).ok_or("dispositivo")?;
        limpar(ed, 64);
        escrever(ed, (ler(ed) & !(0x7FF << 16)) | (pacote.max(8) as u32 & 0x7FF) << 16);
        limpar(ed, 64);
        Ok(())
    }

    fn controle(&mut self, disp: u8, tipo: u8, pedido: u8, valor: u16, indice: u16, dados: &mut [u8]) -> Result<usize, &'static str> {
        let ed = self.ed_do_ep0(disp).ok_or("dispositivo")?;
        self.controle_em(ed, tipo, pedido, valor, indice, dados)
    }

    fn abrir_endpoint(&mut self, disp: u8, velocidade: u8, endereco: u8, tipo: u8, pacote: u16, _intervalo: u8) -> Result<Ep, &'static str> {
        let lista = if tipo == 3 { self.periodico } else { self.bulk };
        let ed = self.novo_ed(lista, disp, endereco & 0xF, pacote, velocidade == 2);
        let buf = if tipo == 3 { dma(64, 64) } else { 0 };
        let entrada = endereco & 0x80 != 0;
        self.eps.push(Endpoint { ed, entrada, td: 0, buf, tamanho: 0, pendente: false });
        Ok(Ep { id: self.eps.len() as u32 - 1, entrada })
    }

    fn bulk(&mut self, _disp: u8, ep: &Ep, dados: &mut [u8]) -> Result<usize, &'static str> {
        let (ed, entrada) = {
            let e = self.eps.get(ep.id as usize).ok_or("endpoint")?;
            (e.ed, e.entrada)
        };
        let mut feito = 0;
        loop {
            let n = (dados.len() - feito).min(MAX_TD);
            if !entrada {
                unsafe { core::ptr::copy_nonoverlapping(dados[feito..].as_ptr(), self.buf as *mut u8, n) };
            }
            limpar(self.buf, n.max(1));
            let td = Self::por_td(ed, if entrada { DP_IN } else { DP_OUT }, None, self.buf, n);
            escrever(self.base + COMANDO, BLF);
            let r = self.esperar_ed(ed, 5000);
            let veio = if r.is_ok() { Self::movidos(td, self.buf, n).min(n) } else { 0 };
            liberar_dma(td, 64, 64);
            r?;
            if entrada && veio > 0 {
                limpar(self.buf, veio);
                unsafe { core::ptr::copy_nonoverlapping(self.buf as *const u8, dados[feito..].as_mut_ptr(), veio) };
            }
            feito += veio;
            if feito >= dados.len() || veio < n {
                break;
            }
        }
        Ok(feito)
    }

    fn pedir(&mut self, _disp: u8, ep: &Ep, tamanho: usize) -> Result<(), &'static str> {
        let e = self.eps.get_mut(ep.id as usize).ok_or("endpoint")?;
        if e.buf == 0 {
            return Err("endpoint sem buffer");
        }
        let n = tamanho.min(64);
        limpar(e.buf, 64);
        e.td = Self::por_td(e.ed, DP_IN, None, e.buf, n);
        e.tamanho = n;
        e.pendente = true;
        Ok(())
    }

    fn colher(&mut self, _disp: u8, ep: &Ep, destino: &mut [u8]) -> Option<usize> {
        let e = self.eps.get_mut(ep.id as usize)?;
        if !e.pendente {
            return None;
        }
        limpar(e.ed, 64);
        let cabeca = ler(e.ed + 8);
        if cabeca & 1 != 0 {
            // NAK nao para o ED; parou = erro (ex.: desconectou)
            escrever(e.ed + 8, ler(e.ed + 4) | (cabeca & 2));
            limpar(e.ed, 64);
            e.pendente = false;
            liberar_dma(e.td, 64, 64);
            e.td = 0;
            return Some(0);
        }
        if cabeca & !0xF != ler(e.ed + 4) {
            return None;
        }
        e.pendente = false;
        let n = Self::movidos(e.td, e.buf, e.tamanho).min(destino.len());
        liberar_dma(e.td, 64, 64);
        e.td = 0;
        limpar(e.buf, 64);
        unsafe { core::ptr::copy_nonoverlapping(e.buf as *const u8, destino.as_mut_ptr(), n) };
        Some(n)
    }
}
