"""Gera src/web/html/entidades.rs: as referencias de caractere nomeadas do
HTML5 ("&amp;", "&nbsp;", "&notin;"...), tiradas da tabela que o proprio
Python traz (html.entities.html5, a mesma da especificacao WHATWG).

    python ferramentas/gerar_entidades.py

Formato: uma lista ordenada de (nome sem o '&', texto), para busca binaria.
Nomes sem ';' no fim sao as formas antigas aceitas sem ponto e virgula.
"""

import html.entities
import pathlib

DESTINO = pathlib.Path(__file__).resolve().parent.parent / "src/web/html/entidades.rs"


def rust(texto: str) -> str:
    return "".join(c if 32 <= ord(c) < 127 and c not in '"\\' else f"\\u{{{ord(c):x}}}" for c in texto)


def main() -> None:
    itens = sorted(html.entities.html5.items(), key=lambda kv: kv[0].encode())
    linhas = [
        "//! Referencias de caractere nomeadas do HTML5. GERADO por",
        "//! ferramentas/gerar_entidades.py: nao editar a mao.",
        "",
        f"pub static ENTIDADES: [(&str, &str); {len(itens)}] = [",
    ]
    for nome, texto in itens:
        linhas.append(f'    ("{nome}", "{rust(texto)}"),')
    linhas.append("];")
    DESTINO.parent.mkdir(parents=True, exist_ok=True)
    DESTINO.write_text("\n".join(linhas) + "\n", encoding="utf-8")
    print(f"{len(itens)} entidades em {DESTINO}")


if __name__ == "__main__":
    main()
