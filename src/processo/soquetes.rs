//! Sockets do Linux sobre a pilha de rede do MXOS (rede/soquete.rs):
//!
//!   AF_INET + SOCK_STREAM   TCP cliente (connect, send, recv, shutdown) e
//!                           servidor (bind, listen, accept)
//!   AF_INET + SOCK_DGRAM    UDP (bind, connect, sendto, recvfrom...)
//!   AF_INET + ICMP          ping (SOCK_RAW ou SOCK_DGRAM com IPPROTO_ICMP)
//!
//!   AF_UNIX                 entre processos e com o kernel (unix.rs)
//!
//! IPv6 ainda nao existe: devolve o erro que o Linux daria sem suporte, e
//! os programas se viram. 127.x.x.x vira o nosso IP (a pilha entrega para
//! si mesma sem passar pela placa).
//!
//! Chamada que espera (connect, recv sem dados) segue a regra das outras:
//! Fim::Esperar e o SVC repete na proxima volta; com O_NONBLOCK (ou
//! MSG_DONTWAIT) volta EAGAIN/EINPROGRESS na hora.

use alloc::string::String;
use alloc::vec::Vec;

use super::arquivos::*;
use super::unix::{self, Ponta, Unix};
use super::chamadas::{Fim, escrever_mem, ler_mem};
use super::Tabela;
use crate::rede::soquete::{self as rede, EstadoTcp};
use crate::rede::ErroRede;

const AF_UNSPEC: u16 = 0;
const AF_UNIX: u64 = 1;
const AF_INET: u64 = 2;
const SOCK_STREAM: u64 = 1;
const SOCK_DGRAM: u64 = 2;
const SOCK_RAW: u64 = 3;
const SOCK_SEQPACKET: u64 = 5;
const SOCK_NONBLOCK: u64 = 0o4000;
const SOCK_CLOEXEC: u64 = 0o2000000;
const IPPROTO_ICMP: u64 = 1;
const MSG_DONTWAIT: u64 = 0x40;
const MSG_PEEK: u64 = 0x2;

const ENOTSOCK: i64 = 88;
const EDESTADDRREQ: i64 = 89;
const EPROTONOSUPPORT: i64 = 93;
const EOPNOTSUPP: i64 = 95;
const EAFNOSUPPORT: i64 = 97;
const EADDRINUSE: i64 = 98;
const ENETUNREACH: i64 = 101;
const ECONNRESET: i64 = 104;
const EISCONN: i64 = 106;
const ENOTCONN: i64 = 107;
const ETIMEDOUT: i64 = 110;
const ECONNREFUSED: i64 = 111;
const EALREADY: i64 = 114;
const EINPROGRESS: i64 = 115;

/// Endereco IPv4 + porta.
type Endereco = ([u8; 4], u16);

/// O que um recv devolve: dados, de onde vieram e descritores (AF_UNIX).
pub type Recebido = (Vec<u8>, Option<Endereco>, Vec<Aberto>);

/// Erro de mentira: "espera e tenta de novo" (nunca chega ao programa).
const ESPERAR: i64 = -1;

pub enum Soquete {
    Tcp {
        /// A conexao na rede (depois do connect).
        conexao: Option<usize>,
        remoto: Option<Endereco>,
        /// Ja respondemos o connect nao bloqueante que terminou.
        avisado: bool,
        /// Porta do bind e se o listen ja foi feito nela.
        local: Option<u16>,
        escutando: bool,
    },
    Udp {
        porta: Option<u16>,
        remoto: Option<Endereco>,
    },
    Icmp {
        /// SOCK_RAW: o que chega vem com o cabecalho IP na frente.
        bruto: bool,
        remoto: Option<[u8; 4]>,
    },
    Unix(Unix),
}

impl Drop for Soquete {
    fn drop(&mut self) {
        match self {
            Soquete::Tcp { conexao: Some(id), .. } => rede::tcp_largar(*id),
            Soquete::Tcp { local: Some(porta), escutando: true, .. } => rede::tcp_parar(*porta),
            Soquete::Udp { porta: Some(porta), .. } => rede::udp_fechar(*porta),
            Soquete::Icmp { .. } => rede::icmp_ouvir(false),
            _ => {}
        }
    }
}

fn errno_rede(e: ErroRede) -> i64 {
    match e {
        ErroRede::Recusado => ECONNREFUSED,
        ErroRede::TempoEsgotado => ETIMEDOUT,
        ErroRede::SemRede => ENETUNREACH,
        _ => ECONNRESET,
    }
}

/// struct sockaddr_in -> (ip, porta). None = AF_UNSPEC.
fn ler_endereco(t: &Tabela, i: usize, endereco: usize, tamanho: usize) -> Result<Option<Endereco>, i64> {
    if endereco == 0 || tamanho < 2 {
        return Err(EINVAL);
    }
    let b = ler_mem(t, i, endereco, tamanho.min(16))?;
    let familia = u16::from_le_bytes([b[0], b[1]]);
    if familia == AF_UNSPEC {
        return Ok(None);
    }
    if familia as u64 != AF_INET {
        return Err(EAFNOSUPPORT);
    }
    if b.len() < 8 {
        return Err(EINVAL);
    }
    let mut ip = [b[4], b[5], b[6], b[7]];
    if ip[0] == 127 {
        ip = rede::ip_local(); // loopback
    }
    Ok(Some((ip, u16::from_be_bytes([b[2], b[3]]))))
}

/// Escreve um sockaddr_in em `endereco` (com o tamanho em `*tamanho`).
fn escrever_endereco(t: &mut Tabela, i: usize, endereco: usize, tamanho: usize, valor: Endereco) -> Result<(), i64> {
    if endereco == 0 || tamanho == 0 {
        return Ok(());
    }
    let cabe = ler_mem(t, i, tamanho, 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)?;
    let mut s = [0u8; 16];
    s[..2].copy_from_slice(&(AF_INET as u16).to_le_bytes());
    s[2..4].copy_from_slice(&valor.1.to_be_bytes());
    s[4..8].copy_from_slice(&valor.0);
    escrever_mem(t, i, endereco, &s[..cabe.min(16)])?;
    escrever_mem(t, i, tamanho, &16u32.to_le_bytes())
}

/// O socket do descritor `fd` (e se ele e nao bloqueante).
fn com_soquete<R>(t: &Tabela, i: usize, fd: u64, acao: impl FnOnce(&mut Soquete, bool) -> R) -> Result<R, i64> {
    let aberto = t.processos[i].linux.fd_aberto(fd)?;
    let mut d = aberto.borrow_mut();
    let nao_bloqueia = d.flags & O_NONBLOCK != 0;
    match &mut d.tipo {
        Tipo::Soquete(s) => Ok(acao(s, nao_bloqueia)),
        _ => Err(ENOTSOCK),
    }
}

/// A familia de um sockaddr no programa.
fn familia(t: &Tabela, i: usize, endereco: usize, tamanho: usize) -> Result<u64, i64> {
    if endereco == 0 || tamanho < 2 {
        return Err(EINVAL);
    }
    let b = ler_mem(t, i, endereco, 2)?;
    Ok(u16::from_le_bytes([b[0], b[1]]) as u64)
}

/// struct sockaddr_un -> nome.
fn ler_nome_unix(t: &Tabela, i: usize, endereco: usize, tamanho: usize) -> Result<String, i64> {
    let b = ler_mem(t, i, endereco, tamanho.min(110))?;
    unix::nome_do_endereco(&b).ok_or(EINVAL)
}

fn escrever_nome_unix(t: &mut Tabela, i: usize, endereco: usize, tamanho: usize, nome: Option<&str>) -> Result<(), i64> {
    if endereco == 0 || tamanho == 0 {
        return Ok(());
    }
    let cabe = ler_mem(t, i, tamanho, 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)?;
    let s = unix::endereco_do_nome(nome);
    escrever_mem(t, i, endereco, &s[..cabe.min(s.len())])?;
    escrever_mem(t, i, tamanho, &(s.len() as u32).to_le_bytes())
}

/// Poe `soquete` num descritor novo (SOCK_NONBLOCK e SOCK_CLOEXEC de `tipo`).
fn novo_soquete(t: &mut Tabela, i: usize, soquete: Soquete, tipo: u64) -> Result<usize, i64> {
    let flags = O_RDWR | if tipo & SOCK_NONBLOCK != 0 { O_NONBLOCK } else { 0 };
    let aberto = Descricao::nova(Tipo::Soquete(soquete), flags);
    t.processos[i].linux.novo_descritor(aberto, tipo & SOCK_CLOEXEC != 0)
}

/// socketpair (so AF_UNIX): dois soquetes ja ligados, em `vetor[0..2]`.
pub fn socketpair(t: &mut Tabela, i: usize, familia: u64, tipo: u64, vetor: usize) -> Result<i64, i64> {
    if familia != AF_UNIX {
        return Err(EAFNOSUPPORT);
    }
    if !matches!(tipo & 0xF, SOCK_STREAM | SOCK_DGRAM | SOCK_SEQPACKET) {
        return Err(EPROTONOSUPPORT);
    }
    let (a, b) = Ponta::par();
    let fa = novo_soquete(t, i, Soquete::Unix(Unix::Conectado { ponta: a, outro: None }), tipo)?;
    let fb = match novo_soquete(t, i, Soquete::Unix(Unix::Conectado { ponta: b, outro: None }), tipo) {
        Ok(fd) => fd,
        Err(e) => {
            t.processos[i].linux.fechar(fa as u64);
            return Err(e);
        }
    };
    let mut v = [0u8; 8];
    v[..4].copy_from_slice(&(fa as i32).to_le_bytes());
    v[4..].copy_from_slice(&(fb as i32).to_le_bytes());
    escrever_mem(t, i, vetor, &v)?;
    Ok(0)
}

pub fn socket(t: &mut Tabela, i: usize, familia: u64, tipo: u64, protocolo: u64) -> Result<i64, i64> {
    if familia == AF_UNIX {
        if !matches!(tipo & 0xF, SOCK_STREAM | SOCK_DGRAM | SOCK_SEQPACKET) {
            return Err(EPROTONOSUPPORT);
        }
        return novo_soquete(t, i, Soquete::Unix(Unix::Novo { nome: None }), tipo).map(|fd| fd as i64);
    }
    if familia != AF_INET {
        return Err(EAFNOSUPPORT);
    }
    let soquete = match (tipo & 0xF, protocolo) {
        (SOCK_STREAM, 0 | 6) => Soquete::Tcp { conexao: None, remoto: None, avisado: false, local: None, escutando: false },
        (SOCK_DGRAM, 0 | 17) => Soquete::Udp { porta: None, remoto: None },
        (SOCK_DGRAM | SOCK_RAW, IPPROTO_ICMP) => {
            rede::icmp_ouvir(true);
            Soquete::Icmp { bruto: tipo & 0xF == SOCK_RAW, remoto: None }
        }
        _ => return Err(EPROTONOSUPPORT),
    };
    let flags = O_RDWR | if tipo & SOCK_NONBLOCK != 0 { O_NONBLOCK } else { 0 };
    let aberto = Descricao::nova(Tipo::Soquete(soquete), flags);
    t.processos[i].linux.novo_descritor(aberto, tipo & SOCK_CLOEXEC != 0).map(|fd| fd as i64)
}

pub fn connect(t: &mut Tabela, i: usize, fd: u64, endereco: usize, tamanho: usize) -> Fim {
    if familia(t, i, endereco, tamanho) == Ok(AF_UNIX) {
        let r = ler_nome_unix(t, i, endereco, tamanho).and_then(|nome| {
            com_soquete(t, i, fd, |s, _| match s {
                Soquete::Unix(u @ Unix::Novo { .. }) => {
                    let ponta = unix::conectar(&nome).ok_or(if nome.starts_with('@') { ECONNREFUSED } else { ENOENT })?;
                    *u = Unix::Conectado { ponta, outro: Some(nome) };
                    Ok(0)
                }
                Soquete::Unix(Unix::Conectado { .. }) => Err(EISCONN),
                Soquete::Unix(_) => Err(EINVAL),
                _ => Err(EAFNOSUPPORT),
            })?
        });
        return Fim::Valor(r.unwrap_or_else(|e| -e));
    }
    let destino = match ler_endereco(t, i, endereco, tamanho) {
        Ok(d) => d,
        Err(e) => return Fim::Valor(-e),
    };
    let r = com_soquete(t, i, fd, |s, nao_bloqueia| -> Result<Option<i64>, i64> {
        match s {
            Soquete::Tcp { conexao, remoto, avisado, .. } => {
                let Some(destino) = destino else { return Err(EAFNOSUPPORT) };
                let id = match *conexao {
                    Some(id) => id,
                    None => {
                        let id = rede::tcp_abrir(destino.0, destino.1).map_err(errno_rede)?;
                        *conexao = Some(id);
                        *remoto = Some(destino);
                        if nao_bloqueia {
                            return Err(EINPROGRESS);
                        }
                        id
                    }
                };
                match rede::tcp_estado(id) {
                    EstadoTcp::Conectando if nao_bloqueia => Err(EALREADY),
                    EstadoTcp::Conectando => Ok(None),
                    EstadoTcp::Conectada if *avisado => Err(EISCONN),
                    EstadoTcp::Conectada => {
                        *avisado = true;
                        Ok(Some(0))
                    }
                    EstadoTcp::Fechada(erro) => Err(erro.map_or(ECONNREFUSED, errno_rede)),
                }
            }
            Soquete::Udp { porta, remoto } => {
                if porta.is_none() {
                    *porta = Some(rede::udp_abrir(0).map_err(errno_rede)?);
                }
                *remoto = destino;
                Ok(Some(0))
            }
            Soquete::Icmp { remoto, .. } => {
                *remoto = destino.map(|d| d.0);
                Ok(Some(0))
            }
            Soquete::Unix(_) => Err(EAFNOSUPPORT),
        }
    });
    match r {
        Ok(Ok(Some(v))) => Fim::Valor(v),
        Ok(Ok(None)) => Fim::Esperar("conectando"),
        Ok(Err(e)) | Err(e) => Fim::Valor(-e),
    }
}

pub fn bind(t: &mut Tabela, i: usize, fd: u64, endereco: usize, tamanho: usize) -> Result<i64, i64> {
    if familia(t, i, endereco, tamanho)? == AF_UNIX {
        let nome = ler_nome_unix(t, i, endereco, tamanho)?;
        return com_soquete(t, i, fd, |s, _| match s {
            Soquete::Unix(Unix::Novo { nome: atual @ None }) => {
                *atual = Some(nome);
                Ok(0)
            }
            Soquete::Unix(_) => Err(EINVAL),
            _ => Err(EAFNOSUPPORT),
        })?;
    }
    let local = ler_endereco(t, i, endereco, tamanho)?;
    com_soquete(t, i, fd, |s, _| match s {
        Soquete::Udp { porta, .. } => {
            if let Some(velha) = porta.take() {
                rede::udp_fechar(velha);
            }
            *porta = Some(rede::udp_abrir(local.map_or(0, |l| l.1)).map_err(errno_rede)?);
            Ok(0)
        }
        Soquete::Tcp { local: porta, conexao: None, .. } => {
            *porta = Some(local.map_or(0, |l| l.1));
            Ok(0)
        }
        _ => Ok(0),
    })?
}

pub fn listen(t: &mut Tabela, i: usize, fd: u64) -> Result<i64, i64> {
    com_soquete(t, i, fd, |s, _| match s {
        Soquete::Tcp { local: Some(porta), escutando, conexao: None, .. } if *porta != 0 => {
            rede::tcp_escutar(*porta).map_err(errno_rede)?;
            *escutando = true;
            Ok(0)
        }
        Soquete::Tcp { .. } => Err(EINVAL),
        Soquete::Unix(u @ Unix::Novo { nome: Some(_) }) => {
            let Unix::Novo { nome } = u else { unreachable!() };
            let nome = nome.clone().unwrap();
            let escuta = unix::escutar(&nome).map_err(|_| EADDRINUSE)?;
            *u = Unix::Escutando { nome, escuta };
            Ok(0)
        }
        Soquete::Unix(Unix::Escutando { .. }) => Ok(0),
        Soquete::Unix(_) => Err(EINVAL),
        _ => Err(EOPNOTSUPP),
    })?
}

/// accept e accept4: um socket novo com a conexao que chegou.
pub fn accept(t: &mut Tabela, i: usize, fd: u64, endereco: usize, tamanho: usize, flags: u64) -> Fim {
    let unix_r = com_soquete(t, i, fd, |s, nao_bloqueia| match s {
        Soquete::Unix(Unix::Escutando { escuta, .. }) => Some(match escuta.borrow_mut().fila.pop_front() {
            Some(p) => Ok(Some(p)),
            None if nao_bloqueia => Err(EAGAIN),
            None => Ok(None),
        }),
        Soquete::Unix(_) => Some(Err(EINVAL)),
        _ => None,
    });
    match unix_r {
        Err(e) | Ok(Some(Err(e))) => return Fim::Valor(-e),
        Ok(Some(Ok(None))) => return Fim::Esperar("esperando uma conexão"),
        Ok(Some(Ok(Some(ponta)))) => {
            let soquete = Soquete::Unix(Unix::Conectado { ponta, outro: None });
            let r = novo_soquete(t, i, soquete, flags).and_then(|novo| {
                escrever_nome_unix(t, i, endereco, tamanho, None)?;
                Ok(novo as i64)
            });
            return Fim::Valor(r.unwrap_or_else(|e| -e));
        }
        Ok(None) => {}
    }
    let r = com_soquete(t, i, fd, |s, nao_bloqueia| match s {
        Soquete::Tcp { local: Some(porta), escutando: true, .. } => match rede::tcp_aceitar(*porta) {
            Some(cliente) => Ok(Some(cliente)),
            None if nao_bloqueia => Err(EAGAIN),
            None => Ok(None),
        },
        Soquete::Tcp { .. } => Err(EINVAL),
        _ => Err(EOPNOTSUPP),
    });
    let (id, ip, porta) = match r {
        Ok(Ok(Some(c))) => c,
        Ok(Ok(None)) => return Fim::Esperar("esperando uma conexão"),
        Ok(Err(e)) | Err(e) => return Fim::Valor(-e),
    };
    let soquete = Soquete::Tcp { conexao: Some(id), remoto: Some((ip, porta)), avisado: true, local: None, escutando: false };
    let flags_abertura = O_RDWR | if flags & SOCK_NONBLOCK != 0 { O_NONBLOCK } else { 0 };
    let aberto = Descricao::nova(Tipo::Soquete(soquete), flags_abertura);
    let novo = match t.processos[i].linux.novo_descritor(aberto, flags & SOCK_CLOEXEC != 0) {
        Ok(fd) => fd,
        Err(e) => return Fim::Valor(-e),
    };
    if endereco != 0 {
        if let Err(e) = escrever_endereco(t, i, endereco, tamanho, (ip, porta)) {
            return Fim::Valor(-e);
        }
    }
    Fim::Valor(novo as i64)
}

/// getsockname (`do_outro` = false) e getpeername (true).
pub fn nome(t: &mut Tabela, i: usize, fd: u64, endereco: usize, tamanho: usize, do_outro: bool) -> Result<i64, i64> {
    let nome_unix = com_soquete(t, i, fd, |s, _| match s {
        Soquete::Unix(Unix::Novo { nome }) if !do_outro => Some(Ok(nome.clone())),
        Soquete::Unix(Unix::Escutando { nome, .. }) if !do_outro => Some(Ok(Some(nome.clone()))),
        Soquete::Unix(Unix::Conectado { outro, .. }) if do_outro => Some(Ok(outro.clone())),
        Soquete::Unix(Unix::Conectado { .. }) => Some(Ok(None)),
        Soquete::Unix(_) => Some(Err(ENOTCONN)),
        _ => None,
    })?;
    if let Some(n) = nome_unix {
        escrever_nome_unix(t, i, endereco, tamanho, n?.as_deref())?;
        return Ok(0);
    }
    let valor = com_soquete(t, i, fd, |s, _| match s {
        Soquete::Tcp { remoto, .. } if do_outro => remoto.ok_or(ENOTCONN),
        Soquete::Udp { remoto, .. } if do_outro => remoto.ok_or(ENOTCONN),
        Soquete::Icmp { remoto, .. } if do_outro => remoto.map(|ip| (ip, 0)).ok_or(ENOTCONN),
        Soquete::Udp { porta, .. } => Ok((rede::ip_local(), porta.unwrap_or(0))),
        Soquete::Tcp { conexao: Some(id), .. } => Ok((rede::ip_local(), rede::tcp_porta_local(*id))),
        Soquete::Tcp { local: Some(porta), .. } => Ok((rede::ip_local(), *porta)),
        _ => Ok((rede::ip_local(), 0)),
    })??;
    escrever_endereco(t, i, endereco, tamanho, valor)?;
    Ok(0)
}

pub fn getsockopt(t: &mut Tabela, i: usize, fd: u64, nivel: u64, opcao: u64, valor: usize, tamanho: usize) -> Result<i64, i64> {
    const SOL_SOCKET: u64 = 1;
    const SO_TYPE: u64 = 3;
    const SO_ERROR: u64 = 4;
    const SO_PEERCRED: u64 = 17;
    if (nivel, opcao) == (SOL_SOCKET, SO_PEERCRED) && valor != 0 {
        // struct ucred (pid, uid, gid): todo mundo e root, e o outro lado
        // aparece como o proprio processo
        let mut c = [0u8; 12];
        c[..4].copy_from_slice(&t.processos[i].pid.to_le_bytes());
        escrever_mem(t, i, valor, &c)?;
        escrever_mem(t, i, tamanho, &12u32.to_le_bytes())?;
        return Ok(0);
    }
    let resposta = com_soquete(t, i, fd, |s, _| match (nivel, opcao) {
        (SOL_SOCKET, SO_TYPE) => match s {
            Soquete::Tcp { .. } | Soquete::Unix(_) => SOCK_STREAM as i32,
            Soquete::Udp { .. } => SOCK_DGRAM as i32,
            Soquete::Icmp { bruto, .. } => if *bruto { SOCK_RAW as i32 } else { SOCK_DGRAM as i32 },
        },
        (SOL_SOCKET, SO_ERROR) => match s {
            // o resultado de um connect nao bloqueante
            Soquete::Tcp { conexao: Some(id), .. } => match rede::tcp_estado(*id) {
                EstadoTcp::Fechada(Some(e)) => errno_rede(e) as i32,
                _ => 0,
            },
            _ => 0,
        },
        _ => 0,
    })?;
    if valor != 0 && tamanho != 0 {
        escrever_mem(t, i, valor, &resposta.to_le_bytes())?;
        escrever_mem(t, i, tamanho, &4u32.to_le_bytes())?;
    }
    Ok(0)
}

pub fn shutdown(t: &mut Tabela, i: usize, fd: u64, como: u64) -> Result<i64, i64> {
    com_soquete(t, i, fd, |s, _| match s {
        Soquete::Tcp { conexao: Some(id), .. } => {
            // SHUT_WR (1) ou SHUT_RDWR (2): manda o FIN
            if como != 0 {
                rede::tcp_fechar_envio(*id);
            }
            Ok(0)
        }
        Soquete::Tcp { .. } => Err(ENOTCONN),
        Soquete::Unix(Unix::Conectado { ponta, .. }) => {
            if como != 0 {
                ponta.saida.borrow_mut().fechado = true;
                unix::avisar();
            }
            Ok(0)
        }
        _ => Ok(0),
    })?
}

/// Envia `dados` (para `destino`, se veio no sendto/sendmsg).
pub fn enviar(t: &mut Tabela, i: usize, fd: u64, dados: &[u8], destino: Option<Endereco>) -> Fim {
    enviar_com(t, i, fd, dados, destino, Vec::new())
}

/// Envia com descritores junto (SCM_RIGHTS; so AF_UNIX usa).
fn enviar_com(t: &mut Tabela, i: usize, fd: u64, dados: &[u8], destino: Option<Endereco>, mut descritores: Vec<Aberto>) -> Fim {
    let r = com_soquete(t, i, fd, |s, nao_bloqueia| -> Result<i64, i64> {
        match s {
            Soquete::Tcp { conexao: Some(id), .. } => match rede::tcp_estado(*id) {
                EstadoTcp::Fechada(erro) => Err(erro.map_or(EPIPE, errno_rede)),
                _ if rede::tcp_enviar(*id, dados) => Ok(dados.len() as i64),
                _ => Err(EPIPE),
            },
            Soquete::Tcp { .. } => Err(ENOTCONN),
            Soquete::Udp { porta, remoto } => {
                let (ip, porta_destino) = destino.or(*remoto).ok_or(EDESTADDRREQ)?;
                if porta.is_none() {
                    *porta = Some(rede::udp_abrir(0).map_err(errno_rede)?);
                }
                rede::udp_enviar(porta.unwrap(), ip, porta_destino, dados).map_err(errno_rede)?;
                Ok(dados.len() as i64)
            }
            Soquete::Icmp { remoto, .. } => {
                let ip = destino.map(|d| d.0).or(*remoto).ok_or(EDESTADDRREQ)?;
                rede::icmp_enviar(ip, dados).map_err(errno_rede)?;
                Ok(dados.len() as i64)
            }
            Soquete::Unix(Unix::Conectado { ponta, .. }) => {
                let mut saida = ponta.saida.borrow_mut();
                if saida.sem_leitor || saida.fechado {
                    return Err(EPIPE);
                }
                if saida.dados.len() >= unix::TAMANHO_CANAL && !dados.is_empty() {
                    return Err(if nao_bloqueia { EAGAIN } else { ESPERAR });
                }
                if !descritores.is_empty() {
                    let pos = saida.fim();
                    saida.fds.push_back((pos, core::mem::take(&mut descritores)));
                }
                Ok(saida.escrever(dados) as i64)
            }
            Soquete::Unix(_) => Err(ENOTCONN),
        }
    });
    match r {
        Ok(Err(ESPERAR)) => Fim::Esperar("escrevendo num soquete"),
        Ok(Ok(n)) => Fim::Valor(n),
        Ok(Err(EPIPE)) => {
            let pid = t.processos[i].pid;
            t.sinal(pid, 13);
            if t.indice(pid) != Some(i) || matches!(t.processos[i].estado, super::Estado::Terminado(_)) {
                return Fim::Nada;
            }
            Fim::Valor(-EPIPE)
        }
        Ok(Err(e)) | Err(e) => Fim::Valor(-e),
    }
}

/// Cabecalho IPv4 de mentira para o ping bruto (o programa so olha o
/// tamanho dele e a origem).
fn cabecalho_ip(origem: [u8; 4], destino: [u8; 4], tamanho: usize) -> [u8; 20] {
    let mut c = [0u8; 20];
    c[0] = 0x45;
    c[2..4].copy_from_slice(&((20 + tamanho) as u16).to_be_bytes());
    c[8] = 64;
    c[9] = IPPROTO_ICMP as u8;
    c[12..16].copy_from_slice(&origem);
    c[16..20].copy_from_slice(&destino);
    c
}

/// Recebe ate `maximo` bytes. Devolve os dados e de onde vieram.
pub fn receber(t: &mut Tabela, i: usize, fd: u64, maximo: usize, flags: u64) -> Result<Result<Recebido, i64>, &'static str> {
    let r = com_soquete(t, i, fd, |s, nao_bloqueia| -> Result<Option<Recebido>, i64> {
        let espera = !(nao_bloqueia || flags & MSG_DONTWAIT != 0);
        match s {
            Soquete::Tcp { conexao: Some(id), remoto, .. } => {
                if let EstadoTcp::Fechada(Some(e)) = rede::tcp_estado(*id) {
                    return Err(errno_rede(e));
                }
                if flags & MSG_PEEK != 0 && !rede::tcp_legivel(*id) {
                    return if espera { Ok(None) } else { Err(EAGAIN) };
                }
                let (dados, fim) = rede::tcp_ler(*id, maximo);
                if dados.is_empty() && !fim {
                    return if espera { Ok(None) } else { Err(EAGAIN) };
                }
                Ok(Some((dados, *remoto, Vec::new())))
            }
            Soquete::Tcp { .. } => Err(ENOTCONN),
            Soquete::Udp { porta, .. } => {
                if porta.is_none() {
                    *porta = Some(rede::udp_abrir(0).map_err(errno_rede)?);
                }
                match rede::udp_tirar(porta.unwrap()) {
                    Some((ip, porta_origem, mut dados)) => {
                        dados.truncate(maximo);
                        Ok(Some((dados, Some((ip, porta_origem)), Vec::new())))
                    }
                    None if espera => Ok(None),
                    None => Err(EAGAIN),
                }
            }
            Soquete::Icmp { bruto, .. } => match rede::icmp_tirar() {
                Some((ip, mensagem)) => {
                    let mut dados = Vec::new();
                    if *bruto {
                        dados.extend_from_slice(&cabecalho_ip(ip, rede::ip_local(), mensagem.len()));
                    }
                    dados.extend_from_slice(&mensagem);
                    dados.truncate(maximo);
                    Ok(Some((dados, Some((ip, 0)), Vec::new())))
                }
                None if espera => Ok(None),
                None => Err(EAGAIN),
            },
            Soquete::Unix(Unix::Conectado { ponta, .. }) => {
                let mut e = ponta.entrada.borrow_mut();
                if e.dados.is_empty() {
                    if e.fechado {
                        return Ok(Some((Vec::new(), None, Vec::new())));
                    }
                    return if espera { Ok(None) } else { Err(EAGAIN) };
                }
                if flags & MSG_PEEK != 0 {
                    return Ok(Some((e.dados.iter().take(maximo).copied().collect(), None, Vec::new())));
                }
                let (dados, fds) = e.ler(maximo);
                Ok(Some((dados, None, fds)))
            }
            Soquete::Unix(_) => Err(ENOTCONN),
        }
    });
    match r {
        Ok(Ok(Some(v))) => Ok(Ok(v)),
        Ok(Ok(None)) => Err("esperando a rede"),
        Ok(Err(e)) | Err(e) => Ok(Err(e)),
    }
}

/// O que o poll ve num socket: (pode ler, pode escrever, erro/desligou).
pub fn prontidao(s: &Soquete) -> (bool, bool, bool) {
    match s {
        Soquete::Tcp { conexao: Some(id), .. } => match rede::tcp_estado(*id) {
            EstadoTcp::Conectando => (false, false, false),
            EstadoTcp::Conectada => (rede::tcp_legivel(*id), true, false),
            EstadoTcp::Fechada(erro) => (true, false, erro.is_some()),
        },
        Soquete::Tcp { local: Some(porta), escutando: true, .. } => (rede::tcp_tem_cliente(*porta), false, false),
        Soquete::Tcp { .. } => (false, false, true),
        Soquete::Udp { porta, .. } => (porta.is_some_and(rede::udp_legivel), true, false),
        Soquete::Icmp { .. } => (rede::icmp_legivel(), true, false),
        Soquete::Unix(Unix::Conectado { ponta, .. }) => (ponta.legivel(), ponta.gravavel(), false),
        Soquete::Unix(Unix::Escutando { escuta, .. }) => (!escuta.borrow().fila.is_empty(), false, false),
        Soquete::Unix(Unix::Novo { .. }) => (false, false, false),
    }
}

// ---------------------------------------------------------------------
// As chamadas de sistema com os ponteiros do programa

pub fn sendto(t: &mut Tabela, i: usize, fd: u64, buf: usize, tamanho: usize, endereco: usize, tamanho_endereco: usize) -> Fim {
    let dados = match ler_mem(t, i, buf, tamanho.min(1 << 20)) {
        Ok(d) => d,
        Err(e) => return Fim::Valor(-e),
    };
    let destino = if endereco != 0 {
        match ler_endereco(t, i, endereco, tamanho_endereco) {
            Ok(d) => d,
            Err(e) => return Fim::Valor(-e),
        }
    } else {
        None
    };
    enviar(t, i, fd, &dados, destino)
}

pub fn recvfrom(t: &mut Tabela, i: usize, fd: u64, buf: usize, tamanho: usize, flags: u64, endereco: usize, tamanho_endereco: usize) -> Fim {
    match receber(t, i, fd, tamanho.min(1 << 20), flags) {
        Err(motivo) => Fim::Esperar(motivo),
        Ok(Err(e)) => Fim::Valor(-e),
        Ok(Ok((dados, origem, _))) => {
            let r = escrever_mem(t, i, buf, &dados).and_then(|_| match origem {
                Some(o) => escrever_endereco(t, i, endereco, tamanho_endereco, o),
                None => Ok(()),
            });
            match r {
                Ok(()) => Fim::Valor(dados.len() as i64),
                Err(e) => Fim::Valor(-e),
            }
        }
    }
}

/// struct msghdr: (nome, tamanho do nome, pedacos (iov), controle e o
/// tamanho dele).
type Msghdr = (usize, usize, Vec<(usize, usize)>, usize, usize);

fn ler_msghdr(t: &Tabela, i: usize, msg: usize) -> Result<Msghdr, i64> {
    let b = ler_mem(t, i, msg, 56)?;
    let campo = |k: usize| u64::from_le_bytes(b[k..k + 8].try_into().unwrap()) as usize;
    let (nome, tamanho_nome, iov, quantos) = (campo(0), campo(8) & 0xFFFF_FFFF, campo(16), campo(24).min(1024));
    let mut pedacos = Vec::with_capacity(quantos);
    if quantos > 0 {
        let v = ler_mem(t, i, iov, quantos * 16)?;
        for k in 0..quantos {
            let base = u64::from_le_bytes(v[k * 16..k * 16 + 8].try_into().unwrap()) as usize;
            let n = u64::from_le_bytes(v[k * 16 + 8..k * 16 + 16].try_into().unwrap()) as usize;
            pedacos.push((base, n));
        }
    }
    Ok((nome, tamanho_nome, pedacos, campo(32), campo(40)))
}

/// Os descritores de um SCM_RIGHTS nos dados de controle do sendmsg.
fn descritores_do_controle(t: &Tabela, i: usize, controle: usize, tamanho: usize) -> Result<Vec<Aberto>, i64> {
    let mut abertos = Vec::new();
    if controle == 0 || tamanho < 16 {
        return Ok(abertos);
    }
    let b = ler_mem(t, i, controle, tamanho.min(4096))?;
    let mut k = 0;
    while k + 16 <= b.len() {
        let len = u64::from_le_bytes(b[k..k + 8].try_into().unwrap()) as usize;
        let nivel = i32::from_le_bytes(b[k + 8..k + 12].try_into().unwrap());
        let tipo = i32::from_le_bytes(b[k + 12..k + 16].try_into().unwrap());
        if len < 16 || k + len > b.len() {
            break;
        }
        // SOL_SOCKET + SCM_RIGHTS: um int por descritor
        if (nivel, tipo) == (1, 1) {
            for f in b[k + 16..k + len].chunks_exact(4) {
                let fd = i32::from_le_bytes(f.try_into().unwrap());
                abertos.push(t.processos[i].linux.fd_aberto(fd as u64)?);
            }
        }
        k += (len + 7) & !7;
    }
    Ok(abertos)
}

pub fn sendmsg(t: &mut Tabela, i: usize, fd: u64, msg: usize) -> Fim {
    let r = (|| -> Result<(Vec<u8>, Option<Endereco>, Vec<Aberto>), i64> {
        let (nome, tamanho_nome, pedacos, controle, tamanho_controle) = ler_msghdr(t, i, msg)?;
        let mut dados = Vec::new();
        for (base, n) in pedacos {
            dados.extend(ler_mem(t, i, base, n.min(1 << 20))?);
        }
        let destino = if nome != 0 && familia(t, i, nome, tamanho_nome)? != AF_UNIX {
            ler_endereco(t, i, nome, tamanho_nome)?
        } else {
            None
        };
        let descritores = descritores_do_controle(t, i, controle, tamanho_controle)?;
        Ok((dados, destino, descritores))
    })();
    match r {
        Ok((dados, destino, descritores)) => enviar_com(t, i, fd, &dados, destino, descritores),
        Err(e) => Fim::Valor(-e),
    }
}

pub fn recvmsg(t: &mut Tabela, i: usize, fd: u64, msg: usize, flags: u64) -> Fim {
    let (nome, _, pedacos, controle, tamanho_controle) = match ler_msghdr(t, i, msg) {
        Ok(m) => m,
        Err(e) => return Fim::Valor(-e),
    };
    let cabe: usize = pedacos.iter().map(|p| p.1).sum();
    match receber(t, i, fd, cabe.min(1 << 20), flags) {
        Err(motivo) => Fim::Esperar(motivo),
        Ok(Err(e)) => Fim::Valor(-e),
        Ok(Ok((dados, origem, descritores))) => {
            let r = (|| -> Result<(), i64> {
                let mut feito = 0;
                for (base, n) in pedacos {
                    let parte = &dados[feito.min(dados.len())..(feito + n).min(dados.len())];
                    escrever_mem(t, i, base, parte)?;
                    feito += parte.len();
                }
                if let (Some(o), true) = (origem, nome != 0) {
                    // msg_namelen fica no msghdr, logo depois do ponteiro
                    escrever_endereco(t, i, nome, msg + 8, o)?;
                }
                // descritores que vieram (SCM_RIGHTS) viram descritores
                // deste processo; msg_flags = 0
                let mut usado = 0u64;
                if !descritores.is_empty() && controle != 0 {
                    const MSG_CMSG_CLOEXEC: u64 = 0x4000_0000;
                    let mut cmsg = Vec::new();
                    let len = 16 + 4 * descritores.len();
                    cmsg.extend_from_slice(&(len as u64).to_le_bytes());
                    cmsg.extend_from_slice(&1i32.to_le_bytes());
                    cmsg.extend_from_slice(&1i32.to_le_bytes());
                    for aberto in descritores {
                        let novo = t.processos[i].linux.novo_descritor(aberto, flags & MSG_CMSG_CLOEXEC != 0)?;
                        cmsg.extend_from_slice(&(novo as i32).to_le_bytes());
                    }
                    cmsg.resize((len + 7) & !7, 0);
                    let n = cmsg.len().min(tamanho_controle);
                    escrever_mem(t, i, controle, &cmsg[..n])?;
                    usado = n as u64;
                }
                escrever_mem(t, i, msg + 40, &usado.to_le_bytes())?;
                escrever_mem(t, i, msg + 48, &0u32.to_le_bytes())?;
                Ok(())
            })();
            match r {
                Ok(()) => Fim::Valor(dados.len() as i64),
                Err(e) => Fim::Valor(-e),
            }
        }
    }
}

/// sendmmsg: varios sendmsg (struct mmsghdr = msghdr + msg_len).
pub fn sendmmsg(t: &mut Tabela, i: usize, fd: u64, vetor: usize, quantos: usize) -> Fim {
    let mut enviados = 0;
    for k in 0..quantos.min(64) {
        let msg = vetor + k * 64;
        match sendmsg(t, i, fd, msg) {
            Fim::Valor(n) if n >= 0 => {
                if let Err(e) = escrever_mem(t, i, msg + 56, &(n as u32).to_le_bytes()) {
                    return Fim::Valor(if enviados > 0 { enviados } else { -e });
                }
                enviados += 1;
            }
            Fim::Valor(e) => return Fim::Valor(if enviados > 0 { enviados } else { e }),
            outro => return if enviados > 0 { Fim::Valor(enviados) } else { outro },
        }
    }
    Fim::Valor(enviados)
}
