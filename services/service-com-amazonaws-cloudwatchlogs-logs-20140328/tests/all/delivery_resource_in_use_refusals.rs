// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the refusal of deleting a delivery-pipeline resource that deliveries
//! still reference.
//!
//! The rule under test: "You can't delete a delivery destination if any current deliveries are
//! associated with it. To find whether any deliveries are associated with this delivery
//! destination, use the DescribeDeliveries operation" — and the same for a delivery source — so
//! such a delete is refused as a conflict whose message is FIXED for that error class apart
//! from the KIND of resource the delete addresses: it names neither the resource the request
//! named nor any of the deliveries that block it, and it directs the caller to enumerate the
//! deliveries itself, because ANY number of them can reference the resource. The refusal
//! changes nothing: the resource is still reported afterwards, and the delete is accepted once
//! the last delivery referencing it is gone.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a message a
//! handler does not actually answer cannot pass.
//!
//! The tables below span what the rule generalises over rather than one request: which end of
//! the pipeline the delete addresses (destination, source), which vendor's resource that end
//! represents, how many deliveries reference it (one, two, three) and how many of them have
//! been deleted when the refusal is asserted again, and what form the resource's name takes
//! (dashes, underscores, the longest name the model admits). The recorded request an issue
//! happens to mention is one row.

use serde_json::json;

use crate::harness::Logs;

/// The operations under test, and the reads that prove a refused delete changed nothing.
const DELETE_DESTINATION: &str = "DeleteDeliveryDestination";
const DELETE_SOURCE: &str = "DeleteDeliverySource";
const GET_DESTINATION: &str = "GetDeliveryDestination";
const GET_SOURCE: &str = "GetDeliverySource";
const PUT_DESTINATION: &str = "PutDeliveryDestination";
const PUT_SOURCE: &str = "PutDeliverySource";
const CREATE_DELIVERY: &str = "CreateDelivery";
const DELETE_DELIVERY: &str = "DeleteDelivery";

/// The code CloudWatch Logs answers a delete the state of the account refuses.
const CONFLICT: &str = "ConflictException";

/// The code CloudWatch Logs answers once the resource is gone.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The whole message the delete of an in-use delivery destination is refused with.
const DESTINATION_IN_USE: &str = "Could not delete this Delivery Destination as it is currently \
                                  in use. Please delete any Deliveries that reference this \
                                  resource and try again.";

/// The whole message the delete of an in-use delivery source is refused with.
const SOURCE_IN_USE: &str = "Could not delete this Delivery Source as it is currently in use. \
                             Please delete any Deliveries that reference this resource and try \
                             again.";

/// The longest name a delivery source or destination can carry: both `DeliverySourceName` and
/// `DeliveryDestinationName` are `@length(min: 1, max: 60)` with pattern `^[\w-]*$`
/// (`smithy_query(query="operation",operation="DeleteDeliveryDestination")`).
const MAX_NAME_LENGTH: usize = 60;

/// The resources a delivery destination represents, one per vendor `DescribeDeliveries`
/// documents ("The destination can be CloudWatch Logs, Amazon S3, Firehose or X-Ray").
const DESTINATION_RESOURCES: [(&str, &str); 4] = [
    (
        "cwl",
        "arn:aws:logs:eu-west-2:123456789012:log-group:/aws/vendedlogs/in-use:*",
    ),
    ("s3", "arn:aws:s3:::vended-logs-in-use"),
    (
        "firehose",
        "arn:aws:firehose:eu-west-2:123456789012:deliverystream/vended-logs-in-use",
    ),
    (
        "xray",
        "arn:aws:xray:eu-west-2:123456789012:segment-destination/vended-traces-in-use",
    ),
];

/// The resources a delivery source represents, each paired with a `logType` its vendor's entry
/// in the catalogue of `PutDeliverySourceRequest$logType` lists
/// (`smithy_query(query="doc",shape="PutDeliverySourceRequest",member="logType")`).
const SOURCE_RESOURCES: [(&str, &str, &str); 3] = [
    (
        "s3",
        "arn:aws:s3:::vended-logs-in-use-sender",
        "S3_SERVER_ACCESS_LOGS",
    ),
    (
        "alb",
        "arn:aws:elasticloadbalancing:eu-west-2:123456789012:loadbalancer/app/in-use/0123456789abcdef",
        "ALB_CONNECTION_LOGS",
    ),
    (
        "workmail",
        "arn:aws:workmail:eu-west-2:123456789012:organization/m-0123456789abcdef0123456789abcdef",
        "WORKMAIL_MAILBOX_ACCESS_LOGS",
    ),
];

/// Store one delivery source under `name`, over the resource `resource_arn` sending `log_type`.
async fn put_source(logs: &Logs, name: &str, resource_arn: &str, log_type: &str) {
    logs.ok(
        PUT_SOURCE,
        &json!({"name": name, "resourceArn": resource_arn, "logType": log_type}),
    )
    .await;
}

/// Store one delivery destination under `name` over `resource_arn`, answering its own ARN.
async fn put_destination(logs: &Logs, name: &str, resource_arn: &str) -> String {
    let answer = logs
        .ok(
            PUT_DESTINATION,
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

/// Pair one source with one destination, answering the delivery's own id.
async fn create_delivery(logs: &Logs, source: &str, destination_arn: &str) -> String {
    let answer = logs
        .ok(
            CREATE_DELIVERY,
            &json!({
                "deliverySourceName": source,
                "deliveryDestinationArn": destination_arn,
            }),
        )
        .await;
    answer["delivery"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("a create answers the delivery's id, got {answer}"))
        .to_string()
}

/// A name of exactly [`MAX_NAME_LENGTH`] characters that starts with `prefix`, for the row that
/// asserts the refusal carries nothing of even the longest name the model admits.
fn longest_name(prefix: &str) -> String {
    let mut name = prefix.to_string();
    while name.len() < MAX_NAME_LENGTH {
        name.push('x');
    }
    name.truncate(MAX_NAME_LENGTH);
    name
}

/// One end of the delivery pipeline: the operations that delete and read the resource a
/// delivery blocks the delete of, the member its read answers, and the whole message that
/// delete is refused with while a delivery references it.
struct End {
    delete: &'static str,
    get: &'static str,
    member: &'static str,
    message: &'static str,
}

/// The destination end: the resource logs are delivered TO.
const DESTINATION: End = End {
    delete: DELETE_DESTINATION,
    get: GET_DESTINATION,
    member: "deliveryDestination",
    message: DESTINATION_IN_USE,
};

/// The source end: the resource that sends the logs.
const SOURCE: End = End {
    delete: DELETE_SOURCE,
    get: GET_SOURCE,
    member: "deliverySource",
    message: SOURCE_IN_USE,
};

/// Assert `end`'s delete refuses `name` as in use, with that end's whole message and nothing
/// of the request or of the deliveries that block it, and that `end`'s read still reports the
/// resource afterwards.
async fn assert_in_use(logs: &Logs, case: &str, end: &End, name: &str, carried: &[String]) {
    let delete = end.delete;
    let expected = end.message;
    let (code, message) = logs.refusal(delete, &json!({"name": name})).await;
    assert_eq!(
        code, CONFLICT,
        "case {case:?} ({delete}): a resource deliveries reference is a conflict, got {code} {message:?}"
    );
    assert_eq!(
        message, expected,
        "case {case:?} ({delete}): expected {expected:?}, got {message:?}"
    );
    for value in carried {
        assert!(
            !message.contains(value.as_str()),
            "case {case:?} ({delete}): the refusal carries nothing of the request or of the \
             deliveries that block it, got {value:?} in {message:?}"
        );
    }

    // The refused delete changed nothing: the resource is still the one the account holds.
    let read = logs.ok(end.get, &json!({"name": name})).await;
    assert_eq!(
        read[end.member]["name"].as_str(),
        Some(name),
        "case {case:?} ({}): a refused delete leaves the resource as it was, got {read}",
        end.get
    );
}

/// Deleting a destination deliveries reference is refused with the error class's fixed in-use
/// text — whichever vendor it represents, however many deliveries reference it, and however
/// many of those have already been deleted — and is accepted once the last one is gone.
#[tokio::test]
async fn an_in_use_delivery_destination_is_refused_with_the_services_fixed_text() {
    let logs = Logs::new();

    // (case, how many deliveries reference the destination, its name)
    let rows: Vec<(&str, usize, String)> = vec![
        ("one delivery", 1, "in-use-dst-one".to_string()),
        ("two deliveries", 2, "in_use_dst_two".to_string()),
        ("three deliveries", 3, longest_name("in-use-dst-three-")),
    ];

    for (case, fan_out, name) in rows {
        let (vendor, resource_arn) = DESTINATION_RESOURCES[fan_out % DESTINATION_RESOURCES.len()];
        let case = format!("{case} on a {vendor} destination");
        let destination_arn = put_destination(&logs, &name, resource_arn).await;

        // Each delivery pairs this destination with a source of its own, so the destination is
        // referenced `fan_out` times over. A (resource, log type) pairing is represented by ONE
        // delivery source, so each of those sources represents its own resource of the vendor
        // the row names — the resource segment of its ARN carries the source's own name.
        let mut ids: Vec<String> = Vec::new();
        let mut sources: Vec<String> = Vec::new();
        for index in 0..fan_out {
            let (sender, sender_arn, log_type) = SOURCE_RESOURCES[index % SOURCE_RESOURCES.len()];
            let source = format!("{name}-src-{index}-{sender}");
            let source = source[source.len().saturating_sub(MAX_NAME_LENGTH)..].to_string();
            put_source(&logs, &source, &format!("{sender_arn}-{source}"), log_type).await;
            ids.push(create_delivery(&logs, &source, &destination_arn).await);
            sources.push(source);
        }

        // Every value a refusal built from the request or the store could carry.
        let mut carried: Vec<String> = ids.clone();
        carried.push(name.clone());
        carried.push(destination_arn.clone());
        carried.extend(sources.iter().cloned());

        // The refusal is identical while ANY delivery still references the destination, so it
        // is not computed from one blocking delivery: assert it, delete one delivery, repeat.
        for (deleted, id) in ids.iter().enumerate() {
            let case = format!("{case}, {deleted} of {fan_out} deliveries deleted");
            assert_in_use(&logs, &case, &DESTINATION, &name, &carried).await;
            logs.ok(DELETE_DELIVERY, &json!({"id": id})).await;
        }

        // With the last reference gone the delete is accepted, and the destination really is
        // gone — so the conflict above followed the account's state rather than the call.
        logs.ok(DELETE_DESTINATION, &json!({"name": name})).await;
        let code = logs.err(GET_DESTINATION, &json!({"name": name})).await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case:?}: the accepted delete removed the destination, got {code}"
        );
    }
}

/// The same rule at the other end of the pipeline: deleting a source deliveries reference is
/// refused with that error class's fixed in-use text, which names the kind of resource and
/// nothing else.
#[tokio::test]
async fn an_in_use_delivery_source_is_refused_with_the_services_fixed_text() {
    let logs = Logs::new();

    // (case, how many deliveries reference the source, its name)
    let rows: Vec<(&str, usize, String)> = vec![
        ("one delivery", 1, "in-use-src-one".to_string()),
        ("two deliveries", 2, "in_use_src_two".to_string()),
        ("three deliveries", 3, longest_name("in-use-src-three-")),
    ];

    for (case, fan_out, name) in rows {
        let (sender, sender_arn, log_type) = SOURCE_RESOURCES[fan_out % SOURCE_RESOURCES.len()];
        let case = format!("{case} on a {sender} source");
        put_source(&logs, &name, sender_arn, log_type).await;

        // Each delivery pairs this source with a destination of its own.
        let mut ids: Vec<String> = Vec::new();
        let mut destinations: Vec<String> = Vec::new();
        for index in 0..fan_out {
            let (vendor, resource_arn) = DESTINATION_RESOURCES[index % DESTINATION_RESOURCES.len()];
            let destination = format!("{name}-dst-{index}-{vendor}");
            let destination = destination[destination.len().saturating_sub(MAX_NAME_LENGTH)..]
                .to_string();
            let destination_arn = put_destination(&logs, &destination, resource_arn).await;
            ids.push(create_delivery(&logs, &name, &destination_arn).await);
            destinations.push(destination_arn);
        }

        let mut carried: Vec<String> = ids.clone();
        carried.push(name.clone());
        carried.extend(destinations.iter().cloned());

        for (deleted, id) in ids.iter().enumerate() {
            let case = format!("{case}, {deleted} of {fan_out} deliveries deleted");
            assert_in_use(&logs, &case, &SOURCE, &name, &carried).await;
            logs.ok(DELETE_DELIVERY, &json!({"id": id})).await;
        }

        logs.ok(DELETE_SOURCE, &json!({"name": name})).await;
        let code = logs.err(GET_SOURCE, &json!({"name": name})).await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case:?}: the accepted delete removed the source, got {code}"
        );
    }
}

/// The boundary of that fixed text: a delete that no delivery blocks is NOT a conflict — a name
/// the account does not hold is a not-found naming the kind of resource that is missing, and a
/// resource no delivery references is deleted. So the in-use text is answered for the in-use
/// state alone.
#[tokio::test]
async fn a_delete_no_delivery_blocks_is_not_an_in_use_conflict() {
    let logs = Logs::new();

    // (the delete, the name the account does not hold, the in-use text it must NOT answer)
    let absent: [(&str, &str, &str); 2] = [
        (DELETE_DESTINATION, "no-such-destination", DESTINATION_IN_USE),
        (DELETE_SOURCE, "no_such_source", SOURCE_IN_USE),
    ];
    for (delete, name, in_use) in absent {
        let (code, message) = logs.refusal(delete, &json!({"name": name})).await;
        assert_eq!(
            code, NOT_FOUND,
            "case a name the account does not hold ({delete}): expected {NOT_FOUND}, got {code} {message:?}"
        );
        assert_ne!(
            message, in_use,
            "case a name the account does not hold ({delete}): a missing resource is no conflict, got {message:?}"
        );
    }

    // A pipeline whose delivery is deleted first: both ends then go, in either order.
    let (_, resource_arn) = DESTINATION_RESOURCES[1];
    let (_, sender_arn, log_type) = SOURCE_RESOURCES[0];
    put_source(&logs, "unreferenced-src", sender_arn, log_type).await;
    let destination_arn = put_destination(&logs, "unreferenced-dst", resource_arn).await;
    let id = create_delivery(&logs, "unreferenced-src", &destination_arn).await;
    logs.ok(DELETE_DELIVERY, &json!({"id": id})).await;
    logs.ok(DELETE_SOURCE, &json!({"name": "unreferenced-src"}))
        .await;
    logs.ok(DELETE_DESTINATION, &json!({"name": "unreferenced-dst"}))
        .await;
    for (operation, name) in [
        (GET_SOURCE, "unreferenced-src"),
        (GET_DESTINATION, "unreferenced-dst"),
    ] {
        let code = logs.err(operation, &json!({"name": name})).await;
        assert_eq!(
            code, NOT_FOUND,
            "case a resource no delivery references ({operation}): it was deleted, got {code}"
        );
    }
}
