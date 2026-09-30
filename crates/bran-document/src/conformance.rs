//! Conformance harness that every enterprise-document adapter registers with.

use crate::{opc, Cancel, Format, Limits, Refusal};
use std::panic::{catch_unwind, AssertUnwindSafe};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Anchor {
    pub id: String,
    pub text_digest: String,
}

/// What an adapter returns: canonical bytes, its versioned fidelity receipt
/// codes, and the citation anchors a round trip must preserve.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Imported {
    pub canonical: Vec<u8>,
    pub receipt: Vec<String>,
    pub anchors: Vec<Anchor>,
}

pub trait Adapter: Sync {
    fn format(&self) -> Format;
    fn import(&self, bytes: &[u8], limits: &Limits, cancel: &Cancel) -> Result<Imported, Refusal>;
    fn export(&self, _imported: &Imported) -> Result<Vec<u8>, Refusal> {
        Err(Refusal::ExportUnsupported)
    }
}

/// Adapters register here. The shared corpus runs every row of a format
/// against every adapter registered for that format.
pub fn registered() -> Vec<&'static dyn Adapter> {
    vec![&crate::xlsx::Xlsx, &crate::pdf::PdfAdapter]
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Expect {
    Admit(Vec<&'static str>),
    Refuse(Refusal),
}

/// Result of one passing check. `round_trip` is false when export refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Outcome {
    pub round_trip: bool,
}

/// The package intake exposed as an adapter, so package rows run through the
/// same checks as format adapters.
pub struct PackageIntake(pub Format);

impl Adapter for PackageIntake {
    fn format(&self) -> Format {
        self.0
    }

    fn import(&self, bytes: &[u8], limits: &Limits, cancel: &Cancel) -> Result<Imported, Refusal> {
        let package = opc::open(bytes, limits, cancel)?;
        Ok(Imported {
            canonical: package.canonical_bytes(),
            receipt: package
                .diagnostics
                .iter()
                .map(|code| (*code).to_owned())
                .collect(),
            anchors: Vec::new(),
        })
    }
}

/// Runs one row against one adapter: no panic, repeatable result, the
/// expected refusal or receipt codes, re-encoding invariance, and, when the
/// adapter exports, an anchor-preserving deterministic round trip.
pub fn check(
    adapter: &dyn Adapter,
    input: &[u8],
    variants: &[Vec<u8>],
    expect: &Expect,
    limits: &Limits,
    cancel: &Cancel,
) -> Result<Outcome, String> {
    let run = |bytes: &[u8]| {
        catch_unwind(AssertUnwindSafe(|| adapter.import(bytes, limits, cancel)))
            .map_err(|_| "adapter panicked".to_owned())
    };
    let first = run(input)?;
    if run(input)? != first {
        return Err("nondeterministic: two imports of the same bytes differ".to_owned());
    }
    let imported = match (expect, first) {
        (Expect::Refuse(expected), Err(actual)) if actual == *expected => {
            return Ok(Outcome { round_trip: false })
        }
        (Expect::Refuse(expected), Err(actual)) => {
            return Err(format!(
                "expected refusal {expected} but got refusal {actual}"
            ))
        }
        (Expect::Refuse(expected), Ok(_)) => {
            return Err(format!(
                "expected refusal {expected} but the input was admitted"
            ))
        }
        (Expect::Admit(_), Err(actual)) => {
            return Err(format!("expected admission but got refusal {actual}"))
        }
        (Expect::Admit(codes), Ok(imported)) => {
            if let Some(code) = codes
                .iter()
                .find(|code| !imported.receipt.iter().any(|r| r == *code))
            {
                return Err(format!("receipt is missing {code}"));
            }
            imported
        }
    };
    for (index, variant) in variants.iter().enumerate() {
        if run(variant)? != Ok(imported.clone()) {
            return Err(format!("re-encoding {index} changed the import result"));
        }
    }
    let export = || {
        catch_unwind(AssertUnwindSafe(|| adapter.export(&imported)))
            .map_err(|_| "export panicked".to_owned())
    };
    let exported = match export()? {
        Err(_) => return Ok(Outcome { round_trip: false }),
        Ok(bytes) => bytes,
    };
    if export()? != Ok(exported.clone()) {
        return Err("nondeterministic: two exports of the same import differ".to_owned());
    }
    let again =
        run(&exported)?.map_err(|refusal| format!("re-import of the export refused: {refusal}"))?;
    if again.anchors != imported.anchors {
        return Err("round trip changed the citation anchors".to_owned());
    }
    Ok(Outcome { round_trip: true })
}
