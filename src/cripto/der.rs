//! Leitor de DER (o ASN.1 dos certificados): so o que o X.509 usa. Nao
//! copia nada: cada elemento e uma fatia do certificado original.

pub const BOOLEANO: u8 = 0x01;
pub const INTEIRO: u8 = 0x02;
pub const BITS: u8 = 0x03;
pub const OCTETOS: u8 = 0x04;
pub const NULO: u8 = 0x05;
pub const OID: u8 = 0x06;
pub const UTF8: u8 = 0x0C;
pub const SEQUENCIA: u8 = 0x30;
pub const CONJUNTO: u8 = 0x31;
pub const IMPRIMIVEL: u8 = 0x13;
pub const IA5: u8 = 0x16;
pub const UTC: u8 = 0x17;
pub const GENERALIZADO: u8 = 0x18;

#[derive(Clone, Copy)]
pub struct Elemento<'a> {
    pub tag: u8,
    /// So o conteudo.
    pub conteudo: &'a [u8],
    /// Tag + tamanho + conteudo (o que se assina, no caso do TBS).
    pub bruto: &'a [u8],
}

/// Percorre os elementos de uma sequencia.
#[derive(Clone, Copy)]
pub struct Leitor<'a> {
    pub resto: &'a [u8],
}

impl<'a> Leitor<'a> {
    pub fn novo(dados: &'a [u8]) -> Self {
        Leitor { resto: dados }
    }

    pub fn vazio(&self) -> bool {
        self.resto.is_empty()
    }

    pub fn espiar(&self) -> Option<u8> {
        self.resto.first().copied()
    }

    pub fn proximo(&mut self) -> Option<Elemento<'a>> {
        let d = self.resto;
        if d.len() < 2 {
            return None;
        }
        let tag = d[0];
        if tag & 0x1F == 0x1F {
            return None; // tags longas: o X.509 nao usa
        }
        let (tamanho, cab) = match d[1] {
            n if n < 0x80 => (n as usize, 2),
            0x81 => (*d.get(2)? as usize, 3),
            0x82 => (u16::from_be_bytes([*d.get(2)?, *d.get(3)?]) as usize, 4),
            0x83 => (((*d.get(2)? as usize) << 16) | ((*d.get(3)? as usize) << 8) | *d.get(4)? as usize, 5),
            _ => return None,
        };
        let fim = (cab as usize).checked_add(tamanho)?;
        if fim > d.len() {
            return None;
        }
        self.resto = &d[fim..];
        Some(Elemento { tag, conteudo: &d[cab..fim], bruto: &d[..fim] })
    }

    /// O proximo, se tiver a tag pedida.
    pub fn esperar(&mut self, tag: u8) -> Option<Elemento<'a>> {
        let e = self.proximo()?;
        (e.tag == tag).then_some(e)
    }

    /// Pula o proximo se ele tiver essa tag (campos OPTIONAL).
    pub fn opcional(&mut self, tag: u8) -> Option<Elemento<'a>> {
        if self.espiar() == Some(tag) { self.proximo() } else { None }
    }
}

impl<'a> Elemento<'a> {
    pub fn leitor(&self) -> Leitor<'a> {
        Leitor::novo(self.conteudo)
    }

    /// INTEGER positivo sem o zero de sinal da frente.
    pub fn inteiro(&self) -> &'a [u8] {
        let mut c = self.conteudo;
        while c.len() > 1 && c[0] == 0 {
            c = &c[1..];
        }
        c
    }

    /// BIT STRING sem o byte de "bits sobrando" (que tem que ser 0).
    pub fn bits(&self) -> Option<&'a [u8]> {
        match self.conteudo.split_first() {
            Some((0, resto)) => Some(resto),
            _ => None,
        }
    }
}
