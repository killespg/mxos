//! Navegador: o motor de src/navegador (HTML, diagramacao e desenho) numa
//! janela, com botoes, barra de endereco, historico e rolagem.
//!
//! A pagina e diagramada uma vez so, quando chega, e fica guardada. Cada
//! desenho so pinta o que mudou: rolar move os pixels que ja estavam na
//! janela e desenha so a faixa que entrou (mais a barra de rolagem), entao
//! rolar uma pagina comprida custa o mesmo que uma curta; passar o mouse
//! num link so redesenha a barra de status.
//!
//! Carregar trava a area de trabalho (disco e rede sao sincronos): primeiro
//! sai um quadro com "Carregando…", e a busca so acontece no Tique
//! seguinte, depois que esse quadro foi para a tela.
//!
//! Mouse: clique num link abre; botao do meio abre numa janela nova.
//! Teclado (com o endereco sem foco): setas, PgUp/PgDn, espaco, Home e End
//! rolam; Backspace volta; F5 recarrega; F6 vai para o endereco.
//!
//! Endereco: quando ganha o foco (clique ou F6) fica todo selecionado, como
//! nos outros navegadores: digitar troca tudo, Backspace apaga tudo e as
//! setas (ou outro clique) passam a editar.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::config::Config;
use crate::entrada::Botao;
use crate::entrada::teclado::{
    KEY_DELETE, KEY_DOWN, KEY_END, KEY_HOME, KEY_LEFT, KEY_PAGEDOWN, KEY_PAGEUP, KEY_RIGHT, KEY_SPACE, KEY_UP,
};
use crate::gui::area;
use crate::gui::grafico::{self, Cor, Retangulo, Superficie};
use crate::gui::janelas::{App, EventoApp, Pedido};
use crate::gui::tema::{self, Tema};
use crate::gui::widgets::{self, ALTURA_BOTAO, AcaoCampo, CampoTexto, Icone};
use crate::fonte;
use crate::navegador::{self as motor, layout, Pagina};

pub const TITULO: &str = "Navegador";
/// Sem pagina_inicial nas configuracoes.
const PAGINA_INICIAL: &str = "disco:/paginas/index.html";

const LARGURA: i32 = 780;
const ALTURA: i32 = 560;
const ALTURA_FERRAMENTAS: i32 = 34;
const ALTURA_STATUS: i32 = 22;
const LARGURA_ROLAGEM: i32 = 12;
const MARGEM: i32 = 6;
/// Botoes com icone (Voltar, Avancar, Recarregar, Inicio) e o "Ir".
const LADO_BOTAO: i32 = 30;
const ESPACO: i32 = 4;
const LARGURA_IR: i32 = 40;
/// Onde a pagina aparece e a barra de rolagem ao lado dela.
const AREA_PAGINA: Retangulo = Retangulo::novo(
    0,
    ALTURA_FERRAMENTAS,
    LARGURA - LARGURA_ROLAGEM,
    ALTURA - ALTURA_FERRAMENTAS - ALTURA_STATUS,
);
const TRILHO: Retangulo = Retangulo::novo(LARGURA - LARGURA_ROLAGEM, ALTURA_FERRAMENTAS, LARGURA_ROLAGEM, AREA_PAGINA.altura);

/// Pixels por passo da roda e por seta; PgUp/PgDn deixam uma seta repetida.
const PASSO_RODA: i32 = 48;
const PASSO_SETA: i32 = 24;
const MINIMO_POLEGAR: i32 = 24;
const MAX_HISTORICO: usize = 50;
/// Tiques esperando o quadro "Carregando" aparecer antes de buscar assim
/// mesmo (janela minimizada nao desenha).
const TIQUES_ESPERA: u8 = 3;
/// Caracteres do titulo da pagina no titulo da janela.
const MAX_TITULO: i32 = 60;
const COR_AVISO: Cor = 0x0070_7070;
/// Da borda do campo ate o texto (a mesma margem do widgets::CampoTexto).
const MARGEM_CAMPO: i32 = 6;

// Codigos evdev que nao estao no teclado.rs.
const TECLA_F5: u16 = 63;
const TECLA_F6: u16 = 64;

// Partes da janela que precisam ser redesenhadas.
const SUJO_FERRAMENTAS: u8 = 1 << 0;
const SUJO_PAGINA: u8 = 1 << 1;
const SUJO_ROLAGEM: u8 = 1 << 2;
const SUJO_STATUS: u8 = 1 << 3;
const SUJO_TUDO: u8 = SUJO_FERRAMENTAS | SUJO_PAGINA | SUJO_ROLAGEM | SUJO_STATUS;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Comando {
    Voltar,
    Avancar,
    Recarregar,
    Inicio,
    Ir,
}

const COMANDOS: [Comando; 5] = [Comando::Voltar, Comando::Avancar, Comando::Recarregar, Comando::Inicio, Comando::Ir];

impl Comando {
    fn retangulo(self) -> Retangulo {
        let y = (ALTURA_FERRAMENTAS - ALTURA_BOTAO) / 2;
        let indice = match self {
            Comando::Voltar => 0,
            Comando::Avancar => 1,
            Comando::Recarregar => 2,
            Comando::Inicio => 3,
            Comando::Ir => return Retangulo::novo(LARGURA - MARGEM - LARGURA_IR, y, LARGURA_IR, ALTURA_BOTAO),
        };
        Retangulo::novo(MARGEM + indice * (LADO_BOTAO + ESPACO), y, LADO_BOTAO, ALTURA_BOTAO)
    }

    fn icone(self) -> Option<Icone> {
        match self {
            Comando::Voltar => Some(Icone::Voltar),
            Comando::Avancar => Some(Icone::Avancar),
            Comando::Recarregar => Some(Icone::Recarregar),
            Comando::Inicio => Some(Icone::Casa),
            Comando::Ir => None,
        }
    }

    fn texto(self) -> &'static str {
        if self == Comando::Ir { "Ir" } else { "" }
    }
}

/// Barra de endereco: entre o botao Inicio e o Ir.
fn retangulo_endereco() -> Retangulo {
    let inicio = Comando::Inicio.retangulo().direita() + 2 * ESPACO;
    let fim = Comando::Ir.retangulo().x - ESPACO - 2;
    Retangulo::novo(inicio, (ALTURA_FERRAMENTAS - ALTURA_BOTAO) / 2, fim - inicio, ALTURA_BOTAO)
}

/// Para onde vai uma pagina pedida quando chegar.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Destino {
    /// Pagina nova: entra no historico depois da atual.
    Nova,
    /// Uma pagina do historico (voltar e avancar).
    Historico(usize),
    /// A mesma pagina de novo.
    Recarregar,
}

/// Pagina pedida que ainda vai ser buscada.
struct Carga {
    url: String,
    destino: Destino,
    /// Rolagem quando chegar (voltar para onde estava).
    rolagem: i32,
    /// O quadro "Carregando" ja foi desenhado.
    mostrada: bool,
    tiques: u8,
}

struct Visita {
    url: String,
    rolagem: i32,
}

pub struct Navegador {
    /// Endereco da pagina mostrada (o final, depois dos redirecionamentos).
    url: String,
    pagina: Option<Pagina>,
    titulo: String,
    historico: Vec<Visita>,
    /// Indice da pagina mostrada no historico.
    posicao: usize,
    carga: Option<Carga>,
    /// Quantos px da pagina ficaram para cima.
    rolagem: i32,
    endereco: CampoTexto,
    /// O endereco inteiro esta selecionado (a primeira tecla troca tudo).
    selecao: bool,
    status: String,
    /// Link (ja resolvido) embaixo do mouse: aparece no lugar do status.
    link_em_cima: Option<String>,
    /// Ultima posicao do mouse no conteudo.
    mouse: (i32, i32),
    /// Botao da barra apertado (soltar em cima dele executa).
    apertado: Option<Comando>,
    /// Link apertado na pagina (soltar em cima dele abre).
    link_apertado: Option<String>,
    /// Arrastando o polegar da rolagem: distancia do clique ao topo dele.
    arrasto: Option<i32>,
    /// Janelas novas para o gerenciador abrir (botao do meio num link).
    pedidos: Vec<Pedido>,
    /// Partes a redesenhar (SUJO_*).
    sujo: u8,
    /// Tema e rolagem do ultimo desenho, para saber o que mudou.
    tema_desenhado: Option<Tema>,
    rolagem_desenhada: i32,
}

/// pagina_inicial das configuracoes (ou a do disco), ja normalizada.
fn pagina_inicial() -> String {
    let cfg = Config::carregar();
    let url = cfg.obter("pagina_inicial").map(str::trim).filter(|u| !u.is_empty()).unwrap_or(PAGINA_INICIAL);
    motor::normalizar_url(url)
}

/// Para o menu iniciar: abre a pagina inicial.
pub fn criar() -> Box<dyn App> {
    Box::new(Navegador::novo(&pagina_inicial()))
}

/// "Titulo da pagina — Navegador"; sem titulo, o fim do endereco.
fn titulo_da_janela(titulo: &str, url: &str) -> String {
    let nome = if titulo.is_empty() {
        let sem_barra = url.trim_end_matches('/');
        match sem_barra.rfind('/') {
            Some(i) if i + 1 < sem_barra.len() => &sem_barra[i + 1..],
            _ => url,
        }
    } else {
        titulo
    };
    format!("{} — {TITULO}", cortar_caracteres(nome, MAX_TITULO))
}

/// Os primeiros `maximo` caracteres de `texto` (com "…" se cortou).
fn cortar_caracteres(texto: &str, maximo: i32) -> String {
    match texto.char_indices().nth(maximo.max(1) as usize) {
        Some((fim, _)) => format!("{}…", texto[..fim].trim_end()),
        None => String::from(texto),
    }
}

/// Move os pixels de `area` `delta` linhas para cima (delta > 0: a pagina
/// rolou para baixo) ou para baixo e devolve a faixa que ficou para
/// desenhar. `area` tem de estar dentro da superficie e |delta| < altura.
fn deslocar(s: &mut Superficie, area: Retangulo, delta: i32) -> Retangulo {
    let passo = s.largura();
    let (x0, x1) = (area.x as usize, area.direita() as usize);
    let d = delta.unsigned_abs() as usize;
    let topo = area.y as usize;
    let ficam = area.altura as usize - d;
    let pixels = s.pixels_mut();
    if delta > 0 {
        // de cima para baixo: cada linha lida ainda nao foi escrita
        for i in 0..ficam {
            let para = (topo + i) * passo;
            let de = para + d * passo;
            pixels.copy_within(de + x0..de + x1, para + x0);
        }
        Retangulo::novo(area.x, area.baixo() - delta, area.largura, delta)
    } else {
        for i in (0..ficam).rev() {
            let para = (topo + d + i) * passo;
            let de = para - d * passo;
            pixels.copy_within(de + x0..de + x1, para + x0);
        }
        Retangulo::novo(area.x, area.y, area.largura, d as i32)
    }
}

/// Mistura cada pixel de `r` com `cor` (peso 0 a 255): desbota o icone de
/// um botao desabilitado, tinge a selecao do endereco.
fn tingir(s: &mut Superficie, r: Retangulo, cor: Cor, peso: u32) {
    let r = r.intersecao(&s.area());
    let passo = s.largura();
    let pixels = s.pixels_mut();
    for y in r.y..r.baixo() {
        let linha = y as usize * passo;
        for p in &mut pixels[linha + r.x as usize..linha + r.direita() as usize] {
            *p = grafico::misturar(*p, cor, peso);
        }
    }
}

impl Navegador {
    /// Janela que ja comeca carregando `url` (normalizada como na barra
    /// de endereco: "/x.html" = disco, "site.com" = http).
    pub fn novo(url: &str) -> Navegador {
        let url = if url.trim().is_empty() { String::from(PAGINA_INICIAL) } else { motor::normalizar_url(url) };
        let mut navegador = Navegador {
            url: url.clone(),
            pagina: None,
            titulo: String::from(TITULO),
            historico: Vec::new(),
            posicao: 0,
            carga: None,
            rolagem: 0,
            endereco: CampoTexto::novo(&url),
            selecao: false,
            status: String::new(),
            link_em_cima: None,
            mouse: (-1, -1),
            apertado: None,
            link_apertado: None,
            arrasto: None,
            pedidos: Vec::new(),
            sujo: SUJO_TUDO,
            tema_desenhado: None,
            rolagem_desenhada: 0,
        };
        navegador.pedir(url, Destino::Nova, 0);
        navegador
    }

    // ---------------- carregar ----------------

    /// Pede uma pagina. O quadro "Carregando" sai antes; a busca fica
    /// para o proximo Tique.
    fn pedir(&mut self, url: String, destino: Destino, rolagem: i32) {
        if let Some(atual) = self.historico.get_mut(self.posicao) {
            atual.rolagem = self.rolagem; // para voltar no mesmo lugar
        }
        if !self.endereco.focado() {
            self.endereco.definir(&url);
        }
        self.status = format!("Carregando {url}…");
        self.link_em_cima = None;
        self.link_apertado = None;
        self.carga = Some(Carga { url, destino, rolagem, mostrada: false, tiques: 0 });
        self.sujo |= SUJO_FERRAMENTAS | SUJO_STATUS;
        if self.pagina.is_none() {
            self.sujo |= SUJO_PAGINA;
        }
    }

    /// Busca a pagina pedida (trava ate o disco ou a rede responder),
    /// diagrama e mostra.
    fn carregar(&mut self) {
        let Some(carga) = self.carga.take() else { return };
        let aberta = motor::abrir(&carga.url);
        let pagina = motor::diagramar(&motor::documento(&aberta.html, &aberta.url_final), AREA_PAGINA.largura);
        self.guardar_no_historico(carga.destino, &aberta.url_final);
        self.titulo = titulo_da_janela(&pagina.titulo, &aberta.url_final);
        self.status = match aberta.erro {
            Some(motivo) => format!("Erro: {motivo}"),
            None => String::from("Pronto"),
        };
        self.url = aberta.url_final;
        if !self.endereco.focado() {
            self.endereco.definir(&self.url);
        }
        let maximo = (pagina.altura - AREA_PAGINA.altura).max(0);
        self.rolagem = carga.rolagem.clamp(0, maximo);
        self.pagina = Some(pagina);
        self.arrasto = None;
        self.atualizar_link();
        self.sujo = SUJO_TUDO;
    }

    fn guardar_no_historico(&mut self, destino: Destino, url: &str) {
        match destino {
            Destino::Historico(i) if i < self.historico.len() => {
                self.posicao = i;
                self.historico[i].url = String::from(url);
            }
            Destino::Recarregar if self.posicao < self.historico.len() => {
                self.historico[self.posicao].url = String::from(url);
            }
            // a pagina que ja esta aberta: nao repete no historico
            _ if self.historico.get(self.posicao).is_some_and(|v| v.url == url) => {}
            _ => {
                self.historico.truncate(self.posicao + 1);
                self.historico.push(Visita { url: String::from(url), rolagem: 0 });
                if self.historico.len() > MAX_HISTORICO {
                    self.historico.remove(0);
                }
                self.posicao = self.historico.len() - 1;
            }
        }
    }

    fn ir_no_historico(&mut self, i: usize) {
        if i == self.posicao {
            return;
        }
        let Some(visita) = self.historico.get(i) else { return };
        let (url, rolagem) = (visita.url.clone(), visita.rolagem);
        self.pedir(url, Destino::Historico(i), rolagem);
    }

    /// Enter ou "Ir": abre o que esta escrito no endereco.
    fn ir_para_endereco(&mut self) {
        let texto = self.endereco.texto().trim();
        if texto.is_empty() {
            return;
        }
        let url = motor::normalizar_url(texto);
        self.endereco.focar(false);
        self.sujo |= SUJO_FERRAMENTAS;
        self.pedir(url, Destino::Nova, 0);
    }

    fn habilitado(&self, c: Comando) -> bool {
        match c {
            Comando::Voltar => self.posicao > 0 && !self.historico.is_empty(),
            Comando::Avancar => self.posicao + 1 < self.historico.len(),
            _ => true,
        }
    }

    fn executar(&mut self, c: Comando) {
        if !self.habilitado(c) {
            return;
        }
        match c {
            Comando::Voltar => self.ir_no_historico(self.posicao - 1),
            Comando::Avancar => self.ir_no_historico(self.posicao + 1),
            Comando::Recarregar => self.pedir(self.url.clone(), Destino::Recarregar, self.rolagem),
            Comando::Inicio => self.pedir(pagina_inicial(), Destino::Nova, 0),
            Comando::Ir => self.ir_para_endereco(),
        }
    }

    // ---------------- rolagem e links ----------------

    fn maximo_rolagem(&self) -> i32 {
        self.pagina.as_ref().map_or(0, |p| (p.altura - AREA_PAGINA.altura).max(0))
    }

    /// Rola para `y` (preso nos limites).
    fn rolar_para(&mut self, y: i32) {
        let y = y.clamp(0, self.maximo_rolagem());
        if y != self.rolagem {
            self.rolagem = y;
            self.sujo |= SUJO_ROLAGEM;
            self.atualizar_link(); // a pagina andou embaixo do mouse
        }
    }

    fn rolar(&mut self, dy: i32) {
        self.rolar_para(self.rolagem.saturating_add(dy));
    }

    /// Polegar da barra de rolagem, na largura toda do trilho (None = a
    /// pagina cabe inteira).
    fn polegar(&self) -> Option<Retangulo> {
        let pagina = self.pagina.as_ref()?;
        let maximo = self.maximo_rolagem();
        if maximo == 0 {
            return None;
        }
        let trilho = TRILHO.altura as i64;
        let altura = (trilho * AREA_PAGINA.altura as i64 / pagina.altura.max(1) as i64) as i32;
        let altura = altura.clamp(MINIMO_POLEGAR, TRILHO.altura);
        let y = TRILHO.y + ((TRILHO.altura - altura) as i64 * self.rolagem as i64 / maximo as i64) as i32;
        Some(Retangulo::novo(TRILHO.x, y, TRILHO.largura, altura))
    }

    /// Clique no trilho: no polegar comeca a arrastar; fora, pula uma tela.
    fn apertar_trilho(&mut self, y: i32) {
        let Some(p) = self.polegar() else { return };
        let tela = AREA_PAGINA.altura - PASSO_SETA;
        if y < p.y {
            self.rolar(-tela);
        } else if y >= p.baixo() {
            self.rolar(tela);
        } else {
            self.arrasto = Some(y - p.y);
            self.sujo |= SUJO_ROLAGEM;
        }
    }

    /// O polegar foi arrastado ate o mouse estar em `y`.
    fn arrastar_polegar(&mut self, y: i32, deslocamento: i32) {
        let Some(p) = self.polegar() else { return };
        let curso = (TRILHO.altura - p.altura).max(1) as i64;
        let topo = (y - deslocamento - TRILHO.y) as i64;
        self.rolar_para((topo * self.maximo_rolagem() as i64 / curso) as i32);
    }

    /// Link (resolvido) no ponto (x, y) do conteudo.
    fn link_no_ponto(&self, x: i32, y: i32) -> Option<String> {
        if !AREA_PAGINA.contem(x, y) {
            return None;
        }
        let pagina = self.pagina.as_ref()?;
        let href = motor::link_em(pagina, x - AREA_PAGINA.x, y - AREA_PAGINA.y + self.rolagem)?;
        Some(motor::resolver_url(&self.url, &href))
    }

    /// Confere o link embaixo do mouse (o status mostra o endereco dele).
    fn atualizar_link(&mut self) {
        let link = if self.carga.is_some() { None } else { self.link_no_ponto(self.mouse.0, self.mouse.1) };
        if link != self.link_em_cima {
            self.link_em_cima = link;
            self.sujo |= SUJO_STATUS;
        }
    }

    // ---------------- eventos ----------------

    /// O campo do endereco ve todo clique (clicar fora tira o foco) e, com
    /// foco, o teclado. Ganhar o foco seleciona o endereco inteiro.
    fn evento_endereco(&mut self, e: EventoApp) {
        let antes = self.endereco.focado();
        if antes && self.selecao && self.trocar_selecao(e) {
            return;
        }
        let acao = self.endereco.evento(e, retangulo_endereco());
        match acao {
            AcaoCampo::Confirmou => self.ir_para_endereco(),
            AcaoCampo::Cancelou => {
                self.endereco.definir(&self.url);
                self.endereco.focar(false);
            }
            AcaoCampo::Mudou | AcaoCampo::Nada => {}
        }
        let agora = self.endereco.focado();
        if agora != antes {
            self.selecao = agora;
        }
        if acao != AcaoCampo::Nada || agora != antes {
            self.sujo |= SUJO_FERRAMENTAS;
        }
    }

    /// Com o endereco todo selecionado: Backspace e Delete apagam tudo (e
    /// param aqui: true), um caractere troca tudo, setas e cliques so tiram
    /// a selecao. Shift e as outras teclas nao mexem nela.
    fn trocar_selecao(&mut self, e: EventoApp) -> bool {
        let apagar = match e {
            EventoApp::Caractere('\u{8}') | EventoApp::Tecla { codigo: KEY_DELETE, pressionada: true } => true,
            EventoApp::Caractere(c) if !c.is_control() => {
                self.endereco.definir(""); // o caractere vai para o campo vazio
                false
            }
            EventoApp::Tecla { codigo: KEY_LEFT | KEY_RIGHT | KEY_HOME | KEY_END, pressionada: true }
            | EventoApp::Botao { botao: Botao::Esquerdo, pressionado: true, .. } => false,
            _ => return false,
        };
        self.selecao = false;
        self.sujo |= SUJO_FERRAMENTAS;
        if apagar {
            self.endereco.definir("");
        }
        apagar
    }

    fn botao(&mut self, x: i32, y: i32, botao: Botao, pressionado: bool) {
        self.evento_endereco(EventoApp::Botao { x, y, botao, pressionado });
        self.mouse = (x, y);
        match (botao, pressionado) {
            (Botao::Esquerdo, true) => self.apertar(x, y),
            (Botao::Esquerdo, false) => self.soltar(x, y),
            (Botao::Meio, true) => {
                if let Some(link) = self.link_no_ponto(x, y) {
                    self.pedidos.push(Pedido::Abrir { app: Box::new(Navegador::novo(&link)), x: -1, y: -1 });
                }
            }
            _ => {}
        }
    }

    fn apertar(&mut self, x: i32, y: i32) {
        if let Some(c) = COMANDOS.iter().copied().find(|c| c.retangulo().contem(x, y)) {
            if self.habilitado(c) {
                self.apertado = Some(c);
                self.sujo |= SUJO_FERRAMENTAS;
            }
        } else if TRILHO.contem(x, y) {
            self.apertar_trilho(y);
        } else {
            self.link_apertado = self.link_no_ponto(x, y);
        }
    }

    fn soltar(&mut self, x: i32, y: i32) {
        if let Some(c) = self.apertado.take() {
            self.sujo |= SUJO_FERRAMENTAS;
            if c.retangulo().contem(x, y) {
                self.executar(c);
            }
        }
        if self.arrasto.take().is_some() {
            self.sujo |= SUJO_ROLAGEM;
        }
        if let Some(link) = self.link_apertado.take()
            && self.link_no_ponto(x, y).as_deref() == Some(link.as_str())
        {
            self.pedir(link, Destino::Nova, 0);
        }
    }

    fn mover(&mut self, x: i32, y: i32, botoes: u8) {
        self.mouse = (x, y);
        match self.arrasto {
            // soltou sem a janela ver (nao devia acontecer: o mouse fica capturado)
            Some(_) if botoes & 1 == 0 => {
                self.arrasto = None;
                self.sujo |= SUJO_ROLAGEM;
            }
            Some(deslocamento) => self.arrastar_polegar(y, deslocamento),
            None => self.atualizar_link(),
        }
    }

    /// Teclas de rolagem e atalhos (com o endereco sem foco).
    fn tecla(&mut self, codigo: u16) {
        let tela = AREA_PAGINA.altura - PASSO_SETA;
        match codigo {
            KEY_UP => self.rolar(-PASSO_SETA),
            KEY_DOWN => self.rolar(PASSO_SETA),
            KEY_PAGEUP => self.rolar(-tela),
            KEY_PAGEDOWN | KEY_SPACE => self.rolar(tela),
            KEY_HOME => self.rolar_para(0),
            KEY_END => self.rolar_para(i32::MAX),
            TECLA_F5 => self.executar(Comando::Recarregar),
            TECLA_F6 => {
                self.endereco.focar(true);
                self.selecao = true;
                self.sujo |= SUJO_FERRAMENTAS;
            }
            _ => {}
        }
    }

    fn tique(&mut self) {
        if self.endereco.focado() {
            self.evento_endereco(EventoApp::Tique); // cursor piscando
        }
        let Some(carga) = &mut self.carga else { return };
        carga.tiques = carga.tiques.saturating_add(1);
        if carga.mostrada || carga.tiques >= TIQUES_ESPERA {
            self.carregar();
        }
    }

    // ---------------- desenho ----------------

    fn desenhar_ferramentas(&self, s: &mut Superficie, t: &Tema) {
        s.retangulo(Retangulo::novo(0, 0, LARGURA, ALTURA_FERRAMENTAS), t.fundo_janela);
        // a linha de baixo fica na cor de destaque enquanto carrega
        let (cor, espessura) = if self.carga.is_some() { (t.destaque, 2) } else { (t.borda, 1) };
        s.retangulo(Retangulo::novo(0, ALTURA_FERRAMENTAS - espessura, LARGURA, espessura), cor);
        for c in COMANDOS {
            let r = c.retangulo();
            // "Ir" e a acao principal; os outros acendem enquanto apertados
            widgets::botao(s, r, c.texto(), c == Comando::Ir || self.apertado == Some(c));
            if let Some(icone) = c.icone() {
                let (x, y) = (r.x + (r.largura - 16) / 2, r.y + (r.altura - 16) / 2);
                widgets::icone(s, x, y, icone, 1);
                if !self.habilitado(c) {
                    tingir(s, Retangulo::novo(x, y, 16, 16), t.botao, 170);
                }
            }
        }
        let r = retangulo_endereco();
        self.endereco.desenhar(s, r);
        if self.selecao && self.endereco.focado() {
            let largura = Superficie::largura_texto(self.endereco.texto(), 1).min(r.largura - 2 * MARGEM_CAMPO);
            tingir(s, Retangulo::novo(r.x + MARGEM_CAMPO - 1, r.y + 4, largura + 2, r.altura - 8), t.destaque, 90);
        }
    }

    fn desenhar_pagina(&self, s: &mut Superficie) {
        match &self.pagina {
            Some(pagina) => motor::desenhar(pagina, s, AREA_PAGINA, self.rolagem),
            None => {
                // ainda sem nenhuma pagina: so o aviso
                s.retangulo(AREA_PAGINA, motor::render::COR_FUNDO);
                if self.carga.is_some() {
                    let base = AREA_PAGINA.y + layout::MARGEM + fonte::INTER.ascendente;
                    s.texto_com(AREA_PAGINA.x + layout::MARGEM, base, "Carregando…", COR_AVISO, &fonte::INTER);
                }
            }
        }
    }

    /// Rolou: move os pixels da pagina e desenha so a faixa que entrou.
    fn rolar_pixels(&self, s: &mut Superficie) {
        let area = AREA_PAGINA.intersecao(&s.area());
        let delta = self.rolagem - self.rolagem_desenhada;
        let Some(pagina) = &self.pagina else { return };
        if area.y != AREA_PAGINA.y || delta.abs() >= area.altura {
            self.desenhar_pagina(s);
            return;
        }
        let faixa = deslocar(s, area, delta);
        motor::desenhar(pagina, s, faixa, self.rolagem + (faixa.y - AREA_PAGINA.y));
    }

    fn desenhar_rolagem(&self, s: &mut Superficie, t: &Tema) {
        s.retangulo(TRILHO, grafico::misturar(t.fundo_janela, t.texto, 14));
        s.retangulo(Retangulo::novo(TRILHO.x, TRILHO.y, 1, TRILHO.altura), grafico::misturar(t.fundo_janela, t.texto, 40));
        if let Some(p) = self.polegar() {
            let cor = if self.arrasto.is_some() { t.destaque } else { grafico::misturar(t.fundo_janela, t.texto, 100) };
            s.retangulo(Retangulo::novo(p.x + 3, p.y + 1, p.largura - 6, p.altura - 2), cor);
        }
    }

    fn desenhar_status(&self, s: &mut Superficie, t: &Tema) {
        let r = Retangulo::novo(0, ALTURA - ALTURA_STATUS, LARGURA, ALTURA_STATUS);
        s.retangulo(r, area::escurecer(t.fundo_janela, 10));
        s.retangulo(Retangulo::novo(0, r.y, LARGURA, 1), t.borda);
        let texto = self.link_em_cima.as_deref().unwrap_or(&self.status);
        let texto = fonte::cortar(texto, LARGURA - 2 * MARGEM - 4, &fonte::INTER);
        let base = fonte::INTER.base_centralizada(r.y + 1, ALTURA_STATUS - 1);
        s.texto_com(MARGEM + 2, base, texto, t.texto, &fonte::INTER);
    }
}

impl App for Navegador {
    fn titulo(&self) -> &str {
        &self.titulo
    }

    fn tamanho(&self) -> (usize, usize) {
        (LARGURA as usize, ALTURA as usize)
    }

    /// So as partes marcadas. Sem nada marcado o pedido veio de fora
    /// (janela nova, tema trocado): tudo de novo.
    fn desenhar(&mut self, s: &mut Superficie) {
        let t = tema::atual();
        if self.sujo == 0 || self.tema_desenhado != Some(t) {
            self.sujo = SUJO_TUDO;
        }
        if self.sujo & SUJO_FERRAMENTAS != 0 {
            self.desenhar_ferramentas(s, &t);
        }
        if self.sujo & SUJO_PAGINA != 0 {
            self.desenhar_pagina(s);
        } else if self.rolagem != self.rolagem_desenhada {
            self.rolar_pixels(s);
        }
        if self.sujo & SUJO_ROLAGEM != 0 {
            self.desenhar_rolagem(s, &t);
        }
        if self.sujo & SUJO_STATUS != 0 {
            self.desenhar_status(s, &t);
        }
        self.sujo = 0;
        self.tema_desenhado = Some(t);
        self.rolagem_desenhada = self.rolagem;
        if let Some(carga) = &mut self.carga {
            carga.mostrada = true;
        }
    }

    fn evento(&mut self, e: EventoApp) -> bool {
        let focado = self.endereco.focado();
        match e {
            EventoApp::Tique => self.tique(),
            EventoApp::Caractere(_) | EventoApp::Tecla { .. } if focado => self.evento_endereco(e),
            EventoApp::Tecla { codigo, pressionada: true } => self.tecla(codigo),
            EventoApp::Caractere('\u{8}') => self.executar(Comando::Voltar),
            EventoApp::Botao { x, y, botao, pressionado } => self.botao(x, y, botao, pressionado),
            EventoApp::Mouse { x, y, botoes } => self.mover(x, y, botoes),
            EventoApp::Roda(passos) => self.rolar(passos.saturating_mul(-PASSO_RODA)),
            _ => {}
        }
        self.sujo != 0
    }

    fn pedidos(&mut self) -> Vec<Pedido> {
        core::mem::take(&mut self.pedidos)
    }
}
