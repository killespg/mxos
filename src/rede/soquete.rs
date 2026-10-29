//! A rede para os programas (os sockets do Linux, em processo/soquetes.rs).
//!
//! TCP: cada socket conectado e uma Conexao guardada em `Pilha::soquetes`;
//! a de numero 0 continua sendo a do navegador e do comando http. Para
//! usar as funcoes de tcp.rs (que trabalham na `Pilha::tcp`), a conexao
//! do socket troca de lugar com ela durante a chamada (`tcp_com`).
//!
//! UDP: uma porta aberta na caixa postal por socket.
//! ICMP: os sockets "de ping" recebem copia de todo ICMP que chega.
//!
//! Nada aqui espera: quem chama (a chamada de sistema) volta e tenta de
//! novo na proxima volta do laco principal, que tambem processa a rede.

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU16, Ordering};

use super::ipv4::PROTO_ICMP;
use super::tcp::{Conexao, Estado};
use super::{ErroRede, Pilha, com};

/// Datagramas ICMP guardados para os sockets de ping.
const MAX_ICMP: usize = 32;
/// Portas UDP dos sockets dos programas.
const PRIMEIRA_PORTA_UDP: u16 = 33000;

static PROXIMA_UDP: AtomicU16 = AtomicU16::new(PRIMEIRA_PORTA_UDP);

/// Como esta a conexao de um socket TCP.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EstadoTcp {
    Conectando,
    Conectada,
    /// Caiu ou nunca abriu (com o motivo, se foi erro).
    Fechada(Option<ErroRede>),
}

impl Pilha {
    /// Roda `acao` com a conexao `id` no lugar da `self.tcp` (0 = a
    /// propria `self.tcp`, a do navegador).
    pub(super) fn tcp_com<R>(&mut self, id: usize, acao: impl FnOnce(&mut Pilha) -> R) -> R {
        if id == 0 {
            return acao(self);
        }
        let mut c = self.soquetes[id - 1].take().expect("rede: socket TCP sem conexao");
        core::mem::swap(&mut self.tcp, &mut c);
        let r = acao(self);
        core::mem::swap(&mut self.tcp, &mut c);
        self.soquetes[id - 1] = Some(c);
        r
    }

    /// Guarda uma copia do ICMP que chegou para os sockets de ping.
    pub(super) fn guardar_icmp(&mut self, origem: [u8; 4], dados: &[u8]) {
        if self.ouvintes_icmp == 0 {
            return;
        }
        if self.icmp_recebidos.len() == MAX_ICMP {
            self.icmp_recebidos.pop_front();
        }
        self.icmp_recebidos.push_back((origem, dados.to_vec()));
    }
}

/// Conecta a `ip:porta`. Devolve o numero da conexao.
pub fn tcp_abrir(ip: [u8; 4], porta: u16) -> Result<usize, ErroRede> {
    com(|p| {
        let id = match p.soquetes.iter().position(Option::is_none) {
            Some(k) => k + 1,
            None => {
                p.soquetes.push(None);
                p.soquetes.len()
            }
        };
        p.soquetes[id - 1] = Some(Conexao::nova(0));
        p.tcp_com(id, |p| p.tcp_conectar(ip, porta));
        id
    })
}

pub fn tcp_estado(id: usize) -> EstadoTcp {
    com(|p| {
        p.processar();
        p.tcp_com(id, |p| match p.tcp.estado {
            Estado::SynEnviado | Estado::SynRecebido => EstadoTcp::Conectando,
            Estado::Fechada if p.tcp.erro.is_some() => EstadoTcp::Fechada(p.tcp.erro),
            Estado::Fechada | Estado::TempoEspera => EstadoTcp::Fechada(None),
            _ => EstadoTcp::Conectada,
        })
    })
    .unwrap_or(EstadoTcp::Fechada(Some(ErroRede::SemRede)))
}

/// Enfileira para enviar. false se a conexao nao aceita mais dados.
pub fn tcp_enviar(id: usize, dados: &[u8]) -> bool {
    com(|p| p.tcp_com(id, |p| p.tcp_enviar(dados))).unwrap_or(false)
}

/// Ate `maximo` bytes que chegaram, e se o outro lado ja fechou (sem
/// nada mais para ler).
pub fn tcp_ler(id: usize, maximo: usize) -> (Vec<u8>, bool) {
    com(|p| {
        p.processar();
        p.tcp_com(id, |p| {
            let mut tudo = Vec::new();
            p.tcp_ler(&mut tudo);
            let resto = if tudo.len() > maximo { tudo.split_off(maximo) } else { Vec::new() };
            // o que nao coube volta para a frente do buffer
            if !resto.is_empty() {
                p.tcp_devolver(resto);
            }
            let fim = p.tcp.terminou();
            (tudo, fim)
        })
    })
    .unwrap_or((Vec::new(), true))
}

/// Ha algo para ler (ou o fim)?
pub fn tcp_legivel(id: usize) -> bool {
    com(|p| {
        p.processar();
        p.tcp_com(id, |p| p.tcp.tem_dados() || p.tcp.terminou())
    })
    .unwrap_or(true)
}

/// Fechamento educado (FIN depois do que falta enviar).
pub fn tcp_fechar_envio(id: usize) {
    let _ = com(|p| p.tcp_com(id, |p| p.tcp_fechar()));
}

/// O programa largou o socket: fecha e a conexao some quando terminar.
pub fn tcp_largar(id: usize) {
    let _ = com(|p| {
        p.tcp_com(id, |p| {
            p.tcp_fechar();
            p.tcp.largada = true;
        })
    });
}

/// Passa a aceitar conexoes na `porta`.
pub fn tcp_escutar(porta: u16) -> Result<(), ErroRede> {
    com(|p| {
        if !p.escutas.contains(&porta) {
            p.escutas.push(porta);
        }
    })
}

/// Para de escutar: as conexoes que ninguem aceitou caem.
pub fn tcp_parar(porta: u16) {
    let _ = com(|p| {
        p.escutas.retain(|&e| e != porta);
        for id in 1..=p.soquetes.len() {
            let pendente = p.soquetes[id - 1].as_ref().is_some_and(|c| c.escuta == Some(porta) && !c.entregue);
            if pendente {
                p.tcp_com(id, |p| p.tcp_abortar());
                p.soquetes[id - 1] = None;
            }
        }
    });
}

/// Uma conexao pronta na `porta` (o accept): (numero, ip e porta do cliente).
pub fn tcp_aceitar(porta: u16) -> Option<(usize, [u8; 4], u16)> {
    com(|p| {
        p.processar();
        let k = p.soquetes.iter().position(|c| {
            c.as_ref().is_some_and(|c| {
                c.escuta == Some(porta) && !c.entregue && matches!(c.estado, Estado::Estabelecida | Estado::EsperaFechar)
            })
        })?;
        let c = p.soquetes[k].as_mut()?;
        c.entregue = true;
        Some((k + 1, c.remoto(), c.porta_remota()))
    })
    .ok()
    .flatten()
}

/// Ha conexao esperando o accept na `porta`?
pub fn tcp_tem_cliente(porta: u16) -> bool {
    com(|p| {
        p.processar();
        p.soquetes.iter().flatten().any(|c| {
            c.escuta == Some(porta) && !c.entregue && matches!(c.estado, Estado::Estabelecida | Estado::EsperaFechar)
        })
    })
    .unwrap_or(false)
}

/// A porta local da conexao.
pub fn tcp_porta_local(id: usize) -> u16 {
    com(|p| p.tcp_com(id, |p| p.tcp.porta_local())).unwrap_or(0)
}

/// Abre uma porta UDP (0 = uma livre). Devolve a porta.
pub fn udp_abrir(porta: u16) -> Result<u16, ErroRede> {
    let porta = if porta != 0 {
        porta
    } else {
        let p = PROXIMA_UDP.fetch_add(1, Ordering::Relaxed);
        if p >= 60000 {
            PROXIMA_UDP.store(PRIMEIRA_PORTA_UDP, Ordering::Relaxed);
        }
        p
    };
    com(|p| p.udp.abrir(porta))?;
    Ok(porta)
}

pub fn udp_fechar(porta: u16) {
    let _ = com(|p| p.udp.fechar(porta));
}

pub fn udp_enviar(porta: u16, destino: [u8; 4], porta_destino: u16, dados: &[u8]) -> Result<(), ErroRede> {
    com(|p| p.enviar_udp(destino, porta, porta_destino, dados))
}

/// Proximo datagrama da porta: (origem, porta de origem, dados).
pub fn udp_tirar(porta: u16) -> Option<([u8; 4], u16, Vec<u8>)> {
    com(|p| {
        p.processar();
        p.udp.tirar(porta).map(|d| (d.origem, d.porta_origem, d.dados))
    })
    .ok()
    .flatten()
}

/// Ha datagrama esperando na porta?
pub fn udp_legivel(porta: u16) -> bool {
    com(|p| {
        p.processar();
        p.udp.tem(porta)
    })
    .unwrap_or(false)
}

/// Mais (ou menos) um socket de ping.
pub fn icmp_ouvir(liga: bool) {
    let _ = com(|p| {
        if liga {
            p.ouvintes_icmp += 1;
        } else {
            p.ouvintes_icmp = p.ouvintes_icmp.saturating_sub(1);
            if p.ouvintes_icmp == 0 {
                p.icmp_recebidos.clear();
            }
        }
    });
}

/// Manda uma mensagem ICMP pronta (o programa calculou o checksum).
pub fn icmp_enviar(destino: [u8; 4], mensagem: &[u8]) -> Result<(), ErroRede> {
    com(|p| p.enviar_ip(destino, PROTO_ICMP, &[mensagem]))
}

/// O ICMP mais antigo que chegou: (origem, mensagem). Todos os sockets de
/// ping veem a mesma fila (normalmente so ha um).
pub fn icmp_tirar() -> Option<([u8; 4], Vec<u8>)> {
    com(|p| {
        p.processar();
        p.icmp_recebidos.pop_front()
    })
    .ok()
    .flatten()
}

pub fn icmp_legivel() -> bool {
    com(|p| {
        p.processar();
        !p.icmp_recebidos.is_empty()
    })
    .unwrap_or(false)
}

/// Nosso IP (0.0.0.0 sem rede).
pub fn ip_local() -> [u8; 4] {
    com(|p| p.ip).unwrap_or([0; 4])
}

/// O servidor de DNS do DHCP.
pub fn dns() -> [u8; 4] {
    com(|p| p.dns).unwrap_or([0; 4])
}

pub(super) type FilaIcmp = VecDeque<([u8; 4], Vec<u8>)>;
