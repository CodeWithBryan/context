use ctx_core::traits::Embedder;
use ctx_embed::FastembedEmbedder;

#[tokio::test]
#[ignore = "downloads ~150MB model on first run; run with `cargo test --ignored -p ctx-embed`"]
async fn embeds_two_strings_with_expected_dim() {
    let embedder = FastembedEmbedder::new_default().await.expect("init embedder");
    let out = embedder
        .embed(&[
            "function add(a: number, b: number) { return a + b; }",
            "const sum = (x, y) => x + y",
        ])
        .await
        .expect("embed call");
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].len(), embedder.dim());
    assert!(
        out[0].len() >= 384,
        "expected embedding dim >= 384, got {}",
        out[0].len()
    );
}
