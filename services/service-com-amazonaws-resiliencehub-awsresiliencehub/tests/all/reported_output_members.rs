// SPDX-License-Identifier: CC0-1.0

//! The members a success body always carries, end to end over HTTP.
//!
//! The rule under test: a member the service itself answers is reported on EVERY
//! success body that carries its shape, resolved from what is stored, never dropped
//! because the request left it out —
//!
//! * `App$tags` and `ResiliencyPolicy$tags`: the resource's tag map, EMPTY for a
//!   resource nobody tagged, so a client reads it without a null guard;
//! * `ResiliencyPolicy$dataLocationConstraint`: the constraint the policy holds, the
//!   one that constrains nothing when the caller stated none;
//! * `ResiliencyPolicy$estimatedCostTier`: computed by the service from the policy's
//!   content;
//! * `AppVersionSummary$identifier`: the ordering key of every listed version, the
//!   draft's below every published version's.
//!
//! The harness (`emulator`, `post`, `Reply`) is the one `application_lifecycle`
//! drives every request of this crate's suites through: the real wire path,
//! `MultiServiceRouter::dispatch` with a restJson1 request.

use serde_json::{json, Map, Value};

use super::application_lifecycle::{emulator, post, Reply};
use service_com_amazonaws_resiliencehub_awsresiliencehub::ops::types::{
    DATA_LOCATION_CONSTRAINT_VALUES, ESTIMATED_COST_TIER_VALUES,
};
use service_com_amazonaws_resiliencehub_awsresiliencehub::state::{
    DEFAULT_DATA_LOCATION_CONSTRAINT, DRAFT_APP_VERSION, DRAFT_APP_VERSION_IDENTIFIER,
};
use smithy_server_runtime::MultiServiceRouter;

/// A tag map of `count` entries, each key distinct. The ceiling a row uses comes
/// from `TagMap`'s modelled `@length(min: 1, max: 50)` (`smithy_query query="shape"
/// shape="TagMap"`).
fn tag_map(count: usize) -> Value {
    let mut tags = Map::new();
    for index in 0..count {
        tags.insert(format!("key-{index}"), json!(format!("value-{index}")));
    }
    Value::Object(tags)
}

/// One tag at the top of `TagKey`'s 1..128 and `TagValue`'s 0..256 modelled
/// lengths (`smithy_query query="shape" shape="TagMap"`).
fn longest_tag() -> Value {
    let mut tags = Map::new();
    tags.insert("k".repeat(128), json!("v".repeat(256)));
    Value::Object(tags)
}

/// A row of the tag table: a `CreateApp` body's `tags` member, and the map every
/// later read of that application must report.
struct TagCase {
    label: &'static str,
    /// The `tags` the create request carries, or `None` when it carries none.
    requested: Option<Value>,
}

/// `tags` is reported by every operation whose body carries the application — on the
/// create, on the read and on the update — and a request that named no tags still
/// gets a map.
///
/// The rows differ along what the rule generalises over: how many tags the request
/// carries (none, one, several, the modelled maximum) and what one tag holds (the
/// bounds of `TagKey`'s 1..128 and `TagValue`'s 0..256 lengths).
#[tokio::test]
async fn app_tags_are_reported_on_every_body_that_carries_the_application() {
    let router = emulator();

    let table = [
        TagCase {
            label: "no tags member at all",
            requested: None,
        },
        TagCase {
            label: "one tag",
            requested: Some(json!({"env": "test"})),
        },
        TagCase {
            label: "three tags",
            requested: Some(json!({"env": "test", "team": "resilience", "cost": "shared"})),
        },
        TagCase {
            label: "a tag whose value is the empty string",
            requested: Some(json!({"owner": ""})),
        },
        TagCase {
            label: "a key and a value at the top of their modelled lengths",
            requested: Some(longest_tag()),
        },
        TagCase {
            label: "the modelled maximum of fifty tags",
            requested: Some(tag_map(50)),
        },
    ];

    for (index, row) in table.iter().enumerate() {
        let case = row.label;
        let expected = row.requested.clone().unwrap_or_else(|| json!({}));

        let mut create_body = json!({"name": format!("tags-app-{index}")});
        if let Some(requested) = &row.requested {
            create_body["tags"] = requested.clone();
        }

        let created = post(&router, "/create-app", &create_body).await;
        let created_app = created.ok(case)["app"].clone();
        assert_eq!(
            created_app["tags"], expected,
            "case {case:?}: CreateApp must report tags {expected:?}, got {:?}",
            created_app["tags"]
        );
        let app_arn = created_app["appArn"]
            .as_str()
            .expect("appArn is @required")
            .to_string();

        let described = post(&router, "/describe-app", &json!({"appArn": &app_arn})).await;
        assert_eq!(
            described.ok(case)["app"]["tags"], expected,
            "case {case:?}: DescribeApp must report tags {expected:?}, got {:?}",
            described.body["app"]["tags"]
        );

        let updated = post(
            &router,
            "/update-app",
            &json!({"appArn": &app_arn, "description": "updated"}),
        )
        .await;
        assert_eq!(
            updated.ok(case)["app"]["tags"], expected,
            "case {case:?}: UpdateApp must report the tags the application holds \
             ({expected:?}), got {:?}",
            updated.body["app"]["tags"]
        );
    }
}

/// A replayed `clientToken` still replays the first create rather than conflicting,
/// although the stored application carries the empty map the request never sent.
#[tokio::test]
async fn an_untagged_create_is_still_idempotent_on_a_replayed_client_token() {
    let router = emulator();
    let body = json!({"name": "tags-idempotent", "clientToken": "token-1"});

    let first = post(&router, "/create-app", &body).await;
    let first_app = first.ok("first call")["app"].clone();
    assert_eq!(
        first_app["tags"],
        json!({}),
        "case \"first call\": expected an empty tag map, got {:?}",
        first_app["tags"]
    );

    let replay = post(&router, "/create-app", &body).await;
    assert_eq!(
        replay.ok("replayed token")["app"], first_app,
        "case \"replayed token\": a replay must answer the first application, got {:?}",
        replay.body["app"]
    );
}

/// A row of the policy table: the `CreateResiliencyPolicy` body's optional members,
/// and the cost tier the policy's content earns.
struct PolicyCase {
    label: &'static str,
    /// The `DisruptionPolicy` map the request carries.
    policy: fn() -> Value,
    /// The `dataLocationConstraint` the request carries, or `None`.
    constraint: Option<&'static str>,
    /// The `tags` the request carries, or `None`.
    tags: Option<fn() -> Value>,
    /// The `EstimatedCostTier` the policy's broadest disruption scope earns.
    expected_cost_tier: &'static str,
}

/// `dataLocationConstraint`, `estimatedCostTier` and `tags` are reported by both the
/// create and the read, resolved from the policy rather than echoed: an unstated
/// constraint resolves to the one that constrains nothing, the cost tier follows the
/// broadest disruption scope the policy states an objective for, and an untagged
/// policy carries an empty map.
///
/// Catalogue sources: `DisruptionType`'s values in the order the model declares them
/// (`smithy_query query="shape" shape="DisruptionType"`: Software, Hardware, AZ,
/// Region — ascending in the redundancy recovering from that scope needs) and
/// `EstimatedCostTier`'s (L1..L4, ascending in cost). Every policy states the three
/// failure types a policy has to carry, so the rows differ along the rest of what the
/// rule generalises over: whether the multi-Region objective is stated as well,
/// whether the optional members are present, and how tight the objectives are.
#[tokio::test]
async fn a_policy_reports_its_constraint_cost_tier_and_tags() {
    let router = emulator();

    let table = [
        PolicyCase {
            label: "the required objectives, nothing optional",
            policy: || {
                json!({
                    "Software": {"rtoInSecs": 86400, "rpoInSecs": 86400},
                    "Hardware": {"rtoInSecs": 86400, "rpoInSecs": 86400},
                    "AZ": {"rtoInSecs": 86400, "rpoInSecs": 86400},
                })
            },
            constraint: None,
            tags: None,
            expected_cost_tier: "L3",
        },
        PolicyCase {
            label: "the required objectives, with a constraint and tags",
            policy: || {
                json!({
                    "Software": {"rtoInSecs": 600, "rpoInSecs": 300},
                    "Hardware": {"rtoInSecs": 1200, "rpoInSecs": 600},
                    "AZ": {"rtoInSecs": 1800, "rpoInSecs": 900},
                })
            },
            constraint: Some("SameCountry"),
            tags: Some(|| json!({"env": "test"})),
            expected_cost_tier: "L3",
        },
        PolicyCase {
            label: "every disruption type the model lists",
            policy: || {
                json!({
                    "Software": {"rtoInSecs": 60, "rpoInSecs": 30},
                    "Hardware": {"rtoInSecs": 120, "rpoInSecs": 60},
                    "AZ": {"rtoInSecs": 180, "rpoInSecs": 90},
                    "Region": {"rtoInSecs": 3600, "rpoInSecs": 1800},
                })
            },
            constraint: Some("SameContinent"),
            tags: Some(|| tag_map(3)),
            expected_cost_tier: "L4",
        },
        PolicyCase {
            label: "every disruption type at the bottom of the modelled range",
            policy: || {
                json!({
                    "Software": {"rtoInSecs": 0, "rpoInSecs": 0},
                    "Hardware": {"rtoInSecs": 0, "rpoInSecs": 0},
                    "AZ": {"rtoInSecs": 0, "rpoInSecs": 0},
                    "Region": {"rtoInSecs": 0, "rpoInSecs": 0},
                })
            },
            constraint: None,
            tags: Some(|| json!({"only": "tag"})),
            expected_cost_tier: "L4",
        },
    ];

    for (index, row) in table.iter().enumerate() {
        let case = row.label;
        let expected_tags = row.tags.map_or_else(|| json!({}), |tags| tags());
        let expected_constraint = row.constraint.unwrap_or(DEFAULT_DATA_LOCATION_CONSTRAINT);

        let mut request = json!({
            "policyName": format!("members-policy-{index}"),
            "tier": "NonCritical",
            "policy": (row.policy)(),
        });
        if let Some(constraint) = row.constraint {
            request["dataLocationConstraint"] = json!(constraint);
        }
        if let Some(tags) = row.tags {
            request["tags"] = tags();
        }

        let created = post(&router, "/create-resiliency-policy", &request).await;
        let created_policy = created.ok(case)["policy"].clone();
        assert_eq!(
            created_policy["dataLocationConstraint"],
            json!(expected_constraint),
            "case {case:?}: expected dataLocationConstraint {expected_constraint:?}, got {:?}",
            created_policy["dataLocationConstraint"]
        );
        assert_eq!(
            created_policy["estimatedCostTier"],
            json!(row.expected_cost_tier),
            "case {case:?}: expected estimatedCostTier {:?}, got {:?}",
            row.expected_cost_tier,
            created_policy["estimatedCostTier"]
        );
        assert_eq!(
            created_policy["tags"], expected_tags,
            "case {case:?}: expected tags {expected_tags:?}, got {:?}",
            created_policy["tags"]
        );
        // Both resolved members are values the model admits.
        assert!(
            DATA_LOCATION_CONSTRAINT_VALUES.contains(&expected_constraint),
            "case {case:?}: {expected_constraint:?} is outside DataLocationConstraint"
        );
        assert!(
            ESTIMATED_COST_TIER_VALUES.contains(&row.expected_cost_tier),
            "case {case:?}: {:?} is outside EstimatedCostTier",
            row.expected_cost_tier
        );

        let policy_arn = created_policy["policyArn"]
            .as_str()
            .expect("a minted policyArn")
            .to_string();
        let described = post(
            &router,
            "/describe-resiliency-policy",
            &json!({"policyArn": policy_arn}),
        )
        .await;
        assert_eq!(
            described.ok(case)["policy"], created_policy,
            "case {case:?}: DescribeResiliencyPolicy must report what was persisted, got {:?}",
            described.body["policy"]
        );
    }
}

/// The cost tier follows the policy's broadest disruption scope, not how tight its
/// objectives are: one disruption set answers one tier whether its RTO/RPO values
/// are seconds or a day apart. The two sets a policy can state differ in the one
/// scope that is optional, so they must earn DIFFERENT tiers while each set earns
/// one tier across both magnitudes.
#[tokio::test]
async fn the_cost_tier_follows_the_disruption_scope_not_the_objective_magnitude() {
    let router = emulator();

    // Each row: the disruption types the policy states objectives for, paired with
    // the two magnitudes the same set is submitted at. Every set states the three
    // failure types a policy has to carry (the application, infrastructure and
    // Availability Zone objectives), so the rows differ in whether the multi-Region
    // objective is stated beside them.
    let scope_sets: [&[&str]; 2] = [
        &["Software", "Hardware", "AZ"],
        &["Software", "Hardware", "AZ", "Region"],
    ];
    let magnitudes = [(1, 0), (86400, 43200)];

    // The tier each set earned, so the two sets can be compared at the end.
    let mut earned: Vec<String> = Vec::new();
    for (index, scopes) in scope_sets.iter().enumerate() {
        let mut tiers: Vec<String> = Vec::new();
        for (magnitude_index, (rto, rpo)) in magnitudes.iter().enumerate() {
            let case = format!("{scopes:?} at rto {rto}s / rpo {rpo}s");
            let mut policy = Map::new();
            for scope in scopes.iter() {
                policy.insert(
                    (*scope).to_string(),
                    json!({"rtoInSecs": rto, "rpoInSecs": rpo}),
                );
            }
            let reply = post(
                &router,
                "/create-resiliency-policy",
                &json!({
                    "policyName": format!("magnitude-{index}-{magnitude_index}"),
                    "tier": "MissionCritical",
                    "policy": Value::Object(policy),
                }),
            )
            .await;
            let tier = reply.ok(&case)["policy"]["estimatedCostTier"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: estimatedCostTier must be reported, got {:?}",
                        reply.body["policy"]
                    )
                })
                .to_string();
            assert!(
                ESTIMATED_COST_TIER_VALUES.contains(&tier.as_str()),
                "case {case:?}: {tier:?} is outside EstimatedCostTier"
            );
            tiers.push(tier);
        }
        assert_eq!(
            tiers[0], tiers[1],
            "case {scopes:?}: one disruption set must earn one cost tier whatever the \
             objectives, got {tiers:?}"
        );
        earned.push(tiers.remove(0));
    }
    // Not vacuous: the set that states the optional scope earns a different tier from
    // the one that does not, so the equalities above are not one fixed answer.
    assert_ne!(
        earned[0], earned[1],
        "the broader disruption set must earn a different cost tier, got {earned:?}"
    );
}

/// Add one Application Component to `app_arn`'s draft and map one physical
/// resource into it, so the draft's app template maps the resource a publish
/// requires.
///
/// The resource is bound to the AppCommon Application Component every app version
/// already carries, so this adds no component other than `id`.
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
    let reply = post(
        router,
        "/create-app-version-resource",
        &json!({
            "appArn": app_arn,
            "logicalResourceId": {"identifier": format!("{id}-resource")},
            "physicalResourceId": format!("{id}-queue"),
            "resourceType": "AWS::SQS::Queue",
            "appComponents": ["appcommon"],
        }),
    )
    .await;
    reply.ok(id);
}

/// `ListAppVersions` for `app_arn`.
async fn list_versions(router: &MultiServiceRouter, app_arn: &str) -> Reply {
    post(router, "/list-app-versions", &json!({"appArn": app_arn})).await
}

/// Every listed app version answers `identifier`: the draft's is the base number
/// every published version's publish sequence number sits above, so a client can
/// order the listing by it.
#[tokio::test]
async fn every_listed_app_version_reports_an_identifier() {
    let router = emulator();
    let created = post(&router, "/create-app", &json!({"name": "version-ids"})).await;
    let app_arn = created.ok("the application")["app"]["appArn"]
        .as_str()
        .expect("appArn is @required")
        .to_string();

    // Row by row: the draft alone, then one more published version per publish.
    for published in 0..=3usize {
        let case = format!("{published} published version(s)");
        if published > 0 {
            add_a_component(&router, &app_arn, &format!("component-{published}")).await;
            post(
                &router,
                "/publish-app-version",
                &json!({"appArn": &app_arn}),
            )
            .await
            .ok(&case);
        }

        let listed = list_versions(&router, &app_arn).await;
        let summaries = listed.ok(&case)["appVersions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: appVersions is @required"))
            .clone();
        assert_eq!(
            summaries.len(),
            published + 1,
            "case {case:?}: expected the draft plus {published} published version(s), \
             got {summaries:?}"
        );

        let identifiers: Vec<i64> = summaries
            .iter()
            .map(|summary| {
                summary["identifier"].as_i64().unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: every listed version reports an identifier, got \
                         {summary:?}"
                    )
                })
            })
            .collect();
        let expected: Vec<i64> = (0..=published as i64).collect();
        assert_eq!(
            identifiers, expected,
            "case {case:?}: expected identifiers {expected:?}, got {identifiers:?}"
        );
        assert_eq!(
            summaries[0]["appVersion"],
            json!(DRAFT_APP_VERSION),
            "case {case:?}: the oldest version is the draft, got {:?}",
            summaries[0]
        );
        assert_eq!(
            identifiers[0], DRAFT_APP_VERSION_IDENTIFIER,
            "case {case:?}: the draft's identifier is {DRAFT_APP_VERSION_IDENTIFIER}, got {}",
            identifiers[0]
        );
    }
}
