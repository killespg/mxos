#[test]
fn depurar() {
    let url = std::env::var("URL").unwrap_or("https://pt.wikipedia.org/wiki/Brasil".into());
    let prof: usize = std::env::var("PROF").ok().and_then(|p| p.parse().ok()).unwrap_or(8);
    let pag = teste_web::render::montar(&url, 1280);
    let s = teste_web::render::despejar(&pag, prof);
    std::fs::write(teste_web::png::pasta().join("arvore.txt"), &s).unwrap();
}
