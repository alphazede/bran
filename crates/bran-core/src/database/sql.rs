//! Deterministic SQL gate. [`gate`] is the only constructor of [`GatedQuery`],
//! so operator SQL, model-generated SQL, and BRAN-generated projections all
//! pass the same checks before an engine sees them.

use crate::agent::result_store::ResultId;
use std::collections::BTreeSet;
use std::fmt;

/// Maximum accepted SQL text.
pub const MAX_SQL_BYTES: usize = 8 * 1024;
/// Maximum accepted token count.
pub const MAX_SQL_TOKENS: usize = 1024;
/// The only callable functions. Everything else is [`SqlRejection::UnsafeFunction`].
pub const ALLOWED_FUNCTIONS: &[&str] = &[
    "abs", "avg", "coalesce", "count", "length", "lower", "max", "min", "round", "sum", "upper",
];

/// A fully qualified `schema.relation` name in lowercase `[a-z_][a-z0-9_]*` parts.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RelationName {
    schema: String,
    relation: String,
}

impl RelationName {
    pub fn parse(value: &str) -> Option<Self> {
        let (schema, relation) = value.split_once('.')?;
        (valid_part(schema) && valid_part(relation)).then(|| Self {
            schema: schema.to_owned(),
            relation: relation.to_owned(),
        })
    }

    pub fn schema(&self) -> &str {
        &self.schema
    }

    pub fn relation(&self) -> &str {
        &self.relation
    }
}

impl fmt::Display for RelationName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.schema, self.relation)
    }
}

/// Why SQL was refused. Content-free: it never carries the input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SqlRejection {
    Empty,
    TooLarge,
    Malformed,
    Comment,
    MultiStatement,
    UnsupportedCharacter,
    UnterminatedLiteral,
    NotSelect,
    WriteOrDdl,
    SessionMutation,
    ProcedureCall,
    ExternalReference,
    Compound,
    UnsafeFunction,
    UnqualifiedRelation,
    RelationNotAllowed,
    Unordered,
    InvalidProjection,
}

impl fmt::Display for SqlRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Empty => "empty SQL",
            Self::TooLarge => "SQL exceeds the size limit",
            Self::Malformed => "SQL shape is not supported",
            Self::Comment => "SQL comments are not accepted",
            Self::MultiStatement => "only one statement is accepted",
            Self::UnsupportedCharacter => "SQL contains an unsupported character",
            Self::UnterminatedLiteral => "SQL literal is not terminated",
            Self::NotSelect => "only SELECT is accepted",
            Self::WriteOrDdl => "write or DDL keyword",
            Self::SessionMutation => "session or transaction keyword",
            Self::ProcedureCall => "procedure execution",
            Self::ExternalReference => "external database reference",
            Self::Compound => "compound or nested query",
            Self::UnsafeFunction => "function is not allowlisted",
            Self::UnqualifiedRelation => "relation must be schema-qualified",
            Self::RelationNotAllowed => "relation is not allowlisted",
            Self::Unordered => "row query needs ORDER BY",
            Self::InvalidProjection => "projection columns are invalid",
        })
    }
}

/// How the gated SQL was produced.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueryDerivation {
    Projection,
    Query,
}

impl QueryDerivation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Projection => "projection",
            Self::Query => "query",
        }
    }
}

/// SQL that passed the gate. The text is the normalized form that was digested.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GatedQuery {
    sql: String,
    digest: String,
    relations: Vec<RelationName>,
    derivation: QueryDerivation,
}

impl GatedQuery {
    pub fn sql(&self) -> &str {
        &self.sql
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }
    pub fn relations(&self) -> &[RelationName] {
        &self.relations
    }
    pub const fn derivation(&self) -> QueryDerivation {
        self.derivation
    }
}

/// Gates one SQL text against a relation allowlist.
pub fn gate(sql: &str, allowed: &[RelationName]) -> Result<GatedQuery, SqlRejection> {
    gate_as(sql, allowed, QueryDerivation::Query)
}

/// Builds `SELECT "c1", ... FROM "schema"."relation" ORDER BY 1, ...` and gates it.
pub fn project(
    relation: &RelationName,
    columns: &[&str],
    allowed: &[RelationName],
) -> Result<GatedQuery, SqlRejection> {
    let unique = columns.iter().collect::<BTreeSet<_>>();
    if columns.is_empty()
        || columns.len() > 256
        || unique.len() != columns.len()
        || !columns.iter().all(|column| valid_part(column))
    {
        return Err(SqlRejection::InvalidProjection);
    }
    let list = columns
        .iter()
        .map(|column| format!("\"{column}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let order = (1..=columns.len())
        .map(|position| position.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    gate_as(
        &format!(
            "SELECT {list} FROM \"{}\".\"{}\" ORDER BY {order}",
            relation.schema, relation.relation
        ),
        allowed,
        QueryDerivation::Projection,
    )
}

const EXTERNAL: &[&str] = &[
    "attach",
    "detach",
    "load",
    "import",
    "openrowset",
    "openquery",
    "opendatasource",
    "dblink",
    "dblink_exec",
    "dblink_connect",
];
const WRITE: &[&str] = &[
    "insert",
    "update",
    "delete",
    "merge",
    "replace",
    "upsert",
    "create",
    "drop",
    "alter",
    "truncate",
    "grant",
    "revoke",
    "copy",
    "into",
    "returning",
    "vacuum",
    "reindex",
    "analyze",
    "comment",
    "rename",
    "cluster",
    "refresh",
];
const SESSION: &[&str] = &[
    "set",
    "reset",
    "pragma",
    "begin",
    "commit",
    "rollback",
    "savepoint",
    "release",
    "start",
    "transaction",
    "lock",
    "for",
    "listen",
    "notify",
    "unlisten",
    "discard",
    "use",
    "declare",
    "fetch",
    "close",
    "prepare",
    "deallocate",
    "checkpoint",
];
const PROCEDURE: &[&str] = &["call", "exec", "execute", "do"];
const COMPOUND: &[&str] = &["union", "intersect", "except"];
/// Words that may precede `(` without being a function call, and are never names.
const GRAMMAR: &[&str] = &[
    "select", "distinct", "all", "from", "where", "and", "or", "not", "as", "join", "inner",
    "left", "outer", "cross", "on", "group", "by", "order", "asc", "desc", "limit", "offset",
    "having", "is", "null", "in", "between", "like", "true", "false", "nulls", "first", "last",
];

#[derive(Clone, Debug, Eq, PartialEq)]
enum Token {
    /// Unquoted word, lowercased.
    Word(String),
    /// Double-quoted identifier content.
    Quoted(String),
    /// String or number literal, verbatim.
    Literal(String),
    Param(String),
    Symbol(&'static str),
}

impl Token {
    fn is_word(&self, word: &str) -> bool {
        matches!(self, Self::Word(found) if found == word)
    }

    /// A column, relation, or alias name.
    fn name(&self) -> Option<&str> {
        match self {
            Self::Word(word) if !GRAMMAR.contains(&word.as_str()) => Some(word),
            Self::Quoted(name) => Some(name),
            _ => None,
        }
    }

    fn text(&self) -> String {
        match self {
            Self::Word(text) | Self::Literal(text) | Self::Param(text) => text.clone(),
            Self::Quoted(name) => format!("\"{name}\""),
            Self::Symbol(symbol) => (*symbol).to_owned(),
        }
    }
}

fn gate_as(
    sql: &str,
    allowed: &[RelationName],
    derivation: QueryDerivation,
) -> Result<GatedQuery, SqlRejection> {
    if sql.len() > MAX_SQL_BYTES {
        return Err(SqlRejection::TooLarge);
    }
    let tokens = tokenize(sql)?;
    if tokens.is_empty() {
        return Err(SqlRejection::Empty);
    }
    if let Some(rejection) = tokens.iter().find_map(|token| match token {
        Token::Word(word) => forbidden(word),
        _ => None,
    }) {
        return Err(rejection);
    }
    if tokens
        .iter()
        .filter(|token| token.is_word("select"))
        .count()
        > 1
    {
        return Err(SqlRejection::Compound);
    }
    if !tokens[0].is_word("select") {
        return Err(SqlRejection::NotSelect);
    }
    // A call must name an allowlisted function directly; `schema.count(` could
    // resolve to a user-defined function in another schema.
    for (index, pair) in tokens.windows(2).enumerate() {
        let unlisted = match &pair[0] {
            Token::Word(word) => {
                !GRAMMAR.contains(&word.as_str()) && !ALLOWED_FUNCTIONS.contains(&word.as_str())
            }
            Token::Quoted(_) => true,
            _ => false,
        };
        let qualified = index > 0 && tokens[index - 1] == Token::Symbol(".");
        if pair[1] == Token::Symbol("(") && (unlisted || qualified) {
            return Err(SqlRejection::UnsafeFunction);
        }
    }
    let relations = relations(&tokens, allowed)?;
    if !tokens
        .windows(2)
        .any(|pair| pair[0].is_word("order") && pair[1].is_word("by"))
    {
        return Err(SqlRejection::Unordered);
    }
    let sql = tokens.iter().map(Token::text).collect::<Vec<_>>().join(" ");
    Ok(GatedQuery {
        digest: ResultId::sha256(sql.as_bytes()).to_string(),
        sql,
        relations,
        derivation,
    })
}

fn forbidden(word: &str) -> Option<SqlRejection> {
    [
        (EXTERNAL, SqlRejection::ExternalReference),
        (WRITE, SqlRejection::WriteOrDdl),
        (SESSION, SqlRejection::SessionMutation),
        (PROCEDURE, SqlRejection::ProcedureCall),
        (COMPOUND, SqlRejection::Compound),
    ]
    .into_iter()
    .find_map(|(words, rejection)| words.contains(&word).then_some(rejection))
}

/// Parses a dotted name chain at `start`; returns its parts and the next index.
fn chain(tokens: &[Token], start: usize) -> (Vec<&str>, usize) {
    let mut parts = Vec::new();
    let mut index = start;
    while let Some(part) = tokens.get(index).and_then(|token| {
        token
            .name()
            .or_else(|| (!parts.is_empty() && *token == Token::Symbol("*")).then_some("*"))
    }) {
        parts.push(part);
        index += 1;
        if tokens.get(index) != Some(&Token::Symbol(".")) {
            break;
        }
        index += 1;
    }
    (parts, index)
}

/// Checks name-chain depth and every `FROM`/`JOIN` relation against the allowlist.
fn relations(
    tokens: &[Token],
    allowed: &[RelationName],
) -> Result<Vec<RelationName>, SqlRejection> {
    let mut found = BTreeSet::new();
    let mut index = 0;
    while index < tokens.len() {
        if !(tokens[index].is_word("from") || tokens[index].is_word("join")) {
            let (parts, next) = chain(tokens, index);
            if parts.len() > 3 {
                return Err(SqlRejection::ExternalReference);
            }
            index = next.max(index + 1);
            continue;
        }
        loop {
            let (parts, next) = chain(tokens, index + 1);
            let relation = match parts.as_slice() {
                [] => return Err(SqlRejection::Malformed),
                [_] => return Err(SqlRejection::UnqualifiedRelation),
                [schema, relation] => RelationName::parse(&format!("{schema}.{relation}"))
                    .filter(|relation| allowed.contains(relation))
                    .ok_or(SqlRejection::RelationNotAllowed)?,
                _ => return Err(SqlRejection::ExternalReference),
            };
            found.insert(relation);
            index = next;
            if tokens.get(index).is_some_and(|token| token.is_word("as")) {
                index += 2;
            } else if tokens.get(index).and_then(Token::name).is_some() {
                index += 1;
            }
            if tokens.get(index) != Some(&Token::Symbol(",")) {
                break;
            }
        }
    }
    Ok(found.into_iter().collect())
}

fn tokenize(sql: &str) -> Result<Vec<Token>, SqlRejection> {
    if sql.contains('\\') {
        return Err(SqlRejection::UnsupportedCharacter);
    }
    let bytes = sql.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let start = index;
        let next = bytes.get(index + 1).copied();
        let token = match bytes[index] {
            b' ' | b'\t' | b'\n' | b'\r' => {
                index += 1;
                continue;
            }
            b'-' if next == Some(b'-') => return Err(SqlRejection::Comment),
            b'/' if next == Some(b'*') => return Err(SqlRejection::Comment),
            b';' => return Err(SqlRejection::MultiStatement),
            b'@' => return Err(SqlRejection::ExternalReference),
            b'\'' => {
                index += 1;
                loop {
                    match bytes.get(index) {
                        None => return Err(SqlRejection::UnterminatedLiteral),
                        Some(0) => return Err(SqlRejection::UnsupportedCharacter),
                        Some(b'\'') if bytes.get(index + 1) == Some(&b'\'') => index += 2,
                        Some(b'\'') => break,
                        Some(_) => index += 1,
                    }
                }
                index += 1;
                Token::Literal(sql[start..index].to_owned())
            }
            b'"' => {
                let length = sql[start + 1..]
                    .find('"')
                    .ok_or(SqlRejection::UnterminatedLiteral)?;
                let name = &sql[start + 1..start + 1 + length];
                if name.is_empty()
                    || name.len() > 63
                    || !name
                        .bytes()
                        .all(|byte| byte.is_ascii_graphic() || byte == b' ')
                {
                    return Err(SqlRejection::UnsupportedCharacter);
                }
                index += length + 2;
                Token::Quoted(name.to_owned())
            }
            b'?' => {
                index += 1;
                Token::Param("?".to_owned())
            }
            b'$' => {
                index += 1;
                while bytes.get(index).is_some_and(u8::is_ascii_digit) {
                    index += 1;
                }
                if index == start + 1 {
                    return Err(SqlRejection::UnsupportedCharacter);
                }
                Token::Param(sql[start..index].to_owned())
            }
            b'0'..=b'9' => {
                while bytes.get(index).is_some_and(u8::is_ascii_digit) {
                    index += 1;
                }
                if bytes.get(index) == Some(&b'.')
                    && bytes.get(index + 1).is_some_and(u8::is_ascii_digit)
                {
                    index += 1;
                    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
                        index += 1;
                    }
                }
                if bytes
                    .get(index)
                    .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
                {
                    return Err(SqlRejection::Malformed);
                }
                Token::Literal(sql[start..index].to_owned())
            }
            byte if byte.is_ascii_alphabetic() || byte == b'_' => {
                while bytes
                    .get(index)
                    .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
                {
                    index += 1;
                }
                Token::Word(sql[start..index].to_ascii_lowercase())
            }
            _ => {
                let symbol = [
                    "<=", ">=", "<>", "!=", "||", "(", ")", ",", ".", "*", "=", "<", ">", "+", "-",
                    "/", "%",
                ]
                .into_iter()
                .find(|symbol| sql[start..].starts_with(symbol))
                .ok_or(SqlRejection::UnsupportedCharacter)?;
                index += symbol.len();
                Token::Symbol(symbol)
            }
        };
        tokens.push(token);
        if tokens.len() > MAX_SQL_TOKENS {
            return Err(SqlRejection::TooLarge);
        }
    }
    Ok(tokens)
}

/// `[a-z_][a-z0-9_]{0,62}`.
fn valid_part(part: &str) -> bool {
    let bytes = part.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 63
        && (bytes[0].is_ascii_lowercase() || bytes[0] == b'_')
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    const CORPUS: &str = include_str!("../../../../fixtures/database/queries.tsv");

    fn allowed() -> Vec<RelationName> {
        ["main.customers", "main.order_totals", "main.orders"]
            .into_iter()
            .map(|name| RelationName::parse(name).unwrap())
            .collect()
    }

    fn expected(class: &str) -> &'static [SqlRejection] {
        match class {
            "write" => &[SqlRejection::WriteOrDdl],
            "multi-statement" => &[SqlRejection::MultiStatement],
            "session" => &[SqlRejection::SessionMutation],
            "procedure" => &[SqlRejection::ProcedureCall],
            "unsafe-function" => &[SqlRejection::UnsafeFunction],
            "external-link" => &[SqlRejection::ExternalReference],
            "compound" => &[SqlRejection::Compound],
            "secret-reflection" => &[
                SqlRejection::MultiStatement,
                SqlRejection::Comment,
                SqlRejection::UnsafeFunction,
            ],
            "policy" => &[
                SqlRejection::UnqualifiedRelation,
                SqlRejection::Unordered,
                SqlRejection::RelationNotAllowed,
            ],
            other => panic!("unknown corpus class {other}"),
        }
    }

    #[test]
    fn p8_database_sql_gate() {
        let allowed = allowed();
        let mut accepted = 0;
        let mut refused = 0;
        for line in CORPUS.lines() {
            let mut fields = line.splitn(3, '\t');
            let (class, _effect, sql) = (
                fields.next().unwrap(),
                fields.next().unwrap(),
                fields.next().unwrap(),
            );
            let result = gate(sql, &allowed);
            if class == "allowed" {
                let query = result.unwrap_or_else(|error| panic!("{error}: {sql}"));
                assert_eq!(query.derivation(), QueryDerivation::Query);
                assert!(query.digest().starts_with("sha256:"));
                accepted += 1;
                continue;
            }
            let error = result.expect_err(sql);
            assert!(expected(class).contains(&error), "{class} {error:?}: {sql}");
            // Secret reflection: a rejection never carries any of its input.
            let shown = format!("{error} {error:?}");
            assert!(!shown.contains("synthetic"), "{shown}");
            for word in sql.split_whitespace().filter(|word| word.len() > 7) {
                assert!(!shown.contains(word), "{shown} echoed {word}");
            }
            refused += 1;
        }
        assert_eq!((accepted, refused), (5, 40));

        // Whitespace and keyword case do not change the normalized digest.
        let canonical = gate("SELECT id, name FROM main.customers ORDER BY id", &allowed).unwrap();
        let spaced = gate(
            "select  ID ,\n\tname from MAIN . Customers order   by id",
            &allowed,
        )
        .unwrap();
        assert_eq!(
            canonical.sql(),
            "select id , name from main . customers order by id"
        );
        assert_eq!(canonical.digest(), spaced.digest());
        assert_eq!(
            canonical.relations(),
            &[RelationName::parse("main.customers").unwrap()]
        );
        assert_ne!(
            canonical.digest(),
            gate("SELECT id FROM main.customers ORDER BY id", &allowed)
                .unwrap()
                .digest()
        );
        // Quoted identifiers keep their case-sensitive form and must still match.
        assert!(gate(r#"SELECT "id" FROM "main"."orders" ORDER BY 1"#, &allowed).is_ok());
        assert_eq!(
            gate(r#"SELECT "id" FROM "main"."Orders" ORDER BY 1"#, &allowed),
            Err(SqlRejection::RelationNotAllowed)
        );
        assert_eq!(
            gate("SELECT a.b.c.d FROM main.orders ORDER BY 1", &allowed),
            Err(SqlRejection::ExternalReference)
        );
        assert_eq!(gate("", &allowed), Err(SqlRejection::Empty));
        assert_eq!(gate("   ", &allowed), Err(SqlRejection::Empty));
        assert_eq!(
            gate(
                &format!(
                    "SELECT id FROM main.orders WHERE status = '{}' ORDER BY id",
                    "x".repeat(MAX_SQL_BYTES)
                ),
                &allowed
            ),
            Err(SqlRejection::TooLarge)
        );
        assert_eq!(
            gate("SELECT 'open FROM main.orders ORDER BY 1", &allowed),
            Err(SqlRejection::UnterminatedLiteral)
        );
        assert_eq!(
            gate(r"SELECT E'\x27' FROM main.orders ORDER BY 1", &allowed),
            Err(SqlRejection::UnsupportedCharacter)
        );
        assert_eq!(
            gate("SELECT id::text FROM main.orders ORDER BY 1", &allowed),
            Err(SqlRejection::UnsupportedCharacter)
        );
        assert_eq!(
            gate(
                "SELECT id FROM main.orders WHERE id = $1 ORDER BY id",
                &allowed
            )
            .unwrap()
            .sql(),
            "select id from main . orders where id = $1 order by id"
        );
        assert_eq!(gate("VALUES (1)", &allowed), Err(SqlRejection::NotSelect));

        // Projections are generated by BRAN and pass the same gate.
        let orders = RelationName::parse("main.orders").unwrap();
        let projection = project(&orders, &["id", "status"], &allowed).unwrap();
        assert_eq!(projection.derivation(), QueryDerivation::Projection);
        assert_eq!(
            projection.sql(),
            r#"select "id" , "status" from "main" . "orders" order by 1 , 2"#
        );
        assert_eq!(projection.relations(), std::slice::from_ref(&orders));
        let hidden = RelationName::parse("main.sqlite_master").unwrap();
        assert_eq!(
            project(&hidden, &["name"], &allowed),
            Err(SqlRejection::RelationNotAllowed)
        );
        let invalid: [&[&str]; 4] = [&[], &["id; drop"], &["id", "id"], &["Id"]];
        for columns in invalid {
            assert_eq!(
                project(&orders, columns, &allowed),
                Err(SqlRejection::InvalidProjection),
                "{columns:?}"
            );
        }
        for name in [
            "orders",
            "main.orders.id",
            "Main.orders",
            "main.",
            "1x.orders",
        ] {
            assert_eq!(RelationName::parse(name), None, "{name}");
        }
    }
}
