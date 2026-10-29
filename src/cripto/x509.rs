//! Certificados X.509 (RFC 5280), so o que um navegador precisa: ler,
//! conferir a assinatura de quem emitiu, a validade, o nome do site e subir
//! a cadeia ate uma das autoridades de raizes.der (a lista da Mozilla, do
//! pacote ca-certificates; ferramentas/gerar_raizes.py).

use alloc::vec::Vec;

use super::der::{self, Elemento, Leitor};
use super::ec::{Contexto, Curva};
use super::rsa::{self, ChavePublica};
use super::Hash;

/// As autoridades confiaveis: certificados DER um atras do outro.
static RAIZES: &[u8] = include_bytes!("raizes.der");

const MAX_CADEIA: usize = 8;

// OIDs (o conteudo, sem tag e tamanho)
const OID_RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
const OID_RSA_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b];
const OID_RSA_SHA384: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0c];
const OID_RSA_SHA512: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0d];
const OID_EC: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const OID_P256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
const OID_P384: &[u8] = &[0x2b, 0x81, 0x04, 0x00, 0x22];
const OID_ECDSA_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
const OID_ECDSA_SHA384: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x03];
const OID_ECDSA_SHA512: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x04];
const OID_NOMES_ALTERNATIVOS: &[u8] = &[0x55, 0x1d, 0x11];
const OID_RESTRICOES_BASICAS: &[u8] = &[0x55, 0x1d, 0x13];
const OID_NOME_COMUM: &[u8] = &[0x55, 0x04, 0x03];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Erro {
    Formato,
    AlgoritmoDesconhecido,
    AssinaturaInvalida,
    Expirado,
    NomeNaoConfere,
    SemAutoridade,
    NaoEhAutoridade,
}

impl Erro {
    pub fn descricao(self) -> &'static str {
        match self {
            Erro::Formato => "certificado mal formado",
            Erro::AlgoritmoDesconhecido => "algoritmo de certificado não suportado",
            Erro::AssinaturaInvalida => "assinatura de certificado inválida",
            Erro::Expirado => "certificado vencido (ou o relógio está errado)",
            Erro::NomeNaoConfere => "o certificado é de outro site",
            Erro::SemAutoridade => "certificado de autoridade desconhecida",
            Erro::NaoEhAutoridade => "certificado intermediário não é de autoridade",
        }
    }
}

#[derive(Clone, Copy)]
pub enum Chave<'a> {
    Rsa { n: &'a [u8], e: &'a [u8] },
    Ec { curva: Curva, ponto: &'a [u8] },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Algoritmo {
    RsaPkcs1(Hash),
    Ecdsa(Hash),
}

pub struct Certificado<'a> {
    pub tbs: &'a [u8],
    pub algoritmo: Algoritmo,
    pub assinatura: &'a [u8],
    pub emissor: &'a [u8],
    pub sujeito: &'a [u8],
    /// Validade em segundos desde 1970.
    pub inicio: i64,
    pub fim: i64,
    pub chave: Chave<'a>,
    pub autoridade: bool,
    /// dNSName dos nomes alternativos (SAN).
    pub nomes: Vec<&'a [u8]>,
    pub nome_comum: Option<&'a [u8]>,
}

fn algoritmo(e: &Elemento) -> Result<Algoritmo, Erro> {
    let oid = e.leitor().esperar(der::OID).ok_or(Erro::Formato)?;
    Ok(match oid.conteudo {
        OID_RSA_SHA256 => Algoritmo::RsaPkcs1(Hash::Sha256),
        OID_RSA_SHA384 => Algoritmo::RsaPkcs1(Hash::Sha384),
        OID_RSA_SHA512 => Algoritmo::RsaPkcs1(Hash::Sha512),
        OID_ECDSA_SHA256 => Algoritmo::Ecdsa(Hash::Sha256),
        OID_ECDSA_SHA384 => Algoritmo::Ecdsa(Hash::Sha384),
        OID_ECDSA_SHA512 => Algoritmo::Ecdsa(Hash::Sha512),
        _ => return Err(Erro::AlgoritmoDesconhecido),
    })
}

fn dois(b: &[u8]) -> Option<i64> {
    let s = core::str::from_utf8(b).ok()?;
    s.parse::<i64>().ok()
}

/// Dias desde 1970-01-01 (algoritmo "days_from_civil").
fn dias(ano: i64, mes: i64, dia: i64) -> i64 {
    let a = if mes <= 2 { ano - 1 } else { ano };
    let era = a.div_euclid(400);
    let ano_era = a - era * 400;
    let m = if mes > 2 { mes - 3 } else { mes + 9 };
    let dia_ano = (153 * m + 2) / 5 + dia - 1;
    let dia_era = ano_era * 365 + ano_era / 4 - ano_era / 100 + dia_ano;
    era * 146097 + dia_era - 719468
}

fn tempo(e: &Elemento) -> Result<i64, Erro> {
    let c = e.conteudo;
    let (ano, resto) = match e.tag {
        der::UTC if c.len() >= 12 => {
            let a = dois(&c[..2]).ok_or(Erro::Formato)?;
            (if a < 50 { 2000 + a } else { 1900 + a }, &c[2..])
        }
        der::GENERALIZADO if c.len() >= 14 => (dois(&c[..4]).ok_or(Erro::Formato)?, &c[4..]),
        _ => return Err(Erro::Formato),
    };
    let campo = |i: usize| dois(&resto[2 * i..2 * i + 2]).ok_or(Erro::Formato);
    let (mes, dia, h, m, s) = (campo(0)?, campo(1)?, campo(2)?, campo(3)?, campo(4)?);
    Ok(dias(ano, mes, dia) * 86400 + h * 3600 + m * 60 + s)
}

fn chave_publica<'a>(spki: &Elemento<'a>) -> Result<Chave<'a>, Erro> {
    let mut l = spki.leitor();
    let alg = l.esperar(der::SEQUENCIA).ok_or(Erro::Formato)?;
    let bits = l.esperar(der::BITS).ok_or(Erro::Formato)?.bits().ok_or(Erro::Formato)?;
    let mut la = alg.leitor();
    let oid = la.esperar(der::OID).ok_or(Erro::Formato)?;
    match oid.conteudo {
        OID_RSA => {
            let mut lr = Leitor::novo(bits).esperar(der::SEQUENCIA).ok_or(Erro::Formato)?.leitor();
            let n = lr.esperar(der::INTEIRO).ok_or(Erro::Formato)?.inteiro();
            let e = lr.esperar(der::INTEIRO).ok_or(Erro::Formato)?.inteiro();
            Ok(Chave::Rsa { n, e })
        }
        OID_EC => {
            let curva = match la.esperar(der::OID).ok_or(Erro::Formato)?.conteudo {
                OID_P256 => Curva::P256,
                OID_P384 => Curva::P384,
                _ => return Err(Erro::AlgoritmoDesconhecido),
            };
            Ok(Chave::Ec { curva, ponto: bits })
        }
        _ => Err(Erro::AlgoritmoDesconhecido),
    }
}

fn nome_comum(nome: &[u8]) -> Option<&[u8]> {
    // Name = SEQUENCE OF SET OF SEQUENCE { OID, valor }
    let mut l = Leitor::novo(nome);
    while let Some(conjunto) = l.proximo() {
        let mut lc = conjunto.leitor();
        while let Some(par) = lc.proximo() {
            let mut lp = par.leitor();
            if lp.esperar(der::OID).is_some_and(|o| o.conteudo == OID_NOME_COMUM) {
                return lp.proximo().map(|v| v.conteudo);
            }
        }
    }
    None
}

impl<'a> Certificado<'a> {
    pub fn ler(dados: &'a [u8]) -> Result<Certificado<'a>, Erro> {
        let f = Erro::Formato;
        let cert = Leitor::novo(dados).esperar(der::SEQUENCIA).ok_or(f)?;
        let mut l = cert.leitor();
        let tbs = l.esperar(der::SEQUENCIA).ok_or(f)?;
        let alg = algoritmo(&l.esperar(der::SEQUENCIA).ok_or(f)?);
        let assinatura = l.esperar(der::BITS).ok_or(f)?.bits().ok_or(f)?;

        let mut t = tbs.leitor();
        t.opcional(0xA0); // versao
        t.esperar(der::INTEIRO).ok_or(f)?; // numero de serie
        t.esperar(der::SEQUENCIA).ok_or(f)?; // algoritmo (repetido)
        let emissor = t.esperar(der::SEQUENCIA).ok_or(f)?;
        let mut validade = t.esperar(der::SEQUENCIA).ok_or(f)?.leitor();
        let inicio = tempo(&validade.proximo().ok_or(f)?)?;
        let fim = tempo(&validade.proximo().ok_or(f)?)?;
        let sujeito = t.esperar(der::SEQUENCIA).ok_or(f)?;
        let spki = t.esperar(der::SEQUENCIA).ok_or(f)?;
        let chave = chave_publica(&spki);
        t.opcional(0x81);
        t.opcional(0x82);

        let mut autoridade = false;
        let mut nomes = Vec::new();
        if let Some(ext) = t.opcional(0xA3) {
            let mut le = ext.leitor().esperar(der::SEQUENCIA).ok_or(f)?.leitor();
            while let Some(e) = le.proximo() {
                let mut lx = e.leitor();
                let oid = lx.esperar(der::OID).ok_or(f)?;
                lx.opcional(der::BOOLEANO);
                let valor = lx.esperar(der::OCTETOS).ok_or(f)?.conteudo;
                match oid.conteudo {
                    OID_RESTRICOES_BASICAS => {
                        let seq = Leitor::novo(valor).esperar(der::SEQUENCIA).ok_or(f)?;
                        autoridade = seq.leitor().opcional(der::BOOLEANO).is_some_and(|b| b.conteudo == [0xFF]);
                    }
                    OID_NOMES_ALTERNATIVOS => {
                        let mut ln = Leitor::novo(valor).esperar(der::SEQUENCIA).ok_or(f)?.leitor();
                        while let Some(n) = ln.proximo() {
                            if n.tag == 0x82 {
                                nomes.push(n.conteudo);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        // a chave e o algoritmo so importam se forem usados (uma raiz com
        // algoritmo estranho nao impede ler as outras)
        Ok(Certificado {
            tbs: tbs.bruto,
            algoritmo: alg.unwrap_or(Algoritmo::RsaPkcs1(Hash::Sha512)),
            assinatura,
            emissor: emissor.bruto,
            sujeito: sujeito.bruto,
            inicio,
            fim,
            chave: chave.unwrap_or(Chave::Rsa { n: &[], e: &[] }),
            autoridade,
            nomes,
            nome_comum: nome_comum(sujeito.conteudo),
        })
    }

    /// A assinatura deste certificado confere com a chave de quem emitiu?
    pub fn assinado_por(&self, chave: &Chave) -> bool {
        match self.algoritmo {
            Algoritmo::RsaPkcs1(h) => verificar(chave, Esquema::RsaPkcs1(h), self.tbs, self.assinatura),
            Algoritmo::Ecdsa(h) => verificar(chave, Esquema::Ecdsa(h), self.tbs, self.assinatura),
        }
    }

    /// O certificado vale para `host`? (nomes alternativos; o nome comum so
    /// se nao houver nenhum)
    pub fn vale_para(&self, host: &str) -> bool {
        if self.nomes.is_empty() {
            return self.nome_comum.is_some_and(|n| confere_nome(n, host.as_bytes()));
        }
        self.nomes.iter().any(|n| confere_nome(n, host.as_bytes()))
    }
}

/// "*.exemplo.com" vale para um rotulo so ("a.exemplo.com", nao
/// "a.b.exemplo.com" nem "exemplo.com").
fn confere_nome(padrao: &[u8], host: &[u8]) -> bool {
    let host = host.strip_suffix(b".").unwrap_or(host);
    if let Some(resto) = padrao.strip_prefix(b"*.") {
        let Some(ponto) = host.iter().position(|&c| c == b'.') else { return false };
        return ponto > 0 && host[ponto + 1..].eq_ignore_ascii_case(resto);
    }
    padrao.eq_ignore_ascii_case(host)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Esquema {
    RsaPkcs1(Hash),
    RsaPss(Hash),
    Ecdsa(Hash),
}

/// Verifica uma assinatura de `mensagem` com a chave. ECDSA vem em DER
/// (SEQUENCE { r, s }).
pub fn verificar(chave: &Chave, esquema: Esquema, mensagem: &[u8], assinatura: &[u8]) -> bool {
    match (*chave, esquema) {
        (Chave::Rsa { n, e }, Esquema::RsaPkcs1(h)) => rsa::verificar_pkcs1(&ChavePublica { n, e }, h, mensagem, assinatura),
        (Chave::Rsa { n, e }, Esquema::RsaPss(h)) => rsa::verificar_pss(&ChavePublica { n, e }, h, mensagem, assinatura),
        (Chave::Ec { curva, ponto }, Esquema::Ecdsa(h)) => {
            let Some(seq) = Leitor::novo(assinatura).esperar(der::SEQUENCIA) else { return false };
            let mut l = seq.leitor();
            let (Some(r), Some(s)) = (l.esperar(der::INTEIRO), l.esperar(der::INTEIRO)) else { return false };
            Contexto::novo(curva).verificar_mensagem(ponto, h, mensagem, r.inteiro(), s.inteiro())
        }
        _ => false,
    }
}

/// As raizes confiaveis, uma a uma.
pub fn raizes() -> impl Iterator<Item = Certificado<'static>> {
    let mut l = Leitor::novo(RAIZES);
    core::iter::from_fn(move || {
        loop {
            let e = l.proximo()?;
            if let Ok(c) = Certificado::ler(e.bruto) {
                return Some(c);
            }
        }
    })
}

/// Confere a cadeia que o servidor mandou (o do site primeiro) para
/// `host`. `agora` em segundos desde 1970 (0 = relogio desconhecido, nao
/// confere validade). Devolve o certificado do site.
pub fn verificar_cadeia<'a>(cadeia: &[&'a [u8]], host: &str, agora: i64) -> Result<Certificado<'a>, Erro> {
    let certs: Vec<Certificado> = cadeia.iter().map(|c| Certificado::ler(c)).collect::<Result<_, _>>()?;
    let folha = certs.first().ok_or(Erro::Formato)?;
    if !folha.vale_para(host) {
        return Err(Erro::NomeNaoConfere);
    }
    let em_dia = |c: &Certificado| agora == 0 || (c.inicio <= agora && agora <= c.fim);
    let mut atual = 0;
    for _ in 0..MAX_CADEIA {
        let c = &certs[atual];
        if !em_dia(c) {
            return Err(Erro::Expirado);
        }
        // uma raiz conhecida assinou este?
        if raizes().any(|r| r.sujeito == c.emissor && c.assinado_por(&r.chave)) {
            return Ok(certs.into_iter().next().unwrap_or_else(|| unreachable!()));
        }
        // senao, um intermediario da lista
        let proximo = (0..certs.len()).find(|&k| k != atual && certs[k].sujeito == c.emissor && c.assinado_por(&certs[k].chave));
        match proximo {
            Some(k) if !certs[k].autoridade => return Err(Erro::NaoEhAutoridade),
            Some(k) => atual = k,
            None => {
                return Err(if certs.iter().any(|x| x.sujeito == c.emissor) { Erro::AssinaturaInvalida } else { Erro::SemAutoridade });
            }
        }
    }
    Err(Erro::SemAutoridade)
}
