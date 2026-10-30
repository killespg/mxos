"""Cliente da depuracao remota do MXOS (src/depuracao.rs, porta 4546).

    python -I ferramentas/depurar_box.py procurar 192.168.3
    python -I ferramentas/depurar_box.py IP INFO
    python -I ferramentas/depurar_box.py IP SH cpu
    python -I ferramentas/depurar_box.py IP TELA tela.png
    python -I ferramentas/depurar_box.py IP ABRIR http://example.com
    python -I ferramentas/depurar_box.py IP ANDROID

A senha e a do build (MXOS_DEPURACAO): vem da variavel de ambiente ou do
arquivo indicado por MXOS_DEPURACAO_ARQUIVO.
"""

import os
import socket
import struct
import sys
import zlib
from concurrent.futures import ThreadPoolExecutor

PORTA = 4546


def senha():
    s = os.environ.get("MXOS_DEPURACAO")
    if s:
        return s.strip()
    arquivo = os.environ.get("MXOS_DEPURACAO_ARQUIVO", "C:/dev/mxos-tmp/token_depuracao.txt")
    return open(arquivo).read().strip()


def ler_ate_ponto(s, buf=b""):
    while not buf.endswith(b"\n.\n") and buf != b".\n":
        d = s.recv(65536)
        if not d:
            break
        buf += d
    return buf


def conectar(ip, tempo=10):
    s = socket.create_connection((ip, PORTA), timeout=tempo)
    s.sendall(senha().encode() + b"\n")
    r = ler_ate_ponto(s)
    if not r.startswith(b"MXOS depuracao"):
        raise SystemExit("senha recusada")
    return s


def png(caminho, l, a, xrgb):
    raw = bytearray()
    for y in range(a):
        raw.append(0)
        linha = xrgb[y * l * 4:(y + 1) * l * 4]
        for x in range(0, l * 4, 4):
            raw += bytes((linha[x + 2], linha[x + 1], linha[x]))

    def ch(t, b):
        return struct.pack(">I", len(b)) + t + b + struct.pack(">I", zlib.crc32(t + b))

    d = b"\x89PNG\r\n\x1a\n" + ch(b"IHDR", struct.pack(">IIBBBBB", l, a, 8, 2, 0, 0, 0)) + ch(b"IDAT", zlib.compress(bytes(raw), 6)) + ch(b"IEND", b"")
    open(caminho, "wb").write(d)


def tela(ip, caminho, tempo=120):
    s = conectar(ip, tempo)
    s.sendall(b"TELA\n")
    cab = b""
    while b"\n" not in cab:
        cab += s.recv(1)
    partes = cab.split()
    if partes[0] != b"OK":
        raise SystemExit(cab.decode(errors="replace"))
    l, a = int(partes[1]), int(partes[2])
    falta = l * a * 4
    dados = bytearray()
    while len(dados) < falta:
        d = s.recv(min(1 << 20, falta - len(dados)))
        if not d:
            break
        dados += d
    s.close()
    png(caminho, l, a, bytes(dados))
    print(f"{caminho}: {l}x{a}")


def procurar(prefixo):
    def tenta(i):
        ip = f"{prefixo}.{i}"
        try:
            socket.create_connection((ip, PORTA), timeout=1).close()
            return ip
        except OSError:
            return None

    with ThreadPoolExecutor(64) as ex:
        for ip in ex.map(tenta, range(1, 255)):
            if ip:
                print(ip)


def main():
    a = sys.argv[1:]
    if len(a) >= 2 and a[0] == "procurar":
        procurar(a[1])
    elif len(a) >= 3 and a[1] == "TELA":
        tela(a[0], a[2])
    elif len(a) >= 2:
        s = conectar(a[0], 300)
        s.sendall((" ".join(a[1:]) + "\n").encode())
        r = ler_ate_ponto(s)
        s.close()
        # bytes direto: o console do Windows (cp1252) nao tem setas e afins
        sys.stdout.buffer.write(r.removesuffix(b".\n"))
    else:
        print(__doc__)
        sys.exit(1)


main()
