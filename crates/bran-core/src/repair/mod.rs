//! Model-neutral RepairCoordinator core lifecycle (R06 / P03 / P04).
//!
//! Proposal accepts a caller-supplied root-relative target and replacement bytes
//! (immutable repair command). It is read-only and captures the exact original
//! bytes/existence for the target at propose time. Fields kept immutable via
//! private + getters.
//!
//! Apply takes the exact RepairProposal plus caller-presented digest. Digest
//! mismatch is rejected separately from stale (byte-for-byte target snapshot
//! compare at apply). Deterministic id reuses ContentIdentity (not cryptographic
//! or tamper-proof).
//!
//! Writes use stdlib staged replace (tmp and backup files in target dir).
//! On post-write validator failure: rollback exactly (restore original or rm
//! if absent). Return IoPartialWrite on recovery failure. Reject symlinks in
//! target/parents and unsafe/non-root paths before any mutation.
//!
//! Success returns ValidationPassed(receipt) that records
//! "proposed -> applied -> validation-passed". Validator failure records
//! "proposed -> applied -> validation-failed -> restored".
//! Never claim success before validation.
//!
//! No model SDK/provider types. Uses std and existing crate code (ContentIdentity).
//! Core exposes explicit authority; inference is forbidden at this layer.

use crate::scan::ContentIdentity;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub const REPAIR_RECEIPT_SCHEMA_VERSION: &str = "1.0.0";

/// Immutable proposal command (read-only). Captures a caller-supplied
/// root-relative target + replacement bytes, plus exact original bytes/existence
/// at propose time. Digest is deterministic identifier (reuses ContentIdentity;
/// not cryptographic/tamper-proof). Fields private; access via getters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepairProposal {
    digest: String,
    target: String,
    content: Vec<u8>,
    original: Option<Vec<u8>>,
}

impl RepairProposal {
    pub fn digest(&self) -> &str {
        &self.digest
    }
    pub fn target(&self) -> &str {
        &self.target
    }
    pub fn content(&self) -> &[u8] {
        &self.content
    }
    /// Exact original bytes at propose time (None if target did not exist).
    pub fn original_bytes(&self) -> Option<&[u8]> {
        self.original.as_deref()
    }
}

/// Attributable receipt returned on successful authorized apply + revalidate
/// (via ValidationPassed terminal). The revalidation field explicitly records
/// the lifecycle "proposed -> applied -> validation-passed".
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepairReceipt {
    pub schema_version: &'static str,
    pub proposal_digest: String,
    pub applied_target: String,
    pub authority_tag: String,
    pub revalidation: String,
}

/// Explicit authority token. Construction asserts authority.
/// Blank reasons are rejected (result in AuthorizationFailure at apply).
/// Core never infers; caller (CLI boundary) must supply for fixture cases only.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaintainerAuthority {
    pub reason: String,
}

impl MaintainerAuthority {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

/// Typed terminal states and error conditions for the proposal/apply/revalidate machine.
/// Distinguishes: proposed, applied, validation-passed, validation-failed,
/// authorization failure, digest mismatch, stale source, unsafe path,
/// and I/O/partial-write failure.
/// Success path returns ValidationPassed(receipt) after revalidation; never
/// claims success prior to validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RepairTerminal {
    Proposed(RepairProposal),
    Applied(RepairReceipt),
    ValidationPassed(RepairReceipt),
    ValidationFailed {
        reason: String,
        receipt: RepairReceipt,
    },
    AuthorizationFailure,
    DigestMismatch {
        expected: String,
        actual: String,
    },
    StaleSource,
    UnsafePath {
        path: String,
    },
    IoPartialWrite {
        path: String,
        reason: String,
    },
}

/// Function type for injected post-apply validator.
/// Failure from the validator must never produce a success receipt.
pub type ValidatorFn = fn(&Path) -> Result<(), String>;

/// Root-bound repair coordinator. Stateless between calls except for captured root.
pub struct RepairCoordinator {
    root: PathBuf,
    validator: ValidatorFn,
}

impl RepairCoordinator {
    /// Construct with explicit root and injected validator.
    /// Root must exist and be a directory after canonicalization.
    #[allow(clippy::result_large_err)] // preserves the public typed terminal-state contract
    pub fn new(root: impl AsRef<Path>, validator: ValidatorFn) -> Result<Self, RepairTerminal> {
        let requested = root.as_ref().to_path_buf();
        let canon = match fs::canonicalize(&requested) {
            Ok(c) => c,
            Err(e) => {
                return Err(RepairTerminal::IoPartialWrite {
                    path: requested.display().to_string(),
                    reason: e.to_string(),
                });
            }
        };
        if !canon.is_dir() {
            return Err(RepairTerminal::UnsafePath {
                path: requested.display().to_string(),
            });
        }
        Ok(Self {
            root: canon,
            validator,
        })
    }

    /// Propose is strictly read-only. Accepts caller-supplied root-relative
    /// target and replacement bytes (the bounded repair command). Captures
    /// exact original bytes/existence. Returns terminal with immutable proposal
    /// carrying deterministic digest. No filesystem mutation occurs.
    pub fn propose(&self, target: impl Into<String>, replacement: Vec<u8>) -> RepairTerminal {
        let target = target.into();
        if !is_safe_relative(&target) {
            return RepairTerminal::UnsafePath { path: target };
        }
        let full = self.root.join(&target);
        if !full.starts_with(&self.root) || is_symlink_ancestor_or_target(&self.root, &full) {
            return RepairTerminal::UnsafePath { path: target };
        }
        let original = match read_existing(&full) {
            Ok(original) => original,
            Err(e) => {
                return RepairTerminal::IoPartialWrite {
                    path: target,
                    reason: e.to_string(),
                };
            }
        };
        let plan_id = make_plan_id(&target, &replacement);
        let source_id = make_source_id(original.as_deref());
        let digest = compute_digest(&plan_id, &source_id);
        let proposal = RepairProposal {
            digest,
            target,
            content: replacement,
            original,
        };
        RepairTerminal::Proposed(proposal)
    }

    /// Apply requires explicit authority and the exact RepairProposal plus the
    /// caller-presented digest. Performs byte-for-byte target snapshot compare
    /// (for stale, including same-length edits), root-bound + symlink checks,
    /// then atomic staged write, then revalidates. Returns ValidationPassed(receipt)
    /// recording full lifecycle only on pass. Validator failure triggers exact
    /// rollback then ValidationFailed. Never claims success before validation.
    pub fn apply(
        &self,
        authority: Option<MaintainerAuthority>,
        proposal: RepairProposal,
        digest: &str,
    ) -> RepairTerminal {
        let auth = match authority {
            Some(a) if !a.reason.trim().is_empty() => a,
            _ => return RepairTerminal::AuthorizationFailure,
        };

        if digest != proposal.digest() {
            return RepairTerminal::DigestMismatch {
                expected: proposal.digest().to_owned(),
                actual: digest.to_owned(),
            };
        }

        let target = proposal.target().to_owned();
        let content = proposal.content().to_vec();

        let full = self.root.join(&target);
        if !is_safe_relative(&target)
            || !full.starts_with(&self.root)
            || is_symlink_ancestor_or_target(&self.root, &full)
        {
            return RepairTerminal::UnsafePath { path: target };
        }

        // Byte-for-byte compare of current target bytes/existence vs proposal snapshot.
        // Catches same-length edits (and any content change).
        let current_state = match read_existing(&full) {
            Ok(current) => current,
            Err(e) => {
                return RepairTerminal::IoPartialWrite {
                    path: target,
                    reason: e.to_string(),
                };
            }
        };
        let snap_state: Option<Vec<u8>> = proposal.original_bytes().map(|b| b.to_vec());
        if current_state != snap_state {
            return RepairTerminal::StaleSource;
        }

        // Staged replace retains the original as a same-directory backup until
        // revalidation completes, avoiding rename-overwrite on Windows.
        let backup = match staged_replace(&full, &content) {
            Ok(backup) => backup,
            Err(e) => {
                return RepairTerminal::IoPartialWrite {
                    path: target,
                    reason: e.to_string(),
                };
            }
        };

        // Revalidate before reporting success. Do not claim success before validation.
        match (self.validator)(&self.root) {
            Ok(()) => {
                if let Some(backup) = backup.as_deref() {
                    if let Err(e) = remove_file_if_exists(backup) {
                        return RepairTerminal::IoPartialWrite {
                            path: target,
                            reason: e.to_string(),
                        };
                    }
                }
                let receipt = RepairReceipt {
                    schema_version: REPAIR_RECEIPT_SCHEMA_VERSION,
                    proposal_digest: digest.to_owned(),
                    applied_target: target,
                    authority_tag: auth.reason,
                    revalidation: "proposed -> applied -> validation-passed".to_owned(),
                };
                RepairTerminal::ValidationPassed(receipt)
            }
            Err(reason) => {
                // Rollback: restore retained original if it existed, else remove created file.
                if let Err(restore_err) = rollback(&full, backup.as_deref()) {
                    return RepairTerminal::IoPartialWrite {
                        path: target,
                        reason: format!(
                            "validator failed; rollback failed: {} (validator: {})",
                            restore_err, reason
                        ),
                    };
                }
                RepairTerminal::ValidationFailed {
                    reason: format!("applied -> validation-failed: {}", reason),
                    receipt: RepairReceipt {
                        schema_version: REPAIR_RECEIPT_SCHEMA_VERSION,
                        proposal_digest: digest.to_owned(),
                        applied_target: target,
                        authority_tag: auth.reason,
                        revalidation: "proposed -> applied -> validation-failed -> restored"
                            .to_owned(),
                    },
                }
            }
        }
    }
}

fn is_safe_relative(rel: &str) -> bool {
    if rel.is_empty() || rel.starts_with('/') || rel.starts_with('\\') || rel.contains('\0') {
        return false;
    }
    let p = Path::new(rel);
    for comp in p.components() {
        if matches!(comp, std::path::Component::ParentDir) {
            return false;
        }
    }
    true
}

fn make_source_id(original: Option<&[u8]>) -> String {
    match original {
        Some(bytes) => {
            let id = ContentIdentity::from_bytes(bytes);
            format!(
                "s:present:{}:{:016x}-{:016x}-{:016x}",
                id.byte_len, id.lanes[0], id.lanes[1], id.lanes[2]
            )
        }
        None => "s:missing".to_owned(),
    }
}

fn make_plan_id(target: &str, content: &[u8]) -> String {
    let c = ContentIdentity::from_bytes(content);
    format!(
        "p:{}:{}:{:016x}-{:016x}-{:016x}",
        target, c.byte_len, c.lanes[0], c.lanes[1], c.lanes[2]
    )
}

fn compute_digest(plan_id: &str, source_id: &str) -> String {
    format!("{}|{}", plan_id, source_id)
}

/// Reject if target or any parent component under root is a symlink (detect via
/// symlink_metadata without following). Checked before any read or mutation.
fn is_symlink_ancestor_or_target(root: &Path, target: &Path) -> bool {
    let rel = match target.strip_prefix(root) {
        Ok(r) => r,
        Err(_) => return true,
    };
    let mut cur = root.to_path_buf();
    for comp in rel.components() {
        cur.push(comp);
        if let Ok(m) = fs::symlink_metadata(&cur) {
            if m.file_type().is_symlink() {
                return true;
            }
        }
    }
    false
}

fn read_existing(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(_) => fs::read(path).map(Some),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Write a staged sibling, retain an existing target as a backup, then install
/// the staged file into the now-empty target name. The backup remains until
/// validation determines whether to commit or restore it.
fn staged_replace(target: &Path, data: &[u8]) -> io::Result<Option<PathBuf>> {
    let parent = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let fname = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repair".to_owned());
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let tmp = parent.join(format!(".{}.tmp.{}.{}", fname, std::process::id(), nonce));
    let backup = parent.join(format!(".{}.bak.{}.{}", fname, std::process::id(), nonce));

    let write_result = (|| -> io::Result<()> {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()
    })();
    if let Err(e) = write_result {
        return Err(cleanup_error(e, remove_file_if_exists(&tmp)));
    }

    match fs::symlink_metadata(target) {
        Ok(_) => {
            if let Err(e) = fs::rename(target, &backup) {
                return Err(cleanup_error(e, remove_file_if_exists(&tmp)));
            }
            if let Err(e) = fs::rename(&tmp, target) {
                let restore = fs::rename(&backup, target);
                let cleanup = remove_file_if_exists(&tmp);
                return Err(cleanup_error(cleanup_error(e, restore), cleanup));
            }
            Ok(Some(backup))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if let Err(e) = fs::rename(&tmp, target) {
                return Err(cleanup_error(e, remove_file_if_exists(&tmp)));
            }
            Ok(None)
        }
        Err(e) => Err(cleanup_error(e, remove_file_if_exists(&tmp))),
    }
}

fn remove_file_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn cleanup_error(primary: io::Error, cleanup: io::Result<()>) -> io::Error {
    match cleanup {
        Ok(()) => primary,
        Err(e) => io::Error::other(format!("{}; cleanup failed: {}", primary, e)),
    }
}

/// On validator failure restore retained original bytes, or remove a newly
/// created target. Errors become IoPartialWrite at the caller.
fn rollback(target: &Path, backup: Option<&Path>) -> io::Result<()> {
    match backup {
        Some(backup) => {
            remove_file_if_exists(target)?;
            fs::rename(backup, target)
        }
        None => remove_file_if_exists(target),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    struct TempDirGuard(PathBuf);

    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn passing_validator(_root: &Path) -> Result<(), String> {
        Ok(())
    }

    fn failing_validator(_root: &Path) -> Result<(), String> {
        Err("injected validator failed".to_owned())
    }

    #[test]
    fn p3_cli_maintainer() {
        let base = std::env::temp_dir();
        let unique = format!(
            "bran-repair-p3-cli-maintainer-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );

        let root = base.join(&unique);
        fs::create_dir_all(&root).expect("create temp root");
        let _guard = TempDirGuard(root.clone());

        fs::write(root.join("seed.rs"), b"fn seed(){}\n").unwrap();

        let coord = RepairCoordinator::new(&root, passing_validator).expect("valid coord");

        // arbitrary caller-supplied target/replacement; zero-mutation deterministic propose
        let target = "p3-repair.txt".to_string();
        let replacement = b"caller-supplied-replacement-bytes-exact\n".to_vec();
        let t1 = coord.propose(target.clone(), replacement.clone());
        fs::write(root.join("seed.rs"), b"fn unrelated_change(){}\n").unwrap();
        let t2 = coord.propose(target.clone(), replacement.clone());
        let p1 = match t1 {
            RepairTerminal::Proposed(p) => p,
            _ => panic!("expected Proposed"),
        };
        let p2 = match t2 {
            RepairTerminal::Proposed(p) => p,
            _ => panic!("expected Proposed"),
        };
        assert_eq!(p1.digest(), p2.digest());
        assert!(!root.join(&target).exists(), "propose must not mutate");

        // unauthorized and bad digest refuse (no mutation)
        let unauth = coord.apply(None, p1.clone(), p1.digest());
        assert!(matches!(unauth, RepairTerminal::AuthorizationFailure));
        assert!(!root.join(&target).exists());
        let bad = coord.apply(
            Some(MaintainerAuthority::new("x")),
            p1.clone(),
            "bad-digest",
        );
        assert!(matches!(bad, RepairTerminal::DigestMismatch { .. }));
        assert!(!root.join(&target).exists());

        // MaintainerAuthority rejects blank reasons (auth fail)
        let blank = coord.apply(
            Some(MaintainerAuthority::new("   ")),
            p1.clone(),
            p1.digest(),
        );
        assert!(matches!(blank, RepairTerminal::AuthorizationFailure));

        // same-length target mutation is stale
        let sl_t = "sl.txt".to_string();
        let sl_rep = b"0123456789abcdef".to_vec();
        fs::write(root.join(&sl_t), b"0000000000000000").unwrap();
        let slp_t = coord.propose(sl_t.clone(), sl_rep.clone());
        let slp = match slp_t {
            RepairTerminal::Proposed(p) => p,
            _ => panic!("expected sl Proposed"),
        };
        fs::write(root.join(&sl_t), b"1111111111111111").unwrap();
        let sl_digest = slp.digest().to_owned();
        let sl_stale = coord.apply(Some(MaintainerAuthority::new("s")), slp, &sl_digest);
        assert!(matches!(sl_stale, RepairTerminal::StaleSource));

        // successful explicit-authority apply/revalidate receipt and exact bytes
        let auth = Some(MaintainerAuthority::new("cli-maintainer-fixture"));
        let ok_term = coord.apply(auth, p1.clone(), p1.digest());
        let receipt = match ok_term {
            RepairTerminal::ValidationPassed(r) => r,
            other => panic!("expected ValidationPassed, got {:?}", other),
        };
        assert_eq!(receipt.schema_version, REPAIR_RECEIPT_SCHEMA_VERSION);
        assert_eq!(receipt.proposal_digest, p1.digest());
        assert_eq!(receipt.applied_target, target);
        assert_eq!(receipt.authority_tag, "cli-maintainer-fixture");
        assert_eq!(
            receipt.revalidation,
            "proposed -> applied -> validation-passed"
        );
        assert_eq!(fs::read(root.join(&target)).unwrap(), replacement);

        // failing validator restores pre-apply bytes
        let unique2 = format!("{}-vf", unique);
        let root2 = base.join(&unique2);
        fs::create_dir_all(&root2).unwrap();
        let _guard2 = TempDirGuard(root2.clone());
        let pre = b"pre-apply-original-bytes-exact\n";
        let vf_t = "vf.txt".to_string();
        fs::write(root2.join(&vf_t), pre).unwrap();
        let coordf = RepairCoordinator::new(&root2, failing_validator).expect("coordf");
        let vrep = b"repaired-bytes-must-not-remain\n".to_vec();
        let vfp_t = coordf.propose(vf_t.clone(), vrep);
        let vfp = match vfp_t {
            RepairTerminal::Proposed(p) => p,
            _ => panic!("expected vf Proposed"),
        };
        let vf_digest = vfp.digest().to_owned();
        let vf_term = coordf.apply(Some(MaintainerAuthority::new("test")), vfp, &vf_digest);
        let failed_receipt = match vf_term {
            RepairTerminal::ValidationFailed { receipt, .. } => receipt,
            other => panic!("expected ValidationFailed, got {:?}", other),
        };
        assert_eq!(failed_receipt.proposal_digest, vf_digest);
        assert_eq!(failed_receipt.applied_target, vf_t);
        assert_eq!(failed_receipt.authority_tag, "test");
        assert_eq!(
            failed_receipt.revalidation,
            "proposed -> applied -> validation-failed -> restored"
        );
        assert_eq!(fs::read(root2.join(&vf_t)).unwrap(), pre);

        // absent-target rollback: validator failure restores non-existence
        let unique3 = format!("{}-absent", unique);
        let root3 = base.join(&unique3);
        fs::create_dir_all(&root3).unwrap();
        let _guard3 = TempDirGuard(root3.clone());
        let abs_t = "absent.txt".to_string();
        let coord2 = RepairCoordinator::new(&root3, failing_validator).expect("coord absent");
        let abs_rep = b"must-not-remain-absent-case\n".to_vec();
        let abs_term = coord2.propose(abs_t.clone(), abs_rep);
        let abs_p = match abs_term {
            RepairTerminal::Proposed(p) => p,
            _ => panic!("expected abs Proposed"),
        };
        let abs_digest = abs_p.digest().to_owned();
        let abs_result = coord2.apply(Some(MaintainerAuthority::new("absent")), abs_p, &abs_digest);
        let abs_receipt = match abs_result {
            RepairTerminal::ValidationFailed { receipt, .. } => receipt,
            other => panic!(
                "expected ValidationFailed for absent rollback, got {:?}",
                other
            ),
        };
        assert_eq!(
            abs_receipt.revalidation,
            "proposed -> applied -> validation-failed -> restored"
        );
        assert!(
            !root3.join(&abs_t).exists(),
            "absent target must not exist after rollback"
        );

        // root-relative regular target accepted (SEIT-14 positive)
        let unique4 = format!("{}-regular", unique);
        let root4 = base.join(&unique4);
        fs::create_dir_all(&root4).unwrap();
        let _guard4 = TempDirGuard(root4.clone());
        fs::create_dir_all(root4.join("subdir")).unwrap();
        fs::write(root4.join("subdir/target.rs"), b"original subdir bytes\n").unwrap();
        let coord3 = RepairCoordinator::new(&root4, passing_validator).expect("coord regular");
        let reg = b"replaced subdir bytes\n".to_vec();
        let reg_term = coord3.propose("subdir/target.rs".to_string(), reg.clone());
        let reg_p = match reg_term {
            RepairTerminal::Proposed(p) => p,
            _ => panic!("expected regular Proposed"),
        };
        let reg_digest = reg_p.digest().to_owned();
        let reg_result = coord3.apply(
            Some(MaintainerAuthority::new("regular")),
            reg_p,
            &reg_digest,
        );
        let reg_receipt = match reg_result {
            RepairTerminal::ValidationPassed(r) => r,
            other => panic!(
                "expected ValidationPassed for regular target, got {:?}",
                other
            ),
        };
        assert_eq!(reg_receipt.applied_target, "subdir/target.rs");
        assert_eq!(
            reg_receipt.revalidation,
            "proposed -> applied -> validation-passed"
        );
        assert_eq!(fs::read(root4.join("subdir/target.rs")).unwrap(), reg);

        // unsafe path refuses
        let us = coord.propose("../escape.txt".to_string(), b"x".to_vec());
        assert!(matches!(us, RepairTerminal::UnsafePath { .. }));

        #[cfg(unix)]
        {
            let outside = base.join(format!("{}-outside", unique));
            fs::create_dir_all(&outside).unwrap();
            let _outside_guard = TempDirGuard(outside.clone());
            fs::write(outside.join("secret.txt"), b"outside-root").unwrap();
            std::os::unix::fs::symlink(&outside, root.join("escape-link")).unwrap();
            let escaped = coord.propose("escape-link/secret.txt", b"replacement".to_vec());
            assert!(matches!(escaped, RepairTerminal::UnsafePath { .. }));
        }
    }
}
