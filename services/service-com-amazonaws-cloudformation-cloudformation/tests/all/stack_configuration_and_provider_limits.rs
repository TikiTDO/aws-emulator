// SPDX-License-Identifier: CC0-1.0

//! Three rules a stack's own configuration and its CUSTOM RESOURCES are held to.
//!
//! 1. **Every pseudo parameter resolves against the stack's own recorded configuration.**
//!    `AWS::NotificationARNs` "returns the list of notification Amazon Resource Names (ARNs)
//!    for the current stack" — a LIST, like a list-typed parameter's `Ref`, so `Fn::Join`,
//!    `Fn::Select` and `Fn::Length` read it; a stack carrying no topics resolves to the EMPTY
//!    list, which it HAS, rather than to nothing. The rows differ in how many topics the
//!    stack carries (none, one, several) and in the function reading them.
//! 2. **A custom resource's `ServiceTimeout` must be an integer from 1 to 14400 seconds**, and
//!    a declaration outside that range fails the resource BEFORE anything is published to its
//!    provider. The rows differ in the value declared: each published bound, one past each
//!    bound, a value that is not an integer at all, and none at all.
//! 3. **A provider response document may not exceed 4096 bytes**, and one past that bound is
//!    not read: the resource fails with the response-too-long reason and its stack rolls
//!    back. The rows differ in the size of the document answered — under the bound, exactly
//!    at it, and one byte past it.
//! 4. **A custom resource's attributes are the `Data` its provider answered with**, so a
//!    resource whose declaration reads one through `Fn::GetAtt` is created only once that
//!    answer is in hand — with the value it carries — and a declaration naming a key the
//!    `Data` LACKS fails the consuming resource with CloudFormation's attribute error. The
//!    rows differ in whether the key the consumer reads is one the provider returned.
//!
//! Every assertion drives the emulator through its own wire: the stack operations over
//! CloudFormation's awsQuery endpoint, and each provider's answer as the plain, unsigned HTTP
//! `PUT` to the published `ResponseURL`, through the shared sequence
//! [`super::custom_resource_providers`] spells once.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::provisioning::custom_resource;
use service_com_amazonaws_cloudformation_cloudformation::state::template;
use smithy_server_runtime::MultiServiceRouter;

use super::custom_resource_providers::{
    awaited_request, emulator, outputs_of, put_response, settled,
    settled_with_a_responsive_provider, POLL_ATTEMPTS, POLL_INTERVAL, PROVIDER_TOPIC,
};
use super::harness::{call, create_stack, member, xml_text, REGION};
use super::stack_lifecycle::resource_event_reason;

/// The topics one row gives the stack, as the `NotificationARNs.member.<n>` members a
/// request spells them in.
fn notification_members(topics: &[&str]) -> Vec<(String, String)> {
    topics
        .iter()
        .enumerate()
        .map(|(index, topic)| {
            member(
                &format!("NotificationARNs.member.{}", index + 1),
                &format!("arn:aws:sns:{REGION}:000000000000:{topic}"),
            )
        })
        .collect()
}

/// The ARN form the rows above expect a topic name to resolve to.
fn topic_arn(topic: &str) -> String {
    format!("arn:aws:sns:{REGION}:000000000000:{topic}")
}

/// A template whose outputs read `AWS::NotificationARNs` through each function that takes a
/// list: the joined list, its length, and its first entry where it has one.
fn pseudo_parameter_template(reads_an_entry: bool) -> String {
    let mut outputs = serde_json::Map::new();
    outputs.insert(
        "Joined".to_string(),
        json!({"Value": {"Fn::Join": [",", {"Ref": "AWS::NotificationARNs"}]}}),
    );
    outputs.insert(
        "Count".to_string(),
        json!({"Value": {"Fn::Length": {"Ref": "AWS::NotificationARNs"}}}),
    );
    if reads_an_entry {
        outputs.insert(
            "First".to_string(),
            json!({"Value": {"Fn::Select": ["0", {"Ref": "AWS::NotificationARNs"}]}}),
        );
    }
    json!({
        "Transform": "AWS::LanguageExtensions",
        "Resources": {
            "Marker": {"Type": "AWS::CloudFormation::WaitConditionHandle"},
        },
        "Outputs": Value::Object(outputs),
    })
    .to_string()
}

/// The stack's outputs once it has settled, with the status it settled in.
async fn settled_outputs(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
) -> (String, BTreeMap<String, String>) {
    let (status, body) = settled(router, case, stack_name).await;
    (status, outputs_of(&body))
}

/// `AWS::NotificationARNs` resolves to the list of the stack's notification topics —
/// including the empty list for a stack that carries none — so every output reading it
/// carries an `OutputValue`.
#[tokio::test]
async fn the_notification_arns_pseudo_parameter_resolves_to_the_stacks_topics() {
    // (case, stack name, the topics the stack is created with)
    let rows: &[(&str, &str, &[&str])] = &[
        ("a stack carrying no topic at all", "pseudo-none", &[]),
        ("a stack carrying one topic", "pseudo-one", &["deployments"]),
        (
            "a stack carrying several topics",
            "pseudo-many",
            &["deployments", "audit", "oncall"],
        ),
    ];
    for (case, stack_name, topics) in rows {
        let (router, _engine) = emulator(case).await;
        // `Fn::Length` is a language-extensions directive, which the transform the template
        // declares expands — and a template with a transform is deployed under
        // `CAPABILITY_AUTO_EXPAND`.
        let mut members = vec![member("Capabilities.member.1", "CAPABILITY_AUTO_EXPAND")];
        members.extend(notification_members(topics));
        let (status_code, created) = create_stack(
            &router,
            stack_name,
            &pseudo_parameter_template(!topics.is_empty()),
            &members,
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to be accepted, got {status_code} with {created}"
        );
        let (status, outputs) = settled_outputs(&router, case, stack_name).await;
        assert_eq!(
            status, "CREATE_COMPLETE",
            "case {case:?}: the stack must complete, got {status}"
        );
        let expected_joined = topics
            .iter()
            .map(|topic| topic_arn(topic))
            .collect::<Vec<String>>()
            .join(",");
        assert_eq!(
            outputs.get("Joined"),
            Some(&expected_joined),
            "case {case:?}: expected the joined topics {expected_joined:?}, got {outputs:?}"
        );
        assert_eq!(
            outputs.get("Count"),
            Some(&topics.len().to_string()),
            "case {case:?}: expected {} topics counted, got {outputs:?}",
            topics.len()
        );
        if let Some(first) = topics.first() {
            assert_eq!(
                outputs.get("First"),
                Some(&topic_arn(first)),
                "case {case:?}: expected the first topic {first:?}, got {outputs:?}"
            );
        }
    }
}

/// A template with one custom resource whose `ServiceTimeout` is `declared`, where the row
/// declares one at all.
fn service_timeout_template(declared: Option<&str>) -> String {
    let mut properties = serde_json::Map::new();
    properties.insert(
        custom_resource::SERVICE_TOKEN.to_string(),
        json!(topic_arn(PROVIDER_TOPIC)),
    );
    if let Some(declared) = declared {
        properties.insert(custom_resource::SERVICE_TIMEOUT.to_string(), json!(declared));
    }
    json!({
        "Resources": {
            "Thing": {
                "Type": "AWS::CloudFormation::CustomResource",
                "Properties": Value::Object(properties),
            },
        },
    })
    .to_string()
}

/// A `ServiceTimeout` outside 1-14400 fails the resource with the rule as its reason, and no
/// request reaches the provider; one inside the range is dispatched.
///
/// The bounds are the documented ones for `AWS::CloudFormation::CustomResource`
/// (`ServiceTimeout`: "the value must be an integer from 1 to 14400"), so each published
/// bound and the value one past it is a row — including a value no bound admits at all.
#[tokio::test]
async fn a_service_timeout_outside_the_documented_range_is_refused_before_the_provider() {
    // (case, stack name, the declared value, whether the provider must be sent a request)
    let rows: &[(&str, &str, Option<&str>, bool)] = &[
        ("no ServiceTimeout at all", "timeout-absent", None, true),
        ("the lowest value the range admits", "timeout-lowest", Some("1"), true),
        ("the highest value the range admits", "timeout-highest", Some("14400"), true),
        ("one below the range", "timeout-below", Some("0"), false),
        ("one above the range", "timeout-above", Some("14401"), false),
        ("a value that is not an integer", "timeout-fractional", Some("0.5"), false),
        ("a value that is not a number", "timeout-text", Some("soon"), false),
    ];
    for (case, stack_name, declared, dispatched) in rows {
        let (router, engine) = emulator(case).await;
        let (status_code, created) = create_stack(
            &router,
            stack_name,
            &service_timeout_template(*declared),
            &[],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to be accepted, got {status_code} with {created}"
        );
        let request = awaited_request(&engine, stack_name, "Thing");
        assert_eq!(
            request.is_some(),
            *dispatched,
            "case {case:?}: expected the provider to be sent a request: {dispatched}, got \
             {request:?}"
        );
        if *dispatched {
            continue;
        }
        // The rollback the failure owes sends the provider a `Delete`, which a provider
        // that answers whatever it is sent carries out.
        let (status, body) =
            settled_with_a_responsive_provider(&router, &engine, case, stack_name, "refused")
                .await;
        assert_eq!(
            status, "ROLLBACK_COMPLETE",
            "case {case:?}: a refused ServiceTimeout must roll the stack back, got {status} with \
             {body}"
        );
        let reason = recorded_reason(&router, stack_name, "Thing", "CREATE_FAILED").await;
        assert_eq!(
            reason, custom_resource::SERVICE_TIMEOUT_OUT_OF_RANGE,
            "case {case:?}: expected the range sentence, got {reason:?}"
        );
    }
}

/// A provider response document of `bytes` bytes exactly: the members a well-formed answer
/// carries, padded in one `Data` value to the size the row asks for.
fn response_of_size(bytes: usize, physical_resource_id: &str) -> Value {
    let mut answer = json!({
        "Status": custom_resource::SUCCESS,
        "PhysicalResourceId": physical_resource_id,
        "Data": {"Padding": ""},
    });
    let overhead = answer.to_string().len();
    let padding = "p".repeat(bytes.saturating_sub(overhead));
    answer["Data"]["Padding"] = json!(padding);
    assert_eq!(
        answer.to_string().len(),
        bytes,
        "the padded document must be exactly {bytes} bytes"
    );
    answer
}

/// The 4096-byte bound on a provider's response document: a document up to it completes the
/// resource, and one past it fails it with the response-too-long reason and rolls the stack
/// back.
#[tokio::test]
async fn a_response_document_past_the_bound_fails_the_resource() {
    let bound = custom_resource::MAX_RESPONSE_BODY_BYTES;
    // (case, stack name, the size of the document the provider answers with, accepted)
    let rows: &[(&str, &str, usize, bool)] = &[
        ("a document well under the bound", "response-small", 256, true),
        ("a document of exactly the bound", "response-exact", bound, true),
        ("a document one byte past the bound", "response-over", bound + 1, false),
    ];
    for (case, stack_name, size, accepted) in rows {
        let (router, engine) = emulator(case).await;
        let (status_code, created) = create_stack(
            &router,
            stack_name,
            &service_timeout_template(None),
            &[],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to be accepted, got {status_code} with {created}"
        );
        let (stack_id, request, _) = awaited_request(&engine, stack_name, "Thing")
            .unwrap_or_else(|| panic!("case {case:?}: the create must be waiting for a request"));
        let (answered, answer_body) = put_response(
            &router,
            &engine,
            &stack_id,
            "Thing",
            &request.request_id,
            &response_of_size(*size, "sized-answer"),
        )
        .await;
        assert_eq!(
            answered, 200,
            "case {case:?}: a provider's write is accepted either way, got {answered} with \
             {answer_body}"
        );
        let (status, body) = if *accepted {
            settled(&router, case, stack_name).await
        } else {
            // The rollback the failure owes sends the provider a `Delete` of the resource
            // under the id CloudFormation itself assigned, which a provider that answers
            // whatever it is sent carries out.
            settled_with_a_responsive_provider(&router, &engine, case, stack_name, "sized-answer")
                .await
        };
        if *accepted {
            assert_eq!(
                status, "CREATE_COMPLETE",
                "case {case:?}: a document within the bound must complete the resource, got \
                 {status} with {body}"
            );
            continue;
        }
        assert_eq!(
            status, "ROLLBACK_COMPLETE",
            "case {case:?}: a document past the bound must roll the stack back, got {status} \
             with {body}"
        );
        let reason = recorded_reason(&router, stack_name, "Thing", "CREATE_FAILED").await;
        assert_eq!(
            reason, custom_resource::RESPONSE_TOO_LONG_REASON,
            "case {case:?}: expected the response-too-long reason, got {reason:?}"
        );
    }
}

/// The attribute the provider of the rows below answers with, and the value it carries.
const RETURNED_KEY: &str = "Present";
const RETURNED_VALUE: &str = "https://provisioned.example.invalid/endpoint";

/// A template with a provider custom resource and a consumer whose own property reads
/// `attribute` of it through `Fn::GetAtt`.
fn consumer_template(attribute: &str) -> String {
    json!({
        "Resources": {
            "Provider": {
                "Type": "Custom::Provider",
                "Properties": {custom_resource::SERVICE_TOKEN: topic_arn(PROVIDER_TOPIC)},
            },
            "Consumer": {
                "Type": "Custom::Consumer",
                "Properties": {
                    custom_resource::SERVICE_TOKEN: topic_arn(PROVIDER_TOPIC),
                    "Value": {"Fn::GetAtt": ["Provider", attribute]},
                },
            },
        },
    })
    .to_string()
}

/// A resource that reads a custom resource's attribute is created with the value the
/// provider's `Data` carried — and fails with the attribute error when the `Data` lacks the
/// key it reads.
#[tokio::test]
async fn a_consumer_of_a_providers_data_is_created_from_the_answer() {
    // (case, stack name, the attribute the consumer reads, whether the Data carries it)
    let rows: &[(&str, &str, &str, bool)] = &[
        ("an attribute the provider returned", "getatt-present", RETURNED_KEY, true),
        ("an attribute the provider's Data lacks", "getatt-missing", "Absent", false),
    ];
    for (case, stack_name, attribute, returned) in rows {
        let (router, engine) = emulator(case).await;
        let (status_code, created) =
            create_stack(&router, stack_name, &consumer_template(attribute), &[]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to be accepted, got {status_code} with {created}"
        );
        // The provider is asked first, and the consumer is asked NOTHING yet: what its
        // declaration reads is not resolved until the provider has answered.
        let (stack_id, request, _) = awaited_request(&engine, stack_name, "Provider")
            .unwrap_or_else(|| panic!("case {case:?}: the provider must be sent a request"));
        let held = awaited_request(&engine, stack_name, "Consumer");
        assert!(
            held.is_none(),
            "case {case:?}: the consumer may not be asked before the provider answers, got \
             {held:?}"
        );
        let (answered, answer_body) = put_response(
            &router,
            &engine,
            &stack_id,
            "Provider",
            &request.request_id,
            &json!({
                "Status": custom_resource::SUCCESS,
                "PhysicalResourceId": "provider-instance",
                "Data": {RETURNED_KEY: RETURNED_VALUE},
            }),
        )
        .await;
        assert_eq!(
            answered, 200,
            "case {case:?}: expected the provider's answer to be accepted, got {answered} with \
             {answer_body}"
        );
        if !returned {
            let (status, body) = settled_with_a_responsive_provider(
                &router,
                &engine,
                case,
                stack_name,
                "provider-instance",
            )
            .await;
            assert_eq!(
                status, "ROLLBACK_COMPLETE",
                "case {case:?}: an attribute the Data lacks must roll the stack back, got \
                 {status} with {body}"
            );
            let reason = recorded_reason(&router, stack_name, "Consumer", "CREATE_FAILED").await;
            assert_eq!(
                reason,
                template::unsupported_attribute_reason("Provider", attribute),
                "case {case:?}: expected CloudFormation's attribute error, got {reason:?}"
            );
            continue;
        }
        // The consumer's own request carries the value the provider published, which is what
        // `Fn::GetAtt` resolved to once the answer was in hand.
        let consumer = awaited_consumer_request(&router, &engine, case, stack_name).await;
        assert_eq!(
            consumer.properties.get("Value").and_then(Value::as_str),
            Some(RETURNED_VALUE),
            "case {case:?}: expected the consumer to be asked with the provider's value, got {:?}",
            consumer.properties
        );
        let (answered, answer_body) = put_response(
            &router,
            &engine,
            &stack_id,
            "Consumer",
            &consumer.request_id,
            &json!({
                "Status": custom_resource::SUCCESS,
                "PhysicalResourceId": "consumer-instance",
            }),
        )
        .await;
        assert_eq!(
            answered, 200,
            "case {case:?}: expected the consumer's answer to be accepted, got {answered} with \
             {answer_body}"
        );
        let (status, body) = settled(&router, case, stack_name).await;
        assert_eq!(
            status, "CREATE_COMPLETE",
            "case {case:?}: both resources answered, so the stack must complete, got {status} \
             with {body}"
        );
    }
}

/// The request the consumer is sent once the provider's answer has released it, polled for:
/// a READ is what makes the provider's answer due, so polling is what a client does.
async fn awaited_consumer_request(
    router: &MultiServiceRouter,
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    case: &str,
    stack_name: &str,
) -> custom_resource::ProviderRequest {
    for _ in 0..POLL_ATTEMPTS {
        let (status_code, body) =
            call(router, "DescribeStacks", &[member("StackName", stack_name)]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected DescribeStacks to answer, got {status_code} with {body}"
        );
        if let Some((_, request, _)) = awaited_request(engine, stack_name, "Consumer") {
            return request;
        }
        assert!(
            xml_text(&body, "StackStatus")
                .as_deref()
                .is_some_and(|status| status.ends_with("_IN_PROGRESS")),
            "case {case:?}: the stack concluded without ever asking the consumer: {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("case {case:?}: the consumer was never sent a request");
}

/// The `ResourceStatusReason` of one resource's event, polled for: a stack's events become
/// readable as its own clock reaches them.
async fn recorded_reason(
    router: &MultiServiceRouter,
    stack_name: &str,
    logical_id: &str,
    status: &str,
) -> String {
    let mut last = String::new();
    for _ in 0..POLL_ATTEMPTS {
        last = resource_event_reason(router, stack_name, logical_id, status).await;
        if !last.is_empty() {
            return last;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    last
}
