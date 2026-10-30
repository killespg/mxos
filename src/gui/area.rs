//! Area de trabalho: papel de parede, atalhos, barra de tarefas e menu
//! iniciar.
//!
//! A barra (botao MXOS, um botao por janela, relogio), o menu e cada
//! atalho sao desenhados numa Superficie propria so quando mudam. Na hora
//! de montar a cena o gerenciador so copia o pedaco que precisa (pintar
//! com recorte).
//!
//! O papel de parede nao tem buffer: e calculado linha a linha a partir da
//! Aparencia (tema::aparencia), barato de refazer em qualquer pedaco da
//! tela. O desenho segue as coordenadas da tela, entao um pedaco pintado
//! numa Superficie separada (o fundo de um atalho) encaixa no resto.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::grafico::{self, Cor, Retangulo, Superficie};
use super::janelas::App;
use crate::fonte::{self, INTER, INTER_NEGRITO, INTER_TITULO};
use super::tema::{self, ALTURA_BARRA_TAREFAS, Aparencia, PapelDeParede, Tema};
use super::widgets::{self, DuploClique, Icone};

/// Versao mostrada no menu iniciar (a do Cargo.toml, sem o terceiro numero).
const VERSAO: &str = concat!(env!("CARGO_PKG_VERSION_MAJOR"), ".", env!("CARGO_PKG_VERSION_MINOR"));

// Medidas da barra de tarefas (em pixels, relativas a barra).
const MARGEM_BARRA: i32 = 3;
const LARGURA_INICIAR: i32 = 84;
/// Hora em cima e data embaixo, encostadas na direita.
const LARGURA_RELOGIO: i32 = 104;
/// Raio dos cantos dos botoes da barra.
const RAIO_BARRA: i32 = 4;
const LARGURA_MAX_BOTAO: i32 = 170;
const LARGURA_MIN_BOTAO: i32 = 28;
const ESPACO_BOTOES: i32 = 4;

// Medidas do menu iniciar.
const LARGURA_MENU: i32 = 224;
const ALTURA_CABECALHO: i32 = 40;
const ALTURA_ITEM: i32 = 28;

// Desenhos do papel de parede (em pixels da tela).
const LADO_XADREZ: i32 = 32;
const FAIXA_LISTRAS: i32 = 16;
const PASSO_PONTOS: i32 = 24;

// Atalhos da area de trabalho: uma coluna de celulas a esquerda.
const MARGEM_ATALHOS: i32 = 12;
const LARGURA_ATALHO: i32 = 112;
const ALTURA_ATALHO: i32 = 64;
const ESPACO_ATALHOS: i32 = 8;
const ESCALA_ATALHO: i32 = 2;
const TOPO_ICONE_ATALHO: i32 = 6;
const COR_NOME_ATALHO: Cor = 0x00FF_FFFF;
const SOMBRA_NOME_ATALHO: Cor = 0x0010_1418;

/// Cria um app novo (entrada do menu iniciar ou atalho).
pub type Criador = fn() -> Box<dyn App>;

/// Clareia cada canal de `cor` em `quanto` (satura em 255).
pub fn clarear(cor: Cor, quanto: u32) -> Cor {
    grafico::misturar(cor, 0x00FF_FFFF, quanto)
}

/// Escurece `cor` (quanto: 0 = igual, 255 = preto).
pub fn escurecer(cor: Cor, quanto: u32) -> Cor {
    grafico::misturar(cor, 0, quanto)
}

/// Multiplica cada canal por fator/256 (satura em 255). Clareia ou
/// escurece sem desbotar (misturar com branco tira a saturacao).
fn escalar(cor: Cor, fator: u32) -> Cor {
    let canal = |deslocamento: u32| ((cor >> deslocamento & 0xFF) * fator / 256).min(255);
    canal(16) << 16 | canal(8) << 8 | canal(0)
}

/// Mistura `cor` com o que ja esta em `r` (alfa 0 a 255), pixel a pixel.
pub fn tingir(s: &mut Superficie, r: Retangulo, cor: Cor, alfa: u32) {
    let r = r.intersecao(&s.area());
    let largura = s.largura();
    let pixels = s.pixels_mut();
    for y in r.y..r.baixo() {
        let linha = y as usize * largura;
        for p in &mut pixels[linha + r.x as usize..linha + r.direita() as usize] {
            *p = grafico::misturar(*p, cor, alfa);
        }
    }
}

/// O comeco de `texto` que cabe em `largura` pixels, medido na fonte que
/// Superficie::texto usa nessa `escala`. Corta sempre numa fronteira de
/// caractere.
pub fn cortar(texto: &str, largura: i32, escala: usize) -> &str {
    fonte::cortar(texto, largura, fonte::por_escala(escala))
}

/// Pinta a parte de `r` que cai dentro de `recorte`.
pub fn preencher(s: &mut Superficie, r: Retangulo, recorte: Retangulo, cor: Cor) {
    let alvo = r.intersecao(&recorte);
    if !alvo.vazio() {
        s.retangulo(alvo, cor);
    }
}

/// Copia `origem` (com o canto em (x, y) da tela) para `destino`, so no
/// pedaco que cai dentro de `recorte`.
pub fn copiar_recortado(origem: &Superficie, x: i32, y: i32, recorte: Retangulo, destino: &mut Superficie) {
    let inteira = Retangulo::novo(x, y, origem.largura() as i32, origem.altura() as i32);
    let alvo = inteira.intersecao(&recorte);
    if !alvo.vazio() {
        origem.copiar_regiao_para(alvo.deslocado(-x, -y), destino, alvo.x, alvo.y);
    }
}

/// Segundos -> "HH:MM:SS" (as horas passam de 99 sem cortar).
pub fn relogio(segundos: u64) -> String {
    format!("{:02}:{:02}:{:02}", segundos / 3600 % 24, segundos / 60 % 60, segundos % 60)
}

/// Segundos -> "HH:MM".
pub fn relogio_curto(segundos: u64) -> String {
    format!("{:02}:{:02}", segundos / 3600 % 24, segundos / 60 % 60)
}

/// Segundos desde 1970 na hora local (o RTC lido no boot mais o contador,
/// com o fuso da aparencia).
pub fn agora_local() -> u64 {
    let freq = crate::cpu::frequencia_timer().max(1);
    let utc = crate::processo::rtc_no_boot() + crate::cpu::contador() / freq;
    (utc as i64 + tema::aparencia().fuso as i64 * 3600).max(0) as u64
}

/// Segundos locais -> "qua, 29/10/2026".
pub fn data(segundos: u64) -> String {
    const DIAS: [&str; 7] = ["qui", "sex", "sáb", "dom", "seg", "ter", "qua"];
    let dias = segundos / 86_400;
    let (ano, mes, dia) = civil(dias as i64);
    format!("{}, {dia:02}/{mes:02}/{ano}", DIAS[(dias % 7) as usize])
}

/// Dias desde 1970-01-01 -> (ano, mes, dia), pelo calendario gregoriano
/// (algoritmo de Howard Hinnant, so com inteiros).
pub fn civil(dias: i64) -> (i64, u32, u32) {
    let z = dias + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let dia = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let mes = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let ano = yoe + era * 400 + (mes <= 2) as i64;
    (ano, mes, dia)
}

// --------------------------------------------------------- papel de parede

/// Imagem do papel de parede ja no tamanho da tela (preenche e corta o que
/// sobra no centro): o fundo so copia pixels. So o laco principal mexe.
struct ImagemPapel(core::cell::UnsafeCell<Option<(i32, i32, Vec<u32>)>>);

unsafe impl Sync for ImagemPapel {}

static IMAGEM_PAPEL: ImagemPapel = ImagemPapel(core::cell::UnsafeCell::new(None));

/// Le, decodifica (JPEG, PNG, GIF, BMP) e ajusta `caminho` a tela.
/// `None` tira a imagem e volta ao papel desenhado.
pub fn definir_imagem_papel(caminho: Option<&str>) -> Result<(), &'static str> {
    let Some(caminho) = caminho else {
        unsafe { *IMAGEM_PAPEL.0.get() = None };
        return Ok(());
    };
    let (l, a) = tema::tamanho_da_tela();
    let dados = crate::armazenamento::ler(caminho).map_err(|e| e.descricao())?;
    // decodifica ja perto do tamanho final (o JPEG reduz por 1/2 a 1/8)
    let img = crate::web::imagem::decodificar_max(&dados, l * 2, a * 2).ok_or("formato de imagem não reconhecido")?;
    drop(dados);
    if img.largura <= 0 || img.altura <= 0 {
        return Err("imagem vazia");
    }
    // preencher: a menor escala que cobre a tela; o excesso sai do centro
    let (il, ia) = (img.largura as i64, img.altura as i64);
    let (tl, ta) = (l as i64, a as i64);
    let (num, den) = if il * ta > ia * tl { (ia, ta) } else { (il, tl) }; // fonte por pixel da tela
    let (cx, cy) = ((il - tl * num / den) / 2, (ia - ta * num / den) / 2);
    let mut px = Vec::with_capacity((l * a) as usize);
    for y in 0..ta {
        let sy = (cy + y * num / den).clamp(0, ia - 1);
        for x in 0..tl {
            let sx = (cx + x * num / den).clamp(0, il - 1);
            px.push(img.px[(sy * il + sx) as usize] & 0x00FF_FFFF);
        }
    }
    unsafe { *IMAGEM_PAPEL.0.get() = Some((l, a, px)) };
    Ok(())
}

/// Copia as linhas da imagem do papel de parede; false = sem imagem.
fn pintar_imagem(s: &mut Superficie, area: Retangulo, origem: (i32, i32), altura_area: i32) -> bool {
    let Some((l, a, px)) = (unsafe { &*IMAGEM_PAPEL.0.get() }) else { return false };
    let largura_s = s.largura() as i32;
    let destino = s.pixels_mut();
    for y in area.y..area.baixo() {
        let y_tela = y + origem.1;
        if y_tela >= altura_area || y_tela >= *a {
            break;
        }
        if y_tela < 0 {
            continue;
        }
        let x0 = (area.x + origem.0).max(0);
        let x1 = (area.direita() + origem.0).min(*l);
        if x1 <= x0 {
            continue;
        }
        let linha = &px[(y_tela * l + x0) as usize..(y_tela * l + x1) as usize];
        let d = (y * largura_s + x0 - origem.0) as usize;
        destino[d..d + linha.len()].copy_from_slice(linha);
    }
    true
}

/// Papel de parede (da aparencia atual) dentro de `area`, so nas linhas
/// acima de `altura_area` (o resto e da barra de tarefas). As cores vem da
/// Aparencia, nao do tema `_t` (o parametro ficou da versao 0.6).
#[allow(dead_code)] // para apps que mostram uma previa do papel de parede
pub fn pintar_fundo(s: &mut Superficie, area: Retangulo, altura_area: i32, _t: &Tema) {
    pintar_papel(s, area, (0, 0), altura_area, &tema::aparencia());
}

/// Papel de parede `ap` na parte `area` de `s`, com o canto de `s` no
/// ponto `origem` da tela. So nas linhas da tela acima de `altura_area`.
pub fn pintar_papel(s: &mut Superficie, area: Retangulo, origem: (i32, i32), altura_area: i32, ap: &Aparencia) {
    let area = area.intersecao(&s.area());
    if area.vazio() {
        return;
    }
    if pintar_imagem(s, area, origem, altura_area) {
        return;
    }
    // degrade da propria cor: mais escura em cima, mais clara embaixo
    let (topo, base) = (escalar(ap.cor_papel, 176), escalar(ap.cor_papel, 435));
    let passos = (altura_area - 1).max(1);
    for y in area.y..area.baixo() {
        let y_tela = y + origem.1;
        if y_tela >= altura_area {
            break;
        }
        let fundo = match ap.papel {
            PapelDeParede::Solido => ap.cor_papel,
            _ => grafico::misturar(topo, base, (y_tela.clamp(0, passos) * 255 / passos) as u32),
        };
        let linha = Retangulo::novo(area.x, y, area.largura, 1);
        s.retangulo(linha, fundo);
        match ap.papel {
            PapelDeParede::Gradiente | PapelDeParede::Solido => {}
            PapelDeParede::Listras => listras(s, linha, origem.0, y_tela, clarear(fundo, 14)),
            PapelDeParede::Xadrez => xadrez(s, linha, origem.0, y_tela, clarear(fundo, 12)),
            PapelDeParede::Pontos => pontos(s, linha, origem.0, y_tela, clarear(fundo, 48)),
        }
    }
}

/// Faixas diagonais: claro onde (x + y da tela) cai na primeira metade de
/// cada periodo. `linha` tem 1 pixel de altura; `ox` leva x de `s` para a tela.
fn listras(s: &mut Superficie, linha: Retangulo, ox: i32, y_tela: i32, cor: Cor) {
    let periodo = 2 * FAIXA_LISTRAS;
    let x_tela = linha.x + ox;
    let mut inicio = x_tela - (x_tela + y_tela).rem_euclid(periodo);
    while inicio < x_tela + linha.largura {
        preencher(s, Retangulo::novo(inicio - ox, linha.y, FAIXA_LISTRAS, 1), linha, cor);
        inicio += periodo;
    }
}

/// Tabuleiro: quadrados claros onde coluna + fileira e impar.
fn xadrez(s: &mut Superficie, linha: Retangulo, ox: i32, y_tela: i32, cor: Cor) {
    let fileira = y_tela.div_euclid(LADO_XADREZ);
    let x_tela = linha.x + ox;
    let mut coluna = x_tela.div_euclid(LADO_XADREZ);
    while coluna * LADO_XADREZ < x_tela + linha.largura {
        if (coluna + fileira).rem_euclid(2) == 1 {
            preencher(s, Retangulo::novo(coluna * LADO_XADREZ - ox, linha.y, LADO_XADREZ, 1), linha, cor);
        }
        coluna += 1;
    }
}

/// Pontinhos de 2x2 no meio de cada celula de PASSO_PONTOS.
fn pontos(s: &mut Superficie, linha: Retangulo, ox: i32, y_tela: i32, cor: Cor) {
    let meio = PASSO_PONTOS / 2;
    if (y_tela - meio).rem_euclid(PASSO_PONTOS) >= 2 {
        return;
    }
    let x_tela = linha.x + ox;
    let mut x = (x_tela - meio).div_euclid(PASSO_PONTOS) * PASSO_PONTOS + meio;
    while x < x_tela + linha.largura {
        preencher(s, Retangulo::novo(x - ox, linha.y, 2, 1), linha, cor);
        x += PASSO_PONTOS;
    }
}

/// Botao chapado com texto centralizado (barra de tarefas, menu, apps).
pub fn botao(s: &mut Superficie, r: Retangulo, texto: &str, fundo: Cor, frente: Cor, borda: Cor, escala: usize) {
    s.retangulo(r, fundo);
    s.contorno(r, borda, 1);
    let f = fonte::por_escala(escala);
    widgets::texto_centrado(s, r, cortar(texto, r.largura - 8, escala), frente, f);
}

// --------------------------------------------------------- barra de tarefas

/// O que foi clicado na barra de tarefas.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AlvoBarra {
    Iniciar,
    Janela(usize),
}

/// Uma janela como aparece na barra.
pub struct ItemBarra<'a> {
    pub id: usize,
    pub titulo: &'a str,
    pub focada: bool,
    pub minimizada: bool,
}

/// Barra de tarefas no pe da tela.
pub struct BarraTarefas {
    superficie: Superficie,
    /// Linha da tela onde a barra comeca.
    y: i32,
    /// Botoes das janelas (coordenadas da tela), do ultimo desenho.
    botoes: Vec<(Retangulo, usize)>,
    suja: bool,
}

impl BarraTarefas {
    pub fn nova(largura: i32, altura_tela: i32, t: &Tema) -> BarraTarefas {
        let largura = largura.max(1) as usize;
        BarraTarefas {
            superficie: Superficie::nova(largura, ALTURA_BARRA_TAREFAS as usize, t.barra_tarefas),
            y: altura_tela - ALTURA_BARRA_TAREFAS,
            botoes: Vec::new(),
            suja: true,
        }
    }

    pub fn retangulo(&self) -> Retangulo {
        Retangulo::novo(0, self.y, self.superficie.largura() as i32, ALTURA_BARRA_TAREFAS)
    }

    pub fn sujar(&mut self) {
        self.suja = true;
    }

    pub fn suja(&self) -> bool {
        self.suja
    }

    /// Botao MXOS, relativo a barra.
    fn iniciar(&self) -> Retangulo {
        Retangulo::novo(MARGEM_BARRA + 1, MARGEM_BARRA, LARGURA_INICIAR, ALTURA_BARRA_TAREFAS - 2 * MARGEM_BARRA)
    }

    /// Caixa do relogio, relativa a barra.
    fn caixa_relogio(&self) -> Retangulo {
        let largura = self.superficie.largura() as i32;
        let x = largura - LARGURA_RELOGIO - MARGEM_BARRA - 1;
        Retangulo::novo(x, MARGEM_BARRA, LARGURA_RELOGIO, ALTURA_BARRA_TAREFAS - 2 * MARGEM_BARRA)
    }

    /// Fundo da barra (um degrade bem leve) na parte `r` dela.
    fn pintar_fundo(&mut self, r: Retangulo, t: &Tema) {
        let area = self.superficie.area();
        let (cima, baixo) = (clarear(t.barra_tarefas, 14), t.barra_tarefas);
        for y in r.y.max(0)..r.baixo().min(area.altura) {
            let cor = grafico::misturar(cima, baixo, (y * 255 / area.altura.max(1)) as u32);
            self.superficie.retangulo(Retangulo::novo(r.x, y, r.largura, 1), cor);
        }
        if r.y == 0 {
            self.superficie.retangulo(Retangulo::novo(r.x, 0, r.largura, 1), clarear(t.barra_tarefas, 50));
        }
    }

    /// Redesenha a barra inteira na propria Superficie.
    pub fn desenhar(&mut self, itens: &[ItemBarra], hora: &str, data: &str, menu_aberto: bool, t: &Tema) {
        let area = self.superficie.area();
        self.pintar_fundo(area, t);

        let iniciar = self.iniciar();
        if menu_aberto {
            self.superficie.retangulo_arredondado(iniciar, RAIO_BARRA, clarear(t.barra_tarefas, 40));
        }
        desenhar_logo(&mut self.superficie, iniciar, t.barra_texto, t);

        self.pintar_relogio(hora, data, t);
        let caixa = self.caixa_relogio();
        // risquinho separando os botoes do relogio
        self.superficie.retangulo(Retangulo::novo(caixa.x - 8, 10, 1, area.altura - 20), clarear(t.barra_tarefas, 30));
        self.desenhar_botoes(itens, iniciar.direita() + 8, caixa.x - 16, t);
        self.suja = false;
    }

    /// So o relogio (a cada segundo ou minuto): devolve o retangulo na
    /// tela que mudou, para sujar so ele e nao a barra toda.
    pub fn atualizar_relogio(&mut self, hora: &str, data: &str, t: &Tema) -> Retangulo {
        self.pintar_relogio(hora, data, t);
        self.caixa_relogio().deslocado(0, self.y)
    }

    /// Hora em cima (seminegrito) e data embaixo, alinhadas a direita.
    fn pintar_relogio(&mut self, hora: &str, data: &str, t: &Tema) {
        let caixa = self.caixa_relogio();
        self.pintar_fundo(caixa, t);
        let meio = caixa.y + caixa.altura / 2;
        let direita = caixa.direita() - 6;
        let x_hora = direita - INTER_NEGRITO.largura(hora);
        self.superficie.texto_com(x_hora, meio - 3, hora, t.barra_texto, &INTER_NEGRITO);
        let x_data = direita - INTER.largura(data);
        let cor_data = grafico::misturar(t.barra_texto, t.barra_tarefas, 70);
        self.superficie.texto_com(x_data, meio + 12, data, cor_data, &INTER);
    }

    /// Um botao por janela entre `inicio` e `fim` (x relativos a barra).
    fn desenhar_botoes(&mut self, itens: &[ItemBarra], inicio: i32, fim: i32, t: &Tema) {
        self.botoes.clear();
        if itens.is_empty() {
            return;
        }
        let n = itens.len() as i32;
        let livre = fim - inicio - (n - 1) * ESPACO_BOTOES;
        let largura = (livre / n).clamp(LARGURA_MIN_BOTAO, LARGURA_MAX_BOTAO);
        let altura = ALTURA_BARRA_TAREFAS - 2 * MARGEM_BARRA;
        for (i, item) in itens.iter().enumerate() {
            let x = inicio + i as i32 * (largura + ESPACO_BOTOES);
            if x + largura > fim {
                break; // janelas demais: as ultimas ficam sem botao
            }
            let r = Retangulo::novo(x, MARGEM_BARRA, largura, altura);
            // botao plano: so a janela ativa tem fundo; o tracinho embaixo
            // diz quem esta aberta (comprido = ativa, curto = aberta)
            let fundo = match (item.focada, item.minimizada) {
                (true, _) => Some(clarear(t.barra_tarefas, 34)),
                (false, false) => Some(clarear(t.barra_tarefas, 14)),
                (false, true) => None,
            };
            if let Some(cor) = fundo {
                self.superficie.retangulo_arredondado(r, RAIO_BARRA, cor);
            }
            let (traco, cor_traco) = if item.focada {
                (r.largura - 2 * RAIO_BARRA - 4, t.destaque)
            } else {
                (14, clarear(t.barra_tarefas, 90))
            };
            self.superficie.retangulo_arredondado(Retangulo::novo(r.x + (r.largura - traco) / 2, r.baixo() - 3, traco, 3), 1, cor_traco);
            let cor_texto = if item.minimizada { grafico::misturar(t.barra_texto, t.barra_tarefas, 110) } else { t.barra_texto };
            let texto = fonte::encurtar(item.titulo, r.largura - 18, &INTER);
            let faixa = Retangulo::novo(r.x, r.y, r.largura, r.altura - 3);
            widgets::texto_na_faixa(&mut self.superficie, r.x + 9, faixa, &texto, cor_texto, &INTER);
            self.botoes.push((r.deslocado(0, self.y), item.id));
        }
    }

    /// Copia para a cena o pedaco da barra que cai em `area`.
    pub fn pintar(&self, cena: &mut Superficie, area: Retangulo) {
        copiar_recortado(&self.superficie, 0, self.y, area, cena);
    }

    /// O que fica no ponto (x, y) da tela.
    pub fn alvo_em(&self, x: i32, y: i32) -> Option<AlvoBarra> {
        if self.iniciar().deslocado(0, self.y).contem(x, y) {
            return Some(AlvoBarra::Iniciar);
        }
        self.botoes.iter().find(|(r, _)| r.contem(x, y)).map(|&(_, id)| AlvoBarra::Janela(id))
    }
}

/// Botao Iniciar: um quadradinho na cor de destaque e "MXOS" em
/// seminegrito, os dois juntos no meio do botao.
fn desenhar_logo(s: &mut Superficie, r: Retangulo, cor: Cor, t: &Tema) {
    const LADO: i32 = 10;
    const ESPACO: i32 = 7;
    let largura = LADO + ESPACO + INTER_NEGRITO.largura("MXOS");
    let x = r.x + (r.largura - largura) / 2;
    let y = r.y + (r.altura - LADO) / 2;
    for (dx, dy) in [(0, 0), (6, 0), (0, 6), (6, 6)] {
        s.retangulo(Retangulo::novo(x + dx, y + dy, 4, 4), t.destaque);
    }
    widgets::texto_na_faixa(s, x + LADO + ESPACO, r, "MXOS", cor, &INTER_NEGRITO);
}

// ------------------------------------------------------------ menu iniciar

/// Uma entrada do menu iniciar.
struct ItemMenu {
    nome: &'static str,
    icone: Option<Icone>,
    criar: Criador,
}

/// Menu iniciar: lista dos apps registrados, em cima do botao MXOS.
pub struct MenuIniciar {
    itens: Vec<ItemMenu>,
    /// Desenho pronto do menu (refeito quando muda o destaque ou os itens).
    superficie: Option<Superficie>,
    aberto: bool,
    destacado: Option<usize>,
    sujo: bool,
    /// Linha da tela onde o menu termina (topo da barra de tarefas).
    base: i32,
}

impl MenuIniciar {
    pub fn novo(base: i32) -> MenuIniciar {
        MenuIniciar { itens: Vec::new(), superficie: None, aberto: false, destacado: None, sujo: true, base }
    }

    /// Entrada nova no fim do menu (sem icone: um quadradinho no lugar).
    pub fn adicionar(&mut self, nome: &'static str, icone: Option<Icone>, criar: Criador) {
        self.itens.push(ItemMenu { nome, icone, criar });
        self.superficie = None; // o tamanho mudou
        self.sujo = true;
    }

    pub fn aberto(&self) -> bool {
        self.aberto
    }

    pub fn abrir(&mut self) {
        self.aberto = true;
        self.destacado = None;
        self.sujo = true;
    }

    pub fn fechar(&mut self) {
        self.aberto = false;
    }

    pub fn sujar(&mut self) {
        self.sujo = true;
    }

    /// Onde o menu fica na tela (com pelo menos uma linha, mesmo vazio).
    pub fn retangulo(&self) -> Retangulo {
        let linhas = self.itens.len().max(1) as i32;
        let altura = ALTURA_CABECALHO + linhas * ALTURA_ITEM + 6;
        Retangulo::novo(0, self.base - altura, LARGURA_MENU, altura)
    }

    /// Linha do item em (x, y) da tela.
    fn indice_em(&self, x: i32, y: i32) -> Option<usize> {
        let r = self.retangulo();
        if !r.contem(x, y) {
            return None;
        }
        let dy = y - r.y - ALTURA_CABECALHO - 3;
        if dy < 0 {
            return None;
        }
        let i = (dy / ALTURA_ITEM) as usize;
        (i < self.itens.len()).then_some(i)
    }

    /// O mouse passou em (x, y): muda o destaque. Devolve true se o menu
    /// precisa ser redesenhado.
    pub fn passar(&mut self, x: i32, y: i32) -> bool {
        let novo = self.indice_em(x, y);
        if novo == self.destacado {
            return false;
        }
        self.destacado = novo;
        self.sujo = true;
        true
    }

    /// O app do item em (x, y), se houver.
    pub fn item_em(&self, x: i32, y: i32) -> Option<Criador> {
        self.indice_em(x, y).map(|i| self.itens[i].criar)
    }

    /// Refaz o desenho se mudou. Devolve true se refez.
    pub fn preparar(&mut self, t: &Tema) -> bool {
        if !self.sujo && self.superficie.is_some() {
            return false;
        }
        let r = self.retangulo();
        let mut s = match self.superficie.take() {
            Some(s) => s,
            None => Superficie::nova(r.largura as usize, r.altura as usize, t.fundo_janela),
        };
        self.desenhar(&mut s, t);
        self.superficie = Some(s);
        self.sujo = false;
        true
    }

    fn desenhar(&self, s: &mut Superficie, t: &Tema) {
        let area = s.area();
        s.limpar(t.fundo_janela);
        let cabecalho = Retangulo::novo(0, 0, area.largura, ALTURA_CABECALHO);
        s.gradiente_vertical(cabecalho, clarear(t.titulo_ativo, 40), t.titulo_ativo);
        let base = INTER_TITULO.base_centralizada(0, ALTURA_CABECALHO);
        let largura = s.texto_com(12, base, "MXOS", t.titulo_texto, &INTER_TITULO);
        let versao = format!("versão {VERSAO}");
        s.texto_com(12 + largura + 8, base, &versao, grafico::misturar(t.titulo_texto, t.titulo_ativo, 70), &INTER);
        if self.itens.is_empty() {
            let linha = Retangulo::novo(12, ALTURA_CABECALHO + 3, area.largura - 24, ALTURA_ITEM);
            widgets::texto_na_faixa(s, linha.x, linha, "(nenhum app)", t.texto, &INTER);
        }
        for (i, item) in self.itens.iter().enumerate() {
            let y = ALTURA_CABECALHO + 3 + i as i32 * ALTURA_ITEM;
            let linha = Retangulo::novo(3, y, area.largura - 6, ALTURA_ITEM);
            let destacado = self.destacado == Some(i);
            let (fundo, frente) = if destacado { (t.destaque, 0x00FF_FFFF) } else { (t.fundo_janela, t.texto) };
            s.retangulo(linha, fundo);
            match item.icone {
                Some(icone) => widgets::icone(s, linha.x + 6, y + (ALTURA_ITEM - 16) / 2, icone, 1),
                // sem icone: um quadradinho no lugar
                None => s.retangulo(Retangulo::novo(linha.x + 10, y + (ALTURA_ITEM - 8) / 2, 8, 8), if destacado { frente } else { t.destaque }),
            }
            let nome = fonte::encurtar(item.nome, linha.largura - 38, &INTER);
            widgets::texto_na_faixa(s, linha.x + 32, linha, &nome, frente, &INTER);
        }
        s.contorno(area, t.borda, 1);
    }

    /// Copia para a cena o pedaco do menu que cai em `area`.
    pub fn pintar(&self, cena: &mut Superficie, area: Retangulo) {
        if let Some(s) = &self.superficie {
            let r = self.retangulo();
            copiar_recortado(s, r.x, r.y, area, cena);
        }
    }
}

// ----------------------------------------------------------------- atalhos

/// Um icone da area de trabalho e o desenho pronto da celula dele (com o
/// papel de parede embaixo, para nao precisar de transparencia).
struct Atalho {
    nome: &'static str,
    icone: Icone,
    criar: Criador,
    desenho: Superficie,
    sujo: bool,
}

/// Atalhos da area de trabalho, numa coluna a esquerda: um clique escolhe,
/// arrastar no papel de parede escolhe varios (selecao em area), clique
/// duplo (ou Enter com nenhuma janela focada) abre o app.
pub struct Atalhos {
    itens: Vec<Atalho>,
    /// Bit i = atalho i escolhido (ate 64 atalhos).
    escolhidos: u64,
    sob_ponteiro: Option<usize>,
    duplo: DuploClique,
}

impl Atalhos {
    pub fn novos() -> Atalhos {
        Atalhos { itens: Vec::new(), escolhidos: 0, sob_ponteiro: None, duplo: DuploClique::novo() }
    }

    /// Acrescenta um atalho no fim da coluna. Devolve a celula dele.
    pub fn adicionar(&mut self, nome: &'static str, icone: Icone, criar: Criador) -> Retangulo {
        let desenho = Superficie::nova(LARGURA_ATALHO as usize, ALTURA_ATALHO as usize, 0);
        self.itens.push(Atalho { nome, icone, criar, desenho, sujo: true });
        self.retangulo(self.itens.len() - 1)
    }

    /// Celula do atalho `i` na tela.
    pub fn retangulo(&self, i: usize) -> Retangulo {
        let y = MARGEM_ATALHOS + i as i32 * (ALTURA_ATALHO + ESPACO_ATALHOS);
        Retangulo::novo(MARGEM_ATALHOS, y, LARGURA_ATALHO, ALTURA_ATALHO)
    }

    pub fn indice_em(&self, x: i32, y: i32) -> Option<usize> {
        (0..self.itens.len()).find(|&i| self.retangulo(i).contem(x, y))
    }

    /// Marca para redesenhar as celulas `a` e `b` e devolve onde elas estao.
    fn mudaram(&mut self, a: Option<usize>, b: Option<usize>) -> [Option<Retangulo>; 2] {
        let mut celulas = [None, None];
        for (lugar, i) in celulas.iter_mut().zip([a, b]) {
            if let Some(i) = i.filter(|&i| i < self.itens.len()) {
                self.itens[i].sujo = true;
                *lugar = Some(self.retangulo(i));
            }
        }
        celulas
    }

    /// So o atalho `i` fica escolhido (None = nenhum). Devolve as celulas
    /// que mudaram (para sujar).
    pub fn escolher(&mut self, i: Option<usize>) -> Vec<Retangulo> {
        self.trocar_escolhidos(i.filter(|&i| i < 64).map_or(0, |i| 1 << i))
    }

    /// Escolhe os atalhos cuja celula toca `r` (selecao em area).
    pub fn escolher_em(&mut self, r: Retangulo) -> Vec<Retangulo> {
        let mascara = (0..self.itens.len().min(64))
            .filter(|&i| !self.retangulo(i).intersecao(&r).vazio())
            .fold(0u64, |m, i| m | 1 << i);
        self.trocar_escolhidos(mascara)
    }

    fn trocar_escolhidos(&mut self, mascara: u64) -> Vec<Retangulo> {
        let mudou = core::mem::replace(&mut self.escolhidos, mascara) ^ mascara;
        let mut celulas = Vec::new();
        for i in (0..self.itens.len().min(64)).filter(|i| mudou >> i & 1 == 1) {
            self.itens[i].sujo = true;
            celulas.push(self.retangulo(i));
        }
        celulas
    }

    fn escolhido(&self, i: usize) -> bool {
        i < 64 && self.escolhidos >> i & 1 == 1
    }

    /// O ponteiro esta em cima de `i` (None = de nenhum).
    pub fn passar(&mut self, i: Option<usize>) -> [Option<Retangulo>; 2] {
        if i == self.sob_ponteiro {
            return [None, None];
        }
        let antes = core::mem::replace(&mut self.sob_ponteiro, i);
        self.mudaram(antes, i)
    }

    /// Clique no atalho `i` (ja escolhido): no segundo clique rapido devolve
    /// o app para abrir.
    pub fn clicar(&mut self, i: usize) -> Option<Criador> {
        let duplo = self.duplo.apertar(i);
        self.itens.get(i).filter(|_| duplo).map(|a| a.criar)
    }

    /// Os apps dos atalhos escolhidos (Enter na area de trabalho abre todos).
    pub fn criadores_escolhidos(&self) -> Vec<Criador> {
        (0..self.itens.len()).filter(|&i| self.escolhido(i)).map(|i| self.itens[i].criar).collect()
    }

    /// Tudo precisa ser redesenhado (trocou o tema ou o papel de parede).
    pub fn sujar_todos(&mut self) {
        for atalho in &mut self.itens {
            atalho.sujo = true;
        }
    }

    /// Refaz o desenho das celulas que mudaram.
    pub fn preparar(&mut self, altura_area: i32, t: &Tema, ap: &Aparencia) {
        for i in 0..self.itens.len() {
            if !self.itens[i].sujo {
                continue;
            }
            let r = self.retangulo(i);
            let (escolhido, sob) = (self.escolhido(i), self.sob_ponteiro == Some(i));
            let atalho = &mut self.itens[i];
            desenhar_atalho(atalho, r, escolhido, sob, altura_area, t, ap);
            atalho.sujo = false;
        }
    }

    /// Copia para a cena o pedaco das celulas que cai em `area`.
    pub fn pintar(&self, cena: &mut Superficie, area: Retangulo) {
        for (i, atalho) in self.itens.iter().enumerate() {
            let r = self.retangulo(i);
            copiar_recortado(&atalho.desenho, r.x, r.y, area, cena);
        }
    }
}

/// Celula de um atalho: papel de parede, destaque, icone grande e o nome
/// com sombra (legivel em qualquer papel de parede).
fn desenhar_atalho(a: &mut Atalho, r: Retangulo, escolhido: bool, sob: bool, altura_area: i32, t: &Tema, ap: &Aparencia) {
    let s = &mut a.desenho;
    let area = s.area();
    pintar_papel(s, area, (r.x, r.y), altura_area, ap);
    if escolhido {
        tingir(s, area, t.destaque, 130);
        s.contorno(area, clarear(t.destaque, 90), 1);
    } else if sob {
        tingir(s, area, 0x00FF_FFFF, 40);
    }
    let lado = 16 * ESCALA_ATALHO;
    widgets::icone(s, (LARGURA_ATALHO - lado) / 2, TOPO_ICONE_ATALHO, a.icone, ESCALA_ATALHO);
    let nome = fonte::encurtar(a.nome, LARGURA_ATALHO - 8, &INTER);
    let x = (LARGURA_ATALHO - INTER.largura(&nome)) / 2;
    let base = TOPO_ICONE_ATALHO + lado + 5 + INTER.ascendente;
    s.texto_com(x + 1, base + 1, &nome, SOMBRA_NOME_ATALHO, &INTER);
    s.texto_com(x, base, &nome, COR_NOME_ATALHO, &INTER);
}
