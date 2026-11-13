// SPDX-License-Identifier: CC0-1.0

//! The auto-termination policy attached to an EMR cluster, end to end over the wire.
//!
//! THE RULE: the policy a cluster reports is the one last attached to it, as attached —
//! by `RunJobFlow$AutoTerminationPolicy` or by `PutAutoTerminationPolicy` — and none at
//! all once `RemoveAutoTerminationPolicy` has taken it off or where nothing ever attached
//! one. The ATTACHMENT and the members of the attached policy are separate facts, so an
//! attached policy that carries no `IdleTimeout` is reported as the empty structure it
//! was attached as and stays distinguishable from no policy at all. An `IdleTimeout`
//! outside the window its member documents is attached by NO path: it is refused, and the
//! cluster keeps what it carried.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the router's
//! `dispatch`, so these are the same bytes an SDK client sends. Nothing reaches into the
//! service's state struct: a policy is only ever read back through
//! `GetAutoTerminationPolicy`, which is what makes these tests evidence that the response
//! is computed from persisted state.
//!
//! The rows span what the rule generalises over: the policy's FORM (empty, or carrying an
//! idle timeout) and its VALUE (each documented bound of `IdleTimeout` and one between
//! them), across each place a policy is attached and each place it stops being attached.

use serde_json::{json, Value};

// The wire harness this crate's suites share: one router over one engine, driving the
// real `POST /` path.
use crate::input_refusals::error_message;
use crate::job_flow_lifecycle::{minimal_request, Emr};

/// The documented bounds of `AutoTerminationPolicy$IdleTimeout`: "Specifies the amount of
/// idle time in seconds after which the cluster automatically terminates. You can specify
/// a minimum of 60 seconds and a maximum of 604800 seconds (seven days)."
/// (`smithy_query(query="doc", shape="AutoTerminationPolicy", member="IdleTimeout")`.)
const IDLE_TIMEOUT_BOUNDS: [i64; 2] = [60, 604800];

/// The policy `GetAutoTerminationPolicy` reports for `cluster_id` — `Value::Null` when
/// the cluster carries none, and the attached structure (possibly empty) otherwise.
async fn policy(emr: &Emr, case: &str, cluster_id: &str) -> Value {
    let (status, payload) = emr
        .call("GetAutoTerminationPolicy", json!({ "ClusterId": cluster_id }))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetAutoTerminationPolicy must answer 200, got {status} with {payload}"
    );
    payload
        .get("AutoTerminationPolicy")
        .cloned()
        .unwrap_or(Value::Null)
}

/// `PutAutoTerminationPolicy` with the policy member the row carries.
async fn put(emr: &Emr, case: &str, cluster_id: &str, policy: &Value) {
    let body = if policy.is_null() {
        // The member is optional, so a row of `null` is a request that carries NO policy
        // member at all rather than one carrying an empty policy.
        json!({ "ClusterId": cluster_id })
    } else {
        json!({ "ClusterId": cluster_id, "AutoTerminationPolicy": policy })
    };
    let (status, payload) = emr.call("PutAutoTerminationPolicy", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutAutoTerminationPolicy must answer 200, got {status} with {payload}"
    );
}

/// Policies spanning what the shape allows: the empty structure (the member is optional,
/// so an attached policy need carry nothing), each documented bound of `IdleTimeout`, and
/// a value between them.
fn policy_table() -> Vec<(&'static str, Value)> {
    vec![
        ("an empty policy", json!({})),
        (
            "the documented minimum idle timeout",
            json!({ "IdleTimeout": IDLE_TIMEOUT_BOUNDS[0] }),
        ),
        (
            "an idle timeout between the documented bounds",
            json!({ "IdleTimeout": 3_600 }),
        ),
        (
            "the documented maximum idle timeout",
            json!({ "IdleTimeout": IDLE_TIMEOUT_BOUNDS[1] }),
        ),
    ]
}

/// Put then Get is the identity on every row, and a second Put REPLACES rather than
/// merges: the policy a cluster reports is the last one attached to it.
#[tokio::test]
async fn put_then_get_reports_the_attached_policy() {
    let emr = Emr::new();
    let table = policy_table();

    for (index, (case, attached)) in table.iter().enumerate() {
        let cluster = emr.run_job_flow(minimal_request("auto terminated")).await;

        // A cluster created without a policy carries none: absent, not an empty shape.
        let before = policy(&emr, case, &cluster).await;
        assert!(
            before.is_null(),
            "case {case:?}: a cluster with no policy attached must report none, got {before}"
        );

        put(&emr, case, &cluster, attached).await;
        let got = policy(&emr, case, &cluster).await;
        assert_eq!(
            &got, attached,
            "case {case:?}: the reported policy must be the one attached, got {got}"
        );

        // Attaching a second time replaces: the next row's members are what the cluster
        // reports, and the members it omits are gone rather than merged in.
        let (next_case, replacement) = &table[(index + 1) % table.len()];
        put(&emr, next_case, &cluster, replacement).await;
        let got = policy(&emr, next_case, &cluster).await;
        assert_eq!(
            &got, replacement,
            "case {next_case:?}: a second Put must replace the attached policy, got {got}"
        );
    }
}

/// The headline distinction: an ATTACHED policy that carries no `IdleTimeout` is reported
/// as the empty structure, while a cluster nothing attached a policy to reports no policy
/// member at all — and the two are told apart over the whole put/remove sequence.
#[tokio::test]
async fn an_empty_policy_is_not_an_absent_one() {
    let emr = Emr::new();
    let with_timeout = json!({ "IdleTimeout": IDLE_TIMEOUT_BOUNDS[0] });
    let empty = json!({});

    // Each step: what to attach (`null` = a Put carrying no policy member, which attaches
    // none), and what the cluster must report afterwards.
    let steps: [(&str, Value, Value); 7] = [
        ("nothing attached yet", Value::Null, Value::Null),
        ("an empty policy attached", empty.clone(), empty.clone()),
        (
            "an idle timeout attached over the empty policy",
            with_timeout.clone(),
            with_timeout.clone(),
        ),
        (
            "the empty policy attached over the idle timeout",
            empty.clone(),
            empty.clone(),
        ),
        (
            "a Put carrying no policy member attaches none",
            Value::Null,
            Value::Null,
        ),
        (
            "an idle timeout attached again",
            with_timeout.clone(),
            with_timeout.clone(),
        ),
        ("the empty policy attached once more", empty.clone(), empty),
    ];

    let cluster = emr.run_job_flow(minimal_request("empty vs absent")).await;
    for (case, attach, expected) in &steps {
        if case != &"nothing attached yet" {
            put(&emr, case, &cluster, attach).await;
        }
        let got = policy(&emr, case, &cluster).await;
        assert_eq!(
            &got, expected,
            "case {case:?}: expected {expected}, got {got}"
        );
    }

    // And the removal is what makes an attached empty policy absent again.
    let (status, payload) = emr
        .call(
            "RemoveAutoTerminationPolicy",
            json!({ "ClusterId": cluster }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"remove the empty policy\": expected HTTP 200, got {status} with {payload}"
    );
    let got = policy(&emr, "remove the empty policy", &cluster).await;
    assert!(
        got.is_null(),
        "case \"remove the empty policy\": removal must leave no policy, got {got}"
    );
}

/// A policy attached by `RunJobFlow$AutoTerminationPolicy` is the same attachment, so the
/// cluster reports it without any `PutAutoTerminationPolicy` call.
#[tokio::test]
async fn a_policy_attached_at_creation_is_reported() {
    let emr = Emr::new();

    for (case, attached) in policy_table() {
        let mut request = minimal_request("created with a policy");
        request["AutoTerminationPolicy"] = attached.clone();
        let cluster = emr.run_job_flow(request).await;

        let got = policy(&emr, case, &cluster).await;
        assert_eq!(
            got, attached,
            "case {case:?}: RunJobFlow's policy must be reported for the new cluster, got {got}"
        );
    }
}

/// Remove takes the policy off, removing again changes nothing, and the cluster takes a
/// policy again afterwards.
#[tokio::test]
async fn remove_detaches_the_policy_and_is_idempotent() {
    let emr = Emr::new();

    for (case, attached) in policy_table() {
        let cluster = emr
            .run_job_flow(minimal_request("to be un-auto-terminated"))
            .await;
        put(&emr, case, &cluster, &attached).await;

        for attempt in 1..=2 {
            let (status, payload) = emr
                .call(
                    "RemoveAutoTerminationPolicy",
                    json!({ "ClusterId": cluster }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: removal {attempt} must answer 200, got {status} with {payload}"
            );
            let got = policy(&emr, case, &cluster).await;
            assert!(
                got.is_null(),
                "case {case:?}: removal {attempt} must leave no policy, got {got}"
            );
        }

        put(&emr, case, &cluster, &attached).await;
        let got = policy(&emr, case, &cluster).await;
        assert_eq!(
            got, attached,
            "case {case:?}: a cluster must take a policy again after removal, got {got}"
        );
    }
}

/// Reading a policy never changes what a later caller sees, and reading one cluster's
/// policy says nothing about another's.
#[tokio::test]
async fn get_is_a_read_and_clusters_are_independent() {
    let emr = Emr::new();
    let table = policy_table();

    let mut clusters = Vec::new();
    for (case, attached) in &table {
        let cluster = emr.run_job_flow(minimal_request("one of many")).await;
        put(&emr, case, &cluster, attached).await;
        clusters.push(cluster);
    }
    // A cluster that never took a policy keeps reporting none, however often it is read.
    let bare = emr.run_job_flow(minimal_request("bare")).await;

    for round in 1..=3 {
        for ((case, attached), cluster) in table.iter().zip(&clusters) {
            let got = policy(&emr, case, cluster).await;
            assert_eq!(
                &got, attached,
                "case {case:?}: read {round} must report the same policy, got {got}"
            );
        }
        let got = policy(&emr, "bare cluster", &bare).await;
        assert!(
            got.is_null(),
            "case \"bare cluster\": read {round} must report no policy, got {got}"
        );
    }
}

/// Every auto-termination-policy operation REFUSES a cluster the service holds no record
/// for, with HTTP 400 `ValidationException` — never a 501, and never an empty success —
/// and the whole family words that refusal the one way: the cluster surface's sentence,
/// quoting the `ClusterId` the request carried.
#[tokio::test]
async fn unknown_clusters_are_refused_by_every_operation() {
    let emr = Emr::new();
    // A real cluster exists, so the refusals are about the id and not about an empty
    // store.
    let live = emr.run_job_flow(minimal_request("live")).await;

    let unknown = [
        ("well-formed but absent id", format!("{live}ZZ")),
        ("another absent job-flow id", "j-3ABCDEFGHIJKL".to_string()),
        ("an id in no EMR form at all", "not-a-cluster".to_string()),
        ("an empty id", String::new()),
    ];

    for (label, cluster_id) in unknown {
        let sentence = format!("Cluster id '{cluster_id}' is not valid.");
        let requests = [
            (
                "GetAutoTerminationPolicy",
                json!({ "ClusterId": cluster_id }),
            ),
            (
                "PutAutoTerminationPolicy",
                json!({
                    "ClusterId": cluster_id,
                    "AutoTerminationPolicy": { "IdleTimeout": IDLE_TIMEOUT_BOUNDS[0] },
                }),
            ),
            (
                "RemoveAutoTerminationPolicy",
                json!({ "ClusterId": cluster_id }),
            ),
        ];
        for (operation, body) in requests {
            let case = format!("{operation} with {label}");
            let (status, payload) = emr.call(operation, body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {payload}"
            );
            let code = payload
                .get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                code.contains("ValidationException"),
                "case {case:?}: expected error code ValidationException, got {code:?} in \
                 {payload}"
            );
            assert_eq!(
                error_message(&payload),
                sentence,
                "case {case:?}: expected {sentence:?}, got {payload}"
            );
        }
    }

    // The refusals left the live cluster alone.
    let got = policy(&emr, "live cluster", &live).await;
    assert!(
        got.is_null(),
        "case \"live cluster\": a refused request must not attach a policy, got {got}"
    );
}

/// A `ClusterId` carrying characters no EMR-minted cluster id is drawn from names no
/// cluster at all, so the reporting operation refuses it on its SHAPE — naming the member
/// the caller has to fix — rather than reporting it as a cluster id it could not resolve.
#[tokio::test]
async fn a_cluster_id_in_no_cluster_form_is_refused_on_its_shape() {
    let emr = Emr::new();

    let malformed = [
        ("a cluster ARN", "arn:aws:elasticmapreduce:us-east-1:123456789012:cluster/j-1X"),
        ("a path", "clusters/j-1X2Y3Z"),
        ("an id with a space", "j-1X2Y 3Z"),
    ];

    for (label, cluster_id) in malformed {
        let case = format!("GetAutoTerminationPolicy with {label}");
        let (status, payload) = emr
            .call("GetAutoTerminationPolicy", json!({ "ClusterId": cluster_id }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            error_message(&payload),
            format!("clusterId '{cluster_id}' is not valid."),
            "case {case:?}: expected the member-shaped refusal, got {payload}"
        );
    }
}

/// The refusal sentence an `IdleTimeout` outside the documented window is answered with:
/// fixed text naming the member and the two documented bounds, and nothing of the request
/// — the rejected value is no part of it.
fn out_of_range_sentence() -> String {
    format!(
        "Auto-termination policy IdleTimeout must be within a minimum of {} seconds, and a \
         maximum of {} seconds.",
        IDLE_TIMEOUT_BOUNDS[0], IDLE_TIMEOUT_BOUNDS[1]
    )
}

/// `IdleTimeout` values spanning the window the member documents: each published bound,
/// one step inside it, one step PAST each bound, and values far outside on both sides —
/// including the zero and the negative a `Long` can carry. `true` is an accepted value.
fn idle_timeout_table() -> Vec<(&'static str, i64, bool)> {
    vec![
        ("far below the minimum", -1, false),
        ("zero idle time", 0, false),
        ("well below the minimum", 10, false),
        ("one second below the minimum", IDLE_TIMEOUT_BOUNDS[0] - 1, false),
        ("the documented minimum", IDLE_TIMEOUT_BOUNDS[0], true),
        ("one second inside the minimum", IDLE_TIMEOUT_BOUNDS[0] + 1, true),
        ("well inside the window", 3_600, true),
        ("one second inside the maximum", IDLE_TIMEOUT_BOUNDS[1] - 1, true),
        ("the documented maximum", IDLE_TIMEOUT_BOUNDS[1], true),
        ("one second past the maximum", IDLE_TIMEOUT_BOUNDS[1] + 1, false),
        ("far past the maximum", 31_536_000, false),
    ]
}

/// `PutAutoTerminationPolicy` holds `IdleTimeout` to the window its member documents: a
/// value inside it is attached and read back unchanged, and a value outside it is refused
/// with HTTP 400 `ValidationException` and attaches nothing — the cluster keeps whatever
/// it carried before.
#[tokio::test]
async fn an_idle_timeout_outside_the_documented_window_is_refused() {
    let emr = Emr::new();
    let sentence = out_of_range_sentence();
    let kept = json!({ "IdleTimeout": 3_600 });

    for (label, idle_timeout, accepted) in idle_timeout_table() {
        let cluster = emr.run_job_flow(minimal_request("bounded idle time")).await;
        // The cluster starts out carrying a policy inside the window, so a refusal is
        // visible as the EARLIER policy still being attached rather than as absence.
        put(&emr, label, &cluster, &kept).await;

        let case = format!("PutAutoTerminationPolicy with {label}");
        let (status, payload) = emr
            .call(
                "PutAutoTerminationPolicy",
                json!({
                    "ClusterId": cluster,
                    "AutoTerminationPolicy": { "IdleTimeout": idle_timeout },
                }),
            )
            .await;

        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {payload}"
            );
            let got = policy(&emr, &case, &cluster).await;
            assert_eq!(
                got,
                json!({ "IdleTimeout": idle_timeout }),
                "case {case:?}: the attached policy must be the one submitted, got {got}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        let code = payload
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            code.contains("ValidationException"),
            "case {case:?}: expected error code ValidationException, got {code:?} in {payload}"
        );
        assert_eq!(
            error_message(&payload),
            sentence,
            "case {case:?}: expected {sentence:?}, got {payload}"
        );
        let got = policy(&emr, &case, &cluster).await;
        assert_eq!(
            got, kept,
            "case {case:?}: a refused Put must leave the attached policy alone, got {got}"
        );
    }
}

/// A policy attached at CREATION is the same attachment, so `RunJobFlow` holds its
/// `IdleTimeout` to the same window: an out-of-range one is refused with the same sentence
/// and launches no cluster, while every in-range one launches one that reports it.
#[tokio::test]
async fn run_job_flow_holds_the_policy_to_the_same_window() {
    let emr = Emr::new();
    let sentence = out_of_range_sentence();

    for (label, idle_timeout, accepted) in idle_timeout_table() {
        let case = format!("RunJobFlow with {label}");
        let mut request = minimal_request("created with a bounded policy");
        request["AutoTerminationPolicy"] = json!({ "IdleTimeout": idle_timeout });
        let (status, payload) = emr.call("RunJobFlow", request).await;

        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {payload}"
            );
            let cluster = payload
                .get("JobFlowId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let got = policy(&emr, &case, &cluster).await;
            assert_eq!(
                got,
                json!({ "IdleTimeout": idle_timeout }),
                "case {case:?}: the new cluster must report the policy it launched with, \
                 got {got}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            error_message(&payload),
            sentence,
            "case {case:?}: expected {sentence:?}, got {payload}"
        );
        assert!(
            payload.get("JobFlowId").is_none(),
            "case {case:?}: a refused launch must mint no cluster, got {payload}"
        );
    }
}
