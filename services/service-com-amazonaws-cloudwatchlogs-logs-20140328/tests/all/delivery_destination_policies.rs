// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `PutDeliveryDestinationPolicy` and `GetDeliveryDestinationPolicy`:
//! the IAM policy an account assigns to one of its delivery destinations.
//!
//! The rule under test: a delivery destination holds at most one delivery destination
//! policy, so assigning one stores the DOCUMENT the request carries on the destination it
//! names and REPLACES whatever that destination held; the assigning call reports the document
//! as the request wrote it, and every later read reports the STORED form — the document's own
//! members, values and order, with the insignificant whitespace between its JSON tokens gone,
//! because the service reads the document rather than keeping the bytes the caller's renderer
//! produced; a destination that has had none assigned reports no policy at all; a
//! `deliveryDestinationName` the account does not hold is refused as a missing resource by
//! both operations; and a policy that is not a delivery destination policy — one that is not
//! an IAM resource policy document at all, or one whose statements are not grants of log
//! delivery (naming the delivery resources they cover, the delivery actions they allow on
//! them, and the ACCOUNT they are granted to) — is refused as an invalid parameter.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so what is
//! asserted here is what a client reads off the wire, and a response that is not computed
//! from the shared store cannot pass.
//!
//! The table spans what the rule generalises over rather than one request: policy documents
//! of several forms and sizes (one statement, two and three, a statement written as the object
//! itself, each element spelled both as one value and as a list and both ways round, actions
//! written out and as the patterns IAM admits, account principals as a root ARN and as a bare
//! id, nested conditions, non-ASCII contents, pretty-printed and spaced whitespace, and a
//! document on the published maximum length and one past it), destinations of several
//! `DeliveryDestinationType`s, and destination names at both published length bounds.

use serde_json::{json, Value};

use crate::harness::{Logs, REGION};

/// The operations under test, plus the destination operations a round trip goes through.
const PUT_POLICY: &str = "PutDeliveryDestinationPolicy";
const GET_POLICY: &str = "GetDeliveryDestinationPolicy";
const PUT_DESTINATION: &str = "PutDeliveryDestination";
const GET_DESTINATION: &str = "GetDeliveryDestination";

/// The refusals these operations model for the cases below.
const VALIDATION: &str = "ValidationException";
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The whole message a `deliveryDestinationPolicy` that is not a delivery destination policy
/// is refused with: fixed text for that class, stating neither the destination nor anything of
/// the document it would not take.
const INVALID_POLICY: &str = "Invalid Delivery Destination resource Policy is provided.";

/// Destination resources of three vendors a delivery destination can represent, so the rule
/// is asserted on more than one kind of destination.
const BUCKET_ARN: &str = "arn:aws:s3:::vended-logs-bucket";
const LOG_GROUP_ARN: &str = "arn:aws:logs:eu-west-2:123456789012:log-group:/aws/vendedlogs/one:*";
const FIREHOSE_ARN: &str = "arn:aws:firehose:eu-west-2:123456789012:deliverystream/vended-logs";

/// The `DeliveryDestinationType` of a trace destination, the one kind that is created with no
/// `deliveryDestinationConfiguration`. Source: the model's `DeliveryDestinationType` enum.
const TRACE: &str = "XRAY";

/// The published bounds of `DeliveryDestinationPolicy`. Source: the model's
/// `DeliveryDestinationPolicy` shape (`@length` 1..51200), which
/// `PutDeliveryDestinationPolicyRequest$deliveryDestinationPolicy` and `Policy$deliveryDestinationPolicy`
/// both target.
const POLICY_MAX_LENGTH: usize = 51_200;

/// A delivery destination to assign policies to: how it is created, and the name it is
/// created under.
#[derive(Debug)]
struct Destination {
    /// What this row varies, for the assertion messages.
    label: &'static str,
    /// `name` (`DeliveryDestinationName`, 1..60, `^[\w-]*$`).
    name: String,
    /// The `destinationResourceArn` this destination represents, or `None` for the trace
    /// destination, which is created from its type alone.
    resource_arn: Option<&'static str>,
}

impl Destination {
    fn of(label: &'static str, name: &str, resource_arn: &'static str) -> Self {
        Self {
            label,
            name: name.to_string(),
            resource_arn: Some(resource_arn),
        }
    }

    /// The `PutDeliveryDestination` request that creates this destination.
    fn put_body(&self) -> Value {
        match self.resource_arn {
            Some(resource_arn) => json!({
                "name": self.name,
                "deliveryDestinationConfiguration": {"destinationResourceArn": resource_arn},
            }),
            None => json!({"name": self.name, "deliveryDestinationType": TRACE}),
        }
    }
}

/// Destinations spanning the vendors and both published bounds of `DeliveryDestinationName`
/// (`@length` 1..60).
fn destinations() -> Vec<Destination> {
    vec![
        Destination::of("an S3 bucket destination", "vended-logs-bucket", BUCKET_ARN),
        Destination::of("a log group destination", "vended_logs_group", LOG_GROUP_ARN),
        Destination::of("a Firehose destination", "vended-logs-stream", FIREHOSE_ARN),
        Destination {
            label: "a trace destination, created from its type alone",
            name: "vended-traces".to_string(),
            resource_arn: None,
        },
        Destination::of("a name at the minimum length", "d", BUCKET_ARN),
        Destination::of(
            "a name at the maximum length",
            &"n".repeat(60),
            FIREHOSE_ARN,
        ),
    ]
}

/// One policy document, as a client sends it.
#[derive(Debug)]
struct PolicyCase {
    /// What this row varies, for the assertion messages.
    label: &'static str,
    /// The `deliveryDestinationPolicy` string the request carries.
    document: String,
    /// The text every LATER reader reports for it: the document with the whitespace between
    /// its JSON tokens dropped, and nothing else about it changed.
    reported: String,
}

impl PolicyCase {
    /// A document already carrying no insignificant whitespace, so it is reported as sent.
    fn of(label: &'static str, document: &str) -> Self {
        Self {
            label,
            document: document.to_string(),
            reported: document.to_string(),
        }
    }

    /// A document whose spelling carries whitespace between its tokens, paired with the text
    /// the service reports for it.
    fn spaced(label: &'static str, document: &str, reported: &str) -> Self {
        Self {
            label,
            document: document.to_string(),
            reported: reported.to_string(),
        }
    }
}

/// The vended-log delivery resources a delivery destination policy grants access to: every
/// delivery destination this account holds in this Region, every delivery source and every
/// delivery, named by the ARN this service builds for one — so no account id is written here.
fn delivery_destinations() -> String {
    ::engine::emulator_arn("logs", REGION, "delivery-destination:*")
}

fn delivery_sources() -> String {
    ::engine::emulator_arn("logs", REGION, "delivery-source:*")
}

fn deliveries() -> String {
    ::engine::emulator_arn("logs", REGION, "delivery:*")
}

/// The account the cross-account grants below are made to, as an IAM root ARN and as a bare
/// account id — the two spellings an account principal is written in.
const OTHER_ACCOUNT_ROOT: &str = "arn:aws:iam::210987654321:root";
const OTHER_ACCOUNT: &str = "210987654321";

/// The two statements the operation's documentation describes — "One statement enables
/// general logs delivery, and the other allows delivery to the chosen destination" — as one
/// IAM policy document.
fn two_statement_policy(destination_arn: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Sid": "AllowLogDeliveryActions",
                "Effect": "Allow",
                "Principal": {"AWS": OTHER_ACCOUNT_ROOT},
                "Action": [
                    "logs:PutDeliverySource",
                    "logs:PutDeliveryDestination",
                    "logs:CreateDelivery",
                ],
                "Resource": [delivery_sources(), deliveries()],
            },
            {
                "Sid": "AllowDeliveryToDestination",
                "Effect": "Allow",
                "Principal": {"AWS": OTHER_ACCOUNT_ROOT},
                "Action": "logs:CreateDelivery",
                "Resource": destination_arn,
            },
        ],
    })
    .to_string()
}

/// A delivery destination policy of exactly `length` characters: the two statements padded
/// through the first one's `Sid` value, so a case can sit on a published length bound.
fn policy_of_length(length: usize) -> String {
    let skeleton = padded_policy("");
    let padding = length - skeleton.len();
    padded_policy(&"p".repeat(padding))
}

/// The two statements the operation documents, the first carrying `sid`: each states an
/// effect, the account it grants to, a delivery action it may take and the delivery resource it
/// may take it on — the elements a delivery destination policy's statements state.
fn padded_policy(sid: &str) -> String {
    let destinations = delivery_destinations();
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Sid\":\"{sid}\",\
         \"Effect\":\"Allow\",\"Principal\":{{\"AWS\":\"{OTHER_ACCOUNT_ROOT}\"}},\
         \"Action\":\"logs:GetDeliveryDestination\",\"Resource\":\"{destinations}\"}},\
         {{\"Sid\":\"general\",\
         \"Effect\":\"Allow\",\"Principal\":{{\"AWS\":\"{OTHER_ACCOUNT_ROOT}\"}},\
         \"Action\":\"logs:CreateDelivery\",\"Resource\":\"{destinations}\"}}]}}"
    )
}

/// One resource-policy statement naming a delivery destination of this account: the elements
/// the IAM policy language requires of a resource-based statement, with no `Sid`.
fn one_statement_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": OTHER_ACCOUNT_ROOT},
            "Action": "logs:CreateDelivery",
            "Resource": delivery_destinations(),
        }],
    })
    .to_string()
}

/// A resource-policy document whose statement grants access to `resource` rather than to a
/// resource of the vended-log delivery surface.
fn policy_for(resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": OTHER_ACCOUNT_ROOT},
            "Action": "logs:CreateDelivery",
            "Resource": resource,
        }],
    })
    .to_string()
}

/// Policy documents spanning form, size, nesting, character set and whitespace.
fn policies() -> Vec<PolicyCase> {
    vec![
        PolicyCase::of(
            "the two statements the operation documents",
            &two_statement_policy(&delivery_destinations()),
        ),
        PolicyCase::of(
            "one statement naming every delivery destination of this account",
            &one_statement_policy(),
        ),
        PolicyCase::of(
            "a statement written as the object itself rather than as a list",
            &json!({
                "Version": "2012-10-17",
                "Statement": {
                    "Effect": "Allow",
                    "Principal": {"AWS": OTHER_ACCOUNT_ROOT},
                    "Action": "logs:CreateDelivery",
                    "Resource": deliveries(),
                },
            })
            .to_string(),
        ),
        PolicyCase::of(
            "two statements, one of them with a nested condition",
            &json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Principal": {"AWS": OTHER_ACCOUNT_ROOT},
                        "Action": "logs:DescribeDeliveries",
                        "Resource": "*",
                        "Condition": {
                            "StringEquals": {
                                "aws:SourceAccount": ["123456789012", OTHER_ACCOUNT],
                            },
                            "ArnLike": {"aws:SourceArn": "arn:aws:logs:eu-west-2:123456789012:*"},
                        },
                    },
                    {
                        "Effect": "Allow",
                        "Principal": {"AWS": OTHER_ACCOUNT_ROOT},
                        "Action": "logs:CreateDelivery",
                        "Resource": deliveries(),
                    },
                ],
            })
            .to_string(),
        ),
        PolicyCase::of(
            "a policy carrying non-ASCII contents",
            &padded_policy("lieferung-\u{00fc}ber-gr\u{00f6}\u{00df}e-\u{65e5}\u{672c}"),
        ),
        PolicyCase::of(
            "statements spelled across both of each element's forms",
            &json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Effect": "Deny",
                        "NotPrincipal": {"AWS": OTHER_ACCOUNT_ROOT},
                        "NotAction": "logs:DeleteDelivery",
                        "NotResource": deliveries(),
                    },
                    {
                        "Effect": "Allow",
                        "Principal": {"AWS": [OTHER_ACCOUNT_ROOT, "arn:aws:iam::123456789012:root"]},
                        "Action": ["logs:CreateDelivery", "logs:PutDeliverySource"],
                        "Resource": [deliveries(), "*"],
                    },
                ],
            })
            .to_string(),
        ),
        PolicyCase::of(
            "three statements, and actions written as the patterns IAM admits",
            &json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Principal": "*",
                        "Action": "logs:*",
                        "Resource": delivery_sources(),
                    },
                    {
                        "Effect": "Allow",
                        "Principal": "*",
                        "Action": "logs:Create*",
                        "Resource": deliveries(),
                    },
                    {
                        "Effect": "Deny",
                        "Principal": "*",
                        "Action": "*",
                        "Resource": delivery_destinations(),
                    },
                ],
            })
            .to_string(),
        ),
        PolicyCase::spaced(
            "a pretty-printed policy, whitespace and all",
            "{\n  \"Version\" : \"2012-10-17\",\n  \"Statement\" : [ {\n    \"Effect\" : \"Allow\",\n \
             \"Principal\" : \"*\",\n    \"Action\" : \"logs:CreateDelivery\",\n    \
             \"Resource\" : \"*\"\n  } ]\n}",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"logs:CreateDelivery","Resource":"*"}]}"#,
        ),
        PolicyCase::spaced(
            "a policy spaced after each separator, with whitespace inside a string",
            "{\"Version\": \"2012-10-17\", \"Statement\": [{\"Sid\": \"two words\", \
             \"Effect\": \"Allow\", \"Principal\": \"*\", \"Action\": \"logs:GetDelivery\", \
             \"Resource\": \"*\"}, {\"Sid\": \"and more\", \"Effect\": \"Allow\", \
             \"Principal\": \"*\", \"Action\": \"logs:CreateDelivery\", \"Resource\": \"*\"}]}",
            r#"{"Version":"2012-10-17","Statement":[{"Sid":"two words","Effect":"Allow","Principal":"*","Action":"logs:GetDelivery","Resource":"*"},{"Sid":"and more","Effect":"Allow","Principal":"*","Action":"logs:CreateDelivery","Resource":"*"}]}"#,
        ),
        PolicyCase::of(
            "a policy on the published maximum length",
            &policy_of_length(POLICY_MAX_LENGTH),
        ),
    ]
}

/// The `policy.deliveryDestinationPolicy` a response carries, or `None` when the reported
/// `policy` structure carries none — which is what a destination no policy has been assigned
/// to reports. The structure itself is always there, which is asserted here for every read.
fn reported_policy(operation: &str, body: &Value, response: &Value) -> Option<String> {
    let policy = response.get("policy").unwrap_or_else(|| {
        panic!("{operation} {body}: the response carries the policy structure, got {response}")
    });
    assert!(
        policy.is_object(),
        "{operation} {body}: policy is the modelled structure, got {response}"
    );
    Some(
        policy
            .get("deliveryDestinationPolicy")?
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "{operation} {body}: a reported policy carries deliveryDestinationPolicy, \
                     got {response}"
                )
            })
            .to_string(),
    )
}

/// The policy the service reports for `name`, through a wire `GetDeliveryDestinationPolicy`.
async fn get_policy(logs: &Logs, name: &str) -> Option<String> {
    let body = json!({"deliveryDestinationName": name});
    let response = logs.ok(GET_POLICY, &body).await;
    reported_policy(GET_POLICY, &body, &response)
}

/// Assign one policy, returning the contents the assigning call itself reports.
async fn put_policy(logs: &Logs, name: &str, document: &str) -> Option<String> {
    let body = json!({"deliveryDestinationName": name, "deliveryDestinationPolicy": document});
    let response = logs.ok(PUT_POLICY, &body).await;
    reported_policy(PUT_POLICY, &body, &response)
}

/// Every policy document, assigned to every destination, is reported by the call that assigns
/// it as the request wrote it, and read back by `GetDeliveryDestinationPolicy` as the DOCUMENT
/// it is — its whitespace between tokens gone and nothing else moved: the round trip through
/// stored state.
#[tokio::test]
async fn an_assigned_policy_is_reported_and_read_back() {
    let logs = Logs::new();
    let destinations = destinations();
    let policies = policies();

    for destination in &destinations {
        logs.ok(PUT_DESTINATION, &destination.put_body()).await;
    }

    for destination in &destinations {
        for policy in &policies {
            let case = (destination.label, policy.label);
            let assigned = put_policy(&logs, &destination.name, &policy.document).await;
            assert_eq!(
                assigned.as_deref(),
                Some(policy.document.as_str()),
                "case {case:?}: {PUT_POLICY} reports the policy it just assigned, got {assigned:?}"
            );

            let read_back = get_policy(&logs, &destination.name).await;
            assert_eq!(
                read_back.as_deref(),
                Some(policy.reported.as_str()),
                "case {case:?}: {GET_POLICY} reports the assigned policy, got {read_back:?}"
            );

            // Reading twice reports the same contents: a read changes nothing.
            let again = get_policy(&logs, &destination.name).await;
            assert_eq!(
                again, read_back,
                "case {case:?}: a second read reports the same policy, got {again:?}"
            );
        }
    }
}

/// The service READS the document rather than keeping the caller's bytes, so what every later
/// reader sees is its canonical form: a one-value list written as that value, and an account
/// principal named by its id alone written as that account's root ARN. The assigning call still
/// reports the document the request wrote.
#[tokio::test]
async fn a_stored_policy_is_reported_in_its_canonical_form() {
    let logs = Logs::new();
    let destination = Destination::of("an S3 bucket destination", "canonical", BUCKET_ARN);
    logs.ok(PUT_DESTINATION, &destination.put_body()).await;

    // (case, the document the request carries, the text every later reader reports)
    let cases: Vec<(&str, String, String)> = vec![
        (
            "an account principal named by its id alone",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"AWS": OTHER_ACCOUNT},
                    "Action": "logs:CreateDelivery",
                    "Resource": "*",
                }],
            })
            .to_string(),
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"AWS": OTHER_ACCOUNT_ROOT},
                    "Action": "logs:CreateDelivery",
                    "Resource": "*",
                }],
            })
            .to_string(),
        ),
        (
            "every value element written as a one-value list",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "oneEach",
                    "Effect": "Allow",
                    "Principal": {"AWS": [OTHER_ACCOUNT]},
                    "Action": ["logs:CreateDelivery"],
                    "Resource": [delivery_destinations()],
                }],
            })
            .to_string(),
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "oneEach",
                    "Effect": "Allow",
                    "Principal": {"AWS": OTHER_ACCOUNT_ROOT},
                    "Action": "logs:CreateDelivery",
                    "Resource": delivery_destinations(),
                }],
            })
            .to_string(),
        ),
        (
            "a one-statement list, which is not a value list",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"AWS": [OTHER_ACCOUNT_ROOT, OTHER_ACCOUNT]},
                    "Action": ["logs:CreateDelivery", "logs:GetDelivery"],
                    "Resource": "*",
                }],
            })
            .to_string(),
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"AWS": [OTHER_ACCOUNT_ROOT, OTHER_ACCOUNT_ROOT]},
                    "Action": ["logs:CreateDelivery", "logs:GetDelivery"],
                    "Resource": "*",
                }],
            })
            .to_string(),
        ),
    ];

    for (case, document, canonical) in cases {
        let assigned = put_policy(&logs, &destination.name, &document).await;
        assert_eq!(
            assigned.as_deref(),
            Some(document.as_str()),
            "case {case:?}: {PUT_POLICY} reports the document the request wrote, got {assigned:?}"
        );
        let read_back = get_policy(&logs, &destination.name).await;
        assert_eq!(
            read_back.as_deref(),
            Some(canonical.as_str()),
            "case {case:?}: expected the canonical form {canonical:?}, got {read_back:?}"
        );
        // The rendering is a fixed point: putting back what was read reports the same text.
        put_policy(&logs, &destination.name, &canonical).await;
        let again = get_policy(&logs, &destination.name).await;
        assert_eq!(
            again.as_deref(),
            Some(canonical.as_str()),
            "case {case:?}: the canonical form read back unchanged, got {again:?}"
        );
    }
}

/// A destination holds at most one policy, so each assignment REPLACES the one before it: the
/// contents assigned last are the contents every later reader sees, whichever order the
/// documents are assigned in.
#[tokio::test]
async fn the_policy_assigned_last_is_the_one_reported() {
    let logs = Logs::new();
    let destination = Destination::of("an S3 bucket destination", "overwritten", BUCKET_ARN);
    logs.ok(PUT_DESTINATION, &destination.put_body()).await;

    let mut documents: Vec<PolicyCase> = policies();
    // The same set assigned in the reverse order too, so what is reported follows the LAST
    // assignment rather than the first, the longest or the shortest.
    let mut reversed = policies();
    reversed.reverse();
    documents.extend(reversed);

    for policy in &documents {
        let assigned = put_policy(&logs, &destination.name, &policy.document).await;
        assert_eq!(
            assigned.as_deref(),
            Some(policy.document.as_str()),
            "case {:?}: the assigning call reports the new policy, got {assigned:?}",
            policy.label
        );
        let read_back = get_policy(&logs, &destination.name).await;
        assert_eq!(
            read_back.as_deref(),
            Some(policy.reported.as_str()),
            "case {:?}: the last policy assigned is the one reported, got {read_back:?}",
            policy.label
        );
    }
}

/// A destination that exists but has had no policy assigned reports NO policy — the response
/// succeeds and carries no `policy` member — and assigning one to a sibling destination
/// leaves it reporting none.
#[tokio::test]
async fn a_destination_with_no_policy_assigned_reports_none() {
    let logs = Logs::new();
    let destinations = destinations();
    for destination in &destinations {
        logs.ok(PUT_DESTINATION, &destination.put_body()).await;
    }

    for destination in &destinations {
        let reported = get_policy(&logs, &destination.name).await;
        assert_eq!(
            reported, None,
            "case {:?}: a destination with no policy assigned reports none, got {reported:?}",
            destination.label
        );
    }

    // One destination is given a policy; every other one still reports none.
    let assigned_to = &destinations[0];
    put_policy(&logs, &assigned_to.name, &two_statement_policy(&delivery_destinations())).await;
    for destination in destinations.iter().skip(1) {
        let reported = get_policy(&logs, &destination.name).await;
        assert_eq!(
            reported, None,
            "case {:?}: a policy on one destination is not reported for another, got {reported:?}",
            destination.label
        );
    }
}

/// A policy is assigned TO a delivery destination, so a `deliveryDestinationName` the account
/// does not hold is refused as a missing resource by BOTH operations — and the refusal stores
/// nothing, so the name still holds no policy once a destination is created under it.
#[tokio::test]
async fn a_name_the_account_does_not_hold_is_refused() {
    let logs = Logs::new();
    let held = Destination::of("an S3 bucket destination", "held", BUCKET_ARN);
    logs.ok(PUT_DESTINATION, &held.put_body()).await;

    // Names that satisfy `DeliveryDestinationName` and that the account does not hold: one
    // never used, one differing from a held name only in case, one at each published length
    // bound.
    let absent = [
        ("a name never used", "absent".to_string()),
        ("a held name in another case", "HELD".to_string()),
        ("a one-character name", "x".to_string()),
        ("a name at the maximum length", "z".repeat(60)),
    ];

    for (case, name) in &absent {
        let code = logs
            .err(
                PUT_POLICY,
                &json!({
                    "deliveryDestinationName": name,
                    "deliveryDestinationPolicy": two_statement_policy(&delivery_destinations()),
                }),
            )
            .await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case:?}: assigning a policy to a destination the account does not hold is \
             refused with {NOT_FOUND}, got {code}"
        );

        let code = logs
            .err(GET_POLICY, &json!({"deliveryDestinationName": name}))
            .await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case:?}: reading the policy of a destination the account does not hold is \
             refused with {NOT_FOUND}, got {code}"
        );
    }

    // The refused assignments wrote nothing: a destination created under one of those names
    // afterwards holds no policy.
    let created = Destination::of("a destination created later", "absent", BUCKET_ARN);
    logs.ok(PUT_DESTINATION, &created.put_body()).await;
    let reported = get_policy(&logs, &created.name).await;
    assert_eq!(
        reported, None,
        "case \"a refused assignment persists nothing\": expected no policy, got {reported:?}"
    );
}

/// "Creates and assigns an IAM policy that grants permissions to CloudWatch Logs to deliver
/// logs cross-account": the document is an IAM policy in its RESOURCE-based form — a JSON
/// object whose statements each state an effect, the actions they cover and the principal they
/// grant them to — and each of its statements is a grant of LOG DELIVERY: it names the
/// delivery resources it covers, allows the delivery actions on them, and grants them to an
/// account. A `deliveryDestinationPolicy` that is not such a document is refused as an invalid
/// parameter, and the refusal leaves the policy the destination already held in place.
#[tokio::test]
async fn a_policy_that_is_not_an_iam_policy_document_is_refused() {
    let logs = Logs::new();
    let destination = Destination::of("an S3 bucket destination", "guarded", BUCKET_ARN);
    logs.ok(PUT_DESTINATION, &destination.put_body()).await;
    let kept = two_statement_policy(&delivery_destinations());
    put_policy(&logs, &destination.name, &kept).await;

    // The rows span the clauses the document is held to: text that is not JSON at all, JSON
    // that is not an object, an object declaring no statement, a statement that is not an
    // object, a statement missing each of the elements a resource-based statement states, and a
    // well-formed policy whose statements are not grants of log delivery.
    let rejected = [
        ("plain text", "not a policy at all".to_string()),
        ("a one-character document", "p".to_string()),
        ("a JSON array of statements", r#"[{"Effect":"Allow"}]"#.to_string()),
        ("a JSON string", r#""{\"Effect\":\"Allow\"}""#.to_string()),
        ("a JSON number", "17".to_string()),
        ("a JSON null", "null".to_string()),
        ("a truncated object", r#"{"Statement":"#.to_string()),
        ("an object with a trailing comma", r#"{"Sid":"a",}"#.to_string()),
        ("the smallest JSON object, which declares no statement", "{}".to_string()),
        (
            "an empty statement list",
            r#"{"Version":"2012-10-17","Statement":[]}"#.to_string(),
        ),
        (
            "a Statement that is a string",
            r#"{"Statement":"let me deliver"}"#.to_string(),
        ),
        (
            "a statement with no Effect",
            r#"{"Statement":[{"Principal":"*","Action":"logs:PutLogEvents"}]}"#.to_string(),
        ),
        (
            "a statement with no Action",
            r#"{"Statement":[{"Effect":"Allow","Principal":"*"}]}"#.to_string(),
        ),
        (
            "a statement with no Principal, which a resource policy states",
            r#"{"Statement":[{"Effect":"Allow","Action":"logs:PutLogEvents"}]}"#.to_string(),
        ),
        (
            "a policy version no policy language has",
            r#"{"Version":"2024-01-01","Statement":[{"Effect":"Allow","Principal":"*","Action":"logs:PutLogEvents"}]}"#
                .to_string(),
        ),
        // A well-formed resource policy that is not a grant of LOG DELIVERY, so it is not a
        // delivery destination policy: one granting access to a resource other than the
        // vended-log delivery resources (the log group one destination represents, the bucket
        // another represents, a value that names no resource at all), one naming no resource,
        // one granting an action that cannot be taken on a delivery resource, and one granting
        // to a service rather than to an account.
        (
            "a policy granting access to a log group",
            policy_for(LOG_GROUP_ARN),
        ),
        (
            "a policy granting access to an S3 bucket",
            policy_for(BUCKET_ARN),
        ),
        (
            "a policy naming a resource that is not an ARN",
            policy_for("vended-logs-bucket"),
        ),
        (
            "a statement naming no resource at all",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"AWS": OTHER_ACCOUNT_ROOT},
                    "Action": "logs:CreateDelivery",
                }],
            })
            .to_string(),
        ),
        (
            "a policy granting an action no delivery resource takes",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"AWS": OTHER_ACCOUNT_ROOT},
                    "Action": ["logs:CreateDelivery", "logs:PutLogEvents"],
                    "Resource": "*",
                }],
            })
            .to_string(),
        ),
        (
            "a policy granting another service's action",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"AWS": OTHER_ACCOUNT_ROOT},
                    "Action": "firehose:PutRecord",
                    "Resource": "*",
                }],
            })
            .to_string(),
        ),
        (
            "a policy granting to a service principal rather than to an account",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"Service": "delivery.logs.amazonaws.com"},
                    "Action": "logs:CreateDelivery",
                    "Resource": delivery_destinations(),
                }],
            })
            .to_string(),
        ),
        (
            "a policy whose second statement grants another service's resource",
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Principal": "*",
                        "Action": "logs:CreateDelivery",
                        "Resource": delivery_destinations(),
                    },
                    {
                        "Effect": "Allow",
                        "Principal": "*",
                        "Action": "firehose:PutRecord",
                        "Resource": FIREHOSE_ARN,
                    },
                ],
            })
            .to_string(),
        ),
    ];

    for (case, document) in &rejected {
        let (code, said) = logs
            .refusal(
                PUT_POLICY,
                &json!({
                    "deliveryDestinationName": destination.name,
                    "deliveryDestinationPolicy": document,
                }),
            )
            .await;
        assert_eq!(
            code, VALIDATION,
            "case {case:?}: a policy that is not a delivery destination policy is refused with \
             {VALIDATION}, got {code} {said:?}"
        );
        assert_eq!(
            said, INVALID_POLICY,
            "case {case:?}: expected {INVALID_POLICY:?}, got {said:?}"
        );

        let reported = get_policy(&logs, &destination.name).await;
        assert_eq!(
            reported.as_deref(),
            Some(kept.as_str()),
            "case {case:?}: a refused assignment leaves the assigned policy in place, \
             got {reported:?}"
        );
    }

    // Both published length bounds of `DeliveryDestinationPolicy` (`@length` 1..51200), whose
    // in-bound sibling is a passing row above: a value outside a modelled constraint is refused
    // for the constraint it breaks, so only the CODE is the same as the rows above.
    for (case, document) in [
        (
            "one character past the maximum length",
            policy_of_length(POLICY_MAX_LENGTH + 1),
        ),
        ("an empty document", String::new()),
    ] {
        let code = logs
            .err(
                PUT_POLICY,
                &json!({
                    "deliveryDestinationName": destination.name,
                    "deliveryDestinationPolicy": document,
                }),
            )
            .await;
        assert_eq!(
            code, VALIDATION,
            "case {case:?}: a document outside the published bounds is refused with \
             {VALIDATION}, got {code}"
        );
        let reported = get_policy(&logs, &destination.name).await;
        assert_eq!(
            reported.as_deref(),
            Some(kept.as_str()),
            "case {case:?}: a refused assignment leaves the assigned policy in place, \
             got {reported:?}"
        );
    }
}

/// The policy is not one of the "delivery destination parameters" a later
/// `PutDeliveryDestination` overwrites, and assigning a policy is not a change to those
/// parameters either: each operation leaves the other's state alone.
#[tokio::test]
async fn a_policy_and_the_destination_parameters_do_not_overwrite_each_other() {
    let logs = Logs::new();
    let name = "paired";
    logs.ok(
        PUT_DESTINATION,
        &json!({
            "name": name,
            "deliveryDestinationConfiguration": {"destinationResourceArn": BUCKET_ARN},
            "outputFormat": "json",
        }),
    )
    .await;

    let document = two_statement_policy(&delivery_destinations());
    put_policy(&logs, name, &document).await;

    // Assigning the policy left every destination parameter as the put set it.
    let destination = logs
        .ok(GET_DESTINATION, &json!({"name": name}))
        .await
        .get("deliveryDestination")
        .cloned()
        .expect("case \"after assigning a policy\": the destination is reported");
    assert_eq!(
        destination.get("outputFormat").and_then(Value::as_str),
        Some("json"),
        "case \"after assigning a policy\": expected outputFormat json, got {destination}"
    );
    assert_eq!(
        destination.get("deliveryDestinationType").and_then(Value::as_str),
        Some("S3"),
        "case \"after assigning a policy\": expected the S3 destination type, got {destination}"
    );

    // Updating the destination's parameters — a different resource, a different output
    // format — leaves the assigned policy where it is.
    logs.ok(
        PUT_DESTINATION,
        &json!({
            "name": name,
            "deliveryDestinationConfiguration": {"destinationResourceArn": FIREHOSE_ARN},
            "outputFormat": "raw",
        }),
    )
    .await;
    let reported = get_policy(&logs, name).await;
    assert_eq!(
        reported.as_deref(),
        Some(document.as_str()),
        "case \"after updating the destination parameters\": the assigned policy stays, \
         got {reported:?}"
    );

    let updated = logs
        .ok(GET_DESTINATION, &json!({"name": name}))
        .await
        .get("deliveryDestination")
        .cloned()
        .expect("case \"after updating the destination\": the destination is reported");
    assert_eq!(
        updated.get("deliveryDestinationType").and_then(Value::as_str),
        Some("FH"),
        "case \"after updating the destination\": expected the Firehose destination type, \
         got {updated}"
    );
}
