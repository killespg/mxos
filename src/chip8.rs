//! Maquina CHIP-8 (com as extensoes do SUPER-CHIP: tela 128x64, rolagem,
//! sprites 16x16 e a fonte grande). Interpretador puro, sem E/S: quem usa
//! entrega as teclas, chama `passo` e `tique_60hz` e le a `tela`.

pub const LARGURA: usize = 128;
pub const ALTURA: usize = 64;
const INICIO: usize = 0x200;

const FONTE: [u8; 80] = [
    0xF0, 0x90, 0x90, 0x90, 0xF0, 0x20, 0x60, 0x20, 0x20, 0x70, 0xF0, 0x10, 0xF0, 0x80, 0xF0, 0xF0, 0x10, 0xF0, 0x10, 0xF0,
    0x90, 0x90, 0xF0, 0x10, 0x10, 0xF0, 0x80, 0xF0, 0x10, 0xF0, 0xF0, 0x80, 0xF0, 0x90, 0xF0, 0xF0, 0x10, 0x20, 0x40, 0x40,
    0xF0, 0x90, 0xF0, 0x90, 0xF0, 0xF0, 0x90, 0xF0, 0x10, 0xF0, 0xF0, 0x90, 0xF0, 0x90, 0x90, 0xE0, 0x90, 0xE0, 0x90, 0xE0,
    0xF0, 0x80, 0x80, 0x80, 0xF0, 0xE0, 0x90, 0x90, 0x90, 0xE0, 0xF0, 0x80, 0xF0, 0x80, 0xF0, 0xF0, 0x80, 0xF0, 0x80, 0x80,
];

/// Digitos 0-9 em 8x10 (Fx30 do SUPER-CHIP).
const FONTE_GRANDE: [u8; 100] = [
    0x3C, 0x7E, 0xE7, 0xC3, 0xC3, 0xC3, 0xC3, 0xE7, 0x7E, 0x3C, 0x18, 0x38, 0x58, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3C,
    0x3E, 0x7F, 0xC3, 0x06, 0x0C, 0x18, 0x30, 0x60, 0xFF, 0xFF, 0x3C, 0x7E, 0xC3, 0x03, 0x0E, 0x0E, 0x03, 0xC3, 0x7E, 0x3C,
    0x06, 0x0E, 0x1E, 0x36, 0x66, 0xC6, 0xFF, 0xFF, 0x06, 0x06, 0xFF, 0xFF, 0xC0, 0xC0, 0xFC, 0xFE, 0x03, 0xC3, 0x7E, 0x3C,
    0x3E, 0x7C, 0xC0, 0xC0, 0xFC, 0xFE, 0xC3, 0xC3, 0x7E, 0x3C, 0xFF, 0xFF, 0x03, 0x06, 0x0C, 0x18, 0x30, 0x60, 0x60, 0x60,
    0x3C, 0x7E, 0xC3, 0xC3, 0x7E, 0x7E, 0xC3, 0xC3, 0x7E, 0x3C, 0x3C, 0x7E, 0xC3, 0xC3, 0x7F, 0x3F, 0x03, 0x03, 0x3E, 0x7C,
];

#[derive(Debug, PartialEq, Eq)]
pub enum Erro {
    Instrucao(u16, u16),
    Pilha,
    Grande,
}

pub struct Chip8 {
    pub mem: [u8; 4096],
    pub v: [u8; 16],
    pub i: u16,
    pub pc: u16,
    pilha: [u16; 16],
    sp: usize,
    pub atraso: u8,
    pub som: u8,
    /// Pixel ligado = true; so os primeiros 64x32 no modo normal.
    pub tela: [bool; LARGURA * ALTURA],
    pub alta: bool,
    pub teclas: [bool; 16],
    /// Fx0A: esperando uma tecla (registrador destino).
    esperando: Option<usize>,
    flags_rpl: [u8; 16],
    semente: u32,
    pub mudou: bool,
    pub parou: bool,
}

impl Chip8 {
    pub fn novo(rom: &[u8]) -> Result<Chip8, Erro> {
        if rom.len() > 4096 - INICIO {
            return Err(Erro::Grande);
        }
        let mut c = Chip8 {
            mem: [0; 4096],
            v: [0; 16],
            i: 0,
            pc: INICIO as u16,
            pilha: [0; 16],
            sp: 0,
            atraso: 0,
            som: 0,
            tela: [false; LARGURA * ALTURA],
            alta: false,
            teclas: [false; 16],
            esperando: None,
            flags_rpl: [0; 16],
            semente: 0x1234_5678,
            mudou: true,
            parou: false,
        };
        c.mem[..80].copy_from_slice(&FONTE);
        c.mem[80..180].copy_from_slice(&FONTE_GRANDE);
        c.mem[INICIO..INICIO + rom.len()].copy_from_slice(rom);
        Ok(c)
    }

    /// Tamanho da tela no modo atual.
    pub fn dimensoes(&self) -> (usize, usize) {
        if self.alta { (128, 64) } else { (64, 32) }
    }

    pub fn pixel(&self, x: usize, y: usize) -> bool {
        self.tela[y * LARGURA + x]
    }

    fn acaso(&mut self) -> u8 {
        // xorshift
        self.semente ^= self.semente << 13;
        self.semente ^= self.semente >> 17;
        self.semente ^= self.semente << 5;
        (self.semente >> 8) as u8
    }

    /// Os timers de atraso e de som (60 vezes por segundo).
    pub fn tique_60hz(&mut self) {
        self.atraso = self.atraso.saturating_sub(1);
        self.som = self.som.saturating_sub(1);
    }

    pub fn tecla(&mut self, k: usize, apertada: bool) {
        if k >= 16 {
            return;
        }
        // Fx0A espera a tecla ser solta (como no COSMAC VIP)
        if let (Some(x), false) = (self.esperando, apertada) {
            if self.teclas[k] {
                self.v[x] = k as u8;
                self.esperando = None;
            }
        }
        self.teclas[k] = apertada;
    }

    fn limpar(&mut self) {
        self.tela.fill(false);
        self.mudou = true;
    }

    fn rolar_baixo(&mut self, n: usize) {
        let (l, a) = self.dimensoes();
        for y in (0..a).rev() {
            for x in 0..l {
                self.tela[y * LARGURA + x] = if y >= n { self.tela[(y - n) * LARGURA + x] } else { false };
            }
        }
        self.mudou = true;
    }

    fn rolar_lado(&mut self, direita: bool) {
        let (l, a) = self.dimensoes();
        let n = 4;
        for y in 0..a {
            let linha = &mut self.tela[y * LARGURA..y * LARGURA + l];
            if direita {
                linha.copy_within(0..l - n, n);
                linha[..n].fill(false);
            } else {
                linha.copy_within(n..l, 0);
                linha[l - n..].fill(false);
            }
        }
        self.mudou = true;
    }

    /// Desenha um sprite; devolve se apagou algum pixel (VF).
    fn desenhar(&mut self, x: u8, y: u8, n: u8) -> u8 {
        let (l, a) = self.dimensoes();
        let (x0, y0) = (x as usize % l, y as usize % a);
        let (largura, linhas, bytes) = if n == 0 && self.alta { (16, 16, 2) } else { (8, if n == 0 { 16 } else { n as usize }, 1) };
        let mut colisao = 0;
        for r in 0..linhas {
            let py = y0 + r;
            if py >= a {
                break; // corta embaixo (nao da a volta)
            }
            let mut bits: u16 = 0;
            for b in 0..bytes {
                bits = (bits << 8) | self.mem[(self.i as usize + r * bytes + b) & 0xFFF] as u16;
            }
            for c in 0..largura {
                if bits & (1 << (largura - 1 - c)) == 0 {
                    continue;
                }
                let px = x0 + c;
                if px >= l {
                    break;
                }
                let p = &mut self.tela[py * LARGURA + px];
                if *p {
                    colisao = 1;
                }
                *p = !*p;
            }
        }
        self.mudou = true;
        colisao
    }

    /// Uma instrucao.
    pub fn passo(&mut self) -> Result<(), Erro> {
        if self.parou || self.esperando.is_some() {
            return Ok(());
        }
        let pc = self.pc as usize & 0xFFF;
        let op = u16::from_be_bytes([self.mem[pc], self.mem[(pc + 1) & 0xFFF]]);
        self.pc = self.pc.wrapping_add(2);
        let x = ((op >> 8) & 0xF) as usize;
        let y = ((op >> 4) & 0xF) as usize;
        let n = (op & 0xF) as u8;
        let nn = (op & 0xFF) as u8;
        let nnn = op & 0xFFF;
        match op >> 12 {
            0x0 => match op {
                0x00E0 => self.limpar(),
                0x00EE => {
                    if self.sp == 0 {
                        return Err(Erro::Pilha);
                    }
                    self.sp -= 1;
                    self.pc = self.pilha[self.sp];
                }
                0x00FB => self.rolar_lado(true),
                0x00FC => self.rolar_lado(false),
                0x00FD => self.parou = true,
                0x00FE => {
                    self.alta = false;
                    self.limpar();
                }
                0x00FF => {
                    self.alta = true;
                    self.limpar();
                }
                o if o & 0xFFF0 == 0x00C0 => self.rolar_baixo((o & 0xF) as usize),
                _ => return Err(Erro::Instrucao(pc as u16, op)),
            },
            0x1 => self.pc = nnn,
            0x2 => {
                if self.sp >= 16 {
                    return Err(Erro::Pilha);
                }
                self.pilha[self.sp] = self.pc;
                self.sp += 1;
                self.pc = nnn;
            }
            0x3 => {
                if self.v[x] == nn {
                    self.pc += 2;
                }
            }
            0x4 => {
                if self.v[x] != nn {
                    self.pc += 2;
                }
            }
            0x5 => {
                if self.v[x] == self.v[y] {
                    self.pc += 2;
                }
            }
            0x6 => self.v[x] = nn,
            0x7 => self.v[x] = self.v[x].wrapping_add(nn),
            0x8 => {
                let (a, b) = (self.v[x], self.v[y]);
                match n {
                    0x0 => self.v[x] = b,
                    0x1 => self.v[x] = a | b,
                    0x2 => self.v[x] = a & b,
                    0x3 => self.v[x] = a ^ b,
                    0x4 => {
                        let (r, c) = a.overflowing_add(b);
                        self.v[x] = r;
                        self.v[0xF] = c as u8;
                    }
                    0x5 => {
                        self.v[x] = a.wrapping_sub(b);
                        self.v[0xF] = (a >= b) as u8;
                    }
                    0x6 => {
                        // quirk do SUPER-CHIP: desloca Vx (nao Vy)
                        self.v[x] = a >> 1;
                        self.v[0xF] = a & 1;
                    }
                    0x7 => {
                        self.v[x] = b.wrapping_sub(a);
                        self.v[0xF] = (b >= a) as u8;
                    }
                    0xE => {
                        self.v[x] = a << 1;
                        self.v[0xF] = a >> 7;
                    }
                    _ => return Err(Erro::Instrucao(pc as u16, op)),
                }
            }
            0x9 => {
                if self.v[x] != self.v[y] {
                    self.pc += 2;
                }
            }
            0xA => self.i = nnn,
            0xB => self.pc = nnn.wrapping_add(self.v[0] as u16),
            0xC => self.v[x] = self.acaso() & nn,
            0xD => {
                let (vx, vy) = (self.v[x], self.v[y]);
                self.v[0xF] = self.desenhar(vx, vy, n);
            }
            0xE => match nn {
                0x9E => {
                    if self.teclas[self.v[x] as usize & 0xF] {
                        self.pc += 2;
                    }
                }
                0xA1 => {
                    if !self.teclas[self.v[x] as usize & 0xF] {
                        self.pc += 2;
                    }
                }
                _ => return Err(Erro::Instrucao(pc as u16, op)),
            },
            0xF => match nn {
                0x07 => self.v[x] = self.atraso,
                0x0A => self.esperando = Some(x),
                0x15 => self.atraso = self.v[x],
                0x18 => self.som = self.v[x],
                0x1E => self.i = self.i.wrapping_add(self.v[x] as u16) & 0xFFF,
                0x29 => self.i = (self.v[x] as u16 & 0xF) * 5,
                0x30 => self.i = 80 + (self.v[x] as u16 % 10) * 10,
                0x33 => {
                    let v = self.v[x];
                    let i = self.i as usize;
                    self.mem[i & 0xFFF] = v / 100;
                    self.mem[(i + 1) & 0xFFF] = v / 10 % 10;
                    self.mem[(i + 2) & 0xFFF] = v % 10;
                }
                0x55 => {
                    for k in 0..=x {
                        self.mem[(self.i as usize + k) & 0xFFF] = self.v[k];
                    }
                }
                0x65 => {
                    for k in 0..=x {
                        self.v[k] = self.mem[(self.i as usize + k) & 0xFFF];
                    }
                }
                0x75 => self.flags_rpl[..=x.min(7)].copy_from_slice(&self.v[..=x.min(7)]),
                0x85 => {
                    let k = x.min(7);
                    self.v[..=k].copy_from_slice(&self.flags_rpl[..=k]);
                }
                _ => return Err(Erro::Instrucao(pc as u16, op)),
            },
            _ => unreachable!(),
        }
        Ok(())
    }
}
