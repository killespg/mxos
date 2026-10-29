# PLANO: o que fazer a seguir no MXOS

Este arquivo é para a próxima sessão do Claude Code que for continuar o
projeto (por exemplo, rodando a noite toda em outro PC). Leia inteiro antes
de começar.

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
- Navegador (`src/navegador/`): HTML → layout → desenho; **ainda ignora CSS**.
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
