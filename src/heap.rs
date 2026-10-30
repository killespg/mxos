//! Alocador de memoria (heap): o que faz `Vec`, `String` e `Box` funcionarem.
//!
//! Duas camadas:
//!   - pedidos pequenos (ate 4 KB, alinhamento ate 16, a imensa maioria:
//!     Vec, String, Box, Rc do JS, do CSS e do layout) vao para classes de
//!     tamanho: cada classe tem a propria lista de livres, e alocar/liberar e
//!     tirar/por na cabeca dela, em tempo constante. As classes se abastecem
//!     em lotes de 64 KB da camada de baixo e nunca devolvem.
//!   - o resto: lista de blocos livres ordenada por endereco. Cada bloco
//!     livre guarda o proprio tamanho e o endereco do proximo nos primeiros
//!     16 bytes dele mesmo. Alocar pega o primeiro que cabe (first fit);
//!     liberar devolve no lugar certo e junta com os vizinhos.
//! Antes era so a lista: depois de navegar ela tinha milhares de blocos, e
//! cada alocacao a percorria (no Cortex-A7, centenas de us por malloc).
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

/// Tamanhos das classes (multiplos de 16, ate 4 KB).
const CLASSES: [usize; 28] = [
    16, 32, 48, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 448, 512, 640, 768, 896, 1024, 1280, 1536, 1792, 2048,
    2560, 3072, 3584, 4096,
];
const MAIOR_CLASSE: usize = 4096;
/// Quanto cada classe pega da lista de baixo por vez.
const LOTE: usize = 64 * 1024;

/// A classe de um tamanho ja arredondado para 16 (o primeiro que cabe).
fn classe(tamanho: usize) -> usize {
    if tamanho <= 128 {
        return tamanho / 16 - 1;
    }
    let mut k = 8;
    while CLASSES[k] < tamanho {
        k += 1;
    }
    k
}

struct No {
    proximo: *mut No,
}

struct Estado {
    primeiro: *mut Livre,
    inicio: usize,
    total: usize,
    livre: usize,
    alocacoes: usize,
    /// Cabeca da lista de livres de cada classe.
    classes: [*mut No; CLASSES.len()],
    /// O pedaco do lote ainda nunca usado de cada classe: [atual, fim).
    lote: [(usize, usize); CLASSES.len()],
}

struct Heap {
    trava: AtomicBool,
    estado: UnsafeCell<Estado>,
}

unsafe impl Sync for Heap {}

#[cfg_attr(not(test), global_allocator)]
static HEAP: Heap = Heap {
    trava: AtomicBool::new(false),
    estado: UnsafeCell::new(Estado {
        primeiro: null_mut(),
        inicio: 0,
        total: 0,
        livre: 0,
        alocacoes: 0,
        classes: [null_mut(); CLASSES.len()],
        lote: [(0, 0); CLASSES.len()],
    }),
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
        if tamanho <= MAIOR_CLASSE && alinhamento <= ALINHAMENTO {
            return self.com(|estado| unsafe { estado.alocar_pequeno(classe(tamanho)) });
        }
        self.com(|estado| unsafe { estado.alocar(tamanho, alinhamento) })
    }

    unsafe fn dealloc(&self, ponteiro: *mut u8, layout: Layout) {
        let tamanho = tamanho_real(layout);
        if tamanho <= MAIOR_CLASSE && layout.align() <= ALINHAMENTO {
            return self.com(|estado| unsafe { estado.liberar_pequeno(ponteiro, classe(tamanho)) });
        }
        self.com(|estado| unsafe { estado.liberar(ponteiro as usize, tamanho) })
    }

    unsafe fn realloc(&self, ponteiro: *mut u8, layout: Layout, novo: usize) -> *mut u8 {
        // mesma classe (Vec crescendo dentro da folga): nada a fazer
        let (antes, depois) = (tamanho_real(layout), arredondar(novo.max(1), ALINHAMENTO));
        if layout.align() <= ALINHAMENTO && antes <= MAIOR_CLASSE && depois <= MAIOR_CLASSE && classe(antes) == classe(depois) {
            return ponteiro;
        }
        let nl = unsafe { Layout::from_size_align_unchecked(novo, layout.align()) };
        let p = unsafe { self.alloc(nl) };
        if !p.is_null() {
            unsafe {
                core::ptr::copy_nonoverlapping(ponteiro, p, layout.size().min(novo));
                self.dealloc(ponteiro, layout);
            }
        }
        p
    }
}

impl Estado {
    /// Um bloco da classe `k`: da lista de livres, do lote ou de um lote novo.
    unsafe fn alocar_pequeno(&mut self, k: usize) -> *mut u8 {
        unsafe {
            let cabeca = self.classes[k];
            if !cabeca.is_null() {
                self.classes[k] = (*cabeca).proximo;
                self.alocacoes += 1;
                return cabeca as *mut u8;
            }
            let t = CLASSES[k];
            let (mut atual, mut fim) = self.lote[k];
            if atual + t > fim {
                let novo = self.alocar(LOTE, ALINHAMENTO);
                if novo.is_null() {
                    // sem lote inteiro: so este bloco, direto da lista
                    return self.alocar(t, ALINHAMENTO);
                }
                // o alocar contou o lote como uma alocacao; os blocos contam
                self.alocacoes -= 1;
                (atual, fim) = (novo as usize, novo as usize + LOTE);
            }
            self.lote[k] = (atual + t, fim);
            self.alocacoes += 1;
            atual as *mut u8
        }
    }

    unsafe fn liberar_pequeno(&mut self, ponteiro: *mut u8, k: usize) {
        unsafe {
            let no = ponteiro as *mut No;
            (*no).proximo = self.classes[k];
            self.classes[k] = no;
            self.alocacoes -= 1;
        }
    }

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

/// So para o teste de estresse no PC (ferramentas/teste_heap).
#[cfg(test)]
pub fn alocar_teste(l: Layout) -> *mut u8 {
    unsafe { HEAP.alloc(l) }
}

#[cfg(test)]
pub fn liberar_teste(p: *mut u8, l: Layout) {
    unsafe { HEAP.dealloc(p, l) }
}

#[cfg(test)]
pub fn realocar_teste(p: *mut u8, l: Layout, novo: usize) -> *mut u8 {
    unsafe { HEAP.realloc(p, l, novo) }
}
