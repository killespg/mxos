//! Calculadora de inteiros (i64), com mouse e teclado.
//!
//! Teclado: 0-9, + - * / % (e tambem x, × e ÷), = ou Enter, Backspace
//! apaga um digito, Esc ou C limpa tudo. Conta que estoura ou divide por
//! zero mostra o erro no visor ate a proxima tecla.

use alloc::format;
use alloc::string::String;

use crate::fonte::{self, Fonte, INTER, INTER_GRANDE, INTER_TITULO};
use crate::gui::area;
use crate::gui::grafico::{self, Cor, Retangulo, Superficie};
use crate::gui::janelas::{App, EventoApp};
use crate::gui::tema::{self, Tema};
use crate::entrada::Botao;

const COLUNAS: usize = 4;
const LINHAS: usize = 5;
const MARGEM: i32 = 8;
/// Conta pendente em cima (Inter 13) e o numero embaixo (Inter 32).
const ALTURA_VISOR: i32 = 64;
const LARGURA_TECLA: i32 = 56;
const ALTURA_TECLA: i32 = 38;
const ESPACO: i32 = 6;
const LARGURA: i32 = 2 * MARGEM + COLUNAS as i32 * LARGURA_TECLA + (COLUNAS as i32 - 1) * ESPACO;
const TOPO_TECLADO: i32 = MARGEM + ALTURA_VISOR + 10;
const ALTURA: i32 = TOPO_TECLADO + LINHAS as i32 * ALTURA_TECLA + (LINHAS as i32 - 1) * ESPACO + MARGEM;
/// Tiques que uma tecla do teclado fisico fica "apertada" na tela.
const TIQUES_APERTADA: u8 = 2;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Operacao {
    Somar,
    Subtrair,
    Multiplicar,
    Dividir,
    Resto,
}

impl Operacao {
    fn simbolo(self) -> &'static str {
        match self {
            Operacao::Somar => "+",
            Operacao::Subtrair => "–",
            Operacao::Multiplicar => "×",
            Operacao::Dividir => "÷",
            Operacao::Resto => "%",
        }
    }

    fn aplicar(self, a: i64, b: i64) -> Result<i64, &'static str> {
        let resultado = match self {
            Operacao::Somar => a.checked_add(b),
            Operacao::Subtrair => a.checked_sub(b),
            Operacao::Multiplicar => a.checked_mul(b),
            Operacao::Dividir | Operacao::Resto if b == 0 => return Err("Divisão por zero"),
            Operacao::Dividir => a.checked_div(b),
            Operacao::Resto => a.checked_rem(b),
        };
        resultado.ok_or("Número grande demais")
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tecla {
    Digito(u8),
    Op(Operacao),
    Igual,
    Limpar,
    LimparEntrada,
    Apagar,
    TrocarSinal,
}

/// Teclado na tela, linha a linha.
const TECLAS: [(&str, Tecla); COLUNAS * LINHAS] = [
    ("C", Tecla::Limpar),
    ("CE", Tecla::LimparEntrada),
    ("%", Tecla::Op(Operacao::Resto)),
    ("÷", Tecla::Op(Operacao::Dividir)),
    ("7", Tecla::Digito(7)),
    ("8", Tecla::Digito(8)),
    ("9", Tecla::Digito(9)),
    ("×", Tecla::Op(Operacao::Multiplicar)),
    ("4", Tecla::Digito(4)),
    ("5", Tecla::Digito(5)),
    ("6", Tecla::Digito(6)),
    ("–", Tecla::Op(Operacao::Subtrair)),
    ("1", Tecla::Digito(1)),
    ("2", Tecla::Digito(2)),
    ("3", Tecla::Digito(3)),
    ("+", Tecla::Op(Operacao::Somar)),
    ("±", Tecla::TrocarSinal),
    ("0", Tecla::Digito(0)),
    ("←", Tecla::Apagar),
    ("=", Tecla::Igual),
];

pub struct Calculadora {
    /// Numero no visor (o que esta sendo digitado ou o ultimo resultado).
    valor: i64,
    /// Primeiro operando, guardado quando aperta uma operacao.
    guardado: i64,
    pendente: Option<Operacao>,
    /// O proximo digito continua o numero do visor (false = comeca outro).
    digitando: bool,
    erro: Option<&'static str>,
    /// Tecla desenhada como apertada e por quantos tiques (0 = ate soltar o mouse).
    apertada: Option<usize>,
    tiques_apertada: u8,
}

/// Retangulo da tecla `i` (indice em TECLAS).
fn retangulo_tecla(i: usize) -> Retangulo {
    let (coluna, linha) = ((i % COLUNAS) as i32, (i / COLUNAS) as i32);
    let x = MARGEM + coluna * (LARGURA_TECLA + ESPACO);
    let y = TOPO_TECLADO + linha * (ALTURA_TECLA + ESPACO);
    Retangulo::novo(x, y, LARGURA_TECLA, ALTURA_TECLA)
}

fn tecla_em(x: i32, y: i32) -> Option<usize> {
    (0..TECLAS.len()).find(|&i| retangulo_tecla(i).contem(x, y))
}

/// Tecla da calculadora para um caractere do teclado.
fn tecla_do_caractere(c: char) -> Option<Tecla> {
    Some(match c {
        '0'..='9' => Tecla::Digito(c as u8 - b'0'),
        '+' => Tecla::Op(Operacao::Somar),
        '-' | '–' => Tecla::Op(Operacao::Subtrair),
        '*' | 'x' | 'X' | '×' => Tecla::Op(Operacao::Multiplicar),
        '/' | '÷' => Tecla::Op(Operacao::Dividir),
        '%' => Tecla::Op(Operacao::Resto),
        '=' | '\r' | '\n' => Tecla::Igual,
        '\x08' | '\x7f' => Tecla::Apagar,
        '\x1b' | 'c' | 'C' => Tecla::Limpar,
        _ => return None,
    })
}

/// Cores de fundo e do texto de cada tipo de tecla.
fn cores_tecla(tecla: Tecla, t: &Tema) -> (Cor, Cor) {
    match tecla {
        Tecla::Igual => (t.destaque, 0x00FF_FFFF),
        Tecla::Limpar => (t.fechar, 0x00FF_FFFF),
        Tecla::Digito(_) => (t.botao, t.botao_texto),
        _ => (t.botao_destaque, t.botao_texto),
    }
}

impl Calculadora {
    pub fn nova() -> Calculadora {
        Calculadora {
            valor: 0,
            guardado: 0,
            pendente: None,
            digitando: false,
            erro: None,
            apertada: None,
            tiques_apertada: 0,
        }
    }

    fn limpar(&mut self) {
        *self = Calculadora { apertada: self.apertada, tiques_apertada: self.tiques_apertada, ..Calculadora::nova() };
    }

    fn digito(&mut self, d: u8) {
        if !self.digitando {
            self.valor = 0;
            self.digitando = true;
        }
        // negativo cresce para baixo; se estourar o digito e ignorado
        let d = d as i64;
        let novo = self.valor.checked_mul(10).and_then(|v| if v < 0 { v.checked_sub(d) } else { v.checked_add(d) });
        if let Some(novo) = novo {
            self.valor = novo;
        }
    }

    /// Faz a conta pendente com o numero do visor.
    fn resolver(&mut self) {
        if let Some(op) = self.pendente.take() {
            match op.aplicar(self.guardado, self.valor) {
                Ok(v) => self.valor = v,
                Err(e) => self.erro = Some(e),
            }
        }
        self.digitando = false;
    }

    fn operacao(&mut self, op: Operacao) {
        // 2 + 3 + ... : faz o 2 + 3 antes de guardar o proximo
        if self.pendente.is_some() && self.digitando {
            self.resolver();
            if self.erro.is_some() {
                return;
            }
        }
        self.guardado = self.valor;
        self.pendente = Some(op);
        self.digitando = false;
    }

    fn apertar(&mut self, tecla: Tecla) {
        if self.erro.is_some() {
            self.limpar(); // qualquer tecla tira o erro do visor
            if matches!(tecla, Tecla::Limpar | Tecla::LimparEntrada | Tecla::Apagar) {
                return;
            }
        }
        match tecla {
            Tecla::Digito(d) => self.digito(d),
            Tecla::Op(op) => self.operacao(op),
            Tecla::Igual => self.resolver(),
            Tecla::Limpar => self.limpar(),
            Tecla::LimparEntrada => {
                self.valor = 0;
                self.digitando = true;
            }
            Tecla::Apagar if self.digitando => self.valor /= 10,
            Tecla::Apagar => {}
            Tecla::TrocarSinal => match self.valor.checked_neg() {
                Some(v) => self.valor = v,
                None => self.erro = Some("Número grande demais"),
            },
        }
    }

    /// Texto grande do visor.
    fn texto_visor(&self) -> String {
        match self.erro {
            Some(e) => String::from(e),
            None => format!("{}", self.valor),
        }
    }

    fn desenhar_visor(&self, s: &mut Superficie, t: &Tema) {
        let r = Retangulo::novo(MARGEM, MARGEM, LARGURA - 2 * MARGEM, ALTURA_VISOR);
        s.retangulo(r, area::escurecer(t.fundo_janela, 14));
        s.contorno(r, t.borda, 1);
        if let Some(op) = self.pendente {
            let conta = format!("{} {}", self.guardado, op.simbolo());
            let conta = fonte::cortar(&conta, r.largura - 16, &INTER);
            let fraco = grafico::misturar(t.texto, t.fundo_janela, 100);
            s.texto_com(r.direita() - 8 - INTER.largura(conta), r.y + 4 + INTER.ascendente, conta, fraco, &INTER);
        }
        let texto = self.texto_visor();
        // a maior fonte em que o numero (ou o erro) cabe
        let cabe = |f: &Fonte| f.largura(&texto) <= r.largura - 16;
        let f = [&INTER_GRANDE, &INTER_TITULO].into_iter().find(|f| cabe(f)).unwrap_or(&INTER);
        let largura = f.largura(&texto);
        s.texto_com(r.direita() - 8 - largura, r.baixo() - 10, &texto, t.texto, f);
    }

    fn desenhar_teclas(&self, s: &mut Superficie, t: &Tema) {
        for (i, &(nome, tecla)) in TECLAS.iter().enumerate() {
            let r = retangulo_tecla(i);
            let (mut fundo, frente) = cores_tecla(tecla, t);
            if self.apertada == Some(i) {
                fundo = area::escurecer(fundo, 50);
            }
            area::botao(s, r, nome, fundo, frente, t.borda, 2);
        }
    }

    fn apertar_indice(&mut self, i: usize, tiques: u8) {
        self.apertada = Some(i);
        self.tiques_apertada = tiques;
        self.apertar(TECLAS[i].1);
    }

    fn caractere(&mut self, c: char) -> bool {
        // toda tecla do teclado fisico tem uma igual na tela, que pisca
        let Some(tecla) = tecla_do_caractere(c) else { return false };
        let Some(i) = TECLAS.iter().position(|&(_, t)| t == tecla) else { return false };
        self.apertar_indice(i, TIQUES_APERTADA);
        true
    }

    /// Solta a tecla apertada pelo teclado fisico depois de uns tiques.
    fn tique(&mut self) -> bool {
        if self.tiques_apertada == 0 {
            return false;
        }
        self.tiques_apertada -= 1;
        if self.tiques_apertada > 0 {
            return false;
        }
        self.apertada = None;
        true
    }
}

impl App for Calculadora {
    fn titulo(&self) -> &str {
        "Calculadora"
    }

    fn tamanho(&self) -> (usize, usize) {
        (LARGURA as usize, ALTURA as usize)
    }

    fn desenhar(&mut self, s: &mut Superficie) {
        let t = tema::atual();
        s.limpar(t.fundo_janela);
        self.desenhar_visor(s, &t);
        self.desenhar_teclas(s, &t);
    }

    fn evento(&mut self, e: EventoApp) -> bool {
        match e {
            EventoApp::Caractere(c) => self.caractere(c),
            EventoApp::Botao { x, y, botao: Botao::Esquerdo, pressionado: true } => match tecla_em(x, y) {
                Some(i) => {
                    self.apertar_indice(i, 0);
                    true
                }
                None => false,
            },
            EventoApp::Botao { botao: Botao::Esquerdo, pressionado: false, .. } => {
                let estava = self.apertada.is_some() && self.tiques_apertada == 0;
                if estava {
                    self.apertada = None;
                }
                estava
            }
            EventoApp::Tique => self.tique(),
            _ => false,
        }
    }
}
