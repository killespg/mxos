/* MXOS no QEMU virt: a RAM comeca em 0x40000000, o kernel fica em 0x40080000. */
__base = 0x40080000;
INCLUDE secoes.ld
