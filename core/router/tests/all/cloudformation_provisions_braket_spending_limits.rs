// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::Braket::SpendingLimit` deploys: the limit is
//! created in Amazon Braket's own control plane, the stack publishes the ARN that service
//! minted, deleting the stack takes the limit away — and a declaration that service REFUSES
//! fails the resource with its own refusal and rolls the stack back.
//!
//! The rule under test: **a resource type AWS publishes is one a template may declare, and a
//! declaration of it is deployed by calling the service that OWNS the type — so what the
//! stack publishes is what that service answered, and what that service refuses is a FAILED
//! RESOURCE rather than a template-format error.** Two crates have to agree for that to hold
//! (CloudFormation submits the template and makes the call, Amazon Braket serves it), so the
//! suite lives here: a crate-local test can only ever see one side of it.
//!
//! The rows of the deploy table differ along what the rule generalises over: which device the
//! limit caps (four quantum processors, under ARNs that carry a Region and ARNs that do not,
//! and one the inventory publishes as `OFFLINE`), the form of the amount (two decimals, one,
//! none — the three the modelled `^\d+(\.\d{1,2})?$` admits), which published return value
//! the output reads (`Ref`, `Fn::GetAtt SpendingLimitArn`, `Fn::GetAtt CreatedAt`), the
//! template spelling (JSON with long-form intrinsics, YAML with the short-form tags), whether
//! the declaration carries the type's own `Tags`, and how many limits one stack declares.
//!
//! The refusal rows differ along the CLASS of declaration Amazon Braket refuses: a device
//! KIND that has no spending limits at all (a simulator), a device the inventory publishes as
//! retired, and an amount outside the modelled pattern. Each is the owning service's own
//! refusal, reported on the resource that failed — never the
//! `Template format error: Unrecognized resource types` a type CloudFormation does not know
//! is refused with, which no row of either table may ever see.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::query_wire::{elements, form_encode, opt_element, outputs_of};

/// The region every request here is signed for, and so the region the stacks belong to.
const REGION: &str = "us-east-1";

/// Long enough for the stack's clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) to come
/// due twice over, so a create and the rollback it opens have both settled.
const PAST_THE_SPANS: Duration = Duration::from_millis(1_500);

/// The ARN prefix a spending limit of this Region carries (`SpendingLimitArn`:
/// `^arn:aws[a-z\-]*:braket:[a-z0-9\-]+:[0-9]{12}:spending-limit/.*$`).
fn spending_limit_arn_prefix() -> String {
    format!("arn:aws:braket:{REGION}:")
}

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_braket_braket::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation action over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
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
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// `SearchSpendingLimits` narrowed to one device, over Amazon Braket's own restJson1 wire
/// (`POST /spending-limits`) — the one operation that reports a spending limit.
async fn spending_limits_of_device(router: &MultiServiceRouter, device_arn: &str) -> Vec<Value> {
    let body = json!({
        "filters": [{
            "name": "deviceArn",
            "values": [device_arn],
            "operator": "EQUAL",
        }]
    })
    .to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri("/spending-limits")
        .header("host", format!("braket.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("braket"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let answered: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    answered
        .get("spendingLimits")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Create a stack, wait past its spans, and answer the `DescribeStacks` body.
async fn create_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack \u{2014} an AWS::Braket::SpendingLimit \
         is a type a template may declare \u{2014} got {status} with body {body}"
    );
    assert!(
        !body.contains(UNRECOGNIZED_TYPES),
        "case {case:?}: expected the type to be recognised, got {body}"
    );
    tokio::time::sleep(PAST_THE_SPANS).await;
    let (status, body) =
        cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
    );
    body
}

/// The fixed text CloudFormation refuses a template declaring a type it does NOT know with,
/// which is what the defect this suite pins answered for a type AWS publishes.
const UNRECOGNIZED_TYPES: &str = "Unrecognized resource types";

/// What one row expects of one spending limit the stack declares.
struct Declared {
    /// The output key whose value is the limit's minted ARN.
    arn_output: &'static str,
    /// The output key whose value is the instant Amazon Braket recorded, where the row reads
    /// one.
    created_at_output: Option<&'static str>,
    /// The device the limit caps, which the listing must report it against.
    device_arn: &'static str,
    /// The amount the declaration asked for, which the listed limit must carry.
    amount: &'static str,
}

/// One row: a template and the limits it declares.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    template: String,
    declared: Vec<Declared>,
}

/// The four ONLINE/OFFLINE quantum processors of this emulator's device inventory the rows
/// below cap, and the online simulator the refusal table uses. Source: Amazon Braket's own
/// `SearchDevices` inventory (`deviceType` `QPU` against `SIMULATOR`) — the ARNs carry a
/// Region segment for some devices and none for others, which is what the rows span.
const ARIA: &str = "arn:aws:braket:us-east-1::device/qpu/ionq/Aria-1";
const GARNET: &str = "arn:aws:braket:eu-north-1::device/qpu/iqm/Garnet";
const ANKAA: &str = "arn:aws:braket:us-west-1::device/qpu/rigetti/Ankaa-3";
const AQUILA: &str = "arn:aws:braket:us-east-1::device/qpu/quera/Aquila";
const FORTE: &str = "arn:aws:braket:us-east-1::device/qpu/ionq/Forte-1";
const SV1: &str = "arn:aws:braket:::device/quantum-simulator/amazon/sv1";
const LUCY: &str = "arn:aws:braket:eu-west-2::device/qpu/oqc/Lucy";

fn rows() -> Vec<Row> {
    vec![
        // The issue's own stack: one limit on a quantum processor, with every published
        // return value read back, in JSON with long-form intrinsics.
        Row {
            case: "one limit, Ref, SpendingLimitArn and CreatedAt read back, JSON spelling",
            stack_name: "braket-limit-round-trip",
            template: json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Resources": {
                    "Limit": {
                        "Type": "AWS::Braket::SpendingLimit",
                        "Properties": { "DeviceArn": ARIA, "SpendingLimit": "5.00" }
                    }
                },
                "Outputs": {
                    "LimitRef": {"Value": {"Ref": "Limit"}},
                    "SpendingLimitArn": {
                        "Value": {"Fn::GetAtt": ["Limit", "SpendingLimitArn"]}
                    },
                    "CreatedAt": {"Value": {"Fn::GetAtt": ["Limit", "CreatedAt"]}}
                }
            })
            .to_string(),
            declared: vec![Declared {
                arn_output: "LimitRef",
                created_at_output: Some("CreatedAt"),
                device_arn: ARIA,
                amount: "5.00",
            }],
        },
        // The same type in YAML with short-form tags, on a device whose ARN names another
        // Region, with the smallest amount the pattern admits and the type's own `Tags`,
        // read back through `Fn::GetAtt SpendingLimitArn`.
        Row {
            case: "a two-decimal minimum amount and declared tags, YAML spelling",
            stack_name: "braket-limit-yaml",
            template: format!(
                "\
Resources:
  Limit:
    Type: AWS::Braket::SpendingLimit
    Properties:
      DeviceArn: {GARNET}
      SpendingLimit: \"0.01\"
      Tags:
        - Key: team
          Value: quantum
Outputs:
  LimitArn:
    Value: !GetAtt Limit.SpendingLimitArn
  LimitCreatedAt:
    Value: !GetAtt Limit.CreatedAt
"
            ),
            declared: vec![Declared {
                arn_output: "LimitArn",
                created_at_output: Some("LimitCreatedAt"),
                device_arn: GARNET,
                amount: "0.01",
            }],
        },
        // An amount with NO decimal part, which the pattern admits too, read back through
        // `Ref` alone.
        Row {
            case: "a whole-dollar amount, Ref alone",
            stack_name: "braket-limit-whole",
            template: json!({
                "Resources": {
                    "Limit": {
                        "Type": "AWS::Braket::SpendingLimit",
                        "Properties": { "DeviceArn": ANKAA, "SpendingLimit": "1000" }
                    }
                },
                "Outputs": {"LimitRef": {"Value": {"Ref": "Limit"}}}
            })
            .to_string(),
            declared: vec![Declared {
                arn_output: "LimitRef",
                created_at_output: None,
                device_arn: ANKAA,
                amount: "1000",
            }],
        },
        // Two limits in one stack, on two different devices — one of them a device the
        // inventory publishes as OFFLINE, which is still a processor a limit may cap — with
        // a one-decimal amount beside a two-decimal one. Each publishes its OWN minted ARN.
        Row {
            case: "two limits in one stack, one on an offline device",
            stack_name: "braket-limit-pair",
            template: json!({
                "Resources": {
                    "First": {
                        "Type": "AWS::Braket::SpendingLimit",
                        "Properties": { "DeviceArn": AQUILA, "SpendingLimit": "12.5" }
                    },
                    "Second": {
                        "Type": "AWS::Braket::SpendingLimit",
                        "Properties": { "DeviceArn": FORTE, "SpendingLimit": "250.75" }
                    }
                },
                "Outputs": {
                    "FirstArn": {"Value": {"Fn::GetAtt": ["First", "SpendingLimitArn"]}},
                    "SecondArn": {"Value": {"Ref": "Second"}}
                }
            })
            .to_string(),
            declared: vec![
                Declared {
                    arn_output: "FirstArn",
                    created_at_output: None,
                    device_arn: AQUILA,
                    amount: "12.5",
                },
                Declared {
                    arn_output: "SecondArn",
                    created_at_output: None,
                    device_arn: FORTE,
                    amount: "250.75",
                },
            ],
        },
    ]
}

/// Whether `arn` has the form a `SpendingLimitArn` publishes.
fn is_spending_limit_arn(arn: &str) -> bool {
    arn.starts_with(&spending_limit_arn_prefix())
        && arn.contains(":spending-limit/")
        && !arn.ends_with(":spending-limit/")
}

/// The summary Amazon Braket reports for one limit of a device, by the ARN the stack
/// published.
fn summary_of<'s>(summaries: &'s [Value], arn: &str) -> Option<&'s Value> {
    summaries
        .iter()
        .find(|summary| summary.get("spendingLimitArn").and_then(Value::as_str) == Some(arn))
}

/// The instant one reported summary carries, in epoch seconds, however that service's wire
/// wrote it: a JSON protocol carries a timestamp as epoch seconds unless the member's own
/// format makes it a date-time string, and either form denotes the same instant.
fn reported_instant(summary: &Value) -> f64 {
    let reported = summary
        .get("createdAt")
        .unwrap_or_else(|| panic!("no createdAt in the reported summary {summary}"));
    if let Some(seconds) = reported.as_f64() {
        return seconds;
    }
    let text = reported.as_str().unwrap_or_default();
    aws_smithy_types::DateTime::from_str(text, aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|error| panic!("the reported createdAt {text:?} is no instant: {error}"))
        .as_secs_f64()
}

/// One instant in epoch MICROSECONDS, which is the precision a reader of a timestamp carried
/// as a number keeps: the two sides of the comparison above are the same instant when they
/// agree there, up to the one microsecond the two different decompositions of a `f64` second
/// can disagree by.
fn microseconds_of(seconds: f64) -> i64 {
    (seconds * 1_000_000.0).round() as i64
}

#[tokio::test]
async fn a_deployed_stack_publishes_the_spending_limit_braket_minted() {
    for row in rows() {
        let router = build_router();
        let case = row.case;
        let body = create_and_settle(&router, case, row.stack_name, &row.template).await;
        assert_eq!(
            opt_element(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the create to complete, got {body}"
        );
        let outputs: BTreeMap<String, String> = outputs_of(&body);

        for declared in &row.declared {
            let arn = outputs
                .get(declared.arn_output)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no {} output in {body}", declared.arn_output)
                })
                .clone();
            assert!(
                is_spending_limit_arn(&arn),
                "case {case:?}: expected the published value to be an ARN of the modelled \
                 spending-limit form, got {arn:?}"
            );

            // The identifier the stack published is one the OWNING service resolves, and the
            // limit it resolves to is the one the template declared.
            let summaries = spending_limits_of_device(&router, declared.device_arn).await;
            let summary = summary_of(&summaries, &arn).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected Amazon Braket to list the published ARN {arn:?} \
                     for {}, got {summaries:?}",
                    declared.device_arn
                )
            });
            assert_eq!(
                summary.get("deviceArn").and_then(Value::as_str),
                Some(declared.device_arn),
                "case {case:?}: expected the limit to cap the declared device, got {summary}"
            );
            assert_eq!(
                summary.get("spendingLimit").and_then(Value::as_str),
                Some(declared.amount),
                "case {case:?}: expected the declared amount, got {summary}"
            );
            assert_eq!(
                summaries.len(),
                1,
                "case {case:?}: expected exactly one limit for {}, got {summaries:?}",
                declared.device_arn
            );

            if let Some(key) = declared.created_at_output {
                let published = outputs.get(key).unwrap_or_else(|| {
                    panic!("case {case:?}: no {key} output in {body}")
                });
                // The attribute is TEXT, and the text a `CreatedAt` carries is the ISO-8601
                // instant...
                assert!(
                    published.contains('T') && published.ends_with('Z'),
                    "case {case:?}: expected the published CreatedAt to be an ISO-8601 \
                     instant, got {published:?}"
                );
                // ...of the very instant Amazon Braket reports for the limit, which parsing
                // it back is what checks.
                let parsed = aws_smithy_types::DateTime::from_str(
                    published,
                    aws_smithy_types::date_time::Format::DateTime,
                )
                .unwrap_or_else(|error| {
                    panic!(
                        "case {case:?}: expected the published CreatedAt {published:?} to \
                         parse as an ISO-8601 instant, got {error}"
                    )
                });
                let reported = reported_instant(summary);
                let drift = microseconds_of(parsed.as_secs_f64()) - microseconds_of(reported);
                assert!(
                    drift.abs() <= 1,
                    "case {case:?}: expected the published CreatedAt to be the instant the \
                     summary reports ({reported}) to the microsecond, got {published:?} \
                     ({drift} microseconds apart) against {summary}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_declared_time_period_reaches_the_limit_whichever_form_it_is_written_in() {
    // (case, the stack, the window the declaration states, the epoch seconds it denotes).
    // The template form is ISO-8601 text, which is how CloudFormation spells an instant,
    // while `TimePeriod`'s own members are the seconds since the epoch their modelled format
    // declares — so both spellings have to reach the same window.
    let rows: [(&str, &str, Value, (f64, f64)); 2] = [
        (
            "the window stated as ISO-8601 text",
            "braket-limit-iso-period",
            json!({"StartAt": "2026-01-01T00:00:00Z", "EndAt": "2026-04-01T00:00:00Z"}),
            (1_767_225_600.0, 1_775_001_600.0),
        ),
        (
            "the window stated as the seconds the member is carried in",
            "braket-limit-epoch-period",
            json!({"StartAt": 1_767_225_600_i64, "EndAt": 1_775_001_600_i64}),
            (1_767_225_600.0, 1_775_001_600.0),
        ),
    ];

    for (case, stack_name, period, (start, end)) in rows {
        let router = build_router();
        let template = json!({
            "Resources": {
                "Limit": {
                    "Type": "AWS::Braket::SpendingLimit",
                    "Properties": {
                        "DeviceArn": ARIA,
                        "SpendingLimit": "9.99",
                        "TimePeriod": period,
                    }
                }
            },
            "Outputs": {"LimitRef": {"Value": {"Ref": "Limit"}}}
        })
        .to_string();

        let body = create_and_settle(&router, case, stack_name, &template).await;
        assert_eq!(
            opt_element(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the create to complete, got {body}"
        );
        let arn = outputs_of(&body)
            .get("LimitRef")
            .unwrap_or_else(|| panic!("case {case:?}: no LimitRef output in {body}"))
            .clone();
        let summaries = spending_limits_of_device(&router, ARIA).await;
        let summary = summary_of(&summaries, &arn).unwrap_or_else(|| {
            panic!("case {case:?}: expected Amazon Braket to list {arn:?}, got {summaries:?}")
        });
        for (member, expected) in [("startAt", start), ("endAt", end)] {
            let reported = summary
                .get("timePeriod")
                .and_then(|window| window.get(member))
                .and_then(Value::as_f64)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no timePeriod.{member} in {summary}")
                });
            assert!(
                (reported - expected).abs() < 1.0,
                "case {case:?}: expected timePeriod.{member} at {expected}, got {reported} in \
                 {summary}"
            );
        }
    }
}

#[tokio::test]
async fn a_stack_update_applies_the_change_to_the_limit_amazon_braket_holds() {
    let router = build_router();
    let stack_name = "braket-limit-updated";
    // The rows differ along which members the re-declaration changes: the amount alone, then
    // the amount and the window together. Each is applied to the limit the stack created —
    // its identity survives, and Amazon Braket's own listing reports the new values.
    let rows: [(&str, &str, Value); 2] = [
        (
            "the amount alone",
            "40.00",
            json!({ "DeviceArn": ANKAA, "SpendingLimit": "40.00" }),
        ),
        (
            "the amount and the window together",
            "55.5",
            json!({
                "DeviceArn": ANKAA,
                "SpendingLimit": "55.5",
                "TimePeriod": {"StartAt": "2026-02-01T00:00:00Z", "EndAt": "2026-03-01T00:00:00Z"},
            }),
        ),
    ];

    let created = create_and_settle(
        &router,
        "the first deployment",
        stack_name,
        &json!({
            "Resources": {
                "Limit": {
                    "Type": "AWS::Braket::SpendingLimit",
                    "Properties": { "DeviceArn": ANKAA, "SpendingLimit": "30.00" }
                }
            },
            "Outputs": {"LimitRef": {"Value": {"Ref": "Limit"}}}
        })
        .to_string(),
    )
    .await;
    let arn = outputs_of(&created)
        .get("LimitRef")
        .unwrap_or_else(|| panic!("no LimitRef output in {created}"))
        .clone();

    for (case, amount, properties) in rows {
        let template = json!({
            "Resources": {"Limit": {"Type": "AWS::Braket::SpendingLimit", "Properties": properties}},
            "Outputs": {"LimitRef": {"Value": {"Ref": "Limit"}}}
        })
        .to_string();
        let (status, body) = cloudformation(
            &router,
            "UpdateStack",
            &[("StackName", stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with body {body}"
        );
        tokio::time::sleep(PAST_THE_SPANS).await;
        let (_, described) =
            cloudformation(&router, "DescribeStacks", &[("StackName", stack_name)]).await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("UPDATE_COMPLETE"),
            "case {case:?}: expected the update to complete, got {described}"
        );
        assert_eq!(
            outputs_of(&described).get("LimitRef"),
            Some(&arn),
            "case {case:?}: expected the limit to keep the identity Amazon Braket minted, got \
             {described}"
        );

        // The change reached the limit THAT service holds, not just the stack's record.
        let summaries = spending_limits_of_device(&router, ANKAA).await;
        assert_eq!(
            summaries.len(),
            1,
            "case {case:?}: expected the one limit to be changed rather than a second one \
             created, got {summaries:?}"
        );
        let summary = summary_of(&summaries, &arn).unwrap_or_else(|| {
            panic!("case {case:?}: expected Amazon Braket to still list {arn:?}, got {summaries:?}")
        });
        assert_eq!(
            summary.get("spendingLimit").and_then(Value::as_str),
            Some(amount),
            "case {case:?}: expected the re-declared amount, got {summary}"
        );
    }
}

#[tokio::test]
async fn deleting_the_stack_deletes_the_spending_limits_it_created() {
    let router = build_router();
    let case = "two limits deleted with their stack";
    let stack_name = "braket-limit-deleted";
    let template = json!({
        "Resources": {
            "First": {
                "Type": "AWS::Braket::SpendingLimit",
                "Properties": { "DeviceArn": ARIA, "SpendingLimit": "7.25" }
            },
            "Second": {
                "Type": "AWS::Braket::SpendingLimit",
                "Properties": { "DeviceArn": GARNET, "SpendingLimit": "8" }
            }
        },
        "Outputs": {
            "FirstArn": {"Value": {"Ref": "First"}},
            "SecondArn": {"Value": {"Ref": "Second"}}
        }
    })
    .to_string();

    let body = create_and_settle(&router, case, stack_name, &template).await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the create to complete, got {body}"
    );
    for device in [ARIA, GARNET] {
        let summaries = spending_limits_of_device(&router, device).await;
        assert_eq!(
            summaries.len(),
            1,
            "case {case:?}: expected Amazon Braket to hold one limit for {device}, got \
             {summaries:?}"
        );
    }

    let (status, deleted) =
        cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {deleted}"
    );
    tokio::time::sleep(PAST_THE_SPANS).await;

    for device in [ARIA, GARNET] {
        let summaries = spending_limits_of_device(&router, device).await;
        assert!(
            summaries.is_empty(),
            "case {case:?}: expected the limit for {device} to be gone from Amazon Braket, \
             got {summaries:?}"
        );
    }
}

/// One refusal row: a declaration Amazon Braket refuses, and what the failed resource must
/// report.
struct Refused {
    case: &'static str,
    stack_name: &'static str,
    device_arn: &'static str,
    amount: &'static str,
    /// The FIXED text of the sentence Amazon Braket words that class of refusal with, which
    /// the resource's failure reason has to carry.
    sentence: &'static str,
    /// The error code that service answers, which the SDK diagnostic in the reason names.
    code: &'static str,
    /// The HTTP status that error carries, from the model's own `@httpError`.
    status: u16,
    /// The `HandlerErrorCode` class the provider puts that refusal in.
    handler_error_code: &'static str,
}

const REFUSED: &[Refused] = &[
    // The issue's own refusal: a spending limit is a cost control over what a device
    // CHARGES, which a simulator is not sold per device at all.
    Refused {
        case: "a simulator device, which has no spending limits at all",
        stack_name: "braket-limit-simulator",
        device_arn: SV1,
        amount: "5.00",
        sentence: "Spending limits can not be created for simulator devices.",
        code: "ValidationException",
        status: 400,
        handler_error_code: "InvalidRequest",
    },
    // A device the inventory publishes as RETIRED is gone, so nothing can be spent on it —
    // a different modelled error, with its own status.
    Refused {
        case: "a retired device",
        stack_name: "braket-limit-retired",
        device_arn: LUCY,
        amount: "5.00",
        sentence: "Device is RETIRED. Try another device.",
        code: "DeviceRetiredException",
        status: 410,
        handler_error_code: "InvalidRequest",
    },
    // A `deviceArn` that is not a device ARN at all names no device: a third class of
    // refusal, naming the member the create refuses the value for.
    Refused {
        case: "a deviceArn that is not a device ARN",
        stack_name: "braket-limit-not-an-arn",
        device_arn: "arn:aws:braket:us-east-1:000000000000:quantum-task/not-a-device",
        amount: "5.00",
        sentence: "deviceArn is not valid.",
        code: "ValidationException",
        status: 400,
        handler_error_code: "InvalidRequest",
    },
];

#[tokio::test]
async fn a_declaration_amazon_braket_refuses_rolls_the_stack_back() {
    for row in REFUSED {
        let router = build_router();
        let case = row.case;
        let template = json!({
            "Resources": {
                "Limit": {
                    "Type": "AWS::Braket::SpendingLimit",
                    "Properties": {
                        "DeviceArn": row.device_arn,
                        "SpendingLimit": row.amount,
                    }
                }
            },
            "Outputs": {"LimitRef": {"Value": {"Ref": "Limit"}}}
        })
        .to_string();

        let body = create_and_settle(&router, case, row.stack_name, &template).await;
        assert_eq!(
            opt_element(&body, "StackStatus").as_deref(),
            Some("ROLLBACK_COMPLETE"),
            "case {case:?}: expected the failed create to be rolled back rather than publish \
             an ARN Amazon Braket never minted, got {body}"
        );
        assert!(
            outputs_of(&body).is_empty(),
            "case {case:?}: expected no output for a resource nothing created, got {body}"
        );

        // The failure is the OWNING service's own refusal, on the resource that failed, and
        // never a refusal of the template: no event reports a validation error at all.
        let (_, events) =
            cloudformation(&router, "DescribeStackEvents", &[("StackName", row.stack_name)]).await;
        assert!(
            !events.contains(UNRECOGNIZED_TYPES),
            "case {case:?}: expected no template-format error for a type AWS publishes, got \
             {events}"
        );
        let reasons = elements(&events, "ResourceStatusReason");
        let expected_envelope = format!(
            "(Service: Braket; Operation: CreateSpendingLimit; Status Code: {}; Error Code: {};",
            row.status, row.code
        );
        let expected_suffix = format!("HandlerErrorCode: {}", row.handler_error_code);
        assert!(
            reasons.iter().any(|reason| reason.contains(row.sentence)
                && reason.contains(&expected_envelope)
                && reason.contains(&expected_suffix)),
            "case {case:?}: expected a failure reason carrying {:?}, {expected_envelope:?} and \
             {expected_suffix:?}, got {reasons:?}",
            row.sentence
        );

        // And nothing was created: Amazon Braket holds no limit for the declared device.
        let summaries = spending_limits_of_device(&router, row.device_arn).await;
        assert!(
            summaries.is_empty(),
            "case {case:?}: expected Amazon Braket to hold no limit for {}, got {summaries:?}",
            row.device_arn
        );
    }
}
