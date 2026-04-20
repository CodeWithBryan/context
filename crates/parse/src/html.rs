use crate::chunker::Region;
use ctx_core::ChunkKind;
use tree_sitter::{Query, QueryCursor, Tree};

const HTML_QUERY_SRC: &str = r"
(element (start_tag (tag_name) @tag)) @element
(element (self_closing_tag (tag_name) @tag)) @element
";

static QUERY: std::sync::LazyLock<Query> = std::sync::LazyLock::new(|| {
    Query::new(&tree_sitter_html::LANGUAGE.into(), HTML_QUERY_SRC).expect("invalid html query")
});

#[must_use]
pub fn extract(tree: &Tree, src: &[u8]) -> Vec<Region> {
    let mut cursor = QueryCursor::new();
    let mut out = Vec::new();

    // tree-sitter 0.23.2 QueryMatches implements the standard Iterator trait
    for m in cursor.matches(&QUERY, tree.root_node(), src) {
        let mut tag: Option<String> = None;
        let mut node: Option<tree_sitter::Node> = None;
        for cap in m.captures {
            let cap_name = QUERY.capture_names()[cap.index as usize];
            match cap_name {
                "tag" => tag = Some(cap.node.utf8_text(src).unwrap_or("").to_string()),
                "element" => node = Some(cap.node),
                _ => {}
            }
        }
        if let Some(node) = node {
            out.push(Region {
                kind: ChunkKind::Element,
                name: tag,
                byte_start: node.start_byte(),
                byte_end: node.end_byte(),
                line_start: u32::try_from(node.start_position().row).unwrap_or(u32::MAX),
                line_end: u32::try_from(node.end_position().row).unwrap_or(u32::MAX) + 1,
            });
        }
    }
    out
}
