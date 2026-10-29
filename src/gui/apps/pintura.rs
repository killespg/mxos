//! Pintura: paleta de cores, tres tamanhos de pincel e um botao de limpar.
//! Botao esquerdo pinta com a cor escolhida, o direito apaga (branco).
//!
//! O desenho mora numa Superficie propria (o quadro); desenhar() so copia
//! o quadro e pinta a barra de ferramentas por cima.

use crate::entrada::Botao;
use crate::gui::area;
use crate::gui::grafico::{Cor, Retangulo, Superficie};
use crate::gui::janelas::{App, EventoApp};
use crate::gui::tema::{self, Tema};

const LARGURA: i32 = 520;
const ALTURA: i32 = 380;
/// Barra de ferramentas em cima; o quadro fica embaixo dela.
const ALTURA_FERRAMENTAS: i32 = 32;
const LADO_COR: i32 = 20;
const PASSO_COR: i32 = 24;
const FUNDO_QUADRO: Cor = 0x00FF_FFFF;

const CORES: [Cor; 14] = [
    0x0000_0000, // preto
    0x0080_8080, // cinza
    0x00FF_FFFF, // branco
    0x00C0_2020, // vermelho
    0x00FF_8C00, // laranja
    0x00FF_D700, // amarelo
    0x0030_A030, // verde
    0x0010_6030, // verde escuro
    0x0020_B0D0, // ciano
    0x0020_50D0, // azul
    0x0010_2070, // azul marinho
    0x0080_30B0, // roxo
    0x00E0_50A0, // rosa
    0x0080_5020, // marrom
];

/// Raios do pincel (0 = 1 pixel).
const PINCEIS: [i32; 3] = [0, 2, 5];

pub struct Pintura {
    quadro: Superficie,
    cor: usize,
    pincel: usize,
    /// Cor do traco em andamento (None = nao esta pintando).
    tinta: Option<Cor>,
    /// Ultimo ponto do traco, em coordenadas do quadro.
    ultimo: (i32, i32),
}

fn retangulo_cor(i: usize) -> Retangulo {
    Retangulo::novo(6 + i as i32 * PASSO_COR, (ALTURA_FERRAMENTAS - LADO_COR) / 2, LADO_COR, LADO_COR)
}

fn retangulo_pincel(i: usize) -> Retangulo {
    let x = retangulo_cor(CORES.len()).x + 8 + i as i32 * PASSO_COR;
    Retangulo::novo(x, (ALTURA_FERRAMENTAS - LADO_COR) / 2, LADO_COR, LADO_COR)
}

fn retangulo_limpar() -> Retangulo {
    Retangulo::novo(LARGURA - 72, 5, 66, ALTURA_FERRAMENTAS - 10)
}

impl Pintura {
    pub fn nova() -> Pintura {
        let quadro = Superficie::nova(LARGURA as usize, (ALTURA - ALTURA_FERRAMENTAS) as usize, FUNDO_QUADRO);
        Pintura { quadro, cor: 0, pincel: 1, tinta: None, ultimo: (0, 0) }
    }

    /// Clique na barra de ferramentas. true = mudou alguma coisa.
    fn ferramenta(&mut self, x: i32, y: i32) -> bool {
        if let Some(i) = (0..CORES.len()).find(|&i| retangulo_cor(i).contem(x, y)) {
            self.cor = i;
        } else if let Some(i) = (0..PINCEIS.len()).find(|&i| retangulo_pincel(i).contem(x, y)) {
            self.pincel = i;
        } else if retangulo_limpar().contem(x, y) {
            self.quadro.limpar(FUNDO_QUADRO);
        } else {
            return false;
        }
        true
    }

    /// Um ponto do pincel atual.
    fn carimbo(&mut self, x: i32, y: i32, cor: Cor) {
        match PINCEIS[self.pincel] {
            0 => self.quadro.pixel(x, y, cor),
            raio => self.quadro.circulo_cheio(x, y, raio, cor),
        }
    }

    /// Traco do ultimo ponto ate (x, y): linha fina, ou carimbos seguidos
    /// para os pinceis grossos.
    fn tracar(&mut self, x: i32, y: i32, cor: Cor) {
        let (x0, y0) = self.ultimo;
        if PINCEIS[self.pincel] == 0 {
            self.quadro.linha(x0, y0, x, y, cor);
        } else {
            let passos = (x - x0).abs().max((y - y0).abs()).max(1);
            for i in 1..=passos {
                self.carimbo(x0 + (x - x0) * i / passos, y0 + (y - y0) * i / passos, cor);
            }
        }
        self.ultimo = (x, y);
    }

    fn apertar(&mut self, x: i32, y: i32, botao: Botao) -> bool {
        if y < ALTURA_FERRAMENTAS {
            return botao == Botao::Esquerdo && self.ferramenta(x, y);
        }
        let tinta = match botao {
            Botao::Esquerdo => CORES[self.cor],
            Botao::Direito => FUNDO_QUADRO,
            Botao::Meio => return false,
        };
        let (x, y) = (x, y - ALTURA_FERRAMENTAS);
        self.tinta = Some(tinta);
        self.ultimo = (x, y);
        self.carimbo(x, y, tinta);
        true
    }

    fn mover(&mut self, x: i32, y: i32, botoes: u8) -> bool {
        let Some(tinta) = self.tinta else { return false };
        if botoes == 0 {
            self.tinta = None; // soltou fora da janela
            return false;
        }
        self.tracar(x, y - ALTURA_FERRAMENTAS, tinta);
        true
    }

    fn desenhar_ferramentas(&self, s: &mut Superficie, t: &Tema) {
        s.retangulo(Retangulo::novo(0, 0, LARGURA, ALTURA_FERRAMENTAS), t.fundo_janela);
        s.retangulo(Retangulo::novo(0, ALTURA_FERRAMENTAS - 1, LARGURA, 1), t.borda);
        for (i, &cor) in CORES.iter().enumerate() {
            let r = retangulo_cor(i);
            s.retangulo(r, cor);
            let escolhida = i == self.cor;
            s.contorno(r, if escolhida { t.destaque } else { t.borda }, if escolhida { 3 } else { 1 });
        }
        for (i, &raio) in PINCEIS.iter().enumerate() {
            let r = retangulo_pincel(i);
            let fundo = if i == self.pincel { t.botao_destaque } else { t.botao };
            s.retangulo(r, fundo);
            s.contorno(r, if i == self.pincel { t.destaque } else { t.borda }, 1);
            s.circulo_cheio(r.x + r.largura / 2, r.y + r.altura / 2, raio.max(1), t.botao_texto);
        }
        area::botao(s, retangulo_limpar(), "Limpar", t.botao, t.botao_texto, t.borda, 1);
    }
}

impl App for Pintura {
    fn titulo(&self) -> &str {
        "Pintura"
    }

    fn tamanho(&self) -> (usize, usize) {
        (LARGURA as usize, ALTURA as usize)
    }

    fn desenhar(&mut self, s: &mut Superficie) {
        let t = tema::atual();
        self.quadro.copiar_para(s, 0, ALTURA_FERRAMENTAS);
        self.desenhar_ferramentas(s, &t);
    }

    fn evento(&mut self, e: EventoApp) -> bool {
        match e {
            EventoApp::Botao { x, y, botao, pressionado: true } => self.apertar(x, y, botao),
            EventoApp::Botao { pressionado: false, .. } => {
                self.tinta = None;
                false
            }
            EventoApp::Mouse { x, y, botoes } => self.mover(x, y, botoes),
            EventoApp::Caractere('l' | 'L') => {
                self.quadro.limpar(FUNDO_QUADRO);
                true
            }
            _ => false,
        }
    }
}
