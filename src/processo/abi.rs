//! ABI dos apps do MXOS (pacotes .mxa): as chamadas de sistema que um app
//! nativo usa, iguais no ARMv7 (r7 = numero, r0-r5 = argumentos, r0 =
//! retorno) e no AArch64 (x8, x0-x5, x0). Pequena de proposito: memoria,
//! arquivos, tempo, janelas e eventos, som e HTTP(S). O kit de
//! desenvolvimento (sdk/) embrulha tudo isso.
//!
//! Erros voltam negativos (ERRO_*). Ponteiros sao do espaco do app; o
//! kernel le e escreve pelos quadros dele (memoria.rs).
//!
//! Janelas: o app desenha os pixels (XRGB 0x00RRGGBB) na memoria dele e
//! chama JANELA_MOSTRAR com o retangulo que mudou; o kernel copia para a
//! janela (gui/apps/janela_app.rs) e o compositor apresenta. Os eventos
//! (teclas, mouse, fechar) entram numa fila por janela.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use super::memoria::{PAGINA, Permissao};
use super::{Estado, Tabela};
use crate::cpu;

pub const SAIR: u64 = 0;
pub const LOG: u64 = 1;
pub const TEMPO_US: u64 = 2;
pub const DORMIR_MS: u64 = 3;
pub const MAPEAR: u64 = 4;
pub const DESMAPEAR: u64 = 5;
pub const ABRIR: u64 = 6;
pub const LER: u64 = 7;
pub const ESCREVER: u64 = 8;
pub const FECHAR: u64 = 9;
pub const TAMANHO: u64 = 10;
pub const POSICIONAR: u64 = 11;
pub const LISTAR: u64 = 12;
pub const JANELA_CRIAR: u64 = 13;
pub const JANELA_MOSTRAR: u64 = 14;
pub const JANELA_EVENTO: u64 = 15;
pub const ESPERAR_EVENTO: u64 = 16;
pub const JANELA_TITULO: u64 = 17;
pub const JANELA_FECHAR: u64 = 18;
pub const HTTP_PEDIR: u64 = 19;
pub const HTTP_ESTADO: u64 = 20;
pub const HTTP_LER: u64 = 21;
pub const SOM: u64 = 22;
pub const ARGUMENTOS: u64 = 23;
pub const TELA: u64 = 24;

pub const ERRO_INVALIDO: i64 = -1;
pub const ERRO_NAO_EXISTE: i64 = -2;
pub const ERRO_MEMORIA: i64 = -3;
pub const ERRO_ENDERECO: i64 = -4;
pub const ERRO_DISCO: i64 = -5;
pub const ERRO_REDE: i64 = -6;
pub const ERRO_DESCONHECIDA: i64 = -38;

/// Onde o MAPEAR procura espaco livre (acima do programa e do brk).
const BASE_MAPEAR: usize = 0x0200_0000;
/// Maior arquivo lido de uma vez pelo ABRIR (o resto, pelo POSICIONAR).
const MAX_LEITURA: usize = 64 << 20;

/// Um arquivo aberto pelo app. Leitura: o arquivo inteiro na memoria do
/// kernel (os do app sao pequenos). Escrita: junta e grava no FECHAR.
struct Arquivo {
    caminho: String,
    dados: Vec<u8>,
    pos: usize,
    escrita: bool,
}

/// Um pedido HTTP(S) andando em segundo plano.
struct PedidoHttp {
    resposta: Option<Result<(u32, Vec<u8>), &'static str>>,
    url: String,
    lido: usize,
}

/// O que a ABI guarda de cada app.
#[derive(Default)]
pub struct EstadoApp {
    arquivos: BTreeMap<u32, Arquivo>,
    proximo_fd: u32,
    http: BTreeMap<u32, PedidoHttp>,
    proximo_http: u32,
    /// Janelas deste app (ids do gui/apps/janela_app.rs).
    pub janelas: Vec<u32>,
    /// Argumentos que o instalador passou (texto).
    pub argumentos: Vec<u8>,
    /// Prazo do ESPERAR_EVENTO em andamento (contador).
    prazo: Option<u64>,
}

fn agora_us() -> u64 {
    cpu::contador() * 1_000_000 / cpu::frequencia_timer().max(1)
}

/// Le `n` bytes do app (no maximo `limite`).
fn ler(t: &Tabela, i: usize, ptr: u64, n: u64, limite: usize) -> Result<Vec<u8>, i64> {
    let n = n as usize;
    if n > limite {
        return Err(ERRO_INVALIDO);
    }
    let mut v = vec![0u8; n];
    t.processos[i].espaco.ler(ptr as usize, &mut v).map_err(|_| ERRO_ENDERECO)?;
    Ok(v)
}

fn texto(t: &Tabela, i: usize, ptr: u64, n: u64) -> Result<String, i64> {
    String::from_utf8(ler(t, i, ptr, n, 4096)?).map_err(|_| ERRO_INVALIDO)
}

fn escrever(t: &mut Tabela, i: usize, ptr: u64, dados: &[u8]) -> Result<(), i64> {
    t.processos[i].espaco.escrever(ptr as usize, dados).map_err(|_| ERRO_ENDERECO)
}

/// Atende a chamada do app `i`. true = algo mudou que o terminal precisa ver.
pub fn atender(t: &mut Tabela, i: usize) -> bool {
    let (numero, a) = t.processos[i].contexto.chamada();
    let r = despachar(t, i, numero, a);
    let Some(p) = t.processos.get_mut(i) else { return true };
    match r {
        Fim::Valor(v) => p.contexto.definir_retorno(v as u64),
        Fim::Valor64(v) => p.contexto.definir_retorno64(v),
        Fim::Esperar(motivo) => {
            p.contexto.repetir_chamada();
            if p.estado == Estado::Pronto {
                p.estado = Estado::Esperando(motivo);
            }
        }
        Fim::Nada => return true,
    }
    numero == LOG
}

enum Fim {
    Valor(i64),
    Valor64(u64),
    Esperar(&'static str),
    Nada,
}

fn despachar(t: &mut Tabela, i: usize, numero: u64, a: [u64; 6]) -> Fim {
    let r: Result<i64, i64> = match numero {
        SAIR => {
            fechar_tudo(t, i);
            t.terminar(i, (a[0] as i32 & 0xFF) << 8);
            return Fim::Nada;
        }
        LOG => match ler(t, i, a[0], a[1], 64 * 1024) {
            Ok(b) => {
                t.escrever(&b);
                Ok(b.len() as i64)
            }
            Err(e) => Err(e),
        },
        TEMPO_US => return Fim::Valor64(agora_us()),
        DORMIR_MS => {
            let alvo = cpu::contador() + a[0] * cpu::frequencia_timer() / 1000;
            t.processos[i].estado = Estado::Dormindo(alvo);
            Ok(0)
        }
        MAPEAR => mapear(t, i, a[0] as usize),
        DESMAPEAR => {
            let (inicio, n) = (a[0] as usize, (a[1] as usize).div_ceil(PAGINA));
            for k in 0..n {
                t.processos[i].espaco.desmapear(inicio + k * PAGINA);
            }
            Ok(0)
        }
        ABRIR => abrir(t, i, a[0], a[1], a[2]),
        LER => {
            let fd = a[0] as u32;
            let Some(arq) = t.processos[i].app.arquivos.get_mut(&fd) else { return Fim::Valor(ERRO_INVALIDO) };
            let n = (a[2] as usize).min(arq.dados.len().saturating_sub(arq.pos));
            let pedaco = arq.dados[arq.pos..arq.pos + n].to_vec();
            arq.pos += n;
            escrever(t, i, a[1], &pedaco).map(|_| n as i64)
        }
        ESCREVER => match ler(t, i, a[1], a[2], 16 << 20) {
            Ok(b) => {
                let fd = a[0] as u32;
                let Some(arq) = t.processos[i].app.arquivos.get_mut(&fd) else { return Fim::Valor(ERRO_INVALIDO) };
                if !arq.escrita {
                    return Fim::Valor(ERRO_INVALIDO);
                }
                let fim = arq.pos + b.len();
                if arq.dados.len() < fim {
                    arq.dados.resize(fim, 0);
                }
                arq.dados[arq.pos..fim].copy_from_slice(&b);
                arq.pos = fim;
                Ok(b.len() as i64)
            }
            Err(e) => Err(e),
        },
        FECHAR => fechar_arquivo(t, i, a[0] as u32),
        TAMANHO => match t.processos[i].app.arquivos.get(&(a[0] as u32)) {
            Some(arq) => return Fim::Valor64(arq.dados.len() as u64),
            None => Err(ERRO_INVALIDO),
        },
        POSICIONAR => match t.processos[i].app.arquivos.get_mut(&(a[0] as u32)) {
            Some(arq) => {
                arq.pos = (a[1] | a[2] << 32) as usize;
                Ok(0)
            }
            None => Err(ERRO_INVALIDO),
        },
        LISTAR => listar(t, i, a),
        JANELA_CRIAR => janela_criar(t, i, a),
        JANELA_MOSTRAR => janela_mostrar(t, i, a),
        JANELA_EVENTO => {
            let id = a[0] as u32;
            if !t.processos[i].app.janelas.contains(&id) {
                return Fim::Valor(ERRO_INVALIDO);
            }
            match crate::gui::apps::janela_app::tirar_evento(id) {
                Some(e) => {
                    let mut b = [0u8; 16];
                    for (k, v) in e.iter().enumerate() {
                        b[k * 4..k * 4 + 4].copy_from_slice(&v.to_le_bytes());
                    }
                    escrever(t, i, a[1], &b).map(|_| 1)
                }
                None => Ok(0),
            }
        }
        ESPERAR_EVENTO => {
            let janelas = t.processos[i].app.janelas.clone();
            let tem = janelas.iter().any(|&id| crate::gui::apps::janela_app::tem_evento(id));
            let agora = cpu::contador();
            let prazo = *t.processos[i].app.prazo.get_or_insert(agora + a[0] * cpu::frequencia_timer() / 1000);
            if tem || agora >= prazo || janelas.is_empty() && a[0] == 0 {
                t.processos[i].app.prazo = None;
                Ok(tem as i64)
            } else {
                return Fim::Esperar("esperando eventos");
            }
        }
        JANELA_TITULO => match texto(t, i, a[1], a[2]) {
            Ok(s) => {
                crate::gui::apps::janela_app::titulo(a[0] as u32, &s);
                Ok(0)
            }
            Err(e) => Err(e),
        },
        JANELA_FECHAR => {
            let id = a[0] as u32;
            t.processos[i].app.janelas.retain(|&j| j != id);
            crate::gui::apps::janela_app::fechar(id);
            Ok(0)
        }
        HTTP_PEDIR => http_pedir(t, i, a),
        HTTP_ESTADO => match t.processos[i].app.http.get(&(a[0] as u32)) {
            None => Err(ERRO_INVALIDO),
            Some(p) => match &p.resposta {
                None => Ok(0),
                Some(Ok((status, corpo))) => {
                    // status nos 16 bits de baixo, tamanho em cima (ate 2 GB)
                    let _ = status;
                    Ok(corpo.len() as i64)
                }
                Some(Err(_)) => Err(ERRO_REDE),
            },
        },
        HTTP_LER => {
            let id = a[0] as u32;
            let Some(p) = t.processos[i].app.http.get_mut(&id) else { return Fim::Valor(ERRO_INVALIDO) };
            let Some(Ok((_, corpo))) = &p.resposta else { return Fim::Valor(ERRO_INVALIDO) };
            let n = (a[2] as usize).min(corpo.len() - p.lido);
            let pedaco = corpo[p.lido..p.lido + n].to_vec();
            p.lido += n;
            if p.lido >= corpo.len() {
                t.processos[i].app.http.remove(&id);
            }
            escrever(t, i, a[1], &pedaco).map(|_| n as i64)
        }
        SOM => match ler(t, i, a[0], a[1], 1 << 20) {
            Ok(b) => {
                crate::audio::dsp::abrir();
                Ok(crate::audio::dsp::escrever(&b) as i64)
            }
            Err(e) => Err(e),
        },
        ARGUMENTOS => {
            let args = t.processos[i].app.argumentos.clone();
            let n = (a[1] as usize).min(args.len());
            escrever(t, i, a[0], &args[..n]).map(|_| args.len() as i64)
        }
        TELA => {
            let (l, h) = crate::gui::tema::tamanho_da_tela();
            return Fim::Valor64((l as u64) | (h as u64) << 32);
        }
        _ => Err(ERRO_DESCONHECIDA),
    };
    Fim::Valor(r.unwrap_or_else(|e| e))
}

fn mapear(t: &mut Tabela, i: usize, tamanho: usize) -> Result<i64, i64> {
    if tamanho == 0 || tamanho > 512 << 20 {
        return Err(ERRO_INVALIDO);
    }
    let tamanho = tamanho.next_multiple_of(PAGINA);
    let e = &mut t.processos[i].espaco;
    let inicio = e.achar_livre(BASE_MAPEAR, tamanho).ok_or(ERRO_MEMORIA)?;
    e.mapear_faixa(inicio, inicio + tamanho, Permissao::Dados).map_err(|_| ERRO_MEMORIA)?;
    Ok(inicio as i64)
}

/// Caminho do app: absoluto ("/documentos/x") ou relativo a pasta dele.
fn caminho(t: &Tabela, i: usize, s: &str) -> String {
    if s.starts_with('/') {
        return String::from(s);
    }
    let base = t.processos[i].exe.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
    alloc::format!("{base}/{s}")
}

/// modo: 0 = ler, 1 = escrever (cria ou zera), 2 = acrescentar.
fn abrir(t: &mut Tabela, i: usize, ptr: u64, n: u64, modo: u64) -> Result<i64, i64> {
    let nome = texto(t, i, ptr, n)?;
    let c = caminho(t, i, &nome);
    let dados = match modo {
        0 | 2 => match crate::armazenamento::ler(&c) {
            Ok(d) if d.len() <= MAX_LEITURA => d,
            Ok(_) => return Err(ERRO_MEMORIA),
            Err(_) if modo == 2 => Vec::new(),
            Err(_) => return Err(ERRO_NAO_EXISTE),
        },
        1 => Vec::new(),
        _ => return Err(ERRO_INVALIDO),
    };
    let app = &mut t.processos[i].app;
    app.proximo_fd += 1;
    let fd = app.proximo_fd;
    let pos = if modo == 2 { dados.len() } else { 0 };
    app.arquivos.insert(fd, Arquivo { caminho: c, dados, pos, escrita: modo != 0 });
    Ok(fd as i64)
}

fn fechar_arquivo(t: &mut Tabela, i: usize, fd: u32) -> Result<i64, i64> {
    let arq = t.processos[i].app.arquivos.remove(&fd).ok_or(ERRO_INVALIDO)?;
    if arq.escrita {
        crate::armazenamento::escrever(&arq.caminho, &arq.dados).map_err(|_| ERRO_DISCO)?;
    }
    Ok(0)
}

/// Fecha arquivos (gravando os de escrita) e janelas do app que saiu.
pub fn fechar_tudo(t: &mut Tabela, i: usize) {
    let fds: Vec<u32> = t.processos[i].app.arquivos.keys().copied().collect();
    for fd in fds {
        let _ = fechar_arquivo(t, i, fd);
    }
    for id in core::mem::take(&mut t.processos[i].app.janelas) {
        crate::gui::apps::janela_app::fechar(id);
    }
}

/// LISTAR(caminho, n, buf, buflen): linhas "D nome" ou "F tamanho nome".
fn listar(t: &mut Tabela, i: usize, a: [u64; 6]) -> Result<i64, i64> {
    let nome = texto(t, i, a[0], a[1])?;
    let c = caminho(t, i, &nome);
    let itens = crate::armazenamento::listar(&c).map_err(|_| ERRO_NAO_EXISTE)?;
    let mut s = String::new();
    for it in itens {
        if it.pasta {
            s += &alloc::format!("D {}\n", it.nome);
        } else {
            s += &alloc::format!("F {} {}\n", it.tamanho, it.nome);
        }
    }
    let n = (a[3] as usize).min(s.len());
    escrever(t, i, a[2], &s.as_bytes()[..n])?;
    Ok(s.len() as i64)
}

/// JANELA_CRIAR(largura, altura, titulo, n) -> id.
fn janela_criar(t: &mut Tabela, i: usize, a: [u64; 6]) -> Result<i64, i64> {
    let (l, h) = (a[0] as usize, a[1] as usize);
    if !(16..=4096).contains(&l) || !(16..=4096).contains(&h) {
        return Err(ERRO_INVALIDO);
    }
    let titulo = texto(t, i, a[2], a[3])?;
    let pid = t.processos[i].pid;
    let id = crate::gui::apps::janela_app::criar(pid, l, h, &titulo);
    t.processos[i].app.janelas.push(id);
    Ok(id as i64)
}

/// JANELA_MOSTRAR(id, pixels, passo em bytes, x | y << 16, l | a << 16):
/// copia o retangulo da memoria do app para a janela.
fn janela_mostrar(t: &mut Tabela, i: usize, a: [u64; 6]) -> Result<i64, i64> {
    let id = a[0] as u32;
    if !t.processos[i].app.janelas.contains(&id) {
        return Err(ERRO_INVALIDO);
    }
    let (ptr, passo) = (a[1] as usize, a[2] as usize);
    let (x, y) = ((a[3] & 0xFFFF) as usize, (a[3] >> 16 & 0xFFFF) as usize);
    let (l, h) = ((a[4] & 0xFFFF) as usize, (a[4] >> 16 & 0xFFFF) as usize);
    let espaco = &t.processos[i].espaco;
    crate::gui::apps::janela_app::com_pixels(id, |pixels, largura, altura| {
        if x + l > largura || y + h > altura || passo < l * 4 {
            return Err(ERRO_INVALIDO);
        }
        let mut linha = vec![0u8; l * 4];
        for k in 0..h {
            espaco.ler(ptr + (y + k) * passo + x * 4, &mut linha).map_err(|_| ERRO_ENDERECO)?;
            let destino = &mut pixels[(y + k) * largura + x..(y + k) * largura + x + l];
            for (d, c) in destino.iter_mut().zip(linha.chunks_exact(4)) {
                *d = u32::from_le_bytes([c[0], c[1], c[2], c[3]]) & 0x00FF_FFFF;
            }
        }
        Ok(0)
    })
    .unwrap_or(Err(ERRO_INVALIDO))
}

/// HTTP_PEDIR(url, n) -> id. Por enquanto a busca acontece aqui mesmo
/// (rede::http, como o navegador); o app le o resultado com HTTP_ESTADO e
/// HTTP_LER.
fn http_pedir(t: &mut Tabela, i: usize, a: [u64; 6]) -> Result<i64, i64> {
    let url = texto(t, i, a[0], a[1])?;
    let resposta = crate::rede::http_get(&url, 30_000).map(|r| (r.status as u32, r.corpo)).map_err(|e| e.descricao());
    let app = &mut t.processos[i].app;
    app.proximo_http += 1;
    let id = app.proximo_http;
    app.http.insert(id, PedidoHttp { resposta: Some(resposta), url, lido: 0 });
    let _ = &app.http[&id].url;
    Ok(id as i64)
}
