//! Configuracoes do MXOS, guardadas no disco em /config/mxos.cfg.
//!
//! O arquivo e texto simples, uma "chave=valor" por linha; linhas vazias
//! e as que comecam com '#' sao ignoradas. Exemplo:
//!
//!   # MXOS: configuracoes (chave=valor)
//!   tema=Azul
//!   papel=gradiente
//!   cor_papel=204060
//!   relogio_segundos=sim
//!   icones=sim
//!   layout=abnt2
//!   pagina_inicial=disco:/paginas/index.html
//!
//! Ler nunca falha: sem disco, sem arquivo ou com o arquivo estragado
//! valem os valores de fabrica (Config::padrao). Uma chave conhecida com
//! valor que nao faz sentido tambem volta para o de fabrica; chaves
//! desconhecidas sao mantidas (e gravadas de novo no salvar).
//!
//! So existe no QEMU (o disco e o virtio-blk).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::armazenamento::{self, Erro};
use crate::entrada::Layout;
use crate::gui::grafico::Cor;
use crate::gui::tema::{self, Aparencia, PapelDeParede};

const PASTA: &str = "/config";
const ARQUIVO: &str = "/config/mxos.cfg";
const CABECALHO: &str = "# MXOS: configuracoes (chave=valor)\n";
/// Um arquivo maior que isto nao e de configuracao: fica o padrao.
const TAMANHO_MAXIMO: usize = 16 * 1024;

/// Chaves conhecidas e os valores de fabrica, na ordem em que sao gravadas.
const PADROES: [(&str, &str); 7] = [
    ("tema", "Azul"),
    ("papel", "gradiente"),
    ("cor_papel", "204060"),
    ("relogio_segundos", "sim"),
    ("icones", "sim"),
    ("layout", "abnt2"),
    ("pagina_inicial", "disco:/paginas/index.html"),
];

pub struct Config {
    /// (chave, valor), as conhecidas primeiro, na ordem de PADROES.
    pares: Vec<(String, String)>,
    /// true = veio do /config/mxos.cfg (false = so os valores de fabrica).
    do_disco: bool,
}

impl Config {
    /// So os valores de fabrica.
    pub fn padrao() -> Config {
        let pares = PADROES.iter().map(|&(k, v)| (String::from(k), String::from(v))).collect();
        Config { pares, do_disco: false }
    }

    /// Le o /config/mxos.cfg por cima dos valores de fabrica. Nunca falha.
    pub fn carregar() -> Config {
        let mut cfg = Config::padrao();
        let Ok(bytes) = armazenamento::ler(ARQUIVO) else { return cfg };
        if bytes.len() > TAMANHO_MAXIMO {
            return cfg;
        }
        let Ok(texto) = core::str::from_utf8(&bytes) else { return cfg };
        for linha in texto.lines() {
            cfg.ler_linha(linha);
        }
        cfg.validar();
        cfg.do_disco = true;
        cfg
    }

    /// Grava tudo no /config/mxos.cfg (cria a pasta /config se precisar).
    pub fn salvar(&self) -> Result<(), &'static str> {
        let mut texto = String::from(CABECALHO);
        for (chave, valor) in &self.pares {
            texto.push_str(chave);
            texto.push('=');
            texto.push_str(valor);
            texto.push('\n');
        }
        match armazenamento::criar_pasta(PASTA) {
            Ok(()) | Err(Erro::JaExiste) => {}
            Err(e) => return Err(e.descricao()),
        }
        armazenamento::escrever(ARQUIVO, texto.as_bytes()).map_err(|e| e.descricao())
    }

    pub fn obter(&self, chave: &str) -> Option<&str> {
        let chave = chave.trim();
        self.pares.iter().find(|(k, _)| k.eq_ignore_ascii_case(chave)).map(|(_, v)| v.as_str())
    }

    /// Troca (ou acrescenta) uma chave. Quebras de linha no valor viram
    /// espaco; uma chave vazia ou com '=' / '#' e ignorada.
    pub fn definir(&mut self, chave: &str, valor: &str) {
        let chave = chave.trim();
        if chave.is_empty() || chave.contains(['=', '#', '\n', '\r']) {
            return;
        }
        let valor: String = valor.trim().chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
        match self.pares.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(chave)) {
            Some(par) => par.1 = valor,
            None => self.pares.push((chave.to_ascii_lowercase(), valor)),
        }
    }

    /// true = os valores vieram do disco (para a linha de estado do boot).
    pub fn do_disco(&self) -> bool {
        self.do_disco
    }

    /// Uma linha "chave=valor" do arquivo.
    fn ler_linha(&mut self, linha: &str) {
        let linha = linha.trim();
        if linha.is_empty() || linha.starts_with('#') {
            return;
        }
        if let Some((chave, valor)) = linha.split_once('=') {
            self.definir(chave, valor);
        }
    }

    /// Valor estragado numa chave conhecida volta para o de fabrica.
    fn validar(&mut self) {
        for &(chave, padrao) in &PADROES {
            if !self.obter(chave).is_some_and(|v| valor_valido(chave, v)) {
                self.definir(chave, padrao);
            }
        }
    }
}

/// O valor faz sentido para a chave? (chaves desconhecidas: sempre)
fn valor_valido(chave: &str, valor: &str) -> bool {
    match chave {
        "tema" => tema::por_nome(valor).is_some(),
        "papel" => PapelDeParede::por_nome(valor).is_some(),
        "cor_papel" => ler_cor(valor).is_some(),
        "relogio_segundos" | "icones" => ler_sim_nao(valor).is_some(),
        "layout" => ler_layout(valor).is_some(),
        _ => !valor.is_empty(),
    }
}

/// "204060", "#204060" ou "0x204060" -> 0x00204060.
pub fn ler_cor(valor: &str) -> Option<Cor> {
    let valor = valor.trim();
    let hex = valor.strip_prefix('#').or_else(|| valor.strip_prefix("0x")).or_else(|| valor.strip_prefix("0X")).unwrap_or(valor);
    if hex.is_empty() || hex.len() > 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(hex, 16).ok()
}

/// Cor -> texto do arquivo ("204060").
pub fn texto_cor(cor: Cor) -> String {
    format!("{:06x}", cor & 0x00FF_FFFF)
}

/// "sim"/"nao" (e tambem 1/0, s/n, true/false, on/off).
pub fn ler_sim_nao(valor: &str) -> Option<bool> {
    let valor = valor.trim();
    const SIM: [&str; 6] = ["sim", "s", "1", "true", "on", "ligado"];
    const NAO: [&str; 6] = ["nao", "n", "0", "false", "off", "desligado"];
    if SIM.iter().any(|s| s.eq_ignore_ascii_case(valor)) {
        Some(true)
    } else if NAO.iter().any(|s| s.eq_ignore_ascii_case(valor)) {
        Some(false)
    } else {
        None
    }
}

/// bool -> texto do arquivo.
pub fn texto_sim_nao(sim: bool) -> &'static str {
    if sim { "sim" } else { "nao" }
}

/// "us" ou "abnt2" (sem diferenciar maiusculas).
pub fn ler_layout(valor: &str) -> Option<Layout> {
    match valor.trim() {
        v if v.eq_ignore_ascii_case("us") => Some(Layout::Us),
        v if v.eq_ignore_ascii_case("abnt2") => Some(Layout::Abnt2),
        _ => None,
    }
}

/// Layout -> texto do arquivo.
pub fn texto_layout(layout: Layout) -> &'static str {
    match layout {
        Layout::Us => "us",
        Layout::Abnt2 => "abnt2",
    }
}

/// Aparencia descrita em `cfg` (o que faltar fica com o de fabrica).
pub fn aparencia_de(cfg: &Config) -> Aparencia {
    let padrao = tema::APARENCIA_PADRAO;
    let sim_nao = |chave, padrao| cfg.obter(chave).and_then(ler_sim_nao).unwrap_or(padrao);
    Aparencia {
        papel: cfg.obter("papel").and_then(PapelDeParede::por_nome).unwrap_or(padrao.papel),
        cor_papel: cfg.obter("cor_papel").and_then(ler_cor).unwrap_or(padrao.cor_papel),
        relogio_segundos: sim_nao("relogio_segundos", padrao.relogio_segundos),
        icones: sim_nao("icones", padrao.icones),
    }
}

/// Poe em uso o tema e a aparencia de `cfg` (tema::definir e
/// tema::definir_aparencia). Quem desenha precisa redesenhar tudo depois.
pub fn aplicar_aparencia(cfg: &Config) {
    let padrao = tema::TEMAS[0].1;
    tema::definir(cfg.obter("tema").and_then(tema::por_nome).unwrap_or(padrao));
    tema::definir_aparencia(aparencia_de(cfg));
}

/// O contrario de aplicar_aparencia: copia o tema e a aparencia em uso
/// para `cfg` (para gravar com salvar).
pub fn guardar_aparencia(cfg: &mut Config) {
    if let Some(nome) = tema::nome_atual() {
        cfg.definir("tema", nome);
    }
    let a = tema::aparencia();
    cfg.definir("papel", a.papel.nome());
    cfg.definir("cor_papel", &texto_cor(a.cor_papel));
    cfg.definir("relogio_segundos", texto_sim_nao(a.relogio_segundos));
    cfg.definir("icones", texto_sim_nao(a.icones));
}

/// Layout do teclado de `cfg`: "us" ou "abnt2" (padrao).
pub fn layout(cfg: &Config) -> Layout {
    cfg.obter("layout").and_then(ler_layout).unwrap_or(Layout::Abnt2)
}
