//! Shared enterprise-document intake and conformance harness (issue #25).
//!
//! DOCX, XLSX, PPTX, and PDF adapters (#21, #26, #22, #23) build on this crate
//! so every format applies one package-safety, XML-safety, DLP, and export
//! policy. Nothing here reads the network, runs a process, or executes content.
//! Every limit counts bytes, parts, or nodes, never wall-clock time, so the
//! same input and limits give the same outcome on every machine.

pub mod admit;
pub mod canonical;
pub mod conformance;
pub mod export;
pub mod opc;
pub mod pdf;
mod pdf_syntax;
mod pdf_text;
pub mod xlsx;
pub mod xml;
pub mod zip;

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Version of the package receipt and refusal vocabulary below.
pub const RECEIPT_VERSION: &str = "1";

/// A typed, bounded refusal. Codes are stable and never echo input bytes.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Refusal {
    MalformedContainer,
    UnsupportedContainer,
    Encrypted,
    UnsafePartPath,
    DuplicatePart,
    Oversized,
    TooManyParts,
    DecompressionLimit,
    MalformedXml,
    XmlDtdRefused,
    XmlDepthLimit,
    XmlNodeLimit,
    ActiveContent,
    ExternalReference,
    Cancelled,
    DlpFindings,
    PublicBoundary,
    ExportFormatMismatch,
    ExportContainment,
    ExportExists,
    ExportIo,
    ExportUnsupported,
    PdfMalformed,
    PdfDepthLimit,
    PdfObjectLimit,
    PdfPageLimit,
    NoAdmissibleText,
}

impl Refusal {
    pub const fn code(self) -> &'static str {
        match self {
            Self::MalformedContainer => "malformed-container",
            Self::UnsupportedContainer => "unsupported-container",
            Self::Encrypted => "encrypted-or-legacy-container",
            Self::UnsafePartPath => "unsafe-part-path",
            Self::DuplicatePart => "duplicate-part",
            Self::Oversized => "oversized",
            Self::TooManyParts => "too-many-parts",
            Self::DecompressionLimit => "decompression-limit",
            Self::MalformedXml => "malformed-xml",
            Self::XmlDtdRefused => "xml-dtd-refused",
            Self::XmlDepthLimit => "xml-depth-limit",
            Self::XmlNodeLimit => "xml-node-limit",
            Self::ActiveContent => "active-content",
            Self::ExternalReference => "external-reference",
            Self::Cancelled => "cancelled",
            Self::DlpFindings => "dlp-findings",
            Self::PublicBoundary => "public-boundary-violation",
            Self::ExportFormatMismatch => "export-format-mismatch",
            Self::ExportContainment => "export-containment",
            Self::ExportExists => "export-exists",
            Self::ExportIo => "export-io",
            Self::ExportUnsupported => "export-unsupported",
            Self::PdfMalformed => "malformed-pdf",
            Self::PdfDepthLimit => "pdf-depth-limit",
            Self::PdfObjectLimit => "pdf-object-limit",
            Self::PdfPageLimit => "pdf-page-limit",
            Self::NoAdmissibleText => "no-admissible-text",
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for Refusal {}

/// Byte, part, and node budgets. Defaults follow the evidence envelope's
/// 20 MiB original-document budget. One part may not exceed 16 MiB, so a
/// stored asset can trip the part budget before the package budget.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Limits {
    pub max_package_bytes: u64,
    pub max_parts: usize,
    pub max_part_bytes: u64,
    pub max_total_bytes: u64,
    /// Largest allowed uncompressed/compressed size ratio for one part.
    pub max_ratio: u64,
    pub max_xml_depth: usize,
    pub max_xml_nodes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_package_bytes: 20 * 1024 * 1024,
            max_parts: 4096,
            max_part_bytes: 16 * 1024 * 1024,
            max_total_bytes: 128 * 1024 * 1024,
            max_ratio: 100,
            max_xml_depth: 256,
            max_xml_nodes: 1_000_000,
        }
    }
}

/// Caller-owned cancellation. Checked before every ZIP entry and XML part, so
/// a cancelled run returns `Refusal::Cancelled` and no partial result.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn check(&self) -> Result<(), Refusal> {
        if self.0.load(Ordering::SeqCst) {
            Err(Refusal::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// The four enterprise formats and the adapter issue that owns each.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Format {
    Docx,
    Xlsx,
    Pptx,
    Pdf,
}

impl Format {
    pub const ALL: [Format; 4] = [Format::Docx, Format::Xlsx, Format::Pptx, Format::Pdf];

    pub const fn extension(self) -> &'static str {
        match self {
            Self::Docx => "docx",
            Self::Xlsx => "xlsx",
            Self::Pptx => "pptx",
            Self::Pdf => "pdf",
        }
    }

    pub const fn adapter_issue(self) -> u32 {
        match self {
            Self::Docx => 21,
            Self::Xlsx => 26,
            Self::Pptx => 22,
            Self::Pdf => 23,
        }
    }
}
