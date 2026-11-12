// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two rules a request is held to BEFORE it reaches state:
//! the documented rules every attribute value must satisfy, and the resource-based
//! policy attached to the table.
//!
//! Both are asserted over a CLASS of inputs rather than one example. The key-size rows
//! differ along the key's role, its type, its size against the documented limit and
//! whether its bytes and its characters agree; the value rows differ along the set
//! type, where the set sits in the item, and which numeric rule a number breaks; the
//! policy rows differ along the principal form, the action pattern, the resource
//! pattern, the negated elements and the effect.
//!
//! Driven over the wire (`POST /` with `X-Amz-Target: DynamoDB_20120810.<Operation>`)
//! through the same `MultiServiceRouter` the emulator's front doors use, reusing the
//! harness in [`super::table_items`].

use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::{
    NumberKey, MAX_PARTITION_KEY_BYTES, MAX_RESOURCE_POLICY_BYTES, MAX_SORT_KEY_BYTES,
    STATUS_TRANSITION,
};

use super::table_items::{call, create_active_table, emulator, ok};
// The refusal-code reader is the harness's, so there is one implementation of it.
use crate::emulator::error_code;

/// The ARN prefix every table of this emulator carries, in the region the harness's
/// credential scope names.
const TABLE_ARN_PREFIX: &str = "arn:aws:dynamodb:us-east-1:000000000000:table/";

/// The account id every generated emulator presents.
const ACCOUNT_ID: &str = "000000000000";

/// The ARN of one of this emulator's tables.
fn table_arn(name: &str) -> String {
    format!("{TABLE_ARN_PREFIX}{name}")
}

/// The `message`/`Message` member of a refusal, whichever casing it carries.
fn message_of(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// Wait out the span `PutResourcePolicy` takes to apply its policy to the data plane:
/// "PutResourcePolicy is an asynchronous operation" and "the policy application is
/// eventually consistent", so the emulator makes the application due one
/// `STATUS_TRANSITION` after the call.
fn wait_for_policy_application() {
    std::thread::sleep(STATUS_TRANSITION + std::time::Duration::from_millis(200));
}

/// A base64 `B` value of exactly `bytes` bytes.
///
/// Built from whole 3-byte groups ("abc") plus a 1- or 2-byte tail, so a case can name
/// a SIZE instead of a literal and the row stays readable.
fn binary_of(bytes: usize) -> String {
    let mut encoded = "YWJj".repeat(bytes / 3);
    match bytes % 3 {
        1 => encoded.push_str("YQ=="),
        2 => encoded.push_str("YWI="),
        _ => {}
    }
    encoded
}

/// One key-size case: the table it is written to, the key it writes, and the refusal
/// it must earn (`None` when the write is accepted).
struct KeyCase {
    case: String,
    table: &'static str,
    partition: Value,
    sort: Value,
    refusal: Option<String>,
}

/// The message a key past its role's documented limit must carry. Both wordings are
/// DynamoDB's own, one per key role, and each names the documented limit for its role.
fn oversize(role: &str, limit: i64) -> String {
    match role {
        "hashkey" => format!("Size of hashkey has exceeded the maximum size limit of{limit} bytes"),
        _ => format!("Aggregated size of all range keys has exceeded the size limit of {limit} bytes"),
    }
}

#[tokio::test]
async fn a_key_value_past_its_documented_size_limit_is_refused_on_every_path() {
    let router = emulator();
    create_active_table(
        &router,
        "KeyLimitsText",
        &[("pk", "HASH", "S"), ("sk", "RANGE", "S")],
    )
    .await;
    create_active_table(
        &router,
        "KeyLimitsBinary",
        &[("pk", "HASH", "B"), ("sk", "RANGE", "B")],
    )
    .await;

    let partition_limit = usize::try_from(MAX_PARTITION_KEY_BYTES).expect("a positive limit");
    let sort_limit = usize::try_from(MAX_SORT_KEY_BYTES).expect("a positive limit");
    let hashkey = oversize("hashkey", MAX_PARTITION_KEY_BYTES);
    let rangekey = oversize("rangekey", MAX_SORT_KEY_BYTES);
    // One row per way a key value can meet or miss its documented limit: at the limit,
    // one byte past it, in each key role, as text and as binary, with characters that
    // are one byte and characters that are two, and empty.
    let cases = vec![
        KeyCase {
            case: format!("a {partition_limit}-byte partition key is at the limit"),
            table: "KeyLimitsText",
            partition: json!({"S": "a".repeat(partition_limit)}),
            sort: json!({"S": "a".repeat(sort_limit)}),
            refusal: None,
        },
        KeyCase {
            case: format!("a {}-byte partition key is one past it", partition_limit + 1),
            table: "KeyLimitsText",
            partition: json!({"S": "a".repeat(partition_limit + 1)}),
            sort: json!({"S": "sort"}),
            refusal: Some(hashkey.clone()),
        },
        KeyCase {
            case: format!("a {}-byte sort key is one past its own limit", sort_limit + 1),
            table: "KeyLimitsText",
            partition: json!({"S": "partition"}),
            sort: json!({"S": "a".repeat(sort_limit + 1)}),
            refusal: Some(rangekey.clone()),
        },
        KeyCase {
            // "é" is two UTF-8 bytes: the limit is on BYTES, not characters.
            case: format!("{} two-byte characters are at the limit", partition_limit / 2),
            table: "KeyLimitsText",
            partition: json!({"S": "é".repeat(partition_limit / 2)}),
            sort: json!({"S": "sort"}),
            refusal: None,
        },
        KeyCase {
            case: format!(
                "{} two-byte characters are past it",
                partition_limit / 2 + 1
            ),
            table: "KeyLimitsText",
            partition: json!({"S": "é".repeat(partition_limit / 2 + 1)}),
            sort: json!({"S": "sort"}),
            refusal: Some(hashkey.clone()),
        },
        KeyCase {
            case: "an empty partition key string".to_string(),
            table: "KeyLimitsText",
            partition: json!({"S": ""}),
            sort: json!({"S": "sort"}),
            refusal: Some("cannot contain an empty string value. Key: pk".to_string()),
        },
        KeyCase {
            case: "an empty sort key string".to_string(),
            table: "KeyLimitsText",
            partition: json!({"S": "partition"}),
            sort: json!({"S": ""}),
            refusal: Some("cannot contain an empty string value. Key: sk".to_string()),
        },
        KeyCase {
            case: format!("a {partition_limit}-byte binary partition key"),
            table: "KeyLimitsBinary",
            partition: json!({"B": binary_of(partition_limit)}),
            sort: json!({"B": binary_of(sort_limit)}),
            refusal: None,
        },
        KeyCase {
            case: format!("a {}-byte binary partition key", partition_limit + 1),
            table: "KeyLimitsBinary",
            partition: json!({"B": binary_of(partition_limit + 1)}),
            sort: json!({"B": binary_of(8)}),
            refusal: Some(hashkey.clone()),
        },
        KeyCase {
            case: format!("a {}-byte binary sort key", sort_limit + 1),
            table: "KeyLimitsBinary",
            partition: json!({"B": binary_of(8)}),
            sort: json!({"B": binary_of(sort_limit + 1)}),
            refusal: Some(rangekey.clone()),
        },
        KeyCase {
            case: "an empty binary partition key".to_string(),
            table: "KeyLimitsBinary",
            partition: json!({"B": ""}),
            sort: json!({"B": binary_of(8)}),
            refusal: Some("cannot contain an empty binary value. Key: pk".to_string()),
        },
    ];

    for KeyCase {
        case,
        table,
        partition,
        sort,
        refusal,
    } in cases
    {
        let key = json!({"pk": partition, "sk": sort});
        let item = json!({"pk": key["pk"], "sk": key["sk"], "v": {"S": "written"}});
        let (status, body) =
            call(&router, "PutItem", &json!({"TableName": table, "Item": item})).await;
        let read = json!({"TableName": table, "Key": key});
        match refusal {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case}: a key inside the documented limit is stored, got {status} {body}"
                );
                // The write is proved by reading it back through the same key.
                let fetched = ok(&router, "GetItem", read).await;
                assert_eq!(
                    fetched["Item"]["v"],
                    json!({"S": "written"}),
                    "case {case}: GetItem reports the item that was written, got {fetched}"
                );
            }
            Some(expected) => {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "ValidationException"),
                    "case {case}: expected 400 ValidationException, got {status} {body}"
                );
                let message = message_of(&body);
                assert!(
                    message.contains(&expected),
                    "case {case}: expected a message carrying {expected:?}, got {message:?}"
                );
                // The READ path applies the same rule: a key that can never be stored
                // is refused rather than looked up.
                let (get_status, get_body) = call(&router, "GetItem", &read).await;
                assert_eq!(
                    (get_status, error_code(&get_body).as_str()),
                    (400, "ValidationException"),
                    "case {case} on GetItem: expected 400 ValidationException, got {get_status} {get_body}"
                );
                assert!(
                    message_of(&get_body).contains(&expected),
                    "case {case} on GetItem: expected a message carrying {expected:?}, got {}",
                    message_of(&get_body)
                );
                let (delete_status, delete_body) = call(&router, "DeleteItem", &read).await;
                assert_eq!(
                    (delete_status, error_code(&delete_body).as_str()),
                    (400, "ValidationException"),
                    "case {case} on DeleteItem: expected 400 ValidationException, got {delete_status} {delete_body}"
                );
            }
        }
    }

    // Nothing the oversize writes carried reached the table: the partition the
    // oversize-sort-key rows named holds no item at all.
    let queried = ok(
        &router,
        "Query",
        json!({
            "TableName": "KeyLimitsText",
            "KeyConditionExpression": "pk = :p",
            "ExpressionAttributeValues": {":p": {"S": "partition"}},
        }),
    )
    .await;
    assert_eq!(
        queried["Count"],
        json!(0),
        "case refused writes: the partition the refused rows named holds nothing, got {queried}"
    );
}

/// One value-rule case: the attribute value written, and the refusal it must earn.
struct ValueCase {
    case: &'static str,
    value: Value,
    refusal: Option<&'static str>,
}

#[tokio::test]
async fn an_attribute_value_outside_the_documented_rules_is_refused() {
    let router = emulator();
    create_active_table(&router, "ValueRules", &[("pk", "HASH", "S")]).await;

    // 38 significant digits is the documented precision ("Number: ... can have up to
    // 38 digits of precision. Exponent range is 10^-130 to 10^125" — "Service,
    // Account, and Table Quotas"), so 38 is accepted and 39 is not; the exponent is
    // checked at each published bound and one step past it.
    let thirty_eight = "1".repeat(38);
    let thirty_nine = "1".repeat(39);
    let cases = vec![
        ValueCase {
            case: "an empty string set",
            value: json!({"SS": []}),
            refusal: Some("An string set  may not be empty"),
        },
        ValueCase {
            case: "an empty number set",
            value: json!({"NS": []}),
            refusal: Some("An number set  may not be empty"),
        },
        ValueCase {
            case: "an empty binary set",
            value: json!({"BS": []}),
            refusal: Some("Binary sets should not be empty"),
        },
        ValueCase {
            case: "a string set repeating a member",
            value: json!({"SS": ["a", "b", "a"]}),
            refusal: Some("Input collection [\"a\", \"b\", \"a\"] contains duplicates."),
        },
        ValueCase {
            case: "a number set repeating a member spelled differently",
            value: json!({"NS": ["1", "1.0"]}),
            refusal: Some("Input collection contains duplicates."),
        },
        ValueCase {
            case: "a binary set repeating a member",
            value: json!({"BS": ["YWJj", "YWJj"]}),
            refusal: Some("Input collection [\"YWJj\",\"YWJj\"]of type BS contains duplicates."),
        },
        ValueCase {
            case: "an empty set nested in a list",
            value: json!({"L": [{"S": "ok"}, {"SS": []}]}),
            refusal: Some("may not be empty"),
        },
        ValueCase {
            case: "an empty set nested in a map",
            value: json!({"M": {"inner": {"NS": []}}}),
            refusal: Some("may not be empty"),
        },
        ValueCase {
            case: "a number carrying 39 significant digits",
            value: json!({"N": thirty_nine.clone()}),
            refusal: Some("significant digits"),
        },
        ValueCase {
            case: "39 significant digits inside a number set",
            value: json!({"NS": ["1", thirty_nine.clone()]}),
            refusal: Some("significant digits"),
        },
        ValueCase {
            case: "39 significant digits nested in a list",
            value: json!({"L": [{"N": thirty_nine.clone()}]}),
            refusal: Some("significant digits"),
        },
        ValueCase {
            case: "a magnitude above the documented exponent range",
            value: json!({"N": "1e126"}),
            refusal: Some("Number overflow"),
        },
        ValueCase {
            case: "a magnitude below the documented exponent range",
            value: json!({"N": "1e-131"}),
            refusal: Some("Number underflow"),
        },
        ValueCase {
            case: "a number that is not a number",
            value: json!({"N": "twelve"}),
            refusal: Some("cannot be converted to a numeric value"),
        },
        ValueCase {
            case: "a number carrying 38 significant digits",
            value: json!({"N": thirty_eight.clone()}),
            refusal: None,
        },
        ValueCase {
            case: "a magnitude at the top of the documented exponent range",
            value: json!({"N": "1e125"}),
            refusal: None,
        },
        ValueCase {
            case: "a magnitude at the bottom of the documented exponent range",
            value: json!({"N": "1e-130"}),
            refusal: None,
        },
        ValueCase {
            case: "zero spelled with trailing zeros",
            value: json!({"N": "0.000"}),
            refusal: None,
        },
        ValueCase {
            case: "sets whose members are distinct",
            value: json!({"SS": ["a", "b"]}),
            refusal: None,
        },
        ValueCase {
            // "Empty String and Binary attribute values are allowed" for an attribute
            // that is not a key (the `PutItem` doc).
            case: "an empty string on a non-key attribute",
            value: json!({"S": ""}),
            refusal: None,
        },
    ];

    for ValueCase {
        case,
        value,
        refusal,
    } in cases
    {
        let body = json!({
            "TableName": "ValueRules",
            "Item": {"pk": {"S": case}, "v": value},
        });
        let (status, answered) = call(&router, "PutItem", &body).await;
        match refusal {
            Some(expected) => {
                assert_eq!(
                    (status, error_code(&answered).as_str()),
                    (400, "ValidationException"),
                    "case {case}: expected 400 ValidationException, got {status} {answered}"
                );
                let message = message_of(&answered);
                assert!(
                    message.contains(expected),
                    "case {case}: expected a message carrying {expected:?}, got {message:?}"
                );
                // Refused before the write: the key holds nothing afterwards.
                let fetched = ok(
                    &router,
                    "GetItem",
                    json!({"TableName": "ValueRules", "Key": {"pk": {"S": case}}}),
                )
                .await;
                assert!(
                    fetched.get("Item").is_none(),
                    "case {case}: a refused write stores nothing, got {fetched}"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case}: a value inside the documented rules is stored, got {status} {answered}"
                );
                let fetched = ok(
                    &router,
                    "GetItem",
                    json!({"TableName": "ValueRules", "Key": {"pk": {"S": case}}}),
                )
                .await;
                // A write stores the NUMBER, not the lexeme that spelled it, so a
                // numeric row round-trips as the same value rather than the same text.
                let sent = &body["Item"]["v"];
                let stored = &fetched["Item"]["v"];
                match (
                    sent.get("N").and_then(Value::as_str),
                    stored.get("N").and_then(Value::as_str),
                ) {
                    (Some(written), Some(read_back)) => assert_eq!(
                        NumberKey::parse(written),
                        NumberKey::parse(read_back),
                        "case {case}: the stored number denotes the value that was \
                         written, got {read_back:?}"
                    ),
                    _ => assert_eq!(
                        stored, sent,
                        "case {case}: GetItem reports the value that was written, got {fetched}"
                    ),
                }
            }
        }
    }
}

/// One resource-policy case: the statement attached to the table, and whether a
/// `GetItem` by this emulator's caller is then denied.
struct PolicyCase {
    case: &'static str,
    statement: Value,
    denied: bool,
}

#[tokio::test]
async fn a_resource_policy_decides_whether_the_data_plane_serves_the_request() {
    let router = emulator();
    create_active_table(&router, "Secrets", &[("pk", "HASH", "S")]).await;
    create_active_table(&router, "Other", &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": "Secrets", "Item": {"pk": {"S": "secret"}, "v": {"S": "s1"}}}),
    )
    .await;
    let arn = table_arn("Secrets");
    let read = json!({"TableName": "Secrets", "Key": {"pk": {"S": "secret"}}});

    // One row per element a statement matches on, each differing from the denying
    // baseline in exactly one way.
    let cases = vec![
        PolicyCase {
            case: "a wildcard principal",
            statement: json!({
                "Effect": "Deny",
                "Principal": {"AWS": "*"},
                "Action": "dynamodb:GetItem",
                "Resource": arn,
            }),
            denied: true,
        },
        PolicyCase {
            case: "the account root as principal",
            statement: json!({
                "Effect": "Deny",
                "Principal": {"AWS": format!("arn:aws:iam::{ACCOUNT_ID}:root")},
                "Action": "dynamodb:GetItem",
                "Resource": arn,
            }),
            denied: true,
        },
        PolicyCase {
            case: "a bare account id as principal",
            statement: json!({
                "Effect": "Deny",
                "Principal": {"AWS": ACCOUNT_ID},
                "Action": "dynamodb:GetItem",
                "Resource": arn,
            }),
            denied: true,
        },
        PolicyCase {
            case: "an identity-specific principal",
            statement: json!({
                "Effect": "Deny",
                "Principal": {"AWS": format!("arn:aws:iam::{ACCOUNT_ID}:role/someone-else")},
                "Action": "dynamodb:GetItem",
                "Resource": arn,
            }),
            denied: false,
        },
        PolicyCase {
            case: "a service principal",
            statement: json!({
                "Effect": "Deny",
                "Principal": {"Service": "lambda.amazonaws.com"},
                "Action": "dynamodb:GetItem",
                "Resource": arn,
            }),
            denied: false,
        },
        PolicyCase {
            case: "a wildcard action",
            statement: json!({
                "Effect": "Deny",
                "Principal": "*",
                "Action": "dynamodb:*",
                "Resource": arn,
            }),
            denied: true,
        },
        PolicyCase {
            case: "an action list naming this one",
            statement: json!({
                "Effect": "Deny",
                // A principal element given as a LIST, which the grammar admits beside
                // the single string: it names this caller among others.
                "Principal": {"AWS": ["*", format!("arn:aws:iam::{ACCOUNT_ID}:role/other")]},
                "Action": ["dynamodb:Query", "dynamodb:GetItem"],
                "Resource": arn,
            }),
            denied: true,
        },
        PolicyCase {
            case: "another operation's action",
            statement: json!({
                "Effect": "Deny",
                "Principal": {"AWS": "*"},
                "Action": "dynamodb:UpdateItem",
                "Resource": arn,
            }),
            denied: false,
        },
        PolicyCase {
            case: "a wildcard resource",
            statement: json!({
                "Effect": "Deny",
                "Principal": {"AWS": "*"},
                "Action": "dynamodb:GetItem",
                "Resource": "*",
            }),
            denied: true,
        },
        PolicyCase {
            case: "another table as the resource",
            statement: json!({
                "Effect": "Deny",
                "Principal": {"AWS": "*"},
                "Action": "dynamodb:GetItem",
                "Resource": table_arn("Other"),
            }),
            denied: false,
        },
        PolicyCase {
            case: "only this table's indexes as the resource",
            statement: json!({
                "Effect": "Deny",
                "Principal": {"AWS": "*"},
                "Action": "dynamodb:GetItem",
                "Resource": format!("{arn}/index/*"),
            }),
            denied: false,
        },
        PolicyCase {
            case: "a statement naming no resource at all",
            statement: json!({
                "Effect": "Deny",
                "Principal": {"AWS": "*"},
                "Action": "dynamodb:GetItem",
            }),
            denied: true,
        },
        PolicyCase {
            case: "NotAction naming another operation",
            statement: json!({
                "Effect": "Deny",
                "Principal": {"AWS": "*"},
                "NotAction": "dynamodb:UpdateItem",
                "Resource": arn,
            }),
            denied: true,
        },
        PolicyCase {
            case: "NotPrincipal naming another identity",
            statement: json!({
                "Effect": "Deny",
                "NotPrincipal": {"AWS": format!("arn:aws:iam::{ACCOUNT_ID}:role/someone-else")},
                "Action": "dynamodb:GetItem",
                "Resource": arn,
            }),
            denied: true,
        },
        PolicyCase {
            case: "an Allow of the same action",
            statement: json!({
                // An Allow may not name a PUBLIC principal, so this row grants to the
                // account that is going to make the read instead: what is under test is
                // that an Allow denies nothing.
                "Effect": "Allow",
                "Principal": {"AWS": format!("arn:aws:iam::{ACCOUNT_ID}:root")},
                "Action": "dynamodb:GetItem",
                "Resource": arn,
            }),
            denied: false,
        },
    ];

    for PolicyCase {
        case,
        statement,
        denied,
    } in cases
    {
        let document =
            json!({"Version": "2012-10-17", "Statement": [statement]}).to_string();
        let attached = ok(
            &router,
            "PutResourcePolicy",
            // Several of these documents deny the caller the policy-management actions
            // too (a `dynamodb:*` Deny does), which the operation refuses unless the
            // request confirms it; the rule under test here is what the data plane
            // does with the attached document, so every row confirms.
            json!({
                "ResourceArn": arn,
                "Policy": document,
                "ConfirmRemoveSelfResourceAccess": true,
            }),
        )
        .await;
        // The attach is asynchronous, so the data plane is asked only once its
        // application is due.
        wait_for_policy_application();
        // The document and its revision id round trip through the reader, which reports
        // the revision in force: the one this attach has now applied.
        let stored = ok(
            &router,
            "GetResourcePolicy",
            json!({"ResourceArn": arn}),
        )
        .await;
        assert_eq!(
            stored["Policy"],
            json!(document),
            "case {case}: GetResourcePolicy reports the document that was attached, got {stored}"
        );
        assert_eq!(
            stored["RevisionId"], attached["RevisionId"],
            "case {case}: the reader reports the revision id the writer returned, got {stored}"
        );

        let (status, body) = call(&router, "GetItem", &read).await;
        if denied {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "AccessDeniedException"),
                "case {case}: expected 400 AccessDeniedException, got {status} {body}"
            );
            assert!(
                message_of(&body).contains("not authorized to perform: dynamodb:GetItem"),
                "case {case}: the refusal names the action it denied, got {}",
                message_of(&body)
            );
            // The service framework behind DynamoDB renders an authorization refusal
            // with a capitalised `Message`, which is the member a client reads.
            assert!(
                body.get("Message").is_some(),
                "case {case}: the refusal carries a Message member, got {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case}: a policy that denies nothing leaves the read alone, got {status} {body}"
            );
            assert_eq!(
                body["Item"]["v"],
                json!({"S": "s1"}),
                "case {case}: the read reports the stored item, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_resource_policy_is_stored_read_back_revised_and_removed() {
    let router = emulator();
    create_active_table(&router, "Revisions", &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": "Revisions", "Item": {"pk": {"S": "k"}, "v": {"S": "v1"}}}),
    )
    .await;
    let arn = table_arn("Revisions");
    let read = json!({"TableName": "Revisions", "Key": {"pk": {"S": "k"}}});

    // Reading a resource that carries no policy is the modelled PolicyNotFound.
    let (status, body) = call(&router, "GetResourcePolicy", &json!({"ResourceArn": arn})).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "PolicyNotFoundException"),
        "case no policy attached: expected 400 PolicyNotFoundException, got {status} {body}"
    );

    let deny_reads = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "DenyReads",
            "Effect": "Deny",
            "Principal": {"AWS": "*"},
            "Action": "dynamodb:GetItem",
            "Resource": arn,
        }],
    })
    .to_string();
    let attached = ok(
        &router,
        "PutResourcePolicy",
        json!({"ResourceArn": arn, "Policy": deny_reads}),
    )
    .await;
    let revision = attached["RevisionId"]
        .as_str()
        .expect("case attach: PutResourcePolicy reports a revision id")
        .to_string();

    // The attach is asynchronous: a read made straight after it is still authorized
    // the way it was, and the deny lands once the application is due.
    let served = ok(&router, "GetItem", read.clone()).await;
    assert_eq!(
        served["Item"]["v"],
        json!({"S": "v1"}),
        "case straight after the attach: the policy is not applied yet, got {served}"
    );
    wait_for_policy_application();
    let (status, body) = call(&router, "GetItem", &read).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "AccessDeniedException"),
        "case once the application is due: the read is denied, got {status} {body}"
    );

    // "Running it multiple times on the same resource using the same policy document
    // will return the same revision id" (the `PutResourcePolicy` doc).
    let again = ok(
        &router,
        "PutResourcePolicy",
        json!({"ResourceArn": arn, "Policy": deny_reads}),
    )
    .await;
    assert_eq!(
        again["RevisionId"],
        json!(revision),
        "case re-attach the same document: expected the same revision id, got {again}"
    );

    // A different document gets a different revision id, and the reader follows it.
    let deny_writes = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Deny",
            "Principal": {"AWS": "*"},
            "Action": "dynamodb:PutItem",
            "Resource": arn,
        }],
    })
    .to_string();
    let revised = ok(
        &router,
        "PutResourcePolicy",
        json!({"ResourceArn": arn, "Policy": deny_writes, "ExpectedRevisionId": revision}),
    )
    .await;
    assert_ne!(
        revised["RevisionId"],
        json!(revision),
        "case revise: a different document gets a different revision id, got {revised}"
    );
    // The revise is asynchronous like the attach: until its application is due the
    // revision the resource carried BEFORE it is the one in force, so the read it denied
    // is still denied.
    let (status, body) = call(&router, "GetItem", &read).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "AccessDeniedException"),
        "case straight after the revise: the superseded policy is still in force, got {status} \
         {body}"
    );
    wait_for_policy_application();
    // The new policy denies writes, so the read works again and the write does not.
    let fetched = ok(&router, "GetItem", read.clone()).await;
    assert_eq!(
        fetched["Item"]["v"],
        json!({"S": "v1"}),
        "case revise: the read the previous policy denied is served again, got {fetched}"
    );
    let (status, body) = call(
        &router,
        "PutItem",
        &json!({"TableName": "Revisions", "Item": {"pk": {"S": "k"}, "v": {"S": "v2"}}}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "AccessDeniedException"),
        "case revise: the newly denied write is refused, got {status} {body}"
    );

    // A stale `ExpectedRevisionId` is the modelled PolicyNotFound, and changes nothing.
    let (status, body) = call(
        &router,
        "PutResourcePolicy",
        &json!({"ResourceArn": arn, "Policy": deny_reads, "ExpectedRevisionId": revision}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "PolicyNotFoundException"),
        "case a stale ExpectedRevisionId: expected 400 PolicyNotFoundException, got {status} {body}"
    );
    let stored = ok(&router, "GetResourcePolicy", json!({"ResourceArn": arn})).await;
    assert_eq!(
        stored["RevisionId"], revised["RevisionId"],
        "case a stale ExpectedRevisionId: the attached policy is unchanged, got {stored}"
    );

    // Deleting reports what was removed and releases the denial once the removal has
    // been applied, which takes the same span an attach takes.
    let removed = ok(
        &router,
        "DeleteResourcePolicy",
        json!({"ResourceArn": arn, "ExpectedRevisionId": revised["RevisionId"]}),
    )
    .await;
    assert_eq!(
        removed["RevisionId"], revised["RevisionId"],
        "case delete: the revision id of what was removed is reported, got {removed}"
    );
    // "If you issue a GetResourcePolicy request immediately after a DeleteResourcePolicy
    // request, DynamoDB might return the policy that was present before the deletion
    // request" (the `GetResourcePolicy` doc), and that revision is the one still in force.
    let lagging = ok(&router, "GetResourcePolicy", json!({"ResourceArn": arn})).await;
    assert_eq!(
        lagging["RevisionId"], revised["RevisionId"],
        "case straight after the delete: the reader reports the revision that was present \
         before it, got {lagging}"
    );
    wait_for_policy_application();
    let (status, body) = call(&router, "GetResourcePolicy", &json!({"ResourceArn": arn})).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "PolicyNotFoundException"),
        "case once the delete is applied: no policy is attached, got {status} {body}"
    );
    ok(
        &router,
        "PutItem",
        json!({"TableName": "Revisions", "Item": {"pk": {"S": "k"}, "v": {"S": "v2"}}}),
    )
    .await;

    // "Running it multiple times on the same resource doesn't result in an error
    // response", and the `RevisionId` "will be empty if you make a request against a
    // resource without a policy" (the `DeleteResourcePolicy` doc).
    let twice = ok(
        &router,
        "DeleteResourcePolicy",
        json!({"ResourceArn": arn}),
    )
    .await;
    assert!(
        twice.get("RevisionId").is_none_or(Value::is_null),
        "case delete twice: no revision id is reported for a resource without a policy, got {twice}"
    );
}

#[tokio::test]
async fn a_policy_document_this_emulator_cannot_honour_is_refused_not_stored() {
    let router = emulator();
    create_active_table(&router, "Documents", &[("pk", "HASH", "S")]).await;
    let arn = table_arn("Documents");
    let statement = |extra: Value| {
        let mut statement = json!({
            "Effect": "Deny",
            "Principal": {"AWS": "*"},
            "Action": "dynamodb:GetItem",
            "Resource": arn,
        });
        for (key, value) in extra.as_object().expect("an object of extra members") {
            statement[key] = value.clone();
        }
        json!({"Version": "2012-10-17", "Statement": [statement]}).to_string()
    };

    // A condition over a key this request carries no value for cannot be decided, and a
    // Deny that rests on it is applied anyway: the emulator never serves a request the
    // attached policy may deny. The document itself is stored verbatim and reported back
    // once its application is due.
    let conditioned = statement(json!({
        "Condition": {"StringEquals": {"aws:SourceVpc": "vpc-1"}},
    }));
    ok(
        &router,
        "PutResourcePolicy",
        json!({"ResourceArn": arn, "Policy": conditioned.clone()}),
    )
    .await;
    wait_for_policy_application();
    let stored = ok(&router, "GetResourcePolicy", json!({"ResourceArn": arn})).await;
    assert_eq!(
        stored["Policy"],
        json!(conditioned),
        "case a statement Condition: the document is stored verbatim, got {stored}"
    );
    let (status, body) = call(
        &router,
        "GetItem",
        &json!({"TableName": "Documents", "Key": {"pk": {"S": "k"}}}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "AccessDeniedException"),
        "case a statement Condition: the deny is applied anyway, got {status} {body}"
    );
    assert!(
        message_of(&body).contains("with an explicit deny in a resource-based policy"),
        "case a statement Condition: the refusal names the document that denied it, got {}",
        message_of(&body)
    );
    ok(
        &router,
        "DeleteResourcePolicy",
        json!({"ResourceArn": arn}),
    )
    .await;
    // The removal is asynchronous too, so the reader below is asked once it is applied.
    wait_for_policy_application();

    // Rows that are not policy documents at all.
    let invalid = [
        ("not JSON", "this is not a policy".to_string()),
        ("a JSON array", json!([]).to_string()),
        (
            "no Statement",
            json!({"Version": "2012-10-17"}).to_string(),
        ),
        (
            "an Effect that is neither Allow nor Deny",
            statement(json!({"Effect": "Maybe"})),
        ),
        (
            "no Principal",
            json!({"Version": "2012-10-17", "Statement": [{
                "Effect": "Deny",
                "Action": "dynamodb:GetItem",
                "Resource": arn,
            }]})
            .to_string(),
        ),
        (
            "both Action and NotAction",
            statement(json!({"NotAction": "dynamodb:PutItem"})),
        ),
    ];
    for (case, document) in invalid {
        let (status, body) = call(
            &router,
            "PutResourcePolicy",
            &json!({"ResourceArn": arn, "Policy": document}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case}: expected 400 ValidationException, got {status} {body}"
        );
    }

    // Past the documented 20 KB the document breaks a bound on a member's VALUE, which
    // is the service-common validation refusal naming that bound.
    let padded = json!({
        "Version": "2012-10-17",
        "Sid": "x".repeat(MAX_RESOURCE_POLICY_BYTES),
        "Statement": [{
            "Effect": "Deny",
            "Principal": {"AWS": "*"},
            "Action": "dynamodb:GetItem",
            "Resource": arn,
        }],
    })
    .to_string();
    let (status, body) = call(
        &router,
        "PutResourcePolicy",
        &json!({"ResourceArn": arn, "Policy": padded}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case a document past {MAX_RESOURCE_POLICY_BYTES} bytes: expected 400 \
         ValidationException, got {status} {body}"
    );
    assert_eq!(
        message_of(&body),
        format!(
            "One or more parameter values were invalid: Policy size exceeds the maximum \
             allowed size of {MAX_RESOURCE_POLICY_BYTES} bytes"
        ),
        "case a document past {MAX_RESOURCE_POLICY_BYTES} bytes: the refusal names the limit, \
         got {body}"
    );

    // None of the refusals attached anything.
    let (status, body) = call(&router, "GetResourcePolicy", &json!({"ResourceArn": arn})).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "PolicyNotFoundException"),
        "case after every refusal: no policy is attached, got {status} {body}"
    );

    // An ARN that names no table of this service, and a table that does not exist.
    let (status, body) = call(
        &router,
        "PutResourcePolicy",
        &json!({"ResourceArn": "arn:aws:sqs:us-east-1:000000000000:queue/q", "Policy": statement(json!({}))}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case an ARN of another service: expected 400 ValidationException, got {status} {body}"
    );
    let (status, body) = call(
        &router,
        "GetResourcePolicy",
        &json!({"ResourceArn": table_arn("Missing")}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "case an unknown table: expected 400 ResourceNotFoundException, got {status} {body}"
    );
}

#[tokio::test]
async fn a_policy_attached_at_create_time_is_enforced_from_the_first_request() {
    let router = emulator();
    let arn = table_arn("Sealed");
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Deny",
            "Principal": {"AWS": "*"},
            "Action": "dynamodb:GetItem",
            "Resource": arn,
        }],
    })
    .to_string();
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "Sealed",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
            "ResourcePolicy": document,
        }),
    )
    .await;
    // The table has to be `ACTIVE` before the data plane serves it ("You can perform
    // read and write operations only on an ACTIVE table"), so the wait is the SPAN the
    // creation takes rather than a literal that a change to that span invalidates.
    std::thread::sleep(STATUS_TRANSITION + std::time::Duration::from_millis(400));
    ok(
        &router,
        "PutItem",
        json!({"TableName": "Sealed", "Item": {"pk": {"S": "k"}}}),
    )
    .await;

    // The policy came with the create: "the policy application is strongly consistent".
    let stored = ok(&router, "GetResourcePolicy", json!({"ResourceArn": arn})).await;
    assert_eq!(
        stored["Policy"],
        json!(document),
        "case create with a policy: the attached document is reported back, got {stored}"
    );
    let (status, body) = call(
        &router,
        "GetItem",
        &json!({"TableName": "Sealed", "Key": {"pk": {"S": "k"}}}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "AccessDeniedException"),
        "case create with a policy: the denied read is refused, got {status} {body}"
    );
}
