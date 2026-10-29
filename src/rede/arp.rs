//! ARP (RFC 826): "quem tem o IP x? diga ao IP y". Pacote de 28 bytes:
//!   htype 1 (Ethernet), ptype 0x0800 (IPv4), hlen 6, plen 4, operacao,
//!   MAC e IP de quem manda, MAC e IP de quem se procura.
//!
//! A tabela e pequena e sem validade: a rede do QEMU nunca muda de MAC.

use alloc::vec::Vec;

use super::ethernet::{BROADCAST, TIPO_ARP};
use super::{Pilha, ip4, ler16};

const TAMANHO: usize = 28;
const PEDIDO: u16 = 1;
const RESPOSTA: u16 = 2;
/// Entradas na tabela; a mais antiga sai quando enche.
const MAX_ENTRADAS: usize = 16;

pub struct Tabela {
    entradas: Vec<([u8; 4], [u8; 6])>,
}

impl Tabela {
    pub fn nova() -> Tabela {
        Tabela { entradas: Vec::new() }
    }

    pub fn procurar(&self, ip: [u8; 4]) -> Option<[u8; 6]> {
        self.entradas.iter().find(|(i, _)| *i == ip).map(|&(_, mac)| mac)
    }

    pub fn guardar(&mut self, ip: [u8; 4], mac: [u8; 6]) {
        if let Some(entrada) = self.entradas.iter_mut().find(|(i, _)| *i == ip) {
            entrada.1 = mac;
            return;
        }
        if self.entradas.len() == MAX_ENTRADAS {
            self.entradas.remove(0);
        }
        self.entradas.push((ip, mac));
    }

    pub fn quantas(&self) -> usize {
        self.entradas.len()
    }
}

/// Monta um pacote ARP.
fn montar(operacao: u16, mac: [u8; 6], ip: [u8; 4], mac_alvo: [u8; 6], ip_alvo: [u8; 4]) -> [u8; TAMANHO] {
    let mut p = [0u8; TAMANHO];
    p[0..8].copy_from_slice(&[0, 1, 0x08, 0x00, 6, 4, 0, operacao as u8]);
    p[8..14].copy_from_slice(&mac);
    p[14..18].copy_from_slice(&ip);
    p[18..24].copy_from_slice(&mac_alvo);
    p[24..28].copy_from_slice(&ip_alvo);
    p
}

impl Pilha {
    /// Aprende o MAC de quem mandou e responde se perguntaram pelo nosso IP.
    pub(super) fn receber_arp(&mut self, dados: &[u8]) {
        if dados.len() < TAMANHO || dados[0..6] != [0, 1, 0x08, 0x00, 6, 4] {
            return;
        }
        let operacao = ler16(dados, 6);
        let mut mac = [0u8; 6];
        mac.copy_from_slice(&dados[8..14]);
        let ip = ip4(dados, 14);
        let alvo = ip4(dados, 24);
        let para_nos = self.ip != [0; 4] && alvo == self.ip;
        // aprende de toda resposta e de pedido para nos (o outro vai falar conosco)
        if ip != [0; 4] && (operacao == RESPOSTA || para_nos || self.arp.procurar(ip).is_some()) {
            self.arp.guardar(ip, mac);
            self.liberar_espera_arp(ip, mac);
        }
        if operacao == PEDIDO && para_nos {
            let resposta = montar(RESPOSTA, self.mac, self.ip, mac, ip);
            self.enviar_quadro(mac, TIPO_ARP, &[&resposta]);
        }
    }

    /// Pergunta em broadcast quem tem `ip`.
    pub(super) fn pedir_arp(&mut self, ip: [u8; 4]) {
        let pedido = montar(PEDIDO, self.mac, self.ip, [0; 6], ip);
        self.enviar_quadro(BROADCAST, TIPO_ARP, &[&pedido]);
    }
}
