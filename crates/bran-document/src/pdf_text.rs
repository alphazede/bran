//! Text, image, and marked-content extraction from PDF content streams.
//!
//! Content is interpreted, never executed: only the text, graphics-state,
//! marked-content, and XObject operators that locate text and images are
//! read. Nesting (`q`, marked content, form XObjects) is bounded by the
//! depth limit, and every executed content byte counts against the total
//! byte budget, so repeated form XObjects cannot amplify work.

use crate::pdf_syntax::{
    is_name, key, parse_object, text_string, Bounds, Dict, Doc, Lexer, Obj, Token,
};
use crate::{Cancel, Refusal};
use std::collections::BTreeMap;
use std::rc::Rc;

pub(crate) type Rect = [f64; 4];
type Matrix = [f64; 6];

const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
/// Operands a single operator may take before the stream counts as malformed.
const MAX_OPERANDS: usize = 1024;

pub(crate) struct Block {
    pub(crate) text: String,
    pub(crate) bbox: Rect,
    pub(crate) heading: bool,
}

pub(crate) struct Image {
    pub(crate) bbox: Rect,
    pub(crate) width: i64,
    pub(crate) height: i64,
}

#[derive(Default)]
pub(crate) struct PageContent {
    pub(crate) blocks: Vec<Block>,
    pub(crate) images: Vec<Image>,
    pub(crate) unmapped_glyphs: bool,
    pub(crate) malformed: bool,
    pub(crate) undecoded: bool,
}

fn multiply(m: &Matrix, n: &Matrix) -> Matrix {
    [
        m[0] * n[0] + m[1] * n[2],
        m[0] * n[1] + m[1] * n[3],
        m[2] * n[0] + m[3] * n[2],
        m[2] * n[1] + m[3] * n[3],
        m[4] * n[0] + m[5] * n[2] + n[4],
        m[4] * n[1] + m[5] * n[3] + n[5],
    ]
}

fn apply(m: &Matrix, x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

fn bounds_of(m: &Matrix, x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
    let corners = [
        apply(m, x0, y0),
        apply(m, x1, y0),
        apply(m, x0, y1),
        apply(m, x1, y1),
    ];
    corners.iter().fold(
        [f64::MAX, f64::MAX, f64::MIN, f64::MIN],
        |[a, b, c, d], (x, y)| [a.min(*x), b.min(*y), c.max(*x), d.max(*y)],
    )
}

fn union(a: Rect, b: Rect) -> Rect {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

// ---- Fonts ----

enum Unicode {
    Range(Vec<u16>),
    Many(Vec<String>),
}

#[derive(Default)]
struct CMap {
    spaces: Vec<(usize, u32, u32)>,
    single: BTreeMap<(usize, u32), String>,
    ranges: Vec<(usize, u32, u32, Unicode)>,
}

impl CMap {
    fn parse(data: &[u8], bounds: &Bounds) -> Result<Self, Refusal> {
        let mut map = Self::default();
        let mut lexer = Lexer::new(data, 0);
        let mut operands: Vec<Obj> = Vec::new();
        let code = |bytes: &[u8]| {
            (
                bytes.len(),
                bytes.iter().fold(0u32, |v, b| v << 8 | u32::from(*b)),
            )
        };
        while let Some(token) = lexer.token()? {
            match token {
                Token::Word(b"endcodespacerange") => {
                    for pair in operands.chunks(2) {
                        if let [Obj::Str(lo), Obj::Str(hi)] = pair {
                            if (1..=4).contains(&lo.len()) {
                                map.spaces.push((lo.len(), code(lo).1, code(hi).1));
                            }
                        }
                    }
                }
                Token::Word(b"endbfchar") => {
                    for pair in operands.chunks(2) {
                        if let [Obj::Str(src), Obj::Str(dst)] = pair {
                            if (1..=4).contains(&src.len()) {
                                map.single.insert(code(src), utf16(dst));
                            }
                        }
                    }
                }
                Token::Word(b"endbfrange") => {
                    for triple in operands.chunks(3) {
                        if let [Obj::Str(lo), Obj::Str(hi), dst] = triple {
                            let target = match dst {
                                Obj::Str(base) => Unicode::Range(units(base)),
                                Obj::Arr(items) => Unicode::Many(
                                    items
                                        .iter()
                                        .map(|item| match item {
                                            Obj::Str(bytes) => utf16(bytes),
                                            _ => String::new(),
                                        })
                                        .collect(),
                                ),
                                _ => continue,
                            };
                            if (1..=4).contains(&lo.len()) {
                                map.ranges.push((lo.len(), code(lo).1, code(hi).1, target));
                            }
                        }
                    }
                }
                Token::Word(_) => {}
                token => {
                    operands.push(parse_object(&mut lexer, token, 0, bounds)?);
                    continue;
                }
            }
            operands.clear();
        }
        Ok(map)
    }

    fn lookup(&self, length: usize, value: u32) -> Option<String> {
        if let Some(text) = self.single.get(&(length, value)) {
            return Some(text.clone());
        }
        self.ranges
            .iter()
            .find(|(len, lo, hi, _)| *len == length && (*lo..=*hi).contains(&value))
            .map(|(_, lo, _, target)| match target {
                Unicode::Range(base) => {
                    let mut units = base.clone();
                    if let Some(last) = units.last_mut() {
                        *last = last.wrapping_add((value - lo) as u16);
                    }
                    String::from_utf16_lossy(&units)
                }
                Unicode::Many(items) => items
                    .get((value - lo) as usize)
                    .cloned()
                    .unwrap_or_default(),
            })
    }
}

fn units(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks(2)
        .map(|pair| u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0)]))
        .collect()
}

fn utf16(bytes: &[u8]) -> String {
    String::from_utf16_lossy(&units(bytes))
}

/// WinAnsiEncoding for 0x80..=0x9F; the rest of the table is Latin-1.
const WIN_ANSI_HIGH: [char; 32] = [
    '€', '\u{FFFD}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{FFFD}', 'Ž',
    '\u{FFFD}', '\u{FFFD}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{FFFD}',
    'ž', 'Ÿ',
];

pub(crate) fn win_ansi(byte: u8) -> Option<char> {
    match byte {
        0x20..=0x7E | 0xA0..=0xFF => Some(char::from(byte)),
        0x80..=0x9F => Some(WIN_ANSI_HIGH[usize::from(byte - 0x80)]).filter(|c| *c != '\u{FFFD}'),
        _ => None,
    }
}

/// Glyph names a `/Differences` array commonly uses. Other names stay
/// unmapped and are reported, never guessed.
fn glyph_char(name: &[u8]) -> Option<char> {
    let name = std::str::from_utf8(name).ok()?;
    if name.len() == 1 && name.as_bytes()[0].is_ascii_alphabetic() {
        return name.chars().next();
    }
    if let Some(hex) = name.strip_prefix("uni").or_else(|| name.strip_prefix('u')) {
        if (4..=6).contains(&hex.len()) {
            return u32::from_str_radix(hex, 16).ok().and_then(char::from_u32);
        }
    }
    const NAMES: &[(&str, char)] = &[
        ("space", ' '),
        ("exclam", '!'),
        ("quotedbl", '"'),
        ("numbersign", '#'),
        ("dollar", '$'),
        ("percent", '%'),
        ("ampersand", '&'),
        ("quotesingle", '\''),
        ("parenleft", '('),
        ("parenright", ')'),
        ("asterisk", '*'),
        ("plus", '+'),
        ("comma", ','),
        ("hyphen", '-'),
        ("period", '.'),
        ("slash", '/'),
        ("zero", '0'),
        ("one", '1'),
        ("two", '2'),
        ("three", '3'),
        ("four", '4'),
        ("five", '5'),
        ("six", '6'),
        ("seven", '7'),
        ("eight", '8'),
        ("nine", '9'),
        ("colon", ':'),
        ("semicolon", ';'),
        ("less", '<'),
        ("equal", '='),
        ("greater", '>'),
        ("question", '?'),
        ("at", '@'),
        ("bracketleft", '['),
        ("backslash", '\\'),
        ("bracketright", ']'),
        ("underscore", '_'),
        ("quoteleft", '‘'),
        ("quoteright", '’'),
        ("quotedblleft", '“'),
        ("quotedblright", '”'),
        ("endash", '–'),
        ("emdash", '—'),
        ("bullet", '•'),
        ("ellipsis", '…'),
    ];
    NAMES
        .iter()
        .find(|(glyph, _)| *glyph == name)
        .map(|(_, c)| *c)
}

struct Font {
    composite: bool,
    to_unicode: Option<CMap>,
    encoding: Vec<Option<char>>,
    first_char: i64,
    widths: Vec<f64>,
    cid_widths: BTreeMap<u32, f64>,
    default_width: f64,
}

impl Font {
    fn load(doc: &Doc<'_>, dict: &Dict, bounds: &Bounds) -> Result<Self, Refusal> {
        let composite = is_name(dict, "Subtype", "Type0");
        let to_unicode = match doc.lookup(dict, "ToUnicode") {
            // A ToUnicode map that does not parse leaves glyphs unmapped.
            Obj::Stream(stream, raw) => doc
                .decode(stream, raw)?
                .and_then(|data| CMap::parse(&data, bounds).ok()),
            _ => None,
        };
        let base = doc.lookup(dict, "BaseFont").name().unwrap_or_default();
        let symbolic = base.ends_with(b"Symbol") || base.ends_with(b"Dingbats");
        let (encoding_name, differences) = match doc.lookup(dict, "Encoding") {
            Obj::Name(name) => (name.as_slice(), None),
            Obj::Dict(encoding) => (
                doc.lookup(encoding, "BaseEncoding")
                    .name()
                    .unwrap_or_default(),
                Some(doc.lookup(encoding, "Differences")),
            ),
            _ => (&b""[..], None),
        };
        let standard = encoding_name.is_empty() && is_name(dict, "Subtype", "Type1");
        let mut encoding: Vec<Option<char>> = (0..=255u8)
            .map(|byte| match encoding_name {
                _ if symbolic => None,
                b"WinAnsiEncoding" => win_ansi(byte),
                // StandardEncoding and MacRomanEncoding share printable ASCII;
                // their upper halves are not mapped.
                _ if standard && byte == b'\'' => Some('’'),
                _ if standard && byte == b'`' => Some('‘'),
                _ if (0x20..=0x7E).contains(&byte) => Some(char::from(byte)),
                b"" if !standard => win_ansi(byte),
                _ => None,
            })
            .collect();
        if let Some(Obj::Arr(items)) = differences {
            let mut code = 0usize;
            for item in items {
                match item {
                    Obj::Int(value) => code = usize::try_from(*value).unwrap_or(256),
                    Obj::Name(name) => {
                        if let Some(slot) = encoding.get_mut(code) {
                            *slot = glyph_char(name);
                        }
                        code += 1;
                    }
                    _ => {}
                }
            }
        }
        let numbers = |object: &Obj| match doc.get(object) {
            Obj::Arr(items) => items
                .iter()
                .map(|item| doc.get(item).number().unwrap_or(0.0))
                .collect(),
            _ => Vec::new(),
        };
        let mut font = Font {
            composite,
            to_unicode,
            encoding,
            first_char: doc.lookup(dict, "FirstChar").int().unwrap_or(0),
            widths: key(dict, "Widths").map(numbers).unwrap_or_default(),
            cid_widths: BTreeMap::new(),
            // ponytail: unknown widths use half an em; embedded or standard-14
            // metrics would tighten bounding boxes.
            default_width: 500.0,
        };
        if let Some(descriptor) = key(dict, "FontDescriptor").and_then(|d| doc.dict(d)) {
            if let Some(missing) = doc.lookup(descriptor, "MissingWidth").number() {
                font.default_width = missing;
            }
        }
        if composite {
            if let Obj::Arr(descendants) = doc.lookup(dict, "DescendantFonts") {
                if let Some(cid) = descendants.first().and_then(|d| doc.dict(d)) {
                    font.default_width = doc.lookup(cid, "DW").number().unwrap_or(1000.0);
                    if let Obj::Arr(spec) = doc.lookup(cid, "W") {
                        font.cid_widths = cid_widths(doc, spec);
                    }
                }
            }
        }
        Ok(font)
    }

    /// Splits `bytes` into codes: (code length, code, text or None, width).
    fn glyphs(&self, bytes: &[u8]) -> Vec<(usize, u32, Option<String>, f64)> {
        let mut out = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            let length = self.code_length(&bytes[at..]);
            let slice = &bytes[at..(at + length).min(bytes.len())];
            let code = slice.iter().fold(0u32, |v, b| v << 8 | u32::from(*b));
            at += length;
            let text = match &self.to_unicode {
                Some(map) => map.lookup(slice.len(), code),
                None if self.composite => None,
                None => self.encoding[code as usize & 0xFF].map(String::from),
            };
            let width = if self.composite {
                self.cid_widths
                    .get(&code)
                    .copied()
                    .unwrap_or(self.default_width)
            } else {
                usize::try_from(i64::from(code) - self.first_char)
                    .ok()
                    .and_then(|index| self.widths.get(index).copied())
                    .unwrap_or(self.default_width)
            };
            out.push((slice.len(), code, text, width));
        }
        out
    }

    fn code_length(&self, bytes: &[u8]) -> usize {
        let spaces = self.to_unicode.as_ref().map(|map| &map.spaces);
        if let Some(spaces) = spaces.filter(|spaces| !spaces.is_empty()) {
            for length in 1..=4.min(bytes.len()) {
                let value = bytes[..length]
                    .iter()
                    .fold(0u32, |v, b| v << 8 | u32::from(*b));
                if spaces
                    .iter()
                    .any(|(len, lo, hi)| *len == length && (*lo..=*hi).contains(&value))
                {
                    return length;
                }
            }
        }
        if self.composite {
            2
        } else {
            1
        }
    }
}

/// Glyph widths a composite font may record; further CIDs use the default
/// width, so a hostile `/W` array cannot amplify work.
const MAX_CID_WIDTHS: usize = 65_536;

fn cid_widths(doc: &Doc<'_>, spec: &[Obj]) -> BTreeMap<u32, f64> {
    let mut widths = BTreeMap::new();
    let mut items = spec.iter().map(|item| doc.get(item));
    while let Some(first) = items.next().and_then(Obj::int) {
        if widths.len() >= MAX_CID_WIDTHS {
            break;
        }
        match items.next() {
            Some(Obj::Arr(list)) => {
                for (offset, width) in list.iter().take(MAX_CID_WIDTHS).enumerate() {
                    let cid = first.saturating_add(offset as i64);
                    if let (Ok(cid), Some(width)) = (u32::try_from(cid), doc.get(width).number()) {
                        widths.insert(cid, width);
                    }
                }
            }
            Some(last) => {
                let (Some(last), Some(width)) = (last.int(), items.next().and_then(Obj::number))
                else {
                    break;
                };
                let room = (MAX_CID_WIDTHS - widths.len()) as i64;
                for cid in first..=last.min(first.saturating_add(room - 1)) {
                    if let Ok(cid) = u32::try_from(cid) {
                        widths.insert(cid, width);
                    }
                }
            }
            None => break,
        }
    }
    widths
}

// ---- Interpreter ----

#[derive(Clone)]
struct State {
    ctm: Matrix,
    font: Option<Rc<Font>>,
    size: f64,
    char_space: f64,
    word_space: f64,
    scale: f64,
    leading: f64,
    rise: f64,
}

struct Marked {
    tag: Vec<u8>,
    mcid: Option<i64>,
    /// `/ActualText` replaces the glyphs shown inside the sequence.
    actual: Option<String>,
}

/// Where the previous run ended, its font size, and its baseline direction.
#[derive(Clone, Copy)]
struct Last {
    x: f64,
    y: f64,
    size: f64,
    ux: f64,
    uy: f64,
}

#[derive(Clone, Copy, PartialEq)]
enum BlockKey {
    Mcid(i64),
    TextObject(usize),
}

struct Open {
    key: BlockKey,
    text: String,
    bbox: Rect,
    heading: bool,
    last: Option<Last>,
    gap: bool,
}

struct Interpreter<'d, 'a> {
    doc: &'d Doc<'a>,
    cancel: &'d Cancel,
    bounds: Bounds,
    /// Fonts by dictionary address, so each font dictionary loads once.
    fonts: BTreeMap<usize, Rc<Font>>,
    state: State,
    stack: Vec<State>,
    marked: Vec<Marked>,
    tm: Matrix,
    tlm: Matrix,
    text_objects: usize,
    open: Option<Open>,
    out: PageContent,
}

/// Extracts blocks and images from a page's decoded content streams.
pub(crate) fn page(
    doc: &Doc<'_>,
    content: &[u8],
    resources: Option<&Dict>,
    cancel: &Cancel,
) -> Result<PageContent, Refusal> {
    let mut interpreter = Interpreter {
        doc,
        cancel,
        bounds: Bounds::new(doc.limits.max_xml_depth, usize::MAX),
        fonts: BTreeMap::new(),
        state: State {
            ctm: IDENTITY,
            font: None,
            size: 0.0,
            char_space: 0.0,
            word_space: 0.0,
            scale: 1.0,
            leading: 0.0,
            rise: 0.0,
        },
        stack: Vec::new(),
        marked: Vec::new(),
        tm: IDENTITY,
        tlm: IDENTITY,
        text_objects: 0,
        open: None,
        out: PageContent::default(),
    };
    interpreter.run(content, resources, 0)?;
    interpreter.close_block();
    Ok(interpreter.out)
}

impl Interpreter<'_, '_> {
    fn run(
        &mut self,
        content: &[u8],
        resources: Option<&Dict>,
        depth: usize,
    ) -> Result<(), Refusal> {
        self.cancel.check()?;
        self.doc.spend(content.len())?;
        let mut lexer = Lexer::new(content, 0);
        let mut operands: Vec<Obj> = Vec::new();
        loop {
            let token = match lexer.token() {
                Ok(Some(token)) => token,
                Ok(None) => return Ok(()),
                Err(_) => {
                    self.out.malformed = true;
                    return Ok(());
                }
            };
            let Token::Word(operator) = token else {
                match parse_object(&mut lexer, token, 0, &self.bounds) {
                    Ok(operand) => operands.push(operand),
                    Err(Refusal::PdfDepthLimit) => return Err(Refusal::PdfDepthLimit),
                    Err(_) => {
                        self.out.malformed = true;
                        return Ok(());
                    }
                }
                if operands.len() > MAX_OPERANDS {
                    self.out.malformed = true;
                    return Ok(());
                }
                continue;
            };
            match operator {
                b"true" | b"false" => {
                    operands.push(Obj::Bool(operator == b"true"));
                    continue;
                }
                b"null" => {
                    operands.push(Obj::Null);
                    continue;
                }
                b"BI" => self.inline_image(&mut lexer),
                _ => self.operator(operator, &operands, resources, depth)?,
            }
            operands.clear();
        }
    }

    fn operator(
        &mut self,
        operator: &[u8],
        operands: &[Obj],
        resources: Option<&Dict>,
        depth: usize,
    ) -> Result<(), Refusal> {
        let number = |index: usize| operands.get(index).and_then(Obj::number).unwrap_or(0.0);
        let last_number = |back: usize| {
            operands
                .len()
                .checked_sub(back)
                .and_then(|index| operands[index].number())
                .unwrap_or(0.0)
        };
        let matrix = || -> Matrix {
            [
                number(0),
                number(1),
                number(2),
                number(3),
                number(4),
                number(5),
            ]
        };
        match operator {
            b"q" => {
                if self.stack.len() >= self.doc.limits.max_xml_depth {
                    return Err(Refusal::PdfDepthLimit);
                }
                self.stack.push(self.state.clone());
            }
            b"Q" => {
                if let Some(state) = self.stack.pop() {
                    self.state = state;
                }
            }
            b"cm" => self.state.ctm = multiply(&matrix(), &self.state.ctm),
            b"BT" => {
                self.tm = IDENTITY;
                self.tlm = IDENTITY;
                self.text_objects += 1;
            }
            b"Tf" => {
                self.state.size = last_number(1);
                let name = operands.first().and_then(Obj::name).unwrap_or_default();
                self.state.font = self.font(resources, name)?;
            }
            b"Tc" => self.state.char_space = last_number(1),
            b"Tw" => self.state.word_space = last_number(1),
            b"Tz" => self.state.scale = last_number(1) / 100.0,
            b"TL" => self.state.leading = last_number(1),
            b"Ts" => self.state.rise = last_number(1),
            b"Td" => self.move_line(number(0), number(1)),
            b"TD" => {
                self.state.leading = -number(1);
                self.move_line(number(0), number(1));
            }
            b"Tm" => {
                self.tlm = matrix();
                self.tm = self.tlm;
            }
            b"T*" => self.move_line(0.0, -self.state.leading),
            b"Tj" => self.show(operands.first(), false),
            b"'" => {
                self.move_line(0.0, -self.state.leading);
                self.show(operands.first(), false);
            }
            b"\"" => {
                self.state.word_space = number(0);
                self.state.char_space = number(1);
                self.move_line(0.0, -self.state.leading);
                self.show(operands.get(2), false);
            }
            b"TJ" => {
                if let Some(Obj::Arr(items)) = operands.first() {
                    for item in items {
                        match item {
                            Obj::Str(_) => self.show(Some(item), false),
                            other => {
                                let adjust = other.number().unwrap_or(0.0);
                                let tx = -adjust / 1000.0 * self.state.size * self.state.scale;
                                self.tm = multiply(&[1.0, 0.0, 0.0, 1.0, tx, 0.0], &self.tm);
                                if adjust < -200.0 {
                                    self.show(None, true);
                                }
                            }
                        }
                    }
                }
            }
            b"BMC" | b"BDC" => {
                if self.marked.len() >= self.doc.limits.max_xml_depth {
                    return Err(Refusal::PdfDepthLimit);
                }
                let (mcid, actual) = match operands.get(1) {
                    Some(Obj::Dict(properties)) => (
                        key(properties, "MCID").and_then(Obj::int),
                        match key(properties, "ActualText") {
                            Some(Obj::Str(value)) => Some(text_string(value)),
                            _ => None,
                        },
                    ),
                    _ => (None, None),
                };
                let tag = operands
                    .first()
                    .and_then(Obj::name)
                    .unwrap_or_default()
                    .to_vec();
                self.marked.push(Marked { tag, mcid, actual });
            }
            b"EMC" => {
                self.marked.pop();
            }
            b"Do" => {
                let name = operands.first().and_then(Obj::name).unwrap_or_default();
                self.xobject(resources, name, depth)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn move_line(&mut self, tx: f64, ty: f64) {
        self.tlm = multiply(&[1.0, 0.0, 0.0, 1.0, tx, ty], &self.tlm);
        self.tm = self.tlm;
    }

    fn font(&mut self, resources: Option<&Dict>, name: &[u8]) -> Result<Option<Rc<Font>>, Refusal> {
        let doc = self.doc;
        let Some(fonts) = resources.and_then(|r| doc.dict(doc.lookup(r, "Font"))) else {
            return Ok(None);
        };
        let Some(entry) = fonts.get(name) else {
            return Ok(None);
        };
        let Some(dict) = doc.dict(entry) else {
            return Ok(None);
        };
        let address = dict as *const Dict as usize;
        if let Some(font) = self.fonts.get(&address) {
            return Ok(Some(font.clone()));
        }
        let font = Rc::new(Font::load(doc, dict, &self.bounds)?);
        self.fonts.insert(address, font.clone());
        Ok(Some(font))
    }

    /// Shows one string, or marks a word gap when `gap` is set. Line and
    /// word breaks are measured along the run's own baseline, so rotated
    /// text joins the same way as upright text.
    fn show(&mut self, string: Option<&Obj>, gap: bool) {
        if gap {
            if let Some(open) = &mut self.open {
                open.gap = true;
            }
            return;
        }
        let (Some(Obj::Str(bytes)), Some(font)) = (string, self.state.font.clone()) else {
            return;
        };
        let state = &self.state;
        let mut text = String::new();
        let mut advance = 0.0;
        for (length, code, glyph, width) in font.glyphs(bytes) {
            match glyph {
                Some(glyph) => text.extend(glyph.chars().filter(|c| !c.is_control())),
                None => {
                    text.push(char::REPLACEMENT_CHARACTER);
                    self.out.unmapped_glyphs = true;
                }
            }
            let word = if length == 1 && code == 32 {
                state.word_space
            } else {
                0.0
            };
            advance += (width / 1000.0 * state.size + state.char_space + word) * state.scale;
        }
        let render = multiply(&self.tm, &state.ctm);
        let (low, high) = (state.rise - 0.2 * state.size, state.rise + 0.8 * state.size);
        let bbox = bounds_of(&render, 0.0, low, advance, high);
        let start = apply(&render, 0.0, 0.0);
        let size = state.size.abs() * render[2].hypot(render[3]);
        let length = render[0].hypot(render[1]).max(f64::MIN_POSITIVE);
        let (ux, uy) = (render[0] / length, render[1] / length);
        let end = apply(&render, advance, 0.0);
        self.tm = multiply(&[1.0, 0.0, 0.0, 1.0, advance, 0.0], &self.tm);
        if let Some(marked) = self.marked.iter_mut().rev().find(|m| m.actual.is_some()) {
            text = marked.actual.take().unwrap_or_default();
            marked.actual = Some(String::new());
        }
        if text.trim().is_empty() {
            return;
        }
        let key = self.block_key();
        let offset = |last: &Last| {
            let (dx, dy) = (start.0 - last.x, start.1 - last.y);
            (
                dx * last.ux + dy * last.uy,
                (dx * last.uy - dy * last.ux).abs(),
            )
        };
        // A different marked-content sequence or text object, or a vertical
        // gap wider than one and a half lines, starts a new block.
        if self.open.as_ref().is_some_and(|open| {
            open.key != key
                || open
                    .last
                    .is_some_and(|last| offset(&last).1 > 1.5 * last.size.max(size))
        }) {
            self.close_block();
        }
        let heading = self.heading();
        let open = self.open.get_or_insert_with(|| Open {
            key,
            text: String::new(),
            bbox,
            heading,
            last: None,
            gap: false,
        });
        if let Some(last) = open.last {
            let (along, across) = offset(&last);
            if across > 0.5 * last.size.max(size) {
                open.text.push('\n');
            } else if (open.gap || along > 0.15 * size)
                && !open.text.ends_with(' ')
                && !text.starts_with(' ')
            {
                open.text.push(' ');
            }
        }
        open.text.push_str(&text);
        open.bbox = union(open.bbox, bbox);
        open.last = Some(Last {
            x: end.0,
            y: end.1,
            size,
            ux,
            uy,
        });
        open.gap = false;
    }

    fn block_key(&self) -> BlockKey {
        match self.marked.iter().rev().find_map(|marked| marked.mcid) {
            Some(mcid) => BlockKey::Mcid(mcid),
            None => BlockKey::TextObject(self.text_objects),
        }
    }

    fn heading(&self) -> bool {
        self.marked
            .iter()
            .rev()
            .find(|marked| marked.mcid.is_some())
            .is_some_and(|marked| {
                matches!(
                    marked.tag.as_slice(),
                    b"H" | b"H1" | b"H2" | b"H3" | b"H4" | b"H5" | b"H6" | b"Title"
                )
            })
    }

    /// Closes the open block: trailing spaces and empty lines are dropped.
    fn close_block(&mut self) {
        let Some(open) = self.open.take() else {
            return;
        };
        let text = open
            .text
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let text = text.trim().to_owned();
        if !text.is_empty() {
            self.out.blocks.push(Block {
                text,
                bbox: open.bbox,
                heading: open.heading,
            });
        }
    }

    fn xobject(
        &mut self,
        resources: Option<&Dict>,
        name: &[u8],
        depth: usize,
    ) -> Result<(), Refusal> {
        let doc = self.doc;
        let Some(xobjects) = resources.and_then(|r| doc.dict(doc.lookup(r, "XObject"))) else {
            return Ok(());
        };
        let Some(Obj::Stream(dict, raw)) = xobjects.get(name).map(|x| doc.get(x)) else {
            return Ok(());
        };
        if is_name(dict, "Subtype", "Image") {
            self.out.images.push(Image {
                bbox: bounds_of(&self.state.ctm, 0.0, 0.0, 1.0, 1.0),
                width: doc.lookup(dict, "Width").int().unwrap_or(0),
                height: doc.lookup(dict, "Height").int().unwrap_or(0),
            });
            return Ok(());
        }
        if !is_name(dict, "Subtype", "Form") {
            return Ok(());
        }
        if depth >= doc.limits.max_xml_depth {
            return Err(Refusal::PdfDepthLimit);
        }
        let Some(content) = doc.decode(dict, raw)? else {
            self.out.undecoded = true;
            return Ok(());
        };
        let form: Matrix = match doc.lookup(dict, "Matrix") {
            Obj::Arr(items) if items.len() == 6 => {
                let mut m = IDENTITY;
                for (slot, item) in m.iter_mut().zip(items) {
                    *slot = doc.get(item).number().unwrap_or(0.0);
                }
                m
            }
            _ => IDENTITY,
        };
        let saved = (self.state.clone(), self.stack.len());
        self.state.ctm = multiply(&form, &self.state.ctm);
        let inner = doc.dict(doc.lookup(dict, "Resources")).or(resources);
        self.run(&content, inner, depth + 1)?;
        self.stack.truncate(saved.1);
        self.state = saved.0;
        Ok(())
    }

    fn inline_image(&mut self, lexer: &mut Lexer<'_>) {
        let (mut width, mut height) = (0, 0);
        let mut pending: Option<Vec<u8>> = None;
        loop {
            let token = match lexer.token() {
                Ok(Some(Token::Word(b"ID"))) => break,
                Ok(Some(token)) => token,
                _ => {
                    self.out.malformed = true;
                    lexer.at = lexer.data.len();
                    return;
                }
            };
            match (pending.take(), parse_object(lexer, token, 0, &self.bounds)) {
                (None, Ok(Obj::Name(name))) => pending = Some(name),
                (Some(name), Ok(value)) => match name.as_slice() {
                    b"W" | b"Width" => width = value.int().unwrap_or(0),
                    b"H" | b"Height" => height = value.int().unwrap_or(0),
                    _ => {}
                },
                _ => {
                    self.out.malformed = true;
                    lexer.at = lexer.data.len();
                    return;
                }
            }
        }
        // Skip the sample bytes up to an `EI` that stands alone.
        let data = lexer.data;
        let from = lexer.at + 1;
        let end = (from..data.len().saturating_sub(1)).find(|&at| {
            &data[at..at + 2] == b"EI"
                && data
                    .get(at.wrapping_sub(1))
                    .is_some_and(|b| b.is_ascii_whitespace())
                && data.get(at + 2).is_none_or(|b| b.is_ascii_whitespace())
        });
        match end {
            Some(at) => lexer.at = at + 2,
            None => {
                self.out.malformed = true;
                lexer.at = data.len();
                return;
            }
        }
        self.out.images.push(Image {
            bbox: bounds_of(&self.state.ctm, 0.0, 0.0, 1.0, 1.0),
            width,
            height,
        });
    }
}
