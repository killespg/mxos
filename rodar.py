"""Compila o MXOS e roda no QEMU.

Uso:
  python rodar.py                 janela com a area de trabalho do MXOS + som; teclado e
                                  mouse da janela vao para o MXOS, e o shell tambem fica
                                  no terminal pela serial (sair: Ctrl+A e depois X)
  python rodar.py --sem-janela    so a serial, sem tela e sem som
  python rodar.py --el2           simula o boot em EL2, igual ao U-Boot da TV box
  python rodar.py --disco-novo    recria saida/disco.img antes de rodar
  python rodar.py --teste "help;info;desligar"
                                  roda os comandos sozinho e mostra a saida
                                  (o som vai para saida/som.wav)
  python rodar.py --teste "ls;http http://example.com" --espera 8
                                  espera ate 8 s pelo prompt depois de cada comando (padrao 3)
  python rodar.py --teste "video;som" --captura saida/tela.png
                                  idem, e salva a tela no fim
  python rodar.py --teste "" --acoes "mover 40 750;clicar;esperar 300" --captura saida/menu.png
                                  depois dos comandos, mexe no mouse e no teclado pelo QMP
                                  e so entao salva a tela. Acoes (separadas por ';'):
                                    mover X Y                 ponteiro em (X, Y) da tela
                                    clicar [esquerdo|direito|meio]
                                    arrastar X1 Y1 X2 Y2      aperta em 1, solta em 2
                                    apertar [botao] / soltar [botao]   segura o botao
                                                              (captura no meio de um arrasto)
                                    tecla QCODE               ex.: ret, esc, tab, f1, up, ctrl+c
                                    digitar TEXTO             teclado americano, sem ';'; letras
                                                              acentuadas e o c cedilha saem pelas
                                                              teclas mortas do ABNT2
                                    esperar MS
  python rodar.py --porta 8080:80 encaminha http://localhost:8080 do PC para a porta 80
                                  do MXOS (ex.: executar httpd -h /paginas)
  python rodar.py --mxq           gera saida/mxos-mxq.img para a TV box

O disco (saida/disco.img, FAT32 de 64 MB) e criado por ferramentas/criar_disco.py
na primeira vez. A rede e a "user" do QEMU (DHCP 10.0.2.15, saida para a internet).
"""

import argparse
import json
import os
import pathlib
import shutil
import socket
import struct
import subprocess
import sys
import time
import unicodedata
import zlib

RAIZ = pathlib.Path(__file__).resolve().parent
ALVO = "aarch64-unknown-none"
FLAGS = ["-C", "link-arg=-Tlinker.ld", "-C", "target-feature=+strict-align"]
BASE_MXQ = 0x01080000  # onde o U-Boot da Amlogic carrega o kernel
PROMPT = b"mxos> "
DISCO = RAIZ / "saida" / "disco.img"
CRIAR_DISCO = RAIZ / "ferramentas" / "criar_disco.py"
# tamanho da tela do MXOS no QEMU (src/terminal.rs) e faixa do tablet do QEMU
LARGURA_TELA, ALTURA_TELA = 1024, 768
MAXIMO_ABS = 0x7FFF


def achar(programa: str, extras: list[pathlib.Path]) -> str:
    encontrado = shutil.which(programa)
    if encontrado:
        return encontrado
    for caminho in extras:
        if caminho.exists():
            return str(caminho)
    sys.exit(f"nao achei o {programa}. Instale e tente de novo.")


def cargo() -> str:
    return achar("cargo", [pathlib.Path.home() / ".cargo" / "bin" / "cargo.exe"])


def qemu() -> str:
    return achar(
        "qemu-system-aarch64",
        [pathlib.Path(r"C:\Program Files\qemu\qemu-system-aarch64.exe")],
    )


def compilar(mxq: bool) -> pathlib.Path:
    comando = [cargo(), "build", "--release"]
    ambiente = os.environ.copy()
    pasta = RAIZ / "target"
    if mxq:
        pasta = pasta / "mxq"
        comando += ["--features", "mxq", "--target-dir", str(pasta)]
        flags = [f if f != "link-arg=-Tlinker.ld" else "link-arg=-Tlinker-mxq.ld" for f in FLAGS]
        # CARGO_ENCODED_RUSTFLAGS substitui as flags do config.toml; a variavel
        # CARGO_TARGET_..._RUSTFLAGS seria somada a elas (dois linker scripts).
        ambiente["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(flags)
    if subprocess.run(comando, cwd=RAIZ, env=ambiente).returncode != 0:
        sys.exit("falha na compilacao")
    return pasta / ALVO / "release" / "mxos"


# Portas TCP encaminhadas do PC para o MXOS: (porta no PC, porta no MXOS).
PORTAS: list[tuple[int, int]] = []


def argumentos_qemu(kernel: pathlib.Path, el2: bool, audio: str) -> list[str]:
    # GICv2 igual ao GIC-400 da TV box (o QEMU escolheria o v3 com virtualization=on)
    maquina = "virt,gic-version=2,virtualization=on" if el2 else "virt,gic-version=2"
    return [
        qemu(), "-M", maquina, "-cpu", "cortex-a53", "-smp", "4", "-accel", "tcg,thread=multi", "-m", "1G", "-kernel", str(kernel),
        # video: framebuffer simples configurado pelo fw_cfg
        "-device", "ramfb",
        # som: o driver so fala virtio-mmio moderno (versao 2)
        "-global", "virtio-mmio.force-legacy=false",
        "-audiodev", audio,
        "-device", "virtio-sound-device,audiodev=som,streams=1",
        # teclado e mouse (tablet: posicao absoluta, sem prender o mouse na janela)
        "-device", "virtio-keyboard-device",
        "-device", "virtio-tablet-device",
        # disco FAT32 (virtio-blk); virgula no caminho vira ",," para o QEMU
        "-drive", f"file={str(DISCO).replace(',', ',,')},if=none,format=raw,id=disco",
        "-device", "virtio-blk-device,drive=disco",
        # rede "user": DHCP, DNS e saida para a internet sem configurar nada;
        # --porta PC:MXOS deixa o PC entrar num servidor que roda no MXOS
        "-netdev", "user,id=rede" + "".join(f",hostfwd=tcp:127.0.0.1:{pc}-:{mxos}" for pc, mxos in PORTAS),
        "-device", "virtio-net-device,netdev=rede",
    ]


def preparar_disco(novo: bool) -> None:
    """Cria saida/disco.img se nao existir (ou se pediram um novo)."""
    if DISCO.exists() and not novo:
        return
    DISCO.parent.mkdir(exist_ok=True)
    DISCO.unlink(missing_ok=True)
    comando = [sys.executable, "-I", str(CRIAR_DISCO), str(DISCO)]
    raiz = RAIZ / "saida" / "raiz"
    if raiz.is_dir():
        # os pacotes do Debian (ferramentas/pacotes.py) vao junto, num disco maior
        comando += ["--raiz", str(raiz), "--tamanho-mb", "512"]
    if subprocess.run(comando, cwd=RAIZ).returncode != 0 or not DISCO.exists():
        sys.exit("falha ao criar o disco (ferramentas/criar_disco.py)")


def audio_em_arquivo(arquivo: pathlib.Path) -> str:
    arquivo.parent.mkdir(exist_ok=True)
    return f"wav,id=som,path={arquivo},out.frequency=48000,out.channels=2,out.format=s16"


def corrigir_wav(arquivo: pathlib.Path) -> None:
    """O QEMU deixa os tamanhos do cabecalho WAV em 0; preenche para os players abrirem."""
    if not arquivo.exists():
        return
    dados = bytearray(arquivo.read_bytes())
    if len(dados) <= 44 or dados[:4] != b"RIFF" or dados[36:40] != b"data":
        return
    struct.pack_into("<I", dados, 4, len(dados) - 8)
    struct.pack_into("<I", dados, 40, len(dados) - 44)
    arquivo.write_bytes(dados)


def ppm_para_png(ppm: pathlib.Path, png: pathlib.Path) -> tuple[int, int]:
    """Converte a captura do QEMU (P6) em PNG, so com zlib."""
    dados = ppm.read_bytes()
    campos = dados.split(maxsplit=4)
    largura, altura = int(campos[1]), int(campos[2])
    pixels = campos[4][: largura * altura * 3]
    linhas = b"".join(b"\x00" + pixels[y * largura * 3:(y + 1) * largura * 3] for y in range(altura))

    def bloco(tipo: bytes, conteudo: bytes) -> bytes:
        corpo = tipo + conteudo
        return struct.pack(">I", len(conteudo)) + corpo + struct.pack(">I", zlib.crc32(corpo))

    cabecalho = struct.pack(">IIBBBBB", largura, altura, 8, 2, 0, 0, 0)
    png.write_bytes(b"\x89PNG\r\n\x1a\n" + bloco(b"IHDR", cabecalho)
                    + bloco(b"IDAT", zlib.compress(linhas, 9)) + bloco(b"IEND", b""))
    return largura, altura


def capturar_tela(porta: int, png: pathlib.Path) -> None:
    """Pede ao monitor do QEMU uma captura da tela e salva como PNG."""
    ppm = png.with_suffix(".ppm")
    ppm.unlink(missing_ok=True)
    with socket.create_connection(("127.0.0.1", porta)) as monitor:
        ler_ate(monitor, b"(qemu) ", 5)
        # aspas duplas por causa dos espacos no caminho
        monitor.sendall(f'screendump "{ppm.as_posix()}"\n'.encode())
        ler_ate(monitor, b"(qemu) ", 5)
    for _ in range(50):
        if ppm.exists() and ppm.stat().st_size > 0:
            break
        time.sleep(0.1)
    largura, altura = ppm_para_png(ppm, png)
    ppm.unlink()
    print(f"[tela capturada: {png.relative_to(RAIZ)} ({largura}x{altura})]")


class Qmp:
    """Cliente minimo do QMP (o protocolo JSON do QEMU): uma mensagem por linha."""

    def __init__(self, porta: int):
        self.conexao = socket.create_connection(("127.0.0.1", porta), timeout=5)
        self.arquivo = self.conexao.makefile("rb")
        self.resposta()  # saudacao {"QMP": ...}
        self.executar("qmp_capabilities")

    def resposta(self) -> dict:
        while True:
            linha = self.arquivo.readline()
            if not linha:
                raise ConnectionError("o QEMU fechou o QMP")
            mensagem = json.loads(linha)
            if "event" in mensagem:  # eventos assincronos nao sao a resposta
                continue
            if "error" in mensagem:
                raise RuntimeError(mensagem["error"].get("desc", "erro no QMP"))
            return mensagem

    def executar(self, comando: str, **argumentos) -> dict:
        pedido = {"execute": comando}
        if argumentos:
            pedido["arguments"] = argumentos
        self.conexao.sendall(json.dumps(pedido).encode() + b"\r\n")
        return self.resposta()

    def eventos(self, *eventos: dict) -> None:
        self.executar("input-send-event", events=list(eventos))

    def fechar(self) -> None:
        self.arquivo.close()
        self.conexao.close()


def evento_abs(eixo: str, pixel: int, tamanho: int) -> dict:
    """Pixel da tela -> faixa 0..0x7fff do tablet. Arredonda para cima: o kernel
    divide para baixo na volta e cai no mesmo pixel."""
    pixel = max(0, min(pixel, tamanho - 1))
    valor = -(-pixel * MAXIMO_ABS // (tamanho - 1))
    return {"type": "abs", "data": {"axis": eixo, "value": min(valor, MAXIMO_ABS)}}


def evento_botao(botao: str, apertado: bool) -> dict:
    return {"type": "btn", "data": {"down": apertado, "button": botao}}


def evento_tecla(qcode: str, apertada: bool) -> dict:
    return {"type": "key", "data": {"down": apertada, "key": {"type": "qcode", "data": qcode}}}


BOTOES = {"esquerdo": "left", "direito": "right", "meio": "middle"}
# teclado americano: caractere -> qcode (sem shift / com shift)
QCODES_SIMPLES = {
    " ": "spc", "-": "minus", "=": "equal", "[": "bracket_left", "]": "bracket_right",
    "\\": "backslash", "'": "apostrophe", "`": "grave_accent", ",": "comma", ".": "dot",
    "/": "slash", "\n": "ret", "\t": "tab",
}
QCODES_SHIFT = {
    "!": "1", "@": "2", "#": "3", "$": "4", "%": "5", "^": "6", "&": "7", "*": "8",
    "(": "9", ")": "0", "_": "minus", "+": "equal", "{": "bracket_left", "}": "bracket_right",
    "|": "backslash", ":": "semicolon", '"': "apostrophe", "~": "grave_accent",
    "<": "comma", ">": "dot", "?": "slash",
}


# teclas mortas do ABNT2 (src/entrada/teclado.rs): acento combinante -> (qcode, shift)
MORTAS_ABNT2 = {
    "\u0301": ("bracket_left", False),  # agudo
    "\u0300": ("bracket_left", True),   # crase
    "\u0303": ("apostrophe", False),    # til
    "\u0302": ("apostrophe", True),     # circunflexo
    "\u0308": ("6", True),              # trema
}


# simbolos ASCII no ABNT2 (src/entrada/teclado.rs): caractere -> teclas.
# Os que viram tecla morta no ABNT2 saem com a morta e depois o espaco.
SIMBOLOS_ABNT2 = {
    "'": [("grave_accent", False)], '"': [("grave_accent", True)],
    "[": [("bracket_right", False)], "{": [("bracket_right", True)],
    "]": [("backslash", False)], "}": [("backslash", True)],
    ";": [("slash", False)], ":": [("slash", True)],
    "/": [("ro", False)], "?": [("ro", True)],
    "\\": [("less", False)], "|": [("less", True)],
    "~": [("apostrophe", False), ("spc", False)], "^": [("apostrophe", True), ("spc", False)],
    "`": [("bracket_left", True), ("spc", False)],
}
# layout que o MXOS esta usando (--layout); muda so os simbolos
LAYOUT = "abnt2"


def teclas_do_caractere(c: str) -> list[tuple[str, bool]]:
    """Teclas para um caractere no layout do MXOS (ABNT2 por padrao): letras,
    numeros e espaco iguais ao americano; acentuadas e o c cedilha pela tecla
    morta e depois a letra."""
    if LAYOUT == "abnt2" and c in SIMBOLOS_ABNT2:
        return SIMBOLOS_ABNT2[c]
    if c in "çÇ" and LAYOUT == "abnt2":
        return [("semicolon", c.isupper())]
    if c.isascii():
        return [qcode_do_caractere(c)]
    if LAYOUT != "abnt2":
        raise ValueError(f"nao sei digitar {c!r} no layout americano")
    decomposto = unicodedata.normalize("NFD", c)
    if len(decomposto) == 2 and decomposto[0].isascii() and decomposto[1] in MORTAS_ABNT2:
        return [MORTAS_ABNT2[decomposto[1]], qcode_do_caractere(decomposto[0])]
    raise ValueError(f"nao sei digitar {c!r}")


def qcode_do_caractere(c: str) -> tuple[str, bool]:
    """Devolve (qcode, precisa de shift)."""
    if c.isascii() and c.isalpha():
        return c.lower(), c.isupper()
    if c.isascii() and c.isdigit():
        return c, False
    if c in QCODES_SIMPLES:
        return QCODES_SIMPLES[c], False
    if c in QCODES_SHIFT:
        return QCODES_SHIFT[c], True
    raise ValueError(f"nao sei digitar {c!r}")


class Acoes:
    """Executa as acoes do --acoes. O MXOS le teclado e mouse 100 vezes por
    segundo; as pausas curtas deixam cada evento chegar numa leitura propria."""

    PAUSA = 0.04

    def __init__(self, qmp: Qmp):
        self.qmp = qmp

    def mover(self, x: int, y: int) -> None:
        self.qmp.eventos(evento_abs("x", x, LARGURA_TELA), evento_abs("y", y, ALTURA_TELA))
        time.sleep(self.PAUSA)

    def botao(self, botao: str, apertado: bool) -> None:
        self.qmp.eventos(evento_botao(botao, apertado))
        time.sleep(self.PAUSA * 2)

    def clicar(self, nome: str = "esquerdo") -> None:
        if nome not in BOTOES:
            raise ValueError(f"botao {nome!r}: use esquerdo, direito ou meio")
        self.botao(BOTOES[nome], True)
        self.botao(BOTOES[nome], False)

    def arrastar(self, x1: int, y1: int, x2: int, y2: int) -> None:
        self.mover(x1, y1)
        self.botao("left", True)
        passos = 10
        for i in range(1, passos + 1):
            self.mover(x1 + (x2 - x1) * i // passos, y1 + (y2 - y1) * i // passos)
        self.botao("left", False)

    def tecla(self, qcode: str, shift: bool = False) -> None:
        # "ctrl+c", "shift+tab": os modificadores ficam apertados durante a tecla
        *modificadores, qcode = qcode.split("+")
        if shift:
            modificadores.append("shift")
        for m in modificadores:
            self.qmp.eventos(evento_tecla(m, True))
        self.qmp.eventos(evento_tecla(qcode, True))
        time.sleep(self.PAUSA)
        self.qmp.eventos(evento_tecla(qcode, False))
        for m in reversed(modificadores):
            self.qmp.eventos(evento_tecla(m, False))
        time.sleep(self.PAUSA)

    def digitar(self, texto: str) -> None:
        for c in texto:
            for qcode, shift in teclas_do_caractere(c):
                self.tecla(qcode, shift)

    def executar(self, acao: str) -> None:
        nome, _, resto = acao.strip().partition(" ")
        argumentos = resto.split()
        if nome == "mover" and len(argumentos) == 2:
            self.mover(*map(int, argumentos))
        elif nome == "clicar" and len(argumentos) <= 1:
            self.clicar(*argumentos)
        elif nome == "arrastar" and len(argumentos) == 4:
            self.arrastar(*map(int, argumentos))
        elif nome in ("apertar", "soltar") and len(argumentos) <= 1:
            self.botao(BOTOES[argumentos[0] if argumentos else "esquerdo"], nome == "apertar")
        elif nome == "tecla" and len(argumentos) == 1:
            self.tecla(argumentos[0])
        elif nome == "digitar" and resto:
            self.digitar(resto)
        elif nome == "esperar" and len(argumentos) == 1:
            time.sleep(int(argumentos[0]) / 1000)
        else:
            raise ValueError(f"acao invalida: {acao!r}")


def validar_acoes(acoes: list[str]) -> None:
    """Confere a sintaxe antes de compilar e ligar o QEMU (sem mandar nada)."""
    for acao in acoes:
        nome, _, resto = acao.partition(" ")
        argumentos = resto.split()
        quantos = {"mover": [2], "clicar": [0, 1], "apertar": [0, 1], "soltar": [0, 1], "arrastar": [4], "tecla": [1], "esperar": [1]}
        if nome == "digitar":
            for c in resto:
                teclas_do_caractere(c)
            if not resto:
                raise ValueError(f"acao invalida: {acao!r}")
        elif nome not in quantos or len(argumentos) not in quantos[nome]:
            raise ValueError(f"acao invalida: {acao!r}")
        elif nome in ("mover", "arrastar", "esperar"):
            [int(a) for a in argumentos]
        elif nome in ("clicar", "apertar", "soltar") and argumentos and argumentos[0] not in BOTOES:
            raise ValueError(f"botao {argumentos[0]!r}: use esquerdo, direito ou meio")


def executar_acoes(porta: int, acoes: list[str]) -> None:
    qmp = Qmp(porta)
    try:
        executor = Acoes(qmp)
        for acao in acoes:
            print(f"[acao: {acao}]")
            executor.executar(acao)
    finally:
        qmp.fechar()


def ler_ate(conexao: socket.socket, fim: bytes, limite: float) -> tuple[bytes, bool]:
    """Le da serial ate aparecer `fim`. Devolve (dados, conexao_ainda_aberta)."""
    dados = b""
    prazo = time.time() + limite
    conexao.settimeout(0.2)
    while time.time() < prazo and not dados.endswith(fim):
        try:
            pedaco = conexao.recv(4096)
        except socket.timeout:
            continue
        except OSError:
            return dados, False
        if not pedaco:
            return dados, False
        dados += pedaco
    return dados, True


def porta_livre() -> int:
    with socket.socket() as livre:
        livre.bind(("127.0.0.1", 0))
        return livre.getsockname()[1]


def testar(kernel: pathlib.Path, el2: bool, comandos: list[str], captura: pathlib.Path | None,
           espera: float, acoes: list[str]) -> int:
    porta, porta_monitor, porta_qmp = porta_livre(), porta_livre(), porta_livre()
    # com --disco cada QEMU grava o proprio som (dois testes juntos nao brigam pelo arquivo)
    som = RAIZ / "saida" / ("som.wav" if DISCO == RAIZ / "saida" / "disco.img" else f"som_{DISCO.stem}.wav")
    som.unlink(missing_ok=True)

    processo = subprocess.Popen(
        argumentos_qemu(kernel, el2, audio_em_arquivo(som))
        + ["-display", "none",
           "-monitor", f"tcp:127.0.0.1:{porta_monitor},server=on,wait=off",
           "-qmp", f"tcp:127.0.0.1:{porta_qmp},server=on,wait=off",
           "-serial", f"tcp:127.0.0.1:{porta},server=on,wait=on"]
    )
    conexao = None
    for _ in range(50):
        try:
            conexao = socket.create_connection(("127.0.0.1", porta))
            break
        except OSError:
            time.sleep(0.1)
    if conexao is None:
        processo.kill()
        sys.exit("o QEMU nao abriu a serial")

    # o boot procura o disco, pede DHCP e liga a area de trabalho antes do prompt
    saida, aberta = ler_ate(conexao, PROMPT, 15)
    for comando in comandos:
        if not aberta:
            break
        conexao.sendall(comando.encode() + b"\r")
        pedaco, aberta = ler_ate(conexao, PROMPT, espera)
        saida += pedaco
    if acoes and aberta and processo.poll() is None:
        executar_acoes(porta_qmp, acoes)
        # o que o shell da janela escreveu na serial enquanto as acoes rodavam
        pedaco, aberta = ler_ate(conexao, PROMPT, 1.0)
        saida += pedaco
    if captura is not None and processo.poll() is None:
        # a area de trabalho desenha o ultimo quadro depois de responder a serial
        time.sleep(0.8)
        capturar_tela(porta_monitor, captura)
    conexao.close()

    try:
        codigo = processo.wait(timeout=3)
    except subprocess.TimeoutExpired:
        processo.kill()
        processo.wait()
        codigo = None
    corrigir_wav(som)
    # a serial fala UTF-8; um byte quebrado vira U+FFFD em vez de parar tudo
    print(saida.decode("utf-8", errors="replace").replace("\r", ""))
    print(f"\n[QEMU {'continuou rodando' if codigo is None else f'saiu com codigo {codigo}'}]")
    if som.exists() and som.stat().st_size > 44:
        print(f"[som gravado: {som.relative_to(RAIZ)} ({som.stat().st_size} bytes)]")
    return 0


def elf_para_imagem(elf: pathlib.Path, destino: pathlib.Path) -> None:
    """Transforma o ELF em imagem crua (o formato que o booti carrega)."""
    dados = elf.read_bytes()
    if dados[:4] != b"\x7fELF":
        sys.exit("o kernel compilado nao e um ELF")
    inicio_ph = struct.unpack_from("<Q", dados, 0x20)[0]
    tamanho_ph, quantidade_ph = struct.unpack_from("<HH", dados, 0x36)

    segmentos = []
    for i in range(quantidade_ph):
        tipo, _, offset, _, endereco, tamanho, _, _ = struct.unpack_from(
            "<IIQQQQQQ", dados, inicio_ph + i * tamanho_ph)
        if tipo == 1 and tamanho:  # PT_LOAD com conteudo
            segmentos.append((endereco, dados[offset:offset + tamanho]))

    inicio = min(endereco for endereco, _ in segmentos)
    fim = max(endereco + len(conteudo) for endereco, conteudo in segmentos)
    if inicio != BASE_MXQ:
        sys.exit(f"kernel ligado em {inicio:#x}, esperado {BASE_MXQ:#x}")

    imagem = bytearray(fim - inicio)
    for endereco, conteudo in segmentos:
        imagem[endereco - inicio:endereco - inicio + len(conteudo)] = conteudo
    if imagem[0x38:0x3C] != b"ARM\x64":
        sys.exit("imagem sem a assinatura ARM64 do cabecalho")

    destino.parent.mkdir(exist_ok=True)
    destino.write_bytes(imagem)
    print(f"ok: {destino.relative_to(RAIZ)} ({len(imagem)} bytes)")
    print("No prompt do U-Boot (pela serial), com a imagem num cartao SD FAT32:")
    print(f"  fatload mmc 0 {BASE_MXQ:#x} mxos-mxq.img")
    print(f"  booti {BASE_MXQ:#x} - ${{dtb_mem_addr}}")
    print("Atencao: ainda nao testado no hardware.")


def main() -> int:
    parser = argparse.ArgumentParser(description="Compila e roda o MXOS.")
    parser.add_argument("--el2", action="store_true", help="comeca em EL2, como o U-Boot")
    parser.add_argument("--teste", metavar="CMDS", help="comandos separados por ';'")
    parser.add_argument("--mxq", action="store_true", help="gera a imagem da TV box")
    parser.add_argument("--captura", metavar="PNG", help="com --teste: salva a tela no fim")
    parser.add_argument("--sem-janela", action="store_true", help="so a serial, sem tela e sem som")
    parser.add_argument("--espera", metavar="SEGUNDOS", type=float, default=3.0,
                        help="com --teste: quanto esperar o prompt depois de cada comando (padrao 3)")
    parser.add_argument("--acoes", metavar="ACOES",
                        help="com --teste: mouse e teclado pelo QMP, separados por ';'")
    parser.add_argument("--layout", choices=["abnt2", "us"], default="abnt2",
                        help="com --acoes: layout do MXOS para o digitar (padrao abnt2)")
    parser.add_argument("--disco-novo", action="store_true", help="recria saida/disco.img")
    parser.add_argument("--porta", metavar="PC:MXOS", action="append", default=[],
                        help="encaminha a porta TCP do PC (127.0.0.1) para o MXOS, ex.: 8080:80 para o httpd")
    parser.add_argument("--disco", metavar="IMG",
                        help="outra imagem de disco (padrao saida/disco.img); cada QEMU precisa da sua")
    opcoes = parser.parse_args()
    # console do Windows em cp1252/cp850: o que nao couber (ex.: "✓") vira "?"
    sys.stdout.reconfigure(errors="replace")
    global LAYOUT
    LAYOUT = opcoes.layout
    for par in opcoes.porta:
        try:
            pc, mxos = (int(x) for x in par.split(":"))
        except ValueError:
            sys.exit(f"--porta {par}: use PC:MXOS, ex.: 8080:80")
        PORTAS.append((pc, mxos))
    if opcoes.disco:
        global DISCO
        DISCO = (RAIZ / opcoes.disco).resolve()

    if opcoes.mxq:
        elf_para_imagem(compilar(mxq=True), RAIZ / "saida" / "mxos-mxq.img")
        return 0

    # "" (nenhum comando) serve para quem so quer as acoes ou a captura
    comandos = [c.strip() for c in (opcoes.teste or "").split(";") if c.strip()]
    acoes = [a.strip() for a in (opcoes.acoes or "").split(";") if a.strip()]
    try:
        validar_acoes(acoes)
    except ValueError as erro:
        sys.exit(f"--acoes: {erro}")

    kernel = compilar(mxq=False)
    preparar_disco(opcoes.disco_novo)
    if opcoes.teste is not None:
        captura = (RAIZ / opcoes.captura).resolve() if opcoes.captura else None
        return testar(kernel, opcoes.el2, comandos, captura, opcoes.espera, acoes)

    print("MXOS no QEMU. Para sair: Ctrl+A e depois X (no terminal).")
    if opcoes.sem_janela:
        return subprocess.run(argumentos_qemu(kernel, opcoes.el2, "none,id=som") + ["-nographic"]).returncode
    # janela do QEMU = a "TV" (com teclado e mouse do MXOS); a serial fica neste terminal
    print("A janela do QEMU mostra a area de trabalho; os comandos tambem podem ser digitados aqui.")
    return subprocess.run(argumentos_qemu(kernel, opcoes.el2, "dsound,id=som") + ["-serial", "mon:stdio"]).returncode


if __name__ == "__main__":
    sys.exit(main())
