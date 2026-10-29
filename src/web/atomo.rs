//! Atomos: nomes de tags, atributos e propriedades viram um numero (u32).
//! Comparar dois nomes vira comparar dois inteiros (o que o casamento de
//! seletores do CSS faz milhoes de vezes por pagina). Os nomes conhecidos
//! tem numero fixo (constantes em `a::`); os outros entram numa tabela de
//! espalhamento global na primeira vez que aparecem e nunca saem.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct Atomo(pub u32);

/// Os nomes fixos: a posicao aqui e o numero do atomo.
static FIXOS: &[&str] = &[
    "", // 0: nenhum
    // tags
    "html", "head", "body", "title", "meta", "link", "style", "script", "noscript", "template", "base",
    "div", "span", "p", "a", "img", "br", "hr", "h1", "h2", "h3", "h4", "h5", "h6", "ul", "ol", "li", "dl",
    "dt", "dd", "table", "thead", "tbody", "tfoot", "tr", "td", "th", "caption", "colgroup", "col", "form",
    "input", "button", "select", "option", "optgroup", "textarea", "label", "fieldset", "legend", "iframe",
    "object", "embed", "video", "audio", "source", "track", "canvas", "svg", "math", "pre", "code",
    "blockquote", "em", "strong", "b", "i", "u", "s", "small", "big", "sub", "sup", "q", "cite", "abbr",
    "address", "article", "aside", "footer", "header", "main", "nav", "section", "figure", "figcaption",
    "details", "summary", "dialog", "picture", "time", "mark", "del", "ins", "kbd", "samp", "var", "wbr",
    "nobr", "center", "font", "marquee", "applet", "frameset", "frame", "noframes", "noembed", "plaintext",
    "xmp", "listing", "basefont", "bgsound", "keygen", "param", "menu", "search", "hgroup", "tt", "strike",
    "rb", "rp", "rt", "rtc", "ruby", "image", "dir", "data", "output", "progress", "meter", "area", "map",
    "foreignObject", "desc", "mi", "mo", "mn", "ms", "mtext", "annotation-xml", "path", "g", "use",
    "circle", "rect", "line", "polygon", "polyline", "ellipse", "defs", "symbol", "slot",
    // atributos
    "id", "class", "href", "src", "alt", "type", "name", "value", "rel", "width", "height", "content",
    "charset", "http-equiv", "lang", "colspan", "rowspan", "align", "bgcolor", "color", "face", "size",
    "border", "cellpadding", "cellspacing", "hidden", "disabled", "checked", "selected", "action",
    "method", "for", "placeholder", "target", "media", "srcset", "sizes", "loading", "async", "defer",
    "role", "tabindex", "encoding", "start", "reversed", "valign", "nowrap", "background", "text",
    "viewBox", "fill", "stroke", "d", "multiple", "readonly", "maxlength", "aria-hidden",
];

const fn igual(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// Numero de um nome fixo (em tempo de compilacao).
pub const fn fixo(nome: &str) -> Atomo {
    let mut i = 0;
    while i < FIXOS.len() {
        if igual(FIXOS[i], nome) {
            return Atomo(i as u32);
        }
        i += 1;
    }
    panic!("atomo fixo desconhecido")
}

/// Os atomos fixos como constantes: a::DIV, a::HREF...
pub mod a {
    use super::{Atomo, fixo};
    macro_rules! constantes {
        ($($c:ident = $s:literal),* $(,)?) => { $(pub const $c: Atomo = fixo($s);)* };
    }
    constantes! {
        VAZIO = "", HTML = "html", HEAD = "head", BODY = "body", TITLE = "title", META = "meta",
        LINK = "link", STYLE = "style", SCRIPT = "script", NOSCRIPT = "noscript", TEMPLATE = "template",
        BASE = "base", DIV = "div", SPAN = "span", P = "p", A = "a", IMG = "img", BR = "br", HR = "hr",
        H1 = "h1", H2 = "h2", H3 = "h3", H4 = "h4", H5 = "h5", H6 = "h6", UL = "ul", OL = "ol", LI = "li",
        DL = "dl", DT = "dt", DD = "dd", TABLE = "table", THEAD = "thead", TBODY = "tbody", TFOOT = "tfoot",
        TR = "tr", TD = "td", TH = "th", CAPTION = "caption", COLGROUP = "colgroup", COL = "col",
        FORM = "form", INPUT = "input", BUTTON = "button", SELECT = "select", OPTION = "option",
        OPTGROUP = "optgroup", TEXTAREA = "textarea", LABEL = "label", FIELDSET = "fieldset",
        LEGEND = "legend", IFRAME = "iframe", OBJECT = "object", EMBED = "embed", VIDEO = "video",
        AUDIO = "audio", SOURCE = "source", TRACK = "track", CANVAS = "canvas", SVG = "svg", MATH = "math",
        PRE = "pre", CODE = "code", BLOCKQUOTE = "blockquote", EM = "em", STRONG = "strong", B = "b",
        I = "i", U = "u", S = "s", SMALL = "small", BIG = "big", SUB = "sub", SUP = "sup", Q = "q",
        CITE = "cite", ABBR = "abbr", ADDRESS = "address", ARTICLE = "article", ASIDE = "aside",
        FOOTER = "footer", HEADER = "header", MAIN = "main", NAV = "nav", SECTION = "section",
        FIGURE = "figure", FIGCAPTION = "figcaption", DETAILS = "details", SUMMARY = "summary",
        DIALOG = "dialog", PICTURE = "picture", TIME = "time", MARK = "mark", DEL = "del", INS = "ins",
        KBD = "kbd", SAMP = "samp", VAR = "var", WBR = "wbr", NOBR = "nobr", CENTER = "center",
        FONT = "font", MARQUEE = "marquee", APPLET = "applet", FRAMESET = "frameset", FRAME = "frame",
        NOFRAMES = "noframes", NOEMBED = "noembed", PLAINTEXT = "plaintext", XMP = "xmp",
        LISTING = "listing", BASEFONT = "basefont", BGSOUND = "bgsound", KEYGEN = "keygen",
        PARAM = "param", MENU = "menu", SEARCH = "search", HGROUP = "hgroup", TT = "tt", STRIKE = "strike",
        RB = "rb", RP = "rp", RT = "rt", RTC = "rtc", RUBY = "ruby", IMAGE = "image", DIR = "dir",
        DATA = "data", OUTPUT = "output", PROGRESS = "progress", METER = "meter", AREA = "area", MAP = "map",
        FOREIGN_OBJECT = "foreignObject", DESC = "desc", MI = "mi", MO = "mo", MN = "mn", MS = "ms",
        MTEXT = "mtext", ANNOTATION_XML = "annotation-xml", SLOT = "slot",
        ID = "id", CLASS = "class", HREF = "href", SRC = "src", ALT = "alt", TYPE = "type", NAME = "name",
        VALUE = "value", REL = "rel", WIDTH = "width", HEIGHT = "height", CONTENT = "content",
        CHARSET = "charset", HTTP_EQUIV = "http-equiv", LANG = "lang", COLSPAN = "colspan",
        ROWSPAN = "rowspan", ALIGN = "align", BGCOLOR = "bgcolor", COLOR = "color", FACE = "face",
        SIZE = "size", BORDER = "border", CELLPADDING = "cellpadding", CELLSPACING = "cellspacing",
        HIDDEN = "hidden", DISABLED = "disabled", CHECKED = "checked", SELECTED = "selected",
        ACTION = "action", METHOD = "method", FOR = "for", PLACEHOLDER = "placeholder", TARGET = "target",
        MEDIA = "media", SRCSET = "srcset", SIZES = "sizes", LOADING = "loading", ASYNC = "async",
        DEFER = "defer", ROLE = "role", TABINDEX = "tabindex", ENCODING = "encoding", START = "start",
        REVERSED = "reversed", VALIGN = "valign", NOWRAP = "nowrap", BACKGROUND = "background",
        TEXT = "text", VIEWBOX = "viewBox", FILL = "fill", STROKE = "stroke", D = "d", MULTIPLE = "multiple",
        READONLY = "readonly", MAXLENGTH = "maxlength", ATRIB_SPAN = "span",
    }
}

// ------------------------------------------------------- tabela global

struct Tabela {
    nomes: Vec<&'static str>,
    /// Espalhamento aberto: indice do atomo + 1 (0 = vazio).
    baldes: Vec<u32>,
}

struct Global {
    trava: AtomicBool,
    tabela: UnsafeCell<Option<Tabela>>,
}

unsafe impl Sync for Global {}

static GLOBAL: Global = Global { trava: AtomicBool::new(false), tabela: UnsafeCell::new(None) };

fn espalhar(s: &[u8]) -> u32 {
    // FNV-1a
    let mut h = 0x811c9dc5u32;
    for &b in s {
        h ^= b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    h
}

impl Tabela {
    fn nova() -> Tabela {
        let mut t = Tabela { nomes: Vec::with_capacity(1024), baldes: alloc::vec![0; 2048] };
        for nome in FIXOS {
            t.inserir(nome);
        }
        t
    }

    fn achar(&self, s: &str) -> Result<u32, usize> {
        let mascara = self.baldes.len() - 1;
        let mut i = espalhar(s.as_bytes()) as usize & mascara;
        loop {
            match self.baldes[i] {
                0 => return Err(i),
                k if self.nomes[k as usize - 1] == s => return Ok(k - 1),
                _ => i = (i + 1) & mascara,
            }
        }
    }

    fn inserir(&mut self, s: &'static str) -> u32 {
        match self.achar(s) {
            Ok(k) => k,
            Err(balde) => {
                let k = self.nomes.len() as u32;
                self.nomes.push(s);
                self.baldes[balde] = k + 1;
                if self.nomes.len() * 2 > self.baldes.len() {
                    self.crescer();
                }
                k
            }
        }
    }

    fn crescer(&mut self) {
        let n = self.baldes.len() * 2;
        self.baldes = alloc::vec![0; n];
        for (k, nome) in self.nomes.iter().enumerate() {
            let mut i = espalhar(nome.as_bytes()) as usize & (n - 1);
            while self.baldes[i] != 0 {
                i = (i + 1) & (n - 1);
            }
            self.baldes[i] = k as u32 + 1;
        }
    }
}

fn com<R>(f: impl FnOnce(&mut Tabela) -> R) -> R {
    while GLOBAL.trava.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    let t = unsafe { &mut *GLOBAL.tabela.get() };
    let r = f(t.get_or_insert_with(Tabela::nova));
    GLOBAL.trava.store(false, Ordering::Release);
    r
}

impl Atomo {
    /// O atomo de um nome (criando se ainda nao existe).
    pub fn de(nome: &str) -> Atomo {
        com(|t| match t.achar(nome) {
            Ok(k) => Atomo(k),
            Err(_) => {
                let guardado: &'static str = Box::leak(Box::from(nome));
                Atomo(t.inserir(guardado))
            }
        })
    }

    /// O atomo de um nome, se ele ja existe (nao cria).
    pub fn se_existe(nome: &str) -> Option<Atomo> {
        com(|t| t.achar(nome).ok().map(Atomo))
    }

    /// O atomo de um nome em minusculas (sem alocar se ja for minusculo).
    pub fn minusculo(nome: &str) -> Atomo {
        if nome.bytes().any(|c| c.is_ascii_uppercase()) {
            Atomo::de(&nome.to_ascii_lowercase())
        } else {
            Atomo::de(nome)
        }
    }

    pub fn texto(self) -> &'static str {
        if (self.0 as usize) < FIXOS.len() {
            return FIXOS[self.0 as usize];
        }
        com(|t| t.nomes.get(self.0 as usize).copied().unwrap_or(""))
    }
}
