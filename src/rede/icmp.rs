//! ICMP (RFC 792), so o echo: tipo 8 pedido, tipo 0 resposta.
//!   tipo, codigo, checksum, identificador, sequencia, dados
//!
//! Respondemos a todo pedido e guardamos a ultima resposta que chegou
//! para o `ping` comparar identificador e sequencia.

use alloc::vec::Vec;

use super::ipv4::PROTO_ICMP;
use super::{ErroRede, Pilha, checksum, com, escrever16, esperar, ler16};
use crate::cpu;

const ECO_RESPOSTA: u8 = 0;
const ECO_PEDIDO: u8 = 8;
const TAMANHO: usize = 8;
/// Identificador dos nossos pings ("MX").
const IDENTIFICADOR: u16 = 0x4d58;
/// Bytes de dados em cada ping.
const DADOS_PING: usize = 32;

pub struct Ecos {
    sequencia: u16,
    /// (identificador, sequencia) da ultima resposta recebida.
    ultima: Option<(u16, u16)>,
}

impl Ecos {
    pub fn novo() -> Ecos {
        Ecos { sequencia: 0, ultima: None }
    }
}

/// Monta uma mensagem echo com o checksum certo.
fn montar(tipo: u8, identificador: u16, sequencia: u16, dados: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(TAMANHO + dados.len());
    m.extend_from_slice(&[tipo, 0, 0, 0]);
    m.extend_from_slice(&identificador.to_be_bytes());
    m.extend_from_slice(&sequencia.to_be_bytes());
    m.extend_from_slice(dados);
    let soma = checksum(&[&m]);
    escrever16(&mut m, 2, soma);
    m
}

impl Pilha {
    pub(super) fn receber_icmp(&mut self, origem: [u8; 4], dados: &[u8]) {
        if dados.len() < TAMANHO || checksum(&[dados]) != 0 {
            return;
        }
        self.guardar_icmp(origem, dados);
        let identificador = ler16(dados, 4);
        let sequencia = ler16(dados, 6);
        match dados[0] {
            ECO_PEDIDO if self.ip != [0; 4] => {
                let resposta = montar(ECO_RESPOSTA, identificador, sequencia, &dados[TAMANHO..]);
                self.enviar_ip(origem, PROTO_ICMP, &[&resposta]);
            }
            ECO_RESPOSTA => self.icmp.ultima = Some((identificador, sequencia)),
            _ => {}
        }
    }

    /// Manda um echo e devolve a sequencia usada.
    fn enviar_ping(&mut self, destino: [u8; 4]) -> u16 {
        self.icmp.sequencia = self.icmp.sequencia.wrapping_add(1);
        let sequencia = self.icmp.sequencia;
        let mut dados = [0u8; DADOS_PING];
        for (i, byte) in dados.iter_mut().enumerate() {
            *byte = b'a' + (i % 26) as u8;
        }
        let mensagem = montar(ECO_PEDIDO, IDENTIFICADOR, sequencia, &dados);
        self.icmp.ultima = None;
        self.enviar_ip(destino, PROTO_ICMP, &[&mensagem]);
        sequencia
    }
}

/// Ida e volta em ms (arredondado para cima: menos de 1 ms vira 1).
/// A espera acorda a cada tique, entao a precisao e de uns 10 ms.
pub fn ping(destino: [u8; 4], tempo_limite_ms: u64) -> Result<u64, ErroRede> {
    let inicio = cpu::contador();
    let sequencia = com(|p| p.enviar_ping(destino))?;
    esperar(tempo_limite_ms, |p| (p.icmp.ultima == Some((IDENTIFICADOR, sequencia))).then_some(()))?;
    let ticks = cpu::contador().wrapping_sub(inicio);
    let por_ms = (cpu::frequencia_timer() / 1000).max(1);
    Ok(ticks.div_ceil(por_ms).max(1))
}
