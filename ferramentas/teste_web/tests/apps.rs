//! Pedacos dos apps que rodam no PC: JSON e realce de sintaxe.

use teste_web::json::{self, Json};
use teste_web::realce::{self, Classe, Linguagem};

#[test]
fn json_resposta_da_api() {
    let r = r#"{"id":"msg_1","content":[{"type":"thinking","thinking":""},{"type":"text","text":"Ol@u00e1@n@"mundo@" @ud83d@ude00"}],"stop_reason":"end_turn","usage":{"input_tokens":12}}"#.replace('@', "\\");
    let r = r.as_bytes();
    let d = json::ler(r).unwrap();
    let blocos = d.campo("content").unwrap().lista();
    assert_eq!(blocos[1].campo("text").and_then(Json::texto), Some("Olá\n\"mundo\" 😀"));
    assert_eq!(d.campo("usage").unwrap().campo("input_tokens"), Some(&Json::Num(12.0)));
    assert_eq!(json::texto("a\"b\n\u{1}"), "\"a\\\"b\\n\\u0001\"");
    assert!(json::ler(b"{\"a\":}").is_none());
}

fn classes(l: Linguagem, s: &str, e: u8) -> (Vec<(String, Classe)>, u8) {
    let mut v = Vec::new();
    let fim = realce::linha(l, s, e, &mut |a, b, c| v.push((s[a..b].to_string(), c)));
    (v, fim)
}

#[test]
fn realce() {
    let (v, e) = classes(Linguagem::C, "int main(void) { return 0x1F; } /* abre", 0);
    assert!(v.contains(&("int".into(), Classe::Tipo)));
    assert!(v.contains(&("main".into(), Classe::Funcao)));
    assert!(v.contains(&("return".into(), Classe::Palavra)));
    assert!(v.contains(&("0x1F".into(), Classe::Numero)));
    assert_ne!(e, 0);
    let (v, e) = classes(Linguagem::C, r#"fecha */ x = "s\"t";"#, e);
    assert_eq!(v[0], ("fecha */".into(), Classe::Comentario));
    assert!(v.contains(&(r#""s\"t""#.into(), Classe::Texto)));
    assert_eq!(e, 0);
    let (v, e) = classes(Linguagem::Js, "const s = `a ${x}", 0);
    assert!(v.contains(&("const".into(), Classe::Palavra)));
    assert_ne!(e, 0);
    let (_, e) = classes(Linguagem::Js, "fim` // ok", e);
    assert_eq!(e, 0);
    let (v, _) = classes(Linguagem::Rust, r#"fn f<'a>(x: &'a str) -> Option<u8> { println!("{}", 'c') }"#, 0);
    assert!(v.contains(&("Option".into(), Classe::Tipo)));
    assert!(v.contains(&("println".into(), Classe::Preproc)));
    assert!(v.contains(&("'c'".into(), Classe::Texto)));
    let (v, _) = classes(Linguagem::Html, r#"<a href="x">t&amp;</a>"#, 0);
    assert_eq!(v[0], ("<a".into(), Classe::Palavra));
    assert!(v.contains(&("href".into(), Classe::Tipo)));
    assert!(v.contains(&("&amp;".into(), Classe::Constante)));
}
