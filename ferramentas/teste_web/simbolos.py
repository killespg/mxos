"""Da nome aos RVAs de um perfil (src/perfil.rs).

    python -I simbolos.py EXE target/saidas/perfil_X.txt [N]
"""

import bisect
import collections
import pathlib
import re
import subprocess
import sys

# o llvm-objdump do rustup (rustup component add llvm-tools)
OBJDUMP = next(
    pathlib.Path.home().glob(".rustup/toolchains/*/lib/rustlib/*/bin/llvm-objdump*"),
    "llvm-objdump",
)


def main():
    exe, arquivo = sys.argv[1], sys.argv[2]
    n = int(sys.argv[3]) if len(sys.argv) > 3 else 30
    cab = subprocess.run([OBJDUMP, "-p", exe], capture_output=True, text=True).stdout
    base = int(re.search(r"ImageBase\s+([0-9a-fA-F]+)", cab).group(1), 16)
    sim = subprocess.run([OBJDUMP, "-t", exe], capture_output=True, text=True).stdout
    secoes = {}
    for linha in subprocess.run([OBJDUMP, "-h", exe], capture_output=True, text=True).stdout.splitlines():
        m = re.match(r"\s*(\d+)\s+\S+\s+[0-9a-f]+\s+([0-9a-f]+)", linha)
        if m:
            secoes[int(m.group(1)) + 1] = int(m.group(2), 16)
    funcs = []
    for linha in sim.splitlines():
        m = re.search(r"\(sec\s+(\d+)\).*\(ty\s+20\).*0x([0-9a-f]+)\s+(\S+)$", linha)
        if m and int(m.group(1)) in secoes:
            funcs.append((secoes[int(m.group(1))] + int(m.group(2), 16), m.group(3)))
    if not funcs:
        for linha in sim.splitlines():
            m = re.match(r"([0-9a-f]{8,16})\s.*\.text\s+[0-9a-f]+\s+(\S+)$", linha)
            if m:
                funcs.append((int(m.group(1), 16), m.group(2)))
    funcs.sort()
    enderecos = [f[0] for f in funcs]
    contagem = collections.Counter()
    total = 0
    for linha in open(arquivo):
        rva = int(linha, 16)
        va = base + rva
        i = bisect.bisect_right(enderecos, va) - 1
        if i < 0:
            # COFF: os valores ja podem ser RVA
            i = bisect.bisect_right(enderecos, rva) - 1
        nome = funcs[i][1] if i >= 0 else "?"
        contagem[nome] += 1
        total += 1
    for nome, c in contagem.most_common(n):
        curto = re.sub(r"17h[0-9a-f]{16}E$", "", nome)
        print(f"{100.0 * c / total:5.1f}%  {curto[:150]}")


main()
