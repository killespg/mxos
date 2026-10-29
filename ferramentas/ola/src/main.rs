//! Programa de Linux comum (Rust + musl, estatico) que o MXOS roda sem
//! mudar nada: mostra os argumentos, o ambiente, a hora, le um arquivo e
//! cria outro. "ola sinais" e "ola threads" testam tratadores de sinal,
//! alarm, fork e std::thread. Compilar: cd ferramentas/ola && cargo build --release

use std::io::{self, BufRead, Write};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "sinais") {
        testar_sinais();
        return;
    }
    if args.get(1).is_some_and(|a| a == "threads") {
        testar_threads();
        return;
    }
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

// ---------------------------------------------------------------------
// Testes de sinais e threads (ola sinais / ola threads)

use std::sync::atomic::{AtomicU32, Ordering};

unsafe extern "C" {
    fn signal(sinal: i32, tratador: extern "C" fn(i32)) -> usize;
    fn raise(sinal: i32) -> i32;
    fn alarm(segundos: u32) -> u32;
    fn pause() -> i32;
    fn getpid() -> i32;
    fn kill(pid: i32, sinal: i32) -> i32;
    fn usleep(us: u32) -> i32;
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, opcoes: i32) -> i32;
}

static RECEBIDOS: AtomicU32 = AtomicU32::new(0);
static ULTIMO: AtomicU32 = AtomicU32::new(0);

extern "C" fn tratador(sinal: i32) {
    RECEBIDOS.fetch_add(1, Ordering::SeqCst);
    ULTIMO.store(sinal as u32, Ordering::SeqCst);
}

fn testar_sinais() {
    unsafe {
        signal(10, tratador); // SIGUSR1
        signal(14, tratador); // SIGALRM
        signal(17, tratador); // SIGCHLD
        raise(10);
        println!("raise(SIGUSR1): tratador rodou {} vez(es), sinal {}", RECEBIDOS.load(Ordering::SeqCst), ULTIMO.load(Ordering::SeqCst));
        kill(getpid(), 10);
        println!("kill(getpid(), SIGUSR1): {} vez(es)", RECEBIDOS.load(Ordering::SeqCst));
        let antes = std::time::Instant::now();
        alarm(1);
        let r = pause();
        let erro = std::io::Error::last_os_error();
        println!("alarm(1) + pause(): voltou {r} ({erro}) depois de {:?}, sinal {}", antes.elapsed(), ULTIMO.load(Ordering::SeqCst));
        alarm(1);
        let r = usleep(3_000_000);
        println!("usleep(3 s) interrompido pelo alarme: {r} ({}), sinal {}", std::io::Error::last_os_error(), ULTIMO.load(Ordering::SeqCst));
        let filho = fork();
        if filho == 0 {
            std::process::exit(7);
        }
        let mut status = 0;
        usleep(200_000);
        let r = waitpid(filho, &mut status, 0);
        println!("filho {filho} saiu: waitpid {r}, código {}, SIGCHLD recebido: {}", status >> 8 & 0xFF, ULTIMO.load(Ordering::SeqCst) == 17);
        let mut x = 1.5f64;
        raise(10);
        x *= 2.0;
        println!("registradores de ponto flutuante preservados: {x}");
    }
}

fn testar_threads() {
    use std::sync::{Arc, Mutex, mpsc};
    let contador = Arc::new(Mutex::new(0u64));
    let (envio, recebe) = mpsc::channel();
    let mut alcas = Vec::new();
    for n in 0..4u64 {
        let contador = contador.clone();
        let envio = envio.clone();
        alcas.push(std::thread::spawn(move || {
            let mut soma = 0u64;
            for k in 0..20_000u64 {
                *contador.lock().unwrap() += 1;
                soma += k * (n + 1);
            }
            envio.send((n, soma)).unwrap();
            n * 10
        }));
    }
    drop(envio);
    let mut somas: Vec<(u64, u64)> = recebe.iter().collect();
    somas.sort();
    let voltas: Vec<u64> = alcas.into_iter().map(|a| a.join().unwrap()).collect();
    println!("4 threads: contador = {} (esperado 80000), somas = {:?}, join = {:?}", *contador.lock().unwrap(), somas, voltas);
    let dorminhoca = std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(300));
        "acordei"
    });
    println!("thread que dorme: {}", dorminhoca.join().unwrap());
    let par = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let par2 = par.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        *par2.0.lock().unwrap() = true;
        par2.1.notify_one();
    });
    let mut pronto = par.0.lock().unwrap();
    while !*pronto {
        pronto = par.1.wait(pronto).unwrap();
    }
    println!("Condvar: avisada = {}", *pronto);
}
