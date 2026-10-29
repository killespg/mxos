//! Desenho num buffer de pixels qualquer (ramfb do QEMU, framebuffer do
//! U-Boot na TV box...). Nao sabe nada do dispositivo: so recebe endereco,
//! largura, altura, passo (bytes por linha) e formato do pixel.
//!
//! Cores sempre em 0x00RRGGBB; cada formato converte na hora de escrever.
//! Todo acesso ao buffer e volatile e alinhado ao tamanho do pixel, para
//! funcionar ate com a MMU desligada (RAM vira memoria de dispositivo).
//!
//! Console: grade de caracteres por cima de uma Tela, com cursor, quebra
//! de linha e rolagem. Implementa fmt::Write, entao pode espelhar a serial.

#![allow(dead_code)]

use core::arch::asm;
use core::fmt;
use core::ptr::{read_volatile, write_volatile};

use crate::fonte::{self, Fonte, MONO, MONO_BASE};

/// Cor no formato 0x00RRGGBB.
pub type Cor = u32;

pub const PRETO: Cor = 0x0000_0000;
pub const BRANCO: Cor = 0x00FF_FFFF;
pub const VERMELHO: Cor = 0x00FF_0000;
pub const VERDE: Cor = 0x0000_FF00;
pub const AZUL: Cor = 0x0000_00FF;
pub const AMARELO: Cor = 0x00FF_FF00;
pub const CIANO: Cor = 0x0000_FFFF;
pub const MAGENTA: Cor = 0x00FF_00FF;
pub const CINZA: Cor = 0x0080_8080;

/// Como cada pixel fica na memoria.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Formato {
    /// u32 0x00RRGGBB (bytes B, G, R, X). ramfb e o modo "posse" da TV box.
    Xrgb8888,
    /// u16 RRRRRGGGGGGBBBBB. Padrao do logo do U-Boot da Amlogic.
    Rgb565,
    /// 3 bytes B, G, R.
    Rgb888,
}

impl Formato {
    pub const fn bytes_por_pixel(self) -> usize {
        match self {
            Formato::Xrgb8888 => 4,
            Formato::Rgb565 => 2,
            Formato::Rgb888 => 3,
        }
    }

    /// Cor 0x00RRGGBB -> valor guardado na memoria.
    pub const fn codificar(self, cor: Cor) -> u32 {
        match self {
            Formato::Xrgb8888 | Formato::Rgb888 => cor & 0x00FF_FFFF,
            Formato::Rgb565 => {
                let r = cor >> 19 & 0x1F;
                let g = cor >> 10 & 0x3F;
                let b = cor >> 3 & 0x1F;
                r << 11 | g << 5 | b
            }
        }
    }

    /// Valor da memoria -> cor 0x00RRGGBB (565 perde os bits baixos).
    pub const fn decodificar(self, valor: u32) -> Cor {
        match self {
            Formato::Xrgb8888 | Formato::Rgb888 => valor & 0x00FF_FFFF,
            Formato::Rgb565 => {
                let r = (valor >> 11 & 0x1F) << 3;
                let g = (valor >> 5 & 0x3F) << 2;
                let b = (valor & 0x1F) << 3;
                r << 16 | g << 8 | b
            }
        }
    }
}

/// Tamanho da celula de texto, em multiplos de 8 pixels. CONSOLE (1x2)
/// e a grade 8x16 do terminal, com a JetBrains Mono; as outras usam a Inter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Escala {
    pub x: usize,
    pub y: usize,
}

impl Escala {
    pub const NORMAL: Escala = Escala { x: 1, y: 1 };
    pub const CONSOLE: Escala = Escala { x: 1, y: 2 };

    pub const fn uniforme(n: usize) -> Escala {
        Escala { x: n, y: n }
    }

    pub const fn largura_celula(self) -> usize {
        8 * self.x
    }

    pub const fn altura_celula(self) -> usize {
        8 * self.y
    }
}

/// Uma area retangular de pixels. Copiar uma Tela so copia a descricao:
/// as duas apontam para os mesmos pixels.
#[derive(Clone, Copy, Debug)]
pub struct Tela {
    base: usize,
    largura: usize,
    altura: usize,
    passo: usize,
    formato: Formato,
    /// Limpar o cache depois de desenhar (dispositivo que le a RAM sem
    /// passar pelo cache da CPU, como o VPU da Amlogic).
    limpar_cache: bool,
}

impl Tela {
    /// # Seguranca
    /// [base, base + passo * altura) precisa ser memoria valida, so desta
    /// Tela, alinhada ao tamanho do pixel, com passo >= largura * bytes.
    pub const unsafe fn new(base: usize, largura: usize, altura: usize, passo: usize, formato: Formato) -> Self {
        Tela { base, largura, altura, passo, formato, limpar_cache: false }
    }

    /// Liga a limpeza de cache ("dc cvac") depois de cada desenho.
    pub const fn com_limpeza_de_cache(mut self, ligar: bool) -> Self {
        self.limpar_cache = ligar;
        self
    }

    pub const fn largura(&self) -> usize {
        self.largura
    }

    pub const fn altura(&self) -> usize {
        self.altura
    }

    pub const fn passo(&self) -> usize {
        self.passo
    }

    pub const fn formato(&self) -> Formato {
        self.formato
    }

    pub const fn base(&self) -> usize {
        self.base
    }

    /// Um pedaco desta tela, com coordenadas proprias (0,0 no canto dele).
    /// None se o pedaco sair da tela.
    pub fn regiao(&self, x: usize, y: usize, largura: usize, altura: usize) -> Option<Tela> {
        let cabe_x = x.checked_add(largura).is_some_and(|fim| fim <= self.largura);
        let cabe_y = y.checked_add(altura).is_some_and(|fim| fim <= self.altura);
        if !cabe_x || !cabe_y {
            return None;
        }
        let mut parte = *self;
        parte.base = self.endereco(x, y);
        parte.largura = largura;
        parte.altura = altura;
        Some(parte)
    }

    fn endereco(&self, x: usize, y: usize) -> usize {
        self.base + y * self.passo + x * self.formato.bytes_por_pixel()
    }

    /// Escreve um valor ja codificado no endereco de um pixel.
    fn gravar(&self, endereco: usize, valor: u32) {
        unsafe {
            match self.formato {
                Formato::Xrgb8888 => write_volatile(endereco as *mut u32, valor),
                Formato::Rgb565 => write_volatile(endereco as *mut u16, valor as u16),
                Formato::Rgb888 => {
                    // 3 bytes nunca ficam alinhados: um byte de cada vez
                    write_volatile(endereco as *mut u8, valor as u8);
                    write_volatile((endereco + 1) as *mut u8, (valor >> 8) as u8);
                    write_volatile((endereco + 2) as *mut u8, (valor >> 16) as u8);
                }
            }
        }
    }

    /// Le o valor codificado guardado no endereco de um pixel.
    fn buscar(&self, endereco: usize) -> u32 {
        unsafe {
            match self.formato {
                Formato::Xrgb8888 => read_volatile(endereco as *const u32),
                Formato::Rgb565 => read_volatile(endereco as *const u16) as u32,
                Formato::Rgb888 => {
                    let b = read_volatile(endereco as *const u8) as u32;
                    let g = read_volatile((endereco + 1) as *const u8) as u32;
                    let r = read_volatile((endereco + 2) as *const u8) as u32;
                    r << 16 | g << 8 | b
                }
            }
        }
    }

    /// Pinta um pixel. Fora da tela nao faz nada.
    pub fn pixel(&self, x: usize, y: usize, cor: Cor) {
        if x < self.largura && y < self.altura {
            self.gravar(self.endereco(x, y), self.formato.codificar(cor));
            self.depois_de_desenhar(y, 1);
        }
    }

    /// Cor de um pixel (None fora da tela).
    pub fn ler_pixel(&self, x: usize, y: usize) -> Option<Cor> {
        if x < self.largura && y < self.altura {
            Some(self.formato.decodificar(self.buscar(self.endereco(x, y))))
        } else {
            None
        }
    }

    /// Retangulo cheio. A parte que sai da tela e cortada.
    pub fn retangulo(&self, x: usize, y: usize, largura: usize, altura: usize, cor: Cor) {
        let fim_x = x.saturating_add(largura).min(self.largura);
        let fim_y = y.saturating_add(altura).min(self.altura);
        if x >= fim_x || y >= fim_y {
            return;
        }
        let valor = self.formato.codificar(cor);
        for linha in y..fim_y {
            self.pintar_trecho(linha, x, fim_x, valor);
        }
        self.depois_de_desenhar(y, fim_y - y);
    }

    /// Pinta as colunas [x, fim_x) de uma linha com um valor codificado.
    fn pintar_trecho(&self, y: usize, x: usize, fim_x: usize, valor: u32) {
        let bytes = self.formato.bytes_por_pixel();
        let mut endereco = self.endereco(x, y);
        for _ in x..fim_x {
            self.gravar(endereco, valor);
            endereco += bytes;
        }
    }

    /// Pinta a tela inteira.
    pub fn limpar(&self, cor: Cor) {
        self.retangulo(0, 0, self.largura, self.altura, cor);
    }

    /// Desenha um caractere na celula com o canto de cima a esquerda em
    /// (x, y), com a fonte monoespacada. fundo = None deixa transparente.
    pub fn caractere(&self, x: usize, y: usize, c: char, frente: Cor, fundo: Option<Cor>, escala: Escala) {
        let (largura, altura) = (escala.largura_celula(), escala.altura_celula());
        if let Some(fundo) = fundo {
            self.retangulo(x, y, largura, altura, fundo);
        }
        let mut alvo = *self;
        let base = y as i32 + MONO_BASE * altura as i32 / fonte::MONO_ALTURA;
        fonte::desenhar_glifo(&mut alvo, x as i32, base, &MONO.glifo(c), frente);
    }

    /// Escreve um texto (UTF-8) numa linha so, sem quebra, com a Inter do
    /// tamanho da escala (maiusculas centralizadas na faixa da celula).
    /// Devolve o x logo depois do ultimo caractere.
    pub fn texto(&self, x: usize, y: usize, texto: &str, frente: Cor, fundo: Option<Cor>, escala: Escala) -> usize {
        let f: &Fonte = fonte::por_escala(escala.y.max(escala.x));
        let altura = escala.altura_celula();
        let largura = f.largura(texto) as usize;
        if let Some(fundo) = fundo {
            self.retangulo(x, y, largura, altura, fundo);
        }
        let mut alvo = *self;
        let base = f.base_centralizada(y as i32, altura as i32);
        fonte::desenhar(&mut alvo, x as i32, base, texto, frente, f);
        x + largura
    }

    /// Escreve um texto (UTF-8) com a fonte `f` e a linha de base em `base`.
    /// Devolve o x logo depois do ultimo caractere.
    pub fn texto_com(&self, x: usize, base: i32, texto: &str, frente: Cor, f: &Fonte) -> usize {
        let mut alvo = *self;
        x + fonte::desenhar(&mut alvo, x as i32, base, texto, frente, f).max(0) as usize
    }

    /// Sobe o conteudo `linhas` pixels e pinta a faixa de baixo com `fundo`.
    pub fn rolar(&self, linhas: usize, fundo: Cor) {
        if linhas >= self.altura {
            self.limpar(fundo);
            return;
        }
        let bytes = self.largura * self.formato.bytes_por_pixel();
        for y in 0..self.altura - linhas {
            copiar_linha(self.endereco(0, y + linhas), self.endereco(0, y), bytes);
        }
        self.depois_de_desenhar(0, self.altura - linhas);
        self.retangulo(0, self.altura - linhas, self.largura, linhas, fundo);
    }

    /// Ponto unico para manutencao de cache depois de mexer nas linhas
    /// [y, y + quantas). No QEMU (ramfb) nao e preciso: o TCG nao tem cache.
    fn depois_de_desenhar(&self, y: usize, quantas: usize) {
        if self.limpar_cache && quantas > 0 {
            let inicio = self.endereco(0, y);
            let fim = self.endereco(0, y + quantas - 1) + self.largura * self.formato.bytes_por_pixel();
            limpar_cache(inicio, fim - inicio);
        }
    }
}

/// Copia `bytes` de uma linha para outra (de cima para baixo nunca se
/// sobrepoe numa rolagem). Usa u32 quando tudo esta alinhado.
fn copiar_linha(origem: usize, destino: usize, bytes: usize) {
    let alinhado = origem % 4 == 0 && destino % 4 == 0 && bytes % 4 == 0;
    let passo = if alinhado { 4 } else { 1 };
    let mut i = 0;
    while i < bytes {
        unsafe {
            if alinhado {
                write_volatile((destino + i) as *mut u32, read_volatile((origem + i) as *const u32));
            } else {
                write_volatile((destino + i) as *mut u8, read_volatile((origem + i) as *const u8));
            }
        }
        i += passo;
    }
}

/// Manda para a RAM as linhas de cache de [inicio, inicio + tamanho)
/// ("dc cvac", limpar ate o ponto de coerencia) e espera terminar.
/// Necessario quando outro dispositivo le a RAM sem ver o cache da CPU.
pub fn limpar_cache(inicio: usize, tamanho: usize) {
    let linha = tamanho_linha_cache();
    let mut endereco = inicio & !(linha - 1);
    while endereco < inicio + tamanho {
        unsafe { asm!("dc cvac, {0}", in(reg) endereco, options(nostack, preserves_flags)) };
        endereco += linha;
    }
    unsafe { asm!("dsb sy", options(nostack, preserves_flags)) };
}

/// Menor linha de cache de dados: CTR_EL0.DminLine = log2 de palavras de 4 bytes.
fn tamanho_linha_cache() -> usize {
    let ctr: u64;
    unsafe { asm!("mrs {0}, ctr_el0", out(reg) ctr, options(nomem, nostack, preserves_flags)) };
    4 << (ctr >> 16 & 0xF)
}

/// Maior sublinhado de cursor que conseguimos guardar e restaurar.
const MAX_PIXELS_CURSOR: usize = 64;
/// Um Tab avanca ate a proxima coluna multipla disto.
const TAB: usize = 8;

/// Console de texto por cima de uma Tela. Nao guarda o texto: rolar move
/// os pixels. O cursor e um sublinhado na ultima linha da celula; o que
/// estava embaixo dele e guardado e volta quando ele sai.
pub struct Console {
    tela: Tela,
    escala: Escala,
    frente: Cor,
    fundo: Cor,
    coluna: usize,
    linha: usize,
    colunas: usize,
    linhas: usize,
    cursor_ligado: bool,
    /// Pixels (ja codificados) que o cursor cobriu; None = cursor apagado.
    embaixo_do_cursor: Option<[u32; MAX_PIXELS_CURSOR]>,
    /// Bytes de um caractere UTF-8 ainda incompleto e quantos faltam.
    utf8: [u8; 4],
    utf8_tem: usize,
    utf8_falta: usize,
}

impl Console {
    /// Cria o console, limpa a tela com `fundo` e poe o cursor em (0, 0).
    pub fn new(tela: Tela, escala: Escala, frente: Cor, fundo: Cor) -> Self {
        let mut console = Console {
            tela,
            escala,
            frente,
            fundo,
            coluna: 0,
            linha: 0,
            colunas: tela.largura() / escala.largura_celula(),
            linhas: tela.altura() / escala.altura_celula(),
            cursor_ligado: true,
            embaixo_do_cursor: None,
            utf8: [0; 4],
            utf8_tem: 0,
            utf8_falta: 0,
        };
        console.limpar();
        console
    }

    pub fn colunas(&self) -> usize {
        self.colunas
    }

    pub fn linhas(&self) -> usize {
        self.linhas
    }

    /// (coluna, linha) do cursor.
    pub fn posicao(&self) -> (usize, usize) {
        (self.coluna, self.linha)
    }

    pub fn tela(&self) -> &Tela {
        &self.tela
    }

    /// Cores do texto daqui em diante.
    pub fn cores(&mut self, frente: Cor, fundo: Cor) {
        self.apagar_cursor();
        self.frente = frente;
        self.fundo = fundo;
        self.desenhar_cursor();
    }

    pub fn mostrar_cursor(&mut self, ligar: bool) {
        self.apagar_cursor();
        self.cursor_ligado = ligar;
        self.desenhar_cursor();
    }

    /// Limpa tudo e volta o cursor ao canto.
    pub fn limpar(&mut self) {
        self.embaixo_do_cursor = None;
        self.tela.limpar(self.fundo);
        self.coluna = 0;
        self.linha = 0;
        self.desenhar_cursor();
    }

    pub fn escrever_texto(&mut self, texto: &str) {
        for byte in texto.bytes() {
            self.escrever_byte(byte);
        }
    }

    /// Trata um byte: imprimivel, \n, \r, \t, backspace (0x08 ou 0x7F).
    /// UTF-8 e montado caractere a caractere; outros controles sao ignorados.
    pub fn escrever_byte(&mut self, byte: u8) {
        if self.colunas == 0 || self.linhas == 0 {
            return;
        }
        self.apagar_cursor();
        self.interpretar(byte);
        self.desenhar_cursor();
    }

    fn interpretar(&mut self, byte: u8) {
        if byte >= 0x80 {
            self.juntar_utf8(byte);
            return;
        }
        self.utf8_falta = 0;
        match byte {
            b'\n' => self.nova_linha(),
            b'\r' => self.coluna = 0,
            b'\t' => self.tabular(),
            0x08 | 0x7F => self.voltar(),
            0x20..=0x7E => self.imprimir(byte as char),
            _ => {}
        }
    }

    /// Junta os bytes de um caractere UTF-8 e imprime quando completa.
    /// Sequencia quebrada vira '?'.
    fn juntar_utf8(&mut self, byte: u8) {
        if byte < 0xC0 {
            if self.utf8_falta == 0 {
                self.imprimir('?');
                return;
            }
            self.utf8[self.utf8_tem] = byte;
            self.utf8_tem += 1;
            self.utf8_falta -= 1;
            if self.utf8_falta == 0 {
                let c = core::str::from_utf8(&self.utf8[..self.utf8_tem]).ok().and_then(|s| s.chars().next());
                self.imprimir(c.unwrap_or('?'));
            }
            return;
        }
        if self.utf8_falta > 0 {
            self.imprimir('?');
        }
        self.utf8[0] = byte;
        self.utf8_tem = 1;
        self.utf8_falta = match byte {
            0xC0..=0xDF => 1,
            0xE0..=0xEF => 2,
            _ => 3,
        };
    }

    /// Desenha o caractere na posicao do cursor e avanca (quebra no fim).
    fn imprimir(&mut self, c: char) {
        let (x, y) = self.pixel_da_celula(self.coluna, self.linha);
        self.tela.caractere(x, y, c, self.frente, Some(self.fundo), self.escala);
        self.coluna += 1;
        if self.coluna == self.colunas {
            self.nova_linha();
        }
    }

    fn tabular(&mut self) {
        self.imprimir(' ');
        while self.coluna % TAB != 0 {
            self.imprimir(' ');
        }
    }

    /// Backspace: volta uma coluna e apaga o que estava la.
    fn voltar(&mut self) {
        if self.coluna > 0 {
            self.coluna -= 1;
            let (x, y) = self.pixel_da_celula(self.coluna, self.linha);
            self.tela.retangulo(x, y, self.escala.largura_celula(), self.escala.altura_celula(), self.fundo);
        }
    }

    fn nova_linha(&mut self) {
        self.coluna = 0;
        self.linha += 1;
        if self.linha == self.linhas {
            self.tela.rolar(self.escala.altura_celula(), self.fundo);
            self.linha = self.linhas - 1;
        }
    }

    fn pixel_da_celula(&self, coluna: usize, linha: usize) -> (usize, usize) {
        (coluna * self.escala.largura_celula(), linha * self.escala.altura_celula())
    }

    /// Area do sublinhado: a ultima linha da fonte, na escala do console.
    fn area_do_cursor(&self) -> Option<(usize, usize, usize, usize)> {
        let (x, y) = self.pixel_da_celula(self.coluna, self.linha);
        let altura = self.escala.y;
        let largura = self.escala.largura_celula();
        let cabe = self.cursor_ligado && largura * altura <= MAX_PIXELS_CURSOR;
        cabe.then_some((x, y + self.escala.altura_celula() - altura, largura, altura))
    }

    /// Guarda os pixels embaixo do cursor e pinta o sublinhado.
    fn desenhar_cursor(&mut self) {
        let Some((x, y, largura, altura)) = self.area_do_cursor() else { return };
        let mut guardado = [0u32; MAX_PIXELS_CURSOR];
        for dy in 0..altura {
            for dx in 0..largura {
                let endereco = self.tela.endereco(x + dx, y + dy);
                guardado[dy * largura + dx] = self.tela.buscar(endereco);
            }
        }
        self.embaixo_do_cursor = Some(guardado);
        self.tela.retangulo(x, y, largura, altura, self.frente);
    }

    /// Devolve os pixels que o cursor cobria.
    fn apagar_cursor(&mut self) {
        let Some(guardado) = self.embaixo_do_cursor.take() else { return };
        let Some((x, y, largura, altura)) = self.area_do_cursor() else { return };
        for dy in 0..altura {
            for dx in 0..largura {
                let endereco = self.tela.endereco(x + dx, y + dy);
                self.tela.gravar(endereco, guardado[dy * largura + dx]);
            }
        }
        self.tela.depois_de_desenhar(y, altura);
    }
}

impl fmt::Write for Console {
    fn write_str(&mut self, texto: &str) -> fmt::Result {
        self.escrever_texto(texto);
        Ok(())
    }
}

impl fonte::Alvo for Tela {
    fn tamanho(&self) -> (i32, i32) {
        (self.largura as i32, self.altura as i32)
    }

    fn ler(&self, x: i32, y: i32) -> u32 {
        self.ler_pixel(x as usize, y as usize).unwrap_or(0)
    }

    fn escrever(&mut self, x: i32, y: i32, cor: u32) {
        self.pixel(x as usize, y as usize, cor);
    }
}
