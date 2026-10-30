//! Rede do MXOS: pilha TCP/IP escrita do zero sobre o virtio-net do QEMU.
//!
//! Uma camada por arquivo, de baixo para cima:
//!   virtio_net  a placa de rede (fila 0 recebe, fila 1 envia)
//!   ethernet    quadros: MAC destino, MAC origem, tipo
//!   arp         descobre o MAC de um IP da rede local
//!   ipv4        cabecalho IP, checksum e rota (direto ou pelo gateway)
//!   icmp        ping (responde e mede)
//!   udp         datagramas (DHCP e DNS)
//!   dhcp        pega IP, mascara, gateway e DNS
//!   dns         nome -> IP (registro A)
//!   tcp         conexoes cliente (a do navegador e as dos programas)
//!   soquete     a rede para os sockets dos programas (TCP, UDP, ICMP)
//!   http        GET com redirecionamentos
//!   comandos    ip, ping, dns e http no shell
//!
//! Tudo por polling: ninguem mexe na rede dentro de interrupcao. O estado
//! global fica atras de uma trava simples. Quem espera resposta pega a
//! trava, processa o que chegou, solta a trava e dorme ate a proxima
//! interrupcao (irq::cochilar); o tique de 100 Hz garante que acorda.
//!
//! No QEMU com "-netdev user" (slirp): DHCP e gateway em 10.0.2.2, DNS em
//! 10.0.2.3 e o nosso IP vira 10.0.2.15. O host responde em 10.0.2.2.

// Nem todo item publico e usado pelo resto do kernel ainda.
#![allow(dead_code)]

mod arp;
mod comandos;
mod dhcp;
mod dns;
mod ethernet;
pub mod http;
mod icmp;
mod ipv4;
pub mod soquete;
mod tcp;
mod udp;
mod ntp;
pub mod tls;
#[cfg(any(feature = "mxq", feature = "rk322x"))]
pub mod dwmac;
mod virtio_net;

/// Uma placa de rede: virtio-net (QEMU) ou o Ethernet da Amlogic (box).
pub trait Placa {
    fn mac(&self) -> [u8; 6];
    fn receber(&mut self, destino: &mut [u8; virtio_net::MAX_QUADRO]) -> Option<usize>;
    /// Junta os pedacos num quadro e manda. false = sem lugar agora.
    fn enviar(&mut self, partes: &[&[u8]]) -> bool;
    fn encerrar(self: alloc::boxed::Box<Self>);
}

impl Placa for virtio_net::VirtioNet {
    fn mac(&self) -> [u8; 6] {
        virtio_net::VirtioNet::mac(self)
    }
    fn receber(&mut self, destino: &mut [u8; virtio_net::MAX_QUADRO]) -> Option<usize> {
        virtio_net::VirtioNet::receber(self, destino)
    }
    fn enviar(&mut self, partes: &[&[u8]]) -> bool {
        virtio_net::VirtioNet::enviar(self, partes)
    }
    fn encerrar(self: alloc::boxed::Box<Self>) {
        virtio_net::VirtioNet::encerrar(*self)
    }
}

/// A primeira placa que ligar.
fn achar_placa() -> Result<alloc::boxed::Box<dyn Placa>, ErroRede> {
    match virtio_net::VirtioNet::iniciar() {
        Ok(nic) => Ok(alloc::boxed::Box::new(nic)),
        #[cfg(any(feature = "mxq", feature = "rk322x"))]
        Err(_) => Ok(alloc::boxed::Box::new(dwmac::Dwmac::iniciar()?)),
        #[cfg(not(any(feature = "mxq", feature = "rk322x")))]
        Err(e) => Err(e),
    }
}

use alloc::string::String;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::{cpu, irq};

pub use comandos::comando;

/// Erros da rede, do driver ao HTTP.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ErroRede {
    /// Sem placa de rede (ou ela nao foi iniciada).
    SemRede,
    /// Ninguem respondeu a tempo.
    TempoEsgotado,
    /// O outro lado recusou a conexao (RST no SYN).
    Recusado,
    /// O DNS nao achou o nome.
    NaoResolvido,
    /// URL mal formada.
    UrlInvalida,
    /// So falamos http:// (sem TLS).
    SoHttp,
    /// Resposta que nao entendemos, ou falha do driver.
    Protocolo(&'static str),
}

impl ErroRede {
    pub fn descricao(&self) -> &'static str {
        match self {
            ErroRede::SemRede => "rede desligada (sem virtio-net)",
            ErroRede::TempoEsgotado => "tempo esgotado",
            ErroRede::Recusado => "conexão recusada",
            ErroRede::NaoResolvido => "nome não encontrado",
            ErroRede::UrlInvalida => "URL inválida",
            ErroRede::SoHttp => "endereço precisa começar com http:// ou https://",
            ErroRede::Protocolo(motivo) => motivo,
        }
    }
}

/// Fotografia da configuracao e dos contadores.
#[derive(Clone, Copy)]
pub struct InfoRede {
    pub mac: [u8; 6],
    pub ip: [u8; 4],
    pub mascara: [u8; 4],
    pub gateway: [u8; 4],
    pub dns: [u8; 4],
    pub pacotes_rx: u64,
    pub pacotes_tx: u64,
}

/// Resposta de um GET.
pub struct Resposta {
    pub status: u16,
    pub cabecalhos: Vec<(String, String)>,
    pub corpo: Vec<u8>,
    /// Endereco que respondeu, depois dos redirecionamentos
    /// ("http://host[:porta]/caminho"): a base dos links relativos.
    pub url_final: String,
}

impl Resposta {
    /// Valor do cabecalho `nome` (sem diferenciar maiusculas).
    pub fn cabecalho(&self, nome: &str) -> Option<&str> {
        http::achar_cabecalho(&self.cabecalhos, nome)
    }
}

// ---------------- estado global ----------------

/// Tudo que a pilha sabe. Cada camada acrescenta metodos a ela no proprio
/// arquivo (impl Pilha em arp.rs, ipv4.rs, tcp.rs...).
struct Pilha {
    nic: alloc::boxed::Box<dyn Placa>,
    mac: [u8; 6],
    ip: [u8; 4],
    mascara: [u8; 4],
    gateway: [u8; 4],
    dns: [u8; 4],
    arp: arp::Tabela,
    /// Pacotes IP esperando o ARP responder.
    espera_arp: Vec<ipv4::Pendente>,
    udp: udp::Caixa,
    icmp: icmp::Ecos,
    tcp: tcp::Conexao,
    /// As conexoes TCP dos sockets dos programas (numero = indice + 1).
    soquetes: Vec<Option<tcp::Conexao>>,
    /// Proxima porta local de uma conexao TCP.
    porta_tcp: u16,
    /// Sockets de ping abertos e o ICMP que chegou para eles.
    ouvintes_icmp: usize,
    icmp_recebidos: soquete::FilaIcmp,
    /// Portas TCP com um socket escutando (listen).
    escutas: Vec<u16>,
    /// Pacotes IP para o nosso proprio IP, entregues no proximo processar.
    loopback: alloc::collections::VecDeque<Vec<u8>>,
    cache_dns: Vec<(String, [u8; 4])>,
    id_ip: u16,
    pacotes_rx: u64,
    pacotes_tx: u64,
}

impl Pilha {
    fn nova(nic: alloc::boxed::Box<dyn Placa>) -> Pilha {
        let semente = cpu::contador();
        Pilha {
            mac: nic.mac(),
            nic,
            ip: [0; 4],
            mascara: [0; 4],
            gateway: [0; 4],
            dns: [0; 4],
            arp: arp::Tabela::nova(),
            espera_arp: Vec::new(),
            udp: udp::Caixa::nova(),
            icmp: icmp::Ecos::novo(),
            tcp: tcp::Conexao::nova(semente),
            soquetes: Vec::new(),
            porta_tcp: tcp::porta_inicial(semente),
            ouvintes_icmp: 0,
            icmp_recebidos: alloc::collections::VecDeque::new(),
            escutas: Vec::new(),
            loopback: alloc::collections::VecDeque::new(),
            cache_dns: Vec::new(),
            id_ip: semente as u16,
            pacotes_rx: 0,
            pacotes_tx: 0,
        }
    }

    /// Esvazia a fila de recebimento e cuida dos temporizadores.
    fn processar(&mut self) {
        let mut quadro = [0u8; virtio_net::MAX_QUADRO];
        while let Some(tamanho) = self.nic.receber(&mut quadro) {
            self.pacotes_rx += 1;
            if self.pacotes_rx % 8 == 0 {
                // a hora exata em que os pacotes chegam alimenta o gerador do TLS
                crate::cripto::acaso::misturar(&cpu::contador().to_le_bytes());
            }
            self.receber_quadro(&quadro[..tamanho]);
        }
        // o que mandamos para nos mesmos (cada resposta pode gerar outra)
        for _ in 0..256 {
            let Some(pacote) = self.loopback.pop_front() else { break };
            self.receber_ipv4(&pacote);
        }
        self.tcp_temporizar();
        self.expirar_espera_arp();
    }

    fn info(&self) -> InfoRede {
        InfoRede {
            mac: self.mac,
            ip: self.ip,
            mascara: self.mascara,
            gateway: self.gateway,
            dns: self.dns,
            pacotes_rx: self.pacotes_rx,
            pacotes_tx: self.pacotes_tx,
        }
    }
}

/// A pilha e a trava dela. So o laco principal usa (nunca a interrupcao),
/// entao a trava nunca e disputada de verdade: se estiver pega e porque
/// alguem chamou a rede de dentro da propria rede, e devolvemos erro em vez
/// de travar a maquina.
struct Global {
    trava: AtomicBool,
    pilha: UnsafeCell<Option<Pilha>>,
}

unsafe impl Sync for Global {}

static REDE: Global = Global { trava: AtomicBool::new(false), pilha: UnsafeCell::new(None) };

/// Por que a rede nao ligou da ultima vez (o comando `ip` mostra).
struct UltimoErro(UnsafeCell<Option<ErroRede>>);

unsafe impl Sync for UltimoErro {}

static ULTIMO_ERRO: UltimoErro = UltimoErro(UnsafeCell::new(None));

pub fn ultimo_erro() -> Option<ErroRede> {
    unsafe { *ULTIMO_ERRO.0.get() }
}

const OCUPADA: ErroRede = ErroRede::Protocolo("rede ocupada (chamada dentro dela mesma)");

/// Roda `acao` com a trava pega sobre o Option da pilha.
fn com_global<R>(acao: impl FnOnce(&mut Option<Pilha>) -> R) -> Result<R, ErroRede> {
    if REDE.trava.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        return Err(OCUPADA);
    }
    let resultado = acao(unsafe { &mut *REDE.pilha.get() });
    REDE.trava.store(false, Ordering::Release);
    Ok(resultado)
}

/// Roda `acao` com a pilha. SemRede se a rede nao foi iniciada.
fn com<R>(acao: impl FnOnce(&mut Pilha) -> R) -> Result<R, ErroRede> {
    com_global(|pilha| pilha.as_mut().map(acao))?.ok_or(ErroRede::SemRede)
}

// ---------------- tempo ----------------

/// Milissegundos desde que a maquina ligou.
fn agora_ms() -> u64 {
    cpu::contador() / (cpu::frequencia_timer() / 1000).max(1)
}

/// Repete `passo` (com a pilha ja processada) ate ele devolver algo ou
/// passarem `limite_ms`. Entre uma tentativa e outra solta a trava e dorme.
fn esperar<T>(limite_ms: u64, mut passo: impl FnMut(&mut Pilha) -> Option<T>) -> Result<T, ErroRede> {
    let fim = agora_ms().saturating_add(limite_ms);
    loop {
        let resultado = com(|p| {
            p.processar();
            passo(p)
        })?;
        if let Some(valor) = resultado {
            return Ok(valor);
        }
        if agora_ms() >= fim {
            return Err(ErroRede::TempoEsgotado);
        }
        irq::cochilar();
    }
}

// ---------------- API publica ----------------

/// Liga o virtio-net e pega a configuracao por DHCP (uns 3 s no maximo).
/// Se o DHCP falhar a placa e desligada e a rede fica desligada.
pub fn iniciar() -> Result<InfoRede, ErroRede> {
    let r = iniciar_de_fato();
    unsafe { *ULTIMO_ERRO.0.get() = r.err() };
    r
}

fn iniciar_de_fato() -> Result<InfoRede, ErroRede> {
    if let Ok(info) = info_ou_erro() {
        return Ok(info); // ja estava ligada
    }
    let nic = achar_placa()?;
    com_global(|global| *global = Some(Pilha::nova(nic)))?;
    if let Ok(mac) = com(|p| p.mac) {
        crate::cripto::acaso::misturar(&mac);
    }
    crate::cripto::acaso::misturar(&cpu::contador().to_le_bytes());
    if let Err(e) = dhcp::configurar() {
        if let Ok(Some(pilha)) = com_global(|global| global.take()) {
            pilha.nic.encerrar();
        }
        return Err(if e == ErroRede::TempoEsgotado { ErroRede::Protocolo("o cabo tem link, mas o roteador não respondeu ao DHCP") } else { e });
    }
    // a hora certa para os certificados do HTTPS (sem NTP fica a do RTC)
    let _ = ntp::acertar();
    info_ou_erro()
}

/// Nao bloqueia: trata o que chegou (ARP, ping, TCP) e os temporizadores.
/// Pode ser chamado a vontade; sem rede volta na hora.
pub fn processar() {
    let _ = com(|p| p.processar());
}

pub fn info() -> Option<InfoRede> {
    info_ou_erro().ok()
}

fn info_ou_erro() -> Result<InfoRede, ErroRede> {
    com(|p| p.info())
}

/// IP de um nome (registro A). IP com pontos ("10.0.2.2") volta direto.
pub fn resolver(nome: &str) -> Result<[u8; 4], ErroRede> {
    dns::resolver(nome)
}

/// Manda um echo ICMP e devolve o tempo de ida e volta em ms.
pub fn ping(ip: [u8; 4], tempo_limite_ms: u64) -> Result<u64, ErroRede> {
    icmp::ping(ip, tempo_limite_ms)
}

/// GET em uma URL http:// (segue ate 5 redirecionamentos).
/// `tempo_limite_ms` vale para conectar e para cada pausa sem dados.
pub fn http_get(url: &str, tempo_limite_ms: u64) -> Result<Resposta, ErroRede> {
    http::get(url, tempo_limite_ms)
}

/// POST com cabecalhos extras (APIs JSON).
pub fn http_post(url: &str, cabecalhos: &[(&str, &str)], corpo: &[u8], tempo_limite_ms: u64) -> Result<Resposta, ErroRede> {
    http::post(url, cabecalhos, corpo, tempo_limite_ms)
}

// ---------------- utilitarios das camadas ----------------

fn ler16(b: &[u8], i: usize) -> u16 {
    u16::from_be_bytes([b[i], b[i + 1]])
}

fn ler32(b: &[u8], i: usize) -> u32 {
    u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

fn escrever16(b: &mut [u8], i: usize, valor: u16) {
    b[i..i + 2].copy_from_slice(&valor.to_be_bytes());
}

fn escrever32(b: &mut [u8], i: usize, valor: u32) {
    b[i..i + 4].copy_from_slice(&valor.to_be_bytes());
}

fn ip4(b: &[u8], i: usize) -> [u8; 4] {
    [b[i], b[i + 1], b[i + 2], b[i + 3]]
}

/// Checksum da internet (RFC 1071) sobre os pedacos colados um no outro.
/// Um pedaco de tamanho impar continua no proximo, como se fosse um so.
/// Sobre um pacote que ja tem o checksum certo, o resultado e 0.
fn checksum(partes: &[&[u8]]) -> u16 {
    let mut soma: u64 = 0;
    let mut sobra: Option<u8> = None;
    for parte in partes {
        let mut dados = *parte;
        if let Some(alto) = sobra.take() {
            match dados.split_first() {
                Some((&baixo, resto)) => {
                    soma += u16::from_be_bytes([alto, baixo]) as u64;
                    dados = resto;
                }
                None => {
                    sobra = Some(alto);
                    continue;
                }
            }
        }
        let (pares, resto) = dados.as_chunks::<2>();
        for par in pares {
            soma += u16::from_be_bytes(*par) as u64;
        }
        if let [ultimo] = resto {
            sobra = Some(*ultimo);
        }
    }
    if let Some(alto) = sobra {
        soma += (alto as u64) << 8;
    }
    while soma >> 16 != 0 {
        soma = (soma & 0xFFFF) + (soma >> 16);
    }
    !(soma as u16)
}

/// Pseudo-cabecalho do UDP e do TCP: origem, destino, 0, protocolo, tamanho.
fn pseudo_cabecalho(origem: [u8; 4], destino: [u8; 4], protocolo: u8, tamanho: usize) -> [u8; 12] {
    let mut p = [0u8; 12];
    p[0..4].copy_from_slice(&origem);
    p[4..8].copy_from_slice(&destino);
    p[9] = protocolo;
    escrever16(&mut p, 10, tamanho as u16);
    p
}

/// "10.0.2.15" -> [10, 0, 2, 15].
fn ler_ip(texto: &str) -> Option<[u8; 4]> {
    let mut ip = [0u8; 4];
    let mut partes = texto.split('.');
    for byte in &mut ip {
        let parte = partes.next()?;
        if parte.is_empty() || parte.len() > 3 || !parte.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        *byte = parte.parse().ok()?;
    }
    partes.next().is_none().then_some(ip)
}

/// Mostra [10, 0, 2, 15] como 10.0.2.15.
pub struct Ip(pub [u8; 4]);

impl core::fmt::Display for Ip {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        let [a, b, c, d] = self.0;
        f.pad(&alloc::format!("{a}.{b}.{c}.{d}"))
    }
}

/// Mostra um MAC como 52:54:00:12:34:56.
pub struct Mac(pub [u8; 6]);

impl core::fmt::Display for Mac {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        let m = self.0;
        write!(f, "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", m[0], m[1], m[2], m[3], m[4], m[5])
    }
}
