//! AAC-LC: a IMDCT rapida contra a formula da norma e o som inteiro do
//! Big Buck Bunny decodificado (sem erro de fluxo, nivel coerente, sem
//! estalo entre quadros). Grava target/saidas/aac_bbb.wav para ouvir.

use teste_web::midia::aac::{self, Aac};
use teste_web::midia::mp4::Mp4;

#[test]
fn imdct_bate_com_a_formula() {
    for n in [256usize, 2048] {
        let mut s = 12345u32;
        let x: Vec<f32> = (0..n / 2)
            .map(|_| {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                (s >> 8) as f32 / (1 << 24) as f32 - 0.5
            })
            .collect();
        let rapido = aac::imdct_teste(n, &x);
        let n0 = (n as f64 / 2.0 + 1.0) / 2.0;
        let mut erro = 0f64;
        for i in 0..n {
            let mut soma = 0f64;
            for (k, &v) in x.iter().enumerate() {
                soma += v as f64 * (2.0 * std::f64::consts::PI / n as f64 * (i as f64 + n0) * (k as f64 + 0.5)).cos();
            }
            let certo = 2.0 / n as f64 * soma;
            erro = erro.max((certo - rapido[i] as f64).abs());
        }
        println!("IMDCT {n}: erro maximo {erro:e}");
        assert!(erro < 1e-4, "IMDCT {n} errada: {erro}");
    }
}

fn gravar_wav(caminho: &std::path::Path, taxa: u32, pcm: &[i16]) {
    let mut d = Vec::new();
    let bytes = (pcm.len() * 2) as u32;
    d.extend_from_slice(b"RIFF");
    d.extend_from_slice(&(36 + bytes).to_le_bytes());
    d.extend_from_slice(b"WAVEfmt ");
    d.extend_from_slice(&16u32.to_le_bytes());
    d.extend_from_slice(&1u16.to_le_bytes());
    d.extend_from_slice(&2u16.to_le_bytes());
    d.extend_from_slice(&taxa.to_le_bytes());
    d.extend_from_slice(&(taxa * 4).to_le_bytes());
    d.extend_from_slice(&4u16.to_le_bytes());
    d.extend_from_slice(&16u16.to_le_bytes());
    d.extend_from_slice(b"data");
    d.extend_from_slice(&bytes.to_le_bytes());
    for v in pcm {
        d.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(caminho, d).unwrap();
}

#[test]
fn som_do_bbb() {
    let caminho = std::env::var("VIDEO").unwrap_or_else(|_| String::from("C:/dev/mxos-tmp/videos/bbb360.mp4"));
    let Ok(arquivo) = std::fs::read(&caminho) else {
        eprintln!("sem {caminho}: pulando");
        return;
    };
    let mut fonte = arquivo.clone();
    let mp4 = Mp4::abrir(&mut fonte).unwrap();
    let a = mp4.audio().expect("trilha de audio");
    let asc = aac::asc_do_esds(&a.config).expect("esds");
    println!("AudioSpecificConfig {:02x?} -> {:?}", asc, aac::config(asc));
    let mut dec = Aac::novo(asc).unwrap();
    let mut pcm: Vec<i16> = Vec::new();
    let mut erros = 0;
    let t = std::time::Instant::now();
    for (k, am) in a.amostras.iter().enumerate() {
        let q = &arquivo[am.pos as usize..am.pos as usize + am.tamanho as usize];
        if let Err(e) = dec.decodificar(q, &mut pcm) {
            erros += 1;
            if erros < 5 {
                println!("quadro {k}: {e}");
            }
        }
    }
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    let pico = pcm.iter().map(|v| (*v as i32).abs()).max().unwrap_or(0);
    let rms = (pcm.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / pcm.len().max(1) as f64).sqrt();
    // estalo: a maior diferenca entre amostras vizinhas na borda dos quadros
    // comparada com a maior diferenca dentro deles
    let (mut borda, mut dentro) = (0i32, 0i32);
    for i in 1..pcm.len() / 2 {
        let d = (pcm[2 * i] as i32 - pcm[2 * i - 2] as i32).abs();
        if i % 1024 == 0 { borda = borda.max(d) } else { dentro = dentro.max(d) }
    }
    println!("{} quadros, {} erros, {} amostras, pico {pico}, rms {rms:.0}, salto na borda {borda} / dentro {dentro}, {ms:.0} ms ({:.1}x tempo real)", a.amostras.len(), erros, pcm.len() / 2, (pcm.len() / 2) as f64 / dec.taxa as f64 * 1000.0 / ms);
    gravar_wav(&teste_web::png::pasta().join("aac_bbb.wav"), dec.taxa, &pcm);
    assert_eq!(erros, 0);
    assert!(pico > 1000 && pico <= 32767 && rms > 100.0, "nivel estranho");
    assert!(borda <= dentro, "estalo na borda dos quadros");
}

/// O som do YouTube (formato 140, MP4 fragmentado): todos os quadros dos
/// segmentos baixados. AUDIO=caminho troca o arquivo.
#[test]
fn som_do_youtube() {
    use teste_web::midia::mp4;
    let caminho = std::env::var("AUDIO").unwrap_or_else(|_| String::from("C:/dev/mxos-tmp/videos/dash_XMPVhbO2YaE_140.mp4"));
    let Ok(arquivo) = std::fs::read(&caminho) else {
        eprintln!("sem {caminho}: pulando");
        return;
    };
    let mut fonte = arquivo.clone();
    let m = Mp4::abrir(&mut fonte).unwrap();
    let a = m.audio().expect("audio");
    let asc = aac::asc_do_esds(&a.config).expect("esds");
    println!("youtube: ASC {:02x?} -> {:?}, escala {}", asc, aac::config(asc), a.escala);
    let padrao = mp4::padroes(mp4::moov_de(&arquivo).unwrap());
    let mut dec = Aac::novo(asc).unwrap();
    let mut pcm: Vec<i16> = Vec::new();
    let (mut quadros, mut erros) = (0, 0);
    let t = std::time::Instant::now();
    for s in mp4::indice(&arquivo, 0) {
        let (ini, fim) = (s.pos as usize, s.pos as usize + s.tamanho as usize);
        if fim > arquivo.len() {
            break;
        }
        let seg = &arquivo[ini..fim];
        for am in mp4::fragmento(seg, padrao) {
            quadros += 1;
            if let Err(e) = dec.decodificar(&seg[am.inicio..am.inicio + am.tamanho as usize], &mut pcm) {
                erros += 1;
                if erros < 5 {
                    println!("quadro {quadros}: {e}");
                }
            }
        }
    }
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    let pico = pcm.iter().map(|v| (*v as i32).abs()).max().unwrap_or(0);
    let rms = (pcm.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / pcm.len().max(1) as f64).sqrt();
    let (mut borda, mut dentro) = (0i32, 0i32);
    for i in 1..pcm.len() / 2 {
        let d = (pcm[2 * i] as i32 - pcm[2 * i - 2] as i32).abs();
        if i % 1024 == 0 { borda = borda.max(d) } else { dentro = dentro.max(d) }
    }
    println!("{quadros} quadros, {erros} erros, {:.1} s de som, pico {pico}, rms {rms:.0}, salto borda {borda} / dentro {dentro}, {:.0}x tempo real", pcm.len() as f64 / 2.0 / dec.taxa as f64, (pcm.len() / 2) as f64 / dec.taxa as f64 * 1000.0 / ms);
    gravar_wav(&teste_web::png::pasta().join("aac_youtube.wav"), dec.taxa, &pcm);
    assert_eq!(erros, 0);
    assert!(pico > 1000 && rms > 100.0, "nivel estranho");
    assert!(borda <= dentro, "estalo na borda dos quadros");
}
