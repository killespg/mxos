//! Perguntas ao Claude pelo sistema (crate::ia no kernel): a chave da API
//! fica na configuracao do MXOS e o app nunca ve. Sincrono: a resposta
//! pode levar dezenas de segundos (mostre um aviso antes).

use alloc::format;
use alloc::string::String;

use crate::sys::{self, chamada};

/// Manda `pergunta` (com `sistema` como instrucao) e devolve o texto da
/// resposta, ou uma mensagem de erro para mostrar.
pub fn perguntar(sistema: &str, pergunta: &str) -> Result<String, String> {
    let id = chamada(sys::IA_PERGUNTAR, [sistema.as_ptr() as usize, sistema.len(), pergunta.as_ptr() as usize, pergunta.len(), 0, 0]);
    if id < 0 {
        return Err(format!("o sistema recusou o pedido (erro {id})"));
    }
    let r = crate::http::esperar(id as usize).map_err(|e| format!("erro {e}"))?;
    let texto = String::from_utf8_lossy(&r.corpo).into_owned();
    if r.status == 200 { Ok(texto) } else { Err(texto) }
}
