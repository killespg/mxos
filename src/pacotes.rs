//! Pacotes .mxa: apps nativos instalados (e atualizados) sem atualizar o
//! sistema. Um .mxa e um tar (ustar) comprimido com gzip:
//!
//!   manifesto.json   {"id": "calculadora", "nome": "Calculadora",
//!                     "versao": "1.0.0", "descricao": "...", "autor": "...",
//!                     "licenca": "GPL-3.0-or-later", "icone": "Calculadora",
//!                     "programas": {"armv7": "armv7/calculadora",
//!                                   "aarch64": "aarch64/calculadora"}}
//!   armv7/...        o programa de cada arquitetura (ELF estatico, sdk/)
//!   o resto          arquivos que o app le (relativos a pasta dele)
//!
//! Instalar copia tudo (menos os programas das outras arquiteturas) para
//! /apps/<id>/; o app roda como processo com a ABI do MXOS
//! (processo/abi.rs) e abre a propria janela. O icone e o nome de um dos
//! icones do sistema (gui::widgets::Icone).

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::armazenamento;
use crate::gui::widgets::Icone;
use crate::json::{self, Json};

/// Onde os pacotes ficam instalados.
pub const PASTA: &str = "/apps";

/// A arquitetura desta maquina, como os manifestos chamam.
#[cfg(target_arch = "arm")]
pub const ARQUITETURA: &str = "armv7";
#[cfg(not(target_arch = "arm"))]
pub const ARQUITETURA: &str = "aarch64";

/// Maior pacote aceito (comprimido e aberto).
const MAX_PACOTE: usize = 64 << 20;

#[derive(Clone, Debug)]
pub struct Manifesto {
    pub id: String,
    pub nome: String,
    pub versao: String,
    pub descricao: String,
    pub autor: String,
    pub licenca: String,
    pub icone: String,
    /// O programa desta arquitetura (caminho dentro do pacote).
    pub programa: String,
}

impl Manifesto {
    pub fn ler(dados: &[u8]) -> Result<Manifesto, String> {
        let j = json::ler(dados).ok_or("manifesto.json não é um JSON válido")?;
        let campo = |nome: &str| j.campo(nome).and_then(Json::texto).unwrap_or("").to_string();
        let id = campo("id");
        if id.is_empty() || id.len() > 48 || !id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
            return Err(String::from("id do pacote inválido (use a-z, 0-9 e -)"));
        }
        let programa = j
            .campo("programas")
            .and_then(|p| p.campo(ARQUITETURA))
            .and_then(Json::texto)
            .ok_or_else(|| format!("o pacote não tem a versão para {ARQUITETURA}"))?
            .to_string();
        if !caminho_seguro(&programa) {
            return Err(String::from("caminho do programa inválido"));
        }
        let nome = campo("nome");
        Ok(Manifesto {
            nome: if nome.is_empty() { id.clone() } else { nome },
            id,
            versao: campo("versao"),
            descricao: campo("descricao"),
            autor: campo("autor"),
            licenca: campo("licenca"),
            icone: campo("icone"),
            programa,
        })
    }

    /// Pasta onde o pacote fica instalado.
    pub fn pasta(&self) -> String {
        format!("{PASTA}/{}", self.id)
    }
}

/// Caminho relativo sem ".." nem barra no comeco.
fn caminho_seguro(c: &str) -> bool {
    !c.is_empty() && !c.starts_with('/') && !c.contains('\\') && c.split('/').all(|p| !p.is_empty() && p != "." && p != "..")
}

/// O icone do sistema com esse nome (o generico se nao existir).
pub fn icone(nome: &str) -> Icone {
    match nome.to_ascii_lowercase().as_str() {
        "pasta" => Icone::Pasta,
        "arquivo" => Icone::Arquivo,
        "texto" => Icone::Texto,
        "html" => Icone::Html,
        "disco" => Icone::Disco,
        "rede" => Icone::Rede,
        "terminal" => Icone::Terminal,
        "navegador" => Icone::Navegador,
        "engrenagem" => Icone::Engrenagem,
        "tarefas" => Icone::Tarefas,
        "chip" => Icone::Chip,
        "pincel" => Icone::Pincel,
        "calculadora" => Icone::Calculadora,
        "info" => Icone::Info,
        "teclado" => Icone::Teclado,
        "som" => Icone::Som,
        _ => Icone::Monitor,
    }
}

/// Os arquivos de um .mxa: (caminho, conteudo).
pub fn abrir_mxa(dados: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let tar = crate::web::imagem::inflate::gzip(dados, MAX_PACOTE).ok_or("o pacote não é um .mxa (gzip) válido")?;
    let mut arquivos = Vec::new();
    let mut pos = 0;
    while pos + 512 <= tar.len() {
        let h = &tar[pos..pos + 512];
        if h.iter().all(|&b| b == 0) {
            break;
        }
        let texto = |a: usize, b: usize| {
            let campo = &h[a..b];
            let fim = campo.iter().position(|&c| c == 0).unwrap_or(campo.len());
            String::from_utf8_lossy(&campo[..fim]).into_owned()
        };
        let tamanho = usize::from_str_radix(texto(124, 136).trim(), 8).map_err(|_| "cabeçalho do tar quebrado")?;
        let mut nome = texto(0, 100);
        if &h[257..262] == b"ustar" {
            let prefixo = texto(345, 500);
            if !prefixo.is_empty() {
                nome = format!("{prefixo}/{nome}");
            }
        }
        let tipo = h[156];
        let inicio = pos + 512;
        let fim = inicio.checked_add(tamanho).filter(|&f| f <= tar.len()).ok_or("o tar acaba no meio de um arquivo")?;
        let nome = nome.trim_start_matches("./").to_string();
        if (tipo == b'0' || tipo == 0) && !nome.is_empty() {
            if !caminho_seguro(&nome) {
                return Err(format!("caminho perigoso no pacote: {nome}"));
            }
            arquivos.push((nome, tar[inicio..fim].to_vec()));
        }
        pos = inicio + tamanho.next_multiple_of(512);
    }
    Ok(arquivos)
}

/// Le o manifesto de um .mxa sem instalar.
pub fn manifesto_do_mxa(dados: &[u8]) -> Result<Manifesto, String> {
    let arquivos = abrir_mxa(dados)?;
    let (_, m) = arquivos.iter().find(|(n, _)| n == "manifesto.json").ok_or("o pacote não tem manifesto.json")?;
    Manifesto::ler(m)
}

/// Cria `pasta` e as pastas acima dela que faltarem.
fn criar_pastas(pasta: &str) -> Result<(), String> {
    let mut atual = String::new();
    for parte in pasta.split('/').filter(|p| !p.is_empty()) {
        atual.push('/');
        atual.push_str(parte);
        if armazenamento::listar(&atual).is_err() {
            armazenamento::criar_pasta(&atual).map_err(|e| format!("{atual}: {}", e.descricao()))?;
        }
    }
    Ok(())
}

/// Apaga `caminho` e tudo dentro.
fn apagar_tudo(caminho: &str, profundidade: u32) -> Result<(), String> {
    if profundidade > 16 {
        return Err(String::from("pastas fundas demais"));
    }
    if let Ok(itens) = armazenamento::listar(caminho) {
        for item in itens {
            apagar_tudo(&format!("{caminho}/{}", item.nome), profundidade + 1)?;
        }
    }
    armazenamento::apagar(caminho).map_err(|e| format!("{caminho}: {}", e.descricao()))
}

/// Instala (ou atualiza) o pacote. Devolve o manifesto.
pub fn instalar(dados: &[u8]) -> Result<Manifesto, String> {
    if !armazenamento::montado() {
        return Err(String::from("nenhum disco montado: ligue um pendrive com FAT32"));
    }
    let arquivos = abrir_mxa(dados)?;
    let (_, m) = arquivos.iter().find(|(n, _)| n == "manifesto.json").ok_or("o pacote não tem manifesto.json")?;
    let manifesto = Manifesto::ler(m)?;
    if !arquivos.iter().any(|(n, _)| *n == manifesto.programa) {
        return Err(format!("o pacote não tem o programa {}", manifesto.programa));
    }
    // os programas das outras arquiteturas nao precisam ocupar o disco
    let outros: Vec<String> = json::ler(m)
        .and_then(|j| {
            j.campo("programas").map(|p| match p {
                Json::Objeto(mapa) => mapa.iter().filter(|(a, _)| a.as_str() != ARQUITETURA).filter_map(|(_, c)| c.texto().map(String::from)).collect(),
                _ => Vec::new(),
            })
        })
        .unwrap_or_default();
    let pasta = manifesto.pasta();
    if armazenamento::listar(&pasta).is_ok() {
        apagar_tudo(&pasta, 0)?;
    }
    criar_pastas(&pasta)?;
    for (nome, conteudo) in &arquivos {
        if outros.contains(nome) {
            continue;
        }
        let destino = format!("{pasta}/{nome}");
        if let Some((pai, _)) = destino.rsplit_once('/') {
            criar_pastas(pai)?;
        }
        armazenamento::escrever(&destino, conteudo).map_err(|e| format!("{destino}: {}", e.descricao()))?;
    }
    Ok(manifesto)
}

/// Os pacotes instalados, em ordem de nome.
pub fn instalados() -> Vec<Manifesto> {
    let Ok(itens) = armazenamento::listar(PASTA) else { return Vec::new() };
    let mut lista: Vec<Manifesto> = itens
        .iter()
        .filter(|i| i.pasta)
        .filter_map(|i| armazenamento::ler(&format!("{PASTA}/{}/manifesto.json", i.nome)).ok())
        .filter_map(|d| Manifesto::ler(&d).ok())
        .collect();
    lista.sort_by_key(|m| m.nome.to_lowercase());
    lista
}

pub fn instalado(id: &str) -> Option<Manifesto> {
    let d = armazenamento::ler(&format!("{PASTA}/{id}/manifesto.json")).ok()?;
    Manifesto::ler(&d).ok()
}

pub fn desinstalar(id: &str) -> Result<(), String> {
    let m = instalado(id).ok_or("o app não está instalado")?;
    apagar_tudo(&m.pasta(), 0)
}

/// Abre o app instalado `id` (o processo cria a janela dele). Devolve o pid.
pub fn abrir(id: &str, argumentos: &str) -> Result<u32, String> {
    let m = instalado(id).ok_or_else(|| format!("{id}: o app não está instalado"))?;
    crate::processo::abrir_app(&m.nome, &format!("{}/{}", m.pasta(), m.programa), argumentos)
}

/// Compara versoes "1.2.10" > "1.2.9" (pedaco a pedaco, numerico).
pub fn versao_maior(a: &str, b: &str) -> bool {
    let partes = |v: &str| v.split(['.', '-']).map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    partes(a) > partes(b)
}
