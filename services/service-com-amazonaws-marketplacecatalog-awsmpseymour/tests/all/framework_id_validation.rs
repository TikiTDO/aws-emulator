// SPDX-License-Identifier: CC0-1.0

//! `ListAssessments` judges its `FrameworkId` against the frameworks this catalog
//! assesses against, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path
//! a client takes. Nothing seeds the store directly: the assessments read back are the
//! ones the change sets started below produced.
//!
//! The rule under test: a request member that NARROWS a page by an attribute whose
//! values this service publishes as a closed set is judged against that set before
//! anything is selected — a value inside it narrows the page, to an empty one when no
//! record carries it, and a value outside it names nothing the attribute can hold and
//! is refused with the modelled `ValidationException` (422) quoting the value and the
//! published set, rather than silently answering the empty page it would match.
//!
//! `FrameworkId` is such a member: "the unique identifier of a framework. When
//! specified, only assessments performed against this framework are returned", and the
//! frameworks are the ones the model's `FrameworkSummary` / `FrameworkFilters` union
//! members name. The sibling narrowing members of the same kind — `ListEntities`'
//! `EntityType` and the `Status` filter of `ListChangeSets` — are judged the same way in
//! `documented_entity_types.rs` and `filter_validation.rs`.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::state::{
    CHANGE_SET_APPLYING, CHANGE_SET_PREPARING,
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

/// The frameworks this catalog assesses against and the entity type whose delivery
/// options each one evaluates.
///
/// Spelled out here rather than imported from the crate, so a framework the service
/// invents fails this test instead of following it. The ids are the ones the model's
/// own union members give (`smithy_query(query="shape",
/// shape="FrameworkSummary,FrameworkFilters")`: `AMISecurityFilters` /
/// `AMISecuritySummary` -> `AMISecurity`, `ContainerSecurityFilters` /
/// `ContainerSecuritySummary` -> `ContainerSecurity`), and `ListAssessments` names the
/// products they cover ("such as AMI Security or Container Security").
const PUBLISHED_FRAMEWORKS: &[(&str, &str)] = &[
    ("AMISecurity", "AmiProduct"),
    ("ContainerSecurity", "ContainerProduct"),
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

    /// The fault this answer carries: `ValidationException$Message`.
    fn message(&self) -> String {
        self.body["Message"].as_str().unwrap_or_default().to_string()
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

async fn call(router: &MultiServiceRouter, uri: &str, body: Value) -> Answer {
    let request = ::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "catalog.marketplace.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1)
        .header("content-type", "application/json")
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "POST {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// One delivery option of the form `form`, carrying everything the framework's controls
/// read — the shape of a submitted delivery option, not of any one assessment result.
fn delivery_option(form: &str, details: Value) -> Value {
    json!({"Details": {form: details}})
}

/// The details of a change that submits one delivery option of a product of
/// `entity_type`, so applying it produces exactly one assessment against the framework
/// that covers that type.
fn submitted_version(entity_type: &str) -> Value {
    let option = match entity_type {
        "ContainerProduct" => delivery_option(
            "EcrDeliveryOptionDetails",
            json!({
                "ContainerImages": [
                    "123456789012.dkr.ecr.eu-west-1.amazonaws.com/product:1.2.3"
                ],
                "UsageInstructions": "helm install product ./chart",
            }),
        ),
        _ => delivery_option(
            "AmiDeliveryOptionDetails",
            json!({
                "AmiSource": {
                    "AmiId": "ami-0abcdef1234567890",
                    "AccessRoleArn": "arn:aws:iam::123456789012:role/MarketplaceScan",
                    "UserName": "ec2-user",
                    "OperatingSystemName": "AMAZONLINUX",
                    "OperatingSystemVersion": "2023",
                },
                "RecommendedInstanceType": "t3.medium",
                "SecurityGroups": [{
                    "IpProtocol": "tcp",
                    "IpRanges": ["10.0.0.0/16"],
                    "FromPort": 22,
                    "ToPort": 22,
                }],
                "UsageInstructions": "Connect over SSH and run /opt/product/start.",
            }),
        ),
    };
    json!({
        "Version": {"VersionTitle": "1.0", "ReleaseNotes": "first release"},
        "DeliveryOptions": [option],
    })
}

/// One succeeded change set per published framework, so the catalog holds exactly one
/// assessment of each.
async fn seed_one_assessment_per_framework(router: &MultiServiceRouter) {
    for (framework, entity_type) in PUBLISHED_FRAMEWORKS {
        let answer = call(
            router,
            "/StartChangeSet",
            json!({
                "Catalog": CATALOG,
                "Intent": "APPLY",
                "ChangeSet": [{
                    "ChangeType": "AddDeliveryOptions",
                    "Entity": {"Type": format!("{entity_type}@1.0")},
                    "Details": submitted_version(entity_type).to_string(),
                }],
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {framework:?}: the seed change set must be accepted, got {} with {:?}",
            answer.status, answer.body
        );
    }
    wait_for_success();
}

/// The `FrameworkId` each reported summary carries.
fn reported_framework_ids(answer: &Answer) -> Vec<String> {
    answer
        .list("/AssessmentSummaryList")
        .iter()
        .map(|summary| {
            summary["FrameworkId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// A `FrameworkId` naming a published framework narrows the page to the assessments of
/// THAT framework, read from state — unversioned it selects every version of it, and at
/// a version it selects that version alone, which is an empty page for a version this
/// catalog did not evaluate.
///
/// The rows differ along what the rule generalises over: which published framework is
/// named, and whether it is named unversioned or at a version.
#[tokio::test]
async fn a_framework_id_naming_a_published_framework_selects_that_framework() {
    let router = emulator();
    seed_one_assessment_per_framework(&router).await;

    // The whole catalog, so "narrowed" is a claim about something: one assessment per
    // published framework.
    let every = call(
        &router,
        "/ListAssessments",
        json!({"Catalog": CATALOG, "MaxResults": 100}),
    )
    .await;
    assert_eq!(
        every.status, 200,
        "listing the whole catalog must answer 200, got {} with {:?}",
        every.status, every.body
    );
    assert_eq!(
        reported_framework_ids(&every).len(),
        PUBLISHED_FRAMEWORKS.len(),
        "the seed must hold one assessment per published framework, got {:?}",
        every.body
    );

    for (framework, _) in PUBLISHED_FRAMEWORKS {
        // Unversioned: the form `ListAssessmentsRequest$FrameworkId` documents.
        let case = format!("the published framework {framework}, unversioned");
        let answer = call(
            &router,
            "/ListAssessments",
            json!({"Catalog": CATALOG, "MaxResults": 100, "FrameworkId": framework}),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a published framework must be served, got {} with {:?}",
            answer.status, answer.body
        );
        let reported = reported_framework_ids(&answer);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected the one assessment of that framework, got {reported:?}"
        );
        assert!(
            reported
                .iter()
                .all(|id| id.starts_with(&format!("{framework}@"))),
            "case {case:?}: every assessment reported must be of that framework, got {reported:?}"
        );

        // At a version: the member's `@pattern` (`^[\w\-@]+$`) admits the version
        // separator but no dot, so every version a request can name through it differs
        // from the one the assessment just read back carries. The framework itself IS
        // one this catalog has, so such a request is served — an empty page, since it
        // selects that version alone — rather than refused.
        let held = reported.first().cloned().unwrap_or_default();
        for version in ["1", "9", "20260101"] {
            let asked = format!("{framework}@{version}");
            let case = format!("the published framework {framework} at version {version}");
            assert_ne!(
                held, asked,
                "case {case:?}: the version asked for must differ from the {held:?} this \
                 catalog holds, or the empty page below would prove nothing"
            );
            let answer = call(
                &router,
                "/ListAssessments",
                json!({"Catalog": CATALOG, "MaxResults": 100, "FrameworkId": asked}),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a published framework at a version must be served, \
                 got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                reported_framework_ids(&answer),
                Vec::<String>::new(),
                "case {case:?}: a version this catalog did not evaluate selects nothing, \
                 while {held:?} is held, got {:?}",
                answer.body
            );
        }
    }
}

/// A `FrameworkId` naming no framework this catalog assesses against is refused with
/// the modelled `ValidationException` and its 422, quoting the value and the published
/// frameworks — never a 200 with the empty page it would have matched.
///
/// The rows differ along how the value misses the published set: a typo, another
/// casing, a name from another vocabulary, a shape name of this very model, an entity
/// type rather than a framework, and the version forms around a framework the catalog
/// does not have.
#[tokio::test]
async fn a_framework_id_naming_no_framework_is_refused() {
    let router = emulator();
    // The store HOLDS assessments, so an empty page is something this catalog could
    // otherwise have answered.
    seed_one_assessment_per_framework(&router).await;

    // Every value below satisfies `FrameworkId`'s own `@pattern` (`^[\w\-@]+$`) and
    // `@length(1..255)`, so each one reaches the handler and is judged by the set
    // rather than by the boundary.
    let rows: Vec<(&str, String)> = vec![
        ("a typo of a published framework", "AMISecurty".to_string()),
        ("an invented framework", "FooSecurity".to_string()),
        (
            "a published framework lower-cased",
            "amisecurity".to_string(),
        ),
        (
            "a published framework upper-cased",
            "AMISECURITY".to_string(),
        ),
        (
            "a published framework with a hyphen in it",
            "AMI-Security".to_string(),
        ),
        (
            "the union member name rather than the framework id",
            "AMISecurityFilters".to_string(),
        ),
        (
            "an entity type rather than a framework",
            "AmiProduct".to_string(),
        ),
        (
            "a framework word from another vocabulary",
            "CIS".to_string(),
        ),
        ("a typo at a version", "AMISecurty@1".to_string()),
        (
            "a published framework with no version after the separator",
            "AMISecurity@".to_string(),
        ),
        (
            "a published framework at an empty version run",
            "AMISecurity@@1".to_string(),
        ),
        ("the longest value the member admits", "A".repeat(255)),
    ];

    for (case, framework_id) in rows {
        // The bad value is refused on its own and beside filters that would otherwise
        // have been honoured.
        let requests = [
            json!({"Catalog": CATALOG, "MaxResults": 100, "FrameworkId": framework_id}),
            json!({
                "Catalog": CATALOG,
                "MaxResults": 100,
                "FrameworkId": framework_id,
                "AssessmentTargetFilter": {"EntityId": "ent-000000000001"},
            }),
        ];
        for request in requests {
            let answer = call(&router, "/ListAssessments", request.clone()).await;
            assert_eq!(
                (answer.status, answer.error_code()),
                (422, "ValidationException"),
                "case {case:?} ({framework_id:?}): expected 422 ValidationException for \
                 {request}, got {} {:?} with {:?}",
                answer.status,
                answer.error_code(),
                answer.body
            );
            let message = answer.message();
            assert!(
                message.contains(&framework_id),
                "case {case:?}: the fault must name the rejected value {framework_id:?}, \
                 got {message:?}"
            );
            for (published, _) in PUBLISHED_FRAMEWORKS {
                assert!(
                    message.contains(published),
                    "case {case:?}: the fault must name the published framework {published}, \
                     got {message:?}"
                );
            }
            let served = answer.list("/AssessmentSummaryList").len();
            assert_eq!(
                served, 0,
                "case {case:?}: a refused call serves no page, got {served} in {:?}",
                answer.body
            );
        }
    }

    // The refusal changed nothing: the assessments the seed produced are still there.
    let every = call(
        &router,
        "/ListAssessments",
        json!({"Catalog": CATALOG, "MaxResults": 100}),
    )
    .await;
    assert_eq!(
        reported_framework_ids(&every).len(),
        PUBLISHED_FRAMEWORKS.len(),
        "the refusals must leave the catalog as it was, got {:?}",
        every.body
    );
}
