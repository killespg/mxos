//! Perguntas ao Claude pela API de mensagens da Anthropic, com o HTTPS e o
//! TLS do proprio MXOS. A chave fica na configuracao `claude_chave` (e o
//! modelo em `claude_modelo`, padrao claude-opus-5-5). Usado pelo botao de
//! IA do editor de codigo.
//!
//! O pedido e sincrono (a resposta pode levar dezenas de segundos): quem
//! chama mostra um aviso antes, como nas tarefas de disco.

use alloc::format;
use alloc::string::String;

use crate::json::{self, Json};

const URL: &str = "https://api.anthropic.com/v1/messages";
const MODELO_PADRAO: &str = "claude-opus-5-5";
const VERSAO_API: &str = "2026-10-21";
/// Se o modelo recusar o pedido, o servidor tenta outro modelo sozinho.
const BETA_RESERVA: &str = "server-side-fallback-2026-07-01";
const TEMPO_MS: u64 = 180_000;
const MAX_TOKENS: u32 = 16_000;

/// Manda `pergunta` (com `sistema` como instrucao) e devolve o texto da
/// resposta, ou uma mensagem de erro para mostrar.
pub fn perguntar(sistema: &str, pergunta: &str) -> Result<String, String> {
    let cfg = crate::config::Config::carregar();
    let chave = cfg.obter("claude_chave").map(str::trim).filter(|c| !c.is_empty()).ok_or_else(|| {
        String::from("Falta a chave da API: defina claude_chave nas configurações (comando: config claude_chave sk-ant-...)")
    })?;
    let modelo = cfg.obter("claude_modelo").map(str::trim).filter(|m| !m.is_empty()).unwrap_or(MODELO_PADRAO);
    let corpo = format!(
        "{{\"model\":{},\"max_tokens\":{MAX_TOKENS},\"fallbacks\":\"default\",\"system\":{},\"messages\":[{{\"role\":\"user\",\"content\":{}}}]}}",
        json::texto(modelo),
        json::texto(sistema),
        json::texto(pergunta)
    );
    let cabecalhos = [
        ("Content-Type", "application/json"),
        ("x-api-key", chave),
        ("anthropic-version", VERSAO_API),
        ("anthropic-beta", BETA_RESERVA),
    ];
    let r = crate::rede::http_post(URL, &cabecalhos, corpo.as_bytes(), TEMPO_MS).map_err(|e| format!("Rede: {}", e.descricao()))?;
    let doc = json::ler(&r.corpo).ok_or_else(|| format!("Resposta inválida da API (HTTP {})", r.status))?;
    if r.status != 200 {
        let msg = doc.campo("error").and_then(|e| e.campo("message")).and_then(Json::texto).unwrap_or("erro desconhecido");
        return Err(format!("API respondeu {}: {msg}", r.status));
    }
    if doc.campo("stop_reason").and_then(Json::texto) == Some("refusal") {
        return Err(String::from("O modelo recusou o pedido."));
    }
    // o texto vem nos blocos "text" (os de "thinking" ficam de fora)
    let mut texto = String::new();
    for bloco in doc.campo("content").map(Json::lista).unwrap_or(&[]) {
        if bloco.campo("type").and_then(Json::texto) == Some("text") {
            if let Some(t) = bloco.campo("text").and_then(Json::texto) {
                texto.push_str(t);
            }
        }
    }
    if doc.campo("stop_reason").and_then(Json::texto) == Some("max_tokens") {
        texto.push_str("\n\n[resposta cortada: limite de tokens]");
    }
    if texto.is_empty() { Err(String::from("A resposta veio vazia")) } else { Ok(texto) }
}
