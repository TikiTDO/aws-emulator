// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateDeliveryConfiguration`: changing the configuration of a
//! delivery the account already holds.
//!
//! The rule under test: `UpdateDeliveryConfiguration` changes the configuration members of
//! the one delivery its `id` names — each of `recordFields`, `fieldDelimiter` and
//! `s3DeliveryConfiguration` stored as the request gives it and left as the delivery already
//! had it when the request names none — while everything that is not configuration (the
//! delivery's source, its destination, the destination type read off that destination, its
//! tags, and the id and ARN that identify it) stays as it was, because "you can't use this
//! operation to change the source or destination of the delivery". The delivery still delivers
//! under a WHOLE configuration afterwards, so a member of the S3 object layout the request
//! restates in part keeps the value the governing configuration template defaults it to. An
//! `id` this account holds no delivery under is refused `ResourceNotFoundException` and changes
//! nothing, and the modelled response carries no member, so what changed is read back through
//! `GetDelivery` and `DescribeDeliveries`.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response a handler does not actually compute cannot pass.
//!
//! The table spans what the rule generalises over rather than one request: which subset of
//! the configuration members a request names (each one alone, all three together), record
//! fields empty / several / as many as the member admits, a delimiter at both published
//! length bounds, an S3 configuration with both members and with one, and a repeat of the
//! same update. The recorded request an issue happens to mention is one row.

use serde_json::{json, Value};

use crate::harness::Logs;

/// The operations under test.
const UPDATE: &str = "UpdateDeliveryConfiguration";
const CREATE: &str = "CreateDelivery";
const GET: &str = "GetDelivery";
const DESCRIBE: &str = "DescribeDeliveries";

/// The modelled error codes the rule names.
const NOT_FOUND: &str = "ResourceNotFoundException";
const INVALID: &str = "ValidationException";

/// The whole message a request naming a delivery this account does not hold is refused with:
/// the sentence states that the requested delivery is not in the account and interpolates no
/// id.
const DELIVERY_NOT_FOUND: &str = "Requested Delivery does not exist in this account.";

/// The configuration members an update may name — the three of
/// `UpdateDeliveryConfigurationRequest` that are not the `id`
/// (`smithy_query(query="operation",operation="UpdateDeliveryConfiguration")`).
const CONFIGURATION_MEMBERS: [&str; 3] = ["recordFields", "fieldDelimiter", "s3DeliveryConfiguration"];

/// The members of the S3 object layout a delivery delivers under — the two of
/// `S3DeliveryConfiguration` (`smithy_query(query="shape",shape="S3DeliveryConfiguration")`).
/// Each stands on its own, and the delivery carries BOTH however few of them a request names.
const S3_CONFIGURATION_MEMBERS: [&str; 2] = ["suffixPath", "enableHiveCompatiblePath"];

/// The members a delivery carries that an update may NOT change: "you can't use this
/// operation to change the source or destination of the delivery", and the id, ARN,
/// destination type and tags are the service's or the create's rather than this call's.
const UNCHANGEABLE_MEMBERS: [&str; 6] = [
    "id",
    "arn",
    "deliverySourceName",
    "deliveryDestinationArn",
    "deliveryDestinationType",
    "tags",
];

/// The longest `fieldDelimiter` the model admits: `FieldDelimiter` is `@length(min: 0, max:
/// 5)`.
const LONGEST_DELIMITER: &str = "|||||";

/// How many record fields one delivery may carry: `RecordFields` is `@length(min: 0, max:
/// 128)`.
const MAX_RECORD_FIELDS: usize = 128;

/// A delivery source over one log group of this account, as `PutDeliverySource` creates one.
async fn delivery_source(logs: &Logs, name: &str, group: &str) {
    logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
        .await;
    let (_, group_arn) = logs.group_arns(group).await;
    logs.ok(
        "PutDeliverySource",
        &json!({"name": name, "resourceArn": group_arn, "logType": "INSIGHTS_QUERY_LOGS"}),
    )
    .await;
}

/// A delivery destination over `resource_arn`, answering the destination's own ARN.
async fn delivery_destination(logs: &Logs, name: &str, resource_arn: &str) -> String {
    let answer = logs
        .ok(
            "PutDeliveryDestination",
            &json!({
                "name": name,
                "deliveryDestinationConfiguration": {"destinationResourceArn": resource_arn},
            }),
        )
        .await;
    answer["deliveryDestination"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("a put answers the destination's ARN, got {answer}"))
        .to_string()
}

/// The delivery the account holds under `id`, as `GetDelivery` reports it.
async fn fetched(logs: &Logs, id: &str, case: &str) -> Value {
    let answer = logs.ok(GET, &json!({"id": id})).await;
    answer
        .get("delivery")
        .filter(|delivery| !delivery.is_null())
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: expected a delivery, got {answer}"))
}

/// Every delivery the account holds, as `DescribeDeliveries` reports them.
async fn described(logs: &Logs) -> Vec<Value> {
    let page = logs.ok(DESCRIBE, &json!({})).await;
    page["deliveries"]
        .as_array()
        .unwrap_or_else(|| panic!("DescribeDeliveries reports a deliveries list, got {page}"))
        .clone()
}

/// A delivery of this account, created with the configuration `created` carries, answering its
/// id.
async fn delivery_with(logs: &Logs, source: &str, destination_arn: &str, created: &Value) -> String {
    let mut body = json!({
        "deliverySourceName": source,
        "deliveryDestinationArn": destination_arn,
    });
    if let Some(members) = created.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    let answer = logs.ok(CREATE, &body).await;
    answer["delivery"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("a create answers the delivery's id, got {answer}"))
        .to_string()
}

/// Each configuration member the request names is stored as given, each one it does not name
/// is left as the delivery already had it, and nothing that is not configuration moves.
#[tokio::test]
async fn an_update_changes_the_members_it_names_and_only_those() {
    let logs = Logs::new();
    delivery_source(&logs, "vpc-flow-logs", "/vended/update-source").await;
    let destination =
        delivery_destination(&logs, "to-s3", "arn:aws:s3:::vended-logs-updates").await;

    // The delivery every case starts from: created with all three configuration members and a
    // tag set, so a case that names one member can show the other two — and the tags — are
    // still what the create left.
    let created_configuration = json!({
        "recordFields": ["@timestamp", "@message"],
        "fieldDelimiter": ",",
        "s3DeliveryConfiguration": {
            "suffixPath": "AWSLogs/{account-id}/{service}/{region}",
            "enableHiveCompatiblePath": false,
        },
        "tags": {"env": "prod", "team": "logs"},
    });

    // The longest record-field list the member admits, each entry a `FieldHeader` (1..64).
    let longest_record_fields: Vec<String> = (0..MAX_RECORD_FIELDS)
        .map(|index| format!("field_{index}"))
        .collect();

    // (case, the members this update names)
    let cases: Vec<(&str, Value)> = vec![
        (
            "no configuration member at all",
            json!({}),
        ),
        (
            "record fields only",
            json!({"recordFields": ["@timestamp", "@message", "@logStream"]}),
        ),
        (
            // `RecordFields` is `@length(min: 0)`: the empty list is a list the caller named,
            // which is not the same as naming none.
            "the empty record-field list only",
            json!({"recordFields": []}),
        ),
        (
            "as many record fields as the member admits",
            json!({"recordFields": longest_record_fields}),
        ),
        (
            // `FieldDelimiter` is `@length(min: 0, max: 5)`: both published bounds.
            "the longest delimiter only",
            json!({"fieldDelimiter": LONGEST_DELIMITER}),
        ),
        (
            "the empty delimiter only",
            json!({"fieldDelimiter": ""}),
        ),
        (
            "an S3 configuration with both of its members only",
            json!({"s3DeliveryConfiguration": {
                "suffixPath": "AWSLogs/{account-id}/{source-type}/{yyyy}/{MM}/{dd}",
                "enableHiveCompatiblePath": true,
            }}),
        ),
        (
            "an S3 configuration with one of its members only",
            json!({"s3DeliveryConfiguration": {"suffixPath": "raw/{yyyy}-{MM}-{dd}"}}),
        ),
        (
            "all three configuration members at once",
            json!({
                "recordFields": ["@message"],
                "fieldDelimiter": "\t",
                "s3DeliveryConfiguration": {
                    "suffixPath": "AWSLogs/{account-id}",
                    "enableHiveCompatiblePath": true,
                },
            }),
        ),
    ];

    for (case, named) in cases {
        // One delivery per case, so each case's "left as it was" is the configuration the
        // create named rather than whatever an earlier case changed.
        let id = delivery_with(
            &logs,
            "vpc-flow-logs",
            &destination,
            &created_configuration,
        )
        .await;
        let before = fetched(&logs, &id, case).await;

        let mut body = json!({"id": id});
        if let Some(members) = named.as_object() {
            for (member, value) in members {
                body[member] = value.clone();
            }
        }
        let answer = logs.ok(UPDATE, &body).await;
        assert!(
            answer.as_object().is_none_or(|members| members.is_empty()),
            "case {case}: the modelled response carries no member, got {answer}"
        );

        let after = fetched(&logs, &id, case).await;
        for member in CONFIGURATION_MEMBERS {
            match named.get(member) {
                // A member the request named is stored as given. `s3DeliveryConfiguration` is
                // a STRUCTURE whose members each stand on their own, so a request restating it
                // in part names those members alone and the rest of the object layout stays at
                // the value the governing template defaults it to.
                Some(Value::Object(named_members)) => {
                    let stored = after
                        .get(member)
                        .and_then(Value::as_object)
                        .unwrap_or_else(|| {
                            panic!(
                                "case {case}: the delivery carries the {member} structure, got \
                                 {after}"
                            )
                        });
                    for (inner, value) in named_members {
                        assert_eq!(
                            stored.get(inner),
                            Some(value),
                            "case {case}: the delivery carries the {member}.{inner} this update \
                             named, got {after}"
                        );
                    }
                    for inner in S3_CONFIGURATION_MEMBERS {
                        assert!(
                            stored.contains_key(inner),
                            "case {case}: the delivery's {member} carries {inner} whether the \
                             update named it or not, got {after}"
                        );
                    }
                }
                Some(value) => assert_eq!(
                    after.get(member),
                    Some(value),
                    "case {case}: the delivery carries the {member} this update named, got \
                     {after}"
                ),
                // A member the request did not name is left as the delivery already had it.
                None => assert_eq!(
                    after.get(member),
                    before.get(member),
                    "case {case}: the {member} this update did not name is unchanged, got \
                     {after}"
                ),
            }
        }
        for member in UNCHANGEABLE_MEMBERS {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {case}: an update does not change {member}, got {after}"
            );
        }
        // The round trip: the delivery the reader reports is the one the enumeration reports.
        assert!(
            described(&logs).await.contains(&after),
            "case {case}: DescribeDeliveries reports the updated delivery"
        );

        // The same update again leaves the delivery exactly as the first one did, and reading
        // it twice reports the same thing.
        logs.ok(UPDATE, &body).await;
        assert_eq!(
            fetched(&logs, &id, case).await,
            after,
            "case {case}: the same update twice leaves the same delivery"
        );

        logs.ok("DeleteDelivery", &json!({"id": id})).await;
    }
}

/// An update is refused when its `id` names no delivery of this account, and changes nothing.
#[tokio::test]
async fn an_update_naming_a_delivery_this_account_does_not_hold_is_refused() {
    let logs = Logs::new();
    delivery_source(&logs, "waf_logs", "/vended/update-refusals").await;
    let destination =
        delivery_destination(&logs, "to-s3", "arn:aws:s3:::vended-logs-update-refusals").await;
    let id = delivery_with(
        &logs,
        "waf_logs",
        &destination,
        &json!({"recordFields": ["@timestamp", "@message"], "fieldDelimiter": ","}),
    )
    .await;
    let untouched = fetched(&logs, &id, "the delivery the refusals must not touch").await;

    // (case, the id this update names)
    let missing: Vec<(&str, String)> = vec![
        ("an id no create ever minted", "0123456789abcdef".to_string()),
        ("the longest id the member admits", "d".repeat(64)),
        ("the shortest id the member admits", "0".to_string()),
        ("a stored id with one more character", format!("{id}0")),
    ];
    for (case, named) in missing {
        let (code, message) = logs.refusal(UPDATE, &json!({"id": named})).await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case}: expected {NOT_FOUND}, got {code} with message {message:?}"
        );
        assert_eq!(
            message, DELIVERY_NOT_FOUND,
            "case {case}: expected {DELIVERY_NOT_FOUND:?}, got {message:?}"
        );
        assert_eq!(
            fetched(&logs, &id, case).await,
            untouched,
            "case {case}: a refused update changes nothing"
        );
        assert_eq!(
            described(&logs).await.len(),
            1,
            "case {case}: a refused update adds no delivery"
        );
    }

    // A delivery that is gone is a delivery this account no longer holds.
    logs.ok("DeleteDelivery", &json!({"id": &id})).await;
    let (code, message) = logs.refusal(UPDATE, &json!({"id": &id})).await;
    assert_eq!(
        code, NOT_FOUND,
        "case a deleted delivery: expected {NOT_FOUND}, got {code} with message {message:?}"
    );
    assert_eq!(
        message, DELIVERY_NOT_FOUND,
        "case a deleted delivery: expected {DELIVERY_NOT_FOUND:?}, got {message:?}"
    );
}

/// A request outside the published bounds of a configuration member is refused as a bad
/// request rather than stored.
#[tokio::test]
async fn an_update_outside_a_published_bound_is_refused() {
    let logs = Logs::new();
    delivery_source(&logs, "waf_logs", "/vended/update-bounds").await;
    let destination =
        delivery_destination(&logs, "to-s3", "arn:aws:s3:::vended-logs-update-bounds").await;
    let id = delivery_with(&logs, "waf_logs", &destination, &json!({})).await;
    let untouched = fetched(&logs, &id, "the delivery the refusals must not touch").await;

    // (case, the members this update names) — one past each published bound.
    let cases: Vec<(&str, Value)> = vec![
        (
            // `FieldDelimiter` is `@length(max: 5)`.
            "a delimiter one character past the longest",
            json!({"fieldDelimiter": format!("{LONGEST_DELIMITER}|")}),
        ),
        (
            // `RecordFields` is `@length(max: 128)`.
            "one record field more than the member admits",
            json!({"recordFields": (0..=MAX_RECORD_FIELDS)
                .map(|index| format!("field_{index}"))
                .collect::<Vec<String>>()}),
        ),
        (
            // `DeliveryId` is `@pattern("^[0-9A-Za-z]+$")`.
            "an id outside the member's pattern",
            json!({"id": "not an id"}),
        ),
    ];
    for (case, named) in cases {
        let mut body = json!({"id": &id});
        if let Some(members) = named.as_object() {
            for (member, value) in members {
                body[member] = value.clone();
            }
        }
        let code = logs.err(UPDATE, &body).await;
        assert_eq!(
            code, INVALID,
            "case {case}: expected {INVALID}, got {code}"
        );
        assert_eq!(
            fetched(&logs, &id, case).await,
            untouched,
            "case {case}: a refused update changes nothing"
        );
    }
}
