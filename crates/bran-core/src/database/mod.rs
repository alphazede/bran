//! Database boundary: read-only evidence sources and BRAN-owned state storage
//! are separate ports with separate descriptors and credential scopes.
//!
//! See `docs/database-boundary.md`. Errors never echo descriptor values, so a
//! secret pasted into the wrong field cannot reach logs, receipts, or packets.

pub mod evidence;
pub mod sql;
pub mod state;

use crate::frontmatter::parse_frontmatter;
use crate::schema::YamlValue;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub use sql::RelationName;

/// The one port a credential reference may authorize.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialScope {
    Evidence,
    State,
}

impl CredentialScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Evidence => "evidence",
            Self::State => "state",
        }
    }
}

/// A reference to host-managed credential material: `credref:<scope>/<name>`.
/// BRAN never holds the material itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CredentialReference {
    scope: CredentialScope,
    name: String,
}

impl CredentialReference {
    /// Accepts only a well-formed reference in `scope`.
    pub fn parse(value: &str, scope: CredentialScope) -> Result<Self, DescriptorError> {
        let (found, name) = value
            .strip_prefix("credref:")
            .and_then(|rest| rest.split_once('/'))
            .filter(|(_, name)| valid_name(name))
            .ok_or(DescriptorError::CredentialNotReference)?;
        let found = match found {
            "evidence" => CredentialScope::Evidence,
            "state" => CredentialScope::State,
            _ => return Err(DescriptorError::CredentialNotReference),
        };
        if found != scope {
            return Err(DescriptorError::CredentialScope);
        }
        Ok(Self {
            scope,
            name: name.to_owned(),
        })
    }

    pub const fn scope(&self) -> CredentialScope {
        self.scope
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Display for CredentialReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "credref:{}/{}", self.scope.as_str(), self.name)
    }
}

/// Content-free descriptor failures. Field names are static; values are never kept.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DescriptorError {
    Malformed,
    WrongKind,
    UnknownKey,
    MissingKey(&'static str),
    InvalidValue(&'static str),
    LimitOutOfRange(&'static str),
    CredentialNotReference,
    CredentialScope,
    CredentialNotAllowed,
}

impl fmt::Display for DescriptorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed => f.write_str("descriptor is not supported YAML"),
            Self::WrongKind => f.write_str("descriptor kind belongs to another port"),
            Self::UnknownKey => f.write_str("descriptor has a key this port does not accept"),
            Self::MissingKey(key) => write!(f, "descriptor is missing {key}"),
            Self::InvalidValue(key) => write!(f, "descriptor {key} is invalid"),
            Self::LimitOutOfRange(key) => write!(f, "descriptor {key} is out of range"),
            Self::CredentialNotReference => {
                f.write_str("credential must be a credref:<scope>/<name> reference")
            }
            Self::CredentialScope => f.write_str("credential reference belongs to another port"),
            Self::CredentialNotAllowed => f.write_str("this engine or backend takes no credential"),
        }
    }
}

/// Evidence engines with a defined contract. Declaring one does not make an
/// adapter available.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Engine {
    Sqlite,
    Postgresql,
}

impl Engine {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sqlite => "sqlite",
            Self::Postgresql => "postgresql",
        }
    }
}

/// Evidence access. `Introspection` is the default.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessMode {
    Introspection,
    BoundedRows,
}

impl AccessMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Introspection => "introspection",
            Self::BoundedRows => "bounded-rows",
        }
    }
}

/// Validated per-source extraction limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EvidenceLimits {
    pub max_rows: usize,
    pub max_bytes: usize,
    pub timeout_ms: u64,
}

/// A validated `kind: evidence-source` descriptor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceSourceDescriptor {
    id: String,
    engine: Engine,
    database: String,
    credential: Option<CredentialReference>,
    access: AccessMode,
    relations: Vec<RelationName>,
    queries: Vec<String>,
    snapshot: Option<String>,
    limits: EvidenceLimits,
}

impl EvidenceSourceDescriptor {
    pub fn parse(text: &str) -> Result<Self, DescriptorError> {
        let fields = Fields::parse(
            text,
            "evidence-source",
            &[
                "kind",
                "id",
                "engine",
                "database",
                "credential",
                "access",
                "relations",
                "queries",
                "snapshot",
                "max_rows",
                "max_bytes",
                "timeout_ms",
            ],
        )?;
        let id = fields.name("id")?;
        let engine = match fields.required("engine")? {
            "sqlite" => Engine::Sqlite,
            "postgresql" => Engine::Postgresql,
            _ => return Err(DescriptorError::InvalidValue("engine")),
        };
        let database = fields.required("database")?;
        let credential = fields.credential("credential", CredentialScope::Evidence)?;
        match engine {
            Engine::Sqlite if !safe_relative_path(database) => {
                return Err(DescriptorError::InvalidValue("database"))
            }
            Engine::Postgresql if !valid_name(database) => {
                return Err(DescriptorError::InvalidValue("database"))
            }
            Engine::Sqlite if credential.is_some() => {
                return Err(DescriptorError::CredentialNotAllowed)
            }
            Engine::Postgresql if credential.is_none() => {
                return Err(DescriptorError::MissingKey("credential"))
            }
            _ => {}
        }
        let access = match fields.text("access")? {
            None | Some("introspection") => AccessMode::Introspection,
            Some("bounded-rows") => AccessMode::BoundedRows,
            Some(_) => return Err(DescriptorError::InvalidValue("access")),
        };
        let listed = fields
            .list("relations")?
            .ok_or(DescriptorError::MissingKey("relations"))?;
        let relations = listed
            .iter()
            .map(|name| RelationName::parse(name))
            .collect::<Option<BTreeSet<_>>>()
            .filter(|relations| !relations.is_empty() && relations.len() == listed.len())
            .ok_or(DescriptorError::InvalidValue("relations"))?;
        let queries = fields.list("queries")?.unwrap_or_default();
        if !queries.iter().all(|query| is_digest(query)) {
            return Err(DescriptorError::InvalidValue("queries"));
        }
        let snapshot = fields.text("snapshot")?;
        if snapshot.is_some_and(|value| !is_digest(value)) {
            return Err(DescriptorError::InvalidValue("snapshot"));
        }
        Ok(Self {
            id: id.to_owned(),
            engine,
            database: database.to_owned(),
            credential,
            access,
            relations: relations.into_iter().collect(),
            queries: queries.into_iter().map(str::to_owned).collect(),
            snapshot: snapshot.map(str::to_owned),
            limits: EvidenceLimits {
                max_rows: fields.number("max_rows", Some(100), 10_000)? as usize,
                max_bytes: fields.number("max_bytes", Some(65_536), 4 * 1024 * 1024)? as usize,
                timeout_ms: fields.number("timeout_ms", Some(5_000), 60_000)?,
            },
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }
    pub const fn engine(&self) -> Engine {
        self.engine
    }
    pub fn database(&self) -> &str {
        &self.database
    }
    pub fn credential(&self) -> Option<&CredentialReference> {
        self.credential.as_ref()
    }
    pub const fn access(&self) -> AccessMode {
        self.access
    }
    pub fn relations(&self) -> &[RelationName] {
        &self.relations
    }
    pub fn queries(&self) -> &[String] {
        &self.queries
    }
    pub fn snapshot(&self) -> Option<&str> {
        self.snapshot.as_deref()
    }
    pub const fn limits(&self) -> EvidenceLimits {
        self.limits
    }
}

/// State-store backends with a defined contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateBackend {
    File,
    Sqlite,
    Postgresql,
}

/// A validated `kind: state-store` descriptor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StateStoreDescriptor {
    id: String,
    backend: StateBackend,
    location: String,
    namespace: String,
    quota_bytes: u64,
    credential: Option<CredentialReference>,
    encryption_key: Option<CredentialReference>,
}

impl StateStoreDescriptor {
    pub fn parse(text: &str) -> Result<Self, DescriptorError> {
        let fields = Fields::parse(
            text,
            "state-store",
            &[
                "kind",
                "id",
                "backend",
                "location",
                "namespace",
                "quota_bytes",
                "credential",
                "encryption_key",
            ],
        )?;
        let id = fields.name("id")?;
        let backend = match fields.required("backend")? {
            "file" => StateBackend::File,
            "sqlite" => StateBackend::Sqlite,
            "postgresql" => StateBackend::Postgresql,
            _ => return Err(DescriptorError::InvalidValue("backend")),
        };
        let location = fields.required("location")?;
        if !safe_relative_path(location) {
            return Err(DescriptorError::InvalidValue("location"));
        }
        let namespace = fields.name("namespace")?;
        let quota_bytes = fields.number("quota_bytes", None, 1 << 30)?;
        let credential = fields.credential("credential", CredentialScope::State)?;
        match backend {
            StateBackend::Postgresql if credential.is_none() => {
                return Err(DescriptorError::MissingKey("credential"))
            }
            StateBackend::File | StateBackend::Sqlite if credential.is_some() => {
                return Err(DescriptorError::CredentialNotAllowed)
            }
            _ => {}
        }
        Ok(Self {
            id: id.to_owned(),
            backend,
            location: location.to_owned(),
            namespace: namespace.to_owned(),
            quota_bytes,
            credential,
            encryption_key: fields.credential("encryption_key", CredentialScope::State)?,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }
    pub const fn backend(&self) -> StateBackend {
        self.backend
    }
    pub fn location(&self) -> &str {
        &self.location
    }
    pub fn namespace(&self) -> &str {
        &self.namespace
    }
    pub const fn quota_bytes(&self) -> u64 {
        self.quota_bytes
    }
    pub fn credential(&self) -> Option<&CredentialReference> {
        self.credential.as_ref()
    }
    pub fn encryption_key(&self) -> Option<&CredentialReference> {
        self.encryption_key.as_ref()
    }
}

/// A descriptor mapping with exactly one `kind` and only the keys its port accepts.
struct Fields(BTreeMap<String, YamlValue>);

impl Fields {
    fn parse(text: &str, kind: &str, allowed: &[&str]) -> Result<Self, DescriptorError> {
        let map = parse_frontmatter(text).map_err(|_| DescriptorError::Malformed)?;
        match map.get("kind") {
            Some(YamlValue::String(found)) if found == kind => {}
            Some(_) => return Err(DescriptorError::WrongKind),
            None => return Err(DescriptorError::MissingKey("kind")),
        }
        if map.keys().any(|key| !allowed.contains(&key.as_str())) {
            return Err(DescriptorError::UnknownKey);
        }
        Ok(Self(map))
    }

    fn text(&self, key: &'static str) -> Result<Option<&str>, DescriptorError> {
        match self.0.get(key) {
            None => Ok(None),
            Some(YamlValue::String(value)) => Ok(Some(value)),
            Some(_) => Err(DescriptorError::InvalidValue(key)),
        }
    }

    fn required(&self, key: &'static str) -> Result<&str, DescriptorError> {
        self.text(key)?.ok_or(DescriptorError::MissingKey(key))
    }

    fn name(&self, key: &'static str) -> Result<&str, DescriptorError> {
        Some(self.required(key)?)
            .filter(|value| valid_name(value))
            .ok_or(DescriptorError::InvalidValue(key))
    }

    fn list(&self, key: &'static str) -> Result<Option<Vec<&str>>, DescriptorError> {
        match self.0.get(key) {
            None => Ok(None),
            Some(YamlValue::Sequence(items)) => items
                .iter()
                .map(|item| match item {
                    YamlValue::String(value) => Ok(value.as_str()),
                    _ => Err(DescriptorError::InvalidValue(key)),
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Some),
            Some(_) => Err(DescriptorError::InvalidValue(key)),
        }
    }

    fn number(
        &self,
        key: &'static str,
        default: Option<u64>,
        max: u64,
    ) -> Result<u64, DescriptorError> {
        match self.text(key)? {
            None => default.ok_or(DescriptorError::MissingKey(key)),
            Some(value) => value
                .parse::<u64>()
                .ok()
                .filter(|number| (1..=max).contains(number))
                .ok_or(DescriptorError::LimitOutOfRange(key)),
        }
    }

    fn credential(
        &self,
        key: &'static str,
        scope: CredentialScope,
    ) -> Result<Option<CredentialReference>, DescriptorError> {
        self.text(key)?
            .map(|value| CredentialReference::parse(value, scope))
            .transpose()
    }
}

/// `[a-z0-9][a-z0-9._-]{0,63}`: ids, namespaces, handles, credential names.
fn valid_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

/// A relative POSIX path of `[A-Za-z0-9._-]` segments, no `.`/`..`, at most 256 bytes.
fn safe_relative_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.split('/').all(|segment| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
}

fn is_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVIDENCE: &str = "\
kind: evidence-source
id: synthetic-orders
engine: sqlite
database: data/orders.sqlite
access: bounded-rows
relations: [main.orders, main.customers]
snapshot: sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
max_rows: 50
";

    const STATE: &str = "\
kind: state-store
id: local-state
backend: file
location: state
namespace: default
quota_bytes: 4096
";

    fn assert_quiet(error: DescriptorError, secret: &str) {
        let shown = format!("{error} {error:?}");
        assert!(!shown.contains(secret), "error reflected input: {shown}");
    }

    #[test]
    fn p8_database_boundary_config() {
        // Both descriptors parse with their own parser and defaults apply.
        let evidence = EvidenceSourceDescriptor::parse(EVIDENCE).unwrap();
        assert_eq!(evidence.id(), "synthetic-orders");
        assert_eq!(evidence.engine(), Engine::Sqlite);
        assert_eq!(evidence.access(), AccessMode::BoundedRows);
        let relations = evidence
            .relations()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        assert_eq!(relations, ["main.customers", "main.orders"]);
        assert_eq!(
            evidence.limits(),
            EvidenceLimits {
                max_rows: 50,
                max_bytes: 65_536,
                timeout_ms: 5_000
            }
        );
        let default_access =
            EvidenceSourceDescriptor::parse(&EVIDENCE.replace("access: bounded-rows\n", ""))
                .unwrap();
        assert_eq!(default_access.access(), AccessMode::Introspection);
        let state = StateStoreDescriptor::parse(STATE).unwrap();
        assert_eq!(state.backend(), StateBackend::File);
        assert_eq!(state.namespace(), "default");
        assert_eq!(state.quota_bytes(), 4096);

        // The ports cannot be configured interchangeably.
        assert_eq!(
            StateStoreDescriptor::parse(EVIDENCE),
            Err(DescriptorError::WrongKind)
        );
        assert_eq!(
            EvidenceSourceDescriptor::parse(STATE),
            Err(DescriptorError::WrongKind)
        );
        for foreign in [
            "namespace: default\n",
            "quota_bytes: 10\n",
            "backend: file\n",
            "location: state\n",
            "encryption_key: credref:state/key\n",
        ] {
            assert_eq!(
                EvidenceSourceDescriptor::parse(&format!("{EVIDENCE}{foreign}")),
                Err(DescriptorError::UnknownKey),
                "{foreign}"
            );
        }
        for foreign in [
            "engine: sqlite\n",
            "relations: [main.orders]\n",
            "access: bounded-rows\n",
            "max_rows: 5\n",
            "queries: [sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa]\n",
        ] {
            assert_eq!(
                StateStoreDescriptor::parse(&format!("{STATE}{foreign}")),
                Err(DescriptorError::UnknownKey),
                "{foreign}"
            );
        }
        let postgres = "\
kind: evidence-source
id: pg-orders
engine: postgresql
database: orders-replica
relations: [public.orders]
credential: credref:evidence/orders-reader
";
        let postgres_source = EvidenceSourceDescriptor::parse(postgres).unwrap();
        assert_eq!(
            postgres_source.credential().unwrap().to_string(),
            "credref:evidence/orders-reader"
        );
        assert_eq!(
            EvidenceSourceDescriptor::parse(
                &postgres.replace("credref:evidence/", "credref:state/")
            ),
            Err(DescriptorError::CredentialScope)
        );
        let pg_state = "\
kind: state-store
id: pg-state
backend: postgresql
location: bran-state
namespace: default
quota_bytes: 4096
credential: credref:state/state-writer
";
        assert!(StateStoreDescriptor::parse(pg_state).is_ok());
        assert_eq!(
            StateStoreDescriptor::parse(&pg_state.replace("credref:state/", "credref:evidence/")),
            Err(DescriptorError::CredentialScope)
        );
        assert_eq!(
            StateStoreDescriptor::parse(&format!("{STATE}encryption_key: credref:evidence/key\n")),
            Err(DescriptorError::CredentialScope)
        );

        // Only validated references to host-managed credentials are accepted.
        for (secret, credential) in [
            ("hunter2", "hunter2"),
            (
                "synthetic-password",
                "postgres://reader:synthetic-password@db.example.invalid/orders",
            ),
            ("PGPASSWORD", "env:PGPASSWORD"),
            ("synthetic-key-file", "file:/run/secrets/synthetic-key-file"),
            ("orders-reader", "credref:orders-reader"),
            ("nested", "credref:evidence/a/nested"),
            ("UPPER", "credref:evidence/UPPER"),
            ("space", "credref:evidence/with space"),
        ] {
            let error = EvidenceSourceDescriptor::parse(
                &postgres.replace("credref:evidence/orders-reader", credential),
            )
            .unwrap_err();
            assert_eq!(
                error,
                DescriptorError::CredentialNotReference,
                "{credential}"
            );
            assert_quiet(error, secret);
        }
        assert_eq!(
            EvidenceSourceDescriptor::parse(
                &postgres.replace("credential: credref:evidence/orders-reader\n", "")
            ),
            Err(DescriptorError::MissingKey("credential"))
        );
        assert_eq!(
            EvidenceSourceDescriptor::parse(&format!(
                "{EVIDENCE}credential: credref:evidence/local\n"
            )),
            Err(DescriptorError::CredentialNotAllowed)
        );
        assert_eq!(
            StateStoreDescriptor::parse(&format!("{STATE}credential: credref:state/local\n")),
            Err(DescriptorError::CredentialNotAllowed)
        );
        // Connection strings cannot hide in the database or location fields.
        for (field, source, value) in [
            ("database", EVIDENCE, "data/orders.sqlite"),
            ("database", postgres, "orders-replica"),
        ] {
            for smuggled in [
                "postgres://reader:synthetic-password@db.example.invalid/orders",
                "host=db.example.invalid password=synthetic-password",
                "file:orders.sqlite?mode=rwc",
                "/var/lib/orders.sqlite",
                "../outside/orders.sqlite",
            ] {
                let error =
                    EvidenceSourceDescriptor::parse(&source.replace(value, smuggled)).unwrap_err();
                assert_eq!(error, DescriptorError::InvalidValue(field), "{smuggled}");
                assert_quiet(error, "synthetic-password");
            }
        }
        for smuggled in ["../repository", "/absolute/state", "state/../..", "."] {
            assert_eq!(
                StateStoreDescriptor::parse(
                    &STATE.replace("location: state", &format!("location: {smuggled}"))
                ),
                Err(DescriptorError::InvalidValue("location")),
                "{smuggled}"
            );
        }

        // Relations, limits, and snapshots are validated, never guessed.
        for relations in [
            "[orders]",
            "[main.orders, main.orders]",
            "[main.Orders]",
            "[a.b.c]",
        ] {
            assert_eq!(
                EvidenceSourceDescriptor::parse(
                    &EVIDENCE.replace("[main.orders, main.customers]", relations)
                ),
                Err(DescriptorError::InvalidValue("relations")),
                "{relations}"
            );
        }
        assert_eq!(
            EvidenceSourceDescriptor::parse(&EVIDENCE.replace("max_rows: 50", "max_rows: 10001")),
            Err(DescriptorError::LimitOutOfRange("max_rows"))
        );
        assert_eq!(
            EvidenceSourceDescriptor::parse(&EVIDENCE.replace("max_rows: 50", "max_rows: 0")),
            Err(DescriptorError::LimitOutOfRange("max_rows"))
        );
        assert_eq!(
            EvidenceSourceDescriptor::parse(&format!("{EVIDENCE}queries: [select 1]\n")),
            Err(DescriptorError::InvalidValue("queries"))
        );
        assert_eq!(
            StateStoreDescriptor::parse(&STATE.replace("quota_bytes: 4096", "quota_bytes: -1")),
            Err(DescriptorError::LimitOutOfRange("quota_bytes"))
        );
        assert_eq!(
            EvidenceSourceDescriptor::parse("kind: [evidence-source]\n"),
            Err(DescriptorError::WrongKind)
        );
        assert_eq!(
            EvidenceSourceDescriptor::parse("not yaml at all"),
            Err(DescriptorError::Malformed)
        );
    }
}
