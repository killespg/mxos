//! Paginas desenhadas em PNG (target/saidas/*.png) para conferir a olho.

use teste_web::render::renderizar;

const TESTE: &str = r##"<!DOCTYPE html>
<html><head><title>Teste</title><style>
body { font-family: sans-serif; margin: 0; background: #f4f6fa; color: #1f2937 }
header { background: linear-gradient(90deg, #1e3a8a, #2563eb); color: white; padding: 16px 24px; display: flex; justify-content: space-between; align-items: center }
header nav a { color: #dbeafe; margin-left: 16px; text-decoration: none }
main { max-width: 760px; margin: 20px auto; padding: 0 16px }
.cartao { background: white; border: 1px solid #d1d5db; border-radius: 8px; padding: 16px; margin-bottom: 16px; box-shadow: 0 2px 4px rgba(0,0,0,.1) }
.grade { display: grid; grid-template-columns: repeat(3, 1fr); gap: 12px }
.grade div { background: #e0e7ff; padding: 12px; text-align: center; border-radius: 6px }
.flutua { float: right; width: 120px; height: 80px; background: #fde68a; margin: 0 0 8px 12px; padding: 4px }
table { border-collapse: collapse; width: 100% }
td, th { border: 1px solid #9ca3af; padding: 6px 10px; text-align: left }
th { background: #f3f4f6 }
.botao { display: inline-block; background: #16a34a; color: white; padding: 6px 14px; border-radius: 4px }
code { background: #f3f4f6; padding: 1px 4px }
.rodape { text-align: center; font-size: 12px; color: #6b7280 }
</style></head>
<body>
<header><strong style="font-size:20px">MXOS Web</strong><nav><a href="#">Início</a><a href="#">Notícias</a><a href="#">Sobre</a></nav></header>
<main>
<div class="cartao"><h1>Motor web feito do zero</h1>
<div class="flutua">Um float à direita com texto</div>
<p>Este parágrafo tem <b>negrito</b>, <i>itálico</i>, <a href="/x">um link sublinhado</a>, <code>código</code> e texto suficiente para quebrar em várias linhas em volta do float à direita, testando a largura livre das linhas.</p>
<p>Segundo parágrafo, <span style="background:#fef08a">com fundo amarelo num span</span> e <span class="botao">Botão</span> inline-block no meio do texto.</p></div>
<div class="cartao"><h2>Grade 3x2</h2><div class="grade"><div>Um</div><div>Dois</div><div>Três</div><div>Quatro</div><div style="grid-column: span 2">Cinco (span 2)</div></div></div>
<div class="cartao"><h2>Lista e tabela</h2>
<ul><li>Primeiro item</li><li>Segundo item com <em>ênfase</em></li></ul>
<ol><li>Um</li><li>Dois</li><li>Três</li></ol>
<table><tr><th>Nome</th><th>Valor</th><th>Obs</th></tr><tr><td>CPU</td><td>4 núcleos</td><td>Cortex-A7</td></tr><tr><td>RAM</td><td>1 GB</td><td>DDR3</td></tr></table></div>
<div class="cartao" style="display:flex; gap: 10px"><div style="flex:1; background:#fee2e2; padding:8px">flex: 1</div><div style="flex:2; background:#dcfce7; padding:8px">flex: 2 com mais texto aqui dentro</div><div style="width:100px; background:#e0f2fe; padding:8px">100px</div></div>
<p class="rodape">Rodapé centralizado · MXOS 2026</p>
</main></body></html>"##;

#[test]
fn pagina_de_teste() {
    let (h, t) = renderizar("teste", TESTE.as_bytes(), None, "", 900, 2000);
    println!("teste: altura {h}px, tempos {:?} ms", t);
}

#[test]
fn sites_reais() {
    let sites = [
        ("example", "https://example.com/"),
        ("hn", "https://news.ycombinator.com/"),
        ("wikipedia", "https://pt.wikipedia.org/wiki/Brasil"),
        ("google", "https://www.google.com/"),
        ("govbr", "https://www.gov.br/pt-br"),
        ("mdn", "https://developer.mozilla.org/pt-BR/"),
        ("github", "https://github.com/"),
        ("uol", "https://www.uol.com.br/"),
    ];
    for (nome, url) in sites {
        let Ok((tipo, bytes)) = teste_web::rede::pagina(url) else { continue };
        let (h, t) = renderizar(nome, &bytes, Some(&tipo), url, 1280, 2400);
        println!("{nome:10} altura {h:6}px  html {:6.1} css {:6.1} layout {:6.1} pintura {:6.1} ms", t[0], t[1], t[2], t[3]);
    }
}
