//! Vetores de teste oficiais: FIPS 180-4, RFC 4231 (HMAC), RFC 5869 (HKDF),
//! FIPS 197 (AES), McGrew-Viega (GCM), RFC 8439 (ChaCha20-Poly1305) e
//! RFC 7748 (X25519).

use teste_cripto::cripto::{aes, chacha, hmac, sha2, x25519};
use sha2::{Resumo, Sha256, Sha384, Sha512};

fn hex(s: &str) -> Vec<u8> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

fn resumo<R: Resumo>(dados: &[u8]) -> Vec<u8> {
    let mut s = vec![0u8; R::TAMANHO];
    sha2::resumir::<R>(dados, &mut s);
    s
}

#[test]
fn sha256() {
    assert_eq!(resumo::<Sha256>(b""), hex("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"));
    assert_eq!(resumo::<Sha256>(b"abc"), hex("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"));
    assert_eq!(
        resumo::<Sha256>(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
        hex("248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1")
    );
    // um milhao de 'a', em pedacos de tamanhos variados
    let mut r = Sha256::novo();
    let a = vec![b'a'; 1000];
    let mut faltam = 1_000_000;
    let mut passo = 1;
    while faltam > 0 {
        let n = passo.min(faltam).min(1000);
        r.atualizar(&a[..n]);
        faltam -= n;
        passo = passo % 997 + 7;
    }
    let mut s = [0u8; 32];
    r.finalizar(&mut s);
    assert_eq!(s.to_vec(), hex("cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"));
}

#[test]
fn sha384_512() {
    assert_eq!(
        resumo::<Sha384>(b"abc"),
        hex("cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7")
    );
    assert_eq!(
        resumo::<Sha512>(b"abc"),
        hex("ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f")
    );
    assert_eq!(
        resumo::<Sha512>(b""),
        hex("cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e")
    );
}

#[test]
fn hmac_rfc4231() {
    let mut s = [0u8; 64];
    hmac::hmac::<Sha256>(&[0x0b; 20], b"Hi There", &mut s);
    assert_eq!(s[..32].to_vec(), hex("b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"));
    hmac::hmac::<Sha256>(b"Jefe", b"what do ya want for nothing?", &mut s);
    assert_eq!(s[..32].to_vec(), hex("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"));
    hmac::hmac::<Sha512>(b"Jefe", b"what do ya want for nothing?", &mut s);
    assert_eq!(
        s.to_vec(),
        hex("164b7a7bfcf819e2e395fbe73b56e0a387bd64222e831fd610270cd7ea2505549758bf75c05a994a6d034f65f8f0e6fdcaeab1a34d4a6b4b636e070a38bce737")
    );
    // chave maior que o bloco (caso 6)
    hmac::hmac::<Sha256>(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First", &mut s);
    assert_eq!(s[..32].to_vec(), hex("60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"));
}

#[test]
fn hkdf_rfc5869() {
    let prk = hmac::extrair::<Sha256>(&hex("000102030405060708090a0b0c"), &[0x0b; 22]);
    assert_eq!(prk, hex("077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5"));
    let okm = hmac::expandir::<Sha256>(&prk, &hex("f0f1f2f3f4f5f6f7f8f9"), 42);
    assert_eq!(
        okm,
        hex("3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865")
    );
}

#[test]
fn aes_fips197() {
    let mut b = [0u8; 16];
    b.copy_from_slice(&hex("00112233445566778899aabbccddeeff"));
    let mut b2 = b;
    aes::Aes::novo(&hex("000102030405060708090a0b0c0d0e0f")).unwrap().cifrar(&mut b);
    assert_eq!(b.to_vec(), hex("69c4e0d86a7b0430d8cdb78070b4c55a"));
    aes::Aes::novo(&hex("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f")).unwrap().cifrar(&mut b2);
    assert_eq!(b2.to_vec(), hex("8ea2b7ca516745bfeafc49904b496089"));
}

#[test]
fn aes_gcm() {
    let nonce = [0u8; 12];
    let g = aes::AesGcm::novo(&[0; 16]).unwrap();
    assert_eq!(g.selar(&nonce, &[], &mut []).to_vec(), hex("58e2fccefa7e3061367f1d57a4e7455a"));
    let mut p = [0u8; 16];
    let t = g.selar(&nonce, &[], &mut p);
    assert_eq!(p.to_vec(), hex("0388dace60b6a392f328c2b971b2fe78"));
    assert_eq!(t.to_vec(), hex("ab6e47d42cec13bdf53a67b21257bddf"));

    // caso 4: com dados adicionais e texto que nao fecha bloco
    let g = aes::AesGcm::novo(&hex("feffe9928665731c6d6a8f9467308308")).unwrap();
    let mut n = [0u8; 12];
    n.copy_from_slice(&hex("cafebabefacedbaddecaf888"));
    let aad = hex("feedfacedeadbeeffeedfacedeadbeefabaddad2");
    let claro = hex(
        "d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b39",
    );
    let mut d = claro.clone();
    let t = g.selar(&n, &aad, &mut d);
    assert_eq!(
        d,
        hex("42831ec2217774244b7221b784d0d49ce3aa212f2c02a4e035c17e2329aca12e21d514b25466931c7d8f6a5aac84aa051ba30b396a0aac973d58e091")
    );
    assert_eq!(t.to_vec(), hex("5bc94fbc3221a5db94fae95ae7121a47"));
    let mut mexido = d.clone();
    mexido[3] ^= 1;
    assert!(!g.abrir(&n, &aad, &mut mexido, &t));
    assert!(g.abrir(&n, &aad, &mut d, &t));
    assert_eq!(d, claro);
}

#[test]
fn chacha20_bloco() {
    let mut chave = [0u8; 32];
    for (i, b) in chave.iter_mut().enumerate() {
        *b = i as u8;
    }
    let mut n = [0u8; 12];
    n.copy_from_slice(&hex("000000090000004a00000000"));
    let b = chacha::bloco(&chave, 1, &n);
    assert_eq!(
        b.to_vec(),
        hex("10f1e7e4d13b5915500fdd1fa32071c4c7d1f4c733c068030422aa9ac3d46c4ed2826446079faa0914c2d705d98b02a2b5129cd1de164eb9cbd083e8a2503c4e")
    );
}

#[test]
fn poly1305() {
    let mut k = [0u8; 32];
    k.copy_from_slice(&hex("85d6be7857556d337f4452fe42d506a80103808afb0db2fd4abff6af4149f51b"));
    let mut p = chacha::Poly1305::novo(&k);
    p.atualizar(b"Cryptographic Forum Research Group");
    assert_eq!(p.finalizar().to_vec(), hex("a8061dc1305136c6c22b8baf0c0127a9"));
}

#[test]
fn chacha20_poly1305() {
    let mut chave = [0u8; 32];
    for (i, b) in chave.iter_mut().enumerate() {
        *b = 0x80 + i as u8;
    }
    let mut n = [0u8; 12];
    n.copy_from_slice(&hex("070000004041424344454647"));
    let aad = hex("50515253c0c1c2c3c4c5c6c7");
    let claro = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.".to_vec();
    let mut d = claro.clone();
    let t = chacha::selar(&chave, &n, &aad, &mut d);
    assert_eq!(d[..16].to_vec(), hex("d31a8d34648e60db7b86afbc53ef7ec2"));
    assert_eq!(t.to_vec(), hex("1ae10b594f09e26a7e902ecbd0600691"));
    assert!(chacha::abrir(&chave, &n, &aad, &mut d, &t));
    assert_eq!(d, claro);
}

#[test]
fn x25519_rfc7748() {
    let mut a = [0u8; 32];
    a.copy_from_slice(&hex("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a"));
    let mut b = [0u8; 32];
    b.copy_from_slice(&hex("5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb"));
    let pa = x25519::publica(&a);
    let pb = x25519::publica(&b);
    assert_eq!(pa.to_vec(), hex("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a"));
    assert_eq!(pb.to_vec(), hex("de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f"));
    let s1 = x25519::multiplicar_ponto(&a, &pb);
    let s2 = x25519::multiplicar_ponto(&b, &pa);
    assert_eq!(s1, s2);
}
