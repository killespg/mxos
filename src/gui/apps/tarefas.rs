//! Gerenciador de tarefas: as janelas abertas e o desempenho da maquina.
//!
//! Aba "Aplicativos": uma linha por janela (janelas::resumo) com a CPU do
//! ultimo segundo (quanto do tempo o app passou em evento + desenhar) e a
//! memoria do conteudo dela. "Finalizar tarefa" (ou Delete) pede ao
//! gerenciador de janelas que feche a selecionada.
//!
//! Aba "Processos": os programas rodando em EL0 (processo::lista), com o
//! PID, o estado, a CPU do ultimo segundo e a memoria de cada um.
//! "Finalizar processo" (ou Delete) mata o selecionado.
//!
//! Aba "Desempenho": uso da CPU nos ultimos 60 s (o tempo que ela NAO
//! passou dormindo no WFI, de irq::contadores), memoria do heap,
//! interrupcoes por segundo e tempo ligado.
//!
//! Mede tudo uma vez por segundo no Tique, inclusive minimizada (para o
//! grafico nao ficar com buraco), e so entao pede para redesenhar.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::entrada::Botao;
use crate::entrada::teclado::KEY_DELETE;
use crate::gui::grafico::{self, Cor, Retangulo, Superficie};
use crate::gui::janelas::{self, App, EventoApp, Pedido};
use crate::gui::tema::{self, Tema};
use crate::gui::widgets::{self, ALTURA_BOTAO, Abas, AcaoLista, Icone, ItemLista, Lista};
use crate::fonte::{self, Fonte, INTER, INTER_NEGRITO, INTER_TITULO};
use crate::{cpu, heap, irq, processo};

pub const TITULO: &str = "Tarefas";
const LARGURA: i32 = 560;
const ALTURA: i32 = 420;
const MARGEM: i32 = 8;
const ESPACO: i32 = 8;
const ALTURA_ABAS: i32 = 28;
/// Onde comeca o conteudo das abas.
const TOPO: i32 = MARGEM + ALTURA_ABAS + ESPACO;
const NOMES_ABAS: [&str; 3] = ["Aplicativos", "Processos", "Desempenho"];
const ABA_APLICATIVOS: usize = 0;
const ABA_PROCESSOS: usize = 1;
const ABA_DESEMPENHO: usize = 2;
const COLUNAS: [&str; 3] = ["Nome", "CPU", "Memória"];
const LARGURAS: [i32; 3] = [330, 80, 100];
const COLUNAS_PROCESSOS: [&str; 5] = ["PID", "Nome", "Estado", "CPU", "Memória"];
const LARGURAS_PROCESSOS: [i32; 5] = [50, 160, 140, 70, 90];
const LARGURA_FINALIZAR: i32 = 136;
/// Segundos guardados no grafico da CPU.
const AMOSTRAS: usize = 60;
/// Por quanto tempo uma janela que mandamos fechar fica fora da lista,
/// mesmo que o resumo (refeito no tique do gerenciador) ainda a mostre.
const SEGUNDOS_FECHANDO: u64 = 2;
// Paineis da aba Desempenho, de cima para baixo.
const ALTURA_CPU: i32 = 188;
const ALTURA_MEMORIA: i32 = 72;
const ALTURA_SISTEMA: i32 = ALTURA - TOPO - MARGEM - ALTURA_CPU - ALTURA_MEMORIA - 2 * ESPACO;
/// Margem interna dos paineis e distancia entre as linhas "chave  valor".
const DENTRO: i32 = 12;
const PASSO_LINHA: i32 = 18;
/// Linha de base da primeira linha de texto de um painel (deixa espaco
/// para o acento das maiusculas: "Á", "É").
const PRIMEIRA_BASE: i32 = 24;
/// Onde o valor comeca nas linhas "chave  valor".
const COLUNA_VALOR: i32 = 128;
/// Quanto o texto secundario puxa para a cor do fundo (0 a 255).
const APAGADO: u32 = 90;

/// Icone pelo comeco do titulo; janela desconhecida ganha um monitor.
const ICONES: [(&str, Icone); 12] = [
    ("Terminal", Icone::Terminal),
    ("Arquivos", Icone::Pasta),
    ("Discos", Icone::Disco),
    ("Editor", Icone::Texto),
    ("Navegador", Icone::Navegador),
    ("Tarefas", Icone::Tarefas),
    ("Drivers", Icone::Chip),
    // com e sem acento, para nao depender da versao do titulo
    ("Configura", Icone::Engrenagem),
    ("Personalizar", Icone::Pincel),
    ("Calculadora", Icone::Calculadora),
    ("Pintura", Icone::Pincel),
    ("Sobre", Icone::Info),
];

/// Para o menu iniciar e o icone da area de trabalho.
pub fn criar() -> Box<dyn App> {
    Box::new(Tarefas::nova())
}

/// Uma janela na aba Aplicativos.
struct Linha {
    id: usize,
    titulo: String,
    /// CPU no ultimo segundo, em milesimos (None = ainda sem medida).
    cpu: Option<u32>,
    memoria: usize,
}

/// Um processo na aba Processos.
struct LinhaProcesso {
    pid: u32,
    nome: String,
    estado: &'static str,
    cpu: Option<u32>,
    memoria: usize,
}

/// Os contadores de irq::contadores que a aba Desempenho compara.
#[derive(Clone, Copy)]
struct Contagem {
    ocioso: u64,
    desde_inicio: u64,
    timer: u64,
    serial: u64,
    outras: u64,
}

impl Contagem {
    fn agora() -> Contagem {
        let c = irq::contadores();
        Contagem {
            ocioso: c.ocioso,
            desde_inicio: c.desde_inicio,
            timer: c.timer,
            serial: c.serial,
            outras: c.outras + c.espurias,
        }
    }
}

/// O que a aba Desempenho mostra, medido uma vez por segundo.
struct Desempenho {
    /// Uso da CPU em milesimos, um por segundo; o mais novo no fim.
    historico: [u32; AMOSTRAS],
    anterior: Contagem,
    /// Interrupcoes por segundo na ultima amostra: timer, serial, outras.
    por_segundo: [u64; 3],
}

impl Desempenho {
    fn novo() -> Desempenho {
        Desempenho { historico: [0; AMOSTRAS], anterior: Contagem::agora(), por_segundo: [0; 3] }
    }

    /// Fecha um segundo: o uso vai para o fim do historico.
    fn amostrar(&mut self) {
        let agora = Contagem::agora();
        let antes = core::mem::replace(&mut self.anterior, agora);
        let passou = agora.desde_inicio.wrapping_sub(antes.desde_inicio).max(1);
        let dormiu = agora.ocioso.wrapping_sub(antes.ocioso);
        self.historico.rotate_left(1);
        self.historico[AMOSTRAS - 1] = 1000 - milesimos(dormiu, passou);
        let freq = cpu::frequencia_timer();
        let taxa = |depois: u64, antes: u64| (depois.wrapping_sub(antes) * freq + passou / 2) / passou;
        self.por_segundo =
            [taxa(agora.timer, antes.timer), taxa(agora.serial, antes.serial), taxa(agora.outras, antes.outras)];
    }

    /// Uso da CPU no ultimo segundo, em milesimos.
    fn uso(&self) -> u32 {
        self.historico[AMOSTRAS - 1]
    }
}

pub struct Tarefas {
    abas: Abas,
    lista: Lista,
    linhas: Vec<Linha>,
    /// tempo_cpu de cada janela na ultima amostra: (id, ticks).
    tempos: Vec<(usize, u64)>,
    /// Janelas que mandamos fechar e que o resumo ainda pode mostrar:
    /// (id, cpu::contador do pedido).
    fechando: Vec<(usize, u64)>,
    pedidos: Vec<Pedido>,
    desempenho: Desempenho,
    lista_processos: Lista,
    processos: Vec<LinhaProcesso>,
    /// tempo_cpu de cada processo na ultima amostra: (pid, ticks).
    tempos_processos: Vec<(u32, u64)>,
    /// cpu::contador da ultima amostra.
    ultima: u64,
    /// A primeira amostra sai no primeiro Tique, sem esperar um segundo.
    amostrou: bool,
}

fn area_abas() -> Retangulo {
    Retangulo::novo(MARGEM, MARGEM, LARGURA - 2 * MARGEM, ALTURA_ABAS)
}

fn area_lista() -> Retangulo {
    Retangulo::novo(MARGEM, TOPO, LARGURA - 2 * MARGEM, ALTURA - TOPO - 2 * MARGEM - ALTURA_BOTAO)
}

fn botao_finalizar() -> Retangulo {
    let y = ALTURA - MARGEM - ALTURA_BOTAO;
    Retangulo::novo(LARGURA - MARGEM - LARGURA_FINALIZAR, y, LARGURA_FINALIZAR, ALTURA_BOTAO)
}

/// `parte` de `todo` em milesimos (0 a 1000).
fn milesimos(parte: u64, todo: u64) -> u32 {
    (parte.saturating_mul(1000) / todo.max(1)).min(1000) as u32
}

/// Milesimos -> "12,3%" ("-" sem medida), com a virgula do portugues.
fn texto_cpu(milesimos: Option<u32>) -> String {
    match milesimos {
        Some(m) => format!("{},{}%", m / 10, m % 10),
        None => String::from("-"),
    }
}

/// Bytes em KB ou MB, como no "Sobre".
fn tamanho(bytes: usize) -> String {
    match bytes {
        b if b >= 10 << 20 => format!("{} MB", b >> 20),
        b if b >= 1 << 10 => format!("{} KB", b >> 10),
        b => format!("{b} bytes"),
    }
}

fn item_do_processo(p: &LinhaProcesso) -> ItemLista {
    ItemLista {
        icone: Some(Icone::Chip),
        colunas: vec![format!("{}", p.pid), p.nome.clone(), String::from(p.estado), texto_cpu(p.cpu), tamanho(p.memoria)],
    }
}

fn icone_da_janela(titulo: &str) -> Icone {
    ICONES.iter().find(|(prefixo, _)| titulo.starts_with(*prefixo)).map_or(Icone::Monitor, |&(_, icone)| icone)
}

fn item_da_linha(linha: &Linha) -> ItemLista {
    ItemLista {
        icone: Some(icone_da_janela(&linha.titulo)),
        colunas: vec![linha.titulo.clone(), texto_cpu(linha.cpu), tamanho(linha.memoria)],
    }
}

/// Cor do texto secundario (legendas, nomes das linhas).
fn apagado(t: &Tema) -> Cor {
    grafico::misturar(t.texto, t.fundo_janela, APAGADO)
}

/// "chave      valor": a chave apagada, o valor na cor do texto.
fn par(s: &mut Superficie, x: i32, base: i32, chave: &str, valor: &str, t: &Tema) {
    s.texto_com(x, base, chave, apagado(t), &INTER);
    s.texto_com(x + COLUNA_VALOR, base, valor, t.texto, &INTER);
}

/// Texto encostado a direita em `direita`.
fn texto_a_direita(s: &mut Superficie, direita: i32, base: i32, texto: &str, cor: Cor, f: &Fonte) {
    s.texto_com(direita - f.largura(texto), base, texto, cor, f);
}

impl Tarefas {
    pub fn nova() -> Tarefas {
        let mut lista = Lista::nova();
        lista.definir_colunas(&COLUNAS, &LARGURAS);
        let mut lista_processos = Lista::nova();
        lista_processos.definir_colunas(&COLUNAS_PROCESSOS, &LARGURAS_PROCESSOS);
        let mut app = Tarefas {
            abas: Abas::novas(&NOMES_ABAS),
            lista,
            linhas: Vec::new(),
            tempos: Vec::new(),
            fechando: Vec::new(),
            pedidos: Vec::new(),
            desempenho: Desempenho::novo(),
            lista_processos,
            processos: Vec::new(),
            tempos_processos: Vec::new(),
            ultima: cpu::contador(),
            amostrou: false,
        };
        // as linhas aparecem logo; a CPU de cada uma so na primeira amostra
        app.atualizar_linhas(1);
        app.atualizar_processos(1);
        app
    }

    /// Uma amostra por segundo. true = mediu (precisa redesenhar).
    fn tique(&mut self) -> bool {
        let agora = cpu::contador();
        let passou = agora.wrapping_sub(self.ultima);
        if self.amostrou && passou < cpu::frequencia_timer() {
            return false;
        }
        self.amostrou = true;
        self.ultima = agora;
        self.desempenho.amostrar();
        self.atualizar_linhas(passou.max(1));
        self.atualizar_processos(passou.max(1));
        true
    }

    /// Refaz a aba Processos (a CPU de cada um e a diferenca do tempo_cpu).
    fn atualizar_processos(&mut self, passou: u64) {
        let selecionado = self.pid_selecionado();
        let anteriores = core::mem::take(&mut self.tempos_processos);
        let lista = processo::lista();
        self.processos = lista
            .iter()
            .map(|p| LinhaProcesso {
                pid: p.pid,
                nome: p.nome.clone(),
                estado: p.estado,
                cpu: anteriores
                    .iter()
                    .find(|&&(pid, _)| pid == p.pid)
                    .map(|&(_, antes)| milesimos(p.tempo_cpu.wrapping_sub(antes), passou)),
                memoria: p.memoria,
            })
            .collect();
        self.tempos_processos = lista.iter().map(|p| (p.pid, p.tempo_cpu)).collect();
        self.lista_processos.definir_itens(self.processos.iter().map(item_do_processo).collect());
        let indice = selecionado.and_then(|pid| self.processos.iter().position(|p| p.pid == pid));
        if self.lista_processos.selecionado() != indice {
            self.lista_processos.selecionar(indice);
        }
    }

    fn pid_selecionado(&self) -> Option<u32> {
        self.lista_processos.selecionado().and_then(|i| self.processos.get(i)).map(|p| p.pid)
    }

    /// Mata o processo selecionado; ele some da lista na hora.
    fn finalizar_processo(&mut self) -> bool {
        let Some(pid) = self.pid_selecionado() else { return false };
        processo::matar(pid);
        self.atualizar_processos(1);
        true
    }

    fn evento_processos(&mut self, e: EventoApp) -> bool {
        match e {
            EventoApp::Botao { x, y, botao: Botao::Esquerdo, pressionado: true } if botao_finalizar().contem(x, y) => {
                self.finalizar_processo()
            }
            EventoApp::Tecla { codigo: KEY_DELETE, pressionada: true } => self.finalizar_processo(),
            _ => self.lista_processos.evento(e, area_lista()) != AcaoLista::Nada,
        }
    }

    fn desenhar_processos(&self, s: &mut Superficie, t: &Tema) {
        let area = area_lista();
        self.lista_processos.desenhar(s, area);
        if self.processos.is_empty() {
            let aviso = "Nenhum processo. No terminal: processo conta &";
            s.texto_com(area.x + 12, area.y + 48, aviso, apagado(t), &INTER);
        }
        let botao = botao_finalizar();
        widgets::botao(s, botao, "Finalizar processo", self.lista_processos.selecionado().is_some());
        let soma_cpu: u32 = self.processos.iter().filter_map(|p| p.cpu).sum();
        let memoria: usize = self.processos.iter().map(|p| p.memoria).sum();
        let resumo = format!(
            "{} processo(s)  ·  CPU {}  ·  memória {}",
            self.processos.len(),
            texto_cpu(Some(soma_cpu.min(1000))),
            tamanho(memoria)
        );
        let largura = botao.x - MARGEM - ESPACO;
        let base = INTER.base_centralizada(botao.y, ALTURA_BOTAO);
        s.texto_com(MARGEM, base, fonte::cortar(&resumo, largura, &INTER), apagado(t), &INTER);
    }

    /// Refaz as linhas com o resumo das janelas. `passou` = ticks do
    /// contador desde a amostra anterior.
    fn atualizar_linhas(&mut self, passou: u64) {
        let janelas = janelas::resumo();
        let (agora, prazo) = (cpu::contador(), cpu::frequencia_timer() * SEGUNDOS_FECHANDO);
        self.fechando.retain(|&(id, pedido)| agora.wrapping_sub(pedido) < prazo && janelas.iter().any(|j| j.id == id));
        let selecionada = self.id_selecionado();
        let linhas: Vec<Linha> = janelas
            .iter()
            .filter(|j| !self.fechando.iter().any(|&(id, _)| id == j.id))
            .map(|j| Linha {
                id: j.id,
                titulo: j.titulo.clone(),
                cpu: self.tempo_anterior(j.id).map(|antes| milesimos(j.tempo_cpu.wrapping_sub(antes), passou)),
                memoria: j.memoria,
            })
            .collect();
        self.linhas = linhas;
        self.tempos = janelas.iter().map(|j| (j.id, j.tempo_cpu)).collect();
        self.mostrar_linhas(selecionada);
    }

    fn tempo_anterior(&self, id: usize) -> Option<u64> {
        self.tempos.iter().find(|&&(i, _)| i == id).map(|&(_, tempo)| tempo)
    }

    fn id_selecionado(&self) -> Option<usize> {
        self.lista.selecionado().and_then(|i| self.linhas.get(i)).map(|l| l.id)
    }

    /// Passa as linhas para a lista, com a janela `selecionada` ainda marcada.
    fn mostrar_linhas(&mut self, selecionada: Option<usize>) {
        self.lista.definir_itens(self.linhas.iter().map(item_da_linha).collect());
        let indice = selecionada.and_then(|id| self.linhas.iter().position(|l| l.id == id));
        // selecionar tambem rola ate a linha: so quando ela mudou de lugar,
        // para a atualizacao de cada segundo nao desfazer a rolagem
        if self.lista.selecionado() != indice {
            self.lista.selecionar(indice);
        }
    }

    /// Pede para fechar a janela selecionada; ela some da lista na hora.
    fn finalizar(&mut self) -> bool {
        let Some(id) = self.id_selecionado() else { return false };
        self.pedidos.push(Pedido::Fechar(id));
        self.fechando.push((id, cpu::contador()));
        self.linhas.retain(|l| l.id != id);
        self.mostrar_linhas(None);
        true
    }

    fn evento_aplicativos(&mut self, e: EventoApp) -> bool {
        match e {
            EventoApp::Botao { x, y, botao: Botao::Esquerdo, pressionado: true } if botao_finalizar().contem(x, y) => {
                self.finalizar()
            }
            EventoApp::Tecla { codigo: KEY_DELETE, pressionada: true } => self.finalizar(),
            _ => self.lista.evento(e, area_lista()) != AcaoLista::Nada,
        }
    }

    fn desenhar_aplicativos(&self, s: &mut Superficie, t: &Tema) {
        self.lista.desenhar(s, area_lista());
        let botao = botao_finalizar();
        widgets::botao(s, botao, "Finalizar tarefa", self.lista.selecionado().is_some());
        let soma_cpu: u32 = self.linhas.iter().filter_map(|l| l.cpu).sum();
        let memoria: usize = self.linhas.iter().map(|l| l.memoria).sum();
        let resumo = format!(
            "{} janela(s)  ·  CPU {}  ·  memória {}",
            self.linhas.len(),
            texto_cpu(Some(soma_cpu.min(1000))),
            tamanho(memoria)
        );
        let largura = botao.x - MARGEM - ESPACO;
        let base = INTER.base_centralizada(botao.y, ALTURA_BOTAO);
        s.texto_com(MARGEM, base, fonte::cortar(&resumo, largura, &INTER), apagado(t), &INTER);
    }

    fn desenhar_desempenho(&self, s: &mut Superficie, t: &Tema) {
        let largura = LARGURA - 2 * MARGEM;
        let area_cpu = Retangulo::novo(MARGEM, TOPO, largura, ALTURA_CPU);
        let area_memoria = Retangulo::novo(MARGEM, area_cpu.baixo() + ESPACO, largura, ALTURA_MEMORIA);
        let area_sistema = Retangulo::novo(MARGEM, area_memoria.baixo() + ESPACO, largura, ALTURA_SISTEMA);
        self.painel_cpu(s, area_cpu, t);
        painel_memoria(s, area_memoria, t);
        self.painel_sistema(s, area_sistema, t);
    }

    /// O uso agora em letra grande e o grafico dos ultimos 60 s.
    fn painel_cpu(&self, s: &mut Superficie, r: Retangulo, t: &Tema) {
        widgets::painel(s, r);
        let base = r.y + PRIMEIRA_BASE + 6;
        s.texto_com(r.x + DENTRO, base, "CPU", t.texto, &INTER_TITULO);
        let uso = format!("{}%", self.desempenho.uso() / 10);
        texto_a_direita(s, r.direita() - DENTRO, base, &uso, t.texto, &INTER_TITULO);
        let legenda = format!("{}, uso nos últimos {AMOSTRAS} segundos", cpu::nome_cpu(cpu::midr()));
        s.texto_com(r.x + DENTRO, base + 20, &legenda, apagado(t), &INTER);
        let topo = base + 30 - r.y;
        let caixa = Retangulo::novo(r.x + DENTRO, r.y + topo, r.largura - 2 * DENTRO, r.altura - topo - 28);
        widgets::grafico_linhas(s, caixa, &self.desempenho.historico, 1000, t.destaque);
        let base = caixa.baixo() + 18;
        s.texto_com(caixa.x, base, "60 s", apagado(t), &INTER);
        texto_a_direita(s, caixa.direita(), base, "agora", apagado(t), &INTER);
    }

    fn painel_sistema(&self, s: &mut Superficie, r: Retangulo, t: &Tema) {
        widgets::painel(s, r);
        let [timer, serial, outras] = self.desempenho.por_segundo;
        let total = timer + serial + outras;
        let segundos = cpu::contador() / cpu::frequencia_timer().max(1);
        let c = irq::contadores();
        let ocioso = milesimos(c.ocioso, c.desde_inicio);
        let linhas = [
            ("Interrupções", format!("{total} por segundo (timer {timer}, serial {serial}, outras {outras})")),
            ("Ligado há", format!("{}h {:02}m {:02}s", segundos / 3600, segundos / 60 % 60, segundos % 60)),
            ("Janelas", format!("{} abertas, {} processo(s)", self.linhas.len(), self.processos.len())),
            ("CPU ociosa", format!("{},{}% do tempo desde o boot", ocioso / 10, ocioso % 10)),
        ];
        let largura = r.largura - 2 * DENTRO - COLUNA_VALOR;
        for (i, (chave, valor)) in linhas.iter().enumerate() {
            let base = r.y + PRIMEIRA_BASE + i as i32 * PASSO_LINHA;
            par(s, r.x + DENTRO, base, chave, fonte::cortar(valor, largura, &INTER), t);
        }
    }
}

/// Barra do heap: usado, total e o maior bloco livre.
fn painel_memoria(s: &mut Superficie, r: Retangulo, t: &Tema) {
    widgets::painel(s, r);
    let h = heap::estatisticas();
    let usado = h.total - h.livre;
    let uso = milesimos(usado as u64, h.total as u64);
    let base = r.y + PRIMEIRA_BASE;
    s.texto_com(r.x + DENTRO, base, "Memória (heap)", t.texto, &INTER_NEGRITO);
    let total = format!("{} de {} ({}%)", tamanho(usado), tamanho(h.total), uso / 10);
    texto_a_direita(s, r.direita() - DENTRO, base, &total, t.texto, &INTER);
    widgets::barra_progresso(s, Retangulo::novo(r.x + DENTRO, base + 8, r.largura - 2 * DENTRO, 12), uso);
    let detalhe = format!("{} alocações, maior bloco livre: {}", h.alocacoes, tamanho(h.maior_bloco));
    s.texto_com(r.x + DENTRO, base + 38, &detalhe, apagado(t), &INTER);
}

impl App for Tarefas {
    fn titulo(&self) -> &str {
        TITULO
    }

    fn tamanho(&self) -> (usize, usize) {
        (LARGURA as usize, ALTURA as usize)
    }

    fn desenhar(&mut self, s: &mut Superficie) {
        let t = tema::atual();
        s.limpar(t.fundo_janela);
        self.abas.desenhar(s, area_abas());
        match self.abas.atual() {
            ABA_PROCESSOS => self.desenhar_processos(s, &t),
            ABA_DESEMPENHO => self.desenhar_desempenho(s, &t),
            _ => self.desenhar_aplicativos(s, &t),
        }
    }

    fn evento(&mut self, e: EventoApp) -> bool {
        if e == EventoApp::Tique {
            return self.tique();
        }
        if self.abas.evento(e, area_abas()) {
            return true;
        }
        match self.abas.atual() {
            ABA_APLICATIVOS => self.evento_aplicativos(e),
            ABA_PROCESSOS => self.evento_processos(e),
            _ => false,
        }
    }

    fn pedidos(&mut self) -> Vec<Pedido> {
        core::mem::take(&mut self.pedidos)
    }
}
