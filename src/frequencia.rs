//! DVFS e overclock da TV box RK322x (R329Q): frequencia e tensao da CPU,
//! perfis, sensor de temperatura (TSADC) com throttling, e o clock da GPU
//! Mali-400 e da VPU.
//!
//! Enderecos e valores conferidos na box real (Android do fabricante, adb,
//! so leitura; ver clk-rk3228.c e rk322x.dtsi do Linux):
//! - CRU em 0x110E0000 (0x11000000 e a GRF). APLL em +0x00, GPLL em +0x24:
//!   CON0 = postdiv1[14:12] fbdiv[11:0], CON1 = dsmpd[12] trava[10]
//!   postdiv2[8:6] refdiv[5:0]; refdiv = postdiv = 1 da f = 24 MHz * fbdiv.
//!   MODE_CON (+0x40) bit 0: APLL normal (1) ou cristal de 24 MHz (0).
//!   O nucleo sai do APLL sem divisor; CLKSEL1 (aclk /2, peri /8) serve ate
//!   1608 MHz e nao muda.
//! - Reguladores por PWM em 0x110B0000: vdd_arm no pwm1 (0,95 a 1,40 V),
//!   vdd_log no pwm2 (1,00 a 1,35 V). duty = periodo * piso((Vmax - V) * 100
//!   / (Vmax - Vmin)) / 100: 1,300 V -> 82 de 375 e 1,175 V -> 187 de 375,
//!   os dois lidos. Duty 0 e o teto do regulador: 1,40 V e o maximo fisico.
//! - TSADC em 0x11150000: canal 0 e a CPU, codigo crescente com a
//!   temperatura (724 = 92 graus, igual ao que o Android mostrava).
//! - GPU: CLKSEL34 mux[6:5] (1 = GPLL) div[4:0]; VPU: CLKSEL32 e o
//!   decodificador: CLKSEL28 mux[7:6] div[4:0], todos do GPLL de 1200 MHz.
//!
//! Nada roda sozinho: o throttling e uma leitura de registrador por segundo
//! feita pelo laco principal (`vigiar`), e as trocas so quando pedidas.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use alloc::format;
use alloc::string::String;

/// Um perfil pronto: frequencia e tensao da CPU.
#[derive(Clone, Copy)]
pub struct Perfil {
    pub nome: &'static str,
    pub mhz: u32,
    pub uv: u32,
    pub nota: &'static str,
}

/// Do mais frio ao mais rapido. Turbo e o limite recomendado; Extreme ja
/// esta no teto do regulador.
pub const PERFIS: &[Perfil] = &[
    Perfil { nome: "stock", mhz: 1008, uv: 1_200_000, nota: "1,0 GHz, frio e econômico" },
    Perfil { nome: "fabrica", mhz: 1200, uv: 1_300_000, nota: "o máximo do Android" },
    Perfil { nome: "turbo", mhz: 1296, uv: 1_350_000, nota: "overclock recomendado (limite seguro)" },
    Perfil { nome: "extreme", mhz: 1464, uv: 1_400_000, nota: "teto do regulador; precisa de dissipador" },
];

/// Perfil para onde o throttling volta quando esquenta.
pub const FRIO: Perfil = PERFIS[0];
/// Acima disto e overclock; acima de RECOMENDADO_MHZ e por conta do dono.
pub const FABRICA_MHZ: u32 = 1200;
pub const RECOMENDADO_MHZ: u32 = 1296;
/// Faixa aceita no perfil personalizado (multiplos de 24 MHz).
pub const MIN_MHZ: u32 = 408;
pub const MAX_MHZ: u32 = 1608;
pub const UV_MIN: u32 = 950_000;
pub const UV_MAX: u32 = 1_400_000;
/// Throttling: desce em LIMITE, volta quando esfria 10 graus.
pub const LIMITE_PADRAO_C: i32 = 85;
const HISTERESE_C: i32 = 10;

/// Tabela de tensao (pares MHz, uV) do device tree, com a folga de fuga
/// que o kernel deles aplica; usada no personalizado sem tensao.
const TABELA: &[(u32, u32)] = &[
    (600, 1_000_000),
    (816, 1_025_000),
    (1008, 1_200_000),
    (1200, 1_300_000),
    (1296, 1_350_000),
    (1392, 1_400_000),
];

/// Tensao sugerida para `mhz`: a do primeiro degrau da tabela que cobre.
pub fn tensao_para(mhz: u32) -> u32 {
    TABELA.iter().find(|&&(f, _)| f >= mhz).map_or(UV_MAX, |&(_, uv)| uv)
}

// ---------------------------------------------------------------- hardware

/// Regulador por PWM: base do canal e faixa de tensao.
#[cfg_attr(not(feature = "rk322x"), allow(dead_code))]
#[derive(Clone, Copy)]
pub struct Regulador {
    base: usize,
    min: u32,
    max: u32,
}

pub const VDD_ARM: Regulador = Regulador { base: 0x110B_0010, min: 950_000, max: 1_400_000 };
pub const VDD_LOG: Regulador = Regulador { base: 0x110B_0020, min: 1_000_000, max: 1_350_000 };

#[cfg(feature = "rk322x")]
mod hw {
    use core::ptr::{read_volatile, write_volatile};

    use super::Regulador;

    const CRU: usize = 0x110E_0000;
    const APLL: usize = CRU;
    const GPLL: usize = CRU + 0x24;
    const MODO: usize = CRU + 0x40;
    const CLKSEL: usize = CRU + 0x44;
    const TRAVOU: u32 = 1 << 10;

    const TSADC: usize = 0x1115_0000;
    const AUTO_CON: usize = TSADC + 0x04;
    const INT_EN: usize = TSADC + 0x08;
    const DADO0: usize = TSADC + 0x20;
    const DESLIGA0: usize = TSADC + 0x40;
    const ESPERA_INT: usize = TSADC + 0x60;
    const ESPERA_DESLIGA: usize = TSADC + 0x64;
    const PERIODO: usize = TSADC + 0x68;
    const PERIODO_QUENTE: usize = TSADC + 0x6C;
    /// Os valores do kernel do fabricante: automatico no canal 0.
    const AUTO_FABRICANTE: u32 = 0x0001_1012;
    /// 120 graus: a CRU reinicia a placa (protecao de hardware).
    const CODIGO_DESLIGA: u32 = 754;

    fn ler(r: usize) -> u32 {
        unsafe { read_volatile(r as *const u32) }
    }

    fn escrever(r: usize, v: u32) {
        unsafe { write_volatile(r as *mut u32, v) }
    }

    /// Registradores da CRU: os 16 bits de cima dizem quais bits de baixo mudam.
    fn campo(valor: u32, mascara: u32, desvio: u32) -> u32 {
        (mascara << desvio) << 16 | (valor & mascara) << desvio
    }

    fn pll_mhz(base: usize) -> u32 {
        let (c0, c1) = (ler(base), ler(base + 4));
        let (fb, pd1) = (c0 & 0xFFF, (c0 >> 12 & 7).max(1));
        let (refdiv, pd2) = ((c1 & 0x3F).max(1), (c1 >> 6 & 7).max(1));
        24 * fb / (refdiv * pd1 * pd2)
    }

    pub fn mhz_cpu() -> u32 {
        if ler(MODO) & 1 == 0 { 24 } else { pll_mhz(APLL) }
    }

    pub fn gpll_mhz() -> u32 {
        pll_mhz(GPLL)
    }

    /// Troca segura do APLL: cristal, novos divisores, trava, normal.
    pub fn trocar_pll_cpu(mhz: u32) {
        let fb = mhz / 24;
        // guarda e volta o estado: o throttling chama de dentro da IRQ do timer
        let estado = crate::cpu::guardar_e_desligar_irq();
        escrever(MODO, campo(0, 1, 0));
        escrever(APLL, campo(1, 7, 12) | campo(fb, 0xFFF, 0));
        escrever(APLL + 4, campo(1, 1, 12) | campo(1, 7, 6) | campo(1, 0x3F, 0));
        escrever(APLL + 8, 0);
        let mut voltas = 0u32;
        while ler(APLL + 4) & TRAVOU == 0 && voltas < 2_000_000 {
            voltas += 1;
            core::hint::spin_loop();
        }
        escrever(MODO, campo(1, 1, 0));
        crate::cpu::restaurar_irq(estado);
    }

    /// O carregador deixou o PWM como o do fabricante (ligado, continuo,
    /// periodo de uns 5 us)? Se nao, nao mexemos na tensao.
    pub fn regulador_ok(r: Regulador) -> bool {
        ler(r.base + 0x0C) & 0x7 == 0x3 && (100..=2000).contains(&ler(r.base + 0x04))
    }

    pub fn tensao(r: Regulador) -> u32 {
        let (periodo, duty) = (ler(r.base + 0x04).max(1), ler(r.base + 0x08));
        r.max - (r.max - r.min) * duty.min(periodo) / periodo
    }

    pub fn por_tensao(r: Regulador, uv: u32) {
        let periodo = ler(r.base + 0x04);
        let uv = uv.clamp(r.min, r.max);
        let porcento = (r.max - uv) * 100 / (r.max - r.min);
        escrever(r.base + 0x08, periodo * porcento / 100);
    }

    /// Liga o TSADC como o kernel do fabricante (se o carregador nao ligou).
    /// O desligamento de hardware so arma depois de uma leitura sa.
    pub fn ligar_sensor() -> bool {
        if ler(AUTO_CON) & 1 == 0 {
            // 24 MHz / 733 = 32,7 kHz
            escrever(CLKSEL + 24 * 4, campo(732, 0x3FF, 6));
            escrever(PERIODO, 1000);
            escrever(PERIODO_QUENTE, 100);
            escrever(ESPERA_INT, 10);
            escrever(ESPERA_DESLIGA, 10);
            escrever(DESLIGA0, CODIGO_DESLIGA);
            escrever(AUTO_CON, AUTO_FABRICANTE);
            escrever(AUTO_CON, AUTO_FABRICANTE | 1);
        }
        // a primeira conversao leva uns 30 ms
        let fim = crate::cpu::contador() + crate::cpu::frequencia_timer() / 5;
        while crate::cpu::contador() < fim {
            if codigo_sensor().is_some() {
                escrever(INT_EN, ler(INT_EN) | 1 << 8);
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }

    pub fn codigo_sensor() -> Option<u32> {
        let c = ler(DADO0) & 0xFFF;
        (560..CODIGO_DESLIGA).contains(&c).then_some(c)
    }

    /// GPU e VPU (e o decodificador) do GPLL com os divisores dados.
    pub fn clock_gpu_vpu(div_gpu: u32, div_vpu: u32) {
        escrever(CLKSEL + 34 * 4, campo(1, 3, 5) | campo(div_gpu - 1, 0x1F, 0));
        escrever(CLKSEL + 32 * 4, campo(1, 3, 5) | campo(div_vpu - 1, 0x1F, 0));
        escrever(CLKSEL + 28 * 4, campo(1, 3, 6) | campo(div_vpu - 1, 0x1F, 0));
    }
}

#[cfg(not(feature = "rk322x"))]
mod hw {
    use super::Regulador;

    pub fn mhz_cpu() -> u32 {
        0
    }
    pub fn gpll_mhz() -> u32 {
        0
    }
    pub fn trocar_pll_cpu(_: u32) {}
    pub fn regulador_ok(_: Regulador) -> bool {
        false
    }
    pub fn tensao(_: Regulador) -> u32 {
        0
    }
    pub fn por_tensao(_: Regulador, _: u32) {}
    pub fn ligar_sensor() -> bool {
        false
    }
    pub fn codigo_sensor() -> Option<u32> {
        None
    }
    pub fn clock_gpu_vpu(_: u32, _: u32) {}
}

/// Da para trocar a frequencia nesta plataforma?
pub const fn suportado() -> bool {
    cfg!(feature = "rk322x")
}

pub fn mhz() -> u32 {
    hw::mhz_cpu()
}

/// Tensao atual da CPU, lida do PWM (None sem o regulador).
pub fn microvolts() -> Option<u32> {
    hw::regulador_ok(VDD_ARM).then(|| hw::tensao(VDD_ARM))
}

// ---------------------------------------------------------------- estado

/// Frequencia em que o carregador deixou a CPU: sem regulador, nunca passa dela.
static MHZ_BOOT: AtomicU32 = AtomicU32::new(0);
static UV_BOOT: AtomicU32 = AtomicU32::new(0);
/// O que o dono escolheu (0 = nada: fica como o carregador deixou).
static ALVO_MHZ: AtomicU32 = AtomicU32::new(0);
static ALVO_UV: AtomicU32 = AtomicU32::new(0);
static LIMITE_C: AtomicU32 = AtomicU32::new(LIMITE_PADRAO_C as u32);
/// Esquentou e o throttling desceu para o Stock.
static LIMITADO: AtomicBool = AtomicBool::new(false);
static SENSOR: AtomicBool = AtomicBool::new(false);
static ULTIMA_OLHADA: AtomicU64 = AtomicU64::new(0);
static GPU_MHZ: AtomicU32 = AtomicU32::new(0);

fn mhz_boot() -> u32 {
    let m = MHZ_BOOT.load(Ordering::Relaxed);
    if m != 0 {
        return m;
    }
    let m = mhz();
    UV_BOOT.store(microvolts().unwrap_or(0), Ordering::Relaxed);
    MHZ_BOOT.store(m, Ordering::Relaxed);
    m
}

/// Codigos do TSADC do rk3228 de -40 a 120 graus, de 5 em 5.
const CODIGOS: [u32; 33] = [
    588, 593, 598, 603, 608, 613, 618, 623, 629, 634, 639, 644, 649, 654, 660, 665, 670, 675, 681, 686, 691, 696, 702,
    707, 712, 717, 723, 728, 733, 738, 744, 749, 754,
];

/// Temperatura da CPU em decimos de grau, interpolada na tabela.
pub fn temperatura_dc() -> Option<i32> {
    if !SENSOR.load(Ordering::Relaxed) {
        return None;
    }
    let c = hw::codigo_sensor()?;
    let i = CODIGOS.iter().position(|&k| k >= c).unwrap_or(CODIGOS.len() - 1).max(1);
    let (a, b) = (CODIGOS[i - 1] as i32, CODIGOS[i] as i32);
    let base = -400 + (i as i32 - 1) * 50;
    Some(base + ((c as i32 - a) * 50 / (b - a).max(1)).clamp(0, 50))
}

pub fn limite_c() -> i32 {
    LIMITE_C.load(Ordering::Relaxed) as i32
}

pub fn limitado() -> bool {
    LIMITADO.load(Ordering::Relaxed)
}

/// Perfil pelo nome (sem acento ou com).
pub fn perfil(nome: &str) -> Option<Perfil> {
    let nome = if nome == "fábrica" { "fabrica" } else { nome };
    PERFIS.iter().copied().find(|p| p.nome == nome)
}

/// Nome do que esta escolhido: um perfil, "personalizado" ou "do boot".
pub fn escolhido() -> &'static str {
    let (m, uv) = (ALVO_MHZ.load(Ordering::Relaxed), ALVO_UV.load(Ordering::Relaxed));
    if m == 0 {
        return "do boot";
    }
    PERFIS.iter().find(|p| p.mhz == m && p.uv == uv).map_or("personalizado", |p| p.nome)
}

// ---------------------------------------------------------------- troca

/// Troca agora. Sobe a tensao antes da frequencia e desce depois.
fn trocar(mhz: u32, uv: u32) -> Result<(), &'static str> {
    let antes = self::mhz();
    let com_tensao = hw::regulador_ok(VDD_ARM);
    if !com_tensao && mhz > mhz_boot() {
        return Err("o carregador não ligou o PWM do vdd_arm: só dá para descer a frequência");
    }
    if mhz >= antes {
        if com_tensao {
            hw::por_tensao(VDD_ARM, uv);
            esperar_us(100);
        }
        hw::trocar_pll_cpu(mhz);
    } else {
        hw::trocar_pll_cpu(mhz);
        if com_tensao {
            hw::por_tensao(VDD_ARM, uv);
        }
    }
    Ok(())
}

fn esperar_us(us: u64) {
    let fim = crate::cpu::contador() + crate::cpu::frequencia_timer() * us / 1_000_000;
    while crate::cpu::contador() < fim {
        core::hint::spin_loop();
    }
}

/// Escolhe frequencia e tensao (perfil ou personalizado). Fora da tabela
/// recomendada vale, mas o chamador avisa. Esquentando, o throttling manda.
pub fn definir(mhz: u32, uv: Option<u32>) -> Result<(), &'static str> {
    if !suportado() {
        return Err("só na TV box RK322x");
    }
    if !(MIN_MHZ..=MAX_MHZ).contains(&mhz) {
        return Err("frequência fora de 408 a 1608 MHz");
    }
    let mhz = mhz / 24 * 24;
    let uv = uv.unwrap_or_else(|| tensao_para(mhz));
    if !(UV_MIN..=UV_MAX).contains(&uv) {
        return Err("tensão fora de 950 a 1400 mV (limite do regulador)");
    }
    mhz_boot();
    ALVO_MHZ.store(mhz, Ordering::Relaxed);
    ALVO_UV.store(uv, Ordering::Relaxed);
    let quente = temperatura_dc().is_some_and(|t| t >= limite_c() * 10);
    if quente && mhz > FRIO.mhz {
        LIMITADO.store(true, Ordering::Relaxed);
        return trocar(FRIO.mhz, FRIO.uv);
    }
    LIMITADO.store(false, Ordering::Relaxed);
    trocar(mhz, uv)
}

pub fn definir_limite(graus: i32) -> Result<(), &'static str> {
    if !(60..=110).contains(&graus) {
        return Err("limite fora de 60 a 110 graus");
    }
    LIMITE_C.store(graus as u32, Ordering::Relaxed);
    Ok(())
}

/// Uma vez por segundo, da interrupcao do timer (anda mesmo com o laco
/// principal preso numa pagina pesada): passou do limite desce para o
/// Stock; esfriou 10 graus volta ao escolhido.
pub fn vigiar() {
    if !SENSOR.load(Ordering::Relaxed) {
        return;
    }
    let agora = crate::timer::tiques();
    if agora < ULTIMA_OLHADA.load(Ordering::Relaxed) + crate::timer::HZ {
        return;
    }
    ULTIMA_OLHADA.store(agora, Ordering::Relaxed);
    let Some(t) = temperatura_dc() else { return };
    let limite = limite_c() * 10;
    if !limitado() && t >= limite && mhz() > FRIO.mhz {
        if trocar(FRIO.mhz, FRIO.uv).is_ok() {
            LIMITADO.store(true, Ordering::Relaxed);
        }
    } else if limitado() && t <= limite - HISTERESE_C * 10 {
        let (m, uv) = (ALVO_MHZ.load(Ordering::Relaxed), ALVO_UV.load(Ordering::Relaxed));
        let (m, uv) = if m == 0 { (mhz_boot(), UV_BOOT.load(Ordering::Relaxed)) } else { (m, uv) };
        if trocar(m, uv).is_ok() {
            LIMITADO.store(false, Ordering::Relaxed);
        }
    }
}

// ---------------------------------------------------------------- GPU e VPU

/// GPU Mali-400 e VPU: normal (400/300 MHz, vdd_log 1,175 V) ou turbo
/// (600/400 MHz, vdd_log 1,25 V). O MXOS desenha na CPU; isto so serve a
/// quem usar a GPU ou a VPU direto.
pub fn gpu_turbo(ligar: bool) -> Result<u32, &'static str> {
    if !suportado() {
        return Err("só na TV box RK322x");
    }
    let gpll = hw::gpll_mhz();
    if gpll == 0 {
        return Err("GPLL desligado");
    }
    let (gpu, vpu, uv) = if ligar { (600, 400, 1_250_000) } else { (400, 300, 1_175_000) };
    let com_tensao = hw::regulador_ok(VDD_LOG);
    if ligar && !com_tensao {
        return Err("o carregador não ligou o PWM do vdd_log: sem turbo da GPU");
    }
    let div = |alvo: u32| gpll.div_ceil(alvo).clamp(1, 32);
    if ligar {
        hw::por_tensao(VDD_LOG, uv);
        esperar_us(100);
    }
    hw::clock_gpu_vpu(div(gpu), div(vpu));
    if !ligar && com_tensao {
        hw::por_tensao(VDD_LOG, uv);
    }
    let mhz = gpll / div(gpu);
    GPU_MHZ.store(mhz, Ordering::Relaxed);
    Ok(mhz)
}

// ---------------------------------------------------------------- boot e texto

/// No boot, depois do disco: liga o sensor e aplica a config
/// (cpu_perfil, cpu_mhz/cpu_mv no personalizado, cpu_limite, gpu_turbo).
/// Devolve a linha de estado, ou None fora da TV box.
pub fn aplicar_config() -> Option<String> {
    if !suportado() {
        return None;
    }
    mhz_boot();
    SENSOR.store(hw::ligar_sensor(), Ordering::Relaxed);
    let cfg = crate::config::Config::carregar();
    if let Some(l) = cfg.obter("cpu_limite").and_then(|v| v.trim().parse::<i32>().ok()) {
        let _ = definir_limite(l);
    }
    let mut erro = None;
    match cfg.obter("cpu_perfil").map(str::trim) {
        Some("personalizado") => {
            let mhz = cfg.obter("cpu_mhz").and_then(|v| v.trim().parse::<u32>().ok());
            let uv = cfg.obter("cpu_mv").and_then(|v| v.trim().parse::<u32>().ok()).map(|mv| mv * 1000);
            match mhz {
                Some(m) => erro = definir(m, uv).err(),
                None => erro = Some("cpu_mhz faltando"),
            }
        }
        Some(nome) => match perfil(nome) {
            Some(p) => erro = definir(p.mhz, Some(p.uv)).err(),
            None => erro = Some("cpu_perfil desconhecido"),
        },
        None => {}
    }
    if cfg.obter("gpu_turbo").is_some_and(|v| v.trim() == "sim") {
        if let Err(e) = gpu_turbo(true) {
            erro = erro.or(Some(e));
        }
    }
    Some(match erro {
        Some(e) => format!("{}  (config: {e})", descricao()),
        None => descricao(),
    })
}

/// "1296 MHz, 1,350 V, 61,5 °C, perfil turbo (overclock)".
pub fn descricao() -> String {
    let mhz = mhz();
    let mut s = format!("{mhz} MHz");
    if let Some(uv) = microvolts() {
        s += &format!(", {},{:03} V", uv / 1_000_000, uv / 1000 % 1000);
    }
    match temperatura_dc() {
        Some(t) => s += &format!(", {},{} °C", t / 10, (t % 10).abs()),
        None => s += ", sem sensor",
    }
    s += &format!(", perfil {}", escolhido());
    if limitado() {
        s += &format!(" (limitado: passou de {} °C)", limite_c());
    } else if mhz > RECOMENDADO_MHZ {
        s += " (overclock acima do recomendado)";
    } else if mhz > FABRICA_MHZ {
        s += " (overclock)";
    }
    s
}

/// GPU: "400 MHz" ou o que foi posto.
pub fn descricao_gpu() -> String {
    match GPU_MHZ.load(Ordering::Relaxed) {
        0 => String::from("como o carregador deixou"),
        m => format!("{m} MHz"),
    }
}
