@ ============================================================
@  MXOS - vetores de excecao do ARMv7
@  - IRQ no kernel: salva o que uma funcao pode estragar (r0-r3,
@    r12, lr, d0-d7, d16-d31 e o FPSCR), chama tratar_irq (irq.rs)
@    e volta.
@  - qualquer excecao vinda de um programa (modo usuario): SVC
@    (chamada de sistema), IRQ (a preempcao), falhas de dados e de
@    busca, instrucao desconhecida. O programa inteiro vai para o
@    Contexto dele e o kernel volta do entrar_usuario com o motivo.
@  - o resto vindo do kernel e fatal: tratar_excecao_arm
@    (excecao.rs) explica o erro na serial e na tela e para.
@
@  Contexto (processo/mod.rs): r0-r14 (0), pc (60), cpsr (64), tls
@  (68), &ContextoKernel (72), status (76) e endereco (80) da falha,
@  fpscr (84), d0-d31 (88). ContextoKernel: r4-r11 (0), sp (32),
@  lr (36), d8-d15 (40), fpscr (104).
@
@  Enquanto o programa roda, a pilha do modo SVC aponta para o
@  Contexto + 68: o "srsdb sp!, #0x13" de qualquer modo grava pc e
@  cpsr do programa nos lugares certos (60 e 64) e o
@  "stmdb sp, {r0-r14}^" grava os registradores dele em 0..59.
@ ============================================================

.section .text.vetores, "ax"
.arm
.arch_extension virt
.arch_extension sec
.balign 32
.global vetores
vetores:
    b       fatal_reset
    b       instrucao_entrada
    b       svc_entrada
    b       busca_entrada
    b       dados_entrada
    b       .
    b       irq_entrada
    b       fatal_fiq

@ ---------------------------------------------------------------
@ IRQ: do programa (preempcao) ou do kernel (atende aqui mesmo)
irq_entrada:
    sub     lr, lr, #4
    push    {r0}
    mrs     r0, spsr
    and     r0, r0, #0x1F
    cmp     r0, #0x10
    pop     {r0}
    beq     usuario_irq
    push    {r0-r3, r12, lr}
    ldr     r0, =ULTIMO_PC_IRQ          @ onde a CPU estava (detector de travamento)
    str     lr, [r0]
    ldr     r0, =ULTIMO_FP_IRQ          @ e o frame pointer dela (r11): a pilha de chamadas
    str     r11, [r0]
    vpush   {d0-d7}
    vpush   {d16-d31}                   @ tambem temporarios no AAPCS-VFP (d32)
    vmrs    r0, fpscr
    push    {r0, r1}                    @ r1 so mantem a pilha em 8 bytes
    bl      tratar_irq
    pop     {r0, r1}
    vmsr    fpscr, r0
    vpop    {d16-d31}
    vpop    {d0-d7}
    pop     {r0-r3, r12, lr}
    movs    pc, lr                      @ volta, com o CPSR do SPSR

usuario_irq:
    srsdb   sp!, #0x13                  @ pc e cpsr do programa no Contexto
    cps     #0x13
    stmdb   sp, {r0-r14}^               @ r0-r14 do programa
    nop
    sub     sp, sp, #60                 @ sp = Contexto
    mov     r0, #1
    b       sair_usuario

@ ---------------------------------------------------------------
@ SVC: so os programas chamam (o kernel nunca faz SVC)
svc_entrada:
    srsdb   sp!, #0x13                  @ lr = a instrucao depois do SVC
    stmdb   sp, {r0-r14}^
    nop
    sub     sp, sp, #60
    mov     r0, #0
    b       sair_usuario

@ ---------------------------------------------------------------
@ Falhas e instrucao desconhecida: o sp do modo (ABT/UND) guarda o r0
@ enquanto o SPSR diz de onde veio; o caminho fatal acerta o sp depois.
dados_entrada:
    mov     sp, r0
    mrs     r0, spsr
    and     r0, r0, #0x1F
    cmp     r0, #0x10
    mov     r0, sp
    bne     fatal_dados
    sub     lr, lr, #8                  @ a instrucao que falhou (repete depois)
    srsdb   sp!, #0x13
    cps     #0x13
    stmdb   sp, {r0-r14}^
    nop
    sub     sp, sp, #60
    mov     r0, #2
    b       sair_usuario

busca_entrada:
    mov     sp, r0
    mrs     r0, spsr
    and     r0, r0, #0x1F
    cmp     r0, #0x10
    mov     r0, sp
    bne     fatal_busca
    sub     lr, lr, #4
    srsdb   sp!, #0x13
    cps     #0x13
    stmdb   sp, {r0-r14}^
    nop
    sub     sp, sp, #60
    mov     r0, #3
    b       sair_usuario

instrucao_entrada:
    mov     sp, r0
    mrs     r0, spsr
    and     r0, r0, #0x1F
    cmp     r0, #0x10
    mov     r0, sp
    bne     fatal_instrucao
    sub     lr, lr, #4                  @ a instrucao (ARM, 4 bytes)
    srsdb   sp!, #0x13
    cps     #0x13
    stmdb   sp, {r0-r14}^
    nop
    sub     sp, sp, #60
    mov     r0, #4
    b       sair_usuario

@ ---------------------------------------------------------------
@ sair_usuario: modo SVC, IRQ desligada, sp = Contexto, r0 = motivo.
@ Termina de salvar o programa (VFP, falha) e volta para o kernel como
@ se o entrar_usuario tivesse retornado r0.
sair_usuario:
    add     r1, sp, #88
    vstmia  r1!, {d0-d15}
    vstmia  r1, {d16-d31}
    vmrs    r1, fpscr
    str     r1, [sp, #84]
    cmp     r0, #2
    bne     1f
    mrc     p15, 0, r1, c5, c0, 0       @ DFSR
    mrc     p15, 0, r2, c6, c0, 0       @ DFAR
    str     r1, [sp, #76]
    str     r2, [sp, #80]
1:  cmp     r0, #3
    bne     2f
    mrc     p15, 0, r1, c5, c0, 1       @ IFSR
    mrc     p15, 0, r2, c6, c0, 2       @ IFAR
    str     r1, [sp, #76]
    str     r2, [sp, #80]
2:  ldr     r1, [sp, #72]               @ ContextoKernel
    add     r2, r1, #40
    vldmia  r2, {d8-d15}
    ldr     r2, [r1, #104]
    vmsr    fpscr, r2
    ldr     sp, [r1, #32]
    ldr     lr, [r1, #36]
    ldmia   r1, {r4-r11}
    bx      lr

@ ---------------------------------------------------------------
@ entrar_usuario(r0 = &Contexto, r1 = &ContextoKernel) -> motivo
@ Chamar com IRQ desligada. Guarda o kernel, carrega o programa e
@ pula para ele em modo usuario.
.section .text.processo, "ax"
.global entrar_usuario
entrar_usuario:
    stmia   r1, {r4-r11}
    str     sp, [r1, #32]
    str     lr, [r1, #36]
    add     r2, r1, #40
    vstmia  r2, {d8-d15}
    vmrs    r2, fpscr
    str     r2, [r1, #104]
    str     r1, [r0, #72]
    ldr     r2, [r0, #68]
    mcr     p15, 0, r2, c13, c0, 3      @ TPIDRURO: o TLS do programa
    add     r2, r0, #88
    vldmia  r2!, {d0-d15}
    vldmia  r2, {d16-d31}
    ldr     r2, [r0, #84]
    vmsr    fpscr, r2
    ldr     r2, [r0, #64]
    msr     spsr_cxsf, r2
    mov     sp, r0
    ldmia   sp, {r0-r14}^               @ os registradores do programa
    nop
    ldr     lr, [sp, #60]
    add     sp, sp, #68                 @ os vetores salvam o programa a partir daqui
    movs    pc, lr                      @ pc e CPSR (do SPSR): modo usuario

.section .text.vetores, "ax"
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

.ltorg
