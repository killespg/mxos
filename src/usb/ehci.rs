//! EHCI (USB 2.0 high speed) da RK322x: so a lista assincrona (controle e
//! bulk; o pendrive), tudo por polling. Dispositivo low/full speed (teclado,
//! mouse) e passado para o OHCI companheiro (PORT_OWNER). Sem hubs.
//!
//! Cada endpoint tem um QH fixo no anel assincrono; uma transferencia poe
//! uma corrente de qTDs no overlay dele e espera o ultimo ficar inativo.
//! Estruturas de 64 bytes alinhadas (uma linha de cache cada) e buffers
//! intermediarios alinhados em 4 KB, limpos da cache antes e depois.
//! NUNCA TESTADO NO HARDWARE.

use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};

use super::{Controlador, Ep, dma, esperar, limpar, pausa_ms};

const T: u32 = 1; // ponteiro terminado
const TIPO_QH: u32 = 1 << 1;
const ATIVO: u32 = 1 << 7;
const PARADO: u32 = 1 << 6;
const ERROS: u32 = 0x7C; // halted + buffer + babble + transacao + quadro perdido
const PID_OUT: u32 = 0;
const PID_IN: u32 = 1;
const PID_SETUP: u32 = 2;
/// Bytes por qTD (5 paginas de 4 KB, comecando alinhado).
const MAX_QTD: usize = 16 * 1024;
const QTDS: usize = 8;

fn ler(e: usize) -> u32 {
    unsafe { read_volatile(e as *const u32) }
}

fn escrever(e: usize, v: u32) {
    unsafe { write_volatile(e as *mut u32, v) }
}

struct Endpoint {
    qh: usize,
    entrada: bool,
    /// Interrupcao: qTD e buffer proprios (pedir/colher).
    td: usize,
    buf: usize,
    tamanho: usize,
    pendente: bool,
}

pub struct Ehci {
    op: usize,
    portas: u8,
    cabeca: usize,
    qtds: [usize; QTDS],
    buf: usize,
    setup: usize,
    /// QH do endpoint 0 de cada endereco.
    ep0: Vec<(u8, usize)>,
    eps: Vec<Endpoint>,
    proximo: u8,
}

impl Ehci {
    pub fn iniciar(base: usize) -> Result<Ehci, &'static str> {
        let caplength = ler(base) & 0xFF;
        if caplength == 0 || caplength == 0xFF {
            return Err("EHCI não responde");
        }
        let hcs = ler(base + 4);
        let op = base + caplength as usize;
        let portas = (hcs & 0xF) as u8;
        // para e reinicia
        escrever(op, ler(op) & !1);
        esperar(|| ler(op + 4) & (1 << 12) != 0, 20);
        escrever(op, 1 << 1);
        if !esperar(|| ler(op) & (1 << 1) == 0, 100) {
            return Err("EHCI não reiniciou");
        }
        escrever(op + 0x08, 0); // sem interrupcoes
        escrever(op + 0x10, 0); // segmento alto (32 bits)
        // cabeca do anel assincrono: QH vazio que aponta para si mesmo
        let cabeca = dma(64, 64);
        escrever(cabeca, cabeca as u32 | TIPO_QH);
        escrever(cabeca + 4, 1 << 15); // H: cabeca da lista
        escrever(cabeca + 16, T);
        escrever(cabeca + 20, T);
        escrever(cabeca + 24, PARADO);
        limpar(cabeca, 64);
        escrever(op + 0x18, cabeca as u32);
        // roda com a lista assincrona; as portas vem para o EHCI
        escrever(op, (8 << 16) | (1 << 5) | 1);
        escrever(op + 0x40, 1);
        pausa_ms(5);
        if hcs & (1 << 4) != 0 {
            for p in 0..portas as usize {
                let r = op + 0x44 + 4 * p;
                escrever(r, (ler(r) & !0x2A) | 1 << 12);
            }
            pausa_ms(30);
        }
        let mut qtds = [0usize; QTDS];
        for q in qtds.iter_mut() {
            *q = dma(64, 64);
        }
        Ok(Ehci { op, portas, cabeca, qtds, buf: dma(MAX_QTD, 4096), setup: dma(64, 64), ep0: Vec::new(), eps: Vec::new(), proximo: 1 })
    }

    fn novo_qh(&mut self, endereco: u8, ep: u8, pacote: u16, controle: bool) -> usize {
        let qh = dma(64, 64);
        let carac = endereco as u32 | (ep as u32 & 0xF) << 8 | 2 << 12 | (controle as u32) << 14 | (pacote as u32 & 0x7FF) << 16;
        escrever(qh + 4, carac);
        escrever(qh + 8, 1 << 30);
        escrever(qh + 16, T);
        escrever(qh + 20, T);
        // entra no anel depois da cabeca
        limpar(self.cabeca, 64);
        escrever(qh, ler(self.cabeca));
        limpar(qh, 64);
        escrever(self.cabeca, qh as u32 | TIPO_QH);
        limpar(self.cabeca, 64);
        qh
    }

    fn qh_do_ep0(&self, endereco: u8) -> Option<usize> {
        self.ep0.iter().find(|(e, _)| *e == endereco).map(|x| x.1)
    }

    /// Preenche um qTD (buffer ja limpo da cache).
    fn qtd(td: usize, proximo: usize, pid: u32, buffer: usize, tamanho: usize, toggle: Option<bool>, ioc: bool) {
        escrever(td, if proximo == 0 { T } else { proximo as u32 });
        escrever(td + 4, T);
        let mut token = (tamanho as u32) << 16 | 3 << 10 | pid << 8 | ATIVO;
        if let Some(t) = toggle {
            token |= (t as u32) << 31;
        }
        if ioc {
            token |= 1 << 15;
        }
        escrever(td + 8, token);
        for k in 0..5 {
            let p = if k == 0 { buffer } else { (buffer & !0xFFF) + k * 4096 };
            escrever(td + 12 + 4 * k, if buffer == 0 { 0 } else { p as u32 });
        }
        limpar(td, 64);
    }

    /// Liga a corrente de qTDs ao QH e espera o ultimo. Devolve os bytes
    /// que sobraram em cada qTD.
    fn rodar(&mut self, qh: usize, n: usize, prazo_ms: u64) -> Result<Vec<usize>, &'static str> {
        limpar(qh, 64);
        let toggle = ler(qh + 24) & (1 << 31);
        escrever(qh + 16, self.qtds[0] as u32);
        escrever(qh + 20, T);
        escrever(qh + 24, toggle);
        limpar(qh, 64);
        let ultimo = self.qtds[n - 1];
        let ok = esperar(
            || {
                (0..n).any(|k| {
                    limpar(self.qtds[k], 64);
                    ler(self.qtds[k] + 8) & PARADO != 0
                }) || {
                    limpar(ultimo, 64);
                    ler(ultimo + 8) & ATIVO == 0
                }
            },
            prazo_ms,
        );
        let mut resto = Vec::with_capacity(n);
        let mut erro = !ok;
        for k in 0..n {
            limpar(self.qtds[k], 64);
            let t = ler(self.qtds[k] + 8);
            if t & ERROS != 0 && t & ATIVO == 0 {
                erro = true;
            }
            resto.push(((t >> 16) & 0x7FFF) as usize);
        }
        if erro {
            // tira o QH do estado parado (o proximo pedido comeca limpo)
            escrever(qh + 16, T);
            escrever(qh + 24, 0);
            limpar(qh, 64);
            return Err(if ok { "transferência USB falhou" } else { "transferência USB sem resposta" });
        }
        Ok(resto)
    }

    fn controle_em(&mut self, qh: usize, tipo: u8, pedido: u8, valor: u16, indice: u16, dados: &mut [u8]) -> Result<usize, &'static str> {
        let entrando = tipo & 0x80 != 0;
        let n = dados.len().min(MAX_QTD);
        let pacote: [u8; 8] = [tipo, pedido, valor as u8, (valor >> 8) as u8, indice as u8, (indice >> 8) as u8, n as u8, (n >> 8) as u8];
        unsafe { core::ptr::copy_nonoverlapping(pacote.as_ptr(), self.setup as *mut u8, 8) };
        limpar(self.setup, 64);
        if !entrando && n > 0 {
            unsafe { core::ptr::copy_nonoverlapping(dados.as_ptr(), self.buf as *mut u8, n) };
        }
        limpar(self.buf, n.max(1));
        let (q0, q1, q2) = (self.qtds[0], self.qtds[1], self.qtds[2]);
        let mut k = 1;
        if n > 0 {
            Self::qtd(q0, q1, PID_SETUP, self.setup, 8, Some(false), false);
            Self::qtd(q1, q2, if entrando { PID_IN } else { PID_OUT }, self.buf, n, Some(true), false);
            Self::qtd(q2, 0, if entrando { PID_OUT } else { PID_IN }, 0, 0, Some(true), true);
            k = 3;
        } else {
            Self::qtd(q0, q1, PID_SETUP, self.setup, 8, Some(false), false);
            Self::qtd(q1, 0, PID_IN, 0, 0, Some(true), true);
            k += 1;
        }
        let resto = self.rodar(qh, k, 1000)?;
        let feito = if n > 0 { n - resto[1].min(n) } else { 0 };
        if entrando && feito > 0 {
            limpar(self.buf, feito);
            unsafe { core::ptr::copy_nonoverlapping(self.buf as *const u8, dados.as_mut_ptr(), feito) };
        }
        Ok(feito)
    }
}

impl Controlador for Ehci {
    fn portas(&self) -> u8 {
        self.portas
    }

    fn ligar_porta(&mut self, porta: u8) -> Option<(u8, u8)> {
        let r = self.op + 0x44 + 4 * (porta as usize - 1);
        let v = ler(r);
        if v & 1 == 0 {
            return None;
        }
        // linha em K = low speed: e do OHCI
        if (v >> 10) & 3 == 1 {
            escrever(r, (v & !0x2A) | 1 << 13);
            return None;
        }
        escrever(r, (v & !(0x2A | 1 << 2)) | 1 << 8);
        pausa_ms(50);
        escrever(r, ler(r) & !(0x2A | 1 << 8));
        esperar(|| ler(r) & (1 << 8) == 0, 10);
        pausa_ms(5);
        if ler(r) & (1 << 2) == 0 {
            // nao ficou habilitada: full speed, tambem do OHCI
            escrever(r, (ler(r) & !0x2A) | 1 << 13);
            return None;
        }
        // endereco 0 -> proximo endereco livre
        let qh0 = self.novo_qh(0, 0, 64, true);
        let mut d = [0u8; 8];
        self.controle_em(qh0, 0x80, 6, 0x0100, 0, &mut d).ok()?;
        let endereco = self.proximo;
        self.proximo += 1;
        self.controle_em(qh0, 0x00, 5, endereco as u16, 0, &mut []).ok()?;
        pausa_ms(3);
        // o mesmo QH passa a ser do endereco novo
        limpar(qh0, 64);
        escrever(qh0 + 4, (ler(qh0 + 4) & !0x7F & !(0x7FF << 16)) | endereco as u32 | (d[7].max(8) as u32) << 16);
        limpar(qh0, 64);
        self.ep0.push((endereco, qh0));
        Some((endereco, 3))
    }

    fn ajustar_ep0(&mut self, disp: u8, pacote: u16) -> Result<(), &'static str> {
        let qh = self.qh_do_ep0(disp).ok_or("dispositivo")?;
        limpar(qh, 64);
        escrever(qh + 4, (ler(qh + 4) & !(0x7FF << 16)) | (pacote.max(8) as u32 & 0x7FF) << 16);
        limpar(qh, 64);
        Ok(())
    }

    fn controle(&mut self, disp: u8, tipo: u8, pedido: u8, valor: u16, indice: u16, dados: &mut [u8]) -> Result<usize, &'static str> {
        let qh = self.qh_do_ep0(disp).ok_or("dispositivo")?;
        self.controle_em(qh, tipo, pedido, valor, indice, dados)
    }

    fn abrir_endpoint(&mut self, disp: u8, _velocidade: u8, endereco: u8, tipo: u8, pacote: u16, _intervalo: u8) -> Result<Ep, &'static str> {
        let qh = self.novo_qh(disp, endereco & 0xF, pacote, false);
        let (td, buf) = if tipo == 3 { (dma(64, 64), dma(64, 64)) } else { (0, 0) };
        self.eps.push(Endpoint { qh, entrada: endereco & 0x80 != 0, td, buf, tamanho: 0, pendente: false });
        Ok(Ep { id: self.eps.len() as u32 - 1, entrada: endereco & 0x80 != 0 })
    }

    fn bulk(&mut self, _disp: u8, ep: &Ep, dados: &mut [u8]) -> Result<usize, &'static str> {
        let (qh, entrada) = {
            let e = self.eps.get(ep.id as usize).ok_or("endpoint")?;
            (e.qh, e.entrada)
        };
        let mut feito = 0;
        while feito < dados.len() || dados.is_empty() {
            let n = (dados.len() - feito).min(MAX_QTD);
            if !entrada {
                unsafe { core::ptr::copy_nonoverlapping(dados[feito..].as_ptr(), self.buf as *mut u8, n) };
            }
            limpar(self.buf, n.max(1));
            Self::qtd(self.qtds[0], 0, if entrada { PID_IN } else { PID_OUT }, self.buf, n, None, true);
            let resto = self.rodar(qh, 1, 5000)?;
            let veio = n - resto[0].min(n);
            if entrada {
                limpar(self.buf, veio.max(1));
                unsafe { core::ptr::copy_nonoverlapping(self.buf as *const u8, dados[feito..].as_mut_ptr(), veio) };
            }
            feito += veio;
            if dados.is_empty() || veio < n {
                break; // pacote curto: acabou
            }
        }
        Ok(feito)
    }

    fn pedir(&mut self, _disp: u8, ep: &Ep, tamanho: usize) -> Result<(), &'static str> {
        let e = self.eps.get_mut(ep.id as usize).ok_or("endpoint")?;
        if e.td == 0 {
            return Err("endpoint sem buffer");
        }
        let n = tamanho.min(64);
        limpar(e.buf, 64);
        Self::qtd(e.td, 0, PID_IN, e.buf, n, None, true);
        let qh = e.qh;
        limpar(qh, 64);
        let toggle = ler(qh + 24) & (1 << 31);
        escrever(qh + 16, e.td as u32);
        escrever(qh + 24, toggle);
        limpar(qh, 64);
        e.tamanho = n;
        e.pendente = true;
        Ok(())
    }

    fn colher(&mut self, _disp: u8, ep: &Ep, destino: &mut [u8]) -> Option<usize> {
        let e = self.eps.get_mut(ep.id as usize)?;
        if !e.pendente {
            return None;
        }
        limpar(e.td, 64);
        let t = ler(e.td + 8);
        if t & ATIVO != 0 {
            return None;
        }
        e.pendente = false;
        if t & ERROS != 0 {
            return Some(0);
        }
        let n = (e.tamanho - ((t >> 16) & 0x7FFF) as usize).min(destino.len());
        limpar(e.buf, 64);
        unsafe { core::ptr::copy_nonoverlapping(e.buf as *const u8, destino.as_mut_ptr(), n) };
        Some(n)
    }
}
