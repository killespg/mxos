//! Sobre o MXOS: versao, plataforma, cpu, memoria do heap e tempo ligado.
//! Atualiza uma vez por segundo (no Tique em que o segundo muda).

use alloc::format;
use alloc::string::String;

use crate::fonte::{INTER, INTER_GRANDE, INTER_NEGRITO, INTER_TITULO};
use crate::gui::area;
use crate::gui::grafico::{self, Retangulo, Superficie};
use crate::gui::janelas::{App, EventoApp};
use crate::gui::tema;
use crate::{cpu, heap, plataforma, timer};

const VERSAO: &str = concat!(env!("CARGO_PKG_VERSION_MAJOR"), ".", env!("CARGO_PKG_VERSION_MINOR"));
const LARGURA: usize = 460;
const ALTURA: usize = 240;
const ALTURA_CABECALHO: i32 = 64;
const ALTURA_LINHA: i32 = 21;
/// Onde comecam os valores (os nomes ficam a esquerda, em seminegrito).
const COLUNA_VALOR: i32 = 116;
const BRANCO: u32 = 0x00FF_FFFF;

pub struct Sobre {
    /// Segundo mostrado no "Ligado ha".
    segundo: u64,
}

/// Bytes em KB ou MB, como no comando "mem" do shell.
fn tamanho(bytes: usize) -> String {
    match bytes {
        b if b >= 10 << 20 => format!("{} MB", b >> 20),
        b if b >= 1 << 10 => format!("{} KB", b >> 10),
        b => format!("{b} bytes"),
    }
}

fn segundos_ligado() -> u64 {
    cpu::contador() / cpu::frequencia_timer().max(1)
}

impl Sobre {
    pub fn novo() -> Sobre {
        Sobre { segundo: segundos_ligado() }
    }

    /// Faixa de cima com o nome e a versao.
    fn cabecalho(&self, s: &mut Superficie) {
        let t = tema::atual();
        let r = Retangulo::novo(0, 0, s.largura() as i32, ALTURA_CABECALHO);
        s.gradiente_vertical(r, area::clarear(t.destaque, 30), t.destaque);
        let base = INTER_GRANDE.base_centralizada(0, ALTURA_CABECALHO);
        let x = 18 + s.texto_com(18, base, "MXOS", BRANCO, &INTER_GRANDE) + 16;
        s.texto_com(x, base - 12, &format!("Versão {VERSAO}"), BRANCO, &INTER_TITULO);
        let suave = grafico::misturar(BRANCO, t.destaque, 50);
        s.texto_com(x, base + 5, "Kernel ARM64 escrito do zero em Rust", suave, &INTER);
    }

    /// Barra de uso do heap embaixo do texto.
    fn barra_memoria(&self, s: &mut Superficie, y: i32, usado: usize, total: usize) {
        let t = tema::atual();
        let r = Retangulo::novo(18, y, s.largura() as i32 - 36, 10);
        s.retangulo(r, t.botao);
        let cheio = if total == 0 { 0 } else { (r.largura as u64 * usado as u64 / total as u64) as i32 };
        s.retangulo(Retangulo::novo(r.x, r.y, cheio.max(1), r.altura), t.destaque);
        s.contorno(r, t.borda, 1);
    }
}

impl App for Sobre {
    fn titulo(&self) -> &str {
        "Sobre o MXOS"
    }

    fn tamanho(&self) -> (usize, usize) {
        (LARGURA, ALTURA)
    }

    fn desenhar(&mut self, s: &mut Superficie) {
        let t = tema::atual();
        s.limpar(t.fundo_janela);
        self.cabecalho(s);

        let h = heap::estatisticas();
        let usado = h.total - h.livre;
        let freq = cpu::frequencia_timer();
        let segundos = self.segundo;
        let linhas = [
            ("Plataforma", String::from(plataforma::NOME)),
            ("CPU", String::from(cpu::nome_cpu(cpu::midr()))),
            ("Timer", format!("{},{} MHz, tique a {} Hz", freq / 1_000_000, freq / 100_000 % 10, timer::HZ)),
            ("Memória", format!("{} usados de {}", tamanho(usado), tamanho(h.total))),
            ("Heap", format!("{} alocações, maior bloco livre {}", h.alocacoes, tamanho(h.maior_bloco))),
            ("Ligado há", format!("{}h {:02}m {:02}s", segundos / 3600, segundos / 60 % 60, segundos % 60)),
        ];
        let mut base = ALTURA_CABECALHO + 14 + INTER.ascendente;
        for (nome, valor) in &linhas {
            s.texto_com(18, base, nome, t.texto, &INTER_NEGRITO);
            s.texto_com(COLUNA_VALOR, base, valor, t.texto, &INTER);
            base += ALTURA_LINHA;
        }
        self.barra_memoria(s, base - INTER.ascendente + 8, usado, h.total);
    }

    fn evento(&mut self, e: EventoApp) -> bool {
        if e != EventoApp::Tique {
            return false;
        }
        let agora = segundos_ligado();
        if agora == self.segundo {
            return false;
        }
        self.segundo = agora;
        true
    }
}
