//! Estresse do alocador do kernel: milhoes de alocacoes, liberacoes e
//! realocacoes aleatorias (tamanhos de 1 byte a 64 KB, alinhamentos de 1 a
//! 4096) numa area de 64 MB. Cada bloco vivo e preenchido com um padrao
//! proprio e conferido antes de ser liberado: sobreposicao ou bloco
//! corrompido derruba o teste.
#![cfg(test)]

#[path = "../../../src/heap.rs"]
#[allow(dead_code)]
mod heap;

use std::alloc::Layout;

struct Acaso(u64);

impl Acaso {
    fn proximo(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

struct Vivo {
    p: *mut u8,
    l: Layout,
    marca: u8,
}

fn preencher(v: &Vivo) {
    unsafe { std::ptr::write_bytes(v.p, v.marca, v.l.size()) };
}

fn conferir(v: &Vivo) {
    let s = unsafe { std::slice::from_raw_parts(v.p, v.l.size()) };
    assert!(s.iter().all(|&b| b == v.marca), "bloco corrompido ({} bytes)", v.l.size());
}

#[test]
fn estresse() {
    // a area do heap (64 MB), alinhada a 4 KB
    let arena = Box::leak(vec![0u8; (64 << 20) + 4096].into_boxed_slice());
    let inicio = (arena.as_ptr() as usize + 4095) & !4095;
    heap::iniciar(inicio, inicio + (64 << 20));
    let total = heap::estatisticas().livre;

    let mut r = Acaso(0x1234_5678_9ABC_DEF1);
    let mut vivos: Vec<Vivo> = Vec::new();
    let mut marca = 0u8;
    for passo in 0..2_000_000u32 {
        let op = r.proximo() % 10;
        if op < 5 || vivos.is_empty() {
            // tamanho: quase sempre pequeno (como no JS e no CSS), as vezes grande
            let tamanho = match r.proximo() % 100 {
                0..=69 => 1 + (r.proximo() % 128) as usize,
                70..=94 => 1 + (r.proximo() % 4096) as usize,
                _ => 1 + (r.proximo() % 65536) as usize,
            };
            let alin = 1usize << (r.proximo() % 13).min(if r.proximo() % 8 == 0 { 12 } else { 4 });
            let l = Layout::from_size_align(tamanho, alin).unwrap();
            let p = heap::alocar_teste(l);
            if p.is_null() {
                continue; // cheio: segue liberando
            }
            assert_eq!(p as usize % alin, 0, "alinhamento errado");
            assert!(p as usize >= inicio && p as usize + tamanho <= inicio + (64 << 20), "fora da area");
            marca = marca.wrapping_add(1);
            let v = Vivo { p, l, marca };
            preencher(&v);
            vivos.push(v);
        } else if op < 8 {
            let i = (r.proximo() as usize) % vivos.len();
            let v = vivos.swap_remove(i);
            conferir(&v);
            heap::liberar_teste(v.p, v.l);
        } else {
            // realocar (cresce ou encolhe) e conferir que o comeco ficou
            let i = (r.proximo() as usize) % vivos.len();
            let v = &mut vivos[i];
            conferir(v);
            let novo = 1 + (r.proximo() % 8192) as usize;
            let p = heap::realocar_teste(v.p, v.l, novo);
            if p.is_null() {
                continue;
            }
            let comum = v.l.size().min(novo);
            let s = unsafe { std::slice::from_raw_parts(p, comum) };
            assert!(s.iter().all(|&b| b == v.marca), "realocar perdeu os dados");
            v.p = p;
            v.l = Layout::from_size_align(novo, v.l.align()).unwrap();
            preencher(v);
        }
        if passo % 200_000 == 0 {
            // todos os vivos intactos (pega sobreposicao entre blocos)
            for v in &vivos {
                conferir(v);
            }
        }
    }
    for v in vivos.drain(..) {
        conferir(&v);
        heap::liberar_teste(v.p, v.l);
    }
    let e = heap::estatisticas();
    println!("livre no comeco {} MB, no fim {} MB (as classes guardam lotes), {} alocacoes vivas", total >> 20, e.livre >> 20, e.alocacoes);
    assert_eq!(e.alocacoes, 0, "contagem de alocacoes nao voltou a zero");
}
