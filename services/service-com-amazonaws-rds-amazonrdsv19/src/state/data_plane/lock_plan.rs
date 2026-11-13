// SPDX-License-Identifier: CC0-1.0

//! WHICH ROWS a statement locks, read off the statement itself.
//!
//! A statement that writes rows (`UPDATE`, `DELETE`) or that reserves them for writing
//! (`SELECT … FOR UPDATE`, `FOR SHARE`, MySQL's `LOCK IN SHARE MODE`) holds those rows for
//! as long as its transaction lasts, so before the engine runs it this module says which
//! table it is about, in which mode, what the statement asked to happen when a row is
//! already held, and the query whose rows ARE that set — `SELECT rowid FROM <table> WHERE
//! <the statement's own predicate>`, built from the statement's own parse tree rather than
//! from its text.
//!
//! The reading is the statement's, not a guess at it: the statement is parsed with the
//! session's own dialect ([`sqlparser`], the parser this crate already reads statements
//! with) and only a statement over ONE table is answered for — a join, a subquery in the
//! `FROM`, a multi-table `DELETE` names rows this module will not claim to have identified,
//! so it answers `None` and the statement runs with no row locks rather than with the wrong
//! ones.
//!
//! A locking CLAUSE also has to come off before the statement reaches the engine, which has
//! no grammar for it: the clause is taken off the statement's own token stream, so what the
//! engine runs is the client's query exactly as it was written up to the clause, and the
//! clause itself is served here ([`LockPlan::engine_sql`], which is also where `SKIP
//! LOCKED` leaves the rows another transaction holds out of the answer). That same
//! clause-less statement is what the statement's bound parameters are resolved against,
//! because it is the only form of it the engine can prepare at all.
//!
//! The four strengths PostgreSQL defines are read as the two the lock table has: `FOR
//! UPDATE` and `FOR NO KEY UPDATE` are a row reserved for writing, `FOR SHARE` and `FOR KEY
//! SHARE` a row reserved for reading — which is the conflict each pair has with the other,
//! and what MySQL's own two clauses mean.

use sqlparser::ast::{Expr, Query, SetExpr, Statement, TableFactor, TableWithJoins};
use sqlparser::dialect::{Dialect, MySqlDialect, PostgreSqlDialect};
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Token, Tokenizer};

use super::row_locks::{LockMode, WaitPolicy};
use super::EngineWire;

/// What one statement locks.
#[derive(Debug, Clone)]
pub struct LockPlan {
    /// The table whose rows are locked, as the statement named it.
    pub table: String,
    pub mode: LockMode,
    pub policy: WaitPolicy,
    /// Whether the statement WRITES those rows, rather than only reserving them.
    pub writes: bool,
    /// The query that answers WHICH rows: `SELECT rowid FROM <table> [WHERE <predicate>]`.
    pub rows_sql: String,
    /// A locking `SELECT` without its clause, kept parsed so `SKIP LOCKED` can leave the
    /// rows another transaction holds out of it; `None` for a statement the engine reads as
    /// the client wrote it.
    query: Option<Box<Query>>,
}

impl LockPlan {
    /// The statement the ENGINE is to run, or `None` where the client's own text is what it
    /// runs.
    ///
    /// `granted` is the rows the lock table gave this statement, where they are known. It
    /// matters for `SKIP LOCKED` and only there: that clause asks for the rows another
    /// transaction does NOT hold, so the query the engine runs is restricted to them.
    ///
    /// A clause-carrying statement with `granted` left out answers the client's own query
    /// WITHOUT its clause, which is the statement the engine can parse — and so also the
    /// statement its bound parameters are matched against.
    #[must_use]
    pub fn engine_sql(&self, granted: Option<&[i64]>) -> Option<String> {
        let query = self.query.as_ref()?;
        let Some(granted) = granted.filter(|_| self.policy == WaitPolicy::SkipLocked) else {
            return Some(query.to_string());
        };
        restricted_to(query, granted)
    }
}

/// What `sql` locks when the session's engine runs it, or `None` for a statement that locks
/// no rows this module can name.
#[must_use]
pub fn plan(wire: EngineWire, sql: &str) -> Option<LockPlan> {
    let dialect = dialect_of(wire);
    if let Some(clause) = locking_clause(dialect.as_ref(), sql) {
        return locking_select(dialect.as_ref(), &clause);
    }
    let mut statements = Parser::parse_sql(dialect.as_ref(), sql).ok()?;
    if statements.len() != 1 {
        return None;
    }
    match statements.remove(0) {
        Statement::Update {
            table,
            from,
            selection,
            ..
        } => {
            // An `UPDATE … FROM` reads rows of another table to decide the new values: the
            // rows it writes are still the target's, but this module only claims a plan for
            // the form whose predicate is over the target alone.
            if from.is_some() {
                return None;
            }
            let table = single_table(&table)?;
            Some(LockPlan {
                rows_sql: rows_sql(&table, selection.as_ref()),
                table,
                mode: LockMode::Exclusive,
                policy: WaitPolicy::Wait,
                writes: true,
                query: None,
            })
        }
        Statement::Delete(delete) => {
            if !delete.tables.is_empty() || delete.using.is_some() {
                return None;
            }
            let from = match &delete.from {
                sqlparser::ast::FromTable::WithFromKeyword(tables)
                | sqlparser::ast::FromTable::WithoutKeyword(tables) => tables,
            };
            let [only] = from.as_slice() else {
                return None;
            };
            let table = single_table(only)?;
            Some(LockPlan {
                rows_sql: rows_sql(&table, delete.selection.as_ref()),
                table,
                mode: LockMode::Exclusive,
                policy: WaitPolicy::Wait,
                writes: true,
                query: None,
            })
        }
        _ => None,
    }
}

/// The parser of the session's own dialect.
fn dialect_of(wire: EngineWire) -> Box<dyn Dialect> {
    match wire {
        EngineWire::Postgres => Box::new(PostgreSqlDialect {}),
        EngineWire::MySql => Box::new(MySqlDialect {}),
    }
}

/// A statement's locking clause: what it asked for, and the statement without it.
#[derive(Debug, Clone)]
struct LockingClause {
    mode: LockMode,
    policy: WaitPolicy,
    /// The statement up to the clause, which is what the engine runs.
    statement: String,
}

/// The plan of a `SELECT` that carried a locking clause.
fn locking_select(dialect: &dyn Dialect, clause: &LockingClause) -> Option<LockPlan> {
    let mut statements = Parser::parse_sql(dialect, &clause.statement).ok()?;
    if statements.len() != 1 {
        return None;
    }
    let Statement::Query(query) = statements.remove(0) else {
        return None;
    };
    let SetExpr::Select(select) = query.body.as_ref() else {
        return None;
    };
    let [only] = select.from.as_slice() else {
        return None;
    };
    let table = single_table(only)?;
    let rows_sql = rows_sql(&table, select.selection.as_ref());
    Some(LockPlan {
        table,
        mode: clause.mode,
        policy: clause.policy,
        writes: false,
        rows_sql,
        query: Some(query),
    })
}

/// The locking clause `sql` ends with, or `None` for a statement that carries none.
///
/// Read off the statement's own TOKENS, so the clause is recognised only as the keywords it
/// is — a quoted name or a string literal that spells one is a token of another kind — and
/// what is handed back as the statement is the client's own text up to the clause.
fn locking_clause(dialect: &dyn Dialect, sql: &str) -> Option<LockingClause> {
    let tokens = Tokenizer::new(dialect, sql).tokenize().ok()?;
    // The significant tokens, as uppercased keywords with their place in the stream.
    let mut keywords: Vec<(usize, String)> = Vec::new();
    for (at, token) in tokens.iter().enumerate() {
        match token {
            Token::Whitespace(_) => {}
            Token::Word(word) if word.quote_style.is_none() => {
                keywords.push((at, word.value.to_ascii_uppercase()));
            }
            other => keywords.push((at, other.to_string())),
        }
    }
    // A trailing statement terminator is not part of the clause.
    if keywords.last().is_some_and(|(_, word)| word == ";") {
        keywords.pop();
    }
    let word = |from_end: usize| -> &str {
        keywords
            .len()
            .checked_sub(from_end)
            .and_then(|at| keywords.get(at))
            .map(|(_, word)| word.as_str())
            .unwrap_or_default()
    };
    // What the statement asked to happen when the row is held, which comes last.
    let (policy, tail) = if word(2) == "SKIP" && word(1) == "LOCKED" {
        (WaitPolicy::SkipLocked, 2)
    } else if word(1) == "NOWAIT" {
        (WaitPolicy::NoWait, 1)
    } else {
        (WaitPolicy::Wait, 0)
    };
    // The strength, which the clause before it names.
    let (mode, length) = match (
        word(tail + 4),
        word(tail + 3),
        word(tail + 2),
        word(tail + 1),
    ) {
        ("FOR", "NO", "KEY", "UPDATE") => (LockMode::Exclusive, 4),
        ("LOCK", "IN", "SHARE", "MODE") => (LockMode::Shared, 4),
        (_, "FOR", "KEY", "SHARE") => (LockMode::Shared, 3),
        (_, _, "FOR", "UPDATE") => (LockMode::Exclusive, 2),
        (_, _, "FOR", "SHARE") => (LockMode::Shared, 2),
        _ => return None,
    };
    let (at, _) = *keywords.get(keywords.len().checked_sub(tail + length)?)?;
    // Everything before the clause, token for token as the client wrote it.
    let statement: String = tokens
        .get(..at)
        .unwrap_or_default()
        .iter()
        .map(ToString::to_string)
        .collect();
    if statement.trim().is_empty() {
        return None;
    }
    Some(LockingClause {
        mode,
        policy,
        statement,
    })
}

/// The one table `from` names, or `None` for a join, a subquery or anything else whose rows
/// are not one table's.
fn single_table(from: &TableWithJoins) -> Option<String> {
    if !from.joins.is_empty() {
        return None;
    }
    match &from.relation {
        TableFactor::Table {
            name,
            args: None,
            with_hints,
            ..
        } if with_hints.is_empty() => Some(name.to_string()),
        _ => None,
    }
}

/// `SELECT rowid FROM <table> [WHERE <predicate>]` — the rows the statement is about, as
/// the engine's own row ids.
fn rows_sql(table: &str, selection: Option<&Expr>) -> String {
    match selection {
        Some(predicate) => format!("SELECT rowid FROM {table} WHERE {predicate}"),
        None => format!("SELECT rowid FROM {table}"),
    }
}

/// `query` restricted to the rows `granted`, which is what `SKIP LOCKED` asked for: the rows
/// another transaction holds are not among them, so they are not in the answer either.
///
/// `None` where the restriction cannot be put back into the query; the caller then runs the
/// statement without its clause.
fn restricted_to(query: &Query, granted: &[i64]) -> Option<String> {
    let mut query = query.clone();
    let restriction = if granted.is_empty() {
        // No row was free, so the answer has no rows — the condition that says so in SQL.
        "1 = 0".to_string()
    } else {
        let ids: Vec<String> = granted.iter().map(i64::to_string).collect();
        format!("rowid IN ({})", ids.join(", "))
    };
    let SetExpr::Select(select) = query.body.as_mut() else {
        return None;
    };
    // The statement's own predicate is parenthesised, so adding to it cannot change how it
    // reads, and the whole predicate is read back by the PARSER rather than built by hand.
    let combined = match &select.selection {
        Some(existing) => format!("({existing}) AND {restriction}"),
        None => restriction,
    };
    select.selection = Some(parsed_predicate(&combined)?);
    Some(query.to_string())
}

/// The predicate `text` as an expression the parser read, so nothing is built by hand.
fn parsed_predicate(text: &str) -> Option<Expr> {
    Parser::new(&PostgreSqlDialect {})
        .try_with_sql(text)
        .and_then(|mut parser| parser.parse_expr())
        .ok()
}
