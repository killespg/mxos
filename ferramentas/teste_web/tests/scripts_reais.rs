//! Os scripts de sites reais rodando no nosso JS (mostra os erros).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use teste_web::web::html::{self, codificacao};
use teste_web::web::script::{Resposta, Scripts};

fn rodar(nome: &str, url: &str) {
    let Ok((tipo, bytes)) = teste_web::rede::pagina(url) else { return };
    let doc = Rc::new(RefCell::new(html::analisar_bytes(&bytes, Some(&tipo))));
    let t = Instant::now();
    let mut s = Scripts::novo(doc.clone(), url, 1280, 800);
    if std::env::var("PERFIL").is_ok() {
        unsafe { teste_web::web::js::embutidos::RELOGIO = Some(relogio) };
        s.vm.perfil = Some(Default::default());
    }
    let mut n = 0;
    let mut bytes_js = 0;
    let mut ultimo = (String::from("inicio"), Instant::now());
    s.rodar_todos(&mut |u| {
        let dt = ultimo.1.elapsed().as_secs_f64() * 1000.0;
        if dt > 150.0 {
            println!("    lento: {:.0} ms depois de {}", dt, ultimo.0);
        }
        ultimo = (u.to_string(), Instant::now());
        n += 1;
        if std::env::var("JSLOG").is_ok() { eprintln!("JS {u}"); }
        let (t, b) = teste_web::rede::pagina(u).ok()?;
        bytes_js += b.len();
        Some(codificacao::texto(&b, Some(&t)))
    });
    let t_scripts = t.elapsed().as_secs_f64() * 1000.0;
    s.carregou();
    let t_carregou = t.elapsed().as_secs_f64() * 1000.0;
    let mut agora = 0.0;
    while agora < 2000.0 {
        s.tique(agora, &mut |p| {
            if p.metodo != "GET" {
                return None;
            }
            let (t, b) = teste_web::rede::pagina(p.url).ok()?;
            Some(Resposta { status: 200, corpo: codificacao::texto(&b, Some(&t)), cabecalhos: vec![("content-type".into(), t)], url: p.url.into() })
        });
        agora += 50.0;
    }
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    let console = s.console();
    let erros: Vec<&str> = console.lines().filter(|l| l.starts_with("erro")).collect();
    println!("{nome:10} {n:3} externos, {:6} KB de JS, {ms:8.1} ms (scripts {t_scripts:.0}, load {:.0}, timers {:.0}), {} erros, heap {}", bytes_js / 1024, t_carregou - t_scripts, ms - t_carregou, erros.len(), s.vm.heap.vivos());
    if let Some(p) = &s.vm.perfil {
        let mut v: Vec<_> = p.iter().collect();
        v.sort_by(|a, b| b.1 .1.total_cmp(&a.1 .1));
        for (id, (n, ms)) in v.into_iter().take(20) {
            println!("    perfil {:30} {:9} chamadas {:9.1} ms", s.vm.nomes_nativas.get(*id as usize).map(|x| &**x).unwrap_or("?"), n, ms);
        }
    }
    for e in erros.iter().take(12) {
        println!("    {}", e.chars().take(400).collect::<String>());
    }
}

#[test]
fn sites() {
    let sites = [
        ("hn", "https://news.ycombinator.com/"),
        ("wikipedia", "https://pt.wikipedia.org/wiki/Brasil"),
        ("google", "https://www.google.com/"),
        ("govbr", "https://www.gov.br/pt-br"),
        ("mdn", "https://developer.mozilla.org/pt-BR/"),
        ("github", "https://github.com/"),
        ("uol", "https://www.uol.com.br/"),
    ];
    let so = std::env::var("SITE").unwrap_or_default();
    for (nome, url) in sites {
        if !so.is_empty() && so != nome {
            continue;
        }
        rodar(nome, url);
    }
}

struct Vigia;
unsafe impl std::alloc::GlobalAlloc for Vigia {
    unsafe fn alloc(&self, l: std::alloc::Layout) -> *mut u8 {
        if l.size() > 1 << 30 {
            eprintln!("ALOCACAO GIGANTE {}\n{}", l.size(), std::backtrace::Backtrace::force_capture());
            std::process::abort();
        }
        unsafe { std::alloc::System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: std::alloc::Layout) {
        unsafe { std::alloc::System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: std::alloc::Layout, n: usize) -> *mut u8 {
        if n > 1 << 30 {
            eprintln!("REALOC GIGANTE {n}\n{}", std::backtrace::Backtrace::force_capture());
            std::process::abort();
        }
        unsafe { std::alloc::System.realloc(p, l, n) }
    }
}
#[global_allocator]
static VIGIA: Vigia = Vigia;

fn relogio() -> f64 {
    use std::sync::OnceLock;
    static INICIO: OnceLock<Instant> = OnceLock::new();
    INICIO.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}
