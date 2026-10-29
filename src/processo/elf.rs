//! Carregador de ELF64 AArch64: le os cabecalhos, mapeia os segmentos
//! PT_LOAD com as permissoes certas, copia o conteudo (o BSS ja vem
//! zerado: as paginas novas sao zeradas) e monta a pilha inicial do jeito
//! que o Linux entrega a um programa novo:
//!
//!   sp -> argc
//!         argv[0..argc], NULL
//!         envp[..], NULL
//!         auxv: (AT_*, valor)..., (AT_NULL, 0)
//!         ... textos (argumentos, ambiente, "aarch64", 16 bytes aleatorios)
//!
//! Aceita ET_EXEC estatico e ET_DYN sem interpretador (PIE estatico, que
//! se reloca sozinho com o AT_PHDR e o AT_ENTRY). Programa dinamico (com
//! PT_INTERP) precisaria do ld-linux e das bibliotecas: recusado.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use super::memoria::{Espaco, PAGINA, Permissao};
use crate::cpu;

/// Onde um PIE estatico (ET_DYN) e carregado.
const BASE_PIE: usize = 0x40_0000;

const PT_LOAD: u32 = 1;
const PT_INTERP: u32 = 3;
const PT_PHDR: u32 = 6;
const PF_X: u32 = 1;
const PF_W: u32 = 2;

// Tipos do vetor auxiliar (auxv).
const AT_NULL: u64 = 0;
const AT_PHDR: u64 = 3;
const AT_PHENT: u64 = 4;
const AT_PHNUM: u64 = 5;
const AT_PAGESZ: u64 = 6;
const AT_BASE: u64 = 7;
const AT_FLAGS: u64 = 8;
const AT_ENTRY: u64 = 9;
const AT_UID: u64 = 11;
const AT_EUID: u64 = 12;
const AT_GID: u64 = 13;
const AT_EGID: u64 = 14;
const AT_PLATFORM: u64 = 15;
const AT_HWCAP: u64 = 16;
const AT_CLKTCK: u64 = 17;
const AT_SECURE: u64 = 23;
const AT_RANDOM: u64 = 25;
const AT_HWCAP2: u64 = 26;
const AT_EXECFN: u64 = 31;

/// O que o carregador achou e mapeou.
pub struct Carregado {
    pub entrada: usize,
    /// Onde os cabecalhos de programa ficaram na memoria (AT_PHDR).
    pub phdr: usize,
    pub phnum: usize,
    /// Fim do ultimo segmento: o brk comeca aqui (arredondado).
    pub fim: usize,
}

fn u16_em(d: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(i..i + 2)?.try_into().ok()?))
}

fn u32_em(d: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(i..i + 4)?.try_into().ok()?))
}

fn u64_em(d: &[u8], i: usize) -> Option<usize> {
    Some(u64::from_le_bytes(d.get(i..i + 8)?.try_into().ok()?) as usize)
}

struct Segmento {
    tipo: u32,
    flags: u32,
    offset: usize,
    vaddr: usize,
    filesz: usize,
    memsz: usize,
}

fn segmentos(d: &[u8]) -> Option<Vec<Segmento>> {
    let phoff = u64_em(d, 32)?;
    let phentsize = u16_em(d, 54)? as usize;
    let phnum = u16_em(d, 56)? as usize;
    if phentsize < 56 || phnum > 64 {
        return None;
    }
    (0..phnum)
        .map(|i| {
            let b = phoff.checked_add(i * phentsize)?;
            Some(Segmento {
                tipo: u32_em(d, b)?,
                flags: u32_em(d, b + 4)?,
                offset: u64_em(d, b + 8)?,
                vaddr: u64_em(d, b + 16)?,
                filesz: u64_em(d, b + 32)?,
                memsz: u64_em(d, b + 40)?,
            })
        })
        .collect()
}

/// E um ELF que o MXOS sabe rodar? (para o "executar" dar um erro claro)
pub fn conferir(d: &[u8]) -> Result<(), &'static str> {
    if d.len() < 64 || &d[..4] != b"\x7fELF" {
        return Err("não é um programa ELF");
    }
    if d[4] != 2 || d[5] != 1 {
        return Err("ELF que não é de 64 bits little endian");
    }
    if u16_em(d, 18) != Some(183) {
        return Err("programa de outra arquitetura (o MXOS roda AArch64)");
    }
    match u16_em(d, 16) {
        Some(2 | 3) => Ok(()),
        _ => Err("ELF que não é executável"),
    }
}

/// Mapeia o programa no espaco. Devolve onde ele comeca e o resto que a
/// pilha inicial precisa.
pub fn carregar(d: &[u8], espaco: &mut Espaco) -> Result<Carregado, &'static str> {
    conferir(d)?;
    let segmentos = segmentos(d).ok_or("cabeçalhos do ELF quebrados")?;
    if segmentos.iter().any(|s| s.tipo == PT_INTERP) {
        return Err("programa dinâmico (precisa do ld-linux): o MXOS só roda programas estáticos");
    }
    let carregaveis: Vec<&Segmento> = segmentos.iter().filter(|s| s.tipo == PT_LOAD && s.memsz > 0).collect();
    if carregaveis.is_empty() {
        return Err("ELF sem segmentos para carregar");
    }
    let menor = carregaveis.iter().map(|s| s.vaddr).min().unwrap_or(0) & !(PAGINA - 1);
    let base = if u16_em(d, 16) == Some(3) { BASE_PIE.wrapping_sub(menor) } else { 0 };

    // permissao de cada pagina: a soma dos segmentos que passam por ela
    let mut paginas: BTreeMap<usize, u32> = BTreeMap::new();
    for s in &carregaveis {
        if s.filesz > s.memsz || s.offset.checked_add(s.filesz).is_none_or(|f| f > d.len()) {
            return Err("segmento do ELF passa do fim do arquivo");
        }
        let inicio = s.vaddr.checked_add(base).ok_or("endereço inválido")?;
        let fim = inicio.checked_add(s.memsz).ok_or("endereço inválido")?;
        let mut pagina = inicio & !(PAGINA - 1);
        while pagina < fim {
            *paginas.entry(pagina).or_insert(0) |= s.flags;
            pagina += PAGINA;
        }
    }
    for (&pagina, &flags) in &paginas {
        let permissao = match (flags & PF_W != 0, flags & PF_X != 0) {
            (true, true) => Permissao::Tudo,
            (true, false) => Permissao::Dados,
            (false, true) => Permissao::Codigo,
            (false, false) => Permissao::Leitura,
        };
        espaco.mapear(pagina, permissao).map_err(|_| "o programa quer memória fora da área dele")?;
    }
    for s in &carregaveis {
        let dados = &d[s.offset..s.offset + s.filesz];
        espaco.escrever_forcado(s.vaddr + base, dados).map_err(|_| "não deu para copiar o programa")?;
    }

    let phoff = u64_em(d, 32).unwrap_or(0);
    let phnum = u16_em(d, 56).unwrap_or(0) as usize;
    // os cabecalhos na memoria: o PT_PHDR, ou o segmento que contem o phoff
    let phdr = segmentos
        .iter()
        .find(|s| s.tipo == PT_PHDR)
        .map(|s| s.vaddr + base)
        .or_else(|| {
            carregaveis
                .iter()
                .find(|s| s.offset <= phoff && phoff < s.offset + s.filesz)
                .map(|s| s.vaddr + base + (phoff - s.offset))
        })
        .unwrap_or(0);
    let fim = carregaveis.iter().map(|s| s.vaddr + base + s.memsz).max().unwrap_or(0);
    Ok(Carregado { entrada: u64_em(d, 24).unwrap_or(0).wrapping_add(base), phdr, phnum, fim })
}

/// O que a CPU tem, no formato do AT_HWCAP do Linux (so o que da para
/// usar em EL0 sem o kernel emular nada).
fn hwcap() -> u64 {
    let (isar0, pfr0): (u64, u64);
    unsafe {
        core::arch::asm!(
            "mrs {0}, id_aa64isar0_el1",
            "mrs {1}, id_aa64pfr0_el1",
            out(reg) isar0, out(reg) pfr0,
            options(nomem, nostack)
        );
    }
    let campo = |valor: u64, bit: u32| valor >> bit & 0xF;
    let mut caps = 0;
    if campo(pfr0, 16) != 0xF {
        caps |= 1 << 0; // FP
    }
    if campo(pfr0, 20) != 0xF {
        caps |= 1 << 1; // ASIMD
    }
    if campo(isar0, 4) >= 1 {
        caps |= 1 << 3; // AES
    }
    if campo(isar0, 4) >= 2 {
        caps |= 1 << 4; // PMULL
    }
    if campo(isar0, 8) >= 1 {
        caps |= 1 << 5; // SHA1
    }
    if campo(isar0, 12) >= 1 {
        caps |= 1 << 6; // SHA2
    }
    if campo(isar0, 16) >= 1 {
        caps |= 1 << 7; // CRC32
    }
    if campo(isar0, 20) >= 2 {
        caps |= 1 << 8; // ATOMICS
    }
    caps
}

/// 16 bytes "aleatorios" (AT_RANDOM): o contador misturado. Serve para o
/// canario de pilha da libc, nao para criptografia.
pub fn aleatorio(semente: u64) -> [u8; 16] {
    let mut x = cpu::contador() ^ semente ^ 0x9E37_79B9_7F4A_7C15;
    let mut saida = [0u8; 16];
    for pedaco in saida.chunks_mut(8) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        pedaco.copy_from_slice(&x.to_le_bytes());
    }
    saida
}

/// Monta a pilha inicial abaixo de `topo` e devolve o sp (aponta para o
/// argc, alinhado em 16).
pub fn montar_pilha(
    espaco: &mut Espaco,
    topo: usize,
    argumentos: &[&[u8]],
    ambiente: &[&[u8]],
    programa: &Carregado,
    nome: &[u8],
) -> Result<usize, &'static str> {
    let mut alto = topo;
    let mut empurrar = |espaco: &mut Espaco, bytes: &[u8], com_zero: bool| -> Result<usize, &'static str> {
        alto -= bytes.len() + com_zero as usize;
        espaco.escrever(alto, bytes).map_err(|_| "pilha pequena demais")?;
        if com_zero {
            espaco.escrever(alto + bytes.len(), &[0]).map_err(|_| "pilha pequena demais")?;
        }
        Ok(alto)
    };
    let aleatorios = empurrar(espaco, &aleatorio(topo as u64), false)?;
    let plataforma = empurrar(espaco, b"aarch64", true)?;
    let execfn = empurrar(espaco, nome, true)?;
    let mut env = Vec::with_capacity(ambiente.len());
    for texto in ambiente {
        env.push(empurrar(espaco, texto, true)?);
    }
    let mut argv = Vec::with_capacity(argumentos.len());
    for texto in argumentos {
        argv.push(empurrar(espaco, texto, true)?);
    }
    let auxv: [(u64, u64); 18] = [
        (AT_PHDR, programa.phdr as u64),
        (AT_PHENT, 56),
        (AT_PHNUM, programa.phnum as u64),
        (AT_PAGESZ, PAGINA as u64),
        (AT_BASE, 0),
        (AT_FLAGS, 0),
        (AT_ENTRY, programa.entrada as u64),
        (AT_UID, 0),
        (AT_EUID, 0),
        (AT_GID, 0),
        (AT_EGID, 0),
        (AT_PLATFORM, plataforma as u64),
        (AT_HWCAP, hwcap()),
        (AT_CLKTCK, 100),
        (AT_SECURE, 0),
        (AT_RANDOM, aleatorios as u64),
        (AT_HWCAP2, 0),
        (AT_EXECFN, execfn as u64),
    ];
    let mut vetor: Vec<u64> = Vec::new();
    vetor.push(argv.len() as u64);
    vetor.extend(argv.iter().map(|&a| a as u64));
    vetor.push(0);
    vetor.extend(env.iter().map(|&e| e as u64));
    vetor.push(0);
    for (tipo, valor) in auxv {
        vetor.push(tipo);
        vetor.push(valor);
    }
    vetor.push(AT_NULL);
    vetor.push(0);
    let sp = (alto - vetor.len() * 8) & !15;
    let bytes: Vec<u8> = vetor.iter().flat_map(|v| v.to_le_bytes()).collect();
    espaco.escrever(sp, &bytes).map_err(|_| "pilha pequena demais")?;
    Ok(sp)
}
