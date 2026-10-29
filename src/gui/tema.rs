//! Temas de cores da interface e medidas fixas das janelas.
//!
//! Um Tema e so uma tabela de cores (0x00RRGGBB). Estilo "flat": cores
//! chapadas, contraste alto entre texto e fundo, uma cor de destaque.
//! Regras que todos os temas seguem, para quem desenha poder contar:
//!   - texto       legivel em fundo_janela
//!   - botao_texto legivel em botao E em botao_destaque
//!   - titulo_texto legivel em titulo_ativo E em titulo_inativo
//!   - barra_texto legivel em barra_tarefas
//!   - branco legivel em destaque e em fechar (selecao, botao [x])
//!
//! O tema atual e global. So o laco principal mexe nele (nunca uma
//! interrupcao), entao basta um UnsafeCell sem trava.
//!
//! A Aparencia (papel de parede, relogio, icones da area de trabalho) fica
//! separada do Tema: trocar as cores das janelas nao mexe no papel de
//! parede, e vice-versa. As duas sao lidas e gravadas pelo config.rs.

use core::cell::UnsafeCell;

use super::grafico::Cor;

pub const ALTURA_TITULO: i32 = 24;
pub const BORDA: i32 = 2;
pub const ALTURA_BARRA_TAREFAS: i32 = 32;

/// Desenho do papel de parede. Todos usam a cor de Aparencia::cor_papel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PapelDeParede {
    /// Degrade vertical, mais escuro em cima.
    Gradiente,
    /// Cor chapada.
    Solido,
    /// Faixas diagonais sobre o degrade.
    Listras,
    /// Quadrados alternados sobre o degrade.
    Xadrez,
    /// Grade de pontinhos sobre o degrade.
    Pontos,
}

impl PapelDeParede {
    pub const TODOS: [PapelDeParede; 5] = [
        PapelDeParede::Gradiente,
        PapelDeParede::Solido,
        PapelDeParede::Listras,
        PapelDeParede::Xadrez,
        PapelDeParede::Pontos,
    ];

    /// Nome usado no arquivo de configuracao (sem acento).
    pub fn nome(self) -> &'static str {
        match self {
            PapelDeParede::Gradiente => "gradiente",
            PapelDeParede::Solido => "solido",
            PapelDeParede::Listras => "listras",
            PapelDeParede::Xadrez => "xadrez",
            PapelDeParede::Pontos => "pontos",
        }
    }

    /// Nome para mostrar na tela (com acento; o arquivo fica com `nome`).
    pub fn rotulo(self) -> &'static str {
        match self {
            PapelDeParede::Gradiente => "Gradiente",
            PapelDeParede::Solido => "Sólido",
            PapelDeParede::Listras => "Listras",
            PapelDeParede::Xadrez => "Xadrez",
            PapelDeParede::Pontos => "Pontos",
        }
    }

    /// O contrario de `nome` (sem diferenciar maiusculas; aceita tambem o
    /// `rotulo`, com acento).
    pub fn por_nome(nome: &str) -> Option<PapelDeParede> {
        let nome = nome.trim();
        let igual = |p: &PapelDeParede| {
            p.nome().eq_ignore_ascii_case(nome) || p.rotulo().to_lowercase() == nome.to_lowercase()
        };
        PapelDeParede::TODOS.iter().copied().find(igual)
    }
}

/// O que a pessoa escolheu para a area de trabalho, fora as cores do tema.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Aparencia {
    pub papel: PapelDeParede,
    /// Cor base do papel de parede (0x00RRGGBB).
    pub cor_papel: Cor,
    /// Relogio da barra com segundos (HH:MM:SS) ou sem (HH:MM).
    pub relogio_segundos: bool,
    /// Icones (atalhos) na coluna da esquerda da area de trabalho.
    pub icones: bool,
}

/// A aparencia de fabrica (a mesma do config::Config::padrao).
pub const APARENCIA_PADRAO: Aparencia = Aparencia {
    papel: PapelDeParede::Gradiente,
    cor_papel: 0x0020_4060,
    relogio_segundos: true,
    icones: true,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Tema {
    /// Papel de parede: degrade de cima para baixo.
    pub area_cima: Cor,
    pub area_baixo: Cor,
    pub barra_tarefas: Cor,
    pub barra_texto: Cor,
    pub titulo_ativo: Cor,
    pub titulo_inativo: Cor,
    pub titulo_texto: Cor,
    pub borda: Cor,
    pub fundo_janela: Cor,
    pub texto: Cor,
    pub botao: Cor,
    /// Botao com o mouse em cima ou apertado.
    pub botao_destaque: Cor,
    pub botao_texto: Cor,
    /// Botao [x] de fechar.
    pub fechar: Cor,
    pub sombra: Cor,
    /// Selecao, item ativo, foco.
    pub destaque: Cor,
}

/// Os temas, com o nome que aparece nas configuracoes. O primeiro e o padrao.
pub const TEMAS: [(&str, Tema); 5] = [
    ("Azul", AZUL),
    ("Escuro", ESCURO),
    ("Claro", CLARO),
    ("Verde", VERDE),
    ("Roxo", ROXO),
];

/// Padrao: azul profundo com janelas claras.
const AZUL: Tema = Tema {
    area_cima: 0x0016_2B4E,
    area_baixo: 0x0038_6FA4,
    barra_tarefas: 0x0014_1B2A,
    barra_texto: 0x00E6_EBF2,
    titulo_ativo: 0x0026_5DC8,
    titulo_inativo: 0x0058_6274,
    titulo_texto: 0x00FF_FFFF,
    borda: 0x0020_2C40,
    fundo_janela: 0x00F4_F6FA,
    texto: 0x001E_2430,
    botao: 0x00DD_E3EC,
    botao_destaque: 0x00C2_D6F7,
    botao_texto: 0x001E_2430,
    fechar: 0x00C9_373D,
    sombra: 0x000A_1020,
    destaque: 0x002F_6FDB,
};

/// Tudo escuro, texto claro (paleta inspirada no "Nord").
const ESCURO: Tema = Tema {
    area_cima: 0x0018_1A1F,
    area_baixo: 0x0036_3B46,
    barra_tarefas: 0x0011_1216,
    barra_texto: 0x00D8_DEE9,
    titulo_ativo: 0x0043_4C5E,
    titulo_inativo: 0x0029_2D35,
    titulo_texto: 0x00EC_EFF4,
    borda: 0x000D_0E11,
    fundo_janela: 0x002B_2F38,
    texto: 0x00E5_E9F0,
    botao: 0x003B_4252,
    botao_destaque: 0x004C_566A,
    botao_texto: 0x00EC_EFF4,
    fechar: 0x00BF_4A55,
    sombra: 0x0005_0506,
    destaque: 0x0046_6F96,
};

/// Claro e discreto: cinzas frios, azul so no destaque.
const CLARO: Tema = Tema {
    area_cima: 0x00E3_E9EF,
    area_baixo: 0x00A9_BACB,
    barra_tarefas: 0x00F1_F3F5,
    barra_texto: 0x0020_2124,
    titulo_ativo: 0x00CF_DCEC,
    titulo_inativo: 0x00EC_EEF1,
    titulo_texto: 0x0020_2124,
    borda: 0x009A_A3AF,
    fundo_janela: 0x00FF_FFFF,
    texto: 0x0020_2124,
    botao: 0x00E6_E9ED,
    botao_destaque: 0x00CF_E0FB,
    botao_texto: 0x0020_2124,
    fechar: 0x00D3_2F2F,
    sombra: 0x0073_7D8A,
    destaque: 0x001A_67D2,
};

/// Verde floresta, janelas quase brancas.
const VERDE: Tema = Tema {
    area_cima: 0x0010_3A40,
    area_baixo: 0x004E_8F6A,
    barra_tarefas: 0x000E_2621,
    barra_texto: 0x00E3_F1EB,
    titulo_ativo: 0x0023_7A4F,
    titulo_inativo: 0x0058_7268,
    titulo_texto: 0x00FF_FFFF,
    borda: 0x001A_3A31,
    fundo_janela: 0x00F3_F8F5,
    texto: 0x001B_2A25,
    botao: 0x00D6_E6DD,
    botao_destaque: 0x00B4_DBC5,
    botao_texto: 0x001B_2A25,
    fechar: 0x00C2_403B,
    sombra: 0x0008_1814,
    destaque: 0x0023_7A4F,
};

/// Roxo noturno, janelas claras com toque lilas.
const ROXO: Tema = Tema {
    area_cima: 0x0024_1638,
    area_baixo: 0x0062_4A8E,
    barra_tarefas: 0x0019_0F26,
    barra_texto: 0x00EC_E6F5,
    titulo_ativo: 0x006F_4BB8,
    titulo_inativo: 0x0066_5E73,
    titulo_texto: 0x00FF_FFFF,
    borda: 0x002A_1A40,
    fundo_janela: 0x00F7_F4FB,
    texto: 0x0023_1A2F,
    botao: 0x00E3_DCEF,
    botao_destaque: 0x00D0_C3E8,
    botao_texto: 0x0023_1A2F,
    fechar: 0x00C9_373D,
    sombra: 0x0010_0919,
    destaque: 0x006F_4BB8,
};

/// Guarda o tema e a aparencia atuais. Sync porque so o laco principal
/// usa (ver topo).
struct Atual(UnsafeCell<(Tema, Aparencia)>);

unsafe impl Sync for Atual {}

static ATUAL: Atual = Atual(UnsafeCell::new((AZUL, APARENCIA_PADRAO)));

/// Copia do tema em uso.
pub fn atual() -> Tema {
    unsafe { (*ATUAL.0.get()).0 }
}

/// Troca o tema. Quem desenha precisa redesenhar tudo depois
/// (nos apps: Pedido::TemaMudou).
pub fn definir(t: Tema) {
    unsafe { (*ATUAL.0.get()).0 = t };
}

/// Tema pelo nome (sem diferenciar maiusculas), para ler das configuracoes.
pub fn por_nome(nome: &str) -> Option<Tema> {
    let nome = nome.trim();
    TEMAS.iter().find(|(n, _)| n.eq_ignore_ascii_case(nome)).map(|(_, t)| *t)
}

/// Nome do tema atual, se for um dos de TEMAS (para salvar a escolha).
pub fn nome_atual() -> Option<&'static str> {
    let agora = atual();
    TEMAS.iter().find(|(_, t)| *t == agora).map(|(n, _)| *n)
}

/// Copia da aparencia em uso.
pub fn aparencia() -> Aparencia {
    unsafe { (*ATUAL.0.get()).1 }
}

/// Troca a aparencia. Como no `definir`, quem desenha redesenha tudo depois.
pub fn definir_aparencia(a: Aparencia) {
    unsafe { (*ATUAL.0.get()).1 = a };
}
