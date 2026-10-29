//! Ethernet II: 6 bytes de MAC destino, 6 de MAC origem e 2 de tipo.
//! O FCS (CRC do fim do quadro) e posto e tirado pelo dispositivo.

use super::{Pilha, ler16};

pub const TAMANHO: usize = 14;
pub const BROADCAST: [u8; 6] = [0xFF; 6];

pub const TIPO_IPV4: u16 = 0x0800;
pub const TIPO_ARP: u16 = 0x0806;

/// Monta o cabecalho Ethernet.
pub fn cabecalho(destino: [u8; 6], origem: [u8; 6], tipo: u16) -> [u8; TAMANHO] {
    let mut c = [0u8; TAMANHO];
    c[0..6].copy_from_slice(&destino);
    c[6..12].copy_from_slice(&origem);
    c[12..14].copy_from_slice(&tipo.to_be_bytes());
    c
}

impl Pilha {
    /// Um quadro que chegou: confere o destino e passa adiante pelo tipo.
    pub(super) fn receber_quadro(&mut self, quadro: &[u8]) {
        if quadro.len() < TAMANHO {
            return;
        }
        let destino = &quadro[0..6];
        if destino != self.mac && destino != BROADCAST {
            return;
        }
        let dados = &quadro[TAMANHO..];
        match ler16(quadro, 12) {
            TIPO_ARP => self.receber_arp(dados),
            TIPO_IPV4 => self.receber_ipv4(dados),
            _ => {}
        }
    }

    /// Manda `partes` (coladas) num quadro para `destino`.
    pub(super) fn enviar_quadro(&mut self, destino: [u8; 6], tipo: u16, partes: &[&[u8]]) {
        let cabecalho = cabecalho(destino, self.mac, tipo);
        let mut todas: [&[u8]; 4] = [&cabecalho, &[], &[], &[]];
        for (i, parte) in partes.iter().take(3).enumerate() {
            todas[i + 1] = parte;
        }
        if self.nic.enviar(&todas) {
            self.pacotes_tx += 1;
        }
    }
}
