"""Qual arquivo deu panico na box: o kernel grava no GRF_OS_REG1 0xDEADxxxx
(xxxx = linha) e no GRF_OS_REG2 o FNV-1a de 32 bits do caminho do arquivo.

    adb shell busybox devmem 0x110005d4 32      (o hash)
    python -I ferramentas/arquivo_do_panico.py <hash hexa> [linha]

Compila no Windows: o caminho vem com \\; os dois jeitos sao testados.
"""

import os
import sys


def fnv(s):
    h = 0x811C9DC5
    for b in s.encode():
        h = ((h ^ b) * 0x01000193) & 0xFFFFFFFF
    return h


def main():
    alvo = int(sys.argv[1], 16)
    linha = int(sys.argv[2], 0) if len(sys.argv) > 2 else None
    raiz = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
    for pasta, _, arquivos in os.walk(os.path.join(raiz, "src")):
        for a in arquivos:
            if not a.endswith(".rs"):
                continue
            rel = os.path.relpath(os.path.join(pasta, a), raiz)
            for c in (rel.replace("/", "\\"), rel.replace("\\", "/")):
                if fnv(c) == alvo:
                    print(c)
                    if linha:
                        with open(os.path.join(raiz, rel), encoding="utf-8") as f:
                            linhas = f.readlines()
                        print(f"{linha}: {linhas[linha - 1].rstrip()}" if linha <= len(linhas) else "(linha alem do fim)")
                    return
    print("nenhum arquivo de src/ com esse hash (dependencia externa?)")


main()
