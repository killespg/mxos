//! Testes do FAT32 do kernel contra imagens do criar_disco.py. Depois das
//! escritas a imagem e conferida pelo `criar_disco.py --verificar`, que
//! foi escrito separado (em Python) e checa FATs, cadeias, LFN e FSInfo.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;

use teste_fat32::fat32::Fat32;
use teste_fat32::{DiscoMemoria, Erro};

fn relogio() -> (u16, u16) {
    // 2026-10-28 12:34:56
    ((46 << 9) | (10 << 5) | 28, (12 << 11) | (34 << 5) | 28)
}

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("criar_disco.py")
}

fn temporario(nome: &str) -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join(nome)
}

/// O Python vem da variavel PYTHON; senao tenta python, py e python3 do PATH.
fn python(args: &[&str]) -> (bool, Vec<u8>) {
    let candidatos: Vec<String> = match std::env::var("PYTHON") {
        Ok(p) => vec![p],
        Err(_) => ["python", "py", "python3"].iter().map(|s| s.to_string()).collect(),
    };
    for programa in &candidatos {
        if let Ok(saida) = Command::new(programa).arg("-I").arg(script()).args(args).output() {
            return (saida.status.success(), saida.stdout);
        }
    }
    panic!("nao achei o Python (PATH={:?}): defina PYTHON com o caminho do python.exe", std::env::var("PATH"));
}

/// Imagem nova do criar_disco.py, ja na memoria.
fn imagem_nova(nome: &str, extras: &[&str]) -> Rc<RefCell<Vec<u8>>> {
    let caminho = temporario(nome);
    let mut args = vec![caminho.to_str().unwrap()];
    args.extend_from_slice(extras);
    let (ok, _) = python(&args);
    assert!(ok, "criar_disco.py falhou");
    Rc::new(RefCell::new(std::fs::read(&caminho).unwrap()))
}

fn montar(imagem: &Rc<RefCell<Vec<u8>>>) -> Fat32 {
    let (disco, _) = DiscoMemoria::novo(imagem.clone());
    Fat32::montar(disco, relogio).expect("montar")
}

/// Salva e roda o verificador; devolve a listagem (linhas DIR/ARQ).
fn verificar(imagem: &Rc<RefCell<Vec<u8>>>, nome: &str) -> Vec<String> {
    let caminho = temporario(nome);
    std::fs::write(&caminho, &*imagem.borrow()).unwrap();
    let (ok, saida) = python(&["--verificar", caminho.to_str().unwrap()]);
    let texto = String::from_utf8_lossy(&saida).into_owned();
    assert!(ok && texto.trim_end().ends_with("OK"), "verificador reclamou:\n{texto}");
    texto.lines().filter(|l| l.starts_with("DIR ") || l.starts_with("ARQ ")).map(String::from).collect()
}

fn ler_python(imagem: &Rc<RefCell<Vec<u8>>>, nome: &str, arquivo: &str) -> Vec<u8> {
    let caminho = temporario(nome);
    std::fs::write(&caminho, &*imagem.borrow()).unwrap();
    let (ok, saida) = python(&["--verificar", caminho.to_str().unwrap(), "--ler", arquivo]);
    assert!(ok, "python nao leu {arquivo}");
    saida
}

fn nomes(fs: &mut Fat32, caminho: &str) -> Vec<String> {
    fs.listar(caminho).unwrap().into_iter().map(|i| i.nome).collect()
}

/// Gerador pseudo-aleatorio simples (xorshift), sempre a mesma sequencia.
struct Sorteio(u64);

impl Sorteio {
    fn proximo(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn ate(&mut self, n: u64) -> u64 {
        self.proximo() % n
    }
}

fn conteudo(sorteio: &mut Sorteio, tamanho: usize) -> Vec<u8> {
    let semente = sorteio.proximo() as u8;
    (0..tamanho).map(|i| (i as u8).wrapping_mul(31).wrapping_add(semente)).collect()
}

#[test]
fn le_a_imagem_do_python() {
    let imagem = imagem_nova("leitura.img", &[]);
    let mut fs = montar(&imagem);
    assert_eq!(fs.rotulo(), "MXOS");
    assert_eq!(nomes(&mut fs, "/"), ["bin", "lib", "etc", "LEIAME.TXT", "config", "paginas", "documentos"]);
    assert_eq!(nomes(&mut fs, "/paginas"), ["index.html", "sobre.html", "guia.html", "tabelas.html", "teste.html", "css.html", "estilo.css"]);
    assert_eq!(nomes(&mut fs, "/documentos"), ["notas.txt", "Lista de compras.txt", "Anotações.txt"]);
    assert!(nomes(&mut fs, "/config").is_empty());
    let index = fs.ler("/paginas/index.html").unwrap();
    assert_eq!(index, ler_python(&imagem, "leitura.img", "/paginas/index.html"));
    assert!(String::from_utf8_lossy(&index).contains("<h1>Bem-vindo ao MXOS</h1>"));
    // busca sem caixa, pelo nome curto e com . e ..
    assert_eq!(fs.ler("/PAGINAS/INDEX.HTML").unwrap(), index);
    assert_eq!(fs.ler("/paginas/INDEX~1.HTM").unwrap(), index);
    assert_eq!(fs.ler("documentos/../paginas/./index.html").unwrap(), index);
    assert_eq!(fs.ler("/documentos/lista de compras.txt").unwrap(), "arroz\nfeijão\ncafé\n".as_bytes());
    // nome longo com acentos (UTF-16 no disco), achado sem diferenciar caixa
    assert_eq!(fs.ler("/documentos/ANOTAÇÕES.TXT").unwrap().len(), 635);
    assert_eq!(fs.ler("/config"), Err(Erro::EPasta));
    assert_eq!(fs.listar("/LEIAME.TXT").err(), Some(Erro::NaoEPasta));
    assert_eq!(fs.ler("/nada.txt"), Err(Erro::NaoAchado));
    assert_eq!(fs.ler("/nada/x.txt"), Err(Erro::NaoAchado));
    // o espaco livre tem de bater com o que o verificador em Python conta
    // ("... clusters de 512 bytes, N livres (...)")
    let (_, saida) = python(&["--verificar", temporario("leitura.img").to_str().unwrap()]);
    let texto = String::from_utf8_lossy(&saida);
    let livres: u64 = texto
        .split(" livres")
        .next()
        .and_then(|antes| antes.rsplit(' ').next())
        .and_then(|n| n.parse().ok())
        .expect("o verificador não disse quantos clusters estão livres");
    assert_eq!(fs.bytes_livres(), livres * 512);
}

#[test]
fn superfloppy() {
    let imagem = imagem_nova("superfloppy.img", &["--sem-mbr"]);
    let mut fs = montar(&imagem);
    assert_eq!(nomes(&mut fs, "/"), ["bin", "lib", "etc", "LEIAME.TXT", "config", "paginas", "documentos"]);
    fs.escrever("/config/tema.cfg", b"tema=Azul\n").unwrap();
    drop(fs);
    verificar(&imagem, "superfloppy.img");
}

#[test]
fn escritas_basicas() {
    let imagem = imagem_nova("escritas.img", &[]);
    let mut fs = montar(&imagem);
    let livre_antes = fs.bytes_livres();

    fs.escrever("/NOVO.TXT", b"curto\n").unwrap();
    fs.escrever("/minusculo.txt", b"so minusculas\n").unwrap();
    fs.escrever("/Misto.Txt", b"caixa misturada\n").unwrap();
    fs.escrever("/vazio", b"").unwrap();
    fs.escrever("/um arquivo com nome bem comprido mesmo.dados", &vec![7u8; 513]).unwrap();
    fs.escrever("/grande.bin", &(0..100_000u32).map(|i| i as u8).collect::<Vec<_>>()).unwrap();
    assert_eq!(fs.escrever("/ruim?.txt", b"x"), Err(Erro::NomeInvalido));
    assert_eq!(fs.escrever("/paginas", b"x"), Err(Erro::EPasta));
    assert_eq!(fs.escrever("/nao/existe.txt", b"x"), Err(Erro::NaoAchado));
    assert_eq!(fs.escrever("/LEIAME.TXT/x", b"x"), Err(Erro::NaoEPasta));

    // substituir maior e menor
    fs.escrever("/novo.txt", &vec![1u8; 5000]).unwrap();
    fs.escrever("/grande.bin", b"agora pequeno").unwrap();
    fs.escrever("/vazio", b"nao mais").unwrap();
    fs.escrever("/minusculo.txt", b"").unwrap();

    fs.criar_pasta("/config/temas").unwrap();
    fs.criar_pasta("/config/temas/Pasta Funda Com Nome Longo").unwrap();
    assert_eq!(fs.criar_pasta("/config/temas"), Err(Erro::JaExiste));
    fs.escrever("/config/temas/Pasta Funda Com Nome Longo/a.txt", b"fundo").unwrap();

    // renomear na mesma pasta, mudar so a caixa, mover arquivo e pasta
    fs.renomear("/Misto.Txt", "/Renomeado com nome longo.txt").unwrap();
    fs.renomear("/novo.txt", "/Novo.TXT").unwrap();
    fs.renomear("/documentos/notas.txt", "/config/notas.txt").unwrap();
    fs.renomear("/config/temas", "/documentos/temas movidos").unwrap();
    assert_eq!(fs.renomear("/documentos", "/documentos/temas movidos/dentro"), Err(Erro::NomeInvalido));
    assert_eq!(fs.renomear("/LEIAME.TXT", "/paginas/index.html"), Err(Erro::JaExiste));
    assert_eq!(fs.renomear("/fantasma", "/x"), Err(Erro::NaoAchado));

    // apagar
    assert_eq!(fs.apagar("/documentos"), Err(Erro::PastaNaoVazia));
    fs.apagar("/paginas/sobre.html").unwrap();
    fs.apagar("/um arquivo com nome bem comprido mesmo.dados").unwrap();
    assert_eq!(fs.apagar("/"), Err(Erro::NomeInvalido));

    assert_eq!(fs.ler("/Novo.TXT").unwrap(), vec![1u8; 5000]);
    assert_eq!(fs.ler("/grande.bin").unwrap(), b"agora pequeno");
    assert_eq!(fs.ler("/documentos/temas movidos/pasta funda com nome longo/a.txt").unwrap(), b"fundo");
    assert_eq!(fs.ler("/config/notas.txt").unwrap().len(), 151);
    let livre_depois = fs.bytes_livres();
    drop(fs);

    let lista = verificar(&imagem, "escritas.img");
    for esperado in [
        "ARQ /Novo.TXT 5000",
        "ARQ /minusculo.txt 0",
        "ARQ /vazio 8",
        "ARQ /grande.bin 13",
        "ARQ /Renomeado com nome longo.txt 16",
        "ARQ /config/notas.txt 151",
        "DIR /documentos/temas movidos",
        "DIR /documentos/temas movidos/Pasta Funda Com Nome Longo",
        "ARQ /documentos/temas movidos/Pasta Funda Com Nome Longo/a.txt 5",
    ] {
        assert!(lista.iter().any(|l| l == esperado), "faltou {esperado} em {lista:#?}");
    }
    assert!(!lista.iter().any(|l| l.contains("sobre.html") || l.contains("Misto")));
    assert_eq!(ler_python(&imagem, "escritas.img", "/Renomeado com nome longo.txt"), b"caixa misturada\n");

    // montar de novo: o FSInfo tem de bater com a FAT
    let fs = montar(&imagem);
    assert_eq!(fs.bytes_livres(), livre_depois);
    assert!(livre_depois < livre_antes);
}

#[test]
fn pasta_com_muitos_arquivos() {
    let imagem = imagem_nova("muitos.img", &[]);
    let mut fs = montar(&imagem);
    fs.criar_pasta("/muitos").unwrap();
    for i in 0..300 {
        fs.escrever(&format!("/muitos/arquivo numero {i:03} com nome longo.txt"), format!("{i}\n").as_bytes()).unwrap();
    }
    assert_eq!(fs.listar("/muitos").unwrap().len(), 300);
    for i in (0..300).step_by(2) {
        fs.apagar(&format!("/muitos/arquivo numero {i:03} com nome longo.txt")).unwrap();
    }
    // nomes curtos repetem o comeco: ARQUIV~1 ... ARQUI~10 ... ARQU~100
    for i in 0..40 {
        fs.escrever(&format!("/muitos/novo {i}.txt"), b"x").unwrap();
    }
    assert_eq!(fs.listar("/muitos").unwrap().len(), 190);
    assert_eq!(fs.ler("/muitos/arquivo numero 299 com nome longo.txt").unwrap(), b"299\n");
    drop(fs);
    let lista = verificar(&imagem, "muitos.img");
    assert_eq!(lista.iter().filter(|l| l.starts_with("ARQ /muitos/")).count(), 190);
}

#[test]
fn encher_o_disco() {
    let imagem = imagem_nova("cheio.img", &[]);
    let mut fs = montar(&imagem);
    let livre = fs.bytes_livres();
    fs.criar_pasta("/lotado").unwrap();
    let pedaco = vec![0xABu8; 4 * 1024 * 1024];
    let mut n = 0;
    loop {
        match fs.escrever(&format!("/lotado/bloco{n}.bin"), &pedaco) {
            Ok(()) => n += 1,
            Err(Erro::Cheio) => break,
            Err(e) => panic!("erro inesperado {e:?}"),
        }
    }
    assert!(n >= 14, "so {n} blocos de 4 MB");
    // o resto do espaco num arquivo menor, ate dar Cheio de novo
    let mut tamanho = fs.bytes_livres() as usize;
    while fs.escrever("/lotado/resto.bin", &vec![1u8; tamanho]).is_err() {
        tamanho -= 512;
    }
    assert!(fs.bytes_livres() < 4096);
    assert_eq!(fs.escrever("/lotado/mais um.bin", &pedaco), Err(Erro::Cheio));
    drop(fs);
    verificar(&imagem, "cheio.img");

    let mut fs = montar(&imagem);
    for i in 0..n {
        fs.apagar(&format!("/lotado/bloco{i}.bin")).unwrap();
    }
    fs.apagar("/lotado/resto.bin").unwrap();
    fs.apagar("/lotado").unwrap();
    assert_eq!(fs.bytes_livres(), livre);
    drop(fs);
    verificar(&imagem, "cheio.img");
}

#[test]
fn operacoes_aleatorias() {
    let imagem = imagem_nova("aleatorio.img", &[]);
    let mut fs = montar(&imagem);
    let mut sorteio = Sorteio(0x4D58_4F53_2026);
    let pastas = ["/", "/config", "/documentos", "/a", "/a/b"];
    fs.criar_pasta("/a").unwrap();
    fs.criar_pasta("/a/b").unwrap();
    let mut modelo: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let tamanhos = [0usize, 1, 100, 511, 512, 513, 1024, 3000, 20_000, 70_000];
    for passo in 0..3000 {
        let pasta = pastas[sorteio.ate(pastas.len() as u64) as usize];
        let nome = match sorteio.ate(4) {
            0 => format!("ARQ{}.TXT", sorteio.ate(30)),
            1 => format!("arquivo {} de teste.bin", sorteio.ate(30)),
            2 => format!("x{}", sorteio.ate(30)),
            _ => format!("Nome Bem Comprido Para Testar Varias Entradas LFN {}.dat", sorteio.ate(30)),
        };
        let caminho = format!("{}/{}", pasta.trim_end_matches('/'), nome);
        let chave = caminho.to_ascii_lowercase();
        match sorteio.ate(10) {
            0..=4 => {
                let tamanho = tamanhos[sorteio.ate(tamanhos.len() as u64) as usize];
                let dados = conteudo(&mut sorteio, tamanho);
                fs.escrever(&caminho, &dados).unwrap();
                modelo.insert(chave, dados);
            }
            5..=6 => {
                let r = fs.apagar(&caminho);
                assert_eq!(r.is_ok(), modelo.remove(&chave).is_some(), "apagar {caminho}");
            }
            7 => {
                let destino_pasta = pastas[sorteio.ate(pastas.len() as u64) as usize];
                let destino = format!("{}/movido {}", destino_pasta.trim_end_matches('/'), sorteio.ate(20));
                let r = fs.renomear(&caminho, &destino);
                let chave_destino = destino.to_ascii_lowercase();
                if modelo.contains_key(&chave) && !modelo.contains_key(&chave_destino) {
                    r.unwrap();
                    let dados = modelo.remove(&chave).unwrap();
                    modelo.insert(chave_destino, dados);
                } else if chave == chave_destino {
                    r.unwrap();
                } else {
                    assert!(r.is_err(), "renomear {caminho} -> {destino} devia falhar");
                }
            }
            _ => match modelo.get(&chave) {
                Some(dados) => assert_eq!(&fs.ler(&caminho).unwrap(), dados, "ler {caminho}"),
                None => assert_eq!(fs.ler(&caminho), Err(Erro::NaoAchado)),
            },
        }
        if passo % 1000 == 999 {
            drop(fs);
            verificar(&imagem, "aleatorio.img");
            fs = montar(&imagem);
            for (caminho, dados) in &modelo {
                assert_eq!(&fs.ler(caminho).unwrap(), dados, "depois de montar de novo: {caminho}");
            }
        }
    }
    let lista = verificar(&imagem, "aleatorio.img");
    let originais = 24; // + /lib (ld-linux, libc, libresolv) e /bin/busybox-dinamico; // com /bin (ola, busybox) e /etc (com resolv.conf, hosts, nsswitch.conf) // arquivos que ja vinham na imagem
    assert_eq!(lista.iter().filter(|l| l.starts_with("ARQ ")).count(), modelo.len() + originais);
}

#[test]
fn poucos_pedidos_ao_disco() {
    let imagem = imagem_nova("pedidos.img", &[]);
    let (disco, pedidos) = DiscoMemoria::novo(imagem.clone());
    let mut fs = Fat32::montar(disco, relogio).unwrap();
    let montagem = *pedidos.borrow();
    fs.escrever("/medio.bin", &vec![9u8; 200_000]).unwrap();
    let escrita = *pedidos.borrow();
    let dados = fs.ler("/medio.bin").unwrap();
    assert_eq!(dados.len(), 200_000);
    let leitura = *pedidos.borrow();
    println!("montagem {montagem:?}\nescrita {escrita:?}\nleitura {leitura:?}");
    assert!(montagem.leituras < 20, "montagem com {} leituras", montagem.leituras);
    assert!(escrita.escritas - montagem.escritas < 20, "escrita com muitos pedidos");
    assert!(leitura.leituras - escrita.leituras < 20, "leitura com muitos pedidos");
}

/// Disco que pode passar a recusar escritas (como um somente leitura).
struct DiscoQueFalha {
    dentro: Box<dyn teste_fat32::Disco>,
    recusar: Rc<std::cell::Cell<bool>>,
}

impl teste_fat32::Disco for DiscoQueFalha {
    fn nome(&self) -> &str {
        "falha"
    }

    fn setores(&self) -> u64 {
        self.dentro.setores()
    }

    fn ler(&mut self, setor: u64, buffer: &mut [u8]) -> Result<(), Erro> {
        self.dentro.ler(setor, buffer)
    }

    fn escrever(&mut self, setor: u64, dados: &[u8]) -> Result<(), Erro> {
        if self.recusar.get() {
            return Err(Erro::Disco);
        }
        self.dentro.escrever(setor, dados)
    }
}

#[test]
fn disco_que_recusa_escrita() {
    let imagem = imagem_nova("recusa.img", &[]);
    let (dentro, _) = DiscoMemoria::novo(imagem.clone());
    let recusar = Rc::new(std::cell::Cell::new(true));
    let mut fs = Fat32::montar(Box::new(DiscoQueFalha { dentro, recusar: recusar.clone() }), relogio).unwrap();
    let livre = fs.bytes_livres();
    let original = imagem.borrow().clone();
    assert_eq!(fs.escrever("/novo.txt", &vec![1u8; 3000]), Err(Erro::Disco));
    assert_eq!(fs.escrever("/LEIAME.TXT", &vec![1u8; 30_000]), Err(Erro::Disco));
    assert_eq!(fs.criar_pasta("/pasta nova"), Err(Erro::Disco));
    assert_eq!(fs.apagar("/paginas/index.html"), Err(Erro::Disco));
    assert_eq!(fs.bytes_livres(), livre);
    assert!(*imagem.borrow() == original, "a imagem mudou");
    assert_eq!(fs.ler("/LEIAME.TXT").unwrap().len(), 433);
    // o disco volta a aceitar: tudo funciona e fica consistente
    recusar.set(false);
    fs.escrever("/novo.txt", &vec![1u8; 3000]).unwrap();
    fs.criar_pasta("/pasta nova").unwrap();
    drop(fs);
    verificar(&imagem, "recusa.img");
}
