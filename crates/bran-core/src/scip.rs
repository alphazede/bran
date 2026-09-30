//! Exact symbol navigation facts read from an existing SCIP index (issue #38).
//!
//! BRAN never generates an index. It validates the parts of the SCIP
//! protobuf schema (sourcegraph/scip `scip.proto`) it reads: document paths
//! and text, occurrences with their ranges and roles, and symbol kinds and
//! implementation relationships. Anything malformed is a decode error, never
//! a partial guess. Validated views borrow from the index bytes, so decoding
//! allocates nothing per message, and every collection a query builds is
//! bounded before it grows.

use std::collections::{BTreeMap, BTreeSet};

/// Largest index BRAN will read.
pub const MAX_INDEX_BYTES: u64 = 256 * 1024 * 1024;

/// Most distinct symbols one query selects, and most (implementing symbol,
/// selected symbol) pairs it tracks.
const MAX_SYMBOLS: usize = 1024;

/// Most candidate facts one query examines.
const MAX_CANDIDATES: usize = 1 << 20;

/// Occurrence coordinates are protobuf `int32` values and never negative.
const MAX_COORDINATE: u64 = i32::MAX as u64;

/// Largest protobuf field number.
const MAX_FIELD_NUMBER: u64 = (1 << 29) - 1;

/// `SymbolRole.Definition` in `Occurrence.symbol_roles`.
const DEFINITION_ROLE: u64 = 1;

/// `SymbolInformation.Kind` values 0..=86 in snake case; 83 is unassigned.
const KIND_NAMES: &str = "unspecified_kind array assertion associated_type attribute axiom boolean class constant constructor data_family enum enum_member event fact field file function getter grammar instance interface key lang lemma macro method method_receiver message module namespace null number object operator package package_object parameter parameter_label pattern predicate property protocol quasiquoter self_parameter setter signature subscript string struct tactic theorem this_parameter trait type type_alias type_class type_family type_parameter union value variable contract error library modifier abstract_method method_specification protocol_method pure_virtual_method trait_method type_class_method accessor delegate method_alias singleton_class singleton_method static_data_member static_event static_field static_method static_property static_variable unspecified_kind extension mixin concept";

/// A validated SCIP `Index`.
pub struct Index<'a> {
    bytes: &'a [u8],
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
#[derive(Debug, Eq, PartialEq)]
pub struct Evidence {
    pub locator: String,
    pub start_line: u32,
    pub end_line: u32,
    pub role: Role,
    pub symbol: String,
    /// The snake-case `SymbolInformation.Kind` recorded for `symbol`.
    pub kind: &'static str,
    /// For an implementation, the selected symbol it implements.
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

#[derive(Clone, Copy)]
struct DocumentView<'a> {
    path: &'a str,
    text: &'a str,
    body: &'a [u8],
}

#[derive(Clone, Copy)]
struct OccurrenceView<'a> {
    symbol: &'a str,
    definition: bool,
    /// Zero-based, inclusive, at most `MAX_COORDINATE`.
    start_line: u32,
    end_line: u32,
}

#[derive(Clone, Copy)]
struct InformationView<'a> {
    symbol: &'a str,
    kind: u64,
    body: &'a [u8],
}

/// One candidate fact: rank, locator, lines, role, symbol, implemented symbol.
type Candidate<'a> = (usize, &'a str, u32, u32, Role, &'a str, Option<&'a str>);

impl<'a> Index<'a> {
    /// Compares each document's recorded text with the scanned source.
    pub fn freshness<'s>(&self, scanned: impl Fn(&str) -> Option<&'s [u8]>) -> Freshness {
        let mut freshness = Freshness::Fresh;
        for document in self.documents() {
            match scanned(document.path) {
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

    /// Definitions, references, and implementations of the global symbols
    /// whose name equals one of `names` (compared ASCII-lowercase), in the
    /// documents `ranked` maps to a rank. Returns at most `limit` facts in
    /// rank, locator, and line order, and whether the index holds more: past
    /// the limit, in an unranked document, or past a collection bound.
    pub fn evidence(
        &self,
        names: &BTreeSet<String>,
        ranked: &BTreeMap<&str, usize>,
        limit: usize,
    ) -> (Vec<Evidence>, bool) {
        let mut truncated = false;
        let mut selected = BTreeSet::new();
        let occurrences = self
            .documents()
            .flat_map(DocumentView::occurrences)
            .map(|occurrence| occurrence.symbol);
        for symbol in occurrences.chain(self.informations().map(|information| information.symbol)) {
            if selected.contains(symbol)
                || !symbol_name(symbol).is_some_and(|name| {
                    name.navigable && names.contains(&name.name.to_ascii_lowercase())
                })
            {
                continue;
            }
            if selected.len() == MAX_SYMBOLS {
                truncated = true;
                break;
            }
            selected.insert(symbol);
        }

        let mut implemented = BTreeMap::<&str, BTreeSet<&str>>::new();
        let mut pairs = 0;
        for information in self.informations() {
            for target in information.implements() {
                if !selected.contains(target)
                    || implemented
                        .get(information.symbol)
                        .is_some_and(|targets| targets.contains(target))
                {
                    continue;
                }
                if pairs == MAX_SYMBOLS {
                    truncated = true;
                    continue;
                }
                implemented
                    .entry(information.symbol)
                    .or_default()
                    .insert(target);
                pairs += 1;
            }
        }

        // Keep only the first `limit` distinct facts while collecting.
        let mut kept = BTreeSet::<Candidate>::new();
        let mut candidates = 0;
        'documents: for document in self.documents() {
            let rank = ranked.get(document.path).copied();
            for occurrence in document.occurrences() {
                let is_selected = selected.contains(occurrence.symbol);
                let targets = occurrence
                    .definition
                    .then(|| implemented.get(occurrence.symbol))
                    .flatten();
                if !is_selected && targets.is_none() {
                    continue;
                }
                let Some(rank) = rank else {
                    truncated = true;
                    continue;
                };
                let own = is_selected.then_some(if occurrence.definition {
                    (Role::Definition, None)
                } else {
                    (Role::Reference, None)
                });
                let implementations = targets
                    .into_iter()
                    .flatten()
                    .map(|target| (Role::Implementation, Some(*target)));
                for (role, implements) in own.into_iter().chain(implementations) {
                    candidates += 1;
                    if candidates > MAX_CANDIDATES {
                        truncated = true;
                        break 'documents;
                    }
                    kept.insert((
                        rank,
                        document.path,
                        occurrence.start_line,
                        occurrence.end_line,
                        role,
                        occurrence.symbol,
                        implements,
                    ));
                    if kept.len() > limit {
                        kept.pop_last();
                        truncated = true;
                    }
                }
            }
        }

        let wanted = kept
            .iter()
            .map(|candidate| candidate.5)
            .collect::<BTreeSet<_>>();
        let mut kinds = BTreeMap::new();
        for information in self.informations() {
            if wanted.contains(information.symbol) {
                kinds.entry(information.symbol).or_insert(information.kind);
            }
        }
        let evidence = kept
            .into_iter()
            .map(
                |(_, locator, start_line, end_line, role, symbol, implements)| Evidence {
                    locator: locator.to_owned(),
                    // Coordinates are at most `i32::MAX`, so this cannot overflow.
                    start_line: start_line + 1,
                    end_line: end_line + 1,
                    role,
                    symbol: symbol.to_owned(),
                    kind: kind_name(kinds.get(symbol).copied().unwrap_or(0)),
                    implements: implements.map(str::to_owned),
                },
            )
            .collect();
        (evidence, truncated)
    }

    fn documents(&self) -> impl Iterator<Item = DocumentView<'a>> {
        messages(self.bytes, 2).filter_map(|bytes| document(bytes).ok())
    }

    /// Symbol information from every document, then external symbols.
    fn informations(&self) -> impl Iterator<Item = InformationView<'a>> {
        self.documents()
            .flat_map(|document| messages(document.body, 3))
            .chain(messages(self.bytes, 3))
            .filter_map(|bytes| information(bytes).ok())
    }
}

impl<'a> DocumentView<'a> {
    fn occurrences(self) -> impl Iterator<Item = OccurrenceView<'a>> {
        messages(self.body, 2).filter_map(|bytes| occurrence(bytes).ok())
    }
}

impl<'a> InformationView<'a> {
    /// Symbols this one implements (`Relationship.is_implementation`).
    fn implements(self) -> impl Iterator<Item = &'a str> {
        messages(self.body, 4)
            .filter_map(|bytes| relationship(bytes).ok())
            .filter_map(|(target, implementation)| implementation.then_some(target))
    }
}

fn kind_name(kind: u64) -> &'static str {
    usize::try_from(kind)
        .ok()
        .and_then(|kind| KIND_NAMES.split(' ').nth(kind))
        .unwrap_or("unspecified_kind")
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

/// Validates a SCIP `Index` message and every document, occurrence, symbol,
/// and relationship in it, without allocating per message.
pub fn decode_index(bytes: &[u8]) -> Result<Index<'_>, DecodeError> {
    for field in fields(bytes) {
        match field? {
            (2, value) => {
                let document = document(value.bytes()?)?;
                for field in fields(document.body) {
                    match field? {
                        (2, value) => {
                            occurrence(value.bytes()?)?;
                        }
                        (3, value) => {
                            information(value.bytes()?)?;
                        }
                        _ => {}
                    }
                }
            }
            (3, value) => {
                information(value.bytes()?)?;
            }
            _ => {}
        }
    }
    Ok(Index { bytes })
}

fn document(bytes: &[u8]) -> Result<DocumentView<'_>, DecodeError> {
    let mut document = DocumentView {
        path: "",
        text: "",
        body: bytes,
    };
    for field in fields(bytes) {
        match field? {
            (1, value) => document.path = value.str()?,
            (5, value) => document.text = value.str()?,
            _ => {}
        }
    }
    Ok(document)
}

fn occurrence(bytes: &[u8]) -> Result<OccurrenceView<'_>, DecodeError> {
    let mut symbol = "";
    let mut roles = 0;
    // The deprecated `range` holds three or four coordinates; a fifth is
    // rejected before it is stored.
    let mut range = [0; 4];
    let mut count = 0;
    let mut push = |value| {
        *range.get_mut(count).ok_or(DecodeError)? = value;
        count += 1;
        Ok(())
    };
    let mut typed = None;
    for field in fields(bytes) {
        match field? {
            // Packed, or one unpacked element per field.
            (1, Value::Bytes(packed)) => {
                let mut reader = Reader {
                    bytes: packed,
                    at: 0,
                };
                while reader.at < packed.len() {
                    push(reader.varint()?)?;
                }
            }
            (1, Value::Varint(element)) => push(element)?,
            (2, value) => symbol = value.str()?,
            (3, value) => roles = value.varint()?,
            // `single_line_range`: line = 1, start_character = 2, end_character = 3.
            (8, value) => {
                let [line, start, end] = message_varints(value.bytes()?, [1, 2, 3])?;
                typed = Some([line, start, line, end]);
            }
            // `multi_line_range`: start_line, start_character, end_line, end_character.
            (9, value) => typed = Some(message_varints(value.bytes()?, [1, 2, 3, 4])?),
            _ => {}
        }
    }
    let [start_line, start_character, end_line, end_character] = match (typed, &range[..count]) {
        (Some(coordinates), _) => coordinates,
        (None, [line, start, end]) => [*line, *start, *line, *end],
        (None, [start_line, start, end_line, end]) => [*start_line, *start, *end_line, *end],
        _ => return Err(DecodeError),
    };
    if [start_line, start_character, end_line, end_character]
        .iter()
        .any(|coordinate| *coordinate > MAX_COORDINATE)
        || (end_line, end_character) < (start_line, start_character)
    {
        return Err(DecodeError);
    }
    let line = |value| u32::try_from(value).map_err(|_| DecodeError);
    Ok(OccurrenceView {
        symbol,
        definition: roles & DEFINITION_ROLE != 0,
        start_line: line(start_line)?,
        end_line: line(end_line)?,
    })
}

/// A symbol information message; its symbol must not be empty.
fn information(bytes: &[u8]) -> Result<InformationView<'_>, DecodeError> {
    let mut information = InformationView {
        symbol: "",
        kind: 0,
        body: bytes,
    };
    for field in fields(bytes) {
        match field? {
            (1, value) => information.symbol = value.str()?,
            (4, value) => {
                relationship(value.bytes()?)?;
            }
            (5, value) => information.kind = value.varint()?,
            _ => {}
        }
    }
    if information.symbol.is_empty() {
        return Err(DecodeError);
    }
    Ok(information)
}

/// A relationship's target symbol and whether it is an implementation.
fn relationship(bytes: &[u8]) -> Result<(&str, bool), DecodeError> {
    let mut target = "";
    let mut implementation = false;
    for field in fields(bytes) {
        match field? {
            (1, value) => target = value.str()?,
            (3, value) => implementation = value.varint()? != 0,
            _ => {}
        }
    }
    Ok((target, implementation))
}

/// The last value of each wanted varint field in one message; zero when absent.
fn message_varints<const N: usize>(
    bytes: &[u8],
    wanted: [u64; N],
) -> Result<[u64; N], DecodeError> {
    let mut found = [0; N];
    for field in fields(bytes) {
        let (field, value) = field?;
        if let Some(slot) = wanted.iter().position(|wanted| *wanted == field) {
            found[slot] = value.varint()?;
        }
    }
    Ok(found)
}

/// Length-delimited values of field `wanted` in an already validated message.
fn messages(bytes: &[u8], wanted: u64) -> impl Iterator<Item = &[u8]> {
    fields(bytes)
        .map_while(Result::ok)
        .filter_map(move |(field, value)| (field == wanted).then(|| value.bytes().ok()).flatten())
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

    fn str(self) -> Result<&'a str, DecodeError> {
        std::str::from_utf8(self.bytes()?).map_err(|_| DecodeError)
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    /// A base-128 varint of at most ten bytes whose value fits in 64 bits.
    fn varint(&mut self) -> Result<u64, DecodeError> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *self.bytes.get(self.at).ok_or(DecodeError)?;
            self.at += 1;
            if shift == 63 && byte > 1 {
                return Err(DecodeError);
            }
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

    /// One field. Field number zero or above the protobuf maximum, group
    /// wire types, and truncated input are errors.
    fn field(&mut self) -> Result<(u64, Value<'a>), DecodeError> {
        let key = self.varint()?;
        let number = key >> 3;
        if number == 0 || number > MAX_FIELD_NUMBER {
            return Err(DecodeError);
        }
        let value = match key & 7 {
            0 => Value::Varint(self.varint()?),
            1 => {
                self.take(8)?;
                Value::Fixed
            }
            2 => {
                let length = self.varint()?;
                Value::Bytes(self.take(length)?)
            }
            5 => {
                self.take(4)?;
                Value::Fixed
            }
            _ => return Err(DecodeError),
        };
        Ok((number, value))
    }
}

/// The fields of one message; iteration stops after the first error.
fn fields(bytes: &[u8]) -> impl Iterator<Item = Result<(u64, Value<'_>), DecodeError>> {
    let mut reader = Reader { bytes, at: 0 };
    let mut failed = false;
    std::iter::from_fn(move || {
        if failed || reader.at == reader.bytes.len() {
            return None;
        }
        let field = reader.field();
        failed = field.is_err();
        Some(field)
    })
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
        let document = decoded.documents().next().unwrap();
        let occurrence = document.occurrences().next().unwrap();
        assert_eq!((document.path, occurrence.symbol), ("a.rs", "s"));
        assert!(occurrence.definition);
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
