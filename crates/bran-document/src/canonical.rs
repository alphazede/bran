//! Canonical JSON bytes matching the envelope rule
//! `json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)`.

use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Arr(Vec<Json>),
    Obj(BTreeMap<String, Json>),
}

impl Json {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = String::new();
        self.write(&mut out);
        out.into_bytes()
    }

    fn write(&self, out: &mut String) {
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Self::Int(value) => out.push_str(&value.to_string()),
            Self::Str(value) => write_string(out, value),
            Self::Arr(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            // BTreeMap<String> orders keys by UTF-8 bytes, which is code-point
            // order, the same order Python's sort_keys uses.
            Self::Obj(map) => {
                out.push('{');
                for (index, (key, item)) in map.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    write_string(out, key);
                    out.push(':');
                    item.write(out);
                }
                out.push('}');
            }
        }
    }
}

/// Python `json.dumps(..., ensure_ascii=False)` escaping: quote, backslash,
/// and C0 controls only; everything else is emitted raw.
fn write_string(out: &mut String, value: &str) {
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    bran_core::agent::result_store::ResultId::sha256(bytes)
        .value()
        .to_owned()
}
