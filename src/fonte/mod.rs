//! Fontes do MXOS: Inter na interface e JetBrains Mono no terminal.
//!
//! Os arquivos .mxf sao gerados no PC por ferramentas/gerar_fontes.py: cada
//! caractere ja vem desenhado, com antialiasing, como uma grade de alfa
//! (0 = transparente, 255 = cheio). O kernel so mistura a cor do texto com
//! o fundo; nao ha rasterizador de curvas aqui dentro.
//!
//! Cobertura: ASCII, Latin-1 (todos os acentos do portugues) e alguns
//! simbolos (aspas curvas, travessao, reticencias, setas). O resto vira '?'.
//! Inter e JetBrains Mono: SIL Open Font License (ferramentas/fontes/).

// Na TV box (sem area de trabalho) so o terminal usa as fontes.
#![cfg_attr(feature = "mxq", allow(dead_code))]

/// Cabecalho: "MXF1" + 4 bytes de metricas + quantidade + reservado.
const TAMANHO_CABECALHO: usize = 12;
const TAMANHO_ENTRADA: usize = 16;

/// Uma fonte pre-renderizada, lida direto dos bytes embutidos no kernel.
pub struct Fonte {
    dados: &'static [u8],
    quantidade: usize,
    /// Distancia entre duas linhas de texto.
    pub altura_linha: i32,
    /// Da linha de cima ate a linha de base.
    pub ascendente: i32,
    /// Altura de uma maiuscula sem acento (o 'H'), para centralizar.
    pub capitular: i32,
}

/// Um caractere pronto para desenhar.
pub struct Glifo {
    pub largura: i32,
    pub altura: i32,
    /// Onde o bitmap comeca em relacao a caneta (x) e a linha de base (y).
    pub dx: i32,
    pub dy: i32,
    /// Quanto a caneta anda depois deste caractere.
    pub avanco: i32,
    alfa: &'static [u8],
}

impl Glifo {
    pub fn alfa(&self, x: i32, y: i32) -> u8 {
        self.alfa[(y * self.largura + x) as usize]
    }
}

impl Fonte {
    const fn nova(dados: &'static [u8]) -> Fonte {
        assert!(dados[0] == b'M' && dados[1] == b'X' && dados[2] == b'F' && dados[3] == b'1');
        Fonte {
            dados,
            quantidade: dados[8] as usize | (dados[9] as usize) << 8,
            altura_linha: dados[4] as i32,
            ascendente: dados[5] as i32,
            capitular: dados[7] as i32,
        }
    }

    fn u32_em(&self, i: usize) -> u32 {
        // byte a byte: os dados embutidos nao tem alinhamento garantido
        u32::from_le_bytes([self.dados[i], self.dados[i + 1], self.dados[i + 2], self.dados[i + 3]])
    }

    fn entrada(&self, i: usize) -> usize {
        TAMANHO_CABECALHO + i * TAMANHO_ENTRADA
    }

    /// Busca binaria pelo codigo (as entradas vem ordenadas).
    fn indice(&self, c: char) -> Option<usize> {
        let (mut baixo, mut alto) = (0, self.quantidade);
        while baixo < alto {
            let meio = (baixo + alto) / 2;
            let codigo = self.u32_em(self.entrada(meio));
            if codigo == c as u32 {
                return Some(meio);
            }
            if codigo < c as u32 {
                baixo = meio + 1;
            } else {
                alto = meio;
            }
        }
        None
    }

    /// O glifo de `c`; sem ele na fonte, o do '?'.
    pub fn glifo(&self, c: char) -> Glifo {
        let i = self.indice(c).or_else(|| self.indice('?')).unwrap_or(0);
        let e = self.entrada(i);
        let inicio_alfa = self.entrada(self.quantidade) + self.u32_em(e + 4) as usize;
        let (largura, altura) = (self.dados[e + 8] as i32, self.dados[e + 9] as i32);
        Glifo {
            largura,
            altura,
            dx: self.dados[e + 10] as i8 as i32,
            dy: self.dados[e + 11] as i8 as i32,
            avanco: self.dados[e + 12] as i32,
            alfa: &self.dados[inicio_alfa..inicio_alfa + (largura * altura) as usize],
        }
    }

    /// Largura do texto em pixels.
    pub fn largura(&self, texto: &str) -> i32 {
        texto.chars().map(|c| self.glifo(c).avanco).sum()
    }

    /// Linha de base que deixa as maiusculas centralizadas numa faixa de
    /// `altura` pixels comecando em `topo`.
    pub fn base_centralizada(&self, topo: i32, altura: i32) -> i32 {
        topo + (altura + self.capitular) / 2
    }
}

pub static INTER: Fonte = Fonte::nova(include_bytes!("inter_13.mxf"));
pub static INTER_NEGRITO: Fonte = Fonte::nova(include_bytes!("inter_13_negrito.mxf"));
pub static INTER_TITULO: Fonte = Fonte::nova(include_bytes!("inter_20.mxf"));
pub static INTER_GRANDE: Fonte = Fonte::nova(include_bytes!("inter_32.mxf"));
/// Monoespacada: toda letra anda 8 px; a celula do terminal e 8x16.
pub static MONO: Fonte = Fonte::nova(include_bytes!("mono_16.mxf"));
pub const MONO_LARGURA: i32 = 8;
pub const MONO_ALTURA: i32 = 16;
/// Linha de base dentro da celula de 16 px do terminal.
pub const MONO_BASE: i32 = 12;

/// A fonte que substitui a antiga 8x8 em cada escala: 1 = texto normal,
/// 2 = titulos, 3 ou mais = letras grandes.
pub fn por_escala(escala: usize) -> &'static Fonte {
    match escala {
        0 | 1 => &INTER,
        2 => &INTER_TITULO,
        _ => &INTER_GRANDE,
    }
}

/// Onde o texto pode ser desenhado (uma Superficie, o framebuffer...).
pub trait Alvo {
    fn tamanho(&self) -> (i32, i32);
    fn ler(&self, x: i32, y: i32) -> u32;
    fn escrever(&mut self, x: i32, y: i32, cor: u32);
}

/// Mistura `frente` sobre `fundo` com cobertura `alfa` (0..=255), por canal.
pub fn misturar(fundo: u32, frente: u32, alfa: u8) -> u32 {
    let a = alfa as u32;
    let canal = |deslocamento: u32| {
        let f = (fundo >> deslocamento) & 0xFF;
        let t = (frente >> deslocamento) & 0xFF;
        ((t * a + f * (255 - a) + 127) / 255) << deslocamento
    };
    canal(16) | canal(8) | canal(0)
}

/// Desenha um glifo com a caneta em `x` e a linha de base em `base`.
pub fn desenhar_glifo(alvo: &mut impl Alvo, x: i32, base: i32, glifo: &Glifo, cor: u32) {
    let (largura, altura) = alvo.tamanho();
    let (x0, y0) = (x + glifo.dx, base + glifo.dy);
    for gy in 0..glifo.altura {
        let y = y0 + gy;
        if y < 0 || y >= altura {
            continue;
        }
        for gx in 0..glifo.largura {
            let px = x0 + gx;
            let a = glifo.alfa(gx, gy);
            if a == 0 || px < 0 || px >= largura {
                continue;
            }
            let pintar = if a == 255 { cor } else { misturar(alvo.ler(px, y), cor, a) };
            alvo.escrever(px, y, pintar);
        }
    }
}

/// Desenha o texto (UTF-8) com a linha de base em `base`. Devolve a largura.
pub fn desenhar(alvo: &mut impl Alvo, x: i32, base: i32, texto: &str, cor: u32, fonte: &Fonte) -> i32 {
    let largura_alvo = alvo.tamanho().0;
    let mut caneta = x;
    for c in texto.chars() {
        let glifo = fonte.glifo(c);
        if caneta >= largura_alvo {
            break;
        }
        if caneta + glifo.avanco > 0 {
            desenhar_glifo(alvo, caneta, base, &glifo, cor);
        }
        caneta += glifo.avanco;
    }
    caneta - x
}

/// Corta o texto para caber em `largura` pixels (inteiro, por caractere).
pub fn cortar<'a>(texto: &'a str, largura: i32, fonte: &Fonte) -> &'a str {
    let mut usado = 0;
    for (i, c) in texto.char_indices() {
        usado += fonte.glifo(c).avanco;
        if usado > largura {
            return &texto[..i];
        }
    }
    texto
}

/// Como `cortar`, mas quando nao cabe termina em reticencias ("Configura…").
pub fn encurtar(texto: &str, largura: i32, fonte: &Fonte) -> alloc::string::String {
    let mut saida = alloc::string::String::new();
    if fonte.largura(texto) <= largura {
        saida.push_str(texto);
        return saida;
    }
    let resto = largura - fonte.glifo('…').avanco;
    saida.push_str(cortar(texto, resto, fonte).trim_end());
    saida.push('…');
    saida
}

/// Byte de `texto` mais perto do pixel `x` (0 = comeco do texto): para
/// por o cursor onde o mouse clicou. Sempre numa fronteira de caractere.
pub fn indice_no_x(texto: &str, x: i32, fonte: &Fonte) -> usize {
    let mut caneta = 0;
    for (i, c) in texto.char_indices() {
        let avanco = fonte.glifo(c).avanco;
        if x < caneta + avanco / 2 {
            return i;
        }
        caneta += avanco;
    }
    texto.len()
}
