// SPDX-License-Identifier: CC0-1.0

//! What the Data API runs is AMAZON REDSHIFT's SQL: the dialect a Redshift
//! client writes, not the dialect of the engine behind this emulator.
//!
//! Every suite here states that one rule over a TABLE of statements rather than
//! one recorded request, and drives it over the wire:
//!
//! 1. a PostgreSQL-derived construct the engine spells differently runs and
//!    computes the same answer — the `::` cast operator, a schema-qualified
//!    name, `LOCK`;
//! 2. a statement about the database's own catalogue of SCHEMAS really changes
//!    that catalogue: a schema a statement creates holds tables, a schema that
//!    is already there is refused, and a dropped schema takes its tables with
//!    it;
//! 3. a Redshift function the engine does not carry is COMPUTED — the hash
//!    functions against their published test vectors, the string functions
//!    against the answers Redshift documents for them.
//!
//! The rows span the class rather than repeating one case: each cast row casts a
//! different kind of expression, each function row a different function, each
//! `LOCK` row a different form of the statement, and the hash rows every digest
//! width `SHA2` admits.

use serde_json::{json, Value};

use super::emulator::{Emulator, WAIT_SECONDS};

/// One statement and the single value it must produce.
struct Row {
    label: &'static str,
    sql: String,
    /// The one cell the statement's one row must carry, as `GetStatementResult`
    /// reports it.
    expected: Value,
}

/// Run `sql` against one database and answer the record the service reports for
/// it, with the statement already in its terminal state.
async fn run(emulator: &Emulator, sql: &str) -> Value {
    let submitted = emulator
        .execute_and_wait(&json!({
            "ClusterIdentifier": "my-cluster",
            "Database": "dev",
            "DbUser": "awsuser",
            "Sql": sql,
        }))
        .await;
    emulator
        .ok("DescribeStatement", &json!({"Id": submitted["Id"]}))
        .await
}

/// Run `sql`, require that it FINISHED, and answer the one cell of its one row.
async fn one_cell(emulator: &Emulator, label: &str, sql: &str) -> Value {
    let described = run(emulator, sql).await;
    assert_eq!(
        described["Status"], "FINISHED",
        "case {label}: {sql} must run on a Redshift-compatible engine, got {described}"
    );
    let fetched = emulator
        .ok("GetStatementResult", &json!({"Id": described["Id"]}))
        .await;
    let cell = fetched["Records"][0][0].clone();
    assert!(
        !cell.is_null(),
        "case {label}: {sql} must produce one row with one value, got {fetched}"
    );
    cell
}

/// A statement that must fail, and the message it must fail with.
async fn failure(emulator: &Emulator, label: &str, sql: &str) -> String {
    let described = run(emulator, sql).await;
    assert_eq!(
        described["Status"], "FAILED",
        "case {label}: {sql} must fail, got {described}"
    );
    described["Error"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label}: a failed statement reports why: {described}"))
        .to_string()
}

/// The `::` cast operator is the standard cast: it runs, and it casts.
///
/// The rows differ in WHAT is cast — a literal, an arithmetic expression, a
/// function's result, a column, a parenthesised expression nested inside another
/// function call — and in the type it is cast to, so the rule is asserted over
/// the class and not over the one expression a recording happened to carry.
#[tokio::test]
async fn the_cast_operator_runs_and_casts() {
    let emulator = Emulator::new();
    let rows = [
        Row {
            label: "an integer literal cast to a character type",
            sql: "SELECT 42::VARCHAR".to_string(),
            expected: json!({"stringValue": "42"}),
        },
        Row {
            label: "a character literal cast to an integer type",
            sql: "SELECT '7'::INTEGER + 1".to_string(),
            expected: json!({"longValue": 8}),
        },
        Row {
            label: "an arithmetic expression cast to a character type",
            sql: "SELECT ((1 - 1) * 300 + 2)::VARCHAR".to_string(),
            expected: json!({"stringValue": "2"}),
        },
        Row {
            label: "a function's result cast to a character type",
            sql: "SELECT LEFT(MD5('x')::VARCHAR, 4)".to_string(),
            expected: json!({"stringValue": "9dd4"}),
        },
        Row {
            label: "a cast of a column, inside a cast of its own",
            sql: "SELECT LEN(seq.n::VARCHAR::VARCHAR) FROM (SELECT 1234 AS n) AS seq".to_string(),
            expected: json!({"longValue": 4}),
        },
        Row {
            label: "a cast nested in the argument list of another function",
            sql: "WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < 3) \
                  SELECT LEN(LEFT(REPEAT(MD5(((a.n - 1) * 300 + b.n)::VARCHAR), 63), 2000)) FROM \
                  seq a CROSS JOIN seq b LIMIT 1"
                .to_string(),
            // 63 repetitions of a 32-character digest, cut to the 2000
            // characters the outer call asks for.
            expected: json!({"longValue": 2000}),
        },
        Row {
            label: "a cast to a floating-point type",
            sql: "SELECT '2.5'::DOUBLE PRECISION * 2".to_string(),
            expected: json!({"doubleValue": 5.0}),
        },
    ];

    for row in &rows {
        let cell = one_cell(&emulator, row.label, &row.sql).await;
        assert_eq!(
            cell, row.expected,
            "case {}: expected {}, got {cell}",
            row.label, row.expected
        );
    }
}

/// `LOCK` takes a lock and the statement FINISHES — in every form the statement
/// is written in — and a `LOCK` on a relation that is not there fails.
///
/// The rows differ in whether `TABLE` is spelled, in how many tables are locked,
/// in whether the table is schema-qualified, and in the lock mode named: every
/// mode the grammar declares, each with and without the keyword noise around it.
#[tokio::test]
async fn lock_runs_in_every_form_the_statement_is_written_in() {
    let emulator = Emulator::new();
    for ddl in [
        "CREATE TABLE locked (id INTEGER)",
        "CREATE TABLE also_locked (id INTEGER)",
        "CREATE SCHEMA warehouse",
        "CREATE TABLE warehouse.locked (id INTEGER)",
    ] {
        let described = run(&emulator, ddl).await;
        assert_eq!(
            described["Status"], "FINISHED",
            "{ddl} must finish before a lock can name what it builds, got {described}"
        );
    }

    // Every mode the `LOCK` grammar declares, which is the catalogue
    // `state::redshift_sql::LOCK_MODES` names.
    let modes = [
        "ACCESS SHARE",
        "ROW SHARE",
        "ROW EXCLUSIVE",
        "SHARE UPDATE EXCLUSIVE",
        "SHARE",
        "SHARE ROW EXCLUSIVE",
        "EXCLUSIVE",
        "ACCESS EXCLUSIVE",
    ];
    let mut forms: Vec<String> = vec![
        "LOCK locked".to_string(),
        "LOCK TABLE locked".to_string(),
        " LOCK TABLE locked ;".to_string(),
        "LOCK TABLE locked, also_locked".to_string(),
        "LOCK TABLE warehouse.locked".to_string(),
        "LOCK TABLE public.locked".to_string(),
        "LOCK TABLE locked IN ACCESS EXCLUSIVE MODE NOWAIT".to_string(),
    ];
    forms.extend(
        modes
            .iter()
            .map(|mode| format!("LOCK TABLE locked IN {mode} MODE;")),
    );

    for form in &forms {
        let described = run(&emulator, form).await;
        assert_eq!(
            described["Status"], "FINISHED",
            "case {form:?}: a lock Redshift accepts must finish, got {described}"
        );
        assert!(
            described["HasResultSet"].as_bool() != Some(true),
            "case {form:?}: a lock produces no result set, got {described}"
        );
    }

    let message = failure(&emulator, "a lock on a relation that is not there", "LOCK TABLE absent")
        .await;
    assert!(
        message.contains("absent"),
        "a lock must fail naming the relation it could not resolve, got {message:?}"
    );
}

/// A schema a statement creates is a real object of the database: tables live in
/// it, it cannot be created twice, and dropping it takes its tables with it.
#[tokio::test]
async fn a_created_schema_holds_tables_until_it_is_dropped() {
    let emulator = Emulator::new();

    for sql in [
        "CREATE SCHEMA sales",
        "CREATE TABLE sales.orders (id INTEGER, total INTEGER)",
        "INSERT INTO sales.orders VALUES (1, 10), (2, 20)",
    ] {
        let described = run(&emulator, sql).await;
        assert_eq!(
            described["Status"], "FINISHED",
            "{sql} must finish, got {described}"
        );
    }

    // The rows are really in the schema's table, reachable by its qualified name
    // and by a qualified column reference.
    let total = one_cell(
        &emulator,
        "a schema's table is read back through its qualified name",
        "SELECT sum(sales.orders.total) FROM sales.orders",
    )
    .await;
    assert_eq!(
        total,
        json!({"longValue": 30}),
        "the rows inserted into the schema's table must be the rows read back, got {total}"
    );

    // Unqualified, the same name is a table of the search path's own schema,
    // which holds no such table.
    let message = failure(
        &emulator,
        "an unqualified name does not reach another schema's table",
        "SELECT * FROM orders",
    )
    .await;
    assert!(
        message.contains("orders"),
        "the engine must report the relation it could not resolve, got {message:?}"
    );

    // A schema that is already there.
    let message = failure(
        &emulator,
        "a schema that already exists",
        "CREATE SCHEMA sales",
    )
    .await;
    assert_eq!(
        message, "ERROR: schema \"sales\" already exists",
        "expected the message a data warehouse refuses a duplicate schema with, got {message:?}"
    );
    let described = run(&emulator, "CREATE SCHEMA IF NOT EXISTS sales").await;
    assert_eq!(
        described["Status"], "FINISHED",
        "IF NOT EXISTS makes no change and no error, got {described}"
    );

    // RESTRICT — the default — refuses a schema that still holds objects.
    let message = failure(
        &emulator,
        "a schema that still holds a table",
        "DROP SCHEMA sales",
    )
    .await;
    assert_eq!(
        message, "ERROR: cannot drop schema sales because other objects depend on it",
        "expected the message a data warehouse refuses a non-empty drop with, got {message:?}"
    );

    // Several schemas are dropped by one statement, each with its objects.
    for sql in [
        "CREATE SCHEMA audit",
        "CREATE SCHEMA staging",
        "CREATE TABLE staging.incoming (id INTEGER)",
    ] {
        let described = run(&emulator, sql).await;
        assert_eq!(
            described["Status"], "FINISHED",
            "{sql} must finish, got {described}"
        );
    }
    let described = run(&emulator, "DROP SCHEMA audit, staging CASCADE").await;
    assert_eq!(
        described["Status"], "FINISHED",
        "a drop may name several schemas, got {described}"
    );
    let message = failure(
        &emulator,
        "a table of one of the dropped schemas",
        "SELECT * FROM staging.incoming",
    )
    .await;
    assert!(
        message.contains("staging"),
        "each dropped schema's tables go with it, got {message:?}"
    );

    // CASCADE drops the schema and the tables it holds.
    let described = run(&emulator, "DROP SCHEMA sales CASCADE").await;
    assert_eq!(
        described["Status"], "FINISHED",
        "CASCADE drops the schema and its objects, got {described}"
    );
    let message = failure(
        &emulator,
        "a table of a dropped schema",
        "SELECT * FROM sales.orders",
    )
    .await;
    assert!(
        message.contains("sales"),
        "the schema's table must be gone with it, got {message:?}"
    );

    // And a schema that was never there.
    let message = failure(
        &emulator,
        "a schema that is not there",
        "DROP SCHEMA sales RESTRICT",
    )
    .await;
    assert_eq!(
        message, "ERROR: schema \"sales\" does not exist",
        "expected the message a data warehouse refuses an absent drop with, got {message:?}"
    );
    let described = run(&emulator, "DROP SCHEMA IF EXISTS sales").await;
    assert_eq!(
        described["Status"], "FINISHED",
        "IF EXISTS makes no change and no error, got {described}"
    );
}

/// The hash functions Redshift documents compute the digests their published
/// test vectors state, in the lower-case hexadecimal form the service reports.
///
/// The expected values are the PUBLISHED vectors for the string `abc`, so what
/// the rows pin is the digest itself and not this emulator's own computation.
#[tokio::test]
async fn the_hash_functions_compute_the_published_digests() {
    let emulator = Emulator::new();
    let rows = [
        Row {
            label: "MD5",
            sql: "SELECT md5('abc')".to_string(),
            expected: json!({"stringValue": "900150983cd24fb0d6963f7d28e17f72"}),
        },
        Row {
            label: "SHA, which is SHA-1",
            sql: "SELECT sha('abc')".to_string(),
            expected: json!({"stringValue": "a9993e364706816aba3e25717850c26c9cd0d89d"}),
        },
        Row {
            label: "SHA1",
            sql: "SELECT sha1('abc')".to_string(),
            expected: json!({"stringValue": "a9993e364706816aba3e25717850c26c9cd0d89d"}),
        },
        Row {
            label: "SHA2 at 224 bits",
            sql: "SELECT sha2('abc', 224)".to_string(),
            expected: json!({
                "stringValue": "23097d223405d8228642a477bda255b32aadbce4bda0b3f7e36c9da7"
            }),
        },
        Row {
            label: "SHA2 at 256 bits",
            sql: "SELECT sha2('abc', 256)".to_string(),
            expected: json!({
                "stringValue":
                    "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
            }),
        },
        Row {
            // "0 is equivalent to 256."
            label: "SHA2 at 0 bits",
            sql: "SELECT sha2('abc', 0)".to_string(),
            expected: json!({
                "stringValue":
                    "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
            }),
        },
        Row {
            label: "SHA2 at 384 bits",
            sql: "SELECT sha2('abc', 384)".to_string(),
            expected: json!({
                "stringValue": "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5\
                                bed8086072ba1e7cc2358baeca134c825a7"
            }),
        },
        Row {
            label: "SHA2 at 512 bits",
            sql: "SELECT sha2('abc', 512)".to_string(),
            expected: json!({
                "stringValue": "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d\
                                39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa5\
                                4ca49f"
            }),
        },
    ];

    for row in &rows {
        let cell = one_cell(&emulator, row.label, &row.sql).await;
        assert_eq!(
            cell, row.expected,
            "case {}: expected {}, got {cell}",
            row.label, row.expected
        );
    }

    // A width the function does not admit is an error, not a digest of some
    // other width.
    let message = failure(
        &emulator,
        "SHA2 at a width it does not admit",
        "SELECT sha2('abc', 123)",
    )
    .await;
    assert!(
        message.contains("123"),
        "expected the refusal to name the width it was given, got {message:?}"
    );
}

/// The string and conditional functions Redshift documents answer what it
/// documents for them, including for the arguments at the edge of their range.
#[tokio::test]
async fn the_string_functions_answer_what_redshift_documents() {
    let emulator = Emulator::new();
    let rows = [
        Row {
            label: "LEFT",
            sql: "SELECT left('abcdef', 3)".to_string(),
            expected: json!({"stringValue": "abc"}),
        },
        Row {
            label: "LEFT past the end of the string",
            sql: "SELECT left('abc', 9)".to_string(),
            expected: json!({"stringValue": "abc"}),
        },
        Row {
            label: "LEFT of no characters at all",
            sql: "SELECT left('abc', 0)".to_string(),
            expected: json!({"stringValue": ""}),
        },
        Row {
            label: "RIGHT",
            sql: "SELECT right('abcdef', 2)".to_string(),
            expected: json!({"stringValue": "ef"}),
        },
        Row {
            label: "REPEAT",
            sql: "SELECT repeat('ab', 3)".to_string(),
            expected: json!({"stringValue": "ababab"}),
        },
        Row {
            label: "REPLICATE, the synonym of REPEAT",
            sql: "SELECT replicate('ab', 2)".to_string(),
            expected: json!({"stringValue": "abab"}),
        },
        Row {
            label: "REPEAT no times",
            sql: "SELECT repeat('ab', 0)".to_string(),
            expected: json!({"stringValue": ""}),
        },
        Row {
            label: "LEN, which counts characters and not bytes",
            sql: "SELECT len('héllo')".to_string(),
            expected: json!({"longValue": 5}),
        },
        Row {
            label: "STRPOS, which is 1-based",
            sql: "SELECT strpos('abcdef', 'cd')".to_string(),
            expected: json!({"longValue": 3}),
        },
        Row {
            label: "STRPOS of a substring that is not there",
            sql: "SELECT strpos('abcdef', 'zz')".to_string(),
            expected: json!({"longValue": 0}),
        },
        Row {
            label: "CHARINDEX, which takes the substring first",
            sql: "SELECT charindex('cd', 'abcdef')".to_string(),
            expected: json!({"longValue": 3}),
        },
        Row {
            label: "SPLIT_PART",
            sql: "SELECT split_part('a,b,c', ',', 2)".to_string(),
            expected: json!({"stringValue": "b"}),
        },
        Row {
            label: "SPLIT_PART past the parts the string has",
            sql: "SELECT split_part('a,b,c', ',', 9)".to_string(),
            expected: json!({"stringValue": ""}),
        },
        Row {
            label: "INITCAP",
            sql: "SELECT initcap('hello WORLD of sql')".to_string(),
            expected: json!({"stringValue": "Hello World Of Sql"}),
        },
        Row {
            label: "REVERSE",
            sql: "SELECT reverse('abc')".to_string(),
            expected: json!({"stringValue": "cba"}),
        },
        Row {
            label: "NVL, which answers the first argument that is not null",
            sql: "SELECT nvl(NULL, NULL, 'third')".to_string(),
            expected: json!({"stringValue": "third"}),
        },
        Row {
            label: "NVL2, when the first argument is null",
            sql: "SELECT nvl2(NULL, 'not null', 'null')".to_string(),
            expected: json!({"stringValue": "null"}),
        },
        Row {
            label: "NVL2, when it is not",
            sql: "SELECT nvl2('x', 'not null', 'null')".to_string(),
            expected: json!({"stringValue": "not null"}),
        },
        Row {
            label: "GETDATE, which reads the clock in the engine's own form",
            sql: "SELECT len(getdate())".to_string(),
            expected: json!({"longValue": 19}),
        },
        Row {
            label: "NOW, the same reading",
            sql: "SELECT CASE WHEN now() > '2020-01-01 00:00:00' THEN 1 ELSE 0 END".to_string(),
            expected: json!({"longValue": 1}),
        },
    ];

    for row in &rows {
        let cell = one_cell(&emulator, row.label, &row.sql).await;
        assert_eq!(
            cell, row.expected,
            "case {}: expected {}, got {cell}",
            row.label, row.expected
        );
    }

    // The family is strict: a null argument makes the answer null rather than a
    // hash or a slice of nothing.
    for sql in [
        "SELECT md5(NULL) IS NULL",
        "SELECT left(NULL, 2) IS NULL",
        "SELECT len(NULL) IS NULL",
        "SELECT split_part(NULL, ',', 1) IS NULL",
    ] {
        let cell = one_cell(&emulator, "a null argument", sql).await;
        assert_eq!(
            cell,
            json!({"longValue": 1}),
            "case {sql}: a null argument makes the answer null, got {cell}"
        );
    }

    // And a position outside the range the function documents is an error.
    let message = failure(
        &emulator,
        "SPLIT_PART at a position below its range",
        "SELECT split_part('a,b', ',', 0)",
    )
    .await;
    assert_eq!(
        message, "field position must be greater than zero",
        "expected the message the function refuses a position below one with, got {message:?}"
    );
}

/// The text of ONE submission may carry SEVERAL statements: a data warehouse
/// runs them in order, the submission reports the LAST of them, and it stops at
/// the first one that fails.
///
/// The rows differ in how many statements the text carries, in what separates
/// them, in whether a semicolon inside a literal is one of those separators, and
/// in which statement of the text fails.
#[tokio::test]
async fn a_submission_runs_every_statement_its_text_carries() {
    let emulator = Emulator::new();

    for (label, sql) in [
        (
            "a transaction opened and a lock taken in one submission",
            "CREATE TABLE held (id INTEGER); BEGIN; LOCK TABLE held IN ACCESS EXCLUSIVE MODE; \
             COMMIT;",
        ),
        (
            "a transaction control with nothing to do, twice over",
            "BEGIN; BEGIN; COMMIT; COMMIT;",
        ),
        (
            "a commit outside a transaction, which the warehouse warns about rather than refuses",
            "COMMIT",
        ),
        (
            "a rollback outside a transaction",
            "ROLLBACK;",
        ),
        (
            "the clauses the transaction statements carry",
            "START TRANSACTION ISOLATION LEVEL SERIALIZABLE READ WRITE; COMMIT WORK;",
        ),
        (
            "a transaction closed with the END alias of COMMIT",
            "BEGIN; END;",
        ),
        (
            "a transaction closed with the ABORT alias of ROLLBACK",
            "BEGIN; ABORT;",
        ),
    ] {
        let described = run(&emulator, sql).await;
        assert_eq!(
            described["Status"], "FINISHED",
            "case {label}: {sql} must run, got {described}"
        );
    }

    // The submission reports the LAST statement its text carried.
    let last = one_cell(
        &emulator,
        "several statements, of which the last produces the result",
        "CREATE TABLE multi (id INTEGER); INSERT INTO multi VALUES (1), (2), (3); \
         SELECT count(*) FROM multi",
    )
    .await;
    assert_eq!(
        last,
        json!({"longValue": 3}),
        "the rows every statement of the text inserted must be the rows the last one counts, got \
         {last}"
    );

    // A semicolon inside a literal separates nothing.
    let literal = one_cell(
        &emulator,
        "a semicolon inside a string literal",
        "SELECT 'a;b' AS semi",
    )
    .await;
    assert_eq!(
        literal,
        json!({"stringValue": "a;b"}),
        "a semicolon inside a literal is part of it, got {literal}"
    );

    // The submission stops at the first statement that fails: what came before
    // it ran, and what came after it did not.
    let message = failure(
        &emulator,
        "a text whose second statement fails",
        "CREATE TABLE ran_first (id INTEGER); SELECT * FROM absent_relation; \
         CREATE TABLE never_ran (id INTEGER)",
    )
    .await;
    assert!(
        message.contains("absent_relation"),
        "the submission must report the statement that failed, got {message:?}"
    );
    let ran = one_cell(
        &emulator,
        "the statement before the failure",
        "SELECT count(*) FROM ran_first",
    )
    .await;
    assert_eq!(
        ran,
        json!({"longValue": 0}),
        "the statement before the failure ran, got {ran}"
    );
    let message = failure(
        &emulator,
        "the statement after the failure",
        "SELECT count(*) FROM never_ran",
    )
    .await;
    assert!(
        message.contains("never_ran"),
        "the statement after the failure must not have run, got {message:?}"
    );
}

/// A statement a caller submits is run as WRITTEN when no rewrite applies: the
/// text the service reports back is the caller's own, whatever the engine then
/// made of it.
#[tokio::test]
async fn a_statement_no_rewrite_applies_to_is_reported_as_written() {
    let emulator = Emulator::new();
    let sql = "SELECT   1 AS  /* a comment */ one, 'a::b' AS text";
    let described = run(&emulator, sql).await;
    assert_eq!(
        described["Status"], "FINISHED",
        "the statement must run, got {described}"
    );
    assert_eq!(
        described["QueryString"], sql,
        "the service reports the statement the caller submitted, got {described}"
    );
    let fetched = emulator
        .ok("GetStatementResult", &json!({"Id": described["Id"]}))
        .await;
    assert_eq!(
        fetched["Records"],
        json!([[{"longValue": 1}, {"stringValue": "a::b"}]]),
        "a `::` inside a string literal is part of that literal, got {fetched}"
    );
}

/// `WaitTimeSeconds` is what every statement here is submitted with, so each one
/// has already reached its terminal state when it is read back.
const _: () = assert!(
    WAIT_SECONDS > 0,
    "the suites above wait for every statement they submit"
);
