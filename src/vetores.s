// ============================================================
//  MXOS - tabela de vetores de excecao (EL1)
//  16 entradas de 128 bytes, alinhada em 2 KB.
//  - IRQ vinda do kernel: salva os registradores, chama
//    tratar_irq (irq.rs) e volta exatamente de onde parou.
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
    // programas em EL0 (ainda nao existem)
    FATAL 8
    FATAL 9
    FATAL 10
    FATAL 11
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
