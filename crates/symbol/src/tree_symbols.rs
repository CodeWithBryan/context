use ctx_core::{ChunkKind, CtxError, Result, Symbol};
use std::sync::LazyLock;
use tree_sitter::{Parser, Query, QueryCursor, Tree};

// --- CSS ---

const CSS_SYMBOL_QUERY_SRC: &str = r"
(class_selector (class_name) @class_name)
(id_selector (id_name) @id_name)
(tag_name) @tag
";

static CSS_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&tree_sitter_css::LANGUAGE.into(), CSS_SYMBOL_QUERY_SRC)
        .expect("invalid css symbol query")
});

pub fn extract_css(file: &str, src: &[u8]) -> Result<Vec<Symbol>> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_css::LANGUAGE.into())
        .map_err(|e| CtxError::Symbol(e.to_string()))?;
    let tree = parser
        .parse(src, None)
        .ok_or_else(|| CtxError::Symbol("css parse returned None".into()))?;
    Ok(walk_css(&tree, src, file))
}

fn walk_css(tree: &Tree, src: &[u8], file: &str) -> Vec<Symbol> {
    let mut cursor = QueryCursor::new();
    let mut out = Vec::new();
    for m in cursor.matches(&CSS_QUERY, tree.root_node(), src) {
        for cap in m.captures {
            let capture_name = CSS_QUERY.capture_names()[cap.index as usize];
            let text = cap.node.utf8_text(src).unwrap_or("").to_string();
            // Convert 0-indexed row → 1-indexed line number
            let line =
                u32::try_from(cap.node.start_position().row).unwrap_or(u32::MAX).saturating_add(1);
            match capture_name {
                "class_name" => {
                    out.push(Symbol {
                        name: format!(".{text}"),
                        kind: ChunkKind::Selector,
                        file: file.to_string(),
                        line,
                        container: None,
                    });
                }
                "id_name" => {
                    out.push(Symbol {
                        name: format!("#{text}"),
                        kind: ChunkKind::Selector,
                        file: file.to_string(),
                        line,
                        container: None,
                    });
                }
                "tag" => {
                    out.push(Symbol {
                        name: text,
                        kind: ChunkKind::Selector,
                        file: file.to_string(),
                        line,
                        container: None,
                    });
                }
                _ => {}
            }
        }
    }
    out
}

// --- HTML ---

const HTML_SYMBOL_QUERY_SRC: &str = r"
(element (start_tag (tag_name) @tag))
(element (self_closing_tag (tag_name) @tag))
(attribute (attribute_name) @attr_name (quoted_attribute_value (attribute_value) @attr_value))
";

static HTML_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&tree_sitter_html::LANGUAGE.into(), HTML_SYMBOL_QUERY_SRC)
        .expect("invalid html symbol query")
});

pub fn extract_html(file: &str, src: &[u8]) -> Result<Vec<Symbol>> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_html::LANGUAGE.into())
        .map_err(|e| CtxError::Symbol(e.to_string()))?;
    let tree = parser
        .parse(src, None)
        .ok_or_else(|| CtxError::Symbol("html parse returned None".into()))?;
    Ok(walk_html(&tree, src, file))
}

fn walk_html(tree: &Tree, src: &[u8], file: &str) -> Vec<Symbol> {
    let mut cursor = QueryCursor::new();
    let mut out = Vec::new();
    for m in cursor.matches(&HTML_QUERY, tree.root_node(), src) {
        // Group captures in this match to pair attr_name with attr_value
        let mut tag: Option<(String, u32)> = None;
        let mut attr_name: Option<String> = None;
        let mut attr_value: Option<(String, u32)> = None;
        for cap in m.captures {
            let capture_name = HTML_QUERY.capture_names()[cap.index as usize];
            let text = cap.node.utf8_text(src).unwrap_or("");
            let line =
                u32::try_from(cap.node.start_position().row).unwrap_or(u32::MAX).saturating_add(1);
            match capture_name {
                "tag" => tag = Some((text.to_string(), line)),
                "attr_name" => attr_name = Some(text.to_string()),
                "attr_value" => attr_value = Some((text.to_string(), line)),
                _ => {}
            }
        }
        if let Some((tag_name, line)) = tag {
            out.push(Symbol {
                name: tag_name,
                kind: ChunkKind::Element,
                file: file.to_string(),
                line,
                container: None,
            });
        }
        if let (Some(name), Some((value, line))) = (&attr_name, &attr_value) {
            match name.as_str() {
                "id" => out.push(Symbol {
                    name: format!("#{value}"),
                    kind: ChunkKind::Selector,
                    file: file.to_string(),
                    line: *line,
                    container: None,
                }),
                "class" => {
                    // Class attribute may contain multiple space-separated classes.
                    for class in value.split_whitespace() {
                        out.push(Symbol {
                            name: format!(".{class}"),
                            kind: ChunkKind::Selector,
                            file: file.to_string(),
                            line: *line,
                            container: None,
                        });
                    }
                }
                _ => {}
            }
        }
    }
    out
}
