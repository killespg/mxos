//! Valores e objetos do JavaScript, o heap e o coletor de lixo.
//!
//! - Valor: 16 bytes (indefinido, nulo, booleano, numero f64, texto Rc,
//!   objeto = indice no heap, simbolo).
//! - Nomes de propriedade sao atomos (u32), como as tags do HTML: comparar
//!   chave e comparar inteiro. Simbolos usam o bit alto.
//! - Objetos guardam as propriedades num Vec (poucas: busca linear e mais
//!   rapida que tabela) e ganham um indice BTreeMap quando passam de 12.
//! - Arrays densos ficam num Vec<Valor> a parte.
//! - Coletor marca-e-varre: raizes = pilhas dos quadros, globais e o que o
//!   anfitriao (DOM, timers) segura.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

use super::bytecode::Prototipo;
use crate::web::atomo::Atomo;

pub type Ref = u32;

#[derive(Clone, Debug, Default)]
pub enum Valor {
    #[default]
    Indef,
    Nulo,
    Bool(bool),
    Num(f64),
    Texto(Rc<str>),
    Obj(Ref),
    Simbolo(u32),
}

impl PartialEq for Valor {
    fn eq(&self, o: &Valor) -> bool {
        match (self, o) {
            (Valor::Indef, Valor::Indef) | (Valor::Nulo, Valor::Nulo) => true,
            (Valor::Bool(a), Valor::Bool(b)) => a == b,
            (Valor::Num(a), Valor::Num(b)) => a == b,
            (Valor::Texto(a), Valor::Texto(b)) => Rc::ptr_eq(a, b) || a == b,
            (Valor::Obj(a), Valor::Obj(b)) => a == b,
            (Valor::Simbolo(a), Valor::Simbolo(b)) => a == b,
            _ => false,
        }
    }
}

impl Valor {
    pub fn texto(s: &str) -> Valor {
        Valor::Texto(Rc::from(s))
    }
    pub fn eh_nulo_ou_indef(&self) -> bool {
        matches!(self, Valor::Indef | Valor::Nulo)
    }
    pub fn obj(&self) -> Option<Ref> {
        if let Valor::Obj(r) = self { Some(*r) } else { None }
    }
    pub fn verdadeiro(&self) -> bool {
        match self {
            Valor::Indef | Valor::Nulo => false,
            Valor::Bool(b) => *b,
            Valor::Num(n) => *n != 0.0 && !n.is_nan(),
            Valor::Texto(s) => !s.is_empty(),
            Valor::Obj(_) | Valor::Simbolo(_) => true,
        }
    }
}

/// Chave de propriedade: atomo do nome ou simbolo (bit alto).
pub type Chave = u32;
pub const SIMBOLO: u32 = 0x8000_0000;

pub fn chave(nome: &str) -> Chave {
    Atomo::de(nome).0
}

pub fn nome_da_chave(c: Chave) -> &'static str {
    if c & SIMBOLO != 0 { "" } else { Atomo(c).texto() }
}

/// Simbolos conhecidos.
pub const SIM_ITERADOR: u32 = SIMBOLO;
pub const SIM_ITERADOR_ASSINC: u32 = SIMBOLO | 1;
pub const SIM_TEM_INSTANCIA: u32 = SIMBOLO | 2;
pub const SIM_PARA_PRIMITIVO: u32 = SIMBOLO | 3;
pub const SIM_ETIQUETA: u32 = SIMBOLO | 4;
pub const PRIMEIRO_SIMBOLO_LIVRE: u32 = 16;

#[derive(Clone, Debug)]
pub struct Slot {
    pub v: Valor,
    /// (get, set) para acessores.
    pub acessor: Option<Box<(Valor, Valor)>>,
    pub enumeravel: bool,
    pub gravavel: bool,
    pub configuravel: bool,
}

impl Slot {
    pub fn valor(v: Valor) -> Slot {
        Slot { v, acessor: None, enumeravel: true, gravavel: true, configuravel: true }
    }
    pub fn oculto(v: Valor) -> Slot {
        Slot { v, acessor: None, enumeravel: false, gravavel: true, configuravel: true }
    }
}

/// Um ambiente de variaveis capturadas (closures).
#[derive(Debug, Default)]
pub struct Ambiente {
    pub vars: Vec<Valor>,
    pub pai: Option<Amb>,
}

pub type Amb = Rc<RefCell<Ambiente>>;

pub fn novo_amb(n: usize, pai: Option<Amb>) -> Amb {
    Rc::new(RefCell::new(Ambiente { vars: alloc::vec![Valor::Indef; n], pai }))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EstadoPromessa {
    Pendente,
    Cumprida,
    Rejeitada,
}

#[derive(Debug)]
pub struct Promessa {
    pub estado: EstadoPromessa,
    pub valor: Valor,
    /// (ao cumprir, ao rejeitar, promessa derivada)
    pub reacoes: Vec<(Valor, Valor, Valor)>,
    pub tratada: bool,
}

/// Chave de Map/Set (SameValueZero).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChaveMapa {
    Indef,
    Nulo,
    Bool(bool),
    Num(u64),
    Texto(Rc<str>),
    Obj(Ref),
    Simbolo(u32),
}

impl ChaveMapa {
    pub fn de(v: &Valor) -> ChaveMapa {
        match v {
            Valor::Indef => ChaveMapa::Indef,
            Valor::Nulo => ChaveMapa::Nulo,
            Valor::Bool(b) => ChaveMapa::Bool(*b),
            Valor::Num(n) => {
                let n = if *n == 0.0 { 0.0 } else { *n };
                ChaveMapa::Num(if n.is_nan() { u64::MAX } else { n.to_bits() })
            }
            Valor::Texto(t) => ChaveMapa::Texto(t.clone()),
            Valor::Obj(r) => ChaveMapa::Obj(*r),
            Valor::Simbolo(s) => ChaveMapa::Simbolo(*s),
        }
    }
}

/// Map e Set: ordem de insercao (itens; None = apagado) + indice.
#[derive(Debug, Default)]
pub struct Mapa {
    pub itens: Vec<Option<(Valor, Valor)>>,
    pub indice: BTreeMap<ChaveMapa, usize>,
    pub eh_mapa: bool,
}

impl Mapa {
    pub fn pegar(&self, k: &Valor) -> Option<&Valor> {
        let i = *self.indice.get(&ChaveMapa::de(k))?;
        self.itens[i].as_ref().map(|x| &x.1)
    }
    pub fn por(&mut self, k: Valor, v: Valor) {
        let c = ChaveMapa::de(&k);
        match self.indice.get(&c) {
            Some(&i) => self.itens[i] = Some((k, v)),
            None => {
                self.itens.push(Some((k, v)));
                self.indice.insert(c, self.itens.len() - 1);
            }
        }
    }
    pub fn apagar(&mut self, k: &Valor) -> bool {
        match self.indice.remove(&ChaveMapa::de(k)) {
            Some(i) => {
                self.itens[i] = None;
                // compacta quando metade virou buraco
                if self.indice.len() * 2 < self.itens.len() && self.itens.len() > 32 {
                    let vivos: Vec<(Valor, Valor)> = self.itens.drain(..).flatten().collect();
                    self.indice.clear();
                    for (n, (k, _)) in vivos.iter().enumerate() {
                        self.indice.insert(ChaveMapa::de(k), n);
                    }
                    self.itens = vivos.into_iter().map(Some).collect();
                }
                true
            }
            None => false,
        }
    }
    pub fn tamanho(&self) -> usize {
        self.indice.len()
    }
    pub fn limpar(&mut self) {
        self.itens.clear();
        self.indice.clear();
    }
}

#[derive(Debug)]
pub enum Classe {
    Comum,
    Array(Vec<Valor>),
    Funcao { proto: Rc<Prototipo>, amb: Option<Amb>, this: Option<Valor>, casa: Option<Ref>, campos: Option<Ref> },
    Nativa { id: u16, dado: Valor },
    Ligada { alvo: Valor, this: Valor, args: Vec<Valor> },
    Erro,
    Booleano(bool),
    Numero(f64),
    Texto(Rc<str>),
    Simbolo(u32),
    Data(f64),
    Regex(Rc<super::regex::Regex>),
    Mapa(Box<Mapa>),
    Promessa(Promessa),
    Gerador(Option<Box<super::vm::Quadro>>, bool),
    /// Iterador interno de array/texto/mapa: (alvo, posicao, tipo 0=valores 1=chaves 2=pares)
    Iterador(Valor, u32, u8),
    Dom(u32),
    Argumentos(Vec<Valor>),
    Proxy(Valor, Valor),
    Buffer(Rc<RefCell<Vec<u8>>>),
}

#[derive(Debug)]
pub struct Objeto {
    pub proto: Option<Ref>,
    pub props: Vec<(Chave, Slot)>,
    pub indice: Option<BTreeMap<Chave, u32>>,
    pub classe: Classe,
    pub extensivel: bool,
}

impl Objeto {
    pub fn novo(proto: Option<Ref>, classe: Classe) -> Objeto {
        Objeto { proto, props: Vec::new(), indice: None, classe, extensivel: true }
    }

    pub fn achar(&self, c: Chave) -> Option<usize> {
        if let Some(i) = &self.indice {
            return i.get(&c).map(|&k| k as usize);
        }
        self.props.iter().position(|(k, _)| *k == c)
    }

    pub fn slot(&self, c: Chave) -> Option<&Slot> {
        self.achar(c).map(|i| &self.props[i].1)
    }

    pub fn slot_mut(&mut self, c: Chave) -> Option<&mut Slot> {
        self.achar(c).map(move |i| &mut self.props[i].1)
    }

    pub fn definir(&mut self, c: Chave, s: Slot) {
        match self.achar(c) {
            Some(i) => self.props[i].1 = s,
            None => {
                let k = self.props.len();
                self.props.push((c, s));
                if let Some(ind) = &mut self.indice {
                    ind.insert(c, k as u32);
                } else if self.props.len() > 12 {
                    let mut ind = BTreeMap::new();
                    for (n, (kk, _)) in self.props.iter().enumerate() {
                        ind.insert(*kk, n as u32);
                    }
                    self.indice = Some(ind);
                }
            }
        }
    }

    pub fn remover(&mut self, c: Chave) -> bool {
        match self.achar(c) {
            Some(i) => {
                if !self.props[i].1.configuravel {
                    return false;
                }
                self.props.remove(i);
                if let Some(ind) = &mut self.indice {
                    // sem reconstruir: tira a chave e puxa os indices de depois
                    ind.remove(&c);
                    for v in ind.values_mut() {
                        if *v as usize > i {
                            *v -= 1;
                        }
                    }
                }
                true
            }
            None => true,
        }
    }
}

pub struct Heap {
    pub objs: Vec<Option<Objeto>>,
    livres: Vec<Ref>,
    marcas: Vec<bool>,
    pub desde_coleta: usize,
}

impl Default for Heap {
    fn default() -> Self {
        Heap { objs: Vec::with_capacity(4096), livres: Vec::new(), marcas: Vec::new(), desde_coleta: 0 }
    }
}

impl Heap {
    pub fn alocar(&mut self, o: Objeto) -> Ref {
        self.desde_coleta += 1;
        if let Some(r) = self.livres.pop() {
            self.objs[r as usize] = Some(o);
            return r;
        }
        self.objs.push(Some(o));
        (self.objs.len() - 1) as Ref
    }

    pub fn get(&self, r: Ref) -> &Objeto {
        self.objs[r as usize].as_ref().expect("js: objeto coletado")
    }

    pub fn get_mut(&mut self, r: Ref) -> &mut Objeto {
        self.objs[r as usize].as_mut().expect("js: objeto coletado")
    }

    pub fn vivos(&self) -> usize {
        self.objs.len() - self.livres.len()
    }

    /// Marca-e-varre a partir das raizes dadas. Devolve quantos liberou.
    pub fn coletar(&mut self, raizes: &[Valor], ambs: &[Amb]) -> usize {
        self.marcas.clear();
        self.marcas.resize(self.objs.len(), false);
        let mut pendentes: Vec<Ref> = Vec::with_capacity(1024);
        let mut ambs_vistos: Vec<*const RefCell<Ambiente>> = Vec::new();
        let mut ambs_pendentes: Vec<Amb> = ambs.to_vec();
        for v in raizes {
            if let Valor::Obj(r) = v {
                pendentes.push(*r);
            }
        }
        loop {
            while let Some(r) = pendentes.pop() {
                let i = r as usize;
                if i >= self.marcas.len() || self.marcas[i] {
                    continue;
                }
                self.marcas[i] = true;
                let Some(o) = self.objs[i].as_ref() else { continue };
                if let Some(p) = o.proto {
                    pendentes.push(p);
                }
                let ver = |v: &Valor, pend: &mut Vec<Ref>| {
                    if let Valor::Obj(x) = v {
                        pend.push(*x);
                    }
                };
                for (_, s) in &o.props {
                    ver(&s.v, &mut pendentes);
                    if let Some(a) = &s.acessor {
                        ver(&a.0, &mut pendentes);
                        ver(&a.1, &mut pendentes);
                    }
                }
                match &o.classe {
                    Classe::Array(v) | Classe::Argumentos(v) => v.iter().for_each(|x| ver(x, &mut pendentes)),
                    Classe::Funcao { amb, this, casa, campos, .. } => {
                        if let Some(a) = amb {
                            ambs_pendentes.push(a.clone());
                        }
                        if let Some(t) = this {
                            ver(t, &mut pendentes);
                        }
                        if let Some(c) = casa {
                            pendentes.push(*c);
                        }
                        if let Some(c) = campos {
                            pendentes.push(*c);
                        }
                    }
                    Classe::Nativa { dado, .. } => ver(dado, &mut pendentes),
                    Classe::Ligada { alvo, this, args } => {
                        ver(alvo, &mut pendentes);
                        ver(this, &mut pendentes);
                        args.iter().for_each(|x| ver(x, &mut pendentes));
                    }
                    Classe::Mapa(m) => {
                        for (a, b) in m.itens.iter().flatten() {
                            ver(a, &mut pendentes);
                            ver(b, &mut pendentes);
                        }
                    }
                    Classe::Promessa(p) => {
                        ver(&p.valor, &mut pendentes);
                        for (a, b, c) in &p.reacoes {
                            ver(a, &mut pendentes);
                            ver(b, &mut pendentes);
                            ver(c, &mut pendentes);
                        }
                    }
                    Classe::Gerador(Some(q), _) => {
                        let mut vs = Vec::new();
                        let mut as_ = Vec::new();
                        q.raizes(&mut vs, &mut as_);
                        vs.iter().for_each(|x| ver(x, &mut pendentes));
                        ambs_pendentes.extend(as_);
                    }
                    Classe::Iterador(v, _, _) => ver(v, &mut pendentes),
                    Classe::Proxy(a, b) => {
                        ver(a, &mut pendentes);
                        ver(b, &mut pendentes);
                    }
                    _ => {}
                }
            }
            // ambientes (closures) apontam para objetos
            let Some(a) = ambs_pendentes.pop() else { break };
            let p = Rc::as_ptr(&a);
            if ambs_vistos.contains(&p) {
                continue;
            }
            ambs_vistos.push(p);
            let amb = a.borrow();
            for v in &amb.vars {
                if let Valor::Obj(x) = v {
                    pendentes.push(*x);
                }
            }
            if let Some(pai) = &amb.pai {
                ambs_pendentes.push(pai.clone());
            }
        }
        let mut liberados = 0;
        for i in 0..self.objs.len() {
            if !self.marcas[i] && self.objs[i].is_some() {
                self.objs[i] = None;
                self.livres.push(i as Ref);
                liberados += 1;
            }
        }
        self.desde_coleta = 0;
        liberados
    }
}

// --------------------------------------------------- numeros em texto

/// Number.prototype.toString() (ECMA-262 6.1.6.1.20): o menor texto que
/// volta ao mesmo numero, com expoente so fora de [1e-7, 1e21).
pub fn numero_texto(n: f64) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n == 0.0 {
        return "0".into();
    }
    if n.is_infinite() {
        return if n > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if n < 0.0 {
        return format!("-{}", numero_texto(-n));
    }
    // inteiros "pequenos": caminho rapido
    if n < 1e15 && n == (n as i64) as f64 {
        return format!("{}", n as i64);
    }
    // digitos e expoente do formato cientifico mais curto do Rust
    let e = format!("{:e}", n); // ex.: "1.2345e-7"
    let (mant, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let digitos: String = mant.chars().filter(|c| c.is_ascii_digit()).collect();
    let k = digitos.len() as i32;
    let nn = exp + 1; // posicao do ponto decimal
    if k <= nn && nn <= 21 {
        let mut s = digitos.clone();
        for _ in 0..(nn - k) {
            s.push('0');
        }
        s
    } else if 0 < nn && nn <= 21 {
        format!("{}.{}", &digitos[..nn as usize], &digitos[nn as usize..])
    } else if -6 < nn && nn <= 0 {
        let mut s = String::from("0.");
        for _ in 0..(-nn) {
            s.push('0');
        }
        s.push_str(&digitos);
        s
    } else {
        let sinal = if nn - 1 >= 0 { "+" } else { "-" };
        if k == 1 {
            format!("{}e{}{}", digitos, sinal, (nn - 1).abs())
        } else {
            format!("{}.{}e{}{}", &digitos[..1], &digitos[1..], sinal, (nn - 1).abs())
        }
    }
}

/// Texto -> numero (ToNumber de string): espacos, hexa, Infinity, vazio=0.
pub fn texto_numero(s: &str) -> f64 {
    let t = s.trim_matches(|c: char| c.is_whitespace() || c == '\u{FEFF}');
    if t.is_empty() {
        return 0.0;
    }
    let (neg, sem_sinal) = match t.as_bytes()[0] {
        b'-' => (true, &t[1..]),
        b'+' => (false, &t[1..]),
        _ => (false, t),
    };
    let v = if let Some(h) = sem_sinal.strip_prefix("0x").or_else(|| sem_sinal.strip_prefix("0X")) {
        if neg || t.starts_with('+') {
            return f64::NAN;
        }
        u64::from_str_radix(h, 16).map(|x| x as f64).unwrap_or(f64::NAN)
    } else if let Some(b) = sem_sinal.strip_prefix("0b").or_else(|| sem_sinal.strip_prefix("0B")) {
        u64::from_str_radix(b, 2).map(|x| x as f64).unwrap_or(f64::NAN)
    } else if let Some(o) = sem_sinal.strip_prefix("0o").or_else(|| sem_sinal.strip_prefix("0O")) {
        u64::from_str_radix(o, 8).map(|x| x as f64).unwrap_or(f64::NAN)
    } else if sem_sinal == "Infinity" {
        f64::INFINITY
    } else if sem_sinal.bytes().all(|c| c.is_ascii_digit() || c == b'.' || c == b'e' || c == b'E' || c == b'+' || c == b'-')
        && !sem_sinal.contains("inf")
    {
        sem_sinal.parse::<f64>().unwrap_or(f64::NAN)
    } else {
        f64::NAN
    };
    if neg { -v } else { v }
}

/// ToInt32.
pub fn para_i32(n: f64) -> i32 {
    if !n.is_finite() {
        return 0;
    }
    let m = (n as i64 as f64 == n.trunc_compat()).then_some(n as i64).unwrap_or_else(|| {
        // grande demais para i64: modulo 2^32
        let d = n % 4294967296.0;
        d as i64
    });
    m as i32
}

trait Trunc {
    fn trunc_compat(self) -> f64;
}

impl Trunc for f64 {
    fn trunc_compat(self) -> f64 {
        if self.abs() >= 9.0e15 { self } else { (self as i64) as f64 }
    }
}

pub fn para_u32(n: f64) -> u32 {
    para_i32(n) as u32
}
