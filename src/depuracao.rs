//! Depuracao remota (porta 4546): comandos do shell, a tela e voltar ao
//! Android pela rede, para testar a TV box sem ninguem do lado dela.
//! Cliente: ferramentas/depurar_box.py.
//!
//! So existe com a senha compilada no binario (variavel MXOS_DEPURACAO no
//! build; o arquivo dela fica fora do git). Sem ela o servico nem abre a
//! porta. A primeira linha da conexao e a senha; depois, uma por linha:
//!   INFO              -> frequencia, temperatura, memoria, rede
//!   SH <comando>      -> roda no shell e devolve a saida
//!   TELA              -> OK <largura> <altura>\n + XRGB8888 linha a linha
//!   ABRIR <url>       -> abre o navegador nessa pagina (ou o visualizador,
//!                        se for um arquivo de midia do disco)
//!   ANDROID           -> reinicia no Android (TV box)
//!   TCHAU
//! Toda resposta de texto termina com uma linha "." sozinha.
//!
//! O laco principal chama `atender` a cada volta: nada aqui espera.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::UnsafeCell;

use crate::rede::soquete;

pub const PORTA: u16 = 4546;
const SENHA: Option<&str> = option_env!("MXOS_DEPURACAO");
const MAX_LINHA: usize = 4096;
/// Bytes na fila do TCP antes de mandar mais da tela.
const FILA_TCP: usize = 256 * 1024;

struct Cliente {
    id: usize,
    autenticado: bool,
    entrada: Vec<u8>,
    /// O que falta mandar (a tela e grande: sai aos pedacos).
    saida: Vec<u8>,
    enviado: usize,
    fechar: bool,
}

struct Estado {
    ligado: bool,
    tentou: bool,
    clientes: Vec<Cliente>,
    /// Paginas para o laco da GUI abrir.
    abrir: Vec<String>,
    shell: Option<crate::shell::Shell>,
    /// Eventos de mouse para o laco da GUI (MOUSE, BOTAO).
    eventos: Vec<crate::entrada::Evento>,
}

struct Global(UnsafeCell<Estado>);
// So o laco principal mexe.
unsafe impl Sync for Global {}

static ESTADO: Global = Global(UnsafeCell::new(Estado { ligado: false, tentou: false, clientes: Vec::new(), abrir: Vec::new(),
    eventos: Vec::new(), shell: None }));

fn estado() -> &'static mut Estado {
    unsafe { &mut *ESTADO.0.get() }
}

/// Paginas pedidas por ABRIR (o laco da GUI abre uma janela para cada).
pub fn paginas_para_abrir() -> Vec<String> {
    core::mem::take(&mut estado().abrir)
}

/// Uma volta: aceita conexoes, le comandos e manda o que estiver na fila.
pub fn atender() {
    let Some(senha) = SENHA else { return };
    let e = estado();
    if !e.tentou {
        e.tentou = true;
        e.ligado = soquete::tcp_escutar(PORTA).is_ok();
    }
    if !e.ligado {
        return;
    }
    while let Some((id, _ip, _porta)) = soquete::tcp_aceitar(PORTA) {
        e.clientes.push(Cliente { id, autenticado: false, entrada: Vec::new(), saida: Vec::new(), enviado: 0, fechar: false });
    }
    let mut clientes = core::mem::take(&mut e.clientes);
    for c in &mut clientes {
        atender_cliente(c, senha);
    }
    clientes.retain(|c| {
        if c.fechar && c.enviado >= c.saida.len() {
            soquete::tcp_fechar_envio(c.id);
            soquete::tcp_largar(c.id);
            return false;
        }
        true
    });
    estado().clientes = clientes;
}

fn atender_cliente(c: &mut Cliente, senha: &str) {
    // manda o que falta, sem encher a fila do TCP
    while c.enviado < c.saida.len() && soquete::tcp_por_enviar(c.id) < FILA_TCP {
        let fim = (c.enviado + 16 * 1024).min(c.saida.len());
        if !soquete::tcp_enviar(c.id, &c.saida[c.enviado..fim]) {
            break;
        }
        c.enviado = fim;
    }
    if c.enviado >= c.saida.len() && !c.saida.is_empty() {
        c.saida.clear();
        c.enviado = 0;
    }
    if !c.saida.is_empty() {
        return; // um comando por vez
    }
    let (dados, fechou) = soquete::tcp_ler(c.id, 64 * 1024);
    c.entrada.extend_from_slice(&dados);
    if fechou {
        c.fechar = true;
    }
    while let Some(fim) = c.entrada.iter().position(|&b| b == b'\n') {
        let linha: Vec<u8> = c.entrada.drain(..=fim).collect();
        let linha = String::from(String::from_utf8_lossy(&linha).trim());
        if !c.autenticado {
            if linha == senha {
                c.autenticado = true;
                c.saida.extend_from_slice(b"MXOS depuracao 1\n.\n");
            } else {
                c.fechar = true;
            }
            return;
        }
        comando(c, &linha);
        if !c.saida.is_empty() {
            return;
        }
    }
    if c.entrada.len() > MAX_LINHA {
        c.fechar = true;
    }
}

fn texto(c: &mut Cliente, s: &str) {
    c.saida.extend_from_slice(s.as_bytes());
    if !s.ends_with('\n') {
        c.saida.push(b'\n');
    }
    c.saida.extend_from_slice(b".\n");
}

fn comando(c: &mut Cliente, linha: &str) {
    let (cmd, resto) = linha.split_once(' ').unwrap_or((linha, ""));
    match cmd {
        "INFO" => {
            let h = crate::heap::estatisticas();
            let ip = crate::rede::info().map(|i| format!("{}.{}.{}.{}", i.ip[0], i.ip[1], i.ip[2], i.ip[3])).unwrap_or_else(|| String::from("sem rede"));
            #[cfg(feature = "rk322x")]
            let rga = crate::video::rga::versao();
            #[cfg(not(feature = "rk322x"))]
            let rga = 0u32;
            let s = crate::gui::relatorio_desempenho() + "\n" + &format!(
                "cpu {}\nrga {rga:08x}\nheap {} MB livres de {} MB\nligado ha {} s\nrede {ip}\n",
                crate::frequencia::descricao(),
                h.livre >> 20,
                h.total >> 20,
                crate::cpu::contador() / crate::cpu::frequencia_timer().max(1)
            );
            #[cfg(feature = "rk322x")]
            let s = {
                use core::sync::atomic::Ordering::Relaxed;
                use crate::gui::apps::filme as f;
                s + &format!("video {} ms, {} quadros, {} esperas
", f::POSICAO_MS.load(Relaxed), f::MOSTRADOS.load(Relaxed), f::ESPERAS.load(Relaxed)) + &crate::audio::hdmi::relatorio() + "
"
            };
            texto(c, &s);
        }
        "SH" => {
            let e = estado();
            let shell = e.shell.get_or_insert_with(crate::shell::Shell::remoto);
            shell.rodar_linha(resto);
            let saida = shell.terminal().tirar_captura();
            texto(c, &saida);
        }
        "TELA" => match crate::excecao::tela() {
            Some(t) if t.formato() == crate::video::tela::Formato::Xrgb8888 => {
                let (l, a) = (t.largura(), t.altura());
                c.saida.reserve(l * a * 4 + 32);
                c.saida.extend_from_slice(format!("OK {l} {a}\n").as_bytes());
                for y in 0..a {
                    let linha = unsafe { core::slice::from_raw_parts((t.base() + y * t.passo()) as *const u8, l * 4) };
                    c.saida.extend_from_slice(linha);
                }
            }
            _ => texto(c, "ERRO sem tela XRGB"),
        },
        "MOUSE" => {
            let mut p = resto.split_whitespace().filter_map(|v| v.parse::<i32>().ok());
            if let (Some(x), Some(y)) = (p.next(), p.next()) {
                estado().eventos.push(crate::entrada::Evento::Mouse { x, y });
            }
            texto(c, "OK");
        }
        "BOTAO" => {
            let pressionado = resto.trim() == "1";
            estado().eventos.push(crate::entrada::Evento::Botao { botao: crate::entrada::Botao::Esquerdo, pressionado });
            texto(c, "OK");
        }
        "ABRIR" => {
            estado().abrir.push(String::from(resto.trim()));
            texto(c, "OK");
        }
        "ANDROID" => {
            texto(c, "OK reiniciando no Android");
            // a marca "normal" do carregador e o reset quente pela CRU
            #[cfg(feature = "rk322x")]
            {
                let mut voltas = 0;
                while soquete::tcp_por_enviar(c.id) > 0 && voltas < 200 {
                    crate::rede::processar();
                    crate::timer::esperar_ms(5);
                    voltas += 1;
                }
                unsafe {
                    core::ptr::write_volatile(0x1100_05C8 as *mut u32, 0x5242_C300);
                    core::ptr::write_volatile(0x110E_01F0 as *mut u32, 0xFDB9);
                }
            }
        }
        "TCHAU" => {
            texto(c, "OK");
            c.fechar = true;
        }
        _ => texto(c, "ERRO comando desconhecido (INFO, SH, TELA, ABRIR, ANDROID, TCHAU)"),
    }
}

/// Eventos de mouse mandados pela depuracao, para o laco da GUI entregar.
pub fn eventos() -> Vec<crate::entrada::Evento> {
    core::mem::take(&mut estado().eventos)
}
