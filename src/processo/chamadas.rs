//! Chamadas de sistema, com os numeros e as regras do Linux aarch64:
//! numero em x8, argumentos em x0-x5, resposta em x0 (erro = -errno).
//! Assim um programa de Linux estatico (musl, glibc, BusyBox) roda sem
//! mudar nada.
//!
//! O que existe: arquivos e pastas do FAT32 (open, read, write, lseek,
//! stat, getdents64, mkdir, unlink, rename...), /dev/null e cia, tubos,
//! dup, fcntl, o terminal (ioctl de termios e tamanho da janela, poll),
//! memoria (brk, mmap anonimo e de arquivo, munmap, mprotect), processos
//! (clone/fork, execve, wait4, exit, kill), tempo (clock_gettime,
//! nanosleep, gettimeofday) e o que a libc pede no comeco (uname,
//! set_tid_address, prlimit64, getrandom, rt_sigaction...).
//!
//! Chamada que o MXOS ainda nao tem devolve -ENOSYS e fica anotada (o
//! comando "syscalls" mostra quais faltam). Sinais com tratador do
//! programa nao sao entregues (nao ha quadro de sinal ainda): o padrao
//! (terminar) vale, e o que tem tratador e ignorado.

use alloc::borrow::ToOwned;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use super::arquivos::{self, *};
use super::memoria::{Espaco, PAGINA, Permissao};
use super::{elf, Contexto, Estado, Tabela, TAMANHO_PILHA, TOPO_PILHA};
use crate::{armazenamento, cpu, psci};

/// Descritores por processo.
const MAX_FDS: usize = 256;
/// O mmap procura espaco a partir daqui (o brk cresce de baixo).
const BASE_MMAP: usize = 0x0300_0000;
/// O brk nao passa daqui.
const LIMITE_BRK: usize = BASE_MMAP;
const AT_FDCWD: i64 = -100;
const AT_REMOVEDIR: u64 = 0x200;
/// Numero de sinais (o 0 nao e usado).
pub const SINAIS: usize = 65;
const SIG_DFL: u64 = 0;
const SIG_IGN: u64 = 1;

/// Um descritor: o arquivo aberto e o FD_CLOEXEC.
#[derive(Clone)]
pub struct Fd {
    pub aberto: Aberto,
    pub fecha_no_exec: bool,
}

/// O que cada processo guarda para as chamadas do Linux.
pub struct Linux {
    /// Hora de acordar de um nanosleep ou de um ppoll com prazo em
    /// andamento (a chamada e repetida quando o processo acorda).
    acordar_em: Option<u64>,
    pub fds: Vec<Option<Fd>>,
    pub pasta: String,
    pub brk_inicio: usize,
    pub brk: usize,
    /// Tratador de cada sinal (SIG_DFL, SIG_IGN ou um endereco).
    pub sinais: [u64; SINAIS],
    /// Grupo de processos (job control do shell).
    pub grupo: u32,
    pub umask: u32,
    /// Pai parado num vfork ate este processo trocar de programa ou sair.
    pub pai_vfork: Option<u32>,
}

impl Linux {
    pub fn novo() -> Linux {
        Linux {
            acordar_em: None,
            fds: Vec::new(),
            pasta: String::from("/"),
            brk_inicio: 0,
            brk: 0,
            sinais: [SIG_DFL; SINAIS],
            grupo: 0,
            umask: 0o022,
            pai_vfork: None,
        }
    }

    /// stdin, stdout e stderr no terminal.
    pub fn com_terminal() -> Linux {
        let mut l = Linux::novo();
        let terminal = Descricao::nova(Tipo::Terminal, 2);
        for _ in 0..3 {
            l.fds.push(Some(Fd { aberto: terminal.clone(), fecha_no_exec: false }));
        }
        l
    }

    /// Copia para um filho (fork): mesmos arquivos abertos, mesma pasta.
    pub fn duplicar(&self) -> Linux {
        Linux {
            acordar_em: None,
            fds: self.fds.clone(),
            pasta: self.pasta.clone(),
            brk_inicio: self.brk_inicio,
            brk: self.brk,
            sinais: self.sinais,
            grupo: self.grupo,
            umask: self.umask,
            pai_vfork: None,
        }
    }

    fn fd(&self, fd: u64) -> Result<Aberto, i64> {
        match self.fds.get(fd as usize) {
            Some(Some(f)) => Ok(f.aberto.clone()),
            _ => Err(EBADF),
        }
    }

    /// Poe `aberto` no menor descritor livre a partir de `minimo`.
    fn novo_fd(&mut self, aberto: Aberto, fecha_no_exec: bool, minimo: usize) -> Result<usize, i64> {
        let fd = (minimo..MAX_FDS).find(|&i| self.fds.get(i).is_none_or(|f| f.is_none())).ok_or(EMFILE)?;
        if self.fds.len() <= fd {
            self.fds.resize(fd + 1, None);
        }
        self.fds[fd] = Some(Fd { aberto, fecha_no_exec });
        Ok(fd)
    }

    fn caminho(&self, caminho: &str) -> String {
        arquivos::absoluto(&self.pasta, caminho)
    }
}

/// O que fazer com o processo depois da chamada.
enum Fim {
    /// Volta para o programa com este valor em x0.
    Valor(i64),
    /// Espera (o estado ja foi trocado): o SVC roda de novo quando acordar.
    Esperar(&'static str),
    /// O processo saiu, trocou de programa ou ja foi arrumado: nao mexe.
    Nada,
}

type R = Result<i64, i64>;

/// Atende a chamada do processo `i` (que acabou de fazer SVC). Devolve
/// true se algo mudou que o terminal precisa ver.
pub fn atender(t: &mut Tabela, i: usize) -> bool {
    let c = &t.processos[i].contexto;
    let numero = c.x[8];
    let a = [c.x[0], c.x[1], c.x[2], c.x[3], c.x[4], c.x[5]];
    let fim = despachar(t, i, numero, a);
    let Some(p) = t.processos.get_mut(i) else { return true };
    match fim {
        Fim::Valor(v) => {
            p.contexto.x[0] = v as u64;
            matches!(numero, 64 | 66)
        }
        Fim::Esperar(motivo) => {
            // volta para o SVC: quando acordar, repete a chamada
            p.contexto.pc -= 4;
            if p.estado == Estado::Pronto {
                p.estado = Estado::Esperando(motivo);
            }
            false
        }
        Fim::Nada => true,
    }
}

fn despachar(t: &mut Tabela, i: usize, numero: u64, a: [u64; 6]) -> Fim {
    let r: R = match numero {
        // arquivos
        17 => getcwd(t, i, a[0] as usize, a[1] as usize),
        23 => dup(t, i, a[0], 0, false),
        24 => dup3(t, i, a[0], a[1], a[2] as u32),
        25 => fcntl(t, i, a[0], a[1], a[2]),
        29 => return ioctl(t, i, a[0], a[1], a[2] as usize),
        34 => mkdirat(t, i, a[0] as i64, a[1] as usize),
        35 => unlinkat(t, i, a[0] as i64, a[1] as usize, a[2]),
        38 => renameat(t, i, a[0] as i64, a[1] as usize, a[2] as i64, a[3] as usize),
        276 => renameat(t, i, a[0] as i64, a[1] as usize, a[2] as i64, a[3] as usize),
        43 | 44 => statfs(t, i, a[1] as usize),
        45 | 46 => truncar(t, i, numero, a[0], a[1] as usize),
        48 | 439 => faccessat(t, i, a[0] as i64, a[1] as usize),
        49 => chdir(t, i, a[0] as usize),
        50 => fchdir(t, i, a[0]),
        52 | 53 | 54 | 55 | 88 => Ok(0), // chmod, chown, utimensat: o FAT32 nao guarda
        56 => openat(t, i, a[0] as i64, a[1] as usize, a[2] as u32),
        57 => close(t, i, a[0]),
        59 => pipe2(t, i, a[0] as usize, a[1] as u32),
        61 => getdents64(t, i, a[0], a[1] as usize, a[2] as usize),
        62 => lseek(t, i, a[0], a[1] as i64, a[2]),
        63 => return ler(t, i, a[0], a[1] as usize, a[2] as usize),
        64 => return escrever(t, i, a[0], a[1] as usize, a[2] as usize),
        65 => return vetor(t, i, a[0], a[1] as usize, a[2] as usize, false),
        66 => return vetor(t, i, a[0], a[1] as usize, a[2] as usize, true),
        67 => pread(t, i, a[0], a[1] as usize, a[2] as usize, a[3] as usize),
        68 => pwrite(t, i, a[0], a[1] as usize, a[2] as usize, a[3] as usize),
        73 => return ppoll(t, i, a[0] as usize, a[1] as usize, a[2] as usize),
        78 => Err(EINVAL), // readlinkat: nao ha links simbolicos
        79 => newfstatat(t, i, a[0] as i64, a[1] as usize, a[2] as usize, a[3]),
        80 => fstat(t, i, a[0], a[1] as usize),
        81 => Ok(0),
        82 | 83 => fsync(t, i, a[0]),
        // processos
        93 | 94 => {
            t.terminar(i, (a[0] as i32 & 0xFF) << 8);
            return Fim::Nada;
        }
        96 => Ok(t.processos[i].pid as i64),
        98 => futex(t, i, a[0] as usize, a[1], a[2] as u32),
        99 => Ok(0),
        101 => return dormir(t, i, a[0] as usize),
        115 => return dormir(t, i, a[2] as usize),
        102 | 103 => Ok(0),
        113 => relogio(t, i, a[0], a[1] as usize),
        114 => relogio_resolucao(t, i, a[1] as usize),
        123 => Err(ENOSYS),
        124 => Ok(0),
        129 => kill(t, i, a[0] as i64, a[1] as usize),
        130 | 131 => kill(t, i, a[1] as i64, a[2] as usize),
        132 => Ok(0),
        134 => rt_sigaction(t, i, a[0] as usize, a[1] as usize, a[2] as usize),
        135 => rt_sigprocmask(t, i, a[2] as usize),
        142 => {
            // reboot do BusyBox: desliga ou reinicia de verdade
            let comando = if a[2] == 0x4321_FEDC { psci::DESLIGAR } else { psci::REINICIAR };
            psci::chamar(psci::via(), comando);
            Ok(0)
        }
        153 => times(t, i, a[0] as usize),
        154 => setpgid(t, i, a[0] as u32, a[1] as u32),
        155 => getpgid(t, i, a[0] as u32),
        156 => Ok(t.processos[i].linux.grupo as i64),
        157 => setsid(t, i),
        158 => Ok(0),
        160 => uname(t, i, a[0] as usize),
        163 => limite(t, i, a[0], a[1] as usize),
        261 => limite(t, i, a[1], a[3] as usize),
        164 => Ok(0),
        165 => Err(ENOSYS),
        166 => {
            let antes = t.processos[i].linux.umask;
            t.processos[i].linux.umask = a[0] as u32 & 0o777;
            Ok(antes as i64)
        }
        167 => Ok(0), // prctl
        169 => gettimeofday(t, i, a[0] as usize),
        172 => Ok(t.processos[i].pid as i64),
        173 => Ok(t.processos[i].pai as i64),
        174..=177 => Ok(0),
        178 => Ok(t.processos[i].pid as i64),
        179 => sysinfo(t, i, a[0] as usize),
        // memoria
        214 => brk(t, i, a[0] as usize),
        215 => munmap(t, i, a[0] as usize, a[1] as usize),
        216 => Err(ENOMEM), // mremap: a libc copia sozinha
        220 => return clone(t, i, a[0], a[1] as usize, a[3]),
        221 => return execve(t, i, a[0] as usize, a[1] as usize, a[2] as usize),
        222 => mmap(t, i, a[0] as usize, a[1] as usize, a[2], a[3], a[4] as i64, a[5] as usize),
        226 => mprotect(t, i, a[0] as usize, a[1] as usize, a[2]),
        233 => Ok(0),
        260 => return wait4(t, i, a[0] as i64, a[1] as usize, a[2]),
        278 => getrandom(t, i, a[0] as usize, a[1] as usize),
        _ => {
            *t.desconhecidas.entry(numero).or_insert(0) += 1;
            Err(ENOSYS)
        }
    };
    Fim::Valor(r.unwrap_or_else(|e| -e))
}

// ---------------------------------------------------------------------
// Memoria do programa

fn ler_mem(t: &Tabela, i: usize, endereco: usize, tamanho: usize) -> Result<Vec<u8>, i64> {
    let mut v = vec![0u8; tamanho];
    t.processos[i].espaco.ler(endereco, &mut v).map_err(|_| EFAULT)?;
    Ok(v)
}

fn escrever_mem(t: &mut Tabela, i: usize, endereco: usize, dados: &[u8]) -> Result<(), i64> {
    t.processos[i].espaco.escrever(endereco, dados).map_err(|_| EFAULT)
}

fn ler_u64(t: &Tabela, i: usize, endereco: usize) -> Result<u64, i64> {
    let mut b = [0u8; 8];
    t.processos[i].espaco.ler(endereco, &mut b).map_err(|_| EFAULT)?;
    Ok(u64::from_le_bytes(b))
}

/// Texto terminado em 0 vindo do programa.
fn texto(t: &Tabela, i: usize, endereco: usize) -> Result<String, i64> {
    let bytes = t.processos[i].espaco.ler_texto(endereco, 4096).map_err(|_| ENAMETOOLONG)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Caminho absoluto a partir de um dirfd (AT_FDCWD = a pasta atual).
fn caminho_em(t: &Tabela, i: usize, dirfd: i64, endereco: usize) -> Result<String, i64> {
    let nome = texto(t, i, endereco)?;
    let l = &t.processos[i].linux;
    if nome.starts_with('/') || dirfd == AT_FDCWD {
        return Ok(l.caminho(&nome));
    }
    let base = l.fd(dirfd as u64)?;
    let base = base.borrow();
    let pasta = base.caminho().ok_or(ENOTDIR)?;
    Ok(arquivos::absoluto(pasta, &nome))
}

// ---------------------------------------------------------------------
// Arquivos

fn openat(t: &mut Tabela, i: usize, dirfd: i64, endereco: usize, flags: u32) -> R {
    let caminho = caminho_em(t, i, dirfd, endereco)?;
    let aberto = match caminho.as_str() {
        "/dev/stdin" | "/dev/fd/0" => t.processos[i].linux.fd(0)?,
        "/dev/stdout" | "/dev/fd/1" => t.processos[i].linux.fd(1)?,
        "/dev/stderr" | "/dev/fd/2" => t.processos[i].linux.fd(2)?,
        _ => arquivos::abrir(&caminho, flags)?,
    };
    t.processos[i].linux.novo_fd(aberto, flags & O_CLOEXEC != 0, 0).map(|fd| fd as i64)
}

fn close(t: &mut Tabela, i: usize, fd: u64) -> R {
    let l = &mut t.processos[i].linux;
    match l.fds.get_mut(fd as usize) {
        Some(lugar @ Some(_)) => {
            *lugar = None;
            Ok(0)
        }
        _ => Err(EBADF),
    }
}

fn dup(t: &mut Tabela, i: usize, fd: u64, minimo: usize, fecha_no_exec: bool) -> R {
    let l = &mut t.processos[i].linux;
    let aberto = l.fd(fd)?;
    l.novo_fd(aberto, fecha_no_exec, minimo).map(|n| n as i64)
}

fn dup3(t: &mut Tabela, i: usize, velho: u64, novo: u64, flags: u32) -> R {
    let l = &mut t.processos[i].linux;
    let aberto = l.fd(velho)?;
    if velho == novo || novo as usize >= MAX_FDS {
        return Err(EINVAL);
    }
    if l.fds.len() <= novo as usize {
        l.fds.resize(novo as usize + 1, None);
    }
    l.fds[novo as usize] = Some(Fd { aberto, fecha_no_exec: flags & O_CLOEXEC != 0 });
    Ok(novo as i64)
}

fn fcntl(t: &mut Tabela, i: usize, fd: u64, comando: u64, arg: u64) -> R {
    match comando {
        0 => dup(t, i, fd, arg as usize, false),
        1030 => dup(t, i, fd, arg as usize, true),
        1 => {
            let l = &t.processos[i].linux;
            match l.fds.get(fd as usize) {
                Some(Some(f)) => Ok(f.fecha_no_exec as i64),
                _ => Err(EBADF),
            }
        }
        2 => match t.processos[i].linux.fds.get_mut(fd as usize) {
            Some(Some(f)) => {
                f.fecha_no_exec = arg & 1 != 0;
                Ok(0)
            }
            _ => Err(EBADF),
        },
        3 => Ok(t.processos[i].linux.fd(fd)?.borrow().flags as i64),
        4 => {
            let aberto = t.processos[i].linux.fd(fd)?;
            let mut d = aberto.borrow_mut();
            d.flags = (d.flags & O_ACESSO) | (arg as u32 & (O_APPEND | O_NONBLOCK));
            Ok(0)
        }
        _ => Ok(0),
    }
}

fn pipe2(t: &mut Tabela, i: usize, endereco: usize, flags: u32) -> R {
    let (leitura, escrita) = arquivos::tubo(flags & O_NONBLOCK);
    let cloexec = flags & O_CLOEXEC != 0;
    let l = &mut t.processos[i].linux;
    let a = l.novo_fd(leitura, cloexec, 0)?;
    let b = match l.novo_fd(escrita, cloexec, 0) {
        Ok(b) => b,
        Err(e) => {
            l.fds[a] = None;
            return Err(e);
        }
    };
    let mut bytes = [0u8; 8];
    bytes[..4].copy_from_slice(&(a as u32).to_le_bytes());
    bytes[4..].copy_from_slice(&(b as u32).to_le_bytes());
    escrever_mem(t, i, endereco, &bytes)?;
    Ok(0)
}

/// read: terminal, tubos, arquivos, dispositivos.
fn ler(t: &mut Tabela, i: usize, fd: u64, endereco: usize, tamanho: usize) -> Fim {
    let aberto = match t.processos[i].linux.fd(fd) {
        Ok(a) => a,
        Err(e) => return Fim::Valor(-e),
    };
    let tamanho = tamanho.min(1 << 20);
    let mut d = aberto.borrow_mut();
    let nao_bloqueia = d.flags & O_NONBLOCK != 0;
    let posicao = d.posicao;
    let mut nova_posicao = posicao;
    let dados: Vec<u8> = match &mut d.tipo {
        Tipo::Terminal => {
            if t.tty.prontos.is_empty() {
                if core::mem::take(&mut t.tty.fim) {
                    return Fim::Valor(0);
                }
                if nao_bloqueia {
                    return Fim::Valor(-EAGAIN);
                }
                return Fim::Esperar("lendo o teclado");
            }
            let n = tamanho.min(t.tty.prontos.len());
            // no modo canonico um read entrega no maximo uma linha
            let n = if t.tty.canonico {
                t.tty.prontos.iter().take(n).position(|&b| b == b'\n').map_or(n, |p| p + 1)
            } else {
                n
            };
            t.tty.prontos.drain(..n).collect()
        }
        Tipo::Nulo => Vec::new(),
        Tipo::Zero => vec![0; tamanho],
        Tipo::Aleatorio => (0..tamanho.div_ceil(16)).flat_map(|k| elf::aleatorio(k as u64)).take(tamanho).collect(),
        Tipo::Pasta { .. } => return Fim::Valor(-EISDIR),
        Tipo::Tubo(tubo, _) => {
            let mut tb = tubo.borrow_mut();
            if tb.buffer.is_empty() {
                if tb.escritores == 0 {
                    return Fim::Valor(0);
                }
                if nao_bloqueia {
                    return Fim::Valor(-EAGAIN);
                }
                return Fim::Esperar("lendo um tubo");
            }
            let n = tamanho.min(tb.buffer.len());
            tb.buffer.drain(..n).collect()
        }
        Tipo::Disco(a) => {
            let inicio = posicao.min(a.dados.len());
            let fim = (inicio + tamanho).min(a.dados.len());
            nova_posicao = fim;
            a.dados[inicio..fim].to_vec()
        }
    };
    d.posicao = nova_posicao;
    drop(d);
    match escrever_mem(t, i, endereco, &dados) {
        Ok(()) => Fim::Valor(dados.len() as i64),
        Err(e) => Fim::Valor(-e),
    }
}

/// write: terminal (vai para a tela), tubos, arquivos.
fn escrever(t: &mut Tabela, i: usize, fd: u64, endereco: usize, tamanho: usize) -> Fim {
    let dados = match ler_mem(t, i, endereco, tamanho.min(1 << 20)) {
        Ok(d) => d,
        Err(e) => return Fim::Valor(-e),
    };
    escrever_dados(t, i, fd, &dados)
}

fn escrever_dados(t: &mut Tabela, i: usize, fd: u64, dados: &[u8]) -> Fim {
    let aberto = match t.processos[i].linux.fd(fd) {
        Ok(a) => a,
        Err(e) => return Fim::Valor(-e),
    };
    let mut d = aberto.borrow_mut();
    if d.flags & O_ACESSO == O_RDONLY && !matches!(d.tipo, Tipo::Terminal | Tipo::Tubo(..)) {
        return Fim::Valor(-EBADF);
    }
    let anexar = d.flags & O_APPEND != 0;
    let posicao = d.posicao;
    match &mut d.tipo {
        Tipo::Terminal => t.escrever(dados),
        Tipo::Nulo | Tipo::Zero | Tipo::Aleatorio => {}
        Tipo::Pasta { .. } => return Fim::Valor(-EISDIR),
        Tipo::Tubo(tubo, _) => {
            let mut tb = tubo.borrow_mut();
            if tb.leitores == 0 {
                // SIGPIPE: termina, a nao ser que o programa ignore
                drop(tb);
                drop(d);
                if t.processos[i].linux.sinais[13] == SIG_DFL {
                    t.terminar(i, 13);
                    return Fim::Nada;
                }
                return Fim::Valor(-EPIPE);
            }
            if tb.buffer.len() >= TAMANHO_TUBO {
                return Fim::Esperar("escrevendo num tubo");
            }
            let n = dados.len().min(TAMANHO_TUBO - tb.buffer.len());
            tb.buffer.extend(&dados[..n]);
            return Fim::Valor(n as i64);
        }
        Tipo::Disco(a) => {
            let inicio = if anexar { a.dados.len() } else { posicao };
            let fim = inicio + dados.len();
            if a.dados.len() < fim {
                a.dados.resize(fim, 0);
            }
            a.dados[inicio..fim].copy_from_slice(dados);
            a.sujo = true;
            d.posicao = fim;
        }
    }
    Fim::Valor(dados.len() as i64)
}

/// readv/writev: uma lista de (endereco, tamanho).
fn vetor(t: &mut Tabela, i: usize, fd: u64, iov: usize, quantos: usize, escrita: bool) -> Fim {
    let mut pedacos = Vec::new();
    for k in 0..quantos.min(1024) {
        match (ler_u64(t, i, iov + k * 16), ler_u64(t, i, iov + k * 16 + 8)) {
            (Ok(base), Ok(n)) => pedacos.push((base as usize, n as usize)),
            _ => return Fim::Valor(-EFAULT),
        }
    }
    if escrita {
        let mut tudo = Vec::new();
        for (base, n) in pedacos {
            match ler_mem(t, i, base, n.min(1 << 20)) {
                Ok(d) => tudo.extend(d),
                Err(e) => return Fim::Valor(-e),
            }
        }
        return escrever_dados(t, i, fd, &tudo);
    }
    // readv: le no primeiro pedaco que tem espaco (basta para a libc)
    let Some(&(base, n)) = pedacos.iter().find(|&&(_, n)| n > 0) else { return Fim::Valor(0) };
    ler(t, i, fd, base, n)
}

fn pread(t: &mut Tabela, i: usize, fd: u64, endereco: usize, tamanho: usize, posicao: usize) -> R {
    let aberto = t.processos[i].linux.fd(fd)?;
    let d = aberto.borrow();
    let Tipo::Disco(a) = &d.tipo else { return Err(ESPIPE) };
    let inicio = posicao.min(a.dados.len());
    let fim = (inicio + tamanho).min(a.dados.len());
    let v = a.dados[inicio..fim].to_vec();
    drop(d);
    escrever_mem(t, i, endereco, &v)?;
    Ok(v.len() as i64)
}

fn pwrite(t: &mut Tabela, i: usize, fd: u64, endereco: usize, tamanho: usize, posicao: usize) -> R {
    let dados = ler_mem(t, i, endereco, tamanho.min(1 << 20))?;
    let aberto = t.processos[i].linux.fd(fd)?;
    let mut d = aberto.borrow_mut();
    let Tipo::Disco(a) = &mut d.tipo else { return Err(ESPIPE) };
    let fim = posicao + dados.len();
    if a.dados.len() < fim {
        a.dados.resize(fim, 0);
    }
    a.dados[posicao..fim].copy_from_slice(&dados);
    a.sujo = true;
    Ok(dados.len() as i64)
}

fn lseek(t: &mut Tabela, i: usize, fd: u64, deslocamento: i64, de_onde: u64) -> R {
    let aberto = t.processos[i].linux.fd(fd)?;
    let mut d = aberto.borrow_mut();
    let tamanho = match &d.tipo {
        Tipo::Disco(a) => a.dados.len() as i64,
        Tipo::Pasta { itens, .. } => itens.len() as i64,
        Tipo::Nulo | Tipo::Zero => 0,
        _ => return Err(ESPIPE),
    };
    let base = match de_onde {
        0 => 0,
        1 => d.posicao as i64,
        2 => tamanho,
        _ => return Err(EINVAL),
    };
    let nova = base.checked_add(deslocamento).filter(|&n| n >= 0).ok_or(EINVAL)?;
    d.posicao = nova as usize;
    Ok(nova)
}

fn fsync(t: &mut Tabela, i: usize, fd: u64) -> R {
    let aberto = t.processos[i].linux.fd(fd)?;
    aberto.borrow_mut().sincronizar()?;
    Ok(0)
}

fn truncar(t: &mut Tabela, i: usize, numero: u64, alvo: u64, tamanho: usize) -> R {
    if numero == 46 {
        let aberto = t.processos[i].linux.fd(alvo)?;
        let mut d = aberto.borrow_mut();
        let Tipo::Disco(a) = &mut d.tipo else { return Err(EINVAL) };
        a.dados.resize(tamanho, 0);
        a.sujo = true;
        return Ok(0);
    }
    let caminho = caminho_em(t, i, AT_FDCWD, alvo as usize)?;
    let mut dados = armazenamento::ler(&caminho).map_err(errno)?;
    dados.resize(tamanho, 0);
    armazenamento::escrever(&caminho, &dados).map_err(errno)?;
    Ok(0)
}

fn getdents64(t: &mut Tabela, i: usize, fd: u64, endereco: usize, tamanho: usize) -> R {
    let aberto = t.processos[i].linux.fd(fd)?;
    let mut d = aberto.borrow_mut();
    let posicao = d.posicao;
    let Tipo::Pasta { caminho, itens } = &d.tipo else { return Err(ENOTDIR) };
    let mut saida = Vec::new();
    let mut usados = 0;
    for item in itens.iter().skip(posicao) {
        let nome = item.nome.as_bytes();
        let registro = (19 + nome.len() + 1 + 7) & !7;
        if saida.len() + registro > tamanho {
            break;
        }
        let ino = arquivos::inode(&arquivos::absoluto(caminho, &item.nome));
        let inicio = saida.len();
        saida.extend_from_slice(&ino.to_le_bytes());
        saida.extend_from_slice(&((posicao + usados + 1) as u64).to_le_bytes());
        saida.extend_from_slice(&(registro as u16).to_le_bytes());
        saida.push(if item.pasta { 4 } else { 8 }); // DT_DIR, DT_REG
        saida.extend_from_slice(nome);
        saida.resize(inicio + registro, 0);
        usados += 1;
    }
    if usados == 0 && posicao < itens.len() {
        return Err(EINVAL); // nem um item coube
    }
    d.posicao += usados;
    drop(d);
    escrever_mem(t, i, endereco, &saida)?;
    Ok(saida.len() as i64)
}

fn getcwd(t: &mut Tabela, i: usize, endereco: usize, tamanho: usize) -> R {
    let mut pasta = t.processos[i].linux.pasta.clone().into_bytes();
    pasta.push(0);
    if pasta.len() > tamanho {
        return Err(ERANGE);
    }
    escrever_mem(t, i, endereco, &pasta)?;
    Ok(pasta.len() as i64)
}

fn chdir(t: &mut Tabela, i: usize, endereco: usize) -> R {
    let caminho = caminho_em(t, i, AT_FDCWD, endereco)?;
    match arquivos::info(&caminho)? {
        (true, _) => {
            t.processos[i].linux.pasta = caminho;
            Ok(0)
        }
        _ => Err(ENOTDIR),
    }
}

fn fchdir(t: &mut Tabela, i: usize, fd: u64) -> R {
    let aberto = t.processos[i].linux.fd(fd)?;
    let caminho = match &aberto.borrow().tipo {
        Tipo::Pasta { caminho, .. } => caminho.clone(),
        _ => return Err(ENOTDIR),
    };
    t.processos[i].linux.pasta = caminho;
    Ok(0)
}

fn mkdirat(t: &mut Tabela, i: usize, dirfd: i64, endereco: usize) -> R {
    let caminho = caminho_em(t, i, dirfd, endereco)?;
    armazenamento::criar_pasta(&caminho).map_err(errno)?;
    Ok(0)
}

fn unlinkat(t: &mut Tabela, i: usize, dirfd: i64, endereco: usize, flags: u64) -> R {
    let caminho = caminho_em(t, i, dirfd, endereco)?;
    let (pasta, _) = arquivos::info(&caminho)?;
    if pasta && flags & AT_REMOVEDIR == 0 {
        return Err(EISDIR);
    }
    if !pasta && flags & AT_REMOVEDIR != 0 {
        return Err(ENOTDIR);
    }
    armazenamento::apagar(&caminho).map_err(errno)?;
    Ok(0)
}

fn renameat(t: &mut Tabela, i: usize, dirfd_de: i64, de: usize, dirfd_para: i64, para: usize) -> R {
    let de = caminho_em(t, i, dirfd_de, de)?;
    let para = caminho_em(t, i, dirfd_para, para)?;
    // o Linux troca o destino se ele existe (e for arquivo)
    if de != para && matches!(arquivos::info(&para), Ok((false, _))) {
        armazenamento::apagar(&para).map_err(errno)?;
    }
    armazenamento::renomear(&de, &para).map_err(errno)?;
    Ok(0)
}

fn faccessat(t: &mut Tabela, i: usize, dirfd: i64, endereco: usize) -> R {
    let caminho = caminho_em(t, i, dirfd, endereco)?;
    arquivos::info(&caminho).map(|_| 0)
}

/// struct stat do aarch64 (128 bytes).
fn estrutura_stat(caminho: &str, pasta: bool, tamanho: u64, dispositivo: bool) -> [u8; 128] {
    let mut s = [0u8; 128];
    let modo: u32 = if pasta {
        0o040755
    } else if dispositivo {
        0o020666
    } else {
        0o100755
    };
    let tempo = segundos_do_rtc() as i64;
    s[0..8].copy_from_slice(&1u64.to_le_bytes());
    s[8..16].copy_from_slice(&arquivos::inode(caminho).to_le_bytes());
    s[16..20].copy_from_slice(&modo.to_le_bytes());
    s[20..24].copy_from_slice(&1u32.to_le_bytes());
    s[48..56].copy_from_slice(&(tamanho as i64).to_le_bytes());
    s[56..60].copy_from_slice(&4096i32.to_le_bytes());
    s[64..72].copy_from_slice(&(tamanho.div_ceil(512) as i64).to_le_bytes());
    for campo in [72, 88, 104] {
        s[campo..campo + 8].copy_from_slice(&tempo.to_le_bytes());
    }
    s
}

fn stat_do_aberto(d: &Descricao) -> [u8; 128] {
    match &d.tipo {
        Tipo::Disco(a) => estrutura_stat(&a.caminho, false, a.dados.len() as u64, false),
        Tipo::Pasta { caminho, .. } => estrutura_stat(caminho, true, 0, false),
        Tipo::Tubo(..) => {
            let mut s = estrutura_stat("tubo", false, 0, false);
            s[16..20].copy_from_slice(&0o010600u32.to_le_bytes());
            s
        }
        _ => estrutura_stat("/dev/tty", false, 0, true),
    }
}

fn fstat(t: &mut Tabela, i: usize, fd: u64, endereco: usize) -> R {
    let aberto = t.processos[i].linux.fd(fd)?;
    let s = stat_do_aberto(&aberto.borrow());
    escrever_mem(t, i, endereco, &s)?;
    Ok(0)
}

fn newfstatat(t: &mut Tabela, i: usize, dirfd: i64, endereco: usize, destino: usize, flags: u64) -> R {
    // AT_EMPTY_PATH (0x1000) com nome vazio: o proprio dirfd
    if flags & 0x1000 != 0 && t.processos[i].espaco.ler_texto(endereco, 1).is_ok_and(|n| n.is_empty()) {
        return fstat(t, i, dirfd as u64, destino);
    }
    let caminho = caminho_em(t, i, dirfd, endereco)?;
    let (pasta, tamanho) = arquivos::info(&caminho)?;
    let s = estrutura_stat(&caminho, pasta, tamanho, arquivos::dispositivo(&caminho).is_some());
    escrever_mem(t, i, destino, &s)?;
    Ok(0)
}

/// statfs/fstatfs: o tamanho e o livre do FAT32 (para o df).
fn statfs(t: &mut Tabela, i: usize, endereco: usize) -> R {
    let disco = armazenamento::discos().into_iter().find(|d| d.montado);
    let (total, livre) = disco.map_or((0, 0), |d| (d.tamanho, d.livre));
    let bloco = 4096u64;
    let mut s = [0u8; 120];
    s[0..8].copy_from_slice(&0x4d44u64.to_le_bytes()); // MSDOS_SUPER_MAGIC
    s[8..16].copy_from_slice(&bloco.to_le_bytes());
    s[16..24].copy_from_slice(&(total / bloco).to_le_bytes());
    s[24..32].copy_from_slice(&(livre / bloco).to_le_bytes());
    s[32..40].copy_from_slice(&(livre / bloco).to_le_bytes());
    s[64..72].copy_from_slice(&255u64.to_le_bytes());
    s[72..80].copy_from_slice(&bloco.to_le_bytes());
    escrever_mem(t, i, endereco, &s)?;
    Ok(0)
}

// ---------------------------------------------------------------------
// Terminal

const TCGETS: u64 = 0x5401;
const TCSETS: u64 = 0x5402;
const TCSETSW: u64 = 0x5403;
const TCSETSF: u64 = 0x5404;
const TIOCGPGRP: u64 = 0x540F;
const TIOCSPGRP: u64 = 0x5410;
const TIOCGWINSZ: u64 = 0x5413;
const FIONREAD: u64 = 0x541B;

fn ioctl(t: &mut Tabela, i: usize, fd: u64, pedido: u64, endereco: usize) -> Fim {
    let r = (|| -> R {
        let aberto = t.processos[i].linux.fd(fd)?;
        if !matches!(aberto.borrow().tipo, Tipo::Terminal) {
            return Err(ENOTTY);
        }
        match pedido {
            TCGETS => {
                let termios = t.tty.termios;
                escrever_mem(t, i, endereco, &termios)?;
                Ok(0)
            }
            TCSETS | TCSETSW | TCSETSF => {
                let novo = ler_mem(t, i, endereco, 36)?;
                t.tty.definir_termios(&novo);
                if pedido == TCSETSF {
                    t.tty.prontos.clear();
                }
                Ok(0)
            }
            TIOCGWINSZ => {
                let mut w = [0u8; 8];
                w[0..2].copy_from_slice(&25u16.to_le_bytes());
                w[2..4].copy_from_slice(&80u16.to_le_bytes());
                escrever_mem(t, i, endereco, &w)?;
                Ok(0)
            }
            TIOCGPGRP => {
                let grupo = t.tty.grupo;
                escrever_mem(t, i, endereco, &grupo.to_le_bytes())?;
                Ok(0)
            }
            TIOCSPGRP => {
                let grupo = ler_mem(t, i, endereco, 4)?;
                t.tty.grupo = u32::from_le_bytes([grupo[0], grupo[1], grupo[2], grupo[3]]);
                Ok(0)
            }
            FIONREAD => {
                let n = t.tty.prontos.len() as u32;
                escrever_mem(t, i, endereco, &n.to_le_bytes())?;
                Ok(0)
            }
            _ => Ok(0),
        }
    })();
    Fim::Valor(r.unwrap_or_else(|e| -e))
}

/// ppoll: quem esta pronto para ler ou escrever.
fn ppoll(t: &mut Tabela, i: usize, endereco: usize, quantos: usize, prazo: usize) -> Fim {
    const POLLIN: u16 = 1;
    const POLLOUT: u16 = 4;
    const POLLHUP: u16 = 0x10;
    const POLLNVAL: u16 = 0x20;
    let quantos = quantos.min(256);
    let Ok(mut fds) = ler_mem(t, i, endereco, quantos * 8) else { return Fim::Valor(-EFAULT) };
    let mut prontos = 0;
    for k in 0..quantos {
        let b = &fds[k * 8..k * 8 + 8];
        let fd = i32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let quer = u16::from_le_bytes([b[4], b[5]]);
        let tem = if fd < 0 {
            0
        } else {
            match t.processos[i].linux.fd(fd as u64) {
                Err(_) => POLLNVAL,
                Ok(aberto) => match &aberto.borrow().tipo {
                    Tipo::Terminal => {
                        let entrada = !t.tty.prontos.is_empty() || t.tty.fim;
                        (if entrada { POLLIN } else { 0 }) | POLLOUT
                    }
                    Tipo::Tubo(tubo, escrita) => {
                        let tb = tubo.borrow();
                        if *escrita {
                            if tb.leitores == 0 { POLLHUP } else { POLLOUT }
                        } else if !tb.buffer.is_empty() {
                            POLLIN
                        } else if tb.escritores == 0 {
                            POLLHUP
                        } else {
                            0
                        }
                    }
                    _ => POLLIN | POLLOUT,
                },
            }
        };
        let resposta = tem & (quer | POLLHUP | POLLNVAL);
        fds[k * 8 + 6..k * 8 + 8].copy_from_slice(&resposta.to_le_bytes());
        if resposta != 0 {
            prontos += 1;
        }
    }
    if prontos > 0 {
        t.processos[i].linux.acordar_em = None;
        return match escrever_mem(t, i, endereco, &fds) {
            Ok(()) => Fim::Valor(prontos),
            Err(e) => Fim::Valor(-e),
        };
    }
    // ninguem pronto: espera ate o prazo (sem prazo = para sempre)
    if prazo != 0 {
        let agora = cpu::contador();
        let alvo = match t.processos[i].linux.acordar_em {
            Some(alvo) => alvo,
            None => match tempo_em_ticks(t, i, prazo) {
                Ok(ticks) => agora.saturating_add(ticks),
                Err(e) => return Fim::Valor(-e),
            },
        };
        if agora >= alvo {
            t.processos[i].linux.acordar_em = None;
            let _ = escrever_mem(t, i, endereco, &fds);
            return Fim::Valor(0);
        }
        t.processos[i].linux.acordar_em = Some(alvo);
    }
    Fim::Esperar("esperando entrada")
}

// ---------------------------------------------------------------------
// Tempo

/// Segundos desde 1970 pelo RTC PL031 do QEMU virt.
pub fn segundos_do_rtc() -> u64 {
    unsafe { core::ptr::read_volatile(0x0901_0000 as *const u32) as u64 }
}

/// Lê um timespec {segundos, nanossegundos} e converte em ticks do contador.
fn tempo_em_ticks(t: &Tabela, i: usize, endereco: usize) -> Result<u64, i64> {
    let segundos = ler_u64(t, i, endereco)?;
    let nanos = ler_u64(t, i, endereco + 8)?;
    if nanos >= 1_000_000_000 {
        return Err(EINVAL);
    }
    let freq = cpu::frequencia_timer();
    Ok(segundos.saturating_mul(freq).saturating_add(nanos * freq / 1_000_000_000))
}

/// nanosleep(&{segundos, nanossegundos}, ...) e clock_nanosleep relativo.
fn dormir(t: &mut Tabela, i: usize, endereco: usize) -> Fim {
    let agora = cpu::contador();
    if let Some(alvo) = t.processos[i].linux.acordar_em {
        if agora >= alvo {
            t.processos[i].linux.acordar_em = None;
            return Fim::Valor(0);
        }
        t.processos[i].estado = Estado::Dormindo(alvo);
        return Fim::Esperar("dormindo");
    }
    let ticks = match tempo_em_ticks(t, i, endereco) {
        Ok(ticks) => ticks,
        Err(e) => return Fim::Valor(-e),
    };
    let alvo = agora.saturating_add(ticks);
    t.processos[i].linux.acordar_em = Some(alvo);
    t.processos[i].estado = Estado::Dormindo(alvo);
    Fim::Esperar("dormindo")
}

/// (segundos, nanossegundos): relogio de parede (RTC) ou desde o boot.
fn agora(parede: bool) -> (u64, u64) {
    let freq = cpu::frequencia_timer();
    let contador = cpu::contador();
    let mut segundos = contador / freq;
    if parede {
        segundos += super::rtc_no_boot();
    }
    (segundos, contador % freq * 1_000_000_000 / freq)
}

fn relogio(t: &mut Tabela, i: usize, qual: u64, endereco: usize) -> R {
    // CLOCK_REALTIME (0) e o _COARSE (5) sao a hora de parede
    let (s, n) = agora(matches!(qual, 0 | 5));
    let mut tempo = [0u8; 16];
    tempo[..8].copy_from_slice(&s.to_le_bytes());
    tempo[8..].copy_from_slice(&n.to_le_bytes());
    escrever_mem(t, i, endereco, &tempo)?;
    Ok(0)
}

fn relogio_resolucao(t: &mut Tabela, i: usize, endereco: usize) -> R {
    if endereco != 0 {
        let mut tempo = [0u8; 16];
        tempo[8..].copy_from_slice(&16u64.to_le_bytes());
        escrever_mem(t, i, endereco, &tempo)?;
    }
    Ok(0)
}

fn gettimeofday(t: &mut Tabela, i: usize, endereco: usize) -> R {
    let (s, n) = agora(true);
    let mut tempo = [0u8; 16];
    tempo[..8].copy_from_slice(&s.to_le_bytes());
    tempo[8..].copy_from_slice(&(n / 1000).to_le_bytes());
    escrever_mem(t, i, endereco, &tempo)?;
    Ok(0)
}

fn times(t: &mut Tabela, i: usize, endereco: usize) -> R {
    let freq = cpu::frequencia_timer().max(1);
    let usuario = t.processos[i].tempo_cpu * 100 / freq;
    if endereco != 0 {
        let mut b = [0u8; 32];
        b[..8].copy_from_slice(&usuario.to_le_bytes());
        escrever_mem(t, i, endereco, &b)?;
    }
    Ok((cpu::contador() * 100 / freq) as i64)
}

fn sysinfo(t: &mut Tabela, i: usize, endereco: usize) -> R {
    let h = crate::heap::estatisticas();
    let mut s = [0u8; 112];
    let uptime = cpu::contador() / cpu::frequencia_timer().max(1);
    s[0..8].copy_from_slice(&uptime.to_le_bytes());
    s[32..40].copy_from_slice(&(h.total as u64).to_le_bytes());
    s[40..48].copy_from_slice(&(h.livre as u64).to_le_bytes());
    s[80..82].copy_from_slice(&(t.processos.len() as u16).to_le_bytes());
    s[104..108].copy_from_slice(&1u32.to_le_bytes()); // mem_unit
    escrever_mem(t, i, endereco, &s)?;
    Ok(0)
}

// ---------------------------------------------------------------------
// Memoria

fn brk(t: &mut Tabela, i: usize, pedido: usize) -> R {
    let p = &mut t.processos[i];
    let l = &mut p.linux;
    if pedido == 0 || pedido < l.brk_inicio || pedido > LIMITE_BRK {
        return Ok(l.brk as i64);
    }
    let atual = l.brk.next_multiple_of(PAGINA);
    let novo = pedido.next_multiple_of(PAGINA);
    if novo > atual {
        if p.espaco.mapear_faixa(atual, novo, Permissao::Dados).is_err() {
            return Ok(l.brk as i64);
        }
    } else {
        let mut pagina = novo;
        while pagina < atual {
            p.espaco.desmapear(pagina);
            pagina += PAGINA;
        }
    }
    l.brk = pedido;
    Ok(pedido as i64)
}

#[allow(clippy::too_many_arguments)]
fn mmap(t: &mut Tabela, i: usize, dica: usize, tamanho: usize, prot: u64, flags: u64, fd: i64, deslocamento: usize) -> R {
    const MAP_FIXED: u64 = 0x10;
    const MAP_ANONYMOUS: u64 = 0x20;
    if tamanho == 0 || deslocamento % PAGINA != 0 {
        return Err(EINVAL);
    }
    let tamanho = tamanho.next_multiple_of(PAGINA);
    // conteudo do arquivo (mmap privado: uma copia)
    let conteudo = if flags & MAP_ANONYMOUS == 0 {
        let aberto = t.processos[i].linux.fd(fd as u64)?;
        let d = aberto.borrow();
        let Tipo::Disco(a) = &d.tipo else { return Err(EACCES) };
        let inicio = deslocamento.min(a.dados.len());
        let fim = (deslocamento + tamanho).min(a.dados.len());
        a.dados[inicio..fim].to_vec()
    } else {
        Vec::new()
    };
    let p = &mut t.processos[i];
    let endereco = if flags & MAP_FIXED != 0 {
        if dica % PAGINA != 0 {
            return Err(EINVAL);
        }
        // MAP_FIXED troca o que estava la
        let mut pagina = dica;
        while pagina < dica + tamanho {
            p.espaco.desmapear(pagina);
            pagina += PAGINA;
        }
        dica
    } else {
        let desde = if dica >= BASE_MMAP { dica } else { BASE_MMAP };
        p.espaco
            .achar_livre(desde, tamanho)
            .filter(|&e| e + tamanho <= TOPO_PILHA - TAMANHO_PILHA)
            .or_else(|| p.espaco.achar_livre(BASE_MMAP, tamanho).filter(|&e| e + tamanho <= TOPO_PILHA - TAMANHO_PILHA))
            .ok_or(ENOMEM)?
    };
    let permissao = Permissao::de_prot(prot);
    // as paginas nascem gravaveis para receber o conteudo; depois ficam como pedido
    p.espaco.mapear_faixa(endereco, endereco + tamanho, Permissao::Tudo).map_err(|_| ENOMEM)?;
    if !conteudo.is_empty() {
        p.espaco.escrever_forcado(endereco, &conteudo).map_err(|_| ENOMEM)?;
    }
    let mut pagina = endereco;
    while pagina < endereco + tamanho {
        p.espaco.proteger(pagina, permissao);
        pagina += PAGINA;
    }
    Ok(endereco as i64)
}

fn munmap(t: &mut Tabela, i: usize, endereco: usize, tamanho: usize) -> R {
    if endereco % PAGINA != 0 {
        return Err(EINVAL);
    }
    let fim = endereco.saturating_add(tamanho.next_multiple_of(PAGINA));
    let mut pagina = endereco;
    while pagina < fim {
        t.processos[i].espaco.desmapear(pagina);
        pagina += PAGINA;
    }
    Ok(0)
}

fn mprotect(t: &mut Tabela, i: usize, endereco: usize, tamanho: usize, prot: u64) -> R {
    if endereco % PAGINA != 0 {
        return Err(EINVAL);
    }
    let permissao = Permissao::de_prot(prot);
    let fim = endereco.saturating_add(tamanho.next_multiple_of(PAGINA));
    let mut pagina = endereco;
    while pagina < fim {
        t.processos[i].espaco.proteger(pagina, permissao);
        pagina += PAGINA;
    }
    Ok(0)
}

fn futex(t: &mut Tabela, i: usize, endereco: usize, operacao: u64, valor: u32) -> R {
    match operacao & 0x7F {
        // FUTEX_WAIT: com um processo de uma thread so, ninguem vai acordar
        0 | 9 => {
            let mut b = [0u8; 4];
            t.processos[i].espaco.ler(endereco, &mut b).map_err(|_| EFAULT)?;
            if u32::from_le_bytes(b) != valor { Err(EAGAIN) } else { Ok(0) }
        }
        _ => Ok(0),
    }
}

fn getrandom(t: &mut Tabela, i: usize, endereco: usize, tamanho: usize) -> R {
    let tamanho = tamanho.min(1 << 16);
    let bytes: Vec<u8> = (0..tamanho.div_ceil(16)).flat_map(|k| elf::aleatorio(k as u64 + endereco as u64)).take(tamanho).collect();
    escrever_mem(t, i, endereco, &bytes)?;
    Ok(tamanho as i64)
}

// ---------------------------------------------------------------------
// Processos

fn uname(t: &mut Tabela, i: usize, endereco: usize) -> R {
    let campos = ["Linux", "mxos", "6.1.0-mxos", "MXOS 0.9", "aarch64", "(none)"];
    let mut u = [0u8; 65 * 6];
    for (k, campo) in campos.iter().enumerate() {
        u[k * 65..k * 65 + campo.len()].copy_from_slice(campo.as_bytes());
    }
    escrever_mem(t, i, endereco, &u)?;
    Ok(0)
}

/// getrlimit e prlimit64: so leitura (mudar o limite nao faz nada).
fn limite(t: &mut Tabela, i: usize, recurso: u64, destino: usize) -> R {
    if destino != 0 {
        let infinito = u64::MAX;
        let (atual, maximo) = match recurso {
            3 => (TAMANHO_PILHA as u64, infinito), // RLIMIT_STACK
            7 => (MAX_FDS as u64, MAX_FDS as u64), // RLIMIT_NOFILE
            _ => (infinito, infinito),
        };
        let mut b = [0u8; 16];
        b[..8].copy_from_slice(&atual.to_le_bytes());
        b[8..].copy_from_slice(&maximo.to_le_bytes());
        escrever_mem(t, i, destino, &b)?;
    }
    Ok(0)
}

fn rt_sigaction(t: &mut Tabela, i: usize, sinal: usize, novo: usize, velho: usize) -> R {
    if sinal == 0 || sinal >= SINAIS || sinal == 9 || sinal == 19 {
        return Err(EINVAL);
    }
    let antes = t.processos[i].linux.sinais[sinal];
    if velho != 0 {
        let mut b = [0u8; 32];
        b[..8].copy_from_slice(&antes.to_le_bytes());
        escrever_mem(t, i, velho, &b)?;
    }
    if novo != 0 {
        let tratador = ler_u64(t, i, novo)?;
        t.processos[i].linux.sinais[sinal] = tratador;
    }
    Ok(0)
}

fn rt_sigprocmask(t: &mut Tabela, i: usize, velho: usize) -> R {
    if velho != 0 {
        escrever_mem(t, i, velho, &[0u8; 8])?;
    }
    Ok(0)
}

fn kill(t: &mut Tabela, i: usize, pid: i64, sinal: usize) -> R {
    if sinal >= SINAIS {
        return Err(EINVAL);
    }
    let alvos: Vec<u32> = match pid {
        p if p > 0 => vec![p as u32],
        0 => {
            let grupo = t.processos[i].linux.grupo;
            t.processos.iter().filter(|p| p.linux.grupo == grupo).map(|p| p.pid).collect()
        }
        -1 => t.processos.iter().map(|p| p.pid).filter(|&p| p != t.processos[i].pid).collect(),
        p => t.processos.iter().filter(|q| q.linux.grupo == (-p) as u32).map(|q| q.pid).collect(),
    };
    if alvos.is_empty() {
        return Err(ESRCH);
    }
    let mut algum = false;
    for alvo in alvos {
        algum |= t.sinal(alvo, sinal);
    }
    if algum { Ok(0) } else { Err(ESRCH) }
}

fn setpgid(t: &mut Tabela, i: usize, pid: u32, grupo: u32) -> R {
    let pid = if pid == 0 { t.processos[i].pid } else { pid };
    let grupo = if grupo == 0 { pid } else { grupo };
    let j = t.indice(pid).ok_or(ESRCH)?;
    t.processos[j].linux.grupo = grupo;
    Ok(0)
}

fn getpgid(t: &mut Tabela, i: usize, pid: u32) -> R {
    let j = if pid == 0 { i } else { t.indice(pid).ok_or(ESRCH)? };
    Ok(t.processos[j].linux.grupo as i64)
}

fn setsid(t: &mut Tabela, i: usize) -> R {
    let pid = t.processos[i].pid;
    t.processos[i].linux.grupo = pid;
    Ok(pid as i64)
}

/// clone: so o fork (com ou sem CLONE_VFORK/CLONE_VM, que viram copia).
/// Threads (CLONE_THREAD) ainda nao existem.
fn clone(t: &mut Tabela, i: usize, flags: u64, pilha: usize, tls: u64) -> Fim {
    const CLONE_VM: u64 = 0x100;
    const CLONE_VFORK: u64 = 0x4000;
    const CLONE_SETTLS: u64 = 0x80000;
    const CLONE_THREAD: u64 = 0x10000;
    let _ = CLONE_VM;
    if flags & CLONE_THREAD != 0 {
        *t.desconhecidas.entry(220).or_insert(0) += 1;
        return Fim::Valor(-ENOSYS);
    }
    let Some(asid) = t.asid_para_novo() else { return Fim::Valor(-EAGAIN) };
    let p = &t.processos[i];
    let espaco = p.espaco.duplicar(asid);
    let mut contexto = p.contexto.copia();
    contexto.x[0] = 0; // o filho ve 0
    if pilha != 0 {
        contexto.sp = pilha as u64;
    }
    if flags & CLONE_SETTLS != 0 {
        contexto.tpidr = tls;
    }
    let linux = p.linux.duplicar();
    let (pid_pai, nome) = (p.pid, p.nome.clone());
    let filho = t.acrescentar(pid_pai, nome, contexto, espaco);
    let j = t.processos.len() - 1;
    t.processos[j].linux = linux;
    if flags & CLONE_VFORK != 0 {
        // o pai para ate o filho trocar de programa ou sair
        t.processos[j].linux.pai_vfork = Some(pid_pai);
        t.processos[i].estado = Estado::Vfork;
    }
    Fim::Valor(filho as i64)
}

/// Le uma lista de ponteiros para textos (argv, envp) terminada em NULL.
fn lista_de_textos(t: &Tabela, i: usize, endereco: usize) -> Result<Vec<Vec<u8>>, i64> {
    let mut lista = Vec::new();
    if endereco == 0 {
        return Ok(lista);
    }
    for k in 0..4096 {
        let ponteiro = ler_u64(t, i, endereco + k * 8)? as usize;
        if ponteiro == 0 {
            return Ok(lista);
        }
        lista.push(t.processos[i].espaco.ler_texto(ponteiro, 128 * 1024).map_err(|_| EFAULT)?);
    }
    Err(EINVAL)
}

fn execve(t: &mut Tabela, i: usize, caminho: usize, argv: usize, envp: usize) -> Fim {
    let r = (|| -> R {
        let caminho = caminho_em(t, i, AT_FDCWD, caminho)?;
        let argumentos = lista_de_textos(t, i, argv)?;
        let ambiente = lista_de_textos(t, i, envp)?;
        trocar_programa(t, i, &caminho, argumentos, ambiente)?;
        Ok(0)
    })();
    match r {
        Ok(_) => Fim::Nada,
        Err(e) => Fim::Valor(-e),
    }
}

/// Carrega `caminho` no lugar do programa do processo `i` (execve).
/// Scripts com "#!" rodam pelo interpretador.
pub fn trocar_programa(t: &mut Tabela, i: usize, caminho: &str, mut argumentos: Vec<Vec<u8>>, ambiente: Vec<Vec<u8>>) -> Result<(), i64> {
    let mut caminho = caminho.to_owned();
    let mut dados = armazenamento::ler(&caminho).map_err(errno)?;
    if dados.starts_with(b"#!") {
        let linha = dados[2..].split(|&b| b == b'\n').next().unwrap_or(&[]);
        let linha = String::from_utf8_lossy(linha).trim().to_owned();
        let mut partes = linha.splitn(2, char::is_whitespace);
        let interpretador = partes.next().unwrap_or("").to_owned();
        let mut novos: Vec<Vec<u8>> = vec![interpretador.clone().into_bytes()];
        if let Some(arg) = partes.next().map(str::trim).filter(|a| !a.is_empty()) {
            novos.push(arg.as_bytes().to_vec());
        }
        novos.push(caminho.clone().into_bytes());
        novos.extend(argumentos.into_iter().skip(1));
        argumentos = novos;
        caminho = interpretador;
        dados = armazenamento::ler(&caminho).map_err(errno)?;
    }
    elf::conferir(&dados).map_err(|_| ENOEXEC)?;
    let asid = t.processos[i].espaco.asid;
    let mut espaco = Espaco::novo(asid);
    let programa = elf::carregar(&dados, &mut espaco).map_err(|_| ENOEXEC)?;
    espaco.mapear_faixa(TOPO_PILHA - TAMANHO_PILHA, TOPO_PILHA, Permissao::Dados).map_err(|_| ENOMEM)?;
    let args: Vec<&[u8]> = argumentos.iter().map(|a| a.as_slice()).collect();
    let env: Vec<&[u8]> = ambiente.iter().map(|a| a.as_slice()).collect();
    let sp = elf::montar_pilha(&mut espaco, TOPO_PILHA, &args, &env, &programa, caminho.as_bytes()).map_err(|_| ENOMEM)?;

    let p = &mut t.processos[i];
    p.espaco = espaco;
    p.contexto = Contexto::novo(programa.entrada, sp);
    p.nome = caminho.rsplit('/').next().unwrap_or(&caminho).to_owned();
    let l = &mut p.linux;
    l.brk_inicio = programa.fim.next_multiple_of(PAGINA);
    l.brk = l.brk_inicio;
    l.acordar_em = None;
    for fd in l.fds.iter_mut() {
        if fd.as_ref().is_some_and(|f| f.fecha_no_exec) {
            *fd = None;
        }
    }
    // tratadores somem no exec (o codigo deles foi embora); ignorados ficam
    for s in l.sinais.iter_mut() {
        if *s != SIG_IGN {
            *s = SIG_DFL;
        }
    }
    if let Some(pai) = l.pai_vfork.take() {
        t.acordar_vfork(pai);
    }
    Ok(())
}

/// wait4(pid, &status, opcoes, rusage).
fn wait4(t: &mut Tabela, i: usize, pid: i64, status: usize, opcoes: u64) -> Fim {
    const WNOHANG: u64 = 1;
    let eu = t.processos[i].pid;
    let grupo = t.processos[i].linux.grupo;
    let serve = |p: &super::Processo| {
        p.pai == eu
            && match pid {
                -1 => true,
                0 => p.linux.grupo == grupo,
                p_ if p_ > 0 => p.pid == p_ as u32,
                g => p.linux.grupo == (-g) as u32,
            }
    };
    if !t.processos.iter().any(|p| serve(p)) {
        return Fim::Valor(-ECHILD);
    }
    let achado = t.processos.iter().position(|p| serve(p) && matches!(p.estado, Estado::Terminado(_)));
    let Some(j) = achado else {
        return if opcoes & WNOHANG != 0 { Fim::Valor(0) } else { Fim::Esperar("esperando um filho") };
    };
    let Estado::Terminado(codigo) = t.processos[j].estado else { return Fim::Valor(-ECHILD) };
    let filho = t.processos[j].pid;
    t.processos.remove(j);
    let i = if j < i { i - 1 } else { i };
    if status != 0 && escrever_mem(t, i, status, &(codigo as u32).to_le_bytes()).is_err() {
        return Fim::Valor(-EFAULT);
    }
    t.processos[i].contexto.x[0] = filho as u64;
    // o indice do processo mudou: devolve o valor direto e nao mexe mais
    Fim::Nada
}
