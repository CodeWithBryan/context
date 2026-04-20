use ctx_parse::Chunker;
use std::fs;

#[test]
fn chunks_ts_functions_and_methods() {
    let src = fs::read_to_string("tests/fixtures/sample.ts").unwrap();
    let chunks = Chunker::new().chunk("tests/fixtures/sample.ts", src.as_bytes()).unwrap();

    let names: Vec<_> = chunks.iter().filter_map(|c| c.name.clone()).collect();
    assert!(names.contains(&"greet".to_string()), "missing greet, got: {names:?}");
    assert!(names.contains(&"Counter".to_string()), "missing Counter, got: {names:?}");
    assert!(names.contains(&"increment".to_string()), "missing increment, got: {names:?}");
}

#[test]
fn chunks_tsx_component() {
    let src = fs::read_to_string("tests/fixtures/sample.tsx").unwrap();
    let chunks = Chunker::new().chunk("tests/fixtures/sample.tsx", src.as_bytes()).unwrap();
    let names: Vec<_> = chunks.iter().filter_map(|c| c.name.clone()).collect();
    assert!(names.contains(&"Button".to_string()), "missing Button, got: {names:?}");
}

#[test]
fn chunks_css_selectors() {
    let src = fs::read_to_string("tests/fixtures/sample.css").unwrap();
    let chunks = Chunker::new().chunk("tests/fixtures/sample.css", src.as_bytes()).unwrap();
    assert!(!chunks.is_empty());
    let names: Vec<_> = chunks.iter().filter_map(|c| c.name.clone()).collect();
    assert!(names.iter().any(|n| n.contains(".btn-primary")));
}

#[test]
fn chunks_html_elements() {
    let src = fs::read_to_string("tests/fixtures/sample.html").unwrap();
    let chunks = Chunker::new().chunk("tests/fixtures/sample.html", src.as_bytes()).unwrap();
    let tags: Vec<_> = chunks.iter().filter_map(|c| c.name.clone()).collect();
    assert!(tags.iter().any(|t| t == "div"), "missing div element, got: {tags:?}");
}

#[test]
fn chunks_json_as_document() {
    let src = fs::read_to_string("tests/fixtures/sample.json").unwrap();
    let chunks = Chunker::new().chunk("tests/fixtures/sample.json", src.as_bytes()).unwrap();
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].kind, ctx_core::ChunkKind::Document);
}

#[test]
fn chunk_hash_is_content_addressed() {
    let src = fs::read_to_string("tests/fixtures/sample.ts").unwrap();
    let a = Chunker::new().chunk("a.ts", src.as_bytes()).unwrap();
    let b = Chunker::new().chunk("b.ts", src.as_bytes()).unwrap();
    assert_eq!(a.len(), b.len());
    // chunks with same bytes produce same hash regardless of file path
    for (x, y) in a.iter().zip(b.iter()) {
        assert_eq!(x.hash, y.hash);
    }
}

#[test]
fn chunker_rejects_unknown_extension() {
    let err = Chunker::new().chunk("sample.py", b"def foo(): pass").unwrap_err();
    assert!(matches!(err, ctx_core::CtxError::Parse(_)));
}
