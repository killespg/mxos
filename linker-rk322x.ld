/* MXOS no RK322x: o kernel roda aqui (o boot_arm.s se copia para ca se o
   U-Boot da Rockchip carregou em outro lugar da RAM, que comeca em 0x60000000). */
__base = 0x60200000;
INCLUDE secoes.ld
