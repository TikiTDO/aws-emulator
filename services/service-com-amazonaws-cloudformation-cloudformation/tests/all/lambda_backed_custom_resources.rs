// SPDX-License-Identifier: CC0-1.0

//! A custom resource whose `ServiceToken` names a LAMBDA FUNCTION is provisioned by
//! INVOKING that function, and the stack settles on the answer that function sends to the
//! request's `ResponseURL`.
//!
//! The rule under test: **a stack that declares a function and a `Custom::` resource whose
//! `ServiceToken` is that function reaches `CREATE_COMPLETE`: CloudFormation invokes the
//! function with the request, the function answers at the request's `ResponseURL`, and the
//! `PhysicalResourceId` and `Data` it answered with are what `Ref` and `Fn::GetAtt` resolve
//! to — including when the function answers through the response module CloudFormation
//! supplies to INLINE code, which "is available only when you use the `ZipFile` property to
//! write your function's source code".**
//!
//! Every row drives the emulator through its own wire: the stack operations over
//! CloudFormation's awsQuery endpoint, the function created in AWS Lambda by the stack
//! itself, and the answer sent by the function's own code — nothing in this suite writes to
//! the `ResponseURL`, which is what makes the row about the provider rather than about the
//! plane.
//!
//! The rows differ along what the rule generalises over: how the function's own code answers
//! (the standard library's HTTP client, and the response module CloudFormation supplies),
//! and what it answers with (a minted physical resource id, and `Data` a template reads).

use std::collections::BTreeMap;

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{call, create_stack, member, router_with_engine, xml_text, xml_texts};

/// How long one row waits for the stack to settle, and how often it looks: the function is
/// invoked asynchronously and answers out of band, so the status is polled exactly as a
/// client polls it.
const POLL_ATTEMPTS: usize = 200;
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// A provider that answers with the standard library's own HTTP client, which is what a
/// function whose package ships no helper does.
const ANSWERS_WITH_URLLIB: &str = r#"
import json
import urllib.request


def handler(event, context):
    properties = event["ResourceProperties"]
    body = json.dumps(
        {
            "Status": "SUCCESS",
            "PhysicalResourceId": "probe-" + event["LogicalResourceId"],
            "Data": {"Echo": properties.get("Marker", "")},
        }
    ).encode("utf-8")
    request = urllib.request.Request(
        event["ResponseURL"],
        data=body,
        headers={"content-type": ""},
        method="PUT",
    )
    with urllib.request.urlopen(request) as answered:
        answered.read()
    return {}
"#;

/// A provider that answers through the response module CloudFormation supplies to inline
/// code — the documented way an inline provider answers.
const ANSWERS_WITH_THE_RESPONSE_MODULE: &str = r#"
import cfnresponse


def handler(event, context):
    properties = event["ResourceProperties"]
    cfnresponse.send(
        event,
        context,
        cfnresponse.SUCCESS,
        {"Echo": properties.get("Marker", "")},
        physicalResourceId="probe-" + event["LogicalResourceId"],
    )
"#;

/// One row: the function's own code, and the runtime it is declared with.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    source: &'static str,
    runtime: &'static str,
    marker: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a provider that answers with the standard library's HTTP client",
            stack_name: "lambda-custom-urllib",
            source: ANSWERS_WITH_URLLIB,
            runtime: "python3.12",
            marker: "v1",
        },
        Row {
            case: "a provider that answers through the supplied response module",
            stack_name: "lambda-custom-cfnresponse",
            source: ANSWERS_WITH_THE_RESPONSE_MODULE,
            runtime: "python3.12",
            marker: "a marker of its own",
        },
        // The same module, on another Python runtime the inline form is published for: the
        // module travels with every package of that family, not with one release.
        Row {
            case: "the supplied response module on another Python runtime",
            stack_name: "lambda-custom-cfnresponse-311",
            source: ANSWERS_WITH_THE_RESPONSE_MODULE,
            runtime: "python3.11",
            marker: "v2",
        },
    ]
}

/// The template one row deploys: the execution role the function assumes, the function
/// itself with the row's inline source, and the custom resource that function backs — with
/// outputs publishing what `Ref` and `Fn::GetAtt` resolve to for it.
fn template(source: &str, runtime: &str, marker: &str) -> String {
    json!({
        "Resources": {
            "Role": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Action": "sts:AssumeRole",
                            "Effect": "Allow",
                            "Principal": {"Service": "lambda.amazonaws.com"},
                        }],
                    },
                },
            },
            "Fn": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "Code": {"ZipFile": source},
                    "Handler": "index.handler",
                    "Role": {"Fn::GetAtt": ["Role", "Arn"]},
                    "Runtime": runtime,
                },
            },
            "Custom": {
                "Type": "Custom::Probe",
                "Properties": {
                    "ServiceToken": {"Fn::GetAtt": ["Fn", "Arn"]},
                    "Marker": marker,
                },
            },
        },
        "Outputs": {
            "ThingRef": {"Value": {"Ref": "Custom"}},
            "ThingEcho": {"Value": {"Fn::GetAtt": ["Custom", "Echo"]}},
        },
    })
    .to_string()
}

/// `DescribeStacks` for one stack: its status and the whole body.
async fn describe(router: &MultiServiceRouter, stack_name: &str) -> (String, String) {
    let (status_code, body) =
        call(router, "DescribeStacks", &[member("StackName", stack_name)]).await;
    assert_eq!(
        status_code, 200,
        "expected DescribeStacks to answer for {stack_name:?}, got {status_code} with {body}"
    );
    (xml_text(&body, "StackStatus").unwrap_or_default(), body)
}

/// A stack's reported outputs, as the key/value map a client reads.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    xml_texts(xml, "OutputKey")
        .into_iter()
        .zip(xml_texts(xml, "OutputValue"))
        .collect()
}

#[tokio::test]
async fn a_function_backed_custom_resource_settles_on_its_providers_answer() {
    for row in rows() {
        let case = row.case;
        let (router, _engine) = router_with_engine();
        let body = template(row.source, row.runtime, row.marker);
        let (status_code, created) = create_stack(
            &router,
            row.stack_name,
            &body,
            &[member("Capabilities.member.1", "CAPABILITY_IAM")],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to be accepted, got {status_code} with {created}"
        );

        let mut last = (String::new(), String::new());
        for _ in 0..POLL_ATTEMPTS {
            last = describe(&router, row.stack_name).await;
            if !last.0.ends_with("_IN_PROGRESS") {
                break;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        let outputs = outputs_of(&last.1);
        assert_eq!(
            last.0, "CREATE_COMPLETE",
            "case {case:?}: expected the stack to complete on its provider's answer, got {:?} \
             with {}",
            last.0, last.1
        );
        // The provider's answer IS the resource: the id it reported is what `Ref` resolves
        // to and the `Data` it published is what `Fn::GetAtt` resolves against.
        assert_eq!(
            outputs.get("ThingRef").map(String::as_str),
            Some("probe-Custom"),
            "case {case:?}: expected Ref to resolve to the answered id, got {outputs:?}"
        );
        assert_eq!(
            outputs.get("ThingEcho").map(String::as_str),
            Some(row.marker),
            "case {case:?}: expected Fn::GetAtt to resolve to the answered Data, got {outputs:?}"
        );
    }
}
