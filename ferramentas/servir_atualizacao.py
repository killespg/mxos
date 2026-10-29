"""Serve uma atualizacao do MXOS pela rede de casa, para a box baixar com
o comando "atualizar IP:PORTA" (src/atualizacao.rs).

O PC se anuncia na rede a cada 2 s (UDP 8001, broadcast, "MXOS-ATUALIZACAO
<porta>"): o app "Atualizar o sistema" da box acha ele sozinho.

Cria PASTA/manifesto.txt (uma linha por arquivo: crc32 em hexa, tamanho e
caminho) e serve PASTA por HTTP. A box compara com o que tem e baixa so o
que mudou.

Uso: python -I ferramentas/servir_atualizacao.py saida/pendrive [PORTA]
(o rodar.py --servir compila, monta a pasta e chama este script)
"""

import functools
import http.server
import pathlib
import socket
import sys
import threading
import time
import zlib

IGNORAR = {"manifesto.txt", "aml_autoscript", "s905_autoscript"}


def manifesto(pasta: pathlib.Path) -> str:
    linhas = []
    for arquivo in sorted(pasta.rglob("*")):
        if not arquivo.is_file() or arquivo.name in IGNORAR:
            continue
        dados = arquivo.read_bytes()
        caminho = "/" + arquivo.relative_to(pasta).as_posix()
        linhas.append(f"{zlib.crc32(dados):08x} {len(dados)} {caminho}\n")
    return "".join(linhas)


def ips_da_casa() -> list:
    ips = set()
    try:
        for info in socket.getaddrinfo(socket.gethostname(), None, socket.AF_INET):
            ips.add(info[4][0])
    except OSError:
        pass
    # o IP que sai para a rede (sem mandar nada)
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
            s.connect(("10.255.255.255", 1))
            ips.add(s.getsockname()[0])
    except OSError:
        pass
    return sorted(ip for ip in ips if not ip.startswith("127."))


def anunciar(porta: int) -> None:
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
        s.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
        while True:
            try:
                s.sendto(f"MXOS-ATUALIZACAO {porta}".encode(), ("255.255.255.255", 8001))
            except OSError:
                pass
            time.sleep(2)


def main() -> int:
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    pasta = pathlib.Path(sys.argv[1]).resolve()
    porta = int(sys.argv[2]) if len(sys.argv) > 2 else 8000
    texto = manifesto(pasta)
    (pasta / "manifesto.txt").write_text(texto)
    print(f"{texto.count(chr(10))} arquivos no manifesto")
    threading.Thread(target=anunciar, args=(porta,), daemon=True).start()
    print('na box: menu iniciar > "Atualizar o sistema" (ela acha este PC sozinha)')
    for ip in ips_da_casa():
        print(f"  ou no terminal do MXOS:  atualizar {ip}:{porta}")
    print("(Ctrl+C para parar)")
    tratador = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(pasta))
    with http.server.ThreadingHTTPServer(("0.0.0.0", porta), tratador) as servidor:
        try:
            servidor.serve_forever()
        except KeyboardInterrupt:
            pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
