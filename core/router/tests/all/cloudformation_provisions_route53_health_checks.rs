// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares `AWS::Route53::HealthCheck` really creates that
//! health check in Amazon Route 53, and the value the stack hands back for it is the
//! identifier Route 53 answers to.
//!
//! The rule under test: **a stack's physical resource id — which is what `Ref` resolves to,
//! what `Fn::GetAtt` reads as the resource's own id and what an output publishes — is the
//! identifier the service that owns the resource knows it by, and for a type whose identifier
//! that service MINTS it is the one its create answered with, never a name CloudFormation
//! computed.** For a health check that identifier is the health check ID, which is what
//! `GetHealthCheck`, `GetHealthCheckStatus`, `UpdateHealthCheck` and a record set's own
//! `HealthCheckId` take. The same rule carries the rest of the resource's life: a changed
//! declaration reaches Route 53 through `UpdateHealthCheck` on that id — so the check keeps
//! its identity and its `HealthCheckVersion` advances — and deleting the stack deletes the
//! health check there.
//!
//! Two crates have to agree for that to hold (CloudFormation makes the calls; Route 53 serves
//! them and mints the ids), so the suite lives here rather than in either crate.
//!
//! The rows differ along what the rule generalises over: the KIND of health check declared
//! (an HTTPS endpoint named by domain name, an HTTP endpoint on an IP address, and a
//! CALCULATED check over a child declared in the same stack), whether the template is JSON or
//! YAML, the Region it deploys in, which intrinsic each output reads the id with (`Ref`,
//! `Fn::GetAtt` of the type's own `HealthCheckId`, `Fn::Sub`), whether a record set in the
//! stack references the check, whether tags are declared, and what the update does to the
//! configuration — change a member's value, add one the create left out, and DROP one, which
//! only `UpdateHealthCheck`'s `ResetElements` can do.
//!
//! Every expected value is computed here from what the row declares, except the health check
//! id itself, which is Route 53's to mint: what is checked about THAT is that one value
//! appears everywhere the stack reports the check's identity, that it has the published form
//! (and so satisfies the modelled 64-character bound), and that Route 53's own
//! `GetHealthCheck` resolves it.

use std::time::Duration;

use bytes::Bytes;
use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{element, form_encode, opt_element, outputs_of, physical_ids_of};

/// Long enough for the stack's own clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) to
/// come due, with room for the second transition an update moves through.
const PAST_PROVISIONING_SPANS: Duration = Duration::from_millis(1_400);

/// The most characters a health check id may have: `HealthCheckId`'s own
/// `@length(min: 0, max: 64)`, the bound every member that takes one is declared with.
const MAX_HEALTH_CHECK_ID_CHARS: usize = 64;

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way
/// (the credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: std::sync::Arc<engine::InMemoryEngine> =
        std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(
            std::sync::Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_route53_awsdnsv20130401::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which
/// is what routes the request. The signature is never checked: this router runs with request
/// authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
async fn cloudformation(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation", region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one Route 53 operation over its restXml wire, signed for `region`.
async fn route53(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
) -> (u16, String) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("route53.{region}.amazonaws.com"))
        .header("content-type", "application/xml")
        .header("authorization", authorization("route53", region))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// One output whose value is COMPUTED from the minted health check id: the key, and the
/// value the rule says it must carry once Route 53 has minted that id.
type DerivedOutput = (&'static str, fn(&str) -> String);

/// A record set the stack declares with a reference to its health check: the logical id it
/// carries, the zone it lives in and the name and type it is read back by.
struct RecordCheck {
    logical_id: &'static str,
    zone_logical_id: &'static str,
    name: &'static str,
    rr_type: &'static str,
}

/// One row of the table: the templates a stack is deployed and then updated with, and what
/// the rule says the health check Route 53 then holds must be.
struct Row {
    case: &'static str,
    /// The Region the row deploys in; the health check is created there.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML), before and after the
    /// update.
    template: fn() -> String,
    updated: fn() -> String,
    /// The logical id of the health check whose identity every output below reports.
    logical_id: &'static str,
    /// The output keys whose value must be the minted health check id, each written with a
    /// different intrinsic over the same resource.
    id_outputs: &'static [&'static str],
    /// Outputs whose value is computed from that id.
    derived_outputs: &'static [DerivedOutput],
    /// The configuration members `GetHealthCheck` must report after the create, and after
    /// the update.
    created_config: &'static [(&'static str, &'static str)],
    updated_config: &'static [(&'static str, &'static str)],
    /// Members the update DROPPED, which the health check must no longer report.
    reset_by_update: &'static [&'static str],
    /// The record set that references the health check, where the row declares one.
    record: Option<RecordCheck>,
}

const ROWS: &[Row] = &[
    Row {
        case: "an HTTPS check named by domain name, referenced by a weighted record, JSON",
        region: "us-east-1",
        template: || {
            health_check_stack_template(json!({
                "Type": "HTTPS",
                "FullyQualifiedDomainName": "api.cfn-health.example",
                "Port": 443,
                "ResourcePath": "/prod/healthy",
                "RequestInterval": 10,
                "FailureThreshold": 1
            }))
        },
        updated: || {
            health_check_stack_template(json!({
                "Type": "HTTPS",
                "FullyQualifiedDomainName": "api.cfn-health.example",
                "Port": 443,
                "ResourcePath": "/prod/ready",
                "RequestInterval": 10,
                "FailureThreshold": 3
            }))
        },
        logical_id: "HealthCheck",
        id_outputs: &["HealthCheckId", "HealthCheckIdAttribute"],
        derived_outputs: &[("HealthCheckPath", |id| {
            format!("/2013-04-01/healthcheck/{id}")
        })],
        created_config: &[
            ("Type", "HTTPS"),
            ("FullyQualifiedDomainName", "api.cfn-health.example"),
            ("Port", "443"),
            ("ResourcePath", "/prod/healthy"),
            ("RequestInterval", "10"),
            ("FailureThreshold", "1"),
        ],
        updated_config: &[
            ("Type", "HTTPS"),
            ("ResourcePath", "/prod/ready"),
            ("FailureThreshold", "3"),
        ],
        reset_by_update: &[],
        record: Some(RecordCheck {
            logical_id: "Record",
            zone_logical_id: "Zone",
            name: "weighted.cfn-health.example.",
            rr_type: "A",
        }),
    },
    Row {
        case: "an HTTP check on an IP address whose update drops its resource path, YAML",
        region: "eu-west-1",
        template: || {
            [
                "Resources:",
                "  Probe:",
                "    Type: AWS::Route53::HealthCheck",
                "    Properties:",
                "      HealthCheckConfig:",
                "        Type: HTTP",
                "        IPAddress: 192.0.2.44",
                "        Port: 80",
                "        ResourcePath: /status",
                "        Inverted: false",
                "      HealthCheckTags:",
                "        - Key: Owner",
                "          Value: networking",
                "Outputs:",
                "  HealthCheckId:",
                "    Value: !Ref Probe",
                "  HealthCheckIdAttribute:",
                "    Value: !GetAtt Probe.HealthCheckId",
                "",
            ]
            .join("\n")
        },
        updated: || {
            [
                "Resources:",
                "  Probe:",
                "    Type: AWS::Route53::HealthCheck",
                "    Properties:",
                "      HealthCheckConfig:",
                "        Type: HTTP",
                "        IPAddress: 192.0.2.44",
                "        Port: 8080",
                "        Inverted: true",
                "        FailureThreshold: 5",
                "      HealthCheckTags:",
                "        - Key: Owner",
                "          Value: networking",
                "Outputs:",
                "  HealthCheckId:",
                "    Value: !Ref Probe",
                "  HealthCheckIdAttribute:",
                "    Value: !GetAtt Probe.HealthCheckId",
                "",
            ]
            .join("\n")
        },
        logical_id: "Probe",
        id_outputs: &["HealthCheckId", "HealthCheckIdAttribute"],
        derived_outputs: &[],
        created_config: &[
            ("Type", "HTTP"),
            ("IPAddress", "192.0.2.44"),
            ("Port", "80"),
            ("ResourcePath", "/status"),
            ("Inverted", "false"),
        ],
        updated_config: &[
            ("Type", "HTTP"),
            ("Port", "8080"),
            ("Inverted", "true"),
            ("FailureThreshold", "5"),
        ],
        reset_by_update: &["ResourcePath"],
        record: None,
    },
    Row {
        case: "a CALCULATED check over a child declared in the same stack, JSON",
        region: "ap-south-1",
        template: || calculated_stack_template(1),
        updated: || calculated_stack_template(2),
        logical_id: "Parent",
        id_outputs: &["HealthCheckId"],
        derived_outputs: &[],
        created_config: &[("Type", "CALCULATED"), ("HealthThreshold", "1")],
        updated_config: &[("Type", "CALCULATED"), ("HealthThreshold", "2")],
        reset_by_update: &[],
        record: None,
    },
];

/// A template declaring a health check, a hosted zone and a weighted record set that
/// references the check through `Ref`, publishing the check's identity three ways.
fn health_check_stack_template(config: serde_json::Value) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Description": "a health check a record set is conditional on",
        "Resources": {
            "HealthCheck": {
                "Type": "AWS::Route53::HealthCheck",
                "Properties": {
                    "HealthCheckConfig": config,
                    "HealthCheckTags": [{"Key": "Environment", "Value": "test"}]
                }
            },
            "Zone": {
                "Type": "AWS::Route53::HostedZone",
                "Properties": {"Name": "cfn-health.example"}
            },
            "Record": {
                "Type": "AWS::Route53::RecordSet",
                "Properties": {
                    "HostedZoneId": {"Ref": "Zone"},
                    "Name": "weighted.cfn-health.example.",
                    "Type": "A",
                    "SetIdentifier": "primary",
                    "Weight": 100,
                    "TTL": "60",
                    "ResourceRecords": ["192.0.2.47"],
                    "HealthCheckId": {"Ref": "HealthCheck"}
                }
            }
        },
        "Outputs": {
            "HealthCheckId": {"Value": {"Ref": "HealthCheck"}},
            "HealthCheckIdAttribute": {
                "Value": {"Fn::GetAtt": ["HealthCheck", "HealthCheckId"]}
            },
            "HealthCheckPath": {
                "Value": {
                    "Fn::Sub": [
                        "/2013-04-01/healthcheck/${Id}",
                        {"Id": {"Ref": "HealthCheck"}}
                    ]
                }
            }
        }
    })
    .to_string()
}

/// A template declaring a TCP check and a CALCULATED check over it, the parent naming the
/// child by `Ref` — which is the health check ID Route 53 minted for it.
fn calculated_stack_template(threshold: i64) -> String {
    json!({
        "Resources": {
            "Child": {
                "Type": "AWS::Route53::HealthCheck",
                "Properties": {
                    "HealthCheckConfig": {
                        "Type": "TCP",
                        "IPAddress": "192.0.2.60",
                        "Port": 443
                    }
                }
            },
            "Parent": {
                "Type": "AWS::Route53::HealthCheck",
                "Properties": {
                    "HealthCheckConfig": {
                        "Type": "CALCULATED",
                        "HealthThreshold": threshold,
                        "ChildHealthChecks": [{"Ref": "Child"}]
                    }
                }
            }
        },
        "Outputs": {
            "HealthCheckId": {"Value": {"Ref": "Parent"}},
            "ChildId": {"Value": {"Ref": "Child"}}
        }
    })
    .to_string()
}

/// Whether `id` is a health check id of the form Route 53 publishes: the 8-4-4-4-12
/// lowercase-hex grouping, inside the 64-character bound `HealthCheckId` fixes.
fn is_health_check_id(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    id.len() <= MAX_HEALTH_CHECK_ID_CHARS
        && groups.len() == 5
        && groups
            .iter()
            .map(|group| group.len())
            .eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()))
}

/// Deploy a stack with `action` and answer the `DescribeStacks` body once it has settled,
/// asserting it settled in `expected`.
async fn deploy_and_settle(
    router: &MultiServiceRouter,
    row: &Row,
    action: &str,
    stack_name: &str,
    template: &str,
    expected: &str,
) -> String {
    let case = row.case;
    let (status, body) = cloudformation(
        router,
        row.region,
        action,
        &[("StackName", stack_name), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_PROVISIONING_SPANS).await;
    let (status, described) = cloudformation(
        router,
        row.region,
        "DescribeStacks",
        &[("StackName", stack_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
         {described}"
    );
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some(expected),
        "case {case:?}: expected the stack to reach {expected}, got {described}"
    );
    described
}

/// Read one health check back through Route 53's own `GetHealthCheck`, which must answer it.
async fn health_check_of(
    router: &MultiServiceRouter,
    row: &Row,
    id: &str,
    stage: &str,
) -> String {
    let (status, body) = route53(
        router,
        row.region,
        "GET",
        &format!("/2013-04-01/healthcheck/{id}"),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?} ({stage}): expected HTTP 200 from GetHealthCheck on the id the stack \
         reported, got {status} with body {body}",
        row.case
    );
    body
}

/// The `HealthCheckVersion` a `GetHealthCheck` answer reports.
fn version_of(body: &str, case: &str, stage: &str) -> i64 {
    let reported = element(body, "HealthCheckVersion");
    reported.parse::<i64>().unwrap_or_else(|_| {
        panic!("case {case:?} ({stage}): HealthCheckVersion must be a number, got {body}")
    })
}

#[tokio::test]
async fn a_stack_publishes_the_health_check_id_route53_minted() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("route53-health-check-stack-{index}");

        let described = deploy_and_settle(
            &router,
            row,
            "CreateStack",
            &stack_name,
            &(row.template)(),
            "CREATE_COMPLETE",
        )
        .await;
        let (status, resources) = cloudformation(
            &router,
            region,
            "DescribeStackResources",
            &[("StackName", &stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with \
             body {resources}"
        );
        let physical_ids = physical_ids_of(&resources);
        let outputs = outputs_of(&described);

        // The id the stack reports for the health check: Route 53's to mint, so what is
        // checked about it is its published FORM and the bound every member that takes one
        // carries.
        let id = physical_ids
            .get(row.logical_id)
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the stack reports no resource {:?}: {resources}",
                    row.logical_id
                )
            });
        assert!(
            is_health_check_id(&id),
            "case {case:?}: expected the health check's physical id to be a Route 53 health \
             check id (8-4-4-4-12 lowercase hex, at most {MAX_HEALTH_CHECK_ID_CHARS} \
             characters), got {id:?}"
        );
        for key in row.id_outputs {
            assert_eq!(
                outputs.get(*key),
                Some(&id),
                "case {case:?}: expected output {key:?} to be the minted health check id \
                 {id:?}, got {:?}",
                outputs.get(*key)
            );
        }
        for (key, expected) in row.derived_outputs {
            let expected = expected(&id);
            assert_eq!(
                outputs.get(*key),
                Some(&expected),
                "case {case:?}: expected output {key:?} to be {expected:?}, got {:?}",
                outputs.get(*key)
            );
        }

        // Route 53 itself resolves that id to the health check the template declared — the
        // very call a client makes with the stack's output — and reports the configuration
        // the declaration stated.
        let read = health_check_of(&router, row, &id, "after the create").await;
        for (member, expected) in row.created_config {
            assert_eq!(
                opt_element(&read, member).as_deref(),
                Some(*expected),
                "case {case:?}: expected the health check to report {member} {expected:?}, \
                 got {read}"
            );
        }
        let created_version = version_of(&read, case, "after the create");
        assert_eq!(
            opt_element(&read, "Id").as_deref(),
            Some(id.as_str()),
            "case {case:?}: expected the health check to report the id the stack published, \
             got {read}"
        );

        // A record set the stack declares against the check carries the same id, which is
        // what makes health-check-backed DNS reachable from a template at all.
        if let Some(record) = &row.record {
            let zone = physical_ids
                .get(record.zone_logical_id)
                .cloned()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: the stack reports no resource {:?}: {resources}",
                        record.zone_logical_id
                    )
                });
            let (status, listed) = route53(
                &router,
                region,
                "GET",
                &format!(
                    "/2013-04-01/hostedzone/{zone}/rrset?name={}&type={}&maxitems=1",
                    form_encode(record.name),
                    record.rr_type
                ),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from ListResourceRecordSets, got {status} \
                 with body {listed}"
            );
            assert_eq!(
                opt_element(&listed, "HealthCheckId").as_deref(),
                Some(id.as_str()),
                "case {case:?}: expected the record set to be conditional on the minted \
                 health check {id:?}, got {listed}"
            );
            // "Ref returns the record name": the name the declaration states, with the
            // terminal dot dropped, which is the form CloudFormation reports a DNS record
            // name in.
            let expected_ref = record.name.trim_end_matches('.');
            assert_eq!(
                physical_ids.get(record.logical_id).map(String::as_str),
                Some(expected_ref),
                "case {case:?}: expected the record set's physical id to be the record name \
                 it declared without its terminal dot ({expected_ref:?}), got {:?} in \
                 {resources}",
                physical_ids.get(record.logical_id)
            );
        }

        // A changed declaration reaches Route 53 through `UpdateHealthCheck` on that id: the
        // check keeps its identity, reports the new configuration and its version advances.
        let described = deploy_and_settle(
            &router,
            row,
            "UpdateStack",
            &stack_name,
            &(row.updated)(),
            "UPDATE_COMPLETE",
        )
        .await;
        let outputs = outputs_of(&described);
        for key in row.id_outputs {
            assert_eq!(
                outputs.get(*key),
                Some(&id),
                "case {case:?}: expected output {key:?} to still be {id:?} after the update, \
                 got {:?}",
                outputs.get(*key)
            );
        }
        let read = health_check_of(&router, row, &id, "after the update").await;
        for (member, expected) in row.updated_config {
            assert_eq!(
                opt_element(&read, member).as_deref(),
                Some(*expected),
                "case {case:?}: expected the updated health check to report {member} \
                 {expected:?}, got {read}"
            );
        }
        for member in row.reset_by_update {
            assert_eq!(
                opt_element(&read, member),
                None,
                "case {case:?}: expected the update to reset {member} to its default, got \
                 {read}"
            );
        }
        let updated_version = version_of(&read, case, "after the update");
        assert!(
            updated_version > created_version,
            "case {case:?}: expected the HealthCheckVersion to advance past \
             {created_version}, got {updated_version}"
        );

        // Deleting the stack deletes the health check in Route 53.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        let (status, gone) = route53(
            &router,
            region,
            "GET",
            &format!("/2013-04-01/healthcheck/{id}"),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 from GetHealthCheck once the stack deleted the \
             health check, got {status} with body {gone}"
        );
        assert_eq!(
            element(&gone, "Code"),
            "NoSuchHealthCheck",
            "case {case:?}: expected the health check to be gone, got {gone}"
        );
    }
}

/// The suite's own helper, held to the forms the rule is about: a minted id passes, the
/// CloudFormation-style physical name this issue was filed about does not.
#[test]
fn the_health_check_id_form_admits_only_an_id() {
    // (case, the value, whether it is a health check id)
    let rows: [(&str, &str, bool); 5] = [
        ("an id Route 53 publishes", "6de0bd49-2070-7764-6e56-4bf63dd5ce8a", true),
        ("another minted id", "00000000-0000-4000-8000-000000000000", true),
        (
            "a synthesized CloudFormation physical name",
            "sim2real-r53-cfn-e9c58310712b-HealthCheck-51481AACD40C",
            false,
        ),
        ("an id with uppercase hex", "6DE0BD49-2070-7764-6E56-4BF63DD5CE8A", false),
        ("a hosted zone id", "Z1D633PJN98FT9", false),
    ];
    for (case, value, expected) in rows {
        let read = is_health_check_id(value);
        assert_eq!(
            read, expected,
            "case {case:?}: expected is_health_check_id({value:?}) to be {expected}, got {read}"
        );
    }
}
