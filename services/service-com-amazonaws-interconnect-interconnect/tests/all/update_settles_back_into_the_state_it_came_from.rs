// SPDX-License-Identifier: CC0-1.0

//! Where a re-provisioning bandwidth change LEAVES a connection, driven over the wire
//! through the shared `crate::emulator` harness.
//!
//! The RULE under test: a bandwidth change hands the connection to the clock in the
//! transient state the model documents for a change in flight ("updating: the
//! connection is being updated") and the clock settles it back into the state the
//! change found it in — never further along the provisioning progression, because
//! entering the accepted state is what an acceptance does ("pending: the connection has
//! been accepted and is being provisioned between AWS and the Partner",
//! `smithy_query(query="doc",shape="Connection",member="state")`) and a bandwidth edit
//! accepts nothing. Once the connection is back in the state it came from, whatever
//! that state owes the clock is owed again from there.
//!
//! Every assertion is checked over a TABLE whose rows differ in the state the update is
//! applied from, in the pair of bandwidths the change moves between (up and down, and
//! across both the `Mbps` and the `Gbps` forms the catalog publishes), in the Region the
//! connection lives in and in the gateway it is attached to.

use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::emulator::{available_bandwidths, remote_account_for, text, Emulator};

/// The `ConnectionState` a new connection is returned in: "requested: The initial state
/// of a connection. The state will remain here until the Connection is accepted on the
/// Partner portal."
const INITIAL_STATE: &str = "requested";

/// The `ConnectionState` an accepted proposal's connection enters: "pending: The
/// connection has been accepted and is being provisioned between AWS and the Partner."
const ACCEPTED_STATE: &str = "pending";

/// The `ConnectionState` the model documents as the end of the progression: "available:
/// The connection has been fully provisioned between AWS and the Partner."
const PROVISIONED_STATE: &str = "available";

/// The `ConnectionState` a change in flight reports: "updating: The connection is being
/// updated."
const UPDATING_STATE: &str = "updating";

/// How long a poll waits for a state the service owes before the case fails — well past
/// every span this service owes a leg of, so a row proves prompt convergence rather than
/// eventual convergence.
const POLL_BUDGET: Duration = Duration::from_secs(5);

/// How often such a poll re-reads: short enough to observe the state the update settles
/// into before the clock takes the connection on from it.
const POLL_INTERVAL: Duration = Duration::from_millis(2);

/// Long enough for a connection updated while it was still awaiting acceptance to have
/// settled back and then been carried the whole way along the progression with nobody
/// polling (one provisioning span plus both activation legs, with margin).
const WHOLE_WALK_WAIT: Duration = Duration::from_millis(1_500);

/// The gateways the rows attach to: an attach point IS a Direct Connect gateway of the
/// account, so the suite creates its own and the rows differ in which one the connection
/// was created on, in each gateway's name and in its `amazonSideAsn`.
const GATEWAY_ROWS: [(&str, Option<i64>); 2] = [
    ("ic-settles-back-a", None),
    ("ic-settles-back-b", Some(64_513)),
];

/// The Regions the rows sign their requests for, so the rule holds across the Region a
/// connection belongs to as well as across the state and the bandwidths.
const REGIONS: [&str; 2] = ["us-east-1", "eu-west-1"];

/// The bandwidth pairs a row moves between — a change UP, a change DOWN, and one that
/// crosses the two unit forms the catalog publishes.
///
/// Both members of each pair are checked against the live
/// `Environment$bandwidths.available` ("the list of currently available bandwidths") of
/// the row's own Region before the row runs, so a pair the catalog stops offering fails
/// as a skipped row rather than as a refused update.
const BANDWIDTH_PAIRS: [(&str, &str); 3] = [
    ("1Gbps", "10Gbps"),
    ("20Gbps", "2Gbps"),
    ("500Mbps", "5Gbps"),
];

/// The states an update is permitted from, which are therefore the states a row can
/// apply its change in: the one a connection is created in and the fully provisioned
/// one.
const FROM_STATES: [&str; 2] = [INITIAL_STATE, PROVISIONED_STATE];

/// One row of the table.
#[derive(Debug, Clone)]
struct Row {
    region: &'static str,
    environment: Value,
    gateway: String,
    /// The state the change is applied in.
    from_state: &'static str,
    created_with: String,
    asked_for: String,
}

impl Row {
    fn environment_id(&self) -> String {
        text(&self.environment, "environmentId")
    }

    fn label(&self) -> String {
        format!(
            "{} / {} / {} -> {} applied while {}",
            self.region,
            self.environment_id(),
            self.created_with,
            self.asked_for,
            self.from_state
        )
    }
}

/// The table: every bandwidth pair the live catalog of its Region still offers, crossed
/// with the states an update is permitted from, with the Region and the gateway varying
/// by row.
async fn table(emulator: &Emulator, gateways: &[String]) -> Vec<Row> {
    let mut rows = Vec::new();
    for (seed, (created_with, asked_for)) in BANDWIDTH_PAIRS.iter().enumerate() {
        let region = REGIONS[seed % REGIONS.len()];
        let catalog = emulator.environments_in(region).await;
        for environment in catalog {
            let offered = available_bandwidths(&environment);
            if !offered.iter().any(|one| one == created_with)
                || !offered.iter().any(|one| one == asked_for)
            {
                continue;
            }
            for (offset, from_state) in FROM_STATES.iter().enumerate() {
                rows.push(Row {
                    region,
                    environment: environment.clone(),
                    gateway: gateways[(seed + offset) % gateways.len()].clone(),
                    from_state,
                    created_with: (*created_with).to_string(),
                    asked_for: (*asked_for).to_string(),
                });
            }
        }
    }
    rows
}

/// `CreateConnection` for one row, answering the stored `Connection`.
async fn create(emulator: &Emulator, row: &Row, description: &str) -> Value {
    let mut request = Map::new();
    request.insert("environmentId".to_string(), json!(row.environment_id()));
    request.insert("bandwidth".to_string(), json!(row.created_with));
    request.insert(
        "attachPoint".to_string(),
        json!({ "directConnectGateway": row.gateway }),
    );
    request.insert("description".to_string(), json!(description));
    if let Some(remote_account) = remote_account_for(&row.environment) {
        request.insert("remoteAccount".to_string(), remote_account);
    }
    let created = emulator
        .ic_ok_in(row.region, "CreateConnection", Value::Object(request))
        .await;
    created["connection"].clone()
}

/// `GetConnection` in the row's Region, answering the stored `Connection`.
async fn read_back(emulator: &Emulator, row: &Row, identifier: &str) -> Value {
    let fetched = emulator
        .ic_ok_in(row.region, "GetConnection", json!({ "identifier": identifier }))
        .await;
    fetched["connection"].clone()
}

/// Poll until the connection reports `wanted`, answering the record it reported it as.
async fn wait_for_state(emulator: &Emulator, row: &Row, identifier: &str, wanted: &str) -> Value {
    let case = row.label();
    let started = Instant::now();
    loop {
        let connection = read_back(emulator, row, identifier).await;
        let state = text(&connection, "state");
        if state == wanted {
            return connection;
        }
        assert!(
            started.elapsed() < POLL_BUDGET,
            "case {case:?}: expected the connection to reach {wanted:?} within {POLL_BUDGET:?}, it is still {state:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Poll until the connection reports anything OTHER than the transient update state,
/// answering the FIRST record it settled as — which is the state the change handed it
/// back to.
async fn wait_until_settled(emulator: &Emulator, row: &Row, identifier: &str) -> Value {
    let case = row.label();
    let started = Instant::now();
    loop {
        let connection = read_back(emulator, row, identifier).await;
        let state = text(&connection, "state");
        if state != UPDATING_STATE {
            return connection;
        }
        assert!(
            started.elapsed() < POLL_BUDGET,
            "case {case:?}: expected the change to settle within {POLL_BUDGET:?}, the connection is still {state:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Bring the row's connection to the state its change is applied in, answering its
/// identifier and the state it is actually in when the change is made.
async fn connection_in_from_state(emulator: &Emulator, row: &Row, description: &str) -> String {
    let case = row.label();
    let created = create(emulator, row, description).await;
    let identifier = text(&created, "id");
    if row.from_state == PROVISIONED_STATE {
        wait_for_state(emulator, row, &identifier, PROVISIONED_STATE).await;
    } else {
        assert_eq!(
            text(&created, "state"),
            INITIAL_STATE,
            "case {case:?}: expected a created connection to report {INITIAL_STATE:?}, got {:?}",
            created["state"]
        );
    }
    identifier
}

// ---------------------------------------------------------------------------
// Where the change leaves the connection
// ---------------------------------------------------------------------------

/// RULE: a bandwidth change reports the transient update state and the clock settles the
/// connection back into the state the change found it in — the state it came from, not
/// the next one the provisioning progression would have taken it to — carrying the new
/// bandwidth with it.
#[tokio::test]
async fn a_bandwidth_change_settles_back_into_the_state_it_came_from() {
    let emulator = Emulator::new();
    let gateways = emulator.ready_gateways(&GATEWAY_ROWS).await;
    let rows = table(&emulator, &gateways).await;
    assert!(
        rows.len() >= FROM_STATES.len() * BANDWIDTH_PAIRS.len(),
        "expected one row per bandwidth pair per permitted state, got {} rows",
        rows.len()
    );

    for row in &rows {
        let case = row.label();
        let identifier = connection_in_from_state(&emulator, row, "settles back where it came from")
            .await;

        // The state the change is actually applied in, read immediately before it.
        let before = read_back(&emulator, row, &identifier).await;
        let came_from = text(&before, "state");
        assert_eq!(
            came_from, row.from_state,
            "case {case:?}: expected the change to be applied while the connection was {:?}, got {came_from:?}",
            row.from_state
        );

        let changed = emulator
            .ic_ok_in(
                row.region,
                "UpdateConnection",
                json!({ "identifier": identifier, "bandwidth": row.asked_for }),
            )
            .await;
        let changed = changed["connection"].clone();
        assert_eq!(
            text(&changed, "state"),
            UPDATING_STATE,
            "case {case:?}: expected a bandwidth change to report {UPDATING_STATE:?}, got {:?}",
            changed["state"]
        );
        assert_eq!(
            text(&changed, "bandwidth"),
            row.asked_for,
            "case {case:?}: expected the change to report the new bandwidth, got {:?}",
            changed["bandwidth"]
        );

        // The settle target IS the state the change found it in.
        let settled = wait_until_settled(&emulator, row, &identifier).await;
        assert_eq!(
            text(&settled, "state"),
            came_from,
            "case {case:?}: expected the clock to settle the connection back into {came_from:?}, got {:?}",
            settled["state"]
        );
        // The round trip: the read reports the change the update made.
        assert_eq!(
            text(&settled, "bandwidth"),
            row.asked_for,
            "case {case:?}: expected the new bandwidth to survive settling, got {:?}",
            settled["bandwidth"]
        );
        assert_eq!(
            text(&settled, "description"),
            "settles back where it came from",
            "case {case:?}: expected the description the create set to survive the change, got {:?}",
            settled["description"]
        );
    }
}

/// RULE: a change that asks for no new bandwidth re-provisions nothing, so it moves no
/// state at all: the connection reports the state it was already in, whichever state
/// that was.
#[tokio::test]
async fn a_description_only_update_moves_no_state() {
    let emulator = Emulator::new();
    let gateways = emulator.ready_gateways(&GATEWAY_ROWS).await;
    let rows = table(&emulator, &gateways).await;

    for row in &rows {
        let case = row.label();
        let identifier = connection_in_from_state(&emulator, row, "renamed not re-provisioned").await;
        let came_from = text(&read_back(&emulator, row, &identifier).await, "state");

        let renamed = emulator
            .ic_ok_in(
                row.region,
                "UpdateConnection",
                json!({ "identifier": identifier, "description": "renamed in place" }),
            )
            .await;
        let renamed = renamed["connection"].clone();
        assert_eq!(
            text(&renamed, "state"),
            came_from,
            "case {case:?}: expected a description-only change to leave the state {came_from:?} alone, got {:?}",
            renamed["state"]
        );
        assert_eq!(
            text(&renamed, "description"),
            "renamed in place",
            "case {case:?}: expected the new description to be persisted, got {:?}",
            renamed["description"]
        );
        assert_eq!(
            text(&renamed, "bandwidth"),
            row.created_with,
            "case {case:?}: expected the bandwidth to be left alone, got {:?}",
            renamed["bandwidth"]
        );
    }
}

// ---------------------------------------------------------------------------
// What the restored state still owes, and what it still allows
// ---------------------------------------------------------------------------

/// RULE: a connection settled back into the state it was created in is owed the rest of
/// the progression from there, so it walks on to the fully provisioned state by itself —
/// the change costs it no leg and gains it none.
#[tokio::test]
async fn a_connection_updated_while_awaiting_acceptance_walks_on_from_where_it_came_back() {
    let emulator = Emulator::new();
    let gateways = emulator.ready_gateways(&GATEWAY_ROWS).await;
    let rows: Vec<Row> = table(&emulator, &gateways)
        .await
        .into_iter()
        .filter(|row| row.from_state == INITIAL_STATE)
        .collect();
    assert!(
        !rows.is_empty(),
        "expected at least one row whose change is applied while the connection awaits acceptance"
    );

    for row in &rows {
        let case = row.label();
        let identifier = connection_in_from_state(&emulator, row, "walks on after a change").await;
        emulator
            .ic_ok_in(
                row.region,
                "UpdateConnection",
                json!({ "identifier": identifier, "bandwidth": row.asked_for }),
            )
            .await;

        // Nobody polls: the whole walk is owed by the clock from the state the change
        // handed the connection back to.
        tokio::time::sleep(WHOLE_WALK_WAIT).await;
        let walked = read_back(&emulator, row, &identifier).await;
        assert_eq!(
            text(&walked, "state"),
            PROVISIONED_STATE,
            "case {case:?}: expected the connection to walk on to {PROVISIONED_STATE:?} with nobody polling, got {:?}",
            walked["state"]
        );
        assert_eq!(
            text(&walked, "bandwidth"),
            row.asked_for,
            "case {case:?}: expected the new bandwidth to survive the walk, got {:?}",
            walked["bandwidth"]
        );
    }
}

/// RULE: a bandwidth change accepts nothing, so the acceptance a connection still
/// awaiting one is owed stays the caller's to make right after the change: it is
/// answered, it applies the attach point it selects, and it is what puts the connection
/// into the accepted state.
#[tokio::test]
async fn a_change_leaves_an_unaccepted_proposal_acceptable() {
    let emulator = Emulator::new();
    let gateways = emulator.ready_gateways(&GATEWAY_ROWS).await;
    let rows: Vec<Row> = table(&emulator, &gateways)
        .await
        .into_iter()
        .filter(|row| row.from_state == INITIAL_STATE)
        .collect();
    assert!(
        !rows.is_empty(),
        "expected at least one row whose change is applied while the connection awaits acceptance"
    );

    for row in &rows {
        let case = row.label();
        let created = create(&emulator, row, "accepted after a change").await;
        let identifier = text(&created, "id");
        let activation_key = text(&created, "activationKey");
        // The acceptance selects the OTHER gateway, so it really has an attach point to
        // apply.
        let selected = gateways
            .iter()
            .find(|gateway| **gateway != row.gateway)
            .cloned()
            .unwrap_or_else(|| row.gateway.clone());

        emulator
            .ic_ok_in(
                row.region,
                "UpdateConnection",
                json!({ "identifier": identifier, "bandwidth": row.asked_for }),
            )
            .await;

        let (status, body) = emulator
            .ic_in(
                row.region,
                "AcceptConnectionProposal",
                json!({
                    "activationKey": activation_key,
                    "attachPoint": { "directConnectGateway": selected },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the acceptance to still be the caller's to make after a bandwidth change, got {status} with {body:?}"
        );
        let accepted = body["connection"].clone();
        assert_eq!(
            text(&accepted, "state"),
            ACCEPTED_STATE,
            "case {case:?}: expected the acceptance to be what enters {ACCEPTED_STATE:?}, got {:?}",
            accepted["state"]
        );
        assert_eq!(
            accepted["attachPoint"],
            json!({ "directConnectGateway": selected }),
            "case {case:?}: expected the acceptance to apply the attach point it selected, got {:?}",
            accepted["attachPoint"]
        );
        assert_eq!(
            text(&accepted, "bandwidth"),
            row.asked_for,
            "case {case:?}: expected the accepted connection to hold the changed bandwidth, got {:?}",
            accepted["bandwidth"]
        );
    }
}
