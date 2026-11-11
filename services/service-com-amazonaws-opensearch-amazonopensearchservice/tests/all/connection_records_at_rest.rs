// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a cross-cluster connection's id answers once the
//! connection has been resolved — deleted by one of its owners, or refused by the
//! destination owner.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`], the connection request helpers from
//! [`super::cross_cluster_connections`], the inbound report from
//! [`super::inbound_connections`] and the connection decisions from
//! [`super::connection_decisions`]; this suite adds no second copy of any of them.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * a resolved connection comes to REST and its id goes on addressing the record, so
//!   deleting a connection this account already deleted is an idempotent no-op that
//!   reports it at rest and does not put it back on its way out — over a table
//!   spanning both delete operations, the status the connection was deleted from, and
//!   which owner deleted it, with the delete repeated through both operations and
//!   twice over;
//! * an id this account never held is still the modelled missing-resource refusal, so
//!   the idempotence above is about a connection this caller deleted and not about
//!   every id — over a table of well-formed ids spanning `ConnectionId`'s own
//!   constraint;
//! * a request the destination owner refused is that owner's permanent record of the
//!   decision: deleting it is refused with the service's `ValidationException` naming
//!   the status it is in, the source owner's delete of its own view of the connection
//!   leaves that record where it is, and it stays there across repeated reads — over a
//!   table of refused requests differing in connection mode, properties and how fully
//!   the source reference names its account and Region.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    CONNECTION_MODE_DIRECT, CONNECTION_MODE_VPC_ENDPOINT, CONNECTION_STATUS_ACTIVE,
    CONNECTION_STATUS_DELETED, CONNECTION_STATUS_DELETING, CONNECTION_STATUS_PENDING_ACCEPTANCE,
    CONNECTION_STATUS_REJECTED,
};

use super::connection_decisions::{
    accept, connection_change_refusal, created_connection, delete_inbound, delete_outbound,
    error_message, inbound_status, outbound_status, pending_connection, reject, rests_at_deleted,
    status_code, walks_to_rejected, POLL_INTERVAL,
};
use super::cross_cluster_connections::{created_domain, domain_info, ACCOUNT, REGION};
use super::domain_lifecycle::{error_code, Answer, Emulator, RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT};

/// The two deletes of one connection, named so an assertion says which one answered.
const DELETES: [&str; 2] = ["DeleteOutboundConnection", "DeleteInboundConnection"];

/// Ask `operation` to delete the connection `id` names.
async fn delete_through(emulator: &Emulator, operation: &str, id: &str) -> Answer {
    if operation == "DeleteInboundConnection" {
        delete_inbound(emulator, id).await
    } else {
        delete_outbound(emulator, id).await
    }
}

/// One connection to delete and then delete again: what makes this row differ, the
/// alias to connect under, the status the connection is deleted FROM, and which
/// operation deletes it.
struct Row {
    case: &'static str,
    alias: &'static str,
    deleted_from: &'static str,
    deleted_through: &'static str,
}

/// The table: both delete operations, each deleting a connection that waits for the
/// destination owner's decision and one that is in use.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the source owner deletes a request waiting for a decision",
            alias: "idem-pending-outbound",
            deleted_from: CONNECTION_STATUS_PENDING_ACCEPTANCE,
            deleted_through: "DeleteOutboundConnection",
        },
        Row {
            case: "the destination owner deletes a request waiting for its decision",
            alias: "idem-pending-inbound",
            deleted_from: CONNECTION_STATUS_PENDING_ACCEPTANCE,
            deleted_through: "DeleteInboundConnection",
        },
        Row {
            case: "the source owner deletes a connection in use",
            alias: "idem-active-outbound",
            deleted_from: CONNECTION_STATUS_ACTIVE,
            deleted_through: "DeleteOutboundConnection",
        },
        Row {
            case: "the destination owner deletes a connection in use",
            alias: "idem-active-inbound",
            deleted_from: CONNECTION_STATUS_ACTIVE,
            deleted_through: "DeleteInboundConnection",
        },
    ]
}

/// Bring the connection `id` names to `deleted_from`, through the decision that
/// status needs and the clock that carries it out, and assert it is there.
async fn brought_to_status(emulator: &Emulator, case: &str, id: &str, deleted_from: &str) {
    if deleted_from == CONNECTION_STATUS_ACTIVE {
        let answer = accept(emulator, id).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the acceptance to be taken, got {} with {:?}",
            answer.status, answer.body
        );
        for _ in 0..60 {
            if outbound_status(emulator, case, id).await.as_deref()
                == Some(CONNECTION_STATUS_ACTIVE)
            {
                break;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
    assert_eq!(
        outbound_status(emulator, case, id).await.as_deref(),
        Some(deleted_from),
        "case {case}: the connection must be at {deleted_from} before it is deleted"
    );
}

#[tokio::test]
async fn deleting_a_connection_this_account_already_deleted_is_an_idempotent_no_op() {
    let emulator = Emulator::new();
    created_domain(&emulator, "idem-source").await;
    created_domain(&emulator, "idem-destination").await;

    for row in rows() {
        let case = row.case;
        let id =
            pending_connection(&emulator, case, row.alias, "idem-source", "idem-destination").await;
        brought_to_status(&emulator, case, &id, row.deleted_from).await;

        let first = delete_through(&emulator, row.deleted_through, &id).await;
        assert_eq!(
            first.status, 200,
            "case {case}: expected {} to answer 200, got {} with {:?}",
            row.deleted_through, first.status, first.body
        );
        assert_eq!(
            status_code(&first.body["Connection"], case),
            CONNECTION_STATUS_DELETING,
            "case {case}: expected the first delete to report {CONNECTION_STATUS_DELETING}, got \
             {:?}",
            first.body
        );

        // The deletion is carried out on the clock alone, and the record comes to rest
        // in the deleted status rather than going away.
        rests_at_deleted(&emulator, case, &id).await;

        // Repeated, through BOTH delete operations and twice over: each answers
        // success and reports the connection exactly as it stands. Nothing resurrects
        // it — a second delete that restarted the lifecycle would report
        // `DELETING` here, and a store that dropped the record would refuse the call.
        for round in 1..=2 {
            for operation in DELETES {
                let again = delete_through(&emulator, operation, &id).await;
                assert_eq!(
                    again.status, 200,
                    "case {case}: expected {operation} (round {round}) on the already-deleted \
                     connection to answer 200, got {} with {:?}",
                    again.status, again.body
                );
                assert!(
                    again.body["message"].is_null(),
                    "case {case}: expected {operation} (round {round}) to answer no error at all, \
                     got {:?}",
                    again.body
                );
                assert_eq!(
                    again.body["Connection"]["ConnectionId"],
                    json!(id),
                    "case {case}: expected {operation} (round {round}) to carry the connection \
                     {id:?}, got {:?}",
                    again.body
                );
                assert_eq!(
                    status_code(&again.body["Connection"], case),
                    CONNECTION_STATUS_DELETED,
                    "case {case}: expected {operation} (round {round}) to report the connection \
                     at {CONNECTION_STATUS_DELETED}, got {:?}",
                    again.body
                );
            }
            // And the reports agree with the answers: the repeat deletes left the
            // record where the first one did.
            for (view, status) in [
                ("DescribeOutboundConnections", outbound_status(&emulator, case, &id).await),
                ("DescribeInboundConnections", inbound_status(&emulator, case, &id).await),
            ] {
                assert_eq!(
                    status.as_deref(),
                    Some(CONNECTION_STATUS_DELETED),
                    "case {case}: expected {view} (round {round}) to report the connection at \
                     {CONNECTION_STATUS_DELETED}, got {status:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_id_this_account_never_held_is_refused_by_both_deletes() {
    let emulator = Emulator::new();

    // Well-formed ids addressing nothing — `ConnectionId` is `@length(min: 10, max:
    // 256)` with pattern `^[a-z][a-z0-9\-]+$`, so the rows span that constraint: one
    // at the published minimum length, one at the maximum, and two in between.
    let longest = format!("cc-{}", "z".repeat(253));
    let table: Vec<String> = vec![
        "cc-1234567".to_owned(),
        "cc-nosuchconnection".to_owned(),
        "zz-0000000000".to_owned(),
        longest,
    ];

    for id in &table {
        for operation in DELETES {
            let answer = delete_through(&emulator, operation, id).await;
            assert_eq!(
                answer.status, RECORDED_NOT_FOUND_STATUS,
                "case {operation}({id:?}): expected {RECORDED_NOT_FOUND_STATUS}, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                error_code(&answer),
                "ResourceNotFoundException",
                "case {operation}({id:?}): expected ResourceNotFoundException, got {:?}",
                answer.body
            );
            assert_eq!(
                error_message(&answer),
                format!("Connection ID {id} does not exist"),
                "case {operation}({id:?}): expected the refusal to name the id that resolved to \
                 nothing, got {:?}",
                answer.body
            );
        }
    }
}

/// One refused request: what makes this row differ, the alias to connect under, the
/// source domain and the reference sent for it, the `ConnectionMode` sent (absent
/// where `None`) and the `ConnectionProperties` sent.
struct RefusedRow {
    case: &'static str,
    alias: &'static str,
    source: &'static str,
    local: Value,
    mode: Option<&'static str>,
    properties: Option<Value>,
}

/// The table of refused requests: the mode omitted and named either way, no
/// properties / an endpoint / a cross-cluster-search property, and a source reference
/// naming its account and Region in full, partly or not at all.
fn refused_rows() -> Vec<RefusedRow> {
    vec![
        RefusedRow {
            case: "a source naming only the domain, and no mode",
            alias: "rest-reject-plain",
            source: "rest-source-a",
            local: domain_info("rest-source-a", None, None),
            mode: None,
            properties: None,
        },
        RefusedRow {
            case: "a source in another account and Region, mode named",
            alias: "rest-reject-cross-account",
            source: "rest-source-b",
            local: domain_info("rest-source-b", Some(ACCOUNT), Some(REGION)),
            mode: Some(CONNECTION_MODE_DIRECT),
            properties: Some(json!({ "CrossClusterSearch": { "SkipUnavailable": "ENABLED" } })),
        },
        RefusedRow {
            case: "the other connection mode, with the endpoint property",
            alias: "rest-reject-vpc",
            source: "rest-source-c",
            local: domain_info("rest-source-c", None, Some("ap-south-1")),
            mode: Some(CONNECTION_MODE_VPC_ENDPOINT),
            properties: Some(json!({ "Endpoint": "vpc-remote.eu-west-1.es.amazonaws.com" })),
        },
    ]
}

#[tokio::test]
async fn a_refused_request_stays_the_destination_owners_record_of_the_refusal() {
    let emulator = Emulator::new();
    created_domain(&emulator, "rest-destination").await;
    let table = refused_rows();
    for row in &table {
        created_domain(&emulator, row.source).await;
    }

    let mut created: Vec<String> = Vec::new();
    for row in &table {
        let mut body = json!({
            "ConnectionAlias": row.alias,
            "LocalDomainInfo": row.local,
            "RemoteDomainInfo": domain_info("rest-destination", None, None),
        });
        if let Some(mode) = row.mode {
            body["ConnectionMode"] = json!(mode);
        }
        if let Some(properties) = &row.properties {
            body["ConnectionProperties"] = properties.clone();
        }
        created.push(created_connection(&emulator, row.case, body).await);
    }

    // Nothing but the clock brings a validated request to the point of being decided.
    tokio::time::sleep(SETTLE_WAIT).await;
    for (row, id) in table.iter().zip(created.iter()) {
        let answer = reject(&emulator, id).await;
        assert_eq!(
            answer.status, 200,
            "case {}: expected the refusal to be taken, got {} with {:?}",
            row.case, answer.status, answer.body
        );
        walks_to_rejected(&emulator, row.case, id).await;
    }

    // The destination owner's refusal is its record of the decision: there is nothing
    // left for it to delete, and the refusal names the status that is the reason.
    for (row, id) in table.iter().zip(created.iter()) {
        let case = row.case;
        let refused = delete_inbound(&emulator, id).await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected DeleteInboundConnection on a refused request to answer 400, \
             got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case}: expected ValidationException, got {:?}",
            refused.body
        );
        assert_eq!(
            error_message(&refused),
            connection_change_refusal(id, CONNECTION_STATUS_REJECTED),
            "case {case}: expected the refusal to name the connection the request addressed and \
             the status it is in, got {:?}",
            refused.body
        );
        assert_eq!(
            inbound_status(&emulator, case, id).await.as_deref(),
            Some(CONNECTION_STATUS_REJECTED),
            "case {case}: a refused delete leaves the record where the refusal left it"
        );
    }

    // The SOURCE owner's delete of its own view of the connection is served, and it
    // is not the destination owner taking its decision back.
    for (row, id) in table.iter().zip(created.iter()) {
        let case = row.case;
        let answer = delete_outbound(&emulator, id).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected DeleteOutboundConnection to answer 200, got {} with {:?}",
            answer.status, answer.body
        );
        rests_at_deleted(&emulator, case, id).await;
    }

    // Three reads, a settle span apart: the destination owner's record of the refusal
    // does not move, and the connection is not deletable from that side however often
    // the call is made.
    for read in 1..=3 {
        for (row, id) in table.iter().zip(created.iter()) {
            let case = row.case;
            assert_eq!(
                inbound_status(&emulator, case, id).await.as_deref(),
                Some(CONNECTION_STATUS_REJECTED),
                "case {case}: expected read {read} of the destination owner's report to still \
                 carry the connection at {CONNECTION_STATUS_REJECTED}"
            );
            assert_eq!(
                outbound_status(&emulator, case, id).await.as_deref(),
                Some(CONNECTION_STATUS_DELETED),
                "case {case}: expected read {read} of the source owner's report to carry the \
                 connection at {CONNECTION_STATUS_DELETED}"
            );
            let refused = delete_inbound(&emulator, id).await;
            assert_eq!(
                error_code(&refused),
                "ValidationException",
                "case {case}: expected read {read}'s DeleteInboundConnection to answer \
                 ValidationException, got {:?}",
                refused.body
            );
            let again = delete_outbound(&emulator, id).await;
            assert_eq!(
                again.status, 200,
                "case {case}: expected read {read}'s DeleteOutboundConnection to answer 200, got \
                 {} with {:?}",
                again.status, again.body
            );
            assert_eq!(
                status_code(&again.body["Connection"], case),
                CONNECTION_STATUS_DELETED,
                "case {case}: expected read {read}'s repeat delete to report the connection at \
                 {CONNECTION_STATUS_DELETED}, got {:?}",
                again.body
            );
        }
        if read < 3 {
            tokio::time::sleep(SETTLE_WAIT).await;
        }
    }
}
