// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for two rules of the Resolver-rule and query-logging
//! planes.
//!
//! 1. A write's `CreatorRequestId` identifies ONE request: replaying it with the
//!    same request resolves to the resource that request already made, while
//!    replaying it with a different request is refused and the resource the
//!    first one made is left exactly as it was. A member a rule SELECTS DNS
//!    queries on (`DomainName`, `DelegationRecord`) is only accepted in the
//!    ASCII spelling the DNS protocol carries, so an internationalized label
//!    reaches the service only as IDNA/punycode.
//!
//! 2. A Route 53 Resolver refusal states its fault as the service's own
//!    sentence: the service error code in square brackets and then that fault's
//!    fixed text, with ONLY the parts the sentence calls for taken from the
//!    request — an unknown query logging configuration and a missing
//!    association name nothing at all, while an unissued pagination cursor and
//!    an unknown filter key quote the value that was sent. Which value a
//!    sentence carries is a property of the fault and not of the operation, so
//!    these cases are asserted over every operation of this service that
//!    reports each fault.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`]; the rule, endpoint and association fixtures are the
//! ones in [`super::resolver_rule_deletion`] and the query-logging ones those
//! in [`super::resolver_query_log_config_associations`].

use serde_json::{json, Value};

use super::harness::{call, emulator, error_type, ok, string_member, traced_message, Emulator};
use super::resolver_query_log_config_associations::{association, config_id};
use super::resolver_rule_deletion::{associate, forward_rule, outbound_endpoint};

/// The code a rule create reports for a `CreatorRequestId` an earlier, different
/// create already spent, from the recorded `ResourceExistsException` body of
/// `CreateResolverRule`.
const CODE_CREATOR_REQUEST_ID_EXISTS: &str = "RSLVR-00701";

/// The code a rule create reports for a domain name or a delegation record it
/// will not read as a DNS name, from the recorded `InvalidParameterException`
/// body of `CreateResolverRule`.
const CODE_INVALID_DOMAIN: &str = "RSLVR-00711";

/// The sentence a query logging configuration the service holds nothing under is
/// refused with: fixed text behind its own service error code, before the trace
/// clause the refusal closes with.
const CONFIG_GONE_SENTENCE: &str =
    "[RSLVR-01601] The specified query logging configuration doesn't exist.";

/// The sentence an association the service holds nothing under is refused with,
/// whichever way the request addressed it.
const ASSOCIATION_GONE_SENTENCE: &str =
    "[RSLVR-01602] The specified query logging configuration association doesn't exist.";

/// The code a listing reports for a `NextToken` it never issued, from the
/// recorded `InvalidNextTokenException` bodies of the paginated listings.
const CODE_INVALID_NEXT_TOKEN: &str = "RSLVR-00302";

/// The code a listing reports for a `Filters[].Name` outside its catalog, from
/// the recorded `InvalidParameterException` bodies of the listings that accept
/// `Filters`.
const CODE_INVALID_FILTER_KEY: &str = "RSLVR-01004";

/// Assert one refusal: HTTP 400, the modelled shape and the exact message.
fn assert_refusal(case: &str, operation: &str, status: u16, parsed: &Value, shape: &str, expected: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}/{operation}: expected HTTP 400, got {status}: {parsed}"
    );
    assert!(
        error_type(parsed).contains(shape),
        "case {case:?}/{operation}: expected {shape}, got {parsed}"
    );
    let message = string_member(parsed, "Message");
    assert_eq!(
        message, expected,
        "case {case:?}/{operation}: expected Message {expected:?}, got {message:?}"
    );
}

/// Assert one refusal whose `Message` closes with the trace clause of the
/// request it refused: HTTP 400, the modelled shape, the exact sentence before
/// the clause, and a clause carrying this call's own trace id.
fn assert_traced_refusal(
    case: &str,
    operation: &str,
    status: u16,
    parsed: &Value,
    shape: &str,
    sentence: &str,
) {
    assert_eq!(
        status, 400,
        "case {case:?}/{operation}: expected HTTP 400, got {status}: {parsed}"
    );
    assert!(
        error_type(parsed).contains(shape),
        "case {case:?}/{operation}: expected {shape}, got {parsed}"
    );
    let message = string_member(parsed, "Message");
    let (stated, _) = traced_message(&message).unwrap_or_else(|| {
        panic!(
            "case {case:?}/{operation}: the refusal must close with its trace clause, got \
             {message:?}"
        )
    });
    assert_eq!(
        stated, sentence,
        "case {case:?}/{operation}: expected the sentence {sentence:?}, got {message:?}"
    );
}

/// The `ResolverRule` of a create or get response.
fn rule(body: &Value) -> &Value {
    body.get("ResolverRule")
        .unwrap_or_else(|| panic!("the response carries a ResolverRule, got {body}"))
}

/// Every rule `ListResolverRules` reports.
async fn listed_rules(router: &Emulator) -> Vec<Value> {
    let listed = ok(router, "ListResolverRules", json!({})).await;
    listed
        .get("ResolverRules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The request that creates the rule a replay is compared against, under
/// `token`.
fn base_rule_request(token: &str, endpoint: &str) -> Value {
    json!({
        "CreatorRequestId": token,
        "Name": "first-rule",
        "RuleType": "FORWARD",
        "DomainName": "replay.example.com",
        "ResolverEndpointId": endpoint,
        "TargetIps": [{ "Ip": "192.0.2.10" }],
        "Tags": [{ "Key": "team", "Value": "net" }],
    })
}

/// A `CreatorRequestId` stands for one request: a replay carrying a DIFFERENT
/// request is refused with the coded sentence that names the token, the rule the
/// first request made is untouched, and no second rule is minted — while a
/// replay of the SAME request resolves to that rule, which is what lets a
/// create be retried after a timeout.
///
/// The table spans what the rule generalises over: WHICH member the replay
/// differs in (the friendly name, the selected domain, one target address, the
/// number of target addresses, the rule's type, the endpoint it forwards
/// through, a tag's value, and a member dropped altogether), because the token
/// identifies the whole request and not one member of it.
#[tokio::test]
async fn a_replayed_creator_request_id_is_refused_when_the_request_differs() {
    let router = emulator();
    let endpoint = outbound_endpoint(&router, "endpoint-replay").await;
    let other_endpoint = {
        router.hold_subnets(["subnet-replay-other-a", "subnet-replay-other-b"]);
        let created = ok(
            &router,
            "CreateResolverEndpoint",
            json!({
                "CreatorRequestId": "endpoint-replay-other",
                "Direction": "OUTBOUND",
                "SecurityGroupIds": ["sg-00000000000000001"],
                "IpAddresses": [
                    { "SubnetId": "subnet-replay-other-a", "Ip": "10.0.0.20" },
                    { "SubnetId": "subnet-replay-other-b", "Ip": "10.0.1.20" },
                ],
            }),
        )
        .await;
        string_member(
            created
                .get("ResolverEndpoint")
                .expect("the create answers a ResolverEndpoint"),
            "Id",
        )
    };

    let token = "2026-10-29T19:32:14Z";
    let first = ok(&router, "CreateResolverRule", base_rule_request(token, &endpoint)).await;
    let first = rule(&first).clone();
    let first_id = string_member(&first, "Id");

    // A replay of the SAME request creates nothing and answers the stored rule.
    let retried = ok(&router, "CreateResolverRule", base_rule_request(token, &endpoint)).await;
    assert_eq!(
        rule(&retried),
        &first,
        "case \"an identical replay\": the retry must answer the rule the first call made, got \
         {retried}"
    );

    let mut differing: Vec<(&str, Value)> = Vec::new();
    for (case, member, value) in [
        ("a different Name", "Name", json!("second-rule")),
        ("a different DomainName", "DomainName", json!("other.example.com")),
        (
            "a different target address",
            "TargetIps",
            json!([{ "Ip": "192.0.2.11" }]),
        ),
        (
            "one more target address",
            "TargetIps",
            json!([{ "Ip": "192.0.2.10" }, { "Ip": "192.0.2.12" }]),
        ),
        (
            "a different ResolverEndpointId",
            "ResolverEndpointId",
            json!(other_endpoint),
        ),
        (
            "a different tag value",
            "Tags",
            json!([{ "Key": "team", "Value": "dns" }]),
        ),
        ("no Tags at all", "Tags", Value::Null),
        ("no Name at all", "Name", Value::Null),
    ] {
        let mut body = base_rule_request(token, &endpoint);
        let object = body.as_object_mut().expect("the request is an object");
        if value.is_null() {
            object.remove(member);
        } else {
            object.insert(member.to_string(), value);
        }
        differing.push((case, body));
    }
    // A rule of another TYPE is a different request too, and SYSTEM admits
    // neither a forwarding endpoint nor target addresses, so the row drops both.
    differing.push((
        "a different RuleType",
        json!({
            "CreatorRequestId": token,
            "Name": "first-rule",
            "RuleType": "SYSTEM",
            "DomainName": "replay.example.com",
            "Tags": [{ "Key": "team", "Value": "net" }],
        }),
    ));

    let expected = format!(
        r#"[{CODE_CREATOR_REQUEST_ID_EXISTS}] Resolver rule with creator request ID "{token}" already exists."#
    );
    for (case, body) in &differing {
        let (status, parsed) = call(&router, "CreateResolverRule", body.clone()).await;
        assert_refusal(
            case,
            "CreateResolverRule",
            status,
            &parsed,
            "ResourceExistsException",
            &expected,
        );
        assert_eq!(
            string_member(&parsed, "ResourceType"),
            "ResolverRule",
            "case {case:?}: the refusal must name the resource that already exists, got {parsed}"
        );

        // The rule the token already made is left exactly as it was, and the
        // refused create minted nothing.
        let fetched = ok(
            &router,
            "GetResolverRule",
            json!({ "ResolverRuleId": first_id.clone() }),
        )
        .await;
        assert_eq!(
            rule(&fetched),
            &first,
            "case {case:?}: the stored rule must be untouched by the refused replay, got {fetched}"
        );
        let held = listed_rules(&router).await;
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: the refused replay must leave exactly the one rule, got {held:?}"
        );
    }

    // The sentence's one variable part is the token the request carried, over
    // the forms `CreatorRequestId` (length 1..255) takes.
    for (case, token) in [
        ("the shortest CreatorRequestId (length 1)", "t".to_string()),
        ("the longest CreatorRequestId (length 255)", "t".repeat(255)),
        (
            "a token carrying the quote character the sentence uses",
            "token-\"quoted\"".to_string(),
        ),
    ] {
        ok(
            &router,
            "CreateResolverRule",
            base_rule_request(&token, &endpoint),
        )
        .await;
        let mut replay = base_rule_request(&token, &endpoint);
        replay["Name"] = json!("another-rule");
        let (status, parsed) = call(&router, "CreateResolverRule", replay).await;
        let expected = format!(
            r#"[{CODE_CREATOR_REQUEST_ID_EXISTS}] Resolver rule with creator request ID "{token}" already exists."#
        );
        assert_refusal(
            case,
            "CreateResolverRule",
            status,
            &parsed,
            "ResourceExistsException",
            &expected,
        );
    }
}

/// A member a rule selects DNS queries on travels as ASCII: the IDNA/punycode
/// spelling of an internationalized label is accepted and round-trips
/// byte-for-byte, while the raw-unicode spelling of the same label is refused
/// with the coded sentence — entirely fixed text — and leaves no rule behind.
///
/// The table spans what the rule generalises over: which member carries the
/// name (`DomainName` on a SYSTEM rule, `DelegationRecord` on a DELEGATE one),
/// where in the name the non-ASCII character sits (the first label, a middle
/// label, the last), and which script it comes from (Latin with a diacritic,
/// Greek, Han, and an emoji) — plus the ASCII spellings that must still be
/// served, including a name whose labels carry the hyphen punycode's prefix is
/// written with.
#[tokio::test]
async fn a_non_ascii_domain_or_delegation_record_is_refused_and_nothing_is_created() {
    let router = emulator();

    // The ACE spellings a client is expected to send, each round-tripped
    // through the read.
    let accepted: Vec<(&str, Value, &str)> = vec![
        (
            "the punycode spelling of an internationalized label",
            json!({
                "CreatorRequestId": "req-ace-first",
                "RuleType": "SYSTEM",
                "DomainName": "xn--mnchen-3ya.example.",
            }),
            "xn--mnchen-3ya.example.",
        ),
        (
            "punycode in a middle label",
            json!({
                "CreatorRequestId": "req-ace-middle",
                "RuleType": "SYSTEM",
                "DomainName": "a.xn--mnchen-3ya.example.",
            }),
            "a.xn--mnchen-3ya.example.",
        ),
        (
            "an ASCII label carrying hyphens",
            json!({
                "CreatorRequestId": "req-ascii-hyphens",
                "RuleType": "SYSTEM",
                "DomainName": "my--zone.example.",
            }),
            "my--zone.example.",
        ),
    ];
    for (case, request, expected_domain) in &accepted {
        let created = ok(&router, "CreateResolverRule", request.clone()).await;
        let made = rule(&created).clone();
        let id = string_member(&made, "Id");
        let fetched = ok(&router, "GetResolverRule", json!({ "ResolverRuleId": id })).await;
        let read = string_member(rule(&fetched), "DomainName");
        assert_eq!(
            &read, expected_domain,
            "case {case:?}: the stored rule must report {expected_domain:?}, got {read:?}"
        );
    }
    let served = listed_rules(&router).await.len();
    assert_eq!(
        served,
        accepted.len(),
        "the ASCII spellings must all be served, got {served} rules"
    );

    // The raw-unicode spellings, each refused with the one fixed sentence.
    let expected = format!("[{CODE_INVALID_DOMAIN}] Invalid domain or delegation record");
    let refused: Vec<(&str, Value)> = vec![
        (
            "a Latin label with a diacritic, first label",
            json!({
                "CreatorRequestId": "req-raw-latin",
                "RuleType": "SYSTEM",
                "DomainName": "münchen.example.",
            }),
        ),
        (
            "a Greek label, middle label",
            json!({
                "CreatorRequestId": "req-raw-greek",
                "RuleType": "SYSTEM",
                "DomainName": "a.παράδειγμα.example.",
            }),
        ),
        (
            "a Han label, last label",
            json!({
                "CreatorRequestId": "req-raw-han",
                "RuleType": "SYSTEM",
                "DomainName": "a.example.例",
            }),
        ),
        (
            "an emoji label",
            json!({
                "CreatorRequestId": "req-raw-emoji",
                "RuleType": "SYSTEM",
                "DomainName": "🌐.example.",
            }),
        ),
        (
            "a raw-unicode DelegationRecord on a DELEGATE rule",
            json!({
                "CreatorRequestId": "req-raw-delegation",
                "RuleType": "DELEGATE",
                "DomainName": "delegated.example.",
                "DelegationRecord": "ns.münchen.example.",
            }),
        ),
        (
            "a FORWARD rule whose DomainName is raw unicode",
            json!({
                "CreatorRequestId": "req-raw-forward",
                "RuleType": "FORWARD",
                "DomainName": "münchen.example.",
                "ResolverEndpointId": "rslvr-out-0000000000000000",
                "TargetIps": [{ "Ip": "192.0.2.30" }],
            }),
        ),
    ];
    for (case, request) in &refused {
        let (status, parsed) = call(&router, "CreateResolverRule", request.clone()).await;
        assert_refusal(
            case,
            "CreateResolverRule",
            status,
            &parsed,
            "InvalidParameterException",
            &expected,
        );
        let held = listed_rules(&router).await.len();
        assert_eq!(
            held, served,
            "case {case:?}: a refused create must leave the store as it was, got {held} rules"
        );
    }
}

/// Every operation that resolves a `ResolverQueryLogConfigId` answers an id it
/// holds no configuration under with the one coded sentence, which names
/// NOTHING from the request, and a refused associate leaves no association
/// behind.
///
/// The table spans the forms a `ResourceId` (length 1..64) takes: the form this
/// service mints, an association's form, no Resolver form at all, and both
/// published bounds of the length.
#[tokio::test]
async fn an_unknown_query_log_config_is_refused_with_the_coded_sentence() {
    let router = emulator();
    let vpc = "vpc-0123456789abcdef0";
    router.hold_vpcs([vpc]);
    // A stored configuration, so the refusals are about the requested id and
    // not about an empty store.
    let stored = config_id(&router, "present-config").await;

    for (case, unknown) in [
        (
            "an id of the form this service mints",
            "rqlc-0123456789abcdef0".to_string(),
        ),
        (
            "an association's id",
            "rqlca-0123456789abcdef0".to_string(),
        ),
        ("no Resolver form at all", "not-a-config-id".to_string()),
        ("the shortest ResourceId (length 1)", "c".to_string()),
        ("the longest ResourceId (length 64)", "c".repeat(64)),
    ] {
        for (operation, body) in [
            (
                "AssociateResolverQueryLogConfig",
                json!({ "ResolverQueryLogConfigId": unknown, "ResourceId": vpc }),
            ),
            (
                "DisassociateResolverQueryLogConfig",
                json!({ "ResolverQueryLogConfigId": unknown, "ResourceId": vpc }),
            ),
            (
                "GetResolverQueryLogConfig",
                json!({ "ResolverQueryLogConfigId": unknown }),
            ),
        ] {
            let (status, parsed) = call(&router, operation, body).await;
            assert_traced_refusal(
                case,
                operation,
                status,
                &parsed,
                "ResourceNotFoundException",
                CONFIG_GONE_SENTENCE,
            );
            assert_eq!(
                string_member(&parsed, "ResourceType"),
                "ResolverQueryLogConfig",
                "case {case:?}/{operation}: the refusal must name the missing resource, got \
                 {parsed}"
            );
        }

        // Nothing dangling was created for the id that resolves to nothing.
        let listed = ok(
            &router,
            "ListResolverQueryLogConfigAssociations",
            json!({}),
        )
        .await;
        assert_eq!(
            listed.get("TotalCount").and_then(Value::as_i64),
            Some(0),
            "case {case:?}: a refused associate must create no association, got {listed}"
        );
    }

    // The refusals are about the id: the stored configuration still reads back.
    let fetched = ok(
        &router,
        "GetResolverQueryLogConfig",
        json!({ "ResolverQueryLogConfigId": stored.clone() }),
    )
    .await;
    assert_eq!(
        fetched
            .get("ResolverQueryLogConfig")
            .map(|config| string_member(config, "Id")),
        Some(stored),
        "case \"the stored id\": the read must answer the stored configuration, got {fetched}"
    );
}

/// Once an association is gone, every way of addressing it answers the one
/// coded sentence for a missing association — by the association's own id and
/// by the (configuration, VPC) pair it was made from — and that sentence names
/// neither half of the pair, so it is told apart from a missing CONFIGURATION by
/// its code alone.
#[tokio::test]
async fn a_gone_query_log_config_association_is_refused_with_the_coded_sentence() {
    let router = emulator();
    let config = config_id(&router, "gone-association").await;
    let other_config = config_id(&router, "other-config").await;
    let vpc = "vpc-0123456789abcdef0";
    let unassociated_vpc = "vpc-00000000000000009";
    router.hold_vpcs([vpc, unassociated_vpc]);

    let made = ok(
        &router,
        "AssociateResolverQueryLogConfig",
        json!({ "ResolverQueryLogConfigId": config.clone(), "ResourceId": vpc }),
    )
    .await;
    let association_id = string_member(association(&made), "Id");

    ok(
        &router,
        "DisassociateResolverQueryLogConfig",
        json!({ "ResolverQueryLogConfigId": config.clone(), "ResourceId": vpc }),
    )
    .await;

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "a read of the association that is gone",
            "GetResolverQueryLogConfigAssociation",
            json!({ "ResolverQueryLogConfigAssociationId": association_id.clone() }),
        ),
        (
            "a second disassociate of the same pair",
            "DisassociateResolverQueryLogConfig",
            json!({ "ResolverQueryLogConfigId": config.clone(), "ResourceId": vpc }),
        ),
        (
            "a pair no association was ever made from",
            "DisassociateResolverQueryLogConfig",
            json!({ "ResolverQueryLogConfigId": config.clone(), "ResourceId": unassociated_vpc }),
        ),
        (
            "a stored configuration the VPC was associated with another one of",
            "DisassociateResolverQueryLogConfig",
            json!({ "ResolverQueryLogConfigId": other_config, "ResourceId": vpc }),
        ),
        (
            "an association id nothing minted",
            "GetResolverQueryLogConfigAssociation",
            json!({ "ResolverQueryLogConfigAssociationId": "rqlca-0123456789abcdef0" }),
        ),
    ];
    for (case, operation, body) in &cases {
        let (status, parsed) = call(&router, operation, body.clone()).await;
        assert_traced_refusal(
            case,
            operation,
            status,
            &parsed,
            "ResourceNotFoundException",
            ASSOCIATION_GONE_SENTENCE,
        );
        assert_eq!(
            string_member(&parsed, "ResourceType"),
            "ResolverQueryLogConfigAssociation",
            "case {case:?}/{operation}: the refusal must name the missing resource, got {parsed}"
        );
    }
}

/// `GetResolverRuleAssociation` answers an id it holds no rule association under
/// with the service's own sentence for that fault: the modelled resource name,
/// the requested id double-quoted, and no service error code — the one message
/// AWS returns, whether the association was never made or has been
/// disassociated.
///
/// The table spans the forms a `ResourceId` (length 1..64) takes, and the one id
/// the store really did mint and then took out.
#[tokio::test]
async fn a_missing_rule_association_is_refused_with_the_service_sentence() {
    let router = emulator();
    let endpoint = outbound_endpoint(&router, "endpoint-assoc-gone").await;
    let rule_id = forward_rule(&router, "rule-assoc-gone", "assoc.example.com", &endpoint).await;
    let vpc = "vpc-00000000000000041";
    let gone = associate(&router, &rule_id, vpc).await;
    ok(
        &router,
        "DisassociateResolverRule",
        json!({ "ResolverRuleId": rule_id, "VPCId": vpc }),
    )
    .await;

    for (case, id) in [
        ("the id of an association that was disassociated", gone),
        (
            "an id of the form this service mints",
            "rslvr-rrassoc-0000000000000000".to_string(),
        ),
        ("no Resolver form at all", "not-an-association".to_string()),
        ("the shortest ResourceId (length 1)", "a".to_string()),
        ("the longest ResourceId (length 64)", "a".repeat(64)),
        (
            "an id carrying the quote character the sentence uses",
            "rslvr-rrassoc-\"quoted\"".to_string(),
        ),
    ] {
        let (status, parsed) = call(
            &router,
            "GetResolverRuleAssociation",
            json!({ "ResolverRuleAssociationId": id.clone() }),
        )
        .await;
        let expected = format!(r#"ResolverRuleAssociation "{id}" does not Exist."#);
        assert_refusal(
            case,
            "GetResolverRuleAssociation",
            status,
            &parsed,
            "ResourceNotFoundException",
            &expected,
        );
        assert_eq!(
            string_member(&parsed, "ResourceType"),
            "ResolverRuleAssociation",
            "case {case:?}: the refusal must name the missing resource, got {parsed}"
        );
    }
}

/// Every paginated listing that models `InvalidNextTokenException` refuses a
/// cursor it never issued with the one coded sentence, which quotes the token
/// that was sent — and the same request without a token is still served.
///
/// The table spans what the rule generalises over: the operation (six listings,
/// one of them scoped to a resource and one to a tagged resource), and the form
/// of the token (text that is no cursor at all, an empty token, hex that decodes
/// to nothing, and a cursor this service really did mint for ANOTHER query).
#[tokio::test]
async fn an_unissued_next_token_is_refused_with_the_coded_sentence() {
    let router = emulator();
    let endpoint = outbound_endpoint(&router, "endpoint-next-token").await;
    let rule_id = forward_rule(&router, "rule-next-token", "token.example.com", &endpoint).await;
    let rule_arn = {
        let fetched = ok(
            &router,
            "GetResolverRule",
            json!({ "ResolverRuleId": rule_id.clone() }),
        )
        .await;
        string_member(rule(&fetched), "Arn")
    };
    let vpc = "vpc-00000000000000051";
    associate(&router, &rule_id, vpc).await;
    config_id(&router, "config-next-token").await;

    // A cursor this service really minted, for a DIFFERENT collection: the
    // listings each page their own, so one is not a cursor another accepts.
    let minted = {
        let first_page = ok(
            &router,
            "ListResolverEndpoints",
            json!({ "MaxResults": 1 }),
        )
        .await;
        string_member(&first_page, "NextToken")
    };

    let listings: Vec<(&str, Value)> = vec![
        ("ListResolverEndpoints", json!({})),
        ("ListResolverRules", json!({})),
        ("ListResolverRuleAssociations", json!({})),
        ("ListResolverQueryLogConfigs", json!({})),
        ("ListResolverDnssecConfigs", json!({})),
        (
            "ListResolverEndpointIpAddresses",
            json!({ "ResolverEndpointId": endpoint.clone() }),
        ),
        (
            "ListTagsForResource",
            json!({ "ResourceArn": rule_arn.clone() }),
        ),
    ];
    let tokens: Vec<(&str, String)> = vec![
        ("text that is no cursor at all", "not-a-real-next-token-!!!".to_string()),
        ("a single character", "x".to_string()),
        ("hex that decodes to no cursor", "deadbeef".to_string()),
        ("a cursor minted for another query", minted),
    ];

    for (operation, base) in &listings {
        for (case, token) in &tokens {
            let mut body = base.clone();
            body.as_object_mut()
                .expect("the request is an object")
                .insert("NextToken".to_string(), json!(token));
            let (status, parsed) = call(&router, operation, body).await;
            let expected = format!(
                r#"[{CODE_INVALID_NEXT_TOKEN}] Invalid next token: "{token}", invalid version."#
            );
            assert_refusal(
                case,
                operation,
                status,
                &parsed,
                "InvalidNextTokenException",
                &expected,
            );
        }

        // The refusal is about the token: the same query without one, and with
        // a page size, is served.
        let mut sized = base.clone();
        sized
            .as_object_mut()
            .expect("the request is an object")
            .insert("MaxResults".to_string(), json!(1));
        ok(&router, operation, sized).await;
    }
}

/// Every listing that accepts `Filters` refuses a `Filters[].Name` outside its
/// catalog with the one coded sentence, which quotes the name that was sent and
/// names no operation — and a valid name on the same call is still served.
///
/// The table spans what the rule generalises over: the operation (six listings,
/// each with its own catalog), and the form of the name (a plausible-looking
/// Resolver member, a name valid for ANOTHER listing's catalog, one that differs
/// from a valid name by a single character, and both published bounds of
/// `FilterName`'s length 1..64).
#[tokio::test]
async fn an_unknown_filter_key_is_refused_with_the_coded_sentence() {
    let router = emulator();
    let endpoint = outbound_endpoint(&router, "endpoint-filter-key").await;
    let rule_id = forward_rule(&router, "rule-filter-key", "filter.example.com", &endpoint).await;
    let vpc = "vpc-00000000000000061";
    associate(&router, &rule_id, vpc).await;
    let config = config_id(&router, "config-filter-key").await;
    ok(
        &router,
        "AssociateResolverQueryLogConfig",
        json!({ "ResolverQueryLogConfigId": config, "ResourceId": vpc }),
    )
    .await;

    // Each listing with a name its own catalog carries, and one a DIFFERENT
    // listing's catalog carries but this one does not — from the catalogs the
    // model documents for `Filter$Name`
    // (`smithy_query(query="doc",shape="Filter",member="Name")`).
    let listings: Vec<(&str, &str, &str)> = vec![
        ("ListResolverEndpoints", "Direction", "ValidationStatus"),
        ("ListResolverRules", "CreatorRequestId", "Direction"),
        ("ListResolverRuleAssociations", "ResolverRuleId", "Direction"),
        ("ListResolverQueryLogConfigs", "Name", "Direction"),
        (
            "ListResolverQueryLogConfigAssociations",
            "ResolverQueryLogConfigId",
            "Direction",
        ),
        ("ListResolverDnssecConfigs", "ResourceId", "Direction"),
    ];
    let names: Vec<(&str, String)> = vec![
        (
            "a plausible-looking Resolver member",
            "NotARealResolverFilterName".to_string(),
        ),
        ("one character off a valid name", "Statuss".to_string()),
        ("the shortest FilterName (length 1)", "f".to_string()),
        ("the longest FilterName (length 64)", "f".repeat(64)),
    ];

    for (operation, valid_name, foreign_name) in &listings {
        let mut rows = names.clone();
        rows.push((
            "a name valid only for another listing's catalog",
            (*foreign_name).to_string(),
        ));
        for (case, name) in &rows {
            // A name another catalog carries is unknown HERE, so a row whose
            // name this listing does accept is not a refusal case.
            if name == valid_name {
                continue;
            }
            let (status, parsed) = call(
                &router,
                operation,
                json!({ "Filters": [{ "Name": name, "Values": ["whatever"] }] }),
            )
            .await;
            let expected =
                format!(r#"[{CODE_INVALID_FILTER_KEY}] The filter key "{name}" is invalid."#);
            assert_refusal(
                case,
                operation,
                status,
                &parsed,
                "InvalidParameterException",
                &expected,
            );
        }

        // The refusal is about the key: a name this listing's catalog carries
        // is served on the same call.
        ok(
            &router,
            operation,
            json!({ "Filters": [{ "Name": valid_name, "Values": ["whatever"] }] }),
        )
        .await;
    }
}
