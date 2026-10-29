//! Editor de texto: varias linhas de UTF-8, cursor, rolagem e arquivos do disco.
//!
//! O texto mora num Vec<String>, uma String por linha (sem o '\n'). O
//! cursor e as colunas contam caracteres (nao bytes): a area do texto usa a
//! JetBrains Mono, 8 px por caractere, e a conta do cursor fica simples.
//! Ao abrir, "\r\n" vira "\n" (e volta a ser "\r\n" ao salvar), Tab vira
//! espacos ate a proxima coluna multipla de 4, bytes que nao formam UTF-8
//! valido sao lidos como Latin-1 (com um aviso na barra de estado) e
//! caracteres de controle viram '?'. Salvar grava sempre UTF-8.
//!
//! Teclado: setas, Home/End (com Ctrl: comeco/fim do texto), PgUp/PgDn,
//! Delete, Backspace, Enter, Tab (ate 4 espacos) e Ctrl+S salva. O clique
//! poe o cursor; a roda e a barra de rolagem rolam. Linha comprida rola
//! para o lado acompanhando o cursor (nao quebra).
//!
//! Na barra de cima: Novo, Abrir, Salvar e Salvar como. Abrir e Salvar
//! como trocam os botoes por um campo com o caminho (Enter confirma, Esc
//! desiste). Novo e Abrir perguntam antes de jogar fora alteracoes. Abrir
//! um arquivo que nao existe comeca um documento novo com aquele caminho.
//! Ler e gravar o disco rodam no Tique seguinte, depois de um quadro com
//! "Carregando ..." ou "Salvando ..." (a area de trabalho para enquanto isso).
//!
//! Realce de sintaxe pela extensao (gui::realce): C, JS, Rust, Python,
//! shell, CSS e HTML, com o estado de cada linha guardado e recalculado
//! so a partir da linha alterada. Os botoes "Explicar" e "Refatorar"
//! mandam o arquivo para o Claude (crate::ia) e abrem a resposta numa
//! janela nova do editor (a refatoracao como documento sem titulo, para
//! conferir e salvar se quiser).

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use super::arquivos::{
    ALTURA_BARRA, ALTURA_ESTADO, BotaoBarra, ESPACO, MARGEM, acao_em, barra_estado, botoes_dialogo, contagem, cor_barra,
    desenhar_aviso, desenhar_botao, evento_campo, largura_botao, nome_do_caminho, normalizar, pai_do_caminho,
    pintar_barra, tamanho_texto,
};
use crate::armazenamento::{self, Erro};
use crate::entrada::Botao;
use crate::entrada::teclado::{
    KEY_DELETE, KEY_DOWN, KEY_END, KEY_HOME, KEY_LEFT, KEY_LEFTCTRL, KEY_PAGEDOWN, KEY_PAGEUP, KEY_RIGHT, KEY_RIGHTCTRL,
    KEY_UP,
};
use crate::fonte::{self, INTER, MONO, MONO_LARGURA};
use crate::gui::grafico::{self, Retangulo, Superficie};
use crate::gui::janelas::{App, EventoApp, Pedido};
use crate::gui::realce::{self, Linguagem};
use crate::gui::tema::{self, Tema};
use crate::gui::widgets::{ALTURA_BOTAO, AcaoCampo, CampoTexto, cor_campo, cor_linha, cor_texto_fraco};

const LARGURA: i32 = 720;
const ALTURA: i32 = 480;
/// Altura de uma linha de texto: a celula de 16 px da Mono com folga para
/// os acentos das maiusculas em cima e o 'g' e o 'c' cedilha embaixo.
const ALTURA_LINHA: i32 = 18;
/// Linha de base dentro de uma linha de texto.
const BASE_LINHA: i32 = 14;
const LARGURA_CARACTERE: i32 = MONO_LARGURA;
/// Folga entre a barra de cima e a primeira linha.
const PREENCHIMENTO: i32 = 4;
const LARGURA_ROLAGEM: i32 = 12;
const ESPACOS_TAB: usize = 4;
/// Colunas que a vista pula quando o cursor sai pelo lado.
const SALTO: usize = 8;
/// Maior arquivo que o editor abre.
const MAX_BYTES: usize = 512 * 1024;
/// Codigo evdev da tecla S (Ctrl+S).
const TECLA_S: u16 = 31;

const AREA_TEXTO: Retangulo = Retangulo::novo(0, ALTURA_BARRA, LARGURA - LARGURA_ROLAGEM, ALTURA - ALTURA_BARRA - ALTURA_ESTADO);
const AREA_ROLAGEM: Retangulo =
    Retangulo::novo(LARGURA - LARGURA_ROLAGEM, ALTURA_BARRA, LARGURA_ROLAGEM, ALTURA - ALTURA_BARRA - ALTURA_ESTADO);

/// Editor vazio (menu iniciar).
pub fn criar() -> Box<dyn App> {
    Box::new(Editor::novo())
}

/// O que a barra de cima mostra.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Modo {
    Normal,
    /// O campo pede o caminho do arquivo para abrir.
    Abrir,
    /// O campo pede o caminho onde salvar.
    SalvarComo,
    /// Ha alteracoes nao salvas: confirma antes de seguir.
    Descartar(Depois),
}

/// O que fazer depois de confirmar o descarte.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Depois {
    Novo,
    Abrir,
}

/// Trabalho com o disco, feito no Tique depois do quadro com o aviso.
enum Tarefa {
    Carregar(String),
    Salvar(String),
    /// Pergunta ao Claude (true = refatorar, false = explicar).
    Ia(bool),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Acao {
    Novo,
    Abrir,
    Salvar,
    SalvarComo,
    Explicar,
    Refatorar,
    Confirmar,
    Cancelar,
}

const BOTOES: [(Acao, &str); 6] = [
    (Acao::Novo, "Novo"),
    (Acao::Abrir, "Abrir"),
    (Acao::Salvar, "Salvar"),
    (Acao::SalvarComo, "Salvar como"),
    (Acao::Explicar, "Explicar (IA)"),
    (Acao::Refatorar, "Refatorar (IA)"),
];

pub struct Editor {
    linhas: Vec<String>,
    /// Cursor: linha e coluna (em caracteres, nao em bytes).
    linha: usize,
    coluna: usize,
    /// Coluna que o cursor tenta manter ao subir e descer.
    coluna_alvo: usize,
    /// Primeira linha e primeira coluna que aparecem.
    topo: usize,
    esquerda: usize,
    caminho: Option<String>,
    modificado: bool,
    /// O arquivo usava "\r\n" (salvar mantem).
    crlf: bool,
    titulo: String,
    modo: Modo,
    campo: CampoTexto,
    /// Botao apertado com o mouse (age ao soltar em cima dele).
    apertado: Option<Acao>,
    /// Mensagem da barra de estado: (texto, e um erro).
    mensagem: Option<(String, bool)>,
    tarefa: Option<Tarefa>,
    /// O quadro com o aviso da tarefa ja foi desenhado.
    quadro_mostrado: bool,
    ctrl: bool,
    /// Arrastando a barra de rolagem: distancia do clique ao topo do cursor dela.
    arrasto: Option<i32>,
    linguagem: Linguagem,
    /// Estado do realce no comeco de cada linha; validos ate `estados_ok`.
    estados: Vec<realce::Estado>,
    estados_ok: usize,
    pedidos: Vec<Pedido>,
}

impl Editor {
    /// Documento novo, sem arquivo.
    pub fn novo() -> Editor {
        let mut editor = Editor {
            linhas: vec![String::new()],
            linha: 0,
            coluna: 0,
            coluna_alvo: 0,
            topo: 0,
            esquerda: 0,
            caminho: None,
            modificado: false,
            crlf: false,
            titulo: String::new(),
            modo: Modo::Normal,
            campo: CampoTexto::novo(""),
            apertado: None,
            mensagem: None,
            tarefa: None,
            quadro_mostrado: false,
            ctrl: false,
            arrasto: None,
            linguagem: Linguagem::Nenhuma,
            estados: vec![realce::NORMAL],
            estados_ok: 1,
            pedidos: Vec::new(),
        };
        editor.atualizar_titulo();
        editor
    }

    /// Janela com um texto pronto (resposta da IA), sem arquivo.
    pub fn com_texto(titulo: &str, texto: &str, linguagem: Linguagem) -> Editor {
        let mut editor = Editor::novo();
        let (linhas, _, _) = linhas_do_arquivo(texto.as_bytes());
        editor.linhas = linhas;
        editor.linguagem = linguagem;
        editor.titulo = String::from(titulo);
        editor
    }

    /// Editor que abre `caminho` (o disco e lido no primeiro Tique).
    pub fn abrir(caminho: &str) -> Editor {
        let mut editor = Editor::novo();
        let caminho = normalizar(caminho);
        editor.titulo = format!("Editor — {}", nome_do_caminho(&caminho));
        editor.agendar(Tarefa::Carregar(caminho));
        editor
    }

    fn atualizar_titulo(&mut self) {
        let nome = self.caminho.as_deref().map_or("sem título", nome_do_caminho);
        let marca = if self.modificado { "*" } else { "" };
        self.titulo = format!("Editor — {nome}{marca}");
    }

    fn info(&mut self, texto: String) {
        self.mensagem = Some((texto, false));
    }

    fn erro(&mut self, texto: String) {
        self.mensagem = Some((texto, true));
    }

    // ---------------------------------------------------------- texto

    /// Tamanho da linha `i` em caracteres.
    fn colunas_da_linha(&self, i: usize) -> usize {
        self.linhas[i].chars().count()
    }

    /// Byte da linha do cursor onde comeca o caractere `coluna`.
    fn byte_da_coluna(&self, coluna: usize) -> usize {
        byte_de(&self.linhas[self.linha], coluna)
    }

    fn inserir(&mut self, texto: &str) {
        let byte = self.byte_da_coluna(self.coluna);
        self.linhas[self.linha].insert_str(byte, texto);
        self.coluna += texto.chars().count();
        self.alterou();
    }

    fn nova_linha(&mut self) {
        let byte = self.byte_da_coluna(self.coluna);
        let resto = self.linhas[self.linha].split_off(byte);
        self.linha += 1;
        self.linhas.insert(self.linha, resto);
        self.coluna = 0;
        self.alterou();
    }

    /// Backspace: apaga antes do cursor; no comeco da linha junta com a de cima.
    fn apagar_antes(&mut self) -> bool {
        if self.coluna > 0 {
            self.coluna -= 1;
            let byte = self.byte_da_coluna(self.coluna);
            self.linhas[self.linha].remove(byte);
        } else if self.linha > 0 {
            let atual = self.linhas.remove(self.linha);
            self.linha -= 1;
            self.coluna = self.colunas_da_linha(self.linha);
            self.linhas[self.linha].push_str(&atual);
        } else {
            return false;
        }
        self.alterou();
        true
    }

    /// Delete: apaga no cursor; no fim da linha junta com a de baixo.
    fn apagar_depois(&mut self) -> bool {
        if self.coluna < self.colunas_da_linha(self.linha) {
            let byte = self.byte_da_coluna(self.coluna);
            self.linhas[self.linha].remove(byte);
        } else if self.linha + 1 < self.linhas.len() {
            let de_baixo = self.linhas.remove(self.linha + 1);
            self.linhas[self.linha].push_str(&de_baixo);
        } else {
            return false;
        }
        self.alterou();
        true
    }

    /// Tab: espacos ate a proxima coluna multipla de 4.
    fn tabulacao(&mut self) {
        let n = ESPACOS_TAB - self.coluna % ESPACOS_TAB;
        self.inserir(&"    "[..n]);
    }

    /// Depois de cada mudanca no texto: o titulo ganha o '*'.
    fn alterou(&mut self) {
        self.estados_ok = self.estados_ok.min(self.linha.saturating_sub(1) + 1);
        self.coluna_alvo = self.coluna;
        self.mensagem = None;
        if !self.modificado {
            self.modificado = true;
            self.atualizar_titulo();
        }
    }

    /// Texto inteiro para gravar, com a quebra de linha que o arquivo tinha.
    fn conteudo(&self) -> Vec<u8> {
        let quebra: &[u8] = if self.crlf { b"\r\n" } else { b"\n" };
        let tamanho = self.linhas.iter().map(|l| l.len() + quebra.len()).sum();
        let mut dados = Vec::with_capacity(tamanho);
        for (i, linha) in self.linhas.iter().enumerate() {
            if i > 0 {
                dados.extend_from_slice(quebra);
            }
            dados.extend_from_slice(linha.as_bytes());
        }
        dados
    }

    /// Documento novo e vazio.
    fn limpar(&mut self) {
        self.estados_ok = 1;
        self.linhas = vec![String::new()];
        self.colocar_cursor(0, 0);
        self.topo = 0;
        self.esquerda = 0;
        self.caminho = None;
        self.modificado = false;
        self.crlf = false;
        self.atualizar_titulo();
    }

    // --------------------------------------------------------- cursor

    fn colocar_cursor(&mut self, linha: usize, coluna: usize) {
        self.linha = linha;
        self.coluna = coluna;
        self.coluna_alvo = coluna;
    }

    fn mover_esquerda(&mut self) {
        if self.coluna > 0 {
            self.coluna -= 1;
        } else if self.linha > 0 {
            self.linha -= 1;
            self.coluna = self.colunas_da_linha(self.linha);
        }
        self.coluna_alvo = self.coluna;
    }

    fn mover_direita(&mut self) {
        if self.coluna < self.colunas_da_linha(self.linha) {
            self.coluna += 1;
        } else if self.linha + 1 < self.linhas.len() {
            self.linha += 1;
            self.coluna = 0;
        }
        self.coluna_alvo = self.coluna;
    }

    /// Sobe (negativo) ou desce linhas tentando manter a coluna.
    fn mover_linhas(&mut self, quantas: isize) {
        let ultima = self.linhas.len() as isize - 1;
        self.linha = (self.linha as isize + quantas).clamp(0, ultima) as usize;
        self.coluna = self.coluna_alvo.min(self.colunas_da_linha(self.linha));
    }

    /// Clique no texto: cursor no caractere mais perto.
    fn posicionar(&mut self, x: i32, y: i32) {
        let fila = ((y - AREA_TEXTO.y - PREENCHIMENTO).max(0) / ALTURA_LINHA) as usize;
        self.linha = (self.topo + fila).min(self.linhas.len() - 1);
        let coluna = ((x - self.x_texto() + LARGURA_CARACTERE / 2).max(0) / LARGURA_CARACTERE) as usize;
        self.coluna = (self.esquerda + coluna).min(self.colunas_da_linha(self.linha));
        self.coluna_alvo = self.coluna;
        self.mostrar_cursor();
    }

    // ---------------------------------------------------------- vista

    fn linhas_visiveis(&self) -> usize {
        ((AREA_TEXTO.altura - 2 * PREENCHIMENTO) / ALTURA_LINHA).max(1) as usize
    }

    /// Faixa dos numeros de linha (cresce com o numero de digitos).
    fn largura_numeros(&self) -> i32 {
        digitos(self.linhas.len()).max(3) as i32 * LARGURA_CARACTERE + 16
    }

    fn x_texto(&self) -> i32 {
        self.largura_numeros() + 6
    }

    fn colunas_visiveis(&self) -> usize {
        ((AREA_TEXTO.direita() - 4 - self.x_texto()) / LARGURA_CARACTERE).max(1) as usize
    }

    fn maior_topo(&self) -> usize {
        self.linhas.len().saturating_sub(self.linhas_visiveis())
    }

    /// Rola o que precisar para o cursor aparecer.
    fn mostrar_cursor(&mut self) {
        let visiveis = self.linhas_visiveis();
        if self.linha < self.topo {
            self.topo = self.linha;
        } else if self.linha >= self.topo + visiveis {
            self.topo = self.linha + 1 - visiveis;
        }
        let colunas = self.colunas_visiveis();
        if self.coluna < self.esquerda {
            self.esquerda = self.coluna.saturating_sub(SALTO);
        } else if self.coluna >= self.esquerda + colunas {
            self.esquerda = (self.coluna + SALTO + 1).saturating_sub(colunas);
        }
    }

    /// Rola `linhas` (negativo = para cima). true = mudou.
    fn rolar(&mut self, linhas: isize) -> bool {
        let novo = (self.topo as isize + linhas).clamp(0, self.maior_topo() as isize) as usize;
        let mudou = novo != self.topo;
        self.topo = novo;
        mudou
    }

    /// O cursor da barra de rolagem (None = o texto cabe inteiro).
    fn polegar(&self) -> Option<Retangulo> {
        let (total, visiveis) = (self.linhas.len(), self.linhas_visiveis());
        if total <= visiveis {
            return None;
        }
        let r = AREA_ROLAGEM;
        let altura = ((r.altura as usize * visiveis / total) as i32).clamp(24, r.altura);
        let livre = (r.altura - altura) as usize;
        let y = r.y + (livre * self.topo.min(self.maior_topo()) / self.maior_topo()) as i32;
        Some(Retangulo::novo(r.x + 3, y, r.largura - 5, altura))
    }

    /// Clique na barra de rolagem: acima ou abaixo do cursor dela pula uma
    /// pagina; em cima dele comeca a arrastar.
    fn clicar_rolagem(&mut self, y: i32) -> bool {
        let Some(p) = self.polegar() else { return false };
        let pagina = self.linhas_visiveis() as isize;
        if y < p.y {
            self.rolar(-pagina)
        } else if y >= p.baixo() {
            self.rolar(pagina)
        } else {
            self.arrasto = Some(y - p.y);
            true
        }
    }

    fn arrastar(&mut self, y: i32) -> bool {
        let (Some(dy), Some(p)) = (self.arrasto, self.polegar()) else { return false };
        let livre = (AREA_ROLAGEM.altura - p.altura).max(1);
        let posicao = (y - dy - AREA_ROLAGEM.y).clamp(0, livre) as usize;
        let topo = (posicao * self.maior_topo() + livre as usize / 2) / livre as usize;
        let mudou = topo != self.topo;
        self.topo = topo;
        mudou
    }

    // ------------------------------------------------ barra e comandos

    fn com_campo(&self) -> bool {
        matches!(self.modo, Modo::Abrir | Modo::SalvarComo)
    }

    /// Botoes da barra de cima, como estao agora.
    fn botoes(&self) -> Vec<BotaoBarra<Acao>> {
        let texto = match self.modo {
            Modo::Normal => {
                let mut x = MARGEM;
                let mut botoes = Vec::with_capacity(BOTOES.len());
                for &(acao, texto) in &BOTOES {
                    let r = Retangulo::novo(x, MARGEM, largura_botao(texto), ALTURA_BOTAO);
                    x = r.direita() + ESPACO;
                    botoes.push(BotaoBarra { acao, r, icone: None, texto });
                }
                return botoes;
            }
            Modo::Abrir => "Abrir",
            Modo::SalvarComo => "Salvar",
            Modo::Descartar(_) => "Descartar",
        };
        Vec::from(botoes_dialogo(Acao::Confirmar, texto, Acao::Cancelar, MARGEM, LARGURA))
    }

    fn rotulo_campo(&self) -> &'static str {
        if self.modo == Modo::SalvarComo { "Salvar como:" } else { "Abrir:" }
    }

    /// Campo do caminho: depois do rotulo, ate os botoes da direita.
    fn area_campo(&self) -> Retangulo {
        let x = MARGEM + 4 + Superficie::largura_texto(self.rotulo_campo(), 1) + 8;
        let fim = self.botoes().first().map_or(LARGURA, |b| b.r.x) - 10;
        Retangulo::novo(x, MARGEM, fim - x, ALTURA_BOTAO)
    }

    fn executar(&mut self, acao: Acao) {
        match acao {
            Acao::Novo => self.pedir(Depois::Novo),
            Acao::Abrir => self.pedir(Depois::Abrir),
            Acao::Salvar => match self.caminho.clone() {
                Some(caminho) => self.agendar(Tarefa::Salvar(caminho)),
                None => self.pedir_caminho(Modo::SalvarComo),
            },
            Acao::SalvarComo => self.pedir_caminho(Modo::SalvarComo),
            Acao::Explicar => self.agendar(Tarefa::Ia(false)),
            Acao::Refatorar => self.agendar(Tarefa::Ia(true)),
            Acao::Confirmar => self.confirmar(),
            Acao::Cancelar => self.cancelar(),
        }
    }

    /// Novo e Abrir: com alteracoes nao salvas, pergunta antes.
    fn pedir(&mut self, depois: Depois) {
        if self.modificado {
            self.modo = Modo::Descartar(depois);
            self.mensagem = None;
        } else {
            self.seguir(depois);
        }
    }

    fn seguir(&mut self, depois: Depois) {
        match depois {
            Depois::Novo => {
                self.limpar();
                self.info(String::from("Documento novo"));
            }
            Depois::Abrir => self.pedir_caminho(Modo::Abrir),
        }
    }

    fn confirmar(&mut self) {
        match self.modo {
            Modo::Descartar(depois) => {
                self.modo = Modo::Normal;
                self.seguir(depois);
            }
            Modo::Abrir | Modo::SalvarComo => self.confirmar_caminho(),
            Modo::Normal => {}
        }
    }

    fn cancelar(&mut self) {
        self.modo = Modo::Normal;
        self.campo.focar(false);
    }

    /// Troca os botoes pelo campo do caminho, ja com uma sugestao.
    fn pedir_caminho(&mut self, modo: Modo) {
        let sugestao = match (&self.caminho, modo) {
            (Some(caminho), Modo::SalvarComo) => caminho.clone(),
            (Some(caminho), _) => com_barra(&pai_do_caminho(caminho)),
            (None, Modo::SalvarComo) => format!("{}novo.txt", pasta_padrao()),
            (None, _) => pasta_padrao(),
        };
        self.modo = modo;
        self.mensagem = None;
        self.campo.definir(&sugestao);
        self.campo.focar(true);
    }

    /// Enter ou o botao do campo: abre ou salva no caminho digitado.
    fn confirmar_caminho(&mut self) {
        let texto = self.campo.texto().trim();
        if texto.is_empty() || texto.ends_with(['/', '\\']) {
            self.erro(String::from("Digite o caminho, por exemplo /documentos/notas.txt"));
            return;
        }
        let caminho = normalizar(texto);
        let tarefa = if self.modo == Modo::Abrir { Tarefa::Carregar(caminho) } else { Tarefa::Salvar(caminho) };
        self.cancelar();
        self.agendar(tarefa);
    }

    /// Guarda a tarefa; ela roda no primeiro Tique depois do quadro com o aviso.
    fn agendar(&mut self, tarefa: Tarefa) {
        self.tarefa = Some(tarefa);
        self.quadro_mostrado = false;
        self.mensagem = None;
    }

    fn tique(&mut self) -> bool {
        if !self.quadro_mostrado {
            return false;
        }
        match self.tarefa.take() {
            Some(Tarefa::Carregar(caminho)) => self.carregar(caminho),
            Some(Tarefa::Salvar(caminho)) => self.salvar(caminho),
            Some(Tarefa::Ia(refatorar)) => self.perguntar_ia(refatorar),
            None => return false,
        }
        true
    }

    // As mensagens usam so o nome do arquivo (o caminho inteiro fica na
    // barra de cima) para caberem na barra de estado.

    fn carregar(&mut self, caminho: String) {
        let nome = String::from(nome_do_caminho(&caminho));
        match armazenamento::ler(&caminho) {
            Ok(dados) if dados.len() > MAX_BYTES => {
                self.atualizar_titulo();
                let (tamanho, maximo) = (tamanho_texto(dados.len() as u64), tamanho_texto(MAX_BYTES as u64));
                self.erro(format!("{nome} é grande demais ({tamanho}; máximo {maximo})"));
            }
            Ok(dados) => {
                let (linhas, crlf, trocados) = linhas_do_arquivo(&dados);
                self.linhas = linhas;
                self.linguagem = realce::linguagem_do_caminho(&caminho);
                self.estados_ok = 1;
                self.crlf = crlf;
                self.colocar_cursor(0, 0);
                self.topo = 0;
                self.esquerda = 0;
                self.modificado = false;
                if trocados > 0 {
                    self.erro(format!("Aviso: {trocados} byte(s) fora do UTF-8 lidos como Latin-1"));
                } else {
                    self.info(format!("Aberto: {nome} ({})", contagem(self.linhas.len(), "linha", "linhas")));
                }
                self.caminho = Some(caminho);
                self.atualizar_titulo();
            }
            Err(Erro::NaoAchado) => {
                self.limpar();
                self.info(format!("{nome} é novo: será criado ao salvar"));
                self.caminho = Some(caminho);
                self.atualizar_titulo();
            }
            Err(e) => {
                self.atualizar_titulo();
                self.erro(format!("Não foi possível abrir {nome}: {}", e.descricao()));
            }
        }
    }

    /// Manda o documento para o Claude e abre a resposta numa janela nova.
    fn perguntar_ia(&mut self, refatorar: bool) {
        let codigo = String::from_utf8_lossy(&self.conteudo()).into_owned();
        if codigo.trim().is_empty() {
            self.erro(String::from("O documento está vazio"));
            return;
        }
        let nome = self.caminho.as_deref().map_or("sem título", nome_do_caminho);
        let lingua = realce::nome(self.linguagem);
        let sistema = "Você é um assistente de programação dentro do editor do MXOS. Responda em português do Brasil,                        em texto simples (sem Markdown), com linhas de no máximo 100 colunas.";
        let pergunta = if refatorar {
            format!(
                "Refatore este arquivo ({lingua}) para ficar mais claro e eficiente, sem mudar o comportamento.                  Responda somente com o arquivo completo refatorado, sem explicações e sem cercas de código.

                 Arquivo: {nome}

{codigo}"
            )
        } else {
            format!(
                "Explique o que este arquivo ({lingua}) faz, parte por parte, e aponte possíveis bugs ou melhorias.                  O cursor está na linha {}.

Arquivo: {nome}

{codigo}",
                self.linha + 1
            )
        };
        match crate::ia::perguntar(sistema, &pergunta) {
            Ok(resposta) => {
                let (titulo, texto, lingua) = if refatorar {
                    (format!("Refatorado — {nome}"), sem_cercas(&resposta), self.linguagem)
                } else {
                    (format!("Explicação — {nome}"), resposta, Linguagem::Nenhuma)
                };
                let app = Editor::com_texto(&titulo, &texto, lingua);
                self.pedidos.push(Pedido::Abrir { app: Box::new(app), x: -1, y: -1 });
                self.info(String::from("Resposta da IA aberta numa janela nova"));
            }
            Err(e) => self.erro(e),
        }
    }

    fn salvar(&mut self, caminho: String) {
        let dados = self.conteudo();
        match armazenamento::escrever(&caminho, &dados) {
            Ok(()) => {
                self.info(format!("Salvo: {} ({})", nome_do_caminho(&caminho), tamanho_texto(dados.len() as u64)));
                self.linguagem = realce::linguagem_do_caminho(&caminho);
                self.estados_ok = 1;
                self.caminho = Some(caminho);
                self.modificado = false;
                self.atualizar_titulo();
            }
            // escrever cria o arquivo: so falta achar a pasta dele
            Err(Erro::NaoAchado) => self.erro(format!("Não salvou: a pasta {} não existe", pai_do_caminho(&caminho))),
            Err(e) => self.erro(format!("Não salvou: {}", e.descricao())),
        }
    }

    // ------------------------------------------------------- eventos

    fn apertar(&mut self, x: i32, y: i32, e: EventoApp) -> bool {
        let mut redesenhar = false;
        if self.modo != Modo::Normal {
            if y >= ALTURA_BARRA {
                // clique fora da barra desiste do dialogo
                self.cancelar();
                redesenhar = true;
            } else if self.com_campo() {
                let area = self.area_campo();
                let _ = self.campo.evento(e, area);
                self.campo.focar(true);
                redesenhar = true;
            }
        }
        if let Some(acao) = acao_em(&self.botoes(), x, y) {
            self.apertado = Some(acao);
            return true;
        }
        if AREA_ROLAGEM.contem(x, y) {
            return self.clicar_rolagem(y) || redesenhar;
        }
        if AREA_TEXTO.contem(x, y) {
            self.posicionar(x, y);
            return true;
        }
        redesenhar
    }

    fn soltar(&mut self, x: i32, y: i32) -> bool {
        let arrastava = self.arrasto.take().is_some();
        let Some(acao) = self.apertado.take() else { return arrastava };
        if acao_em(&self.botoes(), x, y) == Some(acao) {
            self.executar(acao);
        }
        true
    }

    fn caractere(&mut self, c: char, e: EventoApp) -> bool {
        self.ctrl = false; // com Ctrl apertado nao chega caractere
        match self.modo {
            Modo::Abrir | Modo::SalvarComo => return self.evento_do_campo(e),
            Modo::Descartar(_) => {
                match c {
                    '\r' => self.confirmar(),
                    '\u{1b}' => self.cancelar(),
                    _ => return false,
                }
                return true;
            }
            Modo::Normal => {}
        }
        match c {
            '\r' => self.nova_linha(),
            '\u{8}' => {
                if !self.apagar_antes() {
                    return false;
                }
            }
            '\t' => self.tabulacao(),
            c if !c.is_control() => self.inserir(c.encode_utf8(&mut [0; 4])),
            _ => return false,
        }
        self.mostrar_cursor();
        true
    }

    fn tecla(&mut self, codigo: u16, e: EventoApp) -> bool {
        match self.modo {
            Modo::Abrir | Modo::SalvarComo => return self.evento_do_campo(e),
            Modo::Descartar(_) => return false,
            Modo::Normal => {}
        }
        let pagina = self.linhas_visiveis() as isize;
        let ultima = self.linhas.len() - 1;
        match codigo {
            TECLA_S if self.ctrl => {
                self.executar(Acao::Salvar);
                return true;
            }
            // sem selecao: Ctrl+C copia a linha do cursor; Ctrl+V cola
            crate::gui::transferencia::TECLA_C if self.ctrl => {
                crate::gui::transferencia::copiar(&self.linhas[self.linha]);
                return false;
            }
            crate::gui::transferencia::TECLA_V if self.ctrl => {
                let texto = crate::gui::transferencia::texto();
                for (k, parte) in texto.split('\n').enumerate() {
                    if k > 0 {
                        self.nova_linha();
                    }
                    let limpo: String = parte.chars().filter(|c| !c.is_control() || *c == '\t').collect();
                    if !limpo.is_empty() {
                        self.inserir(&limpo);
                    }
                }
                return true;
            }
            KEY_HOME if self.ctrl => self.colocar_cursor(0, 0),
            KEY_END if self.ctrl => self.colocar_cursor(ultima, self.colunas_da_linha(ultima)),
            KEY_LEFT => self.mover_esquerda(),
            KEY_RIGHT => self.mover_direita(),
            KEY_UP => self.mover_linhas(-1),
            KEY_DOWN => self.mover_linhas(1),
            KEY_HOME => self.colocar_cursor(self.linha, 0),
            KEY_END => self.colocar_cursor(self.linha, self.colunas_da_linha(self.linha)),
            KEY_PAGEUP => {
                self.rolar(-pagina);
                self.mover_linhas(-pagina);
            }
            KEY_PAGEDOWN => {
                self.rolar(pagina);
                self.mover_linhas(pagina);
            }
            KEY_DELETE => {
                if !self.apagar_depois() {
                    return false;
                }
            }
            _ => return false,
        }
        self.mostrar_cursor();
        true
    }

    fn evento_do_campo(&mut self, e: EventoApp) -> bool {
        let area = self.area_campo();
        let (acao, redesenhar) = evento_campo(&mut self.campo, e, area);
        match acao {
            AcaoCampo::Confirmou => self.confirmar(),
            AcaoCampo::Cancelou => self.cancelar(),
            _ => return redesenhar,
        }
        true
    }

    // -------------------------------------------------------- desenho

    fn desenhar_barra(&self, s: &mut Superficie, t: &Tema) {
        let fundo = match self.modo {
            Modo::Normal => cor_barra(t),
            Modo::Descartar(_) => grafico::misturar(t.fundo_janela, t.fechar, 36),
            _ => grafico::misturar(t.fundo_janela, t.destaque, 28),
        };
        pintar_barra(s, Retangulo::novo(0, 0, LARGURA, ALTURA_BARRA), fundo, t);
        let botoes = self.botoes();
        for b in &botoes {
            desenhar_botao(s, b, b.acao == Acao::Confirmar, self.apertado == Some(b.acao), true, t);
        }
        let y = (ALTURA_BARRA - 8) / 2;
        match self.modo {
            Modo::Normal => {
                // o caminho do arquivo, encostado na direita
                let inicio = botoes.last().map_or(MARGEM, |b| b.r.direita()) + 16;
                let caminho = self.caminho.as_deref().unwrap_or("(ainda não salvo)");
                let texto = cortar_inicio(caminho, LARGURA - MARGEM - 4 - inicio);
                let largura = Superficie::largura_texto(&texto, 1);
                s.texto(LARGURA - MARGEM - 4 - largura, y, &texto, cor_texto_fraco(t), 1);
            }
            Modo::Descartar(_) => {
                let nome = self.caminho.as_deref().map_or("o documento", nome_do_caminho);
                let pergunta = format!("Descartar as alterações em “{nome}”?");
                let fim = botoes.first().map_or(LARGURA, |b| b.r.x) - 12;
                s.texto(MARGEM + 4, y, fonte::cortar(&pergunta, fim - MARGEM - 4, &INTER), t.texto, 1);
            }
            _ => {
                s.texto(MARGEM + 4, y, self.rotulo_campo(), t.texto, 1);
                self.campo.desenhar(s, self.area_campo());
            }
        }
    }

    /// Numeros de linha, a linha do cursor realcada, o texto e o cursor.
    fn desenhar_texto(&self, s: &mut Superficie, t: &Tema) {
        let area = AREA_TEXTO;
        let papel = cor_campo(t);
        s.retangulo(area, papel);
        let numeros = Retangulo::novo(0, area.y, self.largura_numeros(), area.altura);
        s.retangulo(numeros, cor_barra(t));
        s.retangulo(Retangulo::novo(numeros.direita() - 1, area.y, 1, area.altura), cor_linha(t));
        let (x, colunas, visiveis) = (self.x_texto(), self.colunas_visiveis(), self.linhas_visiveis());
        let apagada = cor_texto_fraco(t);
        let escuro = ((papel >> 16 & 0xFF) * 3 + (papel >> 8 & 0xFF) * 6 + (papel & 0xFF)) / 10 < 128;
        for (fila, i) in (self.topo..self.linhas.len()).take(visiveis).enumerate() {
            let y = area.y + PREENCHIMENTO + fila as i32 * ALTURA_LINHA;
            let atual = i == self.linha;
            if atual {
                let faixa = Retangulo::novo(numeros.direita(), y, area.direita() - numeros.direita(), ALTURA_LINHA);
                s.retangulo(faixa, grafico::misturar(papel, t.destaque, 22));
            }
            let numero = format!("{}", i + 1);
            let xn = numeros.direita() - 8 - MONO.largura(&numero);
            s.texto_com(xn, y + BASE_LINHA, &numero, if atual { t.texto } else { apagada }, &MONO);
            let linha = &self.linhas[i];
            let inicio = byte_de(linha, self.esquerda);
            let fim = inicio + byte_de(&linha[inicio..], colunas);
            if self.linguagem == Linguagem::Nenhuma {
                s.texto_com(x, y + BASE_LINHA, &linha[inicio..fim], t.texto, &MONO);
                continue;
            }
            // trechos coloridos; o que sobra entre eles sai na cor normal
            let estado = self.estados.get(i).copied().unwrap_or(realce::NORMAL);
            let mut ultimo = inicio;
            let mut pintar = |a: usize, b: usize, cor: u32, s: &mut Superficie| {
                let (a, b) = (a.max(inicio), b.min(fim));
                if a < b {
                    let col = linha[inicio..a].chars().count() as i32;
                    s.texto_com(x + col * LARGURA_CARACTERE, y + BASE_LINHA, &linha[a..b], cor, &MONO);
                }
            };
            realce::linha(self.linguagem, linha, estado, &mut |a, b, classe| {
                if a > ultimo {
                    pintar(ultimo, a, t.texto, s);
                }
                pintar(a, b, realce::cor(classe, escuro, t.texto), s);
                ultimo = ultimo.max(b);
            });
            if ultimo < fim {
                pintar(ultimo, fim, t.texto, s);
            }
        }
        let na_vista = (self.topo..self.topo + visiveis).contains(&self.linha)
            && (self.esquerda..=self.esquerda + colunas).contains(&self.coluna);
        if na_vista && self.tarefa.is_none() && !self.com_campo() {
            let cx = x + (self.coluna - self.esquerda) as i32 * LARGURA_CARACTERE;
            let cy = area.y + PREENCHIMENTO + (self.linha - self.topo) as i32 * ALTURA_LINHA;
            s.retangulo(Retangulo::novo(cx - 1, cy, 2, ALTURA_LINHA), t.destaque);
        }
    }

    /// Barra de rolagem com as cores da Lista do kit de widgets.
    fn desenhar_rolagem(&self, s: &mut Superficie, t: &Tema) {
        let r = AREA_ROLAGEM;
        let papel = cor_campo(t);
        s.retangulo(r, grafico::misturar(papel, t.texto, 14));
        if let Some(p) = self.polegar() {
            let cor = if self.arrasto.is_some() { t.destaque } else { grafico::misturar(papel, t.texto, 80) };
            s.retangulo(p, cor);
        }
    }

    fn desenhar_estado(&self, s: &mut Superficie, t: &Tema) {
        let y = barra_estado(s, t);
        let posicao = format!("Lin {}, Col {}", self.linha + 1, self.coluna + 1);
        s.texto(MARGEM + 4, y, &posicao, t.texto, 1);
        let quebra = if self.crlf { "CRLF" } else { "LF" };
        let direita = format!("{}  |  {quebra}", contagem(self.linhas.len(), "linha", "linhas"));
        let largura = Superficie::largura_texto(&direita, 1);
        s.texto(LARGURA - MARGEM - 4 - largura, y, &direita, cor_texto_fraco(t), 1);
        if let Some((texto, erro)) = &self.mensagem {
            // depois de "Lin 99999, Col 9999", fixo para nao pular ao digitar
            let x = MARGEM + 4 + Superficie::largura_texto("Lin 99999, Col 9999", 1) + 16;
            let cor = if *erro { t.fechar } else { t.texto };
            s.texto(x, y, fonte::cortar(texto, LARGURA - largura - x - 24, &INTER), cor, 1);
        }
    }

    /// Aviso da tarefa que esta para rodar.
    fn texto_tarefa(&self) -> Option<String> {
        Some(match self.tarefa.as_ref()? {
            Tarefa::Carregar(caminho) => format!("Carregando {caminho}…"),
            Tarefa::Salvar(caminho) => format!("Salvando {caminho}…"),
            Tarefa::Ia(_) => String::from("Perguntando ao Claude… (pode levar um minuto)"),
        })
    }
}

impl Editor {
    /// Calcula os estados do realce ate a ultima linha visivel.
    fn preparar_realce(&mut self) {
        if self.linguagem == Linguagem::Nenhuma {
            return;
        }
        let ate = (self.topo + self.linhas_visiveis() + 1).min(self.linhas.len());
        self.estados.truncate(self.estados_ok.max(1));
        while self.estados.len() < ate {
            let i = self.estados.len() - 1;
            let e = realce::linha(self.linguagem, &self.linhas[i], self.estados[i], &mut |_, _, _| {});
            self.estados.push(e);
        }
        self.estados_ok = self.estados.len();
    }
}

/// Tira as cercas ``` que o modelo as vezes poe em volta do codigo.
fn sem_cercas(texto: &str) -> String {
    let t = texto.trim();
    if let Some(resto) = t.strip_prefix("```") {
        let corpo = resto.split_once('\n').map_or("", |x| x.1);
        let corpo = corpo.trim_end().strip_suffix("```").unwrap_or(corpo);
        return String::from(corpo.trim_end());
    }
    String::from(t)
}

impl App for Editor {
    fn titulo(&self) -> &str {
        &self.titulo
    }

    fn tamanho(&self) -> (usize, usize) {
        (LARGURA as usize, ALTURA as usize)
    }

    fn desenhar(&mut self, s: &mut Superficie) {
        self.preparar_realce();
        let t = tema::atual();
        s.limpar(t.fundo_janela);
        self.desenhar_barra(s, &t);
        self.desenhar_texto(s, &t);
        self.desenhar_rolagem(s, &t);
        if let Some(texto) = self.texto_tarefa() {
            desenhar_aviso(s, AREA_TEXTO, &texto, &t);
        }
        self.desenhar_estado(s, &t);
        self.quadro_mostrado = true;
    }

    fn pedidos(&mut self) -> Vec<Pedido> {
        core::mem::take(&mut self.pedidos)
    }

    fn evento(&mut self, e: EventoApp) -> bool {
        if let EventoApp::Tecla { codigo: KEY_LEFTCTRL | KEY_RIGHTCTRL, pressionada } = e {
            self.ctrl = pressionada;
            return false;
        }
        if e == EventoApp::Tique {
            return self.tique();
        }
        if self.tarefa.is_some() {
            return false; // carregando ou salvando: espera o proximo Tique
        }
        match e {
            EventoApp::Botao { x, y, botao: Botao::Esquerdo, pressionado: true } => self.apertar(x, y, e),
            EventoApp::Botao { x, y, botao: Botao::Esquerdo, pressionado: false } => self.soltar(x, y),
            EventoApp::Mouse { y, botoes, .. } if botoes != 0 => self.arrastar(y),
            EventoApp::Roda(passos) => self.rolar(-(passos as isize) * 3),
            EventoApp::Caractere(c) => self.caractere(c, e),
            EventoApp::Tecla { codigo, pressionada: true } => self.tecla(codigo, e),
            _ => false,
        }
    }
}

/// Bytes do arquivo -> linhas: (linhas, usava "\r\n", bytes lidos como Latin-1).
fn linhas_do_arquivo(dados: &[u8]) -> (Vec<String>, bool, usize) {
    let mut linhas = Vec::new();
    let mut atual = String::new();
    let (mut crlf, mut trocados, mut i) = (false, 0, 0);
    while i < dados.len() {
        let (c, tamanho) = decodificar(&dados[i..]);
        trocados += usize::from(c.is_none());
        let c = c.unwrap_or(dados[i] as char); // byte solto: Latin-1
        i += tamanho;
        match c {
            '\n' => linhas.push(core::mem::take(&mut atual)),
            '\r' => {
                if dados.get(i) == Some(&b'\n') {
                    i += 1;
                    crlf = true;
                }
                linhas.push(core::mem::take(&mut atual));
            }
            '\t' => {
                let n = ESPACOS_TAB - atual.chars().count() % ESPACOS_TAB;
                atual.push_str(&"    "[..n]);
            }
            '\u{feff}' if linhas.is_empty() && atual.is_empty() => {} // BOM do UTF-8
            c if c.is_control() => atual.push('?'),
            c => atual.push(c),
        }
    }
    linhas.push(atual);
    (linhas, crlf, trocados)
}

/// O primeiro caractere UTF-8 de `bytes` e quantos bytes ele usa; None
/// (com tamanho 1) se ali nao comeca um UTF-8 valido.
fn decodificar(bytes: &[u8]) -> (Option<char>, usize) {
    let tamanho = match bytes[0] {
        0x00..=0x7F => 1,
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => return (None, 1),
    };
    match bytes.get(..tamanho).and_then(|b| core::str::from_utf8(b).ok()) {
        Some(texto) => (texto.chars().next(), tamanho),
        None => (None, 1),
    }
}

/// Byte de `texto` onde comeca o caractere numero `coluna` (o fim do
/// texto se ele for mais curto).
fn byte_de(texto: &str, coluna: usize) -> usize {
    texto.char_indices().nth(coluna).map_or(texto.len(), |(i, _)| i)
}

/// Quantos digitos decimais `n` tem.
fn digitos(mut n: usize) -> usize {
    let mut d = 1;
    while n >= 10 {
        n /= 10;
        d += 1;
    }
    d
}

/// O fim de `texto` que cabe em `largura` px, com "…" na frente se cortou.
fn cortar_inicio(texto: &str, largura: i32) -> String {
    if INTER.largura(texto) <= largura {
        return String::from(texto);
    }
    let livre = largura - INTER.largura("…");
    let mut usado = 0;
    let mut inicio = texto.len();
    for (i, c) in texto.char_indices().rev() {
        usado += INTER.glifo(c).avanco;
        if usado > livre {
            break;
        }
        inicio = i;
    }
    format!("…{}", &texto[inicio..])
}

/// `pasta` terminando com "/" (para o usuario so digitar o nome).
fn com_barra(pasta: &str) -> String {
    if pasta.ends_with('/') { String::from(pasta) } else { format!("{pasta}/") }
}

/// Onde Abrir e Salvar como comecam sem arquivo: /documentos/ se existir.
fn pasta_padrao() -> String {
    if armazenamento::listar("/documentos").is_ok() { String::from("/documentos/") } else { String::from("/") }
}
