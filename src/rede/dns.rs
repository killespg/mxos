//! DNS (RFC 1035): uma pergunta do tipo A por UDP para o servidor do DHCP.
//!   cabecalho de 12 bytes: id, flags, perguntas, respostas, autoridade, extras
//!   pergunta: nome em rotulos ("3www7exemplo3com0"), tipo 1 (A), classe 1 (IN)
//!
//! Na resposta os nomes podem vir comprimidos (ponteiro de 2 bytes com os
//! dois bits altos ligados); como so queremos o primeiro registro A, basta
//! saber pular nomes. Um cache pequeno evita perguntar de novo.

use alloc::string::String;
use alloc::vec::Vec;

use super::{ErroRede, agora_ms, com, esperar, ip4, ler16, ler_ip};

const PORTA_SERVIDOR: u16 = 53;
const TIPO_A: u16 = 1;
const CLASSE_IN: u16 = 1;
/// Flags da pergunta: so "recursao desejada".
const RECURSAO: u16 = 0x0100;
const RESPOSTA: u16 = 0x8000;
const NOME_INEXISTENTE: u16 = 3;

const TEMPO_TOTAL_MS: u64 = 3000;
const REENVIO_MS: u64 = 1000;
const MAX_CACHE: usize = 16;

/// Monta a pergunta. None se o nome nao for valido.
fn montar(id: u16, nome: &str) -> Option<Vec<u8>> {
    let nome = nome.trim_end_matches('.');
    if nome.is_empty() || nome.len() > 253 {
        return None;
    }
    let mut m = Vec::with_capacity(18 + nome.len());
    m.extend_from_slice(&id.to_be_bytes());
    m.extend_from_slice(&RECURSAO.to_be_bytes());
    m.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0]); // 1 pergunta
    for rotulo in nome.split('.') {
        let valido = rotulo.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_');
        if rotulo.is_empty() || rotulo.len() > 63 || !valido {
            return None;
        }
        m.push(rotulo.len() as u8);
        m.extend_from_slice(rotulo.as_bytes());
    }
    m.push(0);
    m.extend_from_slice(&TIPO_A.to_be_bytes());
    m.extend_from_slice(&CLASSE_IN.to_be_bytes());
    Some(m)
}

/// Posicao logo depois do nome que comeca em `i`.
fn pular_nome(dados: &[u8], mut i: usize) -> Option<usize> {
    loop {
        let tamanho = *dados.get(i)? as usize;
        match tamanho {
            0 => return Some(i + 1),
            t if t & 0xC0 == 0xC0 => return Some(i + 2), // ponteiro
            t => i += 1 + t,
        }
    }
}

/// Primeiro registro A da resposta. None = nao e resposta a esta pergunta.
fn ler_resposta(dados: &[u8], id: u16) -> Option<Result<[u8; 4], ErroRede>> {
    if dados.len() < 12 || ler16(dados, 0) != id || ler16(dados, 2) & RESPOSTA == 0 {
        return None;
    }
    let codigo = ler16(dados, 2) & 0xF;
    if codigo == NOME_INEXISTENTE {
        return Some(Err(ErroRede::NaoResolvido));
    }
    if codigo != 0 {
        return Some(Err(ErroRede::Protocolo("o servidor DNS devolveu erro")));
    }
    Some(primeiro_a(dados).unwrap_or(Err(ErroRede::Protocolo("resposta DNS inválida"))))
}

/// Pula as perguntas e procura um registro A nas respostas.
/// None = pacote truncado ou mal formado.
fn primeiro_a(dados: &[u8]) -> Option<Result<[u8; 4], ErroRede>> {
    let perguntas = ler16(dados, 4);
    let respostas = ler16(dados, 6);
    let mut i = 12;
    for _ in 0..perguntas {
        i = pular_nome(dados, i)? + 4;
    }
    for _ in 0..respostas {
        i = pular_nome(dados, i)?;
        let fixo = dados.get(i..i + 10)?;
        let (tipo, classe, tamanho) = (ler16(fixo, 0), ler16(fixo, 2), ler16(fixo, 8) as usize);
        let valor = dados.get(i + 10..i + 10 + tamanho)?;
        if tipo == TIPO_A && classe == CLASSE_IN && tamanho == 4 {
            return Some(Ok(ip4(valor, 0)));
        }
        i += 10 + tamanho;
    }
    Some(Err(ErroRede::NaoResolvido))
}

/// IP de `nome`: IP com pontos, cache ou pergunta ao servidor DNS.
pub fn resolver(nome: &str) -> Result<[u8; 4], ErroRede> {
    let nome = nome.trim();
    if let Some(ip) = ler_ip(nome) {
        return Ok(ip);
    }
    let minusculo: String = nome.to_ascii_lowercase();
    if let Some(ip) = com(|p| p.cache_dns.iter().find(|(n, _)| *n == minusculo).map(|&(_, ip)| ip))? {
        return Ok(ip);
    }
    let id = (agora_ms() as u16) ^ 0x4d58;
    let consulta = montar(id, &minusculo).ok_or(ErroRede::NaoResolvido)?;
    let porta = 0xC000 | (id & 0x3FFF);
    let servidor = com(|p| {
        p.udp.abrir(porta);
        p.enviar_udp(p.dns, porta, PORTA_SERVIDOR, &consulta);
        p.dns
    })?;
    if servidor == [0; 4] {
        let _ = com(|p| p.udp.fechar(porta));
        return Err(ErroRede::NaoResolvido);
    }
    let mut ultimo_envio = agora_ms();
    let resultado = esperar(TEMPO_TOTAL_MS, |p| {
        while let Some(d) = p.udp.tirar(porta) {
            if let Some(r) = ler_resposta(&d.dados, id) {
                return Some(r);
            }
        }
        if agora_ms().saturating_sub(ultimo_envio) >= REENVIO_MS {
            p.enviar_udp(servidor, porta, PORTA_SERVIDOR, &consulta);
            ultimo_envio = agora_ms();
        }
        None
    });
    let _ = com(|p| {
        p.udp.fechar(porta);
        if let Ok(Ok(ip)) = resultado {
            if p.cache_dns.len() == MAX_CACHE {
                p.cache_dns.remove(0);
            }
            p.cache_dns.push((minusculo, ip));
        }
    });
    resultado?
}
