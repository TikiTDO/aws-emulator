// SPDX-License-Identifier: CC0-1.0

//! The Resilience Hub application + resiliency-policy lifecycle, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a
//! restJson1 request — and asserts the response BODY, not just the status. The rule
//! under test is: every one of these operations computes its answer from the single
//! shared store, so a write is observable by a later read and a read is observable by
//! nobody.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_resiliencehub_awsresiliencehub::ops::types::{
    APP_ASSESSMENT_SCHEDULE_TYPE_VALUES, DATA_LOCATION_CONSTRAINT_VALUES, DISRUPTION_TYPE_VALUES,
    EVENT_TYPE_VALUES, PERMISSION_MODEL_TYPE_VALUES, RESILIENCY_POLICY_TIER_VALUES,
};
use service_com_amazonaws_resiliencehub_awsresiliencehub::state::{
    ComAmazonawsResiliencehubAwsresiliencehubState, MAX_APPS, MAX_IDEMPOTENCY_TOKENS,
    MAX_RESILIENCY_POLICIES,
};
use service_com_amazonaws_resiliencehub_awsresiliencehub::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for, so the ARNs the emulator mints are
/// predictable.
pub(crate) const REGION: &str = "us-east-1";

/// The account segment of every emulated ARN (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = "000000000000";

/// An emulator holding only this service, over one shared engine.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These requests carry an unverifiable placeholder signature: this suite tests
    // the operations, not SigV4.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What a dispatched request answered.
#[derive(Debug)]
pub(crate) struct Reply {
    pub(crate) status: u16,
    /// The restJson1 error shape name, from `x-amzn-errortype`; empty on success.
    pub(crate) error_code: String,
    pub(crate) body: Value,
}

impl Reply {
    /// The response body, asserting the call succeeded.
    pub(crate) fn ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} {:?} {:?}",
            self.status, self.error_code, self.body
        );
        &self.body
    }

    /// Assert the call was refused with `status` and `code`.
    pub(crate) fn refused(&self, case: &str, status: u16, code: &str) {
        assert_eq!(
            (self.status, self.error_code.as_str()),
            (status, code),
            "case {case:?}: expected HTTP {status} {code}, got {} {:?} {:?}",
            self.status,
            self.error_code,
            self.body
        );
    }
}

/// Dispatch one request through the router, exactly as the server front door does,
/// signed for `signing_region` (the SigV4 credential scope the service reads to
/// choose the region of the ARNs it mints).
async fn call(
    router: &MultiServiceRouter,
    signing_region: &str,
    method: &str,
    target: &str,
    body: Option<&Value>,
) -> Reply {
    let payload = body.map_or_else(Vec::new, |b| serde_json::to_vec(b).expect("serializable body"));
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://resiliencehub.{REGION}.amazonaws.com{target}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{signing_region}/resiliencehub/\
                 aws4_request, SignedHeaders=host, Signature=00"
            ),
        );
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let raw = response.into_body();
    let body = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or_else(|_| json!({"raw": String::from_utf8_lossy(&raw)}))
    };
    Reply {
        status,
        error_code,
        body,
    }
}

pub(crate) async fn post(router: &MultiServiceRouter, path: &str, body: &Value) -> Reply {
    call(router, REGION, "POST", path, Some(body)).await
}

pub(crate) async fn get(router: &MultiServiceRouter, target: &str) -> Reply {
    call(router, REGION, "GET", target, None).await
}

/// A DELETE request with no body, which is how the model binds `UntagResource`.
pub(crate) async fn delete(router: &MultiServiceRouter, target: &str) -> Reply {
    call(router, REGION, "DELETE", target, None).await
}

/// A resiliency policy to point applications at, and its ARN.
pub(crate) async fn create_a_policy(router: &MultiServiceRouter, policy_name: &str) -> String {
    let reply = post(
        router,
        "/create-resiliency-policy",
        &json!({
            "policyName": policy_name,
            "tier": "Important",
            "policy": {"AZ": {"rtoInSecs": 3600, "rpoInSecs": 3600}},
        }),
    )
    .await;
    reply.ok(policy_name)["policy"]["policyArn"]
        .as_str()
        .expect("a minted policyArn")
        .to_string()
}

/// Whether `text` is a canonical 8-4-4-4-12 hex UUID.
fn is_uuid(text: &str) -> bool {
    let groups: Vec<&str> = text.split('-').collect();
    groups.len() == 5
        && [8usize, 4, 4, 4, 12]
            == [
                groups[0].len(),
                groups[1].len(),
                groups[2].len(),
                groups[3].len(),
                groups[4].len(),
            ]
        && groups
            .iter()
            .all(|group| group.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Assert `arn` is `arn:aws:resiliencehub:<region>:<account>:<kind>/<uuid>`.
fn assert_minted_arn(case: &str, arn: &str, kind: &str) {
    assert_minted_arn_in(case, arn, kind, REGION);
}

/// Assert `arn` is `arn:aws:resiliencehub:<region>:<account>:<kind>/<uuid>` for
/// `region`.
fn assert_minted_arn_in(case: &str, arn: &str, kind: &str, region: &str) {
    let prefix = format!("arn:aws:resiliencehub:{region}:{ACCOUNT}:{kind}/");
    let Some(id) = arn.strip_prefix(&prefix) else {
        panic!("case {case:?}: expected an ARN starting {prefix:?}, got {arn:?}");
    };
    assert!(
        is_uuid(id),
        "case {case:?}: expected a UUID resource id, got {id:?} (arn {arn:?})"
    );
}

/// A row of the lifecycle table: an application shape, and the partial update to
/// apply to it.
struct Lifecycle {
    label: &'static str,
    /// The `CreateApp` body, minus the `policyArn` that `references_a_policy` adds.
    create: fn() -> Value,
    references_a_policy: bool,
    /// The `UpdateApp` body, minus `appArn`.
    update: fn() -> Value,
}

/// The lifecycle rule, over a table spanning the optional members of `CreateApp`
/// and of `UpdateApp`: create persists, describe reads back field-for-field, list
/// projects, update mutates only what it names, delete removes.
#[tokio::test]
async fn create_describe_list_update_delete_round_trip_the_stored_application() {
    let router = emulator();
    let policy_arn = create_a_policy(&router, "lifecycle-policy").await;

    let table = [
        Lifecycle {
            label: "only the required name",
            create: || json!({"name": "lc-minimal"}),
            references_a_policy: false,
            update: || json!({"description": "described by the update"}),
        },
        Lifecycle {
            label: "description, one tag, a Daily schedule",
            create: || {
                json!({
                    "name": "lc-daily",
                    "description": "a daily-assessed app",
                    "tags": {"env": "test"},
                    "assessmentSchedule": "Daily",
                })
            },
            references_a_policy: false,
            update: || json!({"assessmentSchedule": "Disabled"}),
        },
        Lifecycle {
            label: "an empty description, three tags and a policy reference",
            create: || {
                json!({
                    "name": "lc_policy-ref",
                    "description": "",
                    "tags": {"a": "1", "b": "2", "c": "3"},
                })
            },
            references_a_policy: true,
            update: || json!({"clearResiliencyPolicyArn": true}),
        },
        Lifecycle {
            label: "an AppRegistry application ARN and an event subscription",
            create: || {
                json!({
                    "name": "lc-events",
                    "awsApplicationArn": format!("arn:aws:resource-groups:{REGION}:{ACCOUNT}:group/demo"),
                    "eventSubscriptions": [{"name": "drift", "eventType": "DriftDetected"}],
                })
            },
            references_a_policy: false,
            update: || json!({"eventSubscriptions": []}),
        },
        Lifecycle {
            label: "a role-based permission model",
            create: || {
                json!({
                    "name": "lc-permissions",
                    "permissionModel": {"type": "RoleBased", "invokerRoleName": "resilience-invoker"},
                })
            },
            references_a_policy: false,
            update: || json!({"permissionModel": {"type": "LegacyIAMUser"}}),
        },
        Lifecycle {
            label: "a policy reference replaced by the update",
            create: || json!({"name": "lc-repoint", "description": "repointed"}),
            references_a_policy: false,
            update: || json!({}),
        },
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();

    for row in &table {
        let case = row.label;
        let mut create_body = (row.create)();
        if row.references_a_policy {
            create_body
                .as_object_mut()
                .expect("a JSON object")
                .insert("policyArn".to_string(), json!(policy_arn));
        }

        // ---- create ----------------------------------------------------------
        let created = post(&router, "/create-app", &create_body).await;
        let created_app = created.ok(case)["app"].clone();
        let app_arn = created_app["appArn"]
            .as_str()
            .expect("appArn is @required")
            .to_string();
        assert_minted_arn(case, &app_arn, "app");
        assert!(
            minted.insert(app_arn.clone()),
            "case {case:?}: appArn {app_arn:?} was minted twice"
        );
        assert_eq!(
            created_app["status"], json!("Active"),
            "case {case:?}: expected status Active, got {:?}",
            created_app["status"]
        );
        assert_eq!(
            created_app["complianceStatus"], json!("NotAssessed"),
            "case {case:?}: expected complianceStatus NotAssessed, got {:?}",
            created_app["complianceStatus"]
        );
        assert_eq!(
            created_app["driftStatus"], json!("NotChecked"),
            "case {case:?}: expected driftStatus NotChecked, got {:?}",
            created_app["driftStatus"]
        );
        assert_eq!(
            created_app["resiliencyScore"].as_f64(),
            Some(0.0),
            "case {case:?}: expected resiliencyScore 0, got {:?}",
            created_app["resiliencyScore"]
        );
        assert!(
            created_app["creationTime"].is_number(),
            "case {case:?}: expected an epoch-seconds creationTime, got {:?}",
            created_app["creationTime"]
        );
        // Every member the request supplied comes back unchanged.
        for (member, value) in create_body.as_object().expect("a JSON object") {
            if member == "clientToken" {
                continue;
            }
            assert_eq!(
                &created_app[member], value,
                "case {case:?}: expected {member} {value:?}, got {:?}",
                created_app[member]
            );
        }

        // ---- describe reads back exactly what was persisted -------------------
        let described = post(&router, "/describe-app", &json!({"appArn": app_arn})).await;
        assert_eq!(
            described.ok(case)["app"], created_app,
            "case {case:?}: DescribeApp must return what CreateApp persisted, got {:?}",
            described.body["app"]
        );
        // A read changes nothing a later caller sees.
        let described_again = post(&router, "/describe-app", &json!({"appArn": app_arn})).await;
        assert_eq!(
            described_again.ok(case)["app"], created_app,
            "case {case:?}: a second DescribeApp must answer identically, got {:?}",
            described_again.body["app"]
        );

        // ---- list projects the stored application ----------------------------
        let name = created_app["name"].as_str().expect("name is @required");
        let listed = get(&router, &format!("/list-apps?name={name}")).await;
        let summaries = listed.ok(case)["appSummaries"]
            .as_array()
            .expect("appSummaries is @required")
            .clone();
        assert_eq!(
            summaries.len(),
            1,
            "case {case:?}: expected exactly one summary for name {name:?}, got {summaries:?}"
        );
        let summary = &summaries[0];
        for member in [
            "appArn",
            "name",
            "description",
            "creationTime",
            "complianceStatus",
            "resiliencyScore",
            "assessmentSchedule",
            "status",
            "driftStatus",
        ] {
            assert_eq!(
                summary[member], created_app[member],
                "case {case:?}: summary.{member} must project the stored app, got {:?} vs {:?}",
                summary[member], created_app[member]
            );
        }

        // ---- update mutates ONLY the members it names ------------------------
        let mut update_body = (row.update)();
        let update_object = update_body.as_object_mut().expect("a JSON object");
        if row.label == "a policy reference replaced by the update" {
            update_object.insert("policyArn".to_string(), json!(policy_arn));
        }
        let mut expected_app = created_app.clone();
        {
            let expected = expected_app.as_object_mut().expect("a JSON object");
            for (member, value) in update_object.iter() {
                if member == "clearResiliencyPolicyArn" {
                    if value == &json!(true) {
                        expected.remove("policyArn");
                    }
                } else {
                    expected.insert(member.clone(), value.clone());
                }
            }
        }
        update_object.insert("appArn".to_string(), json!(app_arn));

        let updated = post(&router, "/update-app", &update_body).await;
        assert_eq!(
            updated.ok(case)["app"], expected_app,
            "case {case:?}: UpdateApp must change only the members it names, got {:?}",
            updated.body["app"]
        );
        let described = post(&router, "/describe-app", &json!({"appArn": app_arn})).await;
        assert_eq!(
            described.ok(case)["app"], expected_app,
            "case {case:?}: DescribeApp must observe the update, got {:?}",
            described.body["app"]
        );

        // ---- delete removes it ----------------------------------------------
        let deleted = post(&router, "/delete-app", &json!({"appArn": app_arn})).await;
        assert_eq!(
            deleted.ok(case)["appArn"], json!(app_arn),
            "case {case:?}: DeleteApp must echo the deleted appArn, got {:?}",
            deleted.body["appArn"]
        );
        post(&router, "/describe-app", &json!({"appArn": app_arn}))
            .await
            .refused(case, 404, "ResourceNotFoundException");
        post(&router, "/delete-app", &json!({"appArn": app_arn}))
            .await
            .refused(case, 404, "ResourceNotFoundException");
        let listed = get(&router, &format!("/list-apps?name={name}")).await;
        assert_eq!(
            listed.ok(case)["appSummaries"],
            json!([]),
            "case {case:?}: a deleted app must not appear in ListApps, got {:?}",
            listed.body["appSummaries"]
        );
        // The name is free again once the app is gone.
        let recreated = post(&router, "/create-app", &json!({"name": name})).await;
        assert_minted_arn(
            case,
            recreated.ok(case)["app"]["appArn"]
                .as_str()
                .expect("appArn is @required"),
            "app",
        );
    }
}

/// `maxResults` is a real page bound and `nextToken` resumes exactly where the
/// previous page stopped, for every page size — including both published bounds.
#[tokio::test]
async fn list_apps_pages_with_every_page_size_and_reverses_on_request() {
    let router = emulator();
    const APPS: usize = 7;
    for index in 0..APPS {
        post(&router, "/create-app", &json!({"name": format!("page-app-{index}")}))
            .await
            .ok("seed");
    }

    let unpaged = get(&router, "/list-apps").await;
    let forward: Vec<String> = unpaged.ok("unpaged")["appSummaries"]
        .as_array()
        .expect("appSummaries is @required")
        .iter()
        .map(|summary| summary["appArn"].as_str().expect("appArn").to_string())
        .collect();
    assert_eq!(
        forward.len(),
        APPS,
        "unpaged: expected all {APPS} apps, got {forward:?}"
    );

    // Both published bounds of `maxResults` (@range 1..100) plus the sizes in
    // between that actually split this set.
    for page_size in [1usize, 2, 3, 6, 7, 100] {
        let case = format!("maxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut target = format!("/list-apps?maxResults={page_size}");
            if let Some(next) = token.as_deref() {
                target.push_str(&format!("&nextToken={next}"));
            }
            let reply = get(&router, &target).await;
            let body = reply.ok(&case);
            let page = body["appSummaries"].as_array().expect("appSummaries").clone();
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page returned {} summaries, more than maxResults",
                page.len()
            );
            walked.extend(
                page.iter()
                    .map(|summary| summary["appArn"].as_str().expect("appArn").to_string()),
            );
            pages += 1;
            assert!(
                pages <= APPS + 1,
                "case {case:?}: pagination did not terminate after {pages} pages"
            );
            match body["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, forward,
            "case {case:?}: walking the pages must yield the unpaged order exactly"
        );
    }

    // `reverseOrder` is the same total order, reversed.
    let reversed = get(&router, "/list-apps?reverseOrder=true").await;
    let backward: Vec<String> = reversed.ok("reverseOrder=true")["appSummaries"]
        .as_array()
        .expect("appSummaries")
        .iter()
        .map(|summary| summary["appArn"].as_str().expect("appArn").to_string())
        .collect();
    let mut expected = forward.clone();
    expected.reverse();
    assert_eq!(
        backward, expected,
        "case \"reverseOrder=true\": expected the forward order reversed, got {backward:?}"
    );

    // And it still pages: one element at a time, in the reversed order.
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut target = "/list-apps?reverseOrder=true&maxResults=2".to_string();
        if let Some(next) = token.as_deref() {
            target.push_str(&format!("&nextToken={next}"));
        }
        let reply = get(&router, &target).await;
        let body = reply.ok("reverseOrder + maxResults=2");
        walked.extend(
            body["appSummaries"]
                .as_array()
                .expect("appSummaries")
                .iter()
                .map(|summary| summary["appArn"].as_str().expect("appArn").to_string()),
        );
        match body["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    assert_eq!(
        walked, expected,
        "case \"reverseOrder + maxResults=2\": paging must follow the reversed order"
    );

    // One past each published bound of `maxResults` is refused by the modeled range.
    for out_of_range in ["0", "101"] {
        let case = format!("maxResults={out_of_range}");
        get(&router, &format!("/list-apps?maxResults={out_of_range}"))
            .await
            .refused(&case, 400, "ValidationException");
    }

    // A token this operation never issued is refused, not silently ignored.
    get(&router, "/list-apps?nextToken=not-a-token")
        .await
        .refused("nextToken=not-a-token", 400, "ValidationException");
}

/// Every `@httpQuery` filter narrows the listing, and a filter nothing matches
/// returns an empty page rather than everything.
#[tokio::test]
async fn list_apps_honours_every_filter() {
    let router = emulator();
    let aws_application_arn = format!("arn:aws:resource-groups:{REGION}:{ACCOUNT}:group/registered");
    let plain = post(&router, "/create-app", &json!({"name": "filter-plain"})).await;
    let plain_arn = plain.ok("filter-plain")["app"]["appArn"]
        .as_str()
        .expect("appArn")
        .to_string();
    let registered = post(
        &router,
        "/create-app",
        &json!({"name": "filter-registered", "awsApplicationArn": aws_application_arn}),
    )
    .await;
    let registered_arn = registered.ok("filter-registered")["app"]["appArn"]
        .as_str()
        .expect("appArn")
        .to_string();

    let table: Vec<(String, String, Vec<String>)> = vec![
        (
            "no filter".to_string(),
            "/list-apps".to_string(),
            vec![plain_arn.clone(), registered_arn.clone()],
        ),
        (
            "name of the plain app".to_string(),
            "/list-apps?name=filter-plain".to_string(),
            vec![plain_arn.clone()],
        ),
        (
            "name nothing holds".to_string(),
            "/list-apps?name=filter-absent".to_string(),
            vec![],
        ),
        (
            "appArn of the registered app".to_string(),
            format!("/list-apps?appArn={registered_arn}"),
            vec![registered_arn.clone()],
        ),
        (
            "appArn and a name that disagree".to_string(),
            format!("/list-apps?appArn={registered_arn}&name=filter-plain"),
            vec![],
        ),
        (
            "awsApplicationArn".to_string(),
            format!("/list-apps?awsApplicationArn={aws_application_arn}"),
            vec![registered_arn.clone()],
        ),
        (
            // Nothing has been assessed, so no app's last assessment time can fall
            // inside any range.
            "fromLastAssessmentTime on never-assessed apps".to_string(),
            "/list-apps?fromLastAssessmentTime=2020-01-01T00:00:00Z".to_string(),
            vec![],
        ),
        (
            "toLastAssessmentTime on never-assessed apps".to_string(),
            "/list-apps?toLastAssessmentTime=2100-01-01T00:00:00Z".to_string(),
            vec![],
        ),
    ];

    for (case, target, expected) in table {
        let reply = get(&router, &target).await;
        let got: BTreeSet<String> = reply.ok(&case)["appSummaries"]
            .as_array()
            .expect("appSummaries")
            .iter()
            .map(|summary| summary["appArn"].as_str().expect("appArn").to_string())
            .collect();
        let want: BTreeSet<String> = expected.into_iter().collect();
        assert_eq!(
            got, want,
            "case {case:?}: expected {want:?}, got {got:?} (target {target:?})"
        );
    }
}

/// A resiliency policy round-trips through `DescribeResiliencyPolicy` for every
/// tier, every data-location constraint and every disruption type the model lists.
#[tokio::test]
async fn resiliency_policies_round_trip_for_every_modeled_enum_value() {
    let router = emulator();

    // Catalogue source: the model's `ResiliencyPolicyTier`, `DataLocationConstraint`
    // and `DisruptionType` enums, as rendered into
    // `ops::types::shared_types::*_VALUES`.
    for (index, tier) in RESILIENCY_POLICY_TIER_VALUES.iter().enumerate() {
        let constraint = DATA_LOCATION_CONSTRAINT_VALUES[index % DATA_LOCATION_CONSTRAINT_VALUES.len()];
        // A different-sized slice of the disruption catalogue per row.
        let disruptions = &DISRUPTION_TYPE_VALUES[..=(index % DISRUPTION_TYPE_VALUES.len())];
        let mut policy = serde_json::Map::new();
        for (offset, disruption) in disruptions.iter().enumerate() {
            policy.insert(
                (*disruption).to_string(),
                json!({"rtoInSecs": 60 * (offset as i64 + 1), "rpoInSecs": 30 * (offset as i64 + 1)}),
            );
        }
        let case = format!("tier {tier}, {} disruption types", disruptions.len());
        let request = json!({
            "policyName": format!("policy-{}", tier.to_lowercase()),
            "policyDescription": format!("the {tier} tier"),
            "tier": tier,
            "dataLocationConstraint": constraint,
            "policy": Value::Object(policy),
            "tags": {"tier": tier},
        });

        let created = post(&router, "/create-resiliency-policy", &request).await;
        let created_policy = created.ok(&case)["policy"].clone();
        let policy_arn = created_policy["policyArn"]
            .as_str()
            .expect("policyArn")
            .to_string();
        assert_minted_arn(&case, &policy_arn, "resiliency-policy");
        assert!(
            created_policy["creationTime"].is_number(),
            "case {case:?}: expected an epoch-seconds creationTime, got {:?}",
            created_policy["creationTime"]
        );
        for (member, value) in request.as_object().expect("a JSON object") {
            let stored_member = if member == "policyName" {
                "policyName"
            } else {
                member
            };
            assert_eq!(
                &created_policy[stored_member], value,
                "case {case:?}: expected {member} {value:?}, got {:?}",
                created_policy[stored_member]
            );
        }

        let described = post(
            &router,
            "/describe-resiliency-policy",
            &json!({"policyArn": policy_arn}),
        )
        .await;
        assert_eq!(
            described.ok(&case)["policy"], created_policy,
            "case {case:?}: DescribeResiliencyPolicy must return what was persisted, got {:?}",
            described.body["policy"]
        );
    }
}

/// Every modeled enum value is accepted and persisted, and every value outside the
/// set is refused with a 400 naming the member and the constraint — whether the
/// boundary validator refuses it (every enum MEMBER) or the handler does (a map's
/// KEY, which the validating walk does not descend into).
#[tokio::test]
async fn an_enum_value_outside_the_modeled_set_is_refused() {
    let router = emulator();

    // Accepted: every published value of every enum these operations take.
    for schedule in APP_ASSESSMENT_SCHEDULE_TYPE_VALUES {
        let case = format!("assessmentSchedule={schedule}");
        let reply = post(
            &router,
            "/create-app",
            &json!({"name": format!("enum-sched-{}", schedule.to_lowercase()), "assessmentSchedule": schedule}),
        )
        .await;
        assert_eq!(
            reply.ok(&case)["app"]["assessmentSchedule"],
            json!(schedule),
            "case {case:?}: expected the schedule to persist, got {:?}",
            reply.body["app"]["assessmentSchedule"]
        );
    }
    for model_type in PERMISSION_MODEL_TYPE_VALUES {
        let case = format!("permissionModel.type={model_type}");
        let reply = post(
            &router,
            "/create-app",
            &json!({
                "name": format!("enum-perm-{}", model_type.to_lowercase()),
                "permissionModel": {"type": model_type},
            }),
        )
        .await;
        assert_eq!(
            reply.ok(&case)["app"]["permissionModel"]["type"],
            json!(model_type),
            "case {case:?}: expected the permission model to persist, got {:?}",
            reply.body["app"]["permissionModel"]
        );
    }
    for event_type in EVENT_TYPE_VALUES {
        let case = format!("eventSubscriptions.0.eventType={event_type}");
        let reply = post(
            &router,
            "/create-app",
            &json!({
                "name": format!("enum-event-{}", event_type.to_lowercase()),
                "eventSubscriptions": [{"name": "sub", "eventType": event_type}],
            }),
        )
        .await;
        assert_eq!(
            reply.ok(&case)["app"]["eventSubscriptions"][0]["eventType"],
            json!(event_type),
            "case {case:?}: expected the subscription to persist, got {:?}",
            reply.body["app"]["eventSubscriptions"]
        );
    }

    // Refused: one value outside each set, on every operation that takes it. The
    // fourth element is the clause the refusal must carry. Most of these are
    // refused by the BOUNDARY constraint validator before the handler runs (the
    // modeled enum members, and `FailurePolicy`'s `@range(min: 0)`); the map KEY
    // case is the one the handler itself refuses, because the validating walk does
    // not descend into a map's keys. Either way the caller sees one status, one
    // code and one phrasing, which is what this table pins.
    let refusals: Vec<(&str, &str, Value, Option<&str>)> = vec![
        (
            "CreateApp assessmentSchedule",
            "/create-app",
            json!({"name": "enum-bad-sched", "assessmentSchedule": "Hourly"}),
            Some("Value 'Hourly' at 'assessmentSchedule' failed to satisfy constraint"),
        ),
        (
            "CreateApp permissionModel.type",
            "/create-app",
            json!({"name": "enum-bad-perm", "permissionModel": {"type": "Anonymous"}}),
            Some("Value 'Anonymous' at 'permissionModel.type' failed to satisfy constraint"),
        ),
        (
            "CreateApp eventSubscriptions eventType",
            "/create-app",
            json!({
                "name": "enum-bad-event",
                "eventSubscriptions": [{"name": "sub", "eventType": "Whatever"}],
            }),
            Some("Value 'Whatever' at 'eventSubscriptions.1.member.eventType' failed to satisfy constraint"),
        ),
        (
            "CreateResiliencyPolicy tier",
            "/create-resiliency-policy",
            json!({
                "policyName": "enum-bad-tier",
                "tier": "Platinum",
                "policy": {"AZ": {"rtoInSecs": 1, "rpoInSecs": 1}},
            }),
            Some("Value 'Platinum' at 'tier' failed to satisfy constraint"),
        ),
        (
            "CreateResiliencyPolicy dataLocationConstraint",
            "/create-resiliency-policy",
            json!({
                "policyName": "enum-bad-locality",
                "tier": "Critical",
                "dataLocationConstraint": "SameRack",
                "policy": {"AZ": {"rtoInSecs": 1, "rpoInSecs": 1}},
            }),
            Some("Value 'SameRack' at 'dataLocationConstraint' failed to satisfy constraint"),
        ),
        (
            "CreateResiliencyPolicy policy key",
            "/create-resiliency-policy",
            json!({
                "policyName": "enum-bad-disruption",
                "tier": "Critical",
                "policy": {"Meteorite": {"rtoInSecs": 1, "rpoInSecs": 1}},
            }),
            Some("Value 'Meteorite' at 'policy.Meteorite' failed to satisfy constraint"),
        ),
        (
            "CreateResiliencyPolicy empty policy",
            "/create-resiliency-policy",
            json!({"policyName": "enum-empty-policy", "tier": "Critical", "policy": {}}),
            Some("policy: a resiliency policy must carry a failure policy for at least one"),
        ),
        (
            "CreateResiliencyPolicy negative rtoInSecs",
            "/create-resiliency-policy",
            json!({
                "policyName": "enum-negative-rto",
                "tier": "Critical",
                "policy": {"AZ": {"rtoInSecs": -1, "rpoInSecs": 1}},
            }),
            Some("Value '-1' at 'policy.AZ.member.rtoInSecs' failed to satisfy constraint"),
        ),
    ];
    let mut dump: Vec<String> = Vec::new();
    for (case, path, body, expected_clause) in refusals {
        let reply = post(&router, path, &body).await;
        reply.refused(case, 400, "ValidationException");
        if let Some(clause) = expected_clause {
            let message = reply.body["message"].as_str().unwrap_or_default();
            if !message.contains(clause) {
                dump.push(format!(
                    "case {case:?}: expected the refusal to say {clause:?}, got {message:?}"
                ));
            }
        }
    }
    assert!(
        dump.is_empty(),
        "refusals did not name the member and constraint:\n{}",
        dump.join("\n")
    );

    // UpdateApp refuses the same out-of-set values, and leaves the stored app alone.
    let created = post(&router, "/create-app", &json!({"name": "enum-update-target"})).await;
    let app = created.ok("enum-update-target")["app"].clone();
    let app_arn = app["appArn"].as_str().expect("appArn").to_string();
    for (case, update) in [
        (
            "UpdateApp assessmentSchedule",
            json!({"appArn": app_arn, "assessmentSchedule": "Weekly"}),
        ),
        (
            "UpdateApp permissionModel.type",
            json!({"appArn": app_arn, "permissionModel": {"type": "Anonymous"}}),
        ),
        (
            "UpdateApp eventSubscriptions eventType",
            json!({
                "appArn": app_arn,
                "eventSubscriptions": [{"name": "sub", "eventType": "Whatever"}],
            }),
        ),
        (
            "UpdateApp clears and sets policyArn at once",
            json!({
                "appArn": app_arn,
                "clearResiliencyPolicyArn": true,
                "policyArn": format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:resiliency-policy/0"),
            }),
        ),
    ] {
        post(&router, "/update-app", &update)
            .await
            .refused(case, 400, "ValidationException");
        let described = post(&router, "/describe-app", &json!({"appArn": app_arn})).await;
        assert_eq!(
            described.ok(case)["app"], app,
            "case {case:?}: a refused update must leave the stored app unchanged, got {:?}",
            described.body["app"]
        );
    }
}

/// A missing resource is a 404 on every operation that resolves one — never a 200
/// carrying a synthesised resource, and never a 501.
#[tokio::test]
async fn an_unknown_arn_is_a_resource_not_found_on_every_operation() {
    let router = emulator();
    let absent_app = format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:app/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
    let absent_policy =
        format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:resiliency-policy/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");

    let table: Vec<(&str, &str, Value)> = vec![
        (
            "DescribeApp on an app that was never created",
            "/describe-app",
            json!({"appArn": absent_app}),
        ),
        (
            "UpdateApp on an app that was never created",
            "/update-app",
            json!({"appArn": absent_app, "description": "x"}),
        ),
        (
            "DeleteApp on an app that was never created",
            "/delete-app",
            json!({"appArn": absent_app}),
        ),
        (
            "DescribeResiliencyPolicy on a policy that was never created",
            "/describe-resiliency-policy",
            json!({"policyArn": absent_policy}),
        ),
        (
            "CreateApp pointing at a policy that was never created",
            "/create-app",
            json!({"name": "dangling-policy-ref", "policyArn": absent_policy}),
        ),
    ];
    for (case, path, body) in table {
        post(&router, path, &body)
            .await
            .refused(case, 404, "ResourceNotFoundException");
    }

    // The refused CreateApp stored nothing.
    let listed = get(&router, "/list-apps?name=dangling-policy-ref").await;
    assert_eq!(
        listed.ok("dangling-policy-ref")["appSummaries"],
        json!([]),
        "a CreateApp refused for a dangling policyArn must store nothing, got {:?}",
        listed.body["appSummaries"]
    );

    // UpdateApp refuses a dangling policy reference the same way.
    let created = post(&router, "/create-app", &json!({"name": "repoint-target"})).await;
    let app_arn = created.ok("repoint-target")["app"]["appArn"]
        .as_str()
        .expect("appArn")
        .to_string();
    post(
        &router,
        "/update-app",
        &json!({"appArn": app_arn, "policyArn": absent_policy}),
    )
    .await
    .refused("UpdateApp with a dangling policyArn", 404, "ResourceNotFoundException");
}

/// A replayed `@idempotencyToken` returns the first call's resource; the same token
/// with different parameters is a conflict; so is a duplicate application name.
#[tokio::test]
async fn a_replayed_client_token_returns_the_first_resource() {
    let router = emulator();

    let table: Vec<(&str, &str, Value, Value, &str)> = vec![
        (
            "CreateApp",
            "/create-app",
            json!({"name": "idem-app", "description": "first", "clientToken": "token-app-1"}),
            json!({"name": "idem-app", "description": "second", "clientToken": "token-app-1"}),
            "app",
        ),
        (
            "CreateResiliencyPolicy",
            "/create-resiliency-policy",
            json!({
                "policyName": "idem-policy",
                "tier": "Critical",
                "policy": {"AZ": {"rtoInSecs": 10, "rpoInSecs": 10}},
                "clientToken": "token-policy-1",
            }),
            json!({
                "policyName": "idem-policy",
                "tier": "NonCritical",
                "policy": {"AZ": {"rtoInSecs": 10, "rpoInSecs": 10}},
                "clientToken": "token-policy-1",
            }),
            "policy",
        ),
    ];

    for (case, path, request, divergent, output_member) in table {
        let first = post(&router, path, &request).await;
        let first_resource = first.ok(case)[output_member].clone();

        let replay = post(&router, path, &request).await;
        assert_eq!(
            replay.ok(case)[output_member], first_resource,
            "case {case:?}: a replayed clientToken must return the first resource, got {:?}",
            replay.body[output_member]
        );

        post(&router, path, &divergent)
            .await
            .refused(case, 409, "ConflictException");
    }

    // Exactly one application exists for the replayed CreateApp token.
    let listed = get(&router, "/list-apps?name=idem-app").await;
    assert_eq!(
        listed.ok("idem-app")["appSummaries"]
            .as_array()
            .expect("appSummaries")
            .len(),
        1,
        "a replayed clientToken must not create a second app, got {:?}",
        listed.body["appSummaries"]
    );

    // A different token with the same name is a NAME conflict, not a second app.
    post(
        &router,
        "/create-app",
        &json!({"name": "idem-app", "clientToken": "token-app-2"}),
    )
    .await
    .refused("CreateApp duplicate name", 409, "ConflictException");

    // Every member CreateApp stores from the request participates in the replay
    // comparison: a token replayed with a different value for ANY of them is a
    // conflict, not a silent replay of the first app.
    let base = json!({
        "name": "idem-members",
        "description": "the first call",
        "tags": {"env": "test"},
        "assessmentSchedule": "Daily",
        "permissionModel": {"type": "RoleBased", "invokerRoleName": "first-role"},
        "eventSubscriptions": [{"name": "drift", "eventType": "DriftDetected"}],
        "clientToken": "token-members",
    });
    let first = post(&router, "/create-app", &base).await;
    let first_app = first.ok("idem-members")["app"].clone();
    for (case, member, divergent) in [
        ("description", "description", json!("the second call")),
        ("tags", "tags", json!({"env": "prod"})),
        ("assessmentSchedule", "assessmentSchedule", json!("Disabled")),
        (
            "permissionModel",
            "permissionModel",
            json!({"type": "RoleBased", "invokerRoleName": "second-role"}),
        ),
        (
            "eventSubscriptions",
            "eventSubscriptions",
            json!([{"name": "drift", "eventType": "ScheduledAssessmentFailure"}]),
        ),
        (
            "awsApplicationArn",
            "awsApplicationArn",
            json!(format!("arn:aws:resource-groups:{REGION}:{ACCOUNT}:group/other")),
        ),
    ] {
        let mut request = base.clone();
        request
            .as_object_mut()
            .expect("a JSON object")
            .insert(member.to_string(), divergent);
        post(&router, "/create-app", &request)
            .await
            .refused(
                &format!("replayed clientToken with a different {case}"),
                409,
                "ConflictException",
            );
    }
    // And the unchanged request still replays the first app.
    let replay = post(&router, "/create-app", &base).await;
    assert_eq!(
        replay.ok("idem-members")["app"], first_app,
        "an identical replay must still return the first app, got {:?}",
        replay.body["app"]
    );

    // Every member CreateResiliencyPolicy stores from the request participates in
    // its replay comparison too, so the idempotency rule is the same for both
    // creators.
    let policy_base = json!({
        "policyName": "idem-policy-members",
        "policyDescription": "the first call",
        "tier": "Critical",
        "dataLocationConstraint": "AnyLocation",
        "policy": {"AZ": {"rtoInSecs": 10, "rpoInSecs": 10}},
        "tags": {"env": "test"},
        "clientToken": "token-policy-members",
    });
    let first_policy = post(&router, "/create-resiliency-policy", &policy_base).await;
    let first_policy_body = first_policy.ok("idem-policy-members")["policy"].clone();
    for (case, member, divergent) in [
        ("policyDescription", "policyDescription", json!("the second call")),
        ("tier", "tier", json!("NonCritical")),
        (
            "dataLocationConstraint",
            "dataLocationConstraint",
            json!("SameCountry"),
        ),
        (
            "policy",
            "policy",
            json!({"AZ": {"rtoInSecs": 10, "rpoInSecs": 10}, "Region": {"rtoInSecs": 60, "rpoInSecs": 60}}),
        ),
        ("tags", "tags", json!({"env": "prod"})),
    ] {
        let mut request = policy_base.clone();
        request
            .as_object_mut()
            .expect("a JSON object")
            .insert(member.to_string(), divergent);
        post(&router, "/create-resiliency-policy", &request)
            .await
            .refused(
                &format!("replayed policy clientToken with a different {case}"),
                409,
                "ConflictException",
            );
    }
    let policy_replay = post(&router, "/create-resiliency-policy", &policy_base).await;
    assert_eq!(
        policy_replay.ok("idem-policy-members")["policy"], first_policy_body,
        "an identical policy replay must still return the first policy, got {:?}",
        policy_replay.body["policy"]
    );
}

/// A minted ARN carries the caller's own region when the model's `Arn` pattern
/// accepts it, and falls back to `us-east-1` when it does not — so an ARN this
/// service hands out always satisfies the pattern its own inputs are validated
/// against.
#[tokio::test]
async fn a_minted_arn_carries_a_region_the_model_accepts() {
    // Rows differ along the shape of the SigV4 credential-scope region. The
    // accepted forms are the ones `com.amazonaws.resiliencehub#Arn`'s @pattern
    // admits: `[a-z]{2}-((iso[a-z]?-)|(gov-))?[a-z]+-[0-9]`.
    let table = [
        ("a commercial region", "eu-west-3", "eu-west-3"),
        ("a GovCloud region", "us-gov-west-1", "us-gov-west-1"),
        ("an ISO region", "us-iso-east-1", "us-iso-east-1"),
        ("an ISO-B region", "us-isob-east-1", "us-isob-east-1"),
        // The optional `iso`/`gov` infix is optional: a region whose third segment
        // merely STARTS with `iso` matches through the pattern's empty branch.
        ("an iso-prefixed ordinary segment", "us-isolated-1", "us-isolated-1"),
        // Not accepted by the pattern -> the emulator's default region.
        ("an uppercased region", "US-EAST-1", REGION),
        ("a region with no trailing digit", "eu-west", REGION),
        ("a region with a two-digit suffix", "eu-west-12", REGION),
    ];

    for (index, (case, signing_region, expected_region)) in table.iter().enumerate() {
        let router = emulator();
        let created = call(
            &router,
            signing_region,
            "POST",
            "/create-app",
            Some(&json!({"name": format!("region-app-{index}")})),
        )
        .await;
        let app_arn = created.ok(case)["app"]["appArn"]
            .as_str()
            .expect("appArn is @required")
            .to_string();
        assert_minted_arn_in(case, &app_arn, "app", expected_region);

        // The minted ARN is accepted back as input, which is the point.
        let described = call(
            &router,
            signing_region,
            "POST",
            "/describe-app",
            Some(&json!({"appArn": app_arn})),
        )
        .await;
        assert_eq!(
            described.ok(case)["app"]["appArn"], json!(app_arn),
            "case {case:?}: the minted ARN must resolve back through DescribeApp, got {:?}",
            described.body
        );

        let policy = call(
            &router,
            signing_region,
            "POST",
            "/create-resiliency-policy",
            Some(&json!({
                "policyName": format!("region-policy-{index}"),
                "tier": "Critical",
                "policy": {"AZ": {"rtoInSecs": 1, "rpoInSecs": 1}},
            })),
        )
        .await;
        assert_minted_arn_in(
            case,
            policy.ok(case)["policy"]["policyArn"]
                .as_str()
                .expect("policyArn"),
            "resiliency-policy",
            expected_region,
        );
    }
}

/// Every caller-driven store is BOUNDED: past its documented ceiling the create
/// answers the modelled `ServiceQuotaExceededException` instead of growing, and a
/// delete releases the slot it took.
#[tokio::test]
async fn each_caller_driven_store_is_bounded() {
    let router = emulator();

    // Fill `apps` to MAX_APPS, then one more.
    for index in 0..MAX_APPS {
        post(&router, "/create-app", &json!({"name": format!("cap-app-{index}")}))
            .await
            .ok(&format!("app {index} of {MAX_APPS}"));
    }
    post(&router, "/create-app", &json!({"name": "cap-app-overflow"}))
        .await
        .refused(
            "CreateApp past MAX_APPS",
            402,
            "ServiceQuotaExceededException",
        );

    // Deleting one frees exactly one slot.
    let listed = get(&router, "/list-apps?name=cap-app-0").await;
    let victim = listed.ok("cap-app-0")["appSummaries"][0]["appArn"]
        .as_str()
        .expect("appArn")
        .to_string();
    post(&router, "/delete-app", &json!({"appArn": victim}))
        .await
        .ok("delete to free a slot");
    post(&router, "/create-app", &json!({"name": "cap-app-replacement"}))
        .await
        .ok("CreateApp after a delete freed a slot");
    post(&router, "/create-app", &json!({"name": "cap-app-overflow"}))
        .await
        .refused(
            "CreateApp past MAX_APPS again",
            402,
            "ServiceQuotaExceededException",
        );

    // The same bound on `policies`.
    let policies = emulator();
    for index in 0..MAX_RESILIENCY_POLICIES {
        post(
            &policies,
            "/create-resiliency-policy",
            &json!({
                "policyName": format!("cap-policy-{index}"),
                "tier": "Critical",
                "policy": {"AZ": {"rtoInSecs": 1, "rpoInSecs": 1}},
            }),
        )
        .await
        .ok(&format!("policy {index} of {MAX_RESILIENCY_POLICIES}"));
    }
    post(
        &policies,
        "/create-resiliency-policy",
        &json!({
            "policyName": "cap-policy-overflow",
            "tier": "Critical",
            "policy": {"AZ": {"rtoInSecs": 1, "rpoInSecs": 1}},
        }),
    )
    .await
    .refused(
        "CreateResiliencyPolicy past MAX_RESILIENCY_POLICIES",
        402,
        "ServiceQuotaExceededException",
    );
}

/// The idempotency store's own ceiling, exercised directly through the state
/// helper that is its only writer: it never exceeds `MAX_IDEMPOTENCY_TOKENS`, it
/// evicts the oldest entry first, a re-recorded token keeps its position, and
/// forgetting a resource's tokens releases them.
#[test]
fn the_idempotency_store_evicts_its_oldest_entry_at_the_ceiling() {
    let mut state = ComAmazonawsResiliencehubAwsresiliencehubState::default();

    for index in 0..MAX_IDEMPOTENCY_TOKENS {
        state.record_idempotency_token("CreateApp", &format!("token-{index}"), &format!("arn-{index}"));
    }
    assert_eq!(
        state.remembered_token_count(),
        MAX_IDEMPOTENCY_TOKENS,
        "case \"filled to the ceiling\": expected {MAX_IDEMPOTENCY_TOKENS} tokens, got {}",
        state.remembered_token_count()
    );

    // Re-recording an existing token must not evict anything.
    state.record_idempotency_token("CreateApp", "token-0", "arn-0-again");
    assert_eq!(
        state.remembered_token_count(),
        MAX_IDEMPOTENCY_TOKENS,
        "case \"re-recorded an existing token\": the count must not change, got {}",
        state.remembered_token_count()
    );
    assert_eq!(
        state.created_by_token("CreateApp", "token-0").map(String::as_str),
        Some("arn-0-again"),
        "case \"re-recorded an existing token\": the ARN must be replaced, got {:?}",
        state.created_by_token("CreateApp", "token-0")
    );

    // One past the ceiling evicts the OLDEST recorded token, which is still token-0
    // (re-recording kept its position).
    state.record_idempotency_token("CreateApp", "token-new", "arn-new");
    assert_eq!(
        state.remembered_token_count(),
        MAX_IDEMPOTENCY_TOKENS,
        "case \"one past the ceiling\": the store must stay bounded, got {}",
        state.remembered_token_count()
    );
    assert!(
        state.created_by_token("CreateApp", "token-0").is_none(),
        "case \"one past the ceiling\": the oldest token must be evicted, got {:?}",
        state.created_by_token("CreateApp", "token-0")
    );
    assert_eq!(
        state
            .created_by_token("CreateApp", "token-new")
            .map(String::as_str),
        Some("arn-new"),
        "case \"one past the ceiling\": the newest token must be remembered, got {:?}",
        state.created_by_token("CreateApp", "token-new")
    );

    // A different operation with the same token text is a different entry.
    state.record_idempotency_token("CreateResiliencyPolicy", "token-new", "policy-arn");
    assert_eq!(
        state
            .created_by_token("CreateApp", "token-new")
            .map(String::as_str),
        Some("arn-new"),
        "case \"same token, two operations\": the entries must not alias, got {:?}",
        state.created_by_token("CreateApp", "token-new")
    );

    // Forgetting a resource's tokens releases them.
    state.forget_idempotency_tokens_for("arn-new");
    assert!(
        state.created_by_token("CreateApp", "token-new").is_none(),
        "case \"forgot a resource's tokens\": expected the entry gone, got {:?}",
        state.created_by_token("CreateApp", "token-new")
    );
    assert_eq!(
        state
            .created_by_token("CreateResiliencyPolicy", "token-new")
            .map(String::as_str),
        Some("policy-arn"),
        "case \"forgot a resource's tokens\": another resource's token must survive, got {:?}",
        state.created_by_token("CreateResiliencyPolicy", "token-new")
    );
}
