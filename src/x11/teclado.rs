//! Mapa de teclas do X11: o keycode e o codigo evdev + 8 (como no Xorg com
//! o evdev), e cada tecla tem 2 keysyms (sem e com Shift), do teclado
//! americano. Programas que leem o texto pelo keysym (quase todos) veem as
//! letras certas; os acentos do ABNT2 ficam para depois.

pub const PRIMEIRO: u8 = 8;
pub const ULTIMO: u8 = 255;
pub const POR_TECLA: usize = 2;

/// (codigo evdev, sem shift, com shift)
const TECLAS: &[(u16, u32, u32)] = &[
    (1, 0xff1b, 0xff1b), // Esc
    (2, b'1' as u32, b'!' as u32),
    (3, b'2' as u32, b'@' as u32),
    (4, b'3' as u32, b'#' as u32),
    (5, b'4' as u32, b'$' as u32),
    (6, b'5' as u32, b'%' as u32),
    (7, b'6' as u32, b'^' as u32),
    (8, b'7' as u32, b'&' as u32),
    (9, b'8' as u32, b'*' as u32),
    (10, b'9' as u32, b'(' as u32),
    (11, b'0' as u32, b')' as u32),
    (12, b'-' as u32, b'_' as u32),
    (13, b'=' as u32, b'+' as u32),
    (14, 0xff08, 0xff08), // BackSpace
    (15, 0xff09, 0xfe20), // Tab, ISO_Left_Tab
    (16, b'q' as u32, b'Q' as u32),
    (17, b'w' as u32, b'W' as u32),
    (18, b'e' as u32, b'E' as u32),
    (19, b'r' as u32, b'R' as u32),
    (20, b't' as u32, b'T' as u32),
    (21, b'y' as u32, b'Y' as u32),
    (22, b'u' as u32, b'U' as u32),
    (23, b'i' as u32, b'I' as u32),
    (24, b'o' as u32, b'O' as u32),
    (25, b'p' as u32, b'P' as u32),
    (26, b'[' as u32, b'{' as u32),
    (27, b']' as u32, b'}' as u32),
    (28, 0xff0d, 0xff0d), // Return
    (29, 0xffe3, 0xffe3), // Control_L
    (30, b'a' as u32, b'A' as u32),
    (31, b's' as u32, b'S' as u32),
    (32, b'd' as u32, b'D' as u32),
    (33, b'f' as u32, b'F' as u32),
    (34, b'g' as u32, b'G' as u32),
    (35, b'h' as u32, b'H' as u32),
    (36, b'j' as u32, b'J' as u32),
    (37, b'k' as u32, b'K' as u32),
    (38, b'l' as u32, b'L' as u32),
    (39, b';' as u32, b':' as u32),
    (40, b'\'' as u32, b'"' as u32),
    (41, b'`' as u32, b'~' as u32),
    (42, 0xffe1, 0xffe1), // Shift_L
    (43, b'\\' as u32, b'|' as u32),
    (44, b'z' as u32, b'Z' as u32),
    (45, b'x' as u32, b'X' as u32),
    (46, b'c' as u32, b'C' as u32),
    (47, b'v' as u32, b'V' as u32),
    (48, b'b' as u32, b'B' as u32),
    (49, b'n' as u32, b'N' as u32),
    (50, b'm' as u32, b'M' as u32),
    (51, b',' as u32, b'<' as u32),
    (52, b'.' as u32, b'>' as u32),
    (53, b'/' as u32, b'?' as u32),
    (54, 0xffe2, 0xffe2), // Shift_R
    (55, 0xffaa, 0xffaa), // KP_Multiply
    (56, 0xffe9, 0xffe9), // Alt_L
    (57, b' ' as u32, b' ' as u32),
    (58, 0xffe5, 0xffe5), // Caps_Lock
    (59, 0xffbe, 0xffbe), // F1
    (60, 0xffbf, 0xffbf),
    (61, 0xffc0, 0xffc0),
    (62, 0xffc1, 0xffc1),
    (63, 0xffc2, 0xffc2),
    (64, 0xffc3, 0xffc3),
    (65, 0xffc4, 0xffc4),
    (66, 0xffc5, 0xffc5),
    (67, 0xffc6, 0xffc6),
    (68, 0xffc7, 0xffc7), // F10
    (69, 0xff7f, 0xff7f), // Num_Lock
    (71, 0xffb7, 0xff95), // KP_7 / KP_Home
    (72, 0xffb8, 0xff97),
    (73, 0xffb9, 0xff9a),
    (74, 0xffad, 0xffad), // KP_Subtract
    (75, 0xffb4, 0xff96),
    (76, 0xffb5, 0xff9d),
    (77, 0xffb6, 0xff98),
    (78, 0xffab, 0xffab), // KP_Add
    (79, 0xffb1, 0xff9c),
    (80, 0xffb2, 0xff99),
    (81, 0xffb3, 0xff9b),
    (82, 0xffb0, 0xff9e),
    (83, 0xffae, 0xff9f),
    (86, b'\\' as u32, b'|' as u32), // a tecla extra do ABNT2/ISO
    (87, 0xffc8, 0xffc8), // F11
    (88, 0xffc9, 0xffc9), // F12
    (89, b'/' as u32, b'?' as u32), // a do lado do Shift direito no ABNT2
    (96, 0xff8d, 0xff8d), // KP_Enter
    (97, 0xffe4, 0xffe4), // Control_R
    (98, 0xffaf, 0xffaf), // KP_Divide
    (100, 0xffea, 0xffea), // Alt_R
    (102, 0xff50, 0xff50), // Home
    (103, 0xff52, 0xff52), // Up
    (104, 0xff55, 0xff55), // Prior
    (105, 0xff51, 0xff51), // Left
    (106, 0xff53, 0xff53), // Right
    (107, 0xff57, 0xff57), // End
    (108, 0xff54, 0xff54), // Down
    (109, 0xff56, 0xff56), // Next
    (110, 0xff63, 0xff63), // Insert
    (111, 0xffff, 0xffff), // Delete
    (119, 0xff13, 0xff13), // Pause
    (125, 0xffeb, 0xffeb), // Super_L
    (126, 0xffec, 0xffec), // Super_R
    (127, 0xff67, 0xff67), // Menu
];

/// Os keysyms do keycode X `codigo` (0 = nenhum).
pub fn keysyms(codigo: u8) -> [u32; POR_TECLA] {
    let evdev = codigo.wrapping_sub(PRIMEIRO) as u16;
    TECLAS.iter().find(|t| t.0 == evdev).map_or([0, 0], |t| [t.1, t.2])
}

/// keycodes de cada modificador (Shift, Lock, Control, Mod1..Mod5), 2 por
/// modificador, para o GetModifierMapping.
pub const MODIFICADORES: [[u8; 2]; 8] = [
    [42 + 8, 54 + 8],  // Shift
    [58 + 8, 0],       // Lock
    [29 + 8, 97 + 8],  // Control
    [56 + 8, 100 + 8], // Mod1 (Alt)
    [69 + 8, 0],       // Mod2 (Num_Lock)
    [0, 0],
    [125 + 8, 126 + 8], // Mod4 (Super)
    [0, 0],
];

/// Bit do modificador no `state` dos eventos, para um codigo evdev.
pub fn bit_modificador(evdev: u16) -> u16 {
    match evdev {
        42 | 54 => 1,
        29 | 97 => 4,
        56 | 100 => 8,
        125 | 126 => 64,
        _ => 0,
    }
}
