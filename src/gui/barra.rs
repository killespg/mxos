//! Pecas comuns das janelas de ferramentas (Arquivos, Editor e os apps do
//! SDK): caminhos, barras de botoes, barra de estado e avisos.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::fonte::{self, INTER};
use crate::gui::grafico::{self, Cor, Retangulo, Superficie};
use crate::gui::janelas::EventoApp;
use crate::gui::tema::Tema;
use crate::gui::widgets::{self, ALTURA_BOTAO, AcaoCampo, CampoTexto, Icone, cor_linha};

/// Espaco entre a borda e os botoes das barras.
pub const MARGEM: i32 = 6;
/// Altura de cada barra de botoes.
pub const ALTURA_BARRA: i32 = ALTURA_BOTAO + 2 * MARGEM;
pub const ALTURA_ESTADO: i32 = 24;
/// Espaco entre dois botoes vizinhos.
pub const ESPACO: i32 = 4;

// ------------------------------------------- caminhos (editor tambem)

/// Caminho absoluto limpo: comeca com "/", sem barra no fim, "\" vira "/"
/// e "." e ".." sao resolvidos ("docs/../a/" -> "/a").
pub fn normalizar(caminho: &str) -> String {
    let mut partes: Vec<&str> = Vec::new();
    for parte in caminho.trim().split(['/', '\\']) {
        match parte {
            "" | "." => {}
            ".." => {
                partes.pop();
            }
            nome => partes.push(nome),
        }
    }
    if partes.is_empty() {
        return String::from("/");
    }
    let mut texto = String::new();
    for parte in partes {
        texto.push('/');
        texto.push_str(parte);
    }
    texto
}

/// `pasta` + "/" + `nome`, sem barra dupla na raiz.
pub fn juntar(pasta: &str, nome: &str) -> String {
    if pasta.ends_with('/') { format!("{pasta}{nome}") } else { format!("{pasta}/{nome}") }
}

/// Pasta que contem `caminho` (a da raiz e ela mesma).
pub fn pai_do_caminho(caminho: &str) -> String {
    match caminho.trim_end_matches('/').rfind('/') {
        Some(0) | None => String::from("/"),
        Some(i) => String::from(&caminho[..i]),
    }
}

/// Ultimo pedaco do caminho ("/a/b.txt" -> "b.txt"; "/" -> "/").
pub fn nome_do_caminho(caminho: &str) -> &str {
    let sem_barra = caminho.trim_end_matches('/');
    match sem_barra.rfind('/') {
        Some(i) => &sem_barra[i + 1..],
        None if sem_barra.is_empty() => "/",
        None => sem_barra,
    }
}

/// Bytes em KB ou MB, como o resto do sistema mostra ("ls", "df", Sobre).
pub fn tamanho_texto(bytes: u64) -> String {
    match bytes {
        b if b >= 10 << 20 => format!("{} MB", b >> 20),
        b if b >= 1 << 10 => format!("{} KB", b >> 10),
        1 => String::from("1 byte"),
        b => format!("{b} bytes"),
    }
}

/// "1 pasta", "3 pastas".
pub fn contagem(n: usize, um: &str, varios: &str) -> String {
    format!("{n} {}", if n == 1 { um } else { varios })
}

// ------------------------------------- barras e botoes (editor tambem)

/// Um botao de barra de ferramentas e onde ele esta agora.
pub struct BotaoBarra<A> {
    pub acao: A,
    pub r: Retangulo,
    pub icone: Option<Icone>,
    pub texto: &'static str,
}

/// Largura de um botao com `texto` (todos tem pelo menos 64 px).
pub fn largura_botao(texto: &str) -> i32 {
    (Superficie::largura_texto(texto, 1) + 24).max(64)
}

/// A acao do botao que fica em (x, y).
pub fn acao_em<A: Copy>(botoes: &[BotaoBarra<A>], x: i32, y: i32) -> Option<A> {
    botoes.iter().find(|b| b.r.contem(x, y)).map(|b| b.acao)
}

/// Confirmar e Cancelar encostados na direita de uma barra com os botoes
/// em `y`, numa janela de `largura` px.
pub fn botoes_dialogo<A>(confirmar: A, texto: &'static str, cancelar: A, y: i32, largura: i32) -> [BotaoBarra<A>; 2] {
    let lc = largura_botao("Cancelar");
    let c = Retangulo::novo(largura - MARGEM - lc, y, lc, ALTURA_BOTAO);
    let lo = largura_botao(texto);
    let o = Retangulo::novo(c.x - ESPACO - lo, y, lo, ALTURA_BOTAO);
    [
        BotaoBarra { acao: confirmar, r: o, icone: None, texto },
        BotaoBarra { acao: cancelar, r: c, icone: None, texto: "Cancelar" },
    ]
}

/// Desenha um botao de barra. So texto: o botao do kit de widgets. Com
/// icone: o fundo do kit e, por cima, o icone e o texto na cor que contrasta
/// com o fundo que ele pintou. `principal` = cor de destaque; apertado
/// escurece; desabilitado esmaece.
pub fn desenhar_botao<A>(s: &mut Superficie, b: &BotaoBarra<A>, principal: bool, apertado: bool, ativo: bool, t: &Tema) {
    let r = b.r;
    match b.icone {
        None => widgets::botao(s, r, b.texto, principal),
        Some(icone) => {
            widgets::botao(s, r, "", principal);
            let fundo = s.ler_pixel(r.x + r.largura / 2, r.y + r.altura / 2).unwrap_or(t.botao);
            let espaco = if b.texto.is_empty() { 0 } else { 6 };
            let texto = fonte::cortar(b.texto, r.largura - 28 - espaco, &INTER);
            let total = 16 + espaco + Superficie::largura_texto(texto, 1);
            let x = r.x + (r.largura - total) / 2;
            widgets::icone(s, x, r.y + (r.altura - 16) / 2, icone, 1);
            s.texto(x + 16 + espaco, r.y + (r.altura - 8) / 2, texto, texto_sobre(fundo, t.botao_texto), 1);
        }
    }
    if apertado {
        tingir(s, r, 0, 40);
    } else if !ativo {
        let fundo = s.ler_pixel(r.x + 2, r.y + r.altura / 2).unwrap_or(t.botao);
        tingir(s, r, fundo, 150);
    }
}

/// Puxa os pixels de `r` para `cor` (peso 0 a 255): botao apertado
/// (para o preto) ou que nao pode ser usado agora (para o fundo dele).
fn tingir(s: &mut Superficie, r: Retangulo, cor: Cor, peso: u32) {
    let r = r.intersecao(&s.area());
    let largura = s.largura();
    let pixels = s.pixels_mut();
    for y in r.y..r.baixo() {
        let linha = y as usize * largura;
        for p in &mut pixels[linha + r.x as usize..linha + r.direita() as usize] {
            *p = grafico::misturar(*p, cor, peso);
        }
    }
}

/// `preferida` se ela contrasta com `fundo`; senao quase preto ou branco.
pub fn texto_sobre(fundo: Cor, preferida: Cor) -> Cor {
    let claro = |c: Cor| ((c >> 16 & 0xFF) * 299 + (c >> 8 & 0xFF) * 587 + (c & 0xFF) * 114) / 1000 >= 140;
    match (claro(fundo), claro(preferida)) {
        (true, true) => 0x0020_2124,
        (false, false) => 0x00FF_FFFF,
        _ => preferida,
    }
}

/// Fundo das barras de ferramentas e de estado.
pub fn cor_barra(t: &Tema) -> Cor {
    grafico::misturar(t.fundo_janela, t.botao, 110)
}

/// Faixa de barra de ferramentas: fundo `cor` e uma linha fina embaixo.
pub fn pintar_barra(s: &mut Superficie, r: Retangulo, cor: Cor, t: &Tema) {
    s.retangulo(r, cor);
    s.retangulo(Retangulo::novo(r.x, r.baixo() - 1, r.largura, 1), cor_linha(t));
}

/// Fundo da barra de estado no pe da superficie. Devolve o y do texto.
pub fn barra_estado(s: &mut Superficie, t: &Tema) -> i32 {
    let (largura, altura) = (s.largura() as i32, s.altura() as i32);
    let r = Retangulo::novo(0, altura - ALTURA_ESTADO, largura, ALTURA_ESTADO);
    s.retangulo(r, cor_barra(t));
    s.retangulo(Retangulo::novo(0, r.y, largura, 1), cor_linha(t));
    r.y + (ALTURA_ESTADO - 8) / 2
}

/// Caixa no meio de `area` com um aviso ("Copiando ...", "Carregando ...").
pub fn desenhar_aviso(s: &mut Superficie, area: Retangulo, texto: &str, t: &Tema) {
    let largura = (Superficie::largura_texto(texto, 1) + 64).max(240).min(area.largura - 32);
    let r = Retangulo::novo(area.x + (area.largura - largura) / 2, area.y + (area.altura - 56) / 2, largura, 56);
    widgets::painel(s, r);
    s.retangulo(Retangulo::novo(r.x, r.y, r.largura, 3), t.destaque);
    let fundo = s.ler_pixel(r.x + 4, r.y + r.altura / 2).unwrap_or(t.fundo_janela);
    texto_centralizado(s, r, r.y + 26, texto, texto_sobre(fundo, t.texto));
}

/// Texto numa linha so, centralizado na largura de `area` (cortado se precisar).
pub fn texto_centralizado(s: &mut Superficie, area: Retangulo, y: i32, texto: &str, cor: Cor) {
    let texto = fonte::cortar(texto, area.largura - 16, &INTER);
    let x = area.x + (area.largura - Superficie::largura_texto(texto, 1)) / 2;
    s.texto(x, y, texto, cor, 1);
}

/// Entrega `e` a um campo de texto que fica em `area`. Devolve o que o
/// campo fez e se precisa redesenhar (foco, cursor ou texto mudaram).
pub fn evento_campo(campo: &mut CampoTexto, e: EventoApp, area: Retangulo) -> (AcaoCampo, bool) {
    let focado = campo.focado();
    let acao = campo.evento(e, area);
    let tocou = match e {
        EventoApp::Botao { x, y, .. } => area.contem(x, y),
        EventoApp::Caractere(_) | EventoApp::Tecla { .. } => focado,
        _ => false,
    };
    (acao, tocou || acao != AcaoCampo::Nada || focado != campo.focado())
}
