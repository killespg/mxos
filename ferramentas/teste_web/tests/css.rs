//! CSS: seletores, cascata e valores calculados.

use teste_web::web::atomo::a;
use teste_web::web::css::cascata::{self, Indice};
use teste_web::web::css::estilo::{Exibir, Familia, Tam};
use teste_web::web::css::folha::{Folha, Midia};
use teste_web::web::css::seletor::Estado;
use teste_web::web::css::valores::{Cor, Medida, UM};
use teste_web::web::html;

const M: Midia = Midia { largura: 1280.0, altura: 720.0 };

fn estilos(fonte: &str, css: &str) -> (teste_web::web::dom::Documento, cascata::Estilos) {
    let doc = html::analisar(fonte);
    let ind = Indice::novo(vec![Folha::ler(css, &M)], &M);
    let e = cascata::calcular(&doc, &ind, &M, Estado::default());
    (doc, e)
}

#[test]
fn especificidade_e_ordem() {
    let (doc, e) = estilos(
        "<div id=x class='a b'><p class=c>oi</p></div>",
        "p { color: red } .c { color: blue } div > p { color: green } #x .c { color: #123456 } p { margin: 10px 5% }",
    );
    let p = doc.achar(0, a::P).unwrap();
    let s = e.de(p).unwrap();
    assert_eq!(s.cor, Cor(0xFF123456));
    assert_eq!(s.margem[0], Tam::M(Medida::px(10 * UM)));
    assert_eq!(s.margem[1], Tam::M(Medida { px: 0, pct: 5 * UM }));
    assert_eq!(s.exibir, Exibir::Bloco);
}

#[test]
fn heranca_em_e_variaveis() {
    let (doc, e) = estilos(
        "<div><span>x</span></div>",
        ":root { --base: 20px; --cor: rgb(0 128 255 / 50%) } div { font-size: var(--base); font-family: Arial, sans-serif; padding: calc(1em + 2px) } span { color: var(--cor); font-size: 1.5em; width: calc(100% - 2em) }",
    );
    let d = doc.achar(0, a::DIV).unwrap();
    let s = doc.achar(0, a::SPAN).unwrap();
    let ed = e.de(d).unwrap();
    let es = e.de(s).unwrap();
    assert_eq!(ed.tamanho_fonte, 20.0);
    assert_eq!(ed.preenchimento[0], Medida::px(22 * UM));
    assert_eq!(es.tamanho_fonte, 30.0);
    assert_eq!(es.familia, Familia::SemSerif);
    assert_eq!(es.cor, Cor::rgba(0, 128, 255, 128));
    assert_eq!(es.largura, Tam::M(Medida { px: -60 * UM, pct: 100 * UM }));
}

#[test]
fn seletores_complexos_e_media() {
    let (doc, e) = estilos(
        "<ul><li>1<li>2<li>3<li>4</ul><a href=x>l</a><input type=text disabled>",
        "li:nth-child(2n+1) { color: red } li:not(:first-child):last-child { color: blue } \
         a[href]:hover { color: lime } input:disabled { display: none } \
         @media (max-width: 600px) { ul { display: none } } @media (min-width: 1000px) { ul { color: teal } } \
         @supports (display: grid) { li { margin-left: 3px } }",
    );
    let lis: Vec<_> = doc.descendentes(0).filter(|&x| doc.eh(x, a::LI)).collect();
    let cor = |k: usize| e.de(lis[k]).unwrap().cor;
    assert_eq!(cor(0), Cor(0xFFFF0000));
    assert_eq!(cor(1), Cor(0xFF008080));
    assert_eq!(cor(2), Cor(0xFFFF0000));
    assert_eq!(cor(3), Cor(0xFF0000FF));
    assert_eq!(e.de(lis[0]).unwrap().margem[3], Tam::M(Medida::px(3 * UM)));
    let ul = doc.achar(0, a::UL).unwrap();
    assert_eq!(e.de(ul).unwrap().exibir, Exibir::Bloco);
    let input = doc.achar(0, a::INPUT).unwrap();
    assert_eq!(e.de(input).unwrap().exibir, Exibir::Nenhum);
}

#[test]
fn important_e_inline() {
    let (doc, e) = estilos(
        "<p style='color: green; margin-top: 7px'>x</p>",
        "p { color: red !important; margin-top: 1px !important }",
    );
    let p = doc.achar(0, a::P).unwrap();
    assert_eq!(e.de(p).unwrap().cor, Cor(0xFFFF0000));
    assert_eq!(e.de(p).unwrap().margem[0], Tam::M(Medida::px(UM)));
}

#[test]
fn antes_e_depois() {
    let (doc, e) = estilos("<q>x</q><p>y</p>", "p::before { content: '> '; color: red } p:after { content: attr(title) }");
    let p = doc.achar(0, a::P).unwrap();
    assert!(e.antes.contains_key(&p));
    assert!(e.depois.contains_key(&p));
    let q = doc.achar(0, a::Q).unwrap();
    assert!(e.antes.contains_key(&q));
}
