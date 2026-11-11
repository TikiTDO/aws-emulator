// SPDX-License-Identifier: CC0-1.0

//! A template's CUSTOM RESOURCE is provisioned by the PROVIDER its `ServiceToken` names.
//!
//! The rule under test: **on every stack operation that acts on a resource of type
//! `Custom::<Name>` or `AWS::CloudFormation::CustomResource`, CloudFormation sends the
//! provider that resource's `ServiceToken` names a request and holds the resource — and the
//! stack operation behind it — in its `*_IN_PROGRESS` status until that provider answers at
//! the request's `ResponseURL`; the answer's `PhysicalResourceId` is what `Ref` resolves to,
//! its `Data` is what `Fn::GetAtt` resolves against, and a `FAILED` answer fails the
//! resource with the reason it gave.**
//!
//! Every assertion drives the emulator through its own wire: the stack operations over
//! CloudFormation's awsQuery endpoint, and the provider's answer as the plain, unsigned HTTP
//! `PUT` to the published `ResponseURL` a provider really makes. What the provider was SENT
//! is read off the request this emulator is waiting for an answer to, which is the record
//! the `ResponseURL` is addressed by.
//!
//! The helpers this suite builds the sequence out of — the emulator with the provider's
//! topic, the template, the request CloudFormation is waiting for an answer to and the
//! unsigned write that answers it — are `pub` because the sibling suite
//! `custom_resource_response_identity.rs` drives the same sequence to assert what the
//! ANSWER then makes of the resource; one spelling of the sequence keeps the two from
//! drifting about how a provider is answered.
//!
//! The rows differ along what the rule generalises over: the `RequestType` the operation
//! sends (`Create`, `Update`, `Delete`), the spelling of the custom resource's type (the
//! author's own namespace and the generic one), what the answer's `Data` carries (nothing, a
//! string, a value that is not a string, several entries), whether an `Update` is answered
//! with the same `PhysicalResourceId` or a new one — which is what makes it a replacement —
//! and, for the refusals, the method, the path and the body of the write.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::provisioning::custom_resource;
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    call, create_stack, member, router_with_engine, store, wait_for_the_transition, xml_text,
    xml_texts, REGION,
};
use super::stack_lifecycle::form_encode;

/// The topic every suite row names as its `ServiceToken`: an Amazon SNS topic this emulator
/// really holds, created through SNS's own wire before the stack is deployed, so the request
/// CloudFormation publishes is one SNS accepts.
pub const PROVIDER_TOPIC: &str = "custom-resource-provider";

/// The `ServiceToken` a template names that provider by: the ARN of [`PROVIDER_TOPIC`].
///
/// `pub` for the same reason the helpers below are: a sibling suite deploys templates of its
/// own against the SAME provider, and one spelling of the token keeps the two from naming
/// different ones.
pub fn provider_token() -> String {
    format!("arn:aws:sns:{REGION}:000000000000:{PROVIDER_TOPIC}")
}

/// How long one poll of a stack's status waits before asking again, and how many times: the
/// stack's own transition is clock-owed, and what makes a released wait due is a READ.
pub const POLL_INTERVAL: Duration = Duration::from_millis(100);
pub const POLL_ATTEMPTS: usize = 40;

/// The suffix every status a stack operation in flight carries ends in.
const IN_PROGRESS: &str = "_IN_PROGRESS";

/// One awsQuery call of Amazon SNS, which must be answered.
async fn sns_ok(router: &MultiServiceRouter, case: &str, action: &str, members: &[(&str, String)]) {
    let mut body = format!("Action={action}&Version=2010-03-31");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sns.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/sns/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SNS {action}, got {status} with {answered}"
    );
}

/// The provider request this emulator is waiting for an answer to about `logical_id` of
/// `stack_name`, with the stack's own id: exactly what the published `ResponseURL` is
/// addressed by.
pub fn awaited_request(
    engine: &Arc<engine::InMemoryEngine>,
    stack_name: &str,
    logical_id: &str,
) -> Option<(String, custom_resource::ProviderRequest, String)> {
    let held = store(engine);
    let held = held.lock();
    let stack = held
        .stacks
        .iter()
        .find(|stack| stack.stack_name == stack_name)?;
    let barrier = stack
        .stabilisations
        .iter()
        .find(|barrier| {
            !barrier.released()
                && barrier.logical_id == logical_id
                && barrier.provider_request.is_some()
        })?;
    Some((
        stack.stack_id.clone(),
        barrier.provider_request.clone()?,
        barrier.physical_resource_id.clone(),
    ))
}

/// PUT one document to the `ResponseURL` the request `request_id` about `logical_id` of the
/// stack `stack_id` was published under — the plain, unsigned write a provider makes.
///
/// The URL is the one this emulator PUBLISHED for that request, read off the request
/// CloudFormation is waiting for an answer to: it is query-signed by the emulator's own
/// signer, so no suite can spell it and the published value is the only one a provider ever
/// has. A target no request awaits has no published URL, and the write goes to that target's
/// own address ([`unawaited_response_url`]).
pub async fn put_response(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    stack_id: &str,
    logical_id: &str,
    request_id: &str,
    document: &Value,
) -> (u16, String) {
    let url = published_response_url(engine, stack_id, logical_id, request_id);
    put(router, "PUT", &url, document.to_string()).await
}

/// The `ResponseURL` this emulator published for one request, or that target's own address
/// when nothing awaits it ([`unawaited_response_url`]).
pub fn published_response_url(
    engine: &Arc<engine::InMemoryEngine>,
    stack_id: &str,
    logical_id: &str,
    request_id: &str,
) -> String {
    let target = custom_resource::ResponseTarget {
        stack_id: stack_id.to_string(),
        logical_resource_id: logical_id.to_string(),
        request_id: request_id.to_string(),
    };
    store(engine)
        .lock()
        .awaited_response_url(REGION, &target)
        .unwrap_or_else(|| unawaited_response_url(stack_id, logical_id, request_id))
}

/// The address of a response target this emulator published no URL for: the plane's own path
/// for it on the endpoint a test router serves at, with no signing query — which is what a
/// row writing to a target nothing is waiting for an answer to sends.
pub fn unawaited_response_url(stack_id: &str, logical_id: &str, request_id: &str) -> String {
    format!(
        "{base}{path}",
        base = smithy_server_runtime::UNSERVED_URL_BASE,
        path = custom_resource::response_path(stack_id, logical_id, request_id),
    )
}

/// One raw write to a published URL: no credential of any kind, as a pre-signed URL is
/// followed.
async fn put(
    router: &MultiServiceRouter,
    method: &str,
    url: &str,
    body: String,
) -> (u16, String) {
    let host = url
        .strip_prefix("https://")
        .and_then(|rest| rest.split('/').next())
        .unwrap_or_default()
        .to_string();
    let request = http::Request::builder()
        .method(method)
        .uri(url)
        .header("host", host)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (
        status,
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// `DescribeStacks` for one stack: its status and the whole body.
async fn describe(router: &MultiServiceRouter, stack_name: &str) -> (String, String) {
    let (status_code, body) = call(router, "DescribeStacks", &[member("StackName", stack_name)]).await;
    assert_eq!(
        status_code, 200,
        "expected DescribeStacks to answer for {stack_name:?}, got {status_code} with {body}"
    );
    (xml_text(&body, "StackStatus").unwrap_or_default(), body)
}

/// Poll the stack until it leaves every `*_IN_PROGRESS` status, and answer the status it
/// settled in with the body that reported it.
///
/// A read is what asks the provider's answer about, so polling is what a client does.
pub async fn settled(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
) -> (String, String) {
    let mut last = (String::new(), String::new());
    for _ in 0..POLL_ATTEMPTS {
        last = describe(router, stack_name).await;
        if !last.0.ends_with(IN_PROGRESS) {
            return last;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "case {case:?}: {stack_name:?} never left an in-progress status; last was {:?} with {}",
        last.0, last.1
    );
}

/// Poll the stack until it leaves every `*_IN_PROGRESS` status, answering every provider
/// request it is waiting for with `SUCCESS` — a provider that answers whatever
/// CloudFormation sends it, which is what a stack operation needs to conclude.
///
/// Answers under the id the request names where it names one (an `Update` or a `Delete`), so
/// the answer replaces nothing, and under `minted` for a `Create`.
pub async fn settled_with_a_responsive_provider(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    stack_name: &str,
    minted: &str,
) -> (String, String) {
    let mut last = (String::new(), String::new());
    for _ in 0..POLL_ATTEMPTS {
        last = describe(router, stack_name).await;
        if !last.0.ends_with(IN_PROGRESS) {
            return last;
        }
        if let Some((stack_id, request, logical_id)) = any_awaited_request(engine, stack_name) {
            let id = request
                .physical_resource_id
                .clone()
                .unwrap_or_else(|| minted.to_string());
            put_response(
                router,
                engine,
                &stack_id,
                &logical_id,
                &request.request_id,
                &json!({ "Status": "SUCCESS", "PhysicalResourceId": id }),
            )
            .await;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "case {case:?}: {stack_name:?} never left an in-progress status; last was {:?} with {}",
        last.0, last.1
    );
}

/// The provider request `stack_name` is waiting for an answer to, whichever resource it is
/// about: the stack's id, the request, and the logical id it names.
fn any_awaited_request(
    engine: &Arc<engine::InMemoryEngine>,
    stack_name: &str,
) -> Option<(String, custom_resource::ProviderRequest, String)> {
    let held = store(engine);
    let held = held.lock();
    let stack = held
        .stacks
        .iter()
        .find(|stack| stack.stack_name == stack_name)?;
    let barrier = stack.stabilisations.iter().find(|barrier| {
        !barrier.released()
            && barrier.provider_response.is_none()
            && barrier.provider_request.is_some()
    })?;
    Some((
        stack.stack_id.clone(),
        barrier.provider_request.clone()?,
        barrier.logical_id.clone(),
    ))
}

/// A stack's reported outputs, as the key/value map a client reads.
pub fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    xml_texts(xml, "OutputKey")
        .into_iter()
        .zip(xml_texts(xml, "OutputValue"))
        .collect()
}

/// The template one row deploys: one custom resource of `resource_type` whose `ServiceToken`
/// is the provider topic, with `marker` as the property a revision changes, and outputs that
/// publish what `Ref` and `Fn::GetAtt` resolve to for it.
pub fn template(resource_type: &str, marker: &str, attributes: &[&str]) -> String {
    let mut outputs = serde_json::Map::new();
    outputs.insert("ThingRef".to_string(), json!({ "Value": { "Ref": "Thing" } }));
    for attribute in attributes {
        outputs.insert(
            format!("Thing{attribute}"),
            json!({ "Value": { "Fn::GetAtt": ["Thing", attribute] } }),
        );
    }
    json!({
        "Resources": {
            "Thing": {
                "Type": resource_type,
                "Properties": {
                    "ServiceToken": provider_token(),
                    "Marker": marker,
                },
            },
        },
        "Outputs": Value::Object(outputs),
    })
    .to_string()
}

/// The emulator with the provider's topic already created in Amazon SNS.
pub async fn emulator(case: &str) -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = router_with_engine();
    sns_ok(
        &router,
        case,
        "CreateTopic",
        &[("Name", PROVIDER_TOPIC.to_string())],
    )
    .await;
    (router, engine)
}

/// What one row's provider answers a `Create` with, and what the stack must then report.
struct Answered {
    case: &'static str,
    stack_name: &'static str,
    resource_type: &'static str,
    /// The `PhysicalResourceId` the provider mints for the resource.
    physical_resource_id: &'static str,
    /// The `Data` it answers with, and so what `Fn::GetAtt` must resolve against.
    data: fn() -> Value,
    /// The attribute names the template publishes, with the text each must resolve to.
    expected: &'static [(&'static str, &'static str)],
}

const ANSWERS: &[Answered] = &[
    Answered {
        case: "a provider answering with no Data at all",
        stack_name: "custom-no-data",
        resource_type: "Custom::Probe",
        physical_resource_id: "probe-instance-1",
        data: || json!({}),
        expected: &[],
    },
    Answered {
        case: "a provider answering with one string value",
        stack_name: "custom-one-value",
        resource_type: "Custom::DomainProvisioner",
        physical_resource_id: "domain-7f3c",
        data: || json!({ "Endpoint": "https://provisioned.example.invalid" }),
        expected: &[("Endpoint", "https://provisioned.example.invalid")],
    },
    Answered {
        case: "a provider answering with several values, one of them not a string",
        stack_name: "custom-many-values",
        resource_type: "AWS::CloudFormation::CustomResource",
        physical_resource_id: "arn:aws:service:us-east-1:000000000000:thing/42",
        data: || json!({ "Name": "forty-two", "Count": 42, "Ready": true }),
        expected: &[("Name", "forty-two"), ("Count", "42"), ("Ready", "true")],
    },
];

/// Every row: the `Create` request reaches the provider, the resource is HELD until the
/// provider answers, and what the answer says the resource is becomes what the stack
/// reports — the `PhysicalResourceId` for `Ref` and the `Data` for `Fn::GetAtt`.
#[tokio::test]
async fn a_custom_resource_is_what_its_provider_answered() {
    for row in ANSWERS {
        let (router, engine) = emulator(row.case).await;
        let attributes: Vec<&str> = row.expected.iter().map(|(name, _)| *name).collect();
        let body = template(row.resource_type, "v1", &attributes);
        let (status_code, created) = create_stack(&router, row.stack_name, &body, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: expected CreateStack to be accepted, got {status_code} with {created}",
            row.case
        );

        // The resource is HELD: nothing has answered for it, so the stack is still creating
        // even once its own clock-owed span has elapsed.
        wait_for_the_transition().await;
        let (status, described) = describe(&router, row.stack_name).await;
        assert_eq!(
            status, "CREATE_IN_PROGRESS",
            "case {:?}: the stack must be held until the provider answers, got {described}",
            row.case
        );

        // And the request it is held for is a `Create` naming no physical resource — nothing
        // existed when it was sent.
        let (stack_id, request, stand_in) = awaited_request(&engine, row.stack_name, "Thing")
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the stack must be waiting for a provider request, got {described}",
                    row.case
                )
            });
        assert_eq!(
            request.request_type, "Create",
            "case {:?}: expected a Create request, got {request:?}",
            row.case
        );
        assert_eq!(
            request.physical_resource_id, None,
            "case {:?}: a Create names no physical resource, got {request:?}",
            row.case
        );

        let (answered, answer_body) = put_response(
            &router,
            &engine,
            &stack_id,
            "Thing",
            &request.request_id,
            &json!({
                "Status": "SUCCESS",
                "PhysicalResourceId": row.physical_resource_id,
                "Data": (row.data)(),
            }),
        )
        .await;
        assert_eq!(
            answered, 200,
            "case {:?}: expected the provider's answer to be accepted, got {answered} with \
             {answer_body}",
            row.case
        );

        let (status, described) = settled(&router, row.case, row.stack_name).await;
        assert_eq!(
            status, "CREATE_COMPLETE",
            "case {:?}: the answered resource must complete the stack, got {described}",
            row.case
        );
        let outputs = outputs_of(&described);
        assert_eq!(
            outputs.get("ThingRef").map(String::as_str),
            Some(row.physical_resource_id),
            "case {:?}: Ref must resolve to the id the provider answered with (not the \
             stand-in {stand_in:?}), got {outputs:?}",
            row.case
        );
        for (attribute, value) in row.expected {
            assert_eq!(
                outputs.get(&format!("Thing{attribute}")).map(String::as_str),
                Some(*value),
                "case {:?}: Fn::GetAtt {attribute:?} must resolve to the answered Data, got \
                 {outputs:?}",
                row.case
            );
        }
    }
}

/// A provider that answers `FAILED` fails the resource with the reason it gave, and the
/// stack is rolled back — exactly as a refusal from an owning service fails a create.
///
/// The rows differ in whether the answer states a reason at all.
#[tokio::test]
async fn a_provider_that_answers_failed_fails_the_resource() {
    let rows: &[(&str, &str, Option<&str>)] = &[
        (
            "a failure stating its own reason",
            "custom-failed-stated",
            Some("the provisioner could not reach its backend"),
        ),
        ("a failure stating no reason", "custom-failed-silent", None),
    ];
    for (case, stack_name, reason) in rows {
        let (router, engine) = emulator(case).await;
        let body = template("Custom::Probe", "v1", &[]);
        let (status_code, created) = create_stack(&router, stack_name, &body, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to be accepted, got {status_code} with {created}"
        );
        let (stack_id, request, _) = awaited_request(&engine, stack_name, "Thing")
            .unwrap_or_else(|| panic!("case {case:?}: the stack must be waiting for a request"));
        let mut answer = json!({ "Status": "FAILED", "PhysicalResourceId": "probe-failed" });
        if let Some(reason) = reason {
            answer["Reason"] = json!(reason);
        }
        let (answered, answer_body) =
            put_response(&router, &engine, &stack_id, "Thing", &request.request_id, &answer)
                .await;
        assert_eq!(
            answered, 200,
            "case {case:?}: expected the provider's answer to be accepted, got {answered} with \
             {answer_body}"
        );

        // A failed create is ROLLED BACK, and the rollback sends the provider the `Delete`
        // it owes for the resource the create had asked it for. The READ that learns of the
        // FAILED answer is what performs that rollback.
        let (_, described) = describe(&router, stack_name).await;
        let rollback = any_awaited_request(&engine, stack_name);
        assert_eq!(
            rollback
                .as_ref()
                .map(|(_, request, _)| request.request_type.as_str()),
            Some("Delete"),
            "case {case:?}: the rollback must send the provider a Delete, got {rollback:?} \
             with {described}"
        );
        let (status, described) =
            settled_with_a_responsive_provider(&router, &engine, case, stack_name, "probe-failed")
                .await;
        assert_eq!(
            status, "ROLLBACK_COMPLETE",
            "case {case:?}: a FAILED answer must fail the create, got {described}"
        );
        assert!(
            xml_text(&described, "StackStatusReason")
                .unwrap_or_default()
                .contains("Thing"),
            "case {case:?}: the stack must report which resource failed, got {described}"
        );
        // The provider's own reason is what the resource's event carries.
        let (events_status, events) = call(
            &router,
            "DescribeStackEvents",
            &[member("StackName", stack_name)],
        )
        .await;
        assert_eq!(
            events_status, 200,
            "case {case:?}: expected DescribeStackEvents to answer, got {events_status} {events}"
        );
        let expected = reason.unwrap_or("The custom resource provider reported FAILED");
        assert!(
            events.contains(expected),
            "case {case:?}: the resource's failure must carry the provider's reason \
             {expected:?}, got {events}"
        );
    }
}

/// An `Update` the provider answers with a NEW `PhysicalResourceId` is a REPLACEMENT: the
/// stack keeps the new physical resource and a cleanup `Delete` is sent for the OLD one,
/// carrying the OLD properties. The SAME id is an in-place update and owes no `Delete`.
#[tokio::test]
async fn an_update_answered_with_a_new_physical_id_replaces_the_resource() {
    // (case, stack name, the id the Update is answered with, whether that is a replacement)
    let rows: &[(&str, &str, &str, bool)] = &[
        (
            "an update answered with the same physical id",
            "custom-in-place",
            "probe-instance-1",
            false,
        ),
        (
            "an update answered with a new physical id",
            "custom-replaced",
            "probe-instance-2",
            true,
        ),
    ];
    const FIRST_ID: &str = "probe-instance-1";
    for (case, stack_name, updated_id, replaces) in rows {
        let (router, engine) = emulator(case).await;
        let created_body = template("Custom::Probe", "v1", &[]);
        let (status_code, created) = create_stack(&router, stack_name, &created_body, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to be accepted, got {status_code} with {created}"
        );
        let (stack_id, request, _) = awaited_request(&engine, stack_name, "Thing")
            .unwrap_or_else(|| panic!("case {case:?}: the create must be waiting for a request"));
        put_response(
            &router,
            &engine,
            &stack_id,
            "Thing",
            &request.request_id,
            &json!({ "Status": "SUCCESS", "PhysicalResourceId": FIRST_ID }),
        )
        .await;
        let (status, described) = settled(&router, case, stack_name).await;
        assert_eq!(
            status, "CREATE_COMPLETE",
            "case {case:?}: the created resource must complete the stack, got {described}"
        );

        // A revision that changes the resource's properties sends the provider an `Update`
        // naming the resource it already has and the properties it was declared with.
        let revision = template("Custom::Probe", "v2", &[]);
        let (status_code, updated) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", stack_name),
                member("TemplateBody", &revision),
            ],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected UpdateStack to be accepted, got {status_code} with {updated}"
        );
        let (_, update_request, _) = awaited_request(&engine, stack_name, "Thing")
            .unwrap_or_else(|| panic!("case {case:?}: the update must be waiting for a request"));
        assert_eq!(
            update_request.request_type, "Update",
            "case {case:?}: expected an Update request, got {update_request:?}"
        );
        assert_eq!(
            update_request.physical_resource_id.as_deref(),
            Some(FIRST_ID),
            "case {case:?}: the Update must name the resource that exists, got \
             {update_request:?}"
        );
        assert_eq!(
            update_request
                .old_properties
                .as_ref()
                .and_then(|old| old.get("Marker"))
                .and_then(Value::as_str),
            Some("v1"),
            "case {case:?}: the Update must carry the properties the resource was last \
             declared with, got {update_request:?}"
        );
        assert_eq!(
            update_request.properties.get("Marker").and_then(Value::as_str),
            Some("v2"),
            "case {case:?}: the Update must carry the newly declared properties, got \
             {update_request:?}"
        );

        put_response(
            &router,
            &engine,
            &stack_id,
            "Thing",
            &update_request.request_id,
            &json!({ "Status": "SUCCESS", "PhysicalResourceId": updated_id }),
        )
        .await;

        // A replacement owes a cleanup `Delete` of the OLD physical resource, with the OLD
        // properties — and the update is not reported complete until the provider answers it.
        if *replaces {
            let (status, described) = describe(&router, stack_name).await;
            assert!(
                status.ends_with(IN_PROGRESS),
                "case {case:?}: the update must be held while the replaced resource is \
                 cleaned up, got {described}"
            );
            let (_, cleanup, old_id) = awaited_request(&engine, stack_name, "Thing")
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a replacement must owe a cleanup Delete, got {described}")
                });
            assert_eq!(
                cleanup.request_type, "Delete",
                "case {case:?}: the cleanup must be a Delete, got {cleanup:?}"
            );
            assert_eq!(
                (cleanup.physical_resource_id.as_deref(), old_id.as_str()),
                (Some(FIRST_ID), FIRST_ID),
                "case {case:?}: the cleanup must address the resource that was replaced, got \
                 {cleanup:?}"
            );
            assert_eq!(
                cleanup.properties.get("Marker").and_then(Value::as_str),
                Some("v1"),
                "case {case:?}: the cleanup must carry the OLD properties, got {cleanup:?}"
            );
            put_response(
                &router,
                &engine,
                &stack_id,
                "Thing",
                &cleanup.request_id,
                &json!({ "Status": "SUCCESS", "PhysicalResourceId": FIRST_ID }),
            )
            .await;
        }

        let (status, described) = settled(&router, case, stack_name).await;
        assert_eq!(
            status, "UPDATE_COMPLETE",
            "case {case:?}: the answered update must complete, got {described}"
        );
        let outputs = outputs_of(&described);
        assert_eq!(
            outputs.get("ThingRef").map(String::as_str),
            Some(*updated_id),
            "case {case:?}: Ref must resolve to the id the Update was answered with, got \
             {outputs:?}"
        );
        // An in-place update owes no cleanup at all.
        if !replaces {
            let pending = awaited_request(&engine, stack_name, "Thing");
            assert!(
                pending.is_none(),
                "case {case:?}: an in-place update owes no further request, got {pending:?}"
            );
        }
    }
}

/// A stack's DELETION sends the provider a `Delete` naming the physical resource it minted,
/// and the deletion is not reported complete until the provider answers.
#[tokio::test]
async fn a_stacks_deletion_sends_its_custom_resource_provider_a_delete() {
    let case = "a stack deletion";
    let stack_name = "custom-deleted";
    let (router, engine) = emulator(case).await;
    let body = template("Custom::Probe", "v1", &[]);
    let (status_code, created) = create_stack(&router, stack_name, &body, &[]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected CreateStack to be accepted, got {status_code} with {created}"
    );
    let (stack_id, request, _) = awaited_request(&engine, stack_name, "Thing")
        .unwrap_or_else(|| panic!("case {case:?}: the create must be waiting for a request"));
    put_response(
        &router,
        &engine,
        &stack_id,
        "Thing",
        &request.request_id,
        &json!({ "Status": "SUCCESS", "PhysicalResourceId": "probe-deleted" }),
    )
    .await;
    let (status, described) = settled(&router, case, stack_name).await;
    assert_eq!(
        status, "CREATE_COMPLETE",
        "case {case:?}: the created resource must complete the stack, got {described}"
    );

    let (status_code, deleted) =
        call(&router, "DeleteStack", &[member("StackName", stack_name)]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected DeleteStack to be accepted, got {status_code} with {deleted}"
    );
    let (_, delete_request, _) = awaited_request(&engine, stack_name, "Thing")
        .unwrap_or_else(|| panic!("case {case:?}: the deletion must be waiting for a request"));
    assert_eq!(
        delete_request.request_type, "Delete",
        "case {case:?}: expected a Delete request, got {delete_request:?}"
    );
    assert_eq!(
        delete_request.physical_resource_id.as_deref(),
        Some("probe-deleted"),
        "case {case:?}: the Delete must name the resource the provider minted, got \
         {delete_request:?}"
    );
    let (status, described) = describe(&router, stack_name).await;
    assert_eq!(
        status, "DELETE_IN_PROGRESS",
        "case {case:?}: the deletion must be held until the provider answers, got {described}"
    );

    put_response(
        &router,
        &engine,
        &stack_id,
        "Thing",
        &delete_request.request_id,
        &json!({ "Status": "SUCCESS", "PhysicalResourceId": "probe-deleted" }),
    )
    .await;
    for _ in 0..POLL_ATTEMPTS {
        let (status_code, body) =
            call(&router, "DescribeStacks", &[member("StackName", stack_name)]).await;
        if status_code == 200
            && xml_text(&body, "StackStatus").as_deref() == Some("DELETE_COMPLETE")
        {
            return;
        }
        // A deleted stack is no longer resolvable by name, which is the other way this
        // deletion is observed as complete.
        if status_code == 400 {
            return;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("case {case:?}: the answered deletion must complete");
}

/// A write to a `ResponseURL` that names no request this emulator is waiting for an answer
/// to changes nothing, and a write that is not the one the URL describes is REFUSED: the URL
/// answers ONE request of one resource of one stack.
///
/// The write itself is ACCEPTED whether or not CloudFormation is still waiting for it — the
/// published URL is the target a pre-signed `PUT` goes to, so a provider is never told its
/// own answer failed — which is what the first rows pin: they are answered, and the resource
/// they did not answer is still held.
#[tokio::test]
async fn a_write_that_answers_no_awaited_request_is_refused() {
    let case = "the plane's refusals";
    let stack_name = "custom-refusals";
    let (router, engine) = emulator(case).await;
    let body = template("Custom::Probe", "v1", &[]);
    let (status_code, created) = create_stack(&router, stack_name, &body, &[]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected CreateStack to be accepted, got {status_code} with {created}"
    );
    let (stack_id, request, _) = awaited_request(&engine, stack_name, "Thing")
        .unwrap_or_else(|| panic!("case {case:?}: the create must be waiting for a request"));
    let awaited = published_response_url(&engine, &stack_id, "Thing", &request.request_id);
    let good = json!({ "Status": "SUCCESS", "PhysicalResourceId": "probe-1" }).to_string();

    // (case, method, the URL written to, the body, the status the plane answers)
    let rows: Vec<(&str, &str, String, String, u16)> = vec![
        (
            "a request id nothing is waiting for",
            "PUT",
            unawaited_response_url(
                &stack_id,
                "Thing",
                "00000000-0000-0000-0000-000000000000",
            ),
            good.clone(),
            200,
        ),
        (
            "another resource of the same stack",
            "PUT",
            unawaited_response_url(&stack_id, "Other", &request.request_id),
            good.clone(),
            200,
        ),
        (
            "a stack this emulator does not hold",
            "PUT",
            unawaited_response_url(
                "arn:aws:cloudformation:us-east-1:000000000000:stack/absent/0000",
                "Thing",
                &request.request_id,
            ),
            good.clone(),
            200,
        ),
        (
            "a read of the write target",
            "GET",
            awaited.clone(),
            String::new(),
            405,
        ),
        (
            // The URL authorises ONE write: a target assembled by a caller — here the
            // awaited one with its signature altered — carries no signature this service
            // published for it.
            "a signature altered after the URL was published",
            "PUT",
            awaited.replace("X-Amz-Signature=", "X-Amz-Signature=0"),
            good.clone(),
            403,
        ),
        (
            "no signature at all",
            "PUT",
            awaited
                .split('?')
                .next()
                .unwrap_or_default()
                .to_string(),
            good.clone(),
            403,
        ),
        (
            "a body that is not JSON at all",
            "PUT",
            awaited.clone(),
            "not a document".to_string(),
            400,
        ),
        (
            "a document stating no Status",
            "PUT",
            awaited.clone(),
            json!({ "PhysicalResourceId": "probe-1" }).to_string(),
            400,
        ),
        (
            "a document whose Status is outside the two the protocol admits",
            "PUT",
            awaited.clone(),
            json!({ "Status": "Succeeded", "PhysicalResourceId": "probe-1" }).to_string(),
            400,
        ),
        (
            "a document stating no PhysicalResourceId",
            "PUT",
            awaited.clone(),
            json!({ "Status": "SUCCESS" }).to_string(),
            400,
        ),
    ];
    for (row, method, url, body, expected) in rows {
        let (status_code, answered) = put(&router, method, &url, body).await;
        assert_eq!(
            status_code, expected,
            "case {row:?}: expected HTTP {expected} from the response plane, got {status_code} \
             with {answered}"
        );
    }

    // None of them answered the request, so the resource is still held — and the real answer
    // is still accepted.
    let (status, described) = describe(&router, stack_name).await;
    assert_eq!(
        status, "CREATE_IN_PROGRESS",
        "case {case:?}: a write that answered nothing must leave the resource held, got \
         {described}"
    );
    let (status_code, answered) = put(&router, "PUT", &awaited, good).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: the awaited request must still be answerable, got {status_code} with \
         {answered}"
    );
    // ...and a SECOND write to the same URL changes nothing: the first answer stands.
    let (status_code, answered) = put(
        &router,
        "PUT",
        &awaited,
        json!({ "Status": "FAILED", "PhysicalResourceId": "probe-1" }).to_string(),
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: a second write to a published URL is accepted too, got {status_code} \
         with {answered}"
    );
    let (status, described) = settled(&router, case, stack_name).await;
    assert_eq!(
        status, "CREATE_COMPLETE",
        "case {case:?}: the first answer is the one that stands, got {described}"
    );
}
