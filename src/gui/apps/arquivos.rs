//! Gerenciador de arquivos e discos.
//!
//! Duas vistas na mesma janela:
//!   - pasta:  a lista (nome, tamanho, tipo), pastas primeiro e por nome
//!   - discos: um cartao por disco de armazenamento::discos(), com
//!     rotulo, sistema de arquivos, tamanho e a barra de uso
//!
//! Em cima ficam duas barras. A de navegacao tem Voltar, Cima, Raiz,
//! Atualizar, o caminho (Enter navega; um arquivo abre no app certo) e o
//! botao que troca de vista. A de comandos tem Nova pasta, Renomear, Apagar,
//! Recortar, Copiar e Colar, e tambem faz as vezes de dialogo: pede o nome
//! num CampoTexto ou confirma antes de apagar. Embaixo, a barra de estado.
//!
//! Copiar e recortar guardam o caminho numa area de transferencia global
//! (vale de uma janela do gerenciador para outra). Pastas sao copiadas e
//! apagadas com tudo que tem dentro. Colar e apagar rodam no Tique seguinte,
//! depois de um quadro com o aviso, porque a area de trabalho para enquanto
//! o disco trabalha. A cada 2 s a pasta e lida de novo: um arquivo salvo
//! por outra janela aparece sozinho.
//!
//! Teclado fora dos campos: setas e Enter na lista, Backspace sobe uma
//! pasta, Delete apaga, F2 renomeia, F5 atualiza, Ctrl+C, Ctrl+X e Ctrl+V.
//!
//! O editor de texto usa daqui o que e igual nos dois: caminhos, botoes das
//! barras, barra de estado e o aviso de trabalho em andamento.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::cmp::Ordering;

use crate::armazenamento::{self, Erro, InfoDisco, ItemPasta};
use crate::entrada::Botao;
use crate::entrada::teclado::{KEY_DELETE, KEY_DOWN, KEY_ENTER, KEY_KPENTER, KEY_LEFTCTRL, KEY_RIGHTCTRL, KEY_UP};
use crate::fonte::{self, INTER, INTER_NEGRITO};
use crate::gui::grafico::{self, Cor, Retangulo, Superficie};
use crate::gui::janelas::{App, EventoApp, Pedido};
use crate::gui::tema::{self, Tema};
use crate::gui::widgets::{
    self, ALTURA_BOTAO, AcaoCampo, AcaoLista, CampoTexto, DuploClique, Icone, ItemLista, Lista, cor_linha,
    cor_texto_fraco,
};

const LARGURA: i32 = 900;
/// Coluna da arvore de pastas (so na vista de pasta).
const LARGURA_ARVORE: i32 = 190;
const ALTURA: i32 = 480;
/// Espaco entre a borda e os botoes das barras.
pub const MARGEM: i32 = 6;
/// Altura de cada barra de botoes.
pub const ALTURA_BARRA: i32 = ALTURA_BOTAO + 2 * MARGEM;
pub const ALTURA_ESTADO: i32 = 24;
/// Espaco entre dois botoes vizinhos.
pub const ESPACO: i32 = 4;
const TOPO_COMANDOS: i32 = ALTURA_BARRA;
/// Onde comecam a lista e os cartoes dos discos.
const TOPO_CONTEUDO: i32 = 2 * ALTURA_BARRA;
/// Botao so com icone (Voltar, Cima, Raiz, Atualizar).
const LARGURA_ICONE: i32 = 30;
/// Botao que troca de vista ("Discos" / "Arquivos").
const LARGURA_VISTA: i32 = 100;
/// Colunas da lista (a ultima vai ate a borda).
const COLUNAS: [&str; 3] = ["Nome", "Tamanho", "Tipo"];
const LARGURAS: [i32; 3] = [400, 100, 200];
/// Onde o texto da coluna "Tamanho" termina, contado do comeco dela
/// (a lista escreve 6 px depois da borda e corta 4 px antes da outra).
const LARGURA_TAMANHO: i32 = LARGURAS[1] - 6 - 12;
const ALTURA_CARTAO: i32 = 80;
const ESPACO_CARTOES: i32 = 10;
const MAX_HISTORICO: usize = 32;
/// Pastas dentro de pastas que copiar e apagar seguem, no maximo.
const MAX_PROFUNDIDADE: u32 = 16;
/// De quantos em quantos Tiques a pasta e lida de novo (2 s).
const TIQUES_CONFERIR: u32 = 20;

// Codigos evdev (KEY_*) dos atalhos que o teclado.rs nao exporta.
const TECLA_X: u16 = 45;
const TECLA_C: u16 = 46;
const TECLA_V: u16 = 47;
const TECLA_F2: u16 = 60;
const TECLA_F5: u16 = 63;

/// Tipos de arquivo conhecidos: extensoes, nome na coluna "Tipo" e icone.
const TIPOS: &[(&[&str], &str, Icone)] = &[
    (&["txt"], "Documento de texto", Icone::Texto),
    (&["htm", "html"], "Página HTML", Icone::Html),
    (&["md"], "Texto Markdown", Icone::Texto),
    (&["cfg", "ini", "conf"], "Configuração", Icone::Engrenagem),
    (&["log"], "Registro", Icone::Texto),
    (&["csv"], "Tabela CSV", Icone::Texto),
    (&["rs", "c", "h", "py", "s", "js", "css"], "Código-fonte", Icone::Texto),
    (&["png", "bmp", "ppm", "jpg", "gif"], "Imagem", Icone::Pincel),
    (&["wav", "mp3"], "Som", Icone::Som),
    (&["img", "iso"], "Imagem de disco", Icone::Disco),
    (&["bin", "elf", "exe"], "Programa", Icone::Chip),
];

/// Cria o gerenciador na raiz (menu iniciar e icone da area de trabalho).
pub fn criar() -> Box<dyn App> {
    Box::new(Arquivos::novo("/"))
}

/// Cria o gerenciador ja na vista dos discos (menu iniciar).
pub fn criar_discos() -> Box<dyn App> {
    Box::new(Arquivos::discos())
}

// ------------------------------------------------- area de transferencia

/// O que foi copiado ou recortado.
#[derive(Clone)]
struct Copiado {
    caminho: String,
    pasta: bool,
    recortar: bool,
}

/// Area de transferencia de todas as janelas do gerenciador. So o laco
/// principal da GUI mexe (nunca uma interrupcao): basta um UnsafeCell.
/// Guarda varios itens de uma vez (selecao em area).
struct Transferencia(UnsafeCell<Vec<Copiado>>);

unsafe impl Sync for Transferencia {}

static TRANSFERENCIA: Transferencia = Transferencia(UnsafeCell::new(Vec::new()));

impl Transferencia {
    fn ler(&self) -> Vec<Copiado> {
        unsafe { (*self.0.get()).clone() }
    }

    fn tem_algo(&self) -> bool {
        unsafe { !(*self.0.get()).is_empty() }
    }

    fn guardar(&self, copiados: Vec<Copiado>) {
        unsafe { *self.0.get() = copiados };
    }
}

// ------------------------------------------------------------- o app

/// Onde a janela esta.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Local {
    Pasta(String),
    Discos,
}

/// O que a barra de comandos mostra.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Modo {
    Normal,
    /// Pede o nome de uma pasta nova.
    NovaPasta,
    /// Pede o nome novo do item selecionado.
    Renomear,
    /// Confirma antes de apagar o item selecionado.
    Apagar,
}

/// Trabalho demorado, feito no Tique depois do quadro com o aviso.
enum Tarefa {
    Colar(Vec<Copiado>),
    /// Caminhos e posicao na lista (depois seleciona o vizinho).
    Apagar(Vec<String>, usize),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Acao {
    Voltar,
    Cima,
    Raiz,
    Atualizar,
    TrocarVista,
    NovaPasta,
    Renomear,
    Apagar,
    Recortar,
    Copiar,
    Colar,
    AbrirDisco,
    Confirmar,
    Cancelar,
}

/// Botoes so com icone da barra de navegacao, da esquerda para a direita.
const NAVEGACAO: [(Acao, Icone); 4] = [
    (Acao::Voltar, Icone::Voltar),
    (Acao::Cima, Icone::Cima),
    (Acao::Raiz, Icone::Casa),
    (Acao::Atualizar, Icone::Recarregar),
];

const COMANDOS: [(Acao, &str); 6] = [
    (Acao::NovaPasta, "Nova pasta"),
    (Acao::Renomear, "Renomear"),
    (Acao::Apagar, "Apagar"),
    (Acao::Recortar, "Recortar"),
    (Acao::Copiar, "Copiar"),
    (Acao::Colar, "Colar"),
];

pub struct Arquivos {
    local: Local,
    historico: Vec<Local>,
    /// Ultima pasta mostrada: para onde o botao "Arquivos" volta.
    ultima_pasta: String,
    /// Itens da pasta na ordem da lista (pastas primeiro, por nome).
    itens: Vec<ItemPasta>,
    lista: Lista,
    /// Por que a pasta nao pode ser lida (aparece no lugar da lista).
    erro_pasta: Option<&'static str>,
    discos: Vec<InfoDisco>,
    disco: Option<usize>,
    /// Clique duplo nos cartoes dos discos.
    duplo: DuploClique,
    /// Bytes livres no volume montado.
    livre: Option<u64>,
    campo_caminho: CampoTexto,
    campo_nome: CampoTexto,
    modo: Modo,
    /// Botao apertado com o mouse (age ao soltar em cima dele).
    apertado: Option<Acao>,
    /// Mensagem da barra de estado: (texto, e um erro).
    mensagem: Option<(String, bool)>,
    tarefa: Option<Tarefa>,
    /// O quadro com o aviso da tarefa ja foi desenhado.
    quadro_mostrado: bool,
    tiques: u32,
    ctrl: bool,
    titulo: String,
    pedidos: Vec<Pedido>,
    arvore: super::arvore::Arvore,
    /// Ultima posicao do mouse (a roda rola o que esta embaixo dele).
    mouse: (i32, i32),
}

impl Arquivos {
    /// Gerenciador aberto na pasta `pasta`.
    pub fn novo(pasta: &str) -> Arquivos {
        let mut arquivos = Arquivos::vazio();
        arquivos.ir_para(Local::Pasta(normalizar(pasta)), false);
        arquivos
    }

    /// Gerenciador aberto na vista dos discos.
    pub fn discos() -> Arquivos {
        let mut arquivos = Arquivos::vazio();
        arquivos.ir_para(Local::Discos, false);
        arquivos
    }

    fn vazio() -> Arquivos {
        let mut lista = Lista::nova();
        lista.definir_colunas(&COLUNAS, &LARGURAS);
        lista.permitir_varios();
        Arquivos {
            local: Local::Pasta(String::from("/")),
            historico: Vec::new(),
            ultima_pasta: String::from("/"),
            itens: Vec::new(),
            lista,
            erro_pasta: None,
            discos: Vec::new(),
            disco: None,
            duplo: DuploClique::novo(),
            livre: None,
            campo_caminho: CampoTexto::novo("/"),
            campo_nome: CampoTexto::novo(""),
            modo: Modo::Normal,
            apertado: None,
            mensagem: None,
            tarefa: None,
            quadro_mostrado: false,
            tiques: 0,
            ctrl: false,
            titulo: String::new(),
            pedidos: Vec::new(),
            arvore: super::arvore::Arvore::nova(),
            mouse: (0, 0),
        }
    }

    fn area_arvore(&self) -> Retangulo {
        Retangulo::novo(0, TOPO_CONTEUDO, LARGURA_ARVORE, ALTURA - TOPO_CONTEUDO - ALTURA_ESTADO)
    }

    // ------------------------------------------------------- navegacao

    fn pasta_atual(&self) -> Option<&str> {
        match &self.local {
            Local::Pasta(caminho) => Some(caminho),
            Local::Discos => None,
        }
    }

    fn item_selecionado(&self) -> Option<&ItemPasta> {
        self.lista.selecionado().and_then(|i| self.itens.get(i))
    }

    fn disco_escolhido(&self) -> Option<&InfoDisco> {
        self.disco.and_then(|i| self.discos.get(i))
    }

    /// O que o campo do caminho mostra.
    fn texto_local(&self) -> String {
        match &self.local {
            Local::Pasta(caminho) => caminho.clone(),
            Local::Discos => String::from("Discos"),
        }
    }

    /// Mostra `destino`. Com `guardar`, o lugar de agora vai para o historico.
    fn ir_para(&mut self, destino: Local, guardar: bool) {
        if guardar && destino != self.local {
            if self.historico.len() >= MAX_HISTORICO {
                self.historico.remove(0);
            }
            self.historico.push(self.local.clone());
        }
        if let Local::Pasta(caminho) = &destino {
            self.ultima_pasta = caminho.clone();
            let linhas = super::arvore::Arvore::linhas_visiveis(self.area_arvore());
            self.arvore.mostrar(caminho, linhas);
        }
        self.local = destino;
        self.cancelar_modo();
        self.mensagem = None;
        self.duplo.esquecer();
        // lista vazia antes: a pasta nova comeca do topo e sem selecao
        self.lista.definir_itens(Vec::new());
        self.recarregar(None);
        let texto = self.texto_local();
        self.campo_caminho.definir(&texto);
        self.titulo = match &self.local {
            Local::Pasta(caminho) => format!("Arquivos — {}", nome_do_caminho(caminho)),
            Local::Discos => String::from("Discos"),
        };
    }

    /// Le de novo a pasta (ou os discos) e refaz a lista, selecionando o
    /// item `selecionar` se ele existir.
    fn recarregar(&mut self, selecionar: Option<&str>) {
        let discos = armazenamento::discos();
        self.livre = discos.iter().find(|d| d.montado).map(|d| d.livre);
        self.erro_pasta = None;
        match &self.local {
            Local::Discos => {
                self.disco = if discos.is_empty() { None } else { Some(self.disco.unwrap_or(0).min(discos.len() - 1)) };
                self.discos = discos;
                self.itens.clear();
            }
            Local::Pasta(caminho) => match armazenamento::listar(caminho) {
                Ok(mut itens) => {
                    ordenar(&mut itens);
                    self.itens = itens;
                }
                Err(e) => {
                    self.itens.clear();
                    self.erro_pasta = Some(e.descricao());
                }
            },
        }
        self.lista.definir_itens(self.itens.iter().map(item_da_lista).collect());
        let indice = selecionar.and_then(|nome| self.itens.iter().position(|i| mesmo_nome(&i.nome, nome)));
        self.lista.selecionar(indice);
    }

    /// Enter no campo do caminho: vai para a pasta ou abre o arquivo.
    fn ir_para_digitado(&mut self) {
        let texto = String::from(self.campo_caminho.texto().trim());
        if texto.trim_end_matches(':').eq_ignore_ascii_case("discos") {
            self.ir_para(Local::Discos, true);
            return;
        }
        let caminho = normalizar(&texto);
        match armazenamento::listar(&caminho) {
            Ok(_) => self.ir_para(Local::Pasta(caminho), true),
            Err(Erro::NaoEPasta) => {
                self.pedidos.push(Pedido::AbrirArquivo(caminho));
                self.restaurar_caminho();
            }
            Err(e) => {
                self.erro(format!("{caminho}: {}", e.descricao()));
                self.restaurar_caminho();
            }
        }
    }

    fn restaurar_caminho(&mut self) {
        let texto = self.texto_local();
        self.campo_caminho.definir(&texto);
    }

    /// Cima: a pasta de fora (selecionando a de onde veio); da raiz, os discos.
    fn subir(&mut self) {
        let Some(atual) = self.pasta_atual().map(String::from) else { return };
        if atual == "/" {
            self.ir_para(Local::Discos, true);
            return;
        }
        self.ir_para(Local::Pasta(pai_do_caminho(&atual)), true);
        let nome = nome_do_caminho(&atual);
        let indice = self.itens.iter().position(|i| mesmo_nome(&i.nome, nome));
        self.lista.selecionar(indice);
    }

    /// Duplo clique ou Enter num item: entra na pasta ou abre o arquivo.
    fn abrir_item(&mut self, i: usize) {
        let (Some(pasta), Some(item)) = (self.pasta_atual(), self.itens.get(i)) else { return };
        let caminho = juntar(pasta, &item.nome);
        if item.pasta {
            self.ir_para(Local::Pasta(caminho), true);
        } else {
            self.pedidos.push(Pedido::AbrirArquivo(caminho));
        }
    }

    /// O disco escolhido, se estiver montado, abre na raiz.
    fn abrir_disco(&mut self) {
        match self.disco_escolhido() {
            Some(d) if d.montado => self.ir_para(Local::Pasta(String::from("/")), true),
            Some(_) => self.info(String::from("Este disco não está montado (o MXOS monta o primeiro FAT32 em /)")),
            None => {}
        }
    }

    // ------------------------------------------------------- comandos

    fn habilitada(&self, acao: Acao) -> bool {
        let em_pasta = self.pasta_atual().is_some() && self.erro_pasta.is_none();
        let com_item = em_pasta && self.item_selecionado().is_some();
        match acao {
            Acao::Voltar => !self.historico.is_empty(),
            Acao::Cima => self.pasta_atual().is_some(),
            Acao::NovaPasta => em_pasta,
            Acao::Renomear | Acao::Apagar | Acao::Recortar | Acao::Copiar => com_item,
            Acao::Colar => em_pasta && TRANSFERENCIA.tem_algo(),
            Acao::AbrirDisco => self.disco_escolhido().is_some_and(|d| d.montado),
            _ => true,
        }
    }

    /// A acao principal da barra (botao na cor de destaque).
    fn principal(&self, acao: Acao) -> bool {
        matches!(acao, Acao::Confirmar | Acao::AbrirDisco) && self.habilitada(acao)
    }

    /// Executa `acao` se ela puder ser usada agora. true = executou.
    fn tentar(&mut self, acao: Acao) -> bool {
        let pode = self.habilitada(acao);
        if pode {
            self.executar(acao);
        }
        pode
    }

    fn executar(&mut self, acao: Acao) {
        match acao {
            Acao::Voltar => {
                if let Some(local) = self.historico.pop() {
                    self.ir_para(local, false);
                }
            }
            Acao::Cima => self.subir(),
            Acao::Raiz => self.ir_para(Local::Pasta(String::from("/")), true),
            Acao::Atualizar => {
                self.mensagem = None;
                let selecionado = self.item_selecionado().map(|i| i.nome.clone());
                self.recarregar(selecionado.as_deref());
            }
            Acao::TrocarVista => {
                let destino = match self.local {
                    Local::Pasta(_) => Local::Discos,
                    Local::Discos => Local::Pasta(self.ultima_pasta.clone()),
                };
                self.ir_para(destino, true);
            }
            Acao::NovaPasta | Acao::Renomear => self.pedir_nome(acao),
            Acao::Apagar => {
                self.modo = Modo::Apagar;
                self.mensagem = None;
            }
            Acao::Recortar => self.copiar(true),
            Acao::Copiar => self.copiar(false),
            Acao::Colar => self.agendar_colar(),
            Acao::AbrirDisco => self.abrir_disco(),
            Acao::Confirmar if self.modo == Modo::Apagar => self.agendar_apagar(),
            Acao::Confirmar => self.confirmar_nome(),
            Acao::Cancelar => self.cancelar_modo(),
        }
    }

    /// Troca os comandos pelo campo do nome (Nova pasta, Renomear).
    fn pedir_nome(&mut self, acao: Acao) {
        let (modo, texto) = if acao == Acao::Renomear {
            let Some(item) = self.item_selecionado() else { return };
            (Modo::Renomear, item.nome.clone())
        } else {
            (Modo::NovaPasta, nome_livre(&self.itens, "Nova pasta", true))
        };
        self.modo = modo;
        self.mensagem = None;
        self.campo_nome.definir(&texto);
        self.campo_nome.focar(true);
        self.campo_caminho.focar(false);
    }

    fn cancelar_modo(&mut self) {
        self.modo = Modo::Normal;
        self.campo_nome.focar(false);
    }

    /// OK do campo do nome: cria a pasta ou renomeia. Com erro o campo fica
    /// aberto para corrigir o nome.
    fn confirmar_nome(&mut self) {
        let Some(pasta) = self.pasta_atual().map(String::from) else { return };
        let nome = String::from(self.campo_nome.texto().trim());
        if nome.is_empty() {
            self.erro(String::from("Digite um nome"));
            return;
        }
        if nome.contains(['/', '\\']) {
            self.erro(String::from("O nome não pode ter / nem \\"));
            return;
        }
        let resultado = if self.modo == Modo::Renomear {
            let Some(antigo) = self.item_selecionado().map(|i| i.nome.clone()) else {
                self.cancelar_modo();
                return;
            };
            if antigo == nome {
                self.cancelar_modo();
                return;
            }
            armazenamento::renomear(&juntar(&pasta, &antigo), &juntar(&pasta, &nome))
                .map(|()| format!("Renomeado: {antigo} → {nome}"))
                .map_err(|e| format!("Não foi possível renomear: {}", e.descricao()))
        } else {
            armazenamento::criar_pasta(&juntar(&pasta, &nome))
                .map(|()| format!("Pasta criada: {nome}"))
                .map_err(|e| format!("Não foi possível criar a pasta: {}", e.descricao()))
        };
        match resultado {
            Ok(texto) => {
                self.cancelar_modo();
                self.recarregar(Some(&nome));
                self.info(texto);
            }
            Err(texto) => self.erro(texto),
        }
    }

    /// Os itens escolhidos na lista (um, ou varios pela selecao em area).
    fn escolhidos(&self) -> Vec<&ItemPasta> {
        self.lista.marcados().into_iter().filter_map(|i| self.itens.get(i)).collect()
    }

    /// "nome" para um item, "3 itens" para varios.
    fn nome_dos_escolhidos(&self) -> String {
        match self.escolhidos().as_slice() {
            [item] => item.nome.clone(),
            varios => format!("{} itens", varios.len()),
        }
    }

    /// Copiar ou recortar os itens escolhidos para a area de transferencia.
    fn copiar(&mut self, recortar: bool) {
        let Some(pasta) = self.pasta_atual().map(String::from) else { return };
        let copiados: Vec<Copiado> = self
            .escolhidos()
            .iter()
            .map(|item| Copiado { caminho: juntar(&pasta, &item.nome), pasta: item.pasta, recortar })
            .collect();
        if copiados.is_empty() {
            return;
        }
        let verbo = if recortar { "Recortado" } else { "Copiado" };
        let texto = format!("{verbo}: {} (abra a pasta de destino e clique em Colar)", self.nome_dos_escolhidos());
        TRANSFERENCIA.guardar(copiados);
        self.info(texto);
    }

    fn agendar_colar(&mut self) {
        let copiados = TRANSFERENCIA.ler();
        if !copiados.is_empty() {
            self.agendar(Tarefa::Colar(copiados));
        }
    }

    fn agendar_apagar(&mut self) {
        self.modo = Modo::Normal;
        let Some(pasta) = self.pasta_atual().map(String::from) else { return };
        let caminhos: Vec<String> = self.escolhidos().iter().map(|item| juntar(&pasta, &item.nome)).collect();
        let Some(&primeiro) = self.lista.marcados().first() else { return };
        self.agendar(Tarefa::Apagar(caminhos, primeiro));
    }

    /// Guarda a tarefa; ela roda no primeiro Tique depois do quadro com o aviso.
    fn agendar(&mut self, tarefa: Tarefa) {
        self.tarefa = Some(tarefa);
        self.quadro_mostrado = false;
        self.mensagem = None;
    }

    fn fazer_tarefa(&mut self, tarefa: Tarefa) {
        match tarefa {
            Tarefa::Colar(copiados) => self.colar_todos(copiados),
            Tarefa::Apagar(caminhos, i) => self.apagar_todos(&caminhos, i),
        }
    }

    /// Cola tudo o que esta na area de transferencia (um por vez; a
    /// mensagem do ultimo fica, ou o resumo se foram varios).
    fn colar_todos(&mut self, copiados: Vec<Copiado>) {
        let n = copiados.len();
        let mut erros = 0;
        for copiado in copiados {
            self.colar(copiado);
            erros += self.mensagem.as_ref().is_some_and(|(_, erro)| *erro) as usize;
        }
        if n > 1 && erros == 0 {
            self.info(format!("Colados: {n} itens"));
        }
    }

    /// Cola na pasta atual com um nome que ainda nao existe nela.
    fn colar(&mut self, copiado: Copiado) {
        let Some(pasta) = self.pasta_atual().map(String::from) else { return };
        let nome = nome_do_caminho(&copiado.caminho);
        if copiado.recortar && mesmo_nome(&pai_do_caminho(&copiado.caminho), &pasta) {
            self.info(format!("{nome} já está nesta pasta"));
            return;
        }
        if copiado.pasta && dentro_de(&pasta, &copiado.caminho) {
            self.erro(String::from("Não dá para colar uma pasta dentro dela mesma"));
            return;
        }
        let existentes = armazenamento::listar(&pasta).unwrap_or_default();
        let novo = nome_livre(&existentes, nome, copiado.pasta);
        let destino = juntar(&pasta, &novo);
        let resultado = if copiado.recortar {
            armazenamento::renomear(&copiado.caminho, &destino).map(|()| 1).map_err(|e| e.descricao())
        } else {
            copiar_tudo(&copiado.caminho, &destino, 0)
        };
        match resultado {
            Ok(n) => {
                if copiado.recortar {
                    // o original nao existe mais
                    let resto = TRANSFERENCIA.ler().into_iter().filter(|c| c.caminho != copiado.caminho).collect();
                    TRANSFERENCIA.guardar(resto);
                }
                self.recarregar(Some(&novo));
                let extra = if n > 1 { format!(" ({n} itens)") } else { String::new() };
                self.info(format!("Colado: {novo}{extra}"));
            }
            Err(e) => {
                self.recarregar(None);
                self.erro(format!("Não foi possível colar {nome}: {e}"));
            }
        }
    }

    /// Apaga cada caminho (com tudo dentro, se for pasta) e seleciona o
    /// vizinho do primeiro.
    fn apagar_todos(&mut self, caminhos: &[String], indice: usize) {
        let mut total = 0;
        let mut falha = None;
        for caminho in caminhos {
            match apagar_tudo(caminho, 0) {
                Ok(n) => total += n,
                Err(e) => falha = Some(format!("Não foi possível apagar {}: {e}", nome_do_caminho(caminho))),
            }
            // o que estava na area de transferencia pode ter ido junto
            let resto = TRANSFERENCIA.ler().into_iter().filter(|c| !dentro_de(&c.caminho, caminho)).collect();
            TRANSFERENCIA.guardar(resto);
        }
        self.recarregar(None);
        if !self.itens.is_empty() {
            self.lista.selecionar(Some(indice.min(self.itens.len() - 1)));
        }
        match (falha, caminhos) {
            (Some(texto), _) => self.erro(texto),
            (None, [um]) if total > 1 => self.info(format!("Apagado: {} ({total} itens)", nome_do_caminho(um))),
            (None, [um]) => self.info(format!("Apagado: {}", nome_do_caminho(um))),
            (None, varios) if total as usize > varios.len() => {
                self.info(format!("Apagados: {} itens ({total} contando o que havia dentro)", varios.len()))
            }
            (None, varios) => self.info(format!("Apagados: {} itens", varios.len())),
        }
    }

    fn info(&mut self, texto: String) {
        self.mensagem = Some((texto, false));
    }

    fn erro(&mut self, texto: String) {
        self.mensagem = Some((texto, true));
    }

    // ------------------------------------------------- botoes e areas

    /// Todos os botoes das duas barras, como estao agora.
    fn botoes(&self) -> Vec<BotaoBarra<Acao>> {
        let mut botoes = Vec::with_capacity(12);
        for (i, &(acao, icone)) in NAVEGACAO.iter().enumerate() {
            let r = Retangulo::novo(MARGEM + i as i32 * (LARGURA_ICONE + ESPACO), MARGEM, LARGURA_ICONE, ALTURA_BOTAO);
            botoes.push(BotaoBarra { acao, r, icone: Some(icone), texto: "" });
        }
        let (icone, texto) = match self.local {
            Local::Pasta(_) => (Icone::Disco, "Discos"),
            Local::Discos => (Icone::Pasta, "Arquivos"),
        };
        let vista = Retangulo::novo(LARGURA - MARGEM - LARGURA_VISTA, MARGEM, LARGURA_VISTA, ALTURA_BOTAO);
        botoes.push(BotaoBarra { acao: Acao::TrocarVista, r: vista, icone: Some(icone), texto });

        let y = TOPO_COMANDOS + MARGEM;
        match (&self.local, self.modo) {
            (Local::Discos, _) => {
                let r = Retangulo::novo(MARGEM, y, largura_botao("Abrir"), ALTURA_BOTAO);
                botoes.push(BotaoBarra { acao: Acao::AbrirDisco, r, icone: None, texto: "Abrir" });
            }
            (_, Modo::Normal) => {
                let mut x = MARGEM;
                for &(acao, texto) in &COMANDOS {
                    let r = Retangulo::novo(x, y, largura_botao(texto), ALTURA_BOTAO);
                    x = r.direita() + ESPACO;
                    botoes.push(BotaoBarra { acao, r, icone: None, texto });
                }
            }
            (_, modo) => {
                let texto = if modo == Modo::Apagar { "Apagar" } else { "OK" };
                botoes.extend(botoes_dialogo(Acao::Confirmar, texto, Acao::Cancelar, y, LARGURA));
            }
        }
        botoes
    }

    /// O que aparece embaixo das barras (lista ou cartoes).
    fn area_conteudo(&self) -> Retangulo {
        let x = if matches!(self.local, Local::Pasta(_)) { LARGURA_ARVORE } else { 0 };
        Retangulo::novo(x, TOPO_CONTEUDO, LARGURA - x, ALTURA - TOPO_CONTEUDO - ALTURA_ESTADO)
    }

    /// A lista encosta nas bordas: o contorno dela fica por baixo da borda
    /// da janela e das linhas das barras (sem linha dupla).
    fn area_lista(&self) -> Retangulo {
        let c = self.area_conteudo();
        Retangulo::novo(c.x - 1, c.y - 1, c.largura + 2, c.altura + 2)
    }

    /// Campo do caminho: entre os botoes de icone e o de trocar de vista.
    fn area_caminho(&self) -> Retangulo {
        let x = MARGEM + NAVEGACAO.len() as i32 * (LARGURA_ICONE + ESPACO) + 4;
        let fim = LARGURA - MARGEM - LARGURA_VISTA - 8;
        Retangulo::novo(x, MARGEM, fim - x, ALTURA_BOTAO)
    }

    fn rotulo_nome(&self) -> &'static str {
        if self.modo == Modo::Renomear { "Novo nome:" } else { "Nome da pasta:" }
    }

    /// Campo do nome: depois do rotulo, ate os botoes OK e Cancelar.
    fn area_campo_nome(&self) -> Retangulo {
        let x = MARGEM + 4 + INTER_NEGRITO.largura(self.rotulo_nome()) + 8;
        let [ok, _] = botoes_dialogo(Acao::Confirmar, "OK", Acao::Cancelar, 0, LARGURA);
        Retangulo::novo(x, TOPO_COMANDOS + MARGEM, ok.r.x - 10 - x, ALTURA_BOTAO)
    }

    fn retangulo_cartao(&self, i: usize) -> Retangulo {
        let y = TOPO_CONTEUDO + 12 + i as i32 * (ALTURA_CARTAO + ESPACO_CARTOES);
        Retangulo::novo(12, y, LARGURA - 24, ALTURA_CARTAO)
    }

    /// Cartoes que cabem inteiros na area de conteudo.
    fn cartoes_visiveis(&self) -> usize {
        let fim = self.area_conteudo().baixo() - 4;
        (0..self.discos.len()).take_while(|&i| self.retangulo_cartao(i).baixo() <= fim).count()
    }

    // ------------------------------------------------------- eventos

    fn apertar(&mut self, x: i32, y: i32, e: EventoApp) -> bool {
        // o campo do caminho ganha ou perde o foco com qualquer clique
        let mut redesenhar = self.evento_campo_caminho(e);
        if self.modo != Modo::Normal {
            if !Retangulo::novo(0, TOPO_COMANDOS, LARGURA, ALTURA_BARRA).contem(x, y) {
                // clique fora da barra de comandos desiste do dialogo
                self.cancelar_modo();
                redesenhar = true;
            } else if self.modo != Modo::Apagar {
                let area = self.area_campo_nome();
                let _ = self.campo_nome.evento(e, area);
                self.campo_nome.focar(true);
                redesenhar = true;
            }
        }
        if let Some(acao) = acao_em(&self.botoes(), x, y) {
            // botao desabilitado nem afunda
            if self.habilitada(acao) {
                self.apertado = Some(acao);
                return true;
            }
            return redesenhar;
        }
        if matches!(self.local, Local::Pasta(_)) && self.area_arvore().contem(x, y) {
            return match self.arvore.clique(x, y, self.area_arvore()) {
                super::arvore::Clique::Navegar(c) => {
                    self.ir_para(Local::Pasta(c), true);
                    true
                }
                super::arvore::Clique::Mudou => true,
                super::arvore::Clique::Nada => redesenhar,
            };
        }
        if !self.area_conteudo().contem(x, y) {
            return redesenhar;
        }
        match self.local {
            Local::Pasta(_) => self.evento_lista(e) || redesenhar,
            Local::Discos => self.clicar_disco(x, y) || redesenhar,
        }
    }

    fn soltar(&mut self, x: i32, y: i32, e: EventoApp) -> bool {
        if let Some(acao) = self.apertado.take() {
            if acao_em(&self.botoes(), x, y) == Some(acao) {
                self.tentar(acao);
            }
            return true;
        }
        // a lista pode estar arrastando a barra de rolagem
        self.pasta_atual().is_some() && self.evento_lista(e)
    }

    fn caractere(&mut self, c: char, e: EventoApp) -> bool {
        self.ctrl = false; // com Ctrl apertado nao chega caractere
        match self.modo {
            Modo::NovaPasta | Modo::Renomear => return self.evento_campo_nome(e),
            Modo::Apagar => {
                match c {
                    '\r' => self.executar(Acao::Confirmar),
                    '\u{1b}' => self.cancelar_modo(),
                    _ => return false,
                }
                return true;
            }
            Modo::Normal => {}
        }
        if self.campo_caminho.focado() {
            return self.evento_campo_caminho(e);
        }
        match c {
            '\u{8}' => self.tentar(Acao::Cima),
            // o Enter abre aqui (a lista nem ve o '\r', para nao abrir duas vezes)
            '\r' => match self.local {
                Local::Pasta(_) => match self.lista.selecionado() {
                    Some(i) => {
                        self.abrir_item(i);
                        true
                    }
                    None => false,
                },
                Local::Discos => {
                    self.abrir_disco();
                    true
                }
            },
            '\u{1b}' => false,
            _ => self.pasta_atual().is_some() && self.evento_lista(e),
        }
    }

    fn tecla(&mut self, codigo: u16, pressionada: bool, e: EventoApp) -> bool {
        if !pressionada {
            return false;
        }
        match self.modo {
            Modo::NovaPasta | Modo::Renomear => return self.evento_campo_nome(e),
            Modo::Apagar => return false,
            Modo::Normal => {}
        }
        if self.campo_caminho.focado() {
            return self.evento_campo_caminho(e);
        }
        if self.ctrl {
            return match codigo {
                TECLA_C => self.tentar(Acao::Copiar),
                TECLA_X => self.tentar(Acao::Recortar),
                TECLA_V => self.tentar(Acao::Colar),
                _ => false,
            };
        }
        match codigo {
            KEY_DELETE => self.tentar(Acao::Apagar),
            TECLA_F2 => self.tentar(Acao::Renomear),
            TECLA_F5 => self.tentar(Acao::Atualizar),
            KEY_ENTER | KEY_KPENTER => false, // chega tambem como Caractere(13)
            _ if self.pasta_atual().is_some() => self.evento_lista(e),
            KEY_UP | KEY_DOWN => self.mover_disco(codigo == KEY_DOWN),
            _ => false,
        }
    }

    /// Entrega `e` a lista. true = precisa redesenhar.
    fn evento_lista(&mut self, e: EventoApp) -> bool {
        let antes = self.lista.selecionado();
        let area = self.area_lista();
        let acao = self.lista.evento(e, area);
        let mudou = antes != self.lista.selecionado();
        if mudou {
            self.mensagem = None; // a barra de estado passa a falar do item novo
        }
        match acao {
            AcaoLista::Abriu(i) => {
                self.abrir_item(i);
                true
            }
            AcaoLista::Nada => mudou,
            _ => true,
        }
    }

    /// Entrega `e` ao campo do caminho. true = precisa redesenhar.
    fn evento_campo_caminho(&mut self, e: EventoApp) -> bool {
        let area = self.area_caminho();
        let (acao, redesenhar) = evento_campo(&mut self.campo_caminho, e, area);
        match acao {
            AcaoCampo::Confirmou => {
                self.campo_caminho.focar(false);
                self.ir_para_digitado();
                true
            }
            AcaoCampo::Cancelou => {
                self.campo_caminho.focar(false);
                self.restaurar_caminho();
                true
            }
            _ => redesenhar,
        }
    }

    fn evento_campo_nome(&mut self, e: EventoApp) -> bool {
        let area = self.area_campo_nome();
        let (acao, redesenhar) = evento_campo(&mut self.campo_nome, e, area);
        match acao {
            AcaoCampo::Confirmou => self.confirmar_nome(),
            AcaoCampo::Cancelou => self.cancelar_modo(),
            _ => return redesenhar,
        }
        true
    }

    /// A lista so mostra a selecao forte quando o teclado e dela (nenhum
    /// campo de texto com o foco). true = mudou.
    fn acertar_foco_da_lista(&mut self) -> bool {
        let da_lista = !self.campo_caminho.focado() && !self.campo_nome.focado();
        let mudou = self.lista.focada() != da_lista;
        self.lista.focar(da_lista);
        mudou
    }

    /// Clique num cartao: seleciona; o segundo clique logo depois abre.
    fn clicar_disco(&mut self, x: i32, y: i32) -> bool {
        let Some(i) = (0..self.cartoes_visiveis()).find(|&i| self.retangulo_cartao(i).contem(x, y)) else {
            self.duplo.esquecer();
            return false;
        };
        self.disco = Some(i);
        if self.duplo.apertar(i) {
            self.abrir_disco();
        }
        true
    }

    /// Setas na vista dos discos.
    fn mover_disco(&mut self, descer: bool) -> bool {
        let total = self.cartoes_visiveis();
        let Some(atual) = self.disco else { return false };
        let novo = if descer { (atual + 1).min(total.saturating_sub(1)) } else { atual.saturating_sub(1) };
        self.disco = Some(novo);
        novo != atual
    }

    /// Tique: a tarefa pendente, ou de vez em quando confere se a pasta mudou.
    fn tique(&mut self) -> bool {
        if self.tarefa.is_some() {
            if !self.quadro_mostrado {
                return false;
            }
            if let Some(tarefa) = self.tarefa.take() {
                self.fazer_tarefa(tarefa);
            }
            return true;
        }
        self.tiques = self.tiques.wrapping_add(1);
        let ocupado = self.modo != Modo::Normal || self.campo_caminho.focado() || self.apertado.is_some();
        if !self.tiques.is_multiple_of(TIQUES_CONFERIR) || ocupado {
            return false;
        }
        self.conferir_mudancas()
    }

    /// Le a pasta (ou os discos) de novo e so refaz a lista se algo mudou.
    fn conferir_mudancas(&mut self) -> bool {
        let discos = armazenamento::discos();
        let livre = discos.iter().find(|d| d.montado).map(|d| d.livre);
        let mudou = livre != self.livre
            || match &self.local {
                Local::Discos => !mesmos_discos(&discos, &self.discos),
                Local::Pasta(caminho) => match armazenamento::listar(caminho) {
                    Ok(mut itens) => {
                        ordenar(&mut itens);
                        self.erro_pasta.is_some() || !mesmos_itens(&itens, &self.itens)
                    }
                    Err(_) => self.erro_pasta.is_none(),
                },
            };
        if mudou {
            let selecionado = self.item_selecionado().map(|i| i.nome.clone());
            self.recarregar(selecionado.as_deref());
            self.arvore.atualizar();
        }
        mudou
    }

    // ------------------------------------------------------- desenho

    fn desenhar_barras(&self, s: &mut Superficie, t: &Tema) {
        pintar_barra(s, Retangulo::novo(0, 0, LARGURA, ALTURA_BARRA), cor_barra(t), t);
        let fundo = match self.modo {
            Modo::Normal => t.fundo_janela,
            Modo::Apagar => grafico::misturar(t.fundo_janela, t.fechar, 36),
            _ => grafico::misturar(t.fundo_janela, t.destaque, 28),
        };
        pintar_barra(s, Retangulo::novo(0, TOPO_COMANDOS, LARGURA, ALTURA_BARRA), fundo, t);
        for b in &self.botoes() {
            let apertado = self.apertado == Some(b.acao);
            desenhar_botao(s, b, self.principal(b.acao), apertado, self.habilitada(b.acao), t);
        }
        self.campo_caminho.desenhar(s, self.area_caminho());
        self.desenhar_resto_comandos(s, t);
    }

    /// O que a barra de comandos mostra alem dos botoes.
    fn desenhar_resto_comandos(&self, s: &mut Superficie, t: &Tema) {
        let y = TOPO_COMANDOS + (ALTURA_BARRA - 8) / 2;
        match (&self.local, self.modo) {
            (Local::Discos, _) => {
                let x = MARGEM + largura_botao("Abrir") + 12;
                s.texto(x, y, "Clique duas vezes num disco montado para ver os arquivos", cor_texto_fraco(t), 1);
            }
            (_, Modo::Normal) => self.desenhar_transferencia(s, y, t),
            (_, Modo::Apagar) => {
                let escolhidos = self.escolhidos();
                let pergunta = match escolhidos.as_slice() {
                    [] => return,
                    [item] if item.pasta => format!("Apagar a pasta “{}” e tudo o que há nela?", item.nome),
                    [item] => format!("Apagar “{}”? Isso não pode ser desfeito.", item.nome),
                    varios => format!("Apagar os {} itens escolhidos? Isso não pode ser desfeito.", varios.len()),
                };
                let [ok, _] = botoes_dialogo(Acao::Confirmar, "Apagar", Acao::Cancelar, 0, LARGURA);
                let texto = fonte::cortar(&pergunta, ok.r.x - 2 * MARGEM - 16, &INTER);
                s.texto(MARGEM + 4, y, texto, t.texto, 1);
            }
            _ => {
                let base = INTER_NEGRITO.base_centralizada(TOPO_COMANDOS, ALTURA_BARRA);
                s.texto_com(MARGEM + 4, base, self.rotulo_nome(), t.texto, &INTER_NEGRITO);
                self.campo_nome.desenhar(s, self.area_campo_nome());
            }
        }
    }

    /// Lembrete do que esta na area de transferencia, a direita dos comandos.
    fn desenhar_transferencia(&self, s: &mut Superficie, y: i32, t: &Tema) {
        let copiados = TRANSFERENCIA.ler();
        let Some(copiado) = copiados.first() else { return };
        let verbo = if copiado.recortar { "Recortado" } else { "Copiado" };
        let texto = match copiados.len() {
            1 => format!("{verbo}: {}", nome_do_caminho(&copiado.caminho)),
            n => format!("{verbo}: {n} itens"),
        };
        let inicio = MARGEM + COMANDOS.iter().map(|&(_, texto)| largura_botao(texto) + ESPACO).sum::<i32>() + 8;
        let texto = fonte::cortar(&texto, LARGURA - MARGEM - 4 - inicio, &INTER);
        let largura = Superficie::largura_texto(texto, 1);
        s.texto(LARGURA - MARGEM - 4 - largura, y, texto, cor_texto_fraco(t), 1);
    }

    fn desenhar_pasta(&self, s: &mut Superficie, t: &Tema) {
        self.arvore.desenhar(s, self.area_arvore(), self.pasta_atual(), t);
        self.lista.desenhar(s, self.area_lista());
        let aviso = match self.erro_pasta {
            Some(e) => Some((format!("Não foi possível ler a pasta: {e}"), t.fechar)),
            None if self.itens.is_empty() => Some((String::from("Esta pasta está vazia"), cor_texto_fraco(t))),
            None => None,
        };
        if let Some((texto, cor)) = aviso {
            let area = self.area_conteudo();
            texto_centralizado(s, area, area.y + area.altura / 2 - 4, &texto, cor);
        }
    }

    fn desenhar_discos(&self, s: &mut Superficie, t: &Tema) {
        let area = self.area_conteudo();
        if self.discos.is_empty() {
            let y = area.y + area.altura / 2;
            texto_centralizado(s, area, y - 12, "Nenhum disco encontrado", t.texto);
            texto_centralizado(s, area, y + 4, "(no QEMU: -device virtio-blk-device)", cor_texto_fraco(t));
            return;
        }
        let visiveis = self.cartoes_visiveis();
        for (i, d) in self.discos.iter().enumerate().take(visiveis) {
            desenhar_cartao(s, self.retangulo_cartao(i), d, self.disco == Some(i), t);
        }
        if visiveis < self.discos.len() {
            let resto = format!("… e mais {}", contagem(self.discos.len() - visiveis, "disco", "discos"));
            s.texto(12, self.retangulo_cartao(visiveis).y, &resto, cor_texto_fraco(t), 1);
        }
    }

    fn desenhar_estado(&self, s: &mut Superficie, t: &Tema) {
        let y = barra_estado(s, t);
        let direita = match self.livre {
            Some(livre) => format!("{} livres", tamanho_texto(livre)),
            None => String::from("sem disco"),
        };
        let largura = Superficie::largura_texto(&direita, 1);
        s.texto(LARGURA - MARGEM - 4 - largura, y, &direita, cor_texto_fraco(t), 1);
        let (texto, cor) = match &self.mensagem {
            Some((texto, true)) => (texto.clone(), t.fechar),
            Some((texto, false)) => (texto.clone(), t.texto),
            None => (self.resumo(), t.texto),
        };
        s.texto(MARGEM + 4, y, fonte::cortar(&texto, LARGURA - largura - 3 * MARGEM - 24, &INTER), cor, 1);
    }

    /// Contagem da pasta (e o item selecionado) para a barra de estado.
    fn resumo(&self) -> String {
        if self.pasta_atual().is_none() {
            return contagem(self.discos.len(), "disco", "discos");
        }
        if self.erro_pasta.is_some() {
            return String::new(); // o erro ja aparece no meio da lista
        }
        let pastas = self.itens.iter().filter(|i| i.pasta).count();
        let bytes: u64 = self.itens.iter().map(|i| i.tamanho).sum();
        let mut texto = format!(
            "{}, {} ({})",
            contagem(pastas, "pasta", "pastas"),
            contagem(self.itens.len() - pastas, "arquivo", "arquivos"),
            tamanho_texto(bytes)
        );
        match self.escolhidos().as_slice() {
            [] => {}
            [item] => {
                let detalhe = if item.pasta { String::from("pasta") } else { tamanho_texto(item.tamanho) };
                texto.push_str(&format!("   |   {}: {detalhe}", item.nome));
            }
            varios => {
                let bytes: u64 = varios.iter().map(|i| i.tamanho).sum();
                texto.push_str(&format!("   |   {} itens escolhidos ({})", varios.len(), tamanho_texto(bytes)));
            }
        }
        texto
    }

    /// Aviso da tarefa que esta para rodar.
    fn texto_tarefa(&self) -> Option<String> {
        Some(match self.tarefa.as_ref()? {
            Tarefa::Colar(copiados) => {
                let verbo = if copiados.first().is_some_and(|c| c.recortar) { "Movendo" } else { "Copiando" };
                match copiados.as_slice() {
                    [c] => format!("{verbo} {}…", nome_do_caminho(&c.caminho)),
                    varios => format!("{verbo} {} itens…", varios.len()),
                }
            }
            Tarefa::Apagar(caminhos, _) => match caminhos.as_slice() {
                [um] => format!("Apagando {}…", nome_do_caminho(um)),
                varios => format!("Apagando {} itens…", varios.len()),
            },
        })
    }
}

impl App for Arquivos {
    fn titulo(&self) -> &str {
        &self.titulo
    }

    fn tamanho(&self) -> (usize, usize) {
        (LARGURA as usize, ALTURA as usize)
    }

    fn desenhar(&mut self, s: &mut Superficie) {
        let t = tema::atual();
        s.limpar(t.fundo_janela);
        self.desenhar_barras(s, &t);
        match self.local {
            Local::Pasta(_) => self.desenhar_pasta(s, &t),
            Local::Discos => self.desenhar_discos(s, &t),
        }
        if let Some(texto) = self.texto_tarefa() {
            desenhar_aviso(s, self.area_conteudo(), &texto, &t);
        }
        self.desenhar_estado(s, &t);
        self.quadro_mostrado = true;
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
            return false; // espera a tarefa (roda no proximo Tique)
        }
        let redesenhar = match e {
            EventoApp::Botao { x, y, botao: Botao::Esquerdo, pressionado: true } => self.apertar(x, y, e),
            EventoApp::Botao { x, y, botao: Botao::Esquerdo, pressionado: false } => self.soltar(x, y, e),
            // a lista guarda o ponteiro (a roda so rola com ele em cima) e
            // arrasta o polegar da barra de rolagem
            EventoApp::Roda(passos) if self.pasta_atual().is_some() && self.area_arvore().contem(self.mouse.0, self.mouse.1) => {
                let linhas = super::arvore::Arvore::linhas_visiveis(self.area_arvore());
                self.arvore.rolar(-(passos as isize) * 3, linhas)
            }
            EventoApp::Mouse { x, y, .. } => {
                self.mouse = (x, y);
                self.pasta_atual().is_some() && self.evento_lista(e)
            }
            EventoApp::Roda(_) => self.pasta_atual().is_some() && self.evento_lista(e),
            EventoApp::Caractere(c) => self.caractere(c, e),
            EventoApp::Tecla { codigo, pressionada } => self.tecla(codigo, pressionada, e),
            _ => false,
        };
        self.acertar_foco_da_lista() || redesenhar
    }

    fn pedidos(&mut self) -> Vec<Pedido> {
        core::mem::take(&mut self.pedidos)
    }
}

// ------------------------------------------------------- disco e nomes

/// Pastas primeiro, depois arquivos, por nome sem olhar maiusculas (como o "ls").
fn ordenar(itens: &mut [ItemPasta]) {
    itens.sort_by(|a, b| b.pasta.cmp(&a.pasta).then_with(|| comparar_nomes(&a.nome, &b.nome)));
}

/// Ordem alfabetica sem olhar maiusculas nem acentos ("Árvore" junto do
/// "arquivo"); empate (so os acentos diferem) desempata pelo texto.
fn comparar_nomes(a: &str, b: &str) -> Ordering {
    let chave = |t: &str| t.chars().map(letra_base).collect::<String>().to_lowercase();
    chave(a).cmp(&chave(b)).then_with(|| a.cmp(b))
}

/// A letra sem o acento (so Latin-1, o que o teclado digita).
fn letra_base(c: char) -> char {
    const BASE: &str = "AAAAAAACEEEEIIIIDNOOOOO×OUUUUYÞsaaaaaaaceeeeiiiidnooooo÷ouuuuyþy";
    match c as u32 {
        0xC0..=0xFF => BASE.chars().nth(c as usize - 0xC0).unwrap_or(c),
        _ => c,
    }
}

/// Mesmo nome sem olhar maiusculas (tambem as acentuadas: "Ação" = "AÇÃO").
fn mesmo_nome(a: &str, b: &str) -> bool {
    a.chars().flat_map(char::to_lowercase).eq(b.chars().flat_map(char::to_lowercase))
}

fn mesmos_itens(a: &[ItemPasta], b: &[ItemPasta]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.nome == y.nome && x.pasta == y.pasta && x.tamanho == y.tamanho)
}

fn mesmos_discos(a: &[InfoDisco], b: &[InfoDisco]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.nome == y.nome && x.livre == y.livre && x.montado == y.montado)
}

/// Uma linha da lista: icone, nome, tamanho (alinhado a direita) e tipo.
fn item_da_lista(item: &ItemPasta) -> ItemLista {
    let (tipo, icone) = tipo_do_item(item);
    let tamanho =
        if item.pasta { String::new() } else { alinhar_direita(&tamanho_texto(item.tamanho), LARGURA_TAMANHO) };
    ItemLista { icone: Some(icone), colunas: vec![item.nome.clone(), tamanho, tipo] }
}

/// `texto` com espacos na frente ate ocupar uns `largura` px na Inter (a
/// lista so escreve da esquerda: assim os tamanhos ficam alinhados a direita).
fn alinhar_direita(texto: &str, largura: i32) -> String {
    let espaco = INTER.glifo(' ').avanco.max(1);
    let n = ((largura - INTER.largura(texto)) / espaco).max(0) as usize;
    let mut alinhado = String::with_capacity(n + texto.len());
    alinhado.extend(core::iter::repeat_n(' ', n));
    alinhado.push_str(texto);
    alinhado
}

/// Nome do tipo (coluna "Tipo") e icone de um item.
fn tipo_do_item(item: &ItemPasta) -> (String, Icone) {
    if item.pasta {
        return (String::from("Pasta"), Icone::Pasta);
    }
    let extensao = extensao(&item.nome);
    for &(extensoes, nome, icone) in TIPOS {
        if extensoes.iter().any(|e| e.eq_ignore_ascii_case(extensao)) {
            return (String::from(nome), icone);
        }
    }
    if extensao.is_empty() {
        (String::from("Arquivo"), Icone::Arquivo)
    } else {
        (format!("Arquivo {}", extensao.to_ascii_uppercase()), Icone::Arquivo)
    }
}

/// Extensao sem o ponto ("" se nao tem; ".oculto" tambem nao tem).
fn extensao(nome: &str) -> &str {
    match nome.rfind('.') {
        Some(i) if i > 0 => &nome[i + 1..],
        _ => "",
    }
}

/// `nome` se ele ainda nao existe em `existentes`; senao "nome (2)",
/// "nome (3)"... com o numero antes da extensao ("notas (2).txt").
fn nome_livre(existentes: &[ItemPasta], nome: &str, pasta: bool) -> String {
    let existe = |n: &str| existentes.iter().any(|i| mesmo_nome(&i.nome, n));
    if !existe(nome) {
        return String::from(nome);
    }
    let corte = match nome.rfind('.') {
        Some(i) if i > 0 && !pasta => i,
        _ => nome.len(),
    };
    let (base, ext) = nome.split_at(corte);
    let mut n = 2;
    loop {
        let candidato = format!("{base} ({n}){ext}");
        if !existe(&candidato) {
            return candidato;
        }
        n += 1;
    }
}

/// `caminho` e a propria `pasta` ou fica dentro dela? (sem olhar maiusculas)
fn dentro_de(caminho: &str, pasta: &str) -> bool {
    let (c, p) = (caminho.as_bytes(), pasta.as_bytes());
    let comeca = c.len() >= p.len() && c[..p.len()].eq_ignore_ascii_case(p);
    comeca && (c.len() == p.len() || p == b"/" || c[p.len()] == b'/')
}

/// Copia um arquivo ou uma pasta inteira. Devolve quantos itens copiou.
fn copiar_tudo(de: &str, para: &str, profundidade: u32) -> Result<u32, &'static str> {
    match armazenamento::ler(de) {
        Ok(dados) => armazenamento::escrever(para, &dados).map(|()| 1).map_err(|e| e.descricao()),
        Err(Erro::EPasta) if profundidade < MAX_PROFUNDIDADE => {
            armazenamento::criar_pasta(para).map_err(|e| e.descricao())?;
            let mut total = 1;
            for item in armazenamento::listar(de).map_err(|e| e.descricao())? {
                total += copiar_tudo(&juntar(de, &item.nome), &juntar(para, &item.nome), profundidade + 1)?;
            }
            Ok(total)
        }
        Err(Erro::EPasta) => Err("pastas dentro de pastas demais"),
        Err(e) => Err(e.descricao()),
    }
}

/// Apaga um arquivo ou uma pasta com tudo dentro. Devolve quantos itens apagou.
fn apagar_tudo(caminho: &str, profundidade: u32) -> Result<u32, &'static str> {
    match armazenamento::apagar(caminho) {
        Ok(()) => Ok(1),
        Err(Erro::PastaNaoVazia) if profundidade < MAX_PROFUNDIDADE => {
            let mut total = 1;
            for item in armazenamento::listar(caminho).map_err(|e| e.descricao())? {
                total += apagar_tudo(&juntar(caminho, &item.nome), profundidade + 1)?;
            }
            armazenamento::apagar(caminho).map_err(|e| e.descricao())?;
            Ok(total)
        }
        Err(Erro::PastaNaoVazia) => Err("pastas dentro de pastas demais"),
        Err(e) => Err(e.descricao()),
    }
}

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

/// Cartao de um disco: icone, nome, sistema, tamanho e barra de uso.
fn desenhar_cartao(s: &mut Superficie, r: Retangulo, d: &InfoDisco, selecionado: bool, t: &Tema) {
    widgets::painel(s, r);
    if selecionado {
        s.contorno(r, t.destaque, 2);
    }
    let fundo = s.ler_pixel(r.x + 6, r.y + r.altura / 2).unwrap_or(t.fundo_janela);
    let forte = texto_sobre(fundo, t.texto);
    let fraco = grafico::misturar(forte, fundo, 100);
    widgets::icone(s, r.x + 16, r.y + (r.altura - 32) / 2, Icone::Disco, 2);
    let x = r.x + 64;
    let direita = r.direita() - 16;

    let estado = if d.montado { "montado em /" } else { "não montado" };
    let detalhe = format!("{}  ·  {}  ·  {}", d.sistema, tamanho_texto(d.setores * 512), estado);
    let largura = Superficie::largura_texto(&detalhe, 1);
    s.texto(direita - largura, r.y + 16, &detalhe, fraco, 1);
    let titulo = if d.rotulo.is_empty() { d.nome.clone() } else { format!("{} ({})", d.rotulo, d.nome) };
    s.texto(x, r.y + 12, fonte::cortar(&titulo, direita - largura - 16 - x, &fonte::INTER_TITULO), forte, 2);

    if d.sistema == "FAT32" && d.tamanho > 0 {
        let usado = d.tamanho.saturating_sub(d.livre);
        let milesimos = (usado * 1000 / d.tamanho) as u32;
        widgets::barra_progresso(s, Retangulo::novo(x, r.y + 38, direita - x, 12), milesimos);
        let legenda = format!(
            "{} usados de {} ({}%)  ·  {} livres",
            tamanho_texto(usado),
            tamanho_texto(d.tamanho),
            milesimos / 10,
            tamanho_texto(d.livre)
        );
        s.texto(x, r.y + 58, fonte::cortar(&legenda, direita - x, &INTER), fraco, 1);
    } else {
        s.texto(x, r.y + 46, "Sem sistema de arquivos conhecido (o MXOS lê FAT32)", fraco, 1);
    }
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
