//! JavaScript mexendo no DOM: document, eventos, timers, fetch.

use std::cell::RefCell;
use std::rc::Rc;

use teste_web::web::html;
use teste_web::web::script::{Resposta, Scripts};

fn pagina(html_: &str) -> (Rc<RefCell<teste_web::web::dom::Documento>>, Scripts) {
    let doc = Rc::new(RefCell::new(html::analisar(html_)));
    let mut s = Scripts::novo(doc.clone(), "https://exemplo.com/dir/pag.html?q=1", 1280, 720);
    s.rodar_todos(&mut |_| None);
    s.carregou();
    (doc, s)
}

fn avaliar(s: &mut Scripts, codigo: &str) -> String {
    match teste_web::web::js::avaliar(&mut s.vm, codigo) {
        Ok(v) => s.vm.para_texto(&v).map(|t| t.to_string()).unwrap_or_default(),
        Err(e) => format!("ERRO: {}", teste_web::web::js::descrever_erro(&mut s.vm, &e)),
    }
}

fn caso(s: &mut Scripts, codigo: &str, esperado: &str) {
    eprintln!("CASO {codigo}");
    let r = avaliar(s, codigo);
    assert_eq!(r, esperado, "\ncodigo: {codigo}\nconsole: {}", s.vm.console);
}

#[test]
fn consultas_e_propriedades() {
    let (_, mut s) = pagina(
        r#"<!DOCTYPE html><html><head><title>Teste</title></head><body>
        <div id="a" class="x y" data-nome-longo="v"><p>um</p><p class="y">dois</p></div>
        <ul><li>1</li><li>2</li><li>3</li></ul>
        <input id="campo" value="oi"><a id="l" href="/outro">link</a></body></html>"#,
    );
    let c = console(&mut s);
    assert!(c.is_empty(), "{c}");
    caso(&mut s, "document.title", "Teste");
    caso(&mut s, "document.getElementById('a').tagName", "DIV");
    caso(&mut s, "document.querySelectorAll('li').length", "3");
    caso(&mut s, "document.querySelector('#a p.y').textContent", "dois");
    caso(&mut s, "document.getElementById('a').className", "x y");
    caso(&mut s, "document.getElementById('a').classList.contains('y')", "true");
    caso(&mut s, "document.getElementById('a').dataset.nomeLongo", "v");
    caso(&mut s, "document.getElementById('campo').value", "oi");
    caso(&mut s, "document.getElementById('l').href", "https://exemplo.com/outro");
    caso(&mut s, "document.body === document.body", "true");
    caso(&mut s, "document.querySelector('li').nextElementSibling.textContent", "2");
    caso(&mut s, "document.querySelector('p').parentNode.id", "a");
    caso(&mut s, "document.getElementsByTagName('p').length", "2");
    caso(&mut s, "document.querySelector('li') instanceof Element", "true");
    caso(&mut s, "document.querySelector('p').closest('div').id", "a");
    caso(&mut s, "document.querySelector('p').matches('#a > p')", "true");
    caso(&mut s, "location.hostname + location.pathname + location.search", "exemplo.com/dir/pag.html?q=1");
    caso(&mut s, "new URL('../x?y=2', location.href).href", "https://exemplo.com/x?y=2");
    caso(&mut s, "new URLSearchParams('a=1&b=2').get('b')", "2");
}

fn console(s: &mut Scripts) -> String {
    s.console()
}

#[test]
fn mudancas_no_dom() {
    let (doc, mut s) = pagina("<body><div id=caixa></div></body>");
    caso(
        &mut s,
        "const c = document.getElementById('caixa');
         const p = document.createElement('p'); p.textContent = 'novo'; p.id = 'p1';
         c.appendChild(p);
         c.insertAdjacentHTML('beforeend', '<span class=s>a</span><span class=s>b</span>');
         c.innerHTML.length > 0 && document.querySelectorAll('.s').length",
        "2",
    );
    assert!(s.sujo());
    caso(&mut s, "document.getElementById('caixa').innerHTML", r#"<p id="p1">novo</p><span class="s">a</span><span class="s">b</span>"#);
    caso(&mut s, "document.querySelector('.s').remove(); document.querySelectorAll('.s').length", "1");
    caso(&mut s, "const d = document.getElementById('caixa'); d.innerHTML = '<b>x</b>'; d.firstChild.tagName", "B");
    caso(&mut s, "document.getElementById('caixa').style.color = 'red'; 0", "0");
    caso(&mut s, "const e = document.getElementById('caixa'); e.style.setProperty('margin-top', '4px'); e.getAttribute('style')", "color: red; margin-top: 4px");
    caso(&mut s, "const f = document.createDocumentFragment(); f.append('t1', document.createElement('i')); document.body.append(f); document.body.lastChild.tagName", "I");
    caso(&mut s, "document.getElementById('caixa').cloneNode(true).outerHTML", r#"<div id="caixa" style="color: red; margin-top: 4px"><b>x</b></div>"#);
    caso(&mut s, "document.getElementById('caixa').classList.add('k', 'z'); document.getElementById('caixa').className", "k z");
    caso(&mut s, "document.getElementById('caixa').classList.toggle('k'); document.getElementById('caixa').className", "z");
    let d = doc.borrow();
    let corpo = d.corpo().unwrap();
    assert!(d.texto(corpo).contains("t1"));
}

#[test]
fn eventos() {
    let (_, mut s) = pagina(
        r#"<body><div id=fora><button id=b onclick="window.inline = (window.inline||0) + 1">ok</button></div>
        <a id=l href="/x">x</a>
        <script>
          var log = [];
          document.getElementById('fora').addEventListener('click', e => log.push('fora:' + e.target.id));
          document.getElementById('b').addEventListener('click', function (e) { log.push('botao:' + this.id); });
          document.getElementById('l').addEventListener('click', e => e.preventDefault());
          document.addEventListener('DOMContentLoaded', () => log.push('dcl'));
          window.addEventListener('load', () => log.push('load'));
          document.getElementById('b').addEventListener('custom', e => log.push('c' + e.detail), { once: true });
        </script></body>"#,
    );
    caso(&mut s, "log.join()", "dcl,load");
    caso(&mut s, "document.getElementById('b').click(); log.join()", "dcl,load,botao:b,fora:b");
    caso(&mut s, "window.inline", "1");
    caso(&mut s, "const ev = new CustomEvent('custom', {detail: 7}); const b = document.getElementById('b'); b.dispatchEvent(ev); b.dispatchEvent(new CustomEvent('custom', {detail: 8})); log[log.length - 1]", "c7");
    let l = avaliar(&mut s, "document.getElementById('l')");
    assert!(!l.starts_with("ERRO"));
    let id = teste_web::web::dom::Documento::por_id(&s_doc(&mut s).borrow(), "l").unwrap();
    assert!(!s.clique(id, 1, 1), "preventDefault deveria cancelar a navegacao");
    caso(&mut s, "document.getElementById('b').onclick = () => log.push('prop'); document.getElementById('b').click(); log[log.length - 2]", "prop");
}

fn s_doc(s: &mut Scripts) -> Rc<RefCell<teste_web::web::dom::Documento>> {
    teste_web::web::js::dom::com_dom(&mut s.vm, |_, d| d.doc.clone()).unwrap()
}

#[test]
fn timers_e_rede() {
    let (_, mut s) = pagina(
        r#"<body><script>
          var t = [];
          setTimeout(() => t.push('a'), 100);
          setTimeout(() => t.push('b'), 10);
          const iv = setInterval(() => { t.push('i'); if (t.filter(x => x == 'i').length == 3) clearInterval(iv); }, 20);
          Promise.resolve().then(() => t.push('micro'));
          fetch('/api/dados.json').then(r => r.json()).then(j => t.push('json:' + j.v));
          const x = new XMLHttpRequest(); x.open('GET', 'texto.txt'); x.onload = () => t.push('xhr:' + x.responseText); x.send();
        </script></body>"#,
    );
    let mut pedidos = Vec::new();
    let mut agora = 0.0;
    while agora <= 200.0 {
        s.tique(agora, &mut |p| {
            pedidos.push(p.url.to_string());
            let corpo = if p.url.ends_with(".json") { r#"{"v": 42}"# } else { "ola" };
            Some(Resposta { status: 200, corpo: corpo.into(), cabecalhos: vec![("Content-Type".into(), "text/plain".into())], url: p.url.into() })
        });
        agora += 5.0;
    }
    assert_eq!(pedidos, vec!["https://exemplo.com/api/dados.json".to_string(), "https://exemplo.com/dir/texto.txt".to_string()]);
    caso(&mut s, "t.join()", "micro,json:42,xhr:ola,b,i,i,i,a");
    assert!(s.proximo_timer().is_none());
}

#[test]
fn scripts_externos_e_write() {
    let doc = Rc::new(RefCell::new(html::analisar(
        r#"<body><p id=a>a</p><script src="lib.js"></script><script>document.write('<p id=w>escrito</p>'); var depois = window.Lib;</script><p id=z>z</p></body>"#,
    )));
    let mut s = Scripts::novo(doc.clone(), "https://site.org/", 800, 600);
    let mut pedidos = Vec::new();
    s.rodar_todos(&mut |u| {
        pedidos.push(u.to_string());
        Some("window.Lib = { versao: 3 }; const el = document.createElement('script'); el.textContent = 'window.dinamico = 1'; document.body.appendChild(el);".into())
    });
    assert_eq!(pedidos, vec!["https://site.org/lib.js"]);
    caso(&mut s, "depois.versao", "3");
    caso(&mut s, "window.dinamico", "1");
    caso(&mut s, "document.getElementById('w').previousElementSibling.tagName + document.getElementById('w').nextElementSibling.id", "SCRIPTz");
    caso(&mut s, "typeof document.getElementById('w')", "object");
}

#[test]
fn formulario_e_teclado() {
    let (_, mut s) = pagina(r#"<body><form action="/busca"><input id=q name=q><input type=hidden name=l value=pt></form></body>"#);
    let q = s_doc(&mut s).borrow().por_id("q").unwrap();
    s.clique(q, 0, 0);
    for c in "a b".chars() {
        s.tecla(&c.to_string(), Some(c));
    }
    caso(&mut s, "document.getElementById('q').value", "a b");
    s.tecla("Enter", None);
    assert_eq!(s.navegacao().as_deref(), Some("https://exemplo.com/busca?q=a+b&l=pt"));
}

#[test]
fn pagina_com_layout() {
    use teste_web::web::pagina::Pagina;
    let doc = Rc::new(RefCell::new(html::analisar(
        r#"<body style="margin:0"><div id=d style="width:200px;height:50px"></div><script>document.getElementById('d').style.height = '120px'</script></body>"#,
    )));
    let fontes = teste_web::fontes();
    let mut pag = Pagina::montar_rc(doc.clone(), Vec::new(), 800, 600, &fontes, &|_| None);
    let mut s = Scripts::novo(doc, "https://a.b/", 800, 600);
    s.rodar_todos(&mut |_| None);
    assert!(s.sujo());
    pag.refazer(&fontes, &|_| None);
    s.geometria(pag.geometria());
    caso(&mut s, "const r = document.getElementById('d').getBoundingClientRect(); r.width + 'x' + r.height", "200x120");
    caso(&mut s, "document.getElementById('d').offsetHeight", "120");
}

#[test]
fn arquivo_externo() {
    let Ok(a) = std::fs::read_to_string("C:/dev/mxos-tmp/arquivo_dom.js") else { return };
    let (_, mut s) = pagina("<body></body>");
    println!("ANTES {}", avaliar(&mut s, "typeof t + typeof e + Object.keys(globalThis).filter(k => k.length < 3).join()"));
    s.rodar(0, &a);
    println!("CONSOLE {}", s.console());
    println!("TYPEOF {}", avaliar(&mut s, "typeof t + ' ' + typeof e"));
}

#[test]
fn modulos_es() {
    let doc = Rc::new(RefCell::new(html::analisar(
        r#"<body><script type="module">
            import padrao, { dobro as d, NOME } from './lib/a.js';
            import * as tudo from './lib/b.js';
            window.r = [padrao, d(21), NOME, tudo.b, tudo.dobro(1), typeof tudo.default].join();
            import('./lib/b.js').then(m => { window.din = m.b; });
        </script>
        <script type="module">let t = 1; export const x = t;</script>
        <script>var t = 'classico';</script></body>"#,
    )));
    let mut s = Scripts::novo(doc, "https://site.org/pasta/", 800, 600);
    let mut pedidos = Vec::new();
    s.rodar_todos(&mut |u| {
        pedidos.push(u.to_string());
        Some(match u {
            "https://site.org/pasta/lib/a.js" => "import { b } from './b.js';\nexport function dobro(x) { return x * 2 }\nexport const NOME = 'a' + b;\nexport default 'P';".into(),
            "https://site.org/pasta/lib/b.js" => "export let b = 7; export { dobro } from './a.js'; export * from './c.js';".into(),
            "https://site.org/pasta/lib/c.js" => "export const c = 3;".into(),
            _ => return None,
        })
    });
    s.carregou();
    let c = s.console();
    assert!(!c.contains("erro"), "{c}");
    caso(&mut s, "window.r", "P,42,a7,7,2,undefined");
    caso(&mut s, "window.din", "7");
    caso(&mut s, "t", "classico");
    assert_eq!(pedidos.len(), 3, "{pedidos:?}");
}
