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
// ponytail: one writer per namespace; concurrent server writers need the PostgreSQL backend.
#[derive(Debug)]
pub struct FileStateStore {
    dir: PathBuf,
    quota: u64,
    used: u64,
    recovered_partial_writes: usize,
    next_temp: u64,
}

impl FileStateStore {
    /// Opens (or initialises) one namespace. Refuses without writing on any
    /// unsupported backend, requested encryption, or unknown layout version.
    pub fn open(
        descriptor: &StateStoreDescriptor,
        state_root: &Path,
    ) -> Result<Self, StateStoreError> {
        if descriptor.backend() != StateBackend::File || descriptor.encryption_key().is_some() {
            return Err(StateStoreError::Unavailable);
        }
        let dir = state_root
            .join(descriptor.location())
            .join(descriptor.namespace());
        match fs::read(dir.join("VERSION")) {
            Ok(bytes) => {
                let version = std::str::from_utf8(&bytes)
                    .ok()
                    .and_then(|text| text.strip_suffix('\n'))
                    .filter(|text| {
                        !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
                    })
                    .ok_or(StateStoreError::Corrupt)?;
                if version != STATE_LAYOUT_VERSION {
                    return Err(StateStoreError::MigrationFailed);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if dir.exists() && fs::read_dir(&dir)?.next().is_some() {
                    return Err(StateStoreError::Corrupt);
                }
                fs::create_dir_all(&dir)?;
                fs::write(dir.join("VERSION"), format!("{STATE_LAYOUT_VERSION}\n"))?;
            }
            Err(error) => return Err(error.into()),
        }
        fs::create_dir_all(dir.join("objects"))?;
        fs::create_dir_all(dir.join("tmp"))?;
        let mut recovered_partial_writes = 0;
        for entry in fs::read_dir(dir.join("tmp"))? {
            fs::remove_file(entry?.path())?;
            recovered_partial_writes += 1;
        }
        let mut used = 0;
        for entry in fs::read_dir(dir.join("objects"))? {
            used += entry?.metadata()?.len();
        }
        Ok(Self {
            dir,
            quota: descriptor.quota_bytes(),
            used,
            recovered_partial_writes,
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
        let mut count = 0;
        for entry in fs::read_dir(self.dir.join("objects"))? {
            let name = entry?.file_name();
            let id = name
                .to_str()
                .and_then(|hex| ResultId::parse(&format!("sha256:{hex}")).ok())
                .ok_or(StateStoreError::Corrupt)?;
            self.get(&id)?;
            count += 1;
        }
        Ok(count)
    }
}

impl StateStore for FileStateStore {
    fn put(&mut self, bytes: &[u8]) -> Result<ResultId, StateStoreError> {
        if bytes.is_empty() {
            return Err(StateStoreError::EmptyInput);
        }
        let id = ResultId::sha256(bytes);
        let path = self.object(&id);
        // An existing object is already counted at its true size; a corrupt one is rewritten.
        let exists = match fs::read(&path) {
            Ok(existing) if existing == bytes => return Ok(id),
            Ok(_) => true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        let size = bytes.len() as u64;
        if !exists
            && self
                .used
                .checked_add(size)
                .is_none_or(|total| total > self.quota)
        {
            return Err(StateStoreError::QuotaExceeded);
        }
        let temp = self
            .dir
            .join("tmp")
            .join(format!("{}.{}", id.value(), self.next_temp));
        self.next_temp += 1;
        // ponytail: no parent-directory fsync after rename; add it if power-loss durability matters.
        let written = fs::File::create(&temp)
            .and_then(|mut file| file.write_all(bytes).and_then(|()| file.sync_all()))
            .and_then(|()| fs::rename(&temp, &path));
        if let Err(error) = written {
            let _ = fs::remove_file(&temp);
            return Err(error.into());
        }
        if !exists {
            self.used += size;
        }
        Ok(id)
    }

    fn get(&self, id: &ResultId) -> Result<Vec<u8>, StateStoreError> {
        let bytes = fs::read(self.object(id)).map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => StateStoreError::NotFound,
            kind => StateStoreError::Io(kind),
        })?;
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
}
