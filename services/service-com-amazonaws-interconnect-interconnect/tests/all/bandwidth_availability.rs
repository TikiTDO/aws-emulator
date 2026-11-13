// SPDX-License-Identifier: CC0-1.0

//! The ONE availability rule both writers of a connection's bandwidth judge their
//! input with, driven over the wire through the shared `crate::emulator` harness.
//!
//! `Environment$bandwidths.available` is "the list of currently available bandwidths"
//! (`smithy_query(query="doc",shape="Bandwidths",member="available")`), so a bandwidth
//! outside that list cannot be allocated on the Environment — whether it is allocated
//! by the create that opens a connection or by the update that moves an existing one
//! onto a new size. Both operations reach the same rule, so these suites drive it
//! through BOTH and require the two answers to agree byte for byte: the status, the
//! error code and the message, including the offered list the message renders.
//!
//! Each suite asserts that rule over a TABLE whose rows differ along what it
//! generalises over: the Environment (and so its provider kind, its state and the size
//! of the offered list), and the requested bandwidth — one the Environment offers, one
//! it merely `supported`s, and ones of both modelled units well outside anything the
//! catalog offers.

use serde_json::{json, Map, Value};

use service_com_amazonaws_interconnect_interconnect::state::REFUSAL_ENVELOPE;

use crate::emulator::{
    available_bandwidths, code, creatable_environments, message, remote_account_for, text, Emulator,
};

/// Bandwidths no Environment of the catalog offers, written to the
/// `ConnectionBandwidth` shape's own `^\d+[MG]bps$` pattern and 1..8 length
/// (`smithy_query(query="shape",shape="ConnectionBandwidth")`) so the boundary
/// validator passes them through to the availability rule.
///
/// The rows span both modelled units and both ends of the magnitude range: smaller
/// than anything offered, between two offered sizes, and larger than anything offered.
const NOT_OFFERED: [&str; 4] = ["1Mbps", "250Mbps", "40Gbps", "999Gbps"];

/// How the service words a bandwidth the Environment does not currently offer: the
/// service's refusal envelope, then the requested size, the Environment it was
/// requested on, and the list that Environment does offer, in the order the
/// Environment reports it.
fn not_available(bandwidth: &str, environment_id: &str, available: &[String]) -> String {
    format!(
        "{REFUSAL_ENVELOPE}bandwidth '{bandwidth}' is not available on environment \
         '{environment_id}'; available bandwidths are [{}]",
        available.join(", ")
    )
}

/// `Environment$bandwidths.supported` for one listed Environment — "the list of
/// bandwidths supported by this environment", a superset of the available list, so a
/// supported-but-not-available size is the sharpest row the rule has.
fn supported_bandwidths(environment: &Value) -> Vec<String> {
    environment["bandwidths"]["supported"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|bandwidth| bandwidth.as_str().unwrap_or_default().to_string())
        .collect()
}

/// The bandwidths an Environment does NOT offer: everything it merely `supported`s,
/// plus the sizes no Environment of the catalog offers at all.
fn unofferable_bandwidths(environment: &Value) -> Vec<String> {
    let available = available_bandwidths(environment);
    supported_bandwidths(environment)
        .into_iter()
        .chain(NOT_OFFERED.iter().map(|bandwidth| bandwidth.to_string()))
        .filter(|bandwidth| !available.contains(bandwidth))
        .collect()
}

/// A `CreateConnection` request on `environment` asking for `bandwidth`, carrying the
/// identifying information that Environment asks a create for and one of the account's
/// own gateways as the attach point.
fn create_request(
    environment: &Value,
    bandwidth: &str,
    attach_point: &str,
    description: &str,
) -> Value {
    let mut request = Map::new();
    request.insert(
        "environmentId".to_string(),
        json!(text(environment, "environmentId")),
    );
    request.insert("bandwidth".to_string(), json!(bandwidth));
    request.insert(
        "attachPoint".to_string(),
        json!({ "directConnectGateway": attach_point }),
    );
    request.insert("description".to_string(), json!(description));
    if let Some(remote_account) = remote_account_for(environment) {
        request.insert("remoteAccount".to_string(), remote_account);
    }
    Value::Object(request)
}

/// How many connections the account holds on `environment_id`, so a refusal can be
/// shown to have stored nothing.
async fn connection_count(emulator: &Emulator, environment_id: &str) -> usize {
    let listed = emulator
        .ic_ok("ListConnections", json!({ "environmentId": environment_id }))
        .await;
    listed["connections"]
        .as_array()
        .map(|connections| connections.len())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The refusal, from both writers
// ---------------------------------------------------------------------------

/// RULE: a bandwidth outside the Environment's currently available list cannot be
/// allocated on it, so both the create that opens a connection and the update that
/// resizes one answer `InterconnectValidationException` (HTTP 400) naming the
/// requested size, the Environment and the list it does offer — the same status, code
/// and message from either operation — and neither stores anything.
#[tokio::test]
async fn both_writers_refuse_a_bandwidth_the_environment_does_not_offer() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    let gateway = emulator.attach_point().await;
    let environments = creatable_environments(&catalog);
    assert!(
        !environments.is_empty(),
        "case \"catalog\": expected at least one environment that takes connections, got {environments:?}"
    );

    for environment in &environments {
        let environment_id = text(environment, "environmentId");
        let available = available_bandwidths(environment);
        let unofferable = unofferable_bandwidths(environment);
        assert!(
            !unofferable.is_empty(),
            "case {environment_id:?}: expected at least one bandwidth the environment does not offer, got {unofferable:?}"
        );

        // The connection the update rows resize, opened at a bandwidth the
        // Environment does offer.
        let subject = emulator
            .create_connection(&environment_id, &available[0], &gateway, "resize subject")
            .await;
        let subject_id = text(&subject, "id");

        for bandwidth in &unofferable {
            let case = format!("{environment_id} / {bandwidth}");
            let expected = not_available(bandwidth, &environment_id, &available);
            let held = connection_count(&emulator, &environment_id).await;

            let (create_status, created) = emulator
                .ic(
                    "CreateConnection",
                    create_request(environment, bandwidth, &gateway, "unofferable size"),
                )
                .await;
            assert_eq!(
                create_status, 400,
                "case {case:?}: expected CreateConnection to refuse with HTTP 400, got {create_status} with {created:?}"
            );
            assert!(
                code(&created).contains("InterconnectValidationException"),
                "case {case:?}: expected CreateConnection to answer InterconnectValidationException, got {:?}",
                code(&created)
            );
            assert_eq!(
                message(&created),
                expected,
                "case {case:?}: expected CreateConnection to state the availability rule, got {:?}",
                message(&created)
            );

            let (update_status, updated) = emulator
                .ic(
                    "UpdateConnection",
                    json!({ "identifier": subject_id, "bandwidth": bandwidth }),
                )
                .await;
            assert_eq!(
                update_status, create_status,
                "case {case:?}: expected UpdateConnection to answer the same status as CreateConnection ({create_status}), got {update_status} with {updated:?}"
            );
            assert_eq!(
                code(&updated),
                code(&created),
                "case {case:?}: expected both writers to answer the same error code, got {:?} on update and {:?} on create",
                code(&updated),
                code(&created)
            );
            assert_eq!(
                message(&updated),
                expected,
                "case {case:?}: expected UpdateConnection to state the availability rule in the same words, got {:?}",
                message(&updated)
            );

            // Neither refusal wrote anything: no connection was created, and the
            // resize subject still holds the size it was opened at.
            assert_eq!(
                connection_count(&emulator, &environment_id).await,
                held,
                "case {case:?}: expected the refused create to store nothing, the environment now holds a different number of connections"
            );
            let fetched = emulator.get_connection(&subject_id).await.1;
            assert_eq!(
                text(&fetched["connection"], "bandwidth"),
                available[0],
                "case {case:?}: expected the refused update to leave the record alone, got {:?}",
                fetched["connection"]["bandwidth"]
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The other side of the rule, from both writers
// ---------------------------------------------------------------------------

/// RULE: every bandwidth the Environment DOES currently offer is one a connection may
/// hold, so both writers accept it — the create reports the requested size on the new
/// connection and the update leaves an existing one reporting it — which is what shows
/// the shared rule refuses no size the Environment offers.
#[tokio::test]
async fn both_writers_accept_every_bandwidth_the_environment_offers() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    let gateway = emulator.attach_point().await;

    for environment in creatable_environments(&catalog) {
        let environment_id = text(&environment, "environmentId");
        let available = available_bandwidths(&environment);

        for bandwidth in &available {
            let case = format!("{environment_id} / {bandwidth}");

            let created = emulator
                .ic_ok(
                    "CreateConnection",
                    create_request(&environment, bandwidth, &gateway, "offered size"),
                )
                .await;
            assert_eq!(
                text(&created["connection"], "bandwidth"),
                *bandwidth,
                "case {case:?}: expected CreateConnection to open the connection at the offered size, got {:?}",
                created["connection"]["bandwidth"]
            );

            // A fresh connection per row, opened at the Environment's first offered
            // size, so the update is a real change wherever the row's size differs
            // from it and a no-change request where it does not.
            let subject = emulator
                .create_connection(&environment_id, &available[0], &gateway, "resize target")
                .await;
            let subject_id = text(&subject, "id");
            let updated = emulator
                .ic_ok(
                    "UpdateConnection",
                    json!({ "identifier": subject_id, "bandwidth": bandwidth }),
                )
                .await;
            assert_eq!(
                text(&updated["connection"], "bandwidth"),
                *bandwidth,
                "case {case:?}: expected UpdateConnection to move the connection onto the offered size, got {:?}",
                updated["connection"]["bandwidth"]
            );
            let fetched = emulator.get_connection(&subject_id).await.1;
            assert_eq!(
                text(&fetched["connection"], "bandwidth"),
                *bandwidth,
                "case {case:?}: expected GetConnection to observe the applied size, got {:?}",
                fetched["connection"]["bandwidth"]
            );
        }
    }
}
