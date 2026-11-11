// SPDX-License-Identifier: CC0-1.0

//! Probes of the dialect reader itself, driven DIRECTLY against
//! `state::redshift_sql` rather than over the wire.
//!
//! The suites beside this one assert what a caller sees: a statement submitted
//! over HTTP, and the status, error and rows the service reports for it. What is
//! asserted here is the one decision underneath them all — how the text of a
//! submission is cut into the statements it carries — over inputs a wire test
//! cannot reach as sharply: a separator inside a string literal, inside a quoted
//! identifier and inside a comment, an empty statement, and a text the
//! tokenizer cannot read at all.
//!
//! Each row states the cut it expects, so a reader of the table sees the rule:
//! only a semicolon the SQL GRAMMAR reports as a separator separates, and every
//! piece is a slice of the submitted text rather than anything re-rendered.

use std::time::{Duration, Instant};

use service_com_amazonaws_redshiftdata_redshiftdata::state::redshift_sql::split_statements;
use service_com_amazonaws_redshiftdata_redshiftdata::state::MAX_STATEMENTS_PER_SUBMISSION;

/// One row: the text a submission carries, and the statements it holds.
struct Split {
    label: &'static str,
    sql: &'static str,
    expected: &'static [&'static str],
}

/// The text of one submission is cut into statements at the semicolons the
/// grammar reports, and nowhere else.
#[test]
fn a_submissions_text_is_cut_into_the_statements_it_carries() {
    let rows = [
        Split {
            label: "one statement, with no separator at all",
            sql: "SELECT 1",
            expected: &["SELECT 1"],
        },
        Split {
            label: "one statement, terminated",
            sql: "SELECT 1;",
            expected: &["SELECT 1"],
        },
        Split {
            label: "two statements",
            sql: "BEGIN; LOCK TABLE held IN ACCESS EXCLUSIVE MODE;",
            expected: &["BEGIN", " LOCK TABLE held IN ACCESS EXCLUSIVE MODE"],
        },
        Split {
            label: "three statements, the last unterminated",
            sql: "BEGIN;INSERT INTO t VALUES (1);SELECT count(*) FROM t",
            expected: &[
                "BEGIN",
                "INSERT INTO t VALUES (1)",
                "SELECT count(*) FROM t",
            ],
        },
        Split {
            label: "a separator inside a string literal",
            sql: "SELECT 'a;b'",
            expected: &["SELECT 'a;b'"],
        },
        Split {
            label: "a separator inside a quoted identifier",
            sql: "SELECT 1 AS \"a;b\"",
            expected: &["SELECT 1 AS \"a;b\""],
        },
        Split {
            label: "a separator inside a line comment",
            sql: "SELECT 1 -- ; not a separator\n",
            expected: &["SELECT 1 -- ; not a separator\n"],
        },
        Split {
            label: "a separator inside a block comment",
            sql: "SELECT /* ; */ 1; SELECT 2",
            expected: &["SELECT /* ; */ 1", " SELECT 2"],
        },
        Split {
            label: "an empty statement between two separators",
            sql: "SELECT 1;; SELECT 2",
            expected: &["SELECT 1", " SELECT 2"],
        },
        Split {
            label: "a statement spanning several lines",
            sql: "SELECT\n  1;\nSELECT\n  2",
            expected: &["SELECT\n  1", "\nSELECT\n  2"],
        },
        Split {
            label: "nothing but separators, which is one empty statement",
            sql: " ; ",
            expected: &[" ; "],
        },
        Split {
            label: "a text the tokenizer cannot read, which is left whole",
            sql: "SELECT 'unterminated",
            expected: &["SELECT 'unterminated"],
        },
    ];

    for row in rows {
        let statements = split_statements(row.sql);
        assert_eq!(
            statements, row.expected,
            "case {}: expected {:?}, got {statements:?}",
            row.label, row.expected
        );
        for statement in &statements {
            assert!(
                row.sql.contains(statement),
                "case {}: every statement is a slice of the submitted text, but {statement:?} is \
                 not in {:?}",
                row.label,
                row.sql
            );
        }
    }
}

/// The largest text the engine accepts, in bytes (`SQLITE_LIMIT_SQL_LENGTH` as
/// `state::MAX_SQL_BYTES` sets it): the most a submission can ever hand the
/// reader, and therefore the size the rows below use.
const LARGEST_TEXT_BYTES: usize = 1024 * 1024;

/// A pathological text is cut in ONE pass over it: the work is linear in the
/// text, not in the product of its separators and its length, and the number of
/// statements one submission may carry bounds what is cut at all.
///
/// The rows are the degenerate shapes — the largest text the engine accepts made
/// of nothing but separators, and the same size made of the shortest statements
/// — each on ONE line, which is the shape a per-separator rescan of the line
/// would be quadratic in.
#[test]
fn a_pathological_text_is_cut_in_one_pass_over_it() {
    // Generous by two orders of magnitude: one pass over a mebibyte is
    // microseconds, so what this bound catches is a cost that grows with the
    // square of the text rather than a slow machine.
    let budget = Duration::from_secs(10);

    let separators = ";".repeat(LARGEST_TEXT_BYTES);
    let started = Instant::now();
    let statements = split_statements(&separators);
    assert_eq!(
        statements,
        vec![separators.as_str()],
        "a text of nothing but separators carries one statement, the empty one, got {} piece(s)",
        statements.len()
    );

    let shortest = "SELECT 1;";
    let many = shortest.repeat(LARGEST_TEXT_BYTES / shortest.len());
    let statements = split_statements(&many);
    assert!(
        statements.len() <= MAX_STATEMENTS_PER_SUBMISSION + 1,
        "cutting stops once the submission is past the {MAX_STATEMENTS_PER_SUBMISSION} \
         statements one may carry, got {} piece(s)",
        statements.len()
    );
    assert!(
        statements
            .iter()
            .all(|statement| *statement == shortest.trim_end_matches(';')),
        "every statement cut out of it is the statement the text repeats, got {statements:?}"
    );

    assert!(
        started.elapsed() < budget,
        "cutting the largest text the engine accepts must be linear in that text; the two \
         pathological shapes took {:?}, past the {budget:?} this allows",
        started.elapsed()
    );
}
