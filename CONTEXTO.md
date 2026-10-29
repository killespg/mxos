# CONTEXTO: resumo da sessão que criou o MXOS

Leia junto com o `PLANO.md` (o que fazer) e o `README.md` (o que existe).
Este arquivo resume a conversa que levou até aqui, para a próxima sessão
não começar do zero.

## Quem é o dono e como ele trabalha

- Brasileiro, escreve em português informal e curto ("mano", "mete com acento").
  Responda em português, direto, com tabelas quando ajudar.
- Quer resultado rápido; reclama quando demora. Prefere ver capturas de tela e
  números (tempos, frequências, % de CPU) a explicações longas.
- **Não quer mais agentes/subagentes/workflows**: faça tudo você mesmo, em
  sequência, testando cada passo.
- Gosta de entender o porquê (pergunta coisas como "o terminal serve pra quê?",
  "isso seria um emulador?"). Explique com honestidade, inclusive limites.
- Repositório público: https://github.com/killespg/mxos (pode fazer push).

## Como o projeto começou

1. Ele queria instalar Windows numa TV box **MXQ PRO 4K** (Amlogic S905W,
   4x Cortex-A53, 1 GB de RAM, Android 7.1). Expliquei que não dá (ARM,
   pouca RAM); alternativas: Armbian, CoreELEC, Limbo + Windows XP, Box64/Wine.
2. Ele decidiu fazer um sistema próprio do zero "com IA". Um outro modelo
   (DeepSeek) tinha feito um "MXOS" em assembly **x86** (pasta `../so/`, fora
   deste repositório): não roda na box. Achei 2 bugs nele (o boot lia o setor 2
   em vez do 1; o `echo` pulava 5 caracteres a mais).
3. Começamos a versão ARM64 em Rust, evoluindo uma versão por vez:
   0.2 boot/serial/PSCI → 0.3 MMU e heap → 0.4 GIC, timer, interrupções →
   0.5 vídeo (ramfb) e som (virtio-sound) → 0.6 interface gráfica, disco
   FAT32, rede TCP/IP, motor HTML → 0.7 apps (Arquivos, Editor, Navegador,
   Tarefas, Drivers, Configurações) → 0.8 fontes Inter/JetBrains Mono com acentos.

## Decisões já tomadas (não reabrir sem motivo)

- **Linguagem e estilo:** Rust `no_std` + `alloc`, edition 2024, identificadores
  e comentários em português **sem acento**, textos visíveis **com acento**.
- **Testes no QEMU `virt`** (Cortex-A53, `gic-version=2` igual ao GIC-400 da box)
  antes de qualquer coisa; a TV box (`--features mxq`) só compila, nunca foi
  testada no hardware. A box precisa de adaptador USB-TTL 3,3 V na serial e
  boot pelo U-Boot (`fatload` + `booti 0x1080000`).
- **Fontes pré-renderizadas** no PC (`ferramentas/gerar_fontes.py`), não
  rasterizador no kernel: leve (193 KB) e com acentos (Latin-1).
- **Navegador:** só HTTP (HTTPS/TLS é grande demais), sem JavaScript. CSS é o
  próximo passo (subconjunto leve, ver PLANO.md). O dono reclamou que "não
  aparece CSS".
- **Rodar programas de fora:** o dono quer que o sistema seja útil ("um sistema
  que não roda nada não é útil"). Combinamos: **compatibilidade com programas
  Linux ARM64** (nativos, sem emulador) → BusyBox. `.exe` de Windows x86
  precisaria de tradutor x86→ARM + imitar a API do Windows (Wine): fora de
  alcance. ReactOS não serve (não roda em ARM). Explicado que apps Android já
  rodam no Android original da box.
- **Gerenciador de tarefas** mostra os apps (janelas) com CPU e memória; quando
  existirem processos (passo 2), deve mostrar os processos.

## Armadilhas que já custaram tempo

- **Cargo soma rustflags:** a variável `CARGO_TARGET_..._RUSTFLAGS` se soma às do
  `.cargo/config.toml` (dois linker scripts → ELF quebrado). Use
  `CARGO_ENCODED_RUSTFLAGS`, que substitui (o `rodar.py --mxq` faz isso). Uma
  cópia do projeto dentro de outra pasta com `.cargo/config.toml` herda as flags.
- **`+strict-align` é obrigatório:** com a MMU desligada (boot) a RAM é memória de
  dispositivo e acesso desalinhado gera exceção. O aviso "unknown and unstable
  feature strict-align" é esperado (é o único aviso aceito no build).
- **Atômicos só depois da MMU:** as travas (`compare_exchange`) precisam de
  memória normal com cache; por isso `mmu::ligar()` vem antes do `heap::iniciar()`.
- **A IRQ salva q0-q31 inteiros** (a função chamada só preserva d8-d15); a pilha
  de exceção (SP_EL1) é separada da do kernel (SP_EL0).
- **No GIC-400 real** o firmware seguro decide o grupo das interrupções; o kernel
  limpa o GIC no boot e confere `ISTATUS` do timer (tique fantasma).
- **Windows:** caminhos muito longos quebram o QEMU e o Python (use pastas curtas
  em Temp). O `-serial stdio` do QEMU com pipe no Windows perde Enter; os testes
  usam serial por TCP (o `rodar.py` já faz isso).
- **QEMU trava a imagem de disco:** dois QEMU no mesmo `saida/disco.img` não
  sobem. Use `--disco saida/outro.img` em testes paralelos.
- **O WAV do QEMU** sai com tamanhos zerados no cabeçalho; `rodar.py` corrige.
- **Contador de CPU ociosa** só conta o sono feito por `irq::esperar`/`cochilar`;
  um `wfi` direto não entra na conta.
- **Framebuffer e DMA vêm do heap** e nunca podem ser devolvidos enquanto o
  dispositivo usa (resete o dispositivo antes de liberar).
- **Tela de erro gráfica** roda dentro da exceção: não pode alocar nem pegar trava.

## Números de referência (QEMU no PC do dono)

- CPU ociosa com a área de trabalho parada: ~97-98 %.
- Primeiro quadro da área de trabalho: ~23 ms; uma tecla no terminal: ~7 ms.
- `esperar 1000` mede ~1001-1005 ms com 101 tiques (timer de 100 Hz).
- Ping no gateway do QEMU: 7-12 ms. Imagem da TV box: ~127 KB.

## Estado no momento da passagem

Versão 0.9: os passos 0 a 5 do `PLANO.md` estão feitos (acentos conferidos, CSS no
navegador, processos em EL0, carregador de ELF, ABI do Linux e BusyBox com `sh` e
`vi` na janela do terminal). Ver o "Diário" e as "Pendências para depois" no
`PLANO.md`.

## Neste PC (Windows, segunda sessão)

- Projeto em `C:\dev\mxos` (caminho curto). Rust, QEMU e Python 3.12 instalados pelo
  winget. Sem Visual Studio: os testes do FAT32 rodam com a toolchain GNU
  (`cargo +stable-x86_64-pc-windows-gnu test --release` em `ferramentas/teste_fat32`).
- Programa travado sem mensagem: o PC da CPU sai no monitor do QEMU (`info registers`)
  e o `llvm-objdump` (`rustup component add llvm-tools`) mostra a função.
