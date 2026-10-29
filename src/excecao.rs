//! Tratador de excecoes: o equivalente ARM da "tela azul".
//! No MXOS x86 qualquer erro virava triple fault e a maquina reiniciava
//! sem explicar nada. Aqui o erro e descrito na serial e, com a area de
//! trabalho ligada, tambem numa tela de erro desenhada direto no
//! framebuffer (o panic usa a mesma tela).
//!
//! A tela de erro nao aloca e nao pega trava nenhuma (o erro pode ter
//! acontecido no meio do alocador): o texto passa por um buffer fixo na
//! pilha e e desenhado com os metodos da Tela. Os detalhes saem na fonte
//! monoespacada (as colunas do relatorio ficam alinhadas); o resto, na Inter.

use core::cell::UnsafeCell;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::cpu;
use crate::fonte::{INTER, INTER_TITULO};
use crate::mapa;
use crate::uart::Uart;
use crate::video::tela::{Cor, Escala, Tela};

const ORIGENS: [&str; 4] = [
    "kernel (EL1, pilha normal)",
    "kernel (EL1, pilha de exceção)",
    "programa (EL0, 64 bits)",
    "programa (EL0, 32 bits)",
];
const TIPOS: [&str; 4] = ["síncrona", "IRQ", "FIQ", "SError"];

fn motivo(classe: u64) -> &'static str {
    match classe {
        0x00 => "instrução desconhecida",
        0x01 => "WFI/WFE interceptado",
        0x07 => "FP/SIMD desligado",
        0x15 => "chamada de sistema (SVC)",
        0x16 => "chamada ao hypervisor (HVC)",
        0x17 => "chamada ao firmware (SMC)",
        0x18 => "acesso a registrador proibido",
        0x20 | 0x21 => "falha ao buscar instrução",
        0x22 => "PC desalinhado",
        0x24 | 0x25 => "falha de acesso à memória",
        0x26 => "pilha desalinhada",
        0x3C => "ponto de parada (BRK)",
        _ => "outro",
    }
}

/// O que a MMU achou de errado (campo FSC do ESR).
fn falha_mmu(codigo: u64) -> &'static str {
    match codigo >> 2 {
        0b0001 => "endereço sem mapeamento",
        0b0010 => "flag de acesso desligada",
        0b0011 => "permissão negada",
        _ => match codigo {
            0b100001 => "acesso desalinhado",
            0b010000 => "erro externo (barramento)",
            _ => "outra",
        },
    }
}

#[unsafe(no_mangle)]
extern "C" fn tratar_excecao(tipo: u64, esr: u64, elr: u64, far: u64) -> ! {
    let mut console = Uart::new();
    let _ = writeln!(console, "\n*** EXCEÇÃO DA CPU ***");
    let _ = descrever(&mut console, tipo, esr, elr, far);
    let _ = writeln!(console, "Sistema parado.");
    tela_de_erro("EXCEÇÃO DA CPU", |painel| descrever(painel, tipo, esr, elr, far));
    cpu::parar()
}

/// O relatorio da excecao, igual na serial e na tela de erro.
fn descrever(saida: &mut dyn Write, tipo: u64, esr: u64, elr: u64, far: u64) -> fmt::Result {
    let classe = (esr >> 26) & 0x3F;
    writeln!(saida, "  tipo    : {} vinda do {}", TIPOS[(tipo % 4) as usize], ORIGENS[(tipo / 4 % 4) as usize])?;
    writeln!(saida, "  motivo  : {} (classe 0x{classe:02x})", motivo(classe))?;

    if matches!(classe, 0x20 | 0x21 | 0x24 | 0x25) {
        let codigo = esr & 0x3F;
        let acao = match (classe, esr >> 6 & 1) {
            (0x20 | 0x21, _) => "executar",
            (_, 1) => "escrever",
            _ => "ler",
        };
        writeln!(saida, "  MMU     : {} (tabela nível {}) ao {acao}", falha_mmu(codigo), codigo & 3)?;
        let guarda = mapa::kernel().guarda as u64;
        if far >= guarda && far < guarda + 4096 {
            writeln!(saida, "  >>> ESTOURO DE PILHA: a pilha passou do limite e bateu na página de guarda")?;
        }
    }

    writeln!(saida, "  ESR_EL1 : 0x{esr:016x}")?;
    writeln!(saida, "  ELR_EL1 : 0x{elr:016x}  ← instrução que falhou")?;
    writeln!(saida, "  FAR_EL1 : 0x{far:016x}  ← endereço acessado")
}

// ------------------------------------------------------------ tela de erro

/// O framebuffer da tela de erro, registrado pela area de trabalho.
struct TelaDeErro {
    tela: UnsafeCell<Option<Tela>>,
    pronta: AtomicBool,
    /// Ja desenhando: um erro dentro da tela de erro nao desenha de novo.
    desenhando: AtomicBool,
}

// Seguranca: `tela` so e escrita antes de `pronta` virar true (Release) e
// so e lida depois de ver `pronta` (Acquire).
unsafe impl Sync for TelaDeErro {}

static TELA_DE_ERRO: TelaDeErro =
    TelaDeErro { tela: UnsafeCell::new(None), pronta: AtomicBool::new(false), desenhando: AtomicBool::new(false) };

const FUNDO: Cor = 0x0012_3C78;
const FAIXA: Cor = 0x00E0_4B43;
const TEXTO: Cor = 0x00FF_FFFF;
const TEXTO_FRACO: Cor = 0x00B4_C8EA;
const DESTAQUE: Cor = 0x00FF_D166;
const MARGEM: usize = 72;
const TOPO_DETALHES: usize = 250;
const ALTURA_LINHA: usize = 20;
/// Largura da celula da fonte monoespacada (texto dos detalhes).
const CELULA: usize = 8;
const MAX_COLUNAS: usize = 120;
/// Carinha triste ":(" em blocos de LADO_CARINHA pixels.
const CARINHA: [&[u8; 7]; 10] = [
    b"     ##",
    b"    ## ",
    b"##  #  ",
    b"## ##  ",
    b"   ##  ",
    b"   ##  ",
    b"## ##  ",
    b"##  #  ",
    b"    ## ",
    b"     ##",
];
const LADO_CARINHA: usize = 9;

/// A area de trabalho passa o framebuffer: dai em diante um erro fatal
/// tambem aparece na tela. Chamar uma vez, antes do primeiro quadro.
#[cfg_attr(feature = "mxq", allow(dead_code))]
pub fn registrar_tela(tela: Tela) {
    unsafe { *TELA_DE_ERRO.tela.get() = Some(tela) };
    TELA_DE_ERRO.pronta.store(true, Ordering::Release);
}

/// Desenha a tela de erro com o `titulo` e o que `detalhes` escrever
/// (uma linha por '\n'; linhas longas quebram). Sem framebuffer
/// registrado (modo serial, TV box) nao faz nada.
pub fn tela_de_erro(titulo: &str, detalhes: impl FnOnce(&mut dyn Write) -> fmt::Result) {
    if !TELA_DE_ERRO.pronta.load(Ordering::Acquire) || TELA_DE_ERRO.desenhando.swap(true, Ordering::AcqRel) {
        return;
    }
    cpu::desligar_interrupcoes(); // ninguem mais desenha por cima
    let Some(tela) = (unsafe { *TELA_DE_ERRO.tela.get() }) else { return };
    desenhar_moldura(&tela, titulo);
    let colunas = (tela.largura().saturating_sub(2 * MARGEM) / CELULA).clamp(1, MAX_COLUNAS);
    let limite = tela.altura().saturating_sub(80);
    let mut painel = Painel { tela, linha: [' '; MAX_COLUNAS], tamanho: 0, colunas, y: TOPO_DETALHES, limite };
    let _ = detalhes(&mut painel);
    if painel.tamanho > 0 {
        painel.nova_linha();
    }
}

/// Fundo, carinha, a frase de sempre, o titulo do erro e o rodape.
fn desenhar_moldura(tela: &Tela, titulo: &str) {
    let (largura, altura) = (tela.largura(), tela.altura());
    tela.limpar(FUNDO);
    tela.retangulo(0, 0, largura, 6, FAIXA);
    desenhar_carinha(tela, MARGEM, 56);
    tela.texto_com(MARGEM, 186, "O MXOS encontrou um erro e precisou parar.", TEXTO, &INTER_TITULO);
    tela.retangulo(MARGEM, 206, 4, 22, DESTAQUE);
    tela.texto_com(MARGEM + 14, 223, titulo, DESTAQUE, &INTER_TITULO);
    let rodape = "Os detalhes também foram enviados pela serial. Reinicie a máquina para continuar.";
    tela.texto_com(MARGEM, altura.saturating_sub(40) as i32, rodape, TEXTO_FRACO, &INTER);
}

/// A carinha ":(" grande, bloco a bloco (nao ha fonte desse tamanho).
fn desenhar_carinha(tela: &Tela, x: usize, y: usize) {
    for (linha, blocos) in CARINHA.iter().enumerate() {
        for (coluna, &b) in blocos.iter().enumerate() {
            if b == b'#' {
                let (px, py) = (x + coluna * LADO_CARINHA, y + linha * LADO_CARINHA);
                tela.retangulo(px, py, LADO_CARINHA, LADO_CARINHA, TEXTO);
            }
        }
    }
}

/// Escreve os detalhes na tela de erro (monoespacada), uma linha de cada
/// vez. Linha que passa da largura quebra no ultimo espaco (ou no meio da
/// palavra, se ela sozinha nao couber). Os espacos do comeco somem.
struct Painel {
    tela: Tela,
    linha: [char; MAX_COLUNAS],
    tamanho: usize,
    colunas: usize,
    y: usize,
    /// Abaixo daqui nao desenha mais (o rodape).
    limite: usize,
}

impl Painel {
    /// Desenha os primeiros `n` caracteres da linha e desce.
    fn desenhar(&mut self, n: usize) {
        if self.y + ALTURA_LINHA <= self.limite {
            for (i, &c) in self.linha[..n].iter().enumerate() {
                self.tela.caractere(MARGEM + i * CELULA, self.y, c, TEXTO, None, Escala::CONSOLE);
            }
        }
        self.y += ALTURA_LINHA;
    }

    fn nova_linha(&mut self) {
        self.desenhar(self.tamanho);
        self.tamanho = 0;
    }

    /// A linha encheu: desenha ate o ultimo espaco e passa o resto (sem os
    /// espacos) para a linha de baixo.
    fn quebrar(&mut self) {
        let corte = match self.linha[..self.tamanho].iter().rposition(|&c| c == ' ') {
            Some(espaco) if espaco > 0 => espaco,
            _ => self.tamanho,
        };
        self.desenhar(corte);
        let resto = self.linha[corte..self.tamanho].iter().position(|&c| c != ' ').map_or(self.tamanho, |i| corte + i);
        self.linha.copy_within(resto..self.tamanho, 0);
        self.tamanho -= resto;
    }
}

impl Write for Painel {
    fn write_str(&mut self, texto: &str) -> fmt::Result {
        for c in texto.chars() {
            match c {
                '\n' => self.nova_linha(),
                ' ' if self.tamanho == 0 => {}
                '\r' => {}
                _ => {
                    if self.tamanho == self.colunas {
                        self.quebrar();
                    }
                    if c == ' ' && self.tamanho == 0 {
                        continue;
                    }
                    self.linha[self.tamanho] = if c.is_control() { '?' } else { c };
                    self.tamanho += 1;
                }
            }
        }
        Ok(())
    }
}
