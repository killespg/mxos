//! Arvore sintatica do JavaScript.

use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::vec::Vec;

#[derive(Clone, Debug, PartialEq)]
pub enum Chave {
    Nome(Rc<str>),
    Calculada(Box<Expr>),
    Privada(Rc<str>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TipoProp {
    Valor,
    Get,
    Set,
    Metodo,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Prop {
    Par(Chave, Expr, TipoProp),
    Espalhar(Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Padrao {
    Ident(Rc<str>),
    /// Alvo de atribuicao qualquer (a.b, a[c]) — so em atribuicoes.
    Alvo(Box<Expr>),
    Objeto(Vec<(Chave, Padrao, Option<Expr>)>, Option<Box<Padrao>>),
    Array(Vec<Option<(Padrao, Option<Expr>)>>, Option<Box<Padrao>>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Funcao {
    pub nome: Option<Rc<str>>,
    pub params: Vec<(Padrao, Option<Expr>)>,
    pub resto: Option<Padrao>,
    pub corpo: Vec<Stmt>,
    pub seta: bool,
    pub assincrona: bool,
    pub geradora: bool,
    pub metodo: bool,
    pub construtor_derivado: bool,
    pub estrito: bool,
    pub fonte: (u32, u32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct MembroClasse {
    pub chave: Chave,
    pub estatico: bool,
    pub tipo: TipoProp,
    /// Metodo/get/set: Funcao; campo: valor inicial.
    pub funcao: Option<Rc<Funcao>>,
    pub campo: Option<Expr>,
    pub eh_campo: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Classe {
    pub nome: Option<Rc<str>>,
    pub pai: Option<Box<Expr>>,
    pub construtor: Option<Rc<Funcao>>,
    pub membros: Vec<MembroClasse>,
    pub blocos_estaticos: Vec<Vec<Stmt>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Num(f64),
    Texto(Rc<str>),
    Template(Vec<Rc<str>>, Vec<Expr>),
    TemplateTag(Box<Expr>, Vec<Option<Rc<str>>>, Vec<Rc<str>>, Vec<Expr>),
    Regex(Rc<str>, Rc<str>),
    Ident(Rc<str>),
    This,
    SuperMembro(Box<Expr>),
    SuperChamada(Vec<Expr>),
    NovoAlvo,
    Bool(bool),
    Nulo,
    Array(Vec<Option<Expr>>),
    Objeto(Vec<Prop>),
    Funcao(Rc<Funcao>),
    Classe(Rc<Classe>),
    Unario(&'static str, Box<Expr>),
    Binario(&'static str, Box<Expr>, Box<Expr>),
    Logico(&'static str, Box<Expr>, Box<Expr>),
    /// op: "=", "+=", ...; destino como padrao
    Atrib(&'static str, Box<Padrao>, Box<Expr>),
    Atualiza { prefixo: bool, mais: bool, alvo: Box<Expr> },
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    Chamada { f: Box<Expr>, args: Vec<Expr>, opcional: bool },
    Novo(Box<Expr>, Vec<Expr>),
    Membro { obj: Box<Expr>, nome: Rc<str>, opcional: bool },
    Indice { obj: Box<Expr>, idx: Box<Expr>, opcional: bool },
    Privado { obj: Box<Expr>, nome: Rc<str> },
    /// Fim de uma cadeia com ?. (curto-circuito ate aqui)
    Opcional(Box<Expr>),
    Sequencia(Vec<Expr>),
    Espalhar(Box<Expr>),
    Yield(Option<Box<Expr>>, bool),
    Await(Box<Expr>),
    Importar(Box<Expr>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TipoDecl {
    Var,
    Let,
    Const,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Caso {
    pub teste: Option<Expr>,
    pub corpo: Vec<Stmt>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InicioPara {
    Nada,
    Decl(TipoDecl, Vec<(Padrao, Option<Expr>)>),
    Expr(Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    Expr(Expr),
    Decl(TipoDecl, Vec<(Padrao, Option<Expr>)>),
    Funcao(Rc<Funcao>),
    Classe(Rc<Classe>),
    Retorno(Option<Expr>),
    Se(Expr, Box<Stmt>, Option<Box<Stmt>>),
    Bloco(Vec<Stmt>),
    Enquanto(Expr, Box<Stmt>),
    FacaEnquanto(Box<Stmt>, Expr),
    Para(InicioPara, Option<Expr>, Option<Expr>, Box<Stmt>),
    /// for (decl in/of obj): (tipo da decl ou None para alvo existente, padrao, obj, de?, await?, corpo)
    ParaEm(Option<TipoDecl>, Padrao, Expr, bool, bool, Box<Stmt>),
    Quebra(Option<Rc<str>>),
    Continua(Option<Rc<str>>),
    Lancar(Expr),
    Tentar(Vec<Stmt>, Option<Padrao>, Option<Vec<Stmt>>, Option<Vec<Stmt>>),
    Escolha(Expr, Vec<Caso>),
    Rotulo(Rc<str>, Box<Stmt>),
    Vazio,
}
