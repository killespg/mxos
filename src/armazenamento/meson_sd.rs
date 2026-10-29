//! Cartao SD e eMMC da TV box (controlador SD_EMMC da Amlogic GX, o mesmo
//! do driver meson-gx-mmc do Linux). NUNCA TESTADO NO HARDWARE.
//!
//!   SD_EMMC_B (0xD0072000): o slot do cartao SD
//!   SD_EMMC_C (0xD0074000): a eMMC interna (onde mora o Android)
//!
//! O U-Boot ja ligou o relogio e os pinos (ele leu o MXOS do cartao). Aqui:
//! relogio de 400 kHz, a sequencia de partida do cartao (CMD0, CMD8,
//! ACMD41 ou CMD1, CMD2, CMD3, CSD/EXT_CSD, CMD7), barramento de 4 bits e
//! 24 MHz, e leitura/escrita de blocos de 512 bytes por DMA direto (sem
//! cadeia de descritores: um comando por vez, como o "CMD_ARG dispara").
//!
//! A eMMC fica so de leitura, menos nas faixas liberadas pelo instalador
//! (amlogic.rs: a particao de dados e a boot). O comeco dela, com o U-Boot
//! e a tabela de particoes, nunca e escrito.

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};

use super::{Disco, Erro};
use crate::{cpu, mmu};

const SD_EMMC_B: usize = 0xD007_2000;
const SD_EMMC_C: usize = 0xD007_4000;

const CLOCK: usize = 0x00;
const CFG: usize = 0x44;
const STATUS: usize = 0x48;
const IRQ_EN: usize = 0x4C;
const CMD_CFG: usize = 0x50;
const CMD_ARG: usize = 0x54;
const CMD_DAT: usize = 0x58;
const CMD_RSP: usize = 0x5C;

// CLOCK
const CLK_ALWAYS_ON: u32 = 1 << 24;
const CLK_FASE_CORE: u32 = 2 << 8; // 180 graus, como o Linux
// CFG
const CFG_BUS_1BIT: u32 = 0;
const CFG_BUS_4BIT: u32 = 1;
const CFG_BL_LEN_512: u32 = 9 << 4;
const CFG_RESP_TIMEOUT: u32 = 7 << 8;
const CFG_RC_CC: u32 = 4 << 12;
const CFG_ERR_ABORT: u32 = 1 << 27;
// STATUS
const STATUS_ERROS: u32 = 0x1FFF;
const STATUS_TIMEOUT_RESP: u32 = 1 << 11;
const STATUS_FIM: u32 = 1 << 13;
const STATUS_OCUPADO: u32 = 1 << 31;
// CMD_CFG
const CMD_BLOCOS: u32 = 1 << 9;
const CMD_R1B: u32 = 1 << 10;
const CMD_FIM_DA_CADEIA: u32 = 1 << 11;
const CMD_TIMEOUT: u32 = 12 << 12;
const CMD_SEM_RESPOSTA: u32 = 1 << 16;
const CMD_DADOS: u32 = 1 << 18;
const CMD_ESCRITA: u32 = 1 << 19;
const CMD_RESP_SEM_CRC: u32 = 1 << 20;
const CMD_RESP_128: u32 = 1 << 21;
const CMD_RESP_NUM: u32 = 1 << 22;
const CMD_DONO: u32 = 1 << 31;

/// Faixas de setores da eMMC onde pode escrever: (inicio, fim).
struct Liberadas(core::cell::UnsafeCell<Vec<(u64, u64)>>);

// So o laco principal mexe (com a trava do kernel).
unsafe impl Sync for Liberadas {}

static LIBERADAS: Liberadas = Liberadas(core::cell::UnsafeCell::new(Vec::new()));

/// Libera a escrita em [inicio, fim) da eMMC (setores).
pub fn liberar_escrita(inicio: u64, fim: u64) {
    unsafe { (*LIBERADAS.0.get()).push((inicio, fim)) };
}

fn liberado(inicio: u64, fim: u64) -> bool {
    unsafe { (*LIBERADAS.0.get()).iter().any(|&(a, b)| inicio >= a && fim <= b) }
}

/// Blocos por comando (CMD18/CMD25), ate 255 pelo campo LENGTH.
const MAX_BLOCOS: usize = 128;

#[derive(Clone, Copy, PartialEq)]
enum Resposta {
    Nenhuma,
    Curta,
    CurtaSemCrc,
    Ocupado,
    Longa,
}

pub struct MesonSd {
    base: usize,
    nome: &'static str,
    setores: u64,
    rca: u32,
    /// Cartao SDHC/SDXC/eMMC grande: endereco em blocos (senao, em bytes).
    por_bloco: bool,
    so_leitura: bool,
}

fn ler(base: usize, r: usize) -> u32 {
    unsafe { read_volatile((base + r) as *const u32) }
}

fn escrever(base: usize, r: usize, v: u32) {
    unsafe { write_volatile((base + r) as *mut u32, v) }
}

fn esperar_us(us: u64) {
    let alvo = cpu::contador() + cpu::frequencia_timer() * us / 1_000_000;
    while cpu::contador() < alvo {
        core::hint::spin_loop();
    }
}

/// Os discos achados: o cartao SD e a eMMC (os que responderem).
pub fn achar() -> Vec<Box<dyn Disco>> {
    let mut discos: Vec<Box<dyn Disco>> = Vec::new();
    if let Ok(sd) = MesonSd::iniciar(SD_EMMC_B, "sd", false) {
        discos.push(Box::new(sd));
    }
    if let Ok(emmc) = MesonSd::iniciar(SD_EMMC_C, "emmc", true) {
        discos.push(Box::new(emmc));
    }
    discos
}

impl MesonSd {
    fn iniciar(base: usize, nome: &'static str, emmc: bool) -> Result<MesonSd, Erro> {
        let mut d = MesonSd { base, nome, setores: 0, rca: 0, por_bloco: false, so_leitura: emmc };
        // 24 MHz do cristal / 60 = 400 kHz para a partida
        d.relogio(60);
        escrever(base, CFG, CFG_BUS_1BIT | CFG_BL_LEN_512 | CFG_RESP_TIMEOUT | CFG_RC_CC | CFG_ERR_ABORT);
        escrever(base, IRQ_EN, 0);
        escrever(base, STATUS, 0xFFFF_FFFF);
        esperar_us(1000);
        d.comando(0, 0, Resposta::Nenhuma, None)?;
        esperar_us(1000);
        if emmc { d.partida_emmc()? } else { d.partida_sd()? }
        // barramento de 4 bits e relogio de 24 MHz
        if emmc {
            // SWITCH: BUS_WIDTH (byte 183) = 1 (4 bits)
            d.comando(6, 0x03B7_0100, Resposta::Ocupado, None)?;
        } else {
            d.comando(55, d.rca << 16, Resposta::Curta, None)?;
            d.comando(6, 2, Resposta::Curta, None)?;
        }
        escrever(base, CFG, CFG_BUS_4BIT | CFG_BL_LEN_512 | CFG_RESP_TIMEOUT | CFG_RC_CC | CFG_ERR_ABORT);
        d.relogio(1);
        d.comando(16, 512, Resposta::Curta, None)?;
        Ok(d)
    }

    fn relogio(&self, divisor: u32) {
        escrever(self.base, CLOCK, CLK_ALWAYS_ON | CLK_FASE_CORE | (divisor & 0x3F));
        esperar_us(200);
    }

    fn partida_sd(&mut self) -> Result<(), Erro> {
        // CMD8: tensao 2,7-3,6 V e o padrao 0xAA de volta (cartoes v2)
        let v2 = self.comando(8, 0x1AA, Resposta::Curta, None).is_ok_and(|r| r[0] & 0xFFF == 0x1AA);
        let pedido = 0x00FF_8000 | if v2 { 1 << 30 } else { 0 };
        let mut ocr = 0;
        for _ in 0..200 {
            self.comando(55, 0, Resposta::Curta, None)?;
            ocr = self.comando(41, pedido, Resposta::CurtaSemCrc, None)?[0];
            if ocr & (1 << 31) != 0 {
                break;
            }
            esperar_us(10_000);
        }
        if ocr & (1 << 31) == 0 {
            return Err(Erro::Disco);
        }
        self.por_bloco = ocr & (1 << 30) != 0;
        self.comando(2, 0, Resposta::Longa, None)?;
        self.rca = self.comando(3, 0, Resposta::Curta, None)?[0] >> 16;
        let csd = self.comando(9, self.rca << 16, Resposta::Longa, None)?;
        self.setores = setores_do_csd(&csd);
        self.comando(7, self.rca << 16, Resposta::Ocupado, None)?;
        Ok(())
    }

    fn partida_emmc(&mut self) -> Result<(), Erro> {
        let mut ocr = 0;
        for _ in 0..200 {
            ocr = self.comando(1, 0x40FF_8080, Resposta::CurtaSemCrc, None)?[0];
            if ocr & (1 << 31) != 0 {
                break;
            }
            esperar_us(10_000);
        }
        if ocr & (1 << 31) == 0 {
            return Err(Erro::Disco);
        }
        self.por_bloco = ocr & (0x3 << 29) == 2 << 29;
        self.comando(2, 0, Resposta::Longa, None)?;
        self.rca = 1;
        self.comando(3, self.rca << 16, Resposta::Curta, None)?;
        let csd = self.comando(9, self.rca << 16, Resposta::Longa, None)?;
        self.setores = setores_do_csd(&csd);
        self.comando(7, self.rca << 16, Resposta::Ocupado, None)?;
        if self.por_bloco {
            // eMMC grande: o tamanho de verdade esta no EXT_CSD (SEC_COUNT)
            let mut ext = vec![0u8; 512];
            self.comando(8, 0, Resposta::Curta, Some((&mut ext, 1, false)))?;
            self.setores = u32::from_le_bytes([ext[212], ext[213], ext[214], ext[215]]) as u64;
        }
        Ok(())
    }

    /// Manda um comando e espera. `dados`: (buffer, blocos, escrita).
    fn comando(&mut self, indice: u32, argumento: u32, resposta: Resposta, dados: Option<(&mut [u8], usize, bool)>) -> Result<[u32; 4], Erro> {
        let base = self.base;
        let mut cfg = CMD_DONO | CMD_FIM_DA_CADEIA | CMD_TIMEOUT | (indice & 0x3F) << 24;
        cfg |= match resposta {
            Resposta::Nenhuma => CMD_SEM_RESPOSTA,
            Resposta::Curta => CMD_RESP_NUM,
            Resposta::CurtaSemCrc => CMD_RESP_NUM | CMD_RESP_SEM_CRC,
            Resposta::Ocupado => CMD_RESP_NUM | CMD_R1B,
            Resposta::Longa => CMD_RESP_NUM | CMD_RESP_128,
        };
        let mut endereco = 0;
        let mut tamanho = 0;
        if let Some((buffer, blocos, escrita)) = &dados {
            cfg |= CMD_DADOS | CMD_BLOCOS | (*blocos as u32 & 0x1FF);
            if *escrita {
                cfg |= CMD_ESCRITA;
            }
            endereco = buffer.as_ptr() as usize;
            tamanho = buffer.len();
            // o controlador le/escreve a RAM sem passar pelo cache
            mmu::limpar_cache(endereco, tamanho);
        }
        escrever(base, STATUS, 0xFFFF_FFFF);
        escrever(base, CMD_CFG, cfg);
        escrever(base, CMD_DAT, endereco as u32);
        // escrever o argumento dispara o comando
        escrever(base, CMD_ARG, argumento);
        let prazo = cpu::contador() + cpu::frequencia_timer() * 2;
        let status = loop {
            let s = ler(base, STATUS);
            if s & (STATUS_FIM | STATUS_ERROS) != 0 && s & STATUS_OCUPADO == 0 {
                break s;
            }
            if cpu::contador() > prazo {
                return Err(Erro::Disco);
            }
            core::hint::spin_loop();
        };
        escrever(base, STATUS, 0xFFFF_FFFF);
        if tamanho > 0 {
            // o que o DMA escreveu: tira as linhas velhas do cache
            mmu::limpar_cache(endereco, tamanho);
        }
        // R3 (OCR) nao tem CRC: so o timeout conta como erro
        let erros = status & STATUS_ERROS;
        let ignorar = resposta == Resposta::CurtaSemCrc && erros & STATUS_TIMEOUT_RESP == 0;
        if erros != 0 && !ignorar {
            return Err(Erro::Disco);
        }
        Ok([ler(base, CMD_RSP), ler(base, CMD_RSP + 4), ler(base, CMD_RSP + 8), ler(base, CMD_RSP + 12)])
    }

    fn endereco(&self, setor: u64) -> u32 {
        (if self.por_bloco { setor } else { setor * 512 }) as u32
    }
}

/// Tamanho pelo CSD (versao 1: C_SIZE e MULT; versao 2: C_SIZE em 512 KB).
/// A resposta longa vem com o bit 127 no topo de r[3]... no controlador da
/// Amlogic r[0] tem os bits 31-0, r[3] os 127-96.
fn setores_do_csd(r: &[u32; 4]) -> u64 {
    let bits = |alto: u32, baixo: u32| -> u64 {
        let mut v = 0u64;
        for b in (baixo..=alto).rev() {
            let palavra = r[(b / 32) as usize];
            v = v << 1 | (palavra >> (b % 32) & 1) as u64;
        }
        v
    };
    match bits(127, 126) {
        1 => (bits(69, 48) + 1) * 1024,
        _ => {
            let c_size = bits(73, 62);
            let mult = bits(49, 47);
            let bloco = bits(83, 80);
            ((c_size + 1) << (mult + 2)) << bloco >> 9
        }
    }
}

impl Disco for MesonSd {
    fn nome(&self) -> &str {
        self.nome
    }

    fn setores(&self) -> u64 {
        self.setores
    }

    fn ler(&mut self, setor: u64, buffer: &mut [u8]) -> Result<(), Erro> {
        let mut feito = 0;
        while feito < buffer.len() {
            let blocos = ((buffer.len() - feito) / 512).min(MAX_BLOCOS);
            let atual = setor + (feito / 512) as u64;
            // buffer alinhado para o DMA (o do chamador pode nao estar)
            let mut tmp = vec![0u8; blocos * 512];
            let arg = self.endereco(atual);
            if blocos == 1 {
                self.comando(17, arg, Resposta::Curta, Some((&mut tmp, 1, false)))?;
            } else {
                self.comando(18, arg, Resposta::Curta, Some((&mut tmp, blocos, false)))?;
                self.comando(12, 0, Resposta::Ocupado, None)?;
            }
            buffer[feito..feito + blocos * 512].copy_from_slice(&tmp);
            feito += blocos * 512;
        }
        Ok(())
    }

    fn escrever(&mut self, setor: u64, dados: &[u8]) -> Result<(), Erro> {
        if self.so_leitura && !liberado(setor, setor + (dados.len() / 512) as u64) {
            return Err(Erro::SoLeitura);
        }
        let mut feito = 0;
        while feito < dados.len() {
            let blocos = ((dados.len() - feito) / 512).min(MAX_BLOCOS);
            let atual = setor + (feito / 512) as u64;
            let mut tmp = dados[feito..feito + blocos * 512].to_vec();
            let arg = self.endereco(atual);
            if blocos == 1 {
                self.comando(24, arg, Resposta::Curta, Some((&mut tmp, 1, true)))?;
            } else {
                self.comando(25, arg, Resposta::Curta, Some((&mut tmp, blocos, true)))?;
                self.comando(12, 0, Resposta::Ocupado, None)?;
            }
            // espera o cartao terminar de gravar (CMD13 ate "pronto")
            for _ in 0..10_000 {
                let r = self.comando(13, self.rca << 16, Resposta::Curta, None)?;
                if r[0] & (1 << 8) != 0 && (r[0] >> 9) & 0xF == 4 {
                    break;
                }
                esperar_us(100);
            }
            feito += blocos * 512;
        }
        Ok(())
    }
}
