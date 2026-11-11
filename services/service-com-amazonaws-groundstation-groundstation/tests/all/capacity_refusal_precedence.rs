// SPDX-License-Identifier: CC0-1.0

//! Where a CAPACITY refusal sits among the gates of a write, end to end over HTTP.
//!
//! The RULE under test: a refusal that reports a capacity the stored state has already
//! reached — this account's store at its bound, one contact at its maximum number of
//! versions — is judged only AFTER every gate that judges the request itself (the
//! references it names, the parameters it carries and the placement it asks for), so a
//! request nothing could make succeed is answered with what is wrong with IT rather than
//! with a limit error, which says the opposite: that the request was fine and the account
//! is full. A write at its capacity answers the limit error only for a request that is
//! otherwise acceptable.
//!
//! The suites assert that rule over SETS of inputs rather than over one call:
//!
//! * every gate `ReserveContact` applies to a request — the mission-profile reference, the
//!   window, the program-track reference, the satellite reference, the ground station and
//!   the antenna placement — asserted on a contact store BELOW its bound and again on one
//!   AT it, so each row is shown to answer the same way either way;
//! * the same for `UpdateContact`, whose capacity is how many versions one contact may
//!   hold;
//! * the other writes of this crate that model a capacity refusal — `CreateConfig`,
//!   `CreateMissionProfile` and `RegisterAgent` — each on a store at its bound, with a
//!   fault of its own kind;
//! * in every suite, a well-formed request on the full store, which IS what the capacity
//!   refusal answers.
//!
//! Every request goes through the real front door — the router, deserialization,
//! constraint validation, the handler and serialization — on the harness
//! `scheduling_loop.rs` stands up.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    self, MAX_CONTACT_VERSIONS, MAX_RECORDS_PER_STORE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::agent_lifecycle::capability_arn;
use crate::contact_satellite_reference_integrity::{absent_satellite_arns, onboard_satellite_arns};
use crate::ephemeris_catalog::program_track_ephemeris;
use crate::scheduling_loop::{
    create_mission_profile, create_tracking_config, harness_with_engine, is_uuid, send,
    stock_ground_stations, Reply, ABSENT_CONFIG_ARN, ABSENT_MISSION_PROFILE_ARN, ABSENT_UUID,
    STATION,
};

/// The `minimumViableContactDurationSeconds` every mission profile here is created with:
/// short enough that [`WINDOW_SECONDS`] clears it, long enough that a one-minute window
/// does not.
const MINIMUM_SECONDS: i64 = 180;

/// How long a window this suite books is.
const WINDOW_SECONDS: i64 = 600;

/// The spacing between one booked window and the next: wider than [`WINDOW_SECONDS`], so
/// no two of them share an instant and each is placed on the station's single antenna.
const SPACING_SECONDS: i64 = 3_600;

/// The first window this suite books; each filler reservation books one window further on.
/// In the future, because a contact reserves antenna time that has not elapsed yet.
const FILLER_START: i64 = 1_900_000_000;

/// The window a request UNDER TEST books, past every window the fillers hold.
const TEST_START: i64 = FILLER_START + (MAX_RECORDS_PER_STORE as i64 + 1) * SPACING_SECONDS;

/// A window whose antenna time the clock has long since passed.
const ELAPSED_START: i64 = 1_000_000_000;

/// A `GroundStationName` (len 4..97, pattern `^[ a-zA-Z0-9-._:=]{4,97}$` —
/// `smithy_query(query="operation", operation="ReserveContact")`) naming no ground station
/// this account is served by.
const ABSENT_STATION: &str = "Nowhere-1";

/// One request a capacity refusal must not speak for: what is wrong with it, the body it is
/// sent as, and the answer the service owes it.
///
/// `pub(crate)` here and on [`assert_owed`]: the sibling `store_bound_limit_error.rs` suite
/// holds the creates it spans to the same rows and the same assertions from this one
/// harness rather than keeping a second copy of it.
pub(crate) struct Row {
    pub(crate) case: String,
    pub(crate) body: Value,
    pub(crate) status: u16,
    pub(crate) error: &'static str,
    /// The text the answer must carry, for a refusal this crate states in ONE place (a
    /// constant or a message helper of `state`); `None` where the row pins the status and
    /// the error shape only, those being what this suite is about.
    pub(crate) message: Option<String>,
}

/// How many contacts the store holds — what a refused reservation may not have grown.
fn stored_contacts(engine: &engine::InMemoryEngine) -> usize {
    state::service_state(engine)
        .expect("this service's state store")
        .lock()
        .contacts
        .len()
}

/// How many versions the store holds for `contact_id` — what a refused update may not have
/// grown.
fn stored_versions(engine: &engine::InMemoryEngine, contact_id: &str) -> usize {
    state::service_state(engine)
        .expect("this service's state store")
        .lock()
        .contacts
        .get(contact_id)
        .unwrap_or_else(|| panic!("the store holds no contact {contact_id}"))
        .versions
        .len()
}

/// How many configs the store holds.
fn stored_configs(engine: &engine::InMemoryEngine) -> usize {
    state::service_state(engine)
        .expect("this service's state store")
        .lock()
        .configs
        .len()
}

/// How many mission profiles the store holds.
fn stored_mission_profiles(engine: &engine::InMemoryEngine) -> usize {
    state::service_state(engine)
        .expect("this service's state store")
        .lock()
        .mission_profiles
        .len()
}

/// How many agents the store holds. Read from the store because this model declares no
/// listing of agents at all.
fn stored_agents(engine: &engine::InMemoryEngine) -> usize {
    state::service_state(engine)
        .expect("this service's state store")
        .lock()
        .agents
        .len()
}

/// Assert `reply` is the answer `row` is owed, under the label `stage`.
pub(crate) fn assert_owed(stage: &str, row: &Row, reply: &Reply) {
    let case = &row.case;
    assert_eq!(
        reply.status, row.status,
        "case {stage} / {case}: expected status {}, got {} {}",
        row.status, reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        row.error,
        "case {stage} / {case}: expected the {} shape, got {}",
        row.error,
        reply.body
    );
    if let Some(message) = &row.message {
        assert_eq!(
            &reply.string("message"),
            message,
            "case {stage} / {case}: expected the message {message:?}, got {}",
            reply.body
        );
    }
}

/// A `ReserveContact` body for the window `(start, end)` at `station`.
fn reservation(profile_arn: &str, station: &str, start: i64, end: i64) -> Value {
    json!({
        "missionProfileArn": profile_arn,
        "groundStation": station,
        "startTime": start,
        "endTime": end,
    })
}

/// `TrackingOverrides` selecting the `azEl` program track for `ephemeris_id`.
fn az_el_overrides(ephemeris_id: &str) -> Value {
    json!({"programTrackSettings": {"azEl": {"ephemerisId": ephemeris_id}}})
}

/// `ReserveContact`, answering the reply.
async fn reserve(router: &MultiServiceRouter, body: Value) -> Reply {
    send(router, "POST", "/contact", Some(body)).await
}

/// One row per gate `ReserveContact` applies to the request itself, each tripping exactly
/// one of them: the mission-profile reference, the two window rules, the elapsed-window
/// rule, the program-track reference in both its forms, every `satelliteArn` of the absent
/// catalogue, the ground station and the antenna placement.
///
/// `start` is the window the rows that are otherwise placeable book, and `held_start` the
/// window a standing reservation already holds the station's only antenna for.
fn reservation_rows(
    profile_arn: &str,
    absent_satellites: &[(String, String, String)],
    wrong_form_ephemeris: &str,
    start: i64,
    held_start: i64,
) -> Vec<Row> {
    let end = start + WINDOW_SECONDS;
    let mut rows = vec![
        Row {
            case: "a missionProfileArn naming no mission profile of this account".to_string(),
            body: reservation(ABSENT_MISSION_PROFILE_ARN, STATION, start, end),
            status: 434,
            error: "ResourceNotFoundException",
            message: Some(state::MISSION_PROFILE_NOT_FOUND.to_string()),
        },
        Row {
            case: "a window that runs backwards".to_string(),
            body: reservation(profile_arn, STATION, end, start),
            status: 431,
            error: "InvalidParameterException",
            message: None,
        },
        Row {
            case: "a window shorter than the profile's minimum viable duration".to_string(),
            body: reservation(profile_arn, STATION, start, start + MINIMUM_SECONDS - 1),
            status: 431,
            error: "InvalidParameterException",
            message: None,
        },
        Row {
            case: "a window whose antenna time has already elapsed".to_string(),
            body: reservation(
                profile_arn,
                STATION,
                ELAPSED_START,
                ELAPSED_START + WINDOW_SECONDS,
            ),
            status: 431,
            error: "InvalidParameterException",
            message: None,
        },
        Row {
            case: "a program track naming an ephemeris this account does not hold".to_string(),
            body: {
                let mut body = reservation(profile_arn, STATION, start, end);
                body["trackingOverrides"] = az_el_overrides(ABSENT_UUID);
                body
            },
            status: 434,
            error: "ResourceNotFoundException",
            message: Some(state::EPHEMERIS_NOT_FOUND.to_string()),
        },
        Row {
            case: "a program track naming an ephemeris of another form".to_string(),
            body: {
                let mut body = reservation(profile_arn, STATION, start, end);
                body["trackingOverrides"] = az_el_overrides(wrong_form_ephemeris);
                body
            },
            status: 431,
            error: "InvalidParameterException",
            message: Some(state::wrong_ephemeris_type_message(
                state::EPHEMERIS_TYPE_AZ_EL,
            )),
        },
        Row {
            case: "a groundStation this account cannot address".to_string(),
            body: reservation(profile_arn, ABSENT_STATION, start, end),
            status: 431,
            error: "InvalidParameterException",
            message: Some(state::GROUND_STATION_INVALID.to_string()),
        },
        Row {
            case: "a window every antenna at the station is already held for".to_string(),
            body: reservation(
                profile_arn,
                STATION,
                held_start,
                held_start + WINDOW_SECONDS,
            ),
            status: 431,
            error: "InvalidParameterException",
            message: None,
        },
    ];
    // Every `satelliteArn` of the shared catalogue of ARNs naming no onboarding of this
    // account, so the row cannot be passing on one ARN's shape.
    for (case, arn, _) in absent_satellites {
        let mut body = reservation(profile_arn, STATION, start, end);
        body["satelliteArn"] = json!(arn);
        rows.push(Row {
            case: format!("a satelliteArn naming no onboarding: {case}"),
            body,
            status: 434,
            error: "ResourceNotFoundException",
            message: Some(state::satellite_reference_not_found_message(arn)),
        });
    }
    rows
}

/// Reserve windows of their own until the contact store is at its bound.
async fn fill_contact_store(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    profile_arn: &str,
) {
    let mut slot = 1_i64;
    while stored_contacts(engine) < MAX_RECORDS_PER_STORE {
        let start = FILLER_START + slot * SPACING_SECONDS;
        let reply = reserve(
            router,
            reservation(profile_arn, STATION, start, start + WINDOW_SECONDS),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case filler-{slot}: expected ReserveContact 200, got {} {}",
            reply.status, reply.body
        );
        slot += 1;
        assert!(
            slot <= MAX_RECORDS_PER_STORE as i64 + 1,
            "case filler-{slot}: filling the store took more reservations than the bound of \
             {MAX_RECORDS_PER_STORE} allows"
        );
    }
}

#[tokio::test]
async fn a_full_contact_store_still_answers_what_is_wrong_with_the_reservation() {
    let (router, engine) = harness_with_engine();
    stock_ground_stations(&engine, STATION);
    let profile_arn =
        create_mission_profile(&router, "capacity precedence", MINIMUM_SECONDS, None, None).await;
    let onboarded = onboard_satellite_arns(&engine, 1)[0].clone();
    let (_, onboarded_id) = onboarded
        .rsplit_once('/')
        .expect("a satelliteArn ends in the id of the satellite it names");
    let absent_satellites = absent_satellite_arns(onboarded_id);
    // An ephemeris of a form the `azEl` program track does not take, so the reference rule's
    // bad-parameter fault is reachable as well as its missing-resource one.
    let wrong_form = program_track_ephemeris(&router, "tle").await;

    // One standing reservation, so the station's single antenna is held for a window the
    // placement gate can trip on at either stage.
    let held = reserve(
        &router,
        reservation(
            &profile_arn,
            STATION,
            FILLER_START,
            FILLER_START + WINDOW_SECONDS,
        ),
    )
    .await;
    assert_eq!(
        held.status, 200,
        "case the held window: expected ReserveContact 200, got {} {}",
        held.status, held.body
    );

    // The SAME rows on a store below its bound and on one at it: every row's answer is
    // about the request, so the store's fullness may not change any of them. Each stage
    // books a window of its own, so the accepted request of the first stage holds no
    // antenna time the second stage's needs.
    for (stage_index, stage) in ["below the bound", "at the bound"].into_iter().enumerate() {
        if stage_index > 0 {
            fill_contact_store(&router, &engine, &profile_arn).await;
            assert_eq!(
                stored_contacts(&engine),
                MAX_RECORDS_PER_STORE,
                "case {stage}: the store must be at its bound before the rows run"
            );
        }
        let start = TEST_START + stage_index as i64 * SPACING_SECONDS;

        for row in reservation_rows(
            &profile_arn,
            &absent_satellites,
            &wrong_form,
            start,
            FILLER_START,
        ) {
            let before = stored_contacts(&engine);
            let reply = reserve(&router, row.body.clone()).await;
            assert_owed(stage, &row, &reply);
            assert_eq!(
                stored_contacts(&engine),
                before,
                "case {stage} / {}: a refused reservation stores no contact, got {} on record",
                row.case,
                stored_contacts(&engine)
            );
        }

        // The otherwise acceptable request: reserved while the store has room, and refused
        // with the account-limit error — and ONLY then — once it is full.
        let before = stored_contacts(&engine);
        let reply = reserve(
            &router,
            reservation(&profile_arn, STATION, start, start + WINDOW_SECONDS),
        )
        .await;
        if stage_index == 0 {
            assert_eq!(
                reply.status, 200,
                "case {stage} / a well-formed reservation: expected 200, got {} {}",
                reply.status, reply.body
            );
            let contact_id = reply.string("contactId");
            assert!(
                is_uuid(&contact_id),
                "case {stage} / a well-formed reservation: expected a modelled Uuid, got \
                 {contact_id:?}"
            );
            assert_eq!(
                stored_contacts(&engine),
                before + 1,
                "case {stage} / a well-formed reservation: the contact must be on record, got {} \
                 on record",
                stored_contacts(&engine)
            );
        } else {
            assert_owed(
                stage,
                &Row {
                    case: "a well-formed reservation".to_string(),
                    body: Value::Null,
                    status: 429,
                    error: "ResourceLimitExceededException",
                    message: Some(format!(
                        "this account already holds the maximum of {MAX_RECORDS_PER_STORE} contacts"
                    )),
                },
                &reply,
            );
            assert_eq!(
                stored_contacts(&engine),
                before,
                "case {stage} / a well-formed reservation: a refused reservation stores no \
                 contact, got {} on record",
                stored_contacts(&engine)
            );
        }
    }
}

/// `UpdateContact` as the model routes it: `POST /contact/{contactId}/versions`.
async fn update_contact(router: &MultiServiceRouter, contact_id: &str, body: Value) -> Reply {
    send(
        router,
        "POST",
        &format!("/contact/{contact_id}/versions"),
        Some(body),
    )
    .await
}

/// One row per gate `UpdateContact` applies to the request itself: a request naming neither
/// member it can change, the program-track reference in both its forms and every
/// `satelliteArn` of the absent catalogue.
fn update_rows(
    absent_satellites: &[(String, String, String)],
    wrong_form_ephemeris: &str,
) -> Vec<Row> {
    let mut rows = vec![
        Row {
            case: "a request naming neither member it could change".to_string(),
            body: json!({}),
            status: 431,
            error: "InvalidParameterException",
            message: None,
        },
        Row {
            case: "a program track naming an ephemeris this account does not hold".to_string(),
            body: json!({"trackingOverrides": az_el_overrides(ABSENT_UUID)}),
            status: 434,
            error: "ResourceNotFoundException",
            message: Some(state::EPHEMERIS_NOT_FOUND.to_string()),
        },
        Row {
            case: "a program track naming an ephemeris of another form".to_string(),
            body: json!({"trackingOverrides": az_el_overrides(wrong_form_ephemeris)}),
            status: 431,
            error: "InvalidParameterException",
            message: Some(state::wrong_ephemeris_type_message(
                state::EPHEMERIS_TYPE_AZ_EL,
            )),
        },
    ];
    for (case, arn, _) in absent_satellites {
        rows.push(Row {
            case: format!("a satelliteArn naming no onboarding: {case}"),
            body: json!({"satelliteArn": arn}),
            status: 434,
            error: "ResourceNotFoundException",
            message: Some(state::satellite_reference_not_found_message(arn)),
        });
    }
    rows
}

#[tokio::test]
async fn a_contact_at_its_version_ceiling_still_answers_what_is_wrong_with_the_update() {
    let (router, engine) = harness_with_engine();
    stock_ground_stations(&engine, STATION);
    let profile_arn =
        create_mission_profile(&router, "version precedence", MINIMUM_SECONDS, None, None).await;
    let satellites = onboard_satellite_arns(&engine, 2);
    let (_, onboarded_id) = satellites[0]
        .rsplit_once('/')
        .expect("a satelliteArn ends in the id of the satellite it names");
    let absent_satellites = absent_satellite_arns(onboarded_id);
    let wrong_form = program_track_ephemeris(&router, "tle").await;

    let reserved = reserve(
        &router,
        reservation(
            &profile_arn,
            STATION,
            FILLER_START,
            FILLER_START + WINDOW_SECONDS,
        ),
    )
    .await;
    assert_eq!(
        reserved.status, 200,
        "case the contact under test: expected ReserveContact 200, got {} {}",
        reserved.status, reserved.body
    );
    let contact_id = reserved.string("contactId");

    for (stage_index, stage) in ["below the ceiling", "at the ceiling"]
        .into_iter()
        .enumerate()
    {
        if stage_index > 0 {
            // Every version the modelled `VersionId` admits past the ones already appended,
            // each naming a satellite this account IS on-boarded to so the ceiling is what
            // the next update meets.
            while stored_versions(&engine, &contact_id) < MAX_CONTACT_VERSIONS as usize {
                let version = stored_versions(&engine, &contact_id);
                let satellite = &satellites[version % satellites.len()];
                let reply =
                    update_contact(&router, &contact_id, json!({"satelliteArn": satellite})).await;
                assert_eq!(
                    reply.status, 200,
                    "case filler version {version}: expected UpdateContact 200, got {} {}",
                    reply.status, reply.body
                );
            }
        }

        for row in update_rows(&absent_satellites, &wrong_form) {
            let before = stored_versions(&engine, &contact_id);
            let reply = update_contact(&router, &contact_id, row.body.clone()).await;
            assert_owed(stage, &row, &reply);
            assert_eq!(
                stored_versions(&engine, &contact_id),
                before,
                "case {stage} / {}: a refused update appends no version, got {} on record",
                row.case,
                stored_versions(&engine, &contact_id)
            );
        }

        let before = stored_versions(&engine, &contact_id);
        let reply = update_contact(
            &router,
            &contact_id,
            json!({"satelliteArn": satellites[0]}),
        )
        .await;
        if stage_index == 0 {
            assert_eq!(
                reply.status, 200,
                "case {stage} / a well-formed update: expected 200, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                stored_versions(&engine, &contact_id),
                before + 1,
                "case {stage} / a well-formed update: the version must be on record, got {} on \
                 record",
                stored_versions(&engine, &contact_id)
            );
        } else {
            assert_owed(
                stage,
                &Row {
                    case: "a well-formed update".to_string(),
                    body: Value::Null,
                    status: 429,
                    error: "ResourceLimitExceededException",
                    message: Some(format!(
                        "this contact already holds the maximum of {MAX_CONTACT_VERSIONS} versions"
                    )),
                },
                &reply,
            );
            assert_eq!(
                stored_versions(&engine, &contact_id),
                before,
                "case {stage} / a well-formed update: a refused update appends no version, got {} \
                 on record",
                stored_versions(&engine, &contact_id)
            );
        }
    }
}

#[tokio::test]
async fn a_full_config_store_still_answers_what_is_wrong_with_the_create() {
    let (router, engine) = harness_with_engine();

    // A tracking config to echo-reference by ARN — a config that EXISTS, of a capability the
    // echo reference does not take — and then as many more as the bound holds.
    let tracking_arn = create_tracking_config(&router, "echo source of the wrong type").await;
    let mut index = 0;
    while stored_configs(&engine) < MAX_RECORDS_PER_STORE {
        index += 1;
        create_tracking_config(&router, &format!("filler config {index}")).await;
        assert!(
            index <= MAX_RECORDS_PER_STORE,
            "case filler-{index}: filling the store took more creates than the bound of \
             {MAX_RECORDS_PER_STORE} allows"
        );
    }

    let rows = vec![
        Row {
            case: "an uplinkEchoConfig naming no config of this account".to_string(),
            body: json!({
                "name": "echo of nothing",
                "configData": {"uplinkEchoConfig": {
                    "enabled": true,
                    "antennaUplinkConfigArn": ABSENT_CONFIG_ARN,
                }},
            }),
            status: 434,
            error: "ResourceNotFoundException",
            message: Some(state::typed_reference_absent_message(
                state::UPLINK_ECHO_REFERENCED_TYPE,
                ABSENT_CONFIG_ARN,
            )),
        },
        Row {
            case: "an uplinkEchoConfig naming a config of another capability".to_string(),
            body: json!({
                "name": "echo of the wrong type",
                "configData": {"uplinkEchoConfig": {
                    "enabled": true,
                    "antennaUplinkConfigArn": tracking_arn,
                }},
            }),
            status: 431,
            error: "InvalidParameterException",
            message: Some(state::typed_reference_wrong_type_message(
                state::UPLINK_ECHO_REFERENCED_TYPE,
            )),
        },
    ];
    for row in rows {
        let before = stored_configs(&engine);
        let reply = send(&router, "POST", "/config", Some(row.body.clone())).await;
        assert_owed("at the bound", &row, &reply);
        assert_eq!(
            stored_configs(&engine),
            before,
            "case at the bound / {}: a refused create stores no config, got {} on record",
            row.case,
            stored_configs(&engine)
        );
    }

    let well_formed = Row {
        case: "a well-formed create".to_string(),
        body: json!({
            "name": "one config too many",
            "configData": {"trackingConfig": {"autotrack": "PREFERRED"}},
        }),
        status: 429,
        error: "ResourceLimitExceededException",
        message: Some(format!(
            "this account already holds the maximum of {MAX_RECORDS_PER_STORE} configs"
        )),
    };
    let reply = send(&router, "POST", "/config", Some(well_formed.body.clone())).await;
    assert_owed("at the bound", &well_formed, &reply);
}

#[tokio::test]
async fn a_full_mission_profile_store_still_answers_what_is_wrong_with_the_create() {
    let (router, engine) = harness_with_engine();
    let tracking_arn = create_tracking_config(&router, "profile bound tracking").await;

    let profile_body = |name: &str, config_arn: &str| {
        json!({
            "name": name,
            "minimumViableContactDurationSeconds": MINIMUM_SECONDS,
            "dataflowEdges": [],
            "trackingConfigArn": config_arn,
        })
    };
    let mut index = 0;
    while stored_mission_profiles(&engine) < MAX_RECORDS_PER_STORE {
        index += 1;
        let reply = send(
            &router,
            "POST",
            "/missionprofile",
            Some(profile_body(&format!("filler profile {index}"), &tracking_arn)),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case filler-{index}: expected CreateMissionProfile 200, got {} {}",
            reply.status, reply.body
        );
        assert!(
            index <= MAX_RECORDS_PER_STORE,
            "case filler-{index}: filling the store took more creates than the bound of \
             {MAX_RECORDS_PER_STORE} allows"
        );
    }

    let row = Row {
        case: "a trackingConfigArn naming no config of this account".to_string(),
        body: profile_body("profile of nothing", ABSENT_CONFIG_ARN),
        status: 434,
        error: "ResourceNotFoundException",
        message: Some(state::config_reference_not_found_message(
            ABSENT_CONFIG_ARN,
            Some("tracking"),
        )),
    };
    let before = stored_mission_profiles(&engine);
    let reply = send(&router, "POST", "/missionprofile", Some(row.body.clone())).await;
    assert_owed("at the bound", &row, &reply);
    assert_eq!(
        stored_mission_profiles(&engine),
        before,
        "case at the bound / {}: a refused create stores no mission profile, got {} on record",
        row.case,
        stored_mission_profiles(&engine)
    );

    let well_formed = Row {
        case: "a well-formed create".to_string(),
        body: profile_body("one profile too many", &tracking_arn),
        status: 429,
        error: "ResourceLimitExceededException",
        message: Some(format!(
            "this account already holds the maximum of {MAX_RECORDS_PER_STORE} mission profiles"
        )),
    };
    let reply = send(
        &router,
        "POST",
        "/missionprofile",
        Some(well_formed.body.clone()),
    )
    .await;
    assert_owed("at the bound", &well_formed, &reply);
}

/// A `RegisterAgent` body: the required members of `AgentDetails` and `DiscoveryData`
/// plus the cpu-core member the service requires one of
/// (`smithy_query(query="shape", shape="AgentDetails,DiscoveryData")`), where `row`
/// distinguishes one registration's EC2 instance from the next and `public` and `private`
/// are the address lists the discovery data carries.
fn registration(row: usize, public: &[String], private: &[String]) -> Value {
    json!({
        "agentDetails": {
            "agentVersion": "2.0.1",
            "instanceId": format!("i-{row:016x}"),
            "instanceType": "c5.xlarge",
            "agentCpuCores": [0, 1],
            "componentVersions": [{"componentType": "Agent", "versions": ["2.0.1"]}],
        },
        "discoveryData": {
            "publicIpAddresses": public,
            "privateIpAddresses": private,
            "capabilityArns": [capability_arn(row)],
        },
    })
}

/// The dotted-quad address of the `row`th registration.
fn address(prefix: &str, row: usize) -> String {
    format!("{prefix}.{}.{}", row / 256, row % 256)
}

#[tokio::test]
async fn a_full_agent_store_still_answers_what_is_wrong_with_the_registration() {
    let (router, engine) = harness_with_engine();

    let mut row = 0;
    while stored_agents(&engine) < MAX_RECORDS_PER_STORE {
        let body = registration(
            row,
            &[address("52.0", row)],
            &[address("10.0", row)],
        );
        let reply = send(&router, "POST", "/agent", Some(body)).await;
        assert_eq!(
            reply.status, 200,
            "case filler-{row}: expected RegisterAgent 200, got {} {}",
            reply.status, reply.body
        );
        row += 1;
        assert!(
            row <= MAX_RECORDS_PER_STORE,
            "case filler-{row}: filling the store took more registrations than the bound of \
             {MAX_RECORDS_PER_STORE} allows"
        );
    }

    // An address satisfying the modelled `IpV4Address` pattern that names no address at all,
    // in each discovery member that carries addresses: what is wrong is the value, so that
    // is what a registration at the bound is answered about.
    let not_an_address = "256.0.0.1".to_string();
    let rows = vec![
        Row {
            case: "a publicIpAddresses entry that is no IPv4 address".to_string(),
            body: registration(
                row,
                std::slice::from_ref(&not_an_address),
                &[address("10.0", row)],
            ),
            status: 431,
            error: "InvalidParameterException",
            message: None,
        },
        Row {
            case: "a privateIpAddresses entry that is no IPv4 address".to_string(),
            body: registration(row, &[address("52.0", row)], &[not_an_address]),
            status: 431,
            error: "InvalidParameterException",
            message: None,
        },
    ];
    for row_under_test in rows {
        let before = stored_agents(&engine);
        let reply = send(&router, "POST", "/agent", Some(row_under_test.body.clone())).await;
        assert_owed("at the bound", &row_under_test, &reply);
        assert_eq!(
            stored_agents(&engine),
            before,
            "case at the bound / {}: a refused registration stores no agent, got {} on record",
            row_under_test.case,
            stored_agents(&engine)
        );
    }

    let well_formed = Row {
        case: "a well-formed registration".to_string(),
        body: registration(row, &[address("52.0", row)], &[address("10.0", row)]),
        status: 429,
        error: "ResourceLimitExceededException",
        message: Some(format!(
            "this account already holds the maximum of {MAX_RECORDS_PER_STORE} registered agents"
        )),
    };
    let reply = send(&router, "POST", "/agent", Some(well_formed.body.clone())).await;
    assert_owed("at the bound", &well_formed, &reply);
}
