"""Baixa pacotes do Debian (arm64) com as bibliotecas que eles carregam e
monta a arvore que vai para o disco do MXOS. So Python, sem dpkg nem apt.

Uso:
  python -I ferramentas/pacotes.py netsurf-fb netsurf-common ...
      baixa os .deb (cache em saida/deb/) e extrai tudo em saida/raiz/,
      que o criar_disco.py copia para o disco

Regras:
  - As dependencias seguem o que os programas carregam de verdade: o
    DT_NEEDED de cada ELF extraido vira o pacote que tem aquela biblioteca
    (pelo indice Contents do Debian). As Depends do pacote puxariam muito
    mais (o netsurf-fb pede o SDL, que pede o Mesa e o LLVM: 262 MB, quando
    o que ele carrega da uns 30). Pacotes so de dados (fontes, certificados,
    o netsurf-common) entram quando pedidos pelo nome.
  - Versoes nao sao conferidas: tudo vem da mesma distribuicao (trixie). A
    libc6 sempre entra, e o ld-linux dela vai para /lib (o PT_INTERP dos
    programas), igual ao libc.so.6 que ele carrega.
  - O FAT32 nao guarda links simbolicos: cada link vira uma copia do
    arquivo (libfoo.so.1 -> libfoo.so.1.2.3 fica com os dois nomes).
  - Scripts de instalacao (postinst) nao rodam; o que eles fariam e feito
    aqui quando importa (o pacote de certificados ganha o arquivo unico em
    /etc/ssl/certs/ca-certificates.crt, como o update-ca-certificates faz).
"""

import gzip
import io
import lzma
import pathlib
import posixpath
import shutil
import struct
import sys
import tarfile
import urllib.request

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from extrair_deb import membros_ar  # noqa: E402
import biblioteca_vazia  # noqa: E402

ESPELHO = "http://deb.debian.org/debian"
DISTRIBUICAO = "trixie"
RAIZ_PROJETO = pathlib.Path(__file__).resolve().parent.parent
CACHE = RAIZ_PROJETO / "saida" / "deb"
DESTINO = RAIZ_PROJETO / "saida" / "raiz"

# pastas que nao precisam ir para o disco (documentacao, manuais, traducoes)
IGNORAR = ("usr/share/doc/", "usr/share/man/", "usr/share/locale/", "usr/share/lintian/",
           "usr/share/bug/", "usr/share/info/", "usr/share/doc-base/", "usr/share/gdb/")

ELF = b"\x7fELF"

# bibliotecas trocadas por uma vazia (biblioteca_vazia.py): nome -> funcoes
# e o que cada uma devolve. O SDL 1.2 do Debian aborta ao carregar sem o
# SDL2 (que puxa Mesa e LLVM); o netsurf-fb desenha pelo X11 e so precisa
# que os nomes existam (SDL_Init = -1: "o SDL nao funciona aqui").
VAZIAS = {
    "libSDL-1.2.so.0": {
        "SDL_Init": -1, "SDL_Quit": 0, "SDL_WaitEvent": 0, "SDL_PollEvent": 0, "SDL_PushEvent": -1,
        "SDL_AddTimer": 0, "SDL_RemoveTimer": 0, "SDL_SetColors": 0, "SDL_SetVideoMode": 0,
        "SDL_UpperBlit": -1, "SDL_ShowCursor": 0, "SDL_GetError": 0, "SDL_UpdateRect": 0,
        "SDL_EnableKeyRepeat": 0,
    },
}

# das conversoes de caracteres da glibc (gconv, 20 MB) so as das paginas
# ocidentais: UTF-8, UTF-16 e Latin-1 ja estao dentro da libc
GCONV = "usr/lib/aarch64-linux-gnu/gconv/"
GCONV_MANTER = ("gconv-modules", "ISO8859-", "CP125", "IBM850", "KOI8-R", "LIBGCONV")


def ignorar(caminho: str) -> bool:
    if caminho.startswith(IGNORAR):
        return True
    if caminho.startswith(GCONV):
        nome = caminho[len(GCONV):]
        return "/" not in nome and not nome.startswith(GCONV_MANTER)
    return False


def baixar(url: str, destino: pathlib.Path) -> bytes:
    if destino.exists():
        return destino.read_bytes()
    print(f"baixando {url}", file=sys.stderr)
    with urllib.request.urlopen(url) as r:
        dados = r.read()
    destino.parent.mkdir(parents=True, exist_ok=True)
    destino.write_bytes(dados)
    return dados


def indice() -> dict:
    """nome -> campos do pacote, e os virtuais (Provides) apontando para o real."""
    url = f"{ESPELHO}/dists/{DISTRIBUICAO}/main/binary-arm64/Packages.xz"
    texto = lzma.decompress(baixar(url, CACHE / f"Packages-{DISTRIBUICAO}.xz")).decode("utf-8", "replace")
    pacotes, virtuais = {}, {}
    for bloco in texto.split("\n\n"):
        campos, ultimo = {}, None
        for linha in bloco.splitlines():
            if linha.startswith((" ", "\t")) and ultimo:
                campos[ultimo] += "\n" + linha.strip()
            elif ":" in linha:
                ultimo, valor = linha.split(":", 1)
                campos[ultimo] = valor.strip()
        if "Package" not in campos:
            continue
        pacotes[campos["Package"]] = campos
        for p in campos.get("Provides", "").split(","):
            nome = p.split("(")[0].strip()
            if nome:
                virtuais.setdefault(nome, campos["Package"])
    return {"pacotes": pacotes, "virtuais": virtuais}


def conteudos() -> dict:
    """Nome de biblioteca (libfoo.so.1) -> pacote que a tem."""
    url = f"{ESPELHO}/dists/{DISTRIBUICAO}/main/Contents-arm64.gz"
    texto = gzip.decompress(baixar(url, CACHE / f"Contents-{DISTRIBUICAO}-arm64.gz")).decode("utf-8", "replace")
    mapa = {}
    for linha in texto.splitlines():
        caminho, _, pacotes = linha.rpartition(" ")
        caminho = caminho.strip()
        pasta = posixpath.dirname(caminho)
        if pasta not in ("usr/lib/aarch64-linux-gnu", "lib/aarch64-linux-gnu", "usr/lib", "lib"):
            continue
        pacote = pacotes.split(",")[0].split("/")[-1]
        mapa.setdefault(posixpath.basename(caminho), pacote)
    return mapa


def precisa(dados: bytes) -> list:
    """Os DT_NEEDED de um ELF64 little-endian (vazio se nao for)."""
    if dados[:4] != ELF or len(dados) < 64 or dados[4] != 2:
        return []
    phoff, = struct.unpack_from("<Q", dados, 32)
    phentsize, phnum = struct.unpack_from("<HH", dados, 54)
    cargas, dinamico = [], None
    for k in range(phnum):
        tipo, _, offset, vaddr, _, filesz, _, _ = struct.unpack_from("<IIQQQQQQ", dados, phoff + k * phentsize)
        if tipo == 1:
            cargas.append((vaddr, offset, filesz))
        elif tipo == 2:
            dinamico = (offset, filesz)
    if dinamico is None:
        return []

    def no_arquivo(endereco):
        for vaddr, offset, filesz in cargas:
            if vaddr <= endereco < vaddr + filesz:
                return endereco - vaddr + offset
        return None

    nomes, tabela = [], None
    for k in range(dinamico[1] // 16):
        tag, valor = struct.unpack_from("<qQ", dados, dinamico[0] + k * 16)
        if tag == 0:
            break
        if tag == 1:
            nomes.append(valor)
        elif tag == 5:
            tabela = no_arquivo(valor)
    if tabela is None:
        return []
    return [dados[tabela + n:dados.index(b"\0", tabela + n)].decode() for n in nomes]


def extrair(deb: bytes, destino: pathlib.Path, links: list) -> list:
    """Extrai os arquivos; devolve os caminhos (arquivos e links)."""
    extraidos = []
    for nome, conteudo in membros_ar(deb):
        if not nome.startswith("data.tar"):
            continue
        with tarfile.open(fileobj=io.BytesIO(conteudo)) as tar:
            for m in tar.getmembers():
                caminho = posixpath.normpath(m.name.lstrip("./"))
                if caminho in (".", "") or ignorar(caminho):
                    continue
                alvo = destino / caminho
                if m.isdir():
                    alvo.mkdir(parents=True, exist_ok=True)
                elif m.issym():
                    links.append((caminho, m.linkname))
                    extraidos.append(caminho)
                elif m.isfile():
                    alvo.parent.mkdir(parents=True, exist_ok=True)
                    alvo.write_bytes(tar.extractfile(m).read())
                    extraidos.append(caminho)
    return extraidos


def resolver_links(destino: pathlib.Path, links: list) -> None:
    """Troca cada link por uma copia do alvo (varias voltas: link de link)."""
    pendentes = links
    movidos = {}
    for _ in range(8):
        sobrou = []
        for caminho, alvo in pendentes:
            base = posixpath.dirname(caminho)
            real = posixpath.normpath(alvo.lstrip("/") if alvo.startswith("/") else posixpath.join(base, alvo))
            origem = destino / real
            nome, nome_real = posixpath.basename(caminho), posixpath.basename(real)
            if origem.is_file() and posixpath.dirname(real) == base and nome_real.startswith(nome + "."):
                # libfoo.so.4 -> libfoo.so.4.8.0: fica so o nome que se carrega
                origem.replace(destino / caminho)
                movidos[real] = caminho
            elif real in movidos:
                shutil.copyfile(destino / movidos[real], destino / caminho)
            elif origem.is_file():
                (destino / caminho).parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(origem, destino / caminho)
            elif origem.is_dir():
                shutil.copytree(origem, destino / caminho, dirs_exist_ok=True)
            else:
                sobrou.append((caminho, alvo))
        if not sobrou or len(sobrou) == len(pendentes):
            for caminho, alvo in sobrou:
                print(f"aviso: link sem alvo: {caminho} -> {alvo}", file=sys.stderr)
            return
        pendentes = sobrou


def certificados(destino: pathlib.Path) -> None:
    """O que o update-ca-certificates faz: todos os .crt num arquivo so."""
    pasta = destino / "usr/share/ca-certificates"
    if not pasta.is_dir():
        return
    juntos = b"".join(p.read_bytes() for p in sorted(pasta.rglob("*.crt")))
    saida = destino / "etc/ssl/certs/ca-certificates.crt"
    saida.parent.mkdir(parents=True, exist_ok=True)
    saida.write_bytes(juntos)


def instalar(pedidos: list, idx: dict) -> tuple:
    """Extrai os pedidos e, seguindo o DT_NEEDED, as bibliotecas que eles
    carregam. Devolve os pacotes (na ordem) e os links a resolver."""
    mapa = conteudos()
    pacotes = idx["pacotes"]
    fila = ["libc6"] + list(pedidos)
    feitos, tem, links = [], set(), []
    while fila:
        nome = fila.pop(0)
        nome = nome if nome in pacotes else idx["virtuais"].get(nome, nome)
        if nome in feitos:
            continue
        if nome not in pacotes:
            print(f"aviso: {nome} nao existe no {DISTRIBUICAO}", file=sys.stderr)
            continue
        feitos.append(nome)
        arquivo = pacotes[nome]["Filename"]
        deb = baixar(f"{ESPELHO}/{arquivo}", CACHE / posixpath.basename(arquivo))
        novos = extrair(deb, DESTINO, links)
        tem.update(posixpath.basename(c) for c in novos)
        for caminho in novos:
            local = DESTINO / caminho
            if not local.is_file() or local.stat().st_size < 64:
                continue
            with open(local, "rb") as f:
                if f.read(4) != ELF:
                    continue
            for soname in precisa(local.read_bytes()):
                if soname in tem:
                    continue
                if soname in VAZIAS:
                    destino = DESTINO / "usr/lib/aarch64-linux-gnu" / soname
                    destino.parent.mkdir(parents=True, exist_ok=True)
                    destino.write_bytes(biblioteca_vazia.gerar(soname, VAZIAS[soname]))
                    tem.add(soname)
                    continue
                dono = mapa.get(soname)
                if dono is None:
                    print(f"aviso: ninguem tem {soname} (pedida por {caminho})", file=sys.stderr)
                elif dono not in feitos and dono not in fila:
                    fila.append(dono)
    return feitos, links


def main() -> int:
    pedidos = [a for a in sys.argv[1:] if not a.startswith("--")]
    if not pedidos:
        sys.exit(__doc__)
    idx = indice()
    if DESTINO.exists():
        for filho in DESTINO.iterdir():
            shutil.rmtree(filho) if filho.is_dir() else filho.unlink()
    lista, links = instalar(pedidos, idx)
    for nome in lista:
        print(f"{nome:40} {idx['pacotes'][nome]['Version']}")
    certificados(DESTINO)
    resolver_links(DESTINO, links)
    # o PT_INTERP dos programas e /lib/ld-linux-aarch64.so.1
    for ld in ("usr/lib/ld-linux-aarch64.so.1", "usr/lib/aarch64-linux-gnu/ld-linux-aarch64.so.1"):
        if (DESTINO / ld).is_file():
            (DESTINO / "lib").mkdir(exist_ok=True)
            shutil.copyfile(DESTINO / ld, DESTINO / "lib/ld-linux-aarch64.so.1")
            break
    (DESTINO / "var/lib/mxos").mkdir(parents=True, exist_ok=True)
    (DESTINO / "var/lib/mxos/pacotes").write_text("".join(f"{n}\n" for n in lista))
    tamanho = sum(p.stat().st_size for p in DESTINO.rglob("*") if p.is_file())
    print(f"{len(lista)} pacotes; ok: {DESTINO} ({tamanho // (1024 * 1024)} MB)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
