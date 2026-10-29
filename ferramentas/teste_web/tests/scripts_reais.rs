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
    let mut n = 0;
    let mut bytes_js = 0;
    s.rodar_todos(&mut |u| {
        n += 1;
        if std::env::var("JSLOG").is_ok() { eprintln!("JS {u}"); }
        let (t, b) = teste_web::rede::pagina(u).ok()?;
        bytes_js += b.len();
        Some(codificacao::texto(&b, Some(&t)))
    });
    s.carregou();
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
    println!("{nome:10} {n:3} externos, {:6} KB de JS, {ms:8.1} ms, {} erros", bytes_js / 1024, erros.len());
    for e in erros.iter().take(6) {
        println!("    {}", e.chars().take(220).collect::<String>());
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
