// SPDX-License-Identifier: CC0-1.0

//! WHO may act on a workgroup, driven end to end over the wire (awsJson1_1,
//! `x-amz-target: AmazonAthena.<Operation>`) through the same router the emulator boots, so
//! what is asserted is what a client holding those credentials sees.
//!
//! THE RULE UNDER TEST: a workgroup is an IAM resource of this service
//! (`arn:aws:athena:<region>:<account>:workgroup/<WorkGroupName>`), so an operation whose
//! request NAMES a workgroup is authorized against THAT workgroup's ARN for the action
//! `athena:<OperationName>`, out of the identity-based policies in force on the principal
//! the request's access key id speaks for, BEFORE the operation reads or writes anything. A
//! request no statement allows (IAM's default deny), or one an explicit `Deny` covers, is
//! answered `AccessDeniedException` (HTTP 400) naming the action it asked for and performs
//! nothing; a request a matching `Allow` covers reaches the operation's own answer. An
//! operation whose workgroup member is optional addresses the account's default workgroup
//! when the request names none.
//!
//! The user, its access key and its policy live in the IAM store of the SAME shared engine —
//! the store IAM's own handlers write — so the documents under test are the ones
//! `iam:PutUserPolicy` stores and the principal a refusal blames is the one the access key
//! belongs to.
//!
//! The tables span what the rule generalises over: the OPERATION (every implemented one
//! whose request names a workgroup), the WORKGROUP the request names (the granted one, a
//! non-granted one, and none at all), the DOCUMENT in force (none, another action of this
//! service, another service's actions, an explicit deny beside an allow, an allow of exactly
//! the action, of the whole service, of everything), the RESOURCE an allow is scoped to (one
//! workgroup's ARN, every workgroup's, another workgroup's, every resource) and the
//! PRINCIPAL (an IAM user and the account's own published credential).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_athena_amazonathena::state::DEFAULT_WORK_GROUP;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call_as, engine_and_router, error_code, message, request_token, ACCOUNT_ACCESS_KEY_ID,
};

/// This service's IAM action prefix, which is its `arnNamespace` and its signing name
/// (`smithy_query(query="service_info")`).
const ACTION_PREFIX: &str = "athena";

/// The access key id the IAM user signs with, and the user it belongs to.
const USER_KEY_ID: &str = "AKIAEXAMPLEATHENAUSER";
const USER_NAME: &str = "athena-analyst";

/// The name of that user's inline policy.
const POLICY_NAME: &str = "inline";

/// The workgroup the policies under test grant on, and one they do not.
const GRANTED: &str = "granted-work-group";
const DENIED: &str = "other-work-group";

/// Where the queries in this suite write their results: a query has to name a location or
/// inherit one. Nothing is written to it for a refused call.
const OUTPUT_LOCATION: &str = "s3://athena-results/authorization/";

/// The Region the harness signs its requests for, which is the Region the ARN a request is
/// judged against carries.
const REGION: &str = "us-east-1";

/// The ARN of the workgroup `work_group`, as this service publishes it.
fn work_group_arn(work_group: &str) -> String {
    format!(
        "arn:aws:{ACTION_PREFIX}:{REGION}:{}:workgroup/{work_group}",
        ::engine::emulator_account_id()
    )
}

/// The ARN the account holds the IAM user under, which a refusal names as the caller.
fn user_arn() -> String {
    format!(
        "arn:aws:iam::{}:user/{USER_NAME}",
        ::engine::emulator_account_id()
    )
}

/// Record the IAM user [`USER_KEY_ID`] belongs to, as `iam:CreateUser` and
/// `iam:CreateAccessKey` record one: the user, and the key record that maps the access key id
/// to it. NO policy is attached.
fn given_user_key(engine: &Arc<engine::InMemoryEngine>) {
    let minted = ::aws_smithy_types::DateTime::from_secs(0);
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.users.insert(
        iam::name_key(USER_NAME),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::User {
            path: "/".to_string(),
            user_name: USER_NAME.to_string(),
            user_id: "AIDAEXAMPLEATHENAANALYST".to_string(),
            arn: user_arn(),
            create_date: minted,
            password_last_used: None,
            permissions_boundary: None,
            tags: None,
        },
    );
    store.access_keys.insert(
        USER_KEY_ID.to_string(),
        iam::AccessKeyRecord {
            user_key: iam::name_key(USER_NAME),
            status: iam::ACCESS_KEY_ACTIVE_STATUS.to_string(),
            create_date: minted,
        },
    );
}

/// Attach `document` to the user as its inline policy, replacing whatever it carried — what
/// `iam:PutUserPolicy` does. `None` takes every policy off it, as `iam:DeleteUserPolicy`
/// does.
fn given_user_policy(engine: &Arc<engine::InMemoryEngine>, document: Option<&Value>) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.user_inline_policies.remove(&iam::name_key(USER_NAME));
    if let Some(document) = document {
        store.user_inline_policies.insert(
            iam::name_key(USER_NAME),
            BTreeMap::from([(
                iam::name_key(POLICY_NAME),
                iam::InlinePolicy {
                    policy_name: POLICY_NAME.to_string(),
                    policy_document: document.to_string(),
                },
            )]),
        );
    }
}

/// A policy allowing `actions` on `resource`.
fn allow(actions: Value, resource: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": resource }],
    })
}

/// One operation whose request names a workgroup: its name (which is also its action's API
/// name) and the body addressing a given workgroup.
struct Gated {
    name: &'static str,
    body: fn(&str) -> Value,
}

/// Every IMPLEMENTED operation of this service whose request names a workgroup. The rule
/// covers all of them, so the table is that whole set rather than a sample of it: the
/// workgroup is addressed under `WorkGroup` by all but the create, which names the workgroup
/// it would create under `Name`, and it is OPTIONAL on the two that default to the account's
/// own workgroup (`smithy_query(query="operation",operation="StartQueryExecution")`).
fn gated_operations() -> Vec<Gated> {
    vec![
        Gated {
            name: "StartQueryExecution",
            body: |work_group| {
                json!({
                    "QueryString": "SELECT 1 AS one",
                    "WorkGroup": work_group,
                    "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION },
                    "ClientRequestToken": request_token(work_group),
                })
            },
        },
        Gated {
            name: "ListQueryExecutions",
            body: |work_group| json!({ "WorkGroup": work_group, "MaxResults": 50 }),
        },
        Gated {
            name: "CreateWorkGroup",
            body: |work_group| json!({ "Name": work_group }),
        },
        Gated {
            name: "GetWorkGroup",
            body: |work_group| json!({ "WorkGroup": work_group }),
        },
        Gated {
            name: "UpdateWorkGroup",
            body: |work_group| json!({ "WorkGroup": work_group, "Description": "updated" }),
        },
        Gated {
            name: "CreatePreparedStatement",
            body: |work_group| {
                json!({
                    "StatementName": "statement_one",
                    "WorkGroup": work_group,
                    "QueryStatement": "SELECT ?",
                })
            },
        },
        Gated {
            name: "GetPreparedStatement",
            body: |work_group| {
                json!({ "StatementName": "statement_one", "WorkGroup": work_group })
            },
        },
        Gated {
            name: "UpdatePreparedStatement",
            body: |work_group| {
                json!({
                    "StatementName": "statement_one",
                    "WorkGroup": work_group,
                    "QueryStatement": "SELECT ?, ?",
                })
            },
        },
        Gated {
            name: "DeletePreparedStatement",
            body: |work_group| {
                json!({ "StatementName": "statement_one", "WorkGroup": work_group })
            },
        },
        Gated {
            name: "BatchGetPreparedStatement",
            body: |work_group| {
                json!({
                    "PreparedStatementNames": ["statement_one"],
                    "WorkGroup": work_group,
                })
            },
        },
        Gated {
            name: "ImportNotebook",
            body: |work_group| {
                json!({
                    "WorkGroup": work_group,
                    "Name": "notebook-one",
                    "Type": "IPYNB",
                    "Payload": "{\"cells\":[]}",
                })
            },
        },
    ]
}

/// A router over one shared engine, with both workgroups the tables name already created by
/// the account's own credential, and the IAM user recorded carrying `document`.
async fn given(document: Option<&Value>) -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (engine, router) = engine_and_router();
    for work_group in [GRANTED, DENIED] {
        let (status, created) = call_as(
            &router,
            ACCOUNT_ACCESS_KEY_ID,
            "CreateWorkGroup",
            &json!({ "Name": work_group }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case setup: the workgroup {work_group} must be created, got {status} with \
             {created:?}"
        );
    }
    given_user_key(&engine);
    given_user_policy(&engine, document);
    (router, engine)
}

/// Assert that `(status, body)` is this service's authorization refusal for `action`.
///
/// Athena words such a refusal over the ACTION alone, between fixed text, naming neither the
/// calling principal nor the resource the request addressed.
fn assert_refused(case: &str, status: u16, body: &Value, action: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(body),
        "AccessDeniedException",
        "case {case:?}: expected an AccessDeniedException, got {body:?}"
    );
    let stated = format!(
        "You are not authorized to perform: {action} on the resource. After your AWS \
         administrator or you have updated your permissions, please try again."
    );
    assert_eq!(
        message(body),
        stated,
        "case {case:?}: expected Athena's wording {stated:?}, got {:?}",
        message(body)
    );
}

/// THE RULE: an allow scoped to ONE workgroup's ARN covers the operations that name that
/// workgroup and no other — the identical request with only the workgroup changed is refused
/// with `AccessDeniedException` naming the workgroup it named.
#[tokio::test]
async fn an_operation_is_authorized_against_the_work_group_the_request_names() {
    for operation in gated_operations() {
        let action = format!("{ACTION_PREFIX}:{}", operation.name);
        let document = allow(json!(format!("{ACTION_PREFIX}:*")), json!(work_group_arn(GRANTED)));
        let (router, _engine) = given(Some(&document)).await;

        let (status, body) =
            call_as(&router, USER_KEY_ID, operation.name, &(operation.body)(GRANTED)).await;
        // Authorization answers BEFORE anything is read or written, so an allowed call falls
        // through to the operation's own verdict — which for a create of a workgroup this
        // account already holds is that refusal, not another authorization decision.
        assert_ne!(
            error_code(&body),
            "AccessDeniedException",
            "case {:?} on the granted workgroup: an allowed request must not be refused, got \
             {status} with {body:?}",
            operation.name
        );

        let (status, body) =
            call_as(&router, USER_KEY_ID, operation.name, &(operation.body)(DENIED)).await;
        assert_refused(
            &format!("{} on the non-granted workgroup", operation.name),
            status,
            &body,
            &action,
        );
        assert!(
            !body["Message"]
                .as_str()
                .unwrap_or_default()
                .contains(DENIED),
            "case {:?}: a refusal names no workgroup, got {body:?}",
            operation.name
        );
    }
}

/// RULE: the decision rests on the documents in force on the principal — an explicit `Deny`
/// wins, an `Allow` about the request grants it, and with neither the request falls to IAM's
/// default deny.
#[tokio::test]
async fn the_decision_rests_on_the_documents_in_force() {
    let action = format!("{ACTION_PREFIX}:StartQueryExecution");
    // (case, the document in force, whether the request is then refused)
    let rows: Vec<(&str, Option<Value>, bool)> = vec![
        ("no policy at all", None, true),
        (
            "an allow of another action of this service",
            Some(allow(
                json!(format!("{ACTION_PREFIX}:ListQueryExecutions")),
                json!("*"),
            )),
            true,
        ),
        (
            "an allow of every action of another service",
            Some(allow(json!("glue:*"), json!("*"))),
            true,
        ),
        (
            "an explicit deny of this action beside an allow of it",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": action, "Resource": "*" },
                    { "Effect": "Deny", "Action": action, "Resource": "*" },
                ],
            })),
            true,
        ),
        (
            "an explicit deny of this whole service beside an allow of exactly this action",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": action, "Resource": "*" },
                    {
                        "Effect": "Deny",
                        "Action": format!("{ACTION_PREFIX}:*"),
                        "Resource": "*",
                    },
                ],
            })),
            true,
        ),
        (
            "an allow of exactly this action on this workgroup",
            Some(allow(json!(action), json!(work_group_arn(GRANTED)))),
            false,
        ),
        (
            // IAM compares action names case-insensitively, so the spelling of the allow is
            // not what decides the request.
            "an allow of this action spelled in lower case",
            Some(allow(
                json!(action.to_lowercase()),
                json!(work_group_arn(GRANTED)),
            )),
            false,
        ),
        (
            "an allow of every action of this service on every resource",
            Some(allow(json!(format!("{ACTION_PREFIX}:*")), json!("*"))),
            false,
        ),
        (
            "an allow of every action on every resource",
            Some(allow(json!("*"), json!("*"))),
            false,
        ),
    ];

    for (case, document, refused) in rows {
        let (router, _engine) = given(document.as_ref()).await;
        let (status, body) = call_as(
            &router,
            USER_KEY_ID,
            "StartQueryExecution",
            &json!({
                "QueryString": "SELECT 1 AS one",
                "WorkGroup": GRANTED,
                "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION },
            }),
        )
        .await;
        if refused {
            assert_refused(case, status, &body, &action);
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: an allowed submission must be served, got {status} with \
                 {body:?}"
            );
            assert!(
                body["QueryExecutionId"].as_str().is_some(),
                "case {case:?}: an allowed submission must mint an id, got {body:?}"
            );
        }
    }
}

/// RULE: the resource a statement is scoped to decides which workgroups it covers — IAM
/// matches resource ARNs with its own `*` and `?` wildcards, and an ARN naming another
/// workgroup covers nothing the request named.
#[tokio::test]
async fn an_allow_covers_exactly_the_work_groups_its_resource_matches() {
    let action = format!("{ACTION_PREFIX}:StartQueryExecution");
    // (case, the Resource the allow is scoped to, whether it covers a query in GRANTED)
    let rows: Vec<(&str, Value, bool)> = vec![
        ("every resource", json!("*"), true),
        (
            "exactly this workgroup's ARN",
            json!(work_group_arn(GRANTED)),
            true,
        ),
        (
            "every workgroup of this account and Region",
            json!(work_group_arn("*")),
            true,
        ),
        (
            "this workgroup's ARN with one character wildcarded",
            json!(work_group_arn(&GRANTED.replacen('g', "?", 1))),
            true,
        ),
        (
            "every workgroup of every Region",
            json!(format!("arn:aws:{ACTION_PREFIX}:*:*:workgroup/*")),
            true,
        ),
        (
            "another workgroup's ARN",
            json!(work_group_arn(DENIED)),
            false,
        ),
        (
            "a prefix of this workgroup's name with no wildcard",
            json!(work_group_arn(&GRANTED[..GRANTED.len() - 1])),
            false,
        ),
        (
            "another service's resource",
            json!(format!("arn:aws:glue:{REGION}:*:catalog")),
            false,
        ),
    ];

    for (case, resource, covered) in rows {
        let (router, _engine) = given(Some(&allow(json!(action.clone()), resource.clone()))).await;
        let (status, body) = call_as(
            &router,
            USER_KEY_ID,
            "StartQueryExecution",
            &json!({
                "QueryString": "SELECT 1 AS one",
                "WorkGroup": GRANTED,
                "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION },
            }),
        )
        .await;
        let refused = error_code(&body) == "AccessDeniedException";
        assert_eq!(
            refused, !covered,
            "case {case:?}: an allow scoped to {resource} must {}cover a query in {GRANTED}, \
             got {status} with {body:?}",
            if covered { "" } else { "not " }
        );
    }
}

/// RULE: an operation whose workgroup member is optional addresses the account's DEFAULT
/// workgroup when the request names none, so that is the ARN the decision is taken against.
#[tokio::test]
async fn a_request_naming_no_work_group_is_judged_against_the_default_work_group() {
    let action = format!("{ACTION_PREFIX}:StartQueryExecution");
    // (case, the workgroup the allow is scoped to, whether a request naming none is covered)
    let rows: &[(&str, &str, bool)] = &[
        ("an allow on the default workgroup", DEFAULT_WORK_GROUP, true),
        ("an allow on another workgroup", GRANTED, false),
    ];

    for (case, scoped_to, covered) in rows {
        let (router, _engine) =
            given(Some(&allow(json!(action.clone()), json!(work_group_arn(scoped_to))))).await;
        let (status, body) = call_as(
            &router,
            USER_KEY_ID,
            "StartQueryExecution",
            &json!({
                "QueryString": "SELECT 1 AS one",
                "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION },
            }),
        )
        .await;
        if *covered {
            assert_eq!(
                status, 200,
                "case {case:?}: a submission to the default workgroup must be served, got \
                 {status} with {body:?}"
            );
        } else {
            assert_refused(case, status, &body, &action);
        }
    }
}

/// RULE: the decision is taken before the operation performs anything, so a REFUSED
/// submission mints no query execution — the workgroup's history is exactly as it was — and a
/// refused write leaves the workgroup it named untouched.
#[tokio::test]
async fn a_refused_call_performs_nothing() {
    let (router, _engine) = given(Some(&allow(
        json!(format!("{ACTION_PREFIX}:*")),
        json!(work_group_arn(GRANTED)),
    )))
    .await;

    let (status, body) = call_as(
        &router,
        USER_KEY_ID,
        "StartQueryExecution",
        &json!({
            "QueryString": "SELECT 1 AS one",
            "WorkGroup": DENIED,
            "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION },
        }),
    )
    .await;
    assert_eq!(
        error_code(&body),
        "AccessDeniedException",
        "case a refused submission: expected the refusal, got {status} with {body:?}"
    );
    assert!(
        body["QueryExecutionId"].as_str().is_none(),
        "case a refused submission: a refusal carries no id, got {body:?}"
    );

    // Read back with the account's own credential, which this gate does not bound: the
    // refused submission left no execution behind.
    let (status, listed) = call_as(
        &router,
        ACCOUNT_ACCESS_KEY_ID,
        "ListQueryExecutions",
        &json!({ "WorkGroup": DENIED, "MaxResults": 50 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the history after a refusal: expected HTTP 200, got {status} with {listed:?}"
    );
    let ids = listed["QueryExecutionIds"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        ids.is_empty(),
        "case the history after a refusal: expected no execution, got {ids:?}"
    );

    // And a refused update changed nothing of the workgroup it named.
    let (status, updated) = call_as(
        &router,
        USER_KEY_ID,
        "UpdateWorkGroup",
        &json!({ "WorkGroup": DENIED, "Description": "rewritten by an unauthorized caller" }),
    )
    .await;
    assert_eq!(
        error_code(&updated),
        "AccessDeniedException",
        "case a refused update: expected the refusal, got {status} with {updated:?}"
    );
    let (status, read) = call_as(
        &router,
        ACCOUNT_ACCESS_KEY_ID,
        "GetWorkGroup",
        &json!({ "WorkGroup": DENIED }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the workgroup after a refused update: expected HTTP 200, got {status} with \
         {read:?}"
    );
    assert!(
        read["WorkGroup"]["Description"].is_null(),
        "case the workgroup after a refused update: expected no description, got {read:?}"
    );
}

/// RULE: the account's own published credential — a key neither STS nor IAM holds a record
/// for — is not a principal the IAM store holds policies for, so it is served whatever any
/// user's policies say. This is what every other suite of this crate signs with.
#[tokio::test]
async fn the_accounts_own_credential_is_served() {
    // The user carries nothing at all, which is what refuses it above.
    let (router, _engine) = given(None).await;

    for operation in gated_operations() {
        let (status, body) = call_as(
            &router,
            ACCOUNT_ACCESS_KEY_ID,
            operation.name,
            &(operation.body)(DENIED),
        )
        .await;
        assert_ne!(
            error_code(&body),
            "AccessDeniedException",
            "case {:?}: the account's own credential must not be refused, got {status} with \
             {body:?}",
            operation.name
        );
    }
}

/// RULE: a refusal discloses nothing beyond IAM's sentence — not what the workgroup holds,
/// and not whether this account holds it at all, so a workgroup that exists and one that does
/// not are refused identically.
#[tokio::test]
async fn a_refusal_discloses_nothing_about_the_work_group() {
    let action = format!("{ACTION_PREFIX}:GetWorkGroup");
    let (router, _engine) = given(Some(&allow(json!(action.clone()), json!(work_group_arn(GRANTED)))))
        .await;

    // (case, the workgroup the request names — one this account holds, and one it does not)
    let rows: &[(&str, &str)] = &[
        ("a workgroup this account holds", DENIED),
        ("a workgroup nothing created", "no-such-work-group"),
    ];
    for (case, work_group) in rows {
        let (status, body) = call_as(
            &router,
            USER_KEY_ID,
            "GetWorkGroup",
            &json!({ "WorkGroup": work_group }),
        )
        .await;
        assert_refused(case, status, &body, &action);
        let reported: Vec<String> = body
            .as_object()
            .map(|object| {
                object
                    .keys()
                    .filter(|member| {
                        *member != "Message" && *member != "message" && *member != "__type"
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            reported.is_empty(),
            "case {case:?}: a refused call reports only the error, got {reported:?}"
        );
    }
}
