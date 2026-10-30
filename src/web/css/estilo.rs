//! O estilo calculado de um elemento (campos fixos: acessar e ler um
//! campo, sem tabela de espalhamento) e como cada propriedade CSS escreve
//! nele. Abreviacoes (margin, border, font, flex, grid-area...) sao
//! expandidas aqui mesmo.

use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

use super::token::Token;
use super::valores::{
    Comprimento, Contexto, Cor, Fixo, Imagem, Leitor, Linear, Medida, UM, dividir_virgulas, eh_zero, fixo, ler_comprimento, ler_cor, ler_imagem,
};
use crate::web::atomo::Atomo;

// --------------------------------------------------------------- tipos

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Exibir {
    Nenhum,
    Bloco,
    Inline,
    InlineBloco,
    Flex,
    InlineFlex,
    Grid,
    InlineGrid,
    ItemLista,
    Tabela,
    InlineTabela,
    LinhaTabela,
    CelulaTabela,
    GrupoLinhas,
    GrupoCabecalho,
    GrupoRodape,
    GrupoColunas,
    Coluna,
    Legenda,
    Conteudo,
    FlowRoot,
}

impl Exibir {
    pub fn eh_inline(self) -> bool {
        matches!(self, Exibir::Inline | Exibir::InlineBloco | Exibir::InlineFlex | Exibir::InlineGrid | Exibir::InlineTabela)
    }
    /// A versao "bloco" (para floats, absolutos e filhos de flex/grid).
    pub fn bloquificar(self) -> Exibir {
        match self {
            Exibir::Inline | Exibir::InlineBloco => Exibir::Bloco,
            Exibir::InlineFlex => Exibir::Flex,
            Exibir::InlineGrid => Exibir::Grid,
            Exibir::InlineTabela => Exibir::Tabela,
            Exibir::LinhaTabela | Exibir::CelulaTabela | Exibir::GrupoLinhas | Exibir::GrupoCabecalho
            | Exibir::GrupoRodape | Exibir::GrupoColunas | Exibir::Coluna | Exibir::Legenda => Exibir::Bloco,
            x => x,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Posicao {
    Estatica,
    Relativa,
    Absoluta,
    Fixa,
    Grudada,
}

/// Um tamanho: auto, medida, none (max-*), ou de conteudo.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tam {
    Auto,
    M(Medida),
    Nenhum,
    MinConteudo,
    MaxConteudo,
    AjustarConteudo,
}

impl Tam {
    pub fn eh_auto(self) -> bool {
        matches!(self, Tam::Auto)
    }
    pub fn medida(self) -> Option<Medida> {
        if let Tam::M(m) = self { Some(m) } else { None }
    }
    pub const ZERO: Tam = Tam::M(Medida::ZERO);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lado {
    Nenhum,
    Esquerda,
    Direita,
    Ambos,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EstiloBorda {
    Nenhum,
    Escondido,
    Solido,
    Tracejado,
    Pontilhado,
    Duplo,
    Relevo,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Transbordo {
    Visivel,
    Escondido,
    Rolar,
    Auto,
    Cortar,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Familia {
    Serif,
    SemSerif,
    Mono,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum AlturaLinha {
    Normal,
    Fator(f32),
    Px(Fixo),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Alinhamento {
    Esquerda,
    Direita,
    Centro,
    Justificado,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Brancos {
    Normal,
    Pre,
    SemQuebra,
    PreQuebra,
    PreLinha,
}

impl Brancos {
    /// Espacos e quebras sao mantidos?
    pub fn mantem_espacos(self) -> bool {
        matches!(self, Brancos::Pre | Brancos::PreQuebra)
    }
    pub fn mantem_quebras(self) -> bool {
        matches!(self, Brancos::Pre | Brancos::PreQuebra | Brancos::PreLinha)
    }
    pub fn quebra_linha(self) -> bool {
        !matches!(self, Brancos::Pre | Brancos::SemQuebra)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Transformacao {
    Nenhuma,
    Maiusculas,
    Minusculas,
    Capitalizar,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum AlinhamentoVertical {
    Base,
    Topo,
    Meio,
    Baixo,
    Sub,
    Super,
    TextoTopo,
    TextoBaixo,
    M(Medida),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TipoLista {
    Nenhum,
    Disco,
    Circulo,
    Quadrado,
    Decimal,
    DecimalZero,
    LetraMin,
    LetraMai,
    RomanoMin,
    RomanoMai,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Repetir {
    Repetir,
    X,
    Y,
    Nao,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TamanhoFundo {
    Auto,
    Cobrir,
    Conter,
    M(Medida, Option<Medida>),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DirecaoFlex {
    Linha,
    LinhaInversa,
    Coluna,
    ColunaInversa,
}

impl DirecaoFlex {
    pub fn eh_linha(self) -> bool {
        matches!(self, DirecaoFlex::Linha | DirecaoFlex::LinhaInversa)
    }
    pub fn inversa(self) -> bool {
        matches!(self, DirecaoFlex::LinhaInversa | DirecaoFlex::ColunaInversa)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Alinhar {
    Normal,
    Auto,
    Esticar,
    Inicio,
    Fim,
    Centro,
    Base,
    EspacoEntre,
    EspacoEmVolta,
    EspacoUniforme,
    Esquerda,
    Direita,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Faixa {
    M(Medida),
    Fr(f32),
    Auto,
    MinConteudo,
    MaxConteudo,
    MinMax(Box<Faixa>, Box<Faixa>),
    /// repeat(auto-fill/auto-fit, ...)
    Repetir(bool, Vec<Faixa>),
}

use alloc::boxed::Box;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LinhaGrade {
    Auto,
    Num(i32),
    Vao(i32),
}

#[derive(Clone, PartialEq, Debug)]
pub enum ParteConteudo {
    Texto(String),
    Atributo(Atomo),
    Contador(Atomo, TipoLista),
    AbreAspas,
    FechaAspas,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ajuste {
    Encher,
    Conter,
    Cobrir,
    Nenhum,
    Reduzir,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Sombra {
    pub x: Fixo,
    pub y: Fixo,
    pub borrao: Fixo,
    pub cor: Cor,
}

// ------------------------------------------------------------- o estilo

#[derive(Clone, PartialEq, Debug)]
pub struct Estilo {
    pub exibir: Exibir,
    pub posicao: Posicao,
    pub deslocamento: [Tam; 4], // top right bottom left
    pub flutuar: Lado,
    pub limpar: Lado,
    pub largura: Tam,
    pub altura: Tam,
    pub min_largura: Tam,
    pub min_altura: Tam,
    pub max_largura: Tam,
    pub max_altura: Tam,
    pub margem: [Tam; 4],
    pub preenchimento: [Medida; 4],
    pub borda_largura: [Fixo; 4],
    pub borda_estilo: [EstiloBorda; 4],
    pub borda_cor: [Option<Cor>; 4], // None = currentcolor
    pub raio: [Medida; 4],
    pub caixa_borda: bool,
    pub transbordo: [Transbordo; 2],
    pub visivel: bool,
    pub opacidade: u8,
    pub z: Option<i32>,
    pub proporcao: Option<f32>,
    // texto (herdados)
    pub cor: Cor,
    pub familia: Familia,
    pub tamanho_fonte: f32,
    pub peso: u16,
    pub italico: bool,
    pub altura_linha: AlturaLinha,
    pub alinhamento: Alinhamento,
    pub brancos: Brancos,
    pub quebra_palavra: bool,
    pub transformacao: Transformacao,
    pub espaco_letras: Fixo,
    pub espaco_palavras: Fixo,
    pub recuo: Medida,
    pub tipo_lista: TipoLista,
    pub lista_dentro: bool,
    pub imagem_lista: Option<Rc<Imagem>>,
    pub borda_colapsada: bool,
    pub espaco_borda: Fixo,
    pub cursor_mao: bool,
    pub variaveis: Rc<Variaveis>,
    // nao herdados
    pub decoracao: u8, // 1 sublinhado, 2 sobrelinha, 4 riscado
    pub cor_decoracao: Option<Cor>,
    pub alinhamento_vertical: AlinhamentoVertical,
    pub fundo: Cor,
    pub imagem_fundo: Option<Rc<Imagem>>,
    pub repetir_fundo: Repetir,
    pub posicao_fundo: (Medida, Medida),
    pub tamanho_fundo: TamanhoFundo,
    pub direcao_flex: DirecaoFlex,
    pub quebra_flex: bool,
    pub quebra_flex_inversa: bool,
    pub justificar: Alinhar,
    pub alinhar_itens: Alinhar,
    pub alinhar_proprio: Alinhar,
    pub alinhar_conteudo: Alinhar,
    pub justificar_itens: Alinhar,
    pub justificar_proprio: Alinhar,
    pub crescer: f32,
    pub encolher: f32,
    pub base_flex: Tam,
    pub ordem: i32,
    pub lacuna: (Medida, Medida), // (linha, coluna)
    pub colunas_grade: Rc<Vec<Faixa>>,
    pub linhas_grade: Rc<Vec<Faixa>>,
    pub auto_colunas: Rc<Vec<Faixa>>,
    pub auto_linhas: Rc<Vec<Faixa>>,
    pub fluxo_coluna: bool,
    pub fluxo_denso: bool,
    pub areas_grade: Rc<Vec<Vec<Option<Atomo>>>>,
    pub area: Option<Atomo>,
    pub grade_coluna: (LinhaGrade, LinhaGrade),
    pub grade_linha: (LinhaGrade, LinhaGrade),
    pub conteudo: Option<Rc<Vec<ParteConteudo>>>,
    pub reticencias: bool,
    pub ajuste_objeto: Ajuste,
    pub sombra: Option<Sombra>,
    pub deslocar: (Medida, Medida), // transform: translate()
    pub escala: f32,
    pub zera_contador: Option<(Atomo, i32)>,
    pub incrementa_contador: Option<(Atomo, i32)>,
    pub layout_fixo_tabela: bool,
    pub coluna_span: u16,
}

impl Estilo {
    pub fn inicial() -> Estilo {
        let auto = Tam::Auto;
        Estilo {
            exibir: Exibir::Inline,
            posicao: Posicao::Estatica,
            deslocamento: [auto; 4],
            flutuar: Lado::Nenhum,
            limpar: Lado::Nenhum,
            largura: auto,
            altura: auto,
            min_largura: auto,
            min_altura: auto,
            max_largura: Tam::Nenhum,
            max_altura: Tam::Nenhum,
            margem: [Tam::ZERO; 4],
            preenchimento: [Medida::ZERO; 4],
            borda_largura: [3 * UM; 4],
            borda_estilo: [EstiloBorda::Nenhum; 4],
            borda_cor: [None; 4],
            raio: [Medida::ZERO; 4],
            caixa_borda: false,
            transbordo: [Transbordo::Visivel; 2],
            visivel: true,
            opacidade: 255,
            z: None,
            proporcao: None,
            cor: Cor::PRETO,
            familia: Familia::Serif,
            tamanho_fonte: 16.0,
            peso: 400,
            italico: false,
            altura_linha: AlturaLinha::Normal,
            alinhamento: Alinhamento::Esquerda,
            brancos: Brancos::Normal,
            quebra_palavra: false,
            transformacao: Transformacao::Nenhuma,
            espaco_letras: 0,
            espaco_palavras: 0,
            recuo: Medida::ZERO,
            tipo_lista: TipoLista::Disco,
            lista_dentro: false,
            imagem_lista: None,
            borda_colapsada: false,
            espaco_borda: 2 * UM,
            cursor_mao: false,
            variaveis: Rc::new(Variaveis::default()),
            decoracao: 0,
            cor_decoracao: None,
            alinhamento_vertical: AlinhamentoVertical::Base,
            fundo: Cor::TRANSPARENTE,
            imagem_fundo: None,
            repetir_fundo: Repetir::Repetir,
            posicao_fundo: (Medida::ZERO, Medida::ZERO),
            tamanho_fundo: TamanhoFundo::Auto,
            direcao_flex: DirecaoFlex::Linha,
            quebra_flex: false,
            quebra_flex_inversa: false,
            justificar: Alinhar::Normal,
            alinhar_itens: Alinhar::Normal,
            alinhar_proprio: Alinhar::Auto,
            alinhar_conteudo: Alinhar::Normal,
            justificar_itens: Alinhar::Normal,
            justificar_proprio: Alinhar::Auto,
            crescer: 0.0,
            encolher: 1.0,
            base_flex: auto,
            ordem: 0,
            lacuna: (Medida::ZERO, Medida::ZERO),
            colunas_grade: Rc::new(Vec::new()),
            linhas_grade: Rc::new(Vec::new()),
            auto_colunas: Rc::new(Vec::new()),
            auto_linhas: Rc::new(Vec::new()),
            fluxo_coluna: false,
            fluxo_denso: false,
            areas_grade: Rc::new(Vec::new()),
            area: None,
            grade_coluna: (LinhaGrade::Auto, LinhaGrade::Auto),
            grade_linha: (LinhaGrade::Auto, LinhaGrade::Auto),
            conteudo: None,
            reticencias: false,
            ajuste_objeto: Ajuste::Encher,
            sombra: None,
            deslocar: (Medida::ZERO, Medida::ZERO),
            escala: 1.0,
            zera_contador: None,
            incrementa_contador: None,
            layout_fixo_tabela: false,
            coluna_span: 1,
        }
    }

    /// O estilo inicial de um filho: os herdados vem do pai.
    pub fn herdar(pai: &Estilo) -> Estilo {
        Estilo::herdar_de(&Estilo::inicial(), pai)
    }

    /// Como `herdar`, partindo de um estilo inicial ja pronto (poupa as
    /// alocacoes do inicial a cada elemento).
    pub fn herdar_de(inicial: &Estilo, pai: &Estilo) -> Estilo {
        let mut e = inicial.clone();
        e.cor = pai.cor;
        e.familia = pai.familia;
        e.tamanho_fonte = pai.tamanho_fonte;
        e.peso = pai.peso;
        e.italico = pai.italico;
        e.altura_linha = pai.altura_linha;
        e.alinhamento = pai.alinhamento;
        e.brancos = pai.brancos;
        e.quebra_palavra = pai.quebra_palavra;
        e.transformacao = pai.transformacao;
        e.espaco_letras = pai.espaco_letras;
        e.espaco_palavras = pai.espaco_palavras;
        e.recuo = pai.recuo;
        e.tipo_lista = pai.tipo_lista;
        e.lista_dentro = pai.lista_dentro;
        e.imagem_lista = pai.imagem_lista.clone();
        e.borda_colapsada = pai.borda_colapsada;
        e.espaco_borda = pai.espaco_borda;
        e.visivel = pai.visivel;
        e.cursor_mao = pai.cursor_mao;
        e.variaveis = pai.variaveis.clone();
        e.justificar_itens = pai.justificar_itens;
        e
    }

    /// Altura de linha em px (ponto fixo).
    pub fn linha(&self) -> Fixo {
        match self.altura_linha {
            AlturaLinha::Normal => fixo(self.tamanho_fonte * 1.2),
            AlturaLinha::Fator(f) => fixo(self.tamanho_fonte * f),
            AlturaLinha::Px(p) => p,
        }
    }

    pub fn tamanho_fixo(&self) -> Fixo {
        fixo(self.tamanho_fonte)
    }

    pub fn cor_borda(&self, i: usize) -> Cor {
        self.borda_cor[i].unwrap_or(self.cor)
    }

    /// Largura efetiva da borda (0 se o estilo e none/hidden).
    pub fn borda(&self, i: usize) -> Fixo {
        if matches!(self.borda_estilo[i], EstiloBorda::Nenhum | EstiloBorda::Escondido) { 0 } else { self.borda_largura[i] }
    }

    pub fn fora_do_fluxo(&self) -> bool {
        matches!(self.posicao, Posicao::Absoluta | Posicao::Fixa)
    }

    pub fn variavel(&self, nome: Atomo) -> Option<&Rc<[Token]>> {
        self.variaveis.achar(nome)
    }
}

/// Variaveis CSS (--x) em camadas: cada elemento que define alguma ganha
/// uma camada so com as dele, apontando para a do pai. Antes era um mapa
/// inteiro copiado a cada elemento que definia uma variavel (no GitHub,
/// centenas de entradas copiadas milhares de vezes).
#[derive(Clone, Debug, Default)]
pub struct Variaveis {
    proprias: Vec<(Atomo, Rc<[Token]>)>,
    pai: Option<Rc<Variaveis>>,
}

impl PartialEq for Variaveis {
    fn eq(&self, outro: &Variaveis) -> bool {
        let pais = match (&self.pai, &outro.pai) {
            (None, None) => true,
            (Some(a), Some(b)) => Rc::ptr_eq(a, b) || **a == **b,
            _ => false,
        };
        pais && self.proprias.len() == outro.proprias.len() && self.proprias.iter().zip(&outro.proprias).all(|(a, b)| a.0 == b.0 && a.1 == b.1)
    }
}

impl Variaveis {
    /// Uma camada vazia em cima de `pai`.
    pub fn sobre(pai: &Rc<Variaveis>) -> Variaveis {
        Variaveis { proprias: Vec::new(), pai: Some(pai.clone()) }
    }

    /// Define (a ultima definicao vale).
    pub fn definir(&mut self, nome: Atomo, valor: Rc<[Token]>) {
        self.proprias.push((nome, valor));
    }

    pub fn achar(&self, nome: Atomo) -> Option<&Rc<[Token]>> {
        let mut camada = self;
        loop {
            if let Some((_, v)) = camada.proprias.iter().rev().find(|(n, _)| *n == nome) {
                return Some(v);
            }
            camada = camada.pai.as_deref()?;
        }
    }
}

// -------------------------------------------------------- propriedades

/// Contexto para converter medidas.
pub struct Calculo {
    pub ctx: Contexto,
    /// Estilo do pai (para inherit e para em em font-size).
    pub tamanho_pai: f32,
    pub quirks: bool,
}

fn ident(t: &[Token]) -> Option<String> {
    let mut l = Leitor::novo(t);
    let s = l.ident()?;
    if l.vazio() { Some(s) } else { None }
}

fn medida(t: &[Token], c: &Calculo) -> Option<Medida> {
    let mut l = Leitor::novo(t);
    let m = ler_comprimento(&mut l, c.quirks)?;
    if !l.vazio() {
        return None;
    }
    Some(m.calcular(&c.ctx))
}

fn tam(t: &[Token], c: &Calculo, nenhum: bool) -> Option<Tam> {
    match ident(t).as_deref() {
        Some("auto") => return Some(if nenhum { Tam::Nenhum } else { Tam::Auto }),
        Some("none") if nenhum => return Some(Tam::Nenhum),
        Some("min-content") => return Some(Tam::MinConteudo),
        Some("max-content") => return Some(Tam::MaxConteudo),
        Some("fit-content") | Some("-webkit-fit-content") | Some("-moz-fit-content") | Some("stretch")
        | Some("-webkit-fill-available") | Some("-moz-available") => {
            return Some(if ident(t).as_deref() == Some("fit-content") { Tam::AjustarConteudo } else { Tam::Auto });
        }
        _ => {}
    }
    let m = medida(t, c)?;
    Some(Tam::M(m))
}

/// Divide em ate 4 valores ("1px 2px" -> top/bottom=1, left/right=2).
fn quatro<T: Copy>(v: &[T]) -> Option<[T; 4]> {
    Some(match v.len() {
        1 => [v[0]; 4],
        2 => [v[0], v[1], v[0], v[1]],
        3 => [v[0], v[1], v[2], v[1]],
        4 => [v[0], v[1], v[2], v[3]],
        _ => return None,
    })
}

/// Separa os valores de primeiro nivel (por espaco), mantendo funcoes.
pub fn partes(t: &[Token]) -> Vec<&[Token]> {
    let mut v = Vec::new();
    let mut nivel = 0;
    let mut inicio = None;
    for (k, x) in t.iter().enumerate() {
        match x {
            Token::Espaco if nivel == 0 => {
                if let Some(i) = inicio.take() {
                    v.push(&t[i..k]);
                }
                continue;
            }
            Token::Funcao(_) | Token::AbreParen | Token::AbreColchete => nivel += 1,
            Token::FechaParen | Token::FechaColchete => nivel -= 1,
            _ => {}
        }
        if inicio.is_none() {
            inicio = Some(k);
        }
    }
    if let Some(i) = inicio {
        v.push(&t[i..]);
    }
    v
}

fn largura_borda(t: &[Token], c: &Calculo) -> Option<Fixo> {
    match ident(t).as_deref() {
        Some("thin") => Some(UM),
        Some("medium") => Some(3 * UM),
        Some("thick") => Some(5 * UM),
        _ => medida(t, c).map(|m| m.px.max(0)),
    }
}

fn estilo_borda(s: &str) -> Option<EstiloBorda> {
    Some(match s {
        "none" => EstiloBorda::Nenhum,
        "hidden" => EstiloBorda::Escondido,
        "solid" => EstiloBorda::Solido,
        "dashed" => EstiloBorda::Tracejado,
        "dotted" => EstiloBorda::Pontilhado,
        "double" => EstiloBorda::Duplo,
        "groove" | "ridge" | "inset" | "outset" => EstiloBorda::Relevo,
        _ => return None,
    })
}

fn cor(t: &[Token], atual: Cor) -> Option<Option<Cor>> {
    if ident(t).as_deref() == Some("currentcolor") {
        return Some(None);
    }
    let mut l = Leitor::novo(t);
    let c = ler_cor(&mut l, Some(atual))?;
    if l.vazio() { Some(Some(c)) } else { None }
}

/// (largura, estilo, cor) de "1px solid red" em qualquer ordem.
fn borda_abreviada(t: &[Token], c: &Calculo, atual: Cor) -> Option<(Fixo, EstiloBorda, Option<Cor>)> {
    let mut largura = 3 * UM;
    let mut est = EstiloBorda::Nenhum;
    let mut co = None;
    for p in partes(t) {
        if let Some(s) = ident(p).and_then(|s| estilo_borda(&s)) {
            est = s;
        } else if let Some(l) = largura_borda(p, c) {
            largura = l;
        } else if let Some(k) = cor(p, atual) {
            co = k;
        } else {
            return None;
        }
    }
    Some((largura, est, co))
}

fn alinhar(s: &str) -> Option<Alinhar> {
    Some(match s {
        "normal" => Alinhar::Normal,
        "auto" => Alinhar::Auto,
        "stretch" => Alinhar::Esticar,
        "start" | "flex-start" | "self-start" => Alinhar::Inicio,
        "end" | "flex-end" | "self-end" => Alinhar::Fim,
        "center" => Alinhar::Centro,
        "baseline" | "first baseline" | "last baseline" => Alinhar::Base,
        "space-between" => Alinhar::EspacoEntre,
        "space-around" => Alinhar::EspacoEmVolta,
        "space-evenly" => Alinhar::EspacoUniforme,
        "left" => Alinhar::Esquerda,
        "right" => Alinhar::Direita,
        _ => return None,
    })
}

/// Valor de align/justify: ignora "safe"/"unsafe"/"legacy"/"first".
fn alinhar_tokens(t: &[Token]) -> Option<Alinhar> {
    let palavras: Vec<String> = partes(t).iter().filter_map(|p| ident(p)).collect();
    let ultima = palavras.iter().rev().find(|p| !matches!(p.as_str(), "safe" | "unsafe" | "legacy" | "first" | "last"))?;
    alinhar(ultima)
}

fn tipo_lista(s: &str) -> Option<TipoLista> {
    Some(match s {
        "none" => TipoLista::Nenhum,
        "disc" => TipoLista::Disco,
        "circle" => TipoLista::Circulo,
        "square" => TipoLista::Quadrado,
        "decimal" => TipoLista::Decimal,
        "decimal-leading-zero" => TipoLista::DecimalZero,
        "lower-alpha" | "lower-latin" => TipoLista::LetraMin,
        "upper-alpha" | "upper-latin" => TipoLista::LetraMai,
        "lower-roman" => TipoLista::RomanoMin,
        "upper-roman" => TipoLista::RomanoMai,
        _ => return None,
    })
}

fn faixa(t: &[Token], c: &Calculo) -> Option<Faixa> {
    if let [Token::Dimensao(v, u)] = t {
        if u == "fr" {
            return Some(Faixa::Fr(*v));
        }
    }
    if let [Token::Funcao(f), ..] = t {
        let mut l = Leitor::novo(t);
        l.proximo();
        let args = l.argumentos()?;
        let pedacos: Vec<&[Token]> = dividir_virgulas(&args);
        return match f.as_str() {
            "minmax" if pedacos.len() == 2 => {
                Some(Faixa::MinMax(Box::new(faixa(apara(pedacos[0]), c)?), Box::new(faixa(apara(pedacos[1]), c)?)))
            }
            "fit-content" => Some(Faixa::MinMax(Box::new(Faixa::Auto), Box::new(Faixa::M(medida(apara(pedacos[0]), c)?)))),
            "repeat" if pedacos.len() == 2 => {
                let lista = faixas(apara(pedacos[1]), c)?;
                match ident(apara(pedacos[0])).as_deref() {
                    Some("auto-fill") => Some(Faixa::Repetir(true, lista)),
                    Some("auto-fit") => Some(Faixa::Repetir(false, lista)),
                    _ => {
                        let n = Leitor::novo(pedacos[0]).numero()? as usize;
                        let mut v = Vec::new();
                        for _ in 0..n.min(1000) {
                            v.extend(lista.iter().cloned());
                        }
                        Some(Faixa::Repetir(true, v)).map(|f| if let Faixa::Repetir(_, v) = f { Faixa::Repetir(true, v) } else { f })
                            .map(|f| match f {
                                // repeat(N, ...) ja expandido: marca com auto-fill falso e sem "auto"
                                Faixa::Repetir(_, v) => Faixa::MinMax(Box::new(Faixa::Auto), Box::new(Faixa::Repetir(false, v))),
                                f => f,
                            })
                    }
                }
            }
            _ => medida(t, c).map(Faixa::M),
        };
    }
    match ident(t).as_deref() {
        Some("auto") => Some(Faixa::Auto),
        Some("min-content") => Some(Faixa::MinConteudo),
        Some("max-content") => Some(Faixa::MaxConteudo),
        _ => medida(t, c).map(Faixa::M),
    }
}

fn apara(t: &[Token]) -> &[Token] {
    let mut a = 0;
    let mut b = t.len();
    while a < b && t[a] == Token::Espaco {
        a += 1;
    }
    while b > a && t[b - 1] == Token::Espaco {
        b -= 1;
    }
    &t[a..b]
}

/// Lista de faixas da grade ("1fr 200px repeat(3, 1fr)"); nomes de
/// linha ([a]) sao ignorados.
fn faixas(t: &[Token], c: &Calculo) -> Option<Vec<Faixa>> {
    let mut v = Vec::new();
    for p in partes(t) {
        if p.first() == Some(&Token::AbreColchete) {
            continue;
        }
        match faixa(p, c)? {
            // repeat(N, ...) expandido vem embrulhado: desembrulha
            Faixa::MinMax(a, b) if *a == Faixa::Auto && matches!(*b, Faixa::Repetir(false, _)) => {
                if let Faixa::Repetir(_, lista) = *b {
                    v.extend(lista);
                }
            }
            f => v.push(f),
        }
    }
    Some(v)
}

fn linha_grade(t: &[Token]) -> Option<LinhaGrade> {
    let mut l = Leitor::novo(t);
    match l.espiar()? {
        Token::Ident(s) if s.eq_ignore_ascii_case("auto") => Some(LinhaGrade::Auto),
        Token::Ident(s) if s.eq_ignore_ascii_case("span") => {
            l.proximo();
            Some(LinhaGrade::Vao(l.numero().unwrap_or(1.0) as i32))
        }
        Token::Numero(v, _) => Some(LinhaGrade::Num(*v as i32)),
        _ => Some(LinhaGrade::Auto), // nomes de linha: nao suportados
    }
}

fn par_linhas(t: &[Token]) -> Option<(LinhaGrade, LinhaGrade)> {
    let mut it = t.split(|x| *x == Token::Delim('/'));
    let a = linha_grade(apara(it.next()?))?;
    let b = match it.next() {
        Some(b) => linha_grade(apara(b))?,
        None => LinhaGrade::Auto,
    };
    Some((a, b))
}

fn familia(t: &[Token]) -> Familia {
    for nome in t.split(|x| *x == Token::Virgula) {
        let s: String = nome
            .iter()
            .filter_map(|x| match x {
                Token::Ident(s) | Token::Texto(s) => Some(s.to_ascii_lowercase()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");
        if s.is_empty() {
            continue;
        }
        if s.contains("mono") || s.contains("courier") || s.contains("consol") || s.contains("menlo") || s == "code" {
            return Familia::Mono;
        }
        if s == "serif" || s.contains("times") || s.contains("georgia") || (s.contains("serif") && !s.contains("sans")) {
            return Familia::Serif;
        }
        if s.contains("sans") || s.contains("arial") || s.contains("helvetica") || s.contains("roboto") || s.contains("system")
            || s.contains("segoe") || s.contains("verdana") || s.contains("tahoma") || s.contains("inter") || s.contains("open")
            || s.contains("lato") || s.contains("noto") || s.contains("ubuntu") || s.contains("apple")
        {
            return Familia::SemSerif;
        }
    }
    Familia::SemSerif
}

fn tamanho_fonte(t: &[Token], c: &Calculo) -> Option<f32> {
    let pai = c.tamanho_pai;
    let v = match ident(t).as_deref() {
        Some("xx-small") => 9.0,
        Some("x-small") => 10.0,
        Some("small") => 13.0,
        Some("medium") => 16.0,
        Some("large") => 18.0,
        Some("x-large") => 24.0,
        Some("xx-large") => 32.0,
        Some("xxx-large") => 48.0,
        Some("smaller") => pai / 1.2,
        Some("larger") => pai * 1.2,
        _ => {
            let mut l = Leitor::novo(t);
            let comp = ler_comprimento(&mut l, c.quirks)?;
            // em e % no font-size sao do pai
            let ctx = Contexto { fonte: pai, ..c.ctx };
            let m = comp.calcular(&ctx);
            let px = m.px as f32 / 64.0 + pai * m.pct as f32 / (64.0 * 100.0);
            return Some(px.clamp(0.0, 2000.0));
        }
    };
    Some(v)
}

fn peso(t: &[Token], pai: u16) -> Option<u16> {
    match ident(t).as_deref() {
        Some("normal") => Some(400),
        Some("bold") => Some(700),
        Some("bolder") => Some(if pai < 400 { 400 } else if pai < 600 { 700 } else { 900 }),
        Some("lighter") => Some(if pai < 600 { 100 } else if pai < 800 { 400 } else { 700 }),
        _ => Leitor::novo(t).numero().map(|v| v.clamp(1.0, 1000.0) as u16),
    }
}

fn altura_linha(t: &[Token], c: &Calculo, fonte: f32) -> Option<AlturaLinha> {
    if ident(t).as_deref() == Some("normal") {
        return Some(AlturaLinha::Normal);
    }
    let mut l = Leitor::novo(t);
    if let Some(n) = l.numero() {
        return Some(AlturaLinha::Fator(n));
    }
    let m = medida(t, c)?;
    if m.tem_pct() {
        return Some(AlturaLinha::Fator(m.pct as f32 / 6400.0 + m.px as f32 / 64.0 / fonte.max(1.0)));
    }
    Some(AlturaLinha::Px(m.px))
}

impl Estilo {
    /// Aplica "propriedade: valor" (valor ja sem var(), em tokens).
    /// Devolve false se o valor nao e valido para a propriedade.
    pub fn aplicar(&mut self, nome: &str, t: &[Token], c: &Calculo) -> bool {
        let t = apara(t);
        if t.is_empty() {
            return false;
        }
        let palavra = ident(t);
        let p = palavra.as_deref();
        macro_rules! ok {
            ($e:expr) => {{
                match $e {
                    Some(v) => v,
                    None => return false,
                }
            }};
        }
        match nome {
            "display" => {
                let ps: Vec<String> = partes(t).iter().filter_map(|x| ident(x)).collect();
                let tem = |s: &str| ps.iter().any(|x| x == s);
                self.exibir = if tem("none") {
                    Exibir::Nenhum
                } else if tem("contents") {
                    Exibir::Conteudo
                } else if tem("list-item") {
                    Exibir::ItemLista
                } else if tem("flex") || tem("-webkit-flex") || tem("-webkit-box") || tem("-ms-flexbox") {
                    if tem("inline") { Exibir::InlineFlex } else { Exibir::Flex }
                } else if tem("inline-flex") || tem("-webkit-inline-flex") || tem("-webkit-inline-box") {
                    Exibir::InlineFlex
                } else if tem("grid") || tem("-ms-grid") {
                    if tem("inline") { Exibir::InlineGrid } else { Exibir::Grid }
                } else if tem("inline-grid") {
                    Exibir::InlineGrid
                } else if tem("inline-block") || (tem("inline") && tem("flow-root")) {
                    Exibir::InlineBloco
                } else if tem("flow-root") {
                    Exibir::FlowRoot
                } else if tem("table") {
                    if tem("inline") { Exibir::InlineTabela } else { Exibir::Tabela }
                } else if tem("inline-table") {
                    Exibir::InlineTabela
                } else if tem("table-row") {
                    Exibir::LinhaTabela
                } else if tem("table-cell") {
                    Exibir::CelulaTabela
                } else if tem("table-row-group") {
                    Exibir::GrupoLinhas
                } else if tem("table-header-group") {
                    Exibir::GrupoCabecalho
                } else if tem("table-footer-group") {
                    Exibir::GrupoRodape
                } else if tem("table-column-group") {
                    Exibir::GrupoColunas
                } else if tem("table-column") {
                    Exibir::Coluna
                } else if tem("table-caption") {
                    Exibir::Legenda
                } else if tem("block") || tem("run-in") {
                    Exibir::Bloco
                } else if tem("inline") || tem("ruby") || tem("ruby-text") || tem("ruby-base") {
                    Exibir::Inline
                } else {
                    return false;
                };
            }
            "position" => {
                self.posicao = match p {
                    Some("static") => Posicao::Estatica,
                    Some("relative") => Posicao::Relativa,
                    Some("absolute") => Posicao::Absoluta,
                    Some("fixed") => Posicao::Fixa,
                    Some("sticky") | Some("-webkit-sticky") => Posicao::Grudada,
                    _ => return false,
                }
            }
            "top" => self.deslocamento[0] = ok!(tam(t, c, false)),
            "right" => self.deslocamento[1] = ok!(tam(t, c, false)),
            "bottom" => self.deslocamento[2] = ok!(tam(t, c, false)),
            "left" => self.deslocamento[3] = ok!(tam(t, c, false)),
            "inset" => {
                let v: Vec<Tam> = ok!(partes(t).iter().map(|x| tam(x, c, false)).collect::<Option<Vec<_>>>());
                self.deslocamento = ok!(quatro(&v));
            }
            "float" => {
                self.flutuar = match p {
                    Some("left") | Some("inline-start") => Lado::Esquerda,
                    Some("right") | Some("inline-end") => Lado::Direita,
                    Some("none") => Lado::Nenhum,
                    _ => return false,
                }
            }
            "clear" => {
                self.limpar = match p {
                    Some("left") | Some("inline-start") => Lado::Esquerda,
                    Some("right") | Some("inline-end") => Lado::Direita,
                    Some("both") => Lado::Ambos,
                    Some("none") => Lado::Nenhum,
                    _ => return false,
                }
            }
            "width" | "inline-size" => self.largura = ok!(tam(t, c, false)),
            "height" | "block-size" => self.altura = ok!(tam(t, c, false)),
            "min-width" | "min-inline-size" => self.min_largura = ok!(tam(t, c, false)),
            "min-height" | "min-block-size" => self.min_altura = ok!(tam(t, c, false)),
            "max-width" | "max-inline-size" => self.max_largura = ok!(tam(t, c, true)),
            "max-height" | "max-block-size" => self.max_altura = ok!(tam(t, c, true)),
            "margin" => {
                let v: Vec<Tam> = ok!(partes(t).iter().map(|x| tam(x, c, false)).collect::<Option<Vec<_>>>());
                self.margem = ok!(quatro(&v));
            }
            "margin-top" | "margin-block-start" => self.margem[0] = ok!(tam(t, c, false)),
            "margin-right" | "margin-inline-end" => self.margem[1] = ok!(tam(t, c, false)),
            "margin-bottom" | "margin-block-end" => self.margem[2] = ok!(tam(t, c, false)),
            "margin-left" | "margin-inline-start" => self.margem[3] = ok!(tam(t, c, false)),
            "margin-inline" | "margin-block" => {
                let v: Vec<Tam> = ok!(partes(t).iter().map(|x| tam(x, c, false)).collect::<Option<Vec<_>>>());
                let (a, b) = match v.len() {
                    1 => (v[0], v[0]),
                    2 => (v[0], v[1]),
                    _ => return false,
                };
                if nome == "margin-inline" {
                    self.margem[3] = a;
                    self.margem[1] = b;
                } else {
                    self.margem[0] = a;
                    self.margem[2] = b;
                }
            }
            "padding" => {
                let v: Vec<Medida> = ok!(partes(t).iter().map(|x| medida(x, c)).collect::<Option<Vec<_>>>());
                self.preenchimento = ok!(quatro(&v));
            }
            "padding-top" | "padding-block-start" => self.preenchimento[0] = ok!(medida(t, c)),
            "padding-right" | "padding-inline-end" => self.preenchimento[1] = ok!(medida(t, c)),
            "padding-bottom" | "padding-block-end" => self.preenchimento[2] = ok!(medida(t, c)),
            "padding-left" | "padding-inline-start" => self.preenchimento[3] = ok!(medida(t, c)),
            "padding-inline" | "padding-block" => {
                let v: Vec<Medida> = ok!(partes(t).iter().map(|x| medida(x, c)).collect::<Option<Vec<_>>>());
                let (a, b) = match v.len() {
                    1 => (v[0], v[0]),
                    2 => (v[0], v[1]),
                    _ => return false,
                };
                if nome == "padding-inline" {
                    self.preenchimento[3] = a;
                    self.preenchimento[1] = b;
                } else {
                    self.preenchimento[0] = a;
                    self.preenchimento[2] = b;
                }
            }
            "border" => {
                let (l, e, co) = ok!(borda_abreviada(t, c, self.cor));
                self.borda_largura = [l; 4];
                self.borda_estilo = [e; 4];
                self.borda_cor = [co; 4];
            }
            "border-top" | "border-right" | "border-bottom" | "border-left" | "border-block-start" | "border-block-end"
            | "border-inline-start" | "border-inline-end" | "border-block" | "border-inline" => {
                let (l, e, co) = ok!(borda_abreviada(t, c, self.cor));
                for &i in lados(nome) {
                    self.borda_largura[i] = l;
                    self.borda_estilo[i] = e;
                    self.borda_cor[i] = co;
                }
            }
            "border-width" => {
                let v: Vec<Fixo> = ok!(partes(t).iter().map(|x| largura_borda(x, c)).collect::<Option<Vec<_>>>());
                self.borda_largura = ok!(quatro(&v));
            }
            "border-style" => {
                let v: Vec<EstiloBorda> =
                    ok!(partes(t).iter().map(|x| ident(x).and_then(|s| estilo_borda(&s))).collect::<Option<Vec<_>>>());
                self.borda_estilo = ok!(quatro(&v));
            }
            "border-color" => {
                let v: Vec<Option<Cor>> = ok!(partes(t).iter().map(|x| cor(x, self.cor)).collect::<Option<Vec<_>>>());
                self.borda_cor = ok!(quatro(&v));
            }
            n if n.starts_with("border-") && n.ends_with("-width") && !n.contains("image") => {
                let l = ok!(largura_borda(t, c));
                for &i in lados(&n[..n.len() - 6]) {
                    self.borda_largura[i] = l;
                }
            }
            n if n.starts_with("border-") && n.ends_with("-style") => {
                let e = ok!(p.and_then(estilo_borda));
                for &i in lados(&n[..n.len() - 6]) {
                    self.borda_estilo[i] = e;
                }
            }
            n if n.starts_with("border-") && n.ends_with("-color") => {
                let co = ok!(cor(t, self.cor));
                for &i in lados(&n[..n.len() - 6]) {
                    self.borda_cor[i] = co;
                }
            }
            "border-radius" => {
                let antes_barra: Vec<&Token> = t.iter().take_while(|x| **x != Token::Delim('/')).collect();
                let antes: Vec<Token> = antes_barra.into_iter().cloned().collect();
                let v: Vec<Medida> = ok!(partes(&antes).iter().map(|x| medida(x, c)).collect::<Option<Vec<_>>>());
                self.raio = ok!(quatro(&v));
            }
            "border-top-left-radius" | "border-start-start-radius" => self.raio[0] = ok!(medida(partes(t)[0], c)),
            "border-top-right-radius" | "border-start-end-radius" => self.raio[1] = ok!(medida(partes(t)[0], c)),
            "border-bottom-right-radius" | "border-end-end-radius" => self.raio[2] = ok!(medida(partes(t)[0], c)),
            "border-bottom-left-radius" | "border-end-start-radius" => self.raio[3] = ok!(medida(partes(t)[0], c)),
            "border-collapse" => self.borda_colapsada = p == Some("collapse"),
            "border-spacing" => self.espaco_borda = ok!(medida(partes(t)[0], c)).px,
            "box-sizing" | "-webkit-box-sizing" | "-moz-box-sizing" => self.caixa_borda = p == Some("border-box"),
            "overflow" | "overflow-x" | "overflow-y" => {
                let v: Vec<Transbordo> = ok!(partes(t)
                    .iter()
                    .map(|x| match ident(x).as_deref() {
                        Some("visible") => Some(Transbordo::Visivel),
                        Some("hidden") => Some(Transbordo::Escondido),
                        Some("scroll") => Some(Transbordo::Rolar),
                        Some("auto") | Some("overlay") => Some(Transbordo::Auto),
                        Some("clip") => Some(Transbordo::Cortar),
                        _ => None,
                    })
                    .collect::<Option<Vec<_>>>());
                match nome {
                    "overflow-x" => self.transbordo[0] = v[0],
                    "overflow-y" => self.transbordo[1] = v[0],
                    _ => self.transbordo = [v[0], *v.get(1).unwrap_or(&v[0])],
                }
            }
            "visibility" => self.visivel = p == Some("visible"),
            "opacity" => {
                let mut l = Leitor::novo(t);
                let v = match l.proximo() {
                    Some(Token::Numero(v, _)) => *v,
                    Some(Token::Porcentagem(v)) => *v / 100.0,
                    _ => return false,
                };
                self.opacidade = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
            "z-index" => self.z = if p == Some("auto") { None } else { Some(ok!(Leitor::novo(t).numero()) as i32) },
            "aspect-ratio" => {
                let nums: Vec<f32> = t.iter().filter_map(|x| if let Token::Numero(v, _) = x { Some(*v) } else { None }).collect();
                self.proporcao = match nums.as_slice() {
                    [a, b] if *b > 0.0 => Some(a / b),
                    [a] => Some(*a),
                    _ => None,
                };
            }
            "color" => {
                let co = ok!(cor(t, self.cor));
                if let Some(co) = co {
                    self.cor = co;
                }
            }
            "font-family" => self.familia = familia(t),
            "font-size" => self.tamanho_fonte = ok!(tamanho_fonte(t, c)),
            "font-weight" => self.peso = ok!(peso(t, self.peso)),
            "font-style" => self.italico = matches!(p, Some("italic") | Some("oblique")) || t.iter().any(|x| matches!(x, Token::Ident(s) if s == "oblique")),
            "font" => {
                // [style] [variant] [weight] size[/line-height] family
                let ps = partes(t);
                let mut k = 0;
                while k < ps.len() {
                    match ident(ps[k]).as_deref() {
                        Some("italic") | Some("oblique") => self.italico = true,
                        Some("normal") | Some("small-caps") => {}
                        Some(s) if peso(ps[k], self.peso).is_some() && s != "inherit" => self.peso = peso(ps[k], self.peso).unwrap_or(400),
                        None if matches!(ps[k], [Token::Numero(..)]) => self.peso = peso(ps[k], self.peso).unwrap_or(400),
                        _ => break,
                    }
                    k += 1;
                }
                if k >= ps.len() {
                    // font: menu / caption... (fontes do sistema)
                    return p.is_some();
                }
                let tam_tokens = ps[k];
                let barra = tam_tokens.iter().position(|x| *x == Token::Delim('/'));
                let (ft, lh) = match barra {
                    Some(b) => (&tam_tokens[..b], Some(&tam_tokens[b + 1..])),
                    None => (tam_tokens, None),
                };
                self.tamanho_fonte = ok!(tamanho_fonte(ft, c));
                let mut resto_inicio = k + 1;
                let lh = match lh {
                    Some(l) if !l.is_empty() => Some(l),
                    Some(_) => {
                        resto_inicio += 1;
                        ps.get(k + 1).copied()
                    }
                    None if ps.get(k + 1).is_some_and(|x| x.first() == Some(&Token::Delim('/'))) => {
                        resto_inicio += 1;
                        let x = ps[k + 1];
                        if x.len() > 1 { Some(&x[1..]) } else {
                            resto_inicio += 1;
                            ps.get(k + 2).copied()
                        }
                    }
                    None => None,
                };
                self.altura_linha = match lh {
                    Some(l) => ok!(altura_linha(l, c, self.tamanho_fonte)),
                    None => AlturaLinha::Normal,
                };
                if let Some(primeiro) = ps.get(resto_inicio) {
                    let inicio = primeiro.as_ptr() as usize - t.as_ptr() as usize;
                    let inicio = inicio / core::mem::size_of::<Token>();
                    self.familia = familia(&t[inicio..]);
                }
            }
            "line-height" => self.altura_linha = ok!(altura_linha(t, c, self.tamanho_fonte)),
            "text-align" => {
                self.alinhamento = match p {
                    Some("left") | Some("start") | Some("-webkit-left") => Alinhamento::Esquerda,
                    Some("right") | Some("end") | Some("-webkit-right") => Alinhamento::Direita,
                    Some("center") | Some("-webkit-center") | Some("-moz-center") => Alinhamento::Centro,
                    Some("justify") => Alinhamento::Justificado,
                    _ => return false,
                }
            }
            "white-space" | "white-space-collapse" | "text-wrap-mode" => {
                self.brancos = match p {
                    Some("normal") | Some("collapse") | Some("wrap") => Brancos::Normal,
                    Some("pre") | Some("preserve") => Brancos::Pre,
                    Some("nowrap") => Brancos::SemQuebra,
                    Some("pre-wrap") | Some("break-spaces") => Brancos::PreQuebra,
                    Some("pre-line") | Some("preserve-breaks") => Brancos::PreLinha,
                    _ => return false,
                }
            }
            "text-wrap" => {
                if p == Some("nowrap") {
                    self.brancos = Brancos::SemQuebra;
                }
            }
            "word-break" => self.quebra_palavra = p == Some("break-all") || p == Some("break-word"),
            "overflow-wrap" | "word-wrap" => self.quebra_palavra = p == Some("break-word") || p == Some("anywhere"),
            "text-transform" => {
                self.transformacao = match p {
                    Some("uppercase") => Transformacao::Maiusculas,
                    Some("lowercase") => Transformacao::Minusculas,
                    Some("capitalize") => Transformacao::Capitalizar,
                    Some("none") => Transformacao::Nenhuma,
                    _ => return false,
                }
            }
            "letter-spacing" => self.espaco_letras = if p == Some("normal") { 0 } else { ok!(medida(t, c)).px },
            "word-spacing" => self.espaco_palavras = if p == Some("normal") { 0 } else { ok!(medida(t, c)).px },
            "text-indent" => self.recuo = ok!(medida(partes(t)[0], c)),
            "text-decoration" | "text-decoration-line" | "-webkit-text-decoration-line" => {
                let mut d = 0;
                for x in partes(t) {
                    match ident(x).as_deref() {
                        Some("underline") => d |= 1,
                        Some("overline") => d |= 2,
                        Some("line-through") => d |= 4,
                        Some("none") => d = 0,
                        _ => {
                            if nome == "text-decoration" {
                                if let Some(Some(co)) = cor(x, self.cor) {
                                    self.cor_decoracao = Some(co);
                                }
                            }
                        }
                    }
                }
                self.decoracao = d;
            }
            "text-decoration-color" => self.cor_decoracao = ok!(cor(t, self.cor)),
            "vertical-align" => {
                self.alinhamento_vertical = match p {
                    Some("baseline") => AlinhamentoVertical::Base,
                    Some("top") => AlinhamentoVertical::Topo,
                    Some("middle") => AlinhamentoVertical::Meio,
                    Some("bottom") => AlinhamentoVertical::Baixo,
                    Some("sub") => AlinhamentoVertical::Sub,
                    Some("super") => AlinhamentoVertical::Super,
                    Some("text-top") => AlinhamentoVertical::TextoTopo,
                    Some("text-bottom") => AlinhamentoVertical::TextoBaixo,
                    _ => AlinhamentoVertical::M(ok!(medida(t, c))),
                }
            }
            "list-style-type" => self.tipo_lista = tipo_lista(p.unwrap_or("")).unwrap_or(TipoLista::Disco),
            "list-style-position" => self.lista_dentro = p == Some("inside"),
            "list-style-image" => {
                let mut l = Leitor::novo(t);
                self.imagem_lista = ler_imagem(&mut l, Some(self.cor)).map(Rc::new);
            }
            "list-style" => {
                for x in partes(t) {
                    match ident(x).as_deref() {
                        Some("inside") => self.lista_dentro = true,
                        Some("outside") => self.lista_dentro = false,
                        Some(s) => {
                            if let Some(tl) = tipo_lista(s) {
                                self.tipo_lista = tl;
                            }
                        }
                        None => {
                            let mut l = Leitor::novo(x);
                            self.imagem_lista = ler_imagem(&mut l, Some(self.cor)).map(Rc::new);
                        }
                    }
                }
            }
            "cursor" => self.cursor_mao = p == Some("pointer"),
            "background-color" => self.fundo = ok!(cor(t, self.cor)).unwrap_or(self.cor),
            "background" => {
                // so a ultima camada tem cor; a imagem vem da primeira
                let camadas: Vec<&[Token]> = dividir_virgulas(t);
                self.fundo = Cor::TRANSPARENTE;
                self.imagem_fundo = None;
                self.repetir_fundo = Repetir::Repetir;
                self.posicao_fundo = (Medida::ZERO, Medida::ZERO);
                self.tamanho_fundo = TamanhoFundo::Auto;
                for (k, camada) in camadas.iter().enumerate() {
                    let ultima = k + 1 == camadas.len();
                    let mut posicoes: Vec<&[Token]> = Vec::new();
                    let ps = partes(camada);
                    let mut i = 0;
                    while i < ps.len() {
                        let x = ps[i];
                        if x == [Token::Delim('/')] {
                            // tamanho depois da barra
                            if let Some(tf) = ps.get(i + 1).and_then(|y| tamanho_fundo(&[*y], c)) {
                                self.tamanho_fundo = tf;
                            }
                            i += 2;
                            continue;
                        }
                        match ident(x).as_deref() {
                            Some("no-repeat") => self.repetir_fundo = Repetir::Nao,
                            Some("repeat-x") => self.repetir_fundo = Repetir::X,
                            Some("repeat-y") => self.repetir_fundo = Repetir::Y,
                            Some("repeat") | Some("round") | Some("space") => self.repetir_fundo = Repetir::Repetir,
                            Some("none") | Some("scroll") | Some("fixed") | Some("local") | Some("border-box")
                            | Some("padding-box") | Some("content-box") | Some("text") => {}
                            Some("left") | Some("right") | Some("top") | Some("bottom") | Some("center") => posicoes.push(x),
                            _ => {
                                let mut l = Leitor::novo(x);
                                if let Some(img) = ler_imagem(&mut l, Some(self.cor)) {
                                    if self.imagem_fundo.is_none() {
                                        self.imagem_fundo = Some(Rc::new(img));
                                    }
                                } else if ultima && cor(x, self.cor).is_some() {
                                    self.fundo = cor(x, self.cor).flatten().unwrap_or(self.cor);
                                } else if medida(x, c).is_some() {
                                    posicoes.push(x);
                                }
                            }
                        }
                        i += 1;
                    }
                    if !posicoes.is_empty() {
                        self.posicao_fundo = posicao_fundo(&posicoes, c);
                    }
                }
            }
            "background-image" => {
                let mut l = Leitor::novo(t);
                self.imagem_fundo = ler_imagem(&mut l, Some(self.cor)).map(Rc::new);
            }
            "background-repeat" => {
                self.repetir_fundo = match p {
                    Some("no-repeat") => Repetir::Nao,
                    Some("repeat-x") => Repetir::X,
                    Some("repeat-y") => Repetir::Y,
                    _ => Repetir::Repetir,
                }
            }
            "background-position" => {
                let ps = partes(dividir_virgulas(t)[0]);
                self.posicao_fundo = posicao_fundo(&ps, c);
            }
            "background-size" => self.tamanho_fundo = ok!(tamanho_fundo(&partes(t), c)),
            "flex-direction" | "-webkit-flex-direction" => {
                self.direcao_flex = match p {
                    Some("row") => DirecaoFlex::Linha,
                    Some("row-reverse") => DirecaoFlex::LinhaInversa,
                    Some("column") => DirecaoFlex::Coluna,
                    Some("column-reverse") => DirecaoFlex::ColunaInversa,
                    _ => return false,
                }
            }
            "flex-wrap" | "-webkit-flex-wrap" => {
                self.quebra_flex = p != Some("nowrap");
                self.quebra_flex_inversa = p == Some("wrap-reverse");
            }
            "flex-flow" => {
                for x in partes(t) {
                    let s = ident(x).unwrap_or_default();
                    match s.as_str() {
                        "row" | "row-reverse" | "column" | "column-reverse" => {
                            self.aplicar("flex-direction", x, c);
                        }
                        _ => {
                            self.aplicar("flex-wrap", x, c);
                        }
                    }
                }
            }
            "justify-content" | "-webkit-justify-content" => self.justificar = ok!(alinhar_tokens(t)),
            "align-items" | "-webkit-align-items" => self.alinhar_itens = ok!(alinhar_tokens(t)),
            "align-self" | "-webkit-align-self" => self.alinhar_proprio = ok!(alinhar_tokens(t)),
            "align-content" => self.alinhar_conteudo = ok!(alinhar_tokens(t)),
            "justify-items" => self.justificar_itens = ok!(alinhar_tokens(t)),
            "justify-self" => self.justificar_proprio = ok!(alinhar_tokens(t)),
            "place-items" | "place-content" | "place-self" => {
                let ps = partes(t);
                let a = ok!(alinhar_tokens(ps[0]));
                let b = ps.get(1).and_then(|x| alinhar_tokens(x)).unwrap_or(a);
                match nome {
                    "place-items" => {
                        self.alinhar_itens = a;
                        self.justificar_itens = b;
                    }
                    "place-content" => {
                        self.alinhar_conteudo = a;
                        self.justificar = b;
                    }
                    _ => {
                        self.alinhar_proprio = a;
                        self.justificar_proprio = b;
                    }
                }
            }
            "flex-grow" | "-webkit-flex-grow" => self.crescer = ok!(Leitor::novo(t).numero()).max(0.0),
            "flex-shrink" | "-webkit-flex-shrink" => self.encolher = ok!(Leitor::novo(t).numero()).max(0.0),
            "flex-basis" | "-webkit-flex-basis" => {
                self.base_flex = if p == Some("content") { Tam::MaxConteudo } else { ok!(tam(t, c, false)) }
            }
            "flex" | "-webkit-flex" | "-ms-flex" => match p {
                Some("none") => {
                    self.crescer = 0.0;
                    self.encolher = 0.0;
                    self.base_flex = Tam::Auto;
                }
                Some("auto") => {
                    self.crescer = 1.0;
                    self.encolher = 1.0;
                    self.base_flex = Tam::Auto;
                }
                Some("initial") => {
                    self.crescer = 0.0;
                    self.encolher = 1.0;
                    self.base_flex = Tam::Auto;
                }
                _ => {
                    let mut nums = Vec::new();
                    let mut base = None;
                    for x in partes(t) {
                        match x {
                            [Token::Numero(v, _)] => nums.push(*v),
                            _ => base = Some(ok!(tam(x, c, false))),
                        }
                    }
                    self.crescer = nums.first().copied().unwrap_or(1.0);
                    self.encolher = nums.get(1).copied().unwrap_or(1.0);
                    self.base_flex = base.unwrap_or(Tam::ZERO);
                }
            },
            "order" => self.ordem = ok!(Leitor::novo(t).numero()) as i32,
            "gap" | "grid-gap" => {
                let ps = partes(t);
                let a = if ident(ps[0]).as_deref() == Some("normal") { Medida::ZERO } else { ok!(medida(ps[0], c)) };
                let b = match ps.get(1) {
                    Some(x) => ok!(medida(x, c)),
                    None => a,
                };
                self.lacuna = (a, b);
            }
            "row-gap" | "grid-row-gap" => self.lacuna.0 = if p == Some("normal") { Medida::ZERO } else { ok!(medida(t, c)) },
            "column-gap" | "grid-column-gap" => {
                self.lacuna.1 = if p == Some("normal") { Medida::ZERO } else { ok!(medida(t, c)) }
            }
            "grid-template-columns" => self.colunas_grade = Rc::new(if p == Some("none") { Vec::new() } else { ok!(faixas(t, c)) }),
            "grid-template-rows" => self.linhas_grade = Rc::new(if p == Some("none") { Vec::new() } else { ok!(faixas(t, c)) }),
            "grid-auto-columns" => self.auto_colunas = Rc::new(ok!(faixas(t, c))),
            "grid-auto-rows" => self.auto_linhas = Rc::new(ok!(faixas(t, c))),
            "grid-auto-flow" => {
                let ps: Vec<String> = partes(t).iter().filter_map(|x| ident(x)).collect();
                self.fluxo_coluna = ps.iter().any(|x| x == "column");
                self.fluxo_denso = ps.iter().any(|x| x == "dense");
            }
            "grid-template-areas" => {
                let mut linhas = Vec::new();
                for x in t {
                    if let Token::Texto(s) = x {
                        linhas.push(s.split_ascii_whitespace().map(|n| if n.starts_with('.') { None } else { Some(Atomo::de(n)) }).collect());
                    }
                }
                self.areas_grade = Rc::new(linhas);
            }
            "grid-template" | "grid" => {
                // "linhas / colunas" (sem areas nomeadas nessa forma)
                let mut it = t.split(|x| *x == Token::Delim('/'));
                let linhas = apara(it.next().unwrap_or(&[]));
                if linhas.iter().any(|x| matches!(x, Token::Texto(_))) {
                    self.aplicar("grid-template-areas", linhas, c);
                } else if let Some(f) = faixas(linhas, c) {
                    self.linhas_grade = Rc::new(f);
                }
                if let Some(col) = it.next() {
                    if let Some(f) = faixas(apara(col), c) {
                        self.colunas_grade = Rc::new(f);
                    }
                }
            }
            "grid-column" => self.grade_coluna = ok!(par_linhas(t)),
            "grid-row" => self.grade_linha = ok!(par_linhas(t)),
            "grid-column-start" => self.grade_coluna.0 = ok!(linha_grade(t)),
            "grid-column-end" => self.grade_coluna.1 = ok!(linha_grade(t)),
            "grid-row-start" => self.grade_linha.0 = ok!(linha_grade(t)),
            "grid-row-end" => self.grade_linha.1 = ok!(linha_grade(t)),
            "grid-area" => {
                // nomes de area diferenciam maiusculas: o token cru
                if let [Token::Ident(s)] = t {
                    self.area = Some(Atomo::de(s));
                } else {
                    let ps: Vec<&[Token]> = t.split(|x| *x == Token::Delim('/')).collect();
                    let l = |k: usize| ps.get(k).and_then(|x| linha_grade(apara(x))).unwrap_or(LinhaGrade::Auto);
                    self.grade_linha = (l(0), l(2));
                    self.grade_coluna = (l(1), l(3));
                }
            }
            "content" => {
                if matches!(p, Some("none") | Some("normal")) {
                    self.conteudo = None;
                } else {
                    let mut v = Vec::new();
                    let mut l = Leitor::novo(t);
                    while let Some(x) = l.proximo() {
                        match x {
                            Token::Texto(s) => v.push(ParteConteudo::Texto(s.clone())),
                            Token::Funcao(f) if f == "attr" => {
                                let args = l.argumentos().unwrap_or_default();
                                if let Some(Token::Ident(n)) = args.iter().find(|a| matches!(a, Token::Ident(_))) {
                                    v.push(ParteConteudo::Atributo(Atomo::minusculo(n)));
                                }
                            }
                            Token::Funcao(f) if f == "counter" || f == "counters" => {
                                let args = l.argumentos().unwrap_or_default();
                                let ids: Vec<&String> = args.iter().filter_map(|a| if let Token::Ident(n) = a { Some(n) } else { None }).collect();
                                if let Some(n) = ids.first() {
                                    let tl = ids.get(1).and_then(|s| tipo_lista(s)).unwrap_or(TipoLista::Decimal);
                                    v.push(ParteConteudo::Contador(Atomo::de(n), tl));
                                }
                            }
                            Token::Funcao(_) => {
                                l.argumentos();
                            }
                            Token::Ident(s) if s == "open-quote" => v.push(ParteConteudo::AbreAspas),
                            Token::Ident(s) if s == "close-quote" => v.push(ParteConteudo::FechaAspas),
                            Token::Url(_) | Token::Delim('/') => break, // imagem / texto alternativo
                            _ => {}
                        }
                    }
                    self.conteudo = Some(Rc::new(v));
                }
            }
            "counter-reset" | "counter-increment" | "counter-set" => {
                let mut l = Leitor::novo(t);
                let par = match l.proximo() {
                    Some(Token::Ident(n)) if n != "none" => {
                        let v = l.numero().map_or(if nome == "counter-increment" { 1 } else { 0 }, |x| x as i32);
                        Some((Atomo::de(n), v))
                    }
                    _ => None,
                };
                if nome == "counter-increment" {
                    self.incrementa_contador = par;
                } else {
                    self.zera_contador = par;
                }
            }
            "text-overflow" => self.reticencias = p == Some("ellipsis"),
            "object-fit" => {
                self.ajuste_objeto = match p {
                    Some("contain") => Ajuste::Conter,
                    Some("cover") => Ajuste::Cobrir,
                    Some("none") => Ajuste::Nenhum,
                    Some("scale-down") => Ajuste::Reduzir,
                    _ => Ajuste::Encher,
                }
            }
            "box-shadow" => {
                if p == Some("none") {
                    self.sombra = None;
                } else {
                    let primeira = dividir_virgulas(t)[0];
                    let mut nums = Vec::new();
                    let mut co = Cor::rgba(0, 0, 0, 80);
                    let mut dentro = false;
                    for x in partes(primeira) {
                        if ident(x).as_deref() == Some("inset") {
                            dentro = true;
                        } else if let Some(m) = medida(x, c) {
                            nums.push(m.px);
                        } else if let Some(Some(k)) = cor(x, self.cor) {
                            co = k;
                        }
                    }
                    self.sombra = if dentro || nums.len() < 2 {
                        None
                    } else {
                        Some(Sombra { x: nums[0], y: nums[1], borrao: nums.get(2).copied().unwrap_or(0), cor: co })
                    };
                }
            }
            "transform" => {
                self.deslocar = (Medida::ZERO, Medida::ZERO);
                self.escala = 1.0;
                let mut l = Leitor::novo(t);
                while let Some(x) = l.proximo() {
                    if let Token::Funcao(f) = x {
                        let f = f.clone();
                        let args = l.argumentos().unwrap_or_default();
                        let vals: Vec<&[Token]> = dividir_virgulas(&args).into_iter().map(apara).collect();
                        let m = |k: usize| vals.get(k).and_then(|v| medida(v, c)).unwrap_or(Medida::ZERO);
                        match f.as_str() {
                            "translate" => self.deslocar = (m(0), m(1)),
                            "translatex" => self.deslocar.0 = m(0),
                            "translatey" => self.deslocar.1 = m(0),
                            "translate3d" => self.deslocar = (m(0), m(1)),
                            "scale" => self.escala = vals.first().and_then(|v| Leitor::novo(v).numero()).unwrap_or(1.0),
                            _ => {}
                        }
                    }
                }
            }
            "table-layout" => self.layout_fixo_tabela = p == Some("fixed"),
            _ => return false,
        }
        true
    }
}

/// Indices (top, right, bottom, left) de "border-top", "border-inline"...
fn lados(nome: &str) -> &'static [usize] {
    match nome {
        "border-top" | "border-block-start" => &[0],
        "border-right" | "border-inline-end" => &[1],
        "border-bottom" | "border-block-end" => &[2],
        "border-left" | "border-inline-start" => &[3],
        "border-block" => &[0, 2],
        "border-inline" => &[1, 3],
        _ => &[0, 1, 2, 3],
    }
}

fn tamanho_fundo(ps: &[&[Token]], c: &Calculo) -> Option<TamanhoFundo> {
    match ps.first().and_then(|x| ident(x)).as_deref() {
        Some("cover") => return Some(TamanhoFundo::Cobrir),
        Some("contain") => return Some(TamanhoFundo::Conter),
        _ => {}
    }
    let m = |x: &[Token]| if ident(x).as_deref() == Some("auto") { None } else { medida(x, c) };
    let a = ps.first().and_then(|x| m(x));
    let b = ps.get(1).and_then(|x| m(x));
    match a {
        Some(a) => Some(TamanhoFundo::M(a, b)),
        None => Some(TamanhoFundo::Auto),
    }
}

fn posicao_fundo(ps: &[&[Token]], c: &Calculo) -> (Medida, Medida) {
    let pct = |v: f32| Medida { px: 0, pct: fixo(v) };
    let mut x = pct(0.0);
    let mut y = pct(0.0);
    let mut eixo_x = true;
    let mut definido_x = false;
    for p in ps {
        match ident(p).as_deref() {
            Some("left") => {
                x = pct(0.0);
                definido_x = true;
            }
            Some("right") => {
                x = pct(100.0);
                definido_x = true;
            }
            Some("top") => {
                y = pct(0.0);
                eixo_x = true;
            }
            Some("bottom") => {
                y = pct(100.0);
                eixo_x = true;
            }
            Some("center") => {
                if !definido_x && eixo_x {
                    x = pct(50.0);
                    definido_x = true;
                } else {
                    y = pct(50.0);
                }
            }
            _ => {
                if let Some(m) = medida(p, c) {
                    if !definido_x {
                        x = m;
                        definido_x = true;
                    } else {
                        y = m;
                    }
                }
            }
        }
    }
    if ps.len() == 1 && definido_x && ident(ps[0]).as_deref() != Some("top") && ident(ps[0]).as_deref() != Some("bottom") {
        y = pct(50.0);
    }
    (x, y)
}

/// Propriedades herdadas (para "inherit" e "unset" saberem o que fazer).
pub fn eh_herdada(nome: &str) -> bool {
    matches!(
        nome,
        "color" | "font" | "font-family" | "font-size" | "font-weight" | "font-style" | "line-height" | "text-align"
            | "white-space" | "word-break" | "overflow-wrap" | "word-wrap" | "text-transform" | "letter-spacing"
            | "word-spacing" | "text-indent" | "list-style" | "list-style-type" | "list-style-position"
            | "list-style-image" | "visibility" | "cursor" | "border-collapse" | "border-spacing" | "quotes"
    )
}

/// Copia do pai os campos de uma propriedade (inherit).
pub fn herdar_prop(e: &mut Estilo, pai: &Estilo, nome: &str) {
    match nome {
        "color" => e.cor = pai.cor,
        "font-family" => e.familia = pai.familia,
        "font-size" => e.tamanho_fonte = pai.tamanho_fonte,
        "font-weight" => e.peso = pai.peso,
        "font-style" => e.italico = pai.italico,
        "font" => {
            e.familia = pai.familia;
            e.tamanho_fonte = pai.tamanho_fonte;
            e.peso = pai.peso;
            e.italico = pai.italico;
            e.altura_linha = pai.altura_linha;
        }
        "line-height" => e.altura_linha = pai.altura_linha,
        "text-align" => e.alinhamento = pai.alinhamento,
        "white-space" => e.brancos = pai.brancos,
        "display" => e.exibir = pai.exibir,
        "position" => e.posicao = pai.posicao,
        "width" => e.largura = pai.largura,
        "height" => e.altura = pai.altura,
        "margin" => e.margem = pai.margem,
        "padding" => e.preenchimento = pai.preenchimento,
        "background-color" => e.fundo = pai.fundo,
        "background" => {
            e.fundo = pai.fundo;
            e.imagem_fundo = pai.imagem_fundo.clone();
        }
        "border-color" => e.borda_cor = pai.borda_cor,
        "visibility" => e.visivel = pai.visivel,
        "text-decoration" => e.decoracao = pai.decoracao,
        "overflow" => e.transbordo = pai.transbordo,
        "opacity" => e.opacidade = pai.opacidade,
        "box-sizing" => e.caixa_borda = pai.caixa_borda,
        "vertical-align" => e.alinhamento_vertical = pai.alinhamento_vertical,
        _ => {}
    }
}

/// Volta uma propriedade ao valor inicial (initial / unset nao herdada).
pub fn inicial_prop(e: &mut Estilo, nome: &str) {
    let i = Estilo::inicial();
    match nome {
        "color" => e.cor = i.cor,
        "display" => e.exibir = i.exibir,
        "width" => e.largura = i.largura,
        "height" => e.altura = i.altura,
        "max-width" => e.max_largura = i.max_largura,
        "margin" => e.margem = i.margem,
        "padding" => e.preenchimento = i.preenchimento,
        "background" | "background-color" => {
            e.fundo = i.fundo;
            e.imagem_fundo = None;
        }
        "border" => {
            e.borda_estilo = i.borda_estilo;
            e.borda_largura = i.borda_largura;
        }
        "font-size" => e.tamanho_fonte = i.tamanho_fonte,
        "font-weight" => e.peso = i.peso,
        "line-height" => e.altura_linha = i.altura_linha,
        "text-decoration" => e.decoracao = 0,
        "position" => e.posicao = i.posicao,
        "float" => e.flutuar = i.flutuar,
        "box-shadow" => e.sombra = None,
        "transform" => {
            e.deslocar = i.deslocar;
            e.escala = 1.0;
        }
        _ => {}
    }
}

pub fn eh_zero_tokens(t: &[Token]) -> bool {
    let mut l = Leitor::novo(t);
    ler_comprimento(&mut l, false).is_some_and(|c| eh_zero(&c))
}

#[allow(unused)]
fn _usar(_: Linear, _: Comprimento) {}
