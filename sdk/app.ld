/* Apps do MXOS: executavel estatico a partir de 64 KB. Cada grupo de
   permissao (codigo, so leitura, dados) comeca numa pagina nova: o
   carregador (processo/elf.rs) mapeia pagina por pagina. */
ENTRY(_start)

SECTIONS
{
    . = 0x10000;
    .text : {
        KEEP(*(.text._start))
        *(.text .text.*)
    }
    . = ALIGN(4096);
    .rodata : {
        *(.rodata .rodata.*)
    }
    .ARM.exidx : { *(.ARM.exidx .ARM.exidx.*) }
    .eh_frame : { *(.eh_frame) }
    . = ALIGN(4096);
    .data : {
        *(.data .data.*)
        *(.got .got.*)
    }
    .bss : {
        *(.bss .bss.*)
        *(COMMON)
    }
    /DISCARD/ : {
        *(.comment)
        *(.note .note.*)
    }
}
