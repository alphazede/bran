//! XLSX adapter (issue #26): SpreadsheetML to BRAN's grid projection and back.
//!
//! Import runs on the shared intake (`opc`) and XML reader (`xml`), so package
//! safety lives in one place. Nothing is calculated, fetched, or executed:
//! formulas are text, a cached result is labelled as cached, and every
//! workbook feature BRAN does not model is listed in the projection's
//! `unsupported` list and the receipt, never dropped silently.

use crate::canonical::{sha256_hex, Json};
use crate::conformance::{Adapter, Anchor, Imported};
use crate::opc::{self, Package, Part, Relationship};
use crate::xml::{self, Event};
use crate::{zip, Cancel, Format, Limits, Refusal, RECEIPT_VERSION};
use bran_core::export::{validate_emitted_string, ExportError};
use std::collections::{BTreeMap, BTreeSet};

/// Version of the canonical grid projection below.
pub const PROJECTION: &str = "bran-xlsx-grid/1";
/// Part and relationship type of the fidelity receipt inside an export.
pub const RECEIPT_PART: &str = "bran/fidelity-receipt.json";
const RECEIPT_TYPE: &str = "https://schemas.alphazede.dev/bran/relationships/fidelity-receipt";
const WORKBOOK_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml";
const MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const OFFICE_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const PACKAGE_REL: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const MAX_ROW: u32 = 1_048_576;
const MAX_COLUMN: u32 = 16_384;
/// Worksheet children that only carry layout or print settings.
const LAYOUT: [&str; 13] = [
    "sheetPr",
    "sheetViews",
    "sheetFormatPr",
    "cols",
    "sheetCalcPr",
    "pageMargins",
    "pageSetup",
    "headerFooter",
    "printOptions",
    "rowBreaks",
    "colBreaks",
    "phoneticPr",
    "ignoredErrors",
];
/// Workbook children that only carry application settings.
const SETTINGS: [&str; 9] = [
    "fileVersion",
    "workbookPr",
    "bookViews",
    "calcPr",
    "AlternateContent",
    "revisionPtr",
    "absPath",
    "fileRecoveryPr",
    "extLst",
];
/// Relationship kinds consumed without modelling their content, with the
/// receipt code that says so.
const NOT_IMPORTED: [(&str, &str); 6] = [
    ("theme", "styles-reduced-to-number-formats"),
    ("calcChain", "calculation-chain-not-imported"),
    ("core-properties", "document-properties-not-imported"),
    ("extended-properties", "document-properties-not-imported"),
    ("custom-properties", "document-properties-not-imported"),
    ("printerSettings", "layout-not-imported"),
];
const VALIDATION_ATTRIBUTES: [&str; 12] = [
    "type",
    "errorStyle",
    "imeMode",
    "operator",
    "allowBlank",
    "showDropDown",
    "showInputMessage",
    "showErrorMessage",
    "errorTitle",
    "error",
    "promptTitle",
    "prompt",
];
/// Functions that reach outside the workbook when a spreadsheet application
/// calculates them.
const ACTIVE_FUNCTIONS: [&str; 8] = [
    "WEBSERVICE(",
    "FILTERXML(",
    "IMAGE(",
    "RTD(",
    "CALL(",
    "REGISTER(",
    "REGISTER.ID(",
    "EXEC(",
];

pub struct Xlsx;

impl Adapter for Xlsx {
    fn format(&self) -> Format {
        Format::Xlsx
    }

    fn import(&self, bytes: &[u8], limits: &Limits, cancel: &Cancel) -> Result<Imported, Refusal> {
        import(bytes, limits, cancel)
    }

    fn export(&self, imported: &Imported) -> Result<Vec<u8>, Refusal> {
        export(imported).map(|exported| exported.bytes)
    }
}

/// An exported workbook and its canonical fidelity receipt. The same receipt
/// bytes are stored in the package as `bran/fidelity-receipt.json`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Exported {
    pub bytes: Vec<u8>,
    pub receipt: Vec<u8>,
}

// ---- Import ----

/// Imports an untrusted XLSX package into the canonical grid projection.
pub fn import(bytes: &[u8], limits: &Limits, cancel: &Cancel) -> Result<Imported, Refusal> {
    let package = opc::open(bytes, limits, cancel)?;
    refuse_hazards(&package)?;
    let mut reader = Reader {
        package: &package,
        limits,
        cancel,
        consumed: BTreeSet::new(),
        codes: package
            .diagnostics
            .iter()
            .map(|c| (*c).to_owned())
            .collect(),
        unsupported: BTreeSet::new(),
        tables: BTreeSet::new(),
    };
    let (sheets, names, anchors) = reader.workbook()?;
    reader.leftovers();
    let mut strings = Vec::new();
    collect_strings(&sheets, &mut strings);
    collect_strings(&names, &mut strings);
    for text in strings {
        match validate_emitted_string(text) {
            Err(ExportError::DlpViolation(_)) => reader.codes.insert("dlp-findings".to_owned()),
            Err(_) => reader.codes.insert("public-boundary-violation".to_owned()),
            Ok(()) => false,
        };
    }
    let codes = reader.codes;
    let unsupported = reader
        .unsupported
        .into_iter()
        .map(|entry| {
            Json::Obj(
                entry
                    .into_iter()
                    .map(|(key, value)| (key.to_owned(), Json::Str(value)))
                    .collect(),
            )
        })
        .collect();
    let canonical = obj(vec![
        ("schema_version", s(PROJECTION)),
        ("family", s("grid")),
        ("sheets", sheets),
        ("defined_names", names),
        ("anchors", Json::Arr(anchors.clone())),
        ("unsupported", Json::Arr(unsupported)),
        ("fidelity", fidelity(&codes)),
        ("receipt", Json::Arr(codes.iter().map(|c| s(c)).collect())),
    ])
    .to_bytes();
    Ok(Imported {
        canonical,
        receipt: codes.into_iter().collect(),
        anchors: anchors
            .iter()
            .map(|anchor| Anchor {
                id: str_field(anchor, "id").unwrap_or_default().to_owned(),
                text_digest: str_field(anchor, "text_digest")
                    .unwrap_or_default()
                    .to_owned(),
            })
            .collect(),
    })
}

/// Workbook parts the shared intake admits but an XLSX import must not:
/// external workbook links, query tables, data models, and Power Query.
fn refuse_hazards(package: &Package) -> Result<(), Refusal> {
    let utf16 = |order: fn(u16) -> [u8; 2]| -> Vec<u8> {
        "DataMashup".encode_utf16().flat_map(order).collect()
    };
    let mashup = [
        b"DataMashup".to_vec(),
        utf16(u16::to_le_bytes),
        utf16(u16::to_be_bytes),
    ];
    for part in &package.parts {
        let content_type = part.content_type.to_ascii_lowercase();
        if ["externallink", "querytable", "model+data"]
            .iter()
            .any(|marker| content_type.contains(marker))
        {
            return Err(Refusal::ExternalReference);
        }
        // Power Query stores its M code in a custom XML part with a generic
        // content type, often UTF-16, so match the root name as bytes.
        if part.name.to_ascii_lowercase().starts_with("customxml/")
            && mashup
                .iter()
                .any(|needle| part.data.windows(needle.len()).any(|w| w == needle))
        {
            return Err(Refusal::ActiveContent);
        }
    }
    Ok(())
}

type Entry = BTreeMap<&'static str, String>;
/// A comment keyed by its cell, for canonical ordering.
type Located = ((u32, u32), Json);

struct Reader<'a> {
    package: &'a Package,
    limits: &'a Limits,
    cancel: &'a Cancel,
    consumed: BTreeSet<(String, String)>,
    codes: BTreeSet<String>,
    unsupported: BTreeSet<Entry>,
    tables: BTreeSet<String>,
}

/// One `<c>` element before typing.
#[derive(Default)]
struct RawCell {
    row: u32,
    column: u32,
    kind: String,
    style: Option<usize>,
    value: Option<String>,
    inline: Option<String>,
    formula: Option<BTreeMap<String, Json>>,
}

struct Table {
    json: Json,
    top: u32,
    left: u32,
    right: u32,
    header: bool,
}

impl<'a> Reader<'a> {
    fn part(&self, name: &str) -> Option<&'a Part> {
        self.package
            .parts
            .iter()
            .find(|part| part.name.eq_ignore_ascii_case(name))
    }

    fn events(&self, part: &Part) -> Result<Vec<Event>, Refusal> {
        xml::parse(&part.data, self.limits, self.cancel)
    }

    fn relationships(&self, source: &str) -> Vec<&'a Relationship> {
        self.package
            .relationships
            .iter()
            .filter(|r| r.source.eq_ignore_ascii_case(source))
            .collect()
    }

    fn consume(&mut self, relationship: &Relationship) {
        self.consumed.insert((
            relationship.source.to_ascii_lowercase(),
            relationship.id.clone(),
        ));
    }

    fn note(&mut self, entry: Entry) {
        self.codes.insert(format!("unsupported-{}", entry["kind"]));
        self.unsupported.insert(entry);
    }

    #[allow(clippy::type_complexity)]
    fn workbook(&mut self) -> Result<(Json, Json, Vec<Json>), Refusal> {
        let office = self
            .relationships("")
            .into_iter()
            .find(|r| kind(r) == "officeDocument")
            .ok_or(Refusal::MalformedContainer)?;
        self.consume(office);
        let workbook = self
            .part(&office.target)
            .filter(|part| part.content_type.eq_ignore_ascii_case(WORKBOOK_TYPE))
            .ok_or(Refusal::MalformedContainer)?;
        let mut entries: Vec<[String; 4]> = Vec::new();
        let mut defined: Vec<(Vec<(String, String)>, String)> = Vec::new();
        let mut stack: Vec<String> = Vec::new();
        for event in self.events(workbook)? {
            match event {
                Event::Open { name, attributes } => {
                    match (stack.len(), name.as_str()) {
                        (1, "sheets" | "definedNames") => {}
                        (1, other) if SETTINGS.contains(&other) => {
                            self.codes
                                .insert("workbook-settings-not-imported".to_owned());
                        }
                        (1, other) => self.note(Entry::from([
                            ("kind", kebab(other)),
                            ("source", workbook.name.clone()),
                        ])),
                        (2, "sheet") if stack[1] == "sheets" => entries.push([
                            required(&attributes, "name")?.to_owned(),
                            required(&attributes, "sheetId")?.to_owned(),
                            attr(&attributes, "state").unwrap_or("visible").to_owned(),
                            required(&attributes, "id")?.to_owned(),
                        ]),
                        (2, "definedName") if stack[1] == "definedNames" => {
                            defined.push((attributes.clone(), String::new()))
                        }
                        _ => {}
                    }
                    stack.push(name);
                }
                Event::Text(text) => {
                    if stack.len() == 3 && stack[2] == "definedName" {
                        if let Some((_, value)) = defined.last_mut() {
                            value.push_str(&text);
                        }
                    }
                }
                Event::Close => {
                    stack.pop();
                }
            }
        }
        let shared = self.shared_strings(&workbook.name)?;
        let styles = self.styles(&workbook.name)?;
        let mut sheets = Vec::new();
        let mut anchors = Vec::new();
        // Sheet name by `<sheets>` position, for local defined names.
        let mut positions: Vec<Option<(String, u32)>> = Vec::new();
        let (mut names_seen, mut ids_seen) = (BTreeSet::new(), BTreeSet::new());
        for [name, sheet_id, state, id] in entries {
            let sheet_id: u32 = number(&sheet_id).ok_or(Refusal::MalformedContainer)?;
            if !valid_sheet_name(&name)
                || !names_seen.insert(name.to_lowercase())
                || !ids_seen.insert(sheet_id)
                || !["visible", "hidden", "veryHidden"].contains(&state.as_str())
            {
                return Err(Refusal::MalformedContainer);
            }
            let relationship = self
                .relationships(&workbook.name)
                .into_iter()
                .find(|r| r.id == id)
                .ok_or(Refusal::MalformedContainer)?;
            self.consume(relationship);
            if kind(relationship) != "worksheet" {
                self.note(Entry::from([
                    ("kind", kebab(kind(relationship))),
                    ("sheet", name),
                    ("source", workbook.name.clone()),
                    ("target", relationship.target.clone()),
                ]));
                positions.push(None);
                continue;
            }
            let part = self
                .part(&relationship.target)
                .ok_or(Refusal::MalformedContainer)?;
            let (sheet, sheet_anchors) =
                self.worksheet([&name, &state], sheet_id, part, &shared, &styles)?;
            sheets.push(sheet);
            anchors.extend(sheet_anchors);
            positions.push(Some((name, sheet_id)));
        }
        let ids: BTreeMap<String, u32> = positions.iter().flatten().cloned().collect();
        let mut names = BTreeMap::new();
        for (attributes, value) in defined {
            let name = required(&attributes, "name")?.to_owned();
            let scope = match attr(&attributes, "localSheetId") {
                None => None,
                Some(position) => {
                    let position: usize = number(position).ok_or(Refusal::MalformedContainer)?;
                    match positions.get(position).ok_or(Refusal::MalformedContainer)? {
                        Some((sheet, id)) => Some((sheet.clone(), *id)),
                        None => {
                            self.note(Entry::from([
                                ("kind", "defined-name".to_owned()),
                                ("name", name),
                                ("source", workbook.name.clone()),
                            ]));
                            continue;
                        }
                    }
                }
            };
            let key = (name.to_lowercase(), scope.as_ref().map(|(_, id)| *id));
            let quarantined = self.quarantine_formula(&value);
            let mut json = vec![
                ("name", s(&name)),
                ("value", s(&value)),
                ("hidden", flag(attr(&attributes, "hidden") == Some("1"))),
                ("quarantined", flag(quarantined)),
                (
                    "scope",
                    opt(scope.as_ref().map(|(sheet, _)| sheet.as_str())),
                ),
            ];
            if let Some((sheet, row, column)) = simple_reference(&value) {
                if ids.contains_key(&sheet) {
                    let id = match &scope {
                        Some((_, id)) => format!("anc:xlsx:s{id}:name-{}", encode_id(&name)),
                        None => format!("anc:xlsx:name-{}", encode_id(&name)),
                    };
                    anchors.push(anchor(
                        id,
                        "range",
                        &sheet,
                        row,
                        column,
                        format!("{name} = {value}"),
                    ));
                }
            }
            json.sort_by_key(|(key, _)| *key);
            if names.insert(key, obj(json)).is_some() {
                return Err(Refusal::MalformedContainer);
            }
        }
        anchors.retain(|anchor| {
            !str_field(anchor, "text")
                .unwrap_or_default()
                .trim()
                .is_empty()
        });
        anchors.sort_by(|a, b| str_field(a, "id").cmp(&str_field(b, "id")));
        if anchors
            .windows(2)
            .any(|pair| str_field(&pair[0], "id") == str_field(&pair[1], "id"))
        {
            return Err(Refusal::MalformedContainer);
        }
        Ok((
            Json::Arr(sheets),
            Json::Arr(names.into_values().collect()),
            anchors,
        ))
    }

    fn related(&mut self, source: &str, wanted: &str) -> Vec<&'a Relationship> {
        let found: Vec<&Relationship> = self
            .relationships(source)
            .into_iter()
            .filter(|r| kind(r) == wanted && !r.external)
            .collect();
        for relationship in &found {
            self.consume(relationship);
        }
        found
    }

    fn shared_strings(&mut self, workbook: &str) -> Result<Vec<String>, Refusal> {
        let mut strings = Vec::new();
        for relationship in self.related(workbook, "sharedStrings") {
            let part = self
                .part(&relationship.target)
                .ok_or(Refusal::MalformedContainer)?;
            strings = self.rich_texts(part, "si")?;
        }
        Ok(strings)
    }

    /// Text of each `item` element: `<t>` runs joined, phonetic runs skipped.
    fn rich_texts(&mut self, part: &Part, item: &str) -> Result<Vec<String>, Refusal> {
        let mut items = Vec::new();
        let mut stack: Vec<String> = Vec::new();
        for event in self.events(part)? {
            match event {
                Event::Open { name, .. } => {
                    if name == item {
                        items.push(String::new());
                    } else if name == "r" && stack.iter().any(|n| n == item) {
                        self.codes.insert("rich-text-flattened".to_owned());
                    }
                    stack.push(name);
                }
                Event::Text(text) => {
                    if stack.last().is_some_and(|n| n == "t")
                        && stack.iter().any(|n| n == item)
                        && !stack.iter().any(|n| n == "rPh")
                    {
                        if let Some(current) = items.last_mut() {
                            current.push_str(&decode_xstring(&text));
                        }
                    }
                }
                Event::Close => {
                    stack.pop();
                }
            }
        }
        Ok(items)
    }

    /// Number format per `cellXfs` index: `None` for General, an integer for
    /// a built-in format, or the custom format code.
    fn styles(&mut self, workbook: &str) -> Result<Vec<Option<Json>>, Refusal> {
        let Some(relationship) = self.related(workbook, "styles").first().copied() else {
            return Ok(Vec::new());
        };
        self.codes
            .insert("styles-reduced-to-number-formats".to_owned());
        let part = self
            .part(&relationship.target)
            .ok_or(Refusal::MalformedContainer)?;
        let (mut custom, mut formats) = (BTreeMap::new(), Vec::new());
        let mut stack: Vec<String> = Vec::new();
        for event in self.events(part)? {
            match event {
                Event::Open { name, attributes } => {
                    let parent = stack.last().map(String::as_str);
                    if name == "numFmt" && parent == Some("numFmts") {
                        custom.insert(
                            required(&attributes, "numFmtId")?.to_owned(),
                            required(&attributes, "formatCode")?.to_owned(),
                        );
                    } else if name == "xf" && parent == Some("cellXfs") {
                        formats.push(attr(&attributes, "numFmtId").unwrap_or("0").to_owned());
                    }
                    stack.push(name);
                }
                Event::Close => {
                    stack.pop();
                }
                Event::Text(_) => {}
            }
        }
        formats
            .into_iter()
            .map(|id| match (custom.get(&id), number::<i64>(&id)) {
                (Some(code), _) => Ok(Some(s(code))),
                (None, Some(0)) => Ok(None),
                (None, Some(id)) => Ok(Some(Json::Int(id))),
                (None, None) => Err(Refusal::MalformedContainer),
            })
            .collect()
    }

    fn worksheet(
        &mut self,
        [sheet_name, state]: [&str; 2],
        sheet_id: u32,
        part: &Part,
        shared: &[String],
        styles: &[Option<Json>],
    ) -> Result<(Json, Vec<Json>), Refusal> {
        let links: BTreeMap<String, &Relationship> = self
            .relationships(&part.name)
            .into_iter()
            .map(|r| (r.id.clone(), r))
            .collect();
        let mut cells: BTreeMap<(u32, u32), BTreeMap<String, Json>> = BTreeMap::new();
        let (mut merged, mut hyperlinks, mut validations) = (Vec::new(), Vec::new(), Vec::new());
        let mut dimension = None;
        let mut cell: Option<RawCell> = None;
        let (mut row, mut column) = (0u32, 0u32);
        let mut stack: Vec<String> = Vec::new();
        for event in self.events(part)? {
            match event {
                Event::Open { name, attributes } => {
                    let parent = stack.last().map(String::as_str).unwrap_or_default();
                    match (stack.len(), name.as_str()) {
                        (1, "dimension") => {
                            let reference = required(&attributes, "ref")?;
                            range(reference).ok_or(Refusal::MalformedContainer)?;
                            dimension = Some(reference.to_owned());
                        }
                        (
                            1,
                            "sheetData" | "mergeCells" | "dataValidations" | "hyperlinks"
                            | "tableParts" | "drawing" | "legacyDrawing",
                        ) => {}
                        (1, other) if LAYOUT.contains(&other) => {
                            self.codes.insert("layout-not-imported".to_owned());
                        }
                        (1, other) => self.note(Entry::from([
                            ("kind", kebab(other)),
                            ("sheet", sheet_name.to_owned()),
                            ("source", part.name.clone()),
                        ])),
                        (2, "row") if parent == "sheetData" => {
                            row = match attr(&attributes, "r") {
                                Some(r) => number(r).ok_or(Refusal::MalformedContainer)?,
                                None => row + 1,
                            };
                            if row == 0 || row > MAX_ROW {
                                return Err(Refusal::MalformedContainer);
                            }
                            column = 0;
                        }
                        (3, "c") if parent == "row" => {
                            let (r, c) = match attr(&attributes, "r") {
                                Some(r) => cell_ref(r).ok_or(Refusal::MalformedContainer)?,
                                None => (row, column + 1),
                            };
                            if r != row || c > MAX_COLUMN {
                                return Err(Refusal::MalformedContainer);
                            }
                            column = c;
                            cell = Some(RawCell {
                                row,
                                column,
                                kind: attr(&attributes, "t").unwrap_or("n").to_owned(),
                                style: match attr(&attributes, "s") {
                                    Some(style) => {
                                        Some(number(style).ok_or(Refusal::MalformedContainer)?)
                                    }
                                    None => None,
                                },
                                ..RawCell::default()
                            });
                        }
                        (4, "f") if parent == "c" => {
                            let mut formula = BTreeMap::new();
                            if let Some(kind) = attr(&attributes, "t").filter(|k| *k != "normal") {
                                formula.insert("kind".to_owned(), s(kind));
                            }
                            if let Some(reference) = attr(&attributes, "ref") {
                                range(reference).ok_or(Refusal::MalformedContainer)?;
                                formula.insert("ref".to_owned(), s(reference));
                            }
                            if let Some(index) = attr(&attributes, "si") {
                                let index: i64 =
                                    number(index).ok_or(Refusal::MalformedContainer)?;
                                formula.insert("si".to_owned(), Json::Int(index));
                            }
                            formula.insert("text".to_owned(), s(""));
                            if let Some(cell) = cell.as_mut() {
                                cell.formula = Some(formula);
                            }
                        }
                        (4, "is") if parent == "c" => {
                            if let Some(cell) = cell.as_mut() {
                                cell.inline = Some(String::new());
                            }
                        }
                        (_, "r") if stack.iter().any(|n| n == "is") => {
                            self.codes.insert("rich-text-flattened".to_owned());
                        }
                        (2, "mergeCell") if parent == "mergeCells" => {
                            let reference = required(&attributes, "ref")?;
                            range(reference).ok_or(Refusal::MalformedContainer)?;
                            merged.push(s(reference));
                        }
                        (2, "hyperlink") if parent == "hyperlinks" => {
                            let link = self.hyperlink(&attributes, &links)?;
                            hyperlinks.push(link);
                        }
                        (2, "dataValidation") if parent == "dataValidations" => {
                            let sqref = required(&attributes, "sqref")?;
                            if sqref.split_whitespace().any(|r| range(r).is_none()) {
                                return Err(Refusal::MalformedContainer);
                            }
                            let kept: BTreeMap<String, Json> = attributes
                                .iter()
                                .filter(|(key, _)| VALIDATION_ATTRIBUTES.contains(&key.as_str()))
                                .map(|(key, value)| (key.clone(), s(value)))
                                .collect();
                            validations.push(BTreeMap::from([
                                ("attributes".to_owned(), Json::Obj(kept)),
                                ("sqref".to_owned(), s(sqref)),
                            ]));
                        }
                        _ => {}
                    }
                    stack.push(name);
                }
                Event::Text(text) => {
                    let top = stack.last().map(String::as_str).unwrap_or_default();
                    match (top, cell.as_mut()) {
                        ("v", Some(cell)) => cell.value.get_or_insert_default().push_str(&text),
                        ("f", Some(cell)) => {
                            if let Some(Json::Str(formula)) =
                                cell.formula.as_mut().and_then(|f| f.get_mut("text"))
                            {
                                formula.push_str(&text);
                            }
                        }
                        ("t", Some(cell)) if !stack.iter().any(|n| n == "rPh") => {
                            if let Some(inline) = cell.inline.as_mut() {
                                inline.push_str(&decode_xstring(&text));
                            }
                        }
                        ("formula1" | "formula2", None) => {
                            if let Some(validation) = validations.last_mut() {
                                let slot = validation.entry(top.to_owned()).or_insert(s(""));
                                if let Json::Str(value) = slot {
                                    value.push_str(&text);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Event::Close => {
                    if stack.pop().as_deref() == Some("c") && stack.len() == 3 {
                        if let Some(raw) = cell.take() {
                            let key = (raw.row, raw.column);
                            if let Some(json) = self.typed_cell(raw, shared, styles, sheet_name)? {
                                if cells.insert(key, json).is_some() {
                                    return Err(Refusal::MalformedContainer);
                                }
                            }
                        }
                    }
                }
            }
        }
        for validation in &mut validations {
            for key in ["formula1", "formula2"] {
                if let Some(Json::Str(formula)) = validation.get(key) {
                    if self.quarantine_formula(&formula.clone()) {
                        validation.insert("quarantined".to_owned(), Json::Bool(true));
                    }
                }
            }
        }
        let mut comments = Vec::new();
        let comment_parts = self.related(&part.name, "comments");
        for relationship in &comment_parts {
            let target = self
                .part(&relationship.target)
                .ok_or(Refusal::MalformedContainer)?;
            comments.extend(self.comments(target)?);
        }
        if !comment_parts.is_empty() {
            // The legacy VML drawing only positions the comment boxes.
            self.related(&part.name, "vmlDrawing");
        }
        comments.sort_by_key(|(key, _)| *key);
        let mut tables = Vec::new();
        for relationship in self.related(&part.name, "table") {
            let target = self
                .part(&relationship.target)
                .ok_or(Refusal::MalformedContainer)?;
            tables.push(self.table(target)?);
        }
        tables.sort_by_key(|table| (table.top, table.left));
        // ponytail: labels scan every table per cell; a node-scaled work budget
        // keeps hostile table counts bounded. Index tables by column if real
        // workbooks ever hit it.
        if cells.len().saturating_mul(tables.len()) > self.limits.max_xml_nodes.saturating_mul(64) {
            return Err(Refusal::XmlNodeLimit);
        }
        let anchors = label_and_anchor(&mut cells, &tables, sheet_name, sheet_id);
        let sheet = obj(vec![
            (
                "cells",
                Json::Arr(cells.into_values().map(Json::Obj).collect()),
            ),
            (
                "comments",
                Json::Arr(comments.into_iter().map(|(_, json)| json).collect()),
            ),
            (
                "data_validations",
                Json::Arr(validations.into_iter().map(Json::Obj).collect()),
            ),
            ("dimension", opt(dimension.as_deref())),
            ("hyperlinks", Json::Arr(hyperlinks)),
            ("merged_cells", Json::Arr(merged)),
            ("name", s(sheet_name)),
            ("sheet_id", Json::Int(sheet_id.into())),
            ("state", s(state)),
            (
                "tables",
                Json::Arr(tables.into_iter().map(|t| t.json).collect()),
            ),
        ]);
        let mut all = vec![anchor(
            format!("anc:xlsx:s{sheet_id}"),
            "sheet",
            sheet_name,
            1,
            1,
            sheet_name.to_owned(),
        )];
        all.extend(anchors);
        Ok((sheet, all))
    }

    fn typed_cell(
        &mut self,
        raw: RawCell,
        shared: &[String],
        styles: &[Option<Json>],
        sheet: &str,
    ) -> Result<Option<BTreeMap<String, Json>>, Refusal> {
        let text = if raw.kind == "inlineStr" {
            raw.inline
        } else {
            raw.value
        };
        let typed = match text {
            None => None,
            Some(text) => Some(match raw.kind.as_str() {
                "s" => {
                    let index: usize = number(&text).ok_or(Refusal::MalformedContainer)?;
                    typed(
                        "string",
                        s(shared.get(index).ok_or(Refusal::MalformedContainer)?),
                    )
                }
                "inlineStr" => typed("string", s(&text)),
                "str" => typed("string", s(&decode_xstring(&text))),
                "b" => match text.as_str() {
                    "0" | "1" => typed("boolean", Json::Bool(text == "1")),
                    _ => return Err(Refusal::MalformedContainer),
                },
                "e" => typed("error", s(&text)),
                "d" if !text.is_empty() => typed("date", s(&text)),
                "n" if is_number(&text) => typed("number", s(&text)),
                _ => return Err(Refusal::MalformedContainer),
            }),
        };
        let mut cell = BTreeMap::from([
            ("ref".to_owned(), s(&reference(raw.row, raw.column))),
            ("row".to_owned(), Json::Int(raw.row.into())),
            ("column".to_owned(), Json::Int(raw.column.into())),
        ]);
        match raw.style {
            None | Some(0) => {}
            Some(index) => {
                if let Some(format) = styles.get(index).ok_or(Refusal::MalformedContainer)? {
                    cell.insert("number_format".to_owned(), format.clone());
                }
            }
        }
        match (raw.formula, typed) {
            (Some(mut formula), cached) => {
                if formula.get("kind") == Some(&s("dataTable")) {
                    self.note(Entry::from([
                        ("kind", "data-table-formula".to_owned()),
                        ("ref", reference(raw.row, raw.column)),
                        ("sheet", sheet.to_owned()),
                    ]));
                }
                if let Some(Json::Str(text)) = formula.get("text") {
                    if self.quarantine_formula(&text.clone()) {
                        formula.insert("quarantined".to_owned(), Json::Bool(true));
                    }
                }
                cell.insert("formula".to_owned(), Json::Obj(formula));
                if let Some(cached) = cached {
                    self.codes
                        .insert("formula-cached-result-not-recalculated".to_owned());
                    cell.insert("cached".to_owned(), cached);
                }
            }
            (None, Some(value)) => {
                cell.insert("value".to_owned(), value);
            }
            (None, None) => return Ok(None),
        }
        Ok(Some(cell))
    }

    fn quarantine_formula(&mut self, formula: &str) -> bool {
        let active = active_formula(formula);
        if active {
            self.codes.insert("formula-quarantined".to_owned());
        }
        active
    }

    fn hyperlink(
        &mut self,
        attributes: &[(String, String)],
        links: &BTreeMap<String, &Relationship>,
    ) -> Result<Json, Refusal> {
        let reference = required(attributes, "ref")?;
        range(reference).ok_or(Refusal::MalformedContainer)?;
        let target = match attr(attributes, "id").and_then(|id| links.get(id)) {
            Some(relationship) if relationship.external && kind(relationship) == "hyperlink" => {
                self.consume(relationship);
                Some(relationship.target.as_str())
            }
            _ => None,
        };
        let quarantined = target.is_some_and(|target| !safe_link(target));
        if quarantined {
            self.codes.insert("hyperlink-quarantined".to_owned());
        }
        Ok(obj(vec![
            ("display", opt(attr(attributes, "display"))),
            ("location", opt(attr(attributes, "location"))),
            ("quarantined", flag(quarantined)),
            ("ref", s(reference)),
            ("target", opt(target)),
            ("tooltip", opt(attr(attributes, "tooltip"))),
        ]))
    }

    fn comments(&mut self, part: &Part) -> Result<Vec<Located>, Refusal> {
        let texts = self.rich_texts(part, "comment")?;
        let (mut authors, mut refs) = (Vec::new(), Vec::new());
        let mut stack: Vec<String> = Vec::new();
        for event in self.events(part)? {
            match event {
                Event::Open { name, attributes } => {
                    if name == "author" {
                        authors.push(String::new());
                    } else if name == "comment" {
                        let reference = required(&attributes, "ref")?.to_owned();
                        let author: usize = number(required(&attributes, "authorId")?)
                            .ok_or(Refusal::MalformedContainer)?;
                        refs.push((reference, author));
                    }
                    stack.push(name);
                }
                Event::Text(text) => {
                    if stack.last().is_some_and(|n| n == "author") {
                        if let Some(author) = authors.last_mut() {
                            author.push_str(&text);
                        }
                    }
                }
                Event::Close => {
                    stack.pop();
                }
            }
        }
        refs.into_iter()
            .zip(texts)
            .map(|((reference, author), text)| {
                let key = cell_ref(&reference).ok_or(Refusal::MalformedContainer)?;
                let author = authors.get(author).ok_or(Refusal::MalformedContainer)?;
                Ok((
                    key,
                    obj(vec![
                        ("author", s(author)),
                        ("ref", s(&reference)),
                        ("text", s(&text)),
                    ]),
                ))
            })
            .collect()
    }

    fn table(&mut self, part: &Part) -> Result<Table, Refusal> {
        let mut root = Vec::new();
        let mut columns = Vec::new();
        let mut stack: Vec<String> = Vec::new();
        for event in self.events(part)? {
            if let Event::Open { name, attributes } = &event {
                match (stack.len(), name.as_str()) {
                    (0, _) => root = attributes.clone(),
                    (1, "tableColumns") => {}
                    (2, "tableColumn") => columns.push(s(required(attributes, "name")?)),
                    _ => {
                        self.codes.insert("table-details-not-imported".to_owned());
                    }
                }
                stack.push(name.clone());
            } else if event == Event::Close {
                stack.pop();
            }
        }
        let id: u32 = number(required(&root, "id")?).ok_or(Refusal::MalformedContainer)?;
        let name = required(&root, "name")?;
        let display = attr(&root, "displayName").unwrap_or(name);
        let reference = required(&root, "ref")?;
        let ((top, left), (_, right)) = range(reference).ok_or(Refusal::MalformedContainer)?;
        let header = attr(&root, "headerRowCount") != Some("0");
        let totals = attr(&root, "totalsRowCount").is_some_and(|count| count != "0");
        if columns.len() as u32 != right.saturating_sub(left) + 1
            || !self.tables.insert(format!("id:{id}"))
            || !self
                .tables
                .insert(format!("name:{}", display.to_lowercase()))
        {
            return Err(Refusal::MalformedContainer);
        }
        Ok(Table {
            json: obj(vec![
                ("columns", Json::Arr(columns)),
                ("display_name", s(display)),
                ("header_row", Json::Bool(header)),
                ("id", Json::Int(id.into())),
                ("name", s(name)),
                ("ref", s(reference)),
                ("totals_row", Json::Bool(totals)),
            ]),
            top,
            left,
            right,
            header,
        })
    }

    /// Every relationship nothing above consumed becomes an unsupported entry,
    /// content-addressed when it targets a part.
    fn leftovers(&mut self) {
        for relationship in &self.package.relationships {
            let key = (
                relationship.source.to_ascii_lowercase(),
                relationship.id.clone(),
            );
            if self.consumed.contains(&key) || relationship.kind == RECEIPT_TYPE {
                continue;
            }
            if let Some((_, code)) = NOT_IMPORTED.iter().find(|(k, _)| *k == kind(relationship)) {
                self.codes.insert((*code).to_owned());
                continue;
            }
            let mut entry = Entry::from([
                ("kind", kebab(kind(relationship))),
                ("relationship", relationship.kind.clone()),
                ("source", relationship.source.clone()),
                ("target", relationship.target.clone()),
            ]);
            if let Some(part) = self
                .part(&relationship.target)
                .filter(|_| !relationship.external)
            {
                entry.insert("sha256", sha256_hex(&part.data));
            }
            self.note(entry);
        }
    }
}

/// Adds row and column labels to cells and returns their anchors. A column
/// label is the table header above a table cell, otherwise row 1's text; a
/// row label is column A's text.
fn label_and_anchor(
    cells: &mut BTreeMap<(u32, u32), BTreeMap<String, Json>>,
    tables: &[Table],
    sheet: &str,
    sheet_id: u32,
) -> Vec<Json> {
    let strings: BTreeMap<(u32, u32), String> = cells
        .iter()
        .filter_map(|(key, cell)| match cell.get("value") {
            Some(value) if str_field(value, "type") == Some("string") => {
                Some((*key, str_field(value, "value")?.to_owned()))
            }
            _ => None,
        })
        .filter(|(_, text)| !text.trim().is_empty())
        .collect();
    let mut anchors = Vec::new();
    for (&(row, column), cell) in cells.iter_mut() {
        let table = tables
            .iter()
            .find(|t| t.header && (t.left..=t.right).contains(&column) && row >= t.top);
        let header_row = table.map_or(1, |t| t.top);
        let mut labels = BTreeMap::new();
        if row > header_row {
            if let Some(label) = strings.get(&(header_row, column)) {
                labels.insert("column".to_owned(), s(label));
            }
        }
        if column > 1 {
            if let Some(label) = strings.get(&(row, 1)) {
                labels.insert("row".to_owned(), s(label));
            }
        }
        if !labels.is_empty() {
            cell.insert("labels".to_owned(), Json::Obj(labels));
        }
        let role = if table.is_some_and(|t| t.top == row) {
            "header"
        } else {
            "cell"
        };
        anchors.push(anchor(
            format!("anc:xlsx:s{sheet_id}:r{row}c{column}"),
            role,
            sheet,
            row,
            column,
            cell_text(cell),
        ));
    }
    for table in tables {
        let columns: Vec<&str> = match get(&table.json, "columns") {
            Some(Json::Arr(columns)) => columns
                .iter()
                .filter_map(|c| match c {
                    Json::Str(c) => Some(c.as_str()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        anchors.push(anchor(
            format!(
                "anc:xlsx:s{sheet_id}:table-{}",
                match get(&table.json, "id") {
                    Some(Json::Int(id)) => *id,
                    _ => 0,
                }
            ),
            "table",
            sheet,
            table.top,
            table.left,
            format!(
                "{} {}: {}",
                str_field(&table.json, "name").unwrap_or_default(),
                str_field(&table.json, "ref").unwrap_or_default(),
                columns.join(", ")
            ),
        ));
    }
    anchors
}

/// The citable text of a cell. A formula cell shows its formula and, when
/// present, its cached result labelled as cached: BRAN never recalculates.
fn cell_text(cell: &BTreeMap<String, Json>) -> String {
    let display = |value: &Json| match get(value, "value") {
        Some(Json::Bool(true)) => "TRUE".to_owned(),
        Some(Json::Bool(false)) => "FALSE".to_owned(),
        Some(Json::Str(text)) => text.clone(),
        _ => String::new(),
    };
    match cell.get("formula") {
        None => cell.get("value").map(display).unwrap_or_default(),
        Some(formula) => {
            let text = match (str_field(formula, "text"), get(formula, "si")) {
                (Some(""), Some(Json::Int(index))) => format!("=(shared formula {index})"),
                (text, _) => format!("={}", text.unwrap_or_default()),
            };
            match cell.get("cached") {
                Some(cached) => format!("{text} [cached: {}]", display(cached)),
                None => text,
            }
        }
    }
}

fn anchor(id: String, role: &str, sheet: &str, row: u32, column: u32, text: String) -> Json {
    obj(vec![
        ("family", s("grid")),
        ("id", Json::Str(id)),
        (
            "locator",
            obj(vec![
                ("column", Json::Int(column.into())),
                ("family", s("grid")),
                ("row", Json::Int(row.into())),
                ("sheet", s(sheet)),
            ]),
        ),
        ("role", s(role)),
        ("text_digest", s(&sha256_hex(text.as_bytes()))),
        ("text", Json::Str(text)),
    ])
}

/// Grid-family fidelity keys from the evidence envelope. Formulas are never
/// `exact`: their text is kept but nothing is recalculated.
fn fidelity(codes: &BTreeSet<String>) -> Json {
    let level = |code: &str| {
        if codes.contains(code) {
            "normalized"
        } else {
            "exact"
        }
    };
    obj(vec![
        ("cells", s(level("styles-reduced-to-number-formats"))),
        ("charts", s("unsupported")),
        ("formulas", s("normalized")),
        ("macros", s("unsupported")),
        ("sheets", s("exact")),
        ("text", s(level("rich-text-flattened"))),
    ])
}

// ---- Formula and link safety ----

/// True when a formula would reach outside the workbook if a spreadsheet
/// application calculated it: DDE (`|`), an external workbook reference, or
/// a function that fetches, calls, or registers code. String literals are
/// ignored. Nothing here evaluates the formula.
pub fn active_formula(formula: &str) -> bool {
    let mut outside = String::new();
    let mut literal = false;
    for c in formula.chars() {
        if c == '"' {
            literal = !literal;
            outside.push(' ');
        } else if !literal {
            outside.push(c.to_ascii_uppercase());
        }
    }
    if outside.contains('|') || outside.contains(":\\") || outside.contains("\\\\") {
        return true;
    }
    // `[1]Sheet!A1` and `'[Book.xlsx]Sheet'!A1` name another workbook; a
    // structured table reference like `Table1[Value]` is never followed by `!`.
    for (start, _) in outside.match_indices('[') {
        let Some(end) = outside[start..].find(']') else {
            continue;
        };
        let inside = &outside[start + 1..start + end];
        let after = &outside[start + end + 1..];
        // `]Sheet!`: a sheet name, not an operator, sits between `]` and `!`.
        let sheet = after.split_once('!').map(|(sheet, _)| sheet);
        if (!inside.is_empty() && inside.bytes().all(|b| b.is_ascii_digit()))
            || inside.contains(".XL")
            || sheet.is_some_and(|sheet| {
                !sheet.is_empty() && !sheet.contains(|c| "&+-*/^=<>(),;:{}[]".contains(c))
            })
        {
            return true;
        }
    }
    ACTIVE_FUNCTIONS.iter().any(|function| {
        outside.match_indices(function).any(|(at, _)| {
            outside[..at]
                .chars()
                .next_back()
                .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'))
        })
    })
}

fn safe_link(target: &str) -> bool {
    let lower = target.to_ascii_lowercase();
    ["http://", "https://", "mailto:"]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
}

// ---- Export ----

/// Exports a canonical grid projection as a deterministic XLSX package with
/// its fidelity receipt. DLP and the public boundary run on every string
/// first; active formulas and links are refused, and text cells are always
/// written as strings, never as formulas.
pub fn export(imported: &Imported) -> Result<Exported, Refusal> {
    if imported.receipt.iter().any(|c| c == "dlp-findings") {
        return Err(Refusal::DlpFindings);
    }
    if imported
        .receipt
        .iter()
        .any(|c| c == "public-boundary-violation")
    {
        return Err(Refusal::PublicBoundary);
    }
    let projection =
        parse_canonical(&imported.canonical).map_err(|_| Refusal::ExportUnsupported)?;
    if str_field(&projection, "schema_version") != Some(PROJECTION) {
        return Err(Refusal::ExportUnsupported);
    }
    let sheets = list(&projection, "sheets")?;
    let names = list(&projection, "defined_names")?;
    let mut strings = Vec::new();
    collect_strings(
        get(&projection, "sheets").unwrap_or(&Json::Null),
        &mut strings,
    );
    collect_strings(
        get(&projection, "defined_names").unwrap_or(&Json::Null),
        &mut strings,
    );
    for text in strings {
        match validate_emitted_string(text) {
            Err(ExportError::DlpViolation(_)) => return Err(Refusal::DlpFindings),
            Err(_) => return Err(Refusal::PublicBoundary),
            Ok(()) => {}
        }
    }
    let receipt = obj(vec![
        ("format", s("xlsx")),
        ("formulas_recalculated", Json::Bool(false)),
        (
            "fidelity",
            get(&projection, "fidelity").cloned().unwrap_or(Json::Null),
        ),
        (
            "import_receipt",
            Json::Arr(imported.receipt.iter().map(|c| s(c)).collect()),
        ),
        (
            "normalized",
            Json::Arr(
                [
                    "part-names-and-relationship-ids-renumbered",
                    "shared-strings-written-inline",
                    "styles-reduced-to-number-formats",
                ]
                .map(s)
                .to_vec(),
            ),
        ),
        (
            "omitted",
            get(&projection, "unsupported")
                .cloned()
                .unwrap_or(Json::Arr(Vec::new())),
        ),
        ("projection", s(PROJECTION)),
        ("receipt_version", s(RECEIPT_VERSION)),
        (
            "source_canonical_sha256",
            s(&sha256_hex(&imported.canonical)),
        ),
    ])
    .to_bytes();
    let parts = Writer::default().workbook(sheets, names, &receipt)?;
    let entries: Vec<zip::WriteEntry<'_>> = parts
        .iter()
        .map(|(name, data)| zip::WriteEntry {
            name,
            data,
            deflate: true,
            dos_time: 0,
            dos_date: 0x0021, // 1980-01-01, the earliest DOS date: no clock is read
        })
        .collect();
    Ok(Exported {
        bytes: zip::write(&entries),
        receipt,
    })
}

#[derive(Default)]
struct Writer {
    parts: BTreeMap<String, Vec<u8>>,
    overrides: Vec<(String, &'static str)>,
    formats: BTreeMap<NumberFormat, usize>,
    tables: usize,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum NumberFormat {
    Builtin(i64),
    Custom(String),
}

impl Writer {
    fn workbook(
        mut self,
        sheets: &[Json],
        names: &[Json],
        receipt: &[u8],
    ) -> Result<BTreeMap<String, Vec<u8>>, Refusal> {
        if sheets.is_empty() {
            return Err(Refusal::ExportUnsupported);
        }
        for sheet in sheets {
            for cell in list(sheet, "cells")? {
                match get(cell, "number_format") {
                    None => {}
                    Some(Json::Int(id)) if (1..164).contains(id) => {
                        self.formats.insert(NumberFormat::Builtin(*id), 0);
                    }
                    Some(Json::Str(code)) => {
                        self.formats.insert(NumberFormat::Custom(code.clone()), 0);
                    }
                    Some(_) => return Err(Refusal::ExportUnsupported),
                }
            }
        }
        for (index, value) in self.formats.values_mut().enumerate() {
            *value = index + 1;
        }
        let mut positions = BTreeMap::new();
        let (mut sheet_xml, mut rels) = (String::new(), String::new());
        let mut ids = BTreeSet::new();
        for (index, sheet) in sheets.iter().enumerate() {
            let number = index + 1;
            let name = text(sheet, "name")?;
            let sheet_id = int(sheet, "sheet_id")?;
            let state = text(sheet, "state")?;
            if !valid_sheet_name(name)
                || positions.insert(name.to_lowercase(), index).is_some()
                || !ids.insert(sheet_id)
                || !(1..=i64::from(u32::MAX)).contains(&sheet_id)
                || !["visible", "hidden", "veryHidden"].contains(&state)
            {
                return Err(Refusal::ExportUnsupported);
            }
            let state = if state == "visible" {
                String::new()
            } else {
                format!(" state=\"{state}\"")
            };
            sheet_xml.push_str(&format!(
                "<sheet name=\"{}\" sheetId=\"{sheet_id}\"{state} r:id=\"rId{number}\"/>",
                attribute(name)?
            ));
            rels.push_str(&relationship(
                &format!("rId{number}"),
                &format!("{OFFICE_REL}/worksheet"),
                &format!("worksheets/sheet{number}.xml"),
                false,
            )?);
            self.worksheet(sheet, number)?;
        }
        let mut defined = String::new();
        for name in names {
            let value = text(name, "value")?;
            if active_formula(value) {
                return Err(Refusal::ActiveContent);
            }
            let scope = match opt_text(name, "scope")? {
                None => String::new(),
                Some(sheet) => format!(
                    " localSheetId=\"{}\"",
                    positions
                        .get(&sheet.to_lowercase())
                        .ok_or(Refusal::ExportUnsupported)?
                ),
            };
            let hidden = if get(name, "hidden") == Some(&Json::Bool(true)) {
                " hidden=\"1\""
            } else {
                ""
            };
            defined.push_str(&format!(
                "<definedName name=\"{}\"{scope}{hidden}>{}</definedName>",
                attribute(text(name, "name")?)?,
                escape(value)?
            ));
        }
        if !defined.is_empty() {
            defined = format!("<definedNames>{defined}</definedNames>");
        }
        let styles = sheets.len() + 1;
        rels.push_str(&relationship(
            &format!("rId{styles}"),
            &format!("{OFFICE_REL}/styles"),
            "styles.xml",
            false,
        )?);
        self.put(
            "xl/workbook.xml",
            format!(
                "<workbook xmlns=\"{MAIN}\" xmlns:r=\"{OFFICE_REL}\"><sheets>{sheet_xml}</sheets>{defined}</workbook>"
            ),
            Some(WORKBOOK_TYPE),
        );
        self.put("xl/_rels/workbook.xml.rels", rels_part(&rels), None);
        let styles_xml = self.styles()?;
        self.put(
            "xl/styles.xml",
            styles_xml,
            Some("application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"),
        );
        let package_rels = relationship(
            "rId1",
            &format!("{OFFICE_REL}/officeDocument"),
            "xl/workbook.xml",
            false,
        )? + &relationship("rId2", RECEIPT_TYPE, RECEIPT_PART, false)?;
        self.put("_rels/.rels", rels_part(&package_rels), None);
        self.parts.insert(RECEIPT_PART.to_owned(), receipt.to_vec());
        let overrides: String = self
            .overrides
            .iter()
            .map(|(name, content_type)| {
                format!("<Override PartName=\"/{name}\" ContentType=\"{content_type}\"/>")
            })
            .collect();
        self.put(
            "[Content_Types].xml",
            format!(
                "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
                 <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
                 <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
                 <Default Extension=\"vml\" ContentType=\"application/vnd.openxmlformats-officedocument.vmlDrawing\"/>\
                 <Default Extension=\"json\" ContentType=\"application/json\"/>{overrides}</Types>"
            ),
            None,
        );
        Ok(self.parts)
    }

    fn put(&mut self, name: &str, xml: String, content_type: Option<&'static str>) {
        let body = format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n{xml}");
        self.parts.insert(name.to_owned(), body.into_bytes());
        if let Some(content_type) = content_type {
            self.overrides.push((name.to_owned(), content_type));
        }
    }

    fn worksheet(&mut self, sheet: &Json, number: usize) -> Result<(), Refusal> {
        let mut rows: BTreeMap<u32, BTreeMap<u32, String>> = BTreeMap::new();
        for cell in list(sheet, "cells")? {
            let (row, column) = (int(cell, "row")?, int(cell, "column")?);
            let (Ok(row), Ok(column)) = (u32::try_from(row), u32::try_from(column)) else {
                return Err(Refusal::ExportUnsupported);
            };
            if !(1..=MAX_ROW).contains(&row) || !(1..=MAX_COLUMN).contains(&column) {
                return Err(Refusal::ExportUnsupported);
            }
            let xml = self.cell(cell, &reference(row, column))?;
            if rows.entry(row).or_default().insert(column, xml).is_some() {
                return Err(Refusal::ExportUnsupported);
            }
        }
        let mut xml = format!("<worksheet xmlns=\"{MAIN}\" xmlns:r=\"{OFFICE_REL}\">");
        if let Some(dimension) = opt_text(sheet, "dimension")? {
            range(dimension).ok_or(Refusal::ExportUnsupported)?;
            xml.push_str(&format!("<dimension ref=\"{dimension}\"/>"));
        }
        xml.push_str("<sheetData>");
        for (row, cells) in rows {
            xml.push_str(&format!("<row r=\"{row}\">"));
            xml.extend(cells.into_values());
            xml.push_str("</row>");
        }
        xml.push_str("</sheetData>");
        let merged = list(sheet, "merged_cells")?;
        if !merged.is_empty() {
            xml.push_str(&format!("<mergeCells count=\"{}\">", merged.len()));
            for merge in merged {
                let Json::Str(merge) = merge else {
                    return Err(Refusal::ExportUnsupported);
                };
                range(merge).ok_or(Refusal::ExportUnsupported)?;
                xml.push_str(&format!("<mergeCell ref=\"{merge}\"/>"));
            }
            xml.push_str("</mergeCells>");
        }
        let validations = list(sheet, "data_validations")?;
        if !validations.is_empty() {
            xml.push_str(&format!(
                "<dataValidations count=\"{}\">",
                validations.len()
            ));
            for validation in validations {
                xml.push_str(&data_validation(validation)?);
            }
            xml.push_str("</dataValidations>");
        }
        let mut rels = String::new();
        let mut next = 0;
        let mut rel_id = || {
            next += 1;
            format!("rId{next}")
        };
        let hyperlinks = list(sheet, "hyperlinks")?;
        if !hyperlinks.is_empty() {
            xml.push_str("<hyperlinks>");
            for link in hyperlinks {
                let reference = text(link, "ref")?;
                range(reference).ok_or(Refusal::ExportUnsupported)?;
                let mut element = format!("<hyperlink ref=\"{reference}\"");
                if let Some(target) = opt_text(link, "target")? {
                    if !safe_link(target) {
                        return Err(Refusal::ActiveContent);
                    }
                    let id = rel_id();
                    rels.push_str(&relationship(
                        &id,
                        &format!("{OFFICE_REL}/hyperlink"),
                        target,
                        true,
                    )?);
                    element.push_str(&format!(" r:id=\"{id}\""));
                }
                for key in ["location", "display", "tooltip"] {
                    if let Some(value) = opt_text(link, key)? {
                        element.push_str(&format!(" {key}=\"{}\"", attribute(value)?));
                    }
                }
                xml.push_str(&element);
                xml.push_str("/>");
            }
            xml.push_str("</hyperlinks>");
        }
        let comments = list(sheet, "comments")?;
        if !comments.is_empty() {
            let (comments_xml, vml) = comment_parts(comments, number)?;
            let (comments_name, vml_name) = (
                format!("xl/comments{number}.xml"),
                format!("xl/drawings/vmlDrawing{number}.vml"),
            );
            self.put(
                &comments_name,
                comments_xml,
                Some("application/vnd.openxmlformats-officedocument.spreadsheetml.comments+xml"),
            );
            self.parts.insert(vml_name, vml.into_bytes());
            rels.push_str(&relationship(
                &rel_id(),
                &format!("{OFFICE_REL}/comments"),
                &format!("../comments{number}.xml"),
                false,
            )?);
            let id = rel_id();
            rels.push_str(&relationship(
                &id,
                &format!("{OFFICE_REL}/vmlDrawing"),
                &format!("../drawings/vmlDrawing{number}.vml"),
                false,
            )?);
            xml.push_str(&format!("<legacyDrawing r:id=\"{id}\"/>"));
        }
        let tables = list(sheet, "tables")?;
        if !tables.is_empty() {
            xml.push_str(&format!("<tableParts count=\"{}\">", tables.len()));
            for table in tables {
                self.tables += 1;
                let name = format!("xl/tables/table{}.xml", self.tables);
                self.put(
                    &name,
                    table_xml(table)?,
                    Some("application/vnd.openxmlformats-officedocument.spreadsheetml.table+xml"),
                );
                let id = rel_id();
                rels.push_str(&relationship(
                    &id,
                    &format!("{OFFICE_REL}/table"),
                    &format!("../tables/table{}.xml", self.tables),
                    false,
                )?);
                xml.push_str(&format!("<tablePart r:id=\"{id}\"/>"));
            }
            xml.push_str("</tableParts>");
        }
        xml.push_str("</worksheet>");
        self.put(
            &format!("xl/worksheets/sheet{number}.xml"),
            xml,
            Some("application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"),
        );
        if !rels.is_empty() {
            self.put(
                &format!("xl/worksheets/_rels/sheet{number}.xml.rels"),
                rels_part(&rels),
                None,
            );
        }
        Ok(())
    }

    fn cell(&self, cell: &Json, reference: &str) -> Result<String, Refusal> {
        let style = match get(cell, "number_format") {
            None => String::new(),
            Some(Json::Int(id)) => {
                format!(" s=\"{}\"", self.format_index(&NumberFormat::Builtin(*id))?)
            }
            Some(Json::Str(code)) => {
                format!(
                    " s=\"{}\"",
                    self.format_index(&NumberFormat::Custom(code.clone()))?
                )
            }
            Some(_) => return Err(Refusal::ExportUnsupported),
        };
        if let Some(formula) = get(cell, "formula") {
            let formula_text = text(formula, "text")?;
            if active_formula(formula_text) || get(formula, "quarantined").is_some() {
                return Err(Refusal::ActiveContent);
            }
            let mut attributes = String::new();
            match opt_text(formula, "kind")? {
                None => {}
                Some(kind @ ("shared" | "array")) => attributes.push_str(&format!(" t=\"{kind}\"")),
                Some(_) => return Err(Refusal::ExportUnsupported),
            }
            if let Some(reference) = opt_text(formula, "ref")? {
                range(reference).ok_or(Refusal::ExportUnsupported)?;
                attributes.push_str(&format!(" ref=\"{reference}\""));
            }
            match get(formula, "si") {
                None => {}
                Some(Json::Int(index)) if *index >= 0 => {
                    attributes.push_str(&format!(" si=\"{index}\""))
                }
                Some(_) => return Err(Refusal::ExportUnsupported),
            }
            let formula_xml = if formula_text.is_empty() {
                format!("<f{attributes}/>")
            } else {
                format!("<f{attributes}>{}</f>", escape(formula_text)?)
            };
            let (kind, cached) = match get(cell, "cached") {
                None => (String::new(), String::new()),
                Some(cached) => {
                    let (kind, value) = value_xml(cached, "str")?;
                    (kind, format!("<v>{value}</v>"))
                }
            };
            return Ok(format!(
                "<c r=\"{reference}\"{style}{kind}>{formula_xml}{cached}</c>"
            ));
        }
        let value = get(cell, "value").ok_or(Refusal::ExportUnsupported)?;
        if str_field(value, "type") == Some("string") {
            return Ok(format!(
                "<c r=\"{reference}\"{style} t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
                escape(&encode_xstring(text(value, "value")?))?
            ));
        }
        let (kind, value) = value_xml(value, "str")?;
        Ok(format!(
            "<c r=\"{reference}\"{style}{kind}><v>{value}</v></c>"
        ))
    }

    fn format_index(&self, format: &NumberFormat) -> Result<usize, Refusal> {
        self.formats
            .get(format)
            .copied()
            .ok_or(Refusal::ExportUnsupported)
    }

    fn styles(&self) -> Result<String, Refusal> {
        let (mut custom, mut xfs) = (String::new(), String::new());
        let mut next_custom = 164;
        for format in self.formats.keys() {
            let id = match format {
                NumberFormat::Builtin(id) => *id,
                NumberFormat::Custom(code) => {
                    custom.push_str(&format!(
                        "<numFmt numFmtId=\"{next_custom}\" formatCode=\"{}\"/>",
                        attribute(code)?
                    ));
                    next_custom += 1;
                    next_custom - 1
                }
            };
            xfs.push_str(&format!(
                "<xf numFmtId=\"{id}\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\" applyNumberFormat=\"1\"/>"
            ));
        }
        let custom = if custom.is_empty() {
            custom
        } else {
            format!(
                "<numFmts count=\"{}\">{custom}</numFmts>",
                next_custom - 164
            )
        };
        Ok(format!(
            "<styleSheet xmlns=\"{MAIN}\">{custom}\
             <fonts count=\"1\"><font><sz val=\"11\"/><name val=\"Calibri\"/></font></fonts>\
             <fills count=\"2\"><fill><patternFill patternType=\"none\"/></fill><fill><patternFill patternType=\"gray125\"/></fill></fills>\
             <borders count=\"1\"><border><left/><right/><top/><bottom/><diagonal/></border></borders>\
             <cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs>\
             <cellXfs count=\"{}\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/>{xfs}</cellXfs>\
             <cellStyles count=\"1\"><cellStyle name=\"Normal\" xfId=\"0\" builtinId=\"0\"/></cellStyles></styleSheet>",
            self.formats.len() + 1
        ))
    }
}

/// `t` attribute and `<v>` text of a typed value. `string_kind` is `str` for
/// a formula's cached string.
fn value_xml(value: &Json, string_kind: &str) -> Result<(String, String), Refusal> {
    let kind = |t: &str| format!(" t=\"{t}\"");
    match (str_field(value, "type"), get(value, "value")) {
        (Some("number"), Some(Json::Str(number))) if is_number(number) => {
            Ok((String::new(), number.clone()))
        }
        (Some("string"), Some(Json::Str(text))) => {
            Ok((kind(string_kind), escape(&encode_xstring(text))?))
        }
        (Some("boolean"), Some(Json::Bool(value))) => {
            Ok((kind("b"), if *value { "1" } else { "0" }.to_owned()))
        }
        (Some("error"), Some(Json::Str(text))) => Ok((kind("e"), escape(text)?)),
        (Some("date"), Some(Json::Str(text))) if !text.is_empty() => Ok((kind("d"), escape(text)?)),
        _ => Err(Refusal::ExportUnsupported),
    }
}

fn data_validation(validation: &Json) -> Result<String, Refusal> {
    let sqref = text(validation, "sqref")?;
    if sqref.split_whitespace().any(|r| range(r).is_none()) {
        return Err(Refusal::ExportUnsupported);
    }
    let mut xml = String::from("<dataValidation");
    match get(validation, "attributes") {
        Some(Json::Obj(attributes)) => {
            for (key, value) in attributes {
                let Json::Str(value) = value else {
                    return Err(Refusal::ExportUnsupported);
                };
                if !VALIDATION_ATTRIBUTES.contains(&key.as_str()) {
                    return Err(Refusal::ExportUnsupported);
                }
                xml.push_str(&format!(" {key}=\"{}\"", attribute(value)?));
            }
        }
        _ => return Err(Refusal::ExportUnsupported),
    }
    xml.push_str(&format!(" sqref=\"{sqref}\">"));
    for key in ["formula1", "formula2"] {
        if let Some(formula) = opt_text(validation, key)? {
            if active_formula(formula) {
                return Err(Refusal::ActiveContent);
            }
            xml.push_str(&format!("<{key}>{}</{key}>", escape(formula)?));
        }
    }
    xml.push_str("</dataValidation>");
    Ok(xml)
}

fn comment_parts(comments: &[Json], number: usize) -> Result<(String, String), Refusal> {
    let mut authors: Vec<&str> = comments
        .iter()
        .map(|comment| text(comment, "author"))
        .collect::<Result<_, _>>()?;
    authors.sort_unstable();
    authors.dedup();
    let mut list = String::new();
    let mut shapes = String::new();
    for (index, comment) in comments.iter().enumerate() {
        let reference = text(comment, "ref")?;
        let (row, column) = cell_ref(reference).ok_or(Refusal::ExportUnsupported)?;
        let author = authors
            .binary_search(&text(comment, "author")?)
            .map_err(|_| Refusal::ExportUnsupported)?;
        list.push_str(&format!(
            "<comment ref=\"{reference}\" authorId=\"{author}\"><text><t xml:space=\"preserve\">{}</t></text></comment>",
            escape(&encode_xstring(text(comment, "text")?))?
        ));
        shapes.push_str(&format!(
            "<v:shape id=\"_x0000_s{}\" type=\"#_x0000_t202\" style=\"position:absolute;margin-left:59.25pt;margin-top:1.5pt;width:108pt;height:59.25pt;z-index:1;visibility:hidden\" fillcolor=\"#ffffe1\" o:insetmode=\"auto\">\
             <v:fill color2=\"#ffffe1\"/><v:shadow on=\"t\" color=\"black\" obscured=\"t\"/><v:path o:connecttype=\"none\"/>\
             <v:textbox style=\"mso-direction-alt:auto\"><div style=\"text-align:left\"></div></v:textbox>\
             <x:ClientData ObjectType=\"Note\"><x:MoveWithCells/><x:SizeWithCells/><x:AutoFill>False</x:AutoFill><x:Row>{}</x:Row><x:Column>{}</x:Column></x:ClientData></v:shape>",
            1024 * number + index + 1,
            row - 1,
            column - 1
        ));
    }
    let authors: String = authors
        .iter()
        .map(|author| Ok(format!("<author>{}</author>", escape(author)?)))
        .collect::<Result<_, Refusal>>()?;
    let comments = format!(
        "<comments xmlns=\"{MAIN}\"><authors>{authors}</authors><commentList>{list}</commentList></comments>"
    );
    let vml = format!(
        "<xml xmlns:v=\"urn:schemas-microsoft-com:vml\" xmlns:o=\"urn:schemas-microsoft-com:office:office\" xmlns:x=\"urn:schemas-microsoft-com:office:excel\">\
         <o:shapelayout v:ext=\"edit\"><o:idmap v:ext=\"edit\" data=\"{number}\"/></o:shapelayout>\
         <v:shapetype id=\"_x0000_t202\" coordsize=\"21600,21600\" o:spt=\"202\" path=\"m,l,21600r21600,l21600,xe\"><v:stroke joinstyle=\"miter\"/><v:path gradientshapeok=\"t\" o:connecttype=\"rect\"/></v:shapetype>{shapes}</xml>"
    );
    Ok((comments, vml))
}

fn table_xml(table: &Json) -> Result<String, Refusal> {
    let id = int(table, "id")?;
    let reference = text(table, "ref")?;
    let ((_, left), (_, right)) = range(reference).ok_or(Refusal::ExportUnsupported)?;
    let columns = list(table, "columns")?;
    if !(1..=i64::from(u32::MAX)).contains(&id) || columns.len() as u32 != right - left + 1 {
        return Err(Refusal::ExportUnsupported);
    }
    let mut xml = format!(
        "<table xmlns=\"{MAIN}\" id=\"{id}\" name=\"{}\" displayName=\"{}\" ref=\"{reference}\"",
        attribute(text(table, "name")?)?,
        attribute(text(table, "display_name")?)?
    );
    if get(table, "header_row") == Some(&Json::Bool(false)) {
        xml.push_str(" headerRowCount=\"0\"");
    }
    if get(table, "totals_row") == Some(&Json::Bool(true)) {
        xml.push_str(" totalsRowCount=\"1\"");
    }
    xml.push_str(&format!("><tableColumns count=\"{}\">", columns.len()));
    for (index, column) in columns.iter().enumerate() {
        let Json::Str(column) = column else {
            return Err(Refusal::ExportUnsupported);
        };
        xml.push_str(&format!(
            "<tableColumn id=\"{}\" name=\"{}\"/>",
            index + 1,
            attribute(column)?
        ));
    }
    xml.push_str("</tableColumns></table>");
    Ok(xml)
}

fn relationship(id: &str, kind: &str, target: &str, external: bool) -> Result<String, Refusal> {
    let mode = if external {
        " TargetMode=\"External\""
    } else {
        ""
    };
    Ok(format!(
        "<Relationship Id=\"{id}\" Type=\"{kind}\" Target=\"{}\"{mode}/>",
        attribute(target)?
    ))
}

fn rels_part(relationships: &str) -> String {
    format!("<Relationships xmlns=\"{PACKAGE_REL}\">{relationships}</Relationships>")
}

// ---- Text encoding ----

/// Escapes element text. XML 1.0 cannot carry other C0 controls, so text
/// that still has one after `encode_xstring` is refused.
fn escape(value: &str) -> Result<String, Refusal> {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {
                return Err(Refusal::ExportUnsupported)
            }
            c => out.push(c),
        }
    }
    Ok(out)
}

/// Escapes an attribute value; whitespace controls become character
/// references so attribute normalization cannot change them.
fn attribute(value: &str) -> Result<String, Refusal> {
    Ok(escape(value)?
        .replace('"', "&quot;")
        .replace('\t', "&#9;")
        .replace('\n', "&#10;")
        .replace('\r', "&#13;"))
}

/// OOXML `ST_Xstring` escaping: controls become `_xHHHH_`, and a literal
/// `_xHHHH_` keeps its meaning by escaping its underscore.
fn encode_xstring(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for (at, c) in value.char_indices() {
        if c == '_' && xstring_escape_at(&value[at..]).is_some() {
            out.push_str("_x005F_");
        } else if ((c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r'))
            || c == '\u{FFFE}'
            || c == '\u{FFFF}'
        {
            out.push_str(&format!("_x{:04X}_", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

fn decode_xstring(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(at) = rest.find("_x") {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        match xstring_escape_at(rest) {
            Some(c) => {
                out.push(c);
                rest = &rest[7..];
            }
            None => {
                out.push_str("_x");
                rest = &rest[2..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn xstring_escape_at(value: &str) -> Option<char> {
    let hex = value.strip_prefix("_x")?.get(..5)?.strip_suffix('_')?;
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    char::from_u32(u32::from_str_radix(hex, 16).ok()?)
}

// ---- References ----

/// `A1` to (row, column), inside Excel's grid, no leading zeros.
fn cell_ref(value: &str) -> Option<(u32, u32)> {
    let split = value.find(|c: char| !c.is_ascii_uppercase())?;
    let (letters, digits) = value.split_at(split);
    if letters.is_empty() || letters.len() > 3 || digits.starts_with('0') {
        return None;
    }
    let row: u32 = number(digits)?;
    let column = letters
        .bytes()
        .fold(0u32, |acc, b| acc * 26 + u32::from(b - b'A' + 1));
    ((1..=MAX_ROW).contains(&row) && (1..=MAX_COLUMN).contains(&column)).then_some((row, column))
}

/// `A1` or `A1:B2` to its two corners.
#[allow(clippy::type_complexity)]
fn range(value: &str) -> Option<((u32, u32), (u32, u32))> {
    let mut corners = value.split(':');
    let first = cell_ref(corners.next()?)?;
    let second = match corners.next() {
        Some(corner) => cell_ref(corner)?,
        None => first,
    };
    corners.next().is_none().then_some((first, second))
}

fn reference(row: u32, column: u32) -> String {
    let mut letters = Vec::new();
    let mut rest = column;
    while rest > 0 {
        rest -= 1;
        letters.push(b'A' + (rest % 26) as u8);
        rest /= 26;
    }
    letters.reverse();
    format!("{}{row}", String::from_utf8(letters).unwrap_or_default())
}

/// `Sheet!$A$1` or `'My Sheet'!$A$1:$B$2` to the sheet and first cell.
fn simple_reference(value: &str) -> Option<(String, u32, u32)> {
    let (sheet, cells) = value.rsplit_once('!')?;
    let sheet = match sheet.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
        Some(quoted) => quoted.replace("''", "'"),
        None => sheet.to_owned(),
    };
    let ((row, column), _) = range(&cells.replace('$', ""))?;
    Some((sheet, row, column))
}

fn valid_sheet_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= 31
        && !name.contains(['[', ']', ':', '*', '?', '/', '\\'])
        && !name.starts_with('\'')
        && !name.ends_with('\'')
        && !name.chars().any(char::is_control)
}

/// Anchor ids allow `[A-Za-z0-9./:_-]`; every other byte, and `_` itself,
/// becomes `_HH`, so distinct names get distinct ids.
fn encode_id(name: &str) -> String {
    name.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-' => char::from(b).to_string(),
            b => format!("_{b:02X}"),
        })
        .collect()
}

// ---- Small helpers ----

fn is_number(value: &str) -> bool {
    !value.starts_with('+') && value.parse::<f64>().is_ok_and(f64::is_finite)
}

fn number<T: std::str::FromStr>(value: &str) -> Option<T> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

fn kind(relationship: &Relationship) -> &str {
    relationship.kind.rsplit('/').next().unwrap_or_default()
}

/// `conditionalFormatting` to `conditional-formatting`.
fn kebab(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_uppercase() && !out.is_empty() {
            out.push('-');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

fn attr<'b>(attributes: &'b [(String, String)], key: &str) -> Option<&'b str> {
    attributes
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

fn required<'b>(attributes: &'b [(String, String)], key: &str) -> Result<&'b str, Refusal> {
    attr(attributes, key).ok_or(Refusal::MalformedContainer)
}

fn s(value: &str) -> Json {
    Json::Str(value.to_owned())
}

fn opt(value: Option<&str>) -> Json {
    value.map_or(Json::Null, s)
}

fn flag(value: bool) -> Json {
    if value {
        Json::Bool(true)
    } else {
        Json::Null
    }
}

fn typed(kind: &str, value: Json) -> Json {
    obj(vec![("type", s(kind)), ("value", value)])
}

/// An object without its `Null` fields: optional fields are omitted.
fn obj(pairs: Vec<(&str, Json)>) -> Json {
    Json::Obj(
        pairs
            .into_iter()
            .filter(|(_, value)| *value != Json::Null)
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

fn get<'b>(json: &'b Json, key: &str) -> Option<&'b Json> {
    match json {
        Json::Obj(map) => map.get(key),
        _ => None,
    }
}

fn str_field<'b>(json: &'b Json, key: &str) -> Option<&'b str> {
    match get(json, key) {
        Some(Json::Str(value)) => Some(value),
        _ => None,
    }
}

fn text<'b>(json: &'b Json, key: &str) -> Result<&'b str, Refusal> {
    str_field(json, key).ok_or(Refusal::ExportUnsupported)
}

fn opt_text<'b>(json: &'b Json, key: &str) -> Result<Option<&'b str>, Refusal> {
    match get(json, key) {
        None => Ok(None),
        Some(Json::Str(value)) => Ok(Some(value)),
        Some(_) => Err(Refusal::ExportUnsupported),
    }
}

fn int(json: &Json, key: &str) -> Result<i64, Refusal> {
    match get(json, key) {
        Some(Json::Int(value)) => Ok(*value),
        _ => Err(Refusal::ExportUnsupported),
    }
}

fn list<'b>(json: &'b Json, key: &str) -> Result<&'b [Json], Refusal> {
    match get(json, key) {
        Some(Json::Arr(items)) => Ok(items),
        _ => Err(Refusal::ExportUnsupported),
    }
}

fn collect_strings<'b>(json: &'b Json, out: &mut Vec<&'b str>) {
    match json {
        Json::Str(value) => out.push(value),
        Json::Arr(items) => items.iter().for_each(|item| collect_strings(item, out)),
        Json::Obj(map) => map.iter().for_each(|(key, item)| {
            out.push(key);
            collect_strings(item, out);
        }),
        _ => {}
    }
}

// ---- Canonical JSON reader ----

/// Reads canonical JSON bytes (as written by `canonical::Json`) back into a
/// value. Strict: no whitespace, integers only, no duplicate keys, bounded
/// nesting, so a hostile projection costs bounded work.
pub fn parse_canonical(bytes: &[u8]) -> Result<Json, Refusal> {
    let mut reader = JsonReader { bytes, at: 0 };
    let value = reader.value(0)?;
    if reader.at != bytes.len() {
        return Err(Refusal::MalformedContainer);
    }
    Ok(value)
}

struct JsonReader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl JsonReader<'_> {
    fn peek(&self) -> Result<u8, Refusal> {
        self.bytes
            .get(self.at)
            .copied()
            .ok_or(Refusal::MalformedContainer)
    }

    fn eat(&mut self, byte: u8) -> bool {
        let found = self.bytes.get(self.at) == Some(&byte);
        self.at += usize::from(found);
        found
    }

    fn expect(&mut self, byte: u8) -> Result<(), Refusal> {
        self.eat(byte)
            .then_some(())
            .ok_or(Refusal::MalformedContainer)
    }

    fn literal(&mut self, word: &[u8], value: Json) -> Result<Json, Refusal> {
        if self.bytes[self.at..].starts_with(word) {
            self.at += word.len();
            Ok(value)
        } else {
            Err(Refusal::MalformedContainer)
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, Refusal> {
        if depth > 32 {
            return Err(Refusal::MalformedContainer);
        }
        match self.peek()? {
            b'{' => {
                self.at += 1;
                let mut map = BTreeMap::new();
                if self.eat(b'}') {
                    return Ok(Json::Obj(map));
                }
                loop {
                    let key = self.string()?;
                    self.expect(b':')?;
                    let value = self.value(depth + 1)?;
                    if map.insert(key, value).is_some() {
                        return Err(Refusal::MalformedContainer);
                    }
                    if !self.eat(b',') {
                        self.expect(b'}')?;
                        return Ok(Json::Obj(map));
                    }
                }
            }
            b'[' => {
                self.at += 1;
                let mut items = Vec::new();
                if self.eat(b']') {
                    return Ok(Json::Arr(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    if !self.eat(b',') {
                        self.expect(b']')?;
                        return Ok(Json::Arr(items));
                    }
                }
            }
            b'"' => Ok(Json::Str(self.string()?)),
            b't' => self.literal(b"true", Json::Bool(true)),
            b'f' => self.literal(b"false", Json::Bool(false)),
            b'n' => self.literal(b"null", Json::Null),
            b'-' | b'0'..=b'9' => {
                let start = self.at;
                self.eat(b'-');
                while self.peek().is_ok_and(|b| b.is_ascii_digit()) {
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
        self.expect(b'"')?;
        let mut out = Vec::new();
        loop {
            let byte = self.peek()?;
            self.at += 1;
            match byte {
                b'"' => return String::from_utf8(out).map_err(|_| Refusal::MalformedContainer),
                b'\\' => {
                    let escaped = self.peek()?;
                    self.at += 1;
                    let c = match escaped {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let hex = self
                                .bytes
                                .get(self.at..self.at + 4)
                                .and_then(|hex| std::str::from_utf8(hex).ok())
                                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                                .ok_or(Refusal::MalformedContainer)?;
                            self.at += 4;
                            char::from_u32(hex).ok_or(Refusal::MalformedContainer)?
                        }
                        _ => return Err(Refusal::MalformedContainer),
                    };
                    out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
                }
                byte if byte < 0x20 => return Err(Refusal::MalformedContainer),
                byte => out.push(byte),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_round_trip_at_the_grid_edges() {
        for (text, row, column) in [
            ("A1", 1, 1),
            ("Z9", 9, 26),
            ("AA10", 10, 27),
            ("XFD1048576", MAX_ROW, MAX_COLUMN),
        ] {
            assert_eq!(cell_ref(text), Some((row, column)), "{text}");
            assert_eq!(reference(row, column), text);
        }
        for bad in ["", "A", "1", "A0", "A01", "XFE1", "A1048577", "a1", "A1B"] {
            assert_eq!(cell_ref(bad), None, "{bad}");
        }
        assert_eq!(
            simple_reference("'It''s'!$B$2:$C$3"),
            Some(("It's".to_owned(), 2, 2))
        );
    }

    #[test]
    fn xstring_escapes_round_trip() {
        for text in [
            "plain",
            "_x0041_ literal",
            "bell\u{7}",
            "tab\tline\n",
            "_x_",
            "_x00e9_",
        ] {
            assert_eq!(decode_xstring(&encode_xstring(text)), text, "{text:?}");
        }
        assert_eq!(decode_xstring("_x0041_B"), "AB");
    }

    #[test]
    fn canonical_json_reads_back_what_it_writes() {
        let value = obj(vec![
            ("a", Json::Arr(vec![Json::Int(-3), Json::Bool(false)])),
            ("b", s("quote\" slash\\ nl\n \u{1} \u{e9}")),
            ("c", Json::Obj(BTreeMap::new())),
        ]);
        assert_eq!(parse_canonical(&value.to_bytes()), Ok(value));
        for bad in [
            &b"{\"a\":1,\"a\":2}"[..],
            b"[1,]",
            b"\"\x01\"",
            b"01x",
            b" 1",
        ] {
            assert!(parse_canonical(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn active_formula_ignores_string_literals_and_table_references() {
        assert!(!active_formula("SUM(Table1[Value])"));
        assert!(!active_formula("Table1[[#Headers],[Value]]"));
        assert!(!active_formula("\"a|b\"&WEBSERVICEX(1)"));
        assert!(!active_formula("Sheet2!A1+MYRTD(1)"));
        assert!(!active_formula("Table1[Value]&Sheet2!A1"));
        assert!(!active_formula("SUM(Table1[@Value],Sheet2!A1)"));
        assert!(active_formula("cmd|' /C calc'!A0"));
        assert!(active_formula("[1]Sheet1!A1"));
        assert!(active_formula("'C:\\dir\\[b.xlsx]S'!A1"));
        assert!(active_formula("_xlfn.WEBSERVICE(A1)"));
        assert!(active_formula("register.id(\"x\")"));
    }
}
