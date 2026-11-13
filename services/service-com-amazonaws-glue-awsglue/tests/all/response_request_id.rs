// SPDX-License-Identifier: CC0-1.0

//! The request a Glue response names itself with, end to end over the wire.
//!
//! Every row drives a real `awsJson1_1` request through the router the emulator mounts and
//! reads the RESPONSE HEADER an SDK surfaces as the call's request id — so what is asserted
//! is what a `glue` client (and any caller correlating its own logs with the service's) can
//! observe, not an internal call.
//!
//! The rule under test, asserted over a TABLE of answers rather than one recorded request:
//! every response this service sends names the request it answered in the
//! `x-amzn-requestid` header, whichever layer rendered it, and no two calls are named by one
//! id.
//!
//! The rows vary what the rule generalises over: whether the answer is a success or a
//! refusal, the HTTP status, and the LAYER that produced it — a handler's output, a
//! handler-raised modelled refusal, the refusal this service's front door words for a name
//! it was given none of, a constraint refusal the boundary validator raised while an
//! operation was being served, the scaffold's 501, and a routing failure no operation
//! produced at all.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call_reported, emulator, REGION};

/// The header AWS reports a response's own request id in, and the name an SDK reads its
/// response metadata's request id off. Written out as the literal a client sees rather than
/// taken from a constant of the service's, so changing that constant fails this suite
/// instead of moving with it.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// One answer of this service: the call that produces it, and the status it comes back
/// with.
struct Answer {
    /// What this row varies; named in every assertion.
    case: &'static str,
    /// The `X-Amz-Target` operation, which for the routing-failure row names no modelled
    /// operation at all.
    operation: &'static str,
    body: Value,
    status: u16,
}

/// Every layer of this service that can answer, with the resource the rows that read one
/// back created first.
async fn answers(router: &MultiServiceRouter) -> Vec<Answer> {
    // A database that exists, so a row below can read it back and another can collide with
    // its name.
    let created = call(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": "reported_db" } }),
    )
    .await;
    assert_eq!(
        created.0, 200,
        "the suite's own database must be created, got {} with {}",
        created.0, created.2
    );

    vec![
        Answer {
            case: "a mutating success",
            operation: "CreateDatabase",
            body: json!({ "DatabaseInput": { "Name": "reported_db_second" } }),
            status: 200,
        },
        Answer {
            case: "a reporting success",
            operation: "GetDatabase",
            body: json!({ "Name": "reported_db" }),
            status: 200,
        },
        Answer {
            case: "a listing success, whose answer carries a collection",
            operation: "GetDatabases",
            body: json!({}),
            status: 200,
        },
        Answer {
            // A handler-raised modelled refusal: the catalog holds no such table.
            case: "a handler-raised modelled refusal",
            operation: "GetTable",
            body: json!({ "DatabaseName": "reported_db", "Name": "no_such_table" }),
            status: 400,
        },
        Answer {
            // A collision this service refuses, on another status.
            case: "a re-create of a stored name",
            operation: "CreateDatabase",
            body: json!({ "DatabaseInput": { "Name": "reported_db" } }),
            status: 400,
        },
        Answer {
            // The refusal this service's own front door words for a request that supplied
            // no name at all (`crate::service`'s catalog wrapper), which no handler can
            // reach.
            case: "the front door's refusal of an absent name",
            operation: "CreateDatabase",
            body: json!({ "DatabaseInput": {} }),
            status: 400,
        },
        Answer {
            // A constraint refusal the boundary validator raised while an operation the
            // front door does NOT wrap was being served: `GetJobRun` declares both of its
            // members required.
            case: "a boundary constraint refusal",
            operation: "GetJobRun",
            body: json!({}),
            status: 400,
        },
        Answer {
            // The scaffold's 501: an answer a still-stubbed operation produced.
            case: "a scaffolded operation's 501",
            operation: "GetDevEndpoints",
            body: json!({}),
            status: 501,
        },
        Answer {
            // An answer NO operation produced: a target this service's model names no
            // operation for, rendered by the framework's routing-failure renderer.
            case: "a target naming no modelled operation",
            operation: "ThereIsNoSuchOperation",
            body: json!({}),
            status: 501,
        },
    ]
}

/// Make one `awsJson1_1` call and answer `(status, the request id it reported, body)`.
///
/// The request is built by the crate's own wire harness (`crate::wire::call_reported`), so
/// this suite states no request shape or credential scope of its own. The request id is
/// `None` for a response that named no request, which is the divergence this suite is
/// about.
async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Option<String>, Value) {
    let (status, headers, parsed) = call_reported(router, REGION, operation, body).await;
    let reported = headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    (status, reported, parsed)
}

/// Whether `id` has the form almost every AWS service names a request by: the 8-4-4-4-12
/// hex groups of a UUID.
///
/// Stated here rather than read off the service, so the FORM is pinned by this suite.
fn is_uuid_shaped(id: &str) -> bool {
    let mut groups = id.split('-');
    for width in [8usize, 4, 4, 4, 12] {
        match groups.next() {
            Some(group) => {
                if group.len() != width || !group.chars().all(|digit| digit.is_ascii_hexdigit()) {
                    return false;
                }
            }
            None => return false,
        }
    }
    groups.next().is_none()
}

/// RULE: every response this service sends names the request it answered in the
/// `x-amzn-requestid` header, whichever layer rendered it.
#[tokio::test]
async fn every_response_names_the_request_it_answered() {
    let router = emulator();

    for answer in answers(&router).await {
        let case = answer.case;
        let (status, reported, body) = call(&router, answer.operation, &answer.body).await;
        assert_eq!(
            status, answer.status,
            "case {case:?}: expected HTTP {}, got {status} with {body}",
            answer.status
        );
        let reported = reported.unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected the response to name its request in \
                 {REQUEST_ID_HEADER:?}, got none and the body {body}"
            )
        });
        assert!(
            !reported.is_empty() && is_uuid_shaped(&reported),
            "case {case:?}: expected a UUID-shaped request id, got {reported:?}"
        );
    }
}

/// RULE (the id is the CALL's): two answers never name the same request, whether they are
/// successes or refusals, and whichever layer rendered them.
#[tokio::test]
async fn two_responses_never_name_the_same_request() {
    let router = emulator();
    let mut reported: Vec<String> = Vec::new();

    for answer in answers(&router).await {
        // Twice per row, so the id is shown to be per CALL rather than per answer kind.
        for attempt in 1..=2 {
            let case = format!("{} (attempt {attempt})", answer.case);
            let (_, named, body) = call(&router, answer.operation, &answer.body).await;
            let named = named.unwrap_or_else(|| {
                panic!("case {case:?}: expected a reported request id, got the body {body}")
            });
            reported.push(named);
        }
    }

    let mut distinct = reported.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        reported.len(),
        "case every answered call: expected one request id per call, got {reported:?}"
    );
}
