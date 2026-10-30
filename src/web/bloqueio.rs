//! Bloqueio de anuncios e rastreadores (como o Brave ou o Chrome com
//! bloqueador no Android): scripts, fetch/XHR e imagens desses enderecos
//! nem sao baixados. Na TV box isso e a diferenca entre segundos e minutos:
//! no UOL, mais da metade do JavaScript e propaganda e rastreamento.
//!
//! A lista e de dominios (o dominio e todos os subdominios) mais alguns
//! pedacos de caminho bem caracteristicos. A pagina principal nunca e
//! bloqueada (so os recursos dela).

/// Dominios de propaganda, rastreamento e metricas.
const DOMINIOS: &[&str] = &[
    // Google: anuncios e metricas (o resto do Google continua)
    "doubleclick.net",
    "googlesyndication.com",
    "googleadservices.com",
    "googletagmanager.com",
    "googletagservices.com",
    "google-analytics.com",
    "adservice.google.com",
    "imasdk.googleapis.com",
    // leiloes e redes de anuncios
    "adnxs.com",
    "amazon-adsystem.com",
    "criteo.com",
    "criteo.net",
    "taboola.com",
    "outbrain.com",
    "rubiconproject.com",
    "pubmatic.com",
    "openx.net",
    "casalemedia.com",
    "indexww.com",
    "adsrvr.org",
    "smartadserver.com",
    "teads.tv",
    "adform.net",
    "mathtag.com",
    "bidswitch.net",
    "3lift.com",
    "sharethrough.com",
    "onetag-sys.com",
    "yieldmo.com",
    "media.net",
    "adsafeprotected.com",
    "doubleverify.com",
    "moatads.com",
    "id5-sync.com",
    "permutive.app",
    "permutive.com",
    "liadm.com",
    "rlcdn.com",
    "demdex.net",
    "everesttech.net",
    "tapad.com",
    "crwdcntrl.net",
    "zemanta.com",
    "mgid.com",
    // redes sociais (pixels e botoes de rastreio)
    "connect.facebook.net",
    "ads-twitter.com",
    "analytics.tiktok.com",
    "ads.linkedin.com",
    "snap.licdn.com",
    "bat.bing.com",
    // metricas e gravacao de sessao
    "scorecardresearch.com",
    "quantserve.com",
    "hotjar.com",
    "hotjar.io",
    "clarity.ms",
    "chartbeat.com",
    "chartbeat.net",
    "nr-data.net",
    "segment.com",
    "segment.io",
    "mixpanel.com",
    "amplitude.com",
    "mrf.io",
    "newrelic.com",
    "cxense.com",
    "parsely.com",
    "onesignal.com",
    // os do UOL que so carregam anuncios
    "tm.jsuol.com.br",
];

/// Pedacos de caminho que so aparecem em anuncio.
const CAMINHOS: &[&str] = &["prebid", "/pagead/", "/gpt.js", "pubads_impl", "/ads.js", "adsbygoogle"];

/// O servidor de um endereco http(s)://servidor[:porta]/...
fn servidor(url: &str) -> &str {
    let resto = url.split_once("://").map_or(url, |(_, r)| r);
    let fim = resto.find(['/', '?', '#']).unwrap_or(resto.len());
    let host = &resto[..fim];
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    host.split(':').next().unwrap_or(host)
}

/// Este recurso e de anuncio ou rastreamento?
pub fn bloqueado(url: &str) -> bool {
    let host = servidor(url);
    for d in DOMINIOS {
        if host.len() >= d.len() && host[host.len() - d.len()..].eq_ignore_ascii_case(d) {
            // o dominio inteiro ou um subdominio dele (nao "xdoubleclick.net")
            if host.len() == d.len() || host.as_bytes()[host.len() - d.len() - 1] == b'.' {
                return true;
            }
        }
    }
    let caminho = &url[url.len().min(url.find("://").map_or(0, |i| i + 3) + host.len())..];
    CAMINHOS.iter().any(|c| caminho.to_ascii_lowercase().contains(c))
}
