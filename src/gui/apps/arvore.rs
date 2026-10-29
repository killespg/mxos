//! Arvore de pastas da coluna esquerda do gerenciador de arquivos.
//!
//! Guarda so as linhas visiveis (pastas abertas em ordem), lendo as
//! subpastas do disco na hora de abrir cada uma. Clique no triangulo abre
//! ou fecha; clique no nome navega. A pasta mostrada na lista fica
//! realcada e os pais dela abrem sozinhos.

use alloc::string::String;
use alloc::vec::Vec;

use super::arquivos::{juntar, nome_do_caminho, normalizar};
use crate::armazenamento;
use crate::gui::grafico::{self, Retangulo, Superficie};
use crate::gui::tema::Tema;
use crate::gui::widgets::{cor_campo, cor_texto_fraco};

const ALTURA_LINHA: i32 = 20;
const RECUO: i32 = 14;
/// Pastas listadas por pasta (as enormes ficam cortadas).
const MAX_FILHOS: usize = 400;

struct No {
    caminho: String,
    nivel: i32,
    aberto: bool,
    /// None = ainda nao listou.
    tem_filhos: Option<bool>,
}

pub struct Arvore {
    nos: Vec<No>,
    topo: usize,
}

/// O que um clique na arvore pede.
pub enum Clique {
    Nada,
    Mudou,
    Navegar(String),
}

fn subpastas(caminho: &str) -> Vec<String> {
    let mut v: Vec<String> = armazenamento::listar(caminho)
        .map(|itens| itens.into_iter().filter(|i| i.pasta && i.nome != "." && i.nome != "..").map(|i| juntar(caminho, &i.nome)).collect())
        .unwrap_or_default();
    v.sort_by_key(|c| c.to_lowercase());
    v.truncate(MAX_FILHOS);
    v
}

impl Arvore {
    pub fn nova() -> Arvore {
        let mut a = Arvore { nos: alloc::vec![No { caminho: String::from("/"), nivel: 0, aberto: false, tem_filhos: None }], topo: 0 };
        a.abrir(0);
        a
    }

    fn abrir(&mut self, i: usize) {
        if self.nos[i].aberto {
            return;
        }
        let filhos = subpastas(&self.nos[i].caminho);
        let nivel = self.nos[i].nivel + 1;
        self.nos[i].aberto = true;
        self.nos[i].tem_filhos = Some(!filhos.is_empty());
        let novos = filhos.into_iter().map(|caminho| No { caminho, nivel, aberto: false, tem_filhos: None });
        self.nos.splice(i + 1..i + 1, novos);
    }

    fn fechar(&mut self, i: usize) {
        let nivel = self.nos[i].nivel;
        let fim = self.nos[i + 1..].iter().position(|n| n.nivel <= nivel).map_or(self.nos.len(), |p| i + 1 + p);
        self.nos.drain(i + 1..fim);
        self.nos[i].aberto = false;
    }

    /// Abre os pais de `caminho` (a pasta da lista aparece na arvore).
    pub fn mostrar(&mut self, caminho: &str, linhas: usize) {
        let caminho = normalizar(caminho);
        let mut atual = String::from("/");
        let mut i = 0;
        for parte in caminho.split('/').filter(|p| !p.is_empty()) {
            self.abrir(i);
            atual = juntar(&atual, parte);
            match self.nos.iter().position(|n| n.caminho == atual) {
                Some(k) => i = k,
                None => break,
            }
        }
        // rola ate a linha
        if i < self.topo {
            self.topo = i;
        } else if i >= self.topo + linhas {
            self.topo = i + 1 - linhas;
        }
    }

    /// Le de novo (pastas criadas ou apagadas): mantem o que estava aberto.
    pub fn atualizar(&mut self) {
        let abertos: Vec<String> = self.nos.iter().filter(|n| n.aberto).map(|n| n.caminho.clone()).collect();
        *self = Arvore { nos: alloc::vec![No { caminho: String::from("/"), nivel: 0, aberto: false, tem_filhos: None }], topo: self.topo };
        for c in abertos {
            if let Some(i) = self.nos.iter().position(|n| n.caminho == c) {
                self.abrir(i);
            }
        }
        self.topo = self.topo.min(self.nos.len().saturating_sub(1));
    }

    pub fn rolar(&mut self, linhas: isize, visiveis: usize) -> bool {
        let max = self.nos.len().saturating_sub(visiveis);
        let novo = (self.topo as isize + linhas).clamp(0, max as isize) as usize;
        let mudou = novo != self.topo;
        self.topo = novo;
        mudou
    }

    pub fn clique(&mut self, x: i32, y: i32, r: Retangulo) -> Clique {
        if !r.contem(x, y) {
            return Clique::Nada;
        }
        let i = self.topo + ((y - r.y - 2) / ALTURA_LINHA).max(0) as usize;
        let Some(no) = self.nos.get(i) else { return Clique::Nada };
        let x_seta = r.x + 4 + no.nivel * RECUO;
        if x >= x_seta - 2 && x < x_seta + 12 && no.tem_filhos != Some(false) {
            if no.aberto { self.fechar(i) } else { self.abrir(i) }
            return Clique::Mudou;
        }
        Clique::Navegar(no.caminho.clone())
    }

    pub fn linhas_visiveis(r: Retangulo) -> usize {
        ((r.altura - 4) / ALTURA_LINHA).max(1) as usize
    }

    pub fn desenhar(&self, s: &mut Superficie, r: Retangulo, atual: Option<&str>, t: &Tema) {
        let papel = cor_campo(t);
        s.retangulo(r, papel);
        s.retangulo(Retangulo::novo(r.direita() - 1, r.y, 1, r.altura), grafico::misturar(papel, t.texto, 30));
        for (k, no) in self.nos.iter().skip(self.topo).take(Self::linhas_visiveis(r)).enumerate() {
            let y = r.y + 2 + k as i32 * ALTURA_LINHA;
            if atual == Some(no.caminho.as_str()) {
                s.retangulo(Retangulo::novo(r.x, y, r.largura - 1, ALTURA_LINHA), grafico::misturar(papel, t.destaque, 40));
            }
            let x = r.x + 4 + no.nivel * RECUO;
            let seta = match (no.aberto, no.tem_filhos) {
                (_, Some(false)) => " ",
                (true, _) => "-",
                (false, _) => "+",
            };
            s.texto(x, y + 6, seta, cor_texto_fraco(t), 1);
            let nome = if no.caminho == "/" { "/ (disco)" } else { nome_do_caminho(&no.caminho) };
            let max = r.direita() - (x + 14) - 4;
            s.texto(x + 14, y + 6, crate::fonte::cortar(nome, max, &crate::fonte::INTER), t.texto, 1);
        }
    }
}
