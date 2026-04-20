use ctx_core::hash::ContentHash;
use ctx_core::Scope;
use std::path::Path;

#[test]
fn scope_local_relative_path_returns_err() {
    let err = Scope::local(Path::new("relative/repo"), Path::new("/abs/wt"), None);
    assert!(err.is_err(), "relative repo path should be rejected");

    let err2 = Scope::local(Path::new("/abs/repo"), Path::new("relative/wt"), None);
    assert!(err2.is_err(), "relative worktree path should be rejected");
}

#[test]
fn scope_local_absolute_paths_ok() {
    let scope = Scope::local(Path::new("/abs/repo"), Path::new("/abs/wt"), None);
    assert!(scope.is_ok());
}

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
