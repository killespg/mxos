# MXOS (ARM64)

Kernel escrito do zero em Rust para a TV box **MXQ PRO 4K** (Amlogic S905W, 4x Cortex-A53).
Ele é testado primeiro no QEMU (`-M virt`) e depois gera uma imagem para o U-Boot da box.

## Como rodar

```bash
python rodar.py                              # janela com a área de trabalho + som; o shell também fica no terminal (sair: Ctrl+A, depois X)
python rodar.py --sem-janela                 # só a serial, sem tela e sem som
python rodar.py --el2                        # começa em EL2, igual ao U-Boot
python rodar.py --disco-novo                 # recria saida/disco.img (FAT32 de 64 MB) antes de rodar
python rodar.py --teste "help;info;desligar" # roda comandos sozinho e mostra a saída
python rodar.py --teste "esperar 1000;irq"   # testa o timer e as interrupções
python rodar.py --teste "video;som" --captura saida/tela.png  # salva a tela; o som vai para saida/som.wav
python rodar.py --teste "ip;http http://example.com" --espera 10  # espera até 10 s pelo prompt a cada comando (padrão 3)
python rodar.py --teste "" --acoes "mover 30 752;clicar;esperar 300" --captura saida/menu.png  # mouse e teclado pelo QMP
python rodar.py --disco saida/outro.img --disco-novo --teste "ls /documentos"  # outra imagem (cada QEMU precisa da sua)
python rodar.py --mxq                        # gera saida/mxos-mxq.img para a TV box
python rodar.py --teste "executar sh -c 'ls / | wc -l'"   # programa de Linux (BusyBox) no MXOS
python rodar.py --porta 8080:80              # http://localhost:8080 no PC entra na porta 80 do MXOS (httpd)
```

Precisa do Rust (`rustup`) e do QEMU. O alvo `aarch64-unknown-none` é instalado sozinho pelo `rust-toolchain.toml`.

O `rodar.py` sempre liga no QEMU: `virtio-keyboard-device`, `virtio-tablet-device` (mouse com posição absoluta),
o disco `saida/disco.img` num `virtio-blk-device` e a rede `user` num `virtio-net-device`.
Se `saida/disco.img` não existir, ele é criado por `python -I ferramentas/criar_disco.py saida/disco.img`
(do zero, sem `mkfs`). `python -I ferramentas/criar_disco.py --verificar saida/disco.img` confere e lista o que está
no disco, inclusive o que o MXOS gravou.

### Testes automáticos com mouse e teclado

No `--teste` o QEMU também abre o QMP (o protocolo JSON de controle dele). `--acoes` roda depois dos comandos da
serial e antes da `--captura`, com as ações separadas por `;`:

| Ação | O que faz |
|---|---|
| `mover X Y` | põe o ponteiro no pixel (X, Y) da tela (o tablet vai de 0 a 32767; o `rodar.py` converte) |
| `clicar [esquerdo\|direito\|meio]` | aperta e solta o botão (padrão: esquerdo) |
| `arrastar X1 Y1 X2 Y2` | aperta em (X1, Y1), anda em 10 passos e solta em (X2, Y2) |
| `tecla QCODE` | aperta e solta uma tecla pelo nome do QEMU: `ret`, `esc`, `tab`, `backspace`, `up`, `f1`, `a`...; com modificador: `ctrl+c`, `shift+tab` |
| `digitar TEXTO` | digita o texto no layout do MXOS (ABNT2 por padrão; `--layout us` para o americano), com maiúsculas, símbolos e acentos; não aceita `;` (veja abaixo) |
| `esperar MS` | espera MS milissegundos |

Tudo vira `input-send-event` do QMP. O que o shell da janela escrever na serial enquanto as ações rodam também
aparece na saída. As ações são conferidas antes de compilar, então um erro de digitação para tudo logo no começo.

O MXOS liga com o teclado ABNT2 (o padrão das configurações) e o `digitar` manda as teclas do ABNT2: letras,
números e espaço como no americano, e os símbolos nas teclas do ABNT2 (`/` e `?` na tecla ao lado do Shift
direito, `:` em Shift + ponto e vírgula, `'` e `"` à esquerda do 1, `[`/`]` nas teclas do acento e do til...).
`~`, `^` e `` ` `` saem como tecla morta seguida de espaço. Se o MXOS estiver com `layout=us`, passe `--layout us`
ao `rodar.py`.

Acentos no `digitar`: o QMP só manda teclas, então o `rodar.py` faz o que uma pessoa faria no ABNT2: `á` vira a
tecla morta do agudo (`bracket_left`) e depois `a`; `à` é a mesma tecla com shift (crase); `ã`/`õ` usam o til
(`apostrophe`), `â`/`ê`/`ô` o circunflexo (shift + `apostrophe`), `ü` o trema (shift + `6`) e `ç` é a tecla
própria dele (`semicolon`). Maiúsculas acentuadas também funcionam (`É`, `Ç`). Com `--layout us` os acentos não
saem (o teclado americano não tem teclas mortas). Pela serial (`--teste`) tudo vai em UTF-8, então
`--teste "mkdir /Área;ls /"` funciona em qualquer layout.

## Arquivos

| Arquivo | O que faz |
|---|---|
| `src/boot.s` | Cabeçalho `Image` do ARM64, escolhe o núcleo 0, desce de EL2 para EL1, liga o FP, prepara as pilhas e o BSS |
| `src/vetores.s` | Tabela de exceções (16 entradas); a IRQ salva os registradores e chama `tratar_irq`; `entrar_usuario`/`sair_do_usuario` levam o processo para EL0 e trazem de volta |
| `src/excecao.rs` | Explica a exceção na serial (inclusive falhas da MMU e estouro de pilha) e, com a área de trabalho ligada, numa tela de erro desenhada direto no framebuffer (o `panic` usa a mesma tela), sem alocar e sem travas |
| `src/mmu.rs` | Tabelas de páginas (4 KB, 3 níveis), permissões por região, liga MMU e caches |
| `src/gic.rs` | GICv2 (GIC-400 na box): liga cada interrupção, confirma e finaliza |
| `src/irq.rs` | Tratador de IRQ, contadores e `esperar` (dorme no WFI sem perder interrupção) |
| `src/timer.rs` | Timer virtual do ARM: tique de 100 Hz e `esperar_ms` |
| `src/heap.rs` | Alocador com lista de blocos livres: faz `Vec`, `String` e `Box` funcionarem |
| `src/plataforma.rs` | Mapa de memória física do QEMU e da TV box, endereços do GIC e números das interrupções |
| `src/mapa.rs` | Endereços das regiões do kernel (código, constantes, pilhas) |
| `src/terminal.rs` | O que o shell escreve vai para a serial e para um console de texto ao mesmo tempo (o da tela cheia no boot, o da janela do terminal na área de trabalho) |
| `src/video/` | `fwcfg` + `ramfb` (framebuffer do QEMU), `tela` (pixels, texto em JetBrains Mono, console 8x16 com rolagem e UTF-8), `mxq` (framebuffer do U-Boot na box, não testado) |
| `src/fonte/` | Motor de texto: fontes pré-renderizadas com antialiasing (`*.mxf`), medida de largura, corte com reticências e desenho em qualquer superfície |
| `src/virtio/` | Transporte `virtio_mmio` (acha o dispositivo nos slots do QEMU e o inicializa) e `virtqueue`, usados por todos os drivers virtio |
| `src/audio/` | Driver `virtio_som` e gerador de tons só com inteiros |
| `src/som.rs` | Comando `som`: toca Dó, Mi, Sol pelo virtio-sound |
| `src/uart.rs` | Serial: PL011 (QEMU) ou UART_AO_A da Amlogic (`--features mxq`); a interrupção de recepção enche um buffer circular |
| `src/psci.rs` | Desligar e reiniciar pelo firmware (HVC ou SMC); a via fica guardada para a área de trabalho também usar |
| `src/config.rs` | Configurações em `/config/mxos.cfg` (`chave=valor`): tema, papel de parede, relógio, ícones, teclado e página inicial; ler nunca falha |
| `src/drivers.rs` | Sonda (só lendo registradores) CPU, GIC, timer, serial, RTC, fw_cfg e os 32 slots virtio-mmio: dispositivo, driver, endereço, IRQ e estado. Comandos `drivers` e `tarefas` |
| `src/cpu.rs` | Registradores: EL, MIDR, contador; liga/desliga interrupções e `wfi` |
| `src/shell.rs` | Shell e comandos. Recebe um byte de cada vez (`Shell::tecla`), monta a linha em UTF-8 e serve para a serial e para a janela do terminal |
| `src/gui/` | Área de trabalho: `grafico` (retângulos e superfícies na RAM), `cursor`, `tema` (cores e aparência), `janelas` (gerenciador, pedidos dos apps, atalhos), `area` (papel de parede, atalhos, barra de tarefas, menu), `widgets` (botões, campo de texto, lista, abas, ícones, gráficos) e `apps/`. `gui::rodar` é o laço principal |
| `src/gui/apps/` | `arquivos` (arquivos e discos), `editor`, `navegador`, `tarefas`, `drivers`, `configuracoes`, `calculadora`, `pintura`, `sobre` e `terminal` |
| `src/entrada/` | Teclado e mouse virtio-input (layouts US e ABNT2, com teclas mortas e ç) |
| `src/armazenamento/` | Driver virtio-blk e FAT32 com nomes longos em UTF-16 (ler, gravar, pastas, renomear) e os comandos `ls`, `cat`, `escrever`... |
| `src/rede/` | Driver virtio-net e a rede feita do zero: Ethernet, ARP, IPv4, ICMP, UDP, DHCP, DNS, TCP e cliente HTTP |
| `src/processo/` | Processos em EL0: tabela e escalonador (`mod.rs`), espaço de endereços com tabelas de páginas próprias (`memoria.rs`) e chamadas de sistema no padrão do Linux aarch64 (`chamadas.rs`) |
| `src/navegador/` | Motor do navegador: HTML tolerante, CSS leve (`css.rs`), diagramação com quebra de linha e caixas, desenho e modo texto (`navegar`); o app da janela fica em `src/gui/apps/navegador.rs` |
| `ferramentas/criar_disco.py` | Cria a imagem FAT32 do disco do zero e confere imagens (`--verificar`) |
| `ferramentas/gerar_fontes.py` | Gera `src/fonte/*.mxf` a partir das fontes TrueType em `ferramentas/fontes/` (precisa do Pillow e do fontTools só no PC) |
| `ferramentas/fontes/` | Inter e JetBrains Mono (TTF) com as licenças (SIL Open Font License 1.1) |
| `ferramentas/teste_fat32/` | Testes do `fat32.rs` no PC contra imagens do `criar_disco.py`: `cd ferramentas/teste_fat32 && cargo test --release` (se o Python não for achado, defina `PYTHON`; no Windows sem o Visual Studio use a toolchain GNU: `cargo +stable-x86_64-pc-windows-gnu test --release`) |
| `linker.ld` / `linker-mxq.ld` / `secoes.ld` | Onde o kernel fica (QEMU em 0x40080000, box em 0x01080000), páginas alinhadas e página de guarda |

## Memória

| Região | Permissão |
|---|---|
| Código do kernel | lê e executa |
| Constantes | só lê |
| Dados, BSS, pilhas, heap | lê e escreve, nunca executa |
| Periféricos | memória de dispositivo (sem cache), nunca executa |
| Página de guarda abaixo da pilha, endereço 0 | sem mapeamento |

Os comandos `mem`, `mmu`, `traduzir` e `heap` mostram isso funcionando. `erro pilha`, `erro codigo` e `erro nulo` provam que a proteção pega o erro.

## Interrupções

O kernel não fica mais em loop perguntando à serial se chegou tecla: ele dorme com `wfi` e acorda por interrupção.

- O GIC (versão 2, igual ao GIC-400 da box; o `rodar.py` passa `gic-version=2` ao QEMU) entrega duas interrupções à CPU 0:
  o timer virtual (INTID 27) e a serial (INTID 33 no QEMU, 225 na box).
- O timer dispara 100 vezes por segundo. Cada tique reagenda o próximo sem acumular atraso.
- A interrupção da serial copia os bytes recebidos para um buffer circular de 256 bytes, sem alocar e sem travas.
- `irq::esperar` desliga as interrupções, confere se já tem o que esperar e só então executa o `wfi`.
  Assim uma interrupção que chega no meio do caminho nunca é perdida: ela acorda o `wfi` e é atendida logo depois.
- Se a interrupção da serial não funcionar no hardware, o tique de 100 Hz ainda acorda a CPU e a serial é lida direto.

O comando `irq` mostra quantas vezes cada interrupção chegou e quanto tempo a CPU passou dormindo. `esperar 1000` dorme um segundo e compara o tempo pedido com o medido.

## Vídeo e som

No QEMU o kernel liga o `ramfb` no boot: um framebuffer de 1024x768 tirado do heap, com uma faixa de título e um console de texto 8x16 (JetBrains Mono, com acentos) embaixo. Tudo que o shell escreve aparece na serial e na tela. Sem a área de trabalho o teclado continua sendo a serial; com ela, a tela do boot dá lugar às janelas (veja abaixo).

O comando `som` toca uma melodia pelo `virtio-sound`. No `--teste` o QEMU grava o som em `saida/som.wav`; no modo com janela ele sai na caixa de som do Windows. Enquanto toca, a CPU dorme no `wfi` entre um período de áudio e outro (`irq::cochilar`).

Na TV box ainda não tem vídeo nem som: `src/video/mxq.rs` sabe ler o framebuffer que o U-Boot deixa no HDMI, mas falta mapear essa região na MMU e testar no hardware. Som pelo HDMI precisa de outro driver (AIU/I2S da Amlogic).

## Área de trabalho, disco, rede e navegador (0.6, só no QEMU)

O boot segue esta ordem: MMU, heap, interrupções, vídeo, disco, rede e, se houver vídeo e teclado/mouse virtio,
a área de trabalho. Cada parte escreve uma linha de estado na serial antes do banner:

```
video   : 1024x768, ramfb do QEMU, XRGB8888
disco   : vda com 64 MB (62 MB livres), FAT32 "MXOS"
rede    : ip 10.0.2.15, gateway 10.0.2.2, dns 10.0.2.3 (DHCP)
entrada : 2 dispositivo(s): ...
gui     : area de trabalho 1024x768; a serial continua no terminal
config  : tema Azul, papel gradiente, teclado ABNT2 (brasileiro); padrao, sem /config/mxos.cfg
```

- **Área de trabalho**: papel de parede em degradê, barra de tarefas com o botão `MXOS` (menu iniciar), um botão por
  janela e o relógio. As janelas ganham foco com o clique, são arrastadas pela barra de título e fecham no `[x]`.
  Tudo é desenhado num buffer na RAM e só o retângulo que mudou vai para o framebuffer; mexer o mouse só redesenha
  o cursor. Entre um quadro e outro a CPU dorme no `wfi` (o tique de 100 Hz acorda).
- **Terminal**: o shell mora numa janela de 80x25. Tudo que ele escreve vai para a serial e para a janela, e os
  comandos chegam pelo teclado virtio ou pela serial (o `--teste` continua igual). Se a janela fechar, o shell fica
  guardado: o menu `Terminal` ou o próximo byte da serial abre a janela de novo, com a linha que estava sendo digitada.
  Enquanto um comando roda (um `esperar` ou um `http`, por exemplo), a área de trabalho espera ele terminar.
- **Disco**: `ls`, `cat`, `escrever`, `mkdir`, `rm`, `mv`, `df` e `discos` num FAT32 de verdade (com nomes longos),
  que o `criar_disco.py --verificar` lê de volta no Windows.
- **Rede**: `ip`, `ping`, `dns` e `http` (só `http://`, sem TLS) pela rede `user` do QEMU.
- **Navegador**: `navegar disco:/paginas/index.html` ou `navegar http://...` mostra a página em modo texto com os
  links numerados. O mesmo motor desenha a página em pixels para o aplicativo do navegador da próxima versão.

Sem teclado/mouse virtio (ou sem vídeo) o MXOS volta para o shell só na serial, como antes.
Na TV box (`--features mxq`) nada disso é compilado: continua o shell na serial.

## Aplicativos, personalização e drivers (0.7, só no QEMU)

A área de trabalho ganhou os aplicativos de um sistema de verdade, todos escritos do zero e leves: cada um só
redesenha quando algo muda, e o que trava (ler o disco, buscar uma página) mostra antes um quadro com o aviso.

- **Área de trabalho**: ícones na coluna da esquerda (Arquivos, Navegador, Terminal, Tarefas, Configurações).
  Um clique escolhe o ícone, clique duplo (ou Enter) abre o app. Clicar no papel de parede tira o foco das janelas.
  O menu iniciar mostra um ícone por app. Passar o mouse num ícone só redesenha aquele ícone; no resto do papel de
  parede só o cursor é redesenhado, como antes.
- **Arquivos** e **Discos**: gerenciador de arquivos com Voltar, Cima, Raiz, Atualizar, campo do caminho (Enter
  navega), lista com ícone, nome, tamanho e tipo, Nova pasta, Renomear, Apagar (com confirmação), Recortar, Copiar e
  Colar (pastas inteiras também) e a barra de estado com as contagens e o espaço livre. Clique duplo abre a pasta, ou
  o arquivo no app certo: `.htm`/`.html` no navegador, o resto no editor. A vista **Discos** mostra cada disco com
  rótulo, sistema de arquivos, tamanho e a barra de uso. Atalhos: Backspace, Delete, F2, F5, Ctrl+C/X/V.
- **Editor de texto**: várias linhas, cursor, setas, Home/End, PgUp/PgDn, Delete, Backspace, Enter, Tab, clique e
  roda; Novo, Abrir, Salvar e Salvar como (com o caminho num campo), Ctrl+S. O título mostra `Editor - nome*` com
  alterações não salvas e a barra de estado mostra linha, coluna e os erros de gravação.
- **Navegador**: Voltar, Avançar, Recarregar, Início, barra de endereço e Ir; rolagem com a roda, a barra e o
  teclado; passar o mouse num link mostra o endereço embaixo; histórico; o título é o da página. Páginas de exemplo
  no disco: `index.html`, `sobre.html`, `guia.html`, `tabelas.html` e `teste.html` em `/paginas`. Só `http://`:
  um `https://` mostra uma página explicando. A página inicial vem das configurações.
- **Tarefas**: aba Aplicativos (cada janela com a CPU do último segundo e a memória; Finalizar tarefa fecha a
  escolhida) e aba Desempenho (CPU dos últimos 60 s, memória do heap, interrupções por segundo, tempo ligado).
- **Drivers**: cada dispositivo achado (CPU, GIC, timer, serial, vídeo, som, teclado e mouse, disco, rede e cada slot
  virtio), com o driver do MXOS, o endereço, a IRQ e o estado, agrupados por categoria, e os detalhes do escolhido.
- **Configurações** (e **Personalizar**, que abre na primeira aba): tema (Azul, Escuro, Claro, Verde, Roxo), papel
  de parede (degradê, sólido, listras, xadrez ou pontos, em qualquer cor da paleta), relógio com ou sem segundos,
  ícones na área de trabalho, layout do teclado (US ou ABNT2, com um campo para testar), rede (com um ping no
  gateway), discos e sistema (versão, CPU, memória, reiniciar e desligar com confirmação). Tudo vale na hora; o
  **Salvar** grava em `/config/mxos.cfg` e o MXOS liga do mesmo jeito da próxima vez.
- **Tela de erro**: uma exceção da CPU ou um `panic` com a área de trabalho ligada pinta a tela inteira com o motivo
  (os mesmos detalhes da serial). Teste com `erro nulo` ou `erro pilha`.
- **Shell**: `drivers` lista os dispositivos e `tarefas` as janelas abertas, com o tempo de CPU e a memória.

O arquivo de configuração é texto simples; o que faltar (ou não fizer sentido) fica com o padrão:

```
# MXOS: configuracoes (chave=valor)
tema=Azul
papel=gradiente
cor_papel=204060
relogio_segundos=sim
icones=sim
arrastar_contorno=sim
fuso=-3
layout=abnt2
pagina_inicial=disco:/paginas/index.html
```

Os apps conversam com o gerenciador de janelas por pedidos (`janelas::Pedido`: abrir, fechar, abrir um arquivo,
trocar o layout, redesenhar tudo depois de trocar o tema) e desenham com o mesmo kit de `gui/widgets.rs`, nas
cores do tema escolhido.

## Interface leve e mais bonita (0.10)

Pensado para a TV box (1 GB, Cortex-A53, tela de 1920x1080 em RGB565 que o vídeo lê direto da RAM):

- **Só o que mudou vai para a tela**, cada pedaço separado: o relógio e o mouse longe um do outro não viram um
  retângulo enorme. A cena (a tela sem o cursor) vai direto para o framebuffer; o cursor, o contorno do arrasto e o
  retângulo de seleção são desenhados por cima. Não há mais buffer de trás: um buffer de tela inteira a menos
  (3 MB em 1024x768, 8 MB em 1080p). A área de trabalho parada usa uns 9 MB do heap em 1024x768.
- **Conversão para RGB565 por fileira** e o cache limpo uma vez por área. Antes, cada pixel limpava uma linha
  inteira de cache (60 linhas de cache por pixel na box).
- **Arrastar só o contorno** (padrão, desligável em Personalizar): uma linha dupla, escura por fora e clara por
  dentro, anda com o mouse, e a janela vai para o lugar ao soltar. Uns 9 mil pixels por movimento contra uns
  350 mil redesenhando a janela e o que estava embaixo.
- **Seleção em área**: arrastar no papel de parede desenha um retângulo translúcido e escolhe os ícones que ele
  toca (Enter abre todos). No **Arquivos**, arrastar no espaço vazio da lista (fora do nome) faz o mesmo com as
  linhas, rolando a lista na borda; Copiar, Recortar, Colar e Apagar valem para todos os escolhidos, e a barra de
  estado mostra quantos são e o tamanho. Só as faixas do retângulo que mudaram são redesenhadas.
- **Barra de tarefas** de 40 px: hora e data ("qui, 29/10/2026") à direita, pelo RTC e no fuso das
  configurações (`fuso=-3`); botões planos e arredondados, com um tracinho embaixo da janela aberta (comprido na
  ativa). A cada segundo só a caixa do relógio é redesenhada.
- **Janelas**: barra de título de 30 px com brilho no alto, botões [_] e [x] arredondados, janela fora de foco
  apagada, borda de 1 px e **cantos arredondados** (raio 8, 4x4 amostras por pixel, a borda acompanha a curva).
  Nenhum buffer extra: só os pixels dos quatro cantos são guardados e repostos.
- **Clique direito na área de trabalho**: "Alterar plano de fundo…" (abre o Personalizar) e "Próximo plano de
  fundo" (troca na hora; o Salvar das Configurações guarda).
- `rodar.py --acoes` ganhou `apertar` e `soltar` (captura no meio de um arrasto).

## Fontes e acentos (0.8)

O MXOS deixou a fonte 8x8 de lado e passou a escrever com fontes de verdade, com acentos em todo lugar.

- **Fontes**: a interface usa a [Inter](https://github.com/rsms/inter) (13 px normal e seminegrito, 20 px para
  títulos e 32 px em negrito) e o terminal usa a [JetBrains Mono](https://github.com/JetBrains/JetBrainsMono)
  numa grade de 8x16. O kernel não lê TrueType: `ferramentas/gerar_fontes.py` desenha cada caractere no PC, com
  antialiasing, e grava só os pixels (`src/fonte/*.mxf`, embutidos no kernel). Cobrem ASCII, Latin-1 (todos os
  acentos do português) e alguns símbolos: aspas curvas, travessões, reticências, marcador, setas e ✓. A Inter é
  proporcional: a largura de cada texto é medida (`Fonte::largura`) e o que não cabe é cortado com reticências
  (`fonte::cortar`).
- **Licenças**: as duas fontes são da SIL Open Font License 1.1; os textos estão em
  `ferramentas/fontes/Inter-LICENCA.txt` e `ferramentas/fontes/JetBrainsMono-LICENCA.txt`. A licença permite
  embutir as fontes (convertidas) no kernel e redistribuir, desde que a licença vá junto.
- **Textos**: janelas, menus, botões, mensagens, a saída do shell e os arquivos de exemplo estão em português com
  acento ("Configurações", "Área de trabalho", "Memória", "Não"...). Os nomes no código e os comentários
  continuam sem acento.
- **Digitar acentos**: no ABNT2 as teclas mortas (agudo, crase, til, circunflexo e trema) e a tecla do `ç`
  produzem os caracteres de verdade nos campos de texto, no editor, na barra de endereço do navegador, nos nomes
  de arquivo e no terminal. A serial fala UTF-8 nos dois sentidos: o shell monta a linha em UTF-8 e o Backspace
  apaga o caractere inteiro (os dois bytes de um `é`), uma célula na tela.
- **Nomes de arquivo**: o FAT32 grava o nome longo em UTF-16 (só o plano básico, como o Windows faz na prática) e o
  lê de volta em UTF-8, então `/documentos/Anotações.txt` funciona no MXOS e no Windows. O nome curto 8.3 continua
  ASCII sem acento (`ANOTAC~1.TXT`). A busca não diferencia maiúsculas de minúsculas, acentos inclusive
  (`anotações.txt` acha `ANOTAÇÕES.TXT`).
- **Shell**: `cat` mostra texto UTF-8 (bytes inválidos e controles viram `.`) e `http` mostra o corpo como UTF-8.

```
mxos> escrever /documentos/Olá.txt Ação, pão e café
gravado: /documentos/Olá.txt (21 bytes)
mxos> cat /documentos/Anotações.txt
```

## Programas de Linux e BusyBox (0.9 e 0.10)

O MXOS roda programas de **Linux aarch64** sem mudar nada: o mesmo binário que roda num Raspberry Pi com Linux roda
aqui. Não é emulador: o código é ARM64 nativo, e o MXOS responde às chamadas de sistema como o Linux responderia
(números, estruturas e erros iguais). Estáticos (musl ou glibc) e dinâmicos (o `ld-linux` da glibc carrega as `.so`
de `/lib`), com threads, sinais e rede.

```
mxos> executar sh -l                        # shell do BusyBox (ash) na janela do terminal
~ # cd /documentos
/documentos # ls -l
-rwxr-xr-x    1 root     root           635 Oct 29 04:16 Anotações.txt
/documentos # cat notas.txt | grep FAT | wc -l
/documentos # vi notas.txt                  # o vi do BusyBox, com :wq gravando no FAT32
/documentos # exit
mxos> executar df -h                        # sem /bin/df: o BusyBox faz o papel (como os links do Linux)
mxos> executar /bin/ola um "dois três"      # programa de Rust + musl (ferramentas/ola)
mxos> executar wget -O - http://example.com/ # rede: DNS (UDP), TCP; ping e nslookup também
mxos> executar httpd -f -p 80 -h /paginas &  # servidor: o PC abre http://localhost:8080 (rodar.py --porta 8080:80)
mxos> executar busybox-dinamico ls -l /lib   # programa dinâmico: ld-linux + libc.so.6 + libresolv.so.2
mxos> executar ola threads                   # std::thread, Mutex, Condvar, canais
mxos> executar ola sinais                    # tratadores, raise, kill, alarm + pause, SIGCHLD
```

- **O que está no disco**: `/bin/busybox` (o `busybox-static` 1.37 arm64 do Debian, GPLv2, licença em
  `ferramentas/BusyBox-LICENCA.txt`; `ferramentas/extrair_deb.py` tira o binário do `.deb`), `/bin/busybox-dinamico`
  (o `busybox` comum do Debian), `/bin/ola`, `/lib` com `ld-linux-aarch64.so.1`, `libc.so.6` e `libresolv.so.2` (do
  `libc6` 2.41 do Debian, LGPL, licença em `ferramentas/GLIBC-LICENCA.txt`) e `/etc` (`passwd`, `group`, `hostname`,
  `profile`, `resolv.conf`, `hosts`, `nsswitch.conf`). Tudo de `ferramentas/binarios/` vai para `/bin` e tudo de
  `ferramentas/bibliotecas/` para `/lib`.
- **`executar PROGRAMA [args] [&]`**: sem `/` procura em `/bin`; um programa que não existe em `/bin`, `/sbin`,
  `/usr/bin` ou `/usr/sbin` vira um applet do BusyBox. Aspas (`"..."` e `'...'`) juntam argumentos; `&` no fim roda
  no fundo. Enquanto o programa roda, o teclado é dele (setas, Home, End, Delete e Ctrl+letra também).
- **Testado**: `sh` interativo (histórico com as setas, `cd`, `for`, pipes, `&&`), `vi`, `ls`, `cat`, `grep`, `sed`,
  `awk`, `find`, `xargs`, `sort`, `head`, `wc`, `md5sum`, `tar`, `gzip`/`gunzip`, `df`, `free`, `ps`, `uptime`,
  `date`, `uname`, `id`, `env`, `sleep`, `trap`, `wait`, `wget`, `nslookup`, `ping`, `nc` (cliente e `-l`),
  `httpd` e Ctrl+C (mata o comando; o shell, que trata o SIGINT, continua).
- **Sinais** (0.10): o tratador do programa roda de verdade. O kernel monta na pilha o `rt_sigframe` do Linux
  (registradores, FPSIMD, máscara) e o `rt_sigreturn` volta; máscara (`rt_sigprocmask`), `rt_sigsuspend`,
  `SA_RESTART` (só onde o Linux repete: `poll`, `nanosleep` e `sigsuspend` voltam `EINTR`), `SIGCHLD` para o pai,
  `alarm`/`setitimer` com `SIGALRM`.
- **Threads** (0.10): `clone` com `CLONE_THREAD` divide a memória, o `brk` e os descritores; `getpid` é o do grupo,
  `exit` sai só da thread, `CLONE_CHILD_CLEARTID` + `futex` (WAIT/WAKE/REQUEUE, com prazo) fazem o `pthread_join`.
- **Rede** (0.10): sockets `AF_INET` sobre a pilha TCP/IP do MXOS: TCP cliente e servidor (`connect`, `listen`,
  `accept`), UDP e ping (ICMP), com `poll`, `O_NONBLOCK`, `sendmsg`/`recvmsg`/`sendmmsg`. Pacotes para o próprio
  IP (e 127.x) não saem pela placa. O DNS vem do `/etc/resolv.conf` (o do QEMU, 10.0.2.3).
- **Terminal**: o console da janela entende as sequências ANSI que o shell e o `vi` usam (mover o cursor, apagar
  linha e tela, cores, tela alternativa) e responde ao `ESC[6n` (posição do cursor) com a posição de verdade.
- **Ainda não**: `AF_UNIX` e IPv6 (os programas recebem o erro do Linux sem suporte e seguem), links simbólicos e
  permissões (o FAT32 não guarda), `SIGSTOP`/job control de verdade (Ctrl+Z), tratador para `SIGSEGV`.
  O comando `syscalls` lista o que os programas pediram e o MXOS não tem.

### Como funciona por dentro

| Parte | O que faz |
|---|---|
| `src/processo/elf.rs` | Lê o ELF64, mapeia os `PT_LOAD` com as permissões certas e monta a pilha inicial do Linux (argc, argv, envp, auxv com `AT_PHDR`, `AT_ENTRY`, `AT_BASE`, `AT_RANDOM`, `AT_HWCAP`...); com `PT_INTERP` mapeia também o `ld-linux` (em 0x7000000) e começa por ele |
| `src/processo/sinais.rs` | Entrega de sinais: o `rt_sigframe` na pilha, o trampolim do `rt_sigreturn` (na página acima da pilha), máscara e `EINTR` |
| `src/processo/soquetes.rs` | Os sockets: `socket`, `connect`, `bind`, `listen`, `accept4`, `sendto`/`recvfrom`, `sendmsg`/`recvmsg`, `sendmmsg`, `getsockopt`, `shutdown` e o `poll` deles |
| `src/rede/soquete.rs` | O lado da rede: várias conexões TCP (cada socket a sua, além da do navegador), portas UDP, cópia do ICMP para o ping |
| `src/processo/chamadas.rs` | As chamadas do Linux aarch64: arquivos (`openat`, `read`, `write`, `lseek`, `getdents64`, `newfstatat`, `mkdirat`, `unlinkat`, `renameat`...), tubos, `dup3`, `fcntl`, `ioctl` de terminal (termios, tamanho da janela, grupo), `ppoll`, memória (`brk`, `mmap`, `munmap`, `mprotect`), processos (`clone`/fork, `execve` com `#!`, `wait4`, `exit_group`, `kill`), tempo, `uname`, `prlimit64`, `getrandom`... |
| `src/processo/arquivos.rs` | O que um descritor aponta: terminal, `/dev/null`, `/dev/zero`, `/dev/urandom`, arquivo do FAT32 (lido inteiro no open, gravado no último close), pasta, tubo |
| `src/processo/proc.rs` | `/proc` feito na hora: `meminfo`, `mounts`, `uptime`, `cpuinfo`, `stat`, `PID/stat`, `PID/cmdline`, `self/exe`... |
| `ferramentas/ola/` | Programa de teste em Rust (`aarch64-unknown-linux-musl`), ligado pelo `rust-lld` (sem gcc no PC) |

## Processos (EL0) e chamadas de sistema

Programas rodam em EL0, cada um com a própria memória, e falam com o kernel por `svc` com os números e as regras do
Linux aarch64 (número em `x8`, argumentos em `x0`-`x5`, resposta em `x0`, erro = `-errno`).

- **Como o kernel roda um programa**: o laço principal (área de trabalho ou shell da serial) chama `processo::rodar`
  a cada volta. `entrar_usuario` (em `vetores.s`) guarda os registradores do kernel e faz `eret` para o programa.
  Qualquer exceção vinda dele (SVC, interrupção, erro) cai nos vetores de "EL inferior", que salvam o programa inteiro
  (x0-x30, SP_EL0, PC, SPSR, TPIDR_EL0, q0-q31, FPCR/FPSR) e voltam para o kernel como se `entrar_usuario` tivesse
  retornado. Assim a chamada de sistema roda no kernel normal: pode alocar, ler o disco, esperar.
- **Preempção**: o timer de 100 Hz tira o programa da CPU; os prontos se revezam (round-robin) e a área de trabalho
  continua respondendo. Com processo pronto o laço não dorme no WFI; sem nenhum, a CPU volta a ficar ~97% ociosa.
- **Memória**: o kernel continua no mapa identidade, só para EL1, em todas as tabelas. Cada processo tem a raiz
  própria (com ASID no TTBR0) e as páginas dele de `0x1000` a `0x8000000` (128 MB abaixo dos periféricos), não
  globais, com código só leitura + execução e dados sem execução. As páginas vêm do heap; o kernel lê e escreve a
  memória do programa pelos quadros, sem depender da tabela ativa.
- **Erros**: um acesso inválido ou instrução desconhecida mata só o processo, com a mensagem no terminal
  (`[processo 2 (teste-falha) morreu: falha de segmentação ao escrever 0x10, no PC 0x40009c]`, código 139).
- **Esperar**: `read` sem tecla, `nanosleep` e `wait4` voltam o PC para o `svc` e marcam o processo; quando a condição
  muda ele roda de novo e repete a chamada.
- **Terminal**: `write` em 1 e 2 vai para o terminal; o processo em primeiro plano recebe o teclado com eco e edição
  de linha (Backspace, Enter, Ctrl+D = fim de arquivo, Ctrl+C = mata).

Comandos: `processo ola|conta|falha|eco [&]` (programa de teste embutido, em assembly; `&` roda no fundo),
`processos` (pid, estado, CPU, memória), `matar PID` e `syscalls` (chamadas pedidas que ainda não existem). O app
**Tarefas** ganhou a aba **Processos**, com a CPU de cada um e "Finalizar processo".

## CSS no navegador

O navegador lê CSS de `<style>`, do atributo `style="..."` e de `<link rel="stylesheet">` (baixado do disco ou por
`http://` antes de montar a página; até 4 folhas de 256 KB). É um subconjunto leve, feito do zero em
`src/navegador/css.rs`:

| O quê | Suportado |
|---|---|
| Seletores | tipo (`p`), `.classe`, `#id`, `*`, descendente (`div p`), filho (`ul > li`), lista com vírgula, `a:link`/`a:visited` |
| Cascata | especificidade, ordem no texto, `!important`, `style="..."` acima dos seletores; herança das propriedades de texto |
| Texto | `color`, `font-weight`, `font-style`, `font-size` (px, pt, em, rem, %, palavras), `font-family` (monospace ou não), `font` (atalho), `text-align`, `text-decoration`, `line-height` |
| Caixas | `margin` (com `auto` para centralizar), `padding`, `border` e `border-*` (largura, estilo, cor), `width`, `max-width`, `background(-color)` |
| Outros | `display: none / block / inline`, `list-style: none`, cores por nome, `#rgb`, `#rrggbb`, `rgb()`, `rgba()`, `light-dark()` |
| HTML antigo | `align`, `bgcolor`, `<font color>`, `<body text>`, `<center>`, `hidden` |

Fica de fora: flexbox, grid, `position`, `float`, `@media` (pulado inteiro), `@font-face`, seletores de atributo,
`:hover` e `::before` (a regra inteira é ignorada), animações. A fonte continua sendo uma das quatro pré-renderizadas:
`font-size` escolhe a mais perto (até 18 px a normal, até 25 px a de 20 px, acima a de 32 px).

Como cabe num motor sem árvore: o construtor do HTML guarda a pilha dos elementos abertos (para casar os seletores e
herdar o estilo). O que é de texto vai para cada trecho (cor, fundo, negrito...); o que é de caixa vira uma `Caixa`
do documento, e cada bloco sabe em que caixas está. Na diagramação as caixas abrem e fecham entre um bloco e outro;
o fundo e as bordas entram antes do texto e ganham a altura quando a caixa fecha.

A página `disco:/paginas/css.html` (com `estilo.css` por `<link>`) mostra tudo isso.

## Na TV box (ainda não testado no hardware)

1. Solde ou encoste um adaptador **USB-TTL de 3,3 V** nos pinos TX/RX/GND da placa (115200 baud).
2. Copie `saida/mxos-mxq.img` para um cartão SD FAT32.
3. Ligue a box e aperte uma tecla na serial para parar no prompt do U-Boot.
4. Digite:
   ```
   fatload mmc 0 0x1080000 mxos-mxq.img
   booti 0x1080000 - ${dtb_mem_addr}
   ```

## Próximos passos

1. ~~MMU e alocador de memória~~ (feito na 0.3).
2. ~~Timer e interrupções (GIC-400) para o kernel parar de ficar em loop esperando a serial~~ (feito na 0.4).
3. ~~Vídeo e som no QEMU~~ (feito na 0.5). Na box: ligar o framebuffer do U-Boot (`video/mxq.rs`) e escrever o driver de som HDMI.
4. ~~Área de trabalho, disco FAT32, rede TCP/IP e motor de navegador no QEMU~~ (feito na 0.6).
5. ~~Aplicativos da área de trabalho: gerenciador de arquivos e discos, gerenciador de tarefas, navegador,
   personalização, configurações e painel de drivers~~ (feito na 0.7).
6. ~~Fontes com antialiasing e acentos em todo o sistema~~ (feito na 0.8).
7. ~~Processos em EL0 e chamadas de sistema (`svc`)~~ (feito na 0.9).
8. ~~CSS no navegador~~ (feito na 0.9).
9. ~~Carregador de ELF e compatibilidade com programas de Linux aarch64 (BusyBox)~~ (feito na 0.9).
   ~~Sockets, threads, sinais com tratador e programas dinâmicos (`ld-linux`)~~ (feito na 0.10).
10. FAT32 no cartão SD da TV box (o FAT32 já existe; falta o driver do SD da Amlogic).
11. PE (`.exe`) compilado para ARM64.
12. `.exe` de Windows x86: precisa de um tradutor de instruções x86 para ARM e de uma reimplementação da API do Windows (o que o Box64 e o Wine fazem juntos). É o projeto mais longo da lista.
