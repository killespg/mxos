@ ============================================================
@  MXOS - vetores de excecao do ARMv7
@  - IRQ: salva o que uma funcao pode estragar (r0-r3, r12, lr,
@    d0-d7 e o FPSCR), chama tratar_irq (irq.rs) e volta.
@  - o resto e fatal: tratar_excecao_arm (excecao.rs) explica o
@    erro na serial e na tela e para.
@  Processos (EL0) ainda nao rodam no ARMv7: entrar_usuario so
@  devolve "erro" para o kernel compilar igual.
@ ============================================================

.section .text.vetores, "ax"
.arm
.arch_extension virt
.arch_extension sec
.balign 32
.global vetores
vetores:
    b       fatal_reset
    b       fatal_instrucao
    b       fatal_svc
    b       fatal_busca
    b       fatal_dados
    b       .
    b       irq_entrada
    b       fatal_fiq

irq_entrada:
    sub     lr, lr, #4
    push    {r0-r3, r12, lr}
    ldr     r0, =ULTIMO_PC_IRQ          @ onde a CPU estava (detector de travamento)
    str     lr, [r0]
    vpush   {d0-d7}
    vmrs    r0, fpscr
    push    {r0, r1}                    @ r1 so mantem a pilha em 8 bytes
    bl      tratar_irq
    pop     {r0, r1}
    vmsr    fpscr, r0
    vpop    {d0-d7}
    pop     {r0-r3, r12, lr}
    movs    pc, lr                      @ volta, com o CPSR do SPSR

fatal_reset:
    mov     r0, #0
    mov     r1, #0
    mov     r2, #0
    b       fatal
fatal_instrucao:
    mov     r0, #1
    mov     r1, #0
    sub     r2, lr, #4
    b       fatal
fatal_svc:
    mov     r0, #2
    mov     r1, #0
    sub     r2, lr, #4
    b       fatal
fatal_busca:
    mov     r0, #3
    mrc     p15, 0, r1, c5, c0, 1       @ IFSR
    sub     r2, lr, #4
    mrc     p15, 0, r3, c6, c0, 2       @ IFAR
    b       fatal_far
fatal_dados:
    mov     r0, #4
    mrc     p15, 0, r1, c5, c0, 0       @ DFSR
    sub     r2, lr, #8
    mrc     p15, 0, r3, c6, c0, 0       @ DFAR
    b       fatal_far
fatal_fiq:
    mov     r0, #7
    mov     r1, #0
    sub     r2, lr, #4
fatal:
    mov     r3, #0
fatal_far:
    @ numa pilha boa (a de excecao), em SVC, sem interrupcoes
    cpsid   aif
    ldr     sp, =__pilha_excecao_topo
    sub     sp, sp, #0x3000
    b       tratar_excecao_arm

@ entrar_usuario(contexto, kernel): ainda nao ha processos no ARMv7
.global entrar_usuario
entrar_usuario:
    mov     r0, #3
    bx      lr

.ltorg
