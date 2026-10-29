//! TCP (RFC 9293), so o lado cliente e uma conexao por vez.
//!   porta origem, porta destino, seq, ack, tamanho do cabecalho, flags,
//!   janela, checksum (com pseudo-cabecalho IP), urgente, opcoes
//!
//! O que fazemos:
//!   - abertura em 3 passos com a opcao MSS (1460) no SYN
//!   - envio dentro da janela do outro lado, em pedacos de ate MSS
//!   - retransmissao do segmento mais antigo quando o relogio estoura
//!     (1 s, dobrando ate 16 s; desiste na 6a tentativa), medido com cpu::contador
//!   - recebimento so em ordem: pedaco fora de ordem e jogado fora e o ACK
//!     repetido sai na hora (3 repetidos = o outro lado retransmite rapido)
//!   - ACK a cada 2 segmentos em ordem, ou no fim da rodada de `processar`
//!   - fechamento pelos dois lados (FIN), RST para o que nao e nosso
//!
//! Sem window scaling, SACK nem timestamps: a janela que anunciamos e o
//! espaco livre do buffer de recebimento (ate 64 KB).

use alloc::vec::Vec;

use super::ipv4::PROTO_TCP;
use super::{ErroRede, Pilha, agora_ms, checksum, escrever16, escrever32, ler16, ler32, pseudo_cabecalho};

// Flags
const FIN: u8 = 0x01;
const SYN: u8 = 0x02;
const RST: u8 = 0x04;
const PSH: u8 = 0x08;
const ACK: u8 = 0x10;

const TAMANHO: usize = 20;
/// Maior pedaco de dados por segmento (1500 - 20 de IP - 20 de TCP).
pub const MSS: usize = 1460;
/// MSS assumido se o outro lado nao mandar a opcao.
const MSS_PADRAO: usize = 536;
/// Buffer de recebimento (a janela cabe em 16 bits).
const MAX_RECEBIDO: usize = 65535;

const RTO_INICIAL_MS: u64 = 1000;
const RTO_MAXIMO_MS: u64 = 16000;
const MAX_TENTATIVAS: u32 = 6;
/// TIME_WAIT curto: so para responder um FIN repetido.
const TEMPO_ESPERA_MS: u64 = 2000;
const PRIMEIRA_PORTA: u16 = 49152;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Estado {
    Fechada,
    SynEnviado,
    Estabelecida,
    /// Mandamos FIN, esperando o ACK dele.
    FinEspera1,
    /// Nosso FIN foi confirmado, esperando o FIN do outro lado.
    FinEspera2,
    /// Os dois mandaram FIN ao mesmo tempo.
    Fechando,
    TempoEspera,
    /// O outro lado mandou FIN; ainda podemos enviar.
    EsperaFechar,
    /// Mandamos nosso FIN depois do dele.
    UltimoAck,
}

impl Estado {
    pub fn nome(&self) -> &'static str {
        match self {
            Estado::Fechada => "fechada",
            Estado::SynEnviado => "SYN enviado",
            Estado::Estabelecida => "estabelecida",
            Estado::FinEspera1 => "FIN espera 1",
            Estado::FinEspera2 => "FIN espera 2",
            Estado::Fechando => "fechando",
            Estado::TempoEspera => "TIME_WAIT",
            Estado::EsperaFechar => "CLOSE_WAIT",
            Estado::UltimoAck => "último ACK",
        }
    }
}

pub struct Conexao {
    pub estado: Estado,
    remoto: [u8; 4],
    porta_local: u16,
    porta_remota: u16,
    // envio
    snd_una: u32,
    snd_nxt: u32,
    janela_remota: u32,
    mss_remoto: usize,
    /// Bytes ainda nao confirmados (enviados ou nao); envio[0] e o snd_una.
    envio: Vec<u8>,
    fechar_pedido: bool,
    fin_enviado: bool,
    // recebimento
    rcv_nxt: u32,
    recebido: Vec<u8>,
    pub fin_recebido: bool,
    ack_pendente: bool,
    /// Segmentos em ordem recebidos desde o ultimo ACK.
    segmentos_sem_ack: u32,
    janela_anunciada: u32,
    // relogios
    ultimo_envio_ms: u64,
    rto_ms: u64,
    tentativas: u32,
    desde_ms: u64,
    /// Por que a conexao caiu (RST, tempo esgotado).
    pub erro: Option<ErroRede>,
    proxima_porta: u16,
}

/// Um segmento que chegou, ja separado.
struct Segmento<'a> {
    porta_origem: u16,
    porta_destino: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    janela: u16,
    opcoes: &'a [u8],
    dados: &'a [u8],
}

/// `a` vem antes de `b` no espaco circular de numeros de sequencia.
fn antes(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}

impl Conexao {
    pub fn nova(semente: u64) -> Conexao {
        let mut c = Conexao::fechada();
        c.proxima_porta = PRIMEIRA_PORTA + (semente % 10_000) as u16;
        c
    }

    fn fechada() -> Conexao {
        Conexao {
            estado: Estado::Fechada,
            remoto: [0; 4],
            porta_local: 0,
            porta_remota: 0,
            snd_una: 0,
            snd_nxt: 0,
            janela_remota: 0,
            mss_remoto: MSS_PADRAO,
            envio: Vec::new(),
            fechar_pedido: false,
            fin_enviado: false,
            rcv_nxt: 0,
            recebido: Vec::new(),
            fin_recebido: false,
            ack_pendente: false,
            segmentos_sem_ack: 0,
            janela_anunciada: 0,
            ultimo_envio_ms: 0,
            rto_ms: RTO_INICIAL_MS,
            tentativas: 0,
            desde_ms: 0,
            erro: None,
            proxima_porta: PRIMEIRA_PORTA,
        }
    }

    /// Conexao ativa (nem fechada nem em TIME_WAIT)?
    pub fn aberta(&self) -> bool {
        !matches!(self.estado, Estado::Fechada | Estado::TempoEspera)
    }

    /// O outro lado fechou e ja lemos tudo.
    pub fn terminou(&self) -> bool {
        (self.fin_recebido || self.estado == Estado::Fechada) && self.recebido.is_empty()
    }

    fn espaco_livre(&self) -> usize {
        MAX_RECEBIDO - self.recebido.len()
    }

    /// Bytes de dados enviados e ainda nao confirmados.
    fn dados_em_voo(&self) -> usize {
        self.snd_nxt.wrapping_sub(self.snd_una) as usize - self.fin_enviado as usize
    }
}

/// MSS da opcao 2 (kind 2, tamanho 4).
fn ler_mss(opcoes: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i < opcoes.len() {
        match opcoes[i] {
            0 => return None,
            1 => i += 1,
            2 if opcoes.len() >= i + 4 && opcoes[i + 1] == 4 => return Some(ler16(opcoes, i + 2) as usize),
            _ => {
                let tamanho = *opcoes.get(i + 1)? as usize;
                if tamanho < 2 {
                    return None;
                }
                i += tamanho;
            }
        }
    }
    None
}

/// Monta cabecalho (com a opcao MSS se for SYN) e checksum.
/// Devolve o cabecalho e quantos bytes dele valem.
#[allow(clippy::too_many_arguments)]
fn montar(origem: [u8; 4], destino: [u8; 4], portas: (u16, u16), seq: u32, ack: u32, flags: u8, janela: u16, dados: &[u8]) -> ([u8; 24], usize) {
    let mut c = [0u8; 24];
    let tamanho = if flags & SYN != 0 { 24 } else { TAMANHO };
    escrever16(&mut c, 0, portas.0);
    escrever16(&mut c, 2, portas.1);
    escrever32(&mut c, 4, seq);
    escrever32(&mut c, 8, ack);
    c[12] = ((tamanho / 4) as u8) << 4;
    c[13] = flags;
    escrever16(&mut c, 14, janela);
    if tamanho == 24 {
        c[20..24].copy_from_slice(&[2, 4, (MSS >> 8) as u8, MSS as u8]);
    }
    let pseudo = pseudo_cabecalho(origem, destino, PROTO_TCP, tamanho + dados.len());
    let soma = checksum(&[&pseudo, &c[..tamanho], dados]);
    escrever16(&mut c, 16, soma);
    (c, tamanho)
}

impl Pilha {
    // ---------------- envio de segmentos ----------------

    /// Manda um segmento da conexao atual (ACK sempre com rcv_nxt).
    fn tcp_segmento(&mut self, seq: u32, flags: u8, dados: &[u8]) {
        let c = &mut self.tcp;
        let janela = c.espaco_livre() as u32;
        let ack = if flags & ACK != 0 { c.rcv_nxt } else { 0 };
        let (cabecalho, tamanho) = montar(self.ip, c.remoto, (c.porta_local, c.porta_remota), seq, ack, flags, janela as u16, dados);
        c.janela_anunciada = janela;
        if flags & ACK != 0 {
            c.ack_pendente = false;
            c.segmentos_sem_ack = 0;
        }
        let remoto = c.remoto;
        self.enviar_ip(remoto, PROTO_TCP, &[&cabecalho[..tamanho], dados]);
    }

    /// RST para um segmento que nao pertence a conexao nenhuma.
    fn tcp_recusar(&mut self, origem: [u8; 4], s: &Segmento) {
        if self.ip == [0; 4] {
            return;
        }
        let portas = (s.porta_destino, s.porta_origem);
        let (seq, ack, flags) = if s.flags & ACK != 0 {
            (s.ack, 0, RST)
        } else {
            let tamanho = s.dados.len() as u32 + (s.flags & SYN != 0) as u32 + (s.flags & FIN != 0) as u32;
            (0, s.seq.wrapping_add(tamanho), RST | ACK)
        };
        let (cabecalho, tamanho) = montar(self.ip, origem, portas, seq, ack, flags, 0, &[]);
        self.enviar_ip(origem, PROTO_TCP, &[&cabecalho[..tamanho]]);
    }

    /// Liga o relogio de retransmissao se nada estava em voo.
    fn tcp_relogio_se_vazio(&mut self) {
        if self.tcp.snd_nxt == self.tcp.snd_una {
            self.tcp.ultimo_envio_ms = agora_ms();
        }
    }

    /// Manda o que a janela deixa e, se o app fechou, o FIN no fim.
    fn tcp_transmitir(&mut self) {
        if !matches!(self.tcp.estado, Estado::Estabelecida | Estado::EsperaFechar) {
            return;
        }
        loop {
            let c = &self.tcp;
            let enviados = c.dados_em_voo();
            let limite = c.envio.len().min(c.janela_remota as usize);
            if enviados >= limite {
                break;
            }
            let n = (limite - enviados).min(c.mss_remoto);
            let pedaco = c.envio[enviados..enviados + n].to_vec();
            let seq = c.snd_nxt;
            self.tcp_relogio_se_vazio();
            self.tcp_segmento(seq, ACK | PSH, &pedaco);
            self.tcp.snd_nxt = seq.wrapping_add(n as u32);
        }
        let c = &self.tcp;
        if c.fechar_pedido && !c.fin_enviado && c.dados_em_voo() == c.envio.len() {
            let seq = c.snd_nxt;
            self.tcp_relogio_se_vazio();
            self.tcp_segmento(seq, FIN | ACK, &[]);
            let c = &mut self.tcp;
            c.snd_nxt = seq.wrapping_add(1);
            c.fin_enviado = true;
            c.estado = if c.estado == Estado::Estabelecida { Estado::FinEspera1 } else { Estado::UltimoAck };
        }
    }

    /// O relogio estourou: reenvia o mais antigo que nao foi confirmado
    /// (ou sonda a janela zero com 1 byte).
    fn tcp_retransmitir(&mut self) {
        let c = &self.tcp;
        let seq = c.snd_una;
        if c.estado == Estado::SynEnviado {
            self.tcp_segmento(seq, SYN, &[]);
        } else if c.dados_em_voo() > 0 {
            let pedaco = c.envio[..c.dados_em_voo().min(c.mss_remoto)].to_vec();
            self.tcp_segmento(seq, ACK | PSH, &pedaco);
        } else if c.fin_enviado {
            self.tcp_segmento(seq, FIN | ACK, &[]);
        } else if !c.envio.is_empty() {
            // janela zero: 1 byte para descobrir quando ela abre
            let sonda = [c.envio[0]];
            self.tcp_segmento(seq, ACK | PSH, &sonda);
            self.tcp.snd_nxt = seq.wrapping_add(1);
        }
    }

    // ---------------- relogios ----------------

    /// Chamado em todo `processar`: ACK atrasado, retransmissao, TIME_WAIT.
    pub(super) fn tcp_temporizar(&mut self) {
        let c = &self.tcp;
        if c.estado == Estado::Fechada {
            return;
        }
        if c.ack_pendente {
            let seq = c.snd_nxt;
            self.tcp_segmento(seq, ACK, &[]);
        }
        let agora = agora_ms();
        let c = &mut self.tcp;
        if c.estado == Estado::TempoEspera {
            if agora.saturating_sub(c.desde_ms) >= TEMPO_ESPERA_MS {
                c.estado = Estado::Fechada;
            }
            return;
        }
        let em_voo = c.snd_nxt != c.snd_una;
        let sondar = c.janela_remota == 0 && !c.envio.is_empty() && matches!(c.estado, Estado::Estabelecida | Estado::EsperaFechar);
        if !(em_voo || sondar) || agora.saturating_sub(c.ultimo_envio_ms) < c.rto_ms {
            return;
        }
        c.tentativas += 1;
        if c.tentativas > MAX_TENTATIVAS {
            c.erro = Some(ErroRede::TempoEsgotado);
            self.tcp_abortar();
            return;
        }
        c.rto_ms = (c.rto_ms * 2).min(RTO_MAXIMO_MS);
        c.ultimo_envio_ms = agora;
        self.tcp_retransmitir();
    }

    // ---------------- recebimento ----------------

    pub(super) fn receber_tcp(&mut self, origem: [u8; 4], destino: [u8; 4], dados: &[u8]) {
        if dados.len() < TAMANHO {
            return;
        }
        let pseudo = pseudo_cabecalho(origem, destino, PROTO_TCP, dados.len());
        if checksum(&[&pseudo, dados]) != 0 {
            return;
        }
        let tamanho = (dados[12] >> 4) as usize * 4;
        if tamanho < TAMANHO || tamanho > dados.len() {
            return;
        }
        let s = Segmento {
            porta_origem: ler16(dados, 0),
            porta_destino: ler16(dados, 2),
            seq: ler32(dados, 4),
            ack: ler32(dados, 8),
            flags: dados[13],
            janela: ler16(dados, 14),
            opcoes: &dados[TAMANHO..tamanho],
            dados: &dados[tamanho..],
        };
        let c = &self.tcp;
        let nossa = c.estado != Estado::Fechada && origem == c.remoto && s.porta_origem == c.porta_remota && s.porta_destino == c.porta_local;
        if !nossa {
            if s.flags & RST == 0 {
                self.tcp_recusar(origem, &s);
            }
            return;
        }
        if s.flags & RST != 0 {
            self.tcp_rst_recebido(&s);
            return;
        }
        if self.tcp.estado == Estado::SynEnviado {
            self.tcp_syn_ack(&s);
            return;
        }
        self.tcp_ack(&s);
        let ack_ja = self.tcp_dados(&s);
        self.tcp_transmitir();
        if ack_ja && self.tcp.ack_pendente {
            let seq = self.tcp.snd_nxt;
            self.tcp_segmento(seq, ACK, &[]);
        }
    }

    /// RST valido derruba a conexao.
    fn tcp_rst_recebido(&mut self, s: &Segmento) {
        let c = &mut self.tcp;
        let valido = if c.estado == Estado::SynEnviado {
            s.flags & ACK != 0 && s.ack == c.snd_nxt
        } else {
            s.seq.wrapping_sub(c.rcv_nxt) <= c.janela_anunciada.max(1)
        };
        if !valido {
            return;
        }
        c.erro = match c.estado {
            Estado::SynEnviado => Some(ErroRede::Recusado),
            Estado::Estabelecida | Estado::FinEspera1 | Estado::FinEspera2 if !c.fin_recebido => {
                Some(ErroRede::Protocolo("conexão interrompida pelo servidor (RST)"))
            }
            _ => c.erro,
        };
        c.estado = Estado::Fechada;
    }

    /// Resposta ao nosso SYN.
    fn tcp_syn_ack(&mut self, s: &Segmento) {
        let c = &mut self.tcp;
        if s.flags & ACK != 0 && s.ack != c.snd_nxt {
            // ACK de outra conexao: RST com seq = ack dele
            let remoto = c.remoto;
            self.tcp_recusar(remoto, s);
            return;
        }
        if s.flags & (SYN | ACK) != SYN | ACK {
            return; // abertura simultanea: nao suportada
        }
        c.rcv_nxt = s.seq.wrapping_add(1);
        c.snd_una = s.ack;
        c.janela_remota = s.janela as u32;
        c.mss_remoto = ler_mss(s.opcoes).unwrap_or(MSS_PADRAO).clamp(64, MSS);
        c.estado = Estado::Estabelecida;
        c.tentativas = 0;
        c.rto_ms = RTO_INICIAL_MS;
        c.ack_pendente = true;
        self.tcp_transmitir();
    }

    /// Campo ACK: libera o que foi confirmado e anda os estados do FIN.
    fn tcp_ack(&mut self, s: &Segmento) {
        let c = &mut self.tcp;
        if s.flags & ACK == 0 || antes(s.ack, c.snd_una) {
            return; // sem ACK ou ACK velho
        }
        if antes(c.snd_nxt, s.ack) {
            c.ack_pendente = true; // confirma o que nunca mandamos
            return;
        }
        c.janela_remota = s.janela as u32;
        let avanco = s.ack.wrapping_sub(c.snd_una) as usize;
        if avanco == 0 {
            return;
        }
        let fin_confirmado = c.fin_enviado && s.ack == c.snd_nxt;
        let dados = (avanco - fin_confirmado as usize).min(c.envio.len());
        c.envio.drain(..dados);
        c.snd_una = s.ack;
        c.tentativas = 0;
        c.rto_ms = RTO_INICIAL_MS;
        c.ultimo_envio_ms = agora_ms();
        if fin_confirmado {
            // o FIN ja foi confirmado: nao conta mais como em voo
            c.fin_enviado = false;
            c.fechar_pedido = false;
            match c.estado {
                Estado::FinEspera1 => c.estado = Estado::FinEspera2,
                Estado::Fechando => c.entrar_tempo_espera(),
                Estado::UltimoAck => c.estado = Estado::Fechada,
                _ => {}
            }
        }
    }

    /// Dados e FIN: so aceitamos o que vem em ordem. Devolve true se o
    /// ACK deve sair agora (fora de ordem, repeticao, FIN ou 2o segmento).
    fn tcp_dados(&mut self, s: &Segmento) -> bool {
        let c = &mut self.tcp;
        let fin = s.flags & FIN != 0;
        if s.dados.is_empty() && !fin {
            return false;
        }
        c.ack_pendente = true;
        if !matches!(c.estado, Estado::Estabelecida | Estado::FinEspera1 | Estado::FinEspera2) {
            return true; // ja recebemos o FIN: e repeticao, so confirmar de novo
        }
        let mut dados = s.dados;
        let mut seq = s.seq;
        if antes(seq, c.rcv_nxt) {
            // comeco repetido: aproveita so o que e novo
            let repetido = c.rcv_nxt.wrapping_sub(seq) as usize;
            if repetido > dados.len() || (repetido == dados.len() && !fin) {
                return true;
            }
            dados = &dados[repetido..];
            seq = c.rcv_nxt;
        }
        if seq != c.rcv_nxt {
            return true; // fora de ordem: descarta, o ACK repetido pede de novo
        }
        let aceitos = dados.len().min(c.espaco_livre());
        c.recebido.extend_from_slice(&dados[..aceitos]);
        c.rcv_nxt = c.rcv_nxt.wrapping_add(aceitos as u32);
        if fin && aceitos == dados.len() {
            c.rcv_nxt = c.rcv_nxt.wrapping_add(1);
            c.fin_recebido = true;
            match c.estado {
                Estado::Estabelecida => c.estado = Estado::EsperaFechar,
                Estado::FinEspera1 => c.estado = Estado::Fechando,
                _ => c.entrar_tempo_espera(),
            }
        }
        c.segmentos_sem_ack += 1;
        fin || c.segmentos_sem_ack >= 2
    }

    // ---------------- API para o http ----------------

    /// Abre uma conexao nova (derruba a anterior se ainda estiver aberta).
    pub(super) fn tcp_conectar(&mut self, remoto: [u8; 4], porta: u16) {
        if self.tcp.aberta() {
            self.tcp_abortar();
        }
        let porta_local = self.tcp.proxima_porta;
        let iss = (crate::cpu::contador() >> 4) as u32;
        let mut c = Conexao::fechada();
        c.proxima_porta = if porta_local >= 65000 { PRIMEIRA_PORTA } else { porta_local + 1 };
        c.estado = Estado::SynEnviado;
        c.remoto = remoto;
        c.porta_local = porta_local;
        c.porta_remota = porta;
        c.snd_una = iss;
        c.snd_nxt = iss.wrapping_add(1);
        c.ultimo_envio_ms = agora_ms();
        self.tcp = c;
        self.tcp_segmento(iss, SYN, &[]);
    }

    /// Enfileira dados para enviar. false se a conexao nao aceita mais.
    pub(super) fn tcp_enviar(&mut self, dados: &[u8]) -> bool {
        let c = &mut self.tcp;
        let pode = matches!(c.estado, Estado::SynEnviado | Estado::Estabelecida | Estado::EsperaFechar);
        if !pode || c.fechar_pedido {
            return false;
        }
        c.envio.extend_from_slice(dados);
        self.tcp_transmitir();
        true
    }

    /// Move o que chegou para `destino`. Devolve quantos bytes.
    pub(super) fn tcp_ler(&mut self, destino: &mut Vec<u8>) -> usize {
        let c = &mut self.tcp;
        let n = c.recebido.len();
        if n == 0 {
            return 0;
        }
        destino.append(&mut c.recebido);
        // a janela estava quase fechada: avisa que abriu
        if c.janela_anunciada < 2 * MSS as u32 && c.aberta() {
            c.ack_pendente = true;
        }
        n
    }

    /// Fechamento educado: FIN depois dos dados que faltam.
    pub(super) fn tcp_fechar(&mut self) {
        match self.tcp.estado {
            Estado::Fechada | Estado::TempoEspera => {}
            Estado::SynEnviado => self.tcp_abortar(),
            _ => {
                self.tcp.fechar_pedido = true;
                self.tcp_transmitir();
            }
        }
    }

    /// Derruba a conexao com RST.
    pub(super) fn tcp_abortar(&mut self) {
        if self.tcp.aberta() {
            let seq = self.tcp.snd_nxt;
            self.tcp_segmento(seq, RST, &[]);
        }
        self.tcp.estado = Estado::Fechada;
    }
}

impl Conexao {
    fn entrar_tempo_espera(&mut self) {
        self.estado = Estado::TempoEspera;
        self.desde_ms = agora_ms();
    }
}
