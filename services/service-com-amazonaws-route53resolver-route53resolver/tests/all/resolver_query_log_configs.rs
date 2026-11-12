// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Resolver query-logging slice:
//! `CreateResolverQueryLogConfig` and `GetResolverQueryLogConfig`.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::ops::types::{
    RESOLVER_QUERY_LOG_CONFIG_STATUS_VALUES, SHARE_STATUS_VALUES,
};
use service_com_amazonaws_route53resolver_route53resolver::state::PROVISIONING_DELAY;

use super::harness::{
    call, emulator, error_type, expect_error, ok, parse_time, string_member, without_status,
    ACCOUNT, REGION,
};

/// The `ResolverQueryLogConfig` of a create/get response.
fn config(body: &Value) -> &Value {
    body.get("ResolverQueryLogConfig")
        .unwrap_or_else(|| panic!("the response carries a ResolverQueryLogConfig, got {body}"))
}

/// Wait out the span the clock owes a `CREATING` configuration, so the next call
/// sees the settled `CREATED` status.
async fn settle() {
    tokio::time::sleep(PROVISIONING_DELAY + Duration::from_millis(250)).await;
}

/// One row of the create table: the request, and the `DestinationArn` the
/// stored configuration must then report.
struct CreateCase {
    case: &'static str,
    body: Value,
    stored_destination: &'static str,
}

/// A created query logging configuration is persisted: every member of the
/// response is computed from the request plus what the model documents, and
/// `GetResolverQueryLogConfig` answers the very same configuration.
///
/// The table spans what the rule generalises over: each destination kind
/// `CreateResolverQueryLogConfig$DestinationArn` documents (an S3 bucket, an S3
/// bucket with the file prefix the doc allows, a CloudWatch Logs log group with
/// and without the `:*` log-stream wildcard, a Kinesis Data Firehose delivery
/// stream named under both `kinesis` and `firehose`, and one in another
/// partition and region), every character class the `Name` pattern
/// `^(?!^[0-9]+$)([a-zA-Z0-9\-_' ']+)$` admits together with its published
/// length bounds of one and 64, and `Tags` absent, empty and carrying several
/// entries.
#[tokio::test]
async fn a_created_query_log_config_is_read_back_from_state() {
    // 64 characters, the maximum the model's `Name` (length 1..64) allows.
    let longest_name = "n".repeat(64);
    let cases = vec![
        CreateCase {
            case: "an S3 bucket, no Tags",
            body: json!({
                "CreatorRequestId": "req-s3",
                "Name": "queries-to-s3",
                "DestinationArn": "arn:aws:s3:::amzn-s3-demo-bucket",
            }),
            stored_destination: "arn:aws:s3:::amzn-s3-demo-bucket",
        },
        CreateCase {
            case: "an S3 bucket with the documented file prefix",
            body: json!({
                "CreatorRequestId": "req-s3-prefix",
                "Name": "queries_to-s3 2",
                "DestinationArn": "arn:aws:s3:::amzn-s3-demo-bucket/development/",
                "Tags": [{ "Key": "team", "Value": "net" }],
            }),
            stored_destination: "arn:aws:s3:::amzn-s3-demo-bucket/development/",
        },
        CreateCase {
            case: "a log group named with the stream wildcard, several tags",
            body: json!({
                "CreatorRequestId": "req-logs",
                "Name": "ops' queries",
                "DestinationArn":
                    "arn:aws:logs:us-west-1:123456789012:log-group:/mystack-testgroup-12ABC1AB12A1:*",
                "Tags": [
                    { "Key": "team", "Value": "net" },
                    { "Key": "env", "Value": "prod" },
                    // The shortest value the service's tag acceptance criteria
                    // admit: a tag it stores names a value.
                    { "Key": "note", "Value": "-" },
                ],
            }),
            // The wildcard names every stream in the group, not the group, so
            // the destination stored is the log group itself.
            stored_destination:
                "arn:aws:logs:us-west-1:123456789012:log-group:/mystack-testgroup-12ABC1AB12A1",
        },
        CreateCase {
            case: "a log group named without the stream wildcard",
            body: json!({
                "CreatorRequestId": "req-logs-bare",
                "Name": "queries-to-logs",
                "DestinationArn": "arn:aws:logs:us-west-2:123456789012:log-group:/queries",
            }),
            stored_destination: "arn:aws:logs:us-west-2:123456789012:log-group:/queries",
        },
        CreateCase {
            case: "a Kinesis stream, an empty tag list",
            body: json!({
                "CreatorRequestId": "req-kinesis",
                "Name": "q",
                "DestinationArn": "arn:aws:kinesis:us-east-2:012345678901:stream/my_stream_name",
                "Tags": [],
            }),
            stored_destination: "arn:aws:kinesis:us-east-2:012345678901:stream/my_stream_name",
        },
        CreateCase {
            case: "a Firehose delivery stream, the longest Name the model allows",
            body: json!({
                "CreatorRequestId": "req-firehose",
                "Name": longest_name,
                "DestinationArn":
                    "arn:aws:firehose:eu-west-1:012345678901:deliverystream/my-stream",
            }),
            stored_destination: "arn:aws:firehose:eu-west-1:012345678901:deliverystream/my-stream",
        },
        CreateCase {
            case: "a destination in another partition and region",
            body: json!({
                "CreatorRequestId": "2026-10-24T06:21:31Z",
                "Name": "stamped-request",
                "DestinationArn": "arn:aws-us-gov:s3:::gov-bucket",
            }),
            stored_destination: "arn:aws-us-gov:s3:::gov-bucket",
        },
    ];

    let router = emulator();
    // Every row's destination is one the account holds: a create resolves its
    // `DestinationArn` against the store of the service that owns it.
    router.hold_query_log_destinations(
        cases
            .iter()
            .filter_map(|case| case.body.get("DestinationArn").and_then(Value::as_str)),
    );
    let mut seen_ids: Vec<String> = Vec::new();

    for CreateCase {
        case,
        body,
        stored_destination,
    } in cases
    {
        let created = ok(&router, "CreateResolverQueryLogConfig", body.clone()).await;
        let made = config(&created).clone();
        let id = string_member(&made, "Id");

        assert!(
            id.starts_with("rqlc-"),
            "case {case:?}: a query logging configuration id is minted as rqlc-<hex>, got {id:?}"
        );
        assert!(
            !id.is_empty() && id.len() <= 64,
            "case {case:?}: Id must satisfy ResourceId's length 1..64, got {id:?}"
        );
        assert_eq!(
            string_member(&made, "Arn"),
            format!("arn:aws:route53resolver:{REGION}:{ACCOUNT}:resolver-query-log-config/{id}"),
            "case {case:?}: the Arn must be rooted in the signed region and this id, got {made}"
        );
        for member in ["Name", "CreatorRequestId"] {
            assert_eq!(
                made.get(member),
                body.get(member),
                "case {case:?}: {member} must come back exactly as requested, got {made}"
            );
        }
        assert_eq!(
            string_member(&made, "DestinationArn"),
            stored_destination,
            "case {case:?}: the destination must be stored in its canonical form, got {made}"
        );
        assert_eq!(
            string_member(&made, "OwnerId"),
            ACCOUNT,
            "case {case:?}: the configuration is owned by the calling account, got {made}"
        );
        assert_eq!(
            made.get("AssociationCount"),
            Some(&json!(0)),
            "case {case:?}: a configuration is created with no VPC associated, got {made}"
        );
        let share_status = string_member(&made, "ShareStatus");
        assert_eq!(
            share_status, "NOT_SHARED",
            "case {case:?}: a configuration this account just created is shared with nobody, got {made}"
        );
        assert!(
            SHARE_STATUS_VALUES.contains(&share_status.as_str()),
            "case {case:?}: ShareStatus must be a modelled value, got {share_status:?}"
        );
        let status = string_member(&made, "Status");
        assert_eq!(
            status, "CREATING",
            "case {case:?}: the create answers while Resolver is still creating it, got {made}"
        );
        assert!(
            RESOLVER_QUERY_LOG_CONFIG_STATUS_VALUES.contains(&status.as_str()),
            "case {case:?}: Status must be a modelled value, got {status:?}"
        );
        parse_time(case, "CreationTime", &string_member(&made, "CreationTime"));

        assert!(
            !seen_ids.contains(&id),
            "case {case:?}: each create mints a distinct Id, {id:?} is already among {seen_ids:?}"
        );
        seen_ids.push(id.clone());

        // Round trip: the reader answers the record the create wrote. `Status` is
        // the one member the clock is allowed to move, so it is compared apart.
        let fetched = ok(
            &router,
            "GetResolverQueryLogConfig",
            json!({ "ResolverQueryLogConfigId": id }),
        )
        .await;
        assert_eq!(
            without_status(config(&fetched)),
            without_status(&made),
            "case {case:?}: GetResolverQueryLogConfig must answer the stored configuration, got {fetched}"
        );
    }

    // Every configuration created above is still readable after the later
    // creates, so a create never disturbs the records beside it.
    settle().await;
    for id in &seen_ids {
        let fetched = ok(
            &router,
            "GetResolverQueryLogConfig",
            json!({ "ResolverQueryLogConfigId": id }),
        )
        .await;
        assert_eq!(
            string_member(config(&fetched), "Id"),
            *id,
            "case \"all configurations survive\": {id} must still be readable, got {fetched}"
        );
    }
}

/// A configuration answers `CREATING` while the creation the clock owes is still
/// outstanding, and `CREATED` once it has fallen due — without any call having to
/// move it.
#[tokio::test]
async fn a_created_config_settles_from_creating_to_created() {
    let router = emulator();
    router.hold_query_log_destinations(["arn:aws:s3:::amzn-s3-demo-bucket"]);
    let created = ok(
        &router,
        "CreateResolverQueryLogConfig",
        json!({
            "CreatorRequestId": "req-settling",
            "Name": "settling",
            "DestinationArn": "arn:aws:s3:::amzn-s3-demo-bucket",
        }),
    )
    .await;
    let id = string_member(config(&created), "Id");
    assert_eq!(
        string_member(config(&created), "Status"),
        "CREATING",
        "case \"straight after the create\": Resolver is still creating it, got {created}"
    );

    // Still creating well before the transition falls due: the span is never
    // zero, so a caller reading at once observes the transient status.
    let early = ok(
        &router,
        "GetResolverQueryLogConfig",
        json!({ "ResolverQueryLogConfigId": id.clone() }),
    )
    .await;
    assert_eq!(
        string_member(config(&early), "Status"),
        "CREATING",
        "case \"before the transition is due\": the status has not moved yet, got {early}"
    );

    settle().await;
    let settled = ok(
        &router,
        "GetResolverQueryLogConfig",
        json!({ "ResolverQueryLogConfigId": id.clone() }),
    )
    .await;
    assert_eq!(
        string_member(config(&settled), "Status"),
        "CREATED",
        "case \"after the transition is due\": the configuration is created, got {settled}"
    );

    // A read does not change what a later caller sees: the settled status is
    // reported again, and no other member moved with it.
    let again = ok(
        &router,
        "GetResolverQueryLogConfig",
        json!({ "ResolverQueryLogConfigId": id }),
    )
    .await;
    assert_eq!(
        config(&again),
        config(&settled),
        "case \"a second read\": reporting a configuration must not change it, got {again}"
    );
}

/// A retried create with the same `CreatorRequestId` resolves to the
/// configuration that request already made: `CreatorRequestId` "allows failed
/// requests to be retried without the risk of running the operation twice."
#[tokio::test]
async fn a_repeated_creator_request_id_does_not_create_a_second_config() {
    let router = emulator();
    router.hold_query_log_destinations([
        "arn:aws:s3:::amzn-s3-demo-bucket",
        "arn:aws:logs:us-west-2:123456789012:log-group:/queries:*",
    ]);

    for (case, body) in [
        (
            "retried unchanged",
            json!({
                "CreatorRequestId": "req-retried",
                "Name": "once-only",
                "DestinationArn": "arn:aws:s3:::amzn-s3-demo-bucket",
            }),
        ),
        (
            "retried with tags",
            json!({
                "CreatorRequestId": "req-retried-tagged",
                "Name": "once-only-tagged",
                "DestinationArn": "arn:aws:logs:us-west-2:123456789012:log-group:/queries:*",
                "Tags": [{ "Key": "env", "Value": "test" }],
            }),
        ),
    ] {
        let first = ok(&router, "CreateResolverQueryLogConfig", body.clone()).await;
        let second = ok(&router, "CreateResolverQueryLogConfig", body).await;
        assert_eq!(
            string_member(config(&first), "Id"),
            string_member(config(&second), "Id"),
            "case {case:?}: the retry must resolve to the configuration the first call made, got {second}"
        );
        assert_eq!(
            without_status(config(&first)),
            without_status(config(&second)),
            "case {case:?}: the retry must report that same configuration, got {second}"
        );
    }
}

/// A `DestinationArn` that names none of the destinations Resolver delivers
/// query logs to is refused with the `InvalidParameterException` the operation
/// models, and nothing is stored for it.
///
/// The rows walk what the rule generalises over: a string that is no ARN at all,
/// an ARN of a service that is not a log destination, and an ARN of one of the
/// three destination services whose resource part names no bucket, log group or
/// stream.
#[tokio::test]
async fn a_destination_that_is_no_log_destination_is_refused() {
    let router = emulator();
    router.hold_query_log_destinations(["arn:aws:s3:::amzn-s3-demo-bucket"]);

    for (case, destination) in [
        ("not an ARN at all", "amzn-s3-demo-bucket"),
        ("an ARN with too few fields", "arn:aws:s3:us-west-2:123456789012"),
        ("an empty S3 resource", "arn:aws:s3:::"),
        ("a service that is no log destination", "arn:aws:sqs:us-west-2:123456789012:queue"),
        (
            "a DynamoDB table",
            "arn:aws:dynamodb:us-west-2:123456789012:table/queries",
        ),
        (
            "a CloudWatch Logs resource that is no log group",
            "arn:aws:logs:us-west-2:123456789012:destination:queries",
        ),
        (
            "a Kinesis resource that is no stream",
            "arn:aws:kinesis:us-west-2:123456789012:consumer/my-consumer",
        ),
        (
            "a Firehose resource that is no delivery stream",
            "arn:aws:firehose:us-west-2:123456789012:stream/my-stream",
        ),
        ("an ARN with no partition", "arn::s3:::amzn-s3-demo-bucket"),
        ("an ARN-like string", "arn:aws:s3"),
    ] {
        let (status, body) = call(
            &router,
            "CreateResolverQueryLogConfig",
            json!({
                "CreatorRequestId": format!("req-{case}"),
                "Name": "bad-destination",
                "DestinationArn": destination,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a destination Resolver cannot deliver to must be refused, got {status}: {body}"
        );
        assert!(
            error_type(&body).contains("InvalidParameterException"),
            "case {case:?}: the refusal must name InvalidParameterException, got {body}"
        );
        assert_eq!(
            body.get("FieldName").and_then(Value::as_str),
            Some("DestinationArn"),
            "case {case:?}: the refusal must name the parameter that is not valid, got {body}"
        );
    }

    // A legal destination is accepted, so the refusals above are about the
    // destination and not about refusing every create.
    let created = ok(
        &router,
        "CreateResolverQueryLogConfig",
        json!({
            "CreatorRequestId": "req-good-destination",
            "Name": "good-destination",
            "DestinationArn": "arn:aws:s3:::amzn-s3-demo-bucket/logs/",
        }),
    )
    .await;
    assert_eq!(
        string_member(config(&created), "DestinationArn"),
        "arn:aws:s3:::amzn-s3-demo-bucket/logs/",
        "case \"a legal destination\": the create must be served, got {created}"
    );
}

/// A `Name` outside the model's constraints is refused with HTTP 400, and
/// nothing is stored for it.
///
/// The rows walk the published bounds of `Name` (length 1..64, pattern
/// `^(?!^[0-9]+$)([a-zA-Z0-9\-_' ']+)$`): one character past the maximum, the
/// empty string, the all-digits form the pattern's negative lookahead excludes,
/// and characters the pattern admits nothing of.
#[tokio::test]
async fn a_name_outside_the_modelled_constraints_is_refused() {
    let router = emulator();
    router.hold_query_log_destinations(["arn:aws:s3:::amzn-s3-demo-bucket"]);

    for (case, name) in [
        ("one character past the 64-character maximum", "a".repeat(65)),
        ("far past the maximum", "b".repeat(200)),
        ("empty, which the pattern's + excludes", String::new()),
        ("only digits", "12345".to_string()),
        ("one digit", "7".to_string()),
        ("a character outside the pattern", "bad/name".to_string()),
        ("a dot, which the pattern excludes", "config.one".to_string()),
        ("a non-ASCII letter", "requête".to_string()),
    ] {
        let (status, body) = call(
            &router,
            "CreateResolverQueryLogConfig",
            json!({
                "CreatorRequestId": format!("req-name-{case}"),
                "Name": name,
                "DestinationArn": "arn:aws:s3:::amzn-s3-demo-bucket",
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a Name outside the modelled constraints must be refused, got {status}: {body}"
        );
    }

    // The longest legal name is accepted, so the refusals above are about the
    // constraint and not about refusing long names outright.
    let created = ok(
        &router,
        "CreateResolverQueryLogConfig",
        json!({
            "CreatorRequestId": "req-name-at-the-bound",
            "Name": "c".repeat(64),
            "DestinationArn": "arn:aws:s3:::amzn-s3-demo-bucket",
        }),
    )
    .await;
    assert_eq!(
        string_member(config(&created), "Name").len(),
        64,
        "case \"at the bound\": a 64-character Name is legal, got {created}"
    );
}

/// `GetResolverQueryLogConfig` reports a `ResolverQueryLogConfigId` the store
/// holds no configuration under with the `ResourceNotFoundException` it models —
/// never an empty success, and never a configuration it invented.
#[tokio::test]
async fn an_unknown_config_id_is_reported_as_missing() {
    let router = emulator();
    router.hold_query_log_destinations(["arn:aws:s3:::amzn-s3-demo-bucket"]);
    let created = ok(
        &router,
        "CreateResolverQueryLogConfig",
        json!({
            "CreatorRequestId": "req-present",
            "Name": "present",
            "DestinationArn": "arn:aws:s3:::amzn-s3-demo-bucket",
        }),
    )
    .await;
    let present = string_member(config(&created), "Id");

    for (case, id) in [
        ("an id nothing minted", "rqlc-0123456789abcdef".to_string()),
        ("a Resolver rule id", "rslvr-rr-0123456789abcdef".to_string()),
        ("one character", "x".to_string()),
        // The stored id with its last character dropped: a near miss is still a
        // miss.
        (
            "a near miss of a stored id",
            present[..present.len() - 1].to_string(),
        ),
    ] {
        expect_error(
            &router,
            "GetResolverQueryLogConfig",
            json!({ "ResolverQueryLogConfigId": id }),
            case,
            "ResourceNotFoundException",
        )
        .await;
    }

    // The refusals above are about the id, not about refusing every read.
    let fetched = ok(
        &router,
        "GetResolverQueryLogConfig",
        json!({ "ResolverQueryLogConfigId": present.clone() }),
    )
    .await;
    assert_eq!(
        string_member(config(&fetched), "Id"),
        present,
        "case \"the stored id\": the read must answer the stored configuration, got {fetched}"
    );
}

/// The tags a create carries are kept with the configuration: a tag read through
/// `ListTagsForResource` on the configuration's own ARN reports them back.
#[tokio::test]
async fn the_tags_a_create_carries_are_stored_with_the_configuration() {
    let router = emulator();
    router.hold_query_log_destinations(["arn:aws:s3:::amzn-s3-demo-bucket"]);

    for (case, tags) in [
        ("no Tags member", json!(null)),
        ("an empty tag list", json!([])),
        (
            "two tags",
            json!([
                { "Key": "team", "Value": "net" },
                { "Key": "env", "Value": "prod" },
            ]),
        ),
    ] {
        let mut body = json!({
            "CreatorRequestId": format!("req-tags-{case}"),
            "Name": "tagged",
            "DestinationArn": "arn:aws:s3:::amzn-s3-demo-bucket",
        });
        if !tags.is_null() {
            body["Tags"] = tags.clone();
        }
        let created = ok(&router, "CreateResolverQueryLogConfig", body).await;
        let arn = string_member(config(&created), "Arn");

        let listed = ok(&router, "ListTagsForResource", json!({ "ResourceArn": arn })).await;
        let expected = if tags.is_null() { json!([]) } else { tags };
        assert_eq!(
            listed.get("Tags").cloned().unwrap_or(json!([])),
            expected,
            "case {case:?}: the tags the create carried must be readable on the configuration, got {listed}"
        );
    }
}

/// Many creates in one store each mint their own configuration and each read
/// back: the emulator's own ceiling on how many it holds is far above the handful
/// a caller makes, so an ordinary workload is served throughout.
#[tokio::test]
async fn many_creates_in_one_store_are_each_served_and_distinct() {
    let router = emulator();
    router.hold_query_log_destinations(
        (0..25).map(|index| format!("arn:aws:s3:::amzn-s3-demo-bucket-{index}")),
    );
    let mut ids: Vec<String> = Vec::new();
    for index in 0..25 {
        let created = ok(
            &router,
            "CreateResolverQueryLogConfig",
            json!({
                "CreatorRequestId": format!("req-bulk-{index}"),
                "Name": format!("bulk-{index}"),
                "DestinationArn": format!("arn:aws:s3:::amzn-s3-demo-bucket-{index}"),
            }),
        )
        .await;
        let id = string_member(config(&created), "Id");
        assert!(
            !ids.contains(&id),
            "case \"bulk create {index}\": each create mints a distinct Id, got {id:?} among {ids:?}"
        );
        ids.push(id);
    }

    for (index, id) in ids.iter().enumerate() {
        let fetched = ok(
            &router,
            "GetResolverQueryLogConfig",
            json!({ "ResolverQueryLogConfigId": id }),
        )
        .await;
        assert_eq!(
            string_member(config(&fetched), "DestinationArn"),
            format!("arn:aws:s3:::amzn-s3-demo-bucket-{index}"),
            "case \"bulk read {index}\": the read must answer the configuration that create wrote, got {fetched}"
        );
    }
}
