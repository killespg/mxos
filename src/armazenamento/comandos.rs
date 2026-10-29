//! Comandos do shell para o disco: ls, cat, escrever, mkdir, rm, mv, df,
//! discos, montar e desmontar (ext4 em /mnt/<disco>). Os caminhos comecam sempre na raiz ("paginas" = "/paginas").

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::{self, Write};

use super::{Erro, InfoDisco};

/// Maior pedaco de arquivo que o `cat` mostra.
const MAX_CAT: usize = 64 * 1024;

/// Tamanho em bytes, KB ou MB (como o shell mostra a memoria).
struct Bytes(u64);

impl fmt::Display for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let texto = match self.0 {
            b if b >= 10 << 20 => format!("{} MB", b >> 20),
            b if b >= 1 << 10 => format!("{} KB", b >> 10),
            b => format!("{b} bytes"),
        };
        f.pad(&texto)
    }
}

/// Executa o comando se ele for de disco. Devolve false se nao for.
pub fn comando(nome: &str, args: &str, saida: &mut dyn Write) -> bool {
    let args = args.trim();
    let resultado = match nome {
        "ls" => ls(sem_aspas(args), saida),
        "cat" => cat(sem_aspas(args), saida),
        "escrever" => escrever(args, saida),
        "mkdir" => mkdir(sem_aspas(args), saida),
        "rm" => rm(sem_aspas(args), saida),
        "mv" => mv(args, saida),
        "df" => df(saida),
        "discos" => discos(saida),
        "montar" => super::montar_disco(args).map(|_| {
            let _ = writeln!(saida, "{args} montado em /mnt/{args} (só leitura)");
        }),
        "desmontar" => super::desmontar_disco(args),
        _ => return false,
    };
    if let Err(e) = resultado {
        let _ = writeln!(saida, "{nome}: {}", e.descricao());
    }
    true
}

/// "/Area de trabalho" (com aspas) vira /Area de trabalho; sem aspas fica igual.
fn sem_aspas(args: &str) -> &str {
    args.strip_prefix('"').and_then(|a| a.strip_suffix('"')).unwrap_or(args)
}

/// Caminho para mostrar: sempre com a barra do comeco.
fn absoluto(caminho: &str) -> String {
    if caminho.starts_with('/') { String::from(caminho) } else { format!("/{caminho}") }
}

/// Primeira palavra e o resto. Um caminho com espacos vai entre aspas:
/// mv "/Area de trabalho/a.txt" /documentos
fn separar(args: &str) -> (&str, &str) {
    if let Some((primeiro, resto)) = args.strip_prefix('"').and_then(|r| r.split_once('"')) {
        return (primeiro, resto.trim_start());
    }
    match args.split_once(' ') {
        Some((primeiro, resto)) => (primeiro, resto.trim_start()),
        None => (args, ""),
    }
}

fn uso(saida: &mut dyn Write, texto: &str) -> Result<(), Erro> {
    let _ = writeln!(saida, "uso: {texto}");
    Ok(())
}

/// ls [caminho]: pastas primeiro, depois arquivos, por nome.
fn ls(args: &str, saida: &mut dyn Write) -> Result<(), Erro> {
    let caminho = if args.is_empty() { "/" } else { args };
    let mut itens = super::listar(caminho)?;
    itens.sort_by(|a, b| b.pasta.cmp(&a.pasta).then_with(|| chave_ordem(&a.nome).cmp(&chave_ordem(&b.nome))));
    let _ = writeln!(saida, "{}", absoluto(caminho));
    for item in &itens {
        if item.pasta {
            let _ = writeln!(saida, "  {:>10}  {}", "<pasta>", item.nome);
        } else {
            let _ = writeln!(saida, "  {:>10}  {}", item.tamanho, item.nome);
        }
    }
    let pastas = itens.iter().filter(|i| i.pasta).count();
    let _ = writeln!(saida, "{} arquivo(s), {} pasta(s)", itens.len() - pastas, pastas);
    Ok(())
}

/// Ordem de dicionario: "Área" fica junto de "area", nao depois do "z".
fn chave_ordem(nome: &str) -> String {
    nome.chars().flat_map(char::to_lowercase).map(super::fat32::sem_acento).collect()
}

/// cat <arquivo>: texto em UTF-8; controles e bytes que nao sao UTF-8
/// viram '.'.
fn cat(args: &str, saida: &mut dyn Write) -> Result<(), Erro> {
    if args.is_empty() {
        return uso(saida, "cat <arquivo>");
    }
    let dados = super::ler(args)?;
    let mostrar = &dados[..dados.len().min(MAX_CAT)];
    let texto: String = String::from_utf8_lossy(mostrar)
        .chars()
        .filter(|&c| c != '\r')
        .map(|c| match c {
            '\n' => '\n',
            '\t' => ' ',
            '\u{FFFD}' => '.',
            c if c.is_control() => '.',
            c => c,
        })
        .collect();
    let _ = saida.write_str(&texto);
    if !texto.is_empty() && !texto.ends_with('\n') {
        let _ = writeln!(saida);
    }
    if dados.len() > MAX_CAT {
        let _ = writeln!(saida, "… ({} bytes, mostrando os primeiros {MAX_CAT})", dados.len());
    }
    Ok(())
}

/// escrever <arquivo> <texto>: "\n" no texto vira quebra de linha; o
/// arquivo termina com uma quebra de linha.
fn escrever(args: &str, saida: &mut dyn Write) -> Result<(), Erro> {
    let (caminho, texto) = separar(args);
    if caminho.is_empty() {
        return uso(saida, "escrever <arquivo> <texto>   (\\n quebra a linha)");
    }
    let mut conteudo: Vec<u8> = texto.replace("\\n", "\n").into_bytes();
    conteudo.push(b'\n');
    super::escrever(caminho, &conteudo)?;
    let _ = writeln!(saida, "gravado: {} ({} bytes)", absoluto(caminho), conteudo.len());
    Ok(())
}

fn mkdir(args: &str, saida: &mut dyn Write) -> Result<(), Erro> {
    if args.is_empty() {
        return uso(saida, "mkdir <pasta>");
    }
    super::criar_pasta(args)?;
    let _ = writeln!(saida, "pasta criada: {}", absoluto(args));
    Ok(())
}

fn rm(args: &str, saida: &mut dyn Write) -> Result<(), Erro> {
    if args.is_empty() {
        return uso(saida, "rm <arquivo ou pasta vazia>");
    }
    super::apagar(args)?;
    let _ = writeln!(saida, "apagado: {}", absoluto(args));
    Ok(())
}

/// mv <de> <para>: se `para` e uma pasta que existe, move para dentro dela.
fn mv(args: &str, saida: &mut dyn Write) -> Result<(), Erro> {
    let (de, para) = separar(args);
    let para = sem_aspas(para);
    if de.is_empty() || para.is_empty() {
        return uso(saida, "mv <de> <para>   (caminho com espaço vai entre aspas)");
    }
    let nome = de.trim_end_matches('/').rsplit('/').next().unwrap_or(de);
    let para = if super::listar(para).is_ok() {
        format!("{}/{}", para.trim_end_matches('/'), nome)
    } else {
        String::from(para)
    };
    super::renomear(de, &para)?;
    let _ = writeln!(saida, "movido: {} → {}", absoluto(de), absoluto(&para));
    Ok(())
}

fn df(saida: &mut dyn Write) -> Result<(), Erro> {
    let info = super::discos().into_iter().find(|d| d.montado).ok_or(Erro::SemDisco)?;
    let usado = info.tamanho - info.livre;
    let porcento = if info.tamanho == 0 { 0 } else { usado * 100 / info.tamanho };
    let _ = writeln!(saida, "Volume \"{}\" em {} ({}), montado em /", info.rotulo, info.nome, info.sistema);
    let _ = writeln!(
        saida,
        "Tamanho: {}  |  usado: {} ({porcento}%)  |  livre: {}",
        Bytes(info.tamanho),
        Bytes(usado),
        Bytes(info.livre)
    );
    Ok(())
}

fn discos(saida: &mut dyn Write) -> Result<(), Erro> {
    let lista = super::discos();
    if lista.is_empty() {
        let _ = writeln!(saida, "nenhum disco (no QEMU: -device virtio-blk-device)");
        return Ok(());
    }
    for d in &lista {
        linha_disco(d, saida);
    }
    Ok(())
}

fn linha_disco(d: &InfoDisco, saida: &mut dyn Write) {
    let _ = write!(saida, "{:<5} {:>8}  {:<12}", d.nome, Bytes(d.setores * 512), d.sistema);
    if d.sistema == "FAT32" {
        let _ = write!(saida, " \"{}\"  livre {}", d.rotulo, Bytes(d.livre));
    }
    let _ = writeln!(saida, "{}", if d.montado { "  (montado em /)" } else { "" });
}
