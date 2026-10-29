//! FAT32 sobre um `Disco` de setores de 512 bytes, escrito do zero.
//!
//! Disco: setor 0 e um MBR (usamos a primeira particao tipo 0x0B/0x0C) ou
//! ja e o setor de boot do FAT32 (disco inteiro, "superfloppy").
//!
//! Volume, a partir do primeiro setor dele:
//!   setores reservados (boot, FSInfo...) | FAT 1 | FAT 2 | dados
//! Os dados sao divididos em clusters numerados a partir de 2. A FAT tem
//! uma entrada de 32 bits (so 28 contam) por cluster:
//!   0 = livre, 2.. = proximo cluster do arquivo, >= 0x0FFFFFF8 = fim.
//!
//! Pasta = arquivo de entradas de 32 bytes. Nome curto 8.3 na entrada
//! principal; o nome longo (LFN) vem antes dela, em pedacos de 13
//! caracteres UCS-2, do ultimo para o primeiro, com o checksum do nome curto.
//! Os nomes longos sao UTF-8 na memoria e UTF-16 no disco (so o BMP:
//! "Anotações.txt" funciona); o nome curto continua ASCII, sem acento.
//!
//! Cuidados:
//!   - as duas FATs recebem toda escrita (a nao ser com espelhamento desligado)
//!   - o FSInfo guarda quantos clusters estao livres; contamos na montagem
//!     e atualizamos ao fim de cada operacao
//!   - cluster novo de pasta e zerado no disco antes de entrar na cadeia
//!   - cada operacao publica termina em `concluir` (FAT, FSInfo, flush)
//!
//! Este arquivo nao depende do resto do kernel (so de `super::{Disco, Erro,
//! ItemPasta}` e do alloc), para poder ser testado no PC tambem.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use super::{Disco, Erro, ItemPasta};

const SETOR: usize = 512;
const ENTRADA: usize = 32;

const FIM_CADEIA: u32 = 0x0FFF_FFFF;
const MINIMO_FIM: u32 = 0x0FFF_FFF8;
const MASCARA: u32 = 0x0FFF_FFFF;

// Atributos de uma entrada de pasta.
const ATTR_ROTULO: u8 = 0x08;
const ATTR_PASTA: u8 = 0x10;
const ATTR_ARQUIVO: u8 = 0x20;
const ATTR_LFN: u8 = 0x0F;

// Byte 0 da entrada.
const LIVRE_FIM: u8 = 0x00;
const APAGADA: u8 = 0xE5;
/// Byte 12: base e extensao do nome curto em minusculas (Windows NT).
const CAIXA_BASE: u8 = 0x08;
const CAIXA_EXT: u8 = 0x10;

/// Bits do numero de ordem de uma entrada LFN.
const LFN_ULTIMA: u8 = 0x40;
const LFN_ORDEM: u8 = 0x3F;
const CHARS_POR_LFN: usize = 13;
/// Posicao (byte) de cada um dos 13 caracteres UCS-2 numa entrada LFN.
const POSICOES_LFN: [usize; CHARS_POR_LFN] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];

const MAX_NOME: usize = 255;
/// A spec limita a pasta a 65536 entradas.
const MAX_BYTES_PASTA: usize = 65536 * ENTRADA;
/// Setores da FAT guardados na memoria (8 KB = 2048 entradas).
const SETORES_CACHE_FAT: u32 = 16;
/// Setores no maximo por pedido ao disco (64 KB).
const MAX_SETORES_PEDIDO: usize = 128;

const NOME_PONTO: [u8; 11] = *b".          ";
const NOME_PONTO_PONTO: [u8; 11] = *b"..         ";

/// Caracteres aceitos num nome curto alem de A-Z e 0-9.
const ESPECIAIS_CURTOS: &[u8] = b"$%'-_@~`!(){}^#&";
/// Caracteres proibidos num nome longo.
const PROIBIDOS: &[u8] = b"\\/:*?\"<>|";

/// Letras Latin-1 de 0xC0 a 0xFF sem acento (o resto vira '?').
const LATIN1_SEM_ACENTO: &[u8; 64] = b"AAAAAAACEEEEIIIIDNOOOOOxOUUUUY?saaaaaaaceeeeiiiidnooooo/ouuuuy?y";

/// Data e hora no formato da FAT: (data, hora).
/// data = (ano-1980)<<9 | mes<<5 | dia; hora = h<<11 | min<<5 | seg/2.
pub type Relogio = fn() -> (u16, u16);

fn le16(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn le32(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

fn gravar16(b: &mut [u8], i: usize, valor: u16) {
    b[i..i + 2].copy_from_slice(&valor.to_le_bytes());
}

fn gravar32(b: &mut [u8], i: usize, valor: u32) {
    b[i..i + 4].copy_from_slice(&valor.to_le_bytes());
}

// ---------------------------------------------------------------- geometria

/// Onde fica cada parte do volume (setores absolutos do disco).
#[derive(Clone, Copy)]
struct Geometria {
    inicio: u64,
    setores_por_cluster: u32,
    bytes_por_cluster: usize,
    reservados: u32,
    fats: u32,
    setores_por_fat: u32,
    inicio_dados: u64,
    /// Clusters de dados (numerados de 2 a clusters+1).
    clusters: u32,
    raiz: u32,
    setor_fsinfo: u32,
    /// false = so a FAT `fat_ativa` vale (ExtFlags bit 7).
    espelhada: bool,
    fat_ativa: u32,
}

/// O setor parece o boot de um FAT32? (assinatura, salto e BPB coerente)
fn bpb_valido(s: &[u8]) -> bool {
    let por_cluster = s[13];
    s[510] == 0x55
        && s[511] == 0xAA
        && (s[0] == 0xEB || s[0] == 0xE9)
        && le16(s, 11) as usize == SETOR
        && por_cluster != 0
        && por_cluster.is_power_of_two()
        && le16(s, 14) != 0
        && (s[16] == 1 || s[16] == 2)
        && le16(s, 22) == 0 // FATSz16 = 0: e FAT32
        && le32(s, 36) != 0
}

/// Primeira particao FAT32 (0x0B ou 0x0C) do MBR.
fn particao_fat32(mbr: &[u8]) -> Result<u64, Erro> {
    if mbr[510] != 0x55 || mbr[511] != 0xAA {
        return Err(Erro::Corrompido("disco sem MBR nem FAT32"));
    }
    for i in 0..4 {
        let e = 446 + 16 * i;
        let (tipo, inicio, setores) = (mbr[e + 4], le32(mbr, e + 8), le32(mbr, e + 12));
        if (tipo == 0x0B || tipo == 0x0C) && setores != 0 {
            return Ok(inicio as u64);
        }
    }
    Err(Erro::Corrompido("nenhuma partição FAT32 no MBR"))
}

impl Geometria {
    fn ler(bpb: &[u8], inicio: u64, setores_disco: u64) -> Result<Geometria, Erro> {
        let setores_por_cluster = bpb[13] as u32;
        let reservados = le16(bpb, 14) as u32;
        let fats = bpb[16] as u32;
        let total = match le16(bpb, 19) {
            0 => le32(bpb, 32) as u64,
            t => t as u64,
        };
        let setores_por_fat = le32(bpb, 36);
        let flags = le16(bpb, 40);
        let raiz = le32(bpb, 44);
        if inicio + total > setores_disco {
            return Err(Erro::Corrompido("volume maior que o disco"));
        }
        let antes_dos_dados = reservados as u64 + fats as u64 * setores_por_fat as u64;
        if total <= antes_dos_dados {
            return Err(Erro::Corrompido("volume sem área de dados"));
        }
        // a FAT tambem limita: 128 entradas por setor, menos as 2 primeiras
        let pela_fat = (setores_por_fat as u64 * (SETOR as u64 / 4)).saturating_sub(2);
        let clusters = ((total - antes_dos_dados) / setores_por_cluster as u64).min(pela_fat).min(MASCARA as u64 - 16) as u32;
        let geo = Geometria {
            inicio,
            setores_por_cluster,
            bytes_por_cluster: setores_por_cluster as usize * SETOR,
            reservados,
            fats,
            setores_por_fat,
            inicio_dados: inicio + antes_dos_dados,
            clusters,
            raiz,
            setor_fsinfo: le16(bpb, 48) as u32,
            espelhada: flags & 0x80 == 0,
            fat_ativa: (flags & 0x0F) as u32,
        };
        if clusters == 0 || !geo.cluster_valido(raiz) || geo.fat_ativa >= fats {
            return Err(Erro::Corrompido("setor de boot FAT32 inválido"));
        }
        Ok(geo)
    }

    fn cluster_valido(&self, c: u32) -> bool {
        c >= 2 && c < self.clusters + 2
    }

    fn lba_cluster(&self, c: u32) -> u64 {
        self.inicio_dados + (c - 2) as u64 * self.setores_por_cluster as u64
    }

    fn lba_fat(&self, copia: u32) -> u64 {
        self.inicio + self.reservados as u64 + copia as u64 * self.setores_por_fat as u64
    }

    /// FAT de onde lemos.
    fn fat_leitura(&self) -> u32 {
        if self.espelhada { 0 } else { self.fat_ativa }
    }

    fn fsinfo_existe(&self) -> bool {
        self.setor_fsinfo != 0 && self.setor_fsinfo < self.reservados
    }

    fn clusters_para(&self, bytes: usize) -> usize {
        bytes.div_ceil(self.bytes_por_cluster)
    }
}

// ---------------------------------------------------------------- nomes

/// Checksum do nome curto que cada entrada LFN carrega.
fn checksum(curto: &[u8; 11]) -> u8 {
    curto.iter().fold(0u8, |soma, &b| soma.rotate_right(1).wrapping_add(b))
}

fn caractere_curto(c: u8) -> bool {
    c.is_ascii_uppercase() || c.is_ascii_digit() || ESPECIAIS_CURTOS.contains(&c)
}

/// Um caractere UCS-2 (ou byte OEM) do disco em ASCII imprimivel.
/// So para nomes curtos e rotulo; o nome longo guarda o acento.
fn para_ascii(u: u16) -> char {
    match u {
        0x20..=0x7E => u as u8 as char,
        0xC0..=0xFF => LATIN1_SEM_ACENTO[(u - 0xC0) as usize] as char,
        _ => '?',
    }
}

/// Caractere que pode ir num nome longo: sem controles, sem os proibidos
/// e so do BMP (um caractere UCS-2 por letra).
fn caractere_longo(c: char) -> bool {
    let proibido = c.is_ascii() && PROIBIDOS.contains(&(c as u8));
    !c.is_control() && !proibido && (c as u32) <= 0xFFFF
}

/// Nome que se pode criar: ate 255 caracteres UCS-2, sem os proibidos, sem
/// "." e "..", sem espaco ou ponto no fim (o Windows corta esses).
pub fn validar_nome(nome: &str) -> Result<(), Erro> {
    let invalido = nome.is_empty()
        || nome.chars().count() > MAX_NOME
        || nome == "."
        || nome == ".."
        || !nome.chars().all(caractere_longo)
        || nome.ends_with([' ', '.']);
    if invalido { Err(Erro::NomeInvalido) } else { Ok(()) }
}

/// Mesmo nome sem olhar maiusculas e minusculas, acentos inclusive
/// ("anotações.txt" acha "ANOTAÇÕES.TXT"), como o Windows.
fn mesmo_nome(a: &str, b: &str) -> bool {
    a.chars().flat_map(char::to_lowercase).eq(b.chars().flat_map(char::to_lowercase))
}

/// Base e extensao, se o nome cabe em 8.3 (letras de qualquer caixa).
fn partes_83(nome: &str) -> Option<(&str, &str)> {
    let (base, ext) = nome.split_once('.').unwrap_or((nome, ""));
    let cabe = (1..=8).contains(&base.len()) && ext.len() <= 3 && !ext.contains('.');
    let validos = base.bytes().chain(ext.bytes()).all(|c| caractere_curto(c.to_ascii_uppercase()));
    (cabe && validos).then_some((base, ext))
}

/// Toda maiuscula ou toda minuscula?
fn caixa_uniforme(texto: &str) -> bool {
    !texto.bytes().any(|c| c.is_ascii_lowercase()) || !texto.bytes().any(|c| c.is_ascii_uppercase())
}

fn montar_83(base: &[u8], ext: &[u8]) -> [u8; 11] {
    let mut curto = [b' '; 11];
    for (i, &c) in base.iter().take(8).enumerate() {
        curto[i] = c.to_ascii_uppercase();
    }
    for (i, &c) in ext.iter().take(3).enumerate() {
        curto[8 + i] = c.to_ascii_uppercase();
    }
    curto
}

/// Se o nome pode ficar so com a entrada curta: (nome 8.3, bits de caixa).
/// Como o Windows NT: cada parte toda maiuscula ou toda minuscula.
fn so_curto(nome: &str) -> Option<([u8; 11], u8)> {
    let (base, ext) = partes_83(nome)?;
    if !caixa_uniforme(base) || !caixa_uniforme(ext) {
        return None;
    }
    let mut caixa = 0;
    if base.bytes().any(|c| c.is_ascii_lowercase()) {
        caixa |= CAIXA_BASE;
    }
    if ext.bytes().any(|c| c.is_ascii_lowercase()) {
        caixa |= CAIXA_EXT;
    }
    Some((montar_83(base.as_bytes(), ext.as_bytes()), caixa))
}

/// A letra sem acento ("ç" vira 'c', "É" vira 'E'); fora do Latin-1 fica igual.
pub fn sem_acento(c: char) -> char {
    match c as u32 {
        u @ 0xC0..=0xFF => LATIN1_SEM_ACENTO[(u - 0xC0) as usize] as char,
        _ => c,
    }
}

/// Letra do nome longo no nome curto: "ç" vira 'C', "é" vira 'E'.
fn letra_curta(c: char) -> u8 {
    let c = sem_acento(c);
    let ascii = if c.is_ascii() { c as u8 } else { b'_' }.to_ascii_uppercase();
    if caractere_curto(ascii) { ascii } else { b'_' }
}

/// Maiusculas sem acento, sem espacos e pontos, invalidos viram '_'.
fn limpar_para_curto(texto: &str) -> Vec<u8> {
    texto.chars().filter(|&c| c != ' ' && c != '.').map(letra_curta).collect()
}

/// Nome curto BASE~N.EXT que ainda nao existe na pasta.
fn gerar_curto(nome: &str, existentes: &[[u8; 11]]) -> Result<[u8; 11], Erro> {
    let sem_pontos = nome.trim_start_matches('.');
    let (base, ext) = sem_pontos.rsplit_once('.').unwrap_or((sem_pontos, ""));
    let mut base = limpar_para_curto(base);
    if base.is_empty() {
        base.push(b'_');
    }
    let ext = limpar_para_curto(ext);
    let mut numero = [0u8; 8];
    for n in 1..1_000_000u32 {
        // "~N" em ASCII, de tras para frente
        let mut k = numero.len();
        let mut resto = n;
        while resto > 0 {
            k -= 1;
            numero[k] = b'0' + (resto % 10) as u8;
            resto /= 10;
        }
        k -= 1;
        numero[k] = b'~';
        let sufixo = &numero[k..];
        let mut candidato_base: Vec<u8> = base.iter().copied().take(8 - sufixo.len()).collect();
        candidato_base.extend_from_slice(sufixo);
        let candidato = montar_83(&candidato_base, &ext);
        if !existentes.contains(&candidato) {
            return Ok(candidato);
        }
    }
    Err(Erro::Cheio)
}

/// Nome curto para mostrar: "LEIAME.TXT", "config" (com os bits de caixa).
fn texto_curto(curto: &[u8; 11], caixa: u8) -> String {
    let mut nome = String::new();
    let parte = |bytes: &[u8], minuscula: bool, nome: &mut String| {
        let fim = bytes.iter().rposition(|&c| c != b' ').map_or(0, |p| p + 1);
        for (i, &c) in bytes[..fim].iter().enumerate() {
            // 0x05 no primeiro byte quer dizer 0xE5 (que marca apagada)
            let c = if i == 0 && c == 0x05 { '?' } else { para_ascii(c as u16) };
            nome.push(if minuscula { c.to_ascii_lowercase() } else { c });
        }
    };
    parte(&curto[..8], caixa & CAIXA_BASE != 0, &mut nome);
    if curto[8..].iter().any(|&c| c != b' ') {
        nome.push('.');
        parte(&curto[8..], caixa & CAIXA_EXT != 0, &mut nome);
    }
    nome
}

/// Rotulo de volume: os 11 bytes inteiros (sem ponto), sem os espacos do fim.
fn texto_rotulo(bytes: &[u8]) -> String {
    let texto: String = bytes.iter().map(|&c| para_ascii(c as u16)).collect();
    String::from(texto.trim_end())
}

/// Entradas LFN do nome, na ordem em que vao para o disco.
fn entradas_lfn(nome: &str, curto: &[u8; 11]) -> Vec<[u8; ENTRADA]> {
    let mut unidades: Vec<u16> = nome.encode_utf16().collect();
    if unidades.len() % CHARS_POR_LFN != 0 {
        unidades.push(0); // terminador, so se sobrar espaco
    }
    while unidades.len() % CHARS_POR_LFN != 0 {
        unidades.push(0xFFFF);
    }
    let pedacos = unidades.len() / CHARS_POR_LFN;
    let soma = checksum(curto);
    let mut entradas = Vec::with_capacity(pedacos);
    for ordem in (1..=pedacos).rev() {
        let mut e = [0u8; ENTRADA];
        e[0] = ordem as u8 | if ordem == pedacos { LFN_ULTIMA } else { 0 };
        e[11] = ATTR_LFN;
        e[13] = soma;
        let pedaco = &unidades[(ordem - 1) * CHARS_POR_LFN..ordem * CHARS_POR_LFN];
        for (&posicao, &u) in POSICOES_LFN.iter().zip(pedaco) {
            gravar16(&mut e, posicao, u);
        }
        entradas.push(e);
    }
    entradas
}

/// Os 13 caracteres de uma entrada LFN.
fn ler_lfn(e: &[u8]) -> [u16; CHARS_POR_LFN] {
    let mut unidades = [0u16; CHARS_POR_LFN];
    for (u, &posicao) in unidades.iter_mut().zip(POSICOES_LFN.iter()) {
        *u = le16(e, posicao);
    }
    unidades
}

/// Junta os pedacos LFN (indice = ordem - 1) num nome UTF-8. Um UTF-16
/// quebrado (metade de um par) vira '?'.
fn nome_lfn(pedacos: &[[u16; CHARS_POR_LFN]]) -> String {
    let unidades = pedacos.iter().flat_map(|p| p.iter().copied()).take_while(|&u| u != 0 && u != 0xFFFF);
    char::decode_utf16(unidades).map(|c| c.unwrap_or('?')).collect()
}

/// Entrada curta: nome, atributos, cluster, tamanho e datas.
fn entrada_curta(curto: &[u8; 11], caixa: u8, atributos: u8, cluster: u32, tamanho: u32, (data, hora): (u16, u16)) -> [u8; ENTRADA] {
    let mut e = [0u8; ENTRADA];
    e[..11].copy_from_slice(curto);
    e[11] = atributos;
    e[12] = caixa;
    gravar16(&mut e, 14, hora); // criacao
    gravar16(&mut e, 16, data);
    gravar16(&mut e, 18, data); // ultimo acesso
    gravar16(&mut e, 22, hora); // ultima escrita
    gravar16(&mut e, 24, data);
    definir_cluster(&mut e, cluster);
    gravar32(&mut e, 28, tamanho);
    e
}

fn cluster_da_entrada(e: &[u8]) -> u32 {
    (le16(e, 20) as u32) << 16 | le16(e, 26) as u32
}

fn definir_cluster(e: &mut [u8], cluster: u32) {
    gravar16(e, 20, (cluster >> 16) as u16);
    gravar16(e, 26, cluster as u16);
}

// ---------------------------------------------------------------- caminhos

/// "/a/./b/../c/" -> ["a", "c"]. Barra invertida tambem separa.
fn componentes(caminho: &str) -> Vec<&str> {
    let mut partes: Vec<&str> = Vec::new();
    for parte in caminho.split(['/', '\\']) {
        match parte {
            "" | "." => {}
            ".." => {
                partes.pop();
            }
            nome => partes.push(nome),
        }
    }
    partes
}

// ---------------------------------------------------------------- pastas

/// Uma pasta inteira na memoria (pastas sao pequenas).
struct Pasta {
    /// Primeiro cluster (o da raiz e `geo.raiz`, nunca 0).
    cluster: u32,
    cadeia: Vec<u32>,
    dados: Vec<u8>,
    /// Setores alterados que `gravar_pasta` precisa escrever.
    sujos: Vec<bool>,
}

impl Pasta {
    fn marcar(&mut self, de: usize, ate: usize) {
        for setor in de / SETOR..ate.div_ceil(SETOR) {
            self.sujos[setor] = true;
        }
    }

    fn entrada(&self, posicao: usize) -> &[u8] {
        &self.dados[posicao..posicao + ENTRADA]
    }

    /// Nomes curtos ja usados (para gerar um novo sem repetir).
    fn curtos(&self) -> Vec<[u8; 11]> {
        self.dados
            .chunks_exact(ENTRADA)
            .take_while(|e| e[0] != LIVRE_FIM)
            .filter(|e| e[0] != APAGADA && e[11] & 0x3F != ATTR_LFN)
            .map(|e| {
                let mut curto = [0u8; 11];
                curto.copy_from_slice(&e[..11]);
                curto
            })
            .collect()
    }

    /// Entrada curta com este nome exato (".", "..").
    fn posicao_curta(&self, curto: &[u8; 11]) -> Option<usize> {
        self.dados
            .chunks_exact(ENTRADA)
            .take_while(|e| e[0] != LIVRE_FIM)
            .position(|e| e[0] != APAGADA && e[11] & 0x3F != ATTR_LFN && e[..11] == curto[..])
            .map(|i| i * ENTRADA)
    }
}

/// Um item de uma pasta, ja com o nome longo resolvido.
struct Entrada {
    nome: String,
    curto: [u8; 11],
    atributos: u8,
    cluster: u32,
    tamanho: u32,
    /// Byte da entrada curta dentro da pasta.
    posicao: usize,
    /// Byte da primeira entrada LFN (igual a `posicao` sem nome longo).
    inicio: usize,
}

impl Entrada {
    fn e_pasta(&self) -> bool {
        self.atributos & ATTR_PASTA != 0
    }

    fn chama(&self, nome: &str) -> bool {
        mesmo_nome(&self.nome, nome) || texto_curto(&self.curto, 0).eq_ignore_ascii_case(nome)
    }
}

/// Nome longo sendo montado enquanto lemos as entradas LFN.
struct Lfn {
    pedacos: Vec<[u16; CHARS_POR_LFN]>,
    soma: u8,
    /// Ordem da proxima entrada esperada (0 = nenhum nome em andamento).
    esperada: u8,
    inicio: usize,
}

impl Lfn {
    fn novo() -> Lfn {
        Lfn { pedacos: Vec::new(), soma: 0, esperada: 0, inicio: 0 }
    }

    fn descartar(&mut self) {
        self.pedacos.clear();
        self.esperada = 0;
    }

    /// Acrescenta uma entrada LFN; fora de ordem descarta o nome.
    fn acrescentar(&mut self, e: &[u8], posicao: usize) {
        let ordem = e[0] & LFN_ORDEM;
        if e[0] & LFN_ULTIMA != 0 && ordem != 0 {
            self.pedacos = vec![[0xFFFF; CHARS_POR_LFN]; ordem as usize];
            self.soma = e[13];
            self.inicio = posicao;
        } else if ordem == 0 || ordem + 1 != self.esperada || e[13] != self.soma {
            self.descartar();
            return;
        }
        self.pedacos[ordem as usize - 1] = ler_lfn(e);
        self.esperada = ordem;
    }

    /// O nome longo, se ele esta completo e e desta entrada curta.
    fn terminar(&mut self, curto: &[u8; 11]) -> Option<(String, usize)> {
        let completo = self.esperada == 1 && self.soma == checksum(curto);
        let resultado = completo.then(|| (nome_lfn(&self.pedacos), self.inicio));
        self.descartar();
        resultado.filter(|(nome, _)| !nome.is_empty())
    }
}

/// Itens da pasta (sem ".", ".." e rotulo).
fn entradas(pasta: &Pasta) -> Vec<Entrada> {
    let mut itens = Vec::new();
    let mut lfn = Lfn::novo();
    for (i, e) in pasta.dados.chunks_exact(ENTRADA).enumerate() {
        let posicao = i * ENTRADA;
        match e[0] {
            LIVRE_FIM => break,
            APAGADA => lfn.descartar(),
            _ if e[11] & 0x3F == ATTR_LFN => lfn.acrescentar(e, posicao),
            _ => {
                let mut curto = [0u8; 11];
                curto.copy_from_slice(&e[..11]);
                let longo = lfn.terminar(&curto);
                if e[11] & ATTR_ROTULO != 0 || curto == NOME_PONTO || curto == NOME_PONTO_PONTO {
                    continue;
                }
                let (nome, inicio) = longo.unwrap_or_else(|| (texto_curto(&curto, e[12]), posicao));
                itens.push(Entrada {
                    nome,
                    curto,
                    atributos: e[11],
                    cluster: cluster_da_entrada(e),
                    tamanho: le32(e, 28),
                    posicao,
                    inicio,
                });
            }
        }
    }
    itens
}

fn achar(pasta: &Pasta, nome: &str) -> Option<Entrada> {
    entradas(pasta).into_iter().find(|e| e.chama(nome))
}

/// Primeiro trecho com `quantas` entradas livres seguidas. Depois do
/// marcador de fim (byte 0 = 0) tudo esta livre. Devolve (byte, se o trecho
/// passa do marcador).
fn vagas(dados: &[u8], quantas: usize) -> Option<(usize, bool)> {
    let total = dados.len() / ENTRADA;
    let mut inicio = 0;
    let mut seguidas = 0;
    for i in 0..total {
        match dados[i * ENTRADA] {
            LIVRE_FIM => {
                if seguidas == 0 {
                    inicio = i;
                }
                return (seguidas + total - i >= quantas).then_some((inicio * ENTRADA, true));
            }
            APAGADA => {
                if seguidas == 0 {
                    inicio = i;
                }
                seguidas += 1;
                if seguidas == quantas {
                    return Some((inicio * ENTRADA, false));
                }
            }
            _ => seguidas = 0,
        }
    }
    None
}

// ---------------------------------------------------------------- volume

/// Pedaco da FAT guardado na memoria.
struct CacheFat {
    /// Primeiro setor (relativo ao inicio da FAT) e se ha algo carregado.
    primeiro: u32,
    valido: bool,
    sujo: bool,
    dados: Vec<u8>,
}

pub struct Fat32 {
    disco: Box<dyn Disco>,
    geo: Geometria,
    livres: u32,
    proximo_livre: u32,
    fsinfo_sujo: bool,
    rotulo: String,
    cache: CacheFat,
    relogio: Relogio,
}

impl Fat32 {
    /// Acha o volume (MBR ou disco inteiro), le o BPB, conta os clusters
    /// livres e le o rotulo.
    pub fn montar(mut disco: Box<dyn Disco>, relogio: Relogio) -> Result<Fat32, Erro> {
        let mut setor = vec![0u8; SETOR];
        disco.ler(0, &mut setor)?;
        let inicio = if bpb_valido(&setor) { 0 } else { particao_fat32(&setor)? };
        if inicio != 0 {
            disco.ler(inicio, &mut setor)?;
            if !bpb_valido(&setor) {
                return Err(Erro::Corrompido("setor de boot FAT32 inválido"));
            }
        }
        let geo = Geometria::ler(&setor, inicio, disco.setores())?;
        let rotulo_bpb = texto_rotulo(&setor[71..82]);
        let mut fs = Fat32 {
            disco,
            geo,
            livres: 0,
            proximo_livre: 2,
            fsinfo_sujo: false,
            rotulo: String::new(),
            cache: CacheFat { primeiro: 0, valido: false, sujo: false, dados: Vec::new() },
            relogio,
        };
        fs.contar_livres()?;
        fs.rotulo = fs.ler_rotulo()?.unwrap_or(rotulo_bpb);
        if fs.rotulo == "NO NAME" {
            fs.rotulo.clear();
        }
        Ok(fs)
    }

    pub fn nome_disco(&self) -> &str {
        self.disco.nome()
    }

    pub fn setores_disco(&self) -> u64 {
        self.disco.setores()
    }

    pub fn rotulo(&self) -> &str {
        &self.rotulo
    }

    /// Bytes da area de dados (o que cabe em arquivos e pastas).
    pub fn bytes_totais(&self) -> u64 {
        self.geo.clusters as u64 * self.geo.bytes_por_cluster as u64
    }

    pub fn bytes_livres(&self) -> u64 {
        self.livres as u64 * self.geo.bytes_por_cluster as u64
    }

    /// Devolve o disco (para resetar o dispositivo, por exemplo).
    pub fn desmontar(mut self) -> Box<dyn Disco> {
        let _ = self.concluir();
        self.disco
    }

    // ------------------------------------------------ montagem

    /// Conta as entradas 0 da FAT. Compara com o FSInfo, que e so uma dica.
    fn contar_livres(&mut self) -> Result<(), Erro> {
        let geo = self.geo;
        let mut buffer = vec![0u8; MAX_SETORES_PEDIDO * SETOR];
        let ultimo = geo.clusters as usize + 2;
        let mut primeiro_livre = None;
        let mut livres = 0u32;
        let mut setor = 0usize;
        while setor * SETOR / 4 < ultimo {
            let n = MAX_SETORES_PEDIDO.min(geo.setores_por_fat as usize - setor);
            let pedaco = &mut buffer[..n * SETOR];
            self.disco.ler(geo.lba_fat(geo.fat_leitura()) + setor as u64, pedaco)?;
            for (k, e) in pedaco.chunks_exact(4).enumerate() {
                let cluster = setor * SETOR / 4 + k;
                if cluster >= 2 && cluster < ultimo && le32(e, 0) & MASCARA == 0 {
                    livres += 1;
                    primeiro_livre.get_or_insert(cluster as u32);
                }
            }
            setor += n;
        }
        self.livres = livres;
        self.proximo_livre = primeiro_livre.unwrap_or(2);
        if let Some((contagem, proximo)) = self.ler_fsinfo()? {
            if geo.cluster_valido(proximo) {
                self.proximo_livre = proximo;
            }
            // contagem errada (desligou no meio?): corrige na proxima escrita
            self.fsinfo_sujo = contagem != livres;
        }
        Ok(())
    }

    /// (livres, proximo livre) do FSInfo, se ele tem as assinaturas certas.
    fn ler_fsinfo(&mut self) -> Result<Option<(u32, u32)>, Erro> {
        if !self.geo.fsinfo_existe() {
            return Ok(None);
        }
        let mut s = vec![0u8; SETOR];
        self.disco.ler(self.geo.inicio + self.geo.setor_fsinfo as u64, &mut s)?;
        if !fsinfo_valido(&s) {
            return Ok(None);
        }
        Ok(Some((le32(&s, 488), le32(&s, 492))))
    }

    fn gravar_fsinfo(&mut self) -> Result<(), Erro> {
        if !self.geo.fsinfo_existe() {
            return Ok(());
        }
        let lba = self.geo.inicio + self.geo.setor_fsinfo as u64;
        let mut s = vec![0u8; SETOR];
        self.disco.ler(lba, &mut s)?;
        if fsinfo_valido(&s) {
            gravar32(&mut s, 488, self.livres);
            gravar32(&mut s, 492, self.proximo_livre);
            self.disco.escrever(lba, &s)?;
        }
        Ok(())
    }

    /// Entrada de rotulo de volume na raiz (atributo 0x08).
    fn ler_rotulo(&mut self) -> Result<Option<String>, Erro> {
        let raiz = self.abrir_pasta(self.geo.raiz)?;
        let rotulo = raiz
            .dados
            .chunks_exact(ENTRADA)
            .take_while(|e| e[0] != LIVRE_FIM)
            .find(|e| e[0] != APAGADA && e[11] & 0x3F != ATTR_LFN && e[11] & ATTR_ROTULO != 0)
            .map(|e| texto_rotulo(&e[..11]));
        Ok(rotulo)
    }

    /// Termina uma operacao: FAT na memoria para o disco, FSInfo e flush.
    fn concluir(&mut self) -> Result<(), Erro> {
        self.descarregar_fat()?;
        if self.fsinfo_sujo {
            self.gravar_fsinfo()?;
            self.fsinfo_sujo = false;
        }
        self.disco.sincronizar()
    }

    /// Roda a operacao e sempre conclui (mesmo com erro no meio). Se o
    /// disco recusou alguma escrita (somente leitura, defeito), o que ficou
    /// so na memoria e esquecido e a contagem de livres volta a ser a do disco.
    fn operacao<R>(&mut self, acao: impl FnOnce(&mut Fat32) -> Result<R, Erro>) -> Result<R, Erro> {
        let resultado = acao(self);
        let fim = self.concluir();
        if fim.is_err() || matches!(resultado, Err(Erro::Disco)) {
            self.cache.valido = false;
            self.cache.sujo = false;
            self.fsinfo_sujo = false;
            let _ = self.contar_livres();
        }
        let valor = resultado?;
        fim.map(|_| valor)
    }

    // ------------------------------------------------ FAT

    /// Garante na cache o bloco que tem o setor `setor` da FAT.
    fn carregar_fat(&mut self, setor: u32) -> Result<(), Erro> {
        let bloco = setor / SETORES_CACHE_FAT * SETORES_CACHE_FAT;
        if self.cache.valido && self.cache.primeiro == bloco {
            return Ok(());
        }
        self.descarregar_fat()?;
        let n = SETORES_CACHE_FAT.min(self.geo.setores_por_fat - bloco) as usize;
        self.cache.dados.resize(n * SETOR, 0);
        self.cache.valido = false;
        let lba = self.geo.lba_fat(self.geo.fat_leitura()) + bloco as u64;
        self.disco.ler(lba, &mut self.cache.dados)?;
        self.cache.primeiro = bloco;
        self.cache.valido = true;
        Ok(())
    }

    /// Escreve o bloco alterado em todas as FATs que valem.
    fn descarregar_fat(&mut self) -> Result<(), Erro> {
        if !self.cache.valido || !self.cache.sujo {
            return Ok(());
        }
        for copia in 0..self.geo.fats {
            if self.geo.espelhada || copia == self.geo.fat_ativa {
                let lba = self.geo.lba_fat(copia) + self.cache.primeiro as u64;
                self.disco.escrever(lba, &self.cache.dados)?;
            }
        }
        self.cache.sujo = false;
        Ok(())
    }

    /// Posicao (byte na cache) da entrada do cluster, ja carregada.
    fn posicao_fat(&mut self, cluster: u32) -> Result<usize, Erro> {
        if !self.geo.cluster_valido(cluster) {
            return Err(Erro::Corrompido("cluster fora do volume"));
        }
        let byte = cluster as usize * 4;
        self.carregar_fat((byte / SETOR) as u32)?;
        Ok(byte - self.cache.primeiro as usize * SETOR)
    }

    fn ler_fat(&mut self, cluster: u32) -> Result<u32, Erro> {
        let p = self.posicao_fat(cluster)?;
        Ok(le32(&self.cache.dados, p) & MASCARA)
    }

    /// Os 4 bits de cima sao reservados e ficam como estavam.
    fn gravar_fat(&mut self, cluster: u32, valor: u32) -> Result<(), Erro> {
        let p = self.posicao_fat(cluster)?;
        let antigo = le32(&self.cache.dados, p);
        gravar32(&mut self.cache.dados, p, (antigo & !MASCARA) | (valor & MASCARA));
        self.cache.sujo = true;
        Ok(())
    }

    /// Clusters de um arquivo ou pasta (vazio se `primeiro` = 0).
    fn cadeia(&mut self, primeiro: u32) -> Result<Vec<u32>, Erro> {
        let mut cadeia = Vec::new();
        let mut atual = primeiro;
        if primeiro == 0 {
            return Ok(cadeia);
        }
        loop {
            if !self.geo.cluster_valido(atual) {
                return Err(Erro::Corrompido("cadeia de clusters inválida"));
            }
            if cadeia.len() > self.geo.clusters as usize {
                return Err(Erro::Corrompido("cadeia de clusters em laço"));
            }
            cadeia.push(atual);
            let proximo = self.ler_fat(atual)?;
            if proximo >= MINIMO_FIM {
                return Ok(cadeia);
            }
            atual = proximo;
        }
    }

    /// Reserva `quantos` clusters livres ja encadeados (o ultimo com fim).
    /// Se faltar espaco nada muda e o erro e Cheio.
    fn alocar(&mut self, quantos: usize) -> Result<Vec<u32>, Erro> {
        if quantos == 0 {
            return Ok(Vec::new());
        }
        if quantos > self.livres as usize {
            return Err(Erro::Cheio);
        }
        let mut novos = Vec::with_capacity(quantos);
        let mut atual = self.proximo_livre;
        for _ in 0..self.geo.clusters {
            if !self.geo.cluster_valido(atual) {
                atual = 2;
            }
            if self.ler_fat(atual)? == 0 {
                novos.push(atual);
                if novos.len() == quantos {
                    break;
                }
            }
            atual += 1;
        }
        if novos.len() < quantos {
            // a contagem estava errada: fica a verdade
            self.livres = novos.len() as u32;
            self.fsinfo_sujo = true;
            return Err(Erro::Cheio);
        }
        for i in 0..quantos {
            let proximo = novos.get(i + 1).copied().unwrap_or(FIM_CADEIA);
            self.gravar_fat(novos[i], proximo)?;
        }
        self.livres -= quantos as u32;
        self.proximo_livre = novos[quantos - 1] + 1;
        if !self.geo.cluster_valido(self.proximo_livre) {
            self.proximo_livre = 2;
        }
        self.fsinfo_sujo = true;
        Ok(novos)
    }

    fn liberar(&mut self, clusters: &[u32]) -> Result<(), Erro> {
        for &c in clusters {
            self.gravar_fat(c, 0)?;
        }
        self.livres += clusters.len() as u32;
        if let Some(&menor) = clusters.iter().min() {
            self.proximo_livre = self.proximo_livre.min(menor);
        }
        self.fsinfo_sujo |= !clusters.is_empty();
        Ok(())
    }

    /// Aumenta ou corta a cadeia para `quantos` clusters.
    fn ajustar_cadeia(&mut self, cadeia: &mut Vec<u32>, quantos: usize) -> Result<(), Erro> {
        if quantos > cadeia.len() {
            let novos = self.alocar(quantos - cadeia.len())?;
            if let Some(&ultimo) = cadeia.last() {
                self.gravar_fat(ultimo, novos[0])?;
            }
            cadeia.extend(novos);
        } else if quantos < cadeia.len() {
            let sobra = cadeia.split_off(quantos);
            if let Some(&ultimo) = cadeia.last() {
                self.gravar_fat(ultimo, FIM_CADEIA)?;
            }
            self.liberar(&sobra)?;
        }
        Ok(())
    }

    // ------------------------------------------------ clusters

    /// Trechos de clusters seguidos: (indice na cadeia, quantos clusters).
    fn trechos(&self, cadeia: &[u32]) -> Vec<(usize, usize)> {
        let maximo = (MAX_SETORES_PEDIDO / self.geo.setores_por_cluster as usize).max(1);
        let mut trechos: Vec<(usize, usize)> = Vec::new();
        for (i, &c) in cadeia.iter().enumerate() {
            match trechos.last_mut() {
                Some((inicio, n)) if cadeia[*inicio] + *n as u32 == c && *n < maximo => *n += 1,
                _ => trechos.push((i, 1)),
            }
        }
        trechos
    }

    /// Le a cadeia inteira em `destino` (cadeia.len() * bytes_por_cluster).
    fn ler_clusters(&mut self, cadeia: &[u32], destino: &mut [u8]) -> Result<(), Erro> {
        let bpc = self.geo.bytes_por_cluster;
        for (i, n) in self.trechos(cadeia) {
            let lba = self.geo.lba_cluster(cadeia[i]);
            self.disco.ler(lba, &mut destino[i * bpc..(i + n) * bpc])?;
        }
        Ok(())
    }

    /// Grava `dados` nos clusters da cadeia. O ultimo setor e completado
    /// com zeros; os setores depois do fim dos dados nao sao escritos.
    fn gravar_clusters(&mut self, cadeia: &[u32], dados: &[u8]) -> Result<(), Erro> {
        let bpc = self.geo.bytes_por_cluster;
        for (i, n) in self.trechos(cadeia) {
            let de = i * bpc;
            let ate = ((i + n) * bpc).min(dados.len());
            if de >= ate {
                break;
            }
            let lba = self.geo.lba_cluster(cadeia[i]);
            let inteiros = (ate - de) / SETOR * SETOR;
            if inteiros > 0 {
                self.disco.escrever(lba, &dados[de..de + inteiros])?;
            }
            if de + inteiros < ate {
                let mut ultimo = vec![0u8; SETOR];
                ultimo[..ate - de - inteiros].copy_from_slice(&dados[de + inteiros..ate]);
                self.disco.escrever(lba + (inteiros / SETOR) as u64, &ultimo)?;
            }
        }
        Ok(())
    }

    // ------------------------------------------------ pastas

    fn abrir_pasta(&mut self, cluster: u32) -> Result<Pasta, Erro> {
        let cluster = if cluster == 0 { self.geo.raiz } else { cluster };
        let cadeia = self.cadeia(cluster)?;
        let bytes = cadeia.len() * self.geo.bytes_por_cluster;
        if bytes > MAX_BYTES_PASTA {
            return Err(Erro::Corrompido("pasta grande demais"));
        }
        let mut dados = vec![0u8; bytes];
        self.ler_clusters(&cadeia, &mut dados)?;
        Ok(Pasta { cluster, cadeia, sujos: vec![false; bytes / SETOR], dados })
    }

    /// Escreve os setores alterados da pasta.
    fn gravar_pasta(&mut self, pasta: &mut Pasta) -> Result<(), Erro> {
        let setores_por_cluster = self.geo.setores_por_cluster as usize;
        for setor in 0..pasta.sujos.len() {
            if !pasta.sujos[setor] {
                continue;
            }
            let cluster = pasta.cadeia[setor / setores_por_cluster];
            let lba = self.geo.lba_cluster(cluster) + (setor % setores_por_cluster) as u64;
            self.disco.escrever(lba, &pasta.dados[setor * SETOR..(setor + 1) * SETOR])?;
            pasta.sujos[setor] = false;
        }
        Ok(())
    }

    /// Cluster da pasta do caminho, e os clusters de todas as pastas do
    /// caminho desde a raiz (para nao mover uma pasta para dentro dela).
    fn resolver_pasta(&mut self, partes: &[&str]) -> Result<(u32, Vec<u32>), Erro> {
        let mut atual = self.geo.raiz;
        let mut caminho = vec![atual];
        for parte in partes {
            let pasta = self.abrir_pasta(atual)?;
            let e = achar(&pasta, parte).ok_or(Erro::NaoAchado)?;
            if !e.e_pasta() {
                return Err(Erro::NaoEPasta);
            }
            atual = if e.cluster == 0 { self.geo.raiz } else { e.cluster };
            caminho.push(atual);
        }
        Ok((atual, caminho))
    }

    /// Pasta pai do caminho e o nome final. A raiz nao tem pai.
    fn pai_e_nome<'a>(&mut self, caminho: &'a str) -> Result<(Pasta, &'a str, Vec<u32>), Erro> {
        let partes = componentes(caminho);
        let (&nome, pai) = partes.split_last().ok_or(Erro::NomeInvalido)?;
        let (cluster, ancestrais) = self.resolver_pasta(pai)?;
        Ok((self.abrir_pasta(cluster)?, nome, ancestrais))
    }

    /// Acha (ou abre, crescendo a pasta) espaco para `quantas` entradas.
    fn reservar_vagas(&mut self, pasta: &mut Pasta, quantas: usize) -> Result<usize, Erro> {
        loop {
            if let Some((posicao, passa_do_fim)) = vagas(&pasta.dados, quantas) {
                let depois = posicao + quantas * ENTRADA;
                // o marcador de fim tem de continuar depois das entradas novas
                if passa_do_fim && depois < pasta.dados.len() && pasta.dados[depois] != LIVRE_FIM {
                    pasta.dados[depois] = LIVRE_FIM;
                    pasta.marcar(depois, depois + 1);
                }
                return Ok(posicao);
            }
            self.crescer_pasta(pasta, quantas)?;
        }
    }

    /// Mais clusters na pasta: zerados no disco antes de entrar na cadeia.
    fn crescer_pasta(&mut self, pasta: &mut Pasta, quantas: usize) -> Result<(), Erro> {
        let n = self.geo.clusters_para(quantas * ENTRADA);
        let bytes = n * self.geo.bytes_por_cluster;
        if pasta.dados.len() + bytes > MAX_BYTES_PASTA {
            return Err(Erro::Cheio);
        }
        let novos = self.alocar(n)?;
        self.gravar_clusters(&novos, &vec![0u8; bytes])?;
        if let Some(&ultimo) = pasta.cadeia.last() {
            self.gravar_fat(ultimo, novos[0])?;
        }
        pasta.cadeia.extend(novos);
        pasta.dados.resize(pasta.dados.len() + bytes, 0);
        pasta.sujos.resize(pasta.dados.len() / SETOR, false);
        Ok(())
    }

    /// Nome curto, bits de caixa e entradas LFN para um nome novo na pasta.
    fn nomes_para(&self, pasta: &Pasta, nome: &str) -> Result<([u8; 11], u8, Vec<[u8; ENTRADA]>), Erro> {
        if let Some((curto, caixa)) = so_curto(nome) {
            if !pasta.curtos().contains(&curto) {
                return Ok((curto, caixa, Vec::new()));
            }
        }
        let curto = gerar_curto(nome, &pasta.curtos())?;
        Ok((curto, 0, entradas_lfn(nome, &curto)))
    }

    /// Cria as entradas de `nome` com o resto da entrada curta vindo de
    /// `modelo` (atributos, cluster, tamanho, datas). Devolve a posicao.
    fn criar_entradas(&mut self, pasta: &mut Pasta, nome: &str, modelo: &[u8; ENTRADA]) -> Result<usize, Erro> {
        let (curto, caixa, lfn) = self.nomes_para(pasta, nome)?;
        let inicio = self.reservar_vagas(pasta, lfn.len() + 1)?;
        let mut posicao = inicio;
        for e in &lfn {
            pasta.dados[posicao..posicao + ENTRADA].copy_from_slice(e);
            posicao += ENTRADA;
        }
        let mut curta = *modelo;
        curta[..11].copy_from_slice(&curto);
        curta[12] = caixa;
        pasta.dados[posicao..posicao + ENTRADA].copy_from_slice(&curta);
        pasta.marcar(inicio, posicao + ENTRADA);
        Ok(posicao)
    }

    /// Marca como apagadas a entrada curta e as LFN dela.
    fn apagar_entradas(pasta: &mut Pasta, e: &Entrada) {
        for posicao in (e.inicio..=e.posicao).step_by(ENTRADA) {
            pasta.dados[posicao] = APAGADA;
        }
        pasta.marcar(e.inicio, e.posicao + ENTRADA);
    }

    // ------------------------------------------------ operacoes publicas

    /// Itens da pasta, na ordem do disco.
    pub fn listar(&mut self, caminho: &str) -> Result<Vec<ItemPasta>, Erro> {
        let (cluster, _) = self.resolver_pasta(&componentes(caminho))?;
        let pasta = self.abrir_pasta(cluster)?;
        let itens = entradas(&pasta)
            .into_iter()
            .map(|e| ItemPasta { pasta: e.e_pasta(), tamanho: if e.e_pasta() { 0 } else { e.tamanho as u64 }, nome: e.nome })
            .collect();
        Ok(itens)
    }

    /// Conteudo inteiro de um arquivo.
    pub fn ler(&mut self, caminho: &str) -> Result<Vec<u8>, Erro> {
        let (pasta, nome, _) = self.pai_e_nome(caminho)?;
        let e = achar(&pasta, nome).ok_or(Erro::NaoAchado)?;
        if e.e_pasta() {
            return Err(Erro::EPasta);
        }
        let tamanho = e.tamanho as usize;
        let cadeia = self.cadeia(e.cluster)?;
        if cadeia.len() < self.geo.clusters_para(tamanho) {
            return Err(Erro::Corrompido("arquivo maior que a cadeia"));
        }
        let cadeia = &cadeia[..self.geo.clusters_para(tamanho)];
        let mut dados = vec![0u8; cadeia.len() * self.geo.bytes_por_cluster];
        self.ler_clusters(cadeia, &mut dados)?;
        dados.truncate(tamanho);
        Ok(dados)
    }

    /// Tamanho e cadeia de clusters de um arquivo, para ler aos pedacos
    /// (`ler_trecho`) sem trazer o arquivo inteiro.
    pub fn abrir_leitura(&mut self, caminho: &str) -> Result<(usize, Vec<u32>), Erro> {
        let (pasta, nome, _) = self.pai_e_nome(caminho)?;
        let e = achar(&pasta, nome).ok_or(Erro::NaoAchado)?;
        if e.e_pasta() {
            return Err(Erro::EPasta);
        }
        let tamanho = e.tamanho as usize;
        let mut cadeia = self.cadeia(e.cluster)?;
        let precisa = self.geo.clusters_para(tamanho);
        if cadeia.len() < precisa {
            return Err(Erro::Corrompido("arquivo maior que a cadeia"));
        }
        cadeia.truncate(precisa);
        Ok((tamanho, cadeia))
    }

    /// Le `destino.len()` bytes do arquivo de cadeia `cadeia`, a partir do
    /// byte `inicio`. O que passa do fim da cadeia fica como estava.
    pub fn ler_trecho(&mut self, cadeia: &[u32], inicio: usize, destino: &mut [u8]) -> Result<(), Erro> {
        let bpc = self.geo.bytes_por_cluster;
        let primeiro = inicio / bpc;
        let ultimo = (inicio + destino.len()).div_ceil(bpc).min(cadeia.len());
        if primeiro >= ultimo {
            return Ok(());
        }
        let mut lido = vec![0u8; (ultimo - primeiro) * bpc];
        self.ler_clusters(&cadeia[primeiro..ultimo], &mut lido)?;
        let de = inicio - primeiro * bpc;
        let n = destino.len().min(lido.len() - de);
        destino[..n].copy_from_slice(&lido[de..de + n]);
        Ok(())
    }

    /// Cria ou substitui um arquivo.
    pub fn escrever(&mut self, caminho: &str, dados: &[u8]) -> Result<(), Erro> {
        if dados.len() > u32::MAX as usize {
            return Err(Erro::Cheio);
        }
        self.operacao(|fs| {
            let (mut pasta, nome, _) = fs.pai_e_nome(caminho)?;
            let quantos = fs.geo.clusters_para(dados.len());
            let quando = (fs.relogio)();
            match achar(&pasta, nome) {
                Some(e) if e.e_pasta() => Err(Erro::EPasta),
                Some(e) => {
                    let mut cadeia = fs.cadeia(e.cluster)?;
                    fs.ajustar_cadeia(&mut cadeia, quantos)?;
                    fs.gravar_clusters(&cadeia, dados)?;
                    let entrada = &mut pasta.dados[e.posicao..e.posicao + ENTRADA];
                    definir_cluster(entrada, cadeia.first().copied().unwrap_or(0));
                    gravar32(entrada, 28, dados.len() as u32);
                    entrada[11] |= ATTR_ARQUIVO;
                    gravar16(entrada, 18, quando.0);
                    gravar16(entrada, 22, quando.1);
                    gravar16(entrada, 24, quando.0);
                    pasta.marcar(e.posicao, e.posicao + ENTRADA);
                    fs.gravar_pasta(&mut pasta)
                }
                None => {
                    validar_nome(nome)?;
                    // espaco na pasta antes: assim um Cheio nao deixa clusters perdidos
                    let (_, _, lfn) = fs.nomes_para(&pasta, nome)?;
                    fs.reservar_vagas(&mut pasta, lfn.len() + 1)?;
                    let cadeia = fs.alocar(quantos)?;
                    fs.gravar_clusters(&cadeia, dados)?;
                    let primeiro = cadeia.first().copied().unwrap_or(0);
                    let modelo = entrada_curta(&[b' '; 11], 0, ATTR_ARQUIVO, primeiro, dados.len() as u32, quando);
                    fs.criar_entradas(&mut pasta, nome, &modelo)?;
                    fs.gravar_pasta(&mut pasta)
                }
            }
        })
    }

    pub fn criar_pasta(&mut self, caminho: &str) -> Result<(), Erro> {
        self.operacao(|fs| {
            let (mut pasta, nome, _) = fs.pai_e_nome(caminho)?;
            if achar(&pasta, nome).is_some() {
                return Err(Erro::JaExiste);
            }
            validar_nome(nome)?;
            let (_, _, lfn) = fs.nomes_para(&pasta, nome)?;
            fs.reservar_vagas(&mut pasta, lfn.len() + 1)?;
            let quando = (fs.relogio)();
            let novo = fs.alocar(1)?;
            // conteudo da pasta nova: "." e ".." (0 quando o pai e a raiz)
            let pai = if pasta.cluster == fs.geo.raiz { 0 } else { pasta.cluster };
            let mut conteudo = vec![0u8; fs.geo.bytes_por_cluster];
            conteudo[..ENTRADA].copy_from_slice(&entrada_curta(&NOME_PONTO, 0, ATTR_PASTA, novo[0], 0, quando));
            conteudo[ENTRADA..2 * ENTRADA].copy_from_slice(&entrada_curta(&NOME_PONTO_PONTO, 0, ATTR_PASTA, pai, 0, quando));
            fs.gravar_clusters(&novo, &conteudo)?;
            let modelo = entrada_curta(&[b' '; 11], 0, ATTR_PASTA, novo[0], 0, quando);
            fs.criar_entradas(&mut pasta, nome, &modelo)?;
            fs.gravar_pasta(&mut pasta)
        })
    }

    /// Apaga um arquivo ou uma pasta vazia.
    pub fn apagar(&mut self, caminho: &str) -> Result<(), Erro> {
        self.operacao(|fs| {
            let (mut pasta, nome, _) = fs.pai_e_nome(caminho)?;
            let e = achar(&pasta, nome).ok_or(Erro::NaoAchado)?;
            if e.e_pasta() && e.cluster != 0 {
                let dentro = fs.abrir_pasta(e.cluster)?;
                if !entradas(&dentro).is_empty() {
                    return Err(Erro::PastaNaoVazia);
                }
            }
            let cadeia = fs.cadeia(e.cluster)?;
            Fat32::apagar_entradas(&mut pasta, &e);
            fs.gravar_pasta(&mut pasta)?;
            fs.liberar(&cadeia)
        })
    }

    /// Renomeia ou move (`para` e o caminho novo completo, que nao pode existir).
    pub fn renomear(&mut self, de: &str, para: &str) -> Result<(), Erro> {
        self.operacao(|fs| {
            let (mut origem, nome, _) = fs.pai_e_nome(de)?;
            let (destino, novo_nome, ancestrais) = fs.pai_e_nome(para)?;
            let e = achar(&origem, nome).ok_or(Erro::NaoAchado)?;
            validar_nome(novo_nome)?;
            // uma pasta nao pode ir para dentro dela mesma
            if e.e_pasta() && ancestrais.contains(&e.cluster) {
                return Err(Erro::NomeInvalido);
            }
            let mut modelo = [0u8; ENTRADA];
            modelo.copy_from_slice(origem.entrada(e.posicao));
            if destino.cluster == origem.cluster {
                // mesma pasta: so pode "existir" ela mesma (mudar a caixa)
                if achar(&origem, novo_nome).is_some_and(|outro| outro.posicao != e.posicao) {
                    return Err(Erro::JaExiste);
                }
                Fat32::apagar_entradas(&mut origem, &e);
                fs.criar_entradas(&mut origem, novo_nome, &modelo)?;
                return fs.gravar_pasta(&mut origem);
            }
            let mut destino = destino;
            if achar(&destino, novo_nome).is_some() {
                return Err(Erro::JaExiste);
            }
            fs.criar_entradas(&mut destino, novo_nome, &modelo)?;
            fs.gravar_pasta(&mut destino)?;
            Fat32::apagar_entradas(&mut origem, &e);
            fs.gravar_pasta(&mut origem)?;
            if e.e_pasta() && e.cluster != 0 {
                fs.trocar_pai(e.cluster, destino.cluster)?;
            }
            Ok(())
        })
    }

    /// Aponta o ".." da pasta `cluster` para o novo pai.
    fn trocar_pai(&mut self, cluster: u32, pai: u32) -> Result<(), Erro> {
        let mut pasta = self.abrir_pasta(cluster)?;
        let Some(posicao) = pasta.posicao_curta(&NOME_PONTO_PONTO) else { return Ok(()) };
        let pai = if pai == self.geo.raiz { 0 } else { pai };
        definir_cluster(&mut pasta.dados[posicao..posicao + ENTRADA], pai);
        pasta.marcar(posicao, posicao + ENTRADA);
        self.gravar_pasta(&mut pasta)
    }
}

fn fsinfo_valido(s: &[u8]) -> bool {
    le32(s, 0) == 0x4161_5252 && le32(s, 484) == 0x6141_7272 && le32(s, 508) == 0xAA55_0000
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn checksum_conhecido() {
        // a formula da spec: ((soma & 1) << 7) + (soma >> 1) + byte
        let mut soma = 0u8;
        for &b in b"README  TXT" {
            soma = ((soma & 1) << 7).wrapping_add(soma >> 1).wrapping_add(b);
        }
        assert_eq!(checksum(b"README  TXT"), soma);
    }

    #[test]
    fn nomes_curtos() {
        assert_eq!(so_curto("LEIAME.TXT"), Some((*b"LEIAME  TXT", 0)));
        assert_eq!(so_curto("config"), Some((*b"CONFIG     ", CAIXA_BASE)));
        assert_eq!(so_curto("notas.txt"), Some((*b"NOTAS   TXT", CAIXA_BASE | CAIXA_EXT)));
        assert_eq!(so_curto("Notas.txt"), None);
        assert_eq!(so_curto("index.html"), None);
        assert_eq!(so_curto("documentos"), None);
        assert_eq!(gerar_curto("index.html", &[]).unwrap(), *b"INDEX~1 HTM");
        assert_eq!(gerar_curto("Lista de compras.txt", &[]).unwrap(), *b"LISTAD~1TXT");
        assert_eq!(gerar_curto(".oculto", &[]).unwrap(), *b"OCULTO~1   ");
        let usados = [*b"INDEX~1 HTM"];
        assert_eq!(gerar_curto("index.html", &usados).unwrap(), *b"INDEX~2 HTM");
        assert_eq!(texto_curto(b"NOTAS   TXT", CAIXA_BASE | CAIXA_EXT), "notas.txt");
        assert_eq!(so_curto("Ação.txt"), None);
        assert_eq!(gerar_curto("Anotações.txt", &[]).unwrap(), *b"ANOTAC~1TXT");
    }

    #[test]
    fn lfn_com_acentos() {
        let nome = "Anotações de março – é só começar.txt";
        let curto = gerar_curto(nome, &[]).unwrap();
        let mut lfn = Lfn::novo();
        for (i, e) in entradas_lfn(nome, &curto).iter().enumerate() {
            lfn.acrescentar(e, i * ENTRADA);
        }
        assert_eq!(lfn.terminar(&curto), Some((String::from(nome), 0)));
        assert!(mesmo_nome("ANOTAÇÕES.TXT", "anotações.txt"));
        assert!(validar_nome("Área de trabalho").is_ok());
        assert!(validar_nome("emoji 😀").is_err());
    }

    #[test]
    fn lfn_ida_e_volta() {
        let nome = "Um nome bem comprido de arquivo.html";
        let curto = gerar_curto(nome, &[]).unwrap();
        let es = entradas_lfn(nome, &curto);
        assert_eq!(es.len(), 3);
        let mut lfn = Lfn::novo();
        for (i, e) in es.iter().enumerate() {
            lfn.acrescentar(e, i * ENTRADA);
        }
        assert_eq!(lfn.terminar(&curto), Some((String::from(nome), 0)));
    }

    #[test]
    fn caminhos() {
        assert_eq!(componentes("/a/./b/../c/"), ["a", "c"]);
        assert!(componentes("/").is_empty());
        assert!(validar_nome("ok.txt").is_ok());
        assert!(validar_nome("ruim?").is_err());
        assert!(validar_nome("fim.").is_err());
    }
}
