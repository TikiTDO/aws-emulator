// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the REGION a vended-log delivery resource belongs to: the delivery
//! sources, delivery destinations and deliveries of this account, across the endpoints that
//! serve them.
//!
//! The rule under test: each of these resources belongs to the Region the request that created
//! it was signed for, so every other Region's endpoint holds none of it — a read, a write or a
//! delete addressed there is refused with the operation's modelled `ResourceNotFoundException`
//! and that error class's fixed sentence, an enumeration there does not report it, and the same
//! name used in a second Region creates a SECOND, independent resource whose own ARN carries
//! that Region. Nothing a refused call touched changes what the creating Region's endpoint
//! reports.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it, with the SigV4
//! credential scope naming the Region — so a response a handler does not actually compute from
//! the shared store cannot pass.
//!
//! The table spans what the rule generalises over rather than one request: three Region pairs
//! (each drawn from a different part of the commercial partition, and each used in both
//! directions), each of the three resource kinds, each operation of each kind that resolves one
//! by name or id (the reader, the enumerator, the delete, the policy writes, the configuration
//! update and the tagging operations), and sending resources of several vendors. The recorded
//! request an issue happens to mention — a CloudFront access-log source created in us-east-1
//! and read from us-west-2 — is one row.

use serde_json::{json, Value};

use crate::harness::{error_code, Logs};

/// The operations under test, by the resource kind they address.
const PUT_SOURCE: &str = "PutDeliverySource";
const GET_SOURCE: &str = "GetDeliverySource";
const DELETE_SOURCE: &str = "DeleteDeliverySource";
const DESCRIBE_SOURCES: &str = "DescribeDeliverySources";
const PUT_DESTINATION: &str = "PutDeliveryDestination";
const GET_DESTINATION: &str = "GetDeliveryDestination";
const DELETE_DESTINATION: &str = "DeleteDeliveryDestination";
const DESCRIBE_DESTINATIONS: &str = "DescribeDeliveryDestinations";
const PUT_DESTINATION_POLICY: &str = "PutDeliveryDestinationPolicy";
const GET_DESTINATION_POLICY: &str = "GetDeliveryDestinationPolicy";
const DELETE_DESTINATION_POLICY: &str = "DeleteDeliveryDestinationPolicy";
const CREATE_DELIVERY: &str = "CreateDelivery";
const GET_DELIVERY: &str = "GetDelivery";
const DELETE_DELIVERY: &str = "DeleteDelivery";
const DESCRIBE_DELIVERIES: &str = "DescribeDeliveries";
const UPDATE_DELIVERY: &str = "UpdateDeliveryConfiguration";
const LIST_TAGS: &str = "ListTagsForResource";
const TAG: &str = "TagResource";
const UNTAG: &str = "UntagResource";

/// The modelled error code each refusal below carries.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The whole sentence each kind's operations refuse an unheld name or id with: the service
/// states that the requested resource is not in this account and interpolates no value, so a
/// name another Region holds a resource under is refused exactly like one nobody ever created.
const SOURCE_NOT_FOUND: &str = "Requested Delivery Source does not exist in this account.";
const DESTINATION_NOT_FOUND: &str =
    "Requested Delivery Destination does not exist in this account.";
const DELIVERY_NOT_FOUND: &str = "Requested Delivery does not exist in this account.";

/// The whole sentence the tagging operations refuse an ARN of a resource this endpoint holds
/// none of with: the service names the resource KIND that is missing.
const SOURCE_NOT_FOUND_FOR_TAGS: &str = "The specified delivery source does not exist.";

/// One pair of Regions, used in both directions: a resource created through `home`'s endpoint
/// must be absent from `away`'s, and the other way round.
///
/// Each pair is drawn from a different part of the commercial partition, because the rule is
/// about the Region a request is signed for and nothing about which Regions those are.
#[derive(Debug)]
struct Pair {
    /// What this row varies, for the assertion messages.
    label: &'static str,
    home: &'static str,
    away: &'static str,
    /// `resourceArn` — the resource whose logs the row's delivery source represents. A
    /// regional resource pins no Region, so the same ARN can be used from either endpoint.
    resource_arn: &'static str,
    /// The `logType` the row's source sends.
    log_type: &'static str,
    /// The resource the row's delivery destination represents.
    destination_resource_arn: &'static str,
    /// A `logType` the service of `destination_resource_arn` sends — which values are legal
    /// is a property of the SENDING service, so the row needs one for that resource too when
    /// it uses it as a second sender.
    destination_log_type: &'static str,
}

/// The Region pairs every case below runs over.
const PAIRS: [Pair; 3] = [
    Pair {
        label: "two Regions of the United States",
        home: "us-east-1",
        away: "us-west-2",
        resource_arn: "arn:aws:s3:::vended-logs-source-one",
        log_type: "S3_SERVER_ACCESS_LOGS",
        destination_resource_arn: "arn:aws:s3:::vended-logs-bucket-one",
        destination_log_type: "S3_SERVER_ACCESS_LOGS",
    },
    Pair {
        label: "a European and an Asia-Pacific Region",
        home: "eu-west-2",
        away: "ap-southeast-2",
        resource_arn:
            "arn:aws:workmail:eu-west-2:123456789012:organization/m-1234EXAMPLEabcd1234abcd1234abcd",
        log_type: "ACCESS_CONTROL_LOGS",
        destination_resource_arn:
            "arn:aws:firehose:eu-west-2:123456789012:deliverystream/vended-logs",
        // Firehose is not one of the log senders the `logType` catalogue names, so this
        // emulator publishes no list for it and the value is kept as given.
        destination_log_type: "APPLICATION_LOGS",
    },
    Pair {
        label: "two Regions neither of which is the caller's usual one",
        home: "ap-northeast-1",
        away: "eu-central-1",
        resource_arn:
            "arn:aws:elasticloadbalancing:ap-northeast-1:123456789012:loadbalancer/app/web/50dc6c495c0c9188",
        log_type: "ALB_ACCESS_LOGS",
        destination_resource_arn: "arn:aws:s3:::vended-logs-bucket-two",
        destination_log_type: "S3_SERVER_ACCESS_LOGS",
    },
];

/// Both directions of every pair: (case, the Region the resource is created in, the Region it
/// must be absent from, the row it is built from).
fn directions() -> Vec<(String, &'static str, &'static str, &'static Pair)> {
    let mut rows = Vec::new();
    for pair in &PAIRS {
        rows.push((
            format!("{} ({} -> {})", pair.label, pair.home, pair.away),
            pair.home,
            pair.away,
            pair,
        ));
        rows.push((
            format!("{} ({} -> {})", pair.label, pair.away, pair.home),
            pair.away,
            pair.home,
            pair,
        ));
    }
    rows
}

/// The `deliverySource` a put or a get reports.
fn source_of(case: &str, answer: &Value) -> Value {
    answer
        .get("deliverySource")
        .filter(|source| !source.is_null())
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a deliverySource, got {answer}"))
}

/// A string member of a reported resource.
fn member(case: &str, resource: &Value, key: &str) -> String {
    resource
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected {key} on {resource}"))
        .to_string()
}

/// A delivery source in `region`, answering the record the put reports.
async fn put_source(
    logs: &Logs,
    region: &str,
    name: &str,
    resource_arn: &str,
    log_type: &str,
) -> Value {
    let body = json!({
        "name": name,
        "resourceArn": resource_arn,
        "logType": log_type,
        "tags": {"env": "prod"},
    });
    let answer = logs.ok_in_region(region, PUT_SOURCE, &body).await;
    source_of(region, &answer)
}

/// A delivery destination in `region`, answering its own ARN.
async fn put_destination(logs: &Logs, region: &str, name: &str, resource_arn: &str) -> String {
    let answer = logs
        .ok_in_region(
            region,
            PUT_DESTINATION,
            &json!({
                "name": name,
                "deliveryDestinationConfiguration": {"destinationResourceArn": resource_arn},
            }),
        )
        .await;
    let destination = answer
        .get("deliveryDestination")
        .cloned()
        .unwrap_or_else(|| panic!("case {region}: expected a deliveryDestination, got {answer}"));
    member(region, &destination, "arn")
}

/// The names one Region's endpoint reports through `DescribeDeliverySources`.
async fn described_source_names(logs: &Logs, region: &str) -> Vec<String> {
    reported_members(logs, region, DESCRIBE_SOURCES, "deliverySources", "name").await
}

/// The names one Region's endpoint reports through `DescribeDeliveryDestinations`.
async fn described_destination_names(logs: &Logs, region: &str) -> Vec<String> {
    reported_members(
        logs,
        region,
        DESCRIBE_DESTINATIONS,
        "deliveryDestinations",
        "name",
    )
    .await
}

/// The ids one Region's endpoint reports through `DescribeDeliveries`.
async fn described_delivery_ids(logs: &Logs, region: &str) -> Vec<String> {
    reported_members(logs, region, DESCRIBE_DELIVERIES, "deliveries", "id").await
}

/// One member of every record a reporting operation lists through `region`'s endpoint.
async fn reported_members(
    logs: &Logs,
    region: &str,
    operation: &str,
    list: &str,
    key: &str,
) -> Vec<String> {
    let page = logs.ok_in_region(region, operation, &json!({})).await;
    page.get(list)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{operation} in {region}: expected {list}, got {page}"))
        .iter()
        .map(|record| member(region, record, key))
        .collect()
}

/// Assert that `operation`, addressed to `region`, is refused with the modelled not-found code
/// and the whole sentence its error class carries.
async fn refused_as_absent(
    logs: &Logs,
    case: &str,
    region: &str,
    operation: &str,
    body: &Value,
    expected_message: &str,
) {
    let (code, message) = logs.refusal_in_region(region, operation, body).await;
    assert_eq!(
        code, NOT_FOUND,
        "case {case:?}: expected {operation} in {region} to answer {NOT_FOUND}, got {code} \
         {message:?}"
    );
    assert_eq!(
        message, expected_message,
        "case {case:?}: expected {operation} in {region} to answer {expected_message:?}, got \
         {message:?}"
    );
}

/// A delivery source is absent from every Region but the one its put was signed for: the
/// reader, the enumerator, the delete and the tagging operations there all answer as they do
/// for a name nobody created, the creating Region keeps reporting it unchanged, and the same
/// name put in the other Region is a SECOND source carrying that Region's own ARN.
#[tokio::test]
async fn a_delivery_source_is_held_only_by_the_region_that_created_it() {
    let logs = Logs::new();

    for (case, home, away, pair) in directions() {
        let name = format!("source-{home}-{away}");
        let created = put_source(&logs, home, &name, pair.resource_arn, pair.log_type).await;
        let home_arn = member(&case, &created, "arn");
        assert!(
            home_arn.contains(home),
            "case {case:?}: expected the source's ARN to name {home}, got {home_arn}"
        );

        // The reader, the delete and the enumerator of the away Region hold none of it.
        let by_name = json!({"name": name});
        for operation in [GET_SOURCE, DELETE_SOURCE] {
            refused_as_absent(&logs, &case, away, operation, &by_name, SOURCE_NOT_FOUND).await;
        }
        let listed = described_source_names(&logs, away).await;
        assert!(
            !listed.contains(&name),
            "case {case:?}: {DESCRIBE_SOURCES} in {away} must not report {name}, got {listed:?}"
        );

        // Nor do the tagging operations, addressed by the ARN the creating Region minted.
        for (operation, body) in [
            (LIST_TAGS, json!({"resourceArn": home_arn})),
            (TAG, json!({"resourceArn": home_arn, "tags": {"owner": "away"}})),
            (UNTAG, json!({"resourceArn": home_arn, "tagKeys": ["env"]})),
        ] {
            refused_as_absent(
                &logs,
                &case,
                away,
                operation,
                &body,
                SOURCE_NOT_FOUND_FOR_TAGS,
            )
            .await;
        }

        // Every refusal above left the source the creating Region holds exactly as it was.
        let read = source_of(
            &case,
            &logs.ok_in_region(home, GET_SOURCE, &by_name).await,
        );
        assert_eq!(
            read, created,
            "case {case:?}: expected {home} to still report {created}, got {read}"
        );

        // The same name in the away Region is a second, independent source: it is accepted
        // even for ANOTHER sending resource (no cross-Region conflict), carries that Region's
        // own ARN, and leaves the first one alone.
        let elsewhere = put_source(
            &logs,
            away,
            &name,
            pair.destination_resource_arn,
            pair.destination_log_type,
        )
        .await;
        let away_arn = member(&case, &elsewhere, "arn");
        assert_ne!(
            away_arn, home_arn,
            "case {case:?}: two Regions' sources of one name are two resources, got {away_arn} \
             for both"
        );
        assert!(
            away_arn.contains(away),
            "case {case:?}: expected the second source's ARN to name {away}, got {away_arn}"
        );
        let read = source_of(
            &case,
            &logs.ok_in_region(home, GET_SOURCE, &by_name).await,
        );
        assert_eq!(
            read, created,
            "case {case:?}: a put in {away} must not disturb {home}, expected {created}, got \
             {read}"
        );
    }
}

/// A delivery destination is absent from every Region but the one its put was signed for —
/// including for the three operations that address its policy, which is the destination's own
/// state.
#[tokio::test]
async fn a_delivery_destination_is_held_only_by_the_region_that_created_it() {
    let logs = Logs::new();
    // The policy assigned below is a resource-based IAM policy document, which is what
    // `PutDeliveryDestinationPolicy` assigns ("Creates and assigns an IAM policy that grants
    // permissions to CloudWatch Logs to deliver logs cross-account"): the two statements the
    // operation documents, each granting an account a delivery action on this account's
    // delivery resources.
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Effect": "Allow",
                "Principal": {"AWS": "arn:aws:iam::210987654321:root"},
                "Action": ["logs:GetDeliveryDestination", "logs:PutDeliverySource"],
                "Resource": "*",
            },
            {
                "Effect": "Allow",
                "Principal": {"AWS": "arn:aws:iam::210987654321:root"},
                "Action": ["logs:CreateDelivery", "logs:UpdateDeliveryConfiguration"],
                "Resource": "*",
            },
        ],
    })
    .to_string();

    for (case, home, away, pair) in directions() {
        let name = format!("destination-{home}-{away}");
        let home_arn =
            put_destination(&logs, home, &name, pair.destination_resource_arn).await;
        assert!(
            home_arn.contains(home),
            "case {case:?}: expected the destination's ARN to name {home}, got {home_arn}"
        );
        logs.ok_in_region(
            home,
            PUT_DESTINATION_POLICY,
            &json!({"deliveryDestinationName": name, "deliveryDestinationPolicy": policy}),
        )
        .await;

        let by_name = json!({"name": name});
        for operation in [GET_DESTINATION, DELETE_DESTINATION] {
            refused_as_absent(&logs, &case, away, operation, &by_name, DESTINATION_NOT_FOUND)
                .await;
        }
        let by_policy_name = json!({"deliveryDestinationName": name});
        for (operation, body) in [
            (GET_DESTINATION_POLICY, by_policy_name.clone()),
            (DELETE_DESTINATION_POLICY, by_policy_name.clone()),
            (
                PUT_DESTINATION_POLICY,
                json!({"deliveryDestinationName": name, "deliveryDestinationPolicy": policy}),
            ),
        ] {
            refused_as_absent(&logs, &case, away, operation, &body, DESTINATION_NOT_FOUND).await;
        }
        let listed = described_destination_names(&logs, away).await;
        assert!(
            !listed.contains(&name),
            "case {case:?}: {DESCRIBE_DESTINATIONS} in {away} must not report {name}, got \
             {listed:?}"
        );

        // The creating Region still holds the destination AND the policy assigned to it: the
        // refused delete and the refused policy delete took nothing away.
        let answer = logs.ok_in_region(home, GET_DESTINATION_POLICY, &by_policy_name).await;
        let reported = answer
            .get("policy")
            .and_then(|wrapper| wrapper.get("deliveryDestinationPolicy"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected the stored policy, got {answer}"));
        assert_eq!(
            reported, policy,
            "case {case:?}: expected {home} to still report the assigned policy, got \
             {reported:?}"
        );

        // The same name in the away Region is a second destination, with its own ARN.
        let away_arn = put_destination(&logs, away, &name, pair.destination_resource_arn).await;
        assert_ne!(
            away_arn, home_arn,
            "case {case:?}: two Regions' destinations of one name are two resources, got \
             {away_arn} for both"
        );
        // And it carries no policy: the one assigned in the creating Region is not its, so the
        // `policy` structure it reports is empty.
        let answer = logs.ok_in_region(away, GET_DESTINATION_POLICY, &by_policy_name).await;
        assert!(
            answer
                .get("policy")
                .is_some_and(|policy| policy.get("deliveryDestinationPolicy").is_none()),
            "case {case:?}: the second destination carries no policy of its own, got {answer}"
        );
    }
}

/// A delivery is absent from every Region but the one its create was signed for, and a create
/// cannot pair resources of another Region: the source it names must be one THIS endpoint
/// holds.
#[tokio::test]
async fn a_delivery_is_held_only_by_the_region_that_created_it() {
    let logs = Logs::new();

    for (case, home, away, pair) in directions() {
        let source_name = format!("delivery-source-{home}-{away}");
        let destination_name = format!("delivery-destination-{home}-{away}");
        put_source(&logs, home, &source_name, pair.resource_arn, pair.log_type).await;
        let destination_arn =
            put_destination(&logs, home, &destination_name, pair.destination_resource_arn)
                .await;

        let created = logs
            .ok_in_region(
                home,
                CREATE_DELIVERY,
                &json!({
                    "deliverySourceName": source_name,
                    "deliveryDestinationArn": destination_arn,
                }),
            )
            .await;
        let delivery = created
            .get("delivery")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected a delivery, got {created}"));
        let id = member(&case, &delivery, "id");

        // A create in the away Region cannot reach the source or the destination of the home
        // one: both are resolved among that endpoint's own resources, and the destination is
        // the one the create resolves first.
        refused_as_absent(
            &logs,
            &case,
            away,
            CREATE_DELIVERY,
            &json!({
                "deliverySourceName": source_name,
                "deliveryDestinationArn": destination_arn,
            }),
            DESTINATION_NOT_FOUND,
        )
        .await;

        // The delivery's own operations in the away Region hold none of it.
        let by_id = json!({"id": id});
        for operation in [GET_DELIVERY, DELETE_DELIVERY, UPDATE_DELIVERY] {
            refused_as_absent(&logs, &case, away, operation, &by_id, DELIVERY_NOT_FOUND).await;
        }
        let listed = described_delivery_ids(&logs, away).await;
        assert!(
            !listed.contains(&id),
            "case {case:?}: {DESCRIBE_DELIVERIES} in {away} must not report {id}, got {listed:?}"
        );

        // And the creating Region still holds it, unchanged by any of the refusals.
        let answer = logs.ok_in_region(home, GET_DELIVERY, &by_id).await;
        let read = answer
            .get("delivery")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected a delivery, got {answer}"));
        assert_eq!(
            read, delivery,
            "case {case:?}: expected {home} to still report {delivery}, got {read}"
        );
    }
}

/// A delete addressed to the wrong Region destroys nothing: the source the away endpoint
/// refuses is still there afterwards, still in use by its own Region's delivery (so its own
/// Region refuses the delete as a conflict), and still deletable once that delivery is gone.
#[tokio::test]
async fn a_delete_addressed_to_another_region_destroys_nothing() {
    let logs = Logs::new();

    for (case, home, away, pair) in directions() {
        let source_name = format!("kept-source-{home}-{away}");
        let destination_name = format!("kept-destination-{home}-{away}");
        put_source(&logs, home, &source_name, pair.resource_arn, pair.log_type).await;
        let destination_arn =
            put_destination(&logs, home, &destination_name, pair.destination_resource_arn)
                .await;
        let created = logs
            .ok_in_region(
                home,
                CREATE_DELIVERY,
                &json!({
                    "deliverySourceName": source_name,
                    "deliveryDestinationArn": destination_arn,
                }),
            )
            .await;
        let id = member(
            &case,
            created
                .get("delivery")
                .unwrap_or_else(|| panic!("case {case:?}: expected a delivery, got {created}")),
            "id",
        );

        // The away endpoint refuses the delete of each resource as absent...
        refused_as_absent(
            &logs,
            &case,
            away,
            DELETE_SOURCE,
            &json!({"name": source_name}),
            SOURCE_NOT_FOUND,
        )
        .await;
        refused_as_absent(
            &logs,
            &case,
            away,
            DELETE_DESTINATION,
            &json!({"name": destination_name}),
            DESTINATION_NOT_FOUND,
        )
        .await;

        // ...while the creating Region refuses the SAME deletes for the reason its own state
        // gives: the delivery it holds still references both.
        for (operation, body) in [
            (DELETE_SOURCE, json!({"name": source_name})),
            (DELETE_DESTINATION, json!({"name": destination_name})),
        ] {
            let (status, answer) = logs.call_in_region(home, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected {operation} in {home} to be refused, got {status} \
                 {answer}"
            );
            let code = error_code(&answer);
            assert_eq!(
                code, "ConflictException",
                "case {case:?}: expected {operation} in {home} to answer ConflictException, got \
                 {code} {answer}"
            );
        }

        // Once that delivery is deleted through the Region that holds it, both deletes are
        // served there — so the resources really were the creating Region's all along.
        logs.ok_in_region(home, DELETE_DELIVERY, &json!({"id": id}))
            .await;
        logs.ok_in_region(home, DELETE_SOURCE, &json!({"name": source_name}))
            .await;
        logs.ok_in_region(home, DELETE_DESTINATION, &json!({"name": destination_name}))
            .await;
        let listed = described_source_names(&logs, home).await;
        assert!(
            !listed.contains(&source_name),
            "case {case:?}: {DESCRIBE_SOURCES} in {home} must not report the deleted \
             {source_name}, got {listed:?}"
        );
    }
}

/// The recorded shape of the defect, as a client meets it: a CloudFront access-log source can
/// only be created in the Region that global service is reached through, and reading it from
/// any other Region's endpoint is refused — not answered with the us-east-1 resource.
#[tokio::test]
async fn a_source_of_a_global_sender_is_read_only_in_the_region_that_holds_it() {
    let logs = Logs::new();
    // CloudFront is reached through the commercial partition's one global endpoint, whose
    // Region is us-east-1, so that is the only Region a delivery source for a distribution can
    // be created in; the `logType` for it is ACCESS_LOGS.
    let home = "us-east-1";
    let distribution = "arn:aws:cloudfront::123456789012:distribution/E1EXAMPLE1EXAMPLE";
    let name = "cloudfront-access-logs";

    let created = put_source(&logs, home, name, distribution, "ACCESS_LOGS").await;
    let by_name = json!({"name": name});

    for away in ["us-west-2", "eu-west-2", "ap-southeast-2", "ca-central-1"] {
        let case = format!("read from {away}");
        refused_as_absent(&logs, &case, away, GET_SOURCE, &by_name, SOURCE_NOT_FOUND).await;
        let listed = described_source_names(&logs, away).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: {away} holds no delivery source at all, got {listed:?}"
        );
    }

    let read = source_of("home", &logs.ok_in_region(home, GET_SOURCE, &by_name).await);
    assert_eq!(
        read, created,
        "case the home Region: expected {created}, got {read}"
    );
}
