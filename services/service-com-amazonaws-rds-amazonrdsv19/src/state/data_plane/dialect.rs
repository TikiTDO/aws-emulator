// SPDX-License-Identifier: CC0-1.0

//! The statement as the SQL ENGINE is asked to run it: the client's own tokens, with the
//! values this SESSION carries substituted for the session functions and system variables
//! that name them, and with the two dialects' own syntax written in the form the engine
//! parses.
//!
//! Two kinds of rewriting live here, and nothing else:
//!
//! * SUBSTITUTION of a session value — what a server does with `current_user`: the value
//!   comes from the connection, not from the statement. Only WORD tokens are replaced, so a
//!   string literal that happens to spell one is left exactly as the client wrote it.
//! * TRANSLATION of a dialect's syntax for the same operation into the engine's: the
//!   PostgreSQL `::` cast is the engine's `CAST(… AS …)`, and a MySQL table's storage
//!   engine, character set and auto-increment column are declared differently. The
//!   statement still does what the client asked; only the spelling changes.
//!
//! Nothing here answers a statement. A statement this module cannot translate is handed to
//! the engine as the client wrote it, and the engine's own refusal is what the client is
//! told.

use sqlparser::tokenizer::Token;

use super::wire_session::{server_version_banner, SessionFacts};
use super::EngineWire;

/// The statement as the engine is asked to run it; see this module's own notes.
pub fn render_for_engine(facts: &SessionFacts, tokens: &[Token]) -> String {
    // The pieces of the rendered statement, kept separate rather than concatenated,
    // because two of the rewritings below look BACK at what has already been emitted: a
    // cast takes the expression before the `::`, and an auto-increment column takes the
    // type before the keyword.
    let mut out: Vec<String> = Vec::new();
    let mut index = 0usize;
    // Where a MySQL `CREATE TABLE`'s COLUMN-DEFINITION list closes, for the statements that
    // have one. Everything after it is table options, which the engine declares differently
    // (see `MYSQL_TABLE_OPTIONS`); everything before it is the client's own column list.
    let column_list_close = match facts.wire {
        EngineWire::MySql => mysql_column_list_close(tokens),
        EngineWire::Postgres => None,
    };
    while index < tokens.len() {
        let token = &tokens[index];
        let text = token.to_string();
        // The column list is closed: the table is declared, and what follows is the
        // options.
        if Some(index) == column_list_close {
            out.push(text);
            break;
        }
        // A MySQL system variable: `@@name`, `@@scope.name`, or `@@` followed by either.
        if let Some(name) = text.strip_prefix("@@") {
            let (name, consumed) = system_variable_name(tokens, index, name);
            out.push(quote_value(facts.setting(&name)));
            index += consumed;
            continue;
        }
        // The PostgreSQL cast operator: `<expression>::<type>`, which is the engine's
        // `CAST(<expression> AS <type>)`.
        if matches!(token, Token::DoubleColon) {
            let operand = take_operand(&mut out);
            let (target, consumed) = cast_target(tokens, index + 1);
            out.push(format!("CAST({operand} AS {target})"));
            index += 1 + consumed;
            continue;
        }
        match token {
            Token::Word(word) if word.quote_style.is_none() => {
                let lowered = word.value.to_ascii_lowercase();
                // `pg_catalog.<relation>`: the catalog schema is on every session's
                // search path, so the qualified name and the bare one are the same
                // relation — the qualifier is dropped and the relation looked up as
                // itself.
                if facts.wire == EngineWire::Postgres
                    && lowered == PG_CATALOG_SCHEMA
                    && matches!(tokens.get(index + 1), Some(Token::Period))
                {
                    index += 2;
                    continue;
                }
                // A MySQL auto-increment column is the engine's own rowid column, which it
                // declares by the column's TYPE rather than with a keyword.
                if facts.wire == EngineWire::MySql && lowered == "auto_increment" {
                    widen_to_rowid_type(&mut out);
                    index += 1;
                    continue;
                }
                let next = next_significant(tokens, index + 1);
                let called = matches!(next.and_then(|at| tokens.get(at)), Some(Token::LParen));
                match session_function(facts, &lowered, called) {
                    // A function written with an empty argument list is substituted
                    // together with it.
                    Some(value) => {
                        out.push(value);
                        index += 1;
                        if called {
                            let close = next.and_then(|at| next_significant(tokens, at + 1));
                            if matches!(close.and_then(|at| tokens.get(at)), Some(Token::RParen)) {
                                index = close.map_or(index, |at| at + 1);
                            }
                        }
                        continue;
                    }
                    None => out.push(text),
                }
            }
            _ => out.push(text),
        }
        index += 1;
    }
    out.concat()
}

/// The schema PostgreSQL's system catalogs live in, which is on every session's search path
/// — so `pg_catalog.pg_settings` and `pg_settings` name the same relation.
const PG_CATALOG_SCHEMA: &str = "pg_catalog";

/// Where a MySQL `CREATE TABLE … (<columns>) <options>` statement's COLUMN-DEFINITION list
/// closes: the index of the parenthesis that ends it, after which only table options can
/// follow.
///
/// `None` for every other statement, which is handed to the engine WHOLE — in particular
/// for `CREATE TABLE … AS SELECT …` and `CREATE TABLE … (<columns>) SELECT …`, where what
/// follows the table name (or the column list) is a QUERY and not options: dropping it
/// would create the table from something other than what the client asked for. The two
/// conditions are what tell them apart: the column list has to be the first thing after
/// the table name, and a statement that carries a `SELECT` at all is a create-table-as-
/// select whatever its shape.
fn mysql_column_list_close(tokens: &[Token]) -> Option<usize> {
    if !leads_with_create_table(tokens) {
        return None;
    }
    let significant: Vec<usize> = (0..tokens.len())
        .filter(|at| !matches!(tokens.get(*at), Some(Token::Whitespace(_))))
        .collect();
    let word_at = |at: usize| match tokens.get(at) {
        Some(Token::Word(word)) => Some(word.value.to_ascii_uppercase()),
        _ => None,
    };
    // A query anywhere in the statement makes it a create-table-as-select.
    if significant
        .iter()
        .any(|at| word_at(*at).is_some_and(|word| word == "SELECT"))
    {
        return None;
    }
    let mut cursor = significant
        .iter()
        .position(|at| word_at(*at).is_some_and(|word| word == "TABLE"))?
        + 1;
    // The optional `IF NOT EXISTS`, then the table's own name, which may be qualified by
    // the database it is in.
    while significant
        .get(cursor)
        .and_then(|at| word_at(*at))
        .is_some_and(|word| matches!(word.as_str(), "IF" | "NOT" | "EXISTS"))
    {
        cursor += 1;
    }
    significant.get(cursor).and_then(|at| word_at(*at))?;
    cursor += 1;
    if matches!(
        significant.get(cursor).and_then(|at| tokens.get(*at)),
        Some(Token::Period)
    ) {
        cursor += 2;
    }
    // The column list has to start RIGHT HERE for the rest to be options.
    let open = *significant.get(cursor)?;
    if !matches!(tokens.get(open), Some(Token::LParen)) {
        return None;
    }
    let mut depth = 0usize;
    for at in significant.iter().skip(cursor) {
        match tokens.get(*at) {
            Some(Token::LParen) => depth += 1,
            Some(Token::RParen) => {
                depth -= 1;
                if depth == 0 {
                    return Some(*at);
                }
            }
            _ => {}
        }
    }
    None
}

/// Whether the statement is a `CREATE TABLE` — including the `TEMPORARY` form — which is
/// the only statement whose trailing words are table options.
fn leads_with_create_table(tokens: &[Token]) -> bool {
    let words: Vec<String> = tokens
        .iter()
        .filter(|token| !matches!(token, Token::Whitespace(_)))
        .take(4)
        .map(|token| match token {
            Token::Word(word) => word.value.to_ascii_uppercase(),
            other => other.to_string(),
        })
        .collect();
    words.first().is_some_and(|first| first == "CREATE")
        && words.iter().take(4).any(|word| word == "TABLE")
}

/// The column types a MySQL `AUTO_INCREMENT` column may be declared with, and which the
/// engine needs declared as its own rowid type for the column to be assigned values.
///
/// MySQL's `AUTO_INCREMENT` column "generates a unique identity for new rows" and must be
/// an integer column. The engine assigns those values for the column it calls `INTEGER
/// PRIMARY KEY`, so a column declared with any of MySQL's integer widths is declared to it
/// as `INTEGER` — the same column, holding the same values, assigned the same way.
const MYSQL_INTEGER_TYPES: [&str; 6] = [
    "tinyint",
    "smallint",
    "mediumint",
    "int",
    "integer",
    "bigint",
];

/// Declare the column being defined with the engine's rowid type, by rewriting the most
/// recent integer type word that was emitted. A declaration with no integer type before the
/// keyword is left alone — the engine then refuses it, as MySQL does.
fn widen_to_rowid_type(out: &mut [String]) {
    for piece in out.iter_mut().rev() {
        if MYSQL_INTEGER_TYPES
            .iter()
            .any(|known| piece.eq_ignore_ascii_case(known))
        {
            *piece = "INTEGER".to_string();
            return;
        }
        // Only the column currently being declared: a comma or an opening parenthesis ends
        // the walk back, so one column's keyword never rewrites another's type.
        if piece == "," || piece == "(" {
            return;
        }
    }
}

/// The name a MySQL system-variable reference carries, and how many tokens it spans.
///
/// The reference is written `@@name`, `@@scope.name` or `@@` followed by either, and the
/// scope (`GLOBAL`, `SESSION`, `LOCAL`) says WHERE the variable is read, not which variable
/// it is — so it is stripped, whether the tokenizer handed it over joined to the name or as
/// its own token followed by `.` and the name.
fn system_variable_name(tokens: &[Token], at: usize, suffix: &str) -> (String, usize) {
    let (mut name, mut consumed) = if suffix.is_empty() {
        (
            tokens
                .get(at + 1)
                .map(ToString::to_string)
                .unwrap_or_default(),
            2,
        )
    } else {
        (suffix.to_string(), 1)
    };
    for scope in ["global", "session", "local"] {
        if let Some(rest) = name.to_ascii_lowercase().strip_prefix(&format!("{scope}.")) {
            return (rest.to_string(), consumed);
        }
        if name.eq_ignore_ascii_case(scope) {
            // `@@session` `.` `name`: the scope word, the separator and the name.
            if matches!(tokens.get(at + consumed), Some(Token::Period)) {
                name = tokens
                    .get(at + consumed + 1)
                    .map(ToString::to_string)
                    .unwrap_or_default();
                consumed += 2;
            }
            return (name, consumed);
        }
    }
    (name, consumed)
}

/// The expression a `::` casts, taken back off what has already been emitted.
///
/// The operand of the cast operator is the PRIMARY expression to its left: a parenthesised
/// group (with the function name in front of it, where it is a call), or a single literal,
/// placeholder or possibly qualified identifier. Taking exactly that keeps the rest of the
/// statement — the operators and operands around it — as the client wrote it.
fn take_operand(out: &mut Vec<String>) -> String {
    while out.last().is_some_and(|piece| piece.trim().is_empty()) {
        out.pop();
    }
    let Some(last) = out.pop() else {
        return String::new();
    };
    let mut parts = vec![last.clone()];
    if last == ")" {
        let mut depth = 1usize;
        while depth > 0 {
            let Some(piece) = out.pop() else { break };
            if piece == ")" {
                depth += 1;
            } else if piece == "(" {
                depth -= 1;
            }
            parts.push(piece);
        }
        if out
            .last()
            .is_some_and(|piece| piece.chars().all(|c| c.is_alphanumeric() || c == '_'))
        {
            if let Some(name) = out.pop() {
                parts.push(name);
            }
        }
    } else {
        while out.last().map(String::as_str) == Some(".") {
            if let Some(separator) = out.pop() {
                parts.push(separator);
            }
            if let Some(qualifier) = out.pop() {
                parts.push(qualifier);
            }
        }
    }
    parts.reverse();
    parts.concat()
}

/// The type a `::` casts TO, as the engine spells it, and how many tokens it spans.
///
/// The type is read as the client wrote it — a name, optionally with the precision and scale
/// a numeric type carries — and then named in the engine's own type vocabulary
/// ([`engine_type`]).
fn cast_target(tokens: &[Token], from: usize) -> (String, usize) {
    let mut consumed = 0usize;
    let mut words: Vec<String> = Vec::new();
    let mut arguments = String::new();
    let mut at = from;
    while let Some(token) = tokens.get(at) {
        match token {
            Token::Whitespace(_) if words.is_empty() => {}
            Token::Word(word) => {
                // A multi-word type name (`double precision`, `timestamp with time zone`)
                // is read whole; a word that starts the rest of the expression is not part
                // of the type.
                if !words.is_empty() && !TYPE_CONTINUATIONS.contains(&word.value.to_ascii_lowercase().as_str()) {
                    break;
                }
                words.push(word.value.to_ascii_lowercase());
            }
            Token::LParen if !words.is_empty() && arguments.is_empty() => {
                let mut depth = 1usize;
                arguments.push('(');
                let mut inner = at + 1;
                while depth > 0 {
                    let Some(token) = tokens.get(inner) else { break };
                    match token {
                        Token::LParen => depth += 1,
                        Token::RParen => depth -= 1,
                        _ => {}
                    }
                    arguments.push_str(&token.to_string());
                    inner += 1;
                }
                consumed += inner - at - 1;
                at = inner - 1;
            }
            Token::Whitespace(_) => break,
            _ => break,
        }
        consumed += 1;
        at += 1;
    }
    let base = engine_type(&words.join(" "));
    (format!("{base}{arguments}"), consumed)
}

/// The words that CONTINUE a type name rather than starting the next expression.
const TYPE_CONTINUATIONS: [&str; 6] = ["precision", "with", "without", "time", "zone", "varying"];

/// The engine's type for the PostgreSQL type `named`.
///
/// Source: the PostgreSQL "Data Types" chapter's type names and aliases on one side, and the
/// engine's own storage classes on the other — the engine holds exact numbers, integers,
/// floating point, text and byte strings, so each PostgreSQL type is cast to the class that
/// holds its values. A type name this does not carry is cast to text, which is the form the
/// wire reports a value it has no narrower type for.
fn engine_type(named: &str) -> &'static str {
    match named {
        "numeric" | "decimal" | "money" => "NUMERIC",
        "smallint" | "int" | "integer" | "bigint" | "int2" | "int4" | "int8" | "oid" => "INTEGER",
        "real" | "float4" | "float8" | "double precision" | "double" => "REAL",
        "boolean" | "bool" => "NUMERIC",
        "bytea" => "BLOB",
        _ => "TEXT",
    }
}

/// The index of the next token that is not whitespace, at or after `from`.
pub fn next_significant(tokens: &[Token], from: usize) -> Option<usize> {
    tokens
        .iter()
        .enumerate()
        .skip(from)
        .find(|(_, token)| !matches!(token, Token::Whitespace(_)))
        .map(|(index, _)| index)
}

/// The value a session function returns, as a SQL literal, or `None` for a word that is
/// not one.
///
/// `called` says whether the word is followed by an argument list, which is what separates
/// the two kinds: the forms the SQL standard spells as KEYWORDS (`current_user`,
/// `session_user`) carry no parentheses, while the rest are functions and are substituted
/// only where they are CALLED — so a column or alias that happens to be named `version`
/// stays the column it is.
///
/// The functions this server runs in the ENGINE rather than substituting here
/// ([`super::pg_server::SERVER_FUNCTIONS`]) are deliberately absent: a statement that calls
/// one is planned and run like any other statement, which is what keeps its column name and
/// its type the ones PostgreSQL reports.
fn session_function(facts: &SessionFacts, lowered: &str, called: bool) -> Option<String> {
    let text = |value: &str| Some(format!("'{}'", value.replace('\'', "''")));
    if super::pg_server::is_server_function(lowered) {
        return None;
    }
    match lowered {
        // A MySQL session's identity is an ACCOUNT, `user@host`: `CURRENT_USER()` reports
        // the account whose privileges the session holds — for the RDS master user, the
        // `'<master>'@'%'` account RDS grants — and `USER()` reports the account name with
        // the host the client actually connected from, which is the endpoint's own host.
        "current_user" | "current_role" if facts.wire == EngineWire::MySql => {
            text(&facts.mysql_account())
        }
        "user" | "session_user" if facts.wire == EngineWire::MySql => {
            text(&format!("{}@{}", facts.user, facts.endpoint_host))
        }
        "current_user" | "session_user" | "user" | "current_role" => text(&facts.user),
        "current_catalog" => text(&facts.database),
        "current_schema" if !called => text(match facts.wire {
            EngineWire::Postgres => "public",
            EngineWire::MySql => &facts.database,
        }),
        _ if !called => None,
        "current_database" | "database" | "schema" => text(&facts.database),
        "current_schema" => text(match facts.wire {
            EngineWire::Postgres => "public",
            EngineWire::MySql => &facts.database,
        }),
        "connection_id" => Some(facts.connection_id.to_string()),
        "version" => text(&server_version_banner(facts)),
        _ => None,
    }
}

/// `value` as a SQL literal, or `NULL` where the session has no such value.
///
/// A system variable whose value is a WHOLE NUMBER is a numeric variable of the server
/// (`read_only`, `max_connections`), and both engines answer one as a number rather than as
/// text — which is the type the wire then reports for the column, and what a client reads it
/// back as. Every other value is text, quoted so the statement the engine runs carries it as
/// one literal whatever it contains.
fn quote_value(value: Option<String>) -> String {
    match value {
        Some(value) if value.trim().parse::<i64>().is_ok() => value.trim().to_string(),
        Some(value) => format!("'{}'", value.replace('\'', "''")),
        None => "NULL".to_string(),
    }
}
