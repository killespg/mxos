@ ============================================================
@  MXOS - entrada do kernel no ARMv7 (Cortex-A7 do RK322x)
@  O U-Boot da Rockchip pode carregar o kernel em qualquer lugar
@  da RAM: como o zImage do Linux, a primeira coisa e se copiar
@  para o endereco em que foi ligado (__kernel_start). Depois:
@  so o nucleo 0 segue, sai do modo HYP, liga o FP/NEON, instala
@  os vetores, prepara as pilhas, zera o BSS e chama kernel_main.
@ ============================================================

.section .text.boot, "ax"
.arm
.arch_extension virt
.arch_extension sec
.global _start

_start:
    @ cabecalho de zImage: alguns U-Boot conferem a assinatura em 0x24
    b       comeco
    .rept   8
    nop
    .endr
    .word   0x016F2818                  @ 0x24: assinatura do zImage
    .word   0                           @ 0x28: comeco
    .word   __bss_start - _start        @ 0x2C: fim (tamanho do arquivo)

@ Sinal de vida antes do Rust: uma faixa branca no logo que o U-Boot da
@ RK322x deixa na tela. O endereco vem do VOP (WIN0_YRGB_MST, 0x20050040):
@ e o que esta de fato no HDMI. Faixa n = 64 KB a partir de base + n * 256 KB.
@ Caches e MMU desligados: a escrita vai direto para a memoria do VOP.
.macro faixa n
    ldr     r0, =0x20050040
    ldr     r0, [r0]
    add     r0, r0, #(\n * 0x40000)
    mvn     r1, #0
    mov     r2, #0x10000
91: str     r1, [r0], #4
    subs    r2, r2, #4
    bgt     91b
.endm

comeco:
    cpsid   aif
    mrc     p15, 0, r4, c0, c0, 5       @ MPIDR: qual nucleo
    ands    r4, r4, #3
    bne     estacionar
    faixa   1

    @ ---- HYP -> SVC (o U-Boot pode entregar em HYP) ----
    mrs     r0, cpsr
    and     r1, r0, #0x1F
    cmp     r1, #0x1A
    bne     em_svc
    mov     r1, #0x1D3                  @ SVC com A, I e F mascaradas
    msr     spsr_hyp, r1
    adr     r1, em_svc
    msr     elr_hyp, r1
    eret

em_svc:
    cps     #0x13
    @ MMU e cache de dados desligados (o Linux pede o mesmo do U-Boot)
    mrc     p15, 0, r0, c1, c0, 0
    bic     r0, r0, #0x1                @ M
    bic     r0, r0, #0x4                @ C
    mcr     p15, 0, r0, c1, c0, 0
    isb
    faixa   2

    @ ---- copia para o endereco certo, se veio parar em outro ----
    adr     r0, _start                  @ onde estamos
    ldr     r1, =__kernel_start         @ onde fomos ligados
    cmp     r0, r1
    beq     no_lugar
    ldr     r2, =__bss_start
    sub     r2, r2, r1                  @ bytes do arquivo
    @ os dois lugares podem se sobrepor: destino acima da origem copia de
    @ tras para frente; abaixo, da frente para tras (como o memmove)
    cmp     r1, r0
    bhi     3f
1:  ldr     r12, [r0], #4
    str     r12, [r1], #4
    subs    r2, r2, #4
    bgt     1b
    b       4f
3:  add     r0, r0, r2
    add     r3, r1, r2
5:  ldr     r12, [r0, #-4]!
    str     r12, [r3, #-4]!
    subs    r2, r2, #4
    bgt     5b
4:  mov     r12, #0
    mcr     p15, 0, r12, c7, c5, 0      @ ICIALLU
    dsb
    isb
    ldr     pc, =no_lugar

no_lugar:
    @ FP/NEON: CPACR cp10/cp11 e FPEXC.EN (o Rust usa os registradores d)
    mrc     p15, 0, r0, c1, c0, 2
    orr     r0, r0, #(0xF << 20)
    mcr     p15, 0, r0, c1, c0, 2
    isb
    mov     r0, #0x40000000
    vmsr    fpexc, r0

    @ vetores no VBAR (SCTLR.V = 0)
    ldr     r0, =vetores
    mcr     p15, 0, r0, c12, c0, 0
    mrc     p15, 0, r0, c1, c0, 0
    bic     r0, r0, #0x2000
    mcr     p15, 0, r0, c1, c0, 0
    isb

    @ pilhas: IRQ, abort e undefined na area de excecao; o kernel em SVC
    ldr     r0, =__pilha_excecao_topo
    cps     #0x12                       @ IRQ
    mov     sp, r0
    sub     r0, r0, #0x2000
    cps     #0x17                       @ abort
    mov     sp, r0
    cps     #0x1B                       @ undefined
    mov     sp, r0
    cps     #0x13                       @ SVC
    ldr     sp, =__stack_top

    @ zera o BSS
    ldr     r0, =__bss_start
    ldr     r1, =__bss_end
    mov     r2, #0
2:  cmp     r0, r1
    strlo   r2, [r0], #4
    blo     2b

    faixa   3
    mov     r0, #1                      @ kernel_main(el_boot = 1)
    mov     r1, #0
    bl      kernel_main

@ os outros nucleos ainda nao sao ligados no ARMv7 (smp.rs)
.global _start_secundario
_start_secundario:
estacionar:
    wfe
    b       estacionar

.ltorg
