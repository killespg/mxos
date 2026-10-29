//! GET por https com o TLS do MXOS (para os testes com paginas reais).
//! As paginas baixadas ficam em target/paginas (cache entre execucoes).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::tls::{self, Erro, Transporte};

struct Tcp(TcpStream);

impl Transporte for Tcp {
    fn enviar(&mut self, dados: &[u8]) -> Result<(), Erro> {
        self.0.write_all(dados).map_err(|_| Erro::Rede("escrita"))
    }
    fn receber(&mut self, buf: &mut Vec<u8>) -> Result<bool, Erro> {
        let mut b = [0u8; 16384];
        match self.0.read(&mut b) {
            Ok(0) => Ok(false),
            Ok(n) => {
                buf.extend_from_slice(&b[..n]);
                Ok(true)
            }
            Err(_) => Err(Erro::Rede("leitura")),
        }
    }
    fn entropia(&mut self) -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64
    }
}

fn dechunk(mut d: &[u8]) -> Vec<u8> {
    let mut saida = Vec::new();
    loop {
        let Some(fim) = d.windows(2).position(|w| w == b"\r\n") else { break };
        let linha = String::from_utf8_lossy(&d[..fim]);
        let n = usize::from_str_radix(linha.split(';').next().unwrap_or("").trim(), 16).unwrap_or(0);
        d = &d[fim + 2..];
        if n == 0 || d.len() < n {
            saida.extend_from_slice(&d[..n.min(d.len())]);
            break;
        }
        saida.extend_from_slice(&d[..n]);
        d = &d[(n + 2).min(d.len())..];
    }
    saida
}

/// (status, content-type, corpo) de https://host/caminho, seguindo
/// redirecionamentos.
pub fn baixar(url: &str) -> Result<(u16, String, Vec<u8>), String> {
    let mut url = url.to_string();
    for _ in 0..5 {
        let resto = url.strip_prefix("https://").ok_or("so https")?;
        let (host, caminho) = match resto.find('/') {
            Some(i) => (&resto[..i], &resto[i..]),
            None => (resto, "/"),
        };
        let agora = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        let s = TcpStream::connect((host, 443)).map_err(|e| e.to_string())?;
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let mut t = Tcp(s);
        let mut sessao = tls::conectar(&mut t, host, agora).map_err(|e| e.descricao().to_string())?;
        let pedido = format!(
            "GET {caminho} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: Mozilla/5.0 (X11; Linux armv7l) MXOS/0.12\r\nAccept: text/html,*/*\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n"
        );
        sessao.enviar(&mut t, pedido.as_bytes()).map_err(|e| e.descricao().to_string())?;
        let mut bruto = Vec::new();
        while sessao.receber(&mut t, &mut bruto).map_err(|e| e.descricao().to_string())? {}
        let fim = bruto.windows(4).position(|w| w == b"\r\n\r\n").ok_or("sem cabecalho")?;
        let cab = String::from_utf8_lossy(&bruto[..fim]).to_string();
        let status: u16 = cab.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        let campo = |nome: &str| {
            cab.lines().find_map(|l| {
                let (n, v) = l.split_once(':')?;
                n.trim().eq_ignore_ascii_case(nome).then(|| v.trim().to_string())
            })
        };
        if (300..400).contains(&status) {
            if let Some(local) = campo("location") {
                url = if local.starts_with("https://") { local } else { format!("https://{host}{local}") };
                continue;
            }
        }
        let corpo = &bruto[fim + 4..];
        let corpo = if campo("transfer-encoding").is_some_and(|v| v.contains("chunked")) { dechunk(corpo) } else { corpo.to_vec() };
        return Ok((status, campo("content-type").unwrap_or_default(), corpo));
    }
    Err("redirecionamentos demais".into())
}

/// Como `baixar`, mas guarda em target/paginas/<nome> e usa de la depois.
pub fn pagina(url: &str) -> Result<(String, Vec<u8>), String> {
    let mut nome: String = url.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    if nome.len() > 100 {
        // caminhos longos nao cabem no Windows: corta e poe um resumo
        let mut h: u64 = 0xcbf29ce484222325;
        for b in url.bytes() {
            h = (h ^ b as u64).wrapping_mul(0x100000001b3);
        }
        nome = format!("{}_{h:016x}", &nome[..80]);
    }
    let pasta = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/paginas");
    std::fs::create_dir_all(&pasta).ok();
    let arq = pasta.join(&nome);
    let tipo = pasta.join(format!("{nome}.tipo"));
    if let (Ok(c), Ok(t)) = (std::fs::read(&arq), std::fs::read_to_string(&tipo)) {
        return Ok((t, c));
    }
    let (_, t, c) = baixar(url)?;
    std::fs::write(&arq, &c).ok();
    std::fs::write(&tipo, &t).ok();
    Ok((t, c))
}
