//! Saida de som (como o crate::audio::dsp do sistema): PCM de 16 bits,
//! intercalado por canal.

pub mod dsp {
    use core::sync::atomic::{AtomicU32, Ordering};

    use crate::sys::{self, chamada};

    static TAXA: AtomicU32 = AtomicU32::new(48000);
    static CANAIS: AtomicU32 = AtomicU32::new(2);

    fn formato() -> isize {
        let (t, c) = (TAXA.load(Ordering::Relaxed), CANAIS.load(Ordering::Relaxed));
        chamada(sys::SOM_FORMATO, [t as usize, c as usize, 0, 0, 0, 0])
    }

    /// Liga a saida de som. false = nao ha placa de som.
    pub fn abrir() -> bool {
        formato() > 0
    }

    pub fn fechar() {
        chamada(sys::SOM_FORMATO, [0; 6]);
    }

    /// A taxa pedida, se a placa aceita; senao a dela.
    pub fn taxa(pedida: u32) -> u32 {
        TAXA.store(pedida, Ordering::Relaxed);
        let r = formato();
        if r > 0 {
            TAXA.store(r as u32, Ordering::Relaxed);
        }
        TAXA.load(Ordering::Relaxed)
    }

    pub fn canais(pedidos: u32) -> u32 {
        CANAIS.store(pedidos.clamp(1, 2), Ordering::Relaxed);
        formato();
        CANAIS.load(Ordering::Relaxed)
    }

    /// Manda o PCM; devolve quantos bytes couberam.
    pub fn escrever(dados: &[u8]) -> usize {
        chamada(sys::SOM, [dados.as_ptr() as usize, dados.len(), 0, 0, 0, 0]).max(0) as usize
    }
}
