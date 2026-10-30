//! Vetores de teste oficiais da cripto do kernel.

use teste_web::cripto::aes::AesGcm;

fn hex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

/// NIST GCM, casos 2, 3 e 4 (AES-128).
#[test]
fn aes_gcm_nist() {
    let casos = [
        (
            "00000000000000000000000000000000",
            "000000000000000000000000",
            "",
            "00000000000000000000000000000000",
            "0388dace60b6a392f328c2b971b2fe78",
            "ab6e47d42cec13bdf53a67b21257bddf",
        ),
        (
            "feffe9928665731c6d6a8f9467308308",
            "cafebabefacedbaddecaf888",
            "",
            "d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b391aafd255",
            "42831ec2217774244b7221b784d0d49ce3aa212f2c02a4e035c17e2329aca12e21d514b25466931c7d8f6a5aac84aa051ba30b396a0aac973d58e091473f5985",
            "4d5c2af327cd64a62cf35abd2ba6fab4",
        ),
        (
            "feffe9928665731c6d6a8f9467308308",
            "cafebabefacedbaddecaf888",
            "feedfacedeadbeeffeedfacedeadbeefabaddad2",
            "d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b39",
            "42831ec2217774244b7221b784d0d49ce3aa212f2c02a4e035c17e2329aca12e21d514b25466931c7d8f6a5aac84aa051ba30b396a0aac973d58e091",
            "5bc94fbc3221a5db94fae95ae7121a47",
        ),
    ];
    for (k, iv, aad, pt, ct, tag) in casos {
        let g = AesGcm::novo(&hex(k)).unwrap();
        let nonce: [u8; 12] = hex(iv).try_into().unwrap();
        let mut dados = hex(pt);
        let t = g.selar(&nonce, &hex(aad), &mut dados);
        assert_eq!(dados, hex(ct));
        assert_eq!(t.to_vec(), hex(tag));
        assert!(g.abrir(&nonce, &hex(aad), &mut dados, &t));
        assert_eq!(dados, hex(pt));
    }
}
