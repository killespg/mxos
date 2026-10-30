//! HTTP(S) pelo sistema (o kernel faz a conexao e o TLS).

use alloc::vec;
use alloc::vec::Vec;

use crate::sys::{self, chamada};

/// Uma resposta: o status HTTP e o corpo.
pub struct Resposta {
    pub status: u32,
    pub corpo: Vec<u8>,
}

/// GET em `url`. Erro = codigo negativo da ABI (sys::ERRO_REDE...).
pub fn baixar(url: &str) -> Result<Resposta, isize> {
    let id = chamada(sys::HTTP_PEDIR, [url.as_ptr() as usize, url.len(), 0, 0, 0, 0]);
    if id < 0 {
        return Err(id);
    }
    esperar(id as usize)
}

/// Espera o pedido `id` (HTTP_PEDIR, IA_PERGUNTAR) terminar e le tudo.
pub(crate) fn esperar(id: usize) -> Result<Resposta, isize> {
    let (status, tamanho) = loop {
        let v = sys::chamada64(sys::HTTP_ESTADO, [id, 0, 0, 0, 0, 0]) as i64;
        if v < 0 {
            return Err(v as isize);
        }
        if v != 0 {
            break ((v >> 32 & 0x3FFF_FFFF) as u32, (v & 0xFFFF_FFFF) as usize);
        }
        sys::dormir_ms(20);
    };
    let mut corpo = vec![0u8; tamanho];
    let mut feito = 0;
    while feito < tamanho {
        let n = chamada(sys::HTTP_LER, [id, corpo[feito..].as_mut_ptr() as usize, tamanho - feito, 0, 0, 0]);
        if n <= 0 {
            break;
        }
        feito += n as usize;
    }
    corpo.truncate(feito);
    Ok(Resposta { status, corpo })
}
