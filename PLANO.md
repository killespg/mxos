# PLANO: o que fazer a seguir no MXOS

Este arquivo é para a próxima sessão do Claude Code que for continuar o
projeto (por exemplo, rodando a noite toda em outro PC). Leia inteiro antes
de começar.
Leia também o `CONTEXTO.md` (resumo da conversa, decisões e armadilhas).

## Regras (pedidos do dono do projeto)

1. **Não use agentes/subagentes nem workflows.** Faça tudo você mesmo, em sequência.
2. **Um passo por vez, sempre testado no QEMU antes do próximo.** No fim de cada
   passo o MXOS tem que compilar (`cargo build --release` e `python rodar.py --mxq`)
   e os testes antigos têm que continuar passando.
3. **Faça commit no fim de cada passo** (mensagem curta em português dizendo o que
   entrou) e `git push`, para o progresso aparecer no GitHub de manhã.
4. Estilo do código: identificadores e comentários em português **sem acento**;
   textos que o usuário vê (interface, shell, mensagens) **com acento** em
   português do Brasil. Funções pequenas, mesma densidade de comentários dos
   arquivos existentes, edition 2024 (`#[unsafe(no_mangle)]`, `unsafe extern`,
   nada de referência a `static mut`).
5. Mantenha tudo **leve**: estruturas simples, sem bibliotecas externas no kernel.
6. Se um passo travar por horas, deixe o que já funciona commitado, anote o
   problema no fim deste arquivo (seção "Diário") e siga para o próximo passo
   que não depende dele.
7. O dono escreve em português informal; responda em português.

## O que é o MXOS hoje (0.8)

Kernel ARM64 escrito do zero em Rust (`no_std` + `alloc`), testado no QEMU
`virt` (Cortex-A53, GICv2) e pensado para uma TV box MXQ PRO 4K (Amlogic S905W,
feature `mxq`, ainda não testada no hardware). Leia o `README.md` para a lista
de arquivos. Resumo:

- Boot (`src/boot.s`): cabeçalho Image do ARM64, EL2 → EL1, pilhas, BSS.
- MMU (`src/mmu.rs`): mapa identidade, páginas de 4 KB, permissões por região,
  página de guarda; heap (`src/heap.rs`) com lista de blocos livres.
- Interrupções: GICv2 (`src/gic.rs`), timer de 100 Hz (`src/timer.rs`),
  `src/irq.rs` (`esperar`, `cochilar`), vetores em `src/vetores.s`
  (só IRQ do kernel é tratada; o resto é fatal → `src/excecao.rs`).
- Drivers QEMU: virtio-mmio (`src/virtio/`), som (`src/audio/`), teclado e mouse
  (`src/entrada/`), disco virtio-blk + FAT32 com nomes longos (`src/armazenamento/`),
  rede virtio-net + TCP/IP próprio + HTTP (`src/rede/`), vídeo ramfb (`src/video/`).
- Fontes: Inter e JetBrains Mono pré-renderizadas por `ferramentas/gerar_fontes.py`
  em `src/fonte/*.mxf` (licença OFL em `ferramentas/fontes/`).
- Interface gráfica (`src/gui/`): gerenciador de janelas, área de trabalho com
  ícones, menu iniciar, widgets, temas; apps em `src/gui/apps/`: Terminal,
  Arquivos (+ Discos), Editor, Navegador, Tarefas, Drivers, Configurações,
  Calculadora, Pintura, Sobre. Configuração salva em `/config/mxos.cfg`.
- Navegador (`src/navegador/`): HTML + CSS leve → layout → desenho.
- Shell (`src/shell.rs`) na serial e na janela do Terminal (comandos: `help`).

**Limite importante:** todos os apps são compilados dentro do kernel. Ainda não
existem processos nem como rodar um programa de fora. É isso que os passos 2 a 5
resolvem.

## Como compilar e testar

Precisa de: Rust (rustup; o alvo `aarch64-unknown-none` vem do
`rust-toolchain.toml`), QEMU com `qemu-system-aarch64`, Python 3 com
`pillow` e `fonttools` (só para regenerar fontes).

```
cargo build --release
python rodar.py --mxq                                  # imagem da TV box (tem que compilar)
python rodar.py --sem-janela --teste "info;irq;desligar"
python rodar.py --teste "cmd1;cmd2" --captura saida/x.png   # com tela; salva PNG
python rodar.py --teste "" --acoes "mover 40 751;clicar;esperar 300" --captura saida/menu.png
python rodar.py --disco saida/outro.img --disco-novo ...     # outra imagem de disco
python -I ferramentas/criar_disco.py --verificar saida/disco.img   # confere o FAT32 por fora
```

`--acoes` aceita: `mover X Y`, `clicar [esquerdo|direito]`,
`arrastar X1 Y1 X2 Y2`, `tecla QCODE`, `digitar TEXTO`, `esperar MS`.
O botão do menu iniciar fica em (40, 751). Olhe as capturas (PNG) para conferir
a interface. No Windows o QEMU fica em `C:\Program Files\qemu\`; o `rodar.py`
acha sozinho.

## Os passos, em ordem

### Passo 0: conferir os acentos (0.8)

A última rodada trocou a fonte e pôs acentos em todos os textos. Confira:
compila sem avisos novos, o shell mostra e aceita acentos
(`echo ação é ç`), os apps abrem com textos acentuados e sem cortes
(capture cada um pelo menu), nomes de arquivo com acento funcionam
(`/documentos/Anotações.txt`), teclado ABNT2 gera acentos. Corrija o que
estiver errado. Commit.

### Passo 1: CSS no navegador

Subconjunto leve de CSS em `src/navegador/`:
- Fontes do CSS: `<style>`, atributo `style="..."`, `<link rel="stylesheet">`
  baixado por HTTP (ou do disco).
- Seletores: tipo (`p`), `.classe`, `#id`, descendente (`div p`), lista com
  vírgula, `a:link`/`a:visited` (tratar como `a`), `*`. Especificidade e ordem
  corretas; `!important` opcional.
- Propriedades: `color`, `background-color`, `font-weight`, `font-size`
  (mapear para INTER / INTER_TITULO / INTER_GRANDE), `font-family: monospace`
  (MONO), `text-align` (left/center/right), `text-decoration`, `line-height`,
  `margin`, `padding` (atalhos de 1 a 4 valores), `border` / `border-*`
  (largura, cor; estilo sólido), `width` / `max-width` em px ou %,
  `display: none/block/inline`, `list-style: none`.
- Cores: nomes básicos, `#rgb`, `#rrggbb`, `rgb()`.
- Herança das propriedades de texto; o resto não herda.
- Fica de fora: flexbox, grid, position, float, @media, @font-face, animações.
- Ponha uma página de teste com CSS no disco (`ferramentas/criar_disco.py`)
  e confira por captura que as cores, bordas, alinhamento e margens aparecem.

### Passo 2: processos em EL0 e chamadas de sistema

- Cada processo tem as próprias tabelas de página (TTBR0 por processo, com ASID),
  com o código e a pilha dele mapeados como acessíveis em EL0. O kernel continua
  mapeado só para EL1 (AP sem acesso de EL0, e PXN/UXN corretos).
  Cuidado: hoje o kernel usa mapa identidade em TTBR0; uma saída simples é
  manter o kernel no mapa identidade em todas as tabelas (só EL1) e pôr o
  programa numa faixa de endereços baixa e livre (por exemplo a partir de 0x400000,
  que hoje não está mapeada). Outra é mover o kernel para TTBR1 (metade alta).
  Escolha, documente e teste.
- Vetores: tratar "EL inferior, 64 bits" síncrona (SVC = classe 0x15) e IRQ.
  Salvar e restaurar o contexto completo do processo (x0-x30, SP_EL0, ELR,
  SPSR, TPIDR_EL0, registradores de FP/SIMD).
- Escalonador simples (round-robin no tique do timer) com várias tarefas;
  `sleep`, `exit`. Uma falha do processo (acesso inválido) mata só o processo,
  não o kernel.
- Primeiras chamadas próprias de teste e um programa de teste embutido.
- Integração: comando de shell para listar e matar processos; o app Tarefas
  mostra os processos.

### Passo 3: carregador de ELF

- Ler um ELF64 AArch64 (`ET_EXEC` estático primeiro, depois `ET_DYN` com PIE
  estático) do disco FAT32, mapear os segmentos `PT_LOAD` com as permissões
  certas, zerar o BSS, pilha inicial, pular para o `e_entry` em EL0.
- Comando de shell `executar /caminho [args]`.

### Passo 4: compatibilidade com Linux (ABI aarch64)

- Montar a pilha inicial do jeito do Linux: argc, argv, envp, auxv
  (`AT_PHDR`, `AT_PHENT`, `AT_PHNUM`, `AT_PAGESZ`, `AT_ENTRY`, `AT_RANDOM`,
  `AT_HWCAP`, `AT_NULL`...).
- Números das syscalls aarch64 (x8 = número, x0-x5 = argumentos, retorno em x0,
  erro = -errno). Começar por: read 63, write 64, writev 66, readv 65,
  openat 56, close 57, lseek 62, fstat 80, newfstatat 79, getdents64 61,
  getcwd 17, chdir 49, mkdirat 34, unlinkat 35, renameat 38, faccessat 48,
  readlinkat 78, ioctl 29 (TCGETS/TIOCGWINSZ do terminal), fcntl 25, dup3 24,
  pipe2 59, brk 214, mmap 222, munmap 215, mprotect 226, exit 93,
  exit_group 94, set_tid_address 96, set_robust_list 99, rt_sigaction 134,
  rt_sigprocmask 135, uname 160, getpid 172, getppid 173, getuid 174,
  geteuid 175, getgid 176, getegid 177, gettid 178, clock_gettime 113,
  nanosleep 101, sched_yield 124, getrandom 278, prlimit64 261,
  clone 220 (fork), execve 221, wait4 260, ppoll 73. Syscall desconhecida:
  devolver `-ENOSYS` e registrar o número numa lista (comando de shell para ver).
- stdin/stdout/stderr ligados ao terminal (serial + janela Terminal).
- Primeiro teste: um "Olá" estático. Sem toolchain de C: use Rust com o alvo
  `aarch64-unknown-linux-musl` (`rustup target add aarch64-unknown-linux-musl`,
  linkando com `-C linker=rust-lld` e `-C target-feature=+crt-static`), ou monte
  um ELF mínimo à mão em Python. Ponha o binário no disco pelo `criar_disco.py`.

### Passo 5: BusyBox

- Pegue um BusyBox **estático** para arm64 do Debian (pacote `busybox-static`,
  arquitetura `arm64`, em deb.debian.org). O `.deb` é um arquivo `ar` com um
  `data.tar.xz` dentro: extraia com Python (`tarfile` lê xz). Guarde em
  `ferramentas/` e ponha em `/bin/busybox` no disco.
- Objetivo: `executar /bin/busybox ls /`, depois `echo`, `cat`, `wc`, `grep`,
  `vi` (se possível) e por fim `sh` interativo dentro da janela do Terminal.
- Cada falha mostra qual syscall falta ou está errada: implemente e repita.
- Documente no README o que roda.

## Diário

(Anote aqui o que foi feito em cada passo, problemas encontrados e o que ficou
para depois.)

### Antes da noite (estado deixado no outro PC)

A rodada de acentos foi interrompida no meio. Compila limpo (QEMU e mxq), mas:

- **Terminaram:** interface (janelas, widgets, teclado ABNT2, tela de erro,
  `src/main.rs`), arquivos (Arquivos + Editor), sistema (shell em UTF-8, FAT32
  com nomes longos em UTF-8, rede, `rodar.py`, `criar_disco.py`, README).
- **Ficaram pela metade:** painel (`src/gui/apps/tarefas.rs`, `drivers.rs`,
  `configuracoes.rs`, `src/drivers.rs`, `src/psci.rs`) e navegador
  (`src/navegador/*.rs`, `src/gui/apps/navegador.rs`). Confira se os textos
  deles estão com acento e se o layout está medido com a fonte Inter (nada de
  "8 px por letra"); o navegador ainda pode estar tirando acentos das páginas.
- Pendências relatadas pelos agentes:
  - `widgets.rs`: a `Lista` ainda corta texto com o `area::cortar` de 8 px por
    letra e usa linhas de 20 px; trocar por `fonte::cortar` e conferir alturas.
  - `teclado.rs`: trema (Shift+6 no ABNT2) seguido de `u` minúsculo pode estar
    saindo maiúsculo; conferir à mão.
  - `rodar.py` `digitar`: manda `:` como Shift+ponto e vírgula (teclado US); no
    ABNT2 o `:` é Shift+ponto. É limite do script de teste, não do kernel.
  - `ferramentas/teste_fat32/tests/fat32.rs`: 4 testes esperam o disco de exemplo
    antigo (tamanhos de LEIAME.TXT/notas.txt mudaram, falta Anotações.txt na lista).
    Atualizar as listas e tamanhos.

### Passo 0 (feito)

- Conferido por captura: Tarefas, Drivers, Configurações e Navegador abrem com acentos, medidos com a Inter e
  sem cortes; a `Lista` já usa `fonte::encurtar`. Shell: `echo ação é ç` e `cat /documentos/Anotações.txt` ok.
- Trema: `ü`/`Ü` saem certos pelo teclado virtio (testado com `digitar`); não era bug do kernel.
- `rodar.py`: o `digitar` agora manda os símbolos nas teclas do ABNT2 (`/`, `:`, `[`, `'`...) e ganhou
  `--layout us`. Testado: ``echo üÜ /a:b [x]{y} 'q' "d" ção ~ ^ ` ?|\`` sai igual na janela do terminal.
- `teste_fat32`: listas e tamanhos atualizados para o disco de exemplo novo (13 testes passando). Neste PC
  não há Visual Studio: os testes rodam com `cargo +stable-x86_64-pc-windows-gnu test --release`.
- Projeto em `C:\dev\mxos` (caminho curto; o QEMU não gosta de caminho longo).

### Passo 1 (feito): CSS no navegador

- `src/navegador/css.rs`: leitor de CSS (comentários, @-regras puladas, atalhos abertos em propriedades simples),
  seletores (tipo, classe, id, `*`, descendente, filho, `a:link`), cascata com especificidade, ordem e `!important`.
- `html.rs`: pilha de elementos com o estilo calculado de cada um; fecha sozinho `<p>`, `<li>`, `<td>`...;
  `display: none` some com o texto; `display: inline` em bloco não parte a linha. Trecho ganhou `Tinta` (cor e
  fundo) e bits MONO, SEM_SUBLINHADO e TAMANHO; Bloco ganhou `alinhar`, `altura_linha` e a lista de caixas.
- `layout.rs`: caixas do CSS (margem, borda, recheio, largura, `margin: auto`) abrem e fecham entre os blocos;
  `text-align` e `line-height` por linha. As faixas agora são marcadas no fim (a caixa só sabe a altura quando fecha).
- `render.rs`: fundo da página, cor do texto, fundo de trecho, caixas com bordas por lado.
- `mod.rs`: `documento()` baixa as folhas dos `<link>` (disco ou http) antes de analisar.
- Página de teste `disco:/paginas/css.html` + `estilo.css` no disco; conferida por captura. O example.com
  (`max-width: 26em; margin: auto; padding: 25vh`, `light-dark()`) sai igual ao de um navegador de verdade.
- Problema que custou tempo: uma troca em massa de `margem_pendente = true` por `pedir_margem()` pegou a linha de
  dentro do próprio `pedir_margem` (recursão infinita, sem mensagem). Achado pelo PC da CPU no monitor do QEMU
  (`info registers`) + `llvm-objdump` (`rustup component add llvm-tools`).

### Passo 2 (feito): processos em EL0

- Escolha: kernel no mapa identidade só-EL1 em todas as tabelas; programa em endereços baixos (0x1000 a
  0x8000000, abaixo dos periféricos). A raiz do processo é cópia da do kernel com a entrada 0 trocada por uma L2
  própria (periféricos copiados + L3 do programa). ASID de 8 bits por processo, páginas nG.
- Troca de contexto "o kernel chama o programa": `entrar_usuario` salva x19-x30, sp, d8-d15, FPCR do kernel e
  faz eret; os vetores 8-11 (EL0, 64 bits) salvam o programa no Contexto e restauram o kernel, que vê o retorno
  com o motivo. Syscalls rodam em contexto normal do kernel. IRQ vinda de EL0 só tira o programa da CPU: ela é
  atendida quando o kernel religa as interrupções.
- Chamadas já com os números do Linux: read (só tty), write (1 e 2), exit/exit_group, nanosleep, clock_gettime,
  sched_yield, kill, getpid/getppid/gettid, getuid... Desconhecida → -ENOSYS e anotada (`syscalls`).
- Programa de teste embutido (assembly, `global_asm!`): ola, conta, falha, eco. Conferido: falha só mata o
  processo; dois `conta` dividem a CPU (~48% cada, captura da aba Processos); eco lê o teclado.

### Passo 3 (feito): carregador de ELF e `executar`

- `src/processo/elf.rs`: ELF64 AArch64, ET_EXEC estático e ET_DYN sem PT_INTERP (PIE estático, carregado em
  0x400000). Páginas com a soma das permissões dos segmentos que passam por elas; BSS vem zerado. Pilha
  inicial do Linux: argc, argv, envp, auxv (PHDR, PHENT, PHNUM, PAGESZ, ENTRY, RANDOM, HWCAP calculado dos
  registradores ID, PLATFORM, EXECFN...). Programa dinâmico é recusado com mensagem clara.
- Shell: `executar /caminho args... [&]` (sem "/" procura em /bin; aspas juntam argumentos).
- `ferramentas/ola/`: programa de Linux em Rust + musl, estático, ligado com o `rust-lld` (sem gcc no PC):
  `cd ferramentas/ola && cargo build --release`; o binário vai para `ferramentas/binarios/ola` e o
  `criar_disco.py` põe tudo de `ferramentas/binarios/` em `/bin`.
- Conferido: `executar /bin/ola um "dois três"` mostra argumentos, ambiente, `Instant` (clock_gettime), lê
  `/LEIAME.TXT`, grava `/documentos/do_linux.txt`, lê o teclado com acento (`ola ler`) e sai com código 3
  (`ola erro`). Nenhuma chamada faltando (`syscalls`).

### Passo 4 (feito): ABI do Linux

- `chamadas.rs` reescrito com os números do Linux aarch64: arquivos e pastas do FAT32 (`arquivos.rs`: lido
  inteiro no open, gravado no último close/fsync), /dev/null, /dev/zero, /dev/urandom, /dev/tty, tubos,
  dup/dup3/fcntl, ioctl de terminal (TCGETS/TCSETS com ICANON/ECHO/ISIG, TIOCGWINSZ, TIOCGPGRP/TIOCSPGRP),
  ppoll, brk, mmap (anônimo e de arquivo), munmap, mprotect, clone (fork; CLONE_VFORK para o pai),
  execve (com `#!`), wait4, kill com grupos, uname ("Linux 6.1.0-mxos"), prlimit64, getrandom, sysinfo,
  statfs, clock_gettime (RTC PL031 para a hora de parede)...
- Chamada que espera (teclado, tubo, filho) é repetida a cada volta do laço; dormir só acorda na hora.
- Terminal: termios de verdade (TCGETS devolve o que o programa pôs), ICRNL, modo cru, Ctrl+C para o grupo
  do terminal (quem tem tratador recebe o ^C como tecla, para o shell do BusyBox).

### Passo 5 (feito): BusyBox

- `busybox-static_1.37.0-6+b9_arm64.deb` do Debian; `ferramentas/extrair_deb.py` tira `./usr/bin/busybox`
  (Python puro: ar + tarfile com xz). Vai para `ferramentas/binarios/busybox` e daí para `/bin`.
- Rodou de primeira (só pediu `rseq`, que a glibc dispensa). Depois entraram: applets sem link (`/bin/ls`
  inexistente vira o BusyBox com argv[0] = ls), `/proc` mínimo (`proc.rs`) para df/free/ps/uptime, `sendfile`,
  `readlinkat` do `/proc/self/exe`, `/etc/passwd` e cia no disco, `LANG=C.UTF-8` (acentos no `ls`).
- Terminal: o console (`video/tela.rs`) ganhou um parser ANSI (CUP, CUU/CUD/CUF/CUB, ED, EL, SGR com 16 cores,
  ?25, tela alternativa); o shell do MXOS responde ao `ESC[6n` com a posição de verdade do cursor. A janela do
  terminal manda setas, Home, End, Delete, PgUp/PgDn e Ctrl+letra para o processo.
- Ctrl+C manda SIGINT ao grupo do terminal: o `sleep` morre (130); o `sh`, que tem tratador, continua.
- Conferido por captura: `sh -l` na janela (histórico com as setas), `vi` editando e gravando com `:wq`.
- `teste_fat32`: listas novas (bin, etc) e o espaço livre agora é comparado com o que o verificador em Python
  conta (não é mais um número fixo que quebra quando um binário muda).
- Versão 0.9.

### Passo 6 (feito, 0.10): sinais, threads, rede e programas dinâmicos

- Sinais (`processo/sinais.rs`): rt_sigframe do Linux arm64 na pilha (siginfo, ucontext com x0-x30, sp, pc,
  pstate e o FPSIMD), trampolim `mov x8,#139; svc #0` na página acima da pilha (quando a libc não manda
  `SA_RESTORER`), `rt_sigreturn`, `rt_sigprocmask`, `rt_sigsuspend`, `rt_sigpending`. A chamada que espera guarda o
  número (`na_chamada`): um sinal a interrompe com `EINTR` ou a repete com `SA_RESTART`, menos `poll`, `nanosleep`
  e `sigsuspend`, que o Linux nunca repete (o `pause` da musl travava por isso). `setitimer`/`alarm` com `SIGALRM`,
  `SIGCHLD` para o pai; filho de quem ignora `SIGCHLD` some sozinho.
- Threads: `Dividido<T>` (Rc + UnsafeCell) para a memória (o `brk` foi para o `Espaco`) e a tabela de
  descritores; `tgid`, `exit` só da thread, `exit_group`/morte por sinal/`execve` levam o grupo. `futex` de verdade:
  WAIT dorme até um WAKE na mesma memória (ou o prazo), REQUEUE/CMP_REQUEUE.
- Rede: a pilha ganhou várias conexões TCP (as dos sockets trocam de lugar com a `Pilha::tcp` em cada chamada),
  abertura passiva (servidor), loopback (pacote para o nosso IP volta pela fila do `processar`) e cópia do ICMP
  para os sockets de ping. `processo/soquetes.rs` faz as chamadas do Linux; `/etc/resolv.conf` aponta para o DNS
  do QEMU. `rodar.py --porta PC:MXOS` encaminha uma porta do PC.
- Dinâmicos: `PT_INTERP` -> o `ld-linux` (Debian libc6 2.41) entra em 0x7000000 com `AT_BASE`; as `.so` vêm de
  `/lib` por `openat` + `mmap` (que já existiam). Rodou de primeira.
- Conferido no QEMU: `ola sinais`, `ola threads`, `ola rede` (std::net da musl), `trap`/`wait`/Ctrl+C no `sh`,
  `nslookup`, `wget`, `ping`, `nc`, `httpd` (pelo próprio MXOS e pelo PC), BusyBox dinâmico; `teste_fat32` 13/13;
  imagem da TV box compila.
- Versão 0.10.

### Passo 7 (feito, 0.10): interface leve e mais bonita

- Medido no QEMU: o redesenho completo em 1080p custa uns 30 ms; o resto é por área suja. O tempo das escritas
  no framebuffer no QEMU não serve de régua (cada linha da tela é uma página que o QEMU marca para redesenhar),
  então a comparação foi em pixels: arrastar só o contorno troca ~350 mil pixels por ~9 mil por movimento.
- Sem buffer de trás (-3 MB em 1024x768, -8 MB em 1080p); áreas sujas apresentadas separadas; RGB565 por fileira
  com uma limpeza de cache por área (antes uma linha de cache inteira por pixel).
- Barra de tarefas com hora e data do RTC (fuso nas configurações), janelas com cantos arredondados, clique direito
  na área de trabalho para o plano de fundo, seleção em área na área de trabalho e no Arquivos (vários itens em
  copiar/recortar/colar/apagar).

### Passo 8 (feito, 0.10): copy-on-write no fork

- `memoria.rs`: quadros em `Rc<Box<Quadro>>` (o Rc pequeno, o quadro alinhado em 4 KB). No fork o filho divide os
  quadros; páginas de escrita ficam só de leitura nos dois até a primeira escrita, que cai em `resolver_cow`
  (falha de permissão com WnR em EL0) e ganha cópia própria. O kernel, ao escrever na memória do programa,
  também separa a página antes. O código nunca é copiado.
- Medido: 6 subshells `(sleep 4 && true) &` vivos: 997 MB livres sem, 1012 MB com (uns 2,5 MB por fork).
  Subshell que muda variável não afeta o pai; `ola sinais` (fork + waitpid) e `ola threads` continuam certos.

### Pendências para depois

- `AF_UNIX`, IPv6, links simbólicos (o FAT32 não guarda), job control (Ctrl+Z/SIGSTOP), tratador de `SIGSEGV`.
- Memória dos processos: cada programa dinâmico tem a própria cópia da libc (falta dividir as páginas só de
  leitura entre programas diferentes) e o `open` lê o arquivo inteiro.
- Na box: desenhar com os 4 núcleos; montar a cena já em RGB565.
- Na TV box os processos ainda não são compilados (o módulo `processo` depende do disco do QEMU).
- O `--teste` do `rodar.py` separa comandos por `;`: para mandar `;` para o sh use `&&` ou um script.
