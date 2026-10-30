//! O bytecode da maquina virtual (de pilha) e o prototipo de funcao
//! compilada.

use alloc::rc::Rc;
use alloc::vec::Vec;

use super::valor::Chave;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bin {
    Soma,
    Sub,
    Mul,
    Div,
    Resto,
    Pot,
    Igual,
    Diferente,
    Estrito,
    NaoEstrito,
    Menor,
    Maior,
    MenorIgual,
    MaiorIgual,
    E,
    Ou,
    Xor,
    Esq,
    Dir,
    DirSemSinal,
    InstanceOf,
    Em,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Un {
    Neg,
    Mais,
    Nao,
    Til,
    Typeof,
    Void,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Indef,
    Nulo,
    Bool(bool),
    Num(f64),
    Texto(u32),
    Pop,
    Dup,
    Dup2,
    Troca,
    /// [a b c] -> [c a b]
    Rot3,
    /// [a b c d] -> [d a b c]
    Rot4,
    CarregaLocal(u16),
    /// Argumento k da chamada (indefinido se faltou).
    Argumento(u16),
    /// Os argumentos a partir de k, num array (...resto).
    RestoArgs(u16),
    /// A propria funcao em execucao (nome de expressao de funcao).
    PropriaFuncao,
    /// Valor padrao: se o topo NAO e indefinido, salta mantendo; senao tira.
    SeDefinidoMantem(u32),
    GuardaLocal(u16),
    CarregaAmb(u8, u16),
    GuardaAmb(u8, u16),
    CarregaGlobal(Chave),
    GuardaGlobal(Chave),
    DeclaraGlobal(Chave),
    TypeofGlobal(Chave),
    EntraAmb(u16),
    SaiAmb,
    CopiaAmb,
    This,
    NovoAlvo,
    Argumentos,
    Pega(Chave),
    PegaIdx,
    Poe(Chave),
    PoeIdx,
    Apaga(Chave),
    ApagaIdx,
    PegaSuper(Chave),
    PegaSuperIdx,
    ChamaSuper(u8),
    ChamaSuperEspalhado,
    PegaPrivado(Chave),
    PoePrivado(Chave),
    Bin(Bin),
    Un(Un),
    ParaNumero,
    ParaChave,
    Salto(u32),
    SeFalso(u32),
    SeVerdade(u32),
    /// Se falso: salta mantendo; senao tira e segue (&&).
    SeFalsoMantem(u32),
    SeVerdadeMantem(u32),
    /// ??: se NAO nulo, salta mantendo; senao tira.
    SeNaoNuloMantem(u32),
    /// ?.: se o topo e nulo/indefinido, troca por indefinido e salta.
    SeNuloSalta(u32),
    /// ?.(): [f this] — se f e nulo, tira os dois, poe indefinido e salta.
    SeNuloSalta2(u32),
    /// [f this args...] -> resultado
    Chama(u8),
    /// [f this array] -> resultado
    ChamaEspalhado,
    Novo(u8),
    NovoEspalhado,
    Retorna,
    Closure(u32),
    /// [pai?] -> [F proto]  (indice do construtor, tem pai)
    Classe(u32, bool),
    /// [F proto valor] -> [F proto]; (chave, estatico, tipo 0=metodo 1=get 2=set)
    DefineMetodo(Chave, bool, u8),
    /// [F proto chave valor] -> [F proto]
    DefineMetodoIdx(bool, u8),
    /// [F proto inicializador] -> [F proto]
    DefineCampos,
    NovoObjeto,
    NovoArray(u32),
    ArrayEmpurra,
    ArrayEspalha,
    ArrayBuraco,
    /// [obj v] -> [obj]
    DefineProp(Chave),
    /// [obj k v] -> [obj]
    DefinePropIdx,
    /// [obj f] -> [obj] (0=get 1=set)
    DefineAcessor(Chave, u8),
    DefineAcessorIdx(u8),
    /// [obj fonte] -> [obj]
    CopiaProps,
    /// [obj fonte] -> [obj resto] (resto sem as chaves listadas antes: usa o vetor do topo)
    RestoObjeto(u32),
    Lanca,
    Tenta(u32),
    FimTenta,
    PegaIter,
    PegaIterAssinc,
    /// [iter] -> [iter valor feito]
    ProxIter,
    FechaIter,
    ChavesEm,
    Yield,
    YieldDelegado,
    Await,
    Template(u16),
    Regex(u32, u32),
    /// Nome da funcao no topo (para funcoes anonimas atribuidas)
    NomeFuncao(Chave),
    Debug,
    // ---- super-instrucoes (ver `fundir`): ficam no lugar da primeira de
    // uma sequencia comum, que continua inteira logo depois. No caso rapido
    // (numeros) fazem a sequencia toda e pulam o resto; senao fazem so o que
    // a primeira faria, e o resto roda normal. Um salto para o meio da
    // sequencia cai nas instrucoes originais: nada muda de sentido.
    /// Dup, GuardaLocal(k), Pop (`x = ...;` como comando)
    GuardaLocalDescarta(u16),
    /// CarregaLocal(i), ParaNumero, Dup, Num(1), Bin(Soma), GuardaLocal(i), Pop (`i++;`)
    IncLocal(u16),
    /// CarregaLocal(i), Num(c), Bin(op), SeFalso(alvo) (`i < c` num laco)
    CompLocalNum(u16, Bin, f64, u32),
    /// Num(c), Bin(op)
    BinNum(Bin, f64),
    /// CarregaLocal(a), CarregaLocal(b)
    Locais2(u16, u16),
}

/// Troca as sequencias mais comuns por super-instrucoes (no lugar da
/// primeira; as originais ficam). Corta pela metade as instrucoes de um
/// laco tipico: no interpretador o que pesa e o despacho de cada uma.
pub fn fundir(c: &mut [Op]) {
    let mut i = 0;
    while i < c.len() {
        let n = match &c[i..] {
            [Op::CarregaLocal(a), Op::ParaNumero, Op::Dup, Op::Num(um), Op::Bin(Bin::Soma), Op::GuardaLocal(b), Op::Pop, ..] if a == b && *um == 1.0 => {
                c[i] = Op::IncLocal(*a);
                7
            }
            [Op::CarregaLocal(a), Op::Num(v), Op::Bin(op), Op::SeFalso(alvo), ..]
                if matches!(op, Bin::Menor | Bin::Maior | Bin::MenorIgual | Bin::MaiorIgual | Bin::Estrito | Bin::NaoEstrito) =>
            {
                c[i] = Op::CompLocalNum(*a, *op, *v, *alvo);
                4
            }
            [Op::Dup, Op::GuardaLocal(k), Op::Pop, ..] => {
                c[i] = Op::GuardaLocalDescarta(*k);
                3
            }
            [Op::Num(v), Op::Bin(op), ..] if !matches!(op, Bin::InstanceOf | Bin::Em) => {
                c[i] = Op::BinNum(*op, *v);
                2
            }
            [Op::CarregaLocal(a), Op::CarregaLocal(b), ..] => {
                c[i] = Op::Locais2(*a, *b);
                2
            }
            _ => 1,
        };
        i += n;
    }
}

#[derive(Debug)]
pub struct Prototipo {
    pub nome: Rc<str>,
    pub codigo: Vec<Op>,
    pub textos: Vec<Rc<str>>,
    pub filhos: Vec<Rc<Prototipo>>,
    pub n_locais: u16,
    pub n_params: u16,
    /// Tamanho do ambiente da funcao (variaveis capturadas); 0 = sem.
    pub amb_funcao: u16,
    pub seta: bool,
    pub geradora: bool,
    pub assincrona: bool,
    pub metodo: bool,
    pub derivado: bool,
    pub eh_classe: bool,
    pub estrito: bool,
    pub usa_argumentos: bool,
    /// O texto inteiro de onde a funcao veio e o trecho dela (toString e
    /// compilacao preguicosa).
    pub fonte: Rc<str>,
    pub fonte_ini: u32,
    pub fonte_fim: u32,
    /// Posicao no texto de cada op (para mensagens de erro).
    pub linhas: Vec<u32>,
    /// Ainda nao compilada: o corpo e lido de novo do texto na primeira
    /// chamada (a maioria das funcoes de um bundle nunca roda).
    pub pendente: Option<alloc::boxed::Box<Pendente>>,
    pub pronto: core::cell::OnceCell<Rc<Prototipo>>,
}

/// O que falta para compilar uma funcao preguicosa.
#[derive(Debug)]
pub struct Pendente {
    /// 0 = function, 1 = metodo (comeca no '('), 2 = seta (comeca nos parametros)
    pub tipo: u8,
    /// Variaveis capturadas visiveis: por funcao de fora (de fora para
    /// dentro), por escopo com ambiente, (nome, indice no ambiente).
    pub escopos: Vec<Vec<Vec<(Rc<str>, u16)>>>,
}

impl Prototipo {
    pub fn texto_fonte(&self) -> &str {
        self.fonte.get(self.fonte_ini as usize..self.fonte_fim as usize).unwrap_or("")
    }
}
