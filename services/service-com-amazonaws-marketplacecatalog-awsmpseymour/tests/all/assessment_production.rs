// SPDX-License-Identifier: CC0-1.0

//! The assessments a change set PRODUCES, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path
//! a client takes. Nothing here seeds the store: the assessments read back are the ones
//! the change sets started below produced.
//!
//! The rule under test: an assessment is the result of evaluating a product against a
//! framework, so a succeeded `Intent=APPLY` change set that submits a delivery option
//! of a product one of this catalog's frameworks covers (an AMI product against AMI
//! Security, a container product against Container Security) produces ONE assessment
//! per submitted delivery option — reported by `ListAssessments` and detailed, control
//! by control, by `DescribeAssessment` — while a change that submits no delivery
//! option, or one against an entity type no framework covers, produces none.
//!
//! Each control's result is what evaluating the SUBMITTED details found: `PASS` when
//! its condition held, `FAIL` with the error that says why when it did not, and
//! `NOT_EXECUTED` when the delivery option carried nothing for it to read.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::state::{
    is_iso8601, ASSESSMENT_QUOTA, CHANGE_SET_APPLYING, CHANGE_SET_PREPARING,
    MAX_DELIVERY_OPTIONS_PER_CHANGE,
};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The catalog every operation's `Catalog` member documents as its fixed value.
const CATALOG: &str = "AWSMarketplace";

/// A SigV4 credential scope naming this service's signing name (`aws-marketplace`),
/// which is what routes a request here.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The controls of the AMI Security framework, in the order an assessment reports
/// them. Spelled out here rather than imported from the crate, so a renamed or
/// reordered control fails this test instead of following it.
const AMI_CONTROLS: &[&str] = &[
    "AmiSourceImageDeclared",
    "AmiSourceAccessRoleDeclared",
    "AmiSourceLoginUserIsNotSuperuser",
    "AmiSecurityGroupIngressIsScoped",
    "AmiUsageInstructionsDeclared",
];

/// The controls of the Container Security framework, in the same way.
const CONTAINER_CONTROLS: &[&str] = &[
    "ContainerImagesDeclared",
    "ContainerImagesArePinned",
    "ContainerUsageInstructionsDeclared",
];

/// How long after the start a change set has certainly reached `SUCCEEDED`, so what
/// applying its changes produced is observable.
fn wait_for_success() {
    std::thread::sleep(CHANGE_SET_PREPARING + CHANGE_SET_APPLYING + Duration::from_millis(400));
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        let raw = self.body["__type"].as_str().unwrap_or_default();
        raw.rsplit('#').next().unwrap_or(raw)
    }

    /// The string at `pointer`, or the empty string.
    fn text(&self, pointer: &str) -> String {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The array at `pointer`, or an empty one when the member is absent.
    fn list(&self, pointer: &str) -> Vec<Value> {
        self.body
            .pointer(pointer)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "catalog.marketplace.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// Start an `Intent`ed change set of ONE change against a new entity of
/// `entity_type`, whose details are `details`, and report the change set id.
async fn start(
    router: &MultiServiceRouter,
    entity_type: &str,
    details: &Value,
    intent: &str,
) -> String {
    let answer = try_start(router, entity_type, details, intent).await;
    assert_eq!(
        answer.status, 200,
        "starting a {entity_type} change set must be accepted, got {} with {:?}",
        answer.status, answer.body
    );
    answer.text("/ChangeSetId")
}

/// Start a change set of ONE change against a new entity of `entity_type` whose
/// details are `details`, without judging the answer.
async fn try_start(
    router: &MultiServiceRouter,
    entity_type: &str,
    details: &Value,
    intent: &str,
) -> Answer {
    call(
        router,
        "POST",
        "/StartChangeSet",
        Some(json!({
            "Catalog": CATALOG,
            "Intent": intent,
            "ChangeSet": [{
                "ChangeType": "AddDeliveryOptions",
                "Entity": {"Type": format!("{entity_type}@1.0")},
                "Details": details.to_string(),
            }],
        })),
    )
    .await
}

/// The assessments of change set `change_set_id`, as `ListAssessments` reports them
/// through the target filter the model declares for exactly this. The page asked for
/// is the largest the model allows, so one call reports every assessment of one
/// change set.
async fn assessments_of(router: &MultiServiceRouter, change_set_id: &str) -> Answer {
    let answer = call(
        router,
        "POST",
        "/ListAssessments",
        Some(json!({
            "Catalog": CATALOG,
            "MaxResults": 100,
            "AssessmentTargetFilter": {"ChangeSetId": change_set_id},
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "listing the assessments of {change_set_id} must answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer
}

/// How many assessments this catalog holds, counted by paging the whole list.
async fn count_all_assessments(router: &MultiServiceRouter) -> usize {
    let mut total = 0usize;
    let mut token = String::new();
    loop {
        let mut request = json!({"Catalog": CATALOG, "MaxResults": 100});
        if !token.is_empty() {
            request["NextToken"] = json!(token);
        }
        let page = call(router, "POST", "/ListAssessments", Some(request)).await;
        assert_eq!(
            page.status, 200,
            "paging the assessments must answer 200, got {} with {:?}",
            page.status, page.body
        );
        total += page.list("/AssessmentSummaryList").len();
        token = page.text("/NextToken");
        if token.is_empty() {
            return total;
        }
    }
}

/// One delivery option's details, in the form a change submits them: the delivery
/// form's own members wrapped in the member naming that form.
fn delivery_option(form: &str, details: Value) -> Value {
    json!({"Details": {form: details}})
}

/// An AMI delivery option whose login user is `user_name` and whose single ingress
/// rule admits `ip_range`, with every other member a satisfied control reads.
fn ami_option(user_name: &str, ip_range: &str) -> Value {
    delivery_option(
        "AmiDeliveryOptionDetails",
        json!({
            "AmiSource": {
                "AmiId": "ami-0abcdef1234567890",
                "AccessRoleArn": "arn:aws:iam::123456789012:role/MarketplaceScan",
                "UserName": user_name,
                "OperatingSystemName": "AMAZONLINUX",
                "OperatingSystemVersion": "2023",
            },
            "RecommendedInstanceType": "t3.medium",
            "SecurityGroups": [{
                "IpProtocol": "tcp",
                "IpRanges": [ip_range],
                "FromPort": 22,
                "ToPort": 22,
            }],
            "UsageInstructions": "Connect over SSH and run /opt/product/start.",
        }),
    )
}

/// A container delivery option delivering `images`.
fn container_option(images: Value) -> Value {
    delivery_option(
        "EcrDeliveryOptionDetails",
        json!({
            "ContainerImages": images,
            "UsageInstructions": "helm install product ./chart",
        }),
    )
}

/// The details of a change that submits `options` as a product version.
fn version_with(options: Vec<Value>) -> Value {
    json!({
        "Version": {"VersionTitle": "1.0", "ReleaseNotes": "first release"},
        "DeliveryOptions": options,
    })
}

/// The `(ControlId, ControlAssessmentResult)` pairs a `DescribeAssessment` reported.
fn control_results(answer: &Answer) -> Vec<(String, String)> {
    answer
        .list("/ControlAssessments")
        .iter()
        .map(|control| {
            (
                control["ControlId"].as_str().unwrap_or_default().to_string(),
                control["ControlAssessmentResult"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// A succeeded `APPLY` change set assesses every delivery option it submits against
/// the framework that covers the product's type, and each control's result is what
/// evaluating the submitted details found.
///
/// The rows differ along what the rule generalises over: which product type and so
/// which framework, which member of the delivery option is at fault, whether the
/// fault is a missing member or a refused value, and whether a control had anything
/// to read at all.
#[tokio::test]
async fn a_succeeded_change_set_assesses_the_delivery_options_it_submits() {
    let router = emulator();

    // (case, entity type, framework, submitted details, the control results the
    // report must carry, in order)
    let rows: Vec<(&str, &str, &str, Value, Vec<&str>)> = vec![
        (
            "an AMI option that satisfies every control",
            "AmiProduct",
            "AMISecurity",
            version_with(vec![ami_option("ec2-user", "10.0.0.0/16")]),
            vec!["PASS", "PASS", "PASS", "PASS", "PASS"],
        ),
        (
            "an AMI option whose ingress admits every IPv4 address",
            "AmiProduct",
            "AMISecurity",
            version_with(vec![ami_option("ec2-user", "0.0.0.0/0")]),
            vec!["PASS", "PASS", "PASS", "FAIL", "PASS"],
        ),
        (
            "an AMI option whose ingress admits every IPv6 address",
            "AmiProduct",
            "AMISecurity",
            version_with(vec![ami_option("ec2-user", "::/0")]),
            vec!["PASS", "PASS", "PASS", "FAIL", "PASS"],
        ),
        (
            "an AMI option offering the superuser as its login user",
            "AmiProduct",
            "AMISecurity",
            version_with(vec![ami_option("root", "10.0.0.0/16")]),
            vec!["PASS", "PASS", "FAIL", "PASS", "PASS"],
        ),
        (
            "an AMI option that declares nothing a control reads",
            "AmiProduct",
            "AMISecurity",
            version_with(vec![delivery_option("AmiDeliveryOptionDetails", json!({}))]),
            vec!["FAIL", "FAIL", "NOT_EXECUTED", "NOT_EXECUTED", "FAIL"],
        ),
        (
            "an AMI option whose image is the empty string",
            "AmiProduct",
            "AMISecurity",
            version_with(vec![delivery_option(
                "AmiDeliveryOptionDetails",
                json!({"AmiSource": {"AmiId": "", "AccessRoleArn": "arn:aws:iam::123456789012:role/R"}}),
            )]),
            vec!["FAIL", "PASS", "NOT_EXECUTED", "NOT_EXECUTED", "FAIL"],
        ),
        (
            "a container option pinned by tag",
            "ContainerProduct",
            "ContainerSecurity",
            version_with(vec![container_option(json!([
                "123456789012.dkr.ecr.eu-west-1.amazonaws.com/product:1.2.3"
            ]))]),
            vec!["PASS", "PASS", "PASS"],
        ),
        (
            "a container option pinned by digest",
            "ContainerProduct",
            "ContainerSecurity",
            version_with(vec![container_option(json!([
                "123456789012.dkr.ecr.eu-west-1.amazonaws.com/product@sha256:0123456789abcdef"
            ]))]),
            vec!["PASS", "PASS", "PASS"],
        ),
        (
            "a container option on a floating tag",
            "ContainerProduct",
            "ContainerSecurity",
            version_with(vec![container_option(json!([
                "123456789012.dkr.ecr.eu-west-1.amazonaws.com/product:1.2.3",
                "123456789012.dkr.ecr.eu-west-1.amazonaws.com/sidecar:latest",
            ]))]),
            vec!["PASS", "FAIL", "PASS"],
        ),
        (
            "a container option naming no image at all",
            "ContainerProduct",
            "ContainerSecurity",
            version_with(vec![container_option(json!([]))]),
            vec!["FAIL", "NOT_EXECUTED", "PASS"],
        ),
    ];

    // Every change set is started first and waited for ONCE: each one is read back
    // through the target filter that names it, so the answers do not depend on the
    // order they settled in.
    let mut started: Vec<(&str, &str, String, Vec<&str>)> = Vec::with_capacity(rows.len());
    for (case, entity_type, framework, details, expected) in &rows {
        let change_set_id = start(&router, entity_type, details, "APPLY").await;
        started.push((case, framework, change_set_id, expected.clone()));
    }
    wait_for_success();

    for (case, framework, change_set_id, expected) in started {
        let listed = assessments_of(&router, &change_set_id).await;
        let summaries = listed.list("/AssessmentSummaryList");
        assert_eq!(
            summaries.len(),
            1,
            "case {case:?}: one submitted delivery option is one assessment, got {:?}",
            listed.body
        );
        let summary = &summaries[0];
        let assessment_id = summary["AssessmentId"].as_str().unwrap_or_default();
        assert!(
            assessment_id.starts_with("assessment-"),
            "case {case:?}: an assessment id is the form DescribeAssessment accepts back, \
             got {assessment_id:?}"
        );
        assert_eq!(
            summary["FrameworkId"].as_str().unwrap_or_default(),
            format!("{framework}@1.0"),
            "case {case:?}: the report names the framework and its version, got {summary:?}"
        );
        // "The set member corresponds to the framework identified by FrameworkId":
        // the summary of the OTHER framework is not the one that was evaluated.
        let option_id = summary
            .pointer(&format!("/FrameworkSummary/{framework}Summary/DeliveryOptionId"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            !option_id.is_empty(),
            "case {case:?}: the framework summary names the delivery option that was \
             evaluated, got {summary:?}"
        );
        assert!(
            is_iso8601(summary["CreatedAt"].as_str().unwrap_or_default())
                && is_iso8601(summary["ExpiresAt"].as_str().unwrap_or_default()),
            "case {case:?}: both instants are DateTimeISO8601, got {summary:?}"
        );
        assert!(
            summary["ExpiresAt"].as_str() > summary["CreatedAt"].as_str(),
            "case {case:?}: an assessment expires after it was created, got {summary:?}"
        );

        // The overall result is FAIL exactly when one control failed.
        let expected_result = if expected.contains(&"FAIL") {
            "FAIL"
        } else {
            "PASS"
        };
        assert_eq!(
            summary["AssessmentResult"].as_str().unwrap_or_default(),
            expected_result,
            "case {case:?}: expected {expected_result} over controls {expected:?}, \
             got {summary:?}"
        );

        // And the control-level report: `DescribeAssessment` details what the
        // framework evaluated, in the order the framework runs its controls.
        let described = call(
            &router,
            "POST",
            "/DescribeAssessment",
            Some(json!({
                "Catalog": CATALOG,
                "AssessmentIdentifier": assessment_id,
                "MaxResults": 100,
            })),
        )
        .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the assessment the list reported must be describable, \
             got {} with {:?}",
            described.status, described.body
        );
        let catalogue = if framework == "AMISecurity" {
            AMI_CONTROLS
        } else {
            CONTAINER_CONTROLS
        };
        let wanted: Vec<(String, String)> = catalogue
            .iter()
            .zip(expected.iter())
            .map(|(control, result)| (control.to_string(), result.to_string()))
            .collect();
        let got = control_results(&described);
        assert_eq!(
            got, wanted,
            "case {case:?}: expected {wanted:?}, got {got:?} from {:?}",
            described.body
        );

        // A control that failed says why, in the field the error applies to; one
        // that passed or did not run reports no error.
        for control in described.list("/ControlAssessments") {
            let id = control["ControlId"].as_str().unwrap_or_default();
            let result = control["ControlAssessmentResult"].as_str().unwrap_or_default();
            let errors = control["Errors"].as_array().cloned().unwrap_or_default();
            if result == "FAIL" {
                assert_eq!(
                    errors.len(),
                    1,
                    "case {case:?}: control {id:?} failed, so it reports the error that \
                     says why, got {control:?}"
                );
                let error = &errors[0];
                let code = error["Code"].as_str().unwrap_or_default();
                assert!(
                    ["MISSING_VALUE", "INVALID_VALUE"].contains(&code),
                    "case {case:?}: control {id:?} must report a documented error code, \
                     got {error:?}"
                );
                assert!(
                    error["Message"].as_str().unwrap_or_default().len() > 1,
                    "case {case:?}: control {id:?} must report a message, got {error:?}"
                );
                assert!(
                    !error
                        .pointer("/Scope/0/Name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .is_empty(),
                    "case {case:?}: control {id:?} must name the field its error applies \
                     to, got {error:?}"
                );
            } else {
                assert!(
                    errors.is_empty(),
                    "case {case:?}: control {id:?} is {result}, so it reports no error, \
                     got {control:?}"
                );
            }
        }
    }
}

/// A change set assesses nothing when there is nothing for a framework to evaluate:
/// no delivery option was submitted, no framework of this catalog covers the entity
/// type, or the change set applied nothing at all.
///
/// The rows differ along what the rule generalises over: the entity type, whether the
/// details carry delivery options, and the intent.
#[tokio::test]
async fn a_change_set_with_nothing_to_evaluate_produces_no_assessment() {
    let router = emulator();

    // (case, entity type, submitted details, intent)
    let rows: [(&str, &str, Value, &str); 5] = [
        (
            "an AMI product, no delivery option submitted",
            "AmiProduct",
            json!({"ProductTitle": "no version yet"}),
            "APPLY",
        ),
        (
            "an AMI product whose delivery option list is empty",
            "AmiProduct",
            version_with(Vec::new()),
            "APPLY",
        ),
        (
            "a SaaS product, which no framework of this catalog covers",
            "SaaSProduct",
            version_with(vec![ami_option("ec2-user", "10.0.0.0/16")]),
            "APPLY",
        ),
        (
            "a data product, which no framework of this catalog covers",
            "DataProduct",
            version_with(vec![container_option(json!(["registry/image:1"]))]),
            "APPLY",
        ),
        (
            "an AMI product validated rather than applied",
            "AmiProduct",
            version_with(vec![ami_option("ec2-user", "10.0.0.0/16")]),
            "VALIDATE",
        ),
    ];

    let mut started: Vec<(&str, String)> = Vec::with_capacity(rows.len());
    for (case, entity_type, details, intent) in &rows {
        started.push((case, start(&router, entity_type, details, intent).await));
    }
    wait_for_success();

    for (case, change_set_id) in &started {
        let listed = assessments_of(&router, change_set_id).await;
        assert_eq!(
            listed.body.pointer("/AssessmentSummaryList"),
            Some(&json!([])),
            "case {case:?}: nothing was evaluated, so the list is present and empty, \
             got {:?}",
            listed.body
        );
    }

    // And across the whole catalog: none of the change sets above produced one.
    let everything = call(
        &router,
        "POST",
        "/ListAssessments",
        Some(json!({"Catalog": CATALOG})),
    )
    .await;
    assert_eq!(
        everything.body.pointer("/AssessmentSummaryList"),
        Some(&json!([])),
        "no change set above submitted a delivery option a framework covers, got {:?}",
        everything.body
    );
}

/// The delivery option an assessment names is the one the ENTITY carries: an option
/// submitted with an id keeps it, one submitted without is assigned one, and the
/// assessment of each names the same id the entity's details hold — which is what
/// `FrameworkFilters` then selects by.
#[tokio::test]
async fn an_assessment_names_the_delivery_option_the_entity_carries() {
    let router = emulator();

    let submitted_id = "do-submitted-by-the-caller";
    let mut named = ami_option("ec2-user", "10.0.0.0/16");
    named["Id"] = json!(submitted_id);
    let change_set_id = start(
        &router,
        "AmiProduct",
        &version_with(vec![named, ami_option("ec2-user", "192.168.0.0/24")]),
        "APPLY",
    )
    .await;
    wait_for_success();

    // The entity the change set created, and the delivery option ids it now carries.
    let change_set = call(
        &router,
        "GET",
        &format!("/DescribeChangeSet?catalog={CATALOG}&changeSetId={change_set_id}"),
        None,
    )
    .await;
    assert_eq!(
        (change_set.status, change_set.text("/Status").as_str()),
        (200, "SUCCEEDED"),
        "the change set must have succeeded, got {} with {:?}",
        change_set.status,
        change_set.body
    );
    let entity_id = {
        let listed = call(
            &router,
            "POST",
            "/ListChangeSets",
            Some(json!({"Catalog": CATALOG})),
        )
        .await;
        listed
            .list("/ChangeSetSummaryList")
            .into_iter()
            .find(|item| item["ChangeSetId"] == json!(change_set_id))
            .and_then(|item| item["EntityIdList"][0].as_str().map(str::to_string))
            .unwrap_or_else(|| {
                panic!(
                    "change set {change_set_id} must name the entity it created: {:?}",
                    listed.body
                )
            })
    };

    let entity = call(
        &router,
        "GET",
        &format!("/DescribeEntity?catalog={CATALOG}&entityId={entity_id}"),
        None,
    )
    .await;
    assert_eq!(
        entity.status, 200,
        "the entity the change set created must be readable, got {} with {:?}",
        entity.status, entity.body
    );
    let held: Value = serde_json::from_str(&entity.text("/Details"))
        .expect("the entity's Details is the JSON document the change submitted");
    let held_ids: Vec<String> = held["DeliveryOptions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|option| option["Id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        held_ids.len(),
        2,
        "the entity carries both submitted delivery options, got {held:?}"
    );
    assert_eq!(
        held_ids[0], submitted_id,
        "an option submitted with an id keeps it, got {held:?}"
    );
    assert!(
        !held_ids[1].is_empty() && held_ids[1] != submitted_id,
        "an option submitted without an id is assigned its own, got {held:?}"
    );

    // One assessment per delivery option, each naming the option it evaluated.
    let listed = assessments_of(&router, &change_set_id).await;
    let mut assessed: Vec<String> = listed
        .list("/AssessmentSummaryList")
        .iter()
        .map(|summary| {
            summary
                .pointer("/FrameworkSummary/AMISecuritySummary/DeliveryOptionId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assessed.sort();
    let mut wanted = held_ids.clone();
    wanted.sort();
    assert_eq!(
        assessed, wanted,
        "every delivery option the entity carries was assessed, got {:?}",
        listed.body
    );

    // And every assessment names the entity it evaluated, so the entity filter and
    // the framework's own delivery-option filter select the same records.
    let by_entity = call(
        &router,
        "POST",
        "/ListAssessments",
        Some(json!({
            "Catalog": CATALOG,
            "AssessmentTargetFilter": {"EntityId": entity_id},
        })),
    )
    .await;
    assert_eq!(
        by_entity.list("/AssessmentSummaryList").len(),
        2,
        "the entity filter selects the assessments of that entity, got {:?}",
        by_entity.body
    );

    for (case, option_id) in [("submitted", &held_ids[0]), ("assigned", &held_ids[1])] {
        let by_option = call(
            &router,
            "POST",
            "/ListAssessments",
            Some(json!({
                "Catalog": CATALOG,
                "FrameworkFilters": {"AMISecurityFilters": {"DeliveryOptionId": option_id}},
            })),
        )
        .await;
        let ids: Vec<String> = by_option
            .list("/AssessmentSummaryList")
            .iter()
            .map(|summary| {
                summary
                    .pointer("/FrameworkSummary/AMISecuritySummary/DeliveryOptionId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            ids,
            vec![option_id.clone()],
            "case {case:?}: the delivery-option filter selects that option's assessment \
             alone, got {:?}",
            by_option.body
        );
    }

    // The unversioned framework id selects both, and the other framework neither:
    // these assessments were performed against the AMI framework.
    // `ListAssessmentsRequest$FrameworkId` is `^[\w\-@]+$`, which admits no `.`, so
    // the dotted version the report names is not expressible in the filter at all —
    // the unversioned id is how a caller selects a framework's assessments.
    for (case, framework_id, expected) in [
        ("the framework that was evaluated", "AMISecurity", 2),
        ("the other framework", "ContainerSecurity", 0),
    ] {
        let by_framework = call(
            &router,
            "POST",
            "/ListAssessments",
            Some(json!({"Catalog": CATALOG, "FrameworkId": framework_id})),
        )
        .await;
        assert_eq!(
            by_framework.list("/AssessmentSummaryList").len(),
            expected,
            "case {case:?}: expected {expected} assessments, got {:?}",
            by_framework.body
        );
    }
}

/// The delivery options one change may submit are bounded: a change at the bound is
/// accepted and every option it carries is assessed, and one past it is refused
/// rather than stored, so what one request leaves in the store is bounded by the
/// published constant and not by the width of the document it sent.
///
/// The rows differ along what the rule generalises over: the number of options
/// submitted, at both ends of the bound and past it.
#[tokio::test]
async fn the_delivery_options_one_change_may_submit_are_bounded() {
    let router = emulator();
    let option = || ami_option("ec2-user", "10.0.0.0/16");

    // (case, how many options the change submits)
    let accepted: [(&str, usize); 3] = [
        ("one option", 1),
        ("one below the bound", MAX_DELIVERY_OPTIONS_PER_CHANGE - 1),
        ("exactly the bound", MAX_DELIVERY_OPTIONS_PER_CHANGE),
    ];
    let mut started: Vec<(&str, usize, String)> = Vec::with_capacity(accepted.len());
    for (case, count) in accepted {
        let details = version_with(vec![option(); count]);
        started.push((case, count, start(&router, "AmiProduct", &details, "APPLY").await));
    }
    wait_for_success();
    for (case, count, change_set_id) in &started {
        let listed = assessments_of(&router, change_set_id).await;
        assert_eq!(
            listed.list("/AssessmentSummaryList").len(),
            *count,
            "case {case:?}: every one of the {count} submitted options must be assessed, \
             got {:?}",
            listed.body
        );
    }

    // And past the bound: refused with the modelled ValidationException, which names
    // the count submitted and the bound it passed. The options here are empty
    // objects, so a count far past the bound still fits inside `Change$Details`'s
    // own `@length(max: 16384)` and the refusal under test is the one observed.
    for (case, count) in [
        ("one past the bound", MAX_DELIVERY_OPTIONS_PER_CHANGE + 1),
        ("far past the bound", MAX_DELIVERY_OPTIONS_PER_CHANGE * 3),
    ] {
        let details = version_with(vec![json!({}); count]);
        let refused = try_start(&router, "AmiProduct", &details, "APPLY").await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (422, "ValidationException"),
            "case {case:?}: expected 422 ValidationException, got {} {:?} with {:?}",
            refused.status,
            refused.error_code(),
            refused.body
        );
        let message = refused.text("/Message");
        assert!(
            message.contains(&count.to_string())
                && message.contains(&MAX_DELIVERY_OPTIONS_PER_CHANGE.to_string()),
            "case {case:?}: the refusal must name the count and the bound, got {message:?}"
        );
    }
}

/// The assessments the store holds are bounded: past the quota the oldest is dropped,
/// so the assessment surface does not accumulate without end across change sets.
#[tokio::test]
async fn the_assessments_the_store_holds_are_bounded() {
    let router = emulator();
    let details = version_with(vec![
        ami_option("ec2-user", "10.0.0.0/16");
        MAX_DELIVERY_OPTIONS_PER_CHANGE
    ]);
    // Enough change sets to carry the store past the quota, each at the per-change
    // bound: the smallest number of requests that reaches it.
    let change_sets = ASSESSMENT_QUOTA / MAX_DELIVERY_OPTIONS_PER_CHANGE + 1;
    for _ in 0..change_sets {
        start(&router, "AmiProduct", &details, "APPLY").await;
    }
    wait_for_success();

    let held = count_all_assessments(&router).await;
    assert_eq!(
        held, ASSESSMENT_QUOTA,
        "{change_sets} change sets of {MAX_DELIVERY_OPTIONS_PER_CHANGE} options submitted \
         {} assessments, so the store must hold the quota of {ASSESSMENT_QUOTA}, not {held}",
        change_sets * MAX_DELIVERY_OPTIONS_PER_CHANGE
    );
}
