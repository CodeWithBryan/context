use crate::{chunker::Region, languages};
use ctx_core::{ChunkKind, Language};
use std::collections::HashMap;
use std::sync::Mutex;
use tree_sitter::{Query, QueryCursor, Tree};

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

static QUERY_CACHE: std::sync::LazyLock<Mutex<HashMap<Language, Query>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

fn query_for(lang: Language) -> Query {
    let src = match lang {
        Language::TypeScript | Language::Tsx | Language::Jsx => TS_QUERY_SRC,
        Language::JavaScript => JS_QUERY_SRC,
        _ => unreachable!("query_for called on non-JS/TS language"),
    };
    Query::new(&languages::tree_sitter_language(lang), src)
        .expect("invalid ts/js tree-sitter query")
}

pub fn extract(tree: &Tree, src: &[u8], lang: Language) -> Vec<Region> {
    let mut cache = QUERY_CACHE.lock().expect("query cache poisoned");
    let query = cache.entry(lang).or_insert_with(|| query_for(lang));

    let mut cursor = QueryCursor::new();
    let mut out = Vec::new();

    // tree-sitter 0.23.2 QueryMatches implements the standard Iterator trait
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
