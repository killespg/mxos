//! Hora certa pela rede (SNTP, RFC 4330): a TV box nao tem relogio com
//! bateria, e o HTTPS precisa da data para saber se um certificado venceu.
//! Uma pergunta de 48 bytes para pool.ntp.org; da resposta so vale o
//! "transmit timestamp" (segundos desde 1900).

use super::soquete::{udp_abrir, udp_enviar, udp_fechar};
use super::{ErroRede, esperar};

const SERVIDOR: &str = "pool.ntp.org";
const PORTA: u16 = 123;
const DE_1900_A_1970: u64 = 2_208_988_800;
const TEMPO_MS: u64 = 2000;

/// Pergunta a hora e acerta o relogio. Devolve os segundos desde 1970.
pub fn acertar() -> Result<u64, ErroRede> {
    let ip = super::resolver(SERVIDOR)?;
    let porta = udp_abrir(0)?;
    let mut pedido = [0u8; 48];
    pedido[0] = 0x23; // sem aviso, versao 4, modo cliente
    let r = udp_enviar(porta, ip, PORTA, &pedido).and_then(|_| {
        esperar(TEMPO_MS, |p| {
            let d = p.udp.tirar(porta)?;
            (d.porta_origem == PORTA && d.dados.len() >= 48).then(|| {
                let s = u32::from_be_bytes([d.dados[40], d.dados[41], d.dados[42], d.dados[43]]) as u64;
                s.saturating_sub(DE_1900_A_1970)
            })
        })
    });
    udp_fechar(porta);
    let segundos = r?;
    if segundos < 1_700_000_000 {
        return Err(ErroRede::Protocolo("hora do NTP sem sentido"));
    }
    crate::plataforma::acertar_rtc(segundos);
    Ok(segundos)
}
