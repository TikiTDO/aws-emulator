// SPDX-License-Identifier: CC0-1.0

//! `COPY <table> FROM STDIN`: the PostgreSQL bulk-load exchange, and the rows it loads.
//!
//! The statement is not a query the engine runs on its own — it puts the SESSION into the
//! protocol's copy-in mode, where the client streams the rows and the server loads them —
//! so this module owns the three parts of it: which table and columns the statement names,
//! how the text format the client streams is read, and what each row is refused with when
//! it does not fit the table.
//!
//! The rows really are loaded: each one becomes an `INSERT` the engine runs, so what a
//! later `SELECT` returns is what the client streamed. A row the table cannot take is
//! refused with PostgreSQL's own DATA error for that case — the load is a data operation,
//! and a syntax error would be the wrong class for a malformed row — and the rows before it
//! are rolled back with it, because `COPY` "stops operation at the first error".

use super::sql_engine::{EngineError, EngineErrorKind, InstanceDatabase};
use super::wire_session::SessionFacts;
use super::EngineWire;
use sqlparser::tokenizer::Token;

/// How many bytes of copy data one `COPY FROM STDIN` may stream, so a client cannot make
/// this endpoint buffer without bound. Well above a bulk load a test or a tool sends, and
/// the same order as the statement limit the session already carries.
pub const MAX_COPY_BYTES: usize = 8 * 1024 * 1024;

/// What `COPY <table> [(<columns>)] FROM STDIN` names.
#[derive(Debug, Clone)]
pub struct CopyTarget {
    /// The table the rows are loaded into.
    pub table: String,
    /// The columns the statement named, in its own order; empty where it named none, which
    /// means every column of the table in its declared order.
    pub columns: Vec<String>,
}

/// The refusal a `COPY` that names a FILE (or a program) gets, or `None` for a statement
/// that names none.
///
/// Amazon RDS gives no client access to the host the engine runs on, so the server-side
/// forms of `COPY` — `COPY … TO '<file>'`, `COPY … FROM '<file>'` and the `PROGRAM` forms —
/// are not served on an RDS instance at all. RDS answers them with its own sentence rather
/// than PostgreSQL's privilege refusal (recorded behavior: `COPY <table> TO '<path>'` on an
/// RDS for PostgreSQL instance answers SQLSTATE `0A000` with "COPY to a file is not
/// supported"), which is the capability refusal it is — and what a client is told instead of
/// this emulator reading or writing a path on its own host. The client-side `COPY … FROM
/// STDIN`, which streams the rows over the connection, is what this endpoint serves
/// ([`copy_in_target`]).
pub fn refused_file_copy(facts: &SessionFacts, tokens: &[Token]) -> Option<EngineError> {
    if facts.wire != EngineWire::Postgres {
        return None;
    }
    let leads_with_copy = matches!(tokens.first(), Some(Token::Word(word)) if word.value.eq_ignore_ascii_case("COPY"));
    if !leads_with_copy {
        return None;
    }
    for (at, token) in tokens.iter().enumerate() {
        let direction = match token {
            Token::Word(word) if word.value.eq_ignore_ascii_case("TO") => "to",
            Token::Word(word) if word.value.eq_ignore_ascii_case("FROM") => "from",
            _ => continue,
        };
        let names_a_path = match tokens.get(at + 1) {
            // A quoted path, and the `PROGRAM` form, which runs a command on the host.
            Some(Token::SingleQuotedString(_)) => true,
            Some(Token::Word(word)) => word.value.eq_ignore_ascii_case("PROGRAM"),
            _ => false,
        };
        if names_a_path {
            return Some(
                EngineError::new(
                    EngineErrorKind::FeatureNotSupported,
                    format!("COPY {direction} a file is not supported"),
                )
                .with_sqlstate("0A000"),
            );
        }
    }
    None
}

/// The copy-in target a statement names, or `None` for a statement that is not a `COPY …
/// FROM STDIN` at all (including `COPY … TO`, which streams the other way).
///
/// Only the PostgreSQL wire has this exchange; the MySQL protocol's bulk load is a
/// different command, which this endpoint does not serve.
pub fn copy_in_target(facts: &SessionFacts, tokens: &[Token]) -> Option<CopyTarget> {
    if facts.wire != EngineWire::Postgres {
        return None;
    }
    let words: Vec<String> = tokens
        .iter()
        .map(|token| match token {
            Token::Word(word) => word.value.to_ascii_uppercase(),
            other => other.to_string(),
        })
        .collect();
    if words.first().map(String::as_str) != Some("COPY") {
        return None;
    }
    // `FROM STDIN` is what makes it a copy IN; `COPY … TO …` is the other direction.
    let from = words.iter().position(|word| word == "FROM")?;
    if words.get(from + 1).map(String::as_str) != Some("STDIN") {
        return None;
    }
    let table = match tokens.get(1) {
        Some(Token::Word(word)) => word.value.clone(),
        _ => return None,
    };
    // The column list, when the statement carries one: the identifiers between the
    // parentheses that follow the table name.
    let mut columns = Vec::new();
    if matches!(tokens.get(2), Some(Token::LParen)) {
        for token in tokens.iter().skip(3) {
            match token {
                Token::RParen => break,
                Token::Word(word) => columns.push(word.value.clone()),
                _ => {}
            }
        }
    }
    Some(CopyTarget { table, columns })
}

/// Load the rows `data` carries into `target`, and answer with how many were loaded.
///
/// `data` is the whole of what the client streamed, in the format `COPY` documents as its
/// default: one row per line, columns separated by tabs, `\N` for a NULL, the backslash
/// escapes PostgreSQL lists, and an optional `\.` line marking the end of the data.
///
/// Every row is loaded inside ONE transaction, so a row the table refuses leaves none of
/// the load behind: `COPY` "stops operation at the first error", and the rows it had
/// already read are rolled back with it.
pub fn load(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    target: &CopyTarget,
    data: &[u8],
) -> Result<u64, EngineError> {
    if data.len() > MAX_COPY_BYTES {
        return Err(EngineError::new(
            EngineErrorKind::LimitExceeded,
            format!("the copy data is larger than this endpoint serves ({MAX_COPY_BYTES} bytes)"),
        ));
    }
    // The columns the rows are loaded into, and the type each one is declared with: the
    // ENGINE reports them for the table, so a column the statement names that the table
    // does not have is refused by it rather than by a list kept here.
    let declared = database.describe(
        facts.connection_id,
        &facts.database,
        &format!("SELECT * FROM {}", quoted(&target.table)),
        &[],
    )?;
    let columns: Vec<(String, Option<String>)> = if target.columns.is_empty() {
        declared
            .iter()
            .map(|column| (column.name.clone(), column.declared_type.clone()))
            .collect()
    } else {
        target
            .columns
            .iter()
            .map(|named| {
                let declared_type = declared
                    .iter()
                    .find(|column| column.name.eq_ignore_ascii_case(named))
                    .and_then(|column| column.declared_type.clone());
                (named.clone(), declared_type)
            })
            .collect()
    };
    let statement = format!(
        "INSERT INTO {} ({}) VALUES ({})",
        quoted(&target.table),
        columns
            .iter()
            .map(|(name, _)| quoted(name))
            .collect::<Vec<_>>()
            .join(", "),
        std::iter::repeat_n("?", columns.len())
            .collect::<Vec<_>>()
            .join(", ")
    );

    let text = String::from_utf8_lossy(data);
    let mut loaded = 0u64;
    // A `COPY` the client sent INSIDE a transaction of its own loads into that transaction:
    // the rows are the client's to commit or roll back, so each row is loaded for the
    // SESSION and recorded against the transaction it has open. A `COPY` outside one is its
    // OWN transaction — one of this session's, not the engine connection's, so the rows of
    // another session running at the same time are no part of it — which is what makes "COPY
    // stops operation at the first error" leave no rows behind.
    let inside_the_clients_transaction = database.in_transaction(facts.connection_id);
    if !inside_the_clients_transaction {
        database.begin_transaction(facts.connection_id);
    }
    let outcome = (|| -> Result<u64, EngineError> {
        for line in text.split('\n') {
            let line = line.strip_suffix('\r').unwrap_or(line);
            // The end-of-data marker, and the empty tail a trailing newline leaves.
            if line == "\\." {
                break;
            }
            if line.is_empty() {
                continue;
            }
            let values = row_values(line, &columns)?;
            database.execute_for(
                facts.connection_id,
                &facts.database,
                &super::wire_session::server_view(facts),
                &statement,
                &values,
            )?;
            loaded += 1;
        }
        Ok(loaded)
    })();
    if inside_the_clients_transaction {
        // Whatever the outcome, the transaction stays the client's: a failed `COPY` leaves
        // it to be rolled back, exactly as any other failed statement inside one does.
        return outcome;
    }
    match outcome {
        Ok(loaded) => {
            // The copy's own transaction, which no client opened: it is the statement's and
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

/// The values one line of copy data carries, refused the way PostgreSQL refuses a row that
/// does not fit the columns it is being loaded into.
fn row_values(
    line: &str,
    columns: &[(String, Option<String>)],
) -> Result<Vec<Option<String>>, EngineError> {
    let fields: Vec<&str> = line.split('\t').collect();
    if fields.len() > columns.len() {
        return Err(EngineError::new(
            EngineErrorKind::DataError,
            "extra data after last expected column".to_string(),
        ));
    }
    if fields.len() < columns.len() {
        let missing = columns
            .get(fields.len())
            .map(|(name, _)| name.clone())
            .unwrap_or_default();
        return Err(EngineError::new(
            EngineErrorKind::DataError,
            format!("missing data for column \"{missing}\""),
        ));
    }
    let mut values = Vec::with_capacity(fields.len());
    for (field, (_, declared)) in fields.iter().zip(columns) {
        // `\N` — "the default string used to indicate a NULL value" — is the NULL, and is
        // the one unquoted field that is not text.
        if *field == "\\N" {
            values.push(None);
            continue;
        }
        let value = unescaped(field);
        check_input_syntax(declared.as_deref(), &value)?;
        values.push(Some(value));
    }
    Ok(values)
}

/// Refuse a value the column's declared type does not accept, in PostgreSQL's own words
/// for a value it cannot read as that type.
///
/// The engine behind this endpoint reads a value into whatever class its column has, so a
/// value that is not a number in a numeric column would be stored as given rather than
/// refused; a load has to refuse it, because that is the error a bulk load is expected to
/// stop on. The types checked are the ones whose text form has to parse: the integer and
/// the floating/exact numeric families PostgreSQL's "Numeric Types" section lists.
fn check_input_syntax(declared: Option<&str>, value: &str) -> Result<(), EngineError> {
    let Some(declared) = declared else {
        return Ok(());
    };
    let base = declared
        .to_ascii_lowercase()
        .split('(')
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    let refused = |named: &str| {
        Err(EngineError::new(
            EngineErrorKind::DataError,
            format!("invalid input syntax for type {named}: \"{value}\""),
        ))
    };
    match base.as_str() {
        "smallint" | "int2" => {
            if value.trim().parse::<i16>().is_err() {
                return refused("smallint");
            }
        }
        "int" | "integer" | "int4" => {
            if value.trim().parse::<i32>().is_err() {
                return refused("integer");
            }
        }
        "bigint" | "int8" => {
            if value.trim().parse::<i64>().is_err() {
                return refused("bigint");
            }
        }
        "numeric" | "decimal" => {
            if value.trim().parse::<f64>().is_err() {
                return refused("numeric");
            }
        }
        "real" | "float4" => {
            if value.trim().parse::<f32>().is_err() {
                return refused("real");
            }
        }
        "double" | "float8" | "double precision" => {
            if value.trim().parse::<f64>().is_err() {
                return refused("double precision");
            }
        }
        "bool" | "boolean"
            if !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "t" | "f" | "true" | "false" | "y" | "n" | "yes" | "no" | "1" | "0" | "on" | "off"
            ) =>
        {
            return refused("boolean");
        }
        _ => {}
    }
    Ok(())
}

/// One field with the backslash escapes `COPY`'s text format defines resolved.
///
/// Source: the `COPY` reference's "Text Format" section, which lists the escapes the
/// format recognises (`\b`, `\f`, `\n`, `\r`, `\t`, `\v`, `\\`) and states that "any other
/// backslashed character that is not mentioned above will be taken to represent itself".
fn unescaped(field: &str) -> String {
    if !field.contains('\\') {
        return field.to_string();
    }
    let mut out = String::with_capacity(field.len());
    let mut characters = field.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        match characters.next() {
            Some('b') => out.push('\u{8}'),
            Some('f') => out.push('\u{c}'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('v') => out.push('\u{b}'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// An identifier as the engine is handed it, quoted so a name that needs quoting keeps its
/// case and so nothing in it is read as SQL.
///
/// `pub(super)` because every bulk load in this module tree hands the engine the table and
/// column names a client wrote: the other wire's `LOAD DATA FROM S3`
/// ([`super::mysql_s3_load`]) quotes them the same way, and one copy of that is what keeps
/// the two from quoting differently.
pub(super) fn quoted(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}
