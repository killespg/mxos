//! Desenho em memoria para a interface grafica.
//!
//! Superficie: um Vec<u32> de pixels XRGB8888 (0x00RRGGBB), linha a linha,
//! passo == largura. Tudo e desenhado aqui (no heap, memoria normal com
//! cache) e so a area que mudou vai para o framebuffer com apresentar().
//!
//! Desempenho (o QEMU emula cada instrucao): cada funcao recorta UMA vez
//! no comeco e depois trabalha com fatias inteiras de linha (fill e
//! copy_from_slice), sem teste de limite por pixel.
//!
//! O Vec nunca e realocado depois de criado, entao o endereco dos pixels
//! nao muda: da para entregar uma Tela por cima dele (para o Console).

#![allow(dead_code)]

use alloc::vec;
use alloc::vec::Vec;
use core::arch::asm;
use core::ptr;

use crate::fonte::{self, Fonte};
use crate::video::tela::{Formato, Tela};

/// Cor no formato 0x00RRGGBB.
pub type Cor = crate::video::tela::Cor;

/// Retangulo em pixels. Largura ou altura <= 0 = vazio.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Retangulo {
    pub x: i32,
    pub y: i32,
    pub largura: i32,
    pub altura: i32,
}

impl Retangulo {
    pub const fn novo(x: i32, y: i32, largura: i32, altura: i32) -> Self {
        Retangulo { x, y, largura, altura }
    }

    pub fn vazio(&self) -> bool {
        self.largura <= 0 || self.altura <= 0
    }

    /// Primeira coluna depois do retangulo.
    pub fn direita(&self) -> i32 {
        self.x.saturating_add(self.largura)
    }

    /// Primeira linha depois do retangulo.
    pub fn baixo(&self) -> i32 {
        self.y.saturating_add(self.altura)
    }

    pub fn contem(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.direita() && y >= self.y && y < self.baixo()
    }

    /// Parte comum dos dois (vazio = Retangulo::default()).
    pub fn intersecao(&self, outro: &Retangulo) -> Retangulo {
        let x0 = self.x.max(outro.x);
        let y0 = self.y.max(outro.y);
        let x1 = self.direita().min(outro.direita());
        let y1 = self.baixo().min(outro.baixo());
        if x1 <= x0 || y1 <= y0 {
            return Retangulo::default();
        }
        Retangulo::novo(x0, y0, x1 - x0, y1 - y0)
    }

    /// Menor retangulo que cobre os dois (um vazio nao conta).
    pub fn uniao(&self, outro: &Retangulo) -> Retangulo {
        if self.vazio() {
            return *outro;
        }
        if outro.vazio() {
            return *self;
        }
        let x0 = self.x.min(outro.x);
        let y0 = self.y.min(outro.y);
        let x1 = self.direita().max(outro.direita());
        let y1 = self.baixo().max(outro.baixo());
        Retangulo::novo(x0, y0, x1 - x0, y1 - y0)
    }

    pub fn deslocado(&self, dx: i32, dy: i32) -> Retangulo {
        Retangulo::novo(self.x.saturating_add(dx), self.y.saturating_add(dy), self.largura, self.altura)
    }
}

/// Mistura duas cores: peso 0 = so `a`, 255 = so `b`.
pub fn misturar(a: Cor, b: Cor, peso: u32) -> Cor {
    let peso = peso.min(255);
    let canal = |deslocamento: u32| {
        let ca = a >> deslocamento & 0xFF;
        let cb = b >> deslocamento & 0xFF;
        (ca * (255 - peso) + cb * peso + 127) / 255
    };
    canal(16) << 16 | canal(8) << 8 | canal(0)
}

/// Trecho [inicio, fim) de um intervalo que cai dentro de [0, limite).
/// None se nao sobra nada.
fn recortar_intervalo(inicio: i64, fim: i64, limite: usize) -> Option<(usize, usize)> {
    let inicio = inicio.max(0);
    let fim = fim.min(limite as i64);
    (inicio < fim).then_some((inicio as usize, fim as usize))
}

/// Pixels XRGB8888 em memoria comum.
pub struct Superficie {
    largura: usize,
    altura: usize,
    pixels: Vec<u32>,
}

impl Superficie {
    /// Cria a superficie ja pintada com `cor`.
    pub fn nova(largura: usize, altura: usize, cor: Cor) -> Superficie {
        Superficie { largura, altura, pixels: vec![cor & 0x00FF_FFFF; largura * altura] }
    }

    pub fn largura(&self) -> usize {
        self.largura
    }

    pub fn altura(&self) -> usize {
        self.altura
    }

    pub fn pixels(&self) -> &[u32] {
        &self.pixels
    }

    pub fn pixels_mut(&mut self) -> &mut [u32] {
        &mut self.pixels
    }

    /// A superficie inteira como Retangulo.
    pub fn area(&self) -> Retangulo {
        Retangulo::novo(0, 0, self.largura as i32, self.altura as i32)
    }

    /// Uma Tela por cima dos proprios pixels (para usar um Console aqui).
    /// Vale enquanto a Superficie existir: o Vec nunca e realocado.
    pub fn tela(&mut self) -> Tela {
        let base = self.pixels.as_mut_ptr() as usize;
        // Seguranca: o buffer tem largura * altura u32 alinhados e passo
        // largura * 4; so esta Superficie usa esses pixels.
        unsafe { Tela::new(base, self.largura, self.altura, self.largura * 4, Formato::Xrgb8888) }
    }

    /// Pinta tudo.
    pub fn limpar(&mut self, cor: Cor) {
        self.pixels.fill(cor & 0x00FF_FFFF);
    }

    /// Parte de `r` dentro da superficie: (x0, y0, x1, y1) com fim aberto.
    fn recorte(&self, r: Retangulo) -> Option<(usize, usize, usize, usize)> {
        if r.vazio() {
            return None;
        }
        let (x0, x1) = recortar_intervalo(r.x as i64, r.x as i64 + r.largura as i64, self.largura)?;
        let (y0, y1) = recortar_intervalo(r.y as i64, r.y as i64 + r.altura as i64, self.altura)?;
        Some((x0, y0, x1, y1))
    }

    /// Fatia das colunas [x0, x1) da linha y (ja recortadas).
    fn trecho(&mut self, y: usize, x0: usize, x1: usize) -> &mut [u32] {
        let inicio = y * self.largura;
        &mut self.pixels[inicio + x0..inicio + x1]
    }

    pub fn pixel(&mut self, x: i32, y: i32, cor: Cor) {
        if x >= 0 && y >= 0 && (x as usize) < self.largura && (y as usize) < self.altura {
            self.pixels[y as usize * self.largura + x as usize] = cor & 0x00FF_FFFF;
        }
    }

    pub fn ler_pixel(&self, x: i32, y: i32) -> Option<Cor> {
        if x >= 0 && y >= 0 && (x as usize) < self.largura && (y as usize) < self.altura {
            Some(self.pixels[y as usize * self.largura + x as usize])
        } else {
            None
        }
    }

    /// Retangulo cheio, cortado nas bordas.
    pub fn retangulo(&mut self, r: Retangulo, cor: Cor) {
        let Some((x0, y0, x1, y1)) = self.recorte(r) else { return };
        let cor = cor & 0x00FF_FFFF;
        for y in y0..y1 {
            self.trecho(y, x0, x1).fill(cor);
        }
    }

    /// Moldura de `espessura` pixels por dentro de `r`.
    pub fn contorno(&mut self, r: Retangulo, cor: Cor, espessura: i32) {
        if espessura <= 0 || r.vazio() {
            return;
        }
        let e = espessura.min(r.largura).min(r.altura);
        let meio = r.altura - 2 * e;
        self.retangulo(Retangulo::novo(r.x, r.y, r.largura, e), cor);
        self.retangulo(Retangulo::novo(r.x, r.baixo() - e, r.largura, e), cor);
        self.retangulo(Retangulo::novo(r.x, r.y + e, e, meio), cor);
        self.retangulo(Retangulo::novo(r.direita() - e, r.y + e, e, meio), cor);
    }

    /// Degrade de cima para baixo: a primeira linha de `r` e `cima`, a
    /// ultima e `baixo` (mesmo que parte de `r` fique fora da superficie).
    pub fn gradiente_vertical(&mut self, r: Retangulo, cima: Cor, baixo: Cor) {
        let Some((x0, y0, x1, y1)) = self.recorte(r) else { return };
        let passos = (r.altura as i64 - 1).max(1);
        for y in y0..y1 {
            let t = (y as i64 - r.y as i64).clamp(0, passos);
            let cor = misturar(cima, baixo, (t * 255 / passos) as u32);
            self.trecho(y, x0, x1).fill(cor);
        }
    }

    /// Linha de (x0, y0) a (x1, y1), pontas incluidas (Bresenham).
    /// Horizontais e verticais viram retangulos (bem mais rapido).
    pub fn linha(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, cor: Cor) {
        if y0 == y1 {
            let (a, n) = segmento(x0, x1, self.largura);
            self.retangulo(Retangulo::novo(a, y0, n, 1), cor);
            return;
        }
        if x0 == x1 {
            let (a, n) = segmento(y0, y1, self.altura);
            self.retangulo(Retangulo::novo(x0, a, 1, n), cor);
            return;
        }
        if !self.linha_pode_aparecer(x0, y0, x1, y1) {
            return;
        }
        self.bresenham(x0 as i64, y0 as i64, x1 as i64, y1 as i64, cor & 0x00FF_FFFF);
    }

    /// A caixa que envolve a linha toca a superficie?
    fn linha_pode_aparecer(&self, x0: i32, y0: i32, x1: i32, y1: i32) -> bool {
        let toca_x = x0.max(x1) >= 0 && (x0.min(x1) as i64) < self.largura as i64;
        let toca_y = y0.max(y1) >= 0 && (y0.min(y1) as i64) < self.altura as i64;
        toca_x && toca_y
    }

    fn bresenham(&mut self, mut x: i64, mut y: i64, x1: i64, y1: i64, cor: Cor) {
        let dx = (x1 - x).abs();
        let dy = -(y1 - y).abs();
        let passo_x = if x < x1 { 1 } else { -1 };
        let passo_y = if y < y1 { 1 } else { -1 };
        let mut erro = dx + dy;
        loop {
            if x >= 0 && y >= 0 && (x as usize) < self.largura && (y as usize) < self.altura {
                self.pixels[y as usize * self.largura + x as usize] = cor;
            }
            if x == x1 && y == y1 {
                return;
            }
            let dobro = 2 * erro;
            if dobro >= dy {
                erro += dy;
                x += passo_x;
            }
            if dobro <= dx {
                erro += dx;
                y += passo_y;
            }
        }
    }

    /// Disco cheio de centro (cx, cy). Cada linha vira um trecho cheio.
    pub fn circulo_cheio(&mut self, cx: i32, cy: i32, raio: i32, cor: Cor) {
        if raio < 0 {
            return;
        }
        let r = raio as i64;
        // r * r + r arredonda melhor a borda que r * r (fica menos "quadrado")
        let limite = r * r + r;
        let Some((y0, y1)) = recortar_intervalo(cy as i64 - r, cy as i64 + r + 1, self.altura) else { return };
        let cor = cor & 0x00FF_FFFF;
        for y in y0..y1 {
            let dy = y as i64 - cy as i64;
            let meia = raiz_inteira(limite - dy * dy);
            if let Some((x0, x1)) = recortar_intervalo(cx as i64 - meia, cx as i64 + meia + 1, self.largura) {
                self.trecho(y, x0, x1).fill(cor);
            }
        }
    }

    /// Texto numa linha so (UTF-8), fundo transparente, com antialiasing.
    /// As maiusculas ficam centralizadas na faixa de 8*escala pixels a
    /// partir de `y` (a mesma da antiga fonte 8x8). escala 1 = Inter 13 px,
    /// 2 = Inter 20 px, 3 ou mais = Inter 32 px. Devolve a largura em pixels.
    pub fn texto(&mut self, x: i32, y: i32, texto: &str, cor: Cor, escala: usize) -> i32 {
        let f = fonte::por_escala(escala);
        let base = f.base_centralizada(y, 8 * escala.max(1) as i32);
        self.texto_com(x, base, texto, cor, f)
    }

    /// Texto com uma fonte escolhida e a linha de base em `base`.
    pub fn texto_com(&mut self, x: i32, base: i32, texto: &str, cor: Cor, f: &Fonte) -> i32 {
        fonte::desenhar(self, x, base, texto, cor & 0x00FF_FFFF, f)
    }

    /// Largura em pixels que texto() ocupa.
    pub fn largura_texto(texto: &str, escala: usize) -> i32 {
        fonte::por_escala(escala).largura(texto)
    }

    /// Copia a superficie inteira para `destino` com o canto em (x, y).
    pub fn copiar_para(&self, destino: &mut Superficie, x: i32, y: i32) {
        self.copiar_regiao_para(self.area(), destino, x, y);
    }

    /// Copia o pedaco `origem` desta superficie para `destino`, com o canto
    /// de cima a esquerda do pedaco indo para (x, y). Recorta nos dois lados.
    pub fn copiar_regiao_para(&self, origem: Retangulo, destino: &mut Superficie, x: i32, y: i32) {
        let pedido = origem;
        let origem = pedido.intersecao(&self.area());
        if origem.vazio() {
            return;
        }
        // O que foi cortado da origem desloca o canto no destino junto
        let x = x.saturating_add(origem.x.saturating_sub(pedido.x));
        let y = y.saturating_add(origem.y.saturating_sub(pedido.y));
        // Onde o pedaco cai no destino, recortado nas bordas dele
        let alvo = Retangulo::novo(x, y, origem.largura, origem.altura);
        let Some((x0, y0, x1, y1)) = destino.recorte(alvo) else { return };
        // Quanto o recorte no destino "comeu" da esquerda e de cima
        let sx = origem.x as usize + (x0 as i64 - x as i64) as usize;
        let sy = origem.y as usize + (y0 as i64 - y as i64) as usize;
        let largura = x1 - x0;
        for linha in 0..y1 - y0 {
            let de = (sy + linha) * self.largura + sx;
            let para = (y0 + linha) * destino.largura + x0;
            destino.pixels[para..para + largura].copy_from_slice(&self.pixels[de..de + largura]);
        }
    }
}

/// Trecho reto de a ate b (pontas incluidas) preso em [-1, limite], para
/// a conta do comprimento nunca estourar: (inicio, comprimento).
fn segmento(a: i32, b: i32, limite: usize) -> (i32, i32) {
    let limite = limite.min(i32::MAX as usize - 1) as i32;
    let inicio = a.min(b).clamp(-1, limite);
    let fim = a.max(b).clamp(-1, limite);
    (inicio, fim - inicio + 1)
}

/// Maior n com n * n <= valor (0 para valor negativo).
fn raiz_inteira(valor: i64) -> i64 {
    if valor <= 0 {
        return 0;
    }
    // Newton com inteiros: converge em poucas voltas para valores de tela
    let mut n = valor;
    let mut proximo = (n + 1) / 2;
    while proximo < n {
        n = proximo;
        proximo = (n + valor / n) / 2;
    }
    n
}

/// Copia a `area` (recortada) de `origem` para o framebuffer, nas mesmas
/// coordenadas. XRGB8888 copia linhas inteiras; outros formatos convertem
/// pixel a pixel (lento, so para nao ficar sem imagem). Termina com dsb.
pub fn apresentar(origem: &Superficie, framebuffer: &Tela, area: Retangulo) {
    let limite = Retangulo::novo(0, 0, framebuffer.largura() as i32, framebuffer.altura() as i32);
    let area = area.intersecao(&origem.area()).intersecao(&limite);
    if area.vazio() {
        return;
    }
    if framebuffer.formato() == Formato::Xrgb8888 {
        copiar_linhas(origem, framebuffer, area);
    } else {
        converter_pixels(origem, framebuffer, area);
    }
    // Garante que as escritas sairam antes de qualquer outra coisa
    unsafe { asm!("dsb sy", options(nostack, preserves_flags)) };
}

/// Caminho rapido: uma copia de memoria por linha direto na base da Tela.
fn copiar_linhas(origem: &Superficie, framebuffer: &Tela, area: Retangulo) {
    let (x, largura) = (area.x as usize, area.largura as usize);
    for y in area.y as usize..area.baixo() as usize {
        let de = &origem.pixels[y * origem.largura + x..y * origem.largura + x + largura];
        let para = (framebuffer.base() + y * framebuffer.passo() + x * 4) as *mut u32;
        // Seguranca: area foi recortada no tamanho do framebuffer, e a
        // Tela garante que [base, base + passo * altura) e memoria valida
        unsafe { ptr::copy_nonoverlapping(de.as_ptr(), para, largura) };
    }
}

/// Caminho lento para RGB565 / RGB888: a Tela codifica cada pixel.
fn converter_pixels(origem: &Superficie, framebuffer: &Tela, area: Retangulo) {
    for y in area.y as usize..area.baixo() as usize {
        for x in area.x as usize..area.direita() as usize {
            framebuffer.pixel(x, y, origem.pixels[y * origem.largura + x]);
        }
    }
}

impl fonte::Alvo for Superficie {
    fn tamanho(&self) -> (i32, i32) {
        (self.largura as i32, self.altura as i32)
    }

    fn ler(&self, x: i32, y: i32) -> u32 {
        self.pixels[y as usize * self.largura + x as usize]
    }

    fn escrever(&mut self, x: i32, y: i32, cor: u32) {
        self.pixels[y as usize * self.largura + x as usize] = cor;
    }
}
