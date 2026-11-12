// SPDX-License-Identifier: CC0-1.0

//! `ListRoute53HealthChecks` over the wire.
//!
//! The rule under test: the operation reports one health check per
//! `(hostedZoneId, recordName, region)` triple that a `Route53HealthCheck` execution
//! block of the named plan's workflows configures — the block's `hostedZoneId` and
//! `recordName` with the `region` of each of its `recordSets`, or with each Region the
//! plan covers when the block singles out none — deduplicated, in ascending order of that
//! triple, narrowed by the request's `hostedZoneId` and `recordName` filters, and paged by
//! `maxResults`/`nextToken`.
//!
//! Every case drives the emulator through the same `MultiServiceRouter::dispatch` the
//! router serves on a socket, so what these tests observe is what an SDK observes:
//! `awsJson1_0`, `POST /`, `X-Amz-Target: ArcRegionSwitch.<Operation>`.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_arcregionswitch_arcregionswitch::state::HEALTH_CHECK_STATUS_UNKNOWN;
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

/// The canonical account id every ARN a generated emulator mints carries
/// (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// A router carrying this one service on a fresh shared engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // This suite exercises operation behaviour, not request signing.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_arcregionswitch_arcregionswitch::EmulatedService::new(engine),
    );
    router
}

/// Call one operation and return its HTTP status and parsed body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, "us-east-1", operation, body).await
}

/// Call one operation signed for `region` and return its HTTP status and parsed body.
async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("ArcRegionSwitch.{operation}"))
        // The router keys on the signing name in the SigV4 credential scope.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDTEST/20260101/{region}/arc-region-switch/aws4_request, \
                 SignedHeaders=host, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code of an error response: the `__type` member's shape name.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `recordSets` of a health-check block, one per Region in `regions`.
pub(crate) fn record_sets(regions: &[&str]) -> Value {
    Value::Array(
        regions
            .iter()
            .map(|region| {
                json!({
                    "recordSetIdentifier": format!("set-{region}"),
                    "region": region,
                })
            })
            .collect(),
    )
}

/// A `Route53HealthCheck` step whose configuration carries `record_sets`.
pub(crate) fn health_check_step(
    name: &str,
    hosted_zone_id: &str,
    record_name: &str,
    record_sets: Value,
) -> Value {
    json!({
        "name": name,
        "executionBlockType": "Route53HealthCheck",
        "executionBlockConfiguration": {
            "route53HealthCheckConfig": {
                // Sent explicitly: the member carries `@default: 60`, which the framework
                // materialises anyway.
                "timeoutMinutes": 60,
                "hostedZoneId": hosted_zone_id,
                "recordName": record_name,
                "recordSets": record_sets,
            }
        }
    })
}

/// A `ManualApproval` step — a block that configures no Route 53 health check.
fn approval_step(name: &str) -> Value {
    json!({
        "name": name,
        "executionBlockType": "ManualApproval",
        "executionBlockConfiguration": {
            "executionApprovalConfig": {
                "approvalRole": format!("arn:aws:iam::{ACCOUNT}:role/Approver"),
                "timeoutMinutes": 60,
            }
        }
    })
}

/// A `Parallel` step nesting `steps`.
fn parallel_step(name: &str, steps: Vec<Value>) -> Value {
    json!({
        "name": name,
        "executionBlockType": "Parallel",
        "executionBlockConfiguration": {
            "parallelConfig": {"steps": steps}
        }
    })
}

/// Create a plan whose `activate` workflow runs `steps`, and return its ARN.
async fn create_plan(router: &MultiServiceRouter, name: &str, steps: Vec<Value>) -> String {
    create_plan_covering(router, name, ["us-east-1", "us-west-2"], steps).await
}

/// Create a plan covering `regions` whose `activate` workflow runs `steps`, and return its
/// ARN. The Region pair matters to the Region-scoped list, which resolves its `arn` only
/// from a Region the plan covers.
async fn create_plan_covering(
    router: &MultiServiceRouter,
    name: &str,
    regions: [&str; 2],
    steps: Vec<Value>,
) -> String {
    let request = json!({
        "name": name,
        "regions": regions,
        "recoveryApproach": "activePassive",
        "executionRole": format!("arn:aws:iam::{ACCOUNT}:role/ArcRegionSwitchRole"),
        "workflows": [{
            "workflowTargetAction": "activate",
            "workflowDescription": "the activate workflow",
            "steps": steps,
        }],
    });
    let (status, created) = call(router, "CreatePlan", &request).await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreatePlan expected 200, got {status} with {created}"
    );
    created["plan"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The `(hostedZoneId, recordName, region)` triples a response reports, in the order it
/// reported them.
fn reported_triples(body: &Value) -> Vec<(String, String, String)> {
    body["healthChecks"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|check| {
            (
                check["hostedZoneId"].as_str().unwrap_or("").to_string(),
                check["recordName"].as_str().unwrap_or("").to_string(),
                check["region"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

/// The triples `blocks` — `(hostedZoneId, recordName, [regions])` — are expected to
/// yield: deduplicated and in ascending order, computed here independently of the
/// handler.
fn expected_triples(blocks: &[(&str, &str, &[&str])]) -> Vec<(String, String, String)> {
    let mut expected: Vec<(String, String, String)> = blocks
        .iter()
        .flat_map(|(hosted_zone_id, record_name, regions)| {
            regions.iter().map(move |region| {
                (
                    (*hosted_zone_id).to_string(),
                    (*record_name).to_string(),
                    (*region).to_string(),
                )
            })
        })
        .collect();
    expected.sort();
    expected.dedup();
    expected
}

/// Assert every reported health check carries the members this emulator can know:
/// `status` is the modelled `unknown` (nothing probed the record's endpoint),
/// `healthCheckId` is absent (no Route 53 health check was created for it), and `region`
/// matches `Region`'s pattern `^[a-z]{2}-[a-z-]+-\d+$`.
fn assert_check_members(case: &str, body: &Value) {
    for check in body["healthChecks"].as_array().cloned().unwrap_or_default() {
        assert_eq!(
            check["status"],
            json!(HEALTH_CHECK_STATUS_UNKNOWN),
            "case {case:?}: expected status {HEALTH_CHECK_STATUS_UNKNOWN:?}, got {}",
            check["status"]
        );
        assert!(
            check.get("healthCheckId").is_none_or(Value::is_null),
            "case {case:?}: expected no healthCheckId, got {}",
            check["healthCheckId"]
        );
        let region = check["region"].as_str().unwrap_or_default();
        let mut parts = region.split('-');
        let geography = parts.next().unwrap_or_default();
        let number = region.rsplit('-').next().unwrap_or_default();
        assert!(
            geography.len() == 2
                && geography.chars().all(|c| c.is_ascii_lowercase())
                && !number.is_empty()
                && number.chars().all(|c| c.is_ascii_digit())
                && region.split('-').count() >= 3,
            "case {case:?}: expected a Region matching ^[a-z]{{2}}-[a-z-]+-\\d+$, got {region:?}"
        );
    }
}

/// The health checks a plan reports are exactly the triples its `Route53HealthCheck`
/// blocks configure — whatever shape those blocks take.
#[tokio::test]
async fn health_checks_are_the_triples_the_plan_configures() {
    let router = router();
    // The rows differ along what the rule generalises over: how many health-check blocks
    // a plan has, how many Regions each names, whether the block is nested in a
    // `Parallel` one, whether two blocks name the same triple, and whether a block names
    // a Region at all.
    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, Vec<Value>, Vec<(&str, &str, &[&str])>)> = vec![
        (
            "no health-check block at all",
            vec![approval_step("approve"), approval_step("approve-again")],
            vec![],
        ),
        (
            "no step at all",
            vec![],
            vec![],
        ),
        (
            "one block, one Region",
            vec![health_check_step(
                "hc",
                "Z1EXAMPLEZONEID",
                "www.example.com",
                record_sets(&["us-east-1"]),
            )],
            vec![("Z1EXAMPLEZONEID", "www.example.com", &["us-east-1"])],
        ),
        (
            "one block, two Regions, beside a block-less step",
            vec![
                approval_step("approve"),
                health_check_step(
                    "hc",
                    "Z1EXAMPLEZONEID",
                    "www.example.com",
                    record_sets(&["us-west-2", "us-east-1"]),
                ),
            ],
            vec![(
                "Z1EXAMPLEZONEID",
                "www.example.com",
                &["us-east-1", "us-west-2"],
            )],
        ),
        (
            "two blocks: two hosted zones and three record names",
            vec![
                health_check_step(
                    "hc-apex",
                    "Z1EXAMPLEZONEID",
                    "example.com",
                    record_sets(&["eu-west-1", "eu-central-1"]),
                ),
                health_check_step(
                    "hc-api",
                    "Z2OTHERZONEID",
                    "api.example.com",
                    record_sets(&["ap-southeast-2"]),
                ),
            ],
            vec![
                (
                    "Z1EXAMPLEZONEID",
                    "example.com",
                    &["eu-central-1", "eu-west-1"],
                ),
                ("Z2OTHERZONEID", "api.example.com", &["ap-southeast-2"]),
            ],
        ),
        (
            "a block nested inside a Parallel block",
            vec![parallel_step(
                "fan-out",
                vec![
                    approval_step("approve"),
                    health_check_step(
                        "hc-nested",
                        "Z3NESTEDZONEID",
                        "nested.example.com",
                        record_sets(&["ca-central-1"]),
                    ),
                ],
            )],
            vec![("Z3NESTEDZONEID", "nested.example.com", &["ca-central-1"])],
        ),
        (
            "two blocks naming the same record in the same Region",
            vec![
                health_check_step(
                    "hc-first",
                    "Z1EXAMPLEZONEID",
                    "dup.example.com",
                    record_sets(&["sa-east-1"]),
                ),
                health_check_step(
                    "hc-second",
                    "Z1EXAMPLEZONEID",
                    "dup.example.com",
                    record_sets(&["sa-east-1", "us-east-2"]),
                ),
            ],
            vec![(
                "Z1EXAMPLEZONEID",
                "dup.example.com",
                &["sa-east-1", "us-east-2"],
            )],
        ),
        (
            "a block whose record sets name no Region",
            vec![health_check_step(
                "hc-regionless",
                "Z4REGIONLESSZONEID",
                "regionless.example.com",
                json!([{"recordSetIdentifier": "only-an-identifier"}]),
            )],
            // The block singles out no Region, so the record it names is checked in each
            // Region the plan covers.
            vec![(
                "Z4REGIONLESSZONEID",
                "regionless.example.com",
                &["us-east-1", "us-west-2"],
            )],
        ),
        (
            "a block carrying no record sets at all",
            vec![health_check_step(
                "hc-setless",
                "Z5SETLESSZONEID",
                "setless.example.com",
                json!([]),
            )],
            vec![(
                "Z5SETLESSZONEID",
                "setless.example.com",
                &["us-east-1", "us-west-2"],
            )],
        ),
    ];

    for (index, (case, steps, blocks)) in cases.into_iter().enumerate() {
        let arn = create_plan(&router, &format!("plan-{index}"), steps).await;
        let (status, body) = call(&router, "ListRoute53HealthChecks", &json!({"arn": arn})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        let expected = expected_triples(&blocks);
        let reported = reported_triples(&body);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected health checks {expected:?}, got {reported:?} in {body}"
        );
        assert!(
            body["healthChecks"].is_array(),
            "case {case:?}: expected a healthChecks list, got {}",
            body["healthChecks"]
        );
        assert!(
            body.get("nextToken").is_none_or(Value::is_null),
            "case {case:?}: a single unfiltered page expects no nextToken, got {}",
            body["nextToken"]
        );
        assert_check_members(case, &body);

        // Reading does not change what a later caller sees.
        let (again_status, again) =
            call(&router, "ListRoute53HealthChecks", &json!({"arn": arn})).await;
        assert_eq!(
            (again_status, reported_triples(&again)),
            (status, reported.clone()),
            "case {case:?}: a second read expected {reported:?}, got {again}"
        );
    }
}

/// `hostedZoneId` and `recordName` narrow the list to the checks that carry exactly
/// that value; an absent filter selects every check.
#[tokio::test]
async fn filters_select_by_exact_hosted_zone_and_record_name() {
    let router = router();
    let steps = vec![
        health_check_step(
            "hc-apex",
            "Zalpha",
            "example.com",
            record_sets(&["us-east-1", "us-west-2"]),
        ),
        health_check_step(
            "hc-api",
            "Zalpha",
            "api.example.com",
            record_sets(&["us-east-1"]),
        ),
        health_check_step(
            "hc-beta",
            "Zbeta",
            "example.com",
            record_sets(&["eu-west-1"]),
        ),
    ];
    let arn = create_plan(&router, "filtered-plan", steps).await;

    // Each row: the filter members sent, and the triples it selects.
    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, Value, Vec<(&str, &str, &[&str])>)> = vec![
        (
            "no filter",
            json!({}),
            vec![
                ("Zalpha", "example.com", &["us-east-1", "us-west-2"][..]),
                ("Zalpha", "api.example.com", &["us-east-1"][..]),
                ("Zbeta", "example.com", &["eu-west-1"][..]),
            ],
        ),
        (
            "hostedZoneId only",
            json!({"hostedZoneId": "Zalpha"}),
            vec![
                ("Zalpha", "example.com", &["us-east-1", "us-west-2"][..]),
                ("Zalpha", "api.example.com", &["us-east-1"][..]),
            ],
        ),
        (
            "recordName only, shared by two hosted zones",
            json!({"recordName": "example.com"}),
            vec![
                ("Zalpha", "example.com", &["us-east-1", "us-west-2"][..]),
                ("Zbeta", "example.com", &["eu-west-1"][..]),
            ],
        ),
        (
            "both filters",
            json!({"hostedZoneId": "Zbeta", "recordName": "example.com"}),
            vec![("Zbeta", "example.com", &["eu-west-1"][..])],
        ),
        (
            "both filters, combination no block configures",
            json!({"hostedZoneId": "Zbeta", "recordName": "api.example.com"}),
            vec![],
        ),
        (
            "hostedZoneId no block configures",
            json!({"hostedZoneId": "Zgamma"}),
            vec![],
        ),
        (
            "recordName matching no block: a prefix of one, not the whole value",
            json!({"recordName": "example"}),
            vec![],
        ),
    ];

    for (case, filter, blocks) in cases {
        let mut request = json!({"arn": arn});
        for (member, value) in filter.as_object().cloned().unwrap_or_default() {
            request[member] = value;
        }
        let (status, body) = call(&router, "ListRoute53HealthChecks", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        let expected = expected_triples(&blocks);
        let reported = reported_triples(&body);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected health checks {expected:?}, got {reported:?} in {body}"
        );
        assert_check_members(case, &body);
    }
}

/// Paging with any `maxResults` the model allows walks every health check exactly once,
/// in the same order one unpaged call reports, and ends with no token.
#[tokio::test]
async fn pagination_walks_every_health_check_once() {
    let router = router();
    let steps = vec![
        health_check_step(
            "hc-apex",
            "Zalpha",
            "example.com",
            record_sets(&["us-east-1", "us-west-2", "eu-west-1"]),
        ),
        health_check_step(
            "hc-api",
            "Zbeta",
            "api.example.com",
            record_sets(&["ap-south-1", "ap-northeast-1"]),
        ),
    ];
    let arn = create_plan(&router, "paged-plan", steps).await;

    let (status, whole) = call(&router, "ListRoute53HealthChecks", &json!({"arn": arn})).await;
    assert_eq!(
        status, 200,
        "case \"unpaged\": expected 200, got {status} with {whole}"
    );
    let all = reported_triples(&whole);
    assert_eq!(
        all.len(),
        5,
        "case \"unpaged\": expected the 5 configured triples, got {all:?}"
    );

    // `maxResults` is `@range(1..100)`: both published bounds and the sizes around the
    // 5-item list.
    for page_size in [1usize, 2, 4, 5, 6, 100] {
        let case = format!("maxResults {page_size}");
        let mut walked: Vec<(String, String, String)> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut request = json!({"arn": arn, "maxResults": page_size});
            if let Some(token) = token.as_deref() {
                request["nextToken"] = json!(token);
            }
            let (status, body) = call(&router, "ListRoute53HealthChecks", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} with {body}"
            );
            let page = reported_triples(&body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: expected at most {page_size} per page, got {page:?}"
            );
            assert_check_members(&case, &body);
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
            token = body["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, all,
            "case {case:?}: expected paging to walk {all:?}, got {walked:?}"
        );
        let expected_pages = all.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, got {pages}"
        );
    }
}

/// A plan nobody created is a 404 `ResourceNotFoundException`, and a `nextToken` the
/// service never issued is a 400 `IllegalArgumentException`.
#[tokio::test]
async fn unknown_plan_and_unissued_token_are_refused() {
    let router = router();
    let arn = create_plan(
        &router,
        "refusal-plan",
        vec![health_check_step(
            "hc",
            "Zalpha",
            "example.com",
            record_sets(&["us-east-1"]),
        )],
    )
    .await;

    // Rows differ along the shape of the refused value: a well-formed ARN for no plan,
    // tokens that are not positions, and positions at or past the end of the list.
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a well-formed ARN no plan has",
            json!({"arn": format!("arn:aws:arc-region-switch::{ACCOUNT}:plan/absent:abcdef")}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a nextToken that is not a position",
            json!({"arn": arn, "nextToken": "not-a-position"}),
            400,
            "IllegalArgumentException",
        ),
        (
            "a nextToken that is a negative position",
            json!({"arn": arn, "nextToken": "-1"}),
            400,
            "IllegalArgumentException",
        ),
        (
            "a nextToken at the end of the one-check list",
            json!({"arn": arn, "nextToken": "1"}),
            400,
            "IllegalArgumentException",
        ),
        (
            "a nextToken past the end of the one-check list",
            json!({"arn": arn, "nextToken": "99"}),
            400,
            "IllegalArgumentException",
        ),
    ];

    for (case, request, expected_status, expected_code) in cases {
        let (status, body) = call(&router, "ListRoute53HealthChecks", &request).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} with {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected error code {expected_code:?}, got {code:?} in {body}"
        );
    }
}

/// `ListRoute53HealthChecksInRegion` reports the same projection narrowed to the Region
/// the call was made in — the Region of its SigV4 credential scope, since the input
/// names none — and its filters and paging behave like the global list's.
///
/// Being Region-scoped it reports only from a Region the plan covers; from any other
/// Region the `arn` resolves to no plan, which is
/// [`crate::in_region_unresolvable_plan`]'s subject.
#[tokio::test]
async fn in_region_reports_only_the_health_checks_of_the_calling_region() {
    let router = router();
    let steps = vec![
        health_check_step(
            "hc-apex",
            "Zalpha",
            "example.com",
            record_sets(&["us-east-1", "us-west-2", "eu-west-1"]),
        ),
        health_check_step(
            "hc-api",
            "Zbeta",
            "api.example.com",
            record_sets(&["us-east-1"]),
        ),
    ];
    let arn = create_plan(&router, "in-region-plan", steps).await;
    // A second plan, covering another Region pair, so a Region a plan covers but
    // configures no health check in is a case of its own.
    let eu_arn = create_plan_covering(
        &router,
        "eu-in-region-plan",
        ["eu-west-1", "eu-central-1"],
        vec![health_check_step(
            "hc-eu",
            "Zgamma",
            "eu.example.com",
            record_sets(&["eu-west-1"]),
        )],
    )
    .await;

    // Rows differ along the Region called from, among the Regions the row's plan covers:
    // one holding two checks, ones holding one, and one the plan configures nothing in.
    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, &str, Vec<(&str, &str, &[&str])>)> = vec![
        (
            &arn,
            "us-east-1",
            vec![
                ("Zalpha", "example.com", &["us-east-1"][..]),
                ("Zbeta", "api.example.com", &["us-east-1"][..]),
            ],
        ),
        (
            &arn,
            "us-west-2",
            vec![("Zalpha", "example.com", &["us-west-2"][..])],
        ),
        (
            &eu_arn,
            "eu-west-1",
            vec![("Zgamma", "eu.example.com", &["eu-west-1"][..])],
        ),
        (&eu_arn, "eu-central-1", vec![]),
    ];

    for (plan, region, blocks) in cases {
        let (status, body) = call_in_region(
            &router,
            region,
            "ListRoute53HealthChecksInRegion",
            &json!({"arn": plan}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected 200, got {status} with {body}"
        );
        let expected = expected_triples(&blocks);
        let reported = reported_triples(&body);
        assert_eq!(
            reported, expected,
            "case {region:?}: expected health checks {expected:?}, got {reported:?} in {body}"
        );
        assert_check_members(region, &body);

        // The filters narrow the regional list the same way they narrow the global one.
        let hosted_zone_id = expected
            .first()
            .map(|(hosted_zone_id, _, _)| hosted_zone_id.clone())
            .unwrap_or_else(|| "Zalpha".to_string());
        let (status, filtered) = call_in_region(
            &router,
            region,
            "ListRoute53HealthChecksInRegion",
            &json!({"arn": plan, "hostedZoneId": hosted_zone_id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?} filtered: expected 200, got {status} with {filtered}"
        );
        let expected_filtered: Vec<(String, String, String)> = expected
            .iter()
            .filter(|(zone, _, _)| *zone == hosted_zone_id)
            .cloned()
            .collect();
        let reported_filtered = reported_triples(&filtered);
        assert_eq!(
            reported_filtered, expected_filtered,
            "case {region:?} filtered: expected {expected_filtered:?}, got \
             {reported_filtered:?} in {filtered}"
        );
    }

    // An `arn` this Region cannot resolve a plan from is the not-found refusal at the
    // status a Region-scoped read answers it with.
    let (status, body) = call_in_region(
        &router,
        "us-east-1",
        "ListRoute53HealthChecksInRegion",
        &json!({"arn": format!("arn:aws:arc-region-switch::{ACCOUNT}:plan/absent:abcdef")}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"absent plan\": expected 400, got {status} with {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "ResourceNotFoundException",
        "case \"absent plan\": expected ResourceNotFoundException, got {code:?} in {body}"
    );

    let mut walked: Vec<(String, String, String)> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({"arn": arn, "maxResults": 1});
        if let Some(token) = token.as_deref() {
            request["nextToken"] = json!(token);
        }
        let (status, body) = call_in_region(
            &router,
            "us-east-1",
            "ListRoute53HealthChecksInRegion",
            &request,
        )
        .await;
        assert_eq!(
            status, 200,
            "case \"paged in us-east-1\": expected 200, got {status} with {body}"
        );
        walked.extend(reported_triples(&body));
        token = body["nextToken"].as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            walked.len() <= 2,
            "case \"paged in us-east-1\": paging did not terminate, walked {walked:?}"
        );
    }
    let expected = expected_triples(&[
        ("Zalpha", "example.com", &["us-east-1"]),
        ("Zbeta", "api.example.com", &["us-east-1"]),
    ]);
    assert_eq!(
        walked, expected,
        "case \"paged in us-east-1\": expected paging to walk {expected:?}, got {walked:?}"
    );
}

/// The list follows the plan: a plan deleted after its health checks were read reports
/// them no longer.
#[tokio::test]
async fn health_checks_do_not_outlive_their_plan() {
    let router = router();
    let arn = create_plan(
        &router,
        "deleted-plan",
        vec![health_check_step(
            "hc",
            "Zalpha",
            "example.com",
            record_sets(&["us-east-1", "us-west-2"]),
        )],
    )
    .await;
    let (status, body) = call(&router, "ListRoute53HealthChecks", &json!({"arn": arn})).await;
    assert_eq!(
        status, 200,
        "case \"before delete\": expected 200, got {status} with {body}"
    );
    assert_eq!(
        reported_triples(&body).len(),
        2,
        "case \"before delete\": expected the 2 configured triples, got {body}"
    );

    let (status, deleted) = call(&router, "DeletePlan", &json!({"arn": arn})).await;
    assert_eq!(
        status, 200,
        "case \"delete\": expected 200, got {status} with {deleted}"
    );

    let (status, body) = call(&router, "ListRoute53HealthChecks", &json!({"arn": arn})).await;
    assert_eq!(
        status, 404,
        "case \"after delete\": expected 404, got {status} with {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "ResourceNotFoundException",
        "case \"after delete\": expected ResourceNotFoundException, got {code:?} in {body}"
    );
}
