// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the TYPE a PostgreSQL endpoint reports each result column under,
//! and for the server surface two of those columns come from.
//!
//! The rules under test:
//!
//! * a result column is described under the type of the VALUE it carries: a column of a
//!   table carries the type its declaration names, and a COMPUTED column (an aggregate, a
//!   function call, an expression) carries the type the engine holds its value as — so a
//!   client's driver decodes an integral column as an integer and a boolean column as a
//!   boolean, through the extended query protocol's `Describe` as well as through a simple
//!   query. This is the `RowDescription` type OID, which is what a driver picks its decoder
//!   by;
//! * the master user a `CreateDBInstance` mints is a non-superuser role carrying
//!   `CREATEROLE` and `CREATEDB`, which is how Amazon RDS withholds superuser while leaving
//!   the master user able to create the roles and databases an application needs;
//! * `md5(<text>)` and `pg_get_userbyid(<oid>)` are evaluated like any other function call:
//!   the digest over the argument's own bytes, and the role name the server's own role
//!   catalog holds for that object identifier.
//!
//! The tables span what the rules generalise over: an aggregate, a scalar function of the
//! server's own, a cast, an arithmetic expression, a declared integer column, a declared
//! boolean column and a declared text column — read through the extended protocol, where
//! the type is reported BEFORE any row exists, and through a simple query.

use crate::db_instance_engine_endpoint::{
    available_instance, postgres_login, PostgresSession, Transport,
};
use crate::db_instances::emulator;

/// The master user the instance here is created with, and its test password — the only
/// place either appears.
const MASTER_USER: &str = "emuadmin";
const MASTER_PASSWORD: &str = "emu-Secret-99";
/// The `DBName` the instance here is created with.
const DATABASE: &str = "appdb";

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// The `pg_type` OIDs a client decodes a value by, from PostgreSQL's own `pg_type`: the
/// ones this suite asserts over.
const BOOL: i32 = 16;
const BIGINT: i32 = 20;
const INTEGER: i32 = 23;
const TEXT: i32 = 25;
const NUMERIC: i32 = 1700;

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

#[tokio::test]
async fn a_result_column_is_described_under_the_type_of_the_value_it_carries() {
    let router = emulator();
    let (address, port) = available_instance(
        &router,
        "emu-column-types-postgres",
        "postgres",
        &params!["EngineVersion" => "16.4"],
    )
    .await;
    let mut session = session(&address, port).await;
    session
        .query(
            "CREATE TABLE readings (id integer PRIMARY KEY, label text, \
             reading numeric(10,2), live boolean)",
        )
        .await
        .expect("the table is created");
    session
        .query("INSERT INTO readings VALUES (7, 'north', 12.50, true)")
        .await
        .expect("the row is inserted");

    // Each row is one statement run through the EXTENDED protocol — `Parse`, `Bind`,
    // `Describe`, `Execute` — where the server reports the column's type before it has run
    // the statement at all, which is where a column whose type is the value's was reported
    // as text. The rows span the kinds of column a statement produces.
    struct Case {
        case: &'static str,
        sql: &'static str,
        params: &'static [&'static str],
        oids: &'static [i32],
        values: &'static [Option<&'static str>],
    }
    let cases = [
        Case {
            case: "an aggregate over a catalog",
            sql: "SELECT count(*) FROM pg_stat_activity WHERE datname = $1",
            params: &[DATABASE],
            oids: &[BIGINT],
            values: &[Some("1")],
        },
        Case {
            case: "an aggregate over a table",
            sql: "SELECT count(*) FROM readings WHERE label = $1",
            params: &["north"],
            oids: &[BIGINT],
            values: &[Some("1")],
        },
        Case {
            case: "an arithmetic expression over a bound parameter",
            sql: "SELECT length($1) + 1",
            params: &["abcd"],
            oids: &[BIGINT],
            values: &[Some("5")],
        },
        Case {
            // A cast carries no declaration into the engine's answer, so the column is
            // described by the class the engine holds the value in — the widest integer
            // type, which decodes losslessly to the same integer.
            case: "a cast of a bound parameter to an integer",
            sql: "SELECT $1::integer",
            params: &["41"],
            oids: &[BIGINT],
            values: &[Some("41")],
        },
        Case {
            case: "a server function returning text",
            sql: "SELECT md5($1)",
            params: &["abc"],
            oids: &[TEXT],
            // The MD5 digest of "abc", as both engines write it: lower-case hexadecimal.
            values: &[Some("900150983cd24fb0d6963f7d28e17f72")],
        },
        Case {
            case: "a catalog-accessor function returning a role name",
            sql: "SELECT pg_get_userbyid(datdba) FROM pg_database WHERE datname = $1",
            params: &[DATABASE],
            oids: &[TEXT],
            values: &[Some(MASTER_USER)],
        },
        Case {
            // "does user have privilege for role" — the master user is a member of the
            // `rds_superuser` role RDS grants it, and of no role it was never granted.
            case: "a membership question over the role catalog",
            sql: "SELECT pg_has_role($1, 'rds_superuser', 'MEMBER'), \
                  pg_has_role($1, 'rds_replication', 'MEMBER'), \
                  pg_has_role($1, $1, 'USAGE')",
            params: &[MASTER_USER],
            oids: &[BOOL, BOOL, BOOL],
            values: &[Some("t"), Some("f"), Some("t")],
        },
        Case {
            case: "the declared columns of a table, each under its own type",
            sql: "SELECT id, label, reading, live FROM readings WHERE id = $1",
            params: &["7"],
            oids: &[INTEGER, TEXT, NUMERIC, BOOL],
            values: &[Some("7"), Some("north"), Some("12.5"), Some("t")],
        },
        Case {
            case: "a boolean column of a catalog, for a role the create minted",
            sql: "SELECT rolsuper, rolcreaterole, rolcreatedb, rolreplication, rolinherit \
                  FROM pg_roles WHERE rolname = $1",
            params: &[MASTER_USER],
            oids: &[BOOL, BOOL, BOOL, BOOL, BOOL],
            // The master user of an RDS for PostgreSQL instance: no superuser, and
            // CREATEROLE and CREATEDB so it can make roles and databases itself.
            values: &[Some("f"), Some("t"), Some("t"), Some("f"), Some("t")],
        },
        Case {
            case: "a statement whose predicate matches no row",
            sql: "SELECT count(*) FROM readings WHERE label = $1",
            params: &["nowhere"],
            oids: &[BIGINT],
            values: &[Some("0")],
        },
    ];
    for Case {
        case,
        sql,
        params,
        oids,
        values,
    } in cases
    {
        let answer = session
            .query_with_params(sql, params)
            .await
            .unwrap_or_else(|refusal| panic!("case {case:?}: {sql:?} must run, got {refusal:?}"));
        assert_eq!(
            answer.type_oids, oids,
            "case {case:?}: expected the column types {oids:?}, got {:?} for {sql:?}",
            answer.type_oids
        );
        let read: Vec<Option<&str>> = answer
            .rows
            .first()
            .map(|row| row.iter().map(Option::as_deref).collect())
            .unwrap_or_default();
        assert_eq!(
            read, values,
            "case {case:?}: expected the row {values:?}, got {read:?} for {sql:?}"
        );
    }
}

#[tokio::test]
async fn exact_decimal_arithmetic_is_computed_exactly_and_reported_as_numeric() {
    let router = emulator();
    let (address, port) = available_instance(
        &router,
        "emu-exact-decimal-postgres",
        "postgres",
        &params!["EngineVersion" => "16.4"],
    )
    .await;
    let mut session = session(&address, port).await;

    // `numeric` is the EXACT decimal type — "calculations with numeric values yield exact
    // results where possible" — so each row below is an expression whose exact answer a
    // binary floating-point computation cannot give, read back as the digits it has under
    // the `numeric` type. The rows span the four arithmetic operators, a cast with a
    // declared scale (which rounds), a sum that is inexact in binary floating point, a
    // quotient that does not divide, and a negative operand.
    for (case, sql, expected, oid) in [
        (
            "a sum that binary floating point cannot represent",
            "SELECT 0.1::numeric + 0.2::numeric",
            "0.3",
            NUMERIC,
        ),
        (
            // Not exact-decimal arithmetic: an integer division answers an integer, as both
            // engines and the embedded one compute it.
            "an integer division, which truncates",
            "SELECT 7 / 2",
            "3",
            BIGINT,
        ),
        (
            "a quotient rounded to a declared scale",
            "SELECT (1::numeric / 3)::numeric(10,4)",
            "0.3333",
            NUMERIC,
        ),
        (
            "a quotient that does not divide",
            "SELECT 1::numeric / 3",
            "0.33333333333333333333",
            NUMERIC,
        ),
        (
            "a quotient that divides exactly",
            "SELECT 10::numeric / 4",
            "2.5",
            NUMERIC,
        ),
        (
            "a product with an integer operand",
            "SELECT 2.5::numeric * 4",
            "10",
            NUMERIC,
        ),
        (
            "a difference at the far end of the scale",
            "SELECT 1::numeric - 0.0000000001::numeric",
            "0.9999999999",
            NUMERIC,
        ),
        (
            "a negated operand",
            "SELECT -(0.1::numeric) + 0.3::numeric",
            "0.2",
            NUMERIC,
        ),
        (
            "an expression whose operators keep their own precedence",
            "SELECT 1::numeric + 2 * 3",
            "7",
            NUMERIC,
        ),
        (
            "a cast that rounds to no fractional digits",
            "SELECT 2.5::numeric(3,0)",
            "3",
            NUMERIC,
        ),
    ] {
        let answer = session
            .query(sql)
            .await
            .unwrap_or_else(|refusal| panic!("case {case:?}: {sql:?} must run, got {refusal:?}"));
        assert_eq!(
            answer.type_oids,
            vec![oid],
            "case {case:?}: expected the column type {oid}, got {:?} for {sql:?}",
            answer.type_oids
        );
        let read = answer
            .rows
            .first()
            .and_then(|row| row.first())
            .and_then(Option::as_deref);
        assert_eq!(
            read,
            Some(expected),
            "case {case:?}: expected {expected:?}, got {read:?} for {sql:?}"
        );
    }

    // A division by zero is the engine's own refusal, in the words PostgreSQL words it and
    // under its own `division_by_zero` — over exact decimals and over the integers alike,
    // where the embedded engine answers NULL.
    for sql in [
        "SELECT 1::numeric / 0",
        "SELECT 1 / 0",
        "SELECT 1.5 / 0.0",
        "SELECT 1 / (1 - 1)",
    ] {
        let refusal = session
            .query(sql)
            .await
            .expect_err(&format!("case {sql:?}: a division by zero must be refused"));
        assert_eq!(
            (refusal.code.as_str(), refusal.message.as_str()),
            ("22012", "division by zero"),
            "case {sql:?}: a division by zero must be refused in the engine's own words, got \
             {refusal:?}"
        );
    }
    // A value that no longer fits the precision its cast declares is refused, under
    // PostgreSQL's own `numeric_value_out_of_range`.
    let overflowed = session
        .query("SELECT 12345.678::numeric(5,2)")
        .await
        .expect_err("a value past the declared precision must be refused");
    assert_eq!(
        (overflowed.code.as_str(), overflowed.message.as_str()),
        ("22003", "numeric field overflow"),
        "a value past its declared precision must be refused as a numeric field overflow, \
         got {overflowed:?}"
    );
    // A `COPY` that names a FILE or a program on the host is a capability no Amazon RDS
    // instance serves.
    for (sql, says) in [
        (
            "COPY readings TO '/tmp/readings.csv'",
            "COPY to a file is not supported",
        ),
        (
            "COPY readings FROM '/tmp/readings.csv'",
            "COPY from a file is not supported",
        ),
        (
            "COPY readings TO PROGRAM 'cat'",
            "COPY to a file is not supported",
        ),
    ] {
        let refused = session
            .query(sql)
            .await
            .expect_err(&format!("case {sql:?}: a server-side COPY must be refused"));
        assert_eq!(
            (refused.code.as_str(), refused.message.as_str()),
            ("0A000", says),
            "case {sql:?}: expected the refusal {says:?} under 0A000, got {refused:?}"
        );
    }
}

#[tokio::test]
async fn a_simple_query_reports_the_same_column_types_as_the_extended_protocol() {
    let router = emulator();
    let (address, port) = available_instance(
        &router,
        "emu-column-types-simple-postgres",
        "postgres",
        &params!["EngineVersion" => "16.4"],
    )
    .await;
    let mut session = session(&address, port).await;
    session
        .query("CREATE TABLE counters (id integer PRIMARY KEY, name text)")
        .await
        .expect("the table is created");

    // The same statements with no parameter to bind, which a driver sends as a SIMPLE
    // query: the type a client decodes by must be the same one either way.
    for (case, sql, oids) in [
        ("an aggregate", "SELECT count(*) FROM counters", vec![BIGINT]),
        ("a digest", "SELECT md5('abc')", vec![TEXT]),
        (
            "an owner lookup",
            "SELECT pg_get_userbyid(datdba) FROM pg_database WHERE datname = 'appdb'",
            vec![TEXT],
        ),
        (
            "declared columns",
            "SELECT id, name FROM counters",
            vec![INTEGER, TEXT],
        ),
    ] {
        let answer = session
            .query(sql)
            .await
            .unwrap_or_else(|refusal| panic!("case {case:?}: {sql:?} must run, got {refusal:?}"));
        assert_eq!(
            answer.type_oids, oids,
            "case {case:?}: expected the column types {oids:?}, got {:?} for {sql:?}",
            answer.type_oids
        );
    }
}
