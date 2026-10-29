//! O cliente TLS 1.3 contra sites de verdade (precisa de internet).
//!
//!   cargo +stable-x86_64-pc-windows-gnu test --release --test sites -- --nocapture

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use teste_cripto::cripto::{acaso, x509};
use teste_cripto::tls::{self, Erro, Transporte};

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

fn buscar(host: &str) -> Result<(String, String, usize), Erro> {
    let agora = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    let s = TcpStream::connect((host, 443)).map_err(|_| Erro::Rede("conectar"))?;
    s.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
    let mut t = Tcp(s);
    let mut sessao = tls::conectar(&mut t, host, agora)?;
    let pedido = format!("GET / HTTP/1.1\r\nHost: {host}\r\nUser-Agent: MXOS\r\nConnection: close\r\n\r\n");
    sessao.enviar(&mut t, pedido.as_bytes())?;
    let mut corpo = Vec::new();
    while sessao.receber(&mut t, &mut corpo)? {}
    let primeira = String::from_utf8_lossy(&corpo).lines().next().unwrap_or("").to_string();
    Ok((sessao.cifra().to_string(), primeira, corpo.len()))
}

#[test]
fn sites_reais() {
    acaso::misturar(&SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos().to_le_bytes());
    let sites = [
        "example.com", "www.google.com", "www.cloudflare.com", "github.com", "pt.wikipedia.org",
        "www.youtube.com", "www.amazon.com", "letsencrypt.org", "www.uol.com.br", "www.gov.br",
        "www.microsoft.com", "www.facebook.com",
    ];
    let mut falhas = 0;
    for host in sites {
        let inicio = Instant::now();
        match buscar(host) {
            Ok((cifra, linha, n)) => println!("ok   {host:22} {cifra:28} {n:8} bytes {:5} ms  {linha}", inicio.elapsed().as_millis()),
            Err(e) => {
                falhas += 1;
                println!("FALHA {host:22} {:?} ({})", e, e.descricao())
            }
        }
    }
    assert_eq!(falhas, 0);
}

#[test]
fn certificados_ruins() {
    // a cadeia de verdade do github.com, mexida de varios jeitos
    let s = TcpStream::connect(("github.com", 443)).unwrap();
    let mut t = Tcp(s);
    let agora = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    let sessao = tls::conectar(&mut t, "github.com", agora).unwrap();
    let cadeia: Vec<&[u8]> = sessao.cadeia.iter().map(|c| c.as_slice()).collect();
    assert!(x509::verificar_cadeia(&cadeia, "github.com", agora).is_ok());
    assert!(x509::verificar_cadeia(&cadeia, "GitHub.com.", agora).is_ok());
    assert_eq!(x509::verificar_cadeia(&cadeia, "gitlab.com", agora).err(), Some(x509::Erro::NomeNaoConfere));
    assert_eq!(x509::verificar_cadeia(&cadeia, "x.github.com", agora).err(), Some(x509::Erro::NomeNaoConfere));
    assert_eq!(x509::verificar_cadeia(&cadeia, "github.com", 4_102_444_800).err(), Some(x509::Erro::Expirado));
    assert_eq!(x509::verificar_cadeia(&cadeia[..1], "github.com", agora).err(), Some(x509::Erro::SemAutoridade));
    let mut mexida = cadeia[0].to_vec();
    let n = mexida.len();
    mexida[n - 10] ^= 1; // dentro da assinatura
    let mut c2 = cadeia.clone();
    c2[0] = &mexida;
    assert_eq!(x509::verificar_cadeia(&c2, "github.com", agora).err(), Some(x509::Erro::AssinaturaInvalida));
    println!("{} certificados na cadeia; recusas certas", cadeia.len());
}
