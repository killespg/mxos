//! Leitura de HTML: um tokenizador de uma passada so, que ja monta o
//! Documento. Nao existe arvore (DOM): o resultado e uma lista plana de
//! blocos (paragrafo, titulo, item de lista...), cada um com seus trechos
//! de texto ja estilizados (negrito, link...).
//!
//! Tolerante com HTML do mundo real: tags sem fechar, maiusculas,
//! atributos com ou sem aspas, comentarios, doctype, CDATA. Conteudo de
//! script, style e coisas que nao sabemos mostrar (svg, iframe...) e
//! pulado. O texto sai em UTF-8 com os acentos: a pagina e lida como UTF-8
//! (ou Windows-1252, se nao for UTF-8 valido) e fica o que a fonte tem
//! (ASCII, Latin-1 e alguns simbolos); o resto vira um parecido ou '?'.

use alloc::string::String;
use alloc::vec::Vec;

/// Bits de Trecho::estilo.
pub const NEGRITO: u8 = 1 << 0;
pub const ITALICO: u8 = 1 << 1;
pub const SUBLINHADO: u8 = 1 << 2;
pub const CODIGO: u8 = 1 << 3;

/// Limite de aninhamento de listas e citacoes (paginas malucas nao
/// empurram o texto para fora da tela).
const RECUO_MAXIMO: usize = 6;
/// Celulas por linha de tabela; o que passar disso fica na ultima.
pub const MAX_CELULAS: usize = 32;

/// Um pedaco de texto com o mesmo estilo e o mesmo link.
/// '\n' dentro do texto e uma quebra de linha forcada (<br>).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trecho {
    pub texto: String,
    pub estilo: u8,
    /// Indice em Documento::links.
    pub link: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TipoBloco {
    Paragrafo,
    /// h1 a h6.
    Titulo(u8),
    /// Texto pre-formatado: espacos e quebras mantidos, sem quebra automatica.
    Pre,
    /// <hr>.
    Regua,
    /// <li>.
    Item,
    /// Linha de tabela (<tr>): as celulas estao em Bloco::celulas.
    Linha,
}

/// Uma celula de uma linha de tabela.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Celula {
    /// Indice em Bloco::trechos do primeiro trecho da celula.
    pub inicio: usize,
    /// Celula de cabecalho (<th>).
    pub cabecalho: bool,
}

/// O que aparece antes da primeira linha de um item de lista.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Marcador {
    Nenhum,
    /// Lista sem numero; o nivel (1 = mais externa) escolhe o simbolo.
    Bolinha(u8),
    Numero(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bloco {
    pub tipo: TipoBloco,
    /// Niveis de recuo (listas e citacoes por fora deste bloco).
    pub recuo: u8,
    /// Quantas citacoes (<blockquote>) envolvem o bloco.
    pub citacao: u8,
    pub marcador: Marcador,
    /// Espaco em branco antes do bloco (paragrafos, titulos...).
    pub margem: bool,
    pub trechos: Vec<Trecho>,
    /// So em TipoBloco::Linha: onde cada celula comeca (vazio = uma so).
    pub celulas: Vec<Celula>,
    /// Numero da tabela da linha: linhas seguidas da mesma tabela sao
    /// alinhadas em colunas. 0 = celula fora de <table>.
    pub tabela: u32,
}

pub struct Documento {
    pub titulo: String,
    pub blocos: Vec<Bloco>,
    /// href de cada <a>, como estava na pagina (sem resolver).
    pub links: Vec<String>,
}

/// Le o HTML inteiro e devolve o documento pronto para diagramar.
pub fn analisar(html: &str) -> Documento {
    let mut c = Construtor::novo();
    let mut l = Leitor { b: html.as_bytes(), pos: 0 };
    while let Some(ficha) = l.proxima() {
        match ficha {
            Ficha::Texto(cru) => c.texto_cru(cru),
            Ficha::TextoLiteral(cru) => c.texto(&para_texto(cru)),
            Ficha::Abre(tag) => {
                c.abrir(&tag);
                if let Some(fim) = conteudo_cru(&tag.nome) {
                    let conteudo = l.ate_fechar(fim);
                    if tag.nome == "title" {
                        c.definir_titulo(conteudo);
                    }
                }
            }
            Ficha::Fecha(nome) => c.fechar(&nome),
        }
    }
    c.terminar()
}

/// Tags cujo conteudo e lido cru ate o fechamento (e nao e mostrado).
fn conteudo_cru(nome: &str) -> Option<&'static str> {
    Some(match nome {
        "script" => "script",
        "style" => "style",
        "title" => "title",
        "svg" => "svg",
        "math" => "math",
        "template" => "template",
        "iframe" => "iframe",
        "object" => "object",
        "canvas" => "canvas",
        "video" => "video",
        "audio" => "audio",
        "noembed" => "noembed",
        _ => return None,
    })
}

// ---------------------------------------------------------------------
// Tokenizador

struct Tag {
    nome: String,
    atributos: Vec<(String, String)>,
}

impl Tag {
    fn atributo(&self, nome: &str) -> Option<&str> {
        self.atributos.iter().find(|(n, _)| n == nome).map(|(_, v)| v.as_str())
    }
}

enum Ficha<'a> {
    /// Texto normal, ainda com entidades (&amp;...).
    Texto(&'a [u8]),
    /// Texto de CDATA: sem entidades.
    TextoLiteral(&'a [u8]),
    Abre(Tag),
    Fecha(String),
}

struct Leitor<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Leitor<'a> {
    fn resto(&self) -> &'a [u8] {
        &self.b[self.pos..]
    }

    fn comeca_com(&self, prefixo: &[u8]) -> bool {
        let resto = self.resto();
        resto.len() >= prefixo.len() && resto[..prefixo.len()].eq_ignore_ascii_case(prefixo)
    }

    /// Avanca ate depois de `fim` (ou ate o final do texto).
    fn pular_ate(&mut self, fim: &[u8]) {
        match procurar(self.resto(), fim, false) {
            Some(i) => self.pos += i + fim.len(),
            None => self.pos = self.b.len(),
        }
    }

    fn proxima(&mut self) -> Option<Ficha<'a>> {
        loop {
            if self.pos >= self.b.len() {
                return None;
            }
            if self.b[self.pos] != b'<' {
                let inicio = self.pos;
                while self.pos < self.b.len() && self.b[self.pos] != b'<' {
                    self.pos += 1;
                }
                return Some(Ficha::Texto(&self.b[inicio..self.pos]));
            }
            if self.comeca_com(b"<!--") {
                self.pos += 4;
                self.pular_ate(b"-->");
                continue;
            }
            if self.comeca_com(b"<![CDATA[") {
                self.pos += 9;
                let inicio = self.pos;
                let fim = procurar(self.resto(), b"]]>", false).map_or(self.b.len(), |i| inicio + i);
                self.pos = (fim + 3).min(self.b.len());
                return Some(Ficha::TextoLiteral(&self.b[inicio..fim]));
            }
            if self.comeca_com(b"<!") || self.comeca_com(b"<?") {
                // doctype, processamento XML...
                self.pular_ate(b">");
                continue;
            }
            let seguinte = self.b.get(self.pos + 1).copied().unwrap_or(0);
            if seguinte == b'/' {
                self.pos += 2;
                let nome = self.ler_nome();
                self.pular_ate(b">");
                if nome.is_empty() {
                    continue; // "</>" ou "</ 3": lixo
                }
                return Some(Ficha::Fecha(nome));
            }
            if seguinte.is_ascii_alphabetic() {
                self.pos += 1;
                return Some(Ficha::Abre(self.ler_tag()));
            }
            // '<' sozinho ("a < b"): e texto
            self.pos += 1;
            return Some(Ficha::Texto(b"<"));
        }
    }

    /// Nome de tag ou atributo, ja em minusculas.
    fn ler_nome(&mut self) -> String {
        let mut nome = String::new();
        while let Some(&c) = self.b.get(self.pos) {
            if c.is_ascii_whitespace() || c == b'>' || c == b'/' || c == b'=' {
                break;
            }
            if nome.len() < 32 {
                nome.push(c.to_ascii_lowercase() as char);
            }
            self.pos += 1;
        }
        // so ASCII entra no nome; o resto vira algo que nenhuma tag tem
        if !nome.bytes().all(|c| c.is_ascii_graphic()) {
            nome.clear();
            nome.push('?');
        }
        nome
    }

    fn pular_espacos(&mut self) {
        while self.b.get(self.pos).is_some_and(|c| c.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }

    /// Depois do '<': nome, atributos e o '>' (ou '/>').
    fn ler_tag(&mut self) -> Tag {
        let nome = self.ler_nome();
        let mut atributos = Vec::new();
        loop {
            self.pular_espacos();
            match self.b.get(self.pos) {
                None => break,
                Some(b'>') => {
                    self.pos += 1;
                    break;
                }
                Some(b'/') | Some(b'=') => {
                    self.pos += 1; // "/>" ou lixo
                    continue;
                }
                Some(_) => {}
            }
            let nome_atributo = self.ler_nome();
            self.pular_espacos();
            let valor = if self.b.get(self.pos) == Some(&b'=') {
                self.pos += 1;
                self.pular_espacos();
                self.ler_valor()
            } else {
                String::new()
            };
            if atributos.len() < 32 {
                atributos.push((nome_atributo, valor));
            }
        }
        Tag { nome, atributos }
    }

    /// Valor de atributo: "entre aspas", 'apostrofos' ou solto.
    fn ler_valor(&mut self) -> String {
        let inicio;
        let fim;
        match self.b.get(self.pos) {
            Some(&aspa @ (b'"' | b'\'')) => {
                self.pos += 1;
                inicio = self.pos;
                while self.b.get(self.pos).is_some_and(|&c| c != aspa) {
                    self.pos += 1;
                }
                fim = self.pos;
                self.pos = (self.pos + 1).min(self.b.len());
            }
            _ => {
                inicio = self.pos;
                while self.b.get(self.pos).is_some_and(|&c| !c.is_ascii_whitespace() && c != b'>') {
                    self.pos += 1;
                }
                fim = self.pos;
            }
        }
        decodificar(&self.b[inicio..fim]).trim().into()
    }

    /// Conteudo cru de script/style/title...: tudo ate "</nome".
    fn ate_fechar(&mut self, nome: &str) -> &'a [u8] {
        let mut fim = Vec::with_capacity(nome.len() + 2);
        fim.extend_from_slice(b"</");
        fim.extend_from_slice(nome.as_bytes());
        let inicio = self.pos;
        match procurar(self.resto(), &fim, true) {
            Some(i) => {
                self.pos += i + fim.len();
                self.pular_ate(b">");
                &self.b[inicio..inicio + i]
            }
            None => {
                self.pos = self.b.len();
                &self.b[inicio..]
            }
        }
    }
}

/// Posicao de `agulha` em `palheiro` (opcionalmente sem ligar para caixa).
fn procurar(palheiro: &[u8], agulha: &[u8], sem_caixa: bool) -> Option<usize> {
    if agulha.is_empty() || palheiro.len() < agulha.len() {
        return None;
    }
    (0..=palheiro.len() - agulha.len()).find(|&i| {
        let janela = &palheiro[i..i + agulha.len()];
        if sem_caixa { janela.eq_ignore_ascii_case(agulha) } else { janela == agulha }
    })
}

// ---------------------------------------------------------------------
// Texto: UTF-8 (ou Windows-1252) -> caracteres que a fonte tem, e entidades

/// Bytes da pagina -> texto. UTF-8 valido e decodificado; byte que nao
/// forma UTF-8 valido e lido como Windows-1252 (o Latin-1 das paginas
/// antigas). Fica tudo o que a fonte desenha (ASCII, Latin-1 e alguns
/// simbolos); o resto vira um parecido ou '?'.
pub fn para_texto(bytes: &[u8]) -> String {
    let mut saida = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let (codigo, tamanho) = decodificar_utf8(&bytes[i..]);
        empurrar_caractere(&mut saida, codigo);
        i += tamanho;
    }
    saida
}

/// Um caractere do comeco de `b`: (codigo, bytes usados). Sempre usa
/// pelo menos 1 byte; byte solto vale o que vale no Windows-1252.
fn decodificar_utf8(b: &[u8]) -> (u32, usize) {
    let primeiro = b[0];
    let solto = (de_1252(primeiro), 1);
    let (tamanho, minimo, bits) = match primeiro {
        0x00..=0x7F => return (primeiro as u32, 1),
        0xC2..=0xDF => (2, 0x80, primeiro as u32 & 0x1F),
        0xE0..=0xEF => (3, 0x800, primeiro as u32 & 0x0F),
        0xF0..=0xF4 => (4, 0x1_0000, primeiro as u32 & 0x07),
        _ => return solto,
    };
    if b.len() < tamanho {
        return solto;
    }
    let mut codigo = bits;
    for &c in &b[1..tamanho] {
        if c & 0xC0 != 0x80 {
            return solto;
        }
        codigo = codigo << 6 | (c & 0x3F) as u32;
    }
    if codigo < minimo || codigo > 0x10_FFFF {
        return solto;
    }
    (codigo, tamanho)
}

/// Windows-1252 de 0x80 a 0x9F (0 = sem caractere); o resto e Latin-1.
const CP1252: [u16; 32] = [
    0x20AC, 0, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0, 0x017D, 0,
    0, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0, 0x017E, 0x0178,
];

fn de_1252(byte: u8) -> u32 {
    match byte {
        0x80..=0x9F => CP1252[(byte - 0x80) as usize] as u32,
        b => b as u32,
    }
}

/// Letras de U+0100 a U+017F sem acento (a fonte nao tem essas).
const LATIN_A: &[u8; 128] = b"AaAaAaCcCcCcCcDdDdEeEeEeEeEeGgGgGgGgHhHhIiIiIiIiIiIiJjKkkLlLlLlLlLlNnNnNnnNnOoOoOoOoRrRrRrSsSsSsSsTtTtTtUuUuUuUuUuUuWwYyYZzZzZzs";

/// A fonte tem o caractere: ASCII imprimivel, Latin-1 e estes simbolos.
fn na_fonte(codigo: u32) -> bool {
    matches!(
        codigo,
        0x20..=0x7E
            | 0xA0..=0xFF
            | 0x2013
            | 0x2014
            | 0x2018
            | 0x2019
            | 0x201C
            | 0x201D
            | 0x2022
            | 0x2026
            | 0x20AC
            | 0x2190..=0x2193
            | 0x2713
    )
}

/// Acrescenta um caractere Unicode: ele mesmo se a fonte tem, senao um
/// parecido (ou '?').
fn empurrar_caractere(saida: &mut String, codigo: u32) {
    let parecido: &str = match codigo {
        0x09 | 0x0A | 0x0D => {
            saida.push(codigo as u8 as char);
            return;
        }
        // invisiveis e controles: somem (o hifen opcional tambem)
        0xAD | 0x200B..=0x200D | 0x2060 | 0xFEFF | 0x00..=0x1F | 0x7F..=0x9F => return,
        _ if na_fonte(codigo) => {
            saida.push(char::from_u32(codigo).unwrap_or('?'));
            return;
        }
        0x152 => "OE",
        0x153 => "oe",
        0x100..=0x17F => {
            saida.push(LATIN_A[(codigo - 0x100) as usize] as char);
            return;
        }
        0x2000..=0x200A | 0x202F | 0x205F | 0x3000 => " ",
        0x2010..=0x2012 | 0x2212 | 0x2043 => "-",
        0x2015 => "\u{2014}",
        0x201A | 0x201B | 0x2032 | 0x02BC => "\u{2019}",
        0x201E | 0x201F | 0x2033 => "\u{201D}",
        0x2039 => "\u{AB}",
        0x203A => "\u{BB}",
        0x2023 | 0x2219 | 0x25CF | 0x25E6 | 0x25AA | 0x25A0 => "\u{2022}",
        0x02C6 => "^",
        0x02DC => "~",
        0x2122 => "TM",
        0x2030 => "%.",
        0x2194 => "<->",
        0x21D2 => "=>",
        0x2264 => "<=",
        0x2265 => ">=",
        0x2260 => "!=",
        0x2248 => "~",
        0x221E => "inf",
        0x2714 | 0x2705 | 0x2611 => "\u{2713}",
        0x2716 | 0x2717 | 0x2718 | 0x274C => "x",
        0x0192 => "f",
        _ => "?",
    };
    saida.push_str(parecido);
}

/// Entidades com nome -> codigo Unicode. As acentuadas (aacute,
/// ccedil...) sao tratadas por regra em `entidade_nomeada`.
const ENTIDADES: &[(&str, u32)] = &[
    ("amp", 0x26),
    ("lt", 0x3C),
    ("gt", 0x3E),
    ("quot", 0x22),
    ("apos", 0x27),
    ("nbsp", 0xA0),
    ("ensp", 0x2002),
    ("emsp", 0x2003),
    ("thinsp", 0x2009),
    ("shy", 0xAD),
    ("zwj", 0x200D),
    ("zwnj", 0x200C),
    ("copy", 0xA9),
    ("reg", 0xAE),
    ("trade", 0x2122),
    ("hellip", 0x2026),
    ("mdash", 0x2014),
    ("ndash", 0x2013),
    ("minus", 0x2212),
    ("lsquo", 0x2018),
    ("rsquo", 0x2019),
    ("sbquo", 0x201A),
    ("ldquo", 0x201C),
    ("rdquo", 0x201D),
    ("bdquo", 0x201E),
    ("laquo", 0xAB),
    ("raquo", 0xBB),
    ("lsaquo", 0x2039),
    ("rsaquo", 0x203A),
    ("bull", 0x2022),
    ("middot", 0xB7),
    ("deg", 0xB0),
    ("ordm", 0xBA),
    ("ordf", 0xAA),
    ("euro", 0x20AC),
    ("pound", 0xA3),
    ("cent", 0xA2),
    ("yen", 0xA5),
    ("curren", 0xA4),
    ("sect", 0xA7),
    ("para", 0xB6),
    ("micro", 0xB5),
    ("not", 0xAC),
    ("times", 0xD7),
    ("divide", 0xF7),
    ("plusmn", 0xB1),
    ("frac12", 0xBD),
    ("frac14", 0xBC),
    ("frac34", 0xBE),
    ("sup1", 0xB9),
    ("sup2", 0xB2),
    ("sup3", 0xB3),
    ("iexcl", 0xA1),
    ("iquest", 0xBF),
    ("larr", 0x2190),
    ("uarr", 0x2191),
    ("rarr", 0x2192),
    ("darr", 0x2193),
    ("harr", 0x2194),
    ("rArr", 0x21D2),
    ("le", 0x2264),
    ("ge", 0x2265),
    ("ne", 0x2260),
    ("check", 0x2713),
    ("aelig", 0xE6),
    ("AElig", 0xC6),
    ("oelig", 0x153),
    ("OElig", 0x152),
    ("szlig", 0xDF),
    ("eth", 0xF0),
    ("ETH", 0xD0),
    ("thorn", 0xFE),
    ("THORN", 0xDE),
];

/// Letra sem acento seguida da acentuada, por acento: "atilde" -> 'ã'.
const ACENTOS: &[(&str, &str)] = &[
    ("acute", "AÁEÉIÍOÓUÚYÝaáeéiíoóuúyý"),
    ("grave", "AÀEÈIÌOÒUÙaàeèiìoòuù"),
    ("circ", "AÂEÊIÎOÔUÛaâeêiîoôuû"),
    ("tilde", "AÃNÑOÕaãnñoõ"),
    ("uml", "AÄEËIÏOÖUÜaäeëiïoöuüyÿ"),
    ("cedil", "CÇcç"),
    ("ring", "AÅaå"),
    ("slash", "OØoø"),
    ("caron", ""),
];

/// Codigo Unicode da entidade `nome` ("amp", "eacute"...).
fn entidade_nomeada(nome: &str) -> Option<u32> {
    if let Some(&(_, codigo)) = ENTIDADES.iter().find(|(n, _)| *n == nome) {
        return Some(codigo);
    }
    let mut letras = nome.chars();
    let letra = letras.next().filter(char::is_ascii_alphabetic)?;
    let resto = letras.as_str();
    let &(_, pares) = ACENTOS.iter().find(|(acento, _)| *acento == resto)?;
    let mut pares = pares.chars();
    while let (Some(base), Some(acentuada)) = (pares.next(), pares.next()) {
        if base == letra {
            return Some(acentuada as u32);
        }
    }
    // combinacao que a fonte nao tem ("scaron"): fica a letra sem acento
    Some(letra as u32)
}

/// Texto com entidades -> texto. Entidade desconhecida fica como esta.
pub fn decodificar(bytes: &[u8]) -> String {
    let mut saida = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'&' {
            if let Some(usados) = entidade(&bytes[i..], &mut saida) {
                i += usados;
                continue;
            }
            saida.push('&');
            i += 1;
            continue;
        }
        // copia ate o proximo '&' decodificando
        let fim = bytes[i..].iter().position(|&c| c == b'&').map_or(bytes.len(), |p| i + p);
        saida.push_str(&para_texto(&bytes[i..fim]));
        i = fim;
    }
    saida
}

/// Tenta ler uma entidade no comeco de `b` (que comeca com '&').
/// Devolve quantos bytes usou, ou None se nao era entidade.
fn entidade(b: &[u8], saida: &mut String) -> Option<usize> {
    if b.get(1) == Some(&b'#') {
        let hexa = matches!(b.get(2), Some(b'x' | b'X'));
        let inicio = if hexa { 3 } else { 2 };
        let mut fim = inicio;
        while fim < b.len() && fim - inicio < 8 && (if hexa { b[fim].is_ascii_hexdigit() } else { b[fim].is_ascii_digit() }) {
            fim += 1;
        }
        if fim == inicio {
            return None;
        }
        let digitos = core::str::from_utf8(&b[inicio..fim]).ok()?;
        let codigo = u32::from_str_radix(digitos, if hexa { 16 } else { 10 }).ok()?;
        // &#150; e cia: paginas Windows-1252 usam 128..159 como pontuacao
        let codigo = if (0x80..=0x9F).contains(&codigo) { de_1252(codigo as u8) } else { codigo };
        empurrar_caractere(saida, codigo);
        return Some(if b.get(fim) == Some(&b';') { fim + 1 } else { fim });
    }
    let mut fim = 1;
    while fim < b.len() && fim <= 10 && b[fim].is_ascii_alphanumeric() {
        fim += 1;
    }
    let nome = core::str::from_utf8(&b[1..fim]).ok()?;
    let codigo = entidade_nomeada(nome)?;
    let com_ponto_e_virgula = b.get(fim) == Some(&b';');
    // sem ';' so aceitamos as entidades classicas ("&copy 2020", "a&amp b")
    if !com_ponto_e_virgula && !matches!(nome, "amp" | "lt" | "gt" | "quot" | "nbsp" | "copy" | "reg") {
        return None;
    }
    empurrar_caractere(saida, codigo);
    Some(if com_ponto_e_virgula { fim + 1 } else { fim })
}

// ---------------------------------------------------------------------
// Construtor: transforma as fichas em blocos e trechos

struct Lista {
    ordenada: bool,
    contador: u32,
}

struct Construtor {
    doc: Documento,
    atual: Bloco,
    /// O ultimo caractere do bloco atual e espaco (ou o bloco esta vazio):
    /// espacos seguidos viram um so.
    ultimo_espaco: bool,
    margem_pendente: bool,
    negrito: u16,
    italico: u16,
    sublinhado: u16,
    codigo: u16,
    link: Option<usize>,
    pre: u16,
    /// Pular o '\n' logo depois de <pre>.
    pular_nova_linha: bool,
    listas: Vec<Lista>,
    citacao: u8,
    /// A celula atual e <th> (negrito ate fechar).
    em_th: bool,
    /// O proximo texto comeca uma celula: nao pode grudar no trecho de antes.
    nova_celula: bool,
    /// Quantas <table> ja abriram (da o numero de cada uma).
    tabelas: u32,
    /// Tabelas abertas, a de dentro por ultimo.
    pilha_tabelas: Vec<u32>,
    /// Dentro de <dt> (negrito) e de <dd> (recuo).
    em_dt: bool,
    em_dd: bool,
    na_cabeca: bool,
    tem_titulo: bool,
}

impl Construtor {
    fn novo() -> Construtor {
        Construtor {
            doc: Documento { titulo: String::new(), blocos: Vec::new(), links: Vec::new() },
            atual: bloco_vazio(TipoBloco::Paragrafo, 0, 0),
            ultimo_espaco: true,
            margem_pendente: false,
            negrito: 0,
            italico: 0,
            sublinhado: 0,
            codigo: 0,
            link: None,
            pre: 0,
            pular_nova_linha: false,
            listas: Vec::new(),
            citacao: 0,
            em_th: false,
            nova_celula: false,
            tabelas: 0,
            pilha_tabelas: Vec::new(),
            em_dt: false,
            em_dd: false,
            na_cabeca: false,
            tem_titulo: false,
        }
    }

    fn estilo(&self) -> u8 {
        let mut e = 0;
        if self.negrito > 0 {
            e |= NEGRITO;
        }
        if self.italico > 0 {
            e |= ITALICO;
        }
        if self.sublinhado > 0 {
            e |= SUBLINHADO;
        }
        if self.codigo > 0 || self.pre > 0 {
            e |= CODIGO;
        }
        e
    }

    fn recuo(&self) -> u8 {
        (self.listas.len() + self.citacao as usize + self.em_dd as usize).min(RECUO_MAXIMO) as u8
    }

    /// Termina o bloco atual (se tiver conteudo) e abre um anonimo.
    fn fechar_bloco(&mut self) {
        let tipo = if self.pre > 0 { TipoBloco::Pre } else { TipoBloco::Paragrafo };
        let novo = bloco_vazio(tipo, self.recuo(), self.citacao);
        let mut bloco = core::mem::replace(&mut self.atual, novo);
        self.ultimo_espaco = true;
        self.nova_celula = false;
        aparar_fim(&mut bloco);
        let tem_texto = bloco.trechos.iter().any(|t| t.texto.bytes().any(|c| c != b' ' && c != b'\n'));
        if tem_texto || bloco.tipo == TipoBloco::Regua {
            bloco.margem = self.margem_pendente;
            self.margem_pendente = false;
            self.doc.blocos.push(bloco);
        } else {
            // "<li><p>texto": o marcador passa para o bloco de dentro
            self.atual.marcador = bloco.marcador;
        }
    }

    /// Comeca um bloco de um tipo especifico.
    fn comecar(&mut self, tipo: TipoBloco, margem: bool) {
        self.fechar_bloco();
        if margem {
            self.margem_pendente = true;
        }
        self.atual.tipo = tipo;
    }

    /// Acrescenta texto ja decodificado ao bloco atual.
    fn acrescentar(&mut self, texto: &str) {
        if texto.is_empty() {
            return;
        }
        let estilo = self.estilo();
        let link = self.link;
        let grudar = !core::mem::take(&mut self.nova_celula);
        match self.atual.trechos.last_mut() {
            Some(t) if grudar && t.estilo == estilo && t.link == link => t.texto.push_str(texto),
            _ => self.atual.trechos.push(Trecho { texto: texto.into(), estilo, link }),
        }
    }

    fn texto_cru(&mut self, cru: &[u8]) {
        if self.na_cabeca {
            return;
        }
        let texto = decodificar(cru);
        self.texto(&texto);
    }

    /// Texto da pagina: espacos juntados (ou mantidos no <pre>).
    fn texto(&mut self, texto: &str) {
        if self.na_cabeca {
            return;
        }
        if self.pre > 0 {
            self.texto_pre(texto);
            return;
        }
        let mut limpo = String::with_capacity(texto.len());
        for c in texto.chars() {
            if c.is_ascii_whitespace() {
                if !self.ultimo_espaco {
                    limpo.push(' ');
                    self.ultimo_espaco = true;
                }
            } else {
                limpo.push(c);
                self.ultimo_espaco = false;
            }
        }
        self.acrescentar(&limpo);
    }

    fn texto_pre(&mut self, texto: &str) {
        let mut limpo = String::with_capacity(texto.len());
        for c in texto.chars() {
            match c {
                '\r' => continue, // \r\n conta como \n
                '\n' if self.pular_nova_linha => {}
                '\t' => limpo.push_str("    "),
                c => limpo.push(c),
            }
            self.pular_nova_linha = false;
        }
        self.ultimo_espaco = false;
        self.acrescentar(&limpo);
    }

    /// Quebra de linha forcada (<br>).
    fn quebra(&mut self) {
        if self.pre > 0 {
            self.acrescentar("\n");
            return;
        }
        // espaco antes do <br> nao serve para nada
        if let Some(t) = self.atual.trechos.last_mut() {
            while t.texto.ends_with(' ') {
                t.texto.pop();
            }
        }
        self.acrescentar("\n");
        self.ultimo_espaco = true;
    }

    fn definir_titulo(&mut self, cru: &[u8]) {
        let texto = decodificar(cru);
        self.doc.titulo = juntar_espacos(&texto);
        self.tem_titulo = true;
    }

    fn abrir(&mut self, tag: &Tag) {
        let nome = tag.nome.as_str();
        if self.na_cabeca && !matches!(nome, "title" | "meta" | "link" | "script" | "style" | "base" | "noscript") {
            self.na_cabeca = false;
        }
        if self.em_celula() && bloco_leve(nome) {
            // <td><p>texto</p></td>: o bloco de dentro nao parte a linha
            self.quebra_na_celula();
            if nome == "li" {
                self.texto("* ");
            }
            return;
        }
        match nome {
            "head" => self.na_cabeca = true,
            "body" => self.na_cabeca = false,
            "p" => self.comecar(TipoBloco::Paragrafo, true),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let nivel = nome.as_bytes()[1] - b'0';
                self.comecar(TipoBloco::Titulo(nivel), true);
            }
            "br" => self.quebra(),
            "hr" => {
                self.comecar(TipoBloco::Regua, true);
                self.fechar_bloco();
                self.margem_pendente = true;
            }
            "pre" | "listing" | "xmp" | "plaintext" => {
                self.fechar_bloco();
                self.margem_pendente = true;
                self.pre += 1;
                self.pular_nova_linha = true;
                self.atual.tipo = TipoBloco::Pre;
            }
            "ul" | "ol" | "menu" | "dir" => self.abrir_lista(tag),
            "li" => self.abrir_item(tag),
            "blockquote" => {
                self.comecar(TipoBloco::Paragrafo, true);
                self.citacao = self.citacao.saturating_add(1);
                self.atual.recuo = self.recuo();
                self.atual.citacao = self.citacao;
            }
            "table" => {
                self.comecar(TipoBloco::Paragrafo, true);
                self.tabelas = self.tabelas.saturating_add(1);
                self.pilha_tabelas.push(self.tabelas);
            }
            "caption" => self.comecar(TipoBloco::Paragrafo, true),
            "tr" => self.comecar_linha(),
            "option" => self.texto(" "),
            "td" | "th" => self.abrir_celula(nome == "th"),
            "dl" => {
                self.fechar_bloco();
                self.margem_pendente = true;
            }
            "dt" => self.abrir_termo(),
            "dd" => self.abrir_definicao(),
            "div" | "section" | "article" | "header" | "footer" | "nav" | "main" | "aside" | "form"
            | "center" | "figure" | "figcaption" | "address" | "fieldset" | "details" | "summary"
            | "legend" | "tbody" | "thead" | "tfoot" | "noscript" => self.fechar_bloco(),
            "b" | "strong" => self.negrito = self.negrito.saturating_add(1),
            "i" | "em" | "cite" | "var" | "dfn" => self.italico = self.italico.saturating_add(1),
            "u" | "ins" => self.sublinhado = self.sublinhado.saturating_add(1),
            "code" | "tt" | "kbd" | "samp" => self.codigo = self.codigo.saturating_add(1),
            "a" => self.abrir_link(tag),
            "img" => self.imagem(tag),
            "input" => self.entrada(tag),
            _ => {}
        }
    }

    fn fechar(&mut self, nome: &str) {
        if self.em_celula() && bloco_leve(nome) {
            self.quebra_na_celula();
            return;
        }
        match nome {
            "head" => self.na_cabeca = false,
            "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "caption" => {
                self.sair_celula();
                self.fechar_bloco();
                self.margem_pendente = true;
            }
            "table" => {
                self.sair_celula();
                self.fechar_bloco();
                self.pilha_tabelas.pop();
                self.margem_pendente = true;
            }
            "dl" => {
                self.fechar_termo();
                self.fechar_definicao();
                self.margem_pendente = true;
            }
            "dt" => self.fechar_termo(),
            "dd" => self.fechar_definicao(),
            "pre" | "listing" | "xmp" | "plaintext" => {
                self.fechar_bloco();
                self.pre = self.pre.saturating_sub(1);
                self.atual.tipo = if self.pre > 0 { TipoBloco::Pre } else { TipoBloco::Paragrafo };
                self.margem_pendente = true;
            }
            "ul" | "ol" | "menu" | "dir" => {
                self.fechar_bloco();
                self.atual.marcador = Marcador::Nenhum;
                self.listas.pop();
                self.atual.recuo = self.recuo();
                if self.listas.is_empty() {
                    self.margem_pendente = true;
                }
            }
            "blockquote" => {
                self.fechar_bloco();
                self.citacao = self.citacao.saturating_sub(1);
                self.atual.recuo = self.recuo();
                self.atual.citacao = self.citacao;
                self.margem_pendente = true;
            }
            "li" | "tr" => {
                self.sair_celula();
                self.fechar_bloco();
                self.atual.marcador = Marcador::Nenhum;
            }
            "div" | "section" | "article" | "header" | "footer" | "nav" | "main" | "aside" | "form"
            | "center" | "figure" | "figcaption" | "address" | "fieldset" | "details" | "summary"
            | "legend" | "noscript" => self.fechar_bloco(),
            "th" | "td" => self.sair_celula(),
            "b" | "strong" => self.negrito = self.negrito.saturating_sub(1),
            "i" | "em" | "cite" | "var" | "dfn" => self.italico = self.italico.saturating_sub(1),
            "u" | "ins" => self.sublinhado = self.sublinhado.saturating_sub(1),
            "code" | "tt" | "kbd" | "samp" => self.codigo = self.codigo.saturating_sub(1),
            "a" => self.link = None,
            _ => {}
        }
    }

    fn abrir_lista(&mut self, tag: &Tag) {
        self.fechar_bloco();
        if self.listas.is_empty() {
            self.margem_pendente = true;
        }
        let ordenada = tag.nome == "ol";
        let inicio = tag.atributo("start").and_then(|s| s.parse::<u32>().ok()).unwrap_or(1);
        self.listas.push(Lista { ordenada, contador: inicio.saturating_sub(1) });
        self.atual.recuo = self.recuo();
    }

    fn abrir_item(&mut self, tag: &Tag) {
        self.fechar_bloco();
        let nivel = self.listas.len().max(1) as u8;
        let marcador = match self.listas.last_mut() {
            Some(lista) => {
                if let Some(valor) = tag.atributo("value").and_then(|s| s.parse::<u32>().ok()) {
                    lista.contador = valor.saturating_sub(1);
                }
                lista.contador = lista.contador.saturating_add(1);
                if lista.ordenada { Marcador::Numero(lista.contador) } else { Marcador::Bolinha(nivel) }
            }
            None => Marcador::Bolinha(1), // <li> fora de lista
        };
        self.atual.tipo = TipoBloco::Item;
        self.atual.marcador = marcador;
        self.atual.recuo = self.recuo().max(1);
    }

    /// <tr>: uma linha nova da tabela aberta (0 se nao ha <table>).
    fn comecar_linha(&mut self) {
        self.sair_celula();
        self.comecar(TipoBloco::Linha, false);
        self.atual.tabela = self.pilha_tabelas.last().copied().unwrap_or(0);
    }

    fn abrir_celula(&mut self, cabecalho: bool) {
        if self.atual.tipo != TipoBloco::Linha {
            // celula sem <tr>: comeca uma linha
            self.comecar_linha();
        }
        self.sair_celula();
        // os espacos do fim da celula de antes nao contam na largura dela
        aparar_fim(&mut self.atual);
        if self.atual.celulas.len() < MAX_CELULAS {
            let inicio = self.atual.trechos.len();
            self.atual.celulas.push(Celula { inicio, cabecalho });
            self.nova_celula = true;
        } else {
            self.texto(" "); // celulas demais: o resto vai junto na ultima
        }
        self.ultimo_espaco = true;
        if cabecalho {
            self.negrito = self.negrito.saturating_add(1);
            self.em_th = true;
        }
    }

    /// Entre <td> e </tr>?
    fn em_celula(&self) -> bool {
        self.atual.tipo == TipoBloco::Linha && !self.atual.celulas.is_empty()
    }

    /// Um bloco (p, div...) dentro da celula: so uma quebra de linha, e so
    /// se a celula ja tem texto e nao acabou de quebrar.
    fn quebra_na_celula(&mut self) {
        let trechos = &self.atual.trechos;
        let inicio = self.atual.celulas.last().map_or(0, |c| c.inicio).min(trechos.len());
        let tem_texto = trechos[inicio..].iter().any(|t| t.texto.bytes().any(|c| c != b' ' && c != b'\n'));
        let quebrou = trechos.last().is_some_and(|t| t.texto.ends_with('\n'));
        if tem_texto && !quebrou {
            self.quebra();
        }
    }

    /// <dt>: termo em negrito (fecha a definicao de antes, se ficou aberta).
    fn abrir_termo(&mut self) {
        self.fechar_definicao();
        self.fechar_termo();
        self.negrito = self.negrito.saturating_add(1);
        self.em_dt = true;
    }

    fn fechar_termo(&mut self) {
        self.fechar_bloco();
        if core::mem::take(&mut self.em_dt) {
            self.negrito = self.negrito.saturating_sub(1);
        }
    }

    /// <dd>: a definicao fica recuada embaixo do termo.
    fn abrir_definicao(&mut self) {
        self.fechar_termo();
        self.em_dd = true;
        self.atual.recuo = self.recuo();
    }

    fn fechar_definicao(&mut self) {
        self.fechar_bloco();
        if core::mem::take(&mut self.em_dd) {
            self.atual.recuo = self.recuo();
        }
    }

    /// Fim de uma celula <th> (com ou sem </th>): tira o negrito dela.
    fn sair_celula(&mut self) {
        if self.em_th {
            self.em_th = false;
            self.negrito = self.negrito.saturating_sub(1);
        }
    }

    fn abrir_link(&mut self, tag: &Tag) {
        self.link = None;
        let Some(href) = tag.atributo("href") else { return };
        let javascript = href.len() >= 11 && href.as_bytes()[..11].eq_ignore_ascii_case(b"javascript:");
        let ignorar = href.is_empty() || href.starts_with('#') || javascript;
        if ignorar {
            return;
        }
        self.doc.links.push(href.into());
        self.link = Some(self.doc.links.len() - 1);
    }

    fn imagem(&mut self, tag: &Tag) {
        let texto = match tag.atributo("alt") {
            Some("") => return, // imagem decorativa
            Some(alt) => juntar_espacos(alt),
            None => "imagem".into(),
        };
        let mut caixa = String::with_capacity(texto.len() + 2);
        caixa.push('[');
        caixa.push_str(&texto);
        caixa.push(']');
        self.texto(&caixa);
    }

    /// Botoes de formulario aparecem como [texto]; o resto, nada.
    fn entrada(&mut self, tag: &Tag) {
        let tipo = tag.atributo("type").unwrap_or("text");
        if tipo.eq_ignore_ascii_case("submit") || tipo.eq_ignore_ascii_case("button") {
            let valor = tag.atributo("value").filter(|v| !v.is_empty()).unwrap_or("Enviar");
            let mut caixa = String::from("[");
            caixa.push_str(&juntar_espacos(valor));
            caixa.push(']');
            self.texto(&caixa);
        } else if !tipo.eq_ignore_ascii_case("hidden") {
            self.texto("[____]");
        }
    }

    fn terminar(mut self) -> Documento {
        self.fechar_bloco();
        if !self.tem_titulo || self.doc.titulo.is_empty() {
            // sem <title>: usa o primeiro titulo da pagina
            if let Some(b) = self.doc.blocos.iter().find(|b| matches!(b.tipo, TipoBloco::Titulo(_))) {
                let texto: String = b.trechos.iter().map(|t| t.texto.as_str()).collect();
                self.doc.titulo = juntar_espacos(&texto);
            }
        }
        self.doc
    }
}

fn bloco_vazio(tipo: TipoBloco, recuo: u8, citacao: u8) -> Bloco {
    Bloco {
        tipo,
        recuo,
        citacao,
        marcador: Marcador::Nenhum,
        margem: false,
        trechos: Vec::new(),
        celulas: Vec::new(),
        tabela: 0,
    }
}

/// Tags de bloco que, dentro de uma celula, viram so uma quebra de linha.
fn bloco_leve(nome: &str) -> bool {
    matches!(
        nome,
        "p" | "div" | "section" | "article" | "header" | "footer" | "nav" | "main" | "aside" | "form" | "center"
            | "figure" | "figcaption" | "address" | "fieldset" | "details" | "summary" | "legend" | "noscript"
            | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "dl" | "dt" | "dd" | "blockquote" | "ul" | "ol" | "menu"
            | "dir" | "li" | "hr"
    )
}

/// Tira espacos e quebras do fim do bloco (e trechos que ficaram vazios).
/// Celulas que perderam os trechos continuam la, vazias.
fn aparar_fim(bloco: &mut Bloco) {
    let trechos = &mut bloco.trechos;
    while let Some(t) = trechos.last_mut() {
        while t.texto.ends_with(' ') || t.texto.ends_with('\n') {
            t.texto.pop();
        }
        if !t.texto.is_empty() {
            break;
        }
        trechos.pop();
    }
    let n = trechos.len();
    for c in &mut bloco.celulas {
        c.inicio = c.inicio.min(n);
    }
}

/// Espacos seguidos viram um so, sem espaco nas pontas.
pub fn juntar_espacos(texto: &str) -> String {
    let mut saida = String::with_capacity(texto.len());
    for palavra in texto.split_ascii_whitespace() {
        if !saida.is_empty() {
            saida.push(' ');
        }
        saida.push_str(palavra);
    }
    saida
}
