//! A biblioteca padrao do JavaScript: Object, Function, Array, String,
//! Number, Boolean, Symbol, Math, JSON, Promise, Map, Set, RegExp, Date,
//! Error, Reflect, Proxy, console, globais (parseInt...) e iteradores.

#[allow(unused_imports)]
use super::mat64::F64Ext;
use alloc::boxed::Box;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::mat64 as m;
use super::regex::Regex;
use super::valor::*;
use super::vm::{Ctx, R, Vm};

// ------------------------------------------------------------ utilidades

fn num(vm: &mut Vm, c: &Ctx, i: usize) -> R<f64> {
    let v = c.arg(i);
    vm.para_numero(&v)
}

fn texto(vm: &mut Vm, c: &Ctx, i: usize) -> R<Rc<str>> {
    let v = c.arg(i);
    vm.para_texto(&v)
}

fn int_arg(vm: &mut Vm, c: &Ctx, i: usize, padrao: f64) -> R<f64> {
    let v = c.arg(i);
    if matches!(v, Valor::Indef) {
        return Ok(padrao);
    }
    let n = vm.para_numero(&v)?;
    Ok(if n.is_nan() { 0.0 } else { m::trunc(n) })
}

/// Indice relativo (negativo conta do fim) limitado a [0, len].
fn relativo(n: f64, len: usize) -> usize {
    let len = len as f64;
    let r = if n < 0.0 { (len + n).max(0.0) } else { n.min(len) };
    r as usize
}

fn this_array(vm: &mut Vm, c: &Ctx) -> R<Ref> {
    match c.this {
        Valor::Obj(r) if matches!(vm.heap.get(*r).classe, Classe::Array(_)) => Ok(*r),
        _ => vm.erro_tipo("this não é array"),
    }
}

/// Os itens de this (array ou array-like).
fn itens_de(vm: &mut Vm, v: &Valor) -> R<Vec<Valor>> {
    if let Valor::Obj(r) = v {
        match &vm.heap.get(*r).classe {
            Classe::Array(a) | Classe::Argumentos(a) => return Ok(a.clone()),
            _ => {}
        }
    }
    if let Valor::Texto(s) = v {
        return Ok(s.chars().map(|ch| {
            let mut b = [0u8; 4];
            Valor::texto(ch.encode_utf8(&mut b))
        }).collect());
    }
    let len = vm.pegar(v, chave("length"))?;
    let n = vm.para_numero(&len)?;
    let n = if n.is_finite() && n > 0.0 { (n as usize).min(10_000_000) } else { 0 };
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        out.push(vm.pegar_idx(v, &Valor::Num(i as f64))?);
    }
    Ok(out)
}

fn arr_mut(vm: &mut Vm, r: Ref) -> &mut Vec<Valor> {
    match &mut vm.heap.get_mut(r).classe {
        Classe::Array(a) => a,
        _ => unreachable!(),
    }
}

fn arr(vm: &Vm, r: Ref) -> &Vec<Valor> {
    match &vm.heap.get(r).classe {
        Classe::Array(a) => a,
        _ => unreachable!(),
    }
}

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn de_utf16(v: &[u16]) -> Valor {
    Valor::Texto(Rc::from(String::from_utf16_lossy(v).as_str()))
}

fn obj_com(vm: &mut Vm, proto: Ref) -> Ref {
    vm.alocar(Some(proto), Classe::Comum)
}

/// Cria um construtor: funcao nativa + prototype ligado.
fn construtor(vm: &mut Vm, nome: &str, f: super::vm::FnNativa, aridade: u8, proto: Ref) -> Ref {
    let ctor = vm.nativa(nome, f, aridade);
    let r = ctor.obj().unwrap_or(0);
    vm.heap.get_mut(r).definir(chave("prototype"), Slot { v: Valor::Obj(proto), acessor: None, enumeravel: false, gravavel: false, configuravel: false });
    vm.heap.get_mut(proto).definir(chave("constructor"), Slot::oculto(ctor.clone()));
    let g = vm.global;
    vm.heap.get_mut(g).definir(chave(nome), Slot::oculto(ctor));
    r
}

fn getter(vm: &mut Vm, obj: Ref, nome: &str, f: super::vm::FnNativa) {
    let g = vm.nativa(nome, f, 0);
    vm.definir_acessor(obj, chave(nome), g, 0, false);
}

// ================================================================ instalar

pub fn instalar(vm: &mut Vm) {
    // prototipos base
    let po = vm.alocar(None, Classe::Comum);
    vm.p.objeto = po;
    let pf = vm.alocar(Some(po), Classe::Comum);
    vm.p.funcao = pf;
    let novo = |vm: &mut Vm, classe: Classe| vm.alocar(Some(po), classe);
    vm.p.array = novo(vm, Classe::Array(Vec::new()));
    vm.p.texto = novo(vm, Classe::Texto(Rc::from("")));
    vm.p.numero = novo(vm, Classe::Numero(0.0));
    vm.p.booleano = novo(vm, Classe::Booleano(false));
    vm.p.simbolo = novo(vm, Classe::Comum);
    vm.p.erro = novo(vm, Classe::Comum);
    let pe = vm.p.erro;
    for campo in 0..5 {
        let r = vm.alocar(Some(pe), Classe::Comum);
        match campo {
            0 => vm.p.erro_tipo = r,
            1 => vm.p.erro_ref = r,
            2 => vm.p.erro_faixa = r,
            3 => vm.p.erro_sintaxe = r,
            _ => {}
        }
    }
    vm.p.promessa = novo(vm, Classe::Comum);
    vm.p.mapa = novo(vm, Classe::Comum);
    vm.p.conjunto = novo(vm, Classe::Comum);
    vm.p.regex = novo(vm, Classe::Comum);
    vm.p.data = novo(vm, Classe::Comum);
    vm.p.iterador = novo(vm, Classe::Comum);
    let pi = vm.p.iterador;
    vm.p.gerador = vm.alocar(Some(pi), Classe::Comum);
    let g = vm.alocar(Some(po), Classe::Comum);
    vm.global = g;
    vm.definir(g, "globalThis", Valor::Obj(g));

    vm.ids.resolver = vm.registrar(nat_resolver);
    vm.ids.rejeitar = vm.registrar(nat_rejeitar);
    vm.ids.async_cumprir = vm.registrar(nat_async_cumprir);
    vm.ids.async_rejeitar = vm.registrar(nat_async_rejeitar);

    instalar_objeto(vm);
    instalar_funcao(vm);
    instalar_array(vm);
    instalar_texto(vm);
    instalar_numero(vm);
    instalar_simbolo(vm);
    instalar_erros(vm);
    instalar_math(vm);
    instalar_json(vm);
    instalar_promessa(vm);
    instalar_colecoes(vm);
    instalar_regex(vm);
    instalar_data(vm);
    instalar_iteradores(vm);
    instalar_globais(vm);
    instalar_reflect(vm);
}

// ================================================================ Object

fn instalar_objeto(vm: &mut Vm) {
    let po = vm.p.objeto;
    let o = construtor(vm, "Object", obj_ctor, 1, po);
    vm.metodo(o, "keys", obj_keys, 1);
    vm.metodo(o, "values", obj_values, 1);
    vm.metodo(o, "entries", obj_entries, 1);
    vm.metodo(o, "assign", obj_assign, 2);
    vm.metodo(o, "create", obj_create, 2);
    vm.metodo(o, "defineProperty", obj_define_property, 3);
    vm.metodo(o, "defineProperties", obj_define_properties, 2);
    vm.metodo(o, "getOwnPropertyNames", obj_own_names, 1);
    vm.metodo(o, "getOwnPropertySymbols", obj_own_symbols, 1);
    vm.metodo(o, "getOwnPropertyDescriptor", obj_descriptor, 2);
    vm.metodo(o, "getOwnPropertyDescriptors", obj_descriptors, 1);
    vm.metodo(o, "getPrototypeOf", obj_get_proto, 1);
    vm.metodo(o, "setPrototypeOf", obj_set_proto, 2);
    vm.metodo(o, "freeze", obj_freeze, 1);
    vm.metodo(o, "seal", obj_freeze, 1);
    vm.metodo(o, "preventExtensions", obj_freeze, 1);
    vm.metodo(o, "isFrozen", obj_is_frozen, 1);
    vm.metodo(o, "isSealed", obj_is_frozen, 1);
    vm.metodo(o, "isExtensible", obj_is_extensible, 1);
    vm.metodo(o, "fromEntries", obj_from_entries, 1);
    vm.metodo(o, "is", obj_is, 2);
    vm.metodo(o, "hasOwn", obj_has_own, 2);
    vm.metodo(o, "groupBy", obj_group_by, 2);
    vm.metodo(po, "hasOwnProperty", obj_has_own_property, 1);
    vm.metodo(po, "isPrototypeOf", obj_is_prototype_of, 1);
    vm.metodo(po, "propertyIsEnumerable", obj_property_is_enumerable, 1);
    vm.metodo(po, "toString", obj_to_string, 0);
    vm.metodo(po, "toLocaleString", obj_to_string, 0);
    vm.metodo(po, "valueOf", obj_value_of, 0);
    // __proto__ como acessor
    let get = vm.nativa("__proto__", obj_proto_get, 0);
    let set = vm.nativa("__proto__", obj_proto_set, 1);
    vm.definir_acessor(po, chave("__proto__"), get, 0, false);
    vm.definir_acessor(po, chave("__proto__"), set, 1, false);
}

fn obj_ctor(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let v = c.arg(0);
    if v.eh_nulo_ou_indef() {
        return Ok(vm.novo_objeto());
    }
    Ok(Valor::Obj(vm.para_objeto(&v)?))
}

fn obj_keys(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let r = vm.para_objeto(&c.arg(0))?;
    let k = vm.chaves_proprias(r, true);
    Ok(vm.novo_array(k))
}

fn obj_values(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let o = c.arg(0);
    let r = vm.para_objeto(&o)?;
    let ks = vm.chaves_proprias(r, true);
    let mut v = Vec::with_capacity(ks.len());
    for k in ks {
        let ch = vm.chave_de(&k)?;
        v.push(vm.pegar(&Valor::Obj(r), ch)?);
    }
    Ok(vm.novo_array(v))
}

fn obj_entries(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let o = c.arg(0);
    let r = vm.para_objeto(&o)?;
    let ks = vm.chaves_proprias(r, true);
    let mut v = Vec::with_capacity(ks.len());
    for k in ks {
        let ch = vm.chave_de(&k)?;
        let x = vm.pegar(&Valor::Obj(r), ch)?;
        v.push(vm.novo_array(alloc::vec![k, x]));
    }
    Ok(vm.novo_array(v))
}

fn obj_assign(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let alvo = vm.para_objeto(&c.arg(0))?;
    for f in c.args.iter().skip(1) {
        if f.eh_nulo_ou_indef() {
            continue;
        }
        // assign usa [[Set]] (setters do alvo valem)
        if let Valor::Obj(r) = f {
            let ks = vm.chaves_proprias(*r, true);
            for k in ks {
                let ch = vm.chave_de(&k)?;
                let v = vm.pegar(f, ch)?;
                vm.por(&Valor::Obj(alvo), ch, v)?;
            }
        } else {
            vm.copiar_props(alvo, f, &[])?;
        }
    }
    Ok(Valor::Obj(alvo))
}

fn obj_create(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let proto = match c.arg(0) {
        Valor::Obj(r) => Some(r),
        Valor::Nulo => None,
        _ => return vm.erro_tipo("Object.create: protótipo inválido"),
    };
    let r = vm.alocar(proto, Classe::Comum);
    let props = c.arg(1);
    if !props.eh_nulo_ou_indef() {
        definir_props(vm, r, &props)?;
    }
    Ok(Valor::Obj(r))
}

fn definir_descritor(vm: &mut Vm, r: Ref, ch: Chave, d: &Valor) -> R<()> {
    let get = vm.pegar(d, chave("get"))?;
    let set = vm.pegar(d, chave("set"))?;
    let tem = |vm: &mut Vm, n: &str| -> bool { matches!(d, Valor::Obj(x) if vm.tem_propriedade(*x, chave(n))) };
    let existente = vm.heap.get(r).slot(ch).cloned();
    let enumeravel = if tem(vm, "enumerable") { vm.pegar(d, chave("enumerable"))?.verdadeiro() } else { existente.as_ref().is_some_and(|s| s.enumeravel) };
    let configuravel = if tem(vm, "configurable") { vm.pegar(d, chave("configurable"))?.verdadeiro() } else { existente.as_ref().is_some_and(|s| s.configuravel) };
    if !get.eh_nulo_ou_indef() || !set.eh_nulo_ou_indef() {
        let (g0, s0) = existente.as_ref().and_then(|s| s.acessor.as_ref()).map(|a| (a.0.clone(), a.1.clone())).unwrap_or_default();
        let g = if tem(vm, "get") { get } else { g0 };
        let s = if tem(vm, "set") { set } else { s0 };
        vm.heap.get_mut(r).definir(ch, Slot { v: Valor::Indef, acessor: Some(Box::new((g, s))), enumeravel, gravavel: true, configuravel });
        return Ok(());
    }
    let valor = if tem(vm, "value") { vm.pegar(d, chave("value"))? } else { existente.as_ref().map(|s| s.v.clone()).unwrap_or_default() };
    let gravavel = if tem(vm, "writable") { vm.pegar(d, chave("writable"))?.verdadeiro() } else { existente.as_ref().is_some_and(|s| s.gravavel) };
    // arrays: indices vao para o vetor
    if let Classe::Array(_) = vm.heap.get(r).classe {
        if nome_da_chave(ch).parse::<usize>().is_ok() || nome_da_chave(ch) == "length" {
            return vm.por(&Valor::Obj(r), ch, valor);
        }
    }
    vm.heap.get_mut(r).definir(ch, Slot { v: valor, acessor: None, enumeravel, gravavel, configuravel });
    Ok(())
}

fn obj_define_property(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let o = c.arg(0);
    let Valor::Obj(r) = o else { return vm.erro_tipo("defineProperty em não-objeto") };
    let ch = vm.chave_de(&c.arg(1))?;
    definir_descritor(vm, r, ch, &c.arg(2))?;
    Ok(o)
}

fn definir_props(vm: &mut Vm, r: Ref, props: &Valor) -> R<()> {
    let Valor::Obj(p) = props else { return Ok(()) };
    let ks = vm.chaves_proprias(*p, true);
    for k in ks {
        let ch = vm.chave_de(&k)?;
        let d = vm.pegar(props, ch)?;
        definir_descritor(vm, r, ch, &d)?;
    }
    Ok(())
}

fn obj_define_properties(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let o = c.arg(0);
    let Valor::Obj(r) = o else { return vm.erro_tipo("defineProperties em não-objeto") };
    definir_props(vm, r, &c.arg(1))?;
    Ok(o)
}

fn obj_own_names(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let r = vm.para_objeto(&c.arg(0))?;
    let mut k = vm.chaves_proprias(r, false);
    if let Classe::Array(_) = vm.heap.get(r).classe {
        k.push(Valor::texto("length"));
    }
    Ok(vm.novo_array(k))
}

fn obj_own_symbols(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let r = vm.para_objeto(&c.arg(0))?;
    let v: Vec<Valor> = vm.heap.get(r).props.iter().filter(|(k, _)| k & SIMBOLO != 0).map(|(k, _)| Valor::Simbolo(*k)).collect();
    Ok(vm.novo_array(v))
}

fn descritor(vm: &mut Vm, r: Ref, ch: Chave) -> R<Valor> {
    let s = match vm.heap.get(r).slot(ch).cloned() {
        Some(s) => s,
        None => {
            // indices de array e length
            let o = Valor::Obj(r);
            if let Classe::Array(_) = vm.heap.get(r).classe {
                let v = vm.pegar(&o, ch)?;
                if !matches!(v, Valor::Indef) || nome_da_chave(ch) == "length" {
                    let d = vm.novo_objeto();
                    let dr = d.obj().unwrap_or(0);
                    vm.definir_propria(dr, chave("value"), v);
                    vm.definir_propria(dr, chave("writable"), Valor::Bool(true));
                    vm.definir_propria(dr, chave("enumerable"), Valor::Bool(nome_da_chave(ch) != "length"));
                    vm.definir_propria(dr, chave("configurable"), Valor::Bool(nome_da_chave(ch) != "length"));
                    return Ok(d);
                }
            }
            return Ok(Valor::Indef);
        }
    };
    let d = vm.novo_objeto();
    let dr = d.obj().unwrap_or(0);
    match &s.acessor {
        Some(a) => {
            vm.definir_propria(dr, chave("get"), a.0.clone());
            vm.definir_propria(dr, chave("set"), a.1.clone());
        }
        None => {
            vm.definir_propria(dr, chave("value"), s.v.clone());
            vm.definir_propria(dr, chave("writable"), Valor::Bool(s.gravavel));
        }
    }
    vm.definir_propria(dr, chave("enumerable"), Valor::Bool(s.enumeravel));
    vm.definir_propria(dr, chave("configurable"), Valor::Bool(s.configuravel));
    Ok(d)
}

fn obj_descriptor(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let r = vm.para_objeto(&c.arg(0))?;
    let ch = vm.chave_de(&c.arg(1))?;
    descritor(vm, r, ch)
}

fn obj_descriptors(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let r = vm.para_objeto(&c.arg(0))?;
    let chaves: Vec<Chave> = vm.heap.get(r).props.iter().map(|(k, _)| *k).collect();
    let saida = vm.novo_objeto();
    let sr = saida.obj().unwrap_or(0);
    for ch in chaves {
        let d = descritor(vm, r, ch)?;
        vm.definir_propria(sr, ch, d);
    }
    Ok(saida)
}

fn obj_get_proto(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let r = vm.para_objeto(&c.arg(0))?;
    Ok(match vm.heap.get(r).proto {
        Some(p) => Valor::Obj(p),
        None => Valor::Nulo,
    })
}

fn obj_set_proto(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let o = c.arg(0);
    if let Valor::Obj(r) = o {
        vm.heap.get_mut(r).proto = c.arg(1).obj();
    }
    Ok(o)
}

fn obj_proto_get(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let r = vm.para_objeto(c.this)?;
    Ok(vm.heap.get(r).proto.map_or(Valor::Nulo, Valor::Obj))
}

fn obj_proto_set(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    if let Valor::Obj(r) = c.this {
        match c.arg(0) {
            Valor::Obj(p) => vm.heap.get_mut(*r).proto = Some(p),
            Valor::Nulo => vm.heap.get_mut(*r).proto = None,
            _ => {}
        }
    }
    Ok(Valor::Indef)
}

fn obj_freeze(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let o = c.arg(0);
    if let Valor::Obj(r) = o {
        let ob = vm.heap.get_mut(r);
        ob.extensivel = false;
        for (_, s) in ob.props.iter_mut() {
            s.gravavel = false;
            s.configuravel = false;
        }
    }
    Ok(o)
}

fn obj_is_frozen(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(Valor::Bool(match c.arg(0) {
        Valor::Obj(r) => !vm.heap.get(r).extensivel,
        _ => true,
    }))
}

fn obj_is_extensible(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(Valor::Bool(matches!(c.arg(0), Valor::Obj(r) if vm.heap.get(r).extensivel)))
}

fn obj_from_entries(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let itens = vm.para_vec(&c.arg(0))?;
    let o = vm.novo_objeto();
    let r = o.obj().unwrap_or(0);
    for par in itens {
        let k = vm.pegar_idx(&par, &Valor::Num(0.0))?;
        let v = vm.pegar_idx(&par, &Valor::Num(1.0))?;
        let ch = vm.chave_de(&k)?;
        vm.definir_propria(r, ch, v);
    }
    Ok(o)
}

fn mesmo_valor(a: &Valor, b: &Valor) -> bool {
    match (a, b) {
        (Valor::Num(x), Valor::Num(y)) => (x.is_nan() && y.is_nan()) || (x == y && x.is_sign_negative() == y.is_sign_negative()),
        _ => a == b,
    }
}

fn obj_is(_vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(Valor::Bool(mesmo_valor(&c.arg(0), &c.arg(1))))
}

fn tem_propria(vm: &mut Vm, o: &Valor, k: &Valor) -> R<bool> {
    let r = vm.para_objeto(o)?;
    let ch = vm.chave_de(k)?;
    if vm.heap.get(r).achar(ch).is_some() {
        return Ok(true);
    }
    Ok(match &vm.heap.get(r).classe {
        Classe::Array(a) | Classe::Argumentos(a) => nome_da_chave(ch).parse::<usize>().is_ok_and(|i| i < a.len()) || nome_da_chave(ch) == "length",
        Classe::Texto(s) => nome_da_chave(ch).parse::<usize>().is_ok_and(|i| i < s.len()),
        Classe::Dom(_) => false,
        _ => false,
    })
}

fn obj_has_own(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(Valor::Bool(tem_propria(vm, &c.arg(0), &c.arg(1))?))
}

fn obj_has_own_property(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(Valor::Bool(tem_propria(vm, c.this, &c.arg(0))?))
}

fn obj_group_by(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let itens = vm.para_vec(&c.arg(0))?;
    let f = c.arg(1);
    let o = vm.alocar(None, Classe::Comum);
    for (i, x) in itens.into_iter().enumerate() {
        let k = vm.chamar(&f, &Valor::Indef, &[x.clone(), Valor::Num(i as f64)])?;
        let ch = vm.chave_de(&k)?;
        let lista = vm.pegar(&Valor::Obj(o), ch)?;
        match lista {
            Valor::Obj(l) => arr_mut(vm, l).push(x),
            _ => {
                let a = vm.novo_array(alloc::vec![x]);
                vm.definir_propria(o, ch, a);
            }
        }
    }
    Ok(Valor::Obj(o))
}

fn obj_is_prototype_of(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let Valor::Obj(alvo) = c.this else { return Ok(Valor::Bool(false)) };
    let Valor::Obj(mut o) = c.arg(0) else { return Ok(Valor::Bool(false)) };
    while let Some(p) = vm.heap.get(o).proto {
        if p == *alvo {
            return Ok(Valor::Bool(true));
        }
        o = p;
    }
    Ok(Valor::Bool(false))
}

fn obj_property_is_enumerable(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let r = vm.para_objeto(c.this)?;
    let ch = vm.chave_de(&c.arg(0))?;
    Ok(Valor::Bool(vm.heap.get(r).slot(ch).is_some_and(|s| s.enumeravel) || matches!(vm.heap.get(r).classe, Classe::Array(_)) && nome_da_chave(ch).parse::<usize>().is_ok()))
}

fn obj_to_string(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let etiqueta = match c.this {
        Valor::Indef => "Undefined",
        Valor::Nulo => "Null",
        Valor::Num(_) => "Number",
        Valor::Texto(_) => "String",
        Valor::Bool(_) => "Boolean",
        Valor::Obj(r) => {
            let t = vm.pegar(c.this, SIM_ETIQUETA)?;
            if let Valor::Texto(s) = t {
                return Ok(Valor::texto(&format!("[object {s}]")));
            }
            match &vm.heap.get(*r).classe {
                Classe::Array(_) => "Array",
                Classe::Funcao { .. } | Classe::Nativa { .. } | Classe::Ligada { .. } => "Function",
                Classe::Erro => "Error",
                Classe::Data(_) => "Date",
                Classe::Regex(_) => "RegExp",
                Classe::Argumentos(_) => "Arguments",
                Classe::Booleano(_) => "Boolean",
                Classe::Numero(_) => "Number",
                Classe::Texto(_) => "String",
                _ => "Object",
            }
        }
        Valor::Simbolo(_) => "Symbol",
    };
    Ok(Valor::texto(&format!("[object {etiqueta}]")))
}

fn obj_value_of(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(Valor::Obj(vm.para_objeto(c.this)?))
}

// ================================================================ Function

fn instalar_funcao(vm: &mut Vm) {
    let pf = vm.p.funcao;
    let f = construtor(vm, "Function", fun_ctor, 1, pf);
    let _ = f;
    vm.metodo(pf, "call", fun_call, 1);
    vm.metodo(pf, "apply", fun_apply, 2);
    vm.metodo(pf, "bind", fun_bind, 1);
    vm.metodo(pf, "toString", fun_to_string, 0);
    let hi = vm.nativa("[Symbol.hasInstance]", fun_has_instance, 1);
    vm.heap.get_mut(pf).definir(SIM_TEM_INSTANCIA, Slot::oculto(hi));
}

fn fun_ctor(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    // new Function(a, b, corpo)
    let mut params = Vec::new();
    for i in 0..c.args.len().saturating_sub(1) {
        params.push(String::from(&*texto(vm, c, i)?));
    }
    let corpo = if c.args.is_empty() { Rc::from("") } else { texto(vm, c, c.args.len() - 1)? };
    let fonte = format!("(function anonymous({}\n) {{\n{}\n}})", params.join(","), corpo);
    super::avaliar(vm, &fonte)
}

fn fun_call(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let this = c.arg(0);
    let resto = if c.args.is_empty() { &[][..] } else { &c.args[1..] };
    vm.chamar(c.this, &this, resto)
}

fn fun_apply(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let this = c.arg(0);
    let a = c.arg(1);
    let args = if a.eh_nulo_ou_indef() { Vec::new() } else { itens_de(vm, &a)? };
    vm.chamar(c.this, &this, &args)
}

fn fun_bind(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    if !vm.eh_chamavel(c.this) {
        return vm.erro_tipo("bind em não-função");
    }
    let pf = vm.p.funcao;
    let args = if c.args.is_empty() { Vec::new() } else { c.args[1..].to_vec() };
    let nome = vm.pegar(c.this, chave("name"))?;
    let r = vm.alocar(Some(pf), Classe::Ligada { alvo: c.this.clone(), this: c.arg(0), args });
    let n = vm.para_texto(&nome)?;
    vm.heap.get_mut(r).definir(chave("name"), Slot::oculto(Valor::texto(&format!("bound {n}"))));
    Ok(Valor::Obj(r))
}

fn fun_to_string(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    if let Valor::Obj(r) = c.this {
        if let Classe::Funcao { proto, .. } = &vm.heap.get(*r).classe {
            if proto.fonte_fim > proto.fonte_ini {
                return Ok(Valor::texto(proto.texto_fonte()));
            }
            return Ok(Valor::texto(&format!("function {}() {{ [código] }}", proto.nome)));
        }
    }
    let n = vm.pegar(c.this, chave("name"))?;
    let n = vm.para_texto(&n)?;
    Ok(Valor::texto(&format!("function {n}() {{ [native code] }}")))
}

fn fun_has_instance(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(Valor::Bool(vm.instancia_de(&c.arg(0), c.this)?))
}

// ================================================================ Array

fn instalar_array(vm: &mut Vm) {
    let pa = vm.p.array;
    let a = construtor(vm, "Array", arr_ctor, 1, pa);
    vm.metodo(a, "isArray", arr_is_array, 1);
    vm.metodo(a, "from", arr_from, 1);
    vm.metodo(a, "of", arr_of, 0);
    let metodos: &[(&str, super::vm::FnNativa, u8)] = &[
        ("push", arr_push, 1),
        ("pop", arr_pop, 0),
        ("shift", arr_shift, 0),
        ("unshift", arr_unshift, 1),
        ("slice", arr_slice, 2),
        ("splice", arr_splice, 2),
        ("concat", arr_concat, 1),
        ("join", arr_join, 1),
        ("toString", arr_to_string, 0),
        ("toLocaleString", arr_to_string, 0),
        ("reverse", arr_reverse, 0),
        ("indexOf", arr_index_of, 1),
        ("lastIndexOf", arr_last_index_of, 1),
        ("includes", arr_includes, 1),
        ("find", arr_find, 1),
        ("findIndex", arr_find_index, 1),
        ("findLast", arr_find_last, 1),
        ("findLastIndex", arr_find_last_index, 1),
        ("filter", arr_filter, 1),
        ("map", arr_map, 1),
        ("forEach", arr_for_each, 1),
        ("some", arr_some, 1),
        ("every", arr_every, 1),
        ("reduce", arr_reduce, 1),
        ("reduceRight", arr_reduce_right, 1),
        ("sort", arr_sort, 1),
        ("toSorted", arr_to_sorted, 1),
        ("toReversed", arr_to_reversed, 0),
        ("with", arr_with, 2),
        ("fill", arr_fill, 1),
        ("flat", arr_flat, 0),
        ("flatMap", arr_flat_map, 1),
        ("at", arr_at, 1),
        ("keys", arr_keys, 0),
        ("entries", arr_entries, 0),
        ("copyWithin", arr_copy_within, 2),
    ];
    for &(n, f, k) in metodos {
        vm.metodo(pa, n, f, k);
    }
    let valores = vm.nativa("values", arr_values, 0);
    vm.heap.get_mut(pa).definir(chave("values"), Slot::oculto(valores.clone()));
    vm.heap.get_mut(pa).definir(SIM_ITERADOR, Slot::oculto(valores));
}

fn arr_ctor(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    if c.args.len() == 1 {
        if let Valor::Num(n) = c.args[0] {
            if n < 0.0 || n.fract() != 0.0 || n > 1e8 {
                return vm.erro_faixa(&format!("tamanho de array inválido: {n}"));
            }
            return Ok(vm.novo_array(alloc::vec![Valor::Indef; n as usize]));
        }
    }
    Ok(vm.novo_array(c.args.to_vec()))
}

fn arr_is_array(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(Valor::Bool(vm.eh_array(&c.arg(0))))
}

fn arr_from(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let fonte = c.arg(0);
    if fonte.eh_nulo_ou_indef() {
        return vm.erro_tipo("Array.from de undefined");
    }
    let iteravel = match &fonte {
        Valor::Texto(_) => true,
        Valor::Obj(_) => {
            let f = vm.pegar(&fonte, SIM_ITERADOR)?;
            vm.eh_chamavel(&f)
        }
        _ => false,
    };
    let itens = if iteravel { vm.para_vec(&fonte)? } else { itens_de(vm, &fonte)? };
    let f = c.arg(1);
    if vm.eh_chamavel(&f) {
        let this = c.arg(2);
        let mut out = Vec::with_capacity(itens.len());
        for (i, x) in itens.into_iter().enumerate() {
            out.push(vm.chamar(&f, &this, &[x, Valor::Num(i as f64)])?);
        }
        return Ok(vm.novo_array(out));
    }
    Ok(vm.novo_array(itens))
}

/// Metodos que mudam o array, chamados num array-like
/// (Array.prototype.push.call(obj, ...)): roda numa copia e grava de volta.
fn generico(vm: &mut Vm, c: &Ctx, f: super::vm::FnNativa) -> R<Valor> {
    if let Valor::Obj(r) = c.this {
        if matches!(vm.heap.get(*r).classe, Classe::Array(_)) {
            return f(vm, c);
        }
    } else {
        return vm.erro_tipo("this não é objeto");
    }
    let itens = itens_de(vm, c.this)?;
    let antes = itens.len();
    let tmp = vm.novo_array(itens);
    let t = tmp.obj().unwrap_or(0);
    let r = f(vm, &Ctx { this: &tmp, args: c.args, dado: c.dado, novo: None })?;
    let novos = arr(vm, t).clone();
    for (i, v) in novos.iter().enumerate() {
        vm.por_idx(c.this, &Valor::Num(i as f64), v.clone())?;
    }
    if let Valor::Obj(o) = c.this {
        for i in novos.len()..antes {
            vm.heap.get_mut(*o).remover(chave(&alloc::format!("{i}")));
        }
    }
    vm.por(c.this, chave("length"), Valor::Num(novos.len() as f64))?;
    Ok(if r == tmp { c.this.clone() } else { r })
}

fn eh_array_this(vm: &Vm, c: &Ctx) -> bool {
    matches!(c.this, Valor::Obj(r) if matches!(vm.heap.get(*r).classe, Classe::Array(_)))
}

fn tamanho_generico(vm: &mut Vm, v: &Valor) -> R<usize> {
    let len = vm.pegar(v, chave("length"))?;
    let n = vm.para_numero(&len)?;
    Ok(if n.is_finite() && n > 0.0 { n as usize } else { 0 })
}

/// push em array-like sem copiar tudo (dataLayer, jQuery...).
fn arr_push(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    if eh_array_this(vm, c) {
        return arr_push_(vm, c);
    }
    let mut n = tamanho_generico(vm, c.this)?;
    for a in c.args {
        vm.por_idx(c.this, &Valor::Num(n as f64), a.clone())?;
        n += 1;
    }
    vm.por(c.this, chave("length"), Valor::Num(n as f64))?;
    Ok(Valor::Num(n as f64))
}

fn arr_pop(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    if eh_array_this(vm, c) {
        return arr_pop_(vm, c);
    }
    let n = tamanho_generico(vm, c.this)?;
    if n == 0 {
        vm.por(c.this, chave("length"), Valor::Num(0.0))?;
        return Ok(Valor::Indef);
    }
    let k = Valor::Num((n - 1) as f64);
    let v = vm.pegar_idx(c.this, &k)?;
    if let Valor::Obj(o) = c.this {
        vm.heap.get_mut(*o).remover(chave(&alloc::format!("{}", n - 1)));
    }
    vm.por(c.this, chave("length"), Valor::Num((n - 1) as f64))?;
    Ok(v)
}

fn arr_shift(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    generico(vm, c, arr_shift_)
}

fn arr_unshift(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    generico(vm, c, arr_unshift_)
}

fn arr_splice(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    generico(vm, c, arr_splice_)
}

fn arr_reverse(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    generico(vm, c, arr_reverse_)
}

fn arr_sort(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    generico(vm, c, arr_sort_)
}

fn arr_fill(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    generico(vm, c, arr_fill_)
}

fn arr_copy_within(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    generico(vm, c, arr_copy_within_)
}

fn arr_of(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(vm.novo_array(c.args.to_vec()))
}

fn arr_push_(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let r = this_array(vm, c)?;
    let a = arr_mut(vm, r);
    a.extend_from_slice(c.args);
    Ok(Valor::Num(a.len() as f64))
}

fn arr_pop_(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let r = this_array(vm, c)?;
    Ok(arr_mut(vm, r).pop().unwrap_or_default())
}

fn arr_shift_(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let r = this_array(vm, c)?;
    let a = arr_mut(vm, r);
    Ok(if a.is_empty() { Valor::Indef } else { a.remove(0) })
}

fn arr_unshift_(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let r = this_array(vm, c)?;
    let a = arr_mut(vm, r);
    let resto: Vec<Valor> = a.drain(..).collect();
    a.extend_from_slice(c.args);
    a.extend(resto);
    Ok(Valor::Num(a.len() as f64))
}

fn arr_slice(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let itens = itens_de(vm, c.this)?;
    let n = itens.len();
    let ini = relativo(int_arg(vm, c, 0, 0.0)?, n);
    let fim = relativo(int_arg(vm, c, 1, n as f64)?, n);
    let v = if ini < fim { itens[ini..fim].to_vec() } else { Vec::new() };
    Ok(vm.novo_array(v))
}

fn arr_splice_(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let r = this_array(vm, c)?;
    let n = arr(vm, r).len();
    let ini = relativo(int_arg(vm, c, 0, 0.0)?, n);
    let quantos = if c.args.len() < 2 { n - ini } else { (int_arg(vm, c, 1, 0.0)?.max(0.0) as usize).min(n - ini) };
    let novos: Vec<Valor> = c.args.iter().skip(2).cloned().collect();
    let removidos: Vec<Valor> = arr_mut(vm, r).splice(ini..ini + quantos, novos).collect();
    Ok(vm.novo_array(removidos))
}

fn arr_concat(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let mut v = itens_de(vm, c.this)?;
    for a in c.args {
        if vm.eh_array(a) {
            v.extend(itens_de(vm, a)?);
        } else {
            v.push(a.clone());
        }
    }
    Ok(vm.novo_array(v))
}

fn juntar(vm: &mut Vm, itens: &[Valor], sep: &str) -> R<String> {
    let mut s = String::new();
    for (i, x) in itens.iter().enumerate() {
        if i > 0 {
            s.push_str(sep);
        }
        if !x.eh_nulo_ou_indef() {
            s.push_str(&vm.para_texto(x)?);
        }
    }
    Ok(s)
}

fn arr_join(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let itens = itens_de(vm, c.this)?;
    let sep = if matches!(c.arg(0), Valor::Indef) { Rc::from(",") } else { texto(vm, c, 0)? };
    Ok(Valor::texto(&juntar(vm, &itens, &sep)?))
}

fn arr_to_string(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let itens = itens_de(vm, c.this)?;
    Ok(Valor::texto(&juntar(vm, &itens, ",")?))
}

fn arr_reverse_(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let r = this_array(vm, c)?;
    arr_mut(vm, r).reverse();
    Ok(c.this.clone())
}

fn arr_index_of(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let itens = itens_de(vm, c.this)?;
    let alvo = c.arg(0);
    let ini = relativo(int_arg(vm, c, 1, 0.0)?, itens.len());
    Ok(Valor::Num(itens.iter().skip(ini).position(|x| *x == alvo && !matches!(x, Valor::Num(n) if n.is_nan())).map_or(-1.0, |p| (p + ini) as f64)))
}

fn arr_last_index_of(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let itens = itens_de(vm, c.this)?;
    let alvo = c.arg(0);
    Ok(Valor::Num(itens.iter().rposition(|x| *x == alvo).map_or(-1.0, |p| p as f64)))
}

fn arr_includes(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let itens = itens_de(vm, c.this)?;
    let alvo = c.arg(0);
    Ok(Valor::Bool(itens.iter().any(|x| *x == alvo || matches!((x, &alvo), (Valor::Num(a), Valor::Num(b)) if a.is_nan() && b.is_nan()))))
}

/// Percorre chamando f(x, i, arr); `parar` decide o fim.
fn percorrer(vm: &mut Vm, c: &Ctx, mut visita: impl FnMut(&mut Vm, usize, &Valor, &Valor) -> R<bool>) -> R<()> {
    let f = c.arg(0);
    if !vm.eh_chamavel(&f) {
        return vm.erro_tipo("callback não é função");
    }
    let this = c.arg(1);
    let mut i = 0;
    loop {
        // relê a cada volta (o callback pode mudar o array)
        let x = match c.this {
            Valor::Obj(r) => match &vm.heap.get(*r).classe {
                Classe::Array(a) => match a.get(i) {
                    Some(x) => x.clone(),
                    None => break,
                },
                _ => {
                    let itens = itens_de(vm, c.this)?;
                    match itens.get(i) {
                        Some(x) => x.clone(),
                        None => break,
                    }
                }
            },
            _ => break,
        };
        let res = vm.chamar(&f, &this, &[x.clone(), Valor::Num(i as f64), c.this.clone()])?;
        if visita(vm, i, &x, &res)? {
            break;
        }
        i += 1;
    }
    Ok(())
}

fn arr_find(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let mut achado = Valor::Indef;
    percorrer(vm, c, |_, _, x, r| {
        if r.verdadeiro() {
            achado = x.clone();
            return Ok(true);
        }
        Ok(false)
    })?;
    Ok(achado)
}

fn arr_find_index(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let mut achado = -1.0;
    percorrer(vm, c, |_, i, _, r| {
        if r.verdadeiro() {
            achado = i as f64;
            return Ok(true);
        }
        Ok(false)
    })?;
    Ok(Valor::Num(achado))
}

fn arr_find_last(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let itens = itens_de(vm, c.this)?;
    let f = c.arg(0);
    for (i, x) in itens.iter().enumerate().rev() {
        if vm.chamar(&f, &c.arg(1), &[x.clone(), Valor::Num(i as f64)])?.verdadeiro() {
            return Ok(x.clone());
        }
    }
    Ok(Valor::Indef)
}

fn arr_find_last_index(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let itens = itens_de(vm, c.this)?;
    let f = c.arg(0);
    for (i, x) in itens.iter().enumerate().rev() {
        if vm.chamar(&f, &c.arg(1), &[x.clone(), Valor::Num(i as f64)])?.verdadeiro() {
            return Ok(Valor::Num(i as f64));
        }
    }
    Ok(Valor::Num(-1.0))
}

fn arr_filter(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let mut out = Vec::new();
    percorrer(vm, c, |_, _, x, r| {
        if r.verdadeiro() {
            out.push(x.clone());
        }
        Ok(false)
    })?;
    Ok(vm.novo_array(out))
}

fn arr_map(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let mut out = Vec::new();
    percorrer(vm, c, |_, _, _, r| {
        out.push(r.clone());
        Ok(false)
    })?;
    Ok(vm.novo_array(out))
}

fn arr_for_each(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    percorrer(vm, c, |_, _, _, _| Ok(false))?;
    Ok(Valor::Indef)
}

fn arr_some(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let mut sim = false;
    percorrer(vm, c, |_, _, _, r| {
        sim = r.verdadeiro();
        Ok(sim)
    })?;
    Ok(Valor::Bool(sim))
}

fn arr_every(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let mut todos = true;
    percorrer(vm, c, |_, _, _, r| {
        todos = r.verdadeiro();
        Ok(!todos)
    })?;
    Ok(Valor::Bool(todos))
}

fn reduzir(vm: &mut Vm, c: &Ctx, direita: bool) -> R<Valor> {
    let itens = itens_de(vm, c.this)?;
    let f = c.arg(0);
    let ordem: Vec<usize> = if direita { (0..itens.len()).rev().collect() } else { (0..itens.len()).collect() };
    let mut it = ordem.into_iter();
    let mut acc = if c.args.len() >= 2 {
        c.arg(1)
    } else {
        match it.next() {
            Some(i) => itens[i].clone(),
            None => return vm.erro_tipo("reduce de array vazio sem valor inicial"),
        }
    };
    for i in it {
        acc = vm.chamar(&f, &Valor::Indef, &[acc, itens[i].clone(), Valor::Num(i as f64), c.this.clone()])?;
    }
    Ok(acc)
}

fn arr_reduce(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    reduzir(vm, c, false)
}

fn arr_reduce_right(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    reduzir(vm, c, true)
}

/// Ordenacao estavel (merge sort) com comparador JS ou por texto.
fn ordenar(vm: &mut Vm, itens: &mut Vec<Valor>, cmp: &Valor) -> R<()> {
    // indefinidos vao para o fim
    let mut indef = 0;
    itens.retain(|x| {
        if matches!(x, Valor::Indef) {
            indef += 1;
            false
        } else {
            true
        }
    });
    let n = itens.len();
    let mut chaves: Vec<Rc<str>> = Vec::new();
    let por_texto = !vm.eh_chamavel(cmp);
    if por_texto {
        for x in itens.iter() {
            chaves.push(vm.para_texto(x)?);
        }
    }
    let mut idx: Vec<usize> = (0..n).collect();
    let mut tmp = idx.clone();
    let mut largura = 1;
    while largura < n {
        let mut i = 0;
        while i < n {
            let meio = (i + largura).min(n);
            let fim = (i + 2 * largura).min(n);
            let (mut a, mut b, mut k) = (i, meio, i);
            while a < meio && b < fim {
                let menor_ou_igual = if por_texto {
                    chaves[idx[a]] <= chaves[idx[b]]
                } else {
                    let r = vm.chamar(cmp, &Valor::Indef, &[itens[idx[a]].clone(), itens[idx[b]].clone()])?;
                    vm.para_numero(&r)? <= 0.0
                };
                if menor_ou_igual {
                    tmp[k] = idx[a];
                    a += 1;
                } else {
                    tmp[k] = idx[b];
                    b += 1;
                }
                k += 1;
            }
            while a < meio {
                tmp[k] = idx[a];
                a += 1;
                k += 1;
            }
            while b < fim {
                tmp[k] = idx[b];
                b += 1;
                k += 1;
            }
            i += 2 * largura;
        }
        core::mem::swap(&mut idx, &mut tmp);
        largura *= 2;
    }
    let ordenados: Vec<Valor> = idx.iter().map(|&i| itens[i].clone()).collect();
    *itens = ordenados;
    itens.extend(core::iter::repeat_n(Valor::Indef, indef));
    Ok(())
}

fn arr_sort_(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let r = this_array(vm, c)?;
    let mut itens = arr(vm, r).clone();
    ordenar(vm, &mut itens, &c.arg(0))?;
    *arr_mut(vm, r) = itens;
    Ok(c.this.clone())
}

fn arr_to_sorted(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let mut itens = itens_de(vm, c.this)?;
    ordenar(vm, &mut itens, &c.arg(0))?;
    Ok(vm.novo_array(itens))
}

fn arr_to_reversed(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let mut itens = itens_de(vm, c.this)?;
    itens.reverse();
    Ok(vm.novo_array(itens))
}

fn arr_with(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let mut itens = itens_de(vm, c.this)?;
    let n = itens.len();
    let i = int_arg(vm, c, 0, 0.0)?;
    let i = if i < 0.0 { n as f64 + i } else { i };
    if i < 0.0 || i >= n as f64 {
        return vm.erro_faixa("índice fora do array");
    }
    itens[i as usize] = c.arg(1);
    Ok(vm.novo_array(itens))
}

fn arr_fill_(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let r = this_array(vm, c)?;
    let n = arr(vm, r).len();
    let ini = relativo(int_arg(vm, c, 1, 0.0)?, n);
    let fim = relativo(int_arg(vm, c, 2, n as f64)?, n);
    let v = c.arg(0);
    for x in arr_mut(vm, r).iter_mut().take(fim).skip(ini) {
        *x = v.clone();
    }
    Ok(c.this.clone())
}

fn achatar(vm: &mut Vm, itens: Vec<Valor>, prof: f64, out: &mut Vec<Valor>) -> R<()> {
    for x in itens {
        if prof >= 1.0 && vm.eh_array(&x) {
            let sub = itens_de(vm, &x)?;
            achatar(vm, sub, prof - 1.0, out)?;
        } else {
            out.push(x);
        }
    }
    Ok(())
}

fn arr_flat(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let itens = itens_de(vm, c.this)?;
    let prof = int_arg(vm, c, 0, 1.0)?;
    let mut out = Vec::new();
    achatar(vm, itens, prof, &mut out)?;
    Ok(vm.novo_array(out))
}

fn arr_flat_map(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let mapeado = arr_map(vm, c)?;
    let itens = itens_de(vm, &mapeado)?;
    let mut out = Vec::new();
    achatar(vm, itens, 1.0, &mut out)?;
    Ok(vm.novo_array(out))
}

fn arr_at(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let itens = itens_de(vm, c.this)?;
    let i = int_arg(vm, c, 0, 0.0)?;
    let i = if i < 0.0 { itens.len() as f64 + i } else { i };
    Ok(if i >= 0.0 { itens.get(i as usize).cloned().unwrap_or_default() } else { Valor::Indef })
}

fn arr_copy_within_(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let r = this_array(vm, c)?;
    let n = arr(vm, r).len();
    let alvo = relativo(int_arg(vm, c, 0, 0.0)?, n);
    let ini = relativo(int_arg(vm, c, 1, 0.0)?, n);
    let fim = relativo(int_arg(vm, c, 2, n as f64)?, n);
    let pedaco: Vec<Valor> = arr(vm, r)[ini..fim.max(ini)].to_vec();
    for (k, v) in pedaco.into_iter().enumerate() {
        if alvo + k < n {
            arr_mut(vm, r)[alvo + k] = v;
        }
    }
    Ok(c.this.clone())
}

fn iter_de(vm: &mut Vm, alvo: Valor, tipo: u8) -> Valor {
    let pi = vm.p.iterador;
    Valor::Obj(vm.alocar(Some(pi), Classe::Iterador(alvo, 0, tipo)))
}

fn arr_values(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    if let Valor::Texto(_) = c.this {
        return Ok(iter_de(vm, c.this.clone(), 3));
    }
    let o = Valor::Obj(vm.para_objeto(c.this)?);
    if !matches!(&o, Valor::Obj(r) if matches!(vm.heap.get(*r).classe, Classe::Array(_) | Classe::Argumentos(_))) {
        let itens = itens_de(vm, &o)?;
        let a = vm.novo_array(itens);
        return Ok(iter_de(vm, a, 0));
    }
    Ok(iter_de(vm, o, 0))
}

fn arr_keys(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(iter_de(vm, c.this.clone(), 1))
}

fn arr_entries(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(iter_de(vm, c.this.clone(), 2))
}

// ================================================================ String

fn instalar_texto(vm: &mut Vm) {
    let pt = vm.p.texto;
    let s = construtor(vm, "String", txt_ctor, 1, pt);
    vm.metodo(s, "fromCharCode", txt_from_char_code, 1);
    vm.metodo(s, "fromCodePoint", txt_from_code_point, 1);
    vm.metodo(s, "raw", txt_raw, 1);
    let metodos: &[(&str, super::vm::FnNativa, u8)] = &[
        ("charAt", txt_char_at, 1),
        ("charCodeAt", txt_char_code_at, 1),
        ("codePointAt", txt_code_point_at, 1),
        ("at", txt_at, 1),
        ("indexOf", txt_index_of, 1),
        ("lastIndexOf", txt_last_index_of, 1),
        ("includes", txt_includes, 1),
        ("startsWith", txt_starts_with, 1),
        ("endsWith", txt_ends_with, 1),
        ("slice", txt_slice, 2),
        ("substring", txt_substring, 2),
        ("substr", txt_substr, 2),
        ("toUpperCase", txt_upper, 0),
        ("toLowerCase", txt_lower, 0),
        ("toLocaleUpperCase", txt_upper, 0),
        ("toLocaleLowerCase", txt_lower, 0),
        ("trim", txt_trim, 0),
        ("trimStart", txt_trim_start, 0),
        ("trimEnd", txt_trim_end, 0),
        ("trimLeft", txt_trim_start, 0),
        ("trimRight", txt_trim_end, 0),
        ("padStart", txt_pad_start, 2),
        ("padEnd", txt_pad_end, 2),
        ("repeat", txt_repeat, 1),
        ("concat", txt_concat, 1),
        ("split", txt_split, 2),
        ("replace", txt_replace, 2),
        ("replaceAll", txt_replace_all, 2),
        ("match", txt_match, 1),
        ("matchAll", txt_match_all, 1),
        ("search", txt_search, 1),
        ("localeCompare", txt_locale_compare, 1),
        ("normalize", txt_to_string, 0),
        ("toString", txt_to_string, 0),
        ("valueOf", txt_to_string, 0),
        ("isWellFormed", txt_well_formed, 0),
    ];
    for &(n, f, k) in metodos {
        vm.metodo(pt, n, f, k);
    }
    let it = vm.nativa("[Symbol.iterator]", arr_values, 0);
    vm.heap.get_mut(pt).definir(SIM_ITERADOR, Slot::oculto(it));
}

fn this_texto(vm: &mut Vm, c: &Ctx) -> R<Rc<str>> {
    match c.this {
        Valor::Texto(s) => Ok(s.clone()),
        Valor::Obj(r) => match &vm.heap.get(*r).classe {
            Classe::Texto(s) => Ok(s.clone()),
            _ => vm.para_texto(c.this),
        },
        v if v.eh_nulo_ou_indef() => vm.erro_tipo("String.prototype em undefined"),
        v => vm.para_texto(v),
    }
}

fn txt_ctor(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = if c.args.is_empty() {
        Rc::from("")
    } else if let Valor::Simbolo(_) = c.arg(0) {
        Rc::from(vm.mostrar(&c.arg(0), 0).as_str())
    } else {
        texto(vm, c, 0)?
    };
    if c.novo.is_some() {
        let p = vm.p.texto;
        return Ok(Valor::Obj(vm.alocar(Some(p), Classe::Texto(s))));
    }
    Ok(Valor::Texto(s))
}

fn txt_from_char_code(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let mut u = Vec::with_capacity(c.args.len());
    for i in 0..c.args.len() {
        u.push(para_u32(num(vm, c, i)?) as u16);
    }
    Ok(de_utf16(&u))
}

fn txt_from_code_point(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let mut s = String::new();
    for i in 0..c.args.len() {
        let n = num(vm, c, i)?;
        match char::from_u32(n as u32) {
            Some(ch) if n.fract() == 0.0 => s.push(ch),
            _ => return vm.erro_faixa("ponto de código inválido"),
        }
    }
    Ok(Valor::texto(&s))
}

fn txt_raw(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let crus = vm.pegar(&c.arg(0), chave("raw"))?;
    let partes = itens_de(vm, &crus)?;
    let mut s = String::new();
    for (i, p) in partes.iter().enumerate() {
        s.push_str(&vm.para_texto(p)?);
        if i + 1 < partes.len() {
            if let Some(v) = c.args.get(i + 1) {
                s.push_str(&vm.para_texto(v)?);
            }
        }
    }
    Ok(Valor::texto(&s))
}

fn txt_char_at(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = this_texto(vm, c)?;
    let i = int_arg(vm, c, 0, 0.0)?;
    let u = utf16(&s);
    Ok(if i >= 0.0 && (i as usize) < u.len() { de_utf16(&u[i as usize..i as usize + 1]) } else { Valor::texto("") })
}

fn txt_char_code_at(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = this_texto(vm, c)?;
    let i = int_arg(vm, c, 0, 0.0)?;
    if s.is_ascii() {
        return Ok(Valor::Num(if i >= 0.0 { s.as_bytes().get(i as usize).map_or(f64::NAN, |&b| b as f64) } else { f64::NAN }));
    }
    let u = utf16(&s);
    Ok(Valor::Num(if i >= 0.0 { u.get(i as usize).map_or(f64::NAN, |&x| x as f64) } else { f64::NAN }))
}

fn txt_code_point_at(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = this_texto(vm, c)?;
    let i = int_arg(vm, c, 0, 0.0)?;
    let u = utf16(&s);
    if i < 0.0 || i as usize >= u.len() {
        return Ok(Valor::Indef);
    }
    let i = i as usize;
    let a = u[i] as u32;
    if (0xD800..0xDC00).contains(&a) {
        if let Some(&b) = u.get(i + 1) {
            if (0xDC00..0xE000).contains(&(b as u32)) {
                return Ok(Valor::Num((0x10000 + ((a - 0xD800) << 10) + (b as u32 - 0xDC00)) as f64));
            }
        }
    }
    Ok(Valor::Num(a as f64))
}

fn txt_at(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = this_texto(vm, c)?;
    let u = utf16(&s);
    let i = int_arg(vm, c, 0, 0.0)?;
    let i = if i < 0.0 { u.len() as f64 + i } else { i };
    Ok(if i >= 0.0 && (i as usize) < u.len() { de_utf16(&u[i as usize..i as usize + 1]) } else { Valor::Indef })
}

/// Indice UTF-16 de um indice de byte.
fn u16_de_byte(s: &str, b: usize) -> usize {
    if s.is_ascii() { b } else { s[..b].encode_utf16().count() }
}

fn byte_de_u16(s: &str, u: usize) -> usize {
    if s.is_ascii() {
        return u.min(s.len());
    }
    let mut n = 0;
    for (b, ch) in s.char_indices() {
        if n >= u {
            return b;
        }
        n += ch.len_utf16();
    }
    s.len()
}

fn txt_index_of(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = this_texto(vm, c)?;
    let alvo = texto(vm, c, 0)?;
    let ini = byte_de_u16(&s, int_arg(vm, c, 1, 0.0)?.max(0.0) as usize);
    Ok(Valor::Num(s[ini..].find(&*alvo).map_or(-1.0, |p| u16_de_byte(&s, ini + p) as f64)))
}

fn txt_last_index_of(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = this_texto(vm, c)?;
    let alvo = texto(vm, c, 0)?;
    Ok(Valor::Num(s.rfind(&*alvo).map_or(-1.0, |p| u16_de_byte(&s, p) as f64)))
}

fn txt_includes(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = this_texto(vm, c)?;
    if let Valor::Obj(r) = c.arg(0) {
        if let Classe::Regex(_) = vm.heap.get(r).classe {
            return vm.erro_tipo("includes não aceita RegExp");
        }
    }
    let alvo = texto(vm, c, 0)?;
    let ini = byte_de_u16(&s, int_arg(vm, c, 1, 0.0)?.max(0.0) as usize);
    Ok(Valor::Bool(s[ini..].contains(&*alvo)))
}

fn txt_starts_with(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = this_texto(vm, c)?;
    let alvo = texto(vm, c, 0)?;
    let ini = byte_de_u16(&s, int_arg(vm, c, 1, 0.0)?.max(0.0) as usize);
    Ok(Valor::Bool(s[ini..].starts_with(&*alvo)))
}

fn txt_ends_with(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = this_texto(vm, c)?;
    let alvo = texto(vm, c, 0)?;
    let fim = if matches!(c.arg(1), Valor::Indef) { s.len() } else { byte_de_u16(&s, int_arg(vm, c, 1, 0.0)?.max(0.0) as usize) };
    Ok(Valor::Bool(s[..fim].ends_with(&*alvo)))
}

fn txt_slice(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = this_texto(vm, c)?;
    if s.is_ascii() {
        let n = s.len();
        let ini = relativo(int_arg(vm, c, 0, 0.0)?, n);
        let fim = relativo(int_arg(vm, c, 1, n as f64)?, n);
        return Ok(Valor::texto(if ini < fim { &s[ini..fim] } else { "" }));
    }
    let u = utf16(&s);
    let n = u.len();
    let ini = relativo(int_arg(vm, c, 0, 0.0)?, n);
    let fim = relativo(int_arg(vm, c, 1, n as f64)?, n);
    Ok(if ini < fim { de_utf16(&u[ini..fim]) } else { Valor::texto("") })
}

fn txt_substring(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = this_texto(vm, c)?;
    let u = utf16(&s);
    let n = u.len() as f64;
    let a = int_arg(vm, c, 0, 0.0)?.clamp(0.0, n) as usize;
    let b = int_arg(vm, c, 1, n)?.clamp(0.0, n) as usize;
    let (a, b) = if a > b { (b, a) } else { (a, b) };
    Ok(de_utf16(&u[a..b]))
}

fn txt_substr(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = this_texto(vm, c)?;
    let u = utf16(&s);
    let n = u.len();
    let ini = relativo(int_arg(vm, c, 0, 0.0)?, n);
    let len = int_arg(vm, c, 1, (n - ini) as f64)?.max(0.0) as usize;
    let fim = (ini + len).min(n);
    Ok(de_utf16(&u[ini..fim]))
}

fn txt_upper(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = this_texto(vm, c)?;
    Ok(Valor::texto(&s.to_uppercase()))
}

fn txt_lower(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = this_texto(vm, c)?;
    Ok(Valor::texto(&s.to_lowercase()))
}

fn eh_espaco_js(ch: char) -> bool {
    ch.is_whitespace() || ch == '\u{FEFF}'
}

fn txt_trim(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = this_texto(vm, c)?;
    Ok(Valor::texto(s.trim_matches(eh_espaco_js)))
}

fn txt_trim_start(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = this_texto(vm, c)?;
    Ok(Valor::texto(s.trim_start_matches(eh_espaco_js)))
}

fn txt_trim_end(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = this_texto(vm, c)?;
    Ok(Valor::texto(s.trim_end_matches(eh_espaco_js)))
}

fn preencher(vm: &mut Vm, c: &Ctx, inicio: bool) -> R<Valor> {
    let s = this_texto(vm, c)?;
    let alvo = int_arg(vm, c, 0, 0.0)?.max(0.0) as usize;
    let enche: Rc<str> = if matches!(c.arg(1), Valor::Indef) { Rc::from(" ") } else { texto(vm, c, 1)? };
    let n = s.encode_utf16().count();
    if alvo <= n || enche.is_empty() {
        return Ok(Valor::Texto(s));
    }
    let falta = alvo - n;
    let mut p = String::new();
    while p.encode_utf16().count() < falta {
        p.push_str(&enche);
    }
    let pu = utf16(&p);
    let p = String::from_utf16_lossy(&pu[..falta]);
    Ok(Valor::texto(&if inicio { p + &s } else { String::from(&*s) + &p }))
}

fn txt_pad_start(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    preencher(vm, c, true)
}

fn txt_pad_end(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    preencher(vm, c, false)
}

fn txt_repeat(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = this_texto(vm, c)?;
    let n = int_arg(vm, c, 0, 0.0)?;
    if n < 0.0 || n.is_infinite() || n * s.len() as f64 > 1e9 {
        return vm.erro_faixa("repeat inválido");
    }
    Ok(Valor::texto(&s.repeat(n as usize)))
}

fn txt_concat(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let mut s = String::from(&*this_texto(vm, c)?);
    for i in 0..c.args.len() {
        s.push_str(&texto(vm, c, i)?);
    }
    Ok(Valor::texto(&s))
}

fn como_regex(vm: &Vm, v: &Valor) -> Option<Rc<Regex>> {
    match v {
        Valor::Obj(r) => match &vm.heap.get(*r).classe {
            Classe::Regex(x) => Some(x.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn txt_split(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = this_texto(vm, c)?;
    let limite = if matches!(c.arg(1), Valor::Indef) { usize::MAX } else { para_u32(num(vm, c, 1)?) as usize };
    let sep = c.arg(0);
    if matches!(sep, Valor::Indef) {
        return Ok(vm.novo_array(alloc::vec![Valor::Texto(s)]));
    }
    let partes: Vec<Valor> = if let Some(re) = como_regex(vm, &sep) {
        let t: Vec<char> = s.chars().collect();
        let mut v = Vec::new();
        let mut ultimo = 0;
        let mut pos = 0;
        while pos < t.len() {
            match re.procurar(&t, pos) {
                Some(caps) => {
                    let (a, b) = caps[0].unwrap_or((pos, pos));
                    if b == a && a >= t.len() {
                        break;
                    }
                    if b == a && a == ultimo {
                        pos = a + 1;
                        continue;
                    }
                    v.push(Valor::texto(&t[ultimo..a].iter().collect::<String>()));
                    for g in caps.iter().skip(1) {
                        v.push(match g {
                            Some((x, y)) => Valor::texto(&t[*x..*y].iter().collect::<String>()),
                            None => Valor::Indef,
                        });
                    }
                    ultimo = b;
                    pos = if b == a { b + 1 } else { b };
                }
                None => break,
            }
        }
        v.push(Valor::texto(&t[ultimo.min(t.len())..].iter().collect::<String>()));
        v
    } else {
        let sep = vm.para_texto(&sep)?;
        if sep.is_empty() {
            s.chars()
                .map(|ch| {
                    let mut b = [0u8; 4];
                    Valor::texto(ch.encode_utf8(&mut b))
                })
                .collect()
        } else {
            s.split(&*sep).map(Valor::texto).collect()
        }
    };
    let partes: Vec<Valor> = partes.into_iter().take(limite).collect();
    Ok(vm.novo_array(partes))
}

/// Expande $1, $&, $<nome> na substituicao.
fn expandir_substituicao(modelo: &str, t: &[char], caps: &[Option<(usize, usize)>], nomes: &[(Rc<str>, usize)]) -> String {
    let pedaco = |g: usize| -> String {
        match caps.get(g).copied().flatten() {
            Some((a, b)) => t[a..b].iter().collect(),
            None => String::new(),
        }
    };
    let mut s = String::new();
    let b: Vec<char> = modelo.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if b[i] == '$' && i + 1 < b.len() {
            match b[i + 1] {
                '$' => {
                    s.push('$');
                    i += 2;
                    continue;
                }
                '&' => {
                    s.push_str(&pedaco(0));
                    i += 2;
                    continue;
                }
                '`' => {
                    let a = caps[0].map_or(0, |x| x.0);
                    s.extend(&t[..a]);
                    i += 2;
                    continue;
                }
                '\'' => {
                    let f = caps[0].map_or(0, |x| x.1);
                    s.extend(&t[f..]);
                    i += 2;
                    continue;
                }
                '<' => {
                    if let Some(fim) = b[i..].iter().position(|&c| c == '>') {
                        let nome: String = b[i + 2..i + fim].iter().collect();
                        if let Some((_, g)) = nomes.iter().find(|(n, _)| **n == *nome) {
                            s.push_str(&pedaco(*g));
                        }
                        i += fim + 1;
                        continue;
                    }
                }
                d if d.is_ascii_digit() => {
                    let mut n = d.to_digit(10).unwrap_or(0) as usize;
                    let mut k = 2;
                    if let Some(d2) = b.get(i + 2).and_then(|c| c.to_digit(10)) {
                        let n2 = n * 10 + d2 as usize;
                        if n2 < caps.len() {
                            n = n2;
                            k = 3;
                        }
                    }
                    if n > 0 && n < caps.len() {
                        s.push_str(&pedaco(n));
                        i += k;
                        continue;
                    }
                }
                _ => {}
            }
        }
        s.push(b[i]);
        i += 1;
    }
    s
}

fn substituir(vm: &mut Vm, c: &Ctx, todos: bool) -> R<Valor> {
    let s = this_texto(vm, c)?;
    let padrao = c.arg(0);
    let subst = c.arg(1);
    let funcao = vm.eh_chamavel(&subst);
    let modelo: Rc<str> = if funcao { Rc::from("") } else { vm.para_texto(&subst)? };
    let t: Vec<char> = s.chars().collect();
    let mut casamentos: Vec<Vec<Option<(usize, usize)>>> = Vec::new();
    let mut nomes: Vec<(Rc<str>, usize)> = Vec::new();
    if let Some(re) = como_regex(vm, &padrao) {
        nomes = re.nomes.clone();
        let global = re.global || todos;
        let mut pos = 0;
        while pos <= t.len() {
            let Some(caps) = re.procurar(&t, pos) else { break };
            let (a, b) = caps[0].unwrap_or((pos, pos));
            casamentos.push(caps);
            if !global {
                break;
            }
            pos = if b == a { b + 1 } else { b };
        }
        if global {
            if let Valor::Obj(r) = padrao {
                vm.heap.get_mut(r).definir(chave("lastIndex"), Slot::oculto(Valor::Num(0.0)));
            }
        }
    } else {
        let p = vm.para_texto(&padrao)?;
        let pc: Vec<char> = p.chars().collect();
        let mut pos = 0;
        while pos <= t.len() {
            let achou = if pc.is_empty() { Some(pos) } else { (pos..=t.len().saturating_sub(pc.len())).find(|&i| t[i..].starts_with(&pc)) };
            let Some(a) = achou else { break };
            casamentos.push(alloc::vec![Some((a, a + pc.len()))]);
            if !todos {
                break;
            }
            pos = if pc.is_empty() { a + 1 } else { a + pc.len() };
        }
    }
    let mut out = String::with_capacity(s.len());
    let mut ultimo = 0;
    for caps in &casamentos {
        let (a, b) = caps[0].unwrap_or((0, 0));
        if a < ultimo {
            continue;
        }
        out.extend(&t[ultimo..a]);
        if funcao {
            let mut args: Vec<Valor> = caps
                .iter()
                .map(|g| match g {
                    Some((x, y)) => Valor::texto(&t[*x..*y].iter().collect::<String>()),
                    None => Valor::Indef,
                })
                .collect();
            args.push(Valor::Num(a as f64));
            args.push(Valor::Texto(s.clone()));
            let r = vm.chamar(&subst, &Valor::Indef, &args)?;
            out.push_str(&vm.para_texto(&r)?);
        } else {
            out.push_str(&expandir_substituicao(&modelo, &t, caps, &nomes));
        }
        ultimo = b;
    }
    out.extend(&t[ultimo.min(t.len())..]);
    Ok(Valor::texto(&out))
}

fn txt_replace(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    substituir(vm, c, false)
}

fn txt_replace_all(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    substituir(vm, c, true)
}

/// Monta o array de resultado do exec (com index, input, groups).
fn resultado_exec(vm: &mut Vm, re: &Regex, t: &[char], s: &Rc<str>, caps: &[Option<(usize, usize)>]) -> Valor {
    let itens: Vec<Valor> = caps
        .iter()
        .map(|g| match g {
            Some((a, b)) => Valor::texto(&t[*a..*b].iter().collect::<String>()),
            None => Valor::Indef,
        })
        .collect();
    let a = vm.novo_array(itens);
    let r = a.obj().unwrap_or(0);
    let idx: usize = t[..caps[0].map_or(0, |x| x.0)].iter().map(|c| c.len_utf16()).sum();
    vm.heap.get_mut(r).definir(chave("index"), Slot::valor(Valor::Num(idx as f64)));
    vm.heap.get_mut(r).definir(chave("input"), Slot::valor(Valor::Texto(s.clone())));
    let grupos = if re.nomes.is_empty() {
        Valor::Indef
    } else {
        let o = vm.alocar(None, Classe::Comum);
        for (n, g) in &re.nomes {
            let v = match caps.get(*g).copied().flatten() {
                Some((x, y)) => Valor::texto(&t[x..y].iter().collect::<String>()),
                None => Valor::Indef,
            };
            vm.heap.get_mut(o).definir(chave(n), Slot::valor(v));
        }
        Valor::Obj(o)
    };
    vm.heap.get_mut(r).definir(chave("groups"), Slot::valor(grupos));
    a
}

fn txt_match(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = this_texto(vm, c)?;
    let p = c.arg(0);
    let re = match como_regex(vm, &p) {
        Some(r) => r,
        None => {
            let fonte = if matches!(p, Valor::Indef) { Rc::from("") } else { vm.para_texto(&p)? };
            Rc::new(Regex::nova(&fonte, "").map_err(|e| vm.novo_erro(vm.p.erro_sintaxe, e))?)
        }
    };
    let t: Vec<char> = s.chars().collect();
    if !re.global {
        return Ok(match re.procurar(&t, 0) {
            Some(caps) => resultado_exec(vm, &re, &t, &s, &caps),
            None => Valor::Nulo,
        });
    }
    let mut v = Vec::new();
    let mut pos = 0;
    while pos <= t.len() {
        let Some(caps) = re.procurar(&t, pos) else { break };
        let (a, b) = caps[0].unwrap_or((pos, pos));
        v.push(Valor::texto(&t[a..b].iter().collect::<String>()));
        pos = if b == a { b + 1 } else { b };
    }
    Ok(if v.is_empty() { Valor::Nulo } else { vm.novo_array(v) })
}

fn txt_match_all(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = this_texto(vm, c)?;
    let p = c.arg(0);
    let re = match como_regex(vm, &p) {
        Some(r) => r,
        None => {
            let fonte = vm.para_texto(&p)?;
            Rc::new(Regex::nova(&fonte, "g").map_err(|e| vm.novo_erro(vm.p.erro_sintaxe, e))?)
        }
    };
    let t: Vec<char> = s.chars().collect();
    let mut v = Vec::new();
    let mut pos = 0;
    while pos <= t.len() {
        let Some(caps) = re.procurar(&t, pos) else { break };
        let (a, b) = caps[0].unwrap_or((pos, pos));
        v.push(resultado_exec(vm, &re, &t, &s, &caps));
        pos = if b == a { b + 1 } else { b };
    }
    let a = vm.novo_array(v);
    Ok(iter_de(vm, a, 0))
}

fn txt_search(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = this_texto(vm, c)?;
    let p = c.arg(0);
    let re = match como_regex(vm, &p) {
        Some(r) => r,
        None => {
            let fonte = vm.para_texto(&p)?;
            Rc::new(Regex::nova(&fonte, "").map_err(|e| vm.novo_erro(vm.p.erro_sintaxe, e))?)
        }
    };
    let t: Vec<char> = s.chars().collect();
    Ok(Valor::Num(match re.procurar(&t, 0) {
        Some(caps) => t[..caps[0].map_or(0, |x| x.0)].iter().map(|c| c.len_utf16()).sum::<usize>() as f64,
        None => -1.0,
    }))
}

fn txt_locale_compare(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let a = this_texto(vm, c)?;
    let b = texto(vm, c, 0)?;
    // comparacao sem diferenciar maiusculas primeiro (aproxima o Intl)
    let o = a.to_lowercase().cmp(&b.to_lowercase()).then(a.cmp(&b));
    Ok(Valor::Num(match o {
        core::cmp::Ordering::Less => -1.0,
        core::cmp::Ordering::Equal => 0.0,
        core::cmp::Ordering::Greater => 1.0,
    }))
}

fn txt_to_string(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(Valor::Texto(this_texto(vm, c)?))
}

fn txt_well_formed(_vm: &mut Vm, _c: &Ctx) -> R<Valor> {
    Ok(Valor::Bool(true))
}

// ================================================================ Number

fn instalar_numero(vm: &mut Vm) {
    let pn = vm.p.numero;
    let n = construtor(vm, "Number", num_ctor, 1, pn);
    vm.metodo(pn, "toString", num_to_string, 1);
    vm.metodo(pn, "toFixed", num_to_fixed, 1);
    vm.metodo(pn, "toPrecision", num_to_precision, 1);
    vm.metodo(pn, "toExponential", num_to_exponential, 1);
    vm.metodo(pn, "toLocaleString", num_to_locale, 0);
    vm.metodo(pn, "valueOf", num_value_of, 0);
    vm.metodo(n, "isInteger", num_is_integer, 1);
    vm.metodo(n, "isSafeInteger", num_is_integer, 1);
    vm.metodo(n, "isFinite", num_is_finite, 1);
    vm.metodo(n, "isNaN", num_is_nan, 1);
    let pf = vm.nativa("parseFloat", glob_parse_float, 1);
    let pi = vm.nativa("parseInt", glob_parse_int, 2);
    vm.definir(n, "parseFloat", pf);
    vm.definir(n, "parseInt", pi);
    for (k, v) in [
        ("MAX_SAFE_INTEGER", 9007199254740991.0),
        ("MIN_SAFE_INTEGER", -9007199254740991.0),
        ("EPSILON", f64::EPSILON),
        ("MAX_VALUE", f64::MAX),
        ("MIN_VALUE", 5e-324),
        ("POSITIVE_INFINITY", f64::INFINITY),
        ("NEGATIVE_INFINITY", f64::NEG_INFINITY),
        ("NaN", f64::NAN),
    ] {
        vm.definir(n, k, Valor::Num(v));
    }
    let pb = vm.p.booleano;
    construtor(vm, "Boolean", bool_ctor, 1, pb);
    vm.metodo(pb, "toString", bool_to_string, 0);
    vm.metodo(pb, "valueOf", bool_value_of, 0);
}

fn this_num(vm: &mut Vm, c: &Ctx) -> R<f64> {
    match c.this {
        Valor::Num(n) => Ok(*n),
        Valor::Obj(r) => match vm.heap.get(*r).classe {
            Classe::Numero(n) => Ok(n),
            _ => vm.erro_tipo("this não é número"),
        },
        _ => vm.erro_tipo("this não é número"),
    }
}

fn num_ctor(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let n = if c.args.is_empty() { 0.0 } else { num(vm, c, 0)? };
    if c.novo.is_some() {
        let p = vm.p.numero;
        return Ok(Valor::Obj(vm.alocar(Some(p), Classe::Numero(n))));
    }
    Ok(Valor::Num(n))
}

fn num_to_string(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let n = this_num(vm, c)?;
    let base = int_arg(vm, c, 0, 10.0)? as u32;
    if base == 10 || !n.is_finite() {
        return Ok(Valor::texto(&numero_texto(n)));
    }
    if !(2..=36).contains(&base) {
        return vm.erro_faixa("base inválida");
    }
    let neg = n < 0.0;
    let mut i = m::trunc(n.abs());
    let mut frac = n.abs() - i;
    let mut digs = Vec::new();
    if i == 0.0 {
        digs.push(b'0');
    }
    while i >= 1.0 {
        let d = (i % base as f64) as u32;
        digs.push(core::char::from_digit(d, base).unwrap_or('0') as u8);
        i = m::trunc(i / base as f64);
    }
    digs.reverse();
    let mut s = String::from_utf8(digs).unwrap_or_default();
    if frac > 0.0 {
        s.push('.');
        for _ in 0..20 {
            frac *= base as f64;
            let d = frac as u32;
            s.push(core::char::from_digit(d, base).unwrap_or('0'));
            frac -= d as f64;
            if frac <= 0.0 {
                break;
            }
        }
    }
    Ok(Valor::texto(&if neg { format!("-{s}") } else { s }))
}

fn num_to_fixed(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let n = this_num(vm, c)?;
    let d = int_arg(vm, c, 0, 0.0)?;
    if !(0.0..=100.0).contains(&d) {
        return vm.erro_faixa("toFixed: dígitos inválidos");
    }
    if !n.is_finite() || n.abs() >= 1e21 {
        return Ok(Valor::texto(&numero_texto(n)));
    }
    Ok(Valor::texto(&format!("{:.*}", d as usize, n)))
}

fn num_to_precision(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let n = this_num(vm, c)?;
    if matches!(c.arg(0), Valor::Indef) || !n.is_finite() {
        return Ok(Valor::texto(&numero_texto(n)));
    }
    let p = int_arg(vm, c, 0, 1.0)?.clamp(1.0, 100.0) as usize;
    if n == 0.0 {
        return Ok(Valor::texto(&format!("{:.*}", p - 1, 0.0)));
    }
    let e = m::floor(m::ln(n.abs()) / core::f64::consts::LN_10) as i32;
    if e < -6 || e >= p as i32 {
        return Ok(Valor::texto(&format!("{:.*e}", p - 1, n).replace("e", "e+").replace("e+-", "e-")));
    }
    Ok(Valor::texto(&format!("{:.*}", (p as i32 - 1 - e).max(0) as usize, n)))
}

fn num_to_exponential(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let n = this_num(vm, c)?;
    let d = int_arg(vm, c, 0, 6.0)? as usize;
    let s = format!("{:.*e}", d, n);
    let s = if s.contains("e-") { s } else { s.replace('e', "e+") };
    Ok(Valor::texto(&s))
}

/// Formato en-US: 1,234,567.89 (ate 3 casas)
pub fn formatar_local(n: f64) -> String {
    if !n.is_finite() {
        return numero_texto(n);
    }
    let neg = n < 0.0;
    let s = format!("{:.3}", n.abs());
    let (int, frac) = s.split_once('.').unwrap_or((&s, ""));
    let frac = frac.trim_end_matches('0');
    let mut out = String::new();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if !frac.is_empty() {
        out.push('.');
        out.push_str(frac);
    }
    if neg { format!("-{out}") } else { out }
}

fn num_to_locale(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let n = this_num(vm, c)?;
    Ok(Valor::texto(&formatar_local(n)))
}

fn num_value_of(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(Valor::Num(this_num(vm, c)?))
}

fn num_is_integer(_vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(Valor::Bool(matches!(c.arg(0), Valor::Num(n) if n.is_finite() && n.fract() == 0.0)))
}

fn num_is_finite(_vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(Valor::Bool(matches!(c.arg(0), Valor::Num(n) if n.is_finite())))
}

fn num_is_nan(_vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(Valor::Bool(matches!(c.arg(0), Valor::Num(n) if n.is_nan())))
}

fn bool_ctor(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let b = c.arg(0).verdadeiro();
    if c.novo.is_some() {
        let p = vm.p.booleano;
        return Ok(Valor::Obj(vm.alocar(Some(p), Classe::Booleano(b))));
    }
    Ok(Valor::Bool(b))
}

fn this_bool(vm: &mut Vm, c: &Ctx) -> R<bool> {
    match c.this {
        Valor::Bool(b) => Ok(*b),
        Valor::Obj(r) => match vm.heap.get(*r).classe {
            Classe::Booleano(b) => Ok(b),
            _ => vm.erro_tipo("this não é booleano"),
        },
        _ => vm.erro_tipo("this não é booleano"),
    }
}

fn bool_to_string(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(Valor::texto(if this_bool(vm, c)? { "true" } else { "false" }))
}

fn bool_value_of(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(Valor::Bool(this_bool(vm, c)?))
}

// ================================================================ Symbol

fn instalar_simbolo(vm: &mut Vm) {
    let ps = vm.p.simbolo;
    let s = construtor(vm, "Symbol", sim_ctor, 0, ps);
    for (n, id) in [
        ("iterator", SIM_ITERADOR),
        ("asyncIterator", SIM_ITERADOR_ASSINC),
        ("hasInstance", SIM_TEM_INSTANCIA),
        ("toPrimitive", SIM_PARA_PRIMITIVO),
        ("toStringTag", SIM_ETIQUETA),
    ] {
        vm.definir(s, n, Valor::Simbolo(id));
        vm.descricoes_simbolos.push((id, Rc::from(format!("Symbol.{n}").as_str())));
    }
    for n in ["species", "isConcatSpreadable", "unscopables", "match", "replace", "search", "split", "matchAll"] {
        let id = vm.novo_simbolo(&format!("Symbol.{n}"));
        vm.definir(s, n, Valor::Simbolo(id));
    }
    vm.metodo(s, "for", sim_for, 1);
    vm.metodo(ps, "toString", sim_to_string, 0);
    getter(vm, ps, "description", sim_description);
}

fn sim_ctor(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    if c.novo.is_some() {
        return vm.erro_tipo("Symbol não é construtor");
    }
    let d = if matches!(c.arg(0), Valor::Indef) { Rc::from("") } else { texto(vm, c, 0)? };
    Ok(Valor::Simbolo(vm.novo_simbolo(&d)))
}

fn sim_for(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let d = texto(vm, c, 0)?;
    let marca: Rc<str> = Rc::from(format!("\u{0}for:{d}").as_str());
    if let Some((id, _)) = vm.descricoes_simbolos.iter().find(|(_, x)| *x == marca) {
        return Ok(Valor::Simbolo(*id));
    }
    let id = vm.novo_simbolo(&marca);
    Ok(Valor::Simbolo(id))
}

fn sim_desc(vm: &Vm, v: &Valor) -> String {
    let id = match v {
        Valor::Simbolo(i) => *i,
        Valor::Obj(r) => match vm.heap.get(*r).classe {
            Classe::Simbolo(i) => i,
            _ => 0,
        },
        _ => 0,
    };
    let d = vm.descricoes_simbolos.iter().find(|(i, _)| *i == id).map(|x| x.1.clone()).unwrap_or_else(|| Rc::from(""));
    String::from(d.trim_start_matches("\u{0}for:"))
}

fn sim_to_string(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(Valor::texto(&format!("Symbol({})", sim_desc(vm, c.this))))
}

fn sim_description(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(Valor::texto(&sim_desc(vm, c.this)))
}

// ================================================================ Erros

fn instalar_erros(vm: &mut Vm) {
    let pe = vm.p.erro;
    let e = construtor(vm, "Error", erro_ctor, 1, pe);
    vm.definir(pe, "name", Valor::texto("Error"));
    vm.definir(pe, "message", Valor::texto(""));
    vm.metodo(pe, "toString", erro_to_string, 0);
    vm.metodo(e, "captureStackTrace", nada, 1);
    let tipos = [
        ("TypeError", vm.p.erro_tipo),
        ("ReferenceError", vm.p.erro_ref),
        ("RangeError", vm.p.erro_faixa),
        ("SyntaxError", vm.p.erro_sintaxe),
    ];
    for (nome, p) in tipos {
        let f = construtor(vm, nome, erro_ctor, 1, p);
        vm.heap.get_mut(f).proto = Some(e);
        vm.definir(p, "name", Valor::texto(nome));
    }
    for nome in ["EvalError", "URIError", "AggregateError"] {
        let p = vm.alocar(Some(pe), Classe::Comum);
        let f = construtor(vm, nome, erro_ctor, 1, p);
        vm.heap.get_mut(f).proto = Some(e);
        vm.definir(p, "name", Valor::texto(nome));
    }
}

fn nada(_vm: &mut Vm, _c: &Ctx) -> R<Valor> {
    Ok(Valor::Indef)
}

fn erro_ctor(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    // o prototipo vem do construtor chamado (new TypeError) ou do this
    let proto = match c.novo {
        Some(alvo) => vm.pegar(alvo, chave("prototype"))?.obj(),
        None => {
            // chamado sem new: Error("x")
            let f = vm.quadros.last().map(|q| q.funcao.clone());
            let _ = f;
            Some(vm.p.erro)
        }
    };
    let r = vm.alocar(proto, Classe::Erro);
    if !matches!(c.arg(0), Valor::Indef) {
        let m = texto(vm, c, 0)?;
        vm.heap.get_mut(r).definir(chave("message"), Slot::oculto(Valor::Texto(m)));
    }
    let opcoes = c.arg(1);
    if let Valor::Obj(_) = opcoes {
        let causa = vm.pegar(&opcoes, chave("cause"))?;
        if !matches!(causa, Valor::Indef) {
            vm.heap.get_mut(r).definir(chave("cause"), Slot::oculto(causa));
        }
    }
    vm.heap.get_mut(r).definir(chave("stack"), Slot::oculto(Valor::texto("")));
    Ok(Valor::Obj(r))
}

fn erro_to_string(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let n = vm.pegar(c.this, chave("name"))?;
    let m = vm.pegar(c.this, chave("message"))?;
    let n = if matches!(n, Valor::Indef) { Rc::from("Error") } else { vm.para_texto(&n)? };
    let m = if matches!(m, Valor::Indef) { Rc::from("") } else { vm.para_texto(&m)? };
    Ok(Valor::texto(&if m.is_empty() { String::from(&*n) } else if n.is_empty() { String::from(&*m) } else { format!("{n}: {m}") }))
}

// ================================================================ Math

fn instalar_math(vm: &mut Vm) {
    let po = vm.p.objeto;
    let mo = obj_com(vm, po);
    let g = vm.global;
    vm.definir(g, "Math", Valor::Obj(mo));
    vm.heap.get_mut(mo).definir(SIM_ETIQUETA, Slot::oculto(Valor::texto("Math")));
    for (k, v) in [
        ("PI", core::f64::consts::PI),
        ("E", core::f64::consts::E),
        ("LN2", core::f64::consts::LN_2),
        ("LN10", core::f64::consts::LN_10),
        ("LOG2E", core::f64::consts::LOG2_E),
        ("LOG10E", core::f64::consts::LOG10_E),
        ("SQRT2", core::f64::consts::SQRT_2),
        ("SQRT1_2", core::f64::consts::FRAC_1_SQRT_2),
    ] {
        vm.definir(mo, k, Valor::Num(v));
    }
    let f1: &[(&str, fn(f64) -> f64)] = &[
        ("abs", |x| x.abs()),
        ("floor", m::floor),
        ("ceil", m::ceil),
        ("round", m::round),
        ("trunc", m::trunc),
        ("sign", |x| if x.is_nan() { x } else if x > 0.0 { 1.0 } else if x < 0.0 { -1.0 } else { x }),
        ("sqrt", m::sqrt),
        ("cbrt", m::cbrt),
        ("exp", m::exp),
        ("expm1", |x| m::exp(x) - 1.0),
        ("log", m::ln),
        ("log2", |x| m::ln(x) / core::f64::consts::LN_2),
        ("log10", |x| m::ln(x) / core::f64::consts::LN_10),
        ("log1p", |x| m::ln(1.0 + x)),
        ("sin", m::sin),
        ("cos", m::cos),
        ("tan", m::tan),
        ("asin", m::asin),
        ("acos", m::acos),
        ("atan", m::atan),
        ("sinh", |x| (m::exp(x) - m::exp(-x)) / 2.0),
        ("cosh", |x| (m::exp(x) + m::exp(-x)) / 2.0),
        ("tanh", |x| {
            let a = m::exp(2.0 * x);
            if a.is_infinite() { 1.0 } else { (a - 1.0) / (a + 1.0) }
        }),
        ("fround", |x| x as f32 as f64),
        ("clz32", |x| para_u32(x).leading_zeros() as f64),
    ];
    for &(n, f) in f1 {
        // cada funcao de 1 argumento vira uma nativa com o ponteiro no dado
        let id = vm.registrar(math1);
        let fv = vm.nativa_com(id, n, 1, Valor::Num(f as usize as f64));
        vm.definir(mo, n, fv);
    }
    vm.metodo(mo, "max", math_max, 2);
    vm.metodo(mo, "min", math_min, 2);
    vm.metodo(mo, "pow", math_pow, 2);
    vm.metodo(mo, "atan2", math_atan2, 2);
    vm.metodo(mo, "hypot", math_hypot, 2);
    vm.metodo(mo, "random", math_random, 0);
    vm.metodo(mo, "imul", math_imul, 2);
}

fn math1(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let x = num(vm, c, 0)?;
    let Valor::Num(p) = c.dado else { return Ok(Valor::Num(f64::NAN)) };
    // o ponteiro da funcao foi guardado como numero
    let f: fn(f64) -> f64 = unsafe { core::mem::transmute::<usize, fn(f64) -> f64>(*p as usize) };
    Ok(Valor::Num(f(x)))
}

fn math_max(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let mut r = f64::NEG_INFINITY;
    for i in 0..c.args.len() {
        let x = num(vm, c, i)?;
        if x.is_nan() {
            return Ok(Valor::Num(f64::NAN));
        }
        if x > r || (x == 0.0 && r == 0.0 && !x.is_sign_negative()) {
            r = x;
        }
    }
    Ok(Valor::Num(r))
}

fn math_min(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let mut r = f64::INFINITY;
    for i in 0..c.args.len() {
        let x = num(vm, c, i)?;
        if x.is_nan() {
            return Ok(Valor::Num(f64::NAN));
        }
        if x < r || (x == 0.0 && r == 0.0 && x.is_sign_negative()) {
            r = x;
        }
    }
    Ok(Valor::Num(r))
}

fn math_pow(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let a = num(vm, c, 0)?;
    let b = num(vm, c, 1)?;
    Ok(Valor::Num(super::vm::potencia(a, b)))
}

fn math_atan2(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let a = num(vm, c, 0)?;
    let b = num(vm, c, 1)?;
    Ok(Valor::Num(m::atan2(a, b)))
}

fn math_hypot(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let mut s = 0.0;
    for i in 0..c.args.len() {
        let x = num(vm, c, i)?;
        s += x * x;
    }
    Ok(Valor::Num(m::sqrt(s)))
}

static mut SEMENTE: u64 = 0x2545_F491_4F6C_DD1D;

fn math_random(vm: &mut Vm, _c: &Ctx) -> R<Valor> {
    // xorshift64*, semeado pelo relogio na primeira vez
    let mut x = unsafe { SEMENTE };
    if x == 0x2545_F491_4F6C_DD1D {
        x ^= (agora(vm) * 1000.0) as u64;
    }
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    unsafe { SEMENTE = x };
    let r = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
    Ok(Valor::Num((r >> 11) as f64 / (1u64 << 53) as f64))
}

fn math_imul(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let a = para_i32(num(vm, c, 0)?);
    let b = para_i32(num(vm, c, 1)?);
    Ok(Valor::Num(a.wrapping_mul(b) as f64))
}

// ================================================================ JSON

fn instalar_json(vm: &mut Vm) {
    let po = vm.p.objeto;
    let j = obj_com(vm, po);
    let g = vm.global;
    vm.definir(g, "JSON", Valor::Obj(j));
    vm.metodo(j, "parse", json_parse, 2);
    vm.metodo(j, "stringify", json_stringify, 3);
}

struct Json<'a> {
    s: &'a [u8],
    i: usize,
}

impl Json<'_> {
    fn espacos(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn valor(&mut self, vm: &mut Vm, prof: u32) -> Result<Valor, &'static str> {
        if prof > 512 {
            return Err("JSON fundo demais");
        }
        self.espacos();
        let c = *self.s.get(self.i).ok_or("JSON incompleto")?;
        match c {
            b'{' => {
                self.i += 1;
                let o = vm.novo_objeto();
                let r = o.obj().unwrap_or(0);
                self.espacos();
                if self.s.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Ok(o);
                }
                loop {
                    self.espacos();
                    if self.s.get(self.i) != Some(&b'"') {
                        return Err("JSON: esperava chave");
                    }
                    let k = self.texto()?;
                    self.espacos();
                    if self.s.get(self.i) != Some(&b':') {
                        return Err("JSON: esperava ':'");
                    }
                    self.i += 1;
                    let v = self.valor(vm, prof + 1)?;
                    if &*k == "__proto__" {
                        vm.heap.get_mut(r).definir(chave(&k), Slot::valor(v));
                    } else {
                        vm.definir_propria(r, chave(&k), v);
                    }
                    self.espacos();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(o);
                        }
                        _ => return Err("JSON: esperava ',' ou '}'"),
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut v = Vec::new();
                self.espacos();
                if self.s.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Ok(vm.novo_array(v));
                }
                loop {
                    v.push(self.valor(vm, prof + 1)?);
                    self.espacos();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(vm.novo_array(v));
                        }
                        _ => return Err("JSON: esperava ',' ou ']'"),
                    }
                }
            }
            b'"' => Ok(Valor::Texto(self.texto()?)),
            b't' if self.s[self.i..].starts_with(b"true") => {
                self.i += 4;
                Ok(Valor::Bool(true))
            }
            b'f' if self.s[self.i..].starts_with(b"false") => {
                self.i += 5;
                Ok(Valor::Bool(false))
            }
            b'n' if self.s[self.i..].starts_with(b"null") => {
                self.i += 4;
                Ok(Valor::Nulo)
            }
            b'-' | b'0'..=b'9' => {
                let ini = self.i;
                if self.s[self.i] == b'-' {
                    self.i += 1;
                }
                while self.i < self.s.len() && matches!(self.s[self.i], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-') {
                    self.i += 1;
                }
                let t = core::str::from_utf8(&self.s[ini..self.i]).map_err(|_| "JSON: número")?;
                t.parse::<f64>().map(Valor::Num).map_err(|_| "JSON: número inválido")
            }
            _ => Err("JSON: caractere inesperado"),
        }
    }

    fn texto(&mut self) -> Result<Rc<str>, &'static str> {
        self.i += 1;
        let ini = self.i;
        while self.i < self.s.len() && self.s[self.i] != b'"' && self.s[self.i] != b'\\' {
            self.i += 1;
        }
        if self.s.get(self.i) == Some(&b'"') {
            let t = core::str::from_utf8(&self.s[ini..self.i]).map_err(|_| "JSON: UTF-8")?;
            self.i += 1;
            return Ok(Rc::from(t));
        }
        let mut u: Vec<u16> = core::str::from_utf8(&self.s[ini..self.i]).map_err(|_| "JSON: UTF-8")?.encode_utf16().collect();
        loop {
            let c = *self.s.get(self.i).ok_or("JSON: texto sem fim")?;
            match c {
                b'"' => {
                    self.i += 1;
                    return Ok(Rc::from(String::from_utf16_lossy(&u).as_str()));
                }
                b'\\' => {
                    let e = *self.s.get(self.i + 1).ok_or("JSON: escape")?;
                    self.i += 2;
                    match e {
                        b'n' => u.push(10),
                        b't' => u.push(9),
                        b'r' => u.push(13),
                        b'b' => u.push(8),
                        b'f' => u.push(12),
                        b'u' => {
                            let h = core::str::from_utf8(self.s.get(self.i..self.i + 4).ok_or("JSON: \\u")?).map_err(|_| "JSON: \\u")?;
                            u.push(u16::from_str_radix(h, 16).map_err(|_| "JSON: \\u")?);
                            self.i += 4;
                        }
                        x => u.push(x as u16),
                    }
                }
                _ => {
                    // um caractere UTF-8 inteiro
                    let resto = core::str::from_utf8(&self.s[self.i..(self.i + 4).min(self.s.len())]).unwrap_or_else(|e| {
                        core::str::from_utf8(&self.s[self.i..self.i + e.valid_up_to().max(1)]).unwrap_or("?")
                    });
                    let ch = resto.chars().next().unwrap_or('?');
                    let mut b = [0u16; 2];
                    u.extend_from_slice(ch.encode_utf16(&mut b));
                    self.i += ch.len_utf8();
                }
            }
        }
    }
}

fn reviver(vm: &mut Vm, f: &Valor, dono: &Valor, k: Valor) -> R<Valor> {
    let ch = vm.chave_de(&k)?;
    let v = vm.pegar(dono, ch)?;
    if let Valor::Obj(r) = &v {
        let ks = vm.chaves_proprias(*r, true);
        for kk in ks {
            let novo = reviver(vm, f, &v, kk.clone())?;
            let c2 = vm.chave_de(&kk)?;
            if matches!(novo, Valor::Indef) {
                vm.heap.get_mut(*r).remover(c2);
            } else {
                vm.definir_propria(*r, c2, novo);
            }
        }
    }
    vm.chamar(f, dono, &[k, v])
}

fn json_parse(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = texto(vm, c, 0)?;
    let mut j = Json { s: s.as_bytes(), i: 0 };
    let v = match j.valor(vm, 0) {
        Ok(v) => v,
        Err(e) => {
            let p = vm.p.erro_sintaxe;
            return Err(vm.novo_erro(p, e));
        }
    };
    j.espacos();
    if j.i < j.s.len() {
        let p = vm.p.erro_sintaxe;
        return Err(vm.novo_erro(p, "JSON: sobra depois do valor"));
    }
    let f = c.arg(1);
    if vm.eh_chamavel(&f) {
        let raiz = vm.novo_objeto();
        let r = raiz.obj().unwrap_or(0);
        vm.definir_propria(r, chave(""), v);
        return reviver(vm, &f, &raiz, Valor::texto(""));
    }
    Ok(v)
}

fn json_texto(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

struct Serializar<'a> {
    substituto: &'a Valor,
    lista: Option<Vec<Rc<str>>>,
    recuo: String,
    pilha: Vec<Ref>,
}

impl Serializar<'_> {
    fn valor(&mut self, vm: &mut Vm, dono: &Valor, k: Valor, out: &mut String, nivel: &str) -> R<bool> {
        let ch = vm.chave_de(&k)?;
        let mut v = vm.pegar(dono, ch)?;
        if let Valor::Obj(_) = v {
            let tj = vm.pegar(&v, chave("toJSON"))?;
            if vm.eh_chamavel(&tj) {
                v = vm.chamar(&tj, &v, &[k.clone()])?;
            }
        }
        if vm.eh_chamavel(self.substituto) {
            v = vm.chamar(self.substituto, dono, &[k, v])?;
        }
        if let Valor::Obj(r) = &v {
            match &vm.heap.get(*r).classe {
                Classe::Numero(n) => v = Valor::Num(*n),
                Classe::Texto(s) => v = Valor::Texto(s.clone()),
                Classe::Booleano(b) => v = Valor::Bool(*b),
                _ => {}
            }
        }
        match &v {
            Valor::Nulo => out.push_str("null"),
            Valor::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Valor::Num(n) => out.push_str(&if n.is_finite() { numero_texto(*n) } else { "null".into() }),
            Valor::Texto(s) => json_texto(s, out),
            Valor::Indef | Valor::Simbolo(_) => return Ok(false),
            Valor::Obj(r) => {
                if vm.eh_chamavel(&v) {
                    return Ok(false);
                }
                let r = *r;
                if self.pilha.contains(&r) {
                    return vm.erro_tipo("JSON.stringify: estrutura circular");
                }
                self.pilha.push(r);
                let interno = format!("{nivel}{}", self.recuo);
                let quebra = !self.recuo.is_empty();
                if vm.eh_array(&v) {
                    let n = arr(vm, r).len();
                    out.push('[');
                    for i in 0..n {
                        if i > 0 {
                            out.push(',');
                        }
                        if quebra {
                            out.push('\n');
                            out.push_str(&interno);
                        }
                        if !self.valor(vm, &v, Valor::Num(i as f64), out, &interno)? {
                            out.push_str("null");
                        }
                    }
                    if quebra && n > 0 {
                        out.push('\n');
                        out.push_str(nivel);
                    }
                    out.push(']');
                } else {
                    let chaves: Vec<Valor> = match &self.lista {
                        Some(l) => l.iter().map(|s| Valor::Texto(s.clone())).collect(),
                        None => vm.chaves_proprias(r, true),
                    };
                    out.push('{');
                    let mut primeiro = true;
                    for kk in chaves {
                        let mut pedaco = String::new();
                        if quebra {
                            pedaco.push('\n');
                            pedaco.push_str(&interno);
                        }
                        let ks = vm.para_texto(&kk)?;
                        json_texto(&ks, &mut pedaco);
                        pedaco.push(':');
                        if quebra {
                            pedaco.push(' ');
                        }
                        if self.valor(vm, &v, kk, &mut pedaco, &interno)? {
                            if !primeiro {
                                out.push(',');
                            }
                            out.push_str(&pedaco);
                            primeiro = false;
                        }
                    }
                    if quebra && !primeiro {
                        out.push('\n');
                        out.push_str(nivel);
                    }
                    out.push('}');
                }
                self.pilha.pop();
            }
        }
        Ok(true)
    }
}

fn json_stringify(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let sub = c.arg(1);
    let lista = if vm.eh_array(&sub) {
        let itens = itens_de(vm, &sub)?;
        let mut l = Vec::new();
        for x in itens {
            l.push(vm.para_texto(&x)?);
        }
        Some(l)
    } else {
        None
    };
    let recuo = match c.arg(2) {
        Valor::Num(n) => " ".repeat((n.max(0.0) as usize).min(10)),
        Valor::Texto(s) => s.chars().take(10).collect(),
        _ => String::new(),
    };
    let mut s = Serializar { substituto: &sub, lista, recuo, pilha: Vec::new() };
    let raiz = vm.novo_objeto();
    if let Valor::Obj(r) = raiz {
        vm.definir_propria(r, chave(""), c.arg(0));
    }
    let mut out = String::new();
    if s.valor(vm, &raiz, Valor::texto(""), &mut out, "")? {
        Ok(Valor::texto(&out))
    } else {
        Ok(Valor::Indef)
    }
}

// ================================================================ Promise

fn instalar_promessa(vm: &mut Vm) {
    let pp = vm.p.promessa;
    let p = construtor(vm, "Promise", prom_ctor, 1, pp);
    vm.metodo(pp, "then", prom_then, 2);
    vm.metodo(pp, "catch", prom_catch, 1);
    vm.metodo(pp, "finally", prom_finally, 1);
    vm.metodo(p, "resolve", prom_resolve, 1);
    vm.metodo(p, "reject", prom_reject, 1);
    vm.metodo(p, "all", prom_all, 1);
    vm.metodo(p, "allSettled", prom_all_settled, 1);
    vm.metodo(p, "race", prom_race, 1);
    vm.metodo(p, "any", prom_any, 1);
    vm.metodo(p, "withResolvers", prom_with_resolvers, 0);
    vm.heap.get_mut(pp).definir(SIM_ETIQUETA, Slot::oculto(Valor::texto("Promise")));
}

fn nat_resolver(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    vm.resolver_promessa(c.dado, c.arg(0));
    Ok(Valor::Indef)
}

fn nat_rejeitar(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    vm.assentar(c.dado, c.arg(0), true);
    Ok(Valor::Indef)
}

fn nat_async_cumprir(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    if let Valor::Obj(co) = c.dado {
        vm.passo_async(*co, c.arg(0), false);
    }
    Ok(Valor::Indef)
}

fn nat_async_rejeitar(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    if let Valor::Obj(co) = c.dado {
        vm.passo_async(*co, c.arg(0), true);
    }
    Ok(Valor::Indef)
}

fn prom_ctor(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let exec = c.arg(0);
    if !vm.eh_chamavel(&exec) {
        return vm.erro_tipo("Promise precisa de uma função");
    }
    let p = vm.nova_promessa();
    if let (Some(alvo), Valor::Obj(r)) = (c.novo, &p) {
        if let Valor::Obj(pr) = vm.pegar(alvo, chave("prototype"))? {
            vm.heap.get_mut(*r).proto = Some(pr);
        }
    }
    let (res, rej) = vm.funcoes_resolucao(&p);
    if let Err(e) = vm.chamar(&exec, &Valor::Indef, &[res, rej]) {
        vm.assentar(&p, e, true);
    }
    Ok(p)
}

fn eh_promessa(vm: &Vm, v: &Valor) -> bool {
    matches!(v, Valor::Obj(r) if matches!(vm.heap.get(*r).classe, Classe::Promessa(_)))
}

fn prom_then(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    if !eh_promessa(vm, c.this) {
        return vm.erro_tipo("then em não-promessa");
    }
    Ok(vm.entao(c.this, c.arg(0), c.arg(1)))
}

fn prom_catch(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let then = vm.pegar(c.this, chave("then"))?;
    vm.chamar(&then, c.this, &[Valor::Indef, c.arg(0)])
}

fn prom_finally(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    // finally(f): chama f e repassa o resultado original
    let f = c.arg(0);
    if !vm.eh_chamavel(&f) {
        return Ok(vm.entao(c.this, f.clone(), f));
    }
    let id_ok = vm.registrar(fin_ok);
    let id_erro = vm.registrar(fin_erro);
    let a = vm.nativa_com(id_ok, "", 1, f.clone());
    let b = vm.nativa_com(id_erro, "", 1, f);
    Ok(vm.entao(c.this, a, b))
}

fn fin_ok(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    vm.chamar(c.dado, &Valor::Indef, &[])?;
    Ok(c.arg(0))
}

fn fin_erro(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    vm.chamar(c.dado, &Valor::Indef, &[])?;
    Err(c.arg(0))
}

fn prom_resolve(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let v = c.arg(0);
    if eh_promessa(vm, &v) {
        return Ok(v);
    }
    let p = vm.nova_promessa();
    vm.resolver_promessa(&p, v);
    Ok(p)
}

fn prom_reject(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let p = vm.nova_promessa();
    vm.assentar(&p, c.arg(0), true);
    Ok(p)
}

fn prom_with_resolvers(vm: &mut Vm, _c: &Ctx) -> R<Valor> {
    let p = vm.nova_promessa();
    let (res, rej) = vm.funcoes_resolucao(&p);
    let o = vm.novo_objeto();
    let r = o.obj().unwrap_or(0);
    vm.definir_propria(r, chave("promise"), p);
    vm.definir_propria(r, chave("resolve"), res);
    vm.definir_propria(r, chave("reject"), rej);
    Ok(o)
}

/// Estado compartilhado de Promise.all/allSettled/any: um objeto com
/// "r" (array de resultados), "n" (faltam), "p" (promessa final).
fn combinador(vm: &mut Vm, c: &Ctx, modo: u8) -> R<Valor> {
    let itens = vm.para_vec(&c.arg(0))?;
    let final_ = vm.nova_promessa();
    let n = itens.len();
    let resultados = vm.novo_array(alloc::vec![Valor::Indef; n]);
    if n == 0 {
        if modo == 3 {
            let e = vm.novo_erro(vm.p.erro, "todas as promessas foram rejeitadas");
            vm.assentar(&final_, e, true);
        } else {
            vm.resolver_promessa(&final_, resultados);
        }
        return Ok(final_);
    }
    let estado = vm.novo_objeto();
    let er = estado.obj().unwrap_or(0);
    vm.definir_propria(er, chave("r"), resultados);
    vm.definir_propria(er, chave("n"), Valor::Num(n as f64));
    vm.definir_propria(er, chave("p"), final_.clone());
    let id_ok = vm.registrar(comb_ok);
    let id_erro = vm.registrar(comb_erro);
    for (i, x) in itens.into_iter().enumerate() {
        let p = vm.nova_promessa();
        vm.resolver_promessa(&p, x);
        let dado = vm.novo_array(alloc::vec![estado.clone(), Valor::Num(i as f64), Valor::Num(modo as f64)]);
        let a = vm.nativa_com(id_ok, "", 1, dado.clone());
        let b = vm.nativa_com(id_erro, "", 1, dado);
        vm.entao(&p, a, b);
    }
    Ok(final_)
}

fn comb_registrar(vm: &mut Vm, c: &Ctx, valor: Valor, rejeitado: bool) -> R<Valor> {
    let estado = vm.pegar_idx(c.dado, &Valor::Num(0.0))?;
    let i = vm.pegar_idx(c.dado, &Valor::Num(1.0))?;
    let modo = vm.pegar_idx(c.dado, &Valor::Num(2.0))?;
    let modo = if let Valor::Num(m) = modo { m as u8 } else { 0 };
    let final_ = vm.pegar(&estado, chave("p"))?;
    let resultados = vm.pegar(&estado, chave("r"))?;
    match (modo, rejeitado) {
        (0, true) => {
            vm.assentar(&final_, valor, true);
            return Ok(Valor::Indef);
        }
        (3, false) => {
            vm.resolver_promessa(&final_, valor);
            return Ok(Valor::Indef);
        }
        (1, _) => {
            let o = vm.novo_objeto();
            let r = o.obj().unwrap_or(0);
            vm.definir_propria(r, chave("status"), Valor::texto(if rejeitado { "rejected" } else { "fulfilled" }));
            vm.definir_propria(r, chave(if rejeitado { "reason" } else { "value" }), valor);
            vm.por_idx(&resultados, &i, o)?;
        }
        _ => {
            vm.por_idx(&resultados, &i, valor)?;
        }
    }
    let falta = vm.pegar(&estado, chave("n"))?;
    let falta = vm.para_numero(&falta)? - 1.0;
    vm.por(&estado, chave("n"), Valor::Num(falta))?;
    if falta == 0.0 {
        if modo == 3 {
            let e = vm.novo_erro(vm.p.erro, "todas as promessas foram rejeitadas");
            vm.assentar(&final_, e, true);
        } else {
            vm.resolver_promessa(&final_, resultados);
        }
    }
    Ok(Valor::Indef)
}

fn comb_ok(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    comb_registrar(vm, c, c.arg(0), false)
}

fn comb_erro(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    comb_registrar(vm, c, c.arg(0), true)
}

fn prom_all(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    combinador(vm, c, 0)
}

fn prom_all_settled(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    combinador(vm, c, 1)
}

fn prom_any(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    combinador(vm, c, 3)
}

fn prom_race(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let itens = vm.para_vec(&c.arg(0))?;
    let final_ = vm.nova_promessa();
    let (res, rej) = vm.funcoes_resolucao(&final_);
    for x in itens {
        let p = vm.nova_promessa();
        vm.resolver_promessa(&p, x);
        vm.entao(&p, res.clone(), rej.clone());
    }
    Ok(final_)
}

// ================================================================ Map / Set

fn instalar_colecoes(vm: &mut Vm) {
    let pm = vm.p.mapa;
    construtor(vm, "Map", mapa_ctor, 0, pm);
    let pw = vm.alocar(Some(pm), Classe::Comum);
    construtor(vm, "WeakMap", mapa_ctor, 0, pw);
    for (n, f, k) in [
        ("get", mapa_get as super::vm::FnNativa, 1u8),
        ("set", mapa_set, 2),
        ("has", mapa_has, 1),
        ("delete", mapa_delete, 1),
        ("clear", mapa_clear, 0),
        ("forEach", mapa_for_each, 1),
        ("keys", mapa_keys, 0),
        ("values", mapa_values, 0),
    ] {
        vm.metodo(pm, n, f, k);
    }
    let entradas = vm.nativa("entries", mapa_entries, 0);
    vm.heap.get_mut(pm).definir(chave("entries"), Slot::oculto(entradas.clone()));
    vm.heap.get_mut(pm).definir(SIM_ITERADOR, Slot::oculto(entradas));
    getter(vm, pm, "size", mapa_size);
    let pc = vm.p.conjunto;
    construtor(vm, "Set", conj_ctor, 0, pc);
    let pws = vm.alocar(Some(pc), Classe::Comum);
    construtor(vm, "WeakSet", conj_ctor, 0, pws);
    for (n, f, k) in [
        ("add", conj_add as super::vm::FnNativa, 1u8),
        ("has", mapa_has, 1),
        ("delete", mapa_delete, 1),
        ("clear", mapa_clear, 0),
        ("forEach", mapa_for_each, 1),
        ("entries", mapa_entries, 0),
    ] {
        vm.metodo(pc, n, f, k);
    }
    let valores = vm.nativa("values", mapa_values, 0);
    vm.heap.get_mut(pc).definir(chave("values"), Slot::oculto(valores.clone()));
    vm.heap.get_mut(pc).definir(chave("keys"), Slot::oculto(valores.clone()));
    vm.heap.get_mut(pc).definir(SIM_ITERADOR, Slot::oculto(valores));
    getter(vm, pc, "size", mapa_size);
    let g = vm.global;
    let wr = vm.nativa("WeakRef", weakref_ctor, 1);
    vm.definir(g, "WeakRef", wr);
}

fn weakref_ctor(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let o = vm.novo_objeto();
    let alvo = c.arg(0);
    let id = vm.registrar(|_vm, c| Ok(c.dado.clone()));
    let f = vm.nativa_com(id, "deref", 0, alvo);
    if let Valor::Obj(r) = o {
        vm.definir(r, "deref", f);
    }
    Ok(o)
}

fn novo_mapa(vm: &mut Vm, c: &Ctx, eh_mapa: bool) -> R<Ref> {
    let proto = match c.novo {
        Some(alvo) => vm.pegar(alvo, chave("prototype"))?.obj(),
        None => return vm.erro_tipo("Map/Set precisam de new"),
    };
    Ok(vm.alocar(proto, Classe::Mapa(Box::new(Mapa { eh_mapa, ..Default::default() }))))
}

fn mapa_ctor(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let r = novo_mapa(vm, c, true)?;
    let fonte = c.arg(0);
    if !fonte.eh_nulo_ou_indef() {
        for par in vm.para_vec(&fonte)? {
            let k = vm.pegar_idx(&par, &Valor::Num(0.0))?;
            let v = vm.pegar_idx(&par, &Valor::Num(1.0))?;
            mapa(vm, r)?.por(k, v);
        }
    }
    Ok(Valor::Obj(r))
}

fn conj_ctor(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let r = novo_mapa(vm, c, false)?;
    let fonte = c.arg(0);
    if !fonte.eh_nulo_ou_indef() {
        for v in vm.para_vec(&fonte)? {
            mapa(vm, r)?.por(v.clone(), v);
        }
    }
    Ok(Valor::Obj(r))
}

fn mapa(vm: &mut Vm, r: Ref) -> R<&mut Mapa> {
    if !matches!(vm.heap.get(r).classe, Classe::Mapa(_)) {
        return vm.erro_tipo("this não é Map/Set");
    }
    match &mut vm.heap.get_mut(r).classe {
        Classe::Mapa(m) => Ok(m),
        _ => unreachable!(),
    }
}

fn this_mapa(vm: &mut Vm, c: &Ctx) -> R<Ref> {
    match c.this {
        Valor::Obj(r) if matches!(vm.heap.get(*r).classe, Classe::Mapa(_)) => Ok(*r),
        _ => vm.erro_tipo("this não é Map/Set"),
    }
}

fn mapa_get(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let r = this_mapa(vm, c)?;
    Ok(mapa(vm, r)?.pegar(&c.arg(0)).cloned().unwrap_or_default())
}

fn mapa_set(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let r = this_mapa(vm, c)?;
    mapa(vm, r)?.por(c.arg(0), c.arg(1));
    Ok(c.this.clone())
}

fn conj_add(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let r = this_mapa(vm, c)?;
    mapa(vm, r)?.por(c.arg(0), c.arg(0));
    Ok(c.this.clone())
}

fn mapa_has(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let r = this_mapa(vm, c)?;
    Ok(Valor::Bool(mapa(vm, r)?.pegar(&c.arg(0)).is_some()))
}

fn mapa_delete(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let r = this_mapa(vm, c)?;
    Ok(Valor::Bool(mapa(vm, r)?.apagar(&c.arg(0))))
}

fn mapa_clear(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let r = this_mapa(vm, c)?;
    mapa(vm, r)?.limpar();
    Ok(Valor::Indef)
}

fn mapa_size(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let r = this_mapa(vm, c)?;
    Ok(Valor::Num(mapa(vm, r)?.tamanho() as f64))
}

fn mapa_for_each(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let r = this_mapa(vm, c)?;
    let f = c.arg(0);
    let mut i = 0;
    loop {
        let item = match &vm.heap.get(r).classe {
            Classe::Mapa(m) => {
                if i >= m.itens.len() {
                    break;
                }
                m.itens[i].clone()
            }
            _ => break,
        };
        if let Some((k, v)) = item {
            vm.chamar(&f, &c.arg(1), &[v, k, c.this.clone()])?;
        }
        i += 1;
    }
    Ok(Valor::Indef)
}

fn mapa_keys(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(iter_de(vm, c.this.clone(), 1))
}

fn mapa_values(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(iter_de(vm, c.this.clone(), 0))
}

fn mapa_entries(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(iter_de(vm, c.this.clone(), 2))
}

// ================================================================ RegExp

fn instalar_regex(vm: &mut Vm) {
    let pr = vm.p.regex;
    construtor(vm, "RegExp", re_ctor, 2, pr);
    vm.metodo(pr, "exec", re_exec, 1);
    vm.metodo(pr, "test", re_test, 1);
    vm.metodo(pr, "toString", re_to_string, 0);
    getter(vm, pr, "source", re_source);
    getter(vm, pr, "flags", re_flags);
    getter(vm, pr, "global", re_global);
    getter(vm, pr, "ignoreCase", re_ignore_case);
    getter(vm, pr, "multiline", re_multiline);
    getter(vm, pr, "sticky", re_sticky);
    getter(vm, pr, "unicode", re_unicode);
}

pub fn nova_regex(vm: &mut Vm, corpo: &str, flags: &str) -> R<Valor> {
    match Regex::nova(corpo, flags) {
        Ok(re) => {
            let p = vm.p.regex;
            let r = vm.alocar(Some(p), Classe::Regex(Rc::new(re)));
            vm.heap.get_mut(r).definir(chave("lastIndex"), Slot { v: Valor::Num(0.0), acessor: None, enumeravel: false, gravavel: true, configuravel: false });
            Ok(Valor::Obj(r))
        }
        Err(e) => {
            let p = vm.p.erro_sintaxe;
            Err(vm.novo_erro(p, &format!("expressão regular inválida /{corpo}/: {e}")))
        }
    }
}

fn re_ctor(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let p = c.arg(0);
    let (corpo, mut flags) = match como_regex(vm, &p) {
        Some(re) => (re.fonte.clone(), re.flags.clone()),
        None => (if matches!(p, Valor::Indef) { Rc::from("(?:)") } else { vm.para_texto(&p)? }, Rc::from("")),
    };
    if !matches!(c.arg(1), Valor::Indef) {
        flags = texto(vm, c, 1)?;
    }
    nova_regex(vm, &corpo, &flags)
}

fn this_regex(vm: &mut Vm, c: &Ctx) -> R<Rc<Regex>> {
    match como_regex(vm, c.this) {
        Some(r) => Ok(r),
        None => vm.erro_tipo("this não é RegExp"),
    }
}

fn re_exec(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let re = this_regex(vm, c)?;
    let s = texto(vm, c, 0)?;
    let t: Vec<char> = s.chars().collect();
    let usa_ultimo = re.global || re.grudado;
    let li = if usa_ultimo {
        let v = vm.pegar(c.this, chave("lastIndex"))?;
        // lastIndex e em UTF-16: converte para indice de char
        let u = vm.para_numero(&v)?.max(0.0) as usize;
        let mut acc = 0;
        let mut ci = 0;
        while ci < t.len() && acc < u {
            acc += t[ci].len_utf16();
            ci += 1;
        }
        ci
    } else {
        0
    };
    if li > t.len() {
        vm.por(c.this, chave("lastIndex"), Valor::Num(0.0))?;
        return Ok(Valor::Nulo);
    }
    match re.procurar(&t, li) {
        Some(caps) => {
            if usa_ultimo {
                let fim = caps[0].map_or(0, |x| x.1);
                let u: usize = t[..fim].iter().map(|c| c.len_utf16()).sum();
                vm.por(c.this, chave("lastIndex"), Valor::Num(u as f64))?;
            }
            Ok(resultado_exec(vm, &re, &t, &s, &caps))
        }
        None => {
            if usa_ultimo {
                vm.por(c.this, chave("lastIndex"), Valor::Num(0.0))?;
            }
            Ok(Valor::Nulo)
        }
    }
}

fn re_test(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let r = re_exec(vm, c)?;
    Ok(Valor::Bool(!matches!(r, Valor::Nulo)))
}

fn re_to_string(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let re = this_regex(vm, c)?;
    Ok(Valor::texto(&format!("/{}/{}", re.fonte, re.flags)))
}

fn re_source(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(Valor::Texto(this_regex(vm, c)?.fonte.clone()))
}

fn re_flags(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(Valor::Texto(this_regex(vm, c)?.flags.clone()))
}

fn re_global(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(Valor::Bool(this_regex(vm, c)?.global))
}

fn re_ignore_case(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(Valor::Bool(this_regex(vm, c)?.sem_caixa))
}

fn re_multiline(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(Valor::Bool(this_regex(vm, c)?.multilinha))
}

fn re_sticky(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(Valor::Bool(this_regex(vm, c)?.grudado))
}

fn re_unicode(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(Valor::Bool(this_regex(vm, c)?.unicode))
}

// ================================================================ Date

/// Hora atual em ms desde 1970 (o anfitriao define o relogio).
pub static mut RELOGIO: Option<fn() -> f64> = None;

pub fn agora(_vm: &Vm) -> f64 {
    match unsafe { RELOGIO } {
        Some(f) => f(),
        None => 0.0,
    }
}

fn dias_de_civil(a: i64, me: i64, d: i64) -> i64 {
    let a = if me <= 2 { a - 1 } else { a };
    let era = a.div_euclid(400);
    let ae = a - era * 400;
    let mp = (me + 9) % 12;
    let dia_ano = (153 * mp + 2) / 5 + d - 1;
    let dia_era = ae * 365 + ae / 4 - ae / 100 + dia_ano;
    era * 146097 + dia_era - 719468
}

fn civil_de_dias(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let me = if mp < 10 { mp + 3 } else { mp - 9 };
    (if me <= 2 { y + 1 } else { y }, me, d)
}

/// (ano, mes 0-11, dia, hora, min, seg, ms, dia da semana)
fn partes_data(t: f64) -> (i64, i64, i64, i64, i64, i64, i64, i64) {
    let ms = t as i64;
    let dias = ms.div_euclid(86_400_000);
    let resto = ms.rem_euclid(86_400_000);
    let (a, me, d) = civil_de_dias(dias);
    let semana = (dias + 4).rem_euclid(7);
    (a, me - 1, d, resto / 3_600_000, resto / 60_000 % 60, resto / 1000 % 60, resto % 1000, semana)
}

fn tempo_de(a: f64, me: f64, d: f64, h: f64, mi: f64, s: f64, ms: f64) -> f64 {
    if [a, me, d, h, mi, s, ms].iter().any(|x| !x.is_finite()) {
        return f64::NAN;
    }
    let a = a as i64 + (me as i64).div_euclid(12);
    let me = (me as i64).rem_euclid(12) + 1;
    let dias = dias_de_civil(a, me, 1) as f64 + d - 1.0;
    dias * 86_400_000.0 + h * 3_600_000.0 + mi * 60_000.0 + s * 1000.0 + ms
}

fn ler_data(s: &str) -> f64 {
    // ISO 8601: AAAA-MM-DD[THH:MM[:SS[.mmm]]][Z|+hh:mm]
    let s = s.trim();
    let b = s.as_bytes();
    let num = |a: usize, n: usize| -> Option<f64> { s.get(a..a + n)?.parse::<f64>().ok() };
    if b.len() >= 10 && b[4] == b'-' {
        let (Some(a), Some(me), Some(d)) = (num(0, 4), num(5, 2), num(8, 2)) else { return f64::NAN };
        let (mut h, mut mi, mut se, mut ms) = (0.0, 0.0, 0.0, 0.0);
        let mut i = 10;
        if b.len() > 11 && (b[10] == b'T' || b[10] == b' ') {
            h = num(11, 2).unwrap_or(0.0);
            mi = num(14, 2).unwrap_or(0.0);
            i = 16;
            if b.get(16) == Some(&b':') {
                se = num(17, 2).unwrap_or(0.0);
                i = 19;
                if b.get(19) == Some(&b'.') {
                    let fim = s[20..].find(|c: char| !c.is_ascii_digit()).map_or(s.len(), |p| 20 + p);
                    ms = s[20..fim].get(..3).and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0);
                    i = fim;
                }
            }
        }
        let mut t = tempo_de(a, me - 1.0, d, h, mi, se, ms);
        if let Some(fuso) = s.get(i..) {
            if fuso.starts_with('+') || fuso.starts_with('-') {
                let sinal = if fuso.starts_with('-') { -1.0 } else { 1.0 };
                let hh = fuso.get(1..3).and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0);
                let mm = fuso.get(fuso.len().saturating_sub(2)..).and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0);
                t -= sinal * (hh * 3_600_000.0 + mm * 60_000.0);
            }
        }
        return t;
    }
    f64::NAN
}

fn instalar_data(vm: &mut Vm) {
    let pd = vm.p.data;
    let d = construtor(vm, "Date", data_ctor, 7, pd);
    vm.metodo(d, "now", data_now, 0);
    vm.metodo(d, "parse", data_parse, 1);
    vm.metodo(d, "UTC", data_utc, 7);
    let gets: &[(&str, usize)] = &[
        ("getFullYear", 0),
        ("getMonth", 1),
        ("getDate", 2),
        ("getHours", 3),
        ("getMinutes", 4),
        ("getSeconds", 5),
        ("getMilliseconds", 6),
        ("getDay", 7),
        ("getUTCFullYear", 0),
        ("getUTCMonth", 1),
        ("getUTCDate", 2),
        ("getUTCHours", 3),
        ("getUTCMinutes", 4),
        ("getUTCSeconds", 5),
        ("getUTCMilliseconds", 6),
        ("getUTCDay", 7),
    ];
    let id = vm.registrar(data_get_parte);
    for &(n, k) in gets {
        let f = vm.nativa_com(id, n, 0, Valor::Num(k as f64));
        vm.definir(pd, n, f);
    }
    let sets: &[(&str, usize)] = &[
        ("setFullYear", 0),
        ("setMonth", 1),
        ("setDate", 2),
        ("setHours", 3),
        ("setMinutes", 4),
        ("setSeconds", 5),
        ("setMilliseconds", 6),
        ("setUTCFullYear", 0),
        ("setUTCMonth", 1),
        ("setUTCDate", 2),
        ("setUTCHours", 3),
        ("setUTCMinutes", 4),
        ("setUTCSeconds", 5),
        ("setUTCMilliseconds", 6),
    ];
    let id = vm.registrar(data_set_parte);
    for &(n, k) in sets {
        let f = vm.nativa_com(id, n, 1, Valor::Num(k as f64));
        vm.definir(pd, n, f);
    }
    for (n, f) in [
        ("getTime", data_get_time as super::vm::FnNativa),
        ("valueOf", data_get_time),
        ("setTime", data_set_time),
        ("getTimezoneOffset", data_tz),
        ("toISOString", data_iso),
        ("toJSON", data_iso),
        ("toString", data_to_string),
        ("toUTCString", data_to_string),
        ("toDateString", data_to_string),
        ("toTimeString", data_to_string),
        ("toLocaleString", data_locale),
        ("toLocaleDateString", data_locale_data),
        ("toLocaleTimeString", data_locale_hora),
    ] {
        vm.metodo(pd, n, f, 0);
    }
}

fn this_data(vm: &mut Vm, c: &Ctx) -> R<f64> {
    match c.this {
        Valor::Obj(r) => match vm.heap.get(*r).classe {
            Classe::Data(t) => Ok(t),
            _ => vm.erro_tipo("this não é Date"),
        },
        _ => vm.erro_tipo("this não é Date"),
    }
}

fn data_ctor(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let t = match c.args.len() {
        0 => agora(vm),
        1 => match c.arg(0) {
            Valor::Texto(s) => ler_data(&s),
            Valor::Obj(r) if matches!(vm.heap.get(r).classe, Classe::Data(_)) => match vm.heap.get(r).classe {
                Classe::Data(t) => t,
                _ => f64::NAN,
            },
            v => vm.para_numero(&v)?,
        },
        _ => {
            let mut p = [0.0; 7];
            p[2] = 1.0;
            for (i, x) in p.iter_mut().enumerate().take(c.args.len().min(7)) {
                *x = num(vm, c, i)?;
            }
            if (0.0..100.0).contains(&p[0]) {
                p[0] += 1900.0;
            }
            tempo_de(p[0], p[1], p[2], p[3], p[4], p[5], p[6])
        }
    };
    if c.novo.is_none() {
        return Ok(Valor::texto(&texto_data(agora(vm))));
    }
    let p = match c.novo {
        Some(alvo) => vm.pegar(alvo, chave("prototype"))?.obj(),
        None => Some(vm.p.data),
    };
    let t = if t.is_finite() { m::trunc(t) } else { f64::NAN };
    Ok(Valor::Obj(vm.alocar(p, Classe::Data(t))))
}

fn data_now(vm: &mut Vm, _c: &Ctx) -> R<Valor> {
    Ok(Valor::Num(m::floor(agora(vm))))
}

fn data_parse(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = texto(vm, c, 0)?;
    Ok(Valor::Num(ler_data(&s)))
}

fn data_utc(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let mut p = [0.0; 7];
    p[2] = 1.0;
    for (i, x) in p.iter_mut().enumerate().take(c.args.len().min(7)) {
        *x = num(vm, c, i)?;
    }
    Ok(Valor::Num(tempo_de(p[0], p[1], p[2], p[3], p[4], p[5], p[6])))
}

fn data_get_parte(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let t = this_data(vm, c)?;
    if t.is_nan() {
        return Ok(Valor::Num(f64::NAN));
    }
    let p = partes_data(t);
    let k = if let Valor::Num(k) = c.dado { *k as usize } else { 0 };
    Ok(Valor::Num([p.0, p.1, p.2, p.3, p.4, p.5, p.6, p.7][k] as f64))
}

fn data_set_parte(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let t = this_data(vm, c)?;
    let p = partes_data(if t.is_nan() { 0.0 } else { t });
    let mut v = [p.0 as f64, p.1 as f64, p.2 as f64, p.3 as f64, p.4 as f64, p.5 as f64, p.6 as f64];
    let k = if let Valor::Num(k) = c.dado { *k as usize } else { 0 };
    for i in 0..c.args.len().min(7 - k) {
        v[k + i] = num(vm, c, i)?;
    }
    let novo = tempo_de(v[0], v[1], v[2], v[3], v[4], v[5], v[6]);
    if let Valor::Obj(r) = c.this {
        vm.heap.get_mut(*r).classe = Classe::Data(novo);
    }
    Ok(Valor::Num(novo))
}

fn data_get_time(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(Valor::Num(this_data(vm, c)?))
}

fn data_set_time(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    this_data(vm, c)?;
    let t = num(vm, c, 0)?;
    if let Valor::Obj(r) = c.this {
        vm.heap.get_mut(*r).classe = Classe::Data(t);
    }
    Ok(Valor::Num(t))
}

fn data_tz(_vm: &mut Vm, _c: &Ctx) -> R<Valor> {
    Ok(Valor::Num(0.0))
}

fn data_iso(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let t = this_data(vm, c)?;
    if !t.is_finite() {
        return vm.erro_faixa("data inválida");
    }
    let p = partes_data(t);
    Ok(Valor::texto(&format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z", p.0, p.1 + 1, p.2, p.3, p.4, p.5, p.6)))
}

const DIAS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MESES: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

fn texto_data(t: f64) -> String {
    if !t.is_finite() {
        return "Invalid Date".into();
    }
    let p = partes_data(t);
    format!(
        "{} {} {:02} {:04} {:02}:{:02}:{:02} GMT+0000 (Coordinated Universal Time)",
        DIAS[p.7 as usize], MESES[p.1 as usize], p.2, p.0, p.3, p.4, p.5
    )
}

fn data_to_string(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let t = this_data(vm, c)?;
    Ok(Valor::texto(&texto_data(t)))
}

fn data_locale(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let t = this_data(vm, c)?;
    let p = partes_data(t);
    Ok(Valor::texto(&format!("{:02}/{:02}/{:04}, {:02}:{:02}:{:02}", p.2, p.1 + 1, p.0, p.3, p.4, p.5)))
}

fn data_locale_data(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let t = this_data(vm, c)?;
    let p = partes_data(t);
    Ok(Valor::texto(&format!("{:02}/{:02}/{:04}", p.2, p.1 + 1, p.0)))
}

fn data_locale_hora(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let t = this_data(vm, c)?;
    let p = partes_data(t);
    Ok(Valor::texto(&format!("{:02}:{:02}:{:02}", p.3, p.4, p.5)))
}

// ================================================================ iteradores

fn instalar_iteradores(vm: &mut Vm) {
    let pi = vm.p.iterador;
    vm.metodo(pi, "next", iter_next, 0);
    let eu = vm.nativa("[Symbol.iterator]", iter_self, 0);
    vm.heap.get_mut(pi).definir(SIM_ITERADOR, Slot::oculto(eu));
    for (n, f) in [("map", iter_map as super::vm::FnNativa), ("filter", iter_filter), ("toArray", iter_to_array), ("forEach", iter_for_each)] {
        vm.metodo(pi, n, f, 1);
    }
    let pg = vm.p.gerador;
    vm.metodo(pg, "next", ger_next, 1);
    vm.metodo(pg, "return", ger_return, 1);
    vm.metodo(pg, "throw", ger_throw, 1);
    vm.heap.get_mut(pg).definir(SIM_ETIQUETA, Slot::oculto(Valor::texto("Generator")));
}

fn iter_self(_vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(c.this.clone())
}

fn iter_next(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    match vm.proximo(c.this)? {
        Some(v) => Ok(vm.resultado_iter(v, false)),
        None => Ok(vm.resultado_iter(Valor::Indef, true)),
    }
}

fn iter_to_array(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let v = vm.para_vec(c.this)?;
    Ok(vm.novo_array(v))
}

fn iter_map(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let v = vm.para_vec(c.this)?;
    let f = c.arg(0);
    let mut out = Vec::new();
    for (i, x) in v.into_iter().enumerate() {
        out.push(vm.chamar(&f, &Valor::Indef, &[x, Valor::Num(i as f64)])?);
    }
    let a = vm.novo_array(out);
    Ok(iter_de(vm, a, 0))
}

fn iter_filter(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let v = vm.para_vec(c.this)?;
    let f = c.arg(0);
    let mut out = Vec::new();
    for (i, x) in v.into_iter().enumerate() {
        if vm.chamar(&f, &Valor::Indef, &[x.clone(), Valor::Num(i as f64)])?.verdadeiro() {
            out.push(x);
        }
    }
    let a = vm.novo_array(out);
    Ok(iter_de(vm, a, 0))
}

fn iter_for_each(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let v = vm.para_vec(c.this)?;
    let f = c.arg(0);
    for (i, x) in v.into_iter().enumerate() {
        vm.chamar(&f, &Valor::Indef, &[x, Valor::Num(i as f64)])?;
    }
    Ok(Valor::Indef)
}

fn ger(vm: &mut Vm, c: &Ctx, modo: u8) -> R<Valor> {
    let Valor::Obj(g) = c.this else { return vm.erro_tipo("não é gerador") };
    if !matches!(vm.heap.get(*g).classe, Classe::Gerador(_, false) | Classe::Gerador(None, _)) && !matches!(vm.heap.get(*g).classe, Classe::Gerador(..)) {
        return vm.erro_tipo("não é gerador");
    }
    let (v, feito) = vm.retomar_gerador(*g, c.arg(0), modo)?;
    Ok(vm.resultado_iter(v, feito))
}

fn ger_next(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    ger(vm, c, 0)
}

fn ger_return(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    ger(vm, c, 1)
}

fn ger_throw(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    ger(vm, c, 2)
}

// ================================================================ globais

fn instalar_globais(vm: &mut Vm) {
    let g = vm.global;
    vm.definir(g, "NaN", Valor::Num(f64::NAN));
    vm.definir(g, "Infinity", Valor::Num(f64::INFINITY));
    vm.definir(g, "undefined", Valor::Indef);
    vm.metodo(g, "parseInt", glob_parse_int, 2);
    vm.metodo(g, "parseFloat", glob_parse_float, 1);
    vm.metodo(g, "isNaN", glob_is_nan, 1);
    vm.metodo(g, "isFinite", glob_is_finite, 1);
    vm.metodo(g, "encodeURIComponent", glob_encode_component, 1);
    vm.metodo(g, "encodeURI", glob_encode_uri, 1);
    vm.metodo(g, "decodeURIComponent", glob_decode, 1);
    vm.metodo(g, "decodeURI", glob_decode, 1);
    vm.metodo(g, "escape", glob_encode_component, 1);
    vm.metodo(g, "unescape", glob_decode, 1);
    vm.metodo(g, "queueMicrotask", glob_queue_microtask, 1);
    vm.metodo(g, "structuredClone", glob_structured_clone, 1);
    vm.metodo(g, "eval", glob_eval, 1);
    let po = vm.p.objeto;
    let console = obj_com(vm, po);
    vm.definir(g, "console", Valor::Obj(console));
    for n in ["log", "info", "warn", "error", "debug", "trace", "dir", "table", "group", "groupCollapsed"] {
        vm.metodo(console, n, console_log, 0);
    }
    for n in ["groupEnd", "time", "timeEnd", "count", "assert", "clear"] {
        vm.metodo(console, n, nada, 0);
    }
}

fn glob_parse_int(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = texto(vm, c, 0)?;
    let s = s.trim_start_matches(eh_espaco_js);
    let (neg, s) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let mut base = int_arg(vm, c, 1, 0.0)? as u32;
    let mut s = s;
    if (base == 0 || base == 16) && (s.starts_with("0x") || s.starts_with("0X")) {
        s = &s[2..];
        base = 16;
    }
    if base == 0 {
        base = 10;
    }
    if !(2..=36).contains(&base) {
        return Ok(Valor::Num(f64::NAN));
    }
    let mut v = 0f64;
    let mut algum = false;
    for ch in s.chars() {
        match ch.to_digit(base) {
            Some(d) => {
                v = v * base as f64 + d as f64;
                algum = true;
            }
            None => break,
        }
    }
    if !algum {
        return Ok(Valor::Num(f64::NAN));
    }
    Ok(Valor::Num(if neg { -v } else { v }))
}

fn glob_parse_float(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = texto(vm, c, 0)?;
    let s = s.trim_start_matches(eh_espaco_js);
    if s.starts_with("Infinity") || s.starts_with("+Infinity") {
        return Ok(Valor::Num(f64::INFINITY));
    }
    if s.starts_with("-Infinity") {
        return Ok(Valor::Num(f64::NEG_INFINITY));
    }
    // o maior prefixo que e numero
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            i = j;
        }
    }
    Ok(Valor::Num(s[..i].parse::<f64>().unwrap_or(f64::NAN)))
}

fn glob_is_nan(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(Valor::Bool(num(vm, c, 0)?.is_nan()))
}

fn glob_is_finite(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    Ok(Valor::Bool(num(vm, c, 0)?.is_finite()))
}

fn codificar(s: &str, manter: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || manter.as_bytes().contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn glob_encode_component(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = texto(vm, c, 0)?;
    Ok(Valor::texto(&codificar(&s, "-_.!~*'()")))
}

fn glob_encode_uri(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = texto(vm, c, 0)?;
    Ok(Valor::texto(&codificar(&s, "-_.!~*'();/?:@&=+$,#")))
}

fn glob_decode(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = texto(vm, c, 0)?;
    let b = s.as_bytes();
    let mut v = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() + 0 && i + 2 <= b.len() - 1 {
            if let Ok(x) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                v.push(x);
                i += 3;
                continue;
            }
        }
        v.push(b[i]);
        i += 1;
    }
    match String::from_utf8(v) {
        Ok(t) => Ok(Valor::texto(&t)),
        Err(_) => {
            let p = vm.p.erro;
            Err(vm.novo_erro(p, "URI malformada"))
        }
    }
}

fn glob_queue_microtask(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    vm.microtarefas.push_back(super::vm::Tarefa::Chamar(c.arg(0), alloc::vec![Valor::Indef]));
    Ok(Valor::Indef)
}

fn clonar(vm: &mut Vm, v: &Valor, prof: u32) -> R<Valor> {
    if prof > 100 {
        return Ok(v.clone());
    }
    match v {
        Valor::Obj(r) => {
            if vm.eh_array(v) {
                let itens = itens_de(vm, v)?;
                let mut out = Vec::new();
                for x in itens {
                    out.push(clonar(vm, &x, prof + 1)?);
                }
                return Ok(vm.novo_array(out));
            }
            if vm.eh_chamavel(v) {
                return vm.erro_tipo("structuredClone: função não clona");
            }
            let novo = vm.novo_objeto();
            let nr = novo.obj().unwrap_or(0);
            let ks = vm.chaves_proprias(*r, true);
            for k in ks {
                let ch = vm.chave_de(&k)?;
                let x = vm.pegar(v, ch)?;
                let x = clonar(vm, &x, prof + 1)?;
                vm.definir_propria(nr, ch, x);
            }
            Ok(novo)
        }
        x => Ok(x.clone()),
    }
}

fn glob_structured_clone(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    clonar(vm, &c.arg(0), 0)
}

fn glob_eval(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    match c.arg(0) {
        Valor::Texto(s) => super::avaliar(vm, &s),
        v => Ok(v),
    }
}

fn console_log(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let mut partes = Vec::new();
    for a in c.args {
        partes.push(vm.mostrar(a, 0));
    }
    vm.console.push_str(&partes.join(" "));
    vm.console.push('\n');
    if vm.console.len() > 1 << 20 {
        let corte = vm.console.len() - (1 << 19);
        vm.console = vm.console[corte..].to_string();
    }
    Ok(Valor::Indef)
}

// ================================================================ Reflect / Proxy

fn instalar_reflect(vm: &mut Vm) {
    let po = vm.p.objeto;
    let r = obj_com(vm, po);
    let g = vm.global;
    vm.definir(g, "Reflect", Valor::Obj(r));
    vm.metodo(r, "ownKeys", refl_own_keys, 1);
    vm.metodo(r, "get", refl_get, 2);
    vm.metodo(r, "set", refl_set, 3);
    vm.metodo(r, "has", refl_has, 2);
    vm.metodo(r, "apply", refl_apply, 3);
    vm.metodo(r, "construct", refl_construct, 2);
    vm.metodo(r, "defineProperty", obj_define_property, 3);
    vm.metodo(r, "deleteProperty", refl_delete, 2);
    vm.metodo(r, "getPrototypeOf", obj_get_proto, 1);
    vm.metodo(r, "setPrototypeOf", refl_set_proto, 2);
    vm.metodo(r, "getOwnPropertyDescriptor", obj_descriptor, 2);
    let p = vm.nativa("Proxy", proxy_ctor, 2);
    vm.definir(g, "Proxy", p);
}

fn refl_own_keys(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let r = vm.para_objeto(&c.arg(0))?;
    let mut k = vm.chaves_proprias(r, false);
    let simbolos: Vec<Valor> = vm.heap.get(r).props.iter().filter(|(x, _)| x & SIMBOLO != 0).map(|(x, _)| Valor::Simbolo(*x)).collect();
    k.extend(simbolos);
    Ok(vm.novo_array(k))
}

fn refl_get(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    vm.pegar_idx(&c.arg(0), &c.arg(1))
}

fn refl_set(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    vm.por_idx(&c.arg(0), &c.arg(1), c.arg(2))?;
    Ok(Valor::Bool(true))
}

fn refl_has(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let Valor::Obj(r) = c.arg(0) else { return vm.erro_tipo("Reflect.has em não-objeto") };
    let ch = vm.chave_de(&c.arg(1))?;
    Ok(Valor::Bool(vm.tem_propriedade(r, ch)))
}

fn refl_apply(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let args = itens_de(vm, &c.arg(2))?;
    vm.chamar(&c.arg(0), &c.arg(1), &args)
}

fn refl_construct(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let args = itens_de(vm, &c.arg(1))?;
    let alvo = if c.args.len() > 2 { c.arg(2) } else { c.arg(0) };
    vm.construir(&c.arg(0), &args, &alvo)
}

fn refl_delete(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let Valor::Obj(r) = c.arg(0) else { return Ok(Valor::Bool(false)) };
    let ch = vm.chave_de(&c.arg(1))?;
    Ok(Valor::Bool(vm.heap.get_mut(r).remover(ch)))
}

fn refl_set_proto(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    obj_set_proto(vm, c)?;
    Ok(Valor::Bool(true))
}

fn proxy_ctor(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let alvo = c.arg(0);
    let proto = match &alvo {
        Valor::Obj(r) => vm.heap.get(*r).proto,
        _ => return vm.erro_tipo("Proxy precisa de objeto"),
    };
    Ok(Valor::Obj(vm.alocar(proto, Classe::Proxy(alvo, c.arg(1)))))
}
