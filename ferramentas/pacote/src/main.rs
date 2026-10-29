//! pacote: instala programas do Debian (arm64, trixie) dentro do MXOS.
//!
//!   pacote atualizar          baixa a lista de pacotes do Debian
//!   pacote instalar NOME...   instala com as dependencias
//!   pacote lista              o que ja esta instalado
//!   pacote procurar TEXTO     pacotes com TEXTO no nome
//!
//! Tudo pelo HTTP comum (deb.debian.org), sem dpkg: o .deb e um "ar" com
//! um data.tar.xz (ou .gz) que e extraido direto na raiz. O FAT32 nao
//! guarda links simbolicos, entao cada link vira uma copia do arquivo.
//! Scripts de instalacao (postinst) nao rodam. Compilar:
//! cd ferramentas/pacote && cargo build --release

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;

const SERVIDOR: &str = "deb.debian.org";
const LISTA: &str = "/debian/dists/trixie/main/binary-arm64/Packages.xz";
const INDICE: &str = "/var/lib/mxos/indice";
const INSTALADOS: &str = "/var/lib/mxos/pacotes";

/// O que o MXOS ja tem ou que so serve para o dpkg.
const EXCLUIDOS: &[&str] = &[
    "libc6", "libc-bin", "debconf", "debconf-2.0", "dpkg", "perl-base", "init-system-helpers", "sensible-utils",
    "readline-common", "tzdata", "adduser", "passwd", "login", "base-files", "base-passwd", "coreutils", "dash",
    "bash", "util-linux", "libpam-modules", "libpam-runtime", "libpam0g", "debianutils", "mount", "systemd",
    "dbus", "libnss-systemd", "ucf", "install-info", "gcc-14-base", "lsb-base", "sysvinit-utils",
];

const IGNORAR: &[&str] = &["usr/share/doc/", "usr/share/man/", "usr/share/locale/", "usr/share/lintian/", "usr/share/info/"];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        Some("atualizar") => atualizar(),
        Some("instalar") if args.len() > 1 => instalar(&args[1..]),
        Some("lista") => {
            print!("{}", std::fs::read_to_string(INSTALADOS).unwrap_or_default());
            Ok(())
        }
        Some("procurar") if args.len() > 1 => procurar(&args[1]),
        _ => {
            eprintln!("uso: pacote atualizar | instalar NOME... | lista | procurar TEXTO");
            std::process::exit(2);
        }
    };
    if let Err(e) = r {
        eprintln!("pacote: {e}");
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------------
// HTTP

fn baixar(caminho: &str) -> Result<Vec<u8>, String> {
    let mut atual = String::from(caminho);
    for _ in 0..5 {
        let mut c = TcpStream::connect((SERVIDOR, 80)).map_err(|e| format!("conectar em {SERVIDOR}: {e}"))?;
        write!(c, "GET {atual} HTTP/1.1\r\nHost: {SERVIDOR}\r\nUser-Agent: pacote-mxos\r\nConnection: close\r\n\r\n").map_err(|e| e.to_string())?;
        let mut resposta = Vec::new();
        c.read_to_end(&mut resposta).map_err(|e| e.to_string())?;
        let fim = resposta.windows(4).position(|w| w == b"\r\n\r\n").ok_or("resposta sem cabeçalho")?;
        let cabecalho = String::from_utf8_lossy(&resposta[..fim]).to_string();
        let corpo = resposta[fim + 4..].to_vec();
        let codigo: u32 = cabecalho.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
        let campo = |nome: &str| {
            cabecalho.lines().find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.trim().eq_ignore_ascii_case(nome).then(|| v.trim().to_string())
            })
        };
        if (300..400).contains(&codigo) {
            let destino = campo("location").ok_or("redirecionamento sem destino")?;
            atual = destino.strip_prefix(&format!("http://{SERVIDOR}")).unwrap_or(&destino).to_string();
            if !atual.starts_with('/') {
                return Err(format!("redirecionado para fora: {destino}"));
            }
            continue;
        }
        if codigo != 200 {
            return Err(format!("{atual}: HTTP {codigo}"));
        }
        if campo("transfer-encoding").is_some_and(|t| t.eq_ignore_ascii_case("chunked")) {
            return Ok(desfazer_chunked(&corpo));
        }
        return Ok(corpo);
    }
    Err(String::from("redirecionamentos demais"))
}

fn desfazer_chunked(mut b: &[u8]) -> Vec<u8> {
    let mut saida = Vec::new();
    loop {
        let Some(fim) = b.windows(2).position(|w| w == b"\r\n") else { break };
        let tamanho = usize::from_str_radix(String::from_utf8_lossy(&b[..fim]).split(';').next().unwrap_or("0").trim(), 16).unwrap_or(0);
        if tamanho == 0 || fim + 2 + tamanho > b.len() {
            break;
        }
        saida.extend_from_slice(&b[fim + 2..fim + 2 + tamanho]);
        b = &b[(fim + 4 + tamanho).min(b.len())..];
    }
    saida
}

// ---------------------------------------------------------------------
// Compressao

fn descomprimir(nome: &str, dados: &[u8]) -> Result<Vec<u8>, String> {
    if nome.ends_with(".xz") {
        let mut saida = Vec::new();
        lzma_rs::xz_decompress(&mut std::io::BufReader::new(dados), &mut saida).map_err(|e| format!("xz: {e:?}"))?;
        Ok(saida)
    } else if nome.ends_with(".gz") {
        gunzip(dados)
    } else if nome.ends_with(".tar") {
        Ok(dados.to_vec())
    } else {
        Err(format!("formato não suportado: {nome}"))
    }
}

fn gunzip(d: &[u8]) -> Result<Vec<u8>, String> {
    if d.len() < 18 || d[0] != 0x1f || d[1] != 0x8b {
        return Err(String::from("gzip estragado"));
    }
    let flags = d[3];
    let mut i = 10;
    if flags & 4 != 0 {
        i += 2 + u16::from_le_bytes([d[i], d[i + 1]]) as usize;
    }
    for bit in [8, 16] {
        if flags & bit != 0 {
            while i < d.len() && d[i] != 0 {
                i += 1;
            }
            i += 1;
        }
    }
    if flags & 2 != 0 {
        i += 2;
    }
    miniz_oxide::inflate::decompress_to_vec(&d[i..]).map_err(|e| format!("gzip: {e:?}"))
}

// ---------------------------------------------------------------------
// Indice

struct Pacote {
    filename: String,
    depende: Vec<Vec<String>>,
}

/// Baixa o Packages.xz e guarda so o que importa (nome, arquivo, deps).
fn atualizar() -> Result<(), String> {
    println!("baixando a lista de pacotes do Debian (uns 10 MB)...");
    let xz = baixar(LISTA)?;
    let texto = descomprimir(".xz", &xz)?;
    let texto = String::from_utf8_lossy(&texto);
    let mut indice = String::new();
    let mut quantos = 0;
    for bloco in texto.split("\n\n") {
        let mut campos: BTreeMap<&str, String> = BTreeMap::new();
        for linha in bloco.lines() {
            if let Some((k, v)) = linha.split_once(": ") {
                if matches!(k, "Package" | "Filename" | "Depends" | "Pre-Depends" | "Provides") {
                    campos.insert(k, v.to_string());
                }
            }
        }
        let (Some(nome), Some(arquivo)) = (campos.get("Package"), campos.get("Filename")) else { continue };
        let deps = [campos.get("Pre-Depends"), campos.get("Depends")].into_iter().flatten().cloned().collect::<Vec<_>>().join(", ");
        let prov = campos.get("Provides").cloned().unwrap_or_default();
        indice.push_str(&format!("{nome}\t{arquivo}\t{deps}\t{prov}\n"));
        quantos += 1;
    }
    std::fs::create_dir_all("/var/lib/mxos").ok();
    std::fs::write(INDICE, indice).map_err(|e| format!("gravar {INDICE}: {e}"))?;
    println!("{quantos} pacotes na lista");
    Ok(())
}

fn ler_indice() -> Result<(BTreeMap<String, Pacote>, BTreeMap<String, String>), String> {
    let texto = std::fs::read_to_string(INDICE).map_err(|_| String::from("sem lista: rode \"pacote atualizar\" antes"))?;
    let mut pacotes = BTreeMap::new();
    let mut virtuais = BTreeMap::new();
    for linha in texto.lines() {
        let mut c = linha.split('\t');
        let (Some(nome), Some(arquivo)) = (c.next(), c.next()) else { continue };
        let deps = c.next().unwrap_or("");
        let depende = deps
            .split(',')
            .filter(|d| !d.trim().is_empty())
            .map(|d| d.split('|').map(|a| a.split(['(', '[', ':']).next().unwrap_or("").trim().to_string()).collect())
            .collect();
        for p in c.next().unwrap_or("").split(',') {
            let v = p.split('(').next().unwrap_or("").trim();
            if !v.is_empty() {
                virtuais.entry(v.to_string()).or_insert_with(|| nome.to_string());
            }
        }
        pacotes.insert(nome.to_string(), Pacote { filename: arquivo.to_string(), depende });
    }
    Ok((pacotes, virtuais))
}

fn procurar(texto: &str) -> Result<(), String> {
    let (pacotes, _) = ler_indice()?;
    for nome in pacotes.keys().filter(|n| n.contains(texto)).take(50) {
        println!("{nome}");
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Instalar

fn instalar(pedidos: &[String]) -> Result<(), String> {
    let (pacotes, virtuais) = ler_indice()?;
    let instalados: BTreeSet<String> = std::fs::read_to_string(INSTALADOS).unwrap_or_default().lines().map(String::from).collect();
    let mut fila: VecDeque<String> = pedidos.iter().cloned().collect();
    let mut escolhidos = Vec::new();
    let mut vistos = BTreeSet::new();
    while let Some(nome) = fila.pop_front() {
        let nome = if pacotes.contains_key(&nome) { nome } else { virtuais.get(&nome).cloned().unwrap_or(nome) };
        if EXCLUIDOS.contains(&nome.as_str()) || instalados.contains(&nome) || !vistos.insert(nome.clone()) {
            continue;
        }
        let Some(p) = pacotes.get(&nome) else {
            eprintln!("aviso: {nome} não existe");
            continue;
        };
        escolhidos.push(nome.clone());
        for alternativas in &p.depende {
            if alternativas.iter().any(|a| EXCLUIDOS.contains(&a.as_str()) || instalados.contains(a) || vistos.contains(a)) {
                continue;
            }
            let real = alternativas.iter().find(|a| pacotes.contains_key(*a) || virtuais.contains_key(*a)).unwrap_or(&alternativas[0]);
            fila.push_back(real.clone());
        }
    }
    if escolhidos.is_empty() {
        println!("nada para instalar");
        return Ok(());
    }
    println!("vai instalar {} pacote(s): {}", escolhidos.len(), escolhidos.join(" "));
    let mut links = Vec::new();
    for nome in &escolhidos {
        let p = &pacotes[nome];
        println!("baixando {nome}...");
        let deb = baixar(&format!("/debian/{}", p.filename))?;
        extrair_deb(&deb, &mut links).map_err(|e| format!("{nome}: {e}"))?;
        let mut lista = std::fs::OpenOptions::new().create(true).append(true).open(INSTALADOS).map_err(|e| e.to_string())?;
        writeln!(lista, "{nome}").map_err(|e| e.to_string())?;
    }
    resolver_links(&links);
    println!("pronto: {} pacote(s) instalado(s)", escolhidos.len());
    Ok(())
}

/// Membros do "ar" do .deb: extrai o data.tar.*.
fn extrair_deb(deb: &[u8], links: &mut Vec<(String, String)>) -> Result<(), String> {
    if !deb.starts_with(b"!<arch>\n") {
        return Err(String::from("não é um .deb"));
    }
    let mut i = 8;
    while i + 60 <= deb.len() {
        let nome = String::from_utf8_lossy(&deb[i..i + 16]).trim().trim_end_matches('/').to_string();
        let tamanho: usize = String::from_utf8_lossy(&deb[i + 48..i + 58]).trim().parse().map_err(|_| "ar estragado")?;
        let conteudo = &deb[i + 60..(i + 60 + tamanho).min(deb.len())];
        if nome.starts_with("data.tar") {
            let tar = descomprimir(&nome, conteudo)?;
            return extrair_tar(&tar, links);
        }
        i += 60 + tamanho + (tamanho & 1);
    }
    Err(String::from("o .deb não tem data.tar"))
}

fn campo_tar(b: &[u8]) -> String {
    let fim = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..fim]).to_string()
}

fn octal(b: &[u8]) -> usize {
    usize::from_str_radix(campo_tar(b).trim(), 8).unwrap_or(0)
}

fn extrair_tar(tar: &[u8], links: &mut Vec<(String, String)>) -> Result<(), String> {
    let mut i = 0;
    let mut nome_longo: Option<String> = None;
    while i + 512 <= tar.len() {
        let h = &tar[i..i + 512];
        if h.iter().all(|&c| c == 0) {
            break;
        }
        let tamanho = octal(&h[124..136]);
        let tipo = h[156];
        let dados = &tar[i + 512..(i + 512 + tamanho).min(tar.len())];
        i += 512 + tamanho.div_ceil(512) * 512;
        let mut nome = campo_tar(&h[0..100]);
        let prefixo = campo_tar(&h[345..500]);
        if !prefixo.is_empty() {
            nome = format!("{prefixo}/{nome}");
        }
        match tipo {
            b'L' => {
                nome_longo = Some(campo_tar(dados));
                continue;
            }
            b'x' => {
                // cabecalho pax: so o "path"
                for linha in String::from_utf8_lossy(dados).lines() {
                    if let Some(p) = linha.split_once(" path=").map(|(_, p)| p) {
                        nome_longo = Some(p.to_string());
                    }
                }
                continue;
            }
            _ => {}
        }
        if let Some(l) = nome_longo.take() {
            nome = l;
        }
        let caminho = nome.trim_start_matches("./").trim_end_matches('/').to_string();
        if caminho.is_empty() || IGNORAR.iter().any(|p| caminho.starts_with(p)) {
            continue;
        }
        let destino = format!("/{caminho}");
        match tipo {
            b'5' => {
                std::fs::create_dir_all(&destino).ok();
            }
            b'2' | b'1' => links.push((destino, campo_tar(&h[157..257]))),
            b'0' | 0 => {
                if let Some(pai) = Path::new(&destino).parent() {
                    std::fs::create_dir_all(pai).ok();
                }
                std::fs::write(&destino, dados).map_err(|e| format!("{destino}: {e}"))?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Cada link vira copia do alvo (varias voltas: link de link).
fn resolver_links(links: &[(String, String)]) {
    let mut pendentes: Vec<&(String, String)> = links.iter().collect();
    for _ in 0..8 {
        let antes = pendentes.len();
        pendentes.retain(|(caminho, alvo)| {
            let base = Path::new(caminho).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
            let real = if alvo.starts_with('/') { alvo.clone() } else { format!("{base}/{alvo}") };
            match std::fs::read(&real) {
                Ok(dados) => std::fs::write(caminho, dados).is_err(),
                Err(_) => true,
            }
        });
        if pendentes.is_empty() || pendentes.len() == antes {
            break;
        }
    }
    for (caminho, alvo) in pendentes {
        eprintln!("aviso: link sem alvo: {caminho} -> {alvo}");
    }
}
