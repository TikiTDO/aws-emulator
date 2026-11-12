// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CancelUpdate`, driven over the wire through the same
//! `MultiServiceRouter` the production binary mounts.
//!
//! The RULE under test: `CancelUpdate` resolves the cluster named by the request in the
//! region the request is signed for, then that cluster's stored update, and cancels the
//! update only while a cancellation can still stop it — in flight, of the one
//! cancellable `UpdateType`, on an EKS Auto Mode cluster, and still in its node
//! rollback phase; anything else is the modelled refusal, and a cancellation that is
//! performed is persisted on the update and hands its cluster back from `UPDATING` to
//! `ACTIVE`.
//!
//! Which refusal is itself part of the rule, because the error code is what a client
//! branches on: a cluster or an update id the region does not hold is 404
//! `ResourceNotFoundException`; an update cancellation is not supported FOR AT ALL — one
//! whose type is not the cancellable one, or whose cluster is not an Auto Mode cluster —
//! is the permanent, argument-level 400 `InvalidParameterException` naming that supported
//! combination, decided before and regardless of how far the update has got; and only an
//! update of that supported kind whose own progress makes the call impossible (already
//! terminal, or past its node rollback phase) is the state-dependent 400
//! `InvalidRequestException`.
//!
//! Each suite asserts that rule over a TABLE of inputs whose rows differ along what the
//! rule generalises over (update type, update status, how far into the update's life the
//! call arrives, whether the cluster is in Auto Mode, cluster name shape, update id
//! shape, region); the recorded requests this fix was filed from are two of the rows.
//!
//! One row is an update minted by a REAL producer (`AssociateEncryptionConfig`). The rest
//! need a `VersionRollback` update at a precise AGE — inside or past its node rollback
//! phase, or past the instant the clock owes it its terminal status — which a producer
//! call cannot place, so those rows write the update into the same
//! `ClusterRecord::updates` map the producers write (`UpdateClusterVersion` mints a
//! `VersionRollback` there), through the crate's public `state` module; the call under
//! test is always a real HTTP request, and what it persisted is read back through
//! `DescribeUpdate` and `DescribeCluster`.
//!
//! It drives the crate's shared wire harness (`tests/all/wire.rs`), using the
//! engine-returning emulator for the one thing it needs beyond a request: writing that
//! precondition into the store.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_eks_awswesleyfrontend::ops::types::UpdateParam;
use service_com_amazonaws_eks_awswesleyfrontend::state::{
    ComAmazonawsEksAwswesleyfrontendState, UpdateRecord, UpdateScope, CANCELLATION_STATUS_SUCCESSFUL,
    STATUS_ACTIVE, STATUS_UPDATING, UPDATE_CANCELLED, UPDATE_COMPLETION_DELAY, UPDATE_FAILED,
    UPDATE_IN_PROGRESS, UPDATE_SUCCESSFUL, UPDATE_TYPE_VERSION_ROLLBACK,
    VERSION_ROLLBACK_NODE_PHASE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator_with_engine, encode_label, Answer};

/// The `update` an answer carries.
fn update_of(answer: &Answer) -> &serde_json::Value {
    &answer.body["update"]
}

/// The error envelope's `message`.
fn message_of(answer: &Answer) -> &str {
    answer.body["message"].as_str().unwrap_or_default()
}

/// The error code a client matches on, with the `shape#` prefix an envelope may carry
/// stripped.
fn error_code_of(answer: &Answer) -> &str {
    answer
        .error_type
        .as_deref()
        .map(|value| value.rsplit('#').next().unwrap_or(value))
        .unwrap_or_default()
}

const REGION: &str = "us-west-2";

/// The modelled class of the PERMANENT refusal: an update cancellation is not supported
/// for at all — "the specified parameter is invalid. Review the available parameters for
/// the API request" — which no waiting turns into a success.
const NOT_CANCELLABLE: &str = "InvalidParameterException";

/// The modelled class of the STATE-dependent refusal: an update of the supported kind
/// whose own progress makes the call impossible — "the request is invalid given the state
/// of the cluster. Check the state of the cluster and the associated operations".
const NOT_IN_A_STATE_TO_CANCEL: &str = "InvalidRequestException";

/// Whether the cluster a row creates has EKS Auto Mode compute enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    AutoMode,
    SelfManaged,
}

/// `CreateCluster` through the real handler, so the cluster an update hangs off is a
/// genuine record. An Auto Mode cluster is one whose `computeConfig` enables compute —
/// and EKS Auto Mode is one mode made of three capabilities, so a request that asks for
/// it enables all three (compute, block storage and load balancing) the way Amazon EKS
/// requires them to be enabled together.
async fn create_cluster(router: &MultiServiceRouter, region: &str, name: &str, mode: Mode) {
    let mut body = serde_json::json!({
        "name": name,
        "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
        "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
    });
    if mode == Mode::AutoMode {
        body["computeConfig"] = serde_json::json!({
            "enabled": true,
            "nodePools": ["general-purpose"],
            "nodeRoleArn": "arn:aws:iam::000000000000:role/eksNodeRole",
        });
        body["storageConfig"] = serde_json::json!({ "blockStorage": { "enabled": true } });
        body["kubernetesNetworkConfig"] =
            serde_json::json!({ "elasticLoadBalancing": { "enabled": true } });
    }
    let created = call(router, region, "POST", "/clusters", &body.to_string()).await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateCluster for {name:?} in {region:?}, got {created:?}"
    );
}

async fn cancel_update_in(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    update_id: &str,
    client_request_token: Option<&str>,
) -> Answer {
    let body = match client_request_token {
        Some(token) => serde_json::json!({ "clientRequestToken": token }),
        None => serde_json::json!({}),
    };
    call(
        router,
        region,
        "POST",
        &format!(
            "/clusters/{}/updates/{}/cancel-update",
            encode_label(cluster),
            encode_label(update_id)
        ),
        &body.to_string(),
    )
    .await
}

async fn cancel_update(
    router: &MultiServiceRouter,
    cluster: &str,
    update_id: &str,
    client_request_token: Option<&str>,
) -> Answer {
    cancel_update_in(router, REGION, cluster, update_id, client_request_token).await
}

/// `DescribeUpdate` — the implemented reader this suite reads a cancellation back
/// through.
async fn describe_update(router: &MultiServiceRouter, cluster: &str, update_id: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!(
            "/clusters/{}/updates/{}",
            encode_label(cluster),
            encode_label(update_id)
        ),
        "",
    )
    .await
}

async fn describe_cluster(router: &MultiServiceRouter, cluster: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!("/clusters/{}", encode_label(cluster)),
        "",
    )
    .await
}

/// The precondition a row needs: an update of `update_type` in `status`, created `age`
/// ago and owed its terminal `Successful` status `completes_after` its creation (`None`
/// for one already in a terminal status).
///
/// It goes into the same `ClusterRecord::updates` map `AssociateEncryptionConfig` writes
/// and `DescribeUpdate` reads, and the cluster is left in the status a cluster carries
/// while one of its updates is in flight — which is what a producer does.
#[allow(clippy::too_many_arguments)]
fn seed_update(
    engine: &engine::InMemoryEngine,
    region: &str,
    cluster: &str,
    id: &str,
    update_type: &str,
    status: &str,
    age: Duration,
    completes_after: Option<Duration>,
    params: Vec<UpdateParam>,
) {
    let created_at = SystemTime::now() - age;
    let record = UpdateRecord {
        id: id.to_string(),
        r#type: update_type.to_string(),
        status: status.to_string(),
        created_at,
        params,
        client_request_token: None,
        succeeds_at: completes_after.map(|after| created_at + after),
        pending_config: None,
        cancellation: None,
        cancellation_request_token: None,
        // Every update this suite seeds is an update of the cluster itself.
        scope: UpdateScope::Cluster,
    };
    let store = engine.state::<ComAmazonawsEksAwswesleyfrontendState>();
    let mut guard = store.lock();
    let record_of = guard
        .clusters
        .get_mut(&(region.to_string(), cluster.to_string()))
        .expect("setup: the cluster was created above");
    // A cluster takes an update only from `ACTIVE`, and reports `UPDATING` while one
    // runs, so the seeded precondition carries that too; clearing `active_at` drops the
    // creation move the clock would otherwise still apply on top of it.
    record_of.active_at = None;
    record_of.status = if status == UPDATE_IN_PROGRESS {
        STATUS_UPDATING.to_string()
    } else {
        STATUS_ACTIVE.to_string()
    };
    record_of.updates.insert(id.to_string(), record);
}

/// The stored update, as the handler left it.
fn stored_update(
    engine: &engine::InMemoryEngine,
    region: &str,
    cluster: &str,
    id: &str,
) -> Option<UpdateRecord> {
    engine
        .state::<ComAmazonawsEksAwswesleyfrontendState>()
        .lock()
        .clusters
        .get(&(region.to_string(), cluster.to_string()))
        .and_then(|cluster| cluster.updates.get(id).cloned())
}

/// What the rule requires of the answer to one row.
#[derive(Debug, Clone, Copy)]
enum Expected {
    /// The cancellation is performed: 200 with the update reporting `Cancelled`.
    Cancelled,
    /// The cancellation cannot be performed: this HTTP status and error code.
    Refused(u16, &'static str),
}

/// One stored update, and what `CancelUpdate` must answer for it.
#[derive(Debug)]
struct Case {
    label: &'static str,
    id: &'static str,
    update_type: &'static str,
    status: &'static str,
    age: Duration,
    completes_after: Option<Duration>,
    mode: Mode,
    expected: Expected,
    /// The status the update must carry AFTER the call.
    stored_status_after: &'static str,
}

// ---------------------------------------------------------------------------
// RULE: an update is cancelled only while a cancellation can still stop it —
// in flight, of the one cancellable update type, on an Auto Mode cluster, and
// still in its node rollback phase. Anything outside the supported KIND (type,
// Auto Mode) is a permanent 400 InvalidParameterException naming that
// combination, whatever the update's status; an update of the supported kind
// whose own progress makes the call impossible is a 400
// InvalidRequestException. Either way the update is left as it was.
// ---------------------------------------------------------------------------

/// The table spans what the rule generalises over: the update's TYPE (five of the
/// model's `UpdateType` members — the catalog is `smithy_query query="shape"
/// shape="UpdateType"`), its STATUS (all four members of `UpdateStatus`), HOW FAR into
/// its life the call arrives (inside the node rollback phase, past it, and past the
/// moment the clock owes the update its terminal status), and whether its cluster is an
/// Auto Mode one — so the rows cross the KIND boundary with the STATE one in both
/// directions, which is what pins which refusal each produces.
#[tokio::test]
async fn an_update_is_cancelled_only_while_a_cancellation_can_still_stop_it() {
    let (router, engine) = emulator_with_engine();
    create_cluster(&router, REGION, "auto-mode-cluster", Mode::AutoMode).await;
    create_cluster(&router, REGION, "self-managed-cluster", Mode::SelfManaged).await;

    let in_flight = Some(UPDATE_COMPLETION_DELAY);
    // An update still in flight well past its node rollback phase: the phase is derived
    // from `createdAt`, so the row only has to arrive after it, and the long completion
    // keeps the row's meaning independent of how fast the test runs. The cancellable
    // rows seed `createdAt` as `now`, leaving the whole node phase for the in-process
    // dispatch to land in.
    let slow = Some(Duration::from_secs(3600));
    let past_node_phase = VERSION_ROLLBACK_NODE_PHASE + Duration::from_millis(100);

    let cases = vec![
        Case {
            label: "a version rollback whose nodes are still rolling back",
            id: "11111111-1111-1111-1111-111111111111",
            update_type: UPDATE_TYPE_VERSION_ROLLBACK,
            status: UPDATE_IN_PROGRESS,
            age: Duration::ZERO,
            completes_after: in_flight,
            mode: Mode::AutoMode,
            expected: Expected::Cancelled,
            stored_status_after: UPDATE_CANCELLED,
        },
        Case {
            label: "a version rollback whose control plane rollback has begun",
            id: "22222222-2222-2222-2222-222222222222",
            update_type: UPDATE_TYPE_VERSION_ROLLBACK,
            status: UPDATE_IN_PROGRESS,
            age: past_node_phase,
            completes_after: slow,
            mode: Mode::AutoMode,
            expected: Expected::Refused(400, NOT_IN_A_STATE_TO_CANCEL),
            stored_status_after: UPDATE_IN_PROGRESS,
        },
        Case {
            label: "a version rollback the clock has already carried to Successful",
            id: "33333333-3333-3333-3333-333333333333",
            update_type: UPDATE_TYPE_VERSION_ROLLBACK,
            status: UPDATE_IN_PROGRESS,
            age: UPDATE_COMPLETION_DELAY + Duration::from_secs(1),
            completes_after: in_flight,
            mode: Mode::AutoMode,
            expected: Expected::Refused(400, NOT_IN_A_STATE_TO_CANCEL),
            stored_status_after: UPDATE_SUCCESSFUL,
        },
        Case {
            label: "a version rollback on a cluster that is not in Auto Mode",
            id: "44444444-4444-4444-4444-444444444444",
            update_type: UPDATE_TYPE_VERSION_ROLLBACK,
            status: UPDATE_IN_PROGRESS,
            age: Duration::ZERO,
            completes_after: in_flight,
            mode: Mode::SelfManaged,
            expected: Expected::Refused(400, NOT_CANCELLABLE),
            stored_status_after: UPDATE_IN_PROGRESS,
        },
        Case {
            label: "an in-flight version update",
            id: "55555555-5555-5555-5555-555555555555",
            update_type: "VersionUpdate",
            status: UPDATE_IN_PROGRESS,
            age: Duration::ZERO,
            completes_after: in_flight,
            mode: Mode::AutoMode,
            expected: Expected::Refused(400, NOT_CANCELLABLE),
            stored_status_after: UPDATE_IN_PROGRESS,
        },
        Case {
            label: "an in-flight config update",
            id: "66666666-6666-6666-6666-666666666666",
            update_type: "ConfigUpdate",
            status: UPDATE_IN_PROGRESS,
            age: Duration::ZERO,
            completes_after: in_flight,
            mode: Mode::AutoMode,
            expected: Expected::Refused(400, NOT_CANCELLABLE),
            stored_status_after: UPDATE_IN_PROGRESS,
        },
        Case {
            label: "a config update the clock has already carried to Successful",
            id: "66666666-6666-6666-6666-66666666666a",
            update_type: "ConfigUpdate",
            status: UPDATE_IN_PROGRESS,
            age: UPDATE_COMPLETION_DELAY + Duration::from_secs(1),
            completes_after: in_flight,
            mode: Mode::AutoMode,
            expected: Expected::Refused(400, NOT_CANCELLABLE),
            stored_status_after: UPDATE_SUCCESSFUL,
        },
        Case {
            label: "a config update that already succeeded",
            id: "66666666-6666-6666-6666-66666666666b",
            update_type: "ConfigUpdate",
            status: UPDATE_SUCCESSFUL,
            age: Duration::ZERO,
            completes_after: None,
            mode: Mode::AutoMode,
            expected: Expected::Refused(400, NOT_CANCELLABLE),
            stored_status_after: UPDATE_SUCCESSFUL,
        },
        Case {
            label: "a logging update that failed",
            id: "66666666-6666-6666-6666-66666666666c",
            update_type: "LoggingUpdate",
            status: UPDATE_FAILED,
            age: Duration::ZERO,
            completes_after: None,
            mode: Mode::AutoMode,
            expected: Expected::Refused(400, NOT_CANCELLABLE),
            stored_status_after: UPDATE_FAILED,
        },
        Case {
            label: "an in-flight config update on a cluster that is not in Auto Mode",
            id: "66666666-6666-6666-6666-66666666666d",
            update_type: "ConfigUpdate",
            status: UPDATE_IN_PROGRESS,
            age: Duration::ZERO,
            completes_after: in_flight,
            mode: Mode::SelfManaged,
            expected: Expected::Refused(400, NOT_CANCELLABLE),
            stored_status_after: UPDATE_IN_PROGRESS,
        },
        Case {
            label: "an in-flight auto mode update",
            id: "77777777-7777-7777-7777-777777777777",
            update_type: "AutoModeUpdate",
            status: UPDATE_IN_PROGRESS,
            age: Duration::ZERO,
            completes_after: in_flight,
            mode: Mode::AutoMode,
            expected: Expected::Refused(400, NOT_CANCELLABLE),
            stored_status_after: UPDATE_IN_PROGRESS,
        },
        Case {
            label: "an in-flight certificate authority update",
            id: "88888888-8888-8888-8888-888888888888",
            update_type: "CertificateAuthorityUpdate",
            status: UPDATE_IN_PROGRESS,
            age: Duration::ZERO,
            completes_after: in_flight,
            mode: Mode::AutoMode,
            expected: Expected::Refused(400, NOT_CANCELLABLE),
            stored_status_after: UPDATE_IN_PROGRESS,
        },
        Case {
            label: "a version rollback that already succeeded",
            id: "99999999-9999-9999-9999-999999999999",
            update_type: UPDATE_TYPE_VERSION_ROLLBACK,
            status: UPDATE_SUCCESSFUL,
            age: Duration::ZERO,
            completes_after: None,
            mode: Mode::AutoMode,
            expected: Expected::Refused(400, NOT_IN_A_STATE_TO_CANCEL),
            stored_status_after: UPDATE_SUCCESSFUL,
        },
        Case {
            label: "a version rollback that failed",
            id: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            update_type: UPDATE_TYPE_VERSION_ROLLBACK,
            status: UPDATE_FAILED,
            age: Duration::ZERO,
            completes_after: None,
            mode: Mode::AutoMode,
            expected: Expected::Refused(400, NOT_IN_A_STATE_TO_CANCEL),
            stored_status_after: UPDATE_FAILED,
        },
        Case {
            label: "a version rollback cancelled by an earlier request",
            id: "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
            update_type: UPDATE_TYPE_VERSION_ROLLBACK,
            status: UPDATE_CANCELLED,
            age: Duration::ZERO,
            completes_after: None,
            mode: Mode::AutoMode,
            expected: Expected::Refused(400, NOT_IN_A_STATE_TO_CANCEL),
            stored_status_after: UPDATE_CANCELLED,
        },
    ];

    // One param on every row, so a 200 can be checked to carry what the RECORD holds
    // rather than anything rebuilt from the request (which names no params at all).
    let params = vec![UpdateParam {
        r#type: Some("Version".to_string()),
        value: Some("1.30".to_string()),
    }];

    for case in &cases {
        let cluster = match case.mode {
            Mode::AutoMode => "auto-mode-cluster",
            Mode::SelfManaged => "self-managed-cluster",
        };
        seed_update(
            &engine,
            REGION,
            cluster,
            case.id,
            case.update_type,
            case.status,
            case.age,
            case.completes_after,
            params.clone(),
        );

        let answer = cancel_update(&router, cluster, case.id, None).await;

        match case.expected {
            Expected::Cancelled => {
                assert_eq!(
                    answer.status, 200,
                    "case {:?}: expected 200, got {answer:?}",
                    case.label
                );
                assert_eq!(
                    update_of(&answer)["status"],
                    serde_json::json!(UPDATE_CANCELLED),
                    "case {:?}: expected the update to report Cancelled, got {:?}",
                    case.label,
                    update_of(&answer)["status"]
                );
                assert_eq!(
                    update_of(&answer)["id"],
                    serde_json::json!(case.id),
                    "case {:?}: expected the stored update id, got {:?}",
                    case.label,
                    update_of(&answer)["id"]
                );
                assert_eq!(
                    update_of(&answer)["type"],
                    serde_json::json!(case.update_type),
                    "case {:?}: expected the stored update type, got {:?}",
                    case.label,
                    update_of(&answer)["type"]
                );
                assert_eq!(
                    update_of(&answer)["params"],
                    serde_json::json!([{ "type": "Version", "value": "1.30" }]),
                    "case {:?}: expected the stored params, got {:?}",
                    case.label,
                    update_of(&answer)["params"]
                );
                assert_eq!(
                    update_of(&answer)["cancellation"]["status"],
                    serde_json::json!(CANCELLATION_STATUS_SUCCESSFUL),
                    "case {:?}: expected a recorded successful cancellation, got {:?}",
                    case.label,
                    update_of(&answer)["cancellation"]
                );
                assert!(
                    update_of(&answer)["cancellation"]["reason"]
                        .as_str()
                        .is_some_and(|reason| !reason.is_empty()),
                    "case {:?}: expected a cancellation reason, got {:?}",
                    case.label,
                    update_of(&answer)["cancellation"]
                );
            }
            Expected::Refused(status, code) => {
                assert_eq!(
                    answer.status, status,
                    "case {:?}: expected HTTP {status}, got {answer:?}",
                    case.label
                );
                assert_eq!(
                    error_code_of(&answer),
                    code,
                    "case {:?}: expected error code {code}, got {answer:?}",
                    case.label
                );
                if code == NOT_CANCELLABLE {
                    // The permanent refusal is built from the operation's contract: it
                    // names the boundary it enforces (the supported update type), not
                    // where this update happens to have got to.
                    assert!(
                        message_of(&answer).contains(UPDATE_TYPE_VERSION_ROLLBACK),
                        "case {:?}: expected the refusal to name the supported update \
                         type {UPDATE_TYPE_VERSION_ROLLBACK}, got {answer:?}",
                        case.label
                    );
                    assert!(
                        !message_of(&answer).contains(case.stored_status_after),
                        "case {:?}: expected the refusal to be keyed on the update's \
                         kind rather than its status {}, got {answer:?}",
                        case.label,
                        case.stored_status_after
                    );
                }
                assert!(
                    answer.body.get("update").is_none(),
                    "case {:?}: a refusal must carry no update, got {answer:?}",
                    case.label
                );
                assert!(
                    stored_update(&engine, REGION, cluster, case.id)
                        .is_some_and(|update| update.cancellation.is_none()),
                    "case {:?}: a refusal must record no cancellation on the update",
                    case.label
                );
            }
        }

        // What the call persisted, read back through the implemented reader.
        let read_back = describe_update(&router, cluster, case.id).await;
        assert_eq!(
            read_back.status, 200,
            "case {:?}: expected DescribeUpdate to report the update, got {read_back:?}",
            case.label
        );
        assert_eq!(
            update_of(&read_back)["status"],
            serde_json::json!(case.stored_status_after),
            "case {:?}: expected DescribeUpdate to report {}, got {:?}",
            case.label,
            case.stored_status_after,
            update_of(&read_back)["status"]
        );
        let stored = stored_update(&engine, REGION, cluster, case.id)
            .unwrap_or_else(|| panic!("case {:?}: the update must still be stored", case.label));
        assert_eq!(
            stored.status, case.stored_status_after,
            "case {:?}: expected the stored update to be {}, got {:?}",
            case.label, case.stored_status_after, stored.status
        );
        assert!(
            stored.succeeds_at.is_none_or(|due| due > SystemTime::now()),
            "case {:?}: a due status move must have been materialised, got {:?}",
            case.label,
            stored.succeeds_at
        );
    }
}

/// An update minted by a real producer is subject to the same rule: it is not a
/// `VersionRollback`, so cancelling it can never be supported and the call is refused
/// with the permanent, argument-level class while the update keeps running.
#[tokio::test]
async fn an_update_started_by_a_real_producer_is_refused_for_its_type() {
    const CLUSTER: &str = "producer-cluster";
    let (router, _engine) = emulator_with_engine();
    create_cluster(&router, REGION, CLUSTER, Mode::AutoMode).await;
    // The producer starts an update only on an `ACTIVE` cluster.
    tokio::time::sleep(
        service_com_amazonaws_eks_awswesleyfrontend::state::CLUSTER_ACTIVATION_DELAY
            + Duration::from_millis(150),
    )
    .await;

    let started = call(
        &router,
        REGION,
        "POST",
        &format!("/clusters/{CLUSTER}/encryption-config/associate"),
        &serde_json::json!({
            "encryptionConfig": [{
                "resources": ["secrets"],
                "provider": {
                    "keyArn": format!(
                        "arn:aws:kms:{REGION}:000000000000:key/9b1deb4d-3b7d-4bad-9bdd-2b0d7b3dcb6d"
                    ),
                },
            }],
        })
        .to_string(),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "setup: expected AssociateEncryptionConfig to start an update, got {started:?}"
    );
    let update_id = update_of(&started)["id"]
        .as_str()
        .expect("setup: the started update carries an id")
        .to_owned();

    let answer = cancel_update(&router, CLUSTER, &update_id, None).await;
    assert_eq!(
        answer.status, 400,
        "case \"a real AssociateEncryptionConfig update\": expected HTTP 400, got {answer:?}"
    );
    assert_eq!(
        error_code_of(&answer),
        NOT_CANCELLABLE,
        "case \"a real AssociateEncryptionConfig update\": expected {NOT_CANCELLABLE}, \
         got {answer:?}"
    );
    assert!(
        message_of(&answer).contains(UPDATE_TYPE_VERSION_ROLLBACK),
        "case \"a real AssociateEncryptionConfig update\": expected the refusal to name \
         the supported update type, got {answer:?}"
    );

    let read_back = describe_update(&router, CLUSTER, &update_id).await;
    assert_eq!(
        update_of(&read_back)["status"],
        serde_json::json!(UPDATE_IN_PROGRESS),
        "case \"a real AssociateEncryptionConfig update\": the refused update must keep \
         running, got {:?}",
        update_of(&read_back)["status"]
    );
}

/// The refusal for an update cancellation is not supported for does NOT depend on how
/// far that update has got: the same real producer update is refused the same way while
/// it is still running and after the clock has carried it to its terminal status, and
/// neither refusal disturbs it.
#[tokio::test]
async fn an_unsupported_update_is_refused_the_same_way_running_or_finished() {
    const CLUSTER: &str = "logging-cluster";
    let (router, _engine) = emulator_with_engine();
    create_cluster(&router, REGION, CLUSTER, Mode::AutoMode).await;
    // The producer starts an update only on an `ACTIVE` cluster.
    tokio::time::sleep(
        service_com_amazonaws_eks_awswesleyfrontend::state::CLUSTER_ACTIVATION_DELAY
            + Duration::from_millis(150),
    )
    .await;

    let started = call(
        &router,
        REGION,
        "POST",
        &format!("/clusters/{CLUSTER}/update-config"),
        &serde_json::json!({
            "logging": { "clusterLogging": [{ "types": ["api"], "enabled": true }] },
        })
        .to_string(),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "setup: expected UpdateClusterConfig to start an update, got {started:?}"
    );
    let update_id = update_of(&started)["id"]
        .as_str()
        .expect("setup: the started update carries an id")
        .to_owned();

    // Row 1: the update is still in flight. Row 2: the clock has carried it to its
    // terminal status. The refusal must be identical.
    for case in ["still in progress", "already successful"] {
        if case == "already successful" {
            tokio::time::sleep(UPDATE_COMPLETION_DELAY + Duration::from_millis(250)).await;
        }
        let expected_status = if case == "still in progress" {
            UPDATE_IN_PROGRESS
        } else {
            UPDATE_SUCCESSFUL
        };

        let answer = cancel_update(&router, CLUSTER, &update_id, None).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            error_code_of(&answer),
            NOT_CANCELLABLE,
            "case {case:?}: expected {NOT_CANCELLABLE}, got {answer:?}"
        );
        assert!(
            message_of(&answer).contains(UPDATE_TYPE_VERSION_ROLLBACK),
            "case {case:?}: expected the refusal to name the supported update type, \
             got {answer:?}"
        );

        // The rejected call had no effect: the update keeps its status and acquired no
        // cancellation.
        let read_back = describe_update(&router, CLUSTER, &update_id).await;
        assert_eq!(
            update_of(&read_back)["status"],
            serde_json::json!(expected_status),
            "case {case:?}: expected the refused update to still report {expected_status}, \
             got {:?}",
            update_of(&read_back)["status"]
        );
        assert!(
            update_of(&read_back).get("cancellation").is_none(),
            "case {case:?}: a refused cancel must add no cancellation, got {read_back:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the cluster is resolved first, and only in the region the request is
// signed for; a cluster the region does not hold is a 404 naming the cluster.
// ---------------------------------------------------------------------------

/// The table spans the shapes a cluster name takes — length and character class — plus
/// the case where the id DOES name a real update, which pins the ORDER of the two
/// resolutions.
#[tokio::test]
async fn a_cluster_the_region_does_not_hold_is_a_not_found_naming_the_cluster() {
    const REAL_CLUSTER: &str = "real-cluster";
    const REAL_UPDATE: &str = "cccccccc-cccc-cccc-cccc-cccccccccccc";
    let (router, engine) = emulator_with_engine();
    create_cluster(&router, REGION, REAL_CLUSTER, Mode::AutoMode).await;
    seed_update(
        &engine,
        REGION,
        REAL_CLUSTER,
        REAL_UPDATE,
        UPDATE_TYPE_VERSION_ROLLBACK,
        UPDATE_IN_PROGRESS,
        Duration::ZERO,
        Some(UPDATE_COMPLETION_DELAY),
        Vec::new(),
    );

    let hundred_character_name = "c".repeat(100);
    let cases: Vec<(&str, &str, &str)> = vec![
        ("a name that was never created", "ghost-cluster", REAL_UPDATE),
        ("a one-character name", "z", REAL_UPDATE),
        (
            "a mixed-case name with both legal separators",
            "Ghost_Cluster-9",
            REAL_UPDATE,
        ),
        (
            "a 100-character name, the top of CreateCluster's modelled length",
            &hundred_character_name,
            REAL_UPDATE,
        ),
        (
            "an unknown cluster AND an unknown update id",
            "another-ghost",
            "dddddddd-dddd-dddd-dddd-dddddddddddd",
        ),
    ];

    for (label, cluster, update_id) in &cases {
        let answer = cancel_update(&router, cluster, update_id, None).await;
        assert_eq!(
            answer.status, 404,
            "case {label:?}: expected HTTP 404, got {answer:?}"
        );
        assert_eq!(
            error_code_of(&answer),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        // The message is built from the request, naming the cluster that could not be
        // resolved — the same construction `DescribeCluster` and `DescribeUpdate` use.
        assert_eq!(
            message_of(&answer),
            format!("No cluster found for name: {cluster}."),
            "case {label:?}: expected the cluster-not-found message, got {answer:?}"
        );
    }

    // The real update was never touched by any of those calls.
    let stored = stored_update(&engine, REGION, REAL_CLUSTER, REAL_UPDATE)
        .expect("the seeded update is still stored");
    assert_eq!(
        stored.status, UPDATE_IN_PROGRESS,
        "a refusal about another cluster must not touch this update, got {:?}",
        stored.status
    );
}

/// An update is minted for a cluster in ONE region, so the same name and id in another
/// region resolves nothing. The table spans the regions a client can sign for.
#[tokio::test]
async fn an_update_is_reachable_only_in_the_region_its_cluster_lives_in() {
    const CLUSTER: &str = "regional-cluster";
    const UPDATE_ID: &str = "eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee";
    let (router, engine) = emulator_with_engine();
    create_cluster(&router, REGION, CLUSTER, Mode::AutoMode).await;
    seed_update(
        &engine,
        REGION,
        CLUSTER,
        UPDATE_ID,
        UPDATE_TYPE_VERSION_ROLLBACK,
        UPDATE_IN_PROGRESS,
        Duration::ZERO,
        Some(UPDATE_COMPLETION_DELAY),
        Vec::new(),
    );

    for other_region in ["us-east-1", "eu-central-1", "ap-southeast-2"] {
        let answer = cancel_update_in(&router, other_region, CLUSTER, UPDATE_ID, None).await;
        assert_eq!(
            answer.status, 404,
            "case {other_region:?}: expected HTTP 404 for another region, got {answer:?}"
        );
        assert_eq!(
            message_of(&answer),
            format!("No cluster found for name: {CLUSTER}."),
            "case {other_region:?}: expected the cluster-not-found message, got {answer:?}"
        );
    }

    // The cluster's own region still cancels it, so the refusals above were about the
    // region and nothing else.
    let answer = cancel_update(&router, CLUSTER, UPDATE_ID, None).await;
    assert_eq!(
        answer.status, 200,
        "case {REGION:?}: expected the update's own region to cancel it, got {answer:?}"
    );
    assert_eq!(
        update_of(&answer)["status"],
        serde_json::json!(UPDATE_CANCELLED),
        "case {REGION:?}: expected Cancelled, got {:?}",
        update_of(&answer)["status"]
    );
}

// ---------------------------------------------------------------------------
// RULE: an update id the cluster does not hold is a 404 naming the id, and
// leaves every stored update alone.
// ---------------------------------------------------------------------------

/// The table spans the shapes an update id takes on the wire: a well-formed UUID that
/// was never minted, the right id in the wrong case (ids are case-sensitive), a value
/// that is not a UUID at all, one carrying characters that must survive path encoding,
/// and a very long one.
#[tokio::test]
async fn an_update_id_the_cluster_does_not_hold_is_a_not_found_naming_the_id() {
    const CLUSTER: &str = "id-cluster";
    const UPDATE_ID: &str = "ffffffff-ffff-ffff-ffff-ffffffffffff";
    let (router, engine) = emulator_with_engine();
    create_cluster(&router, REGION, CLUSTER, Mode::AutoMode).await;
    seed_update(
        &engine,
        REGION,
        CLUSTER,
        UPDATE_ID,
        UPDATE_TYPE_VERSION_ROLLBACK,
        UPDATE_IN_PROGRESS,
        Duration::ZERO,
        Some(UPDATE_COMPLETION_DELAY),
        Vec::new(),
    );

    let long_id = "1".repeat(256);
    let cases: Vec<(&str, &str)> = vec![
        (
            "a well-formed UUID that was never minted",
            "00000000-0000-0000-0000-000000000000",
        ),
        (
            "the stored id in upper case",
            "FFFFFFFF-FFFF-FFFF-FFFF-FFFFFFFFFFFF",
        ),
        ("a value that is not a UUID", "not-an-update-id"),
        ("a value needing path encoding", "update id/with slash"),
        ("a 256-character value", &long_id),
    ];

    for (label, update_id) in &cases {
        let answer = cancel_update(&router, CLUSTER, update_id, None).await;
        assert_eq!(
            answer.status, 404,
            "case {label:?}: expected HTTP 404, got {answer:?}"
        );
        assert_eq!(
            error_code_of(&answer),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            message_of(&answer),
            format!("Update {update_id} not found"),
            "case {label:?}: expected the update-not-found message, got {answer:?}"
        );
    }

    let stored = stored_update(&engine, REGION, CLUSTER, UPDATE_ID)
        .expect("the seeded update is still stored");
    assert_eq!(
        stored.status, UPDATE_IN_PROGRESS,
        "a refusal about another id must not touch this update, got {:?}",
        stored.status
    );
    assert!(
        stored.cancellation.is_none(),
        "a refusal about another id must record no cancellation, got {:?}",
        stored.cancellation
    );
}

// ---------------------------------------------------------------------------
// RULE (invariant): the cancellation is persisted — the update reads back
// `Cancelled` and its cluster is handed back to ACTIVE — and
// `clientRequestToken` makes the request idempotent: a retry carrying the same
// token is answered with the cancellation that already ran, while a different
// request is refused because the update is no longer in progress.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_retry_with_the_same_idempotency_token_is_answered_with_the_cancellation_it_performed() {
    const CLUSTER: &str = "idempotent-cluster";
    const UPDATE_ID: &str = "12345678-1234-1234-1234-123456789abc";
    const TOKEN: &str = "cancel-token-1";
    let (router, engine) = emulator_with_engine();
    create_cluster(&router, REGION, CLUSTER, Mode::AutoMode).await;
    seed_update(
        &engine,
        REGION,
        CLUSTER,
        UPDATE_ID,
        UPDATE_TYPE_VERSION_ROLLBACK,
        UPDATE_IN_PROGRESS,
        Duration::ZERO,
        Some(UPDATE_COMPLETION_DELAY),
        Vec::new(),
    );

    let first = cancel_update(&router, CLUSTER, UPDATE_ID, Some(TOKEN)).await;
    assert_eq!(
        first.status, 200,
        "case \"first cancel\": expected 200, got {first:?}"
    );

    // Three retries of the SAME request, each answered with what that request did —
    // including the `createdAt` of the update it cancelled, which no answer rebuilds.
    for attempt in 1..=3 {
        let retry = cancel_update(&router, CLUSTER, UPDATE_ID, Some(TOKEN)).await;
        assert_eq!(
            retry.status, 200,
            "case \"retry {attempt}\": expected 200, got {retry:?}"
        );
        assert_eq!(
            retry.body, first.body,
            "case \"retry {attempt}\": expected the first answer verbatim, got {retry:?}"
        );
    }

    // A different cancel request is NOT that retry: the update is no longer in
    // progress, so it is refused.
    for (label, token) in [
        ("another token", Some("cancel-token-2")),
        ("no token at all", None),
    ] {
        let other = cancel_update(&router, CLUSTER, UPDATE_ID, token).await;
        assert_eq!(
            other.status, 400,
            "case {label:?}: expected HTTP 400 for a second cancellation, got {other:?}"
        );
        assert_eq!(
            error_code_of(&other),
            NOT_IN_A_STATE_TO_CANCEL,
            "case {label:?}: expected {NOT_IN_A_STATE_TO_CANCEL}, got {other:?}"
        );
    }

    // The cancellation is in the store and readable through DescribeUpdate, not just in
    // the answer.
    let read_back = describe_update(&router, CLUSTER, UPDATE_ID).await;
    assert_eq!(
        update_of(&read_back)["status"],
        serde_json::json!(UPDATE_CANCELLED),
        "expected DescribeUpdate to report Cancelled, got {read_back:?}"
    );
    assert_eq!(
        update_of(&read_back)["cancellation"],
        update_of(&first)["cancellation"],
        "expected DescribeUpdate to report the recorded cancellation, got {read_back:?}"
    );
    let stored =
        stored_update(&engine, REGION, CLUSTER, UPDATE_ID).expect("the update is still stored");
    assert_eq!(
        stored.cancellation_request_token.as_deref(),
        Some(TOKEN),
        "expected the stored cancellation to remember its token, got {:?}",
        stored.cancellation_request_token
    );
    assert_eq!(
        stored.succeeds_at, None,
        "expected the clock-owed Successful move to be dropped, got {:?}",
        stored.succeeds_at
    );

    // The cluster the cancelled update was taken from is running again.
    let cluster = describe_cluster(&router, CLUSTER).await;
    assert_eq!(
        cluster.body["cluster"]["status"],
        serde_json::json!(STATUS_ACTIVE),
        "expected the cluster to be handed back to ACTIVE, got {cluster:?}"
    );
}

/// Cancelling one update changes that update and nothing else: not the other updates of
/// the same cluster, and not the same-shaped update of another cluster.
#[tokio::test]
async fn cancelling_one_update_leaves_every_other_update_alone() {
    let (router, engine) = emulator_with_engine();
    let ids = [
        "a0000000-0000-0000-0000-000000000001",
        "a0000000-0000-0000-0000-000000000002",
        "a0000000-0000-0000-0000-000000000003",
    ];
    for cluster in ["cluster-one", "cluster-two"] {
        create_cluster(&router, REGION, cluster, Mode::AutoMode).await;
        for id in ids {
            seed_update(
                &engine,
                REGION,
                cluster,
                id,
                UPDATE_TYPE_VERSION_ROLLBACK,
                UPDATE_IN_PROGRESS,
                Duration::ZERO,
                Some(UPDATE_COMPLETION_DELAY),
                Vec::new(),
            );
        }
    }

    let answer = cancel_update(&router, "cluster-one", ids[1], None).await;
    assert_eq!(answer.status, 200, "expected 200, got {answer:?}");

    for cluster in ["cluster-one", "cluster-two"] {
        for id in ids {
            let expected = if cluster == "cluster-one" && id == ids[1] {
                UPDATE_CANCELLED
            } else {
                UPDATE_IN_PROGRESS
            };
            let stored = stored_update(&engine, REGION, cluster, id)
                .unwrap_or_else(|| panic!("case {cluster:?}/{id:?}: the update is still stored"));
            assert_eq!(
                stored.status, expected,
                "case {cluster:?}/{id:?}: expected {expected}, got {:?}",
                stored.status
            );
        }
    }
}
