use ctx_core::hash::ContentHash;

#[test]
fn content_hash_is_deterministic() {
    let a = ContentHash::of(b"hello world");
    let b = ContentHash::of(b"hello world");
    assert_eq!(a, b);
    assert_eq!(a.to_hex().len(), 64);
}

#[test]
fn content_hash_differs_for_different_input() {
    let a = ContentHash::of(b"hello");
    let b = ContentHash::of(b"world");
    assert_ne!(a, b);
}
