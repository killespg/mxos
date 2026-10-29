//! Grava um quadro XRGB em PNG (deflate "guardado", sem compressao: so
//! para os testes olharem o resultado).

fn crc32(dados: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in dados {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

fn pedaco(saida: &mut Vec<u8>, tipo: &[u8; 4], dados: &[u8]) {
    saida.extend_from_slice(&(dados.len() as u32).to_be_bytes());
    let mut t = tipo.to_vec();
    t.extend_from_slice(dados);
    saida.extend_from_slice(&t);
    saida.extend_from_slice(&crc32(&t).to_be_bytes());
}

pub fn gravar(caminho: &std::path::Path, largura: usize, altura: usize, px: &[u32]) {
    let mut cru = Vec::with_capacity((largura * 3 + 1) * altura);
    for y in 0..altura {
        cru.push(0);
        for x in 0..largura {
            let p = px[y * largura + x];
            cru.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
        }
    }
    let mut z = vec![0x78, 0x01];
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &cru {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    let pedacos: Vec<&[u8]> = cru.chunks(65535).collect();
    for (k, p) in pedacos.iter().enumerate() {
        z.push(if k + 1 == pedacos.len() { 1 } else { 0 });
        z.extend_from_slice(&(p.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(p.len() as u16)).to_le_bytes());
        z.extend_from_slice(p);
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut s = vec![0x89, b'P', b'N', b'G', 13, 10, 26, 10];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(largura as u32).to_be_bytes());
    ihdr.extend_from_slice(&(altura as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    pedaco(&mut s, b"IHDR", &ihdr);
    pedaco(&mut s, b"IDAT", &z);
    pedaco(&mut s, b"IEND", &[]);
    if let Some(p) = caminho.parent() {
        std::fs::create_dir_all(p).ok();
    }
    std::fs::write(caminho, s).unwrap();
}

/// Pasta das imagens dos testes (target/saidas).
pub fn pasta() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/saidas")
}
