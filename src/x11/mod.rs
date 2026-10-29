//! Servidor X11 do MXOS: programas de Linux com janela (o NetSurf, xterm,
//! programas em SDL ou GTK pelo X11) falam o protocolo X pelo soquete
//! /tmp/.X11-unix/X0 (AF_UNIX, que o kernel atende direto) e cada janela
//! X de primeiro nivel vira uma janela do MXOS (gui/apps/janela_x.rs).
//!
//! O que existe (o necessario para os programas comuns abrirem):
//!   - janelas com filhas, pixmaps, GCs, propriedades, atomos, cursores e
//!     fontes de mentira, cores TrueColor de 24 bits (XRGB)
//!   - desenho: PutImage, CopyArea, PolyFillRectangle, ClearArea, GetImage,
//!     linhas e pontos simples
//!   - eventos: Expose, teclado, mouse (e roda), Map/Configure/Property,
//!     Focus, Enter/Leave, ClientMessage (WM_DELETE_WINDOW ao fechar)
//!   - extensoes: BIG-REQUESTS (imagens grandes) e MIT-SHM (a imagem fica
//!     na memoria dividida com o programa: shm.rs, sem copiar pelo soquete)
//!
//! So clientes little-endian (todo aarch64). Tudo roda no laco principal:
//! `atender` le os pedidos de todos os clientes e responde.

pub mod teclado;

use alloc::collections::{BTreeMap, VecDeque};
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::{RefCell, UnsafeCell};

use crate::processo::unix::{self, Escuta, Ponta};
use crate::processo::shm;

const RAIZ: u32 = 0x100;
const MAPA_CORES: u32 = 0x101;
const VISUAL: u32 = 0x102;
const VISUAL_ARGB: u32 = 0x103;
/// Janela invisivel do proprio servidor: dona da selecao quando o texto
/// foi copiado no MXOS, e quem recebe o texto copiado num programa X.
const JANELA_SERVIDOR: u32 = 0x104;
const MASCARA_ID: u32 = 0x001F_FFFF;

const OP_BIG_REQUESTS: u8 = 128;
const OP_SHM: u8 = 129;
const PRIMEIRO_EVENTO_SHM: u8 = 64;

// mascaras de eventos
const KEY_PRESS: u32 = 1;
const KEY_RELEASE: u32 = 2;
const BUTTON_PRESS: u32 = 4;
const BUTTON_RELEASE: u32 = 8;
const ENTER_WINDOW: u32 = 0x10;
const LEAVE_WINDOW: u32 = 0x20;
const POINTER_MOTION: u32 = 0x40;
const BUTTON_MOTION: u32 = 0x2000;
const BUTTONS_MOTION: u32 = 0x1F00 | BUTTON_MOTION;
const EXPOSURE: u32 = 0x8000;
const STRUCTURE_NOTIFY: u32 = 0x20000;
const SUBSTRUCTURE_NOTIFY: u32 = 0x80000;
const FOCUS_CHANGE: u32 = 0x200000;
const PROPERTY_CHANGE: u32 = 0x400000;

// erros
const BAD_REQUEST: u8 = 1;
const BAD_VALUE: u8 = 2;
const BAD_WINDOW: u8 = 3;
const BAD_PIXMAP: u8 = 4;
const BAD_ATOM: u8 = 5;
const BAD_MATCH: u8 = 8;
const BAD_DRAWABLE: u8 = 9;
const BAD_ACCESS: u8 = 10;
const BAD_NAME: u8 = 15;
const BAD_IMPLEMENTATION: u8 = 17;

const ATOMOS_PREDEFINIDOS: [&str; 68] = [
    "PRIMARY", "SECONDARY", "ARC", "ATOM", "BITMAP", "CARDINAL", "COLORMAP", "CURSOR", "CUT_BUFFER0",
    "CUT_BUFFER1", "CUT_BUFFER2", "CUT_BUFFER3", "CUT_BUFFER4", "CUT_BUFFER5", "CUT_BUFFER6", "CUT_BUFFER7",
    "DRAWABLE", "FONT", "INTEGER", "PIXMAP", "POINT", "RECTANGLE", "RESOURCE_MANAGER", "RGB_COLOR_MAP",
    "RGB_BEST_MAP", "RGB_BLUE_MAP", "RGB_DEFAULT_MAP", "RGB_GRAY_MAP", "RGB_GREEN_MAP", "RGB_RED_MAP",
    "STRING", "VISUALID", "WINDOW", "WM_COMMAND", "WM_HINTS", "WM_CLIENT_MACHINE", "WM_ICON_NAME",
    "WM_ICON_SIZE", "WM_NAME", "WM_NORMAL_HINTS", "WM_SIZE_HINTS", "WM_ZOOM_HINTS", "MIN_SPACE",
    "NORM_SPACE", "MAX_SPACE", "END_SPACE", "SUPERSCRIPT_X", "SUPERSCRIPT_Y", "SUBSCRIPT_X", "SUBSCRIPT_Y",
    "UNDERLINE_POSITION", "UNDERLINE_THICKNESS", "STRIKEOUT_ASCENT", "STRIKEOUT_DESCENT", "ITALIC_ANGLE",
    "X_HEIGHT", "QUAD_WIDTH", "WEIGHT", "POINT_SIZE", "RESOLUTION", "COPYRIGHT", "NOTICE", "FONT_NAME",
    "FAMILY_NAME", "FULL_NAME", "CAP_HEIGHT", "WM_CLASS", "WM_TRANSIENT_FOR",
];
const ATOMO_WM_NAME: u32 = 39;

// ---------------------------------------------------------------------
// Recursos

/// Pixels XRGB (ou 0/1 nos pixmaps de profundidade 1).
pub struct Imagem {
    pub largura: usize,
    pub altura: usize,
    pub pixels: Vec<u32>,
}

impl Imagem {
    fn nova(largura: usize, altura: usize, cor: u32) -> Imagem {
        Imagem { largura, altura, pixels: vec![cor; largura * altura] }
    }

    fn redimensionar(&mut self, largura: usize, altura: usize, cor: u32) {
        if (largura, altura) == (self.largura, self.altura) {
            return;
        }
        let mut nova = Imagem::nova(largura, altura, cor);
        for y in 0..altura.min(self.altura) {
            let n = largura.min(self.largura);
            nova.pixels[y * largura..y * largura + n].copy_from_slice(&self.pixels[y * self.largura..y * self.largura + n]);
        }
        *self = nova;
    }

    /// Pinta o retangulo (ja recortado aqui).
    fn preencher(&mut self, x: i32, y: i32, l: i32, a: i32, cor: u32, recorte: Option<&[Ret]>) {
        let (x0, y0) = (x.max(0), y.max(0));
        let (x1, y1) = ((x + l).min(self.largura as i32), (y + a).min(self.altura as i32));
        for yy in y0..y1 {
            for xx in x0..x1 {
                if recorte.is_none_or(|r| dentro(r, xx, yy)) {
                    self.pixels[yy as usize * self.largura + xx as usize] = cor;
                }
            }
        }
    }

    fn ponto(&mut self, x: i32, y: i32, cor: u32) {
        if x >= 0 && y >= 0 && (x as usize) < self.largura && (y as usize) < self.altura {
            self.pixels[y as usize * self.largura + x as usize] = cor;
        }
    }

    fn linha(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, cor: u32) {
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let (mut x, mut y, mut erro) = (x0, y0, dx + dy);
        for _ in 0..4096 {
            self.ponto(x, y, cor);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * erro;
            if e2 >= dy {
                erro += dy;
                x += sx;
            }
            if e2 <= dx {
                erro += dx;
                y += sy;
            }
        }
    }
}

/// Retangulo de recorte (x, y, largura, altura).
type Ret = (i32, i32, i32, i32);

fn dentro(r: &[Ret], x: i32, y: i32) -> bool {
    r.iter().any(|&(rx, ry, rl, ra)| x >= rx && y >= ry && x < rx + rl && y < ry + ra)
}

struct Propriedade {
    tipo: u32,
    formato: u8,
    dados: Vec<u8>,
}

pub struct Janela {
    pai: u32,
    filhas: Vec<u32>,
    x: i32,
    y: i32,
    pub imagem: Imagem,
    profundidade: u8,
    mapeada: bool,
    sobrepor: bool,
    so_entrada: bool,
    fundo: Option<u32>,
    /// Quem quer quais eventos desta janela: (cliente, mascara).
    interesses: Vec<(u32, u32)>,
    propriedades: BTreeMap<u32, Propriedade>,
    /// Ja entregue a area de trabalho (janela de primeiro nivel).
    na_tela: bool,
    /// Mudou desde a ultima vez que a area de trabalho copiou.
    pub suja: bool,
}

struct Gc {
    frente: u32,
    fundo: u32,
    exposicoes: bool,
    recorte: Option<Vec<Ret>>,
    origem_recorte: (i32, i32),
}

enum Recurso {
    Janela(Janela),
    Pixmap(Imagem, u8),
    Gc(Gc),
    Outro,
    /// Segmento MIT-SHM preso a um id (shmid).
    Shm(u32),
}

struct Cliente {
    ponta: Ponta,
    entrada: Vec<u8>,
    pronto: bool,
    base: u32,
    seq: u16,
    grande: bool,
}

pub struct Servidor {
    escutas: Vec<Rc<RefCell<Escuta>>>,
    clientes: BTreeMap<u32, Cliente>,
    proximo_cliente: u32,
    recursos: BTreeMap<u32, (u32, Recurso)>,
    atomos: Vec<String>,
    largura: u16,
    altura: u16,
    foco: u32,
    /// Ultima posicao do ponteiro na tela e a janela sob ele.
    ponteiro: (i32, i32),
    sob_ponteiro: u32,
    botoes: u16,
    modificadores: u16,
    donos_selecao: BTreeMap<u32, u32>,
    /// Ultima versao da area de transferencia do MXOS ja vista.
    versao_transferencia: u64,
}

struct Global(UnsafeCell<Option<Servidor>>);

// So o laco principal mexe (como a tabela de processos).
unsafe impl Sync for Global {}

static SERVIDOR: Global = Global(UnsafeCell::new(None));

fn servidor() -> Option<&'static mut Servidor> {
    unsafe { (*SERVIDOR.0.get()).as_mut() }
}

/// Liga o servidor (uma vez, com a area de trabalho de pe).
pub fn iniciar(largura: usize, altura: usize) {
    let s = Servidor {
        escutas: vec![unix::servir("@/tmp/.X11-unix/X0"), unix::servir("/tmp/.X11-unix/X0")],
        clientes: BTreeMap::new(),
        proximo_cliente: 1,
        recursos: BTreeMap::new(),
        atomos: ATOMOS_PREDEFINIDOS.iter().map(|&a| String::from(a)).collect(),
        largura: largura as u16,
        altura: altura as u16,
        foco: RAIZ,
        ponteiro: (0, 0),
        sob_ponteiro: RAIZ,
        botoes: 0,
        modificadores: 0,
        donos_selecao: BTreeMap::new(),
        versao_transferencia: 0,
    };
    unsafe { *SERVIDOR.0.get() = Some(s) };
    let s = servidor().unwrap();
    let j = Janela {
        pai: RAIZ,
        filhas: Vec::new(),
        x: -1,
        y: -1,
        imagem: Imagem::nova(1, 1, 0),
        profundidade: 24,
        mapeada: false,
        sobrepor: true,
        so_entrada: true,
        fundo: None,
        interesses: Vec::new(),
        propriedades: BTreeMap::new(),
        na_tela: true,
        suja: false,
    };
    s.recursos.insert(JANELA_SERVIDOR, (0, Recurso::Janela(j)));
}

/// Atende quem conectou e os pedidos de todos. Devolve true se algo mudou.
pub fn atender() -> bool {
    let Some(s) = servidor() else { return false };
    let mut mexeu = false;
    for k in 0..s.escutas.len() {
        while let Some(ponta) = s.escutas[k].borrow_mut().fila.pop_front() {
            let id = s.proximo_cliente;
            s.proximo_cliente += 1;
            let base = id << 21;
            s.clientes.insert(id, Cliente { ponta, entrada: Vec::new(), pronto: false, base, seq: 0, grande: false });
            mexeu = true;
        }
    }
    let ids: Vec<u32> = s.clientes.keys().copied().collect();
    for id in ids {
        mexeu |= s.atender_cliente(id);
    }
    // copiaram no MXOS: a selecao CLIPBOARD passa a ser do servidor
    let versao = crate::gui::transferencia::versao();
    if versao != s.versao_transferencia {
        s.versao_transferencia = versao;
        let clipboard = s.atomo_de("CLIPBOARD");
        s.tomar_selecao(clipboard);
        mexeu = true;
    }
    mexeu
}

// ---------------------------------------------------------------------
// Leitura de numeros

fn u16_em(b: &[u8], i: usize) -> u16 {
    b.get(i..i + 2).map_or(0, |v| u16::from_le_bytes([v[0], v[1]]))
}

fn i16_em(b: &[u8], i: usize) -> i32 {
    u16_em(b, i) as i16 as i32
}

fn u32_em(b: &[u8], i: usize) -> u32 {
    b.get(i..i + 4).map_or(0, |v| u32::from_le_bytes([v[0], v[1], v[2], v[3]]))
}

fn preencher4(v: &mut Vec<u8>) {
    while v.len() % 4 != 0 {
        v.push(0);
    }
}

fn agora_ms() -> u32 {
    (crate::cpu::contador() / (crate::cpu::frequencia_timer() / 1000).max(1)) as u32
}

impl Servidor {
    fn atender_cliente(&mut self, id: u32) -> bool {
        let Some(c) = self.clientes.get_mut(&id) else { return false };
        let (dados, _) = c.ponta.entrada.borrow_mut().ler(usize::MAX);
        let fechou = c.ponta.desligada() && dados.is_empty();
        c.entrada.extend_from_slice(&dados);
        let mut mexeu = !dados.is_empty();
        if !c.pronto {
            if !self.apresentar(id) {
                if fechou {
                    self.desligar(id);
                }
                return mexeu;
            }
            mexeu = true;
        }
        loop {
            let Some(c) = self.clientes.get_mut(&id) else { return true };
            let b = &c.entrada;
            if b.len() < 4 {
                break;
            }
            let mut tamanho = u16_em(b, 2) as usize * 4;
            let mut cabecalho_grande = false;
            if tamanho == 0 && c.grande {
                if b.len() < 8 {
                    break;
                }
                tamanho = u32_em(b, 4) as usize * 4;
                cabecalho_grande = true;
            }
            if tamanho < 4 {
                // pedido estragado: o Xorg desliga o cliente
                self.desligar(id);
                return true;
            }
            if b.len() < tamanho {
                break;
            }
            let mut pedido: Vec<u8> = c.entrada.drain(..tamanho).collect();
            if cabecalho_grande {
                // tira o tamanho de 32 bits: os campos ficam onde sempre estao
                pedido.drain(4..8);
            }
            c.seq = c.seq.wrapping_add(1);
            self.pedido(id, &pedido);
            mexeu = true;
        }
        let fechou = self.clientes.get(&id).is_some_and(|c| c.ponta.desligada() && c.ponta.entrada.borrow().dados.is_empty());
        if fechou {
            self.desligar(id);
        }
        mexeu
    }

    /// A conexao: o cliente manda a ordem dos bytes, a versao e a
    /// autorizacao (ignorada); o servidor responde com a tela.
    fn apresentar(&mut self, id: u32) -> bool {
        let c = self.clientes.get_mut(&id).unwrap();
        let b = &c.entrada;
        if b.len() < 12 {
            return false;
        }
        let nome = u16_em(b, 6) as usize;
        let dados = u16_em(b, 8) as usize;
        let total = 12 + nome.div_ceil(4) * 4 + dados.div_ceil(4) * 4;
        if b.len() < total {
            return false;
        }
        let little = b[0] == b'l';
        c.entrada.drain(..total);
        if !little {
            let motivo = b"MXOS: so clientes little-endian";
            let mut r = vec![0u8, motivo.len() as u8, 11, 0, 0, 0];
            let mut texto = Vec::from(&motivo[..]);
            preencher4(&mut texto);
            r.extend_from_slice(&((texto.len() / 4) as u16).to_le_bytes());
            r.extend_from_slice(&texto);
            c.ponta.saida.borrow_mut().escrever(&r);
            return false;
        }
        c.pronto = true;
        let base = c.base;
        let r = self.configuracao(base);
        self.enviar(id, &r);
        true
    }

    fn configuracao(&self, base: u32) -> Vec<u8> {
        let fornecedor = b"MXOS";
        let mut d = Vec::new();
        d.extend_from_slice(&11u32.to_le_bytes()); // release
        d.extend_from_slice(&base.to_le_bytes());
        d.extend_from_slice(&MASCARA_ID.to_le_bytes());
        d.extend_from_slice(&256u32.to_le_bytes()); // motion buffer
        d.extend_from_slice(&(fornecedor.len() as u16).to_le_bytes());
        d.extend_from_slice(&0xFFFFu16.to_le_bytes()); // tamanho maximo de pedido
        let formatos: [(u8, u8); 4] = [(1, 1), (8, 8), (24, 32), (32, 32)];
        d.extend_from_slice(&[1, formatos.len() as u8, 0, 0, 32, 32, teclado::PRIMEIRO, teclado::ULTIMO]);
        d.extend_from_slice(&[0; 4]);
        d.extend_from_slice(fornecedor);
        preencher4(&mut d);
        for (prof, bpp) in formatos {
            d.extend_from_slice(&[prof, bpp, 32, 0, 0, 0, 0, 0]);
        }
        // a tela
        d.extend_from_slice(&RAIZ.to_le_bytes());
        d.extend_from_slice(&MAPA_CORES.to_le_bytes());
        d.extend_from_slice(&0x00FF_FFFFu32.to_le_bytes()); // branco
        d.extend_from_slice(&0u32.to_le_bytes()); // preto
        d.extend_from_slice(&0u32.to_le_bytes()); // mascaras atuais
        d.extend_from_slice(&self.largura.to_le_bytes());
        d.extend_from_slice(&self.altura.to_le_bytes());
        d.extend_from_slice(&((self.largura as u32 * 254 / 960) as u16).to_le_bytes()); // 96 dpi
        d.extend_from_slice(&((self.altura as u32 * 254 / 960) as u16).to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&VISUAL.to_le_bytes());
        d.extend_from_slice(&[0, 0, 24, 3]); // backing, save-unders, profundidade, 3 profundidades
        let visual = |id: u32| {
            let mut v = Vec::new();
            v.extend_from_slice(&id.to_le_bytes());
            v.extend_from_slice(&[4, 8]); // TrueColor, 8 bits por cor
            v.extend_from_slice(&256u16.to_le_bytes());
            v.extend_from_slice(&0x00FF_0000u32.to_le_bytes());
            v.extend_from_slice(&0x0000_FF00u32.to_le_bytes());
            v.extend_from_slice(&0x0000_00FFu32.to_le_bytes());
            v.extend_from_slice(&[0; 4]);
            v
        };
        for (prof, vis) in [(24u8, Some(VISUAL)), (32, Some(VISUAL_ARGB)), (1, None)] {
            d.extend_from_slice(&[prof, 0]);
            d.extend_from_slice(&(vis.is_some() as u16).to_le_bytes());
            d.extend_from_slice(&[0; 4]);
            if let Some(v) = vis {
                d.extend(visual(v));
            }
        }
        let mut r = vec![1u8, 0];
        r.extend_from_slice(&11u16.to_le_bytes());
        r.extend_from_slice(&0u16.to_le_bytes());
        r.extend_from_slice(&((d.len() / 4) as u16).to_le_bytes());
        r.extend(d);
        r
    }

    fn enviar(&mut self, id: u32, bytes: &[u8]) {
        if let Some(c) = self.clientes.get(&id) {
            // um cliente que nao le (canal cheio) perde o resto; nao trava
            // o servidor inteiro por ele
            c.ponta.saida.borrow_mut().escrever(bytes);
        }
    }

    fn seq(&self, id: u32) -> u16 {
        self.clientes.get(&id).map_or(0, |c| c.seq)
    }

    /// Resposta: `dado` vai no byte 1, `corpo` e o que vem depois dos 8
    /// bytes do cabecalho (completado ate 24 e ate multiplo de 4).
    fn responder(&mut self, id: u32, dado: u8, mut corpo: Vec<u8>) {
        if corpo.len() < 24 {
            corpo.resize(24, 0);
        }
        preencher4(&mut corpo);
        let mut r = Vec::with_capacity(8 + corpo.len());
        r.push(1);
        r.push(dado);
        r.extend_from_slice(&self.seq(id).to_le_bytes());
        r.extend_from_slice((((corpo.len() - 24) / 4) as u32).to_le_bytes().as_slice());
        r.extend(corpo);
        self.enviar(id, &r);
    }

    fn erro(&mut self, id: u32, codigo: u8, valor: u32, maior: u8) {
        let mut e = [0u8; 32];
        e[1] = codigo;
        e[2..4].copy_from_slice(&self.seq(id).to_le_bytes());
        e[4..8].copy_from_slice(&valor.to_le_bytes());
        e[10] = maior;
        self.enviar(id, &e);
    }

    /// Evento de 32 bytes (o numero de sequencia e posto aqui).
    fn evento(&mut self, id: u32, mut e: [u8; 32]) {
        let seq = self.seq(id);
        e[2..4].copy_from_slice(&seq.to_le_bytes());
        self.enviar(id, &e);
    }

    /// Manda `e` a quem escolheu `mascara` na janela.
    fn evento_para_interessados(&mut self, janela: u32, mascara: u32, e: [u8; 32]) {
        let interessados: Vec<u32> = match self.janela(janela) {
            Some(j) => j.interesses.iter().filter(|(_, m)| m & mascara != 0).map(|(c, _)| *c).collect(),
            None => return,
        };
        for c in interessados {
            self.evento(c, e);
        }
    }

    fn desligar(&mut self, id: u32) {
        let Some(c) = self.clientes.remove(&id) else { return };
        drop(c);
        // os recursos dele somem (as janelas da tela fecham sozinhas)
        let meus: Vec<u32> = self.recursos.iter().filter(|(_, (dono, _))| *dono == id).map(|(r, _)| *r).collect();
        for r in meus {
            if let Some((_, Recurso::Shm(shmid))) = self.recursos.get(&r) {
                shm::soltar(*shmid);
            }
            self.destruir(r);
        }
        for j in self.recursos.values_mut() {
            if let Recurso::Janela(j) = &mut j.1 {
                j.interesses.retain(|(c, _)| *c != id);
            }
        }
        unix::avisar();
    }

    fn janela(&self, id: u32) -> Option<&Janela> {
        match self.recursos.get(&id) {
            Some((_, Recurso::Janela(j))) => Some(j),
            _ => None,
        }
    }

    fn janela_mut(&mut self, id: u32) -> Option<&mut Janela> {
        match self.recursos.get_mut(&id) {
            Some((_, Recurso::Janela(j))) => Some(j),
            _ => None,
        }
    }

    /// A janela de primeiro nivel (filha da raiz) que contem `id`.
    fn primeiro_nivel(&self, mut id: u32) -> Option<u32> {
        for _ in 0..64 {
            let j = self.janela(id)?;
            if j.pai == RAIZ {
                return Some(id);
            }
            id = j.pai;
        }
        None
    }

    fn sujar(&mut self, drawable: u32) {
        if let Some(topo) = self.primeiro_nivel(drawable) {
            if let Some(j) = self.janela_mut(topo) {
                j.suja = true;
            }
        }
    }

    /// Posicao de `id` na tela.
    fn absoluta(&self, mut id: u32) -> (i32, i32) {
        let (mut x, mut y) = (0, 0);
        for _ in 0..64 {
            let Some(j) = self.janela(id) else { break };
            x += j.x;
            y += j.y;
            id = j.pai;
        }
        (x, y)
    }

    fn destruir(&mut self, id: u32) {
        let Some((_, recurso)) = self.recursos.remove(&id) else { return };
        if let Recurso::Janela(j) = recurso {
            for filha in j.filhas {
                self.destruir(filha);
            }
            if let Some(pai) = self.janela_mut(j.pai) {
                pai.filhas.retain(|&f| f != id);
            }
            let mut e = [0u8; 32];
            e[0] = 17; // DestroyNotify
            e[4..8].copy_from_slice(&id.to_le_bytes());
            e[8..12].copy_from_slice(&id.to_le_bytes());
            for (c, m) in j.interesses {
                if m & STRUCTURE_NOTIFY != 0 {
                    self.evento(c, e);
                }
            }
            if self.foco == id {
                self.foco = RAIZ;
            }
            if self.sob_ponteiro == id {
                self.sob_ponteiro = RAIZ;
            }
            self.sujar(j.pai);
        }
    }

    // -----------------------------------------------------------------
    // Pedidos

    fn pedido(&mut self, id: u32, p: &[u8]) {
        let op = p[0];
        match op {
            1 => self.criar_janela(id, p),
            2 => {
                let w = u32_em(p, 4);
                if self.janela(w).is_none() {
                    return self.erro(id, BAD_WINDOW, w, op);
                }
                self.atributos(id, w, u32_em(p, 8), &p[12..]);
            }
            3 => self.pegar_atributos(id, u32_em(p, 4)),
            4 => {
                let w = u32_em(p, 4);
                if w != RAIZ {
                    self.destruir(w);
                }
            }
            5 => {
                let filhas = self.janela(u32_em(p, 4)).map(|j| j.filhas.clone()).unwrap_or_default();
                for f in filhas {
                    self.destruir(f);
                }
            }
            6 | 7 | 11 | 13 | 41 | 100 | 102 | 104 | 105 | 107 | 110 | 111 | 112 | 113 | 114 | 115 | 127 => {} // sem efeito aqui
            8 => self.mapear(u32_em(p, 4), true),
            9 => {
                let filhas = self.janela(u32_em(p, 4)).map(|j| j.filhas.clone()).unwrap_or_default();
                for f in filhas {
                    self.mapear(f, true);
                }
            }
            10 => self.mapear(u32_em(p, 4), false),
            12 => self.configurar(u32_em(p, 4), u16_em(p, 8) as u32, &p[12..]),
            14 => self.geometria(id, u32_em(p, 4)),
            15 => self.arvore(id, u32_em(p, 4)),
            16 => self.atomo(id, p),
            17 => {
                let a = u32_em(p, 4);
                match self.atomos.get((a as usize).wrapping_sub(1)).cloned() {
                    Some(nome) => {
                        let mut c = Vec::new();
                        c.extend_from_slice(&(nome.len() as u16).to_le_bytes());
                        c.extend_from_slice(&[0; 22]);
                        c.extend_from_slice(nome.as_bytes());
                        self.responder(id, 0, c);
                    }
                    None => self.erro(id, BAD_ATOM, a, op),
                }
            }
            18 => self.mudar_propriedade(id, p),
            19 => {
                let (w, a) = (u32_em(p, 4), u32_em(p, 8));
                if let Some(j) = self.janela_mut(w) {
                    if j.propriedades.remove(&a).is_some() {
                        self.avisar_propriedade(w, a, 1);
                    }
                }
            }
            20 => self.pegar_propriedade(id, p),
            21 => {
                let atomos: Vec<u32> = self.janela(u32_em(p, 4)).map(|j| j.propriedades.keys().copied().collect()).unwrap_or_default();
                let mut c = Vec::new();
                c.extend_from_slice(&(atomos.len() as u16).to_le_bytes());
                c.extend_from_slice(&[0; 22]);
                for a in atomos {
                    c.extend_from_slice(&a.to_le_bytes());
                }
                self.responder(id, 0, c);
            }
            22 => {
                let (dono, selecao) = (u32_em(p, 4), u32_em(p, 8));
                let velho = self.donos_selecao.insert(selecao, dono);
                if let Some(v) = velho.filter(|&v| v != dono) {
                    self.avisar_perda(v, selecao);
                }
                // o texto copiado num programa X vai para o MXOS tambem
                if selecao == self.atomo_de("CLIPBOARD") && dono != 0 {
                    let (utf8, prop) = (self.atomo_de("UTF8_STRING"), self.atomo_de("MXOS_SELECAO"));
                    self.pedir_conversao(dono, selecao, utf8, JANELA_SERVIDOR, prop);
                }
            }
            23 => {
                let dono = self.donos_selecao.get(&u32_em(p, 4)).copied().unwrap_or(0);
                self.responder(id, 0, Vec::from(dono.to_le_bytes()));
            }
            24 => self.converter_selecao(id, p),
            25 => self.mandar_evento(p),
            26 | 31 => self.responder(id, 0, Vec::new()), // GrabPointer/GrabKeyboard: Success
            27 | 28 | 29 | 30 | 32 | 33 | 34 | 35 | 36 | 37 | 42 => {
                if op == 42 {
                    let f = u32_em(p, 4);
                    self.foco = if f <= 1 { RAIZ } else { f };
                }
            }
            38 => self.pegar_ponteiro(id, u32_em(p, 4)),
            39 => {
                let mut c = vec![0u8; 24];
                c[..4].copy_from_slice(&0u32.to_le_bytes());
                self.responder(id, 0, c);
            }
            40 => self.traduzir(id, p),
            43 => {
                let foco = self.foco;
                self.responder(id, 1, Vec::from(foco.to_le_bytes()));
            }
            44 => self.responder(id, 0, vec![0; 32]),
            45 | 46 | 54 | 60 | 79 | 95 | 96 => {
                let r = u32_em(p, 4);
                if matches!(op, 54 | 60 | 79 | 95 | 46) {
                    self.recursos.remove(&r);
                } else if op == 45 {
                    self.recursos.insert(r, (id, Recurso::Outro));
                }
            }
            47 => self.erro(id, 7, u32_em(p, 4), op), // QueryFont: BadFont
            49 => self.responder(id, 0, Vec::new()),  // ListFonts: nenhuma
            50 => self.responder(id, 0, Vec::new()),  // ListFontsWithInfo: fim da lista
            52 => self.responder(id, 0, Vec::new()),  // GetFontPath
            53 => {
                let (prof, pid) = (p[1], u32_em(p, 4));
                let (l, a) = (u16_em(p, 12) as usize, u16_em(p, 14) as usize);
                if l == 0 || a == 0 || l * a > 16 << 20 {
                    return self.erro(id, BAD_VALUE, 0, op);
                }
                self.recursos.insert(pid, (id, Recurso::Pixmap(Imagem::nova(l, a, 0), prof)));
            }
            55 => {
                let mut gc = Gc { frente: 0, fundo: 0x00FF_FFFF, exposicoes: true, recorte: None, origem_recorte: (0, 0) };
                valores_gc(&mut gc, u32_em(p, 12), &p[16..]);
                self.recursos.insert(u32_em(p, 4), (id, Recurso::Gc(gc)));
            }
            56 => {
                if let Some((_, Recurso::Gc(gc))) = self.recursos.get_mut(&u32_em(p, 4)) {
                    valores_gc(gc, u32_em(p, 8), &p[12..]);
                }
            }
            57 | 58 => {}
            59 => {
                if let Some((_, Recurso::Gc(gc))) = self.recursos.get_mut(&u32_em(p, 4)) {
                    let mut r = Vec::new();
                    for k in (12..p.len()).step_by(8) {
                        if k + 8 <= p.len() {
                            r.push((i16_em(p, k), i16_em(p, k + 2), u16_em(p, k + 4) as i32, u16_em(p, k + 6) as i32));
                        }
                    }
                    gc.origem_recorte = (i16_em(p, 8), i16_em(p, 10));
                    gc.recorte = Some(r);
                }
            }
            61 => self.limpar_area(p),
            62 | 63 => self.copiar_area(id, p),
            64 | 65 | 66 | 67 => self.linhas(p),
            68 | 69 | 71 | 74 | 75 | 76 | 77 => {} // arcos, poligonos e texto: ignorados
            70 => self.retangulos(p),
            72 => self.por_imagem(id, p),
            73 => self.pegar_imagem(id, p),
            78 => {
                self.recursos.insert(u32_em(p, 4), (id, Recurso::Outro));
            }
            83 => {
                let mut c = Vec::new();
                c.extend_from_slice(&1u16.to_le_bytes());
                c.extend_from_slice(&[0; 22]);
                c.extend_from_slice(&MAPA_CORES.to_le_bytes());
                self.responder(id, 0, c);
            }
            84 => {
                let (r, g, b) = (u16_em(p, 8), u16_em(p, 10), u16_em(p, 12));
                let pixel = (r as u32 >> 8) << 16 | (g as u32 >> 8) << 8 | b as u32 >> 8;
                let mut c = Vec::new();
                for v in [r & 0xFF00 | r >> 8, g & 0xFF00 | g >> 8, b & 0xFF00 | b >> 8, 0] {
                    c.extend_from_slice(&v.to_le_bytes());
                }
                c.extend_from_slice(&pixel.to_le_bytes());
                self.responder(id, 0, c);
            }
            85 | 92 => self.cor_por_nome(id, p),
            86 => self.erro(id, BAD_ACCESS, 0, op), // AllocColorCells
            88 | 89 => {}
            91 => {
                let mut c = Vec::new();
                let n = (p.len() - 8) / 4;
                c.extend_from_slice(&(n as u16).to_le_bytes());
                c.extend_from_slice(&[0; 22]);
                for k in 0..n {
                    let px = u32_em(p, 8 + 4 * k);
                    for v in [(px >> 16) & 0xFF, (px >> 8) & 0xFF, px & 0xFF] {
                        c.extend_from_slice(&((v * 0x101) as u16).to_le_bytes());
                    }
                    c.extend_from_slice(&[0, 0]);
                }
                self.responder(id, 0, c);
            }
            93 | 94 => {
                self.recursos.insert(u32_em(p, 4), (id, Recurso::Outro));
            }
            97 => {
                let mut c = Vec::new();
                c.extend_from_slice(&u16_em(p, 8).to_le_bytes());
                c.extend_from_slice(&u16_em(p, 10).to_le_bytes());
                self.responder(id, 0, c);
            }
            98 => self.extensao(id, p),
            99 => {
                let mut c = vec![0u8; 24];
                for nome in [&b"BIG-REQUESTS"[..], b"MIT-SHM"] {
                    c.push(nome.len() as u8);
                    c.extend_from_slice(nome);
                }
                self.responder(id, 2, c);
            }
            101 => self.mapa_teclado(id, p[4], p[5]),
            103 => {
                let mut c = vec![0u8; 44];
                c[4] = 50;
                c[5] = 50;
                self.responder(id, 1, c);
            }
            106 => {
                let mut c = Vec::new();
                for v in [2u16, 1, 4] {
                    c.extend_from_slice(&v.to_le_bytes());
                }
                self.responder(id, 0, c);
            }
            108 => self.responder(id, 0, Vec::new()),
            116 | 118 => self.responder(id, 0, Vec::new()),
            117 => {
                let mut c = vec![0u8; 24];
                c.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 0]);
                self.responder(id, 7, c);
            }
            119 => {
                let mut c = vec![0u8; 24];
                for m in teclado::MODIFICADORES {
                    c.extend_from_slice(&m);
                }
                self.responder(id, 2, c);
            }
            OP_BIG_REQUESTS => {
                if let Some(c) = self.clientes.get_mut(&id) {
                    c.grande = true;
                }
                self.responder(id, 0, Vec::from((1u32 << 26).to_le_bytes()));
            }
            OP_SHM => self.pedido_shm(id, p),
            _ => self.erro(id, BAD_REQUEST, 0, op),
        }
    }

    fn criar_janela(&mut self, id: u32, p: &[u8]) {
        let (wid, pai) = (u32_em(p, 4), u32_em(p, 8));
        if pai != RAIZ && self.janela(pai).is_none() {
            return self.erro(id, BAD_WINDOW, pai, 1);
        }
        let (l, a) = (u16_em(p, 16).max(1) as usize, u16_em(p, 18).max(1) as usize);
        if l * a > 16 << 20 {
            return self.erro(id, BAD_VALUE, 0, 1);
        }
        let classe = u16_em(p, 22);
        let profundidade = if p[1] == 0 { 24 } else { p[1] };
        let j = Janela {
            pai,
            filhas: Vec::new(),
            x: i16_em(p, 12),
            y: i16_em(p, 14),
            imagem: Imagem::nova(if classe == 2 { 1 } else { l }, if classe == 2 { 1 } else { a }, 0),
            profundidade,
            mapeada: false,
            sobrepor: false,
            so_entrada: classe == 2,
            fundo: None,
            interesses: Vec::new(),
            propriedades: BTreeMap::new(),
            na_tela: false,
            suja: true,
        };
        self.recursos.insert(wid, (id, Recurso::Janela(j)));
        if let Some(pj) = self.janela_mut(pai) {
            pj.filhas.push(wid);
        }
        self.atributos(id, wid, u32_em(p, 28), &p[32..]);
        if let Some(cor) = self.janela(wid).and_then(|j| j.fundo) {
            let j = self.janela_mut(wid).unwrap();
            j.imagem.pixels.fill(cor);
        }
        // CreateNotify para quem olha a raiz/o pai (gerenciadores de janela)
        let mut e = [0u8; 32];
        e[0] = 16;
        e[4..8].copy_from_slice(&pai.to_le_bytes());
        e[8..12].copy_from_slice(&wid.to_le_bytes());
        e[12..14].copy_from_slice(&(i16_em(p, 12) as i16).to_le_bytes());
        e[14..16].copy_from_slice(&(i16_em(p, 14) as i16).to_le_bytes());
        e[16..18].copy_from_slice(&(l as u16).to_le_bytes());
        e[18..20].copy_from_slice(&(a as u16).to_le_bytes());
        self.evento_para_interessados(pai, SUBSTRUCTURE_NOTIFY, e);
    }

    /// Valores de CreateWindow/ChangeWindowAttributes (um u32 por bit da mascara).
    fn atributos(&mut self, cliente: u32, w: u32, mascara: u32, valores: &[u8]) {
        let mut k = 0;
        let Some(j) = self.janela_mut(w) else { return };
        for bit in 0..15 {
            if mascara & (1 << bit) == 0 {
                continue;
            }
            let v = u32_em(valores, k);
            k += 4;
            match bit {
                0 if v == 0 => j.fundo = None,
                1 => j.fundo = Some(v & 0x00FF_FFFF),
                9 => j.sobrepor = v != 0,
                11 => {
                    j.interesses.retain(|(c, _)| *c != cliente);
                    if v != 0 {
                        j.interesses.push((cliente, v));
                    }
                }
                _ => {}
            }
        }
    }

    fn pegar_atributos(&mut self, id: u32, w: u32) {
        let Some(j) = self.janela(w) else { return self.erro(id, BAD_WINDOW, w, 3) };
        let todas = j.interesses.iter().fold(0, |m, (_, v)| m | v);
        let minha = j.interesses.iter().find(|(c, _)| *c == id).map_or(0, |(_, v)| *v);
        let mut c = Vec::new();
        c.extend_from_slice(&VISUAL.to_le_bytes());
        c.extend_from_slice(&(if j.so_entrada { 2u16 } else { 1 }).to_le_bytes());
        c.extend_from_slice(&[1, 1]);
        c.extend_from_slice(&u32::MAX.to_le_bytes());
        c.extend_from_slice(&0u32.to_le_bytes());
        c.extend_from_slice(&[0, 1, if j.mapeada { 2 } else { 0 }, j.sobrepor as u8]);
        c.extend_from_slice(&MAPA_CORES.to_le_bytes());
        c.extend_from_slice(&todas.to_le_bytes());
        c.extend_from_slice(&minha.to_le_bytes());
        c.extend_from_slice(&[0; 4]);
        self.responder(id, 0, c);
    }

    fn mapear(&mut self, w: u32, mapa: bool) {
        let Some(j) = self.janela_mut(w) else { return };
        if j.mapeada == mapa {
            return;
        }
        j.mapeada = mapa;
        let (pai, sobrepor, l, a) = (j.pai, j.sobrepor, j.imagem.largura, j.imagem.altura);
        let mut e = [0u8; 32];
        e[0] = if mapa { 19 } else { 18 }; // MapNotify / UnmapNotify
        e[4..8].copy_from_slice(&w.to_le_bytes());
        e[8..12].copy_from_slice(&w.to_le_bytes());
        e[12] = sobrepor as u8;
        self.evento_para_interessados(w, STRUCTURE_NOTIFY, e);
        e[4..8].copy_from_slice(&pai.to_le_bytes());
        self.evento_para_interessados(pai, SUBSTRUCTURE_NOTIFY, e);
        if mapa {
            self.expor(w, 0, 0, l, a);
            // as filhas ja mapeadas aparecem junto
            let filhas = self.janela(w).map(|j| j.filhas.clone()).unwrap_or_default();
            for f in filhas {
                if let Some(fj) = self.janela(f) {
                    if fj.mapeada {
                        let (fl, fa) = (fj.imagem.largura, fj.imagem.altura);
                        self.expor(f, 0, 0, fl, fa);
                    }
                }
            }
        }
        self.sujar(w);
    }

    fn expor(&mut self, w: u32, x: usize, y: usize, l: usize, a: usize) {
        let mut e = [0u8; 32];
        e[0] = 12;
        e[4..8].copy_from_slice(&w.to_le_bytes());
        e[8..10].copy_from_slice(&(x as u16).to_le_bytes());
        e[10..12].copy_from_slice(&(y as u16).to_le_bytes());
        e[12..14].copy_from_slice(&(l as u16).to_le_bytes());
        e[14..16].copy_from_slice(&(a as u16).to_le_bytes());
        self.evento_para_interessados(w, EXPOSURE, e);
    }

    fn configurar(&mut self, w: u32, mascara: u32, valores: &[u8]) {
        let Some(j) = self.janela_mut(w) else { return };
        let mut k = 0;
        let (mut l, mut a) = (j.imagem.largura, j.imagem.altura);
        for bit in 0..7 {
            if mascara & (1 << bit) == 0 {
                continue;
            }
            let v = u32_em(valores, k);
            k += 4;
            match bit {
                0 => j.x = v as i16 as i32,
                1 => j.y = v as i16 as i32,
                2 => l = (v as usize).clamp(1, 8192),
                3 => a = (v as usize).clamp(1, 8192),
                _ => {}
            }
        }
        if !j.so_entrada {
            let cor = j.fundo.unwrap_or(0);
            j.imagem.redimensionar(l, a, cor);
        }
        let (x, y, pai) = (j.x, j.y, j.pai);
        let mut e = [0u8; 32];
        e[0] = 22; // ConfigureNotify
        e[4..8].copy_from_slice(&w.to_le_bytes());
        e[8..12].copy_from_slice(&w.to_le_bytes());
        e[16..18].copy_from_slice(&(x as i16).to_le_bytes());
        e[18..20].copy_from_slice(&(y as i16).to_le_bytes());
        e[20..22].copy_from_slice(&(l as u16).to_le_bytes());
        e[22..24].copy_from_slice(&(a as u16).to_le_bytes());
        self.evento_para_interessados(w, STRUCTURE_NOTIFY, e);
        e[4..8].copy_from_slice(&pai.to_le_bytes());
        self.evento_para_interessados(pai, SUBSTRUCTURE_NOTIFY, e);
        self.sujar(w);
    }

    fn geometria(&mut self, id: u32, d: u32) {
        let (prof, x, y, l, a) = match self.recursos.get(&d) {
            Some((_, Recurso::Janela(j))) => (j.profundidade, j.x, j.y, j.imagem.largura, j.imagem.altura),
            Some((_, Recurso::Pixmap(img, prof))) => (*prof, 0, 0, img.largura, img.altura),
            _ if d == RAIZ => (24, 0, 0, self.largura as usize, self.altura as usize),
            _ => return self.erro(id, BAD_DRAWABLE, d, 14),
        };
        let mut c = Vec::new();
        c.extend_from_slice(&RAIZ.to_le_bytes());
        c.extend_from_slice(&(x as i16).to_le_bytes());
        c.extend_from_slice(&(y as i16).to_le_bytes());
        c.extend_from_slice(&(l as u16).to_le_bytes());
        c.extend_from_slice(&(a as u16).to_le_bytes());
        self.responder(id, prof, c);
    }

    fn arvore(&mut self, id: u32, w: u32) {
        let (pai, filhas) = if w == RAIZ {
            let topo: Vec<u32> = self.recursos.iter().filter_map(|(r, (_, x))| match x {
                Recurso::Janela(j) if j.pai == RAIZ => Some(*r),
                _ => None,
            }).collect();
            (0, topo)
        } else {
            match self.janela(w) {
                Some(j) => (j.pai, j.filhas.clone()),
                None => return self.erro(id, BAD_WINDOW, w, 15),
            }
        };
        let mut c = Vec::new();
        c.extend_from_slice(&RAIZ.to_le_bytes());
        c.extend_from_slice(&pai.to_le_bytes());
        c.extend_from_slice(&(filhas.len() as u16).to_le_bytes());
        c.extend_from_slice(&[0; 14]);
        for f in filhas {
            c.extend_from_slice(&f.to_le_bytes());
        }
        self.responder(id, 0, c);
    }

    fn atomo(&mut self, id: u32, p: &[u8]) {
        let n = u16_em(p, 4) as usize;
        let nome = String::from_utf8_lossy(p.get(8..8 + n).unwrap_or(&[])).into_owned();
        let atomo = match self.atomos.iter().position(|a| *a == nome) {
            Some(i) => i as u32 + 1,
            None if p[1] != 0 => 0,
            None => {
                self.atomos.push(nome);
                self.atomos.len() as u32
            }
        };
        self.responder(id, 0, Vec::from(atomo.to_le_bytes()));
    }

    fn mudar_propriedade(&mut self, id: u32, p: &[u8]) {
        let (modo, w, atomo, tipo, formato) = (p[1], u32_em(p, 4), u32_em(p, 8), u32_em(p, 12), p[16]);
        let n = u32_em(p, 20) as usize * (formato as usize / 8);
        let dados = p.get(24..24 + n).unwrap_or(&[]).to_vec();
        if !matches!(formato, 8 | 16 | 32) {
            return self.erro(id, BAD_VALUE, formato as u32, 18);
        }
        let Some(j) = self.janela_mut(w) else { return self.erro(id, BAD_WINDOW, w, 18) };
        match (modo, j.propriedades.get_mut(&atomo)) {
            (1, Some(prop)) => {
                let mut novo = dados;
                novo.extend_from_slice(&prop.dados);
                prop.dados = novo;
            }
            (2, Some(prop)) => prop.dados.extend_from_slice(&dados),
            _ => {
                j.propriedades.insert(atomo, Propriedade { tipo, formato, dados });
            }
        }
        if atomo == ATOMO_WM_NAME || self.atomos.get(atomo as usize - 1).is_some_and(|a| a == "_NET_WM_NAME") {
            self.sujar(w);
        }
        if w == JANELA_SERVIDOR {
            // a resposta do programa dono da selecao: o texto copiado
            if let Some(prop) = self.janela_mut(w).and_then(|j| j.propriedades.remove(&atomo)) {
                crate::gui::transferencia::guardar(&String::from_utf8_lossy(&prop.dados));
            }
            return;
        }
        self.avisar_propriedade(w, atomo, 0);
    }

    fn avisar_propriedade(&mut self, w: u32, atomo: u32, estado: u8) {
        let mut e = [0u8; 32];
        e[0] = 28;
        e[4..8].copy_from_slice(&w.to_le_bytes());
        e[8..12].copy_from_slice(&atomo.to_le_bytes());
        e[12..16].copy_from_slice(&agora_ms().to_le_bytes());
        e[16] = estado;
        self.evento_para_interessados(w, PROPERTY_CHANGE, e);
    }

    fn pegar_propriedade(&mut self, id: u32, p: &[u8]) {
        let (apagar, w, atomo, tipo) = (p[1] != 0, u32_em(p, 4), u32_em(p, 8), u32_em(p, 12));
        let (deslocamento, quanto) = (u32_em(p, 16) as usize * 4, u32_em(p, 20) as usize * 4);
        let Some(j) = self.janela(w) else { return self.erro(id, BAD_WINDOW, w, 20) };
        let mut c = Vec::new();
        let Some(prop) = j.propriedades.get(&atomo) else {
            c.extend_from_slice(&[0; 12]);
            return self.responder(id, 0, c);
        };
        if tipo != 0 && tipo != prop.tipo {
            c.extend_from_slice(&prop.tipo.to_le_bytes());
            c.extend_from_slice(&(prop.dados.len() as u32).to_le_bytes());
            c.extend_from_slice(&0u32.to_le_bytes());
            let formato = prop.formato;
            return self.responder(id, formato, c);
        }
        let inicio = deslocamento.min(prop.dados.len());
        let fim = (inicio + quanto).min(prop.dados.len());
        let unidade = prop.formato as usize / 8;
        c.extend_from_slice(&prop.tipo.to_le_bytes());
        c.extend_from_slice(&((prop.dados.len() - fim) as u32).to_le_bytes());
        c.extend_from_slice((((fim - inicio) / unidade) as u32).to_le_bytes().as_slice());
        c.extend_from_slice(&[0; 12]);
        c.extend_from_slice(&prop.dados[inicio..fim]);
        let formato = prop.formato;
        let tudo = fim == prop.dados.len();
        self.responder(id, formato, c);
        if apagar && tudo {
            self.janela_mut(w).unwrap().propriedades.remove(&atomo);
            self.avisar_propriedade(w, atomo, 1);
        }
    }

    fn mandar_evento(&mut self, p: &[u8]) {
        let destino = u32_em(p, 4);
        let mascara = u32_em(p, 8);
        let mut e = [0u8; 32];
        e.copy_from_slice(&p[12..44]);
        e[0] |= 0x80; // veio de SendEvent
        let destino = if destino == 0 || destino == 1 { self.foco } else { destino };
        if mascara == 0 {
            if let Some((dono, _)) = self.recursos.get(&destino) {
                let dono = *dono;
                self.evento(dono, e);
            }
        } else {
            self.evento_para_interessados(destino, mascara, e);
        }
    }

    /// O numero de um atomo (criando, se precisar).
    fn atomo_de(&mut self, nome: &str) -> u32 {
        match self.atomos.iter().position(|a| a == nome) {
            Some(i) => i as u32 + 1,
            None => {
                self.atomos.push(String::from(nome));
                self.atomos.len() as u32
            }
        }
    }

    /// SelectionClear para o dono antigo.
    fn avisar_perda(&mut self, janela: u32, selecao: u32) {
        let Some(&(dono, _)) = self.recursos.get(&janela) else { return };
        let mut e = [0u8; 32];
        e[0] = 29;
        e[4..8].copy_from_slice(&agora_ms().to_le_bytes());
        e[8..12].copy_from_slice(&janela.to_le_bytes());
        e[12..16].copy_from_slice(&selecao.to_le_bytes());
        self.evento(dono, e);
    }

    fn tomar_selecao(&mut self, selecao: u32) {
        if let Some(v) = self.donos_selecao.insert(selecao, JANELA_SERVIDOR).filter(|&v| v != JANELA_SERVIDOR) {
            self.avisar_perda(v, selecao);
        }
    }

    /// SelectionRequest para o programa dono de `janela_dona`.
    fn pedir_conversao(&mut self, janela_dona: u32, selecao: u32, alvo: u32, quem: u32, propriedade: u32) {
        let Some(&(dono, _)) = self.recursos.get(&janela_dona) else { return };
        let mut e = [0u8; 32];
        e[0] = 30;
        e[4..8].copy_from_slice(&agora_ms().to_le_bytes());
        e[8..12].copy_from_slice(&janela_dona.to_le_bytes());
        e[12..16].copy_from_slice(&quem.to_le_bytes());
        e[16..20].copy_from_slice(&selecao.to_le_bytes());
        e[20..24].copy_from_slice(&alvo.to_le_bytes());
        e[24..28].copy_from_slice(&propriedade.to_le_bytes());
        self.evento(dono, e);
    }

    /// ConvertSelection: o dono (um programa) converte; sem dono, ou com o
    /// texto copiado no MXOS, o servidor responde com a area de transferencia.
    fn converter_selecao(&mut self, id: u32, p: &[u8]) {
        let (quem, selecao, alvo, propriedade, tempo) = (u32_em(p, 4), u32_em(p, 8), u32_em(p, 12), u32_em(p, 16), u32_em(p, 20));
        let dono = self.donos_selecao.get(&selecao).copied().unwrap_or(0);
        let do_programa = dono != JANELA_SERVIDOR && self.recursos.get(&dono).is_some_and(|(c, _)| self.clientes.contains_key(c));
        if do_programa {
            return self.pedir_conversao(dono, selecao, alvo, quem, propriedade);
        }
        let propriedade = if propriedade == 0 { alvo } else { propriedade };
        let (utf8, texto_a, targets, atom) = (self.atomo_de("UTF8_STRING"), self.atomo_de("TEXT"), self.atomo_de("TARGETS"), 4);
        let texto = crate::gui::transferencia::texto();
        let resposta = if alvo == targets {
            let mut d = Vec::new();
            for a in [targets, utf8, 31, texto_a] {
                d.extend_from_slice(&a.to_le_bytes());
            }
            Some((atom, 32u8, d))
        } else if alvo == utf8 || alvo == texto_a {
            Some((utf8, 8, Vec::from(texto.as_bytes())))
        } else if alvo == 31 {
            // STRING: Latin-1
            Some((31, 8, texto.chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).collect()))
        } else {
            None
        };
        let feito = match (resposta, self.janela_mut(quem)) {
            (Some((tipo, formato, dados)), Some(j)) => {
                j.propriedades.insert(propriedade, Propriedade { tipo, formato, dados });
                true
            }
            _ => false,
        };
        let mut e = [0u8; 32];
        e[0] = 31; // SelectionNotify
        e[4..8].copy_from_slice(&tempo.to_le_bytes());
        e[8..12].copy_from_slice(&quem.to_le_bytes());
        e[12..16].copy_from_slice(&selecao.to_le_bytes());
        e[16..20].copy_from_slice(&alvo.to_le_bytes());
        e[20..24].copy_from_slice(&(if feito { propriedade } else { 0 }).to_le_bytes());
        self.evento(id, e);
    }

    fn pegar_ponteiro(&mut self, id: u32, w: u32) {
        let (ax, ay) = self.absoluta(w);
        let (px, py) = self.ponteiro;
        let mut c = Vec::new();
        c.extend_from_slice(&RAIZ.to_le_bytes());
        c.extend_from_slice(&0u32.to_le_bytes());
        for v in [px, py, px - ax, py - ay] {
            c.extend_from_slice(&(v as i16).to_le_bytes());
        }
        c.extend_from_slice(&(self.botoes | self.modificadores).to_le_bytes());
        self.responder(id, 1, c);
    }

    fn traduzir(&mut self, id: u32, p: &[u8]) {
        let (de, para) = (u32_em(p, 4), u32_em(p, 8));
        let (ox, oy) = self.absoluta(de);
        let (dx, dy) = self.absoluta(para);
        let (x, y) = (i16_em(p, 12) + ox - dx, i16_em(p, 14) + oy - dy);
        let mut c = Vec::new();
        c.extend_from_slice(&0u32.to_le_bytes());
        c.extend_from_slice(&(x as i16).to_le_bytes());
        c.extend_from_slice(&(y as i16).to_le_bytes());
        self.responder(id, 1, c);
    }

    fn extensao(&mut self, id: u32, p: &[u8]) {
        let n = u16_em(p, 4) as usize;
        let nome = p.get(8..8 + n).unwrap_or(&[]);
        let (presente, maior, evento) = match nome {
            b"BIG-REQUESTS" => (1, OP_BIG_REQUESTS, 0),
            b"MIT-SHM" => (1, OP_SHM, PRIMEIRO_EVENTO_SHM),
            _ => (0, 0, 0),
        };
        self.responder(id, 0, vec![presente, maior, evento, 0]);
    }

    fn mapa_teclado(&mut self, id: u32, primeiro: u8, quantos: u8) {
        let mut c = vec![0u8; 24];
        for k in 0..quantos as u16 {
            for sym in teclado::keysyms((primeiro as u16 + k) as u8) {
                c.extend_from_slice(&sym.to_le_bytes());
            }
        }
        self.responder(id, teclado::POR_TECLA as u8, c);
    }

    fn cor_por_nome(&mut self, id: u32, p: &[u8]) {
        let n = u16_em(p, 8) as usize;
        let nome = String::from_utf8_lossy(p.get(12..12 + n).unwrap_or(&[])).to_ascii_lowercase();
        let rgb = match nome.as_str() {
            "black" => 0x000000,
            "white" => 0xFFFFFF,
            "red" => 0xFF0000,
            "green" => 0x00FF00,
            "blue" => 0x0000FF,
            "yellow" => 0xFFFF00,
            "cyan" => 0x00FFFF,
            "magenta" => 0xFF00FF,
            "gray" | "grey" => 0xBEBEBE,
            "darkgray" | "darkgrey" | "dark gray" => 0xA9A9A9,
            "lightgray" | "lightgrey" | "light gray" => 0xD3D3D3,
            n if n.starts_with('#') && n.len() == 7 => u32::from_str_radix(&n[1..], 16).unwrap_or(0),
            _ => return self.erro(id, BAD_NAME, 0, p[0]),
        };
        let mut c = Vec::new();
        if p[0] == 85 {
            c.extend_from_slice(&(rgb as u32).to_le_bytes());
        }
        for _ in 0..2 {
            for v in [(rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF] {
                c.extend_from_slice(&((v * 0x101) as u16).to_le_bytes());
            }
        }
        self.responder(id, 0, c);
    }

    // -----------------------------------------------------------------
    // Desenho

    /// A imagem de um drawable (janela ou pixmap).
    fn imagem_mut(&mut self, d: u32) -> Option<&mut Imagem> {
        match self.recursos.get_mut(&d) {
            Some((_, Recurso::Janela(j))) if !j.so_entrada => Some(&mut j.imagem),
            Some((_, Recurso::Pixmap(img, _))) => Some(img),
            _ => None,
        }
    }

    fn gc(&self, g: u32) -> Option<(u32, u32, bool, Option<Vec<Ret>>)> {
        match self.recursos.get(&g) {
            Some((_, Recurso::Gc(gc))) => {
                let recorte = gc.recorte.as_ref().map(|r| {
                    r.iter().map(|&(x, y, l, a)| (x + gc.origem_recorte.0, y + gc.origem_recorte.1, l, a)).collect()
                });
                Some((gc.frente, gc.fundo, gc.exposicoes, recorte))
            }
            _ => None,
        }
    }

    fn limpar_area(&mut self, p: &[u8]) {
        let w = u32_em(p, 4);
        let Some(j) = self.janela_mut(w) else { return };
        let Some(cor) = j.fundo else { return };
        let (x, y) = (i16_em(p, 8), i16_em(p, 10));
        let mut l = u16_em(p, 12) as i32;
        let mut a = u16_em(p, 14) as i32;
        if l == 0 {
            l = j.imagem.largura as i32 - x;
        }
        if a == 0 {
            a = j.imagem.altura as i32 - y;
        }
        j.imagem.preencher(x, y, l, a, cor, None);
        if p[1] != 0 {
            self.expor(w, x.max(0) as usize, y.max(0) as usize, l.max(0) as usize, a.max(0) as usize);
        }
        self.sujar(w);
    }

    fn copiar_area(&mut self, id: u32, p: &[u8]) {
        let (origem, destino, g) = (u32_em(p, 4), u32_em(p, 8), u32_em(p, 12));
        let (sx, sy, dx, dy) = (i16_em(p, 16), i16_em(p, 18), i16_em(p, 20), i16_em(p, 22));
        let (l, a) = (u16_em(p, 24) as i32, u16_em(p, 26) as i32);
        let Some((frente, fundo, exposicoes, recorte)) = self.gc(g) else { return };
        let plano = p[0] == 63;
        // copia a parte de origem (a origem pode ser o proprio destino)
        let Some(img) = self.imagem_mut(origem) else { return self.erro(id, BAD_DRAWABLE, origem, p[0]) };
        let mut pedaco = Vec::with_capacity((l.max(0) * a.max(0)) as usize);
        for y in sy..sy + a {
            for x in sx..sx + l {
                let dentro_img = x >= 0 && y >= 0 && (x as usize) < img.largura && (y as usize) < img.altura;
                let px = if dentro_img { Some(img.pixels[y as usize * img.largura + x as usize]) } else { None };
                pedaco.push(px.map(|v| if plano { if v & 1 != 0 { frente } else { fundo } } else { v }));
            }
        }
        let Some(img) = self.imagem_mut(destino) else { return self.erro(id, BAD_DRAWABLE, destino, p[0]) };
        let (dl, da) = (img.largura as i32, img.altura as i32);
        for yy in 0..a {
            let y = dy + yy;
            if y < 0 || y >= da {
                continue;
            }
            for xx in 0..l {
                let x = dx + xx;
                if x < 0 || x >= dl || recorte.as_ref().is_some_and(|r| !dentro(r, x, y)) {
                    continue;
                }
                if let Some(v) = pedaco[(yy * l + xx) as usize] {
                    img.pixels[y as usize * img.largura + x as usize] = v;
                }
            }
        }
        self.sujar(destino);
        if exposicoes {
            let mut e = [0u8; 32];
            e[0] = 14; // NoExpose
            e[4..8].copy_from_slice(&destino.to_le_bytes());
            e[10] = p[0];
            self.evento(id, e);
        }
    }

    fn retangulos(&mut self, p: &[u8]) {
        let (d, g) = (u32_em(p, 4), u32_em(p, 8));
        let Some((frente, _, _, recorte)) = self.gc(g) else { return };
        let Some(img) = self.imagem_mut(d) else { return };
        for k in (12..p.len()).step_by(8) {
            if k + 8 > p.len() {
                break;
            }
            img.preencher(i16_em(p, k), i16_em(p, k + 2), u16_em(p, k + 4) as i32, u16_em(p, k + 6) as i32, frente, recorte.as_deref());
        }
        self.sujar(d);
    }

    fn linhas(&mut self, p: &[u8]) {
        let (op, modo, d, g) = (p[0], p[1], u32_em(p, 4), u32_em(p, 8));
        let Some((frente, _, _, _)) = self.gc(g) else { return };
        let Some(img) = self.imagem_mut(d) else { return };
        let pontos: Vec<(i32, i32)> = (12..p.len()).step_by(4).filter(|k| k + 4 <= p.len()).map(|k| (i16_em(p, k), i16_em(p, k + 2))).collect();
        match op {
            64 => {
                let (mut ux, mut uy) = (0, 0);
                for (x, y) in pontos {
                    let (x, y) = if modo == 1 { (ux + x, uy + y) } else { (x, y) };
                    img.ponto(x, y, frente);
                    (ux, uy) = (x, y);
                }
            }
            65 => {
                let mut ultimo: Option<(i32, i32)> = None;
                for (x, y) in pontos {
                    let (x, y) = match (modo, ultimo) {
                        (1, Some((ux, uy))) => (ux + x, uy + y),
                        _ => (x, y),
                    };
                    if let Some((ux, uy)) = ultimo {
                        img.linha(ux, uy, x, y, frente);
                    }
                    ultimo = Some((x, y));
                }
            }
            66 => {
                for par in pontos.chunks_exact(2) {
                    img.linha(par[0].0, par[0].1, par[1].0, par[1].1, frente);
                }
            }
            _ => {
                for r in pontos.chunks_exact(2) {
                    let ((x, y), (l, a)) = (r[0], (r[1].0 & 0xFFFF, r[1].1 & 0xFFFF));
                    img.linha(x, y, x + l, y, frente);
                    img.linha(x, y + a, x + l, y + a, frente);
                    img.linha(x, y, x, y + a, frente);
                    img.linha(x + l, y, x + l, y + a, frente);
                }
            }
        }
        self.sujar(d);
    }

    fn por_imagem(&mut self, id: u32, p: &[u8]) {
        let (formato, d, g) = (p[1], u32_em(p, 4), u32_em(p, 8));
        let (l, a) = (u16_em(p, 12) as usize, u16_em(p, 14) as usize);
        let (dx, dy) = (i16_em(p, 16), i16_em(p, 18));
        let (margem, prof) = (p[20] as usize, p[21]);
        let Some((frente, fundo, _, recorte)) = self.gc(g) else { return };
        let dados = &p[24.min(p.len())..];
        let Some(img) = self.imagem_mut(d) else { return self.erro(id, BAD_DRAWABLE, d, 72) };
        let por = |img: &mut Imagem, x: i32, y: i32, v: u32| {
            if recorte.as_ref().is_none_or(|r| dentro(r, x, y)) {
                img.ponto(x, y, v);
            }
        };
        if formato == 2 && prof >= 24 {
            for y in 0..a {
                let linha = &dados[(y * l * 4).min(dados.len())..((y + 1) * l * 4).min(dados.len())];
                let yy = dy + y as i32;
                if yy < 0 || yy as usize >= img.altura {
                    continue;
                }
                if recorte.is_none() && dx >= 0 && dx as usize + l <= img.largura && linha.len() == l * 4 {
                    // o caso comum: a fileira inteira de uma vez
                    let inicio = yy as usize * img.largura + dx as usize;
                    for (k, px) in linha.chunks_exact(4).enumerate() {
                        img.pixels[inicio + k] = u32::from_le_bytes([px[0], px[1], px[2], 0]);
                    }
                    continue;
                }
                for (k, px) in linha.chunks_exact(4).enumerate() {
                    por(img, dx + k as i32, yy, u32::from_le_bytes([px[0], px[1], px[2], 0]));
                }
            }
        } else if prof == 1 || formato == 0 {
            // bitmap: 1 = frente, 0 = fundo (num pixmap de 1 bit: 1 e 0)
            let linha = (l + margem).div_ceil(32) * 4;
            let um_bit = matches!(self.recursos.get(&d), Some((_, Recurso::Pixmap(_, 1))));
            let Some(img) = self.imagem_mut(d) else { return };
            for y in 0..a {
                for x in 0..l {
                    let bit = x + margem;
                    let byte = dados.get(y * linha + bit / 8).copied().unwrap_or(0);
                    let ligado = byte >> (bit % 8) & 1 != 0;
                    let v = if um_bit { ligado as u32 } else if ligado { frente } else { fundo };
                    if recorte.as_ref().is_none_or(|r| dentro(r, dx + x as i32, dy + y as i32)) {
                        img.ponto(dx + x as i32, dy + y as i32, v);
                    }
                }
            }
        }
        self.sujar(d);
    }

    fn pegar_imagem(&mut self, id: u32, p: &[u8]) {
        let d = u32_em(p, 4);
        let (x, y) = (i16_em(p, 8), i16_em(p, 10));
        let (l, a) = (u16_em(p, 12) as i32, u16_em(p, 14) as i32);
        let Some(img) = self.imagem_mut(d) else { return self.erro(id, BAD_DRAWABLE, d, 73) };
        if x < 0 || y < 0 || x + l > img.largura as i32 || y + a > img.altura as i32 {
            return self.erro(id, BAD_MATCH, 0, 73);
        }
        let mut c = Vec::with_capacity(24 + (l * a * 4) as usize);
        c.extend_from_slice(&VISUAL.to_le_bytes());
        c.extend_from_slice(&[0; 20]);
        for yy in y..y + a {
            for xx in x..x + l {
                c.extend_from_slice(&img.pixels[yy as usize * img.largura + xx as usize].to_le_bytes());
            }
        }
        self.responder(id, 24, c);
    }

    // -----------------------------------------------------------------
    // MIT-SHM: a imagem mora num segmento de memoria dividida (shmget do
    // programa); ShmPutImage le direto dele, sem passar pelo soquete.

    fn pedido_shm(&mut self, id: u32, p: &[u8]) {
        match p[1] {
            0 => {
                // QueryVersion 1.2, sem pixmaps divididos
                let mut c = Vec::new();
                c.extend_from_slice(&1u16.to_le_bytes());
                c.extend_from_slice(&2u16.to_le_bytes());
                c.extend_from_slice(&0u16.to_le_bytes());
                c.extend_from_slice(&0u16.to_le_bytes());
                c.push(2); // formato ZPixmap
                self.responder(id, 0, c);
            }
            1 => {
                // Attach: seg, shmid, so leitura
                let (seg, shmid) = (u32_em(p, 4), u32_em(p, 8));
                if !shm::prender(shmid) {
                    return self.erro(id, BAD_ACCESS, shmid, OP_SHM);
                }
                self.recursos.insert(seg, (id, Recurso::Shm(shmid)));
            }
            2 => {
                let seg = u32_em(p, 4);
                if let Some((_, Recurso::Shm(shmid))) = self.recursos.remove(&seg) {
                    shm::soltar(shmid);
                }
            }
            3 => self.shm_por_imagem(id, p),
            _ => self.erro(id, BAD_IMPLEMENTATION, 0, OP_SHM),
        }
    }

    fn shm_por_imagem(&mut self, id: u32, p: &[u8]) {
        let (d, g) = (u32_em(p, 4), u32_em(p, 8));
        let (total_l, _total_a) = (u16_em(p, 12) as usize, u16_em(p, 14) as usize);
        let (sx, sy, l, a) = (u16_em(p, 16) as usize, u16_em(p, 18) as usize, u16_em(p, 20) as usize, u16_em(p, 22) as usize);
        let (dx, dy) = (i16_em(p, 24), i16_em(p, 26));
        let (prof, formato, avisar) = (p[28], p[29], p[30] != 0);
        let (seg, deslocamento) = (u32_em(p, 32), u32_em(p, 36) as usize);
        let Some((_, Recurso::Shm(shmid))) = self.recursos.get(&seg) else { return self.erro(id, BAD_VALUE, seg, OP_SHM) };
        let shmid = *shmid;
        let Some((_, _, _, recorte)) = self.gc(g) else { return };
        if formato != 2 || prof < 24 {
            return self.erro(id, BAD_MATCH, 0, OP_SHM);
        }
        let Some(img) = self.imagem_mut(d) else { return self.erro(id, BAD_DRAWABLE, d, OP_SHM) };
        let mut fileira = vec![0u8; l * 4];
        for y in 0..a {
            let yy = dy + y as i32;
            if yy < 0 || yy as usize >= img.altura {
                continue;
            }
            let origem = deslocamento + ((sy + y) * total_l + sx) * 4;
            if !shm::ler(shmid, origem, &mut fileira) {
                break;
            }
            for (k, px) in fileira.chunks_exact(4).enumerate() {
                let xx = dx + k as i32;
                if xx < 0 || xx as usize >= img.largura || recorte.as_ref().is_some_and(|r| !dentro(r, xx, yy)) {
                    continue;
                }
                img.pixels[yy as usize * img.largura + xx as usize] = u32::from_le_bytes([px[0], px[1], px[2], 0]);
            }
        }
        self.sujar(d);
        if avisar {
            let mut e = [0u8; 32];
            e[0] = PRIMEIRO_EVENTO_SHM; // ShmCompletion
            e[4..8].copy_from_slice(&d.to_le_bytes());
            e[8..10].copy_from_slice(&(3u16).to_le_bytes());
            e[10] = OP_SHM;
            e[12..16].copy_from_slice(&seg.to_le_bytes());
            e[16..20].copy_from_slice(&(deslocamento as u32).to_le_bytes());
            self.evento(id, e);
        }
    }

    // -----------------------------------------------------------------
    // Entrada (vinda da janela do MXOS)

    /// A janela mapeada mais funda sob (x, y) dentro de `topo` (x, y
    /// relativos ao topo).
    fn janela_em(&self, topo: u32, x: i32, y: i32) -> u32 {
        let mut atual = topo;
        let (mut ox, mut oy) = (x, y);
        'fora: for _ in 0..32 {
            let Some(j) = self.janela(atual) else { break };
            for &f in j.filhas.iter().rev() {
                if let Some(fj) = self.janela(f) {
                    let dentro = ox >= fj.x && oy >= fj.y && ox < fj.x + fj.imagem.largura as i32 && oy < fj.y + fj.imagem.altura as i32;
                    if fj.mapeada && (dentro || fj.so_entrada && dentro) {
                        ox -= fj.x;
                        oy -= fj.y;
                        atual = f;
                        continue 'fora;
                    }
                }
            }
            break;
        }
        atual
    }

    /// Evento de ponteiro/teclado: sobe da janela `origem` ate achar quem
    /// escolheu a mascara, como o X faz (event_x/y relativos a ela).
    fn propagar(&mut self, origem: u32, mascara: u32, codigo: u8, detalhe: u8) {
        let mut w = origem;
        let mut filha: u32 = 0;
        for _ in 0..32 {
            let interessados: Vec<u32> = match self.janela(w) {
                Some(j) => j.interesses.iter().filter(|(_, m)| m & mascara != 0).map(|(c, _)| *c).collect(),
                None => return,
            };
            if !interessados.is_empty() {
                let (ax, ay) = self.absoluta(w);
                let (px, py) = self.ponteiro;
                let mut e = [0u8; 32];
                e[0] = codigo;
                e[1] = detalhe;
                e[4..8].copy_from_slice(&agora_ms().to_le_bytes());
                e[8..12].copy_from_slice(&RAIZ.to_le_bytes());
                e[12..16].copy_from_slice(&w.to_le_bytes());
                e[16..20].copy_from_slice(&filha.to_le_bytes());
                for (k, v) in [px, py, px - ax, py - ay].into_iter().enumerate() {
                    e[20 + 2 * k..22 + 2 * k].copy_from_slice(&(v as i16).to_le_bytes());
                }
                e[28..30].copy_from_slice(&(self.botoes | self.modificadores).to_le_bytes());
                e[30] = 1;
                for c in interessados {
                    self.evento(c, e);
                }
                return;
            }
            let Some(j) = self.janela(w) else { return };
            if j.pai == RAIZ {
                return;
            }
            filha = w;
            w = j.pai;
        }
    }

    fn entrar(&mut self, nova: u32) {
        if nova == self.sob_ponteiro {
            return;
        }
        let velha = self.sob_ponteiro;
        self.sob_ponteiro = nova;
        for (w, codigo, mascara) in [(velha, 8u8, LEAVE_WINDOW), (nova, 7, ENTER_WINDOW)] {
            if self.janela(w).is_none() {
                continue;
            }
            let (ax, ay) = self.absoluta(w);
            let (px, py) = self.ponteiro;
            let mut e = [0u8; 32];
            e[0] = codigo;
            e[4..8].copy_from_slice(&agora_ms().to_le_bytes());
            e[8..12].copy_from_slice(&RAIZ.to_le_bytes());
            e[12..16].copy_from_slice(&w.to_le_bytes());
            for (k, v) in [px, py, px - ax, py - ay].into_iter().enumerate() {
                e[20 + 2 * k..22 + 2 * k].copy_from_slice(&(v as i16).to_le_bytes());
            }
            e[28..30].copy_from_slice(&(self.botoes | self.modificadores).to_le_bytes());
            e[31] = 3; // same-screen + focus
            self.evento_para_interessados(w, mascara, e);
        }
    }
}

fn valores_gc(gc: &mut Gc, mascara: u32, valores: &[u8]) {
    let mut k = 0;
    for bit in 0..23 {
        if mascara & (1 << bit) == 0 {
            continue;
        }
        let v = u32_em(valores, k);
        k += 4;
        match bit {
            2 => gc.frente = v & 0x00FF_FFFF,
            3 => gc.fundo = v & 0x00FF_FFFF,
            16 => gc.exposicoes = v != 0,
            17 => gc.origem_recorte.0 = v as i16 as i32,
            18 => gc.origem_recorte.1 = v as i16 as i32,
            19 => gc.recorte = None, // clip-mask None (um pixmap de mascara nao e seguido)
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------
// O que a area de trabalho usa

/// Janelas de primeiro nivel que acabaram de aparecer (mapeadas, sem
/// override-redirect): a area de trabalho abre uma janela do MXOS para cada.
pub fn janelas_novas() -> Vec<u32> {
    let Some(s) = servidor() else { return Vec::new() };
    let mut novas = Vec::new();
    for (id, (_, r)) in s.recursos.iter_mut() {
        if let Recurso::Janela(j) = r {
            if j.pai == RAIZ && j.mapeada && !j.na_tela && !j.so_entrada {
                j.na_tela = true;
                novas.push(*id);
            }
        }
    }
    novas
}

/// Ainda existe e esta mapeada?
pub fn viva(id: u32) -> bool {
    servidor().and_then(|s| s.janela(id)).is_some_and(|j| j.mapeada)
}

pub fn tamanho(id: u32) -> (usize, usize) {
    servidor().and_then(|s| s.janela(id)).map_or((1, 1), |j| (j.imagem.largura, j.imagem.altura))
}

/// O titulo (_NET_WM_NAME em UTF-8 ou WM_NAME).
pub fn titulo(id: u32) -> String {
    let Some(s) = servidor() else { return String::new() };
    let Some(j) = s.janela(id) else { return String::new() };
    let net = s.atomos.iter().position(|a| a == "_NET_WM_NAME").map(|i| i as u32 + 1);
    let prop = net.and_then(|a| j.propriedades.get(&a)).or_else(|| j.propriedades.get(&ATOMO_WM_NAME));
    match prop {
        Some(p) => String::from_utf8_lossy(&p.dados).into_owned(),
        // sem titulo: o nome do programa (o "netsurf-fb" vira "Netsurf-fb")
        None => s.recursos.get(&id).and_then(|(dono, _)| s.clientes.get(dono)).map_or(String::new(), |c| {
            let nome = c.ponta.quem.rsplit('/').next().unwrap_or("");
            let mut letras = nome.chars();
            letras.next().map_or(String::new(), |p| p.to_uppercase().chain(letras).collect())
        }),
    }
}

/// Mudou desde a ultima copia? (e esquece)
pub fn tirar_sujeira(id: u32) -> bool {
    servidor().and_then(|s| s.janela_mut(id)).is_some_and(|j| core::mem::take(&mut j.suja))
}

/// Copia a janela (com as filhas mapeadas por cima) para `destino`.
pub fn copiar(id: u32, destino: &mut [u32], largura: usize, altura: usize) {
    let Some(s) = servidor() else { return };
    copiar_rec(s, id, destino, largura, altura, 0, 0);
}

fn copiar_rec(s: &Servidor, id: u32, destino: &mut [u32], largura: usize, altura: usize, ox: i32, oy: i32) {
    let Some(j) = s.janela(id) else { return };
    if !j.so_entrada {
        let img = &j.imagem;
        for y in 0..img.altura {
            let yy = oy + y as i32;
            if yy < 0 || yy as usize >= altura {
                continue;
            }
            let x0 = ox.max(0);
            let x1 = (ox + img.largura as i32).min(largura as i32);
            if x0 >= x1 {
                continue;
            }
            let de = y * img.largura + (x0 - ox) as usize;
            let para = yy as usize * largura + x0 as usize;
            let n = (x1 - x0) as usize;
            destino[para..para + n].copy_from_slice(&img.pixels[de..de + n]);
        }
    }
    for &f in &j.filhas {
        if let Some(fj) = s.janela(f) {
            if fj.mapeada {
                copiar_rec(s, f, destino, largura, altura, ox + fj.x, oy + fj.y);
            }
        }
    }
}

/// A janela do MXOS mudou de lugar na tela (para as coordenadas absolutas).
pub fn posicao(id: u32, x: i32, y: i32) {
    if let Some(j) = servidor().and_then(|s| s.janela_mut(id)) {
        j.x = x;
        j.y = y;
    }
}

/// Mouse em (x, y) do conteudo da janela `id`, com os botoes de agora
/// (bit 0 esquerdo, 1 direito, 2 meio).
pub fn mouse(id: u32, x: i32, y: i32) {
    let Some(s) = servidor() else { return };
    let (ax, ay) = s.absoluta(id);
    s.ponteiro = (ax + x, ay + y);
    let alvo = s.janela_em(id, x, y);
    s.entrar(alvo);
    let mascara = POINTER_MOTION | if s.botoes != 0 { BUTTONS_MOTION } else { 0 };
    s.propagar(alvo, mascara, 6, 0);
    unix::avisar();
}

/// Botao: 1 esquerdo, 2 meio, 3 direito, 4/5 roda.
pub fn botao(id: u32, x: i32, y: i32, botao: u8, apertado: bool) {
    let Some(s) = servidor() else { return };
    let (ax, ay) = s.absoluta(id);
    // (-1, -1): onde o ponteiro ja esta (a roda nao diz a posicao)
    let (x, y) = if (x, y) == (-1, -1) { (s.ponteiro.0 - ax, s.ponteiro.1 - ay) } else { (x, y) };
    s.ponteiro = (ax + x, ay + y);
    let alvo = s.janela_em(id, x, y);
    let bit = match botao {
        1..=5 => 1u16 << (7 + botao),
        _ => 0,
    };
    // o state do evento e o de antes do botao
    if apertado {
        s.propagar(alvo, BUTTON_PRESS, 4, botao);
        s.botoes |= bit;
    } else {
        s.botoes &= !bit;
        s.propagar(alvo, BUTTON_RELEASE, 5, botao);
    }
    unix::avisar();
}

/// Tecla (codigo evdev) com a janela `id` em foco.
pub fn tecla(id: u32, codigo: u16, apertada: bool) {
    let Some(s) = servidor() else { return };
    let keycode = (codigo + teclado::PRIMEIRO as u16).min(255) as u8;
    let foco = if s.foco != RAIZ && s.primeiro_nivel(s.foco) == Some(id) { s.foco } else { id };
    let alvo = if s.primeiro_nivel(s.sob_ponteiro) == Some(id) && foco == id { s.sob_ponteiro } else { foco };
    if apertada {
        s.propagar(alvo, KEY_PRESS, 2, keycode);
        s.modificadores |= teclado::bit_modificador(codigo);
    } else {
        s.modificadores &= !teclado::bit_modificador(codigo);
        s.propagar(alvo, KEY_RELEASE, 3, keycode);
    }
    unix::avisar();
}

/// A janela do MXOS ganhou ou perdeu o foco.
pub fn foco(id: u32, ganhou: bool) {
    let Some(s) = servidor() else { return };
    if ganhou {
        s.foco = id;
    } else if s.primeiro_nivel(s.foco) == Some(id) {
        s.foco = RAIZ;
    }
    let mut e = [0u8; 32];
    e[0] = if ganhou { 9 } else { 10 };
    e[1] = 3; // NotifyNonlinear
    e[4..8].copy_from_slice(&id.to_le_bytes());
    s.evento_para_interessados(id, FOCUS_CHANGE, e);
    unix::avisar();
}

/// O X da janela do MXOS: pede ao programa para fechar (WM_DELETE_WINDOW)
/// ou, se ele nao entende isso, desliga o cliente.
pub fn fechar(id: u32) {
    let Some(s) = servidor() else { return };
    let Some(&(dono, _)) = s.recursos.get(&id) else { return };
    let protocolos = s.atomos.iter().position(|a| a == "WM_PROTOCOLS").map(|i| i as u32 + 1);
    let apagar = s.atomos.iter().position(|a| a == "WM_DELETE_WINDOW").map(|i| i as u32 + 1);
    let entende = match (protocolos, apagar, s.janela(id)) {
        (Some(p), Some(a), Some(j)) => j.propriedades.get(&p).is_some_and(|prop| prop.dados.chunks_exact(4).any(|c| u32_em(c, 0) == a)),
        _ => false,
    };
    if entende {
        let mut e = [0u8; 32];
        e[0] = 33; // ClientMessage
        e[1] = 32;
        e[4..8].copy_from_slice(&id.to_le_bytes());
        e[8..12].copy_from_slice(&protocolos.unwrap().to_le_bytes());
        e[12..16].copy_from_slice(&apagar.unwrap().to_le_bytes());
        e[16..20].copy_from_slice(&agora_ms().to_le_bytes());
        s.evento(dono, e);
    } else {
        s.desligar(dono);
    }
    unix::avisar();
}

/// Quantos clientes e janelas (para o comando "x11").
pub fn resumo() -> (usize, usize) {
    servidor().map_or((0, 0), |s| {
        let janelas = s.recursos.values().filter(|(_, r)| matches!(r, Recurso::Janela(_))).count();
        (s.clientes.len(), janelas)
    })
}

#[allow(dead_code)]
fn _usar(_: VecDeque<u8>, _: Recurso) {
    let _ = (BUTTON_MOTION, BAD_PIXMAP, KEY_RELEASE);
}
