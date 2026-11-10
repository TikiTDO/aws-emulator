// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UpdateClusterVersion`, driven over the wire through the same
//! `MultiServiceRouter` the production binary mounts.
//!
//! The RULE under test: `UpdateClusterVersion` starts ONE asynchronous update that moves
//! the cluster named by the request, in the region the request is signed for, by exactly
//! one published Kubernetes minor version — the DIRECTION deciding the update's type
//! (forward a `VersionUpdate`, backward a `VersionRollback`) — and the cluster keeps its
//! old version and reports `UPDATING` until the clock completes that update, only then
//! reporting the requested version and `ACTIVE`. A request the cluster cannot take is the
//! modelled refusal instead: 404 `ResourceNotFoundException` for a cluster the region does
//! not hold, 409 `ResourceInUseException` while another update of it is running, 400
//! `InvalidParameterException` for a version this service does not offer or a move of more
//! than one minor version, 400 `InvalidRequestException` for a cluster whose state cannot
//! take the update, and 400 `InvalidStateException` for the documented upgrade readiness
//! issue `force` overrides.
//!
//! Every suite asserts that rule over a TABLE of inputs whose rows differ along what the
//! rule generalises over (which version pair is moved between, the direction, the
//! `rollbackConfig` timeout, the cluster's state, the node groups hanging off it, the
//! idempotency token), and each assertion names its case and prints the actual value. The
//! version pairs are not written out here: they are read from the Kubernetes version
//! calendar this service publishes (`DescribeClusterVersions`, seeded in
//! `crate::state`), so the table spans whatever that calendar carries rather than a
//! snapshot of it.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    offers_cluster_version, ComAmazonawsEksAwswesleyfrontendState, CLUSTER_ACTIVATION_DELAY,
    MAX_ROLLBACK_TIMEOUT_MINUTES, MIN_ROLLBACK_TIMEOUT_MINUTES, PLATFORM_VERSION, STATUS_ACTIVE,
    STATUS_UPDATING, UPDATE_COMPLETION_DELAY, UPDATE_IN_PROGRESS, UPDATE_SUCCESSFUL,
    UPDATE_TYPE_VERSION, UPDATE_TYPE_VERSION_ROLLBACK,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator_with_engine, encode_label, error_code, Answer};

const REGION: &str = "us-west-2";

/// A role of the single account this emulator serves, which is what `iam:PassRole`
/// allows.
const PASSABLE_ROLE: &str = "arn:aws:iam::000000000000:role/eksClusterRole";

/// The `update` an answer carries.
fn update_of(answer: &Answer) -> &serde_json::Value {
    &answer.body["update"]
}

/// The `Update$id` an answer reports.
fn update_id_of(answer: &Answer) -> String {
    update_of(answer)["id"].as_str().unwrap_or_default().to_owned()
}

/// The value the update reports for one `UpdateParamType`.
fn param_value(answer: &Answer, param_type: &str) -> String {
    update_of(answer)["params"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .find(|param| param["type"] == serde_json::json!(param_type))
        .and_then(|param| param["value"].as_str().map(str::to_owned))
        .unwrap_or_default()
}

async fn create_cluster(
    router: &MultiServiceRouter,
    name: &str,
    version: Option<&str>,
) -> Answer {
    let mut body = serde_json::json!({
        "name": name,
        "roleArn": PASSABLE_ROLE,
        "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
    });
    if let Some(version) = version {
        body["version"] = serde_json::json!(version);
    }
    let created = call(router, REGION, "POST", "/clusters", &body.to_string()).await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateCluster for {name:?}, got {created:?}"
    );
    created
}

async fn create_nodegroup(
    router: &MultiServiceRouter,
    cluster: &str,
    name: &str,
    version: Option<&str>,
) -> Answer {
    let mut body = serde_json::json!({
        "nodegroupName": name,
        "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
        "subnets": ["subnet-aaaa1111", "subnet-bbbb2222"],
    });
    if let Some(version) = version {
        body["version"] = serde_json::json!(version);
    }
    let created = call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{}/node-groups", encode_label(cluster)),
        &body.to_string(),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateNodegroup for {name:?} on {cluster:?}, got {created:?}"
    );
    created
}

async fn register_cluster(router: &MultiServiceRouter, name: &str) -> Answer {
    let body = serde_json::json!({
        "name": name,
        "connectorConfig": { "roleArn": PASSABLE_ROLE, "provider": "EKS_ANYWHERE" },
    });
    let registered = call(
        router,
        REGION,
        "POST",
        "/cluster-registrations",
        &body.to_string(),
    )
    .await;
    assert_eq!(
        registered.status, 200,
        "setup: expected 200 from RegisterCluster for {name:?}, got {registered:?}"
    );
    registered
}

/// `UpdateClusterVersion` over the wire: `POST /clusters/{name}/updates`.
async fn update_version(
    router: &MultiServiceRouter,
    cluster: &str,
    body: &serde_json::Value,
) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{}/updates", encode_label(cluster)),
        &body.to_string(),
    )
    .await
}

async fn describe_cluster(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!("/clusters/{}", encode_label(name)),
        "",
    )
    .await
}

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

/// Wait out the span the clock owes a newly created cluster before it is `ACTIVE`, plus a
/// margin so the test does not race the due instant.
async fn settle_creation() {
    tokio::time::sleep(CLUSTER_ACTIVATION_DELAY + Duration::from_millis(250)).await;
}

/// Wait out the span the clock owes an in-flight update before it is `Successful`.
async fn settle_update() {
    tokio::time::sleep(UPDATE_COMPLETION_DELAY + Duration::from_millis(250)).await;
}

/// The Kubernetes versions this service OFFERS a control plane for, newest first.
///
/// Read from the store the handlers read rather than written out here: the calendar is
/// what `DescribeClusterVersions` reports, and which of its rows are offered depends on
/// the clock, so a hand-copied list would both duplicate the catalog and go stale. A row
/// the clock has carried past the end of extended support is not one a cluster can be
/// created on, so it is not one these rows move between either.
fn published_versions(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = engine.state::<ComAmazonawsEksAwswesleyfrontendState>();
    let guard = store.lock();
    let now = SystemTime::now();
    guard
        .released_cluster_versions(now)
        .map(|row| row.cluster_version.clone())
        .filter(|version| offers_cluster_version(&guard, version, now))
        .collect()
}

/// A cluster name for a version pair: a name may not carry a `.`
/// (`ClusterName`'s pattern is `^[0-9A-Za-z][A-Za-z0-9\-_]*$`).
fn cluster_name(prefix: &str, version: &str) -> String {
    format!("{prefix}-{}", version.replace('.', "-"))
}

// ---------------------------------------------------------------------------
// RULE: a version update moves the cluster by one published minor version, the
// direction deciding the update's type, and the cluster reports the new version
// only once the clock completes the update.
// ---------------------------------------------------------------------------

/// One row: the version a cluster is created at, the version the update moves it to, and
/// the `UpdateType` that move must report.
struct Move {
    from: String,
    to: String,
    update_type: &'static str,
    cluster: String,
}

/// The table spans every ADJACENT pair of published versions, in BOTH directions: each
/// pair is one row forward (an upgrade) and one row back (a rollback), so the rows differ
/// in the version values, in the direction and in how far up the calendar they sit.
fn version_moves(published: &[String]) -> Vec<Move> {
    let mut moves = Vec::new();
    for (index, pair) in published.windows(2).enumerate() {
        let (newer, older) = (pair[0].clone(), pair[1].clone());
        moves.push(Move {
            cluster: cluster_name(&format!("upgrade{index}"), &older),
            from: older.clone(),
            to: newer.clone(),
            update_type: UPDATE_TYPE_VERSION,
        });
        moves.push(Move {
            cluster: cluster_name(&format!("rollback{index}"), &newer),
            from: newer,
            to: older,
            update_type: UPDATE_TYPE_VERSION_ROLLBACK,
        });
    }
    moves
}

#[tokio::test]
async fn a_version_update_moves_the_cluster_one_minor_version_in_the_direction_its_type_reports() {
    let (router, engine) = emulator_with_engine();
    let published = published_versions(&engine);
    assert!(
        published.len() >= 2,
        "setup: the published version calendar must carry at least two released versions, got \
         {published:?}"
    );
    let moves = version_moves(&published);

    for row in &moves {
        create_cluster(&router, &row.cluster, Some(&row.from)).await;
    }
    settle_creation().await;

    // Every row's update is started before anything is settled, so each one is observed
    // while it is still in flight.
    let mut started = Vec::new();
    for row in &moves {
        let case = format!("{} -> {}", row.from, row.to);
        let answer = update_version(&router, &row.cluster, &serde_json::json!({
            "version": row.to,
        }))
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 from UpdateClusterVersion, got {answer:?}"
        );
        assert_eq!(
            update_of(&answer)["type"],
            serde_json::json!(row.update_type),
            "case {case:?}: expected update type {:?}, got {:?}",
            row.update_type,
            update_of(&answer)["type"]
        );
        assert_eq!(
            update_of(&answer)["status"],
            serde_json::json!(UPDATE_IN_PROGRESS),
            "case {case:?}: expected a {UPDATE_IN_PROGRESS} update, got {:?}",
            update_of(&answer)["status"]
        );
        assert_eq!(
            param_value(&answer, "Version"),
            row.to,
            "case {case:?}: expected the Version param to report {:?}, got {:?}",
            row.to,
            param_value(&answer, "Version")
        );
        assert_eq!(
            param_value(&answer, "PlatformVersion"),
            PLATFORM_VERSION,
            "case {case:?}: expected the PlatformVersion param to report {PLATFORM_VERSION:?}, \
             got {:?}",
            param_value(&answer, "PlatformVersion")
        );

        // The update is asynchronous: while it runs the cluster is `UPDATING` and still
        // reports the version it is moving AWAY from.
        let described = describe_cluster(&router, &row.cluster).await;
        assert_eq!(
            described.body["cluster"]["status"],
            serde_json::json!(STATUS_UPDATING),
            "case {case:?}: expected the cluster to be {STATUS_UPDATING} while the update runs, \
             got {:?}",
            described.body["cluster"]["status"]
        );
        assert_eq!(
            described.body["cluster"]["version"],
            serde_json::json!(row.from),
            "case {case:?}: expected the cluster to still report {:?} while the update runs, got \
             {:?}",
            row.from,
            described.body["cluster"]["version"]
        );

        let id = update_id_of(&answer);
        let read_back = describe_update(&router, &row.cluster, &id).await;
        assert_eq!(
            read_back.status, 200,
            "case {case:?}: expected DescribeUpdate to find update {id:?}, got {read_back:?}"
        );
        assert_eq!(
            update_of(&read_back),
            update_of(&answer),
            "case {case:?}: expected DescribeUpdate to report the update the call wrote, got {:?}",
            update_of(&read_back)
        );
        started.push((row, id));
    }

    settle_update().await;

    for (row, id) in &started {
        let case = format!("{} -> {}", row.from, row.to);
        let completed = describe_update(&router, &row.cluster, id).await;
        assert_eq!(
            update_of(&completed)["status"],
            serde_json::json!(UPDATE_SUCCESSFUL),
            "case {case:?}: expected the completed update to be {UPDATE_SUCCESSFUL}, got {:?}",
            update_of(&completed)["status"]
        );
        let described = describe_cluster(&router, &row.cluster).await;
        assert_eq!(
            described.body["cluster"]["version"],
            serde_json::json!(row.to),
            "case {case:?}: expected the completed update to leave the cluster at {:?}, got {:?}",
            row.to,
            described.body["cluster"]["version"]
        );
        assert_eq!(
            described.body["cluster"]["status"],
            serde_json::json!(STATUS_ACTIVE),
            "case {case:?}: expected the cluster to be {STATUS_ACTIVE} once the update completed, \
             got {:?}",
            described.body["cluster"]["status"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the version asked for must be one this service offers, exactly one
// minor version from the cluster's own; anything else is a 400.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_version_this_service_does_not_offer_or_a_move_of_more_than_one_minor_version_is_refused()
{
    let (router, engine) = emulator_with_engine();
    let published = published_versions(&engine);
    assert!(
        published.len() >= 3,
        "setup: the published version calendar must carry at least three released versions, got \
         {published:?}"
    );
    let newest = published[0].clone();
    let third = published[2].clone();

    create_cluster(&router, "refuse-at-newest", Some(&newest)).await;
    create_cluster(&router, "refuse-at-third", Some(&third)).await;
    settle_creation().await;

    // The rows differ in HOW the requested version fails the rule: not on the calendar at
    // all, not a Kubernetes minor version, a patch version rather than a minor one, the
    // version the cluster already runs, and two minor versions away in either direction.
    let rows: Vec<(&str, &str, &str, u16, &str)> = vec![
        (
            "a version the published calendar does not carry",
            "refuse-at-newest",
            "1.99",
            400,
            "InvalidParameterException",
        ),
        (
            "a version that is not a Kubernetes version at all",
            "refuse-at-newest",
            "not-a-version",
            400,
            "InvalidParameterException",
        ),
        (
            "the version the cluster already runs",
            "refuse-at-newest",
            &newest,
            400,
            "InvalidRequestException",
        ),
        (
            "two minor versions forward",
            "refuse-at-third",
            &newest,
            400,
            "InvalidParameterException",
        ),
        (
            "two minor versions back",
            "refuse-at-newest",
            &third,
            400,
            "InvalidParameterException",
        ),
    ];

    for (case, cluster, version, status, code) in rows {
        let answer = update_version(&router, cluster, &serde_json::json!({ "version": version }))
            .await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status} for version {version:?}, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            code,
            "case {case:?}: expected {code} for version {version:?}, got {:?}",
            error_code(&answer)
        );
        // A refused request starts nothing, so the cluster is left as it was.
        let described = describe_cluster(&router, cluster).await;
        assert_eq!(
            described.body["cluster"]["status"],
            serde_json::json!(STATUS_ACTIVE),
            "case {case:?}: expected a refused request to leave the cluster {STATUS_ACTIVE}, got \
             {:?}",
            described.body["cluster"]["status"]
        );
    }

    // A patch version is not a minor version either, and the calendar carries none.
    let patch = format!("{newest}.1");
    let answer = update_version(
        &router,
        "refuse-at-newest",
        &serde_json::json!({ "version": patch }),
    )
    .await;
    assert_eq!(
        error_code(&answer),
        "InvalidParameterException",
        "case \"a patch version rather than a minor version\": expected \
         InvalidParameterException for {patch:?}, got {answer:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: `rollbackConfig.timeoutMinutes` is accepted only inside the range the
// model's documentation publishes for it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_rollback_timeout_is_accepted_only_within_the_published_range() {
    let (router, engine) = emulator_with_engine();
    let published = published_versions(&engine);
    assert!(
        published.len() >= 2,
        "setup: the published version calendar must carry at least two released versions, got \
         {published:?}"
    );
    let newest = published[0].clone();
    let previous = published[1].clone();

    // The rows are each published bound, a value inside the range, and one past each
    // bound. Every accepted row starts an update, so each row gets its own cluster.
    let rows: Vec<(&str, i32, bool)> = vec![
        ("one minute below the published minimum", MIN_ROLLBACK_TIMEOUT_MINUTES - 1, false),
        ("the published minimum", MIN_ROLLBACK_TIMEOUT_MINUTES, true),
        ("a value inside the published range", 720, true),
        ("the published maximum", MAX_ROLLBACK_TIMEOUT_MINUTES, true),
        ("one minute above the published maximum", MAX_ROLLBACK_TIMEOUT_MINUTES + 1, false),
    ];

    for (index, _) in rows.iter().enumerate() {
        create_cluster(&router, &format!("timeout-row{index}"), Some(&newest)).await;
    }
    settle_creation().await;

    for (index, (case, timeout_minutes, accepted)) in rows.iter().enumerate() {
        let cluster = format!("timeout-row{index}");
        let answer = update_version(
            &router,
            &cluster,
            &serde_json::json!({
                "version": previous,
                "rollbackConfig": { "timeoutMinutes": timeout_minutes },
            }),
        )
        .await;
        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200 for timeoutMinutes {timeout_minutes}, got {answer:?}"
            );
            assert_eq!(
                update_of(&answer)["type"],
                serde_json::json!(UPDATE_TYPE_VERSION_ROLLBACK),
                "case {case:?}: expected a {UPDATE_TYPE_VERSION_ROLLBACK} update, got {:?}",
                update_of(&answer)["type"]
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400 for timeoutMinutes {timeout_minutes}, got {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "InvalidParameterException",
                "case {case:?}: expected InvalidParameterException for timeoutMinutes \
                 {timeout_minutes}, got {:?}",
                error_code(&answer)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: only a cluster whose state can take an update takes one — it must
// exist in the region, be an Amazon EKS cluster, be ACTIVE, and have no other
// update of its own in flight.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn only_a_cluster_whose_state_can_take_a_version_update_takes_one() {
    let (router, engine) = emulator_with_engine();
    let published = published_versions(&engine);
    assert!(
        published.len() >= 2,
        "setup: the published version calendar must carry at least two released versions, got \
         {published:?}"
    );
    let newest = published[0].clone();
    let previous = published[1].clone();

    // An Amazon EKS cluster that is ACTIVE and already has an update in flight.
    create_cluster(&router, "state-in-flight", Some(&newest)).await;
    settle_creation().await;
    let first = update_version(
        &router,
        "state-in-flight",
        &serde_json::json!({ "version": previous }),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "setup: expected the first update of state-in-flight to be accepted, got {first:?}"
    );

    // A connected cluster, which has no Amazon EKS control plane at all.
    register_cluster(&router, "state-connected").await;
    // A cluster the clock has not carried to ACTIVE yet: created last and asked
    // immediately, so the request lands inside its activation window.
    create_cluster(&router, "state-creating", Some(&newest)).await;

    let rows: Vec<(&str, &str, u16, &str)> = vec![
        (
            "a cluster the region does not hold",
            "state-missing",
            404,
            "ResourceNotFoundException",
        ),
        (
            "a cluster that is still being created",
            "state-creating",
            400,
            "InvalidRequestException",
        ),
        (
            "a connected cluster, which EKS does not manage a control plane for",
            "state-connected",
            400,
            "ClientException",
        ),
        (
            "a cluster that already has an update in flight",
            "state-in-flight",
            409,
            "ResourceInUseException",
        ),
    ];

    for (case, cluster, status, code) in rows {
        let answer = update_version(&router, cluster, &serde_json::json!({ "version": previous }))
            .await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status} for cluster {cluster:?}, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            code,
            "case {case:?}: expected {code} for cluster {cluster:?}, got {:?}",
            error_code(&answer)
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: `clientRequestToken` makes the call idempotent — a retry carrying the
// token of an update this operation already started is answered with THAT
// update, and a token that identifies nothing is not a retry.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_retried_request_is_answered_with_the_update_it_already_started() {
    let (router, engine) = emulator_with_engine();
    let published = published_versions(&engine);
    assert!(
        published.len() >= 2,
        "setup: the published version calendar must carry at least two released versions, got \
         {published:?}"
    );
    let newest = published[0].clone();
    let previous = published[1].clone();

    // The rows differ in the token the retry carries: the same one (a retry), a different
    // one (a second request), and none at all (nothing to recognise it by).
    let rows: Vec<(&str, &str, Option<&str>, Option<&str>)> = vec![
        (
            "the same token twice",
            "token-same",
            Some("aa11bb22-cc33-dd44-ee55-ff6677889900"),
            Some("aa11bb22-cc33-dd44-ee55-ff6677889900"),
        ),
        (
            "a different token",
            "token-different",
            Some("11111111-2222-3333-4444-555555555555"),
            Some("99999999-8888-7777-6666-555555555555"),
        ),
        ("no token at all", "token-none", None, None),
    ];

    for (_, cluster, _, _) in &rows {
        create_cluster(&router, cluster, Some(&newest)).await;
    }
    settle_creation().await;

    for (case, cluster, first_token, second_token) in &rows {
        let mut body = serde_json::json!({ "version": previous });
        if let Some(token) = first_token {
            body["clientRequestToken"] = serde_json::json!(token);
        }
        let first = update_version(&router, cluster, &body).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: expected 200 from the first call, got {first:?}"
        );

        let mut retry_body = serde_json::json!({ "version": previous });
        if let Some(token) = second_token {
            retry_body["clientRequestToken"] = serde_json::json!(token);
        }
        let second = update_version(&router, cluster, &retry_body).await;

        if first_token.is_some() && first_token == second_token {
            assert_eq!(
                second.status, 200,
                "case {case:?}: expected the retry to be answered, got {second:?}"
            );
            assert_eq!(
                update_id_of(&second),
                update_id_of(&first),
                "case {case:?}: expected the retry to report update {:?}, got {:?}",
                update_id_of(&first),
                update_id_of(&second)
            );
        } else {
            assert_eq!(
                second.status, 409,
                "case {case:?}: expected a second request to be refused while the first update \
                 runs, got {second:?}"
            );
            assert_eq!(
                error_code(&second),
                "ResourceInUseException",
                "case {case:?}: expected ResourceInUseException, got {:?}",
                error_code(&second)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: a managed node group whose Kubernetes version does not match the
// cluster's blocks a version update, and `force` overrides that readiness
// check.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn node_groups_that_lag_the_cluster_version_block_an_update_unless_force_overrides_it() {
    let (router, engine) = emulator_with_engine();
    let published = published_versions(&engine);
    assert!(
        published.len() >= 3,
        "setup: the published version calendar must carry at least three released versions, got \
         {published:?}"
    );
    let newest = published[0].clone();
    let middle = published[1].clone();
    let oldest = published[2].clone();

    // Two clusters start at `middle`, where the update under test is the move to
    // `newest`; the two that must LAG start a version behind and are carried to `middle`
    // by an update of their own, which their node group — created at the older version,
    // the only version `CreateNodegroup` accepts — does not follow.
    create_cluster(&router, "readiness-no-nodegroups", Some(&middle)).await;
    create_cluster(&router, "readiness-matching", Some(&middle)).await;
    create_cluster(&router, "readiness-lagging-blocked", Some(&oldest)).await;
    create_cluster(&router, "readiness-lagging-forced", Some(&oldest)).await;
    settle_creation().await;

    create_nodegroup(&router, "readiness-matching", "matching", Some(&middle)).await;
    create_nodegroup(&router, "readiness-lagging-blocked", "left-behind", None).await;
    create_nodegroup(&router, "readiness-lagging-forced", "left-behind", None).await;

    for cluster in ["readiness-lagging-blocked", "readiness-lagging-forced"] {
        let answer = update_version(&router, cluster, &serde_json::json!({ "version": middle }))
            .await;
        assert_eq!(
            answer.status, 200,
            "setup: expected {cluster:?} to accept the update to {middle:?} while its node group \
             still matches, got {answer:?}"
        );
    }
    settle_update().await;

    // The rows differ in the node groups hanging off the cluster and in `force`.
    let rows: Vec<(&str, &str, bool, u16, &str)> = vec![
        ("a cluster with no node groups", "readiness-no-nodegroups", false, 200, ""),
        (
            "a node group at the cluster's own version",
            "readiness-matching",
            false,
            200,
            "",
        ),
        (
            "a node group left behind by an earlier version update",
            "readiness-lagging-blocked",
            false,
            400,
            "InvalidStateException",
        ),
        (
            "the same node group, with force overriding the readiness check",
            "readiness-lagging-forced",
            true,
            200,
            "",
        ),
    ];

    for (case, cluster, force, status, code) in rows {
        let answer = update_version(
            &router,
            cluster,
            &serde_json::json!({ "version": newest, "force": force }),
        )
        .await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status} from UpdateClusterVersion, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            code,
            "case {case:?}: expected error code {code:?}, got {:?}",
            error_code(&answer)
        );
        if status == 200 {
            assert_eq!(
                update_of(&answer)["type"],
                serde_json::json!(UPDATE_TYPE_VERSION),
                "case {case:?}: expected a {UPDATE_TYPE_VERSION} update, got {:?}",
                update_of(&answer)["type"]
            );
        }
    }
}
