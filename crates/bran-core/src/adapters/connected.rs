//! Packet-only Connected synthesis and post-grounding SQZ coordination.

use super::sqz::{SqzAdapter, SqzAdapterConfig, SqzFailureReason, SqzPort, SqzReceipt, SqzStatus};
use crate::agent::result_store::ResultId;
use crate::agent::runtime::ProviderTokenUsage;
use crate::packet::{ContextPacket, PacketReceipt, PreservationAnchor, StructuralPacket};
use std::collections::{BTreeMap, BTreeSet};

const MAX_QUERY: usize = 1_024;
const MAX_ID: usize = 40;
const MAX_TEXT: usize = 512;
const MAX_RESPONSE_ITEMS: usize = 64;

/// This is deliberately a denial-only manifest: Connected receives no tools.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectedCapabilities {
    pub search: bool,
    pub filesystem: bool,
    pub shell: bool,
    pub network: bool,
    pub mutation: bool,
    pub recursive_delegation: bool,
}
impl ConnectedCapabilities {
    pub const DENIED: Self = Self {
        search: false,
        filesystem: false,
        shell: false,
        network: false,
        mutation: false,
        recursive_delegation: false,
    };
    pub const fn denied(self) -> bool {
        !self.search
            && !self.filesystem
            && !self.shell
            && !self.network
            && !self.mutation
            && !self.recursive_delegation
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectedRequest {
    pub packet: StructuralPacket,
    pub query: String,
    pub capabilities: ConnectedCapabilities,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectedClaim {
    pub id: String,
    pub text: String,
    pub material: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectedCitation {
    pub id: String,
    pub claim_id: String,
    pub node_id: String,
    pub locator: String,
    pub content_digest: String,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectedUsage {
    pub id: String,
    pub node_id: String,
    pub locator: String,
    pub content_digest: String,
    pub support: String,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectedResponse {
    pub claims: Vec<ConnectedClaim>,
    pub citations: Vec<ConnectedCitation>,
    pub usages: Vec<ConnectedUsage>,
    pub conflicts: Vec<String>,
    pub missing_evidence: Vec<String>,
    pub token_usage: ProviderTokenUsage,
    pub result_id: Option<ResultId>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedError {
    InvalidRequest,
    Port,
    Grounding,
}
pub trait ConnectedPort {
    fn synthesize(&self, request: &ConnectedRequest) -> Result<ConnectedResponse, ConnectedError>;
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroundingStatus {
    Accepted,
    Rejected,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroundingReceipt {
    pub packet_digest: String,
    pub request_digest: String,
    pub original_digest: String,
    pub returned_digest: String,
    pub claim_ids: Vec<String>,
    pub citation_ids: Vec<String>,
    pub usage_ids: Vec<String>,
    pub unsupported_count: usize,
    pub uncited_material_count: usize,
    pub material_error_count: usize,
    pub conflicts: Vec<String>,
    pub missing_evidence: Vec<String>,
    pub actual_tokens: ProviderTokenUsage,
    pub status: GroundingStatus,
    pub connected_calls: usize,
    pub sqz_calls: usize,
    pub fallback_reason: Option<String>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectedOutcome {
    pub response: ConnectedResponse,
    pub bytes: Vec<u8>,
    pub original_bytes: Vec<u8>,
    pub grounding: GroundingReceipt,
    pub sqz: Option<SqzReceipt>,
    pub fallback_reason: Option<String>,
}

/// One validator/coordinator. It owns neither provider transport nor process lifecycle.
#[derive(Clone, Debug)]
pub struct ConnectedCoordinator<C, S> {
    connected: C,
    sqz: SqzAdapter<S>,
    max_output_bytes: usize,
}
impl<C: ConnectedPort, S: SqzPort> ConnectedCoordinator<C, S> {
    pub fn new(connected: C, sqz: S, config: SqzAdapterConfig, max_output_bytes: usize) -> Self {
        Self {
            connected,
            sqz: SqzAdapter::new(sqz, config),
            max_output_bytes,
        }
    }
    pub fn run(
        &self,
        packet: StructuralPacket,
        query: impl Into<String>,
    ) -> Result<ConnectedOutcome, ConnectedError> {
        let request = ConnectedRequest {
            packet,
            query: query.into(),
            capabilities: ConnectedCapabilities::DENIED,
        };
        if request.query.trim().is_empty()
            || request.query.len() > MAX_QUERY
            || !request.capabilities.denied()
        {
            return Err(ConnectedError::InvalidRequest);
        }
        let request_digest = digest(format!("{:?}", request).as_bytes());
        let response = self.connected.synthesize(&request)?;
        let (valid, unsupported, uncited, errors) = validate(&request.packet, &response);
        let original = canonical(&request.packet, &response);
        let mut receipt = GroundingReceipt {
            packet_digest: request.packet.receipt.digest.clone(),
            request_digest,
            original_digest: digest(&original),
            returned_digest: digest(&[]),
            claim_ids: response.claims.iter().map(|v| v.id.clone()).collect(),
            citation_ids: response.citations.iter().map(|v| v.id.clone()).collect(),
            usage_ids: response.usages.iter().map(|v| v.id.clone()).collect(),
            unsupported_count: unsupported,
            uncited_material_count: uncited,
            material_error_count: errors,
            conflicts: request.packet.receipt.conflict_ids.clone(),
            missing_evidence: missing(&request.packet),
            actual_tokens: response.token_usage,
            status: if valid {
                GroundingStatus::Accepted
            } else {
                GroundingStatus::Rejected
            },
            connected_calls: 1,
            sqz_calls: 0,
            fallback_reason: None,
        };
        if !valid {
            receipt.fallback_reason = Some("grounding-rejected".into());
            return Ok(ConnectedOutcome {
                response,
                bytes: Vec::new(),
                original_bytes: original,
                grounding: receipt,
                sqz: None,
                fallback_reason: Some("grounding-rejected".into()),
            });
        }
        let anchors = anchors(&request.packet, &response, &original)?;
        let packet = ContextPacket {
            items: Vec::new(),
            payload: String::from_utf8(original.clone()).map_err(|_| ConnectedError::Grounding)?,
            receipt: empty_receipt(),
        };
        let evaluated = self
            .sqz
            .evaluate_with_anchors(packet, self.max_output_bytes, &anchors);
        match evaluated {
            Ok(value) => {
                receipt.sqz_calls = sqz_port_calls(&value.receipt);
                let applied = value.receipt.status == SqzStatus::Applied;
                let candidate = value.packet.payload.into_bytes();
                let bytes = if applied { candidate } else { original.clone() };
                let fallback_reason = (!applied).then(|| fallback_reason(&value.receipt));
                receipt.returned_digest = digest(&bytes);
                receipt.fallback_reason.clone_from(&fallback_reason);
                Ok(ConnectedOutcome {
                    response,
                    bytes,
                    original_bytes: original,
                    grounding: receipt,
                    sqz: Some(value.receipt),
                    fallback_reason,
                })
            }
            Err(error) => {
                receipt.sqz_calls = sqz_port_calls(&error.receipt);
                let fallback_reason = fallback_reason(&error.receipt);
                receipt.returned_digest = digest(&original);
                receipt.fallback_reason = Some(fallback_reason.clone());
                Ok(ConnectedOutcome {
                    response,
                    bytes: original.clone(),
                    original_bytes: original,
                    grounding: receipt,
                    sqz: Some(*error.receipt),
                    fallback_reason: Some(fallback_reason),
                })
            }
        }
    }
}

fn sqz_port_calls(receipt: &SqzReceipt) -> usize {
    usize::from(
        receipt.candidate_compressed_bytes.is_some()
            || matches!(
                receipt.failure_reason,
                Some(SqzFailureReason::PortUnavailable(_))
            ),
    )
}

fn fallback_reason(receipt: &SqzReceipt) -> String {
    match receipt.status {
        SqzStatus::Off => "sqz-off".into(),
        SqzStatus::NotBeneficial => "sqz-not-beneficial".into(),
        SqzStatus::Failed => format!("sqz-failed:{:?}", receipt.failure_reason),
        SqzStatus::Applied => "none".into(),
    }
}

fn validate(
    packet: &StructuralPacket,
    response: &ConnectedResponse,
) -> (bool, usize, usize, usize) {
    let mut ids = BTreeSet::new();
    let mut bad = 0;
    let mut uncited = 0;
    if response.claims.len() > MAX_RESPONSE_ITEMS
        || response.citations.len() > MAX_RESPONSE_ITEMS
        || response.usages.len() > MAX_RESPONSE_ITEMS
        || response.conflicts.len() > MAX_RESPONSE_ITEMS
        || response.missing_evidence.len() > MAX_RESPONSE_ITEMS
    {
        bad += 1;
    }
    for claim in &response.claims {
        if !valid_id(&claim.id) || !valid_field(&claim.text) || !ids.insert(claim.id.clone()) {
            bad += 1;
        }
        if claim.material && !response.citations.iter().any(|c| c.claim_id == claim.id) {
            uncited += 1;
        }
    }
    let admitted: BTreeMap<_, _> = packet
        .items
        .iter()
        .map(|item| (item.node_id.as_str(), item))
        .collect();
    for citation in &response.citations {
        if !valid_id(&citation.id)
            || !valid_id(&citation.claim_id)
            || !valid_field(&citation.node_id)
            || !valid_field(&citation.locator)
            || !valid_digest(&citation.content_digest)
            || !ids.insert(citation.id.clone())
        {
            bad += 1;
            continue;
        }
        match admitted.get(citation.node_id.as_str()) {
            Some(item)
                if !item.locator_only
                    && item.locator == citation.locator
                    && item.content_digest == citation.content_digest
                    && response.claims.iter().any(|c| c.id == citation.claim_id) => {}
            _ => bad += 1,
        }
    }
    for usage in &response.usages {
        if !valid_id(&usage.id)
            || usage.support.trim().is_empty()
            || usage.support.len() > MAX_TEXT
            || !valid_field(&usage.node_id)
            || !valid_field(&usage.locator)
            || !valid_digest(&usage.content_digest)
            || !ids.insert(usage.id.clone())
        {
            bad += 1;
            continue;
        }
        match admitted.get(usage.node_id.as_str()) {
            Some(item)
                if !item.locator_only
                    && item.locator == usage.locator
                    && item.content_digest == usage.content_digest => {}
            _ => bad += 1,
        }
    }
    let conflicts: BTreeSet<_> = response.conflicts.iter().collect();
    if conflicts.len() != response.conflicts.len()
        || response.conflicts.iter().any(|value| !valid_field(value))
    {
        bad += 1;
    }
    bad += packet
        .receipt
        .conflict_ids
        .iter()
        .filter(|v| !conflicts.contains(v))
        .count();
    let returned_missing: BTreeSet<_> = response.missing_evidence.iter().collect();
    if returned_missing.len() != response.missing_evidence.len()
        || response
            .missing_evidence
            .iter()
            .any(|value| !valid_field(value))
    {
        bad += 1;
    }
    bad += missing(packet)
        .iter()
        .filter(|v| !returned_missing.contains(v))
        .count();
    (
        bad == 0 && uncited == 0 && !response.claims.is_empty(),
        bad,
        uncited,
        bad + uncited,
    )
}
fn missing(packet: &StructuralPacket) -> Vec<String> {
    packet
        .receipt
        .missing_domains
        .iter()
        .cloned()
        .chain(
            packet
                .receipt
                .missing_evidence_classes
                .iter()
                .map(|v| format!("{:?}", v)),
        )
        .collect()
}
fn canonical(packet: &StructuralPacket, response: &ConnectedResponse) -> Vec<u8> {
    use std::fmt::Write;

    let mut out = format!("packet={}\n", packet.receipt.digest);
    for (index, value) in packet.receipt.required_domains.iter().enumerate() {
        writeln!(out, "requirement.{index}={value}").expect("writing to String cannot fail");
    }
    for (index, value) in packet.receipt.selected_ids.iter().enumerate() {
        writeln!(out, "register.{index}={value}").expect("writing to String cannot fail");
    }
    for (id, value) in [
        ("max-items", packet.receipt.max_items),
        ("max-packet-bytes", packet.receipt.max_packet_bytes),
        ("max-excerpt-bytes", packet.receipt.max_excerpt_bytes),
    ] {
        writeln!(out, "numeric.{id}={value}").expect("writing to String cannot fail");
    }
    for (index, value) in response.conflicts.iter().enumerate() {
        writeln!(out, "conflict.{index}={value}").expect("writing to String cannot fail");
    }
    for (index, value) in response.missing_evidence.iter().enumerate() {
        writeln!(out, "missing.{index}={value}").expect("writing to String cannot fail");
    }
    for claim in &response.claims {
        writeln!(out, "claim.{}.id={}", claim.id, claim.id).expect("writing to String cannot fail");
        writeln!(out, "claim.{}.text={}", claim.id, claim.text)
            .expect("writing to String cannot fail");
    }
    for citation in &response.citations {
        writeln!(
            out,
            "citation.{}={}|{}|{}|{}|{}",
            citation.id,
            citation.id,
            citation.claim_id,
            citation.node_id,
            citation.locator,
            citation.content_digest
        )
        .expect("writing to String cannot fail");
    }
    for usage in &response.usages {
        writeln!(
            out,
            "usage.{}={}|{}|{}|{}|{}",
            usage.id, usage.id, usage.node_id, usage.locator, usage.content_digest, usage.support
        )
        .expect("writing to String cannot fail");
    }
    writeln!(
        out,
        "actual-input-tokens={}",
        optional_usize(response.token_usage.actual_input_tokens)
    )
    .expect("writing to String cannot fail");
    writeln!(
        out,
        "actual-output-tokens={}",
        optional_usize(response.token_usage.actual_output_tokens)
    )
    .expect("writing to String cannot fail");
    writeln!(
        out,
        "result-id={}",
        response
            .result_id
            .as_ref()
            .map_or("unavailable", ResultId::value)
    )
    .expect("writing to String cannot fail");
    out.into_bytes()
}

fn optional_usize(value: Option<usize>) -> String {
    value.map_or_else(|| "unavailable".into(), |value| value.to_string())
}
fn anchors(
    packet: &StructuralPacket,
    response: &ConnectedResponse,
    bytes: &[u8],
) -> Result<Vec<PreservationAnchor>, ConnectedError> {
    let mut out = Vec::new();
    let original = std::str::from_utf8(bytes).map_err(|_| ConnectedError::Grounding)?;
    let mut push = |id: String, value: String| -> Result<(), ConnectedError> {
        let anchor = PreservationAnchor::new(id, value).map_err(|_| ConnectedError::Grounding)?;
        if !original.contains(anchor.value()) {
            return Err(ConnectedError::Grounding);
        }
        out.push(anchor);
        Ok(())
    };
    push("packet.digest".into(), packet.receipt.digest.clone())?;
    for claim in &response.claims {
        push(format!("claim.{}.id", claim.id), claim.id.clone())?;
        push(format!("claim.{}.text", claim.id), claim.text.clone())?;
    }
    for citation in &response.citations {
        for (field, value) in [
            ("id", citation.id.as_str()),
            ("claim", citation.claim_id.as_str()),
            ("node", citation.node_id.as_str()),
            ("locator", citation.locator.as_str()),
            ("digest", citation.content_digest.as_str()),
        ] {
            push(
                format!("citation.{}.{}", citation.id, field),
                value.to_owned(),
            )?;
        }
    }
    for usage in &response.usages {
        for (field, value) in [
            ("id", usage.id.as_str()),
            ("node", usage.node_id.as_str()),
            ("locator", usage.locator.as_str()),
            ("digest", usage.content_digest.as_str()),
            ("support", usage.support.as_str()),
        ] {
            push(format!("usage.{}.{}", usage.id, field), value.to_owned())?;
        }
    }
    for (prefix, values) in [
        ("requirement", &packet.receipt.required_domains),
        ("register", &packet.receipt.selected_ids),
        ("conflict", &response.conflicts),
        ("missing", &response.missing_evidence),
    ] {
        for (index, value) in values.iter().enumerate() {
            push(format!("{prefix}.{index}"), value.clone())?;
        }
    }
    for (id, value) in [
        ("numeric.max-items", packet.receipt.max_items),
        ("numeric.max-packet-bytes", packet.receipt.max_packet_bytes),
        (
            "numeric.max-excerpt-bytes",
            packet.receipt.max_excerpt_bytes,
        ),
    ] {
        push(id.into(), value.to_string())?;
    }
    Ok(out)
}
fn empty_receipt() -> PacketReceipt {
    PacketReceipt {
        schema_version: "connected-grounded-v1",
        seed_ids: vec![],
        admitted_dependency_ids: vec![],
        selected_ids: vec![],
        omitted_ids: vec![],
        dependency_closure_truncated: false,
        dependency_max_depth: 0,
        dependency_max_nodes: 0,
        raw_bytes: 0,
        estimated_tokens: 0,
        token_estimate_method: crate::packet::TokenEstimateMethod::BytesDividedByFourCeiling,
        effective_max_items: 0,
        effective_max_bytes: 0,
        runtime_token_ceiling: None,
        truncated: false,
    }
}
fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}
fn valid_field(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= MAX_TEXT && !value.contains(['\r', '\n'])
}
fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn digest(bytes: &[u8]) -> String {
    ResultId::sha256(bytes).value().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::{SqzIdentity, SqzPolicy, SqzPortError, SqzPortErrorCode, SqzPortOutput};
    use crate::graph::{
        StructuralAuthority, StructuralGeneratedStatus, StructuralLifecycle, StructuralSourceType,
    };
    use crate::packet::{StructuralPacketItem, StructuralPacketTier, StructuralSufficiencyReceipt};
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeMap;
    use std::rc::Rc;
    use std::time::Duration;

    #[derive(Clone)]
    struct FakeConnected {
        calls: Rc<Cell<usize>>,
        response: ConnectedResponse,
    }
    impl ConnectedPort for FakeConnected {
        fn synthesize(
            &self,
            request: &ConnectedRequest,
        ) -> Result<ConnectedResponse, ConnectedError> {
            self.calls.set(self.calls.get() + 1);
            // Exhaustive destructuring is a compile-time guard against adding a
            // repository, search, session, history, tool, or mutable-state surface.
            let ConnectedRequest {
                packet,
                query,
                capabilities,
            } = request;
            assert!(capabilities.denied());
            assert_eq!(*capabilities, ConnectedCapabilities::DENIED);
            assert_eq!(packet.items.len(), 1);
            assert_eq!(query, "answer from packet");
            Ok(self.response.clone())
        }
    }

    #[derive(Clone, Copy)]
    enum SqzBehavior {
        Applied(Option<usize>, Option<usize>),
        Echo,
        Error(SqzPortErrorCode),
        IdentityMismatch,
        Oversize,
        Dlp,
        MissingAnchor,
    }

    #[derive(Clone)]
    struct FakeSqz {
        calls: Rc<Cell<usize>>,
        inputs: Rc<RefCell<Vec<String>>>,
        behavior: SqzBehavior,
    }
    impl SqzPort for FakeSqz {
        fn compress(&self, input: &str) -> Result<SqzPortOutput, SqzPortError> {
            self.calls.set(self.calls.get() + 1);
            self.inputs.borrow_mut().push(input.to_owned());
            let output = |payload, identity, actual_input_tokens, actual_output_tokens| {
                Ok(SqzPortOutput {
                    payload,
                    identity,
                    actual_input_tokens,
                    actual_output_tokens,
                })
            };
            match self.behavior {
                SqzBehavior::Applied(input_tokens, output_tokens) => output(
                    grounded_candidate(),
                    SqzIdentity::approved(),
                    input_tokens,
                    output_tokens,
                ),
                SqzBehavior::Echo => {
                    output(input.to_owned(), SqzIdentity::approved(), Some(5), Some(4))
                }
                SqzBehavior::Error(code) => Err(SqzPortError::new(code)),
                SqzBehavior::IdentityMismatch => output(
                    grounded_candidate(),
                    SqzIdentity::new("wrong", "wrong", "b".repeat(64)),
                    Some(5),
                    Some(4),
                ),
                SqzBehavior::Oversize => {
                    output("x".repeat(4097), SqzIdentity::approved(), Some(5), Some(4))
                }
                SqzBehavior::Dlp => output(
                    format!("{} api_key=abcdefghijklmnop", grounded_candidate()),
                    SqzIdentity::approved(),
                    Some(5),
                    Some(4),
                ),
                SqzBehavior::MissingAnchor => output(
                    "claim-1 citation-1 usage-1 node-1 docs/a.md#one".into(),
                    SqzIdentity::approved(),
                    Some(5),
                    Some(4),
                ),
            }
        }
    }

    fn response() -> ConnectedResponse {
        ConnectedResponse {
            claims: vec![ConnectedClaim {
                id: "claim-1".into(),
                text: "The bounded excerpt supports the answer.".into(),
                material: true,
            }],
            citations: vec![ConnectedCitation {
                id: "citation-1".into(),
                claim_id: "claim-1".into(),
                node_id: "node-1".into(),
                locator: "docs/a.md#one".into(),
                content_digest: "a".repeat(64),
            }],
            usages: vec![ConnectedUsage {
                id: "usage-1".into(),
                node_id: "node-1".into(),
                locator: "docs/a.md#one".into(),
                content_digest: "a".repeat(64),
                support: "direct excerpt".into(),
            }],
            conflicts: vec!["conflict-1".into()],
            missing_evidence: vec!["domain-b".into()],
            token_usage: ProviderTokenUsage {
                actual_input_tokens: Some(11),
                actual_output_tokens: Some(7),
            },
            result_id: Some(ResultId::sha256(b"provider-result")),
        }
    }

    fn grounded_candidate() -> String {
        format!(
            "{} claim-1 The bounded excerpt supports the answer. citation-1 node-1 docs/a.md#one {} usage-1 direct excerpt domain-a conflict-1 domain-b 2 2048 512",
            "p".repeat(64),
            "a".repeat(64)
        )
    }

    fn packet() -> StructuralPacket {
        StructuralPacket {
            items: vec![StructuralPacketItem {
                node_id: "node-1".into(),
                canonical_path: "docs/a.md".into(),
                tier: StructuralPacketTier::PrimaryCanonical,
                role: "requirement".into(),
                source_type: StructuralSourceType::Markdown,
                lifecycle: StructuralLifecycle::Current,
                authority: StructuralAuthority::Canonical,
                generated_status: StructuralGeneratedStatus::Source,
                why_selected: "fixture".into(),
                edge_trail: vec![],
                locator: "docs/a.md#one".into(),
                content_digest: "a".repeat(64),
                excerpt: "bounded excerpt".into(),
                excerpt_bytes: 15,
                locator_only: false,
                non_authoritative: false,
            }],
            receipt: StructuralSufficiencyReceipt {
                snapshot_digest: "s".repeat(64),
                ranking_seed: "seed".into(),
                traversal_seed: None,
                ranking_binding_digest: "r".repeat(64),
                traversal_binding_digest: None,
                required_domains: vec!["domain-a".into()],
                covered_domains: vec!["domain-a".into()],
                missing_domains: vec!["domain-b".into()],
                required_evidence_classes: vec![],
                covered_evidence_classes: vec![],
                missing_evidence_classes: vec![],
                tier_counts: BTreeMap::new(),
                tier_bytes: BTreeMap::new(),
                max_items: 2,
                max_packet_bytes: 2048,
                max_excerpt_bytes: 512,
                selected_ids: vec!["node-1".into()],
                omitted_ids: vec![],
                omitted_reasons: vec![],
                conflict_ids: vec!["conflict-1".into()],
                bounded_omissions: vec![],
                ambiguity_visible: true,
                total_locator_bytes: 15,
                total_excerpt_bytes: 15,
                total_packet_bytes: 30,
                sufficient: false,
                digest: "p".repeat(64),
            },
        }
    }

    struct Execution {
        outcome: Result<ConnectedOutcome, ConnectedError>,
        connected_calls: Rc<Cell<usize>>,
        sqz_calls: Rc<Cell<usize>>,
        inputs: Rc<RefCell<Vec<String>>>,
    }

    fn execute(
        packet: StructuralPacket,
        response: ConnectedResponse,
        policy: SqzPolicy,
        configured_identity: SqzIdentity,
        configured_anchors: Vec<PreservationAnchor>,
        behavior: SqzBehavior,
    ) -> Execution {
        let connected_calls = Rc::new(Cell::new(0));
        let sqz_calls = Rc::new(Cell::new(0));
        let inputs = Rc::new(RefCell::new(Vec::new()));
        let coordinator = ConnectedCoordinator::new(
            FakeConnected {
                calls: Rc::clone(&connected_calls),
                response,
            },
            FakeSqz {
                calls: Rc::clone(&sqz_calls),
                inputs: Rc::clone(&inputs),
                behavior,
            },
            SqzAdapterConfig::new(policy, configured_identity, 4096, configured_anchors),
            4096,
        );
        Execution {
            outcome: coordinator.run(packet, "answer from packet"),
            connected_calls,
            sqz_calls,
            inputs,
        }
    }

    fn assert_grounding_rejected(packet: StructuralPacket, response: ConnectedResponse) {
        let Execution {
            outcome,
            connected_calls,
            sqz_calls,
            inputs,
        } = execute(
            packet,
            response,
            SqzPolicy::PublicOn,
            SqzIdentity::approved(),
            vec![],
            SqzBehavior::Applied(Some(5), Some(4)),
        );
        let outcome = outcome.unwrap();
        assert_eq!(connected_calls.get(), 1);
        assert_eq!(sqz_calls.get(), 0);
        assert!(inputs.borrow().is_empty());
        assert_eq!(outcome.grounding.status, GroundingStatus::Rejected);
        assert_eq!(outcome.grounding.connected_calls, 1);
        assert_eq!(outcome.grounding.sqz_calls, 0);
        assert!(outcome.bytes.is_empty());
        assert!(outcome.sqz.is_none());
        assert_eq!(
            outcome.fallback_reason.as_deref(),
            Some("grounding-rejected")
        );
        assert_eq!(outcome.grounding.fallback_reason, outcome.fallback_reason);
    }

    fn assert_exact_fallback(
        policy: SqzPolicy,
        configured_identity: SqzIdentity,
        configured_anchors: Vec<PreservationAnchor>,
        behavior: SqzBehavior,
        expected_calls: usize,
        expected_reason: &str,
    ) -> ConnectedOutcome {
        let Execution {
            outcome,
            sqz_calls: calls,
            ..
        } = execute(
            packet(),
            response(),
            policy,
            configured_identity,
            configured_anchors,
            behavior,
        );
        let outcome = outcome.unwrap();
        assert_eq!(calls.get(), expected_calls);
        assert_eq!(outcome.grounding.sqz_calls, expected_calls);
        assert_eq!(outcome.bytes, outcome.original_bytes);
        assert_eq!(
            outcome.grounding.original_digest,
            digest(&outcome.original_bytes)
        );
        assert_eq!(outcome.grounding.returned_digest, digest(&outcome.bytes));
        assert_eq!(outcome.fallback_reason.as_deref(), Some(expected_reason));
        assert_eq!(outcome.grounding.fallback_reason, outcome.fallback_reason);
        assert_ne!(outcome.sqz.as_ref().unwrap().status, SqzStatus::Applied);
        outcome
    }

    fn assert_applied_preservation(outcome: &ConnectedOutcome, inputs: &Rc<RefCell<Vec<String>>>) {
        let returned = String::from_utf8(outcome.bytes.clone()).unwrap();
        for anchor in [
            "claim-1",
            "The bounded excerpt supports the answer.",
            "citation-1",
            "usage-1",
            "node-1",
            "docs/a.md#one",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "direct excerpt",
            "domain-a",
            "conflict-1",
            "domain-b",
            "2",
            "2048",
            "512",
        ] {
            assert!(returned.contains(anchor));
        }
        let sqz_input = &inputs.borrow()[0];
        for line in [
            "requirement.0=domain-a",
            "register.0=node-1",
            "numeric.max-items=2",
            "numeric.max-packet-bytes=2048",
            "numeric.max-excerpt-bytes=512",
            "conflict.0=conflict-1",
            "missing.0=domain-b",
            "claim.claim-1.id=claim-1",
            "claim.claim-1.text=The bounded excerpt supports the answer.",
            "citation.citation-1=citation-1|claim-1|node-1|docs/a.md#one|aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "usage.usage-1=usage-1|node-1|docs/a.md#one|aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa|direct excerpt",
            "actual-input-tokens=11",
            "actual-output-tokens=7",
        ] {
            assert!(sqz_input.contains(line), "missing canonical line: {line}");
        }
    }

    #[test]
    fn p8_connected_sqz() {
        // The happy path is packet-only, reports exact separate inner usage,
        // validates before one SQZ call, and preserves every required anchor.
        let Execution {
            outcome: applied,
            connected_calls,
            sqz_calls,
            inputs,
        } = execute(
            packet(),
            response(),
            SqzPolicy::PublicOn,
            SqzIdentity::approved(),
            vec![],
            SqzBehavior::Applied(Some(5), Some(4)),
        );
        let outcome = applied.unwrap();
        assert_eq!(connected_calls.get(), 1);
        assert_eq!(sqz_calls.get(), 1);
        assert_eq!(outcome.grounding.status, GroundingStatus::Accepted);
        assert_eq!(
            outcome.grounding.actual_tokens.actual_input_tokens,
            Some(11)
        );
        assert_eq!(
            outcome.grounding.actual_tokens.actual_output_tokens,
            Some(7)
        );
        assert_eq!(outcome.grounding.sqz_calls, 1);
        assert_ne!(outcome.bytes, outcome.original_bytes);
        assert_eq!(
            outcome.grounding.original_digest,
            digest(&outcome.original_bytes)
        );
        assert_eq!(outcome.grounding.returned_digest, digest(&outcome.bytes));
        assert_eq!(outcome.fallback_reason, None);
        let sqz_receipt = outcome.sqz.as_ref().unwrap();
        assert_eq!(sqz_receipt.status, SqzStatus::Applied);
        assert_eq!(sqz_receipt.actual_input_tokens, Some(5));
        assert_eq!(sqz_receipt.actual_output_tokens, Some(4));
        assert_applied_preservation(&outcome, &inputs);

        // Every local grounding rejection is visible and bypasses SQZ.
        let mut invalid = response();
        invalid.citations.clear();
        assert_grounding_rejected(packet(), invalid); // uncited material claim

        let mut invalid = response();
        invalid.citations[0].node_id = "outside-packet".into();
        assert_grounding_rejected(packet(), invalid); // unsupported citation

        let mut locator_only = packet();
        locator_only.items[0].locator_only = true;
        assert_grounding_rejected(locator_only, response());

        let mut invalid = response();
        invalid.usages[0].node_id = "outside-packet".into();
        assert_grounding_rejected(packet(), invalid); // unsupported usage

        let mut invalid = response();
        invalid.claims.push(ConnectedClaim {
            id: "claim-1".into(),
            text: "The same fact is contradicted under its duplicate ID.".into(),
            material: true,
        });
        assert_grounding_rejected(packet(), invalid); // contradictory duplicate fact

        let mut invalid = response();
        invalid.citations.push(invalid.citations[0].clone());
        assert_grounding_rejected(packet(), invalid); // duplicate citation ID

        let mut invalid = response();
        invalid.usages.push(invalid.usages[0].clone());
        assert_grounding_rejected(packet(), invalid); // duplicate usage ID

        let mut invalid = response();
        invalid.claims[0].id = "malformed id".into();
        invalid.citations[0].id = "malformed citation".into();
        invalid.usages[0].id = "malformed usage".into();
        assert_grounding_rejected(packet(), invalid);

        let mut invalid = response();
        invalid.claims[0].text = "x".repeat(MAX_TEXT + 1);
        invalid.citations[0].locator = "x".repeat(MAX_TEXT + 1);
        invalid.usages[0].support = "x".repeat(MAX_TEXT + 1);
        assert_grounding_rejected(packet(), invalid);

        let mut invalid = response();
        invalid.conflicts.clear();
        assert_grounding_rejected(packet(), invalid); // conflict erasure

        let mut invalid = response();
        invalid.missing_evidence.clear();
        assert_grounding_rejected(packet(), invalid); // missing-evidence erasure

        // Unattested values remain unavailable in both receipts, never estimates.
        let mut unavailable = response();
        unavailable.token_usage = ProviderTokenUsage {
            actual_input_tokens: None,
            actual_output_tokens: None,
        };
        let Execution {
            outcome: unavailable,
            sqz_calls: calls,
            ..
        } = execute(
            packet(),
            unavailable,
            SqzPolicy::PublicOn,
            SqzIdentity::approved(),
            vec![],
            SqzBehavior::Applied(None, None),
        );
        let unavailable = unavailable.unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(
            unavailable.grounding.actual_tokens.actual_input_tokens,
            None
        );
        assert_eq!(
            unavailable.grounding.actual_tokens.actual_output_tokens,
            None
        );
        assert_eq!(unavailable.sqz.as_ref().unwrap().actual_input_tokens, None);
        assert_eq!(unavailable.sqz.as_ref().unwrap().actual_output_tokens, None);

        // Every SQZ non-application/fault returns the byte-identical canonical
        // original, both exact digests, and a specific fallback reason.
        assert_exact_fallback(
            SqzPolicy::PublicOff,
            SqzIdentity::approved(),
            vec![],
            SqzBehavior::Applied(Some(5), Some(4)),
            0,
            "sqz-off",
        );
        assert_exact_fallback(
            SqzPolicy::PublicOn,
            SqzIdentity::approved(),
            vec![],
            SqzBehavior::Echo,
            1,
            "sqz-not-beneficial",
        );
        assert_exact_fallback(
            SqzPolicy::PublicOn,
            SqzIdentity::approved(),
            vec![],
            SqzBehavior::Error(SqzPortErrorCode::Timeout),
            1,
            "sqz-failed:Some(PortUnavailable(Timeout))",
        );
        assert_exact_fallback(
            SqzPolicy::PublicOn,
            SqzIdentity::approved(),
            vec![],
            SqzBehavior::Error(SqzPortErrorCode::Unavailable),
            1,
            "sqz-failed:Some(PortUnavailable(Unavailable))",
        );
        assert_exact_fallback(
            SqzPolicy::PublicOn,
            SqzIdentity::approved(),
            vec![],
            SqzBehavior::Error(SqzPortErrorCode::InvalidOutput),
            1,
            "sqz-failed:Some(PortUnavailable(InvalidOutput))",
        );
        assert_exact_fallback(
            SqzPolicy::PublicOn,
            SqzIdentity::approved(),
            vec![],
            SqzBehavior::IdentityMismatch,
            1,
            "sqz-failed:Some(ReturnedIdentityMismatch)",
        );
        assert_exact_fallback(
            SqzPolicy::PublicOn,
            SqzIdentity::approved(),
            vec![],
            SqzBehavior::Oversize,
            1,
            "sqz-failed:Some(OutputExceedsBound)",
        );
        assert_exact_fallback(
            SqzPolicy::PublicOn,
            SqzIdentity::approved(),
            vec![],
            SqzBehavior::Dlp,
            1,
            "sqz-failed:Some(DlpFindings)",
        );
        assert_exact_fallback(
            SqzPolicy::PublicOn,
            SqzIdentity::approved(),
            vec![],
            SqzBehavior::MissingAnchor,
            1,
            "sqz-failed:Some(MissingFidelityAnchors)",
        );

        assert_exact_fallback(
            SqzPolicy::PublicOn,
            SqzIdentity::new("wrong", "wrong", "b".repeat(64)),
            vec![],
            SqzBehavior::Applied(Some(5), Some(4)),
            0,
            "sqz-failed:Some(ConfiguredIdentityMismatch)",
        );
        assert_exact_fallback(
            SqzPolicy::PublicOn,
            SqzIdentity::approved(),
            vec![PreservationAnchor::new("claim.claim-1.text", "contradiction").unwrap()],
            SqzBehavior::Applied(Some(5), Some(4)),
            0,
            "sqz-failed:Some(ConflictingFidelityAnchorIds)",
        );
        assert_exact_fallback(
            SqzPolicy::PublicOn,
            SqzIdentity::approved(),
            vec![PreservationAnchor::new("configured.absent", "absent-value").unwrap()],
            SqzBehavior::Applied(Some(5), Some(4)),
            0,
            "sqz-failed:Some(FidelityAnchorMissingFromInput)",
        );

        // Grounding receipts and content are deterministic; SQZ timing is the
        // sole explicitly excluded field and never enters either content digest.
        let first = execute(
            packet(),
            response(),
            SqzPolicy::PublicOn,
            SqzIdentity::approved(),
            vec![],
            SqzBehavior::Applied(Some(5), Some(4)),
        );
        let second = execute(
            packet(),
            response(),
            SqzPolicy::PublicOn,
            SqzIdentity::approved(),
            vec![],
            SqzBehavior::Applied(Some(5), Some(4)),
        );
        let first = first.outcome.unwrap();
        let second = second.outcome.unwrap();
        assert_eq!(first.response, second.response);
        assert_eq!(first.original_bytes, second.original_bytes);
        assert_eq!(first.bytes, second.bytes);
        assert_eq!(first.grounding, second.grounding);
        let mut first_sqz = first.sqz.unwrap();
        let mut second_sqz = second.sqz.unwrap();
        first_sqz.monotonic_call_latency = Duration::ZERO;
        second_sqz.monotonic_call_latency = Duration::ZERO;
        assert_eq!(first_sqz, second_sqz);
    }
}
