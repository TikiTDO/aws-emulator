// SPDX-License-Identifier: CC0-1.0

//! A request refused because the connector is in the wrong state is worded by the
//! service's rule, over EVERY state the model enumerates.
//!
//! The rule pinned here: a connector that is not `ACTIVE` refuses an update with the
//! modelled 409, naming the connector by the `Id` the service assigned it and the state it
//! is in spelled as the name of that state; a connector in a state it cannot be deleted
//! from refuses the delete with the sentence that states that state and names no
//! identifier. Neither refusal changes the resource.
//!
//! The sibling suite `connector_identity_and_conflicts.rs` asserts the same rule over the
//! two states a caller can drive a connector into through this API alone (`PENDING` and
//! `DELETING`). This suite spans the WHOLE class the rule generalises over: every
//! non-`ACTIVE` value of `NetworkConnectorState` (`PENDING`, `INACTIVE`, `FAILED`,
//! `DELETING`, `DELETE_FAILED` — the enum's values, less `ACTIVE`), including the three
//! only the service itself puts a connector in, each one addressed by each of the three
//! identifier forms `NetworkConnectorIdentifier` admits, and with the connector named by a
//! different `Name` shape in each row — among them a `Name` spelled exactly like an id the
//! service mints, which an implementation that names the connector by its `Name` cannot
//! pass.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_lambdacore_lambdacoreapiservice::state::service_state;

use crate::network_connector_lifecycle::{
    assert_conflict, at, create, delete, emulator, emulator_with_engine, get, minimal_create, text,
    update, vpc_egress, wait_for_the_clock,
};

/// What the create's configuration carries, so a refused update can be shown to have
/// changed nothing.
const CREATED_SUBNET: &str = "subnet-0123456789abcdef0";

/// The subnet a refused update asks for: never the one the create stored.
const REQUESTED_SUBNET: &str = "subnet-0fedcba9876543210";

/// One row of the state class: the state the connector is in when the request arrives, how
/// the service spells that state's name in the update refusal, the `Name` the connector
/// carries, and whether a delete is refused in that state.
///
/// The states are the values of `NetworkConnectorState` (PENDING, ACTIVE, INACTIVE,
/// FAILED, DELETING, DELETE_FAILED) less `ACTIVE`, which accepts the update. A delete is
/// refused in every state except the ones the operation accepts: "you can delete
/// connectors in ACTIVE or FAILED states", and it is idempotent for one that is already
/// deleting.
struct Row {
    state: &'static str,
    spelled: &'static str,
    name: &'static str,
    delete_is_refused: bool,
}

/// The whole non-`ACTIVE` state class, one row each.
fn rows() -> Vec<Row> {
    vec![
        Row {
            state: "PENDING",
            spelled: "Pending",
            name: "pending-connector",
            delete_is_refused: true,
        },
        Row {
            // A `Name` the model admits (`[a-zA-Z0-9_-]{1,64}`) that is spelled like an id
            // the service mints: the refusal must still carry the Id of the record.
            state: "INACTIVE",
            spelled: "Inactive",
            name: "nc-00000000000000ff",
            delete_is_refused: true,
        },
        Row {
            state: "FAILED",
            spelled: "Failed",
            name: "f",
            delete_is_refused: false,
        },
        Row {
            state: "DELETING",
            spelled: "Deleting",
            name: "1234567890",
            delete_is_refused: false,
        },
        Row {
            // The one state whose name is two words, so a renderer that only lowercases
            // or only uppercases fails this row.
            state: "DELETE_FAILED",
            spelled: "DeleteFailed",
            name: "delete_failed-connector_09",
            delete_is_refused: true,
        },
    ]
}

/// Put the connector named `name` in `state`, with nothing owed to it by the clock, so the
/// request under test arrives at a connector the service itself could have left in that
/// state.
fn force_state(engine: &Arc<engine::InMemoryEngine>, name: &str, state: &str) {
    let store = service_state(&**engine).expect("the shared engine holds this service's store");
    let mut store = store.lock();
    let connector = store
        .connectors
        .get_mut(name)
        .unwrap_or_else(|| panic!("the create stored a connector named {name:?}"));
    connector.state = state.to_string();
    connector.pending = None;
}

/// RULE: in every non-`ACTIVE` state, and whichever identifier form the request used, the
/// update is refused with the service's sentence naming the connector's `Id` and spelling
/// the state's name; the delete is refused, in a state it cannot be deleted from, with the
/// sentence that states the state's wire value and names nothing else.
#[tokio::test]
async fn a_state_gated_refusal_is_worded_by_the_state_not_by_the_request() {
    for row in rows() {
        for form in ["the name", "the id", "the unqualified ARN"] {
            let (router, engine) = emulator_with_engine();
            let created = create(&router, minimal_create(row.name)).await;
            assert_eq!(
                created.status, 202,
                "case {:?}/{form}: the create must succeed, got {} with {:?}",
                row.state, created.status, created.body
            );
            let id = text(&created.body, "/Id");
            let arn = text(&created.body, "/Arn");
            assert_ne!(
                id, row.name,
                "case {:?}/{form}: the row needs an Id the service minted, distinct from \
                 the Name, for the refusal's identifier to be decidable",
                row.state
            );
            force_state(&engine, row.name, row.state);

            let identifier = match form {
                "the id" => id.clone(),
                "the unqualified ARN" => arn.clone(),
                _ => row.name.to_string(),
            };
            let case = format!("{} addressed by {form}", row.state);

            let refused = update(
                &router,
                &identifier,
                json!({ "Configuration": vpc_egress(&[REQUESTED_SUBNET], &[], None, None) }),
            )
            .await;
            assert_conflict(
                &format!("an update while {case}"),
                &refused,
                &format!(
                    "Network connector {id} is in state {} and cannot be updated. Only \
                     ACTIVE connectors can be updated.",
                    row.spelled
                ),
            );
            let message = text(&refused.body, "/message");
            assert!(
                message.starts_with(&format!("Network connector {id} is in state ")),
                "case {case:?}: the refusal names the connector by the Id the service \
                 assigned it, got {message:?}"
            );
            // A one-character Name is a substring of too much to be decisive, so the rows
            // that carry a distinctive Name are the ones that prove the Name is absent.
            assert!(
                row.name.len() < 3 || !message.contains(row.name),
                "case {case:?}: the refusal must not name the connector by the Name the \
                 caller chose, got {message:?}"
            );
            assert!(
                !message.contains(row.state),
                "case {case:?}: the refusal spells the state's name rather than its wire \
                 value, got {message:?}"
            );

            // The refusal changed nothing: the connector is still in the same state and
            // still holds the configuration the create stored.
            let fetched = get(&router, &identifier).await;
            assert_eq!(
                text(&fetched.body, "/State"),
                row.state,
                "case {case:?}: a refused update must not move the connector, got {:?}",
                at(&fetched.body, "/State")
            );
            assert_eq!(
                at(&fetched.body, "/Configuration/VpcEgressConfiguration/SubnetIds"),
                json!([CREATED_SUBNET]),
                "case {case:?}: a refused update must not be applied, got {:?}",
                at(&fetched.body, "/Configuration/VpcEgressConfiguration/SubnetIds")
            );

            let deleted = delete(&router, &identifier).await;
            if row.delete_is_refused {
                assert_conflict(
                    &format!("a delete while {case}"),
                    &deleted,
                    &format!(
                        "Network connector is in {} state and cannot be deleted",
                        row.state
                    ),
                );
                let after = get(&router, &identifier).await;
                assert_eq!(
                    text(&after.body, "/State"),
                    row.state,
                    "case {case:?}: a refused delete must not move the connector, got {:?}",
                    at(&after.body, "/State")
                );
            } else {
                assert_eq!(
                    deleted.status, 202,
                    "case {case:?}: a connector in this state is deletable, got {} with {:?}",
                    deleted.status, deleted.body
                );
            }
        }
    }
}

/// The same gate is only about the STATE: the very update a connector refuses while it is
/// not `ACTIVE` is accepted once the clock owes it `ACTIVE`, and the new configuration is
/// what the next read reports.
///
/// The rows differ along how many subnets the accepted update carries, so the row that
/// succeeds is not one particular request.
#[tokio::test]
async fn the_same_update_is_accepted_once_the_connector_is_active() {
    let rows: Vec<(&str, Vec<&str>)> = vec![
        ("one subnet", vec![REQUESTED_SUBNET]),
        (
            "two subnets",
            vec![REQUESTED_SUBNET, "subnet-00112233445566778"],
        ),
    ];

    for (case, subnets) in rows {
        let router = emulator();
        let created = create(&router, minimal_create("settles")).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: the create must succeed, got {} with {:?}",
            created.status, created.body
        );
        let id = text(&created.body, "/Id");
        let request = json!({ "Configuration": vpc_egress(&subnets, &[], None, None) });

        let refused = update(&router, &id, request.clone()).await;
        assert_conflict(
            &format!("{case} while the create is still pending"),
            &refused,
            &format!(
                "Network connector {id} is in state Pending and cannot be updated. Only \
                 ACTIVE connectors can be updated."
            ),
        );

        wait_for_the_clock();
        let accepted = update(&router, &id, request).await;
        assert_eq!(
            accepted.status, 202,
            "case {case:?}: an ACTIVE connector accepts the same update, got {} with {:?}",
            accepted.status, accepted.body
        );
        assert_eq!(
            text(&accepted.body, "/State"),
            "ACTIVE",
            "case {case:?}: the answer reports the state the record is in, got {:?}",
            at(&accepted.body, "/State")
        );

        let fetched: Value = get(&router, &id).await.body;
        assert_eq!(
            at(&fetched, "/Configuration/VpcEgressConfiguration/SubnetIds"),
            json!(subnets),
            "case {case:?}: the accepted update is what the next read reports, got {:?}",
            at(&fetched, "/Configuration/VpcEgressConfiguration/SubnetIds")
        );
    }
}
