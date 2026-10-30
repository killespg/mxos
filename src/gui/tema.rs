//! Temas de cores da interface e medidas fixas das janelas.
//!
//! Um Tema e so uma tabela de cores (0x00RRGGBB). Estilo atual: a barra de
//! titulo e da mesma familia de cor da janela (sem degrade), cantos
//! redondos, sombra macia, uma cor de destaque. O padrao e escuro (a box
//! fica numa TV, vista de longe e com pouca luz); o Claro fica para quem
//! prefere.
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
use core::sync::atomic::{AtomicI32, Ordering};

use super::grafico::Cor;

pub const ALTURA_TITULO: i32 = 36;
pub const BORDA: i32 = 1;
pub const ALTURA_BARRA_TAREFAS: i32 = 48;

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
    /// Arrastar uma janela mostra so o contorno; ela vai para o lugar ao
    /// soltar (bem mais leve que redesenhar tudo a cada movimento).
    pub arrastar_contorno: bool,
    /// Fuso do relogio da barra, em horas (Brasilia = -3).
    pub fuso: i32,
}

/// A aparencia de fabrica (a mesma do config::Config::padrao).
pub const APARENCIA_PADRAO: Aparencia = Aparencia {
    papel: PapelDeParede::Gradiente,
    cor_papel: 0x0018_2448,
    relogio_segundos: true,
    icones: true,
    arrastar_contorno: true,
    fuso: -3,
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
pub const TEMAS: [(&str, Tema); 6] = [
    ("Grafite", GRAFITE),
    ("Claro", CLARO),
    ("Azul", AZUL),
    ("Verde", VERDE),
    ("Roxo", ROXO),
    ("Escuro", ESCURO),
];

/// Padrao: grafite neutro, janelas escuras, destaque azul.
const GRAFITE: Tema = Tema {
    area_cima: 0x000E_1116,
    area_baixo: 0x001B_2130,
    barra_tarefas: 0x0014_171D,
    barra_texto: 0x00E8_EBF1,
    titulo_ativo: 0x0027_2C36,
    titulo_inativo: 0x001F_232B,
    titulo_texto: 0x00E8_EBF1,
    borda: 0x0039_4050,
    fundo_janela: 0x001F_232B,
    texto: 0x00E8_EBF1,
    botao: 0x0031_3744,
    botao_destaque: 0x003D_4454,
    botao_texto: 0x00E8_EBF1,
    fechar: 0x00E5_484D,
    sombra: 0x0000_0000,
    destaque: 0x004C_8DFF,
};

/// Claro: janelas brancas, barra de titulo junto do conteudo.
const CLARO: Tema = Tema {
    area_cima: 0x00E9_EDF2,
    area_baixo: 0x00C9_D2DD,
    barra_tarefas: 0x00F3_F4F7,
    barra_texto: 0x001B_1F27,
    titulo_ativo: 0x00F7_F8FA,
    titulo_inativo: 0x00FF_FFFF,
    titulo_texto: 0x001B_1F27,
    borda: 0x00D3_D8E0,
    fundo_janela: 0x00FF_FFFF,
    texto: 0x001B_1F27,
    botao: 0x00EC_EFF3,
    botao_destaque: 0x00DC_E6FF,
    botao_texto: 0x001B_1F27,
    fechar: 0x00E5_484D,
    sombra: 0x002A_3240,
    destaque: 0x0025_63EB,
};

/// Azul noturno.
const AZUL: Tema = Tema {
    area_cima: 0x000B_1222,
    area_baixo: 0x001A_2B4A,
    barra_tarefas: 0x000E_1627,
    barra_texto: 0x00E6_EDF8,
    titulo_ativo: 0x001F_2D47,
    titulo_inativo: 0x0017_2238,
    titulo_texto: 0x00E6_EDF8,
    borda: 0x002E_4163,
    fundo_janela: 0x0017_2238,
    texto: 0x00E6_EDF8,
    botao: 0x0025_3858,
    botao_destaque: 0x0030_476E,
    botao_texto: 0x00E6_EDF8,
    fechar: 0x00E5_484D,
    sombra: 0x0000_0000,
    destaque: 0x003B_82F6,
};

/// Verde floresta escuro.
const VERDE: Tema = Tema {
    area_cima: 0x000A_1512,
    area_baixo: 0x0017_3229,
    barra_tarefas: 0x000D_1915,
    barra_texto: 0x00E4_F1EB,
    titulo_ativo: 0x001D_2F29,
    titulo_inativo: 0x0016_241F,
    titulo_texto: 0x00E4_F1EB,
    borda: 0x002D_4A40,
    fundo_janela: 0x0016_241F,
    texto: 0x00E4_F1EB,
    botao: 0x0023_3A31,
    botao_destaque: 0x002D_4A3E,
    botao_texto: 0x00E4_F1EB,
    fechar: 0x00E5_484D,
    sombra: 0x0000_0000,
    destaque: 0x0016_A34A,
};

/// Roxo noturno.
const ROXO: Tema = Tema {
    area_cima: 0x0012_0C1E,
    area_baixo: 0x002A_1E47,
    barra_tarefas: 0x0015_0F20,
    barra_texto: 0x00EE_E8F8,
    titulo_ativo: 0x002A_2140,
    titulo_inativo: 0x0021_1A33,
    titulo_texto: 0x00EE_E8F8,
    borda: 0x0043_355F,
    fundo_janela: 0x0021_1A33,
    texto: 0x00EE_E8F8,
    botao: 0x0032_2753,
    botao_destaque: 0x003F_3166,
    botao_texto: 0x00EE_E8F8,
    fechar: 0x00E5_484D,
    sombra: 0x0000_0000,
    destaque: 0x008B_5CF6,
};

/// Preto de verdade (bom em TV OLED).
const ESCURO: Tema = Tema {
    area_cima: 0x0000_0000,
    area_baixo: 0x0010_1012,
    barra_tarefas: 0x0000_0000,
    barra_texto: 0x00ED_EDED,
    titulo_ativo: 0x0018_181A,
    titulo_inativo: 0x000F_0F10,
    titulo_texto: 0x00ED_EDED,
    borda: 0x002C_2C30,
    fundo_janela: 0x000F_0F10,
    texto: 0x00ED_EDED,
    botao: 0x0022_2225,
    botao_destaque: 0x002E_2E33,
    botao_texto: 0x00ED_EDED,
    fechar: 0x00E5_484D,
    sombra: 0x0000_0000,
    destaque: 0x000A_84FF,
};

/// Guarda o tema e a aparencia atuais. Sync porque so o laco principal
/// usa (ver topo).
struct Atual(UnsafeCell<(Tema, Aparencia)>);

unsafe impl Sync for Atual {}

static ATUAL: Atual = Atual(UnsafeCell::new((GRAFITE, APARENCIA_PADRAO)));

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

/// Tamanho da tela (a area de trabalho conta na partida).
static TELA: (AtomicI32, AtomicI32) = (AtomicI32::new(1024), AtomicI32::new(768));

pub fn definir_tela(largura: i32, altura: i32) {
    TELA.0.store(largura, Ordering::Relaxed);
    TELA.1.store(altura, Ordering::Relaxed);
}

pub fn tamanho_da_tela() -> (i32, i32) {
    (TELA.0.load(Ordering::Relaxed), TELA.1.load(Ordering::Relaxed))
}
