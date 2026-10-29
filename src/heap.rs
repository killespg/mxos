//! Alocador de memoria (heap): o que faz `Vec`, `String` e `Box` funcionarem.
//!
//! Lista de blocos livres ordenada por endereco. Cada bloco livre guarda o
//! proprio tamanho e o endereco do proximo nos primeiros 16 bytes dele mesmo.
//!   - alocar: pega o primeiro bloco que cabe (first fit) e corta o que sobra
//!   - liberar: devolve o bloco na posicao certa e junta com os vizinhos,
//!     para a memoria nao virar picadinho
//!
//! A trava usa instrucoes atomicas, que no ARM so funcionam com a MMU ligada.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::hint::spin_loop;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicBool, Ordering};

/// Todo bloco tem tamanho e endereco multiplos de 16, entao qualquer sobra
/// tem no minimo 16 bytes e cabe o cabecalho de bloco livre.
const ALINHAMENTO: usize = 16;

#[repr(C)]
struct Livre {
    tamanho: usize,
    proximo: *mut Livre,
}

struct Estado {
    primeiro: *mut Livre,
    inicio: usize,
    total: usize,
    livre: usize,
    alocacoes: usize,
}

struct Heap {
    trava: AtomicBool,
    estado: UnsafeCell<Estado>,
}

unsafe impl Sync for Heap {}

#[global_allocator]
static HEAP: Heap = Heap {
    trava: AtomicBool::new(false),
    estado: UnsafeCell::new(Estado { primeiro: null_mut(), inicio: 0, total: 0, livre: 0, alocacoes: 0 }),
};

impl Heap {
    fn com<R>(&self, acao: impl FnOnce(&mut Estado) -> R) -> R {
        while self.trava.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            spin_loop();
        }
        let resultado = acao(unsafe { &mut *self.estado.get() });
        self.trava.store(false, Ordering::Release);
        resultado
    }
}

fn arredondar(valor: usize, alinhamento: usize) -> usize {
    (valor + alinhamento - 1) & !(alinhamento - 1)
}

fn tamanho_real(layout: Layout) -> usize {
    arredondar(layout.size().max(1), ALINHAMENTO)
}

/// Entrega [inicio, fim) para o heap. Chamar depois de ligar a MMU.
pub fn iniciar(inicio: usize, fim: usize) {
    let inicio = arredondar(inicio, ALINHAMENTO);
    let fim = fim & !(ALINHAMENTO - 1);
    HEAP.com(|estado| {
        let bloco = inicio as *mut Livre;
        unsafe { bloco.write(Livre { tamanho: fim - inicio, proximo: null_mut() }) };
        estado.primeiro = bloco;
        estado.inicio = inicio;
        estado.total = fim - inicio;
        estado.livre = fim - inicio;
    });
}

/// Mais um pedaco de RAM para o heap (separado do primeiro: na TV box ha
/// um buraco do firmware no meio).
#[cfg_attr(not(feature = "mxq"), allow(dead_code))]
pub fn acrescentar(inicio: usize, fim: usize) {
    let inicio = arredondar(inicio, ALINHAMENTO);
    let fim = fim & !(ALINHAMENTO - 1);
    if fim <= inicio {
        return;
    }
    HEAP.com(|estado| unsafe {
        estado.liberar(inicio, fim - inicio);
        estado.total += fim - inicio;
        // liberar contou como devolucao de uma alocacao que nunca houve
        estado.alocacoes += 1;
    });
}

unsafe impl GlobalAlloc for Heap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let tamanho = tamanho_real(layout);
        let alinhamento = layout.align().max(ALINHAMENTO);
        self.com(|estado| unsafe { estado.alocar(tamanho, alinhamento) })
    }

    unsafe fn dealloc(&self, ponteiro: *mut u8, layout: Layout) {
        let tamanho = tamanho_real(layout);
        self.com(|estado| unsafe { estado.liberar(ponteiro as usize, tamanho) })
    }
}

impl Estado {
    unsafe fn alocar(&mut self, tamanho: usize, alinhamento: usize) -> *mut u8 {
        unsafe {
            let mut anterior: *mut Livre = null_mut();
            let mut atual = self.primeiro;
            while !atual.is_null() {
                let bloco = atual as usize;
                let fim_bloco = bloco + (*atual).tamanho;
                let inicio = arredondar(bloco, alinhamento);
                let fim = inicio + tamanho;
                if fim <= fim_bloco {
                    let proximo = (*atual).proximo;
                    // a sobra do fim vira um bloco livre novo
                    let depois = if fim < fim_bloco {
                        let sobra = fim as *mut Livre;
                        sobra.write(Livre { tamanho: fim_bloco - fim, proximo });
                        sobra
                    } else {
                        proximo
                    };
                    if inicio > bloco {
                        // a sobra do comeco (por causa do alinhamento) continua livre
                        (*atual).tamanho = inicio - bloco;
                        (*atual).proximo = depois;
                    } else if anterior.is_null() {
                        self.primeiro = depois;
                    } else {
                        (*anterior).proximo = depois;
                    }
                    self.livre -= tamanho;
                    self.alocacoes += 1;
                    return inicio as *mut u8;
                }
                anterior = atual;
                atual = (*atual).proximo;
            }
            null_mut() // sem memoria: o Rust transforma isso em panic
        }
    }

    unsafe fn liberar(&mut self, endereco: usize, tamanho: usize) {
        unsafe {
            let mut anterior: *mut Livre = null_mut();
            let mut atual = self.primeiro;
            while !atual.is_null() && (atual as usize) < endereco {
                anterior = atual;
                atual = (*atual).proximo;
            }

            let bloco = endereco as *mut Livre;
            bloco.write(Livre { tamanho, proximo: atual });
            if !atual.is_null() && endereco + tamanho == atual as usize {
                (*bloco).tamanho += (*atual).tamanho;
                (*bloco).proximo = (*atual).proximo;
            }
            if anterior.is_null() {
                self.primeiro = bloco;
            } else if anterior as usize + (*anterior).tamanho == endereco {
                (*anterior).tamanho += (*bloco).tamanho;
                (*anterior).proximo = (*bloco).proximo;
            } else {
                (*anterior).proximo = bloco;
            }
            self.livre += tamanho;
            self.alocacoes -= 1;
        }
    }
}

pub struct Estatisticas {
    pub inicio: usize,
    pub total: usize,
    pub livre: usize,
    pub alocacoes: usize,
    pub blocos_livres: usize,
    pub maior_bloco: usize,
}

pub fn estatisticas() -> Estatisticas {
    HEAP.com(|estado| {
        let mut blocos_livres = 0;
        let mut maior_bloco = 0;
        let mut atual = estado.primeiro;
        while !atual.is_null() {
            unsafe {
                blocos_livres += 1;
                maior_bloco = maior_bloco.max((*atual).tamanho);
                atual = (*atual).proximo;
            }
        }
        Estatisticas {
            inicio: estado.inicio,
            total: estado.total,
            livre: estado.livre,
            alocacoes: estado.alocacoes,
            blocos_livres,
            maior_bloco,
        }
    })
}
