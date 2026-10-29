//! Gerador de numeros aleatorios para criptografia (chaves do TLS, nonces,
//! getrandom dos programas). Um "poco" SHA-256 junta entropia (o contador
//! de ciclos em momentos imprevisiveis: pacotes chegando, teclas,
//! interrupcoes); a saida e o fluxo da ChaCha20 com a chave tirada do poco,
//! trocada a cada pedido (apagar a chave antiga: quem roubar o estado nao
//! descobre o que ja saiu).

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use super::chacha;
use super::sha2::{Resumo, Sha256};

struct Estado {
    poco: [u8; 32],
    chave: [u8; 32],
    contador: u64,
    misturas: u64,
}

struct Global {
    trava: AtomicBool,
    estado: UnsafeCell<Estado>,
}

unsafe impl Sync for Global {}

static GLOBAL: Global = Global {
    trava: AtomicBool::new(false),
    estado: UnsafeCell::new(Estado { poco: [0; 32], chave: [0; 32], contador: 0, misturas: 0 }),
};

fn com<R>(f: impl FnOnce(&mut Estado) -> R) -> R {
    while GLOBAL.trava.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    let r = f(unsafe { &mut *GLOBAL.estado.get() });
    GLOBAL.trava.store(false, Ordering::Release);
    r
}

/// Junta entropia ao poco (barato: chamar a vontade).
pub fn misturar(dados: &[u8]) {
    com(|e| {
        let mut h = Sha256::novo();
        h.atualizar(&e.poco);
        h.atualizar(&e.misturas.to_le_bytes());
        h.atualizar(dados);
        h.finalizar(&mut e.poco);
        e.misturas += 1;
    });
}

/// Enche `saida` de bytes aleatorios. `extra` (o contador de ciclos de
/// agora, por exemplo) entra no poco antes.
pub fn preencher(saida: &mut [u8], extra: u64) {
    com(|e| {
        // chave nova = H(chave antiga, poco, extra)
        let mut h = Sha256::novo();
        h.atualizar(&e.chave);
        h.atualizar(&e.poco);
        h.atualizar(&extra.to_le_bytes());
        h.atualizar(&e.contador.to_le_bytes());
        h.finalizar(&mut e.chave);
        e.contador += 1;
        let mut nonce = [0u8; 12];
        nonce[..8].copy_from_slice(&e.contador.to_le_bytes());
        saida.fill(0);
        chacha::cifrar(&e.chave, 0, &nonce, saida);
        // a chave usada vira outra (apagamento para frente)
        let b = chacha::bloco(&e.chave, u32::MAX, &nonce);
        e.chave.copy_from_slice(&b[..32]);
    });
}

/// Quantas vezes o poco recebeu entropia.
pub fn misturas() -> u64 {
    com(|e| e.misturas)
}
