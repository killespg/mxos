//! Modulos ES: o texto do modulo vira uma funcao
//! `(function (__ns, __imp, __meta) { ... })` trocando import/export no
//! nivel dos tokens. Os imports viram `const` lidos do namespace da
//! dependencia (copia no comeco, sem ligacao viva); os exports viram
//! getters no namespace (`__defExp`), entao funcoes e let declarados
//! depois tambem aparecem. O carregador (PRELUDIO de js::dom) busca as
//! dependencias, avalia em pos-ordem e guarda os namespaces por URL.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::lexico::{Tok, Token};

pub struct Modulo {
    /// Expressao JS que avalia para a funcao do modulo.
    pub codigo: String,
    /// URLs absolutas das dependencias estaticas.
    pub deps: Vec<String>,
}

struct Leitor<'a> {
    t: &'a [Token],
    i: usize,
}

impl Leitor<'_> {
    fn tok(&self, k: usize) -> &Tok {
        self.t.get(self.i + k).map_or(&Tok::Fim, |t| &t.tok)
    }
    fn eh(&self, k: usize, s: &str) -> bool {
        match self.tok(k) {
            Tok::Pont(p) | Tok::Palavra(p) => *p == s,
            Tok::Ident(n) => &**n == s,
            _ => false,
        }
    }
    fn pos(&self) -> usize {
        self.t.get(self.i).map_or(0, |t| t.pos as usize)
    }
    /// Nome (identificador ou palavra usada como nome: `default`, `as`...).
    fn nome(&self, k: usize) -> Option<String> {
        match self.tok(k) {
            Tok::Ident(n) => Some(String::from(&**n)),
            Tok::Palavra(p) => Some(String::from(*p)),
            Tok::Texto(s) => Some(String::from(&**s)),
            _ => None,
        }
    }
}

fn eh_ident(s: &str) -> bool {
    let mut c = s.chars();
    c.next().is_some_and(|x| x.is_alphabetic() || x == '_' || x == '$') && c.all(|x| x.is_alphanumeric() || x == '_' || x == '$')
}

fn texto_js(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\u{2028}' => o.push_str("\\u2028"),
            '\u{2029}' => o.push_str("\\u2029"),
            _ => o.push(c),
        }
    }
    o.push('"');
    o
}

/// Resolve um especificador: mapa de imports (exato ou prefixo com /),
/// senao relativo a `base`.
pub fn resolver(spec: &str, base: &str, mapa: &[(String, String)]) -> String {
    if let Some((_, v)) = mapa.iter().find(|(k, _)| k == spec) {
        return v.clone();
    }
    let mut melhor: Option<&(String, String)> = None;
    for par in mapa {
        if par.0.ends_with('/') && spec.starts_with(par.0.as_str()) && melhor.is_none_or(|m| par.0.len() > m.0.len()) {
            melhor = Some(par);
        }
    }
    if let Some((k, v)) = melhor {
        return format!("{v}{}", &spec[k.len()..]);
    }
    crate::web::url::resolver(base, spec)
}

/// Transforma o texto de um modulo (`url` resolve os especificadores).
pub fn transformar(fonte: &str, url: &str, mapa: &[(String, String)]) -> Result<Modulo, String> {
    let toks = super::parser::tokenizar(fonte).map_err(|e| format!("{} (linha {})", e.msg, e.linha))?;
    let mut l = Leitor { t: &toks, i: 0 };
    // trechos do texto a trocar: (inicio, fim, novo)
    let mut trocas: Vec<(usize, usize, String)> = Vec::new();
    let mut topo = String::new(); // imports (sobem para o comeco)
    let mut deps: Vec<String> = Vec::new();
    let mut exportados: Vec<(String, String)> = Vec::new(); // (nome exportado, expressao)
    let mut estrelas: Vec<String> = Vec::new();
    let mut prof: i32 = 0;
    let mut anterior_ponto = false;

    let dep = |spec: &str, deps: &mut Vec<String>, topo: &mut String| -> String {
        let abs = resolver(spec, url, mapa);
        let k = match deps.iter().position(|d| *d == abs) {
            Some(k) => k,
            None => {
                deps.push(abs.clone());
                topo.push_str(&format!("const __m{} = __imp({});\n", deps.len() - 1, texto_js(&abs)));
                deps.len() - 1
            }
        };
        format!("__m{k}")
    };

    while !matches!(l.tok(0), Tok::Fim) {
        let ponto_antes = anterior_ponto;
        anterior_ponto = l.eh(0, ".") || l.eh(0, "?.");
        match l.tok(0) {
            Tok::Pont("{" | "(" | "[") => prof += 1,
            Tok::Pont("}" | ")" | "]") => prof -= 1,
            _ => {}
        }
        // import.meta e import() valem em qualquer profundidade
        if l.eh(0, "import") && !ponto_antes {
            if l.eh(1, ".") && l.eh(2, "meta") {
                let ini = l.pos();
                l.i += 3;
                trocas.push((ini, l.pos(), String::from("__meta ")));
                anterior_ponto = false;
                continue;
            }
            if l.eh(1, "(") {
                let ini = l.pos();
                l.i += 2;
                prof += 1;
                trocas.push((ini, l.pos(), String::from("__importarDe(__meta.url, ")));
                anterior_ponto = false;
                continue;
            }
        }
        if prof != 0 || ponto_antes {
            l.i += 1;
            continue;
        }
        if l.eh(0, "import") {
            let ini = l.pos();
            l.i += 1;
            let mut padrao: Option<String> = None;
            let mut ns: Option<String> = None;
            let mut nomes: Vec<(String, String)> = Vec::new();
            if !matches!(l.tok(0), Tok::Texto(_)) {
                if let Tok::Ident(n) = l.tok(0) {
                    if !l.eh(0, "from") || l.eh(1, "from") {
                        padrao = Some(String::from(&**n));
                        l.i += 1;
                        if l.eh(0, ",") {
                            l.i += 1;
                        }
                    }
                }
                if l.eh(0, "*") {
                    // * as ns
                    ns = l.nome(2);
                    l.i += 3;
                } else if l.eh(0, "{") {
                    l.i += 1;
                    while !l.eh(0, "}") && !matches!(l.tok(0), Tok::Fim) {
                        let Some(importado) = l.nome(0) else { return Err(String::from("import inválido")) };
                        l.i += 1;
                        let local = if l.eh(0, "as") {
                            let n = l.nome(1).unwrap_or_default();
                            l.i += 2;
                            n
                        } else {
                            importado.clone()
                        };
                        nomes.push((importado, local));
                        if l.eh(0, ",") {
                            l.i += 1;
                        }
                    }
                    l.i += 1;
                }
                if l.eh(0, "from") {
                    l.i += 1;
                }
            }
            let Tok::Texto(spec) = l.tok(0).clone() else { return Err(String::from("import sem módulo")) };
            l.i += 1;
            if (l.eh(0, "with") || l.eh(0, "assert")) && l.eh(1, "{") {
                while !l.eh(0, "}") && !matches!(l.tok(0), Tok::Fim) {
                    l.i += 1;
                }
                l.i += 1;
            }
            if l.eh(0, ";") {
                l.i += 1;
            }
            let m = dep(&spec, &mut deps, &mut topo);
            if let Some(p) = padrao {
                topo.push_str(&format!("const {p} = {m}.default;\n"));
            }
            if let Some(n) = ns {
                topo.push_str(&format!("const {n} = {m};\n"));
            }
            for (imp, loc) in nomes {
                topo.push_str(&format!("const {loc} = {m}[{}];\n", texto_js(&imp)));
            }
            trocas.push((ini, l.pos(), String::from("\n")));
            anterior_ponto = false;
            continue;
        }
        if l.eh(0, "export") {
            let ini = l.pos();
            l.i += 1;
            if l.eh(0, "default") {
                l.i += 1;
                let async_ = l.eh(0, "async") && l.eh(1, "function");
                let k = if async_ { 1 } else { 0 };
                if l.eh(k, "function") || l.eh(k, "class") {
                    let mut j = k + 1;
                    if l.eh(j, "*") {
                        j += 1;
                    }
                    if let Tok::Ident(n) = l.tok(j) {
                        // declaracao com nome: so tira o "export default"
                        exportados.push((String::from("default"), String::from(&**n)));
                        trocas.push((ini, l.pos(), String::new()));
                        anterior_ponto = false;
                        continue;
                    }
                }
                exportados.push((String::from("default"), String::from("__padrao")));
                trocas.push((ini, l.pos(), String::from("const __padrao = ")));
                anterior_ponto = false;
                continue;
            }
            if l.eh(0, "{") {
                l.i += 1;
                let mut lista: Vec<(String, String)> = Vec::new();
                while !l.eh(0, "}") && !matches!(l.tok(0), Tok::Fim) {
                    let Some(local) = l.nome(0) else { return Err(String::from("export inválido")) };
                    l.i += 1;
                    let fora = if l.eh(0, "as") {
                        let n = l.nome(1).unwrap_or_default();
                        l.i += 2;
                        n
                    } else {
                        local.clone()
                    };
                    lista.push((local, fora));
                    if l.eh(0, ",") {
                        l.i += 1;
                    }
                }
                l.i += 1;
                if l.eh(0, "from") {
                    let Some(spec) = l.nome(1) else { return Err(String::from("export from sem módulo")) };
                    l.i += 2;
                    let m = dep(&spec, &mut deps, &mut topo);
                    for (local, fora) in lista {
                        exportados.push((fora, format!("{m}[{}]", texto_js(&local))));
                    }
                } else {
                    for (local, fora) in lista {
                        exportados.push((fora, local));
                    }
                }
                if l.eh(0, ";") {
                    l.i += 1;
                }
                trocas.push((ini, l.pos(), String::from("\n")));
                anterior_ponto = false;
                continue;
            }
            if l.eh(0, "*") {
                // export * from 'x' / export * as ns from 'x'
                let como = if l.eh(1, "as") { l.nome(2) } else { None };
                l.i += if como.is_some() { 3 } else { 1 };
                if l.eh(0, "from") {
                    l.i += 1;
                }
                let Tok::Texto(spec) = l.tok(0).clone() else { return Err(String::from("export * sem módulo")) };
                l.i += 1;
                if l.eh(0, ";") {
                    l.i += 1;
                }
                let m = dep(&spec, &mut deps, &mut topo);
                match como {
                    Some(n) => exportados.push((n, m)),
                    None => estrelas.push(m),
                }
                trocas.push((ini, l.pos(), String::from("\n")));
                anterior_ponto = false;
                continue;
            }
            // export function/class/const/let/var: tira o "export" e anota os nomes
            trocas.push((ini, l.pos(), String::new()));
            let async_ = l.eh(0, "async");
            let k = if async_ { 1 } else { 0 };
            if l.eh(k, "function") || l.eh(k, "class") {
                let mut j = k + 1;
                if l.eh(j, "*") {
                    j += 1;
                }
                if let Some(n) = l.nome(j) {
                    exportados.push((n.clone(), n));
                }
            } else if l.eh(0, "const") || l.eh(0, "let") || l.eh(0, "var") {
                for n in nomes_declarados(&l) {
                    exportados.push((n.clone(), n));
                }
            }
            anterior_ponto = false;
            continue;
        }
        l.i += 1;
    }

    // monta o texto novo
    trocas.sort_by_key(|t| t.0);
    let mut corpo = String::with_capacity(fonte.len() + 256);
    let mut ultimo = 0;
    for (ini, fim, novo) in &trocas {
        if *ini < ultimo {
            continue;
        }
        corpo.push_str(&fonte[ultimo..*ini]);
        corpo.push_str(novo);
        ultimo = *fim;
    }
    corpo.push_str(&fonte[ultimo..]);
    let mut codigo = String::with_capacity(corpo.len() + topo.len() + 256);
    // async: await no topo do modulo vale
    codigo.push_str("(async function (__ns, __imp, __meta) {\n__defExp(__ns, {");
    for (fora, expr) in &exportados {
        codigo.push_str(&texto_js(fora));
        // __padrao e nomes simples: getter direto; reexport: le do outro namespace
        if eh_ident(expr) || expr.starts_with("__m") {
            codigo.push_str(&format!(": () => {expr}, "));
        }
    }
    codigo.push_str("});\n");
    codigo.push_str(&topo);
    for m in &estrelas {
        codigo.push_str(&format!("__estrela(__ns, {m});\n"));
    }
    codigo.push_str(&corpo);
    codigo.push_str("\n})");
    Ok(Modulo { codigo, deps })
}

/// Nomes de `const a = 1, {b, c: d} = x, [e] = y` (aproximado: o leitor
/// esta no const/let/var).
fn nomes_declarados(l: &Leitor) -> Vec<String> {
    let mut v = Vec::new();
    let mut k = 1;
    loop {
        // um alvo: nome, { ... } ou [ ... ]
        if l.eh(k, "{") || l.eh(k, "[") {
            let mut prof = 0;
            loop {
                match l.tok(k) {
                    Tok::Pont("{" | "[") => prof += 1,
                    Tok::Pont("}" | "]") => prof -= 1,
                    Tok::Fim => return v,
                    Tok::Ident(n) => {
                        // `a` em {a}, {x: a}, [a]: o que vem depois e , } ] ou =
                        if l.eh(k + 1, ",") || l.eh(k + 1, "}") || l.eh(k + 1, "]") || l.eh(k + 1, "=") {
                            v.push(String::from(&**n));
                        }
                    }
                    _ => {}
                }
                k += 1;
                if prof == 0 {
                    break;
                }
            }
        } else if let Tok::Ident(n) = l.tok(k) {
            v.push(String::from(&**n));
            k += 1;
        } else {
            return v;
        }
        // pula o inicializador ate a proxima virgula do mesmo nivel
        let mut prof = 0;
        loop {
            match l.tok(k) {
                Tok::Pont("{" | "(" | "[") => prof += 1,
                Tok::Pont("}" | ")" | "]") => {
                    if prof == 0 {
                        return v;
                    }
                    prof -= 1
                }
                Tok::Pont(";") if prof == 0 => return v,
                Tok::Pont(",") if prof == 0 => {
                    k += 1;
                    break;
                }
                Tok::Fim => return v,
                _ => {}
            }
            k += 1;
            // fim de comando por quebra de linha (ASI) antes de uma palavra de comando
            if prof == 0 {
                if let Some(t) = l.t.get(l.i + k) {
                    if t.linha_antes && matches!(t.tok, Tok::Palavra("export" | "import" | "function" | "class" | "const" | "let" | "var" | "if" | "for" | "return")) {
                        return v;
                    }
                }
            }
        }
    }
}
