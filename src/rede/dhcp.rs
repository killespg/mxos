//! DHCP (RFC 2131): DISCOVER -> OFFER -> REQUEST -> ACK, tudo em broadcast
//! (ainda nao temos IP). Mensagem BOOTP de 236 bytes + "magic cookie" +
//! opcoes no formato {codigo, tamanho, valor}.
//!
//! Sem renovacao: o QEMU empresta o IP por um dia e o MXOS nao fica ligado
//! tanto tempo assim.

use alloc::vec::Vec;

use super::ipv4::BROADCAST_IP;
use super::{ErroRede, Pilha, agora_ms, com, esperar, ip4, ler32};
use crate::cpu;

const PORTA_SERVIDOR: u16 = 67;
const PORTA_CLIENTE: u16 = 68;

// Tipos de mensagem (opcao 53)
const DISCOVER: u8 = 1;
const OFFER: u8 = 2;
const REQUEST: u8 = 3;
const ACK: u8 = 5;
const NAK: u8 = 6;

// Opcoes
const OP_MASCARA: u8 = 1;
const OP_GATEWAY: u8 = 3;
const OP_DNS: u8 = 6;
const OP_NOME: u8 = 12;
const OP_IP_PEDIDO: u8 = 50;
const OP_TIPO: u8 = 53;
const OP_SERVIDOR: u8 = 54;
const OP_PARAMETROS: u8 = 55;
const OP_FIM: u8 = 255;
const OP_PREENCHIMENTO: u8 = 0;

const COOKIE: u32 = 0x6382_5363;
const TAMANHO_BOOTP: usize = 236;
/// Tamanho minimo de uma mensagem BOOTP.
const TAMANHO_MINIMO: usize = 300;

/// Tempo total e intervalo de reenvio.
const TEMPO_TOTAL_MS: u64 = 3000;
const REENVIO_MS: u64 = 700;

/// O que interessa numa resposta do servidor.
#[derive(Clone, Copy)]
struct Mensagem {
    tipo: u8,
    ip: [u8; 4],
    servidor: Option<[u8; 4]>,
    mascara: Option<[u8; 4]>,
    gateway: Option<[u8; 4]>,
    dns: Option<[u8; 4]>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fase {
    Descobrindo,
    /// Pedindo (ip oferecido, servidor).
    Pedindo([u8; 4], [u8; 4]),
}

/// Monta DISCOVER (pedido = None) ou REQUEST (pedido = (ip, servidor)).
fn montar(tipo: u8, xid: u32, mac: [u8; 6], pedido: Option<([u8; 4], [u8; 4])>) -> Vec<u8> {
    let mut m = alloc::vec![0u8; TAMANHO_BOOTP];
    m[0..4].copy_from_slice(&[1, 1, 6, 0]); // op BOOTREQUEST, Ethernet, hlen 6, hops
    m[4..8].copy_from_slice(&xid.to_be_bytes());
    m[10] = 0x80; // flags: responda em broadcast
    m[28..34].copy_from_slice(&mac);
    m.extend_from_slice(&COOKIE.to_be_bytes());
    m.extend_from_slice(&[OP_TIPO, 1, tipo]);
    if let Some((ip, servidor)) = pedido {
        m.extend_from_slice(&[OP_IP_PEDIDO, 4]);
        m.extend_from_slice(&ip);
        m.extend_from_slice(&[OP_SERVIDOR, 4]);
        m.extend_from_slice(&servidor);
    }
    m.extend_from_slice(&[OP_NOME, 4, b'm', b'x', b'o', b's']);
    m.extend_from_slice(&[OP_PARAMETROS, 3, OP_MASCARA, OP_GATEWAY, OP_DNS]);
    m.push(OP_FIM);
    if m.len() < TAMANHO_MINIMO {
        m.resize(TAMANHO_MINIMO, 0);
    }
    m
}

/// Le uma resposta do servidor para a nossa transacao.
fn ler(dados: &[u8], xid: u32, mac: [u8; 6]) -> Option<Mensagem> {
    if dados.len() < TAMANHO_BOOTP + 4 || dados[0] != 2 || ler32(dados, 4) != xid || dados[28..34] != mac {
        return None;
    }
    if ler32(dados, TAMANHO_BOOTP) != COOKIE {
        return None;
    }
    let mut m = Mensagem { tipo: 0, ip: ip4(dados, 16), servidor: None, mascara: None, gateway: None, dns: None };
    let mut i = TAMANHO_BOOTP + 4;
    while i < dados.len() {
        let codigo = dados[i];
        if codigo == OP_FIM {
            break;
        }
        if codigo == OP_PREENCHIMENTO {
            i += 1;
            continue;
        }
        let tamanho = *dados.get(i + 1)? as usize;
        let valor = dados.get(i + 2..i + 2 + tamanho)?;
        let primeiro_ip = (tamanho >= 4).then(|| ip4(valor, 0));
        match codigo {
            OP_TIPO if tamanho >= 1 => m.tipo = valor[0],
            OP_SERVIDOR => m.servidor = primeiro_ip,
            OP_MASCARA => m.mascara = primeiro_ip,
            OP_GATEWAY => m.gateway = primeiro_ip,
            OP_DNS => m.dns = primeiro_ip,
            _ => {}
        }
        i += 2 + tamanho;
    }
    (m.tipo != 0).then_some(m)
}

impl Pilha {
    fn enviar_dhcp(&mut self, fase: Fase, xid: u32) {
        let mensagem = match fase {
            Fase::Descobrindo => montar(DISCOVER, xid, self.mac, None),
            Fase::Pedindo(ip, servidor) => montar(REQUEST, xid, self.mac, Some((ip, servidor))),
        };
        self.enviar_udp(BROADCAST_IP, PORTA_CLIENTE, PORTA_SERVIDOR, &mensagem);
    }

    /// Usa a configuracao do ACK.
    fn aplicar_dhcp(&mut self, ack: &Mensagem, oferecido: [u8; 4]) {
        self.ip = if ack.ip != [0; 4] { ack.ip } else { oferecido };
        self.mascara = ack.mascara.unwrap_or([255, 255, 255, 0]);
        self.gateway = ack.gateway.unwrap_or([0; 4]);
        self.dns = ack.dns.or(ack.gateway).unwrap_or([0; 4]);
    }
}

/// Pega IP, mascara, gateway e DNS. Reenvia a cada 700 ms, desiste em 3 s.
pub fn configurar() -> Result<(), ErroRede> {
    let xid = (cpu::contador() as u32) ^ 0x4d58_4f53;
    let mut fase = Fase::Descobrindo;
    let mut ultimo_envio = agora_ms();
    com(|p| {
        p.udp.abrir(PORTA_CLIENTE);
        p.enviar_dhcp(fase, xid);
    })?;
    let resultado = esperar(TEMPO_TOTAL_MS, |p| {
        while let Some(d) = p.udp.tirar(PORTA_CLIENTE) {
            let Some(m) = ler(&d.dados, xid, p.mac) else { continue };
            match (fase, m.tipo) {
                (Fase::Descobrindo, OFFER) => {
                    fase = Fase::Pedindo(m.ip, m.servidor.unwrap_or(d.origem));
                    p.enviar_dhcp(fase, xid);
                    ultimo_envio = agora_ms();
                }
                (Fase::Pedindo(oferecido, _), ACK) => {
                    p.aplicar_dhcp(&m, oferecido);
                    return Some(());
                }
                (Fase::Pedindo(..), NAK) => {
                    fase = Fase::Descobrindo;
                    p.enviar_dhcp(fase, xid);
                    ultimo_envio = agora_ms();
                }
                _ => {}
            }
        }
        if agora_ms().saturating_sub(ultimo_envio) >= REENVIO_MS {
            p.enviar_dhcp(fase, xid);
            ultimo_envio = agora_ms();
        }
        None
    });
    let _ = com(|p| p.udp.fechar(PORTA_CLIENTE));
    resultado
}
