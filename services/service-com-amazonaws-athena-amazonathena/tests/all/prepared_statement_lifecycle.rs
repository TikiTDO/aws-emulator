// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for Athena's prepared-statement slice: `CreatePreparedStatement`
//! registers a statement in the workgroup it belongs to and `GetPreparedStatement`,
//! `BatchGetPreparedStatement`, `UpdatePreparedStatement` and `DeletePreparedStatement`
//! address that same record, driven over the wire (awsJson1_1, `x-amz-target:
//! AmazonAthena.<Operation>`) through the same router the emulator boots, so every
//! assertion is about what an SDK client observes.
//!
//! Each test states a rule and asserts it over a TABLE of inputs rather than one request:
//! the rows differ along whatever the rule generalises over — whether a description was
//! carried, the character set and the length of the statement name, the size and the form
//! of the query string, which workgroup holds the statement, and how many names a batch
//! read asks for.

use serde_json::{json, Value};
use service_com_amazonaws_athena_amazonathena::state::{
    no_prepared_statement_with_name, prepared_statement_already_exists,
    prepared_statement_does_not_exist, prepared_statement_name_not_found,
    prepared_statement_not_found, too_many_prepared_statement_names, too_many_prepared_statements,
    ERROR_CODE_INVALID_INPUT, ERROR_CODE_STATEMENT_NOT_FOUND,
    MAX_PREPARED_STATEMENTS_PER_WORK_GROUP, MAX_PREPARED_STATEMENT_NAMES_PER_BATCH,
    WORK_GROUP_NOT_FOUND,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, create_work_group, error_code, message, router};

/// The workgroup every account has without creating one, which holds prepared statements
/// like any other.
const DEFAULT_WORK_GROUP: &str = "primary";

/// The wire error code a request this service refuses itself carries.
const INVALID_REQUEST: &str = "InvalidRequestException";

/// The wire error code the three single-statement operations report a resource this
/// account does not hold with.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// A statement name of `length` characters drawn from the documented
/// `StatementName` set (`^[a-zA-Z_][a-zA-Z0-9_@:]{1,256}$`: a letter or an underscore,
/// then letters, digits, underscores, at signs and colons).
fn name_of_length(length: usize) -> String {
    let tail: String = "aA0_@:".chars().cycle().take(length - 1).collect();
    format!("s{tail}")
}

/// `GetPreparedStatement`'s view of one statement: `(status, PreparedStatement)`.
async fn read_statement(
    router: &MultiServiceRouter,
    work_group: &str,
    name: &str,
) -> (u16, Value) {
    let (status, body) = call(
        router,
        "GetPreparedStatement",
        &json!({ "WorkGroup": work_group, "StatementName": name }),
    )
    .await;
    (status, body["PreparedStatement"].clone())
}

/// Create one statement and assert the create was accepted.
async fn create_statement(router: &MultiServiceRouter, request: &Value) {
    let (status, created) = call(router, "CreatePreparedStatement", request).await;
    assert_eq!(
        status, 200,
        "CreatePreparedStatement({request}) must be accepted, got {status} with {created:?}"
    );
}

/// One row of the batch-read table: the names asked for, and how the answer must split
/// them.
struct BatchCase {
    case: &'static str,
    asked: Vec<&'static str>,
    expected_found: Vec<&'static str>,
    expected_unprocessed: Vec<&'static str>,
}

/// One row of the create/read-back table.
struct CreateCase {
    case: &'static str,
    work_group: String,
    name: String,
    query_statement: String,
    description: Option<String>,
}

/// Every member a create persists is reported by a read of the statement, and a member the
/// create did not carry is absent from that read rather than reported as an empty one.
#[tokio::test]
async fn a_created_prepared_statement_is_read_back_exactly_as_it_was_written() {
    let router = router();
    const WORK_GROUP: &str = "prepared-statement-suite";
    create_work_group(&router, WORK_GROUP).await;

    let rows: Vec<CreateCase> = vec![
        CreateCase {
            case: "a statement with a description",
            work_group: WORK_GROUP.to_string(),
            name: "with_description".to_string(),
            query_statement: "SELECT * FROM orders WHERE id = ?".to_string(),
            description: Some("one parameter".to_string()),
        },
        CreateCase {
            case: "a statement whose create carried no description",
            work_group: WORK_GROUP.to_string(),
            name: "without_description".to_string(),
            query_statement: "SELECT 1".to_string(),
            description: None,
        },
        CreateCase {
            case: "a name using every character class the model's pattern admits",
            work_group: WORK_GROUP.to_string(),
            name: "_aZ0@:".to_string(),
            query_statement: "SELECT * FROM t WHERE a = ? AND b = ?".to_string(),
            description: Some("two parameters".to_string()),
        },
        CreateCase {
            case: "the shortest name the modelled pattern admits",
            work_group: WORK_GROUP.to_string(),
            name: name_of_length(2),
            query_statement: "SELECT 2".to_string(),
            description: None,
        },
        CreateCase {
            case: "the longest name the modelled length admits",
            work_group: WORK_GROUP.to_string(),
            name: name_of_length(256),
            query_statement: "SELECT 3".to_string(),
            description: Some("x".repeat(1024)),
        },
        CreateCase {
            case: "a multi-line query statement",
            work_group: WORK_GROUP.to_string(),
            name: "multi_line".to_string(),
            query_statement: "SELECT a,\n       b\n  FROM t\n WHERE a = ?".to_string(),
            description: Some("a".to_string()),
        },
        CreateCase {
            case: "a long query statement",
            work_group: WORK_GROUP.to_string(),
            name: "long_query".to_string(),
            query_statement: format!("SELECT * FROM t WHERE c IN ({})", "?, ".repeat(500)),
            description: None,
        },
        CreateCase {
            case: "a statement in the workgroup every account has without creating one",
            work_group: DEFAULT_WORK_GROUP.to_string(),
            name: "in_the_default_work_group".to_string(),
            query_statement: "SELECT * FROM logs WHERE day = ?".to_string(),
            description: Some("held by primary".to_string()),
        },
    ];

    for row in rows {
        let case = row.case;
        let mut request = json!({
            "WorkGroup": row.work_group,
            "StatementName": row.name,
            "QueryStatement": row.query_statement,
        });
        if let Some(description) = &row.description {
            request["Description"] = json!(description);
        }
        create_statement(&router, &request).await;

        let (status, read) = read_statement(&router, &row.work_group, &row.name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the created statement must be readable, got {status} with {read:?}"
        );
        assert_eq!(
            read["StatementName"].as_str(),
            Some(row.name.as_str()),
            "case {case:?}: expected the name {:?}, got {read:?}",
            row.name
        );
        assert_eq!(
            read["WorkGroupName"].as_str(),
            Some(row.work_group.as_str()),
            "case {case:?}: expected the workgroup {:?}, got {read:?}",
            row.work_group
        );
        assert_eq!(
            read["QueryStatement"].as_str(),
            Some(row.query_statement.as_str()),
            "case {case:?}: expected the query statement it was created with, got {read:?}"
        );
        assert_eq!(
            read["Description"].as_str(),
            row.description.as_deref(),
            "case {case:?}: expected the description {:?}, got {read:?}",
            row.description
        );
        assert!(
            read["LastModifiedTime"].as_f64().is_some(),
            "case {case:?}: expected a last modified time, got {read:?}"
        );
    }

    // The boundary of the description rule: `DescriptionString` is 1 to 1024 characters, so
    // an EMPTY description is not a description a create may carry at all — which is what
    // keeps "no description" one state rather than two.
    let (status, refused) = call(
        &router,
        "CreatePreparedStatement",
        &json!({
            "WorkGroup": WORK_GROUP,
            "StatementName": "empty_description",
            "QueryStatement": "SELECT 1",
            "Description": "",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "an empty description is outside the modelled length, got {status} with {refused:?}"
    );
    let (status, absent) = read_statement(&router, WORK_GROUP, "empty_description").await;
    assert_eq!(
        status, 400,
        "a refused create must have persisted nothing, got {status} with {absent:?}"
    );
}

/// One workgroup holds at most the statements this emulator bounds it to: the create at
/// that bound is served and the one past it is refused, with the registry left holding
/// exactly the bound.
#[tokio::test]
async fn a_work_group_holds_at_most_the_statements_the_bound_allows() {
    let router = router();
    const WORK_GROUP: &str = "prepared-statement-count-bound";
    create_work_group(&router, WORK_GROUP).await;

    for index in 0..MAX_PREPARED_STATEMENTS_PER_WORK_GROUP {
        create_statement(
            &router,
            &json!({
                "WorkGroup": WORK_GROUP,
                "StatementName": format!("held{index}"),
                "QueryStatement": "SELECT 1",
            }),
        )
        .await;
    }

    let (status, refused) = call(
        &router,
        "CreatePreparedStatement",
        &json!({
            "WorkGroup": WORK_GROUP,
            "StatementName": "one_too_many",
            "QueryStatement": "SELECT 1",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a create past the bound must be refused, got {status} with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        INVALID_REQUEST,
        "expected {INVALID_REQUEST}, got {refused:?}"
    );
    assert_eq!(
        message(&refused),
        too_many_prepared_statements(WORK_GROUP),
        "expected the bound's sentence, got {:?}",
        message(&refused)
    );

    // The refused create stored nothing, and the statements the workgroup does hold are
    // still readable — the bound refuses a new statement rather than evicting one.
    let (status, absent) = read_statement(&router, WORK_GROUP, "one_too_many").await;
    assert_eq!(
        status, 400,
        "a refused create must have persisted nothing, got {status} with {absent:?}"
    );
    for name in ["held0", "held999"] {
        let (status, held) = read_statement(&router, WORK_GROUP, name).await;
        assert_eq!(
            status, 200,
            "the held statement {name} must still be readable, got {status} with {held:?}"
        );
    }

    // Another workgroup is bounded on its own: the account-wide total is a different bound.
    const OTHER: &str = "prepared-statement-count-bound-other";
    create_work_group(&router, OTHER).await;
    create_statement(
        &router,
        &json!({
            "WorkGroup": OTHER,
            "StatementName": "one_too_many",
            "QueryStatement": "SELECT 1",
        }),
    )
    .await;
}

/// A statement name identifies one statement WITHIN a workgroup: a create under a name the
/// addressed workgroup already holds is refused, and the same name in another workgroup is
/// a statement of its own, with its own query string.
#[tokio::test]
async fn a_statement_name_is_unique_within_its_work_group_only() {
    let router = router();
    const FIRST: &str = "prepared-statement-scope-one";
    const SECOND: &str = "prepared-statement-scope-two";
    create_work_group(&router, FIRST).await;
    create_work_group(&router, SECOND).await;

    // (the case, the name, the query string each workgroup stores it with)
    let rows: [(&str, &str, &str); 3] = [
        ("a plain name", "shared_name", "SELECT 1"),
        ("a name of the shortest admitted length", "s0", "SELECT 2"),
        (
            "a name using every character class",
            "_aZ0@:",
            "SELECT * FROM t WHERE a = ?",
        ),
    ];

    for (case, name, query) in rows {
        // The same name in three workgroups — two created and the account's default — is
        // three statements, each with a query string of its own.
        for work_group in [FIRST, SECOND, DEFAULT_WORK_GROUP] {
            create_statement(
                &router,
                &json!({
                    "WorkGroup": work_group,
                    "StatementName": name,
                    "QueryStatement": format!("{query} -- {work_group}"),
                }),
            )
            .await;
        }
        for work_group in [FIRST, SECOND, DEFAULT_WORK_GROUP] {
            let (status, read) = read_statement(&router, work_group, name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {work_group} must hold its own {name}, got {status} with {read:?}"
            );
            assert_eq!(
                read["QueryStatement"].as_str(),
                Some(format!("{query} -- {work_group}").as_str()),
                "case {case:?}: {work_group} must report its own query string, got {read:?}"
            );
        }

        // And a second create under that name in a workgroup that already holds it is
        // refused rather than overwriting the statement.
        let (status, refused) = call(
            &router,
            "CreatePreparedStatement",
            &json!({
                "WorkGroup": FIRST,
                "StatementName": name,
                "QueryStatement": "SELECT 'overwritten'",
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a duplicate create must be refused, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            INVALID_REQUEST,
            "case {case:?}: expected {INVALID_REQUEST}, got {refused:?}"
        );
        assert_eq!(
            refused["AthenaErrorCode"].as_str(),
            Some(ERROR_CODE_INVALID_INPUT),
            "case {case:?}: expected the {ERROR_CODE_INVALID_INPUT} class, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            prepared_statement_already_exists(name, FIRST),
            "case {case:?}: expected Athena's duplicate wording, got {:?}",
            message(&refused)
        );
        // The refused create changed nothing: the statement still carries what the first
        // create stored.
        let (_, read) = read_statement(&router, FIRST, name).await;
        assert_eq!(
            read["QueryStatement"].as_str(),
            Some(format!("{query} -- {FIRST}").as_str()),
            "case {case:?}: a refused create must leave the statement alone, got {read:?}"
        );
    }
}

/// An update writes the whole statement: the query string and the description it carries
/// replace what the statement held, its last modified time moves to the update, and an
/// update that carries no description leaves the statement with none.
#[tokio::test]
async fn an_update_replaces_the_query_statement_and_the_description() {
    let router = router();
    const WORK_GROUP: &str = "prepared-statement-update";
    create_work_group(&router, WORK_GROUP).await;

    // (the case, the description the create carried, the query string and the description
    //  the update carries)
    let rows: [(&str, Option<&str>, &str, Option<&str>); 4] = [
        (
            "a described statement rewritten with another description",
            Some("the first"),
            "SELECT * FROM t WHERE a = ?",
            Some("the second"),
        ),
        (
            "a described statement rewritten without a description",
            Some("the only one"),
            "SELECT 1",
            None,
        ),
        (
            "an undescribed statement given a description",
            None,
            "SELECT * FROM t WHERE b = ? AND c = ?",
            Some("now described"),
        ),
        (
            "an undescribed statement rewritten with a longer query",
            None,
            "SELECT a, b, c FROM t WHERE a IN (?, ?, ?)",
            None,
        ),
    ];

    for (index, (case, created_description, new_query, new_description)) in
        rows.into_iter().enumerate()
    {
        let name = format!("updated_{index}");
        let mut create = json!({
            "WorkGroup": WORK_GROUP,
            "StatementName": name,
            "QueryStatement": "SELECT 'before'",
        });
        if let Some(description) = created_description {
            create["Description"] = json!(description);
        }
        create_statement(&router, &create).await;
        let (_, before) = read_statement(&router, WORK_GROUP, &name).await;
        let created_at = before["LastModifiedTime"].as_f64().unwrap_or_default();

        let mut update = json!({
            "WorkGroup": WORK_GROUP,
            "StatementName": name,
            "QueryStatement": new_query,
        });
        if let Some(description) = new_description {
            update["Description"] = json!(description);
        }
        let (status, updated) = call(&router, "UpdatePreparedStatement", &update).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the update must be accepted, got {status} with {updated:?}"
        );

        let (status, after) = read_statement(&router, WORK_GROUP, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the statement must still be readable, got {status} with {after:?}"
        );
        assert_eq!(
            after["QueryStatement"].as_str(),
            Some(new_query),
            "case {case:?}: expected the query string the update carried, got {after:?}"
        );
        assert_eq!(
            after["Description"].as_str(),
            new_description,
            "case {case:?}: expected the description {new_description:?}, got {after:?}"
        );
        let updated_at = after["LastModifiedTime"].as_f64().unwrap_or_default();
        assert!(
            updated_at >= created_at,
            "case {case:?}: expected a last modified time at or after the create's \
             {created_at}, got {updated_at}"
        );
        // One statement was written, not another created: the name still names one
        // statement, in the workgroup the update addressed.
        assert_eq!(
            after["StatementName"].as_str(),
            Some(name.as_str()),
            "case {case:?}: expected the same statement, got {after:?}"
        );
    }
}

/// A delete removes the statement from its workgroup: a read of it afterwards reports it
/// as absent, every other statement is untouched, and the name can be created again.
#[tokio::test]
async fn a_delete_removes_one_statement_and_leaves_the_rest() {
    let router = router();
    const WORK_GROUP: &str = "prepared-statement-delete";
    create_work_group(&router, WORK_GROUP).await;

    // The statements the workgroup holds, and which of them each row deletes.
    let held = ["first", "second", "third"];
    for name in held {
        create_statement(
            &router,
            &json!({
                "WorkGroup": WORK_GROUP,
                "StatementName": name,
                "QueryStatement": format!("SELECT '{name}'"),
            }),
        )
        .await;
    }

    for (case, deleted) in [
        ("the first statement held", "first"),
        ("the last statement held", "third"),
        ("the one statement left", "second"),
    ] {
        let (status, removed) = call(
            &router,
            "DeletePreparedStatement",
            &json!({ "WorkGroup": WORK_GROUP, "StatementName": deleted }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the delete must be accepted, got {status} with {removed:?}"
        );

        let (status, gone) = read_statement(&router, WORK_GROUP, deleted).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a deleted statement must not be readable, got {status} with {gone:?}"
        );

        // A deleted name is free again, and the statement created under it is the new one.
        create_statement(
            &router,
            &json!({
                "WorkGroup": WORK_GROUP,
                "StatementName": deleted,
                "QueryStatement": format!("SELECT 'recreated {deleted}'"),
            }),
        )
        .await;
        let (status, recreated) = read_statement(&router, WORK_GROUP, deleted).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the recreated statement must be readable, got {status} with \
             {recreated:?}"
        );
        assert_eq!(
            recreated["QueryStatement"].as_str(),
            Some(format!("SELECT 'recreated {deleted}'").as_str()),
            "case {case:?}: expected the recreated query string, got {recreated:?}"
        );
    }
}

/// A batch read partitions the names it was given: one the workgroup holds a statement
/// under is reported from that statement, and one it does not is listed as an unprocessed
/// name with the reason — in the order the request listed them.
#[tokio::test]
async fn a_batch_read_reports_held_statements_and_lists_the_rest_as_unprocessed() {
    let router = router();
    const WORK_GROUP: &str = "prepared-statement-batch";
    create_work_group(&router, WORK_GROUP).await;
    let held = ["batch_one", "batch_two", "batch_three"];
    for name in held {
        create_statement(
            &router,
            &json!({
                "WorkGroup": WORK_GROUP,
                "StatementName": name,
                "QueryStatement": format!("SELECT '{name}'"),
            }),
        )
        .await;
    }

    let rows: Vec<BatchCase> = vec![
        BatchCase {
            case: "one name held",
            asked: vec!["batch_one"],
            expected_found: vec!["batch_one"],
            expected_unprocessed: vec![],
        },
        BatchCase {
            case: "one name the workgroup holds none under",
            asked: vec!["absent"],
            expected_found: vec![],
            expected_unprocessed: vec!["absent"],
        },
        BatchCase {
            case: "every name held, in reverse order",
            asked: vec!["batch_three", "batch_two", "batch_one"],
            expected_found: vec!["batch_three", "batch_two", "batch_one"],
            expected_unprocessed: vec![],
        },
        BatchCase {
            case: "held and absent names interleaved",
            asked: vec!["absent_one", "batch_two", "absent_two", "batch_one"],
            expected_found: vec!["batch_two", "batch_one"],
            expected_unprocessed: vec!["absent_one", "absent_two"],
        },
        BatchCase {
            case: "the same held name twice, which names one statement",
            asked: vec!["batch_one", "batch_one"],
            expected_found: vec!["batch_one"],
            expected_unprocessed: vec![],
        },
        BatchCase {
            case: "an absent name repeated beside a held one",
            asked: vec!["absent", "batch_two", "absent"],
            expected_found: vec!["batch_two"],
            expected_unprocessed: vec!["absent"],
        },
    ];

    for BatchCase {
        case,
        asked,
        expected_found,
        expected_unprocessed,
    } in rows
    {
        let (status, body) = call(
            &router,
            "BatchGetPreparedStatement",
            &json!({ "WorkGroup": WORK_GROUP, "PreparedStatementNames": asked }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the batch read must be accepted, got {status} with {body:?}"
        );

        let found: Vec<String> = body["PreparedStatements"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|statement| {
                statement["StatementName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            found, expected_found,
            "case {case:?}: expected the statements {expected_found:?}, got {found:?}"
        );

        let unprocessed = body["UnprocessedPreparedStatementNames"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let unprocessed_names: Vec<String> = unprocessed
            .iter()
            .map(|entry| {
                entry["StatementName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            unprocessed_names, expected_unprocessed,
            "case {case:?}: expected the unprocessed names {expected_unprocessed:?}, got \
             {unprocessed_names:?}"
        );
        for entry in &unprocessed {
            assert_eq!(
                entry["ErrorCode"].as_str(),
                Some(ERROR_CODE_STATEMENT_NOT_FOUND),
                "case {case:?}: expected the {ERROR_CODE_STATEMENT_NOT_FOUND} class, got {entry:?}"
            );
            assert_eq!(
                entry["ErrorMessage"].as_str(),
                Some(prepared_statement_name_not_found().as_str()),
                "case {case:?}: expected the documented reason, got {entry:?}"
            );
        }
        // Every statement reported carries the query string it was created with, so the
        // batch read answers out of the stored records.
        for statement in body["PreparedStatements"].as_array().unwrap_or(&Vec::new()) {
            let name = statement["StatementName"].as_str().unwrap_or_default();
            assert_eq!(
                statement["QueryStatement"].as_str(),
                Some(format!("SELECT '{name}'").as_str()),
                "case {case:?}: expected the stored query string of {name}, got {statement:?}"
            );
        }
    }
}

/// A batch read asks for at most the number of statements the operation documents it
/// returns: a request at that bound is served and one past it is refused.
#[tokio::test]
async fn a_batch_read_is_bounded_by_the_number_of_statements_it_can_return() {
    let router = router();
    const WORK_GROUP: &str = "prepared-statement-batch-bound";
    create_work_group(&router, WORK_GROUP).await;
    // One statement really held, so a served row reports something rather than only
    // unprocessed names.
    create_statement(
        &router,
        &json!({
            "WorkGroup": WORK_GROUP,
            "StatementName": "held",
            "QueryStatement": "SELECT 'held'",
        }),
    )
    .await;

    // (the case, how many names the request asks for) — each published bound and one past
    // it. The limit is the one `BatchGetPreparedStatement` documents ("a list of up to 256
    // prepared statements").
    let limit = MAX_PREPARED_STATEMENT_NAMES_PER_BATCH;
    for (case, count) in [
        ("one name", 1usize),
        ("one name short of the documented limit", limit - 1),
        ("exactly the documented limit", limit),
        ("one name past the documented limit", limit + 1),
    ] {
        let mut names: Vec<String> = (1..count).map(|index| format!("absent{index}")).collect();
        names.push("held".to_string());
        let (status, body) = call(
            &router,
            "BatchGetPreparedStatement",
            &json!({ "WorkGroup": WORK_GROUP, "PreparedStatementNames": names }),
        )
        .await;

        if count <= limit {
            assert_eq!(
                status, 200,
                "case {case:?}: {count} names must be served, got {status} with {body:?}"
            );
            assert_eq!(
                body["PreparedStatements"].as_array().map(Vec::len),
                Some(1),
                "case {case:?}: expected the one held statement, got {body:?}"
            );
            assert_eq!(
                body["UnprocessedPreparedStatementNames"]
                    .as_array()
                    .map(Vec::len),
                Some(count - 1),
                "case {case:?}: expected {} unprocessed names, got {body:?}",
                count - 1
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: {count} names must be refused, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                INVALID_REQUEST,
                "case {case:?}: expected {INVALID_REQUEST}, got {body:?}"
            );
            assert_eq!(
                message(&body),
                too_many_prepared_statement_names(count),
                "case {case:?}: expected the bound's sentence, got {:?}",
                message(&body)
            );
        }
    }
}

/// Every operation of this slice refuses a resource this account does not hold, each with
/// the code and the sentence Athena words that refusal with.
#[tokio::test]
async fn an_operation_naming_a_resource_this_account_does_not_hold_is_refused() {
    let router = router();
    const WORK_GROUP: &str = "prepared-statement-misses";
    const ABSENT_WORK_GROUP: &str = "never-created";
    const ABSENT: &str = "never_created";
    create_work_group(&router, WORK_GROUP).await;
    create_statement(
        &router,
        &json!({
            "WorkGroup": WORK_GROUP,
            "StatementName": "held",
            "QueryStatement": "SELECT 1",
        }),
    )
    .await;

    // (the case, the operation, the request, the wire code, the sentence)
    let rows: Vec<(&str, &str, Value, &str, String)> = vec![
        (
            "a create in a workgroup this account does not hold",
            "CreatePreparedStatement",
            json!({
                "WorkGroup": ABSENT_WORK_GROUP,
                "StatementName": ABSENT,
                "QueryStatement": "SELECT 1",
            }),
            INVALID_REQUEST,
            WORK_GROUP_NOT_FOUND.to_string(),
        ),
        (
            "a read of a statement the workgroup holds none of",
            "GetPreparedStatement",
            json!({ "WorkGroup": WORK_GROUP, "StatementName": ABSENT }),
            NOT_FOUND,
            no_prepared_statement_with_name(ABSENT, WORK_GROUP),
        ),
        (
            "a read of a statement whose name differs only in case from one held",
            "GetPreparedStatement",
            json!({ "WorkGroup": WORK_GROUP, "StatementName": "HELD" }),
            NOT_FOUND,
            no_prepared_statement_with_name("HELD", WORK_GROUP),
        ),
        (
            "a read in a workgroup this account does not hold",
            "GetPreparedStatement",
            json!({ "WorkGroup": ABSENT_WORK_GROUP, "StatementName": "held" }),
            NOT_FOUND,
            WORK_GROUP_NOT_FOUND.to_string(),
        ),
        (
            "an update of a statement the workgroup holds none of",
            "UpdatePreparedStatement",
            json!({
                "WorkGroup": WORK_GROUP,
                "StatementName": ABSENT,
                "QueryStatement": "SELECT 1",
            }),
            NOT_FOUND,
            prepared_statement_does_not_exist(ABSENT, WORK_GROUP),
        ),
        (
            "an update in a workgroup this account does not hold",
            "UpdatePreparedStatement",
            json!({
                "WorkGroup": ABSENT_WORK_GROUP,
                "StatementName": "held",
                "QueryStatement": "SELECT 1",
            }),
            NOT_FOUND,
            WORK_GROUP_NOT_FOUND.to_string(),
        ),
        (
            "a delete of a statement the workgroup holds none of",
            "DeletePreparedStatement",
            json!({ "WorkGroup": WORK_GROUP, "StatementName": ABSENT }),
            NOT_FOUND,
            prepared_statement_not_found(ABSENT, WORK_GROUP),
        ),
        (
            "a delete in a workgroup this account does not hold",
            "DeletePreparedStatement",
            json!({ "WorkGroup": ABSENT_WORK_GROUP, "StatementName": "held" }),
            NOT_FOUND,
            WORK_GROUP_NOT_FOUND.to_string(),
        ),
        (
            "a batch read in a workgroup this account does not hold",
            "BatchGetPreparedStatement",
            json!({
                "WorkGroup": ABSENT_WORK_GROUP,
                "PreparedStatementNames": ["held"],
            }),
            INVALID_REQUEST,
            WORK_GROUP_NOT_FOUND.to_string(),
        ),
    ];

    for (case, operation, request, expected_code, expected_message) in rows {
        let (status, refused) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            expected_code,
            "case {case:?}: expected {expected_code}, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            expected_message,
            "case {case:?}: expected the message {expected_message:?}, got {:?}",
            message(&refused)
        );
    }

    // Nothing above wrote the store: the one statement this account holds is still there,
    // and the names the misses used still name nothing.
    let (status, held) = read_statement(&router, WORK_GROUP, "held").await;
    assert_eq!(
        status, 200,
        "the held statement must still be readable, got {status} with {held:?}"
    );
    let (status, absent) = read_statement(&router, WORK_GROUP, ABSENT).await;
    assert_eq!(
        status, 400,
        "a refused create must have persisted nothing, got {status} with {absent:?}"
    );
}
