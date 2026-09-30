//! Issue #46: exercise the real CLI against the committed XLSX corpus.
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const BRAN: &str = env!("CARGO_BIN_EXE_bran");

struct Repository(PathBuf);

impl Repository {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "bran-document-admission-{name}-{}",
            std::process::id()
        ));
        fs::create_dir_all(root.join(".bran")).unwrap();
        fs::write(root.join(".bran/policy.yaml"), "schema_version: \"1\"\n").unwrap();
        Self(root)
    }

    fn workbook(&self, replacement: Option<&str>) -> Vec<u8> {
        // Python is already required by the offline contract gate. Build a
        // native workbook from the same reviewable parts as adapter tests.
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/enterprise-documents/conformance/xlsx-base.parts");
        let output = Command::new("python3")
            .args([
                "-c",
                "import io,pathlib,sys,zipfile\nparts={}\nfor line in pathlib.Path(sys.argv[1]).read_text().splitlines():\n if line.startswith('--- '): name=line[4:]; parts[name]=[]\n else: parts[name].append(line)\nout=io.BytesIO()\nwith zipfile.ZipFile(out,'w',compression=zipfile.ZIP_DEFLATED) as archive:\n for name,lines in parts.items():\n  data='\\n'.join(lines)\n  if sys.argv[2]: data=data.replace('Synthetic label',sys.argv[2])\n  archive.writestr(zipfile.ZipInfo(name,(2026,1,1,0,0,0)),data.encode())\nsys.stdout.buffer.write(out.getvalue())",
            ])
            .arg(fixture)
            .arg(replacement.unwrap_or_default())
            .output()
            .expect("offline corpus ZIP builder runs");
        assert!(output.status.success(), "ZIP builder failed");
        fs::write(self.0.join("ledger.xlsx"), &output.stdout).unwrap();
        output.stdout
    }

    fn run(&self, args: &[&str]) -> (i32, String) {
        let output = Command::new(BRAN)
            .env_remove("BRAN_REGISTERED_ROOTS")
            .current_dir(&self.0)
            .args(args)
            .output()
            .expect("bran runs");
        (
            output.status.code().unwrap_or(-1),
            format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
        )
    }
}

impl Drop for Repository {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

/// Minimal classic-layout PDF writer over the shared `.objects` fixture (the
/// same reviewable parts the adapter corpus builds from).
fn classic_pdf(fixture: &str) -> Vec<u8> {
    let mut trailer = String::new();
    let mut objects: Vec<(u32, Option<String>, Vec<u8>)> = Vec::new();
    let mut lines = fixture.lines();
    while let Some(line) = lines.next() {
        if let Some(header) = line.strip_prefix("--- ") {
            if header == "trailer" {
                trailer = lines.next().expect("trailer entries").to_owned();
                continue;
            }
            let (number, kind) = header.split_once(' ').unwrap_or((header, ""));
            let dict = if kind == "stream" {
                Some(lines.next().expect("stream dictionary").to_owned())
            } else {
                None
            };
            objects.push((number.parse().expect("object number"), dict, Vec::new()));
            continue;
        }
        let body = &mut objects.last_mut().expect("object header first").2;
        if !body.is_empty() {
            body.push(b'\n');
        }
        body.extend_from_slice(line.as_bytes());
    }
    let mut out = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::new();
    for (number, dict, data) in &objects {
        offsets.push((*number, out.len()));
        out.extend(format!("{number} 0 obj\n").bytes());
        match dict {
            None => out.extend_from_slice(data),
            Some(dict) => {
                let dict = dict.replacen("<<", &format!("<< /Length {}", data.len()), 1);
                out.extend(format!("{dict}\nstream\n").bytes());
                out.extend_from_slice(data);
                out.extend(b"\nendstream");
            }
        }
        out.extend(b"\nendobj\n");
    }
    offsets.sort();
    let size = objects.iter().map(|(n, _, _)| *n).max().unwrap_or(0) + 1;
    let start = out.len();
    out.extend(b"xref\n0 1\n0000000000 65535 f \n");
    for (number, offset) in offsets {
        out.extend(format!("{number} 1\n{offset:010} 00000 n \n").bytes());
    }
    out.extend(
        format!("trailer\n<< /Size {size} {trailer} >>\nstartxref\n{start}\n%%EOF\n").bytes(),
    );
    out
}

#[test]
fn document_inspection_is_read_only_and_schema_valid() {
    let repo = Repository::new("inspect");
    let bytes = repo.workbook(None);
    let (code, output) = repo.run(&["document", "inspect", ".", "ledger.xlsx"]);
    assert_eq!(code, 0, "{output}");
    assert!(output.contains("\"envelope\":"), "{output}");
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut validator = Command::new("python3")
        .args([
            "-c",
            "import json,pathlib,sys\nroot=pathlib.Path(sys.argv[1])\nsys.path.insert(0,str(root/'tools/ci'))\nimport enterprise_contract_check as oracle\nimport test_budget_check as schema_validator\nenvelope=json.load(sys.stdin)['data']['envelope']\nschema=json.loads((root/'schemas/enterprise-document-evidence-envelope.schema.json').read_text())\nerrors=schema_validator.validate_instance(envelope,schema,schema)\nassert not errors,errors\nassert oracle.classify(envelope) is None\nassert envelope['source']['locator']=='ledger.xlsx'\n",
        ])
        .arg(repository)
        .stdin(Stdio::piped())
        .spawn()
        .expect("offline envelope validator runs");
    validator
        .stdin
        .take()
        .unwrap()
        .write_all(output.as_bytes())
        .unwrap();
    assert!(validator.wait().unwrap().success(), "invalid envelope");
    assert_eq!(fs::read(repo.0.join("ledger.xlsx")).unwrap(), bytes);
    assert_eq!(fs::read_dir(&repo.0).unwrap().count(), 2);
    assert_eq!(fs::read_dir(repo.0.join(".bran")).unwrap().count(), 1);
    assert_eq!(
        repo.run(&["document", "inspect", ".", "ledger.xlsx"]),
        (code, output)
    );
}

#[test]
fn document_query_returns_native_xlsx_anchors_deterministically() {
    let repo = Repository::new("query");
    repo.workbook(None);
    let (code, output) = repo.run(&["query", ".", "Synthetic label"]);
    assert_eq!(code, 0, "{output}");
    for field in [
        "\"document_evidence\":",
        "\"source_type\":\"xlsx\"",
        "\"native_locator\":",
        "\"source_digest\":",
        "\"fidelity\":",
        "\"derivation\":\"embedded\"",
        "anc:xlsx:s1:r1c1",
    ] {
        assert!(output.contains(field), "missing {field}: {output}");
    }
    assert_eq!(repo.run(&["query", ".", "Synthetic label"]), (code, output));
}

#[test]
fn document_packet_selects_native_xlsx_anchors_deterministically() {
    let repo = Repository::new("packet");
    repo.workbook(None);
    let (code, output) = repo.run(&["packet", ".", "Synthetic label"]);
    assert_eq!(code, 0, "{output}");
    for field in [
        "\"document_evidence\":",
        "\"source_type\":\"xlsx\"",
        "\"native_locator\":",
        "\"source_digest\":",
        "\"fidelity\":",
        "\"derivation\":\"embedded\"",
        "anc:xlsx:s1:r1c1",
        "Synthetic label",
    ] {
        assert!(output.contains(field), "missing {field}: {output}");
    }
    assert_eq!(
        repo.run(&["packet", ".", "Synthetic label"]),
        (code, output)
    );
}

#[test]
fn document_packet_refuses_malformed_documents_with_typed_reason() {
    let repo = Repository::new("refused");
    fs::write(repo.0.join("ledger.xlsx"), b"not a ZIP container").unwrap();
    let (code, output) = repo.run(&["packet", ".", "ledger"]);
    assert_eq!(code, 0, "{output}");
    assert!(output.contains("\"document_evidence\":"), "{output}");
    assert!(
        output.contains("\"code\":\"malformed-container\""),
        "{output}"
    );
    assert!(output.contains("\"selected_locators\":[]"), "{output}");
    assert!(!output.contains("not a ZIP container"), "{output}");
}

#[test]
fn document_outputs_unchanged_without_document_evidence() {
    let repo = Repository::new("unchanged");
    fs::write(
        repo.0.join("notes.md"),
        "---\ntype: note\n---\n# Notes\n\nSynthetic label\n",
    )
    .unwrap();
    for args in [
        ["query", ".", "Synthetic label"].as_slice(),
        ["packet", ".", "Synthetic label"].as_slice(),
    ] {
        let (code, output) = repo.run(args);
        assert_eq!(code, 0, "{output}");
        assert!(
            !output.contains("document_evidence"),
            "member leaked: {output}"
        );
        assert_eq!(repo.run(args), (code, output));
    }
}

#[test]
fn document_query_reports_unsupported_formats() {
    let repo = Repository::new("unsupported");
    fs::write(repo.0.join("draft.docx"), b"placeholder").unwrap();
    let (code, output) = repo.run(&["query", ".", "draft"]);
    assert_eq!(code, 0, "{output}");
    assert!(
        output.contains("\"unsupported\":[{\"path\":\"draft.docx\",\"source_type\":\"docx\"}]"),
        "{output}"
    );
    assert!(output.contains("\"refusals\":[]"), "{output}");
    assert_eq!(repo.run(&["query", ".", "draft"]), (code, output));
}

#[test]
fn document_packet_selects_native_pdf_anchors() {
    let repo = Repository::new("pdf");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/enterprise-documents/conformance/pdf-base.objects");
    let bytes = classic_pdf(&fs::read_to_string(fixture).unwrap());
    fs::write(repo.0.join("memo.pdf"), &bytes).unwrap();
    let (code, output) = repo.run(&["packet", ".", "Synthetic PDF heading"]);
    assert_eq!(code, 0, "{output}");
    for field in [
        "\"document_evidence\":",
        "\"source_type\":\"pdf\"",
        "\"native_locator\":\"page 1 block 1\"",
        "\"source_digest\":",
        "\"fidelity\":",
        "\"derivation\":\"embedded\"",
        "pdf:p1:b1",
        "Synthetic PDF heading",
    ] {
        assert!(output.contains(field), "missing {field}: {output}");
    }
    assert_eq!(
        repo.run(&["packet", ".", "Synthetic PDF heading"]),
        (code, output)
    );
    let (code, output) = repo.run(&["document", "inspect", ".", "memo.pdf"]);
    assert_eq!(code, 0, "{output}");
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut validator = Command::new("python3")
        .args([
            "-c",
            "import json,pathlib,sys\nroot=pathlib.Path(sys.argv[1])\nsys.path.insert(0,str(root/'tools/ci'))\nimport enterprise_contract_check as oracle\nimport test_budget_check as schema_validator\nenvelope=json.load(sys.stdin)['data']['envelope']\nschema=json.loads((root/'schemas/enterprise-document-evidence-envelope.schema.json').read_text())\nerrors=schema_validator.validate_instance(envelope,schema,schema)\nassert not errors,errors\nassert oracle.classify(envelope) is None\nassert envelope['source']['locator']=='memo.pdf'\nassert envelope['normalized']['content']['family']=='fixed-layout'\n",
        ])
        .arg(repository)
        .stdin(Stdio::piped())
        .spawn()
        .expect("offline envelope validator runs");
    validator
        .stdin
        .take()
        .unwrap()
        .write_all(output.as_bytes())
        .unwrap();
    assert!(validator.wait().unwrap().success(), "invalid envelope");
}

#[test]
fn document_packet_refuses_dlp_findings_without_reflecting_them() {
    let repo = Repository::new("dlp");
    let canaries = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/public-boundary/rejected/synthetic-canaries.txt"),
    )
    .unwrap();
    let canary = canaries.lines().find(|line| !line.is_empty()).unwrap();
    repo.workbook(Some(canary));
    let (code, output) = repo.run(&["packet", ".", "ledger"]);
    assert_eq!(code, 0, "{output}");
    assert!(output.contains("\"code\":\"dlp-findings\""), "{output}");
    assert!(output.contains("\"selected_locators\":[]"), "{output}");
    assert!(!output.contains(canary), "DLP input was reflected");
}
