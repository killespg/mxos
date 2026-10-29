//! UDP (RFC 768): porta origem, porta destino, tamanho, checksum, dados.
//!
//! Uma "caixa postal" simples: quem quer receber abre a porta, e os
//! datagramas que chegam para ela ficam numa fila ate serem tirados.
//! Datagrama para porta fechada e ignorado.

use alloc::collections::VecDeque;
use alloc::vec::Vec;

use super::ipv4::PROTO_UDP;
use super::{Pilha, checksum, escrever16, ler16, pseudo_cabecalho};

const TAMANHO: usize = 8;
/// Datagramas guardados no maximo (o mais antigo sai).
const MAX_FILA: usize = 16;

pub struct Datagrama {
    pub origem: [u8; 4],
    pub porta_origem: u16,
    pub porta: u16,
    pub dados: Vec<u8>,
}

pub struct Caixa {
    abertas: Vec<u16>,
    fila: VecDeque<Datagrama>,
}

impl Caixa {
    pub fn nova() -> Caixa {
        Caixa { abertas: Vec::new(), fila: VecDeque::new() }
    }

    pub fn abrir(&mut self, porta: u16) {
        if !self.abertas.contains(&porta) {
            self.abertas.push(porta);
        }
    }

    /// Fecha a porta e joga fora o que estava esperando nela.
    pub fn fechar(&mut self, porta: u16) {
        self.abertas.retain(|&p| p != porta);
        self.fila.retain(|d| d.porta != porta);
    }

    /// Proximo datagrama que chegou para `porta`.
    pub fn tirar(&mut self, porta: u16) -> Option<Datagrama> {
        let i = self.fila.iter().position(|d| d.porta == porta)?;
        self.fila.remove(i)
    }
}

/// Cabecalho UDP com checksum (sobre pseudo-cabecalho, cabecalho e dados).
fn cabecalho(origem: [u8; 4], destino: [u8; 4], porta_origem: u16, porta_destino: u16, dados: &[u8]) -> [u8; TAMANHO] {
    let mut c = [0u8; TAMANHO];
    escrever16(&mut c, 0, porta_origem);
    escrever16(&mut c, 2, porta_destino);
    escrever16(&mut c, 4, (TAMANHO + dados.len()) as u16);
    let pseudo = pseudo_cabecalho(origem, destino, PROTO_UDP, TAMANHO + dados.len());
    let soma = match checksum(&[&pseudo, &c, dados]) {
        0 => 0xFFFF, // 0 quer dizer "sem checksum"
        s => s,
    };
    escrever16(&mut c, 6, soma);
    c
}

impl Pilha {
    pub(super) fn receber_udp(&mut self, origem: [u8; 4], destino: [u8; 4], dados: &[u8]) {
        if dados.len() < TAMANHO {
            return;
        }
        let tamanho = ler16(dados, 4) as usize;
        if tamanho < TAMANHO || tamanho > dados.len() {
            return;
        }
        let dados = &dados[..tamanho];
        if ler16(dados, 6) != 0 {
            let pseudo = pseudo_cabecalho(origem, destino, PROTO_UDP, tamanho);
            if checksum(&[&pseudo, dados]) != 0 {
                return;
            }
        }
        let porta = ler16(dados, 2);
        if !self.udp.abertas.contains(&porta) {
            return;
        }
        if self.udp.fila.len() == MAX_FILA {
            self.udp.fila.pop_front();
        }
        let porta_origem = ler16(dados, 0);
        self.udp.fila.push_back(Datagrama { origem, porta_origem, porta, dados: dados[TAMANHO..].to_vec() });
    }

    pub(super) fn enviar_udp(&mut self, destino: [u8; 4], porta_origem: u16, porta_destino: u16, dados: &[u8]) {
        let c = cabecalho(self.ip, destino, porta_origem, porta_destino, dados);
        self.enviar_ip(destino, PROTO_UDP, &[&c, dados]);
    }
}
