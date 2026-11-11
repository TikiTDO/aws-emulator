// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `AcceptInboundConnection` — the destination (remote) domain
//! owner's decision on a cross-cluster connection request that is waiting for one.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`], the connection request helpers from
//! [`super::cross_cluster_connections`] and the inbound report from
//! [`super::inbound_connections`]; this suite adds no second copy of any of them.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * accepting a request that waits for a decision moves the ONE stored record both
//!   views report out of `PENDING_ACCEPTANCE`, and the clock then owes it the rest of
//!   the lifecycle an accepted request drives itself through — over a table of
//!   connections differing in connection mode, in connection properties and in how
//!   fully the source reference names its account and Region;
//! * an id that addresses no connection THIS caller has a view of is the modelled
//!   missing-resource refusal that names the id, over a table spanning ids this
//!   service never minted (including the shortest the model allows), a request still
//!   being validated and a request whose validation failed — and the same id becomes
//!   acceptable once its validation falls due, so the refusal is about the status and
//!   not about the id;
//! * a connection that is not waiting for a decision cannot be accepted, over every
//!   status an accepted connection goes on to hold, and the refused call leaves the
//!   record exactly where it was.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    CONNECTION_MODE_DIRECT, CONNECTION_MODE_VPC_ENDPOINT, CONNECTION_STATUS_ACTIVE,
    CONNECTION_STATUS_APPROVED, CONNECTION_STATUS_PENDING_ACCEPTANCE,
    CONNECTION_STATUS_PROVISIONING, CONNECTION_STATUS_VALIDATING,
    CONNECTION_STATUS_VALIDATION_FAILED,
};

use super::cross_cluster_connections::{
    create_connection, created_domain, describe_connections as describe_outbound, domain_info,
    reported, ACCOUNT, REGION,
};
use super::domain_lifecycle::{
    error_code, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT,
};
use super::inbound_connections::describe_inbound;

/// The statuses an accepted connection holds, in the order its lifecycle visits them:
/// the acceptance itself, the service putting the connection in place, and the
/// connection in use. From `InboundConnectionStatusCode`'s own documentation
/// (`smithy_query(query="doc",shape="InboundConnectionStatus",member="StatusCode")`):
/// `APPROVED`, then `PROVISIONING` — "Inbound connection is being provisioned" —
/// then `ACTIVE` — "Inbound connection is active and ready to use".
const ACCEPTED_CHAIN: [&str; 3] = [
    CONNECTION_STATUS_APPROVED,
    CONNECTION_STATUS_PROVISIONING,
    CONNECTION_STATUS_ACTIVE,
];

/// The members `InboundConnection` does NOT model, so an answer carrying one would be
/// reporting a member the shape has no place for
/// (`smithy_query(query="shape",shape="InboundConnection")`).
const UNMODELLED_MEMBERS: [&str; 2] = ["ConnectionAlias", "ConnectionProperties"];

/// How long the suite polls for a status the clock owes before giving up, and how
/// long it waits between polls. Nothing in the emulator sleeps: the span is
/// materialised by whichever later call reads the record.
///
/// Each step of [`ACCEPTED_CHAIN`] is owed one provisioning span of this service
/// (`crate::state::PROVISIONING`, seconds rather than milliseconds), so the budget
/// below — the limit times the interval — is several times the whole chain.
const POLL_LIMIT: u32 = 60;
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// `AcceptInboundConnection`: `PUT /opensearch/cc/inboundConnection/{ConnectionId}/accept`.
async fn accept(emulator: &Emulator, id: &str) -> Answer {
    emulator
        .call(
            "PUT",
            &format!("{PREFIX}/opensearch/cc/inboundConnection/{id}/accept"),
            None,
        )
        .await
}

/// One filter selecting exactly the connection `id` names.
fn by_id(id: &str) -> Value {
    json!({ "Filters": [{ "Name": "connection-id", "Values": [id] }] })
}

/// The status the destination owner's report gives `id`, or `None` where that report
/// does not carry the connection at all.
async fn inbound_status(emulator: &Emulator, case: &str, id: &str) -> Option<String> {
    let answer = describe_inbound(emulator, by_id(id)).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected DescribeInboundConnections to answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    reported(&answer, case, id).map(|connection| status_code(&connection, case))
}

/// The status the source owner's report gives `id`, which is the same record seen
/// from the other side.
async fn outbound_status(emulator: &Emulator, case: &str, id: &str) -> String {
    let answer = describe_outbound(emulator, by_id(id)).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected DescribeOutboundConnections to answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    let connection = reported(&answer, case, id).unwrap_or_else(|| {
        panic!(
            "case {case}: the source owner's report must still carry {id:?}, got {:?}",
            answer.body
        )
    });
    status_code(&connection, case)
}

/// One reported connection's `ConnectionStatus.StatusCode`.
fn status_code(connection: &Value, case: &str) -> String {
    connection["ConnectionStatus"]["StatusCode"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a StatusCode, got {connection:?}"))
        .to_owned()
}

/// A failed answer's `message`.
fn error_message(answer: &Answer) -> String {
    answer.body["message"].as_str().unwrap_or_default().to_owned()
}

/// Create one connection between two domains and answer its `ConnectionId`.
async fn created_connection(emulator: &Emulator, case: &str, body: Value) -> String {
    let answer = create_connection(emulator, body).await;
    assert_eq!(
        answer.status, 200,
        "case CreateOutboundConnection({case}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["ConnectionId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case CreateOutboundConnection({case}): expected a ConnectionId, got {:?}",
                answer.body
            )
        })
        .to_owned()
}

/// Poll the destination owner's report until the accepted connection is in place,
/// asserting it only ever moves FORWARD along [`ACCEPTED_CHAIN`] — the lifecycle an
/// accepted request drives itself through — and that it gets there.
async fn walks_to_active(emulator: &Emulator, case: &str, id: &str) {
    let mut reached = 0usize;
    for _ in 0..POLL_LIMIT {
        let status = inbound_status(emulator, case, id).await.unwrap_or_else(|| {
            panic!("case {case}: an accepted connection stays in the destination owner's view")
        });
        let at = ACCEPTED_CHAIN
            .iter()
            .position(|step| *step == status)
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: an accepted connection only holds a status of \
                     {ACCEPTED_CHAIN:?}, got {status:?}"
                )
            });
        assert!(
            at >= reached,
            "case {case}: an accepted connection never moves back along its lifecycle, got \
             {status:?} after {:?}",
            ACCEPTED_CHAIN[reached]
        );
        reached = at;
        if status == CONNECTION_STATUS_ACTIVE {
            return;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "case {case}: an accepted connection must reach {CONNECTION_STATUS_ACTIVE}, it stopped at \
         {:?}",
        ACCEPTED_CHAIN[reached]
    );
}

/// One connection to accept: what makes this row differ from its siblings, the source
/// domain and the reference sent for it with the one expected back, the
/// `ConnectionMode` sent (absent where `None`) with the mode expected back, and the
/// `ConnectionProperties` sent.
struct Row {
    case: &'static str,
    alias: &'static str,
    source: &'static str,
    local: Value,
    expected_local: Value,
    mode: Option<&'static str>,
    expected_mode: &'static str,
    properties: Option<Value>,
}

/// The table of connections accepted: the mode omitted and named either way, no
/// properties / an endpoint / a cross-cluster-search property, and a source reference
/// naming its account and Region in full, partly or not at all.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a source naming only the domain, and no mode",
            alias: "accept-plain",
            source: "acc-source-a",
            local: domain_info("acc-source-a", None, None),
            expected_local: domain_info("acc-source-a", Some(ACCOUNT), Some(REGION)),
            mode: None,
            expected_mode: CONNECTION_MODE_DIRECT,
            properties: None,
        },
        Row {
            case: "a source in another account and Region, mode named",
            alias: "accept-cross-account",
            source: "acc-source-b",
            local: domain_info("acc-source-b", Some("210987654321"), Some("eu-west-1")),
            expected_local: domain_info("acc-source-b", Some("210987654321"), Some("eu-west-1")),
            mode: Some(CONNECTION_MODE_DIRECT),
            expected_mode: CONNECTION_MODE_DIRECT,
            properties: Some(json!({ "CrossClusterSearch": { "SkipUnavailable": "ENABLED" } })),
        },
        Row {
            case: "the other connection mode, with the endpoint property",
            alias: "accept-vpc",
            source: "acc-source-c",
            local: domain_info("acc-source-c", None, Some("ap-south-1")),
            expected_local: domain_info("acc-source-c", Some(ACCOUNT), Some("ap-south-1")),
            mode: Some(CONNECTION_MODE_VPC_ENDPOINT),
            expected_mode: CONNECTION_MODE_VPC_ENDPOINT,
            properties: Some(json!({ "Endpoint": "vpc-remote.eu-west-1.es.amazonaws.com" })),
        },
    ]
}

/// The create body of a row, connecting its source domain to `destination`.
fn create_body(row: &Row, destination: &str) -> Value {
    let mut body = json!({
        "ConnectionAlias": row.alias,
        "LocalDomainInfo": row.local,
        "RemoteDomainInfo": domain_info(destination, None, None),
    });
    if let Some(mode) = row.mode {
        body["ConnectionMode"] = json!(mode);
    }
    if let Some(properties) = &row.properties {
        body["ConnectionProperties"] = properties.clone();
    }
    body
}

#[tokio::test]
async fn accepting_a_pending_request_moves_the_one_record_both_views_report() {
    let emulator = Emulator::new();
    created_domain(&emulator, "acc-destination").await;
    let table = rows();
    for row in &table {
        created_domain(&emulator, row.source).await;
    }

    let mut created: Vec<String> = Vec::new();
    for row in &table {
        created
            .push(created_connection(&emulator, row.case, create_body(row, "acc-destination")).await);
    }

    // Nothing but the clock brings a validated request to the point of being decided.
    tokio::time::sleep(SETTLE_WAIT).await;

    for (row, id) in table.iter().zip(created.iter()) {
        let case = row.case;
        assert_eq!(
            inbound_status(&emulator, case, id).await.as_deref(),
            Some(CONNECTION_STATUS_PENDING_ACCEPTANCE),
            "case {case}: the request must be waiting for the destination owner's decision \
             before it is accepted"
        );

        let answer = accept(&emulator, id).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected AcceptInboundConnection to answer 200, got {} with {:?}",
            answer.status, answer.body
        );
        let accepted = &answer.body["Connection"];
        assert_eq!(
            accepted["ConnectionId"],
            json!(id),
            "case {case}: expected the answer to carry the accepted connection {id:?}, got {:?}",
            answer.body
        );
        // The decision is taken by this call, and the connection is not in place yet:
        // the span is never zero, so a caller acting straight after the accept sees
        // the transient status.
        assert_eq!(
            status_code(accepted, case),
            CONNECTION_STATUS_APPROVED,
            "case {case}: expected an accepted request at {CONNECTION_STATUS_APPROVED}, got {:?}",
            accepted["ConnectionStatus"]
        );
        assert!(
            accepted["ConnectionStatus"]["Message"].is_null(),
            "case {case}: an accepted request has nothing to add to its status, got {:?}",
            accepted["ConnectionStatus"]["Message"]
        );
        assert_eq!(
            accepted["LocalDomainInfo"], row.expected_local,
            "case {case}: expected the source reference {:?}, got {:?}",
            row.expected_local, accepted["LocalDomainInfo"]
        );
        assert_eq!(
            accepted["RemoteDomainInfo"],
            domain_info("acc-destination", Some(ACCOUNT), Some(REGION)),
            "case {case}: expected the destination reference resolved in full, got {:?}",
            accepted["RemoteDomainInfo"]
        );
        assert_eq!(
            accepted["ConnectionMode"],
            json!(row.expected_mode),
            "case {case}: expected ConnectionMode {:?}, got {:?}",
            row.expected_mode,
            accepted["ConnectionMode"]
        );
        for member in UNMODELLED_MEMBERS {
            assert!(
                accepted[member].is_null(),
                "case {case}: InboundConnection models no {member}, got {:?}",
                accepted[member]
            );
        }

        // Read back through the two reports of the SAME record: the acceptance is
        // persisted, and both owners see it.
        assert_eq!(
            inbound_status(&emulator, case, id).await.as_deref(),
            Some(CONNECTION_STATUS_APPROVED),
            "case {case}: the destination owner's report must carry the accepted connection"
        );
        assert_eq!(
            outbound_status(&emulator, case, id).await,
            CONNECTION_STATUS_APPROVED,
            "case {case}: the source owner's report must carry the same decision"
        );
    }

    // Putting an accepted connection in place is work the service does: each record
    // walks its lifecycle to the active connection on the clock alone.
    for (row, id) in table.iter().zip(created.iter()) {
        walks_to_active(&emulator, row.case, id).await;
        assert_eq!(
            outbound_status(&emulator, row.case, id).await,
            CONNECTION_STATUS_ACTIVE,
            "case {}: the two views of one connection must agree it is in place",
            row.case
        );
    }
}

#[tokio::test]
async fn an_id_addressing_no_inbound_connection_is_refused_as_missing() {
    let emulator = Emulator::new();
    created_domain(&emulator, "miss-source").await;
    created_domain(&emulator, "miss-destination").await;

    // A request still being validated, accepted before its validation falls due: the
    // destination owner has nothing to decide about it yet.
    let validating = created_connection(
        &emulator,
        "a request still being validated",
        json!({
            "ConnectionAlias": "miss-validating",
            "LocalDomainInfo": domain_info("miss-source", None, None),
            "RemoteDomainInfo": domain_info("miss-destination", None, None),
        }),
    )
    .await;

    // A request whose validation failed, because its destination reference resolves to
    // no domain this service holds: it never reaches the destination owner either.
    let failed = created_connection(
        &emulator,
        "a request whose validation failed",
        json!({
            "ConnectionAlias": "miss-failed",
            "LocalDomainInfo": domain_info("miss-source", None, None),
            "RemoteDomainInfo": domain_info("miss-absent", None, None),
        }),
    )
    .await;

    // The ids below are well formed — `ConnectionId` is `@length(min: 10, max: 256)`
    // with pattern `^[a-z][a-z0-9\-]+$` — so each reaches the handler rather than the
    // request boundary. The last is at the published minimum length.
    let unminted = ["cc-nosuchconnection", "cc-000000000000", "cc-1234567"];
    let mut table: Vec<(String, String, Vec<&str>)> = unminted
        .iter()
        .map(|id| {
            (
                format!("an id this service never minted ({id})"),
                (*id).to_owned(),
                Vec::new(),
            )
        })
        .collect();
    table.push((
        "a request still being validated".to_owned(),
        validating.clone(),
        // The refusal changes nothing, so the record is wherever the clock has it: the
        // status it was created in, or the one its validation owes it.
        vec![
            CONNECTION_STATUS_VALIDATING,
            CONNECTION_STATUS_PENDING_ACCEPTANCE,
        ],
    ));
    table.push((
        "a request whose validation failed".to_owned(),
        failed.clone(),
        vec![
            CONNECTION_STATUS_VALIDATING,
            CONNECTION_STATUS_VALIDATION_FAILED,
        ],
    ));

    for (case, id, unchanged) in &table {
        let answer = accept(&emulator, id).await;
        assert_eq!(
            answer.status, RECORDED_NOT_FOUND_STATUS,
            "case {case}: expected {RECORDED_NOT_FOUND_STATUS}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {:?}",
            answer.body
        );
        assert_eq!(
            error_message(&answer),
            format!("Connection ID {id} does not exist"),
            "case {case}: expected the refusal to name the id that resolved to nothing, got {:?}",
            answer.body
        );
        if !unchanged.is_empty() {
            let status = outbound_status(&emulator, case, id).await;
            assert!(
                unchanged.contains(&status.as_str()),
                "case {case}: a refused accept changes nothing, expected one of {unchanged:?}, \
                 got {status:?}"
            );
        }
    }

    // The refusal is about the STATUS, not about the id: once the clock has validated
    // the same request, the very same id is accepted.
    tokio::time::sleep(SETTLE_WAIT).await;
    let answer = accept(&emulator, &validating).await;
    assert_eq!(
        answer.status, 200,
        "case a validated request under the same id: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        status_code(&answer.body["Connection"], "a validated request under the same id"),
        CONNECTION_STATUS_APPROVED,
        "case a validated request under the same id: expected the acceptance to take, got {:?}",
        answer.body
    );

    // A request whose validation failed has no decision waiting however long the
    // caller leaves it.
    let answer = accept(&emulator, &failed).await;
    assert_eq!(
        error_code(&answer),
        "ResourceNotFoundException",
        "case a settled failed validation: expected ResourceNotFoundException, got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn a_connection_not_waiting_for_a_decision_cannot_be_accepted() {
    let emulator = Emulator::new();
    created_domain(&emulator, "twice-source").await;
    created_domain(&emulator, "twice-destination").await;

    let id = created_connection(
        &emulator,
        "accepted twice",
        json!({
            "ConnectionAlias": "accept-twice",
            "LocalDomainInfo": domain_info("twice-source", None, None),
            "RemoteDomainInfo": domain_info("twice-destination", None, None),
        }),
    )
    .await;
    tokio::time::sleep(SETTLE_WAIT).await;

    let first = accept(&emulator, &id).await;
    assert_eq!(
        first.status, 200,
        "case the first accept: expected 200, got {} with {:?}",
        first.status, first.body
    );

    // Every status an accepted connection goes on to hold is one it cannot be accepted
    // from: straight after the decision, and once the connection is in place.
    let stages = [
        ("straight after the decision", false),
        ("once the connection is in place", true),
    ];
    for (case, after_active) in stages {
        if after_active {
            walks_to_active(&emulator, case, &id).await;
        }
        let before = outbound_status(&emulator, case, &id).await;

        let answer = accept(&emulator, &id).await;
        assert_eq!(
            answer.status, 409,
            "case {case}: expected 409, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "DisabledOperationException",
            "case {case}: expected DisabledOperationException, got {:?}",
            answer.body
        );
        // The refusal names the status the connection is actually in. Which step of the
        // lifecycle that is depends on the clock, so every status the record could hold
        // by then is allowed — and none of them is the one acceptance needs.
        let allowed: Vec<String> = ACCEPTED_CHAIN
            .iter()
            .map(|status| {
                format!("Connection {id} is in {status} state, not {CONNECTION_STATUS_PENDING_ACCEPTANCE}")
            })
            .collect();
        let message = error_message(&answer);
        assert!(
            allowed.contains(&message),
            "case {case}: expected the refusal to name the status the connection is in, one of \
             {allowed:?}, got {message:?}"
        );

        // A refused decision is not a decision: the record is where the clock left it,
        // never back at the start of the accepted lifecycle.
        let after = outbound_status(&emulator, case, &id).await;
        let was = ACCEPTED_CHAIN.iter().position(|step| *step == before);
        let is = ACCEPTED_CHAIN.iter().position(|step| *step == after);
        assert!(
            is >= was,
            "case {case}: a refused accept never moves the connection back, it was {before:?} and \
             is {after:?}"
        );
    }
}
