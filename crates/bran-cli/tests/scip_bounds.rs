//! SCIP decoding and evidence collection stay bounded under a 192 MiB
//! address-space limit (alphazede/bran#38 review findings 2 and 3). The
//! inputs are the reviewer's reproducers.
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::Command;

const RENDERER: &[u8] = b"rust-analyzer cargo demo 0.1.0 render/Renderer#";
const HTML: &[u8] = b"rust-analyzer cargo demo 0.1.0 render/Html#";
const TEXT: &[u8] = b"pub trait Renderer {}\npub struct Html;\n";

fn varint(mut value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
    out
}

fn field_bytes(field: u64, bytes: &[u8]) -> Vec<u8> {
    [
        varint((field << 3) | 2),
        varint(bytes.len() as u64),
        bytes.to_vec(),
    ]
    .concat()
}

fn field_varint(field: u64, value: u64) -> Vec<u8> {
    [varint(field << 3), varint(value)].concat()
}

fn definition(symbol: &[u8], line: u64) -> Vec<u8> {
    let range = [varint(line), varint(10), varint(18)].concat();
    [
        field_bytes(1, &range),
        field_bytes(2, symbol),
        field_varint(3, 1),
    ]
    .concat()
}

fn index(occurrences: &[Vec<u8>], symbols: &[u8]) -> Vec<u8> {
    let mut document = [field_bytes(1, b"src/render.rs"), field_bytes(5, TEXT)].concat();
    for occurrence in occurrences {
        document.extend(field_bytes(2, occurrence));
    }
    document.extend_from_slice(symbols);
    field_bytes(2, &document)
}

fn repository(name: &str, index: &[u8]) -> PathBuf {
    let root = std::env::temp_dir().join(format!("bran-scip-bounds-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".bran")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join(".bran/policy.yaml"), "schema_version: \"1\"\n").unwrap();
    std::fs::write(root.join("src/render.rs"), TEXT).unwrap();
    std::fs::write(root.join("index.scip"), index).unwrap();
    root
}

/// Runs `bran <command> <root> Renderer` with a 192 MiB address-space limit.
fn limited(command: &str, root: &Path) -> String {
    let output = Command::new("sh")
        .arg("-c")
        .arg("ulimit -v 196608 && exec \"$0\" \"$@\"")
        .arg(env!("CARGO_BIN_EXE_bran"))
        .arg(command)
        .arg(root)
        .arg("Renderer")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{command}: {:?} {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn assert_navigation(name: &str, index: &[u8], expected: &str) {
    let root = repository(name, index);
    for command in ["query", "packet"] {
        let output = limited(command, &root);
        assert!(output.contains(expected), "{name} {command}: {output}");
    }
    let _ = std::fs::remove_dir_all(root);
}

/// Finding 2: 2,000 repeated implementation relationships and 2,000
/// implementing definitions.
#[test]
fn scip_repeated_implementations_stay_bounded() {
    let target = [field_bytes(1, RENDERER), field_varint(3, 1)].concat();
    let mut information = field_bytes(1, HTML);
    for _ in 0..2000 {
        information.extend(field_bytes(4, &target));
    }
    let mut occurrences = vec![definition(RENDERER, 0)];
    occurrences.extend(std::iter::repeat_n(definition(HTML, 1), 2000));
    assert_navigation(
        "evidence-amplification",
        &index(&occurrences, &field_bytes(3, &information)),
        "\"symbol_navigation\":{\"schema_version\":\"1.0.0\",\"outcome\":\"hit\",\"truncated\":false,",
    );
}

const UNREADABLE: &str = "\"scip\":{\"status\":\"unavailable\",\"reason\":\"index_unreadable\"";

/// Finding 3: four million empty external-symbol messages.
#[test]
fn scip_decoder_rejects_symbol_flood_before_allocating() {
    assert_navigation(
        "decoder-amplification",
        &[0x1a, 0x00].repeat(4_000_000),
        UNREADABLE,
    );
}

/// Finding 3: a packed range of 16,777,217 coordinates.
#[test]
fn scip_decoder_rejects_oversized_range_before_allocating() {
    let range = vec![0; 16 * 1024 * 1024 + 1];
    let occurrence = [
        field_bytes(1, &range),
        field_bytes(2, RENDERER),
        field_varint(3, 1),
    ]
    .concat();
    assert_navigation(
        "packed-range-amplification",
        &index(&[occurrence], &[]),
        UNREADABLE,
    );
}
