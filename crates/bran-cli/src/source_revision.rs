//! Read-only git HEAD for registered-root provenance
//! (docs/deployment-profile.md). Parses git metadata directly and never runs
//! git, so no hook, config, or filter executes. Every file it reads must be a
//! regular file that resolves inside its git directory: a symlink leading
//! elsewhere or a FIFO makes the revision unavailable instead of reading
//! outside data or blocking.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, ErrorKind, Read};
use std::path::{Path, PathBuf};

const MAX_REF_FILE_BYTES: u64 = 4096;
const MAX_PACKED_REFS_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SYMREF_DEPTH: usize = 5;
/// Ref namespaces git keeps per worktree. They are never packed.
const PER_WORKTREE_REFS: [&str; 3] = ["refs/worktree/", "refs/bisect/", "refs/rewritten/"];

/// The commit a registered root's HEAD names, or why it is unavailable.
#[derive(Debug, PartialEq, Eq)]
pub enum SourceRevision {
    Attested {
        reference: Option<String>,
        commit: String,
    },
    Unavailable(&'static str),
}

pub fn read(root: &Path) -> SourceRevision {
    match head(root) {
        Ok((reference, commit)) => SourceRevision::Attested { reference, commit },
        Err(reason) => SourceRevision::Unavailable(reason),
    }
}

enum Small {
    Missing,
    Unreadable,
    Invalid,
    /// Not a regular file inside the expected directory.
    Unsafe,
    Text(String),
}

#[cfg(unix)]
fn open_no_follow_no_block(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}

#[cfg(not(unix))]
fn open_no_follow_no_block(path: &Path) -> std::io::Result<File> {
    File::open(path)
}

/// Opens `path` only when it resolves, through any symlinks, to a regular
/// file inside the canonical directory `within`. A directory counts as missing.
fn open_confined(path: &Path, within: &Path) -> Result<File, Small> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Err(Small::Missing),
        Err(_) => return Err(Small::Unreadable),
    }
    let real = fs::canonicalize(path).map_err(|_| Small::Unreadable)?;
    if !real.starts_with(within) {
        return Err(Small::Unsafe);
    }
    let kind = fs::symlink_metadata(&real)
        .map_err(|_| Small::Unreadable)?
        .file_type();
    if kind.is_dir() {
        return Err(Small::Missing);
    }
    if !kind.is_file() {
        return Err(Small::Unsafe);
    }
    let file = open_no_follow_no_block(&real).map_err(|_| Small::Unreadable)?;
    match file.metadata() {
        Ok(metadata) if metadata.is_file() => Ok(file),
        Ok(_) => Err(Small::Unsafe),
        Err(_) => Err(Small::Unreadable),
    }
}

/// A bounded UTF-8 read of a confined file.
fn read_small(path: &Path, within: &Path) -> Small {
    let file = match open_confined(path, within) {
        Ok(file) => file,
        Err(outcome) => return outcome,
    };
    let mut bytes = Vec::new();
    match file.take(MAX_REF_FILE_BYTES + 1).read_to_end(&mut bytes) {
        Ok(_) if bytes.len() as u64 > MAX_REF_FILE_BYTES => Small::Invalid,
        Ok(_) => String::from_utf8(bytes).map_or(Small::Invalid, Small::Text),
        Err(_) => Small::Unreadable,
    }
}

/// One path line such as a `.git` file's `gitdir:` target or `commondir`.
fn path_line(text: &str) -> Option<&str> {
    let line = text.trim_end_matches(['\r', '\n']);
    (!line.is_empty() && !line.contains(['\r', '\n', '\0'])).then_some(line)
}

/// The canonical per-worktree git dir (HEAD, per-worktree refs) and common
/// dir (shared refs).
fn git_dirs(root: &Path) -> Result<(PathBuf, PathBuf), &'static str> {
    let root = fs::canonicalize(root).map_err(|_| "git_metadata_invalid")?;
    let dot_git = root.join(".git");
    let kind = match fs::symlink_metadata(&dot_git) {
        Ok(metadata) => metadata.file_type(),
        Err(error) if error.kind() == ErrorKind::NotFound => return Err("not_a_git_checkout"),
        Err(_) => return Err("git_metadata_invalid"),
    };
    let git_dir = if kind.is_dir() {
        dot_git
    } else if kind.is_file() {
        let Small::Text(text) = read_small(&dot_git, &root) else {
            return Err("git_metadata_invalid");
        };
        let target = text
            .strip_prefix("gitdir: ")
            .and_then(path_line)
            .ok_or("git_metadata_invalid")?;
        root.join(target)
    } else {
        return Err("git_metadata_invalid");
    };
    let git_dir = fs::canonicalize(git_dir)
        .ok()
        .filter(|directory| directory.is_dir())
        .ok_or("git_metadata_invalid")?;
    let common_dir = match read_small(&git_dir.join("commondir"), &git_dir) {
        Small::Missing => git_dir.clone(),
        Small::Text(text) => {
            fs::canonicalize(git_dir.join(path_line(&text).ok_or("git_metadata_invalid")?))
                .map_err(|_| "git_metadata_invalid")?
        }
        Small::Unsafe => return Err("git_metadata_unsafe"),
        Small::Unreadable | Small::Invalid => return Err("git_metadata_invalid"),
    };
    Ok((git_dir, common_dir))
}

/// A ref name that stays below `refs/` when joined to a git dir.
fn safe_ref(name: &str) -> bool {
    name.starts_with("refs/")
        && !name.contains(['\\', '\0'])
        && name
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// A full lowercase SHA-1 or SHA-256 object id.
fn object_id(text: &str) -> bool {
    matches!(text.len(), 40 | 64)
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn head(root: &Path) -> Result<(Option<String>, String), &'static str> {
    let (git_dir, common_dir) = git_dirs(root)?;
    if common_dir.join("reftable").exists() {
        return Err("reftable_unsupported");
    }
    let mut content = match read_small(&git_dir.join("HEAD"), &git_dir) {
        Small::Text(text) => text,
        Small::Invalid => return Err("head_corrupt"),
        Small::Unsafe => return Err("git_metadata_unsafe"),
        Small::Missing | Small::Unreadable => return Err("head_unreadable"),
    };
    let mut reference = None;
    for _ in 0..=MAX_SYMREF_DEPTH {
        let text = content.trim_end();
        let Some(target) = text.strip_prefix("ref: ") else {
            return if object_id(text) {
                Ok((reference, text.to_owned()))
            } else if reference.is_none() {
                Err("head_corrupt")
            } else {
                Err("ref_corrupt")
            };
        };
        if !safe_ref(target) {
            return Err("ref_unsafe");
        }
        let target = target.to_owned();
        reference.get_or_insert_with(|| target.clone());
        // Per-worktree refs live in the worktree's git dir and are never
        // packed. Shared refs: a loose ref wins over packed-refs, as in git.
        let per_worktree = PER_WORKTREE_REFS
            .iter()
            .any(|prefix| target.starts_with(prefix));
        let store = if per_worktree { &git_dir } else { &common_dir };
        content = match read_small(&store.join(&target), store) {
            Small::Text(text) => text,
            Small::Missing if per_worktree => return Err("ref_unresolved"),
            Small::Missing => return packed_ref(&common_dir, &target).map(|id| (reference, id)),
            Small::Invalid => return Err("ref_corrupt"),
            Small::Unsafe => return Err("ref_unsafe"),
            Small::Unreadable => return Err("ref_unreadable"),
        };
    }
    Err("ref_cycle")
}

fn packed_ref(common_dir: &Path, target: &str) -> Result<String, &'static str> {
    let file = match open_confined(&common_dir.join("packed-refs"), common_dir) {
        Ok(file) => file,
        Err(Small::Missing) => return Err("ref_unresolved"),
        Err(Small::Unsafe) => return Err("ref_unsafe"),
        Err(_) => return Err("ref_unreadable"),
    };
    let mut reader = BufReader::new(file.take(MAX_PACKED_REFS_BYTES + 1));
    let mut total = 0u64;
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line).map_err(|error| {
            if error.kind() == ErrorKind::InvalidData {
                "ref_corrupt"
            } else {
                "ref_unreadable"
            }
        })?;
        if read == 0 {
            return Err("ref_unresolved");
        }
        total += read as u64;
        if total > MAX_PACKED_REFS_BYTES {
            return Err("ref_corrupt");
        }
        let entry = line.trim_end();
        if entry.is_empty() || entry.starts_with(['#', '^']) {
            continue;
        }
        let (id, name) = entry.split_once(' ').ok_or("ref_corrupt")?;
        if name == target {
            return if object_id(id) {
                Ok(id.to_owned())
            } else {
                Err("ref_corrupt")
            };
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::{read, SourceRevision};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
    const OTHER: &str = "89abcdef0123456789abcdef0123456789abcdef";

    fn scratch(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "bran-source-revision-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn put(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn attested(reference: Option<&str>, commit: &str) -> SourceRevision {
        SourceRevision::Attested {
            reference: reference.map(str::to_owned),
            commit: commit.to_owned(),
        }
    }

    /// Runs real git with no user or system config and no hooks.
    fn git(directory: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(directory)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", directory)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "BRAN test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "BRAN test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(["-c", "init.defaultBranch=main"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    #[test]
    fn p8_source_revision() {
        // Checkout: HEAD names a loose branch ref, which wins over a stale packed entry.
        let checkout = scratch("checkout");
        put(&checkout.join(".git/HEAD"), "ref: refs/heads/main\n");
        put(
            &checkout.join(".git/refs/heads/main"),
            &format!("{COMMIT}\n"),
        );
        assert_eq!(read(&checkout), attested(Some("refs/heads/main"), COMMIT));
        put(
            &checkout.join(".git/packed-refs"),
            &format!("# pack-refs with: peeled fully-peeled sorted \n{OTHER} refs/heads/main\n"),
        );
        assert_eq!(read(&checkout), attested(Some("refs/heads/main"), COMMIT));

        // Detached HEAD, including a SHA-256 object id.
        let detached = scratch("detached");
        put(&detached.join(".git/HEAD"), &format!("{COMMIT}\n"));
        assert_eq!(read(&detached), attested(None, COMMIT));
        let long = "ab".repeat(32);
        put(&detached.join(".git/HEAD"), &format!("{long}\n"));
        assert_eq!(read(&detached), attested(None, &long));

        // Packed ref only; peeled lines are skipped.
        let packed = scratch("packed");
        put(&packed.join(".git/HEAD"), "ref: refs/heads/release\n");
        put(
            &packed.join(".git/packed-refs"),
            &format!(
                "# pack-refs with: peeled fully-peeled sorted \n{OTHER} refs/heads/main\n{COMMIT} refs/heads/release\n^{OTHER}\n"
            ),
        );
        assert_eq!(read(&packed), attested(Some("refs/heads/release"), COMMIT));

        // Linked worktree: `.git` file, per-worktree HEAD, shared refs via commondir.
        let main = scratch("main");
        put(&main.join(".git/HEAD"), "ref: refs/heads/main\n");
        put(&main.join(".git/refs/heads/main"), &format!("{OTHER}\n"));
        put(
            &main.join(".git/refs/heads/feature"),
            &format!("{COMMIT}\n"),
        );
        put(
            &main.join(".git/worktrees/feature/HEAD"),
            "ref: refs/heads/feature\n",
        );
        put(&main.join(".git/worktrees/feature/commondir"), "../..\n");
        let linked = scratch("linked");
        put(
            &linked.join(".git"),
            &format!(
                "gitdir: {}\n",
                main.join(".git/worktrees/feature").display()
            ),
        );
        assert_eq!(read(&linked), attested(Some("refs/heads/feature"), COMMIT));
        let nested = main.join("nested");
        put(&nested.join(".git"), "gitdir: ../.git/worktrees/feature\n");
        assert_eq!(read(&nested), attested(Some("refs/heads/feature"), COMMIT));
        assert_eq!(read(&main), attested(Some("refs/heads/main"), OTHER));

        // Not a git checkout.
        let plain = scratch("plain");
        assert_eq!(
            read(&plain),
            SourceRevision::Unavailable("not_a_git_checkout")
        );

        // Corrupt or unresolvable metadata is reported, never guessed.
        let broken = scratch("broken");
        let short = &COMMIT[..39];
        let upper = COMMIT.to_ascii_uppercase();
        let overlong = format!("{COMMIT}0");
        for (head, reason) in [
            ("garbage\n", "head_corrupt"),
            ("", "head_corrupt"),
            (short, "head_corrupt"),
            (upper.as_str(), "head_corrupt"),
            (overlong.as_str(), "head_corrupt"),
            ("ref: ../../../etc/passwd\n", "ref_unsafe"),
            ("ref: refs/heads/../../config\n", "ref_unsafe"),
            ("ref: HEAD\n", "ref_unsafe"),
            ("ref: refs/heads/unborn\n", "ref_unresolved"),
            ("ref: refs/heads/bad\n", "ref_corrupt"),
            ("ref: refs/heads/loop-a\n", "ref_cycle"),
            ("ref: refs/heads/packed-bad\n", "ref_corrupt"),
        ] {
            put(&broken.join(".git/HEAD"), head);
            put(&broken.join(".git/refs/heads/bad"), "not-a-commit\n");
            put(
                &broken.join(".git/refs/heads/loop-a"),
                "ref: refs/heads/loop-b\n",
            );
            put(
                &broken.join(".git/refs/heads/loop-b"),
                "ref: refs/heads/loop-a\n",
            );
            put(
                &broken.join(".git/packed-refs"),
                &format!("{short} refs/heads/packed-bad\n"),
            );
            assert_eq!(
                read(&broken),
                SourceRevision::Unavailable(reason),
                "HEAD {head:?}"
            );
        }
        fs::remove_file(broken.join(".git/HEAD")).unwrap();
        assert_eq!(
            read(&broken),
            SourceRevision::Unavailable("head_unreadable")
        );
        let invalid = scratch("invalid");
        put(&invalid.join(".git"), "not a gitdir line\n");
        assert_eq!(
            read(&invalid),
            SourceRevision::Unavailable("git_metadata_invalid")
        );
        let reftable = scratch("reftable");
        put(&reftable.join(".git/HEAD"), "ref: refs/heads/.invalid\n");
        fs::create_dir_all(reftable.join(".git/reftable")).unwrap();
        assert_eq!(
            read(&reftable),
            SourceRevision::Unavailable("reftable_unsupported")
        );

        // Cross-check against real git: loose, packed, linked worktree, detached.
        let real = scratch("real");
        git(&real, &["init", "-q"]);
        fs::write(real.join("a.txt"), "a\n").unwrap();
        git(&real, &["add", "a.txt"]);
        git(&real, &["commit", "-q", "-m", "first"]);
        let first = git(&real, &["rev-parse", "HEAD"]);
        assert_eq!(read(&real), attested(Some("refs/heads/main"), &first));
        git(&real, &["pack-refs", "--all"]);
        assert!(!real.join(".git/refs/heads/main").exists());
        assert_eq!(read(&real), attested(Some("refs/heads/main"), &first));
        let worktree = real.with_file_name(format!(
            "bran-source-revision-{}-real-worktree",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&worktree);
        git(
            &real,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "side",
                worktree.to_str().unwrap(),
            ],
        );
        assert_eq!(read(&worktree), attested(Some("refs/heads/side"), &first));
        fs::write(real.join("b.txt"), "b\n").unwrap();
        git(&real, &["add", "b.txt"]);
        git(&real, &["commit", "-q", "-m", "second"]);
        git(&real, &["checkout", "-q", "--detach"]);
        let second = git(&real, &["rev-parse", "HEAD"]);
        assert_ne!(first, second);
        assert_eq!(read(&real), attested(None, &second));

        for directory in [
            checkout, detached, packed, main, linked, plain, broken, invalid, reftable, real,
            worktree,
        ] {
            fs::remove_dir_all(directory).unwrap();
        }
    }

    /// `read` on another thread; `None` when it has not returned in time.
    fn read_within(root: &Path, seconds: u64) -> Option<SourceRevision> {
        let root = root.to_owned();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(read(&root));
        });
        receiver
            .recv_timeout(std::time::Duration::from_secs(seconds))
            .ok()
    }

    fn fifo(path: &Path) {
        let _ = fs::remove_file(path);
        assert!(Command::new("mkfifo").arg(path).status().unwrap().success());
    }

    // Review finding 2: a symlink in git metadata must not pull in an outside value.
    #[test]
    fn git_metadata_symlinks_stay_confined() {
        let outside = scratch("symlink-outside");
        put(&outside.join("value"), &format!("{COMMIT}\n"));
        put(&outside.join("heads/main"), &format!("{COMMIT}\n"));
        let root = scratch("symlink-root");
        put(&root.join(".git/HEAD"), "ref: refs/heads/main\n");
        put(&root.join(".git/refs/heads/main"), &format!("{OTHER}\n"));
        assert_eq!(read(&root), attested(Some("refs/heads/main"), OTHER));

        fs::remove_file(root.join(".git/HEAD")).unwrap();
        std::os::unix::fs::symlink(outside.join("value"), root.join(".git/HEAD")).unwrap();
        assert_eq!(
            read(&root),
            SourceRevision::Unavailable("git_metadata_unsafe")
        );

        fs::remove_file(root.join(".git/HEAD")).unwrap();
        put(&root.join(".git/HEAD"), "ref: refs/heads/main\n");
        fs::remove_file(root.join(".git/refs/heads/main")).unwrap();
        std::os::unix::fs::symlink(outside.join("value"), root.join(".git/refs/heads/main"))
            .unwrap();
        assert_eq!(read(&root), SourceRevision::Unavailable("ref_unsafe"));

        fs::remove_dir_all(root.join(".git/refs")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join(".git/refs")).unwrap();
        assert_eq!(read(&root), SourceRevision::Unavailable("ref_unsafe"));

        fs::remove_file(root.join(".git/refs")).unwrap();
        std::os::unix::fs::symlink(outside.join("value"), root.join(".git/packed-refs")).unwrap();
        assert_eq!(read(&root), SourceRevision::Unavailable("ref_unsafe"));

        // A symlink that stays inside the git directory is still followed.
        fs::remove_file(root.join(".git/packed-refs")).unwrap();
        put(&root.join(".git/refs/heads/main"), &format!("{OTHER}\n"));
        std::os::unix::fs::symlink("main", root.join(".git/refs/heads/alias")).unwrap();
        put(&root.join(".git/HEAD"), "ref: refs/heads/alias\n");
        assert_eq!(read(&root), attested(Some("refs/heads/alias"), OTHER));

        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    // Review finding 3: FIFOs in git metadata must not block a request.
    #[test]
    fn git_metadata_fifos_never_block() {
        let root = scratch("fifo");
        put(&root.join(".git/HEAD"), "ref: refs/heads/main\n");
        put(&root.join(".git/refs/heads/main"), &format!("{COMMIT}\n"));
        for (file, reason) in [
            ("HEAD", "git_metadata_unsafe"),
            ("commondir", "git_metadata_unsafe"),
            ("refs/heads/main", "ref_unsafe"),
        ] {
            let saved = fs::read(root.join(".git").join(file)).ok();
            fifo(&root.join(".git").join(file));
            assert_eq!(
                read_within(&root, 5),
                Some(SourceRevision::Unavailable(reason)),
                "{file}"
            );
            fs::remove_file(root.join(".git").join(file)).unwrap();
            if let Some(bytes) = saved {
                fs::write(root.join(".git").join(file), bytes).unwrap();
            }
        }
        fs::remove_file(root.join(".git/refs/heads/main")).unwrap();
        fifo(&root.join(".git/packed-refs"));
        assert_eq!(
            read_within(&root, 5),
            Some(SourceRevision::Unavailable("ref_unsafe"))
        );
        fs::remove_dir_all(root).unwrap();
    }

    // Review finding 4: worktree-local refs resolve in the worktree's git directory.
    #[test]
    fn worktree_local_refs_resolve_in_worktree_git_dir() {
        let main = scratch("local-main");
        put(&main.join(".git/HEAD"), "ref: refs/heads/main\n");
        put(
            &main.join(".git/refs/worktree/topic"),
            &format!("{COMMIT}\n"),
        );
        let worktree_dir = main.join(".git/worktrees/linked");
        put(&worktree_dir.join("HEAD"), "ref: refs/worktree/topic\n");
        put(&worktree_dir.join("commondir"), "../..\n");
        put(
            &worktree_dir.join("refs/worktree/topic"),
            &format!("{OTHER}\n"),
        );
        let linked = scratch("local-linked");
        put(
            &linked.join(".git"),
            &format!("gitdir: {}\n", worktree_dir.display()),
        );
        assert_eq!(read(&linked), attested(Some("refs/worktree/topic"), OTHER));
        // Per-worktree refs are never packed: no fallback to the shared packed-refs.
        fs::remove_file(worktree_dir.join("refs/worktree/topic")).unwrap();
        put(
            &main.join(".git/packed-refs"),
            &format!("{COMMIT} refs/worktree/topic\n"),
        );
        assert_eq!(read(&linked), SourceRevision::Unavailable("ref_unresolved"));

        // Real git agrees.
        let real = scratch("local-real");
        git(&real, &["init", "-q"]);
        fs::write(real.join("a.txt"), "a\n").unwrap();
        git(&real, &["add", "a.txt"]);
        git(&real, &["commit", "-q", "-m", "first"]);
        let first = git(&real, &["rev-parse", "HEAD"]);
        fs::write(real.join("b.txt"), "b\n").unwrap();
        git(&real, &["add", "b.txt"]);
        git(&real, &["commit", "-q", "-m", "second"]);
        let second = git(&real, &["rev-parse", "HEAD"]);
        let worktree = real.with_file_name(format!(
            "bran-source-revision-{}-local-real-worktree",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&worktree);
        git(
            &real,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                worktree.to_str().unwrap(),
                &first,
            ],
        );
        git(&real, &["update-ref", "refs/worktree/topic", &second]);
        git(&worktree, &["update-ref", "refs/worktree/topic", &first]);
        git(&worktree, &["symbolic-ref", "HEAD", "refs/worktree/topic"]);
        assert_eq!(git(&worktree, &["rev-parse", "HEAD"]), first);
        assert_eq!(
            read(&worktree),
            attested(Some("refs/worktree/topic"), &first)
        );

        for directory in [main, linked, real, worktree] {
            fs::remove_dir_all(directory).unwrap();
        }
    }
}
