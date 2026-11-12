// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteResolverQueryLogConfig`.
//!
//! The rule these pin: the call "deletes a query logging configuration", so it
//! takes out of state EXACTLY the one configuration the id it names was stored
//! under, answers that record with `DELETING` ("Resolver is deleting the query
//! logging configuration") and otherwise verbatim, releases what the record
//! held (its `CreatorRequestId` and its tags), refuses an id the store holds no
//! configuration under with the `ResourceNotFoundException` it models, and —
//! "before you can delete a query logging configuration, you must first
//! disassociate all VPCs from the configuration" — refuses a configuration a
//! VPC is still associated with with the `InvalidRequestException` it models,
//! writing nothing. Every other configuration is left as it was.
//!
//! The table spans what the rule generalises over: each destination kind
//! `CreateResolverQueryLogConfig$DestinationArn` documents, the forms and
//! published length bounds a `Name` takes, a configuration with and without
//! tags, where in the store's creation order the configuration sits, and how
//! many VPCs are still associated with it.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::ops::types::RESOLVER_QUERY_LOG_CONFIG_STATUS_VALUES;

use super::harness::{
    call, emulator, error_type, ok, parse_time, string_member, without_status, Emulator,
};

/// The sentence Resolver answers for a query logging configuration it holds
/// nothing under: entirely fixed text behind the service error code for that
/// fault, naming no id.
const CONFIG_GONE_MESSAGE: &str =
    "[RSLVR-01601] The specified query logging configuration doesn't exist.";

/// The documented precondition of the delete, which is the sentence it answers
/// when the precondition is not met.
const STILL_ASSOCIATED_MESSAGE: &str = "Before you can delete a query logging configuration, \
                                        you must first disassociate all VPCs from the \
                                        configuration.";

/// The 64-character `Name` one row uses: the maximum of the model's length
/// 1..64.
const LONGEST_NAME: &str = "nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn";

/// One row of the delete table: a configuration to create and then delete.
struct DeleteCase {
    case: &'static str,
    creator_request_id: &'static str,
    name: &'static str,
    destination_arn: &'static str,
    /// The `Tags` the create carries, as the (`Key`, `Value`) pairs they are
    /// written with; empty for a row that carries no `Tags` member at all.
    tags: &'static [(&'static str, &'static str)],
}

/// The rows differ along what the rule generalises over: the destination kinds
/// `CreateResolverQueryLogConfig$DestinationArn` documents (an S3 bucket, a
/// CloudWatch Logs log group named with and without the `:*` log-stream
/// wildcard, a Kinesis Data Firehose delivery stream under both of its service
/// names, and a destination in another partition), the forms a `Name` takes
/// including both published bounds of its length 1..64
/// (`smithy_query(query="operation",operation="CreateResolverQueryLogConfig")`),
/// and whether the configuration carries tags. Every row is created on ONE
/// emulator and deleted in turn, so each delete is also a case of "one of
/// several stored configurations".
const DELETE_CASES: &[DeleteCase] = &[
    DeleteCase {
        case: "an S3 bucket, no tags",
        creator_request_id: "req-delete-s3",
        name: "queries-to-s3",
        destination_arn: "arn:aws:s3:::amzn-s3-demo-bucket",
        tags: &[],
    },
    DeleteCase {
        case: "a log group named with the stream wildcard, several tags",
        creator_request_id: "req-delete-logs",
        name: "ops' queries",
        destination_arn:
            "arn:aws:logs:us-west-1:123456789012:log-group:/mystack-testgroup-12ABC1AB12A1:*",
        tags: &[("team", "net"), ("env", "prod")],
    },
    DeleteCase {
        case: "a log group named without the stream wildcard",
        creator_request_id: "req-delete-logs-bare",
        name: "queries_to-logs 2",
        destination_arn: "arn:aws:logs:us-west-2:123456789012:log-group:/queries",
        tags: &[],
    },
    DeleteCase {
        case: "a Kinesis stream, the shortest Name the model allows",
        creator_request_id: "req-delete-kinesis",
        name: "q",
        destination_arn: "arn:aws:kinesis:us-east-2:012345678901:stream/my_stream_name",
        tags: &[("team", "net")],
    },
    DeleteCase {
        case: "a Firehose delivery stream, the longest Name the model allows",
        creator_request_id: "req-delete-firehose",
        name: LONGEST_NAME,
        destination_arn: "arn:aws:firehose:eu-west-1:012345678901:deliverystream/my-stream",
        tags: &[],
    },
    DeleteCase {
        case: "a destination in another partition",
        creator_request_id: "2026-10-29T19:32:14Z",
        name: "stamped-request",
        destination_arn: "arn:aws-us-gov:s3:::gov-bucket",
        tags: &[],
    },
];

/// The `ResolverQueryLogConfig` of a create/get/delete response.
fn config(body: &Value) -> &Value {
    body.get("ResolverQueryLogConfig")
        .unwrap_or_else(|| panic!("the response carries a ResolverQueryLogConfig, got {body}"))
}

/// Create the configuration `row` describes and answer the record the create
/// reported.
async fn create(router: &Emulator, row: &DeleteCase) -> Value {
    // The create resolves its `DestinationArn` against the store of the service
    // that owns the destination, so the account holds that destination first.
    router.hold_query_log_destinations([row.destination_arn]);
    let mut body = json!({
        "CreatorRequestId": row.creator_request_id,
        "Name": row.name,
        "DestinationArn": row.destination_arn,
    });
    if !row.tags.is_empty() {
        let tags: Vec<Value> = row
            .tags
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect();
        body.as_object_mut()
            .expect("the request body is an object")
            .insert("Tags".to_string(), Value::Array(tags));
    }
    let created = ok(router, "CreateResolverQueryLogConfig", body).await;
    config(&created).clone()
}

/// Read the configuration `id` back, or `None` when the store holds none under
/// it (which the reader reports with the fault that names the configuration).
async fn read(router: &Emulator, id: &str) -> Option<Value> {
    let (status, parsed) = call(
        router,
        "GetResolverQueryLogConfig",
        json!({ "ResolverQueryLogConfigId": id }),
    )
    .await;
    if status == 200 {
        return Some(config(&parsed).clone());
    }
    None
}

/// The `Id`s `ListResolverQueryLogConfigs` reports.
async fn listed_ids(router: &Emulator) -> Vec<String> {
    let page = ok(router, "ListResolverQueryLogConfigs", json!({})).await;
    page.get("ResolverQueryLogConfigs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|config| string_member(config, "Id"))
        .collect()
}

/// Assert `operation` refused `body` with `expected`, and answer the parsed
/// refusal so the caller can read its message.
async fn refusal(
    router: &Emulator,
    operation: &str,
    body: Value,
    case: &str,
    expected: &str,
) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: {operation} must answer HTTP 400, got {status}: {parsed}"
    );
    assert!(
        error_type(&parsed).contains(expected),
        "case {case:?}: {operation} must answer {expected}, got {parsed}"
    );
    parsed
}

/// A delete takes out exactly the configuration its id was stored under: the
/// answer is that record with `DELETING`, the record is gone from every reader
/// and from the listing, its tags go with it, and every other configuration is
/// exactly as it was.
#[tokio::test]
async fn a_delete_takes_out_exactly_the_configuration_it_names() {
    assert_eq!(
        LONGEST_NAME.len(),
        64,
        "the longest row must name a 64-character Name, got {}",
        LONGEST_NAME.len()
    );

    let router = emulator();
    let mut stored: Vec<(usize, Value)> = Vec::new();
    for (index, row) in DELETE_CASES.iter().enumerate() {
        stored.push((index, create(&router, row).await));
    }

    let mut remaining: Vec<(usize, Value)> = stored.clone();
    while !remaining.is_empty() {
        // Take rows out from the FRONT and the BACK in turn, so a deleted
        // configuration is sometimes the oldest in creation order and sometimes
        // the newest.
        let position = if remaining.len().is_multiple_of(2) {
            0
        } else {
            remaining.len() - 1
        };
        let (index, target) = remaining.remove(position);
        let row = &DELETE_CASES[index];
        let case = row.case;
        let id = string_member(&target, "Id");
        let arn = string_member(&target, "Arn");

        // What each surviving configuration reported before the call, so the
        // scope of the delete can be attributed to the one id it named.
        let mut survivors_before = Vec::new();
        for (_, survivor) in &remaining {
            let survivor_id = string_member(survivor, "Id");
            survivors_before.push((
                survivor_id.clone(),
                read(&router, &survivor_id)
                    .await
                    .unwrap_or_else(|| panic!("case {case:?}: {survivor_id} must still be stored")),
            ));
        }

        let answered = ok(
            &router,
            "DeleteResolverQueryLogConfig",
            json!({ "ResolverQueryLogConfigId": id.clone() }),
        )
        .await;
        let answered = config(&answered).clone();

        let status = string_member(&answered, "Status");
        assert_eq!(
            status, "DELETING",
            "case {case:?}: the configuration on its way out reports DELETING, got {answered}"
        );
        assert!(
            RESOLVER_QUERY_LOG_CONFIG_STATUS_VALUES.contains(&status.as_str()),
            "case {case:?}: Status must be a modelled value, got {status:?}"
        );
        assert_eq!(
            without_status(&answered),
            without_status(&target),
            "case {case:?}: only Status may differ from the configuration that was stored, got \
             {answered}"
        );
        assert_eq!(
            string_member(&answered, "DestinationArn"),
            string_member(&target, "DestinationArn"),
            "case {case:?}: the answer must report the destination the record was stored with, \
             got {answered}"
        );
        parse_time(
            case,
            "CreationTime",
            &string_member(&answered, "CreationTime"),
        );

        // Gone from the reader that addresses it by its id, and from a second
        // delete of the same id, both answered with the one fixed sentence
        // every query-logging operation reports a missing configuration with.
        for operation in ["GetResolverQueryLogConfig", "DeleteResolverQueryLogConfig"] {
            let parsed = refusal(
                &router,
                operation,
                json!({ "ResolverQueryLogConfigId": id.clone() }),
                case,
                "ResourceNotFoundException",
            )
            .await;
            assert_eq!(
                string_member(&parsed, "Message"),
                CONFIG_GONE_MESSAGE,
                "case {case:?}: {operation} must carry the service's own coded sentence, got \
                 {parsed}"
            );
            assert_eq!(
                string_member(&parsed, "ResourceType"),
                "ResolverQueryLogConfig",
                "case {case:?}: {operation} must name the resource that is missing, got {parsed}"
            );
        }

        // The tags the record held go with it: the ARN names no resource of
        // this account any more.
        refusal(
            &router,
            "ListTagsForResource",
            json!({ "ResourceArn": arn.clone() }),
            case,
            "ResourceNotFoundException",
        )
        .await;

        // Gone from the listing, which still reports every survivor.
        let listed = listed_ids(&router).await;
        assert!(
            !listed.contains(&id),
            "case {case:?}: the deleted configuration must leave the listing, got {listed:?}"
        );
        for (survivor_id, before) in &survivors_before {
            assert!(
                listed.contains(survivor_id),
                "case {case:?}: {survivor_id} must still be listed, got {listed:?}"
            );
            let after = read(&router, survivor_id)
                .await
                .unwrap_or_else(|| panic!("case {case:?}: {survivor_id} must survive the delete"));
            assert_eq!(
                without_status(&after),
                without_status(before),
                "case {case:?}: {survivor_id} must be untouched by the delete, got {after}"
            );
        }
    }

    let listed = listed_ids(&router).await;
    assert!(
        listed.is_empty(),
        "every configuration was deleted, so the listing reports none, got {listed:?}"
    );
}

/// The `CreatorRequestId` is released with the configuration: a create that
/// reuses it mints a NEW configuration rather than resolving to the one that was
/// deleted, as a repeat of a request id that is still spent does.
#[tokio::test]
async fn a_deleted_configuration_releases_its_creator_request_id() {
    let router = emulator();
    router.hold_query_log_destinations(["arn:aws:s3:::amzn-s3-demo-bucket"]);
    let request = json!({
        "CreatorRequestId": "req-released",
        "Name": "released",
        "DestinationArn": "arn:aws:s3:::amzn-s3-demo-bucket",
    });

    let mut seen: Vec<String> = Vec::new();
    for round in 0..3 {
        let case = format!("round {round}");
        let created = ok(&router, "CreateResolverQueryLogConfig", request.clone()).await;
        let id = string_member(config(&created), "Id");
        assert!(
            !seen.contains(&id),
            "case {case:?}: a create after the delete must mint a new configuration, got {id:?} \
             among {seen:?}"
        );
        seen.push(id.clone());

        // While the id is spent, the same request id resolves to THAT
        // configuration and creates nothing.
        let retried = ok(&router, "CreateResolverQueryLogConfig", request.clone()).await;
        assert_eq!(
            string_member(config(&retried), "Id"),
            id,
            "case {case:?}: a retry of a spent request id must answer the configuration it made, \
             got {retried}"
        );

        let deleted = ok(
            &router,
            "DeleteResolverQueryLogConfig",
            json!({ "ResolverQueryLogConfigId": id.clone() }),
        )
        .await;
        assert_eq!(
            string_member(config(&deleted), "Id"),
            id,
            "case {case:?}: the delete must answer the configuration it took out, got {deleted}"
        );
    }
}

/// One row of the association table: how many VPCs are associated with the
/// configuration, and how many of them are disassociated before the delete.
struct AssociatedCase {
    case: &'static str,
    vpcs: &'static [&'static str],
    /// How many of `vpcs`, from the front, are disassociated before the delete
    /// is attempted.
    disassociate: usize,
}

/// The rows differ along the count the rule generalises over: how many VPCs the
/// configuration still logs queries for when the delete arrives — all of them,
/// all but one, and none.
const ASSOCIATED_CASES: &[AssociatedCase] = &[
    AssociatedCase {
        case: "one VPC, still associated",
        vpcs: &["vpc-0123456789abcdef0"],
        disassociate: 0,
    },
    AssociatedCase {
        case: "three VPCs, none disassociated",
        vpcs: &["vpc-0a1b2c3d", "vpc-0123456789abcdef0", "vpc-00000000000000001"],
        disassociate: 0,
    },
    AssociatedCase {
        case: "three VPCs, all but the last disassociated",
        vpcs: &["vpc-0a1b2c3d", "vpc-0123456789abcdef0", "vpc-00000000000000001"],
        disassociate: 2,
    },
    AssociatedCase {
        case: "a one-character VPC id, ResourceId's minimum length",
        vpcs: &["v"],
        disassociate: 0,
    },
];

/// "Before you can delete a query logging configuration, you must first
/// disassociate all VPCs from the configuration": while ANY VPC is still
/// associated the delete is refused and nothing is written, and once the last
/// one has been disassociated the very same call is served.
#[tokio::test]
async fn a_configuration_a_vpc_is_still_associated_with_cannot_be_deleted() {
    for (index, row) in ASSOCIATED_CASES.iter().enumerate() {
        let case = row.case;
        let router = emulator();
        let created = create(
            &router,
            &DeleteCase {
                case,
                creator_request_id: "req-associated",
                name: "associated",
                destination_arn: "arn:aws:s3:::amzn-s3-demo-bucket",
                tags: &[],
            },
        )
        .await;
        let id = string_member(&created, "Id");
        router.hold_vpcs(row.vpcs);
        for vpc in row.vpcs {
            ok(
                &router,
                "AssociateResolverQueryLogConfig",
                json!({ "ResolverQueryLogConfigId": id.clone(), "ResourceId": vpc }),
            )
            .await;
        }
        for vpc in &row.vpcs[..row.disassociate] {
            ok(
                &router,
                "DisassociateResolverQueryLogConfig",
                json!({ "ResolverQueryLogConfigId": id.clone(), "ResourceId": vpc }),
            )
            .await;
        }
        let still_associated = row.vpcs.len() - row.disassociate;
        // The record as it stands when the delete arrives, so what a refused
        // delete may change can be compared against it.
        let before = read(&router, &id)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the configuration is stored"));

        let parsed = refusal(
            &router,
            "DeleteResolverQueryLogConfig",
            json!({ "ResolverQueryLogConfigId": id.clone() }),
            case,
            "InvalidRequestException",
        )
        .await;
        assert_eq!(
            string_member(&parsed, "Message"),
            STILL_ASSOCIATED_MESSAGE,
            "case {case:?}: the refusal must state the precondition the caller has to meet, got \
             {parsed}"
        );

        // Nothing was written: the configuration still reads back, and it still
        // counts every VPC that was not disassociated.
        let read_back = read(&router, &id)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: a refused delete must leave the record"));
        assert_eq!(
            without_status(&read_back),
            without_status(&before),
            "case {case:?}: a refused delete must leave the configuration as it was, got \
             {read_back}"
        );
        assert_eq!(
            string_member(&read_back, "Id"),
            string_member(&created, "Id"),
            "case {case:?}: a refused delete must leave the configuration the create made, got \
             {read_back}"
        );
        assert_eq!(
            read_back.get("AssociationCount").and_then(Value::as_i64),
            i64::try_from(still_associated).ok(),
            "case {case:?}: the configuration must still count {still_associated} VPCs, got \
             {read_back}"
        );

        // Once the last VPC has been disassociated, the same call is served.
        for vpc in &row.vpcs[row.disassociate..] {
            ok(
                &router,
                "DisassociateResolverQueryLogConfig",
                json!({ "ResolverQueryLogConfigId": id.clone(), "ResourceId": vpc }),
            )
            .await;
        }
        let answered = ok(
            &router,
            "DeleteResolverQueryLogConfig",
            json!({ "ResolverQueryLogConfigId": id.clone() }),
        )
        .await;
        let answered = config(&answered).clone();
        assert_eq!(
            string_member(&answered, "Id"),
            id,
            "case {case:?}: the delete must answer the configuration it took out, got {answered}"
        );
        assert_eq!(
            answered.get("AssociationCount").and_then(Value::as_i64),
            Some(0),
            "case {case:?}: the deleted configuration counts no VPC, got {answered}"
        );
        assert!(
            read(&router, &id).await.is_none(),
            "case {case:?}: row {index}'s configuration must be gone once the delete is served"
        );
    }
}
