//! Mapa de teclado: codigo evdev (KEY_*) + modificadores -> caractere (char).
//!
//! Os codigos sao os do Linux (include/uapi/linux/input-event-codes.h), que
//! o virtio-input manda. Eles dizem a POSICAO da tecla no teclado americano;
//! o layout decide o que cada posicao escreve.
//!
//! Dois layouts: Us (americano) e Abnt2 (brasileiro). No ABNT2 as teclas
//! mortas (acento agudo, crase, til, circunflexo, trema) ficam pendentes:
//! com uma vogal depois sai a vogal acentuada (a, A, e, E... e tambem
//! agudo + c = c cedilha), com espaco sai o proprio acento e com outra
//! coisa saem os dois. A tecla do c cedilha escreve o c cedilha. O AltGr
//! do ABNT2 escreve os extras impressos nas teclas (barra, ordinais, grau...).
//!
//! Produz caracteres imprimiveis (ASCII e Latin-1) e os de controle 13
//! (Enter), 8 (Backspace), 9 (Tab) e 27 (Esc). Setas, F1..F12, Delete etc.
//! nao viram caractere (o
//! chamador manda o codigo cru como Evento::Tecla). Com Ctrl ou Alt
//! apertado nada vira caractere (atalhos).
//!
//! Logica pura, sem hardware: da para testar fora do kernel.

use super::Layout;

// Codigos evdev usados aqui.
pub const KEY_ESC: u16 = 1;
pub const KEY_BACKSPACE: u16 = 14;
pub const KEY_TAB: u16 = 15;
pub const KEY_ENTER: u16 = 28;
pub const KEY_LEFTCTRL: u16 = 29;
pub const KEY_LEFTSHIFT: u16 = 42;
pub const KEY_RIGHTSHIFT: u16 = 54;
pub const KEY_LEFTALT: u16 = 56;
pub const KEY_SPACE: u16 = 57;
pub const KEY_CAPSLOCK: u16 = 58;
pub const KEY_NUMLOCK: u16 = 69;
pub const KEY_KPENTER: u16 = 96;
pub const KEY_RIGHTCTRL: u16 = 97;
pub const KEY_RIGHTALT: u16 = 100;
pub const KEY_HOME: u16 = 102;
pub const KEY_UP: u16 = 103;
pub const KEY_PAGEUP: u16 = 104;
pub const KEY_LEFT: u16 = 105;
pub const KEY_RIGHT: u16 = 106;
pub const KEY_END: u16 = 107;
pub const KEY_DOWN: u16 = 108;
pub const KEY_PAGEDOWN: u16 = 109;
pub const KEY_INSERT: u16 = 110;
pub const KEY_DELETE: u16 = 111;
pub const KEY_LEFTMETA: u16 = 125;
pub const KEY_RIGHTMETA: u16 = 126;
pub const KEY_F1: u16 = 59;
pub const KEY_F10: u16 = 68;
pub const KEY_F11: u16 = 87;
pub const KEY_F12: u16 = 88;

// Teclas que mudam entre os layouts.
const KEY_6: u16 = 7;
const KEY_Q: u16 = 16;
const KEY_LEFTBRACE: u16 = 26;
const KEY_SEMICOLON: u16 = 39;
const KEY_APOSTROPHE: u16 = 40;
const KEY_KPDOT: u16 = 83;
const KEY_KPCOMMA: u16 = 121;

/// Valor do evento EV_KEY: 0 soltou, 1 apertou, 2 repeticao automatica.
pub const SOLTOU: u32 = 0;

/// Letras da fileira de cima, do meio e de baixo (codigos 16, 30 e 44).
const FILEIRA_Q: &[u8] = b"qwertyuiop";
const FILEIRA_A: &[u8] = b"asdfghjkl";
const FILEIRA_Z: &[u8] = b"zxcvbnm";

/// Teclas de simbolo do americano: (codigo, normal, com shift).
const SIMBOLOS_US: &[(u16, u8, u8)] = &[
    (2, b'1', b'!'),
    (3, b'2', b'@'),
    (4, b'3', b'#'),
    (5, b'4', b'$'),
    (6, b'5', b'%'),
    (7, b'6', b'^'),
    (8, b'7', b'&'),
    (9, b'8', b'*'),
    (10, b'9', b'('),
    (11, b'0', b')'),
    (12, b'-', b'_'),
    (13, b'=', b'+'),
    (26, b'[', b'{'),
    (27, b']', b'}'),
    (39, b';', b':'),
    (40, b'\'', b'"'),
    (41, b'`', b'~'),
    (43, b'\\', b'|'),
    (51, b',', b'<'),
    (52, b'.', b'>'),
    (53, b'/', b'?'),
    (86, b'\\', b'|'),
];

/// Teclas de simbolo do ABNT2 (as mortas e o c cedilha sao tratados a parte).
const SIMBOLOS_ABNT2: &[(u16, u8, u8)] = &[
    (2, b'1', b'!'),
    (3, b'2', b'@'),
    (4, b'3', b'#'),
    (5, b'4', b'$'),
    (6, b'5', b'%'),
    (8, b'7', b'&'),
    (9, b'8', b'*'),
    (10, b'9', b'('),
    (11, b'0', b')'),
    (12, b'-', b'_'),
    (13, b'=', b'+'),
    (27, b'[', b'{'),
    (41, b'\'', b'"'),
    (43, b']', b'}'),
    (51, b',', b'<'),
    (52, b'.', b'>'),
    (53, b';', b':'),
    (86, b'\\', b'|'),
    (89, b'/', b'?'),
];

/// Teclado numerico com NumLock ligado: (codigo, caractere).
const NUMERICO: &[(u16, u8)] = &[
    (71, b'7'),
    (72, b'8'),
    (73, b'9'),
    (75, b'4'),
    (76, b'5'),
    (77, b'6'),
    (79, b'1'),
    (80, b'2'),
    (81, b'3'),
    (82, b'0'),
];

/// Operadores do teclado numerico (valem com NumLock desligado tambem).
const OPERADORES_NUMERICO: &[(u16, u8)] = &[(55, b'*'), (74, b'-'), (78, b'+'), (98, b'/')];

/// Extras do AltGr no ABNT2 (fora Q e W, que estao nas letras): (codigo, caractere).
const ALTGR_ABNT2: &[(u16, char)] = &[
    (2, '¹'),
    (3, '²'),
    (4, '³'),
    (5, '£'),
    (6, '¢'),
    (7, '¬'),
    (13, '§'),
    (27, 'ª'),
    (43, 'º'),
    (89, '°'),
];

/// Letras que cada acento sabe combinar: (sem acento, com acento), em
/// minusculas; a maiuscula sai pela caixa da letra digitada.
const COM_AGUDO: &[(char, char)] = &[('a', 'á'), ('e', 'é'), ('i', 'í'), ('o', 'ó'), ('u', 'ú'), ('y', 'ý'), ('c', 'ç')];
const COM_CRASE: &[(char, char)] = &[('a', 'à'), ('e', 'è'), ('i', 'ì'), ('o', 'ò'), ('u', 'ù')];
const COM_TIL: &[(char, char)] = &[('a', 'ã'), ('o', 'õ'), ('n', 'ñ')];
const COM_CIRCUNFLEXO: &[(char, char)] = &[('a', 'â'), ('e', 'ê'), ('i', 'î'), ('o', 'ô'), ('u', 'û')];
const COM_TREMA: &[(char, char)] = &[('a', 'ä'), ('e', 'ë'), ('i', 'ï'), ('o', 'ö'), ('u', 'ü'), ('y', 'ÿ')];

/// Acento pendente do ABNT2.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Morta {
    Agudo,
    Crase,
    Til,
    Circunflexo,
    Trema,
}

impl Morta {
    /// O acento sozinho (acento + espaco, ou acento que nao combina).
    fn simbolo(self) -> char {
        match self {
            Morta::Agudo => '´',
            Morta::Crase => '`',
            Morta::Til => '~',
            Morta::Circunflexo => '^',
            Morta::Trema => '¨',
        }
    }

    fn tabela(self) -> &'static [(char, char)] {
        match self {
            Morta::Agudo => COM_AGUDO,
            Morta::Crase => COM_CRASE,
            Morta::Til => COM_TIL,
            Morta::Circunflexo => COM_CIRCUNFLEXO,
            Morta::Trema => COM_TREMA,
        }
    }

    /// A letra `c` com este acento, se existir (mantem a caixa).
    fn aplicar(self, c: char) -> Option<char> {
        let minuscula = c.to_ascii_lowercase();
        let &(_, acentuada) = self.tabela().iter().find(|&&(base, _)| base == minuscula)?;
        if c.is_ascii_uppercase() { acentuada.to_uppercase().next() } else { Some(acentuada) }
    }
}

/// O que uma tecla escreve, antes de considerar teclas mortas.
enum Saida {
    Nada,
    Caractere(char),
    Morta(Morta),
}

/// Estado dos modificadores e do layout. Um so para todos os teclados.
pub struct Teclado {
    layout: Layout,
    shift_esq: bool,
    shift_dir: bool,
    ctrl_esq: bool,
    ctrl_dir: bool,
    alt: bool,
    /// Alt da direita: no ABNT2 e o AltGr; no americano conta como Alt.
    alt_dir: bool,
    caps: bool,
    num: bool,
    morta: Option<Morta>,
}

impl Teclado {
    pub const fn novo(layout: Layout) -> Teclado {
        Teclado {
            layout,
            shift_esq: false,
            shift_dir: false,
            ctrl_esq: false,
            ctrl_dir: false,
            alt: false,
            alt_dir: false,
            caps: false,
            // o QEMU comeca com o NumLock ligado
            num: true,
            morta: None,
        }
    }

    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// Troca o layout e esquece um acento pendente.
    pub fn definir_layout(&mut self, layout: Layout) {
        self.layout = layout;
        self.morta = None;
    }

    pub fn shift(&self) -> bool {
        self.shift_esq || self.shift_dir
    }

    pub fn ctrl(&self) -> bool {
        self.ctrl_esq || self.ctrl_dir
    }

    /// Alt de qualquer lado (no ABNT2 o da direita e o AltGr e nao conta).
    pub fn alt(&self) -> bool {
        self.alt || (self.alt_dir && self.layout == Layout::Us)
    }

    pub fn caps_lock(&self) -> bool {
        self.caps
    }

    pub fn num_lock(&self) -> bool {
        self.num
    }

    fn alt_gr(&self) -> bool {
        self.alt_dir && self.layout == Layout::Abnt2
    }

    /// Trata um EV_KEY (`valor` 0 soltou, 1 apertou, 2 repeticao) e chama
    /// `emitir` com os caracteres resultantes (0, 1 ou 2: um acento pendente
    /// que nao combina sai antes do caractere).
    pub fn processar(&mut self, codigo: u16, valor: u32, emitir: &mut dyn FnMut(char)) {
        let apertada = valor != SOLTOU;
        if self.modificador(codigo, apertada, valor == 1) || !apertada {
            return;
        }
        if self.ctrl() || self.alt() {
            return;
        }
        match self.traduzir(codigo) {
            Saida::Nada => {}
            Saida::Caractere(c) => self.combinar(c, emitir),
            Saida::Morta(m) => self.nova_morta(m, emitir),
        }
    }

    /// Atualiza shift/ctrl/alt/caps/num. Devolve true se era modificador.
    /// `primeira` evita que a repeticao automatica alterne o CapsLock.
    fn modificador(&mut self, codigo: u16, apertada: bool, primeira: bool) -> bool {
        match codigo {
            KEY_LEFTSHIFT => self.shift_esq = apertada,
            KEY_RIGHTSHIFT => self.shift_dir = apertada,
            KEY_LEFTCTRL => self.ctrl_esq = apertada,
            KEY_RIGHTCTRL => self.ctrl_dir = apertada,
            KEY_LEFTALT => self.alt = apertada,
            KEY_RIGHTALT => self.alt_dir = apertada,
            KEY_CAPSLOCK => self.caps ^= apertada && primeira,
            KEY_NUMLOCK => self.num ^= apertada && primeira,
            KEY_LEFTMETA | KEY_RIGHTMETA => {}
            _ => return false,
        }
        true
    }

    /// Codigo -> o que a tecla escreve no layout atual.
    fn traduzir(&self, codigo: u16) -> Saida {
        if let Some(c) = controle(codigo) {
            return Saida::Caractere(c as char);
        }
        if let Some(c) = self.numerico(codigo) {
            return Saida::Caractere(c as char);
        }
        if let Some(letra) = letra(codigo) {
            return self.letra(letra);
        }
        match self.layout {
            Layout::Us => self.simbolo(SIMBOLOS_US, codigo),
            Layout::Abnt2 => self.abnt2(codigo),
        }
    }

    /// Letra com Shift xor CapsLock.
    fn letra(&self, letra: u8) -> Saida {
        if self.alt_gr() {
            return match letra {
                b'q' => Saida::Caractere('/'),
                b'w' => Saida::Caractere('?'),
                b'e' => Saida::Caractere('€'),
                _ => Saida::Nada,
            };
        }
        Saida::Caractere(self.caixa(letra as char))
    }

    /// Maiuscula se Shift xor CapsLock (vale para o c cedilha tambem).
    fn caixa(&self, letra: char) -> char {
        if self.shift() != self.caps { letra.to_uppercase().next().unwrap_or(letra) } else { letra }
    }

    fn simbolo(&self, tabela: &[(u16, u8, u8)], codigo: u16) -> Saida {
        match tabela.iter().find(|&&(c, _, _)| c == codigo) {
            Some(&(_, normal, com_shift)) => Saida::Caractere(if self.shift() { com_shift } else { normal } as char),
            None => Saida::Nada,
        }
    }

    /// Teclas proprias do ABNT2: mortas, c cedilha e as do AltGr.
    fn abnt2(&self, codigo: u16) -> Saida {
        if self.alt_gr() {
            // AltGr+Q e AltGr+W (barra e interrogacao) ja foram tratados em letra
            return match ALTGR_ABNT2.iter().find(|&&(k, _)| k == codigo) {
                Some(&(_, c)) => Saida::Caractere(c),
                None => Saida::Nada,
            };
        }
        let shift = self.shift();
        match codigo {
            KEY_LEFTBRACE => Saida::Morta(if shift { Morta::Crase } else { Morta::Agudo }),
            KEY_APOSTROPHE => Saida::Morta(if shift { Morta::Circunflexo } else { Morta::Til }),
            KEY_6 if shift => Saida::Morta(Morta::Trema),
            KEY_SEMICOLON => Saida::Caractere(self.caixa('ç')),
            _ => self.simbolo(SIMBOLOS_ABNT2, codigo),
        }
    }

    /// Teclado numerico: digitos so com NumLock, operadores sempre.
    fn numerico(&self, codigo: u16) -> Option<u8> {
        if let Some(&(_, c)) = OPERADORES_NUMERICO.iter().find(|&&(k, _)| k == codigo) {
            return Some(c);
        }
        if !self.num {
            return None;
        }
        if let Some(&(_, c)) = NUMERICO.iter().find(|&&(k, _)| k == codigo) {
            return Some(c);
        }
        // no ABNT2 o ponto do teclado numerico e a virgula, e a tecla extra e o ponto
        match (codigo, self.layout) {
            (KEY_KPDOT, Layout::Us) => Some(b'.'),
            (KEY_KPDOT, Layout::Abnt2) => Some(b','),
            (KEY_KPCOMMA, Layout::Us) => Some(b','),
            (KEY_KPCOMMA, Layout::Abnt2) => Some(b'.'),
            _ => None,
        }
    }

    /// Caractere comum: resolve o acento pendente, se houver.
    fn combinar(&mut self, c: char, emitir: &mut dyn FnMut(char)) {
        let Some(morta) = self.morta.take() else {
            emitir(c);
            return;
        };
        if let Some(acentuada) = morta.aplicar(c) {
            emitir(acentuada);
            return;
        }
        match c {
            ' ' => emitir(morta.simbolo()),
            // Backspace e Esc cancelam o acento
            '\x08' | '\x1b' => emitir(c),
            _ => {
                emitir(morta.simbolo());
                emitir(c);
            }
        }
    }

    /// Tecla morta: fica pendente; duas seguidas saem as duas.
    fn nova_morta(&mut self, m: Morta, emitir: &mut dyn FnMut(char)) {
        match self.morta.take() {
            Some(anterior) => {
                emitir(anterior.simbolo());
                emitir(m.simbolo());
            }
            None => self.morta = Some(m),
        }
    }
}

/// Enter, Backspace, Tab, Esc e espaco (iguais em todos os layouts).
fn controle(codigo: u16) -> Option<u8> {
    match codigo {
        KEY_ENTER | KEY_KPENTER => Some(13),
        KEY_BACKSPACE => Some(8),
        KEY_TAB => Some(9),
        KEY_ESC => Some(27),
        KEY_SPACE => Some(b' '),
        _ => None,
    }
}

/// Letra minuscula da posicao (igual no americano e no ABNT2).
fn letra(codigo: u16) -> Option<u8> {
    let fileira = |inicio: u16, letras: &[u8]| {
        codigo.checked_sub(inicio).and_then(|i| letras.get(i as usize).copied())
    };
    fileira(KEY_Q, FILEIRA_Q).or_else(|| fileira(30, FILEIRA_A)).or_else(|| fileira(44, FILEIRA_Z))
}

/// Nome curto de teclas especiais (para depuracao e para os apps).
pub fn nome_tecla(codigo: u16) -> Option<&'static str> {
    let nome = match codigo {
        KEY_ESC => "Esc",
        KEY_ENTER => "Enter",
        KEY_BACKSPACE => "Backspace",
        KEY_TAB => "Tab",
        KEY_UP => "Cima",
        KEY_DOWN => "Baixo",
        KEY_LEFT => "Esquerda",
        KEY_RIGHT => "Direita",
        KEY_HOME => "Home",
        KEY_END => "End",
        KEY_PAGEUP => "PageUp",
        KEY_PAGEDOWN => "PageDown",
        KEY_INSERT => "Insert",
        KEY_DELETE => "Delete",
        KEY_F11 => "F11",
        KEY_F12 => "F12",
        _ => return None,
    };
    Some(nome)
}

/// Numero da tecla de funcao (F1 = 1 .. F12 = 12), se for uma.
pub fn tecla_de_funcao(codigo: u16) -> Option<u8> {
    match codigo {
        KEY_F1..=KEY_F10 => Some((codigo - KEY_F1 + 1) as u8),
        KEY_F11 => Some(11),
        KEY_F12 => Some(12),
        _ => None,
    }
}

