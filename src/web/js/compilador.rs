//! Compilador AST -> bytecode.
//!
//! Variaveis: cada nome e resolvido na compilacao para um slot local do
//! quadro (rapido), um slot de ambiente (se alguma funcao interna usa o
//! nome: closures) ou global. A analise de captura e por nome (exagera
//! um pouco, mas nunca erra). Blocos com let/const capturados ganham
//! ambiente proprio (cada volta de laco tem o seu, como manda a norma).
//! finally e copiado em cada saida (break/continue/return) do try.

use alloc::boxed::Box;
use alloc::collections::BTreeSet;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

use super::ast::*;
use super::bytecode::{Bin, Op, Pendente, Prototipo, Un};
use super::valor::chave;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Onde {
    Local(u16),
    Amb(u16),
    Global,
}

struct Ligacao {
    nome: Rc<str>,
    onde: Onde,
}

struct Escopo {
    ligacoes: Vec<Ligacao>,
    tem_amb: bool,
    tamanho_amb: u16,
}

struct Laco {
    rotulos: Vec<Rc<str>>,
    quebras: Vec<usize>,
    continuas: Vec<usize>,
    alvo_continua: Option<usize>,
    profundidade_amb: usize,
    profundidade_tentativa: usize,
    eh_laco: bool,
    /// Iterador aberto (for-of): fechar ao sair com break.
    tem_iterador: bool,
}

struct FuncC {
    codigo: Vec<Op>,
    linhas: Vec<u32>,
    textos: Vec<Rc<str>>,
    filhos: Vec<Rc<Prototipo>>,
    n_locais: u16,
    escopos: Vec<Escopo>,
    capturadas: BTreeSet<Rc<str>>,
    lacos: Vec<Laco>,
    /// finally pendentes (corpo, profundidade de ambientes ao entrar)
    finais: Vec<(Vec<Stmt>, usize)>,
    tentativas: usize,
    global: bool,
    seta: bool,
    geradora: bool,
    assincrona: bool,
    cadeias: Vec<Vec<usize>>,
}

#[derive(Debug, Clone)]
pub struct ErroCompilacao(pub String);

type R<T> = Result<T, ErroCompilacao>;

// ------------------------------------------------- analise de nomes

fn nomes_padrao(p: &Padrao, v: &mut Vec<Rc<str>>) {
    match p {
        Padrao::Ident(n) => v.push(n.clone()),
        Padrao::Alvo(_) => {}
        Padrao::Objeto(ps, r) => {
            for (_, p, _) in ps {
                nomes_padrao(p, v);
            }
            if let Some(r) = r {
                nomes_padrao(r, v);
            }
        }
        Padrao::Array(ps, r) => {
            for (p, _) in ps.iter().flatten() {
                nomes_padrao(p, v);
            }
            if let Some(r) = r {
                nomes_padrao(r, v);
            }
        }
    }
}

/// var e funcoes declaradas (sobem para a funcao), sem entrar em funcoes.
fn vars_de(stmts: &[Stmt], v: &mut Vec<Rc<str>>, f: &mut Vec<Rc<Funcao>>) {
    for s in stmts {
        var_de(s, v, f);
    }
}

fn var_de(s: &Stmt, v: &mut Vec<Rc<str>>, f: &mut Vec<Rc<Funcao>>) {
    match s {
        Stmt::Decl(TipoDecl::Var, d) => {
            for (p, _) in d {
                nomes_padrao(p, v);
            }
        }
        Stmt::Funcao(fu) => f.push(fu.clone()),
        Stmt::Se(_, a, b) => {
            var_de(a, v, f);
            if let Some(b) = b {
                var_de(b, v, f);
            }
        }
        Stmt::Bloco(b) => vars_de(b, v, f),
        Stmt::Enquanto(_, c) | Stmt::FacaEnquanto(c, _) | Stmt::Rotulo(_, c) => var_de(c, v, f),
        Stmt::Para(i, _, _, c) => {
            if let InicioPara::Decl(TipoDecl::Var, d) = i {
                for (p, _) in d {
                    nomes_padrao(p, v);
                }
            }
            var_de(c, v, f);
        }
        Stmt::ParaEm(t, p, _, _, _, c) => {
            if *t == Some(TipoDecl::Var) {
                nomes_padrao(p, v);
            }
            var_de(c, v, f);
        }
        Stmt::Tentar(a, _, b, c) => {
            vars_de(a, v, f);
            if let Some(b) = b {
                vars_de(b, v, f);
            }
            if let Some(c) = c {
                vars_de(c, v, f);
            }
        }
        Stmt::Escolha(_, casos) => {
            for c in casos {
                vars_de(&c.corpo, v, f);
            }
        }
        _ => {}
    }
}

/// Coleta identificadores usados (dentro = ja esta numa funcao interna).
struct Coletor {
    internos: BTreeSet<Rc<str>>,
    usa_argumentos: bool,
    usa_this_super: bool,
}

impl Coletor {
    fn expr(&mut self, e: &Expr, dentro: bool) {
        match e {
            Expr::Ident(n) => {
                if dentro {
                    self.internos.insert(n.clone());
                } else if &**n == "arguments" {
                    self.usa_argumentos = true;
                }
            }
            Expr::Template(_, es) => es.iter().for_each(|x| self.expr(x, dentro)),
            Expr::TemplateTag(t, _, _, es) => {
                self.expr(t, dentro);
                es.iter().for_each(|x| self.expr(x, dentro));
            }
            Expr::Array(v) => v.iter().flatten().for_each(|x| self.expr(x, dentro)),
            Expr::Objeto(ps) => {
                for p in ps {
                    match p {
                        Prop::Par(c, v, _) => {
                            if let Chave::Calculada(e) = c {
                                self.expr(e, dentro);
                            }
                            self.expr(v, dentro);
                        }
                        Prop::Espalhar(e) => self.expr(e, dentro),
                    }
                }
            }
            Expr::Funcao(f) => self.funcao(f),
            Expr::Classe(c) => self.classe(c),
            Expr::Unario(_, a) | Expr::Espalhar(a) | Expr::Await(a) | Expr::Opcional(a) | Expr::Importar(a) => self.expr(a, dentro),
            Expr::SuperMembro(a) => {
                self.usa_this_super = true;
                self.expr(a, dentro)
            }
            Expr::Binario(_, a, b) | Expr::Logico(_, a, b) => {
                self.expr(a, dentro);
                self.expr(b, dentro);
            }
            Expr::Atrib(_, p, v) => {
                self.padrao(p, dentro);
                self.expr(v, dentro);
            }
            Expr::Atualiza { alvo, .. } => self.expr(alvo, dentro),
            Expr::Cond(a, b, c) => {
                self.expr(a, dentro);
                self.expr(b, dentro);
                self.expr(c, dentro);
            }
            Expr::Chamada { f, args, .. } => {
                self.expr(f, dentro);
                args.iter().for_each(|x| self.expr(x, dentro));
            }
            Expr::SuperChamada(args) => {
                self.usa_this_super = true;
                args.iter().for_each(|x| self.expr(x, dentro))
            }
            Expr::Novo(f, args) => {
                self.expr(f, dentro);
                args.iter().for_each(|x| self.expr(x, dentro));
            }
            Expr::Membro { obj, .. } | Expr::Privado { obj, .. } => self.expr(obj, dentro),
            Expr::Indice { obj, idx, .. } => {
                self.expr(obj, dentro);
                self.expr(idx, dentro);
            }
            Expr::Sequencia(v) => v.iter().for_each(|x| self.expr(x, dentro)),
            Expr::Yield(a, _) => {
                if let Some(a) = a {
                    self.expr(a, dentro)
                }
            }
            Expr::This => self.usa_this_super = true,
            _ => {}
        }
    }

    fn padrao(&mut self, p: &Padrao, dentro: bool) {
        match p {
            Padrao::Ident(n) => {
                if dentro {
                    self.internos.insert(n.clone());
                }
            }
            Padrao::Alvo(e) => self.expr(e, dentro),
            Padrao::Objeto(ps, r) => {
                for (c, p, d) in ps {
                    if let Chave::Calculada(e) = c {
                        self.expr(e, dentro);
                    }
                    self.padrao(p, dentro);
                    if let Some(d) = d {
                        self.expr(d, dentro);
                    }
                }
                if let Some(r) = r {
                    self.padrao(r, dentro);
                }
            }
            Padrao::Array(ps, r) => {
                for (p, d) in ps.iter().flatten() {
                    self.padrao(p, dentro);
                    if let Some(d) = d {
                        self.expr(d, dentro);
                    }
                }
                if let Some(r) = r {
                    self.padrao(r, dentro);
                }
            }
        }
    }

    fn funcao(&mut self, f: &Funcao) {
        if let Some(nomes) = &f.adiado {
            // corpo pulado pelo parser: os nomes usados la dentro
            self.internos.extend(nomes.iter().cloned());
            if f.seta && nomes.iter().any(|n| &**n == "arguments") {
                self.usa_argumentos = true;
            }
            for (p, d) in &f.params {
                self.padrao(p, true);
                if let Some(d) = d {
                    self.expr(d, true);
                }
            }
            return;
        }
        // tudo dentro de uma funcao interna conta como capturado
        let mut c = Coletor { internos: BTreeSet::new(), usa_argumentos: false, usa_this_super: false };
        for (p, d) in &f.params {
            c.padrao(p, true);
            if let Some(d) = d {
                c.expr(d, true);
            }
        }
        if let Some(r) = &f.resto {
            c.padrao(r, true);
        }
        c.stmts(&f.corpo, true);
        self.internos.extend(c.internos);
        if f.seta && c.usa_argumentos {
            self.usa_argumentos = true;
        }
    }

    fn classe(&mut self, c: &Classe) {
        if let Some(p) = &c.pai {
            self.expr(p, true);
        }
        if let Some(k) = &c.construtor {
            self.funcao(k);
        }
        for m in &c.membros {
            if let Chave::Calculada(e) = &m.chave {
                self.expr(e, true);
            }
            if let Some(f) = &m.funcao {
                self.funcao(f);
            }
            if let Some(e) = &m.campo {
                self.expr(e, true);
            }
        }
        for b in &c.blocos_estaticos {
            self.stmts(b, true);
        }
    }

    fn stmts(&mut self, v: &[Stmt], dentro: bool) {
        for s in v {
            self.stmt(s, dentro);
        }
    }

    fn stmt(&mut self, s: &Stmt, d: bool) {
        match s {
            Stmt::Expr(e) | Stmt::Lancar(e) => self.expr(e, d),
            Stmt::Decl(_, v) => {
                for (p, i) in v {
                    self.padrao(p, d);
                    if let Some(i) = i {
                        self.expr(i, d);
                    }
                }
            }
            Stmt::Funcao(f) => self.funcao(f),
            Stmt::Classe(c) => self.classe(c),
            Stmt::Retorno(e) => {
                if let Some(e) = e {
                    self.expr(e, d)
                }
            }
            Stmt::Se(c, a, b) => {
                self.expr(c, d);
                self.stmt(a, d);
                if let Some(b) = b {
                    self.stmt(b, d);
                }
            }
            Stmt::Bloco(b) => self.stmts(b, d),
            Stmt::Enquanto(c, b) | Stmt::FacaEnquanto(b, c) => {
                self.expr(c, d);
                self.stmt(b, d);
            }
            Stmt::Para(i, c, p, b) => {
                match i {
                    InicioPara::Decl(_, v) => {
                        for (pp, ii) in v {
                            self.padrao(pp, d);
                            if let Some(ii) = ii {
                                self.expr(ii, d);
                            }
                        }
                    }
                    InicioPara::Expr(e) => self.expr(e, d),
                    InicioPara::Nada => {}
                }
                if let Some(c) = c {
                    self.expr(c, d);
                }
                if let Some(p) = p {
                    self.expr(p, d);
                }
                self.stmt(b, d);
            }
            Stmt::ParaEm(_, p, o, _, _, b) => {
                self.padrao(p, d);
                self.expr(o, d);
                self.stmt(b, d);
            }
            Stmt::Tentar(a, p, b, c) => {
                self.stmts(a, d);
                if let Some(p) = p {
                    self.padrao(p, d);
                }
                if let Some(b) = b {
                    self.stmts(b, d);
                }
                if let Some(c) = c {
                    self.stmts(c, d);
                }
            }
            Stmt::Escolha(e, casos) => {
                self.expr(e, d);
                for c in casos {
                    if let Some(t) = &c.teste {
                        self.expr(t, d);
                    }
                    self.stmts(&c.corpo, d);
                }
            }
            Stmt::Rotulo(_, s) => self.stmt(s, d),
            _ => {}
        }
    }
}

fn op_bin(op: &str) -> Bin {
    match op {
        "+" => Bin::Soma,
        "-" => Bin::Sub,
        "*" => Bin::Mul,
        "/" => Bin::Div,
        "%" => Bin::Resto,
        "**" => Bin::Pot,
        "==" => Bin::Igual,
        "!=" => Bin::Diferente,
        "===" => Bin::Estrito,
        "!==" => Bin::NaoEstrito,
        "<" => Bin::Menor,
        ">" => Bin::Maior,
        "<=" => Bin::MenorIgual,
        ">=" => Bin::MaiorIgual,
        "&" => Bin::E,
        "|" => Bin::Ou,
        "^" => Bin::Xor,
        "<<" => Bin::Esq,
        ">>" => Bin::Dir,
        ">>>" => Bin::DirSemSinal,
        "instanceof" => Bin::InstanceOf,
        _ => Bin::Em,
    }
}

pub struct Compilador {
    fs: Vec<FuncC>,
    fonte: Rc<str>,
    linha: u32,
}

/// Compila agora uma funcao preguicosa (primeira chamada).
pub fn compilar_pendente(p: &Prototipo) -> R<Rc<Prototipo>> {
    let Some(pend) = &p.pendente else { return Err(ErroCompilacao("função já compilada".into())) };
    let f = super::parser::analisar_funcao(&p.fonte, p.fonte_ini, p.fonte_fim, pend.tipo, p.assincrona, p.geradora, p.nome.clone())
        .map_err(|e| ErroCompilacao(alloc::format!("{} (linha {})", e.msg, e.linha)))?;
    let mut c = Compilador { fs: Vec::new(), fonte: p.fonte.clone(), linha: 0 };
    for nivel in &pend.escopos {
        let mut fc = FuncC::nova(false, false, false, false);
        for esc in nivel {
            fc.escopos.push(Escopo {
                ligacoes: esc.iter().map(|(n, i)| Ligacao { nome: n.clone(), onde: Onde::Amb(*i) }).collect(),
                tem_amb: true,
                tamanho_amb: esc.len() as u16,
            });
        }
        c.fs.push(fc);
    }
    let mut f = f;
    {
        let m = Rc::make_mut(&mut f);
        m.metodo = p.metodo;
        m.construtor_derivado = p.derivado;
        if pend.tipo != 0 {
            m.nome = if p.nome.is_empty() { None } else { Some(p.nome.clone()) };
        }
    }
    c.compilar_funcao(&f, None)
}

/// Compila um script inteiro (topo = escopo global).
pub fn compilar_script(stmts: &[Stmt], fonte: &str) -> R<Rc<Prototipo>> {
    let mut c = Compilador { fs: Vec::new(), fonte: Rc::from(fonte), linha: 0 };
    c.fs.push(FuncC::nova(true, false, false, false));
    // nomes usados dentro de funcoes do script: blocos/lacos do topo
    // que os declaram precisam de ambiente
    let mut col = Coletor { internos: BTreeSet::new(), usa_argumentos: false, usa_this_super: false };
    col.stmts(stmts, false);
    c.f().capturadas = col.internos;
    // escopo global: var, funcoes e let/const do topo viram globais
    let mut nomes = Vec::new();
    let mut funcs = Vec::new();
    vars_de(stmts, &mut nomes, &mut funcs);
    for s in stmts {
        if let Stmt::Decl(_, d) = s {
            for (p, _) in d {
                nomes_padrao(p, &mut nomes);
            }
        }
        if let Stmt::Classe(k) = s {
            if let Some(n) = &k.nome {
                nomes.push(n.clone());
            }
        }
    }
    c.f().escopos.push(Escopo { ligacoes: Vec::new(), tem_amb: false, tamanho_amb: 0 });
    for n in nomes {
        c.f().escopos[0].ligacoes.push(Ligacao { nome: n.clone(), onde: Onde::Global });
        c.emitir(Op::DeclaraGlobal(chave(&n)));
    }
    for f in funcs {
        let n = f.nome.clone().unwrap_or_else(|| Rc::from(""));
        c.f().escopos[0].ligacoes.push(Ligacao { nome: n.clone(), onde: Onde::Global });
        c.funcao_expr(&f)?;
        c.emitir(Op::GuardaGlobal(chave(&n)));
    }
    // o valor do ultimo comando de expressao fica num local (resultado do eval)
    let resultado = c.novo_local();
    c.emitir(Op::Indef);
    c.emitir(Op::GuardaLocal(resultado));
    for s in stmts {
        if let Stmt::Expr(e) = s {
            c.expr(e)?;
            c.emitir(Op::GuardaLocal(resultado));
        } else {
            c.stmt(s)?;
        }
    }
    c.emitir(Op::CarregaLocal(resultado));
    c.emitir(Op::Retorna);
    let f = c.fs.pop().unwrap_or_else(|| unreachable!());
    Ok(Rc::new(f.prototipo(Rc::from("<script>"), 0, 0, false, false, false, false, false, (c.fonte.clone(), 0, 0))))
}

impl FuncC {
    fn nova(global: bool, seta: bool, geradora: bool, assincrona: bool) -> FuncC {
        FuncC {
            codigo: Vec::new(),
            linhas: Vec::new(),
            textos: Vec::new(),
            filhos: Vec::new(),
            n_locais: 0,
            escopos: Vec::new(),
            capturadas: BTreeSet::new(),
            lacos: Vec::new(),
            finais: Vec::new(),
            tentativas: 0,
            global,
            seta,
            geradora,
            assincrona,
            cadeias: Vec::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn prototipo(self, nome: Rc<str>, n_params: u16, amb: u16, metodo: bool, derivado: bool, classe: bool, estrito: bool, usa_args: bool, fonte: (Rc<str>, u32, u32)) -> Prototipo {
        let mut codigo = self.codigo;
        super::bytecode::fundir(&mut codigo);
        Prototipo {
            nome,
            codigo,
            textos: self.textos,
            filhos: self.filhos,
            n_locais: self.n_locais,
            n_params,
            amb_funcao: amb,
            seta: self.seta,
            geradora: self.geradora,
            assincrona: self.assincrona,
            metodo,
            derivado,
            eh_classe: classe,
            estrito,
            usa_argumentos: usa_args,
            fonte: fonte.0,
            fonte_ini: fonte.1,
            fonte_fim: fonte.2,
            linhas: self.linhas,
            pendente: None,
            pronto: core::cell::OnceCell::new(),
        }
    }
}

impl Compilador {
    fn f(&mut self) -> &mut FuncC {
        self.fs.last_mut().unwrap_or_else(|| unreachable!())
    }

    fn emitir(&mut self, op: Op) -> usize {
        let l = self.linha;
        let f = self.f();
        f.codigo.push(op);
        f.linhas.push(l);
        f.codigo.len() - 1
    }

    fn aqui(&mut self) -> usize {
        self.f().codigo.len()
    }

    fn remendar(&mut self, i: usize) {
        let alvo = self.aqui() as u32;
        self.remendar_para(i, alvo);
    }

    fn remendar_para(&mut self, i: usize, alvo: u32) {
        match &mut self.f().codigo[i] {
            Op::Salto(x) | Op::SeFalso(x) | Op::SeVerdade(x) | Op::SeFalsoMantem(x) | Op::SeVerdadeMantem(x)
            | Op::SeNaoNuloMantem(x) | Op::SeNuloSalta(x) | Op::SeNuloSalta2(x) | Op::Tenta(x) | Op::SeDefinidoMantem(x) => *x = alvo,
            _ => {}
        }
    }

    fn texto(&mut self, s: &str) -> u32 {
        let f = self.f();
        if let Some(i) = f.textos.iter().position(|t| &**t == s) {
            return i as u32;
        }
        f.textos.push(Rc::from(s));
        (f.textos.len() - 1) as u32
    }

    fn novo_local(&mut self) -> u16 {
        let f = self.f();
        f.n_locais += 1;
        f.n_locais - 1
    }

    fn profundidade_amb(&mut self) -> usize {
        self.f().escopos.iter().filter(|e| e.tem_amb).count()
    }

    // --------------------------------------------------- escopos

    /// Abre um escopo com estas declaracoes (let/const/class/params).
    fn abrir_escopo(&mut self, nomes: &[Rc<str>]) {
        let global_topo = self.f().global && self.f().escopos.len() == 1 && false;
        let _ = global_topo;
        let capt: Vec<bool> = nomes.iter().map(|n| self.f().capturadas.contains(n)).collect();
        let tem_amb = capt.iter().any(|&c| c);
        let mut esc = Escopo { ligacoes: Vec::new(), tem_amb, tamanho_amb: 0 };
        for (n, c) in nomes.iter().zip(capt) {
            if esc.ligacoes.iter().any(|l| l.nome == *n) {
                continue;
            }
            let onde = if c {
                esc.tamanho_amb += 1;
                Onde::Amb(esc.tamanho_amb - 1)
            } else {
                Onde::Local(self.novo_local())
            };
            esc.ligacoes.push(Ligacao { nome: n.clone(), onde });
        }
        if tem_amb {
            let n = esc.tamanho_amb;
            self.emitir(Op::EntraAmb(n));
        }
        self.f().escopos.push(esc);
    }

    fn fechar_escopo(&mut self) {
        if let Some(e) = self.f().escopos.pop() {
            if e.tem_amb {
                self.emitir(Op::SaiAmb);
            }
        }
    }

    /// Onde mora `nome`: (Local(slot) | Amb(saltos, idx) | Global).
    fn resolver(&mut self, nome: &str) -> (Onde, u8) {
        let mut saltos: u32 = 0;
        let nf = self.fs.len();
        for fi in (0..nf).rev() {
            let f = &self.fs[fi];
            for e in f.escopos.iter().rev() {
                if let Some(l) = e.ligacoes.iter().rev().find(|l| &*l.nome == nome) {
                    return match l.onde {
                        Onde::Local(s) if fi == nf - 1 => (Onde::Local(s), 0),
                        Onde::Local(_) => (Onde::Global, 0), // nao deveria (captura por nome cobre)
                        Onde::Amb(i) => (Onde::Amb(i), saltos.min(255) as u8),
                        Onde::Global => (Onde::Global, 0),
                    };
                }
                if e.tem_amb {
                    saltos += 1;
                }
            }
        }
        (Onde::Global, 0)
    }

    fn carregar_nome(&mut self, nome: &str) {
        match self.resolver(nome) {
            (Onde::Local(s), _) => {
                self.emitir(Op::CarregaLocal(s));
            }
            (Onde::Amb(i), sl) => {
                self.emitir(Op::CarregaAmb(sl, i));
            }
            (Onde::Global, _) => {
                if nome == "undefined" {
                    self.emitir(Op::Indef);
                } else {
                    self.emitir(Op::CarregaGlobal(chave(nome)));
                }
            }
        }
    }

    fn guardar_nome(&mut self, nome: &str) {
        match self.resolver(nome) {
            (Onde::Local(s), _) => {
                self.emitir(Op::GuardaLocal(s));
            }
            (Onde::Amb(i), sl) => {
                self.emitir(Op::GuardaAmb(sl, i));
            }
            (Onde::Global, _) => {
                self.emitir(Op::GuardaGlobal(chave(nome)));
            }
        }
    }

    // --------------------------------------------------- funcoes

    fn funcao_expr(&mut self, f: &Funcao) -> R<()> {
        // corpo pulado pelo parser: compila na primeira chamada
        let p = if f.adiado.is_some() { self.funcao_pendente(f) } else { self.compilar_funcao(f, None)? };
        let fc = self.f();
        fc.filhos.push(p);
        let i = (fc.filhos.len() - 1) as u32;
        self.emitir(Op::Closure(i));
        Ok(())
    }

    /// Funcao para compilar so quando for chamada: guarda o trecho do texto
    /// e as variaveis capturadas visiveis daqui.
    fn funcao_pendente(&mut self, f: &Funcao) -> Rc<Prototipo> {
        let escopos = self
            .fs
            .iter()
            .map(|fc| {
                fc.escopos
                    .iter()
                    .filter(|e| e.tem_amb)
                    .map(|e| e.ligacoes.iter().filter_map(|l| if let Onde::Amb(i) = l.onde { Some((l.nome.clone(), i)) } else { None }).collect())
                    .collect()
            })
            .collect();
        let tipo = if f.seta { 2 } else if f.metodo { 1 } else { 0 };
        let mut fc = FuncC::nova(false, f.seta, f.geradora, f.assincrona);
        fc.codigo.clear();
        let nome = f.nome.clone().unwrap_or_else(|| Rc::from(""));
        let mut p = fc.prototipo(nome, f.params.len() as u16, 0, f.metodo, f.construtor_derivado, false, f.estrito, false, (self.fonte.clone(), f.fonte.0, f.fonte.1));
        p.pendente = Some(alloc::boxed::Box::new(Pendente { tipo, escopos }));
        Rc::new(p)
    }

    /// Compila uma funcao (com `campos`: codigo extra no comeco, para
    /// inicializar campos de classe no construtor base).
    fn compilar_funcao(&mut self, f: &Funcao, nome_forcado: Option<Rc<str>>) -> R<Rc<Prototipo>> {
        let mut col = Coletor { internos: BTreeSet::new(), usa_argumentos: false, usa_this_super: false };
        for (p, d) in &f.params {
            col.padrao(p, false);
            if let Some(d) = d {
                col.expr(d, false);
            }
        }
        col.stmts(&f.corpo, false);
        let mut fc = FuncC::nova(false, f.seta, f.geradora, f.assincrona);
        fc.capturadas = col.internos;
        let usa_args = col.usa_argumentos && !f.seta;
        self.fs.push(fc);
        // escopo da funcao: parametros, var, funcoes, arguments
        let mut nomes: Vec<Rc<str>> = Vec::new();
        for (p, _) in &f.params {
            nomes_padrao(p, &mut nomes);
        }
        if let Some(r) = &f.resto {
            nomes_padrao(r, &mut nomes);
        }
        let mut vars = Vec::new();
        let mut funcs = Vec::new();
        vars_de(&f.corpo, &mut vars, &mut funcs);
        nomes.extend(vars);
        for fu in &funcs {
            if let Some(n) = &fu.nome {
                nomes.push(n.clone());
            }
        }
        if usa_args {
            nomes.push(Rc::from("arguments"));
        }
        // let/const/class do corpo moram no escopo da funcao
        for s in &f.corpo {
            match s {
                Stmt::Decl(TipoDecl::Let | TipoDecl::Const, d) => {
                    for (p, _) in d {
                        nomes_padrao(p, &mut nomes);
                    }
                }
                Stmt::Classe(c) => {
                    if let Some(n) = &c.nome {
                        nomes.push(n.clone());
                    }
                }
                _ => {}
            }
        }
        // o nome da propria funcao (expressao nomeada) fica visivel dentro
        let nome_proprio = f.nome.clone().filter(|n| !nomes.contains(n));
        if let Some(n) = &nome_proprio {
            nomes.push(n.clone());
        }
        // o primeiro escopo e o da funcao: o EntraAmb dele vem no comeco
        self.abrir_escopo(&nomes);
        let amb = self.f().escopos[0].tamanho_amb;
        // tira o EntraAmb (a VM cria o ambiente da funcao sozinha)
        if amb > 0 {
            self.f().codigo.clear();
            self.f().linhas.clear();
        }
        if let Some(n) = &nome_proprio {
            self.emitir(Op::PropriaFuncao);
            self.guardar_nome(n);
        }
        if usa_args {
            self.emitir(Op::Argumentos);
            self.guardar_nome("arguments");
        }
        let n_params = f.params.len() as u16;
        for (k, (p, d)) in f.params.iter().enumerate() {
            self.emitir(Op::Argumento(k as u16));
            if let Some(d) = d {
                let pula = self.emitir(Op::SeDefinidoMantem(0));
                self.expr(d)?;
                self.remendar(pula);
            }
            self.ligar_padrao(p, true)?;
        }
        if let Some(r) = &f.resto {
            self.emitir(Op::RestoArgs(n_params));
            self.ligar_padrao(r, true)?;
        }
        for fu in &funcs {
            self.funcao_expr(fu)?;
            if let Some(n) = &fu.nome {
                let n = n.clone();
                self.emitir(Op::NomeFuncao(chave(&n)));
                self.guardar_nome(&n);
            }
        }
        for s in &f.corpo {
            self.stmt(s)?;
        }
        self.emitir(Op::Indef);
        self.emitir(Op::Retorna);
        let fc = self.fs.pop().unwrap_or_else(|| unreachable!());
        let nome = nome_forcado.or_else(|| f.nome.clone()).unwrap_or_else(|| Rc::from(""));
        let fonte = (self.fonte.clone(), f.fonte.0, f.fonte.1);
        Ok(Rc::new(fc.prototipo(nome, n_params, amb, f.metodo, f.construtor_derivado, false, f.estrito, usa_args, fonte)))
    }

    // --------------------------------------------------- padroes

    /// O valor no topo da pilha vai para o padrao (e sai da pilha).
    /// `declarar`: e uma declaracao (nomes ja existem no escopo).
    fn ligar_padrao(&mut self, p: &Padrao, declarar: bool) -> R<()> {
        let _ = declarar;
        match p {
            Padrao::Ident(n) => {
                self.guardar_nome(n);
            }
            Padrao::Alvo(e) => match &**e {
                Expr::Membro { obj, nome, .. } => {
                    self.expr(obj)?;
                    self.emitir(Op::Troca);
                    self.emitir(Op::Poe(chave(nome)));
                    self.emitir(Op::Pop);
                }
                Expr::Indice { obj, idx, .. } => {
                    self.expr(obj)?;
                    self.expr(idx)?;
                    self.emitir(Op::Rot3);
                    self.emitir(Op::PoeIdx);
                    self.emitir(Op::Pop);
                }
                Expr::Ident(n) => {
                    self.guardar_nome(n);
                }
                Expr::Privado { obj, nome } => {
                    self.expr(obj)?;
                    self.emitir(Op::Troca);
                    self.emitir(Op::PoePrivado(chave(&alloc::format!("#{nome}"))));
                    self.emitir(Op::Pop);
                }
                _ => return Err(ErroCompilacao("alvo inválido".into())),
            },
            Padrao::Objeto(ps, resto) => {
                let mut chaves_usadas: Vec<u32> = Vec::new();
                for (c, alvo, d) in ps {
                    self.emitir(Op::Dup);
                    match c {
                        Chave::Nome(n) => {
                            chaves_usadas.push(chave(n));
                            self.emitir(Op::Pega(chave(n)));
                        }
                        Chave::Calculada(e) => {
                            self.expr(e)?;
                            self.emitir(Op::PegaIdx);
                        }
                        Chave::Privada(_) => return Err(ErroCompilacao("privado em padrão".into())),
                    }
                    if let Some(d) = d {
                        let pula = self.emitir(Op::SeDefinidoMantem(0));
                        self.expr(d)?;
                        self.remendar(pula);
                    }
                    self.ligar_padrao(alvo, declarar)?;
                }
                if let Some(r) = resto {
                    let i = self.texto(&chaves_usadas.iter().map(|c| alloc::format!("{c},")).collect::<String>());
                    self.emitir(Op::RestoObjeto(i));
                    self.ligar_padrao(r, declarar)?;
                }
                self.emitir(Op::Pop);
            }
            Padrao::Array(ps, resto) => {
                self.emitir(Op::PegaIter);
                for item in ps {
                    self.emitir(Op::ProxIter); // [it v feito]
                    self.emitir(Op::Pop);
                    match item {
                        None => {
                            self.emitir(Op::Pop);
                        }
                        Some((alvo, d)) => {
                            if let Some(d) = d {
                                let pula = self.emitir(Op::SeDefinidoMantem(0));
                                self.expr(d)?;
                                self.remendar(pula);
                            }
                            self.ligar_padrao(alvo, declarar)?;
                        }
                    }
                }
                if let Some(r) = resto {
                    // junta o resto num array
                    self.emitir(Op::NovoArray(0));
                    self.emitir(Op::Troca);
                    // [arr it]: espalha o iterador restante no array
                    self.emitir(Op::ArrayEspalha);
                    self.ligar_padrao(r, declarar)?;
                } else {
                    self.emitir(Op::FechaIter);
                }
            }
        }
        Ok(())
    }

    // --------------------------------------------------- comandos

    fn bloco(&mut self, v: &[Stmt]) -> R<()> {
        let mut nomes = Vec::new();
        for s in v {
            match s {
                Stmt::Decl(TipoDecl::Let | TipoDecl::Const, d) => {
                    for (p, _) in d {
                        nomes_padrao(p, &mut nomes);
                    }
                }
                Stmt::Classe(c) => {
                    if let Some(n) = &c.nome {
                        nomes.push(n.clone());
                    }
                }
                _ => {}
            }
        }
        // (o topo do script nao passa por aqui: compilar_script declara os globais)
        if nomes.is_empty() {

            for s in v {
                self.stmt(s)?;
            }
            return Ok(());
        }
        self.abrir_escopo(&nomes);
        for s in v {
            self.stmt(s)?;
        }
        self.fechar_escopo();
        Ok(())
    }

    fn stmt(&mut self, s: &Stmt) -> R<()> {
        match s {
            Stmt::Expr(e) => {
                self.expr(e)?;
                self.emitir(Op::Pop);
            }
            Stmt::Decl(_, d) => {
                for (p, init) in d {
                    match init {
                        Some(e) => {
                            self.expr(e)?;
                            if let (Padrao::Ident(n), Expr::Funcao(_) | Expr::Classe(_)) = (p, e) {
                                self.emitir(Op::NomeFuncao(chave(n)));
                            }
                        }
                        None => {
                            if matches!(p, Padrao::Ident(_)) {
                                // "var x;" nao zera um x que ja existe
                                if let (Padrao::Ident(n), true) = (p, true) {
                                    let _ = n;
                                }
                            }
                            self.emitir(Op::Indef);
                        }
                    }
                    if init.is_none() {
                        // var sem valor: so declara (nao sobrescreve)
                        if let Stmt::Decl(TipoDecl::Var, _) = s {
                            self.emitir(Op::Pop);
                            continue;
                        }
                    }
                    self.ligar_padrao(p, true)?;
                }
            }
            Stmt::Funcao(_) => {} // ja criada no comeco da funcao
            Stmt::Classe(c) => {
                self.classe(c)?;
                let n = c.nome.clone().unwrap_or_else(|| Rc::from(""));
                self.guardar_nome(&n);
            }
            Stmt::Retorno(e) => {
                match e {
                    Some(e) => self.expr(e)?,
                    None => {
                        self.emitir(Op::Indef);
                    }
                }
                if !self.f().finais.is_empty() {
                    let t = self.novo_local();
                    self.emitir(Op::GuardaLocal(t));
                    self.sair_finais(0, true)?;
                    self.emitir(Op::CarregaLocal(t));
                }
                self.emitir(Op::Retorna);
            }
            Stmt::Se(c, a, b) => {
                self.expr(c)?;
                let sf = self.emitir(Op::SeFalso(0));
                self.stmt(a)?;
                if let Some(b) = b {
                    let fim = self.emitir(Op::Salto(0));
                    self.remendar(sf);
                    self.stmt(b)?;
                    self.remendar(fim);
                } else {
                    self.remendar(sf);
                }
            }
            Stmt::Bloco(b) => self.bloco(b)?,
            Stmt::Enquanto(c, corpo) => {
                let inicio = self.aqui();
                self.expr(c)?;
                let sai = self.emitir(Op::SeFalso(0));
                self.laco(Vec::new(), Some(inicio), |cc| cc.stmt(corpo))?;
                self.emitir(Op::Salto(inicio as u32));
                self.remendar(sai);
                self.fechar_laco();
            }
            Stmt::FacaEnquanto(corpo, c) => {
                let inicio = self.aqui();
                self.laco(Vec::new(), None, |cc| cc.stmt(corpo))?;
                let cont = self.aqui();
                self.remendar_continuas(cont);
                self.expr(c)?;
                self.emitir(Op::SeVerdade(inicio as u32));
                self.fechar_laco();
            }
            Stmt::Para(ini, cond, passo, corpo) => self.para(ini, cond.as_ref(), passo.as_ref(), corpo, Vec::new())?,
            Stmt::ParaEm(td, p, obj, de, espera, corpo) => self.para_em(*td, p, obj, *de, *espera, corpo, Vec::new())?,
            Stmt::Rotulo(n, s) => {
                // rotulos em lacos viram os do laco
                let mut rotulos = alloc::vec![n.clone()];
                let mut alvo: &Stmt = s;
                while let Stmt::Rotulo(n2, s2) = alvo {
                    rotulos.push(n2.clone());
                    alvo = s2;
                }
                match alvo {
                    Stmt::Para(ini, c, p, corpo) => self.para(ini, c.as_ref(), p.as_ref(), corpo, rotulos)?,
                    Stmt::ParaEm(td, p, o, de, es, corpo) => self.para_em(*td, p, o, *de, *es, corpo, rotulos)?,
                    Stmt::Enquanto(c, corpo) => {
                        let inicio = self.aqui();
                        self.expr(c)?;
                        let sai = self.emitir(Op::SeFalso(0));
                        self.laco(rotulos, Some(inicio), |cc| cc.stmt(corpo))?;
                        self.emitir(Op::Salto(inicio as u32));
                        self.remendar(sai);
                        self.fechar_laco();
                    }
                    outro => {
                        // bloco rotulado: so break
                        let prof = self.profundidade_amb();
                        let pt = self.f().tentativas;
                        self.f().lacos.push(Laco {
                            rotulos,
                            quebras: Vec::new(),
                            continuas: Vec::new(),
                            alvo_continua: None,
                            profundidade_amb: prof,
                            profundidade_tentativa: pt,
                            eh_laco: false,
                            tem_iterador: false,
                        });
                        self.stmt(outro)?;
                        self.fechar_laco();
                    }
                }
            }
            Stmt::Quebra(r) | Stmt::Continua(r) => {
                let quebra = matches!(s, Stmt::Quebra(_));
                let lacos = &self.f().lacos;
                let alvo = lacos.iter().rposition(|l| match r {
                    Some(n) => l.rotulos.contains(n),
                    None => l.eh_laco || (quebra && l.rotulos.is_empty() && !l.eh_laco && false) || (quebra && !l.eh_laco && l.rotulos.is_empty()),
                });
                let Some(k) = alvo else { return Err(ErroCompilacao("break/continue fora de laço".into())) };
                let (prof, pt, iter) = {
                    let l = &self.f().lacos[k];
                    (l.profundidade_amb, l.profundidade_tentativa, l.tem_iterador)
                };
                // sai dos tries e finais no meio do caminho
                let finais_ate = self.f().finais.len() - (self.f().tentativas.saturating_sub(pt)).min(self.f().finais.len());
                let _ = finais_ate;
                self.sair_finais(pt, false)?;
                let atual = self.profundidade_amb();
                for _ in prof..atual {
                    self.emitir(Op::SaiAmb);
                }
                // break de for-of fecha o iterador (que esta na pilha)
                let i = if quebra {
                    if iter {
                        self.emitir(Op::FechaIter);
                        self.emitir(Op::Indef);
                    }
                    self.emitir(Op::Salto(0))
                } else {
                    self.emitir(Op::Salto(0))
                };
                if quebra {
                    self.f().lacos[k].quebras.push(i);
                } else {
                    self.f().lacos[k].continuas.push(i);
                }
                // "reabre" os ambientes para o codigo que segue (morto) ficar coerente
                let _ = atual;
            }
            Stmt::Lancar(e) => {
                self.expr(e)?;
                self.emitir(Op::Lanca);
            }
            Stmt::Tentar(bloco, param, captura, finalmente) => self.tentar(bloco, param.as_ref(), captura.as_ref(), finalmente.as_ref())?,
            Stmt::Escolha(d, casos) => {
                self.expr(d)?;
                let prof = self.profundidade_amb();
                let pt = self.f().tentativas;
                self.f().lacos.push(Laco {
                    rotulos: Vec::new(),
                    quebras: Vec::new(),
                    continuas: Vec::new(),
                    alvo_continua: None,
                    profundidade_amb: prof,
                    profundidade_tentativa: pt,
                    eh_laco: false,
                    tem_iterador: false,
                });
                // let/const de todos os casos num escopo so
                let mut nomes = Vec::new();
                for c in casos {
                    for s in &c.corpo {
                        if let Stmt::Decl(TipoDecl::Let | TipoDecl::Const, d) = s {
                            for (p, _) in d {
                                nomes_padrao(p, &mut nomes);
                            }
                        }
                    }
                }
                let t = self.novo_local();
                self.emitir(Op::GuardaLocal(t));
                self.abrir_escopo(&nomes);
                let mut saltos = Vec::new();
                for c in casos {
                    if let Some(teste) = &c.teste {
                        self.emitir(Op::CarregaLocal(t));
                        self.expr(teste)?;
                        self.emitir(Op::Bin(Bin::Estrito));
                        saltos.push(Some(self.emitir(Op::SeVerdade(0))));
                    } else {
                        saltos.push(None);
                    }
                }
                let padrao = self.emitir(Op::Salto(0));
                let mut tem_padrao = false;
                for (k, c) in casos.iter().enumerate() {
                    match saltos[k] {
                        Some(i) => self.remendar(i),
                        None => {
                            tem_padrao = true;
                            self.remendar(padrao);
                        }
                    }
                    for s in &c.corpo {
                        self.stmt(s)?;
                    }
                }
                if !tem_padrao {
                    self.remendar(padrao);
                }
                self.fechar_escopo();
                let fim = self.aqui();
                let l = self.f().lacos.pop().unwrap_or_else(|| unreachable!());
                for q in l.quebras {
                    self.remendar_para(q, fim as u32);
                }
            }
            Stmt::Vazio => {}
        }
        Ok(())
    }

    /// Copia os finally pendentes (de dentro para fora) ate a profundidade
    /// de tentativas `ate`, fechando os handlers.
    fn sair_finais(&mut self, ate: usize, tudo: bool) -> R<()> {
        let n = self.f().tentativas;
        let mut k = n;
        while k > ate || (tudo && k > 0) {
            k -= 1;
            self.emitir(Op::FimTenta);
            // o finally desta tentativa (se tinha), sem ele mesmo na pilha
            let idx = k;
            let fin = self.f().finais.get(idx).map(|f| f.0.clone());
            if let Some(corpo) = fin {
                let salvo_f = self.f().finais.split_off(idx);
                let salvo_t = self.f().tentativas;
                self.f().tentativas = idx;
                for s in &corpo {
                    self.stmt(s)?;
                }
                self.f().tentativas = salvo_t;
                self.f().finais.extend(salvo_f);
            }
            if !tudo && k <= ate {
                break;
            }
        }
        Ok(())
    }

    fn tentar(&mut self, bloco: &[Stmt], param: Option<&Padrao>, captura: Option<&Vec<Stmt>>, finalmente: Option<&Vec<Stmt>>) -> R<()> {
        let fin: Vec<Stmt> = finalmente.cloned().unwrap_or_default();
        let prof = self.profundidade_amb();
        // tentativa 1: o bloco
        let t1 = self.emitir(Op::Tenta(0));
        self.f().tentativas += 1;
        self.f().finais.push((fin.clone(), prof));
        self.bloco(bloco)?;
        self.f().tentativas -= 1;
        self.f().finais.pop();
        self.emitir(Op::FimTenta);
        for s in &fin {
            self.stmt(s)?;
        }
        let fim = self.emitir(Op::Salto(0));
        self.remendar(t1);
        // [excecao] na pilha
        if let Some(cap) = captura {
            let t2 = if finalmente.is_some() {
                let t = self.emitir(Op::Tenta(0));
                self.f().tentativas += 1;
                self.f().finais.push((fin.clone(), prof));
                Some(t)
            } else {
                None
            };
            let mut nomes = Vec::new();
            if let Some(p) = param {
                nomes_padrao(p, &mut nomes);
            }
            self.abrir_escopo(&nomes);
            match param {
                Some(p) => self.ligar_padrao(p, true)?,
                None => {
                    self.emitir(Op::Pop);
                }
            }
            for s in cap {
                self.stmt(s)?;
            }
            self.fechar_escopo();
            if let Some(t2) = t2 {
                self.f().tentativas -= 1;
                self.f().finais.pop();
                self.emitir(Op::FimTenta);
                for s in &fin {
                    self.stmt(s)?;
                }
                let fim2 = self.emitir(Op::Salto(0));
                self.remendar(t2);
                // excecao dentro do catch: finally e relanca
                let e = self.novo_local();
                self.emitir(Op::GuardaLocal(e));
                for s in &fin {
                    self.stmt(s)?;
                }
                self.emitir(Op::CarregaLocal(e));
                self.emitir(Op::Lanca);
                self.remendar(fim2);
            }
        } else {
            // sem catch: finally e relanca
            let e = self.novo_local();
            self.emitir(Op::GuardaLocal(e));
            for s in &fin {
                self.stmt(s)?;
            }
            self.emitir(Op::CarregaLocal(e));
            self.emitir(Op::Lanca);
        }
        self.remendar(fim);
        Ok(())
    }

    fn laco(&mut self, rotulos: Vec<Rc<str>>, continua: Option<usize>, corpo: impl FnOnce(&mut Self) -> R<()>) -> R<()> {
        let prof = self.profundidade_amb();
        let pt = self.f().tentativas;
        self.f().lacos.push(Laco {
            rotulos,
            quebras: Vec::new(),
            continuas: Vec::new(),
            alvo_continua: continua,
            profundidade_amb: prof,
            profundidade_tentativa: pt,
            eh_laco: true,
            tem_iterador: false,
        });
        corpo(self)?;
        if let Some(c) = continua {
            let l = self.f().lacos.last_mut().unwrap_or_else(|| unreachable!());
            let cs = core::mem::take(&mut l.continuas);
            for i in cs {
                self.remendar_para(i, c as u32);
            }
        }
        Ok(())
    }

    fn remendar_continuas(&mut self, alvo: usize) {
        let l = self.f().lacos.last_mut().unwrap_or_else(|| unreachable!());
        let cs = core::mem::take(&mut l.continuas);
        for i in cs {
            self.remendar_para(i, alvo as u32);
        }
    }

    fn fechar_laco(&mut self) {
        let fim = self.aqui() as u32;
        let l = self.f().lacos.pop().unwrap_or_else(|| unreachable!());
        for q in l.quebras {
            self.remendar_para(q, fim);
        }
        for c in l.continuas {
            self.remendar_para(c, fim);
        }
    }

    fn para(&mut self, ini: &InicioPara, cond: Option<&Expr>, passo: Option<&Expr>, corpo: &Stmt, rotulos: Vec<Rc<str>>) -> R<()> {
        let mut nomes = Vec::new();
        let mut eh_let = false;
        if let InicioPara::Decl(TipoDecl::Let | TipoDecl::Const, d) = ini {
            for (p, _) in d {
                nomes_padrao(p, &mut nomes);
            }
            eh_let = true;
        }
        let topo_global = self.f().global && self.f().escopos.len() == 1;
        let abriu = eh_let && !(topo_global && false);
        if abriu {
            self.abrir_escopo(&nomes);
        }
        match ini {
            InicioPara::Decl(t, d) => self.stmt(&Stmt::Decl(*t, d.clone()))?,
            InicioPara::Expr(e) => {
                self.expr(e)?;
                self.emitir(Op::Pop);
            }
            InicioPara::Nada => {}
        }
        let copia = abriu && self.f().escopos.last().is_some_and(|e| e.tem_amb);
        let inicio = self.aqui();
        let sai = match cond {
            Some(c) => {
                self.expr(c)?;
                Some(self.emitir(Op::SeFalso(0)))
            }
            None => None,
        };
        self.laco(rotulos, None, |cc| cc.stmt(corpo))?;
        let cont = self.aqui();
        self.remendar_continuas(cont);
        if copia {
            self.emitir(Op::CopiaAmb);
        }
        if let Some(p) = passo {
            self.expr(p)?;
            self.emitir(Op::Pop);
        }
        self.emitir(Op::Salto(inicio as u32));
        if let Some(s) = sai {
            self.remendar(s);
        }
        self.fechar_laco();
        if abriu {
            self.fechar_escopo();
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn para_em(&mut self, td: Option<TipoDecl>, p: &Padrao, obj: &Expr, de: bool, espera: bool, corpo: &Stmt, rotulos: Vec<Rc<str>>) -> R<()> {
        self.expr(obj)?;
        if de {
            self.emitir(if espera { Op::PegaIterAssinc } else { Op::PegaIter });
        } else {
            self.emitir(Op::ChavesEm);
        }
        let inicio = self.aqui();
        self.emitir(Op::ProxIter); // [it v feito]
        let sai = self.emitir(Op::SeVerdade(0));
        if espera {
            self.emitir(Op::Await);
        }
        let mut nomes = Vec::new();
        let novo = matches!(td, Some(TipoDecl::Let) | Some(TipoDecl::Const));
        if novo {
            nomes_padrao(p, &mut nomes);
            self.abrir_escopo(&nomes);
        }
        self.ligar_padrao(p, td.is_some())?;
        let prof = self.profundidade_amb();
        let pt = self.f().tentativas;
        let menos = if novo && self.f().escopos.last().is_some_and(|e| e.tem_amb) { 1 } else { 0 };
        self.f().lacos.push(Laco {
            rotulos,
            quebras: Vec::new(),
            continuas: Vec::new(),
            alvo_continua: None,
            profundidade_amb: prof - menos,
            profundidade_tentativa: pt,
            eh_laco: true,
            tem_iterador: de,
        });
        self.stmt(corpo)?;
        let cont = self.aqui();
        self.remendar_continuas(cont);
        if novo {
            self.fechar_escopo();
        }
        self.emitir(Op::Salto(inicio as u32));
        self.remendar(sai);
        // [it v]: tira o valor; o iterador sai depois do laco
        self.emitir(Op::Pop);
        let fim = self.aqui() as u32;
        let l = self.f().lacos.pop().unwrap_or_else(|| unreachable!());
        // break ja fechou o iterador e deixou um indefinido no lugar do valor
        for q in l.quebras {
            self.remendar_para(q, fim);
        }
        self.emitir(Op::Pop);
        Ok(())
    }

    // --------------------------------------------------- classes

    fn classe(&mut self, c: &Classe) -> R<()> {
        if let Some(p) = &c.pai {
            self.expr(p)?;
        }
        // nome da classe visivel dentro dela
        let nome = c.nome.clone().unwrap_or_else(|| Rc::from(""));
        // construtor
        let ctor = match &c.construtor {
            Some(f) => f.clone(),
            None => {
                let corpo = if c.pai.is_some() {
                    alloc::vec![Stmt::Expr(Expr::SuperChamada(alloc::vec![Expr::Espalhar(Box::new(Expr::Ident(Rc::from("arguments"))))]))]
                } else {
                    Vec::new()
                };
                Rc::new(Funcao {
                    nome: Some(nome.clone()),
                    params: Vec::new(),
                    resto: None,
                    corpo,
                    seta: false,
                    assincrona: false,
                    geradora: false,
                    metodo: true,
                    construtor_derivado: c.pai.is_some(),
                    estrito: true,
                    fonte: (0, 0),
                    adiado: None,
                })
            }
        };
        // construtor de classe aninhada: o parser pulou o corpo; le agora
        let ctor = match &ctor.adiado {
            Some(_) => {
                let mut f = super::parser::analisar_funcao(&self.fonte, ctor.fonte.0, ctor.fonte.1, 1, ctor.assincrona, ctor.geradora, Rc::from("constructor"))
                    .map_err(|e| ErroCompilacao(alloc::format!("{} (linha {})", e.msg, e.linha)))?;
                Rc::make_mut(&mut f).construtor_derivado = ctor.construtor_derivado;
                f
            }
            None => ctor,
        };
        let mut p = self.compilar_funcao(&ctor, Some(nome.clone()))?;
        if let Some(pp) = Rc::get_mut(&mut p) {
            pp.eh_classe = true;
        }
        self.f().filhos.push(p);
        let i = (self.f().filhos.len() - 1) as u32;
        self.emitir(Op::Classe(i, c.pai.is_some()));
        // [F proto]: o nome da classe ja vale dentro dela (static, metodos)
        if let Some(n) = &c.nome {
            self.emitir(Op::Troca);
            self.emitir(Op::Dup);
            self.guardar_nome(n);
            self.emitir(Op::Troca);
        }
        for m in &c.membros {
            if m.eh_campo && !m.estatico {
                continue;
            }
            if m.eh_campo {
                // campo estatico: F.chave = valor (this = F)
                self.emitir(Op::Troca); // [proto F]
                self.emitir(Op::Dup); // [proto F F]
                match &m.chave {
                    Chave::Nome(n) => {
                        match &m.campo {
                            Some(e) => self.expr(e)?,
                            None => {
                                self.emitir(Op::Indef);
                            }
                        }
                        self.emitir(Op::DefineProp(chave(n)));
                    }
                    Chave::Calculada(k) => {
                        self.expr(k)?;
                        match &m.campo {
                            Some(e) => self.expr(e)?,
                            None => {
                                self.emitir(Op::Indef);
                            }
                        }
                        self.emitir(Op::DefinePropIdx);
                    }
                    Chave::Privada(n) => {
                        match &m.campo {
                            Some(e) => self.expr(e)?,
                            None => {
                                self.emitir(Op::Indef);
                            }
                        }
                        self.emitir(Op::DefineProp(chave(&alloc::format!("#{n}"))));
                    }
                }
                self.emitir(Op::Pop); // [proto F]
                self.emitir(Op::Troca); // [F proto]
                continue;
            }
            let f = m.funcao.as_ref().ok_or(ErroCompilacao("membro sem função".into()))?;
            let tipo = match m.tipo {
                TipoProp::Get => 1,
                TipoProp::Set => 2,
                _ => 0,
            };
            match &m.chave {
                Chave::Nome(n) => {
                    self.funcao_expr(f)?;
                    self.emitir(Op::DefineMetodo(chave(n), m.estatico, tipo));
                }
                Chave::Privada(n) => {
                    self.funcao_expr(f)?;
                    self.emitir(Op::DefineMetodo(chave(&alloc::format!("#{n}")), m.estatico, tipo));
                }
                Chave::Calculada(k) => {
                    self.expr(k)?;
                    self.funcao_expr(f)?;
                    self.emitir(Op::DefineMetodoIdx(m.estatico, tipo));
                }
            }
        }
        // campos de instancia: uma funcao inicializadora (this = objeto novo)
        let campos: Vec<&MembroClasse> = c.membros.iter().filter(|m| m.eh_campo && !m.estatico).collect();
        if !campos.is_empty() {
            let mut corpo = Vec::new();
            for m in campos {
                let alvo = match &m.chave {
                    Chave::Nome(n) => Expr::Membro { obj: Box::new(Expr::This), nome: n.clone(), opcional: false },
                    Chave::Privada(n) => Expr::Privado { obj: Box::new(Expr::This), nome: n.clone() },
                    Chave::Calculada(k) => Expr::Indice { obj: Box::new(Expr::This), idx: k.clone(), opcional: false },
                };
                corpo.push(Stmt::Expr(Expr::Atrib(
                    "=",
                    Box::new(Padrao::Alvo(Box::new(alvo))),
                    Box::new(m.campo.clone().unwrap_or(Expr::Ident(Rc::from("undefined")))),
                )));
            }
            let f = Funcao {
                nome: Some(Rc::from("<campos>")),
                params: Vec::new(),
                resto: None,
                corpo,
                seta: false,
                assincrona: false,
                geradora: false,
                metodo: true,
                construtor_derivado: false,
                estrito: true,
                fonte: (0, 0),
                adiado: None,
            };
            self.funcao_expr(&f)?;
            self.emitir(Op::DefineCampos);
        }
        // blocos estaticos: chamados com this = F
        for b in &c.blocos_estaticos {
            let f = Funcao {
                nome: None,
                params: Vec::new(),
                resto: None,
                corpo: b.clone(),
                seta: false,
                assincrona: false,
                geradora: false,
                metodo: true,
                construtor_derivado: false,
                estrito: true,
                fonte: (0, 0),
                adiado: None,
            };
            self.emitir(Op::Troca); // [proto F]
            self.emitir(Op::Dup);
            self.funcao_expr(&f)?;
            self.emitir(Op::Troca); // [proto F f F]
            self.emitir(Op::Chama(0));
            self.emitir(Op::Pop);
            self.emitir(Op::Troca);
        }
        self.emitir(Op::Pop); // [F]
        Ok(())
    }

    // --------------------------------------------------- expressoes

    fn args(&mut self, args: &[Expr]) -> R<Option<u8>> {
        if args.iter().any(|a| matches!(a, Expr::Espalhar(_))) {
            self.array(&args.iter().map(|a| Some(a.clone())).collect::<Vec<_>>())?;
            return Ok(None);
        }
        for a in args {
            self.expr(a)?;
        }
        Ok(Some(args.len().min(255) as u8))
    }

    fn array(&mut self, itens: &[Option<Expr>]) -> R<()> {
        let simples = itens.iter().all(|i| matches!(i, Some(e) if !matches!(e, Expr::Espalhar(_))));
        if simples && itens.len() < 256 {
            for e in itens.iter().flatten() {
                self.expr(e)?;
            }
            self.emitir(Op::NovoArray(itens.len() as u32));
            return Ok(());
        }
        self.emitir(Op::NovoArray(0));
        for it in itens {
            match it {
                None => {
                    self.emitir(Op::ArrayBuraco);
                }
                Some(Expr::Espalhar(e)) => {
                    self.expr(e)?;
                    self.emitir(Op::PegaIter);
                    self.emitir(Op::ArrayEspalha);
                }
                Some(e) => {
                    self.expr(e)?;
                    self.emitir(Op::ArrayEmpurra);
                }
            }
        }
        Ok(())
    }

    fn atribuicao(&mut self, op: &str, alvo: &Padrao, valor: &Expr) -> R<()> {
        if op == "=" {
            match alvo {
                Padrao::Ident(n) => {
                    self.expr(valor)?;
                    if matches!(valor, Expr::Funcao(_) | Expr::Classe(_)) {
                        self.emitir(Op::NomeFuncao(chave(n)));
                    }
                    self.emitir(Op::Dup);
                    self.guardar_nome(n);
                }
                Padrao::Alvo(e) => match &**e {
                    Expr::Membro { obj, nome, .. } => {
                        self.expr(obj)?;
                        self.expr(valor)?;
                        self.emitir(Op::Poe(chave(nome)));
                    }
                    Expr::Indice { obj, idx, .. } => {
                        self.expr(obj)?;
                        self.expr(idx)?;
                        self.emitir(Op::ParaChave);
                        self.expr(valor)?;
                        self.emitir(Op::PoeIdx);
                    }
                    Expr::Privado { obj, nome } => {
                        self.expr(obj)?;
                        self.expr(valor)?;
                        self.emitir(Op::PoePrivado(chave(&alloc::format!("#{nome}"))));
                    }
                    Expr::SuperMembro(k) => {
                        self.emitir(Op::This);
                        self.expr(k)?;
                        self.expr(valor)?;
                        self.emitir(Op::PoeIdx);
                    }
                    _ => return Err(ErroCompilacao("alvo inválido".into())),
                },
                p => {
                    self.expr(valor)?;
                    self.emitir(Op::Dup);
                    self.ligar_padrao(p, false)?;
                }
            }
            return Ok(());
        }
        // operadores compostos (+=, ||=, ...)
        let logico = matches!(op, "&&=" | "||=" | "??=");
        let base = &op[..op.len() - 1];
        let aplicar = |cc: &mut Self| -> R<Option<usize>> {
            if logico {
                let j = match base {
                    "&&" => cc.emitir(Op::SeFalsoMantem(0)),
                    "||" => cc.emitir(Op::SeVerdadeMantem(0)),
                    _ => cc.emitir(Op::SeNaoNuloMantem(0)),
                };
                cc.expr(valor)?;
                Ok(Some(j))
            } else {
                cc.expr(valor)?;
                cc.emitir(Op::Bin(op_bin(base)));
                Ok(None)
            }
        };
        match alvo {
            Padrao::Ident(n) => {
                self.carregar_nome(n);
                let j = aplicar(self)?;
                self.emitir(Op::Dup);
                self.guardar_nome(n);
                if let Some(j) = j {
                    let fim = self.emitir(Op::Salto(0));
                    self.remendar(j);
                    self.remendar(fim);
                }
            }
            Padrao::Alvo(e) => match &**e {
                Expr::Membro { obj, nome, .. } => {
                    self.expr(obj)?;
                    self.emitir(Op::Dup);
                    self.emitir(Op::Pega(chave(nome)));
                    let j = aplicar(self)?;
                    self.emitir(Op::Poe(chave(nome)));
                    if let Some(j) = j {
                        let fim = self.emitir(Op::Salto(0));
                        self.remendar(j);
                        // [obj atual]: tira o obj
                        self.emitir(Op::Troca);
                        self.emitir(Op::Pop);
                        self.remendar(fim);
                    }
                }
                Expr::Indice { obj, idx, .. } => {
                    self.expr(obj)?;
                    self.expr(idx)?;
                    self.emitir(Op::ParaChave);
                    self.emitir(Op::Dup2);
                    self.emitir(Op::PegaIdx);
                    let j = aplicar(self)?;
                    self.emitir(Op::PoeIdx);
                    if let Some(j) = j {
                        let fim = self.emitir(Op::Salto(0));
                        self.remendar(j);
                        self.emitir(Op::Rot3);
                        self.emitir(Op::Pop);
                        self.emitir(Op::Pop);
                        self.remendar(fim);
                    }
                }
                Expr::Privado { obj, nome } => {
                    let k = chave(&alloc::format!("#{nome}"));
                    self.expr(obj)?;
                    self.emitir(Op::Dup);
                    self.emitir(Op::PegaPrivado(k));
                    let j = aplicar(self)?;
                    self.emitir(Op::PoePrivado(k));
                    if let Some(j) = j {
                        let fim = self.emitir(Op::Salto(0));
                        self.remendar(j);
                        self.emitir(Op::Troca);
                        self.emitir(Op::Pop);
                        self.remendar(fim);
                    }
                }
                _ => return Err(ErroCompilacao("alvo inválido".into())),
            },
            _ => return Err(ErroCompilacao("alvo inválido".into())),
        }
        Ok(())
    }

    fn chamada(&mut self, f: &Expr, args: &[Expr], opcional: bool) -> R<()> {
        // empilha [funcao this]
        match f {
            Expr::Membro { obj, nome, opcional: o2 } => {
                self.expr(obj)?;
                if *o2 {
                    let j = self.emitir(Op::SeNuloSalta(0));
                    self.f().cadeias.last_mut().map(|c| c.push(j));
                }
                self.emitir(Op::Dup);
                self.emitir(Op::Pega(chave(nome)));
                self.emitir(Op::Troca);
            }
            Expr::Indice { obj, idx, opcional: o2 } => {
                self.expr(obj)?;
                if *o2 {
                    let j = self.emitir(Op::SeNuloSalta(0));
                    self.f().cadeias.last_mut().map(|c| c.push(j));
                }
                self.emitir(Op::Dup);
                self.expr(idx)?;
                self.emitir(Op::PegaIdx);
                self.emitir(Op::Troca);
            }
            Expr::Privado { obj, nome } => {
                self.expr(obj)?;
                self.emitir(Op::Dup);
                self.emitir(Op::PegaPrivado(chave(&alloc::format!("#{nome}"))));
                self.emitir(Op::Troca);
            }
            Expr::SuperMembro(k) => {
                self.expr(k)?;
                self.emitir(Op::PegaSuperIdx);
                self.emitir(Op::This);
            }
            _ => {
                self.expr(f)?;
                self.emitir(Op::Indef);
            }
        }
        if opcional {
            let j = self.emitir(Op::SeNuloSalta2(0));
            self.f().cadeias.last_mut().map(|c| c.push(j));
        }
        match self.args(args)? {
            Some(n) => {
                self.emitir(Op::Chama(n));
            }
            None => {
                self.emitir(Op::ChamaEspalhado);
            }
        }
        Ok(())
    }

    fn expr(&mut self, e: &Expr) -> R<()> {
        match e {
            Expr::Num(n) => {
                self.emitir(Op::Num(*n));
            }
            Expr::Texto(s) => {
                let i = self.texto(s);
                self.emitir(Op::Texto(i));
            }
            Expr::Template(partes, exprs) => {
                let mut n = 0u16;
                for (k, p) in partes.iter().enumerate() {
                    if !p.is_empty() {
                        let i = self.texto(p);
                        self.emitir(Op::Texto(i));
                        n += 1;
                    }
                    if let Some(x) = exprs.get(k) {
                        self.expr(x)?;
                        n += 1;
                    }
                }
                if n == 0 {
                    let i = self.texto("");
                    self.emitir(Op::Texto(i));
                } else {
                    self.emitir(Op::Template(n));
                }
            }
            Expr::TemplateTag(tag, cozidos, crus, exprs) => {
                // tag(strings, ...valores); strings.raw = crus
                self.expr(tag)?;
                self.emitir(Op::Indef);
                for c in cozidos {
                    match c {
                        Some(s) => {
                            let i = self.texto(s);
                            self.emitir(Op::Texto(i));
                        }
                        None => {
                            self.emitir(Op::Indef);
                        }
                    }
                }
                self.emitir(Op::NovoArray(cozidos.len() as u32));
                for c in crus {
                    let i = self.texto(c);
                    self.emitir(Op::Texto(i));
                }
                self.emitir(Op::NovoArray(crus.len() as u32));
                self.emitir(Op::DefineProp(chave("raw")));
                for x in exprs {
                    self.expr(x)?;
                }
                self.emitir(Op::Chama((exprs.len() + 1).min(255) as u8));
            }
            Expr::Regex(c, f) => {
                let a = self.texto(c);
                let b = self.texto(f);
                self.emitir(Op::Regex(a, b));
            }
            Expr::Ident(n) => self.carregar_nome(n),
            Expr::This => {
                self.emitir(Op::This);
            }
            Expr::NovoAlvo => {
                self.emitir(Op::NovoAlvo);
            }
            Expr::Bool(b) => {
                self.emitir(Op::Bool(*b));
            }
            Expr::Nulo => {
                self.emitir(Op::Nulo);
            }
            Expr::Array(v) => self.array(v)?,
            Expr::Objeto(ps) => {
                self.emitir(Op::NovoObjeto);
                for p in ps {
                    match p {
                        Prop::Espalhar(x) => {
                            self.expr(x)?;
                            self.emitir(Op::CopiaProps);
                        }
                        Prop::Par(c, v, tipo) => {
                            let ac = match tipo {
                                TipoProp::Get => Some(0),
                                TipoProp::Set => Some(1),
                                _ => None,
                            };
                            match c {
                                Chave::Nome(n) => {
                                    if n.as_ref() == "__proto__" && *tipo == TipoProp::Valor {
                                        self.expr(v)?;
                                        self.emitir(Op::DefineProp(chave("__proto__")));
                                        continue;
                                    }
                                    self.expr(v)?;
                                    if matches!(v, Expr::Funcao(_) | Expr::Classe(_)) && ac.is_none() {
                                        self.emitir(Op::NomeFuncao(chave(n)));
                                    }
                                    match ac {
                                        Some(a) => self.emitir(Op::DefineAcessor(chave(n), a)),
                                        None => self.emitir(Op::DefineProp(chave(n))),
                                    };
                                }
                                Chave::Calculada(k) => {
                                    self.expr(k)?;
                                    self.expr(v)?;
                                    match ac {
                                        Some(a) => self.emitir(Op::DefineAcessorIdx(a)),
                                        None => self.emitir(Op::DefinePropIdx),
                                    };
                                }
                                Chave::Privada(_) => return Err(ErroCompilacao("privado em objeto".into())),
                            }
                        }
                    }
                }
            }
            Expr::Funcao(f) => self.funcao_expr(f)?,
            Expr::Classe(c) => self.classe(c)?,
            Expr::Unario(op, a) => match *op {
                "delete" => match &**a {
                    Expr::Membro { obj, nome, .. } => {
                        self.expr(obj)?;
                        self.emitir(Op::Apaga(chave(nome)));
                    }
                    Expr::Indice { obj, idx, .. } => {
                        self.expr(obj)?;
                        self.expr(idx)?;
                        self.emitir(Op::ApagaIdx);
                    }
                    _ => {
                        self.expr(a)?;
                        self.emitir(Op::Pop);
                        self.emitir(Op::Bool(true));
                    }
                },
                "typeof" => {
                    if let Expr::Ident(n) = &**a {
                        if let (Onde::Global, _) = self.resolver(n) {
                            self.emitir(Op::TypeofGlobal(chave(n)));
                            return Ok(());
                        }
                    }
                    self.expr(a)?;
                    self.emitir(Op::Un(Un::Typeof));
                }
                _ => {
                    self.expr(a)?;
                    self.emitir(Op::Un(match *op {
                        "-" => Un::Neg,
                        "+" => Un::Mais,
                        "!" => Un::Nao,
                        "~" => Un::Til,
                        _ => Un::Void,
                    }));
                }
            },
            Expr::Binario(op, a, b) => {
                self.expr(a)?;
                self.expr(b)?;
                self.emitir(Op::Bin(op_bin(op)));
            }
            Expr::Logico(op, a, b) => {
                self.expr(a)?;
                let j = match *op {
                    "&&" => self.emitir(Op::SeFalsoMantem(0)),
                    "||" => self.emitir(Op::SeVerdadeMantem(0)),
                    _ => self.emitir(Op::SeNaoNuloMantem(0)),
                };
                self.expr(b)?;
                self.remendar(j);
            }
            Expr::Atrib(op, alvo, v) => self.atribuicao(op, alvo, v)?,
            Expr::Atualiza { prefixo, mais, alvo } => {
                let delta = if *mais { Bin::Soma } else { Bin::Sub };
                match &**alvo {
                    Expr::Ident(n) => {
                        self.carregar_nome(n);
                        self.emitir(Op::ParaNumero);
                        if !prefixo {
                            self.emitir(Op::Dup);
                        }
                        self.emitir(Op::Num(1.0));
                        self.emitir(Op::Bin(delta));
                        if *prefixo {
                            self.emitir(Op::Dup);
                        }
                        self.guardar_nome(n);
                    }
                    Expr::Membro { obj, nome, .. } => {
                        self.expr(obj)?;
                        self.emitir(Op::Dup);
                        self.emitir(Op::Pega(chave(nome)));
                        self.emitir(Op::ParaNumero);
                        if *prefixo {
                            self.emitir(Op::Num(1.0));
                            self.emitir(Op::Bin(delta));
                            self.emitir(Op::Poe(chave(nome)));
                        } else {
                            self.emitir(Op::Dup); // [obj v v]
                            self.emitir(Op::Rot3); // [v obj v]
                            self.emitir(Op::Num(1.0));
                            self.emitir(Op::Bin(delta));
                            self.emitir(Op::Poe(chave(nome))); // [v novo]
                            self.emitir(Op::Pop);
                        }
                    }
                    Expr::Indice { obj, idx, .. } => {
                        self.expr(obj)?;
                        self.expr(idx)?;
                        self.emitir(Op::ParaChave);
                        self.emitir(Op::Dup2);
                        self.emitir(Op::PegaIdx);
                        self.emitir(Op::ParaNumero);
                        if *prefixo {
                            self.emitir(Op::Num(1.0));
                            self.emitir(Op::Bin(delta));
                            self.emitir(Op::PoeIdx);
                        } else {
                            self.emitir(Op::Dup); // [o k v v]
                            self.emitir(Op::Rot4); // [v o k v]
                            self.emitir(Op::Num(1.0));
                            self.emitir(Op::Bin(delta));
                            self.emitir(Op::PoeIdx); // [v novo]
                            self.emitir(Op::Pop);
                        }
                    }
                    Expr::Privado { .. } => {
                        // this.#x++: vira this.#x += 1 (o objeto e avaliado de novo)
                        let um = Expr::Num(1.0);
                        let op = if *mais { "+=" } else { "-=" };
                        let p = Padrao::Alvo(alvo.clone());
                        if *prefixo {
                            self.atribuicao(op, &p, &um)?;
                        } else {
                            self.expr(alvo)?;
                            self.emitir(Op::ParaNumero);
                            self.atribuicao(op, &p, &um)?;
                            self.emitir(Op::Pop);
                        }
                    }
                    _ => return Err(ErroCompilacao("alvo de ++/-- inválido".into())),
                }
            }
            Expr::Cond(c, a, b) => {
                self.expr(c)?;
                let sf = self.emitir(Op::SeFalso(0));
                self.expr(a)?;
                let fim = self.emitir(Op::Salto(0));
                self.remendar(sf);
                self.expr(b)?;
                self.remendar(fim);
            }
            Expr::Chamada { f, args, opcional, pos } => {
                self.linha = *pos;
                self.chamada(f, args, *opcional)?
            }
            Expr::Novo(f, args) => {
                self.expr(f)?;
                match self.args(args)? {
                    Some(n) => self.emitir(Op::Novo(n)),
                    None => self.emitir(Op::NovoEspalhado),
                };
            }
            Expr::Membro { obj, nome, opcional } => {
                self.expr(obj)?;
                if *opcional {
                    let j = self.emitir(Op::SeNuloSalta(0));
                    self.f().cadeias.last_mut().map(|c| c.push(j));
                }
                self.emitir(Op::Pega(chave(nome)));
            }
            Expr::Indice { obj, idx, opcional } => {
                self.expr(obj)?;
                if *opcional {
                    let j = self.emitir(Op::SeNuloSalta(0));
                    self.f().cadeias.last_mut().map(|c| c.push(j));
                }
                self.expr(idx)?;
                self.emitir(Op::PegaIdx);
            }
            Expr::Privado { obj, nome } => {
                self.expr(obj)?;
                self.emitir(Op::PegaPrivado(chave(&alloc::format!("#{nome}"))));
            }
            Expr::Opcional(e) => {
                self.f().cadeias.push(Vec::new());
                self.expr(e)?;
                let saltos = self.f().cadeias.pop().unwrap_or_default();
                for j in saltos {
                    self.remendar(j);
                }
            }
            Expr::SuperMembro(k) => {
                self.expr(k)?;
                self.emitir(Op::PegaSuperIdx);
            }
            Expr::SuperChamada(args) => {
                match self.args(args)? {
                    Some(n) => self.emitir(Op::ChamaSuper(n)),
                    None => self.emitir(Op::ChamaSuperEspalhado),
                };
            }
            Expr::Sequencia(v) => {
                for (k, x) in v.iter().enumerate() {
                    self.expr(x)?;
                    if k + 1 < v.len() {
                        self.emitir(Op::Pop);
                    }
                }
            }
            Expr::Espalhar(_) => return Err(ErroCompilacao("... fora de lugar".into())),
            Expr::Yield(a, delegar) => {
                match a {
                    Some(a) => self.expr(a)?,
                    None => {
                        self.emitir(Op::Indef);
                    }
                }
                if *delegar {
                    // yield*: repassa cada valor do iterador interno
                    self.emitir(Op::PegaIter);
                    let laco = self.aqui();
                    self.emitir(Op::ProxIter);
                    let fim = self.emitir(Op::SeVerdade(0));
                    self.emitir(Op::Yield);
                    self.emitir(Op::Pop);
                    self.emitir(Op::Salto(laco as u32));
                    self.remendar(fim);
                    self.emitir(Op::Troca);
                    self.emitir(Op::Pop);
                } else {
                    self.emitir(Op::Yield);
                }
            }
            Expr::Await(a) => {
                self.expr(a)?;
                self.emitir(Op::Await);
            }
            Expr::Importar(a) => {
                // import() dinamico num script classico: o carregador de modulos
                // (__importarDe(base, especificador) -> promessa)
                self.emitir(Op::CarregaGlobal(chave("__importarDe")));
                self.emitir(Op::Indef);
                self.emitir(Op::Indef);
                self.expr(a)?;
                self.emitir(Op::Chama(2));
            }
        }
        Ok(())
    }
}
