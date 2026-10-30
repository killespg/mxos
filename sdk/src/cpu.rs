//! O relogio do app: o "contador" conta microssegundos (TEMPO_US), entao
//! as contas do sistema com contador() e frequencia_timer() valem igual.

pub fn contador() -> u64 {
    crate::sys::tempo_us()
}

pub fn frequencia_timer() -> u64 {
    1_000_000
}

pub fn barreira() {
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
}
