// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares `AWS::Route53::HostedZone` and
//! `AWS::Route53::RecordSet` really creates them in Amazon Route 53, and the value the stack
//! hands back for the zone is the identifier Route 53 answers to.
//!
//! The rule under test: **a stack's physical resource id — which is what `Ref` resolves to,
//! what `Fn::GetAtt` reads as the resource's `Id` and what an output publishes — is the
//! identifier the service that owns the resource knows it by, and for a type whose
//! identifier that service MINTS it is the one its create answered with, never a name the
//! template happened to state.** For a hosted zone that identifier is the `Z…` hosted zone
//! ID, which is what `GetHostedZone`, `TestDNSAnswer` and a record set's own `HostedZoneId`
//! take — each a `ResourceId` of at most 32 characters, so a DNS name is not even a legal
//! value for one. Two crates have to agree for that to hold (CloudFormation makes the calls;
//! Route 53 serves them and mints the ids), so the suite lives here rather than in either
//! crate.
//!
//! The rows differ along what the rule generalises over: how many labels the zone name has,
//! how LONG it is — well inside the 32-character `ResourceId` bound, EXACTLY at it, one
//! character past it, and far past it — whether the template states it literally or through a
//! parameter, whether it writes the
//! trailing dot, whether the template is JSON or YAML, which intrinsic each output reads the
//! zone with (`Ref`, `Fn::GetAtt` of the conventional `Id` and of the type's own
//! `HostedZoneId`, `Fn::Sub`, `Fn::Join`), how the record set names its zone (`Ref` to the
//! declared zone, or the zone's DNS name through `HostedZoneName`), the record type and how
//! many values it carries, and whether the zone declares a comment and tags.
//!
//! Every expected value is computed here from what the row declares, except the hosted zone
//! id itself, which is Route 53's to mint: what is checked about THAT is that one value
//! appears everywhere the stack reports the zone's identity, that it has the published form
//! (and so satisfies the modelled 32-character bound), and that Route 53's own
//! `GetHostedZone`, `TestDNSAnswer` and `ListResourceRecordSets` all resolve it.
//!
//! The other half of the same rule is checked too: the zone's DNS NAME is not an identifier
//! Route 53 answers to, so handing it to `GetHostedZone` or `TestDNSAnswer` is refused —
//! past the `ResourceId` bound with `InvalidInput` (400), and inside it with
//! `NoSuchHostedZone` (404), the two modelled errors both operations declare. A refusal that
//! reached the lookup is read for its SENTENCE as well as its code, because that sentence is
//! what tells a caller the stack handed it a name: Route 53's fixed text followed by the
//! value the request asked about.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element and list readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{element, elements, form_encode, opt_element, outputs_of, physical_ids_of};

/// Long enough for the stack's own clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) to
/// come due, so the stack reports the template it deployed.
const PAST_PROVISIONING_SPANS: Duration = Duration::from_millis(900);

/// The most characters a hosted zone id may have: `ResourceId`'s own
/// `@length(min: 0, max: 32)`, the bound every member that takes one is declared with.
const MAX_RESOURCE_ID_CHARS: usize = 32;

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way
/// (the credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
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

/// One output whose value is COMPUTED from the minted zone id: the key, and the value the
/// rule says it must carry once Route 53 has minted that id.
type DerivedOutput = (&'static str, fn(&str) -> String);

/// One row of the table: a template, and what the rule says the Route 53 resources it
/// declares must then be.
struct Row {
    case: &'static str,
    /// The Region the row deploys in; the zone and its records are created there.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: fn() -> String,
    /// The logical id of the declared zone and of the declared record set.
    zone_logical_id: &'static str,
    record_logical_id: &'static str,
    /// The zone's `Name` as Route 53 must report it, and the comment it must carry.
    expected_zone_name: &'static str,
    expected_comment: Option<&'static str>,
    /// The record set the template declares: the `Name` the declaration states (which is
    /// this type's own `Ref` value), the name Route 53 must report, its type, its TTL, and
    /// the values a DNS query for it must be answered with.
    declared_record_name: &'static str,
    expected_record_name: &'static str,
    expected_record_type: &'static str,
    expected_ttl: &'static str,
    expected_record_data: &'static [&'static str],
    /// The output keys whose value must be the minted zone id, each written with a different
    /// intrinsic over the same resource.
    zone_id_outputs: &'static [&'static str],
    /// Outputs whose value is computed from that id.
    derived_outputs: &'static [DerivedOutput],
}

const ROWS: &[Row] = &[
    Row {
        case: "a two-label zone with an apex A record referencing the zone, JSON",
        region: "us-east-1",
        template: || {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "a hosted zone and a record set in it",
                "Resources": {
                    "Zone": {
                        "Type": "AWS::Route53::HostedZone",
                        "Properties": {
                            "Name": "cfn-zone-one.example",
                            "HostedZoneConfig": { "Comment": "deployed by a stack" },
                            "HostedZoneTags": [{ "Key": "Owner", "Value": "networking" }]
                        }
                    },
                    "Record": {
                        "Type": "AWS::Route53::RecordSet",
                        "Properties": {
                            "HostedZoneId": { "Ref": "Zone" },
                            "Name": "cfn-zone-one.example.",
                            "Type": "A",
                            "TTL": "300",
                            "ResourceRecords": ["192.0.2.10"]
                        }
                    }
                },
                "Outputs": {
                    "ZoneId": { "Value": { "Ref": "Zone" } },
                    "ZoneIdAttribute": { "Value": { "Fn::GetAtt": ["Zone", "Id"] } },
                    "ZoneHostedZoneId": { "Value": { "Fn::GetAtt": ["Zone", "HostedZoneId"] } },
                    "ZoneArn": {
                        "Value": {
                            "Fn::Sub": [
                                "arn:aws:route53:::hostedzone/${Id}",
                                { "Id": { "Ref": "Zone" } }
                            ]
                        }
                    }
                }
            })
            .to_string()
        },
        zone_logical_id: "Zone",
        record_logical_id: "Record",
        expected_zone_name: "cfn-zone-one.example.",
        expected_comment: Some("deployed by a stack"),
        declared_record_name: "cfn-zone-one.example.",
        expected_record_name: "cfn-zone-one.example.",
        expected_record_type: "A",
        expected_ttl: "300",
        expected_record_data: &["192.0.2.10"],
        zone_id_outputs: &["ZoneId", "ZoneIdAttribute", "ZoneHostedZoneId"],
        derived_outputs: &[("ZoneArn", |id| {
            format!("arn:aws:route53:::hostedzone/{id}")
        })],
    },
    Row {
        case: "a three-label zone written with the trailing dot, a two-value MX record, YAML",
        region: "eu-west-1",
        template: || {
            [
                "Resources:",
                "  Zone:",
                "    Type: AWS::Route53::HostedZone",
                "    Properties:",
                "      Name: mail.cfn-zone-two.example.",
                "  Record:",
                "    Type: AWS::Route53::RecordSet",
                "    Properties:",
                "      HostedZoneId: !Ref Zone",
                "      Name: smtp.mail.cfn-zone-two.example",
                "      Type: MX",
                "      TTL: 60",
                "      ResourceRecords:",
                "        - 10 inbound.mail.cfn-zone-two.example.",
                "        - 20 backup.mail.cfn-zone-two.example.",
                "Outputs:",
                "  ZoneId:",
                "    Value: !Ref Zone",
                "  ZoneIdAttribute:",
                "    Value: !GetAtt Zone.Id",
                "",
            ]
            .join("\n")
        },
        zone_logical_id: "Zone",
        record_logical_id: "Record",
        expected_zone_name: "mail.cfn-zone-two.example.",
        expected_comment: None,
        // Declared without the trailing dot, reported with it: Route 53 treats the two
        // spellings as the same name and stores the fully-qualified form.
        declared_record_name: "smtp.mail.cfn-zone-two.example",
        expected_record_name: "smtp.mail.cfn-zone-two.example.",
        expected_record_type: "MX",
        expected_ttl: "60",
        expected_record_data: &[
            "10 inbound.mail.cfn-zone-two.example.",
            "20 backup.mail.cfn-zone-two.example.",
        ],
        zone_id_outputs: &["ZoneId", "ZoneIdAttribute"],
        derived_outputs: &[],
    },
    Row {
        case: "a record set naming its zone by DNS name through HostedZoneName, CNAME, JSON",
        region: "ap-northeast-1",
        template: || {
            json!({
                "Resources": {
                    "Zone": {
                        "Type": "AWS::Route53::HostedZone",
                        "Properties": { "Name": "cfn-zone-three.example." }
                    },
                    "Record": {
                        "Type": "AWS::Route53::RecordSet",
                        "Properties": {
                            "HostedZoneName": "cfn-zone-three.example.",
                            "Name": "www.cfn-zone-three.example.",
                            "Type": "CNAME",
                            "TTL": "900",
                            "Comment": "the record the stack declares",
                            "ResourceRecords": ["origin.cfn-zone-three.example."]
                        },
                        "DependsOn": "Zone"
                    }
                },
                "Outputs": {
                    "ZoneId": { "Value": { "Ref": "Zone" } }
                }
            })
            .to_string()
        },
        zone_logical_id: "Zone",
        record_logical_id: "Record",
        expected_zone_name: "cfn-zone-three.example.",
        expected_comment: None,
        declared_record_name: "www.cfn-zone-three.example.",
        expected_record_name: "www.cfn-zone-three.example.",
        expected_record_type: "CNAME",
        expected_ttl: "900",
        expected_record_data: &["origin.cfn-zone-three.example."],
        zone_id_outputs: &["ZoneId"],
        derived_outputs: &[],
    },
    Row {
        case: "a zone whose name is longer than the ResourceId bound, named through a \
               parameter, a two-value AAAA record, JSON",
        region: "us-west-2",
        template: || {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Parameters": {
                    "ZoneName": {
                        "Type": "String",
                        "Default": "cfn-zone-four-beyond-the-id-bound.example."
                    }
                },
                "Resources": {
                    "Zone": {
                        "Type": "AWS::Route53::HostedZone",
                        "Properties": { "Name": { "Ref": "ZoneName" } }
                    },
                    "Record": {
                        "Type": "AWS::Route53::RecordSet",
                        "Properties": {
                            "HostedZoneId": { "Ref": "Zone" },
                            "Name": { "Fn::Sub": "ipv6.${ZoneName}" },
                            "Type": "AAAA",
                            "TTL": "120",
                            "ResourceRecords": ["2001:db8::1", "2001:db8::2"]
                        }
                    }
                },
                "Outputs": {
                    "ZoneId": { "Value": { "Ref": "Zone" } },
                    "ZoneIdAttribute": { "Value": { "Fn::GetAtt": ["Zone", "Id"] } },
                    "ZoneIdPath": {
                        "Value": {
                            "Fn::Join": ["", ["/hostedzone/", { "Ref": "Zone" }]]
                        }
                    }
                }
            })
            .to_string()
        },
        zone_logical_id: "Zone",
        record_logical_id: "Record",
        expected_zone_name: "cfn-zone-four-beyond-the-id-bound.example.",
        expected_comment: None,
        declared_record_name: "ipv6.cfn-zone-four-beyond-the-id-bound.example.",
        expected_record_name: "ipv6.cfn-zone-four-beyond-the-id-bound.example.",
        expected_record_type: "AAAA",
        expected_ttl: "120",
        expected_record_data: &["2001:db8::1", "2001:db8::2"],
        zone_id_outputs: &["ZoneId", "ZoneIdAttribute"],
        derived_outputs: &[("ZoneIdPath", |id| format!("/hostedzone/{id}"))],
    },
    Row {
        case: "a zone past that bound declaring a comment and two tags, a three-value A \
               record, YAML",
        region: "ap-south-1",
        template: || {
            [
                "Resources:",
                "  Zone:",
                "    Type: AWS::Route53::HostedZone",
                "    Properties:",
                "      Name: shard.cfn-zone-five-long.example.",
                "      HostedZoneConfig:",
                "        Comment: the fifth zone",
                "      HostedZoneTags:",
                "        - Key: Environment",
                "          Value: test",
                "        - Key: Owner",
                "          Value: dns",
                "  Record:",
                "    Type: AWS::Route53::RecordSet",
                "    Properties:",
                "      HostedZoneId: !Ref Zone",
                "      Name: api.shard.cfn-zone-five-long.example.",
                "      Type: A",
                "      TTL: 30",
                "      ResourceRecords:",
                "        - 192.0.2.30",
                "        - 192.0.2.31",
                "        - 192.0.2.32",
                "Outputs:",
                "  ZoneId:",
                "    Value: !Ref Zone",
                "  ZoneHostedZoneId:",
                "    Value: !GetAtt Zone.HostedZoneId",
                "",
            ]
            .join("\n")
        },
        zone_logical_id: "Zone",
        record_logical_id: "Record",
        expected_zone_name: "shard.cfn-zone-five-long.example.",
        expected_comment: Some("the fifth zone"),
        declared_record_name: "api.shard.cfn-zone-five-long.example.",
        expected_record_name: "api.shard.cfn-zone-five-long.example.",
        expected_record_type: "A",
        expected_ttl: "30",
        expected_record_data: &["192.0.2.30", "192.0.2.31", "192.0.2.32"],
        zone_id_outputs: &["ZoneId", "ZoneHostedZoneId"],
        derived_outputs: &[],
    },
    Row {
        case: "a zone whose name is inside the ResourceId bound, a subdomain A record \
               referencing the zone, JSON",
        region: "ca-central-1",
        template: || {
            json!({
                "Resources": {
                    "Zone": {
                        "Type": "AWS::Route53::HostedZone",
                        "Properties": { "Name": "cfn-zone-thirty-chars.example." }
                    },
                    "Record": {
                        "Type": "AWS::Route53::RecordSet",
                        "Properties": {
                            "HostedZoneId": { "Ref": "Zone" },
                            "Name": "www.cfn-zone-thirty-chars.example.",
                            "Type": "A",
                            "TTL": "300",
                            "ResourceRecords": ["192.0.2.123"]
                        }
                    }
                },
                "Outputs": {
                    "ZoneId": { "Value": { "Ref": "Zone" } }
                }
            })
            .to_string()
        },
        zone_logical_id: "Zone",
        record_logical_id: "Record",
        expected_zone_name: "cfn-zone-thirty-chars.example.",
        expected_comment: None,
        declared_record_name: "www.cfn-zone-thirty-chars.example.",
        expected_record_name: "www.cfn-zone-thirty-chars.example.",
        expected_record_type: "A",
        expected_ttl: "300",
        expected_record_data: &["192.0.2.123"],
        zone_id_outputs: &["ZoneId"],
        derived_outputs: &[],
    },
    Row {
        case: "a zone whose name is exactly as long as the ResourceId bound, a subdomain \
               CNAME referencing the zone, JSON",
        region: "eu-central-1",
        template: || {
            json!({
                "Resources": {
                    "Zone": {
                        "Type": "AWS::Route53::HostedZone",
                        "Properties": { "Name": "cfn-zone-id-bound-exact.example." }
                    },
                    "Record": {
                        "Type": "AWS::Route53::RecordSet",
                        "Properties": {
                            "HostedZoneId": { "Ref": "Zone" },
                            "Name": "www.cfn-zone-id-bound-exact.example.",
                            "Type": "CNAME",
                            "TTL": "600",
                            "ResourceRecords": ["origin.cfn-zone-id-bound-exact.example."]
                        }
                    }
                },
                "Outputs": {
                    "ZoneId": { "Value": { "Ref": "Zone" } },
                    "ZoneIdAttribute": { "Value": { "Fn::GetAtt": ["Zone", "Id"] } }
                }
            })
            .to_string()
        },
        zone_logical_id: "Zone",
        record_logical_id: "Record",
        expected_zone_name: "cfn-zone-id-bound-exact.example.",
        expected_comment: None,
        declared_record_name: "www.cfn-zone-id-bound-exact.example.",
        expected_record_name: "www.cfn-zone-id-bound-exact.example.",
        expected_record_type: "CNAME",
        expected_ttl: "600",
        expected_record_data: &["origin.cfn-zone-id-bound-exact.example."],
        zone_id_outputs: &["ZoneId", "ZoneIdAttribute"],
        derived_outputs: &[],
    },
    Row {
        case: "a zone whose name is one character past the ResourceId bound, a two-value A \
               record, YAML",
        region: "sa-east-1",
        template: || {
            [
                "Resources:",
                "  Zone:",
                "    Type: AWS::Route53::HostedZone",
                "    Properties:",
                "      Name: cfn-zone-id-bound-exceed.example.",
                "  Record:",
                "    Type: AWS::Route53::RecordSet",
                "    Properties:",
                "      HostedZoneId: !Ref Zone",
                "      Name: api.cfn-zone-id-bound-exceed.example.",
                "      Type: A",
                "      TTL: 300",
                "      ResourceRecords:",
                "        - 192.0.2.40",
                "        - 192.0.2.41",
                "Outputs:",
                "  ZoneId:",
                "    Value: !Ref Zone",
                "",
            ]
            .join("\n")
        },
        zone_logical_id: "Zone",
        record_logical_id: "Record",
        expected_zone_name: "cfn-zone-id-bound-exceed.example.",
        expected_comment: None,
        declared_record_name: "api.cfn-zone-id-bound-exceed.example.",
        expected_record_name: "api.cfn-zone-id-bound-exceed.example.",
        expected_record_type: "A",
        expected_ttl: "300",
        expected_record_data: &["192.0.2.40", "192.0.2.41"],
        zone_id_outputs: &["ZoneId"],
        derived_outputs: &[],
    },
];

/// Every row: the value the stack reports for its hosted zone is the id Route 53 minted,
/// that id resolves the zone and the record set the template declared through Route 53's own
/// reads, and deleting the stack deletes both there.
#[tokio::test]
async fn a_stack_publishes_the_hosted_zone_id_route53_minted() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("route53-zone-stack-{index}");
        let template = (row.template)();

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", &stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        tokio::time::sleep(PAST_PROVISIONING_SPANS).await;
        let (status, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {described}"
        );
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );

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

        // The id the stack reports for the zone: Route 53's to mint, so what is checked
        // about it is its published FORM and the bound every member that takes one carries.
        let zone_id = physical_ids
            .get(row.zone_logical_id)
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the stack reports no resource {:?}: {resources}", row.zone_logical_id)
            });
        assert!(
            is_hosted_zone_id(&zone_id),
            "case {case:?}: expected the zone's physical id to be a Route 53 hosted zone id \
             (`Z` and uppercase alphanumerics, at most {MAX_RESOURCE_ID_CHARS} characters), \
             got {zone_id:?}"
        );
        for key in row.zone_id_outputs {
            assert_eq!(
                outputs.get(*key),
                Some(&zone_id),
                "case {case:?}: expected output {key:?} to be the minted zone id {zone_id:?}, \
                 got {:?}",
                outputs.get(*key)
            );
        }
        for (key, expected) in row.derived_outputs {
            let expected = expected(&zone_id);
            assert_eq!(
                outputs.get(*key),
                Some(&expected),
                "case {case:?}: expected output {key:?} to be {expected:?}, got {:?}",
                outputs.get(*key)
            );
        }

        // Route 53 itself resolves that id to the zone the template declared — the very call
        // a client makes with the stack's output.
        let (status, zone) = route53(
            &router,
            region,
            "GET",
            &format!("/2013-04-01/hostedzone/{zone_id}"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetHostedZone on the id the stack reported, \
             got {status} with body {zone}"
        );
        assert_eq!(
            opt_element(&zone, "Name").as_deref(),
            Some(row.expected_zone_name),
            "case {case:?}: expected the declared zone name, got {zone}"
        );
        assert_eq!(
            opt_element(&zone, "Comment").as_deref(),
            row.expected_comment,
            "case {case:?}: expected the declared comment {:?}, got {zone}",
            row.expected_comment
        );
        assert_eq!(
            opt_element(&zone, "Id").as_deref(),
            Some(format!("/hostedzone/{zone_id}").as_str()),
            "case {case:?}: expected the zone to report the id the stack published, got {zone}"
        );

        // The record set the template declared is in THAT zone, with the values it declared.
        let (status, records) = route53(
            &router,
            region,
            "GET",
            &format!(
                "/2013-04-01/hostedzone/{zone_id}/rrset?name={}&type={}&maxitems=1",
                form_encode(row.expected_record_name),
                row.expected_record_type
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListResourceRecordSets, got {status} with \
             body {records}"
        );
        assert_eq!(
            opt_element(&records, "Name").as_deref(),
            Some(row.expected_record_name),
            "case {case:?}: expected the zone to hold the declared record name, got {records}"
        );
        assert_eq!(
            opt_element(&records, "Type").as_deref(),
            Some(row.expected_record_type),
            "case {case:?}: expected the declared record type, got {records}"
        );
        assert_eq!(
            opt_element(&records, "TTL").as_deref(),
            Some(row.expected_ttl),
            "case {case:?}: expected the declared TTL, got {records}"
        );
        let values = elements(&records, "Value");
        assert_eq!(
            values, row.expected_record_data,
            "case {case:?}: expected the declared record values {:?}, got {records}",
            row.expected_record_data
        );
        // And the stack reports the record set under the name its declaration states, with
        // the terminal dot dropped: "Ref returns the record name", and CloudFormation reports
        // a DNS record name in the form a client uses it in rather than in the
        // fully-qualified form Route 53 compares names in.
        let expected_ref = row.declared_record_name.trim_end_matches('.');
        assert_eq!(
            physical_ids.get(row.record_logical_id).map(String::as_str),
            Some(expected_ref),
            "case {case:?}: expected the record set's physical id to be the record name it \
             declared without its terminal dot ({expected_ref:?}), got {:?} in {resources}",
            physical_ids.get(row.record_logical_id)
        );

        // A DNS query for that name through the stack's own output is answered from the
        // record the stack created.
        let (status, answer) = route53(
            &router,
            region,
            "GET",
            &format!(
                "/2013-04-01/testdnsanswer?hostedzoneid={zone_id}&recordname={}&recordtype={}",
                form_encode(row.expected_record_name),
                row.expected_record_type
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from TestDNSAnswer on the id the stack reported, \
             got {status} with body {answer}"
        );
        assert_eq!(
            opt_element(&answer, "ResponseCode").as_deref(),
            Some("NOERROR"),
            "case {case:?}: expected the query to be answered NOERROR, got {answer}"
        );
        let answered = elements(&answer, "RecordDataEntry");
        assert_eq!(
            answered, row.expected_record_data,
            "case {case:?}: expected the declared record values {:?} in the DNS answer, got \
             {answer}",
            row.expected_record_data
        );

        // The other half of the rule: the zone's DNS NAME is not an identifier either
        // operation answers to. Which refusal it draws follows from the modelled bound
        // alone — a name past `ResourceId`'s 32 characters cannot be a legal value at all
        // (`InvalidInput`, 400), and one inside it names no zone (`NoSuchHostedZone`, 404) —
        // so both classes are exercised by the rows above.
        let (expected_status, expected_code) =
            if row.expected_zone_name.len() > MAX_RESOURCE_ID_CHARS {
                (400, "InvalidInput")
            } else {
                (404, "NoSuchHostedZone")
            };
        // A refusal that got as far as the LOOKUP names the zone it looked for, in the one
        // sentence Route 53 words a missing zone in: the fixed text, then the value the
        // request sent as the id. That sentence is how a caller sees that the stack handed it
        // a DNS name rather than an id, so it is asserted and not just the code. A value past
        // the modelled bound never reaches the lookup, so it has no such sentence.
        let expected_message = (expected_code == "NoSuchHostedZone")
            .then(|| format!("No hosted zone found with ID: {}", row.expected_zone_name));
        let (status, refused) = route53(
            &router,
            region,
            "GET",
            &format!("/2013-04-01/hostedzone/{}", row.expected_zone_name),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from GetHostedZone on the zone's \
             DNS name {:?}, got {status} with body {refused}",
            row.expected_zone_name
        );
        assert_eq!(
            element(&refused, "Code"),
            expected_code,
            "case {case:?}: expected {expected_code} from GetHostedZone on the zone's DNS \
             name, got {refused}"
        );
        if let Some(expected) = &expected_message {
            let reported = element(&refused, "Message");
            assert_eq!(
                &reported, expected,
                "case {case:?}: expected GetHostedZone to refuse the zone's DNS name with \
                 {expected:?}, got {reported:?}"
            );
        }
        let (status, refused) = route53(
            &router,
            region,
            "GET",
            &format!(
                "/2013-04-01/testdnsanswer?hostedzoneid={}&recordname={}&recordtype={}",
                form_encode(row.expected_zone_name),
                form_encode(row.expected_record_name),
                row.expected_record_type
            ),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from TestDNSAnswer on the zone's \
             DNS name {:?}, got {status} with body {refused}",
            row.expected_zone_name
        );
        assert_eq!(
            element(&refused, "Code"),
            expected_code,
            "case {case:?}: expected {expected_code} from TestDNSAnswer on the zone's DNS \
             name, got {refused}"
        );
        if let Some(expected) = &expected_message {
            let reported = element(&refused, "Message");
            assert_eq!(
                &reported, expected,
                "case {case:?}: expected TestDNSAnswer to refuse the zone's DNS name with \
                 {expected:?}, got {reported:?}"
            );
        }

        // Deleting the stack deletes the record set and then the zone in Route 53.
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
            &format!("/2013-04-01/hostedzone/{zone_id}"),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 from GetHostedZone once the stack deleted the \
             zone, got {status} with body {gone}"
        );
        assert_eq!(
            element(&gone, "Code"),
            "NoSuchHostedZone",
            "case {case:?}: expected the zone to be gone, got {gone}"
        );
    }
}

/// A record set whose declaration states no zone at all names no zone to write the record
/// in: CloudFormation fails the resource rather than reporting a record set nothing created,
/// and the zone it was deployed beside is rolled back with it.
#[tokio::test]
async fn a_record_set_naming_no_zone_fails_the_resource() {
    let router = build_router();
    let region = "us-west-2";
    let template = json!({
        "Resources": {
            "Record": {
                "Type": "AWS::Route53::RecordSet",
                "Properties": {
                    "Name": "orphan.cfn-zone-four.example.",
                    "Type": "A",
                    "TTL": "300",
                    "ResourceRecords": ["192.0.2.20"]
                }
            }
        }
    })
    .to_string();

    let (status, body) = cloudformation(
        &router,
        region,
        "CreateStack",
        &[("StackName", "route53-orphan-record"), ("TemplateBody", &template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_PROVISIONING_SPANS).await;
    let (status, described) = cloudformation(
        &router,
        region,
        "DescribeStacks",
        &[("StackName", "route53-orphan-record")],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from DescribeStacks, got {status} with body {described}"
    );
    let reported = opt_element(&described, "StackStatus").unwrap_or_default();
    assert_eq!(
        reported, "ROLLBACK_COMPLETE",
        "case \"a record set naming no zone\": expected the create to roll back, got \
         {reported:?} in {described}"
    );
    // And the resource's own event carries the provider refusal, naming the two properties a
    // record set can name its zone with — the reason CloudFormation reports for a resource a
    // provider would not act on.
    let (status, events) = cloudformation(
        &router,
        region,
        "DescribeStackEvents",
        &[("StackName", "route53-orphan-record")],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from DescribeStackEvents, got {status} with body {events}"
    );
    let failed = elements(&events, "ResourceStatusReason")
        .into_iter()
        .find(|reason| reason.contains("HostedZoneId"));
    let failed = failed.unwrap_or_else(|| {
        panic!(
            "case \"a record set naming no zone\": expected an event naming the missing \
             HostedZoneId, got {events}"
        )
    });
    assert!(
        failed.contains("HostedZoneName") && failed.contains("AWS::Route53::RecordSet"),
        "case \"a record set naming no zone\": expected the refusal to name the type and both \
         zone properties, got {failed:?}"
    );
}

/// Whether `id` is a hosted zone id of the form Route 53 publishes: `Z` followed by
/// uppercase alphanumerics, inside the 32-character bound `ResourceId` fixes.
fn is_hosted_zone_id(id: &str) -> bool {
    let mut characters = id.chars();
    characters.next() == Some('Z')
        && id.len() > 1
        && id.len() <= MAX_RESOURCE_ID_CHARS
        && characters.all(|character| {
            character.is_ascii_uppercase() || character.is_ascii_digit()
        })
}

/// The table really samples the `ResourceId` bound the refusal half of the rule turns on: a
/// zone name comfortably inside it, one exactly as long as it, and one a single character
/// past it. Held here so a later edit to a row's name cannot quietly drop a boundary case.
#[test]
fn the_rows_sample_the_resource_id_bound() {
    let lengths: Vec<usize> = ROWS
        .iter()
        .map(|row| row.expected_zone_name.len())
        .collect();
    for (case, expected) in [
        ("inside the bound", MAX_RESOURCE_ID_CHARS - 2),
        ("exactly at the bound", MAX_RESOURCE_ID_CHARS),
        ("one character past the bound", MAX_RESOURCE_ID_CHARS + 1),
    ] {
        assert!(
            lengths.contains(&expected),
            "case {case:?}: expected a row whose zone name is {expected} characters long, \
             got {lengths:?}"
        );
    }
}

/// The suite's own helper, held to the forms the rule is about: a minted id passes, a DNS
/// name — the value this issue was filed about — does not.
#[test]
fn the_zone_id_form_admits_only_an_id() {
    // (case, the value, whether it is a hosted zone id)
    let rows: [(&str, &str, bool); 5] = [
        ("an id Route 53 publishes", "Z1D633PJN98FT9", true),
        ("a minted id of this emulator's length", "Z0123456789ABCD", true),
        ("a zone DNS name", "cfn-zone-one.example.", false),
        ("a name that merely starts with Z", "Zone.example.", false),
        ("the prefixed form a read reports", "/hostedzone/Z1D633PJN98FT9", false),
    ];
    for (case, value, expected) in rows {
        let read = is_hosted_zone_id(value);
        assert_eq!(
            read, expected,
            "case {case:?}: expected is_hosted_zone_id({value:?}) to be {expected}, got {read}"
        );
    }
}
