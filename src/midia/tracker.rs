//! Musicas MOD (ProTracker e parentes: M.K., FLT4, 6CHN, 8CHN, xxCH):
//! 31 amostras de 8 bits, padroes de 64 linhas, e os efeitos do
//! ProTracker (arpejo, portamentos, vibrato, tremolo, slides de volume,
//! saltos, quebras, retrigger, corte e atraso de nota, loop de padrao).
//! Mistura em inteiros na taxa da placa, com separacao de estereo como
//! no Amiga (canais 0 e 3 a esquerda, 1 e 2 a direita).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::{Audio, Fonte};

/// Relogio do Paula (PAL) / 2.
const RELOGIO: u64 = 3_546_895;
const SEPARACAO: i32 = 192; // de 256: 75% do lado do canal

/// Periodos de C-1 a B-3 (finetune 0).
const PERIODOS: [u16; 36] = [
    856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226,
    214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113,
];

/// 2^(-finetune/96) em 16.16, finetune de 0..7 e depois -8..-1.
const FINETUNE: [u32; 16] = [65536, 65065, 64596, 64131, 63670, 63212, 62757, 62305, 69433, 68933, 68437, 67945, 67456, 66971, 66489, 66010];

const SENO: [u8; 32] = [
    0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244, 250, 253, 255, 253, 250, 244, 235, 224, 212, 197, 180, 161,
    141, 120, 97, 74, 49, 24,
];

#[derive(Clone, Default)]
struct Amostra {
    dados: Vec<i8>,
    finetune: u8,
    volume: u8,
    repete: usize,
    tam_repete: usize,
}

#[derive(Clone, Copy, Default)]
struct Celula {
    amostra: u8,
    periodo: u16,
    efeito: u8,
    param: u8,
}

#[derive(Clone, Default)]
struct Canal {
    amostra: usize,
    /// Posicao na amostra (32.32? nao: 16.16 em u64).
    pos: u64,
    passo: u64,
    tocando: bool,
    periodo: u16,
    periodo_base: u16,
    volume: i32,
    alvo_porta: u16,
    vel_porta: u8,
    vib_vel: u8,
    vib_prof: u8,
    vib_pos: u8,
    trem_vel: u8,
    trem_prof: u8,
    trem_pos: u8,
    trem_volume: i32,

    loop_linha: usize,
    loop_conta: u8,
    deslocamento: u8,
    /// Nota atrasada (EDx): celula guardada.
    atrasada: Option<Celula>,
    pan: i32,
}

pub struct Tracker {
    titulo: String,
    canais_n: usize,
    amostras: Vec<Amostra>,
    ordem: Vec<u8>,
    padroes: Vec<Vec<Celula>>,
    taxa: u32,
    canais: Vec<Canal>,
    // posicao na musica
    pos_ordem: usize,
    linha: usize,
    tique: u32,
    velocidade: u32,
    bpm: u32,
    /// Amostras de saida ate o proximo tique.
    resta_tique: u32,
    atraso_padrao: u32,
    pular_para: Option<(usize, usize)>,
    vistos: Vec<bool>,
    acabou: bool,
    quadros: u64,
}

fn tag_canais(tag: &[u8]) -> Option<usize> {
    match tag {
        b"M.K." | b"M!K!" | b"FLT4" | b"4CHN" | b"N.T." => Some(4),
        b"6CHN" => Some(6),
        b"8CHN" | b"CD81" | b"OKTA" | b"FLT8" => Some(8),
        [a, b, b'C', b'H'] if a.is_ascii_digit() && b.is_ascii_digit() => Some(((a - b'0') * 10 + (b - b'0')) as usize),
        [a, b'C', b'H', b'N'] if a.is_ascii_digit() => Some((a - b'0') as usize),
        _ => None,
    }
}

/// A assinatura de MOD no byte 1080.
pub fn parece_mod(f: &mut dyn Fonte) -> bool {
    let mut t = [0u8; 4];
    f.ler(1080, &mut t) == 4 && tag_canais(&t).is_some()
}

impl Tracker {
    pub fn novo(f: &mut dyn Fonte, taxa: u32) -> Result<Tracker, &'static str> {
        let n = f.tamanho();
        if n < 1084 || n > 16 << 20 {
            return Err("MOD inválido");
        }
        let mut d = alloc::vec![0u8; n];
        f.ler(0, &mut d);
        let canais_n = tag_canais(&d[1080..1084]).unwrap_or(4).clamp(1, 32);
        let titulo: String = d[..20].iter().take_while(|&&b| b != 0).map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { ' ' }).collect();
        let mut amostras = Vec::with_capacity(31);
        let mut info = Vec::new();
        for i in 0..31 {
            let b = &d[20 + i * 30..20 + (i + 1) * 30];
            let tam = u16::from_be_bytes([b[22], b[23]]) as usize * 2;
            let rep = u16::from_be_bytes([b[26], b[27]]) as usize * 2;
            let tam_rep = u16::from_be_bytes([b[28], b[29]]) as usize * 2;
            info.push((tam, b[24] & 0x0F, b[25].min(64), rep, tam_rep));
        }
        let comprimento = (d[950] as usize).clamp(1, 128);
        let ordem: Vec<u8> = d[952..952 + comprimento].to_vec();
        let n_padroes = d[952..1080].iter().copied().max().unwrap_or(0) as usize + 1;
        let tam_padrao = 64 * canais_n * 4;
        let mut pos = 1084;
        let mut padroes = Vec::with_capacity(n_padroes);
        for _ in 0..n_padroes {
            let mut p = Vec::with_capacity(64 * canais_n);
            for k in 0..64 * canais_n {
                let c = d.get(pos + k * 4..pos + k * 4 + 4).unwrap_or(&[0, 0, 0, 0]);
                p.push(Celula {
                    amostra: (c[0] & 0xF0) | (c[2] >> 4),
                    periodo: (((c[0] & 0x0F) as u16) << 8) | c[1] as u16,
                    efeito: c[2] & 0x0F,
                    param: c[3],
                });
            }
            padroes.push(p);
            pos += tam_padrao;
        }
        for (tam, ft, vol, rep, tam_rep) in info {
            let fim = (pos + tam).min(n);
            let dados: Vec<i8> = d.get(pos..fim).unwrap_or(&[]).iter().map(|&b| b as i8).collect();
            pos += tam;
            let (rep, tam_rep) = if tam_rep > 2 && rep < dados.len() { (rep, tam_rep.min(dados.len() - rep)) } else { (0, 0) };
            amostras.push(Amostra { dados, finetune: ft, volume: vol, repete: rep, tam_repete: tam_rep });
        }
        let mut canais = alloc::vec![Canal::default(); canais_n];
        for (i, c) in canais.iter_mut().enumerate() {
            // Amiga: L R R L
            c.pan = if matches!(i % 4, 0 | 3) { -1 } else { 1 };
        }
        let comprimento = ordem.len();
        let mut t = Tracker {
            titulo,
            canais_n,
            amostras,
            ordem,
            padroes,
            taxa,
            canais,
            pos_ordem: 0,
            linha: 0,
            tique: 0,
            velocidade: 6,
            bpm: 125,
            resta_tique: 0,
            atraso_padrao: 0,
            pular_para: None,
            vistos: alloc::vec![false; comprimento * 64],
            acabou: false,
            quadros: 0,
        };
        t.tocar_linha();
        t.resta_tique = t.amostras_por_tique();
        Ok(t)
    }

    fn amostras_por_tique(&self) -> u32 {
        self.taxa * 5 / (self.bpm.max(32) * 2)
    }

    fn periodo_com_finetune(periodo: u16, ft: u8) -> u16 {
        // acha a nota mais perto e aplica o finetune
        let i = PERIODOS.iter().enumerate().min_by_key(|(_, p)| (**p as i32 - periodo as i32).abs()).map_or(0, |(i, _)| i);
        ((PERIODOS[i] as u64 * FINETUNE[ft as usize & 15] as u64) >> 16) as u16
    }

    fn atualizar_passo(&mut self, c: usize, periodo: u16) {
        let p = periodo.clamp(28, 6848) as u64;
        self.canais[c].passo = (RELOGIO << 16) / (p * self.taxa as u64);
    }

    fn disparar(&mut self, c: usize, cel: Celula) {
        let ch = &mut self.canais[c];
        if cel.amostra > 0 && (cel.amostra as usize) <= self.amostras.len() {
            ch.amostra = cel.amostra as usize - 1;
            ch.volume = self.amostras[ch.amostra].volume as i32;
        }
        if cel.periodo > 0 {
            let ft = self.amostras.get(ch.amostra).map_or(0, |a| a.finetune);
            let p = Self::periodo_com_finetune(cel.periodo, ft);
            if cel.efeito == 3 || cel.efeito == 5 {
                ch.alvo_porta = p;
            } else {
                ch.periodo = p;
                ch.periodo_base = p;
                ch.pos = if cel.efeito == 9 { ((if cel.param > 0 { cel.param } else { ch.deslocamento }) as u64 * 256) << 16 } else { 0 };
                ch.tocando = true;
                if ch.vib_pos & 0x80 == 0 {
                    ch.vib_pos = 0;
                }
                let p = ch.periodo;
                self.atualizar_passo(c, p);
            }
        }
    }

    /// Tique 0 de uma linha: notas e efeitos de linha.
    fn tocar_linha(&mut self) {
        if self.pos_ordem >= self.ordem.len() {
            self.acabou = true;
            return;
        }
        let marca = self.pos_ordem * 64 + self.linha;
        if self.vistos.get(marca).copied().unwrap_or(false) {
            // a musica voltou para um lugar ja tocado: acabou (sem repetir)
            self.acabou = true;
            return;
        }
        if let Some(v) = self.vistos.get_mut(marca) {
            *v = true;
        }
        let padrao = self.ordem[self.pos_ordem] as usize;
        for c in 0..self.canais_n {
            let cel = self.padroes.get(padrao).and_then(|p| p.get(self.linha * self.canais_n + c)).copied().unwrap_or_default();
            let (ef, pa) = (cel.efeito, cel.param);
            let atraso = ef == 0xE && pa >> 4 == 0xD && pa & 0xF > 0;
            if atraso {
                self.canais[c].atrasada = Some(cel);
            } else {
                self.disparar(c, cel);
            }
            let ch = &mut self.canais[c];
            ch.trem_volume = ch.volume;
            match ef {
                0x3 | 0x5 if ef == 0x3 && pa > 0 => ch.vel_porta = pa,
                0x4 => {
                    if pa >> 4 > 0 {
                        ch.vib_vel = pa >> 4;
                    }
                    if pa & 0xF > 0 {
                        ch.vib_prof = pa & 0xF;
                    }
                }
                0x7 => {
                    if pa >> 4 > 0 {
                        ch.trem_vel = pa >> 4;
                    }
                    if pa & 0xF > 0 {
                        ch.trem_prof = pa & 0xF;
                    }
                }
                0x8 => ch.pan = (pa as i32 - 128).signum(),
                0x9 if pa > 0 => ch.deslocamento = pa,
                0xB => {
                    self.pular_para = Some((pa as usize, 0));
                }
                0xC => ch.volume = pa.min(64) as i32,
                0xD => {
                    let linha = ((pa >> 4) * 10 + (pa & 0xF)) as usize;
                    let ordem = self.pular_para.map_or(self.pos_ordem + 1, |(o, _)| o);
                    self.pular_para = Some((ordem, linha.min(63)));
                }
                0xE => {
                    let (x, y) = (pa >> 4, pa & 0xF);
                    match x {
                        0x1 => {
                            ch.periodo = ch.periodo.saturating_sub(y as u16).max(113);
                            let p = ch.periodo;
                            self.atualizar_passo(c, p);
                        }
                        0x2 => {
                            ch.periodo = (ch.periodo + y as u16).min(856);
                            let p = ch.periodo;
                            self.atualizar_passo(c, p);
                        }
                        0x6 => {
                            if y == 0 {
                                ch.loop_linha = self.linha;
                            } else if ch.loop_conta == 0 {
                                ch.loop_conta = y;
                                let l = ch.loop_linha;
                                self.pular_para = Some((self.pos_ordem, l));
                                self.esquecer_vistos(self.pos_ordem);
                            } else {
                                ch.loop_conta -= 1;
                                if ch.loop_conta > 0 {
                                    let l = ch.loop_linha;
                                    self.pular_para = Some((self.pos_ordem, l));
                                    self.esquecer_vistos(self.pos_ordem);
                                }
                            }
                        }
                        0xA => ch.volume = (ch.volume + y as i32).min(64),
                        0xB => ch.volume = (ch.volume - y as i32).max(0),
                        0xE => self.atraso_padrao = y as u32,
                        _ => {}
                    }
                }
                0xF => {
                    if pa == 0 {
                    } else if pa < 32 {
                        self.velocidade = pa as u32;
                    } else {
                        self.bpm = pa as u32;
                    }
                }
                _ => {}
            }
        }
    }

    fn esquecer_vistos(&mut self, ordem: usize) {
        for v in self.vistos.iter_mut().skip(ordem * 64).take(64) {
            *v = false;
        }
    }

    /// Tiques 1..: efeitos que andam.
    fn tocar_tique(&mut self) {
        if self.pos_ordem >= self.ordem.len() {
            return;
        }
        let padrao = self.ordem[self.pos_ordem] as usize;
        for c in 0..self.canais_n {
            let cel = self.padroes.get(padrao).and_then(|p| p.get(self.linha * self.canais_n + c)).copied().unwrap_or_default();
            let (ef, pa) = (cel.efeito, cel.param);
            let t = self.tique;
            let mut atrasada = None;
            let ch = &mut self.canais[c];
            let mut periodo = ch.periodo;
            let slide = |ch: &mut Canal, pa: u8| {
                let (x, y) = (pa >> 4, pa & 0xF);
                ch.volume = if x > 0 { (ch.volume + x as i32).min(64) } else { (ch.volume - y as i32).max(0) };
            };
            match ef {
                0x0 if pa > 0 => {
                    let nota = match t % 3 {
                        0 => 0,
                        1 => pa >> 4,
                        _ => pa & 0xF,
                    };
                    periodo = ch.periodo;
                    if nota > 0 {
                        let i = PERIODOS.iter().position(|&p| p <= ch.periodo).unwrap_or(0);
                        periodo = PERIODOS[(i + nota as usize).min(35)];
                    }
                }
                0x1 => {
                    ch.periodo = ch.periodo.saturating_sub(pa as u16).max(113);
                    periodo = ch.periodo;
                }
                0x2 => {
                    ch.periodo = (ch.periodo + pa as u16).min(856);
                    periodo = ch.periodo;
                }
                0x3 | 0x5 => {
                    let alvo = ch.alvo_porta;
                    if alvo > 0 {
                        let v = ch.vel_porta as u16;
                        if ch.periodo < alvo {
                            ch.periodo = (ch.periodo + v).min(alvo);
                        } else {
                            ch.periodo = ch.periodo.saturating_sub(v).max(alvo);
                        }
                    }
                    periodo = ch.periodo;
                    if ef == 5 {
                        slide(ch, pa);
                    }
                }
                0x4 | 0x6 => {
                    let delta = (SENO[(ch.vib_pos & 31) as usize] as i32 * ch.vib_prof as i32) >> 7;
                    let delta = if ch.vib_pos & 32 != 0 { -delta } else { delta };
                    periodo = (ch.periodo as i32 + delta).clamp(28, 6848) as u16;
                    ch.vib_pos = ch.vib_pos.wrapping_add(ch.vib_vel) & 63;
                    if ef == 6 {
                        slide(ch, pa);
                    }
                }
                0x7 => {
                    let delta = (SENO[(ch.trem_pos & 31) as usize] as i32 * ch.trem_prof as i32) >> 6;
                    let delta = if ch.trem_pos & 32 != 0 { -delta } else { delta };
                    ch.trem_volume = (ch.volume + delta).clamp(0, 64);
                    ch.trem_pos = ch.trem_pos.wrapping_add(ch.trem_vel) & 63;
                }
                0xA => slide(ch, pa),
                0xE => {
                    let (x, y) = (pa >> 4, pa & 0xF);
                    match x {
                        0x9 if y > 0 && t % y as u32 == 0 => ch.pos = 0,
                        0xC if t == y as u32 => ch.volume = 0,
                        0xD if t == y as u32 => atrasada = ch.atrasada.take(),
                        _ => {}
                    }
                }
                _ => {}
            }
            if let Some(cel) = atrasada {
                self.disparar(c, cel);
                continue;
            }
            if ef != 7 {
                self.canais[c].trem_volume = self.canais[c].volume;
            }
            if self.canais[c].tocando {
                self.atualizar_passo(c, periodo);
            }
        }
    }

    /// Fim de um tique: anda tique, linha e ordem.
    fn avancar(&mut self) {
        self.tique += 1;
        if self.tique < self.velocidade * (1 + self.atraso_padrao) {
            self.tocar_tique();
            return;
        }
        self.tique = 0;
        self.atraso_padrao = 0;
        match self.pular_para.take() {
            Some((ordem, linha)) => {
                self.pos_ordem = ordem;
                self.linha = linha;
            }
            None => {
                self.linha += 1;
                if self.linha >= 64 {
                    self.linha = 0;
                    self.pos_ordem += 1;
                }
            }
        }
        self.tocar_linha();
    }

    fn misturar(&mut self, saida: &mut [i16]) {
        let n = self.canais_n as i32;
        for par in saida.chunks_exact_mut(2) {
            let (mut e, mut d) = (0i32, 0i32);
            for ch in &mut self.canais {
                if !ch.tocando {
                    continue;
                }
                let Some(a) = self.amostras.get(ch.amostra) else { continue };
                let i = (ch.pos >> 16) as usize;
                let fim = if a.tam_repete > 0 { a.repete + a.tam_repete } else { a.dados.len() };
                if i >= fim {
                    if a.tam_repete > 0 {
                        ch.pos -= (a.tam_repete as u64) << 16;
                    } else {
                        ch.tocando = false;
                        continue;
                    }
                }
                let i = ((ch.pos >> 16) as usize).min(a.dados.len().saturating_sub(1));
                // interpolacao linear entre as duas amostras
                let s0 = *a.dados.get(i).unwrap_or(&0) as i32;
                let prox = if i + 1 < fim { i + 1 } else if a.tam_repete > 0 { a.repete } else { i };
                let s1 = *a.dados.get(prox).unwrap_or(&0) as i32;
                let f = ((ch.pos & 0xFFFF) >> 1) as i32;
                let s = s0 + (((s1 - s0) * f) >> 15);
                let v = s * ch.trem_volume; // +-8192
                let (lado, outro) = ((v * SEPARACAO) >> 8, (v * (256 - SEPARACAO)) >> 8);
                if ch.pan < 0 {
                    e += lado;
                    d += outro;
                } else {
                    d += lado;
                    e += outro;
                }
                ch.pos += ch.passo;
            }
            par[0] = (e * 8 / n).clamp(-32768, 32767) as i16;
            par[1] = (d * 8 / n).clamp(-32768, 32767) as i16;
        }
    }
}

impl Audio for Tracker {
    fn taxa(&self) -> u32 {
        self.taxa
    }

    fn ler(&mut self, saida: &mut [i16]) -> usize {
        let mut k = 0;
        while k + 1 < saida.len() && !self.acabou {
            if self.resta_tique == 0 {
                self.avancar();
                self.resta_tique = self.amostras_por_tique();
                if self.acabou {
                    break;
                }
            }
            let quadros = (self.resta_tique as usize).min((saida.len() - k) / 2);
            self.misturar(&mut saida[k..k + quadros * 2]);
            self.resta_tique -= quadros as u32;
            k += quadros * 2;
            self.quadros += quadros as u64;
        }
        k
    }

    fn duracao_ms(&self) -> Option<u64> {
        None
    }

    fn posicao_ms(&self) -> u64 {
        self.quadros * 1000 / self.taxa.max(1) as u64
    }

    fn descricao(&self) -> String {
        let t = if self.titulo.trim().is_empty() { "sem título" } else { self.titulo.trim() };
        format!("MOD “{t}”, {} canais, ordem {}/{}", self.canais_n, (self.pos_ordem + 1).min(self.ordem.len()), self.ordem.len())
    }
}
