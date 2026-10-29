//! Configuracoes do MXOS, em abas:
//!   - Personalizar: tema (cartoes com a area de trabalho em miniatura),
//!     papel de parede (tipo e cor), relogio com segundos e icones
//!   - Teclado: layout US ou ABNT2 e um campo para testar
//!   - Rede: enderecos, contadores e um ping no gateway; pagina inicial
//!   - Disco: cada disco com a barra de uso
//!   - Sistema: versao, cpu, memoria, tempo ligado, reiniciar e desligar
//!
//! Toda mudanca vale na hora: tema e aparencia trocam no estado global do
//! tema.rs e o app pede Pedido::TemaMudou (o gerenciador redesenha tudo);
//! o layout vai num Pedido::DefinirLayout para o laco principal, que e o
//! dono do Entrada. O Salvar grava tudo em /config/mxos.cfg (config.rs),
//! que o gui::rodar le no boot.
//!
//! Nada que bloqueia roda no clique: o clique so muda o estado e pede um
//! quadro com o aviso ("Salvando...", "Testando..."); o trabalho fica para
//! o proximo Tique, com o aviso ja na tela.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU8, Ordering};

use crate::armazenamento::{self, InfoDisco};
use crate::config::{self, Config};
use crate::entrada::{Botao, Layout, teclado};
use crate::fonte::{self, Fonte, INTER, INTER_GRANDE, INTER_NEGRITO};
use crate::gui::area;
use crate::gui::grafico::{self, Cor, Retangulo, Superficie};
use crate::gui::janelas::{App, EventoApp, Pedido};
use crate::gui::tema::{self, Aparencia, PapelDeParede, TEMAS, Tema};
use crate::gui::widgets::{self, Abas, AcaoCampo, CampoTexto, Icone};
use crate::rede::{self, Ip, Mac};
use crate::{cpu, heap, plataforma, psci, timer};

const TITULO: &str = "Configurações";
/// Versao do MXOS: major.minor do Cargo.toml.
const VERSAO: &str = concat!(env!("CARGO_PKG_VERSION_MAJOR"), ".", env!("CARGO_PKG_VERSION_MINOR"));

const LARGURA: i32 = 640;
const ALTURA: i32 = 480;
const ALTURA_ABAS: i32 = widgets::ALTURA_ABAS;
const ALTURA_RODAPE: i32 = 44;
/// Onde a pagina da aba termina e o rodape (Salvar) comeca.
const BASE_PAGINA: i32 = ALTURA - ALTURA_RODAPE;
const MARGEM: i32 = 16;
const LARGURA_UTIL: i32 = LARGURA - 2 * MARGEM;
/// Primeira linha de conteudo de toda pagina.
const TOPO: i32 = ALTURA_ABAS + 14;
/// Do titulo de uma secao ate o conteudo dela.
const ABAIXO_TITULO: i32 = 16;
/// Coluna dos valores nas linhas "rotulo  valor" dos paineis.
const COLUNA_VALOR: i32 = 112;
const ALTURA_LINHA: i32 = widgets::ALTURA_LINHA;
const ALTURA_BOTAO: i32 = widgets::ALTURA_BOTAO;
const LARGURA_BOTAO: i32 = 96;
/// Espaco entre cartoes lado a lado.
const ESPACO_CARTAO: i32 = 12;
const PRAZO_PING_MS: u64 = 1000;
const BRANCO: Cor = 0x00FF_FFFF;

const ABAS: [&str; 5] = ["Personalizar", "Teclado", "Rede", "Disco", "Sistema"];
const ABA_PERSONALIZAR: usize = 0;
const ABA_TECLADO: usize = 1;
const ABA_REDE: usize = 2;
const ABA_DISCO: usize = 3;
const ABA_SISTEMA: usize = 4;

// Aba Personalizar, de cima para baixo: temas, papel, cor e chaves.
/// 112 x 84: o formato 4:3 da tela.
const ALTURA_PREVIA: i32 = 84;
const ALTURA_MINIATURA: i32 = 52;
/// Nome embaixo de cada cartao.
const ALTURA_LEGENDA: i32 = 18;
/// Barra de tarefas das miniaturas.
const ALTURA_BARRA_MINI: i32 = 8;
const LADO_COR: i32 = 24;
const ESPACO_COR: i32 = 8;
/// Folga entre o fim de uma secao e o titulo da proxima.
const ESPACO_SECAO: i32 = 18;
const Y_TEMAS: i32 = TOPO;
const Y_PAPEIS: i32 = Y_TEMAS + ABAIXO_TITULO + ALTURA_PREVIA + ALTURA_LEGENDA + ESPACO_SECAO;
const Y_CORES: i32 = Y_PAPEIS + ABAIXO_TITULO + ALTURA_MINIATURA + ALTURA_LEGENDA + ESPACO_SECAO;
const Y_CHAVES: i32 = Y_CORES + ABAIXO_TITULO + LADO_COR + ESPACO_SECAO + 4;

// Aba Teclado.
const Y_LAYOUTS: i32 = TOPO;
const ALTURA_CARTAO_LAYOUT: i32 = 60;
const Y_TESTE: i32 = Y_LAYOUTS + ABAIXO_TITULO + ALTURA_CARTAO_LAYOUT + 16;
const Y_ATALHOS: i32 = Y_TESTE + ABAIXO_TITULO + ALTURA_BOTAO + 32;

// Aba Rede.
const Y_PAINEL_REDE: i32 = TOPO + 44;
const LINHAS_REDE: i32 = 6;
const Y_TESTAR: i32 = Y_PAINEL_REDE + LINHAS_REDE * ALTURA_LINHA + 24;
const Y_NAVEGADOR: i32 = Y_TESTAR + ALTURA_BOTAO + 20;
/// Largura do rotulo "Pagina inicial" antes do campo.
const ROTULO_PAGINA: i32 = 120;

// Aba Disco.
const Y_DISCOS: i32 = TOPO + 32;
const ALTURA_DISCO: i32 = 76;
/// Cabem na pagina; os outros viram um "+ N".
const MAX_DISCOS: usize = 4;

// Aba Sistema.
const Y_PAINEL_SISTEMA: i32 = TOPO + 60;
const LINHAS_SISTEMA: i32 = 6;
const Y_SAIR: i32 = Y_PAINEL_SISTEMA + LINHAS_SISTEMA * ALTURA_LINHA + 36;
const LARGURA_SIM: i32 = 128;

/// Cores do papel de parede: tons escuros e medios, onde o texto claro dos
/// icones da area de trabalho continua legivel. A primeira e a de fabrica.
const CORES_PAPEL: [Cor; 12] = [
    0x0020_4060, // azul petroleo (padrao)
    0x000B_2545, // azul marinho
    0x000F_4C5C, // azul esverdeado
    0x001F_5C45, // verde
    0x0043_306B, // roxo
    0x005C_1F3A, // vinho
    0x007A_3B1E, // terracota
    0x006B_5B14, // oliva
    0x002E_3440, // grafite
    0x004C_566A, // cinza azulado
    0x0010_1418, // quase preto
    0x0058_6A80, // cinza
];

/// Layouts do teclado: o valor, o nome grande e a explicacao.
const LAYOUTS: [(Layout, &str, &str); 2] =
    [(Layout::Us, "US", "Teclado americano"), (Layout::Abnt2, "ABNT2", "Teclado brasileiro")];

/// Atalhos globais do gerenciador de janelas.
const ATALHOS: [(&str, &str); 4] = [
    ("Alt+F4", "fecha a janela ativa"),
    ("Alt+Tab", "traz a janela de baixo para a frente"),
    ("Windows", "abre e fecha o menu iniciar"),
    ("Esc", "fecha o menu iniciar"),
];

/// Layout escolhido nesta sessao (0 = nenhum ainda: vale o do arquivo).
/// O Entrada mora no laco principal; uma janela de configuracoes aberta
/// depois le daqui o que esta valendo. Atomico: nada de static mut.
static LAYOUT_ESCOLHIDO: AtomicU8 = AtomicU8::new(0);

fn lembrar_layout(layout: Layout) {
    let codigo = match layout {
        Layout::Us => 1,
        Layout::Abnt2 => 2,
    };
    LAYOUT_ESCOLHIDO.store(codigo, Ordering::Relaxed);
}

/// O layout que o laco principal esta usando.
fn layout_em_uso(cfg: &Config) -> Layout {
    match LAYOUT_ESCOLHIDO.load(Ordering::Relaxed) {
        1 => Layout::Us,
        2 => Layout::Abnt2,
        _ => config::layout(cfg),
    }
}

/// Menu iniciar "Configuracoes": abre no resumo do sistema.
pub fn criar() -> Box<dyn App> {
    Box::new(Configuracoes::nova(ABA_SISTEMA))
}

/// Menu iniciar "Personalizar": o mesmo app, na primeira aba.
pub fn criar_personalizar() -> Box<dyn App> {
    Box::new(Configuracoes::nova(ABA_PERSONALIZAR))
}

/// Como sair do MXOS pela aba Sistema.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Saida {
    Reiniciar,
    Desligar,
}

impl Saida {
    fn pergunta(self) -> &'static str {
        match self {
            Saida::Reiniciar => "Reiniciar o MXOS agora?",
            Saida::Desligar => "Desligar o MXOS agora?",
        }
    }

    fn confirmar(self) -> &'static str {
        match self {
            Saida::Reiniciar => "Sim, reiniciar",
            Saida::Desligar => "Sim, desligar",
        }
    }

    fn andamento(self) -> &'static str {
        match self {
            Saida::Reiniciar => "Reiniciando o MXOS...",
            Saida::Desligar => "Desligando o MXOS...",
        }
    }
}

/// Trabalho que bloqueia: fica para o proximo Tique, depois do aviso.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Trabalho {
    Salvar,
    TestarRede,
    Sair(Saida),
}

/// Cor de uma mensagem.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tom {
    Normal,
    Sucesso,
    Erro,
}

pub struct Configuracoes {
    abas: Abas,
    /// Copia do layout em uso (o Entrada mora no laco principal).
    layout: Layout,
    campo_teste: CampoTexto,
    /// Ultima tecla apertada na aba Teclado e o caractere que ela escreveu.
    ultima_tecla: Option<(u16, Option<char>)>,
    campo_pagina: CampoTexto,
    /// Discos da aba Disco: lidos ao abrir a aba e no Atualizar.
    discos: Vec<InfoDisco>,
    /// Resultado do ultimo Testar da aba Rede.
    teste_rede: Option<(String, Tom)>,
    /// Reiniciar ou desligar esperando o "Sim".
    confirmar: Option<Saida>,
    /// Trabalho que bloqueia, feito no proximo Tique.
    pendente: Option<Trabalho>,
    /// Mensagem do rodape (None = a dica de sempre).
    aviso: Option<(String, Tom)>,
    /// Algo mudou desde o ultimo Salvar.
    modificado: bool,
    /// Versao do PSCI, perguntada uma vez ao firmware.
    versao_psci: u64,
    /// Segundo mostrado: Rede e Sistema se atualizam uma vez por segundo.
    segundo: u64,
    pedidos: Vec<Pedido>,
}

impl Configuracoes {
    /// Abre na aba `aba` (indice em ABAS).
    pub fn nova(aba: usize) -> Configuracoes {
        let cfg = Config::carregar();
        let mut abas = Abas::novas(&ABAS);
        abas.definir(aba);
        let mut app = Configuracoes {
            abas,
            layout: layout_em_uso(&cfg),
            campo_teste: CampoTexto::novo(""),
            ultima_tecla: None,
            campo_pagina: CampoTexto::novo(cfg.obter("pagina_inicial").unwrap_or("")),
            discos: Vec::new(),
            teste_rede: None,
            confirmar: None,
            pendente: None,
            aviso: None,
            modificado: false,
            versao_psci: psci::chamar(psci::via(), psci::VERSAO),
            segundo: segundos_ligado(),
            pedidos: Vec::new(),
        };
        app.trocou_de_aba();
        app
    }

    // ------------------------------------------------------------ mudancas

    /// Algo mudou e ainda nao foi salvo.
    fn mudou(&mut self) {
        self.modificado = true;
        self.aviso = None;
    }

    /// Tema ou aparencia trocados: o gerenciador redesenha tudo.
    fn aparencia_mudou(&mut self) {
        self.pedidos.push(Pedido::TemaMudou);
        self.mudou();
    }

    fn escolher_tema(&mut self, i: usize) -> bool {
        let novo = TEMAS[i].1;
        if tema::atual() == novo {
            return false;
        }
        tema::definir(novo);
        self.aparencia_mudou();
        true
    }

    /// Muda a aparencia com `mudar` e aplica na hora (se mudou mesmo).
    fn mudar_aparencia(&mut self, mudar: impl FnOnce(&mut Aparencia)) -> bool {
        let antes = tema::aparencia();
        let mut nova = antes;
        mudar(&mut nova);
        if nova == antes {
            return false;
        }
        tema::definir_aparencia(nova);
        self.aparencia_mudou();
        true
    }

    fn escolher_layout(&mut self, layout: Layout) -> bool {
        if layout == self.layout {
            return false;
        }
        self.layout = layout;
        lembrar_layout(layout);
        self.pedidos.push(Pedido::DefinirLayout(layout));
        self.mudou();
        true
    }

    /// Volta tudo para os valores de fabrica, na hora (o Salvar grava).
    fn restaurar_padrao(&mut self) -> bool {
        let padrao = Config::padrao();
        config::aplicar_aparencia(&padrao);
        self.pedidos.push(Pedido::TemaMudou);
        self.escolher_layout(config::layout(&padrao));
        if let Some(pagina) = padrao.obter("pagina_inicial") {
            self.campo_pagina.definir(pagina);
        }
        self.modificado = true;
        self.aviso = Some((String::from("Padrão de fábrica aplicado. Salve para manter."), Tom::Normal));
        true
    }

    // ------------------------------------------------------------ trabalhos

    /// Deixa `trabalho` para o proximo Tique (um de cada vez). true = o
    /// aviso dele precisa aparecer.
    fn agendar(&mut self, trabalho: Trabalho) -> bool {
        if self.pendente.is_some() {
            return false;
        }
        self.pendente = Some(trabalho);
        true
    }

    fn fazer(&mut self, trabalho: Trabalho) {
        match trabalho {
            Trabalho::Salvar => self.salvar(),
            Trabalho::TestarRede => self.testar_rede(),
            Trabalho::Sair(Saida::Reiniciar) => psci::reiniciar(),
            Trabalho::Sair(Saida::Desligar) => psci::desligar(),
        }
    }

    /// Grava tema, aparencia, layout e pagina inicial. As outras chaves do
    /// arquivo ficam como estavam.
    fn salvar(&mut self) {
        let mut cfg = Config::carregar();
        config::guardar_aparencia(&mut cfg);
        cfg.definir("layout", config::texto_layout(self.layout));
        let pagina = self.campo_pagina.texto().trim();
        if !pagina.is_empty() {
            cfg.definir("pagina_inicial", pagina);
        }
        self.aviso = Some(match cfg.salvar() {
            Ok(()) => {
                self.modificado = false;
                (String::from("Salvo em /config/mxos.cfg."), Tom::Sucesso)
            }
            Err(e) => (format!("Não deu para salvar: {e}"), Tom::Erro),
        });
    }

    /// Um ping no gateway (bloqueia ate PRAZO_PING_MS).
    fn testar_rede(&mut self) {
        let resultado = match rede::info() {
            None => (String::from("Sem rede para testar."), Tom::Erro),
            Some(i) => match rede::ping(i.gateway, PRAZO_PING_MS) {
                Ok(ms) => (format!("O gateway {} respondeu em {ms} ms.", Ip(i.gateway)), Tom::Sucesso),
                Err(e) => (format!("O gateway {} não respondeu: {}.", Ip(i.gateway), e.descricao()), Tom::Erro),
            },
        };
        self.teste_rede = Some(resultado);
    }

    // ------------------------------------------------------------ eventos

    /// Saiu de uma aba: larga o foco e a confirmacao; a do Disco rele os discos.
    fn trocou_de_aba(&mut self) {
        self.campo_teste.focar(false);
        self.campo_pagina.focar(false);
        self.confirmar = None;
        if self.abas.atual() == ABA_DISCO {
            self.discos = armazenamento::discos();
        }
    }

    /// Entrega o evento as abas; se a aba mudou, arruma o resto.
    fn abas_evento(&mut self, e: EventoApp) -> bool {
        let antes = self.abas.atual();
        let redesenhar = self.abas.evento(e, area_abas());
        if self.abas.atual() != antes {
            self.trocou_de_aba();
            return true;
        }
        redesenhar
    }

    fn campo_focado(&self) -> bool {
        match self.abas.atual() {
            ABA_TECLADO => self.campo_teste.focado(),
            ABA_REDE => self.campo_pagina.focado(),
            _ => false,
        }
    }

    /// Entrega o evento ao campo de texto da aba atual, se ela tiver um.
    fn campo_evento(&mut self, e: EventoApp) -> bool {
        match self.abas.atual() {
            ABA_TECLADO => {
                let antes = self.campo_teste.focado();
                let acao = self.campo_teste.evento(e, area_campo_teste());
                if acao == AcaoCampo::Cancelou {
                    self.campo_teste.focar(false);
                }
                acao != AcaoCampo::Nada || antes != self.campo_teste.focado()
            }
            ABA_REDE => {
                let antes = self.campo_pagina.focado();
                let acao = self.campo_pagina.evento(e, area_campo_pagina());
                match acao {
                    AcaoCampo::Mudou => self.mudou(),
                    AcaoCampo::Confirmou | AcaoCampo::Cancelou => self.campo_pagina.focar(false),
                    AcaoCampo::Nada => {}
                }
                acao != AcaoCampo::Nada || antes != self.campo_pagina.focado()
            }
            _ => false,
        }
    }

    /// Clique do botao esquerdo: o campo sempre ve (e assim que perde o
    /// foco); depois vale o que estiver embaixo.
    fn clicar(&mut self, e: EventoApp, x: i32, y: i32) -> bool {
        let campo = self.campo_evento(e);
        let alvo = if y < ALTURA_ABAS {
            self.abas_evento(e)
        } else if y >= BASE_PAGINA {
            self.clicar_rodape(x, y)
        } else {
            self.clicar_pagina(x, y)
        };
        campo || alvo
    }

    fn clicar_rodape(&mut self, x: i32, y: i32) -> bool {
        if botao_salvar().contem(x, y) {
            return self.agendar(Trabalho::Salvar);
        }
        botao_padrao().contem(x, y) && self.restaurar_padrao()
    }

    fn clicar_pagina(&mut self, x: i32, y: i32) -> bool {
        match self.abas.atual() {
            ABA_PERSONALIZAR => self.clicar_personalizar(x, y),
            ABA_TECLADO => self.clicar_teclado(x, y),
            ABA_REDE => botao_testar().contem(x, y) && self.agendar(Trabalho::TestarRede),
            ABA_DISCO => self.clicar_disco(x, y),
            _ => self.clicar_sistema(x, y),
        }
    }

    fn clicar_personalizar(&mut self, x: i32, y: i32) -> bool {
        if let Some(i) = (0..TEMAS.len()).find(|&i| com_legenda(previa_tema(i)).contem(x, y)) {
            return self.escolher_tema(i);
        }
        if let Some(i) = (0..PapelDeParede::TODOS.len()).find(|&i| com_legenda(miniatura_papel(i)).contem(x, y)) {
            let papel = PapelDeParede::TODOS[i];
            return self.mudar_aparencia(|a| a.papel = papel);
        }
        if let Some(i) = (0..CORES_PAPEL.len()).find(|&i| quadrado_cor(i).contem(x, y)) {
            let cor = CORES_PAPEL[i];
            return self.mudar_aparencia(|a| a.cor_papel = cor);
        }
        if chave(0).contem(x, y) {
            return self.mudar_aparencia(|a| a.relogio_segundos = !a.relogio_segundos);
        }
        if chave(1).contem(x, y) {
            return self.mudar_aparencia(|a| a.icones = !a.icones);
        }
        chave(2).contem(x, y) && self.mudar_aparencia(|a| a.arrastar_contorno = !a.arrastar_contorno)
    }

    fn clicar_teclado(&mut self, x: i32, y: i32) -> bool {
        match (0..LAYOUTS.len()).find(|&i| cartao_layout(i).contem(x, y)) {
            Some(i) => self.escolher_layout(LAYOUTS[i].0),
            None => false,
        }
    }

    fn clicar_disco(&mut self, x: i32, y: i32) -> bool {
        if !botao_atualizar().contem(x, y) {
            return false;
        }
        self.discos = armazenamento::discos();
        true
    }

    /// Reiniciar e Desligar so pedem confirmacao; o "Sim" agenda a saida.
    fn clicar_sistema(&mut self, x: i32, y: i32) -> bool {
        if let Some(saida) = self.confirmar {
            if botao_sim().contem(x, y) {
                if self.agendar(Trabalho::Sair(saida)) {
                    self.confirmar = None;
                }
                return true;
            }
            if botao_nao().contem(x, y) {
                self.confirmar = None;
                return true;
            }
            return false;
        }
        let saida = if botao_reiniciar().contem(x, y) {
            Saida::Reiniciar
        } else if botao_desligar().contem(x, y) {
            Saida::Desligar
        } else {
            return false;
        };
        self.confirmar = Some(saida);
        true
    }

    /// Caractere ou tecla: a aba Teclado anota; o campo focado escreve; sem
    /// campo focado, Esc desiste da confirmacao.
    fn tecla(&mut self, e: EventoApp) -> bool {
        let anotou = self.abas.atual() == ABA_TECLADO && self.anotar_tecla(e);
        if self.campo_focado() {
            return self.campo_evento(e) || anotou;
        }
        if e == EventoApp::Caractere('\u{1b}') && self.confirmar.take().is_some() {
            return true;
        }
        anotou
    }

    /// Guarda a ultima tecla (e o caractere dela) para a aba Teclado.
    fn anotar_tecla(&mut self, e: EventoApp) -> bool {
        match (e, &mut self.ultima_tecla) {
            (EventoApp::Tecla { codigo, pressionada: true }, ultima) => {
                *ultima = Some((codigo, None));
                true
            }
            // o caractere vem logo depois da tecla que o escreveu
            (EventoApp::Caractere(c), Some((_, caractere @ None))) => {
                *caractere = Some(c);
                true
            }
            _ => false,
        }
    }

    /// O trabalho agendado; senao, uma vez por segundo, Rede e Sistema.
    fn tique(&mut self) -> bool {
        if let Some(trabalho) = self.pendente.take() {
            self.fazer(trabalho);
            return true;
        }
        let agora = segundos_ligado();
        if agora == self.segundo {
            return false;
        }
        self.segundo = agora;
        matches!(self.abas.atual(), ABA_REDE | ABA_SISTEMA)
    }

    // ------------------------------------------------------------ desenho

    fn desenhar_personalizar(&self, s: &mut Superficie, t: &Tema) {
        let a = tema::aparencia();
        titulo_secao(s, MARGEM, Y_TEMAS, "Tema", t);
        for (i, (nome, opcao)) in TEMAS.iter().enumerate() {
            let r = previa_tema(i);
            desenhar_previa(s, r, opcao, &a);
            legenda(s, r, nome, opcao == t, t);
        }
        titulo_secao(s, MARGEM, Y_PAPEIS, "Papel de parede", t);
        for (i, &papel) in PapelDeParede::TODOS.iter().enumerate() {
            let r = miniatura_papel(i);
            desenhar_miniatura(s, r, papel, a.cor_papel, t);
            legenda(s, r, nome_papel(papel), papel == a.papel, t);
        }
        titulo_secao(s, MARGEM, Y_CORES, "Cor do papel de parede", t);
        let atual = a.cor_papel & 0x00FF_FFFF;
        for (i, &cor) in CORES_PAPEL.iter().enumerate() {
            desenhar_cor(s, quadrado_cor(i), cor, cor == atual, t);
        }
        let ultima = quadrado_cor(CORES_PAPEL.len() - 1);
        widgets::rotulo(s, ultima.direita() + 16, ultima.y + 8, &format!("#{atual:06X}"), cor_fraca(t));
        titulo_secao(s, MARGEM, Y_CHAVES, "Área de trabalho", t);
        desenhar_chave(s, chave(0), "Relógio com segundos", "12:34:56 em vez de 12:34", a.relogio_segundos, t);
        desenhar_chave(s, chave(1), "Ícones na área de trabalho", "Atalhos na coluna da esquerda", a.icones, t);
        desenhar_chave(s, chave(2), "Arrastar só o contorno", "Mais leve: a janela vai ao soltar", a.arrastar_contorno, t);
    }

    fn desenhar_teclado(&self, s: &mut Superficie, t: &Tema) {
        titulo_secao(s, MARGEM, Y_LAYOUTS, "Layout do teclado", t);
        for (i, &(layout, nome, detalhe)) in LAYOUTS.iter().enumerate() {
            desenhar_opcao_layout(s, cartao_layout(i), nome, detalhe, layout == self.layout, t);
        }
        titulo_secao(s, MARGEM, Y_TESTE, "Experimente aqui", t);
        let campo = area_campo_teste();
        self.campo_teste.desenhar(s, campo);
        widgets::rotulo(s, MARGEM, campo.baixo() + 8, &self.texto_ultima_tecla(), cor_fraca(t));
        titulo_secao(s, MARGEM, Y_ATALHOS, "Atalhos", t);
        for (i, &(tecla, efeito)) in ATALHOS.iter().enumerate() {
            let y = Y_ATALHOS + ABAIXO_TITULO + i as i32 * ALTURA_LINHA;
            desenhar_tecla(s, MARGEM, y, tecla, t);
            widgets::rotulo(s, MARGEM + 96, y + 4, efeito, t.texto);
        }
        let y = Y_ATALHOS + ABAIXO_TITULO + ATALHOS.len() as i32 * ALTURA_LINHA + 10;
        let dica = "No ABNT2, acento seguido de letra escreve a letra acentuada (´ e a = á; ~ e o = õ).";
        widgets::rotulo(s, MARGEM, y, dica, cor_fraca(t));
        let dica = "A tecla Ç escreve ç; acento seguido de espaço escreve o próprio acento.";
        widgets::rotulo(s, MARGEM, y + 18, dica, cor_fraca(t));
    }

    /// "Ultima tecla: ..." embaixo do campo de teste (com o caractere que
    /// ela escreveu, acentos inclusive).
    fn texto_ultima_tecla(&self) -> String {
        match self.ultima_tecla {
            None => String::from("Aperte qualquer tecla: o código dela aparece aqui."),
            Some((codigo, Some(c))) if !c.is_control() => format!("Última tecla: código {codigo}, escreveu “{c}”"),
            Some((codigo, _)) => match nome_da_tecla(codigo) {
                Some(nome) => format!("Última tecla: código {codigo} ({nome})"),
                None => format!("Última tecla: código {codigo}"),
            },
        }
    }

    fn desenhar_rede(&self, s: &mut Superficie, t: &Tema) {
        let info = rede::info();
        widgets::icone(s, MARGEM, TOPO, Icone::Rede, 2);
        negrito(s, MARGEM + 44, TOPO + 4, "Rede virtio-net", t.texto, 1);
        let (estado, tom) = match info {
            Some(_) => ("Conectada, endereço recebido por DHCP", Tom::Sucesso),
            None => ("Desligada: sem virtio-net, ou o DHCP não respondeu", Tom::Erro),
        };
        widgets::rotulo(s, MARGEM + 44, TOPO + 20, estado, cor_do_tom(tom, t));

        let painel = Retangulo::novo(MARGEM, Y_PAINEL_REDE, LARGURA_UTIL, LINHAS_REDE * ALTURA_LINHA + 12);
        widgets::painel(s, painel);
        // sem rede a tabela fica igual, com tracos: nada pula de lugar
        let valor = |texto: fn(&rede::InfoRede) -> String| info.as_ref().map_or_else(|| String::from("-"), texto);
        let linhas = [
            ("MAC", valor(|i| format!("{}", Mac(i.mac)))),
            ("IP", valor(|i| format!("{}", Ip(i.ip)))),
            ("Máscara", valor(|i| format!("{}", Ip(i.mascara)))),
            ("Gateway", valor(|i| format!("{}", Ip(i.gateway)))),
            ("DNS", valor(|i| format!("{}", Ip(i.dns)))),
            ("Pacotes", valor(|i| format!("{} recebidos, {} enviados", i.pacotes_rx, i.pacotes_tx))),
        ];
        linhas_info(s, painel, &linhas, t);

        let botao = botao_testar();
        widgets::botao(s, botao, "Testar", false);
        let (texto, tom) = self.texto_teste();
        let x = botao.direita() + 12;
        widgets::rotulo(s, x, botao.y + 8, fonte::cortar(texto, LARGURA - MARGEM - x, &INTER), cor_do_tom(tom, t));

        titulo_secao(s, MARGEM, Y_NAVEGADOR, "Navegador", t);
        let campo = area_campo_pagina();
        widgets::rotulo(s, MARGEM, campo.y + 8, "Página inicial", t.texto);
        self.campo_pagina.desenhar(s, campo);
        let dica = "Vale para os navegadores abertos depois de Salvar.";
        widgets::rotulo(s, campo.x, campo.baixo() + 8, dica, cor_fraca(t));
    }

    fn texto_teste(&self) -> (&str, Tom) {
        if self.pendente == Some(Trabalho::TestarRede) {
            return ("Testando o gateway...", Tom::Normal);
        }
        match &self.teste_rede {
            Some((texto, tom)) => (texto.as_str(), *tom),
            None => ("Manda um ping ao gateway e mede a resposta.", Tom::Normal),
        }
    }

    fn desenhar_disco(&self, s: &mut Superficie, t: &Tema) {
        titulo_secao(s, MARGEM, TOPO + 4, "Discos", t);
        let contagem = format!("{} encontrado(s)", self.discos.len());
        widgets::rotulo(s, MARGEM + 64, TOPO + 4, &contagem, cor_fraca(t));
        widgets::botao(s, botao_atualizar(), "Atualizar", false);
        if self.discos.is_empty() {
            let r = cartao_disco(0);
            widgets::painel(s, r);
            widgets::icone(s, r.x + 12, r.y + 20, Icone::Disco, 2);
            negrito(s, r.x + 56, r.y + 20, "Nenhum disco", t.texto, 1);
            widgets::rotulo(s, r.x + 56, r.y + 36, "O QEMU precisa de um virtio-blk-device.", cor_fraca(t));
            return;
        }
        for (i, disco) in self.discos.iter().take(MAX_DISCOS).enumerate() {
            desenhar_um_disco(s, cartao_disco(i), disco, t);
        }
        if self.discos.len() > MAX_DISCOS {
            let y = cartao_disco(MAX_DISCOS).y;
            let resto = format!("+ {} outro(s): veja o comando \"discos\" no terminal", self.discos.len() - MAX_DISCOS);
            widgets::rotulo(s, MARGEM, y, &resto, cor_fraca(t));
        }
    }

    fn desenhar_sistema(&self, s: &mut Superficie, t: &Tema) {
        widgets::icone(s, MARGEM, TOPO, Icone::Chip, 3);
        negrito(s, MARGEM + 64, TOPO + 2, "MXOS", t.texto, 3);
        // a versao na mesma linha de base do nome, logo depois dele
        let x = MARGEM + 64 + INTER_GRANDE.largura("MXOS") + 10;
        s.texto(x, TOPO + 10, VERSAO, cor_escolha(t), 2);
        widgets::rotulo(s, MARGEM + 64, TOPO + 36, "Kernel ARM64 escrito do zero em Rust", cor_fraca(t));

        let painel = Retangulo::novo(MARGEM, Y_PAINEL_SISTEMA, LARGURA_UTIL, LINHAS_SISTEMA * ALTURA_LINHA + 12);
        widgets::painel(s, painel);
        let h = heap::estatisticas();
        let (usado, total) = ((h.total - h.livre) as u64, h.total as u64);
        let memoria = format!("{} usados de {}", tamanho(usado), tamanho(total));
        let fim_memoria = painel.x + 12 + COLUNA_VALOR + Superficie::largura_texto(&memoria, 1);
        let (midr, freq, psci_v) = (cpu::midr(), cpu::frequencia_timer(), self.versao_psci);
        let segundos = self.segundo;
        let linhas = [
            ("Plataforma", String::from(plataforma::NOME)),
            ("Processador", format!("{} (MIDR 0x{midr:08x})", cpu::nome_cpu(midr))),
            ("Timer", format!("{},{} MHz, tique a {} Hz", freq / 1_000_000, freq / 100_000 % 10, timer::HZ)),
            ("Firmware", format!("PSCI v{}.{} via {}", psci_v >> 16 & 0xFFFF, psci_v & 0xFFFF, psci::via().nome())),
            ("Memória", memoria),
            ("Ligado há", format!("{}h {:02}m {:02}s", segundos / 3600, segundos / 60 % 60, segundos % 60)),
        ];
        linhas_info(s, painel, &linhas, t);
        // barra do heap na mesma linha, depois do texto da memoria
        let x = fim_memoria + 16;
        let barra = Retangulo::novo(x, linha_painel(painel, 4) - 1, painel.direita() - 12 - x, 10);
        widgets::barra_progresso(s, barra, milesimos(usado, total));

        match self.confirmar {
            Some(saida) => self.desenhar_confirmacao(s, saida, t),
            None => {
                widgets::botao(s, botao_reiniciar(), "Reiniciar", false);
                widgets::botao(s, botao_desligar(), "Desligar", false);
                let x = botao_desligar().direita() + 16;
                widgets::rotulo(s, x, Y_SAIR + 8, "Pelo firmware (PSCI).", cor_fraca(t));
            }
        }
    }

    /// Faixa de aviso no lugar dos botoes Reiniciar e Desligar.
    fn desenhar_confirmacao(&self, s: &mut Superficie, saida: Saida, t: &Tema) {
        let r = faixa_confirmacao();
        s.retangulo(r, grafico::misturar(t.fundo_janela, t.fechar, 30));
        s.contorno(r, grafico::misturar(t.fundo_janela, t.fechar, 140), 1);
        widgets::icone(s, r.x + 12, r.y + 12, Icone::Info, 2);
        negrito(s, r.x + 52, r.y + 14, saida.pergunta(), t.texto, 1);
        let detalhe = if self.modificado { "O que não foi salvo se perde." } else { "Os arquivos já estão no disco." };
        widgets::rotulo(s, r.x + 52, r.y + 32, detalhe, cor_fraca(t));
        widgets::botao(s, botao_sim(), saida.confirmar(), true);
        widgets::botao(s, botao_nao(), "Cancelar", false);
    }

    fn desenhar_rodape(&self, s: &mut Superficie, t: &Tema) {
        let r = Retangulo::novo(0, BASE_PAGINA, LARGURA, ALTURA_RODAPE);
        s.retangulo(r, grafico::misturar(t.fundo_janela, t.botao, 110));
        s.retangulo(Retangulo::novo(0, r.y, LARGURA, 1), widgets::cor_linha(t));
        let y = r.y + (r.altura - 8) / 2;
        let mut x = MARGEM;
        if self.modificado && self.aviso.is_none() {
            // bolinha de "ainda nao salvo"
            s.circulo_cheio(x + 3, y + 3, 3, cor_escolha(t));
            x += 14;
        }
        let (texto, tom) = self.texto_rodape();
        let livre = botao_padrao().x - 12 - x;
        widgets::rotulo(s, x, y, fonte::cortar(texto, livre, &INTER), cor_do_tom(tom, t));
        widgets::botao(s, botao_padrao(), "Padrão", false);
        widgets::botao(s, botao_salvar(), "Salvar", true);
    }

    fn texto_rodape(&self) -> (&str, Tom) {
        if self.pendente == Some(Trabalho::Salvar) {
            return ("Salvando em /config/mxos.cfg...", Tom::Normal);
        }
        match &self.aviso {
            Some((texto, tom)) => (texto.as_str(), *tom),
            None if self.modificado => ("Alterações aplicadas e ainda não salvas.", Tom::Normal),
            None => ("Tudo vale na hora; Salvar guarda no disco.", Tom::Normal),
        }
    }
}

impl App for Configuracoes {
    fn titulo(&self) -> &str {
        TITULO
    }

    fn tamanho(&self) -> (usize, usize) {
        (LARGURA as usize, ALTURA as usize)
    }

    fn desenhar(&mut self, s: &mut Superficie) {
        let t = tema::atual();
        if let Some(Trabalho::Sair(saida)) = self.pendente {
            desenhar_saindo(s, saida, &t);
            return;
        }
        s.limpar(t.fundo_janela);
        self.abas.desenhar(s, area_abas());
        match self.abas.atual() {
            ABA_PERSONALIZAR => self.desenhar_personalizar(s, &t),
            ABA_TECLADO => self.desenhar_teclado(s, &t),
            ABA_REDE => self.desenhar_rede(s, &t),
            ABA_DISCO => self.desenhar_disco(s, &t),
            _ => self.desenhar_sistema(s, &t),
        }
        self.desenhar_rodape(s, &t);
    }

    fn evento(&mut self, e: EventoApp) -> bool {
        if matches!(self.pendente, Some(Trabalho::Sair(_))) {
            // saindo: so falta o Tique que chama o firmware
            return e == EventoApp::Tique && self.tique();
        }
        match e {
            EventoApp::Tique => self.tique(),
            EventoApp::Botao { x, y, botao: Botao::Esquerdo, pressionado: true } => self.clicar(e, x, y),
            EventoApp::Botao { .. } | EventoApp::Mouse { .. } => self.abas_evento(e) | self.campo_evento(e),
            EventoApp::Caractere(_) | EventoApp::Tecla { .. } => self.tecla(e),
            EventoApp::Roda(_) | EventoApp::Processos | EventoApp::Quadro => false,
        }
    }

    fn pedidos(&mut self) -> Vec<Pedido> {
        core::mem::take(&mut self.pedidos)
    }
}

// ---------------------------------------------------------------- onde fica

fn area_abas() -> Retangulo {
    Retangulo::novo(0, 0, LARGURA, ALTURA_ABAS)
}

/// Cartao `i` de uma fileira de `n` cartoes iguais na largura util.
fn cartao_na_fileira(i: usize, n: usize, y: i32, altura: i32) -> Retangulo {
    let n = n.max(1) as i32;
    let largura = (LARGURA_UTIL - (n - 1) * ESPACO_CARTAO) / n;
    Retangulo::novo(MARGEM + i as i32 * (largura + ESPACO_CARTAO), y, largura, altura)
}

/// O cartao mais o nome embaixo dele (tudo clicavel).
fn com_legenda(r: Retangulo) -> Retangulo {
    Retangulo::novo(r.x, r.y, r.largura, r.altura + ALTURA_LEGENDA)
}

fn previa_tema(i: usize) -> Retangulo {
    cartao_na_fileira(i, TEMAS.len(), Y_TEMAS + ABAIXO_TITULO, ALTURA_PREVIA)
}

fn miniatura_papel(i: usize) -> Retangulo {
    cartao_na_fileira(i, PapelDeParede::TODOS.len(), Y_PAPEIS + ABAIXO_TITULO, ALTURA_MINIATURA)
}

fn quadrado_cor(i: usize) -> Retangulo {
    let x = MARGEM + i as i32 * (LADO_COR + ESPACO_COR);
    Retangulo::novo(x, Y_CORES + ABAIXO_TITULO, LADO_COR, LADO_COR)
}

/// Chave `i` da secao Area de trabalho (0 relogio, 1 icones), a linha toda.
/// As chaves liga/desliga, duas por fileira.
fn chave(i: usize) -> Retangulo {
    let coluna = LARGURA_UTIL / 2;
    let (fileira, lado) = ((i / 2) as i32, (i % 2) as i32);
    Retangulo::novo(MARGEM + lado * coluna, Y_CHAVES + ABAIXO_TITULO + fileira * 34, coluna - 8, 22)
}

fn cartao_layout(i: usize) -> Retangulo {
    cartao_na_fileira(i, LAYOUTS.len(), Y_LAYOUTS + ABAIXO_TITULO, ALTURA_CARTAO_LAYOUT)
}

fn area_campo_teste() -> Retangulo {
    Retangulo::novo(MARGEM, Y_TESTE + ABAIXO_TITULO, LARGURA_UTIL, ALTURA_BOTAO)
}

fn botao_testar() -> Retangulo {
    Retangulo::novo(MARGEM, Y_TESTAR, LARGURA_BOTAO, ALTURA_BOTAO)
}

fn area_campo_pagina() -> Retangulo {
    Retangulo::novo(MARGEM + ROTULO_PAGINA, Y_NAVEGADOR + ABAIXO_TITULO, LARGURA_UTIL - ROTULO_PAGINA, ALTURA_BOTAO)
}

fn botao_atualizar() -> Retangulo {
    Retangulo::novo(LARGURA - MARGEM - LARGURA_BOTAO, TOPO - 4, LARGURA_BOTAO, ALTURA_BOTAO)
}

fn cartao_disco(i: usize) -> Retangulo {
    Retangulo::novo(MARGEM, Y_DISCOS + i as i32 * (ALTURA_DISCO + 10), LARGURA_UTIL, ALTURA_DISCO)
}

fn botao_reiniciar() -> Retangulo {
    Retangulo::novo(MARGEM, Y_SAIR, 112, ALTURA_BOTAO)
}

fn botao_desligar() -> Retangulo {
    botao_reiniciar().deslocado(112 + ESPACO_CARTAO, 0)
}

fn faixa_confirmacao() -> Retangulo {
    Retangulo::novo(MARGEM, Y_SAIR - 16, LARGURA_UTIL, 56)
}

fn botao_nao() -> Retangulo {
    let f = faixa_confirmacao();
    Retangulo::novo(f.direita() - 12 - LARGURA_BOTAO, f.y + 16, LARGURA_BOTAO, ALTURA_BOTAO)
}

fn botao_sim() -> Retangulo {
    let nao = botao_nao();
    Retangulo::novo(nao.x - 8 - LARGURA_SIM, nao.y, LARGURA_SIM, ALTURA_BOTAO)
}

fn botao_salvar() -> Retangulo {
    let y = BASE_PAGINA + (ALTURA_RODAPE - ALTURA_BOTAO) / 2;
    Retangulo::novo(LARGURA - MARGEM - LARGURA_BOTAO, y, LARGURA_BOTAO, ALTURA_BOTAO)
}

fn botao_padrao() -> Retangulo {
    botao_salvar().deslocado(-(LARGURA_BOTAO + 8), 0)
}

// ---------------------------------------------------------------- cores
// (as de linha, texto fraco e campo sao as do widgets, iguais em todo app)

/// Brilho percebido de uma cor (0 a 255).
fn luminancia(cor: Cor) -> u32 {
    let (r, g, b) = (cor >> 16 & 0xFF, cor >> 8 & 0xFF, cor & 0xFF);
    (r * 299 + g * 587 + b * 114) / 1000
}

/// Texto secundario: explicacoes e rotulos dos paineis.
fn cor_fraca(t: &Tema) -> Cor {
    widgets::cor_texto_fraco(t)
}

/// Item escolhido: o destaque do tema (mais claro nos temas escuros).
fn cor_escolha(t: &Tema) -> Cor {
    if widgets::tema_escuro(t) { area::clarear(t.destaque, 90) } else { t.destaque }
}

fn cor_do_tom(tom: Tom, t: &Tema) -> Cor {
    let escuro = widgets::tema_escuro(t);
    match tom {
        Tom::Normal => cor_fraca(t),
        Tom::Sucesso if escuro => 0x0073_C991,
        Tom::Sucesso => 0x0021_7A45,
        Tom::Erro if escuro => area::clarear(t.fechar, 70),
        Tom::Erro => t.fechar,
    }
}

// ---------------------------------------------------------------- pedacos

/// A versao em negrito da fonte de cada escala (a de 20 e a de 32 px ja
/// sao seminegrito e negrito).
fn fonte_negrito(escala: usize) -> &'static Fonte {
    if escala <= 1 { &INTER_NEGRITO } else { fonte::por_escala(escala) }
}

/// Texto em negrito, com as maiusculas na mesma faixa que texto() usaria.
fn negrito(s: &mut Superficie, x: i32, y: i32, texto: &str, cor: Cor, escala: usize) {
    let f = fonte_negrito(escala);
    s.texto_com(x, f.base_centralizada(y, 8 * escala as i32), texto, cor, f);
}

fn titulo_secao(s: &mut Superficie, x: i32, y: i32, texto: &str, t: &Tema) {
    negrito(s, x, y, texto, t.texto, 1);
}

/// y do texto da linha `i` de um painel.
fn linha_painel(painel: Retangulo, i: usize) -> i32 {
    painel.y + 6 + i as i32 * ALTURA_LINHA + (ALTURA_LINHA - 8) / 2
}

/// Linhas "rotulo  valor" dentro de um painel.
fn linhas_info(s: &mut Superficie, painel: Retangulo, linhas: &[(&str, String)], t: &Tema) {
    let x = painel.x + 12 + COLUNA_VALOR;
    for (i, (rotulo, valor)) in linhas.iter().enumerate() {
        let y = linha_painel(painel, i);
        widgets::rotulo(s, painel.x + 12, y, rotulo, cor_fraca(t));
        widgets::rotulo(s, x, y, fonte::cortar(valor, painel.direita() - 12 - x, &INTER), t.texto);
    }
}

/// Nome embaixo de um cartao. O escolhido ganha um anel de 2 px (com 1 px
/// de folga) e o nome em negrito na cor de destaque.
fn legenda(s: &mut Superficie, r: Retangulo, nome: &str, escolhido: bool, t: &Tema) {
    let f = if escolhido { &INTER_NEGRITO } else { &INTER };
    let texto = fonte::cortar(nome, r.largura, f);
    let x = r.x + (r.largura - f.largura(texto)) / 2;
    let y = r.baixo() + 6;
    if escolhido {
        s.contorno(Retangulo::novo(r.x - 3, r.y - 3, r.largura + 6, r.altura + 6), cor_escolha(t), 2);
        negrito(s, x, y, texto, cor_escolha(t), 1);
    } else {
        s.contorno(r, widgets::cor_linha(t), 1);
        s.texto(x, y, texto, t.texto, 1);
    }
}

/// Pinta so a parte de `r` que cai dentro de `limite`.
fn pintar_recortado(s: &mut Superficie, r: Retangulo, limite: Retangulo, cor: Cor) {
    let alvo = r.intersecao(&limite);
    if !alvo.vazio() {
        s.retangulo(alvo, cor);
    }
}

/// Cada canal vezes fator/256 (no maximo 255), como no area::pintar_papel.
fn escalar(cor: Cor, fator: u32) -> Cor {
    let canal = |deslocamento: u32| ((cor >> deslocamento & 0xFF) * fator / 256).min(255);
    canal(16) << 16 | canal(8) << 8 | canal(0)
}

/// O papel de parede em ponto pequeno dentro de `r`, com as cores do de
/// verdade (area::pintar_papel): o degrade da cor, escuro em cima e claro
/// embaixo, e o enfeite por cima, linha a linha. As faixas, quadrados e
/// pontos ficam menores que os da tela, mas ainda dao para ver.
fn pintar_papel(s: &mut Superficie, r: Retangulo, papel: PapelDeParede, cor: Cor) {
    let (cima, baixo) = (escalar(cor, 176), escalar(cor, 435));
    let passos = (r.altura - 1).max(1);
    for dy in 0..r.altura {
        let base = match papel {
            PapelDeParede::Solido => cor,
            _ => grafico::misturar(cima, baixo, (dy * 255 / passos) as u32),
        };
        let y = r.y + dy;
        s.retangulo(Retangulo::novo(r.x, y, r.largura, 1), base);
        // (primeiro x, largura e passo do enfeite nesta linha, quanto clareia)
        let (inicio, largura, passo, claro) = match papel {
            PapelDeParede::Gradiente | PapelDeParede::Solido => continue,
            // cada linha anda 1 px: as faixas ficam diagonais
            PapelDeParede::Listras => (-(dy % 12), 6, 12, 14),
            PapelDeParede::Xadrez => (dy / 8 % 2 * 8, 8, 16, 12),
            PapelDeParede::Pontos if dy % 8 == 3 || dy % 8 == 4 => (3, 2, 8, 48),
            PapelDeParede::Pontos => continue,
        };
        let enfeite = area::clarear(base, claro);
        let mut x = r.x + inicio;
        while x < r.direita() {
            pintar_recortado(s, Retangulo::novo(x, y, largura, 1), r, enfeite);
            x += passo;
        }
    }
}

/// Barra de tarefas de brinquedo: botao MXOS, um botao de janela e o relogio.
fn barra_mini(s: &mut Superficie, barra: Retangulo, t: &Tema) {
    s.gradiente_vertical(barra, area::clarear(t.barra_tarefas, 24), t.barra_tarefas);
    let altura = (barra.altura - 4).max(1);
    s.retangulo(Retangulo::novo(barra.x + 3, barra.y + 2, 12, altura), t.botao);
    s.retangulo(Retangulo::novo(barra.x + 18, barra.y + 2, 22, altura), t.botao_destaque);
    s.retangulo(Retangulo::novo(barra.direita() - 17, barra.y + barra.altura / 2 - 1, 14, 2), t.barra_texto);
}

/// Janela de brinquedo: sombra (na cor `sombra`), borda, titulo com o [x]
/// e linhas de texto.
fn janela_mini(s: &mut Superficie, r: Retangulo, t: &Tema, ativa: bool, sombra: Cor) {
    s.retangulo(r.deslocado(2, 2), sombra);
    s.retangulo(r, t.borda);
    let dentro = Retangulo::novo(r.x + 1, r.y + 1, r.largura - 2, r.altura - 2);
    s.retangulo(dentro, t.fundo_janela);
    let titulo = Retangulo::novo(dentro.x, dentro.y, dentro.largura, 8);
    let base = if ativa { t.titulo_ativo } else { t.titulo_inativo };
    s.gradiente_vertical(titulo, area::clarear(base, 36), base);
    s.retangulo(Retangulo::novo(titulo.x + 3, titulo.y + 3, titulo.largura / 3, 2), t.titulo_texto);
    s.retangulo(Retangulo::novo(titulo.direita() - 7, titulo.y + 1, 6, 6), t.fechar);
    // tres linhas de "texto" de larguras diferentes
    let tinta = grafico::misturar(t.texto, t.fundo_janela, 70);
    for (i, porcento) in [72, 88, 54].into_iter().enumerate() {
        let y = titulo.baixo() + 4 + i as i32 * 5;
        if y + 2 <= dentro.baixo() - 2 {
            s.retangulo(Retangulo::novo(dentro.x + 4, y, (dentro.largura - 8) * porcento / 100, 2), tinta);
        }
    }
    if ativa {
        // um botao com a cor de destaque no canto
        s.retangulo(Retangulo::novo(dentro.direita() - 18, dentro.baixo() - 8, 14, 5), t.destaque);
    }
}

/// Miniatura da area de trabalho com o tema `t` e o papel de parede atual:
/// uma janela ativa na frente de uma inativa e a barra de tarefas.
fn desenhar_previa(s: &mut Superficie, r: Retangulo, t: &Tema, a: &Aparencia) {
    let area_janelas = Retangulo::novo(r.x, r.y, r.largura, r.altura - ALTURA_BARRA_MINI - 1);
    pintar_papel(s, area_janelas, a.papel, a.cor_papel);
    // a sombra cai sobre o papel de parede: a cor dele puxada para a do tema
    let sombra = grafico::misturar(a.cor_papel, t.sombra, 150);
    let (l, h) = (r.largura, area_janelas.altura);
    let atras = Retangulo::novo(r.x + l * 44 / 100, r.y + 5, l * 46 / 100, h * 62 / 100);
    janela_mini(s, atras, t, false, sombra);
    let frente = Retangulo::novo(r.x + l * 9 / 100, r.y + h * 22 / 100, l * 54 / 100, h * 70 / 100);
    janela_mini(s, frente, t, true, sombra);
    let barra = Retangulo::novo(r.x, r.baixo() - ALTURA_BARRA_MINI - 1, r.largura, ALTURA_BARRA_MINI + 1);
    barra_mini(s, barra, t);
}

/// Miniatura de um tipo de papel de parede, com a barra do tema atual.
fn desenhar_miniatura(s: &mut Superficie, r: Retangulo, papel: PapelDeParede, cor: Cor, t: &Tema) {
    let altura = r.altura - ALTURA_BARRA_MINI;
    pintar_papel(s, Retangulo::novo(r.x, r.y, r.largura, altura), papel, cor);
    barra_mini(s, Retangulo::novo(r.x, r.y + altura, r.largura, ALTURA_BARRA_MINI), t);
}

/// Quadrado de uma cor do papel; a escolhida ganha anel e um "v".
fn desenhar_cor(s: &mut Superficie, r: Retangulo, cor: Cor, escolhida: bool, t: &Tema) {
    s.retangulo(r, cor);
    s.contorno(r, area::escurecer(cor, 60), 1);
    if escolhida {
        s.contorno(Retangulo::novo(r.x - 3, r.y - 3, r.largura + 6, r.altura + 6), cor_escolha(t), 2);
        marca(s, r, if luminancia(cor) > 150 { 0x0020_2020 } else { BRANCO });
    }
}

/// Um "v" de 2 px no meio de `r`.
fn marca(s: &mut Superficie, r: Retangulo, cor: Cor) {
    let (x, y) = (r.x + r.largura / 2, r.y + r.altura / 2);
    for d in 0..2 {
        s.linha(x - 6, y - 1 + d, x - 2, y + 3 + d, cor);
        s.linha(x - 2, y + 3 + d, x + 5, y - 4 + d, cor);
    }
}

/// Chave liga/desliga (trilho redondo e bolinha) com titulo e explicacao.
fn desenhar_chave(s: &mut Superficie, r: Retangulo, texto: &str, detalhe: &str, ligada: bool, t: &Tema) {
    let (x, y) = (r.x, r.y);
    let trilho = if ligada { cor_escolha(t) } else { grafico::misturar(t.borda, t.fundo_janela, 120) };
    s.circulo_cheio(x + 9, y + 9, 9, trilho);
    s.circulo_cheio(x + 28, y + 9, 9, trilho);
    s.retangulo(Retangulo::novo(x + 9, y, 20, 19), trilho);
    s.circulo_cheio(if ligada { x + 28 } else { x + 9 }, y + 9, 7, BRANCO);
    // duas linhas de Inter 13 px: 16 px entre as linhas de base
    let livre = r.direita() - (x + 50);
    s.texto_com(x + 50, y + 9, fonte::cortar(texto, livre, &INTER), t.texto, &INTER);
    s.texto_com(x + 50, y + 25, fonte::cortar(detalhe, livre, &INTER), cor_fraca(t), &INTER);
}

/// Botao de radio: anel com o miolo da cor dos campos e, se escolhido, o
/// ponto no meio. O anel vazio usa o texto fraco (aparece no tema escuro).
fn bolinha(s: &mut Superficie, cx: i32, cy: i32, escolhida: bool, t: &Tema) {
    let cor = if escolhida { cor_escolha(t) } else { cor_fraca(t) };
    s.circulo_cheio(cx, cy, 8, cor);
    s.circulo_cheio(cx, cy, 6, widgets::cor_campo(t));
    if escolhida {
        s.circulo_cheio(cx, cy, 3, cor);
    }
}

/// Cartao de um layout: bolinha de escolha, icone, nome grande e explicacao.
fn desenhar_opcao_layout(s: &mut Superficie, r: Retangulo, nome: &str, detalhe: &str, escolhido: bool, t: &Tema) {
    widgets::painel(s, r);
    if escolhido {
        s.contorno(r, cor_escolha(t), 2);
    }
    let meio = r.y + r.altura / 2;
    bolinha(s, r.x + 20, meio, escolhido, t);
    widgets::icone(s, r.x + 40, meio - 16, Icone::Teclado, 2);
    negrito(s, r.x + 84, r.y + 14, nome, t.texto, 2);
    widgets::rotulo(s, r.x + 84, r.y + 38, detalhe, cor_fraca(t));
}

/// Nome de uma tecla desenhado como uma tecla (lista de atalhos).
fn desenhar_tecla(s: &mut Superficie, x: i32, y: i32, nome: &str, t: &Tema) {
    let r = Retangulo::novo(x, y - 1, Superficie::largura_texto(nome, 1) + 14, 18);
    s.retangulo(r, t.botao);
    s.contorno(r, widgets::cor_linha(t), 1);
    // a borda de baixo mais grossa da o volume de uma tecla
    s.retangulo(Retangulo::novo(r.x + 1, r.baixo() - 2, r.largura - 2, 1), area::escurecer(t.botao, 40));
    s.texto_com(r.x + 7, INTER.base_centralizada(r.y, r.altura - 2), nome, t.botao_texto, &INTER);
}

/// Um disco: icone, nome e rotulo, sistema e tamanho, barra de uso.
fn desenhar_um_disco(s: &mut Superficie, r: Retangulo, d: &InfoDisco, t: &Tema) {
    widgets::painel(s, r);
    widgets::icone(s, r.x + 12, r.y + 20, Icone::Disco, 2);
    let x = r.x + 56;
    let nome = if d.rotulo.is_empty() { d.nome.clone() } else { format!("{}  \"{}\"", d.nome, d.rotulo) };
    negrito(s, x, r.y + 10, &nome, t.texto, 1);
    let onde = if d.montado { "montado em /" } else { "não montado" };
    let resumo = format!("{}, {} no total, {onde}", d.sistema, tamanho(d.setores * 512));
    widgets::rotulo(s, x, r.y + 27, &resumo, cor_fraca(t));
    if d.tamanho == 0 {
        return; // sem sistema de arquivos conhecido: nao ha uso para mostrar
    }
    let usado = d.tamanho.saturating_sub(d.livre);
    widgets::barra_progresso(s, Retangulo::novo(x, r.y + 42, r.direita() - 12 - x, 10), milesimos(usado, d.tamanho));
    let uso = format!("{} livres de {} ({} usados)", tamanho(d.livre), tamanho(d.tamanho), tamanho(usado));
    widgets::rotulo(s, x, r.y + 58, &uso, t.texto);
}

/// Tela de "Reiniciando..." enquanto espera o Tique que chama o firmware.
fn desenhar_saindo(s: &mut Superficie, saida: Saida, t: &Tema) {
    s.limpar(grafico::misturar(t.fundo_janela, t.titulo_ativo, 40));
    let r = Retangulo::novo((LARGURA - 360) / 2, (ALTURA - 100) / 2, 360, 100);
    widgets::painel(s, r);
    widgets::icone(s, r.x + 20, r.y + 26, Icone::Info, 3);
    negrito(s, r.x + 88, r.y + 36, saida.andamento(), t.texto, 1);
    widgets::rotulo(s, r.x + 88, r.y + 56, "Até a próxima!", cor_fraca(t));
}

// ---------------------------------------------------------------- textos

/// Nome de cada papel na tela (o do arquivo de configuracao fica sem acento).
fn nome_papel(papel: PapelDeParede) -> &'static str {
    match papel {
        PapelDeParede::Gradiente => "Gradiente",
        PapelDeParede::Solido => "Sólido",
        PapelDeParede::Listras => "Listras",
        PapelDeParede::Xadrez => "Xadrez",
        PapelDeParede::Pontos => "Pontos",
    }
}

/// Nome de uma tecla que nao escreve nada (setas, F1, Shift...).
fn nome_da_tecla(codigo: u16) -> Option<String> {
    if let Some(nome) = teclado::nome_tecla(codigo) {
        return Some(String::from(nome));
    }
    if let Some(n) = teclado::tecla_de_funcao(codigo) {
        return Some(format!("F{n}"));
    }
    let nome = match codigo {
        teclado::KEY_LEFTSHIFT | teclado::KEY_RIGHTSHIFT => "Shift",
        teclado::KEY_LEFTCTRL | teclado::KEY_RIGHTCTRL => "Ctrl",
        teclado::KEY_LEFTALT => "Alt",
        teclado::KEY_RIGHTALT => "AltGr",
        teclado::KEY_CAPSLOCK => "CapsLock",
        teclado::KEY_NUMLOCK => "NumLock",
        teclado::KEY_LEFTMETA | teclado::KEY_RIGHTMETA => "Windows",
        _ => return None,
    };
    Some(String::from(nome))
}

/// Bytes em KB, MB ou GB (como o "mem" do shell: 10 MB ou mais em MB).
fn tamanho(bytes: u64) -> String {
    match bytes {
        b if b >= 10 << 30 => format!("{} GB", b >> 30),
        b if b >= 10 << 20 => format!("{} MB", b >> 20),
        b if b >= 1 << 10 => format!("{} KB", b >> 10),
        b => format!("{b} bytes"),
    }
}

/// `parte` de `todo` em milesimos (0 a 1000), para as barras.
fn milesimos(parte: u64, todo: u64) -> u32 {
    if todo == 0 {
        return 0;
    }
    (parte.min(todo) * 1000 / todo) as u32
}

fn segundos_ligado() -> u64 {
    cpu::contador() / cpu::frequencia_timer().max(1)
}
