// SPDX-License-Identifier: CC0-1.0

//! How an assessment's resiliency score breaks down over the scoring components,
//! end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a
//! restJson1 request — and asserts the response BODY, not just the status.
//!
//! The rule under test: a resiliency score is reported as the FRACTION of its
//! maximum that was earned, and `componentScore` breaks that same score down over
//! the scoring components the model's `ResiliencyScoreType` enum names — so the
//! components' `possibleScore` shares add up to the whole of a score, what they
//! earn adds up to the score itself, no component earns more than its share, and
//! the compliance component's `outstandingCount` is the number of Application
//! Components that breached the policy.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_resiliencehub_awsresiliencehub::state::{
    ComAmazonawsResiliencehubAwsresiliencehubState, RELEASE_APP_VERSION,
};
use service_com_amazonaws_resiliencehub_awsresiliencehub::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for.
const REGION: &str = "us-east-1";

/// The whole of a resiliency score: a score is the fraction of its maximum that was
/// earned, so meeting everything an application is judged on earns exactly this.
const WHOLE_SCORE: f64 = 1.0;

/// How close two scores have to be to count as the same, so a mean of shares is not
/// compared for exact bit equality.
const SCORE_TOLERANCE: f64 = 1e-9;

/// The scoring components a score breaks down over, from the enum the model gives
/// `ScoringComponentResiliencyScores`' keys (`smithy_query query="shape"
/// shape="ResiliencyScoreType"`: Compliance | Test | Alarm | Sop).
const SCORING_COMPONENTS: [&str; 4] = ["Alarm", "Compliance", "Sop", "Test"];

/// The one scoring component graded on the policy's objectives; the others are
/// graded on implemented recommendations.
const COMPLIANCE_COMPONENT: &str = "Compliance";

/// An emulator holding only this service, over one shared engine — and that engine,
/// so a case can move the CLOCK an assessment is waiting on instead of sleeping
/// through its span.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These requests carry an unverifiable placeholder signature: this suite tests
    // the operations, not SigV4.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// What a dispatched request answered.
#[derive(Debug)]
struct Reply {
    status: u16,
    /// The restJson1 error shape name, from `x-amzn-errortype`; empty on success.
    error_code: String,
    body: Value,
}

impl Reply {
    /// The response body, asserting the call succeeded.
    fn ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} {:?} {:?}",
            self.status, self.error_code, self.body
        );
        &self.body
    }
}

/// Dispatch one request through the router, exactly as the server front door does.
async fn post(router: &MultiServiceRouter, target: &str, body: &Value) -> Reply {
    let payload = serde_json::to_vec(body).expect("serializable body");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://resiliencehub.{REGION}.amazonaws.com{target}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/resiliencehub/\
                 aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body = response.into_body();
    let body = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    Reply {
        status,
        error_code,
        body,
    }
}

/// A resiliency policy stating `objectives` (a disruption type to its RTO/RPO
/// target), and its ARN.
async fn create_a_policy(
    router: &MultiServiceRouter,
    policy_name: &str,
    objectives: &Value,
) -> String {
    let reply = post(
        router,
        "/create-resiliency-policy",
        &json!({
            "policyName": policy_name,
            "tier": "Important",
            "policy": objectives,
        }),
    )
    .await;
    reply.ok(policy_name)["policy"]["policyArn"]
        .as_str()
        .expect("a minted policyArn")
        .to_string()
}

/// An application named `name`, pointing at `policy_arn` when the case gives one.
async fn create_an_app(
    router: &MultiServiceRouter,
    name: &str,
    policy_arn: Option<&str>,
) -> String {
    let body = match policy_arn {
        Some(policy_arn) => json!({"name": name, "policyArn": policy_arn}),
        None => json!({"name": name}),
    };
    let reply = post(router, "/create-app", &body).await;
    reply.ok(name)["app"]["appArn"]
        .as_str()
        .expect("a minted appArn")
        .to_string()
}

/// Add one Application Component to `app_arn`'s draft version.
async fn add_a_component(router: &MultiServiceRouter, app_arn: &str, id: &str) {
    let reply = post(
        router,
        "/create-app-version-app-component",
        &json!({
            "appArn": app_arn,
            "id": id,
            "name": id,
            "type": "AWS::ResilienceHub::ComputeAppComponent",
        }),
    )
    .await;
    reply.ok(id);
}

/// Add one physical resource to `app_arn`'s draft version, mapped to `component`.
async fn add_a_resource(router: &MultiServiceRouter, app_arn: &str, component: &str) {
    let identifier = format!("{component}-bucket");
    let reply = post(
        router,
        "/create-app-version-resource",
        &json!({
            "appArn": app_arn,
            "resourceName": identifier,
            "logicalResourceId": {"identifier": identifier},
            "physicalResourceId": identifier,
            "resourceType": "AWS::S3::Bucket",
            "appComponents": [component],
        }),
    )
    .await;
    reply.ok(&identifier);
}

/// Publish `app_arn`'s draft, so the application has a release to assess.
async fn publish(router: &MultiServiceRouter, app_arn: &str) {
    post(router, "/publish-app-version", &json!({"appArn": app_arn}))
        .await
        .ok("publish");
}

/// Move the clock an assessment is waiting on into the PAST, so a case can read
/// what the finished assessment reports without sleeping through its span.
fn finish_the_assessment(engine: &Arc<engine::InMemoryEngine>, assessment_arn: &str) {
    engine
        .state::<ComAmazonawsResiliencehubAwsresiliencehubState>()
        .lock()
        .assessment_mut(assessment_arn)
        .expect("the assessment the call recorded")
        .completes_at = SystemTime::now() - Duration::from_secs(1);
}

/// Start an assessment of the application's release, report it finished, and answer
/// the finished assessment as `DescribeAppAssessment` reports it — the round trip
/// through the reader, not the starter's own answer.
async fn assess(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    app_arn: &str,
    assessment_name: &str,
) -> Value {
    let started = post(
        router,
        "/start-app-assessment",
        &json!({
            "appArn": app_arn,
            "appVersion": RELEASE_APP_VERSION,
            "assessmentName": assessment_name,
        }),
    )
    .await;
    let assessment_arn = started.ok(assessment_name)["assessment"]["assessmentArn"]
        .as_str()
        .expect("a minted assessmentArn")
        .to_string();
    finish_the_assessment(engine, &assessment_arn);
    let read = post(
        router,
        "/describe-app-assessment",
        &json!({"assessmentArn": assessment_arn}),
    )
    .await;
    read.ok(assessment_name)["assessment"].clone()
}

/// A row of the table: the objectives the application's policy states, which of its
/// Application Components have a resource mapped to them, and which have nothing.
struct Breakdown {
    label: &'static str,
    /// The policy's disruption objectives, or `None` for an application that names
    /// no policy at all.
    objectives: Option<Value>,
    /// Component ids each holding at least one mapped resource.
    mapped: &'static [&'static str],
    /// Component ids with nothing mapped to them — each one a breach of every
    /// objective the policy states.
    unmapped: &'static [&'static str],
}

/// The table. The rows differ along what the rule generalises over: which disruption
/// types the policy states objectives for beyond the three every policy has to state,
/// how much the published version maps, how many of its components have nothing to
/// recover them (so the compliance component's outstanding count runs 0, 1, 2, 3),
/// and whether there is a policy to judge the application against at all.
fn breakdown_table() -> Vec<Breakdown> {
    vec![
        Breakdown {
            label: "the required objectives, every component mapped",
            objectives: Some(json!({
                "Software": {"rtoInSecs": 3600, "rpoInSecs": 3600},
                "Hardware": {"rtoInSecs": 3600, "rpoInSecs": 3600},
                "AZ": {"rtoInSecs": 3600, "rpoInSecs": 3600},
            })),
            mapped: &["compute-tier"],
            unmapped: &[],
        },
        Breakdown {
            label: "four objectives, every component mapped",
            objectives: Some(json!({
                "Software": {"rtoInSecs": 60, "rpoInSecs": 30},
                "Hardware": {"rtoInSecs": 600, "rpoInSecs": 300},
                "AZ": {"rtoInSecs": 7200, "rpoInSecs": 3600},
                "Region": {"rtoInSecs": 86400, "rpoInSecs": 3600},
            })),
            mapped: &["compute-tier", "storage-tier"],
            unmapped: &[],
        },
        Breakdown {
            label: "the multi-Region objective beside the required three, one component \
                    with nothing mapped to it",
            objectives: Some(json!({
                "Software": {"rtoInSecs": 86400, "rpoInSecs": 3600},
                "Hardware": {"rtoInSecs": 86400, "rpoInSecs": 3600},
                "AZ": {"rtoInSecs": 86400, "rpoInSecs": 3600},
                "Region": {"rtoInSecs": 86400, "rpoInSecs": 3600},
            })),
            mapped: &["compute-tier"],
            unmapped: &["empty-tier"],
        },
        Breakdown {
            label: "the required objectives, two components with nothing mapped to them",
            objectives: Some(json!({
                "Software": {"rtoInSecs": 120, "rpoInSecs": 60},
                "Hardware": {"rtoInSecs": 600, "rpoInSecs": 300},
                "AZ": {"rtoInSecs": 86400, "rpoInSecs": 3600},
            })),
            mapped: &["compute-tier"],
            unmapped: &["empty-tier", "other-empty-tier"],
        },
        Breakdown {
            label: "three objectives, three of four components with nothing mapped to them",
            objectives: Some(json!({
                "Software": {"rtoInSecs": 0, "rpoInSecs": 0},
                "Hardware": {"rtoInSecs": 300, "rpoInSecs": 150},
                "AZ": {"rtoInSecs": 1800, "rpoInSecs": 900},
            })),
            // An app template is publishable only once it maps a resource, so even
            // the row whose point is unmapped components maps one.
            mapped: &["compute-tier"],
            unmapped: &["empty-tier", "other-empty-tier", "third-empty-tier"],
        },
        Breakdown {
            label: "no policy to judge against",
            objectives: None,
            mapped: &["compute-tier"],
            unmapped: &[],
        },
    ]
}

/// The whole table: each row's finished assessment breaks its score down over every
/// scoring component, with the shares adding up to the whole of a score, what the
/// components earn adding up to the score itself, and the compliance component
/// carrying the count of Application Components that breached the policy.
#[tokio::test]
async fn the_score_breaks_down_over_the_scoring_components() {
    for (index, row) in breakdown_table().into_iter().enumerate() {
        let (router, engine) = emulator();
        let case = row.label;

        let policy_arn = match row.objectives.as_ref() {
            Some(objectives) => {
                Some(create_a_policy(&router, &format!("policy-{index}"), objectives).await)
            }
            None => None,
        };
        let app_arn = create_an_app(&router, &format!("app-{index}"), policy_arn.as_deref()).await;
        for component in row.mapped {
            add_a_component(&router, &app_arn, component).await;
            add_a_resource(&router, &app_arn, component).await;
        }
        for component in row.unmapped {
            add_a_component(&router, &app_arn, component).await;
        }
        publish(&router, &app_arn).await;

        let assessed = assess(&router, &engine, &app_arn, &format!("assessment-{index}")).await;
        let score = assessed["resiliencyScore"].clone();
        let overall = score["score"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: a finished assessment carries a score, got {assessed:?}")
        });

        // A score is a fraction of its maximum, whatever it is a score of, and it
        // is the whole of that fraction exactly when every objective the policy
        // states is reached — which is while every Application Component of the
        // assessed version has something to recover it.
        let judged = row
            .objectives
            .as_ref()
            .and_then(|objectives| objectives.as_object())
            .is_some_and(|objectives| !objectives.is_empty());
        let expected_score = if judged && row.unmapped.is_empty() {
            WHOLE_SCORE
        } else {
            0.0
        };
        assert!(
            (overall - expected_score).abs() < SCORE_TOLERANCE,
            "case {case:?}: the score must be {expected_score:?}, got {overall:?}"
        );
        assert!(
            (0.0..=WHOLE_SCORE).contains(&overall),
            "case {case:?}: a score is a fraction of {WHOLE_SCORE}, got {overall:?}"
        );
        for (disruption_type, earned) in score["disruptionScore"]
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: a score carries disruptionScore, got {score:?}"))
        {
            assert_eq!(
                earned.as_f64(),
                Some(expected_score),
                "case {case:?}: {disruption_type} must earn {expected_score:?} of \
                 {WHOLE_SCORE}, got {earned:?}"
            );
        }

        // One entry per scoring component, and nothing else.
        let components = score["componentScore"].as_object().unwrap_or_else(|| {
            panic!("case {case:?}: a finished assessment breaks its score down, got {score:?}")
        });
        assert_eq!(
            components.keys().cloned().collect::<BTreeSet<String>>(),
            SCORING_COMPONENTS
                .iter()
                .map(|component| (*component).to_string())
                .collect::<BTreeSet<String>>(),
            "case {case:?}: expected one entry per scoring component, got {components:?}"
        );

        let mut possible_total = 0.0;
        let mut earned_total = 0.0;
        for component in SCORING_COMPONENTS {
            let entry = &components[component];
            let earned = entry["score"].as_f64().unwrap_or(f64::NAN);
            let possible = entry["possibleScore"].as_f64().unwrap_or(f64::NAN);
            assert!(
                earned <= possible,
                "case {case:?}: {component} cannot earn more than its share {possible:?}, got \
                 {earned:?}"
            );
            assert_eq!(
                entry["excludedCount"].as_i64(),
                Some(0),
                "case {case:?}: nothing is excluded from the assessment, got {entry:?}"
            );
            possible_total += possible;
            earned_total += earned;
        }

        // The shares add up to the whole of a score, and what the components earn
        // adds up to the score itself.
        assert!(
            (possible_total - WHOLE_SCORE).abs() < SCORE_TOLERANCE,
            "case {case:?}: the shares must add up to {WHOLE_SCORE}, got {possible_total:?} over \
             {components:?}"
        );
        assert!(
            (earned_total - overall).abs() < SCORE_TOLERANCE,
            "case {case:?}: what the components earn must add up to the score {overall:?}, got \
             {earned_total:?} over {components:?}"
        );

        // Compliance is the one component this service grades, so it carries the
        // whole share, earns exactly the score, and counts the Application
        // Components that breached the policy — none while there is no objective to
        // breach.
        let compliance = &components[COMPLIANCE_COMPONENT];
        let outstanding = if judged { row.unmapped.len() as i64 } else { 0 };
        assert_eq!(
            (
                compliance["possibleScore"].as_f64(),
                compliance["score"].as_f64(),
                compliance["outstandingCount"].as_i64()
            ),
            (Some(WHOLE_SCORE), Some(overall), Some(outstanding)),
            "case {case:?}: compliance must carry the whole share {WHOLE_SCORE}, earn the score \
             {overall:?} and report {outstanding} outstanding, got {compliance:?}"
        );

        // Nothing recorded a test, alarm or standard operating procedure
        // recommendation, so those components have nothing to earn and nothing
        // outstanding.
        for component in SCORING_COMPONENTS
            .iter()
            .filter(|component| **component != COMPLIANCE_COMPONENT)
        {
            let entry = &components[*component];
            assert_eq!(
                (
                    entry["possibleScore"].as_f64(),
                    entry["score"].as_f64(),
                    entry["outstandingCount"].as_i64()
                ),
                (Some(0.0), Some(0.0), Some(0)),
                "case {case:?}: {component} has no recommendation to score, got {entry:?}"
            );
        }

        // The application's own score is the one its newest finished assessment
        // reported — the same fraction, through the reader of an application.
        let app = post(&router, "/describe-app", &json!({"appArn": app_arn})).await;
        let app = app.ok(case)["app"].clone();
        assert_eq!(
            app["resiliencyScore"].as_f64(),
            Some(overall),
            "case {case:?}: the application must report the assessment's score {overall:?}, got \
             {app:?}"
        );
    }
}
