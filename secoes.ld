/*
 * MXOS - secoes do kernel, comum ao QEMU e a TV box.
 * Quem inclui este arquivo define __base antes (linker.ld / linker-mxq.ld).
 *
 * Cada regiao comeca numa pagina de 4 KB nova, porque a MMU da permissao
 * por pagina: codigo (le e executa), constantes (so le), dados (le e escreve).
 */
ENTRY(_start)

SECTIONS
{
    . = __base;
    __kernel_start = .;

    .text : {
        KEEP(*(.text.boot))     /* cabecalho + _start precisam vir primeiro */
        *(.text .text.*)
    }
    . = ALIGN(4096);
    __text_end = .;

    .rodata : { *(.rodata .rodata.*) }
    . = ALIGN(4096);
    __rodata_end = .;

    .data : ALIGN(8) { *(.data .data.*) }

    .bss (NOLOAD) : ALIGN(16) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    }

    /* guarda: fica sem mapeamento, entao um estouro de pilha vira uma
       excecao em vez de corromper o BSS em silencio. 1 MB alinhado: o
       ARMv7 mapeia em secoes de 1 MB (o AArch64 tira so a primeira pagina) */
    . = ALIGN(0x100000);
    __guarda = .;
    . += 0x100000;

    /* pilha do kernel: 2 MB (o parser e o interpretador de JS sao
       recursivos; com 64 KB o JS do Google estourava na TV box) */
    __stack_bottom = .;
    . += 0x200000;
    __stack_top = .;

    /* pilha separada para tratar excecoes (inclusive o estouro da outra) */
    . += 0x4000;
    __pilha_excecao_topo = .;

    __kernel_end = .;
    __image_size = __kernel_end - __kernel_start;

    /DISCARD/ : { *(.comment) *(.note .note.*) *(.eh_frame*) }
}
