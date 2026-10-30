//! A ponte entre o JavaScript e a pagina: document, elementos, eventos,
//! timers, fetch/XMLHttpRequest, localStorage, location, navigator...
//! Os nos do DOM viram objetos JS (Classe::Dom(id)) criados sob demanda
//! e guardados (document.body === document.body). Propriedades dos nos
//! sao resolvidas aqui (Anfitriao::pegar/por); metodos ficam nos
//! prototipos como funcoes nativas. O que e mais simples em JS esta no
//! PRELUDIO no fim do arquivo.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::any::Any;
use core::cell::RefCell;

use super::valor::*;
use super::vm::{Anfitriao, Ctx, R, Vm};
use crate::web::atomo::{Atomo, a};
use crate::web::dom::{Documento, Id, NENHUM, RAIZ, Tipo};

/// Alvo de eventos: um no ou a janela.
pub const JANELA: Id = u32::MAX - 7;

pub struct Ouvinte {
    pub alvo: Id,
    pub tipo: Rc<str>,
    pub f: Valor,
    pub captura: bool,
    pub uma_vez: bool,
    /// definido por el.onclick = f (um so por tipo)
    pub propriedade: bool,
}

pub struct Timer {
    pub id: u32,
    pub quando: f64,
    pub f: Valor,
    pub args: Vec<Valor>,
    pub intervalo: Option<f64>,
}

pub struct Pedido {
    pub metodo: String,
    pub url: String,
    pub corpo: Option<String>,
    pub cabecalhos: Vec<(String, String)>,
    /// fetch: a promessa; XHR: o objeto
    pub promessa: Valor,
    pub xhr: Valor,
}

#[derive(Default)]
pub struct Protos {
    pub no: Ref,
    pub elemento: Ref,
    pub texto: Ref,
    pub documento: Ref,
    pub lista_classes: Ref,
    pub estilo: Ref,
}

pub struct Dom {
    pub doc: Rc<RefCell<Documento>>,
    pub objetos: BTreeMap<Id, Ref>,
    pub ouvintes: Vec<Ouvinte>,
    pub timers: Vec<Timer>,
    pub prox_timer: u32,
    pub pedidos: Vec<Pedido>,
    /// O DOM mudou (precisa refazer estilo e layout).
    pub sujo: bool,
    pub url: String,
    pub armazenamento: BTreeMap<String, String>,
    pub cookies: BTreeMap<String, String>,
    pub geometria: BTreeMap<Id, (f32, f32, f32, f32)>,
    pub p: Protos,
    pub agora: f64,
    pub largura: f32,
    pub altura: f32,
    pub rolagem: f32,
    /// location.href = ... pede navegacao
    pub navegar: Option<String>,
    pub foco: Id,
    pub script_atual: Id,
    pub escrito: String,
    /// <template> -> fragmento do content
    pub conteudos: BTreeMap<Id, Id>,
}

impl Anfitriao for Dom {
    fn pegar(&mut self, vm: &mut Vm, no: u32, c: Chave) -> Option<R<Valor>> {
        propriedade(self, vm, no, nome_da_chave(c))
    }
    fn por(&mut self, vm: &mut Vm, no: u32, c: Chave, v: &Valor) -> Option<R<()>> {
        definir(self, vm, no, nome_da_chave(c), v)
    }
    fn raizes(&self, vs: &mut Vec<Valor>) {
        vs.extend(self.objetos.values().map(|r| Valor::Obj(*r)));
        for o in &self.ouvintes {
            vs.push(o.f.clone());
        }
        for t in &self.timers {
            vs.push(t.f.clone());
            vs.extend(t.args.iter().cloned());
        }
        for p in &self.pedidos {
            vs.push(p.promessa.clone());
            vs.push(p.xhr.clone());
        }
        let p = &self.p;
        for r in [p.no, p.elemento, p.texto, p.documento, p.lista_classes, p.estilo] {
            vs.push(Valor::Obj(r));
        }
    }
    fn como_any(&mut self) -> &mut dyn Any {
        self
    }
}

/// Roda `f` com o Dom (tira da VM e devolve depois).
pub fn com_dom<T>(vm: &mut Vm, f: impl FnOnce(&mut Vm, &mut Dom) -> T) -> Option<T> {
    let mut h = vm.anfitriao.take()?;
    let r = h.como_any().downcast_mut::<Dom>().map(|d| f(vm, d));
    vm.anfitriao = Some(h);
    r
}

fn no_de(vm: &Vm, v: &Valor) -> Option<Id> {
    match v {
        Valor::Obj(r) => match vm.heap.get(*r).classe {
            Classe::Dom(id) => Some(id),
            _ => None,
        },
        _ => None,
    }
}

fn this_no(vm: &mut Vm, c: &Ctx) -> R<Id> {
    match no_de(vm, c.this) {
        Some(id) => Ok(id),
        None => {
            let d = vm.descrever_curto(c.this);
            vm.erro_tipo(&format!("this não é um nó do DOM ({d})"))
        }
    }
}

/// O objeto JS de um no (criado na primeira vez).
pub fn objeto(vm: &mut Vm, d: &mut Dom, id: Id) -> Valor {
    if id == NENHUM {
        return Valor::Nulo;
    }
    if let Some(r) = d.objetos.get(&id) {
        return Valor::Obj(*r);
    }
    let proto = {
        let doc = d.doc.borrow();
        match doc.nos.get(id as usize).map(|n| &n.tipo) {
            Some(Tipo::Elemento(_)) => d.p.elemento,
            Some(Tipo::Documento) => d.p.documento,
            Some(Tipo::Texto(_)) | Some(Tipo::Comentario(_)) => d.p.texto,
            _ => d.p.no,
        }
    };
    let r = vm.alocar(Some(proto), Classe::Dom(id));
    d.objetos.insert(id, r);
    Valor::Obj(r)
}

fn lista(vm: &mut Vm, d: &mut Dom, ids: Vec<Id>) -> Valor {
    let v: Vec<Valor> = ids.into_iter().map(|i| objeto(vm, d, i)).collect();
    vm.novo_array(v)
}

fn texto_valor(vm: &mut Vm, v: &Valor) -> R<String> {
    Ok(if v.eh_nulo_ou_indef() { String::new() } else { String::from(&*vm.para_texto(v)?) })
}

/// Nomes de atributo que viram propriedades com o mesmo nome.
fn atributo_refletido(nome: &str) -> Option<&'static str> {
    Some(match nome {
        "id" => "id",
        "title" => "title",
        "lang" => "lang",
        "name" => "name",
        "type" => "type",
        "alt" => "alt",
        "placeholder" => "placeholder",
        "rel" => "rel",
        "target" => "target",
        "role" => "role",
        "content" => "content",
        "htmlFor" => "for",
        "tabIndex" => "tabindex",
        "width" => "width",
        "height" => "height",
        "action" => "action",
        "method" => "method",
        "dir" => "dir",
        "srcset" => "srcset",
        "media" => "media",
        _ => return None,
    })
}

const BOOLEANOS: &[&str] = &["hidden", "disabled", "checked", "selected", "readonly", "required", "multiple", "autofocus", "async", "defer", "open"];

fn propriedade(d: &mut Dom, vm: &mut Vm, no: Id, nome: &str) -> Option<R<Valor>> {
    let doc_rc = d.doc.clone();
    // propriedades comuns a todos os nos
    let (pai, prox, ant, primeiro, ultimo, tipo_no) = {
        let doc = doc_rc.borrow();
        let n = doc.nos.get(no as usize)?;
        let tipo = match &n.tipo {
            Tipo::Elemento(_) => 1,
            Tipo::Texto(_) => 3,
            Tipo::Comentario(_) => 8,
            Tipo::Documento => 9,
            Tipo::Doctype(_) => 10,
        };
        (n.pai, n.proximo, n.anterior, n.primeiro, n.ultimo, tipo)
    };
    let elem_rel = |doc: &Documento, mut x: Id, para_frente: bool| -> Id {
        while x != NENHUM {
            if doc.elemento(x).is_some() {
                return x;
            }
            x = if para_frente { doc.no(x).proximo } else { doc.no(x).anterior };
        }
        NENHUM
    };
    let v = match nome {
        "nodeType" => Valor::Num(tipo_no as f64),
        "parentNode" => objeto(vm, d, pai),
        "parentElement" => {
            let eh_elem = doc_rc.borrow().elemento(pai).is_some();
            if eh_elem { objeto(vm, d, pai) } else { Valor::Nulo }
        }
        "nextSibling" => objeto(vm, d, prox),
        "previousSibling" => objeto(vm, d, ant),
        "firstChild" => objeto(vm, d, primeiro),
        "lastChild" => objeto(vm, d, ultimo),
        "nextElementSibling" => {
            let x = elem_rel(&doc_rc.borrow(), prox, true);
            objeto(vm, d, x)
        }
        "previousElementSibling" => {
            let x = elem_rel(&doc_rc.borrow(), ant, false);
            objeto(vm, d, x)
        }
        "firstElementChild" => {
            let x = elem_rel(&doc_rc.borrow(), primeiro, true);
            objeto(vm, d, x)
        }
        "lastElementChild" => {
            let x = elem_rel(&doc_rc.borrow(), ultimo, false);
            objeto(vm, d, x)
        }
        "childNodes" => {
            let ids: Vec<Id> = doc_rc.borrow().filhos(no).collect();
            lista(vm, d, ids)
        }
        "children" => {
            let ids: Vec<Id> = {
                let doc = doc_rc.borrow();
                doc.filhos(no).filter(|&f| doc.elemento(f).is_some()).collect()
            };
            lista(vm, d, ids)
        }
        "childElementCount" => {
            let doc = doc_rc.borrow();
            Valor::Num(doc.filhos(no).filter(|&f| doc.elemento(f).is_some()).count() as f64)
        }
        "ownerDocument" => objeto(vm, d, RAIZ),
        "baseURI" => Valor::texto(&d.url),
        "isConnected" => Valor::Bool(doc_rc.borrow().dentro_de(no, RAIZ)),
        "textContent" | "innerText" | "nodeValue" | "data" | "wholeText" => {
            if tipo_no == 9 && nome == "textContent" {
                Valor::Nulo
            } else {
                Valor::texto(&doc_rc.borrow().texto(no))
            }
        }
        "nodeName" | "tagName" | "localName" => {
            let doc = doc_rc.borrow();
            match &doc.no(no).tipo {
                Tipo::Elemento(e) => {
                    let n = e.nome.texto();
                    if nome == "localName" { Valor::texto(n) } else { Valor::texto(&n.to_ascii_uppercase()) }
                }
                Tipo::Texto(_) => Valor::texto("#text"),
                Tipo::Comentario(_) => Valor::texto("#comment"),
                Tipo::Documento => Valor::texto("#document"),
                Tipo::Doctype(n) => Valor::texto(n),
            }
        }
        "length" if tipo_no == 3 => Valor::Num(doc_rc.borrow().texto(no).encode_utf16().count() as f64),
        _ if tipo_no == 9 => return propriedade_documento(d, vm, nome),
        _ if tipo_no != 1 => return None,
        // --- elementos
        "id" | "className" => {
            let doc = doc_rc.borrow();
            let e = doc.elemento(no)?;
            Valor::texto(e.atributo(if nome == "id" { a::ID } else { a::CLASS }).unwrap_or(""))
        }
        "innerHTML" | "outerHTML" => {
            let mut s = String::new();
            doc_rc.borrow().serializar(no, nome == "outerHTML", &mut s);
            Valor::texto(&s)
        }
        "classList" => {
            let pl = d.p.lista_classes;
            let r = vm.alocar(Some(pl), Classe::Comum);
            let o = objeto(vm, d, no);
            vm.definir(r, "_el", o);
            Valor::Obj(r)
        }
        "style" => {
            // um Proxy (PRELUDIO) que le e escreve o atributo style
            let o = objeto(vm, d, no);
            let g = Valor::Obj(vm.global);
            let f = match vm.pegar(&g, chave("__estilo")) {
                Ok(f) => f,
                Err(e) => return Some(Err(e)),
            };
            return Some(vm.chamar(&f, &g, &[o]));
        }
        "dataset" => {
            let r = vm.alocar(Some(vm.p.objeto), Classe::Comum);
            let attrs: Vec<(String, String)> = doc_rc
                .borrow()
                .elemento(no)
                .map(|e| e.atributos.iter().filter_map(|at| at.nome.texto().strip_prefix("data-").map(|n| (camel_case(n), at.valor.clone()))).collect())
                .unwrap_or_default();
            for (k, v) in attrs {
                vm.heap.get_mut(r).definir(chave(&k), Slot::valor(Valor::texto(&v)));
            }
            Valor::Obj(r)
        }
        "attributes" => {
            let attrs: Vec<(String, String)> =
                doc_rc.borrow().elemento(no).map(|e| e.atributos.iter().map(|at| (String::from(at.nome.texto()), at.valor.clone())).collect()).unwrap_or_default();
            let v: Vec<Valor> = attrs
                .into_iter()
                .map(|(n, val)| {
                    let o = vm.novo_objeto();
                    if let Valor::Obj(r) = o {
                        vm.definir_propria(r, const { crate::web::atomo::fixo("name").0 }, Valor::texto(&n));
                        vm.definir_propria(r, const { crate::web::atomo::fixo("value").0 }, Valor::texto(&val));
                    }
                    o
                })
                .collect();
            vm.novo_array(v)
        }
        "href" | "src" | "action" => {
            let doc = doc_rc.borrow();
            let e = doc.elemento(no)?;
            let at = e.atributo(Atomo::de(nome))?;
            Valor::texto(&crate::web::url::resolver(&d.url, at))
        }
        "value" => {
            let doc = doc_rc.borrow();
            let e = doc.elemento(no)?;
            if e.eh(a::TEXTAREA) {
                Valor::texto(&doc.texto(no))
            } else if e.eh(a::SELECT) {
                let sel = doc.descendentes(no).filter(|&o| doc.eh(o, a::OPTION)).find(|&o| doc.elemento(o).is_some_and(|x| x.tem(a::SELECTED)));
                let sel = sel.or_else(|| doc.descendentes(no).find(|&o| doc.eh(o, a::OPTION)));
                match sel {
                    Some(o) => Valor::texto(doc.elemento(o).and_then(|x| x.atributo(a::VALUE)).map(String::from).unwrap_or_else(|| doc.texto(o)).trim()),
                    None => Valor::texto(""),
                }
            } else {
                Valor::texto(e.atributo(a::VALUE).unwrap_or(if e.eh(a::OPTION) { "" } else { "" }))
            }
        }
        n if BOOLEANOS.contains(&n) => Valor::Bool(doc_rc.borrow().elemento(no)?.tem(Atomo::de(n))),
        "offsetWidth" | "clientWidth" | "scrollWidth" | "offsetHeight" | "clientHeight" | "scrollHeight" | "offsetTop" | "offsetLeft" => {
            let g = d.geometria.get(&no).copied().unwrap_or((0.0, 0.0, 0.0, 0.0));
            Valor::Num(match nome {
                "offsetWidth" | "clientWidth" | "scrollWidth" => g.2,
                "offsetHeight" | "clientHeight" | "scrollHeight" => g.3,
                "offsetTop" => g.1,
                _ => g.0,
            } as f64)
        }
        "scrollTop" | "scrollLeft" => Valor::Num(0.0),
        "offsetParent" => objeto(vm, d, pai),
        "form" => {
            let mut x = pai;
            let doc = doc_rc.borrow();
            while x != NENHUM && !doc.eh(x, a::FORM) {
                x = doc.pai(x);
            }
            drop(doc);
            objeto(vm, d, x)
        }
        "naturalWidth" | "naturalHeight" => Valor::Num(0.0),
        "complete" => Valor::Bool(true),
        "options" => {
            let ids: Vec<Id> = {
                let doc = doc_rc.borrow();
                doc.descendentes(no).filter(|&o| doc.eh(o, a::OPTION)).collect()
            };
            lista(vm, d, ids)
        }
        "selectedIndex" => {
            let doc = doc_rc.borrow();
            let ops: Vec<Id> = doc.descendentes(no).filter(|&o| doc.eh(o, a::OPTION)).collect();
            Valor::Num(ops.iter().position(|&o| doc.elemento(o).is_some_and(|x| x.tem(a::SELECTED))).map_or(0.0, |p| p as f64))
        }
        "shadowRoot" => Valor::Nulo,
        "content" if doc_rc.borrow().eh(no, a::TEMPLATE) => {
            // o conteudo de um <template>: um fragmento com os filhos dele
            let frag = match d.conteudos.get(&no) {
                Some(&f) => f,
                None => {
                    let mut doc = doc_rc.borrow_mut();
                    let f = doc.criar_elemento(Atomo::de("#fragment"), crate::web::dom::Espaco::Html, Vec::new());
                    let filhos: Vec<Id> = doc.filhos(no).collect();
                    for x in filhos {
                        doc.anexar(f, x);
                    }
                    drop(doc);
                    d.conteudos.insert(no, f);
                    f
                }
            };
            objeto(vm, d, frag)
        }
        "contentWindow" | "contentDocument" => Valor::Nulo,
        n if n.starts_with("on") => {
            // handler definido por propriedade
            let alvo = no;
            let tipo = &n[2..];
            d.ouvintes.iter().find(|o| o.alvo == alvo && &*o.tipo == tipo && o.propriedade).map(|o| o.f.clone()).unwrap_or(Valor::Nulo)
        }
        n => {
            let at = atributo_refletido(n)?;
            let doc = doc_rc.borrow();
            Valor::texto(doc.elemento(no)?.atributo(Atomo::de(at)).unwrap_or(""))
        }
    };
    Some(Ok(v))
}

fn propriedade_documento(d: &mut Dom, vm: &mut Vm, nome: &str) -> Option<R<Valor>> {
    let doc_rc = d.doc.clone();
    let v = match nome {
        "body" => {
            let b = doc_rc.borrow().corpo().unwrap_or(NENHUM);
            objeto(vm, d, b)
        }
        "head" => {
            let h = doc_rc.borrow().cabeca().unwrap_or(NENHUM);
            objeto(vm, d, h)
        }
        "documentElement" => {
            let h = doc_rc.borrow().html().unwrap_or(NENHUM);
            objeto(vm, d, h)
        }
        "title" => Valor::texto(&doc_rc.borrow().titulo()),
        "readyState" => Valor::texto("complete"),
        "visibilityState" => Valor::texto("visible"),
        "hidden" => Valor::Bool(false),
        "cookie" => Valor::texto(&d.cookies.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; ")),
        "URL" | "documentURI" => Valor::texto(&d.url),
        "referrer" => Valor::texto(""),
        "characterSet" | "charset" => Valor::texto("UTF-8"),
        "compatMode" => Valor::texto("CSS1Compat"),
        "currentScript" => objeto(vm, d, d.script_atual),
        "activeElement" => {
            let f = if d.foco == NENHUM { doc_rc.borrow().corpo().unwrap_or(NENHUM) } else { d.foco };
            objeto(vm, d, f)
        }
        "scrollingElement" => {
            let h = doc_rc.borrow().html().unwrap_or(NENHUM);
            objeto(vm, d, h)
        }
        "forms" | "images" | "links" | "scripts" => {
            let alvo = match nome {
                "forms" => a::FORM,
                "images" => a::IMG,
                "links" => a::A,
                _ => a::SCRIPT,
            };
            let ids: Vec<Id> = {
                let doc = doc_rc.borrow();
                doc.descendentes(RAIZ).filter(|&x| doc.eh(x, alvo)).collect()
            };
            lista(vm, d, ids)
        }
        _ => return None,
    };
    Some(Ok(v))
}

pub fn camel_case(s: &str) -> String {
    let mut out = String::new();
    let mut maiuscula = false;
    for c in s.chars() {
        if c == '-' {
            maiuscula = true;
        } else if maiuscula {
            out.extend(c.to_uppercase());
            maiuscula = false;
        } else {
            out.push(c);
        }
    }
    out
}

pub fn kebab_case(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_uppercase() {
            out.push('-');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn por_atributo(d: &mut Dom, no: Id, nome: &str, valor: Option<&str>) {
    let mut doc = d.doc.borrow_mut();
    if let Some(e) = doc.elemento_mut(no) {
        match valor {
            Some(v) => e.definir(Atomo::minusculo(nome), v),
            None => e.tirar(Atomo::minusculo(nome)),
        }
        d.sujo = true;
    }
}

/// Troca os filhos de `no` por um texto.
fn por_texto(d: &mut Dom, no: Id, t: &str) {
    let mut doc = d.doc.borrow_mut();
    let filhos: Vec<Id> = doc.filhos(no).collect();
    for f in filhos {
        doc.soltar(f);
    }
    if !t.is_empty() {
        doc.anexar_texto(no, t);
    }
    d.sujo = true;
}

fn definir(d: &mut Dom, vm: &mut Vm, no: Id, nome: &str, v: &Valor) -> Option<R<()>> {
    let tipo_elem = d.doc.borrow().elemento(no).is_some();
    let r = (|| -> R<bool> {
        match nome {
            "textContent" | "innerText" | "nodeValue" | "data" => {
                let t = texto_valor(vm, v)?;
                let eh_texto = matches!(d.doc.borrow().nos.get(no as usize).map(|n| &n.tipo), Some(Tipo::Texto(_)) | Some(Tipo::Comentario(_)));
                if eh_texto {
                    if let Some(n) = d.doc.borrow_mut().nos.get_mut(no as usize) {
                        n.tipo = match n.tipo {
                            Tipo::Comentario(_) => Tipo::Comentario(t),
                            _ => Tipo::Texto(t),
                        };
                    }
                    d.sujo = true;
                } else {
                    por_texto(d, no, &t);
                }
                Ok(true)
            }
            "title" if no == RAIZ => {
                let t = texto_valor(vm, v)?;
                let titulo = { d.doc.borrow().achar(RAIZ, a::TITLE) };
                if let Some(ti) = titulo {
                    por_texto(d, ti, &t);
                }
                Ok(true)
            }
            "cookie" if no == RAIZ => {
                let t = texto_valor(vm, v)?;
                if let Some((k, val)) = t.split(';').next().and_then(|p| p.split_once('=')) {
                    d.cookies.insert(String::from(k.trim()), String::from(val.trim()));
                }
                Ok(true)
            }
            _ if !tipo_elem => Ok(false),
            "innerHTML" | "outerHTML" => {
                let html = texto_valor(vm, v)?;
                let mut doc = d.doc.borrow_mut();
                let novos = doc.fragmento(&html);
                if nome == "innerHTML" {
                    let filhos: Vec<Id> = doc.filhos(no).collect();
                    for f in filhos {
                        doc.soltar(f);
                    }
                    for n in novos {
                        doc.anexar(no, n);
                    }
                } else {
                    let pai = doc.pai(no);
                    if pai != NENHUM {
                        for n in novos {
                            doc.inserir_antes(pai, n, no);
                        }
                        doc.soltar(no);
                    }
                }
                d.sujo = true;
                Ok(true)
            }
            "id" => {
                let t = texto_valor(vm, v)?;
                por_atributo(d, no, "id", Some(&t));
                Ok(true)
            }
            "className" => {
                let t = texto_valor(vm, v)?;
                por_atributo(d, no, "class", Some(&t));
                Ok(true)
            }
            "value" => {
                let t = texto_valor(vm, v)?;
                let eh_area = d.doc.borrow().eh(no, a::TEXTAREA);
                if eh_area {
                    por_texto(d, no, &t);
                } else {
                    por_atributo(d, no, "value", Some(&t));
                }
                Ok(true)
            }
            "href" | "src" | "action" => {
                let t = texto_valor(vm, v)?;
                por_atributo(d, no, nome, Some(&t));
                Ok(true)
            }
            n if BOOLEANOS.contains(&n) => {
                por_atributo(d, no, n, if v.verdadeiro() { Some("") } else { None });
                Ok(true)
            }
            "style" => {
                let t = texto_valor(vm, v)?;
                por_atributo(d, no, "style", Some(&t));
                Ok(true)
            }
            n if n.starts_with("on") && n.len() > 2 => {
                let tipo: Rc<str> = Rc::from(&n[2..]);
                d.ouvintes.retain(|o| !(o.alvo == no && o.tipo == tipo && o.propriedade));
                if vm.eh_chamavel(v) {
                    d.ouvintes.push(Ouvinte { alvo: no, tipo, f: v.clone(), captura: false, uma_vez: false, propriedade: true });
                }
                Ok(true)
            }
            n => match atributo_refletido(n) {
                Some(at) => {
                    let t = texto_valor(vm, v)?;
                    por_atributo(d, no, at, Some(&t));
                    Ok(true)
                }
                None => Ok(false),
            },
        }
    })();
    match r {
        Ok(true) => Some(Ok(())),
        Ok(false) => None,
        Err(e) => Some(Err(e)),
    }
}

// ------------------------------------------------------------ metodos

macro_rules! dom {
    ($vm:ident, |$d:ident| $corpo:expr) => {
        match com_dom($vm, |$vm, $d| {
            let _ = &$vm;
            $corpo
        }) {
            Some(x) => x,
            None => Ok(Valor::Indef),
        }
    };
}

fn no_arg(vm: &mut Vm, v: &Valor) -> R<Id> {
    match no_de(vm, v) {
        Some(i) => Ok(i),
        None => vm.erro_tipo("o argumento não é um nó"),
    }
}

/// Seletores CSS sobre o documento (querySelector[All]).
fn consultar(d: &Dom, raiz: Id, sel: &str, so_um: bool) -> Result<Vec<Id>, &'static str> {
    use crate::web::css::seletor::{Casador, Estado, ler_lista};
    let t = crate::web::css::token::tokens(sel);
    let lista = ler_lista(&t).ok_or("seletor inválido")?;
    let doc = d.doc.borrow();
    let cas = Casador::novo(&doc, Estado::default());
    let mut v = Vec::new();
    for id in doc.descendentes(raiz) {
        if doc.elemento(id).is_none() {
            continue;
        }
        if lista.iter().any(|s| cas.casa(s, id)) {
            v.push(id);
            if so_um {
                break;
            }
        }
    }
    Ok(v)
}

fn el_query(vm: &mut Vm, c: &Ctx, so_um: bool) -> R<Valor> {
    let raiz = this_no(vm, c)?;
    let sel = vm.para_texto(&c.arg(0))?;
    dom!(vm, |d| {
        match consultar(d, raiz, &sel, so_um) {
            Ok(ids) => {
                if so_um {
                    Ok(match ids.first() {
                        Some(&i) => objeto(vm, d, i),
                        None => Valor::Nulo,
                    })
                } else {
                    Ok(lista(vm, d, ids))
                }
            }
            Err(e) => {
                let p = vm.p.erro_sintaxe;
                Err(vm.novo_erro(p, e))
            }
        }
    })
}

fn el_query_selector(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    el_query(vm, c, true)
}

fn el_query_selector_all(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    el_query(vm, c, false)
}

fn el_by_tag(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let raiz = this_no(vm, c)?;
    let t = vm.para_texto(&c.arg(0))?.to_ascii_lowercase();
    dom!(vm, |d| {
        let ids: Vec<Id> = {
            let doc = d.doc.borrow();
            doc.descendentes(raiz).filter(|&x| doc.elemento(x).is_some_and(|e| t == "*" || e.nome.texto() == t)).collect()
        };
        Ok(lista(vm, d, ids))
    })
}

fn el_by_class(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let raiz = this_no(vm, c)?;
    let t = vm.para_texto(&c.arg(0))?;
    let classes: Vec<Atomo> = t.split_ascii_whitespace().map(Atomo::de).collect();
    dom!(vm, |d| {
        let ids: Vec<Id> = {
            let doc = d.doc.borrow();
            doc.descendentes(raiz).filter(|&x| doc.elemento(x).is_some_and(|e| classes.iter().all(|c| e.classes.contains(c)))).collect()
        };
        Ok(lista(vm, d, ids))
    })
}

fn doc_by_id(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let id = vm.para_texto(&c.arg(0))?;
    dom!(vm, |d| {
        let achado = d.doc.borrow().por_id(&id).unwrap_or(NENHUM);
        Ok(objeto(vm, d, achado))
    })
}

fn doc_by_name(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let n = vm.para_texto(&c.arg(0))?;
    dom!(vm, |d| {
        let ids: Vec<Id> = {
            let doc = d.doc.borrow();
            doc.descendentes(RAIZ).filter(|&x| doc.elemento(x).is_some_and(|e| e.atributo(a::NAME) == Some(&n))).collect()
        };
        Ok(lista(vm, d, ids))
    })
}

fn doc_create_element(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let n = vm.para_texto(&c.arg(0))?.to_ascii_lowercase();
    dom!(vm, |d| {
        let id = d.doc.borrow_mut().criar_elemento(Atomo::de(&n), crate::web::dom::Espaco::Html, Vec::new());
        Ok(objeto(vm, d, id))
    })
}

fn doc_create_element_ns(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let ns = vm.para_texto(&c.arg(0))?;
    let n = vm.para_texto(&c.arg(1))?;
    let espaco = if ns.contains("svg") { crate::web::dom::Espaco::Svg } else { crate::web::dom::Espaco::Html };
    dom!(vm, |d| {
        let id = d.doc.borrow_mut().criar_elemento(Atomo::de(&n), espaco, Vec::new());
        Ok(objeto(vm, d, id))
    })
}

fn doc_create_text(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let t = texto_valor(vm, &c.arg(0))?;
    dom!(vm, |d| {
        let id = d.doc.borrow_mut().criar(Tipo::Texto(t));
        Ok(objeto(vm, d, id))
    })
}

fn doc_create_comment(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let t = texto_valor(vm, &c.arg(0))?;
    dom!(vm, |d| {
        let id = d.doc.borrow_mut().criar(Tipo::Comentario(t));
        Ok(objeto(vm, d, id))
    })
}

fn doc_create_fragment(vm: &mut Vm, _c: &Ctx) -> R<Valor> {
    dom!(vm, |d| {
        // fragmento: um elemento solto que some ao ser inserido
        let id = d.doc.borrow_mut().criar_elemento(Atomo::de("#fragment"), crate::web::dom::Espaco::Html, Vec::new());
        Ok(objeto(vm, d, id))
    })
}

fn doc_write(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let mut s = String::new();
    for x in c.args {
        s.push_str(&vm.para_texto(x)?);
    }
    dom!(vm, |d| {
        d.escrito.push_str(&s);
        Ok(Valor::Indef)
    })
}

/// Insere `filho` em `pai` antes de `ref_` (fragmentos levam os filhos).
fn inserir(d: &mut Dom, pai: Id, filho: Id, ref_: Id) {
    let mut doc = d.doc.borrow_mut();
    let eh_frag = doc.elemento(filho).is_some_and(|e| e.nome.texto() == "#fragment");
    if eh_frag {
        let filhos: Vec<Id> = doc.filhos(filho).collect();
        for f in filhos {
            doc.inserir_antes(pai, f, ref_);
        }
    } else {
        doc.inserir_antes(pai, filho, ref_);
    }
    d.sujo = true;
}

fn el_append_child(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let pai = this_no(vm, c)?;
    let filho = no_arg(vm, &c.arg(0))?;
    dom!(vm, |d| {
        inserir(d, pai, filho, NENHUM);
        Ok(c.arg(0))
    })
}

fn el_insert_before(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let pai = this_no(vm, c)?;
    let filho = no_arg(vm, &c.arg(0))?;
    let r = no_de(vm, &c.arg(1)).unwrap_or(NENHUM);
    dom!(vm, |d| {
        inserir(d, pai, filho, r);
        Ok(c.arg(0))
    })
}

fn el_remove_child(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let filho = no_arg(vm, &c.arg(0))?;
    dom!(vm, |d| {
        d.doc.borrow_mut().soltar(filho);
        d.sujo = true;
        Ok(c.arg(0))
    })
}

fn el_replace_child(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let pai = this_no(vm, c)?;
    let novo = no_arg(vm, &c.arg(0))?;
    let velho = no_arg(vm, &c.arg(1))?;
    dom!(vm, |d| {
        inserir(d, pai, novo, velho);
        d.doc.borrow_mut().soltar(velho);
        Ok(c.arg(1))
    })
}

fn el_remove(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let no = this_no(vm, c)?;
    dom!(vm, |d| {
        d.doc.borrow_mut().soltar(no);
        d.sujo = true;
        Ok(Valor::Indef)
    })
}

/// append/prepend/before/after/replaceWith: aceitam nos e textos.
fn nos_dos_args(vm: &mut Vm, c: &Ctx) -> R<Vec<Result<Id, String>>> {
    let mut v = Vec::new();
    for x in c.args {
        match no_de(vm, x) {
            Some(i) => v.push(Ok(i)),
            None => v.push(Err(String::from(&*vm.para_texto(x)?))),
        }
    }
    Ok(v)
}

fn colocar(vm: &mut Vm, c: &Ctx, modo: u8) -> R<Valor> {
    let alvo = this_no(vm, c)?;
    let itens = nos_dos_args(vm, c)?;
    dom!(vm, |d| {
        let (pai, ref_) = {
            let doc = d.doc.borrow();
            match modo {
                0 => (alvo, NENHUM),                             // append
                1 => (alvo, doc.no(alvo).primeiro),              // prepend
                2 => (doc.pai(alvo), alvo),                      // before
                _ => (doc.pai(alvo), doc.no(alvo).proximo),      // after / replaceWith
            }
        };
        if pai == NENHUM {
            return Ok(Valor::Indef);
        }
        for it in itens {
            let id = match it {
                Ok(i) => i,
                Err(t) => d.doc.borrow_mut().criar(Tipo::Texto(t)),
            };
            inserir(d, pai, id, ref_);
        }
        if modo == 4 {
            d.doc.borrow_mut().soltar(alvo);
        }
        Ok(Valor::Indef)
    })
}

fn el_append(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    colocar(vm, c, 0)
}
fn el_prepend(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    colocar(vm, c, 1)
}
fn el_before(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    colocar(vm, c, 2)
}
fn el_after(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    colocar(vm, c, 3)
}
fn el_replace_with(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    colocar(vm, c, 4)
}

fn el_clone(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let no = this_no(vm, c)?;
    let profundo = c.arg(0).verdadeiro();
    dom!(vm, |d| {
        let id = d.doc.borrow_mut().clonar(no, profundo);
        Ok(objeto(vm, d, id))
    })
}

fn el_contains(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let no = this_no(vm, c)?;
    let Some(outro) = no_de(vm, &c.arg(0)) else { return Ok(Valor::Bool(false)) };
    dom!(vm, |d| Ok(Valor::Bool(d.doc.borrow().dentro_de(outro, no))))
}

fn el_has_child_nodes(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let no = this_no(vm, c)?;
    dom!(vm, |d| Ok(Valor::Bool(d.doc.borrow().no(no).primeiro != NENHUM)))
}

fn el_get_attribute(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let no = this_no(vm, c)?;
    let n = vm.para_texto(&c.arg(0))?;
    dom!(vm, |d| {
        let doc = d.doc.borrow();
        Ok(match doc.elemento(no).and_then(|e| e.atributo(Atomo::minusculo(&n))) {
            Some(v) => Valor::texto(v),
            None => Valor::Nulo,
        })
    })
}

fn el_set_attribute(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let no = this_no(vm, c)?;
    let n = vm.para_texto(&c.arg(0))?;
    let v = vm.para_texto(&c.arg(1))?;
    dom!(vm, |d| {
        por_atributo(d, no, &n, Some(&v));
        Ok(Valor::Indef)
    })
}

fn el_remove_attribute(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let no = this_no(vm, c)?;
    let n = vm.para_texto(&c.arg(0))?;
    dom!(vm, |d| {
        por_atributo(d, no, &n, None);
        Ok(Valor::Indef)
    })
}

fn el_has_attribute(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let no = this_no(vm, c)?;
    let n = vm.para_texto(&c.arg(0))?;
    dom!(vm, |d| Ok(Valor::Bool(d.doc.borrow().elemento(no).is_some_and(|e| e.tem(Atomo::minusculo(&n))))))
}

fn el_toggle_attribute(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let no = this_no(vm, c)?;
    let n = vm.para_texto(&c.arg(0))?;
    dom!(vm, |d| {
        let tem = d.doc.borrow().elemento(no).is_some_and(|e| e.tem(Atomo::minusculo(&n)));
        let quer = if matches!(c.arg(1), Valor::Indef) { !tem } else { c.arg(1).verdadeiro() };
        por_atributo(d, no, &n, if quer { Some("") } else { None });
        Ok(Valor::Bool(quer))
    })
}

fn el_attribute_names(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let no = this_no(vm, c)?;
    let nomes: Vec<Valor> = com_dom(vm, |_, d| {
        d.doc.borrow().elemento(no).map(|e| e.atributos.iter().map(|at| Valor::texto(at.nome.texto())).collect()).unwrap_or_default()
    })
    .unwrap_or_default();
    Ok(vm.novo_array(nomes))
}

fn el_matches(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let no = this_no(vm, c)?;
    let sel = vm.para_texto(&c.arg(0))?;
    dom!(vm, |d| {
        use crate::web::css::seletor::{Casador, Estado, ler_lista};
        let t = crate::web::css::token::tokens(&sel);
        let Some(lista) = ler_lista(&t) else { return Ok(Valor::Bool(false)) };
        let doc = d.doc.borrow();
        let cas = Casador::novo(&doc, Estado::default());
        Ok(Valor::Bool(lista.iter().any(|s| cas.casa(s, no))))
    })
}

fn el_closest(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let no = this_no(vm, c)?;
    let sel = vm.para_texto(&c.arg(0))?;
    dom!(vm, |d| {
        use crate::web::css::seletor::{Casador, Estado, ler_lista};
        let t = crate::web::css::token::tokens(&sel);
        let Some(lista) = ler_lista(&t) else { return Ok(Valor::Nulo) };
        let achado = {
            let doc = d.doc.borrow();
            let cas = Casador::novo(&doc, Estado::default());
            let mut x = no;
            let mut r = NENHUM;
            while x != NENHUM {
                if doc.elemento(x).is_some() && lista.iter().any(|s| cas.casa(s, x)) {
                    r = x;
                    break;
                }
                x = doc.pai(x);
            }
            r
        };
        Ok(objeto(vm, d, achado))
    })
}

fn el_insert_adjacent_html(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let no = this_no(vm, c)?;
    let onde = vm.para_texto(&c.arg(0))?.to_ascii_lowercase();
    let html = vm.para_texto(&c.arg(1))?;
    dom!(vm, |d| {
        let novos = d.doc.borrow_mut().fragmento(&html);
        let (pai, ref_) = {
            let doc = d.doc.borrow();
            match onde.as_str() {
                "beforebegin" => (doc.pai(no), no),
                "afterbegin" => (no, doc.no(no).primeiro),
                "beforeend" => (no, NENHUM),
                _ => (doc.pai(no), doc.no(no).proximo),
            }
        };
        for n in novos {
            inserir(d, pai, n, ref_);
        }
        Ok(Valor::Indef)
    })
}

fn el_bounding(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let no = this_no(vm, c)?;
    let (g, rol) = com_dom(vm, |_, d| (d.geometria.get(&no).copied().unwrap_or((0.0, 0.0, 0.0, 0.0)), d.rolagem)).unwrap_or(((0.0, 0.0, 0.0, 0.0), 0.0));
    let o = vm.novo_objeto();
    let r = o.obj().unwrap_or(0);
    let (x, y, w, h) = (g.0 as f64, g.1 as f64 - rol as f64, g.2 as f64, g.3 as f64);
    for (k, v) in [("x", x), ("y", y), ("left", x), ("top", y), ("width", w), ("height", h), ("right", x + w), ("bottom", y + h)] {
        vm.definir_propria(r, chave(k), Valor::Num(v));
    }
    Ok(o)
}

fn el_focus(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let no = this_no(vm, c)?;
    com_dom(vm, |_, d| d.foco = no);
    Ok(Valor::Indef)
}

fn el_click(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let no = this_no(vm, c)?;
    disparar(vm, no, "click", &[]);
    Ok(Valor::Indef)
}

fn nada(_vm: &mut Vm, _c: &Ctx) -> R<Valor> {
    Ok(Valor::Indef)
}

// --- classList

fn lista_el(vm: &mut Vm, c: &Ctx) -> R<Id> {
    let el = vm.pegar(c.this, chave("_el"))?;
    match no_de(vm, &el) {
        Some(i) => Ok(i),
        None => vm.erro_tipo("classList sem elemento"),
    }
}

fn classes_de(d: &Dom, no: Id) -> Vec<String> {
    d.doc.borrow().elemento(no).and_then(|e| e.atributo(a::CLASS)).map(|s| s.split_ascii_whitespace().map(String::from).collect()).unwrap_or_default()
}

fn mudar_classes(vm: &mut Vm, c: &Ctx, f: impl Fn(&mut Vec<String>, &[String]) -> Valor) -> R<Valor> {
    let no = lista_el(vm, c)?;
    let mut nomes = Vec::new();
    for x in c.args {
        nomes.push(String::from(&*vm.para_texto(x)?));
    }
    dom!(vm, |d| {
        let mut cl = classes_de(d, no);
        let r = f(&mut cl, &nomes);
        por_atributo(d, no, "class", Some(&cl.join(" ")));
        Ok(r)
    })
}

fn cl_add(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    mudar_classes(vm, c, |cl, n| {
        for x in n {
            if !cl.contains(x) {
                cl.push(x.clone());
            }
        }
        Valor::Indef
    })
}

fn cl_remove(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    mudar_classes(vm, c, |cl, n| {
        cl.retain(|x| !n.contains(x));
        Valor::Indef
    })
}

fn cl_toggle(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let forcar = c.args.get(1).map(|v| v.verdadeiro());
    let nome = vm.para_texto(&c.arg(0))?;
    let no = lista_el(vm, c)?;
    dom!(vm, |d| {
        let mut cl = classes_de(d, no);
        let tem = cl.iter().any(|x| **x == *nome);
        let quer = forcar.unwrap_or(!tem);
        if quer && !tem {
            cl.push(String::from(&*nome));
        } else if !quer {
            cl.retain(|x| **x != *nome);
        }
        por_atributo(d, no, "class", Some(&cl.join(" ")));
        Ok(Valor::Bool(quer))
    })
}

fn cl_contains(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let nome = vm.para_texto(&c.arg(0))?;
    let no = lista_el(vm, c)?;
    dom!(vm, |d| Ok(Valor::Bool(classes_de(d, no).iter().any(|x| **x == *nome))))
}

fn cl_replace(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    mudar_classes(vm, c, |cl, n| {
        if n.len() == 2 {
            for x in cl.iter_mut() {
                if *x == n[0] {
                    *x = n[1].clone();
                }
            }
        }
        Valor::Indef
    })
}

// --- style

/// Nome de propriedade CSS como no atributo: marginTop -> margin-top.
fn nome_css(p: &str) -> String {
    if p == "cssFloat" {
        return String::from("float");
    }
    if p.starts_with("--") || p.contains('-') {
        return String::from(p);
    }
    let k = kebab_case(p);
    if k.starts_with("webkit-") || k.starts_with("moz-") || k.starts_with("ms-") { format!("-{k}") } else { k }
}

fn declaracoes(estilo: &str) -> Vec<(String, String)> {
    estilo.split(';').filter_map(|x| x.split_once(':')).map(|(k, v)| (String::from(k.trim()), String::from(v.trim()))).collect()
}

/// __estiloPor(el, propriedade, valor)
fn estilo_por(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let Some(no) = no_de(vm, &c.arg(0)) else { return Ok(Valor::Indef) };
    let prop = nome_css(&vm.para_texto(&c.arg(1))?);
    let valor = texto_valor(vm, &c.arg(2))?;
    dom!(vm, |d| {
        let atual = d.doc.borrow().elemento(no).and_then(|e| e.atributo(Atomo::de("style")).map(String::from)).unwrap_or_default();
        let mut decls = declaracoes(&atual);
        decls.retain(|(k, _)| *k != prop);
        if !valor.is_empty() {
            decls.push((prop.clone(), valor.clone()));
        }
        let novo: String = decls.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("; ");
        por_atributo(d, no, "style", Some(&novo));
        Ok(Valor::Indef)
    })
}

/// __estiloLer(el, propriedade)
fn estilo_ler(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let Some(no) = no_de(vm, &c.arg(0)) else { return Ok(Valor::texto("")) };
    let prop = nome_css(&vm.para_texto(&c.arg(1))?);
    let atual = com_dom(vm, |_, d| d.doc.borrow().elemento(no).and_then(|e| e.atributo(Atomo::de("style")).map(String::from))).flatten().unwrap_or_default();
    let v = declaracoes(&atual).into_iter().rev().find(|(k, _)| *k == prop).map(|(_, v)| v).unwrap_or_default();
    Ok(Valor::texto(&v))
}

/// getPropertyValue do getComputedStyle: le o campo em camelCase.
fn estilo_get(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let prop = camel_case(&vm.para_texto(&c.arg(0))?);
    let v = vm.pegar(c.this, chave(&prop))?;
    Ok(if v.eh_nulo_ou_indef() { Valor::texto("") } else { v })
}

// ------------------------------------------------------------- eventos

fn add_listener(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let alvo = if c.this.eh_nulo_ou_indef() || matches!(c.this, Valor::Obj(r) if *r == vm.global) { JANELA } else { this_no(vm, c)? };
    let tipo = vm.para_texto(&c.arg(0))?;
    let f = c.arg(1);
    if !vm.eh_chamavel(&f) && !matches!(f, Valor::Obj(_)) {
        return Ok(Valor::Indef);
    }
    let op = c.arg(2);
    let (captura, uma_vez) = match &op {
        Valor::Bool(b) => (*b, false),
        Valor::Obj(_) => (vm.pegar(&op, chave("capture"))?.verdadeiro(), vm.pegar(&op, chave("once"))?.verdadeiro()),
        _ => (false, false),
    };
    com_dom(vm, |_, d| {
        if !d.ouvintes.iter().any(|o| o.alvo == alvo && o.tipo == tipo && o.f == f) {
            d.ouvintes.push(Ouvinte { alvo, tipo, f, captura, uma_vez, propriedade: false });
        }
    });
    Ok(Valor::Indef)
}

fn remove_listener(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let alvo = if c.this.eh_nulo_ou_indef() || matches!(c.this, Valor::Obj(r) if *r == vm.global) { JANELA } else { this_no(vm, c)? };
    let tipo = vm.para_texto(&c.arg(0))?;
    let f = c.arg(1);
    com_dom(vm, |_, d| d.ouvintes.retain(|o| !(o.alvo == alvo && o.tipo == tipo && o.f == f)));
    Ok(Valor::Indef)
}

fn dispatch(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let alvo = if c.this.eh_nulo_ou_indef() || matches!(c.this, Valor::Obj(r) if *r == vm.global) { JANELA } else { this_no(vm, c)? };
    let ev = c.arg(0);
    let tipo = vm.pegar(&ev, const { crate::web::atomo::fixo("type").0 })?;
    let tipo = vm.para_texto(&tipo)?;
    Ok(Valor::Bool(disparar_evento(vm, alvo, &tipo, ev)))
}

/// Cria o objeto de evento e dispara. Devolve false se preventDefault.
pub fn disparar(vm: &mut Vm, alvo: Id, tipo: &str, extras: &[(&str, Valor)]) -> bool {
    let ev = match vm.pegar(&Valor::Obj(vm.global), chave("Event")) {
        Ok(ctor) if vm.eh_chamavel(&ctor) => vm.construir(&ctor, &[Valor::texto(tipo), Valor::Indef], &ctor).unwrap_or_else(|_| vm.novo_objeto()),
        _ => vm.novo_objeto(),
    };
    if let Valor::Obj(r) = ev {
        vm.definir_propria(r, chave("bubbles"), Valor::Bool(!matches!(tipo, "load" | "focus" | "blur" | "DOMContentLoaded" | "readystatechange")));
        for (k, v) in extras {
            vm.definir_propria(r, chave(k), v.clone());
        }
    }
    disparar_evento(vm, alvo, tipo, ev)
}

fn disparar_evento(vm: &mut Vm, alvo: Id, tipo: &str, ev: Valor) -> bool {
    // caminho: o alvo e os ancestrais, depois a janela
    let caminho: Vec<Id> = com_dom(vm, |_, d| {
        let mut v = Vec::new();
        if alvo != JANELA {
            let doc = d.doc.borrow();
            let mut x = alvo;
            while x != NENHUM {
                v.push(x);
                x = doc.pai(x);
            }
        }
        v.push(JANELA);
        v
    })
    .unwrap_or_default();
    let alvo_obj = if alvo == JANELA { Valor::Obj(vm.global) } else { com_dom(vm, |vm, d| objeto(vm, d, alvo)).unwrap_or_default() };
    let _ = vm.por(&ev, chave("target"), alvo_obj.clone());
    let _ = vm.por(&ev, chave("srcElement"), alvo_obj);
    let borbulha = vm.pegar(&ev, chave("bubbles")).map(|b| b.verdadeiro()).unwrap_or(true);
    for (k, no) in caminho.iter().enumerate() {
        if k > 0 && !borbulha {
            break;
        }
        let atual = if *no == JANELA { Valor::Obj(vm.global) } else { com_dom(vm, |vm, d| objeto(vm, d, *no)).unwrap_or_default() };
        let _ = vm.por(&ev, chave("currentTarget"), atual.clone());
        let fs: Vec<(Valor, bool)> = com_dom(vm, |_, d| {
            d.ouvintes.iter().filter(|o| o.alvo == *no && &*o.tipo == tipo).map(|o| (o.f.clone(), o.uma_vez)).collect()
        })
        .unwrap_or_default();
        // handler em atributo HTML (onclick="...")
        let inline = if *no != JANELA {
            com_dom(vm, |_, d| d.doc.borrow().elemento(*no).and_then(|e| e.atributo(Atomo::de(&format!("on{tipo}"))).map(String::from))).flatten()
        } else {
            None
        };
        if let Some(codigo) = inline {
            let fonte = format!("(function(event){{{codigo}\n}})");
            if let Ok(f) = super::avaliar(vm, &fonte) {
                if let Ok(r) = vm.chamar(&f, &atual, &[ev.clone()]) {
                    if matches!(r, Valor::Bool(false)) {
                        let _ = vm.por(&ev, chave("defaultPrevented"), Valor::Bool(true));
                    }
                }
            }
        }
        for (f, uma_vez) in fs {
            let r = if vm.eh_chamavel(&f) {
                vm.chamar(&f, &atual, &[ev.clone()])
            } else {
                // objeto com handleEvent
                match vm.pegar(&f, chave("handleEvent")) {
                    Ok(h) if vm.eh_chamavel(&h) => vm.chamar(&h, &f, &[ev.clone()]),
                    _ => Ok(Valor::Indef),
                }
            };
            if let Err(e) = r {
                let m = vm.mostrar(&e, 0);
                vm.console.push_str(&format!("erro em evento {tipo}: {m}\n"));
            }
            if uma_vez {
                let no2 = *no;
                com_dom(vm, |_, d| d.ouvintes.retain(|o| !(o.alvo == no2 && &*o.tipo == tipo && o.f == f)));
            }
        }
        if vm.pegar(&ev, chave("_parar")).map(|v| v.verdadeiro()).unwrap_or(false) {
            break;
        }
    }
    vm.rodar_microtarefas();
    !vm.pegar(&ev, chave("defaultPrevented")).map(|v| v.verdadeiro()).unwrap_or(false)
}

// -------------------------------------------------------- timers e rede

fn set_timer(vm: &mut Vm, c: &Ctx, repete: bool) -> R<Valor> {
    let f = c.arg(0);
    let atraso = if c.args.len() > 1 { vm.para_numero(&c.arg(1))?.max(0.0) } else { 0.0 };
    let f = if vm.eh_chamavel(&f) {
        f
    } else {
        let s = vm.para_texto(&f)?;
        super::avaliar(vm, &format!("(function(){{{s}\n}})"))?
    };
    let args: Vec<Valor> = c.args.iter().skip(2).cloned().collect();
    let id = com_dom(vm, |_, d| {
        d.prox_timer += 1;
        let id = d.prox_timer;
        d.timers.push(Timer { id, quando: d.agora + atraso, f, args, intervalo: if repete { Some(atraso.max(4.0)) } else { None } });
        id
    })
    .unwrap_or(0);
    Ok(Valor::Num(id as f64))
}

fn set_timeout(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    set_timer(vm, c, false)
}

fn set_interval(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    set_timer(vm, c, true)
}

fn clear_timer(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let id = vm.para_numero(&c.arg(0)).unwrap_or(0.0) as u32;
    com_dom(vm, |_, d| d.timers.retain(|t| t.id != id));
    Ok(Valor::Indef)
}

fn request_animation_frame(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let f = c.arg(0);
    let id = com_dom(vm, |_, d| {
        d.prox_timer += 1;
        let id = d.prox_timer;
        let agora = d.agora;
        d.timers.push(Timer { id, quando: agora + 16.0, f, args: alloc::vec![Valor::Num(agora + 16.0)], intervalo: None });
        id
    })
    .unwrap_or(0);
    Ok(Valor::Num(id as f64))
}

fn performance_now(vm: &mut Vm, _c: &Ctx) -> R<Valor> {
    Ok(Valor::Num(com_dom(vm, |_, d| d.agora).unwrap_or(0.0)))
}

/// fetch(url, opcoes) e o envio do XHR: so enfileira; a pagina busca.
fn enfileirar(vm: &mut Vm, url: &Valor, opcoes: &Valor, promessa: Valor, xhr: Valor) -> R<()> {
    let u = vm.para_texto(url)?;
    let mut metodo = String::from("GET");
    let mut corpo = None;
    let mut cabecalhos = Vec::new();
    if let Valor::Obj(_) = opcoes {
        let m = vm.pegar(opcoes, chave("method"))?;
        if !m.eh_nulo_ou_indef() {
            metodo = vm.para_texto(&m)?.to_ascii_uppercase();
        }
        let b = vm.pegar(opcoes, chave("body"))?;
        if !b.eh_nulo_ou_indef() {
            corpo = Some(String::from(&*vm.para_texto(&b)?));
        }
        let h = vm.pegar(opcoes, chave("headers"))?;
        if let Valor::Obj(r) = h {
            for k in vm.chaves_proprias(r, true) {
                let ch = vm.chave_de(&k)?;
                let v = vm.pegar(&h, ch)?;
                cabecalhos.push((String::from(&*vm.para_texto(&k)?), String::from(&*vm.para_texto(&v)?)));
            }
        }
    }
    com_dom(vm, |_, d| {
        let url = crate::web::url::resolver(&d.url, &u);
        d.pedidos.push(Pedido { metodo, url, corpo, cabecalhos, promessa, xhr });
    });
    Ok(())
}

fn fetch(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let p = vm.nova_promessa();
    let alvo = c.arg(0);
    // fetch(Request) ou fetch(url)
    let url = match &alvo {
        Valor::Obj(_) if !vm.eh_chamavel(&alvo) => {
            let u = vm.pegar(&alvo, chave("url"))?;
            if matches!(u, Valor::Indef) { Valor::Texto(vm.para_texto(&alvo)?) } else { u }
        }
        v => v.clone(),
    };
    enfileirar(vm, &url, &c.arg(1), p.clone(), Valor::Indef)?;
    Ok(p)
}

fn xhr_enviar(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let url = vm.pegar(c.this, chave("_url"))?;
    let metodo = vm.pegar(c.this, chave("_metodo"))?;
    let op = vm.novo_objeto();
    if let Valor::Obj(r) = op {
        vm.definir_propria(r, chave("method"), metodo);
        vm.definir_propria(r, chave("body"), c.arg(0));
        let h = vm.pegar(c.this, chave("_cab"))?;
        vm.definir_propria(r, chave("headers"), h);
    }
    enfileirar(vm, &url, &op, Valor::Indef, c.this.clone())?;
    Ok(Valor::Indef)
}

fn storage_get(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let k = vm.para_texto(&c.arg(0))?;
    Ok(com_dom(vm, |_, d| d.armazenamento.get(&*k).map(|v| Valor::texto(v))).flatten().unwrap_or(Valor::Nulo))
}

fn storage_set(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let k = vm.para_texto(&c.arg(0))?;
    let v = vm.para_texto(&c.arg(1))?;
    com_dom(vm, |_, d| d.armazenamento.insert(String::from(&*k), String::from(&*v)));
    Ok(Valor::Indef)
}

fn storage_remove(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let k = vm.para_texto(&c.arg(0))?;
    com_dom(vm, |_, d| d.armazenamento.remove(&*k));
    Ok(Valor::Indef)
}

fn storage_clear(vm: &mut Vm, _c: &Ctx) -> R<Valor> {
    com_dom(vm, |_, d| d.armazenamento.clear());
    Ok(Valor::Indef)
}

fn storage_key(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let i = vm.para_numero(&c.arg(0))? as usize;
    Ok(com_dom(vm, |_, d| d.armazenamento.keys().nth(i).map(|k| Valor::texto(k))).flatten().unwrap_or(Valor::Nulo))
}

fn storage_len(vm: &mut Vm, _c: &Ctx) -> R<Valor> {
    Ok(Valor::Num(com_dom(vm, |_, d| d.armazenamento.len()).unwrap_or(0) as f64))
}

fn match_media(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let q = vm.para_texto(&c.arg(0))?;
    let (l, a) = com_dom(vm, |_, d| (d.largura, d.altura)).unwrap_or((1280.0, 720.0));
    let t = crate::web::css::token::tokens(&q);
    let m = crate::web::css::folha::avaliar_midia(&t, &crate::web::css::folha::Midia { largura: l, altura: a });
    let o = vm.novo_objeto();
    if let Valor::Obj(r) = o {
        vm.definir_propria(r, chave("matches"), Valor::Bool(m));
        vm.definir_propria(r, chave("media"), Valor::Texto(q));
        for n in ["addListener", "removeListener", "addEventListener", "removeEventListener"] {
            vm.metodo(r, n, nada, 1);
        }
    }
    Ok(o)
}

fn computed_style(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    // estilo calculado aproximado: o style inline + valores conhecidos
    let el = c.arg(0);
    let o = vm.novo_objeto();
    let r = o.obj().unwrap_or(0);
    if let Some(no) = no_de(vm, &el) {
        let (estilo, oculto) = com_dom(vm, |_, d| {
            let doc = d.doc.borrow();
            let e = doc.elemento(no);
            (e.and_then(|e| e.atributo(Atomo::de("style")).map(String::from)).unwrap_or_default(), e.is_some_and(|e| e.tem(a::HIDDEN)))
        })
        .unwrap_or_default();
        vm.definir_propria(r, chave("display"), Valor::texto(if oculto { "none" } else { "block" }));
        vm.definir_propria(r, chave("visibility"), Valor::texto("visible"));
        vm.definir_propria(r, chave("opacity"), Valor::texto("1"));
        for decl in estilo.split(';') {
            if let Some((k, v)) = decl.split_once(':') {
                vm.definir_propria(r, chave(&camel_case(k.trim())), Valor::texto(v.trim()));
            }
        }
    }
    let gp = vm.nativa("getPropertyValue", estilo_get, 1);
    vm.definir(r, "getPropertyValue", gp);
    Ok(o)
}

fn atob(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = vm.para_texto(&c.arg(0))?;
    let b = crate::web::url::base64(&s);
    Ok(Valor::texto(&b.iter().map(|&x| x as char).collect::<String>()))
}

fn btoa(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let s = vm.para_texto(&c.arg(0))?;
    let bytes: Vec<u8> = s.chars().map(|ch| ch as u32 as u8).collect();
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for pedaco in bytes.chunks(3) {
        let n = (pedaco[0] as u32) << 16 | (*pedaco.get(1).unwrap_or(&0) as u32) << 8 | *pedaco.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if pedaco.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if pedaco.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    Ok(Valor::texto(&out))
}

/// __compilarModulo(url, texto) -> { f, deps }
fn compilar_modulo(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let url = vm.para_texto(&c.arg(0))?;
    let fonte = vm.para_texto(&c.arg(1))?;
    // mapa de imports (<script type=importmap>), guardado pelo PRELUDIO
    let mut mapa = Vec::new();
    let g = Valor::Obj(vm.global);
    if let Ok(Valor::Obj(r)) = vm.pegar(&g, chave("__mapaImport")) {
        for k in vm.chaves_proprias(r, true) {
            let ch = vm.chave_de(&k)?;
            let v = vm.pegar(&Valor::Obj(r), ch)?;
            mapa.push((String::from(&*vm.para_texto(&k)?), String::from(&*vm.para_texto(&v)?)));
        }
    }
    let m = match super::modulo::transformar(&fonte, &url, &mapa) {
        Ok(m) => m,
        Err(e) => {
            let p = vm.p.erro_sintaxe;
            return Err(vm.novo_erro(p, &format!("{e} [{url}]")));
        }
    };
    let f = super::avaliar(vm, &m.codigo)?;
    let deps: Vec<Valor> = m.deps.iter().map(|d| Valor::texto(d)).collect();
    let deps = vm.novo_array(deps);
    let o = vm.novo_objeto();
    if let Valor::Obj(r) = o {
        vm.definir_propria(r, chave("f"), f);
        vm.definir_propria(r, chave("deps"), deps);
    }
    Ok(o)
}

fn location_assign(vm: &mut Vm, c: &Ctx) -> R<Valor> {
    let u = vm.para_texto(&c.arg(0))?;
    com_dom(vm, |_, d| d.navegar = Some(crate::web::url::resolver(&d.url, &u)));
    Ok(Valor::Indef)
}

fn location_reload(vm: &mut Vm, _c: &Ctx) -> R<Valor> {
    com_dom(vm, |_, d| d.navegar = Some(d.url.clone()));
    Ok(Valor::Indef)
}

// ------------------------------------------------------------ instalar

/// Prepara a VM para uma pagina: document, window e companhia.
pub fn instalar(vm: &mut Vm, doc: Rc<RefCell<Documento>>, url: &str, largura: f32, altura: f32) {
    let po = vm.p.objeto;
    let no = vm.alocar(Some(po), Classe::Comum);
    let elemento = vm.alocar(Some(no), Classe::Comum);
    let texto = vm.alocar(Some(no), Classe::Comum);
    let documento = vm.alocar(Some(no), Classe::Comum);
    let lista_classes = vm.alocar(Some(po), Classe::Comum);
    let estilo = vm.alocar(Some(po), Classe::Comum);
    let d = Dom {
        doc,
        objetos: BTreeMap::new(),
        ouvintes: Vec::new(),
        timers: Vec::new(),
        prox_timer: 0,
        pedidos: Vec::new(),
        sujo: false,
        url: String::from(url),
        armazenamento: BTreeMap::new(),
        cookies: BTreeMap::new(),
        geometria: BTreeMap::new(),
        p: Protos { no, elemento, texto, documento, lista_classes, estilo },
        agora: 0.0,
        largura,
        altura,
        rolagem: 0.0,
        navegar: None,
        foco: NENHUM,
        script_atual: NENHUM,
        escrito: String::new(),
        conteudos: BTreeMap::new(),
    };
    vm.anfitriao = Some(alloc::boxed::Box::new(d));
    let m: &[(&str, super::vm::FnNativa, u8)] = &[
        ("appendChild", el_append_child, 1),
        ("insertBefore", el_insert_before, 2),
        ("removeChild", el_remove_child, 1),
        ("replaceChild", el_replace_child, 2),
        ("remove", el_remove, 0),
        ("append", el_append, 1),
        ("prepend", el_prepend, 1),
        ("before", el_before, 1),
        ("after", el_after, 1),
        ("replaceWith", el_replace_with, 1),
        ("cloneNode", el_clone, 1),
        ("contains", el_contains, 1),
        ("hasChildNodes", el_has_child_nodes, 0),
        ("addEventListener", add_listener, 2),
        ("removeEventListener", remove_listener, 2),
        ("dispatchEvent", dispatch, 1),
        ("querySelector", el_query_selector, 1),
        ("querySelectorAll", el_query_selector_all, 1),
        ("getElementsByTagName", el_by_tag, 1),
        ("getElementsByClassName", el_by_class, 1),
    ];
    for &(n, f, k) in m {
        vm.metodo(no, n, f, k);
    }
    let e: &[(&str, super::vm::FnNativa, u8)] = &[
        ("getAttribute", el_get_attribute, 1),
        ("setAttribute", el_set_attribute, 2),
        ("removeAttribute", el_remove_attribute, 1),
        ("hasAttribute", el_has_attribute, 1),
        ("toggleAttribute", el_toggle_attribute, 1),
        ("getAttributeNames", el_attribute_names, 0),
        ("matches", el_matches, 1),
        ("closest", el_closest, 1),
        ("insertAdjacentHTML", el_insert_adjacent_html, 2),
        ("getBoundingClientRect", el_bounding, 0),
        ("focus", el_focus, 0),
        ("blur", nada, 0),
        ("click", el_click, 0),
        ("scrollIntoView", nada, 0),
        ("scrollTo", nada, 0),
        ("scrollBy", nada, 0),
        ("setPointerCapture", nada, 1),
        ("releasePointerCapture", nada, 1),
        ("attachShadow", nada, 1),
        ("animate", nada, 1),
        ("requestFullscreen", nada, 0),
        ("submit", nada, 0),
        ("reset", nada, 0),
        ("select", nada, 0),
        ("setSelectionRange", nada, 2),
        ("play", nada, 0),
        ("pause", nada, 0),
        ("load", nada, 0),
        ("checkValidity", nada, 0),
    ];
    for &(n, f, k) in e {
        vm.metodo(elemento, n, f, k);
    }
    let dm: &[(&str, super::vm::FnNativa, u8)] = &[
        ("getElementById", doc_by_id, 1),
        ("getElementsByName", doc_by_name, 1),
        ("createElement", doc_create_element, 1),
        ("createElementNS", doc_create_element_ns, 2),
        ("createTextNode", doc_create_text, 1),
        ("createComment", doc_create_comment, 1),
        ("createDocumentFragment", doc_create_fragment, 0),
        ("write", doc_write, 1),
        ("writeln", doc_write, 1),
        ("hasFocus", nada, 0),
        ("execCommand", nada, 1),
    ];
    for &(n, f, k) in dm {
        vm.metodo(documento, n, f, k);
    }
    for (n, f, k) in [
        ("add", cl_add as super::vm::FnNativa, 1u8),
        ("remove", cl_remove, 1),
        ("toggle", cl_toggle, 1),
        ("contains", cl_contains, 1),
        ("replace", cl_replace, 2),
    ] {
        vm.metodo(lista_classes, n, f, k);
    }
    // window = o objeto global
    let g = vm.global;
    let gv = Valor::Obj(g);
    for n in ["window", "self", "top", "parent", "frames"] {
        vm.definir(g, n, gv.clone());
    }
    let doc_obj = com_dom(vm, |vm, d| objeto(vm, d, RAIZ)).unwrap_or_default();
    vm.definir(g, "document", doc_obj);
    let gm: &[(&str, super::vm::FnNativa, u8)] = &[
        ("setTimeout", set_timeout, 2),
        ("setInterval", set_interval, 2),
        ("clearTimeout", clear_timer, 1),
        ("clearInterval", clear_timer, 1),
        ("requestAnimationFrame", request_animation_frame, 1),
        ("cancelAnimationFrame", clear_timer, 1),
        ("requestIdleCallback", set_timeout, 1),
        ("cancelIdleCallback", clear_timer, 1),
        ("addEventListener", add_listener, 2),
        ("removeEventListener", remove_listener, 2),
        ("dispatchEvent", dispatch, 1),
        ("fetch", fetch, 1),
        ("matchMedia", match_media, 1),
        ("getComputedStyle", computed_style, 1),
        ("atob", atob, 1),
        ("btoa", btoa, 1),
        ("scrollTo", nada, 2),
        ("scrollBy", nada, 2),
        ("scroll", nada, 2),
        ("alert", nada, 1),
        ("confirm", nada, 1),
        ("prompt", nada, 1),
        ("print", nada, 0),
        ("focus", nada, 0),
        ("blur", nada, 0),
        ("open", nada, 1),
        ("close", nada, 0),
        ("postMessage", nada, 1),
        ("__xhrEnviar", xhr_enviar, 1),
        ("__estiloPor", estilo_por, 3),
        ("__compilarModulo", compilar_modulo, 2),
        ("__estiloLer", estilo_ler, 2),
        ("__locationAssign", location_assign, 1),
        ("__locationReload", location_reload, 0),
    ];
    for &(n, f, k) in gm {
        vm.metodo(g, n, f, k);
    }
    for (n, v) in [
        ("innerWidth", largura as f64),
        ("innerHeight", altura as f64),
        ("outerWidth", largura as f64),
        ("outerHeight", altura as f64),
        ("devicePixelRatio", 1.0),
        ("scrollX", 0.0),
        ("scrollY", 0.0),
        ("pageXOffset", 0.0),
        ("pageYOffset", 0.0),
    ] {
        vm.definir(g, n, Valor::Num(v));
    }
    // performance e localStorage
    let perf = vm.novo_objeto();
    if let Valor::Obj(r) = perf {
        vm.metodo(r, "now", performance_now, 0);
        vm.metodo(r, "mark", nada, 1);
        vm.metodo(r, "measure", nada, 1);
        vm.metodo(r, "getEntriesByType", |vm, _| Ok(vm.novo_array(Vec::new())), 1);
        vm.metodo(r, "getEntriesByName", |vm, _| Ok(vm.novo_array(Vec::new())), 1);
    }
    vm.definir(g, "performance", perf);
    for nome in ["localStorage", "sessionStorage"] {
        let s = vm.novo_objeto();
        if let Valor::Obj(r) = s {
            vm.metodo(r, "getItem", storage_get, 1);
            vm.metodo(r, "setItem", storage_set, 2);
            vm.metodo(r, "removeItem", storage_remove, 1);
            vm.metodo(r, "clear", storage_clear, 0);
            vm.metodo(r, "key", storage_key, 1);
            let len = vm.nativa("length", storage_len, 0);
            vm.definir_acessor(r, const { crate::web::atomo::fixo("length").0 }, len, 0, false);
        }
        vm.definir(g, nome, s);
    }
    vm.definir(g, "__url", Valor::texto(url));
    if let Err(e) = super::avaliar(vm, PRELUDIO) {
        let m = vm.mostrar(&e, 0);
        vm.console.push_str(&format!("erro no prelúdio: {m}\n"));
    }
}

/// O resto do ambiente do navegador, em JavaScript.
pub const PRELUDIO: &str = r#"
(function (g) {
  function def(o, n, v) { Object.defineProperty(o, n, { value: v, writable: true, configurable: true, enumerable: false }); }
  // ---------------- Event
  class Event {
    constructor(type, init) {
      init = init || {};
      this.type = String(type); this.bubbles = !!init.bubbles; this.cancelable = !!init.cancelable;
      this.composed = !!init.composed; this.defaultPrevented = false; this.timeStamp = performance.now();
      this.target = null; this.currentTarget = null; this.isTrusted = false; this.eventPhase = 2;
      for (const k in init) if (!(k in this)) this[k] = init[k];
    }
    preventDefault() { if (this.cancelable !== false) this.defaultPrevented = true; }
    stopPropagation() { this._parar = true; }
    stopImmediatePropagation() { this._parar = true; }
    composedPath() { const p = []; let n = this.target; while (n) { p.push(n); n = n.parentNode; } p.push(window); return p; }
  }
  g.Event = Event;
  class CustomEvent extends Event { constructor(t, i) { super(t, i); this.detail = i && i.detail !== undefined ? i.detail : null; } }
  g.CustomEvent = CustomEvent;
  for (const n of ['UIEvent','MouseEvent','KeyboardEvent','FocusEvent','InputEvent','PointerEvent','TouchEvent','WheelEvent','ProgressEvent','ErrorEvent','MessageEvent','PopStateEvent','HashChangeEvent','SubmitEvent','AnimationEvent','TransitionEvent'])
    g[n] = class extends Event {};
  class EventTarget {
    constructor() { this._l = {}; }
    addEventListener(t, f) { (this._l[t] = this._l[t] || []).push(f); }
    removeEventListener(t, f) { const l = this._l[t]; if (l) this._l[t] = l.filter(x => x !== f); }
    dispatchEvent(e) { e.target = e.currentTarget = this; for (const f of (this._l[e.type] || []).slice()) (typeof f === 'function' ? f : f.handleEvent).call(this, e); const h = this['on' + e.type]; if (typeof h === 'function') h.call(this, e); return !e.defaultPrevented; }
  }
  g.EventTarget = EventTarget;
  // ---------------- el.style: Proxy sobre o atributo style
  const metodosEstilo = {
    setProperty(el) { return (p, v) => __estiloPor(el, p, v); },
    getPropertyValue(el) { return p => __estiloLer(el, p); },
    removeProperty(el) { return p => { const v = __estiloLer(el, p); __estiloPor(el, p, ''); return v; }; },
    item(el) { return i => ''; },
  };
  g.__estilo = function (el) {
    return new Proxy({}, {
      get(t, k) {
        if (typeof k !== 'string') return undefined;
        if (k in metodosEstilo) return metodosEstilo[k](el);
        if (k === 'cssText') return el.getAttribute('style') || '';
        if (k === 'length') return (el.getAttribute('style') || '').split(';').filter(x => x.trim()).length;
        return __estiloLer(el, k);
      },
      set(t, k, v) { if (k === 'cssText') el.setAttribute('style', v); else __estiloPor(el, k, v); return true; },
      has(t, k) { return true; },
    });
  };
  // ---------------- construtores do DOM (instanceof)
  const protoEl = Object.getPrototypeOf(document.createElement('div'));
  const protoNo = Object.getPrototypeOf(protoEl);
  const protoTexto = Object.getPrototypeOf(document.createTextNode(''));
  function ctor(proto, nome) { const f = function () { throw new TypeError('Illegal constructor'); }; f.prototype = proto; def(proto, 'constructor', f); g[nome] = f; return f; }
  ctor(protoNo, 'Node'); ctor(protoEl, 'Element'); ctor(protoTexto, 'Text');
  ctor(Object.getPrototypeOf(document), 'Document');
  for (const n of ['HTMLMetaElement','HTMLParagraphElement','HTMLHeadingElement','HTMLUListElement','HTMLOListElement','HTMLLIElement','HTMLTableElement','HTMLTableRowElement','HTMLTableCellElement','HTMLLabelElement','HTMLOptionElement','HTMLPreElement','HTMLBRElement','HTMLHRElement','HTMLPictureElement','HTMLSourceElement','HTMLAudioElement','HTMLDialogElement','HTMLDetailsElement','HTMLSlotElement','HTMLTitleElement','HTMLBaseElement','HTMLObjectElement','HTMLEmbedElement','HTMLFieldSetElement','HTMLOutputElement','HTMLProgressElement','HTMLMeterElement','HTMLDataElement','HTMLTimeElement','HTMLQuoteElement','HTMLModElement','HTMLAreaElement','HTMLMapElement','HTMLTrackElement','HTMLMenuElement','HTMLFrameElement','HTMLFrameSetElement','HTMLLegendElement','HTMLDListElement','HTMLTableSectionElement','HTMLTableColElement','HTMLTableCaptionElement','SVGSVGElement','SVGGraphicsElement','SVGPathElement','ShadowRoot','Attr','CSSStyleSheet','CSSStyleDeclaration','DOMRect','DOMTokenList','NamedNodeMap','Range','Selection','StyleSheet','MediaQueryList','Storage','Location','History','Navigator','Screen','Window'])
    if (!g[n]) g[n] = n.indexOf('Element') >= 0 ? g.Element : function () {};
  for (const n of ['HTMLElement','HTMLDivElement','HTMLAnchorElement','HTMLImageElement','HTMLInputElement','HTMLButtonElement','HTMLFormElement','HTMLScriptElement','HTMLStyleElement','HTMLLinkElement','HTMLIFrameElement','HTMLCanvasElement','HTMLVideoElement','HTMLMediaElement','HTMLTemplateElement','HTMLSelectElement','HTMLTextAreaElement','HTMLSpanElement','HTMLBodyElement','HTMLHeadElement','HTMLHtmlElement','SVGElement','HTMLUnknownElement','DocumentFragment','CharacterData','Comment','HTMLCollection','NodeList'])
    if (!g[n]) g[n] = n.indexOf('Element') >= 0 ? g.Element : function () {};
  Object.assign(protoNo, { ELEMENT_NODE: 1, TEXT_NODE: 3, COMMENT_NODE: 8, DOCUMENT_NODE: 9, DOCUMENT_FRAGMENT_NODE: 11 });
  Object.assign(g.Node, { ELEMENT_NODE: 1, TEXT_NODE: 3, COMMENT_NODE: 8, DOCUMENT_NODE: 9, DOCUMENT_FRAGMENT_NODE: 11 });
  document.defaultView = window;
  document.createEvent = function (t) { const e = new Event(''); e.initEvent = function (tp, b, c) { this.type = tp; this.bubbles = b; this.cancelable = c; }; e.initCustomEvent = function (tp, b, c, d) { this.initEvent(tp, b, c); this.detail = d; }; return e; };
  document.createRange = function () { return { setStart() {}, setEnd() {}, selectNodeContents() {}, collapse() {}, getBoundingClientRect() { return { x: 0, y: 0, width: 0, height: 0, top: 0, left: 0, right: 0, bottom: 0 }; }, createContextualFragment(h) { const t = document.createElement('div'); t.innerHTML = h; const f = document.createDocumentFragment(); while (t.firstChild) f.appendChild(t.firstChild); return f; } }; };
  document.getSelection = g.getSelection = function () { return { rangeCount: 0, removeAllRanges() {}, addRange() {}, toString() { return ''; } }; };
  document.fonts = { ready: Promise.resolve(), load() { return Promise.resolve([]); }, check() { return true; }, add() {}, addEventListener() {} };
  // ---------------- location / history / navigator / screen
  function lerUrl(u) {
    const m = /^([a-z][a-z0-9+.-]*:)?(\/\/([^/?#]*))?([^?#]*)(\?[^#]*)?(#.*)?$/i.exec(u) || [];
    const host = m[3] || ''; const hp = host.split(':');
    return { href: u, protocol: m[1] || '', host: host, hostname: hp[0] || '', port: hp[1] || '', pathname: m[4] || '/', search: m[5] || '', hash: m[6] || '', origin: (m[1] || '') + '//' + host };
  }
  const loc = lerUrl(g.__url);
  loc.assign = function (u) { __locationAssign(u); }; loc.replace = loc.assign; loc.reload = function () { __locationReload(); };
  loc.toString = function () { return this.href; };
  g.location = loc; document.location = loc;
  g.history = { length: 1, state: null, pushState(s, t, u) { this.state = s; if (u) Object.assign(loc, lerUrl(new URL(u, loc.href).href)); }, replaceState(s, t, u) { this.pushState(s, t, u); }, back() {}, forward() {}, go() {} };
  g.navigator = { userAgent: 'Mozilla/5.0 (X11; Linux armv7l) AppleWebKit/537.36 (KHTML, like Gecko) MXOS/0.12 Safari/537.36', language: 'pt-BR', languages: ['pt-BR', 'pt', 'en'], platform: 'Linux armv7l', vendor: 'MXOS', onLine: true, cookieEnabled: true, hardwareConcurrency: 4, maxTouchPoints: 0, doNotTrack: '1', webdriver: false,
    serviceWorker: undefined, clipboard: { writeText() { return Promise.resolve(); }, readText() { return Promise.resolve(''); } }, sendBeacon() { return true; }, userAgentData: undefined, mediaDevices: undefined, permissions: { query() { return Promise.resolve({ state: 'denied' }); } } };
  g.screen = { width: innerWidth, height: innerHeight, availWidth: innerWidth, availHeight: innerHeight, colorDepth: 24, pixelDepth: 24, orientation: { type: 'landscape-primary', angle: 0, addEventListener() {} } };
  g.visualViewport = { width: innerWidth, height: innerHeight, scale: 1, offsetTop: 0, offsetLeft: 0, addEventListener() {}, removeEventListener() {} };
  // ---------------- URL e URLSearchParams
  class URLSearchParams {
    constructor(i) {
      this._p = [];
      if (typeof i === 'string') { i = i.replace(/^\?/, ''); for (const par of i.split('&')) { if (!par) continue; const k = par.indexOf('='); const d = s => decodeURIComponent(s.replace(/\+/g, ' ')); this._p.push(k < 0 ? [d(par), ''] : [d(par.slice(0, k)), d(par.slice(k + 1))]); } }
      else if (Array.isArray(i)) this._p = i.map(x => [String(x[0]), String(x[1])]);
      else if (i && typeof i === 'object') for (const k of Object.keys(i)) this._p.push([k, String(i[k])]);
    }
    get(k) { const x = this._p.find(p => p[0] === k); return x ? x[1] : null; }
    getAll(k) { return this._p.filter(p => p[0] === k).map(p => p[1]); }
    has(k) { return this._p.some(p => p[0] === k); }
    set(k, v) { const i = this._p.findIndex(p => p[0] === k); if (i < 0) this._p.push([k, String(v)]); else { this._p[i][1] = String(v); this._p = this._p.filter((p, j) => j <= i || p[0] !== k); } }
    append(k, v) { this._p.push([k, String(v)]); }
    delete(k) { this._p = this._p.filter(p => p[0] !== k); }
    forEach(f) { for (const [k, v] of this._p) f(v, k, this); }
    keys() { return this._p.map(p => p[0])[Symbol.iterator](); }
    values() { return this._p.map(p => p[1])[Symbol.iterator](); }
    entries() { return this._p.map(p => [p[0], p[1]])[Symbol.iterator](); }
    [Symbol.iterator]() { return this.entries(); }
    get size() { return this._p.length; }
    sort() { this._p.sort((a, b) => a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0); }
    toString() { return this._p.map(p => encodeURIComponent(p[0]) + '=' + encodeURIComponent(p[1])).join('&'); }
  }
  g.URLSearchParams = URLSearchParams;
  class URL {
    constructor(u, base) {
      u = String(u);
      if (base !== undefined && !/^[a-z][a-z0-9+.-]*:/i.test(u)) {
        const b = lerUrl(String(base));
        if (u.startsWith('//')) u = b.protocol + u;
        else if (u.startsWith('/')) u = b.origin + u;
        else if (u.startsWith('?')) u = b.origin + b.pathname + u;
        else if (u.startsWith('#')) u = b.origin + b.pathname + b.search + u;
        else u = b.origin + b.pathname.replace(/[^/]*$/, '') + u;
      }
      if (!/^[a-z][a-z0-9+.-]*:/i.test(u)) throw new TypeError('URL inválida: ' + u);
      const p = lerUrl(u);
      // resolve ./ e ../
      const partes = []; for (const s of p.pathname.split('/')) { if (s === '..') partes.pop(); else if (s !== '.') partes.push(s); }
      p.pathname = partes.join('/') || '/'; delete p.href;
      Object.assign(this, p);
      this.searchParams = new URLSearchParams(this.search);
      this.username = ''; this.password = '';
    }
    get href() { const q = this.searchParams ? this.searchParams.toString() : ''; return this.protocol + (this.host || this.protocol === 'file:' ? '//' + this.host : '') + this.pathname + (q ? '?' + q : '') + this.hash; }
    set href(v) { Object.assign(this, new URL(v)); }
    toString() { return this.href; }
    toJSON() { return this.href; }
    static createObjectURL() { return 'blob:mxos'; }
    static revokeObjectURL() {}
    static canParse(u, b) { try { new URL(u, b); return true; } catch (e) { return false; } }
  }
  g.URL = URL;
  // ---------------- fetch: Response e Headers
  class Headers {
    constructor(i) { this._h = {}; if (i) { if (i instanceof Headers) i = i._h; for (const k in i) this._h[k.toLowerCase()] = String(i[k]); } }
    get(k) { k = k.toLowerCase(); return k in this._h ? this._h[k] : null; }
    set(k, v) { this._h[k.toLowerCase()] = String(v); }
    append(k, v) { this.set(k, v); }
    has(k) { return k.toLowerCase() in this._h; }
    delete(k) { delete this._h[k.toLowerCase()]; }
    forEach(f) { for (const k in this._h) f(this._h[k], k); }
    entries() { return Object.entries(this._h)[Symbol.iterator](); }
    [Symbol.iterator]() { return this.entries(); }
  }
  g.Headers = Headers;
  class Response {
    constructor(corpo, init) { init = init || {}; this._t = corpo == null ? '' : String(corpo); this.status = init.status === undefined ? 200 : init.status; this.ok = this.status >= 200 && this.status < 300; this.statusText = init.statusText || ''; this.headers = new Headers(init.headers); this.url = init.url || ''; this.bodyUsed = false; this.type = 'basic'; this.redirected = false; }
    text() { this.bodyUsed = true; return Promise.resolve(this._t); }
    json() { this.bodyUsed = true; try { return Promise.resolve(JSON.parse(this._t)); } catch (e) { return Promise.reject(e); } }
    arrayBuffer() { return Promise.resolve(new TextEncoder().encode(this._t).buffer); }
    blob() { return Promise.resolve({ size: this._t.length, type: this.headers.get('content-type') || '', text: () => Promise.resolve(this._t) }); }
    clone() { return new Response(this._t, { status: this.status, headers: this.headers._h, url: this.url }); }
  }
  g.Response = Response;
  g.Request = class Request { constructor(u, o) { this.url = String(u && u.url || u); Object.assign(this, o || {}); this.method = (o && o.method) || 'GET'; this.headers = new Headers(o && o.headers); } };
  g.AbortController = class AbortController { constructor() { this.signal = { aborted: false, addEventListener() {}, removeEventListener() {}, throwIfAborted() {} }; } abort() { this.signal.aborted = true; } };
  g.AbortSignal = { timeout() { return { aborted: false, addEventListener() {} }; }, any() { return { aborted: false, addEventListener() {} }; } };
  // ---------------- XMLHttpRequest
  class XMLHttpRequest extends EventTarget {
    constructor() { super(); this.readyState = 0; this.status = 0; this.responseText = ''; this.response = ''; this.responseType = ''; this._cab = {}; this.timeout = 0; this.withCredentials = false; this.upload = new EventTarget(); }
    open(m, u) { this._metodo = m; this._url = u; this.readyState = 1; }
    setRequestHeader(k, v) { this._cab[k] = v; }
    getResponseHeader(k) { return this._resp ? this._resp.get(k) : null; }
    getAllResponseHeaders() { return ''; }
    overrideMimeType() {}
    abort() {}
    send(b) { __xhrEnviar.call(this, b); }
    _fim(status, texto, cab) {
      this.status = status; this.responseText = texto; this._resp = new Headers(cab);
      this.response = this.responseType === 'json' ? (function () { try { return JSON.parse(texto); } catch (e) { return null; } })() : texto;
      this.readyState = 4;
      const e = new Event('readystatechange'); this.dispatchEvent(e);
      this.dispatchEvent(new Event(status ? 'load' : 'error')); this.dispatchEvent(new Event('loadend'));
    }
  }
  XMLHttpRequest.DONE = 4;
  g.XMLHttpRequest = XMLHttpRequest;
  // ---------------- observadores (o layout e estatico: avisam uma vez)
  g.MutationObserver = class { constructor(f) { this.f = f; } observe() {} disconnect() {} takeRecords() { return []; } };
  g.IntersectionObserver = class { constructor(f, o) { this.f = f; this.alvos = []; } observe(el) { this.alvos.push(el); const f = this.f, eu = this; setTimeout(() => f([{ target: el, isIntersecting: true, intersectionRatio: 1, boundingClientRect: el.getBoundingClientRect(), intersectionRect: el.getBoundingClientRect(), rootBounds: null, time: performance.now() }], eu), 0); } unobserve() {} disconnect() {} takeRecords() { return []; } };
  g.ResizeObserver = class { constructor(f) { this.f = f; } observe(el) { const f = this.f, eu = this; setTimeout(() => { const r = el.getBoundingClientRect(); f([{ target: el, contentRect: r, borderBoxSize: [{ inlineSize: r.width, blockSize: r.height }], contentBoxSize: [{ inlineSize: r.width, blockSize: r.height }] }], eu); }, 0); } unobserve() {} disconnect() {} };
  g.PerformanceObserver = class { constructor() {} observe() {} disconnect() {} };
  PerformanceObserver.supportedEntryTypes = [];
  // ---------------- texto e bytes
  g.TextEncoder = class { encode(s) { s = String(s); const u = unescape(encodeURIComponent(s)); const a = new Uint8Array(u.length); for (let i = 0; i < u.length; i++) a[i] = u.charCodeAt(i); return a; } get encoding() { return 'utf-8'; } };
  g.TextDecoder = class { constructor(e) { this.encoding = e || 'utf-8'; } decode(a) { if (!a) return ''; let s = ''; for (let i = 0; i < a.length; i++) s += String.fromCharCode(a[i]); try { return decodeURIComponent(escape(s)); } catch (e) { return s; } } };
  function tipado(nome, por) {
    g[nome] = function (x, off, len) {
      let a;
      if (typeof x === 'number') { a = new Array(x).fill(0); }
      else if (x && x._bytes) { a = x._bytes; }
      else if (x && x.length !== undefined) { a = Array.from(x, v => Number(v) || 0); }
      else a = [];
      a.BYTES_PER_ELEMENT = por; a.buffer = { byteLength: a.length * por, _bytes: a }; a.byteLength = a.length * por; a.byteOffset = 0;
      a.subarray = function (i, j) { return g[nome](Array.prototype.slice.call(this, i, j)); };
      a.set = function (src, o) { o = o || 0; for (let i = 0; i < src.length; i++) this[o + i] = src[i]; };
      return a;
    };
    g[nome].BYTES_PER_ELEMENT = por;
    g[nome].from = function (x, f) { return g[nome](f ? Array.from(x, f) : x); };
    g[nome].of = function () { return g[nome](Array.from(arguments)); };
  }
  for (const [n, b] of [['Uint8Array', 1], ['Uint8ClampedArray', 1], ['Int8Array', 1], ['Uint16Array', 2], ['Int16Array', 2], ['Uint32Array', 4], ['Int32Array', 4], ['Float32Array', 4], ['Float64Array', 8], ['BigInt64Array', 8], ['BigUint64Array', 8]]) tipado(n, b);
  g.ArrayBuffer = function (n) { this.byteLength = n; this._bytes = new Array(n).fill(0); };
  g.ArrayBuffer.isView = function (x) { return !!(x && x.BYTES_PER_ELEMENT); };
  g.DataView = function (b) { this.buffer = b; this.byteLength = b.byteLength; };
  g.Blob = class Blob { constructor(p, o) { this._t = (p || []).join(''); this.size = this._t.length; this.type = (o && o.type) || ''; } text() { return Promise.resolve(this._t); } slice() { return this; } };
  g.File = class File extends Blob { constructor(p, n, o) { super(p, o); this.name = n; } };
  g.FileReader = class extends EventTarget { readAsText(b) { this.result = b._t; setTimeout(() => this.dispatchEvent(new Event('load')), 0); } readAsDataURL(b) { this.result = 'data:,' + encodeURIComponent(b._t); setTimeout(() => this.dispatchEvent(new Event('load')), 0); } };
  g.FormData = class FormData { constructor() { this._p = []; } append(k, v) { this._p.push([k, v]); } get(k) { const x = this._p.find(p => p[0] === k); return x ? x[1] : null; } has(k) { return this._p.some(p => p[0] === k); } set(k, v) { this.delete(k); this.append(k, v); } delete(k) { this._p = this._p.filter(p => p[0] !== k); } entries() { return this._p[Symbol.iterator](); } [Symbol.iterator]() { return this.entries(); } };
  // ---------------- crypto e Intl
  g.crypto = { getRandomValues(a) { for (let i = 0; i < a.length; i++) a[i] = Math.floor(Math.random() * 256); return a; },
    randomUUID() { const h = '0123456789abcdef'; let s = ''; for (let i = 0; i < 36; i++) s += [8, 13, 18, 23].includes(i) ? '-' : i === 14 ? '4' : h[Math.floor(Math.random() * 16)]; return s; }, subtle: {} };
  g.Intl = {
    NumberFormat: class { constructor(l, o) { this.o = o || {}; } format(n) { if (this.o.style === 'percent') return Math.round(n * 100) + '%'; let s = Number(n).toLocaleString(); if (this.o.style === 'currency') s = (this.o.currency === 'BRL' ? 'R$ ' : '$') + s; return s; } formatToParts(n) { return [{ type: 'integer', value: this.format(n) }]; } resolvedOptions() { return { locale: 'pt-BR' }; } },
    DateTimeFormat: class { constructor(l, o) { this.o = o || {}; } format(d) { return (d === undefined ? new Date() : new Date(d)).toLocaleDateString(); } formatToParts(d) { return [{ type: 'literal', value: this.format(d) }]; } resolvedOptions() { return { timeZone: 'UTC', locale: 'pt-BR' }; } },
    PluralRules: class { select(n) { return n === 1 ? 'one' : 'other'; } },
    RelativeTimeFormat: class { format(n, u) { return n + ' ' + u; } },
    Collator: class { compare(a, b) { return String(a).localeCompare(String(b)); } },
    ListFormat: class { format(l) { return l.join(', '); } },
    Segmenter: class { segment(s) { return Array.from(s).map((c, i) => ({ segment: c, index: i })); } },
    getCanonicalLocales(l) { return [].concat(l || []); },
    supportedValuesOf() { return []; },
  };
  // ---------------- miudezas
  g.customElements = { define() {}, get() { return undefined; }, whenDefined() { return Promise.resolve(); } };
  g.CSS = { supports() { return true; }, escape(s) { return String(s).replace(/([^\w-])/g, '\\$1'); } };
  g.Image = function (w, h) { const i = document.createElement('img'); if (w) i.width = w; if (h) i.height = h; return i; };
  g.Option = function (t, v) { const o = document.createElement('option'); o.textContent = t || ''; if (v !== undefined) o.value = v; return o; };
  g.origin = loc.origin; g.isSecureContext = loc.protocol === 'https:'; g.name = '';
  g.onload = null; g.onerror = null;
  g.indexedDB = undefined; g.caches = undefined;
  g.BroadcastChannel = class { postMessage() {} close() {} addEventListener() {} };
  g.Worker = class extends EventTarget { postMessage() {} terminate() {} };
  g.MessageChannel = class { constructor() { this.port1 = new EventTarget(); this.port2 = new EventTarget(); this.port1.postMessage = this.port2.postMessage = function (m) { const alvo = this === eu.port1 ? eu.port2 : eu.port1; setTimeout(() => alvo.dispatchEvent(Object.assign(new Event('message'), { data: m })), 0); }; const eu = this; this.port1.start = this.port2.start = function () {}; } };
  // ---------------- modulos ES (o texto vira funcao em js::modulo)
  const modulos = Object.create(null);
  g.__defExp = function (ns, gs) { for (const k in gs) Object.defineProperty(ns, k, { get: gs[k], enumerable: true, configurable: true }); };
  g.__estrela = function (ns, m) { for (const k of Object.keys(m)) if (k !== 'default' && !(k in ns)) Object.defineProperty(ns, k, { get: () => m[k], enumerable: true, configurable: true }); };
  function registrar(url, texto) {
    const m = modulos[url];
    try { const c = __compilarModulo(url, texto); m.f = c.f; m.deps = c.deps; }
    catch (e) { console.error('erro: módulo ' + url + ': ' + e); m.deps = []; }
    for (const d of m.deps) buscarModulo(d);
  }
  // busca e compila um modulo (as dependencias comecam a vir em paralelo)
  function buscarModulo(url, texto) {
    let m = modulos[url];
    if (m) return m.promessa;
    m = modulos[url] = { ns: Object.create(null), estado: 0, deps: [], f: null };
    m.promessa = texto != null ? Promise.resolve(registrar(url, texto))
      : fetch(url).then(r => r.text()).then(t => registrar(url, t), e => { console.error('erro: módulo ' + url + ' não carregou'); });
    return m.promessa;
  }
  // espera o modulo e o grafo todo de dependencias (ciclos nao travam)
  function carregarModulo(url, texto, vistos) {
    vistos = vistos || Object.create(null);
    if (vistos[url]) return Promise.resolve();
    vistos[url] = true;
    return buscarModulo(url, texto).then(() => Promise.all(modulos[url].deps.map(d => carregarModulo(d, null, vistos))));
  }
  function avaliarModulo(url) {
    const m = modulos[url];
    if (!m) return Object.create(null);
    if (m.estado) return m.ns;
    m.estado = 1;
    for (const d of m.deps) avaliarModulo(d);
    if (m.f) {
      const falhou = e => console.error('erro: ' + e + ' [' + url.split('/').pop() + '] ' + (e && e.stack || ''));
      try { const p = m.f(m.ns, d => avaliarModulo(d), { url: url, resolve: s => resolverEspec(s, url) }); if (p && p.then) p.then(null, falhou); }
      catch (e) { falhou(e); }
    }
    m.estado = 2;
    return m.ns;
  }
  g.__carregarModulo = function (url, texto) { return carregarModulo(url, texto).then(() => avaliarModulo(url)); };
  g.__mapaImport = Object.create(null);
  g.__definirMapa = function (t) {
    try { const im = JSON.parse(t).imports || {}; for (const k in im) __mapaImport[k] = new URL(im[k], location.href).href; }
    catch (e) { console.error('erro: importmap: ' + e); }
  };
  function resolverEspec(spec, base) {
    spec = String(spec);
    if (spec in __mapaImport) return __mapaImport[spec];
    let melhor = '';
    for (const k in __mapaImport) if (k.endsWith('/') && spec.startsWith(k) && k.length > melhor.length) melhor = k;
    if (melhor) return __mapaImport[melhor] + spec.slice(melhor.length);
    return new URL(spec, base || location.href).href;
  }
  g.__importarDe = function (base, spec) {
    const u = resolverEspec(spec, base);
    return carregarModulo(u).then(() => { const m = modulos[u]; if (!m || !m.f) throw new TypeError('módulo não carregou: ' + u); return avaliarModulo(u); });
  };
  g.WebSocket = class extends EventTarget { constructor() { super(); this.readyState = 3; setTimeout(() => this.dispatchEvent(new Event('error')), 0); } send() {} close() {} };
})(globalThis);
"#;
