//! Realce de sintaxe por linha para o editor: um leitor rapido por bytes
//! que diz a classe de cada trecho (palavra reservada, tipo, texto,
//! numero, comentario...). O estado entre linhas (dentro de /* */, de
//! template `...` ou de string tripla do Python) e um u8, entao o editor
//! guarda um por linha e so recalcula a partir da linha alterada.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Linguagem {
    Nenhuma,
    C,
    Js,
    Rust,
    Python,
    Shell,
    Css,
    Html,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Classe {
    Normal,
    Palavra,
    Tipo,
    Texto,
    Numero,
    Comentario,
    Preproc,
    Funcao,
    Constante,
}

/// Estado no fim de uma linha (e no comeco da proxima).
pub type Estado = u8;
pub const NORMAL: Estado = 0;
const EM_COMENTARIO: Estado = 1;
const EM_TEMPLATE: Estado = 2;
const EM_TRIPLA_DUPLA: Estado = 3;
const EM_TRIPLA_SIMPLES: Estado = 4;
const EM_COMENTARIO_HTML: Estado = 5;

pub fn linguagem_do_caminho(caminho: &str) -> Linguagem {
    let ext = caminho.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "c" | "h" | "cpp" | "cc" | "hpp" | "cxx" | "java" | "cs" | "go" => Linguagem::C,
        "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" | "json" => Linguagem::Js,
        "rs" => Linguagem::Rust,
        "py" => Linguagem::Python,
        "sh" | "bash" | "cfg" | "conf" | "toml" | "ini" => Linguagem::Shell,
        "css" => Linguagem::Css,
        "html" | "htm" | "xml" | "svg" => Linguagem::Html,
        _ => Linguagem::Nenhuma,
    }
}

pub fn nome(l: Linguagem) -> &'static str {
    match l {
        Linguagem::Nenhuma => "Texto",
        Linguagem::C => "C",
        Linguagem::Js => "JavaScript",
        Linguagem::Rust => "Rust",
        Linguagem::Python => "Python",
        Linguagem::Shell => "Shell",
        Linguagem::Css => "CSS",
        Linguagem::Html => "HTML",
    }
}

const C_PALAVRAS: &[&str] = &[
    "if", "else", "for", "while", "do", "switch", "case", "default", "break", "continue", "return", "goto", "sizeof", "typedef",
    "struct", "union", "enum", "static", "extern", "const", "volatile", "inline", "register", "auto", "restrict", "class",
    "public", "private", "protected", "virtual", "template", "typename", "namespace", "using", "new", "delete", "try",
    "catch", "throw", "this", "operator", "func", "package", "import", "var", "go", "defer", "chan", "select", "range",
];
const C_TIPOS: &[&str] = &[
    "void", "char", "short", "int", "long", "float", "double", "signed", "unsigned", "bool", "size_t", "ssize_t", "uint8_t",
    "uint16_t", "uint32_t", "uint64_t", "int8_t", "int16_t", "int32_t", "int64_t", "uintptr_t", "FILE", "string", "byte",
];
const C_CONST: &[&str] = &["NULL", "true", "false", "nullptr", "nil", "EOF"];

const JS_PALAVRAS: &[&str] = &[
    "if", "else", "for", "while", "do", "switch", "case", "default", "break", "continue", "return", "function", "var", "let",
    "const", "new", "delete", "typeof", "instanceof", "in", "of", "class", "extends", "super", "this", "import", "export",
    "from", "as", "async", "await", "yield", "try", "catch", "finally", "throw", "void", "with", "static", "get", "set",
    "interface", "type", "implements", "enum", "readonly", "declare", "keyof",
];
const JS_TIPOS: &[&str] = &[
    "Object", "Array", "String", "Number", "Boolean", "Promise", "Map", "Set", "Date", "RegExp", "Error", "JSON", "Math",
    "Symbol", "console", "window", "document", "string", "number", "boolean", "any", "unknown", "never",
];
const JS_CONST: &[&str] = &["true", "false", "null", "undefined", "NaN", "Infinity"];

const RUST_PALAVRAS: &[&str] = &[
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "fn", "for", "if", "impl", "in", "let", "loop",
    "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "type",
    "unsafe", "use", "where", "while", "async", "await", "dyn",
];
const RUST_TIPOS: &[&str] = &[
    "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32", "f64", "bool", "char",
    "str", "String", "Vec", "Option", "Result", "Box", "Rc", "Arc", "RefCell", "Cell", "BTreeMap", "HashMap",
];
const RUST_CONST: &[&str] = &["true", "false", "None", "Some", "Ok", "Err"];

const PY_PALAVRAS: &[&str] = &[
    "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else", "except", "finally",
    "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try",
    "while", "with", "yield", "self", "print",
];
const PY_TIPOS: &[&str] = &["int", "float", "str", "bool", "list", "dict", "set", "tuple", "bytes", "object", "range", "len"];
const PY_CONST: &[&str] = &["True", "False", "None"];

const SH_PALAVRAS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "in", "do", "done", "while", "until", "case", "esac", "function", "return",
    "export", "local", "source", "echo", "cd", "exit", "set", "unset",
];

fn listas(l: Linguagem) -> (&'static [&'static str], &'static [&'static str], &'static [&'static str]) {
    match l {
        Linguagem::C => (C_PALAVRAS, C_TIPOS, C_CONST),
        Linguagem::Js => (JS_PALAVRAS, JS_TIPOS, JS_CONST),
        Linguagem::Rust => (RUST_PALAVRAS, RUST_TIPOS, RUST_CONST),
        Linguagem::Python => (PY_PALAVRAS, PY_TIPOS, PY_CONST),
        Linguagem::Shell => (SH_PALAVRAS, &[], &["true", "false"]),
        _ => (&[], &[], &[]),
    }
}

fn eh_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

/// Fim (exclusivo) de um texto entre `q` comecando em `i` (depois da
/// aspa); `None` se a linha acaba antes.
fn fim_texto(b: &[u8], mut i: usize, q: u8) -> Option<usize> {
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            c if c == q => return Some(i + 1),
            _ => i += 1,
        }
    }
    None
}

/// Le uma linha. `f(ini, fim, classe)` recebe os trechos em ordem (so os
/// que nao sao Normal); devolve o estado para a linha seguinte.
pub fn linha(l: Linguagem, s: &str, mut estado: Estado, f: &mut impl FnMut(usize, usize, Classe)) -> Estado {
    let b = s.as_bytes();
    let n = b.len();
    let mut i = 0;
    if l == Linguagem::Nenhuma {
        return NORMAL;
    }
    // continuacao de algo aberto na linha de cima
    match estado {
        EM_COMENTARIO | EM_COMENTARIO_HTML => {
            let fecho: &[u8] = if estado == EM_COMENTARIO { b"*/" } else { b"-->" };
            match s.find(core::str::from_utf8(fecho).unwrap_or("*/")) {
                Some(p) => {
                    i = p + fecho.len();
                    f(0, i, Classe::Comentario);
                    estado = NORMAL;
                }
                None => {
                    f(0, n, Classe::Comentario);
                    return estado;
                }
            }
        }
        EM_TEMPLATE => match fim_texto(b, 0, b'`') {
            Some(p) => {
                f(0, p, Classe::Texto);
                i = p;
                estado = NORMAL;
            }
            None => {
                f(0, n, Classe::Texto);
                return estado;
            }
        },
        EM_TRIPLA_DUPLA | EM_TRIPLA_SIMPLES => {
            let fecho = if estado == EM_TRIPLA_DUPLA { "\"\"\"" } else { "'''" };
            match s.find(fecho) {
                Some(p) => {
                    i = p + 3;
                    f(0, i, Classe::Texto);
                    estado = NORMAL;
                }
                None => {
                    f(0, n, Classe::Texto);
                    return estado;
                }
            }
        }
        _ => {}
    }
    if l == Linguagem::Html {
        return linha_html(s, i, f);
    }
    let (palavras, tipos, constantes) = listas(l);
    let comentario_hash = matches!(l, Linguagem::Python | Linguagem::Shell);
    let barra_barra = matches!(l, Linguagem::C | Linguagem::Js | Linguagem::Rust);
    // diretiva do pre-processador ocupa a linha toda
    if l == Linguagem::C && s.trim_start().starts_with('#') {
        f(0, n, Classe::Preproc);
        return NORMAL;
    }
    while i < n {
        let c = b[i];
        // comentarios
        if barra_barra && c == b'/' && i + 1 < n && b[i + 1] == b'/' {
            f(i, n, Classe::Comentario);
            return NORMAL;
        }
        if (barra_barra || l == Linguagem::Css) && c == b'/' && i + 1 < n && b[i + 1] == b'*' {
            match s[i + 2..].find("*/") {
                Some(p) => {
                    let fim = i + 2 + p + 2;
                    f(i, fim, Classe::Comentario);
                    i = fim;
                    continue;
                }
                None => {
                    f(i, n, Classe::Comentario);
                    return EM_COMENTARIO;
                }
            }
        }
        if comentario_hash && c == b'#' {
            f(i, n, Classe::Comentario);
            return NORMAL;
        }
        // textos
        if l == Linguagem::Python && (s[i..].starts_with("\"\"\"") || s[i..].starts_with("'''")) {
            let fecho = &s[i..i + 3];
            match s[i + 3..].find(fecho) {
                Some(p) => {
                    let fim = i + 3 + p + 3;
                    f(i, fim, Classe::Texto);
                    i = fim;
                    continue;
                }
                None => {
                    f(i, n, Classe::Texto);
                    return if fecho == "\"\"\"" { EM_TRIPLA_DUPLA } else { EM_TRIPLA_SIMPLES };
                }
            }
        }
        // em Rust, 'a e tempo de vida (nao char) quando nao fecha logo
        let aspa_rust_vida = l == Linguagem::Rust && c == b'\'' && !(i + 2 < n && (b[i + 2] == b'\'' || b[i + 1] == b'\\'));
        if (c == b'"' || (c == b'\'' && !aspa_rust_vida)) || (c == b'`' && matches!(l, Linguagem::Js | Linguagem::Shell)) {
            match fim_texto(b, i + 1, c) {
                Some(fim) => {
                    f(i, fim, Classe::Texto);
                    i = fim;
                }
                None => {
                    f(i, n, Classe::Texto);
                    return if c == b'`' && l == Linguagem::Js { EM_TEMPLATE } else { NORMAL };
                }
            }
            continue;
        }
        // numeros
        if c.is_ascii_digit() || (c == b'.' && i + 1 < n && b[i + 1].is_ascii_digit() && (i == 0 || !eh_ident(b[i - 1]))) {
            let ini = i;
            i += 1;
            while i < n && (b[i].is_ascii_alphanumeric() || b[i] == b'.' || b[i] == b'_') {
                i += 1;
            }
            f(ini, i, Classe::Numero);
            continue;
        }
        // nomes
        if eh_ident(c) {
            let ini = i;
            while i < n && eh_ident(b[i]) {
                i += 1;
            }
            let p = &s[ini..i];
            // macro do Rust: nome!
            let macro_rust = l == Linguagem::Rust && i < n && b[i] == b'!';
            let classe = if l == Linguagem::Css {
                if i < n && b[i] == b':' { Classe::Tipo } else { Classe::Normal }
            } else if palavras.contains(&p) {
                Classe::Palavra
            } else if tipos.contains(&p) {
                Classe::Tipo
            } else if constantes.contains(&p) {
                Classe::Constante
            } else if macro_rust {
                Classe::Preproc
            } else if i < n && b[i] == b'(' {
                Classe::Funcao
            } else if l == Linguagem::Rust && p.as_bytes()[0].is_ascii_uppercase() {
                Classe::Tipo
            } else {
                Classe::Normal
            };
            if classe != Classe::Normal {
                f(ini, i, classe);
            }
            continue;
        }
        // atributos #[...] do Rust e $VAR do shell
        if l == Linguagem::Rust && c == b'#' && i + 1 < n && (b[i + 1] == b'[' || b[i + 1] == b'!') {
            let fim = s[i..].find(']').map_or(n, |p| i + p + 1);
            f(i, fim, Classe::Preproc);
            i = fim;
            continue;
        }
        if l == Linguagem::Css && (c == b'#' || c == b'.') && i + 1 < n && eh_ident(b[i + 1]) {
            let ini = i;
            i += 1;
            while i < n && (eh_ident(b[i]) || b[i] == b'-') {
                i += 1;
            }
            f(ini, i, Classe::Funcao);
            continue;
        }
        i += 1;
    }
    estado
}

/// HTML/XML: tags, atributos, textos e comentarios.
fn linha_html(s: &str, mut i: usize, f: &mut impl FnMut(usize, usize, Classe)) -> Estado {
    let b = s.as_bytes();
    let n = b.len();
    while i < n {
        if s[i..].starts_with("<!--") {
            match s[i + 4..].find("-->") {
                Some(p) => {
                    let fim = i + 4 + p + 3;
                    f(i, fim, Classe::Comentario);
                    i = fim;
                    continue;
                }
                None => {
                    f(i, n, Classe::Comentario);
                    return EM_COMENTARIO_HTML;
                }
            }
        }
        if b[i] == b'<' {
            // nome da tag
            let ini = i;
            i += 1;
            if i < n && (b[i] == b'/' || b[i] == b'!' || b[i] == b'?') {
                i += 1;
            }
            while i < n && (eh_ident(b[i]) || b[i] == b'-' || b[i] == b':') {
                i += 1;
            }
            f(ini, i, Classe::Palavra);
            // atributos ate o '>'
            while i < n && b[i] != b'>' {
                if b[i] == b'"' || b[i] == b'\'' {
                    let fim = fim_texto(b, i + 1, b[i]).unwrap_or(n);
                    f(i, fim, Classe::Texto);
                    i = fim;
                } else if eh_ident(b[i]) {
                    let a = i;
                    while i < n && (eh_ident(b[i]) || b[i] == b'-' || b[i] == b':') {
                        i += 1;
                    }
                    f(a, i, Classe::Tipo);
                } else {
                    i += 1;
                }
            }
            if i < n {
                f(i, i + 1, Classe::Palavra);
                i += 1;
            }
            continue;
        }
        if b[i] == b'&' {
            let fim = s[i..].find(';').filter(|&p| p < 12).map_or(i + 1, |p| i + p + 1);
            f(i, fim, Classe::Constante);
            i = fim;
            continue;
        }
        i += 1;
    }
    NORMAL
}

/// Cor de cada classe (tema escuro ou claro).
pub fn cor(c: Classe, escuro: bool, normal: u32) -> u32 {
    if escuro {
        match c {
            Classe::Normal => normal,
            Classe::Palavra => 0x00C5_86C0,
            Classe::Tipo => 0x004E_C9B0,
            Classe::Texto => 0x00CE_9178,
            Classe::Numero => 0x00B5_CEA8,
            Classe::Comentario => 0x006A_9955,
            Classe::Preproc => 0x009C_DCFE,
            Classe::Funcao => 0x00DC_DCAA,
            Classe::Constante => 0x004F_C1FF,
        }
    } else {
        match c {
            Classe::Normal => normal,
            Classe::Palavra => 0x00AF_00DB,
            Classe::Tipo => 0x0026_7F99,
            Classe::Texto => 0x00A3_1515,
            Classe::Numero => 0x0009_8658,
            Classe::Comentario => 0x0000_8000,
            Classe::Preproc => 0x0000_00FF,
            Classe::Funcao => 0x0079_5E26,
            Classe::Constante => 0x0000_70C1,
        }
    }
}
