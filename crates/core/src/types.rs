use crate::hash::ContentHash;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ByteRange {
    pub start: usize,
    pub end: usize,
}

impl ByteRange {
    #[must_use]
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }

    #[must_use]
    pub fn as_range(&self) -> std::ops::Range<usize> {
        self.start..self.end
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LineRange {
    pub start: u32,
    pub end: u32,
}

impl LineRange {
    #[must_use]
    pub fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    #[must_use]
    pub fn len(&self) -> u32 {
        self.end.saturating_sub(self.start)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
}

// PartialEq only — Eq and Hash are not derived because Vec<f32> does not implement
// Eq/Hash (f32::NaN breaks Eq semantics). Known limitation; revisit if vector is removed.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Chunk {
    pub hash: ContentHash,
    pub file: String,
    pub lang: Language,
    pub kind: ChunkKind,
    pub name: Option<String>,
    pub byte_range: ByteRange,
    pub line_range: LineRange,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: ChunkKind,
    pub file: String,
    pub line: u32,
    pub container: Option<String>,
}

// ChunkRef gets Eq + Hash but not Copy because `file: String` is not Copy.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChunkRef {
    pub hash: ContentHash,
    pub file: String,
    pub line_range: LineRange,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub chunk: Chunk,
    pub score: f32,
}
