// SPDX-License-Identifier: CC0-1.0

//! `LOAD DATA FROM S3`: the Amazon S3 import the MySQL engines serve, and the rows it loads.
//!
//! The statement is not one the embedded SQL engine has a grammar for — it is the engine
//! extension Amazon Aurora MySQL and Amazon RDS for MySQL add to `LOAD DATA`, which reads an
//! object out of Amazon S3 under an IAM role the resource is configured with — so this module
//! owns the three parts of it: what the statement names, whether the engine has the S3
//! capability the statement needs, and how the object's bytes become rows of the table.
//!
//! The rows really are loaded: the object is read out of Amazon S3's OWN store on this engine
//! and each record becomes an `INSERT` the engine runs, so what a later `SELECT` returns is
//! what that object holds. Nothing is answered from the shape of the statement's text.
//!
//! The capability is the resource's own state, never the statement's: the engine acts under
//! the IAM role the resource's parameter group names ([`IMPORT_ROLE_PARAMETERS`]) and that
//! the resource has ASSOCIATED, which is what "associate the IAM role with the DB cluster"
//! plus "set the `aurora_load_from_s3_role` or `aws_default_s3_role` parameter to the ARN of
//! that role" (Loading data into an Amazon Aurora MySQL DB cluster from text files in an
//! Amazon S3 bucket) asks of a cluster. With no such role the statement PARSES and fails
//! where the engine would reach S3 — the server's S3 API error — so a client can tell a
//! missing role association from a statement the engine does not have.

use sqlparser::tokenizer::Token;

use super::pg_copy::quoted;
use super::sql_engine::{EngineError, EngineErrorKind, InstanceDatabase};
use super::wire_session::{SessionContext, SessionFacts};
use super::EngineWire;

/// The object store this statement reads through: Amazon S3's own store, on the engine this
/// service is mounted on.
pub type ObjectStore =
    ::engine::ServiceState<::service_com_amazonaws_s3_amazons3::state::ComAmazonawsS3Amazons3State>;

/// The object store on `engine`, or `None` when no Amazon S3 service is mounted on it.
#[must_use]
pub fn object_store<Eng: Send + Sync + 'static>(engine: &Eng) -> Option<ObjectStore> {
    ::service_com_amazonaws_s3_amazons3::state::service_state(engine)
}

/// The engines whose `LOAD DATA` grammar carries the `FROM S3` source.
///
/// The Amazon S3 load is an AURORA MySQL and RDS for MySQL extension to the statement
/// ("Loading data into an Amazon Aurora MySQL DB cluster from text files in an Amazon S3
/// bucket"; "Loading data from an Amazon S3 bucket into an Amazon RDS for MySQL DB
/// instance"), so an engine that is not one of them has no such statement and the parse
/// error its own server answers is the right answer — MariaDB speaks the same wire and has
/// no `FROM S3`.
const S3_LOAD_ENGINES: [&str; 3] = ["aurora", "aurora-mysql", "mysql"];

/// The configuration parameters that name the IAM role the engine loads from Amazon S3
/// under, in the order the engine reads them: the parameter specific to the load, then the
/// one that serves every S3 integration of the resource.
///
/// Source: "set either the `aurora_load_from_s3_role` or `aws_default_s3_role` DB cluster
/// parameter to the Amazon Resource Name (ARN) of the new IAM role" — the same two
/// parameters an RDS for MySQL DB parameter group carries for the same purpose.
const IMPORT_ROLE_PARAMETERS: [&str; 2] = ["aurora_load_from_s3_role", "aws_default_s3_role"];

/// `ER_S3_API_ERROR` — the server error number Aurora MySQL reports a failure of its S3 API
/// call under, and its SQLSTATE class.
///
/// The condition is the ENGINE's call to Amazon S3 failing, which is what a client that has
/// just configured an import branches on: it is not a parse error and not a privilege error
/// of the session's own account.
const ER_S3_API_ERROR: u16 = 1871;
const ER_S3_API_SQLSTATE: &str = "HY000";

/// `ER_NO_DB_ERROR` — what MySQL answers a table reference that no database can be resolved
/// in: the statement named no database and the session is on none.
const ER_NO_DB_ERROR: u16 = 1046;

/// `ER_BAD_DB_ERROR` — what it answers a reference to a database the server has not got,
/// which is what a table name qualified by one carries.
const ER_BAD_DB_ERROR: u16 = 1049;

/// How many bytes of S3 objects one statement may load, so a client cannot make this
/// endpoint buffer without bound. The same order as the bulk load the other wire serves
/// ([`super::pg_copy::MAX_COPY_BYTES`]).
pub const MAX_LOAD_BYTES: usize = 8 * 1024 * 1024;

/// How many objects one `PREFIX` or `MANIFEST` load may read, for the same reason.
pub const MAX_LOAD_OBJECTS: usize = 256;

/// How many records one object may carry into the engine, so the insert work one statement
/// asks of the blocking pool is bounded by more than the object's byte size: a record can be
/// two bytes long, so the byte ceiling alone bounds the row count only very loosely.
pub const MAX_LOAD_ROWS: usize = 100_000;

/// Which objects the statement's URI names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum S3Source {
    /// `LOAD DATA FROM S3 [FILE] 's3://bucket/key'` — one object.
    File,
    /// `LOAD DATA FROM S3 PREFIX 's3://bucket/prefix'` — every object whose key starts with
    /// the prefix, in key order.
    Prefix,
    /// `LOAD DATA FROM S3 MANIFEST 's3://bucket/key'` — the objects the manifest's own
    /// entries name.
    Manifest,
}

/// What a duplicate key does to the row being loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Duplicates {
    /// The statement named neither `REPLACE` nor `IGNORE`: the row is refused and the load
    /// stops, which is what makes the engine's own constraint error reach the client.
    Refuse,
    /// `REPLACE`: the incoming row replaces the one already stored.
    Replace,
    /// `IGNORE`: the incoming row is skipped.
    Ignore,
}

/// The field format the statement declares, with MySQL's own defaults for the parts it
/// leaves out: tab-separated fields, nothing enclosing them, and the backslash escape.
#[derive(Debug, Clone)]
pub struct FieldFormat {
    pub terminated_by: String,
    pub enclosed_by: Option<char>,
    pub escaped_by: Option<char>,
}

impl Default for FieldFormat {
    fn default() -> Self {
        Self {
            terminated_by: "\t".to_string(),
            enclosed_by: None,
            escaped_by: Some('\\'),
        }
    }
}

/// The line format the statement declares, with MySQL's own defaults: nothing starting a
/// line and a newline ending it.
#[derive(Debug, Clone)]
pub struct LineFormat {
    pub starting_by: String,
    pub terminated_by: String,
}

impl Default for LineFormat {
    fn default() -> Self {
        Self {
            starting_by: String::new(),
            terminated_by: "\n".to_string(),
        }
    }
}

/// One `LOAD DATA FROM S3` statement, as the server read it.
#[derive(Debug, Clone)]
pub struct S3Load {
    pub source: S3Source,
    /// The `s3://bucket/key` URI the statement named, as the client wrote it.
    pub uri: String,
    /// The table the rows are loaded into.
    pub table: String,
    /// The DATABASE the statement's own name for the table put it in (`<database>.<table>`),
    /// or `None` for an unqualified name — which is the database the session is on, exactly
    /// as every other statement's names are resolved.
    pub database: Option<String>,
    /// The columns the statement named, in its own order; empty where it named none, which
    /// means every column of the table in its declared order.
    pub columns: Vec<String>,
    pub duplicates: Duplicates,
    pub fields: FieldFormat,
    pub lines: LineFormat,
    /// `IGNORE <n> LINES|ROWS`: how many records at the head of each object are skipped,
    /// which is how a file with a header row is loaded.
    pub ignore_rows: u64,
}

/// The `LOAD DATA FROM S3` statement `sql` is, or `None` for a statement that is not one —
/// including one on an engine that has no such statement, whose own parse error is then the
/// answer.
///
/// `Some(Err(..))` is a statement that IS one and that this server could not read: it is
/// refused with the parse error the MySQL server answers, rather than handed to an engine
/// that would refuse it for another reason.
pub fn s3_load_statement(facts: &SessionFacts, sql: &str) -> Option<Result<S3Load, EngineError>> {
    if facts.wire != EngineWire::MySql
        || !S3_LOAD_ENGINES.contains(&facts.engine.to_ascii_lowercase().as_str())
        // A statement past the length this endpoint serves is not read here either: the
        // session's own limit refuses it ([`super::wire_session::MAX_STATEMENT_BYTES`])
        // before anything is tokenized.
        || sql.len() > super::wire_session::MAX_STATEMENT_BYTES
    {
        return None;
    }
    let tokens = super::wire_session::significant_tokens(facts.wire, sql).ok()?;
    if !leads_with_s3_load(&tokens) {
        return None;
    }
    Some(parse(&tokens))
}

/// Whether the statement's leading keywords are `LOAD DATA FROM S3`.
fn leads_with_s3_load(tokens: &[Token]) -> bool {
    ["LOAD", "DATA", "FROM", "S3"]
        .iter()
        .enumerate()
        .all(|(at, keyword)| word_at(tokens, at).as_deref() == Some(*keyword))
}

/// The token at `at` as an UPPERCASED bare word, or `None` for a token that is not one.
fn word_at(tokens: &[Token], at: usize) -> Option<String> {
    match tokens.get(at) {
        Some(Token::Word(word)) if word.quote_style.is_none() => {
            Some(word.value.to_ascii_uppercase())
        }
        _ => None,
    }
}

/// The identifier at `at` — a bare or quoted word — as the client wrote it.
fn identifier_at(tokens: &[Token], at: usize) -> Option<String> {
    match tokens.get(at) {
        Some(Token::Word(word)) => Some(word.value.clone()),
        _ => None,
    }
}

/// The string literal at `at`, with the escape sequences MySQL defines inside one resolved.
fn string_at(tokens: &[Token], at: usize) -> Option<String> {
    match tokens.get(at) {
        Some(Token::SingleQuotedString(value) | Token::DoubleQuotedString(value)) => {
            Some(unescaped(value))
        }
        _ => None,
    }
}

/// Read the statement, or refuse it with the parse error the MySQL server answers for the
/// point it stopped reading at.
fn parse(tokens: &[Token]) -> Result<S3Load, EngineError> {
    let mut at = 4usize;
    let source = match word_at(tokens, at).as_deref() {
        Some("FILE") => {
            at += 1;
            S3Source::File
        }
        Some("PREFIX") => {
            at += 1;
            S3Source::Prefix
        }
        Some("MANIFEST") => {
            at += 1;
            S3Source::Manifest
        }
        _ => S3Source::File,
    };
    let Some(uri) = string_at(tokens, at) else {
        return Err(syntax_error(tokens, at));
    };
    at += 1;
    let duplicates = match word_at(tokens, at).as_deref() {
        Some("REPLACE") => {
            at += 1;
            Duplicates::Replace
        }
        Some("IGNORE") => {
            at += 1;
            Duplicates::Ignore
        }
        _ => Duplicates::Refuse,
    };
    if word_at(tokens, at).as_deref() != Some("INTO")
        || word_at(tokens, at + 1).as_deref() != Some("TABLE")
    {
        return Err(syntax_error(tokens, at));
    }
    at += 2;
    let Some(mut table) = identifier_at(tokens, at) else {
        return Err(syntax_error(tokens, at));
    };
    at += 1;
    // A table name qualified by the database it is in: the qualifier is what SELECTS the
    // database the load runs against, and the name after it is the table inside it — the
    // same reading every other statement's qualified names get.
    let mut database = None;
    if matches!(tokens.get(at), Some(Token::Period)) {
        let Some(qualified) = identifier_at(tokens, at + 1) else {
            return Err(syntax_error(tokens, at + 1));
        };
        database = Some(table);
        table = qualified;
        at += 2;
    }
    let mut load = S3Load {
        source,
        uri,
        table,
        database,
        columns: Vec::new(),
        duplicates,
        fields: FieldFormat::default(),
        lines: LineFormat::default(),
        ignore_rows: 0,
    };
    // The clauses, in any order the client wrote them: each one is read whole, and the
    // first token that starts none of them is where the statement stops being readable.
    while at < tokens.len() {
        match word_at(tokens, at).as_deref() {
            // `CHARACTER SET <name>`: which character set the file's bytes are read as. The
            // engine holds text as UTF-8 and the wire reports `utf8mb4`, so the clause is
            // read and the name it carries changes nothing.
            Some("CHARACTER") if word_at(tokens, at + 1).as_deref() == Some("SET") => {
                if identifier_at(tokens, at + 2).is_none() {
                    return Err(syntax_error(tokens, at + 2));
                }
                at += 3;
            }
            Some("FIELDS" | "COLUMNS") => {
                at = fields_clause(tokens, at + 1, &mut load.fields)?;
            }
            Some("LINES") => {
                at = lines_clause(tokens, at + 1, &mut load.lines)?;
            }
            Some("IGNORE") => {
                let Some(Token::Number(count, _)) = tokens.get(at + 1) else {
                    return Err(syntax_error(tokens, at + 1));
                };
                let Ok(count) = count.parse::<u64>() else {
                    return Err(syntax_error(tokens, at + 1));
                };
                if !matches!(word_at(tokens, at + 2).as_deref(), Some("LINES" | "ROWS")) {
                    return Err(syntax_error(tokens, at + 2));
                }
                load.ignore_rows = count;
                at += 3;
            }
            _ if matches!(tokens.get(at), Some(Token::LParen)) => {
                at = column_list(tokens, at + 1, &mut load.columns)?;
            }
            _ => return Err(syntax_error(tokens, at)),
        }
    }
    Ok(load)
}

/// The `FIELDS`/`COLUMNS` clause's own parts, and the index after it.
fn fields_clause(
    tokens: &[Token],
    from: usize,
    fields: &mut FieldFormat,
) -> Result<usize, EngineError> {
    let mut at = from;
    let mut read_one = false;
    loop {
        let optionally = word_at(tokens, at).as_deref() == Some("OPTIONALLY");
        let head = word_at(tokens, at + usize::from(optionally));
        match head.as_deref() {
            Some("TERMINATED") if !optionally => {
                let Some(value) = terminator(tokens, at + 1) else {
                    return Err(syntax_error(tokens, at + 1));
                };
                fields.terminated_by = value;
                at += 3;
            }
            Some("ENCLOSED") => {
                let base = at + usize::from(optionally);
                let Some(value) = terminator(tokens, base + 1) else {
                    return Err(syntax_error(tokens, base + 1));
                };
                fields.enclosed_by = one_character(&value, "FIELDS ENCLOSED BY")?;
                at = base + 3;
            }
            Some("ESCAPED") if !optionally => {
                let Some(value) = terminator(tokens, at + 1) else {
                    return Err(syntax_error(tokens, at + 1));
                };
                fields.escaped_by = one_character(&value, "FIELDS ESCAPED BY")?;
                at += 3;
            }
            _ => {
                if !read_one {
                    return Err(syntax_error(tokens, at));
                }
                return Ok(at);
            }
        }
        read_one = true;
    }
}

/// The ONE character an `ENCLOSED BY`/`ESCAPED BY` literal declares, `None` for the empty
/// literal — which is how MySQL says "no enclosing character" and "no escaping".
///
/// A literal of any other length is refused rather than cut down to its first character: the
/// reader would then split fields by something the statement never named and the load would
/// report rows the client did not ask for, which is what every other format value this
/// endpoint cannot honour is refused for.
fn one_character(value: &str, clause: &str) -> Result<Option<char>, EngineError> {
    match value.chars().count() {
        0 => Ok(None),
        1 if value.len() == 1 => Ok(value.chars().next()),
        _ => Err(unreadable_format(clause)),
    }
}

/// The `LINES` clause's own parts, and the index after it.
fn lines_clause(tokens: &[Token], from: usize, lines: &mut LineFormat) -> Result<usize, EngineError> {
    let mut at = from;
    let mut read_one = false;
    loop {
        match word_at(tokens, at).as_deref() {
            Some("STARTING") => {
                let Some(value) = terminator(tokens, at + 1) else {
                    return Err(syntax_error(tokens, at + 1));
                };
                lines.starting_by = value;
                at += 3;
            }
            Some("TERMINATED") => {
                let Some(value) = terminator(tokens, at + 1) else {
                    return Err(syntax_error(tokens, at + 1));
                };
                lines.terminated_by = value;
                at += 3;
            }
            _ => {
                if !read_one {
                    return Err(syntax_error(tokens, at));
                }
                return Ok(at);
            }
        }
        read_one = true;
    }
}

/// The `BY '<string>'` a `TERMINATED`/`ENCLOSED`/`ESCAPED`/`STARTING` carries, at `at`.
fn terminator(tokens: &[Token], at: usize) -> Option<String> {
    (word_at(tokens, at).as_deref() == Some("BY"))
        .then(|| string_at(tokens, at + 1))
        .flatten()
}

/// The parenthesised column list, and the index after it.
fn column_list(
    tokens: &[Token],
    from: usize,
    columns: &mut Vec<String>,
) -> Result<usize, EngineError> {
    let mut at = from;
    loop {
        let Some(name) = identifier_at(tokens, at) else {
            return Err(syntax_error(tokens, at));
        };
        columns.push(name);
        at += 1;
        match tokens.get(at) {
            Some(Token::Comma) => at += 1,
            Some(Token::RParen) => return Ok(at + 1),
            _ => return Err(syntax_error(tokens, at)),
        }
    }
}

/// `ER_PARSE_ERROR` for a statement this server stopped reading at `at`: the MySQL server's
/// own sentence, quoting the statement from there.
fn syntax_error(tokens: &[Token], at: usize) -> EngineError {
    let rest: String = tokens
        .iter()
        .skip(at)
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(super::server_statements::MYSQL_PARSE_ERROR_QUOTE_CHARS)
        .collect();
    EngineError::new(
        EngineErrorKind::Syntax,
        super::server_statements::mysql_parse_error(&rest, 1),
    )
}

/// One string literal with the escape sequences MySQL resolves inside one applied.
///
/// Source: the escape sequences the MySQL reference manual's "String Literals" section
/// lists; a backslash before anything else "is taken to represent itself", which is also
/// what a tokenizer that has already resolved them leaves behind.
fn unescaped(value: &str) -> String {
    unescaped_by(value, '\\')
}

/// The same resolution for a field of a loaded object, whose escape character the statement
/// declares (`FIELDS ESCAPED BY`) and is the backslash only by default — so a load that
/// names another one has its data read by THAT character rather than by this module's.
fn unescaped_by(value: &str, escape: char) -> String {
    if !value.contains(escape) {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len());
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character != escape {
            out.push(character);
            continue;
        }
        match characters.next() {
            Some('0') => out.push('\0'),
            Some('b') => out.push('\u{8}'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('Z') => out.push('\u{1a}'),
            Some(other) => out.push(other),
            None => out.push(escape),
        }
    }
    out
}

/// The IAM role the resource's engine loads from Amazon S3 under: the role one of
/// [`IMPORT_ROLE_PARAMETERS`] names AND that the resource has associated.
///
/// Both halves are required, because each is a different grant: the parameter is what tells
/// the engine which role to act as, and the association is what lets Amazon RDS assume it on
/// the resource's behalf. A parameter naming a role the resource has not associated — and an
/// association no parameter names — leaves the engine with no credentials to call S3 with,
/// which is the condition the statement then fails on.
#[must_use]
pub fn import_role(facts: &SessionFacts, associated: &[String]) -> Option<String> {
    IMPORT_ROLE_PARAMETERS
        .iter()
        .filter_map(|parameter| facts.runtime.parameter(parameter))
        .map(str::trim)
        .find(|named| !named.is_empty() && associated.iter().any(|held| held == named))
        .map(str::to_string)
}

/// The objects `load` names, read out of Amazon S3's own store under the resource's import
/// role.
///
/// Every refusal here is the server's S3 API error ([`ER_S3_API_ERROR`]), because each one is
/// the engine's own call to Amazon S3 not happening or not answering with an object: no role
/// to call it under, and nothing found under the URI.
///
/// WHAT the role may read is deliberately not modelled: the role is resolved and the object
/// is read, and the per-bucket and per-key authorization real Amazon RDS performs as that
/// role — the access-denied half of the integration — belongs with the other role-permission
/// causes of this surface, which the open `rds__s3` integration issue owns. So the check
/// above is the resource's CONFIGURATION, not an authorization decision, and a reader must
/// not take it for one.
pub fn objects_to_load(
    context: &SessionContext,
    facts: &SessionFacts,
    load: &S3Load,
) -> Result<Vec<(String, Vec<u8>)>, EngineError> {
    if import_role(facts, &context.associated_role_arns()).is_none() {
        return Err(missing_credentials());
    }
    // A URI that names no bucket and key is one the engine's S3 call resolves to no object,
    // which is the same condition as a key the bucket has not got.
    let Some((bucket, key)) = split_s3_uri(&load.uri) else {
        return Err(not_found());
    };
    let Some(store) = &context.objects else {
        // Nothing was read and nothing is claimed: this emulator has no object store to
        // read through, which is a failure of the emulator rather than of the statement.
        return Err(EngineError::new(
            EngineErrorKind::Internal,
            "the Amazon S3 service this endpoint reads objects through is not reachable from \
             this emulator",
        ));
    };
    let objects = {
        let store = store.lock();
        // A bucket this store does not hold answers the engine's read the same way a missing
        // key does, so it is the same condition.
        let Some(held) = store.bucket(&bucket) else {
            return Err(not_found());
        };
        match load.source {
            S3Source::File | S3Source::Manifest => match held.object(&key) {
                Some(object) => vec![(key.clone(), object.body.clone())],
                None => Vec::new(),
            },
            // Taken one object and one byte at a time, so a prefix naming more than this
            // endpoint loads is REFUSED rather than copied in whole and refused afterwards.
            S3Source::Prefix => {
                let mut taken: Vec<(String, Vec<u8>)> = Vec::new();
                let mut bytes = 0usize;
                for (held_key, object) in held
                    .objects()
                    .filter(|(held_key, _)| held_key.starts_with(key.as_str()))
                {
                    bytes += object.body.len();
                    if bytes > MAX_LOAD_BYTES {
                        return Err(too_large());
                    }
                    if taken.len() == MAX_LOAD_OBJECTS {
                        return Err(too_many());
                    }
                    taken.push((held_key.to_string(), object.body.clone()));
                }
                taken
            }
        }
    };
    if objects.is_empty() {
        return Err(not_found());
    }
    // A MANIFEST names the objects to load rather than carrying their rows, so the manifest
    // object is read and then the objects its entries name are.
    let objects = match load.source {
        S3Source::Manifest => manifest_objects(store, &objects)?,
        S3Source::File | S3Source::Prefix => objects,
    };
    let bytes: usize = objects.iter().map(|(_, body)| body.len()).sum();
    if bytes > MAX_LOAD_BYTES {
        return Err(too_large());
    }
    if objects.len() > MAX_LOAD_OBJECTS {
        return Err(too_many());
    }
    Ok(objects)
}

/// What a load naming more bytes than this endpoint reads is refused with.
fn too_large() -> EngineError {
    EngineError::new(
        EngineErrorKind::LimitExceeded,
        format!("the objects named are larger than this endpoint loads ({MAX_LOAD_BYTES} bytes)"),
    )
}

/// What a load naming more objects than this endpoint reads is refused with.
fn too_many() -> EngineError {
    EngineError::new(
        EngineErrorKind::LimitExceeded,
        format!(
            "the URI names more objects than this endpoint loads in one statement \
             ({MAX_LOAD_OBJECTS})"
        ),
    )
}

/// The objects a manifest's entries name, read through the same store.
///
/// The manifest is the JSON document Aurora documents for the form: an `entries` array whose
/// members each carry the `url` of one object. An entry marked `"mandatory": true` whose
/// object is missing fails the load, which is what that member means ("the load fails if the
/// file is not found"); a missing non-mandatory one is skipped.
///
/// Bounded the way the prefix read is, and for the same reason: the entries are a
/// CALLER-SUPPLIED list that need name no distinct object, so each body is counted before it
/// is copied and the read stops at the first byte past [`MAX_LOAD_BYTES`] — a manifest
/// repeating one large object cannot make this endpoint hold a multiple of it.
fn manifest_objects(
    store: &ObjectStore,
    manifest: &[(String, Vec<u8>)],
) -> Result<Vec<(String, Vec<u8>)>, EngineError> {
    let body: &[u8] = manifest.first().map_or(&[], |(_, body)| body.as_slice());
    // The manifest itself is an object this endpoint read, so it counts against the same
    // ceiling before it is parsed rather than after.
    if body.len() > MAX_LOAD_BYTES {
        return Err(too_large());
    }
    let document: ::serde_json::Value = ::serde_json::from_slice(body).map_err(|error| {
        s3_error(format!(
            "S3 API returned error: the manifest is not the document this form takes: {error}"
        ))
    })?;
    let entries = document
        .get("entries")
        .and_then(::serde_json::Value::as_array)
        .ok_or_else(|| {
            s3_error(
                "S3 API returned error: the manifest is not the document this form takes: it \
                 carries no entries",
            )
        })?;
    if entries.len() > MAX_LOAD_OBJECTS {
        return Err(too_many());
    }
    let mut loaded = Vec::new();
    let mut bytes = 0usize;
    let store = store.lock();
    for entry in entries {
        let url = entry
            .get("url")
            .and_then(::serde_json::Value::as_str)
            .unwrap_or_default();
        let mandatory = entry
            .get("mandatory")
            .and_then(::serde_json::Value::as_bool)
            .unwrap_or(false);
        let Some((bucket, key)) = split_s3_uri(url) else {
            if mandatory {
                return Err(not_found());
            }
            continue;
        };
        let held = store.bucket(&bucket).and_then(|held| held.object(&key));
        let Some(object) = held else {
            if mandatory {
                return Err(not_found());
            }
            continue;
        };
        // Counted first, copied second: the entry's bytes are what the ceiling is about.
        bytes += object.body.len();
        if bytes > MAX_LOAD_BYTES {
            return Err(too_large());
        }
        if loaded.len() == MAX_LOAD_OBJECTS {
            return Err(too_many());
        }
        loaded.push((key, object.body.clone()));
    }
    Ok(loaded)
}

/// The bucket and key an `s3://bucket/key` URI names.
#[must_use]
pub fn split_s3_uri(uri: &str) -> Option<(String, String)> {
    let rest = uri
        .strip_prefix("s3://")
        .or_else(|| uri.strip_prefix("S3://"))?;
    let (bucket, key) = rest.split_once('/')?;
    (!bucket.is_empty() && !key.is_empty()).then(|| (bucket.to_string(), key.to_string()))
}

/// What the engine answers when it has no credentials to call Amazon S3 with: the role the
/// resource is configured with is missing, so the client's remedy is the association and the
/// parameter rather than the statement.
///
/// The sentence is the server's own for that condition — `Missing Credentials: Cannot
/// instantiate S3 Client`, the reason the engine reports when it has no role to assume — and
/// carries no variable part.
#[must_use]
pub fn missing_credentials() -> EngineError {
    s3_error("S3 API returned error: Missing Credentials: Cannot instantiate S3 Client")
}

/// What the engine answers when its Amazon S3 call resolves no object: `Not Found`, the
/// status S3 answers a read of a key or a bucket it does not hold with, reported as the
/// engine reports an S3 API error's status and reason.
///
/// One sentence for every such URI — a key the bucket has not got, a bucket that does not
/// exist, a prefix nothing lies under, a manifest entry marked mandatory whose object is
/// gone — because the engine learns the same thing about each of them: S3 answered its read
/// with no object.
#[must_use]
pub fn not_found() -> EngineError {
    s3_error("S3 API returned error: Not Found:Not Found")
}

/// One failure of the engine's own Amazon S3 call, under the server error number that
/// condition carries.
fn s3_error(message: impl Into<String>) -> EngineError {
    EngineError::new(EngineErrorKind::InsufficientPrivilege, message)
        .with_mysql_error(ER_S3_API_ERROR, ER_S3_API_SQLSTATE)
}

/// Load the records `objects` carry into the statement's table, and answer with how many
/// rows were loaded.
///
/// Every row is loaded inside ONE transaction, so a row the table refuses leaves none of the
/// load behind — and a load the client sent inside a transaction of its own stays that
/// client's to commit or roll back, exactly as the other wire's bulk load does
/// ([`super::pg_copy::load`]).
///
/// Blocking: the engine runs each insert on the calling thread, so a wire handler calls this
/// on a blocking task.
pub fn load(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    load: &S3Load,
    objects: &[(String, Vec<u8>)],
) -> Result<u64, EngineError> {
    // WHICH database the rows are loaded into: the one the statement's own name for the
    // table put it in, else the one the session is on.
    let target = target_database(database, facts, load)?;
    // The columns the rows are loaded into: the ENGINE reports them for the table, so a
    // table the session has not got, and a column the statement names that the table does
    // not have, are refused by it rather than by a list kept here.
    let declared = database.describe(
        facts.connection_id,
        &target,
        &format!("SELECT * FROM {}", quoted(&load.table)),
        &[],
    )?;
    let columns: Vec<String> = if load.columns.is_empty() {
        declared.iter().map(|column| column.name.clone()).collect()
    } else {
        load.columns.clone()
    };
    if columns.is_empty() {
        return Err(EngineError::new(
            EngineErrorKind::UndefinedObject,
            format!("Table '{}' has no columns to load into", load.table),
        ));
    }
    let statement = format!(
        "INSERT {}INTO {} ({}) VALUES ({})",
        match load.duplicates {
            Duplicates::Refuse => "",
            Duplicates::Replace => "OR REPLACE ",
            Duplicates::Ignore => "OR IGNORE ",
        },
        quoted(&load.table),
        columns
            .iter()
            .map(|name| quoted(name))
            .collect::<Vec<_>>()
            .join(", "),
        std::iter::repeat_n("?", columns.len())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let inside_the_clients_transaction = database.in_transaction(facts.connection_id);
    if !inside_the_clients_transaction {
        // The load's own transaction is one of this SESSION's, not the engine connection's,
        // so the rows another session writes while it runs are no part of it.
        database.begin_transaction(facts.connection_id);
    }
    let outcome = (|| -> Result<u64, EngineError> {
        let mut loaded = 0u64;
        for (_, body) in objects {
            for record in records(load, body)? {
                let values = row_values(load, &record, columns.len());
                // Loaded FOR the session, so a row this load wrote is one the client's own
                // transaction can roll back, exactly as the other wire's bulk load does.
                database.execute_for(
                    facts.connection_id,
                    &target,
                    &super::wire_session::server_view(facts),
                    &statement,
                    &values,
                )?;
                loaded += 1;
            }
        }
        Ok(loaded)
    })();
    if inside_the_clients_transaction {
        return outcome;
    }
    match outcome {
        Ok(loaded) => {
            // The load's own transaction, which no client opened: it is the statement's and
            // it is committed here, so there is no isolation level of the client's to hold
            // it to.
            database.commit_transaction(facts.connection_id, false)?;
            Ok(loaded)
        }
        Err(error) => {
            let _ = database.rollback_transaction(facts.connection_id);
            Err(error)
        }
    }
}

/// The database the load runs against: the qualifier the statement's table name carries,
/// resolved against the databases the resource has, else the database the session is on.
///
/// A qualifier naming a database the resource has not got is refused the way MySQL refuses
/// any reference to one (`ER_BAD_DB_ERROR`), rather than handed to the engine as a database
/// name it has never heard of. A statement that names neither — an unqualified table on a
/// session that is on no database — is the other of MySQL's two conditions
/// (`ER_NO_DB_ERROR`), because there is then no database for the name to be resolved in.
fn target_database(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    load: &S3Load,
) -> Result<String, EngineError> {
    let target = match &load.database {
        Some(named) => database
            .database_names()
            .into_iter()
            .find(|known| known.eq_ignore_ascii_case(named))
            .ok_or_else(|| {
                EngineError::new(
                    EngineErrorKind::UndefinedDatabase,
                    format!("Unknown database '{named}'"),
                )
                .with_mysql_error(ER_BAD_DB_ERROR, "42000")
            })?,
        None => facts.database.clone(),
    };
    if target.is_empty() {
        return Err(
            EngineError::new(EngineErrorKind::UndefinedDatabase, "No database selected")
                .with_mysql_error(ER_NO_DB_ERROR, "3D000"),
        );
    }
    Ok(target)
}

/// The records one object carries, read in the format the statement declares.
///
/// The object's bytes are read by the `csv` crate's reader, configured from the statement's
/// own `FIELDS`/`LINES` clauses: the delimiter, the enclosing character and the line
/// terminator are what that reader is for, and the escape sequences are resolved per field
/// afterwards ([`unescaped`]) because MySQL resolves them in an enclosed field and a bare one
/// alike.
fn records(load: &S3Load, body: &[u8]) -> Result<Vec<Vec<String>>, EngineError> {
    if !load.lines.starting_by.is_empty() {
        return Err(unreadable_format("LINES STARTING BY"));
    }
    let delimiter = single_byte(&load.fields.terminated_by)
        .ok_or_else(|| unreadable_format("FIELDS TERMINATED BY"))?;
    let terminator = match load.lines.terminated_by.as_str() {
        "\r\n" => ::csv::Terminator::CRLF,
        other => ::csv::Terminator::Any(
            single_byte(other).ok_or_else(|| unreadable_format("LINES TERMINATED BY"))?,
        ),
    };
    let mut builder = ::csv::ReaderBuilder::new();
    builder
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .terminator(terminator)
        // The escapes are resolved per field below, so the reader leaves them alone.
        .escape(None);
    match load.fields.enclosed_by {
        Some(enclosure) => {
            // One byte, because that is what the reader quotes by; the parse refused any
            // other declared enclosure ([`one_character`]), so this cannot silently become
            // a character the statement did not name.
            let Some(quote) = single_byte(&enclosure.to_string()) else {
                return Err(unreadable_format("FIELDS ENCLOSED BY"));
            };
            builder.quoting(true);
            builder.quote(quote);
        }
        None => {
            builder.quoting(false);
        }
    }
    let mut reader = builder.from_reader(body);
    let mut records = Vec::new();
    for (index, record) in reader.records().enumerate() {
        let record = record.map_err(|error| {
            EngineError::new(
                EngineErrorKind::DataError,
                format!("Row {} was not read: {error}", index + 1),
            )
        })?;
        if u64::try_from(index).unwrap_or(u64::MAX) < load.ignore_rows {
            continue;
        }
        // The empty record a trailing line terminator leaves is no row.
        if record.iter().all(str::is_empty) && record.len() <= 1 {
            continue;
        }
        records.push(record.iter().map(str::to_string).collect());
        if records.len() > MAX_LOAD_ROWS {
            return Err(EngineError::new(
                EngineErrorKind::LimitExceeded,
                format!(
                    "the object carries more records than this endpoint loads from one object \
                     ({MAX_LOAD_ROWS})"
                ),
            ));
        }
    }
    Ok(records)
}

/// The values one record is loaded as: a field the record does not carry is NULL, a field
/// past the columns named is dropped, and the escape the statement declares is resolved — so
/// `\N`, the NULL marker, is the one field that is not text.
fn row_values(load: &S3Load, record: &[String], columns: usize) -> Vec<Option<String>> {
    (0..columns)
        .map(|at| {
            let field = record.get(at)?;
            let null_marker = load
                .fields
                .escaped_by
                .map(|escape| format!("{escape}N"))
                .unwrap_or_default();
            if !null_marker.is_empty() && field == &null_marker {
                return None;
            }
            Some(match load.fields.escaped_by {
                Some(escape) => unescaped_by(field, escape),
                None => field.clone(),
            })
        })
        .collect()
}

/// `b` as the one byte a delimiter or terminator has to be, or `None` for a string of
/// another length.
fn single_byte(value: &str) -> Option<u8> {
    match value.as_bytes() {
        [byte] => Some(*byte),
        _ => None,
    }
}

/// What a clause this server cannot READ the object by is refused with: the engine does not
/// serve that form of the load, which is its own condition rather than a parse error.
fn unreadable_format(clause: &str) -> EngineError {
    EngineError::new(
        EngineErrorKind::FeatureNotSupported,
        format!("The {clause} clause of this LOAD DATA FROM S3 is not served by this endpoint"),
    )
}
