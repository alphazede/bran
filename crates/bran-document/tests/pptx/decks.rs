//! Synthetic PPTX decks for the PPTX adapter tests and the shared corpus.
//!
//! The deck text lives in the reviewable `.parts` fixtures. The one binary
//! part, a small PNG, is generated here because fixtures must stay text.
#![allow(dead_code)]

use bran_document::conformance::Expect;
use bran_document::zip::{self, WriteEntry};
use std::io::Write;

const ORDINARY: &str =
    include_str!("../../../../fixtures/enterprise-documents/conformance/pptx-ordinary.parts");
const SLIDES: &str =
    include_str!("../../../../fixtures/enterprise-documents/conformance/pptx-slides.parts");
const DESIGN: &str =
    include_str!("../../../../fixtures/enterprise-documents/conformance/pptx-design.parts");
pub const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// Receipt codes the unsupported-but-benign deck must carry.
pub const BENIGN_CODES: [&str; 6] = [
    "alternate-content-fallback",
    "unsupported-animation",
    "unsupported-chart",
    "unsupported-media",
    "unsupported-smartart",
    "unsupported-transition",
];

/// Ordered ZIP entries built from `.parts` text.
#[derive(Clone)]
pub struct Deck(pub Vec<(String, Vec<u8>)>);

impl Deck {
    pub fn parse(texts: &[&str]) -> Self {
        let mut parts: Vec<(String, Vec<u8>)> = Vec::new();
        for line in texts.iter().flat_map(|text| text.lines()) {
            if let Some(name) = line.strip_prefix("--- ") {
                parts.push((name.to_owned(), Vec::new()));
            } else {
                let body = &mut parts.last_mut().expect("part header first").1;
                if !body.is_empty() {
                    body.push(b'\n');
                }
                body.extend_from_slice(line.as_bytes());
            }
        }
        Self(parts)
    }

    /// Three slides with layouts, sections, a text box, a group, a table,
    /// notes, a comment, alt text, links, and one image used twice.
    pub fn ordinary() -> Self {
        Self::parse(&[ORDINARY, SLIDES, DESIGN]).add("ppt/media/image1.png", png([37, 99, 235]))
    }

    pub fn add(mut self, name: &str, data: impl Into<Vec<u8>>) -> Self {
        self.0.push((name.to_owned(), data.into()));
        self
    }

    pub fn remove(mut self, name: &str) -> Self {
        self.0.retain(|(part, _)| part != name);
        self
    }

    pub fn edit(mut self, name: &str, from: &str, to: &str) -> Self {
        let part = self
            .0
            .iter_mut()
            .find(|(part, _)| part == name)
            .expect("part");
        let text = String::from_utf8(part.1.clone()).expect("utf-8 part");
        assert!(text.contains(from), "edit anchor missing in {name}: {from}");
        part.1 = text.replacen(from, to, 1).into_bytes();
        self
    }

    /// Adds a relationship to an existing `.rels` part.
    pub fn relate(self, rels: &str, id: &str, kind: &str, target: &str, external: bool) -> Self {
        let mode = if external {
            " TargetMode=\"External\""
        } else {
            ""
        };
        let line = format!(
            "<Relationship Id=\"{id}\" Type=\"{kind}\" Target=\"{target}\"{mode}/>\n</Relationships>"
        );
        self.edit(rels, "</Relationships>", &line)
    }

    pub fn content_type(self, part: &str, content_type: &str) -> Self {
        let line =
            format!("<Override PartName=\"/{part}\" ContentType=\"{content_type}\"/>\n</Types>");
        self.edit("[Content_Types].xml", "</Types>", &line)
    }

    /// Inserts shapes at the end of a slide's shape tree.
    pub fn shapes(self, slide: &str, xml: &str) -> Self {
        self.edit(slide, "</p:spTree>", &format!("{xml}</p:spTree>"))
    }

    pub fn zip_with(&self, reverse: bool, deflate: bool, dos_date: u16) -> Vec<u8> {
        let mut entries: Vec<WriteEntry<'_>> = self
            .0
            .iter()
            .map(|(name, data)| WriteEntry {
                name,
                data,
                deflate,
                dos_time: 0x6000,
                dos_date,
            })
            .collect();
        if reverse {
            entries.reverse();
        }
        zip::write(&entries)
    }

    pub fn zip(&self) -> Vec<u8> {
        self.zip_with(false, true, 0x5921)
    }
}

/// The ordinary deck plus a transition, an animation, a chart, SmartArt, a
/// video, and alternate content. None of them may disappear silently.
pub fn unsupported_benign() -> Deck {
    let frame = |id: u32, uri: &str, body: &str| {
        format!(
            "<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id=\"{id}\" name=\"Frame {id}\"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"100\" cy=\"100\"/></p:xfrm><a:graphic><a:graphicData uri=\"{uri}\">{body}</a:graphicData></a:graphic></p:graphicFrame>"
        )
    };
    let text_shape = |id: u32, text: &str| {
        format!(
            "<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"{text}\"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp>"
        )
    };
    let shapes = [
        frame(
            20,
            "http://schemas.openxmlformats.org/drawingml/2006/chart",
            "<c:chart xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\" r:id=\"rId5\"/>",
        ),
        frame(
            21,
            "http://schemas.openxmlformats.org/drawingml/2006/diagram",
            "<dgm:relIds xmlns:dgm=\"http://schemas.openxmlformats.org/drawingml/2006/diagram\"/>",
        ),
        format!(
            "<mc:AlternateContent xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\"><mc:Choice xmlns:p14=\"http://schemas.microsoft.com/office/powerpoint/2010/main\" Requires=\"p14\">{}</mc:Choice><mc:Fallback>{}</mc:Fallback></mc:AlternateContent>",
            text_shape(22, "Choice text"),
            text_shape(22, "Fallback text")
        ),
        "<p:pic><p:nvPicPr><p:cNvPr id=\"23\" name=\"Video 22\"/><p:cNvPicPr/><p:nvPr><a:videoFile r:link=\"rId6\"/></p:nvPr></p:nvPicPr><p:blipFill><a:blip r:embed=\"rId2\"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr/></p:pic>".to_owned(),
    ]
    .concat();
    Deck::ordinary()
        .edit(
            "ppt/slides/slide1.xml",
            "</p:clrMapOvr></p:sld>",
            "</p:clrMapOvr><p:transition spd=\"med\"><p:fade/></p:transition><p:timing><p:tnLst><p:par><p:cTn id=\"1\" dur=\"indefinite\" restart=\"never\" nodeType=\"tmRoot\"/></p:par></p:tnLst></p:timing></p:sld>",
        )
        .shapes("ppt/slides/slide3.xml", &shapes)
        .relate(
            "ppt/slides/_rels/slide3.xml.rels",
            "rId5",
            &format!("{REL}/chart"),
            "../charts/chart1.xml",
            false,
        )
        .relate(
            "ppt/slides/_rels/slide3.xml.rels",
            "rId6",
            &format!("{REL}/video"),
            "../media/media1.mp4",
            false,
        )
        .add(
            "ppt/charts/chart1.xml",
            "<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"/>",
        )
        .content_type(
            "ppt/charts/chart1.xml",
            "application/vnd.openxmlformats-officedocument.drawingml.chart+xml",
        )
        .add("ppt/media/media1.mp4", "synthetic inert video bytes")
        .content_type("ppt/media/media1.mp4", "video/mp4")
}

/// Executable PPTX rows of the shared corpus.
pub fn row(row: &str) -> Option<(Vec<u8>, Expect)> {
    match row {
        "pptx-ordinary-projection" => Some((
            Deck::ordinary().zip(),
            Expect::Admit(vec!["hyperlink-not-fetched", "field-as-text"]),
        )),
        "pptx-unsupported-benign-fidelity" => Some((
            unsupported_benign().zip(),
            Expect::Admit(BENIGN_CODES.to_vec()),
        )),
        "pptx-round-trip-anchors" => Some((Deck::ordinary().zip(), Expect::Admit(vec![]))),
        _ => None,
    }
}

/// A valid one-pixel RGB PNG.
pub fn png(rgb: [u8; 3]) -> Vec<u8> {
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let crc = crc32fast::hash(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&[0, rgb[0], rgb[1], rgb[2]]).unwrap();
    let pixels = encoder.finish().unwrap();
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut out, b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0]);
    chunk(&mut out, b"IDAT", &pixels);
    chunk(&mut out, b"IEND", &[]);
    out
}
