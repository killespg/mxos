//! Chamadas de sistema cruas da ABI do MXOS (no kernel: processo/abi.rs).
//! ARMv7: numero em r7, argumentos r0-r5, resultado r0 (r1 = parte de
//! cima nos valores de 64 bits). AArch64: x8, x0-x5, x0. Erros voltam
//! negativos (ERRO_*).

pub const SAIR: usize = 0;
pub const LOG: usize = 1;
pub const TEMPO_US: usize = 2;
pub const DORMIR_MS: usize = 3;
pub const MAPEAR: usize = 4;
pub const DESMAPEAR: usize = 5;
pub const ABRIR: usize = 6;
pub const LER: usize = 7;
pub const ESCREVER: usize = 8;
pub const FECHAR: usize = 9;
pub const TAMANHO: usize = 10;
pub const POSICIONAR: usize = 11;
pub const LISTAR: usize = 12;
pub const JANELA_CRIAR: usize = 13;
pub const JANELA_MOSTRAR: usize = 14;
pub const JANELA_EVENTO: usize = 15;
pub const ESPERAR_EVENTO: usize = 16;
pub const JANELA_TITULO: usize = 17;
pub const JANELA_FECHAR: usize = 18;
pub const HTTP_PEDIR: usize = 19;
pub const HTTP_ESTADO: usize = 20;
pub const HTTP_LER: usize = 21;
pub const SOM: usize = 22;
pub const ARGUMENTOS: usize = 23;
pub const TELA: usize = 24;
pub const TEMA: usize = 25;
pub const COPIAR: usize = 26;
pub const COLAR: usize = 27;

pub const ERRO_INVALIDO: isize = -1;
pub const ERRO_NAO_EXISTE: isize = -2;
pub const ERRO_MEMORIA: isize = -3;
pub const ERRO_ENDERECO: isize = -4;
pub const ERRO_DISCO: isize = -5;
pub const ERRO_REDE: isize = -6;
pub const ERRO_DESCONHECIDA: isize = -38;

/// Chamada com ate 6 argumentos. Devolve (r0, r1) no ARMv7 e (x0, 0) no
/// AArch64.
#[cfg(target_arch = "arm")]
#[inline(always)]
pub fn chamar(numero: usize, a: [usize; 6]) -> (usize, usize) {
    let (r0, r1);
    // o r7 pode ser o frame pointer: guardado em volta do svc
    unsafe {
        core::arch::asm!(
            "push {{r7}}",
            "mov r7, {n}",
            "svc 0",
            "pop {{r7}}",
            n = in(reg) numero,
            inlateout("r0") a[0] => r0,
            inlateout("r1") a[1] => r1,
            in("r2") a[2],
            in("r3") a[3],
            in("r4") a[4],
            in("r5") a[5],
            options(nostack),
        );
    }
    (r0, r1)
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub fn chamar(numero: usize, a: [usize; 6]) -> (usize, usize) {
    let x0;
    unsafe {
        core::arch::asm!(
            "svc 0",
            in("x8") numero,
            inlateout("x0") a[0] => x0,
            in("x1") a[1],
            in("x2") a[2],
            in("x3") a[3],
            in("x4") a[4],
            in("x5") a[5],
            options(nostack),
        );
    }
    (x0, 0)
}

/// Resultado de 32 bits (ou do tamanho do ponteiro), com sinal.
#[inline(always)]
pub fn chamada(numero: usize, a: [usize; 6]) -> isize {
    #[cfg(target_arch = "arm")]
    {
        chamar(numero, a).0 as i32 as isize
    }
    #[cfg(not(target_arch = "arm"))]
    {
        chamar(numero, a).0 as isize
    }
}

/// Resultado de 64 bits sem sinal (TEMPO_US, TAMANHO, TELA).
#[inline(always)]
pub fn chamada64(numero: usize, a: [usize; 6]) -> u64 {
    let (baixo, alto) = chamar(numero, a);
    #[cfg(target_arch = "arm")]
    {
        baixo as u64 | (alto as u64) << 32
    }
    #[cfg(not(target_arch = "arm"))]
    {
        let _ = alto;
        baixo as u64
    }
}

pub fn sair(codigo: i32) -> ! {
    chamar(SAIR, [codigo as usize, 0, 0, 0, 0, 0]);
    loop {
        core::hint::spin_loop();
    }
}

/// Escreve no terminal do sistema (depuracao).
pub fn log(texto: &str) {
    chamada(LOG, [texto.as_ptr() as usize, texto.len(), 0, 0, 0, 0]);
}

pub fn tempo_us() -> u64 {
    chamada64(TEMPO_US, [0; 6])
}

pub fn dormir_ms(ms: u32) {
    chamada(DORMIR_MS, [ms as usize, 0, 0, 0, 0, 0]);
}

/// `tamanho` bytes de memoria zerada (alinhada na pagina) ou None.
pub fn mapear(tamanho: usize) -> Option<*mut u8> {
    let r = chamada(MAPEAR, [tamanho, 0, 0, 0, 0, 0]);
    (r > 0).then_some(r as usize as *mut u8)
}

pub fn desmapear(ptr: *mut u8, tamanho: usize) {
    chamada(DESMAPEAR, [ptr as usize, tamanho, 0, 0, 0, 0]);
}

/// Largura e altura da tela.
pub fn tela() -> (u32, u32) {
    let v = chamada64(TELA, [0; 6]);
    (v as u32, (v >> 32) as u32)
}
