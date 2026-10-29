//! IPv4 (RFC 791): cabecalho de 20 bytes sem opcoes na saida.
//!   versao/ihl, tos, tamanho total, id, flags/fragmento, ttl, protocolo,
//!   checksum do cabecalho, IP origem, IP destino
//!
//! Fragmentos sao descartados (com MSS 1460 o TCP nunca precisa deles).
//! Rota: destino na mesma sub-rede vai direto, o resto vai pelo gateway.
//! Se o MAC do proximo salto nao esta na tabela ARP, o pacote espera ate
//! 1 s pela resposta do ARP.

use alloc::vec::Vec;

use super::ethernet::{BROADCAST, TIPO_IPV4};
use super::{Pilha, agora_ms, checksum, escrever16, ip4, ler16};

pub const TAMANHO: usize = 20;
pub const PROTO_ICMP: u8 = 1;
pub const PROTO_TCP: u8 = 6;
pub const PROTO_UDP: u8 = 17;

const TTL: u8 = 64;
/// Bit "nao fragmente".
const NAO_FRAGMENTAR: u16 = 0x4000;
/// Pacotes esperando ARP, no maximo, e por quanto tempo.
const MAX_ESPERA: usize = 8;
const ESPERA_ARP_MS: u64 = 1000;

pub const BROADCAST_IP: [u8; 4] = [255; 4];

/// Um pacote IP pronto que so falta o MAC do proximo salto.
pub struct Pendente {
    proximo: [u8; 4],
    pacote: Vec<u8>,
    desde_ms: u64,
}

/// Cabecalho que chegou.
pub struct Recebido<'a> {
    pub origem: [u8; 4],
    pub destino: [u8; 4],
    pub protocolo: u8,
    pub dados: &'a [u8],
}

/// Confere e separa um pacote IPv4. None = invalido, fragmento ou com
/// checksum errado.
pub fn ler(pacote: &[u8]) -> Option<Recebido<'_>> {
    if pacote.len() < TAMANHO || pacote[0] >> 4 != 4 {
        return None;
    }
    let ihl = (pacote[0] & 0xF) as usize * 4;
    let total = ler16(pacote, 2) as usize;
    if ihl < TAMANHO || total < ihl || total > pacote.len() {
        return None;
    }
    if checksum(&[&pacote[..ihl]]) != 0 {
        return None;
    }
    let fragmento = ler16(pacote, 6);
    if fragmento & 0x3FFF != 0 {
        return None; // MF ligado ou deslocamento diferente de zero
    }
    Some(Recebido { origem: ip4(pacote, 12), destino: ip4(pacote, 16), protocolo: pacote[9], dados: &pacote[ihl..total] })
}

/// Monta o cabecalho para `tamanho_dados` bytes de dados.
pub fn cabecalho(origem: [u8; 4], destino: [u8; 4], protocolo: u8, id: u16, tamanho_dados: usize) -> [u8; TAMANHO] {
    let mut c = [0u8; TAMANHO];
    c[0] = 0x45;
    escrever16(&mut c, 2, (TAMANHO + tamanho_dados) as u16);
    escrever16(&mut c, 4, id);
    escrever16(&mut c, 6, NAO_FRAGMENTAR);
    c[8] = TTL;
    c[9] = protocolo;
    c[12..16].copy_from_slice(&origem);
    c[16..20].copy_from_slice(&destino);
    let soma = checksum(&[&c]);
    escrever16(&mut c, 10, soma);
    c
}

fn mesma_rede(a: [u8; 4], b: [u8; 4], mascara: [u8; 4]) -> bool {
    (0..4).all(|i| a[i] & mascara[i] == b[i] & mascara[i])
}

impl Pilha {
    /// Broadcast da sub-rede (ex.: 10.0.2.255).
    fn broadcast_local(&self) -> [u8; 4] {
        core::array::from_fn(|i| self.ip[i] | !self.mascara[i])
    }

    /// E para nos? Sem IP ainda (no meio do DHCP) aceitamos tudo.
    fn e_para_nos(&self, destino: [u8; 4]) -> bool {
        self.ip == [0; 4] || destino == self.ip || destino == BROADCAST_IP || destino == self.broadcast_local()
    }

    pub(super) fn receber_ipv4(&mut self, pacote: &[u8]) {
        let Some(p) = ler(pacote) else { return };
        if !self.e_para_nos(p.destino) {
            return;
        }
        match p.protocolo {
            PROTO_ICMP => self.receber_icmp(p.origem, p.dados),
            PROTO_UDP => self.receber_udp(p.origem, p.destino, p.dados),
            PROTO_TCP => self.receber_tcp(p.origem, p.destino, p.dados),
            _ => {}
        }
    }

    /// Proximo salto para `destino`.
    fn rota(&self, destino: [u8; 4]) -> [u8; 4] {
        if self.gateway == [0; 4] || mesma_rede(destino, self.ip, self.mascara) {
            destino
        } else {
            self.gateway
        }
    }

    /// Manda um pacote IP com os dados feitos das `partes` (no maximo 2).
    pub(super) fn enviar_ip(&mut self, destino: [u8; 4], protocolo: u8, partes: &[&[u8]]) {
        let tamanho: usize = partes.iter().map(|p| p.len()).sum();
        self.id_ip = self.id_ip.wrapping_add(1);
        let cabecalho = cabecalho(self.ip, destino, protocolo, self.id_ip, tamanho);
        let mut todas: [&[u8]; 3] = [&cabecalho, &[], &[]];
        for (i, parte) in partes.iter().take(2).enumerate() {
            todas[i + 1] = parte;
        }
        if destino == BROADCAST_IP || destino == self.broadcast_local() {
            self.enviar_quadro(BROADCAST, TIPO_IPV4, &todas);
            return;
        }
        let proximo = self.rota(destino);
        match self.arp.procurar(proximo) {
            Some(mac) => self.enviar_quadro(mac, TIPO_IPV4, &todas),
            None => self.esperar_arp(proximo, todas.concat()),
        }
    }

    /// Guarda o pacote e pergunta o MAC do proximo salto.
    fn esperar_arp(&mut self, proximo: [u8; 4], pacote: Vec<u8>) {
        if self.espera_arp.len() == MAX_ESPERA {
            self.espera_arp.remove(0);
        }
        self.espera_arp.push(Pendente { proximo, pacote, desde_ms: agora_ms() });
        self.pedir_arp(proximo);
    }

    /// O ARP respondeu: manda o que esperava por esse IP.
    pub(super) fn liberar_espera_arp(&mut self, ip: [u8; 4], mac: [u8; 6]) {
        let mut i = 0;
        while i < self.espera_arp.len() {
            if self.espera_arp[i].proximo == ip {
                let pendente = self.espera_arp.remove(i);
                self.enviar_quadro(mac, TIPO_IPV4, &[&pendente.pacote]);
            } else {
                i += 1;
            }
        }
    }

    /// Joga fora o que esperou demais (quem precisa retransmite).
    pub(super) fn expirar_espera_arp(&mut self) {
        if self.espera_arp.is_empty() {
            return;
        }
        let agora = agora_ms();
        self.espera_arp.retain(|p| agora.saturating_sub(p.desde_ms) < ESPERA_ARP_MS);
    }
}
