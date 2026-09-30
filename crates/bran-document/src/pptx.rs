//! PPTX adapter (issue #22).
//!
//! Import runs the shared OPC intake first, so package safety, budgets, active
//! content parts, and external references are decided once for every format.
//! This module then maps PresentationML into canonical JSON: slide order and
//! identifiers, sections, layout names, shapes in reading order, text, tables,
//! notes, comments, alt text, links, images as content-addressed assets, and
//! envelope-shaped citation anchors. Export projects that content into a new
//! deck on a generic master, layouts, and theme, with a fidelity receipt part.
//! Nothing is fetched, run, or played.

use crate::canonical::{sha256_hex, Json};
use crate::conformance::{Adapter, Anchor, Imported};
use crate::opc::{self, Part, Relationship};
use crate::xml::{self, Event};
use crate::zip::{self, WriteEntry};
use crate::{Cancel, Format, Limits, Refusal, RECEIPT_VERSION};
use bran_core::export::{validate_emitted_string, ExportError};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// Version tag of the canonical content this adapter emits and exports from.
pub const SCHEMA: &str = "bran.pptx.content/1";
/// Package part and package relationship type of an export's fidelity receipt.
pub const RECEIPT_PART: &str = "bran/fidelity-receipt.json";
pub const RECEIPT_RELATIONSHIP: &str =
    "https://schemas.alphazede.dev/bran/relationships/fidelity-receipt";
const EXPORT_RECEIPT_SCHEMA: &str = "bran.pptx.export-receipt/1";

/// Group nesting and canonical JSON depth are bounded so no walk can exhaust
/// the stack, whatever XML depth limit the caller chose.
const MAX_NESTING: usize = 64;
const MAX_JSON_DEPTH: usize = 256;
/// Canonical content that does not match this module's model.
const BAD: Refusal = Refusal::MalformedContainer;

const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const PML: &str = "application/vnd.openxmlformats-officedocument.presentationml";
const MAIN_TYPES: [&str; 3] = [
    "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml",
    "application/vnd.openxmlformats-officedocument.presentationml.slideshow.main+xml",
    "application/vnd.openxmlformats-officedocument.presentationml.template.main+xml",
];
const TABLE_URI: &str = "http://schemas.openxmlformats.org/drawingml/2006/table";
/// Relationship types whose targets are embedded or executable objects.
const EMBEDDED: [&str; 3] = ["/package", "/oleObject", "/control"];
/// Click actions that start a program, macro, or OLE verb.
const RUNNING_ACTIONS: [&str; 3] = ["ppaction://program", "ppaction://macro", "ppaction://ole"];
/// Presentation fidelity per envelope feature (#20). It states what this
/// adapter can carry, not what one deck contained.
const FIDELITY: [(&str, &str); 7] = [
    ("animations", "unsupported"),
    ("macros", "unsupported"),
    ("shapes", "normalized"),
    ("slides", "exact"),
    ("speaker_notes", "normalized"),
    ("text", "normalized"),
    ("z_order", "exact"),
];

/// The PPTX adapter registered with the conformance harness.
pub struct Pptx;

impl Adapter for Pptx {
    fn format(&self) -> Format {
        Format::Pptx
    }

    fn import(&self, bytes: &[u8], limits: &Limits, cancel: &Cancel) -> Result<Imported, Refusal> {
        import(bytes, limits, cancel)
    }

    fn export(&self, imported: &Imported) -> Result<Vec<u8>, Refusal> {
        export(imported)
    }
}

// ---- Import ----

/// Imports an untrusted deck. Every refusal is typed; nothing partial returns.
pub fn import(bytes: &[u8], limits: &Limits, cancel: &Cancel) -> Result<Imported, Refusal> {
    let package = opc::open(bytes, limits, cancel)?;
    if package
        .relationships
        .iter()
        .any(|r| EMBEDDED.iter().any(|kind| r.kind.ends_with(kind)))
    {
        return Err(Refusal::ActiveContent);
    }
    let mut deck = Deck {
        parts: package
            .parts
            .iter()
            .map(|part| (part.name.to_ascii_lowercase(), part))
            .collect(),
        relationships: package
            .relationships
            .iter()
            .map(|r| ((r.source.to_ascii_lowercase(), r.id.clone()), r))
            .collect(),
        limits,
        cancel,
        codes: BTreeSet::new(),
        used: BTreeSet::new(),
        assets: BTreeMap::new(),
        slide_ids: BTreeMap::new(),
        layouts: BTreeMap::new(),
        legacy_authors: BTreeMap::new(),
        modern_authors: BTreeMap::new(),
        anchors: Vec::new(),
        slide: (0, 0),
        z: 0,
        shape_ids: BTreeSet::new(),
    };
    let content = deck.content()?;
    if deck
        .parts
        .keys()
        .any(|name| !name.ends_with(".rels") && !deck.used.contains(name))
    {
        deck.codes.insert("unmapped-parts-omitted");
    }
    let canonical = content.to_bytes();
    // The intake's byte scan cannot see a canary split across runs; the
    // canonical content carries each paragraph joined.
    match validate_emitted_string(&String::from_utf8_lossy(&canonical)) {
        Err(ExportError::DlpViolation(_)) => deck.codes.insert("dlp-findings"),
        Err(_) => deck.codes.insert("public-boundary-violation"),
        Ok(()) => false,
    };
    let receipt: BTreeSet<&str> = package.diagnostics.union(&deck.codes).copied().collect();
    Ok(Imported {
        canonical,
        receipt: receipt.into_iter().map(str::to_owned).collect(),
        anchors: deck
            .anchors
            .iter()
            .map(|(id, text, _)| Anchor {
                id: id.clone(),
                text_digest: sha256_hex(text.as_bytes()),
            })
            .collect(),
    })
}

/// Parses canonical JSON written by this module (content or export receipt).
pub fn parse_canonical(bytes: &[u8]) -> Result<Json, Refusal> {
    let text = std::str::from_utf8(bytes).map_err(|_| BAD)?;
    let mut parser = JsonParser { text, at: 0 };
    let value = parser.value(0)?;
    if parser.at != text.len() {
        return Err(BAD);
    }
    Ok(value)
}

struct Node {
    name: String,
    attributes: Vec<(String, String)>,
    children: Vec<usize>,
    text: String,
    end: usize,
}

/// Arena tree over the shared reader's events. Nodes are in document order
/// and `end` closes each subtree, so nothing here recurses to build or drop.
struct Dom(Vec<Node>);

impl Dom {
    fn parse(data: &[u8], limits: &Limits, cancel: &Cancel) -> Result<Self, Refusal> {
        let mut nodes: Vec<Node> = Vec::new();
        let mut open: Vec<usize> = Vec::new();
        for event in xml::parse(data, limits, cancel)? {
            match event {
                Event::Open { name, attributes } => {
                    let index = nodes.len();
                    if let Some(&parent) = open.last() {
                        nodes[parent].children.push(index);
                    }
                    nodes.push(Node {
                        name,
                        attributes,
                        children: Vec::new(),
                        text: String::new(),
                        end: index + 1,
                    });
                    open.push(index);
                }
                Event::Close => {
                    if let Some(index) = open.pop() {
                        nodes[index].end = nodes.len();
                    }
                }
                Event::Text(text) => {
                    if let Some(&index) = open.last() {
                        nodes[index].text.push_str(&text);
                    }
                }
            }
        }
        Ok(Self(nodes))
    }

    fn name(&self, node: usize) -> &str {
        &self.0[node].name
    }

    fn text(&self, node: usize) -> &str {
        &self.0[node].text
    }

    fn attr(&self, node: usize, key: &str) -> Option<&str> {
        self.0[node]
            .attributes
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    fn flag(&self, node: usize, key: &str) -> bool {
        matches!(self.attr(node, key), Some("1" | "true"))
    }

    fn kids(&self, node: usize) -> impl Iterator<Item = usize> + '_ {
        self.0[node].children.iter().copied()
    }

    fn named<'a>(&'a self, node: usize, name: &'a str) -> impl Iterator<Item = usize> + 'a {
        self.kids(node).filter(move |&kid| self.name(kid) == name)
    }

    fn child(&self, node: usize, name: &str) -> Option<usize> {
        self.named(node, name).next()
    }

    fn path(&self, node: usize, names: &[&str]) -> Option<usize> {
        names.iter().try_fold(node, |at, name| self.child(at, name))
    }

    fn num(&self, node: usize, key: &str) -> Result<i64, Refusal> {
        self.attr(node, key)
            .and_then(|value| value.parse().ok())
            .ok_or(Refusal::MalformedXml)
    }
}

type Authors = BTreeMap<String, (String, Option<String>)>;

/// Where a shape sits: its anchor key, locator shape id, and z-order index.
struct Placed {
    key: String,
    shape: i64,
    z: i64,
}

struct Deck<'a> {
    parts: BTreeMap<String, &'a Part>,
    relationships: BTreeMap<(String, String), &'a Relationship>,
    limits: &'a Limits,
    cancel: &'a Cancel,
    codes: BTreeSet<&'static str>,
    used: BTreeSet<String>,
    assets: BTreeMap<String, &'a Part>,
    slide_ids: BTreeMap<String, i64>,
    layouts: BTreeMap<String, Option<String>>,
    legacy_authors: Authors,
    modern_authors: Authors,
    /// (anchor id, text, envelope-shaped anchor)
    anchors: Vec<(String, String, Json)>,
    /// (slide number, slide id) of the slide being read.
    slide: (i64, i64),
    z: i64,
    shape_ids: BTreeSet<i64>,
}

impl<'a> Deck<'a> {
    fn part(&mut self, name: &str) -> Option<&'a Part> {
        let key = name.to_ascii_lowercase();
        let part = self.parts.get(&key).copied();
        if part.is_some() {
            self.used.insert(key);
        }
        part
    }

    fn dom(&self, part: &Part) -> Result<Dom, Refusal> {
        self.cancel.check()?;
        Dom::parse(&part.data, self.limits, self.cancel)
    }

    fn relationship(&self, source: &str, id: &str) -> Option<&'a Relationship> {
        self.relationships
            .get(&(source.to_ascii_lowercase(), id.to_owned()))
            .copied()
    }

    fn related(&self, source: &str, suffix: &str) -> Vec<&'a Relationship> {
        let source = source.to_ascii_lowercase();
        self.relationships
            .range((source.clone(), String::new())..)
            .take_while(|((from, _), _)| *from == source)
            .filter(|(_, r)| r.kind.ends_with(suffix))
            .map(|(_, r)| *r)
            .collect()
    }

    fn content(&mut self) -> Result<Json, Refusal> {
        let main = self
            .related("", "/officeDocument")
            .first()
            .map(|r| r.target.clone())
            .ok_or(Refusal::MalformedContainer)?;
        let part = self.part(&main).ok_or(Refusal::MalformedContainer)?;
        if !MAIN_TYPES.contains(&part.content_type.as_str()) {
            return Err(Refusal::UnsupportedContainer);
        }
        let dom = self.dom(part)?;
        if dom.name(0) != "presentation" {
            return Err(Refusal::MalformedXml);
        }
        let slides = self.slide_list(&dom, &main)?;
        let size = match dom.child(0, "sldSz") {
            Some(size) => obj([
                ("cx", int(dom.num(size, "cx")?)),
                ("cy", int(dom.num(size, "cy")?)),
            ]),
            None => Json::Null,
        };
        let mut sections = Vec::new();
        for ext in dom
            .child(0, "extLst")
            .into_iter()
            .flat_map(|l| dom.named(l, "ext"))
        {
            for section in dom
                .child(ext, "sectionLst")
                .into_iter()
                .flat_map(|list| dom.named(list, "section"))
            {
                let mut ids = Vec::new();
                for entry in dom
                    .child(section, "sldIdLst")
                    .into_iter()
                    .flat_map(|list| dom.named(list, "sldId"))
                {
                    ids.push(int(dom.num(entry, "id")?));
                }
                sections.push(obj([
                    ("id", opt(dom.attr(section, "id"))),
                    ("name", s(dom.attr(section, "name").unwrap_or_default())),
                    ("slides", Json::Arr(ids)),
                ]));
            }
        }
        self.legacy_authors = self.authors(&main, "/commentAuthors", "cmAuthor")?;
        self.modern_authors = self.authors(&main, "/authors", "author")?;
        let mut out = Vec::with_capacity(slides.len());
        for (index, (id, name)) in slides.iter().enumerate() {
            out.push(self.slide(index as i64 + 1, *id, name)?);
        }
        let assets = self
            .assets
            .iter()
            .map(|(digest, part)| {
                obj([
                    ("byte_length", int(part.data.len() as i64)),
                    ("data_hex", s(&hex(&part.data))),
                    ("id", s(&asset_id(digest))),
                    ("media_type", s(&part.content_type)),
                    ("sha256", s(digest)),
                ])
            })
            .collect();
        self.anchors.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(obj([
            (
                "anchors",
                Json::Arr(self.anchors.iter().map(|(_, _, a)| a.clone()).collect()),
            ),
            ("assets", Json::Arr(assets)),
            ("family", s("presentation")),
            ("fidelity", fidelity()),
            ("schema", s(SCHEMA)),
            ("sections", Json::Arr(sections)),
            ("slide_size", size),
            ("slides", Json::Arr(out)),
        ]))
    }

    /// Slides in `sldIdLst` order: (slide id, part name).
    fn slide_list(&mut self, dom: &Dom, main: &str) -> Result<Vec<(i64, String)>, Refusal> {
        let mut slides = Vec::new();
        let (mut parts, mut ids) = (BTreeSet::new(), BTreeSet::new());
        for entry in dom
            .child(0, "sldIdLst")
            .into_iter()
            .flat_map(|list| dom.named(list, "sldId"))
        {
            // `id` and `r:id` share a local name. A slide id is numeric; a
            // relationship id is an xsd:ID, which never starts with a digit.
            let (mut id, mut rid) = (None, None);
            for (key, value) in &dom.0[entry].attributes {
                if key == "id" && value.starts_with(|c: char| c.is_ascii_digit()) {
                    id = Some(value.as_str());
                } else if key == "id" {
                    rid = Some(value.as_str());
                }
            }
            let id = id
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|value| (256..=2_147_483_647).contains(value))
                .ok_or(Refusal::MalformedXml)?;
            let target = rid
                .and_then(|rid| self.relationship(main, rid))
                .filter(|r| !r.external && r.kind.ends_with("/slide"))
                .map(|r| r.target.clone())
                .ok_or(Refusal::MalformedContainer)?;
            let key = target.to_ascii_lowercase();
            let is_slide = self
                .parts
                .get(&key)
                .is_some_and(|part| part.content_type == format!("{PML}.slide+xml"));
            if !is_slide || !parts.insert(key.clone()) || !ids.insert(id) {
                return Err(Refusal::MalformedContainer);
            }
            self.slide_ids.insert(key, i64::from(id));
            slides.push((i64::from(id), target));
        }
        Ok(slides)
    }

    fn authors(&mut self, main: &str, suffix: &str, element: &str) -> Result<Authors, Refusal> {
        let mut authors = BTreeMap::new();
        for relationship in self.related(main, suffix) {
            let Some(part) = self.part(&relationship.target) else {
                continue;
            };
            let dom = self.dom(part)?;
            for author in dom.named(0, element) {
                if let Some(id) = dom.attr(author, "id") {
                    let name = dom.attr(author, "name").unwrap_or_default().to_owned();
                    let initials = dom.attr(author, "initials").map(str::to_owned);
                    authors.insert(id.to_owned(), (name, initials));
                }
            }
        }
        Ok(authors)
    }

    fn slide(&mut self, number: i64, id: i64, name: &str) -> Result<Json, Refusal> {
        let part = self.part(name).ok_or(Refusal::MalformedContainer)?;
        let dom = self.dom(part)?;
        if dom.name(0) != "sld" {
            return Err(Refusal::MalformedXml);
        }
        self.scan(&dom)?;
        self.slide = (number, id);
        self.z = 0;
        self.shape_ids.clear();
        let layout = self.layout(name)?;
        let mut shapes = Vec::new();
        if let Some(tree) = dom.path(0, &["cSld", "spTree"]) {
            self.shapes(&dom, name, tree, 0, &mut shapes)?;
        }
        let notes = self.notes(name)?;
        let comments = self.comments(name)?;
        Ok(obj([
            ("comments", Json::Arr(comments)),
            (
                "hidden",
                Json::Bool(matches!(dom.attr(0, "show"), Some("0" | "false"))),
            ),
            ("id", int(id)),
            ("layout", opt(layout.as_deref())),
            (
                "name",
                opt(dom.child(0, "cSld").and_then(|c| dom.attr(c, "name"))),
            ),
            ("notes", notes),
            ("shapes", Json::Arr(shapes)),
        ]))
    }

    /// Refuses running actions and embedded objects anywhere in a part and
    /// receipts content this adapter does not carry.
    fn scan(&mut self, dom: &Dom) -> Result<(), Refusal> {
        for node in 0..dom.0.len() {
            let code = match dom.name(node) {
                "oleObj" | "control" => return Err(Refusal::ActiveContent),
                "hlinkClick" => {
                    check_action(dom.attr(node, "action"))?;
                    continue;
                }
                "hlinkHover" | "hlinkMouseOver" => {
                    check_action(dom.attr(node, "action"))?;
                    "hover-action-omitted"
                }
                "transition" => "unsupported-transition",
                "timing" => "unsupported-animation",
                "videoFile" | "audioFile" | "quickTimeFile" | "wavAudioFile" | "media" | "snd" => {
                    "unsupported-media"
                }
                "contentPart" => "unsupported-content-part",
                _ => continue,
            };
            self.codes.insert(code);
        }
        Ok(())
    }

    fn layout(&mut self, slide: &str) -> Result<Option<String>, Refusal> {
        let Some(relationship) = self.related(slide, "/slideLayout").first().copied() else {
            return Ok(None);
        };
        self.codes.insert("layout-design-normalized");
        let key = relationship.target.to_ascii_lowercase();
        if let Some(name) = self.layouts.get(&key) {
            return Ok(name.clone());
        }
        let name = match self.part(&relationship.target) {
            Some(part) => {
                let dom = self.dom(part)?;
                self.scan(&dom)?;
                dom.child(0, "cSld")
                    .and_then(|c| dom.attr(c, "name"))
                    .map(str::to_owned)
            }
            None => None,
        };
        self.layouts.insert(key, name.clone());
        Ok(name)
    }

    fn shapes(
        &mut self,
        dom: &Dom,
        source: &str,
        parent: usize,
        depth: usize,
        out: &mut Vec<Json>,
    ) -> Result<(), Refusal> {
        if depth > MAX_NESTING {
            return Err(Refusal::XmlDepthLimit);
        }
        for node in dom.kids(parent) {
            let shape = match dom.name(node) {
                "sp" => {
                    let (mut shape, placed) = self.common(dom, source, node, "shape")?;
                    let text_box = dom
                        .path(node, &["nvSpPr", "cNvSpPr"])
                        .is_some_and(|c| dom.flag(c, "txBox"));
                    let (paragraphs, text) = match dom.child(node, "txBody") {
                        Some(body) => self.paragraphs(dom, source, body)?,
                        None => (Json::Null, String::new()),
                    };
                    let role = match dom
                        .path(node, &["nvSpPr", "nvPr", "ph"])
                        .and_then(|ph| dom.attr(ph, "type"))
                    {
                        Some("title" | "ctrTitle") => "title",
                        Some("subTitle") => "heading",
                        _ => "paragraph",
                    };
                    self.anchor(&placed, "", role, text);
                    shape.insert("text_box".to_owned(), Json::Bool(text_box));
                    shape.insert("paragraphs".to_owned(), paragraphs);
                    shape
                }
                "cxnSp" => self.common(dom, source, node, "connector")?.0,
                "pic" => {
                    let (mut shape, _) = self.common(dom, source, node, "picture")?;
                    let image = match dom
                        .path(node, &["blipFill", "blip"])
                        .and_then(|blip| dom.attr(blip, "embed"))
                    {
                        Some(rid) => self.asset(source, rid)?,
                        None => Json::Null,
                    };
                    shape.insert("image".to_owned(), image);
                    shape
                }
                "grpSp" => {
                    let (mut shape, _) = self.common(dom, source, node, "group")?;
                    let mut children = Vec::new();
                    self.shapes(dom, source, node, depth + 1, &mut children)?;
                    shape.insert("children".to_owned(), Json::Arr(children));
                    shape
                }
                "graphicFrame" => match self.frame(dom, source, node)? {
                    Some(shape) => shape,
                    None => continue,
                },
                "AlternateContent" => {
                    match dom.child(node, "Fallback") {
                        Some(fallback) => {
                            self.codes.insert("alternate-content-fallback");
                            self.shapes(dom, source, fallback, depth + 1, out)?;
                        }
                        None => {
                            self.codes.insert("alternate-content-omitted");
                        }
                    }
                    continue;
                }
                _ => continue,
            };
            out.push(Json::Obj(shape));
        }
        Ok(())
    }

    /// Identity, name, accessibility metadata, placeholder, link, and geometry
    /// shared by every shape kind. Also places the shape in z-order.
    fn common(
        &mut self,
        dom: &Dom,
        source: &str,
        node: usize,
        kind: &str,
    ) -> Result<(BTreeMap<String, Json>, Placed), Refusal> {
        self.codes.insert("shape-styling-normalized");
        let z = self.z;
        self.z += 1;
        let nv = dom.kids(node).find(|&kid| dom.name(kid).starts_with("nv"));
        let c = nv.and_then(|nv| dom.child(nv, "cNvPr"));
        let id = match c.and_then(|c| dom.attr(c, "id")) {
            Some(value) => Some(i64::from(
                value.parse::<u32>().map_err(|_| Refusal::MalformedXml)?,
            )),
            None => {
                self.codes.insert("shape-identity-missing");
                None
            }
        };
        let key = match id {
            Some(id) if self.shape_ids.insert(id) => format!("shape-{id}"),
            Some(id) => {
                self.codes.insert("duplicate-shape-id");
                format!("shape-{id}-z{z}")
            }
            None => format!("shape-z{z}"),
        };
        let placed = Placed {
            key,
            shape: id.unwrap_or(0),
            z,
        };
        let attr = |key: &str| c.and_then(|c| dom.attr(c, key));
        let link = self.link(dom, source, c.and_then(|c| dom.child(c, "hlinkClick")))?;
        let placeholder = match nv.and_then(|nv| dom.path(nv, &["nvPr", "ph"])) {
            Some(ph) => obj([
                ("idx", opt(dom.attr(ph, "idx"))),
                ("type", opt(dom.attr(ph, "type"))),
            ]),
            None => Json::Null,
        };
        let props = dom
            .child(node, "spPr")
            .or_else(|| dom.child(node, "grpSpPr"));
        let frame = dom
            .child(node, "xfrm")
            .or_else(|| props.and_then(|p| dom.child(p, "xfrm")));
        let preset = props
            .and_then(|p| dom.child(p, "prstGeom"))
            .and_then(|g| dom.attr(g, "prst"));
        if let Some(alt) = attr("descr").filter(|alt| !alt.trim().is_empty()) {
            self.anchor(&placed, ":alt", "shape", alt.to_owned());
        }
        let shape = BTreeMap::from([
            ("alt_text".to_owned(), opt(attr("descr"))),
            ("alt_title".to_owned(), opt(attr("title"))),
            (
                "hidden".to_owned(),
                Json::Bool(c.is_some_and(|c| dom.flag(c, "hidden"))),
            ),
            ("id".to_owned(), id.map_or(Json::Null, int)),
            ("kind".to_owned(), s(kind)),
            ("link".to_owned(), link),
            ("name".to_owned(), s(attr("name").unwrap_or_default())),
            ("placeholder".to_owned(), placeholder),
            ("preset".to_owned(), opt(preset)),
            ("xfrm".to_owned(), xfrm(dom, frame)?),
        ]);
        Ok((shape, placed))
    }

    fn anchor(&mut self, placed: &Placed, suffix: &str, role: &str, text: String) {
        if text.trim().is_empty() {
            return;
        }
        let (number, slide) = self.slide;
        let id = format!("anc:pptx:slide-{slide}:{}{suffix}", placed.key);
        let locator = obj([
            ("family", s("presentation")),
            ("shape", int(placed.shape)),
            ("slide", int(number)),
            ("z_index", int(placed.z)),
        ]);
        let anchor = obj([
            ("family", s("presentation")),
            ("id", s(&id)),
            ("locator", locator),
            ("role", s(role)),
            ("text", s(&text)),
            ("text_digest", s(&sha256_hex(text.as_bytes()))),
        ]);
        self.anchors.push((id, text, anchor));
    }

    /// Paragraphs of a text body, and their text joined for the anchor.
    fn paragraphs(
        &mut self,
        dom: &Dom,
        source: &str,
        body: usize,
    ) -> Result<(Json, String), Refusal> {
        let mut paragraphs = Vec::new();
        let mut lines = Vec::new();
        for paragraph in dom.named(body, "p") {
            let level = match dom.child(paragraph, "pPr").and_then(|p| dom.attr(p, "lvl")) {
                Some(value) => value.parse::<i64>().map_err(|_| Refusal::MalformedXml)?,
                None => 0,
            };
            // Text-level alternate content (equations) is read from its fallback.
            let mut items = Vec::new();
            for item in dom.kids(paragraph) {
                if dom.name(item) != "AlternateContent" {
                    items.push(item);
                } else if let Some(fallback) = dom.child(item, "Fallback") {
                    self.codes.insert("alternate-content-fallback");
                    items.extend(dom.kids(fallback));
                } else {
                    self.codes.insert("alternate-content-omitted");
                }
            }
            let (mut runs, mut line) = (Vec::new(), String::new());
            for item in items {
                let text = match dom.name(item) {
                    "r" | "fld" => {
                        if dom.name(item) == "fld" {
                            self.codes.insert("field-as-text");
                        }
                        self.codes.insert("text-formatting-normalized");
                        dom.child(item, "t").map_or("", |t| dom.text(t))
                    }
                    "br" => "\n",
                    _ => continue,
                };
                let link = dom
                    .child(item, "rPr")
                    .and_then(|properties| dom.child(properties, "hlinkClick"));
                let link = self.link(dom, source, link)?;
                line.push_str(text);
                runs.push(obj([("link", link), ("text", s(text))]));
            }
            lines.push(line);
            paragraphs.push(obj([("level", int(level)), ("runs", Json::Arr(runs))]));
        }
        Ok((Json::Arr(paragraphs), lines.join("\n")))
    }

    fn link(&mut self, dom: &Dom, source: &str, node: Option<usize>) -> Result<Json, Refusal> {
        let Some(node) = node else {
            return Ok(Json::Null);
        };
        let action = dom.attr(node, "action").filter(|a| !a.is_empty());
        check_action(action)?;
        let (mut url, mut slide) = (None, None);
        if let Some(rid) = dom.attr(node, "id").filter(|id| !id.is_empty()) {
            let relationship = self
                .relationship(source, rid)
                .ok_or(Refusal::MalformedContainer)?;
            if relationship.external {
                url = Some(relationship.target.as_str());
            } else if let Some(id) = self
                .slide_ids
                .get(&relationship.target.to_ascii_lowercase())
            {
                slide = Some(*id);
            } else {
                self.codes.insert("link-target-omitted");
            }
        }
        Ok(obj([
            ("action", opt(action)),
            ("slide", slide.map_or(Json::Null, int)),
            ("url", opt(url)),
        ]))
    }

    fn asset(&mut self, source: &str, rid: &str) -> Result<Json, Refusal> {
        let relationship = self
            .relationship(source, rid)
            .ok_or(Refusal::MalformedContainer)?;
        if relationship.external {
            return Err(Refusal::ExternalReference);
        }
        // A missing target is already receipted by the intake.
        let Some(part) = self.part(&relationship.target) else {
            return Ok(Json::Null);
        };
        if !part.content_type.starts_with("image/") {
            self.codes.insert("unsupported-media");
            return Ok(Json::Null);
        }
        let digest = sha256_hex(&part.data);
        let id = asset_id(&digest);
        self.assets.insert(digest, part);
        Ok(s(&id))
    }

    /// Tables become table shapes; charts, SmartArt, and other frames are
    /// receipted and left out of the projection.
    fn frame(
        &mut self,
        dom: &Dom,
        source: &str,
        node: usize,
    ) -> Result<Option<BTreeMap<String, Json>>, Refusal> {
        let data = dom.path(node, &["graphic", "graphicData"]);
        let Some(table) = data.and_then(|d| dom.child(d, "tbl")) else {
            let uri = data.and_then(|d| dom.attr(d, "uri")).unwrap_or_default();
            self.codes.insert(if uri.contains("chart") {
                "unsupported-chart"
            } else if uri.ends_with("/diagram") {
                "unsupported-smartart"
            } else {
                "unsupported-graphic-frame"
            });
            return Ok(None);
        };
        self.codes.insert("table-formatting-normalized");
        let (mut shape, placed) = self.common(dom, source, node, "table")?;
        let mut columns = Vec::new();
        for column in dom
            .child(table, "tblGrid")
            .into_iter()
            .flat_map(|grid| dom.named(grid, "gridCol"))
        {
            columns.push(int(dom.num(column, "w")?));
        }
        let (mut rows, mut lines) = (Vec::new(), Vec::new());
        for row in dom.named(table, "tr") {
            let (mut cells, mut texts) = (Vec::new(), Vec::new());
            for cell in dom.named(row, "tc") {
                let (paragraphs, text) = match dom.child(cell, "txBody") {
                    Some(body) => self.paragraphs(dom, source, body)?,
                    None => (Json::Arr(Vec::new()), String::new()),
                };
                cells.push(paragraphs);
                texts.push(text);
            }
            lines.push(texts.join("\t"));
            rows.push(obj([
                ("cells", Json::Arr(cells)),
                ("height", int(dom.num(row, "h")?)),
            ]));
        }
        self.anchor(&placed, "", "table", lines.join("\n"));
        shape.insert("columns".to_owned(), Json::Arr(columns));
        shape.insert("rows".to_owned(), Json::Arr(rows));
        Ok(Some(shape))
    }

    fn notes(&mut self, slide: &str) -> Result<Json, Refusal> {
        let Some(relationship) = self.related(slide, "/notesSlide").first().copied() else {
            return Ok(Json::Null);
        };
        let Some(part) = self.part(&relationship.target) else {
            return Ok(Json::Null);
        };
        let dom = self.dom(part)?;
        self.scan(&dom)?;
        let body = dom.path(0, &["cSld", "spTree"]).and_then(|tree| {
            dom.named(tree, "sp").find(|&sp| {
                dom.path(sp, &["nvSpPr", "nvPr", "ph"])
                    .is_some_and(|ph| dom.attr(ph, "type") == Some("body"))
            })
        });
        let Some(body) = body else {
            return Ok(Json::Null);
        };
        let id = match dom
            .path(body, &["nvSpPr", "cNvPr"])
            .and_then(|c| dom.attr(c, "id"))
        {
            Some(value) => Some(i64::from(
                value.parse::<u32>().map_err(|_| Refusal::MalformedXml)?,
            )),
            None => None,
        };
        let (paragraphs, text) = match dom.child(body, "txBody") {
            Some(text_body) => self.paragraphs(&dom, &relationship.target, text_body)?,
            None => (Json::Arr(Vec::new()), String::new()),
        };
        let placed = Placed {
            key: "notes".to_owned(),
            shape: id.unwrap_or(0),
            z: 0,
        };
        self.anchor(&placed, "", "notes", text);
        Ok(obj([
            ("paragraphs", paragraphs),
            ("shape", id.map_or(Json::Null, int)),
        ]))
    }

    /// Legacy and modern comments, flattened in document order.
    fn comments(&mut self, slide: &str) -> Result<Vec<Json>, Refusal> {
        let mut comments = Vec::new();
        for relationship in self.related(slide, "/comments") {
            let modern = relationship.kind.contains("microsoft.com");
            let Some(part) = self.part(&relationship.target) else {
                continue;
            };
            let dom = self.dom(part)?;
            self.scan(&dom)?;
            for node in 0..dom.0.len() {
                let name = dom.name(node);
                if !(name == "cm" || modern && name == "reply") {
                    continue;
                }
                if name == "reply" {
                    self.codes.insert("comment-threads-flattened");
                }
                let authors = if modern {
                    &self.modern_authors
                } else {
                    &self.legacy_authors
                };
                let (author, initials) = dom
                    .attr(node, "authorId")
                    .and_then(|id| authors.get(id))
                    .cloned()
                    .unwrap_or_default();
                let text = match (modern, dom.child(node, "txBody"), dom.child(node, "text")) {
                    (true, Some(body), _) => self.paragraphs(&dom, &relationship.target, body)?.1,
                    (false, _, Some(text)) => dom.text(text).to_owned(),
                    _ => String::new(),
                };
                self.codes.insert("comment-metadata-normalized");
                let placed = Placed {
                    key: format!("comment-{}", comments.len() + 1),
                    shape: comments.len() as i64 + 1,
                    z: 0,
                };
                self.anchor(&placed, "", "notes", text.clone());
                comments.push(obj([
                    ("author", s(&author)),
                    ("initials", opt(initials.as_deref())),
                    ("text", s(&text)),
                ]));
            }
        }
        Ok(comments)
    }
}

fn check_action(action: Option<&str>) -> Result<(), Refusal> {
    let action = action.unwrap_or_default().to_ascii_lowercase();
    if RUNNING_ACTIONS
        .iter()
        .any(|prefix| action.starts_with(prefix))
    {
        return Err(Refusal::ActiveContent);
    }
    Ok(())
}

fn xfrm(dom: &Dom, frame: Option<usize>) -> Result<Json, Refusal> {
    let Some(frame) = frame else {
        return Ok(Json::Null);
    };
    let pair = |element: &str, a: &str, b: &str| -> Result<Json, Refusal> {
        Ok(match dom.child(frame, element) {
            Some(node) => Json::Arr(vec![int(dom.num(node, a)?), int(dom.num(node, b)?)]),
            None => Json::Null,
        })
    };
    Ok(obj([
        ("ch_ext", pair("chExt", "cx", "cy")?),
        ("ch_off", pair("chOff", "x", "y")?),
        ("ext", pair("ext", "cx", "cy")?),
        ("off", pair("off", "x", "y")?),
    ]))
}

fn fidelity() -> Json {
    Json::Obj(
        FIDELITY
            .iter()
            .map(|(feature, status)| ((*feature).to_owned(), s(status)))
            .collect(),
    )
}

fn asset_id(digest: &str) -> String {
    format!("asset:pptx:sha256-{digest}")
}

fn s(value: &str) -> Json {
    Json::Str(value.to_owned())
}

fn opt(value: Option<&str>) -> Json {
    value.map_or(Json::Null, s)
}

fn int(value: i64) -> Json {
    Json::Int(value)
}

fn obj<const N: usize>(pairs: [(&str, Json); N]) -> Json {
    Json::Obj(
        pairs
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

// ponytail: hex doubles asset bytes inside canonical content; move to base64
// or a separate asset store if canonical size starts to matter.
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn unhex(text: &str) -> Result<Vec<u8>, Refusal> {
    if !text.len().is_multiple_of(2) || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(BAD);
    }
    let nibble = |b: u8| (b as char).to_digit(16).unwrap_or_default() as u8;
    Ok(text
        .as_bytes()
        .chunks(2)
        .map(|pair| nibble(pair[0]) << 4 | nibble(pair[1]))
        .collect())
}

/// Bounded reader for the canonical JSON subset (`Json` has no floats).
struct JsonParser<'a> {
    text: &'a str,
    at: usize,
}

impl JsonParser<'_> {
    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.at).copied()
    }

    fn eat(&mut self, byte: u8) -> bool {
        let matched = self.peek() == Some(byte);
        self.at += usize::from(matched);
        matched
    }

    fn expect(&mut self, byte: u8) -> Result<(), Refusal> {
        if self.eat(byte) {
            Ok(())
        } else {
            Err(BAD)
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, Refusal> {
        if depth > MAX_JSON_DEPTH {
            return Err(BAD);
        }
        let literal = |parser: &mut Self, word: &str, value: Json| {
            if parser.text[parser.at..].starts_with(word) {
                parser.at += word.len();
                Ok(value)
            } else {
                Err(BAD)
            }
        };
        match self.peek().ok_or(BAD)? {
            b'n' => literal(self, "null", Json::Null),
            b't' => literal(self, "true", Json::Bool(true)),
            b'f' => literal(self, "false", Json::Bool(false)),
            b'"' => self.string().map(Json::Str),
            b'[' => {
                self.at += 1;
                let mut items = Vec::new();
                if !self.eat(b']') {
                    loop {
                        items.push(self.value(depth + 1)?);
                        if self.eat(b']') {
                            break;
                        }
                        self.expect(b',')?;
                    }
                }
                Ok(Json::Arr(items))
            }
            b'{' => {
                self.at += 1;
                let mut map = BTreeMap::new();
                if !self.eat(b'}') {
                    loop {
                        let key = self.string()?;
                        self.expect(b':')?;
                        if map.insert(key, self.value(depth + 1)?).is_some() {
                            return Err(BAD);
                        }
                        if self.eat(b'}') {
                            break;
                        }
                        self.expect(b',')?;
                    }
                }
                Ok(Json::Obj(map))
            }
            _ => {
                let start = self.at;
                self.eat(b'-');
                while self.peek().is_some_and(|b| b.is_ascii_digit()) {
                    self.at += 1;
                }
                self.text[start..self.at]
                    .parse()
                    .map(Json::Int)
                    .map_err(|_| BAD)
            }
        }
    }

    fn string(&mut self) -> Result<String, Refusal> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let rest = &self.text[self.at..];
            let stop = rest
                .find(|c: char| c == '"' || c == '\\' || c < ' ')
                .ok_or(BAD)?;
            out.push_str(&rest[..stop]);
            self.at += stop;
            match self.peek() {
                Some(b'"') => {
                    self.at += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    let escape = self.text.as_bytes().get(self.at + 1).copied().ok_or(BAD)?;
                    self.at += 2;
                    out.push(match escape {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let digits = self.text.get(self.at..self.at + 4).ok_or(BAD)?;
                            if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
                                return Err(BAD);
                            }
                            self.at += 4;
                            let code = u32::from_str_radix(digits, 16).map_err(|_| BAD)?;
                            char::from_u32(code).ok_or(BAD)?
                        }
                        _ => return Err(BAD),
                    });
                }
                _ => return Err(BAD),
            }
        }
    }
}

// ---- Export ----

/// Projects imported content into a new deck. DLP and public-boundary checks
/// run on everything the deck would contain before any bytes are returned.
pub fn export(imported: &Imported) -> Result<Vec<u8>, Refusal> {
    for (code, refusal) in [
        ("dlp-findings", Refusal::DlpFindings),
        ("public-boundary-violation", Refusal::PublicBoundary),
    ] {
        if imported.receipt.iter().any(|item| item == code) {
            return Err(refusal);
        }
    }
    let deck = parse_canonical(&imported.canonical)?;
    if deck.get("schema")?.str()? != SCHEMA {
        return Err(Refusal::ExportUnsupported);
    }
    let mut writer = Writer::default();
    let comments = writer.deck(&deck)?;
    let mut codes = vec!["generic-master-layout-theme", "run-properties-defaulted"];
    if comments {
        codes.push("comments-written-legacy");
    }
    let receipt = obj([
        ("content_sha256", s(&sha256_hex(&imported.canonical))),
        (
            "export_receipt",
            Json::Arr(codes.into_iter().map(s).collect()),
        ),
        ("fidelity", fidelity()),
        (
            "import_receipt",
            Json::Arr(imported.receipt.iter().map(|code| s(code)).collect()),
        ),
        ("receipt_version", s(RECEIPT_VERSION)),
        ("schema", s(EXPORT_RECEIPT_SCHEMA)),
    ]);
    writer.part(RECEIPT_PART.to_owned(), None, receipt.to_bytes());
    let mut root = Rels::default();
    root.add(
        &format!("{R}/officeDocument"),
        "ppt/presentation.xml",
        false,
    );
    root.add(RECEIPT_RELATIONSHIP, RECEIPT_PART, false);
    writer.rels("", &root);
    writer.finish()
}

/// Reads canonical content; any mismatch with the model is `BAD`.
trait Field {
    fn get(&self, key: &str) -> Result<&Json, Refusal>;
    fn str(&self) -> Result<&str, Refusal>;
    fn opt_str(&self) -> Result<Option<&str>, Refusal>;
    fn int(&self) -> Result<i64, Refusal>;
    fn opt_int(&self) -> Result<Option<i64>, Refusal>;
    fn arr(&self) -> Result<&[Json], Refusal>;
    fn bool(&self) -> Result<bool, Refusal>;
}

impl Field for Json {
    fn get(&self, key: &str) -> Result<&Json, Refusal> {
        match self {
            Json::Obj(map) => map.get(key).ok_or(BAD),
            _ => Err(BAD),
        }
    }

    fn str(&self) -> Result<&str, Refusal> {
        match self {
            Json::Str(text) => Ok(text),
            _ => Err(BAD),
        }
    }

    fn opt_str(&self) -> Result<Option<&str>, Refusal> {
        match self {
            Json::Null => Ok(None),
            other => other.str().map(Some),
        }
    }

    fn int(&self) -> Result<i64, Refusal> {
        match self {
            Json::Int(value) => Ok(*value),
            _ => Err(BAD),
        }
    }

    fn opt_int(&self) -> Result<Option<i64>, Refusal> {
        match self {
            Json::Null => Ok(None),
            other => other.int().map(Some),
        }
    }

    fn arr(&self) -> Result<&[Json], Refusal> {
        match self {
            Json::Arr(items) => Ok(items),
            _ => Err(BAD),
        }
    }

    fn bool(&self) -> Result<bool, Refusal> {
        match self {
            Json::Bool(value) => Ok(*value),
            _ => Err(BAD),
        }
    }
}

/// Relationships of one part, deduplicated, numbered `rId1..` in order added.
#[derive(Default)]
struct Rels(Vec<(String, String, bool)>);

impl Rels {
    fn add(&mut self, kind: &str, target: &str, external: bool) -> String {
        let key = (kind.to_owned(), target.to_owned(), external);
        let index = match self.0.iter().position(|item| *item == key) {
            Some(index) => index,
            None => {
                self.0.push(key);
                self.0.len() - 1
            }
        };
        format!("rId{}", index + 1)
    }
}

/// Parts of the deck being written, with the text they emit for DLP.
#[derive(Default)]
struct Writer {
    parts: BTreeMap<String, Vec<u8>>,
    types: BTreeMap<String, String>,
    texts: Vec<String>,
}

const DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";
const NS: &str = "xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\"";
const TREE_ROOT: &str =
    "<p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>";
const PLACEHOLDERS: &str = "<p:sp><p:nvSpPr><p:cNvPr id=\"2\" name=\"Title Placeholder 1\"/><p:cNvSpPr/><p:nvPr><p:ph type=\"title\"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x=\"838200\" y=\"365125\"/><a:ext cx=\"10515600\" cy=\"1325563\"/></a:xfrm></p:spPr></p:sp><p:sp><p:nvSpPr><p:cNvPr id=\"3\" name=\"Text Placeholder 2\"/><p:cNvSpPr/><p:nvPr><p:ph type=\"body\" idx=\"1\"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x=\"838200\" y=\"1825625\"/><a:ext cx=\"10515600\" cy=\"4351338\"/></a:xfrm></p:spPr></p:sp>";
const CLR_MAP: &str = "<p:clrMap bg1=\"lt1\" tx1=\"dk1\" bg2=\"lt2\" tx2=\"dk2\" accent1=\"accent1\" accent2=\"accent2\" accent3=\"accent3\" accent4=\"accent4\" accent5=\"accent5\" accent6=\"accent6\" hlink=\"hlink\" folHlink=\"folHlink\"/>";
const MASTER_CLR: &str = "<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>";
const THEME: &str = "<a:theme xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" name=\"BRAN\"><a:themeElements><a:clrScheme name=\"BRAN\"><a:dk1><a:srgbClr val=\"000000\"/></a:dk1><a:lt1><a:srgbClr val=\"FFFFFF\"/></a:lt1><a:dk2><a:srgbClr val=\"1F2937\"/></a:dk2><a:lt2><a:srgbClr val=\"F3F4F6\"/></a:lt2><a:accent1><a:srgbClr val=\"2563EB\"/></a:accent1><a:accent2><a:srgbClr val=\"059669\"/></a:accent2><a:accent3><a:srgbClr val=\"D97706\"/></a:accent3><a:accent4><a:srgbClr val=\"DC2626\"/></a:accent4><a:accent5><a:srgbClr val=\"7C3AED\"/></a:accent5><a:accent6><a:srgbClr val=\"0891B2\"/></a:accent6><a:hlink><a:srgbClr val=\"2563EB\"/></a:hlink><a:folHlink><a:srgbClr val=\"7C3AED\"/></a:folHlink></a:clrScheme><a:fontScheme name=\"BRAN\"><a:majorFont><a:latin typeface=\"Calibri\"/><a:ea typeface=\"\"/><a:cs typeface=\"\"/></a:majorFont><a:minorFont><a:latin typeface=\"Calibri\"/><a:ea typeface=\"\"/><a:cs typeface=\"\"/></a:minorFont></a:fontScheme><a:fmtScheme name=\"BRAN\"><a:fillStyleLst><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:fillStyleLst><a:lnStyleLst><a:ln w=\"6350\"><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:ln><a:ln w=\"12700\"><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:ln><a:ln w=\"19050\"><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:ln></a:lnStyleLst><a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst><a:bgFillStyleLst><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:bgFillStyleLst></a:fmtScheme></a:themeElements></a:theme>";

impl Writer {
    fn part(&mut self, name: String, content_type: Option<&str>, data: impl Into<Vec<u8>>) {
        if let Some(content_type) = content_type {
            self.types.insert(name.clone(), content_type.to_owned());
        }
        self.parts.insert(name, data.into());
    }

    fn rels(&mut self, source: &str, rels: &Rels) {
        let name = match source.rsplit_once('/') {
            Some((directory, file)) => format!("{directory}/_rels/{file}.rels"),
            None => format!("_rels/{source}.rels"),
        };
        let mut xml = format!(
            "{DECL}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">"
        );
        for (index, (kind, target, external)) in rels.0.iter().enumerate() {
            let mode = if *external {
                " TargetMode=\"External\""
            } else {
                ""
            };
            let _ = write!(
                xml,
                "<Relationship Id=\"rId{}\" Type=\"{}\" Target=\"{}\"{mode}/>",
                index + 1,
                esc(kind),
                esc(target)
            );
        }
        xml.push_str("</Relationships>");
        self.parts.insert(name, xml.into_bytes());
    }

    /// Writes every deck part. Returns whether any comment was written.
    fn deck(&mut self, deck: &Json) -> Result<bool, Refusal> {
        let slides = deck.get("slides")?.arr()?;
        let mut media = BTreeMap::new();
        for (index, asset) in deck.get("assets")?.arr()?.iter().enumerate() {
            let data = unhex(asset.get("data_hex")?.str()?)?;
            if sha256_hex(&data) != asset.get("sha256")?.str()? {
                return Err(BAD);
            }
            let media_type = asset.get("media_type")?.str()?;
            let file = format!("image{}.{}", index + 1, extension(media_type));
            self.part(format!("ppt/media/{file}"), Some(media_type), data);
            media.insert(asset.get("id")?.str()?, file);
        }
        let mut ids = BTreeMap::new();
        let mut layouts: Vec<Option<&str>> = Vec::new();
        for (index, slide) in slides.iter().enumerate() {
            if ids.insert(slide.get("id")?.int()?, index + 1).is_some() {
                return Err(BAD);
            }
            let layout = slide.get("layout")?.opt_str()?;
            if !layouts.contains(&layout) {
                layouts.push(layout);
            }
        }
        if layouts.is_empty() {
            layouts.push(None);
        }

        let mut master = Rels::default();
        let mut layout_ids = String::new();
        for (index, name) in layouts.iter().enumerate() {
            let file = format!("slideLayout{}.xml", index + 1);
            let rid = master.add(
                &format!("{R}/slideLayout"),
                &format!("../slideLayouts/{file}"),
                false,
            );
            let _ = write!(
                layout_ids,
                "<p:sldLayoutId id=\"{}\" r:id=\"{rid}\"/>",
                2_147_483_649_u64 + index as u64
            );
            let name = match name {
                Some(name) => {
                    self.texts.push((*name).to_owned());
                    format!(" name=\"{}\"", esc(name))
                }
                None => String::new(),
            };
            let part = format!("ppt/slideLayouts/{file}");
            self.part(
                part.clone(),
                Some(&format!("{PML}.slideLayout+xml")),
                format!("{DECL}<p:sldLayout {NS} preserve=\"1\"><p:cSld{name}><p:spTree>{TREE_ROOT}{PLACEHOLDERS}</p:spTree></p:cSld>{MASTER_CLR}</p:sldLayout>"),
            );
            let mut rels = Rels::default();
            rels.add(
                &format!("{R}/slideMaster"),
                "../slideMasters/slideMaster1.xml",
                false,
            );
            self.rels(&part, &rels);
        }
        master.add(&format!("{R}/theme"), "../theme/theme1.xml", false);
        self.part(
            "ppt/slideMasters/slideMaster1.xml".to_owned(),
            Some(&format!("{PML}.slideMaster+xml")),
            format!("{DECL}<p:sldMaster {NS}><p:cSld><p:spTree>{TREE_ROOT}{PLACEHOLDERS}</p:spTree></p:cSld>{CLR_MAP}<p:sldLayoutIdLst>{layout_ids}</p:sldLayoutIdLst></p:sldMaster>"),
        );
        self.rels("ppt/slideMasters/slideMaster1.xml", &master);
        self.part(
            "ppt/theme/theme1.xml".to_owned(),
            Some("application/vnd.openxmlformats-officedocument.theme+xml"),
            format!("{DECL}{THEME}"),
        );

        let mut presentation = Rels::default();
        presentation.add(
            &format!("{R}/slideMaster"),
            "slideMasters/slideMaster1.xml",
            false,
        );
        let mut slide_list = String::new();
        let mut any_notes = false;
        // (author, initials) in first-use order, with each author's last index.
        let mut authors: Vec<((&str, Option<&str>), usize)> = Vec::new();
        for (index, slide) in slides.iter().enumerate() {
            let number = index + 1;
            let part = format!("ppt/slides/slide{number}.xml");
            let rid = presentation.add(
                &format!("{R}/slide"),
                &format!("slides/slide{number}.xml"),
                false,
            );
            let _ = write!(
                slide_list,
                "<p:sldId id=\"{}\" r:id=\"{rid}\"/>",
                slide.get("id")?.int()?
            );
            let mut rels = Rels::default();
            let layout = slide.get("layout")?.opt_str()?;
            let layout = layouts
                .iter()
                .position(|item| *item == layout)
                .unwrap_or_default();
            rels.add(
                &format!("{R}/slideLayout"),
                &format!("../slideLayouts/slideLayout{}.xml", layout + 1),
                false,
            );
            let mut shapes = String::new();
            for shape in slide.get("shapes")?.arr()? {
                self.shape(&mut shapes, shape, &mut rels, &media, &ids, 0)?;
            }
            let notes = slide.get("notes")?;
            if !matches!(notes, Json::Null) {
                any_notes = true;
                self.notes(notes, number, &ids)?;
                rels.add(
                    &format!("{R}/notesSlide"),
                    &format!("../notesSlides/notesSlide{number}.xml"),
                    false,
                );
            }
            let comments = slide.get("comments")?.arr()?;
            if !comments.is_empty() {
                let mut xml = format!("{DECL}<p:cmLst {NS}>");
                for comment in comments {
                    let author = comment.get("author")?.str()?;
                    let initials = comment.get("initials")?.opt_str()?;
                    let text = comment.get("text")?.str()?;
                    self.texts.extend([author.to_owned(), text.to_owned()]);
                    let key = (author, initials);
                    let id = match authors.iter().position(|(item, _)| *item == key) {
                        Some(id) => id,
                        None => {
                            authors.push((key, 0));
                            authors.len() - 1
                        }
                    };
                    authors[id].1 += 1;
                    let _ = write!(
                        xml,
                        "<p:cm authorId=\"{id}\" idx=\"{}\"><p:pos x=\"0\" y=\"0\"/><p:text>{}</p:text></p:cm>",
                        authors[id].1,
                        esc(text)
                    );
                }
                xml.push_str("</p:cmLst>");
                self.part(
                    format!("ppt/comments/comment{number}.xml"),
                    Some(&format!("{PML}.comments+xml")),
                    xml,
                );
                rels.add(
                    &format!("{R}/comments"),
                    &format!("../comments/comment{number}.xml"),
                    false,
                );
            }
            let hidden = if slide.get("hidden")?.bool()? {
                " show=\"0\""
            } else {
                ""
            };
            let name = match slide.get("name")?.opt_str()? {
                Some(name) => {
                    self.texts.push(name.to_owned());
                    format!(" name=\"{}\"", esc(name))
                }
                None => String::new(),
            };
            self.part(
                part.clone(),
                Some(&format!("{PML}.slide+xml")),
                format!("{DECL}<p:sld {NS}{hidden}><p:cSld{name}><p:spTree>{TREE_ROOT}{shapes}</p:spTree></p:cSld>{MASTER_CLR}</p:sld>"),
            );
            self.rels(&part, &rels);
        }

        let notes_master = if any_notes {
            let rid = presentation.add(
                &format!("{R}/notesMaster"),
                "notesMasters/notesMaster1.xml",
                false,
            );
            self.part(
                "ppt/notesMasters/notesMaster1.xml".to_owned(),
                Some(&format!("{PML}.notesMaster+xml")),
                format!("{DECL}<p:notesMaster {NS}><p:cSld><p:spTree>{TREE_ROOT}</p:spTree></p:cSld>{CLR_MAP}</p:notesMaster>"),
            );
            let mut rels = Rels::default();
            rels.add(&format!("{R}/theme"), "../theme/theme2.xml", false);
            self.rels("ppt/notesMasters/notesMaster1.xml", &rels);
            self.part(
                "ppt/theme/theme2.xml".to_owned(),
                Some("application/vnd.openxmlformats-officedocument.theme+xml"),
                format!("{DECL}{THEME}"),
            );
            format!("<p:notesMasterIdLst><p:notesMasterId r:id=\"{rid}\"/></p:notesMasterIdLst>")
        } else {
            String::new()
        };
        if !authors.is_empty() {
            presentation.add(&format!("{R}/commentAuthors"), "commentAuthors.xml", false);
            let mut xml = format!("{DECL}<p:cmAuthorLst {NS}>");
            for (id, ((name, initials), last)) in authors.iter().enumerate() {
                let initials = initials
                    .map(|value| format!(" initials=\"{}\"", esc(value)))
                    .unwrap_or_default();
                let _ = write!(
                    xml,
                    "<p:cmAuthor id=\"{id}\" name=\"{}\"{initials} lastIdx=\"{last}\" clrIdx=\"{}\"/>",
                    esc(name),
                    id % 8
                );
            }
            xml.push_str("</p:cmAuthorLst>");
            self.part(
                "ppt/commentAuthors.xml".to_owned(),
                Some(&format!("{PML}.commentAuthors+xml")),
                xml,
            );
        }
        presentation.add(&format!("{R}/theme"), "theme/theme1.xml", false);
        let size = match deck.get("slide_size")? {
            Json::Null => String::new(),
            size => format!(
                "<p:sldSz cx=\"{}\" cy=\"{}\"/>",
                size.get("cx")?.int()?,
                size.get("cy")?.int()?
            ),
        };
        let mut sections = String::new();
        for section in deck.get("sections")?.arr()? {
            let name = section.get("name")?.str()?;
            self.texts.push(name.to_owned());
            let id = section
                .get("id")?
                .opt_str()?
                .map(|id| format!(" id=\"{}\"", esc(id)))
                .unwrap_or_default();
            let _ = write!(
                sections,
                "<p14:section name=\"{}\"{id}><p14:sldIdLst>",
                esc(name)
            );
            for slide in section.get("slides")?.arr()? {
                let _ = write!(sections, "<p14:sldId id=\"{}\"/>", slide.int()?);
            }
            sections.push_str("</p14:sldIdLst></p14:section>");
        }
        if !sections.is_empty() {
            sections = format!("<p:extLst><p:ext uri=\"{{521415D9-36F7-43E2-AB2F-B90AF26B5E84}}\"><p14:sectionLst xmlns:p14=\"http://schemas.microsoft.com/office/powerpoint/2010/main\">{sections}</p14:sectionLst></p:ext></p:extLst>");
        }
        self.part(
            "ppt/presentation.xml".to_owned(),
            Some(MAIN_TYPES[0]),
            format!("{DECL}<p:presentation {NS}><p:sldMasterIdLst><p:sldMasterId id=\"2147483648\" r:id=\"rId1\"/></p:sldMasterIdLst>{notes_master}<p:sldIdLst>{slide_list}</p:sldIdLst>{size}<p:notesSz cx=\"6858000\" cy=\"9144000\"/>{sections}</p:presentation>"),
        );
        self.rels("ppt/presentation.xml", &presentation);
        Ok(!authors.is_empty())
    }

    fn notes(
        &mut self,
        notes: &Json,
        number: usize,
        ids: &BTreeMap<i64, usize>,
    ) -> Result<(), Refusal> {
        let id = notes
            .get("shape")?
            .opt_int()?
            .ok_or(Refusal::ExportUnsupported)?;
        let mut rels = Rels::default();
        rels.add(
            &format!("{R}/notesMaster"),
            "../notesMasters/notesMaster1.xml",
            false,
        );
        rels.add(
            &format!("{R}/slide"),
            &format!("../slides/slide{number}.xml"),
            false,
        );
        let body = self.paragraphs(notes.get("paragraphs")?, &mut rels, ids)?;
        let part = format!("ppt/notesSlides/notesSlide{number}.xml");
        self.part(
            part.clone(),
            Some(&format!("{PML}.notesSlide+xml")),
            format!("{DECL}<p:notes {NS}><p:cSld><p:spTree>{TREE_ROOT}<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"Notes Placeholder\"/><p:cNvSpPr/><p:nvPr><p:ph type=\"body\" idx=\"1\"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/>{body}</p:txBody></p:sp></p:spTree></p:cSld>{MASTER_CLR}</p:notes>"),
        );
        self.rels(&part, &rels);
        Ok(())
    }

    fn shape(
        &mut self,
        out: &mut String,
        shape: &Json,
        rels: &mut Rels,
        media: &BTreeMap<&str, String>,
        ids: &BTreeMap<i64, usize>,
        depth: usize,
    ) -> Result<(), Refusal> {
        if depth > MAX_NESTING {
            return Err(BAD);
        }
        // A shape without an identity cannot be projected faithfully.
        let id = shape
            .get("id")?
            .opt_int()?
            .ok_or(Refusal::ExportUnsupported)?;
        let name = shape.get("name")?.str()?;
        self.texts.push(name.to_owned());
        let mut attributes = format!(" id=\"{id}\" name=\"{}\"", esc(name));
        for (key, attribute) in [("alt_text", "descr"), ("alt_title", "title")] {
            if let Some(value) = shape.get(key)?.opt_str()? {
                self.texts.push(value.to_owned());
                let _ = write!(attributes, " {attribute}=\"{}\"", esc(value));
            }
        }
        if shape.get("hidden")?.bool()? {
            attributes.push_str(" hidden=\"1\"");
        }
        let link = self.link(shape.get("link")?, rels, ids)?;
        let c_nv_pr = format!("<p:cNvPr{attributes}>{link}</p:cNvPr>");
        let nv_pr = match shape.get("placeholder")? {
            Json::Null => "<p:nvPr/>".to_owned(),
            placeholder => {
                let mut ph = String::new();
                for key in ["type", "idx"] {
                    if let Some(value) = placeholder.get(key)?.opt_str()? {
                        let _ = write!(ph, " {key}=\"{}\"", esc(value));
                    }
                }
                format!("<p:nvPr><p:ph{ph}/></p:nvPr>")
            }
        };
        let geometry = match shape.get("preset")?.opt_str()? {
            Some(preset) => format!(
                "<a:prstGeom prst=\"{}\"><a:avLst/></a:prstGeom>",
                esc(preset)
            ),
            None => String::new(),
        };
        let frame = shape.get("xfrm")?;
        let _ = match shape.get("kind")?.str()? {
            "shape" => {
                let text_box = if shape.get("text_box")?.bool()? { " txBox=\"1\"" } else { "" };
                let body = match shape.get("paragraphs")? {
                    Json::Null => String::new(),
                    paragraphs => format!(
                        "<p:txBody><a:bodyPr/><a:lstStyle/>{}</p:txBody>",
                        self.paragraphs(paragraphs, rels, ids)?
                    ),
                };
                write!(out, "<p:sp><p:nvSpPr>{c_nv_pr}<p:cNvSpPr{text_box}/>{nv_pr}</p:nvSpPr><p:spPr>{}{geometry}</p:spPr>{body}</p:sp>", xfrm_xml(frame, "a:xfrm")?)
            }
            "connector" => write!(out, "<p:cxnSp><p:nvCxnSpPr>{c_nv_pr}<p:cNvCxnSpPr/>{nv_pr}</p:nvCxnSpPr><p:spPr>{}{geometry}</p:spPr></p:cxnSp>", xfrm_xml(frame, "a:xfrm")?),
            "picture" => {
                let blip = match shape.get("image")?.opt_str()? {
                    Some(asset) => {
                        let file = media.get(asset).ok_or(BAD)?;
                        let rid = rels.add(&format!("{R}/image"), &format!("../media/{file}"), false);
                        format!("<a:blip r:embed=\"{rid}\"/>")
                    }
                    None => String::new(),
                };
                write!(out, "<p:pic><p:nvPicPr>{c_nv_pr}<p:cNvPicPr/>{nv_pr}</p:nvPicPr><p:blipFill>{blip}<a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr>{}{geometry}</p:spPr></p:pic>", xfrm_xml(frame, "a:xfrm")?)
            }
            "group" => {
                let mut children = String::new();
                for child in shape.get("children")?.arr()? {
                    self.shape(&mut children, child, rels, media, ids, depth + 1)?;
                }
                write!(out, "<p:grpSp><p:nvGrpSpPr>{c_nv_pr}<p:cNvGrpSpPr/>{nv_pr}</p:nvGrpSpPr><p:grpSpPr>{}</p:grpSpPr>{children}</p:grpSp>", xfrm_xml(frame, "a:xfrm")?)
            }
            "table" => {
                let mut table = String::from("<a:tbl><a:tblPr firstRow=\"1\" bandRow=\"1\"/><a:tblGrid>");
                for column in shape.get("columns")?.arr()? {
                    let _ = write!(table, "<a:gridCol w=\"{}\"/>", column.int()?);
                }
                table.push_str("</a:tblGrid>");
                for row in shape.get("rows")?.arr()? {
                    let _ = write!(table, "<a:tr h=\"{}\">", row.get("height")?.int()?);
                    for cell in row.get("cells")?.arr()? {
                        let _ = write!(table, "<a:tc><a:txBody><a:bodyPr/><a:lstStyle/>{}</a:txBody><a:tcPr/></a:tc>", self.paragraphs(cell, rels, ids)?);
                    }
                    table.push_str("</a:tr>");
                }
                table.push_str("</a:tbl>");
                write!(out, "<p:graphicFrame><p:nvGraphicFramePr>{c_nv_pr}<p:cNvGraphicFramePr><a:graphicFrameLocks noGrp=\"1\"/></p:cNvGraphicFramePr>{nv_pr}</p:nvGraphicFramePr>{}<a:graphic><a:graphicData uri=\"{TABLE_URI}\">{table}</a:graphicData></a:graphic></p:graphicFrame>", xfrm_xml(frame, "p:xfrm")?)
            }
            _ => return Err(BAD),
        };
        Ok(())
    }

    fn paragraphs(
        &mut self,
        paragraphs: &Json,
        rels: &mut Rels,
        ids: &BTreeMap<i64, usize>,
    ) -> Result<String, Refusal> {
        let mut out = String::new();
        for paragraph in paragraphs.arr()? {
            out.push_str("<a:p>");
            let level = paragraph.get("level")?.int()?;
            if level != 0 {
                let _ = write!(out, "<a:pPr lvl=\"{level}\"/>");
            }
            let mut line = String::new();
            for run in paragraph.get("runs")?.arr()? {
                let text = run.get("text")?.str()?;
                let link = self.link(run.get("link")?, rels, ids)?;
                line.push_str(text);
                let _ = match (text, link.is_empty()) {
                    ("\n", true) => write!(out, "<a:br/>"),
                    (_, true) => write!(
                        out,
                        "<a:r><a:rPr dirty=\"0\"/><a:t>{}</a:t></a:r>",
                        esc(text)
                    ),
                    (_, false) => write!(
                        out,
                        "<a:r><a:rPr dirty=\"0\">{link}</a:rPr><a:t>{}</a:t></a:r>",
                        esc(text)
                    ),
                };
            }
            self.texts.push(line);
            out.push_str("</a:p>");
        }
        Ok(out)
    }

    fn link(
        &mut self,
        link: &Json,
        rels: &mut Rels,
        ids: &BTreeMap<i64, usize>,
    ) -> Result<String, Refusal> {
        if matches!(link, Json::Null) {
            return Ok(String::new());
        }
        let action = link.get("action")?.opt_str()?;
        check_action(action)?;
        let rid = match (link.get("url")?.opt_str()?, link.get("slide")?.opt_int()?) {
            (Some(url), _) => {
                self.texts.push(url.to_owned());
                rels.add(&format!("{R}/hyperlink"), url, true)
            }
            (None, Some(slide)) => {
                let number = ids.get(&slide).ok_or(BAD)?;
                rels.add(
                    &format!("{R}/slide"),
                    &format!("../slides/slide{number}.xml"),
                    false,
                )
            }
            (None, None) => String::new(),
        };
        let action = action
            .map(|action| format!(" action=\"{}\"", esc(action)))
            .unwrap_or_default();
        Ok(format!("<a:hlinkClick r:id=\"{rid}\"{action}/>"))
    }

    /// Checks everything the deck contains, then builds the package with
    /// sorted names, one fixed timestamp, and deflate.
    fn finish(mut self) -> Result<Vec<u8>, Refusal> {
        let mut types = String::from("<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Default Extension=\"json\" ContentType=\"application/json\"/>");
        for (name, content_type) in &self.types {
            let _ = write!(
                types,
                "<Override PartName=\"/{}\" ContentType=\"{}\"/>",
                esc(name),
                esc(content_type)
            );
        }
        types.push_str("</Types>");
        self.parts.insert(
            "[Content_Types].xml".to_owned(),
            format!("{DECL}{types}").into_bytes(),
        );
        let emitted = self.texts.iter().map(String::as_str).chain(
            self.parts
                .values()
                .map(|data| std::str::from_utf8(data).unwrap_or_default()),
        );
        for text in emitted {
            match validate_emitted_string(text) {
                Err(ExportError::DlpViolation(_)) => return Err(Refusal::DlpFindings),
                Err(_) => return Err(Refusal::PublicBoundary),
                Ok(()) => {}
            }
        }
        // `[Content_Types].xml` sorts first; readers that expect it first get it.
        let entries: Vec<WriteEntry<'_>> = self
            .parts
            .iter()
            .map(|(name, data)| WriteEntry {
                name,
                data,
                deflate: true,
                dos_time: 0,
                dos_date: 0x0021,
            })
            .collect();
        Ok(zip::write(&entries))
    }
}

fn xfrm_xml(frame: &Json, tag: &str) -> Result<String, Refusal> {
    if matches!(frame, Json::Null) {
        return Ok(String::new());
    }
    let mut inner = String::new();
    for (key, element, a, b) in [
        ("off", "a:off", "x", "y"),
        ("ext", "a:ext", "cx", "cy"),
        ("ch_off", "a:chOff", "x", "y"),
        ("ch_ext", "a:chExt", "cx", "cy"),
    ] {
        match frame.get(key)? {
            Json::Null => {}
            Json::Arr(pair) if pair.len() == 2 => {
                let _ = write!(
                    inner,
                    "<{element} {a}=\"{}\" {b}=\"{}\"/>",
                    pair[0].int()?,
                    pair[1].int()?
                );
            }
            _ => return Err(BAD),
        }
    }
    Ok(format!("<{tag}>{inner}</{tag}>"))
}

/// XML text and attribute escaping. Tabs and line breaks become character
/// references so attribute normalization cannot change them on re-import;
/// characters XML 1.0 cannot carry become U+FFFD.
fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' => out.push_str("&#9;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => out.push('\u{FFFD}'),
            c => out.push(c),
        }
    }
    out
}

fn extension(media_type: &str) -> &'static str {
    match media_type {
        "image/png" => "png",
        "image/jpeg" => "jpeg",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        "image/tiff" => "tiff",
        "image/x-emf" => "emf",
        "image/x-wmf" => "wmf",
        "image/svg+xml" => "svg",
        _ => "bin",
    }
}
