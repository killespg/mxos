//! Enderecos das regioes do kernel, definidos pelo secoes.ld.

unsafe extern "C" {
    static __kernel_start: u8;
    static __text_end: u8;
    static __rodata_end: u8;
    static __guarda: u8;
    static __stack_bottom: u8;
    static __stack_top: u8;
    static __pilha_excecao_topo: u8;
    static __kernel_end: u8;
}

pub struct Mapa {
    pub inicio: usize,
    pub codigo_fim: usize,
    pub constantes_fim: usize,
    pub guarda: usize,
    pub pilha_inicio: usize,
    pub pilha_fim: usize,
    pub pilha_excecao_fim: usize,
    pub fim: usize,
}

pub fn kernel() -> Mapa {
    Mapa {
        inicio: &raw const __kernel_start as usize,
        codigo_fim: &raw const __text_end as usize,
        constantes_fim: &raw const __rodata_end as usize,
        guarda: &raw const __guarda as usize,
        pilha_inicio: &raw const __stack_bottom as usize,
        pilha_fim: &raw const __stack_top as usize,
        pilha_excecao_fim: &raw const __pilha_excecao_topo as usize,
        fim: &raw const __kernel_end as usize,
    }
}
