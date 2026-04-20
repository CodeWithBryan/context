use crate::hash::ContentHash;
use serde::{Deserialize, Serialize};
use std::ops::Range;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Chunk {
    pub hash: ContentHash,
    pub file: String,
    pub lang: Language,
    pub kind: ChunkKind,
    pub name: Option<String>,
    pub byte_range: Range<usize>,
    pub line_range: Range<u32>,
    pub text: String,
    pub vector: Option<Vec<f32>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Language {
    TypeScript,
    Tsx,
    JavaScript,
    Jsx,
    Css,
    Html,
    Json,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChunkKind {
    Function,
    Method,
    Class,
    Interface,
    Type,
    Const,
    Enum,
    Selector,
    Element,
    Document,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: ChunkKind,
    pub file: String,
    pub line: u32,
    pub container: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChunkRef {
    pub hash: ContentHash,
    pub file: String,
    pub line_range: Range<u32>,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub chunk: Chunk,
    pub score: f32,
}
