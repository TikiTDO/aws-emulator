// SPDX-License-Identifier: CC0-1.0

//! The exclusion between the two GuardDuty Runtime Monitoring features, end to end
//! over the wire.
//!
//! Every request goes through `MultiServiceRouter::dispatch` (the harness lives in
//! the `detector_lifecycle` suite), so routing, restJson1 (de)serialization,
//! modelled-constraint validation and the handlers are exercised together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one call:
//!
//! 1. A `Features` list configures only the switches it names, so the exclusion is
//!    judged on the status each of the two features ENDS UP with once the list has
//!    been applied over the configuration that already exists — the list's own value
//!    where it names the feature, the stored value otherwise. A list that would
//!    leave both enabled is refused with HTTP 400 under the modelled
//!    `BadRequestException` shape, and configures nothing. The same rows hold for
//!    the detector's own switches (`UpdateDetector`) and for a member account's
//!    (`UpdateMemberDetectors`), which document the exclusion identically.
//! 2. It is the list's own ADDITION that is refused: a list that enables neither of
//!    the two adds neither, so it is accepted.
//! 3. A create has no stored configuration for its list to be applied over, and only
//!    asking for BOTH is an error: the default set the service picks yields to the
//!    feature the create itself enables, which it then stores with the other one
//!    DISABLED.
//! 4. Whatever a caller sends through these three operations, no read reports both
//!    features `ENABLED`.
//! 5. Rule 3 over the WHOLE create input space rather than a chosen handful: every
//!    way a create's `Features` list can treat the two features — leaving one out,
//!    naming it with no status, naming it `ENABLED`, naming it `DISABLED`, in either
//!    entry order — with the expectation COMPUTED from the rule for each
//!    combination instead of written out per row, and with the deprecated
//!    `DataSources` view (which the create applies over the same stored switches)
//!    present alongside the list.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    call, create_detector, enabled_detector, error_code, error_shape, new_router, router_over,
    Router,
};
use crate::member_association::{create_members, invite_members, MEMBERS};
use crate::member_detector_configuration::{get_member_detectors, update_member_detectors};
use crate::member_relationship_scope::monitor_member;

/// The modelled error SHAPE every GuardDuty client rejection is enveloped in, as
/// `x-amzn-errortype` carries it.
const ENVELOPE: &str = "BadRequestException";

/// The error CLASS the rejection carries.
const CLASS: &str = "InvalidInputException";

/// Runtime Monitoring already covers Amazon EKS, so "you can add only one of these
/// two features" (`smithy_query(query="doc",shape="DetectorFeatureConfiguration")`):
/// the message the service refuses the combination with.
const BOTH_ENABLED_MESSAGE: &str = "Features: EKS_RUNTIME_MONITORING and RUNTIME_MONITORING cannot both be enabled because RUNTIME_MONITORING already includes threat detection for Amazon EKS resources.";

/// The older feature, which covers only the Amazon EKS resources.
const EKS: &str = "EKS_RUNTIME_MONITORING";

/// The feature that covers every runtime resource type, Amazon EKS included.
const RUNTIME: &str = "RUNTIME_MONITORING";

const ENABLED: &str = "ENABLED";
const DISABLED: &str = "DISABLED";

/// One row of the exclusion table: the configuration to put in place first, the list
/// to then send, whether it is refused, and the `(EKS_RUNTIME_MONITORING,
/// RUNTIME_MONITORING)` statuses a read must report afterwards.
struct Row {
    case: &'static str,
    /// Applied first, through the same update operation, and asserted accepted: the
    /// stored configuration the row's own list is judged against. `None` leaves the
    /// configuration a create (or a `CreateMembers`) starts at, where EKS Runtime
    /// Monitoring is enabled and Runtime Monitoring is not.
    setup: Option<Value>,
    request: Value,
    refused: bool,
    expected: (&'static str, &'static str),
}

/// The rows, differing along WHICH of the two the list enables, which one the stored
/// configuration already holds enabled, whether the list states a status at all,
/// whether it names the other feature beside it, the ORDER of its entries, how many
/// entries it carries and whether it touches the pair at all.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "enabling Runtime Monitoring while the stored configuration still has EKS \
                   Runtime Monitoring",
            setup: None,
            request: json!([{"name": RUNTIME, "status": ENABLED}]),
            refused: true,
            expected: (ENABLED, DISABLED),
        },
        Row {
            case: "the same entry carrying no status at all, which enables the feature",
            setup: None,
            request: json!([{"name": RUNTIME}]),
            refused: true,
            expected: (ENABLED, DISABLED),
        },
        Row {
            case: "both features enabled by one list",
            setup: None,
            request: json!([{"name": EKS, "status": ENABLED}, {"name": RUNTIME, "status": ENABLED}]),
            refused: true,
            expected: (ENABLED, DISABLED),
        },
        Row {
            case: "the one it replaces disabled in the same call",
            setup: None,
            request: json!([{"name": EKS, "status": DISABLED}, {"name": RUNTIME, "status": ENABLED}]),
            refused: false,
            expected: (DISABLED, ENABLED),
        },
        Row {
            case: "the same two entries in the other order",
            setup: None,
            request: json!([{"name": RUNTIME, "status": ENABLED}, {"name": EKS, "status": DISABLED}]),
            refused: false,
            expected: (DISABLED, ENABLED),
        },
        Row {
            case: "enabling Runtime Monitoring once an earlier call switched EKS Runtime \
                   Monitoring off",
            setup: Some(json!([{"name": EKS, "status": DISABLED}])),
            request: json!([{"name": RUNTIME, "status": ENABLED}]),
            refused: false,
            expected: (DISABLED, ENABLED),
        },
        Row {
            case: "the other direction: enabling EKS Runtime Monitoring while Runtime \
                   Monitoring is stored enabled",
            setup: Some(json!([{"name": EKS, "status": DISABLED}, {"name": RUNTIME, "status": ENABLED}])),
            request: json!([{"name": EKS, "status": ENABLED}]),
            refused: true,
            expected: (DISABLED, ENABLED),
        },
        Row {
            case: "re-enabling the one the stored configuration already has enabled",
            setup: None,
            request: json!([{"name": EKS, "status": ENABLED}]),
            refused: false,
            expected: (ENABLED, DISABLED),
        },
        Row {
            case: "a list naming one feature twice: the last entry naming it is the one that \
                   lands, so it enables nothing",
            setup: None,
            request: json!([{"name": RUNTIME, "status": ENABLED}, {"name": RUNTIME, "status": DISABLED}]),
            refused: false,
            expected: (ENABLED, DISABLED),
        },
        Row {
            case: "a list that disables both",
            setup: None,
            request: json!([{"name": EKS, "status": DISABLED}, {"name": RUNTIME, "status": DISABLED}]),
            refused: false,
            expected: (DISABLED, DISABLED),
        },
        Row {
            case: "a list that touches neither of the two",
            setup: None,
            request: json!([{"name": "S3_DATA_EVENTS", "status": DISABLED}]),
            refused: false,
            expected: (ENABLED, DISABLED),
        },
        Row {
            case: "a list of three entries, one of them enabling Runtime Monitoring without \
                   disabling EKS Runtime Monitoring",
            setup: None,
            request: json!([{"name": "S3_DATA_EVENTS", "status": DISABLED},
                            {"name": RUNTIME, "status": ENABLED},
                            {"name": "EKS_AUDIT_LOGS", "status": DISABLED}]),
            refused: true,
            expected: (ENABLED, DISABLED),
        },
    ]
}

// ---------------------------------------------------------------------------
// Rules 1 + 2 + 4 on the detector's own switches.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_of_a_detector_is_judged_on_the_configuration_it_leaves_behind() {
    for row in rows() {
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        if let Some(setup) = row.setup.as_ref() {
            let (status, body) = update_detector(&router, &detector_id, setup.clone()).await;
            assert_eq!(
                status, 200,
                "case {:?}: the setup list must be accepted, got {status} {body:?}",
                row.case
            );
        }

        let before = detector_features(&router, &detector_id).await;
        let (status, body) = update_detector(&router, &detector_id, row.request.clone()).await;
        if row.refused {
            assert_refused(row.case, status, &body);
            let after = detector_features(&router, &detector_id).await;
            assert_eq!(
                after, before,
                "case {:?}: a refused update must configure nothing, got {after:?} after {before:?}",
                row.case
            );
        } else {
            assert_eq!(
                status, 200,
                "case {:?}: expected 200, got {status} {body:?}",
                row.case
            );
        }

        let reported = detector_features(&router, &detector_id).await;
        assert_pair(row.case, &reported, row.expected);
    }
}

// ---------------------------------------------------------------------------
// The same rows on a member account's configuration.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_of_a_member_account_is_judged_on_the_configuration_it_leaves_behind() {
    let (member_id, _) = MEMBERS[0];
    let (other_id, _) = MEMBERS[1];

    for row in rows() {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_over(Arc::clone(&engine));
        let detector_id = enabled_detector(&router).await;
        let accounts: Vec<(&str, &str)> = MEMBERS[..2].to_vec();
        let (status, body) = create_members(&router, &detector_id, &accounts).await;
        assert_eq!(
            status, 200,
            "case {:?}: create-members expected 200, got {status} {body:?}",
            row.case
        );
        // The invitation associates the accounts, and GuardDuty maintains a member
        // account's detector once that account has ANSWERED it — the relationship the
        // `member_relationship_scope` seam drives, since answering is the member
        // account's own call to make — which is what makes their detectors ones the
        // administrator configures and reads.
        let (status, body) = invite_members(&router, &detector_id, &[member_id, other_id]).await;
        assert_eq!(
            status, 200,
            "case {:?}: invite-members expected 200, got {status} {body:?}",
            row.case
        );
        for (account_id, _) in &accounts {
            monitor_member(&engine, &detector_id, account_id);
        }
        if let Some(setup) = row.setup.as_ref() {
            // Applied to BOTH accounts the row's own call names: the call configures
            // every account it names, so it is judged against each of them.
            let (status, body) = update_member_detectors(
                &router,
                &detector_id,
                &[member_id, other_id],
                json!({"features": setup}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: the setup list must be accepted, got {status} {body:?}",
                row.case
            );
        }

        let before = member_features(&router, &detector_id, member_id).await;
        let untouched = member_features(&router, &detector_id, other_id).await;
        // The call names BOTH accounts, so a refusal is the whole call's: the member
        // the row configured and the one it did not must each be left alone.
        let (status, body) = update_member_detectors(
            &router,
            &detector_id,
            &[member_id, other_id],
            json!({"features": row.request}),
        )
        .await;
        if row.refused {
            assert_refused(row.case, status, &body);
            let after = member_features(&router, &detector_id, member_id).await;
            assert_eq!(
                after, before,
                "case {:?}: a refused update must configure no member, got {after:?} after \
                 {before:?}",
                row.case
            );
            let other_after = member_features(&router, &detector_id, other_id).await;
            assert_eq!(
                other_after, untouched,
                "case {:?}: a refused update must leave every account named alongside alone, got \
                 {other_after:?} after {untouched:?}",
                row.case
            );
        } else {
            assert_eq!(
                status, 200,
                "case {:?}: expected 200, got {status} {body:?}",
                row.case
            );
        }

        let reported = member_features(&router, &detector_id, member_id).await;
        assert_pair(row.case, &reported, row.expected);
    }
}

/// One list is judged against EVERY account the call configures, so an account the
/// list cannot be applied to refuses the whole call — including for the accounts it
/// could have been applied to.
#[tokio::test]
async fn one_account_the_list_cannot_configure_refuses_the_whole_call() {
    let (prepared_id, _) = MEMBERS[0];
    let (default_id, _) = MEMBERS[1];

    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let detector_id = enabled_detector(&router).await;
    let accounts: Vec<(&str, &str)> = MEMBERS[..2].to_vec();
    let (status, body) = create_members(&router, &detector_id, &accounts).await;
    assert_eq!(
        status, 200,
        "create-members: expected 200, got {status} {body:?}"
    );
    // The invitation associates the accounts, and GuardDuty maintains a member
    // account's detector once that account has ANSWERED it, which is what makes their
    // detectors ones the administrator configures and reads.
    let (status, body) = invite_members(&router, &detector_id, &[prepared_id, default_id]).await;
    assert_eq!(
        status, 200,
        "invite-members: expected 200, got {status} {body:?}"
    );
    for (account_id, _) in &accounts {
        monitor_member(&engine, &detector_id, account_id);
    }

    // Only one of the two accounts has EKS Runtime Monitoring switched off, so only
    // that one could take Runtime Monitoring.
    let (status, body) = update_member_detectors(
        &router,
        &detector_id,
        &[prepared_id],
        json!({"features": [{"name": EKS, "status": DISABLED}]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "switching EKS Runtime Monitoring off for one account: expected 200, got {status} {body:?}"
    );

    let case = "one of two accounts still holding EKS Runtime Monitoring";
    let before_prepared = member_features(&router, &detector_id, prepared_id).await;
    let before_default = member_features(&router, &detector_id, default_id).await;
    let (status, body) = update_member_detectors(
        &router,
        &detector_id,
        &[prepared_id, default_id],
        json!({"features": [{"name": RUNTIME, "status": ENABLED}]}),
    )
    .await;
    assert_refused(case, status, &body);

    for (account_id, before) in [
        (prepared_id, before_prepared),
        (default_id, before_default),
    ] {
        let after = member_features(&router, &detector_id, account_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: {account_id} must be left exactly as it was, got {after:?} after \
             {before:?}"
        );
        assert_eq!(
            status_of(&after, RUNTIME),
            DISABLED,
            "case {case:?}: the refused call must have added {RUNTIME} to no account, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the default set a create picks yields to the create's own list.
// ---------------------------------------------------------------------------

/// One row of the create table: the `Features` list the create carries, whether it
/// is refused, and the `(EKS_RUNTIME_MONITORING, RUNTIME_MONITORING)` statuses the
/// created detector reports.
struct CreateRow {
    case: &'static str,
    features: Option<Value>,
    refused: bool,
    expected: (&'static str, &'static str),
}

#[tokio::test]
async fn a_create_stores_a_default_that_never_carries_both_features() {
    // Rows differ along which of the two the list enables, whether it states a status
    // at all, whether it names the other one beside it, and how many entries name the
    // same feature.
    let rows = [
        CreateRow {
            case: "a create naming no features at all",
            features: None,
            refused: false,
            expected: (ENABLED, DISABLED),
        },
        CreateRow {
            case: "a create enabling Runtime Monitoring",
            features: Some(json!([{"name": RUNTIME, "status": ENABLED}])),
            refused: false,
            expected: (DISABLED, ENABLED),
        },
        CreateRow {
            case: "a create naming Runtime Monitoring without a status, which enables it",
            features: Some(json!([{"name": RUNTIME}])),
            refused: false,
            expected: (DISABLED, ENABLED),
        },
        CreateRow {
            case: "a create enabling Runtime Monitoring and disabling the one it replaces",
            features: Some(
                json!([{"name": RUNTIME, "status": ENABLED}, {"name": EKS, "status": DISABLED}]),
            ),
            refused: false,
            expected: (DISABLED, ENABLED),
        },
        CreateRow {
            case: "a create enabling EKS Runtime Monitoring, which the default enables anyway",
            features: Some(json!([{"name": EKS, "status": ENABLED}])),
            refused: false,
            expected: (ENABLED, DISABLED),
        },
        CreateRow {
            case: "a create enabling both, which is the error the model documents",
            features: Some(
                json!([{"name": EKS, "status": ENABLED}, {"name": RUNTIME, "status": ENABLED}]),
            ),
            refused: true,
            expected: (ENABLED, DISABLED),
        },
        CreateRow {
            case: "a create disabling Runtime Monitoring, which the default leaves off",
            features: Some(json!([{"name": RUNTIME, "status": DISABLED}])),
            refused: false,
            expected: (ENABLED, DISABLED),
        },
        CreateRow {
            case: "a create whose last entry for Runtime Monitoring disables it",
            features: Some(
                json!([{"name": RUNTIME, "status": ENABLED}, {"name": RUNTIME, "status": DISABLED}]),
            ),
            refused: false,
            expected: (ENABLED, DISABLED),
        },
    ];

    for row in rows {
        let case = row.case;
        // One detector per account per Region, so each row gets a router of its own.
        let router = new_router();
        let mut body = json!({"enable": true});
        if let Some(features) = row.features {
            body["features"] = features;
        }
        let (status, created) = create_detector(&router, body.clone()).await;
        if row.refused {
            assert_refused(case, status, &created);
            let (status, listed) = call(&router, "GET", "/detector", None).await;
            assert_eq!(
                (status, &listed["detectorIds"]),
                (200, &json!([])),
                "case {case:?}: a refused create must store no detector, got {listed:?}"
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {created:?}"
        );
        let detector_id = created["detectorId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a detectorId, got {created:?}"))
            .to_string();
        let reported = detector_features(&router, &detector_id).await;
        assert_pair(case, &reported, row.expected);
    }
}

// ---------------------------------------------------------------------------
// Rule 5: the same rule over the whole create input space.
// ---------------------------------------------------------------------------

/// One way a create's `Features` list can treat one of the two features.
///
/// A list either leaves the feature out, or carries an entry for it — and an entry
/// may state `ENABLED`, state `DISABLED`, or state no status at all, which enables
/// the feature it names (`smithy_query(shape="DetectorFeatureConfiguration")` makes
/// `Status` optional).
#[derive(Clone, Copy, Debug)]
enum Form {
    Absent,
    WithoutStatus,
    Status(&'static str),
}

/// Every form, as the exhaustive walk enumerates them for each of the two features.
const FORMS: [Form; 4] = [
    Form::Absent,
    Form::WithoutStatus,
    Form::Status(ENABLED),
    Form::Status(DISABLED),
];

/// The entry `form` puts in the list for `name`, or nothing where it names the
/// feature at all.
fn entry(name: &str, form: Form) -> Option<Value> {
    match form {
        Form::Absent => None,
        Form::WithoutStatus => Some(json!({"name": name})),
        Form::Status(status) => Some(json!({"name": name, "status": status})),
    }
}

/// Whether the list ASKS FOR the feature `form` describes: `None` where it does not
/// name it, otherwise whether the status it states enables it.
fn asks_for(form: Form) -> Option<bool> {
    match form {
        Form::Absent => None,
        // An entry that states no status enables the feature it names, so only an
        // explicit DISABLED does not ask for it.
        Form::WithoutStatus => Some(true),
        Form::Status(status) => Some(status != DISABLED),
    }
}

/// The status a feature ends up with, computed from the RULE rather than looked up
/// per row: the list's own value where it names the feature; `DISABLED` where it
/// does not and the list enables the other one of the pair, the service's default
/// yielding to what the create asked for; and otherwise the default itself —
/// `ENABLED` for every optional feature "with an exception to RUNTIME_MONITORING"
/// (`smithy_query(query="doc",shape="CreateDetector")`), which is `enabled_by_default`.
fn resolved(own: Option<bool>, other: Option<bool>, enabled_by_default: bool) -> &'static str {
    let enabled = match own {
        Some(enabled) => enabled,
        None if other == Some(true) => false,
        None => enabled_by_default,
    };
    if enabled {
        ENABLED
    } else {
        DISABLED
    }
}

#[tokio::test]
async fn no_create_feature_list_can_store_both_runtime_monitoring_features() {
    for eks in FORMS {
        for runtime in FORMS {
            // The entries in both orders, so the walk covers which of the two the
            // list states first; with one of them absent the two lists are the same
            // one, so it is sent once.
            let orders: &[bool] = if matches!(eks, Form::Absent) || matches!(runtime, Form::Absent) {
                &[false]
            } else {
                &[false, true]
            };
            for eks_first in orders.iter().copied() {
                let ordered = if eks_first {
                    [(EKS, eks), (RUNTIME, runtime)]
                } else {
                    [(RUNTIME, runtime), (EKS, eks)]
                };
                // A list that names neither feature is the empty one, which the walk
                // covers too.
                let entries: Vec<Value> = ordered
                    .into_iter()
                    .filter_map(|(name, form)| entry(name, form))
                    .collect();
                let case = format!(
                    "create with {EKS} {eks:?}, {RUNTIME} {runtime:?}, EKS first {eks_first}: \
                     {entries:?}"
                );

                let eks_asked = asks_for(eks);
                let runtime_asked = asks_for(runtime);
                // "Specifying both ... will cause an error": it is the list naming
                // both of them as enabled that is refused, and nothing else.
                let refused = eks_asked == Some(true) && runtime_asked == Some(true);
                let expected = (
                    resolved(eks_asked, runtime_asked, true),
                    resolved(runtime_asked, eks_asked, false),
                );

                let router = new_router();
                let (status, created) =
                    create_detector(&router, json!({"enable": true, "features": entries})).await;
                if refused {
                    assert_refused(&case, status, &created);
                    assert_no_detector(&router, &case).await;
                    continue;
                }
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, got {status} {created:?}"
                );
                let reported = detector_features(&router, &created_id(&case, &created)).await;
                assert_pair(&case, &reported, expected);
            }
        }
    }
}

/// The create applies the deprecated `DataSources` view over the SAME stored
/// switches its `Features` list is applied over, so the pair resolves the same way
/// with a view present: the view configures the switches it names and neither of
/// these two.
#[tokio::test]
async fn the_deprecated_data_sources_view_does_not_change_how_the_pair_resolves() {
    // Every node the view carries, so the case covers a view that writes a switch
    // beside the pair rather than one that writes nothing.
    let view = json!({
        "s3Logs": {"enable": true},
        "kubernetes": {"auditLogs": {"enable": true}},
        "malwareProtection": {"scanEc2InstanceWithFindings": {"ebsVolumes": false}},
    });
    // `expected` is the pair a read reports, so the refused row — which stores no
    // detector at all — carries the default pair and never has it read.
    let rows = [
        ("no features beside the view", None, false, (ENABLED, DISABLED)),
        (
            "the view and a list enabling Runtime Monitoring",
            Some(json!([{"name": RUNTIME, "status": ENABLED}])),
            false,
            (DISABLED, ENABLED),
        ),
        (
            "the view and a list enabling both",
            Some(json!([{"name": RUNTIME, "status": ENABLED}, {"name": EKS, "status": ENABLED}])),
            true,
            (ENABLED, DISABLED),
        ),
    ];

    for (case, features, refused, expected) in rows {
        let router = new_router();
        let mut body = json!({"enable": true, "dataSources": view.clone()});
        if let Some(features) = features {
            body["features"] = features;
        }
        let (status, created) = create_detector(&router, body).await;
        if refused {
            assert_refused(case, status, &created);
            assert_no_detector(&router, case).await;
            continue;
        }
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {created:?}"
        );
        let reported = detector_features(&router, &created_id(case, &created)).await;
        assert_pair(case, &reported, expected);
    }
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

/// The id a successful create minted.
fn created_id(case: &str, created: &Value) -> String {
    created["detectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a detectorId, got {created:?}"))
        .to_string()
}

/// A refused create stores nothing, so the account has no detector in this Region.
async fn assert_no_detector(router: &Router, case: &str) {
    let (status, listed) = call(router, "GET", "/detector", None).await;
    assert_eq!(
        (status, &listed["detectorIds"]),
        (200, &json!([])),
        "case {case:?}: a refused create must store no detector, got {listed:?}"
    );
}

/// `UpdateDetector` carrying only a `Features` list.
async fn update_detector(router: &Router, detector_id: &str, features: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}"),
        Some(json!({"features": features})),
    )
    .await
}

/// The `(name, status)` pairs `GetDetector` reports for the detector's features.
async fn detector_features(router: &Router, detector_id: &str) -> Vec<(String, String)> {
    let (status, body) = call(router, "GET", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(
        status, 200,
        "get-detector: expected 200, got {status} {body:?}"
    );
    reported_pairs(&body)
}

/// The same pairs `GetMemberDetectors` reports for one member account.
async fn member_features(
    router: &Router,
    detector_id: &str,
    account_id: &str,
) -> Vec<(String, String)> {
    let (status, body) = get_member_detectors(router, detector_id, &[account_id]).await;
    assert_eq!(
        status, 200,
        "get-member-detectors {account_id}: expected 200, got {status} {body:?}"
    );
    let member = body["members"]
        .as_array()
        .and_then(|members| {
            members
                .iter()
                .find(|entry| entry["accountId"].as_str() == Some(account_id))
        })
        .unwrap_or_else(|| panic!("expected a configuration for {account_id}, got {body:?}"))
        .clone();
    reported_pairs(&member)
}

/// The `(name, status)` pairs a reported `features` list carries.
fn reported_pairs(reported: &Value) -> Vec<(String, String)> {
    reported["features"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a features list, got {reported:?}"))
        .iter()
        .map(|entry| {
            (
                entry["name"].as_str().unwrap_or_default().to_string(),
                entry["status"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The status a reported list holds for one feature.
fn status_of(reported: &[(String, String)], name: &str) -> String {
    reported
        .iter()
        .find(|(reported_name, _)| reported_name == name)
        .map(|(_, status)| status.clone())
        .unwrap_or_else(|| panic!("feature {name} is not reported at all: got {reported:?}"))
}

/// Assert the pair a read reports — and rule 4 with it: the two are never both
/// enabled, whatever the row sent.
fn assert_pair(case: &str, reported: &[(String, String)], expected: (&str, &str)) {
    let got = (status_of(reported, EKS), status_of(reported, RUNTIME));
    assert_eq!(
        (got.0.as_str(), got.1.as_str()),
        expected,
        "case {case:?}: expected ({EKS}, {RUNTIME}) = {expected:?}, got {reported:?}"
    );
    assert!(
        !(got.0 == ENABLED && got.1 == ENABLED),
        "case {case:?}: {EKS} and {RUNTIME} must never both be enabled, got {reported:?}"
    );
}

/// Assert one refusal: HTTP 400, the error class, the modelled envelope and the
/// message the exclusion carries.
fn assert_refused(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        CLASS,
        "case {case:?}: expected the {CLASS} class, got {body:?}"
    );
    assert_eq!(
        error_shape(body),
        ENVELOPE,
        "case {case:?}: expected the {ENVELOPE} envelope, got {body:?}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(BOTH_ENABLED_MESSAGE),
        "case {case:?}: expected the exclusion's own message, got {body:?}"
    );
}
