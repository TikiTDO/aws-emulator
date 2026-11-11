// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for connecting an EXTERNAL Kubernetes cluster:
//! `RegisterCluster`, the record it writes, the `DescribeCluster` read-back of
//! that record, and the `ListClusters` projection that decides whether a
//! connected cluster is listed at all.
//!
//! Every suite asserts a RULE over a TABLE whose rows differ along what the rule
//! generalises over — which provider is named, the shape of the connector role
//! ARN, which optional members are supplied, how many clusters of each kind the
//! store holds and at what page size — rather than one recorded request. Each
//! assertion names its case and prints the actual value.
//!
//! The wire harness (`crate::wire`) and the per-operation helpers the
//! `cluster_lifecycle` suite owns are reused as they are; nothing is
//! re-implemented here.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_eks_awswesleyfrontend::ops::types::CONNECTOR_CONFIG_PROVIDER_VALUES;
use service_com_amazonaws_eks_awswesleyfrontend::state::{
    service_state, CONNECTOR_ACTIVATION_WINDOW, CONNECTOR_ROLE_ARN_REFUSAL, LIST_INCLUDE_ALL,
};
use service_com_amazonaws_eks_awswesleyfrontend::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{create, describe, list, minimal_request, names_of, REGION};
use crate::wire::{call, emulator, Answer};

/// A connector role ARN of the form the rule accepts: an IAM role in the single
/// account this emulator serves (`engine::emulator_account_id`, which every ARN
/// it mints carries).
const PASSABLE_ROLE: &str = "arn:aws:iam::000000000000:role/AmazonEKSConnectorRole";

/// The emulator and the engine behind it, so a test can reach the very store the
/// handlers read and write.
///
/// [`emulator`] is enough for everything driven over the wire; this is for the
/// one rule the wire cannot reach, the connector lapse, whose due instant is
/// three days out. Nothing sleeps for it and no handler takes a clock argument,
/// so the test moves the stored due instant into the past and then reads the
/// cluster back over the wire — which is exactly what a caller three days later
/// does.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

async fn register(router: &MultiServiceRouter, body: &serde_json::Value) -> Answer {
    call(
        router,
        REGION,
        "POST",
        "/cluster-registrations",
        &body.to_string(),
    )
    .await
}

/// The smallest request `RegisterCluster` accepts: the name and the connector
/// configuration, which are its only required members.
fn minimal_registration(name: &str, provider: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "connectorConfig": { "roleArn": PASSABLE_ROLE, "provider": provider },
    })
}

/// The epoch seconds of a restJson1 timestamp member.
fn epoch_seconds(value: &serde_json::Value) -> f64 {
    value.as_f64().unwrap_or(f64::NAN)
}

/// Whether `value` has the hyphenated 8-4-4-4-12 lowercase-hex form an activation
/// id takes.
fn is_activation_id(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && value
            .chars()
            .all(|c| c == '-' || (c.is_ascii_hexdigit() && !c.is_ascii_uppercase()))
}

// ---------------------------------------------------------------------------
// RULE: a registration is stored as a CONNECTED cluster — it reports the
// connector configuration and nothing an Amazon EKS control plane would carry,
// it is PENDING until its manifest is applied, and a read returns exactly what
// the registration wrote.
// ---------------------------------------------------------------------------

/// The rows differ along the provider named, which optional members are supplied
/// (tags, idempotency token), and the shape and length of the cluster name.
#[tokio::test]
async fn a_registered_cluster_reads_back_exactly_as_stored() {
    let router = emulator();

    // Source for the providers: `ConnectorConfigProvider`'s own member list
    // (`smithy_query query="shape" shape="ConnectorConfigProvider"`), exposed as
    // `CONNECTOR_CONFIG_PROVIDER_VALUES`. `TANZU` and `AKS` are not providers the
    // issue cited.
    let cases: Vec<(&str, serde_json::Value)> = vec![
        (
            "only the required members",
            minimal_registration("minimal-connected", "OTHER"),
        ),
        (
            "every optional member a registration stores",
            serde_json::json!({
                "name": "fully-specified-connected",
                "connectorConfig": {
                    "roleArn": "arn:aws:iam::000000000000:role/service-role/OtherConnectorRole",
                    "provider": "EKS_ANYWHERE",
                },
                "clientRequestToken": "token-fully-specified",
                "tags": { "env": "dev", "team": "platform" },
            }),
        ),
        (
            "a single tag, the low end of the modelled 1..50 tag map",
            serde_json::json!({
                "name": "one_tag_connected",
                "connectorConfig": { "roleArn": PASSABLE_ROLE, "provider": "TANZU" },
                "tags": { "only": "one" },
            }),
        ),
        (
            "a unicode tag value, which is stored verbatim",
            serde_json::json!({
                "name": "Unicode9",
                "connectorConfig": { "roleArn": PASSABLE_ROLE, "provider": "AKS" },
                "tags": { "ключ": "значение-日本語-🙂" },
            }),
        ),
        (
            "a one-character name, the low end of the modelled 1..100 name",
            minimal_registration("a", "GKE"),
        ),
        (
            "a 100-character name, the high end of the modelled 1..100 name",
            minimal_registration(&"n".repeat(100), "RANCHER"),
        ),
    ];

    let mut activation_ids = BTreeSet::new();
    let window_seconds = CONNECTOR_ACTIVATION_WINDOW.as_secs_f64();

    for (case, request) in &cases {
        let name = request["name"].as_str().expect("the case names its cluster");
        let registered = register(&router, request).await;
        assert_eq!(
            registered.status, 200,
            "case {case:?}: expected 200 from RegisterCluster, got {registered:?}"
        );

        let cluster = registered.cluster();
        assert_eq!(
            cluster["name"],
            serde_json::json!(name),
            "case {case:?}: expected name {name:?}, got {:?}",
            cluster["name"]
        );
        let expected_arn = format!("arn:aws:eks:{REGION}:000000000000:cluster/{name}");
        assert_eq!(
            cluster["arn"],
            serde_json::json!(expected_arn),
            "case {case:?}: expected arn {expected_arn:?}, got {:?}",
            cluster["arn"]
        );
        // "After the manifest is updated and applied, the connected cluster is
        // visible to the Amazon EKS control plane" — nothing has applied one, so
        // the registration is still pending.
        assert_eq!(
            cluster["status"],
            serde_json::json!("PENDING"),
            "case {case:?}: a fresh registration is PENDING, got {:?}",
            cluster["status"]
        );

        // The registration itself: what the connector manifest is built from.
        let connector = &cluster["connectorConfig"];
        assert_eq!(
            connector["provider"],
            request["connectorConfig"]["provider"],
            "case {case:?}: expected the requested provider, got {connector:?}"
        );
        assert_eq!(
            connector["roleArn"],
            request["connectorConfig"]["roleArn"],
            "case {case:?}: expected the requested roleArn, got {connector:?}"
        );
        let activation_id = connector["activationId"].as_str().unwrap_or_default();
        assert!(
            is_activation_id(activation_id),
            "case {case:?}: expected a hyphenated 8-4-4-4-12 hex activationId, got \
             {activation_id:?}"
        );
        assert!(
            activation_ids.insert(activation_id.to_owned()),
            "case {case:?}: activationId {activation_id:?} was already issued to another \
             registration"
        );
        let activation_code = connector["activationCode"].as_str().unwrap_or_default();
        assert!(
            !activation_code.is_empty(),
            "case {case:?}: expected an activationCode, got {connector:?}"
        );
        // "If the manifest isn't applied within three days, the connected cluster
        // will no longer be visible" (`RegisterCluster`'s documentation), so the
        // activation expires exactly one window after the registration.
        let created_at = epoch_seconds(&cluster["createdAt"]);
        let expiry = epoch_seconds(&connector["activationExpiry"]);
        assert!(
            (expiry - created_at - window_seconds).abs() < 1.0,
            "case {case:?}: expected activationExpiry {window_seconds}s after createdAt \
             {created_at}, got {expiry}"
        );

        // Nothing an Amazon EKS control plane would carry: this cluster runs
        // outside Amazon Web Services.
        for absent in [
            "endpoint",
            "roleArn",
            "resourcesVpcConfig",
            "version",
            "platformVersion",
            "certificateAuthority",
            "identity",
            "kubernetesNetworkConfig",
        ] {
            assert!(
                cluster[absent].is_null(),
                "case {case:?}: a connected cluster reports no {absent}, got {:?}",
                cluster[absent]
            );
        }

        let expected_tags = request
            .get("tags")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        assert_eq!(
            cluster["tags"], expected_tags,
            "case {case:?}: expected tags {expected_tags:?}, got {:?}",
            cluster["tags"]
        );

        // THE ROUND TRIP: the describe is the same object. No clock-owed move
        // falls due within the test — the activation window is three days — so
        // even the status is identical.
        let described = describe(&router, name).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected 200 from DescribeCluster, got {described:?}"
        );
        assert_eq!(
            described.cluster(),
            registered.cluster(),
            "case {case:?}: DescribeCluster must return what RegisterCluster stored"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the connector role must name a role of the caller's own partition and
// account. Every value that is not one — a role of another account, an IAM ARN
// naming something other than a role, a value that is not an ARN at all — is
// refused with HTTP 400 `InvalidParameterException` and the one sentence that
// rule carries, which is what the recorded service answers for this member.
// (`CreateCluster$roleArn` is recorded answering the SAME question with its
// pass-role 403 instead — see `crate::pass_role_authorization`.)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_connector_role_must_be_a_role_this_caller_can_pass() {
    // (case, roleArn, expected status, expected error code)
    // Source for the ARN fields: the six-field ARN form
    // `arn:<partition>:<vendor>:<region>:<account>:<resource>`, with the account
    // every ARN this emulator mints carries (`000000000000`).
    let rows: &[(&str, &str, u16, Option<&str>)] = &[
        ("an IAM role in this account", PASSABLE_ROLE, 200, None),
        (
            "an IAM role under a path",
            "arn:aws:iam::000000000000:role/service-role/connector",
            200,
            None,
        ),
        (
            "an IAM role in another account",
            "arn:aws:iam::210987654321:role/connector",
            400,
            Some("InvalidParameterException"),
        ),
        (
            "an IAM role in another partition",
            "arn:aws-cn:iam::000000000000:role/connector",
            400,
            Some("InvalidParameterException"),
        ),
        (
            "an IAM user, which no service assumes",
            "arn:aws:iam::000000000000:user/operator",
            400,
            Some("InvalidParameterException"),
        ),
        (
            "an IAM instance profile, which no service assumes",
            "arn:aws:iam::000000000000:instance-profile/nodes",
            400,
            Some("InvalidParameterException"),
        ),
        (
            "an IAM ARN naming no role at all",
            "arn:aws:iam::000000000000:role/",
            400,
            Some("InvalidParameterException"),
        ),
        (
            "another vendor's ARN",
            "arn:aws:eks:us-west-2:000000000000:cluster/other",
            400,
            Some("InvalidParameterException"),
        ),
        (
            "an ARN that stops before its resource",
            "arn:aws:iam::000000000000",
            400,
            Some("InvalidParameterException"),
        ),
        (
            "a bare role name",
            "AmazonEKSConnectorRole",
            400,
            Some("InvalidParameterException"),
        ),
        (
            "a value that is not an ARN at all",
            "not-an-arn",
            400,
            Some("InvalidParameterException"),
        ),
        ("the empty string", "", 400, Some("InvalidParameterException")),
    ];

    for (index, (case, role_arn, expected_status, expected_code)) in rows.iter().enumerate() {
        let router = emulator();
        let name = format!("role-case-{index}");
        let request = serde_json::json!({
            "name": name,
            "connectorConfig": { "roleArn": role_arn, "provider": "OTHER" },
        });
        let answer = register(&router, &request).await;
        assert_eq!(
            answer.status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            *expected_code,
            "case {case:?}: expected error {expected_code:?}, got {answer:?}"
        );
        if *expected_status != 200 {
            // One sentence for the whole class, as the recording carries it.
            assert_eq!(
                answer.body["message"].as_str(),
                Some(CONNECTOR_ROLE_ARN_REFUSAL),
                "case {case:?}: expected the connector-role refusal, got {answer:?}"
            );
            assert_eq!(
                answer.body["clusterName"].as_str(),
                Some(name.as_str()),
                "case {case:?}: the refusal must name the cluster the request asked for, \
                 got {answer:?}"
            );
        }

        // A refused registration stored nothing; an accepted one is there.
        let described = describe(&router, &name).await;
        let expected_read = if *expected_status == 200 { 200 } else { 404 };
        assert_eq!(
            described.status, expected_read,
            "case {case:?}: expected DescribeCluster to answer {expected_read}, got \
             {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: `connectorConfig.provider` names a member of the modelled
// `ConnectorConfigProvider` enum; any other value is refused.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_provider_must_be_a_modelled_member() {
    let router = emulator();

    // Every member of the enum, straight from the model — so a model that grows a
    // provider is covered without editing this test.
    for provider in CONNECTOR_CONFIG_PROVIDER_VALUES {
        let name = format!("provider-{}", provider.to_ascii_lowercase());
        let answer = register(&router, &minimal_registration(&name, provider)).await;
        assert_eq!(
            answer.status, 200,
            "case {provider:?}: a modelled provider is accepted, got {answer:?}"
        );
        assert_eq!(
            answer.cluster()["connectorConfig"]["provider"],
            serde_json::json!(provider),
            "case {provider:?}: expected the provider echoed, got {answer:?}"
        );
    }

    // (case, the value sent)
    let refused: &[(&str, &str)] = &[
        ("a member in the wrong case", "eks_anywhere"),
        ("a provider the enum does not list", "NOMAD"),
        ("the empty string", ""),
        ("a near-miss of a member", "EKS-ANYWHERE"),
    ];
    for (index, (case, provider)) in refused.iter().enumerate() {
        let name = format!("refused-provider-{index}");
        let answer = register(&router, &minimal_registration(&name, provider)).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {answer:?}"
        );
        assert_eq!(
            describe(&router, &name).await.status,
            404,
            "case {case:?}: a refused registration stores nothing"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a cluster name identifies one cluster per Region however it got there,
// and the NAME is the whole conflict key of a registration — the
// `clientRequestToken` participates in no deduplication, so a second
// registration of a name this Region already holds is refused whatever token it
// carries, while the same token under another name registers a second, distinct
// cluster.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_second_registration_of_a_name_is_refused_whatever_the_token() {
    // (case, the token the first call sends, the token the replay sends)
    let rows: &[(&str, Option<&str>, Option<&str>)] = &[
        (
            "the same token as the first call",
            Some("token-a"),
            Some("token-a"),
        ),
        ("a different token", Some("token-a"), Some("token-b")),
        ("no token at all", Some("token-a"), None),
        ("no token on either call", None, None),
        ("a token only on the replay", None, Some("token-b")),
        (
            "a blank token on both calls",
            Some("   "),
            Some("   "),
        ),
    ];

    for (index, (case, first_token, replay_token)) in rows.iter().enumerate() {
        let router = emulator();
        let name = format!("replay-case-{index}");
        let mut first = minimal_registration(&name, "OTHER");
        if let Some(token) = first_token {
            first["clientRequestToken"] = serde_json::json!(token);
        }
        let registered = register(&router, &first).await;
        assert_eq!(
            registered.status, 200,
            "case {case:?}: expected 200 from the first RegisterCluster, got {registered:?}"
        );

        let mut replay = minimal_registration(&name, "OTHER");
        if let Some(token) = replay_token {
            replay["clientRequestToken"] = serde_json::json!(token);
        }
        let second = register(&router, &replay).await;
        assert_eq!(
            second.status, 409,
            "case {case:?}: expected 409 for a name already registered, got {second:?}"
        );
        assert_eq!(
            second.error_type.as_deref(),
            Some("ResourceInUseException"),
            "case {case:?}: expected ResourceInUseException, got {second:?}"
        );
        assert_eq!(
            second.message(),
            format!("Cluster already exists with name: {name}"),
            "case {case:?}: expected the conflict to name the cluster, got {:?}",
            second.message()
        );

        // Exactly one cluster carries the name afterwards, and it is the one the
        // first call registered.
        let listed = list(&router, &format!("include={LIST_INCLUDE_ALL}")).await;
        let names = names_of(&listed);
        assert_eq!(
            names.iter().filter(|listed| **listed == name).count(),
            1,
            "case {case:?}: expected the name exactly once, got {names:?}"
        );
        let described = describe(&router, &name).await;
        assert_eq!(
            described.cluster(),
            registered.cluster(),
            "case {case:?}: expected the refused replay to have changed nothing, got \
             {described:?}"
        );
    }

    // The same token under a DIFFERENT name is a different registration, which
    // mints a second, distinct cluster: the token deduplicates nothing.
    let router = emulator();
    let mut first = minimal_registration("shared-token-one", "OTHER");
    first["clientRequestToken"] = serde_json::json!("one-token-two-names");
    let mut second = minimal_registration("shared-token-two", "OTHER");
    second["clientRequestToken"] = serde_json::json!("one-token-two-names");
    let one = register(&router, &first).await;
    let two = register(&router, &second).await;
    assert_eq!(
        (one.status, two.status),
        (200, 200),
        "the same token under two names registers both, got {one:?} and {two:?}"
    );
    assert_ne!(
        one.cluster()["arn"],
        two.cluster()["arn"],
        "the two registrations are distinct clusters, got {:?} and {:?}",
        one.cluster()["arn"],
        two.cluster()["arn"]
    );

    // The name space is shared with created clusters: neither kind can take a
    // name the other holds — not even when the two calls carry the SAME
    // idempotency token, because a retry of one operation is not a retry of the
    // other.
    let router = emulator();
    let mut create_body = minimal_request("shared-name");
    create_body["clientRequestToken"] = serde_json::json!("shared-token");
    let mut register_body = minimal_registration("shared-name", "OTHER");
    register_body["clientRequestToken"] = serde_json::json!("shared-token");

    let created = create(&router, &create_body).await;
    assert_eq!(
        created.status, 200,
        "a created cluster is the precondition, got {created:?}"
    );
    let registered = register(&router, &register_body).await;
    assert_eq!(
        registered.error_type.as_deref(),
        Some("ResourceInUseException"),
        "registering a name a created cluster holds conflicts, even on the same token, got \
         {registered:?}"
    );

    let router = emulator();
    let registered = register(&router, &register_body).await;
    assert_eq!(
        registered.status, 200,
        "a registration is the precondition, got {registered:?}"
    );
    let created = create(&router, &create_body).await;
    assert_eq!(
        created.error_type.as_deref(),
        Some("ResourceInUseException"),
        "creating a name a registration holds conflicts, even on the same token, got {created:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: `ListClusters` projects connected clusters only for a caller that asked
// for them with `include=all`; the projection holds at every page size, and any
// other `include` value is refused.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_connected_cluster_is_listed_only_when_include_asks_for_it() {
    let router = emulator();
    let created_names = ["eks-a", "eks-b", "eks-c"];
    let connected_names = ["connected-a", "connected-b"];
    for name in created_names {
        let answer = create(&router, &minimal_request(name)).await;
        assert_eq!(answer.status, 200, "creating {name} must succeed: {answer:?}");
    }
    for name in connected_names {
        let answer = register(&router, &minimal_registration(name, "OTHER")).await;
        assert_eq!(
            answer.status, 200,
            "registering {name} must succeed: {answer:?}"
        );
    }

    let mut only_eks: Vec<String> = created_names.iter().map(|name| (*name).to_owned()).collect();
    only_eks.sort();
    let mut everything: Vec<String> = created_names
        .iter()
        .chain(connected_names.iter())
        .map(|name| (*name).to_owned())
        .collect();
    everything.sort();

    // (case, the query string, the names the projection reports)
    let projections: Vec<(&str, String, &Vec<String>)> = vec![
        ("no include at all", String::new(), &only_eks),
        (
            "an empty include, which is blank",
            "include=".to_string(),
            &only_eks,
        ),
        (
            "include=all",
            format!("include={LIST_INCLUDE_ALL}"),
            &everything,
        ),
    ];

    for (case, query, expected) in &projections {
        // The projection holds whole and page by page, at every page size from one
        // cluster to more than the store holds.
        for page_size in [None, Some(1), Some(2), Some(3), Some(100)] {
            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            loop {
                let mut parts: Vec<String> = Vec::new();
                if !query.is_empty() {
                    parts.push(query.clone());
                }
                if let Some(size) = page_size {
                    parts.push(format!("maxResults={size}"));
                }
                if let Some(next) = &token {
                    parts.push(format!("nextToken={next}"));
                }
                let answer = list(&router, &parts.join("&")).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?} (page size {page_size:?}): expected 200, got {answer:?}"
                );
                let page = names_of(&answer);
                if let Some(size) = page_size {
                    assert!(
                        page.len() <= size,
                        "case {case:?}: a page must not exceed maxResults {size}, got {page:?}"
                    );
                }
                walked.extend(page);
                token = answer.body["nextToken"].as_str().map(str::to_owned);
                if token.is_none() {
                    break;
                }
            }
            assert_eq!(
                &walked, *expected,
                "case {case:?} (page size {page_size:?}): expected {expected:?}, got {walked:?}"
            );
        }
    }

    // "'all' must be in lowercase otherwise an error occurs", and no other value
    // is documented at all: the token set is matched as an exact set membership
    // test, so a case variant is refused exactly like an unknown token.
    for value in ["ALL", "All", "connected", "eks"] {
        let answer = list(&router, &format!("include={value}")).await;
        assert_eq!(
            answer.status, 400,
            "case {value:?}: expected 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidRequestException"),
            "case {value:?}: expected InvalidRequestException, got {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a connected cluster still PENDING when its activation window closes has
// lapsed — "if the manifest isn't applied within three days, the connected
// cluster will no longer be visible and must be deregistered". The move is owed
// by the CLOCK, so it is materialised by whichever read first runs after the due
// instant, and only for a cluster that is still PENDING.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_registration_lapses_when_its_activation_window_closes() {
    // (case, the status the record is in, how far the due instant is from now,
    //  the status a read must then report)
    let rows: &[(&str, &str, Duration, bool, &str)] = &[
        (
            "the window is still open",
            "PENDING",
            CONNECTOR_ACTIVATION_WINDOW,
            false,
            "PENDING",
        ),
        (
            "the window closed with no manifest applied",
            "PENDING",
            Duration::from_secs(1),
            true,
            "FAILED",
        ),
        (
            "the window closed to the second",
            "PENDING",
            Duration::ZERO,
            true,
            "FAILED",
        ),
        (
            "a registration that already left PENDING is not dragged back",
            "ACTIVE",
            Duration::from_secs(1),
            true,
            "ACTIVE",
        ),
    ];

    for (index, (case, status_before, offset, in_the_past, expected)) in rows.iter().enumerate() {
        let (router, engine) = emulator_with_engine();
        let name = format!("lapse-case-{index}");
        let registered = register(&router, &minimal_registration(&name, "OTHER")).await;
        assert_eq!(
            registered.status, 200,
            "case {case:?}: expected 200 from RegisterCluster, got {registered:?}"
        );
        assert_eq!(
            registered.cluster()["status"],
            serde_json::json!("PENDING"),
            "case {case:?}: a fresh registration is PENDING, got {registered:?}"
        );

        // Move the stored due instant to where the case wants it, and put the
        // record in the status the case starts from. Nothing else is touched.
        let store = service_state(&*engine).expect("the eks store is reachable");
        let now = SystemTime::now();
        let due = if *in_the_past { now - *offset } else { now + *offset };
        {
            let mut guard = store.lock();
            let record = guard
                .clusters
                .get_mut(&(REGION.to_string(), name.clone()))
                .expect("RegisterCluster stored the record");
            record.status = (*status_before).to_string();
            record.activation_expires_at = Some(due);
        }

        let described = describe(&router, &name).await;
        assert_eq!(
            described.cluster()["status"],
            serde_json::json!(*expected),
            "case {case:?}: expected status {expected:?}, got {described:?}"
        );

        // The move is materialised once and stays: a second read reports the same
        // status, and the due instant it consumed is cleared rather than re-fired.
        let again = describe(&router, &name).await;
        assert_eq!(
            again.cluster()["status"],
            serde_json::json!(*expected),
            "case {case:?}: a later read reports the same status, got {again:?}"
        );
        let cleared = store
            .lock()
            .clusters
            .get(&(REGION.to_string(), name.clone()))
            .and_then(|record| record.activation_expires_at)
            .is_none();
        assert_eq!(
            cleared, *in_the_past,
            "case {case:?}: a due instant that has passed is consumed and one that has \
             not is kept; got cleared={cleared}"
        );
    }
}
