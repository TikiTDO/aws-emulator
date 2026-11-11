// SPDX-License-Identifier: CC0-1.0

//! WHICH STATEMENTS a READ-ONLY endpoint refuses, and how each engine words the refusal.
//!
//! Two of the addresses this service publishes are read-only: a DB cluster's reader
//! endpoint, which "load-balances connections across the Aurora Replicas that are available
//! in a DB cluster" — and those replicas are "read-only instances" — and the endpoint of a
//! DB instance created as a read replica, which "RDS then uses the asynchronous replication
//! method for the DB engine to update the read replica". A session on either runs on an
//! engine that is replaying someone else's writes, so the engine refuses a statement of its
//! own that would write, and that refusal is this module.
//!
//! WHICH statements those are is the catalogue PostgreSQL's own `SET TRANSACTION` page
//! publishes for a read-only transaction: `INSERT`, `UPDATE`, `DELETE`, `MERGE` and `COPY
//! FROM`, every `CREATE`, `ALTER` and `DROP` command, and `COMMENT`, `GRANT`, `REVOKE` and
//! `TRUNCATE`. MySQL's `read_only` system variable draws the same line — "the server permits
//! no client updates", refusing the statements that modify tables or data — so the two wires
//! refuse the same statements and differ only in what they call the refusal.
//!
//! Read off the statement's own leading KEYWORDS, which is what names the command a server
//! reports in that refusal. A statement whose first keyword is none of them is handed to the
//! engine: a `WITH … INSERT` (a data-modifying statement written as a query) and a function
//! call that writes inside itself are not recognised here, and the engine runs them — the
//! limit of reading the command without planning the statement.

use sqlparser::tokenizer::Token;

use super::sql_engine::{EngineError, EngineErrorKind};
use super::EngineWire;

/// The SQLSTATE PostgreSQL reports a write on a read-only session under:
/// `read_only_sql_transaction`.
pub const POSTGRES_READ_ONLY_SQLSTATE: &str = "25006";

/// The MySQL server error a write on a read-only server is refused with:
/// `ER_OPTION_PREVENTS_STATEMENT`, under the general-error SQLSTATE it carries.
pub const MYSQL_READ_ONLY_ERROR: (u16, &str) = (1290, "HY000");

/// What MySQL answers a statement its `read_only` setting refuses with, worded as the
/// server words it.
const MYSQL_READ_ONLY_MESSAGE: &str =
    "The MySQL server is running with the --read-only option so it cannot execute this \
     statement";

/// The commands a read-only session refuses that are named by ONE keyword, with the command
/// name the refusal reports for each.
///
/// `TRUNCATE` and `COPY` are here under the names the server reports for them (`TRUNCATE
/// TABLE`, and `COPY FROM` — the direction that writes; a `COPY … TO` reads and is served).
const SINGLE_WORD_COMMANDS: [(&str, &str); 10] = [
    ("INSERT", "INSERT"),
    ("UPDATE", "UPDATE"),
    ("DELETE", "DELETE"),
    ("MERGE", "MERGE"),
    ("REPLACE", "REPLACE"),
    ("TRUNCATE", "TRUNCATE TABLE"),
    ("COMMENT", "COMMENT"),
    ("GRANT", "GRANT"),
    ("REVOKE", "REVOKE"),
    ("RENAME", "RENAME TABLE"),
];

/// The commands whose name is the keyword plus the KIND OF OBJECT it is about, which is how
/// a server reports them (`CREATE TABLE`, `DROP INDEX`).
const OBJECT_COMMANDS: [&str; 3] = ["CREATE", "ALTER", "DROP"];

/// The words between such a command and the object kind it names, which are part of neither:
/// `CREATE OR REPLACE VIEW` is a `CREATE VIEW`, `CREATE UNIQUE INDEX` a `CREATE INDEX`, and
/// `DROP TABLE IF EXISTS` a `DROP TABLE`.
const OBJECT_MODIFIERS: [&str; 9] = [
    "OR",
    "REPLACE",
    "UNIQUE",
    "TEMP",
    "TEMPORARY",
    "GLOBAL",
    "LOCAL",
    "MATERIALIZED",
    "CONCURRENTLY",
];

/// The command a read-only session refuses `tokens`, under the name the server's refusal
/// reports for it — or `None` for a statement such a session serves.
#[must_use]
pub fn refused_command(tokens: &[Token]) -> Option<String> {
    // Enough of the statement to carry a command name and the direction of a `COPY`.
    let words = keywords(tokens, MAX_COMMAND_WORDS);
    let first = words.first()?.clone();
    let first = first.as_str();
    if let Some((_, named)) = SINGLE_WORD_COMMANDS
        .iter()
        .find(|(keyword, _)| *keyword == first)
    {
        return Some((*named).to_string());
    }
    // Only the direction of a `COPY` that loads rows into a table writes; a `COPY … TO`
    // reads, and is served.
    if first == "COPY" {
        return words
            .iter()
            .any(|word| word == "FROM")
            .then(|| "COPY FROM".to_string());
    }
    // `LOAD DATA`, the MySQL statement that loads rows into a table.
    if first == "LOAD" {
        return Some("LOAD DATA".to_string());
    }
    if OBJECT_COMMANDS.contains(&first) {
        // The object kind, past the words that only qualify the command.
        let kind = words
            .into_iter()
            .skip(1)
            .find(|word| !OBJECT_MODIFIERS.contains(&word.as_str()))?;
        return Some(format!("{first} {kind}"));
    }
    None
}

/// How many leading keywords name a command: the longest of the forms above is a `COPY
/// <table> (<columns>) FROM …`, whose direction comes after the column list.
const MAX_COMMAND_WORDS: usize = 24;

/// How `wire`'s engine refuses `command` on a read-only session.
///
/// PostgreSQL names the command it refused and reports the condition's own SQLSTATE;
/// MySQL's refusal is the one sentence its `read_only` setting has, whatever the statement
/// was.
#[must_use]
pub fn refusal(wire: EngineWire, command: &str) -> EngineError {
    match wire {
        EngineWire::Postgres => EngineError::new(
            EngineErrorKind::ReadOnlySqlTransaction,
            format!("cannot execute {command} in a read-only transaction"),
        )
        .with_sqlstate(POSTGRES_READ_ONLY_SQLSTATE),
        EngineWire::MySql => EngineError::new(
            EngineErrorKind::ReadOnlySqlTransaction,
            MYSQL_READ_ONLY_MESSAGE.to_string(),
        )
        .with_mysql_error(MYSQL_READ_ONLY_ERROR.0, MYSQL_READ_ONLY_ERROR.1),
    }
}

/// How PostgreSQL refuses a session's attempt to put ITSELF back into read-write mode while
/// the engine it is on is replaying from a primary.
///
/// "cannot set transaction read-write mode during recovery" is the server's own sentence for
/// it, under the same condition a write is refused with: a standby has no read-write mode to
/// move to, so the session cannot grant itself one.
#[must_use]
pub fn postgres_cannot_leave_read_only() -> EngineError {
    EngineError::new(
        EngineErrorKind::ReadOnlySqlTransaction,
        "cannot set transaction read-write mode during recovery".to_string(),
    )
    .with_sqlstate(POSTGRES_READ_ONLY_SQLSTATE)
}

/// The leading `count` keywords of `tokens`, uppercased: the words that name the command.
fn keywords(tokens: &[Token], count: usize) -> Vec<String> {
    tokens
        .iter()
        .filter(|token| !matches!(token, Token::Whitespace(_)))
        .take(count)
        .map(|token| match token {
            Token::Word(word) => word.value.to_ascii_uppercase(),
            other => other.to_string(),
        })
        .collect()
}
