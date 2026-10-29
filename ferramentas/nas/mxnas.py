#!/usr/bin/env python3
"""Cliente do NAS do MXOS (protocolo proprio sobre TCP, porta 4545).

Uso:
  python mxnas.py <ip-da-box> listar [pasta]
  python mxnas.py <ip-da-box> baixar <arquivo-na-box> [destino-local]
  python mxnas.py <ip-da-box> enviar <arquivo-ou-pasta-local> [pasta-na-box]
  python mxnas.py <ip-da-box> pasta <pasta-na-box>
  python mxnas.py <ip-da-box> apagar <caminho-na-box>
  python mxnas.py <ip-da-box> renomear <de> <para>

A senha e a mesma do `config nas_senha` da box: vem da variavel MXNAS_SENHA
ou e perguntada. Ela nao passa pela rede (desafio com HMAC-SHA256).
"""

import getpass
import hashlib
import hmac
import os
import socket
import sys
import time

PORTA = 4545


class Nas:
    def __init__(self, ip, senha):
        self.s = socket.create_connection((ip, PORTA), timeout=30)
        self.buf = b""
        linha = self.linha()
        partes = linha.split()
        if len(partes) != 3 or partes[0] != "MXNAS":
            raise SystemExit(f"resposta estranha: {linha}")
        nonce = bytes.fromhex(partes[2])
        assinatura = hmac.new(senha.encode(), nonce, hashlib.sha256).hexdigest()
        self.comando(f"AUTH {assinatura}")

    def ler(self, n):
        while len(self.buf) < n:
            pedaco = self.s.recv(1 << 16)
            if not pedaco:
                raise SystemExit("a conexão caiu")
            self.buf += pedaco
        dados, self.buf = self.buf[:n], self.buf[n:]
        return dados

    def linha(self):
        while b"\n" not in self.buf:
            pedaco = self.s.recv(1 << 16)
            if not pedaco:
                raise SystemExit("a conexão caiu")
            self.buf += pedaco
        l, self.buf = self.buf.split(b"\n", 1)
        return l.decode("utf-8", "replace")

    def comando(self, texto, corpo=b""):
        self.s.sendall(texto.encode() + b"\n" + corpo)
        r = self.linha()
        if r.startswith("ERRO"):
            raise SystemExit(f"box: {r[5:]}")
        return r

    def listar(self, pasta):
        n = int(self.comando(f"LISTAR {pasta}").split()[1])
        itens = []
        for _ in range(n):
            tipo, tamanho, nome = self.linha().split(" ", 2)
            itens.append((tipo == "D", int(tamanho), nome))
        return itens

    def baixar(self, remoto, local):
        tamanho = int(self.comando(f"BAIXAR {remoto}").split()[1])
        inicio = time.time()
        with open(local, "wb") as f:
            falta = tamanho
            while falta:
                pedaco = self.ler(min(falta, 1 << 20))
                f.write(pedaco)
                falta -= len(pedaco)
        return tamanho, time.time() - inicio

    def enviar(self, local, remoto):
        with open(local, "rb") as f:
            dados = f.read()
        inicio = time.time()
        self.comando(f"ENVIAR {len(dados)} {remoto}", dados)
        return len(dados), time.time() - inicio


def velocidade(n, s):
    return f"{n / 1024 / max(s, 1e-3):.0f} KB/s"


def juntar(pasta, nome):
    return pasta.rstrip("/") + "/" + nome


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        sys.exit(1)
    ip, acao, args = sys.argv[1], sys.argv[2], sys.argv[3:]
    senha = os.environ.get("MXNAS_SENHA") or getpass.getpass("senha do NAS: ")
    nas = Nas(ip, senha)
    if acao == "listar":
        for pasta, tamanho, nome in nas.listar(args[0] if args else "/"):
            print(f"{'<pasta>':>12}  {nome}/" if pasta else f"{tamanho:>12}  {nome}")
    elif acao == "baixar":
        destino = args[1] if len(args) > 1 else os.path.basename(args[0])
        n, s = nas.baixar(args[0], destino)
        print(f"{destino}: {n} bytes ({velocidade(n, s)})")
    elif acao == "enviar":
        origem = args[0]
        pasta = args[1] if len(args) > 1 else "/"
        if os.path.isdir(origem):
            base = juntar(pasta, os.path.basename(os.path.abspath(origem)))
            for raiz, dirs, arquivos in os.walk(origem):
                rel = os.path.relpath(raiz, origem).replace("\\", "/")
                destino = base if rel == "." else juntar(base, rel)
                try:
                    nas.comando(f"PASTA {destino}")
                except SystemExit:
                    pass  # ja existe
                for a in arquivos:
                    n, s = nas.enviar(os.path.join(raiz, a), juntar(destino, a))
                    print(f"{juntar(destino, a)}: {n} bytes ({velocidade(n, s)})")
        else:
            destino = juntar(pasta, os.path.basename(origem))
            n, s = nas.enviar(origem, destino)
            print(f"{destino}: {n} bytes ({velocidade(n, s)})")
    elif acao == "pasta":
        nas.comando(f"PASTA {args[0]}")
    elif acao == "apagar":
        nas.comando(f"APAGAR {args[0]}")
    elif acao == "renomear":
        nas.comando(f"RENOMEAR {args[0]}\t{args[1]}")
    else:
        print(__doc__)
        sys.exit(1)
    nas.comando("TCHAU")


if __name__ == "__main__":
    main()
