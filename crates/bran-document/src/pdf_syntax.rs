//! Bounded PDF object reader for the PDF adapter (issue #23).
//!
//! BRAN reads PDF syntax itself rather than through `hayro-syntax`, which
//! aborted on deep nesting and inflates streams without a bound (see the
//! dependency review). Every nesting level, object, and decoded byte here
//! counts against `Limits`, so hostile input ends in a typed refusal. Nothing
//! is executed, fetched, or decrypted.

use crate::{Cancel, Limits, Refusal};
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

pub(crate) type Dict = BTreeMap<Vec<u8>, Obj>;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Obj {
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    Name(Vec<u8>),
    Str(Vec<u8>),
    Arr(Vec<Obj>),
    Dict(Dict),
    Ref(u32),
    Stream(Dict, Vec<u8>),
}

impl Obj {
    pub(crate) fn int(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(*value),
            _ => None,
        }
    }

    pub(crate) fn number(&self) -> Option<f64> {
        match self {
            Self::Int(value) => Some(*value as f64),
            Self::Real(value) => Some(*value),
            _ => None,
        }
    }

    pub(crate) fn name(&self) -> Option<&[u8]> {
        match self {
            Self::Name(name) => Some(name),
            _ => None,
        }
    }
}

/// `dict[key]` for a `&str` key.
pub(crate) fn key<'a>(dict: &'a Dict, key: &str) -> Option<&'a Obj> {
    dict.get(key.as_bytes())
}

pub(crate) fn is_name(dict: &Dict, name: &str, value: &str) -> bool {
    key(dict, name).and_then(Obj::name) == Some(value.as_bytes())
}

// ---- Lexer ----

#[derive(Debug, PartialEq)]
pub(crate) enum Token<'a> {
    Int(i64),
    Real(f64),
    Name(Vec<u8>),
    Str(Vec<u8>),
    ArrayOpen,
    ArrayClose,
    DictOpen,
    DictClose,
    Word(&'a [u8]),
}

fn is_space(byte: u8) -> bool {
    matches!(byte, 0 | 9 | 10 | 12 | 13 | 32)
}

fn is_delimiter(byte: u8) -> bool {
    matches!(
        byte,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

fn is_regular(byte: u8) -> bool {
    !is_space(byte) && !is_delimiter(byte)
}

#[derive(Clone)]
pub(crate) struct Lexer<'a> {
    pub(crate) data: &'a [u8],
    pub(crate) at: usize,
}

impl<'a> Lexer<'a> {
    pub(crate) fn new(data: &'a [u8], at: usize) -> Self {
        Self { data, at }
    }

    pub(crate) fn skip_space(&mut self) {
        while let Some(&byte) = self.data.get(self.at) {
            if is_space(byte) {
                self.at += 1;
            } else if byte == b'%' {
                while self
                    .data
                    .get(self.at)
                    .is_some_and(|b| *b != b'\n' && *b != b'\r')
                {
                    self.at += 1;
                }
            } else {
                break;
            }
        }
    }

    /// The next token, `Ok(None)` at the end, or `PdfMalformed` for an
    /// unterminated string.
    pub(crate) fn token(&mut self) -> Result<Option<Token<'a>>, Refusal> {
        self.skip_space();
        let Some(&byte) = self.data.get(self.at) else {
            return Ok(None);
        };
        let start = self.at;
        self.at += 1;
        let token = match byte {
            b'[' => Token::ArrayOpen,
            b']' => Token::ArrayClose,
            b'<' if self.data.get(self.at) == Some(&b'<') => {
                self.at += 1;
                Token::DictOpen
            }
            b'>' if self.data.get(self.at) == Some(&b'>') => {
                self.at += 1;
                Token::DictClose
            }
            b'<' => Token::Str(self.hex_string()?),
            b'(' => Token::Str(self.literal_string()?),
            b'/' => Token::Name(self.name()),
            b'{' | b'}' | b')' | b'>' => Token::Word(&self.data[start..self.at]),
            _ => {
                while self.data.get(self.at).is_some_and(|b| is_regular(*b)) {
                    self.at += 1;
                }
                let word = &self.data[start..self.at];
                number(word).unwrap_or(Token::Word(word))
            }
        };
        Ok(Some(token))
    }

    fn name(&mut self) -> Vec<u8> {
        let mut name = Vec::new();
        while let Some(&byte) = self.data.get(self.at).filter(|b| is_regular(**b)) {
            self.at += 1;
            let escaped = self
                .data
                .get(self.at..self.at + 2)
                .and_then(|hex| std::str::from_utf8(hex).ok())
                .and_then(|hex| u8::from_str_radix(hex, 16).ok());
            match escaped {
                Some(value) if byte == b'#' => {
                    self.at += 2;
                    name.push(value);
                }
                _ => name.push(byte),
            }
        }
        name
    }

    fn hex_string(&mut self) -> Result<Vec<u8>, Refusal> {
        let mut digits = Vec::new();
        loop {
            let byte = *self.data.get(self.at).ok_or(Refusal::PdfMalformed)?;
            self.at += 1;
            match byte {
                b'>' => break,
                _ if is_space(byte) => {}
                _ => digits.push((byte as char).to_digit(16).ok_or(Refusal::PdfMalformed)? as u8),
            }
        }
        if digits.len() % 2 == 1 {
            digits.push(0);
        }
        Ok(digits
            .chunks(2)
            .map(|pair| pair[0] << 4 | pair[1])
            .collect())
    }

    fn literal_string(&mut self) -> Result<Vec<u8>, Refusal> {
        let mut out = Vec::new();
        let mut depth = 1usize;
        loop {
            let byte = *self.data.get(self.at).ok_or(Refusal::PdfMalformed)?;
            self.at += 1;
            match byte {
                b'(' => {
                    depth += 1;
                    out.push(byte);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(out);
                    }
                    out.push(byte);
                }
                b'\r' => {
                    if self.data.get(self.at) == Some(&b'\n') {
                        self.at += 1;
                    }
                    out.push(b'\n');
                }
                b'\\' => {
                    let escaped = *self.data.get(self.at).ok_or(Refusal::PdfMalformed)?;
                    self.at += 1;
                    match escaped {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'0'..=b'7' => {
                            let mut value = u32::from(escaped - b'0');
                            for _ in 0..2 {
                                match self.data.get(self.at) {
                                    Some(digit @ b'0'..=b'7') => {
                                        value = value * 8 + u32::from(digit - b'0');
                                        self.at += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push(value as u8);
                        }
                        b'\r' => {
                            if self.data.get(self.at) == Some(&b'\n') {
                                self.at += 1;
                            }
                        }
                        b'\n' => {}
                        other => out.push(other),
                    }
                }
                _ => out.push(byte),
            }
        }
    }
}

fn number(word: &[u8]) -> Option<Token<'static>> {
    let text = std::str::from_utf8(word).ok()?;
    let digits = text.trim_start_matches(['+', '-']);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return None;
    }
    if !digits.contains('.') {
        if let Ok(value) = text.parse::<i64>() {
            return Some(Token::Int(value));
        }
    }
    let value: f64 = text
        .trim_start_matches('+')
        .parse()
        .or_else(|_| format!("{text}0").parse())
        .ok()?;
    value.is_finite().then_some(Token::Real(value))
}

// ---- Objects ----

/// Nesting and node budgets for one parse. Content streams pass no node
/// budget: their size is already bounded by the decoded-byte budget.
pub(crate) struct Bounds {
    pub(crate) max_depth: usize,
    pub(crate) max_nodes: usize,
    pub(crate) nodes: Cell<usize>,
}

impl Bounds {
    pub(crate) fn new(max_depth: usize, max_nodes: usize) -> Self {
        Self {
            max_depth,
            max_nodes,
            nodes: Cell::new(0),
        }
    }

    fn node(&self) -> Result<(), Refusal> {
        let nodes = self.nodes.get() + 1;
        self.nodes.set(nodes);
        if nodes > self.max_nodes {
            return Err(Refusal::PdfObjectLimit);
        }
        Ok(())
    }
}

/// Parses one object that starts with `first`. `n g R` becomes a reference.
pub(crate) fn parse_object(
    lexer: &mut Lexer<'_>,
    first: Token<'_>,
    depth: usize,
    bounds: &Bounds,
) -> Result<Obj, Refusal> {
    if depth > bounds.max_depth {
        return Err(Refusal::PdfDepthLimit);
    }
    bounds.node()?;
    Ok(match first {
        Token::Int(value) => {
            let saved = lexer.at;
            match (lexer.token()?, lexer.token()?) {
                (Some(Token::Int(0..=65535)), Some(Token::Word(b"R")))
                    if (0..=i64::from(u32::MAX)).contains(&value) =>
                {
                    Obj::Ref(value as u32)
                }
                _ => {
                    lexer.at = saved;
                    Obj::Int(value)
                }
            }
        }
        Token::Real(value) => Obj::Real(value),
        Token::Name(name) => Obj::Name(name),
        Token::Str(bytes) => Obj::Str(bytes),
        Token::ArrayOpen => {
            let mut items = Vec::new();
            loop {
                match lexer.token()?.ok_or(Refusal::PdfMalformed)? {
                    Token::ArrayClose => break Obj::Arr(items),
                    token => items.push(parse_object(lexer, token, depth + 1, bounds)?),
                }
            }
        }
        Token::DictOpen => {
            let mut dict = Dict::new();
            loop {
                match lexer.token()?.ok_or(Refusal::PdfMalformed)? {
                    Token::DictClose => break Obj::Dict(dict),
                    Token::Name(name) => {
                        let token = lexer.token()?.ok_or(Refusal::PdfMalformed)?;
                        if token == Token::DictClose {
                            // A key without a value: ignore it, as readers do.
                            break Obj::Dict(dict);
                        }
                        let value = parse_object(lexer, token, depth + 1, bounds)?;
                        dict.insert(name, value);
                    }
                    _ => return Err(Refusal::PdfMalformed),
                }
            }
        }
        Token::Word(b"true") => Obj::Bool(true),
        Token::Word(b"false") => Obj::Bool(false),
        Token::Word(b"null") => Obj::Null,
        _ => return Err(Refusal::PdfMalformed),
    })
}

// ---- Document ----

type Entries = BTreeMap<u32, Entry>;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Entry {
    At(usize),
    In(u32),
    Free,
}

pub(crate) struct Doc<'a> {
    pub(crate) data: &'a [u8],
    pub(crate) objects: BTreeMap<u32, Obj>,
    pub(crate) trailer: Dict,
    pub(crate) receipts: BTreeSet<&'static str>,
    pub(crate) limits: Limits,
    pub(crate) bounds: Bounds,
    entries: BTreeMap<u32, Entry>,
    spent: Cell<u64>,
    dangling: Cell<bool>,
    length_repaired: Cell<bool>,
}

impl<'a> Doc<'a> {
    /// Reads the cross-reference data and every object. A damaged
    /// cross-reference table is rebuilt by scanning object headers and
    /// reported as `xref-repaired`. Encrypted files are refused.
    pub(crate) fn open(data: &'a [u8], limits: &Limits, cancel: &Cancel) -> Result<Self, Refusal> {
        let header = &data[..data.len().min(1024)];
        if !header.windows(5).any(|window| window == b"%PDF-") {
            return Err(Refusal::PdfMalformed);
        }
        let mut doc = Doc {
            data,
            objects: BTreeMap::new(),
            trailer: Dict::new(),
            receipts: BTreeSet::new(),
            limits: limits.clone(),
            bounds: Bounds::new(limits.max_xml_depth, limits.max_xml_nodes),
            entries: BTreeMap::new(),
            spent: Cell::new(0),
            dangling: Cell::new(false),
            length_repaired: Cell::new(false),
        };
        let read = match doc.read_xref(cancel) {
            Err(Refusal::PdfMalformed) => None,
            other => other?,
        };
        match read {
            Some((entries, trailer))
                if key(&trailer, "Root").is_some() && doc.offsets_valid(&entries) =>
            {
                doc.trailer = trailer;
                doc.entries = entries;
            }
            _ => {
                doc.receipts.insert("xref-repaired");
                doc.entries = doc.rebuild_xref(cancel)?;
            }
        }
        if key(&doc.trailer, "Encrypt").is_some() {
            return Err(Refusal::Encrypted);
        }
        doc.load(cancel)?;
        if doc.length_repaired.get() {
            doc.receipts.insert("stream-length-repaired");
        }
        Ok(doc)
    }

    /// Resolves a reference; a reference to a missing object is null and is
    /// reported as `dangling-reference`.
    pub(crate) fn get<'b>(&'b self, object: &'b Obj) -> &'b Obj {
        let mut current = object;
        for _ in 0..8 {
            match current {
                Obj::Ref(number) => match self.objects.get(number) {
                    Some(found) => current = found,
                    None => {
                        self.dangling.set(true);
                        return &Obj::Null;
                    }
                },
                other => return other,
            }
        }
        &Obj::Null
    }

    pub(crate) fn dict<'b>(&'b self, object: &'b Obj) -> Option<&'b Dict> {
        match self.get(object) {
            Obj::Dict(dict) | Obj::Stream(dict, _) => Some(dict),
            _ => None,
        }
    }

    pub(crate) fn lookup<'b>(&'b self, dict: &'b Dict, name: &str) -> &'b Obj {
        key(dict, name).map_or(&Obj::Null, |value| self.get(value))
    }

    pub(crate) fn dangling_seen(&self) -> bool {
        self.dangling.get()
    }

    /// Counts expanded bytes (inflated data and executed content) against
    /// the total byte budget.
    pub(crate) fn spend(&self, bytes: usize) -> Result<(), Refusal> {
        let spent = self.spent.get().saturating_add(bytes as u64);
        self.spent.set(spent);
        if spent > self.limits.max_total_bytes {
            return Err(Refusal::DecompressionLimit);
        }
        Ok(())
    }

    /// Decodes a stream within the byte budgets. `Ok(None)` means the filter
    /// is not supported or the data is corrupt; the caller records it.
    pub(crate) fn decode(&self, dict: &Dict, raw: &[u8]) -> Result<Option<Vec<u8>>, Refusal> {
        if raw.len() as u64 > self.limits.max_part_bytes {
            return Err(Refusal::Oversized);
        }
        let filters = match self.lookup(dict, "Filter") {
            Obj::Null => Vec::new(),
            Obj::Name(name) => vec![name.as_slice()],
            Obj::Arr(items) => items.iter().filter_map(Obj::name).collect(),
            _ => return Ok(None),
        };
        let params = match self.lookup(dict, "DecodeParms") {
            Obj::Arr(items) => items.iter().map(|item| self.dict(item)).collect(),
            other => vec![self.dict(other)],
        };
        let mut data = raw.to_vec();
        for (index, filter) in filters.into_iter().enumerate() {
            if !matches!(filter, b"FlateDecode" | b"Fl") {
                return Ok(None);
            }
            let budget = self.limits.max_part_bytes.min(
                self.limits
                    .max_ratio
                    .saturating_mul(data.len().max(1) as u64),
            );
            let mut inflated = Vec::new();
            let read = flate2::read::ZlibDecoder::new(data.as_slice())
                .take(budget + 1)
                .read_to_end(&mut inflated);
            if inflated.len() as u64 > budget {
                return Err(Refusal::DecompressionLimit);
            }
            if read.is_err() {
                return Ok(None);
            }
            let param = params.get(index).copied().flatten();
            data = match param.map(|p| self.lookup(p, "Predictor").int().unwrap_or(1)) {
                None | Some(1) => inflated,
                Some(10..=15) => match png_predictor(&inflated, param.unwrap(), self) {
                    Some(data) => data,
                    None => return Ok(None),
                },
                Some(_) => return Ok(None),
            };
        }
        self.spend(data.len())?;
        Ok(Some(data))
    }

    // -- cross-reference data --

    fn read_xref(&self, cancel: &Cancel) -> Result<Option<(Entries, Dict)>, Refusal> {
        let tail = self.data.len().saturating_sub(4096);
        let Some(found) = self.data[tail..]
            .windows(9)
            .rposition(|w| w == b"startxref")
        else {
            return Ok(None);
        };
        let mut lexer = Lexer::new(self.data, tail + found + 9);
        let Some(Token::Int(start)) = lexer.token()? else {
            return Ok(None);
        };
        let mut entries = BTreeMap::new();
        let mut trailer = Dict::new();
        let mut pending = vec![start];
        let mut seen = BTreeSet::new();
        while let Some(start) = pending.pop() {
            cancel.check()?;
            let Ok(start) = usize::try_from(start) else {
                return Ok(None);
            };
            if !seen.insert(start) || start >= self.data.len() {
                return Ok(None);
            }
            let Some(section) = self.xref_section(start, &mut entries)? else {
                return Ok(None);
            };
            for (name, value) in &section {
                trailer.entry(name.clone()).or_insert_with(|| value.clone());
            }
            if let Some(previous) = key(&section, "Prev").and_then(Obj::int) {
                pending.push(previous);
            }
            // A hybrid file's cross-reference stream is newer than /Prev.
            if let Some(stream) = key(&section, "XRefStm").and_then(Obj::int) {
                pending.push(stream);
            }
        }
        Ok(Some((entries, trailer)))
    }

    /// Adds one section's entries (older sections never override newer ones)
    /// and returns its trailer dictionary.
    fn xref_section(
        &self,
        start: usize,
        entries: &mut BTreeMap<u32, Entry>,
    ) -> Result<Option<Dict>, Refusal> {
        let mut lexer = Lexer::new(self.data, start);
        if self.data[start..].starts_with(b"xref") {
            lexer.at += 4;
            loop {
                match lexer.token()? {
                    Some(Token::Word(b"trailer")) => {
                        let first = lexer.token()?.ok_or(Refusal::PdfMalformed)?;
                        return match parse_object(&mut lexer, first, 0, &self.bounds) {
                            Ok(Obj::Dict(dict)) => Ok(Some(dict)),
                            Err(refusal @ (Refusal::PdfDepthLimit | Refusal::PdfObjectLimit)) => {
                                Err(refusal)
                            }
                            _ => Ok(None),
                        };
                    }
                    Some(Token::Int(first)) => {
                        let Some(Token::Int(count)) = lexer.token()? else {
                            return Ok(None);
                        };
                        for number in first..first.saturating_add(count) {
                            self.bounds.node()?;
                            let (
                                Some(Token::Int(offset)),
                                Some(Token::Int(_)),
                                Some(Token::Word(kind)),
                            ) = (lexer.token()?, lexer.token()?, lexer.token()?)
                            else {
                                return Ok(None);
                            };
                            let entry = match kind {
                                b"n" => Entry::At(usize::try_from(offset).unwrap_or(usize::MAX)),
                                b"f" => Entry::Free,
                                _ => return Ok(None),
                            };
                            if let Ok(number) = u32::try_from(number) {
                                entries.entry(number).or_insert(entry);
                            }
                        }
                    }
                    _ => return Ok(None),
                }
            }
        }
        let Ok((_, Obj::Stream(dict, raw))) = self.indirect_at(start) else {
            return Ok(None);
        };
        if !is_name(&dict, "Type", "XRef") {
            return Ok(None);
        }
        let Some(data) = self.decode(&dict, &raw)? else {
            return Ok(None);
        };
        let widths: Vec<usize> = match key(&dict, "W") {
            Some(Obj::Arr(items)) if items.len() == 3 => items
                .iter()
                .map(|item| {
                    item.int()
                        .and_then(|w| usize::try_from(w).ok())
                        .filter(|w| *w <= 8)
                })
                .collect::<Option<_>>()
                .unwrap_or_default(),
            _ => return Ok(None),
        };
        let row = widths.iter().sum::<usize>();
        if widths.len() != 3 || row == 0 {
            return Ok(None);
        }
        let size = key(&dict, "Size").and_then(Obj::int).unwrap_or(0);
        let index = match key(&dict, "Index") {
            Some(Obj::Arr(items)) => items.iter().filter_map(Obj::int).collect(),
            _ => vec![0, size],
        };
        let mut rows = data.chunks_exact(row);
        for pair in index.chunks(2) {
            let [first, count] = pair else {
                return Ok(None);
            };
            for number in *first..first.saturating_add(*count) {
                self.bounds.node()?;
                let Some(bytes) = rows.next() else {
                    return Ok(None);
                };
                let field = |from: usize, width: usize| {
                    bytes[from..from + width]
                        .iter()
                        .fold(0u64, |value, byte| value << 8 | u64::from(*byte))
                };
                let kind = if widths[0] == 0 {
                    1
                } else {
                    field(0, widths[0])
                };
                let second = field(widths[0], widths[1]);
                let entry = match kind {
                    0 => Entry::Free,
                    1 => Entry::At(usize::try_from(second).unwrap_or(usize::MAX)),
                    2 => Entry::In(u32::try_from(second).unwrap_or(u32::MAX)),
                    _ => continue,
                };
                if let Ok(number) = u32::try_from(number) {
                    entries.entry(number).or_insert(entry);
                }
            }
        }
        Ok(Some(dict))
    }

    fn offsets_valid(&self, entries: &BTreeMap<u32, Entry>) -> bool {
        entries.iter().all(|(number, entry)| match entry {
            Entry::At(offset) => self.header_at(*offset) == Some(*number),
            _ => true,
        })
    }

    /// The object number of an `n g obj` header at `offset`.
    fn header_at(&self, offset: usize) -> Option<u32> {
        let mut lexer = Lexer::new(self.data, offset);
        match (lexer.token(), lexer.token(), lexer.token()) {
            (
                Ok(Some(Token::Int(number))),
                Ok(Some(Token::Int(_))),
                Ok(Some(Token::Word(b"obj"))),
            ) if lexer.data.get(offset).is_some_and(u8::is_ascii_digit) => {
                u32::try_from(number).ok()
            }
            _ => None,
        }
    }

    /// Rebuilds the cross-reference data from object headers; later
    /// definitions win, as incremental updates require.
    fn rebuild_xref(&mut self, cancel: &Cancel) -> Result<BTreeMap<u32, Entry>, Refusal> {
        let mut entries = BTreeMap::new();
        let mut trailer = Dict::new();
        let data = self.data;
        for (at, _) in data.windows(3).enumerate().filter(|(_, w)| *w == b"obj") {
            let after = data.get(at + 3).copied().unwrap_or(b' ');
            if is_regular(after) {
                continue;
            }
            let mut start = at;
            for _ in 0..2 {
                while start > 0 && is_space(data[start - 1]) {
                    start -= 1;
                }
                while start > 0 && data[start - 1].is_ascii_digit() {
                    start -= 1;
                }
            }
            if start > 0 && is_regular(data[start - 1]) {
                continue;
            }
            if let Some(number) = self.header_at(start) {
                cancel.check()?;
                self.bounds.node()?;
                entries.insert(number, Entry::At(start));
            }
        }
        for (at, _) in data
            .windows(7)
            .enumerate()
            .filter(|(_, w)| *w == b"trailer")
        {
            let mut lexer = Lexer::new(data, at + 7);
            if let Ok(Some(Token::DictOpen)) = lexer.token() {
                // The word can also sit inside stream data; only a parsable
                // dictionary counts, but limits still refuse.
                let parsed = match parse_object(&mut lexer, Token::DictOpen, 0, &self.bounds) {
                    Err(refusal @ (Refusal::PdfDepthLimit | Refusal::PdfObjectLimit)) => {
                        return Err(refusal)
                    }
                    other => other,
                };
                if let Ok(Obj::Dict(dict)) = parsed {
                    if key(&dict, "Root").is_some() || key(&dict, "Encrypt").is_some() {
                        trailer = dict;
                    }
                }
            }
        }
        if key(&trailer, "Root").is_none() {
            for (number, entry) in &entries {
                let Entry::At(offset) = entry else { continue };
                match self.indirect_at(*offset) {
                    Ok((_, Obj::Dict(dict))) if is_name(&dict, "Type", "Catalog") => {
                        trailer.insert(b"Root".to_vec(), Obj::Ref(*number));
                    }
                    Ok((_, Obj::Stream(dict, _))) if is_name(&dict, "Type", "XRef") => {
                        for (name, value) in dict {
                            trailer.entry(name).or_insert(value);
                        }
                    }
                    _ => {}
                }
            }
        }
        if key(&trailer, "Root").is_none() {
            return Err(Refusal::PdfMalformed);
        }
        self.trailer = trailer;
        Ok(entries)
    }

    // -- objects --

    fn load(&mut self, cancel: &Cancel) -> Result<(), Refusal> {
        let entries = self.entries.clone();
        let mut packed: BTreeSet<u32> = BTreeSet::new();
        for (number, entry) in &entries {
            cancel.check()?;
            match entry {
                Entry::At(offset) => {
                    let (found, object) = match self.indirect_at(*offset) {
                        Ok(parsed) => parsed,
                        Err(refusal @ (Refusal::PdfDepthLimit | Refusal::PdfObjectLimit)) => {
                            return Err(refusal)
                        }
                        Err(_) => continue,
                    };
                    if found == *number {
                        self.objects.insert(*number, object);
                    }
                }
                Entry::In(stream) => {
                    packed.insert(*stream);
                }
                Entry::Free => {}
            }
        }
        let repaired = self.receipts.contains("xref-repaired");
        if repaired {
            for (number, object) in &self.objects {
                if let Obj::Stream(dict, _) = object {
                    if is_name(dict, "Type", "ObjStm") {
                        packed.insert(*number);
                    }
                }
            }
        }
        for stream in packed {
            cancel.check()?;
            let Some(Obj::Stream(dict, raw)) = self.objects.get(&stream) else {
                continue;
            };
            let (dict, raw) = (dict.clone(), raw.clone());
            let Some(data) = self.decode(&dict, &raw)? else {
                self.receipts.insert("stream-not-decoded");
                continue;
            };
            let count = key(&dict, "N").and_then(Obj::int).unwrap_or(0);
            let first = key(&dict, "First")
                .and_then(Obj::int)
                .and_then(|v| usize::try_from(v).ok());
            let Some(first) = first.filter(|first| *first <= data.len()) else {
                continue;
            };
            let mut header = Lexer::new(&data[..first], 0);
            for _ in 0..count {
                self.bounds.node()?;
                let (Some(Token::Int(number)), Some(Token::Int(offset))) =
                    (header.token()?, header.token()?)
                else {
                    break;
                };
                let (Ok(number), Ok(offset)) = (u32::try_from(number), usize::try_from(offset))
                else {
                    break;
                };
                let belongs = match entries.get(&number) {
                    Some(Entry::In(owner)) => *owner == stream,
                    None => repaired && !self.objects.contains_key(&number),
                    _ => false,
                };
                if !belongs || first + offset >= data.len() {
                    continue;
                }
                let mut lexer = Lexer::new(&data, first + offset);
                let Some(token) = lexer.token()? else {
                    continue;
                };
                match parse_object(&mut lexer, token, 0, &self.bounds) {
                    Ok(object) => {
                        self.objects.insert(number, object);
                    }
                    Err(refusal @ (Refusal::PdfDepthLimit | Refusal::PdfObjectLimit)) => {
                        return Err(refusal)
                    }
                    Err(_) => {}
                }
            }
        }
        Ok(())
    }

    /// Parses `n g obj <object> [stream ... endstream]` at `offset`.
    fn indirect_at(&self, offset: usize) -> Result<(u32, Obj), Refusal> {
        let number = self.header_at(offset).ok_or(Refusal::PdfMalformed)?;
        let mut lexer = Lexer::new(self.data, offset);
        for _ in 0..3 {
            lexer.token()?;
        }
        let first = lexer.token()?.ok_or(Refusal::PdfMalformed)?;
        let object = parse_object(&mut lexer, first, 0, &self.bounds)?;
        let Obj::Dict(dict) = object else {
            return Ok((number, object));
        };
        if lexer.token()? != Some(Token::Word(b"stream")) {
            return Ok((number, Obj::Dict(dict)));
        }
        let mut start = lexer.at;
        if self.data.get(start) == Some(&b'\r') {
            start += 1;
        }
        if self.data.get(start) == Some(&b'\n') {
            start += 1;
        }
        let declared = match key(&dict, "Length") {
            Some(Obj::Int(length)) => usize::try_from(*length).ok(),
            Some(Obj::Ref(target)) => self.plain_int(*target),
            _ => None,
        };
        let fits = |length: usize| {
            let end = start.checked_add(length)?;
            let mut after = Lexer::new(self.data, end);
            after.skip_space();
            self.data[after.at.min(self.data.len())..]
                .starts_with(b"endstream")
                .then_some(end)
        };
        let end = match declared.and_then(fits) {
            Some(end) => end,
            None => {
                let found = self.data[start..]
                    .windows(9)
                    .position(|w| w == b"endstream")
                    .ok_or(Refusal::PdfMalformed)?;
                let mut end = start + found;
                if end > start && self.data[end - 1] == b'\n' {
                    end -= 1;
                }
                if end > start && self.data[end - 1] == b'\r' {
                    end -= 1;
                }
                // An indirect length stored in an object stream is not known
                // yet; finding the end marker is then ordinary, not a repair.
                if declared.is_some() || !matches!(key(&dict, "Length"), Some(Obj::Ref(_))) {
                    self.length_repaired.set(true);
                }
                end
            }
        };
        let raw = self.data[start..end].to_vec();
        Ok((number, Obj::Stream(dict, raw)))
    }

    /// An integer object stored at a plain offset, for indirect lengths.
    fn plain_int(&self, target: u32) -> Option<usize> {
        let Some(Entry::At(offset)) = self.entries.get(&target) else {
            return None;
        };
        let mut lexer = Lexer::new(self.data, *offset);
        match (lexer.token(), lexer.token(), lexer.token(), lexer.token()) {
            (_, _, Ok(Some(Token::Word(b"obj"))), Ok(Some(Token::Int(value)))) => {
                usize::try_from(value).ok()
            }
            _ => None,
        }
    }
}

fn png_predictor(data: &[u8], params: &Dict, doc: &Doc<'_>) -> Option<Vec<u8>> {
    let get = |name: &str, default: i64| doc.lookup(params, name).int().unwrap_or(default);
    let (colors, bits, columns) = (
        get("Colors", 1),
        get("BitsPerComponent", 8),
        get("Columns", 1),
    );
    if !(1..=32).contains(&colors)
        || !matches!(bits, 1 | 2 | 4 | 8 | 16)
        || !(1..=1 << 20).contains(&columns)
    {
        return None;
    }
    let pixel = ((colors * bits + 7) / 8) as usize;
    let width = ((colors * bits * columns + 7) / 8) as usize;
    let mut out = Vec::with_capacity(data.len());
    let mut previous = vec![0u8; width];
    for row in data.chunks(width + 1) {
        let (&kind, bytes) = row.split_first()?;
        let mut current = bytes.to_vec();
        current.resize(width, 0);
        for index in 0..width {
            let left = if index >= pixel {
                current[index - pixel]
            } else {
                0
            };
            let up = previous[index];
            let upper_left = if index >= pixel {
                previous[index - pixel]
            } else {
                0
            };
            let add = match kind {
                0 => 0,
                1 => left,
                2 => up,
                3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
                4 => paeth(left, up, upper_left),
                _ => return None,
            };
            current[index] = current[index].wrapping_add(add);
        }
        out.extend_from_slice(&current);
        previous = current;
    }
    Some(out)
}

fn paeth(left: u8, up: u8, upper_left: u8) -> u8 {
    let estimate = i16::from(left) + i16::from(up) - i16::from(upper_left);
    let (a, b, c) = (
        (estimate - i16::from(left)).abs(),
        (estimate - i16::from(up)).abs(),
        (estimate - i16::from(upper_left)).abs(),
    );
    if a <= b && a <= c {
        left
    } else if b <= c {
        up
    } else {
        upper_left
    }
}

/// Decodes a PDF text string: UTF-16BE or UTF-8 with a byte-order mark,
/// otherwise PDFDocEncoding.
pub(crate) fn text_string(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units = rest
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]));
        return char::decode_utf16(units)
            .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect();
    }
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    const HIGH: &str = "•†‡…—–ƒ⁄‹›−‰„“”‘’‚™ﬁﬂŁŒŠŸŽıłœšž\u{FFFD}€";
    bytes
        .iter()
        .map(|&byte| match byte {
            0x80..=0xA0 => HIGH
                .chars()
                .nth(usize::from(byte - 0x80))
                .unwrap_or('\u{FFFD}'),
            _ => char::from(byte),
        })
        .collect()
}
