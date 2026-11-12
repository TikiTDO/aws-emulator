// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateDomainName`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder, the domain-name
//! create helper and the PATCH wrappers are this crate's existing harness, reused from
//! `rest_api_lifecycle`, `domain_name_collection` and `patch_updates`.
//!
//! The RULE under test: `UpdateDomainName` applies every entry of `patchOperations` to the
//! stored `DomainName` record the `domainName` (and, for a private record, the
//! `domainNameId`) addresses, in request order, and reports the patched record — which a
//! later `GetDomainName` reads back member for member — while a patch the service cannot
//! apply is refused with a modelled error and leaves the stored record exactly as it was.
//! An update that moves the record to another endpoint type migrates it to that endpoint,
//! which reassigns the endpoint names the record reports and starts the migration its
//! status is named for.
//!
//! The cases assert that rule over SETS of inputs rather than one recorded call: a table
//! of members, ops, starting records and request lengths; the whole declared value set of
//! each enum-typed member and values outside it; the paths and ops the resource does not
//! support; each patchable member's character bound and one past it; both endpoint types a
//! public record can be migrated between; and the forms a name that addresses no stored
//! record takes.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::types::{
    ENDPOINT_ACCESS_MODE_VALUES, ENDPOINT_TYPE_VALUES, IP_ADDRESS_TYPE_VALUES, ROUTING_MODE_VALUES,
    SECURITY_POLICY_VALUES,
};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    invalid_request_input, DEFAULT_SECURITY_POLICY, DOMAIN_NAME_AVAILABLE, DOMAIN_NAME_CREATE_SPAN,
    DOMAIN_NAME_MIGRATION_SPAN, DOMAIN_NAME_UPDATING, EDGE_DISTRIBUTION_HOSTED_ZONE_ID,
    IPV4_ADDRESS_TYPE, MAX_POLICY_CHARS, MAX_STRING_CHARS,
};

use crate::domain_name_collection::created_domain;
use crate::patch_updates::{patch, patch_ok, replace};
use crate::rest_api_lifecycle::{assert_failure, string_member, without, Answer, Gateway};

const BAD_REQUEST: &str = "BadRequestException";
const NOT_FOUND: &str = "NotFoundException";
const CONFLICT: &str = "ConflictException";

/// Slack added to a stored span before asserting the state it is due in, so a loaded
/// machine does not read the record one tick early.
const MARGIN: Duration = Duration::from_millis(100);

/// One `patchOperations` entry, as a client spells it on the wire; `None` is an op that
/// carries no `value`.
fn op(kind: &str, path: &str, value: Option<&str>) -> Value {
    match value {
        Some(value) => json!({ "op": kind, "path": path, "value": value }),
        None => json!({ "op": kind, "path": path }),
    }
}

/// The `UpdateDomainName`/`GetDomainName` URI for one domain name of the account.
fn domain_uri(domain_name: &str) -> String {
    format!("/domainnames/{domain_name}")
}

/// `GetDomainName` for `domain_name`, with `query` appended when it is not empty.
async fn get_domain(gateway: &Gateway, domain_name: &str, query: &str) -> Answer {
    let uri = if query.is_empty() {
        domain_uri(domain_name)
    } else {
        format!("{}?{query}", domain_uri(domain_name))
    };
    gateway.call("GET", &uri, None).await
}

/// `UpdateDomainName` on `domain_name`, answering whatever the service said.
async fn update(gateway: &Gateway, domain_name: &str, operations: Vec<Value>) -> Answer {
    patch(gateway, &domain_uri(domain_name), operations).await
}

/// A response body with `domainNameStatus` taken out: the status of a domain name is a
/// transition the CLOCK owes, so two reads of one record taken at different instants may
/// legitimately report different values for it. Every test that cares about the status
/// asserts it under its own rule.
fn without_status(body: &Value) -> Value {
    without(body, &["domainNameStatus"])
}

/// A regional baseline record: its endpoint is in place the moment it is created, so no
/// create span runs while a row is patching it.
///
/// Every patchable member that a row replaces or removes starts out SET, and the ones a
/// row adds start out absent, so both directions have something to do.
fn regional_request() -> Value {
    json!({
        "endpointConfiguration": { "types": ["REGIONAL"] },
        "regionalCertificateName": "initial-regional-certificate",
        "regionalCertificateArn": "arn:aws:acm:us-west-2:000000000000:certificate/initial",
        "securityPolicy": "TLS_1_0",
        "policy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
    })
}

/// Assert every `(member, expected)` pair holds in `body`; a `Value::Null` expectation is
/// the member being ABSENT, which is what a cleared optional member looks like on the wire.
fn assert_members(body: &Value, expectations: &[(&str, Value)], case: &str, source: &str) {
    for (member, expected) in expectations {
        assert_eq!(
            body[*member], *expected,
            "case {case}: {source} must report {member} as {expected:?}, got {:?}",
            body[*member]
        );
    }
}

/// The nth distinct domain name this suite creates — a name the service's domain-name
/// grammar accepts, so only the rule under test can refuse it.
fn nth_name(n: usize) -> String {
    format!("update-{n}.example.com")
}

#[tokio::test]
async fn a_patched_domain_name_member_is_stored_and_read_back() {
    // Rows differ along what the rule generalises over: which member the path names, how
    // deeply the path reaches into the record, whether that member started out set, which
    // op the row uses, how many ops one request carries, whether two ops target one
    // member, and the form and size of the value.
    type Row = (
        &'static str,
        Value,
        Vec<Value>,
        Vec<(&'static str, Value)>,
    );
    let rows: Vec<Row> = vec![
        (
            "replace the regional certificate ARN",
            regional_request(),
            vec![replace(
                "/regionalCertificateArn",
                "arn:aws:acm:us-west-2:000000000000:certificate/renewed",
            )],
            vec![
                (
                    "regionalCertificateArn",
                    json!("arn:aws:acm:us-west-2:000000000000:certificate/renewed"),
                ),
                (
                    "regionalCertificateName",
                    json!("initial-regional-certificate"),
                ),
            ],
        ),
        (
            "replace the regional certificate name",
            regional_request(),
            vec![replace("/regionalCertificateName", "renewed-certificate")],
            vec![("regionalCertificateName", json!("renewed-certificate"))],
        ),
        (
            "remove an optional member clears it",
            regional_request(),
            vec![op("remove", "/regionalCertificateName", None)],
            vec![("regionalCertificateName", Value::Null)],
        ),
        (
            "add a member the record had none of",
            regional_request(),
            vec![op(
                "add",
                "/ownershipVerificationCertificateArn",
                Some("arn:aws:acm:us-west-2:000000000000:certificate/ownership"),
            )],
            vec![(
                "ownershipVerificationCertificateArn",
                json!("arn:aws:acm:us-west-2:000000000000:certificate/ownership"),
            )],
        ),
        (
            "replace the security policy",
            regional_request(),
            vec![replace("/securityPolicy", "TLS_1_2")],
            vec![("securityPolicy", json!("TLS_1_2"))],
        ),
        (
            "replace the routing mode",
            regional_request(),
            vec![replace("/routingMode", "ROUTING_RULE_ONLY")],
            vec![("routingMode", json!("ROUTING_RULE_ONLY"))],
        ),
        (
            "add the endpoint access mode",
            regional_request(),
            vec![op("add", "/endpointAccessMode", Some("STRICT"))],
            vec![("endpointAccessMode", json!("STRICT"))],
        ),
        (
            "replace the resource policy",
            regional_request(),
            vec![replace(
                "/policy",
                "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Deny\"}]}",
            )],
            vec![(
                "policy",
                json!("{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Deny\"}]}"),
            )],
        ),
        (
            "add the management policy",
            regional_request(),
            vec![op(
                "add",
                "/managementPolicy",
                Some("{\"Version\":\"2012-10-17\",\"Statement\":[]}"),
            )],
            vec![(
                "managementPolicy",
                json!("{\"Version\":\"2012-10-17\",\"Statement\":[]}"),
            )],
        ),
        (
            "the mutual TLS truststore of a record configured with one",
            json!({
                "endpointConfiguration": { "types": ["REGIONAL"] },
                "mutualTlsAuthentication": {
                    "truststoreUri": "s3://truststore-bucket/initial.pem",
                    "truststoreVersion": "1",
                },
            }),
            vec![
                replace(
                    "/mutualTlsAuthentication/truststoreUri",
                    "s3://truststore-bucket/rotated.pem",
                ),
                replace("/mutualTlsAuthentication/truststoreVersion", "2"),
            ],
            vec![(
                "mutualTlsAuthentication",
                json!({
                    "truststoreUri": "s3://truststore-bucket/rotated.pem",
                    "truststoreVersion": "2",
                }),
            )],
        ),
        (
            "the mutual TLS truststore of a record configured with none",
            regional_request(),
            vec![op(
                "add",
                "/mutualTlsAuthentication/truststoreUri",
                Some("s3://truststore-bucket/first.pem"),
            )],
            vec![(
                "mutualTlsAuthentication",
                json!({ "truststoreUri": "s3://truststore-bucket/first.pem" }),
            )],
        ),
        (
            "the truststore version cleared by a remove",
            json!({
                "endpointConfiguration": { "types": ["REGIONAL"] },
                "mutualTlsAuthentication": {
                    "truststoreUri": "s3://truststore-bucket/initial.pem",
                    "truststoreVersion": "1",
                },
            }),
            vec![op(
                "remove",
                "/mutualTlsAuthentication/truststoreVersion",
                None,
            )],
            vec![(
                "mutualTlsAuthentication",
                json!({ "truststoreUri": "s3://truststore-bucket/initial.pem" }),
            )],
        ),
        (
            "a truststore member cleared on a record that carried none reports none",
            regional_request(),
            vec![op(
                "remove",
                "/mutualTlsAuthentication/truststoreVersion",
                None,
            )],
            vec![("mutualTlsAuthentication", Value::Null)],
        ),
        (
            "every truststore member cleared reports no mutual TLS at all",
            json!({
                "endpointConfiguration": { "types": ["REGIONAL"] },
                "mutualTlsAuthentication": {
                    "truststoreUri": "s3://truststore-bucket/initial.pem",
                    "truststoreVersion": "1",
                },
            }),
            vec![
                op("remove", "/mutualTlsAuthentication/truststoreUri", None),
                op("remove", "/mutualTlsAuthentication/truststoreVersion", None),
            ],
            vec![("mutualTlsAuthentication", Value::Null)],
        ),
        (
            "a certificate removed from a regional record clears its upload date too",
            json!({
                "endpointConfiguration": { "types": ["REGIONAL"] },
                "certificateArn": "arn:aws:acm:us-west-2:000000000000:certificate/stale",
            }),
            vec![op("remove", "/certificateArn", None)],
            vec![
                ("certificateArn", Value::Null),
                ("certificateUploadDate", Value::Null),
            ],
        ),
        (
            "the endpoint's IP address type",
            regional_request(),
            vec![replace("/endpointConfiguration/ipAddressType", "dualstack")],
            vec![(
                "endpointConfiguration",
                json!({ "types": ["REGIONAL"], "ipAddressType": "dualstack" }),
            )],
        ),
        (
            "a VPC endpoint id added to the endpoint configuration",
            regional_request(),
            vec![op(
                "add",
                "/endpointConfiguration/vpcEndpointIds/vpce-0abc",
                None,
            )],
            vec![(
                "endpointConfiguration",
                json!({
                    "types": ["REGIONAL"],
                    "ipAddressType": IPV4_ADDRESS_TYPE,
                    "vpcEndpointIds": ["vpce-0abc"],
                }),
            )],
        ),
        (
            "several members in one request",
            regional_request(),
            vec![
                replace("/securityPolicy", "TLS_1_2"),
                replace("/regionalCertificateName", "bundled"),
                replace("/routingMode", "BASE_PATH_MAPPING_ONLY"),
            ],
            vec![
                ("securityPolicy", json!("TLS_1_2")),
                ("regionalCertificateName", json!("bundled")),
                ("routingMode", json!("BASE_PATH_MAPPING_ONLY")),
            ],
        ),
        (
            "two ops on one member apply in request order",
            regional_request(),
            vec![
                replace("/securityPolicy", "TLS_1_2"),
                replace("/securityPolicy", "SecurityPolicy_TLS13_1_2_2021_06"),
            ],
            vec![(
                "securityPolicy",
                json!("SecurityPolicy_TLS13_1_2_2021_06"),
            )],
        ),
        (
            "a member cleared and then set again",
            regional_request(),
            vec![
                op("remove", "/regionalCertificateName", None),
                replace("/regionalCertificateName", "set-again"),
            ],
            vec![("regionalCertificateName", json!("set-again"))],
        ),
        (
            "a value outside ASCII",
            regional_request(),
            vec![replace("/regionalCertificateName", "сертификат-✓")],
            vec![("regionalCertificateName", json!("сертификат-✓"))],
        ),
        (
            "a value at the character bound",
            regional_request(),
            vec![replace(
                "/regionalCertificateName",
                &"c".repeat(MAX_STRING_CHARS),
            )],
            vec![(
                "regionalCertificateName",
                json!("c".repeat(MAX_STRING_CHARS)),
            )],
        ),
        (
            "a policy at the published document bound",
            regional_request(),
            vec![replace("/policy", &"p".repeat(MAX_POLICY_CHARS))],
            vec![("policy", json!("p".repeat(MAX_POLICY_CHARS)))],
        ),
        (
            "an empty list reports the record unchanged",
            regional_request(),
            vec![],
            vec![
                ("securityPolicy", json!(DEFAULT_SECURITY_POLICY)),
                (
                    "regionalCertificateName",
                    json!("initial-regional-certificate"),
                ),
            ],
        ),
        (
            "a tagged record keeps its tags",
            json!({
                "endpointConfiguration": { "types": ["REGIONAL"] },
                "tags": { "team": "api", "env": "test" },
            }),
            vec![replace("/securityPolicy", "TLS_1_2")],
            vec![
                ("securityPolicy", json!("TLS_1_2")),
                ("tags", json!({ "team": "api", "env": "test" })),
            ],
        ),
    ];

    let gateway = Gateway::new();
    for (index, (case, request, operations, expected)) in rows.into_iter().enumerate() {
        let name = nth_name(index);
        created_domain(&gateway, &name, request).await;
        let updated = patch_ok(&gateway, case, &domain_uri(&name), operations).await;
        assert_members(&updated, &expected, case, "UpdateDomainName");
        assert_eq!(
            updated["domainName"],
            json!(name),
            "case {case}: the update must report the record's own name, got {:?}",
            updated["domainName"]
        );

        // The patched record is what is STORED, not just what the update answered.
        let read = get_domain(&gateway, &name, "").await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetDomainName, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            without_status(&read.body),
            without_status(&updated),
            "case {case}: GetDomainName must read back the patched record, got {:?} after {:?}",
            read.body,
            updated
        );
    }
}

#[tokio::test]
async fn each_enum_typed_member_takes_exactly_its_declared_value_set() {
    // The value sets are the model's own, not a list this suite invented:
    // `smithy_query(query="shape", shape="SecurityPolicy,RoutingMode,EndpointAccessMode")`
    // and `EndpointConfiguration$ipAddressType`, as the crate's generated catalogs hold
    // them. Every value of every set is driven, the ones the issue cited and the ones it
    // did not, and each set is then driven with values outside it.
    let members: Vec<(&str, &str, &[&str])> = vec![
        ("securityPolicy", "/securityPolicy", SECURITY_POLICY_VALUES),
        ("routingMode", "/routingMode", ROUTING_MODE_VALUES),
        (
            "endpointAccessMode",
            "/endpointAccessMode",
            ENDPOINT_ACCESS_MODE_VALUES,
        ),
        (
            "ipAddressType",
            "/endpointConfiguration/ipAddressType",
            IP_ADDRESS_TYPE_VALUES,
        ),
    ];

    let gateway = Gateway::new();
    let name = "enum-values.example.com";
    created_domain(&gateway, name, regional_request()).await;

    for (member, path, values) in &members {
        for value in *values {
            let case = format!("{path} takes {value}");
            let updated = patch_ok(
                &gateway,
                &case,
                &domain_uri(name),
                vec![replace(path, value)],
            )
            .await;
            let reported = if *member == "ipAddressType" {
                updated["endpointConfiguration"][*member].clone()
            } else {
                updated[*member].clone()
            };
            assert_eq!(
                reported,
                json!(value),
                "case {case}: expected the value the patch set, got {reported:?}"
            );
            // Stored, not just answered.
            let read = get_domain(&gateway, name, "").await;
            assert_eq!(
                without_status(&read.body),
                without_status(&updated),
                "case {case}: GetDomainName must read back the patched record, got {:?}",
                read.body
            );
        }

        // Every value OUTSIDE the declared set is a bad request, whatever shape it takes:
        // another member's value, a value with its case changed, one with whitespace
        // around it, and the empty string.
        let outside: Vec<String> = vec![
            "TLS_1_1".to_string(),
            "NOT_A_VALUE".to_string(),
            values[0].to_lowercase(),
            values[0].to_uppercase(),
            format!(" {} ", values[0]),
            String::new(),
        ];
        for value in &outside {
            if values.contains(&value.as_str()) {
                continue;
            }
            let case = format!("{path} refuses {value:?}");
            let answer = update(&gateway, name, vec![replace(path, value)]).await;
            assert_failure(&answer, 400, BAD_REQUEST, &case);
        }
    }
}

#[tokio::test]
async fn a_patch_this_operation_cannot_apply_is_refused_and_changes_nothing() {
    // Rows differ along the KIND of refusal: an op no single-valued member supports, an op
    // missing its value, a path naming a member the service assigns or that another
    // operation owns, a path that is not a JSON Pointer, a path reaching into a
    // single-valued member, a value one character past a published bound, and a list whose
    // LAST op is refused (so the earlier ops must not be kept either).
    let cases: Vec<(&str, Vec<Value>)> = vec![
        (
            "move is not supported",
            vec![op("move", "/regionalCertificateName", None)],
        ),
        (
            "copy is not supported",
            vec![json!({
                "op": "copy",
                "path": "/regionalCertificateName",
                "from": "/certificateName",
            })],
        ),
        (
            "test is not supported",
            vec![op("test", "/regionalCertificateName", None)],
        ),
        (
            "a replace with no value",
            vec![op("replace", "/regionalCertificateName", None)],
        ),
        ("an entry with no path", vec![json!({ "op": "replace" })]),
        (
            "an entry with no op",
            vec![json!({ "path": "/regionalCertificateName" })],
        ),
        (
            "the name addresses the record, it is not patchable",
            vec![replace("/domainName", "renamed.example.com")],
        ),
        (
            "nor is the id a private record is addressed by",
            vec![replace("/domainNameId", "zzzzzzzzzz")],
        ),
        (
            "nor the ARN that spells that address",
            vec![replace(
                "/domainNameArn",
                "arn:aws:apigateway:us-west-2::/domainnames/other.example.com",
            )],
        ),
        (
            "the status is the clock's, not the caller's",
            vec![replace("/domainNameStatus", DOMAIN_NAME_AVAILABLE)],
        ),
        (
            "the status message is not patchable either",
            vec![replace("/domainNameStatusMessage", "all good")],
        ),
        (
            "the certificate upload date is stamped by the service",
            vec![replace("/certificateUploadDate", "2026-01-01T00:00:00Z")],
        ),
        (
            "the regional endpoint name is assigned with the endpoint",
            vec![replace("/regionalDomainName", "d-zzz.example.com")],
        ),
        (
            "so is the distribution name",
            vec![replace("/distributionDomainName", "dzzz.cloudfront.net")],
        ),
        (
            "and the distribution hosted zone",
            vec![replace(
                "/distributionHostedZoneId",
                EDGE_DISTRIBUTION_HOSTED_ZONE_ID,
            )],
        ),
        (
            "and the regional hosted zone",
            vec![replace("/regionalHostedZoneId", "Z2OJLYMUO9EFXC")],
        ),
        (
            "tags are changed through the tagging operations",
            vec![replace("/tags/team", "other")],
        ),
        (
            "a path naming no updateable member",
            vec![replace("/nosuchmember", "x")],
        ),
        (
            "a path that is not a JSON Pointer",
            vec![replace("regionalCertificateName", "x")],
        ),
        ("an empty path", vec![replace("", "x")]),
        (
            "a path reaching into a single-valued member",
            vec![replace("/securityPolicy/version", "TLS_1_2")],
        ),
        (
            "a sub-member the mutual TLS structure does not have",
            vec![replace("/mutualTlsAuthentication/truststoreWarnings", "none")],
        ),
        (
            "a sub-member the endpoint configuration does not have",
            vec![replace("/endpointConfiguration/nosuchmember", "x")],
        ),
        (
            "a certificate name one character past the bound",
            vec![replace(
                "/regionalCertificateName",
                &"c".repeat(MAX_STRING_CHARS + 1),
            )],
        ),
        (
            "a policy one character past the published document bound",
            vec![replace("/policy", &"p".repeat(MAX_POLICY_CHARS + 1))],
        ),
        (
            "a list whose last op is refused",
            vec![
                replace("/regionalCertificateName", "never stored"),
                replace("/nosuchmember", "x"),
            ],
        ),
    ];

    let gateway = Gateway::new();
    let name = "refusals.example.com";
    let mut request = regional_request();
    request["tags"] = json!({ "team": "api" });
    let created = created_domain(&gateway, name, request).await;
    let baseline_members = vec![
        (
            "regionalCertificateName",
            json!("initial-regional-certificate"),
        ),
        ("securityPolicy", json!(DEFAULT_SECURITY_POLICY)),
        ("tags", json!({ "team": "api" })),
    ];

    for (case, operations) in &cases {
        let answer = update(&gateway, name, operations.clone()).await;
        assert_failure(&answer, 400, BAD_REQUEST, case);

        // Nothing was applied: the stored record is the one the create left behind, which
        // every earlier row must have left alone too.
        let read = get_domain(&gateway, name, "").await;
        assert_members(&read.body, &baseline_members, case, "GetDomainName");
        assert_eq!(
            without_status(&read.body),
            without_status(&created),
            "case {case}: the refused patch must leave the created record as it was, got {:?}",
            read.body
        );
    }

    // A path the resource has no updateable member for is refused as request input: the
    // path the request sent and then the fixed sentence, and nothing else. The sentence is
    // written out here rather than read off the service's own constant, so a change of
    // wording is caught rather than followed; the constant is asserted to agree with it.
    for path in ["/domainName", "/nosuchmember", "/endpointConfiguration/zzz"] {
        let expected = format!("{path} Invalid request input");
        assert_eq!(
            invalid_request_input(path),
            expected,
            "case the service constant for {path}: expected {expected:?}, got {:?}",
            invalid_request_input(path)
        );
        let answer = update(&gateway, name, vec![replace(path, "x")]).await;
        let message = answer.body["message"].as_str().unwrap_or_default();
        assert_eq!(
            message, expected,
            "case the message of the unsupported path {path}: expected the path and the \
             fixed sentence, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_name_no_stored_record_answers_to_is_the_modelled_not_found() {
    let gateway = Gateway::new();
    let stored = "stored.example.com";
    created_domain(&gateway, stored, regional_request()).await;
    let operations = vec![replace("/securityPolicy", "TLS_1_2")];

    // Rows differ along the form of the name: a name of the same grammar that was never
    // created, a sibling label, the stored name with one label dropped, with one character
    // stepped on, and upper-cased.
    let rows: Vec<(&str, String)> = vec![
        ("a name never created", "absent.example.com".to_string()),
        ("a sibling label", "stored.example.net".to_string()),
        ("the stored name without a label", "example.com".to_string()),
        (
            "the stored name with one character changed",
            "storee.example.com".to_string(),
        ),
        ("the stored name upper-cased", stored.to_uppercase()),
    ];

    for (case, name) in &rows {
        assert_failure(
            &update(&gateway, name, operations.clone()).await,
            404,
            NOT_FOUND,
            case,
        );
    }

    // None of those refusals touched the stored record.
    let read = get_domain(&gateway, stored, "").await;
    assert_members(
        &read.body,
        &[("securityPolicy", json!(DEFAULT_SECURITY_POLICY))],
        "after the refusals",
        "GetDomainName",
    );
}

#[tokio::test]
async fn a_private_record_is_patched_through_the_id_that_addresses_it() {
    let gateway = Gateway::new();
    // Two PRIVATE records carry ONE name: each is addressed by its own `domainNameId`, so
    // a patch reaches exactly the one the id names and leaves its namesake alone.
    let name = "private.example.com";
    let request = json!({
        "endpointConfiguration": { "types": ["PRIVATE"] },
        "policy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
    });
    let first = created_domain(&gateway, name, request.clone()).await;
    let second = created_domain(&gateway, name, request).await;
    let first_id = string_member(&first, "domainNameId", "the first private record");
    let second_id = string_member(&second, "domainNameId", "the second private record");
    assert_ne!(
        first_id, second_id,
        "case two private records of one name: expected distinct ids, got {first_id}"
    );

    let updated = patch_ok(
        &gateway,
        "UpdateDomainName on the first private record",
        &format!("{}?domainNameId={first_id}", domain_uri(name)),
        vec![replace(
            "/managementPolicy",
            "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\"}]}",
        )],
    )
    .await;
    assert_members(
        &updated,
        &[
            (
                "managementPolicy",
                json!("{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\"}]}"),
            ),
            ("domainNameId", json!(first_id)),
        ],
        "the first private record",
        "UpdateDomainName",
    );

    // The namesake is the record its own create left behind.
    let sibling = get_domain(&gateway, name, &format!("domainNameId={second_id}")).await;
    assert_members(
        &sibling.body,
        &[
            ("managementPolicy", Value::Null),
            ("domainNameId", json!(second_id)),
        ],
        "the second private record",
        "GetDomainName",
    );

    // The name alone addresses no record when every record carrying it is private.
    assert_failure(
        &update(&gateway, name, vec![replace("/policy", "{}")]).await,
        404,
        NOT_FOUND,
        "the name without an id",
    );
}

#[tokio::test]
async fn an_endpoint_type_migration_reassigns_the_endpoint_and_moves_the_status() {
    let gateway = Gateway::new();
    // An edge-optimized record is served by a CloudFront distribution; migrating it to
    // REGIONAL moves it to the Region's API Gateway endpoint, so the names it reports are
    // the new endpoint's and none of the old one's.
    let name = "migrated.example.com";
    let created = created_domain(
        &gateway,
        name,
        json!({
            "certificateArn": "arn:aws:acm:us-east-1:000000000000:certificate/edge",
            "endpointConfiguration": { "types": ["EDGE"] },
        }),
    )
    .await;
    assert!(
        created["distributionDomainName"].is_string(),
        "case the created edge record: expected a distribution domain name, got {created:?}"
    );

    // Wait the create span out, so the status asserted below is the migration's and not
    // the create's.
    tokio::time::sleep(DOMAIN_NAME_CREATE_SPAN + MARGIN).await;
    let started = std::time::Instant::now();
    let migrated = patch_ok(
        &gateway,
        "migrate EDGE to REGIONAL",
        &domain_uri(name),
        vec![replace("/endpointConfiguration/types/EDGE", "REGIONAL")],
    )
    .await;
    let elapsed = started.elapsed();
    assert_members(
        &migrated,
        &[
            ("distributionDomainName", Value::Null),
            ("distributionHostedZoneId", Value::Null),
            (
                "endpointConfiguration",
                json!({ "types": ["REGIONAL"], "ipAddressType": IPV4_ADDRESS_TYPE }),
            ),
            // The upload date dates the edge-optimized endpoint's certificate, which the
            // migrated record is no longer served by.
            ("certificateUploadDate", Value::Null),
        ],
        "migrate EDGE to REGIONAL",
        "UpdateDomainName",
    );
    let regional_domain_name = string_member(
        &migrated,
        "regionalDomainName",
        "migrate EDGE to REGIONAL",
    );
    // `regionalDomainName` is the address a client points DNS at and then DIALS, so a
    // migrated record publishes an address this emulator answers on — the `host:port` of the
    // socket the update opened for it. The front door on it is driven end to end by the
    // `regional_custom_domain_front_door` suite.
    assert!(
        regional_domain_name
            .parse::<std::net::SocketAddr>()
            .is_ok(),
        "case migrate EDGE to REGIONAL: expected an address a client can dial, got \
         {regional_domain_name:?}"
    );

    // The migration is a transition the CLOCK owes: inside the span the record reports it,
    // and a second edit is refused while it runs.
    if elapsed < DOMAIN_NAME_MIGRATION_SPAN {
        assert_eq!(
            migrated["domainNameStatus"].as_str(),
            Some(DOMAIN_NAME_UPDATING),
            "case migrate EDGE to REGIONAL: expected {DOMAIN_NAME_UPDATING} {elapsed:?} \
             into the span ({DOMAIN_NAME_MIGRATION_SPAN:?}), got {:?}",
            migrated["domainNameStatus"]
        );
        let refused = update(
            &gateway,
            name,
            vec![replace("/regionalCertificateName", "while-migrating")],
        )
        .await;
        if started.elapsed() < DOMAIN_NAME_MIGRATION_SPAN {
            assert_failure(&refused, 409, CONFLICT, "an edit while the migration runs");
        }
    }

    // Once the span has passed the record is available again, at the endpoint the
    // migration moved it to, and a later read reports exactly that.
    tokio::time::sleep(DOMAIN_NAME_MIGRATION_SPAN + MARGIN).await;
    let read = get_domain(&gateway, name, "").await;
    assert_members(
        &read.body,
        &[
            ("domainNameStatus", json!(DOMAIN_NAME_AVAILABLE)),
            ("regionalDomainName", json!(regional_domain_name)),
            ("distributionDomainName", Value::Null),
        ],
        "after the migration span",
        "GetDomainName",
    );

    // And the migration runs the other way too: back to EDGE assigns a distribution.
    let back = patch_ok(
        &gateway,
        "migrate REGIONAL back to EDGE",
        &domain_uri(name),
        vec![replace("/endpointConfiguration/types/REGIONAL", "EDGE")],
    )
    .await;
    assert_members(
        &back,
        &[
            ("regionalDomainName", Value::Null),
            (
                "distributionHostedZoneId",
                json!(EDGE_DISTRIBUTION_HOSTED_ZONE_ID),
            ),
            (
                "endpointConfiguration",
                json!({ "types": ["EDGE"], "ipAddressType": IPV4_ADDRESS_TYPE }),
            ),
        ],
        "migrate REGIONAL back to EDGE",
        "UpdateDomainName",
    );
    let distribution = string_member(&back, "distributionDomainName", "migrate back to EDGE");
    assert!(
        distribution.ends_with(".cloudfront.net"),
        "case migrate REGIONAL back to EDGE: expected a distribution name, got \
         {distribution:?}"
    );
    assert_ne!(
        distribution,
        string_member(&created, "distributionDomainName", "the created edge record"),
        "case migrate REGIONAL back to EDGE: the new endpoint must carry its own name"
    );
}

#[tokio::test]
async fn an_endpoint_type_patch_the_service_cannot_serve_is_refused() {
    let gateway = Gateway::new();
    let public = "endpoint-types.example.com";
    created_domain(&gateway, public, regional_request()).await;
    let private = "endpoint-types-private.example.com";
    let created_private = created_domain(
        &gateway,
        private,
        json!({ "endpointConfiguration": { "types": ["PRIVATE"] } }),
    )
    .await;
    let private_id = string_member(&created_private, "domainNameId", "the private record");

    // Rows differ along the KIND of refusal: a value outside the declared endpoint-type
    // set, an op that would add to or empty the single-entry set, and a path naming an
    // endpoint type the record is not served by.
    let cases: Vec<(&str, Vec<Value>)> = vec![
        (
            "a value outside the declared endpoint types",
            vec![replace("/endpointConfiguration/types/REGIONAL", "GLOBAL")],
        ),
        (
            "an empty endpoint type",
            vec![replace("/endpointConfiguration/types/REGIONAL", "")],
        ),
        (
            "the declared set is case-sensitive",
            vec![replace("/endpointConfiguration/types/REGIONAL", "regional")],
        ),
        (
            "an add would give the record a second endpoint",
            vec![op("add", "/endpointConfiguration/types/EDGE", None)],
        ),
        (
            "a remove would leave it with none",
            vec![op("remove", "/endpointConfiguration/types/REGIONAL", None)],
        ),
        (
            "a path naming an endpoint type the record is not served by",
            vec![replace("/endpointConfiguration/types/EDGE", "REGIONAL")],
        ),
        (
            "PRIVATE is not an endpoint type a public record migrates to",
            vec![replace("/endpointConfiguration/types/REGIONAL", "PRIVATE")],
        ),
    ];

    for (case, operations) in &cases {
        assert_failure(
            &update(&gateway, public, operations.clone()).await,
            400,
            BAD_REQUEST,
            case,
        );
        // The endpoint the record is served by is the one its create assigned.
        let read = get_domain(&gateway, public, "").await;
        assert_members(
            &read.body,
            &[(
                "endpointConfiguration",
                json!({ "types": ["REGIONAL"], "ipAddressType": IPV4_ADDRESS_TYPE }),
            )],
            case,
            "GetDomainName",
        );
    }

    // A private record is addressed by the id its create minted, so it is not migrated out
    // of PRIVATE either, whichever public type is named.
    for value in ENDPOINT_TYPE_VALUES
        .iter()
        .filter(|value| **value != "PRIVATE")
    {
        let case = format!("a private record does not migrate to {value}");
        let answer = patch(
            &gateway,
            &format!("{}?domainNameId={private_id}", domain_uri(private)),
            vec![replace("/endpointConfiguration/types/PRIVATE", value)],
        )
        .await;
        assert_failure(&answer, 400, BAD_REQUEST, &case);
    }
}

#[tokio::test]
async fn a_patched_certificate_is_stamped_with_the_instant_it_was_supplied() {
    let gateway = Gateway::new();
    // `certificateUploadDate` is the instant the certificate an edge-optimized or private
    // endpoint uses was supplied, so a record created without one carries none until a
    // patch supplies it, and a remove of that certificate clears it again.
    let name = "certificate.example.com";
    let created = created_domain(
        &gateway,
        name,
        json!({ "endpointConfiguration": { "types": ["EDGE"] } }),
    )
    .await;
    assert_eq!(
        created["certificateUploadDate"],
        Value::Null,
        "case a record created with no certificate: expected no upload date, got {:?}",
        created["certificateUploadDate"]
    );

    let supplied = patch_ok(
        &gateway,
        "a certificate supplied by a patch",
        &domain_uri(name),
        vec![replace(
            "/certificateArn",
            "arn:aws:acm:us-east-1:000000000000:certificate/supplied",
        )],
    )
    .await;
    // A whole-second epoch number, as every timestamp of this surface travels (the
    // `epoch_second_timestamps` suite asserts that form); the floor is on the UNIT, so a
    // value below it is not a count of seconds at all.
    let stamped = supplied["certificateUploadDate"].as_i64();
    assert!(
        stamped.is_some_and(|stamped| stamped > 1_600_000_000),
        "case a certificate supplied by a patch: expected the epoch second it was \
         supplied, got {:?}",
        supplied["certificateUploadDate"]
    );
    let read = get_domain(&gateway, name, "").await;
    assert_eq!(
        read.body["certificateUploadDate"], supplied["certificateUploadDate"],
        "case a certificate supplied by a patch: the stamp must be stored, got {:?}",
        read.body["certificateUploadDate"]
    );

    let cleared = patch_ok(
        &gateway,
        "the certificate removed again",
        &domain_uri(name),
        vec![op("remove", "/certificateArn", None)],
    )
    .await;
    assert_members(
        &cleared,
        &[
            ("certificateArn", Value::Null),
            ("certificateUploadDate", Value::Null),
        ],
        "the certificate removed again",
        "UpdateDomainName",
    );
}

#[tokio::test]
async fn one_record_is_patched_and_its_siblings_are_left_alone() {
    let gateway = Gateway::new();
    let target = "target.example.com";
    let sibling = "sibling.example.com";
    created_domain(&gateway, target, regional_request()).await;
    let mut sibling_request = regional_request();
    sibling_request["regionalCertificateName"] = json!("the sibling's own certificate");
    created_domain(&gateway, sibling, sibling_request).await;

    // Applying one list repeatedly lands on the same record: a patch SETS members, it does
    // not accumulate.
    let mut seen: Vec<Value> = Vec::new();
    for round in 0..3 {
        let case = format!("UpdateDomainName(round {round})");
        seen.push(
            patch_ok(
                &gateway,
                &case,
                &domain_uri(target),
                vec![
                    replace("/securityPolicy", "TLS_1_2"),
                    replace("/regionalCertificateName", "settled"),
                ],
            )
            .await,
        );
    }
    assert_eq!(
        without_status(&seen[0]),
        without_status(&seen[2]),
        "case UpdateDomainName repeated: the same list must land on the same record, got \
         {:?} then {:?}",
        seen[0],
        seen[2]
    );

    let read = get_domain(&gateway, sibling, "").await;
    assert_members(
        &read.body,
        &[
            (
                "regionalCertificateName",
                json!("the sibling's own certificate"),
            ),
            ("securityPolicy", json!(DEFAULT_SECURITY_POLICY)),
            ("domainName", json!(sibling)),
        ],
        "the sibling record",
        "GetDomainName",
    );
}
