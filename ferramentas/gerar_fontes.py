"""Pre-renderiza as fontes do MXOS (Inter e JetBrains Mono) em arquivos .mxf.

O kernel nao le TrueType: este script desenha cada caractere no PC, com
antialiasing (FreeType, pelo Pillow), e grava so os pixels prontos. Assim
o MXOS desenha texto suave sem um rasterizador de curvas.

Uso:  python -I ferramentas/gerar_fontes.py
Saida: src/fonte/*.mxf

Formato .mxf (little endian):
  "MXF1"
  u8 altura_linha, u8 ascendente, u8 descendente, u8 capitular (altura do 'H')
  u16 quantidade, u16 reservado
  quantidade entradas de 16 bytes, ordenadas pelo codigo:
    u32 codigo, u32 deslocamento (nos dados de alfa),
    u8 largura, u8 altura, i8 dx, i8 dy (topo do bitmap em relacao a linha de base),
    u8 avanco, 3 bytes reservados
  dados de alfa: 1 byte por pixel (0 = transparente, 255 = cheio), linha a linha
"""

import pathlib
import struct
import sys

from fontTools.ttLib import TTFont
from PIL import Image, ImageDraw, ImageFont

RAIZ = pathlib.Path(__file__).resolve().parent.parent
PASTA_TTF = RAIZ / "ferramentas" / "fontes"
PASTA_SAIDA = RAIZ / "src" / "fonte"

# nome do arquivo, ttf, tamanho em pixels, avanco fixo (so monoespacada)
FONTES = [
    ("inter_13", "Inter-Regular.ttf", 13, None),
    ("inter_13_negrito", "Inter-SemiBold.ttf", 13, None),
    ("inter_20", "Inter-SemiBold.ttf", 20, None),
    ("inter_32", "Inter-Bold.ttf", 32, None),
    # 13.34 px: o avanco da JetBrains Mono da exatamente 8 px (grade 8x16 do terminal)
    ("mono_16", "JetBrainsMono-Regular.ttf", 13.34, 8),
]

# ASCII, Latin-1 (todos os acentos do portugues) e alguns simbolos uteis
CARACTERES = (
    list(range(0x20, 0x7F))
    + list(range(0xA0, 0x100))
    + [0x2013, 0x2014, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2026, 0x20AC,
       0x2190, 0x2191, 0x2192, 0x2193, 0x2713]
)


def codigos_presentes(ttf: pathlib.Path) -> set[int]:
    """Caracteres que a fonte realmente desenha (o resto vira '?' no kernel)."""
    return set(TTFont(ttf)["cmap"].getBestCmap())


def renderizar(fonte: ImageFont.FreeTypeFont, codigo: int, avanco_fixo: int | None):
    """Devolve (largura, altura, dx, dy, avanco, pixels) de um caractere."""
    c = chr(codigo)
    avanco = avanco_fixo if avanco_fixo else round(fonte.getlength(c))
    esquerda, topo, direita, baixo = fonte.getbbox(c, anchor="ls")
    largura, altura = max(direita - esquerda, 0), max(baixo - topo, 0)
    if largura == 0 or altura == 0:
        return 0, 0, 0, 0, avanco, b""
    imagem = Image.new("L", (largura, altura), 0)
    ImageDraw.Draw(imagem).text((-esquerda, -topo), c, font=fonte, fill=255, anchor="ls")
    return largura, altura, esquerda, topo, avanco, imagem.tobytes()


def gerar(nome: str, arquivo: str, tamanho: float, avanco_fixo: int | None) -> int:
    ttf = PASTA_TTF / arquivo
    fonte = ImageFont.truetype(str(ttf), tamanho)
    presentes = codigos_presentes(ttf)
    ascendente, descendente = fonte.getmetrics()
    capitular = -fonte.getbbox("H", anchor="ls")[1]

    entradas, alfa = [], bytearray()
    for codigo in CARACTERES:
        if codigo not in presentes:
            continue
        largura, altura, dx, dy, avanco, pixels = renderizar(fonte, codigo, avanco_fixo)
        if not (0 <= largura < 256 and 0 <= altura < 256 and -128 <= dx < 128 and -128 <= dy < 128):
            sys.exit(f"{nome}: caractere U+{codigo:04X} grande demais")
        entradas.append(struct.pack("<IIBBbbB3x", codigo, len(alfa), largura, altura, dx, dy, avanco))
        alfa += pixels

    cabecalho = b"MXF1" + struct.pack("<BBBBHH", ascendente + descendente, ascendente, descendente,
                                      capitular, len(entradas), 0)
    dados = cabecalho + b"".join(entradas) + alfa
    (PASTA_SAIDA / f"{nome}.mxf").write_bytes(dados)
    print(f"{nome}.mxf: {arquivo} {tamanho}px, {len(entradas)} caracteres, {len(dados) // 1024} KB")
    return len(dados)


def main() -> None:
    PASTA_SAIDA.mkdir(parents=True, exist_ok=True)
    total = sum(gerar(*fonte) for fonte in FONTES)
    print(f"total: {total // 1024} KB")


if __name__ == "__main__":
    main()
