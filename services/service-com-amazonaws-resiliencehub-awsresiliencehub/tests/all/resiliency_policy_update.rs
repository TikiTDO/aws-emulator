// SPDX-License-Identifier: CC0-1.0

//! `UpdateResiliencyPolicy`, end to end over HTTP.
//!
//! The rule under test: an update applies every member the request carries onto the
//! stored resiliency policy, replacing that member's value in full, and leaves every
//! member the request omits exactly as it was — so the body the update answers and the
//! body a later `DescribeResiliencyPolicy` answers are the same record, and the members
//! the service derives from the policy's own content (`estimatedCostTier`) follow what
//! is now stored rather than what used to be.
//!
//! The harness (`emulator`, `post`, `Reply`) is the one `application_lifecycle` drives
//! every request of this crate's suites through: the real wire path,
//! `MultiServiceRouter::dispatch` with a restJson1 request.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::application_lifecycle::{emulator, post, Reply, ACCOUNT, REGION};
use smithy_server_runtime::MultiServiceRouter;

/// The mutable members of a stored resiliency policy — the ones
/// `UpdateResiliencyPolicy` can write.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PolicyShape {
    policy_name: String,
    policy_description: Option<String>,
    data_location_constraint: String,
    tier: String,
    /// `DisruptionType` -> (`rtoInSecs`, `rpoInSecs`).
    policy: BTreeMap<String, (i64, i64)>,
}

impl PolicyShape {
    /// The shape the rule says an update carrying `request` leaves behind: each member
    /// the request names replaces the stored one in full, the rest are untouched.
    fn after(&self, request: &Value) -> Self {
        let mut next = self.clone();
        if let Some(name) = request.get("policyName") {
            next.policy_name = as_text(name);
        }
        if let Some(description) = request.get("policyDescription") {
            next.policy_description = Some(as_text(description));
        }
        if let Some(constraint) = request.get("dataLocationConstraint") {
            next.data_location_constraint = as_text(constraint);
        }
        if let Some(tier) = request.get("tier") {
            next.tier = as_text(tier);
        }
        if let Some(policy) = request.get("policy") {
            next.policy = disruption_map(policy);
        }
        next
    }

    /// The shape a response's `policy` body reports.
    fn observed(policy: &Value) -> Self {
        Self {
            policy_name: as_text(&policy["policyName"]),
            policy_description: policy["policyDescription"].as_str().map(str::to_string),
            data_location_constraint: as_text(&policy["dataLocationConstraint"]),
            tier: as_text(&policy["tier"]),
            policy: disruption_map(&policy["policy"]),
        }
    }
}

fn as_text(value: &Value) -> String {
    value
        .as_str()
        .unwrap_or_else(|| panic!("expected a string member, got {value:?}"))
        .to_string()
}

fn disruption_map(value: &Value) -> BTreeMap<String, (i64, i64)> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("expected a DisruptionPolicy map, got {value:?}"))
        .iter()
        .map(|(disruption, failure)| {
            let rto = failure["rtoInSecs"]
                .as_i64()
                .unwrap_or_else(|| panic!("expected rtoInSecs, got {failure:?}"));
            let rpo = failure["rpoInSecs"]
                .as_i64()
                .unwrap_or_else(|| panic!("expected rpoInSecs, got {failure:?}"));
            (disruption.clone(), (rto, rpo))
        })
        .collect()
}

/// The policy every row starts from: every optional member stated, so a row that omits
/// a member can tell "left alone" from "reset to a default".
fn baseline_request(name: &str) -> Value {
    json!({
        "policyName": name,
        "policyDescription": "the policy as it was created",
        "dataLocationConstraint": "SameCountry",
        "tier": "NonCritical",
        "tags": {"env": "update-suite"},
        "policy": {
            "Software": {"rtoInSecs": 600, "rpoInSecs": 300},
            "Hardware": {"rtoInSecs": 1200, "rpoInSecs": 600},
            "AZ": {"rtoInSecs": 1800, "rpoInSecs": 900},
        },
    })
}

/// Create the baseline policy and answer its whole body.
async fn create_baseline(router: &MultiServiceRouter, name: &str) -> Value {
    let created = post(router, "/create-resiliency-policy", &baseline_request(name)).await;
    created.ok("the baseline policy")["policy"].clone()
}

async fn update_policy(router: &MultiServiceRouter, request: &Value) -> Reply {
    post(router, "/update-resiliency-policy", request).await
}

async fn describe_policy(router: &MultiServiceRouter, policy_arn: &str) -> Reply {
    post(
        router,
        "/describe-resiliency-policy",
        &json!({"policyArn": policy_arn}),
    )
    .await
}

/// The `estimatedCostTier` a disruption set earns, read off the policy the CREATE path
/// mints for that same set: the member is derived from the policy's current content, so
/// an updated policy and a freshly created one holding the same set must report the same
/// tier. Taking it from the create rather than restating a ladder here is what makes
/// this an independent expectation.
async fn cost_tier_of_a_freshly_created_policy(
    router: &MultiServiceRouter,
    name: &str,
    policy: &BTreeMap<String, (i64, i64)>,
) -> String {
    let mut disruption_policy = serde_json::Map::new();
    for (disruption, (rto, rpo)) in policy {
        disruption_policy.insert(
            disruption.clone(),
            json!({"rtoInSecs": rto, "rpoInSecs": rpo}),
        );
    }
    let created = post(
        router,
        "/create-resiliency-policy",
        &json!({
            "policyName": name,
            "tier": "NonCritical",
            "policy": Value::Object(disruption_policy),
        }),
    )
    .await;
    as_text(&created.ok("the reference policy")["policy"]["estimatedCostTier"])
}

/// An `EntityDescription` of `length` characters (the member is `@length(min: 0,
/// max: 500)`).
fn description_of_length(length: usize) -> String {
    "d".repeat(length)
}

/// The rows differ along what the rule generalises over: WHICH members the request
/// carries (none, one, several, all), and for the `DisruptionPolicy` map, whether the
/// supplied entry set is the same scopes with new objectives, a narrower set, a wider
/// set, or objectives at the bottom of `Seconds`' modelled range (`@range(min: 0)`).
/// `policyDescription` is driven across its modelled length range (0, mid, 500).
fn update_table(policy_arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "nothing but the required policyArn",
            json!({"policyArn": policy_arn}),
        ),
        (
            "the tier alone",
            json!({"policyArn": policy_arn, "tier": "Critical"}),
        ),
        (
            "the name alone",
            json!({"policyArn": policy_arn, "policyName": "renamed-policy"}),
        ),
        (
            "the description alone",
            json!({"policyArn": policy_arn, "policyDescription": "retiered, tightened"}),
        ),
        (
            "an empty description, the bottom of the modelled length range",
            json!({"policyArn": policy_arn, "policyDescription": ""}),
        ),
        (
            "a description at the modelled length ceiling",
            json!({"policyArn": policy_arn, "policyDescription": description_of_length(500)}),
        ),
        (
            "the data location constraint alone",
            json!({"policyArn": policy_arn, "dataLocationConstraint": "AnyLocation"}),
        ),
        (
            "the same disruption scopes with tightened objectives",
            json!({
                "policyArn": policy_arn,
                "policy": {
                    "Software": {"rtoInSecs": 60, "rpoInSecs": 30},
                    "Hardware": {"rtoInSecs": 120, "rpoInSecs": 60},
                    "AZ": {"rtoInSecs": 180, "rpoInSecs": 90},
                },
            }),
        ),
        (
            "a narrower disruption set, the multi-Region objective dropped",
            json!({
                "policyArn": policy_arn,
                "policy": {
                    "Software": {"rtoInSecs": 45, "rpoInSecs": 15},
                    "Hardware": {"rtoInSecs": 90, "rpoInSecs": 30},
                    "AZ": {"rtoInSecs": 135, "rpoInSecs": 45},
                },
            }),
        ),
        (
            "a wider disruption set, every scope the model lists",
            json!({
                "policyArn": policy_arn,
                "policy": {
                    "Software": {"rtoInSecs": 30, "rpoInSecs": 15},
                    "Hardware": {"rtoInSecs": 60, "rpoInSecs": 30},
                    "AZ": {"rtoInSecs": 90, "rpoInSecs": 45},
                    "Region": {"rtoInSecs": 7200, "rpoInSecs": 3600},
                },
            }),
        ),
        (
            "objectives of zero, the bottom of the modelled range",
            json!({
                "policyArn": policy_arn,
                "policy": {
                    "Software": {"rtoInSecs": 0, "rpoInSecs": 0},
                    "Hardware": {"rtoInSecs": 0, "rpoInSecs": 0},
                    "AZ": {"rtoInSecs": 0, "rpoInSecs": 0},
                    "Region": {"rtoInSecs": 0, "rpoInSecs": 0},
                },
            }),
        ),
        (
            "every writable member at once",
            json!({
                "policyArn": policy_arn,
                "policyName": "fully-updated-policy",
                "policyDescription": description_of_length(120),
                "dataLocationConstraint": "SameContinent",
                "tier": "MissionCritical",
                "policy": {
                    "Software": {"rtoInSecs": 60, "rpoInSecs": 30},
                    "Hardware": {"rtoInSecs": 120, "rpoInSecs": 60},
                    "AZ": {"rtoInSecs": 180, "rpoInSecs": 90},
                },
            }),
        ),
    ]
}

/// Each supplied member replaces the stored one, each omitted member is left alone, and
/// the update's own response and a later read report the same record.
#[tokio::test]
async fn an_update_replaces_the_members_it_carries_and_leaves_the_rest() {
    // Every row runs against its OWN freshly created policy, so each row reads the
    // creation-time record rather than whatever the previous row left behind. A row
    // names the policy it updates, so the table is rebuilt once that ARN is known; the
    // placeholder build only counts the rows.
    let row_count = update_table("placeholder").len();
    for index in 0..row_count {
        let router = emulator();
        let baseline = create_baseline(&router, "baseline-policy").await;
        let policy_arn = as_text(&baseline["policyArn"]);
        let before = PolicyShape::observed(&baseline);

        let (case, request) = update_table(&policy_arn)
            .into_iter()
            .nth(index)
            .expect("the row under test");
        let expected = before.after(&request);

        let updated = update_policy(&router, &request).await;
        let updated_policy = updated.ok(case)["policy"].clone();
        assert_eq!(
            PolicyShape::observed(&updated_policy),
            expected,
            "case {case:?}: UpdateResiliencyPolicy must answer the updated policy, got {:?}",
            updated_policy
        );

        // The write is what a later READ sees — the defect this suite pins.
        let described = describe_policy(&router, &policy_arn).await;
        let described_policy = described.ok(case)["policy"].clone();
        assert_eq!(
            PolicyShape::observed(&described_policy),
            expected,
            "case {case:?}: DescribeResiliencyPolicy must report the updated policy, got {:?}",
            described_policy
        );

        // Identity and the members an update cannot write are carried over unchanged.
        for member in ["policyArn", "creationTime", "tags"] {
            assert_eq!(
                described_policy[member], baseline[member],
                "case {case:?}: {member} must be unchanged by an update, got {:?} (was {:?})",
                described_policy[member], baseline[member]
            );
        }

        // The derived member follows the disruption set now stored, not the one the
        // update replaced.
        let reference =
            cost_tier_of_a_freshly_created_policy(&router, "cost-tier-reference", &expected.policy)
                .await;
        for (source, body) in [
            ("the update", &updated_policy),
            ("the read", &described_policy),
        ] {
            assert_eq!(
                as_text(&body["estimatedCostTier"]),
                reference,
                "case {case:?}: {source} must report the cost tier the stored disruption \
                 set earns ({reference}), got {:?}",
                body["estimatedCostTier"]
            );
        }
    }
}

/// Updates accumulate: the record one update leaves behind is what the next one updates,
/// so a member set two updates ago survives an update that does not name it.
#[tokio::test]
async fn consecutive_updates_accumulate_on_the_stored_policy() {
    let router = emulator();
    let baseline = create_baseline(&router, "accumulating-policy").await;
    let policy_arn = as_text(&baseline["policyArn"]);
    let mut expected = PolicyShape::observed(&baseline);

    for (case, request) in update_table(&policy_arn) {
        expected = expected.after(&request);
        let updated = update_policy(&router, &request).await;
        assert_eq!(
            PolicyShape::observed(&updated.ok(case)["policy"]),
            expected,
            "case {case:?}: the update must build on the previous one, got {:?}",
            updated.body
        );
        let described = describe_policy(&router, &policy_arn).await;
        assert_eq!(
            PolicyShape::observed(&described.ok(case)["policy"]),
            expected,
            "case {case:?}: the read must agree with the accumulated record, got {:?}",
            described.body
        );
    }
}

/// An update names one policy: every other policy in the store is untouched by it.
#[tokio::test]
async fn an_update_touches_only_the_policy_it_names() {
    let router = emulator();
    let target = create_baseline(&router, "target-policy").await;
    let bystander = create_baseline(&router, "bystander-policy").await;
    let target_arn = as_text(&target["policyArn"]);
    let bystander_arn = as_text(&bystander["policyArn"]);

    update_policy(
        &router,
        &json!({
            "policyArn": target_arn,
            "policyName": "target-policy-renamed",
            "tier": "Critical",
            "policy": {
                "Software": {"rtoInSecs": 10, "rpoInSecs": 5},
                "Hardware": {"rtoInSecs": 10, "rpoInSecs": 5},
                "AZ": {"rtoInSecs": 10, "rpoInSecs": 5},
                "Region": {"rtoInSecs": 10, "rpoInSecs": 5},
            },
        }),
    )
    .await
    .ok("update the target");

    let read_back = describe_policy(&router, &bystander_arn).await;
    assert_eq!(
        PolicyShape::observed(&read_back.ok("the bystander")["policy"]),
        PolicyShape::observed(&bystander),
        "case \"the bystander\": updating one policy must leave the others alone, got {:?}",
        read_back.body
    );
}

/// A `policyArn` no policy stands behind is the modelled 404, whatever its shape, and
/// persists nothing. The rows differ along the shape of a well-formed-but-unknown `Arn`
/// (the member's @pattern bounds them: partition, region, account, resource id).
#[tokio::test]
async fn updating_a_policy_that_does_not_exist_is_a_resource_not_found() {
    let router = emulator();
    let table: Vec<(&str, String)> = vec![
        (
            "never created, in this region",
            format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:resiliency-policy/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"),
        ),
        (
            "never created, another region",
            format!("arn:aws:resiliencehub:eu-west-3:{ACCOUNT}:resiliency-policy/11111111-2222-3333-4444-555555555555"),
        ),
        (
            "never created, another partition",
            format!("arn:aws-us-gov:resiliencehub:us-gov-west-1:{ACCOUNT}:resiliency-policy/99999999-8888-7777-6666-555555555555"),
        ),
        (
            "never created, another account",
            "arn:aws:resiliencehub:us-east-1:123456789012:resiliency-policy/deadbeef-dead-beef-dead-beefdeadbeef".to_string(),
        ),
        (
            "an app ARN, not a policy ARN",
            format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:app/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"),
        ),
    ];

    for (case, policy_arn) in &table {
        let refused =
            update_policy(&router, &json!({"policyArn": policy_arn, "tier": "Critical"})).await;
        refused.refused(case, 404, "ResourceNotFoundException");
        assert_eq!(
            refused.body["message"].as_str(),
            Some(format!("ResiliencyPolicy {policy_arn} not found").as_str()),
            "case {case:?}: the 404 must name the policy the request named, got {:?}",
            refused.body
        );
        assert_eq!(
            refused.body["resourceId"].as_str(),
            Some(policy_arn.as_str()),
            "case {case:?}: the 404 must carry the policyArn as resourceId, got {:?}",
            refused.body
        );
    }

    // A policy that was deleted is in the same class as one never created.
    let deletable = create_baseline(&router, "deleted-policy").await;
    let deleted_arn = as_text(&deletable["policyArn"]);
    post(
        &router,
        "/delete-resiliency-policy",
        &json!({"policyArn": deleted_arn}),
    )
    .await
    .ok("delete the policy");
    update_policy(
        &router,
        &json!({"policyArn": deleted_arn, "tier": "Critical"}),
    )
    .await
    .refused("a deleted policy", 404, "ResourceNotFoundException");
}

/// A refused update changes nothing: the stored policy stays the one the create left.
///
/// The rows are the ways a `DisruptionPolicy` the request supplies is not one this
/// service will store — a key outside `DisruptionType` (whose values the model declares
/// as Software, Hardware, AZ, Region; `smithy_query query="shape"
/// shape="DisruptionType"`) names no failure type, and a set that leaves out one of the
/// failure types every policy has to state (the application, infrastructure and
/// Availability Zone objectives) has no failure policy for it. A map's KEY is not walked
/// by the boundary validator, so these reach the handler.
#[tokio::test]
async fn an_update_this_service_refuses_persists_nothing() {
    let table: Vec<(&str, Value)> = vec![
        ("an empty disruption set", json!({})),
        (
            "a key outside DisruptionType",
            json!({"Datacenter": {"rtoInSecs": 60, "rpoInSecs": 30}}),
        ),
        (
            "a key that differs from a modelled one only in case",
            json!({"software": {"rtoInSecs": 60, "rpoInSecs": 30}}),
        ),
        (
            "one modelled key beside one that is not",
            json!({
                "Software": {"rtoInSecs": 60, "rpoInSecs": 30},
                "Continent": {"rtoInSecs": 60, "rpoInSecs": 30},
            }),
        ),
        (
            "a required failure type left out",
            json!({
                "Software": {"rtoInSecs": 60, "rpoInSecs": 30},
                "AZ": {"rtoInSecs": 60, "rpoInSecs": 30},
            }),
        ),
        (
            "only the multi-Region objective",
            json!({"Region": {"rtoInSecs": 60, "rpoInSecs": 30}}),
        ),
    ];

    for (case, disruption_policy) in &table {
        let router = emulator();
        let baseline = create_baseline(&router, "refusal-policy").await;
        let policy_arn = as_text(&baseline["policyArn"]);

        update_policy(
            &router,
            &json!({
                "policyArn": policy_arn,
                "tier": "Critical",
                "policy": disruption_policy,
            }),
        )
        .await
        .refused(case, 400, "ValidationException");

        let described = describe_policy(&router, &policy_arn).await;
        assert_eq!(
            PolicyShape::observed(&described.ok(case)["policy"]),
            PolicyShape::observed(&baseline),
            "case {case:?}: a refused update must leave the policy as it was, got {:?}",
            described.body
        );
    }
}

/// A member outside its modelled constraint is refused before any state is written. The
/// rows name one constrained member each: `tier` and `dataLocationConstraint` (enums),
/// `policyName` (@pattern), `policyDescription` (@length max 500) and `rtoInSecs`
/// (@range min 0).
#[tokio::test]
async fn an_update_outside_the_modelled_constraints_is_refused() {
    let router = emulator();
    let baseline = create_baseline(&router, "constrained-policy").await;
    let policy_arn = as_text(&baseline["policyArn"]);

    let table: Vec<(&str, Value)> = vec![
        (
            "a tier outside ResiliencyPolicyTier",
            json!({"policyArn": policy_arn, "tier": "Urgent"}),
        ),
        (
            "a constraint outside DataLocationConstraint",
            json!({"policyArn": policy_arn, "dataLocationConstraint": "SamePlanet"}),
        ),
        (
            "a policyName the EntityName pattern refuses",
            json!({"policyArn": policy_arn, "policyName": "a name with spaces"}),
        ),
        (
            "a description one character past the modelled ceiling",
            json!({"policyArn": policy_arn, "policyDescription": description_of_length(501)}),
        ),
        (
            "an rtoInSecs below the modelled range",
            json!({
                "policyArn": policy_arn,
                "policy": {"AZ": {"rtoInSecs": -1, "rpoInSecs": 0}},
            }),
        ),
    ];

    for (case, request) in &table {
        update_policy(&router, request)
            .await
            .refused(case, 400, "ValidationException");
        let described = describe_policy(&router, &policy_arn).await;
        assert_eq!(
            PolicyShape::observed(&described.ok(case)["policy"]),
            PolicyShape::observed(&baseline),
            "case {case:?}: a refused update must leave the policy as it was, got {:?}",
            described.body
        );
    }
}
