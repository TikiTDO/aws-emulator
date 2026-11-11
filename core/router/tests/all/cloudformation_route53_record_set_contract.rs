// SPDX-License-Identifier: CC0-1.0

//! What a CloudFormation stack reports for the Amazon Route 53 resources it declares beyond
//! their existence: the VALUES a hosted zone and a record set hand back, the declarations the
//! service refuses before provisioning anything, and the words it reports a refusal of the
//! owning service in.
//!
//! Three rules are under test, one per section:
//!
//! * **An output whose value a stack can resolve carries that value, and an attribute whose
//!   documented return value is a LIST resolves to that list** — so the functions that read a
//!   list (`Fn::Join`, `Fn::Select`) read a hosted zone's `NameServers` the way they read any
//!   other list, and no `Outputs` entry of a `CREATE_COMPLETE` stack is reported without its
//!   `OutputValue`.
//! * **A resource whose declaration satisfies more than one subschema of a `oneOf` its type's
//!   schema states is refused BEFORE provisioning**: the operation fails pre-deployment, the
//!   stack ends `ROLLBACK_COMPLETE`, `DescribeEvents` records the validation that failed, and
//!   no resource of the stack is created. `AWS::Route53::RecordSet` and
//!   `AWS::Route53::RecordSetGroup` each publish `HostedZoneId` and `HostedZoneName` with
//!   "specify either `HostedZoneName` or `HostedZoneId`, but not both".
//! * **A resource provider reports the refusal of the service it called as that service's own
//!   SDK raises it** — the service's message followed by the SDK's diagnostic of the call —
//!   **and classifies it by what the refusal is ABOUT**: a zone the declaration NAMES that
//!   Route 53 does not hold is a fault in the request (`InvalidRequest`), not the record set
//!   the handler manages going missing.
//!
//! Each section's rows differ along what its rule generalises over: how the declaration writes
//! a DNS name, which type declares the mutually exclusive pair, and which (syntactically
//! valid) zone id the record names.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element and list readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{
    elements, form_encode, members_of, opt_element, outputs_of, physical_ids_of,
};

/// Long enough for the stack's own clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) to
/// come due, so the stack reports the template it deployed.
const PAST_PROVISIONING_SPANS: Duration = Duration::from_millis(900);

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them.
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

/// Create a stack, wait for its clock-owed span and answer its `DescribeStacks` body.
async fn deploy(
    router: &MultiServiceRouter,
    region: &str,
    stack_name: &str,
    template: &str,
    case: &str,
) -> String {
    let (status, body) = cloudformation(
        router,
        region,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_PROVISIONING_SPANS).await;
    let (status, described) =
        cloudformation(router, region, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {described}"
    );
    described
}

// ---------------------------------------------------------------------------
// A hosted zone's NameServers reach the stack's outputs
// ---------------------------------------------------------------------------

/// One row of the name-servers table: how the zone's name is written, and the Region the
/// stack deploys in.
struct ZoneRow {
    case: &'static str,
    region: &'static str,
    zone_name: &'static str,
    /// The zone's `Name` as Route 53 must report it, which is the declared name fully
    /// qualified.
    reported_name: &'static str,
}

const ZONE_ROWS: &[ZoneRow] = &[
    ZoneRow {
        case: "a two-label zone written with the trailing dot",
        region: "us-east-1",
        zone_name: "cfn-ns-one.example.",
        reported_name: "cfn-ns-one.example.",
    },
    ZoneRow {
        case: "a two-label zone written without it",
        region: "eu-west-1",
        zone_name: "cfn-ns-two.example",
        reported_name: "cfn-ns-two.example.",
    },
    ZoneRow {
        case: "a four-label zone",
        region: "ap-southeast-2",
        zone_name: "a.b.cfn-ns-three.example.",
        reported_name: "a.b.cfn-ns-three.example.",
    },
];

/// Every row: the stack's outputs carry a VALUE for every entry its template declares, and
/// the entries that read the zone's `NameServers` carry the name servers Route 53 delegated
/// the zone to.
#[tokio::test]
async fn a_hosted_zones_name_servers_reach_the_stacks_outputs() {
    let router = build_router();

    for (index, row) in ZONE_ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("route53-name-servers-{index}");
        let template = json!({
            "Resources": {
                "Zone": {
                    "Type": "AWS::Route53::HostedZone",
                    "Properties": {
                        "Name": row.zone_name,
                        "HostedZoneConfig": { "Comment": "a zone with a delegation" }
                    }
                }
            },
            "Outputs": {
                "ZoneId": { "Value": { "Ref": "Zone" } },
                "NameServers": {
                    "Value": { "Fn::Join": [",", { "Fn::GetAtt": ["Zone", "NameServers"] }] }
                },
                "FirstNameServer": {
                    "Value": { "Fn::Select": [0, { "Fn::GetAtt": ["Zone", "NameServers"] }] }
                },
                "LastNameServer": {
                    "Value": { "Fn::Select": [3, { "Fn::GetAtt": ["Zone", "NameServers"] }] }
                }
            }
        })
        .to_string();

        let described = deploy(&router, region, &stack_name, &template, case).await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );

        // EVERY declared output is reported WITH a value: the keys the template declares and
        // the keys the answer pairs with a value are the same set.
        let outputs = outputs_of(&described);
        let declared_keys = elements(&described, "OutputKey");
        let valued_keys: Vec<String> = outputs.keys().cloned().collect();
        let mut sorted = declared_keys.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            valued_keys, sorted,
            "case {case:?}: expected every declared output to carry an OutputValue \
             (declared {sorted:?}), got {valued_keys:?} in {described}"
        );

        // The zone Route 53 created reports the delegation it made, and the outputs that read
        // the type's `NameServers` attribute carry exactly those name servers.
        let zone_id = outputs.get("ZoneId").cloned().unwrap_or_else(|| {
            panic!("case {case:?}: the stack reports no ZoneId output: {described}")
        });
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
            Some(row.reported_name),
            "case {case:?}: expected the declared zone name, got {zone}"
        );
        let name_servers = elements(&zone, "NameServer");
        assert!(
            name_servers.len() > 1,
            "case {case:?}: expected Route 53 to delegate the zone to several name servers, \
             got {zone}"
        );
        let joined = name_servers.join(",");
        assert_eq!(
            outputs.get("NameServers"),
            Some(&joined),
            "case {case:?}: expected output \"NameServers\" to be the zone's name servers \
             joined ({joined:?}), got {:?}",
            outputs.get("NameServers")
        );
        assert_eq!(
            outputs.get("FirstNameServer"),
            name_servers.first(),
            "case {case:?}: expected output \"FirstNameServer\" to be {:?}, got {:?}",
            name_servers.first(),
            outputs.get("FirstNameServer")
        );
        assert_eq!(
            outputs.get("LastNameServer"),
            name_servers.get(3),
            "case {case:?}: expected output \"LastNameServer\" to be {:?}, got {:?}",
            name_servers.get(3),
            outputs.get("LastNameServer")
        );
    }
}

// ---------------------------------------------------------------------------
// A record set's Ref is its name without the terminal dot
// ---------------------------------------------------------------------------

/// One row of the record-name table: the `Name` the declaration states, and the name Route 53
/// must hold the record under.
struct RecordRow {
    case: &'static str,
    zone_name: &'static str,
    declared_name: &'static str,
    held_name: &'static str,
    record_type: &'static str,
    record_values: &'static [&'static str],
}

const RECORD_ROWS: &[RecordRow] = &[
    RecordRow {
        case: "a subdomain written with the trailing dot",
        zone_name: "cfn-ref-one.example.",
        declared_name: "txt.cfn-ref-one.example.",
        held_name: "txt.cfn-ref-one.example.",
        record_type: "TXT",
        record_values: &["\"hello\""],
    },
    RecordRow {
        case: "a subdomain written without it",
        zone_name: "cfn-ref-two.example.",
        declared_name: "www.cfn-ref-two.example",
        held_name: "www.cfn-ref-two.example.",
        record_type: "A",
        record_values: &["192.0.2.20"],
    },
    RecordRow {
        case: "the zone apex itself, with the dot",
        zone_name: "cfn-ref-three.example.",
        declared_name: "cfn-ref-three.example.",
        held_name: "cfn-ref-three.example.",
        record_type: "A",
        record_values: &["192.0.2.30"],
    },
    RecordRow {
        case: "a four-label name with two values",
        zone_name: "cfn-ref-four.example.",
        declared_name: "a.b.cfn-ref-four.example.",
        held_name: "a.b.cfn-ref-four.example.",
        record_type: "MX",
        record_values: &["10 in.cfn-ref-four.example.", "20 bk.cfn-ref-four.example."],
    },
];

/// Every row: the value the stack hands back for the record set is its declared name with the
/// terminal dot dropped, while Route 53 holds the record under the fully-qualified name.
#[tokio::test]
async fn a_record_sets_ref_is_its_name_without_the_terminal_dot() {
    let router = build_router();
    let region = "us-west-2";

    for (index, row) in RECORD_ROWS.iter().enumerate() {
        let case = row.case;
        let stack_name = format!("route53-record-ref-{index}");
        let template = json!({
            "Resources": {
                "Zone": {
                    "Type": "AWS::Route53::HostedZone",
                    "Properties": { "Name": row.zone_name }
                },
                "Rec": {
                    "Type": "AWS::Route53::RecordSet",
                    "Properties": {
                        "HostedZoneId": { "Ref": "Zone" },
                        "Name": row.declared_name,
                        "Type": row.record_type,
                        "TTL": "300",
                        "ResourceRecords": row.record_values
                    }
                }
            },
            "Outputs": {
                "RecRef": { "Value": { "Ref": "Rec" } },
                "ZoneId": { "Value": { "Ref": "Zone" } }
            }
        })
        .to_string();

        let described = deploy(&router, region, &stack_name, &template, case).await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );
        let outputs = outputs_of(&described);
        // The rule: the record's name WITHOUT its terminal dot, however the declaration
        // wrote it.
        let expected_ref = row.declared_name.trim_end_matches('.').to_string();
        assert_eq!(
            outputs.get("RecRef"),
            Some(&expected_ref),
            "case {case:?}: expected the record set's Ref to be {expected_ref:?}, got {:?} in \
             {described}",
            outputs.get("RecRef")
        );

        // The same value is the resource's physical id, which is what `Ref` resolves to.
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
        assert_eq!(
            physical_ids_of(&resources).get("Rec"),
            Some(&expected_ref),
            "case {case:?}: expected the record set's physical id to be {expected_ref:?}, got \
             {resources}"
        );

        // ...while Route 53 holds the record under the fully-qualified name, which is how a
        // DNS query for it is answered.
        let zone_id = outputs.get("ZoneId").cloned().unwrap_or_else(|| {
            panic!("case {case:?}: the stack reports no ZoneId output: {described}")
        });
        let (status, records) = route53(
            &router,
            region,
            "GET",
            &format!(
                "/2013-04-01/hostedzone/{zone_id}/rrset?name={}&type={}&maxitems=1",
                form_encode(row.held_name),
                row.record_type
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
            Some(row.held_name),
            "case {case:?}: expected the zone to hold the record under its fully-qualified \
             name, got {records}"
        );
        assert_eq!(
            elements(&records, "Value"),
            row.record_values,
            "case {case:?}: expected the declared record values {:?}, got {records}",
            row.record_values
        );
    }
}

// ---------------------------------------------------------------------------
// Both members of a mutually exclusive pair are refused before provisioning
// ---------------------------------------------------------------------------

/// The reason CloudFormation's schema validation reports for a declaration that satisfies
/// more than one subschema of a `oneOf`: its fixed sentence, whose one variable part is HOW
/// MANY of them the document matched — two, for a pair.
const TWO_SUBSCHEMAS_MATCHED: &str = "#: #: 2 subschemas matched instead of one";

/// One row of the mutual-exclusion table: the type that declares both members, and the
/// resource's logical id in the template.
struct ExclusiveRow {
    case: &'static str,
    resource_type: &'static str,
    logical_id: &'static str,
    /// The further resources the template declares beside it, which the refusal means are
    /// never created either.
    declares_a_zone: bool,
    /// The properties the declaration carries beside the two zone members.
    properties: fn() -> serde_json::Value,
}

const EXCLUSIVE_ROWS: &[ExclusiveRow] = &[
    ExclusiveRow {
        case: "a record set naming its zone both ways, on its own",
        resource_type: "AWS::Route53::RecordSet",
        logical_id: "R",
        declares_a_zone: false,
        properties: || {
            json!({
                "Name": "both.cfn-exclusive.example.",
                "Type": "A",
                "TTL": "300",
                "ResourceRecords": ["192.0.2.40"]
            })
        },
    },
    ExclusiveRow {
        case: "the same, beside a zone of the same template",
        resource_type: "AWS::Route53::RecordSet",
        logical_id: "Record",
        declares_a_zone: true,
        properties: || {
            json!({
                "Name": "both.cfn-exclusive.example.",
                "Type": "TXT",
                "TTL": "60",
                "ResourceRecords": ["\"both\""]
            })
        },
    },
    ExclusiveRow {
        case: "a record set GROUP naming its zone both ways",
        resource_type: "AWS::Route53::RecordSetGroup",
        logical_id: "Group",
        declares_a_zone: false,
        properties: || {
            json!({
                "RecordSets": [{
                    "Name": "group.cfn-exclusive.example.",
                    "Type": "A",
                    "TTL": "300",
                    "ResourceRecords": ["192.0.2.41"]
                }]
            })
        },
    },
];

/// Every row: the declaration carrying both members of the pair is refused before anything is
/// provisioned — the stack rolls back, the validation that failed is on the record, and
/// neither the resource nor anything else the template declared was created.
#[tokio::test]
async fn a_declaration_matching_two_subschemas_is_refused_before_provisioning() {
    let router = build_router();
    let region = "eu-central-1";

    for (index, row) in EXCLUSIVE_ROWS.iter().enumerate() {
        let case = row.case;
        let stack_name = format!("route53-exclusive-{index}");
        let mut properties = (row.properties)();
        let members = properties
            .as_object_mut()
            .expect("the row's properties are an object");
        members.insert(
            "HostedZoneId".to_string(),
            json!("Z00000000000000EXAMPLE"),
        );
        members.insert(
            "HostedZoneName".to_string(),
            json!("cfn-exclusive.example."),
        );
        let mut resources = json!({
            row.logical_id: { "Type": row.resource_type, "Properties": properties }
        });
        if row.declares_a_zone {
            resources
                .as_object_mut()
                .expect("the resources are an object")
                .insert(
                    "Zone".to_string(),
                    json!({
                        "Type": "AWS::Route53::HostedZone",
                        "Properties": { "Name": "cfn-exclusive.example." }
                    }),
                );
        }
        let template = json!({ "Resources": resources }).to_string();

        let described = deploy(&router, region, &stack_name, &template, case).await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("ROLLBACK_COMPLETE"),
            "case {case:?}: expected the create to roll back, got {described}"
        );

        // `DescribeEvents` records exactly one VALIDATION_ERROR event for the resource, with
        // the schema validator's own sentence and the path into the submitted template.
        let (status, events) = cloudformation(
            &router,
            region,
            "DescribeEvents",
            &[("StackName", &stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeEvents, got {status} with body \
             {events}"
        );
        let validation: Vec<&str> = members_of(&events)
            .into_iter()
            .filter(|event| elements(event, "EventType") == vec!["VALIDATION_ERROR".to_string()])
            .collect();
        assert_eq!(
            validation.len(),
            1,
            "case {case:?}: expected exactly one VALIDATION_ERROR event, got {events}"
        );
        let reported = validation[0];
        assert_eq!(
            opt_element(reported, "LogicalResourceId").as_deref(),
            Some(row.logical_id),
            "case {case:?}: expected the event to be about the resource that declared both \
             members, got {reported}"
        );
        assert_eq!(
            elements(reported, "ValidationStatusReason"),
            vec![TWO_SUBSCHEMAS_MATCHED.to_string()],
            "case {case:?}: expected the reason {TWO_SUBSCHEMAS_MATCHED:?}, got {reported}"
        );
        let expected_path = format!("/Resources/{}/Properties", row.logical_id);
        assert_eq!(
            opt_element(reported, "ValidationPath").as_deref(),
            Some(expected_path.as_str()),
            "case {case:?}: expected the path {expected_path:?}, got {reported}"
        );

        // Nothing was provisioned: the stack lists no resource at all, so neither the record
        // nor a zone the same template declared exists.
        let (status, listed) = cloudformation(
            &router,
            region,
            "ListStackResources",
            &[("StackName", &stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListStackResources, got {status} with body \
             {listed}"
        );
        assert!(
            elements(&listed, "LogicalResourceId").is_empty(),
            "case {case:?}: expected a rolled-back stack to list no resources, got {listed}"
        );
        let (status, zones) = route53(
            &router,
            region,
            "GET",
            "/2013-04-01/hostedzonesbyname?dnsname=cfn-exclusive.example.&maxitems=1",
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListHostedZonesByName, got {status} with \
             body {zones}"
        );
        assert!(
            !elements(&zones, "Name").contains(&"cfn-exclusive.example.".to_string()),
            "case {case:?}: expected no zone to have been created, got {zones}"
        );
    }
}

/// The control: a declaration carrying exactly ONE of the two members satisfies the `oneOf`
/// and deploys — by the zone's id, and by the zone's DNS name.
#[tokio::test]
async fn a_declaration_naming_its_zone_one_way_deploys() {
    let router = build_router();
    let region = "ca-central-1";
    // (case, which member the record declares)
    for (index, (case, by_name)) in [
        ("the record names its zone by id", false),
        ("the record names its zone by DNS name", true),
    ]
    .into_iter()
    .enumerate()
    {
        let zone_name = format!("cfn-oneof-{index}.example.");
        let stack_name = format!("route53-oneof-{index}");
        let zone_member = if by_name {
            json!({ "HostedZoneName": zone_name.clone() })
        } else {
            json!({ "HostedZoneId": { "Ref": "Zone" } })
        };
        let mut properties = json!({
            "Name": format!("one.cfn-oneof-{index}.example."),
            "Type": "A",
            "TTL": "300",
            "ResourceRecords": ["192.0.2.50"]
        });
        let members = properties.as_object_mut().expect("an object");
        for (key, value) in zone_member.as_object().expect("an object") {
            members.insert(key.clone(), value.clone());
        }
        let template = json!({
            "Resources": {
                "Zone": {
                    "Type": "AWS::Route53::HostedZone",
                    "Properties": { "Name": zone_name }
                },
                "Record": {
                    "Type": "AWS::Route53::RecordSet",
                    "DependsOn": "Zone",
                    "Properties": properties
                }
            }
        })
        .to_string();

        let described = deploy(&router, region, &stack_name, &template, case).await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );
    }
}

// ---------------------------------------------------------------------------
// The refusal of the owning service, as its own SDK raises it
// ---------------------------------------------------------------------------

/// The fixed text Route 53 words a zone it does not hold in, whose one variable part is the
/// zone id the request named.
const MISSING_ZONE_PREFIX: &str = "No hosted zone found with ID: ";

/// The SDK diagnostic of the refused call, as the SDK of the called service appends it: which
/// service refused it and the status it answered, then the id of the call and how many
/// attempts were made. The ids are per-run; the clause's shape is not, so the fragments
/// around them are what the rule fixes.
const SDK_SERVICE_CLAUSE: &str = "(Service: Route53, Status Code: 404, Request ID: ";
const SDK_ATTEMPT_CLAUSE: &str = ") (SDK Attempt Count: 1)";

/// The class CloudFormation reports the refusal under: the declaration named a zone that is
/// not there, which is a fault in the request rather than the record set this handler manages
/// having gone missing.
const HANDLER_ERROR_CLASS: &str = "HandlerErrorCode: InvalidRequest";

/// Every row: a record set naming a zone Route 53 does not hold fails with that service's own
/// message and the SDK diagnostic of the call, classified as a fault in the request.
#[tokio::test]
async fn a_record_set_naming_a_missing_zone_reports_route53s_own_refusal() {
    let router = build_router();
    let region = "us-east-2";
    // Syntactically valid zone ids — `ResourceId` admits up to 32 characters — that no zone
    // of the account carries, in the spellings a template can state one in.
    let rows: [(&str, &str); 3] = [
        ("a zone id of the published form", "Z0000000000000NOTREAL"),
        ("a shorter one", "ZNOTREAL"),
        ("the prefixed form a read hands back", "/hostedzone/Z0000000000000ABSENT"),
    ];

    for (index, (case, declared_zone)) in rows.into_iter().enumerate() {
        let stack_name = format!("route53-missing-zone-{index}");
        let template = json!({
            "Resources": {
                "Record": {
                    "Type": "AWS::Route53::RecordSet",
                    "Properties": {
                        "HostedZoneId": declared_zone,
                        "Name": format!("absent-{index}.cfn-missing.example."),
                        "Type": "A",
                        "TTL": "300",
                        "ResourceRecords": ["192.0.2.60"]
                    }
                }
            }
        })
        .to_string();

        let described = deploy(&router, region, &stack_name, &template, case).await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("ROLLBACK_COMPLETE"),
            "case {case:?}: expected the create to roll back, got {described}"
        );

        let (status, events) = cloudformation(
            &router,
            region,
            "DescribeStackEvents",
            &[("StackName", &stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackEvents, got {status} with body \
             {events}"
        );
        let reason = elements(&events, "ResourceStatusReason")
            .into_iter()
            .find(|reason| reason.contains(MISSING_ZONE_PREFIX))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected an event carrying Route 53's own refusal, got \
                     {events}"
                )
            });
        // Route 53 reports the zone it looked for by its BARE id, whichever spelling the
        // declaration used, and that half of the message comes from the request.
        let bare = declared_zone
            .strip_prefix("/hostedzone/")
            .unwrap_or(declared_zone);
        let expected_message = format!("{MISSING_ZONE_PREFIX}{bare}");
        assert!(
            reason.contains(&expected_message),
            "case {case:?}: expected the reason to carry {expected_message:?}, got {reason:?}"
        );
        // The SDK of the called service raises its exception with the diagnostic of the call:
        // which service refused it, the HTTP status it answered, the id of the call and how
        // many attempts were made. The ids are per-run; the clause's shape is not.
        for fragment in [SDK_SERVICE_CLAUSE, SDK_ATTEMPT_CLAUSE] {
            assert!(
                reason.contains(fragment),
                "case {case:?}: expected the reason to carry the SDK clause {fragment:?}, got \
                 {reason:?}"
            );
        }
        // ...and the class is a fault in the REQUEST: the zone the declaration named is not
        // there, rather than the record set this handler manages having gone missing.
        assert!(
            reason.contains(HANDLER_ERROR_CLASS),
            "case {case:?}: expected the refusal to be classified {HANDLER_ERROR_CLASS:?}, got \
             {reason:?}"
        );

        // Nothing was created: the record's own zone never existed, so no zone of the account
        // holds it.
        let (status, listed) = route53(&router, region, "GET", "/2013-04-01/hostedzone").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListHostedZones, got {status} with body \
             {listed}"
        );
        assert!(
            elements(&listed, "Id").is_empty(),
            "case {case:?}: expected the account to hold no hosted zone, got {listed}"
        );
    }
}
