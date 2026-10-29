//! Diagramacao: transforma os blocos do Documento em linhas.
//!
//! A quebra de linha mede cada palavra com uma Medida: no modo texto
//! (serial) um caractere vale 1; na tela vale a largura de verdade do
//! glifo na fonte do trecho (Inter no texto, Inter negrito em <b>/<th>,
//! Inter grande nos titulos, JetBrains Mono em <pre> e <code>). A mesma
//! rotina serve para os dois.
//!
//! A Pagina e uma lista plana de itens ja posicionados (texto, linhas,
//! bolinhas, fundos) em coordenadas da pagina; o render so recorta e
//! desenha os que aparecem. As faixas (uma por bloco) dizem onde estao os
//! itens de cada altura: o render pula direto para as que aparecem.
//!
//! Cada linha da tela tem a altura das fontes que aparecem nela (a maior
//! subida mais a maior descida), com o texto todo na mesma linha de base.
//!
//! Tabelas: as linhas seguidas da mesma <table> viram uma grade. Cada
//! coluna fica com a largura natural do texto se couber; se nao, com a
//! maior palavra e mais uma parte do que sobra, e o texto quebra dentro
//! da celula. O modo texto usa a mesma conta, em caracteres.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use super::css::{self, BAIXO, CIMA, DIREITA, ESQUERDA};
use super::html::{
    Bloco, Caixa, Documento, Marcador, Tinta, TipoBloco, Trecho, CODIGO, MAX_CELULAS, MONO, NEGRITO, SEM_SUBLINHADO,
    SUBLINHADO, TAMANHO, TAMANHO_GRANDE, TAMANHO_TITULO,
};
use crate::fonte::{self, Fonte};
use crate::gui::grafico::Retangulo;

/// Largura de um texto com um estilo: caracteres ou pixels.
pub type Medida<'a> = &'a dyn Fn(&str, u8) -> i32;

/// Modo texto: cada caractere ocupa uma coluna.
fn caracteres(texto: &str, _estilo: u8) -> i32 {
    texto.chars().count() as i32
}

fn medir_caractere(medir: Medida, c: char, estilo: u8) -> i32 {
    let mut buffer = [0u8; 4];
    medir(c.encode_utf8(&mut buffer), estilo)
}

/// Pedaco de uma linha diagramada: texto com o mesmo estilo e link.
/// `x` e `largura` estao na unidade da Medida (colunas ou px).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pedaco {
    pub x: i32,
    pub largura: i32,
    pub texto: String,
    pub estilo: u8,
    pub link: Option<usize>,
    pub tinta: Tinta,
}

pub type Linha = Vec<Pedaco>;

/// Uma palavra pode ter varios estilos ("<b>neg</b>rito"): partes coladas.
struct Parte<'a> {
    texto: &'a str,
    jeito: Jeito,
}

/// Estilo, link e cores de um pedaco: pedacos vizinhos com o mesmo jeito
/// viram um so.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Jeito {
    estilo: u8,
    link: Option<usize>,
    tinta: Tinta,
}

impl Jeito {
    fn de(t: &Trecho) -> Jeito {
        Jeito { estilo: t.estilo, link: t.link, tinta: t.tinta }
    }
}

/// Monta as linhas de um bloco que cabem em `largura`.
struct Quebrador<'m> {
    largura: i32,
    medir: Medida<'m>,
    linhas: Vec<Linha>,
    atual: Linha,
    x: i32,
}

impl<'m> Quebrador<'m> {
    fn novo(largura: i32, medir: Medida<'m>) -> Quebrador<'m> {
        Quebrador { largura: largura.max(1), medir, linhas: Vec::new(), atual: Vec::new(), x: 0 }
    }

    fn nova_linha(&mut self) {
        self.linhas.push(core::mem::take(&mut self.atual));
        self.x = 0;
    }

    /// Escreve na linha atual, juntando com o pedaco anterior se puder.
    /// Nao quebra: quem chama garante que cabe.
    fn por(&mut self, texto: &str, j: Jeito) {
        if texto.is_empty() {
            return;
        }
        let x = self.x;
        let largura = (self.medir)(texto, j.estilo);
        self.x += largura;
        if let Some(p) = self.atual.last_mut()
            && (Jeito { estilo: p.estilo, link: p.link, tinta: p.tinta }) == j
            && p.x + p.largura == x
        {
            p.texto.push_str(texto);
            p.largura += largura;
            return;
        }
        self.atual.push(Pedaco { x, largura, texto: texto.into(), estilo: j.estilo, link: j.link, tinta: j.tinta });
    }

    /// Quantos bytes do comeco de `texto` cabem no resto da linha. No
    /// comeco da linha cabe pelo menos um caractere (senao nao anda).
    fn quanto_cabe(&self, texto: &str, estilo: u8) -> usize {
        let mut usado = self.x;
        for (i, c) in texto.char_indices() {
            usado += medir_caractere(self.medir, c, estilo);
            if usado > self.largura {
                return if i == 0 && self.x == 0 { c.len_utf8() } else { i };
            }
        }
        texto.len()
    }

    /// Escreve quebrando a forca onde a linha acaba (palavra gigante, pre).
    fn por_cortando(&mut self, mut texto: &str, j: Jeito) {
        while !texto.is_empty() {
            let corte = self.quanto_cabe(texto, j.estilo);
            if corte == 0 {
                self.nova_linha();
                continue;
            }
            self.por(&texto[..corte], j);
            texto = &texto[corte..];
        }
    }

    /// Uma palavra inteira, com o espaco que vinha antes dela (se houver).
    fn palavra(&mut self, partes: &[Parte], espaco: Option<Jeito>) {
        let tamanho: i32 = partes.iter().map(|p| (self.medir)(p.texto, p.jeito.estilo)).sum();
        let com_espaco = espaco.filter(|_| self.x > 0);
        let largura_espaco = com_espaco.map_or(0, |j| (self.medir)(" ", j.estilo));
        if self.x > 0 && self.x + largura_espaco + tamanho > self.largura {
            self.nova_linha();
        } else if let Some(j) = com_espaco {
            self.por(" ", j);
        }
        for p in partes {
            self.por_cortando(p.texto, p.jeito);
        }
    }

    fn terminar(mut self) -> Vec<Linha> {
        if !self.atual.is_empty() || self.linhas.is_empty() {
            self.nova_linha();
        }
        self.linhas
    }
}

/// Quebra os trechos de um bloco normal em linhas de `largura`.
pub fn quebrar(trechos: &[Trecho], largura: i32, medir: Medida) -> Vec<Linha> {
    let mut q = Quebrador::novo(largura, medir);
    let mut partes: Vec<Parte> = Vec::new();
    let mut espaco: Option<Jeito> = None;
    for t in trechos {
        let mut resto = t.texto.as_str();
        while !resto.is_empty() {
            let c = resto.as_bytes()[0];
            if c == b' ' || c == b'\n' {
                if !partes.is_empty() {
                    q.palavra(&partes, espaco.take());
                    partes.clear();
                }
                if c == b'\n' {
                    q.nova_linha();
                    espaco = None;
                } else if q.x > 0 || espaco.is_some() {
                    espaco = Some(Jeito::de(t));
                }
                resto = &resto[1..];
                continue;
            }
            let fim = resto.find([' ', '\n']).unwrap_or(resto.len());
            partes.push(Parte { texto: &resto[..fim], jeito: Jeito::de(t) });
            resto = &resto[fim..];
        }
    }
    if !partes.is_empty() {
        q.palavra(&partes, espaco.take());
    }
    q.terminar()
}

/// Texto pre-formatado: quebra nos '\n' (e onde a linha acaba).
pub fn quebrar_pre(trechos: &[Trecho], largura: i32, medir: Medida) -> Vec<Linha> {
    let mut q = Quebrador::novo(largura, medir);
    for t in trechos {
        for (i, pedaco) in t.texto.split('\n').enumerate() {
            if i > 0 {
                q.nova_linha();
            }
            q.por_cortando(pedaco, Jeito::de(t));
        }
    }
    q.terminar()
}

fn linhas_do_bloco(tipo: TipoBloco, trechos: &[Trecho], largura: i32, medir: Medida) -> Vec<Linha> {
    if tipo == TipoBloco::Pre { quebrar_pre(trechos, largura, medir) } else { quebrar(trechos, largura, medir) }
}

/// Simbolo do marcador no modo texto.
fn texto_marcador(m: Marcador) -> Option<String> {
    match m {
        Marcador::Nenhum => None,
        Marcador::Bolinha(1) => Some("•".into()),
        Marcador::Bolinha(2) => Some("-".into()),
        Marcador::Bolinha(_) => Some("·".into()),
        Marcador::Numero(n) => Some(format!("{n}.")),
    }
}

// ---------------------------------------------------------------------
// Modo texto

/// Colunas de recuo por nivel (lista ou citacao) no modo texto.
const RECUO_TEXTO: usize = 4;

/// O documento como texto de `colunas` colunas, com os links numerados
/// ("palavra[3]"). Devolve o texto (linhas terminadas em '\n') e o href
/// de cada link: o [n] do texto e o elemento n-1 da lista.
pub fn texto(doc: &Documento, colunas: usize) -> (String, Vec<String>) {
    let colunas = colunas.max(20);
    let mut saida = String::new();
    let mut i = 0;
    while i < doc.blocos.len() {
        let bloco = &doc.blocos[i];
        // todo bloco escreve linhas, entao so o comeco fica sem margem
        if bloco.margem && !saida.is_empty() {
            saida.push('\n');
        }
        let recuo = (bloco.recuo as usize * RECUO_TEXTO).min(colunas / 2);
        let largura = colunas - recuo;
        if bloco.tipo == TipoBloco::Linha {
            let fim = fim_da_tabela(&doc.blocos, i);
            tabela_texto(&mut saida, &doc.blocos[i..fim], recuo, largura);
            i = fim;
            continue;
        }
        bloco_texto(&mut saida, bloco, recuo, largura);
        i += 1;
    }
    (saida, doc.links.clone())
}

/// Um bloco (que nao e tabela) no modo texto.
fn bloco_texto(saida: &mut String, bloco: &Bloco, recuo: usize, largura: usize) {
    if bloco.tipo == TipoBloco::Regua {
        empurrar_linha(saida, recuo, &"-".repeat(largura));
        return;
    }
    let trechos = com_numeros_de_link(&bloco.trechos);
    let linhas = linhas_do_bloco(bloco.tipo, &trechos, largura as i32, &caracteres);
    let mut maior = 0;
    for (i, linha) in linhas.iter().enumerate() {
        let texto = juntar_linha(linha);
        maior = maior.max(texto.trim_end().chars().count());
        let prefixo = match (i, texto_marcador(bloco.marcador)) {
            (0, Some(m)) if recuo > 0 => format!("{m:>w$} ", w = recuo - 1),
            (0, Some(m)) => format!("{m} "),
            _ => " ".repeat(recuo),
        };
        saida.push_str(&prefixo);
        saida.push_str(texto.trim_end());
        saida.push('\n');
    }
    // titulos principais ganham um sublinhado de '=' ou '-'
    if let TipoBloco::Titulo(nivel @ (1 | 2)) = bloco.tipo {
        let traco = if nivel == 1 { "=" } else { "-" };
        empurrar_linha(saida, recuo, &traco.repeat(maior.min(largura)));
    }
}

/// Linhas de uma tabela no modo texto: colunas alinhadas, separadas por " | ".
fn tabela_texto(saida: &mut String, linhas: &[Bloco], recuo: usize, largura: usize) {
    let copias: Vec<Vec<Trecho>> = linhas.iter().map(|b| com_numeros_de_link(&b.trechos)).collect();
    let celulas: Vec<Vec<(&[Trecho], bool)>> =
        linhas.iter().zip(&copias).map(|(bloco, trechos)| celulas_da_linha(bloco, trechos)).collect();
    let n = numero_de_colunas(&celulas);
    let (natural, minima) = medir_colunas(&celulas, n, &caracteres);
    let disponivel = (largura as i32 - 3 * (n as i32 - 1)).max(n as i32);
    let larguras = repartir(&natural, &minima, disponivel);
    let (quebradas, larguras) = quebrar_tabela(&celulas, &larguras, &caracteres);
    for linha in &quebradas {
        let altura = linha.iter().map(Vec::len).max().unwrap_or(0).max(1);
        for k in 0..altura {
            let mut texto = " ".repeat(recuo);
            for (c, &w) in larguras.iter().enumerate() {
                if c > 0 {
                    texto.push_str(" | ");
                }
                let celula = linha[c].get(k).map(juntar_linha).unwrap_or_default();
                texto.push_str(&celula);
                for _ in celula.chars().count() as i32..w {
                    texto.push(' ');
                }
            }
            saida.push_str(texto.trim_end());
            saida.push('\n');
        }
    }
}

// ---------------------------------------------------------------------
// Tabelas (comum ao modo texto e aos pixels)

/// Fim (exclusivo) das linhas seguidas da mesma tabela que comeca em `i`.
fn fim_da_tabela(blocos: &[Bloco], i: usize) -> usize {
    let primeira = &blocos[i];
    let mesma = |b: &Bloco| {
        b.tipo == TipoBloco::Linha
            && b.tabela == primeira.tabela
            && b.recuo == primeira.recuo
            && !b.margem
            && b.caixas == primeira.caixas
    };
    i + 1 + blocos[i + 1..].iter().take_while(|b| mesma(b)).count()
}

/// Os trechos de cada celula de uma linha (`trechos` e o do bloco ou uma
/// copia com o mesmo tamanho) e se e de cabecalho. Texto antes da
/// primeira celula fica na primeira.
fn celulas_da_linha<'a>(bloco: &Bloco, trechos: &'a [Trecho]) -> Vec<(&'a [Trecho], bool)> {
    if bloco.celulas.is_empty() {
        return vec![(trechos, false)];
    }
    let n = bloco.celulas.len().min(MAX_CELULAS);
    let inicio = |c: usize| if c == 0 { 0 } else { bloco.celulas[c].inicio.min(trechos.len()) };
    (0..n)
        .map(|c| {
            let fim = if c + 1 < n { inicio(c + 1) } else { trechos.len() };
            (&trechos[inicio(c)..fim.max(inicio(c))], bloco.celulas[c].cabecalho)
        })
        .collect()
}

fn numero_de_colunas(celulas: &[Vec<(&[Trecho], bool)>]) -> usize {
    celulas.iter().map(Vec::len).max().unwrap_or(1).max(1)
}

/// Largura natural (o texto todo numa linha so) e minima (a maior
/// palavra) de um texto.
fn medir(trechos: &[Trecho], medir: Medida) -> (i32, i32) {
    let (mut natural, mut linha, mut maior, mut palavra) = (0, 0, 0, 0);
    for t in trechos {
        for c in t.texto.chars() {
            match c {
                '\n' => {
                    linha = 0;
                    palavra = 0;
                }
                ' ' => {
                    linha += medir_caractere(medir, c, t.estilo);
                    palavra = 0;
                }
                _ => {
                    let w = medir_caractere(medir, c, t.estilo);
                    linha += w;
                    palavra += w;
                    maior = maior.max(palavra);
                }
            }
            natural = natural.max(linha);
        }
    }
    (natural, maior)
}

/// (natural, minima) de cada uma das `n` colunas: o maximo das celulas.
fn medir_colunas(celulas: &[Vec<(&[Trecho], bool)>], n: usize, medida: Medida) -> (Vec<i32>, Vec<i32>) {
    let mut natural = vec![0; n];
    let mut minima = vec![0; n];
    for linha in celulas {
        for (c, (trechos, _)) in linha.iter().enumerate() {
            let (nat, min) = medir(trechos, medida);
            natural[c] = natural[c].max(nat);
            minima[c] = minima[c].max(min);
        }
    }
    (natural, minima)
}

/// Largura de cada coluna para caber em `disponivel`: a natural se
/// couber; senao cada uma ganha a maior palavra e o resto e dividido
/// conforme o que falta para a natural.
fn repartir(natural: &[i32], minima: &[i32], disponivel: i32) -> Vec<i32> {
    let soma_natural: i64 = natural.iter().map(|&n| n as i64).sum();
    let disponivel = disponivel as i64;
    if soma_natural <= disponivel {
        return natural.iter().map(|&n| n.max(1)).collect();
    }
    let soma_minima: i64 = minima.iter().map(|&m| m as i64).sum();
    if soma_minima >= disponivel {
        // nem as palavras inteiras cabem: elas vao ser cortadas
        let divisor = soma_minima.max(1);
        return minima.iter().map(|&m| ((m as i64 * disponivel / divisor) as i32).max(1)).collect();
    }
    let extra = disponivel - soma_minima;
    let falta = soma_natural - soma_minima;
    natural
        .iter()
        .zip(minima)
        .map(|(&n, &m)| ((m as i64 + extra * (n - m) as i64 / falta) as i32).max(1))
        .collect()
}

/// Quanto a linha ocupa de verdade.
fn largura_da_linha(linha: &Linha) -> i32 {
    linha.last().map_or(0, |p| p.x + p.largura)
}

/// Quebra cada celula na largura da sua coluna. Devolve as linhas de cada
/// celula (por linha da tabela, por coluna) e a largura que cada coluna
/// usou de fato: com ela a coluna encolhe sem mudar nenhuma quebra.
fn quebrar_tabela(
    celulas: &[Vec<(&[Trecho], bool)>],
    larguras: &[i32],
    medida: Medida,
) -> (Vec<Vec<Vec<Linha>>>, Vec<i32>) {
    let mut usadas = vec![1; larguras.len()];
    let mut quebradas = Vec::with_capacity(celulas.len());
    for linha in celulas {
        let mut colunas = Vec::with_capacity(larguras.len());
        for (c, &w) in larguras.iter().enumerate() {
            let linhas = linha.get(c).map_or_else(Vec::new, |(trechos, _)| quebrar(trechos, w, medida));
            for l in &linhas {
                usadas[c] = usadas[c].max(largura_da_linha(l));
            }
            colunas.push(linhas);
        }
        quebradas.push(colunas);
    }
    (quebradas, usadas)
}

fn empurrar_linha(saida: &mut String, recuo: usize, texto: &str) {
    for _ in 0..recuo {
        saida.push(' ');
    }
    saida.push_str(texto);
    saida.push('\n');
}

/// Junta os pedacos de uma linha do modo texto respeitando as colunas.
fn juntar_linha(linha: &Linha) -> String {
    let mut s = String::new();
    let mut coluna = 0;
    for p in linha {
        while coluna < p.x {
            s.push(' ');
            coluna += 1;
        }
        s.push_str(&p.texto);
        coluna += p.largura;
    }
    s
}

/// Copia os trechos colocando "[n]" no fim de cada link (antes dos
/// espacos do fim, para ficar "palavra[1] seguinte").
fn com_numeros_de_link(trechos: &[Trecho]) -> Vec<Trecho> {
    let mut saida: Vec<Trecho> = Vec::with_capacity(trechos.len());
    for (i, t) in trechos.iter().enumerate() {
        let mut copia = t.clone();
        let fim_do_link = t.link.is_some() && trechos.get(i + 1).is_none_or(|p| p.link != t.link);
        if let (true, Some(n)) = (fim_do_link, t.link) {
            let sem_espaco = copia.texto.trim_end_matches([' ', '\n']).len();
            let resto: String = copia.texto[sem_espaco..].into();
            copia.texto.truncate(sem_espaco);
            copia.texto.push_str(&format!("[{}]", n + 1));
            copia.texto.push_str(&resto);
        }
        saida.push(copia);
    }
    saida
}

// ---------------------------------------------------------------------
// Diagramacao em pixels

/// Margem em volta da pagina.
pub const MARGEM: i32 = 16;
/// Recuo por nivel de lista ou citacao.
const RECUO_PX: i32 = 28;
/// Espaco antes de paragrafos (os titulos tem mais, veja `espaco_antes`).
const ESPACO_BLOCO: i32 = 10;
/// Folga do texto pre-formatado ate a borda do fundo.
const FOLGA_PRE_X: i32 = 8;
const FOLGA_PRE_Y: i32 = 6;
/// Grade das tabelas: linha de 1 px e folga do texto ate ela.
const BORDA_TABELA: i32 = 1;
const FOLGA_CELULA: i32 = 7;
const FOLGA_VERTICAL: i32 = 4;
/// Folga em volta do <code> no meio do texto (o fundo cinza).
const FOLGA_CODIGO: i32 = 2;

/// As fontes da pagina.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Letra {
    /// Inter 13 px: o texto normal.
    Normal,
    /// Inter 13 px seminegrito: <b>, <strong>, <th>, h4 a h6.
    Negrito,
    /// Inter 20 px seminegrito: h2 e h3.
    Titulo,
    /// Inter 32 px negrito: h1.
    Grande,
    /// JetBrains Mono: <pre>, <code>, <kbd>...
    Mono,
}

impl Letra {
    pub fn fonte(self) -> &'static Fonte {
        match self {
            Letra::Normal => &fonte::INTER,
            Letra::Negrito => &fonte::INTER_NEGRITO,
            Letra::Titulo => &fonte::INTER_TITULO,
            Letra::Grande => &fonte::INTER_GRANDE,
            Letra::Mono => &fonte::MONO,
        }
    }

    /// (subida, descida) da linha: da linha de base ate o alto e ate o pe.
    fn metricas(self) -> (i32, i32) {
        let f = self.fonte();
        (f.ascendente, f.altura_linha - f.ascendente)
    }
}

/// A fonte de um trecho com `estilo` num bloco do `tipo`.
/// O tamanho pedido pelo CSS (bits TAMANHO) ganha do tipo do bloco.
pub fn letra(tipo: TipoBloco, estilo: u8) -> Letra {
    if tipo == TipoBloco::Pre || estilo & (CODIGO | MONO) != 0 {
        return Letra::Mono;
    }
    match (estilo & TAMANHO, tipo) {
        (TAMANHO_GRANDE, _) | (0, TipoBloco::Titulo(1)) => Letra::Grande,
        (TAMANHO_TITULO, _) | (0, TipoBloco::Titulo(2 | 3)) => Letra::Titulo,
        (0, TipoBloco::Titulo(_)) => Letra::Negrito,
        _ if estilo & NEGRITO != 0 => Letra::Negrito,
        _ => Letra::Normal,
    }
}

/// Coisas desenhaveis, em coordenadas da pagina.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// Texto com a caneta em `x` e a linha de base em `base`.
    Texto { x: i32, base: i32, texto: String, letra: Letra, estilo: u8, link: Option<usize>, tinta: Tinta },
    /// Caixa do CSS: fundo e bordas (cima, direita, baixo, esquerda).
    Caixa { r: Retangulo, fundo: Option<u32>, bordas: [i32; 4], cores: [Option<u32>; 4] },
    /// Linha horizontal de 1 px (hr, sublinhado de titulo).
    Regua { x: i32, y: i32, largura: i32 },
    /// Bolinha de lista. `cheia` = nivel 1.
    Bolinha { cx: i32, cy: i32, raio: i32, cheia: bool },
    /// Fundo do texto pre-formatado (e do <code> no meio do texto).
    FundoPre(Retangulo),
    /// Barra vertical de citacao.
    Barra(Retangulo),
    /// Fundo de uma celula de cabecalho (<th>).
    FundoCelula(Retangulo),
    /// Linha da grade de uma tabela.
    Grade(Retangulo),
}

/// Folga em volta do texto na faixa: acentos de maiuscula e o 'Å' passam
/// um pouco da subida da fonte.
const FOLGA_FAIXA: i32 = 3;

impl Item {
    /// (topo, base) do item, para saber se aparece na janela.
    pub fn faixa(&self) -> (i32, i32) {
        match self {
            Item::Texto { base, letra, .. } => {
                let (subida, descida) = letra.metricas();
                (base - subida - FOLGA_FAIXA, base + descida + FOLGA_FAIXA)
            }
            Item::Regua { y, .. } => (*y, y + 1),
            Item::Bolinha { cy, raio, .. } => (cy - raio, cy + raio + 1),
            Item::FundoPre(r) | Item::Barra(r) | Item::FundoCelula(r) | Item::Grade(r) | Item::Caixa { r, .. } => {
                (r.y, r.y + r.altura)
            }
        }
    }
}

/// Os itens de um bloco: `itens[inicio..]` ate o `inicio` da proxima.
/// `topo` e o menor topo desta faixa e de todas as de baixo, e `base` a
/// maior base desta e de todas as de cima: assim as duas sobem em ordem e
/// o render acha as faixas visiveis por busca binaria.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Faixa {
    pub topo: i32,
    pub base: i32,
    pub inicio: usize,
}

pub struct Pagina {
    /// Altura total em px (para a barra de rolagem).
    pub altura: i32,
    pub largura: i32,
    pub titulo: String,
    pub itens: Vec<Item>,
    /// Uma por bloco (ou tabela), de cima para baixo.
    pub faixas: Vec<Faixa>,
    /// Retangulo de cada pedaco de link -> indice em `links`.
    pub caixas_de_link: Vec<(Retangulo, usize)>,
    /// href de cada link (copiado do Documento, sem resolver).
    pub links: Vec<String>,
    /// Fundo da pagina pedido pelo CSS (None = branco).
    pub fundo: Option<u32>,
    /// Cor do texto do <body> pedida pelo CSS.
    pub cor_texto: Option<u32>,
}

impl Pagina {
    /// A faixa dos itens[inicio..fim] (se o bloco pos algum).
    fn marcar_faixa(&mut self, inicio: usize, fim: usize) {
        let Some(primeiro) = self.itens.get(inicio).filter(|_| inicio < fim) else { return };
        let (mut topo, mut base) = primeiro.faixa();
        for item in &self.itens[inicio + 1..fim] {
            let (t, b) = item.faixa();
            topo = topo.min(t);
            base = base.max(b);
        }
        if let Some(anterior) = self.faixas.last() {
            base = base.max(anterior.base);
        }
        self.faixas.push(Faixa { topo, base, inicio });
    }

    /// Depois de tudo: o topo de cada faixa vira o menor dali para baixo.
    fn acertar_topos(&mut self) {
        let mut menor = i32::MAX;
        for faixa in self.faixas.iter_mut().rev() {
            menor = menor.min(faixa.topo);
            faixa.topo = menor;
        }
    }
}

/// Espaco antes de um bloco com margem: titulos maiores pedem mais.
fn espaco_antes(tipo: TipoBloco) -> i32 {
    match tipo {
        TipoBloco::Titulo(1) => 18,
        TipoBloco::Titulo(2 | 3) => 16,
        TipoBloco::Titulo(_) => 12,
        _ => ESPACO_BLOCO,
    }
}

/// Faixa horizontal onde vai o conteudo: a pagina ou o miolo de uma caixa.
#[derive(Clone, Copy, Debug)]
struct Area {
    esq: i32,
    dir: i32,
}

impl Area {
    fn largura(&self) -> i32 {
        self.dir - self.esq
    }
}

/// Uma caixa do CSS aberta enquanto os blocos de dentro sao diagramados.
struct CaixaAberta {
    indice: u16,
    /// Onde o fundo e as bordas vao entrar (antes do texto de dentro).
    item: usize,
    x: i32,
    largura: i32,
    topo: i32,
    /// Recheio e borda de baixo.
    fim_de_baixo: i32,
    margem_baixo: i32,
    area_de_fora: Area,
}

/// Abre e fecha as caixas do CSS entre um bloco e outro.
struct Caixas<'d> {
    doc: &'d Documento,
    abertas: Vec<CaixaAberta>,
    area: Area,
    /// Acabou de abrir uma caixa: o primeiro bloco de dentro nao ganha o
    /// espaco de antes (como a margem que encosta no topo da caixa).
    colado: bool,
}

impl<'d> Caixas<'d> {
    /// Deixa abertas exatamente as caixas de `cadeia` e poe o espaco antes
    /// do bloco. Devolve o y onde o bloco comeca.
    fn ajustar(&mut self, p: &mut Pagina, cadeia: &[u16], espaco: i32, mut y: i32) -> i32 {
        let comum = self.abertas.iter().zip(cadeia).take_while(|(a, c)| a.indice == **c).count();
        while self.abertas.len() > comum {
            y = self.fechar(p, y);
        }
        if cadeia.len() > comum {
            y += espaco;
            for &c in &cadeia[comum..] {
                y = self.abrir(p, c, y);
            }
            self.colado = true;
        } else if !self.colado {
            y += espaco;
        }
        y
    }

    fn abrir(&mut self, p: &mut Pagina, indice: u16, mut y: i32) -> i32 {
        let Some(c) = self.doc.caixas.get(indice as usize) else { return y };
        let fora = self.area;
        let disponivel = fora.largura();
        let px = |m: css::Medida| m.px(0, disponivel);
        // margem None = auto (centraliza quando a largura e menor)
        let margem = c.margem.map(|m| px(m).map(|v| v.clamp(0, 2000)));
        let recheio = c.recheio.map(|m| px(m).unwrap_or(0).clamp(0, 2000));
        let horizontal = recheio[DIREITA] + recheio[ESQUERDA] + c.borda[DIREITA] + c.borda[ESQUERDA];
        let (me, md) = (margem[ESQUERDA].unwrap_or(0), margem[DIREITA].unwrap_or(0));
        let mut largura = match c.largura.and_then(px) {
            Some(w) => w + horizontal,
            None => disponivel - me - md,
        };
        if let Some(maxima) = c.largura_max.and_then(px) {
            largura = largura.min(maxima + horizontal);
        }
        largura = largura.min(disponivel).max(horizontal + 16);
        let sobra = (disponivel - largura).max(0);
        let x = match (margem[ESQUERDA], margem[DIREITA]) {
            (None, None) => fora.esq + sobra / 2,
            (None, Some(d)) => fora.esq + (sobra - d).max(0),
            (Some(e), _) => fora.esq + e.min(sobra),
        };
        y += margem[CIMA].unwrap_or(0);
        let item = p.itens.len();
        // o fundo e as bordas sao acertados no fechar, quando a altura e conhecida
        p.itens.push(Item::Caixa { r: Retangulo::novo(x, y, largura, 0), fundo: None, bordas: [0; 4], cores: [None; 4] });
        self.abertas.push(CaixaAberta {
            indice,
            item,
            x,
            largura,
            topo: y,
            fim_de_baixo: recheio[BAIXO] + c.borda[BAIXO],
            margem_baixo: margem[BAIXO].unwrap_or(0),
            area_de_fora: fora,
        });
        let esq = x + c.borda[ESQUERDA] + recheio[ESQUERDA];
        let dir = (x + largura - c.borda[DIREITA] - recheio[DIREITA]).max(esq + 8);
        self.area = Area { esq, dir };
        y + c.borda[CIMA] + recheio[CIMA]
    }

    fn fechar(&mut self, p: &mut Pagina, mut y: i32) -> i32 {
        let Some(a) = self.abertas.pop() else { return y };
        let c: &Caixa = &self.doc.caixas[a.indice as usize];
        y += a.fim_de_baixo;
        // borda sem cor: a cor do texto da caixa (ou da pagina)
        let cores = c.borda_cor.map(|cor| cor.or(c.cor_texto).or(p.cor_texto));
        let r = Retangulo::novo(a.x, a.topo, a.largura, y - a.topo);
        p.itens[a.item] = Item::Caixa { r, fundo: c.fundo, bordas: c.borda, cores };
        self.area = a.area_de_fora;
        self.colado = false;
        y + a.margem_baixo
    }
}

/// Diagrama o documento para uma janela de `largura` px.
pub fn diagramar(doc: &Documento, largura: i32) -> Pagina {
    let largura = largura.max(96);
    let mut p = Pagina {
        altura: 0,
        largura,
        titulo: doc.titulo.clone(),
        itens: Vec::new(),
        faixas: Vec::new(),
        caixas_de_link: Vec::new(),
        links: doc.links.clone(),
        fundo: doc.fundo,
        cor_texto: doc.cor_texto,
    };
    let pagina = Area { esq: MARGEM, dir: largura - MARGEM };
    let mut caixas = Caixas { doc, abertas: Vec::new(), area: pagina, colado: false };
    let mut y = MARGEM;
    let mut inicios = Vec::with_capacity(doc.blocos.len() + 1);
    let mut i = 0;
    while i < doc.blocos.len() {
        inicios.push(p.itens.len());
        let bloco = &doc.blocos[i];
        let espaco = if bloco.margem && y > MARGEM { espaco_antes(bloco.tipo) } else { 0 };
        y = caixas.ajustar(&mut p, &bloco.caixas, espaco, y);
        caixas.colado = false;
        if bloco.tipo == TipoBloco::Linha {
            let fim = fim_da_tabela(&doc.blocos, i);
            y = diagramar_tabela(&mut p, &doc.blocos[i..fim], y, caixas.area);
            i = fim;
        } else {
            y = diagramar_bloco(&mut p, bloco, y, caixas.area);
            i += 1;
        }
    }
    // as caixas que ainda estao abertas fecham no fim da pagina
    y = caixas.ajustar(&mut p, &[], 0, y);
    inicios.push(p.itens.len());
    for k in 0..inicios.len() - 1 {
        p.marcar_faixa(inicios[k], inicios[k + 1]);
    }
    p.acertar_topos();
    p.altura = y + MARGEM;
    p
}

/// x onde comeca o texto de um bloco com `recuo` niveis (no maximo
/// metade da area).
fn esquerda(area: Area, recuo: u8) -> i32 {
    area.esq + (recuo as i32 * RECUO_PX).min(area.largura() / 2)
}

/// (subida, descida) de uma linha: o maximo das fontes dos pedacos (linha
/// vazia: a fonte normal do bloco).
fn metricas_da_linha(linha: &Linha, tipo: TipoBloco) -> (i32, i32) {
    let (mut subida, mut descida) = letra(tipo, 0).metricas();
    for pedaco in linha {
        let (s, d) = letra(tipo, pedaco.estilo).metricas();
        subida = subida.max(s);
        descida = descida.max(d);
    }
    (subida, descida)
}

/// Poe os pedacos de uma linha com o comeco em `x0` e o alto em `topo`,
/// guardando a caixa de cada pedaco que e link. `altura_minima` e o
/// line-height (0 = o das fontes): a sobra fica metade em cima, metade
/// embaixo. Devolve (base, altura).
fn por_linha(p: &mut Pagina, linha: Linha, x0: i32, topo: i32, tipo: TipoBloco, altura_minima: i32) -> (i32, i32) {
    let (subida, descida) = metricas_da_linha(&linha, tipo);
    let sobra = (altura_minima - subida - descida).max(0);
    let base = topo + subida + sobra / 2;
    for pedaco in linha {
        let x = x0 + pedaco.x;
        let letra = letra(tipo, pedaco.estilo);
        if let Some(link) = pedaco.link {
            let caixa = Retangulo::novo(x, topo, pedaco.largura, subida + descida + sobra);
            p.caixas_de_link.push((caixa, link));
        }
        let codigo = pedaco.estilo & CODIGO != 0;
        if letra == Letra::Mono && tipo != TipoBloco::Pre && codigo && !pedaco.texto.trim().is_empty() {
            let (s, d) = Letra::Mono.metricas();
            let fundo = Retangulo::novo(x - FOLGA_CODIGO, base - s + 1, pedaco.largura + 2 * FOLGA_CODIGO, s + d - 2);
            p.itens.push(Item::FundoPre(fundo));
        }
        // o <pre> inteiro e codigo: fica na cor do texto, sem o vermelho do <code>
        let estilo = if tipo == TipoBloco::Pre { pedaco.estilo & !CODIGO } else { pedaco.estilo };
        p.itens.push(Item::Texto { x, base, texto: pedaco.texto, letra, estilo, link: pedaco.link, tinta: pedaco.tinta });
    }
    (base, subida + descida + sobra)
}

/// Coloca um bloco a partir de `y` (o espaco antes ja foi posto) dentro
/// de `area` e devolve o y depois dele.
fn diagramar_bloco(p: &mut Pagina, bloco: &Bloco, mut y: i32, area: Area) -> i32 {
    let x0 = esquerda(area, bloco.recuo);
    let direita = area.dir;
    let topo = y;

    if bloco.tipo == TipoBloco::Regua {
        p.itens.push(Item::Regua { x: x0, y: y + 8, largura: direita - x0 });
        return y + 17;
    }
    let tipo = bloco.tipo;
    let medida = |texto: &str, estilo: u8| letra(tipo, estilo).fonte().largura(texto);
    let pre = tipo == TipoBloco::Pre;
    let largura = if pre { direita - x0 - 2 * FOLGA_PRE_X } else { direita - x0 };
    let linhas = linhas_do_bloco(tipo, &bloco.trechos, largura, &medida);
    let x_texto = if pre { x0 + FOLGA_PRE_X } else { x0 };
    if pre {
        let (s, d) = Letra::Mono.metricas();
        let altura = linhas.len() as i32 * (s + d) + 2 * FOLGA_PRE_Y;
        p.itens.push(Item::FundoPre(Retangulo::novo(x0, y, direita - x0, altura)));
        y += FOLGA_PRE_Y;
    }
    for (i, linha) in linhas.into_iter().enumerate() {
        // text-align: a sobra da linha vai toda para a esquerda (direita)
        // ou metade para cada lado (centro)
        let sobra = if pre { 0 } else { (largura - largura_da_linha(&linha)).max(0) };
        let deslocamento = match bloco.alinhar {
            1 => sobra / 2,
            2 => sobra,
            _ => 0,
        };
        let (base, altura) = por_linha(p, linha, x_texto + deslocamento, y, tipo, bloco.altura_linha);
        if i == 0 {
            marcador(p, bloco.marcador, x0, base, letra(tipo, 0));
        }
        y += altura;
    }
    if pre {
        y += FOLGA_PRE_Y;
    }
    if tipo == TipoBloco::Titulo(1) {
        p.itens.push(Item::Regua { x: x0, y: y + 3, largura: direita - x0 });
        y += 6;
    }
    if bloco.citacao > 0 {
        p.itens.push(Item::Barra(Retangulo::novo(x0 - 16, topo, 3, y - topo)));
    }
    y
}

/// Altura das linhas de uma celula.
fn altura_das_linhas(linhas: &[Linha]) -> i32 {
    linhas
        .iter()
        .map(|l| {
            let (s, d) = metricas_da_linha(l, TipoBloco::Linha);
            s + d
        })
        .sum()
}

/// Coloca as linhas de uma tabela a partir de `y` (o espaco antes ja foi
/// posto) como uma grade e devolve o y depois dela.
fn diagramar_tabela(p: &mut Pagina, linhas: &[Bloco], mut y: i32, area: Area) -> i32 {
    let x0 = esquerda(area, linhas[0].recuo);
    let celulas: Vec<Vec<(&[Trecho], bool)>> = linhas.iter().map(|b| celulas_da_linha(b, &b.trechos)).collect();
    let n = numero_de_colunas(&celulas);
    let medida = |texto: &str, estilo: u8| letra(TipoBloco::Linha, estilo).fonte().largura(texto);
    let (natural, minima) = medir_colunas(&celulas, n, &medida);
    // o texto fica com o que sobra das bordas e folgas
    let gasto = (n as i32 + 1) * BORDA_TABELA + n as i32 * 2 * FOLGA_CELULA;
    let disponivel = (area.dir - x0 - gasto).max(n as i32 * 8);
    let larguras = repartir(&natural, &minima, disponivel);
    let (quebradas, larguras) = quebrar_tabela(&celulas, &larguras, &medida);
    let colunas_px: Vec<i32> = larguras.iter().map(|&w| w + 2 * FOLGA_CELULA).collect();
    let total = colunas_px.iter().sum::<i32>() + (n as i32 + 1) * BORDA_TABELA;
    let topo = y;
    // a grade vai por cima dos fundos, entao entra depois do texto
    let mut grade = Vec::with_capacity(linhas.len() + n + 2);
    for (linha, colunas) in celulas.iter().zip(quebradas) {
        grade.push(Retangulo::novo(x0, y, total, BORDA_TABELA));
        y += BORDA_TABELA;
        let texto = colunas.iter().map(|c| altura_das_linhas(c)).max().unwrap_or(0);
        let altura = texto.max(fonte::INTER.altura_linha) + 2 * FOLGA_VERTICAL;
        let mut x = x0 + BORDA_TABELA;
        for (c, celula) in colunas.into_iter().enumerate() {
            if linha.get(c).is_some_and(|&(_, cabecalho)| cabecalho) {
                p.itens.push(Item::FundoCelula(Retangulo::novo(x, y, colunas_px[c], altura)));
            }
            let mut topo_linha = y + FOLGA_VERTICAL;
            for l in celula {
                let (_, h) = por_linha(p, l, x + FOLGA_CELULA, topo_linha, TipoBloco::Linha, 0);
                topo_linha += h;
            }
            x += colunas_px[c] + BORDA_TABELA;
        }
        y += altura;
    }
    grade.push(Retangulo::novo(x0, y, total, BORDA_TABELA));
    y += BORDA_TABELA;
    let mut x = x0;
    grade.push(Retangulo::novo(x, topo, BORDA_TABELA, y - topo));
    for w in &colunas_px {
        x += BORDA_TABELA + w;
        grade.push(Retangulo::novo(x, topo, BORDA_TABELA, y - topo));
    }
    p.itens.extend(grade.into_iter().map(Item::Grade));
    y
}

/// Bolinha ou numero na frente da primeira linha de um item, com a linha
/// de base em `base`.
fn marcador(p: &mut Pagina, m: Marcador, x0: i32, base: i32, letra: Letra) {
    let f = letra.fonte();
    match m {
        Marcador::Nenhum => {}
        Marcador::Bolinha(nivel) => {
            // no meio da altura de um 'x' (uns 3/8 da maiuscula)
            let cy = base - (f.capitular * 3 + 4) / 8;
            p.itens.push(Item::Bolinha { cx: x0 - 12, cy, raio: 2, cheia: nivel <= 1 });
        }
        Marcador::Numero(n) => {
            let texto = format!("{n}.");
            let x = x0 - f.largura(&texto) - 6;
            p.itens.push(Item::Texto { x, base, texto, letra, estilo: 0, link: None, tinta: Tinta::default() });
        }
    }
}

/// href do link no ponto (x, y) da pagina, se houver um.
pub fn link_em(pagina: &Pagina, x: i32, y: i32) -> Option<String> {
    pagina
        .caixas_de_link
        .iter()
        .find(|(caixa, _)| caixa.contem(x, y))
        .and_then(|&(_, i)| pagina.links.get(i).cloned())
}

/// Os bits de estilo que o render precisa saber (reexportados aqui
/// para o render nao depender do html).
pub const ESTILO_NEGRITO: u8 = NEGRITO;
pub const ESTILO_SUBLINHADO: u8 = SUBLINHADO;
pub const ESTILO_CODIGO: u8 = CODIGO;
pub const ESTILO_SEM_SUBLINHADO: u8 = SEM_SUBLINHADO;
