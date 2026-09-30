//! Exact symbol navigation facts read from an existing SCIP index (issue #38).
//!
//! BRAN never generates an index. It decodes only the parts of the SCIP
//! protobuf schema (sourcegraph/scip `scip.proto`) it needs: document paths
//! and text, occurrences with their ranges and roles, and symbol kinds and
//! implementation relationships. Anything malformed is a decode error, never
//! a partial guess.

use std::collections::{BTreeMap, BTreeSet};

/// Largest index BRAN will read.
pub const MAX_INDEX_BYTES: u64 = 256 * 1024 * 1024;

/// `SymbolRole.Definition` in `Occurrence.symbol_roles`.
const DEFINITION_ROLE: u64 = 1;

/// `SymbolInformation.Kind` values 0..=86 in snake case; 83 is unassigned.
const KIND_NAMES: &str = "unspecified_kind array assertion associated_type attribute axiom boolean class constant constructor data_family enum enum_member event fact field file function getter grammar instance interface key lang lemma macro method method_receiver message module namespace null number object operator package package_object parameter parameter_label pattern predicate property protocol quasiquoter self_parameter setter signature subscript string struct tactic theorem this_parameter trait type type_alias type_class type_family type_parameter union value variable contract error library modifier abstract_method method_specification protocol_method pure_virtual_method trait_method type_class_method accessor delegate method_alias singleton_class singleton_method static_data_member static_event static_field static_method static_property static_variable unspecified_kind extension mixin concept";

#[derive(Debug, Default)]
pub struct Index {
    pub documents: Vec<Document>,
    pub external_symbols: Vec<SymbolInformation>,
}

#[derive(Debug, Default)]
pub struct Document {
    pub relative_path: String,
    /// Source text as indexed; empty when the indexer did not record it.
    pub text: String,
    pub occurrences: Vec<Occurrence>,
    pub symbols: Vec<SymbolInformation>,
}

#[derive(Debug, Default)]
pub struct Occurrence {
    pub symbol: String,
    pub definition: bool,
    /// Zero-based, inclusive.
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Debug, Default)]
pub struct SymbolInformation {
    pub symbol: String,
    pub kind: u64,
    /// Symbols this one implements (`Relationship.is_implementation`).
    pub implements: Vec<String>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct DecodeError;

#[derive(Debug, Eq, PartialEq)]
pub enum Freshness {
    /// Every document's recorded text equals the scanned file.
    Fresh,
    /// Some document has no recorded text or was not scanned.
    Unverified,
    /// Some document's recorded text differs from the scanned file.
    Stale,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Role {
    Definition,
    Implementation,
    Reference,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Definition => "definition",
            Role::Implementation => "implementation",
            Role::Reference => "reference",
        }
    }
}

/// One exact navigation fact, with one-based inclusive lines.
#[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Evidence {
    pub locator: String,
    pub start_line: u32,
    pub end_line: u32,
    pub role: Role,
    pub symbol: String,
    /// For an implementation, the matched symbol it implements.
    pub implements: Option<String>,
}

/// Human-readable parts of a global SCIP symbol.
#[derive(Debug, Eq, PartialEq)]
pub struct SymbolName {
    /// The last namespace, type, term, method, or macro descriptor name.
    pub name: String,
    /// Namespace, type, term, method, and macro descriptor names joined by `::`.
    pub qualified_name: String,
    /// The last descriptor is `name`, not a parameter, type parameter, or
    /// meta descriptor; only navigable symbols match query names.
    pub navigable: bool,
}

impl Index {
    /// Compares each document's recorded text with the scanned source.
    pub fn freshness<'a>(&self, scanned: impl Fn(&str) -> Option<&'a [u8]>) -> Freshness {
        let mut freshness = Freshness::Fresh;
        for document in &self.documents {
            match scanned(&document.relative_path) {
                Some(source) if !document.text.is_empty() => {
                    if source != document.text.as_bytes() {
                        return Freshness::Stale;
                    }
                }
                _ => freshness = Freshness::Unverified,
            }
        }
        freshness
    }

    /// Definitions, references, and implementations of every global symbol
    /// whose name equals one of `names` (compared lowercase), in locator and
    /// line order.
    pub fn evidence(&self, names: &BTreeSet<String>) -> Vec<Evidence> {
        let mut matched = BTreeSet::new();
        for symbol in self.symbols() {
            if symbol_name(symbol).is_some_and(|name| {
                name.navigable && names.contains(&name.name.to_ascii_lowercase())
            }) {
                matched.insert(symbol);
            }
        }
        let mut implemented = BTreeMap::<&str, Vec<&str>>::new();
        for information in self.informations() {
            for target in &information.implements {
                if matched.contains(target.as_str()) {
                    implemented
                        .entry(&information.symbol)
                        .or_default()
                        .push(target);
                }
            }
        }
        let mut evidence = Vec::new();
        for document in &self.documents {
            for occurrence in &document.occurrences {
                let fact = |role, implements: Option<&str>| Evidence {
                    locator: document.relative_path.clone(),
                    start_line: occurrence.start_line + 1,
                    end_line: occurrence.end_line + 1,
                    role,
                    symbol: occurrence.symbol.clone(),
                    implements: implements.map(str::to_owned),
                };
                if matched.contains(occurrence.symbol.as_str()) {
                    evidence.push(fact(
                        if occurrence.definition {
                            Role::Definition
                        } else {
                            Role::Reference
                        },
                        None,
                    ));
                }
                if occurrence.definition {
                    for target in implemented
                        .get(occurrence.symbol.as_str())
                        .into_iter()
                        .flatten()
                    {
                        evidence.push(fact(Role::Implementation, Some(target)));
                    }
                }
            }
        }
        evidence.sort();
        evidence.dedup();
        evidence
    }

    /// The snake-case `SymbolInformation.Kind` recorded for `symbol`.
    pub fn kind(&self, symbol: &str) -> &'static str {
        let kind = self
            .informations()
            .find(|information| information.symbol == symbol)
            .map_or(0, |information| information.kind);
        usize::try_from(kind)
            .ok()
            .and_then(|kind| KIND_NAMES.split(' ').nth(kind))
            .unwrap_or("unspecified_kind")
    }

    fn informations(&self) -> impl Iterator<Item = &SymbolInformation> {
        self.documents
            .iter()
            .flat_map(|document| &document.symbols)
            .chain(&self.external_symbols)
    }

    fn symbols(&self) -> BTreeSet<&str> {
        self.documents
            .iter()
            .flat_map(|document| &document.occurrences)
            .map(|occurrence| occurrence.symbol.as_str())
            .chain(
                self.informations()
                    .map(|information| information.symbol.as_str()),
            )
            .collect()
    }
}

/// Name and qualified name of a global symbol. Local symbols, malformed
/// symbols, and symbols without a namespace, type, term, method, or macro
/// descriptor return `None`.
pub fn symbol_name(symbol: &str) -> Option<SymbolName> {
    if symbol.starts_with("local ") {
        return None;
    }
    // Skip scheme, package manager, package name, and version; a double
    // space is an escaped space inside one of them.
    let mut rest = symbol;
    for _ in 0..4 {
        let mut offset = 0;
        loop {
            let space = offset + rest[offset..].find(' ')?;
            if rest[space + 1..].starts_with(' ') {
                offset = space + 2;
            } else {
                rest = &rest[space + 1..];
                break;
            }
        }
    }
    let mut names = Vec::new();
    let mut navigable = false;
    while !rest.is_empty() {
        let closing = match rest.as_bytes()[0] {
            b'[' => Some(']'),
            b'(' => Some(')'),
            _ => None,
        };
        if let Some(closing) = closing {
            rest = rest[1..].split_once(closing)?.1;
            navigable = false;
            continue;
        }
        let (name, after) = descriptor_name(rest)?;
        let suffix = after.chars().next()?;
        rest = &after[suffix.len_utf8()..];
        navigable = match suffix {
            '/' | '#' | '.' | '!' => true,
            '(' => {
                rest = rest.split_once(").")?.1;
                true
            }
            ':' => false,
            _ => return None,
        };
        if navigable {
            names.push(name);
        }
    }
    Some(SymbolName {
        name: names.last()?.clone(),
        qualified_name: names.join("::"),
        navigable,
    })
}

fn descriptor_name(text: &str) -> Option<(String, &str)> {
    if let Some(mut rest) = text.strip_prefix('`') {
        let mut name = String::new();
        loop {
            let (part, after) = rest.split_once('`')?;
            name.push_str(part);
            match after.strip_prefix('`') {
                Some(after) => {
                    name.push('`');
                    rest = after;
                }
                // A control character could forge packet payload lines.
                None => return (!name.contains(char::is_control)).then_some((name, after)),
            }
        }
    }
    let end = text
        .find(|character: char| !(character.is_alphanumeric() || "_+-$".contains(character)))
        .unwrap_or(text.len());
    (end > 0).then(|| (text[..end].to_owned(), &text[end..]))
}

/// Decodes a SCIP `Index` message.
pub fn decode_index(bytes: &[u8]) -> Result<Index, DecodeError> {
    let mut index = Index::default();
    each_field(bytes, |field, value| {
        match field {
            2 => index.documents.push(document(value.bytes()?)?),
            3 => index
                .external_symbols
                .push(symbol_information(value.bytes()?)?),
            _ => {}
        }
        Ok(())
    })?;
    Ok(index)
}

fn document(bytes: &[u8]) -> Result<Document, DecodeError> {
    let mut document = Document::default();
    each_field(bytes, |field, value| {
        match field {
            1 => document.relative_path = value.string()?,
            2 => document.occurrences.push(occurrence(value.bytes()?)?),
            3 => document.symbols.push(symbol_information(value.bytes()?)?),
            5 => document.text = value.string()?,
            _ => {}
        }
        Ok(())
    })?;
    Ok(document)
}

fn occurrence(bytes: &[u8]) -> Result<Occurrence, DecodeError> {
    let mut symbol = String::new();
    let mut roles = 0;
    let mut range = Vec::new();
    let mut typed = None;
    each_field(bytes, |field, value| {
        match (field, value) {
            // Deprecated `range`: packed, or one unpacked element per field.
            (1, Value::Bytes(packed)) => {
                let mut reader = Reader {
                    bytes: packed,
                    at: 0,
                };
                while reader.at < packed.len() {
                    range.push(reader.varint()?);
                }
            }
            (1, Value::Varint(element)) => range.push(element),
            (2, value) => symbol = value.string()?,
            (3, value) => roles = value.varint()?,
            // `single_line_range`: line = 1.
            (8, value) => {
                let line = message_varint(value.bytes()?, 1)?;
                typed = Some((line, line));
            }
            // `multi_line_range`: start_line = 1, end_line = 3.
            (9, value) => {
                let bytes = value.bytes()?;
                typed = Some((message_varint(bytes, 1)?, message_varint(bytes, 3)?));
            }
            _ => {}
        }
        Ok(())
    })?;
    let (start, end) = match (typed, range.as_slice()) {
        (Some(lines), _) => lines,
        (None, [line, _, _]) => (*line, *line),
        (None, [start, _, end, _]) => (*start, *end),
        _ => return Err(DecodeError),
    };
    let line = |value: u64| u32::try_from(value).map_err(|_| DecodeError);
    let (start_line, end_line) = (line(start)?, line(end)?);
    if end_line < start_line {
        return Err(DecodeError);
    }
    Ok(Occurrence {
        symbol,
        definition: roles & DEFINITION_ROLE != 0,
        start_line,
        end_line,
    })
}

fn symbol_information(bytes: &[u8]) -> Result<SymbolInformation, DecodeError> {
    let mut information = SymbolInformation::default();
    each_field(bytes, |field, value| {
        match field {
            1 => information.symbol = value.string()?,
            4 => {
                let mut target = String::new();
                let mut implementation = false;
                each_field(value.bytes()?, |field, value| {
                    match field {
                        1 => target = value.string()?,
                        3 => implementation = value.varint()? != 0,
                        _ => {}
                    }
                    Ok(())
                })?;
                if implementation {
                    information.implements.push(target);
                }
            }
            5 => information.kind = value.varint()?,
            _ => {}
        }
        Ok(())
    })?;
    Ok(information)
}

/// The last value of varint field `wanted` in one message; zero when absent.
fn message_varint(bytes: &[u8], wanted: u64) -> Result<u64, DecodeError> {
    let mut found = 0;
    each_field(bytes, |field, value| {
        if field == wanted {
            found = value.varint()?;
        }
        Ok(())
    })?;
    Ok(found)
}

enum Value<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    Fixed,
}

impl<'a> Value<'a> {
    fn varint(self) -> Result<u64, DecodeError> {
        match self {
            Value::Varint(value) => Ok(value),
            _ => Err(DecodeError),
        }
    }

    fn bytes(self) -> Result<&'a [u8], DecodeError> {
        match self {
            Value::Bytes(bytes) => Ok(bytes),
            _ => Err(DecodeError),
        }
    }

    fn string(self) -> Result<String, DecodeError> {
        String::from_utf8(self.bytes()?.to_vec()).map_err(|_| DecodeError)
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn varint(&mut self) -> Result<u64, DecodeError> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *self.bytes.get(self.at).ok_or(DecodeError)?;
            self.at += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte < 0x80 {
                return Ok(value);
            }
        }
        Err(DecodeError)
    }

    fn take(&mut self, length: u64) -> Result<&'a [u8], DecodeError> {
        let end = usize::try_from(length)
            .ok()
            .and_then(|length| self.at.checked_add(length))
            .filter(|end| *end <= self.bytes.len())
            .ok_or(DecodeError)?;
        let bytes = &self.bytes[self.at..end];
        self.at = end;
        Ok(bytes)
    }
}

/// Calls `visit` for each field of one message. Group wire types and
/// truncated input are errors.
fn each_field<'a>(
    bytes: &'a [u8],
    mut visit: impl FnMut(u64, Value<'a>) -> Result<(), DecodeError>,
) -> Result<(), DecodeError> {
    let mut reader = Reader { bytes, at: 0 };
    while reader.at < bytes.len() {
        let key = reader.varint()?;
        let value = match key & 7 {
            0 => Value::Varint(reader.varint()?),
            1 => {
                reader.take(8)?;
                Value::Fixed
            }
            2 => {
                let length = reader.varint()?;
                Value::Bytes(reader.take(length)?)
            }
            5 => {
                reader.take(4)?;
                Value::Fixed
            }
            _ => return Err(DecodeError),
        };
        visit(key >> 3, value)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{decode_index, symbol_name, SymbolName};

    #[test]
    fn scip_symbol_names_follow_descriptor_grammar() {
        let name = |symbol: &str| {
            symbol_name(symbol)
                .filter(|name| name.navigable)
                .map(|name| (name.name, name.qualified_name))
        };
        assert_eq!(
            name("rust-analyzer cargo demo 0.1.0 render/Html#render()."),
            Some(("render".to_owned(), "render::Html::render".to_owned()))
        );
        assert_eq!(
            name("scip-go gomod example.com/a  b v1 `x.y`/Type#Method(+1)."),
            Some(("Method".to_owned(), "x.y::Type::Method".to_owned()))
        );
        assert_eq!(
            symbol_name("rust-analyzer cargo demo 0.1.0 m/`a``b`!"),
            Some(SymbolName {
                name: "a`b".to_owned(),
                qualified_name: "m::a`b".to_owned(),
                navigable: true,
            })
        );
        assert_eq!(name("rust-analyzer cargo demo 0.1.0 f().(value)"), None);
        assert_eq!(
            symbol_name("rust-analyzer cargo demo 0.1.0 m/impl#[Html][Renderer]"),
            Some(SymbolName {
                name: "impl".to_owned(),
                qualified_name: "m::impl".to_owned(),
                navigable: false,
            })
        );
        assert_eq!(name("rust-analyzer cargo demo 0.1.0 Vec#[T]"), None);
        assert_eq!(name("local 7"), None);
        assert_eq!(name("rust-analyzer cargo demo"), None);
        assert_eq!(name("rust-analyzer cargo demo 0.1.0 m/f(."), None);
        assert_eq!(name("rust-analyzer cargo demo 0.1.0 m→"), None);
        assert_eq!(name("rust-analyzer cargo demo 0.1.0 `a\nrank: 1`."), None);
    }

    #[test]
    fn scip_decoder_rejects_truncated_index_without_panicking() {
        // Document { relative_path: "a.rs", occurrences: [{ range: [0, 1, 2], symbol: "s", roles: 1 }] }
        let occurrence = [0x0a, 0x03, 0, 1, 2, 0x12, 0x01, b's', 0x18, 0x01];
        let mut document = vec![
            0x0a,
            0x04,
            b'a',
            b'.',
            b'r',
            b's',
            0x12,
            occurrence.len() as u8,
        ];
        document.extend_from_slice(&occurrence);
        let mut index = vec![0x12, document.len() as u8];
        index.extend_from_slice(&document);
        let decoded = decode_index(&index).unwrap();
        assert_eq!(decoded.documents[0].occurrences[0].symbol, "s");
        assert!(decoded.documents[0].occurrences[0].definition);
        for end in 1..index.len() {
            assert!(decode_index(&index[..end]).is_err(), "prefix {end}");
        }
        assert!(
            decode_index(&[0x12, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01])
                .is_err()
        );
        assert!(decode_index(&[0x0b]).is_err());
    }
}
