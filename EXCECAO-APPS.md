# Exceção para os apps do MXOS

O MXOS (o kernel e todo o sistema neste repositório) é software livre sob a **GNU General Public License,
versão 3 ou (a seu critério) qualquer versão posterior** (`LICENSE`). Quem modificar o MXOS ou fizer um
sistema a partir dele tem de distribuir o resultado sob a mesma licença, com o código-fonte.

Como **exceção adicional** (seção 7 da GPL-3.0), os detentores dos direitos do MXOS dão a seguinte permissão:

1. Um **app do MXOS** é um programa que roda como processo sobre o MXOS e fala com o sistema apenas pela ABI
   dos apps (as chamadas de sistema descritas em `src/processo/abi.rs` e `sdk/README.md`), distribuído
   separadamente do sistema (por exemplo, como pacote `.mxa`).

2. Um app do MXOS **não é considerado obra derivada do MXOS** só por usar essa ABI. Isso vale também quando o
   app é compilado com o kit de desenvolvimento (`sdk/`), inclusive com as partes do sistema que o kit compila
   junto com o app (como `src/fonte`, `src/gui/grafico.rs`, `src/gui/tema.rs`, `src/gui/widgets.rs`,
   `src/gui/realce.rs`, `src/gui/barra.rs` e `src/entrada/teclado.rs`).

3. O autor de um app do MXOS pode distribuí-lo **sob a licença que quiser**, livre ou proprietária, sem as
   obrigações da GPL em relação ao código do app.

4. A exceção **não** se aplica a modificações do próprio kernel ou do sistema, nem a obras que incorporem o
   código do MXOS de outra forma que não a descrita acima: essas continuam sob a GPL-3.0-or-later.

Quem modificar o MXOS pode manter esta exceção na sua versão ou retirá-la.

## Terceiros

- As fontes em `src/fonte/*.mxf` são convertidas da **Inter** (Rasmus Andersson) e da **JetBrains Mono**
  (JetBrains), ambas sob a SIL Open Font License 1.1.
- As tabelas de códigos de Huffman e de quantização do decodificador AAC (`src/midia/aac_tabelas.rs`) são os
  valores da norma ISO/IEC 14496-3, conferidos com as tabelas do FFmpeg (LGPL-2.1-or-later).
