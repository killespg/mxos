//! Os scripts de uma pagina: acha os <script> na ordem do documento, roda
//! (os externos o anfitriao busca), dispara DOMContentLoaded/load, os
//! timers e as respostas de fetch/XHR, e eventos de clique e teclado.
//! Quem usa (navegador do kernel, testes no PC) so precisa buscar URLs e
//! refazer o layout quando `sujo()`.

use alloc::collections::BTreeSet;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::web::atomo::{Atomo, a};
use crate::web::dom::{Documento, Id, NENHUM, RAIZ};
use crate::web::js::dom::{self as jsdom, JANELA, com_dom};
use crate::web::js::valor::{Valor, chave};
use crate::web::js::vm::Vm;

/// Passos de bytecode por script/callback antes de desistir (laco infinito
/// nao pode travar a box).
const LIMITE_PASSOS: u64 = 40_000_000;
/// Timers por tique (setInterval de 0 ms nao pode prender o tique).
const MAX_TIMERS_TIQUE: usize = 64;

pub enum FonteScript {
    Inline(String),
    Externo(String),
}

/// Uma resposta HTTP para fetch/XHR: status, corpo, cabecalhos.
pub struct Resposta {
    pub status: u16,
    pub corpo: String,
    pub cabecalhos: Vec<(String, String)>,
    pub url: String,
}

/// Pedido de rede feito por um script.
pub struct PedidoRede<'a> {
    pub metodo: &'a str,
    pub url: &'a str,
    pub corpo: Option<&'a str>,
    pub cabecalhos: &'a [(String, String)],
}

pub struct Scripts {
    pub vm: Vm,
    executados: BTreeSet<Id>,
}

impl Scripts {
    pub fn novo(doc: Rc<RefCell<Documento>>, url: &str, largura: i32, altura: i32) -> Scripts {
        let mut vm = Vm::novo();
        jsdom::instalar(&mut vm, doc, url, largura as f32, altura as f32);
        Scripts { vm, executados: BTreeSet::new() }
    }

    fn doc(&mut self) -> Option<Rc<RefCell<Documento>>> {
        com_dom(&mut self.vm, |_, d| d.doc.clone())
    }

    /// O proximo <script> ainda nao rodado, na ordem do documento (scripts
    /// inseridos por outros scripts entram na fila tambem).
    pub fn proximo(&mut self) -> Option<(Id, FonteScript)> {
        let doc_rc = self.doc()?;
        let doc = doc_rc.borrow();
        for id in doc.descendentes(RAIZ) {
            let Some(e) = doc.elemento(id) else { continue };
            if !e.eh(a::SCRIPT) || self.executados.contains(&id) {
                continue;
            }
            self.executados.insert(id);
            let tipo = e.atributo(a::TYPE).unwrap_or("").trim().to_ascii_lowercase();
            let eh_js = tipo.is_empty()
                || tipo == "module"
                || tipo.contains("javascript")
                || tipo.contains("ecmascript")
                || tipo == "text/jscript";
            if !eh_js || e.tem(Atomo::de("nomodule")) {
                continue;
            }
            return Some(match e.atributo(a::SRC) {
                Some(s) if !s.trim().is_empty() => (id, FonteScript::Externo(String::from(s.trim()))),
                _ => (id, FonteScript::Inline(doc.texto(id))),
            });
        }
        None
    }

    /// Roda um script (erros vao para o console).
    pub fn rodar(&mut self, id: Id, fonte: &str) {
        com_dom(&mut self.vm, |_, d| d.script_atual = id);
        self.vm.passos = 0;
        self.vm.limite_passos = LIMITE_PASSOS;
        if let Err(e) = crate::web::js::avaliar(&mut self.vm, &fonte) {
            let m = crate::web::js::descrever_erro(&mut self.vm, &e);
            self.vm.console.push_str(&format!("erro: {m}\n"));
        }
        self.vm.rodar_microtarefas();
        self.vm.limite_passos = 0;
        com_dom(&mut self.vm, |_, d| d.script_atual = NENHUM);
        // document.write: o HTML entra logo depois do script
        let escrito = com_dom(&mut self.vm, |_, d| core::mem::take(&mut d.escrito)).unwrap_or_default();
        if !escrito.is_empty() {
            if let Some(doc_rc) = self.doc() {
                let mut doc = doc_rc.borrow_mut();
                let novos = doc.fragmento(&escrito);
                let pai = doc.pai(id);
                let depois = if pai == NENHUM { NENHUM } else { doc.no(id).proximo };
                let pai = if pai == NENHUM { doc.corpo().unwrap_or(RAIZ) } else { pai };
                for n in novos {
                    doc.inserir_antes(pai, n, depois);
                }
                drop(doc);
                com_dom(&mut self.vm, |_, d| d.sujo = true);
            }
        }
    }

    /// Roda todos os scripts, buscando os externos com `buscar`.
    pub fn rodar_todos(&mut self, buscar: &mut dyn FnMut(&str) -> Option<String>) {
        let base = self.url();
        // pagina com dezenas de scripts: limite para nao passar minutos nisso
        for _ in 0..200 {
            let Some((id, f)) = self.proximo() else { break };
            match f {
                FonteScript::Inline(t) => self.rodar(id, &t),
                FonteScript::Externo(u) => {
                    let u = crate::web::url::resolver(&base, &u);
                    match buscar(&u) {
                        Some(t) => {
                            self.rodar(id, &t);
                            jsdom::disparar(&mut self.vm, id, "load", &[]);
                        }
                        None => {
                            jsdom::disparar(&mut self.vm, id, "error", &[]);
                        }
                    }
                }
            }
        }
    }

    /// Depois dos scripts: DOMContentLoaded e load.
    pub fn carregou(&mut self) {
        jsdom::disparar(&mut self.vm, RAIZ, "readystatechange", &[]);
        jsdom::disparar(&mut self.vm, RAIZ, "DOMContentLoaded", &[]);
        jsdom::disparar(&mut self.vm, JANELA, "load", &[]);
        jsdom::disparar(&mut self.vm, JANELA, "pageshow", &[]);
    }

    pub fn url(&mut self) -> String {
        com_dom(&mut self.vm, |_, d| d.url.clone()).unwrap_or_default()
    }

    /// Avanca o relogio: roda timers vencidos e entrega respostas de rede.
    /// Devolve true se ainda ha timers (o anfitriao deve chamar de novo).
    pub fn tique(&mut self, agora: f64, buscar: &mut dyn FnMut(&PedidoRede) -> Option<Resposta>) -> bool {
        com_dom(&mut self.vm, |_, d| d.agora = agora);
        self.rede(buscar);
        for _ in 0..MAX_TIMERS_TIQUE {
            let t = com_dom(&mut self.vm, |_, d| {
                let i = d.timers.iter().enumerate().filter(|(_, t)| t.quando <= agora).min_by(|x, y| x.1.quando.total_cmp(&y.1.quando)).map(|(i, _)| i)?;
                let mut t = d.timers.remove(i);
                let r = (t.f.clone(), core::mem::take(&mut t.args));
                if let Some(iv) = t.intervalo {
                    t.quando = agora + iv;
                    t.args = r.1.clone();
                    d.timers.push(t);
                }
                Some(r)
            })
            .flatten();
            let Some((f, args)) = t else { break };
            self.chamar(&f, &args);
        }
        self.rede(buscar);
        com_dom(&mut self.vm, |_, d| !d.timers.is_empty()).unwrap_or(false)
    }

    /// Quando vence o proximo timer (ms), se houver.
    pub fn proximo_timer(&mut self) -> Option<f64> {
        com_dom(&mut self.vm, |_, d| d.timers.iter().map(|t| t.quando).min_by(|a, b| a.total_cmp(b))).flatten()
    }

    fn chamar(&mut self, f: &Valor, args: &[Valor]) {
        self.vm.passos = 0;
        self.vm.limite_passos = LIMITE_PASSOS;
        let g = Valor::Obj(self.vm.global);
        if let Err(e) = self.vm.chamar(f, &g, args) {
            let m = crate::web::js::descrever_erro(&mut self.vm, &e);
            self.vm.console.push_str(&format!("erro: {m}\n"));
        }
        self.vm.rodar_microtarefas();
        self.vm.limite_passos = 0;
    }

    /// Entrega os fetch/XHR pendentes.
    fn rede(&mut self, buscar: &mut dyn FnMut(&PedidoRede) -> Option<Resposta>) {
        for _ in 0..16 {
            let pedidos = com_dom(&mut self.vm, |_, d| core::mem::take(&mut d.pedidos)).unwrap_or_default();
            if pedidos.is_empty() {
                break;
            }
            for p in pedidos {
                let r = buscar(&PedidoRede { metodo: &p.metodo, url: &p.url, corpo: p.corpo.as_deref(), cabecalhos: &p.cabecalhos });
                let cab = self.vm.novo_objeto();
                let (status, corpo, url) = match &r {
                    Some(r) => {
                        if let Valor::Obj(o) = cab {
                            for (k, v) in &r.cabecalhos {
                                self.vm.definir_propria(o, chave(&k.to_ascii_lowercase()), Valor::texto(v));
                            }
                        }
                        (r.status, r.corpo.as_str(), r.url.as_str())
                    }
                    None => (0, "", p.url.as_str()),
                };
                self.vm.passos = 0;
                self.vm.limite_passos = LIMITE_PASSOS;
                if !matches!(p.promessa, Valor::Indef) {
                    if r.is_none() {
                        let e = self.vm.erro_tipo::<()>("Failed to fetch").err().unwrap_or_default();
                        self.vm.assentar(&p.promessa, e, true);
                    } else {
                        let g = Valor::Obj(self.vm.global);
                        let ctor = self.vm.pegar(&g, chave("Response")).unwrap_or_default();
                        let init = self.vm.novo_objeto();
                        if let Valor::Obj(o) = init {
                            self.vm.definir_propria(o, chave("status"), Valor::Num(status as f64));
                            self.vm.definir_propria(o, chave("headers"), cab.clone());
                            self.vm.definir_propria(o, chave("url"), Valor::texto(url));
                        }
                        match self.vm.construir(&ctor, &[Valor::texto(corpo), init], &ctor) {
                            Ok(resp) => self.vm.resolver_promessa(&p.promessa, resp),
                            Err(e) => self.vm.assentar(&p.promessa, e, true),
                        }
                    }
                } else if !matches!(p.xhr, Valor::Indef) {
                    if let Ok(fim) = self.vm.pegar(&p.xhr, chave("_fim")) {
                        if let Err(e) = self.vm.chamar(&fim, &p.xhr, &[Valor::Num(status as f64), Valor::texto(corpo), cab]) {
                            let m = crate::web::js::descrever_erro(&mut self.vm, &e);
                            self.vm.console.push_str(&format!("erro em XHR: {m}\n"));
                        }
                    }
                }
                self.vm.rodar_microtarefas();
                self.vm.limite_passos = 0;
            }
        }
    }

    /// Dispara um evento num no. Devolve false se o script chamou
    /// preventDefault (o clique nao deve navegar, o form nao envia...).
    pub fn evento(&mut self, no: Id, tipo: &str, extras: &[(&str, Valor)]) -> bool {
        self.vm.passos = 0;
        self.vm.limite_passos = LIMITE_PASSOS;
        let r = jsdom::disparar(&mut self.vm, no, tipo, extras);
        self.vm.limite_passos = 0;
        r
    }

    /// Clique de verdade: mousedown, mouseup, click (com coordenadas).
    pub fn clique(&mut self, no: Id, x: i32, y: i32) -> bool {
        let pos = [("clientX", Valor::Num(x as f64)), ("clientY", Valor::Num(y as f64)), ("button", Valor::Num(0.0)), ("cancelable", Valor::Bool(true))];
        self.evento(no, "pointerdown", &pos);
        self.evento(no, "mousedown", &pos);
        com_dom(&mut self.vm, |_, d| d.foco = no);
        self.evento(no, "pointerup", &pos);
        self.evento(no, "mouseup", &pos);
        self.evento(no, "click", &pos)
    }

    /// Uma tecla no elemento com foco (keydown/keypress/keyup); texto
    /// digitado vai para o value de input/textarea.
    pub fn tecla(&mut self, tecla: &str, texto: Option<char>) -> bool {
        let alvo = com_dom(&mut self.vm, |_, d| d.foco).unwrap_or(NENHUM);
        let alvo = if alvo == NENHUM { self.doc().and_then(|d| d.borrow().corpo()).unwrap_or(RAIZ) } else { alvo };
        let ext = [("key", Valor::texto(tecla)), ("code", Valor::texto(tecla)), ("cancelable", Valor::Bool(true))];
        let ok = self.evento(alvo, "keydown", &ext);
        if ok {
            if let Some(doc_rc) = self.doc() {
                let editavel = {
                    let doc = doc_rc.borrow();
                    doc.eh(alvo, a::INPUT) || doc.eh(alvo, a::TEXTAREA)
                };
                if editavel {
                    {
                        let mut doc = doc_rc.borrow_mut();
                        let eh_area = doc.eh(alvo, a::TEXTAREA);
                        let mut v = if eh_area { doc.texto(alvo) } else { doc.elemento(alvo).and_then(|e| e.atributo(a::VALUE)).map(String::from).unwrap_or_default() };
                        match (tecla, texto) {
                            ("Backspace", _) => {
                                v.pop();
                            }
                            (_, Some(c)) => v.push(c),
                            _ => {}
                        }
                        if eh_area {
                            let filhos: Vec<Id> = doc.filhos(alvo).collect();
                            for f in filhos {
                                doc.soltar(f);
                            }
                            doc.anexar_texto(alvo, &v);
                        } else if let Some(e) = doc.elemento_mut(alvo) {
                            e.definir(a::VALUE, &v);
                        }
                    }
                    com_dom(&mut self.vm, |_, d| d.sujo = true);
                    self.evento(alvo, "input", &[]);
                    if tecla == "Enter" {
                        self.enviar_form(alvo);
                    }
                }
            }
        }
        self.evento(alvo, "keyup", &ext);
        ok
    }

    /// Enter num campo: dispara submit no form (se ninguem impedir, o
    /// anfitriao navega para a URL montada em `navegacao()`).
    fn enviar_form(&mut self, campo: Id) {
        let Some(doc_rc) = self.doc() else { return };
        let form = {
            let doc = doc_rc.borrow();
            let mut x = campo;
            while x != NENHUM && !doc.eh(x, a::FORM) {
                x = doc.pai(x);
            }
            x
        };
        if form == NENHUM || !self.evento(form, "submit", &[("cancelable", Valor::Bool(true))]) {
            return;
        }
        let destino = {
            let doc = doc_rc.borrow();
            let e = doc.elemento(form);
            let acao = e.and_then(|e| e.atributo(Atomo::de("action"))).unwrap_or("");
            let mut q = String::new();
            for id in doc.descendentes(form) {
                let Some(c) = doc.elemento(id) else { continue };
                if !(c.eh(a::INPUT) || c.eh(a::TEXTAREA) || c.eh(a::SELECT)) {
                    continue;
                }
                let Some(nome) = c.atributo(a::NAME) else { continue };
                let tipo = c.atributo(a::TYPE).unwrap_or("").to_ascii_lowercase();
                if matches!(tipo.as_str(), "submit" | "button" | "image" | "file" | "reset") || ((tipo == "checkbox" || tipo == "radio") && !c.tem(a::CHECKED)) {
                    continue;
                }
                let v = if c.eh(a::TEXTAREA) { doc.texto(id) } else { String::from(c.atributo(a::VALUE).unwrap_or(if tipo == "checkbox" { "on" } else { "" })) };
                if !q.is_empty() {
                    q.push('&');
                }
                q.push_str(&codificar(nome));
                q.push('=');
                q.push_str(&codificar(&v));
            }
            let base = acao.split('?').next().unwrap_or("");
            format!("{base}?{q}")
        };
        com_dom(&mut self.vm, |_, d| d.navegar = Some(crate::web::url::resolver(&d.url, &destino)));
    }

    /// O DOM mudou desde a ultima vez? (zera o aviso)
    pub fn sujo(&mut self) -> bool {
        com_dom(&mut self.vm, |_, d| core::mem::take(&mut d.sujo)).unwrap_or(false)
    }

    /// Algum script pediu para ir a outra URL?
    pub fn navegacao(&mut self) -> Option<String> {
        com_dom(&mut self.vm, |_, d| d.navegar.take()).flatten()
    }

    /// Passa a geometria do layout (getBoundingClientRect etc).
    pub fn geometria(&mut self, g: alloc::collections::BTreeMap<Id, (f32, f32, f32, f32)>) {
        com_dom(&mut self.vm, |_, d| d.geometria = g);
    }

    pub fn rolagem(&mut self, y: i32) {
        com_dom(&mut self.vm, |_, d| d.rolagem = y as f32);
        let g = self.vm.global;
        for n in ["scrollY", "pageYOffset"] {
            self.vm.definir(g, n, Valor::Num(y as f64));
        }
    }

    /// Tira o texto do console (console.log e erros).
    pub fn console(&mut self) -> String {
        core::mem::take(&mut self.vm.console)
    }

    /// Libera lixo do JS (chamar fora de qualquer script).
    pub fn coletar(&mut self) -> usize {
        self.vm.coletar()
    }
}

/// application/x-www-form-urlencoded
fn codificar(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => o.push(b as char),
            b' ' => o.push('+'),
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}
