use crate::chunker::Region;
use ctx_core::ChunkKind;
use tree_sitter::{Query, QueryCursor, Tree};

const CSS_QUERY_SRC: &str = r"
(rule_set (selectors) @sel) @rule
";

static QUERY: std::sync::LazyLock<Query> = std::sync::LazyLock::new(|| {
    Query::new(&tree_sitter_css::LANGUAGE.into(), CSS_QUERY_SRC).expect("invalid css query")
});

#[must_use]
pub fn extract(tree: &Tree, src: &[u8]) -> Vec<Region> {
    let mut cursor = QueryCursor::new();
    let mut out = Vec::new();

    // tree-sitter 0.23.2 QueryMatches implements the standard Iterator trait
    for m in cursor.matches(&QUERY, tree.root_node(), src) {
        let mut selector: Option<String> = None;
        let mut node: Option<tree_sitter::Node> = None;
        for cap in m.captures {
            let cap_name = QUERY.capture_names()[cap.index as usize];
            match cap_name {
                "sel" => {
                    selector = Some(
                        cap.node.utf8_text(src).unwrap_or("").trim().to_string(),
                    );
                }
                "rule" => node = Some(cap.node),
                _ => {}
            }
        }
        if let Some(node) = node {
            out.push(Region {
                kind: ChunkKind::Selector,
                name: selector,
                byte_start: node.start_byte(),
                byte_end: node.end_byte(),
                line_start: u32::try_from(node.start_position().row).unwrap_or(u32::MAX),
                line_end: u32::try_from(node.end_position().row).unwrap_or(u32::MAX) + 1,
            });
        }
    }
    out
}
