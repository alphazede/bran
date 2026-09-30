//! Issue #46: exercise the real CLI against the committed document corpus.
use bran_document::canonical::{sha256_hex, Json};
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
        self.package(
            "fixtures/enterprise-documents/conformance/xlsx-base.parts",
            "ledger.xlsx",
            replacement.map(|text| ("Synthetic label", text)),
        )
    }

    fn package(&self, fixture: &str, filename: &str, patch: Option<(&str, &str)>) -> Vec<u8> {
        // Python is already required by the offline contract gate. Build a
        // native package from the same reviewable parts as adapter tests.
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../{fixture}"));
        let (from, to) = patch.unwrap_or_default();
        let output = Command::new("python3")
            .args([
                "-c",
                "import io,pathlib,sys,zipfile\nparts={}\nfor line in pathlib.Path(sys.argv[1]).read_text().splitlines():\n if line.startswith('--- '): name=line[4:]; parts[name]=[]\n else: parts[name].append(line)\nout=io.BytesIO()\nwith zipfile.ZipFile(out,'w',compression=zipfile.ZIP_DEFLATED) as archive:\n for name,lines in parts.items():\n  data='\\n'.join(lines)\n  if sys.argv[2]: data=data.replace(sys.argv[2],sys.argv[3])\n  archive.writestr(zipfile.ZipInfo(name,(2026,1,1,0,0,0)),data.encode())\nsys.stdout.buffer.write(out.getvalue())",
            ])
            .arg(fixture)
            .arg(from)
            .arg(to)
            .output()
            .expect("offline corpus ZIP builder runs");
        assert!(output.status.success(), "ZIP builder failed");
        fs::write(self.0.join(filename), &output.stdout).unwrap();
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

    fn review_input(&self, name: &str, filename: &str) {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../fixtures/enterprise-documents/admission-review/{name}.fixture"
        ));
        let fixture = fs::read_to_string(fixture).unwrap();
        let mut lines = fixture.lines();
        let digest = lines.next().unwrap();
        let length: usize = lines.next().unwrap().parse().unwrap();
        let output = Command::new("python3")
            .args([
                "-c",
                "import sys,zlib;sys.stdout.buffer.write(zlib.decompress(bytes.fromhex(sys.argv[1]),-15))",
            ])
            .arg(lines.collect::<String>())
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout.len(), length);
        assert_eq!(sha256_hex(&output.stdout), digest);
        fs::write(self.0.join(filename), output.stdout).unwrap();
    }
}

impl Drop for Repository {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn review_canary() -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/public-boundary/rejected/synthetic-canaries.txt"),
    )
    .unwrap()
    .lines()
    .next()
    .unwrap()
    .to_owned()
}

#[test]
fn document_review_locator_dlp_refuses_admission() {
    let repo = Repository::new("review-locator");
    let filename = format!("{}.docx", review_canary());
    repo.review_input("filename", &filename);
    for args in [
        vec!["query", ".", "Synthetic"],
        vec!["packet", ".", "Synthetic"],
        vec!["document", "inspect", ".", &filename],
    ] {
        let (code, output) = repo.run(&args);
        assert_eq!(code, 0);
        assert!(
            output.contains("\"code\":\"dlp-findings\""),
            "unsafe locator admitted"
        );
        assert!(!output.contains("\"envelope\":"));
        if args[0] != "document" {
            assert!(output.contains("\"sources\":[],\"matches\":[]"));
        }
    }
}

#[test]
fn document_review_refusals_do_not_echo_unsafe_paths() {
    let canary = review_canary();
    for (name, filename, prohibited, refusal) in [
        ("both", format!("{canary}.docx"), canary, "dlp-findings"),
        (
            "boundary",
            "private_key_SYNTHETIC.docx".to_owned(),
            "private_key".to_owned(),
            "public-boundary-violation",
        ),
    ] {
        let repo = Repository::new(&format!("review-refusal-{name}"));
        repo.review_input(name, &filename);
        for args in [
            vec!["query", ".", "Synthetic"],
            vec!["packet", ".", "Synthetic"],
            vec!["document", "inspect", ".", &filename],
        ] {
            let (code, output) = repo.run(&args);
            assert_eq!(code, 0);
            assert!(output.contains(&format!("\"code\":\"{refusal}\"")));
            assert!(
                !output.contains(&prohibited),
                "refusal echoed prohibited path"
            );
        }
    }
}

#[test]
fn document_review_packet_excerpts_obey_and_account_for_budgets() {
    let repo = Repository::new("review-budget");
    repo.review_input("budget", "memo.docx");
    fs::write(
        repo.0.join(".bran/settings.conf"),
        include_str!("../../../fixtures/enterprise-documents/admission-review/settings.conf"),
    )
    .unwrap();
    for args in [
        vec!["packet", "--excerpt-bytes", "1", ".", "Synthetic"],
        vec!["packet", ".", "Synthetic"],
        vec![
            "packet",
            "--excerpt-bytes",
            "1",
            ".",
            "Synthetic",
            "missingterm",
        ],
    ] {
        let (code, output) = repo.run(&args);
        assert_eq!(code, 0, "{output}");
        let result = Json::parse(output.as_bytes()).unwrap();
        let data = result.get("data").unwrap();
        let Some(Json::Arr(matches)) = data.get("document_evidence").unwrap().get("matches") else {
            panic!("missing matches")
        };
        let mut bytes = 0;
        for matched in matches {
            let Some(Json::Str(excerpt)) = matched.get("excerpt") else {
                panic!("missing excerpt")
            };
            bytes += excerpt.len();
            if args.contains(&"--excerpt-bytes") {
                assert!(
                    excerpt.len() <= 1,
                    "document excerpt bypassed byte limit: {}",
                    excerpt.len()
                );
            }
        }
        let Some(Json::Int(estimated)) = data.get("estimated_tokens") else {
            panic!("missing estimate")
        };
        assert!(
            *estimated <= 100,
            "document excerpts bypassed token ceiling"
        );
        assert_eq!(
            data.get("excerpt_bytes"),
            Some(&Json::Int(bytes as i64)),
            "document excerpts absent from accounting"
        );
        assert_eq!(
            data.get("encoded_packet_bytes"),
            Some(&Json::Int(bytes as i64))
        );
        assert_eq!(data.get("raw_bytes"), Some(&Json::Int(bytes as i64)));
        assert_eq!(*estimated, bytes.div_ceil(4) as i64);
        assert_eq!(
            result.get("metrics").unwrap().get("estimated_tokens"),
            Some(&Json::Int(*estimated))
        );
        assert_eq!(data.get("truncated"), Some(&Json::Bool(true)));
    }
}

#[test]
fn document_review_unrepresentable_envelope_is_refused() {
    let repo = Repository::new("review-envelope");
    repo.review_input("many", "many.docx");
    for args in [
        vec!["document", "inspect", ".", "many.docx"],
        vec!["query", ".", "needle"],
        vec!["packet", ".", "needle"],
    ] {
        let (code, output) = repo.run(&args);
        assert_eq!(code, 0);
        assert!(
            output.contains("\"code\":\"oversized\""),
            "schema-invalid envelope admitted"
        );
        assert!(!output.contains("\"envelope\":"));
        if args[0] != "document" {
            assert!(output.contains("\"sources\":[],\"matches\":[]"));
        }
    }
}

#[test]
fn document_review_matches_contribute_to_query_coverage() {
    let repo = Repository::new("review-coverage");
    for format in ["docx", "xlsx", "pptx"] {
        repo.review_input(&format!("positive-{format}"), &format!("memo.{format}"));
    }
    let second = Repository::new("review-coverage-second");
    for args in [
        vec!["query", ".", "Synthetic"],
        vec!["packet", ".", "Synthetic"],
        vec![
            "query",
            ".",
            "--add-dir",
            second.0.to_str().unwrap(),
            "Synthetic",
        ],
    ] {
        let (code, output) = repo.run(&args);
        assert_eq!(code, 0, "{output}");
        assert!(
            output.contains("\"query_outcome\":\"grounded\""),
            "document matches reported as miss"
        );
        assert!(output.contains(
            "\"query_coverage\":{\"matched_terms\":[\"synthetic\"],\"unmatched_terms\":[]}"
        ));
        assert!(!output.contains("unmatched_query_terms"));
        let result = Json::parse(output.as_bytes()).unwrap();
        let Some(Json::Arr(matches)) = result
            .get("data")
            .unwrap()
            .get("document_evidence")
            .unwrap()
            .get("matches")
        else {
            panic!("missing matches")
        };
        assert_eq!(matches.len(), 5);
    }
    let (_, partial) = repo.run(&["query", ".", "Synthetic missingterm"]);
    assert!(partial.contains("\"query_outcome\":\"partial_unanchored\""));
    assert!(
        partial.contains("\"matched_terms\":[\"synthetic\"],\"unmatched_terms\":[\"missingterm\"]")
    );
    let (_, miss) = repo.run(&["query", ".", "missingterm"]);
    assert!(miss.contains("\"query_outcome\":\"miss\""));
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
    for extension in ["docx", "pptx", "xlsx", "pdf"] {
        let filename = format!("malformed.{extension}");
        fs::write(repo.0.join(&filename), b"not a ZIP container").unwrap();
        let refusal = if extension == "pdf" {
            "malformed-pdf"
        } else {
            "malformed-container"
        };
        for command in ["query", "packet"] {
            let (code, output) = repo.run(&[command, ".", "malformed"]);
            assert_eq!(code, 0, "{output}");
            assert!(output.contains("\"document_evidence\":"), "{output}");
            assert!(
                output.contains(&format!("\"code\":\"{refusal}\"")),
                "{output}"
            );
            assert!(output.contains("\"unsupported\":[]"), "{output}");
            assert!(output.contains("\"sources\":[],\"matches\":[]"), "{output}");
            assert!(output.contains("\"selected_locators\":[]"), "{output}");
            assert!(!output.contains("not a ZIP container"), "{output}");
        }
        fs::remove_file(repo.0.join(filename)).unwrap();
    }
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

fn assert_document_evidence(
    repo: &Repository,
    filename: &str,
    source_type: &str,
    query: &str,
    anchor: &str,
    native_locator: &str,
    fidelity: &str,
) {
    let bytes = fs::read(repo.0.join(filename)).unwrap();
    let fidelity = Json::parse(fidelity.as_bytes()).unwrap();
    for command in ["query", "packet"] {
        let (code, output) = repo.run(&[command, ".", query]);
        assert_eq!(code, 0, "{output}");
        let result = Json::parse(output.as_bytes()).unwrap();
        let evidence = result
            .get("data")
            .unwrap()
            .get("document_evidence")
            .unwrap();
        assert_eq!(evidence.get("unsupported"), Some(&Json::Arr(Vec::new())));
        assert_eq!(evidence.get("refusals"), Some(&Json::Arr(Vec::new())));
        let Some(Json::Arr(matches)) = evidence.get("matches") else {
            panic!("missing matches: {output}");
        };
        let matched = matches
            .iter()
            .find(|value| value.get("anchor") == Some(&Json::Str(anchor.to_owned())))
            .unwrap_or_else(|| panic!("missing {anchor}: {output}"));
        for (field, value) in [
            ("path", filename.to_owned()),
            ("source_type", source_type.to_owned()),
            ("native_locator", native_locator.to_owned()),
            ("source_digest", sha256_hex(&bytes)),
            ("derivation", "embedded".to_owned()),
        ] {
            assert_eq!(matched.get(field), Some(&Json::Str(value)), "{output}");
        }
        assert_eq!(matched.get("fidelity"), Some(&fidelity), "{output}");
        assert_eq!(matched.get("rank"), Some(&Json::Int(1)), "{output}");
        if command == "packet" {
            assert_eq!(matched.get("excerpt"), Some(&Json::Str(query.to_owned())));
        }
        assert_eq!(repo.run(&[command, ".", query]), (code, output));
    }
    let (code, output) = repo.run(&["document", "inspect", ".", filename]);
    assert_eq!(code, 0, "{output}");
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut validator = Command::new("python3")
        .args([
            "-c",
            "import json,pathlib,sys\nroot=pathlib.Path(sys.argv[1])\nsys.path.insert(0,str(root/'tools/ci'))\nimport enterprise_contract_check as oracle\nimport test_budget_check as schema_validator\nenvelope=json.load(sys.stdin)['data']['envelope']\nschema=json.loads((root/'schemas/enterprise-document-evidence-envelope.schema.json').read_text())\nerrors=schema_validator.validate_instance(envelope,schema,schema)\nassert not errors,errors\nassert oracle.classify(envelope) is None\nassert envelope['source']['locator']==sys.argv[2]\n",
        ])
        .arg(repository)
        .arg(filename)
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
    assert_eq!(fs::read(repo.0.join(filename)).unwrap(), bytes);
}

#[test]
fn document_query_and_packet_return_native_docx_anchors_deterministically() {
    let repo = Repository::new("docx");
    repo.package(
        "fixtures/enterprise-documents/conformance/docx-base.parts",
        "memo.docx",
        None,
    );
    assert_document_evidence(
        &repo,
        "memo.docx",
        "docx",
        "Synthetic heading",
        "docx:s1:b1",
        "s1#1",
        r#"{"text":"normalized","paragraphs":"normalized","headings":"normalized","lists":"normalized","tables":"normalized","headers_footers":"unsupported","macros":"unsupported"}"#,
    );
}

#[test]
fn document_query_and_packet_return_native_pptx_anchors_deterministically() {
    let repo = Repository::new("pptx");
    // The base corpus omits the native shape id. Supply one for this
    // positive case; the missing-id refusal is covered separately.
    repo.package(
        "fixtures/enterprise-documents/conformance/pptx-base.parts",
        "deck.pptx",
        Some((
            "<p:sp>",
            "<p:sp><p:nvSpPr><p:cNvPr id=\"2\" name=\"Title\"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>",
        )),
    );
    assert_document_evidence(
        &repo,
        "deck.pptx",
        "pptx",
        "Synthetic slide title",
        "anc:pptx:slide-256:shape-2",
        "slide 1 shape 2",
        r#"{"text":"normalized","slides":"exact","shapes":"normalized","speaker_notes":"normalized","z_order":"exact","macros":"unsupported","animations":"unsupported"}"#,
    );
}

#[test]
fn document_packet_refuses_pptx_without_native_shape_identity() {
    let repo = Repository::new("pptx-missing-shape");
    repo.package(
        "fixtures/enterprise-documents/conformance/pptx-base.parts",
        "deck.pptx",
        None,
    );
    let (code, output) = repo.run(&["packet", ".", "deck"]);
    assert_eq!(code, 0, "{output}");
    assert!(
        output.contains("\"code\":\"unsupported-container\""),
        "{output}"
    );
    assert!(output.contains("\"sources\":[],\"matches\":[]"), "{output}");
    assert!(!output.contains("Synthetic slide title"), "{output}");
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
    repo.package(
        "fixtures/enterprise-documents/conformance/docx-base.parts",
        "memo.docx",
        Some(("Synthetic heading", canary)),
    );
    repo.package(
        "fixtures/enterprise-documents/conformance/pptx-base.parts",
        "deck.pptx",
        Some(("Synthetic slide title", canary)),
    );
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/enterprise-documents/conformance/pdf-base.objects");
    let pdf = classic_pdf(
        &fs::read_to_string(fixture)
            .unwrap()
            .replace("Synthetic PDF heading", canary),
    );
    fs::write(repo.0.join("memo.pdf"), pdf).unwrap();
    let (code, output) = repo.run(&["packet", ".", "ledger"]);
    assert_eq!(code, 0, "{output}");
    assert!(output.contains("\"code\":\"dlp-findings\""), "{output}");
    assert!(output.contains("\"selected_locators\":[]"), "{output}");
    assert!(output.contains("\"sources\":[],\"matches\":[]"), "{output}");
    let result = Json::parse(output.as_bytes()).unwrap();
    let Some(Json::Arr(refusals)) = result
        .get("data")
        .unwrap()
        .get("document_evidence")
        .unwrap()
        .get("refusals")
    else {
        panic!("missing refusals: {output}");
    };
    for filename in ["ledger.xlsx", "memo.docx", "deck.pptx", "memo.pdf"] {
        assert!(
            refusals.iter().any(|refusal| {
                refusal.get("path") == Some(&Json::Str(filename.to_owned()))
                    && refusal.get("code") == Some(&Json::Str("dlp-findings".to_owned()))
            }),
            "missing DLP refusal for {filename}: {output}"
        );
    }
    assert!(!output.contains(canary), "DLP input was reflected");
}
