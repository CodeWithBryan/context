use crate::{chunker::Region, languages};
use ctx_core::{ChunkKind, Language};
use std::sync::LazyLock;
use tree_sitter::{Query, QueryCursor, Tree};

// Note: `ChunkKind::Const` is intentionally NOT emitted by this extractor.
// Arrow-function consts are captured as `@arrow` and become `ChunkKind::Function`.
// Non-function consts (e.g. `const MAX_RETRIES = 5`) are skipped for now —
// they provide low signal for code-focused embeddings. Revisit if symbol
// search needs them.

const TS_QUERY_SRC: &str = r"
(function_declaration name: (identifier) @name) @function
(method_definition name: (property_identifier) @name) @method
(class_declaration name: (type_identifier) @name) @class
(interface_declaration name: (type_identifier) @name) @interface
(type_alias_declaration name: (type_identifier) @name) @type
(enum_declaration name: (identifier) @name) @enum
(lexical_declaration (variable_declarator name: (identifier) @name value: (arrow_function))) @arrow
";

const JS_QUERY_SRC: &str = r"
(function_declaration name: (identifier) @name) @function
(method_definition name: (property_identifier) @name) @method
(class_declaration name: (identifier) @name) @class
(lexical_declaration (variable_declarator name: (identifier) @name value: (arrow_function))) @arrow
";

// One compiled Query per (grammar, query-source) combination.
// JS and TSX use different grammars so we build four queries total.
static TS_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&languages::tree_sitter_language(Language::TypeScript), TS_QUERY_SRC)
        .expect("invalid typescript query")
});

static TSX_QUERY: LazyLock<Query> = LazyLock::new(|| {
    // Tsx grammar is also used for Jsx.
    Query::new(&languages::tree_sitter_language(Language::Tsx), TS_QUERY_SRC)
        .expect("invalid tsx query")
});

static JS_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&languages::tree_sitter_language(Language::JavaScript), JS_QUERY_SRC)
        .expect("invalid javascript query")
});

fn query_for(lang: Language) -> &'static Query {
    match lang {
        Language::TypeScript => &TS_QUERY,
        Language::Tsx | Language::Jsx => &TSX_QUERY,
        Language::JavaScript => &JS_QUERY,
        _ => unreachable!("query_for called on non-JS/TS language"),
    }
}

#[must_use]
pub fn extract(tree: &Tree, src: &[u8], lang: Language) -> Vec<Region> {
    let query = query_for(lang);
    let mut cursor = QueryCursor::new();
    let mut out = Vec::new();
    for m in cursor.matches(query, tree.root_node(), src) {
        let mut kind: Option<ChunkKind> = None;
        let mut name: Option<String> = None;
        let mut node: Option<tree_sitter::Node> = None;
        for cap in m.captures {
            let cap_name = query.capture_names()[cap.index as usize];
            match cap_name {
                "name" => {
                    name = Some(cap.node.utf8_text(src).unwrap_or("").to_string());
                }
                "function" | "arrow" => {
                    kind = Some(ChunkKind::Function);
                    node = Some(cap.node);
                }
                "method" => {
                    kind = Some(ChunkKind::Method);
                    node = Some(cap.node);
                }
                "class" => {
                    kind = Some(ChunkKind::Class);
                    node = Some(cap.node);
                }
                "interface" => {
                    kind = Some(ChunkKind::Interface);
                    node = Some(cap.node);
                }
                "type" => {
                    kind = Some(ChunkKind::Type);
                    node = Some(cap.node);
                }
                "enum" => {
                    kind = Some(ChunkKind::Enum);
                    node = Some(cap.node);
                }
                _ => {}
            }
        }
        if let (Some(kind), Some(node)) = (kind, node) {
            out.push(Region {
                kind,
                name,
                byte_start: node.start_byte(),
                byte_end: node.end_byte(),
                line_start: u32::try_from(node.start_position().row).unwrap_or(u32::MAX),
                line_end: u32::try_from(node.end_position().row).unwrap_or(u32::MAX) + 1,
            });
        }
    }
    out
}
