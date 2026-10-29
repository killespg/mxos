//! "Atualizar o sistema": abre parado; so procura quando o usuario clica
//! em "Procurar atualização", e so instala quando ele clica em "Instalar e
//! reiniciar" (crate::atualizacao faz o trabalho).

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;

use crate::atualizacao::{self, Atualizacao};
use crate::entrada::Botao;
use crate::fonte::{INTER, INTER_NEGRITO};
use crate::gui::grafico::{Retangulo, Superficie};
use crate::gui::janelas::{App, EventoApp};
use crate::gui::{tema, widgets};

const LARGURA: usize = 560;
const ALTURA: usize = 460;
const ALTURA_LINHA: i32 = 20;

pub struct Atualizar {
    a: Atualizacao,
}

pub fn criar() -> Box<dyn App> {
    Box::new(Atualizar { a: Atualizacao::default() })
}

fn botao_procurar() -> Retangulo {
    Retangulo::novo(16, ALTURA as i32 - 44, 200, widgets::ALTURA_BOTAO)
}

fn botao_instalar() -> Retangulo {
    Retangulo::novo(LARGURA as i32 - 216, ALTURA as i32 - 44, 200, widgets::ALTURA_BOTAO)
}

impl App for Atualizar {
    fn titulo(&self) -> &str {
        "Atualizar o sistema"
    }

    fn tamanho(&self) -> (usize, usize) {
        (LARGURA, ALTURA)
    }

    fn desenhar(&mut self, s: &mut Superficie) {
        let t = tema::atual();
        s.limpar(t.fundo_janela);
        let mut base = 16 + INTER.ascendente;
        s.texto_com(16, base, "Atualizar o MXOS", t.texto, &INTER_NEGRITO);
        base += ALTURA_LINHA + 4;
        let i = &self.a.info;
        let ultima = if i.ultima_busca.is_empty() { "nunca" } else { i.ultima_busca.as_str() };
        let mut campos = vec![("Versão instalada", i.versao_atual.clone()), ("Última busca", String::from(ultima))];
        if !i.origem.is_empty() {
            campos.push(("Origem", i.origem.clone()));
        }
        if let Some(nova) = &i.versao_nova {
            campos.push(("Versão nova", nova.clone()));
        }
        if let Some(bytes) = i.tamanho {
            campos.push(("Tamanho", atualizacao::tamanho_texto(bytes)));
        }
        for (nome, valor) in &campos {
            s.texto_com(16, base, nome, t.texto, &INTER_NEGRITO);
            s.texto_com(150, base, valor, t.texto, &INTER);
            base += ALTURA_LINHA;
        }
        if !i.novidades.is_empty() {
            base += 4;
            s.texto_com(16, base, "O que mudou:", t.texto, &INTER_NEGRITO);
            base += ALTURA_LINHA;
            for n in i.novidades.iter().take(6) {
                s.texto_com(24, base, &format!("• {n}"), t.texto, &INTER);
                base += ALTURA_LINHA;
            }
        }
        // as ultimas mensagens do que esta acontecendo
        base += 4;
        let cabem = ((ALTURA as i32 - base - 70) / ALTURA_LINHA).max(0) as usize;
        let inicio = self.a.linhas.len().saturating_sub(cabem);
        for linha in &self.a.linhas[inicio..] {
            s.texto_com(16, base, linha, crate::gui::grafico::misturar(t.texto, t.fundo_janela, 40), &INTER);
            base += ALTURA_LINHA;
        }
        let (feito, total) = self.a.feito;
        if total > 0 {
            let r = Retangulo::novo(16, ALTURA as i32 - 64, LARGURA as i32 - 32, 10);
            s.retangulo(r, t.botao);
            let cheio = (r.largura as u64 * feito.min(total) as u64 / total as u64) as i32;
            s.retangulo(Retangulo::novo(r.x, r.y, cheio.max(1), r.altura), t.destaque);
            s.contorno(r, t.borda, 1);
        }
        let (procurar, instalar) = self.a.botoes();
        if procurar {
            widgets::botao(s, botao_procurar(), "Procurar atualização", true);
        }
        if instalar {
            widgets::botao(s, botao_instalar(), "Instalar e reiniciar", true);
        }
    }

    fn evento(&mut self, e: EventoApp) -> bool {
        match e {
            EventoApp::Tique | EventoApp::Processos => self.a.avancar(),
            EventoApp::Botao { x, y, botao: Botao::Esquerdo, pressionado: true } => {
                let (procurar, instalar) = self.a.botoes();
                if procurar && botao_procurar().contem(x, y) {
                    self.a.procurar(None);
                    return true;
                }
                if instalar && botao_instalar().contem(x, y) {
                    self.a.instalar();
                    return true;
                }
                false
            }
            _ => false,
        }
    }
}
