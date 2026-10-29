// ============================================================
//  MXOS - tabela de vetores de excecao (EL1)
//  16 entradas de 128 bytes, alinhada em 2 KB.
//  - IRQ vinda do kernel: salva os registradores, chama
//    tratar_irq (irq.rs) e volta exatamente de onde parou.
//  - qualquer excecao vinda de um programa (EL0, 64 bits): salva
//    o programa inteiro no Contexto dele e volta para o kernel como
//    se entrar_usuario tivesse retornado (src/processo/).
//  - todo o resto e fatal: tratar_excecao (excecao.rs) explica
//    o erro na serial e para.
// ============================================================

.section .text.vetores, "ax"

.macro FATAL tipo
    .balign 0x80
    mov     x0, #\tipo
    mrs     x1, esr_el1             // motivo
    mrs     x2, elr_el1             // endereco da instrucao
    mrs     x3, far_el1             // endereco acessado (em falhas de memoria)
    b       tratar_excecao
.endm

.macro INTERRUPCAO
    .balign 0x80
    b       irq_entrada
.endm

// Excecao vinda do programa: x0 e x1 dele vao para a pilha de excecao
// (vazia neste momento) e o motivo segue em x1.
.macro DO_PROGRAMA motivo
    .balign 0x80
    stp     x0, x1, [sp, #-16]!
    mov     x1, #\motivo
    b       sair_do_usuario
.endm

.balign 2048
.global vetores
vetores:
    // kernel rodando na pilha normal (SP_EL0): onde o MXOS passa o tempo todo
    FATAL 0                         // sincrona
    INTERRUPCAO                     // IRQ
    FATAL 2                         // FIQ
    FATAL 3                         // SError
    // kernel na pilha de excecao (SP_EL1): IRQ aqui seria um bug
    FATAL 4
    FATAL 5
    FATAL 6
    FATAL 7
    // programas em EL0, 64 bits: sincrona (SVC, falhas), IRQ, FIQ, SError
    DO_PROGRAMA 0
    DO_PROGRAMA 1
    DO_PROGRAMA 2
    DO_PROGRAMA 3
    // programas de 32 bits nunca rodam no MXOS
    FATAL 12
    FATAL 13
    FATAL 14
    FATAL 15

// A IRQ pode chegar no meio de qualquer instrucao do kernel. Antes de chamar
// codigo Rust, salva tudo que uma funcao pode estragar: x0-x18, x29, x30,
// q0-q31 e os registradores de estado (ELR, SPSR, FPCR, FPSR).
// q8-q15 tambem vao inteiros: a funcao chamada so preserva a metade de
// baixo (d8-d15), e o codigo interrompido pode estar usando os 128 bits.
// Roda na pilha de excecao (SP_EL1).
.equ QUADRO, 720

irq_entrada:
    sub     sp, sp, #QUADRO
    stp     x0, x1, [sp, #0]
    stp     x2, x3, [sp, #16]
    stp     x4, x5, [sp, #32]
    stp     x6, x7, [sp, #48]
    stp     x8, x9, [sp, #64]
    stp     x10, x11, [sp, #80]
    stp     x12, x13, [sp, #96]
    stp     x14, x15, [sp, #112]
    stp     x16, x17, [sp, #128]
    stp     x18, x29, [sp, #144]
    mrs     x0, elr_el1
    stp     x30, x0, [sp, #160]
    mrs     x0, spsr_el1
    mrs     x1, fpcr
    stp     x0, x1, [sp, #176]
    mrs     x0, fpsr
    str     x0, [sp, #192]
    stp     q0, q1, [sp, #208]
    stp     q2, q3, [sp, #240]
    stp     q4, q5, [sp, #272]
    stp     q6, q7, [sp, #304]
    stp     q8, q9, [sp, #336]
    stp     q10, q11, [sp, #368]
    stp     q12, q13, [sp, #400]
    stp     q14, q15, [sp, #432]
    stp     q16, q17, [sp, #464]
    stp     q18, q19, [sp, #496]
    stp     q20, q21, [sp, #528]
    stp     q22, q23, [sp, #560]
    stp     q24, q25, [sp, #592]
    stp     q26, q27, [sp, #624]
    stp     q28, q29, [sp, #656]
    stp     q30, q31, [sp, #688]

    bl      tratar_irq

    ldp     q30, q31, [sp, #688]
    ldp     q28, q29, [sp, #656]
    ldp     q26, q27, [sp, #624]
    ldp     q24, q25, [sp, #592]
    ldp     q22, q23, [sp, #560]
    ldp     q20, q21, [sp, #528]
    ldp     q18, q19, [sp, #496]
    ldp     q16, q17, [sp, #464]
    ldp     q14, q15, [sp, #432]
    ldp     q12, q13, [sp, #400]
    ldp     q10, q11, [sp, #368]
    ldp     q8, q9, [sp, #336]
    ldp     q6, q7, [sp, #304]
    ldp     q4, q5, [sp, #272]
    ldp     q2, q3, [sp, #240]
    ldp     q0, q1, [sp, #208]
    ldr     x0, [sp, #192]
    msr     fpsr, x0
    ldp     x0, x1, [sp, #176]
    msr     spsr_el1, x0
    msr     fpcr, x1
    ldp     x30, x0, [sp, #160]
    msr     elr_el1, x0
    ldp     x18, x29, [sp, #144]
    ldp     x16, x17, [sp, #128]
    ldp     x14, x15, [sp, #112]
    ldp     x12, x13, [sp, #96]
    ldp     x10, x11, [sp, #80]
    ldp     x8, x9, [sp, #64]
    ldp     x6, x7, [sp, #48]
    ldp     x4, x5, [sp, #32]
    ldp     x2, x3, [sp, #16]
    ldp     x0, x1, [sp, #0]
    add     sp, sp, #QUADRO
    eret

// ============================================================
//  Processos (src/processo/): o kernel "chama" o programa.
//
//  entrar_usuario(x0 = &Contexto do processo, x1 = &ContextoKernel)
//  guarda os registradores que o kernel precisa de volta (x19-x30, sp,
//  d8-d15, FPCR), carrega o programa inteiro e faz eret para EL0.
//  Quando o programa faz SVC, recebe uma interrupcao ou erra, o vetor
//  DO_PROGRAMA cai em sair_do_usuario, que salva o programa no Contexto
//  e restaura o kernel: entrar_usuario "retorna" com o motivo em x0
//  (0 sincrona, 1 IRQ, 2 FIQ, 3 SError). Chamar com IRQ desligada.
//
//  Contexto: x0-x30 (0), sp (248), pc (256), spsr (264), tpidr_el0 (272),
//  fpcr (280), fpsr (288), &ContextoKernel (296), esr (304), far (312),
//  q0-q31 (320). ContextoKernel: x19-x30 (0), sp (96), d8-d15 (104),
//  fpcr (168).
// ============================================================

.section .text.processo, "ax"
.global entrar_usuario
entrar_usuario:
    stp     x19, x20, [x1, #0]
    stp     x21, x22, [x1, #16]
    stp     x23, x24, [x1, #32]
    stp     x25, x26, [x1, #48]
    stp     x27, x28, [x1, #64]
    stp     x29, x30, [x1, #80]
    mov     x9, sp
    str     x9, [x1, #96]
    stp     d8, d9, [x1, #104]
    stp     d10, d11, [x1, #120]
    stp     d12, d13, [x1, #136]
    stp     d14, d15, [x1, #152]
    mrs     x9, fpcr
    str     x9, [x1, #168]
    str     x1, [x0, #296]
    msr     tpidr_el1, x0           // o vetor acha o Contexto por aqui

    // "sp" passa a ser o SP_EL1 (vazio): o SP_EL0 vira a pilha do programa
    msr     spsel, #1
    ldr     x9, [x0, #248]
    msr     sp_el0, x9
    ldp     x9, x10, [x0, #256]
    msr     elr_el1, x9
    msr     spsr_el1, x10
    ldp     x9, x10, [x0, #272]
    msr     tpidr_el0, x9
    msr     fpcr, x10
    ldr     x9, [x0, #288]
    msr     fpsr, x9
    add     x9, x0, #320
    ldp     q0, q1, [x9, #0]
    ldp     q2, q3, [x9, #32]
    ldp     q4, q5, [x9, #64]
    ldp     q6, q7, [x9, #96]
    ldp     q8, q9, [x9, #128]
    ldp     q10, q11, [x9, #160]
    ldp     q12, q13, [x9, #192]
    ldp     q14, q15, [x9, #224]
    ldp     q16, q17, [x9, #256]
    ldp     q18, q19, [x9, #288]
    ldp     q20, q21, [x9, #320]
    ldp     q22, q23, [x9, #352]
    ldp     q24, q25, [x9, #384]
    ldp     q26, q27, [x9, #416]
    ldp     q28, q29, [x9, #448]
    ldp     q30, q31, [x9, #480]
    ldp     x2, x3, [x0, #16]
    ldp     x4, x5, [x0, #32]
    ldp     x6, x7, [x0, #48]
    ldp     x8, x9, [x0, #64]
    ldp     x10, x11, [x0, #80]
    ldp     x12, x13, [x0, #96]
    ldp     x14, x15, [x0, #112]
    ldp     x16, x17, [x0, #128]
    ldp     x18, x19, [x0, #144]
    ldp     x20, x21, [x0, #160]
    ldp     x22, x23, [x0, #176]
    ldp     x24, x25, [x0, #192]
    ldp     x26, x27, [x0, #208]
    ldp     x28, x29, [x0, #224]
    ldr     x30, [x0, #240]
    ldp     x0, x1, [x0, #0]
    eret

// Vem do DO_PROGRAMA, na pilha de excecao, com x0/x1 do programa nela.
sair_do_usuario:
    mrs     x0, tpidr_el1
    stp     x2, x3, [x0, #16]
    stp     x4, x5, [x0, #32]
    stp     x6, x7, [x0, #48]
    stp     x8, x9, [x0, #64]
    stp     x10, x11, [x0, #80]
    stp     x12, x13, [x0, #96]
    stp     x14, x15, [x0, #112]
    stp     x16, x17, [x0, #128]
    stp     x18, x19, [x0, #144]
    stp     x20, x21, [x0, #160]
    stp     x22, x23, [x0, #176]
    stp     x24, x25, [x0, #192]
    stp     x26, x27, [x0, #208]
    stp     x28, x29, [x0, #224]
    str     x30, [x0, #240]
    ldp     x2, x3, [sp], #16       // x0 e x1 do programa (a pilha volta a ficar vazia)
    stp     x2, x3, [x0, #0]
    mrs     x2, sp_el0
    str     x2, [x0, #248]
    mrs     x2, elr_el1
    mrs     x3, spsr_el1
    stp     x2, x3, [x0, #256]
    mrs     x2, tpidr_el0
    mrs     x3, fpcr
    stp     x2, x3, [x0, #272]
    mrs     x2, fpsr
    str     x2, [x0, #288]
    mrs     x2, esr_el1
    mrs     x3, far_el1
    stp     x2, x3, [x0, #304]
    add     x2, x0, #320
    stp     q0, q1, [x2, #0]
    stp     q2, q3, [x2, #32]
    stp     q4, q5, [x2, #64]
    stp     q6, q7, [x2, #96]
    stp     q8, q9, [x2, #128]
    stp     q10, q11, [x2, #160]
    stp     q12, q13, [x2, #192]
    stp     q14, q15, [x2, #224]
    stp     q16, q17, [x2, #256]
    stp     q18, q19, [x2, #288]
    stp     q20, q21, [x2, #320]
    stp     q22, q23, [x2, #352]
    stp     q24, q25, [x2, #384]
    stp     q26, q27, [x2, #416]
    stp     q28, q29, [x2, #448]
    stp     q30, q31, [x2, #480]

    // de volta ao kernel, logo depois da chamada a entrar_usuario
    ldr     x9, [x0, #296]
    ldp     x19, x20, [x9, #0]
    ldp     x21, x22, [x9, #16]
    ldp     x23, x24, [x9, #32]
    ldp     x25, x26, [x9, #48]
    ldp     x27, x28, [x9, #64]
    ldp     x29, x30, [x9, #80]
    ldp     d8, d9, [x9, #104]
    ldp     d10, d11, [x9, #120]
    ldp     d12, d13, [x9, #136]
    ldp     d14, d15, [x9, #152]
    ldr     x10, [x9, #168]
    msr     fpcr, x10
    ldr     x10, [x9, #96]
    msr     spsel, #0               // "sp" volta a ser a pilha do kernel (SP_EL0)
    mov     sp, x10
    mov     x0, x1                  // motivo
    ret
