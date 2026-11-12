// SPDX-License-Identifier: CC0-1.0

//! What a `logging` configuration that enables NO control plane log type does, as this
//! service decides it.
//!
//! RULE: `LogSetup$enabled` is "If a log type is enabled, that log type exports its
//! control plane logs to CloudWatch Logs. If a log type isn't enabled, that log type
//! doesn't export its control plane logs. Each individual log type can be enabled or
//! disabled independently" — so a configuration that enables no type exports nothing at
//! all: the service provisions no log group and no stream for it, and the cluster reports
//! every type as disabled. The rule holds whichever call carries the configuration,
//! `CreateCluster` or `UpdateClusterConfig`.
//!
//! This suite pins the EKS half of that: the export a configuration asks for (none), and
//! the configuration the cluster reports back for it. What a client observes in CloudWatch
//! Logs — that no group exists for such a cluster — is pinned end to end in
//! `core/router/tests/all/eks_control_plane_log_suppression.rs`, since only EKS and
//! CloudWatch Logs on ONE engine make it observable.
//!
//! The rows differ along the FORM a configuration that enables nothing can take: every
//! type named and switched off, a subset switched off, each type switched off by its own
//! entry, an enabling entry that names no type, an entry that names types but carries no
//! `enabled` at all (both members of `LogSetup` are optional), a configuration with no
//! entry, and no `logging` member at all. Each row is read against the one row that DOES
//! enable a type, so a suite that provisioned nothing for every input would fail.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_eks_awswesleyfrontend::ops::types::{LogSetup, Logging, LOG_TYPE_VALUES};
use service_com_amazonaws_eks_awswesleyfrontend::state::{
    cluster_log_group_name, enabled_log_types, service_state, ControlPlaneLogExport,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{create, describe, REGION};
use crate::wire::{call, emulator_with_engine, Answer};

/// A role of the single account this emulator serves, which is what `iam:PassRole`
/// allows.
const PASSABLE_ROLE: &str = "arn:aws:iam::000000000000:role/eksClusterRole";

/// One row: a `logging` a request carries, and the log types it enables.
struct Row {
    case: &'static str,
    /// The `logging` member the request carries, or `None` for a request carrying none.
    logging: Option<serde_json::Value>,
    /// The types the configuration enables, in the order the model declares them.
    enabled: Vec<&'static str>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "every type named and switched off",
            logging: Some(serde_json::json!({
                "clusterLogging": [{ "types": LOG_TYPE_VALUES, "enabled": false }],
            })),
            enabled: vec![],
        },
        Row {
            case: "one type switched off, the rest not named at all",
            logging: Some(serde_json::json!({
                "clusterLogging": [{ "types": ["controllerManager"], "enabled": false }],
            })),
            enabled: vec![],
        },
        Row {
            case: "each type switched off by its own entry",
            logging: Some(serde_json::json!({
                "clusterLogging": LOG_TYPE_VALUES
                    .iter()
                    .map(|log_type| serde_json::json!({ "types": [log_type], "enabled": false }))
                    .collect::<Vec<serde_json::Value>>(),
            })),
            enabled: vec![],
        },
        Row {
            case: "an enabling entry that names no type",
            logging: Some(serde_json::json!({
                "clusterLogging": [{ "types": [], "enabled": true }],
            })),
            enabled: vec![],
        },
        Row {
            case: "an entry that names types but carries no enabled member",
            logging: Some(serde_json::json!({
                "clusterLogging": [{ "types": ["api", "scheduler"] }],
            })),
            enabled: vec![],
        },
        Row {
            case: "a configuration with no entry at all",
            logging: Some(serde_json::json!({ "clusterLogging": [] })),
            enabled: vec![],
        },
        Row {
            case: "a logging member carrying nothing",
            logging: Some(serde_json::json!({})),
            enabled: vec![],
        },
        Row {
            case: "no logging member at all",
            logging: None,
            enabled: vec![],
        },
        // The row the others are read against: one type switched on, beside the four this
        // same configuration switches off.
        Row {
            case: "one type switched on beside four switched off",
            logging: Some(serde_json::json!({
                "clusterLogging": [
                    { "types": ["authenticator"], "enabled": true },
                    {
                        "types": ["api", "audit", "controllerManager", "scheduler"],
                        "enabled": false,
                    },
                ],
            })),
            enabled: vec!["authenticator"],
        },
    ]
}

/// The row's `logging` as the `Logging` the state layer reads, so the export decision and
/// the wire answer are asked about the SAME configuration.
fn parse_logging(logging: Option<&serde_json::Value>) -> Option<Logging> {
    let setups = logging?["clusterLogging"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|setup| LogSetup {
            types: setup["types"].as_array().map(|types| {
                types
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_owned))
                    .collect()
            }),
            enabled: setup["enabled"].as_bool(),
        })
        .collect();
    Some(Logging {
        cluster_logging: Some(setups),
    })
}

/// A `CreateCluster` body, carrying the row's `logging` when it has one.
fn cluster_body(name: &str, logging: Option<&serde_json::Value>) -> serde_json::Value {
    let mut body = serde_json::json!({
        "name": name,
        "roleArn": PASSABLE_ROLE,
        "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
    });
    if let Some(logging) = logging {
        body["logging"] = logging.clone();
    }
    body
}

/// The types the cluster `described` reports as EXPORTING, read off the two `LogSetup`
/// groups a cluster reports (the enabled set first, then the disabled set).
fn reported_enabled_types(described: &Answer) -> Vec<String> {
    described.cluster()["logging"]["clusterLogging"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|setup| setup["enabled"] == serde_json::json!(true))
        .flat_map(|setup| {
            setup["types"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect::<Vec<String>>()
        })
        .collect()
}

/// Every type the cluster reports, enabled or not: a cluster reports the state of all five
/// whatever the request named.
fn reported_types(described: &Answer) -> Vec<String> {
    described.cluster()["logging"]["clusterLogging"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .flat_map(|setup| {
            setup["types"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect::<Vec<String>>()
        })
        .collect()
}

/// Move the cluster's owed `CREATING` -> `ACTIVE` instant, and the completion instant of
/// every update it carries, into the past, so the next read materialises them: an update
/// can only be started on an `ACTIVE` cluster, the configuration an update asks for is
/// written onto the cluster only when the clock completes it, and nothing in the emulator
/// sleeps for a due instant.
fn make_active(engine: &std::sync::Arc<engine::InMemoryEngine>, cluster: &str) {
    let store = service_state(&**engine).expect("the eks store");
    let mut guard = store.lock();
    let record = guard
        .clusters
        .get_mut(&(REGION.to_string(), cluster.to_string()))
        .expect("the cluster just created");
    let past = SystemTime::now() - Duration::from_secs(1);
    record.active_at = Some(past);
    for update in record.updates.values_mut() {
        if update.succeeds_at.is_some() {
            update.succeeds_at = Some(past);
        }
    }
}

/// The export the STORED cluster asks for: the decision read off the configuration the
/// cluster persisted, rather than off the request the test sent, so a configuration a
/// call discarded cannot pass for one it kept. `None` is the decision that leaves
/// CloudWatch Logs with nothing at all.
fn stored_export(
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    cluster: &str,
) -> Option<ControlPlaneLogExport> {
    let store = service_state(&**engine).expect("the eks store");
    let guard = store.lock();
    let record = guard
        .clusters
        .get(&(REGION.to_string(), cluster.to_string()))
        .expect("the cluster just created");
    ControlPlaneLogExport::of(record, record.logging.as_ref())
}

async fn update_logging(
    router: &MultiServiceRouter,
    cluster: &str,
    logging: &serde_json::Value,
) -> Answer {
    let body = serde_json::json!({ "logging": logging });
    call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{cluster}/update-config"),
        &body.to_string(),
    )
    .await
}

// ---------------------------------------------------------------------------
// RULE: a configuration that enables no log type exports nothing — no group and
// no stream are provisioned for it — and the cluster reports every type as
// disabled. The one configuration that enables a type is the control.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_configuration_that_enables_no_type_provisions_no_export() {
    let (router, engine) = emulator_with_engine();

    for (index, row) in rows().into_iter().enumerate() {
        let parsed = parse_logging(row.logging.as_ref());
        let derived = enabled_log_types(parsed.as_ref());
        assert_eq!(
            derived, row.enabled,
            "case {:?}: expected the exporting types {:?}, got {derived:?}",
            row.case, row.enabled
        );

        let cluster = format!("log-off-{index}");
        let created = create(&router, &cluster_body(&cluster, row.logging.as_ref())).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: the cluster must be created, got {created:?} ({})",
            row.case,
            created.message()
        );

        // The export the created cluster asks for, read off the configuration it stored:
        // nothing at all for one that enables no type, so no log group of this cluster's
        // own is ever asked for.
        let export = stored_export(&engine, &cluster);
        let exporting: Vec<&str> = export
            .as_ref()
            .map(|export| export.log_types.clone())
            .unwrap_or_default();
        assert_eq!(
            exporting, row.enabled,
            "case {:?}: expected an export of {:?}, got {exporting:?}",
            row.case, row.enabled
        );
        assert_eq!(
            export.is_none(),
            row.enabled.is_empty(),
            "case {:?}: a configuration enabling {:?} must provision an export exactly \
             when a type is enabled, got {export:?}",
            row.case,
            row.enabled
        );
        if let Some(export) = export.as_ref() {
            assert_eq!(
                cluster_log_group_name(&export.cluster_name),
                format!("/aws/eks/{cluster}/cluster"),
                "case {:?}: an export lands in the cluster's own log group",
                row.case
            );
        }

        // And what the cluster reports: all five types, with exactly the enabled ones in
        // the enabled group.
        let described = describe(&router, &cluster).await;
        assert_eq!(
            reported_enabled_types(&described),
            row.enabled,
            "case {:?}: the cluster must report {:?} as exporting, got {}",
            row.case,
            row.enabled,
            described.cluster()["logging"]
        );
        let mut reported = reported_types(&described);
        reported.sort();
        let mut every: Vec<String> = LOG_TYPE_VALUES.iter().map(|t| (*t).to_string()).collect();
        every.sort();
        assert_eq!(
            reported, every,
            "case {:?}: the cluster must report the state of every log type, got {}",
            row.case,
            described.cluster()["logging"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE, through the other call that carries a configuration: asking for the same
// configuration AFTER the fact is accepted as a `LoggingUpdate`, provisions the
// same export (none, for a configuration that enables nothing) and leaves the
// cluster reporting it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn switching_types_off_after_the_fact_provisions_no_export() {
    let (router, engine) = emulator_with_engine();

    for (index, row) in rows().into_iter().enumerate() {
        // Every cluster starts with a type EXPORTING, so each row is a change away from
        // an export rather than a cluster that never had one.
        let cluster = format!("log-off-update-{index}");
        let created = create(
            &router,
            &cluster_body(
                &cluster,
                Some(&serde_json::json!({
                    "clusterLogging": [{ "types": ["api"], "enabled": true }],
                })),
            ),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: the cluster must be created, got {created:?} ({})",
            row.case,
            created.message()
        );
        make_active(&engine, &cluster);

        // A request carrying no `logging` at all asks for no logging change, which this
        // operation reports under another update type: the row's configuration is only
        // asked for when there is one.
        let Some(logging) = row.logging.as_ref() else {
            continue;
        };
        let updated = update_logging(&router, &cluster, logging).await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: the configuration must be accepted after the fact, got \
             {updated:?} ({})",
            row.case,
            updated.message()
        );
        assert_eq!(
            updated.body["update"]["type"],
            serde_json::json!("LoggingUpdate"),
            "case {:?}: a logging change is a LoggingUpdate, got {}",
            row.case,
            updated.body["update"]
        );

        // The update is asynchronous, so the cluster reports the new configuration once
        // the clock completes it — and then it reports exactly the row's enabled types.
        // Nothing in the emulator sleeps for a due instant, so this moves the completion
        // instant into the past and lets the next read materialise it.
        make_active(&engine, &cluster);
        let described = describe(&router, &cluster).await;
        assert_eq!(
            reported_enabled_types(&described),
            row.enabled,
            "case {:?}: after the update the cluster must report {:?} as exporting, got {}",
            row.case,
            row.enabled,
            described.cluster()["logging"]
        );

        // And the export the cluster asks for from then on, read off the configuration
        // the completed update left on it: none, once no type is enabled — which is the
        // decision that leaves the `api` stream this cluster opened as the last thing
        // ever exported for it.
        let export = stored_export(&engine, &cluster);
        assert_eq!(
            export.is_none(),
            row.enabled.is_empty(),
            "case {:?}: after the update the cluster must ask for an export exactly when \
             a type is enabled, got {export:?}",
            row.case
        );
        let exporting: Vec<&str> = export
            .as_ref()
            .map(|export| export.log_types.clone())
            .unwrap_or_default();
        assert_eq!(
            exporting, row.enabled,
            "case {:?}: expected an export of {:?}, got {exporting:?}",
            row.case, row.enabled
        );
    }
}
