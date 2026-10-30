"""Agrupa as amostras de um perfil por linha de codigo (DWARF via objdump -l).

    python -I linhas.py EXE target/saidas/perfil_X.txt [N]
"""

import bisect
import collections
import re
import subprocess
import sys

OBJDUMP = r"C:\Users\VenâncioKilles\.rustup\toolchains\stable-x86_64-pc-windows-msvc\lib\rustlib\x86_64-pc-windows-msvc\bin\llvm-objdump.exe"


def main():
    exe, arquivo = sys.argv[1], sys.argv[2]
    n = int(sys.argv[3]) if len(sys.argv) > 3 else 40
    cab = subprocess.run([OBJDUMP, "-p", exe], capture_output=True, text=True).stdout
    base = int(re.search(r"ImageBase\s+([0-9a-fA-F]+)", cab).group(1), 16)
    rvas = collections.Counter(int(l, 16) for l in open(arquivo))
    total = sum(rvas.values())
    quentes = sorted(rvas)
    lo, hi = base + quentes[0], base + quentes[-1] + 16
    # so o trecho com amostras (o executavel inteiro com linhas e enorme)
    alvo = [r for r in quentes if r < 0x10000000]
    lo, hi = base + min(alvo), base + max(alvo) + 16
    dis = subprocess.run([OBJDUMP, "-d", "-l", "--no-show-raw-insn", f"--start-address={lo:#x}", f"--stop-address={hi:#x}", exe], capture_output=True, text=True).stdout
    linha_atual = "?"
    enderecos, linhas = [], []
    for l in dis.splitlines():
        m = re.match(r"^; (.+):(\d+)", l)
        if m:
            linha_atual = f"{m.group(1).split(chr(92))[-1].split('/')[-1]}:{m.group(2)}"
            continue
        m = re.match(r"^\s*([0-9a-f]+):", l)
        if m:
            enderecos.append(int(m.group(1), 16))
            linhas.append(linha_atual)
    por_linha = collections.Counter()
    for rva, c in rvas.items():
        va = base + rva
        i = bisect.bisect_right(enderecos, va) - 1
        por_linha[linhas[i] if 0 <= i < len(linhas) and va - enderecos[i] < 64 else "(fora)"] += c
    for nome, c in por_linha.most_common(n):
        print(f"{100.0 * c / total:5.1f}%  {nome}")


main()
