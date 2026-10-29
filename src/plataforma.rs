//! O que muda de uma maquina para outra: nome e mapa de memoria fisica.
//! Os enderecos sao alinhados em 2 MB (tamanho de um bloco da MMU).

#[cfg(not(feature = "mxq"))]
mod atual {
    pub const NOME: &str = "QEMU virt";
    /// RAM de 1 GB (o rodar.py passa -m 1G).
    pub const RAM_INICIO: usize = 0x4000_0000;
    pub const RAM_FIM: usize = 0x8000_0000;
    /// GIC, UART, RTC, virtio... O comeco (flash) fica sem mapear para
    /// que um ponteiro nulo de erro.
    pub const DISPOSITIVOS: &[(usize, usize)] = &[(0x0800_0000, 0x4000_0000)];
    /// Mais RAM alem da principal (para o heap).
    pub const RAM_EXTRA: &[(usize, usize)] = &[];

    /// GICv2: distribuidor e interface da CPU.
    pub const GIC_DISTRIBUIDOR: usize = 0x0800_0000;
    pub const GIC_CPU: usize = 0x0801_0000;
    /// PL011 = SPI 1 (32 + 1), sensivel a nivel.
    pub const IRQ_SERIAL: u32 = 33;
    pub const IRQ_SERIAL_BORDA: bool = false;
}

// Ainda nao testado no hardware.
#[cfg(feature = "mxq")]
mod atual {
    pub const NOME: &str = "MXQ PRO 4K (Amlogic S905W)";
    /// So um pedaco seguro da RAM: os primeiros 16 MB sao do ROM de boot e
    /// 0x05000000 em diante tem areas do firmware seguro (secmon).
    pub const RAM_INICIO: usize = 0x0100_0000;
    pub const RAM_FIM: usize = 0x0500_0000;
    /// Registradores dos perifericos (UART, GIC, timers...).
    pub const DISPOSITIVOS: &[(usize, usize)] = &[(0xC000_0000, 0x1_0000_0000)];
    /// O resto da RAM de 1 GB: depois do secmon (0x05000000, 3 MB) ate o
    /// framebuffer do U-Boot (0x3D800000). Mapeado e entregue ao heap.
    /// A regiao do framebuffer (8 MB) tambem e mapeada como RAM (a Tela
    /// limpa o cache do que desenha).
    pub const RAM_EXTRA: &[(usize, usize)] = &[(0x0540_0000, 0x3D80_0000)];
    pub const FRAMEBUFFER: (usize, usize) = (0x3D80_0000, 0x3E80_0000);

    /// GIC-400 (GICv2), enderecos do device tree meson-gx.
    pub const GIC_DISTRIBUIDOR: usize = 0xC430_1000;
    pub const GIC_CPU: usize = 0xC430_2000;
    /// UART_AO_A = SPI 193 (32 + 193), sensivel a borda de subida.
    pub const IRQ_SERIAL: u32 = 225;
    pub const IRQ_SERIAL_BORDA: bool = true;
}

pub use atual::*;

/// Segundos desde 1970 (UTC). QEMU: o RTC PL031. TV box: ela nao tem
/// relogio com bateria; ate a rede acertar a hora (rede::ntp) vale uma data
/// fixa de 2026, para os certificados do HTTPS nao parecerem do futuro.
pub fn segundos_rtc() -> u64 {
    let ajuste = AJUSTE_RTC.load(core::sync::atomic::Ordering::Relaxed);
    if ajuste != 0 {
        return ajuste.wrapping_add(crate::cpu::contador() / crate::cpu::frequencia_timer().max(1));
    }
    if cfg!(feature = "mxq") {
        return 1_793_000_000 + crate::cpu::contador() / crate::cpu::frequencia_timer().max(1);
    }
    unsafe { core::ptr::read_volatile(0x0901_0000 as *const u32) as u64 }
}

/// Hora certa vinda da rede: segundos de 1970 quando o contador era 0.
static AJUSTE_RTC: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

pub fn acertar_rtc(segundos_agora: u64) {
    let desde_o_boot = crate::cpu::contador() / crate::cpu::frequencia_timer().max(1);
    AJUSTE_RTC.store(segundos_agora.saturating_sub(desde_o_boot).max(1), core::sync::atomic::Ordering::Relaxed);
}

/// Timer virtual do ARM (PPI 11), o mesmo numero nas duas maquinas.
pub const IRQ_TIMER: u32 = 27;
