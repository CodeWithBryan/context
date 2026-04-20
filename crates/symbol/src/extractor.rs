use ctx_core::{Language, Result, Symbol};
use ctx_parse::detect;
use std::path::Path;

use crate::tree_symbols;

pub fn extract_from_file(file: &str, bytes: &[u8]) -> Result<Vec<Symbol>> {
    let path = Path::new(file);
    let Some(lang) = detect(path) else {
        return Ok(vec![]);
    };
    match lang {
        Language::Css => tree_symbols::extract_css(file, bytes),
        Language::Html => tree_symbols::extract_html(file, bytes),
        // TS/JS/JSON: handled elsewhere (tsserver bridge or no symbols).
        _ => Ok(vec![]),
    }
}
