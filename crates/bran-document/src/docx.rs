//! DOCX adapter (issue #21): WordprocessingML to BRAN's flow model and back.
//!
//! Import runs on the shared package intake (`opc`), so package safety is not
//! this module's job. It maps the main document, styles, numbering, notes, and
//! comments into a small typed model and records every normalization or
//! omission in a versioned fidelity map. Nothing is fetched, evaluated, or
//! executed: fields keep their cached result, hyperlinks are recorded, and
//! unsupported content is skipped with a receipt entry. Export writes that
//! model as a deterministic DOCX, so import, export, and import again give the
//! same model and the same citation anchors.

use crate::canonical::{sha256_hex, Json};
use crate::conformance::{Adapter, Anchor, Imported};
use crate::opc::{self, Package, Part, Relationship};
use crate::xml::{self, Event};
use crate::{zip, Cancel, Format, Limits, Refusal};
use bran_core::export::{validate_emitted_string, ExportError};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Version of the canonical model, fidelity vocabulary, and export receipt.
pub const MODEL_VERSION: &str = "1";
const MAIN_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml";
const WML_TYPE: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
/// Nesting of tables and block content controls. Deeper input is refused, so
/// the recursive import, serialization, and export stay far from stack limits
/// whatever XML depth the caller allows.
const MAX_BLOCK_NESTING: usize = 32;
const MAX_JSON_DEPTH: usize = 256;
/// Largest DrawingML extent (ST_PositiveCoordinate) and the 1-inch fallback.
const MAX_EMU: i64 = 27_273_042_316_900;
const DEFAULT_EMU: i64 = 914_400;

/// Fidelity of one Word feature in the model, worst observation wins.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Status {
    Exact,
    Normalized,
    Approximated,
    Unsupported,
}

impl Status {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Normalized => "normalized",
            Self::Approximated => "approximated",
            Self::Unsupported => "unsupported",
        }
    }

    fn parse(code: &str) -> Option<Self> {
        [
            Self::Exact,
            Self::Normalized,
            Self::Approximated,
            Self::Unsupported,
        ]
        .into_iter()
        .find(|status| status.code() == code)
    }
}

/// An imported document: content, the fidelity of every observed feature, and
/// package-level diagnostics (hyperlinks not fetched, DLP findings, ...).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Document {
    pub content: Content,
    pub fidelity: BTreeMap<String, Status>,
    pub diagnostics: BTreeSet<String>,
}

/// The structural model an export writes and a re-import must reproduce.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Content {
    pub blocks: Vec<Block>,
    pub footnotes: Vec<Note>,
    pub endnotes: Vec<Note>,
    pub comments: Vec<Comment>,
    /// Images keyed by the SHA-256 of their bytes.
    pub assets: BTreeMap<String, Asset>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Block {
    Paragraph(Paragraph),
    Table(Vec<Vec<Cell>>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Paragraph {
    pub kind: Kind,
    /// The paragraph ends a body section (a `sectPr` in its properties).
    pub section_break: bool,
    pub inlines: Vec<Inline>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Kind {
    Body,
    Title,
    Caption,
    Heading(u8),
    List { num: u32, level: u8, format: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Cell {
    pub span: u32,
    pub merge: Merge,
    pub blocks: Vec<Block>,
}

/// Vertical merge state of a table cell.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Merge {
    Single,
    Restart,
    Continue,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Inline {
    Run(Run),
    CommentStart(i64),
    CommentEnd(i64),
    BookmarkStart(String),
    BookmarkEnd(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Run {
    pub content: RunContent,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub link: Option<Link>,
    pub change: Option<Change>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunContent {
    Text(String),
    Image(Image),
    FootnoteRef(i64),
    EndnoteRef(i64),
    CommentRef(i64),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Image {
    pub asset: String,
    /// Extent in EMU.
    pub width: i64,
    pub height: i64,
    pub name: String,
    pub description: String,
}

/// A hyperlink target. External targets are recorded, never fetched.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Link {
    External(String),
    Internal(String),
}

/// A tracked change, kept as-is: import neither accepts nor rejects it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Change {
    pub kind: ChangeKind,
    pub id: i64,
    pub author: String,
    pub date: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChangeKind {
    Insert,
    Delete,
    MoveFrom,
    MoveTo,
}

impl ChangeKind {
    const ALL: [Self; 4] = [Self::Insert, Self::Delete, Self::MoveFrom, Self::MoveTo];

    const fn element(self) -> &'static str {
        match self {
            Self::Insert => "ins",
            Self::Delete => "del",
            Self::MoveFrom => "moveFrom",
            Self::MoveTo => "moveTo",
        }
    }

    const fn removes(self) -> bool {
        matches!(self, Self::Delete | Self::MoveFrom)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Note {
    pub id: i64,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Comment {
    pub id: i64,
    pub author: String,
    pub date: String,
    pub initials: String,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Asset {
    pub media_type: String,
    pub data: Vec<u8>,
}

/// A citable unit with a flow locator matching the evidence envelope:
/// `section` plus a 1-based `ordinal` inside it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Citation {
    pub id: String,
    pub role: &'static str,
    pub section: String,
    pub ordinal: u32,
    pub text: String,
}

/// An export: DOCX bytes plus the canonical JSON fidelity receipt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Exported {
    pub bytes: Vec<u8>,
    pub receipt: Vec<u8>,
}

impl Paragraph {
    /// Citable text: inserted and moved-in text, not deleted or moved-away text.
    pub fn text(&self) -> String {
        self.collect(false)
    }

    /// Every run's text, deleted text included.
    pub fn full_text(&self) -> String {
        self.collect(true)
    }

    fn collect(&self, removed: bool) -> String {
        self.inlines
            .iter()
            .filter_map(|inline| match inline {
                Inline::Run(Run {
                    content: RunContent::Text(text),
                    change,
                    ..
                }) if removed || !change.as_ref().is_some_and(|c| c.kind.removes()) => {
                    Some(text.as_str())
                }
                _ => None,
            })
            .collect()
    }
}

impl Block {
    /// Citable text; table cells are separated by tabs and rows by newlines.
    pub fn text(&self) -> String {
        match self {
            Self::Paragraph(paragraph) => paragraph.text(),
            Self::Table(rows) => rows
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|cell| blocks_text(&cell.blocks))
                        .collect::<Vec<_>>()
                        .join("\t")
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

fn blocks_text(blocks: &[Block]) -> String {
    blocks
        .iter()
        .map(Block::text)
        .collect::<Vec<_>>()
        .join("\n")
}

fn each_paragraph<'c>(blocks: &'c [Block], visit: &mut impl FnMut(&'c Paragraph)) {
    for block in blocks {
        match block {
            Block::Paragraph(paragraph) => visit(paragraph),
            Block::Table(rows) => rows
                .iter()
                .flatten()
                .for_each(|cell| each_paragraph(&cell.blocks, visit)),
        }
    }
}

fn each_paragraph_mut(blocks: &mut [Block], visit: &mut impl FnMut(&mut Paragraph)) {
    for block in blocks {
        match block {
            Block::Paragraph(paragraph) => visit(paragraph),
            Block::Table(rows) => rows
                .iter_mut()
                .flatten()
                .for_each(|cell| each_paragraph_mut(&mut cell.blocks, visit)),
        }
    }
}

impl Content {
    fn stories(&self) -> impl Iterator<Item = &[Block]> {
        std::iter::once(self.blocks.as_slice())
            .chain(self.footnotes.iter().map(|note| note.blocks.as_slice()))
            .chain(self.endnotes.iter().map(|note| note.blocks.as_slice()))
            .chain(
                self.comments
                    .iter()
                    .map(|comment| comment.blocks.as_slice()),
            )
    }

    fn paragraphs<'c>(&'c self, visit: &mut impl FnMut(&'c Paragraph)) {
        for story in self.stories() {
            each_paragraph(story, visit);
        }
    }
}

impl Document {
    /// Citable units: body blocks by section and ordinal, then notes and comments.
    pub fn citations(&self) -> Vec<Citation> {
        let mut citations = Vec::new();
        let mut cite = |id: String, role, section: String, ordinal: usize, text: String| {
            if !text.is_empty() {
                citations.push(Citation {
                    id,
                    role,
                    section,
                    ordinal: ordinal as u32,
                    text,
                });
            }
        };
        let (mut section, mut ordinal) = (1, 0);
        for block in &self.content.blocks {
            ordinal += 1;
            let role = match block {
                Block::Table(_) => "table",
                Block::Paragraph(paragraph) => match paragraph.kind {
                    Kind::Heading(_) => "heading",
                    Kind::Title => "title",
                    Kind::List { .. } => "list",
                    Kind::Body | Kind::Caption => "paragraph",
                },
            };
            cite(
                format!("docx:s{section}:b{ordinal}"),
                role,
                format!("s{section}"),
                ordinal,
                block.text(),
            );
            if matches!(block, Block::Paragraph(paragraph) if paragraph.section_break) {
                section += 1;
                ordinal = 0;
            }
        }
        for (kind, notes) in [
            ("footnote", &self.content.footnotes),
            ("endnote", &self.content.endnotes),
        ] {
            for (index, note) in notes.iter().enumerate() {
                cite(
                    format!("docx:{kind}:{}", note.id),
                    "paragraph",
                    format!("{kind}s"),
                    index + 1,
                    blocks_text(&note.blocks),
                );
            }
        }
        for (index, comment) in self.content.comments.iter().enumerate() {
            cite(
                format!("docx:comment:{}", comment.id),
                "paragraph",
                "comments".to_owned(),
                index + 1,
                blocks_text(&comment.blocks),
            );
        }
        citations
    }

    /// Receipt codes: package diagnostics plus `feature:status` per feature.
    pub fn receipt(&self) -> Vec<String> {
        let mut codes = self.diagnostics.clone();
        codes.extend(
            self.fidelity
                .iter()
                .map(|(feature, status)| format!("{feature}:{}", status.code())),
        );
        codes.into_iter().collect()
    }

    pub fn imported(&self) -> Imported {
        Imported {
            canonical: self.canonical(),
            receipt: self.receipt(),
            anchors: self
                .citations()
                .into_iter()
                .map(|citation| Anchor {
                    text_digest: sha256_hex(citation.text.as_bytes()),
                    id: citation.id,
                })
                .collect(),
        }
    }
}

// ---- Import ----

/// Imports an untrusted DOCX. Refusals are typed; nothing partial is returned.
pub fn read(bytes: &[u8], limits: &Limits, cancel: &Cancel) -> Result<Document, Refusal> {
    let package = opc::open(bytes, limits, cancel)?;
    let main = package
        .relationships
        .iter()
        .find(|rel| rel.source.is_empty() && !rel.external && kind_of(rel) == "officeDocument")
        .and_then(|rel| part(&package, &rel.target))
        .ok_or(Refusal::MalformedContainer)?;
    if main.content_type != MAIN_TYPE {
        return Err(Refusal::UnsupportedContainer);
    }
    let mut importer = Importer {
        package: &package,
        limits,
        cancel,
        source: String::new(),
        styles: BTreeMap::new(),
        default_style: None,
        numbers: BTreeMap::new(),
        formats: BTreeMap::new(),
        bookmarks: BTreeMap::new(),
        bookmark_names: BTreeSet::new(),
        fields: Vec::new(),
        document: Document::default(),
        model_bytes: 0,
    };
    importer.survey(&main.name);
    if let Some(styles) = importer.related(&main.name, "styles") {
        importer.read_styles(styles)?;
    }
    if let Some(numbering) = importer.related(&main.name, "numbering") {
        importer.read_numbering(numbering)?;
    }
    importer.document.content.blocks = importer.body(main)?;
    if let Some(notes) = importer.related(&main.name, "footnotes") {
        importer.document.content.footnotes = importer.notes(notes, "footnote")?;
    }
    if let Some(notes) = importer.related(&main.name, "endnotes") {
        importer.document.content.endnotes = importer.notes(notes, "endnote")?;
    }
    if let Some(comments) = importer.related(&main.name, "comments") {
        importer.document.content.comments = importer.comments(comments)?;
    }
    importer.enter("");
    importer.prune();
    let mut document = importer.document;
    document
        .diagnostics
        .extend(package.diagnostics.iter().map(|code| (*code).to_owned()));
    for text in emitted_texts(&document.content) {
        if let Err(refusal) = check_texts(std::slice::from_ref(&text)) {
            document.diagnostics.insert(refusal.code().to_owned());
        }
    }
    Ok(document)
}

/// Canonical dispatch names from expanded XML names, independent of the written
/// prefix. Only WordprocessingML elements use bare local names; foreign names
/// can never enter that dispatch. Default namespaces do not apply to attributes
/// (Namespaces in XML 1.0, sections 6.1–6.3).
fn namespace_name(uri: Option<&str>, local: &str, attribute: bool) -> String {
    let prefix = match uri {
        Some(
            "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
            | "http://purl.oclc.org/ooxml/wordprocessingml/main",
        ) => {
            if attribute {
                "w"
            } else {
                return local.to_owned();
            }
        }
        Some(
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
            | "http://purl.oclc.org/ooxml/officeDocument/relationships",
        ) => "r",
        Some(
            "http://schemas.openxmlformats.org/drawingml/2006/main"
            | "http://purl.oclc.org/ooxml/drawingml/main",
        ) => "a",
        Some(
            "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
            | "http://purl.oclc.org/ooxml/drawingml/wordprocessingDrawing",
        ) => "wp",
        Some(
            "http://schemas.openxmlformats.org/officeDocument/2006/math"
            | "http://purl.oclc.org/ooxml/officeDocument/math",
        ) => "m",
        Some(
            "http://schemas.openxmlformats.org/drawingml/2006/chart"
            | "http://purl.oclc.org/ooxml/drawingml/chart",
        ) => "c",
        Some(
            "http://schemas.openxmlformats.org/drawingml/2006/diagram"
            | "http://purl.oclc.org/ooxml/drawingml/diagram",
        ) => "dgm",
        Some("http://schemas.microsoft.com/office/word/2010/wordprocessingShape") => "wps",
        Some("http://schemas.microsoft.com/office/word/2010/wordprocessingGroup") => "wpg",
        Some("http://schemas.microsoft.com/office/word/2010/wordprocessingCanvas") => "wpc",
        Some(
            "http://schemas.openxmlformats.org/drawingml/2006/lockedCanvas"
            | "http://purl.oclc.org/ooxml/drawingml/lockedCanvas",
        ) => "lc",
        Some("http://schemas.microsoft.com/office/drawing/2016/SVG/main") => "asvg",
        Some("http://schemas.openxmlformats.org/markup-compatibility/2006") => "mc",
        None if attribute => return local.to_owned(),
        // No need to replicate a possibly huge foreign namespace URI into every
        // event: these names are only dispatched to the unsupported handler.
        _ => "?",
    };
    format!("{prefix}:{local}")
}

fn namespace_uri(result: quick_xml::name::ResolveResult<'_>) -> Result<Option<&str>, Refusal> {
    use quick_xml::name::ResolveResult;
    match result {
        ResolveResult::Bound(uri) => Ok(Some(uri.into_inner())),
        ResolveResult::Unbound => Ok(None),
        ResolveResult::Unknown(_) => Err(Refusal::MalformedXml),
    }
}

/// The shared reader already validated XML and its budgets. Revisit only the
/// opening tags with the installed parser's scoped namespace resolver; keep the
/// shared event representation and all other adapters' parsing unchanged.
fn resolve_names(bytes: &[u8], events: &mut [Event]) -> Result<(), Refusal> {
    use quick_xml::events::Event as Q;
    let mut reader = quick_xml::reader::NsReader::from_reader(bytes);
    let mut opens = events.iter_mut().filter_map(|event| match event {
        Event::Open { name, attributes } => Some((name, attributes)),
        _ => None,
    });
    loop {
        match reader.read_event().map_err(|_| Refusal::MalformedXml)? {
            Q::Start(element) | Q::Empty(element) => {
                let (name, attributes) = opens.next().ok_or(Refusal::MalformedXml)?;
                let (uri, local) = reader.resolver().resolve_element(element.name());
                *name = namespace_name(namespace_uri(uri)?, local.as_ref(), false);
                let mut seen = BTreeSet::new();
                for (raw, (key, _)) in element.attributes().zip(attributes.iter_mut()) {
                    let raw = raw.map_err(|_| Refusal::MalformedXml)?;
                    if key == "xmlns" || key.starts_with("xmlns:") {
                        continue;
                    }
                    let (uri, local) = reader.resolver().resolve_attribute(raw.key);
                    let uri = namespace_uri(uri)?;
                    if !seen.insert((uri, local.into_inner())) {
                        return Err(Refusal::MalformedXml);
                    }
                    *key = namespace_name(uri, local.as_ref(), true);
                }
            }
            Q::Eof => return Ok(()),
            _ => {}
        }
    }
}

fn part<'p>(package: &'p Package, name: &str) -> Option<&'p Part> {
    package
        .parts
        .iter()
        .find(|part| part.name.eq_ignore_ascii_case(name))
}

fn kind_of(rel: &Relationship) -> &str {
    rel.kind.rsplit('/').next().unwrap_or_default()
}

fn attr<'e>(attributes: &'e [(String, String)], key: &str) -> Option<&'e str> {
    attributes
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

fn number<T: std::str::FromStr>(value: Option<&str>) -> Option<T> {
    value.and_then(|value| value.parse().ok())
}

/// OOXML on/off value: absent means on.
fn on(value: Option<&str>) -> bool {
    !matches!(value, Some("0" | "false" | "off" | "none"))
}

/// Walks a flat event list. Each method that takes an element expects its
/// `Open` event to have been consumed and consumes through its `Close`.
struct Cursor<'e> {
    events: &'e [Event],
    at: usize,
}

type Element<'e> = (usize, &'e str, &'e [(String, String)]);

impl<'e> Cursor<'e> {
    fn next(&mut self) -> Result<&'e Event, Refusal> {
        let event = self.events.get(self.at).ok_or(Refusal::MalformedXml)?;
        self.at += 1;
        Ok(event)
    }

    fn skip(&mut self) -> Result<(), Refusal> {
        self.elements().map(|_| ())
    }

    /// Text directly inside the element, nested elements skipped.
    fn text(&mut self) -> Result<String, Refusal> {
        let mut text = String::new();
        loop {
            match self.next()? {
                Event::Close => return Ok(text),
                Event::Text(value) => text.push_str(value),
                Event::Open { .. } => self.skip()?,
            }
        }
    }

    /// Every descendant element with its depth (1 = child).
    fn elements(&mut self) -> Result<Vec<Element<'e>>, Refusal> {
        let (mut depth, mut found) = (1, Vec::new());
        while depth > 0 {
            match self.next()? {
                Event::Open { name, attributes } => {
                    found.push((depth, name.as_str(), attributes.as_slice()));
                    depth += 1;
                }
                Event::Close => depth -= 1,
                Event::Text(_) => {}
            }
        }
        Ok(found)
    }

    /// Skips whitespace and returns the next element name without consuming it.
    fn peek_open(&mut self) -> Option<&'e str> {
        while let Some(Event::Text(_)) = self.events.get(self.at) {
            self.at += 1;
        }
        match self.events.get(self.at) {
            Some(Event::Open { name, .. }) => Some(name),
            _ => None,
        }
    }
}

#[derive(Default)]
struct Style {
    name: String,
    based_on: Option<String>,
    outline: Option<u8>,
    num: Option<u32>,
    level: Option<u8>,
}

#[derive(Default)]
struct ParagraphProps {
    style: Option<String>,
    num: Option<u32>,
    level: Option<u8>,
    outline: Option<u8>,
    section_break: bool,
}

enum Frame {
    Plain,
    Link(Option<Link>),
    Change(Change),
}

struct Importer<'a> {
    package: &'a Package,
    limits: &'a Limits,
    cancel: &'a Cancel,
    /// Part being read, for relationship lookups.
    source: String,
    styles: BTreeMap<String, Style>,
    default_style: Option<String>,
    numbers: BTreeMap<u32, u32>,
    formats: BTreeMap<(u32, u8), String>,
    /// Open bookmarks of the current part by `w:id`.
    bookmarks: BTreeMap<String, String>,
    bookmark_names: BTreeSet<String>,
    /// Complex field stack of the current part; `true` while in the field code.
    fields: Vec<bool>,
    document: Document,
    /// Cumulative run payload and metadata copies, independent of machine layout.
    model_bytes: u64,
}

impl<'a> Importer<'a> {
    fn reserve_model(&mut self, bytes: usize) -> Result<(), Refusal> {
        self.model_bytes = self
            .model_bytes
            .checked_add(bytes as u64)
            .filter(|bytes| *bytes <= self.limits.max_total_bytes)
            .ok_or(Refusal::Oversized)?;
        Ok(())
    }

    fn emit(
        &mut self,
        content: RunContent,
        template: &Run,
        inlines: &mut Vec<Inline>,
    ) -> Result<(), Refusal> {
        let payload = match &content {
            RunContent::Text(text) => text.len(),
            RunContent::Image(image) => {
                image.asset.len() + image.name.len() + image.description.len()
            }
            _ => 0,
        };
        self.reserve_model(payload + metadata_bytes(template))?;
        inlines.push(Inline::Run(Run {
            content,
            ..template.clone()
        }));
        Ok(())
    }

    fn note(&mut self, feature: &str, status: Status) {
        let entry = self
            .document
            .fidelity
            .entry(feature.to_owned())
            .or_insert(status);
        *entry = (*entry).max(status);
    }

    /// Drops characters XML 1.0 cannot carry (and CR), recording the change.
    fn clean(&mut self, value: &str) -> String {
        let kept = |c: &char| match c {
            '\t' | '\n' => true,
            '\u{0}'..='\u{1f}' | '\u{fffe}' | '\u{ffff}' => false,
            _ => true,
        };
        let cleaned: String = value.chars().filter(kept).collect();
        if cleaned.len() != value.len() {
            self.note("text", Status::Normalized);
        }
        cleaned
    }

    fn enter(&mut self, source: &str) {
        if !self.bookmarks.is_empty() {
            self.note("bookmarks", Status::Normalized);
        }
        if !self.fields.is_empty() {
            // An unterminated field hid everything after its code.
            self.note("fields", Status::Approximated);
        }
        self.bookmarks.clear();
        self.fields.clear();
        self.source = source.to_owned();
    }

    fn related(&self, source: &str, kind: &str) -> Option<&'a Part> {
        let package = self.package;
        package
            .relationships
            .iter()
            .find(|rel| {
                rel.source.eq_ignore_ascii_case(source) && !rel.external && kind_of(rel) == kind
            })
            .and_then(|rel| part(package, &rel.target))
    }

    fn relationship(&self, id: &str) -> Option<&'a Relationship> {
        let package = self.package;
        package
            .relationships
            .iter()
            .find(|rel| rel.source.eq_ignore_ascii_case(&self.source) && rel.id == id)
    }

    fn parse(&self, part: &Part, root: &str) -> Result<Vec<Event>, Refusal> {
        self.cancel.check()?;
        let mut events = xml::parse(&part.data, self.limits, self.cancel)?;
        resolve_names(&part.data, &mut events)?;
        if !matches!(events.first(), Some(Event::Open { name, .. }) if name == root)
            || events.iter().any(|event| {
                matches!(event,
                Event::Open { name, .. } if name == "mc:AlternateContent")
            })
        {
            return Err(Refusal::UnsupportedContainer);
        }
        Ok(events)
    }

    /// Records package and main-part relationships this model does not carry.
    fn survey(&mut self, main: &str) {
        let package = self.package;
        for rel in &package.relationships {
            let feature = if rel.source.is_empty() {
                match kind_of(rel) {
                    "officeDocument" => None,
                    "core-properties"
                    | "extended-properties"
                    | "custom-properties"
                    | "thumbnail" => Some("document-properties"),
                    "origin" | "signature" => Some("digital-signatures"),
                    _ => Some("unrecognized-parts"),
                }
            } else if rel.source.eq_ignore_ascii_case(main) {
                match kind_of(rel) {
                    // Read below, reached from content, or formatting only.
                    "styles" | "numbering" | "footnotes" | "endnotes" | "comments"
                    | "hyperlink" | "image" | "chart" | "diagramData" | "diagramLayout"
                    | "diagramQuickStyle" | "diagramColors" | "diagramDrawing" | "settings"
                    | "webSettings" | "fontTable" | "theme" | "stylesWithEffects" => None,
                    "header" | "footer" => Some("headers-footers"),
                    "commentsExtended" | "commentsIds" | "commentsExtensible" | "people" => {
                        Some("comment-threads")
                    }
                    _ => Some("unrecognized-parts"),
                }
            } else {
                None
            };
            if let Some(feature) = feature {
                self.note(feature, Status::Unsupported);
            }
        }
    }

    fn read_styles(&mut self, part: &Part) -> Result<(), Refusal> {
        self.note("styles", Status::Normalized);
        let events = self.parse(part, "styles")?;
        let mut cursor = Cursor {
            events: &events,
            at: 1,
        };
        let mut path: Vec<&str> = Vec::new();
        let mut current: Option<(String, Style)> = None;
        for (depth, name, attributes) in cursor.elements()? {
            path.truncate(depth - 1);
            path.push(name);
            let val = attr(attributes, "w:val");
            if depth == 1 {
                if let Some((id, style)) = current.take() {
                    self.styles.entry(id).or_insert(style);
                }
                if name == "style" && attr(attributes, "w:type") == Some("paragraph") {
                    if let Some(id) = attr(attributes, "w:styleId") {
                        if matches!(attr(attributes, "w:default"), Some("1" | "true" | "on")) {
                            self.default_style.get_or_insert_with(|| id.to_owned());
                        }
                        current = Some((id.to_owned(), Style::default()));
                    }
                }
                continue;
            }
            let Some((_, style)) = current.as_mut() else {
                continue;
            };
            match &path[1..] {
                ["name"] => style.name = val.unwrap_or_default().to_ascii_lowercase(),
                ["basedOn"] => style.based_on = val.map(str::to_owned),
                ["pPr", "outlineLvl"] => style.outline = number(val),
                ["pPr", "numPr", "numId"] => style.num = number(val),
                ["pPr", "numPr", "ilvl"] => style.level = number(val),
                _ => {}
            }
        }
        if let Some((id, style)) = current {
            self.styles.entry(id).or_insert(style);
        }
        Ok(())
    }

    fn read_numbering(&mut self, part: &Part) -> Result<(), Refusal> {
        let events = self.parse(part, "numbering")?;
        let mut cursor = Cursor {
            events: &events,
            at: 1,
        };
        let mut path: Vec<&str> = Vec::new();
        let (mut abstract_id, mut level, mut num) = (None, None, None);
        for (depth, name, attributes) in cursor.elements()? {
            path.truncate(depth - 1);
            path.push(name);
            match path.as_slice() {
                ["abstractNum"] => abstract_id = number(attr(attributes, "w:abstractNumId")),
                ["abstractNum", "lvl"] => level = number(attr(attributes, "w:ilvl")),
                ["abstractNum", "lvl", "numFmt"] => {
                    if let (Some(abstract_id), Some(level), Some(format)) =
                        (abstract_id, level, attr(attributes, "w:val"))
                    {
                        self.formats
                            .entry((abstract_id, level))
                            .or_insert_with(|| format.to_owned());
                    }
                }
                ["num"] => num = number(attr(attributes, "w:numId")),
                ["num", "abstractNumId"] => {
                    if let (Some(num), Some(abstract_id)) = (num, number(attr(attributes, "w:val")))
                    {
                        self.numbers.entry(num).or_insert(abstract_id);
                    }
                }
                ["num", "lvlOverride", ..] => self.note("lists", Status::Normalized),
                _ => {}
            }
        }
        Ok(())
    }

    fn body(&mut self, part: &Part) -> Result<Vec<Block>, Refusal> {
        self.enter(&part.name);
        let events = self.parse(part, "document")?;
        let mut cursor = Cursor {
            events: &events,
            at: 1,
        };
        loop {
            match cursor.next()? {
                Event::Open { name, .. } if name == "body" => {
                    return self.blocks(&mut cursor, 0, true);
                }
                Event::Open { .. } => cursor.skip()?,
                Event::Text(_) => {}
                Event::Close => return Err(Refusal::MalformedContainer),
            }
        }
    }

    fn notes(&mut self, part: &Part, item: &str) -> Result<Vec<Note>, Refusal> {
        let feature = if item == "footnote" {
            "footnotes"
        } else {
            "endnotes"
        };
        self.enter(&part.name);
        let events = self.parse(part, &format!("{item}s"))?;
        let mut cursor = Cursor {
            events: &events,
            at: 1,
        };
        let (mut notes, mut ids) = (Vec::new(), BTreeSet::new());
        loop {
            let attributes = match cursor.next()? {
                Event::Close => return Ok(notes),
                Event::Text(_) => continue,
                Event::Open { name, attributes } if name == item => attributes,
                Event::Open { .. } => {
                    cursor.skip()?;
                    continue;
                }
            };
            // Separator notes are layout, not content.
            if attr(attributes, "w:type").is_some_and(|kind| kind != "normal") {
                cursor.skip()?;
                continue;
            }
            self.note(feature, Status::Normalized);
            match number::<i64>(attr(attributes, "w:id")) {
                Some(id) if ids.insert(id) => {
                    let blocks = self.story(&mut cursor, 0, feature)?;
                    notes.push(Note { id, blocks });
                }
                _ => cursor.skip()?,
            }
        }
    }

    fn comments(&mut self, part: &Part) -> Result<Vec<Comment>, Refusal> {
        self.enter(&part.name);
        let events = self.parse(part, "comments")?;
        let mut cursor = Cursor {
            events: &events,
            at: 1,
        };
        let (mut comments, mut ids) = (Vec::new(), BTreeSet::new());
        loop {
            let attributes = match cursor.next()? {
                Event::Close => return Ok(comments),
                Event::Text(_) => continue,
                Event::Open { name, attributes } if name == "comment" => attributes,
                Event::Open { .. } => {
                    cursor.skip()?;
                    continue;
                }
            };
            self.note("comments", Status::Normalized);
            match number::<i64>(attr(attributes, "w:id")) {
                Some(id) if ids.insert(id) => {
                    let author = self.clean(attr(attributes, "w:author").unwrap_or_default());
                    let date = self.clean(attr(attributes, "w:date").unwrap_or_default());
                    let initials = self.clean(attr(attributes, "w:initials").unwrap_or_default());
                    let blocks = self.story(&mut cursor, 0, "comments")?;
                    comments.push(Comment {
                        id,
                        author,
                        date,
                        initials,
                        blocks,
                    });
                }
                _ => cursor.skip()?,
            }
        }
    }

    /// Blocks of a cell, note, or comment. Word needs a trailing paragraph, so
    /// one is added (and recorded) when missing, keeping export round-trips exact.
    fn story(
        &mut self,
        cursor: &mut Cursor<'_>,
        depth: usize,
        feature: &str,
    ) -> Result<Vec<Block>, Refusal> {
        let mut blocks = self.blocks(cursor, depth, false)?;
        if !matches!(blocks.last(), Some(Block::Paragraph(_))) {
            self.note(feature, Status::Normalized);
            blocks.push(Block::Paragraph(Paragraph {
                kind: Kind::Body,
                section_break: false,
                inlines: Vec::new(),
            }));
        }
        Ok(blocks)
    }

    fn blocks(
        &mut self,
        cursor: &mut Cursor<'_>,
        depth: usize,
        body: bool,
    ) -> Result<Vec<Block>, Refusal> {
        if depth > MAX_BLOCK_NESTING {
            return Err(Refusal::XmlDepthLimit);
        }
        self.cancel.check()?;
        let mut blocks = Vec::new();
        loop {
            let name = match cursor.next()? {
                Event::Close => return Ok(blocks),
                Event::Text(_) => continue,
                Event::Open { name, .. } => name.as_str(),
            };
            match name {
                "p" => blocks.push(Block::Paragraph(self.paragraph(cursor, body)?)),
                "tbl" => {
                    if let Some(table) = self.table(cursor, depth + 1)? {
                        blocks.push(table);
                    }
                }
                "sdt" => {
                    self.note("content-controls", Status::Normalized);
                    blocks.extend(self.blocks(cursor, depth + 1, body)?);
                }
                "sdtContent" | "customXml" => {
                    blocks.extend(self.blocks(cursor, depth + 1, body)?);
                }
                "sectPr" => {
                    self.note("sections", Status::Normalized);
                    cursor.skip()?;
                }
                "bookmarkStart" | "bookmarkEnd" => {
                    self.note("bookmarks", Status::Normalized);
                    cursor.skip()?;
                }
                "commentRangeStart" | "commentRangeEnd" => {
                    self.note("comments", Status::Normalized);
                    cursor.skip()?;
                }
                "tcPr" => {
                    self.note("tables", Status::Normalized);
                    cursor.skip()?;
                }
                "m:oMath" | "m:oMathPara" => self.unsupported(cursor, "math")?,
                "altChunk" => self.unsupported(cursor, "embedded-documents")?,
                "sdtPr" | "sdtEndPr" | "customXmlPr" | "proofErr" | "permStart" | "permEnd" => {
                    cursor.skip()?
                }
                _ => self.unsupported(cursor, "unrecognized-content")?,
            }
        }
    }

    fn unsupported(&mut self, cursor: &mut Cursor<'_>, feature: &str) -> Result<(), Refusal> {
        self.note(feature, Status::Unsupported);
        cursor.skip()
    }

    fn table(&mut self, cursor: &mut Cursor<'_>, depth: usize) -> Result<Option<Block>, Refusal> {
        if depth > MAX_BLOCK_NESTING {
            return Err(Refusal::XmlDepthLimit);
        }
        self.note("tables", Status::Normalized);
        let mut rows = Vec::new();
        loop {
            let name = match cursor.next()? {
                Event::Close => break,
                Event::Text(_) => continue,
                Event::Open { name, .. } => name.as_str(),
            };
            match name {
                "tr" => {
                    let row = self.row(cursor, depth)?;
                    if !row.is_empty() {
                        rows.push(row);
                    }
                }
                "tblPr" | "tblGrid" => cursor.skip()?,
                _ => self.unsupported(cursor, "unrecognized-content")?,
            }
        }
        // Word cannot open a table without rows, so an empty one is dropped.
        Ok((!rows.is_empty()).then_some(Block::Table(rows)))
    }

    fn row(&mut self, cursor: &mut Cursor<'_>, depth: usize) -> Result<Vec<Cell>, Refusal> {
        let mut cells = Vec::new();
        loop {
            let name = match cursor.next()? {
                Event::Close => return Ok(cells),
                Event::Text(_) => continue,
                Event::Open { name, .. } => name.as_str(),
            };
            match name {
                "tc" => cells.push(self.cell(cursor, depth)?),
                "trPr" => {
                    for (_, element, _) in cursor.elements()? {
                        if matches!(element, "ins" | "del") {
                            self.note("table-row-changes", Status::Unsupported);
                        }
                    }
                }
                "tblPrEx" => cursor.skip()?,
                _ => self.unsupported(cursor, "unrecognized-content")?,
            }
        }
    }

    fn cell(&mut self, cursor: &mut Cursor<'_>, depth: usize) -> Result<Cell, Refusal> {
        let (mut span, mut merge) = (1, Merge::Single);
        if cursor.peek_open() == Some("tcPr") {
            cursor.next()?;
            for (level, name, attributes) in cursor.elements()? {
                match (level, name) {
                    (1, "gridSpan") => {
                        span = number(attr(attributes, "w:val"))
                            .filter(|span| (1..=63).contains(span))
                            .unwrap_or(1);
                    }
                    (1, "vMerge") => {
                        merge = if attr(attributes, "w:val") == Some("restart") {
                            Merge::Restart
                        } else {
                            Merge::Continue
                        };
                    }
                    (1, "cellIns" | "cellDel" | "cellMerge" | "tcPrChange") => {
                        self.note("table-cell-changes", Status::Unsupported);
                    }
                    _ => {}
                }
            }
        }
        let blocks = self.story(cursor, depth, "tables")?;
        Ok(Cell {
            span,
            merge,
            blocks,
        })
    }

    fn paragraph(&mut self, cursor: &mut Cursor<'_>, body: bool) -> Result<Paragraph, Refusal> {
        let mut inlines = Vec::new();
        let mut props = ParagraphProps::default();
        let mut frames: Vec<Frame> = Vec::new();
        loop {
            let (name, attributes) = match cursor.next()? {
                Event::Close => match frames.pop() {
                    None => break,
                    Some(_) => continue,
                },
                Event::Text(_) => continue,
                Event::Open { name, attributes } => (name.as_str(), attributes.as_slice()),
            };
            match name {
                "pPr" => props = self.paragraph_properties(cursor)?,
                "r" => self.run(cursor, &frames, &mut inlines)?,
                "hyperlink" => frames.push(Frame::Link(self.link(attributes))),
                "ins" | "del" | "moveFrom" | "moveTo" => {
                    let nested = frames.iter().any(|frame| matches!(frame, Frame::Change(_)));
                    self.note(
                        "tracked-changes",
                        if nested {
                            Status::Approximated
                        } else {
                            Status::Exact
                        },
                    );
                    frames.push(Frame::Change(self.change(name, attributes)));
                }
                "fldSimple" => {
                    self.note("fields", Status::Normalized);
                    frames.push(Frame::Plain);
                }
                "sdt" => {
                    self.note("content-controls", Status::Normalized);
                    frames.push(Frame::Plain);
                }
                "sdtContent" | "smartTag" | "customXml" | "dir" | "bdo" => {
                    frames.push(Frame::Plain)
                }
                "bookmarkStart" => {
                    match (attr(attributes, "w:id"), attr(attributes, "w:name")) {
                        (Some(id), Some(name)) if self.bookmark_names.insert(name.to_owned()) => {
                            let name = self.clean(name);
                            self.note("bookmarks", Status::Exact);
                            self.bookmarks.insert(id.to_owned(), name.clone());
                            inlines.push(Inline::BookmarkStart(name));
                        }
                        _ => self.note("bookmarks", Status::Normalized),
                    }
                    cursor.skip()?;
                }
                "bookmarkEnd" => {
                    match attr(attributes, "w:id").and_then(|id| self.bookmarks.remove(id)) {
                        Some(name) => inlines.push(Inline::BookmarkEnd(name)),
                        None => self.note("bookmarks", Status::Normalized),
                    }
                    cursor.skip()?;
                }
                "commentRangeStart" | "commentRangeEnd" => {
                    if let Some(id) = number(attr(attributes, "w:id")) {
                        inlines.push(if name == "commentRangeStart" {
                            Inline::CommentStart(id)
                        } else {
                            Inline::CommentEnd(id)
                        });
                    }
                    cursor.skip()?;
                }
                "m:oMath" | "m:oMathPara" => self.unsupported(cursor, "math")?,
                "sdtPr" | "sdtEndPr" | "smartTagPr" | "customXmlPr" | "proofErr" | "permStart"
                | "permEnd" | "moveFromRangeStart" | "moveFromRangeEnd" | "moveToRangeStart"
                | "moveToRangeEnd" => cursor.skip()?,
                _ => self.unsupported(cursor, "unrecognized-content")?,
            }
        }
        Ok(Paragraph {
            kind: self.kind(&props),
            section_break: body && props.section_break,
            inlines,
        })
    }

    fn paragraph_properties(&mut self, cursor: &mut Cursor<'_>) -> Result<ParagraphProps, Refusal> {
        let mut props = ParagraphProps::default();
        let mut parent = "";
        for (depth, name, attributes) in cursor.elements()? {
            let val = attr(attributes, "w:val");
            if depth == 1 {
                parent = name;
            }
            match (depth, parent, name) {
                (1, _, "pStyle") => props.style = val.map(str::to_owned),
                (1, _, "outlineLvl") => props.outline = number(val),
                (1, _, "sectPr") => {
                    self.note("sections", Status::Normalized);
                    props.section_break = true;
                }
                (1, _, "numPr" | "rPr") => {}
                (1, _, "pPrChange") => self.note("formatting-changes", Status::Unsupported),
                (1, _, _) => self.note("paragraph-formatting", Status::Normalized),
                (2, "numPr", "numId") => props.num = number(val),
                (2, "numPr", "ilvl") => props.level = number(val),
                (2, "rPr", "ins" | "del" | "moveFrom" | "moveTo") => {
                    self.note("paragraph-mark-changes", Status::Unsupported);
                }
                (2, "rPr", "rPrChange") => self.note("formatting-changes", Status::Unsupported),
                _ => {}
            }
        }
        Ok(props)
    }

    fn kind(&mut self, props: &ParagraphProps) -> Kind {
        let (mut outline, mut num, mut level) = (props.outline, props.num, props.level);
        let mut named = None;
        let mut next = props.style.clone().or_else(|| self.default_style.clone());
        let mut seen = BTreeSet::new();
        while let Some(id) = next {
            let Some(style) = self.styles.get(&id) else {
                break;
            };
            if !seen.insert(id) || seen.len() > MAX_BLOCK_NESTING {
                break;
            }
            outline = outline.or(style.outline);
            num = num.or(style.num);
            level = level.or(style.level);
            named = named.or_else(|| named_kind(&style.name));
            next = style.based_on.clone();
        }
        let num = num.filter(|num| *num != 0);
        let heading = match (outline.filter(|level| *level < 9), &named) {
            (Some(level), _) => Some(level + 1),
            (None, Some(Kind::Heading(level))) => Some(*level),
            _ => None,
        };
        if let Some(level) = heading {
            self.note("headings", Status::Normalized);
            if num.is_some() {
                // Heading numbering is not carried; the heading level is.
                self.note("lists", Status::Normalized);
            }
            return Kind::Heading(level);
        }
        match named {
            Some(Kind::Title) => {
                self.note("headings", Status::Normalized);
                return Kind::Title;
            }
            Some(Kind::Caption) => {
                self.note("captions", Status::Normalized);
                return Kind::Caption;
            }
            _ => {}
        }
        let Some(num) = num else {
            return Kind::Body;
        };
        self.note("lists", Status::Normalized);
        let Some(abstract_id) = self.numbers.get(&num).copied() else {
            return Kind::Body;
        };
        let level = level.unwrap_or(0).min(8);
        let format = self
            .formats
            .get(&(abstract_id, level))
            .filter(|format| valid_format(format))
            .cloned()
            .unwrap_or_else(|| "decimal".to_owned());
        Kind::List { num, level, format }
    }

    fn link(&mut self, attributes: &[(String, String)]) -> Option<Link> {
        let anchor = attr(attributes, "w:anchor");
        let link = match attr(attributes, "r:id") {
            Some(id) => match self.relationship(id) {
                Some(rel) if rel.external && kind_of(rel) == "hyperlink" => {
                    let target = match anchor {
                        Some(anchor) => format!("{}#{anchor}", rel.target),
                        None => rel.target.clone(),
                    };
                    Some(Link::External(self.clean(&target)))
                }
                _ => None,
            },
            None => anchor.map(|anchor| Link::Internal(self.clean(anchor))),
        };
        self.note(
            "hyperlinks",
            if link.is_some() {
                Status::Exact
            } else {
                Status::Normalized
            },
        );
        link
    }

    fn change(&mut self, name: &str, attributes: &[(String, String)]) -> Change {
        Change {
            kind: ChangeKind::ALL
                .into_iter()
                .find(|kind| kind.element() == name)
                .unwrap_or(ChangeKind::Insert),
            id: number(attr(attributes, "w:id")).unwrap_or(0),
            author: self.clean(attr(attributes, "w:author").unwrap_or_default()),
            date: self.clean(attr(attributes, "w:date").unwrap_or_default()),
        }
    }

    fn run(
        &mut self,
        cursor: &mut Cursor<'_>,
        frames: &[Frame],
        inlines: &mut Vec<Inline>,
    ) -> Result<(), Refusal> {
        let link = frames
            .iter()
            .rev()
            .find_map(|frame| match frame {
                Frame::Link(link) => Some(link.as_ref()),
                _ => None,
            })
            .flatten();
        let change = frames.iter().rev().find_map(|frame| match frame {
            Frame::Change(change) => Some(change),
            _ => None,
        });
        self.reserve_model(link_bytes(link) + change_bytes(change))?;
        let mut template = Run {
            content: RunContent::Text(String::new()),
            bold: false,
            italic: false,
            underline: false,
            link: link.cloned(),
            change: change.cloned(),
        };
        let mut text = String::new();
        loop {
            let (name, attributes) = match cursor.next()? {
                Event::Close => break,
                Event::Text(_) => continue,
                Event::Open { name, attributes } => (name.as_str(), attributes.as_slice()),
            };
            // Content between a field's begin and separate is its code.
            let hidden = self.fields.iter().any(|code| *code);
            match name {
                "rPr" => {
                    for (depth, element, attributes) in cursor.elements()? {
                        let val = attr(attributes, "w:val");
                        match (depth, element) {
                            (1, "b") => template.bold = on(val),
                            (1, "i") => template.italic = on(val),
                            (1, "u") => template.underline = on(val),
                            (1, "rPrChange") => {
                                self.note("formatting-changes", Status::Unsupported);
                            }
                            (1, _) => self.note("run-formatting", Status::Normalized),
                            _ => {}
                        }
                    }
                }
                "t" | "delText" => {
                    let value = cursor.text()?;
                    if !hidden {
                        text.push_str(&self.clean(&value));
                    }
                }
                "tab" | "ptab" | "br" | "cr" | "noBreakHyphen" | "softHyphen" => {
                    if name == "ptab"
                        || attr(attributes, "w:type").is_some_and(|t| t != "textWrapping")
                    {
                        self.note("breaks", Status::Normalized);
                    }
                    if !hidden {
                        text.push(match name {
                            "tab" | "ptab" => '\t',
                            "noBreakHyphen" => '\u{2011}',
                            "softHyphen" => '\u{ad}',
                            _ => '\n',
                        });
                    }
                    cursor.skip()?;
                }
                "fldChar" => {
                    self.note("fields", Status::Normalized);
                    match attr(attributes, "w:fldCharType") {
                        Some("begin") => self.fields.push(true),
                        Some("separate") => {
                            if let Some(code) = self.fields.last_mut() {
                                *code = false;
                            }
                        }
                        Some("end") => {
                            self.fields.pop();
                        }
                        _ => {}
                    }
                    cursor.skip()?;
                }
                "drawing" => {
                    let image = self.drawing(cursor)?;
                    if let (Some(image), false) = (image, hidden) {
                        if !text.is_empty() {
                            self.emit(
                                RunContent::Text(std::mem::take(&mut text)),
                                &template,
                                inlines,
                            )?;
                        }
                        self.emit(RunContent::Image(image), &template, inlines)?;
                    }
                }
                "footnoteReference" | "endnoteReference" | "commentReference" => {
                    if let (Some(id), false) = (number(attr(attributes, "w:id")), hidden) {
                        if !text.is_empty() {
                            self.emit(
                                RunContent::Text(std::mem::take(&mut text)),
                                &template,
                                inlines,
                            )?;
                        }
                        let content = match name {
                            "footnoteReference" => RunContent::FootnoteRef(id),
                            "endnoteReference" => RunContent::EndnoteRef(id),
                            _ => RunContent::CommentRef(id),
                        };
                        self.emit(content, &template, inlines)?;
                    }
                    cursor.skip()?;
                }
                "pict" => self.unsupported(cursor, "vml")?,
                "object" => self.unsupported(cursor, "embedded-objects")?,
                "sym" => self.unsupported(cursor, "symbols")?,
                "m:oMath" | "m:oMathPara" => self.unsupported(cursor, "math")?,
                "instrText"
                | "delInstrText"
                | "footnoteRef"
                | "endnoteRef"
                | "annotationRef"
                | "lastRenderedPageBreak"
                | "separator"
                | "continuationSeparator" => cursor.skip()?,
                _ => self.unsupported(cursor, "unrecognized-content")?,
            }
        }
        if !text.is_empty() {
            self.emit(RunContent::Text(text), &template, inlines)?;
        }
        Ok(())
    }

    fn drawing(&mut self, cursor: &mut Cursor<'_>) -> Result<Option<Image>, Refusal> {
        let (mut floating, mut blip, mut other) = (false, None, None);
        let (mut width, mut height, mut name, mut description) = (None, None, "", "");
        for (depth, element, attributes) in cursor.elements()? {
            match (depth, element) {
                (1, "wp:anchor") => floating = true,
                (2, "wp:extent") => {
                    width = number::<i64>(attr(attributes, "cx"));
                    height = number::<i64>(attr(attributes, "cy"));
                }
                (2, "wp:docPr") => {
                    name = attr(attributes, "name").unwrap_or_default();
                    description = attr(attributes, "descr").unwrap_or_default();
                }
                (_, "a:blip") => blip = blip.or(attr(attributes, "r:embed")),
                (_, "asvg:svgBlip") => self.note("svg-images", Status::Normalized),
                (_, "c:chart") => other = Some("charts"),
                (_, "dgm:relIds") => other = Some("smartart"),
                (_, "wps:wsp" | "wpg:wgp" | "wpc:wpc" | "wps:txbx" | "lc:lockedCanvas") => {
                    other = Some("shapes")
                }
                _ => {}
            }
        }
        if let Some(feature) = other {
            self.note(feature, Status::Unsupported);
            return Ok(None);
        }
        let Some(blip) = blip else {
            self.note("unrecognized-content", Status::Unsupported);
            return Ok(None);
        };
        let target = self
            .relationship(blip)
            .filter(|rel| !rel.external)
            .and_then(|rel| part(self.package, &rel.target));
        let Some(target) = target else {
            self.note("images", Status::Unsupported);
            return Ok(None);
        };
        if floating {
            self.note("floating-images", Status::Normalized);
        }
        let extent = |value: Option<i64>| value.filter(|value| (1..=MAX_EMU).contains(value));
        let (width, height) = match (extent(width), extent(height)) {
            (Some(width), Some(height)) => {
                self.note("images", Status::Exact);
                (width, height)
            }
            _ => {
                self.note("images", Status::Normalized);
                (DEFAULT_EMU, DEFAULT_EMU)
            }
        };
        let asset = sha256_hex(&target.data);
        self.document
            .content
            .assets
            .entry(asset.clone())
            .or_insert_with(|| Asset {
                media_type: target.content_type.clone(),
                data: target.data.clone(),
            });
        Ok(Some(Image {
            asset,
            width,
            height,
            name: self.clean(name),
            description: self.clean(description),
        }))
    }

    /// Drops references to notes and comments that do not exist, so an export
    /// never points at a missing part entry.
    fn prune(&mut self) {
        let ids = |notes: &[Note]| notes.iter().map(|note| note.id).collect::<BTreeSet<_>>();
        let content = &mut self.document.content;
        let (footnotes, endnotes) = (ids(&content.footnotes), ids(&content.endnotes));
        let comments: BTreeSet<i64> = content.comments.iter().map(|comment| comment.id).collect();
        let mut dropped = false;
        let mut keep = |paragraph: &mut Paragraph| {
            let before = paragraph.inlines.len();
            paragraph.inlines.retain(|inline| match inline {
                Inline::Run(Run {
                    content: RunContent::FootnoteRef(id),
                    ..
                }) => footnotes.contains(id),
                Inline::Run(Run {
                    content: RunContent::EndnoteRef(id),
                    ..
                }) => endnotes.contains(id),
                Inline::Run(Run {
                    content: RunContent::CommentRef(id),
                    ..
                })
                | Inline::CommentStart(id)
                | Inline::CommentEnd(id) => comments.contains(id),
                _ => true,
            });
            dropped |= paragraph.inlines.len() != before;
        };
        each_paragraph_mut(&mut content.blocks, &mut keep);
        for note in content.footnotes.iter_mut().chain(&mut content.endnotes) {
            each_paragraph_mut(&mut note.blocks, &mut keep);
        }
        for comment in &mut content.comments {
            each_paragraph_mut(&mut comment.blocks, &mut keep);
        }
        if dropped {
            self.note("references", Status::Normalized);
        }
    }
}

fn link_bytes(link: Option<&Link>) -> usize {
    match link {
        Some(Link::External(target) | Link::Internal(target)) => target.len(),
        None => 0,
    }
}

fn change_bytes(change: Option<&Change>) -> usize {
    change.map_or(0, |change| change.author.len() + change.date.len())
}

fn metadata_bytes(run: &Run) -> usize {
    link_bytes(run.link.as_ref()) + change_bytes(run.change.as_ref())
}

fn named_kind(name: &str) -> Option<Kind> {
    match name {
        "title" => Some(Kind::Title),
        "caption" => Some(Kind::Caption),
        _ => name
            .strip_prefix("heading ")
            .and_then(|level| level.parse().ok())
            .filter(|level| (1..=9).contains(level))
            .map(Kind::Heading),
    }
}

fn valid_format(format: &str) -> bool {
    (1..=32).contains(&format.len()) && format.bytes().all(|byte| byte.is_ascii_alphabetic())
}

/// Every string an export would emit, for the DLP and public-boundary check.
/// Paragraph text is joined across runs, so a marker split over runs is found.
fn emitted_texts(content: &Content) -> Vec<String> {
    let mut texts = Vec::new();
    content.paragraphs(&mut |paragraph| {
        texts.push(paragraph.full_text());
        texts.push(paragraph.text());
        for inline in &paragraph.inlines {
            match inline {
                Inline::Run(run) => {
                    if let RunContent::Image(image) = &run.content {
                        texts.extend([image.name.clone(), image.description.clone()]);
                    }
                    if let Some(Link::External(target) | Link::Internal(target)) = &run.link {
                        texts.push(target.clone());
                    }
                    if let Some(change) = &run.change {
                        texts.extend([change.author.clone(), change.date.clone()]);
                    }
                }
                Inline::BookmarkStart(name) | Inline::BookmarkEnd(name) => texts.push(name.clone()),
                Inline::CommentStart(_) | Inline::CommentEnd(_) => {}
            }
        }
    });
    for comment in &content.comments {
        texts.extend([
            comment.author.clone(),
            comment.date.clone(),
            comment.initials.clone(),
        ]);
    }
    texts.retain(|text| !text.is_empty());
    texts
}

fn check_texts(texts: &[String]) -> Result<(), Refusal> {
    for text in texts {
        match validate_emitted_string(text) {
            Err(ExportError::DlpViolation(_)) => return Err(Refusal::DlpFindings),
            Err(_) => return Err(Refusal::PublicBoundary),
            Ok(()) => {}
        }
    }
    Ok(())
}

// ---- Canonical JSON ----

fn text_json(value: &str) -> Json {
    Json::Str(value.to_owned())
}

fn fields<const N: usize>(entries: [(&str, Json); N]) -> Map {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

fn object<const N: usize>(entries: [(&str, Json); N]) -> Json {
    Json::Obj(fields(entries))
}

fn blocks_json(blocks: &[Block]) -> Json {
    Json::Arr(blocks.iter().map(block_json).collect())
}

fn block_json(block: &Block) -> Json {
    let paragraph = match block {
        Block::Paragraph(paragraph) => paragraph,
        Block::Table(rows) => {
            let cell = |cell: &Cell| {
                object([
                    ("span", Json::Int(cell.span.into())),
                    ("merge", text_json(merge_code(cell.merge))),
                    ("blocks", blocks_json(&cell.blocks)),
                ])
            };
            let rows = rows
                .iter()
                .map(|row| Json::Arr(row.iter().map(cell).collect()));
            return object([
                ("type", text_json("table")),
                ("rows", Json::Arr(rows.collect())),
            ]);
        }
    };
    let mut map = fields([
        ("type", text_json("paragraph")),
        ("section_break", Json::Bool(paragraph.section_break)),
        (
            "inlines",
            Json::Arr(paragraph.inlines.iter().map(inline_json).collect()),
        ),
    ]);
    let kind = match &paragraph.kind {
        Kind::Body => "body",
        Kind::Title => "title",
        Kind::Caption => "caption",
        Kind::Heading(level) => {
            map.insert("level".to_owned(), Json::Int((*level).into()));
            "heading"
        }
        Kind::List { num, level, format } => {
            map.insert("level".to_owned(), Json::Int((*level).into()));
            map.insert("num".to_owned(), Json::Int((*num).into()));
            map.insert("format".to_owned(), text_json(format));
            "list"
        }
    };
    map.insert("kind".to_owned(), text_json(kind));
    Json::Obj(map)
}

const fn merge_code(merge: Merge) -> &'static str {
    match merge {
        Merge::Single => "single",
        Merge::Restart => "restart",
        Merge::Continue => "continue",
    }
}

fn inline_json(inline: &Inline) -> Json {
    let run = match inline {
        Inline::CommentStart(id) => {
            return object([("type", text_json("comment-start")), ("id", Json::Int(*id))])
        }
        Inline::CommentEnd(id) => {
            return object([("type", text_json("comment-end")), ("id", Json::Int(*id))])
        }
        Inline::BookmarkStart(name) => {
            return object([
                ("type", text_json("bookmark-start")),
                ("name", text_json(name)),
            ])
        }
        Inline::BookmarkEnd(name) => {
            return object([
                ("type", text_json("bookmark-end")),
                ("name", text_json(name)),
            ])
        }
        Inline::Run(run) => run,
    };
    let mut map = match &run.content {
        RunContent::Text(text) => fields([("type", text_json("text")), ("text", text_json(text))]),
        RunContent::Image(image) => fields([
            ("type", text_json("image")),
            ("asset", text_json(&image.asset)),
            ("width", Json::Int(image.width)),
            ("height", Json::Int(image.height)),
            ("name", text_json(&image.name)),
            ("description", text_json(&image.description)),
        ]),
        RunContent::FootnoteRef(id) => {
            fields([("type", text_json("footnote-ref")), ("id", Json::Int(*id))])
        }
        RunContent::EndnoteRef(id) => {
            fields([("type", text_json("endnote-ref")), ("id", Json::Int(*id))])
        }
        RunContent::CommentRef(id) => {
            fields([("type", text_json("comment-ref")), ("id", Json::Int(*id))])
        }
    };
    map.insert("bold".to_owned(), Json::Bool(run.bold));
    map.insert("italic".to_owned(), Json::Bool(run.italic));
    map.insert("underline".to_owned(), Json::Bool(run.underline));
    map.insert(
        "link".to_owned(),
        match &run.link {
            None => Json::Null,
            Some(Link::External(target)) => object([("external", text_json(target))]),
            Some(Link::Internal(target)) => object([("internal", text_json(target))]),
        },
    );
    map.insert(
        "change".to_owned(),
        match &run.change {
            None => Json::Null,
            Some(change) => object([
                ("kind", text_json(change.kind.element())),
                ("id", Json::Int(change.id)),
                ("author", text_json(&change.author)),
                ("date", text_json(&change.date)),
            ]),
        },
    );
    Json::Obj(map)
}

impl Document {
    /// Canonical JSON (the envelope byte rule) of the model, citation anchors,
    /// fidelity, and diagnostics. Asset bytes are carried base64-encoded, so an
    /// export needs nothing but these bytes.
    pub fn canonical(&self) -> Vec<u8> {
        let content = &self.content;
        let notes = |notes: &[Note]| {
            Json::Arr(
                notes
                    .iter()
                    .map(|note| {
                        object([
                            ("id", Json::Int(note.id)),
                            ("blocks", blocks_json(&note.blocks)),
                        ])
                    })
                    .collect(),
            )
        };
        let comments = content.comments.iter().map(|comment| {
            object([
                ("id", Json::Int(comment.id)),
                ("author", text_json(&comment.author)),
                ("date", text_json(&comment.date)),
                ("initials", text_json(&comment.initials)),
                ("blocks", blocks_json(&comment.blocks)),
            ])
        });
        let assets = content.assets.iter().map(|(digest, asset)| {
            object([
                ("sha256", text_json(digest)),
                ("media_type", text_json(&asset.media_type)),
                ("byte_length", Json::Int(asset.data.len() as i64)),
                ("data", Json::Str(base64(&asset.data))),
            ])
        });
        let anchors = self.citations().into_iter().map(|citation| {
            object([
                ("id", text_json(&citation.id)),
                ("family", text_json("flow")),
                ("role", text_json(citation.role)),
                (
                    "locator",
                    object([
                        ("family", text_json("flow")),
                        ("section", text_json(&citation.section)),
                        ("ordinal", Json::Int(citation.ordinal.into())),
                    ]),
                ),
                (
                    "text_digest",
                    text_json(&sha256_hex(citation.text.as_bytes())),
                ),
                ("text", Json::Str(citation.text)),
            ])
        });
        object([
            ("schema_version", text_json(MODEL_VERSION)),
            ("format", text_json("docx")),
            ("family", text_json("flow")),
            ("blocks", blocks_json(&content.blocks)),
            ("footnotes", notes(&content.footnotes)),
            ("endnotes", notes(&content.endnotes)),
            ("comments", Json::Arr(comments.collect())),
            ("assets", Json::Arr(assets.collect())),
            ("anchors", Json::Arr(anchors.collect())),
            (
                "fidelity",
                Json::Obj(
                    self.fidelity
                        .iter()
                        .map(|(feature, status)| (feature.clone(), text_json(status.code())))
                        .collect(),
                ),
            ),
            (
                "diagnostics",
                Json::Arr(
                    self.diagnostics
                        .iter()
                        .map(|code| text_json(code))
                        .collect(),
                ),
            ),
        ])
        .to_bytes()
    }

    /// Reads canonical bytes back. Anything that does not re-serialize to the
    /// same bytes, or whose assets do not match their digests, is refused.
    pub fn from_canonical(bytes: &[u8]) -> Result<Document, Refusal> {
        let root = parse_json(bytes)?;
        let map = as_map(&root)?;
        if string(map, "schema_version")? != MODEL_VERSION || string(map, "format")? != "docx" {
            return Err(Refusal::MalformedContainer);
        }
        let mut assets = BTreeMap::new();
        for asset in array(map, "assets")? {
            let asset = as_map(asset)?;
            let digest = string(asset, "sha256")?;
            let data = unbase64(&string(asset, "data")?).ok_or(Refusal::MalformedContainer)?;
            if sha256_hex(&data) != digest {
                return Err(Refusal::MalformedContainer);
            }
            let media_type = string(asset, "media_type")?;
            assets.insert(digest, Asset { media_type, data });
        }
        let notes = |key| {
            array(map, key)?
                .iter()
                .map(|note| {
                    let note = as_map(note)?;
                    Ok(Note {
                        id: integer(note, "id")?,
                        blocks: blocks_from(array(note, "blocks")?)?,
                    })
                })
                .collect::<Result<Vec<_>, Refusal>>()
        };
        let comments = array(map, "comments")?
            .iter()
            .map(|comment| {
                let comment = as_map(comment)?;
                Ok(Comment {
                    id: integer(comment, "id")?,
                    author: string(comment, "author")?,
                    date: string(comment, "date")?,
                    initials: string(comment, "initials")?,
                    blocks: blocks_from(array(comment, "blocks")?)?,
                })
            })
            .collect::<Result<Vec<_>, Refusal>>()?;
        let content = Content {
            blocks: blocks_from(array(map, "blocks")?)?,
            footnotes: notes("footnotes")?,
            endnotes: notes("endnotes")?,
            comments,
            assets,
        };
        let mut missing = false;
        content.paragraphs(&mut |paragraph| {
            for inline in &paragraph.inlines {
                if let Inline::Run(Run {
                    content: RunContent::Image(image),
                    ..
                }) = inline
                {
                    missing |= !content.assets.contains_key(&image.asset);
                }
            }
        });
        let fidelity = as_map(field(map, "fidelity")?)?
            .iter()
            .map(|(feature, status)| match status {
                Json::Str(code) => Status::parse(code)
                    .map(|status| (feature.clone(), status))
                    .ok_or(Refusal::MalformedContainer),
                _ => Err(Refusal::MalformedContainer),
            })
            .collect::<Result<_, _>>()?;
        let diagnostics = array(map, "diagnostics")?
            .iter()
            .map(|code| match code {
                Json::Str(code) => Ok(code.clone()),
                _ => Err(Refusal::MalformedContainer),
            })
            .collect::<Result<_, _>>()?;
        let document = Document {
            content,
            fidelity,
            diagnostics,
        };
        if missing || document.canonical() != bytes {
            return Err(Refusal::MalformedContainer);
        }
        Ok(document)
    }
}

type Map = BTreeMap<String, Json>;

fn as_map(json: &Json) -> Result<&Map, Refusal> {
    match json {
        Json::Obj(map) => Ok(map),
        _ => Err(Refusal::MalformedContainer),
    }
}

fn field<'j>(map: &'j Map, key: &str) -> Result<&'j Json, Refusal> {
    map.get(key).ok_or(Refusal::MalformedContainer)
}

fn string(map: &Map, key: &str) -> Result<String, Refusal> {
    match field(map, key)? {
        Json::Str(value) => Ok(value.clone()),
        _ => Err(Refusal::MalformedContainer),
    }
}

fn integer(map: &Map, key: &str) -> Result<i64, Refusal> {
    match field(map, key)? {
        Json::Int(value) => Ok(*value),
        _ => Err(Refusal::MalformedContainer),
    }
}

fn ranged<T: TryFrom<i64>>(map: &Map, key: &str, min: i64, max: i64) -> Result<T, Refusal> {
    let value = integer(map, key)?;
    if !(min..=max).contains(&value) {
        return Err(Refusal::MalformedContainer);
    }
    T::try_from(value).map_err(|_| Refusal::MalformedContainer)
}

fn flag(map: &Map, key: &str) -> Result<bool, Refusal> {
    match field(map, key)? {
        Json::Bool(value) => Ok(*value),
        _ => Err(Refusal::MalformedContainer),
    }
}

fn array<'j>(map: &'j Map, key: &str) -> Result<&'j [Json], Refusal> {
    match field(map, key)? {
        Json::Arr(items) => Ok(items),
        _ => Err(Refusal::MalformedContainer),
    }
}

fn blocks_from(items: &[Json]) -> Result<Vec<Block>, Refusal> {
    items.iter().map(block_from).collect()
}

fn block_from(json: &Json) -> Result<Block, Refusal> {
    let map = as_map(json)?;
    if string(map, "type")? == "table" {
        let rows = array(map, "rows")?.iter().map(|row| {
            let Json::Arr(cells) = row else {
                return Err(Refusal::MalformedContainer);
            };
            cells
                .iter()
                .map(|cell| {
                    let cell = as_map(cell)?;
                    let merge = [Merge::Single, Merge::Restart, Merge::Continue]
                        .into_iter()
                        .find(|merge| {
                            merge_code(*merge) == string(cell, "merge").unwrap_or_default()
                        })
                        .ok_or(Refusal::MalformedContainer)?;
                    Ok(Cell {
                        span: ranged(cell, "span", 1, 63)?,
                        merge,
                        blocks: blocks_from(array(cell, "blocks")?)?,
                    })
                })
                .collect()
        });
        return Ok(Block::Table(rows.collect::<Result<_, _>>()?));
    }
    let kind = match string(map, "kind")?.as_str() {
        "body" => Kind::Body,
        "title" => Kind::Title,
        "caption" => Kind::Caption,
        "heading" => Kind::Heading(ranged(map, "level", 1, 9)?),
        "list" => {
            let format = string(map, "format")?;
            if !valid_format(&format) {
                return Err(Refusal::MalformedContainer);
            }
            Kind::List {
                num: ranged(map, "num", 1, u32::MAX.into())?,
                level: ranged(map, "level", 0, 8)?,
                format,
            }
        }
        _ => return Err(Refusal::MalformedContainer),
    };
    Ok(Block::Paragraph(Paragraph {
        kind,
        section_break: flag(map, "section_break")?,
        inlines: array(map, "inlines")?
            .iter()
            .map(inline_from)
            .collect::<Result<_, _>>()?,
    }))
}

fn inline_from(json: &Json) -> Result<Inline, Refusal> {
    let map = as_map(json)?;
    let content = match string(map, "type")?.as_str() {
        "comment-start" => return Ok(Inline::CommentStart(integer(map, "id")?)),
        "comment-end" => return Ok(Inline::CommentEnd(integer(map, "id")?)),
        "bookmark-start" => return Ok(Inline::BookmarkStart(string(map, "name")?)),
        "bookmark-end" => return Ok(Inline::BookmarkEnd(string(map, "name")?)),
        "text" => RunContent::Text(string(map, "text")?),
        "image" => RunContent::Image(Image {
            asset: string(map, "asset")?,
            width: ranged(map, "width", 1, MAX_EMU)?,
            height: ranged(map, "height", 1, MAX_EMU)?,
            name: string(map, "name")?,
            description: string(map, "description")?,
        }),
        "footnote-ref" => RunContent::FootnoteRef(integer(map, "id")?),
        "endnote-ref" => RunContent::EndnoteRef(integer(map, "id")?),
        "comment-ref" => RunContent::CommentRef(integer(map, "id")?),
        _ => return Err(Refusal::MalformedContainer),
    };
    let link = match field(map, "link")? {
        Json::Null => None,
        link => {
            let link = as_map(link)?;
            match (link.get("external"), link.get("internal")) {
                (Some(Json::Str(target)), None) => Some(Link::External(target.clone())),
                (None, Some(Json::Str(target))) => Some(Link::Internal(target.clone())),
                _ => return Err(Refusal::MalformedContainer),
            }
        }
    };
    let change = match field(map, "change")? {
        Json::Null => None,
        change => {
            let change = as_map(change)?;
            let kind = string(change, "kind")?;
            Some(Change {
                kind: ChangeKind::ALL
                    .into_iter()
                    .find(|candidate| candidate.element() == kind)
                    .ok_or(Refusal::MalformedContainer)?,
                id: integer(change, "id")?,
                author: string(change, "author")?,
                date: string(change, "date")?,
            })
        }
    };
    Ok(Inline::Run(Run {
        content,
        bold: flag(map, "bold")?,
        italic: flag(map, "italic")?,
        underline: flag(map, "underline")?,
        link,
        change,
    }))
}

/// Parses canonical JSON only: no whitespace, integers only, bounded depth.
/// `from_canonical` then insists the value re-serializes to the same bytes.
fn parse_json(bytes: &[u8]) -> Result<Json, Refusal> {
    let mut reader = JsonReader { bytes, at: 0 };
    let value = reader.value(0)?;
    if reader.at != bytes.len() {
        return Err(Refusal::MalformedContainer);
    }
    Ok(value)
}

struct JsonReader<'b> {
    bytes: &'b [u8],
    at: usize,
}

impl JsonReader<'_> {
    fn byte(&mut self) -> Result<u8, Refusal> {
        let byte = *self.bytes.get(self.at).ok_or(Refusal::MalformedContainer)?;
        self.at += 1;
        Ok(byte)
    }

    fn literal(&mut self, literal: &[u8]) -> Result<(), Refusal> {
        if !self.bytes[self.at..].starts_with(literal) {
            return Err(Refusal::MalformedContainer);
        }
        self.at += literal.len();
        Ok(())
    }

    fn value(&mut self, depth: usize) -> Result<Json, Refusal> {
        if depth > MAX_JSON_DEPTH {
            return Err(Refusal::MalformedContainer);
        }
        match self.bytes.get(self.at) {
            Some(b'{') => {
                self.at += 1;
                let mut map = BTreeMap::new();
                if self.bytes.get(self.at) == Some(&b'}') {
                    self.at += 1;
                    return Ok(Json::Obj(map));
                }
                loop {
                    let key = self.string()?;
                    self.literal(b":")?;
                    if map.insert(key, self.value(depth + 1)?).is_some() {
                        return Err(Refusal::MalformedContainer);
                    }
                    match self.byte()? {
                        b',' => {}
                        b'}' => return Ok(Json::Obj(map)),
                        _ => return Err(Refusal::MalformedContainer),
                    }
                }
            }
            Some(b'[') => {
                self.at += 1;
                let mut items = Vec::new();
                if self.bytes.get(self.at) == Some(&b']') {
                    self.at += 1;
                    return Ok(Json::Arr(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    match self.byte()? {
                        b',' => {}
                        b']' => return Ok(Json::Arr(items)),
                        _ => return Err(Refusal::MalformedContainer),
                    }
                }
            }
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => self.literal(b"true").map(|()| Json::Bool(true)),
            Some(b'f') => self.literal(b"false").map(|()| Json::Bool(false)),
            Some(b'n') => self.literal(b"null").map(|()| Json::Null),
            Some(b'-' | b'0'..=b'9') => {
                let start = self.at;
                self.at += 1;
                while matches!(self.bytes.get(self.at), Some(b'0'..=b'9')) {
                    self.at += 1;
                }
                std::str::from_utf8(&self.bytes[start..self.at])
                    .ok()
                    .and_then(|digits| digits.parse().ok())
                    .map(Json::Int)
                    .ok_or(Refusal::MalformedContainer)
            }
            _ => Err(Refusal::MalformedContainer),
        }
    }

    fn string(&mut self) -> Result<String, Refusal> {
        if self.byte()? != b'"' {
            return Err(Refusal::MalformedContainer);
        }
        let mut out = Vec::new();
        loop {
            match self.byte()? {
                b'"' => return String::from_utf8(out).map_err(|_| Refusal::MalformedContainer),
                b'\\' => {
                    let decoded = match self.byte()? {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        // The canonical writer escapes only C0 controls.
                        b'u' => char::from_u32(self.hex()?)
                            .filter(|c| (*c as u32) < 0x20)
                            .ok_or(Refusal::MalformedContainer)?,
                        _ => return Err(Refusal::MalformedContainer),
                    };
                    out.extend_from_slice(decoded.encode_utf8(&mut [0; 4]).as_bytes());
                }
                byte if byte < 0x20 => return Err(Refusal::MalformedContainer),
                byte => out.push(byte),
            }
        }
    }

    fn hex(&mut self) -> Result<u32, Refusal> {
        let digits = self
            .bytes
            .get(self.at..self.at + 4)
            .filter(|digits| digits.iter().all(u8::is_ascii_hexdigit))
            .ok_or(Refusal::MalformedContainer)?;
        self.at += 4;
        let digits = std::str::from_utf8(digits).map_err(|_| Refusal::MalformedContainer)?;
        u32::from_str_radix(digits, 16).map_err(|_| Refusal::MalformedContainer)
    }
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let bytes = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let value = u32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]]);
        for index in 0..4 {
            out.push(if index <= chunk.len() {
                BASE64[(value >> (18 - 6 * index)) as usize & 63] as char
            } else {
                '='
            });
        }
    }
    out
}

/// Strict inverse of `base64`: anything it would not have written is refused.
fn unbase64(text: &str) -> Option<Vec<u8>> {
    let sextet = |byte: u8| match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    if !text.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    for chunk in text.as_bytes().chunks(4) {
        let padding = chunk.iter().rev().take_while(|byte| **byte == b'=').count();
        let mut value = 0u32;
        for byte in &chunk[..4 - padding] {
            value = value << 6 | u32::from(sextet(*byte)?);
        }
        value <<= 6 * padding;
        out.extend_from_slice(&value.to_be_bytes()[1..4 - padding.min(2)]);
    }
    (base64(&out) == text).then_some(out)
}

// ---- Export ----

/// Exports canonical DOCX bytes and a fidelity receipt. DLP and the public
/// boundary are checked on every emitted string before any bytes are built.
pub fn export(imported: &Imported) -> Result<Exported, Refusal> {
    let (document, _) = prepare(imported)?;
    Ok(render(&document, &imported.canonical))
}

/// Exports to `root/relative` through the shared gate: explicit `.docx`
/// destination, contained, never overwritten, DLP checked before writing.
/// Returns the written path and the fidelity receipt.
pub fn export_file(
    root: &Path,
    relative: &str,
    imported: &Imported,
) -> Result<(PathBuf, Vec<u8>), Refusal> {
    let (document, texts) = prepare(imported)?;
    let exported = render(&document, &imported.canonical);
    let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
    let path = crate::export::write_new(root, relative, Format::Docx, &exported.bytes, &texts)?;
    Ok((path, exported.receipt))
}

fn prepare(imported: &Imported) -> Result<(Document, Vec<String>), Refusal> {
    let document = Document::from_canonical(&imported.canonical)?;
    for code in imported.receipt.iter().chain(&document.diagnostics) {
        match code.as_str() {
            "dlp-findings" => return Err(Refusal::DlpFindings),
            "public-boundary-violation" => return Err(Refusal::PublicBoundary),
            _ => {}
        }
    }
    let texts = emitted_texts(&document.content);
    check_texts(&texts)?;
    Ok((document, texts))
}

fn render(document: &Document, canonical: &[u8]) -> Exported {
    let bytes = write_docx(&document.content);
    let status = |statuses: &[(&str, Status)]| {
        Json::Obj(
            statuses
                .iter()
                .map(|(feature, status)| ((*feature).to_owned(), text_json(status.code())))
                .collect(),
        )
    };
    let receipt = object([
        ("schema_version", text_json(MODEL_VERSION)),
        ("format", text_json("docx")),
        ("source_canonical_sha256", text_json(&sha256_hex(canonical))),
        ("output_sha256", text_json(&sha256_hex(&bytes))),
        ("output_byte_length", Json::Int(bytes.len() as i64)),
        (
            "import_fidelity",
            Json::Obj(
                document
                    .fidelity
                    .iter()
                    .map(|(feature, status)| (feature.clone(), text_json(status.code())))
                    .collect(),
            ),
        ),
        // Everything in the model is written; formatting outside it is regenerated.
        (
            "export_fidelity",
            status(&[
                ("model", Status::Exact),
                ("styles", Status::Normalized),
                ("numbering", Status::Normalized),
                ("sections", Status::Normalized),
                ("tables", Status::Normalized),
                ("run-formatting", Status::Normalized),
            ]),
        ),
        (
            "anchors",
            Json::Arr(
                document
                    .citations()
                    .iter()
                    .map(|citation| text_json(&citation.id))
                    .collect(),
            ),
        ),
    ])
    .to_bytes();
    Exported { bytes, receipt }
}

const XML_DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";
const NAMESPACES: &str =
    "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" \
xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" \
xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"";
const SECTION: &str = "<w:sectPr><w:pgSz w:w=\"12240\" w:h=\"15840\"/><w:pgMar w:top=\"1440\" \
w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\" w:header=\"720\" w:footer=\"720\" \
w:gutter=\"0\"/></w:sectPr>";

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\t', "&#9;")
        .replace('\n', "&#10;")
        .replace('\r', "&#13;")
}

/// Relationships of one part, numbered in first-use order.
#[derive(Default)]
struct Rels(Vec<(String, String, bool)>);

impl Rels {
    fn id(&mut self, kind: &str, target: &str, external: bool) -> String {
        let entry = (kind.to_owned(), target.to_owned(), external);
        let index = match self.0.iter().position(|existing| *existing == entry) {
            Some(index) => index,
            None => {
                self.0.push(entry);
                self.0.len() - 1
            }
        };
        format!("rId{}", index + 1)
    }

    fn xml(&self) -> Vec<u8> {
        let mut out = format!(
            "{XML_DECL}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">"
        );
        for (index, (kind, target, external)) in self.0.iter().enumerate() {
            out.push_str(&format!(
                "<Relationship Id=\"rId{}\" Type=\"{REL}/{kind}\" Target=\"{}\"{}/>",
                index + 1,
                escape(target),
                if *external {
                    " TargetMode=\"External\""
                } else {
                    ""
                }
            ));
        }
        out.push_str("</Relationships>");
        out.into_bytes()
    }
}

fn extension(media_type: &str) -> &'static str {
    match media_type.to_ascii_lowercase().as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpeg",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        "image/tiff" => "tiff",
        "image/x-emf" | "image/emf" => "emf",
        "image/x-wmf" | "image/wmf" => "wmf",
        "image/svg+xml" => "svg",
        _ => "bin",
    }
}

struct Writer<'c> {
    content: &'c Content,
    media: BTreeSet<&'c str>,
    bookmarks: BTreeMap<&'c str, usize>,
    drawings: usize,
}

fn write_docx(content: &Content) -> Vec<u8> {
    let mut writer = Writer {
        content,
        media: BTreeSet::new(),
        bookmarks: BTreeMap::new(),
        drawings: 0,
    };
    let mut parts: Vec<(String, Vec<u8>)> = Vec::new();
    let mut types = vec![
        ("/word/document.xml".to_owned(), MAIN_TYPE.to_owned()),
        (
            "/word/styles.xml".to_owned(),
            format!("{WML_TYPE}styles+xml"),
        ),
    ];
    let mut rels = Rels::default();
    rels.id("styles", "styles.xml", false);
    parts.push(("word/styles.xml".to_owned(), styles_xml().into_bytes()));
    if let Some(numbering) = numbering_xml(content) {
        rels.id("numbering", "numbering.xml", false);
        parts.push(("word/numbering.xml".to_owned(), numbering.into_bytes()));
        types.push((
            "/word/numbering.xml".to_owned(),
            format!("{WML_TYPE}numbering+xml"),
        ));
    }
    let stories: [(&str, &[Note]); 2] = [
        ("footnote", &content.footnotes),
        ("endnote", &content.endnotes),
    ];
    for (item, notes) in stories {
        if notes.is_empty() {
            continue;
        }
        let mut part_rels = Rels::default();
        let xml = writer.notes(item, notes, &mut part_rels);
        add_story(
            &mut parts,
            &mut types,
            &mut rels,
            &format!("{item}s"),
            xml,
            part_rels,
        );
    }
    if !content.comments.is_empty() {
        let mut part_rels = Rels::default();
        let xml = writer.comments(&mut part_rels);
        add_story(
            &mut parts, &mut types, &mut rels, "comments", xml, part_rels,
        );
    }
    let mut body = format!("{XML_DECL}<w:document {NAMESPACES}><w:body>");
    writer.blocks(&mut body, &content.blocks, &mut rels, None);
    body.push_str(SECTION);
    body.push_str("</w:body></w:document>");
    parts.push(("word/document.xml".to_owned(), body.into_bytes()));
    parts.push(("word/_rels/document.xml.rels".to_owned(), rels.xml()));
    for digest in &writer.media {
        let asset = &content.assets[*digest];
        let name = format!("word/media/{digest}.{}", extension(&asset.media_type));
        types.push((format!("/{name}"), asset.media_type.clone()));
        parts.push((name, asset.data.clone()));
    }
    let mut package_rels = Rels::default();
    package_rels.id("officeDocument", "word/document.xml", false);
    parts.push(("_rels/.rels".to_owned(), package_rels.xml()));
    let mut content_types = format!(
        "{XML_DECL}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>"
    );
    types.sort();
    for (name, content_type) in &types {
        content_types.push_str(&format!(
            "<Override PartName=\"{}\" ContentType=\"{}\"/>",
            escape(name),
            escape(content_type)
        ));
    }
    content_types.push_str("</Types>");
    parts.push(("[Content_Types].xml".to_owned(), content_types.into_bytes()));
    parts.sort();
    let entries: Vec<zip::WriteEntry<'_>> = parts
        .iter()
        .map(|(name, data)| zip::WriteEntry {
            name,
            data,
            deflate: true,
            dos_time: 0,
            dos_date: 0x0021,
        })
        .collect();
    zip::write_with_ratio(&entries, Limits::default().max_ratio)
}

/// Adds a notes or comments part, its content type, and its relationships.
fn add_story(
    parts: &mut Vec<(String, Vec<u8>)>,
    types: &mut Vec<(String, String)>,
    rels: &mut Rels,
    name: &str,
    xml: String,
    part_rels: Rels,
) {
    rels.id(name, &format!("{name}.xml"), false);
    types.push((format!("/word/{name}.xml"), format!("{WML_TYPE}{name}+xml")));
    parts.push((format!("word/{name}.xml"), xml.into_bytes()));
    if !part_rels.0.is_empty() {
        parts.push((format!("word/_rels/{name}.xml.rels"), part_rels.xml()));
    }
}

impl<'c> Writer<'c> {
    fn notes(&mut self, item: &str, notes: &'c [Note], rels: &mut Rels) -> String {
        let mut out = format!("{XML_DECL}<w:{item}s {NAMESPACES}>");
        if !notes.iter().any(|note| note.id == -1 || note.id == 0) {
            for (id, kind) in [(-1, "separator"), (0, "continuationSeparator")] {
                out.push_str(&format!(
                    "<w:{item} w:type=\"{kind}\" w:id=\"{id}\"><w:p><w:r><w:{kind}/></w:r></w:p></w:{item}>"
                ));
            }
        }
        for note in notes {
            out.push_str(&format!("<w:{item} w:id=\"{}\">", note.id));
            self.blocks(&mut out, &note.blocks, rels, Some(&format!("{item}Ref")));
            out.push_str(&format!("</w:{item}>"));
        }
        out.push_str(&format!("</w:{item}s>"));
        out
    }

    fn comments(&mut self, rels: &mut Rels) -> String {
        let mut out = format!("{XML_DECL}<w:comments {NAMESPACES}>");
        for comment in &self.content.comments {
            out.push_str(&format!(
                "<w:comment w:id=\"{}\" w:author=\"{}\"",
                comment.id,
                escape(&comment.author)
            ));
            for (key, value) in [("date", &comment.date), ("initials", &comment.initials)] {
                if !value.is_empty() {
                    out.push_str(&format!(" w:{key}=\"{}\"", escape(value)));
                }
            }
            out.push('>');
            self.blocks(&mut out, &comment.blocks, rels, None);
            out.push_str("</w:comment>");
        }
        out.push_str("</w:comments>");
        out
    }

    /// `mark` is the note reference mark written at the start of the first paragraph.
    fn blocks(
        &mut self,
        out: &mut String,
        blocks: &'c [Block],
        rels: &mut Rels,
        mark: Option<&str>,
    ) {
        for (index, block) in blocks.iter().enumerate() {
            match block {
                Block::Paragraph(paragraph) => {
                    self.paragraph(out, paragraph, rels, mark.filter(|_| index == 0));
                }
                Block::Table(rows) => self.table(out, rows, rels),
            }
        }
    }

    fn table(&mut self, out: &mut String, rows: &'c [Vec<Cell>], rels: &mut Rels) {
        let columns = rows
            .iter()
            .map(|row| row.iter().map(|cell| cell.span as usize).sum::<usize>())
            .max()
            .unwrap_or(1)
            .max(1);
        out.push_str("<w:tbl><w:tblPr><w:tblStyle w:val=\"TableGrid\"/><w:tblW w:w=\"0\" w:type=\"auto\"/></w:tblPr><w:tblGrid>");
        for _ in 0..columns {
            out.push_str(&format!("<w:gridCol w:w=\"{}\"/>", (9360 / columns).max(1)));
        }
        out.push_str("</w:tblGrid>");
        for row in rows {
            out.push_str("<w:tr>");
            for cell in row {
                out.push_str("<w:tc>");
                if cell.span > 1 || cell.merge != Merge::Single {
                    out.push_str("<w:tcPr>");
                    if cell.span > 1 {
                        out.push_str(&format!("<w:gridSpan w:val=\"{}\"/>", cell.span));
                    }
                    match cell.merge {
                        Merge::Single => {}
                        Merge::Restart => out.push_str("<w:vMerge w:val=\"restart\"/>"),
                        Merge::Continue => out.push_str("<w:vMerge/>"),
                    }
                    out.push_str("</w:tcPr>");
                }
                self.blocks(out, &cell.blocks, rels, None);
                out.push_str("</w:tc>");
            }
            out.push_str("</w:tr>");
        }
        out.push_str("</w:tbl>");
    }

    fn paragraph(
        &mut self,
        out: &mut String,
        paragraph: &'c Paragraph,
        rels: &mut Rels,
        mark: Option<&str>,
    ) {
        out.push_str("<w:p>");
        let style = match &paragraph.kind {
            Kind::Body => None,
            Kind::Title => Some("Title".to_owned()),
            Kind::Caption => Some("Caption".to_owned()),
            Kind::Heading(level) => Some(format!("Heading{level}")),
            Kind::List { .. } => Some("ListParagraph".to_owned()),
        };
        if style.is_some() || paragraph.section_break {
            out.push_str("<w:pPr>");
            if let Some(style) = style {
                out.push_str(&format!("<w:pStyle w:val=\"{style}\"/>"));
            }
            if let Kind::List { num, level, .. } = &paragraph.kind {
                out.push_str(&format!(
                    "<w:numPr><w:ilvl w:val=\"{level}\"/><w:numId w:val=\"{num}\"/></w:numPr>"
                ));
            }
            if paragraph.section_break {
                out.push_str(SECTION);
            }
            out.push_str("</w:pPr>");
        }
        if let Some(mark) = mark {
            out.push_str(&format!(
                "<w:r><w:rPr><w:vertAlign w:val=\"superscript\"/></w:rPr><w:{mark}/></w:r>"
            ));
        }
        self.inlines(out, &paragraph.inlines, rels);
        out.push_str("</w:p>");
    }

    /// Runs are grouped into one hyperlink per link and, inside it, one
    /// change element per tracked change, matching WordprocessingML nesting.
    fn inlines(&mut self, out: &mut String, inlines: &'c [Inline], rels: &mut Rels) {
        let mut at = 0;
        while at < inlines.len() {
            let Inline::Run(first) = &inlines[at] else {
                self.marker(out, &inlines[at]);
                at += 1;
                continue;
            };
            let runs: Vec<&Run> = inlines[at..]
                .iter()
                .map_while(|inline| match inline {
                    Inline::Run(run) if run.link == first.link => Some(run),
                    _ => None,
                })
                .collect();
            at += runs.len();
            match &first.link {
                None => {}
                Some(Link::External(target)) => out.push_str(&format!(
                    "<w:hyperlink r:id=\"{}\">",
                    rels.id("hyperlink", target, true)
                )),
                Some(Link::Internal(name)) => {
                    out.push_str(&format!("<w:hyperlink w:anchor=\"{}\">", escape(name)));
                }
            }
            for group in runs.chunk_by(|left, right| left.change == right.change) {
                let change = &group[0].change;
                if let Some(change) = change {
                    out.push_str(&format!(
                        "<w:{} w:id=\"{}\" w:author=\"{}\"",
                        change.kind.element(),
                        change.id,
                        escape(&change.author)
                    ));
                    if !change.date.is_empty() {
                        out.push_str(&format!(" w:date=\"{}\"", escape(&change.date)));
                    }
                    out.push('>');
                }
                for run in group {
                    self.run(out, run, rels);
                }
                if let Some(change) = change {
                    out.push_str(&format!("</w:{}>", change.kind.element()));
                }
            }
            if first.link.is_some() {
                out.push_str("</w:hyperlink>");
            }
        }
    }

    fn marker(&mut self, out: &mut String, inline: &'c Inline) {
        let next = self.bookmarks.len();
        match inline {
            Inline::CommentStart(id) => {
                out.push_str(&format!("<w:commentRangeStart w:id=\"{id}\"/>"));
            }
            Inline::CommentEnd(id) => out.push_str(&format!("<w:commentRangeEnd w:id=\"{id}\"/>")),
            Inline::BookmarkStart(name) => {
                let id = *self.bookmarks.entry(name).or_insert(next);
                out.push_str(&format!(
                    "<w:bookmarkStart w:id=\"{id}\" w:name=\"{}\"/>",
                    escape(name)
                ));
            }
            Inline::BookmarkEnd(name) => {
                let id = *self.bookmarks.entry(name).or_insert(next);
                out.push_str(&format!("<w:bookmarkEnd w:id=\"{id}\"/>"));
            }
            Inline::Run(_) => {}
        }
    }

    fn run(&mut self, out: &mut String, run: &'c Run, rels: &mut Rels) {
        out.push_str("<w:r>");
        let mut props = String::new();
        for (on, element) in [
            (run.bold, "<w:b/>"),
            (run.italic, "<w:i/>"),
            (run.underline, "<w:u w:val=\"single\"/>"),
            (
                matches!(
                    run.content,
                    RunContent::FootnoteRef(_) | RunContent::EndnoteRef(_)
                ),
                "<w:vertAlign w:val=\"superscript\"/>",
            ),
        ] {
            if on {
                props.push_str(element);
            }
        }
        if !props.is_empty() {
            out.push_str(&format!("<w:rPr>{props}</w:rPr>"));
        }
        match &run.content {
            RunContent::Text(text) => {
                let removed = run
                    .change
                    .as_ref()
                    .is_some_and(|change| change.kind.removes());
                let tag = if removed { "w:delText" } else { "w:t" };
                for (line_index, line) in text.split('\n').enumerate() {
                    if line_index > 0 {
                        out.push_str("<w:br/>");
                    }
                    for (index, piece) in line.split('\t').enumerate() {
                        if index > 0 {
                            out.push_str("<w:tab/>");
                        }
                        if !piece.is_empty() {
                            out.push_str(&format!(
                                "<{tag} xml:space=\"preserve\">{}</{tag}>",
                                escape(piece)
                            ));
                        }
                    }
                }
            }
            RunContent::Image(image) => self.drawing(out, image, rels),
            RunContent::FootnoteRef(id) => {
                out.push_str(&format!("<w:footnoteReference w:id=\"{id}\"/>"));
            }
            RunContent::EndnoteRef(id) => {
                out.push_str(&format!("<w:endnoteReference w:id=\"{id}\"/>"));
            }
            RunContent::CommentRef(id) => {
                out.push_str(&format!("<w:commentReference w:id=\"{id}\"/>"));
            }
        }
        out.push_str("</w:r>");
    }

    fn drawing(&mut self, out: &mut String, image: &'c Image, rels: &mut Rels) {
        let Some(asset) = self.content.assets.get(&image.asset) else {
            return;
        };
        self.media.insert(&image.asset);
        self.drawings += 1;
        let target = format!("media/{}.{}", image.asset, extension(&asset.media_type));
        let id = rels.id("image", &target, false);
        let (drawing, cx, cy) = (self.drawings, image.width, image.height);
        let (name, description) = (escape(&image.name), escape(&image.description));
        out.push_str(&format!(
            "<w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\">\
<wp:extent cx=\"{cx}\" cy=\"{cy}\"/><wp:docPr id=\"{drawing}\" name=\"{name}\" descr=\"{description}\"/>\
<a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
<pic:pic><pic:nvPicPr><pic:cNvPr id=\"{drawing}\" name=\"{name}\"/><pic:cNvPicPr/></pic:nvPicPr>\
<pic:blipFill><a:blip r:embed=\"{id}\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
<pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic>\
</wp:inline></w:drawing>"
        ));
    }
}

fn numbering_xml(content: &Content) -> Option<String> {
    let mut lists: BTreeMap<u32, BTreeMap<u8, &str>> = BTreeMap::new();
    content.paragraphs(&mut |paragraph| {
        if let Kind::List { num, level, format } = &paragraph.kind {
            lists
                .entry(*num)
                .or_default()
                .entry(*level)
                .or_insert(format);
        }
    });
    if lists.is_empty() {
        return None;
    }
    let mut out = format!(
        "{XML_DECL}<w:numbering xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">"
    );
    for (num, levels) in &lists {
        out.push_str(&format!(
            "<w:abstractNum w:abstractNumId=\"{num}\"><w:multiLevelType w:val=\"hybridMultilevel\"/>"
        ));
        for level in 0..9u8 {
            let format = levels.get(&level).copied().unwrap_or("decimal");
            let text = if format == "bullet" {
                "\u{2022}".to_owned()
            } else {
                format!("%{}.", level + 1)
            };
            out.push_str(&format!(
                "<w:lvl w:ilvl=\"{level}\"><w:start w:val=\"1\"/><w:numFmt w:val=\"{format}\"/>\
<w:lvlText w:val=\"{text}\"/><w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"{}\" w:hanging=\"360\"/></w:pPr></w:lvl>",
                720 * (u32::from(level) + 1)
            ));
        }
        out.push_str("</w:abstractNum>");
    }
    for num in lists.keys() {
        out.push_str(&format!(
            "<w:num w:numId=\"{num}\"><w:abstractNumId w:val=\"{num}\"/></w:num>"
        ));
    }
    out.push_str("</w:numbering>");
    Some(out)
}

/// The fixed style sheet every export carries: only the styles the model uses.
fn styles_xml() -> String {
    let paragraph = |id: &str, name: &str, extra: &str| {
        format!(
            "<w:style w:type=\"paragraph\" w:styleId=\"{id}\"><w:name w:val=\"{name}\"/>\
<w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:qFormat/>{extra}</w:style>"
        )
    };
    let mut out = format!(
        "{XML_DECL}<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
<w:docDefaults><w:rPrDefault><w:rPr><w:sz w:val=\"22\"/></w:rPr></w:rPrDefault>\
<w:pPrDefault><w:pPr><w:spacing w:after=\"120\"/></w:pPr></w:pPrDefault></w:docDefaults>\
<w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/><w:qFormat/></w:style>"
    );
    out.push_str(&paragraph(
        "Title",
        "Title",
        "<w:rPr><w:b/><w:sz w:val=\"48\"/></w:rPr>",
    ));
    for level in 1..=9u32 {
        out.push_str(&paragraph(
            &format!("Heading{level}"),
            &format!("heading {level}"),
            &format!(
                "<w:pPr><w:keepNext/><w:outlineLvl w:val=\"{}\"/></w:pPr><w:rPr><w:b/><w:sz w:val=\"{}\"/></w:rPr>",
                level - 1,
                (36 - 2 * level.min(6)).max(22)
            ),
        ));
    }
    out.push_str(&paragraph(
        "Caption",
        "caption",
        "<w:rPr><w:i/><w:sz w:val=\"18\"/></w:rPr>",
    ));
    out.push_str(&paragraph("ListParagraph", "List Paragraph", ""));
    let border = |side: &str| {
        format!("<w:{side} w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>")
    };
    out.push_str(&format!(
        "<w:style w:type=\"table\" w:styleId=\"TableGrid\"><w:name w:val=\"Table Grid\"/><w:tblPr><w:tblBorders>{}</w:tblBorders></w:tblPr></w:style></w:styles>",
        ["top", "left", "bottom", "right", "insideH", "insideV"]
            .map(border)
            .concat()
    ));
    out
}

// ---- Adapter ----

pub struct Docx;

impl Adapter for Docx {
    fn format(&self) -> Format {
        Format::Docx
    }

    fn import(&self, bytes: &[u8], limits: &Limits, cancel: &Cancel) -> Result<Imported, Refusal> {
        read(bytes, limits, cancel).map(|document| document.imported())
    }

    fn export(&self, imported: &Imported) -> Result<Vec<u8>, Refusal> {
        export(imported).map(|exported| exported.bytes)
    }
}
