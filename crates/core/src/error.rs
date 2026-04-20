#[derive(Debug, thiserror::Error)]
pub enum CtxError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse: {0}")]
    Parse(String),
    #[error("store: {0}")]
    Store(String),
    #[error("embed: {0}")]
    Embed(String),
    #[error("symbol: {0}")]
    Symbol(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type Result<T, E = CtxError> = std::result::Result<T, E>;
