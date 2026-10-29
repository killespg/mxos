//! ext4 (e ext2/ext3), so leitura: o sistema de arquivos dos pendrives e
//! discos formatados no Linux. Monta em /mnt/<disco> (mod.rs).
//!
//!   superbloco (byte 1024) -> tamanho do bloco, grupos, inodes
//!   descritores dos grupos -> onde fica a tabela de inodes de cada grupo
//!   inode -> tamanho, tipo e onde estao os dados: arvore de extents
//!            (ext4) ou blocos diretos/indiretos (ext2/ext3)
//!   pasta -> entradas (inode, tamanho do registro, nome) uma atras da
//!            outra; pastas com indice (htree) leem igual, so mais devagar
//!
//! O diario (journal) nao e lido: um disco desmontado direito esta em dia.
//! Links simbolicos sao seguidos (ate 8 por caminho).

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use super::{Disco, Erro, ItemPasta};

const MAGICO: u16 = 0xEF53;
const RAIZ: u32 = 2;
const EXTENTS: u32 = 0x80000;
const DADOS_NO_INODE: u32 = 0x1000_0000;
const INCOMPAT_64BIT: u32 = 0x80;
const MAGICO_EXTENT: u16 = 0xF30A;
const TIPO: u16 = 0xF000;
const PASTA: u16 = 0x4000;
const LINK: u16 = 0xA000;

fn u16_em(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_em(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

pub struct Ext4 {
    disco: Box<dyn Disco>,
    bloco: usize,
    inodes_por_grupo: u32,
    tamanho_inode: usize,
    /// Onde comeca a tabela de inodes de cada grupo (em blocos).
    tabelas: Vec<u64>,
    blocos: u64,
    livres: u64,
    rotulo: String,
}

pub struct Inode {
    modo: u16,
    pub tamanho: u64,
    flags: u32,
    blocos: [u8; 60],
}

impl Inode {
    pub fn pasta(&self) -> bool {
        self.modo & TIPO == PASTA
    }

    fn link(&self) -> bool {
        self.modo & TIPO == LINK
    }
}

impl Ext4 {
    /// Le o superbloco e os grupos. Err(NaoAchado) se nao e ext2/3/4.
    pub fn montar(mut disco: Box<dyn Disco>) -> Result<Ext4, (Erro, Box<dyn Disco>)> {
        let mut sb = vec![0u8; 1024];
        if let Err(e) = disco.ler(2, &mut sb) {
            return Err((e, disco));
        }
        if u16_em(&sb, 56) != MAGICO {
            return Err((Erro::NaoAchado, disco));
        }
        let bloco = 1024usize << u32_em(&sb, 24).min(6);
        let blocos = u32_em(&sb, 4) as u64 | (u32_em(&sb, 336) as u64) << 32;
        let livres = u32_em(&sb, 12) as u64 | (u32_em(&sb, 344) as u64) << 32;
        let blocos_por_grupo = u32_em(&sb, 32).max(1) as u64;
        let inodes_por_grupo = u32_em(&sb, 40);
        let revisao = u32_em(&sb, 76);
        let tamanho_inode = if revisao == 0 { 128 } else { u16_em(&sb, 88) as usize };
        let incompat = u32_em(&sb, 96);
        let tamanho_desc = if incompat & INCOMPAT_64BIT != 0 { u16_em(&sb, 254).max(32) as usize } else { 32 };
        let primeiro = u32_em(&sb, 20) as u64;
        let rotulo_bytes = &sb[120..136];
        let fim = rotulo_bytes.iter().position(|&c| c == 0).unwrap_or(16);
        let rotulo = String::from_utf8_lossy(&rotulo_bytes[..fim]).into_owned();
        let grupos = blocos.saturating_sub(primeiro).div_ceil(blocos_por_grupo) as usize;
        if grupos == 0 || tamanho_inode < 128 || inodes_por_grupo == 0 {
            return Err((Erro::Corrompido("superbloco do ext4 estranho"), disco));
        }
        // os descritores vem no bloco depois do superbloco
        let inicio_desc = (primeiro + 1) * bloco as u64;
        let mut desc = vec![0u8; (grupos * tamanho_desc).div_ceil(512) * 512];
        if let Err(e) = disco.ler(inicio_desc / 512, &mut desc) {
            return Err((e, disco));
        }
        let tabelas = (0..grupos)
            .map(|g| {
                let d = &desc[g * tamanho_desc..];
                let alto = if tamanho_desc >= 64 { (u32_em(d, 0x28) as u64) << 32 } else { 0 };
                u32_em(d, 8) as u64 | alto
            })
            .collect();
        Ok(Ext4 { disco, bloco, inodes_por_grupo, tamanho_inode, tabelas, blocos, livres, rotulo })
    }

    pub fn desmontar(self) -> Box<dyn Disco> {
        self.disco
    }

    pub fn nome_disco(&self) -> &str {
        self.disco.nome()
    }

    pub fn rotulo(&self) -> &str {
        &self.rotulo
    }

    pub fn bytes_totais(&self) -> u64 {
        self.blocos * self.bloco as u64
    }

    pub fn bytes_livres(&self) -> u64 {
        self.livres * self.bloco as u64
    }

    fn ler_bytes(&mut self, posicao: u64, destino: &mut [u8]) -> Result<(), Erro> {
        let setor = posicao / 512;
        let antes = (posicao % 512) as usize;
        let mut tmp = vec![0u8; (antes + destino.len()).div_ceil(512) * 512];
        self.disco.ler(setor, &mut tmp)?;
        destino.copy_from_slice(&tmp[antes..antes + destino.len()]);
        Ok(())
    }

    fn ler_bloco(&mut self, numero: u64) -> Result<Vec<u8>, Erro> {
        if numero >= self.blocos {
            return Err(Erro::Corrompido("bloco fora do ext4"));
        }
        let mut b = vec![0u8; self.bloco];
        self.disco.ler(numero * (self.bloco as u64 / 512), &mut b)?;
        Ok(b)
    }

    fn inode(&mut self, numero: u32) -> Result<Inode, Erro> {
        if numero == 0 {
            return Err(Erro::Corrompido("inode 0"));
        }
        let grupo = ((numero - 1) / self.inodes_por_grupo) as usize;
        let indice = ((numero - 1) % self.inodes_por_grupo) as u64;
        let tabela = *self.tabelas.get(grupo).ok_or(Erro::Corrompido("inode fora do ext4"))?;
        let mut b = vec![0u8; 160.min(self.tamanho_inode)];
        self.ler_bytes(tabela * self.bloco as u64 + indice * self.tamanho_inode as u64, &mut b)?;
        let mut blocos = [0u8; 60];
        blocos.copy_from_slice(&b[40..100]);
        Ok(Inode {
            modo: u16_em(&b, 0),
            tamanho: u32_em(&b, 4) as u64 | (u32_em(&b, 108) as u64) << 32,
            flags: u32_em(&b, 32),
            blocos,
        })
    }

    /// Blocos fisicos de cada bloco do arquivo, em ordem (0 = buraco).
    pub fn mapa(&mut self, ino: &Inode) -> Result<Vec<u64>, Erro> {
        let quantos = ino.tamanho.div_ceil(self.bloco as u64) as usize;
        let mut mapa = vec![0u64; quantos];
        if ino.flags & EXTENTS != 0 {
            let raiz = ino.blocos;
            self.extents(&raiz, &mut mapa, 0)?;
        } else {
            let por_bloco = self.bloco / 4;
            for (k, lugar) in mapa.iter_mut().enumerate().take(12) {
                *lugar = u32_em(&ino.blocos, k * 4) as u64;
            }
            let mut base = 12;
            for nivel in 1..=3u32 {
                let raiz = u32_em(&ino.blocos, (11 + nivel as usize) * 4) as u64;
                if base >= quantos {
                    break;
                }
                if raiz != 0 {
                    self.indiretos(raiz, nivel, &mut mapa, base)?;
                }
                base += por_bloco.pow(nivel);
            }
        }
        Ok(mapa)
    }

    fn indiretos(&mut self, bloco: u64, nivel: u32, mapa: &mut [u64], base: usize) -> Result<(), Erro> {
        let por_bloco = self.bloco / 4;
        let dados = self.ler_bloco(bloco)?;
        let passo = por_bloco.pow(nivel - 1);
        for k in 0..por_bloco {
            let inicio = base + k * passo;
            if inicio >= mapa.len() {
                break;
            }
            let n = u32_em(&dados, k * 4) as u64;
            if n == 0 {
                continue;
            }
            if nivel == 1 {
                mapa[inicio] = n;
            } else {
                self.indiretos(n, nivel - 1, mapa, inicio)?;
            }
        }
        Ok(())
    }

    fn extents(&mut self, no: &[u8], mapa: &mut [u64], profundidade_max: u32) -> Result<(), Erro> {
        if u16_em(no, 0) != MAGICO_EXTENT || profundidade_max > 8 {
            return Err(Erro::Corrompido("árvore de extents estragada"));
        }
        let entradas = u16_em(no, 2) as usize;
        let profundidade = u16_em(no, 6);
        for k in 0..entradas {
            let e = &no[12 + k * 12..24 + k * 12];
            if profundidade == 0 {
                let logico = u32_em(e, 0) as usize;
                let mut tamanho = u16_em(e, 4) as usize;
                let nao_iniciado = tamanho > 32768;
                if nao_iniciado {
                    tamanho -= 32768; // reservado mas nunca escrito: le zeros
                }
                let fisico = (u16_em(e, 6) as u64) << 32 | u32_em(e, 8) as u64;
                for j in 0..tamanho {
                    if let Some(lugar) = mapa.get_mut(logico + j) {
                        *lugar = if nao_iniciado { 0 } else { fisico + j as u64 };
                    }
                }
            } else {
                let filho = (u16_em(e, 8) as u64) << 32 | u32_em(e, 4) as u64;
                let dados = self.ler_bloco(filho)?;
                self.extents(&dados, mapa, profundidade_max + 1)?;
            }
        }
        Ok(())
    }

    /// O conteudo inteiro de um inode.
    fn conteudo(&mut self, ino: &Inode) -> Result<Vec<u8>, Erro> {
        if ino.flags & DADOS_NO_INODE != 0 || (ino.link() && ino.tamanho < 60 && ino.flags & EXTENTS == 0) {
            let n = (ino.tamanho as usize).min(60);
            return Ok(ino.blocos[..n].to_vec());
        }
        let mapa = self.mapa(ino)?;
        let mut dados = vec![0u8; mapa.len() * self.bloco];
        for (k, &fisico) in mapa.iter().enumerate() {
            if fisico != 0 {
                let b = self.ler_bloco(fisico)?;
                dados[k * self.bloco..(k + 1) * self.bloco].copy_from_slice(&b);
            }
        }
        dados.truncate(ino.tamanho as usize);
        Ok(dados)
    }

    /// (nome, inode, tipo do registro) de cada entrada de uma pasta.
    fn entradas(&mut self, ino: &Inode) -> Result<Vec<(String, u32, u8)>, Erro> {
        let dados = self.conteudo(ino)?;
        let mut lista = Vec::new();
        let mut k = 0;
        while k + 8 <= dados.len() {
            let numero = u32_em(&dados, k);
            let registro = u16_em(&dados, k + 4) as usize;
            let tamanho_nome = dados[k + 6] as usize;
            if registro < 8 {
                break;
            }
            if numero != 0 && tamanho_nome > 0 && k + 8 + tamanho_nome <= dados.len() {
                let nome = String::from_utf8_lossy(&dados[k + 8..k + 8 + tamanho_nome]).into_owned();
                if nome != "." && nome != ".." {
                    lista.push((nome, numero, dados[k + 7]));
                }
            }
            k += registro;
        }
        Ok(lista)
    }

    /// O inode de um caminho (relativo a raiz deste volume), seguindo links.
    pub fn achar(&mut self, caminho: &str) -> Result<(u32, Inode), Erro> {
        self.achar_com(caminho, 0)
    }

    fn achar_com(&mut self, caminho: &str, saltos: u32) -> Result<(u32, Inode), Erro> {
        let mut atual = RAIZ;
        let mut ino = self.inode(RAIZ)?;
        let partes: Vec<&str> = caminho.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
        let mut pilha: Vec<u32> = Vec::new();
        for (k, parte) in partes.iter().enumerate() {
            if *parte == ".." {
                atual = pilha.pop().unwrap_or(RAIZ);
                ino = self.inode(atual)?;
                continue;
            }
            if !ino.pasta() {
                return Err(Erro::NaoEPasta);
            }
            let (_, numero, _) = self.entradas(&ino)?.into_iter().find(|(n, _, _)| n == parte).ok_or(Erro::NaoAchado)?;
            let proximo = self.inode(numero)?;
            if proximo.link() {
                if saltos >= 8 {
                    return Err(Erro::Corrompido("links simbólicos demais"));
                }
                let alvo = String::from_utf8_lossy(&self.conteudo(&proximo)?).into_owned();
                // o resto do caminho continua a partir do alvo do link
                let base: String = if alvo.starts_with('/') {
                    String::new()
                } else {
                    let mut b = String::new();
                    for p in &partes[..k] {
                        b.push('/');
                        b.push_str(p);
                    }
                    b
                };
                let mut novo = alloc::format!("{base}/{alvo}");
                for p in &partes[k + 1..] {
                    novo.push('/');
                    novo.push_str(p);
                }
                return self.achar_com(&novo, saltos + 1);
            }
            pilha.push(atual);
            atual = numero;
            ino = proximo;
        }
        Ok((atual, ino))
    }

    pub fn listar(&mut self, caminho: &str) -> Result<Vec<ItemPasta>, Erro> {
        let (_, ino) = self.achar(caminho)?;
        if !ino.pasta() {
            return Err(Erro::NaoEPasta);
        }
        let mut itens = Vec::new();
        for (nome, numero, tipo) in self.entradas(&ino)? {
            // tipo no registro (2 = pasta) evita ler o inode; sem ele, le
            let (pasta, tamanho) = match tipo {
                2 => (true, 0),
                1 | 3..=7 => (false, self.inode(numero).map_or(0, |i| i.tamanho)),
                _ => {
                    let i = self.inode(numero)?;
                    (i.pasta(), if i.pasta() { 0 } else { i.tamanho })
                }
            };
            itens.push(ItemPasta { nome, pasta, tamanho });
        }
        Ok(itens)
    }

    pub fn ler(&mut self, caminho: &str) -> Result<Vec<u8>, Erro> {
        let (_, ino) = self.achar(caminho)?;
        if ino.pasta() {
            return Err(Erro::EPasta);
        }
        self.conteudo(&ino)
    }

    /// Tamanho e blocos fisicos (para ler aos pedacos, como no FAT32).
    pub fn abrir_leitura(&mut self, caminho: &str) -> Result<(usize, Vec<u64>), Erro> {
        let (_, ino) = self.achar(caminho)?;
        if ino.pasta() {
            return Err(Erro::EPasta);
        }
        if ino.flags & DADOS_NO_INODE != 0 {
            return Err(Erro::Corrompido("arquivo com dados dentro do inode"));
        }
        Ok((ino.tamanho as usize, self.mapa(&ino)?))
    }

    /// Le `destino.len()` bytes a partir do byte `inicio` do arquivo cujos
    /// blocos sao `mapa`.
    pub fn ler_trecho(&mut self, mapa: &[u64], inicio: usize, destino: &mut [u8]) -> Result<(), Erro> {
        let mut feito = 0;
        while feito < destino.len() {
            let atual = inicio + feito;
            let k = atual / self.bloco;
            let dentro = atual % self.bloco;
            let n = (self.bloco - dentro).min(destino.len() - feito);
            match mapa.get(k) {
                Some(&fisico) if fisico != 0 => {
                    // blocos seguidos no disco: um pedido so
                    let mut seguidos = 1;
                    while dentro == 0
                        && feito + seguidos * self.bloco < destino.len()
                        && mapa.get(k + seguidos) == Some(&(fisico + seguidos as u64))
                        && seguidos < 64
                    {
                        seguidos += 1;
                    }
                    if seguidos > 1 {
                        let total = (seguidos * self.bloco).min(destino.len() - feito);
                        let mut b = vec![0u8; seguidos * self.bloco];
                        self.disco.ler(fisico * (self.bloco as u64 / 512), &mut b)?;
                        destino[feito..feito + total].copy_from_slice(&b[..total]);
                        feito += total;
                        continue;
                    }
                    let b = self.ler_bloco(fisico)?;
                    destino[feito..feito + n].copy_from_slice(&b[dentro..dentro + n]);
                }
                _ => destino[feito..feito + n].fill(0),
            }
            feito += n;
        }
        Ok(())
    }
}
