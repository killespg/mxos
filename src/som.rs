//! Comando "som" do shell: toca uma melodia curta pelo virtio-sound.
//! So existe no QEMU virt (na TV box o som vai ser HDMI, outro driver).
//!
//! A memoria de DMA vem do heap (RAM normal com cache, mapa identidade, entao
//! endereco virtual = fisico). Ela so volta para o heap depois do reset do
//! dispositivo, para ele nunca escrever em memoria ja reaproveitada.

use alloc::alloc::{Layout, alloc_zeroed, dealloc};
use core::fmt::Write;

use crate::audio::gerador::{Forma, Melodia, Nota};
use crate::virtio::virtio_mmio::{TIPO_SOM, VirtioMmio};
use crate::audio::virtio_som::{self, ErroSom, VirtioSom};
use crate::{cpu, irq, timer};

/// Do, Mi, Sol.
const NOTAS: [Nota; 3] = [
    Nota { freq_hz: 523, duracao_ms: 500 },
    Nota { freq_hz: 659, duracao_ms: 500 },
    Nota { freq_hz: 784, duracao_ms: 500 },
];
const VOLUME: u16 = 12000;
const TAMANHO_DMA: usize = 64 * 1024;
/// A cauda do audio ainda esta no mixer do QEMU quando o ultimo periodo volta.
const ESPERA_CAUDA_MS: u64 = 300;

/// Toca a melodia e escreve o resumo em `saida`.
pub fn tocar_melodia(saida: &mut impl Write) -> Result<(), ErroSom> {
    let Some(disp) = VirtioMmio::procurar(TIPO_SOM) else {
        let _ = writeln!(saida, "nenhum virtio-sound (QEMU: -device virtio-sound-device,audiodev=...)");
        return Ok(());
    };
    let base = disp.base();
    let layout = Layout::from_size_align(TAMANHO_DMA, 4096).map_err(|_| ErroSom::SemMemoria)?;
    let memoria = unsafe { alloc_zeroed(layout) };
    if memoria.is_null() {
        return Err(ErroSom::SemMemoria);
    }
    let resultado = unsafe { tocar_com_memoria(saida, disp, memoria as usize) };
    // reset antes de devolver a memoria: o dispositivo para de fazer DMA nela
    unsafe { VirtioMmio::em(base) }.resetar();
    unsafe { dealloc(memoria, layout) };
    resultado
}

unsafe fn tocar_com_memoria(saida: &mut impl Write, disp: VirtioMmio, memoria: usize) -> Result<(), ErroSom> {
    let slot = disp.slot_qemu_virt().unwrap_or(0);
    let intid = disp.intid_qemu_virt().unwrap_or(0);
    let _ = writeln!(saida, "virtio-sound em 0x{:x} (slot {slot}, INTID {intid})", disp.base());
    let mut som = unsafe { VirtioSom::iniciar(disp, memoria, TAMANHO_DMA)? };
    let (stream, info) = som.primeiro_stream_de_saida()?;
    let p = virtio_som::escolher_parametros(&info, &[48000, 44100], 25, 8)?;
    som.configurar(stream, &p)?;
    let _ = writeln!(
        saida,
        "stream {stream}: S16, {} Hz, {} canais, {} slots de {} quadros",
        p.taxa_hz,
        p.canais,
        som.slots(),
        p.quadros_periodo
    );
    let mut melodia = Melodia::nova(&NOTAS, p.taxa_hz, Forma::Seno, VOLUME);
    let inicio = cpu::contador();
    // entre um periodo e outro a CPU dorme; o tique de 100 Hz acorda
    let e = som.tocar(&mut melodia, irq::cochilar)?;
    let ms = (cpu::contador() - inicio) * 1000 / cpu::frequencia_timer();
    timer::esperar_ms(ESPERA_CAUDA_MS);
    som.parar()?;
    som.liberar()?;
    let _ = writeln!(
        saida,
        "tocou {} quadros em {} períodos ({} ms), erros {}, xruns {}",
        e.quadros, e.periodos, ms, e.erros_tx, e.xruns
    );
    som.encerrar();
    Ok(())
}
