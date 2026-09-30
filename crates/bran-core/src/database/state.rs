//! The BRAN-owned state-store port and its first durable backend.
//!
//! State is derived and discardable: a store never reads or writes repository
//! content and is never a source of truth for canonical documents.

use super::{StateBackend, StateStoreDescriptor};
use crate::agent::result_store::ResultId;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The only on-disk layout this build reads or writes.
pub const STATE_LAYOUT_VERSION: &str = "1";

/// Content-free state-store failures: no paths, bytes, or credentials.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateStoreError {
    Unavailable,
    Corrupt,
    MigrationFailed,
    QuotaExceeded,
    NotFound,
    EmptyInput,
    /// A path below the state root is a symlink or not the expected kind of entry.
    Unconfined,
    Io(io::ErrorKind),
}

impl fmt::Display for StateStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => f.write_str("state backend or capability is unavailable"),
            Self::Corrupt => f.write_str("state object or layout failed verification"),
            Self::MigrationFailed => f.write_str("state layout version is not supported"),
            Self::QuotaExceeded => f.write_str("state quota would be exceeded"),
            Self::NotFound => f.write_str("state object not found"),
            Self::EmptyInput => f.write_str("state object is empty"),
            Self::Unconfined => {
                f.write_str("state path crosses a symlink or unexpected entry below its root")
            }
            Self::Io(kind) => write!(f, "state storage I/O failure: {kind:?}"),
        }
    }
}

impl From<io::Error> for StateStoreError {
    fn from(error: io::Error) -> Self {
        Self::Io(error.kind())
    }
}

/// The state-store port: content-addressed objects inside one namespace.
pub trait StateStore {
    fn put(&mut self, bytes: &[u8]) -> Result<ResultId, StateStoreError>;
    fn get(&self, id: &ResultId) -> Result<Vec<u8>, StateStoreError>;
    fn used_bytes(&self) -> u64;
}

/// Durable single-node store under `<state root>/<location>/<namespace>/`.
///
/// Every directory and file below the host-supplied state root is inspected
/// without following symlinks before it is read, written, or removed.
// ponytail: checks precede use (no openat/O_NOFOLLOW in std), so a concurrent
// writer inside the state root could still race them; one writer per namespace,
// and concurrent server writers need the PostgreSQL backend.
#[derive(Debug)]
pub struct FileStateStore {
    dir: PathBuf,
    quota: u64,
    used: u64,
    recovered_partial_writes: usize,
    next_temp: u64,
}

/// `Some(metadata)` for an existing entry, never following a symlink.
fn entry(path: &Path) -> Result<Option<fs::Metadata>, StateStoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Whether a real directory exists at `path`; a symlink or file is refused.
fn real_dir(path: &Path) -> Result<bool, StateStoreError> {
    match entry(path)? {
        None => Ok(false),
        Some(metadata) if metadata.is_dir() => Ok(true),
        Some(_) => Err(StateStoreError::Unconfined),
    }
}

/// The length of a regular file at `path`; a symlink or directory is refused.
fn regular_file(path: &Path) -> Result<Option<u64>, StateStoreError> {
    match entry(path)? {
        None => Ok(None),
        Some(metadata) if metadata.is_file() => Ok(Some(metadata.len())),
        Some(_) => Err(StateStoreError::Unconfined),
    }
}

/// Every entry of `dir` with its length; anything but a regular file is refused.
fn regular_files(dir: &Path) -> Result<Vec<(PathBuf, u64)>, StateStoreError> {
    let mut files = Vec::new();
    for item in fs::read_dir(dir)? {
        let path = item?.path();
        let length = regular_file(&path)?.ok_or(StateStoreError::Unconfined)?;
        files.push((path, length));
    }
    Ok(files)
}

impl FileStateStore {
    /// Opens (or initialises) one namespace. Refuses without writing on any
    /// unsupported backend, requested encryption, unknown layout version, or
    /// path that crosses a symlink below `state_root`.
    pub fn open(
        descriptor: &StateStoreDescriptor,
        state_root: &Path,
    ) -> Result<Self, StateStoreError> {
        if descriptor.backend() != StateBackend::File || descriptor.encryption_key().is_some() {
            return Err(StateStoreError::Unavailable);
        }
        let mut components = vec![state_root.to_path_buf()];
        for segment in descriptor
            .location()
            .split('/')
            .chain([descriptor.namespace()])
        {
            let next = components[components.len() - 1].join(segment);
            components.push(next);
        }
        let dir = components[components.len() - 1].clone();
        let mut present = true;
        for component in &components[1..] {
            present = present && real_dir(component)?;
        }
        let objects = dir.join("objects");
        let staging = dir.join("tmp");
        let version = match present {
            true => regular_file(&dir.join("VERSION"))?,
            false => None,
        };
        if version.is_some() {
            let bytes = fs::read(dir.join("VERSION"))?;
            let found = std::str::from_utf8(&bytes)
                .ok()
                .and_then(|text| text.strip_suffix('\n'))
                .filter(|text| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
                .ok_or(StateStoreError::Corrupt)?;
            if found != STATE_LAYOUT_VERSION {
                return Err(StateStoreError::MigrationFailed);
            }
        } else if present && fs::read_dir(&dir)?.next().is_some() {
            return Err(StateStoreError::Corrupt);
        }
        let objects_present = present && real_dir(&objects)?;
        let staging_present = present && real_dir(&staging)?;
        let staged = match staging_present {
            true => regular_files(&staging)?,
            false => Vec::new(),
        };
        let stored = match objects_present {
            true => regular_files(&objects)?,
            false => Vec::new(),
        };
        // Everything below the root is now known to be real; only then mutate.
        fs::create_dir_all(state_root)?;
        for component in &components[1..] {
            if !real_dir(component)? {
                fs::create_dir(component)?;
            }
        }
        if version.is_none() {
            fs::write(dir.join("VERSION"), format!("{STATE_LAYOUT_VERSION}\n"))?;
        }
        for (present, path) in [(objects_present, &objects), (staging_present, &staging)] {
            if !present {
                fs::create_dir(path)?;
            }
        }
        for (path, _) in &staged {
            fs::remove_file(path)?;
        }
        Ok(Self {
            dir,
            quota: descriptor.quota_bytes(),
            used: stored.iter().map(|(_, length)| length).sum(),
            recovered_partial_writes: staged.len(),
            next_temp: 0,
        })
    }

    fn object(&self, id: &ResultId) -> PathBuf {
        self.dir.join("objects").join(id.value())
    }

    /// Temp files from interrupted writes that `open` removed.
    pub const fn recovered_partial_writes(&self) -> usize {
        self.recovered_partial_writes
    }

    /// Re-hashes every object. Returns the object count, or `Corrupt`.
    pub fn verify(&self) -> Result<usize, StateStoreError> {
        let objects = regular_files(&self.dir.join("objects"))?;
        for (path, _) in &objects {
            let id = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|hex| ResultId::parse(&format!("sha256:{hex}")).ok())
                .ok_or(StateStoreError::Corrupt)?;
            self.get(&id)?;
        }
        Ok(objects.len())
    }
}

impl StateStore for FileStateStore {
    fn put(&mut self, bytes: &[u8]) -> Result<ResultId, StateStoreError> {
        if bytes.is_empty() {
            return Err(StateStoreError::EmptyInput);
        }
        let id = ResultId::sha256(bytes);
        let path = self.object(&id);
        let existing = regular_file(&path)?;
        if existing.is_some() && fs::read(&path)? == bytes {
            return Ok(id);
        }
        // A damaged object is replaced. Usage is recounted from disk, because
        // the damage changed its size behind this handle's back.
        let used = match existing {
            Some(_) => regular_files(&self.dir.join("objects"))?
                .iter()
                .map(|(_, length)| length)
                .sum(),
            None => self.used,
        };
        let replacing = existing.unwrap_or(0);
        let total = used
            .saturating_sub(replacing)
            .checked_add(bytes.len() as u64)
            .filter(|total| *total <= self.quota)
            .ok_or(StateStoreError::QuotaExceeded)?;
        let temp = self
            .dir
            .join("tmp")
            .join(format!("{}.{}", id.value(), self.next_temp));
        self.next_temp += 1;
        // ponytail: no parent-directory fsync after rename; add it if power-loss durability matters.
        let written = fs::File::create_new(&temp)
            .and_then(|mut file| file.write_all(bytes).and_then(|()| file.sync_all()))
            .and_then(|()| fs::rename(&temp, &path));
        if let Err(error) = written {
            let _ = fs::remove_file(&temp);
            return Err(error.into());
        }
        self.used = total;
        Ok(id)
    }

    fn get(&self, id: &ResultId) -> Result<Vec<u8>, StateStoreError> {
        let path = self.object(id);
        regular_file(&path)?.ok_or(StateStoreError::NotFound)?;
        let bytes = fs::read(&path)?;
        if ResultId::sha256(&bytes) != *id {
            return Err(StateStoreError::Corrupt);
        }
        Ok(bytes)
    }

    fn used_bytes(&self) -> u64 {
        self.used
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;

    fn descriptor(extra: &str) -> StateStoreDescriptor {
        StateStoreDescriptor::parse(&format!(
            "kind: state-store\nid: local-state\nbackend: file\nlocation: state\nnamespace: alpha\nquota_bytes: 64\n{extra}"
        ))
        .unwrap()
    }

    fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut files = BTreeMap::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(dir) = pending.pop() {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    files.insert(path.clone(), fs::read(path).unwrap());
                }
            }
        }
        files
    }

    #[test]
    fn p8_state_store_failures() {
        let base = std::env::temp_dir().join(format!("bran-p8-state-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let repository = base.join("repository");
        let state_root = base.join("state-root");
        fs::create_dir_all(repository.join("docs")).unwrap();
        fs::write(repository.join("docs/canonical.md"), "# Canonical\n").unwrap();
        fs::write(repository.join("README.md"), "synthetic repository\n").unwrap();
        let canonical = snapshot(&repository);

        // Round trip, content addressing, idempotence, and usage.
        let mut store = FileStateStore::open(&descriptor(""), &state_root).unwrap();
        let first = store.put(b"receipt-one").unwrap();
        assert_eq!(first, ResultId::sha256(b"receipt-one"));
        assert_eq!(store.put(b"receipt-one").unwrap(), first);
        assert_eq!(store.get(&first).unwrap(), b"receipt-one");
        let second = store.put(b"index-two").unwrap();
        assert_eq!(store.used_bytes(), 20);
        assert_eq!(store.verify(), Ok(2));
        assert_eq!(store.put(b""), Err(StateStoreError::EmptyInput));
        let namespace = state_root.join("state/alpha");
        assert_eq!(
            fs::read_to_string(namespace.join("VERSION")).unwrap(),
            "1\n"
        );

        // Quota exhaustion: refused before any byte is written.
        assert_eq!(store.put(&[b'q'; 45]), Err(StateStoreError::QuotaExceeded));
        assert_eq!(store.used_bytes(), 20);
        assert_eq!(fs::read_dir(namespace.join("tmp")).unwrap().count(), 0);
        assert_eq!(store.verify(), Ok(2));

        // Namespace isolation.
        let other = FileStateStore::open(
            &StateStoreDescriptor::parse(
                "kind: state-store\nid: local-state\nbackend: file\nlocation: state\nnamespace: beta\nquota_bytes: 64\n",
            )
            .unwrap(),
            &state_root,
        )
        .unwrap();
        assert_eq!(other.get(&first), Err(StateStoreError::NotFound));

        // Corruption: a modified or truncated object is never returned as data.
        let object = |id: &ResultId| namespace.join("objects").join(id.value());
        fs::write(object(&second), b"index-tw0").unwrap();
        assert_eq!(store.get(&second), Err(StateStoreError::Corrupt));
        assert_eq!(store.verify(), Err(StateStoreError::Corrupt));
        assert_eq!(store.get(&first).unwrap(), b"receipt-one");
        fs::write(object(&second), b"index").unwrap();
        assert_eq!(store.get(&second), Err(StateStoreError::Corrupt));
        // Putting the true bytes again repairs the object.
        assert_eq!(store.put(b"index-two").unwrap(), second);
        assert_eq!(store.verify(), Ok(2));
        assert_eq!(store.used_bytes(), 20);

        // Partial write: a crash between write and rename leaves only a temp file.
        drop(store);
        fs::write(namespace.join("tmp/interrupted.0"), b"half-writt").unwrap();
        let store = FileStateStore::open(&descriptor(""), &state_root).unwrap();
        assert_eq!(store.recovered_partial_writes(), 1);
        assert_eq!(fs::read_dir(namespace.join("tmp")).unwrap().count(), 0);
        assert_eq!(
            store.get(&ResultId::sha256(b"half-writt")),
            Err(StateStoreError::NotFound)
        );
        assert_eq!(store.used_bytes(), 20);
        assert_eq!(store.verify(), Ok(2));
        drop(store);

        // Migration failure: an unknown layout version refuses without writing.
        let before = snapshot(&state_root);
        fs::write(namespace.join("VERSION"), "2\n").unwrap();
        let versioned = snapshot(&state_root);
        assert_eq!(
            FileStateStore::open(&descriptor(""), &state_root).unwrap_err(),
            StateStoreError::MigrationFailed
        );
        assert_eq!(snapshot(&state_root), versioned);
        fs::write(namespace.join("VERSION"), "one\n").unwrap();
        assert_eq!(
            FileStateStore::open(&descriptor(""), &state_root).unwrap_err(),
            StateStoreError::Corrupt
        );
        fs::write(namespace.join("VERSION"), "1\n").unwrap();
        assert_eq!(snapshot(&state_root), before);
        // An unversioned, non-empty directory is not adopted.
        let stray = state_root.join("state/gamma");
        fs::create_dir_all(&stray).unwrap();
        fs::write(stray.join("unknown"), b"x").unwrap();
        let gamma = StateStoreDescriptor::parse(
            "kind: state-store\nid: local-state\nbackend: file\nlocation: state\nnamespace: gamma\nquota_bytes: 64\n",
        )
        .unwrap();
        assert_eq!(
            FileStateStore::open(&gamma, &state_root).unwrap_err(),
            StateStoreError::Corrupt
        );
        assert!(!stray.join("VERSION").exists());

        // Unavailable backends and capabilities are reported, never simulated.
        let before = snapshot(&state_root);
        assert_eq!(
            FileStateStore::open(
                &descriptor("encryption_key: credref:state/state-key\n"),
                &state_root
            )
            .unwrap_err(),
            StateStoreError::Unavailable
        );
        for backend in [
            "backend: sqlite\n",
            "backend: postgresql\ncredential: credref:state/state-writer\n",
        ] {
            let text = format!(
                "kind: state-store\nid: other\n{backend}location: state\nnamespace: delta\nquota_bytes: 64\n"
            );
            assert_eq!(
                FileStateStore::open(&StateStoreDescriptor::parse(&text).unwrap(), &state_root)
                    .unwrap_err(),
                StateStoreError::Unavailable
            );
        }
        assert_eq!(snapshot(&state_root), before);

        // Errors are content-free, and canonical repository data is untouched.
        for error in [
            StateStoreError::Corrupt,
            StateStoreError::Io(io::ErrorKind::PermissionDenied),
        ] {
            let shown = format!("{error} {error:?}");
            assert!(!shown.contains(state_root.to_str().unwrap()), "{shown}");
        }
        assert_eq!(snapshot(&repository), canonical);
        fs::remove_dir_all(&base).unwrap();
    }

    fn scratch(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("bran-p8-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        base
    }

    fn quota_eight() -> StateStoreDescriptor {
        StateStoreDescriptor::parse(
            "kind: state-store\nid: local-state\nbackend: file\nlocation: state\nnamespace: alpha\nquota_bytes: 8\n",
        )
        .unwrap()
    }

    fn disk_bytes(dir: &Path) -> u64 {
        fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().metadata().unwrap().len())
            .sum()
    }

    #[test]
    fn repair_accounts_quota() {
        // Review F6: repairing a truncated object must count its true size.
        let base = scratch("state-repair");
        let objects = base.join("state/alpha/objects");
        let mut store = FileStateStore::open(&quota_eight(), &base).unwrap();
        let id = store.put(b"12345678").unwrap();
        drop(store);
        fs::write(objects.join(id.value()), b"x").unwrap();
        let mut store = FileStateStore::open(&quota_eight(), &base).unwrap();
        assert_eq!(store.put(b"12345678").unwrap(), id);
        assert_eq!(store.used_bytes(), 8);
        assert_eq!(store.put(b"abcdefg"), Err(StateStoreError::QuotaExceeded));
        assert_eq!(store.used_bytes(), 8);
        assert_eq!(disk_bytes(&objects), 8);
        // The same holds when the object is damaged while the store is open.
        fs::write(objects.join(id.value()), b"x").unwrap();
        assert_eq!(store.put(b"12345678").unwrap(), id);
        assert_eq!(store.used_bytes(), disk_bytes(&objects));
        fs::remove_dir_all(&base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn state_paths_refuse_symlink_escape() {
        // Review F1: no read, write, or cleanup may follow a symlink out of the state root.
        use std::os::unix::fs::symlink;
        let base = scratch("state-symlink");
        let outside = base.join("canonical");
        let root = base.join("root");
        fs::create_dir_all(outside.join("alpha/tmp")).unwrap();
        fs::write(outside.join("alpha/VERSION"), "1\n").unwrap();
        fs::write(outside.join("alpha/tmp/canonical.md"), "# Canonical\n").unwrap();
        fs::create_dir_all(&root).unwrap();
        let before = snapshot(&outside);

        // The location itself is a symlink out of the root.
        symlink(&outside, root.join("state")).unwrap();
        assert_eq!(
            FileStateStore::open(&quota_eight(), &root).unwrap_err(),
            StateStoreError::Unconfined
        );
        assert_eq!(snapshot(&outside), before);
        fs::remove_file(root.join("state")).unwrap();

        // The namespace directory is a symlink.
        fs::create_dir_all(root.join("state")).unwrap();
        symlink(outside.join("alpha"), root.join("state/alpha")).unwrap();
        assert_eq!(
            FileStateStore::open(&quota_eight(), &root).unwrap_err(),
            StateStoreError::Unconfined
        );
        assert_eq!(snapshot(&outside), before);
        fs::remove_file(root.join("state/alpha")).unwrap();

        // The staging or object directory, or a staged entry, is a symlink.
        FileStateStore::open(&quota_eight(), &root).unwrap();
        let namespace = root.join("state/alpha");
        fs::remove_dir(namespace.join("tmp")).unwrap();
        symlink(outside.join("alpha/tmp"), namespace.join("tmp")).unwrap();
        assert_eq!(
            FileStateStore::open(&quota_eight(), &root).unwrap_err(),
            StateStoreError::Unconfined
        );
        assert_eq!(snapshot(&outside), before);
        fs::remove_file(namespace.join("tmp")).unwrap();
        fs::create_dir(namespace.join("tmp")).unwrap();
        symlink(
            outside.join("alpha/tmp/canonical.md"),
            namespace.join("tmp/staged"),
        )
        .unwrap();
        assert_eq!(
            FileStateStore::open(&quota_eight(), &root).unwrap_err(),
            StateStoreError::Unconfined
        );
        assert_eq!(snapshot(&outside), before);
        fs::remove_file(namespace.join("tmp/staged")).unwrap();
        // An object entry that is a symlink is refused, not followed.
        let mut store = FileStateStore::open(&quota_eight(), &root).unwrap();
        let id = ResultId::sha256(b"# Canonical\n");
        symlink(
            outside.join("alpha/tmp/canonical.md"),
            namespace.join("objects").join(id.value()),
        )
        .unwrap();
        assert_eq!(store.get(&id), Err(StateStoreError::Unconfined));
        assert_eq!(
            store.put(b"# Canonical\n"),
            Err(StateStoreError::Unconfined)
        );
        assert_eq!(snapshot(&outside), before);
        drop(store);
        fs::remove_file(namespace.join("objects").join(id.value())).unwrap();
        fs::remove_dir(namespace.join("objects")).unwrap();
        symlink(outside.join("alpha"), namespace.join("objects")).unwrap();
        assert_eq!(
            FileStateStore::open(&quota_eight(), &root).unwrap_err(),
            StateStoreError::Unconfined
        );
        assert_eq!(snapshot(&outside), before);
        fs::remove_dir_all(&base).unwrap();
    }
}
