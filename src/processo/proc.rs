//! /proc feito na hora: o que o ps, o free, o df, o uptime e o top do
//! BusyBox leem. Nada disso esta no disco; cada open gera o texto.
//!
//!   /proc/meminfo, /proc/mounts, /proc/uptime, /proc/cpuinfo,
//!   /proc/version, /proc/loadavg, /proc/stat
//!   /proc/PID/stat, /proc/PID/status, /proc/PID/cmdline, /proc/PID/exe
//!   /proc/self = o processo que pergunta

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::arquivos::{Aberto, Descricao, ItemDePasta, Tipo, ENOENT};
use super::{Estado, Tabela};
use crate::{cpu, heap};

const ARQUIVOS: [&str; 7] = ["meminfo", "mounts", "uptime", "cpuinfo", "version", "loadavg", "stat"];
const DO_PROCESSO: [&str; 4] = ["stat", "status", "cmdline", "exe"];

/// "/proc/self/x" -> (Some(pid), "x"); "/proc/meminfo" -> (None, "meminfo").
fn separar(t: &Tabela, i: usize, caminho: &str) -> (Option<u32>, String) {
    let resto = caminho.trim_start_matches("/proc").trim_start_matches('/');
    let (primeiro, depois) = resto.split_once('/').unwrap_or((resto, ""));
    let pid = match primeiro {
        "self" | "thread-self" => Some(t.processos[i].pid),
        p => p.parse::<u32>().ok(),
    };
    match pid {
        Some(pid) => (Some(pid), String::from(depois)),
        None => (None, String::from(resto)),
    }
}

/// /proc/PID/exe e /proc/self/exe sao "links" para o programa.
pub fn link(t: &Tabela, i: usize, caminho: &str) -> Option<String> {
    if !caminho.starts_with("/proc/") {
        return None;
    }
    match separar(t, i, caminho) {
        (Some(pid), resto) if resto == "exe" => t.indice(pid).map(|j| t.processos[j].exe.clone()),
        _ => None,
    }
}

/// (pasta, tamanho) para o stat.
pub fn info(t: &Tabela, i: usize, caminho: &str) -> Result<(bool, u64), i64> {
    let (pid, resto) = separar(t, i, caminho);
    match pid {
        None if resto.is_empty() => Ok((true, 0)),
        None if ARQUIVOS.contains(&resto.as_str()) => Ok((false, 0)),
        Some(pid) if t.indice(pid).is_some() => match resto.as_str() {
            "" => Ok((true, 0)),
            r if DO_PROCESSO.contains(&r) => Ok((false, 0)),
            _ => Err(ENOENT),
        },
        _ => Err(ENOENT),
    }
}

pub fn abrir(t: &Tabela, i: usize, caminho: &str) -> Result<Aberto, i64> {
    let (pid, resto) = separar(t, i, caminho);
    let tipo = match pid {
        None if resto.is_empty() => {
            let mut itens: Vec<ItemDePasta> = [".", ".."].iter().map(|n| ItemDePasta { nome: (*n).into(), pasta: true }).collect();
            itens.extend(t.processos.iter().map(|p| ItemDePasta { nome: format!("{}", p.pid), pasta: true }));
            itens.push(ItemDePasta { nome: "self".into(), pasta: true });
            itens.extend(ARQUIVOS.iter().map(|n| ItemDePasta { nome: (*n).into(), pasta: false }));
            Tipo::Pasta { caminho: String::from("/proc"), itens }
        }
        None => Tipo::Gerado(geral(t, &resto).ok_or(ENOENT)?.into_bytes()),
        Some(pid) => {
            let j = t.indice(pid).ok_or(ENOENT)?;
            if resto.is_empty() {
                let mut itens: Vec<ItemDePasta> = [".", ".."].iter().map(|n| ItemDePasta { nome: (*n).into(), pasta: true }).collect();
                itens.extend(DO_PROCESSO.iter().map(|n| ItemDePasta { nome: (*n).into(), pasta: false }));
                Tipo::Pasta { caminho: format!("/proc/{pid}"), itens }
            } else {
                Tipo::Gerado(do_processo(t, j, &resto).ok_or(ENOENT)?)
            }
        }
    };
    Ok(Descricao::nova(tipo, 0))
}

fn segundos() -> (u64, u64) {
    let freq = cpu::frequencia_timer().max(1);
    let c = cpu::contador();
    (c / freq, c % freq * 100 / freq)
}

/// Os arquivos de /proc que nao sao de um processo.
fn geral(t: &Tabela, nome: &str) -> Option<String> {
    let h = heap::estatisticas();
    let (s, cs) = segundos();
    Some(match nome {
        "meminfo" => format!(
            "MemTotal:       {:>8} kB\nMemFree:        {:>8} kB\nMemAvailable:   {:>8} kB\nBuffers:               0 kB\nCached:                0 kB\nSwapTotal:             0 kB\nSwapFree:              0 kB\n",
            h.total / 1024,
            h.livre / 1024,
            h.livre / 1024
        ),
        "mounts" => String::from("/dev/vda / vfat rw 0 0\nproc /proc proc rw 0 0\n"),
        "uptime" => {
            let c = crate::irq::contadores();
            let ocioso = c.ocioso / cpu::frequencia_timer().max(1);
            format!("{s}.{cs:02} {ocioso}.00\n")
        }
        "cpuinfo" => format!(
            "processor\t: 0\nBogoMIPS\t: {}.00\nFeatures\t: fp asimd\nCPU implementer\t: 0x{:02x}\nCPU architecture: 8\nCPU part\t: 0x{:03x}\nCPU revision\t: {}\nmodel name\t: {}\n\n",
            cpu::frequencia_timer() * 2 / 1_000_000,
            cpu::midr() >> 24 & 0xFF,
            cpu::midr() >> 4 & 0xFFF,
            cpu::midr() & 0xF,
            cpu::nome_cpu(cpu::midr())
        ),
        "version" => String::from("Linux version 6.1.0-mxos (MXOS 0.9, kernel ARM64 escrito do zero em Rust)\n"),
        "loadavg" => format!("0.00 0.00 0.00 1/{} {}\n", t.processos.len(), t.processos.last().map_or(0, |p| p.pid)),
        "stat" => {
            let c = crate::irq::contadores();
            let freq = cpu::frequencia_timer().max(1);
            let total = c.desde_inicio * 100 / freq;
            let ocioso = c.ocioso * 100 / freq;
            format!("cpu  {} 0 0 {ocioso} 0 0 0 0 0 0\ncpu0 {} 0 0 {ocioso} 0 0 0 0 0 0\nbtime {}\n", total - ocioso, total - ocioso, super::rtc_no_boot())
        }
        _ => return None,
    })
}

/// /proc/PID/stat, status e cmdline.
fn do_processo(t: &Tabela, j: usize, nome: &str) -> Option<Vec<u8>> {
    let p = &t.processos[j];
    let freq = cpu::frequencia_timer().max(1);
    let estado = match p.estado {
        Estado::Pronto => 'R',
        Estado::Terminado(_) => 'Z',
        _ => 'S',
    };
    let memoria = p.espaco.bytes();
    Some(match nome {
        "cmdline" => p.cmdline.clone(),
        "stat" => format!(
            "{} ({}) {estado} {} {} {} 0 -1 0 0 0 0 0 {} 0 0 0 20 0 1 0 {} {} {} 0\n",
            p.pid,
            p.nome,
            p.pai,
            p.linux.grupo,
            p.linux.grupo,
            p.tempo_cpu * 100 / freq,
            p.inicio * 100 / freq,
            memoria,
            memoria / 4096
        )
        .into_bytes(),
        "status" => format!(
            "Name:\t{}\nState:\t{estado}\nPid:\t{}\nPPid:\t{}\nUid:\t0\t0\t0\t0\nGid:\t0\t0\t0\t0\nVmSize:\t{} kB\nVmRSS:\t{} kB\nThreads:\t1\n",
            p.nome,
            p.pid,
            p.pai,
            memoria / 1024,
            memoria / 1024
        )
        .into_bytes(),
        _ => return None,
    })
}
