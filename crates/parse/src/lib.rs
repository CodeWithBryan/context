//! ctx-parse — tree-sitter chunker for ts/tsx/js/jsx/css/html/json. Implemented in Task 3.
//!
//! JS and JSX reuse the same extractor as TypeScript/TSX (`ts.rs`), since the TSX grammar
//! covers JSX syntax and the JavaScript grammar covers plain JS. There is no separate `js.rs`.

pub mod chunker;
pub mod css;
pub mod html;
pub mod json;
pub mod languages;
pub mod ts;

pub use chunker::Chunker;
pub use languages::detect;
