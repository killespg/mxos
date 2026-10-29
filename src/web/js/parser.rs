//! Parser do JavaScript: tokens -> AST. Expressoes por precedencia
//! (Pratt); arrow functions pela "cover grammar" (le como expressao entre
//! parenteses e converte em parametros se vier "=>"); desestruturacao;
//! classes; insercao automatica de ponto e virgula.

use alloc::boxed::Box;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

use super::ast::*;
use super::lexico::{Lexico, Tok, Token, contextual};

#[derive(Debug, Clone)]
pub struct ErroSintaxe {
    pub msg: String,
    pub linha: u32,
}

pub struct Parser {
    t: Vec<Token>,
    i: usize,
    em_funcao: bool,
    em_gerador: bool,
    em_async: bool,
    sem_in: bool,
}

/// Quebra o texto em tokens decidindo se '/' e divisao ou regex pelo
/// token anterior.
pub(crate) fn tokenizar(fonte: &str) -> Result<Vec<Token>, ErroSintaxe> {
    let mut lx = Lexico::novo(fonte);
    let mut v: Vec<Token> = Vec::with_capacity(fonte.len() / 4);
    // para cada '(' aberto: veio depois de if/while/for/with? (entao o ')'
    // que fecha comeca um comando e '/' ali e regex: `if (x) /a/.test(s)`)
    let mut parens: Vec<bool> = Vec::new();
    let mut fechou_controle = false;
    loop {
        let regex_ok = match v.last().map(|t| &t.tok) {
            None => true,
            Some(Tok::Pont(")")) => fechou_controle,
            Some(Tok::Pont(p)) => !matches!(*p, ")" | "]"),
            Some(Tok::Palavra(p)) => !matches!(*p, "this" | "super" | "true" | "false" | "null"),
            Some(Tok::Template(_, _, false)) | Some(Tok::TemplateMeio(_, _, false)) => true,
            _ => false,
        };
        let t = lx.proximo(regex_ok).map_err(|e| ErroSintaxe { msg: e.msg.into(), linha: e.linha })?;
        match &t.tok {
            Tok::Pont("(") => {
                let controle = matches!(v.last().map(|t| &t.tok), Some(Tok::Palavra("if" | "while" | "for" | "with")));
                parens.push(controle);
            }
            Tok::Pont(")") => fechou_controle = parens.pop().unwrap_or(false),
            _ => {}
        }
        let fim = t.tok == Tok::Fim;
        v.push(t);
        if fim {
            return Ok(v);
        }
    }
}

pub fn analisar(fonte: &str) -> Result<Vec<Stmt>, ErroSintaxe> {
    let t = tokenizar(fonte)?;
    let mut p = Parser { t, i: 0, em_funcao: false, em_gerador: false, em_async: false, sem_in: false };
    let mut v = Vec::new();
    while !p.fim() {
        let antes = p.i;
        v.push(p.declaracao()?);
        if p.i == antes {
            return p.erro("comando que não avança");
        }
    }
    Ok(v)
}

/// Prioridade dos operadores binarios (maior = liga mais forte).
fn prioridade(op: &str) -> u8 {
    match op {
        "??" => 1,
        "||" => 2,
        "&&" => 3,
        "|" => 4,
        "^" => 5,
        "&" => 6,
        "==" | "!=" | "===" | "!==" => 7,
        "<" | ">" | "<=" | ">=" | "instanceof" | "in" => 8,
        "<<" | ">>" | ">>>" => 9,
        "+" | "-" => 10,
        "*" | "/" | "%" => 11,
        "**" => 12,
        _ => 0,
    }
}

const ATRIBUICOES: &[&str] = &["=", "+=", "-=", "*=", "/=", "%=", "**=", "<<=", ">>=", ">>>=", "&=", "|=", "^=", "&&=", "||=", "??="];

impl Parser {
    // ------------------------------------------------------------ tokens

    fn tok(&self) -> &Tok {
        &self.t[self.i].tok
    }
    fn tok_em(&self, k: usize) -> &Tok {
        &self.t[(self.i + k).min(self.t.len() - 1)].tok
    }
    fn fim(&self) -> bool {
        *self.tok() == Tok::Fim
    }
    fn avancar(&mut self) -> Tok {
        let t = self.t[self.i].tok.clone();
        if self.i + 1 < self.t.len() {
            self.i += 1;
        }
        t
    }
    fn eh(&self, p: &str) -> bool {
        matches!(self.tok(), Tok::Pont(x) if *x == p) || matches!(self.tok(), Tok::Palavra(x) if *x == p)
    }
    fn eh_em(&self, k: usize, p: &str) -> bool {
        matches!(self.tok_em(k), Tok::Pont(x) if *x == p) || matches!(self.tok_em(k), Tok::Palavra(x) if *x == p)
    }
    fn aceitar(&mut self, p: &str) -> bool {
        if self.eh(p) {
            self.avancar();
            true
        } else {
            false
        }
    }
    fn erro<T>(&self, msg: &str) -> Result<T, ErroSintaxe> {
        let t = &self.t[self.i];
        Err(ErroSintaxe { msg: format!("{msg} (perto de {:?}, pos {})", t.tok, t.pos), linha: t.linha })
    }
    fn esperar(&mut self, p: &str) -> Result<(), ErroSintaxe> {
        if self.aceitar(p) { Ok(()) } else { self.erro(&format!("esperava '{p}'")) }
    }
    fn linha_antes(&self) -> bool {
        self.t[self.i].linha_antes
    }
    /// Ponto e virgula (com a insercao automatica).
    fn fim_de_comando(&mut self) -> Result<(), ErroSintaxe> {
        if self.aceitar(";") || self.eh("}") || self.fim() || self.linha_antes() {
            return Ok(());
        }
        self.erro("esperava ';'")
    }
    /// Um nome (identificador ou palavra contextual).
    fn nome(&mut self) -> Result<Rc<str>, ErroSintaxe> {
        match self.tok().clone() {
            Tok::Ident(n) => {
                self.avancar();
                Ok(n)
            }
            Tok::Palavra(p) if contextual(p) => {
                self.avancar();
                Ok(Rc::from(p))
            }
            _ => self.erro("esperava um nome"),
        }
    }
    fn eh_nome(&self) -> bool {
        matches!(self.tok(), Tok::Ident(_)) || matches!(self.tok(), Tok::Palavra(p) if contextual(p) && *p != "yield" || *p == "yield" && false)
    }
    /// Nome de propriedade: qualquer identificador ou palavra.
    fn nome_propriedade(&mut self) -> Result<Rc<str>, ErroSintaxe> {
        match self.tok().clone() {
            Tok::Ident(n) => {
                self.avancar();
                Ok(n)
            }
            Tok::Palavra(p) => {
                self.avancar();
                Ok(Rc::from(p))
            }
            _ => self.erro("esperava nome de propriedade"),
        }
    }

    // -------------------------------------------------------- declaracoes

    fn declaracao(&mut self) -> Result<Stmt, ErroSintaxe> {
        match self.tok().clone() {
            Tok::Palavra("function") => {
                let f = self.funcao(false, false)?;
                Ok(Stmt::Funcao(f))
            }
            Tok::Palavra("async") if self.eh_em(1, "function") && !self.t[self.i + 1].linha_antes => {
                self.avancar();
                let f = self.funcao(true, false)?;
                Ok(Stmt::Funcao(f))
            }
            Tok::Palavra("class") => Ok(Stmt::Classe(self.classe()?)),
            Tok::Palavra("const") => {
                self.avancar();
                let d = self.lista_decl(TipoDecl::Const)?;
                self.fim_de_comando()?;
                Ok(d)
            }
            Tok::Palavra("let") if matches!(self.tok_em(1), Tok::Ident(_) | Tok::Palavra(_)) || self.eh_em(1, "[") || self.eh_em(1, "{") => {
                if let Tok::Palavra(p) = self.tok_em(1) {
                    if !contextual(p) {
                        return self.comando();
                    }
                }
                self.avancar();
                let d = self.lista_decl(TipoDecl::Let)?;
                self.fim_de_comando()?;
                Ok(d)
            }
            Tok::Palavra("import") if !self.eh_em(1, "(") && !self.eh_em(1, ".") => {
                // modulos: ignora ate o especificador ('x') e o fim do comando
                self.avancar();
                self.pular_ate_modulo();
                Ok(Stmt::Vazio)
            }
            Tok::Palavra("export") => {
                self.avancar();
                if self.aceitar("default") {
                    if self.eh("function") || self.eh("class") || self.eh("async") {
                        return self.declaracao();
                    }
                    let e = self.expressao()?;
                    self.fim_de_comando()?;
                    return Ok(Stmt::Expr(e));
                }
                if self.eh("{") || self.eh("*") {
                    // export { a, b } [from 'x'] / export * from 'x'
                    let estrela = self.eh("*");
                    if !estrela {
                        while !self.fim() && !self.aceitar("}") {
                            self.avancar();
                        }
                    }
                    if estrela || self.eh("from") {
                        self.pular_ate_modulo();
                    } else {
                        self.aceitar(";");
                    }
                    return Ok(Stmt::Vazio);
                }
                self.declaracao()
            }
            _ => self.comando(),
        }
    }

    /// Pula ate o texto do modulo ('x'), o `with {...}` e o `;`.
    fn pular_ate_modulo(&mut self) {
        while !self.fim() {
            let texto = matches!(self.tok_em(0), Tok::Texto(_));
            self.avancar();
            if texto {
                break;
            }
        }
        if (self.eh("with") || self.eh("assert")) && !self.linha_antes() {
            while !self.fim() && !self.aceitar("}") {
                self.avancar();
            }
        }
        self.aceitar(";");
    }

    fn lista_decl(&mut self, tipo: TipoDecl) -> Result<Stmt, ErroSintaxe> {
        let mut v = Vec::new();
        loop {
            let p = self.padrao_ligacao()?;
            let init = if self.aceitar("=") { Some(self.atribuicao()?) } else { None };
            v.push((p, init));
            if !self.aceitar(",") {
                break;
            }
        }
        Ok(Stmt::Decl(tipo, v))
    }

    /// Padrao de declaracao: nome, { } ou [ ].
    fn padrao_ligacao(&mut self) -> Result<Padrao, ErroSintaxe> {
        if self.eh("[") || self.eh("{") {
            let e = self.primaria()?;
            return self.para_padrao(e);
        }
        Ok(Padrao::Ident(self.nome()?))
    }

    fn bloco(&mut self) -> Result<Vec<Stmt>, ErroSintaxe> {
        self.esperar("{")?;
        let mut v = Vec::new();
        while !self.eh("}") {
            if self.fim() {
                return self.erro("bloco sem fim");
            }
            v.push(self.declaracao()?);
        }
        self.avancar();
        Ok(v)
    }

    fn comando(&mut self) -> Result<Stmt, ErroSintaxe> {
        match self.tok().clone() {
            Tok::Pont("{") => Ok(Stmt::Bloco(self.bloco()?)),
            Tok::Pont(";") => {
                self.avancar();
                Ok(Stmt::Vazio)
            }
            Tok::Palavra("var") => {
                self.avancar();
                let d = self.lista_decl(TipoDecl::Var)?;
                self.fim_de_comando()?;
                Ok(d)
            }
            Tok::Palavra("if") => {
                self.avancar();
                self.esperar("(")?;
                let c = self.expressao()?;
                self.esperar(")")?;
                let entao = Box::new(self.comando_ou_funcao()?);
                let senao = if self.aceitar("else") { Some(Box::new(self.comando_ou_funcao()?)) } else { None };
                Ok(Stmt::Se(c, entao, senao))
            }
            Tok::Palavra("while") => {
                self.avancar();
                self.esperar("(")?;
                let c = self.expressao()?;
                self.esperar(")")?;
                Ok(Stmt::Enquanto(c, Box::new(self.comando()?)))
            }
            Tok::Palavra("do") => {
                self.avancar();
                let corpo = Box::new(self.comando()?);
                if !self.aceitar("while") {
                    return self.erro("esperava while");
                }
                self.esperar("(")?;
                let c = self.expressao()?;
                self.esperar(")")?;
                self.aceitar(";");
                Ok(Stmt::FacaEnquanto(corpo, c))
            }
            Tok::Palavra("for") => self.para(),
            Tok::Palavra("return") => {
                self.avancar();
                let v = if self.eh(";") || self.eh("}") || self.fim() || self.linha_antes() { None } else { Some(self.expressao()?) };
                self.fim_de_comando()?;
                Ok(Stmt::Retorno(v))
            }
            Tok::Palavra("break") | Tok::Palavra("continue") => {
                let quebra = self.avancar() == Tok::Palavra("break");
                let rotulo = if !self.linha_antes() && matches!(self.tok(), Tok::Ident(_)) { Some(self.nome()?) } else { None };
                self.fim_de_comando()?;
                Ok(if quebra { Stmt::Quebra(rotulo) } else { Stmt::Continua(rotulo) })
            }
            Tok::Palavra("throw") => {
                self.avancar();
                let e = self.expressao()?;
                self.fim_de_comando()?;
                Ok(Stmt::Lancar(e))
            }
            Tok::Palavra("try") => {
                self.avancar();
                let bloco = self.bloco()?;
                let (mut param, mut captura, mut finalmente) = (None, None, None);
                if self.aceitar("catch") {
                    if self.aceitar("(") {
                        param = Some(self.padrao_ligacao()?);
                        self.esperar(")")?;
                    }
                    captura = Some(self.bloco()?);
                }
                if self.aceitar("finally") {
                    finalmente = Some(self.bloco()?);
                }
                Ok(Stmt::Tentar(bloco, param, captura, finalmente))
            }
            Tok::Palavra("switch") => {
                self.avancar();
                self.esperar("(")?;
                let d = self.expressao()?;
                self.esperar(")")?;
                self.esperar("{")?;
                let mut casos = Vec::new();
                while !self.aceitar("}") {
                    let teste = if self.aceitar("default") {
                        None
                    } else {
                        if !self.aceitar("case") {
                            return self.erro("esperava case");
                        }
                        Some(self.expressao()?)
                    };
                    self.esperar(":")?;
                    let mut corpo = Vec::new();
                    while !self.eh("case") && !self.eh("default") && !self.eh("}") {
                        corpo.push(self.declaracao()?);
                    }
                    casos.push(Caso { teste, corpo });
                }
                Ok(Stmt::Escolha(d, casos))
            }
            Tok::Palavra("debugger") => {
                self.avancar();
                self.fim_de_comando()?;
                Ok(Stmt::Vazio)
            }
            Tok::Palavra("with") => self.erro("with não é suportado"),
            Tok::Ident(n) if self.eh_em(1, ":") => {
                self.avancar();
                self.avancar();
                Ok(Stmt::Rotulo(n, Box::new(self.comando_ou_funcao()?)))
            }
            _ => {
                let e = self.expressao()?;
                self.fim_de_comando()?;
                Ok(Stmt::Expr(e))
            }
        }
    }

    fn comando_ou_funcao(&mut self) -> Result<Stmt, ErroSintaxe> {
        if self.eh("function") { self.declaracao() } else { self.comando() }
    }

    fn para(&mut self) -> Result<Stmt, ErroSintaxe> {
        self.avancar();
        let espera = self.aceitar("await");
        self.esperar("(")?;
        // inicio
        let mut tipo = None;
        if self.eh("var") {
            tipo = Some(TipoDecl::Var);
        } else if self.eh("const") {
            tipo = Some(TipoDecl::Const);
        } else if self.eh("let") && (matches!(self.tok_em(1), Tok::Ident(_)) || self.eh_em(1, "[") || self.eh_em(1, "{") || matches!(self.tok_em(1), Tok::Palavra(p) if contextual(p))) {
            tipo = Some(TipoDecl::Let);
        }
        let inicio = if let Some(td) = tipo {
            self.avancar();
            let p = self.padrao_ligacao()?;
            if self.eh("of") || self.eh("in") {
                let de = self.avancar() == Tok::Palavra("of");
                let obj = if de { self.atribuicao()? } else { self.expressao()? };
                self.esperar(")")?;
                let corpo = Box::new(self.comando()?);
                return Ok(Stmt::ParaEm(Some(td), p, obj, de, espera, corpo));
            }
            let mut v = Vec::new();
            self.sem_in = true;
            let init = if self.aceitar("=") { Some(self.atribuicao()?) } else { None };
            v.push((p, init));
            while self.aceitar(",") {
                let p = self.padrao_ligacao()?;
                let init = if self.aceitar("=") { Some(self.atribuicao()?) } else { None };
                v.push((p, init));
            }
            self.sem_in = false;
            InicioPara::Decl(td, v)
        } else if self.eh(";") {
            InicioPara::Nada
        } else {
            self.sem_in = true;
            let e = self.expressao_sem_virgula_em()?;
            self.sem_in = false;
            if self.eh("of") || self.eh("in") {
                let de = self.avancar() == Tok::Palavra("of");
                let alvo = self.para_padrao(e)?;
                let obj = if de { self.atribuicao()? } else { self.expressao()? };
                self.esperar(")")?;
                let corpo = Box::new(self.comando()?);
                return Ok(Stmt::ParaEm(None, alvo, obj, de, espera, corpo));
            }
            InicioPara::Expr(e)
        };
        self.esperar(";")?;
        let cond = if self.eh(";") { None } else { Some(self.expressao()?) };
        self.esperar(";")?;
        let passo = if self.eh(")") { None } else { Some(self.expressao()?) };
        self.esperar(")")?;
        let corpo = Box::new(self.comando()?);
        Ok(Stmt::Para(inicio, cond, passo, corpo))
    }

    fn expressao_sem_virgula_em(&mut self) -> Result<Expr, ErroSintaxe> {
        let mut v = alloc::vec![self.atribuicao()?];
        while self.aceitar(",") {
            v.push(self.atribuicao()?);
        }
        Ok(if v.len() == 1 { v.pop().unwrap_or(Expr::Nulo) } else { Expr::Sequencia(v) })
    }

    // ------------------------------------------------------------ funcoes

    fn params(&mut self) -> Result<(Vec<(Padrao, Option<Expr>)>, Option<Padrao>), ErroSintaxe> {
        self.esperar("(")?;
        let mut v = Vec::new();
        let mut resto = None;
        while !self.eh(")") {
            if self.aceitar("...") {
                resto = Some(self.padrao_ligacao()?);
                self.aceitar(",");
                break;
            }
            let p = self.padrao_ligacao()?;
            let d = if self.aceitar("=") { Some(self.atribuicao()?) } else { None };
            v.push((p, d));
            if !self.aceitar(",") {
                break;
            }
        }
        self.esperar(")")?;
        Ok((v, resto))
    }

    fn corpo_funcao(&mut self, assincrona: bool, geradora: bool) -> Result<Vec<Stmt>, ErroSintaxe> {
        let salvo = (self.em_funcao, self.em_gerador, self.em_async, self.sem_in);
        self.em_funcao = true;
        self.em_gerador = geradora;
        self.em_async = assincrona;
        self.sem_in = false;
        let r = self.bloco();
        (self.em_funcao, self.em_gerador, self.em_async, self.sem_in) = salvo;
        r
    }

    fn eh_estrito(corpo: &[Stmt]) -> bool {
        matches!(corpo.first(), Some(Stmt::Expr(Expr::Texto(s))) if &**s == "use strict")
    }

    /// "function [*] [nome] (params) { corpo }" (o "function" ainda nao lido).
    fn funcao(&mut self, assincrona: bool, expressao: bool) -> Result<Rc<Funcao>, ErroSintaxe> {
        let ini = self.t[self.i].pos;
        self.esperar("function")?;
        let geradora = self.aceitar("*");
        let nome = if self.eh("(") && expressao { None } else { Some(self.nome()?) };
        let (params, resto) = self.params()?;
        let corpo = self.corpo_funcao(assincrona, geradora)?;
        let fim = self.t[self.i.saturating_sub(1)].pos + 1;
        let estrito = Self::eh_estrito(&corpo);
        Ok(Rc::new(Funcao {
            nome,
            params,
            resto,
            corpo,
            seta: false,
            assincrona,
            geradora,
            metodo: false,
            construtor_derivado: false,
            estrito,
            fonte: (ini, fim),
        }))
    }

    fn metodo(&mut self, nome: Option<Rc<str>>, assincrona: bool, geradora: bool) -> Result<Rc<Funcao>, ErroSintaxe> {
        let ini = self.t[self.i].pos;
        let (params, resto) = self.params()?;
        let corpo = self.corpo_funcao(assincrona, geradora)?;
        let fim = self.t[self.i.saturating_sub(1)].pos + 1;
        Ok(Rc::new(Funcao {
            nome,
            params,
            resto,
            corpo,
            seta: false,
            assincrona,
            geradora,
            metodo: true,
            construtor_derivado: false,
            estrito: true,
            fonte: (ini, fim),
        }))
    }

    fn seta(&mut self, params: Vec<(Padrao, Option<Expr>)>, resto: Option<Padrao>, assincrona: bool) -> Result<Expr, ErroSintaxe> {
        let ini = self.t[self.i].pos;
        self.esperar("=>")?;
        let corpo = if self.eh("{") {
            let salvo = (self.em_funcao, self.em_gerador, self.em_async, self.sem_in);
            self.em_funcao = true;
            self.em_gerador = false;
            self.em_async = assincrona;
            self.sem_in = false;
            let r = self.bloco();
            (self.em_funcao, self.em_gerador, self.em_async, self.sem_in) = salvo;
            r?
        } else {
            let salvo = (self.em_gerador, self.em_async);
            self.em_gerador = false;
            self.em_async = assincrona;
            let e = self.atribuicao();
            (self.em_gerador, self.em_async) = salvo;
            alloc::vec![Stmt::Retorno(Some(e?))]
        };
        let fim = self.t[self.i.saturating_sub(1)].pos + 1;
        Ok(Expr::Funcao(Rc::new(Funcao {
            nome: None,
            params,
            resto,
            corpo,
            seta: true,
            assincrona,
            geradora: false,
            metodo: false,
            construtor_derivado: false,
            estrito: false,
            fonte: (ini, fim),
        })))
    }

    fn chave(&mut self) -> Result<Chave, ErroSintaxe> {
        match self.tok().clone() {
            Tok::Texto(s) => {
                self.avancar();
                Ok(Chave::Nome(s))
            }
            Tok::Num(n) => {
                self.avancar();
                Ok(Chave::Nome(Rc::from(super::valor::numero_texto(n).as_str())))
            }
            Tok::Pont("[") => {
                self.avancar();
                let e = self.atribuicao()?;
                self.esperar("]")?;
                Ok(Chave::Calculada(Box::new(e)))
            }
            Tok::PrivadoIdent(n) => {
                self.avancar();
                Ok(Chave::Privada(n))
            }
            _ => Ok(Chave::Nome(self.nome_propriedade()?)),
        }
    }

    fn classe(&mut self) -> Result<Rc<Classe>, ErroSintaxe> {
        self.esperar("class")?;
        let nome = if matches!(self.tok(), Tok::Ident(_)) { Some(self.nome()?) } else { None };
        let pai = if self.aceitar("extends") { Some(Box::new(self.lado_esquerdo()?)) } else { None };
        self.esperar("{")?;
        let mut c = Classe { nome, pai, construtor: None, membros: Vec::new(), blocos_estaticos: Vec::new() };
        while !self.aceitar("}") {
            if self.aceitar(";") {
                continue;
            }
            let mut estatico = false;
            if self.eh("static") && !self.eh_em(1, "(") && !self.eh_em(1, "=") {
                self.avancar();
                estatico = true;
                if self.eh("{") {
                    let b = self.corpo_funcao(false, false)?;
                    c.blocos_estaticos.push(b);
                    continue;
                }
            }
            let mut assincrona = false;
            let mut tipo = TipoProp::Metodo;
            if self.eh("async") && !self.eh_em(1, "(") && !self.eh_em(1, "=") && !self.t[self.i + 1].linha_antes {
                self.avancar();
                assincrona = true;
            }
            let geradora = self.aceitar("*");
            if (self.eh("get") || self.eh("set")) && !self.eh_em(1, "(") && !self.eh_em(1, "=") && !self.eh_em(1, ";") && !self.eh_em(1, "}") {
                tipo = if self.avancar() == Tok::Palavra("get") { TipoProp::Get } else { TipoProp::Set };
            }
            let chave = self.chave()?;
            if self.eh("(") {
                let nome = match &chave {
                    Chave::Nome(n) => Some(n.clone()),
                    _ => None,
                };
                let eh_construtor = !estatico && matches!(&chave, Chave::Nome(n) if &**n == "constructor") && tipo == TipoProp::Metodo;
                let mut f = self.metodo(nome, assincrona, geradora)?;
                if eh_construtor {
                    if c.pai.is_some() {
                        Rc::make_mut(&mut f).construtor_derivado = true;
                    }
                    c.construtor = Some(f);
                } else {
                    c.membros.push(MembroClasse { chave, estatico, tipo, funcao: Some(f), campo: None, eh_campo: false });
                }
            } else {
                // campo
                let valor = if self.aceitar("=") {
                    let salvo = self.em_funcao;
                    self.em_funcao = true;
                    let e = self.atribuicao();
                    self.em_funcao = salvo;
                    Some(e?)
                } else {
                    None
                };
                self.fim_de_comando()?;
                c.membros.push(MembroClasse { chave, estatico, tipo: TipoProp::Valor, funcao: None, campo: valor, eh_campo: true });
            }
        }
        Ok(Rc::new(c))
    }

    // ---------------------------------------------------------- expressoes

    pub fn expressao(&mut self) -> Result<Expr, ErroSintaxe> {
        let mut v = alloc::vec![self.atribuicao()?];
        while self.aceitar(",") {
            v.push(self.atribuicao()?);
        }
        Ok(if v.len() == 1 { v.pop().unwrap_or(Expr::Nulo) } else { Expr::Sequencia(v) })
    }

    /// Indice do ')' que fecha o '(' em `i`.
    fn fecha_paren(&self, i: usize) -> usize {
        let mut nivel = 0;
        let mut k = i;
        while k < self.t.len() {
            match &self.t[k].tok {
                Tok::Pont("(") | Tok::Pont("[") | Tok::Pont("{") => nivel += 1,
                Tok::Pont(")") | Tok::Pont("]") | Tok::Pont("}") => {
                    nivel -= 1;
                    if nivel == 0 {
                        return k;
                    }
                }
                Tok::Template(_, _, false) => nivel += 1,
                Tok::TemplateMeio(_, _, true) => nivel -= 1,
                Tok::Fim => return k,
                _ => {}
            }
            k += 1;
        }
        k
    }

    fn eh_seta_adiante(&self) -> bool {
        // "(" ... ")" "=>"  ou  ident "=>"
        if self.eh("(") {
            let f = self.fecha_paren(self.i);
            return matches!(self.t.get(f + 1).map(|t| &t.tok), Some(Tok::Pont("=>"))) && !self.t[(f + 1).min(self.t.len() - 1)].linha_antes;
        }
        false
    }

    pub fn atribuicao(&mut self) -> Result<Expr, ErroSintaxe> {
        // arrow functions
        if self.eh("async") && !self.t[self.i + 1].linha_antes {
            if matches!(self.tok_em(1), Tok::Ident(_)) && self.eh_em(2, "=>") {
                self.avancar();
                let n = self.nome()?;
                return self.seta(alloc::vec![(Padrao::Ident(n), None)], None, true);
            }
            if self.eh_em(1, "(") {
                let f = self.fecha_paren(self.i + 1);
                if matches!(self.t.get(f + 1).map(|t| &t.tok), Some(Tok::Pont("=>"))) {
                    self.avancar();
                    let (p, r) = self.params()?;
                    return self.seta(p, r, true);
                }
            }
        }
        if self.eh_seta_adiante() {
            let (p, r) = self.params()?;
            return self.seta(p, r, false);
        }
        if (matches!(self.tok(), Tok::Ident(_)) || matches!(self.tok(), Tok::Palavra(p) if contextual(p))) && self.eh_em(1, "=>") {
            let n = self.nome()?;
            return self.seta(alloc::vec![(Padrao::Ident(n), None)], None, false);
        }
        if self.eh("yield") && self.em_gerador {
            self.avancar();
            let delegar = self.aceitar("*");
            let fim = self.eh(")") || self.eh("]") || self.eh("}") || self.eh(",") || self.eh(";") || self.eh(":") || self.fim() || self.linha_antes();
            let arg = if fim && !delegar { None } else { Some(Box::new(self.atribuicao()?)) };
            return Ok(Expr::Yield(arg, delegar));
        }
        let esq = self.condicional()?;
        if let Tok::Pont(op) = self.tok().clone() {
            if ATRIBUICOES.contains(&op) {
                self.avancar();
                let alvo = if op == "=" { self.para_padrao(esq)? } else { self.alvo_simples(esq)? };
                let dir = self.atribuicao()?;
                // nome de funcao anonima: f = function(){}
                return Ok(Expr::Atrib(op, Box::new(alvo), Box::new(dir)));
            }
        }
        Ok(esq)
    }

    fn alvo_simples(&self, e: Expr) -> Result<Padrao, ErroSintaxe> {
        match e {
            Expr::Ident(n) => Ok(Padrao::Ident(n)),
            Expr::Membro { .. } | Expr::Indice { .. } | Expr::Privado { .. } | Expr::SuperMembro(_) => Ok(Padrao::Alvo(Box::new(e))),
            _ => Err(ErroSintaxe { msg: "alvo de atribuição inválido".into(), linha: self.t[self.i].linha }),
        }
    }

    /// Converte uma expressao (literal de objeto/array) em padrao.
    fn para_padrao(&self, e: Expr) -> Result<Padrao, ErroSintaxe> {
        match e {
            Expr::Array(itens) => {
                let mut v = Vec::new();
                let mut resto = None;
                let n = itens.len();
                for (k, it) in itens.into_iter().enumerate() {
                    match it {
                        None => v.push(None),
                        Some(Expr::Espalhar(x)) if k + 1 == n => resto = Some(Box::new(self.para_padrao(*x)?)),
                        Some(Expr::Atrib("=", alvo, d)) => v.push(Some((*alvo, Some(*d)))),
                        Some(x) => v.push(Some((self.para_padrao(x)?, None))),
                    }
                }
                Ok(Padrao::Array(v, resto))
            }
            Expr::Objeto(props) => {
                let mut v = Vec::new();
                let mut resto = None;
                for p in props {
                    match p {
                        Prop::Espalhar(x) => resto = Some(Box::new(self.para_padrao(x)?)),
                        Prop::Par(chave, valor, _) => match valor {
                            Expr::Atrib("=", alvo, d) => v.push((chave, *alvo, Some(*d))),
                            x => v.push((chave, self.para_padrao(x)?, None)),
                        },
                    }
                }
                Ok(Padrao::Objeto(v, resto))
            }
            Expr::Atrib("=", alvo, _) => Ok(*alvo),
            x => self.alvo_simples(x),
        }
    }

    fn condicional(&mut self) -> Result<Expr, ErroSintaxe> {
        let c = self.binario(0)?;
        if self.aceitar("?") {
            let salvo = self.sem_in;
            self.sem_in = false;
            let a = self.atribuicao()?;
            self.sem_in = salvo;
            self.esperar(":")?;
            let b = self.atribuicao()?;
            return Ok(Expr::Cond(Box::new(c), Box::new(a), Box::new(b)));
        }
        Ok(c)
    }

    fn op_binario(&self) -> Option<&'static str> {
        match self.tok() {
            Tok::Pont(p) if prioridade(p) > 0 => Some(p),
            Tok::Palavra("instanceof") => Some("instanceof"),
            Tok::Palavra("in") if !self.sem_in => Some("in"),
            _ => None,
        }
    }

    fn binario(&mut self, minimo: u8) -> Result<Expr, ErroSintaxe> {
        let mut esq = self.unario()?;
        while let Some(op) = self.op_binario() {
            let p = prioridade(op);
            if p <= minimo && !(op == "**" && p == minimo) {
                break;
            }
            if p < minimo {
                break;
            }
            self.avancar();
            // ** associa a direita
            let dir = if op == "**" { self.binario(p - 1)? } else { self.binario(p)? };
            esq = match op {
                "&&" | "||" | "??" => Expr::Logico(op, Box::new(esq), Box::new(dir)),
                _ => Expr::Binario(op, Box::new(esq), Box::new(dir)),
            };
        }
        Ok(esq)
    }

    fn unario(&mut self) -> Result<Expr, ErroSintaxe> {
        match self.tok().clone() {
            Tok::Pont(op @ ("!" | "~" | "+" | "-")) => {
                self.avancar();
                Ok(Expr::Unario(op, Box::new(self.unario()?)))
            }
            Tok::Palavra(op @ ("typeof" | "void" | "delete")) => {
                self.avancar();
                Ok(Expr::Unario(op, Box::new(self.unario()?)))
            }
            Tok::Pont(op @ ("++" | "--")) => {
                self.avancar();
                let alvo = self.unario()?;
                Ok(Expr::Atualiza { prefixo: true, mais: op == "++", alvo: Box::new(alvo) })
            }
            Tok::Palavra("await") if self.em_async || !self.em_funcao => {
                self.avancar();
                Ok(Expr::Await(Box::new(self.unario()?)))
            }
            _ => {
                let e = self.lado_esquerdo()?;
                if let Tok::Pont(op @ ("++" | "--")) = self.tok().clone() {
                    if !self.linha_antes() {
                        self.avancar();
                        return Ok(Expr::Atualiza { prefixo: false, mais: op == "++", alvo: Box::new(e) });
                    }
                }
                Ok(e)
            }
        }
    }

    fn argumentos(&mut self) -> Result<Vec<Expr>, ErroSintaxe> {
        self.esperar("(")?;
        let mut v = Vec::new();
        let salvo = self.sem_in;
        self.sem_in = false;
        while !self.eh(")") {
            if self.aceitar("...") {
                v.push(Expr::Espalhar(Box::new(self.atribuicao()?)));
            } else {
                v.push(self.atribuicao()?);
            }
            if !self.aceitar(",") {
                break;
            }
        }
        self.sem_in = salvo;
        self.esperar(")")?;
        Ok(v)
    }

    /// Chamadas, membros, new, ?. (expressoes "do lado esquerdo").
    fn lado_esquerdo(&mut self) -> Result<Expr, ErroSintaxe> {
        let mut e = if self.eh("new") {
            self.avancar();
            if self.aceitar(".") {
                let _ = self.nome_propriedade()?; // new.target
                Expr::NovoAlvo
            } else {
                let alvo = self.membro_sem_chamada()?;
                let args = if self.eh("(") { self.argumentos()? } else { Vec::new() };
                Expr::Novo(Box::new(alvo), args)
            }
        } else if self.eh("super") {
            self.avancar();
            if self.eh("(") {
                Expr::SuperChamada(self.argumentos()?)
            } else if self.aceitar(".") {
                let n = self.nome_propriedade()?;
                Expr::SuperMembro(Box::new(Expr::Texto(n)))
            } else {
                self.esperar("[")?;
                let i = self.expressao()?;
                self.esperar("]")?;
                Expr::SuperMembro(Box::new(i))
            }
        } else if self.eh("import") && self.eh_em(1, "(") {
            self.avancar();
            let mut a = self.argumentos()?;
            Expr::Importar(Box::new(a.pop().unwrap_or(Expr::Nulo)))
        } else {
            self.primaria()?
        };
        let mut opcional_na_cadeia = false;
        loop {
            if self.aceitar(".") {
                if let Tok::PrivadoIdent(n) = self.tok().clone() {
                    self.avancar();
                    e = Expr::Privado { obj: Box::new(e), nome: n };
                } else {
                    let n = self.nome_propriedade()?;
                    e = Expr::Membro { obj: Box::new(e), nome: n, opcional: false };
                }
            } else if self.eh("?.") {
                self.avancar();
                opcional_na_cadeia = true;
                if self.eh("(") {
                    let pos = self.t[self.i].pos;
                    let args = self.argumentos()?;
                    e = Expr::Chamada { f: Box::new(e), args, opcional: true, pos };
                } else if self.aceitar("[") {
                    let i = self.expressao()?;
                    self.esperar("]")?;
                    e = Expr::Indice { obj: Box::new(e), idx: Box::new(i), opcional: true };
                } else {
                    let n = self.nome_propriedade()?;
                    e = Expr::Membro { obj: Box::new(e), nome: n, opcional: true };
                }
            } else if self.eh("[") {
                self.avancar();
                let salvo = self.sem_in;
                self.sem_in = false;
                let i = self.expressao()?;
                self.sem_in = salvo;
                self.esperar("]")?;
                e = Expr::Indice { obj: Box::new(e), idx: Box::new(i), opcional: false };
            } else if self.eh("(") {
                let pos = self.t[self.i].pos;
                let args = self.argumentos()?;
                e = Expr::Chamada { f: Box::new(e), args, opcional: false, pos };
            } else if let Tok::Template(..) = self.tok() {
                let (cozidos, crus, exprs) = self.template_partes()?;
                e = Expr::TemplateTag(Box::new(e), cozidos, crus, exprs);
            } else {
                break;
            }
        }
        if opcional_na_cadeia {
            e = Expr::Opcional(Box::new(e));
        }
        Ok(e)
    }

    /// O alvo de um new: membros sem chamada.
    fn membro_sem_chamada(&mut self) -> Result<Expr, ErroSintaxe> {
        let mut e = if self.eh("new") {
            self.avancar();
            let alvo = self.membro_sem_chamada()?;
            let args = if self.eh("(") { self.argumentos()? } else { Vec::new() };
            Expr::Novo(Box::new(alvo), args)
        } else {
            self.primaria()?
        };
        loop {
            if self.aceitar(".") {
                let n = self.nome_propriedade()?;
                e = Expr::Membro { obj: Box::new(e), nome: n, opcional: false };
            } else if self.aceitar("[") {
                let i = self.expressao()?;
                self.esperar("]")?;
                e = Expr::Indice { obj: Box::new(e), idx: Box::new(i), opcional: false };
            } else {
                return Ok(e);
            }
        }
    }

    fn template_partes(&mut self) -> Result<(Vec<Option<Rc<str>>>, Vec<Rc<str>>, Vec<Expr>), ErroSintaxe> {
        let mut cozidos = Vec::new();
        let mut crus = Vec::new();
        let mut exprs = Vec::new();
        loop {
            match self.avancar() {
                Tok::Template(c, r, fim) | Tok::TemplateMeio(c, r, fim) => {
                    cozidos.push(c);
                    crus.push(r);
                    if fim {
                        return Ok((cozidos, crus, exprs));
                    }
                    exprs.push(self.expressao()?);
                }
                _ => return self.erro("template inválido"),
            }
        }
    }

    fn primaria(&mut self) -> Result<Expr, ErroSintaxe> {
        let t = self.tok().clone();
        match t {
            Tok::Num(n) => {
                self.avancar();
                Ok(Expr::Num(n))
            }
            Tok::BigInt(s) => {
                self.avancar();
                Ok(Expr::Num(s.parse::<f64>().unwrap_or(0.0)))
            }
            Tok::Texto(s) => {
                self.avancar();
                Ok(Expr::Texto(s))
            }
            Tok::Template(..) => {
                let (cozidos, _, exprs) = self.template_partes()?;
                Ok(Expr::Template(cozidos.into_iter().map(|c| c.unwrap_or_else(|| Rc::from(""))).collect(), exprs))
            }
            Tok::Regex(c, f) => {
                self.avancar();
                Ok(Expr::Regex(c, f))
            }
            Tok::Ident(n) => {
                self.avancar();
                Ok(Expr::Ident(n))
            }
            Tok::Palavra("this") => {
                self.avancar();
                Ok(Expr::This)
            }
            Tok::Palavra("true") => {
                self.avancar();
                Ok(Expr::Bool(true))
            }
            Tok::Palavra("false") => {
                self.avancar();
                Ok(Expr::Bool(false))
            }
            Tok::Palavra("null") => {
                self.avancar();
                Ok(Expr::Nulo)
            }
            Tok::Palavra("function") => Ok(Expr::Funcao(self.funcao(false, true)?)),
            Tok::Palavra("async") if self.eh_em(1, "function") => {
                self.avancar();
                Ok(Expr::Funcao(self.funcao(true, true)?))
            }
            Tok::Palavra("class") => Ok(Expr::Classe(self.classe()?)),
            Tok::Palavra(p) if contextual(p) => {
                self.avancar();
                Ok(Expr::Ident(Rc::from(p)))
            }
            Tok::Pont("(") => {
                self.avancar();
                let salvo = self.sem_in;
                self.sem_in = false;
                let e = self.expressao()?;
                self.sem_in = salvo;
                self.esperar(")")?;
                Ok(e)
            }
            Tok::Pont("[") => {
                self.avancar();
                let mut v = Vec::new();
                while !self.eh("]") {
                    if self.eh(",") {
                        self.avancar();
                        v.push(None);
                        continue;
                    }
                    if self.aceitar("...") {
                        v.push(Some(Expr::Espalhar(Box::new(self.atribuicao()?))));
                    } else {
                        v.push(Some(self.atribuicao()?));
                    }
                    if !self.eh("]") {
                        self.esperar(",")?;
                    }
                }
                self.avancar();
                Ok(Expr::Array(v))
            }
            Tok::Pont("{") => {
                self.avancar();
                let mut v = Vec::new();
                while !self.eh("}") {
                    if self.aceitar("...") {
                        v.push(Prop::Espalhar(self.atribuicao()?));
                    } else {
                        v.push(self.prop_objeto()?);
                    }
                    if !self.eh("}") {
                        self.esperar(",")?;
                    }
                }
                self.avancar();
                Ok(Expr::Objeto(v))
            }
            _ => self.erro("expressão inesperada"),
        }
    }

    fn prop_objeto(&mut self) -> Result<Prop, ErroSintaxe> {
        let mut assincrona = false;
        let mut tipo = TipoProp::Valor;
        if self.eh("async") && !self.eh_em(1, ",") && !self.eh_em(1, ":") && !self.eh_em(1, "(") && !self.eh_em(1, "}") && !self.eh_em(1, "=") {
            self.avancar();
            assincrona = true;
        }
        let geradora = self.aceitar("*");
        if (self.eh("get") || self.eh("set")) && !self.eh_em(1, ",") && !self.eh_em(1, ":") && !self.eh_em(1, "(") && !self.eh_em(1, "}") && !self.eh_em(1, "=") {
            tipo = if self.avancar() == Tok::Palavra("get") { TipoProp::Get } else { TipoProp::Set };
        }
        let abreviada = matches!(self.tok(), Tok::Ident(_)) || matches!(self.tok(), Tok::Palavra(p) if contextual(p));
        let chave = self.chave()?;
        if self.eh("(") {
            let nome = match &chave {
                Chave::Nome(n) => Some(n.clone()),
                _ => None,
            };
            let f = self.metodo(nome, assincrona, geradora)?;
            let t = if tipo == TipoProp::Valor { TipoProp::Metodo } else { tipo };
            return Ok(Prop::Par(chave, Expr::Funcao(f), t));
        }
        if self.aceitar(":") {
            let v = self.atribuicao()?;
            return Ok(Prop::Par(chave, v, TipoProp::Valor));
        }
        // {a} ou {a = 1} (este so vale em padrao)
        if abreviada {
            if let Chave::Nome(n) = &chave {
                if self.aceitar("=") {
                    let d = self.atribuicao()?;
                    return Ok(Prop::Par(chave.clone(), Expr::Atrib("=", Box::new(Padrao::Ident(n.clone())), Box::new(d)), TipoProp::Valor));
                }
                return Ok(Prop::Par(chave.clone(), Expr::Ident(n.clone()), TipoProp::Valor));
            }
        }
        self.erro("propriedade inválida")
    }
}
