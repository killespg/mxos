//! H.264 e MP4 com um video de verdade (Big Buck Bunny 360p, 10 s), sem o
//! hardware: indice do MP4, SPS/PPS, cabecalhos de todas as fatias, POC e
//! o DPB inteiro. A prova: a ordem em que o DPB manda mostrar os quadros
//! tem que ser a mesma dos tempos de apresentacao do MP4 (ctts).
//!
//! VIDEO=caminho.mp4 troca o arquivo (padrao: C:/dev/mxos-tmp/videos/bbb360.mp4).

use teste_web::midia::h264::{self, Dpb, Pps, Sps};
use teste_web::midia::mp4::{self, Mp4};

#[test]
fn bbb_ordem_de_exibicao() {
    let caminho = std::env::var("VIDEO").unwrap_or_else(|_| String::from("C:/dev/mxos-tmp/videos/bbb360.mp4"));
    let Ok(arquivo) = std::fs::read(&caminho) else {
        eprintln!("sem {caminho}: pulando");
        return;
    };
    let mut fonte = arquivo.clone();
    let mp4 = Mp4::abrir(&mut fonte).expect("moov");
    let v = mp4.video().expect("trilha de video");
    assert_eq!(&v.codec, b"avc1");
    println!("video {}x{}, {} amostras, escala {}, {} ms", v.largura, v.altura, v.amostras.len(), v.escala, v.duracao_ms());
    if let Some(a) = mp4.audio() {
        println!("audio {:?} {} canais {} Hz, {} pacotes", std::str::from_utf8(&a.codec), a.canais, a.taxa, a.amostras.len());
    }

    let (tam, spss_b, ppss_b) = mp4::avcc(&v.config).expect("avcC");
    let mut spss: Vec<Option<Sps>> = vec![None; 32];
    let mut ppss: Vec<Option<Pps>> = vec![None; 256];
    for s in spss_b {
        let sps = h264::ler_sps(s).expect("SPS");
        println!(
            "SPS {}: perfil {} nivel {} {}x{} (visivel {:?}) poc {} refs {} dpb {} reordenar {:?} 8x8 {} escala {}",
            sps.id,
            sps.perfil,
            sps.nivel,
            sps.largura(),
            sps.altura(),
            sps.visivel(),
            sps.tipo_poc,
            sps.max_refs,
            sps.tamanho_dpb(),
            sps.reordenar,
            sps.direto_8x8,
            sps.escala.is_some()
        );
        let id = sps.id as usize;
        spss[id] = Some(sps);
    }
    for p in ppss_b {
        let pps = h264::ler_pps(p, &spss).expect("PPS");
        println!("PPS {}: cabac {} 8x8 {} pesos {} {}", pps.id, pps.cabac, pps.transformada_8x8, pps.predicao_pesos, pps.bipred_pesos);
        let id = pps.id as usize;
        ppss[id] = Some(pps);
    }
    let sps0 = spss.iter().flatten().next().unwrap().clone();
    assert_eq!(sps0.visivel(), (v.largura, v.altura), "tamanho do SPS diferente do MP4");

    let buffers = Dpb::buffers_para(&sps0);
    let mut dpb = Dpb::novo(&sps0, buffers);
    let mut ordem: Vec<usize> = Vec::new(); // amostras na ordem em que o DPB mostrou
    let mut por_buffer: Vec<usize> = vec![usize::MAX; buffers];
    let mut tipos = [0usize; 5];
    for (k, a) in v.amostras.iter().enumerate() {
        let dados = &arquivo[a.pos as usize..a.pos as usize + a.tamanho as usize];
        let nals = h264::nals_com_tamanho(dados, tam);
        let mut primeira = None;
        for n in &nals {
            let t = n[0] & 0x1F;
            if t == 1 || t == 5 {
                let f = h264::ler_fatia(n, &spss, &ppss).unwrap_or_else(|e| panic!("fatia da amostra {k}: {e}"));
                if primeira.is_none() {
                    tipos[f.tipo as usize] += 1;
                    primeira = Some(f);
                }
            }
        }
        let f = primeira.expect("amostra sem fatia");
        assert_eq!(f.idr() || !a.chave || k > 0, true);
        let q = dpb.comecar(&sps0, &f).unwrap_or_else(|e| panic!("comecar {k}: {e}"));
        // listas: so lugares com referencia, sem repetir
        for l in [&q.lista_p, &q.lista_b0, &q.lista_b1] {
            for &i in l.iter() {
                assert!(q.dpb[i as usize].ocupado && q.dpb[i as usize].referencia != h264::Ref::Nao);
            }
        }
        por_buffer[q.buffer] = k;
        dpb.terminar(&sps0, &f, &q);
        while let Some((b, _poc)) = dpb.saida.pop_front() {
            ordem.push(por_buffer[b]);
        }
    }
    dpb.esvaziar();
    while let Some((b, _)) = dpb.saida.pop_front() {
        ordem.push(por_buffer[b]);
    }
    println!("fatias: {} P, {} B, {} I", tipos[0], tipos[1], tipos[2]);
    assert_eq!(ordem.len(), v.amostras.len(), "quadros perdidos ou repetidos");
    // a ordem certa: pelo tempo de apresentacao do MP4
    let mut certa: Vec<usize> = (0..v.amostras.len()).collect();
    certa.sort_by_key(|&i| v.amostras[i].dts as i64 + v.amostras[i].desvio as i64);
    if let Some(p) = ordem.iter().zip(&certa).position(|(a, b)| a != b) {
        println!("primeira diferenca na posicao {p}: {:?} vs {:?}", &ordem[p.saturating_sub(5)..(p + 10).min(ordem.len())], &certa[p.saturating_sub(5)..(p + 10).min(certa.len())]);
    }
    let erros = ordem.iter().zip(&certa).filter(|(a, b)| a != b).count();
    assert_eq!(erros, 0, "ordem de exibicao diferente em {erros} quadros; primeiros: {:?} vs {:?}", &ordem[..20.min(ordem.len())], &certa[..20.min(certa.len())]);
}
