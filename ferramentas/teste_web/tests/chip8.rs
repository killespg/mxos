use teste_web::chip8::Chip8;

#[test]
fn aritmetica_bcd_e_desenho() {
    // V0=0xFE; V1=3; V0+=V1 (VF=1); I=0x300; BCD de V0 (=1) em I; le V2 de I+2;
    // desenha o '1' da fonte em (0,0); depois desenha de novo (VF=1, apaga)
    let rom: [u8; 28] = [
        0x60, 0xFE, 0x61, 0x03, 0x80, 0x14, 0xA3, 0x00, 0xF0, 0x33, 0xA3, 0x02, 0xF0, 0x65, 0x62, 0x01,
        0xF2, 0x29, 0x63, 0x00, 0xD3, 0x35, 0xD3, 0x35, 0x12, 0x18, 0x00, 0x00,
    ];
    let mut c = Chip8::novo(&rom).unwrap();
    for _ in 0..11 {
        c.passo().unwrap();
    }
    assert_eq!(c.v[0], 1);
    assert_eq!(c.mem[0x302], 1);
    assert!(c.pixel(2, 0), "o '1' da fonte (0x20) liga o pixel 2 da primeira linha");
    c.passo().unwrap();
    assert_eq!(c.v[0xF], 1, "colisao ao desenhar por cima");
    assert!(!c.pixel(2, 0));
}

#[test]
fn super_chip_alta_e_rolagem() {
    let rom: [u8; 8] = [0x00, 0xFF, 0x00, 0xC2, 0x00, 0xFB, 0x12, 0x06];
    let mut c = Chip8::novo(&rom).unwrap();
    c.passo().unwrap();
    assert!(c.alta);
    assert_eq!(c.dimensoes(), (128, 64));
    c.passo().unwrap();
    c.passo().unwrap();
}
