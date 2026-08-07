//! Minimal deterministic YAML-frontmatter parser for BRAN profile validation.
//!
//! Parses the OKF frontmatter subset: flat scalars, nested block mappings,
//! block sequences of scalars, block sequences of mappings, and inline flow
//! scalar lists (`[a, b]`). Every scalar is preserved as
//! [`YamlValue::String`], matching the scanner's fact stream, while nested
//! structure is recovered so profile validation can shape-check the OKF v0.2
//! optional families (`sources`, `usage_window`, `generated`, `verified`,
//! `status`, `stale_after`, and the Attested Computation fields).
//!
//! Unknown fields and unknown types are preserved verbatim, never rewritten.
//! Parsing never touches the body, never resolves links, and error messages
//! never echo raw values (secret-bearing content stays out of diagnostics).

use crate::schema::YamlValue;
use std::collections::BTreeMap;

/// Parse a YAML frontmatter block into a normalized mapping.
///
/// `raw` may be the full fenced block (`---\n...\n---\n`) or the fence-free
/// content. A leading `---` line and a trailing `---` line are stripped when
/// present. Errors carry a structural reason only; raw values are never
/// echoed.
pub fn parse_frontmatter(raw: &str) -> Result<BTreeMap<String, YamlValue>, String> {
    let content = strip_fences(raw);
    let tokens = tokenize(content)?;
    let mut entries = Vec::new();
    let mut index = 0;
    build_mapping(&tokens, &mut index, 0, &mut entries)?;
    if index != tokens.len() {
        return Err("unexpected indented content".to_owned());
    }
    let mut map = BTreeMap::new();
    for (key, value) in entries {
        if map.insert(key.clone(), value).is_some() {
            return Err(format!("duplicate frontmatter key: {key}"));
        }
    }
    Ok(map)
}

/// Strip a leading and trailing `---` fence line when present.
fn strip_fences(raw: &str) -> &str {
    let trimmed = raw.trim_start_matches('\u{feff}');
    let Some(content) = trimmed
        .strip_prefix("---\n")
        .or_else(|| trimmed.strip_prefix("---\r\n"))
    else {
        return trimmed;
    };
    // A trailing fence is the final line equal to "---".
    let trimmed_end = content.trim_end();
    let final_line = trimmed_end.rsplit('\n').next().unwrap_or("");
    if final_line.trim() == "---" {
        let end = trimmed_end.len() - final_line.len();
        trimmed_end[..end].trim_end()
    } else {
        content
    }
}

/// One inline value on a `key:` or `- ` line.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Inline {
    Scalar(String),
    FlowList(Vec<String>),
}

/// A structural line token. Indentation is measured in leading spaces.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Token {
    /// `key: value` (inline) or `key:` (nested block follows).
    Key {
        indent: usize,
        key: String,
        inline: Option<Inline>,
    },
    /// `- value` or `- key: value` (mapping item; deeper lines are its fields).
    SeqItem {
        indent: usize,
        key: Option<String>,
        inline: Option<Inline>,
    },
}

fn tokenize(raw: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    for raw_line in raw.lines() {
        let line = raw_line.trim_end();
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - trimmed.len();
        if let Some(rest) = trimmed.strip_prefix("- ") {
            let (key, inline) = split_sequence_value(rest)?;
            tokens.push(Token::SeqItem {
                indent,
                key,
                inline,
            });
            continue;
        }
        let Some(colon) = trimmed.find(':') else {
            return Err("not key/value YAML".to_owned());
        };
        let key = trimmed[..colon].trim();
        if !valid_key(key) {
            return Err("unsupported YAML scalar".to_owned());
        }
        let rest = &trimmed[colon + 1..];
        tokens.push(Token::Key {
            indent,
            key: key.to_owned(),
            inline: parse_inline(rest)?,
        });
    }
    Ok(tokens)
}

/// Split `- rest` into an optional mapping key and an inline value.
///
/// A colon followed by whitespace (or end of line) marks a mapping item
/// (`- by: x`); a colon followed by other characters keeps the whole text a
/// scalar (`- https://x/y`).
fn split_sequence_value(rest: &str) -> Result<(Option<String>, Option<Inline>), String> {
    let mut candidate = None;
    for (offset, byte) in rest.bytes().enumerate() {
        if byte == b':' {
            let head = &rest[..offset];
            if valid_key(head.trim()) {
                if offset + 1 >= rest.len() || rest.as_bytes()[offset + 1] == b' ' {
                    candidate = Some(offset);
                }
                break;
            }
        }
    }
    match candidate {
        Some(colon) => Ok((
            Some(rest[..colon].trim().to_owned()),
            parse_inline(&rest[colon + 1..])?,
        )),
        None => Ok((None, Some(Inline::Scalar(scalar(rest)?)))),
    }
}

/// Parse the value part of a `key:` line. `None` marks an empty value that
/// must be followed by a nested block.
fn parse_inline(rest: &str) -> Result<Option<Inline>, String> {
    let rest = rest.trim();
    if rest.is_empty() {
        return Ok(None);
    }
    if rest.starts_with(['{', '|', '>']) {
        return Err("unsupported YAML scalar".to_owned());
    }
    // Inline comments only start after whitespace; quoted values keep any ` #`.
    let uncommented = if rest.starts_with(['"', '\'']) {
        rest
    } else {
        strip_inline_comment(rest).unwrap_or("")
    };
    let uncommented = uncommented.trim();
    if uncommented.is_empty() {
        return Err("empty YAML scalar".to_owned());
    }
    if uncommented.starts_with('[') {
        if !uncommented.ends_with(']') {
            return Err("malformed YAML list".to_owned());
        }
        let inner = &uncommented[1..uncommented.len() - 1];
        let mut values = Vec::new();
        for item in inner.split(',') {
            values.push(scalar(item)?);
        }
        return Ok(Some(Inline::FlowList(values)));
    }
    Ok(Some(Inline::Scalar(scalar(uncommented)?)))
}

/// Drop a trailing ` # comment` from an unquoted scalar value.
fn strip_inline_comment(value: &str) -> Option<&str> {
    value.split_once(" #").map_or(Some(value), |(head, _)| {
        let head = head.trim_end();
        if head.is_empty() {
            None
        } else {
            Some(head)
        }
    })
}

/// Parse and trim a scalar, stripping one layer of matching quotes.
fn scalar(value: &str) -> Result<String, String> {
    let value = value.trim();
    let unquoted = value
        .strip_prefix('"')
        .and_then(|item| item.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|item| item.strip_suffix('\''))
        })
        .unwrap_or(value)
        .trim();
    if unquoted.is_empty() {
        Err("empty YAML scalar".to_owned())
    } else {
        Ok(unquoted.to_owned())
    }
}

fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

/// Collect mapping entries at exactly `indent`. Stops at the first token with
/// a shallower or equal indentation (or end of input).
fn build_mapping(
    tokens: &[Token],
    index: &mut usize,
    indent: usize,
    entries: &mut Vec<(String, YamlValue)>,
) -> Result<(), String> {
    while *index < tokens.len() {
        match &tokens[*index] {
            Token::SeqItem {
                indent: item_indent,
                ..
            } => {
                // A shallower sequence item closes a nested mapping block;
                // anything deeper is malformed.
                if *item_indent < indent {
                    break;
                }
                return Err("sequence item outside of a sequence".to_owned());
            }
            Token::Key {
                indent: key_indent,
                key,
                inline,
            } if *key_indent == indent => {
                let inline = inline.clone();
                if let Some(inline) = inline {
                    entries.push((key.clone(), yaml_value(inline)));
                    *index += 1;
                    continue;
                }
                // Nested block: the next token must be deeper. Skip the key
                // token so the block parser reads the nested lines.
                let Some(next) = tokens.get(*index + 1) else {
                    return Err("frontmatter key has an empty value".to_owned());
                };
                let next_indent = next_indent(next);
                if next_indent <= indent {
                    return Err("frontmatter key has an empty value".to_owned());
                }
                *index += 1;
                let value = if matches!(next, Token::SeqItem { .. }) {
                    let mut items = Vec::new();
                    build_sequence(tokens, index, next_indent, &mut items)?;
                    YamlValue::Sequence(items)
                } else {
                    let mut nested = Vec::new();
                    build_mapping(tokens, index, next_indent, &mut nested)?;
                    YamlValue::Mapping(nested.into_iter().collect())
                };
                entries.push((key.clone(), value));
            }
            Token::Key { .. } => break,
        }
    }
    Ok(())
}

fn next_indent(token: &Token) -> usize {
    match token {
        Token::Key { indent, .. } | Token::SeqItem { indent, .. } => *indent,
    }
}

/// Collect sequence items at exactly `indent`. Each item is a scalar or a
/// mapping whose deeper lines (indent greater than the item's) are its fields.
fn build_sequence(
    tokens: &[Token],
    index: &mut usize,
    indent: usize,
    items: &mut Vec<YamlValue>,
) -> Result<(), String> {
    while *index < tokens.len() {
        let Token::SeqItem {
            indent: item_indent,
            key,
            inline,
        } = &tokens[*index]
        else {
            break;
        };
        if *item_indent != indent {
            break;
        }
        let key = key.clone();
        let inline = inline.clone();
        match key {
            None => {
                let Some(inline) = inline else {
                    return Err("empty sequence item".to_owned());
                };
                items.push(yaml_value(inline));
                *index += 1;
            }
            Some(key) => {
                let mut mapping = BTreeMap::new();
                if let Some(inline) = inline {
                    mapping.insert(key.clone(), yaml_value(inline));
                }
                // Deeper lines are this item's remaining fields. Skip the item
                // token so the mapping parser reads the field lines, then push
                // the completed item.
                if let Some(next) = tokens.get(*index + 1) {
                    let next_indent = next_indent(next);
                    if next_indent > *item_indent {
                        *index += 1;
                        let mut fields = Vec::new();
                        build_mapping(tokens, index, next_indent, &mut fields)?;
                        for (field, value) in fields {
                            if mapping.insert(field.clone(), value).is_some() {
                                return Err(format!("duplicate frontmatter key: {field}"));
                            }
                        }
                        items.push(YamlValue::Mapping(mapping));
                        continue;
                    }
                }
                *index += 1;
                items.push(YamlValue::Mapping(mapping));
            }
        }
    }
    Ok(())
}

fn yaml_value(inline: Inline) -> YamlValue {
    match inline {
        Inline::Scalar(value) => YamlValue::String(value),
        Inline::FlowList(values) => {
            YamlValue::Sequence(values.into_iter().map(YamlValue::String).collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn string(value: &str) -> YamlValue {
        YamlValue::String(value.to_owned())
    }

    fn mapping(pairs: &[(&str, YamlValue)]) -> YamlValue {
        YamlValue::Mapping(
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect(),
        )
    }

    /// Each row is one supported YAML shape: (case, input, key, expected).
    #[test]
    fn parses_supported_yaml_shapes() {
        let flow_and_comments = "tags: [a, b]  # inline comment\ntype: Concept # note\n";
        let fenced = "---\ntype: Concept\nokf_version: \"0.2\"\n---\n";
        for (case, input, key, expected) in [
            ("flat scalar inside fences", fenced, "type", string("Concept")),
            ("quoted scalar keeps its value", fenced, "okf_version", string("0.2")),
            ("fence-free content", "type: Concept\n", "type", string("Concept")),
            (
                "nested block mapping",
                "generated:\n  by: agent/1\n  at: 2026-07-01T00:00:00Z\n",
                "generated",
                mapping(&[
                    ("by", string("agent/1")),
                    ("at", string("2026-07-01T00:00:00Z")),
                ]),
            ),
            (
                "block sequence of scalars",
                "tags:\n  - internal\n  - public\n",
                "tags",
                YamlValue::Sequence(vec![string("internal"), string("public")]),
            ),
            (
                "block sequence of mappings",
                "verified:\n  - by: human:alice\n    at: 2026-07-01T00:00:00Z\n  - by: human:bob\n    at: 2026-07-02T00:00:00Z\n",
                "verified",
                YamlValue::Sequence(vec![
                    mapping(&[
                        ("by", string("human:alice")),
                        ("at", string("2026-07-01T00:00:00Z")),
                    ]),
                    mapping(&[
                        ("by", string("human:bob")),
                        ("at", string("2026-07-02T00:00:00Z")),
                    ]),
                ]),
            ),
            (
                "nested mapping inside a sequence item",
                "sources:\n  - resource: https://example.invalid/a\n    usage_count: 3\n",
                "sources",
                YamlValue::Sequence(vec![mapping(&[
                    ("resource", string("https://example.invalid/a")),
                    ("usage_count", string("3")),
                ])]),
            ),
            (
                "flow list with an inline comment",
                flow_and_comments,
                "tags",
                YamlValue::Sequence(vec![string("a"), string("b")]),
            ),
            (
                "scalar on a commented line",
                flow_and_comments,
                "type",
                string("Concept"),
            ),
            (
                "url scalar keeps its colons",
                "resource: https://example.invalid/a#frag\n",
                "resource",
                string("https://example.invalid/a#frag"),
            ),
            (
                "sequence item with a colon stays scalar",
                "sources:\n  - https://example.invalid/b\n",
                "sources",
                YamlValue::Sequence(vec![string("https://example.invalid/b")]),
            ),
        ] {
            let map = parse_frontmatter(input).unwrap_or_else(|error| panic!("{case}: {error}"));
            assert_eq!(map.get(key), Some(&expected), "{case}");
        }
    }

    #[test]
    fn rejects_malformed_input_without_echoing_values() {
        for (case, input) in [
            ("duplicate key", "type: A\ntype: B\n"),
            ("unterminated flow sequence", "type: [\n"),
            ("mapping key with no value", "generated:\n"),
            ("scalar key with no value", "type:\n"),
            ("flow mapping is unsupported", "type: {by: x}\n"),
        ] {
            assert!(parse_frontmatter(input).is_err(), "{case} must be rejected");
        }

        // A rejection must never echo the offending value back to the caller.
        for input in [
            "type: [secret-value\n",
            "generated: |secret-value\n",
            "type: {secret-value\n",
        ] {
            let error = parse_frontmatter(input).expect_err("must be rejected");
            assert!(
                !error.contains("secret-value"),
                "error echoes raw value: {error}"
            );
        }
    }

    #[test]
    fn empty_frontmatter_parses_to_empty_map() {
        assert!(parse_frontmatter("---\n---\n").unwrap().is_empty());
        assert!(parse_frontmatter("").unwrap().is_empty());
    }
}
