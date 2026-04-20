use crate::{css, html, json, languages, ts};
use ctx_core::types::{ByteRange, LineRange};
use ctx_core::{Chunk, ChunkKind, ContentHash, CtxError, Language, Result};
use std::path::Path;
use tree_sitter::Parser;

pub struct Chunker;

impl Chunker {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    pub fn chunk(&self, file: &str, bytes: &[u8]) -> Result<Vec<Chunk>> {
        let path = Path::new(file);
        let lang = languages::detect(path)
            .ok_or_else(|| CtxError::Parse(format!("no grammar for {file}")))?;
        let mut parser = Parser::new();
        parser
            .set_language(&languages::tree_sitter_language(lang))
            .map_err(|e| CtxError::Parse(e.to_string()))?;
        let tree = parser
            .parse(bytes, None)
            .ok_or_else(|| CtxError::Parse("parse returned None".into()))?;

        let regions = match lang {
            Language::TypeScript | Language::Tsx | Language::JavaScript | Language::Jsx => {
                ts::extract(&tree, bytes, lang)
            }
            Language::Css => css::extract(&tree, bytes),
            Language::Html => html::extract(&tree, bytes),
            Language::Json => json::extract(bytes),
        };

        Ok(regions
            .into_iter()
            .map(|r| Chunk {
                hash: ContentHash::of(&bytes[r.byte_start..r.byte_end]),
                file: file.to_string(),
                lang,
                kind: r.kind,
                name: r.name,
                byte_range: ByteRange::new(r.byte_start, r.byte_end),
                line_range: LineRange::new(r.line_start, r.line_end),
                text: std::str::from_utf8(&bytes[r.byte_start..r.byte_end])
                    .unwrap_or("")
                    .to_string(),
                vector: None,
            })
            .collect())
    }
}

impl Default for Chunker {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Region {
    pub kind: ChunkKind,
    pub name: Option<String>,
    pub byte_start: usize,
    pub byte_end: usize,
    pub line_start: u32,
    pub line_end: u32,
}
