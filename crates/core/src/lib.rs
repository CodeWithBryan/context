//! ctx-core — shared types, traits, scope, hashing, errors. Implemented in Tasks 1-2.

pub mod error;
pub mod hash;
pub mod scope;
pub mod types;

pub use error::{CtxError, Result};
pub use hash::ContentHash;
pub use scope::Scope;
pub use types::*;
