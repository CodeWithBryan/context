use crate::chunker::Region;
use ctx_core::ChunkKind;

#[must_use]
pub fn extract(bytes: &[u8]) -> Vec<Region> {
    if bytes.is_empty() {
        return vec![];
    }
    // Count newlines via split to avoid the naive_bytecount clippy lint.
    let newline_count = bytes.split(|b| *b == b'\n').count().saturating_sub(1);
    let line_count = u32::try_from(newline_count).unwrap_or(u32::MAX);
    vec![Region {
        kind: ChunkKind::Document,
        name: None,
        byte_start: 0,
        byte_end: bytes.len(),
        line_start: 0,
        line_end: line_count + 1,
    }]
}
