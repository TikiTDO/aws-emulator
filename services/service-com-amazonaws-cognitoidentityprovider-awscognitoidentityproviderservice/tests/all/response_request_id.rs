// SPDX-License-Identifier: CC0-1.0

//! Every response of this service NAMES THE REQUEST it answers, over the wire (`POST /`
//! with `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! RULE: a response reports the id of the request it answers in the header this protocol
//! reports that id in (`x-amzn-RequestId`, matched case-insensitively), freshly minted per
//! request — on a SUCCESS exactly as on a failure. It is a property of the RESPONSE rather
//! than of any modelled shape: no output and no error shape of this model declares such a
//! member (`smithy_query(query="operation",operation="AdminGetUser,CreateUserPool")`), so no
//! handler can set it, and it is what an SDK surfaces as `ResponseMetadata.RequestId` on a
//! success and on a `ClientError` alike — the value a client correlating a call with a
//! record of it joins on.
//!
//! The rows span what the rule generalises over: the LAYER that answered — a handler's
//! success, a handler's own modelled error, a refusal the framework raised before any
//! handler ran (a value outside a modelled constraint), and a request naming no modelled
//! operation at all, which no operation of this service ever sees — and, across the table,
//! that two responses never name the same request.
//!
//! The router, the one call and the fixtures are `crate::error_wire`'s, so this suite drives
//! the same endpoint every other wire suite does rather than a second copy of it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::error_wire::{answered_with_headers, ok, router, UNMINTED_POOL};

/// The response header this protocol reports the id of the answered request in.
const REQUEST_ID_HEADER: &str = "x-amzn-RequestId";

/// Whether `id` has the FORM of an AWS request id: the 8-4-4-4-12 hex groups of an RFC 4122
/// UUID. The VALUE is the response's own and is never asserted — only its form, and that no
/// two responses share one.
fn names_a_request(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(width, group)| {
                group.len() == *width && group.chars().all(|c| c.is_ascii_hexdigit())
            })
}

/// The id the answer to `operation` names its request by, with the status and body beside it.
async fn reported(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> (u16, String, Value) {
    let (status, headers, payload) = answered_with_headers(router, operation, body).await;
    let reported = headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a {REQUEST_ID_HEADER} header on the answer to \
                 {operation}, got {headers:?} with {payload}"
            )
        })
        .to_string();
    (status, reported, payload)
}

/// RULE: every response names the request it answers, whichever layer answered it, and no two
/// responses name the same request.
#[tokio::test]
async fn every_response_names_the_request_it_answers() {
    let router = router();
    // One live pool with one user, so a row that must be SERVED is served and a row that must
    // be refused is refused for the reason it names.
    let pool = ok(
        &router,
        "CreateUserPool",
        &json!({ "PoolName": "request-id" }),
    )
    .await;
    let user_pool_id = pool["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    ok(
        &router,
        "AdminCreateUser",
        &json!({ "UserPoolId": &user_pool_id, "Username": "alice" }),
    )
    .await;

    // (case, operation, body, expected status)
    let rows: Vec<(&str, &str, Value, u16)> = vec![
        (
            "a read that succeeds",
            "AdminGetUser",
            json!({ "UserPoolId": &user_pool_id, "Username": "alice" }),
            200,
        ),
        (
            "a write that succeeds",
            "AdminUpdateUserAttributes",
            json!({
                "UserPoolId": &user_pool_id,
                "Username": "alice",
                "UserAttributes": [{"Name": "email", "Value": "alice@example.com"}],
            }),
            200,
        ),
        (
            "a listing that succeeds",
            "ListUserPools",
            json!({ "MaxResults": 10 }),
            200,
        ),
        (
            "a handler's own modelled refusal",
            "AdminGetUser",
            json!({ "UserPoolId": &user_pool_id, "Username": "absent" }),
            400,
        ),
        (
            "a refusal about a pool nothing minted",
            "AdminGetUser",
            json!({ "UserPoolId": UNMINTED_POOL, "Username": "alice" }),
            400,
        ),
        (
            "a refusal the framework raised before any handler ran",
            "AdminGetUser",
            json!({ "UserPoolId": &user_pool_id }),
            400,
        ),
        (
            "a request naming no modelled operation",
            "NoSuchOperation",
            json!({}),
            501,
        ),
    ];

    let mut named: Vec<String> = Vec::new();
    for (case, operation, body, expected_status) in rows {
        let (status, reported, payload) = reported(&router, case, operation, &body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {payload}"
        );
        assert!(
            names_a_request(&reported),
            "case {case:?}: expected a request id of the 8-4-4-4-12 hex form, got {reported:?}"
        );
        assert!(
            !named.contains(&reported),
            "case {case:?}: expected each response to name its own request, got {reported} twice"
        );
        named.push(reported);
    }
}
