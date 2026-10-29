//! Cliente TLS 1.3 (RFC 8446), do zero, sobre a cripto de src/cripto.
//!
//! - suites: ChaCha20-Poly1305 primeiro (o Cortex-A7 nao tem AES em
//!   hardware), depois AES-128-GCM e AES-256-GCM;
//! - troca de chaves: X25519 no primeiro ClientHello; P-256/P-384 se o
//!   servidor pedir (HelloRetryRequest);
//! - certificados: cadeia ate as raizes da Mozilla (cripto::x509), RSA
//!   (PKCS#1 e PSS) e ECDSA;
//! - sem retomada de sessao (0-RTT/PSK): toda conexao faz o aperto de mao
//!   completo.
//!
//! O transporte (o TCP do kernel, ou um TcpStream nos testes do PC) fica
//! atras do trait Transporte: este arquivo nao sabe de rede.

use alloc::vec;
use alloc::vec::Vec;

use crate::cripto::aes::AesGcm;
use crate::cripto::ec::{Contexto, Curva};
use crate::cripto::x509::{self, Certificado, Esquema};
use crate::cripto::{Hash, acaso, chacha, hmac, x25519};

pub trait Transporte {
    fn enviar(&mut self, dados: &[u8]) -> Result<(), Erro>;
    /// Acrescenta em `buf` o que chegar (espera chegar algo). Ok(false) =
    /// a conexao fechou.
    fn receber(&mut self, buf: &mut Vec<u8>) -> Result<bool, Erro>;
    /// Entropia para o gerador (o contador de ciclos, por exemplo).
    fn entropia(&mut self) -> u64;
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Erro {
    Rede(&'static str),
    Protocolo(&'static str),
    /// O servidor mandou um alerta (o codigo do RFC).
    Alerta(u8),
    Certificado(x509::Erro),
    Fechou,
}

impl Erro {
    pub fn descricao(self) -> &'static str {
        match self {
            Erro::Rede(s) | Erro::Protocolo(s) => s,
            Erro::Alerta(40) => "o servidor recusou o aperto de mão TLS",
            Erro::Alerta(70) => "o servidor não fala TLS 1.3",
            Erro::Alerta(_) => "o servidor mandou um alerta TLS",
            Erro::Certificado(e) => e.descricao(),
            Erro::Fechou => "a conexão fechou no meio do TLS",
        }
    }
}

const P: Erro = Erro::Protocolo("resposta TLS inválida");

// tipos de registro
const MUDAR_CIFRA: u8 = 20;
const ALERTA: u8 = 21;
const APERTO: u8 = 22;
const DADOS: u8 = 23;

// mensagens do aperto de mao
const CLIENT_HELLO: u8 = 1;
const SERVER_HELLO: u8 = 2;
const NOVO_TICKET: u8 = 4;
const EXTENSOES_CIFRADAS: u8 = 8;
const CERTIFICADO: u8 = 11;
const PEDIDO_CERTIFICADO: u8 = 13;
const VERIFICA_CERTIFICADO: u8 = 15;
const FINALIZADO: u8 = 20;
const ATUALIZA_CHAVE: u8 = 24;

const X25519: u16 = 0x001d;
const SECP256R1: u16 = 0x0017;
const SECP384R1: u16 = 0x0018;

const CHACHA20_POLY1305_SHA256: u16 = 0x1303;
const AES_128_GCM_SHA256: u16 = 0x1301;
const AES_256_GCM_SHA384: u16 = 0x1302;

const MAX_FRAGMENTO: usize = 16384;

/// random do ServerHello que na verdade e um HelloRetryRequest
const HRR: [u8; 32] = [
    0xCF, 0x21, 0xAD, 0x74, 0xE5, 0x9A, 0x61, 0x11, 0xBE, 0x1D, 0x8C, 0x02, 0x1E, 0x65, 0xB8, 0x91, 0xC2, 0xA2, 0x11,
    0x16, 0x7A, 0xBB, 0x8C, 0x5E, 0x07, 0x9E, 0x09, 0xE2, 0xC8, 0xA8, 0x33, 0x9C,
];

// ------------------------------------------------------------ utilidades

fn u16_be(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

fn u24_be(b: &[u8]) -> usize {
    ((b[0] as usize) << 16) | ((b[1] as usize) << 8) | b[2] as usize
}

/// Le campos com tamanho na frente sem estourar.
struct Leitura<'a> {
    d: &'a [u8],
}

impl<'a> Leitura<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], Erro> {
        if self.d.len() < n {
            return Err(P);
        }
        let (a, b) = self.d.split_at(n);
        self.d = b;
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8, Erro> {
        Ok(self.bytes(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, Erro> {
        Ok(u16_be(self.bytes(2)?))
    }
    fn vetor8(&mut self) -> Result<&'a [u8], Erro> {
        let n = self.u8()? as usize;
        self.bytes(n)
    }
    fn vetor16(&mut self) -> Result<&'a [u8], Erro> {
        let n = self.u16()? as usize;
        self.bytes(n)
    }
    fn vetor24(&mut self) -> Result<&'a [u8], Erro> {
        let n = u24_be(self.bytes(3)?);
        self.bytes(n)
    }
}

fn por16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_be_bytes());
}

/// Acrescenta `conteudo` com o tamanho (2 bytes) na frente.
fn com_tamanho16(v: &mut Vec<u8>, conteudo: &[u8]) {
    por16(v, conteudo.len() as u16);
    v.extend_from_slice(conteudo);
}

fn extensao(v: &mut Vec<u8>, tipo: u16, conteudo: &[u8]) {
    por16(v, tipo);
    com_tamanho16(v, conteudo);
}

fn mensagem(tipo: u8, corpo: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(corpo.len() + 4);
    m.push(tipo);
    m.extend_from_slice(&(corpo.len() as u32).to_be_bytes()[1..]);
    m.extend_from_slice(corpo);
    m
}

// ----------------------------------------------------------- agenda de chaves

/// HKDF-Expand-Label do TLS 1.3.
fn expandir_rotulo(h: Hash, segredo: &[u8], rotulo: &str, contexto: &[u8], tamanho: usize) -> Vec<u8> {
    let mut info = Vec::with_capacity(10 + rotulo.len() + contexto.len());
    por16(&mut info, tamanho as u16);
    info.push((6 + rotulo.len()) as u8);
    info.extend_from_slice(b"tls13 ");
    info.extend_from_slice(rotulo.as_bytes());
    info.push(contexto.len() as u8);
    info.extend_from_slice(contexto);
    h.expandir(segredo, &info, tamanho)
}

fn derivar(h: Hash, segredo: &[u8], rotulo: &str, transcricao: &[u8]) -> Vec<u8> {
    expandir_rotulo(h, segredo, rotulo, &h.resumir(transcricao), h.tamanho())
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Suite {
    codigo: u16,
    hash: Hash,
    chave: usize,
}

fn suite(codigo: u16) -> Option<Suite> {
    Some(match codigo {
        CHACHA20_POLY1305_SHA256 => Suite { codigo, hash: Hash::Sha256, chave: 32 },
        AES_128_GCM_SHA256 => Suite { codigo, hash: Hash::Sha256, chave: 16 },
        AES_256_GCM_SHA384 => Suite { codigo, hash: Hash::Sha384, chave: 32 },
        _ => return None,
    })
}

enum Cifra {
    ChaCha([u8; 32]),
    Aes(AesGcm),
}

/// As chaves de um sentido da conexao.
struct Chaves {
    cifra: Cifra,
    iv: [u8; 12],
    seq: u64,
    segredo: Vec<u8>,
}

impl Chaves {
    fn novas(s: Suite, segredo: Vec<u8>) -> Chaves {
        let chave = expandir_rotulo(s.hash, &segredo, "key", &[], s.chave);
        let iv_v = expandir_rotulo(s.hash, &segredo, "iv", &[], 12);
        let mut iv = [0u8; 12];
        iv.copy_from_slice(&iv_v);
        let cifra = if s.codigo == CHACHA20_POLY1305_SHA256 {
            let mut k = [0u8; 32];
            k.copy_from_slice(&chave);
            Cifra::ChaCha(k)
        } else {
            Cifra::Aes(AesGcm::novo(&chave).expect("tamanho de chave AES"))
        };
        Chaves { cifra, iv, seq: 0, segredo }
    }

    fn nonce(&mut self) -> [u8; 12] {
        let mut n = self.iv;
        for (i, b) in self.seq.to_be_bytes().iter().enumerate() {
            n[4 + i] ^= b;
        }
        self.seq += 1;
        n
    }

    /// Registro cifrado inteiro (cabecalho + dados + etiqueta).
    fn selar(&mut self, tipo: u8, dados: &[u8]) -> Vec<u8> {
        let tamanho = dados.len() + 1 + 16;
        let mut r = Vec::with_capacity(5 + tamanho);
        r.extend_from_slice(&[DADOS, 3, 3]);
        por16(&mut r, tamanho as u16);
        r.extend_from_slice(dados);
        r.push(tipo);
        let nonce = self.nonce();
        let (cab, corpo) = r.split_at_mut(5);
        let etiqueta = match &self.cifra {
            Cifra::ChaCha(k) => chacha::selar(k, &nonce, cab, corpo),
            Cifra::Aes(g) => g.selar(&nonce, cab, corpo),
        };
        r.extend_from_slice(&etiqueta);
        r
    }

    /// Decifra um registro (cabecalho de 5 bytes + o resto). Devolve o
    /// tipo de dentro e o conteudo.
    fn abrir(&mut self, cab: &[u8], corpo: &mut Vec<u8>) -> Result<u8, Erro> {
        if corpo.len() < 17 {
            return Err(P);
        }
        let n = corpo.len() - 16;
        let mut etiqueta = [0u8; 16];
        etiqueta.copy_from_slice(&corpo[n..]);
        corpo.truncate(n);
        let nonce = self.nonce();
        let ok = match &self.cifra {
            Cifra::ChaCha(k) => chacha::abrir(k, &nonce, cab, corpo, &etiqueta),
            Cifra::Aes(g) => g.abrir(&nonce, cab, corpo, &etiqueta),
        };
        if !ok {
            return Err(Erro::Protocolo("registro TLS adulterado"));
        }
        // tira os zeros do fim; o ultimo byte nao zero e o tipo
        while corpo.last() == Some(&0) {
            corpo.pop();
        }
        corpo.pop().ok_or(P)
    }

    /// KeyUpdate: segredo novo = HKDF-Expand-Label(antigo, "traffic upd").
    fn atualizar(&mut self, s: Suite) {
        let novo = expandir_rotulo(s.hash, &self.segredo, "traffic upd", &[], s.hash.tamanho());
        *self = Chaves::novas(s, novo);
    }
}

// ----------------------------------------------------------------- conexao

/// Uma conexao TLS pronta para os dados da aplicacao.
pub struct Sessao {
    suite: Suite,
    escrita: Chaves,
    leitura: Chaves,
    /// Bytes que chegaram e ainda nao formam um registro inteiro.
    entrada: Vec<u8>,
    /// Mensagens de aperto de mao que chegaram depois (tickets...).
    aperto: Vec<u8>,
    fechou: bool,
    /// Os certificados que o servidor mandou (o do site primeiro), ja
    /// conferidos: para mostrar no cadeado.
    pub cadeia: Vec<Vec<u8>>,
}

/// O estado durante o aperto de mao.
struct Aperto<'t, T: Transporte> {
    t: &'t mut T,
    entrada: Vec<u8>,
    mensagens: Vec<u8>,
    leitura: Option<Chaves>,
}

/// Le um registro inteiro de `entrada` (esperando chegar mais se preciso).
fn ler_registro<T: Transporte>(t: &mut T, entrada: &mut Vec<u8>) -> Result<([u8; 5], Vec<u8>), Erro> {
    loop {
        if entrada.len() >= 5 {
            let n = u16_be(&entrada[3..5]) as usize;
            if n > MAX_FRAGMENTO + 256 {
                return Err(Erro::Protocolo("registro TLS grande demais"));
            }
            if entrada.len() >= 5 + n {
                let mut cab = [0u8; 5];
                cab.copy_from_slice(&entrada[..5]);
                let corpo = entrada[5..5 + n].to_vec();
                entrada.drain(..5 + n);
                return Ok((cab, corpo));
            }
        }
        if !t.receber(entrada)? {
            return Err(Erro::Fechou);
        }
    }
}

fn alerta(corpo: &[u8]) -> Erro {
    match corpo {
        [_, 0] => Erro::Fechou,
        [_, codigo] => Erro::Alerta(*codigo),
        _ => P,
    }
}

impl<T: Transporte> Aperto<'_, T> {
    /// A proxima mensagem do aperto de mao (tipo, mensagem inteira com o
    /// cabecalho de 4 bytes).
    fn proxima(&mut self) -> Result<(u8, Vec<u8>), Erro> {
        loop {
            if self.mensagens.len() >= 4 {
                let n = 4 + u24_be(&self.mensagens[1..4]);
                if self.mensagens.len() >= n {
                    let m: Vec<u8> = self.mensagens.drain(..n).collect();
                    return Ok((m[0], m));
                }
            }
            let (cab, mut corpo) = ler_registro(self.t, &mut self.entrada)?;
            let tipo = match (&mut self.leitura, cab[0]) {
                (_, MUDAR_CIFRA) => continue, // compatibilidade: ignorar
                (Some(k), DADOS) => k.abrir(&cab, &mut corpo)?,
                (None, t) => t,
                (Some(_), _) => return Err(P),
            };
            match tipo {
                APERTO => self.mensagens.extend_from_slice(&corpo),
                ALERTA => return Err(alerta(&corpo)),
                _ => return Err(P),
            }
        }
    }
}

/// Um par de chaves efemero para um grupo.
enum Efemera {
    X25519([u8; 32]),
    Nist(Curva, Vec<u8>),
}

impl Efemera {
    fn nova<T: Transporte>(t: &mut T, grupo: u16) -> Result<Efemera, Erro> {
        let mut s = [0u8; 48];
        acaso::preencher(&mut s, t.entropia());
        match grupo {
            X25519 => {
                let mut k = [0u8; 32];
                k.copy_from_slice(&s[..32]);
                Ok(Efemera::X25519(k))
            }
            SECP256R1 | SECP384R1 => {
                let (curva, n) = if grupo == SECP256R1 { (Curva::P256, 32) } else { (Curva::P384, 48) };
                s[0] &= 0x7F; // menor que a ordem da curva
                s[0] |= 0x40;
                Ok(Efemera::Nist(curva, s[..n].to_vec()))
            }
            _ => Err(Erro::Protocolo("o servidor pediu um grupo desconhecido")),
        }
    }

    fn grupo(&self) -> u16 {
        match self {
            Efemera::X25519(_) => X25519,
            Efemera::Nist(Curva::P256, _) => SECP256R1,
            Efemera::Nist(Curva::P384, _) => SECP384R1,
        }
    }

    fn publica(&self) -> Vec<u8> {
        match self {
            Efemera::X25519(k) => x25519::publica(k).to_vec(),
            Efemera::Nist(c, k) => Contexto::novo(*c).publica(k).unwrap_or_default(),
        }
    }

    fn segredo(&self, outra: &[u8]) -> Result<Vec<u8>, Erro> {
        let ruim = Erro::Protocolo("chave do servidor inválida");
        match self {
            Efemera::X25519(k) => {
                let o: [u8; 32] = outra.try_into().map_err(|_| ruim)?;
                let s = x25519::multiplicar_ponto(k, &o);
                if s == [0; 32] {
                    return Err(ruim);
                }
                Ok(s.to_vec())
            }
            Efemera::Nist(c, k) => Contexto::novo(*c).segredo(k, outra).ok_or(ruim),
        }
    }
}

fn client_hello(host: &str, aleatorio: &[u8; 32], sessao: &[u8; 32], chave: &Efemera, cookie: Option<&[u8]>) -> Vec<u8> {
    let mut c = Vec::with_capacity(512);
    por16(&mut c, 0x0303);
    c.extend_from_slice(aleatorio);
    c.push(32);
    c.extend_from_slice(sessao);
    let suites = [CHACHA20_POLY1305_SHA256, AES_128_GCM_SHA256, AES_256_GCM_SHA384];
    por16(&mut c, 2 * suites.len() as u16);
    for s in suites {
        por16(&mut c, s);
    }
    c.extend_from_slice(&[1, 0]); // compressao: nenhuma

    let mut ext = Vec::with_capacity(256);
    // server_name
    let mut sni = Vec::new();
    por16(&mut sni, (host.len() + 3) as u16);
    sni.push(0);
    com_tamanho16(&mut sni, host.as_bytes());
    extensao(&mut ext, 0, &sni);
    // supported_groups
    let mut grupos = Vec::new();
    por16(&mut grupos, 6);
    for g in [X25519, SECP256R1, SECP384R1] {
        por16(&mut grupos, g);
    }
    extensao(&mut ext, 10, &grupos);
    // signature_algorithms
    let algs: [u16; 9] = [0x0403, 0x0503, 0x0603, 0x0804, 0x0805, 0x0806, 0x0401, 0x0501, 0x0601];
    let mut a = Vec::new();
    por16(&mut a, 2 * algs.len() as u16);
    for x in algs {
        por16(&mut a, x);
    }
    extensao(&mut ext, 13, &a);
    // ALPN: so HTTP/1.1
    extensao(&mut ext, 16, &[0, 9, 8, b'h', b't', b't', b'p', b'/', b'1', b'.', b'1']);
    // supported_versions: TLS 1.3
    extensao(&mut ext, 43, &[2, 3, 4]);
    // cookie (se o HelloRetryRequest mandou)
    if let Some(k) = cookie {
        let mut v = Vec::new();
        com_tamanho16(&mut v, k);
        extensao(&mut ext, 44, &v);
    }
    // key_share
    let publica = chave.publica();
    let mut ks = Vec::new();
    por16(&mut ks, (publica.len() + 4) as u16);
    por16(&mut ks, chave.grupo());
    com_tamanho16(&mut ks, &publica);
    extensao(&mut ext, 51, &ks);

    com_tamanho16(&mut c, &ext);
    mensagem(CLIENT_HELLO, &c)
}

/// Registros de texto aberto (so o ClientHello).
fn enviar_aberto<T: Transporte>(t: &mut T, tipo: u8, dados: &[u8]) -> Result<(), Erro> {
    let mut r = Vec::with_capacity(dados.len() + 5);
    r.extend_from_slice(&[tipo, 3, 1]);
    com_tamanho16(&mut r, dados);
    t.enviar(&r)
}

struct ServerHello<'a> {
    hrr: bool,
    suite: u16,
    grupo: u16,
    chave: &'a [u8],
    cookie: Option<&'a [u8]>,
}

fn ler_server_hello(m: &[u8]) -> Result<ServerHello<'_>, Erro> {
    let mut l = Leitura { d: &m[4..] };
    l.u16()?;
    let aleatorio = l.bytes(32)?;
    l.vetor8()?;
    let suite = l.u16()?;
    l.u8()?;
    let hrr = aleatorio == HRR;
    let mut sh = ServerHello { hrr, suite, grupo: 0, chave: &[], cookie: None };
    let mut versao = 0;
    let mut ext = Leitura { d: l.vetor16()? };
    while !ext.d.is_empty() {
        let tipo = ext.u16()?;
        let mut v = Leitura { d: ext.vetor16()? };
        match tipo {
            43 => versao = v.u16()?,
            51 => {
                sh.grupo = v.u16()?;
                if !hrr {
                    sh.chave = v.vetor16()?;
                }
            }
            44 => sh.cookie = Some(v.vetor16()?),
            _ => {}
        }
    }
    if versao != 0x0304 {
        return Err(Erro::Alerta(70));
    }
    Ok(sh)
}

fn esquema(codigo: u16) -> Option<Esquema> {
    Some(match codigo {
        0x0403 => Esquema::Ecdsa(Hash::Sha256),
        0x0503 => Esquema::Ecdsa(Hash::Sha384),
        0x0603 => Esquema::Ecdsa(Hash::Sha512),
        0x0804 => Esquema::RsaPss(Hash::Sha256),
        0x0805 => Esquema::RsaPss(Hash::Sha384),
        0x0806 => Esquema::RsaPss(Hash::Sha512),
        _ => return None,
    })
}

/// Aperto de mao completo com `host`. `agora`: segundos desde 1970 (0 =
/// nao conferir a validade dos certificados).
pub fn conectar<T: Transporte>(t: &mut T, host: &str, agora: i64) -> Result<Sessao, Erro> {
    let mut aleatorio = [0u8; 32];
    let mut id_sessao = [0u8; 32];
    acaso::preencher(&mut aleatorio, t.entropia());
    acaso::preencher(&mut id_sessao, t.entropia());

    let mut efemera = Efemera::nova(t, X25519)?;
    let ch1 = client_hello(host, &aleatorio, &id_sessao, &efemera, None);
    enviar_aberto(t, APERTO, &ch1)?;

    let mut a = Aperto { t, entrada: Vec::new(), mensagens: Vec::new(), leitura: None };
    let mut transcricao = ch1;

    let (tipo, mut sh_msg) = a.proxima()?;
    if tipo != SERVER_HELLO {
        return Err(P);
    }
    let mut sh = ler_server_hello(&sh_msg)?;
    if sh.hrr {
        // o servidor quer outro grupo: transcricao = message_hash(CH1) + HRR
        let s = suite(sh.suite).ok_or(Erro::Protocolo("o servidor escolheu uma cifra desconhecida"))?;
        let mut nova = vec![254, 0, 0, s.hash.tamanho() as u8];
        nova.extend_from_slice(&s.hash.resumir(&transcricao));
        nova.extend_from_slice(&sh_msg);
        transcricao = nova;
        efemera = Efemera::nova(a.t, sh.grupo)?;
        let cookie = sh.cookie.map(|c| c.to_vec());
        let ch2 = client_hello(host, &aleatorio, &id_sessao, &efemera, cookie.as_deref());
        enviar_aberto(a.t, APERTO, &ch2)?;
        transcricao.extend_from_slice(&ch2);
        let (tipo, m) = a.proxima()?;
        if tipo != SERVER_HELLO {
            return Err(P);
        }
        sh_msg = m;
        sh = ler_server_hello(&sh_msg)?;
        if sh.hrr {
            return Err(Erro::Protocolo("dois HelloRetryRequest"));
        }
    }
    let s = suite(sh.suite).ok_or(Erro::Protocolo("o servidor escolheu uma cifra desconhecida"))?;
    if sh.grupo != efemera.grupo() {
        return Err(P);
    }
    let compartilhado = efemera.segredo(sh.chave)?;
    transcricao.extend_from_slice(&sh_msg);

    // agenda de chaves
    let h = s.hash;
    let zeros = vec![0u8; h.tamanho()];
    let cedo = h.extrair(&[], &zeros);
    let derivado = derivar(h, &cedo, "derived", &[]);
    let segredo_aperto = h.extrair(&derivado, &compartilhado);
    let c_aperto = derivar(h, &segredo_aperto, "c hs traffic", &transcricao);
    let s_aperto = derivar(h, &segredo_aperto, "s hs traffic", &transcricao);
    a.leitura = Some(Chaves::novas(s, s_aperto.clone()));
    let mut escrita = Chaves::novas(s, c_aperto.clone());

    // o resto do aperto de mao, cifrado
    let mut cadeia: Vec<Vec<u8>> = Vec::new();
    let mut verificou = false;
    let mut pediu_certificado: Option<Vec<u8>> = None;
    loop {
        let (tipo, m) = a.proxima()?;
        match tipo {
            EXTENSOES_CIFRADAS => {}
            PEDIDO_CERTIFICADO => {
                let mut l = Leitura { d: &m[4..] };
                pediu_certificado = Some(l.vetor8()?.to_vec());
            }
            CERTIFICADO => {
                let mut l = Leitura { d: &m[4..] };
                l.vetor8()?;
                let mut lista = Leitura { d: l.vetor24()? };
                while !lista.d.is_empty() {
                    cadeia.push(lista.vetor24()?.to_vec());
                    lista.vetor16()?;
                }
                let refs: Vec<&[u8]> = cadeia.iter().map(|c| c.as_slice()).collect();
                x509::verificar_cadeia(&refs, host, agora).map_err(Erro::Certificado)?;
            }
            VERIFICA_CERTIFICADO => {
                let folha = cadeia.first().ok_or(P)?;
                let cert = Certificado::ler(folha).map_err(Erro::Certificado)?;
                let mut l = Leitura { d: &m[4..] };
                let esq = esquema(l.u16()?).ok_or(Erro::Protocolo("assinatura TLS desconhecida"))?;
                let assinatura = l.vetor16()?;
                let mut assinado = vec![0x20u8; 64];
                assinado.extend_from_slice(b"TLS 1.3, server CertificateVerify\0");
                assinado.extend_from_slice(&h.resumir(&transcricao));
                if !x509::verificar(&cert.chave, esq, &assinado, assinatura) {
                    return Err(Erro::Certificado(x509::Erro::AssinaturaInvalida));
                }
                verificou = true;
            }
            FINALIZADO => {
                if !verificou {
                    return Err(Erro::Protocolo("o servidor não provou ser dono do certificado"));
                }
                let chave = expandir_rotulo(h, &s_aperto, "finished", &[], h.tamanho());
                let esperado = h.hmac(&chave, &h.resumir(&transcricao));
                if !hmac::iguais(&esperado, &m[4..]) {
                    return Err(Erro::Protocolo("Finished do servidor não confere"));
                }
                transcricao.extend_from_slice(&m);
                break;
            }
            _ => return Err(P),
        }
        transcricao.extend_from_slice(&m);
    }

    // segredos da aplicacao (transcricao ate o Finished do servidor)
    let derivado = derivar(h, &segredo_aperto, "derived", &[]);
    let mestre = h.extrair(&derivado, &zeros);
    let c_app = derivar(h, &mestre, "c ap traffic", &transcricao);
    let s_app = derivar(h, &mestre, "s ap traffic", &transcricao);

    // nosso lado: certificado vazio (se pediram) e o Finished
    if let Some(contexto) = pediu_certificado {
        let mut corpo = vec![contexto.len() as u8];
        corpo.extend_from_slice(&contexto);
        corpo.extend_from_slice(&[0, 0, 0]);
        let m = mensagem(CERTIFICADO, &corpo);
        a.t.enviar(&escrita.selar(APERTO, &m))?;
        transcricao.extend_from_slice(&m);
    }
    let chave = expandir_rotulo(h, &c_aperto, "finished", &[], h.tamanho());
    let fim = mensagem(FINALIZADO, &h.hmac(&chave, &h.resumir(&transcricao)));
    a.t.enviar(&escrita.selar(APERTO, &fim))?;

    Ok(Sessao {
        suite: s,
        escrita: Chaves::novas(s, c_app),
        leitura: Chaves::novas(s, s_app),
        entrada: a.entrada,
        aperto: a.mensagens,
        fechou: false,
        cadeia,
    })
}

impl Sessao {
    /// Nome da cifra combinada (para mostrar).
    pub fn cifra(&self) -> &'static str {
        match self.suite.codigo {
            CHACHA20_POLY1305_SHA256 => "TLS 1.3 ChaCha20-Poly1305",
            AES_128_GCM_SHA256 => "TLS 1.3 AES-128-GCM",
            _ => "TLS 1.3 AES-256-GCM",
        }
    }

    pub fn enviar<T: Transporte>(&mut self, t: &mut T, dados: &[u8]) -> Result<(), Erro> {
        let mut saida = Vec::with_capacity(dados.len() + 64);
        for pedaco in dados.chunks(MAX_FRAGMENTO) {
            saida.extend_from_slice(&self.escrita.selar(DADOS, pedaco));
        }
        t.enviar(&saida)
    }

    /// Acrescenta em `saida` os dados do proximo registro de aplicacao.
    /// Ok(false) = o servidor fechou (close_notify ou fim do TCP).
    pub fn receber<T: Transporte>(&mut self, t: &mut T, saida: &mut Vec<u8>) -> Result<bool, Erro> {
        loop {
            if self.fechou {
                return Ok(false);
            }
            let (cab, mut corpo) = match ler_registro(t, &mut self.entrada) {
                Ok(r) => r,
                Err(Erro::Fechou) => {
                    self.fechou = true;
                    return Ok(false);
                }
                Err(e) => return Err(e),
            };
            if cab[0] == MUDAR_CIFRA {
                continue;
            }
            if cab[0] != DADOS {
                return Err(P);
            }
            match self.leitura.abrir(&cab, &mut corpo)? {
                DADOS => {
                    saida.extend_from_slice(&corpo);
                    return Ok(true);
                }
                ALERTA => {
                    self.fechou = true;
                    return match alerta(&corpo) {
                        Erro::Fechou => Ok(false),
                        e => Err(e),
                    };
                }
                APERTO => {
                    self.aperto.extend_from_slice(&corpo);
                    self.mensagens_tardias(t)?;
                }
                _ => return Err(P),
            }
        }
    }

    /// Tickets (ignorados: sem retomada) e KeyUpdate.
    fn mensagens_tardias<T: Transporte>(&mut self, t: &mut T) -> Result<(), Erro> {
        while self.aperto.len() >= 4 {
            let n = 4 + u24_be(&self.aperto[1..4]);
            if self.aperto.len() < n {
                break;
            }
            let m: Vec<u8> = self.aperto.drain(..n).collect();
            match m[0] {
                NOVO_TICKET => {}
                ATUALIZA_CHAVE => {
                    self.leitura.atualizar(self.suite);
                    if m.get(4) == Some(&1) {
                        // o servidor pediu que atualizemos tambem
                        let r = mensagem(ATUALIZA_CHAVE, &[0]);
                        let registro = self.escrita.selar(APERTO, &r);
                        t.enviar(&registro)?;
                        self.escrita.atualizar(self.suite);
                    }
                }
                _ => return Err(P),
            }
        }
        Ok(())
    }

    /// Avisa que nao vamos mandar mais nada (close_notify).
    pub fn fechar<T: Transporte>(&mut self, t: &mut T) {
        let r = self.escrita.selar(ALERTA, &[1, 0]);
        let _ = t.enviar(&r);
    }
}
