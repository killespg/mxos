"""Gera src/cripto/raizes.der: as autoridades certificadoras confiaveis (a
lista da Mozilla, do pacote ca-certificates do Debian) em DER, um
certificado atras do outro. O kernel le direto (x509.rs), sem PEM.

    python ferramentas/gerar_raizes.py [arquivo.crt]

Sem argumento usa saida/pendrive/etc/ssl/certs/ca-certificates.crt (criado
pelo rodar.py ao montar o pendrive).
"""

import base64
import pathlib
import sys

RAIZ = pathlib.Path(__file__).resolve().parent.parent
PADRAO = RAIZ / "saida/pendrive/etc/ssl/certs/ca-certificates.crt"
DESTINO = RAIZ / "src/cripto/raizes.der"


def main() -> None:
    origem = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else PADRAO
    texto = origem.read_text(encoding="ascii", errors="replace")
    certs = []
    dentro = None
    for linha in texto.splitlines():
        linha = linha.strip()
        if linha == "-----BEGIN CERTIFICATE-----":
            dentro = []
        elif linha == "-----END CERTIFICATE-----" and dentro is not None:
            certs.append(base64.b64decode("".join(dentro)))
            dentro = None
        elif dentro is not None:
            dentro.append(linha)
    vistos = set()
    saida = bytearray()
    for c in certs:
        if c in vistos or c[0] != 0x30:
            continue
        vistos.add(c)
        saida += c
    DESTINO.write_bytes(bytes(saida))
    print(f"{len(vistos)} autoridades, {len(saida)} bytes em {DESTINO}")


if __name__ == "__main__":
    main()
