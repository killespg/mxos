"""Gera o guia de instalacao da TV box em PDF (precisa do fpdf2: pip install fpdf2).

Uso: python ferramentas/tutorial_pdf.py SAIDA.pdf
"""

import sys
from fpdf import FPDF

FONTES = "C:/dev/mxos/ferramentas/fontes/"
saida = sys.argv[1]

pdf = FPDF(format="A4")
pdf.set_margins(18, 16, 18)
pdf.set_auto_page_break(True, 16)
pdf.add_font("Inter", "", FONTES + "Inter-Regular.ttf")
pdf.add_font("Inter", "B", FONTES + "Inter-Bold.ttf")
pdf.add_font("Mono", "", FONTES + "JetBrainsMono-Regular.ttf")
AZUL = (30, 64, 140)


def titulo(texto):
    pdf.ln(3)
    pdf.set_font("Inter", "B", 14)
    pdf.set_text_color(*AZUL)
    pdf.multi_cell(0, 8, texto)
    pdf.set_text_color(0, 0, 0)
    pdf.ln(1)


def texto(t):
    pdf.set_font("Inter", "", 10.5)
    pdf.multi_cell(0, 5.6, t)
    pdf.ln(1.5)


def passos(lista):
    pdf.set_font("Inter", "", 10.5)
    for k, p in enumerate(lista, 1):
        pdf.set_x(22)
        pdf.multi_cell(0, 5.6, f"{k}. {p}")
        pdf.ln(0.8)
    pdf.ln(1)


def itens(lista):
    pdf.set_font("Inter", "", 10.5)
    for p in lista:
        pdf.set_x(22)
        pdf.multi_cell(0, 5.6, f"•  {p}")
        pdf.ln(0.5)
    pdf.ln(1)


def codigo(t):
    pdf.set_font("Mono", "", 10)
    pdf.set_fill_color(235, 238, 245)
    pdf.set_x(22)
    pdf.multi_cell(168, 6, t, fill=True)
    pdf.ln(2)


def aviso(t):
    pdf.set_font("Inter", "B", 10.5)
    pdf.set_fill_color(255, 243, 205)
    pdf.multi_cell(0, 5.8, t, fill=True)
    pdf.ln(2)


pdf.add_page()
pdf.set_font("Inter", "B", 22)
pdf.set_text_color(*AZUL)
pdf.cell(0, 12, "MXOS na TV box", new_x="LMARGIN", new_y="NEXT")
pdf.set_font("Inter", "", 12)
pdf.set_text_color(90, 90, 90)
pdf.cell(0, 7, "Guia de instalação — MXQ PRO 4K (Amlogic S905W)", new_x="LMARGIN", new_y="NEXT")
pdf.set_text_color(0, 0, 0)
pdf.ln(4)

aviso("Atenção: o MXOS nunca foi testado nesta TV box. Na primeira vez pode dar tela preta ou travar. "
      "Ligar pelo pendrive não apaga o Android: sem o pendrive a box liga o Android como sempre.")

titulo("1. O que tem nesta pasta")
itens([
    "MXOS-pendrive.img — a imagem para gravar no pendrive (512 MB).",
    "MXOS-pendrive-arquivos.zip — os mesmos arquivos soltos (outro jeito de montar o pendrive).",
    "Este guia.",
])

titulo("2. O que você precisa")
itens([
    "Um pendrive de 1 GB ou mais. Tudo o que estiver nele será apagado.",
    "Teclado e mouse USB (um receptor sem fio com os dois serve). Hub USB ainda não funciona.",
    "Cabo HDMI na TV e, se quiser internet, cabo de rede. O Wi-Fi da box ainda não funciona.",
    "Um palito (para o botão de reset escondido no buraco do AV).",
])

titulo("3. Gravar o pendrive")
texto("Jeito recomendado: balenaEtcher (grátis, etcher.balena.io).")
passos([
    "Abra o balenaEtcher e clique em \"Flash from file\". Escolha MXOS-pendrive.img.",
    "Clique em \"Select target\" e escolha o pendrive (confira o tamanho para não apagar o disco errado).",
    "Clique em \"Flash!\" e espere terminar a gravação e a verificação.",
    "Se o Windows perguntar se quer formatar o pendrive, clique em Cancelar.",
])
texto("Com o Rufus (rufus.ie): escolha o pendrive, em \"Seleção de boot\" escolha MXOS-pendrive.img e clique em "
      "Iniciar. Se ele perguntar o modo de gravação, escolha \"DD\".")
texto("Sem programa nenhum: formate o pendrive em FAT32 (botão direito no pendrive > Formatar > FAT32), abra "
      "o MXOS-pendrive-arquivos.zip e copie tudo o que tem dentro para a raiz do pendrive.")

titulo("4. Ligar a box pelo pendrive (primeira vez)")
passos([
    "Com a box desligada da tomada, plugue o pendrive, o teclado/mouse, o HDMI e o cabo de rede.",
    "Enfie o palito no buraco do AV (atrás da box) até sentir o clique do botão, e segure.",
    "Segurando o botão, ligue a box na tomada. Solte depois de uns 5 a 10 segundos.",
    "A box roda o script do pendrive, grava a ordem de boot nova e reinicia sozinha.",
    "Depois disso aparece o MXOS. Nas próximas vezes não precisa mais do palito: com o pendrive plugado "
    "ela liga o MXOS; sem ele, o Android.",
])

titulo("5. Se não funcionar")
itens([
    "Tela preta ou travada: tire da tomada, tire o pendrive e ligue: o Android volta normal.",
    "Para saber onde parou é preciso um cabo serial USB-TTL de 3,3 V (uns 15 reais) nos pinos TX, RX e GND "
    "da placa, a 115200 baud. Com ele dá para ver as mensagens do MXOS.",
    "O botão não fez nada: em algumas boxes o botão fica dentro do conector de áudio ou num furinho na base.",
])

titulo("6. Usando o MXOS")
itens([
    "Menu MXOS (canto de baixo, à esquerda): todos os programas.",
    "NetSurf: o navegador completo (com HTTPS). O \"Navegador\" é o leve, feito para o MXOS.",
    "Terminal: comandos do MXOS (digite help) e programas de Linux (executar /bin/busybox sh).",
    "Ctrl+C e Ctrl+V copiam e colam entre os programas.",
])

titulo("7. Atualizar o sistema")
texto("No MXOS abra \"Atualizar o sistema\" no menu e clique em \"Procurar atualização\". Ele mostra a versão "
      "instalada, a data da última busca e, se tiver versão nova, o número dela, o tamanho e o que mudou. "
      "Nada é baixado antes de você clicar em \"Instalar e reiniciar\".")
texto("Na mesma rede de casa: no PC, na pasta do projeto, rode o comando abaixo e deixe a janela aberta. A box "
      "acha o PC sozinha.")
codigo("python rodar.py --servir")
texto("De qualquer lugar (pela internet): publique a versão nova no GitHub. A box acha quando não tem PC na rede.")
codigo("python rodar.py --publicar")

titulo("8. Instalar na memória interna (opcional — apaga o Android)")
aviso("Só faça isso depois que o MXOS estiver funcionando bem pelo pendrive. O Android e os dados dele "
      "serão apagados. Para ter o Android de volta é preciso o firmware de fábrica da box.")
texto("Ligado pelo pendrive, abra o Terminal e digite:")
codigo("instalar-emmc APAGAR-ANDROID")
itens([
    "Antes de apagar, ele guarda no pendrive (pasta mxos-backup) a parte de boot do Android e o começo da memória.",
    "O U-Boot (o programa que liga a box) não é tocado: a box continua ligando.",
    "Com o pendrive plugado a box sempre liga por ele primeiro: serve de socorro.",
    "Para voltar a parte de boot do Android: restaurar-boot",
])

pdf.output(saida)
print("ok", saida)
