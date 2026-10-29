//! Gerador de som so com inteiros (sem ponto flutuante, sem libm).
//!
//! Um oscilador guarda a "fase" num u32: 0 e o comeco da onda e 2^32 e uma
//! volta inteira. A cada amostra a fase anda `passo` = freq * 2^32 / taxa,
//! e o u32 da a volta sozinho (wrapping), exatamente como uma onda periodica.
//!
//! O seno vem de uma tabela de um quarto de onda (65 valores) com
//! interpolacao linear; os outros tres quartos sao espelhos dela.
//! Cada nota ganha uma rampa curta de entrada e de saida para nao estalar.

/// Quem produz amostras para um driver de som (virtio-sound hoje, HDMI depois).
pub trait Fonte {
    /// Escreve um quadro (uma amostra S16 por canal) em `quadro`.
    /// Devolve false quando o som acabou (o quadro e ignorado).
    fn proximo_quadro(&mut self, quadro: &mut [i16]) -> bool;
}

/// Seno de 0 a 90 graus: TABELA_SENO[i] = round(32767 * sin(i * pi / 128)).
pub const TABELA_SENO: [i16; 65] = [
    0, 804, 1608, 2410, 3212, 4011, 4808, 5602,
    6393, 7179, 7962, 8739, 9512, 10278, 11039, 11793,
    12539, 13279, 14010, 14732, 15446, 16151, 16846, 17530,
    18204, 18868, 19519, 20159, 20787, 21403, 22005, 22594,
    23170, 23731, 24279, 24811, 25329, 25832, 26319, 26790,
    27245, 27683, 28105, 28510, 28898, 29268, 29621, 29956,
    30273, 30571, 30852, 31113, 31356, 31580, 31785, 31971,
    32137, 32285, 32412, 32521, 32609, 32678, 32728, 32757,
    32767,
];

/// Maior valor de uma onda (o simetrico negativo e o menor).
pub const PICO: i32 = 32767;

/// Duracao das rampas de entrada e de saida de cada nota.
pub const RAMPA_MS: u32 = 5;

/// Ganho maximo da rampa (1.0 em ponto fixo Q15).
const GANHO_CHEIO: i32 = 1 << 15;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Forma {
    Seno,
    Quadrada,
    Triangulo,
    DenteDeSerra,
}

/// Uma nota: frequencia em Hz (0 = pausa) e duracao em milissegundos.
#[derive(Clone, Copy)]
pub struct Nota {
    pub freq_hz: u32,
    pub duracao_ms: u32,
}

/// Quanto a fase anda por amostra. Limita a frequencia a metade da taxa
/// (acima disso a onda nao pode ser representada).
pub fn passo_de(freq_hz: u32, taxa_hz: u32) -> u32 {
    if taxa_hz == 0 {
        return 0;
    }
    let freq = freq_hz.min(taxa_hz / 2) as u64;
    ((freq << 32) / taxa_hz as u64) as u32
}

/// Quarto de seno interpolado. `posicao` vai de 0 (0 graus) a 65536 (90 graus).
fn quarto_de_seno(posicao: u32) -> i32 {
    let i = (posicao >> 10) as usize;
    if i >= 64 {
        return TABELA_SENO[64] as i32;
    }
    let fracao = (posicao & 0x3FF) as i32;
    let a = TABELA_SENO[i] as i32;
    let b = TABELA_SENO[i + 1] as i32;
    a + (((b - a) * fracao) >> 10)
}

/// Seno da fase (u32 = uma volta), de -PICO a PICO.
fn seno(fase: u32) -> i32 {
    let quadrante = fase >> 30;
    let posicao = (fase >> 14) & 0xFFFF;
    match quadrante {
        0 => quarto_de_seno(posicao),
        1 => quarto_de_seno(65536 - posicao),
        2 => -quarto_de_seno(posicao),
        _ => -quarto_de_seno(65536 - posicao),
    }
}

/// Triangulo com o mesmo desenho do seno: sobe de 0 a PICO no primeiro quarto.
fn triangulo(fase: u32) -> i32 {
    let posicao = ((fase >> 15) & 0x7FFF) as i32;
    match fase >> 30 {
        0 => posicao,
        1 => PICO - posicao,
        2 => -posicao,
        _ => -(PICO - posicao),
    }
}

/// Valor da onda na fase dada, de -PICO a PICO.
pub fn onda(forma: Forma, fase: u32) -> i32 {
    match forma {
        Forma::Seno => seno(fase),
        Forma::Triangulo => triangulo(fase),
        Forma::Quadrada => {
            if fase < 0x8000_0000 { PICO } else { -PICO }
        }
        Forma::DenteDeSerra => ((fase >> 16) as i32 - 32768).max(-PICO),
    }
}

/// Oscilador de frequencia fixa.
pub struct Oscilador {
    fase: u32,
    passo: u32,
    forma: Forma,
    /// 0 a 32767 (32767 = volume maximo).
    volume: i32,
}

impl Oscilador {
    pub fn novo(forma: Forma, freq_hz: u32, taxa_hz: u32, volume: u16) -> Oscilador {
        Oscilador {
            fase: 0,
            passo: passo_de(freq_hz, taxa_hz),
            forma,
            volume: (volume as i32).min(PICO),
        }
    }

    /// Proxima amostra; a fase anda um passo.
    pub fn amostra(&mut self) -> i16 {
        let valor = (onda(self.forma, self.fase) * self.volume) >> 15;
        self.fase = self.fase.wrapping_add(self.passo);
        valor as i16
    }
}

/// Ganho Q15 (0 a 32768) da rampa no quadro `k` de uma nota com `total` quadros.
pub fn ganho_rampa(k: u32, total: u32, rampa: u32) -> i32 {
    if rampa == 0 {
        return GANHO_CHEIO;
    }
    let mut ganho = GANHO_CHEIO;
    if k < rampa {
        ganho = ((k as u64 * GANHO_CHEIO as u64) / rampa as u64) as i32;
    }
    let resto = total - k;
    if resto < rampa {
        let saida = ((resto as u64 * GANHO_CHEIO as u64) / rampa as u64) as i32;
        ganho = ganho.min(saida);
    }
    ganho
}

/// Quantos quadros dura `ms` milissegundos na taxa dada.
pub fn quadros_de_ms(ms: u32, taxa_hz: u32) -> u32 {
    ((ms as u64 * taxa_hz as u64) / 1000) as u32
}

/// Toca uma lista de notas em sequencia, a mesma amostra em todos os canais.
pub struct Melodia<'a> {
    notas: &'a [Nota],
    taxa_hz: u32,
    forma: Forma,
    volume: u16,
    rampa: u32,
    /// Nota atual, quadro dentro dela e total de quadros dela.
    atual: usize,
    quadro: u32,
    total: u32,
    oscilador: Oscilador,
}

impl<'a> Melodia<'a> {
    pub fn nova(notas: &'a [Nota], taxa_hz: u32, forma: Forma, volume: u16) -> Melodia<'a> {
        let mut m = Melodia {
            notas,
            taxa_hz,
            forma,
            volume,
            rampa: quadros_de_ms(RAMPA_MS, taxa_hz),
            atual: 0,
            quadro: 0,
            total: 0,
            oscilador: Oscilador::novo(forma, 0, taxa_hz, 0),
        };
        m.carregar_nota(0);
        m
    }

    /// Duracao total em quadros (soma das notas).
    pub fn total_quadros(&self) -> u64 {
        self.notas.iter().map(|n| quadros_de_ms(n.duracao_ms, self.taxa_hz) as u64).sum()
    }

    /// Prepara a nota `i`: oscilador novo com fase zero.
    fn carregar_nota(&mut self, i: usize) {
        self.atual = i;
        self.quadro = 0;
        let Some(nota) = self.notas.get(i) else { return };
        self.total = quadros_de_ms(nota.duracao_ms, self.taxa_hz);
        self.oscilador = Oscilador::novo(self.forma, nota.freq_hz, self.taxa_hz, self.volume);
    }

    /// Pula notas que ja acabaram (ou tem duracao zero). False = melodia acabou.
    fn avancar_se_preciso(&mut self) -> bool {
        while self.atual < self.notas.len() && self.quadro >= self.total {
            self.carregar_nota(self.atual + 1);
        }
        self.atual < self.notas.len()
    }

    /// Amostra da nota atual com a rampa aplicada (pausa = silencio).
    fn amostra(&mut self) -> i16 {
        let pausa = self.notas[self.atual].freq_hz == 0;
        let bruta = self.oscilador.amostra() as i32;
        let ganho = ganho_rampa(self.quadro, self.total, self.rampa);
        self.quadro += 1;
        if pausa { 0 } else { ((bruta * ganho) >> 15) as i16 }
    }
}

impl Fonte for Melodia<'_> {
    fn proximo_quadro(&mut self, quadro: &mut [i16]) -> bool {
        if !self.avancar_se_preciso() {
            return false;
        }
        let valor = self.amostra();
        for canal in quadro.iter_mut() {
            *canal = valor;
        }
        true
    }
}
