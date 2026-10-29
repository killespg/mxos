//! O motor de JavaScript do MXOS, escrito do zero: lexico, parser,
//! compilador para bytecode, maquina virtual com coletor de lixo e a
//! biblioteca padrao. O DOM e ligado pelo anfitriao.

pub mod ast;
pub mod bytecode;
pub mod compilador;
pub mod dom;
pub mod embutidos;
pub mod lexico;
pub mod mat64;
pub mod parser;
pub mod regex;
pub mod valor;
pub mod vm;

use valor::Valor;
use vm::{R, Vm};

/// Compila e roda um texto JavaScript no ambiente global da VM.
pub fn avaliar(vm: &mut Vm, fonte: &str) -> R<Valor> {
    let ast = match parser::analisar(fonte) {
        Ok(a) => a,
        Err(e) => {
            let p = vm.p.erro_sintaxe;
            return Err(vm.novo_erro(p, &alloc::format!("{} (linha {})", e.msg, e.linha)));
        }
    };
    let proto = match compilador::compilar_script(&ast, fonte) {
        Ok(p) => p,
        Err(e) => {
            let p = vm.p.erro_sintaxe;
            return Err(vm.novo_erro(p, &e.0));
        }
    };
    vm.rodar(proto)
}

/// Texto de uma excecao (para mostrar no console).
pub fn descrever_erro(vm: &mut Vm, e: &Valor) -> alloc::string::String {
    vm.mostrar(e, 0)
}
