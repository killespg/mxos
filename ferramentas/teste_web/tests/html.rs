//! Arvores do HTML5 (formato dos testes do html5lib).

use teste_web::web::{dom, html};

fn arvore(fonte: &str) -> String {
    html::depurar(&html::analisar(fonte))
}

fn confere(fonte: &str, esperado: &str) {
    let obtido = arvore(fonte);
    let esperado: String = esperado.lines().map(|l| l.trim_start()).filter(|l| !l.is_empty()).map(|l| format!("{l}\n")).collect();
    assert_eq!(obtido, esperado, "\nfonte: {fonte}\nobtido:\n{obtido}");
}

#[test]
fn paragrafos_implicitos() {
    confere(
        "<p>One<p>Two",
        r#"| <html>
           |   <head>
           |   <body>
           |     <p>
           |       "One"
           |     <p>
           |       "Two""#,
    );
}

#[test]
fn adocao_simples() {
    confere(
        "<b><i></b></i>",
        "| <html>
         |   <head>
         |   <body>
         |     <b>
         |       <i>",
    );
}

#[test]
fn adocao_com_bloco() {
    confere(
        "<b>1<p>2</b>3</p>",
        r#"| <html>
           |   <head>
           |   <body>
           |     <b>
           |       "1"
           |     <p>
           |       <b>
           |         "2"
           |       "3""#,
    );
}

#[test]
fn texto_fora_da_celula_vai_para_antes_da_tabela() {
    confere(
        "<table><tr><td>a</td></tr>x</table>",
        r#"| <html>
           |   <head>
           |   <body>
           |     "x"
           |     <table>
           |       <tbody>
           |         <tr>
           |           <td>
           |             "a""#,
    );
}

#[test]
fn links_aninhados() {
    confere(
        "<a href=x>1<a href=y>2",
        r#"| <html>
           |   <head>
           |   <body>
           |     <a>
           |       href="x"
           |       "1"
           |     <a>
           |       href="y"
           |       "2""#,
    );
}

#[test]
fn entidades() {
    confere(
        "&amp;&lt;&notin;&notit;&#x41;&#65;&copy x&#128;",
        "| <html>
         |   <head>
         |   <body>
         |     \"&<∉¬it;AA© x€\"",
    );
}

#[test]
fn svg_e_foreign_object() {
    confere(
        "<svg viewbox='0 0 1 1'><path d='M0'/><foreignObject><p>hi</p></foreignObject></svg>",
        r#"| <html>
           |   <head>
           |   <body>
           |     <svg svg>
           |       viewBox="0 0 1 1"
           |       <svg path>
           |         d="M0"
           |       <svg foreignObject>
           |         <p>
           |           "hi""#,
    );
}

#[test]
fn listas_e_tabelas_implicitas() {
    confere(
        "<ul><li>a<li>b</ul><table><td>1<td>2</table>",
        r#"| <html>
           |   <head>
           |   <body>
           |     <ul>
           |       <li>
           |         "a"
           |       <li>
           |         "b"
           |     <table>
           |       <tbody>
           |         <tr>
           |           <td>
           |             "1"
           |           <td>
           |             "2""#,
    );
}

#[test]
fn rcdata_e_script() {
    confere(
        "<title>x &amp; <b></title><script>if(a<b)w('</div>')</script>",
        r#"| <html>
           |   <head>
           |     <title>
           |       "x & <b>"
           |     <script>
           |       "if(a<b)w('</div>')"
           |   <body>"#,
    );
}

#[test]
fn doctype_e_atributos_do_html() {
    let doc = html::analisar("<!DOCTYPE html><html lang=pt><body class='a  b'>oi");
    assert_eq!(doc.modo, dom::Modo::Normal);
    assert_eq!(html::analisar("<p>sem doctype").modo, dom::Modo::Quirks);
    confere(
        "<!DOCTYPE html><html lang=pt><body class='a  b'>oi",
        r#"| <!DOCTYPE html>
           | <html>
           |   lang="pt"
           |   <head>
           |   <body>
           |     class="a  b"
           |     "oi""#,
    );
    let corpo = doc.corpo().unwrap();
    assert_eq!(doc.elemento(corpo).unwrap().classes.len(), 2);
}

#[test]
fn select_e_comentarios() {
    confere(
        "<!-- c --><select><option>a<option>b</select><!-->",
        r#"| <!--  c  -->
           | <html>
           |   <head>
           |   <body>
           |     <select>
           |       <option>
           |         "a"
           |       <option>
           |         "b"
           |     <!--  -->"#,
    );
}

#[test]
fn head_com_espacos_e_meta() {
    let doc = html::analisar("<html>
<head>
<meta charset=utf-8>
</head>
<body>
<div>x</div>
</body>
</html>
");
    let cabeca = doc.cabeca().unwrap();
    assert!(doc.achar(cabeca, teste_web::web::atomo::a::META).is_some());
    let corpo = doc.corpo().unwrap();
    let ultimo = doc.no(corpo).ultimo;
    match &doc.no(ultimo).tipo {
        dom::Tipo::Texto(t) => assert_eq!(t, "


"),
        _ => panic!("o fim do body deveria ser texto"),
    }
}

#[test]
fn atomos_fixos_sem_repeticao() {
    use teste_web::web::atomo::{Atomo, a};
    assert_eq!(Atomo::de("div"), a::DIV);
    assert_eq!(Atomo::de("viewBox"), a::VIEWBOX);
    assert_eq!(a::DIV.texto(), "div");
    assert_eq!(a::MAXLENGTH.texto(), "maxlength");
    let x = Atomo::de("meu-elemento");
    assert_eq!(Atomo::de("meu-elemento"), x);
    assert_eq!(x.texto(), "meu-elemento");
}
