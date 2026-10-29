//! Programa de Linux comum (Rust + musl, estatico) que o MXOS roda sem
//! mudar nada: mostra os argumentos, o ambiente, a hora, le um arquivo e
//! cria outro. Compilar: cd ferramentas/ola && cargo build --release

use std::io::{self, BufRead, Write};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    println!("Olá do Linux! Sou um programa de Rust + musl rodando no MXOS.");
    println!("argumentos ({}): {:?}", args.len(), args);
    for (chave, valor) in std::env::vars() {
        println!("  {chave}={valor}");
    }
    let agora = std::time::Instant::now();
    let soma: u64 = (1..=1_000_000u64).sum();
    println!("soma de 1 a 1.000.000 = {soma} ({:?})", agora.elapsed());
    match std::fs::read_to_string("/LEIAME.TXT") {
        Ok(texto) => println!("/LEIAME.TXT tem {} bytes; primeira linha: {:?}", texto.len(), texto.lines().next()),
        Err(e) => println!("não consegui ler /LEIAME.TXT: {e}"),
    }
    match std::fs::write("/documentos/do_linux.txt", "escrito por um programa de Linux\n") {
        Ok(()) => println!("gravei /documentos/do_linux.txt"),
        Err(e) => println!("não consegui gravar: {e}"),
    }
    let mut v: Vec<String> = (0..5).map(|i| format!("item {i}")).collect();
    v.reverse();
    println!("vetor: {}", v.join(", "));
    if args.iter().any(|a| a == "ler") {
        print!("digite seu nome: ");
        let _ = io::stdout().flush();
        let mut linha = String::new();
        let _ = io::stdin().lock().read_line(&mut linha);
        println!("prazer, {}!", linha.trim());
    }
    std::process::exit(if args.iter().any(|a| a == "erro") { 3 } else { 0 });
}
