//! Valores do CSS: cores, medidas (com calc/min/max/clamp), numeros e um
//! leitor de tokens que pula espacos. O layout trabalha em ponto fixo
//! (1/64 de pixel): `Fixo`.

#[allow(unused_imports)]
use crate::web::mat::F32Ext;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use super::token::Token;

/// 1/64 de pixel.
pub type Fixo = i32;
pub const UM: Fixo = 64;

pub fn fixo(v: f32) -> Fixo {
    let x = v * 64.0;
    if x >= i32::MAX as f32 {
        i32::MAX / 2
    } else if x <= i32::MIN as f32 {
        i32::MIN / 2
    } else if x >= 0.0 {
        (x + 0.5) as i32
    } else {
        (x - 0.5) as i32
    }
}

// ------------------------------------------------------------------ cores

/// 0xAARRGGBB
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Cor(pub u32);

impl Cor {
    pub const TRANSPARENTE: Cor = Cor(0);
    pub const PRETO: Cor = Cor(0xFF00_0000);
    pub const BRANCO: Cor = Cor(0xFFFF_FFFF);

    pub fn rgba(r: u8, g: u8, b: u8, a: u8) -> Cor {
        Cor(((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | b as u32)
    }
    pub fn a(self) -> u8 {
        (self.0 >> 24) as u8
    }
    pub fn r(self) -> u8 {
        (self.0 >> 16) as u8
    }
    pub fn g(self) -> u8 {
        (self.0 >> 8) as u8
    }
    pub fn b(self) -> u8 {
        self.0 as u8
    }
    pub fn com_alfa(self, a: u8) -> Cor {
        Cor((self.0 & 0x00FF_FFFF) | ((a as u32) << 24))
    }
    pub fn visivel(self) -> bool {
        self.a() != 0
    }
}

static NOMES: &[(&str, u32)] = &[
    ("aliceblue", 0xf0f8ff), ("antiquewhite", 0xfaebd7), ("aqua", 0x00ffff), ("aquamarine", 0x7fffd4),
    ("azure", 0xf0ffff), ("beige", 0xf5f5dc), ("bisque", 0xffe4c4), ("black", 0x000000),
    ("blanchedalmond", 0xffebcd), ("blue", 0x0000ff), ("blueviolet", 0x8a2be2), ("brown", 0xa52a2a),
    ("burlywood", 0xdeb887), ("cadetblue", 0x5f9ea0), ("chartreuse", 0x7fff00), ("chocolate", 0xd2691e),
    ("coral", 0xff7f50), ("cornflowerblue", 0x6495ed), ("cornsilk", 0xfff8dc), ("crimson", 0xdc143c),
    ("cyan", 0x00ffff), ("darkblue", 0x00008b), ("darkcyan", 0x008b8b), ("darkgoldenrod", 0xb8860b),
    ("darkgray", 0xa9a9a9), ("darkgreen", 0x006400), ("darkgrey", 0xa9a9a9), ("darkkhaki", 0xbdb76b),
    ("darkmagenta", 0x8b008b), ("darkolivegreen", 0x556b2f), ("darkorange", 0xff8c00), ("darkorchid", 0x9932cc),
    ("darkred", 0x8b0000), ("darksalmon", 0xe9967a), ("darkseagreen", 0x8fbc8f), ("darkslateblue", 0x483d8b),
    ("darkslategray", 0x2f4f4f), ("darkslategrey", 0x2f4f4f), ("darkturquoise", 0x00ced1),
    ("darkviolet", 0x9400d3), ("deeppink", 0xff1493), ("deepskyblue", 0x00bfff), ("dimgray", 0x696969),
    ("dimgrey", 0x696969), ("dodgerblue", 0x1e90ff), ("firebrick", 0xb22222), ("floralwhite", 0xfffaf0),
    ("forestgreen", 0x228b22), ("fuchsia", 0xff00ff), ("gainsboro", 0xdcdcdc), ("ghostwhite", 0xf8f8ff),
    ("gold", 0xffd700), ("goldenrod", 0xdaa520), ("gray", 0x808080), ("green", 0x008000),
    ("greenyellow", 0xadff2f), ("grey", 0x808080), ("honeydew", 0xf0fff0), ("hotpink", 0xff69b4),
    ("indianred", 0xcd5c5c), ("indigo", 0x4b0082), ("ivory", 0xfffff0), ("khaki", 0xf0e68c),
    ("lavender", 0xe6e6fa), ("lavenderblush", 0xfff0f5), ("lawngreen", 0x7cfc00), ("lemonchiffon", 0xfffacd),
    ("lightblue", 0xadd8e6), ("lightcoral", 0xf08080), ("lightcyan", 0xe0ffff),
    ("lightgoldenrodyellow", 0xfafad2), ("lightgray", 0xd3d3d3), ("lightgreen", 0x90ee90),
    ("lightgrey", 0xd3d3d3), ("lightpink", 0xffb6c1), ("lightsalmon", 0xffa07a), ("lightseagreen", 0x20b2aa),
    ("lightskyblue", 0x87cefa), ("lightslategray", 0x778899), ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xb0c4de), ("lightyellow", 0xffffe0), ("lime", 0x00ff00), ("limegreen", 0x32cd32),
    ("linen", 0xfaf0e6), ("magenta", 0xff00ff), ("maroon", 0x800000), ("mediumaquamarine", 0x66cdaa),
    ("mediumblue", 0x0000cd), ("mediumorchid", 0xba55d3), ("mediumpurple", 0x9370db),
    ("mediumseagreen", 0x3cb371), ("mediumslateblue", 0x7b68ee), ("mediumspringgreen", 0x00fa9a),
    ("mediumturquoise", 0x48d1cc), ("mediumvioletred", 0xc71585), ("midnightblue", 0x191970),
    ("mintcream", 0xf5fffa), ("mistyrose", 0xffe4e1), ("moccasin", 0xffe4b5), ("navajowhite", 0xffdead),
    ("navy", 0x000080), ("oldlace", 0xfdf5e6), ("olive", 0x808000), ("olivedrab", 0x6b8e23),
    ("orange", 0xffa500), ("orangered", 0xff4500), ("orchid", 0xda70d6), ("palegoldenrod", 0xeee8aa),
    ("palegreen", 0x98fb98), ("paleturquoise", 0xafeeee), ("palevioletred", 0xdb7093),
    ("papayawhip", 0xffefd5), ("peachpuff", 0xffdab9), ("peru", 0xcd853f), ("pink", 0xffc0cb),
    ("plum", 0xdda0dd), ("powderblue", 0xb0e0e6), ("purple", 0x800080), ("rebeccapurple", 0x663399),
    ("red", 0xff0000), ("rosybrown", 0xbc8f8f), ("royalblue", 0x4169e1), ("saddlebrown", 0x8b4513),
    ("salmon", 0xfa8072), ("sandybrown", 0xf4a460), ("seagreen", 0x2e8b57), ("seashell", 0xfff5ee),
    ("sienna", 0xa0522d), ("silver", 0xc0c0c0), ("skyblue", 0x87ceeb), ("slateblue", 0x6a5acd),
    ("slategray", 0x708090), ("slategrey", 0x708090), ("snow", 0xfffafa), ("springgreen", 0x00ff7f),
    ("steelblue", 0x4682b4), ("tan", 0xd2b48c), ("teal", 0x008080), ("thistle", 0xd8bfd8),
    ("tomato", 0xff6347), ("turquoise", 0x40e0d0), ("violet", 0xee82ee), ("wheat", 0xf5deb3),
    ("white", 0xffffff), ("whitesmoke", 0xf5f5f5), ("yellow", 0xffff00), ("yellowgreen", 0x9acd32),
    // cores do sistema
    ("canvas", 0xffffff), ("canvastext", 0x000000), ("linktext", 0x0000ee), ("visitedtext", 0x551a8b),
    ("activetext", 0xff0000), ("buttonface", 0xefefef), ("buttontext", 0x000000), ("buttonborder", 0x767676),
    ("field", 0xffffff), ("fieldtext", 0x000000), ("graytext", 0x6d6d6d), ("highlight", 0x3390ff),
    ("highlighttext", 0xffffff), ("mark", 0xffff00), ("marktext", 0x000000), ("window", 0xffffff),
    ("windowtext", 0x000000),
];

/// Cor pelo nome (sem "currentcolor", que depende do elemento).
pub fn cor_nomeada(n: &str) -> Option<Cor> {
    if n.eq_ignore_ascii_case("transparent") {
        return Some(Cor::TRANSPARENTE);
    }
    let n = n.to_ascii_lowercase();
    NOMES.iter().find(|(x, _)| *x == n).map(|&(_, v)| Cor(0xFF00_0000 | v))
}

fn hexa(s: &str) -> Option<Cor> {
    let d: Vec<u8> = s.bytes().map(|c| (c as char).to_digit(16).map(|x| x as u8)).collect::<Option<_>>()?;
    let (r, g, b, a) = match d.len() {
        3 => (d[0] * 17, d[1] * 17, d[2] * 17, 255),
        4 => (d[0] * 17, d[1] * 17, d[2] * 17, d[3] * 17),
        6 => (d[0] * 16 + d[1], d[2] * 16 + d[3], d[4] * 16 + d[5], 255),
        8 => (d[0] * 16 + d[1], d[2] * 16 + d[3], d[4] * 16 + d[5], d[6] * 16 + d[7]),
        _ => return None,
    };
    Some(Cor::rgba(r, g, b, a))
}

fn hsl_para_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    let h = ((h % 360.0) + 360.0) % 360.0 / 360.0;
    let t2 = if l <= 0.5 { l * (s + 1.0) } else { l + s - l * s };
    let t1 = l * 2.0 - t2;
    let canal = |mut h: f32| {
        if h < 0.0 {
            h += 1.0;
        }
        if h > 1.0 {
            h -= 1.0;
        }
        if h * 6.0 < 1.0 {
            t1 + (t2 - t1) * h * 6.0
        } else if h * 2.0 < 1.0 {
            t2
        } else if h * 3.0 < 2.0 {
            t1 + (t2 - t1) * (2.0 / 3.0 - h) * 6.0
        } else {
            t1
        }
    };
    (canal(h + 1.0 / 3.0), canal(h), canal(h - 1.0 / 3.0))
}

fn byte(v: f32) -> u8 {
    if v <= 0.0 {
        0
    } else if v >= 255.0 {
        255
    } else {
        (v + 0.5) as u8
    }
}

/// Le uma cor. `atual` = a cor do proprio elemento (currentcolor).
pub fn ler_cor(l: &mut Leitor, atual: Option<Cor>) -> Option<Cor> {
    let t = l.proximo()?.clone();
    match t {
        Token::Hash(h, _) => hexa(&h),
        Token::Ident(n) if n.eq_ignore_ascii_case("currentcolor") => atual.or(Some(Cor::PRETO)),
        Token::Ident(n) => cor_nomeada(&n),
        Token::Funcao(f) if matches!(f.as_str(), "rgb" | "rgba" | "hsl" | "hsla") => {
            let args = l.argumentos()?;
            let mut nums: Vec<(f32, bool)> = Vec::new(); // (valor, porcentagem)
            for t in &args {
                match t {
                    Token::Numero(v, _) => nums.push((*v, false)),
                    Token::Porcentagem(v) => nums.push((*v, true)),
                    Token::Dimensao(v, u) if u == "deg" => nums.push((*v, false)),
                    Token::Dimensao(v, u) if u == "turn" => nums.push((*v * 360.0, false)),
                    Token::Dimensao(v, u) if u == "rad" => nums.push((*v * 57.29578, false)),
                    Token::Ident(n) if n == "none" => nums.push((0.0, false)),
                    Token::Virgula | Token::Espaco | Token::Delim('/') => {}
                    _ => return None,
                }
            }
            if nums.len() < 3 {
                return None;
            }
            let alfa = nums.get(3).map_or(255, |&(v, p)| byte(if p { v * 2.55 } else { v * 255.0 }));
            if f.starts_with("rgb") {
                let c = |(v, p): (f32, bool)| byte(if p { v * 2.55 } else { v });
                Some(Cor::rgba(c(nums[0]), c(nums[1]), c(nums[2]), alfa))
            } else {
                let (r, g, b) = hsl_para_rgb(nums[0].0, nums[1].0 / 100.0, nums[2].0 / 100.0);
                Some(Cor::rgba(byte(r * 255.0), byte(g * 255.0), byte(b * 255.0), alfa))
            }
        }
        _ => None,
    }
}

// ---------------------------------------------------------------- medidas

/// Uma medida como combinacao linear de unidades (o que calc() produz na
/// pratica: "calc(100% - 2em + 4px)").
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Linear {
    pub px: f32,
    pub em: f32,
    pub rem: f32,
    pub vw: f32,
    pub vh: f32,
    pub vmin: f32,
    pub vmax: f32,
    pub pct: f32,
}

impl Linear {
    pub fn px(v: f32) -> Linear {
        Linear { px: v, ..Default::default() }
    }
    pub fn pct(v: f32) -> Linear {
        Linear { pct: v, ..Default::default() }
    }
    fn somar(self, o: Linear, sinal: f32) -> Linear {
        Linear {
            px: self.px + sinal * o.px,
            em: self.em + sinal * o.em,
            rem: self.rem + sinal * o.rem,
            vw: self.vw + sinal * o.vw,
            vh: self.vh + sinal * o.vh,
            vmin: self.vmin + sinal * o.vmin,
            vmax: self.vmax + sinal * o.vmax,
            pct: self.pct + sinal * o.pct,
        }
    }
    fn vezes(self, k: f32) -> Linear {
        Linear {
            px: self.px * k,
            em: self.em * k,
            rem: self.rem * k,
            vw: self.vw * k,
            vh: self.vh * k,
            vmin: self.vmin * k,
            vmax: self.vmax * k,
            pct: self.pct * k,
        }
    }
    fn so_numero(&self) -> bool {
        *self == Linear::default()
    }
}

/// Uma medida especificada (antes de virar pixels).
#[derive(Clone, PartialEq, Debug)]
pub enum Comprimento {
    Linear(Linear),
    Min(Vec<Comprimento>),
    Max(Vec<Comprimento>),
}

/// O que e preciso para virar pixels.
#[derive(Clone, Copy, Debug)]
pub struct Contexto {
    pub fonte: f32,
    pub fonte_raiz: f32,
    pub largura_tela: f32,
    pub altura_tela: f32,
}

/// Medida calculada: px + pct% (em ponto fixo; pct em 1/64 de %).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Medida {
    pub px: Fixo,
    pub pct: i32,
}

impl Medida {
    pub const ZERO: Medida = Medida { px: 0, pct: 0 };
    pub fn px(v: Fixo) -> Medida {
        Medida { px: v, pct: 0 }
    }
    /// Valor final, dado o tamanho de referencia das porcentagens.
    pub fn resolver(self, base: Fixo) -> Fixo {
        if self.pct == 0 {
            return self.px;
        }
        self.px.saturating_add(((base as i64 * self.pct as i64) / (100 * 64)) as i32)
    }
    pub fn tem_pct(self) -> bool {
        self.pct != 0
    }
}

impl Linear {
    pub fn calcular(&self, c: &Contexto) -> Medida {
        let px = self.px
            + self.em * c.fonte
            + self.rem * c.fonte_raiz
            + self.vw * c.largura_tela / 100.0
            + self.vh * c.altura_tela / 100.0
            + self.vmin * c.largura_tela.min(c.altura_tela) / 100.0
            + self.vmax * c.largura_tela.max(c.altura_tela) / 100.0;
        Medida { px: fixo(px), pct: fixo(self.pct) }
    }
}

impl Comprimento {
    pub fn px(v: f32) -> Comprimento {
        Comprimento::Linear(Linear::px(v))
    }

    pub fn calcular(&self, c: &Contexto) -> Medida {
        match self {
            Comprimento::Linear(l) => l.calcular(c),
            Comprimento::Min(v) | Comprimento::Max(v) => {
                // porcentagens aqui: aproximadas pela largura da tela
                let mut melhor: Option<Fixo> = None;
                for x in v {
                    let m = x.calcular(c);
                    let px = m.resolver(fixo(c.largura_tela));
                    melhor = Some(match (melhor, self) {
                        (None, _) => px,
                        (Some(a), Comprimento::Min(_)) => a.min(px),
                        (Some(a), _) => a.max(px),
                    });
                }
                Medida::px(melhor.unwrap_or(0))
            }
        }
    }
}

fn unidade(v: f32, u: &str) -> Option<Linear> {
    let mut l = Linear::default();
    match u {
        "px" => l.px = v,
        "em" => l.em = v,
        "rem" => l.rem = v,
        "ex" | "ch" | "cap" => l.em = v * 0.5,
        "ic" | "lh" => l.em = v,
        "rlh" => l.rem = v,
        "vw" | "svw" | "lvw" | "dvw" | "cqw" | "cqi" => l.vw = v,
        "vh" | "svh" | "lvh" | "dvh" | "cqh" | "cqb" => l.vh = v,
        "vmin" | "svmin" | "dvmin" | "lvmin" | "cqmin" => l.vmin = v,
        "vmax" | "svmax" | "dvmax" | "lvmax" | "cqmax" => l.vmax = v,
        "pt" => l.px = v * 4.0 / 3.0,
        "pc" => l.px = v * 16.0,
        "in" => l.px = v * 96.0,
        "cm" => l.px = v * 96.0 / 2.54,
        "mm" => l.px = v * 96.0 / 25.4,
        "q" => l.px = v * 96.0 / 101.6,
        _ => return None,
    }
    Some(l)
}

/// Le uma medida: dimensao, porcentagem, 0, calc(), min(), max(),
/// clamp(). `numero_e_px`: numeros sem unidade valem como px (atributos
/// HTML e modo quirks).
pub fn ler_comprimento(l: &mut Leitor, numero_e_px: bool) -> Option<Comprimento> {
    match l.espiar()?.clone() {
        Token::Dimensao(v, u) => {
            l.proximo();
            unidade(v, &u).map(Comprimento::Linear)
        }
        Token::Porcentagem(v) => {
            l.proximo();
            Some(Comprimento::Linear(Linear::pct(v)))
        }
        Token::Numero(v, _) if v == 0.0 || numero_e_px => {
            l.proximo();
            Some(Comprimento::px(v))
        }
        Token::Funcao(f) if f == "calc" || f == "-webkit-calc" => {
            l.proximo();
            let args = l.argumentos()?;
            let mut sub = Leitor::novo(&args);
            let e = expressao(&mut sub)?;
            Some(e)
        }
        Token::Funcao(f) if f == "min" || f == "max" || f == "clamp" => {
            l.proximo();
            let args = l.argumentos()?;
            let mut partes = Vec::new();
            for parte in dividir_virgulas(&args) {
                let mut sub = Leitor::novo(parte);
                partes.push(expressao(&mut sub)?);
            }
            match f.as_str() {
                "min" => Some(Comprimento::Min(partes)),
                "max" => Some(Comprimento::Max(partes)),
                _ if partes.len() == 3 => {
                    // clamp(a, b, c) = max(a, min(b, c))
                    let c = partes.pop()?;
                    let b = partes.pop()?;
                    let a = partes.pop()?;
                    Some(Comprimento::Max(alloc::vec![a, Comprimento::Min(alloc::vec![b, c])]))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// Termo de calc(): numero puro ou medida.
enum Termo {
    Numero(f32),
    Medida(Comprimento),
}

fn termo(l: &mut Leitor) -> Option<Termo> {
    match l.espiar()?.clone() {
        Token::Numero(v, _) => {
            l.proximo();
            Some(Termo::Numero(v))
        }
        Token::AbreParen => {
            l.proximo();
            let mut dentro = Vec::new();
            let mut nivel = 1;
            while let Some(t) = l.proximo_cru() {
                match t {
                    Token::AbreParen | Token::Funcao(_) => nivel += 1,
                    Token::FechaParen => {
                        nivel -= 1;
                        if nivel == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                dentro.push(t.clone());
            }
            let mut sub = Leitor::novo(&dentro);
            Some(Termo::Medida(expressao(&mut sub)?))
        }
        Token::Funcao(f) if f == "calc" => {
            l.proximo();
            let args = l.argumentos()?;
            let mut sub = Leitor::novo(&args);
            Some(Termo::Medida(expressao(&mut sub)?))
        }
        _ => ler_comprimento(l, false).map(Termo::Medida),
    }
}

fn produto(l: &mut Leitor) -> Option<Termo> {
    let mut a = termo(l)?;
    loop {
        let op = match l.espiar() {
            Some(Token::Delim('*')) => '*',
            Some(Token::Delim('/')) => '/',
            _ => return Some(a),
        };
        l.proximo();
        let b = termo(l)?;
        a = match (a, b, op) {
            (Termo::Numero(x), Termo::Numero(y), '*') => Termo::Numero(x * y),
            (Termo::Numero(x), Termo::Numero(y), _) => Termo::Numero(if y == 0.0 { 0.0 } else { x / y }),
            (Termo::Medida(Comprimento::Linear(m)), Termo::Numero(k), '*') | (Termo::Numero(k), Termo::Medida(Comprimento::Linear(m)), '*') => {
                Termo::Medida(Comprimento::Linear(m.vezes(k)))
            }
            (Termo::Medida(Comprimento::Linear(m)), Termo::Numero(k), '/') if k != 0.0 => {
                Termo::Medida(Comprimento::Linear(m.vezes(1.0 / k)))
            }
            _ => return None,
        };
    }
}

fn expressao(l: &mut Leitor) -> Option<Comprimento> {
    let mut acc = match produto(l)? {
        Termo::Numero(v) if v == 0.0 => Linear::default(),
        Termo::Numero(_) => return None,
        Termo::Medida(Comprimento::Linear(m)) => m,
        Termo::Medida(outro) => {
            return if l.vazio() { Some(outro) } else { None };
        }
    };
    loop {
        let sinal = match l.espiar() {
            Some(Token::Delim('+')) => 1.0,
            Some(Token::Delim('-')) => -1.0,
            // "+4px" grudado vira numero com sinal
            Some(Token::Dimensao(..) | Token::Numero(..) | Token::Porcentagem(_)) => 1.0,
            None => return Some(Comprimento::Linear(acc)),
            _ => return None,
        };
        if matches!(l.espiar(), Some(Token::Delim(_))) {
            l.proximo();
        }
        match produto(l)? {
            Termo::Medida(Comprimento::Linear(m)) => acc = acc.somar(m, sinal),
            Termo::Numero(v) if v == 0.0 => {}
            _ => return None,
        }
    }
}

pub fn eh_zero(c: &Comprimento) -> bool {
    matches!(c, Comprimento::Linear(l) if l.so_numero())
}

// ----------------------------------------------------------------- leitor

/// Divide por virgulas do nivel de fora (as de dentro de rgba(), calc()...
/// ficam onde estao).
pub fn dividir_virgulas(t: &[Token]) -> Vec<&[Token]> {
    let mut v = Vec::new();
    let mut nivel = 0i32;
    let mut inicio = 0;
    for (k, x) in t.iter().enumerate() {
        match x {
            Token::Funcao(_) | Token::AbreParen | Token::AbreColchete => nivel += 1,
            Token::FechaParen | Token::FechaColchete => nivel -= 1,
            Token::Virgula if nivel == 0 => {
                v.push(&t[inicio..k]);
                inicio = k + 1;
            }
            _ => {}
        }
    }
    v.push(&t[inicio..]);
    v
}

/// Le tokens pulando espacos (com `proximo_cru` para quem precisa deles).
#[derive(Clone)]
pub struct Leitor<'a> {
    t: &'a [Token],
    i: usize,
}

impl<'a> Leitor<'a> {
    pub fn novo(t: &'a [Token]) -> Self {
        Leitor { t, i: 0 }
    }

    fn pular(&mut self) {
        while self.t.get(self.i) == Some(&Token::Espaco) {
            self.i += 1;
        }
    }

    pub fn espiar(&mut self) -> Option<&'a Token> {
        self.pular();
        self.t.get(self.i)
    }

    pub fn proximo(&mut self) -> Option<&'a Token> {
        self.pular();
        let t = self.t.get(self.i);
        if t.is_some() {
            self.i += 1;
        }
        t
    }

    pub fn proximo_cru(&mut self) -> Option<&'a Token> {
        let t = self.t.get(self.i);
        if t.is_some() {
            self.i += 1;
        }
        t
    }

    pub fn vazio(&mut self) -> bool {
        self.espiar().is_none()
    }

    pub fn posicao(&self) -> usize {
        self.i
    }

    pub fn voltar(&mut self, i: usize) {
        self.i = i;
    }

    /// Depois de um Token::Funcao: os tokens ate o ')' que fecha.
    pub fn argumentos(&mut self) -> Option<Vec<Token>> {
        let mut v = Vec::new();
        let mut nivel = 1;
        loop {
            let t = self.proximo_cru()?;
            match t {
                Token::AbreParen | Token::Funcao(_) => nivel += 1,
                Token::FechaParen => {
                    nivel -= 1;
                    if nivel == 0 {
                        return Some(v);
                    }
                }
                _ => {}
            }
            v.push(t.clone());
        }
    }

    /// Um identificador (sem diferenciar maiusculas, devolvido minusculo).
    pub fn ident(&mut self) -> Option<String> {
        match self.espiar()? {
            Token::Ident(s) => {
                self.proximo();
                Some(s.to_ascii_lowercase())
            }
            _ => None,
        }
    }

    pub fn numero(&mut self) -> Option<f32> {
        match self.espiar()? {
            Token::Numero(v, _) => {
                let v = *v;
                self.proximo();
                Some(v)
            }
            _ => None,
        }
    }

    pub fn resto(&self) -> &'a [Token] {
        &self.t[self.i.min(self.t.len())..]
    }
}

/// Imagem de fundo / lista: url() ou gradiente.
#[derive(Clone, PartialEq, Debug)]
pub enum Imagem {
    Url(String),
    Gradiente(Box<Gradiente>),
}

#[derive(Clone, PartialEq, Debug)]
pub struct Gradiente {
    /// Angulo em graus (180 = de cima para baixo).
    pub angulo: f32,
    pub radial: bool,
    pub repetir: bool,
    pub paradas: Vec<(Cor, Option<f32>)>, // (cor, posicao 0..1)
}

pub fn ler_imagem(l: &mut Leitor, atual: Option<Cor>) -> Option<Imagem> {
    match l.espiar()?.clone() {
        Token::Url(u) => {
            l.proximo();
            Some(Imagem::Url(u))
        }
        Token::Funcao(f) if f == "url" => {
            l.proximo();
            let args = l.argumentos()?;
            match args.iter().find(|t| **t != Token::Espaco) {
                Some(Token::Texto(s)) => Some(Imagem::Url(s.clone())),
                _ => None,
            }
        }
        Token::Funcao(f) if f.ends_with("linear-gradient") || f.ends_with("radial-gradient") => {
            l.proximo();
            let radial = f.contains("radial");
            let repetir = f.contains("repeating");
            let args = l.argumentos()?;
            let mut g = Gradiente { angulo: 180.0, radial, repetir, paradas: Vec::new() };
            for (k, parte) in dividir_virgulas(&args).into_iter().enumerate() {
                let mut sub = Leitor::novo(parte);
                if k == 0 {
                    match sub.espiar() {
                        Some(Token::Dimensao(v, u)) => {
                            g.angulo = match u.as_str() {
                                "deg" => *v,
                                "turn" => *v * 360.0,
                                "rad" => *v * 57.29578,
                                "grad" => *v * 0.9,
                                _ => 180.0,
                            };
                            continue;
                        }
                        Some(Token::Ident(i)) if i.eq_ignore_ascii_case("to") || radial => {
                            let palavras: Vec<String> = parte
                                .iter()
                                .filter_map(|t| if let Token::Ident(s) = t { Some(s.to_ascii_lowercase()) } else { None })
                                .collect();
                            if radial {
                                if cor_nomeada(palavras.first().map_or("", |s| s.as_str())).is_none() {
                                    continue;
                                }
                            } else {
                                let tem = |p: &str| palavras.iter().any(|x| x == p);
                                g.angulo = match (tem("top"), tem("bottom"), tem("left"), tem("right")) {
                                    (true, _, true, _) => 315.0,
                                    (true, _, _, true) => 45.0,
                                    (_, true, true, _) => 225.0,
                                    (_, true, _, true) => 135.0,
                                    (true, _, _, _) => 0.0,
                                    (_, _, true, _) => 270.0,
                                    (_, _, _, true) => 90.0,
                                    _ => 180.0,
                                };
                                continue;
                            }
                        }
                        _ => {}
                    }
                }
                let antes = sub.posicao();
                let cor = match ler_cor(&mut sub, atual) {
                    Some(c) => c,
                    None => {
                        sub.voltar(antes);
                        continue;
                    }
                };
                let pos = match sub.espiar() {
                    Some(Token::Porcentagem(p)) => Some(*p / 100.0),
                    _ => None,
                };
                g.paradas.push((cor, pos));
            }
            if g.paradas.is_empty() {
                return None;
            }
            Some(Imagem::Gradiente(Box::new(g)))
        }
        _ => None,
    }
}
