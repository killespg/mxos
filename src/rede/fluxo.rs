//! Download HTTPS que nao trava o laco principal (video em streaming): uma
//! conexao propria (soquete), o aperto TLS feito uma vez e reaproveitado,
//! e cada pedaco pedido com `&range=` chega aos poucos — `andar` pega o
//! que ja chegou e volta na hora.
//!
//! O TLS ja guarda o registro pela metade quando os bytes acabam (ver
//! tls::ler_registro), entao o transporte aqui so diz "sem dados agora".

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;

use super::soquete::{self, EstadoTcp};
use super::tls::{self, Transporte};

/// O soquete como transporte do TLS. Bloqueante no aperto de mao (com
/// limite); fora dele, sem dados = erro "agora nao".
struct Soquete {
    id: usize,
    bloqueante: bool,
}

const SEM_DADOS: tls::Erro = tls::Erro::Rede("sem dados agora");

impl Transporte for Soquete {
    fn enviar(&mut self, dados: &[u8]) -> Result<(), tls::Erro> {
        if soquete::tcp_enviar(self.id, dados) { Ok(()) } else { Err(tls::Erro::Fechou) }
    }

    fn receber(&mut self, buf: &mut Vec<u8>) -> Result<bool, tls::Erro> {
        let fim = crate::cpu::contador() + crate::cpu::frequencia_timer() * 15;
        loop {
            let (d, acabou) = soquete::tcp_ler(self.id, 64 * 1024);
            if !d.is_empty() {
                buf.extend_from_slice(&d);
                return Ok(true);
            }
            if acabou {
                return Ok(false);
            }
            if !self.bloqueante {
                return Err(SEM_DADOS);
            }
            if crate::cpu::contador() > fim {
                return Err(tls::Erro::Rede("tempo esgotado"));
            }
            crate::irq::cochilar();
        }
    }

    fn entropia(&mut self) -> u64 {
        crate::cpu::contador()
    }
}

pub struct Baixador {
    host: String,
    caminho: String,
    conexao: Option<(usize, tls::Sessao)>,
    /// O que ja saiu do TLS e ainda nao foi usado (cabecalho + corpo).
    bruto: Vec<u8>,
    /// Pedido em andamento: (inicio, fim) e o tamanho do corpo (quando o
    /// cabecalho chegou) e onde o corpo comeca em `bruto`.
    pedido: Option<(u64, u64)>,
    corpo: Option<(usize, usize)>,
    tentativas: u32,
    pub recebidos: u64,
}

impl Baixador {
    pub fn novo(url: &str) -> Result<Baixador, &'static str> {
        let resto = url.strip_prefix("https://").ok_or("so https")?;
        let (host, caminho) = resto.split_once('/').unwrap_or((resto, ""));
        Ok(Baixador {
            host: String::from(host),
            caminho: format!("/{caminho}"),
            conexao: None,
            bruto: Vec::new(),
            pedido: None,
            corpo: None,
            tentativas: 0,
            recebidos: 0,
        })
    }

    pub fn ocupado(&self) -> bool {
        self.pedido.is_some()
    }

    fn conectar(&mut self) -> Result<(), &'static str> {
        if self.conexao.is_some() {
            return Ok(());
        }
        let ip = super::resolver(&self.host).map_err(|e| e.descricao())?;
        let id = soquete::tcp_abrir(ip, 443).map_err(|e| e.descricao())?;
        let fim = crate::cpu::contador() + crate::cpu::frequencia_timer() * 10;
        loop {
            match soquete::tcp_estado(id) {
                EstadoTcp::Conectada => break,
                EstadoTcp::Fechada(_) => {
                    soquete::tcp_largar(id);
                    return Err("o servidor de vídeo não aceitou a conexão");
                }
                EstadoTcp::Conectando => {}
            }
            if crate::cpu::contador() > fim {
                soquete::tcp_largar(id);
                return Err("o servidor de vídeo não respondeu");
            }
            crate::irq::cochilar();
        }
        let agora = crate::plataforma::segundos_rtc() as i64;
        let mut t = Soquete { id, bloqueante: true };
        match tls::conectar(&mut t, &self.host, agora) {
            Ok(s) => {
                self.conexao = Some((id, s));
                Ok(())
            }
            Err(e) => {
                soquete::tcp_largar(id);
                Err(e.descricao())
            }
        }
    }

    fn largar(&mut self) {
        if let Some((id, _)) = self.conexao.take() {
            soquete::tcp_largar(id);
        }
        self.bruto.clear();
        self.corpo = None;
    }

    /// Pede os bytes [inicio, fim] (fim incluso). Conecta se precisar (isso
    /// bloqueia: so o aperto de mao).
    pub fn pedir(&mut self, inicio: u64, fim: u64) -> Result<(), &'static str> {
        self.conectar()?;
        let envio = format!(
            "GET {}&range={inicio}-{fim} HTTP/1.1\r\nHost: {}\r\nUser-Agent: MXOS/0.12\r\nAccept: */*\r\nConnection: keep-alive\r\n\r\n",
            self.caminho, self.host
        );
        let (id, s) = self.conexao.as_mut().unwrap();
        let mut t = Soquete { id: *id, bloqueante: true };
        if let Err(e) = s.enviar(&mut t, envio.as_bytes()) {
            self.largar();
            return Err(e.descricao());
        }
        self.pedido = Some((inicio, fim));
        self.corpo = None;
        Ok(())
    }

    /// Anda o download sem esperar. Some(corpo) quando o pedido terminou.
    pub fn andar(&mut self) -> Option<Result<Vec<u8>, &'static str>> {
        let (inicio, fim) = self.pedido?;
        let mut caiu = false;
        if let Some((id, s)) = self.conexao.as_mut() {
            let mut t = Soquete { id: *id, bloqueante: false };
            // tudo o que ja chegou (ate uns 2 MB por volta, para nao segurar)
            let antes = self.bruto.len();
            loop {
                match s.receber(&mut t, &mut self.bruto) {
                    Ok(true) => {
                        if self.bruto.len() - antes > 2 << 20 {
                            break;
                        }
                    }
                    Ok(false) => {
                        caiu = true;
                        break;
                    }
                    Err(e) if e == SEM_DADOS => break,
                    Err(_) => {
                        caiu = true;
                        break;
                    }
                }
            }
            self.recebidos += (self.bruto.len() - antes) as u64;
        } else {
            caiu = true;
        }
        // cabecalho
        if self.corpo.is_none() {
            if let Some(p) = self.bruto.windows(4).position(|w| w == b"\r\n\r\n") {
                let cab = String::from_utf8_lossy(&self.bruto[..p]).into_owned();
                let status: u32 = cab.split(' ').nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                let tamanho = cab
                    .lines()
                    .find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case("content-length")).map(|(_, v)| v.trim().parse::<usize>().ok()))
                    .flatten();
                if !(200..300).contains(&status) {
                    self.pedido = None;
                    self.largar();
                    return Some(Err("o servidor de vídeo recusou o pedaço"));
                }
                let Some(n) = tamanho else {
                    self.pedido = None;
                    self.largar();
                    return Some(Err("resposta de vídeo sem tamanho"));
                };
                self.corpo = Some((p + 4, n));
            }
        }
        if let Some((ini, n)) = self.corpo {
            if self.bruto.len() >= ini + n {
                let corpo: Vec<u8> = self.bruto[ini..ini + n].to_vec();
                self.bruto.drain(..ini + n);
                self.corpo = None;
                self.pedido = None;
                self.tentativas = 0;
                return Some(Ok(corpo));
            }
        }
        if caiu {
            // a conexao caiu no meio: abre outra e pede de novo (2 vezes)
            self.largar();
            self.pedido = None;
            self.tentativas += 1;
            if self.tentativas > 2 {
                return Some(Err("a conexão com o servidor de vídeo caiu"));
            }
            if let Err(e) = self.pedir(inicio, fim) {
                return Some(Err(e));
            }
        }
        None
    }

    /// Baixa [inicio, fim] esperando (inicio do video: init e indice).
    pub fn baixar(&mut self, inicio: u64, fim: u64) -> Result<Vec<u8>, &'static str> {
        self.pedir(inicio, fim)?;
        let limite = crate::cpu::contador() + crate::cpu::frequencia_timer() * 30;
        loop {
            if let Some(r) = self.andar() {
                return r;
            }
            if crate::cpu::contador() > limite {
                self.largar();
                self.pedido = None;
                return Err("o servidor de vídeo demorou demais");
            }
            crate::irq::cochilar();
        }
    }
}

impl Drop for Baixador {
    fn drop(&mut self) {
        self.largar();
    }
}
