//! Midia: MOD, WAV com troca de taxa e AVI/MJPEG, com arquivos montados aqui.

use teste_web::midia::{self, Audio, Reamostrador};

fn be16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_be_bytes());
}

/// MOD de 4 canais: amostra 1 quadrada com loop, notas e efeitos.
fn mod_teste() -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(b"teste mxos\0\0\0\0\0\0\0\0\0\0");
    for i in 0..31 {
        d.extend_from_slice(&[0u8; 22]);
        if i == 0 {
            be16(&mut d, 32); // 64 bytes
            d.push(0);
            d.push(64);
            be16(&mut d, 0);
            be16(&mut d, 32); // loop inteiro
        } else {
            be16(&mut d, 0);
            d.push(0);
            d.push(0);
            be16(&mut d, 0);
            be16(&mut d, 1);
        }
    }
    d.push(2); // 2 posicoes
    d.push(0);
    let mut ordem = [0u8; 128];
    ordem[1] = 1;
    d.extend_from_slice(&ordem);
    d.extend_from_slice(b"M.K.");
    for p in 0..2 {
        for linha in 0..64 {
            for canal in 0..4 {
                let cel: [u8; 4] = if linha % 8 == 0 && canal == p {
                    // C-2 (428), amostra 1, volume slide / vibrato
                    [0x01, 0xAC, 0x1A, 0x01]
                } else if linha == 0 && canal == 3 {
                    [0, 0, 0x0F, 0x03] // velocidade 3
                } else {
                    [0, 0, 0, 0]
                };
                d.extend_from_slice(&cel);
            }
        }
    }
    for k in 0..64 {
        d.push(if k < 32 { 100 } else { (-100i8) as u8 });
    }
    d
}

#[test]
fn tracker_toca_e_termina() {
    let mut f = mod_teste();
    let mut a = midia::abrir_audio(Box::new(std::mem::take(&mut f)), "x.mod", 48000).unwrap();
    assert!(a.descricao().contains("teste mxos"));
    let mut buf = vec![0i16; 4096];
    let mut total = 0usize;
    let mut pico = 0i32;
    loop {
        let n = a.ler(&mut buf);
        if n == 0 {
            break;
        }
        total += n / 2;
        pico = pico.max(buf[..n].iter().map(|&x| (x as i32).abs()).max().unwrap_or(0));
        assert!(total < 48000 * 60, "nao terminou");
    }
    // 2 padroes x 64 linhas x 3 tiques x 20 ms = 7,68 s
    let s = total as f64 / 48000.0;
    println!("MOD: {s:.2} s, pico {pico}");
    assert!((s - 7.68).abs() < 0.1, "{s}");
    assert!(pico > 4000);
}

fn wav(taxa: u32, canais: u16, amostras: &[i16]) -> Vec<u8> {
    let dados: Vec<u8> = amostras.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut w = Vec::new();
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + dados.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&canais.to_le_bytes());
    w.extend_from_slice(&taxa.to_le_bytes());
    w.extend_from_slice(&(taxa * canais as u32 * 2).to_le_bytes());
    w.extend_from_slice(&(canais * 2).to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(dados.len() as u32).to_le_bytes());
    w.extend_from_slice(&dados);
    w
}

#[test]
fn wav_mono_8k_para_48k() {
    // 1 s de rampa em 8 kHz mono
    let amostras: Vec<i16> = (0..8000).map(|i| (i * 4) as i16).collect();
    let mut a = midia::abrir_audio(Box::new(wav(8000, 1, &amostras)), "x.wav", 48000).unwrap();
    assert_eq!(a.duracao_ms(), Some(1000));
    let mut r = Reamostrador::novo(a.taxa(), 48000);
    let mut buf = vec![0i16; 1000];
    let mut todas = Vec::new();
    loop {
        let n = r.ler(&mut *a, &mut buf);
        if n == 0 {
            break;
        }
        todas.extend_from_slice(&buf[..n]);
        if todas.len() > 200_000 {
            panic!("nao acaba: pos {} n {n} acabou {}", a.posicao_ms(), r.acabou());
        }
    }
    let quadros = todas.len() / 2;
    assert!((quadros as i32 - 48000).abs() < 20, "{quadros}");
    // a rampa cresce e os dois lados sao iguais
    assert!(todas.chunks(2).all(|p| p[0] == p[1]));
    assert!(todas[2 * 24000] > todas[2 * 12000]);
}

#[test]
fn avi_mjpeg() {
    let pasta = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/paginas/");
    let Ok(jpeg) = std::fs::read(format!("{pasta}https___images_ctfassets_net_8aevphvgewt8_home2026_01ac80921a7a68bb3d4ce527_1f45_ac95b2871582936b")) else {
        return;
    };
    // RIFF AVI com hdrl (avih + strl/strh vids MJPG) e movi com 3 quadros
    let mut avih = vec![0u8; 56];
    avih[0..4].copy_from_slice(&40_000u32.to_le_bytes());
    avih[32..36].copy_from_slice(&320u32.to_le_bytes());
    avih[36..40].copy_from_slice(&240u32.to_le_bytes());
    let mut strh = vec![0u8; 56];
    strh[0..4].copy_from_slice(b"vids");
    strh[4..8].copy_from_slice(b"MJPG");
    let pedaco = |id: &[u8], dados: &[u8]| {
        let mut v = id.to_vec();
        v.extend_from_slice(&(dados.len() as u32).to_le_bytes());
        v.extend_from_slice(dados);
        if dados.len() % 2 == 1 {
            v.push(0);
        }
        v
    };
    let lista = |tipo: &[u8], dentro: Vec<u8>| {
        let mut d = tipo.to_vec();
        d.extend(dentro);
        pedaco(b"LIST", &d)
    };
    let strl = lista(b"strl", pedaco(b"strh", &strh));
    let mut hdrl_d = pedaco(b"avih", &avih);
    hdrl_d.extend(strl);
    let hdrl = lista(b"hdrl", hdrl_d);
    let mut movi_d = Vec::new();
    for _ in 0..3 {
        movi_d.extend(pedaco(b"00dc", &jpeg));
    }
    let movi = lista(b"movi", movi_d);
    let mut corpo = b"AVI ".to_vec();
    corpo.extend(hdrl);
    corpo.extend(movi);
    let arquivo = pedaco(b"RIFF", &corpo);
    let mut avi = midia::avi::Avi::abrir(Box::new(arquivo)).unwrap();
    assert_eq!(avi.total_quadros(), 3);
    assert_eq!(avi.duracao_ms(), 120);
    let q = avi.quadro(2).expect("quadro decodificado");
    println!("{} -> quadro {}x{}", avi.descricao(), q.largura, q.altura);
    assert!(q.largura > 0);
}
