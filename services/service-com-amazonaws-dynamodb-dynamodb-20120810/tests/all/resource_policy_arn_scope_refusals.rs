// SPDX-License-Identifier: CC0-1.0

//! How the resource-policy surface refuses a `ResourceArn` that is a well-formed ARN but
//! addresses no resource of the endpoint the request reached.
//!
//! One rule, asserted over a set of inputs rather than the one recorded example: a
//! `ResourceArn` whose ARN scope — partition, region or account — is not the scope of the
//! endpoint the request was signed for, the empty region field every global service's ARN
//! carries included, names no resource that could be looked up from here, so every
//! `*ResourcePolicy` operation refuses it as INPUT with HTTP 400 `ValidationException` and
//! the sentence "One or more parameter values were invalid: Invalid resource Arn: " followed
//! by the value exactly as the request spelled it.
//!
//! The adjacent rules bound it, and are asserted side by side so the classification itself
//! is pinned rather than one branch of it:
//!
//! * a value that is not an ARN at all is refused by THAT rule ("ARNs must start with
//!   'arn:': <value>"), because the member's whole shape is wrong;
//! * an in-scope DynamoDB ARN naming something other than a table or that table's stream —
//!   an index, a backup — is refused for the resource it names ("Invalid resource arn
//!   provided, only table or stream is accepted. Provided resource arn: <value>"), the two
//!   resources a resource-based policy attaches to being a table and a stream
//!   (`smithy_query(query="doc",shape="GetResourcePolicy")`: "the resource, which can be a
//!   table or stream").
//!
//! The expected sentences are written out here rather than taken from the crate, so the
//! wording is what is pinned. The rows differ along the ARN field that leaves the
//! endpoint's scope (region, account, partition, an absent region), the service the ARN
//! names, whether it addresses a table or a table's stream, and the length and character
//! classes of the table name; every row is driven through all three operations whose whole
//! selector is a `ResourceArn`.
//!
//! Driven over the wire (`POST /` with `X-Amz-Target: DynamoDB_20120810.<Operation>`)
//! through the same `MultiServiceRouter` the emulator's front doors use, reusing the
//! harness in [`super::table_items`].

use serde_json::{json, Value};

use super::table_items::{call, create_active_table, emulator, ok};
use crate::emulator::{error_code, message};

/// The region the harness's credential scope names, which is the scope every ARN this
/// endpoint can address lives in.
const REGION: &str = "us-east-1";

/// The account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// The table these rows address, created so that no row can be refused merely because
/// nothing of that name exists: the refusal under test happens before any lookup.
const TABLE: &str = "PolicyArnScope";

/// The fixed text of the refusal an out-of-scope ARN earns, written out so this suite
/// asserts the wording and not the emulator's own copy of it.
const OUT_OF_SCOPE_PREFIX: &str = "One or more parameter values were invalid: Invalid resource Arn: ";

/// The fixed text of the refusal a value that is not an ARN earns.
const NOT_AN_ARN_PREFIX: &str = "One or more parameter values were invalid: ARNs must start with 'arn:': ";

/// The fixed text of the refusal an in-scope ARN naming neither a table nor a stream earns.
const UNSUPPORTED_RESOURCE_PREFIX: &str = "One or more parameter values were invalid: Invalid \
     resource arn provided, only table or stream is accepted. Provided resource arn: ";

/// A resource-policy document this emulator accepts, so that a `PutResourcePolicy` row is
/// refused for its `ResourceArn` and not for its document.
fn policy(resource_arn: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Deny",
            "Principal": {"AWS": "*"},
            "Action": "dynamodb:GetItem",
            "Resource": resource_arn,
        }],
    })
    .to_string()
}

/// Every operation whose whole selector is a `ResourceArn`, with a body that is otherwise
/// valid, over the value under test.
fn policy_operations(resource_arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("GetResourcePolicy", json!({"ResourceArn": resource_arn})),
        (
            "PutResourcePolicy",
            json!({"ResourceArn": resource_arn, "Policy": policy(resource_arn)}),
        ),
        ("DeleteResourcePolicy", json!({"ResourceArn": resource_arn})),
    ]
}

/// Drive one `ResourceArn` through all three operations and assert each answers 400
/// `ValidationException` with `expected`.
async fn each_operation_refuses(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    resource_arn: &str,
    expected: &str,
) {
    for (operation, body) in policy_operations(resource_arn) {
        let (status, answered) = call(router, operation, &body).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {case:?} ({operation}): expected 400 ValidationException, got {status} with \
             {answered}"
        );
        assert_eq!(
            message(&answered),
            expected,
            "case {case:?} ({operation}): expected {expected:?}, got {:?}",
            message(&answered)
        );
    }
}

/// An ARN whose scope is not this endpoint's — including the empty region field a global
/// service's ARN carries — is refused by the scope rule, whichever resource-policy
/// operation it is sent to, and the value is echoed exactly as it was sent.
#[tokio::test]
async fn an_out_of_scope_resource_arn_is_refused_by_the_scope_rule() {
    let router = emulator();
    create_active_table(&router, TABLE, &[("pk", "HASH", "S")]).await;
    // A second table whose name sits at the other end of the published name range
    // (`[A-Za-z0-9_.-]{3,255}`), so the echoed part of the message is exercised over more
    // than one spelling.
    let long_name = format!("{TABLE}{}", "x".repeat(255 - TABLE.len()));
    create_active_table(&router, &long_name, &[("pk", "HASH", "S")]).await;

    let rows: Vec<(&str, String)> = vec![
        // The class the issue cited: a global service's ARN, whose region field is empty,
        // so it can name no resource of a regional endpoint.
        (
            "a role ARN of the global identity service",
            format!("arn:aws:iam::{ACCOUNT}:role/not-a-dynamodb-resource"),
        ),
        (
            "an account ARN of another global service",
            format!("arn:aws:organizations::{ACCOUNT}:account/o-example/111122223333"),
        ),
        (
            "a DynamoDB table ARN with no region",
            format!("arn:aws:dynamodb::{ACCOUNT}:table/{TABLE}"),
        ),
        // The scope fields, one at a time.
        (
            "this table's ARN in another region",
            format!("arn:aws:dynamodb:eu-west-1:{ACCOUNT}:table/{TABLE}"),
        ),
        (
            "this table's ARN in another account",
            format!("arn:aws:dynamodb:{REGION}:111122223333:table/{TABLE}"),
        ),
        (
            "this table's ARN in another partition",
            format!("arn:aws-cn:dynamodb:{REGION}:{ACCOUNT}:table/{TABLE}"),
        ),
        // A STREAM ARN is a resource this surface does address, so an out-of-scope one is
        // refused by the scope rule too and not as an unsupported resource.
        (
            "a stream ARN in another region",
            format!("arn:aws:dynamodb:ap-south-1:{ACCOUNT}:table/{TABLE}/stream/2026-01-01T00:00:00.000"),
        ),
        (
            "a stream ARN in another account",
            format!("arn:aws:dynamodb:{REGION}:111122223333:table/{TABLE}/stream/2026-01-01T00:00:00.000"),
        ),
        // The longest table name a DynamoDB table can carry, out of scope by its region.
        (
            "the longest table name's ARN in another region",
            format!("arn:aws:dynamodb:eu-central-1:{ACCOUNT}:table/{long_name}"),
        ),
        // A truncated ARN: it begins with the scheme, so it is past the not-an-ARN rule,
        // and it names no addressable scope.
        ("an ARN missing its account and resource", "arn:aws:dynamodb:us-east-1".to_string()),
    ];

    for (case, resource_arn) in &rows {
        each_operation_refuses(
            &router,
            case,
            resource_arn,
            &format!("{OUT_OF_SCOPE_PREFIX}{resource_arn}"),
        )
        .await;
    }

    // None of the refusals attached anything to the table they spelled out of scope: the
    // in-scope table still carries no policy.
    let in_scope = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/{TABLE}");
    let (status, answered) = call(
        &router,
        "GetResourcePolicy",
        &json!({"ResourceArn": &in_scope}),
    )
    .await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "PolicyNotFoundException"),
        "case the in-scope table after every out-of-scope refusal: expected 400 \
         PolicyNotFoundException, got {status} with {answered}"
    );
}

/// The rules on either side of the scope rule, so the classification is pinned and not
/// just one branch of it: a value that is not an ARN, and an in-scope DynamoDB ARN naming
/// a resource this surface does not attach policies to.
#[tokio::test]
async fn the_adjacent_resource_arn_rules_keep_their_own_wording() {
    let router = emulator();
    create_active_table(&router, TABLE, &[("pk", "HASH", "S")]).await;
    let table_arn = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/{TABLE}");

    // Values that are not ARNs: the rule they break is the member's shape.
    let not_arns = [
        ("a bare table name", TABLE.to_string()),
        ("a bare name of no table", "absent-table".to_string()),
        (
            "an ARN with its scheme missing",
            format!("aws:dynamodb:{REGION}:{ACCOUNT}:table/{TABLE}"),
        ),
        ("a single character", "x".to_string()),
    ];
    for (case, value) in &not_arns {
        each_operation_refuses(
            &router,
            case,
            value,
            &format!("{NOT_AN_ARN_PREFIX}{value}"),
        )
        .await;
    }

    // In-scope DynamoDB ARNs that name neither a table nor that table's stream: the rule
    // they break is which resource they name.
    let unsupported = [
        (
            "an index of this table",
            format!("{table_arn}/index/by-rank"),
        ),
        (
            "a backup of this table",
            format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/{TABLE}/backup/01700000000000-abcdef12"),
        ),
        (
            "a stream ARN with an empty label",
            format!("{table_arn}/stream/"),
        ),
    ];
    for (case, value) in &unsupported {
        each_operation_refuses(
            &router,
            case,
            value,
            &format!("{UNSUPPORTED_RESOURCE_PREFIX}{value}"),
        )
        .await;
    }

    // And the in-scope table ARN itself is classified as addressable: it reaches the
    // lookup, where it round-trips a policy through the implemented reader.
    let document = policy(&table_arn);
    ok(
        &router,
        "PutResourcePolicy",
        json!({"ResourceArn": &table_arn, "Policy": &document}),
    )
    .await;
    let read = read_policy_when_visible(&router, &table_arn).await;
    assert_eq!(
        read["Policy"],
        json!(&document),
        "case the in-scope table ARN: the reader reports the document that was attached, got \
         {read}"
    );
}

/// Poll `GetResourcePolicy` until the attach the request accepted is in force — the reader
/// is eventually consistent ("If you issue a GetResourcePolicy request immediately after a
/// PutResourcePolicy request, DynamoDB might return a PolicyNotFoundException"), so the
/// wait is for the CONDITION and is bounded by the harness's generous deadline.
async fn read_policy_when_visible(
    router: &smithy_server_runtime::MultiServiceRouter,
    resource_arn: &str,
) -> Value {
    let started = std::time::Instant::now();
    let mut last = Value::Null;
    while started.elapsed() < crate::emulator::ACTIVE_DEADLINE {
        let (status, answered) = call(router, "GetResourcePolicy", &json!({"ResourceArn": resource_arn})).await;
        if status == 200 {
            return answered;
        }
        last = answered;
        tokio::time::sleep(crate::emulator::POLL_INTERVAL).await;
    }
    panic!(
        "case {resource_arn}: the attached policy never became readable within {:?}, last {last}",
        crate::emulator::ACTIVE_DEADLINE
    )
}
