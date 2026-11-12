// SPDX-License-Identifier: CC0-1.0

//! The statements a client sends that the SERVER answers out of its own catalog rather
//! than by running them over the session's tables: installing an extension, creating a
//! database, and opening or ending a transaction.
//!
//! Why they are here and not handed to the engine: each one is defined by the engine the
//! endpoint reports itself as — Amazon RDS for PostgreSQL ships `aws_s3` and installs it
//! with `CREATE EXTENSION`, RDS for MySQL creates a schema with `CREATE DATABASE`, and
//! both answer a `COMMIT` with no transaction open as a success — while the embedded
//! engine (SQLite, see [`super::sql_engine`]) has no grammar for any of them. Handing them
//! to it turned statements the real engines SERVE into refusals, under that engine's own
//! message and a code class neither engine uses.
//!
//! So each one is served from the INSTANCE's own state: the databases the instance holds
//! ([`super::sql_engine::InstanceDatabase`]) and, for a PostgreSQL database, the
//! `pg_extension` catalog kept inside it — which is where a client reads its extensions
//! back from, with ordinary SQL. Nothing is answered from the shape of the statement's
//! text: a create that cannot be served is refused with the error its own engine defines
//! for the condition, and the engine's transaction is the transaction both wires report.

use sqlparser::tokenizer::Token;

use super::sql_engine::{Cell, EngineError, EngineErrorKind, InstanceDatabase, ResultSet};
use super::wire_session::{SessionFacts, StatementAnswer, WireNotice, MYSQL_SYSTEM_DATABASES};
use super::EngineWire;

/// One extension an Amazon RDS for PostgreSQL DB instance ships.
#[derive(Debug, Clone, Copy)]
pub struct Extension {
    /// The name `CREATE EXTENSION` installs it under.
    pub name: &'static str,
    /// The version its control file declares as the default, which is what
    /// `pg_extension.extversion` reports for an install that names no version.
    pub version: &'static str,
    /// The extensions it requires, which `CASCADE` installs with it.
    pub requires: &'static [&'static str],
}

/// The extensions this endpoint ships, which is what `CREATE EXTENSION` can install.
///
/// Source: the list of extensions Amazon RDS for PostgreSQL supports, in the Amazon RDS
/// User Guide ("Extension versions for Amazon RDS for PostgreSQL"), with each default
/// version as that extension's own control file declares it. A name this list does not
/// carry is one the instance has no control file for, which is the condition PostgreSQL
/// answers `extension "<name>" is not available` to.
const EXTENSIONS: &[Extension] = &[
    Extension {
        name: "plpgsql",
        version: "1.0",
        requires: &[],
    },
    Extension {
        name: "aws_commons",
        version: "1.2",
        requires: &[],
    },
    // The two integrations whose functions live in these extensions: `aws_s3` imports and
    // exports S3 objects, `aws_lambda` invokes a function, and AWS documents both as
    // requiring `aws_commons`.
    Extension {
        name: "aws_s3",
        version: "1.1",
        requires: &["aws_commons"],
    },
    Extension {
        name: "aws_lambda",
        version: "1.0",
        requires: &["aws_commons"],
    },
    Extension {
        name: "pgcrypto",
        version: "1.3",
        requires: &[],
    },
    Extension {
        name: "uuid-ossp",
        version: "1.1",
        requires: &[],
    },
    Extension {
        name: "hstore",
        version: "1.8",
        requires: &[],
    },
    Extension {
        name: "citext",
        version: "1.6",
        requires: &[],
    },
    Extension {
        name: "ltree",
        version: "1.2",
        requires: &[],
    },
    Extension {
        name: "pg_trgm",
        version: "1.6",
        requires: &[],
    },
    Extension {
        name: "btree_gin",
        version: "1.3",
        requires: &[],
    },
    Extension {
        name: "btree_gist",
        version: "1.7",
        requires: &[],
    },
    Extension {
        name: "tablefunc",
        version: "1.0",
        requires: &[],
    },
    Extension {
        name: "pg_stat_statements",
        version: "1.10",
        requires: &[],
    },
];

/// The extension `name` names among the ones this endpoint ships.
///
/// PostgreSQL resolves an extension by the name of its control file, which is
/// case-sensitive, so this is an exact match.
#[must_use]
pub fn extension(name: &str) -> Option<&'static Extension> {
    EXTENSIONS.iter().find(|entry| entry.name == name)
}

/// `pg_extension`, as this server keeps it: one row per extension installed in THIS
/// database, which is where a client reads them back from.
///
/// The catalog lives inside the database the extension is installed in, because that is
/// what an extension belongs to on PostgreSQL — installing it in one database does not
/// install it in another.
const PG_EXTENSION_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS pg_extension (\
     oid integer PRIMARY KEY, extname text NOT NULL, extowner text NOT NULL, \
     extnamespace text NOT NULL, extversion text NOT NULL)";

/// The schema an extension's objects go in when the statement names none, which is the
/// schema a client's own objects are in.
const DEFAULT_EXTENSION_SCHEMA: &str = "public";

/// The extension every PostgreSQL database is created with.
///
/// `CREATE DATABASE` copies `template1`, which carries the procedural language, so a
/// client reads it back from the catalog of a database nobody has installed anything in.
const PRE_INSTALLED_EXTENSION: &str = "plpgsql";

/// Bring a PostgreSQL database's extension catalog into existence, once, with the
/// extension the database was created with already in it.
///
/// Called as a session opens, because `pg_extension` is a catalog of the DATABASE and a
/// client may read it before it installs anything. Only a database whose catalog does not
/// exist yet gets one: an extension a client dropped is not installed again behind it.
pub fn open_extension_catalog(
    database: &InstanceDatabase,
    facts: &SessionFacts,
) -> Result<(), EngineError> {
    let present = database.execute_in(
        facts.connection_id,
        &facts.database,
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'pg_extension'",
        &[],
    )?;
    let present = matches!(
        present.rows.first().and_then(|row| row.first()),
        Some(Cell::Integer(1..))
    );
    if present {
        return Ok(());
    }
    database.execute_in(facts.connection_id, &facts.database, PG_EXTENSION_SCHEMA, &[])?;
    if let Some(entry) = extension(PRE_INSTALLED_EXTENSION) {
        record_extension(database, facts, *entry, None)?;
    }
    Ok(())
}

/// The extensions installed in the session's database, in the order they were installed.
fn installed(database: &InstanceDatabase, facts: &SessionFacts) -> Result<Vec<String>, EngineError> {
    database.execute_in(facts.connection_id, &facts.database, PG_EXTENSION_SCHEMA, &[])?;
    let rows = database.execute_in(
        facts.connection_id,
        &facts.database,
        "SELECT extname FROM pg_extension ORDER BY oid",
        &[],
    )?;
    Ok(rows
        .rows
        .iter()
        .filter_map(|row| match row.first() {
            Some(Cell::Text(name)) => Some(name.clone()),
            _ => None,
        })
        .collect())
}

/// Record a statement that changed what a DB instance HOLDS.
///
/// Creating or dropping a database or an extension is a change to the instance's own
/// contents — a drop takes every table in that namespace with it — so each one leaves a
/// line naming the instance, the session that ran it and the object it changed, the way an
/// engine session's refusals are recorded ([`super::wire_session::record_refused_login`]).
/// The statement's text is not recorded: what the object was and what happened to it is
/// what a reader needs.
fn record_catalog_change(facts: &SessionFacts, outcome: &'static str, object: &str) {
    ::tracing::info!(
        audit = true,
        operation = "DbInstanceEngineStatement",
        outcome,
        db_instance_identifier = %facts.identifier,
        connection_id = facts.connection_id,
        object = %object,
        "a statement changed what a DB instance's engine holds"
    );
}

/// `CREATE EXTENSION [IF NOT EXISTS] <name> [WITH] [SCHEMA s] [VERSION v] [CASCADE]`.
///
/// What PostgreSQL does, in its own order: an extension already installed is a refusal (or
/// a notice and nothing else, under `IF NOT EXISTS`); a name the instance has no control
/// file for is not available; an extension whose requirements are not installed needs
/// `CASCADE`, which installs them first.
pub fn create_extension(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    tokens: &[Token],
) -> Result<StatementAnswer, EngineError> {
    let statement = object_statement(tokens).ok_or_else(syntax_error_at_end)?;
    let name = statement.name;
    let installed = installed(database, facts)?;
    if installed.iter().any(|known| known == &name) {
        if statement.conditional {
            return Ok(StatementAnswer::new("CREATE EXTENSION", ResultSet::changed(0, 0))
                .with_notice(WireNotice::notice(
                    "42710",
                    format!("extension \"{name}\" already exists, skipping"),
                )));
        }
        return Err(EngineError::new(
            EngineErrorKind::DuplicateObject,
            format!("extension \"{name}\" already exists"),
        ));
    }
    let Some(entry) = extension(&name) else {
        return Err(EngineError::new(
            EngineErrorKind::FeatureNotSupported,
            format!("extension \"{name}\" is not available"),
        ));
    };
    // What has to be installed before it, in dependency order. `CASCADE` is what allows
    // this server to install them; without it PostgreSQL refuses and says so.
    let mut order = Vec::new();
    required_extensions(entry, &installed, &mut order);
    let mut answer = StatementAnswer::new("CREATE EXTENSION", ResultSet::changed(0, 0));
    for required in &order {
        if !statement.cascade {
            return Err(EngineError::new(
                EngineErrorKind::UndefinedObject,
                format!("required extension \"{}\" is not installed", required.name),
            )
            .with_sqlstate("42704"));
        }
        answer = answer.with_notice(WireNotice::notice(
            "00000",
            format!("installing required extension \"{}\"", required.name),
        ));
    }
    order.push(*entry);
    let last = order.len().saturating_sub(1);
    for (position, install) in order.into_iter().enumerate() {
        // A `VERSION` clause names the version of the extension the statement is FOR; the
        // requirements CASCADE installs are installed at the version each declares.
        let version = (position == last).then_some(statement.version.as_deref()).flatten();
        record_extension(database, facts, install, version)?;
        record_catalog_change(facts, "extension-installed", install.name);
    }
    Ok(answer)
}

/// The extensions `entry` requires that are not installed yet, deepest first.
fn required_extensions(
    entry: &Extension,
    installed: &[String],
    order: &mut Vec<Extension>,
) {
    for required in entry.requires {
        if installed.iter().any(|known| known == required)
            || order.iter().any(|queued| queued.name == *required)
        {
            continue;
        }
        if let Some(required) = extension(required) {
            required_extensions(required, installed, order);
            order.push(*required);
        }
    }
}

/// Write one installed extension into the database's `pg_extension`.
fn record_extension(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    entry: Extension,
    version: Option<&str>,
) -> Result<(), EngineError> {
    database.execute_in(facts.connection_id, &facts.database, PG_EXTENSION_SCHEMA, &[])?;
    database.execute_in(
        facts.connection_id,
        &facts.database,
        "INSERT INTO pg_extension (oid, extname, extowner, extnamespace, extversion) \
         VALUES (NULL, ?, ?, ?, ?)",
        &[
            Some(entry.name.to_string()),
            Some(facts.user.clone()),
            Some(DEFAULT_EXTENSION_SCHEMA.to_string()),
            Some(version.unwrap_or(entry.version).to_string()),
        ],
    )?;
    Ok(())
}

/// `DROP EXTENSION [IF EXISTS] <name> [CASCADE | RESTRICT]`.
///
/// An extension another installed extension requires is not dropped on its own:
/// PostgreSQL refuses it unless `CASCADE` drops the dependents with it.
pub fn drop_extension(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    tokens: &[Token],
) -> Result<StatementAnswer, EngineError> {
    let statement = object_statement(tokens).ok_or_else(syntax_error_at_end)?;
    let name = statement.name;
    let installed = installed(database, facts)?;
    if !installed.iter().any(|known| known == &name) {
        if statement.conditional {
            return Ok(StatementAnswer::new("DROP EXTENSION", ResultSet::changed(0, 0))
                .with_notice(WireNotice::notice(
                    "00000",
                    format!("extension \"{name}\" does not exist, skipping"),
                )));
        }
        return Err(EngineError::new(
            EngineErrorKind::UndefinedObject,
            format!("extension \"{name}\" does not exist"),
        )
        .with_sqlstate("42704"));
    }
    let dependents: Vec<String> = installed
        .iter()
        .filter(|candidate| {
            extension(candidate)
                .is_some_and(|entry| entry.requires.iter().any(|required| *required == name))
        })
        .cloned()
        .collect();
    if !dependents.is_empty() && !statement.cascade {
        return Err(EngineError::new(
            EngineErrorKind::ObjectInUse,
            format!("cannot drop extension {name} because other objects depend on it"),
        )
        .with_sqlstate("2BP01"));
    }
    let mut answer = StatementAnswer::new("DROP EXTENSION", ResultSet::changed(0, 0));
    for dependent in dependents {
        database.execute_in(
            facts.connection_id,
            &facts.database,
            "DELETE FROM pg_extension WHERE extname = ?",
            &[Some(dependent.clone())],
        )?;
        record_catalog_change(facts, "extension-dropped", &dependent);
        answer = answer.with_notice(WireNotice::notice(
            "00000",
            format!("drop cascades to extension {dependent}"),
        ));
    }
    database.execute_in(
        facts.connection_id,
        &facts.database,
        "DELETE FROM pg_extension WHERE extname = ?",
        &[Some(name.clone())],
    )?;
    record_catalog_change(facts, "extension-dropped", &name);
    Ok(answer)
}

/// `CREATE {DATABASE | SCHEMA} [IF NOT EXISTS] <name> [<options>]` — the statement that
/// creates a MySQL schema or a PostgreSQL database.
///
/// The name becomes a database of this instance: its own namespace of tables, which a
/// later `USE`, a later login and `SHOW DATABASES` all see.
pub fn create_database(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    words: &[String],
    tokens: &[Token],
) -> Result<StatementAnswer, EngineError> {
    let object = words.get(1).map(String::as_str).unwrap_or("DATABASE");
    let statement = object_statement(tokens).ok_or_else(syntax_error_at_end)?;
    let name = statement.name;
    // PostgreSQL's `CREATE DATABASE` has no `IF NOT EXISTS`, so a client that wrote one
    // wrote something its grammar does not define.
    if facts.wire == EngineWire::Postgres && statement.conditional {
        return Err(EngineError::new(
            EngineErrorKind::Syntax,
            "syntax error at or near \"NOT\"".to_string(),
        ));
    }
    if database_exists(database, facts, &name) {
        return match (facts.wire, statement.conditional) {
            // MySQL's note for a create of a schema that is already there: the statement
            // succeeds and creates nothing.
            (EngineWire::MySql, true) => Ok(StatementAnswer::new(
                format!("CREATE {object}"),
                ResultSet::changed(0, 0),
            )),
            (EngineWire::MySql, false) => Err(EngineError::new(
                EngineErrorKind::DuplicateObject,
                format!("Can't create database '{name}'; database exists"),
            )
            // `ER_DB_CREATE_EXISTS`, which is what makes a client's create idempotent.
            .with_mysql_error(1007, "HY000")),
            (EngineWire::Postgres, _) => Err(EngineError::new(
                EngineErrorKind::DuplicateObject,
                format!("database \"{name}\" already exists"),
            )
            .with_sqlstate("42P04")),
        };
    }
    database.create_database(&name)?;
    record_catalog_change(facts, "database-created", &name);
    let mut answer = StatementAnswer::new(
        format!("CREATE {object}"),
        // MySQL answers its OK packet for a created schema with one row affected;
        // PostgreSQL's `CommandComplete` carries no count for a created database.
        match facts.wire {
            EngineWire::MySql => ResultSet::changed(1, 0),
            EngineWire::Postgres => ResultSet::changed(0, 0),
        },
    );
    answer.databases_changed = true;
    Ok(answer)
}

/// `DROP {DATABASE | SCHEMA} [IF EXISTS] <name>` — the statement that drops a MySQL
/// schema or a PostgreSQL database, with every table in it.
pub fn drop_database(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    words: &[String],
    tokens: &[Token],
) -> Result<StatementAnswer, EngineError> {
    let object = words.get(1).map(String::as_str).unwrap_or("DATABASE");
    let statement = object_statement(tokens).ok_or_else(syntax_error_at_end)?;
    let name = statement.name;
    if !database_exists(database, facts, &name) {
        return match (facts.wire, statement.conditional) {
            (EngineWire::MySql, true) => Ok(StatementAnswer::new(
                format!("DROP {object}"),
                ResultSet::changed(0, 0),
            )),
            // `ER_DB_DROP_EXISTS`.
            (EngineWire::MySql, false) => Err(EngineError::new(
                EngineErrorKind::UndefinedDatabase,
                format!("Can't drop database '{name}'; database doesn't exist"),
            )
            .with_mysql_error(1008, "HY000")),
            (EngineWire::Postgres, true) => Ok(StatementAnswer::new(
                format!("DROP {object}"),
                ResultSet::changed(0, 0),
            )
            .with_notice(WireNotice::notice(
                "00000",
                format!("database \"{name}\" does not exist, skipping"),
            ))),
            (EngineWire::Postgres, false) => Err(EngineError::new(
                EngineErrorKind::UndefinedDatabase,
                format!("database \"{name}\" does not exist"),
            )),
        };
    }
    // A MySQL server's own schemas are not a user's to drop, whatever that user holds on
    // the instance's own databases.
    if facts.wire == EngineWire::MySql
        && MYSQL_SYSTEM_DATABASES
            .iter()
            .any(|system| name.eq_ignore_ascii_case(system))
    {
        return Err(EngineError::new(
            EngineErrorKind::InsufficientPrivilege,
            format!(
                "Access denied for user '{}'@'{}' to database '{name}'",
                facts.user, facts.endpoint_host
            ),
        )
        // `ER_DBACCESS_DENIED_ERROR`.
        .with_mysql_error(1044, "42000"));
    }
    // PostgreSQL drops a database only from a session connected to another one.
    if facts.wire == EngineWire::Postgres && facts.database.eq_ignore_ascii_case(&name) {
        return Err(EngineError::new(
            EngineErrorKind::ObjectInUse,
            "cannot drop the currently open database".to_string(),
        ));
    }
    database.drop_database(&name);
    record_catalog_change(facts, "database-dropped", &name);
    let mut answer = StatementAnswer::new(format!("DROP {object}"), ResultSet::changed(0, 0));
    answer.databases_changed = true;
    // A MySQL session that drops the schema it was using is left with none selected.
    if facts.wire == EngineWire::MySql && facts.database.eq_ignore_ascii_case(&name) {
        answer.database_changed = Some(String::new());
    }
    Ok(answer)
}

/// Whether the instance holds the database `name`: one the engine has, or one the session
/// was admitted against.
#[must_use]
pub fn database_exists(database: &InstanceDatabase, facts: &SessionFacts, name: &str) -> bool {
    database.has_database(name)
        || facts
            .databases
            .iter()
            .any(|known| known.eq_ignore_ascii_case(name))
}

/// `BEGIN` / `START TRANSACTION`, `COMMIT` / `END`, `ROLLBACK` / `ABORT`: the transaction
/// control both wires define, and what each engine answers when the session is not in the
/// state the statement assumes.
///
/// `None` for a statement that is not one of them (`ROLLBACK TO SAVEPOINT`, a `BEGIN` that
/// opens a stored program on MySQL), which the engine runs itself.
///
/// Why the server answers these rather than the engine: with `autocommit` on — the default
/// on both engines — a `COMMIT` with no transaction open is a SUCCESS, which is what every
/// driver that commits after a statement batch depends on, and the embedded engine refuses
/// it. The transaction is the SESSION's ([`super::session_transactions`]), so what this
/// decides is whether this session's transaction is opened or ended.
pub fn transaction_control(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    words: &[String],
) -> Result<Option<StatementAnswer>, EngineError> {
    let first = words.first().map(String::as_str).unwrap_or_default();
    let second = words.get(1).map(String::as_str).unwrap_or_default();
    let session = facts.connection_id;
    let open = database.in_transaction(session);
    match (first, second) {
        ("BEGIN", "" | "WORK" | "TRANSACTION" | "ISOLATION" | "READ" | "DEFERRABLE" | "NOT")
        | ("START", "TRANSACTION") => {
            if open {
                return Ok(Some(match facts.wire {
                    // "Beginning a transaction causes any pending transaction to be
                    // committed", so the session is left in a new one.
                    EngineWire::MySql => {
                        database.commit_transaction(session, facts.serializable())?;
                        database.begin_transaction(session);
                        StatementAnswer::new("BEGIN", ResultSet::changed(0, 0))
                    }
                    // PostgreSQL warns and stays in the transaction it is in.
                    EngineWire::Postgres => {
                        StatementAnswer::new("BEGIN", ResultSet::changed(0, 0)).with_notice(
                            WireNotice::warning(
                                "25001",
                                "there is already a transaction in progress",
                            ),
                        )
                    }
                }));
            }
            database.begin_transaction(session);
            Ok(Some(StatementAnswer::new("BEGIN", ResultSet::changed(0, 0))))
        }
        ("COMMIT", "" | "WORK" | "TRANSACTION" | "AND")
        | ("END", "" | "WORK" | "TRANSACTION")
        | ("ROLLBACK", "" | "WORK" | "TRANSACTION" | "AND")
        | ("ABORT", "" | "WORK" | "TRANSACTION") => {
            // `END` and `ABORT` are PostgreSQL's spellings; on MySQL `END` closes a
            // compound statement, which is the engine's to read.
            if facts.wire == EngineWire::MySql && matches!(first, "END" | "ABORT") {
                return Ok(None);
            }
            let rollback = matches!(first, "ROLLBACK" | "ABORT");
            let verb = if rollback { "ROLLBACK" } else { "COMMIT" };
            // `AND CHAIN` ends this transaction and opens the next one immediately.
            let chain = second == "AND"
                && words.get(2).map(String::as_str).unwrap_or_default() == "CHAIN";
            if open {
                if rollback {
                    database.rollback_transaction(session)?;
                } else {
                    database.commit_transaction(session, facts.serializable())?;
                }
            }
            if chain {
                database.begin_transaction(session);
            }
            if open {
                return Ok(Some(StatementAnswer::new(verb, ResultSet::changed(0, 0))));
            }
            let answer = StatementAnswer::new(verb, ResultSet::changed(0, 0));
            Ok(Some(match facts.wire {
                // With autocommit on there is nothing to end, and the statement succeeds.
                EngineWire::MySql => answer,
                EngineWire::Postgres => answer.with_notice(WireNotice::warning(
                    "25P01",
                    "there is no transaction in progress",
                )),
            }))
        }
        _ => Ok(None),
    }
}

/// What a `CREATE`/`DROP` statement of a server object names, and the clauses that decide
/// how it is served.
struct ObjectStatement {
    /// The object the statement names, without the quotes a client wrote it in.
    name: String,
    /// Whether it carried `IF NOT EXISTS` / `IF EXISTS`.
    conditional: bool,
    /// Whether it carried `CASCADE`.
    cascade: bool,
    /// The version a `VERSION '<v>'` clause named.
    version: Option<String>,
}

/// Read a `CREATE <object> ...` / `DROP <object> ...` statement: the name it names and the
/// clauses above. `None` where it names nothing at all.
fn object_statement(tokens: &[Token]) -> Option<ObjectStatement> {
    let mut name: Option<String> = None;
    let mut conditional = false;
    let mut cascade = false;
    let mut version = None;
    let mut naming_version = false;
    // Everything after the two leading keywords (`CREATE EXTENSION`, `DROP DATABASE`).
    for token in tokens.iter().skip(2) {
        match token {
            Token::Word(word) => {
                let upper = word.value.to_ascii_uppercase();
                if word.quote_style.is_some() {
                    if name.is_none() {
                        name = Some(word.value.clone());
                    }
                    continue;
                }
                match upper.as_str() {
                    "IF" => conditional = true,
                    "NOT" | "EXISTS" => {}
                    "CASCADE" => cascade = true,
                    "VERSION" => naming_version = true,
                    _ if name.is_none() => name = Some(word.value.clone()),
                    // A trailing clause of its own (`WITH SCHEMA public`, a MySQL
                    // `DEFAULT CHARACTER SET utf8mb4`): the name is already read.
                    _ => {}
                }
            }
            Token::SingleQuotedString(value) | Token::DoubleQuotedString(value) => {
                if naming_version {
                    version = Some(value.clone());
                    naming_version = false;
                } else if name.is_none() {
                    name = Some(value.clone());
                }
            }
            _ => {}
        }
    }
    name.map(|name| ObjectStatement {
        name,
        conditional,
        cascade,
        version,
    })
}

/// A statement that ended before it said what it was about.
fn syntax_error_at_end() -> EngineError {
    EngineError::new(EngineErrorKind::Syntax, "syntax error at end of input")
}

/// The parse error the session's OWN engine answers for a statement its grammar does not
/// define, in place of the embedded engine's.
///
/// The embedded engine is SQLite: its sentence (`near "X": syntax error`) and its position
/// are its own, and a client branches on neither. What a client reads is its engine's —
/// MySQL's `ER_PARSE_ERROR`, which quotes the statement from where the server stopped
/// reading it, and PostgreSQL's `syntax error at or near "<token>"`. Only a statement the
/// engine could not READ is reworded (the failures that carry where they stopped); every
/// other refusal keeps the sentence it already had.
#[must_use]
pub fn as_the_engine_words_it(wire: EngineWire, sql: &str, error: EngineError) -> EngineError {
    if error.kind != EngineErrorKind::Syntax || error.offset.is_none() {
        return error;
    }
    let at = char_boundary(sql, error.offset.unwrap_or_default().min(sql.len()));
    let message = match wire {
        // The MySQL server quotes the statement from the point it stopped reading, and
        // names the line it stopped on.
        EngineWire::MySql => {
            let rest: String = sql
                .get(at..)
                .unwrap_or_default()
                .chars()
                .take(MYSQL_PARSE_ERROR_QUOTE_CHARS)
                .collect();
            let line = 1 + sql
                .get(..at)
                .unwrap_or_default()
                .matches('\n')
                .count();
            mysql_parse_error(&rest, line)
        }
        EngineWire::Postgres => match offending_token(&error.message, sql, at) {
            Some(token) => format!("syntax error at or near \"{token}\""),
            None => "syntax error at end of input".to_string(),
        },
    };
    EngineError::new(error.kind, message).at_offset(error.offset)
}

/// How many characters of the statement a MySQL parse error quotes.
///
/// The server's own limit on the text it echoes back, which is what keeps the message a
/// message rather than a copy of the statement.
pub const MYSQL_PARSE_ERROR_QUOTE_CHARS: usize = 80;

/// `ER_PARSE_ERROR` as the MySQL server words it: the statement from the point the server
/// stopped reading it, and the line it stopped on.
///
/// The ONE place that sentence is written, so every statement this endpoint could not read
/// — one the embedded engine refused ([`as_the_engine_words_it`]) and one a server-side
/// grammar of this crate stopped on ([`super::mysql_s3_load`]) — is reported in the same
/// words a client branches on.
#[must_use]
pub fn mysql_parse_error(rest: &str, line: usize) -> String {
    format!(
        "You have an error in your SQL syntax; check the manual that corresponds to your \
         MySQL server version for the right syntax to use near '{rest}' at line {line}"
    )
}

/// The token the statement stopped on: the one the engine named in its own message, else
/// the word at the position it stopped at.
fn offending_token(message: &str, sql: &str, at: usize) -> Option<String> {
    let quoted = message
        .split('"')
        .nth(1)
        .filter(|token| !token.is_empty())
        .map(str::to_string);
    quoted.or_else(|| {
        let word: String = sql
            .get(at..)
            .unwrap_or_default()
            .chars()
            .take_while(|character| character.is_alphanumeric() || *character == '_')
            .collect();
        (!word.is_empty()).then_some(word)
    })
}

/// The largest character boundary of `sql` at or before `at`, so a position the engine
/// reported in bytes can be used to cut the statement's text.
fn char_boundary(sql: &str, at: usize) -> usize {
    if sql.is_char_boundary(at) {
        return at;
    }
    (0..at)
        .rev()
        .find(|candidate| sql.is_char_boundary(*candidate))
        .unwrap_or_default()
}
