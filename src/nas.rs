//! NAS do MXOS: um protocolo proprio sobre TCP (porta 4545) para mandar e
//! buscar arquivos entre o PC e a box. O cliente do PC esta em
//! ferramentas/nas/mxnas.py.
//!
//! Liga com `config nas_senha <senha>` (sem senha o servico fica
//! desligado). Cada conexao comeca com um desafio: o servidor manda
//! `MXNAS 1 <nonce em hex>` e o cliente responde `AUTH <hmac>` com o
//! HMAC-SHA256(senha, nonce) em hex; a senha nunca passa pela rede.
//!
//! Depois, comandos em linhas de texto (caminhos podem ter espacos; o
//! ultimo campo vai ate o fim da linha):
//!   LISTAR <pasta>             -> OK <n>, e n linhas "D|F <tamanho> <nome>"
//!   BAIXAR <arquivo>           -> OK <tamanho>, e os bytes
//!   ENVIAR <tamanho> <arquivo> -> (manda os bytes) -> OK
//!   PASTA <pasta>              -> OK      (cria)
//!   APAGAR <caminho>           -> OK
//!   RENOMEAR <de>\t<para>      -> OK
//!   TCHAU
//! Erro: `ERRO <mensagem>`. O download sai aos pedacos (o arquivo nao
//! precisa caber na memoria); o upload junta tudo antes de gravar.
//!
//! O laco principal chama `atender` a cada volta: nada aqui espera.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::UnsafeCell;

use crate::armazenamento::{self, Abertura};
use crate::cripto::{self, Hash};
use crate::rede::soquete;

pub const PORTA: u16 = 4545;
const MAX_CLIENTES: usize = 4;
const MAX_LINHA: usize = 4096;
const MAX_ENVIO: usize = 256 << 20;
/// Pedaco lido do disco por vez no download.
const PEDACO: usize = 32 * 1024;
/// Bytes na fila do TCP antes de ler mais do disco.
const FILA_TCP: usize = 128 * 1024;

struct Download {
    abertura: Abertura,
    pos: usize,
    tamanho: usize,
}

struct Upload {
    caminho: String,
    faltam: usize,
    dados: Vec<u8>,
}

struct Cliente {
    id: usize,
    nonce: [u8; 16],
    autenticado: bool,
    entrada: Vec<u8>,
    baixando: Option<Download>,
    enviando: Option<Upload>,
    fechar: bool,
}

struct Estado {
    ligado: bool,
    senha: Vec<u8>,
    clientes: Vec<Cliente>,
    tentou: bool,
}

struct Global(UnsafeCell<Estado>);
// So o laco principal mexe.
unsafe impl Sync for Global {}

static ESTADO: Global = Global(UnsafeCell::new(Estado { ligado: false, senha: Vec::new(), clientes: Vec::new(), tentou: false }));

fn estado() -> &'static mut Estado {
    unsafe { &mut *ESTADO.0.get() }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Liga o servico se houver senha na configuracao. Devolve uma linha de estado.
pub fn iniciar() -> String {
    let e = estado();
    e.tentou = true;
    let cfg = crate::config::Config::carregar();
    let Some(senha) = cfg.obter("nas_senha").map(str::trim).filter(|s| !s.is_empty()) else {
        return String::from("NAS desligado (defina a senha: config nas_senha <senha>)");
    };
    e.senha = senha.as_bytes().to_vec();
    if !e.ligado {
        if soquete::tcp_escutar(PORTA).is_err() {
            return format!("NAS: a porta {PORTA} não abriu");
        }
        e.ligado = true;
    }
    format!("NAS ligado na porta {PORTA}")
}

pub fn parar() {
    let e = estado();
    if e.ligado {
        soquete::tcp_parar(PORTA);
        for c in e.clientes.drain(..) {
            soquete::tcp_largar(c.id);
        }
        e.ligado = false;
    }
}

pub fn ligado() -> bool {
    estado().ligado
}

pub fn clientes() -> usize {
    estado().clientes.len()
}

/// Uma volta: aceita conexoes, le comandos e manda pedacos de download.
pub fn atender() {
    let e = estado();
    if !e.tentou {
        // na primeira volta do sistema: liga se a configuracao pedir
        let _ = iniciar();
    }
    if !e.ligado {
        return;
    }
    while let Some((id, _ip, _porta)) = soquete::tcp_aceitar(PORTA) {
        if e.clientes.len() >= MAX_CLIENTES {
            soquete::tcp_enviar(id, b"ERRO servidor cheio\n");
            soquete::tcp_fechar_envio(id);
            soquete::tcp_largar(id);
            continue;
        }
        let mut nonce = [0u8; 16];
        cripto::acaso::preencher(&mut nonce, crate::cpu::contador());
        soquete::tcp_enviar(id, format!("MXNAS 1 {}\n", hex(&nonce)).as_bytes());
        e.clientes.push(Cliente { id, nonce, autenticado: false, entrada: Vec::new(), baixando: None, enviando: None, fechar: false });
    }
    let senha = e.senha.clone();
    for c in &mut e.clientes {
        atender_cliente(c, &senha);
    }
    e.clientes.retain(|c| {
        if c.fechar {
            soquete::tcp_fechar_envio(c.id);
            soquete::tcp_largar(c.id);
        }
        !c.fechar
    });
}

fn responder(c: &Cliente, texto: &str) {
    soquete::tcp_enviar(c.id, texto.as_bytes());
    soquete::tcp_enviar(c.id, b"\n");
}

fn atender_cliente(c: &mut Cliente, senha: &[u8]) {
    let (dados, fim) = soquete::tcp_ler(c.id, 256 * 1024);
    if !dados.is_empty() {
        if let Some(up) = &mut c.enviando {
            // bytes de um ENVIAR (o que sobrar e o proximo comando)
            let n = up.faltam.min(dados.len());
            up.dados.extend_from_slice(&dados[..n]);
            up.faltam -= n;
            c.entrada.extend_from_slice(&dados[n..]);
            if up.faltam == 0 {
                let up = c.enviando.take().unwrap_or_else(|| unreachable!());
                match armazenamento::escrever(&up.caminho, &up.dados) {
                    Ok(()) => responder(c, "OK"),
                    Err(e) => responder(c, &format!("ERRO {}", e.descricao())),
                }
            }
        } else {
            c.entrada.extend_from_slice(&dados);
        }
    }
    // comandos inteiros (uma linha cada)
    while c.enviando.is_none() && c.baixando.is_none() {
        let Some(p) = c.entrada.iter().position(|&b| b == b'\n') else { break };
        let linha: Vec<u8> = c.entrada.drain(..=p).collect();
        let linha: String = String::from_utf8_lossy(&linha).trim_end_matches(['\r', '\n']).into();
        comando(c, &linha, senha);
        // o comeco de um upload pode ter vindo junto com a linha
        if let Some(up) = &mut c.enviando {
            let n = up.faltam.min(c.entrada.len());
            up.dados.extend(c.entrada.drain(..n));
            up.faltam -= n;
            if up.faltam == 0 {
                let up = c.enviando.take().unwrap_or_else(|| unreachable!());
                match armazenamento::escrever(&up.caminho, &up.dados) {
                    Ok(()) => responder(c, "OK"),
                    Err(e) => responder(c, &format!("ERRO {}", e.descricao())),
                }
            }
        }
        if c.fechar {
            return;
        }
    }
    if c.entrada.len() > MAX_LINHA && c.enviando.is_none() {
        responder(c, "ERRO linha comprida demais");
        c.fechar = true;
    }
    // download em andamento: um pedaco por volta, se a fila do TCP deixar
    if let Some(d) = &mut c.baixando {
        while soquete::tcp_por_enviar(c.id) < FILA_TCP && d.pos < d.tamanho {
            let n = PEDACO.min(d.tamanho - d.pos);
            let mut buf = alloc::vec![0u8; n];
            if armazenamento::ler_trecho(&d.abertura, d.pos, &mut buf).is_err() {
                c.fechar = true; // o cliente ve a conexao cair antes do fim
                return;
            }
            soquete::tcp_enviar(c.id, &buf);
            d.pos += n;
        }
        if d.pos >= d.tamanho {
            c.baixando = None;
        }
    }
    if fim && c.baixando.is_none() {
        c.fechar = true;
    }
}

fn comando(c: &mut Cliente, linha: &str, senha: &[u8]) {
    let (verbo, resto) = linha.split_once(' ').unwrap_or((linha, ""));
    if !c.autenticado {
        let esperado = hex(&Hash::Sha256.hmac(senha, &c.nonce));
        if verbo == "AUTH" && cripto::hmac::iguais(resto.trim().as_bytes(), esperado.as_bytes()) {
            c.autenticado = true;
            responder(c, "OK");
        } else {
            responder(c, "ERRO senha errada");
            c.fechar = true;
        }
        return;
    }
    let caminho = armazenamento_caminho(resto);
    match verbo {
        "LISTAR" => match armazenamento::listar(&caminho) {
            Ok(itens) => {
                let mut s = format!("OK {}\n", itens.len());
                for i in itens {
                    s.push_str(&format!("{} {} {}\n", if i.pasta { 'D' } else { 'F' }, i.tamanho, i.nome));
                }
                soquete::tcp_enviar(c.id, s.as_bytes());
            }
            Err(e) => responder(c, &format!("ERRO {}", e.descricao())),
        },
        "BAIXAR" => match armazenamento::abrir_leitura(&caminho) {
            Ok((tamanho, abertura)) => {
                responder(c, &format!("OK {tamanho}"));
                c.baixando = Some(Download { abertura, pos: 0, tamanho });
            }
            Err(e) => responder(c, &format!("ERRO {}", e.descricao())),
        },
        "ENVIAR" => {
            let (n, nome) = resto.split_once(' ').unwrap_or(("", ""));
            match n.parse::<usize>() {
                Ok(n) if n <= MAX_ENVIO && !nome.trim().is_empty() => {
                    let up = Upload { caminho: armazenamento_caminho(nome), faltam: n, dados: Vec::with_capacity(n.min(16 << 20)) };
                    if n == 0 {
                        match armazenamento::escrever(&up.caminho, &[]) {
                            Ok(()) => responder(c, "OK"),
                            Err(e) => responder(c, &format!("ERRO {}", e.descricao())),
                        }
                    } else {
                        c.enviando = Some(up);
                    }
                }
                _ => {
                    responder(c, "ERRO tamanho inválido");
                    c.fechar = true;
                }
            }
        }
        "PASTA" => match armazenamento::criar_pasta(&caminho) {
            Ok(()) => responder(c, "OK"),
            Err(e) => responder(c, &format!("ERRO {}", e.descricao())),
        },
        "APAGAR" => match armazenamento::apagar(&caminho) {
            Ok(()) => responder(c, "OK"),
            Err(e) => responder(c, &format!("ERRO {}", e.descricao())),
        },
        "RENOMEAR" => match resto.split_once('\t') {
            Some((de, para)) => match armazenamento::renomear(&armazenamento_caminho(de), &armazenamento_caminho(para)) {
                Ok(()) => responder(c, "OK"),
                Err(e) => responder(c, &format!("ERRO {}", e.descricao())),
            },
            None => responder(c, "ERRO uso: RENOMEAR de<TAB>para"),
        },
        "TCHAU" => {
            responder(c, "OK");
            c.fechar = true;
        }
        _ => responder(c, "ERRO comando desconhecido"),
    }
}

/// Caminho absoluto, com '/' (aceita '\' do Windows).
fn armazenamento_caminho(c: &str) -> String {
    let c = c.trim().replace('\\', "/");
    if c.starts_with('/') { c } else { format!("/{c}") }
}
