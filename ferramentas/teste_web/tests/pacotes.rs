//! O .mxa de verdade (do repositorio mxos-apps, ao lado do mxos) abre no
//! leitor do kernel: gzip, tar e manifesto.

use teste_web::pacotes;

#[test]
fn abre_o_pacote_da_calculadora() {
    let caminho = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../mxos-apps/pacotes/calculadora.mxa");
    let Ok(dados) = std::fs::read(caminho) else {
        eprintln!("sem {caminho}: rode ferramentas/empacotar.py no mxos-apps");
        return;
    };
    let arquivos = pacotes::abrir_mxa(&dados).expect("abrir");
    let nomes: Vec<&str> = arquivos.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(nomes, ["manifesto.json", "armv7/calculadora", "aarch64/calculadora"]);
    let m = pacotes::manifesto_do_mxa(&dados).expect("manifesto");
    assert_eq!(m.id, "calculadora");
    assert_eq!(m.nome, "Calculadora");
    assert_eq!(m.versao, "1.0.0");
    assert_eq!(m.programa, "aarch64/calculadora");
    let elf = &arquivos.iter().find(|(n, _)| n == "armv7/calculadora").unwrap().1;
    assert_eq!(&elf[..4], b"\x7fELF");
    assert_eq!(elf[4], 1, "ELF de 32 bits");
}

#[test]
fn versoes() {
    assert!(pacotes::versao_maior("1.2.10", "1.2.9"));
    assert!(pacotes::versao_maior("2.0", "1.9.9"));
    assert!(!pacotes::versao_maior("1.0.0", "1.0.0"));
    assert!(!pacotes::versao_maior("1.0", "1.0.1"));
}
