//! A interface grafica do sistema para os apps: os mesmos arquivos do
//! src/gui (desenho, temas, widgets, realce de sintaxe, barras) e, no
//! lugar das partes do gerenciador de janelas, versoes por cima da ABI.

#[allow(dead_code)]
#[path = "../../../src/gui/grafico.rs"]
pub mod grafico;

#[allow(dead_code)]
#[path = "../../../src/gui/tema.rs"]
pub mod tema;

#[allow(dead_code)]
#[path = "../../../src/gui/widgets.rs"]
pub mod widgets;

#[allow(dead_code)]
#[path = "../../../src/gui/realce.rs"]
pub mod realce;

#[allow(dead_code)]
#[path = "../../../src/gui/barra.rs"]
pub mod barra;

pub mod area;
pub mod janelas;
pub mod transferencia;
