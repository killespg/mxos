# SDK dos apps do MXOS

Kit para fazer **apps nativos** do MXOS: executáveis estáticos (ARMv7 para a TV box RK322x, AArch64 para o
QEMU e a MXQ) que rodam como processo e falam com o sistema pela ABI dos apps (`src/processo/abi.rs`).
O app vira um pacote `.mxa`, que a **Loja** instala e atualiza sem atualizar o sistema.

Os apps prontos, o catálogo da Loja e o empacotador ficam no repositório
[mxos-apps](https://github.com/killespg/mxos-apps).

## Um app

```rust
#![no_std]
#![no_main]

extern crate alloc;

use mxos::gui::grafico::Superficie;
use mxos::gui::janelas::{App, EventoApp};
use mxos::gui::tema;

struct Ola;

impl App for Ola {
    fn titulo(&self) -> &str {
        "Olá"
    }

    fn tamanho(&self) -> (usize, usize) {
        (320, 120)
    }

    fn desenhar(&mut self, s: &mut Superficie) {
        let t = tema::atual();
        s.limpar(t.fundo_janela);
        s.texto(16, 16, "Olá, MXOS!", t.texto, 2);
    }

    fn evento(&mut self, _e: EventoApp) -> bool {
        false
    }
}

mxos::app!(|| Ola);
```

`Cargo.toml` do app:

```toml
[dependencies]
mxos = { package = "mxos-sdk", git = "https://github.com/killespg/mxos" }
```

`.cargo/config.toml` (e o `app.ld` desta pasta copiado para a raiz do projeto do app):

```toml
[build]
target = "armv7a-none-eabihf"

[target.armv7a-none-eabihf]
rustflags = ["-C", "link-arg=-Tapp.ld", "-C", "target-cpu=cortex-a7", "-C", "target-feature=+neon,+vfp4,+d32"]

[target.aarch64-unknown-none]
rustflags = ["-C", "link-arg=-Tapp.ld"]
```

e `[profile.release] panic = "abort"`. Os alvos: `rustup target add armv7a-none-eabihf aarch64-unknown-none`.

## O que o kit tem

| Módulo | O que é |
|---|---|
| `mxos::app!` / `mxos::rodar` | abre a janela do `App`, entrega eventos, `Tique` (~10/s) e `Quadro` (~60/s, com `quer_quadros`), manda os pixels quando `evento` devolve `true`; `Pedido::Abrir` abre mais janelas no mesmo processo |
| `gui::grafico`, `gui::tema`, `gui::widgets`, `gui::realce`, `gui::barra`, `fonte` | **os mesmos arquivos** da interface do sistema (`src/gui`, `src/fonte`): desenho, temas, botões, campos de texto, listas, realce de sintaxe, fontes Inter e JetBrains Mono |
| `entrada` | botões do mouse e códigos de tecla (`entrada::teclado::KEY_*`) |
| `armazenamento` | `ler`, `escrever`, `listar`, `Arquivo` (caminho absoluto ou relativo à pasta do app) |
| `http::baixar` | GET em HTTP ou HTTPS (o sistema faz a conexão e o TLS) |
| `ia::perguntar` | pergunta ao Claude com a chave guardada no sistema (o app nunca vê a chave) |
| `audio::dsp` | som PCM de 16 bits |
| `gui::transferencia` | Ctrl+C / Ctrl+V do sistema |
| `argumentos()` | o que o sistema passou ao abrir (o caminho de um arquivo) |
| `sys` | as chamadas cruas |
| `cpu`, `video::tela` | as partes do kernel que o código do sistema usa, por cima da ABI |

Um app do sistema (`src/gui/apps/*.rs`) vira pacote trocando `crate::` por `mxos::`.

## A ABI

Número da chamada em `r7` (ARMv7) ou `x8` (AArch64), argumentos em `r0`-`r5` / `x0`-`x5`, `svc 0`, resultado em
`r0` / `x0` (valores de 64 bits no ARMv7: `r0` baixo, `r1` alto). Erro = negativo.

| # | Chamada | Argumentos → resultado |
|---|---|---|
| 0 | SAIR | código |
| 1 | LOG | texto, n → escreve no terminal do sistema |
| 2 | TEMPO_US | → microssegundos desde que ligou (64 bits) |
| 3 | DORMIR_MS | ms |
| 4 | MAPEAR | tamanho → endereço de memória zerada |
| 5 | DESMAPEAR | endereço, tamanho |
| 6 | ABRIR | caminho, n, modo (0 ler, 1 escrever, 2 acrescentar) → fd |
| 7 | LER | fd, buf, n → lidos |
| 8 | ESCREVER | fd, dados, n → escritos (grava no disco ao fechar) |
| 9 | FECHAR | fd |
| 10 | TAMANHO | fd → bytes (64 bits) |
| 11 | POSICIONAR | fd, posição (baixo, alto) |
| 12 | LISTAR | caminho, n, buf, buflen → linhas `D nome` / `F tamanho nome` (tamanho total) |
| 13 | JANELA_CRIAR | largura, altura, título, n → id |
| 14 | JANELA_MOSTRAR | id, pixels (XRGB), passo em bytes, x \| y << 16, l \| a << 16 |
| 15 | JANELA_EVENTO | id, buf de 16 bytes → 1 se veio evento `[tipo, a, b, c]` |
| 16 | ESPERAR_EVENTO | prazo em ms → 1 se há evento em alguma janela |
| 17 | JANELA_TITULO | id, título, n |
| 18 | JANELA_FECHAR | id |
| 19 | HTTP_PEDIR | url, n → id |
| 20 | HTTP_ESTADO | id → 0 baixando, `1 << 62 \| status << 32 \| tamanho` pronto, negativo erro (64 bits) |
| 21 | HTTP_LER | id, buf, n → lidos |
| 22 | SOM | pcm, n → bytes que couberam |
| 23 | ARGUMENTOS | buf, n → tamanho |
| 24 | TELA | → largura \| altura << 32 |
| 25 | TEMA | buf, n → nome do tema do sistema |
| 26 | COPIAR | texto, n |
| 27 | COLAR | buf, n → tamanho |
| 28 | IA_PERGUNTAR | sistema, n, pergunta, n → id (lido como HTTP: status 200 = resposta) |
| 29 | SOM_FORMATO | taxa, canais → taxa aceita (taxa 0 fecha) |

Eventos (`tipo`): 1 tecla (`a` = código evdev, `b` = apertada), 2 caractere (`a` = Unicode), 3 mouse (`a`, `b` =
x, y; `c` = botões), 4 botão (`a`, `b` = x, y; `c` = botão \| apertado << 8), 5 roda (`a`), 6 fechar.

## Licença

O kit é GPL-3.0-or-later como o resto do MXOS, **com a exceção dos apps** (`EXCECAO-APPS.md` na raiz): um app que
só usa a ABI e este kit (inclusive os arquivos de `src/` que ele compila junto) pode ter a licença que o autor
quiser, aberta ou fechada.
