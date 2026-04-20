use ctx_core::ChunkKind;
use ctx_symbol::tree_symbols;

#[test]
fn extracts_css_class_and_id_selectors() {
    let css = r".btn-primary { color: red; } #hero h1 { font-size: 2rem; }";
    let symbols = tree_symbols::extract_css("theme.css", css.as_bytes()).unwrap();
    let names: Vec<_> = symbols.iter().map(|s| s.name.clone()).collect();
    // Expect at least the .btn-primary class and the #hero id
    assert!(
        names.iter().any(|n| n == ".btn-primary"),
        "missing .btn-primary, got: {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "#hero"),
        "missing #hero, got: {names:?}"
    );
    // Selectors are reported as ChunkKind::Selector
    assert!(symbols.iter().all(|s| s.kind == ChunkKind::Selector));
}

#[test]
fn extracts_html_tag_id_and_class_symbols() {
    let html = r#"<div id="app" class="root layout"><span>hi</span></div>"#;
    let symbols = tree_symbols::extract_html("index.html", html.as_bytes()).unwrap();
    let names: Vec<_> = symbols.iter().map(|s| s.name.clone()).collect();
    // Expect at least: the 'div' tag (Element), '#app' id, '.root' class, '.layout' class, 'span' tag
    assert!(
        names.iter().any(|n| n == "div"),
        "missing div, got: {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "span"),
        "missing span, got: {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "#app"),
        "missing #app, got: {names:?}"
    );
    assert!(
        names.iter().any(|n| n == ".root"),
        "missing .root class, got: {names:?}"
    );
    assert!(
        names.iter().any(|n| n == ".layout"),
        "missing .layout class, got: {names:?}"
    );
}

#[test]
fn css_selectors_include_line_numbers() {
    let css = "\n\n.btn { color: red; }\n";
    let symbols = tree_symbols::extract_css("f.css", css.as_bytes()).unwrap();
    let btn = symbols.iter().find(|s| s.name == ".btn").expect("has .btn");
    // Line 3 (1-indexed) — the selector is on the 3rd line
    assert_eq!(btn.line, 3, "expected line 3, got {}", btn.line);
}
