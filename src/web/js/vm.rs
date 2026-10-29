//! A maquina virtual: executa o bytecode, com os quadros de chamada fora
//! da pilha do Rust (JS chamando JS nao gasta pilha nativa; geradores e
//! funcoes async so guardam o quadro). Tambem: conversoes do ECMAScript,
//! propriedades (prototipos, acessores, arrays, textos), excecoes,
//! promessas e a fila de microtarefas.

#[allow(unused_imports)]
use super::mat64::F64Ext;
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

use super::bytecode::{Bin, Op, Prototipo, Un};
use super::valor::*;

pub type R<T> = Result<T, Valor>;

/// Contexto de uma chamada nativa.
pub struct Ctx<'a> {
    pub this: &'a Valor,
    pub args: &'a [Valor],
    pub dado: &'a Valor,
    /// Chamada com new: o construtor alvo.
    pub novo: Option<&'a Valor>,
}

impl Ctx<'_> {
    pub fn arg(&self, i: usize) -> Valor {
        self.args.get(i).cloned().unwrap_or(Valor::Indef)
    }
}

pub type FnNativa = fn(&mut Vm, &Ctx) -> R<Valor>;

#[derive(Debug)]
pub struct Quadro {
    pub proto: Rc<Prototipo>,
    pub ip: usize,
    pub pilha: Vec<Valor>,
    pub locais: Vec<Valor>,
    pub amb: Option<Amb>,
    pub this: Valor,
    pub funcao: Valor,
    pub novo_alvo: Valor,
    pub args: Vec<Valor>,
    pub tentativas: Vec<(u32, u32, Option<Amb>)>,
    pub construindo: bool,
}

impl Quadro {
    pub fn raizes(&self, vs: &mut Vec<Valor>, ambs: &mut Vec<Amb>) {
        vs.extend(self.pilha.iter().cloned());
        vs.extend(self.locais.iter().cloned());
        vs.extend(self.args.iter().cloned());
        vs.push(self.this.clone());
        vs.push(self.funcao.clone());
        vs.push(self.novo_alvo.clone());
        if let Some(a) = &self.amb {
            ambs.push(a.clone());
        }
        for (_, _, a) in &self.tentativas {
            if let Some(a) = a {
                ambs.push(a.clone());
            }
        }
    }
}

pub enum Saida {
    Retorno(Valor),
    Yield(Valor),
    Await(Valor),
}

pub enum Tarefa {
    /// handler(arg) -> resolve/rejeita a promessa derivada
    Reacao { handler: Valor, arg: Valor, derivada: Valor, rejeitada: bool },
    Chamar(Valor, Vec<Valor>),
}

#[derive(Default)]
pub struct Protos {
    pub objeto: Ref,
    pub funcao: Ref,
    pub array: Ref,
    pub texto: Ref,
    pub numero: Ref,
    pub booleano: Ref,
    pub simbolo: Ref,
    pub erro: Ref,
    pub erro_tipo: Ref,
    pub erro_ref: Ref,
    pub erro_faixa: Ref,
    pub erro_sintaxe: Ref,
    pub promessa: Ref,
    pub mapa: Ref,
    pub conjunto: Ref,
    pub regex: Ref,
    pub data: Ref,
    pub iterador: Ref,
    pub gerador: Ref,
}

/// O que o anfitriao (a pagina) oferece: propriedades de objetos do DOM.
pub trait Anfitriao {
    fn pegar(&mut self, vm: &mut Vm, no: u32, c: Chave) -> Option<R<Valor>>;
    fn por(&mut self, vm: &mut Vm, no: u32, c: Chave, v: &Valor) -> Option<R<()>>;
    fn raizes(&self, vs: &mut Vec<Valor>);
    fn como_any(&mut self) -> &mut dyn core::any::Any;
}

pub struct Vm {
    pub heap: Heap,
    pub global: Ref,
    pub p: Protos,
    pub quadros: Vec<Quadro>,
    pub microtarefas: VecDeque<Tarefa>,
    pub nativas: Vec<FnNativa>,
    pub prox_simbolo: u32,
    pub descricoes_simbolos: Vec<(u32, Rc<str>)>,
    pub anfitriao: Option<Box<dyn Anfitriao>>,
    pub raizes_extras: Vec<Valor>,
    pub console: String,
    pub nativas_ativas: u32,
    pub passos: u64,
    /// Limite de passos (protege a box de laco infinito): 0 = sem.
    pub limite_passos: u64,
    pub ids: Ids,
}

/// Ids de nativas que a VM usa por dentro.
#[derive(Default, Clone, Copy)]
pub struct Ids {
    pub resolver: u16,
    pub rejeitar: u16,
    pub async_cumprir: u16,
    pub async_rejeitar: u16,
}

pub const CH_LENGTH: &str = "length";

impl Vm {
    pub fn novo() -> Vm {
        let mut vm = Vm {
            heap: Heap::default(),
            global: 0,
            p: Protos::default(),
            quadros: Vec::new(),
            microtarefas: VecDeque::new(),
            nativas: Vec::new(),
            prox_simbolo: PRIMEIRO_SIMBOLO_LIVRE,
            descricoes_simbolos: Vec::new(),
            anfitriao: None,
            raizes_extras: Vec::new(),
            console: String::new(),
            nativas_ativas: 0,
            passos: 0,
            limite_passos: 0,
            ids: Ids::default(),
        };
        super::embutidos::instalar(&mut vm);
        vm
    }

    // ------------------------------------------------------- objetos

    pub fn alocar(&mut self, proto: Option<Ref>, classe: Classe) -> Ref {
        self.heap.alocar(Objeto::novo(proto, classe))
    }

    pub fn novo_objeto(&mut self) -> Valor {
        let p = self.p.objeto;
        Valor::Obj(self.alocar(Some(p), Classe::Comum))
    }

    pub fn novo_array(&mut self, v: Vec<Valor>) -> Valor {
        let p = self.p.array;
        Valor::Obj(self.alocar(Some(p), Classe::Array(v)))
    }

    pub fn novo_erro(&mut self, proto: Ref, msg: &str) -> Valor {
        let r = self.alocar(Some(proto), Classe::Erro);
        let m = Valor::texto(msg);
        self.heap.get_mut(r).definir(chave("message"), Slot::oculto(m));
        // pilha: a linha atual, se houver
        let mut pilha = String::new();
        for q in self.quadros.iter().rev().take(6) {
            pilha.push_str(&format!("    at {} (pos {})\n", q.proto.nome, q.proto.linhas.get(q.ip.saturating_sub(1)).copied().unwrap_or(0)));
        }
        self.heap.get_mut(r).definir(chave("stack"), Slot::oculto(Valor::texto(&pilha)));
        Valor::Obj(r)
    }

    pub fn erro_tipo<T>(&mut self, msg: &str) -> R<T> {
        let p = self.p.erro_tipo;
        Err(self.novo_erro(p, msg))
    }

    pub fn erro_ref<T>(&mut self, msg: &str) -> R<T> {
        let p = self.p.erro_ref;
        Err(self.novo_erro(p, msg))
    }

    pub fn erro_faixa<T>(&mut self, msg: &str) -> R<T> {
        let p = self.p.erro_faixa;
        Err(self.novo_erro(p, msg))
    }

    /// Registra uma funcao nativa e devolve o objeto funcao.
    pub fn nativa(&mut self, nome: &str, f: FnNativa, aridade: u8) -> Valor {
        let id = self.registrar(f);
        self.nativa_com(id, nome, aridade, Valor::Indef)
    }

    pub fn registrar(&mut self, f: FnNativa) -> u16 {
        self.nativas.push(f);
        (self.nativas.len() - 1) as u16
    }

    pub fn nativa_com(&mut self, id: u16, nome: &str, aridade: u8, dado: Valor) -> Valor {
        let pf = self.p.funcao;
        let r = self.alocar(Some(pf), Classe::Nativa { id, dado });
        let o = self.heap.get_mut(r);
        o.definir(chave("name"), Slot { v: Valor::texto(nome), acessor: None, enumeravel: false, gravavel: false, configuravel: true });
        o.definir(chave(CH_LENGTH), Slot { v: Valor::Num(aridade as f64), acessor: None, enumeravel: false, gravavel: false, configuravel: true });
        Valor::Obj(r)
    }

    /// obj.nome = funcao nativa (nao enumeravel).
    pub fn metodo(&mut self, obj: Ref, nome: &str, f: FnNativa, aridade: u8) {
        let v = self.nativa(nome, f, aridade);
        self.heap.get_mut(obj).definir(chave(nome), Slot::oculto(v));
    }

    pub fn definir(&mut self, obj: Ref, nome: &str, v: Valor) {
        self.heap.get_mut(obj).definir(chave(nome), Slot::oculto(v));
    }

    pub fn eh_chamavel(&self, v: &Valor) -> bool {
        match v {
            Valor::Obj(r) => matches!(self.heap.get(*r).classe, Classe::Funcao { .. } | Classe::Nativa { .. } | Classe::Ligada { .. }),
            _ => false,
        }
    }

    pub fn eh_array(&self, v: &Valor) -> bool {
        matches!(v, Valor::Obj(r) if matches!(self.heap.get(*r).classe, Classe::Array(_)))
    }

    pub fn novo_simbolo(&mut self, desc: &str) -> u32 {
        let id = SIMBOLO | self.prox_simbolo;
        self.prox_simbolo += 1;
        self.descricoes_simbolos.push((id, Rc::from(desc)));
        id
    }

    // --------------------------------------------------- conversoes

    pub fn para_primitivo(&mut self, v: &Valor, dica_texto: bool) -> R<Valor> {
        let Valor::Obj(r) = v else { return Ok(v.clone()) };
        match &self.heap.get(*r).classe {
            Classe::Numero(n) => return Ok(Valor::Num(*n)),
            Classe::Texto(s) => return Ok(Valor::Texto(s.clone())),
            Classe::Booleano(b) => return Ok(Valor::Bool(*b)),
            Classe::Data(t) if !dica_texto => return Ok(Valor::Num(*t)),
            _ => {}
        }
        let ordem = if dica_texto { ["toString", "valueOf"] } else { ["valueOf", "toString"] };
        for nome in ordem {
            let f = self.pegar(v, chave(nome))?;
            if self.eh_chamavel(&f) {
                let x = self.chamar(&f, v, &[])?;
                if !matches!(x, Valor::Obj(_)) {
                    return Ok(x);
                }
            }
        }
        self.erro_tipo("não converte para primitivo")
    }

    pub fn para_numero(&mut self, v: &Valor) -> R<f64> {
        Ok(match v {
            Valor::Indef => f64::NAN,
            Valor::Nulo => 0.0,
            Valor::Bool(b) => *b as u8 as f64,
            Valor::Num(n) => *n,
            Valor::Texto(s) => texto_numero(s),
            Valor::Simbolo(_) => return self.erro_tipo("símbolo não vira número"),
            Valor::Obj(_) => {
                let p = self.para_primitivo(v, false)?;
                return self.para_numero(&p);
            }
        })
    }

    pub fn para_texto(&mut self, v: &Valor) -> R<Rc<str>> {
        Ok(match v {
            Valor::Indef => Rc::from("undefined"),
            Valor::Nulo => Rc::from("null"),
            Valor::Bool(b) => Rc::from(if *b { "true" } else { "false" }),
            Valor::Num(n) => Rc::from(numero_texto(*n).as_str()),
            Valor::Texto(s) => s.clone(),
            Valor::Simbolo(_) => return self.erro_tipo("símbolo não vira texto"),
            Valor::Obj(_) => {
                let p = self.para_primitivo(v, true)?;
                return self.para_texto(&p);
            }
        })
    }

    /// Texto para mostrar (console.log): nunca falha.
    pub fn mostrar(&mut self, v: &Valor, prof: u32) -> String {
        match v {
            Valor::Texto(s) if prof > 0 => format!("'{s}'"),
            Valor::Simbolo(id) => {
                let d = self.descricoes_simbolos.iter().find(|(i, _)| i == id).map(|x| x.1.clone()).unwrap_or_else(|| Rc::from(""));
                format!("Symbol({d})")
            }
            Valor::Obj(r) => {
                let r = *r;
                if prof > 2 {
                    return "[...]".into();
                }
                let classe_resumo = match &self.heap.get(r).classe {
                    Classe::Array(v) => Some(v.clone()),
                    _ => None,
                };
                if let Some(itens) = classe_resumo {
                    let partes: Vec<String> = itens.iter().take(50).map(|x| self.mostrar(x, prof + 1)).collect();
                    return format!("[ {} ]", partes.join(", "));
                }
                if self.eh_chamavel(v) {
                    let n = self.pegar(v, chave("name")).ok().map(|n| self.mostrar(&n, 0)).unwrap_or_default();
                    return format!("[Function: {n}]");
                }
                if matches!(self.heap.get(r).classe, Classe::Erro) {
                    let n = self.pegar(v, chave("name")).ok().map(|n| self.mostrar(&n, 0)).unwrap_or_default();
                    let m = self.pegar(v, chave("message")).ok().map(|n| self.mostrar(&n, 0)).unwrap_or_default();
                    return format!("{n}: {m}");
                }
                if let Classe::Dom(_) = self.heap.get(r).classe {
                    return self.para_texto(v).map(|s| String::from(&*s)).unwrap_or_default();
                }
                let props: Vec<(Chave, Valor)> = self
                    .heap
                    .get(r)
                    .props
                    .iter()
                    .filter(|(_, s)| s.enumeravel)
                    .take(30)
                    .map(|(c, s)| (*c, s.v.clone()))
                    .collect();
                let partes: Vec<String> = props.iter().map(|(c, x)| format!("{}: {}", nome_da_chave(*c), self.mostrar(x, prof + 1))).collect();
                format!("{{ {} }}", partes.join(", "))
            }
            outro => self.para_texto(outro).map(|s| String::from(&*s)).unwrap_or_default(),
        }
    }

    pub fn para_objeto(&mut self, v: &Valor) -> R<Ref> {
        Ok(match v {
            Valor::Obj(r) => *r,
            Valor::Indef | Valor::Nulo => return self.erro_tipo("não é possível ler propriedade de undefined/null"),
            Valor::Bool(b) => {
                let p = self.p.booleano;
                self.alocar(Some(p), Classe::Booleano(*b))
            }
            Valor::Num(n) => {
                let p = self.p.numero;
                self.alocar(Some(p), Classe::Numero(*n))
            }
            Valor::Texto(s) => {
                let p = self.p.texto;
                self.alocar(Some(p), Classe::Texto(s.clone()))
            }
            Valor::Simbolo(id) => {
                let p = self.p.simbolo;
                self.alocar(Some(p), Classe::Simbolo(*id))
            }
        })
    }

    /// ToPropertyKey.
    pub fn chave_de(&mut self, v: &Valor) -> R<Chave> {
        Ok(match v {
            Valor::Texto(s) => chave(s),
            Valor::Simbolo(id) => *id,
            Valor::Num(n) => chave(&numero_texto(*n)),
            Valor::Obj(_) => {
                let p = self.para_primitivo(v, true)?;
                return self.chave_de(&p);
            }
            outro => {
                let s = self.para_texto(outro)?;
                chave(&s)
            }
        })
    }

    pub fn typeof_(&self, v: &Valor) -> &'static str {
        match v {
            Valor::Indef => "undefined",
            Valor::Nulo => "object",
            Valor::Bool(_) => "boolean",
            Valor::Num(_) => "number",
            Valor::Texto(_) => "string",
            Valor::Simbolo(_) => "symbol",
            Valor::Obj(_) => {
                if self.eh_chamavel(v) {
                    "function"
                } else {
                    "object"
                }
            }
        }
    }

    pub fn estrito(&self, a: &Valor, b: &Valor) -> bool {
        a == b
    }

    pub fn frouxo(&mut self, a: &Valor, b: &Valor) -> R<bool> {
        Ok(match (a, b) {
            (Valor::Indef | Valor::Nulo, Valor::Indef | Valor::Nulo) => true,
            (Valor::Indef | Valor::Nulo, _) | (_, Valor::Indef | Valor::Nulo) => false,
            (Valor::Num(_), Valor::Texto(s)) => *a == Valor::Num(texto_numero(s)),
            (Valor::Texto(s), Valor::Num(_)) => Valor::Num(texto_numero(s)) == *b,
            (Valor::Bool(x), _) => {
                let n = Valor::Num(*x as u8 as f64);
                return self.frouxo(&n, b);
            }
            (_, Valor::Bool(y)) => {
                let n = Valor::Num(*y as u8 as f64);
                return self.frouxo(a, &n);
            }
            (Valor::Obj(_), Valor::Obj(_)) => a == b,
            (Valor::Obj(_), _) => {
                let p = self.para_primitivo(a, false)?;
                return self.frouxo(&p, b);
            }
            (_, Valor::Obj(_)) => {
                let p = self.para_primitivo(b, false)?;
                return self.frouxo(a, &p);
            }
            _ => a == b,
        })
    }

    // ------------------------------------------------- propriedades

    /// Indice de array a partir da chave ("0", "12"...).
    fn indice_de_chave(c: Chave) -> Option<usize> {
        if c & SIMBOLO != 0 {
            return None;
        }
        let s = nome_da_chave(c);
        let b = s.as_bytes();
        if b.is_empty() || b.len() > 9 || (b[0] == b'0' && b.len() > 1) || !b.iter().all(|x| x.is_ascii_digit()) {
            return None;
        }
        s.parse().ok()
    }

    pub fn pegar(&mut self, alvo: &Valor, c: Chave) -> R<Valor> {
        let r = match alvo {
            Valor::Obj(r) => *r,
            Valor::Texto(s) => {
                if nome_da_chave(c) == CH_LENGTH && c & SIMBOLO == 0 {
                    return Ok(Valor::Num(s.encode_utf16().count() as f64));
                }
                if let Some(i) = Self::indice_de_chave(c) {
                    return Ok(match s.encode_utf16().nth(i) {
                        Some(u) => Valor::Texto(Rc::from(String::from_utf16_lossy(&[u]).as_str())),
                        None => Valor::Indef,
                    });
                }
                let p = self.p.texto;
                return self.pegar_em(p, c, alvo);
            }
            Valor::Num(_) => {
                let p = self.p.numero;
                return self.pegar_em(p, c, alvo);
            }
            Valor::Bool(_) => {
                let p = self.p.booleano;
                return self.pegar_em(p, c, alvo);
            }
            Valor::Simbolo(_) => {
                let p = self.p.simbolo;
                return self.pegar_em(p, c, alvo);
            }
            Valor::Indef | Valor::Nulo => {
                let n = nome_da_chave(c);
                return self.erro_tipo(&format!("não dá para ler '{n}' de {}", if matches!(alvo, Valor::Nulo) { "null" } else { "undefined" }));
            }
        };
        self.pegar_em(r, c, alvo)
    }

    /// Procura `c` em `r` e na cadeia de prototipos; `this` para getters.
    pub fn pegar_em(&mut self, r: Ref, c: Chave, this: &Valor) -> R<Valor> {
        let mut atual = Some(r);
        let mut primeiro = true;
        while let Some(x) = atual {
            let o = self.heap.get(x);
            // casos especiais da classe
            match &o.classe {
                Classe::Array(v) | Classe::Argumentos(v) => {
                    if let Some(i) = Self::indice_de_chave(c) {
                        if let Some(e) = v.get(i) {
                            return Ok(e.clone());
                        }
                    } else if c == chave(CH_LENGTH) {
                        return Ok(Valor::Num(v.len() as f64));
                    }
                }
                Classe::Texto(s) if primeiro => {
                    if c == chave(CH_LENGTH) {
                        return Ok(Valor::Num(s.encode_utf16().count() as f64));
                    }
                }
                Classe::Funcao { proto, .. } if primeiro && o.achar(c).is_none() => {
                    let nome = nome_da_chave(c);
                    if nome == "prototype" && !proto.seta && !proto.metodo && !proto.geradora && !proto.assincrona || nome == "prototype" && proto.eh_classe {
                        // prototype criado sob demanda
                        let po = self.p.objeto;
                        let pg = self.p.gerador;
                        let novo = self.alocar(Some(if proto.geradora { pg } else { po }), Classe::Comum);
                        self.heap.get_mut(novo).definir(chave("constructor"), Slot::oculto(Valor::Obj(x)));
                        let s = Slot { v: Valor::Obj(novo), acessor: None, enumeravel: false, gravavel: true, configuravel: false };
                        self.heap.get_mut(x).definir(c, s);
                        return Ok(Valor::Obj(novo));
                    }
                    if nome == "name" {
                        return Ok(Valor::Texto(proto.nome.clone()));
                    }
                    if nome == CH_LENGTH {
                        return Ok(Valor::Num(proto.n_params as f64));
                    }
                }
                Classe::Dom(no) if primeiro && o.achar(c).is_none() => {
                    let no = *no;
                    if let Some(mut h) = self.anfitriao.take() {
                        let res = h.pegar(self, no, c);
                        self.anfitriao = Some(h);
                        if let Some(res) = res {
                            return res;
                        }
                    }
                }
                Classe::Proxy(alvo, manip) => {
                    let (alvo, manip) = (alvo.clone(), manip.clone());
                    let get = self.pegar(&manip, chave("get"))?;
                    let k = if c & SIMBOLO != 0 { Valor::Simbolo(c) } else { Valor::texto(nome_da_chave(c)) };
                    if self.eh_chamavel(&get) {
                        return self.chamar(&get, &manip, &[alvo, k, this.clone()]);
                    }
                    return self.pegar(&alvo, c);
                }
                _ => {}
            }
            let o = self.heap.get(x);
            if let Some(s) = o.slot(c) {
                if let Some(a) = &s.acessor {
                    let g = a.0.clone();
                    if matches!(g, Valor::Indef) {
                        return Ok(Valor::Indef);
                    }
                    return self.chamar(&g, this, &[]);
                }
                return Ok(s.v.clone());
            }
            atual = o.proto;
            primeiro = false;
        }
        Ok(Valor::Indef)
    }

    pub fn pegar_idx(&mut self, alvo: &Valor, k: &Valor) -> R<Valor> {
        // caminho rapido: array[numero]
        if let (Valor::Obj(r), Valor::Num(n)) = (alvo, k) {
            if *n >= 0.0 && n.fract() == 0.0 {
                if let Classe::Array(v) = &self.heap.get(*r).classe {
                    if let Some(e) = v.get(*n as usize) {
                        return Ok(e.clone());
                    }
                }
            }
        }
        if let (Valor::Texto(s), Valor::Num(n)) = (alvo, k) {
            if *n >= 0.0 && n.fract() == 0.0 && s.is_ascii() {
                return Ok(match s.as_bytes().get(*n as usize) {
                    Some(b) => Valor::Texto(Rc::from(core::str::from_utf8(&[*b]).unwrap_or(""))),
                    None => Valor::Indef,
                });
            }
        }
        let c = self.chave_de(k)?;
        self.pegar(alvo, c)
    }

    /// Procura um setter ou propriedade nao gravavel na cadeia.
    fn setter_na_cadeia(&self, r: Ref, c: Chave) -> Option<Result<Valor, bool>> {
        let mut atual = Some(r);
        while let Some(x) = atual {
            let o = self.heap.get(x);
            if let Some(s) = o.slot(c) {
                if let Some(a) = &s.acessor {
                    return Some(Ok(a.1.clone()));
                }
                if !s.gravavel {
                    return Some(Err(false));
                }
                return None;
            }
            atual = o.proto;
        }
        None
    }

    pub fn por(&mut self, alvo: &Valor, c: Chave, v: Valor) -> R<()> {
        let Valor::Obj(r) = alvo else {
            if alvo.eh_nulo_ou_indef() {
                let n = nome_da_chave(c);
                return self.erro_tipo(&format!("não dá para definir '{n}' em undefined/null"));
            }
            return Ok(()); // primitivos: some
        };
        let r = *r;
        // arrays
        let eh_length = c == chave(CH_LENGTH);
        let idx = Self::indice_de_chave(c);
        if let Classe::Array(arr) = &mut self.heap.get_mut(r).classe {
            if let Some(i) = idx {
                if i < arr.len() {
                    arr[i] = v;
                    return Ok(());
                }
                if i < arr.len() + 1_000_000 {
                    arr.resize(i, Valor::Indef);
                    arr.push(v);
                    return Ok(());
                }
            } else if eh_length {
                if let Valor::Num(n) = v {
                    if n >= 0.0 && n.fract() == 0.0 && n < 1e8 {
                        arr.resize(n as usize, Valor::Indef);
                        return Ok(());
                    }
                }
                return self.erro_faixa("tamanho de array inválido");
            }
        }
        match &self.heap.get(r).classe {
            Classe::Dom(no) => {
                let no = *no;
                if self.heap.get(r).achar(c).is_none() {
                    if let Some(mut h) = self.anfitriao.take() {
                        let res = h.por(self, no, c, &v);
                        self.anfitriao = Some(h);
                        if let Some(res) = res {
                            return res;
                        }
                    }
                }
            }
            Classe::Proxy(alvo, manip) => {
                let (alvo, manip) = (alvo.clone(), manip.clone());
                let set = self.pegar(&manip, chave("set"))?;
                if self.eh_chamavel(&set) {
                    let k = if c & SIMBOLO != 0 { Valor::Simbolo(c) } else { Valor::texto(nome_da_chave(c)) };
                    self.chamar(&set, &manip, &[alvo, k, v, Valor::Obj(r)])?;
                    return Ok(());
                }
                return self.por(&alvo, c, v);
            }
            _ => {}
        }
        if nome_da_chave(c) == "__proto__" && c & SIMBOLO == 0 {
            match v {
                Valor::Obj(p) => self.heap.get_mut(r).proto = Some(p),
                Valor::Nulo => self.heap.get_mut(r).proto = None,
                _ => {}
            }
            return Ok(());
        }
        // propriedade propria comum: caminho rapido
        if let Some(s) = self.heap.get_mut(r).slot_mut(c) {
            if s.acessor.is_none() {
                if s.gravavel {
                    s.v = v;
                }
                return Ok(());
            }
        }
        match self.setter_na_cadeia(r, c) {
            Some(Ok(set)) => {
                if !matches!(set, Valor::Indef) {
                    self.chamar(&set, alvo, &[v])?;
                }
                Ok(())
            }
            Some(Err(_)) => Ok(()),
            None => {
                let o = self.heap.get_mut(r);
                if o.extensivel {
                    o.definir(c, Slot::valor(v));
                }
                Ok(())
            }
        }
    }

    pub fn por_idx(&mut self, alvo: &Valor, k: &Valor, v: Valor) -> R<()> {
        if let (Valor::Obj(r), Valor::Num(n)) = (alvo, k) {
            if *n >= 0.0 && n.fract() == 0.0 {
                if let Classe::Array(arr) = &mut self.heap.get_mut(*r).classe {
                    let i = *n as usize;
                    if i < arr.len() {
                        arr[i] = v;
                        return Ok(());
                    }
                    if i == arr.len() {
                        arr.push(v);
                        return Ok(());
                    }
                }
            }
        }
        let c = self.chave_de(k)?;
        self.por(alvo, c, v)
    }

    /// Define propriedade propria (literal de objeto, defineProperty simples).
    pub fn definir_propria(&mut self, r: Ref, c: Chave, v: Valor) {
        if let Classe::Array(arr) = &mut self.heap.get_mut(r).classe {
            if let Some(i) = Self::indice_de_chave(c) {
                if i < arr.len() {
                    arr[i] = v;
                    return;
                }
                if i == arr.len() {
                    arr.push(v);
                    return;
                }
            }
        }
        self.heap.get_mut(r).definir(c, Slot::valor(v));
    }

    /// Chaves proprias enumeraveis (indices primeiro, como na norma).
    pub fn chaves_proprias(&mut self, r: Ref, so_enumeraveis: bool) -> Vec<Valor> {
        let mut v = Vec::new();
        let o = self.heap.get(r);
        match &o.classe {
            Classe::Array(a) | Classe::Argumentos(a) => {
                for i in 0..a.len() {
                    v.push(Valor::texto(&numero_texto(i as f64)));
                }
            }
            Classe::Texto(s) => {
                for i in 0..s.encode_utf16().count() {
                    v.push(Valor::texto(&numero_texto(i as f64)));
                }
            }
            _ => {}
        }
        let mut indices: Vec<(usize, Valor)> = Vec::new();
        let mut outros: Vec<Valor> = Vec::new();
        for (c, s) in &o.props {
            if c & SIMBOLO != 0 || (so_enumeraveis && !s.enumeravel) {
                continue;
            }
            let n = nome_da_chave(*c);
            if let Some(i) = Self::indice_de_chave(*c) {
                indices.push((i, Valor::texto(n)));
            } else {
                outros.push(Valor::texto(n));
            }
        }
        indices.sort_by_key(|x| x.0);
        v.extend(indices.into_iter().map(|x| x.1));
        v.extend(outros);
        v
    }

    pub fn tem_propriedade(&mut self, r: Ref, c: Chave) -> bool {
        let mut atual = Some(r);
        while let Some(x) = atual {
            let o = self.heap.get(x);
            match &o.classe {
                Classe::Array(a) => {
                    if let Some(i) = Self::indice_de_chave(c) {
                        if i < a.len() {
                            return true;
                        }
                    } else if c == chave(CH_LENGTH) {
                        return true;
                    }
                }
                Classe::Dom(_) => {
                    // propriedades do DOM existem se o anfitriao responde
                    if o.achar(c).is_none() {
                        if let Ok(v) = self.pegar(&Valor::Obj(x), c) {
                            if !matches!(v, Valor::Indef) {
                                return true;
                            }
                        }
                    }
                }
                _ => {}
            }
            let o = self.heap.get(x);
            if o.achar(c).is_some() {
                return true;
            }
            atual = o.proto;
        }
        false
    }

    pub fn instancia_de(&mut self, v: &Valor, f: &Valor) -> R<bool> {
        if !self.eh_chamavel(f) {
            return self.erro_tipo("instanceof: o lado direito não é função");
        }
        let f = match f {
            Valor::Obj(r) => match &self.heap.get(*r).classe {
                Classe::Ligada { alvo, .. } => alvo.clone(),
                _ => f.clone(),
            },
            _ => f.clone(),
        };
        let Valor::Obj(mut o) = v.clone() else { return Ok(false) };
        let proto = self.pegar(&f, chave("prototype"))?;
        let Valor::Obj(p) = proto else { return Ok(false) };
        loop {
            match self.heap.get(o).proto {
                Some(x) if x == p => return Ok(true),
                Some(x) => o = x,
                None => return Ok(false),
            }
        }
    }

    // --------------------------------------------------------- chamadas

    /// Cria um objeto funcao a partir de um prototipo compilado.
    pub fn criar_funcao(&mut self, proto: Rc<Prototipo>, amb: Option<Amb>, this: Option<Valor>, casa: Option<Ref>) -> Valor {
        let pf = self.p.funcao;
        Valor::Obj(self.alocar(Some(pf), Classe::Funcao { proto, amb, this, casa, campos: None }))
    }

    /// Chama f(this, args) e espera terminar (pode rodar JS por dentro).
    pub fn chamar(&mut self, f: &Valor, this: &Valor, args: &[Valor]) -> R<Valor> {
        let Valor::Obj(r) = f else { return self.erro_tipo("não é uma função") };
        let r = *r;
        match &self.heap.get(r).classe {
            Classe::Nativa { id, dado } => {
                let (id, dado) = (*id, dado.clone());
                let fun = self.nativas[id as usize];
                self.nativas_ativas += 1;
                let res = fun(self, &Ctx { this, args, dado: &dado, novo: None });
                self.nativas_ativas -= 1;
                res
            }
            Classe::Ligada { alvo, this: t, args: a } => {
                let (alvo, t) = (alvo.clone(), t.clone());
                let mut todos = a.clone();
                todos.extend_from_slice(args);
                self.chamar(&alvo, &t, &todos)
            }
            Classe::Funcao { proto, .. } => {
                if proto.eh_classe {
                    return self.erro_tipo("classe precisa de new");
                }
                let base = self.quadros.len();
                self.empurrar_quadro(r, this.clone(), args.to_vec(), Valor::Indef, false)?;
                match self.empilhado_especial(base)? {
                    Some(v) => Ok(v),
                    None => match self.executar(base)? {
                        Saida::Retorno(v) => Ok(v),
                        _ => self.erro_tipo("yield/await fora de lugar"),
                    },
                }
            }
            _ => self.erro_tipo("não é uma função"),
        }
    }

    /// Geradores e async nao executam direto: viram objeto/promessa.
    /// Se o quadro empurrado e desses, trata e devolve o valor.
    fn empilhado_especial(&mut self, base: usize) -> R<Option<Valor>> {
        let q = match self.quadros.get(base) {
            Some(q) => q,
            None => return Ok(None),
        };
        if q.proto.geradora {
            let q = self.quadros.pop().unwrap_or_else(|| unreachable!());
            let pg = match self.pegar(&q.funcao, chave("prototype"))? {
                Valor::Obj(p) => p,
                _ => self.p.gerador,
            };
            let g = self.alocar(Some(pg), Classe::Gerador(Some(Box::new(q)), false));
            return Ok(Some(Valor::Obj(g)));
        }
        if q.proto.assincrona {
            let q = self.quadros.pop().unwrap_or_else(|| unreachable!());
            let promessa = self.nova_promessa();
            let co = self.alocar(None, Classe::Gerador(Some(Box::new(q)), true));
            self.heap.get_mut(co).definir(chave("#p"), Slot::oculto(promessa.clone()));
            self.passo_async(co, Valor::Indef, false);
            return Ok(Some(promessa));
        }
        Ok(None)
    }

    pub fn empurrar_quadro(&mut self, r: Ref, mut this: Valor, args: Vec<Valor>, novo_alvo: Valor, construindo: bool) -> R<()> {
        let (proto, amb, this_lex) = match &self.heap.get(r).classe {
            Classe::Funcao { proto, amb, this, .. } => (proto.clone(), amb.clone(), this.clone()),
            _ => return self.erro_tipo("não é função JS"),
        };
        if self.quadros.len() > 4000 {
            return self.erro_faixa("pilha de chamadas estourou (recursão infinita?)");
        }
        if let Some(t) = this_lex {
            this = t;
        } else if !proto.estrito && this.eh_nulo_ou_indef() && !construindo {
            this = Valor::Obj(self.global);
        }
        let amb = if proto.amb_funcao > 0 { Some(novo_amb(proto.amb_funcao as usize, amb)) } else { amb };
        let n = proto.n_locais as usize;
        self.quadros.push(Quadro {
            proto,
            ip: 0,
            pilha: Vec::with_capacity(16),
            locais: alloc::vec![Valor::Indef; n],
            amb,
            this,
            funcao: Valor::Obj(r),
            novo_alvo,
            args,
            tentativas: Vec::new(),
            construindo,
        });
        Ok(())
    }

    /// new f(args)
    pub fn construir(&mut self, f: &Valor, args: &[Valor], novo_alvo: &Valor) -> R<Valor> {
        let Valor::Obj(r) = f else { return self.erro_tipo("não é construtor") };
        let r = *r;
        match &self.heap.get(r).classe {
            Classe::Nativa { id, dado } => {
                let (id, dado) = (*id, dado.clone());
                let fun = self.nativas[id as usize];
                // this: objeto com o prototype do alvo
                let pr = self.pegar(novo_alvo, chave("prototype"))?;
                let po = self.p.objeto;
                let proto = pr.obj().unwrap_or(po);
                let this = Valor::Obj(self.alocar(Some(proto), Classe::Comum));
                self.nativas_ativas += 1;
                let res = fun(self, &Ctx { this: &this, args, dado: &dado, novo: Some(novo_alvo) });
                self.nativas_ativas -= 1;
                let v = res?;
                Ok(if matches!(v, Valor::Obj(_)) { v } else { this })
            }
            Classe::Ligada { alvo, args: a, .. } => {
                let alvo = alvo.clone();
                let mut todos = a.clone();
                todos.extend_from_slice(args);
                let na = if novo_alvo == f { alvo.clone() } else { novo_alvo.clone() };
                self.construir(&alvo, &todos, &na)
            }
            Classe::Funcao { proto, campos, .. } => {
                if proto.seta || (proto.metodo && !proto.eh_classe) || proto.geradora || proto.assincrona {
                    return self.erro_tipo("não é construtor");
                }
                let derivado = proto.derivado;
                let campos = *campos;
                let this = if derivado {
                    Valor::Indef
                } else {
                    let pr = self.pegar(novo_alvo, chave("prototype"))?;
                    let po = self.p.objeto;
                    let proto = pr.obj().unwrap_or(po);
                    let t = Valor::Obj(self.alocar(Some(proto), Classe::Comum));
                    if let Some(c) = campos {
                        self.chamar_metodo_campos(c, &t)?;
                    }
                    t
                };
                let base = self.quadros.len();
                self.empurrar_quadro(r, this, args.to_vec(), novo_alvo.clone(), true)?;
                match self.executar(base)? {
                    Saida::Retorno(v) => Ok(v),
                    _ => self.erro_tipo("yield em construtor"),
                }
            }
            _ => self.erro_tipo("não é construtor"),
        }
    }

    fn chamar_metodo_campos(&mut self, campos: Ref, this: &Valor) -> R<()> {
        let base = self.quadros.len();
        self.empurrar_quadro(campos, this.clone(), Vec::new(), Valor::Indef, false)?;
        self.executar(base)?;
        Ok(())
    }

    // --------------------------------------------------------- promessas

    pub fn nova_promessa(&mut self) -> Valor {
        let p = self.p.promessa;
        Valor::Obj(self.alocar(
            Some(p),
            Classe::Promessa(Promessa { estado: EstadoPromessa::Pendente, valor: Valor::Indef, reacoes: Vec::new(), tratada: false }),
        ))
    }

    /// As funcoes resolve/reject de uma promessa.
    pub fn funcoes_resolucao(&mut self, p: &Valor) -> (Valor, Valor) {
        let (a, b) = (self.ids.resolver, self.ids.rejeitar);
        let r = self.nativa_com(a, "", 1, p.clone());
        let j = self.nativa_com(b, "", 1, p.clone());
        (r, j)
    }

    pub fn resolver_promessa(&mut self, p: &Valor, v: Valor) {
        if p == &v {
            let e = self.novo_erro(self.p.erro_tipo, "promessa resolvida com ela mesma");
            self.assentar(p, e, true);
            return;
        }
        // thenable: espera ele
        if let Valor::Obj(r) = &v {
            if let Classe::Promessa(_) = self.heap.get(*r).classe {
                // atalho para promessas nossas
            }
            let then = match self.pegar(&v, chave("then")) {
                Ok(t) => t,
                Err(e) => {
                    self.assentar(p, e, true);
                    return;
                }
            };
            if self.eh_chamavel(&then) {
                let (res, rej) = self.funcoes_resolucao(p);
                self.microtarefas.push_back(Tarefa::Chamar(then, alloc::vec![v, res, rej]));
                return;
            }
        }
        self.assentar(p, v, false);
    }

    /// Cumpre ou rejeita (sem olhar thenables).
    pub fn assentar(&mut self, p: &Valor, v: Valor, rejeitada: bool) {
        let Valor::Obj(r) = p else { return };
        let reacoes = match &mut self.heap.get_mut(*r).classe {
            Classe::Promessa(pr) if pr.estado == EstadoPromessa::Pendente => {
                pr.estado = if rejeitada { EstadoPromessa::Rejeitada } else { EstadoPromessa::Cumprida };
                pr.valor = v.clone();
                core::mem::take(&mut pr.reacoes)
            }
            _ => return,
        };
        for (cumprir, rejeitar, derivada) in reacoes {
            let handler = if rejeitada { rejeitar } else { cumprir };
            self.microtarefas.push_back(Tarefa::Reacao { handler, arg: v.clone(), derivada, rejeitada });
        }
    }

    /// p.then(cumprir, rejeitar) -> promessa derivada
    pub fn entao(&mut self, p: &Valor, cumprir: Valor, rejeitar: Valor) -> Valor {
        let derivada = self.nova_promessa();
        let Valor::Obj(r) = p else { return derivada };
        let estado = match &mut self.heap.get_mut(*r).classe {
            Classe::Promessa(pr) => {
                pr.tratada = true;
                if pr.estado == EstadoPromessa::Pendente {
                    pr.reacoes.push((cumprir.clone(), rejeitar.clone(), derivada.clone()));
                    None
                } else {
                    Some((pr.estado, pr.valor.clone()))
                }
            }
            _ => return derivada,
        };
        if let Some((e, v)) = estado {
            let rej = e == EstadoPromessa::Rejeitada;
            let handler = if rej { rejeitar } else { cumprir };
            self.microtarefas.push_back(Tarefa::Reacao { handler, arg: v, derivada: derivada.clone(), rejeitada: rej });
        }
        derivada
    }

    /// Roda as microtarefas ate a fila esvaziar.
    pub fn rodar_microtarefas(&mut self) {
        let mut n = 0;
        while let Some(t) = self.microtarefas.pop_front() {
            n += 1;
            if n > 1_000_000 {
                break;
            }
            match t {
                Tarefa::Chamar(f, args) => {
                    let this = args.first().cloned().unwrap_or_default();
                    let resto = if args.is_empty() { &[][..] } else { &args[1..] };
                    if let Err(e) = self.chamar(&f, &this, resto) {
                        // then que lanca: rejeita a promessa (args[2] e o reject)
                        if let Some(rej) = args.get(2) {
                            let _ = self.chamar(rej, &Valor::Indef, &[e]);
                        }
                    }
                }
                Tarefa::Reacao { handler, arg, derivada, rejeitada } => {
                    if self.eh_chamavel(&handler) {
                        match self.chamar(&handler, &Valor::Indef, &[arg]) {
                            Ok(v) => self.resolver_promessa(&derivada, v),
                            Err(e) => self.assentar(&derivada, e, true),
                        }
                    } else if rejeitada {
                        self.assentar(&derivada, arg, true);
                    } else {
                        self.resolver_promessa(&derivada, arg);
                    }
                }
            }
        }
    }

    /// Um passo de uma funcao async: continua o quadro com o valor (ou
    /// excecao) e trata o proximo await/retorno.
    pub fn passo_async(&mut self, co: Ref, v: Valor, excecao: bool) {
        let quadro = match &mut self.heap.get_mut(co).classe {
            Classe::Gerador(q, _) => q.take(),
            _ => None,
        };
        let Some(q) = quadro else { return };
        let promessa = self.heap.get(co).slot(chave("#p")).map(|s| s.v.clone()).unwrap_or_default();
        let base = self.quadros.len();
        self.quadros.push(*q);
        let r = if excecao {
            match self.desenrolar(v, base) {
                Ok(()) => self.executar(base),
                Err(e) => Err(e),
            }
        } else {
            if let Some(q) = self.quadros.last_mut() {
                if q.ip > 0 {
                    q.pilha.push(v);
                }
            }
            self.executar(base)
        };
        match r {
            Ok(Saida::Retorno(v)) => self.resolver_promessa(&promessa, v),
            Ok(Saida::Await(x)) | Ok(Saida::Yield(x)) => {
                // guarda o quadro e espera a promessa
                let q = self.quadros.pop().unwrap_or_else(|| unreachable!());
                if let Classe::Gerador(slot, _) = &mut self.heap.get_mut(co).classe {
                    *slot = Some(Box::new(q));
                }
                let p = self.nova_promessa();
                self.resolver_promessa(&p, x);
                let (a, b) = (self.ids.async_cumprir, self.ids.async_rejeitar);
                let f = self.nativa_com(a, "", 1, Valor::Obj(co));
                let g = self.nativa_com(b, "", 1, Valor::Obj(co));
                self.entao(&p, f, g);
            }
            Err(e) => self.assentar(&promessa, e, true),
        }
    }

    /// Continua um gerador: (valor, terminou)
    pub fn retomar_gerador(&mut self, g: Ref, v: Valor, modo: u8) -> R<(Valor, bool)> {
        let (quadro, iniciado) = match &mut self.heap.get_mut(g).classe {
            Classe::Gerador(q, ini) => {
                let i = *ini;
                *ini = true;
                (q.take(), i)
            }
            _ => return self.erro_tipo("não é gerador"),
        };
        let Some(q) = quadro else {
            // terminado
            return match modo {
                1 => Ok((v, true)),
                2 => Err(v),
                _ => Ok((Valor::Indef, true)),
            };
        };
        if modo == 1 {
            return Ok((v, true)); // return(): termina
        }
        let base = self.quadros.len();
        self.quadros.push(*q);
        let r = if modo == 2 {
            match self.desenrolar(v, base) {
                Ok(()) => self.executar(base),
                Err(e) => Err(e),
            }
        } else {
            if iniciado {
                if let Some(q) = self.quadros.last_mut() {
                    q.pilha.push(v);
                }
            }
            self.executar(base)
        };
        match r {
            Ok(Saida::Yield(x)) => {
                let q = self.quadros.pop().unwrap_or_else(|| unreachable!());
                if let Classe::Gerador(slot, _) = &mut self.heap.get_mut(g).classe {
                    *slot = Some(Box::new(q));
                }
                Ok((x, false))
            }
            Ok(Saida::Retorno(x)) => Ok((x, true)),
            Ok(Saida::Await(x)) => Ok((x, false)),
            Err(e) => Err(e),
        }
    }

    // --------------------------------------------------------- iteracao

    pub fn iterador(&mut self, v: &Valor) -> R<Valor> {
        if let Valor::Obj(r) = v {
            match &self.heap.get(*r).classe {
                Classe::Array(_) | Classe::Argumentos(_) => {
                    // so se Symbol.iterator nao foi trocado (quase sempre)
                    let pi = self.p.iterador;
                    return Ok(Valor::Obj(self.alocar(Some(pi), Classe::Iterador(v.clone(), 0, 0))));
                }
                Classe::Mapa(m) => {
                    let tipo = if m.eh_mapa { 2 } else { 0 };
                    let pi = self.p.iterador;
                    return Ok(Valor::Obj(self.alocar(Some(pi), Classe::Iterador(v.clone(), 0, tipo + 10))));
                }
                Classe::Iterador(..) => return Ok(v.clone()),
                _ => {}
            }
        }
        if let Valor::Texto(_) = v {
            let pi = self.p.iterador;
            return Ok(Valor::Obj(self.alocar(Some(pi), Classe::Iterador(v.clone(), 0, 3))));
        }
        let f = self.pegar(v, SIM_ITERADOR)?;
        if !self.eh_chamavel(&f) {
            let s = self.mostrar(v, 0);
            return self.erro_tipo(&format!("{s} não é iterável"));
        }
        let it = self.chamar(&f, v, &[])?;
        if !matches!(it, Valor::Obj(_)) {
            return self.erro_tipo("iterador inválido");
        }
        Ok(it)
    }

    /// Proximo valor: Some(v) ou None (fim).
    pub fn proximo(&mut self, it: &Valor) -> R<Option<Valor>> {
        if let Valor::Obj(r) = it {
            let r = *r;
            if let Classe::Iterador(alvo, pos, tipo) = &self.heap.get(r).classe {
                let (alvo, pos, tipo) = (alvo.clone(), *pos, *tipo);
                let res = match (&alvo, tipo) {
                    (Valor::Texto(s), 3) => {
                        // por caractere (code point)
                        let resto = &s[pos as usize..];
                        match resto.chars().next() {
                            Some(c) => {
                                let novo = pos + c.len_utf8() as u32;
                                if let Classe::Iterador(_, p, _) = &mut self.heap.get_mut(r).classe {
                                    *p = novo;
                                }
                                let mut b = [0u8; 4];
                                return Ok(Some(Valor::texto(c.encode_utf8(&mut b))));
                            }
                            None => None,
                        }
                    }
                    (Valor::Obj(a), _) => {
                        let mut pos = pos;
                        let item = match &self.heap.get(*a).classe {
                            Classe::Array(v) | Classe::Argumentos(v) => v.get(pos as usize).map(|x| (Valor::Num(pos as f64), x.clone())),
                            Classe::Mapa(m) => {
                                // pula os apagados
                                while (pos as usize) < m.itens.len() && m.itens[pos as usize].is_none() {
                                    pos += 1;
                                }
                                m.itens.get(pos as usize).and_then(|x| x.as_ref()).map(|(k, x)| (k.clone(), x.clone()))
                            }
                            _ => None,
                        };
                        if let Classe::Iterador(_, p, _) = &mut self.heap.get_mut(r).classe {
                            *p = pos;
                        }
                        item.map(|(k, x)| match tipo % 10 {
                            1 => k,
                            2 => (k, x).into_par(self),
                            _ => {
                                if tipo >= 10 {
                                    // Set: o valor e a chave
                                    k
                                } else {
                                    x
                                }
                            }
                        })
                    }
                    _ => None,
                };
                if let Classe::Iterador(_, p, _) = &mut self.heap.get_mut(r).classe {
                    *p += 1;
                }
                return Ok(res);
            }
        }
        let next = self.pegar(it, chave("next"))?;
        let res = self.chamar(&next, it, &[])?;
        if !matches!(res, Valor::Obj(_)) {
            return self.erro_tipo("resultado de iterador não é objeto");
        }
        let feito = self.pegar(&res, chave("done"))?;
        if feito.verdadeiro() {
            return Ok(None);
        }
        Ok(Some(self.pegar(&res, chave("value"))?))
    }

    pub fn para_vec(&mut self, v: &Valor) -> R<Vec<Valor>> {
        if let Valor::Obj(r) = v {
            if let Classe::Array(a) = &self.heap.get(*r).classe {
                return Ok(a.clone());
            }
        }
        let it = self.iterador(v)?;
        let mut out = Vec::new();
        while let Some(x) = self.proximo(&it)? {
            out.push(x);
            if out.len() > 50_000_000 {
                return self.erro_faixa("iterável grande demais");
            }
        }
        Ok(out)
    }

    pub fn resultado_iter(&mut self, v: Valor, feito: bool) -> Valor {
        let o = self.novo_objeto();
        if let Valor::Obj(r) = o {
            self.heap.get_mut(r).definir(chave("value"), Slot::valor(v));
            self.heap.get_mut(r).definir(chave("done"), Slot::valor(Valor::Bool(feito)));
        }
        o
    }

    // ------------------------------------------------------------- lixo

    pub fn talvez_coletar(&mut self) {
        if self.heap.desde_coleta < 200_000 || self.nativas_ativas > 0 {
            return;
        }
        self.coletar();
    }

    pub fn coletar(&mut self) -> usize {
        let mut raizes: Vec<Valor> = alloc::vec![Valor::Obj(self.global)];
        let p = &self.p;
        for r in [
            p.objeto, p.funcao, p.array, p.texto, p.numero, p.booleano, p.simbolo, p.erro, p.erro_tipo, p.erro_ref, p.erro_faixa,
            p.erro_sintaxe, p.promessa, p.mapa, p.conjunto, p.regex, p.data, p.iterador, p.gerador,
        ] {
            raizes.push(Valor::Obj(r));
        }
        let mut ambs = Vec::new();
        for q in &self.quadros {
            q.raizes(&mut raizes, &mut ambs);
        }
        for t in &self.microtarefas {
            match t {
                Tarefa::Reacao { handler, arg, derivada, .. } => {
                    raizes.push(handler.clone());
                    raizes.push(arg.clone());
                    raizes.push(derivada.clone());
                }
                Tarefa::Chamar(f, a) => {
                    raizes.push(f.clone());
                    raizes.extend(a.iter().cloned());
                }
            }
        }
        raizes.extend(self.raizes_extras.iter().cloned());
        if let Some(h) = &self.anfitriao {
            h.raizes(&mut raizes);
        }
        self.heap.coletar(&raizes, &ambs)
    }

    // ------------------------------------------------------- execucao

    /// Roda um script compilado (topo).
    pub fn rodar(&mut self, proto: Rc<Prototipo>) -> R<Valor> {
        let f = self.criar_funcao(proto, None, None, None);
        let g = Valor::Obj(self.global);
        let r = self.chamar(&f, &g, &[]);
        self.rodar_microtarefas();
        r
    }

    /// Procura o tratador da excecao a partir do quadro do topo (ate
    /// `base`). Ok = achou e o quadro continua; Err = saiu pela base.
    pub fn desenrolar(&mut self, exc: Valor, base: usize) -> R<()> {
        loop {
            let Some(q) = self.quadros.last_mut() else { return Err(exc) };
            if let Some((ip, altura, amb)) = q.tentativas.pop() {
                q.pilha.truncate(altura as usize);
                q.amb = amb;
                q.ip = ip as usize;
                q.pilha.push(exc);
                return Ok(());
            }
            self.quadros.pop();
            if self.quadros.len() <= base {
                return Err(exc);
            }
        }
    }

    fn binario(&mut self, op: Bin, a: Valor, b: Valor) -> R<Valor> {
        // caminho rapido: dois numeros
        if let (Valor::Num(x), Valor::Num(y)) = (&a, &b) {
            let (x, y) = (*x, *y);
            return Ok(match op {
                Bin::Soma => Valor::Num(x + y),
                Bin::Sub => Valor::Num(x - y),
                Bin::Mul => Valor::Num(x * y),
                Bin::Div => Valor::Num(x / y),
                Bin::Resto => Valor::Num(if y == 0.0 { f64::NAN } else { x % y }),
                Bin::Menor => Valor::Bool(x < y),
                Bin::Maior => Valor::Bool(x > y),
                Bin::MenorIgual => Valor::Bool(x <= y),
                Bin::MaiorIgual => Valor::Bool(x >= y),
                Bin::Estrito | Bin::Igual => Valor::Bool(x == y),
                Bin::NaoEstrito | Bin::Diferente => Valor::Bool(x != y),
                Bin::E => Valor::Num((para_i32(x) & para_i32(y)) as f64),
                Bin::Ou => Valor::Num((para_i32(x) | para_i32(y)) as f64),
                Bin::Xor => Valor::Num((para_i32(x) ^ para_i32(y)) as f64),
                Bin::Esq => Valor::Num(para_i32(x).wrapping_shl(para_u32(y) & 31) as f64),
                Bin::Dir => Valor::Num((para_i32(x) >> (para_u32(y) & 31)) as f64),
                Bin::DirSemSinal => Valor::Num((para_u32(x) >> (para_u32(y) & 31)) as f64),
                Bin::Pot => Valor::Num(potencia(x, y)),
                _ => return self.binario_lento(op, a, b),
            });
        }
        self.binario_lento(op, a, b)
    }

    fn binario_lento(&mut self, op: Bin, a: Valor, b: Valor) -> R<Valor> {
        Ok(match op {
            Bin::Soma => {
                let pa = self.para_primitivo(&a, false)?;
                let pb = self.para_primitivo(&b, false)?;
                if matches!(pa, Valor::Texto(_)) || matches!(pb, Valor::Texto(_)) {
                    let sa = self.para_texto(&pa)?;
                    let sb = self.para_texto(&pb)?;
                    let mut s = String::with_capacity(sa.len() + sb.len());
                    s.push_str(&sa);
                    s.push_str(&sb);
                    Valor::Texto(Rc::from(s.as_str()))
                } else {
                    Valor::Num(self.para_numero(&pa)? + self.para_numero(&pb)?)
                }
            }
            Bin::Estrito => Valor::Bool(a == b),
            Bin::NaoEstrito => Valor::Bool(a != b),
            Bin::Igual => Valor::Bool(self.frouxo(&a, &b)?),
            Bin::Diferente => Valor::Bool(!self.frouxo(&a, &b)?),
            Bin::Menor | Bin::Maior | Bin::MenorIgual | Bin::MaiorIgual => {
                let pa = self.para_primitivo(&a, false)?;
                let pb = self.para_primitivo(&b, false)?;
                if let (Valor::Texto(x), Valor::Texto(y)) = (&pa, &pb) {
                    let o = x.as_ref().cmp(y.as_ref());
                    return Ok(Valor::Bool(match op {
                        Bin::Menor => o.is_lt(),
                        Bin::Maior => o.is_gt(),
                        Bin::MenorIgual => o.is_le(),
                        _ => o.is_ge(),
                    }));
                }
                let x = self.para_numero(&pa)?;
                let y = self.para_numero(&pb)?;
                return self.binario(op, Valor::Num(x), Valor::Num(y));
            }
            Bin::InstanceOf => Valor::Bool(self.instancia_de(&a, &b)?),
            Bin::Em => {
                let Valor::Obj(r) = b else { return self.erro_tipo("'in' precisa de objeto") };
                let c = self.chave_de(&a)?;
                Valor::Bool(self.tem_propriedade(r, c))
            }
            _ => {
                let x = self.para_numero(&a)?;
                let y = self.para_numero(&b)?;
                return self.binario(op, Valor::Num(x), Valor::Num(y));
            }
        })
    }

    /// Executa ate o quadro `base` terminar (retorno, yield ou await).
    pub fn executar(&mut self, base: usize) -> R<Saida> {
        'quadro: loop {
            macro_rules! lancar {
                ($e:expr) => {{
                    let e = $e;
                    self.desenrolar(e, base)?;
                    continue 'quadro;
                }};
            }
            macro_rules! t {
                ($e:expr) => {
                    match $e {
                        Ok(v) => v,
                        Err(e) => lancar!(e),
                    }
                };
            }
            let fi = self.quadros.len() - 1;
            let proto = self.quadros[fi].proto.clone();
            let codigo = &proto.codigo;
            macro_rules! q {
                () => {
                    self.quadros[fi]
                };
            }
            macro_rules! pop {
                () => {
                    q!().pilha.pop().unwrap_or_default()
                };
            }
            macro_rules! push {
                ($v:expr) => {{
                    let v = $v;
                    q!().pilha.push(v)
                }};
            }
            loop {
                let ip = q!().ip;
                let Some(op) = codigo.get(ip) else {
                    // fim do codigo sem retorno
                    push!(Valor::Indef);
                    q!().ip = codigo.len();
                    break;
                };
                q!().ip = ip + 1;
                match op {
                    Op::Indef => push!(Valor::Indef),
                    Op::Nulo => push!(Valor::Nulo),
                    Op::Bool(b) => push!(Valor::Bool(*b)),
                    Op::Num(n) => push!(Valor::Num(*n)),
                    Op::Texto(i) => push!(Valor::Texto(proto.textos[*i as usize].clone())),
                    Op::Pop => {
                        q!().pilha.pop();
                    }
                    Op::Dup => {
                        let v = q!().pilha.last().cloned().unwrap_or_default();
                        push!(v);
                    }
                    Op::Dup2 => {
                        let n = q!().pilha.len();
                        let a = q!().pilha[n - 2].clone();
                        let b = q!().pilha[n - 1].clone();
                        push!(a);
                        push!(b);
                    }
                    Op::Troca => {
                        let n = q!().pilha.len();
                        q!().pilha.swap(n - 1, n - 2);
                    }
                    Op::Rot3 => {
                        let c = pop!();
                        let n = q!().pilha.len();
                        q!().pilha.insert(n - 2, c);
                    }
                    Op::Rot4 => {
                        let d = pop!();
                        let n = q!().pilha.len();
                        q!().pilha.insert(n - 3, d);
                    }
                    Op::CarregaLocal(i) => {
                        let v = q!().locais[*i as usize].clone();
                        push!(v);
                    }
                    Op::GuardaLocal(i) => {
                        let v = pop!();
                        q!().locais[*i as usize] = v;
                    }
                    Op::Argumento(k) => {
                        let v = q!().args.get(*k as usize).cloned().unwrap_or_default();
                        push!(v);
                    }
                    Op::RestoArgs(k) => {
                        let resto: Vec<Valor> = q!().args.iter().skip(*k as usize).cloned().collect();
                        let a = self.novo_array(resto);
                        push!(a);
                    }
                    Op::PropriaFuncao => {
                        let f = q!().funcao.clone();
                        push!(f);
                    }
                    Op::CarregaAmb(saltos, i) => {
                        let mut a = q!().amb.clone();
                        for _ in 0..*saltos {
                            a = a.and_then(|x| x.borrow().pai.clone());
                        }
                        let v = a.map(|x| x.borrow().vars.get(*i as usize).cloned().unwrap_or_default()).unwrap_or_default();
                        push!(v);
                    }
                    Op::GuardaAmb(saltos, i) => {
                        let v = pop!();
                        let mut a = q!().amb.clone();
                        for _ in 0..*saltos {
                            a = a.and_then(|x| x.borrow().pai.clone());
                        }
                        if let Some(a) = a {
                            let mut b = a.borrow_mut();
                            if let Some(slot) = b.vars.get_mut(*i as usize) {
                                *slot = v;
                            }
                        }
                    }
                    Op::CarregaGlobal(c) => {
                        let g = self.global;
                        // caminho rapido: propriedade propria comum
                        let rapido = self.heap.get(g).slot(*c).filter(|s| s.acessor.is_none()).map(|s| s.v.clone());
                        match rapido {
                            Some(v) => push!(v),
                            None => {
                                if !self.tem_propriedade(g, *c) {
                                    let n = nome_da_chave(*c);
                                    lancar!(self.erro_ref::<()>(&format!("{n} não está definido")).unwrap_err());
                                }
                                let v = t!(self.pegar_em(g, *c, &Valor::Obj(g)));
                                push!(v);
                            }
                        }
                    }
                    Op::GuardaGlobal(c) => {
                        let v = pop!();
                        let g = Valor::Obj(self.global);
                        t!(self.por(&g, *c, v));
                    }
                    Op::DeclaraGlobal(c) => {
                        let g = self.global;
                        if self.heap.get(g).achar(*c).is_none() && !self.tem_propriedade(g, *c) {
                            self.heap.get_mut(g).definir(*c, Slot::valor(Valor::Indef));
                        }
                    }
                    Op::TypeofGlobal(c) => {
                        let g = self.global;
                        let v = if self.tem_propriedade(g, *c) { t!(self.pegar_em(g, *c, &Valor::Obj(g))) } else { Valor::Indef };
                        let s = self.typeof_(&v);
                        push!(Valor::texto(s));
                    }
                    Op::EntraAmb(n) => {
                        let pai = q!().amb.clone();
                        q!().amb = Some(novo_amb(*n as usize, pai));
                    }
                    Op::SaiAmb => {
                        let pai = q!().amb.as_ref().and_then(|a| a.borrow().pai.clone());
                        q!().amb = pai;
                    }
                    Op::CopiaAmb => {
                        let atual = q!().amb.clone();
                        if let Some(a) = atual {
                            let (vars, pai) = {
                                let b = a.borrow();
                                (b.vars.clone(), b.pai.clone())
                            };
                            q!().amb = Some(Rc::new(core::cell::RefCell::new(Ambiente { vars, pai })));
                        }
                    }
                    Op::This => {
                        let t = q!().this.clone();
                        push!(t);
                    }
                    Op::NovoAlvo => {
                        let t = q!().novo_alvo.clone();
                        push!(t);
                    }
                    Op::Argumentos => {
                        let a = q!().args.clone();
                        let p = self.p.objeto;
                        let r = self.alocar(Some(p), Classe::Argumentos(a));
                        push!(Valor::Obj(r));
                    }
                    Op::Pega(c) => {
                        let o = pop!();
                        let v = t!(self.pegar(&o, *c));
                        push!(v);
                    }
                    Op::PegaIdx => {
                        let k = pop!();
                        let o = pop!();
                        let v = t!(self.pegar_idx(&o, &k));
                        push!(v);
                    }
                    Op::Poe(c) => {
                        let v = pop!();
                        let o = pop!();
                        t!(self.por(&o, *c, v.clone()));
                        push!(v);
                    }
                    Op::PoeIdx => {
                        let v = pop!();
                        let k = pop!();
                        let o = pop!();
                        t!(self.por_idx(&o, &k, v.clone()));
                        push!(v);
                    }
                    Op::Apaga(c) => {
                        let o = pop!();
                        let ok = match o {
                            Valor::Obj(r) => self.heap.get_mut(r).remover(*c),
                            _ => true,
                        };
                        push!(Valor::Bool(ok));
                    }
                    Op::ApagaIdx => {
                        let k = pop!();
                        let o = pop!();
                        let c = t!(self.chave_de(&k));
                        let ok = match o {
                            Valor::Obj(r) => {
                                if let (Classe::Array(a), Some(i)) = (&mut self.heap.get_mut(r).classe, Self::indice_de_chave(c)) {
                                    if i < a.len() {
                                        a[i] = Valor::Indef;
                                    }
                                    true
                                } else {
                                    self.heap.get_mut(r).remover(c)
                                }
                            }
                            _ => true,
                        };
                        push!(Valor::Bool(ok));
                    }
                    Op::PegaSuper(c) => {
                        let v = t!(self.pegar_super(fi, *c));
                        push!(v);
                    }
                    Op::PegaSuperIdx => {
                        let k = pop!();
                        let c = t!(self.chave_de(&k));
                        let v = t!(self.pegar_super(fi, c));
                        push!(v);
                    }
                    Op::ChamaSuper(n) => {
                        let n = *n as usize;
                        let l = q!().pilha.len();
                        let args: Vec<Valor> = q!().pilha.drain(l - n..).collect();
                        let v = t!(self.chamar_super(fi, args));
                        push!(v);
                    }
                    Op::ChamaSuperEspalhado => {
                        let a = pop!();
                        let args = t!(self.para_vec(&a));
                        let v = t!(self.chamar_super(fi, args));
                        push!(v);
                    }
                    Op::PegaPrivado(c) => {
                        let o = pop!();
                        let v = t!(self.pegar(&o, *c));
                        push!(v);
                    }
                    Op::PoePrivado(c) => {
                        let v = pop!();
                        let o = pop!();
                        if let Valor::Obj(r) = o {
                            match self.heap.get_mut(r).slot_mut(*c) {
                                Some(s) if s.acessor.is_some() => {
                                    let set = s.acessor.as_ref().map(|a| a.1.clone()).unwrap_or_default();
                                    t!(self.chamar(&set, &o, &[v.clone()]));
                                }
                                Some(s) => s.v = v.clone(),
                                None => self.heap.get_mut(r).definir(*c, Slot::oculto(v.clone())),
                            }
                        }
                        push!(v);
                    }
                    Op::Bin(b) => {
                        let y = pop!();
                        let x = pop!();
                        let v = t!(self.binario(*b, x, y));
                        push!(v);
                    }
                    Op::Un(u) => {
                        let x = pop!();
                        let v = match u {
                            Un::Nao => Valor::Bool(!x.verdadeiro()),
                            Un::Typeof => Valor::texto(self.typeof_(&x)),
                            Un::Void => Valor::Indef,
                            Un::Neg => Valor::Num(-t!(self.para_numero(&x))),
                            Un::Mais => Valor::Num(t!(self.para_numero(&x))),
                            Un::Til => Valor::Num(!para_i32(t!(self.para_numero(&x))) as f64),
                        };
                        push!(v);
                    }
                    Op::ParaNumero => {
                        let x = pop!();
                        let n = match x {
                            Valor::Num(n) => n,
                            _ => t!(self.para_numero(&x)),
                        };
                        push!(Valor::Num(n));
                    }
                    Op::ParaChave => {
                        let x = pop!();
                        let v = match x {
                            Valor::Num(_) | Valor::Texto(_) | Valor::Simbolo(_) => x,
                            _ => {
                                let c = t!(self.chave_de(&x));
                                if c & SIMBOLO != 0 { Valor::Simbolo(c) } else { Valor::texto(nome_da_chave(c)) }
                            }
                        };
                        push!(v);
                    }
                    Op::Salto(a) => {
                        let a = *a as usize;
                        if a <= ip {
                            // laco: passos e coleta de lixo
                            self.passos += 1;
                            if self.limite_passos > 0 && self.passos > self.limite_passos {
                                lancar!(self.erro_faixa::<()>("script demorou demais").unwrap_err());
                            }
                            self.talvez_coletar();
                        }
                        q!().ip = a;
                    }
                    Op::SeFalso(a) => {
                        if !pop!().verdadeiro() {
                            q!().ip = *a as usize;
                        }
                    }
                    Op::SeVerdade(a) => {
                        if pop!().verdadeiro() {
                            q!().ip = *a as usize;
                        }
                    }
                    Op::SeFalsoMantem(a) => {
                        if !q!().pilha.last().is_some_and(|v| v.verdadeiro()) {
                            q!().ip = *a as usize;
                        } else {
                            q!().pilha.pop();
                        }
                    }
                    Op::SeVerdadeMantem(a) => {
                        if q!().pilha.last().is_some_and(|v| v.verdadeiro()) {
                            q!().ip = *a as usize;
                        } else {
                            q!().pilha.pop();
                        }
                    }
                    Op::SeNaoNuloMantem(a) => {
                        if q!().pilha.last().is_some_and(|v| !v.eh_nulo_ou_indef()) {
                            q!().ip = *a as usize;
                        } else {
                            q!().pilha.pop();
                        }
                    }
                    Op::SeDefinidoMantem(a) => {
                        if q!().pilha.last().is_some_and(|v| !matches!(v, Valor::Indef)) {
                            q!().ip = *a as usize;
                        } else {
                            q!().pilha.pop();
                        }
                    }
                    Op::SeNuloSalta(a) => {
                        if q!().pilha.last().is_some_and(|v| v.eh_nulo_ou_indef()) {
                            q!().pilha.pop();
                            push!(Valor::Indef);
                            q!().ip = *a as usize;
                        }
                    }
                    Op::SeNuloSalta2(a) => {
                        let n = q!().pilha.len();
                        if q!().pilha[n - 2].eh_nulo_ou_indef() {
                            q!().pilha.truncate(n - 2);
                            push!(Valor::Indef);
                            q!().ip = *a as usize;
                        }
                    }
                    Op::Chama(n) => {
                        let n = *n as usize;
                        let l = q!().pilha.len();
                        let args: Vec<Valor> = q!().pilha.drain(l - n..).collect();
                        let this = pop!();
                        let f = pop!();
                        // JS chamando JS: so empurra o quadro
                        if let Valor::Obj(r) = &f {
                            if let Classe::Funcao { proto: p2, .. } = &self.heap.get(*r).classe {
                                if !p2.geradora && !p2.assincrona && !p2.eh_classe {
                                    t!(self.empurrar_quadro(*r, this, args, Valor::Indef, false));
                                    continue 'quadro;
                                }
                            }
                        }
                        if !self.eh_chamavel(&f) {
                            let nome = self.nome_do_alvo(&proto, ip);
                            lancar!(self.erro_tipo::<()>(&format!("{nome} não é uma função")).unwrap_err());
                        }
                        let v = t!(self.chamar(&f, &this, &args));
                        push!(v);
                    }
                    Op::ChamaEspalhado => {
                        let a = pop!();
                        let args = t!(self.para_vec(&a));
                        let this = pop!();
                        let f = pop!();
                        if !self.eh_chamavel(&f) {
                            lancar!(self.erro_tipo::<()>("não é uma função").unwrap_err());
                        }
                        let v = t!(self.chamar(&f, &this, &args));
                        push!(v);
                    }
                    Op::Novo(n) => {
                        let n = *n as usize;
                        let l = q!().pilha.len();
                        let args: Vec<Valor> = q!().pilha.drain(l - n..).collect();
                        let f = pop!();
                        let v = t!(self.construir(&f, &args, &f));
                        push!(v);
                    }
                    Op::NovoEspalhado => {
                        let a = pop!();
                        let args = t!(self.para_vec(&a));
                        let f = pop!();
                        let v = t!(self.construir(&f, &args, &f));
                        push!(v);
                    }
                    Op::Retorna => {
                        let v = pop!();
                        let q = self.quadros.pop().unwrap_or_else(|| unreachable!());
                        let v = if q.construindo && !matches!(v, Valor::Obj(_)) { q.this } else { v };
                        if self.quadros.len() <= base {
                            return Ok(Saida::Retorno(v));
                        }
                        let l = self.quadros.len() - 1;
                        self.quadros[l].pilha.push(v);
                        continue 'quadro;
                    }
                    Op::Closure(i) => {
                        let filho = proto.filhos[*i as usize].clone();
                        let amb = q!().amb.clone();
                        let (this, casa) = if filho.seta {
                            let casa = match &q!().funcao {
                                Valor::Obj(r) => match &self.heap.get(*r).classe {
                                    Classe::Funcao { casa, .. } => *casa,
                                    _ => None,
                                },
                                _ => None,
                            };
                            (Some(q!().this.clone()), casa)
                        } else {
                            (None, None)
                        };
                        let f = self.criar_funcao(filho, amb, this, casa);
                        push!(f);
                    }
                    Op::Classe(i, tem_pai) => {
                        let filho = proto.filhos[*i as usize].clone();
                        let amb = q!().amb.clone();
                        let pai = if *tem_pai { Some(pop!()) } else { None };
                        let f = self.criar_funcao(filho, amb, None, None);
                        let fr = f.obj().unwrap_or(0);
                        let po = self.p.objeto;
                        let proto_pai = match &pai {
                            Some(Valor::Nulo) => None,
                            Some(p) => {
                                if !self.eh_chamavel(p) {
                                    lancar!(self.erro_tipo::<()>("extends precisa de uma classe").unwrap_err());
                                }
                                match t!(self.pegar(p, chave("prototype"))) {
                                    Valor::Obj(x) => Some(x),
                                    _ => None,
                                }
                            }
                            None => Some(po),
                        };
                        let pr = self.alocar(proto_pai, Classe::Comum);
                        self.heap.get_mut(pr).definir(chave("constructor"), Slot::oculto(f.clone()));
                        self.heap.get_mut(fr).definir(
                            chave("prototype"),
                            Slot { v: Valor::Obj(pr), acessor: None, enumeravel: false, gravavel: false, configuravel: false },
                        );
                        if let Some(Valor::Obj(p)) = &pai {
                            self.heap.get_mut(fr).proto = Some(*p);
                            // herda o inicializador de campos? (cada classe tem o seu)
                        }
                        if let Classe::Funcao { casa, .. } = &mut self.heap.get_mut(fr).classe {
                            *casa = Some(pr);
                        }
                        push!(f);
                        push!(Valor::Obj(pr));
                    }
                    Op::DefineMetodo(c, estatico, tipo) => {
                        let fv = pop!();
                        let n = q!().pilha.len();
                        let alvo = if *estatico { q!().pilha[n - 2].clone() } else { q!().pilha[n - 1].clone() };
                        self.definir_metodo(&alvo, *c, fv, *tipo);
                    }
                    Op::DefineMetodoIdx(estatico, tipo) => {
                        let fv = pop!();
                        let k = pop!();
                        let c = t!(self.chave_de(&k));
                        let n = q!().pilha.len();
                        let alvo = if *estatico { q!().pilha[n - 2].clone() } else { q!().pilha[n - 1].clone() };
                        self.definir_metodo(&alvo, c, fv, *tipo);
                    }
                    Op::DefineCampos => {
                        let init = pop!();
                        let n = q!().pilha.len();
                        let fv = q!().pilha[n - 2].clone();
                        let proto_casa = q!().pilha[n - 1].obj();
                        if let (Valor::Obj(f), Valor::Obj(i)) = (fv, init) {
                            if let Classe::Funcao { casa, .. } = &mut self.heap.get_mut(i).classe {
                                *casa = proto_casa;
                            }
                            if let Classe::Funcao { campos, .. } = &mut self.heap.get_mut(f).classe {
                                *campos = Some(i);
                            }
                        }
                    }
                    Op::NovoObjeto => {
                        let o = self.novo_objeto();
                        push!(o);
                    }
                    Op::NovoArray(n) => {
                        let n = *n as usize;
                        let l = q!().pilha.len();
                        let itens: Vec<Valor> = q!().pilha.drain(l - n..).collect();
                        let a = self.novo_array(itens);
                        push!(a);
                    }
                    Op::ArrayEmpurra => {
                        let v = pop!();
                        if let Some(Valor::Obj(r)) = q!().pilha.last() {
                            let r = *r;
                            if let Classe::Array(a) = &mut self.heap.get_mut(r).classe {
                                a.push(v);
                            }
                        }
                    }
                    Op::ArrayBuraco => {
                        if let Some(Valor::Obj(r)) = q!().pilha.last() {
                            let r = *r;
                            if let Classe::Array(a) = &mut self.heap.get_mut(r).classe {
                                a.push(Valor::Indef);
                            }
                        }
                    }
                    Op::ArrayEspalha => {
                        let it = pop!();
                        let mut itens = Vec::new();
                        loop {
                            match t!(self.proximo(&it)) {
                                Some(v) => itens.push(v),
                                None => break,
                            }
                        }
                        if let Some(Valor::Obj(r)) = q!().pilha.last() {
                            let r = *r;
                            if let Classe::Array(a) = &mut self.heap.get_mut(r).classe {
                                a.extend(itens);
                            }
                        }
                    }
                    Op::DefineProp(c) => {
                        let v = pop!();
                        if let Some(Valor::Obj(r)) = q!().pilha.last() {
                            let r = *r;
                            if nome_da_chave(*c) == "__proto__" && *c & SIMBOLO == 0 {
                                match v {
                                    Valor::Obj(p) => self.heap.get_mut(r).proto = Some(p),
                                    Valor::Nulo => self.heap.get_mut(r).proto = None,
                                    _ => {}
                                }
                            } else {
                                self.definir_propria(r, *c, v);
                            }
                        }
                    }
                    Op::DefinePropIdx => {
                        let v = pop!();
                        let k = pop!();
                        let c = t!(self.chave_de(&k));
                        if let Some(Valor::Obj(r)) = q!().pilha.last() {
                            let r = *r;
                            self.definir_propria(r, c, v);
                        }
                    }
                    Op::DefineAcessor(c, tipo) => {
                        let f = pop!();
                        if let Some(Valor::Obj(r)) = q!().pilha.last() {
                            let r = *r;
                            self.definir_acessor(r, *c, f, *tipo, true);
                        }
                    }
                    Op::DefineAcessorIdx(tipo) => {
                        let f = pop!();
                        let k = pop!();
                        let c = t!(self.chave_de(&k));
                        if let Some(Valor::Obj(r)) = q!().pilha.last() {
                            let r = *r;
                            self.definir_acessor(r, c, f, *tipo, true);
                        }
                    }
                    Op::CopiaProps => {
                        let fonte = pop!();
                        let Some(Valor::Obj(destino)) = q!().pilha.last().cloned() else { continue };
                        t!(self.copiar_props(destino, &fonte, &[]));
                    }
                    Op::RestoObjeto(i) => {
                        let lista = proto.textos[*i as usize].clone();
                        let excluir: Vec<Chave> = lista.split(',').filter_map(|x| x.parse().ok()).collect();
                        let fonte = q!().pilha.last().cloned().unwrap_or_default();
                        let novo = self.novo_objeto();
                        if let Valor::Obj(d) = novo {
                            t!(self.copiar_props(d, &fonte, &excluir));
                        }
                        push!(novo);
                    }
                    Op::Lanca => {
                        let e = pop!();
                        lancar!(e);
                    }
                    Op::Tenta(alvo) => {
                        let altura = q!().pilha.len() as u32;
                        let amb = q!().amb.clone();
                        q!().tentativas.push((*alvo, altura, amb));
                    }
                    Op::FimTenta => {
                        q!().tentativas.pop();
                    }
                    Op::PegaIter | Op::PegaIterAssinc => {
                        let v = pop!();
                        let it = t!(self.iterador(&v));
                        push!(it);
                    }
                    Op::ProxIter => {
                        let it = q!().pilha.last().cloned().unwrap_or_default();
                        match t!(self.proximo(&it)) {
                            Some(v) => {
                                push!(v);
                                push!(Valor::Bool(false));
                            }
                            None => {
                                push!(Valor::Indef);
                                push!(Valor::Bool(true));
                            }
                        }
                    }
                    Op::FechaIter => {
                        let it = pop!();
                        if let Valor::Obj(r) = &it {
                            if !matches!(self.heap.get(*r).classe, Classe::Iterador(..)) {
                                if let Ok(f) = self.pegar(&it, chave("return")) {
                                    if self.eh_chamavel(&f) {
                                        let _ = self.chamar(&f, &it, &[]);
                                    }
                                }
                            }
                        }
                    }
                    Op::ChavesEm => {
                        let o = pop!();
                        let chaves = match &o {
                            Valor::Obj(r) => {
                                let mut todas: Vec<Valor> = Vec::new();
                                let mut atual = Some(*r);
                                while let Some(x) = atual {
                                    for k in self.chaves_proprias(x, true) {
                                        if !todas.contains(&k) {
                                            todas.push(k);
                                        }
                                    }
                                    atual = self.heap.get(x).proto;
                                }
                                todas
                            }
                            Valor::Texto(s) => (0..s.encode_utf16().count()).map(|i| Valor::texto(&numero_texto(i as f64))).collect(),
                            _ => Vec::new(),
                        };
                        let arr = self.novo_array(chaves);
                        let pi = self.p.iterador;
                        let it = self.alocar(Some(pi), Classe::Iterador(arr, 0, 0));
                        push!(Valor::Obj(it));
                    }
                    Op::Yield => {
                        let v = pop!();
                        if fi != base {
                            lancar!(self.erro_tipo::<()>("yield fora do gerador").unwrap_err());
                        }
                        return Ok(Saida::Yield(v));
                    }
                    Op::YieldDelegado => {
                        lancar!(self.erro_tipo::<()>("yield* interno").unwrap_err());
                    }
                    Op::Await => {
                        let v = pop!();
                        if fi != base || !proto.assincrona {
                            // await no topo do script: so desembrulha promessas ja prontas
                            let pronto = match &v {
                                Valor::Obj(r) => match &self.heap.get(*r).classe {
                                    Classe::Promessa(p) => match p.estado {
                                        EstadoPromessa::Cumprida => Some(Ok(p.valor.clone())),
                                        EstadoPromessa::Rejeitada => Some(Err(p.valor.clone())),
                                        _ => None,
                                    },
                                    _ => None,
                                },
                                _ => None,
                            };
                            match pronto {
                                Some(Ok(x)) => push!(x),
                                Some(Err(e)) => lancar!(e),
                                None => push!(v),
                            }
                            continue;
                        }
                        return Ok(Saida::Await(v));
                    }
                    Op::Template(n) => {
                        let n = *n as usize;
                        let l = q!().pilha.len();
                        let partes: Vec<Valor> = q!().pilha.drain(l - n..).collect();
                        let mut s = String::new();
                        for p in &partes {
                            let t = t!(self.para_texto(p));
                            s.push_str(&t);
                        }
                        push!(Valor::Texto(Rc::from(s.as_str())));
                    }
                    Op::Regex(c, f) => {
                        let corpo = proto.textos[*c as usize].clone();
                        let flags = proto.textos[*f as usize].clone();
                        let v = t!(super::embutidos::nova_regex(self, &corpo, &flags));
                        push!(v);
                    }
                    Op::NomeFuncao(c) => {
                        // funcao anonima ganha o nome da variavel
                        if let Some(Valor::Obj(r)) = q!().pilha.last() {
                            let r = *r;
                            let sem_nome = match &self.heap.get(r).classe {
                                Classe::Funcao { proto, .. } => proto.nome.is_empty(),
                                _ => false,
                            };
                            if sem_nome && self.heap.get(r).achar(chave("name")).is_none() {
                                let s = Slot { v: Valor::texto(nome_da_chave(*c)), acessor: None, enumeravel: false, gravavel: false, configuravel: true };
                                self.heap.get_mut(r).definir(chave("name"), s);
                            }
                        }
                    }
                    Op::Debug => {}
                }
            }
        }
    }

    fn nome_do_alvo(&self, proto: &Prototipo, ip: usize) -> String {
        // procura o Pega anterior para dizer o nome
        for k in (0..ip).rev().take(4) {
            match &proto.codigo[k] {
                Op::Pega(c) | Op::CarregaGlobal(c) => return String::from(nome_da_chave(*c)),
                _ => {}
            }
        }
        String::from("valor")
    }

    fn definir_metodo(&mut self, alvo: &Valor, c: Chave, f: Valor, tipo: u8) {
        let Valor::Obj(a) = alvo else { return };
        if let Valor::Obj(fr) = &f {
            if let Classe::Funcao { casa, .. } = &mut self.heap.get_mut(*fr).classe {
                *casa = Some(*a);
            }
        }
        if tipo == 0 {
            self.heap.get_mut(*a).definir(c, Slot::oculto(f));
        } else {
            self.definir_acessor(*a, c, f, tipo - 1, false);
        }
    }

    pub fn definir_acessor(&mut self, r: Ref, c: Chave, f: Valor, tipo: u8, enumeravel: bool) {
        let o = self.heap.get_mut(r);
        let (mut g, mut s) = match o.slot(c).and_then(|x| x.acessor.as_ref()) {
            Some(a) => (a.0.clone(), a.1.clone()),
            None => (Valor::Indef, Valor::Indef),
        };
        if tipo == 0 {
            g = f;
        } else {
            s = f;
        }
        o.definir(c, Slot { v: Valor::Indef, acessor: Some(Box::new((g, s))), enumeravel, gravavel: true, configuravel: true });
    }

    pub fn copiar_props(&mut self, destino: Ref, fonte: &Valor, excluir: &[Chave]) -> R<()> {
        match fonte {
            Valor::Obj(r) => {
                let r = *r;
                let chaves = self.chaves_proprias(r, true);
                for k in chaves {
                    let c = self.chave_de(&k)?;
                    if excluir.contains(&c) {
                        continue;
                    }
                    let v = self.pegar(fonte, c)?;
                    self.definir_propria(destino, c, v);
                }
                // simbolos enumeraveis
                let simbolos: Vec<(Chave, Valor)> =
                    self.heap.get(r).props.iter().filter(|(c, s)| c & SIMBOLO != 0 && s.enumeravel).map(|(c, s)| (*c, s.v.clone())).collect();
                for (c, v) in simbolos {
                    self.definir_propria(destino, c, v);
                }
            }
            Valor::Texto(s) => {
                for (i, ch) in s.chars().enumerate() {
                    let mut b = [0u8; 4];
                    self.definir_propria(destino, chave(&numero_texto(i as f64)), Valor::texto(ch.encode_utf8(&mut b)));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn casa_do_quadro(&self, fi: usize) -> Option<Ref> {
        match &self.quadros[fi].funcao {
            Valor::Obj(r) => match &self.heap.get(*r).classe {
                Classe::Funcao { casa, .. } => *casa,
                _ => None,
            },
            _ => None,
        }
    }

    fn pegar_super(&mut self, fi: usize, c: Chave) -> R<Valor> {
        let Some(casa) = self.casa_do_quadro(fi) else { return self.erro_tipo("super fora de método") };
        let this = self.quadros[fi].this.clone();
        match self.heap.get(casa).proto {
            Some(p) => self.pegar_em(p, c, &this),
            None => Ok(Valor::Indef),
        }
    }

    fn chamar_super(&mut self, fi: usize, args: Vec<Valor>) -> R<Valor> {
        // o construtor pai e o prototipo da classe atual
        let f = self.quadros[fi].funcao.clone();
        let Valor::Obj(fr) = f else { return self.erro_tipo("super() fora de construtor") };
        // em arrow dentro do construtor, a funcao do quadro e a arrow: sobe pelo this lexico (simplificado)
        let pai = self.heap.get(fr).proto;
        let Some(pai) = pai else { return self.erro_tipo("super() sem classe pai") };
        let novo_alvo = match &self.quadros[fi].novo_alvo {
            Valor::Indef => f.clone(),
            v => v.clone(),
        };
        let this = self.construir(&Valor::Obj(pai), &args, &novo_alvo)?;
        self.quadros[fi].this = this.clone();
        let campos = match &self.heap.get(fr).classe {
            Classe::Funcao { campos, .. } => *campos,
            _ => None,
        };
        if let Some(c) = campos {
            self.chamar_metodo_campos(c, &this)?;
        }
        Ok(this)
    }
}

trait ParIter {
    fn into_par(self, vm: &mut Vm) -> Valor;
}

impl ParIter for (Valor, Valor) {
    fn into_par(self, vm: &mut Vm) -> Valor {
        vm.novo_array(alloc::vec![self.0, self.1])
    }
}

pub fn potencia(x: f64, y: f64) -> f64 {
    use crate::web::mat::F32Ext as _;
    if y.is_nan() {
        return f64::NAN;
    }
    if y == 0.0 {
        return 1.0;
    }
    if y.fract() == 0.0 && y.abs() < 1024.0 {
        let mut r = 1.0f64;
        let mut b = x;
        let mut e = y.abs() as u64;
        while e > 0 {
            if e & 1 == 1 {
                r *= b;
            }
            b *= b;
            e >>= 1;
        }
        return if y < 0.0 { 1.0 / r } else { r };
    }
    super::mat64::pow(x, y)
}
