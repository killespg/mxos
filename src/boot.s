// ============================================================
//  MXOS - entrada do kernel (ARM64)
//  Roda antes de qualquer codigo Rust: escolhe o nucleo 0,
//  desce de EL2 para EL1, liga o FP, instala os vetores de
//  excecao, prepara as pilhas, zera o BSS e chama kernel_main.
// ============================================================

.section .text.boot, "ax"
.global _start

_start:
    // Cabecalho "Image" do Linux ARM64. O comando "booti" do U-Boot
    // so aceita o arquivo se ele tiver este formato.
    b       inicio                  // code0: pula o cabecalho
    .long   0                       // code1
    .quad   0x80000                 // text_offset
    .quad   __image_size            // tamanho na memoria (com BSS e pilha)
    .quad   0xA                     // flags: little endian, pagina 4K, pode ir em qualquer lugar
    .quad   0, 0, 0                 // reservado
    .ascii  "ARM\x64"               // assinatura
    .long   0                       // reservado (offset do PE/COFF)

inicio:
    // Os 4 nucleos podem chegar aqui. So o nucleo 0 segue; os outros dormem.
    mrs     x1, mpidr_el1
    and     x1, x1, #0xFF
    cbnz    x1, estacionar

    // Em qual nivel de excecao (EL) a CPU entregou o controle?
    mrs     x0, CurrentEL
    lsr     x0, x0, #2
    and     x0, x0, #3
    mov     x19, x0                 // guardado para o kernel_main
    cmp     x0, #2
    b.ne    em_el1

    // ---- EL2 -> EL1 (o U-Boot entrega em EL2; o kernel roda em EL1) ----
    mov     x1, #(1 << 31)          // HCR_EL2.RW: EL1 roda em 64 bits
    msr     hcr_el2, x1
    mrs     x1, cnthctl_el2
    orr     x1, x1, #3              // EL1 pode ler os contadores do timer
    msr     cnthctl_el2, x1
    msr     cntvoff_el2, xzr        // contador virtual = contador fisico
    mov     x1, #0x3c5              // destino: EL1h com interrupcoes mascaradas
    msr     spsr_el2, x1
    adr     x1, em_el1
    msr     elr_el2, x1
    eret

em_el1:
    mov     x1, #(3 << 20)          // CPACR_EL1.FPEN: libera FP/SIMD (o Rust usa)
    msr     cpacr_el1, x1
    ldr     x1, =0x30d00800         // SCTLR_EL1: bits obrigatorios, MMU e caches desligados
    msr     sctlr_el1, x1
    ldr     x1, =vetores            // tabela de excecoes (vetores.s)
    msr     vbar_el1, x1
    isb

    // Duas pilhas: SP_EL1 so para excecoes, SP_EL0 para o kernel.
    // Assim uma excecao sempre tem pilha boa, ate num estouro de pilha.
    ldr     x1, =__pilha_excecao_topo
    msr     spsel, #1               // garante que "sp" e o SP_EL1 (o loader pode entrar em EL1t)
    mov     sp, x1                  // SP_EL1
    msr     spsel, #0               // daqui em diante "sp" e o SP_EL0
    ldr     x1, =__stack_top
    mov     sp, x1

    // Zera o BSS: variaveis globais comecam em zero.
    ldr     x1, =__bss_start
    ldr     x2, =__bss_end
1:  cmp     x1, x2
    b.hs    2f
    str     xzr, [x1], #8
    b       1b

2:  mov     x0, x19                 // kernel_main(el_boot)
    bl      kernel_main

estacionar:
    wfe
    b       estacionar

// ============================================================
//  Os outros nucleos (smp.rs): o PSCI CPU_ON comeca aqui com
//  x0 = &Partida (pilhas e os registradores da MMU do nucleo 0).
//  Desce para EL1 se precisar, liga a MMU com as tabelas do
//  kernel (antes disso nada de pilha: cache desligado) e chama
//  nucleo_secundario.
// ============================================================

.global _start_secundario
_start_secundario:
    mov     x19, x0
    mrs     x1, CurrentEL
    lsr     x1, x1, #2
    and     x1, x1, #3
    cmp     x1, #2
    b.ne    secundario_el1
    mov     x1, #(1 << 31)
    msr     hcr_el2, x1
    mrs     x1, cnthctl_el2
    orr     x1, x1, #3
    msr     cnthctl_el2, x1
    msr     cntvoff_el2, xzr
    mov     x1, #0x3c5
    msr     spsr_el2, x1
    adr     x1, secundario_el1
    msr     elr_el2, x1
    eret

secundario_el1:
    mov     x1, #(3 << 20)          // FP/SIMD liberado
    msr     cpacr_el1, x1
    ldr     x1, =0x30d00800
    msr     sctlr_el1, x1
    ldr     x1, =vetores
    msr     vbar_el1, x1
    isb
    ldr     x1, [x19, #16]          // MAIR
    msr     mair_el1, x1
    ldr     x1, [x19, #24]          // TCR
    msr     tcr_el1, x1
    ldr     x1, [x19, #32]          // TTBR0 (tabelas do kernel)
    msr     ttbr0_el1, x1
    isb
    tlbi    vmalle1
    ic      iallu
    dsb     ish
    isb
    mrs     x1, sctlr_el1
    mov     x2, #0x1005             // M, C e I: MMU e caches
    orr     x1, x1, x2
    msr     sctlr_el1, x1
    isb
    ldr     x1, [x19, #0]           // pilha de excecao (SP_EL1)
    msr     spsel, #1
    mov     sp, x1
    msr     spsel, #0
    ldr     x1, [x19, #8]           // pilha do kernel (SP_EL0)
    mov     sp, x1
    ldr     x0, [x19, #40]          // numero do nucleo
    bl      nucleo_secundario
1:  wfe
    b       1b
