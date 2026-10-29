"""Tira um arquivo de dentro de um pacote .deb do Debian, so com Python.

Uso:
  python -I ferramentas/extrair_deb.py pacote.deb ./bin/busybox destino

O .deb e um arquivo "ar" com um data.tar.xz (ou .gz, .zst nao) dentro; o
tarfile do Python le xz e gz. Usado para pegar o BusyBox estatico do
Debian (pacote busybox-static, arquitetura arm64):

  http://deb.debian.org/debian/pool/main/b/busybox/busybox-static_1.37.0-6+b9_arm64.deb
"""

import io
import pathlib
import sys
import tarfile


def membros_ar(dados: bytes):
    """(nome, conteudo) de cada membro de um arquivo ar."""
    if not dados.startswith(b"!<arch>\n"):
        sys.exit("nao e um .deb (falta o cabecalho do ar)")
    i = 8
    while i + 60 <= len(dados):
        cabecalho = dados[i:i + 60]
        nome = cabecalho[:16].decode().strip().rstrip("/")
        tamanho = int(cabecalho[48:58].decode().strip())
        yield nome, dados[i + 60:i + 60 + tamanho]
        i += 60 + tamanho + (tamanho & 1)


def main() -> int:
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    deb, caminho, destino = pathlib.Path(sys.argv[1]), sys.argv[2], pathlib.Path(sys.argv[3])
    for nome, conteudo in membros_ar(deb.read_bytes()):
        if not nome.startswith("data.tar"):
            continue
        with tarfile.open(fileobj=io.BytesIO(conteudo)) as tar:
            membro = tar.getmember(caminho)
            arquivo = tar.extractfile(membro)
            if arquivo is None:
                sys.exit(f"{caminho} nao e um arquivo comum")
            destino.write_bytes(arquivo.read())
            print(f"ok: {destino} ({destino.stat().st_size} bytes)")
            return 0
    sys.exit("o .deb nao tem data.tar")


if __name__ == "__main__":
    sys.exit(main())
