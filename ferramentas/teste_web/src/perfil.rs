//! Profiler por amostragem (so Windows x64, so para os testes): roda `f`
//! numa thread e, de fora, suspende ela umas mil vezes por segundo para ler
//! o RIP (onde ela esta). Devolve os enderecos relativos ao inicio do
//! executavel (RVA); `simbolos.rs`/o script do objdump da os nomes.

#[repr(C, align(16))]
struct Contexto([u8; 1232]);

const CONTEXT_CONTROL: u32 = 0x0010_0001;
const DESLOC_FLAGS: usize = 0x30;
const DESLOC_RIP: usize = 0xF8;

unsafe extern "system" {
    fn SuspendThread(h: *mut core::ffi::c_void) -> u32;
    fn ResumeThread(h: *mut core::ffi::c_void) -> u32;
    fn GetThreadContext(h: *mut core::ffi::c_void, c: *mut Contexto) -> i32;
    fn GetModuleHandleW(nome: *const u16) -> *mut core::ffi::c_void;
}

/// Roda `f` e devolve os RVAs amostrados.
pub fn amostrar<F: FnOnce() + Send + 'static>(f: F) -> Vec<u64> {
    use std::os::windows::io::AsRawHandle;
    let base = unsafe { GetModuleHandleW(core::ptr::null()) } as u64;
    let t = std::thread::Builder::new().stack_size(64 << 20).spawn(f).unwrap();
    let h = t.as_raw_handle() as *mut core::ffi::c_void;
    let mut amostras = Vec::new();
    while !t.is_finished() {
        std::thread::sleep(std::time::Duration::from_micros(500));
        unsafe {
            if SuspendThread(h) == u32::MAX {
                break;
            }
            let mut c = Contexto([0; 1232]);
            c.0[DESLOC_FLAGS..DESLOC_FLAGS + 4].copy_from_slice(&CONTEXT_CONTROL.to_le_bytes());
            if GetThreadContext(h, &mut c) != 0 {
                let rip = u64::from_le_bytes(c.0[DESLOC_RIP..DESLOC_RIP + 8].try_into().unwrap());
                amostras.push(rip.wrapping_sub(base));
            }
            ResumeThread(h);
        }
    }
    let _ = t.join();
    amostras
}

/// Grava os RVAs (um por linha, em hexa) para o script de simbolos.
pub fn gravar(nome: &str, amostras: &[u64]) {
    let texto: String = amostras.iter().map(|a| format!("{a:x}\n")).collect();
    std::fs::write(crate::png::pasta().join(format!("perfil_{nome}.txt")), texto).unwrap();
}
