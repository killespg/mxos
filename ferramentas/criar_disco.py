"""Cria do zero a imagem de disco do MXOS (FAT32) e confere imagens FAT32.

Sem mkfs e sem bibliotecas de fora: o MBR, o setor de boot, o FSInfo, as
duas FATs e as pastas sao montados aqui byte a byte.

Formato (o mesmo que o kernel le em src/armazenamento/fat32.rs):
  - 64 MB, MBR com UMA particao FAT32 (tipo 0x0C, LBA) comecando no setor 2048
    (com --sem-mbr o FAT32 ocupa o disco inteiro, "superfloppy")
  - setores de 512 bytes, 1 setor por cluster (o certo para FAT32 deste
    tamanho: da uns 127 mil clusters, acima do minimo de 65525)
  - 32 setores reservados, FSInfo no setor 1, copia do boot no setor 6
  - duas FATs iguais, pasta raiz no cluster 2, rotulo "MXOS"
  - nomes longos (LFN) para tudo que nao cabe em 8.3 maiusculo, em UTF-16
    ("Anotações.txt"); o nome curto fica em ASCII sem acento (ANOTAC~1.TXT)
  - os textos de exemplo sao UTF-8

Uso:
  python -I ferramentas/criar_disco.py saida/disco.img            cria a imagem
  python -I ferramentas/criar_disco.py saida/disco.img --sem-mbr  sem tabela de particoes
  python -I ferramentas/criar_disco.py --verificar saida/disco.img
        confere a imagem (FATs, cadeias, tamanhos, LFN, FSInfo) e lista tudo:
          DIR /caminho
          ARQ /caminho TAMANHO
        termina com "OK" (codigo 0) ou com os erros (codigo 1)
  python -I ferramentas/criar_disco.py --verificar saida/disco.img --ler /paginas/index.html
        mostra o conteudo de um arquivo
"""

import argparse
import pathlib
import struct
import sys
import time

SETOR = 512
TAMANHO_PADRAO = 64 * 1024 * 1024
INICIO_PARTICAO = 2048
RESERVADOS = 32
FATS = 2
SETORES_POR_CLUSTER = 1
SETOR_FSINFO = 1
SETOR_COPIA_BOOT = 6
FIM_CADEIA = 0x0FFFFFFF
RUIM = 0x0FFFFFF7
ROTULO = "MXOS"

ATTR_SO_LEITURA = 0x01
ATTR_OCULTO = 0x02
ATTR_SISTEMA = 0x04
ATTR_ROTULO = 0x08
ATTR_PASTA = 0x10
ATTR_ARQUIVO = 0x20
ATTR_LFN = 0x0F

# caracteres permitidos num nome curto alem de letras e digitos
ESPECIAIS_CURTOS = set("$%'-_@~`!(){}^#&")
PROIBIDOS_LONGOS = set('\\/:*?"<>|')
CHARS_POR_LFN = 13


# ---------------------------------------------------------------- conteudo

LEIAME = """MXOS — disco de exemplo
=======================

Este disco foi criado do zero pelo ferramentas/criar_disco.py.
É um FAT32 de 64 MB com uma partição e nomes longos.

Pastas:
  /config       configurações do sistema (começa vazia)
  /paginas      páginas HTML para o navegador
  /documentos   textos de exemplo

Comandos para testar no shell do MXOS:
  ls /
  cat /LEIAME.TXT
  escrever /documentos/teste.txt olá mundo
  df
"""

INDEX_HTML = """<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<title>MXOS — Página inicial</title>
<style>body { color: black; }</style>
</head>
<body>
<h1>Bem-vindo ao MXOS</h1>
<p>Esta página está no <b>disco FAT32</b> do MXOS e foi aberta pelo
<i>navegador</i> escrito do zero.</p>
<h2>O que o MXOS tem</h2>
<ul>
<li>Kernel ARM64 em Rust, sem bibliotecas prontas</li>
<li>Interface gráfica leve com janelas</li>
<li>Disco virtio-blk com FAT32 e nomes longos</li>
<li>Rede com TCP/IP próprio</li>
</ul>
<h2>Passos para começar</h2>
<ol>
<li>Abra o terminal</li>
<li>Digite <code>ls /paginas</code></li>
<li>Leia a página <a href="sobre.html">Sobre o MXOS</a></li>
</ol>
<h2>Mais páginas</h2>
<ul>
<li><a href="guia.html">Guia do MXOS</a>: aplicativos, atalhos e comandos (página longa)</li>
<li><a href="tabelas.html">Tabelas e listas</a>: o que o navegador sabe mostrar</li>
<li><a href="teste.html">Teste do navegador</a>: estilos, links e páginas de erro</li>
<li><a href="css.html">Teste de CSS</a>: cores, bordas, caixas e alinhamento</li>
</ul>
<hr>
<p>Veja também as <a href="/documentos/notas.txt">notas</a> e o
<a href="disco:/LEIAME.TXT">LEIAME</a>.</p>
</body>
</html>
"""

SOBRE_HTML = """<html>
<head><meta charset="utf-8"><title>Sobre o MXOS</title></head>
<body>
<h1>Sobre o MXOS</h1>
<p>O MXOS é um sistema operacional de estudo para a TV box
<b>MXQ PRO 4K</b> (Amlogic S905W), testado primeiro no QEMU.</p>
<h3>Partes do sistema</h3>
<ul>
<li>Memória: MMU com mapa identidade e heap próprio</li>
<li>Interrupções: GICv2 e timer de 100 Hz</li>
<li>Vídeo: framebuffer e as fontes Inter e JetBrains Mono, com acentos</li>
</ul>
<blockquote>Feito do zero, um byte de cada vez.</blockquote>
<pre>
  mxos&gt; ls /
  mxos&gt; navegar disco:/paginas/index.html
</pre>
<p><a href="index.html">Voltar para a página inicial</a></p>
</body>
</html>
"""

GUIA_HTML = """<!DOCTYPE html>
<html>
<head><meta charset="utf-8"><title>Guia do MXOS</title></head>
<body>
<h1>Guia do MXOS</h1>
<p>Este guia mostra a área de trabalho do MXOS por dentro: os aplicativos,
os atalhos de teclado e os comandos do terminal. A página é comprida de
propósito, para testar a rolagem do navegador: use a roda do mouse, as setas,
<kbd>PgUp</kbd>, <kbd>PgDn</kbd>, a barra de espaço, <kbd>Home</kbd> e
<kbd>End</kbd>, ou arraste a barra de rolagem à direita.</p>

<h2>1. A área de trabalho</h2>
<p>A tela tem 1024 por 768 pixels. Embaixo fica a <b>barra de tarefas</b>, com o
botão <b>MXOS</b> (o menu iniciar), um botão para cada janela aberta e o relógio.
No papel de parede ficam os ícones dos aplicativos mais usados: um clique
seleciona, dois cliques seguidos abrem.</p>
<ul>
<li>Clique numa janela para trazer ela para a frente.</li>
<li>Arraste a janela pela barra de título.</li>
<li>O botão <code>[_]</code> minimiza; o botão da barra de tarefas traz de volta.</li>
<li>O botão <code>[x]</code> fecha a janela.</li>
</ul>
<p>Tudo é desenhado num buffer na memória e só o pedaço que mudou vai para a
tela. Mexer o mouse redesenha só o cursor. Entre um quadro e outro a CPU dorme
esperando a próxima interrupção, então a área de trabalho parada quase não
gasta processador.</p>

<h2>2. Aplicativos</h2>
<dl>
<dt>Arquivos e Discos</dt>
<dd>Navega pelas pastas do disco FAT32: cria pastas, renomeia, apaga, copia e
cola arquivos. A vista de discos mostra o tamanho e o espaço livre de cada um.</dd>
<dt>Navegador</dt>
<dd>Este programa. Abre páginas do disco (<code>disco:/</code>) e da rede
(<code>http://</code>), com histórico, rolagem e links.</dd>
<dt>Editor de texto</dt>
<dd>Abre, edita e salva arquivos de texto do disco.</dd>
<dt>Tarefas</dt>
<dd>Mostra as janelas abertas com o tempo de CPU de cada uma, o uso do
processador e da memória, e finaliza o programa que travar.</dd>
<dt>Drivers</dt>
<dd>Lista os dispositivos que o kernel achou (processador, interrupções,
relógio, serial, vídeo, disco, rede, teclado e mouse) e o driver de cada um.</dd>
<dt>Configurações e Personalizar</dt>
<dd>Tema de cores, papel de parede, relógio, ícones, teclado (US ou ABNT2),
rede, disco e sistema. O botão Salvar guarda tudo em
<code>/config/mxos.cfg</code>.</dd>
<dt>Terminal</dt>
<dd>O shell do MXOS numa janela de 80 colunas.</dd>
<dt>Calculadora e Pintura</dt>
<dd>Dois programas pequenos para testar o mouse e o teclado.</dd>
</dl>

<h2>3. Atalhos de teclado</h2>
<table>
<tr><th>Tecla</th><th>Onde</th><th>O que faz</th></tr>
<tr><td>Alt+Tab</td><td>área de trabalho</td><td>traz a próxima janela para a frente</td></tr>
<tr><td>Alt+F4</td><td>área de trabalho</td><td>fecha a janela com foco</td></tr>
<tr><td>tecla Windows</td><td>área de trabalho</td><td>abre e fecha o menu iniciar</td></tr>
<tr><td>Esc</td><td>menu iniciar</td><td>fecha o menu</td></tr>
<tr><td>F5</td><td>navegador</td><td>recarrega a página</td></tr>
<tr><td>F6</td><td>navegador</td><td>vai para a barra de endereço</td></tr>
<tr><td>Backspace</td><td>navegador</td><td>volta para a página anterior</td></tr>
<tr><td>espaço ou PgDn</td><td>navegador</td><td>desce uma tela</td></tr>
<tr><td>Home e End</td><td>navegador</td><td>vai para o começo ou para o fim da página</td></tr>
</table>

<h2>4. Comandos do terminal</h2>
<p>O terminal aceita os mesmos comandos da serial. Alguns exemplos:</p>
<pre>
mxos&gt; help                      lista todos os comandos
mxos&gt; info                      processador, memória e versão
mxos&gt; ls /paginas               arquivos de uma pasta
mxos&gt; cat /LEIAME.TXT           mostra um arquivo
mxos&gt; escrever /a.txt olá       grava um arquivo
mxos&gt; df                        espaço livre no disco
mxos&gt; ip                        configuração da rede
mxos&gt; ping 10.0.2.2             testa a rede
mxos&gt; navegar disco:/paginas/guia.html
</pre>
<p>O comando <code>navegar</code> mostra a mesma página em modo texto, com os
links numerados: <code>navegar 3</code> segue o terceiro link.</p>

<h2>5. Como o navegador funciona</h2>
<ol>
<li><b>Carregar</b>: lê o arquivo do disco ou faz um GET pela rede. Texto puro
vira um bloco pré-formatado.</li>
<li><b>Analisar</b>: um leitor de HTML de uma passada só transforma as tags em
blocos (parágrafo, título, item de lista, linha de tabela) com trechos de
texto estilizados. Não existe árvore de elementos.</li>
<li><b>Diagramar</b>: quebra o texto em linhas na largura da janela e calcula
a posição de cada pedaço. Isso acontece uma vez só, quando a página chega.</li>
<li><b>Desenhar</b>: pinta só o que aparece. Ao rolar, os pixels que já estavam
na tela são movidos e só a faixa nova é desenhada.</li>
</ol>
<blockquote>Sem JavaScript, sem CSS e sem imagens: o texto alternativo das
imagens aparece entre colchetes.</blockquote>

<h2>6. Por dentro do kernel</h2>
<p>O MXOS começa no <code>boot.s</code>: escolhe o primeiro núcleo, desce do
nível de exceção 2 para o 1, liga a unidade de ponto flutuante, prepara as
pilhas e zera o BSS. Depois o Rust assume: liga a MMU com páginas de 4 KB,
cria o heap, liga o controlador de interrupções e o timer de 100 Hz, procura o
vídeo, o disco, a rede, o teclado e o mouse, e abre a área de trabalho.</p>
<p>Cada dispositivo do QEMU fala o protocolo virtio: uma fila de descritores
na memória que o dispositivo lê e devolve. O mesmo transporte serve para o
disco, a rede, o teclado, o mouse e o som.</p>
<p>A rede foi escrita camada por camada: Ethernet, ARP, IPv4, ICMP, UDP, DHCP,
DNS e TCP, e por cima de tudo o HTTP que traz as páginas deste navegador.
Tudo funciona por consulta: ninguém mexe na rede dentro de uma interrupção.</p>
<p>O disco usa FAT32 com nomes longos. O mesmo formato é criado no PC pelo
<code>ferramentas/criar_disco.py</code>, que também confere o que o MXOS gravou.</p>

<h2>7. Perguntas frequentes</h2>
<h4>Por que um site https não abre?</h4>
<p>O https precisa de criptografia (TLS), que o MXOS ainda não tem. O
navegador mostra uma página explicando isso e oferece a versão http.</p>
<h4>A área de trabalho parou enquanto a página carregava</h4>
<p>Disco e rede ainda são síncronos: enquanto a página chega, nada mais roda.
Por isso o navegador mostra "Carregando" antes de buscar.</p>
<h4>Onde ficam as configurações?</h4>
<p>Em <a href="disco:/config/">/config</a>, no arquivo <code>mxos.cfg</code>,
com uma linha <code>chave=valor</code> para cada opção.</p>

<hr>
<p><a href="index.html">Página inicial</a> |
<a href="tabelas.html">Tabelas e listas</a> |
<a href="teste.html">Teste do navegador</a></p>
</body>
</html>
"""

TABELAS_HTML = """<html>
<head><meta charset="utf-8"><title>Tabelas e listas</title></head>
<body>
<h1>Tabelas e listas</h1>
<p>O navegador alinha as tabelas em colunas: cada coluna fica com a largura
do texto mais comprido dela e, quando a tabela não cabe na janela, o texto
quebra dentro da célula. Células de cabeçalho ficam em negrito, com fundo.</p>

<h2>Partes do kernel</h2>
<table>
<tr><th>Parte</th><th>Arquivos</th><th>O que faz</th></tr>
<tr><td>Boot</td><td><code>boot.s</code></td><td>escolhe o núcleo, desce para EL1 e prepara as pilhas</td></tr>
<tr><td>Memória</td><td><code>mmu.rs</code>, <code>heap.rs</code></td><td>páginas de 4 KB e o alocador</td></tr>
<tr><td>Interrupções</td><td><code>gic.rs</code>, <code>irq.rs</code></td><td>GICv2, timer e serial</td></tr>
<tr><td>Disco</td><td><code>armazenamento/</code></td><td>virtio-blk e FAT32</td></tr>
<tr><td>Rede</td><td><code>rede/</code></td><td>TCP/IP e HTTP</td></tr>
<tr><td>Navegador</td><td><code>navegador/</code></td><td>HTML, diagramação e desenho</td></tr>
<tr><td>Área de trabalho</td><td><code>gui/</code></td><td>janelas, temas e aplicativos</td></tr>
</table>

<h2>Números</h2>
<table>
<tr><th>Medida</th><th>Valor</th></tr>
<tr><td>Tela</td><td>1024 x 768 pixels</td></tr>
<tr><td>Fonte</td><td>Inter de 13, 20 e 32 pixels; terminal em JetBrains Mono 8 x 16</td></tr>
<tr><td>Tique do timer</td><td>100 por segundo</td></tr>
<tr><td>Disco de exemplo</td><td>64 MB em FAT32</td></tr>
<tr><td>Barra de tarefas</td><td>32 pixels</td></tr>
</table>

<h2>Uma tabela larga</h2>
<p>Esta tem texto demais para uma linha só: as colunas dividem o espaço e
as palavras quebram dentro das células.</p>
<table>
<tr><th>Camada</th><th>Protocolos</th><th>Como funciona no MXOS</th></tr>
<tr><td>Enlace</td><td>Ethernet e ARP</td><td>Cada quadro leva o endereço MAC de destino e o de origem. O ARP pergunta na rede local quem tem um IP e guarda a resposta numa tabela pequena.</td></tr>
<tr><td>Rede</td><td>IPv4 e ICMP</td><td>O IPv4 decide se o pacote vai direto ou pelo gateway. O ICMP responde ao ping e mede o tempo de ida e volta.</td></tr>
<tr><td>Transporte</td><td>UDP e TCP</td><td>O UDP leva o DHCP e o DNS. O TCP abre uma conexão por vez, com retransmissão.</td></tr>
<tr><td>Aplicação</td><td>HTTP/1.1</td><td>Só GET, com Content-Length, chunked ou até o servidor fechar, seguindo até cinco redirecionamentos.</td></tr>
</table>

<h2>Listas</h2>
<ul>
<li>Lista sem número
<ul>
<li>segundo nível
<ul><li>terceiro nível</li></ul>
</li>
<li>outro item do segundo nível</li>
</ul>
</li>
<li>Item com <b>negrito</b>, <i>itálico</i> e <code>código</code></li>
<li>Item com um <a href="guia.html">link para o guia</a></li>
</ul>
<ol start="5">
<li>Lista numerada que começa no 5</li>
<li>Seis</li>
<li>Sete, com um texto comprido que precisa quebrar a linha para mostrar
que o recuo continua alinhado embaixo do texto, e não embaixo do número.</li>
</ol>

<h2>Definições</h2>
<dl>
<dt>FAT32</dt>
<dd>Sistema de arquivos com duas cópias da tabela de alocação e nomes longos.</dd>
<dt>virtio</dt>
<dd>Dispositivos virtuais do QEMU: disco, rede, teclado, mouse e som.</dd>
<dt>WFI</dt>
<dd>Instrução que faz a CPU dormir até a próxima interrupção.</dd>
</dl>

<hr>
<p><a href="index.html">Página inicial</a> |
<a href="guia.html">Guia do MXOS</a> |
<a href="teste.html">Teste do navegador</a></p>
</body>
</html>
"""

TESTE_HTML = """<html>
<head><meta charset="utf-8"><title>Teste do navegador</title></head>
<body>
<h1>Teste do navegador</h1>
<p>Uma página para conferir o que o motor de HTML do MXOS faz.</p>

<h2>Estilos</h2>
<p>Texto normal, <b>negrito</b>, <i>itálico</i>, <u>sublinhado</u>,
<code>código</code>, <kbd>Ctrl</kbd>, <b><i>negrito e itálico</i></b> e
<em>ênfase</em>.</p>
<p>Entidades: &amp; &lt; &gt; &quot; &copy; 2026, &eacute; vira é,
&ccedil; vira ç, 3 &times; 4, meio = &frac12; e &hellip;</p>
<p>Uma quebra<br>de linha forçada<br>e uma imagem sem arquivo:
<img src="logo.png" alt="Logo do MXOS">.</p>

<h3>Título de nível 3</h3>
<h4>Título de nível 4</h4>
<p>Os títulos 1 a 3 usam a fonte grande; do 4 em diante, a normal em negrito.</p>

<h2>Texto pré-formatado</h2>
<pre>
+-----------+     +-----------+     +-----------+
| carregar  | --> | analisar  | --> | diagramar |
+-----------+     +-----------+     +-----------+
                                          |
                                          v
                                    +-----------+
                                    | desenhar  |
                                    +-----------+
</pre>

<h2>Citação</h2>
<blockquote>Um sistema pequeno o bastante para caber inteiro na cabeça de
quem lê o código.
<blockquote>Citação dentro de citação.</blockquote>
</blockquote>

<h2>Links</h2>
<ul>
<li><a href="index.html">Página inicial</a> (link relativo)</li>
<li><a href="/paginas/guia.html">Guia do MXOS</a> (caminho a partir da raiz)</li>
<li><a href="disco:/paginas/">A pasta /paginas</a> (lista os arquivos)</li>
<li><a href="../LEIAME.TXT">LEIAME.TXT</a> (texto puro, uma pasta acima)</li>
<li><a href="disco:/documentos/notas.txt">notas.txt</a></li>
<li><a href="nao-existe.html">Uma página que não existe</a> (página de erro)</li>
<li><a href="https://example.com/">https://example.com/</a> (aviso de só http)</li>
<li><a href="http://example.com/">http://example.com/</a> (precisa de internet)</li>
<li><a href="http://10.0.2.2:8000/">http://10.0.2.2:8000/</a> (servidor no PC:
<code>python -m http.server</code>)</li>
</ul>
<p>Com o botão do meio do mouse o link abre numa janela nova.</p>

<h2>Formulário</h2>
<p>Campos aparecem só como texto: <input type="text" name="q">
<input type="submit" value="Buscar"></p>

<hr>
<p>Fim do teste. <a href="tabelas.html">Tabelas e listas</a> |
<a href="guia.html">Guia do MXOS</a></p>
</body>
</html>
"""

NOTAS = """Notas do MXOS
- O disco usa FAT32 com duas cópias da FAT.
- Arquivos com nomes longos usam entradas LFN.
- O kernel atualiza o FSInfo a cada escrita.
"""

COMPRAS = """arroz
feijão
café
"""

ANOTACOES = """Anotações
=========

Este arquivo tem nome longo com acentos: no disco ele é gravado em UTF-16
(entradas LFN) e o nome curto fica ANOTAC~1.TXT, sem acento.

Lembretes:
- Reunião às 9h na sala de configuração.
- Ligar para a Conceição sobre o orçamento da área de trabalho.
- Comprar pão, maçã, açúcar e café.
- Revisar a seção “Memória” do manual — página 3.

Frases para testar o teclado ABNT2:
  À noite, vovô Kowalsky vê o ímã cair no pé do pinguim queixoso e vovó
  põe açúcar no chá de tâmaras do jabuti feliz.
  Ç ç Á á À à Â â Ã ã É é Ê ê Í í Ó ó Ô ô Õ õ Ú ú Ü ü
"""


CSS_HTML = """<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<title>Teste de CSS</title>
<link rel="stylesheet" href="estilo.css">
<style>
/* seletores de tipo, classe, id, descendente e filho */
h1 { color: #1d4ed8; text-align: center; }
.aviso { background-color: #fff7d6; border: 1px solid #e0b000; padding: 8px 12px; margin: 10px 0; }
.aviso b { color: #a15c00; }
#centro { text-align: center; color: rgb(120, 40, 160); }
.direita { text-align: right; }
.cartao { width: 300px; margin: 12px auto; padding: 10px; border: 2px solid #3b82f6; background: #eff6ff; }
.cartao > p { margin: 0; }
.menu { list-style: none; background: #1f2937; color: #9ca3af; padding: 6px 10px; }
.menu li { display: inline; }
.menu a { color: white; text-decoration: none; font-weight: bold; }
.mono { font-family: Consolas, monospace; color: green; }
.grande { font-size: 22px; }
.enorme { font-size: 2em; color: crimson; }
.escondido { display: none; }
.espacado { line-height: 1.9; }
.marca { background: yellow; }
.forte { color: red !important; }
p.especifico { color: teal; }
p { color: #333333; }
</style>
</head>
<body>
<h1>Teste de CSS</h1>
<ul class="menu"><li><a href="index.html">Início</a> |</li><li><a href="guia.html">Guia</a> |</li><li><a href="css.html">CSS</a></li></ul>
<p class="intro">Esta página testa o CSS do navegador do MXOS: cores, fundos, bordas, alinhamento,
margens, recheio, largura e <span class="marca">texto marcado</span>.</p>
<div class="aviso"><b>Aviso:</b> esta caixa tem fundo amarelo, borda e recheio (padding).</div>
<p id="centro">Este parágrafo está centralizado e roxo (seletor #id).</p>
<p class="direita">Este está alinhado à direita.</p>
<div class="cartao"><p>Cartão com 300 px de largura, centralizado com <code>margin: auto</code>.</p></div>
<p class="especifico">Especificidade: <code>p.especifico</code> ganha de <code>p</code> (verde-azulado).</p>
<p class="forte especifico" style="color: blue">Com <code>!important</code> o vermelho ganha do style="...".</p>
<p style="color: #c2410c; font-weight: bold">Estilo inline (atributo style) laranja e negrito.</p>
<p class="mono">font-family: monospace em verde.</p>
<p class="grande">Texto de 22 px.</p>
<p class="enorme">Texto de 2em.</p>
<p class="escondido">VOCÊ NÃO DEVIA VER ISTO (display: none).</p>
<p class="espacado">Este parágrafo tem line-height 1.9, então as linhas ficam mais afastadas umas das outras
quando o texto é comprido e quebra em mais de uma linha dentro da janela do navegador.</p>
<p class="externo">Esta linha vem da folha externa (estilo.css, por &lt;link&gt;).</p>
<p>Fim da página. <a href="index.html">Voltar</a></p>
</body>
</html>
"""

ESTILO_CSS = """/* folha de estilo de fora, ligada por <link rel="stylesheet"> */
body { background-color: #f8fafc; }
.externo { color: white; background-color: #16a34a; padding: 6px; border-left: 6px solid #14532d; }
@media (max-width: 600px) { body { background: red; } }
"""


# Programas de Linux aarch64 (estaticos) que vao para /bin: ferramentas/binarios/.
# O ola sai de ferramentas/ola (cargo build --release); o busybox vem do Debian.
BINARIOS = pathlib.Path(__file__).resolve().parent / "binarios"


def programas() -> dict:
    """Os executaveis de ferramentas/binarios (os que existirem)."""
    if not BINARIOS.is_dir():
        return {}
    return {arquivo.name: arquivo.read_bytes() for arquivo in sorted(BINARIOS.iterdir()) if arquivo.is_file()}


def conteudo_padrao() -> dict:
    """Arvore do disco: dict = pasta, bytes = arquivo."""
    return {
        "bin": programas(),
        # para os programas de Linux (ls -l, id, whoami e o prompt do sh)
        "etc": {
            "passwd": b"root:x:0:0:root:/:/bin/sh\n",
            "group": b"root:x:0:\n",
            "hostname": b"mxos\n",
            "profile": "# lido pelo sh -l do BusyBox\nexport PS1='\\w # '\necho \"BusyBox no MXOS: help lista os comandos do shell, busybox --list os programas.\"\n".encode("utf-8"),
        },
        "LEIAME.TXT": LEIAME.encode("utf-8"),
        "config": {},
        "paginas": {
            "index.html": INDEX_HTML.encode("utf-8"),
            "sobre.html": SOBRE_HTML.encode("utf-8"),
            "guia.html": GUIA_HTML.encode("utf-8"),
            "tabelas.html": TABELAS_HTML.encode("utf-8"),
            "teste.html": TESTE_HTML.encode("utf-8"),
            "css.html": CSS_HTML.encode("utf-8"),
            "estilo.css": ESTILO_CSS.encode("utf-8"),
        },
        "documentos": {
            "notas.txt": NOTAS.encode("utf-8"),
            "Lista de compras.txt": COMPRAS.encode("utf-8"),
            "Anotações.txt": ANOTACOES.encode("utf-8"),
        },
    }


# ---------------------------------------------------------------- nomes

def checksum_curto(curto: bytes) -> int:
    soma = 0
    for byte in curto:
        soma = (((soma & 1) << 7) + (soma >> 1) + byte) & 0xFF
    return soma


def caractere_curto_valido(c: str) -> bool:
    return ("A" <= c <= "Z") or ("0" <= c <= "9") or c in ESPECIAIS_CURTOS


def partes_83(nome: str):
    """(base, extensao) se o nome cabe em 8.3, senao None."""
    if nome in (".", "..") or nome.count(".") > 1:
        return None
    base, _, ext = nome.partition(".")
    if not 1 <= len(base) <= 8 or len(ext) > 3 or (nome.endswith(".") and not ext):
        return None
    if not all(caractere_curto_valido(c.upper()) for c in base + ext):
        return None
    return base, ext


def caixa_uniforme(texto: str):
    """'maiuscula', 'minuscula' ou None (misturado)."""
    if texto == texto.upper():
        return "maiuscula"
    if texto == texto.lower():
        return "minuscula"
    return None


def nome_83(base: str, ext: str) -> bytes:
    return (base.upper().ljust(8) + ext.upper().ljust(3)).encode("ascii")


def precisa_lfn(nome: str):
    """None se o nome pode ser so curto; senao o motivo. Igual ao kernel:
    8.3 com cada parte toda maiuscula ou toda minuscula usa os bits de
    caixa do byte 12 (como o Windows NT) e dispensa LFN."""
    partes = partes_83(nome)
    if partes is None:
        return "nao cabe em 8.3"
    base, ext = partes
    if caixa_uniforme(base) is None or (ext and caixa_uniforme(ext) is None):
        return "caixa misturada"
    return None


def bits_de_caixa(nome: str) -> int:
    base, ext = partes_83(nome)
    bits = 0
    if base != base.upper():
        bits |= 0x08
    if ext and ext != ext.upper():
        bits |= 0x10
    return bits


# Latin-1 de 0xC0 a 0xFF sem acento (igual ao LATIN1_SEM_ACENTO do kernel)
LATIN1_SEM_ACENTO = "AAAAAAACEEEEIIIIDNOOOOOxOUUUUY?saaaaaaaceeeeiiiidnooooo/ouuuuy?y"


def letra_curta(c: str) -> str:
    """Letra do nome longo no nome curto: "ç" vira "C" (como o kernel)."""
    if 0xC0 <= ord(c) <= 0xFF:
        c = LATIN1_SEM_ACENTO[ord(c) - 0xC0]
    elif not c.isascii():
        return "_"
    c = c.upper()
    return c if caractere_curto_valido(c) else "_"


def limpar_para_curto(texto: str) -> str:
    return "".join(letra_curta(c) for c in texto if c not in " .")


def gerar_curto(nome: str, existentes: set) -> bytes:
    """BASE~N.EXT unico na pasta (o mesmo algoritmo do kernel)."""
    sem_pontos = nome.lstrip(".")
    if "." in sem_pontos:
        base, ext = sem_pontos.rsplit(".", 1)
    else:
        base, ext = sem_pontos, ""
    base = limpar_para_curto(base) or "_"
    ext = limpar_para_curto(ext)[:3]
    for n in range(1, 1_000_000):
        sufixo = f"~{n}"
        candidato = nome_83(base[: 8 - len(sufixo)] + sufixo, ext)
        if candidato not in existentes:
            return candidato
    raise ValueError("sem nome curto livre")


def utf16(nome: str) -> tuple:
    """Unidades UTF-16 do nome (o que vai nas entradas LFN)."""
    dados = nome.encode("utf-16-le")
    return struct.unpack(f"<{len(dados) // 2}H", dados)


def entradas_lfn(nome: str, curto: bytes) -> list:
    """Entradas LFN na ordem do disco (a ultima parte do nome vem primeiro)."""
    unidades = list(utf16(nome))
    if len(unidades) % CHARS_POR_LFN:
        unidades.append(0)
    while len(unidades) % CHARS_POR_LFN:
        unidades.append(0xFFFF)
    soma = checksum_curto(curto)
    pedacos = [unidades[i:i + CHARS_POR_LFN] for i in range(0, len(unidades), CHARS_POR_LFN)]
    entradas = []
    for ordem, pedaco in enumerate(pedacos, start=1):
        e = bytearray(32)
        e[0] = ordem | (0x40 if ordem == len(pedacos) else 0)
        struct.pack_into("<5H", e, 1, *pedaco[0:5])
        e[11] = ATTR_LFN
        e[13] = soma
        struct.pack_into("<6H", e, 14, *pedaco[5:11])
        struct.pack_into("<2H", e, 28, *pedaco[11:13])
        entradas.append(bytes(e))
    return list(reversed(entradas))


def quantas_entradas(nome: str) -> int:
    if precisa_lfn(nome) is None:
        return 1
    return 1 + (len(utf16(nome)) + CHARS_POR_LFN - 1) // CHARS_POR_LFN


def data_hora_fat(segundos: float) -> tuple:
    t = time.localtime(segundos)
    data = ((t.tm_year - 1980) << 9) | (t.tm_mon << 5) | t.tm_mday
    hora = (t.tm_hour << 11) | (t.tm_min << 5) | (t.tm_sec // 2)
    return data, hora


def entrada_curta(curto: bytes, atributos: int, cluster: int, tamanho: int, caixa: int, quando: tuple) -> bytes:
    data, hora = quando
    e = bytearray(32)
    e[0:11] = curto
    e[11] = atributos
    e[12] = caixa
    struct.pack_into("<HHHH", e, 14, hora, data, data, cluster >> 16)
    struct.pack_into("<HHHI", e, 22, hora, data, cluster & 0xFFFF, tamanho)
    return bytes(e)


# ---------------------------------------------------------------- criacao

def geometria(setores_volume: int) -> tuple:
    """(setores_por_fat, clusters) pela formula do fatgen103."""
    tmp1 = setores_volume - RESERVADOS
    tmp2 = (256 * SETORES_POR_CLUSTER + FATS) // 2
    por_fat = (tmp1 + tmp2 - 1) // tmp2
    clusters = (setores_volume - RESERVADOS - FATS * por_fat) // SETORES_POR_CLUSTER
    if clusters < 65525:
        sys.exit("disco pequeno demais para FAT32")
    return por_fat, clusters


class Volume:
    def __init__(self, setores_volume: int):
        self.setores_volume = setores_volume
        self.por_fat, self.clusters = geometria(setores_volume)
        self.bytes_cluster = SETOR * SETORES_POR_CLUSTER
        self.fat = [0] * (self.clusters + 2)
        self.fat[0] = 0x0FFFFFF8
        self.fat[1] = FIM_CADEIA
        self.dados = {}
        self.proximo = 2
        self.quando = data_hora_fat(time.time())

    def alocar(self, quantos: int) -> list:
        if self.proximo + quantos > self.clusters + 2:
            sys.exit("o conteudo nao cabe no disco")
        cadeia = list(range(self.proximo, self.proximo + quantos))
        self.proximo += quantos
        for atual, seguinte in zip(cadeia, cadeia[1:] + [FIM_CADEIA]):
            self.fat[atual] = seguinte
        return cadeia

    def gravar(self, cadeia: list, conteudo: bytes) -> None:
        for i, cluster in enumerate(cadeia):
            pedaco = conteudo[i * self.bytes_cluster:(i + 1) * self.bytes_cluster]
            self.dados[cluster] = pedaco.ljust(self.bytes_cluster, b"\0")

    def clusters_para(self, tamanho: int) -> int:
        return (tamanho + self.bytes_cluster - 1) // self.bytes_cluster

    def criar_pasta(self, filhos: dict, cadeia: list, pai: int, raiz: bool) -> None:
        """Grava a pasta em `cadeia` (ja alocada) e cria os filhos."""
        entradas = []
        if raiz:
            entradas.append(entrada_curta(nome_83(ROTULO, ""), ATTR_ROTULO, 0, 0, 0, self.quando))
        else:
            entradas.append(entrada_curta(b".          ", ATTR_PASTA, cadeia[0], 0, 0, self.quando))
            entradas.append(entrada_curta(b"..         ", ATTR_PASTA, pai, 0, 0, self.quando))
        curtos = set()
        for nome, valor in filhos.items():
            if precisa_lfn(nome) is None:
                curto, caixa, lfn = nome_83(*partes_83(nome)), bits_de_caixa(nome), []
            else:
                curto, caixa = gerar_curto(nome, curtos), 0
                lfn = entradas_lfn(nome, curto)
            curtos.add(curto)
            if isinstance(valor, dict):
                sub = self.alocar(self.clusters_para(32 * tamanho_pasta(valor)))
                # ".." aponta para 0 quando o pai e a raiz
                self.criar_pasta(valor, sub, 0 if raiz else cadeia[0], False)
                curta = entrada_curta(curto, ATTR_PASTA, sub[0], 0, caixa, self.quando)
            else:
                arq = self.alocar(self.clusters_para(len(valor)))
                self.gravar(arq, valor)
                curta = entrada_curta(curto, ATTR_ARQUIVO, arq[0] if arq else 0, len(valor), caixa, self.quando)
            entradas += lfn + [curta]
        self.gravar(cadeia, b"".join(entradas))

    def setor_boot(self, ocultos: int) -> bytes:
        b = bytearray(SETOR)
        b[0:3] = b"\xEB\x58\x90"
        b[3:11] = b"MXOS    "
        struct.pack_into("<HBHBHHBHHHII", b, 11, SETOR, SETORES_POR_CLUSTER, RESERVADOS, FATS,
                         0, 0, 0xF8, 0, 63, 255, ocultos, self.setores_volume)
        struct.pack_into("<IHHIHH", b, 36, self.por_fat, 0, 0, 2, SETOR_FSINFO, SETOR_COPIA_BOOT)
        b[64] = 0x80  # numero do drive
        b[66] = 0x29  # assinatura estendida
        struct.pack_into("<I", b, 67, int(time.time()) & 0xFFFFFFFF)
        b[71:82] = nome_83(ROTULO, "")
        b[82:90] = b"FAT32   "
        # codigo de boot: so imprime nada e para (int 18h); nao e bootavel
        b[90:92] = b"\xCD\x18"
        b[510:512] = b"\x55\xAA"
        return bytes(b)

    def fsinfo(self) -> bytes:
        f = bytearray(SETOR)
        struct.pack_into("<I", f, 0, 0x41615252)
        struct.pack_into("<III", f, 484, 0x61417272, self.clusters + 2 - self.proximo, self.proximo)
        struct.pack_into("<I", f, 508, 0xAA550000)
        return bytes(f)

    def imagem(self, ocultos: int) -> bytearray:
        img = bytearray(self.setores_volume * SETOR)
        boot, info = self.setor_boot(ocultos), self.fsinfo()
        for base in (0, SETOR_COPIA_BOOT):
            img[base * SETOR:(base + 1) * SETOR] = boot
            img[(base + 1) * SETOR:(base + 2) * SETOR] = info
            # terceiro setor de boot do FAT32: so a assinatura
            img[(base + 2) * SETOR + 510:(base + 3) * SETOR] = b"\x55\xAA"
        fat = struct.pack(f"<{len(self.fat)}I", *self.fat)
        for i in range(FATS):
            inicio = (RESERVADOS + i * self.por_fat) * SETOR
            img[inicio:inicio + len(fat)] = fat
        inicio_dados = (RESERVADOS + FATS * self.por_fat) * SETOR
        for cluster, pedaco in self.dados.items():
            pos = inicio_dados + (cluster - 2) * self.bytes_cluster
            img[pos:pos + len(pedaco)] = pedaco
        return img


def tamanho_pasta(filhos: dict) -> int:
    """Entradas de 32 bytes da pasta: . e .. (ou o rotulo) + filhos + uma folga."""
    return 2 + sum(quantas_entradas(nome) for nome in filhos) + 1


def mbr(setores_disco: int, inicio: int, setores_particao: int) -> bytes:
    m = bytearray(SETOR)
    struct.pack_into("<I", m, 440, 0x4D584F53)  # assinatura do disco "SOXM"
    entrada = bytearray(16)
    entrada[0] = 0x00
    entrada[1:4] = b"\xFE\xFF\xFF"  # CHS fora de alcance: use o LBA
    entrada[4] = 0x0C  # FAT32 com LBA
    entrada[5:8] = b"\xFE\xFF\xFF"
    struct.pack_into("<II", entrada, 8, inicio, setores_particao)
    m[446:462] = entrada
    m[510:512] = b"\x55\xAA"
    return bytes(m)


def criar(destino: pathlib.Path, tamanho: int, com_mbr: bool, arvore: dict) -> None:
    setores_disco = tamanho // SETOR
    inicio = INICIO_PARTICAO if com_mbr else 0
    volume = Volume(setores_disco - inicio)
    raiz = volume.alocar(volume.clusters_para(32 * tamanho_pasta(arvore)))
    assert raiz[0] == 2
    volume.criar_pasta(arvore, raiz, 0, True)
    disco = bytearray(inicio * SETOR)
    if com_mbr:
        disco[0:SETOR] = mbr(setores_disco, inicio, setores_disco - inicio)
    disco += volume.imagem(inicio)
    destino.parent.mkdir(parents=True, exist_ok=True)
    destino.write_bytes(disco)
    usados = volume.proximo - 2
    print(f"ok: {destino} ({tamanho // (1024 * 1024)} MB, FAT32 {'com MBR' if com_mbr else 'sem MBR'}, "
          f"{volume.clusters} clusters de {volume.bytes_cluster} bytes, {usados} usados)")


# ---------------------------------------------------------------- verificacao

class Verificador:
    def __init__(self, img: bytes):
        self.img = img
        self.erros = []
        self.itens = []
        self.inicio = self.achar_volume()
        self.ler_bpb()

    def erro(self, texto: str) -> None:
        self.erros.append(texto)

    def setor(self, lba: int) -> bytes:
        return self.img[lba * SETOR:(lba + 1) * SETOR]

    @staticmethod
    def bpb_valido(s: bytes) -> bool:
        if len(s) < SETOR or s[510:512] != b"\x55\xAA" or s[0] not in (0xEB, 0xE9):
            return False
        bps, spc, res, nfats = struct.unpack_from("<HBHB", s, 11)
        fat16, = struct.unpack_from("<H", s, 22)
        fat32, = struct.unpack_from("<I", s, 36)
        return bps == SETOR and spc in (1, 2, 4, 8, 16, 32, 64, 128) and res > 0 and \
            nfats in (1, 2) and fat16 == 0 and fat32 > 0

    def achar_volume(self) -> int:
        s0 = self.setor(0)
        if self.bpb_valido(s0):
            print("disco sem MBR (FAT32 no disco inteiro)", file=sys.stderr)
            return 0
        if s0[510:512] != b"\x55\xAA":
            sys.exit("ERRO: o setor 0 nao tem a assinatura 55AA")
        for i in range(4):
            e = s0[446 + 16 * i:462 + 16 * i]
            tipo = e[4]
            inicio, quantos = struct.unpack_from("<II", e, 8)
            if tipo in (0x0B, 0x0C) and quantos:
                print(f"MBR: particao {i + 1} tipo {tipo:#04x} no setor {inicio} ({quantos} setores)", file=sys.stderr)
                return inicio
        sys.exit("ERRO: nenhuma particao FAT32 no MBR")

    def ler_bpb(self) -> None:
        b = self.setor(self.inicio)
        if not self.bpb_valido(b):
            sys.exit("ERRO: setor de boot FAT32 invalido")
        self.spc = b[13]
        self.reservados, self.nfats = struct.unpack_from("<HB", b, 14)
        tot16, = struct.unpack_from("<H", b, 19)
        tot32, = struct.unpack_from("<I", b, 32)
        self.total = tot32 or tot16
        self.por_fat, flags, _, self.raiz, self.setor_fsinfo = struct.unpack_from("<IHHIH", b, 36)
        self.espelhada = not (flags & 0x80)
        self.fat_ativa = flags & 0x0F
        self.bytes_cluster = self.spc * SETOR
        self.inicio_dados = self.inicio + self.reservados + self.nfats * self.por_fat
        self.clusters = (self.total - self.reservados - self.nfats * self.por_fat) // self.spc
        self.rotulo_bpb = b[71:82].decode("latin-1").rstrip()
        if self.clusters < 65525:
            self.erro(f"so {self.clusters} clusters: pelo tamanho seria FAT12/16")
        if self.por_fat * SETOR // 4 < self.clusters + 2:
            self.erro("a FAT e pequena demais para os clusters")
        self.ler_fats()

    def ler_fats(self) -> None:
        copias = []
        for i in range(self.nfats):
            inicio = (self.inicio + self.reservados + i * self.por_fat) * SETOR
            bruto = self.img[inicio:inicio + 4 * (self.clusters + 2)]
            copias.append([v & 0x0FFFFFFF for v in struct.unpack(f"<{self.clusters + 2}I", bruto)])
        if self.espelhada:
            for i in range(1, self.nfats):
                if copias[i] != copias[0]:
                    diferentes = sum(1 for a, b in zip(copias[0], copias[i]) if a != b)
                    self.erro(f"FAT {i + 1} diferente da FAT 1 ({diferentes} entradas)")
            self.fat = copias[0]
        else:
            self.fat = copias[self.fat_ativa]
        self.dono = [None] * (self.clusters + 2)

    def cluster(self, c: int) -> bytes:
        pos = (self.inicio_dados + (c - 2) * self.spc) * SETOR
        return self.img[pos:pos + self.bytes_cluster]

    def cadeia(self, primeiro: int, quem: str) -> list:
        cadeia = []
        c = primeiro
        while True:
            if not 2 <= c < self.clusters + 2:
                self.erro(f"{quem}: cluster invalido {c:#x} na cadeia")
                break
            if self.dono[c] is not None:
                self.erro(f"{quem}: cluster {c} ja pertence a {self.dono[c]}")
                break
            self.dono[c] = quem
            cadeia.append(c)
            proximo = self.fat[c]
            if proximo >= 0x0FFFFFF8:
                break
            if proximo == 0:
                self.erro(f"{quem}: a cadeia aponta para um cluster livre")
                break
            c = proximo
        return cadeia

    def ler_pasta(self, primeiro: int, caminho: str, raiz: bool) -> None:
        dados = b"".join(self.cluster(c) for c in self.cadeia(primeiro, caminho))
        lfn, soma, esperado = {}, None, 0
        curtos, longos, posicao = set(), set(), 0
        for pos in range(0, len(dados), 32):
            e = dados[pos:pos + 32]
            if e[0] == 0:
                break
            posicao += 1
            if e[0] == 0xE5:
                lfn, esperado = {}, 0
                continue
            atributos = e[11]
            if atributos & 0x3F == ATTR_LFN:
                ordem = e[0] & 0x3F
                if e[0] & 0x40:
                    if esperado:
                        self.erro(f"{caminho}: LFN interrompido antes de {ordem}")
                    lfn, soma, esperado = {}, e[13], ordem
                elif ordem != esperado - 1 or e[13] != soma:
                    self.erro(f"{caminho}: LFN fora de ordem (entrada {posicao})")
                    lfn, esperado = {}, 0
                    continue
                else:
                    esperado = ordem
                unidades = struct.unpack_from("<5H", e, 1) + struct.unpack_from("<6H", e, 14) + \
                    struct.unpack_from("<2H", e, 28)
                lfn[ordem] = unidades
                continue
            curto = e[0:11]
            nome_lfn = None
            if lfn:
                if esperado != 1:
                    self.erro(f"{caminho}: LFN incompleto antes de {curto!r}")
                elif checksum_curto(curto) != soma:
                    self.erro(f"{caminho}: checksum LFN errado para {curto!r}")
                else:
                    unidades = sum((lfn[k] for k in sorted(lfn)), ())
                    fim = unidades.index(0) if 0 in unidades else len(unidades)
                    bruto = struct.pack(f"<{fim}H", *unidades[:fim])
                    nome_lfn = bruto.decode("utf-16-le", errors="replace")
                lfn, esperado = {}, 0
            if atributos & ATTR_ROTULO:
                if raiz:
                    self.rotulo = curto.decode("latin-1").rstrip()
                continue
            nome = nome_lfn or self.nome_curto(e)
            cluster = struct.unpack_from("<H", e, 20)[0] << 16 | struct.unpack_from("<H", e, 26)[0]
            tamanho, = struct.unpack_from("<I", e, 28)
            if curto in (b".          ", b"..         "):
                self.conferir_ponto(curto, cluster, primeiro, caminho, raiz)
                continue
            if curto in curtos:
                self.erro(f"{caminho}: nome curto repetido {curto!r}")
            curtos.add(curto)
            if nome.lower() in longos:
                self.erro(f"{caminho}: nome repetido {nome}")
            longos.add(nome.lower())
            filho = caminho.rstrip("/") + "/" + nome
            if atributos & ATTR_PASTA:
                self.itens.append(("DIR", filho, 0))
                if cluster < 2:
                    self.erro(f"{filho}: pasta sem cluster")
                    continue
                self.pais[cluster] = 0 if raiz else primeiro
                self.ler_pasta(cluster, filho, False)
            else:
                self.conferir_arquivo(filho, cluster, tamanho)

    def conferir_ponto(self, curto: bytes, cluster: int, primeiro: int, caminho: str, raiz: bool) -> None:
        if raiz:
            self.erro(f"a raiz tem a entrada {curto!r}")
        elif curto.startswith(b". ") and cluster != primeiro:
            self.erro(f"{caminho}: '.' aponta para {cluster}, devia ser {primeiro}")
        elif curto.startswith(b"..") and cluster != self.pais.get(primeiro):
            self.erro(f"{caminho}: '..' aponta para {cluster}, devia ser {self.pais.get(primeiro)}")

    def conferir_arquivo(self, caminho: str, cluster: int, tamanho: int) -> None:
        self.itens.append(("ARQ", caminho, tamanho))
        if tamanho == 0:
            if cluster != 0:
                self.erro(f"{caminho}: vazio mas com cluster {cluster}")
            return
        cadeia = self.cadeia(cluster, caminho)
        precisa = (tamanho + self.bytes_cluster - 1) // self.bytes_cluster
        if len(cadeia) != precisa:
            self.erro(f"{caminho}: {tamanho} bytes pedem {precisa} clusters, a cadeia tem {len(cadeia)}")
        self.cadeias[caminho] = (cadeia, tamanho)

    @staticmethod
    def nome_curto(e: bytes) -> str:
        base = e[0:8].decode("latin-1").rstrip()
        ext = e[8:11].decode("latin-1").rstrip()
        if base.startswith("\x05"):
            base = "\xE5" + base[1:]
        if e[12] & 0x08:
            base = base.lower()
        if e[12] & 0x10:
            ext = ext.lower()
        return base + ("." + ext if ext else "")

    def conferir_livres(self) -> int:
        livres = 0
        for c in range(2, self.clusters + 2):
            valor = self.fat[c]
            if valor == 0:
                livres += 1
            elif self.dono[c] is None and valor != RUIM:
                self.erro(f"cluster {c} ocupado na FAT mas sem dono (perdido)")
        info = self.setor(self.inicio + self.setor_fsinfo)
        assinaturas = struct.unpack_from("<I", info, 0)[0], struct.unpack_from("<I", info, 484)[0], \
            struct.unpack_from("<I", info, 508)[0]
        if assinaturas != (0x41615252, 0x61417272, 0xAA550000):
            self.erro("FSInfo com assinatura errada")
        else:
            contagem, = struct.unpack_from("<I", info, 488)
            if contagem != 0xFFFFFFFF and contagem != livres:
                self.erro(f"FSInfo diz {contagem} clusters livres, a FAT tem {livres}")
        return livres

    def preparar(self) -> None:
        self.rotulo = self.rotulo_bpb
        self.pais = {}
        self.cadeias = {}

    def verificar(self) -> int:
        self.preparar()
        if self.fat[0] & 0xFF != 0xF8 and self.fat[0] & 0xFF != 0xF0:
            self.erro(f"FAT[0] estranho: {self.fat[0]:#x}")
        self.ler_pasta(self.raiz, "/", True)
        livres = self.conferir_livres()
        print(f"Volume \"{self.rotulo}\": FAT32, {self.clusters} clusters de {self.bytes_cluster} bytes, "
              f"{livres} livres ({livres * self.bytes_cluster // 1024} KB)")
        for tipo, caminho, tamanho in self.itens:
            print(f"{tipo} {caminho}" + (f" {tamanho}" if tipo == "ARQ" else ""))
        if self.erros:
            for e in self.erros:
                print(f"ERRO: {e}")
            return 1
        print("OK")
        return 0

    def ler(self, caminho: str) -> bytes:
        alvo = caminho.rstrip("/").lower()
        for nome, (cadeia, tamanho) in self.cadeias.items():
            if nome.lower() == alvo:
                return b"".join(self.cluster(c) for c in cadeia)[:tamanho]
        for tipo, nome, _ in self.itens:
            if tipo == "ARQ" and nome.lower() == alvo:
                return b""
        sys.exit(f"ERRO: arquivo nao encontrado: {caminho}")


def main() -> int:
    parser = argparse.ArgumentParser(description="Cria ou confere o disco FAT32 do MXOS.")
    parser.add_argument("imagem", nargs="?", help="imagem a criar")
    parser.add_argument("--verificar", metavar="IMG", help="confere e lista uma imagem")
    parser.add_argument("--ler", metavar="CAMINHO", help="com --verificar: mostra um arquivo")
    parser.add_argument("--sem-mbr", action="store_true", help="FAT32 no disco inteiro (superfloppy)")
    parser.add_argument("--tamanho-mb", type=int, default=TAMANHO_PADRAO // (1024 * 1024))
    opcoes = parser.parse_args()
    # nomes com acento no console do Windows: o que nao couber vira "?"
    sys.stdout.reconfigure(errors="replace")

    if opcoes.verificar:
        v = Verificador(pathlib.Path(opcoes.verificar).read_bytes())
        if opcoes.ler:
            v.preparar()
            v.ler_pasta(v.raiz, "/", True)
            sys.stdout.buffer.write(v.ler(opcoes.ler))
            return 0
        return v.verificar()
    if not opcoes.imagem:
        parser.error("diga a imagem a criar ou use --verificar IMG")
    criar(pathlib.Path(opcoes.imagem), opcoes.tamanho_mb * 1024 * 1024, not opcoes.sem_mbr, conteudo_padrao())
    return 0


if __name__ == "__main__":
    sys.exit(main())
