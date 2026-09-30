//! Exact symbol navigation through `bran query` and `bran packet` (issue #38).
//!
//! The SCIP indexes used here are built by a test-only protobuf encoder whose
//! field numbers follow sourcegraph/scip `scip.proto`, so no binary fixture is
//! committed.

use super::{CliApp, ExitCode};
use std::path::{Path, PathBuf};

const RENDERER: &str = "rust-analyzer cargo demo 0.1.0 render/Renderer#";
const RENDERER_RENDER: &str = "rust-analyzer cargo demo 0.1.0 render/Renderer#render().";
const HTML: &str = "rust-analyzer cargo demo 0.1.0 render/Html#";
const HTML_RENDER: &str = "rust-analyzer cargo demo 0.1.0 render/Html#render().";
const PAGE: &str = "rust-analyzer cargo demo 0.1.0 page/page().";

const LIB_RS: &str = "pub mod page;\npub mod render;\n";
const RENDER_RS: &str = "pub trait Renderer {\n    fn render(&self) -> String;\n}\n\npub struct Html;\n\nimpl Renderer for Html {\n    fn render(&self) -> String {\n        String::from(\"<p></p>\")\n    }\n}\n";
const PAGE_RS: &str =
    "use crate::render::{Html, Renderer};\n\npub fn page() -> String {\n    Html.render()\n}\n";

const MISSING_INDEX_NAVIGATION: &str = ",\"symbol_navigation\":{\"schema_version\":\"1.0.0\",\"outcome\":\"unavailable\",\"truncated\":false,\"scip\":{\"status\":\"unavailable\",\"reason\":\"index_missing\",\"index\":\"index.scip\"},\"lsp\":{\"status\":\"unavailable\",\"reason\":\"not_implemented\"}}";

fn varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn field_bytes(out: &mut Vec<u8>, field: u64, bytes: &[u8]) {
    varint(out, (field << 3) | 2);
    varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn field_varint(out: &mut Vec<u8>, field: u64, value: u64) {
    varint(out, field << 3);
    varint(out, value);
}

/// One occurrence: `(symbol, zero-based line, start column, end column, is definition)`.
type Occurrence = (&'static str, u64, u64, u64, bool);

/// One symbol: `(symbol, SCIP Kind value, implemented symbol)`.
type Information = (&'static str, u64, Option<&'static str>);

fn occurrence((symbol, line, start, end, definition): Occurrence) -> Vec<u8> {
    let mut out = Vec::new();
    if definition {
        // Deprecated packed `range` (field 1), as rust-analyzer writes it.
        let mut range = Vec::new();
        for value in [line, start, end] {
            varint(&mut range, value);
        }
        field_bytes(&mut out, 1, &range);
        field_varint(&mut out, 3, 1);
    } else {
        // Typed `single_line_range` (field 8).
        let mut range = Vec::new();
        field_varint(&mut range, 1, line);
        field_varint(&mut range, 2, start);
        field_varint(&mut range, 3, end);
        field_bytes(&mut out, 8, &range);
    }
    field_bytes(&mut out, 2, symbol.as_bytes());
    out
}

fn information((symbol, kind, implements): Information) -> Vec<u8> {
    let mut out = Vec::new();
    field_bytes(&mut out, 1, symbol.as_bytes());
    if let Some(target) = implements {
        let mut relationship = Vec::new();
        field_bytes(&mut relationship, 1, target.as_bytes());
        field_varint(&mut relationship, 3, 1);
        field_bytes(&mut out, 4, &relationship);
    }
    field_varint(&mut out, 5, kind);
    out
}

fn document(
    path: &str,
    text: Option<&str>,
    occurrences: &[Occurrence],
    symbols: &[Information],
) -> Vec<u8> {
    let mut out = Vec::new();
    field_bytes(&mut out, 4, b"rust");
    field_bytes(&mut out, 1, path.as_bytes());
    for item in occurrences {
        field_bytes(&mut out, 2, &occurrence(*item));
    }
    for item in symbols {
        field_bytes(&mut out, 3, &information(*item));
    }
    if let Some(text) = text {
        field_bytes(&mut out, 5, text.as_bytes());
    }
    out
}

fn index(documents: &[Vec<u8>]) -> Vec<u8> {
    let mut tool = Vec::new();
    field_bytes(&mut tool, 1, b"rust-analyzer");
    let mut metadata = Vec::new();
    field_bytes(&mut metadata, 2, &tool);
    field_bytes(&mut metadata, 3, b"file:///demo");
    let mut out = Vec::new();
    field_bytes(&mut out, 1, &metadata);
    for item in documents {
        field_bytes(&mut out, 2, item);
    }
    out
}

/// The demo crate index. `with_text` embeds each document's source so BRAN
/// can prove the index matches the scanned files.
fn demo_index(with_text: bool) -> Vec<u8> {
    let text = |source: &'static str| with_text.then_some(source);
    index(&[
        document("src/lib.rs", text(LIB_RS), &[], &[]),
        document(
            "src/render.rs",
            text(RENDER_RS),
            &[
                (RENDERER, 0, 10, 18, true),
                (RENDERER_RENDER, 1, 7, 13, true),
                (HTML, 4, 11, 15, true),
                (RENDERER, 6, 5, 13, false),
                (HTML, 6, 18, 22, false),
                (HTML_RENDER, 7, 7, 13, true),
            ],
            &[
                (RENDERER, 53, None),
                (RENDERER_RENDER, 70, None),
                (HTML, 49, Some(RENDERER)),
                (HTML_RENDER, 26, Some(RENDERER_RENDER)),
            ],
        ),
        document(
            "src/page.rs",
            text(PAGE_RS),
            &[
                (HTML, 0, 20, 24, false),
                (RENDERER, 0, 26, 34, false),
                (PAGE, 2, 7, 11, true),
                (HTML, 3, 4, 8, false),
                (HTML_RENDER, 3, 9, 15, false),
            ],
            &[(PAGE, 17, None)],
        ),
    ])
}

fn demo_repository(name: &str, index: Option<&[u8]>) -> PathBuf {
    let root = std::env::temp_dir().join(format!("bran-scip-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".bran")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join(".bran/policy.yaml"), "schema_version: \"1\"\n").unwrap();
    std::fs::write(root.join("src/lib.rs"), LIB_RS).unwrap();
    std::fs::write(root.join("src/render.rs"), RENDER_RS).unwrap();
    std::fs::write(root.join("src/page.rs"), PAGE_RS).unwrap();
    if let Some(index) = index {
        std::fs::write(root.join("index.scip"), index).unwrap();
    }
    root
}

fn run(command: &str, root: &Path, query: &str) -> String {
    let result = CliApp::run(vec![
        command.to_owned(),
        root.to_string_lossy().into_owned(),
        query.to_owned(),
    ]);
    assert_eq!(result.exit_code, ExitCode::SUCCESS, "{}", result.output);
    assert!(!result.is_error, "{}", result.output);
    result.output
}

/// The `symbols` evidence list attached to one ranked source.
fn ranked_symbols(output: &str, locator: &str) -> String {
    let start = output
        .find(&format!("{{\"locator\":\"{locator}\",\"rank\":"))
        .unwrap_or_else(|| panic!("{locator} is not ranked: {output}"));
    let ranked = &output[start..];
    let ranked = &ranked[..ranked[1..]
        .find("{\"locator\":")
        .map_or(ranked.len(), |end| end + 1)];
    let symbols = ranked
        .split_once(",\"symbols\":[")
        .unwrap_or_else(|| panic!("{locator} carries no symbols: {output}"))
        .1;
    symbols[..symbols.find("}]").unwrap() + 1].to_owned()
}

fn navigation(output: &str) -> &str {
    let start = output
        .find("\"symbol_navigation\":")
        .unwrap_or_else(|| panic!("no symbol_navigation: {output}"));
    let navigation = &output[start..];
    &navigation[..navigation.find("}}").unwrap() + 2]
}

#[test]
fn p8_scip_symbols() {
    let root = demo_repository("hit", Some(&demo_index(true)));
    let fixture = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/symbols/symbol-navigation-v1.json"),
    )
    .unwrap();
    for command in ["query", "packet"] {
        let output = run(command, &root, "Renderer");
        assert_eq!(
            navigation(&output),
            format!("\"symbol_navigation\":{}", fixture.trim()),
            "{command}"
        );
        assert!(
            output.contains(
                "\"provenance\":{\"sources\":[\"repository-scanner\",\"bran-core\",\"scip\"]"
            ),
            "{output}"
        );
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn scip_query_resolves_definition_with_span() {
    let root = demo_repository("definition", Some(&demo_index(true)));
    let output = run("query", &root, "Renderer");
    assert!(
        ranked_symbols(&output, "src/render.rs").starts_with(&format!(
            "{{\"role\":\"definition\",\"name\":\"Renderer\",\"qualified_name\":\"render::Renderer\",\"kind\":\"trait\",\"id\":\"{RENDERER}\",\"source\":\"scip\",\"span\":{{\"start_line\":1,\"end_line\":1}}}}"
        )),
        "{output}"
    );
    assert!(navigation(&output).contains("\"outcome\":\"hit\""));
    assert!(navigation(&output).contains("\"status\":\"available\",\"reason\":null"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn scip_query_returns_references() {
    let root = demo_repository("references", Some(&demo_index(true)));
    let output = run("query", &root, "Renderer");
    assert_eq!(
        ranked_symbols(&output, "src/page.rs"),
        format!("{{\"role\":\"reference\",\"name\":\"Renderer\",\"qualified_name\":\"render::Renderer\",\"kind\":\"trait\",\"id\":\"{RENDERER}\",\"source\":\"scip\",\"span\":{{\"start_line\":1,\"end_line\":1}}}}")
    );
    assert!(
        ranked_symbols(&output, "src/render.rs").contains(&format!(
            "{{\"role\":\"reference\",\"name\":\"Renderer\",\"qualified_name\":\"render::Renderer\",\"kind\":\"trait\",\"id\":\"{RENDERER}\",\"source\":\"scip\",\"span\":{{\"start_line\":7,\"end_line\":7}}}}"
        )),
        "{output}"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn scip_query_returns_trait_implementations() {
    let root = demo_repository("implementations", Some(&demo_index(true)));
    let output = run("query", &root, "Renderer");
    assert!(
        ranked_symbols(&output, "src/render.rs").contains(&format!(
            "{{\"role\":\"implementation\",\"name\":\"Html\",\"qualified_name\":\"render::Html\",\"kind\":\"struct\",\"id\":\"{HTML}\",\"source\":\"scip\",\"span\":{{\"start_line\":5,\"end_line\":5}},\"implements\":\"{RENDERER}\"}}"
        )),
        "{output}"
    );
    let method = run("query", &root, "render");
    assert!(
        ranked_symbols(&method, "src/render.rs").contains(&format!(
            "{{\"role\":\"implementation\",\"name\":\"render\",\"qualified_name\":\"render::Html::render\",\"kind\":\"method\",\"id\":\"{HTML_RENDER}\",\"source\":\"scip\",\"span\":{{\"start_line\":8,\"end_line\":8}},\"implements\":\"{RENDERER_RENDER}\"}}"
        )),
        "{method}"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn scip_packet_carries_symbol_evidence() {
    let root = demo_repository("packet", Some(&demo_index(true)));
    let output = run("packet", &root, "Renderer");
    assert!(ranked_symbols(&output, "src/render.rs").contains("\"role\":\"definition\""));
    assert!(
        output.contains("scip_symbols: definition render::Renderer 1-1; implementation render::Html 5-5 implements render::Renderer; reference render::Renderer 7-7\\n"),
        "{output}"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn scip_missing_index_is_typed_unavailable() {
    let root = demo_repository("missing", None);
    for command in ["query", "packet"] {
        let output = run(command, &root, "Renderer");
        assert!(output.contains(MISSING_INDEX_NAVIGATION), "{output}");
        assert!(!output.contains("\"symbols\":"), "{output}");
    }
    let other = demo_repository("missing-other", None);
    let multi = CliApp::run(vec![
        "query".to_owned(),
        root.to_string_lossy().into_owned(),
        "--add-dir".to_owned(),
        other.to_string_lossy().into_owned(),
        "Renderer".to_owned(),
    ]);
    assert!(
        navigation(&multi.output)
            .contains("\"scip\":{\"status\":\"unavailable\",\"reason\":\"multi_root_unsupported\""),
        "{}",
        multi.output
    );
    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(other);
}

#[test]
fn scip_corrupt_index_is_typed_unavailable() {
    // Field 2 claims a 127-byte document but the index ends after one byte.
    let root = demo_repository("corrupt", Some(&[0x12, 0x7f, 0x0a]));
    for command in ["query", "packet"] {
        let output = run(command, &root, "Renderer");
        assert!(
            navigation(&output).contains("\"outcome\":\"unavailable\",\"truncated\":false,\"scip\":{\"status\":\"unavailable\",\"reason\":\"index_unreadable\""),
            "{output}"
        );
        assert!(!output.contains("\"symbols\":"), "{output}");
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn scip_stale_index_withholds_symbol_evidence() {
    let root = demo_repository("stale", Some(&demo_index(true)));
    std::fs::write(root.join("src/render.rs"), format!("// moved\n{RENDER_RS}")).unwrap();
    for command in ["query", "packet"] {
        let output = run(command, &root, "Renderer");
        assert!(
            navigation(&output).contains("\"outcome\":\"unavailable\",\"truncated\":false,\"scip\":{\"status\":\"stale\",\"reason\":\"index_stale\""),
            "{output}"
        );
        assert!(!output.contains("\"symbols\":"), "{output}");
        assert!(!output.contains("scip_symbols:"), "{output}");
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn scip_index_without_text_is_partial() {
    let root = demo_repository("partial", Some(&demo_index(false)));
    let output = run("query", &root, "Renderer");
    assert!(
        navigation(&output).contains("\"outcome\":\"hit\",\"truncated\":false,\"scip\":{\"status\":\"partial\",\"reason\":\"freshness_unverified\""),
        "{output}"
    );
    assert!(ranked_symbols(&output, "src/render.rs").contains("\"role\":\"definition\""));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn scip_unknown_symbol_is_typed_miss() {
    let root = demo_repository("unknown", Some(&demo_index(true)));
    let output = run("query", &root, "String");
    assert!(
        output.contains("\"query_outcome\":\"grounded\""),
        "{output}"
    );
    assert!(
        navigation(&output).contains("\"outcome\":\"miss\",\"truncated\":false,\"scip\":{\"status\":\"available\",\"reason\":null"),
        "{output}"
    );
    assert!(!output.contains("\"symbols\":"), "{output}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn scip_symbol_evidence_is_bounded() {
    let mut source = String::from("pub fn tick() {}\n");
    let mut occurrences = vec![("rust-analyzer cargo demo 0.1.0 tick().", 0, 7, 11, true)];
    for line in 1..=70u64 {
        source.push_str("fn caller() { tick(); }\n");
        occurrences.push((
            "rust-analyzer cargo demo 0.1.0 tick().",
            line,
            14,
            18,
            false,
        ));
    }
    let source: &'static str = Box::leak(source.into_boxed_str());
    let root = demo_repository(
        "bounded",
        Some(&index(&[document(
            "src/tick.rs",
            Some(source),
            &occurrences,
            &[],
        )])),
    );
    std::fs::write(root.join("src/tick.rs"), source).unwrap();
    let output = run("query", &root, "tick");
    assert_eq!(
        ranked_symbols(&output, "src/tick.rs")
            .matches("\"source\":\"scip\"")
            .count(),
        64
    );
    assert!(navigation(&output).contains("\"outcome\":\"hit\",\"truncated\":true"));
    let _ = std::fs::remove_dir_all(root);
}

/// With no index, every pre-existing byte of `query` and `packet` output is
/// what origin/main (0dd8944) produced, captured in the baseline fixtures with
/// the scratch root replaced by `ROOT`; only the typed `symbol_navigation`
/// member is added.
#[test]
fn scip_absent_index_keeps_existing_output() {
    let root = demo_repository("unchanged", None);
    let root_text = root.to_string_lossy().into_owned();
    for command in ["query", "packet"] {
        let expected = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../../fixtures/symbols/baseline-{command}-v1.json")),
        )
        .unwrap();
        let output = run(command, &root, "Renderer");
        let existing = output
            .replacen(MISSING_INDEX_NAVIGATION, "", 1)
            .replace(&root_text, "ROOT");
        assert_eq!(existing, expected.trim_end(), "{command}");
    }
    let _ = std::fs::remove_dir_all(root);
}
