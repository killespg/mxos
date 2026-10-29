//! JavaScript: cada caso roda e compara o valor final (como texto).

use teste_web::web::js::{self, vm::Vm};

fn rodar(fonte: &str) -> String {
    let mut vm = Vm::novo();
    match js::avaliar(&mut vm, fonte) {
        Ok(v) => vm.para_texto(&v).map(|s| s.to_string()).unwrap_or_else(|_| "<obj>".into()),
        Err(e) => format!("ERRO: {}", js::descrever_erro(&mut vm, &e)),
    }
}

fn caso(fonte: &str, esperado: &str) {
    let r = rodar(fonte);
    assert_eq!(r, esperado, "\nfonte: {fonte}");
}

#[test]
fn basico() {
    caso("1 + 2 * 3", "7");
    caso("'a' + 1 + 2", "a12");
    caso("let x = 10; x += 5; x", "15");
    caso("var a = [1,2,3]; a.length", "3");
    caso("typeof null + typeof undefined + typeof 1 + typeof 'x' + typeof {} + typeof function(){}", "objectundefinednumberstringobjectfunction");
    caso("0.1 + 0.2", "0.30000000000000004");
    caso("1/3", "0.3333333333333333");
    caso("1e21 + ''", "1e+21");
    caso("(123.456).toFixed(1)", "123.5");
    caso("[1,2,3].map(x => x * 2).join('-')", "2-4-6");
    caso("'abc'.toUpperCase().split('').reverse().join('')", "CBA");
    caso("null ?? 'padrao'", "padrao");
    caso("let o = {a: {b: 2}}; o?.a?.b + (o.x?.y ?? 0)", "2");
    caso("2 ** 10", "1024");
    caso("'5' * '2'", "10");
    caso("[] + {}", "[object Object]");
    caso("1 == '1' && null == undefined && !(null === undefined)", "true");
}

#[test]
fn funcoes_e_closures() {
    caso("function f(n) { return n <= 1 ? 1 : n * f(n - 1) } f(10)", "3628800");
    caso("const cont = (() => { let n = 0; return () => ++n })(); cont(); cont(); cont()", "3");
    caso("var fs = []; for (let i = 0; i < 3; i++) fs.push(() => i); fs.map(f => f()).join()", "0,1,2");
    caso("var fs = []; for (var i = 0; i < 3; i++) fs.push(() => i); fs.map(f => f()).join()", "3,3,3");
    caso("function soma(...xs) { return xs.reduce((a, b) => a + b, 0) } soma(1, 2, 3, 4)", "10");
    caso("function p(a, b = a * 2) { return a + b } p(3)", "9");
    caso("const {a, b: {c = 5} = {}, ...resto} = {a: 1, d: 4, e: 5}; a + c + Object.keys(resto).join()", "6d,e");
    caso("const [x, , y = 9, ...z] = [1, 2, undefined, 4, 5]; x + y + z.length", "12");
    caso("function f() { return arguments.length } f(1, 2, 3)", "3");
    caso("var o = { n: 1, f() { return this.n } }; o.f()", "1");
    caso("function F() { this.v = 7 } new F().v", "7");
    caso("(function nome() { return typeof nome })()", "function");
    caso("var g = function() { return 1 }; g.name", "g");
    caso("Math.max.apply(null, [1, 5, 3])", "5");
    caso("var f = function(a) { return this.x + a }.bind({x: 10}, 5); f()", "15");
}

#[test]
fn classes() {
    caso(
        "class A { constructor(x) { this.x = x } get dobro() { return this.x * 2 } static fab() { return new A(4) } }
         class B extends A { constructor() { super(5); this.y = 1 } soma() { return this.x + this.y + super.toString().length } }
         const b = new B(); [b.dobro, b.soma(), A.fab().x, b instanceof A].join()",
        "10,21,4,true",
    );
    caso("class C { #p = 3; n = 2; get p() { return this.#p } } new C().p + new C().n", "5");
    caso("class D { static s = 9; static { D.t = D.s + 1 } } D.t", "10");
}

#[test]
fn controle() {
    caso("let s = 0; for (const x of [1,2,3]) s += x; s", "6");
    caso("let s = ''; for (const k in {a:1, b:2}) s += k; s", "ab");
    caso("let n = 0; while (true) { if (++n > 5) break } n", "6");
    caso("let r = []; outer: for (let i = 0; i < 3; i++) { for (let j = 0; j < 3; j++) { if (j == 1) continue outer; if (i == 2) break outer; r.push(i+''+j) } } r.join()", "00,10");
    caso("function f(x) { switch (x) { case 1: return 'um'; case 2: case 3: return 'dois ou tres'; default: return 'outro' } } [f(1), f(3), f(9)].join()", "um,dois ou tres,outro");
    caso("let r; try { throw new Error('x') } catch (e) { r = e.message } finally { r += '!' } r", "x!");
    caso("function f() { try { return 1 } finally { g = 2 } } var g; f() + g", "3");
    caso("let s = ''; for (const [k, v] of Object.entries({a: 1})) s += k + v; s", "a1");
    caso("let r; try { null.x } catch (e) { r = e instanceof TypeError } r", "true");
    caso("let r; try { naoExiste } catch (e) { r = e.name } r", "ReferenceError");
}

#[test]
fn geradores_e_promessas() {
    caso("function* g() { yield 1; yield 2; return 3 } [...g()].join()", "1,2");
    caso("function* g() { const x = yield 1; yield x * 2 } const it = g(); it.next(); it.next(21).value", "42");
    caso("function* a() { yield* [1, 2]; yield 3 } Array.from(a()).join()", "1,2,3");
    caso("var r = []; Promise.resolve(1).then(v => r.push(v)); r.push(0); r.join()", "0");
}

#[test]
fn async_completo() {
    let mut vm = Vm::novo();
    let r = js::avaliar(
        &mut vm,
        "var log = []; async function f() { log.push('a'); const x = await Promise.resolve(2); log.push('c' + x); return 5 }
         f().then(v => log.push('v' + v)); log.push('b');",
    );
    assert!(r.is_ok(), "{:?}", r.err().map(|e| js::descrever_erro(&mut vm, &e)));
    let v = js::avaliar(&mut vm, "log.join()").unwrap();
    assert_eq!(&*vm.para_texto(&v).unwrap(), "a,b,c2,v5");
    js::avaliar(&mut vm, "var t = []; Promise.all([1, Promise.resolve(2), new Promise(r => r(3))]).then(v => t.push(v.join('+')));").unwrap();
    let v = js::avaliar(&mut vm, "t.join()").unwrap();
    assert_eq!(&*vm.para_texto(&v).unwrap(), "1+2+3");
}

#[test]
fn biblioteca() {
    caso("JSON.stringify({a: [1, 'x', null, true], b: {c: 1.5}})", r#"{"a":[1,"x",null,true],"b":{"c":1.5}}"#);
    caso(r#"JSON.parse('{"a": [1, 2, {"b": "\\u00e7"}]}').a[2].b"#, "ç");
    caso("JSON.stringify({a: 1, b: [2]}, null, 2)", "{\n  \"a\": 1,\n  \"b\": [\n    2\n  ]\n}");
    caso("'a-b-c'.replace(/-/g, '+')", "a+b+c");
    caso(r"'2024-01-15'.match(/(\d+)-(\d+)/)[2]", "01");
    caso(r"'John Smith'.replace(/(?<p>\w+) (?<s>\w+)/, '$<s>, $<p>')", "Smith, John");
    caso(r"/^[a-z]+@[a-z]+\.com$/i.test('Ana@Site.COM')", "true");
    caso(r"'a1b22c333'.split(/\d+/).join('|')", "a|b|c|");
    caso("[3, 1, 10, 2].sort().join() + ' ' + [3, 1, 10, 2].sort((a, b) => a - b).join()", "1,10,2,3 1,2,3,10");
    caso("const m = new Map([[1, 'a']]); m.set('x', 2); m.size + m.get(1) + [...m.keys()].join()", "2a1,x");
    caso("new Set([1, 2, 2, 3]).size", "3");
    caso("Object.assign({}, {a: 1}, {b: 2}).b", "2");
    caso("Array.from({length: 3}, (_, i) => i * i).join()", "0,1,4");
    caso("[[1, [2]], [3]].flat(Infinity).join()", "1,2,3");
    caso("Math.round(Math.sqrt(2) * 1e6) / 1e6", "1.414214");
    caso("Math.floor(Math.sin(Math.PI / 6) * 1e10)", "4999999999");
    caso("Math.round(Math.exp(1) * 1e8)", "271828183");
    caso("parseInt('ff', 16) + parseFloat('3.5abc')", "258.5");
    caso("(255).toString(16) + (5).toString(2)", "ff101");
    caso("new Date(Date.UTC(2024, 0, 15, 10, 30)).toISOString()", "2024-01-15T10:30:00.000Z");
    caso("encodeURIComponent('a b&c=ç')", "a%20b%26c%3D%C3%A7");
    caso("'abc'.padStart(5, '*') + 'x'.repeat(3)", "**abcxxx");
    caso("`soma: ${1 + 2} e ${'ok'}`", "soma: 3 e ok");
    caso("Symbol('x').toString()", "Symbol(x)");
    caso("[...'ação'].length", "4");
    caso("Object.entries({x: 1, y: 2}).map(([k, v]) => k + v).join()", "x1,y2");
    caso("Number('  12  ') + Number('') + Number('0x10')", "28");
    caso("[1, 2, 3].includes(2) && 'abc'.includes('b')", "true");
    caso("String(Symbol.iterator in [])", "true");
}

#[test]
fn coletor_de_lixo() {
    let mut vm = Vm::novo();
    js::avaliar(&mut vm, "var guardado = {x: [1, 2, 3]}; for (let i = 0; i < 200000; i++) { let o = {i: i, a: [i]}; }").unwrap();
    let antes = vm.heap.vivos();
    let liberados = vm.coletar();
    let v = js::avaliar(&mut vm, "guardado.x.join()").unwrap();
    assert_eq!(&*vm.para_texto(&v).unwrap(), "1,2,3");
    println!("vivos antes {antes}, liberados {liberados}, vivos {}", vm.heap.vivos());
}
