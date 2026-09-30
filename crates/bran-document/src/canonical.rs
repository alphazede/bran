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

/// Deepest nesting `Json::parse` accepts; deeper input is refused, not recursed.
const MAX_PARSE_DEPTH: usize = 64;

impl Json {
    /// Parses the JSON subset `to_bytes` emits (integers only, no floats).
    /// Anything else, trailing bytes, or nesting past 64 levels is `None`.
    pub fn parse(bytes: &[u8]) -> Option<Json> {
        let text = std::str::from_utf8(bytes).ok()?;
        let mut parser = Parser { text, at: 0 };
        let value = parser.value(0)?;
        parser.space();
        (parser.at == text.len()).then_some(value)
    }

    /// The value under `key` when `self` is an object.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Self::Obj(map) => map.get(key),
            _ => None,
        }
    }
}

struct Parser<'a> {
    text: &'a str,
    at: usize,
}

impl Parser<'_> {
    fn space(&mut self) {
        let rest = &self.text[self.at..];
        self.at += rest.len() - rest.trim_start_matches([' ', '\t', '\n', '\r']).len();
    }

    fn eat(&mut self, token: &str) -> bool {
        self.space();
        let found = self.text[self.at..].starts_with(token);
        if found {
            self.at += token.len();
        }
        found
    }

    fn value(&mut self, depth: usize) -> Option<Json> {
        if depth >= MAX_PARSE_DEPTH {
            return None;
        }
        self.space();
        let rest = &self.text[self.at..];
        for (word, value) in [
            ("null", Json::Null),
            ("true", Json::Bool(true)),
            ("false", Json::Bool(false)),
        ] {
            if rest.starts_with(word) {
                self.at += word.len();
                return Some(value);
            }
        }
        match rest.chars().next()? {
            '"' => self.string().map(Json::Str),
            '[' => {
                self.at += 1;
                let mut items = Vec::new();
                if self.eat("]") {
                    return Some(Json::Arr(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    if self.eat("]") {
                        return Some(Json::Arr(items));
                    }
                    if !self.eat(",") {
                        return None;
                    }
                }
            }
            '{' => {
                self.at += 1;
                let mut map = BTreeMap::new();
                if self.eat("}") {
                    return Some(Json::Obj(map));
                }
                loop {
                    self.space();
                    let key = self.string()?;
                    if !self.eat(":") {
                        return None;
                    }
                    map.insert(key, self.value(depth + 1)?);
                    if self.eat("}") {
                        return Some(Json::Obj(map));
                    }
                    if !self.eat(",") {
                        return None;
                    }
                }
            }
            _ => {
                let end = rest
                    .find(|c: char| !(c == '-' || c.is_ascii_digit()))
                    .unwrap_or(rest.len());
                self.at += end;
                rest[..end].parse().ok().map(Json::Int)
            }
        }
    }

    fn string(&mut self) -> Option<String> {
        let mut chars = self.text[self.at..].char_indices();
        if chars.next()?.1 != '"' {
            return None;
        }
        let mut out = String::new();
        while let Some((index, c)) = chars.next() {
            match c {
                '"' => {
                    self.at += index + 1;
                    return Some(out);
                }
                '\\' => out.push(match chars.next()?.1 {
                    '"' => '"',
                    '\\' => '\\',
                    '/' => '/',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    'b' => '\u{8}',
                    'f' => '\u{c}',
                    'u' => {
                        let hex: String = (0..4)
                            .filter_map(|_| chars.next().map(|(_, c)| c))
                            .collect();
                        char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?
                    }
                    _ => return None,
                }),
                c if (c as u32) < 0x20 => return None,
                c => out.push(c),
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::Json;
    use std::collections::BTreeMap;

    #[test]
    fn parse_reads_back_what_to_bytes_writes() {
        let mut map = BTreeMap::new();
        map.insert(
            "text".to_owned(),
            Json::Str("Grüße \"q\" \\ \n\u{1}".to_owned()),
        );
        map.insert(
            "list".to_owned(),
            Json::Arr(vec![Json::Int(-7), Json::Null, Json::Bool(true)]),
        );
        map.insert("empty".to_owned(), Json::Obj(BTreeMap::new()));
        let value = Json::Obj(map);
        assert_eq!(Json::parse(&value.to_bytes()), Some(value));
    }

    #[test]
    fn parse_refuses_what_to_bytes_never_writes() {
        for input in ["{} x", "1.5", "[", "\"\u{1}\"", "{\"a\" 1}"] {
            assert_eq!(Json::parse(input.as_bytes()), None, "{input:?}");
        }
        let deep = format!("{}{}", "[".repeat(65), "]".repeat(65));
        assert_eq!(Json::parse(deep.as_bytes()), None);
        let allowed = format!("{}{}", "[".repeat(64), "]".repeat(64));
        assert!(Json::parse(allowed.as_bytes()).is_some());
    }
}
