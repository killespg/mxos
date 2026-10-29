//! Atualizacao do sistema, so quando o usuario pede (app "Atualizar o
//! sistema" ou o comando "atualizar"):
//!
//!   1. procurar: um PC na rede de casa (python rodar.py --servir; ele se
//!      anuncia por UDP 8001) ou, sem PC, a ultima versao no GitHub
//!      (python rodar.py --publicar), pela internet
//!   2. mostrar a versao nova, o tamanho e o que mudou, e esperar o
//!      "instalar"
//!   3. instalar e reiniciar
//!
//! O servidor (PC ou GitHub) tem versao.txt, novidades.txt e tamanho.txt.
//! Do PC: manifesto.txt com crc32, tamanho e caminho de cada arquivo; so os
//! que mudaram descem, e cada um e conferido antes de gravar. Do GitHub: o
//! kernel so fala HTTP, entao o curl do Debian baixa (HTTPS) e o tar do
//! BusyBox abre o mxos.tar na raiz. Kernel novo com a box ligada da eMMC: a
//! particao boot tambem e regravada.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

use crate::rede::soquete as udp;
use crate::{armazenamento, cpu, psci, rede};

const KERNEL: &str = "/mxos-mxq.img";
const PORTA_ANUNCIO: u16 = 8001;
/// Quanto esperar o anuncio do PC antes de tentar a internet.
const PRAZO_PROCURA_S: u64 = 8;
const GITHUB: &str = "https://github.com/killespg/mxos/releases/latest/download";
const CURL: &str = "/usr/bin/curl";
const PASTA: &str = "/var/lib/mxos";
const ULTIMA_BUSCA: &str = "/var/lib/mxos/ultima-busca.txt";
/// Arquivos conferidos por passo (cada um e lido e passa pelo crc32).
const POR_PASSO: usize = 8;

/// crc32 (o mesmo do zlib).
pub fn crc32(dados: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in dados {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { crc >> 1 ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

#[derive(Clone)]
struct Item {
    crc: u32,
    tamanho: usize,
    caminho: String,
}

enum Passo {
    Parado,
    Procurando { prazo: u64 },
    /// Do PC: conferindo o manifesto com o disco.
    Conferindo { servidor: String, itens: Vec<Item>, proximo: usize, mudados: Vec<Item> },
    /// Internet: curl/tar rodando (0 versao, 1 novidades, 2 tamanho, 3 pacote, 4 tar).
    Internet { etapa: u8, pid: u32 },
    /// Achou algo novo: espera o "instalar".
    Pronto(Fonte),
    Baixando { servidor: String, mudados: Vec<Item>, proximo: usize },
    Reiniciando { quando: u64 },
    Fim,
}

enum Fonte {
    Pc { servidor: String, mudados: Vec<Item> },
    Internet,
}

/// O que o app mostra.
#[derive(Default)]
pub struct Info {
    pub versao_atual: String,
    pub versao_nova: Option<String>,
    pub novidades: Vec<String>,
    /// Bytes a baixar.
    pub tamanho: Option<usize>,
    /// "qua, 29/10/2026 14:05" (ou vazio: nunca procurou).
    pub ultima_busca: String,
    /// "PC 192.168.0.10:8000" ou "GitHub".
    pub origem: String,
}

pub struct Atualizacao {
    passo: Passo,
    /// O que aconteceu (o app mostra as ultimas).
    pub linhas: Vec<String>,
    pub info: Info,
    trocados: usize,
    /// (feito, total) para a barra.
    pub feito: (usize, usize),
}

fn segundos() -> u64 {
    cpu::contador() / cpu::frequencia_timer().max(1)
}

fn texto_do_arquivo(caminho: &str) -> String {
    armazenamento::ler(caminho).map(|d| String::from(String::from_utf8_lossy(&d).trim())).unwrap_or_default()
}

/// Agora, na hora local: "qua, 29/10/2026 14:05".
fn agora_texto() -> String {
    let s = crate::gui::area::agora_local();
    format!("{} {:02}:{:02}", crate::gui::area::data(s), s % 86_400 / 3600, s % 3600 / 60)
}

impl Default for Atualizacao {
    fn default() -> Self {
        let info = Info {
            versao_atual: {
                let v = texto_do_arquivo("/versao.txt");
                if v.is_empty() { String::from(concat!(env!("CARGO_PKG_VERSION"), " (sem versao.txt)")) } else { v }
            },
            ultima_busca: texto_do_arquivo(ULTIMA_BUSCA),
            ..Info::default()
        };
        Atualizacao { passo: Passo::Parado, linhas: Vec::new(), info, trocados: 0, feito: (0, 0) }
    }
}

impl Atualizacao {
    fn dizer(&mut self, linha: String) {
        self.linhas.push(linha);
    }

    /// (pode procurar, pode instalar).
    pub fn botoes(&self) -> (bool, bool) {
        match self.passo {
            Passo::Parado | Passo::Fim => (true, false),
            Passo::Pronto(_) => (false, true),
            _ => (false, false),
        }
    }

    pub fn terminou(&self) -> bool {
        matches!(self.passo, Passo::Fim | Passo::Parado)
    }

    pub fn esperando_confirmacao(&self) -> bool {
        matches!(self.passo, Passo::Pronto(_))
    }

    /// O usuario pediu: procura (no PC `servidor`, ou na rede e na internet).
    pub fn procurar(&mut self, servidor: Option<&str>) {
        self.linhas.clear();
        self.trocados = 0;
        self.feito = (0, 0);
        self.info.versao_nova = None;
        self.info.novidades.clear();
        self.info.tamanho = None;
        self.info.origem.clear();
        // a data da busca fica guardada (o app mostra na proxima vez)
        self.info.ultima_busca = agora_texto();
        for pasta in ["/var", "/var/lib", PASTA] {
            let _ = armazenamento::criar_pasta(pasta);
        }
        let _ = armazenamento::escrever(ULTIMA_BUSCA, self.info.ultima_busca.as_bytes());
        if rede::info().is_none() {
            let _ = rede::iniciar();
        }
        match servidor {
            Some(s) => self.abrir_pc(String::from(s)),
            None => {
                let _ = udp::udp_abrir(PORTA_ANUNCIO);
                self.dizer(String::from("Procurando um PC com atualização na rede de casa..."));
                self.passo = Passo::Procurando { prazo: segundos() + PRAZO_PROCURA_S };
            }
        }
    }

    /// O usuario confirmou: instala o que foi achado.
    pub fn instalar(&mut self) {
        match core::mem::replace(&mut self.passo, Passo::Fim) {
            Passo::Pronto(Fonte::Pc { servidor, mudados }) => {
                self.feito = (0, mudados.len());
                self.dizer(String::from("Baixando..."));
                self.passo = Passo::Baixando { servidor, mudados, proximo: 0 };
            }
            Passo::Pronto(Fonte::Internet) => {
                self.dizer(String::from("Baixando a versão nova..."));
                self.rodar(3, CURL, &["-fSL", "-o", "/var/lib/mxos/mxos.tar", &format!("{GITHUB}/mxos.tar")]);
            }
            outro => self.passo = outro,
        }
    }

    fn abrir_pc(&mut self, servidor: String) {
        self.info.origem = format!("PC {servidor}");
        self.dizer(format!("Achei o PC: {servidor}"));
        let texto = |c: &str| baixar(&servidor, c).map(|d| String::from(String::from_utf8_lossy(&d).trim())).unwrap_or_default();
        let versao = texto("/versao.txt");
        let novidades = texto("/novidades.txt");
        self.info.versao_nova = (!versao.is_empty()).then_some(versao);
        self.info.novidades = novidades.lines().map(String::from).collect();
        let manifesto = match baixar(&servidor, "/manifesto.txt") {
            Ok(t) => String::from_utf8_lossy(&t).into_owned(),
            Err(e) => {
                self.dizer(format!("Erro: {e}"));
                self.passo = Passo::Fim;
                return;
            }
        };
        let mut itens = Vec::new();
        for linha in manifesto.lines() {
            let mut p = linha.splitn(3, ' ');
            if let (Some(c), Some(t), Some(caminho)) = (p.next(), p.next(), p.next()) {
                if let (Ok(crc), Ok(tamanho)) = (u32::from_str_radix(c, 16), t.parse()) {
                    itens.push(Item { crc, tamanho, caminho: String::from(caminho) });
                }
            }
        }
        self.feito = (0, itens.len());
        self.dizer(format!("Conferindo {} arquivos com os desta box...", itens.len()));
        self.passo = Passo::Conferindo { servidor, itens, proximo: 0, mudados: Vec::new() };
    }

    /// Um passo do trabalho. true = algo mudou (redesenhar).
    pub fn avancar(&mut self) -> bool {
        match &mut self.passo {
            Passo::Parado | Passo::Pronto(_) | Passo::Fim => false,
            Passo::Procurando { prazo } => {
                let prazo = *prazo;
                rede::processar();
                if let Some((ip, _, dados)) = udp::udp_tirar(PORTA_ANUNCIO) {
                    if let Some(porta) = String::from_utf8_lossy(&dados).strip_prefix("MXOS-ATUALIZACAO ") {
                        udp::udp_fechar(PORTA_ANUNCIO);
                        let servidor = format!("{}.{}.{}.{}:{}", ip[0], ip[1], ip[2], ip[3], porta.trim());
                        self.abrir_pc(servidor);
                        return true;
                    }
                }
                if segundos() > prazo {
                    udp::udp_fechar(PORTA_ANUNCIO);
                    self.info.origem = String::from("GitHub (internet)");
                    self.dizer(String::from("Nenhum PC na rede; olhando a última versão no GitHub..."));
                    self.rodar(0, CURL, &["-fsSL", "-o", "/var/lib/mxos/versao-nova.txt", &format!("{GITHUB}/versao.txt")]);
                    return true;
                }
                false
            }
            Passo::Conferindo { itens, proximo, mudados, .. } => {
                for _ in 0..POR_PASSO {
                    let Some(item) = itens.get(*proximo) else { break };
                    *proximo += 1;
                    let igual = armazenamento::ler(&item.caminho).is_ok_and(|d| d.len() == item.tamanho && crc32(&d) == item.crc);
                    if !igual {
                        mudados.push(item.clone());
                    }
                }
                self.feito.0 = *proximo;
                if *proximo >= itens.len() {
                    let Passo::Conferindo { servidor, mudados, .. } = core::mem::replace(&mut self.passo, Passo::Fim) else { return true };
                    self.feito = (0, 0);
                    if mudados.is_empty() {
                        self.dizer(String::from("Já está tudo atualizado."));
                        self.info.versao_nova = None;
                    } else {
                        let bytes: usize = mudados.iter().map(|m| m.tamanho).sum();
                        self.info.tamanho = Some(bytes);
                        self.dizer(format!("Atualização disponível: {} arquivo(s) mudaram.", mudados.len()));
                        self.passo = Passo::Pronto(Fonte::Pc { servidor, mudados });
                    }
                }
                true
            }
            Passo::Internet { etapa, pid } => {
                let (etapa, pid) = (*etapa, *pid);
                match crate::processo::terminou(pid) {
                    None => false,
                    Some(codigo) => {
                        self.internet(etapa, codigo);
                        true
                    }
                }
            }
            Passo::Baixando { .. } => self.baixar_proximo(),
            Passo::Reiniciando { quando } => {
                if segundos() >= *quando {
                    psci::reiniciar();
                }
                false
            }
        }
    }

    /// Baixa e grava um arquivo mudado por passo.
    fn baixar_proximo(&mut self) -> bool {
        let Passo::Baixando { servidor, mudados, proximo } = &mut self.passo else { return false };
        let Some(item) = mudados.get(*proximo).cloned() else {
            self.finalizar();
            return true;
        };
        *proximo += 1;
        self.feito.0 = *proximo;
        let servidor = servidor.clone();
        let resultado = baixar(&servidor, &item.caminho).and_then(|dados| {
            if dados.len() != item.tamanho || crc32(&dados) != item.crc {
                return Err(String::from("chegou estragado (tente de novo)"));
            }
            criar_pastas(&item.caminho);
            armazenamento::escrever(&item.caminho, &dados).map_err(|e| String::from(e.descricao()))
        });
        match resultado {
            Ok(()) => {
                self.trocados += 1;
                self.dizer(format!("Atualizado: {}", item.caminho));
            }
            Err(e) => {
                self.dizer(format!("Erro em {}: {e}", item.caminho));
                self.passo = Passo::Fim;
            }
        }
        true
    }

    /// Comeca o curl ou o tar da etapa `etapa` da internet.
    fn rodar(&mut self, etapa: u8, programa: &str, args: &[&str]) {
        for pasta in ["/var", "/var/lib", PASTA] {
            let _ = armazenamento::criar_pasta(pasta);
        }
        match crate::processo::executar(programa, args, false) {
            Ok(pid) => self.passo = Passo::Internet { etapa, pid },
            Err(e) => {
                self.dizer(format!("Erro: {e}"));
                self.passo = Passo::Fim;
            }
        }
    }

    fn internet(&mut self, etapa: u8, codigo: i32) {
        // novidades e tamanho sao opcionais: sem eles, segue
        if codigo != 0 && !matches!(etapa, 1 | 2) {
            let motivo = match etapa {
                0 => "não deu para falar com o GitHub (sem internet, ou nada publicado ainda)",
                3 => "o download falhou",
                _ => "não deu para abrir o pacote",
            };
            self.dizer(format!("Erro: {motivo} (código {codigo})"));
            self.passo = Passo::Fim;
            return;
        }
        match etapa {
            0 => {
                let nova = texto_do_arquivo("/var/lib/mxos/versao-nova.txt");
                if !nova.is_empty() && nova == self.info.versao_atual {
                    self.dizer(format!("Já está na última versão ({nova})."));
                    self.passo = Passo::Fim;
                    return;
                }
                self.info.versao_nova = Some(nova);
                self.rodar(1, CURL, &["-fsSL", "-o", "/var/lib/mxos/novidades.txt", &format!("{GITHUB}/novidades.txt")]);
            }
            1 => {
                self.info.novidades = texto_do_arquivo("/var/lib/mxos/novidades.txt").lines().map(String::from).collect();
                self.rodar(2, CURL, &["-fsSL", "-o", "/var/lib/mxos/tamanho.txt", &format!("{GITHUB}/tamanho.txt")]);
            }
            2 => {
                self.info.tamanho = texto_do_arquivo("/var/lib/mxos/tamanho.txt").parse().ok();
                self.dizer(String::from("Atualização disponível no GitHub."));
                self.passo = Passo::Pronto(Fonte::Internet);
            }
            3 => {
                self.dizer(String::from("Instalando os arquivos..."));
                self.rodar(4, "/bin/busybox", &["tar", "-xf", "/var/lib/mxos/mxos.tar", "-C", "/"]);
            }
            _ => {
                let _ = armazenamento::apagar("/var/lib/mxos/mxos.tar");
                self.trocados = 1;
                self.finalizar();
            }
        }
    }

    fn finalizar(&mut self) {
        #[cfg(feature = "mxq")]
        if let Ok(kernel) = armazenamento::ler(KERNEL) {
            match armazenamento::atualizar_boot_emmc(&kernel) {
                Ok(true) => self.dizer(String::from("Kernel novo gravado na memória interna.")),
                Ok(false) => {}
                Err(e) => {
                    self.dizer(format!("Erro ao gravar o kernel: {}", e.descricao()));
                    self.passo = Passo::Fim;
                    return;
                }
            }
        }
        let _ = KERNEL;
        self.dizer(format!("Pronto ({} arquivo(s)). Reiniciando em 3 segundos...", self.trocados));
        self.passo = Passo::Reiniciando { quando: segundos() + 3 };
    }
}

fn criar_pastas(caminho: &str) {
    let mut atual = String::new();
    let partes: Vec<&str> = caminho.split('/').filter(|p| !p.is_empty()).collect();
    for parte in &partes[..partes.len().saturating_sub(1)] {
        atual.push('/');
        atual.push_str(parte);
        let _ = armazenamento::criar_pasta(&atual);
    }
}

fn baixar(servidor: &str, caminho: &str) -> Result<Vec<u8>, String> {
    let url = format!("http://{servidor}{caminho}");
    let r = rede::http::get(&url, 60_000).map_err(|e| format!("{url}: {}", e.descricao()))?;
    if r.status != 200 {
        return Err(format!("{url}: HTTP {}", r.status));
    }
    Ok(r.corpo)
}

/// Tamanho para mostrar.
pub fn tamanho_texto(bytes: usize) -> String {
    match bytes {
        b if b >= 1 << 20 => format!("{},{} MB", b >> 20, (b % (1 << 20)) * 10 >> 20),
        b if b >= 1 << 10 => format!("{} KB", b >> 10),
        b => format!("{b} bytes"),
    }
}

/// "atualizar [IP:PORTA]" procura; "atualizar instalar" instala o achado.
pub fn comando(nome: &str, args: &str, saida: &mut dyn Write) -> bool {
    if nome != "atualizar" {
        return false;
    }
    let a = unsafe { &mut *ESTADO.0.get() }.get_or_insert_with(Atualizacao::default);
    match args.trim() {
        "instalar" if a.esperando_confirmacao() => a.instalar(),
        "instalar" => {
            let _ = writeln!(saida, "nada para instalar: rode \"atualizar\" antes");
            return true;
        }
        "" => a.procurar(None),
        servidor => a.procurar(Some(servidor)),
    }
    let mut mostradas = 0;
    loop {
        a.avancar();
        // o curl e o tar precisam da CPU enquanto o shell espera
        crate::processo::rodar();
        rede::processar();
        for linha in &a.linhas[mostradas..] {
            let _ = writeln!(saida, "{linha}");
        }
        mostradas = a.linhas.len();
        if a.esperando_confirmacao() {
            let i = &a.info;
            let _ = writeln!(saida, "versão instalada: {}", i.versao_atual);
            let _ = writeln!(saida, "versão nova: {}", i.versao_nova.as_deref().unwrap_or("?"));
            if let Some(t) = i.tamanho {
                let _ = writeln!(saida, "tamanho: {}", tamanho_texto(t));
            }
            for n in &i.novidades {
                let _ = writeln!(saida, "  {n}");
            }
            let _ = writeln!(saida, "para instalar: atualizar instalar");
            return true;
        }
        if a.terminou() {
            return true;
        }
    }
}

struct Guardado(core::cell::UnsafeCell<Option<Atualizacao>>);

// So o laco principal mexe (com a trava do kernel).
unsafe impl Sync for Guardado {}

static ESTADO: Guardado = Guardado(core::cell::UnsafeCell::new(None));
