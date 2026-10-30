//! So o que o gui::grafico do sistema usa da tela: uma Tela por cima de
//! memoria comum (a Superficie::tela e as copias de apresentar).

pub mod tela {
    pub type Cor = u32;

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Formato {
        Xrgb8888,
    }

    pub struct Tela {
        base: usize,
        largura: usize,
        altura: usize,
        passo: usize,
    }

    impl Tela {
        /// # Safety
        /// `base` aponta para `altura` linhas de `passo` bytes validas
        /// enquanto a Tela existir.
        pub unsafe fn new(base: usize, largura: usize, altura: usize, passo: usize, _formato: Formato) -> Tela {
            Tela { base, largura, altura, passo }
        }

        pub fn largura(&self) -> usize {
            self.largura
        }

        pub fn altura(&self) -> usize {
            self.altura
        }

        pub fn base(&self) -> usize {
            self.base
        }

        pub fn passo(&self) -> usize {
            self.passo
        }

        pub fn formato(&self) -> Formato {
            Formato::Xrgb8888
        }

        pub fn escrever_cores(&self, x: usize, y: usize, cores: &[Cor]) {
            if y >= self.altura || x >= self.largura {
                return;
            }
            let n = cores.len().min(self.largura - x);
            let linha = (self.base + y * self.passo) as *mut u32;
            unsafe { core::ptr::copy_nonoverlapping(cores.as_ptr(), linha.add(x), n) };
        }

        pub fn limpar_cache_area(&self, _x: usize, _y: usize, _largura: usize, _altura: usize) {}
    }
}
