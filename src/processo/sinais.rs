//! Sinais com tratador do programa, como no Linux aarch64.
//!
//! Quando um sinal tratado chega, ele fica pendente. Antes de o processo
//! voltar a rodar, `entregar` monta na pilha dele o rt_sigframe do Linux
//! (siginfo + ucontext com os registradores e o FPSIMD) e desvia o PC
//! para o tratador, com x30 apontando para o restaurador. O restaurador
//! (o da libc, ou o nosso trampolim) chama rt_sigreturn, que le o quadro
//! de volta.
//!
//! Chamada que estava esperando (read do terminal, wait4, nanosleep...)
//! e interrompida: volta -EINTR, ou e repetida depois do tratador se o
//! sigaction pediu SA_RESTART.

use super::Tabela;
use super::arquivos::{EFAULT, EINTR, EINVAL};
use super::memoria::Permissao;
use super::TOPO_PILHA;

pub const SIG_DFL: u64 = 0;
pub const SIG_IGN: u64 = 1;
const SA_RESTORER: u64 = 0x0400_0000;
const SA_RESTART: u64 = 0x1000_0000;
const SA_NODEFER: u64 = 0x4000_0000;
const SA_RESETHAND: u64 = 0x8000_0000;

/// SIGKILL e SIGSTOP nao podem ser bloqueados.
const NUNCA_BLOQUEADOS: u64 = bit(9) | bit(19);

/// O trampolim fica na pagina acima da pilha (que ninguem usa):
/// mov x8, #139 (rt_sigreturn); svc #0.
const TRAMPOLIM: usize = TOPO_PILHA;
const CODIGO_TRAMPOLIM: [u32; 2] = [0xD280_1168, 0xD400_0001];

// Layout do rt_sigframe do Linux arm64.
const SIGINFO: usize = 0;
const UCONTEXT: usize = 128;
const UC_SIGMASK: usize = UCONTEXT + 40;
const SIGCONTEXT: usize = UCONTEXT + 176;
const REGS: usize = SIGCONTEXT + 8;
const SP: usize = REGS + 31 * 8;
const PC: usize = SP + 8;
const PSTATE: usize = PC + 8;
const RESERVADO: usize = SIGCONTEXT + 288;
const FPSIMD_MAGIC: u32 = 0x4650_8001;
const FPSIMD_TAMANHO: usize = 528;
const QUADRO: usize = RESERVADO + 4096;

const _: () = assert!(QUADRO % 16 == 0);

pub const fn bit(sinal: usize) -> u64 {
    1 << (sinal - 1)
}

/// O sigaction de um sinal.
#[derive(Clone, Copy)]
pub struct Acao {
    pub tratador: u64,
    pub flags: u64,
    pub restaurador: u64,
    pub mascara: u64,
}

impl Acao {
    pub const PADRAO: Acao = Acao { tratador: SIG_DFL, flags: 0, restaurador: 0, mascara: 0 };

    /// O programa tem codigo para este sinal.
    pub fn tem_tratador(&self) -> bool {
        self.tratador > SIG_IGN
    }
}

/// Acao padrao de um sinal sem tratador: true = termina o processo.
pub fn padrao_termina(sinal: usize) -> bool {
    // SIGCHLD, SIGCONT, SIGSTOP, SIGTSTP, SIGTTIN, SIGTTOU, SIGURG, SIGWINCH
    !matches!(sinal, 0 | 17..=23 | 28)
}

fn escrever_u64(quadro: &mut [u8], posicao: usize, valor: u64) {
    quadro[posicao..posicao + 8].copy_from_slice(&valor.to_le_bytes());
}

fn ler_u64(quadro: &[u8], posicao: usize) -> u64 {
    u64::from_le_bytes(quadro[posicao..posicao + 8].try_into().unwrap())
}

/// Poe o trampolim no espaco do processo, se ainda nao estiver la.
fn trampolim(t: &mut Tabela, i: usize) -> Result<u64, ()> {
    let espaco = &mut t.processos[i].espaco;
    if !espaco.mapeada(TRAMPOLIM) {
        espaco.mapear(TRAMPOLIM, Permissao::Codigo).map_err(|_| ())?;
        let mut codigo = [0u8; 8];
        codigo[..4].copy_from_slice(&CODIGO_TRAMPOLIM[0].to_le_bytes());
        codigo[4..].copy_from_slice(&CODIGO_TRAMPOLIM[1].to_le_bytes());
        espaco.escrever_forcado(TRAMPOLIM, &codigo).map_err(|_| ())?;
    }
    Ok(TRAMPOLIM as u64)
}

/// Entrega o primeiro sinal pendente e nao bloqueado do processo `i`, se
/// houver. Devolve true se o processo morreu por causa dele.
pub fn entregar(t: &mut Tabela, i: usize) -> bool {
    let l = &t.processos[i].linux;
    let prontos = l.pendentes & !l.mascara;
    if prontos == 0 {
        return false;
    }
    let sinal = prontos.trailing_zeros() as usize + 1;
    t.processos[i].linux.pendentes &= !bit(sinal);
    let acao = t.processos[i].linux.acoes[sinal];
    if !acao.tem_tratador() {
        // o tratador foi tirado enquanto o sinal esperava
        if acao.tratador == SIG_DFL && padrao_termina(sinal) {
            t.terminar(i, sinal as i32);
            return true;
        }
        return false;
    }

    // chamada interrompida: -EINTR, ou repete depois (SA_RESTART)
    let p = &mut t.processos[i];
    let suspensa = p.linux.mascara_suspensa.take();
    if let Some(numero) = p.linux.na_chamada.take() {
        p.linux.futex = None;
        // poll, select, nanosleep e sigsuspend nunca sao repetidos
        let repete = acao.flags & SA_RESTART != 0 && !matches!(numero, 22 | 72 | 73 | 101 | 115 | 133);
        if !repete || suspensa.is_some() {
            p.linux.acordar_em = None;
            p.contexto.x[0] = (-EINTR) as u64;
            p.contexto.pc += 4;
        }
    }
    let mascara_antes = suspensa.unwrap_or(p.linux.mascara);

    let restaurador = if acao.flags & SA_RESTORER != 0 && acao.restaurador != 0 {
        acao.restaurador
    } else {
        match trampolim(t, i) {
            Ok(r) => r,
            Err(()) => {
                t.terminar(i, 11);
                return true;
            }
        }
    };

    let p = &mut t.processos[i];
    let c = &p.contexto;
    let mut quadro = alloc::vec![0u8; QUADRO];
    // siginfo: numero, errno, codigo (SI_USER = 0)
    quadro[SIGINFO..SIGINFO + 4].copy_from_slice(&(sinal as u32).to_le_bytes());
    escrever_u64(&mut quadro, UC_SIGMASK, mascara_antes);
    for (k, x) in c.x.iter().enumerate() {
        escrever_u64(&mut quadro, REGS + k * 8, *x);
    }
    escrever_u64(&mut quadro, SP, c.sp);
    escrever_u64(&mut quadro, PC, c.pc);
    escrever_u64(&mut quadro, PSTATE, c.spsr);
    quadro[RESERVADO..RESERVADO + 4].copy_from_slice(&FPSIMD_MAGIC.to_le_bytes());
    quadro[RESERVADO + 4..RESERVADO + 8].copy_from_slice(&(FPSIMD_TAMANHO as u32).to_le_bytes());
    quadro[RESERVADO + 8..RESERVADO + 12].copy_from_slice(&(c.fpsr as u32).to_le_bytes());
    quadro[RESERVADO + 12..RESERVADO + 16].copy_from_slice(&(c.fpcr as u32).to_le_bytes());
    for (k, q) in c.q.iter().enumerate() {
        let posicao = RESERVADO + 16 + k * 16;
        quadro[posicao..posicao + 16].copy_from_slice(&q.to_le_bytes());
    }
    let sp = (c.sp as usize).wrapping_sub(QUADRO) & !15;
    if p.espaco.escrever(sp, &quadro).is_err() {
        // pilha estragada: o Linux mata com SIGSEGV
        t.terminar(i, 11);
        return true;
    }

    let c = &mut p.contexto;
    c.x[0] = sinal as u64;
    c.x[1] = (sp + SIGINFO) as u64;
    c.x[2] = (sp + UCONTEXT) as u64;
    c.x[29] = sp as u64;
    c.x[30] = restaurador;
    c.sp = sp as u64;
    c.pc = acao.tratador;
    let l = &mut p.linux;
    l.mascara |= acao.mascara;
    if acao.flags & SA_NODEFER == 0 {
        l.mascara |= bit(sinal);
    }
    l.mascara &= !NUNCA_BLOQUEADOS;
    if acao.flags & SA_RESETHAND != 0 {
        l.acoes[sinal] = Acao::PADRAO;
    }
    p.estado = super::Estado::Pronto;
    false
}

/// rt_sigreturn: os registradores e a mascara voltam do quadro.
pub fn sigreturn(t: &mut Tabela, i: usize) -> Result<(), i64> {
    let p = &mut t.processos[i];
    let sp = p.contexto.sp as usize;
    let mut quadro = alloc::vec![0u8; QUADRO];
    p.espaco.ler(sp, &mut quadro).map_err(|_| EFAULT)?;
    let c = &mut p.contexto;
    for k in 0..31 {
        c.x[k] = ler_u64(&quadro, REGS + k * 8);
    }
    c.sp = ler_u64(&quadro, SP);
    c.pc = ler_u64(&quadro, PC);
    // so as flags NZCV: o programa nao escolhe o nivel nem as mascaras
    c.spsr = ler_u64(&quadro, PSTATE) & 0xF000_0000;
    if u32::from_le_bytes(quadro[RESERVADO..RESERVADO + 4].try_into().unwrap()) == FPSIMD_MAGIC {
        c.fpsr = u32::from_le_bytes(quadro[RESERVADO + 8..RESERVADO + 12].try_into().unwrap()) as u64;
        c.fpcr = u32::from_le_bytes(quadro[RESERVADO + 12..RESERVADO + 16].try_into().unwrap()) as u64;
        for k in 0..32 {
            let posicao = RESERVADO + 16 + k * 16;
            c.q[k] = u128::from_le_bytes(quadro[posicao..posicao + 16].try_into().unwrap());
        }
    }
    p.linux.mascara = ler_u64(&quadro, UC_SIGMASK) & !NUNCA_BLOQUEADOS;
    Ok(())
}

/// rt_sigprocmask(como, &novo, &velho).
pub fn procmask(t: &mut Tabela, i: usize, como: u64, novo: usize, velho: usize) -> Result<i64, i64> {
    let atual = t.processos[i].linux.mascara;
    let pedido = if novo != 0 {
        let mut b = [0u8; 8];
        t.processos[i].espaco.ler(novo, &mut b).map_err(|_| EFAULT)?;
        Some(u64::from_le_bytes(b))
    } else {
        None
    };
    if velho != 0 {
        t.processos[i].espaco.escrever(velho, &atual.to_le_bytes()).map_err(|_| EFAULT)?;
    }
    if let Some(s) = pedido {
        let nova = match como {
            0 => atual | s,
            1 => atual & !s,
            2 => s,
            _ => return Err(EINVAL),
        };
        t.processos[i].linux.mascara = nova & !NUNCA_BLOQUEADOS;
    }
    Ok(0)
}
