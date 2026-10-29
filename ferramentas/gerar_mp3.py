"""Gera src/midia/mp3_tabelas.rs com as tabelas de Huffman do Layer III
(ISO 11172-3, anexo B), tiradas do tables.c do LAME (so os numeros:
codigo e tamanho de cada par x,y das tabelas grandes e das count1).

A janela de sintese (D, tabela 3-B.3, 512 coeficientes) vem do pdmp3.c
(decodificador de dominio publico), tambem so os numeros.

Uso: python -I ferramentas/gerar_mp3.py caminho/do/tables.c caminho/do/pdmp3.c
"""

import pathlib
import re
import sys

TABELAS = [1, 2, 3, 5, 6, 7, 8, 9, 10, 11, 12, 13, 15, 16, 24, 32, 33]
# dimensao (xlen) de cada tabela grande
DIM = {1: 2, 2: 3, 3: 3, 5: 4, 6: 4, 7: 6, 8: 6, 9: 6, 10: 8, 11: 8, 12: 8, 13: 16, 15: 16, 16: 16, 24: 16}


def vetor(texto, nome):
    m = re.search(r"\b" + nome + r"\[\]\s*=\s*\{(.*?)\};", texto, re.S)
    if not m:
        raise SystemExit(f"nao achei {nome}")
    corpo = re.sub(r"/\*.*?\*/", "", m.group(1), flags=re.S)
    return [eval(x.strip()) for x in corpo.split(",") if x.strip()]


def main():
    texto = pathlib.Path(sys.argv[1]).read_text(encoding="latin-1")
    saida = [
        "//! Tabelas de Huffman do MP3 (ISO 11172-3, anexo B), geradas por",
        "//! ferramentas/gerar_mp3.py. Cada tabela: (dimensao, codigos, tamanhos),",
        "//! no indice x * dimensao + y; as count1 (32 e 33) no indice vwxy.",
        "",
    ]
    for n in TABELAS:
        codigos = vetor(texto, f"t{n}HB")
        tamanhos = vetor(texto, f"t{n}l")
        if n in (32, 33):
            # o LAME guarda os bits de sinal junto: tira
            pc = [bin(i).count("1") for i in range(16)]
            codigos = [c >> pc[i] for i, c in enumerate(codigos)]
            tamanhos = [t - pc[i] for i, t in enumerate(tamanhos)]
        dim = DIM.get(n, 0)
        if dim:
            # idem nas grandes: cada x ou y diferente de zero soma um bit de sinal
            tamanhos = [t - (i // dim != 0) - (i % dim != 0) for i, t in enumerate(tamanhos)]
        esperado = dim * dim if dim else 16
        assert len(codigos) == esperado and len(tamanhos) == esperado, (n, len(codigos), len(tamanhos))
        saida.append(f"pub const C{n}: [u16; {esperado}] = {codigos};")
        saida.append(f"pub const L{n}: [u8; {esperado}] = {tamanhos};")
    saida.append("")
    saida.append("/// (dimensao, codigos, tamanhos) de cada tabela 0..=33 (None = nao existe).")
    saida.append("pub fn tabela(n: usize) -> Option<(usize, &'static [u16], &'static [u8])> {")
    saida.append("    Some(match n {")
    for n in TABELAS:
        if n in (16, 24):
            continue
        dim = DIM.get(n, 0)
        saida.append(f"        {n} => ({dim}, &C{n}, &L{n}),")
    saida.append("        16..=23 => (16, &C16, &L16),")
    saida.append("        24..=31 => (16, &C24, &L24),")
    saida.append("        _ => return None,")
    saida.append("    })")
    saida.append("}")
    saida.append("")
    saida.append("/// Bits extras (linbits) das tabelas 0..=31.")
    lin = [0] * 16 + [1, 2, 3, 4, 6, 8, 10, 13, 4, 5, 6, 7, 8, 9, 11, 13]
    saida.append(f"pub const LINBITS: [u8; 32] = {lin};")
    t = pathlib.Path(sys.argv[2]).read_text(encoding="latin-1")
    m = re.search(r"g_synth_dtbl\[512\]\s*=\s*\{(.*?)\}", t, re.S)
    corpo = re.sub(r"//[^\n]*", "", re.sub(r"/\*.*?\*/", "", m.group(1), flags=re.S))
    janela = [float(x) for x in re.findall(r"-?\d+\.\d+(?:[eE]-?\d+)?", corpo)]
    assert len(janela) == 512 and abs(janela[256] - 1.144989014) < 1e-9
    saida.append("")
    saida.append("/// Janela da sintese polifasica (D[i], tabela 3-B.3).")
    saida.append("pub const JANELA: [f32; 512] = [" + ", ".join(repr(x) for x in janela) + "];")
    destino = pathlib.Path(__file__).parent.parent / "src" / "midia" / "mp3_tabelas.rs"
    destino.write_text("\n".join(saida) + "\n", encoding="utf-8")
    print("gravado", destino)


main()
