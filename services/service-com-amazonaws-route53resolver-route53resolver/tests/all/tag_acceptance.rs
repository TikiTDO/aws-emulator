// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the tag acceptance criteria every write of this
//! service's `Tags` enforces: `TagResource` and the creates that carry a `Tags`
//! list.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].
//!
//! What these cases pin is the rule the service follows: a tag it stores names
//! a value, so a write carrying a tag whose `Value` is the empty string the
//! model's `TagValue` (length 0..256) would admit is refused with HTTP 400 and
//! the service error code for an unacceptable tag parameter, before anything is
//! written — and every value the criteria do admit round-trips byte for byte.

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::state::RSLVR_CODE_TAG_NOT_ACCEPTABLE;

use super::harness::{
    call, emulator_with_outposts, error_type, ok, rack_outpost, string_member, Emulator, REGION,
};

/// The whole sentence pair Route 53 Resolver answers for a tag parameter it
/// will not accept. FIXED text for this fault — it interpolates nothing from
/// the request — behind the bracketed service error code the service puts in
/// front of its messages.
fn unacceptable_tag_message() -> String {
    format!(
        "[{RSLVR_CODE_TAG_NOT_ACCEPTABLE}] One of the input parameters of the tags does not meet \
         the acceptance criteria specified in the restrictions. Please fix the invalid parameter \
         before retrying."
    )
}

/// One row of the value table: the tags a single call carries, and whether the
/// service's acceptance criteria admit them.
struct TagCase {
    case: &'static str,
    tags: Vec<(String, String)>,
    accepted: bool,
}

/// One write that carries a `Tags` list: the operation, the modelled error it
/// reports a parameter it will not accept with, the member it answers the
/// resource in, and the request body WITHOUT its `Tags`.
struct WriteCase {
    case: &'static str,
    operation: &'static str,
    expected_error: &'static str,
    member: &'static str,
    body: Value,
}

/// One key/value pair, as the rows below name them.
fn pair(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// A `Tags` list, as the wire carries one.
fn tag_list(tags: &[(String, String)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

/// Create one DNS Firewall domain list and answer the `Arn` it reports: the
/// resource the tag operation below addresses.
async fn create_domain_list(router: &Emulator, creator: &str) -> String {
    let created = ok(
        router,
        "CreateFirewallDomainList",
        json!({ "CreatorRequestId": creator, "Name": creator }),
    )
    .await;
    string_member(
        created
            .get("FirewallDomainList")
            .expect("the create answers a FirewallDomainList"),
        "Arn",
    )
}

/// Every tag of one resource, as `ListTagsForResource` reports them.
async fn all_tags(router: &Emulator, resource_arn: &str) -> Vec<(String, String)> {
    let mut tags: Vec<(String, String)> = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut body = json!({ "ResourceArn": resource_arn });
        if let Some(token) = &next_token {
            body["NextToken"] = json!(token);
        }
        let page = ok(router, "ListTagsForResource", body).await;
        for tag in page
            .get("Tags")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            tags.push((string_member(&tag, "Key"), string_member(&tag, "Value")));
        }
        next_token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            return tags;
        }
    }
}

/// Assert one call was refused as an unacceptable tag: HTTP 400, the modelled
/// error this operation reports a parameter fault with, and the whole fixed
/// sentence pair behind the service error code.
async fn expect_unacceptable(
    router: &Emulator,
    operation: &str,
    body: Value,
    case: &str,
    expected_error: &str,
) {
    let (status, answered) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: {operation} must refuse an unacceptable tag with HTTP 400, got {status}: {answered}"
    );
    assert!(
        error_type(&answered).contains(expected_error),
        "case {case:?}: {operation} must answer {expected_error}, got {answered}"
    );
    assert_eq!(
        string_member(&answered, "Message"),
        unacceptable_tag_message(),
        "case {case:?}: {operation}'s refusal must state the acceptance-criteria message, got {answered}"
    );
}

/// `TagResource` refuses a `Tags` list carrying a value the acceptance criteria
/// do not admit, and persists nothing — while every value they do admit is
/// stored and read back byte for byte.
///
/// The rows span what the rule generalises over, the form of the value: the
/// empty string, a one-character value, a value of only spaces, non-ASCII text,
/// the 256-character maximum `TagValue` declares, and a 128-character key (the
/// maximum `TagKey` declares) beside it — plus the shapes of a request that
/// carries one unacceptable tag among acceptable ones.
#[tokio::test]
async fn a_tag_value_outside_the_acceptance_criteria_is_refused_and_persists_nothing() {
    let router = emulator_with_outposts();
    let arn = create_domain_list(&router, "tag-acceptance").await;

    // Tags written by an earlier successful call, which no refusal below may
    // disturb.
    let held = vec![pair("team", "net")];
    ok(
        &router,
        "TagResource",
        json!({ "ResourceArn": arn, "Tags": tag_list(&held) }),
    )
    .await;

    // The published maxima of `TagKey` (length 1..128) and `TagValue`
    // (length 0..256).
    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);

    let rows = vec![
        TagCase {
            case: "an empty value",
            tags: vec![pair("empty-value-key", "")],
            accepted: false,
        },
        TagCase {
            case: "an empty value beside an acceptable tag",
            tags: vec![pair("fine", "yes"), pair("also-empty", "")],
            accepted: false,
        },
        TagCase {
            case: "two tags, both with an empty value",
            tags: vec![pair("first-empty", ""), pair("second-empty", "")],
            accepted: false,
        },
        TagCase {
            case: "a one-character value",
            tags: vec![pair("short", "x")],
            accepted: true,
        },
        TagCase {
            case: "a value of only spaces",
            tags: vec![pair("spaced", "   ")],
            accepted: true,
        },
        TagCase {
            case: "a value of non-ASCII text",
            tags: vec![pair("环境-key", "生产-value")],
            accepted: true,
        },
        TagCase {
            case: "the longest key and value the model admits",
            tags: vec![pair(&longest_key, &longest_value)],
            accepted: true,
        },
    ];

    let mut expected = held.clone();
    for row in &rows {
        let body = json!({ "ResourceArn": arn, "Tags": tag_list(&row.tags) });
        if row.accepted {
            ok(&router, "TagResource", body).await;
            expected.extend(row.tags.iter().cloned());
        } else {
            expect_unacceptable(
                &router,
                "TagResource",
                body,
                row.case,
                "InvalidParameterException",
            )
            .await;
        }
        assert_eq!(
            all_tags(&router, &arn).await,
            expected,
            "case {:?}: the stored tags must be exactly the ones acceptable writes left",
            row.case
        );
    }

    // A refused write persists NOTHING, not even the acceptable tag it carried
    // beside the unacceptable one.
    for row in rows.iter().filter(|row| !row.accepted) {
        for (key, _) in &row.tags {
            assert!(
                !expected.iter().any(|(stored, _)| stored == key),
                "case {:?}: {key:?} must not have been stored",
                row.case
            );
        }
    }
}

/// The writes that carry a `Tags` list, with the body each needs minus its
/// tags: every create this service can serve that models a `Tags` member, plus
/// the association that models one.
async fn tag_carrying_writes(router: &Emulator) -> Vec<WriteCase> {
    // An endpoint's addresses are placed in subnets of the caller's VPC, and an
    // association is made WITH a VPC of the account, so both are registered in
    // the account's own Amazon EC2 store first.
    router.hold_subnets(["sn-tagged-a", "sn-tagged-b"]);
    router.hold_vpcs(["vpc-00000000000000077"]);
    let rule_group = ok(
        router,
        "CreateFirewallRuleGroup",
        json!({ "CreatorRequestId": "group-for-tagged-assoc", "Name": "group-for-tagged-assoc" }),
    )
    .await;
    let rule_group_id = string_member(
        rule_group
            .get("FirewallRuleGroup")
            .expect("the create answers a FirewallRuleGroup"),
        "Id",
    );

    vec![
        WriteCase {
            case: "a DNS Firewall domain list",
            operation: "CreateFirewallDomainList",
            expected_error: "ValidationException",
            member: "FirewallDomainList",
            body: json!({ "CreatorRequestId": "create-tags-domain-list", "Name": "tagged-list" }),
        },
        WriteCase {
            case: "a DNS Firewall rule group",
            operation: "CreateFirewallRuleGroup",
            expected_error: "ValidationException",
            member: "FirewallRuleGroup",
            body: json!({ "CreatorRequestId": "create-tags-rule-group", "Name": "tagged-group" }),
        },
        WriteCase {
            case: "a Resolver rule",
            operation: "CreateResolverRule",
            expected_error: "InvalidParameterException",
            member: "ResolverRule",
            body: json!({
                "CreatorRequestId": "create-tags-rule",
                "Name": "tagged-rule",
                "RuleType": "SYSTEM",
                "DomainName": "tagged.example.com",
            }),
        },
        WriteCase {
            case: "a Resolver query log configuration",
            operation: "CreateResolverQueryLogConfig",
            expected_error: "InvalidParameterException",
            member: "ResolverQueryLogConfig",
            body: json!({
                "CreatorRequestId": "create-tags-query-log",
                "Name": "tagged-config",
                "DestinationArn": "arn:aws:s3:::amzn-s3-demo-bucket",
            }),
        },
        WriteCase {
            case: "a Resolver endpoint",
            operation: "CreateResolverEndpoint",
            expected_error: "InvalidParameterException",
            member: "ResolverEndpoint",
            body: json!({
                "CreatorRequestId": "create-tags-endpoint",
                "Direction": "OUTBOUND",
                "SecurityGroupIds": ["sg-00000000000000001"],
                "IpAddresses": [
                    { "SubnetId": "sn-tagged-a", "Ip": "10.0.0.10" },
                    { "SubnetId": "sn-tagged-b", "Ip": "10.0.1.10" },
                ],
            }),
        },
        WriteCase {
            case: "a Resolver on an Outpost",
            operation: "CreateOutpostResolver",
            expected_error: "ValidationException",
            member: "OutpostResolver",
            body: json!({
                "CreatorRequestId": "create-tags-outpost-resolver",
                "Name": "tagged-outpost-resolver",
                "PreferredInstanceType": "m5.large",
                "OutpostArn": rack_outpost(router, REGION, "outpost-for-tagged-resolver").await,
            }),
        },
        WriteCase {
            case: "a DNS Firewall rule group association",
            operation: "AssociateFirewallRuleGroup",
            expected_error: "ValidationException",
            member: "FirewallRuleGroupAssociation",
            body: json!({
                "CreatorRequestId": "create-tags-association",
                "FirewallRuleGroupId": rule_group_id,
                "VpcId": "vpc-00000000000000077",
                "Priority": 101,
                "Name": "tagged-association",
                "MutationProtection": "DISABLED",
            }),
        },
    ]
}

/// Every write that carries a `Tags` list enforces the same criteria, with the
/// error ITS model gives it for a parameter it will not accept, and creates
/// nothing: the resource the refused request would have made is absent, so the
/// same `CreatorRequestId` is free to create it with acceptable tags afterwards.
///
/// The rows span every operation of this service whose model declares a `Tags`
/// member and that this emulator can serve, so the rule is not pinned to the
/// tag operation alone.
#[tokio::test]
async fn a_write_carrying_an_unacceptable_tag_is_refused_and_creates_nothing() {
    let router = emulator_with_outposts();
    // The query-logging create resolves its `DestinationArn` against the store
    // of the service that owns the destination, so the account holds it first.
    router.hold_query_log_destinations(["arn:aws:s3:::amzn-s3-demo-bucket"]);

    for write in tag_carrying_writes(&router).await {
        let mut refused = write.body.clone();
        refused["Tags"] = tag_list(&[pair("owner", "netops"), pair("empty-value-key", "")]);
        expect_unacceptable(
            &router,
            write.operation,
            refused,
            write.case,
            write.expected_error,
        )
        .await;

        // The refused write wrote nothing at all, so the same request id still
        // creates the resource once its tags are acceptable, and the resource
        // carries exactly those tags.
        let acceptable = vec![pair("owner", "netops"), pair("note", "-")];
        let mut served = write.body;
        served["Tags"] = tag_list(&acceptable);
        let created = ok(&router, write.operation, served).await;
        let resource = created.get(write.member).unwrap_or_else(|| {
            panic!(
                "case {:?}: {} must answer a {}, got {created}",
                write.case, write.operation, write.member
            )
        });
        let arn = string_member(resource, "Arn");
        assert_eq!(
            all_tags(&router, &arn).await,
            acceptable,
            "case {:?}: the created resource must carry the tags the served write named",
            write.case
        );
    }
}
