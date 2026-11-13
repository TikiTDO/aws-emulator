// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SERVER SURFACE each engine endpoint answers from this
//! service's own state: a PostgreSQL session's configuration parameters, system catalogs
//! and server functions, and the dialect each engine's own statements are written in.
//!
//! The rules under test:
//!
//! * a PostgreSQL DB instance answers for a configuration parameter — through `SHOW`,
//!   `current_setting()` and `pg_settings` — with the value the instance's effective DB
//!   parameter group holds for it, and with the engine default its family publishes where
//!   the group sets none; a change that waits for a reboot is not in force until the
//!   instance has had one;
//! * it serves the system catalogs from its own state: the databases it holds, the roles
//!   it has, and the sessions it is serving;
//! * it serves the server functions a client identifies the server with
//!   (`pg_postmaster_start_time()`, `pg_is_in_recovery()`, `pg_sleep()`), each answering
//!   from the instance's own lifecycle;
//! * each endpoint parses ITS OWN engine's dialect: PostgreSQL's `::` cast,
//!   `pg_catalog`-qualified names, role DDL and transaction modes; MySQL's table options,
//!   auto-increment columns, system variables, `SHOW GRANTS` and account identity.
//!
//! Every table below spans what its rule generalises over — the parameter's source (group
//! or engine default), its apply class (dynamic or static), the catalog read, the function,
//! and the statement form — rather than one recorded statement.

use crate::db_instance_engine_endpoint::{
    available_instance, mysql_login, parameter_group, postgres_login, PostgresSession, Rows,
    Transport,
};
use crate::db_instances::{call, emulator, expect_ok, text, wait_for_settle};

/// The master user every instance here is created with, and its test password — the only
/// place either appears.
const MASTER_USER: &str = "emuadmin";
const MASTER_PASSWORD: &str = "emu-Secret-99";
/// The `DBName` every instance here is created with.
const DATABASE: &str = "appdb";

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// Open a session as the master user on a PostgreSQL instance's published endpoint.
async fn session(address: &str, port: u16) -> PostgresSession {
    postgres_login(
        address,
        port,
        Transport::Tls,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await
    .expect("the master user must be admitted")
}

/// The one value a statement answered with, or the refusal's message.
async fn value(session: &mut PostgresSession, sql: &str) -> String {
    match session.query(sql).await {
        Ok(rows) => rows.only().unwrap_or_default().to_string(),
        Err(refusal) => panic!("{sql:?} must run, got {refusal:?}"),
    }
}

/// The rows a statement answered with.
async fn rows(session: &mut PostgresSession, sql: &str) -> Rows {
    session
        .query(sql)
        .await
        .unwrap_or_else(|refusal| panic!("{sql:?} must run, got {refusal:?}"))
}

#[tokio::test]
async fn a_postgresql_endpoint_answers_for_every_parameter_its_family_publishes() {
    let router = emulator();
    // The group sets a DYNAMIC parameter — "changes to dynamic parameters are applied
    // immediately" — so what the engine reports for it is the group's value, while every
    // parameter the group does not set reports the engine default its family publishes.
    parameter_group(
        &router,
        "emu-guc-group",
        "postgres16",
        "log_statement",
        "all",
    )
    .await;
    let (address, port) = available_instance(
        &router,
        "emu-guc-postgres",
        "postgres",
        &params!["EngineVersion" => "16.4", "DBParameterGroupName" => "emu-guc-group"],
    )
    .await;
    let mut session = session(&address, port).await;

    // The table spans the two sources a value can come from and the three ways a client
    // reads one. The engine defaults are the ones the family publishes
    // (`DescribeEngineDefaultParameters` reports the same catalog), and `max_connections`
    // is a parameter whose published RDS default is an instance-class formula, so the
    // engine's own compiled default stands for it.
    for (case, parameter, expected) in [
        ("a dynamic parameter the group sets", "log_statement", "all"),
        ("an engine default the group leaves alone", "statement_timeout", "0"),
        (
            "an engine default that is an enumeration",
            "default_transaction_isolation",
            "read committed",
        ),
        (
            "a parameter whose published default is an instance-class formula",
            "max_connections",
            "100",
        ),
        ("a parameter the server itself answers for", "server_encoding", "UTF8"),
    ] {
        let shown = value(&mut session, &format!("SHOW {parameter}")).await;
        assert_eq!(
            shown, expected,
            "case {case:?}: SHOW {parameter} must report the effective value, got {shown:?}"
        );
        let through_function = value(
            &mut session,
            &format!("SELECT current_setting('{parameter}')"),
        )
        .await;
        assert_eq!(
            through_function, expected,
            "case {case:?}: current_setting('{parameter}') must agree with SHOW, got \
             {through_function:?}"
        );
        let catalogued = session
            .query_with_params("SELECT setting FROM pg_settings WHERE name = $1", &[parameter])
            .await
            .unwrap_or_else(|refusal| panic!("case {case:?}: pg_settings must be readable, got {refusal:?}"));
        assert_eq!(
            catalogued.only(),
            Some(expected),
            "case {case:?}: pg_settings must agree with SHOW, got {catalogued:?}"
        );
    }

    // A parameter no engine has is not a parameter this server answers for either.
    let unknown = session
        .query("SHOW emu_not_a_parameter")
        .await
        .expect_err("a parameter the server does not have must be refused");
    assert_eq!(
        unknown.message, "unrecognized configuration parameter \"emu_not_a_parameter\"",
        "the refusal must be PostgreSQL's own sentence, got {unknown:?}"
    );

    // `pg_catalog` is on every session's search path, so the qualified name reads the same
    // relation as the bare one.
    let qualified = value(
        &mut session,
        "SELECT setting FROM pg_catalog.pg_settings WHERE name = 'log_statement'",
    )
    .await;
    assert_eq!(
        qualified, "all",
        "a pg_catalog-qualified catalog must read the same relation, got {qualified:?}"
    );
}

#[tokio::test]
async fn a_static_parameter_change_takes_effect_only_once_the_instance_has_rebooted() {
    let router = emulator();
    parameter_group(
        &router,
        "emu-static-group",
        "postgres16",
        "log_statement",
        "none",
    )
    .await;
    let (address, port) = available_instance(
        &router,
        "emu-static-postgres",
        "postgres",
        &params!["EngineVersion" => "16.4", "DBParameterGroupName" => "emu-static-group"],
    )
    .await;

    // `max_connections` is published as a STATIC parameter: "changes to static parameters
    // require a reboot … before the change can take effect".
    expect_ok(
        "set the static parameter",
        call(
            &router,
            "ModifyDBParameterGroup",
            &params![
                "DBParameterGroupName" => "emu-static-group",
                "Parameters.Parameter.1.ParameterName" => "max_connections",
                "Parameters.Parameter.1.ParameterValue" => 70,
                "Parameters.Parameter.1.ApplyMethod" => "pending-reboot",
            ],
        )
        .await,
    );
    let mut before = session(&address, port).await;
    let running = value(&mut before, "SHOW max_connections").await;
    assert_eq!(
        running, "100",
        "a change waiting for a reboot must not be in force yet, got {running:?}"
    );
    let pending = value(
        &mut before,
        "SELECT pending_restart FROM pg_settings WHERE name = 'max_connections'",
    )
    .await;
    assert_eq!(
        pending, "t",
        "the parameter must report that a restart is pending, got {pending:?}"
    );
    let reset = value(
        &mut before,
        "SELECT reset_val FROM pg_settings WHERE name = 'max_connections'",
    )
    .await;
    assert_eq!(
        reset, "70",
        "the value the restart will install must be reported, got {reset:?}"
    );

    expect_ok(
        "reboot",
        call(
            &router,
            "RebootDBInstance",
            &params!["DBInstanceIdentifier" => "emu-static-postgres"],
        )
        .await,
    );
    wait_for_settle();
    let mut after = session(&address, port).await;
    let running = value(&mut after, "SHOW max_connections").await;
    assert_eq!(
        running, "70",
        "the reboot must put the change in force, got {running:?}"
    );
    let pending = value(
        &mut after,
        "SELECT pending_restart FROM pg_settings WHERE name = 'max_connections'",
    )
    .await;
    assert_eq!(
        pending, "f",
        "nothing is pending once the restart has happened, got {pending:?}"
    );
}

#[tokio::test]
async fn a_postgresql_endpoint_serves_its_catalogs_and_its_server_functions() {
    let router = emulator();
    let (address, port) = available_instance(
        &router,
        "emu-catalog-postgres",
        "postgres",
        &params!["EngineVersion" => "16.4"],
    )
    .await;
    let mut session = session(&address, port).await;

    // The catalogs, each read for a fact this service has state for: the databases the
    // instance holds, the master user's own role attributes, and the sessions open now.
    let databases = rows(&mut session, "SELECT datname FROM pg_database ORDER BY datname").await;
    assert_eq!(
        databases.rows,
        vec![
            vec![Some(DATABASE.to_string())],
            vec![Some("postgres".to_string())]
        ],
        "pg_database must list exactly the databases the instance holds, got {databases:?}"
    );
    let attributes = session
        .query_with_params(
            "SELECT rolsuper, rolinherit, rolcreaterole, rolcreatedb, rolcanlogin \
             FROM pg_roles WHERE rolname = $1",
            &[MASTER_USER],
        )
        .await
        .expect("pg_roles must be readable");
    assert_eq!(
        attributes.rows,
        vec![vec![
            Some("f".to_string()),
            Some("t".to_string()),
            Some("t".to_string()),
            Some("t".to_string()),
            Some("t".to_string())
        ]],
        "the master user must be no superuser, must be able to log in and must carry \
         CREATEROLE and CREATEDB, got {attributes:?}"
    );
    let superusers = value(
        &mut session,
        "SELECT count(*) FROM pg_roles WHERE rolsuper AND rolname = 'rdsadmin'",
    )
    .await;
    assert_eq!(
        superusers, "1",
        "the one superuser account Amazon RDS keeps must be listed, got {superusers:?}"
    );
    let own_backend = rows(
        &mut session,
        "SELECT usename, datname, state FROM pg_stat_activity WHERE pid = pg_backend_pid()",
    )
    .await;
    assert_eq!(
        own_backend.rows,
        vec![vec![
            Some(MASTER_USER.to_string()),
            Some(DATABASE.to_string()),
            Some("active".to_string())
        ]],
        "a session must find itself in pg_stat_activity, got {own_backend:?}"
    );
    // `pg_user` is the view of the roles that can log in, and the master user is one.
    let user = rows(
        &mut session,
        "SELECT usename, usesuper, passwd FROM pg_user WHERE usename = current_user",
    )
    .await;
    assert_eq!(
        user.rows,
        vec![vec![
            Some(MASTER_USER.to_string()),
            Some("f".to_string()),
            Some("********".to_string())
        ]],
        "pg_user must report the session's own account, got {user:?}"
    );
    let mut second = postgres_login(
        &address,
        port,
        Transport::Tls,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await
    .expect("a second session must be admitted");
    let _ = value(&mut second, "SELECT 1").await;
    let open = value(&mut session, "SELECT count(*) FROM pg_stat_activity").await;
    assert_eq!(
        open, "2",
        "both open sessions must be listed, got {open:?}"
    );

    // The server functions, each answering from the instance's own lifecycle.
    let writer = value(&mut session, "SELECT pg_is_in_recovery()").await;
    assert_eq!(
        writer, "f",
        "an instance with no replication source is a writer, got {writer:?}"
    );
    let started = value(&mut session, "SELECT pg_postmaster_start_time()").await;
    assert!(
        started.len() >= "2026-01-01 00:00:00".len() && started.ends_with("+00"),
        "the start time must be a timestamp with the server's own offset, got {started:?}"
    );
    let created = text(
        &expect_ok(
            "describe the instance",
            call(
                &router,
                "DescribeDBInstances",
                &params!["DBInstanceIdentifier" => "emu-catalog-postgres"],
            )
            .await,
        ),
        "InstanceCreateTime",
    );
    assert_eq!(
        started.get(..4),
        created.get(..4),
        "the engine's start time must be the instance's own, got {started:?} against \
         {created:?}"
    );
    // `pg_sleep` really waits, and it waits for as long as it was asked to.
    let before = std::time::Instant::now();
    let _ = value(&mut session, "SELECT pg_sleep(0.2)").await;
    assert!(
        before.elapsed() >= std::time::Duration::from_millis(150),
        "pg_sleep must wait, got {:?}",
        before.elapsed()
    );
}

#[tokio::test]
async fn a_statement_past_the_sessions_statement_timeout_is_cancelled() {
    let router = emulator();
    let (address, port) = available_instance(
        &router,
        "emu-timeout-postgres",
        "postgres",
        &params!["EngineVersion" => "16.4"],
    )
    .await;
    let mut session = session(&address, port).await;
    // "The time in milliseconds after which a statement is aborted; 0 disables the
    // timeout" — so the table spans the parameter's two classes: disabled, and a value a
    // statement runs past.
    for (case, timeout, sleep, cancelled) in [
        ("the timeout disabled", "0", "0.1", false),
        ("a statement well inside the timeout", "5000", "0.1", false),
        ("a statement past the timeout", "100", "3", true),
    ] {
        let _ = value(&mut session, &format!("SET statement_timeout = {timeout}")).await;
        let answer = session.query(&format!("SELECT pg_sleep({sleep})")).await;
        match (cancelled, answer) {
            (false, Ok(_)) => {}
            (true, Err(refusal)) => {
                assert_eq!(
                    (refusal.code.as_str(), refusal.message.as_str()),
                    (
                        "57014",
                        "canceling statement due to statement timeout"
                    ),
                    "case {case:?}: the cancellation must be PostgreSQL's own, got {refusal:?}"
                );
            }
            (expected, got) => panic!(
                "case {case:?}: expected a cancellation = {expected}, got {got:?}"
            ),
        }
    }
}

/// RULE: every piece of caller SQL an engine endpoint runs for one statement is bounded by
/// THAT statement's deadline — the session's `statement_timeout` clamped by the endpoint's
/// own budget — including the row query the endpoint runs to identify the rows a locking
/// statement will take. So a server function that WAITS, called from such a statement's
/// predicate, stops at the statement's deadline and never at the number the caller asked
/// for.
///
/// The rows span the statement forms whose predicate the endpoint evaluates ahead of the
/// statement itself: a locking `SELECT`, an `UPDATE` and a `DELETE`. Each is refused
/// promptly, and with the engine's OWN cancellation (SQLSTATE `57014`) whatever clause it
/// carried: a statement whose rows the endpoint could not identify inside the budget goes
/// on to run and is cancelled by the same deadline, so the client reads the condition the
/// engine raised rather than a limit of the endpoint. In no case does the endpoint wait for
/// the number the predicate asked for.
#[tokio::test]
async fn a_waiting_function_in_a_locking_statements_predicate_stops_at_the_deadline() {
    let router = emulator();
    let (address, port) = available_instance(
        &router,
        "emu-rowquery-postgres",
        "postgres",
        &params!["EngineVersion" => "16.4"],
    )
    .await;
    let mut session = session(&address, port).await;
    let _ = value(
        &mut session,
        "CREATE TABLE waits (id int PRIMARY KEY, label text)",
    )
    .await;
    let _ = value(&mut session, "INSERT INTO waits (id, label) VALUES (1, 'a')").await;
    // "The time in milliseconds after which a statement is aborted."
    let _ = value(&mut session, "SET statement_timeout = 100").await;

    // The number the predicate asks to wait for, far past the timeout above.
    let asked = std::time::Duration::from_secs(3);
    // (case, the statement whose predicate waits, the refusal the endpoint answers with)
    let cases = [
        (
            "a locking SELECT",
            "SELECT id FROM waits WHERE pg_sleep(3) IS NULL FOR UPDATE",
            "57014",
        ),
        (
            "an UPDATE",
            "UPDATE waits SET label = 'b' WHERE pg_sleep(3) IS NULL",
            "57014",
        ),
        (
            "a DELETE",
            "DELETE FROM waits WHERE pg_sleep(3) IS NULL",
            "57014",
        ),
    ];

    for (case, statement, code) in cases {
        let before = std::time::Instant::now();
        let answer = session.query(statement).await;
        let elapsed = before.elapsed();
        let refusal = answer.err().unwrap_or_else(|| {
            panic!("case {case:?}: {statement:?} must not be served, it was")
        });
        assert_eq!(
            refusal.code, code,
            "case {case:?}: expected SQLSTATE {code}, got {refusal:?}"
        );
        assert!(
            elapsed < asked,
            "case {case:?}: the statement must stop at its own deadline, not after the \
             {asked:?} its predicate asked for, got {elapsed:?}"
        );
    }
}

#[tokio::test]
async fn a_postgresql_endpoint_parses_the_postgresql_dialect() {
    let router = emulator();
    let (address, port) = available_instance(
        &router,
        "emu-dialect-postgres",
        "postgres",
        &params!["EngineVersion" => "16.4"],
    )
    .await;
    let mut session = session(&address, port).await;

    // The cast operator, over the forms its operand takes: a literal, a parenthesised
    // expression, a function call and a qualified column.
    let _ = value(
        &mut session,
        "CREATE TABLE readings (id int PRIMARY KEY, label text)",
    )
    .await;
    let _ = value(&mut session, "INSERT INTO readings VALUES (4, '7')").await;
    for (case, expression, expected) in [
        ("a numeric literal", "SELECT 3::numeric", "3"),
        // A cast to `numeric` with a declared scale carries that scale: "the value will be
        // rounded to the specified number of fractional digits", and those digits are what
        // the value then has.
        ("a precision and scale", "SELECT 2::numeric(10,2)", "2.00"),
        ("a parenthesised expression", "SELECT (1 + 2)::int", "3"),
        ("a function call", "SELECT length('abcd')::text", "4"),
        (
            "a qualified column",
            "SELECT readings.label::int FROM readings",
            "7",
        ),
    ] {
        let got = value(&mut session, expression).await;
        assert_eq!(
            got, expected,
            "case {case:?}: {expression:?} must answer {expected:?}, got {got:?}"
        );
    }

    // Role DDL is server state, kept in the server's own role catalog.
    for (case, statement, role, can_login) in [
        ("a role with LOGIN", "CREATE ROLE \"emu_app\" WITH LOGIN", "emu_app", "t"),
        ("a role without it", "CREATE ROLE emu_reader", "emu_reader", "f"),
        ("a user, which is a role with LOGIN", "CREATE USER emu_user", "emu_user", "t"),
    ] {
        let _ = value(&mut session, statement).await;
        let listed = session
            .query_with_params(
                "SELECT rolcanlogin FROM pg_roles WHERE rolname = $1",
                &[role],
            )
            .await
            .expect("the role catalog must be readable");
        assert_eq!(
            listed.only(),
            Some(can_login),
            "case {case:?}: the created role must be in pg_roles, got {listed:?}"
        );
    }
    let again = session
        .query("CREATE ROLE emu_app")
        .await
        .expect_err("a role that is already there must be refused");
    assert_eq!(
        again.message, "role \"emu_app\" already exists",
        "the refusal must be PostgreSQL's own sentence, got {again:?}"
    );
    let missing = session
        .query("DROP ROLE emu_absent")
        .await
        .expect_err("a role the server does not have must be refused");
    assert_eq!(
        missing.message, "role \"emu_absent\" does not exist",
        "the refusal must be PostgreSQL's own sentence, got {missing:?}"
    );
    // The conditional forms: "Do not throw an error if the role does not exist", and the
    // same for a create of one that is already there.
    for (case, statement) in [
        ("a drop of a role that is not there", "DROP ROLE IF EXISTS emu_absent"),
        ("a create of a role that is", "CREATE ROLE IF NOT EXISTS emu_app"),
    ] {
        session
            .query(statement)
            .await
            .unwrap_or_else(|refusal| panic!("case {case:?}: {statement:?} must be a no-op, got {refusal:?}"));
    }
    let still_there = session
        .query_with_params(
            "SELECT rolcanlogin FROM pg_roles WHERE rolname = $1",
            &["emu_app"],
        )
        .await
        .expect("the role catalog must be readable");
    assert_eq!(
        still_there.only(),
        Some("t"),
        "a conditional create must leave the role it found as it was, got {still_there:?}"
    );

    // A bulk load through the copy-in protocol, over the rows the format spans: plain
    // values, a NULL, an escaped separator — and the two ways a row does not fit the
    // table, each refused with PostgreSQL's own DATA error rather than a syntax error.
    let _ = value(
        &mut session,
        "CREATE TABLE loaded (id int PRIMARY KEY, note text)",
    )
    .await;
    let loaded = session
        .copy_in(
            "COPY loaded (id, note) FROM STDIN WITH (FORMAT text)",
            "1\tfirst\n2\t\\N\n3\ttab\\there\n",
        )
        .await
        .expect("a well-formed bulk load must be accepted");
    assert_eq!(
        loaded.rows,
        Vec::<Vec<Option<String>>>::new(),
        "a copy answers with no rows, got {loaded:?}"
    );
    // A last row the client did not terminate with a newline is still a row.
    session
        .copy_in(
            "COPY loaded (id, note) FROM STDIN WITH (FORMAT text)",
            "4\tunterminated",
        )
        .await
        .expect("a load whose last row carries no newline must be accepted");
    let read_back = rows(&mut session, "SELECT id, note FROM loaded ORDER BY id").await;
    assert_eq!(
        read_back.rows,
        vec![
            vec![Some("1".to_string()), Some("first".to_string())],
            vec![Some("2".to_string()), None],
            vec![Some("3".to_string()), Some("tab\there".to_string())],
            vec![Some("4".to_string()), Some("unterminated".to_string())],
        ],
        "the loaded rows must be the rows the client streamed, got {read_back:?}"
    );
    // A load the client sent INSIDE its own transaction belongs to that transaction: the
    // rows are there for it, and its own rollback is what takes them away.
    let _ = value(&mut session, "BEGIN").await;
    session
        .copy_in(
            "COPY loaded (id, note) FROM STDIN WITH (FORMAT text)",
            "5\tinside a transaction\n",
        )
        .await
        .expect("a load inside the client's own transaction must be accepted");
    let inside = rows(&mut session, "SELECT count(*) FROM loaded").await;
    assert_eq!(
        inside.only(),
        Some("5"),
        "the loaded row must be visible inside the transaction that loaded it, got \
         {inside:?}"
    );
    let _ = value(&mut session, "ROLLBACK").await;
    let after = rows(&mut session, "SELECT count(*) FROM loaded").await;
    assert_eq!(
        after.only(),
        Some("4"),
        "the client's rollback must take its loaded rows with it, got {after:?}"
    );
    for (case, data, expected) in [
        (
            "a value the column's type cannot read",
            "6\tfine\nnot-a-number\talso fine\n",
            "invalid input syntax for type integer: \"not-a-number\"",
        ),
        (
            "a row with a column too many",
            "7\tfine\textra\n",
            "extra data after last expected column",
        ),
        (
            "a row with a column too few",
            "8\n",
            "missing data for column \"note\"",
        ),
    ] {
        let refusal = session
            .copy_in("COPY loaded (id, note) FROM STDIN WITH (FORMAT text)", data)
            .await
            .expect_err(&format!("case {case:?}: the malformed row must be refused"));
        assert_eq!(
            refusal.message, expected,
            "case {case:?}: the refusal must be PostgreSQL's own data error, got {refusal:?}"
        );
        // "COPY stops operation at the first error": the rows before the bad one are
        // rolled back with it, so the table is untouched.
        let still = rows(&mut session, "SELECT count(*) FROM loaded").await;
        assert_eq!(
            still.only(),
            Some("4"),
            "case {case:?}: a refused load must leave no rows behind, got {still:?}"
        );
    }

    // A transaction's isolation level belongs to that transaction: the level a `BEGIN`
    // names is what the session reports while it is open, a transaction that names none
    // runs at `default_transaction_isolation`, and ending the transaction puts that
    // default back. The table spans both spellings of the opening statement, a level named
    // and not named, and the state between transactions.
    let between = value(&mut session, "SHOW transaction_isolation").await;
    assert_eq!(
        between, "read committed",
        "with no transaction open the session must report its default level, got \
         {between:?}"
    );
    for (case, statement, reported) in [
        (
            "repeatable read",
            "BEGIN ISOLATION LEVEL REPEATABLE READ",
            "repeatable read",
        ),
        (
            "serializable, through START TRANSACTION",
            "START TRANSACTION ISOLATION LEVEL SERIALIZABLE",
            "serializable",
        ),
        (
            "a read-only transaction, which names no level",
            "BEGIN READ ONLY",
            "read committed",
        ),
        (
            "read uncommitted, which PostgreSQL runs as read committed",
            "BEGIN ISOLATION LEVEL READ UNCOMMITTED",
            "read uncommitted",
        ),
    ] {
        let _ = value(&mut session, statement).await;
        let level = value(&mut session, "SHOW transaction_isolation").await;
        assert_eq!(
            level, reported,
            "case {case:?}: the session must report the level the transaction runs at, got \
             {level:?}"
        );
        let _ = value(&mut session, "COMMIT").await;
        let after = value(&mut session, "SHOW transaction_isolation").await;
        assert_eq!(
            after, "read committed",
            "case {case:?}: the level must not outlive the transaction, got {after:?}"
        );
    }
}

#[tokio::test]
async fn a_mysql_endpoint_parses_the_mysql_dialect_and_reports_an_account_identity() {
    let router = emulator();
    let (address, port) = available_instance(
        &router,
        "emu-dialect-mysql",
        "mysql",
        &params!["EngineVersion" => "8.0.39"],
    )
    .await;
    let mut session = mysql_login(
        &address,
        port,
        Transport::Plaintext,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await
    .expect("the master user must be admitted");

    // A table declared with MySQL's own table options and an auto-increment column: the
    // table is created and the column is assigned values.
    for statement in [
        "CREATE TABLE gauges (id INT AUTO_INCREMENT PRIMARY KEY, label VARCHAR(32)) \
         ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci",
        "INSERT INTO gauges (label) VALUES ('pressure')",
        "INSERT INTO gauges (label) VALUES ('flow')",
    ] {
        session
            .query(statement)
            .await
            .unwrap_or_else(|refusal| panic!("{statement:?} must run, got {refusal:?}"));
    }
    let assigned = session
        .query("SELECT id, label FROM gauges ORDER BY id")
        .await
        .expect("the read-back runs");
    assert_eq!(
        assigned.rows,
        vec![
            vec![Some("1".to_string()), Some("pressure".to_string())],
            vec![Some("2".to_string()), Some("flow".to_string())]
        ],
        "an auto-increment column must be assigned values, got {assigned:?}"
    );

    // A table created FROM A QUERY: what follows the table name is the query, not table
    // options, so the whole of it has to reach the engine — a create that dropped the
    // query's `FROM` or its `WHERE` would report a table built from something else.
    for (case, statement, expected) in [
        (
            "a create-table-as-select over an aggregate",
            "CREATE TABLE counted AS SELECT count(*) AS rows_held FROM gauges",
            vec![vec![Some("2".to_string())]],
        ),
        (
            "a create-table-as-select with a predicate and an order",
            "CREATE TABLE filtered AS SELECT label FROM gauges WHERE id IN (1, 2) \
             ORDER BY label",
            vec![
                vec![Some("flow".to_string())],
                vec![Some("pressure".to_string())],
            ],
        ),
    ] {
        session
            .query(statement)
            .await
            .unwrap_or_else(|refusal| panic!("case {case:?}: {statement:?} must run, got {refusal:?}"));
        let table = statement
            .split_whitespace()
            .nth(2)
            .expect("the statement names its table");
        let held = session
            .query(&format!("SELECT * FROM {table}"))
            .await
            .unwrap_or_else(|refusal| panic!("case {case:?}: the read-back must run, got {refusal:?}"));
        assert_eq!(
            held.rows, expected,
            "case {case:?}: the table must hold what the client's own query selected, got \
             {held:?}"
        );
    }

    // A system variable, in each of the scopes a client may name it with.
    for (case, statement) in [
        ("no scope", "SELECT @@require_secure_transport"),
        ("the session scope", "SELECT @@session.require_secure_transport"),
        ("the global scope", "SELECT @@global.require_secure_transport"),
    ] {
        let read = session
            .query(statement)
            .await
            .unwrap_or_else(|refusal| panic!("case {case:?}: {statement:?} must run, got {refusal:?}"));
        assert_eq!(
            read.only(),
            Some("OFF"),
            "case {case:?}: the variable must report the instance's value, got {read:?}"
        );
    }

    // The session's identity is an ACCOUNT, and its grants are the master user's.
    let identity = session
        .query("SELECT CURRENT_USER()")
        .await
        .expect("the identity query runs");
    assert_eq!(
        identity.only(),
        Some(format!("{MASTER_USER}@%").as_str()),
        "CURRENT_USER() must report the account, got {identity:?}"
    );
    let grants = session
        .query(&format!("SHOW GRANTS FOR '{MASTER_USER}'@'%'"))
        .await
        .expect("SHOW GRANTS runs");
    let granted = grants.only().unwrap_or_default().to_string();
    assert!(
        granted.starts_with("GRANT SELECT, INSERT")
            && granted.ends_with(&format!("TO `{MASTER_USER}`@`%` WITH GRANT OPTION")),
        "the master user's grants must be listed as a GRANT statement, got {granted:?}"
    );
    assert!(
        !granted.contains("SUPER"),
        "the master user does not hold SUPER, got {granted:?}"
    );
    let unknown = session
        .query("SHOW GRANTS FOR 'nobody'@'%'")
        .await
        .expect_err("an account nothing granted anything to must be refused");
    assert_eq!(
        unknown.message, "There is no such grant defined for user 'nobody' on host '%'",
        "the refusal must be MySQL's own sentence, got {unknown:?}"
    );
}
