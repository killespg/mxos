//! Alocador dos apps: classes de tamanho em potencias de 2 (16 B a 64 KB)
//! com uma lista de livres cada, tiradas de lotes que o MAPEAR entrega;
//! o que passa de 64 KB vai direto para o MAPEAR/DESMAPEAR. Os apps tem
//! uma linha de execucao so: sem trava.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ptr;

use crate::sys;

const MENOR: usize = 16;
const CLASSES: usize = 13; // 16 << 12 = 64 KB
const MAIOR: usize = MENOR << (CLASSES - 1);
/// Cada classe pede memoria em lotes deste tamanho (ou de um bloco, se maior).
const LOTE: usize = 64 * 1024;
const PAGINA: usize = 4096;

struct Estado {
    livres: [*mut u8; CLASSES],
}

pub struct Heap(UnsafeCell<Estado>);

// Uma linha de execucao por app.
unsafe impl Sync for Heap {}

#[global_allocator]
static HEAP: Heap = Heap(UnsafeCell::new(Estado { livres: [ptr::null_mut(); CLASSES] }));

fn classe(tamanho: usize) -> usize {
    let t = tamanho.max(MENOR).next_power_of_two();
    (t.trailing_zeros() - MENOR.trailing_zeros()) as usize
}

fn grande(layout: &Layout) -> bool {
    layout.size().max(layout.align()) > MAIOR || layout.align() > PAGINA
}

unsafe impl GlobalAlloc for Heap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if grande(&layout) {
            if layout.align() > PAGINA {
                return ptr::null_mut();
            }
            return sys::mapear(layout.size().next_multiple_of(PAGINA)).unwrap_or(ptr::null_mut());
        }
        let k = classe(layout.size().max(layout.align()));
        let e = unsafe { &mut *self.0.get() };
        if e.livres[k].is_null() {
            // lote novo, cortado em blocos do tamanho da classe (alinhados
            // no proprio tamanho: o lote comeca numa pagina)
            let bloco = MENOR << k;
            let tamanho = LOTE.max(bloco);
            let Some(base) = sys::mapear(tamanho) else { return ptr::null_mut() };
            let mut p = base as usize + tamanho;
            while p > base as usize {
                p -= bloco;
                unsafe { *(p as *mut *mut u8) = e.livres[k] };
                e.livres[k] = p as *mut u8;
            }
        }
        let p = e.livres[k];
        e.livres[k] = unsafe { *(p as *mut *mut u8) };
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        if grande(&layout) {
            sys::desmapear(p, layout.size().next_multiple_of(PAGINA));
            return;
        }
        let k = classe(layout.size().max(layout.align()));
        let e = unsafe { &mut *self.0.get() };
        unsafe { *(p as *mut *mut u8) = e.livres[k] };
        e.livres[k] = p;
    }

    unsafe fn realloc(&self, p: *mut u8, layout: Layout, novo: usize) -> *mut u8 {
        let mesma_classe = !grande(&layout)
            && novo.max(layout.align()) <= MAIOR
            && classe(layout.size().max(layout.align())) == classe(novo.max(layout.align()));
        if mesma_classe {
            return p;
        }
        let novo_layout = unsafe { Layout::from_size_align_unchecked(novo, layout.align()) };
        let q = unsafe { self.alloc(novo_layout) };
        if !q.is_null() {
            unsafe {
                ptr::copy_nonoverlapping(p, q, layout.size().min(novo));
                self.dealloc(p, layout);
            }
        }
        q
    }
}
