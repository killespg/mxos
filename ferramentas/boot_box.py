"""Arquivos para ligar o MXOS na TV box so com um pendrive (sem cabo
serial), do jeito que o Armbian e o CoreELEC fazem nas boxes Amlogic.

  aml_autoscript   o U-Boot da Amlogic roda este script quando a box liga
                   com o botao de reset apertado (o botao fica escondido
                   dentro do buraco do AV: aperte com um palito). Ele grava
                   no U-Boot: "ao ligar, procure s905_autoscript no
                   pendrive ou no cartao; se nao achar, ligue o Android".
  s905_autoscript  carrega mxos-mxq.img e mxos.dtb do pendrive (ou do
                   cartao SD) e chama o kernel com booti
  mxos.dtb         device tree minima (o booti exige uma; o MXOS nao usa)

Os scripts sao do formato do mkimage (cabecalho legado de 64 bytes, tipo
"script"), gerados aqui sem o mkimage.

Uso: python -I ferramentas/boot_box.py PASTA
"""

import pathlib
import struct
import sys
import time
import zlib

ENDERECO_KERNEL = 0x1080000
ENDERECO_DTB = 0x1000000

AML_AUTOSCRIPT = """\
setenv start_autoscript 'if usb start; then run start_usb_autoscript; fi; if mmcinfo; then run start_mmc_autoscript; fi;'
setenv start_usb_autoscript 'for usbdev in 0 1 2 3; do if fatload usb ${usbdev} 1020000 s905_autoscript; then autoscr 1020000; fi; done;'
setenv start_mmc_autoscript 'if fatload mmc 0 1020000 s905_autoscript; then autoscr 1020000; fi;'
setenv bootcmd 'run start_autoscript; run storeboot'
saveenv
sleep 1
reboot
"""

S905_AUTOSCRIPT = f"""\
for usbdev in 0 1 2 3; do if fatload usb ${{usbdev}} {ENDERECO_KERNEL:#x} mxos-mxq.img; then fatload usb ${{usbdev}} {ENDERECO_DTB:#x} mxos.dtb; booti {ENDERECO_KERNEL:#x} - {ENDERECO_DTB:#x}; fi; done
if fatload mmc 0 {ENDERECO_KERNEL:#x} mxos-mxq.img; then fatload mmc 0 {ENDERECO_DTB:#x} mxos.dtb; booti {ENDERECO_KERNEL:#x} - {ENDERECO_DTB:#x}; fi
"""


def script_mkimage(texto: str, nome: str) -> bytes:
    """Imagem legada do U-Boot do tipo script (mkimage -T script)."""
    corpo = texto.encode()
    corpo += b"\0" * (-len(corpo) % 4)
    dados = struct.pack(">II", len(corpo), 0) + corpo
    def cabecalho(crc_cabecalho: int) -> bytes:
        return struct.pack(">IIIIIIIBBBB32s", 0x27051956, crc_cabecalho, int(time.time()), len(dados),
                           0, 0, zlib.crc32(dados), 5, 22, 6, 0, nome.encode()[:32])
    return cabecalho(zlib.crc32(cabecalho(0))) + dados


def dtb_minima() -> bytes:
    """Device tree com a raiz (compatible, model) e /chosen vazio."""
    textos = b""
    def texto(nome: str) -> int:
        nonlocal textos
        pos = textos.find(nome.encode() + b"\0")
        if pos < 0:
            pos = len(textos)
            textos += nome.encode() + b"\0"
        return pos
    def prop(nome: str, valor: bytes) -> bytes:
        valor_pad = valor + b"\0" * (-len(valor) % 4)
        return struct.pack(">III", 3, len(valor), texto(nome)) + valor_pad
    def no(nome: str) -> bytes:
        n = nome.encode() + b"\0"
        return struct.pack(">I", 1) + n + b"\0" * (-len(n) % 4)
    estrutura = no("")
    estrutura += prop("#address-cells", struct.pack(">I", 2))
    estrutura += prop("#size-cells", struct.pack(">I", 2))
    estrutura += prop("compatible", b"amlogic,p281\0amlogic,s905w\0amlogic,meson-gxl\0")
    estrutura += prop("model", b"MXQ PRO 4K (MXOS)\0")
    estrutura += no("chosen") + struct.pack(">I", 2)
    estrutura += no("memory@0")
    estrutura += prop("device_type", b"memory\0")
    estrutura += prop("reg", struct.pack(">QQ", 0, 0x40000000))
    estrutura += struct.pack(">I", 2)
    estrutura += struct.pack(">II", 2, 9)  # fim da raiz e da arvore
    reservas = b"\0" * 16
    inicio_reservas = 40
    inicio_estrutura = inicio_reservas + len(reservas)
    inicio_textos = inicio_estrutura + len(estrutura)
    # espaco livre no fim: o booti acrescenta /chosen e /memory
    total = inicio_textos + len(textos) + 4096
    cabecalho = struct.pack(">IIIIIIIIII", 0xD00DFEED, total, inicio_estrutura, inicio_textos, inicio_reservas,
                            17, 16, 0, len(textos), len(estrutura))
    blob = cabecalho + reservas + estrutura + textos
    return blob + b"\0" * (total - len(blob))


def main() -> int:
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    pasta = pathlib.Path(sys.argv[1])
    pasta.mkdir(parents=True, exist_ok=True)
    (pasta / "aml_autoscript").write_bytes(script_mkimage(AML_AUTOSCRIPT, "aml_autoscript"))
    (pasta / "s905_autoscript").write_bytes(script_mkimage(S905_AUTOSCRIPT, "s905_autoscript"))
    (pasta / "mxos.dtb").write_bytes(dtb_minima())
    print(f"ok: aml_autoscript, s905_autoscript e mxos.dtb em {pasta}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
