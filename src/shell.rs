//! Shell do MXOS: recebe uma tecla de cada vez, monta a linha e executa o
//! comando no Enter. Quem entrega as teclas e o laco da serial (`rodar`) ou
//! a janela do terminal da area de trabalho (gui::apps::terminal).

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::{self, Write};
use core::hint::black_box;
use core::ptr;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::psci::{self, Via};
use crate::terminal::Terminal;
use crate::uart;
use crate::{cpu, gic, heap, irq, mapa, mmu, plataforma, timer};

const VERSAO: &str = concat!(env!("CARGO_PKG_VERSION_MAJOR"), ".", env!("CARGO_PKG_VERSION_MINOR"));
pub const PROMPT: &str = "mxos> ";
/// Bytes da linha (UTF-8: um acento ocupa dois).
const TAM_LINHA: usize = 256;
const ESPERA_MAXIMA_MS: u64 = 60_000;

macro_rules! diga {
    ($shell:expr, $($arg:tt)*) => {{ let _ = writeln!($shell.console, $($arg)*); }};
}

macro_rules! escreva {
    ($shell:expr, $($arg:tt)*) => {{ let _ = write!($shell.console, $($arg)*); }};
}

/// Mostra um tamanho em bytes, KB ou MB.
struct Bytes(usize);

impl fmt::Display for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let texto = match self.0 {
            b if b >= 10 << 20 => format!("{} MB", b >> 20),
            b if b >= 1 << 10 => format!("{} KB", b >> 10),
            b => format!("{b} bytes"),
        };
        f.pad(&texto) // respeita alinhamento, ex.: {:>8}
    }
}

/// O shell e dono do terminal: assim pode morar dentro de uma janela.
pub struct Shell {
    console: Terminal,
    el_boot: u64,
    psci: Via,
    /// Linha sendo digitada, em UTF-8 (so caracteres imprimiveis inteiros).
    linha: [u8; TAM_LINHA],
    tamanho: usize,
    /// Comeco de um caractere UTF-8 que ainda nao chegou inteiro.
    pendente: [u8; 4],
    pendentes: usize,
    ultimo_foi_cr: bool,
    /// Processo em primeiro plano: o prompt so volta quando ele termina.
    esperando: Option<u32>,
    /// 1 = o shell principal (serial); os paineis do terminal com abas
    /// ganham 2, 3...
    id: u32,
}

impl Drop for Shell {
    /// Painel fechado com um programa rodando: o tty fica livre.
    fn drop(&mut self) {
        let _ = DONO.compare_exchange(self.id, 0, Ordering::Relaxed, Ordering::Relaxed);
    }
}

/// Proximo id de shell.
static PROXIMO_ID: AtomicU32 = AtomicU32::new(1);
/// O shell com um programa em primeiro plano (0 = nenhum): so ele le a
/// saida e manda teclas para os processos (o tty e um so).
static DONO: AtomicU32 = AtomicU32::new(0);
/// Nivel de excecao do boot (para os shells extras escolherem o PSCI).
static EL_BOOT: AtomicU32 = AtomicU32::new(1);

impl Shell {
    pub fn nova(terminal: Terminal, el_boot: u64) -> Shell {
        // Quem entrega o kernel em EL2 (U-Boot, QEMU com virtualization=on)
        // atende o PSCI por SMC; o QEMU em EL1 atende por HVC.
        let psci = if cfg!(any(feature = "mxq", feature = "rk322x")) || el_boot >= 2 { Via::Smc } else { Via::Hvc };
        // as configuracoes reiniciam e desligam sem passar pelo shell
        psci::definir_via(psci);
        anotar_video(&terminal);
        EL_BOOT.store(el_boot as u32, Ordering::Relaxed);
        Shell::montar(terminal, el_boot, psci)
    }

    /// Shell extra so de tela (um painel do terminal com abas).
    #[cfg_attr(feature = "mxq", allow(dead_code))]
    pub fn painel(console: crate::video::tela::Console) -> Shell {
        let el_boot = EL_BOOT.load(Ordering::Relaxed) as u64;
        let psci = if cfg!(any(feature = "mxq", feature = "rk322x")) || el_boot >= 2 { Via::Smc } else { Via::Hvc };
        Shell::montar(Terminal::so_console(console), el_boot, psci)
    }

    /// Shell da depuracao remota: a saida fica no Terminal::captura.
    pub fn remoto() -> Shell {
        let el_boot = EL_BOOT.load(Ordering::Relaxed) as u64;
        let psci = if cfg!(any(feature = "mxq", feature = "rk322x")) || el_boot >= 2 { Via::Smc } else { Via::Hvc };
        Shell::montar(Terminal::captura(), el_boot, psci)
    }

    /// Roda uma linha de comando (a depuracao remota).
    pub fn rodar_linha(&mut self, linha: &str) {
        self.executar(linha.trim());
    }

    fn montar(terminal: Terminal, el_boot: u64, psci: Via) -> Shell {
        Shell {
            console: terminal,
            el_boot,
            psci,
            linha: [0; TAM_LINHA],
            tamanho: 0,
            pendente: [0; 4],
            pendentes: 0,
            ultimo_foi_cr: false,
            esperando: None,
            id: PROXIMO_ID.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// Outro shell esta com um programa em primeiro plano?
    fn outro_e_dono(&self) -> bool {
        let dono = DONO.load(Ordering::Relaxed);
        dono != 0 && dono != self.id
    }

    /// Comecou um programa em primeiro plano: a saida e as teclas sao deste shell.
    fn tomar_tty(&mut self, pid: u32) {
        self.esperando = Some(pid);
        DONO.store(self.id, Ordering::Relaxed);
    }

    #[cfg_attr(feature = "mxq", allow(dead_code))]
    pub fn terminal(&mut self) -> &mut Terminal {
        &mut self.console
    }

    /// Um processo em primeiro plano esta com o teclado.
    #[cfg_attr(feature = "mxq", allow(dead_code))]
    pub fn ocupado(&self) -> bool {
        self.esperando.is_some()
    }

    /// O que ja foi digitado na linha atual (para redesenhar o prompt).
    #[cfg_attr(feature = "mxq", allow(dead_code))]
    pub fn linha_atual(&self) -> &str {
        // so guardamos caracteres UTF-8 inteiros, entao a conversao nunca falha
        core::str::from_utf8(&self.linha[..self.tamanho]).unwrap_or("")
    }

    /// Modo so serial: banner e depois uma tecla de cada vez, para sempre.
    /// Entre uma tecla e outra a CPU dorme; a rede e atendida a cada acordada.
    pub fn rodar(mut self) -> ! {
        self.banner();
        loop {
            self.atender_processos();
            crate::console_boot::alimentar();
            if processos::tem_prontos() {
                // com processo para rodar a CPU nao dorme: so olha a serial
                processar_fundo();
                if let Some(byte) = self.console.tentar_receber() {
                    self.tecla(byte);
                }
                continue;
            }
            // o tique de 100 Hz acorda (um processo dormindo pode ter de acordar)
            let tique = timer::tiques();
            let byte = irq::esperar(|| {
                processar_fundo();
                match self.console.tentar_receber() {
                    Some(byte) => Some(Some(byte)),
                    None => (timer::tiques() != tique).then_some(None),
                }
            });
            if let Some(byte) = byte {
                self.tecla(byte);
            }
        }
    }

    /// Roda os processos um pouco, mostra o que eles escreveram e, quando o
    /// do primeiro plano termina, devolve o prompt. Devolve true se mudou
    /// alguma coisa na tela.
    pub fn atender_processos(&mut self) -> bool {
        processos::rodar();
        // sem programa em primeiro plano, a saida (de quem roda no fundo) e do principal
        let dono = DONO.load(Ordering::Relaxed);
        if (dono == 0 && self.id != 1) || (dono != 0 && dono != self.id) {
            return false;
        }
        let saida = processos::tirar_saida();
        let mut mudou = !saida.is_empty();
        if mudou {
            // "ESC[6n": o programa pergunta onde esta o cursor (o shell do
            // BusyBox faz isso antes do prompt); a resposta vai como teclas
            let pergunta = b"\x1b[6n";
            let mut resto: &[u8] = &saida;
            while let Some(p) = resto.windows(pergunta.len()).position(|j| j == pergunta) {
                escreva!(self, "{}", String::from_utf8_lossy(&resto[..p]));
                let (coluna, linha) = self.console.posicao().unwrap_or((0, 0));
                for byte in format!("\x1b[{};{}R", linha + 1, coluna + 1).bytes() {
                    processos::tecla(byte);
                }
                resto = &resto[p + pergunta.len()..];
            }
            escreva!(self, "{}", String::from_utf8_lossy(resto));
        }
        if let Some(pid) = self.esperando {
            if let Some(codigo) = processos::terminou(pid) {
                self.esperando = None;
                DONO.store(0, Ordering::Relaxed);
                if codigo != 0 {
                    diga!(self, "[processo {pid} saiu com o código {codigo}]");
                }
                escreva!(self, "{PROMPT}");
                mudou = true;
            }
        }
        mudou
    }

    /// Abertura de um painel do terminal com abas.
    #[cfg_attr(feature = "mxq", allow(dead_code))]
    pub fn banner_curto(&mut self) {
        diga!(self, "MXOS {VERSAO} — shell {} (\"ajuda\" lista os comandos)", self.id);
        escreva!(self, "{PROMPT}");
    }

    pub fn banner(&mut self) {
        diga!(self, "");
        diga!(self, "MXOS {VERSAO} — {}", crate::plataforma::DESCRICAO_KERNEL.to_lowercase());
        diga!(
            self,
            "Plataforma: {}  |  MMU ligada  |  heap de {}  |  timer a {} Hz",
            plataforma::NOME,
            Bytes(heap::estatisticas().total),
            timer::HZ
        );
        match self.console.video() {
            Ok(v) => diga!(self, "Vídeo: {}x{} ({}), este terminal também aparece na tela", v.largura, v.altura, v.nome),
            Err(motivo) => diga!(self, "Vídeo: desligado ({motivo})"),
        }
        diga!(self, "Digite \"help\" (ou \"ajuda\") para ver os comandos.");
        diga!(self, "");
        escreva!(self, "{PROMPT}");
    }

    /// Edicao de linha: imprimivel (ASCII ou UTF-8, um byte de cada vez),
    /// backspace (8 ou 127, apaga o caractere inteiro) e Enter (\r, \n ou
    /// \r\n, que conta uma vez so). No Enter executa e mostra o prompt.
    pub fn tecla(&mut self, byte: u8) {
        if self.esperando.is_some() {
            // o teclado e do processo em primeiro plano
            processos::tecla(byte);
            return;
        }
        // terminais mandam \r, \n ou \r\n no Enter: \r\n conta como um so
        if byte == b'\n' && self.ultimo_foi_cr {
            self.ultimo_foi_cr = false;
            return;
        }
        self.ultimo_foi_cr = byte == b'\r';

        if byte < 0x80 {
            // um ASCII no meio de um UTF-8 descarta o caractere quebrado
            self.pendentes = 0;
        }
        match byte {
            b'\r' | b'\n' => self.enter(),
            // backspace (8) ou delete (127), depende do terminal
            8 | 127 => self.apagar_caractere(),
            32..=126 => self.acrescentar(&[byte]),
            0x80.. => self.juntar_utf8(byte),
            _ => {}
        }
    }

    /// Junta os bytes de um caractere UTF-8; quando ele fecha (e e valido e
    /// imprimivel), vai para a linha.
    fn juntar_utf8(&mut self, byte: u8) {
        if byte >= 0xC0 {
            self.pendente[0] = byte;
            self.pendentes = 1;
            return;
        }
        // continuacao sem comeco: lixo
        if self.pendentes == 0 || self.pendentes >= 4 {
            self.pendentes = 0;
            return;
        }
        self.pendente[self.pendentes] = byte;
        self.pendentes += 1;
        let precisa = match self.pendente[0] {
            0xC0..=0xDF => 2,
            0xE0..=0xEF => 3,
            _ => 4,
        };
        if self.pendentes < precisa {
            return;
        }
        let bytes = self.pendente;
        let tamanho = core::mem::take(&mut self.pendentes);
        let valido = core::str::from_utf8(&bytes[..tamanho]).is_ok_and(|t| !t.chars().any(char::is_control));
        if valido {
            self.acrescentar(&bytes[..tamanho]);
        }
    }

    /// Poe um caractere inteiro (seus bytes UTF-8) na linha e faz o eco.
    fn acrescentar(&mut self, bytes: &[u8]) {
        let fim = self.tamanho + bytes.len();
        if fim > TAM_LINHA {
            return;
        }
        self.linha[self.tamanho..fim].copy_from_slice(bytes);
        self.tamanho = fim;
        for &byte in bytes {
            self.console.enviar(byte);
        }
    }

    /// Tira o ultimo caractere (todos os bytes dele) e apaga uma celula.
    fn apagar_caractere(&mut self) {
        if self.tamanho == 0 {
            return;
        }
        self.tamanho -= 1;
        // volta pelos bytes de continuacao (10xxxxxx) ate o primeiro do caractere
        while self.tamanho > 0 && self.linha[self.tamanho] & 0xC0 == 0x80 {
            self.tamanho -= 1;
        }
        escreva!(self, "\x08 \x08");
    }

    fn enter(&mut self) {
        diga!(self, "");
        // copia: o comando pode usar o shell inteiro enquanto le a linha
        let linha = self.linha;
        let tamanho = core::mem::take(&mut self.tamanho);
        let texto = core::str::from_utf8(&linha[..tamanho]).unwrap_or("");
        self.executar(texto.trim());
        if self.esperando.is_none() {
            escreva!(self, "{PROMPT}");
        }
    }

    fn executar(&mut self, linha: &str) {
        let (comando, argumentos) = match linha.split_once(' ') {
            Some((comando, resto)) => (comando, resto.trim_start()),
            None => (linha, ""),
        };

        match comando {
            "" => {}
            "help" | "ajuda" => self.ajuda(),
            "echo" => diga!(self, "{argumentos}"),
            "info" => self.info(),
            "uptime" => self.uptime(),
            "config" => self.config(argumentos),
            "cpu" => self.frequencia(argumentos),
            "papel" => self.papel(argumentos.trim()),
            #[cfg(any(feature = "mxq", feature = "rk322x"))]
            "usb" => {
                let linhas = crate::usb::relatorio();
                if linhas.is_empty() {
                    diga!(self, "usb: nada encontrado no boot");
                }
                for l in linhas {
                    diga!(self, "{l}");
                }
                diga!(self, "(o USB e lido so no boot: pendrive ligado depois nao aparece)");
            }
            "nas" => match argumentos.trim() {
                "parar" => {
                    crate::nas::parar();
                    diga!(self, "NAS parado");
                }
                "ligar" => diga!(self, "{}", crate::nas::iniciar()),
                _ if crate::nas::ligado() => diga!(self, "NAS ligado na porta {}, {} cliente(s)  (nas parar | nas ligar)", crate::nas::PORTA, crate::nas::clientes()),
                _ => diga!(self, "NAS desligado  (config nas_senha <senha>, depois nas ligar)"),
            },
            "mem" => self.memoria(),
            "mmu" => self.mmu(),
            "traduzir" => self.traduzir(argumentos),
            "heap" => self.testar_heap(),
            "irq" => self.irq(),
            "esperar" => self.esperar(argumentos),
            "video" => self.video(),
            "som" => self.som(),
            "clear" | "limpar" => self.console.limpar(),
            "erro" => self.erro(argumentos),
            "processo" => self.processo(argumentos),
            "executar" | "exec" => self.executar_programa(argumentos),
            "reboot" | "reiniciar" => {
                diga!(self, "Reiniciando…");
                psci::chamar(self.psci, psci::REINICIAR);
                diga!(self, "O firmware não reiniciou.");
            }
            "desligar" | "sair" => {
                diga!(self, "Desligando…");
                psci::chamar(self.psci, psci::DESLIGAR);
                diga!(self, "O firmware não desligou.");
            }
            "halt" | "parar" => {
                diga!(self, "MXOS parado. Pode desligar a energia.");
                cpu::parar();
            }
            _ => self.de_modulo(comando, argumentos),
        }
    }

    /// Disco, rede e navegador (so no QEMU) tem os proprios comandos: cada
    /// um diz se atendeu. So depois disso o comando e dado como desconhecido.
    fn de_modulo(&mut self, comando: &str, argumentos: &str) {
        if !comando_de_modulo(&mut self.console, comando, argumentos) {
            diga!(self, "comando não encontrado: {comando}  (use \"help\")");
        }
    }

    fn ajuda(&mut self) {
        diga!(self, "Comandos:");
        diga!(self, "  help          mostra esta lista");
        diga!(self, "  echo T        imprime o texto T");
        diga!(self, "  info          dados do hardware e do kernel");
        diga!(self, "  uptime        tempo desde que ligou");
        diga!(self, "  cpu [perfil]  frequência, overclock, temperatura e GPU (TV box)");
        diga!(self, "  mem           mapa de memória do kernel e uso do heap");
        diga!(self, "  mmu           estado da MMU e das tabelas de páginas");
        diga!(self, "  traduzir E    mostra como a MMU traduz o endereço E (hex)");
        diga!(self, "  heap          testa Vec, String e Box no alocador");
        diga!(self, "  irq           contadores de interrupções e tempo de CPU ociosa");
        diga!(self, "  esperar MS    dorme MS milissegundos no WFI (acorda pelo timer)");
        diga!(self, "  video         liga o vídeo (ramfb no QEMU) e mostra o estado");
        diga!(self, "  som           toca uma melodia (virtio-sound, só no QEMU)");
        self.ajuda_modulos();
        diga!(self, "  clear         limpa a tela");
        diga!(self, "  erro [tipo]   dispara uma exceção: brk, pilha, codigo, nulo");
        diga!(self, "  reboot        reinicia (PSCI)");
        diga!(self, "  desligar      desliga (PSCI)");
        diga!(self, "  halt          congela a CPU");
    }

    fn ajuda_modulos(&mut self) {
        diga!(self, "  ls [C]        lista a pasta C do disco (sem C: a raiz)");
        diga!(self, "  cat A         mostra o arquivo A");
        diga!(self, "  escrever A T  grava o texto T no arquivo A (cria ou troca)");
        diga!(self, "  mkdir P       cria a pasta P");
        diga!(self, "  rm C          apaga um arquivo ou uma pasta vazia");
        diga!(self, "  mv DE PARA    renomeia ou move (caminho com espaço vai entre aspas)");
        diga!(self, "  df            espaço usado e livre no disco");
        diga!(self, "  discos        lista os discos (virtio-blk, FAT32)");
        diga!(self, "  ip            configuração da rede (virtio-net, DHCP)");
        diga!(self, "  ping H        mede o tempo de ida e volta até o host H");
        diga!(self, "  dns N         resolve o nome N para um IP");
        diga!(self, "  http U        baixa a URL U (só http://) e mostra o começo");
        diga!(self, "  navegar U     abre a página U em modo texto (http:// ou disco:/)");
        diga!(self, "  drivers       dispositivos achados e o driver de cada um");
        diga!(self, "  tarefas       janelas abertas, com o tempo de CPU e a memória de cada uma");
        diga!(self, "  executar P [A] roda o programa P do disco (Linux aarch64 estático); & no fim = no fundo");
        diga!(self, "  processo M [&] programa de teste em EL0: ola, conta, falha, eco (& = no fundo)");
        diga!(self, "  processos     lista os processos (pid, estado, CPU, memória)");
        diga!(self, "  matar PID     finaliza um processo");
        diga!(self, "  syscalls      chamadas de sistema pedidas que o MXOS ainda não tem");
    }


    fn info(&mut self) {
        let midr = cpu::midr();
        let freq = cpu::frequencia_timer();
        let k = mapa::kernel();
        let psci = psci::chamar(self.psci, psci::VERSAO);

        diga!(self, "MXOS {VERSAO}");
        diga!(self, "  plataforma   : {}", plataforma::NOME);
        diga!(self, "  cpu          : {} (MIDR 0x{midr:08x})", cpu::nome_cpu(midr));
        diga!(self, "  privilégio   : entregue em EL{}, rodando em EL{}", self.el_boot, cpu::el_atual());
        diga!(self, "  timer        : {}.{} MHz", freq / 1_000_000, freq / 100_000 % 10);
        diga!(self, "  interrupções : GICv2, {} linhas, timer a {} Hz", gic::linhas(), timer::HZ);
        diga!(self, "  serial       : {}", uart::NOME);
        diga!(self, "  kernel       : 0x{:08x} - 0x{:08x} ({} com as pilhas)", k.inicio, k.fim, Bytes(k.fim - k.inicio));
        diga!(self, "  psci         : v{}.{} via {}", psci >> 16 & 0xFFFF, psci & 0xFFFF, self.psci.nome());
        match self.console.video() {
            Ok(v) => diga!(self, "  vídeo        : {}x{}, {}", v.largura, v.altura, v.nome),
            Err(motivo) => diga!(self, "  vídeo        : desligado ({motivo})"),
        }
    }

    fn video(&mut self) {
        let ligado = self.console.ligar_video();
        anotar_video(&self.console);
        match ligado {
            Ok(v) => {
                diga!(self, "vídeo ligado: {}x{}, {}", v.largura, v.altura, v.nome);
                diga!(self, "  framebuffer : 0x{:08x} ({} no heap)", v.base, Bytes(v.largura * v.altura * 4));
                diga!(self, "  console     : texto 8x16 (JetBrains Mono), tudo que sai na serial sai na tela");
            }
            Err(motivo) => diga!(self, "vídeo desligado: {motivo}"),
        }
    }

    fn som(&mut self) {
        if let Err(e) = crate::som::tocar_melodia(&mut self.console) {
            diga!(self, "som: {}", e.descricao());
        }
    }


    /// config            -> lista tudo
    /// config chave       -> mostra um valor
    /// config chave valor -> grava em /config/mxos.cfg
    fn config(&mut self, argumentos: &str) {
        let mut cfg = crate::config::Config::carregar();
        let (chave, valor) = match argumentos.trim().split_once(char::is_whitespace) {
            Some((c, v)) => (c, Some(v.trim())),
            None => (argumentos.trim(), None),
        };
        match (chave, valor) {
            ("", _) => {
                for (k, v) in cfg.pares() {
                    // chaves de API nao aparecem inteiras
                    let mostrado = if k.contains("chave") && v.len() > 8 { alloc::format!("{}…", &v[..8]) } else { String::from(v) };
                    diga!(self, "{k} = {mostrado}");
                }
            }
            (k, None) => match cfg.obter(k) {
                Some(v) => diga!(self, "{k} = {v}"),
                None => diga!(self, "{k}: não definido"),
            },
            (k, Some(v)) => {
                cfg.definir(k, v);
                match cfg.salvar() {
                    Ok(()) => diga!(self, "{k} gravado"),
                    Err(e) => diga!(self, "config: não gravou ({e})"),
                }
            }
        }
    }

    /// cpu                       -> estado e perfis
    /// cpu PERFIL [fixo]         -> stock, fabrica, turbo, extreme
    /// cpu MHZ [MV] [fixo]       -> personalizado (passa do recomendado)
    /// cpu limite GRAUS [fixo]   -> throttling (padrao 85)
    /// cpu gpu turbo|normal [fixo]
    /// "fixo" grava na config e vale nos proximos boots.
    fn frequencia(&mut self, argumentos: &str) {
        use crate::frequencia as f;
        if !f::suportado() {
            diga!(self, "cpu: DVFS e overclock só na TV box RK322x");
            return;
        }
        let partes: Vec<&str> = argumentos.split_whitespace().collect();
        let fixo = partes.last() == Some(&"fixo");
        let partes = &partes[..partes.len() - fixo as usize];
        let mut gravar: Vec<(&str, String)> = Vec::new();
        match partes {
            [] => {
                diga!(self, "CPU: {}", f::descricao());
                diga!(self, "GPU: {}   limite: {} °C (volta a {} MHz)", f::descricao_gpu(), f::limite_c(), f::FRIO.mhz);
                for p in f::PERFIS {
                    diga!(self, "  {:<8} {:>5} MHz  {},{:03} V  {}", p.nome, p.mhz, p.uv / 1_000_000, p.uv / 1000 % 1000, p.nota);
                }
                diga!(self, "uso: cpu <perfil> | cpu <MHz> [mV] | cpu limite <°C> | cpu gpu turbo|normal   (+ fixo)");
                return;
            }
            ["limite", graus] => {
                match graus.parse::<i32>().map_err(|_| "número inválido").and_then(f::definir_limite) {
                    Ok(()) => diga!(self, "throttling a {} °C", f::limite_c()),
                    Err(e) => return diga!(self, "cpu: {e}"),
                }
                gravar.push(("cpu_limite", format!("{}", f::limite_c())));
            }
            ["gpu", modo @ ("turbo" | "normal")] => {
                let turbo = *modo == "turbo";
                match f::gpu_turbo(turbo) {
                    Ok(mhz) => diga!(self, "GPU a {mhz} MHz"),
                    Err(e) => return diga!(self, "gpu: {e}"),
                }
                gravar.push(("gpu_turbo", String::from(if turbo { "sim" } else { "nao" })));
            }
            [nome] if f::perfil(nome).is_some() => {
                let p = f::perfil(nome).unwrap();
                if let Err(e) = f::definir(p.mhz, Some(p.uv)) {
                    return diga!(self, "cpu: {e}");
                }
                gravar.push(("cpu_perfil", String::from(p.nome)));
            }
            [mhz] | [mhz, _] => {
                let Ok(m) = mhz.parse::<u32>() else {
                    return diga!(self, "cpu: perfil ou MHz desconhecido: {mhz}");
                };
                let mv = match partes.get(1).map(|v| v.parse::<u32>()) {
                    Some(Ok(mv)) => Some(mv),
                    Some(Err(_)) => return diga!(self, "cpu: tensão em mV, ex.: cpu 1392 1400"),
                    None => None,
                };
                if let Err(e) = f::definir(m, mv.map(|mv| mv * 1000)) {
                    return diga!(self, "cpu: {e}");
                }
                gravar.push(("cpu_perfil", String::from("personalizado")));
                gravar.push(("cpu_mhz", format!("{}", m / 24 * 24)));
                gravar.push(("cpu_mv", format!("{}", mv.unwrap_or(f::tensao_para(m) / 1000))));
            }
            _ => return diga!(self, "uso: cpu <perfil> | cpu <MHz> [mV] | cpu limite <°C> | cpu gpu turbo|normal   (+ fixo)"),
        }
        diga!(self, "CPU: {}", f::descricao());
        if f::mhz() > f::RECOMENDADO_MHZ {
            diga!(self, "Atenção: acima do recomendado ({} MHz). Use dissipador; se travar, reinicie (o fixo só vale se gravado).", f::RECOMENDADO_MHZ);
        }
        if fixo {
            let mut cfg = crate::config::Config::carregar();
            for (k, v) in &gravar {
                cfg.definir(k, v);
            }
            match cfg.salvar() {
                Ok(()) => diga!(self, "gravado em /config/mxos.cfg"),
                Err(e) => diga!(self, "config: não gravou ({e})"),
            }
        }
    }

    /// papel ARQUIVO -> imagem de fundo (JPEG, PNG, GIF, BMP) e grava
    /// papel nenhum  -> volta ao papel desenhado
    fn papel(&mut self, caminho: &str) {
        if caminho.is_empty() {
            diga!(self, "uso: papel <arquivo de imagem> | papel nenhum   (ou abra a imagem e aperte P)");
            return;
        }
        let nenhum = caminho == "nenhum";
        match crate::gui::area::definir_imagem_papel((!nenhum).then_some(caminho)) {
            Ok(()) => {
                let mut cfg = crate::config::Config::carregar();
                cfg.definir("papel_imagem", if nenhum { "" } else { caminho });
                let _ = cfg.salvar();
                diga!(self, "papel de parede {}", if nenhum { "desenhado de volta" } else { "trocado" });
            }
            Err(e) => diga!(self, "papel: {e}"),
        }
    }

    fn uptime(&mut self) {
        let freq = cpu::frequencia_timer();
        let ticks = cpu::contador();
        let segundos = ticks / freq;
        let milissegundos = ticks % freq * 1000 / freq;
        diga!(
            self,
            "ligado há {}h {:02}m {:02}.{:03}s",
            segundos / 3600,
            segundos / 60 % 60,
            segundos % 60,
            milissegundos
        );
    }

    fn irq(&mut self) {
        let c = irq::contadores();
        // em milesimos, para mostrar uma casa decimal sem ponto flutuante
        let ocioso = if c.desde_inicio == 0 { 0 } else { c.ocioso * 1000 / c.desde_inicio };
        diga!(self, "GICv2 com {} linhas, distribuidor em 0x{:08x}", gic::linhas(), plataforma::GIC_DISTRIBUIDOR);
        diga!(self, "  INTID  origem         vezes");
        diga!(self, "  {:>5}  timer     {:>10}", plataforma::IRQ_TIMER, c.timer);
        diga!(self, "  {:>5}  serial    {:>10}  ({} bytes recebidos)", plataforma::IRQ_SERIAL, c.serial, c.bytes_serial);
        diga!(self, "      -  outras    {:>10}", c.outras);
        diga!(self, "      -  espúrias  {:>10}", c.espurias);
        diga!(
            self,
            "CPU ociosa (dormindo no WFI): {}.{}% do tempo desde que as interrupções ligaram",
            ocioso / 10,
            ocioso % 10
        );
    }

    fn esperar(&mut self, argumento: &str) {
        // sem Ctrl+C ainda: um limite evita travar o shell sem querer
        let Some(ms) = argumento.parse::<u64>().ok().filter(|&ms| ms <= ESPERA_MAXIMA_MS) else {
            diga!(self, "uso: esperar <ms>   (0 a {ESPERA_MAXIMA_MS})  ex.: esperar 1000");
            return;
        };
        let freq = cpu::frequencia_timer();
        let (contador, tiques) = (cpu::contador(), timer::tiques());
        timer::esperar_ms(ms);
        let micros = (cpu::contador() - contador) * 1_000_000 / freq;
        let passaram = timer::tiques() - tiques;
        diga!(self, "pediu {ms} ms: passaram {}.{:03} ms de verdade e {passaram} tiques do timer", micros / 1000, micros % 1000);
    }

    fn regiao(&mut self, inicio: usize, fim: usize, nome: &str, permissao: &str) {
        diga!(self, "  0x{inicio:08x} - 0x{fim:08x}  {nome:<17} {:>8}  {permissao}", Bytes(fim - inicio));
    }

    fn memoria(&mut self) {
        let k = mapa::kernel();
        let h = heap::estatisticas();
        diga!(self, "Kernel:");
        self.regiao(k.inicio, k.codigo_fim, "código", "lê e executa");
        self.regiao(k.codigo_fim, k.constantes_fim, "constantes", "só lê");
        self.regiao(k.constantes_fim, k.guarda, "dados e BSS", "lê e escreve");
        self.regiao(k.guarda, k.pilha_inicio, "página de guarda", "sem mapeamento");
        self.regiao(k.pilha_inicio, k.pilha_fim, "pilha", "lê e escreve");
        self.regiao(k.pilha_fim, k.pilha_excecao_fim, "pilha de exceção", "lê e escreve");
        diga!(self, "Heap:");
        self.regiao(h.inicio, h.inicio + h.total, "heap", "lê e escreve");
        diga!(
            self,
            "  em uso: {} em {} alocações | livre: {} em {} bloco(s), maior de {}",
            Bytes(h.total - h.livre),
            h.alocacoes,
            Bytes(h.livre),
            h.blocos_livres,
            Bytes(h.maior_bloco)
        );
        {
            let (arquivos, kb) = crate::processo::cache::resumo();
            diga!(self, "  cache de arquivos: {} em {} arquivo(s) (dividido pelos programas)", Bytes(kb * 1024), arquivos);
        }
    }

    fn mmu(&mut self) {
        let e = mmu::estado();
        let sim = |ligado: bool| if ligado { "ligado" } else { "desligado" };
        let mmu = if e.mmu { "ligada" } else { "desligada" };
        diga!(self, "MMU {mmu} | cache de dados {} | cache de instruções {}", sim(e.cache_dados), sim(e.cache_instrucoes));
        diga!(self, "  endereços virtuais : {} bits, páginas de 4 KB, 3 níveis de tabela", e.bits_virtuais);
        diga!(self, "  endereços físicos  : {} bits", e.bits_fisicos);
        diga!(self, "  tabela raiz        : 0x{:08x} (TTBR0_EL1)", e.raiz);
        diga!(self, "  tabelas usadas     : {} de {} (4 KB cada)", e.tabelas, mmu::MAX_TABELAS);
        diga!(self, "  mapa da {}:", plataforma::NOME);
        for &(inicio, fim) in plataforma::DISPOSITIVOS {
            diga!(self, "    0x{inicio:09x} - 0x{fim:09x}  periféricos (sem cache, nunca executa)");
        }
        diga!(self, "    0x{:09x} - 0x{:09x}  RAM (com cache)", plataforma::RAM_INICIO, plataforma::RAM_FIM);
        diga!(self, "  o resto (inclusive o endereço 0) fica sem mapeamento");
    }

    fn traduzir(&mut self, argumento: &str) {
        let numero = argumento.trim_start_matches("0x").trim_start_matches("0X");
        let Ok(virtual_) = u64::from_str_radix(numero, 16) else {
            diga!(self, "uso: traduzir <endereço em hex>   ex.: traduzir 0x09000000");
            return;
        };
        match mmu::traduzir(virtual_) {
            None => diga!(self, "0x{virtual_:x} → sem mapeamento (acessar gera exceção)"),
            Some(t) => {
                let tipo = match t.tipo_memoria {
                    mmu::MAIR_NORMAL => "RAM com cache",
                    mmu::MAIR_DISPOSITIVO => "periférico",
                    _ => "outro",
                };
                let escrita = if t.escrita { "lê e escreve" } else { "só lê" };
                diga!(self, "0x{virtual_:x} → físico 0x{:x} | {tipo} | {escrita}", t.fisico);
            }
        }
    }

    fn testar_heap(&mut self) {
        let antes = heap::estatisticas();
        {
            let mut numeros: Vec<u64> = (1..=10_000).collect();
            let soma: u64 = numeros.iter().sum();
            numeros.retain(|n| n % 7 == 0);
            let mut nomes: Vec<String> = Vec::new();
            for i in 1..=5 {
                nomes.push(format!("processo-{i}"));
            }
            let buffer = Box::new([0xABu8; 4096]);

            diga!(self, "Vec<u64>    : soma de 1 a 10000 = {soma}; {} múltiplos de 7", numeros.len());
            diga!(self, "Vec<String> : {}", nomes.join(", "));
            diga!(self, "Box<[u8]>   : 4 KB em 0x{:x}, primeiro byte 0x{:X}", buffer.as_ptr() as usize, buffer[0]);
            let durante = heap::estatisticas();
            diga!(self, "durante     : {} em uso em {} alocações", Bytes(durante.total - durante.livre), durante.alocacoes);
        }
        let depois = heap::estatisticas();
        let devolvido = depois.livre == antes.livre && depois.blocos_livres == antes.blocos_livres;
        diga!(
            self,
            "depois      : {} em uso, {} bloco(s) livre(s) → {}",
            Bytes(depois.total - depois.livre),
            depois.blocos_livres,
            if devolvido { "tudo devolvido e juntado" } else { "VAZAMENTO DE MEMÓRIA" }
        );
    }

    /// "processo ola" (primeiro plano) ou "processo conta &" (fundo).
    fn processo(&mut self, argumentos: &str) {
        let (modo, fundo) = match argumentos.strip_suffix('&') {
            Some(resto) => (resto.trim(), true),
            None => (argumentos.trim(), false),
        };
        if !fundo && self.outro_e_dono() {
            diga!(self, "processo: outro painel já está com um programa em primeiro plano");
            return;
        }
        match processos::criar_teste(modo, !fundo) {
            Ok(pid) if fundo => diga!(self, "[{pid}] rodando no fundo"),
            Ok(pid) => self.tomar_tty(pid),
            Err(motivo) => diga!(self, "processo: {motivo}  (modos: ola, conta, falha, eco; & no fim roda no fundo)"),
        }
    }

    /// "executar /bin/ola um dois" (primeiro plano) ou "... &" (fundo).
    /// Aspas juntam um argumento com espacos.
    fn executar_programa(&mut self, argumentos: &str) {
        let (linha, fundo) = match argumentos.trim_end().strip_suffix('&') {
            Some(resto) => (resto, true),
            None => (argumentos, false),
        };
        let partes = separar_argumentos(linha);
        let Some((programa, resto)) = partes.split_first() else {
            diga!(self, "uso: executar <programa> [argumentos] [&]   ex.: executar /bin/ola um dois");
            return;
        };
        let resto: Vec<&str> = resto.iter().map(String::as_str).collect();
        if !fundo && self.outro_e_dono() {
            diga!(self, "executar: outro painel já está com um programa em primeiro plano");
            return;
        }
        match processos::executar(programa, &resto, !fundo) {
            Ok(pid) if fundo => diga!(self, "[{pid}] rodando no fundo"),
            Ok(pid) => self.tomar_tty(pid),
            Err(motivo) => diga!(self, "executar: {motivo}"),
        }
    }

    fn erro(&mut self, tipo: &str) {
        match tipo {
            "" | "brk" => {
                diga!(self, "Disparando um ponto de parada (brk)…");
                cpu::ponto_de_parada();
            }
            "pilha" => {
                diga!(self, "Chamando uma função recursiva até estourar a pilha…");
                recursao(0);
            }
            "codigo" | "código" => {
                diga!(self, "Tentando escrever por cima do código do kernel…");
                let codigo = cpu::ponto_de_parada as *const () as *mut u32;
                unsafe { ptr::write_volatile(black_box(codigo), 0) };
            }
            "nulo" => {
                diga!(self, "Lendo o endereço 0 (ponteiro nulo)…");
                let nulo = black_box(0usize) as *const u64;
                let valor = unsafe { ptr::read_volatile(nulo) };
                diga!(self, "leu {valor}?!");
            }
            _ => diga!(self, "tipos: brk, pilha, codigo, nulo"),
        }
    }
}

/// O que roda cada vez que a CPU acorda esperando tecla no modo serial.
fn processar_fundo() {
    crate::rede::processar();
    crate::nas::atender();
    crate::depuracao::atender();
}


fn comando_de_modulo(saida: &mut Terminal, nome: &str, argumentos: &str) -> bool {
    crate::armazenamento::comando(nome, argumentos, &mut *saida)
        || crate::processo::comando(nome, argumentos, &mut *saida)
        || crate::rede::comando(nome, argumentos, &mut *saida)
        || crate::navegador::comando(nome, argumentos, &mut *saida)
        || crate::drivers::comando(nome, argumentos, &mut *saida)
        || crate::atualizacao::comando(nome, argumentos, &mut *saida)
}


mod processos {
    pub use crate::processo::{criar_teste, executar, rodar, tecla, tem_prontos, terminou, tirar_saida};
}


/// Separa os argumentos por espacos; "entre aspas" ou 'entre apostrofos'
/// fica junto (como no sh).
fn separar_argumentos(linha: &str) -> Vec<String> {
    let mut partes = Vec::new();
    let mut atual = String::new();
    let mut aspas: Option<char> = None;
    let mut tem = false;
    for c in linha.chars() {
        match c {
            '"' | '\'' if aspas.is_none() => {
                aspas = Some(c);
                tem = true;
            }
            c if aspas == Some(c) => aspas = None,
            ' ' if aspas.is_none() => {
                if tem {
                    partes.push(core::mem::take(&mut atual));
                    tem = false;
                }
            }
            c => {
                atual.push(c);
                tem = true;
            }
        }
    }
    if tem {
        partes.push(atual);
    }
    partes
}

/// O painel de drivers (so no QEMU) mostra o video que o terminal ligou.
#[cfg(not(any(feature = "mxq", feature = "rk322x")))]
fn anotar_video(terminal: &Terminal) {
    crate::drivers::definir_video(terminal.video().ok());
}

#[cfg(any(feature = "mxq", feature = "rk322x"))]
fn anotar_video(_terminal: &Terminal) {}

/// Cada chamada ocupa ~1 KB de pilha; com 64 KB, logo bate na pagina de guarda.
#[inline(never)]
fn recursao(nivel: u64) -> u64 {
    let espaco = black_box([nivel; 128]);
    if nivel == u64::MAX {
        return 0;
    }
    recursao(nivel + 1) + espaco[7]
}
