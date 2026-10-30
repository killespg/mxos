"""Gera src/midia/h264_cabac.rs: os valores m e n da inicializacao do CABAC
do H.264 (tabelas 9-12 a 9-33 da norma ITU-T H.264), lidos das tabelas do
FFmpeg (libavcodec/h264_cabac.c, LGPL-2.1-or-later), no formato que o
RKVDEC le: [cabac_init_idc 0..2 dos quadros P/B e o I][ctxIdx 0..463][m, n].

Uso: python -I ferramentas/gerar_cabac.py <h264_cabac.c do FFmpeg> [saida.rs]
"""

import pathlib
import re
import sys

CONTEXTOS = 464
# ctxIdx 460..463 ja sao do 4:4:4 (High 4:4:4, que o RKVDEC nao decodifica):
# zerados, como o driver do Linux manda para o hardware.
SO_444 = range(460, 464)


def tabela(texto: str, nome: str) -> list[tuple[int, int]]:
    inicio = texto.index(nome)
    corpo = texto[texto.index("=", inicio) + 1:texto.index("};", inicio)]
    pares = [(int(m), int(n)) for m, n in re.findall(r"\{\s*(-?\d+)\s*,\s*(-?\d+)\s*\}", corpo)]
    return pares


def main() -> None:
    texto = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8")
    i = tabela(texto, "cabac_context_init_I[1024][2]")
    pb = tabela(texto, "cabac_context_init_PB[3][1024][2]")
    assert len(i) == 1024 and len(pb) == 3 * 1024, (len(i), len(pb))
    grupos = [pb[k * 1024:k * 1024 + CONTEXTOS] for k in range(3)] + [i[:CONTEXTOS]]
    grupos = [[(0, 0) if c in SO_444 else par for c, par in enumerate(g)] for g in grupos]
    linhas = [
        "//! Valores m e n da inicializacao do CABAC do H.264 (tabelas 9-12 a",
        "//! 9-33 da norma ITU-T H.264), no formato que o RKVDEC le da RAM:",
        "//! [cabac_init_idc 0..2 dos quadros P/B e o I][ctxIdx 0..463][m, n].",
        "//!",
        "//! Gerado por ferramentas/gerar_cabac.py a partir das tabelas do FFmpeg",
        "//! (libavcodec/h264_cabac.c, LGPL-2.1-or-later); os ctxIdx 460..463",
        "//! (so do 4:4:4) vao zerados. Nao editar a mao.",
        "",
        f"pub const TABELA: [[[i8; 2]; {CONTEXTOS}]; 4] = [",
    ]
    for grupo in grupos:
        linhas.append("    [")
        for k in range(0, CONTEXTOS, 8):
            linhas.append("        " + " ".join(f"[{m}, {n}]," for m, n in grupo[k:k + 8]))
        linhas.append("    ],")
    linhas.append("];")
    saida = "\n".join(linhas) + "\n"
    if len(sys.argv) > 2:
        pathlib.Path(sys.argv[2]).write_text(saida, encoding="utf-8")
    else:
        sys.stdout.write(saida)


if __name__ == "__main__":
    main()
