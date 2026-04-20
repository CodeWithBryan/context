use ctx_core::traits::{ChunkStore, Embedder, Filter, RefStore};

// Compile-only: prove the traits are object-safe-ish (boxed behind async-trait).
fn _assert_trait_objects(
    _: Box<dyn ChunkStore>,
    _: Box<dyn RefStore>,
    _: Box<dyn Embedder>,
    _: Filter,
) {
}
