//! MP3: decodifica as falas MP3 do Windows, grava WAV e um espectrograma em
//! target/saidas para conferir.

use teste_web::midia::{self, Audio};

fn decodificar(caminho: &str) -> Option<(Box<dyn Audio>, Vec<i16>, f64)> {
    let dados = std::fs::read(caminho).ok()?;
    let mut a = midia::abrir_audio(Box::new(dados), caminho, 44100).expect("abre");
    let t = std::time::Instant::now();
    let mut todas = Vec::new();
    let mut buf = vec![0i16; 8192];
    loop {
        let n = a.ler(&mut buf);
        if n == 0 {
            break;
        }
        todas.extend_from_slice(&buf[..n]);
    }
    Some((a, todas, t.elapsed().as_secs_f64()))
}

fn gravar_wav(nome: &str, taxa: u32, pcm: &[i16]) {
    let mut w = Vec::new();
    let dados: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + dados.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&taxa.to_le_bytes());
    w.extend_from_slice(&(taxa * 4).to_le_bytes());
    w.extend_from_slice(&4u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(dados.len() as u32).to_le_bytes());
    w.extend_from_slice(&dados);
    std::fs::write(teste_web::png::pasta().join(nome), w).unwrap();
}

/// Espectrograma simples (DFT de 512 pontos, 0-8 kHz) em PNG.
fn espectrograma(nome: &str, taxa: u32, pcm: &[i16]) {
    let mono: Vec<f32> = pcm.chunks(2).map(|p| p[0] as f32).collect();
    let n = 512;
    let colunas = (mono.len() / 256).min(1200);
    let linhas = 186; // ate ~8 kHz
    let mut px = vec![0u32; colunas * linhas];
    for c in 0..colunas {
        let ini = c * mono.len().saturating_sub(n) / colunas.max(1);
        let janela = &mono[ini..(ini + n).min(mono.len())];
        for k in 0..linhas {
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (i, &x) in janela.iter().enumerate() {
                let w = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos();
                let a = 2.0 * std::f32::consts::PI * k as f32 * i as f32 / n as f32;
                re += x * w * a.cos();
                im -= x * w * a.sin();
            }
            let db = 10.0 * (re * re + im * im + 1.0).log10();
            let v = ((db - 60.0) * 4.0).clamp(0.0, 255.0) as u32;
            px[(linhas - 1 - k) * colunas + c] = (v << 16) | (v << 8) | v;
        }
    }
    let _ = taxa;
    teste_web::png::gravar(&teste_web::png::pasta().join(nome), colunas, linhas, &px);
}

#[test]
fn falas_do_windows() {
    for nome in ["Aria", "Guy"] {
        let caminho = format!("C:/dev/mxos-tmp/{nome}.mp3");
        let Some((a, pcm, s)) = decodificar(&caminho) else { continue };
        let segundos = pcm.len() as f64 / 2.0 / a.taxa() as f64;
        let pico = pcm.iter().map(|&x| (x as i32).abs()).max().unwrap_or(0);
        let saturadas = pcm.iter().filter(|&&x| x == i16::MAX || x == i16::MIN).count();
        let rms = (pcm.iter().map(|&x| (x as f64).powi(2)).sum::<f64>() / pcm.len().max(1) as f64).sqrt();
        println!(
            "{nome}: {} | {segundos:.2} s (cabecalho diz {:?} ms), pico {pico}, rms {rms:.0}, saturadas {saturadas}, decodificou em {:.1} ms ({:.0}x tempo real)",
            a.descricao(),
            a.duracao_ms(),
            s * 1000.0,
            segundos / s
        );
        gravar_wav(&format!("mp3_{nome}.wav"), a.taxa(), &pcm);
        espectrograma(&format!("mp3_{nome}.png"), a.taxa(), &pcm);
        assert!(segundos > 1.0);
        assert!(pico > 1000 && saturadas < pcm.len() / 1000, "pico {pico} saturadas {saturadas}");
    }
}
