"""MXOS para a TV box Rockchip RK322x.

1) Converte o kernel (ELF do alvo armv7a, feature rk322x) em binario cru
   com cabecalho de zImage:
     python -I ferramentas/gerar_box_rk.py binario SAIDA.bin

2) Troca o kernel dentro da imagem de recovery ORIGINAL da box (tirada com
   adb), mantendo cabecalho, enderecos, ramdisk e o "second" (resource com
   o device tree) que o U-Boot da Rockchip espera:
     python -I ferramentas/gerar_box_rk.py recovery recovery-original.img mxos.bin recovery-mxos.img

A box liga o MXOS quando entra em recovery (botao de reset apertado ao
ligar, ou "adb reboot recovery"); o Android normal continua igual.
"""

import hashlib
import pathlib
import struct
import sys

ELF_PADRAO = pathlib.Path(__file__).parent.parent / "target" / "rk322x" / "armv7a-none-eabihf" / "release" / "mxos"


def binario(elf_caminho, saida):
    d = pathlib.Path(elf_caminho).read_bytes()
    if d[:4] != b"\x7fELF" or d[4] != 1:
        raise SystemExit("não é um ELF de 32 bits")
    phoff, = struct.unpack_from("<I", d, 0x1C)
    phnum, = struct.unpack_from("<H", d, 0x2C)
    segmentos = []
    for i in range(phnum):
        tipo, off, va, pa, fs, ms, fl, al = struct.unpack_from("<8I", d, phoff + 32 * i)
        if tipo == 1 and fs > 0:
            segmentos.append((pa, d[off:off + fs]))
    segmentos.sort()
    base = segmentos[0][0]
    fim = max(pa + len(b) for pa, b in segmentos)
    img = bytearray(fim - base)
    for pa, b in segmentos:
        img[pa - base:pa - base + len(b)] = b
    magia, comeco, tamanho = struct.unpack_from("<III", img, 0x24)
    if magia != 0x016F2818:
        raise SystemExit("o kernel não tem o cabeçalho de zImage (boot_arm.s)")
    if tamanho > len(img):
        # o boot_arm.s copia ate o __bss_start (alinhado): completa com zeros
        img += bytes(tamanho - len(img))
    elif tamanho != len(img):
        raise SystemExit(f"cabeçalho diz {tamanho} bytes, o binário tem {len(img)}")
    pathlib.Path(saida).write_bytes(img)
    print(f"{saida}: {len(img)} bytes, carregado em {base:#x}")


def alinhar(n, pagina):
    return (n + pagina - 1) // pagina * pagina


def recovery(original, kernel, saida):
    d = pathlib.Path(original).read_bytes()
    if d[:8] != b"ANDROID!":
        raise SystemExit("a imagem não começa com ANDROID! (não é uma imagem de boot do Android)")
    (k_tam, k_end, r_tam, r_end, s_tam, s_end, tags, pagina, versao, os_ver) = struct.unpack_from("<10I", d, 8)
    if pagina not in (2048, 4096, 16384):
        raise SystemExit(f"tamanho de página estranho: {pagina}")
    nome = d[48:64].split(b"\0")[0].decode("latin-1")
    print(f"original: kernel {k_tam} bytes em {k_end:#x}, ramdisk {r_tam}, second {s_tam}, página {pagina}, versão {versao}, nome '{nome}'")
    pos = pagina
    partes = {}
    for nome_parte, tam in (("kernel", k_tam), ("ramdisk", r_tam), ("second", s_tam)):
        partes[nome_parte] = d[pos:pos + tam]
        pos += alinhar(tam, pagina)
    extras = []
    if versao in (1, 2):
        dtbo_tam, = struct.unpack_from("<I", d, 1632)
        extras.append(("dtbo", d[pos:pos + dtbo_tam]))
        pos += alinhar(dtbo_tam, pagina)
    if versao == 2:
        dtb_tam, = struct.unpack_from("<I", d, 1648)
        extras.append(("dtb", d[pos:pos + dtb_tam]))
        pos += alinhar(dtb_tam, pagina)
    novo_kernel = pathlib.Path(kernel).read_bytes()
    cab = bytearray(d[:pagina])
    struct.pack_into("<I", cab, 8, len(novo_kernel))
    # id = SHA1 de cada parte seguida do tamanho dela
    h = hashlib.sha1()
    for b in (novo_kernel, partes["ramdisk"], partes["second"]) + tuple(b for _, b in extras):
        h.update(b)
        h.update(struct.pack("<I", len(b)))
    cab[576:608] = h.digest().ljust(32, b"\0")
    out = bytearray(cab)
    for b in (novo_kernel, partes["ramdisk"], partes["second"]) + tuple(b for _, b in extras):
        out += b + b"\0" * (alinhar(len(b), pagina) - len(b))
    if len(out) > len(d) and len(d) % (1 << 20) == 0:
        print(f"aviso: a imagem nova ({len(out)} bytes) é maior que a partição lida ({len(d)} bytes)")
    pathlib.Path(saida).write_bytes(out)
    print(f"{saida}: {len(out)} bytes (kernel do MXOS com {len(novo_kernel)} bytes)")


def main():
    if len(sys.argv) >= 2 and sys.argv[1] == "binario":
        binario(sys.argv[3] if len(sys.argv) > 3 else ELF_PADRAO, sys.argv[2])
    elif len(sys.argv) == 5 and sys.argv[1] == "recovery":
        recovery(sys.argv[2], sys.argv[3], sys.argv[4])
    else:
        print(__doc__)
        sys.exit(1)


main()
