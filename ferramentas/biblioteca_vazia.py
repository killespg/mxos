"""Gera uma biblioteca .so de mentira (ELF64 aarch64) so com funcoes que
devolvem um numero fixo, sem compilador C.

Para que: um programa do Debian pode pedir uma biblioteca que ele so usa
num caminho que o MXOS nunca toma. O netsurf-fb, por exemplo, e ligado ao
SDL 1.2 (BIND_NOW: todas as funcoes tem de existir ao carregar), mas aqui
desenha pelo X11; e o SDL 1.2 do Debian aborta ao abrir se nao achar o
SDL2 (que puxaria o Mesa e o LLVM). A biblioteca vazia tem os mesmos
nomes, e o SDL_Init devolve -1 ("nao deu"), que o programa ja trata.

Uso (o pacotes.py chama sozinho para as que estao em VAZIAS):
  python -I ferramentas/biblioteca_vazia.py saida.so libSDL-1.2.so.0 SDL_Init=-1 SDL_Quit ...

O arquivo: cabecalho ELF, um segmento de leitura e execucao (simbolos,
nomes, tabela de hash SysV e o codigo, 2 instrucoes por funcao) e um de
escrita com o .dynamic (o ld.so do glibc ajusta os enderecos dele no
lugar). Sem relocacoes e sem cabecalhos de secao.
"""

import struct
import sys

PAGINA = 0x1000
DT_NULL, DT_HASH, DT_STRTAB, DT_SYMTAB, DT_STRSZ, DT_SYMENT, DT_SONAME = 0, 4, 5, 6, 10, 11, 14


def hash_elf(nome: bytes) -> int:
    h = 0
    for c in nome:
        h = (h << 4) + c
        g = h & 0xF0000000
        if g:
            h ^= g >> 24
        h &= ~g & 0xFFFFFFFF
    return h


def mov_x0(valor: int) -> int:
    """movz x0, #v ou movn x0, #~v (valores pequenos)."""
    if valor >= 0:
        return 0xD2800000 | (valor & 0xFFFF) << 5
    return 0x92800000 | ((~valor) & 0xFFFF) << 5


def gerar(soname: str, funcoes: dict) -> bytes:
    nomes = list(funcoes)
    # nomes: "\0soname\0f1\0f2..."
    dynstr = b"\0" + soname.encode() + b"\0"
    deslocamentos = {}
    for n in nomes:
        deslocamentos[n] = len(dynstr)
        dynstr += n.encode() + b"\0"

    cabecalhos = 64 + 3 * 56
    pos_dynsym = (cabecalhos + 7) & ~7
    pos_dynstr = pos_dynsym + 24 * (len(nomes) + 1)
    pos_hash = (pos_dynstr + len(dynstr) + 3) & ~3
    nbucket = max(1, len(nomes))
    tamanho_hash = 4 * (2 + nbucket + len(nomes) + 1)
    pos_codigo = (pos_hash + tamanho_hash + 15) & ~15
    fim_texto = pos_codigo + 8 * len(nomes)

    # tabela de hash SysV
    baldes = [0] * nbucket
    cadeias = [0] * (len(nomes) + 1)
    for k, n in enumerate(nomes, start=1):
        b = hash_elf(n.encode()) % nbucket
        cadeias[k] = baldes[b]
        baldes[b] = k
    tabela_hash = struct.pack(f"<II{nbucket}I{len(cadeias)}I", nbucket, len(cadeias), *baldes, *cadeias)

    dynsym = bytes(24)
    codigo = b""
    for k, n in enumerate(nomes):
        endereco = pos_codigo + 8 * k
        # STB_GLOBAL | STT_FUNC, visibilidade padrao, secao "qualquer" != 0
        dynsym += struct.pack("<IBBHQQ", deslocamentos[n], 0x12, 0, 7, endereco, 8)
        codigo += struct.pack("<II", mov_x0(funcoes[n]), 0xD65F03C0)  # ret

    dinamico = b"".join(struct.pack("<qQ", tag, valor) for tag, valor in [
        (DT_SONAME, 1), (DT_HASH, pos_hash), (DT_STRTAB, pos_dynstr), (DT_SYMTAB, pos_dynsym),
        (DT_STRSZ, len(dynstr)), (DT_SYMENT, 24), (DT_NULL, 0)])
    pos_dinamico = (fim_texto + PAGINA - 1) & ~(PAGINA - 1)

    elf = bytearray(pos_dinamico + len(dinamico))
    # e_ident, ET_DYN, EM_AARCH64 (183)
    struct.pack_into("<4sBBBBB7xHHIQQQIHHHHHH", elf, 0, b"\x7fELF", 2, 1, 1, 0, 0,
                     3, 183, 1, 0, 64, 0, 0, 64, 56, 3, 64, 0, 0)
    fases = [
        (1, 5, 0, 0, fim_texto, fim_texto, PAGINA),                               # PT_LOAD R+X
        (1, 6, pos_dinamico, pos_dinamico, len(dinamico), len(dinamico), PAGINA),  # PT_LOAD R+W
        (2, 6, pos_dinamico, pos_dinamico, len(dinamico), len(dinamico), 8),       # PT_DYNAMIC
    ]
    for k, (tipo, flags, offset, vaddr, filesz, memsz, alinhamento) in enumerate(fases):
        struct.pack_into("<IIQQQQQQ", elf, 64 + 56 * k, tipo, flags, offset, vaddr, vaddr, filesz, memsz, alinhamento)
    elf[pos_dynsym:pos_dynsym + len(dynsym)] = dynsym
    elf[pos_dynstr:pos_dynstr + len(dynstr)] = dynstr
    elf[pos_hash:pos_hash + len(tabela_hash)] = tabela_hash
    elf[pos_codigo:pos_codigo + len(codigo)] = codigo
    elf[pos_dinamico:] = dinamico
    return bytes(elf)


def main() -> int:
    if len(sys.argv) < 4:
        sys.exit(__doc__)
    funcoes = {}
    for f in sys.argv[3:]:
        nome, _, valor = f.partition("=")
        funcoes[nome] = int(valor or "0")
    open(sys.argv[1], "wb").write(gerar(sys.argv[2], funcoes))
    return 0


if __name__ == "__main__":
    sys.exit(main())
