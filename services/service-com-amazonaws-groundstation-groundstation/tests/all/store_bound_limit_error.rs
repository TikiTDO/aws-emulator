// SPDX-License-Identifier: CC0-1.0

//! What a create is answered when the store it would grow already holds the maximum
//! number of that resource, end to end over HTTP.
//!
//! The RULE under test: a create refused because this account already holds the maximum
//! number of a resource is refused on a limit of what the ACCOUNT holds — the request is
//! valid, the resources it names exist and nothing the service depends on failed — so it
//! is answered with the client-side limit error this service models for that operation
//! (`ResourceLimitExceededException`, HTTP 429, "Account limits for this resource have
//! been exceeded"; the `ServiceQuotaExceededException`, HTTP 402, the V2
//! dataflow-endpoint-group create models), never with a server-side fault; and that limit
//! is judged only AFTER the gates that judge the request itself.
//!
//! The suites assert that rule over SETS of inputs rather than over one call:
//!
//! * every well-formed `CreateEphemeris` body a caller may send against a full ephemeris
//!   store — each member of `EphemerisData` (`tle`, `oem`, `azEl`) supplied inline and
//!   from an Amazon S3 object, each published bound of `CustomerEphemerisPriority`, both
//!   values of `enabled`, an expiration the request names and one left to the data's own
//!   validity, a satellite this account is on-boarded to, and tags — so no row rides on
//!   the shape of one request;
//! * the same full store answering a request that is wrong about ITSELF about that
//!   instead, and storing nothing either way;
//! * one store two creates share, each answering with the limit error ITS operation
//!   models.
//!
//! Every request goes through the real front door — the router, deserialization,
//! constraint validation, the handler and serialization — on the harness
//! `scheduling_loop.rs` stands up, and each row and each assertion is the
//! `capacity_refusal_precedence.rs` harness's own `Row`/`assert_owed` rather than a second
//! copy of them: that suite owns where a capacity refusal sits among the gates of a write,
//! this one owns WHICH error it is.

use serde_json::{json, Map, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    satellite_not_found_message, service_state, MAX_RECORDS_PER_STORE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::capacity_refusal_precedence::{assert_owed, Row};
use crate::dataflow_endpoint_groups::create_group;
use crate::dataflow_endpoint_groups_v2::{create_group_v2, uplink};
use crate::ephemeris_catalog::{az_el_inline, onboard, s3_object, tle_inline, FUTURE, OEM_BODY};
use crate::scheduling_loop::{harness_with_engine, send, Reply, ABSENT_UUID};

/// The validity range every row's supplied data carries, well ahead of the clock.
const VALID_FROM: i64 = 1_800_000_000;
/// See [`VALID_FROM`].
const VALID_TO: i64 = 1_800_086_400;

/// The published bounds of `CustomerEphemerisPriority` (range 1..99999 —
/// `smithy_query(query="operation", operation="CreateEphemeris")`).
const PRIORITY_MIN: i64 = 1;
/// See [`PRIORITY_MIN`].
const PRIORITY_MAX: i64 = 99_999;

/// The port and the agent-side port range every dataflow endpoint of this suite is
/// addressed at, well inside the modelled bounds.
const PORT: i32 = 55888;
/// See [`PORT`].
const RANGE: (i32, i32) = (55000, 55100);

/// How many ephemerides the store holds — what a refused create may not have grown.
fn stored_ephemerides(engine: &engine::InMemoryEngine) -> usize {
    service_state(engine)
        .expect("this service's state store")
        .lock()
        .ephemerides
        .len()
}

/// How many dataflow endpoint groups the store holds.
fn stored_groups(engine: &engine::InMemoryEngine) -> usize {
    service_state(engine)
        .expect("this service's state store")
        .lock()
        .dataflow_endpoint_groups
        .len()
}

/// The message every refusal of a create on a full ephemeris store carries.
fn ephemeris_limit_message() -> String {
    format!("this account already holds the maximum of {MAX_RECORDS_PER_STORE} ephemerides")
}

/// The message every refusal of a create on a full dataflow-endpoint-group store carries.
fn group_limit_message() -> String {
    format!(
        "this account already holds the maximum of {MAX_RECORDS_PER_STORE} dataflow endpoint \
         groups"
    )
}

/// A `CreateEphemeris` body named `name`, carrying `data` plus the members of `extra`.
fn ephemeris_body(name: &str, data: Value, extra: &Value) -> Value {
    let mut body = Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("ephemeris".to_string(), data);
    for (member, value) in extra.as_object().into_iter().flatten() {
        body.insert(member.clone(), value.clone());
    }
    Value::Object(body)
}

/// `CreateEphemeris` with `body`.
async fn create_ephemeris(router: &MultiServiceRouter, body: Value) -> Reply {
    send(router, "POST", "/ephemeris", Some(body)).await
}

/// Each form of ephemeris data a request may carry: every member of the `EphemerisData`
/// union (`smithy_query(query="shape", shape="EphemerisData")` — `tle`, `oem`, `azEl`),
/// supplied inline and from an Amazon S3 object.
fn data_forms() -> Vec<(&'static str, Value)> {
    vec![
        ("tle inline", tle_inline(&[(VALID_FROM, VALID_TO)])),
        (
            "tle from an Amazon S3 object",
            json!({"tle": {"s3Object": s3_object("tle/aqua.txt")}}),
        ),
        ("oem inline", json!({"oem": {"oemData": OEM_BODY}})),
        (
            "oem from an Amazon S3 object",
            json!({"oem": {"s3Object": s3_object("oem/aqua.oem")}}),
        ),
        ("azEl inline", az_el_inline(&[(VALID_FROM, VALID_TO)])),
        (
            "azEl from an Amazon S3 object",
            json!({"azEl": {
                "groundStation": "Ohio 1",
                "data": {"s3Object": s3_object("azel/ohio.json")},
            }}),
        ),
    ]
}

/// Every well-formed create this suite sends against the full store: one per form of
/// supplied data, and one per optional member a request may vary besides the data, each
/// owed the account-limit answer.
///
/// `satellite_id` is a satellite this account IS on-boarded to, so the row carrying it
/// trips no reference gate.
fn well_formed_rows(satellite_id: &str) -> Vec<Row> {
    let mut rows: Vec<Row> = data_forms()
        .into_iter()
        .map(|(case, data)| Row {
            case: format!("a well-formed create of {case}"),
            body: ephemeris_body(&format!("one too many {case}"), data, &json!({})),
            status: 429,
            error: "ResourceLimitExceededException",
            message: Some(ephemeris_limit_message()),
        })
        .collect();
    // The optional members a request may vary besides the data: each published bound of
    // `CustomerEphemerisPriority`, both values of `enabled`, an expiration the request
    // names (the forms above name none and are expired off their own data's validity), a
    // satellite this account holds, and tags.
    let varied: Vec<(&str, Value)> = vec![
        (
            "priority at the modelled minimum",
            json!({"priority": PRIORITY_MIN}),
        ),
        (
            "priority at the modelled maximum",
            json!({"priority": PRIORITY_MAX}),
        ),
        ("enabled", json!({"enabled": true})),
        ("disabled", json!({"enabled": false})),
        ("an expirationTime of its own", json!({"expirationTime": FUTURE})),
        (
            "a satelliteId this account is on-boarded to",
            json!({"satelliteId": satellite_id}),
        ),
        ("tags", json!({"tags": {"mission": "aqua"}})),
    ];
    for (case, extra) in varied {
        rows.push(Row {
            case: format!("a well-formed create with {case}"),
            body: ephemeris_body(
                &format!("one too many with {case}"),
                tle_inline(&[(VALID_FROM, VALID_TO)]),
                &extra,
            ),
            status: 429,
            error: "ResourceLimitExceededException",
            message: Some(ephemeris_limit_message()),
        });
    }
    rows
}

/// Create ephemerides of their own until the store is at its bound.
async fn fill_ephemeris_store(router: &MultiServiceRouter, engine: &engine::InMemoryEngine) {
    let mut index = 0;
    while stored_ephemerides(engine) < MAX_RECORDS_PER_STORE {
        index += 1;
        let reply = create_ephemeris(
            router,
            ephemeris_body(
                &format!("filler ephemeris {index}"),
                tle_inline(&[(VALID_FROM, VALID_TO)]),
                &json!({"expirationTime": FUTURE}),
            ),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case filler-{index}: expected CreateEphemeris 200, got {} {}",
            reply.status, reply.body
        );
        assert!(
            index <= MAX_RECORDS_PER_STORE,
            "case filler-{index}: filling the store took more creates than the bound of \
             {MAX_RECORDS_PER_STORE} allows"
        );
    }
}

#[tokio::test]
async fn a_full_ephemeris_store_answers_every_well_formed_create_with_the_account_limit_error() {
    let (router, engine) = harness_with_engine();
    let satellites = onboard(&engine, "25994=Ohio 1,27424");
    let satellite_id = satellites
        .first()
        .expect("the onboarding seeds at least one satellite")
        .clone();

    fill_ephemeris_store(&router, &engine).await;

    for row in well_formed_rows(&satellite_id) {
        let before = stored_ephemerides(&engine);
        let reply = create_ephemeris(&router, row.body.clone()).await;
        assert_owed("at the bound", &row, &reply);
        assert_eq!(
            stored_ephemerides(&engine),
            before,
            "case at the bound / {}: a refused create stores no ephemeris, got {} on record",
            row.case,
            stored_ephemerides(&engine)
        );
    }
}

#[tokio::test]
async fn a_full_ephemeris_store_still_answers_what_is_wrong_with_the_create() {
    let (router, engine) = harness_with_engine();
    onboard(&engine, "25994=Ohio 1,27424");

    fill_ephemeris_store(&router, &engine).await;

    // Each row is wrong about the request ITSELF — it names no data, names two sources of
    // it, names none, or references a satellite this account is not on-boarded to — so the
    // full store does not speak for it.
    let rows = vec![
        Row {
            case: "a create naming no ephemeris data at all".to_string(),
            body: json!({"name": "no data"}),
            status: 431,
            error: "InvalidParameterException",
            message: None,
        },
        Row {
            case: "tle data supplied inline AND from an Amazon S3 object".to_string(),
            body: ephemeris_body(
                "two sources",
                json!({"tle": {
                    "tleData": [{
                        "tleLine1": crate::ephemeris_catalog::TLE_LINE_1,
                        "tleLine2": crate::ephemeris_catalog::TLE_LINE_2,
                        "validTimeRange": {"startTime": VALID_FROM, "endTime": VALID_TO},
                    }],
                    "s3Object": s3_object("tle/aqua.txt"),
                }}),
                &json!({}),
            ),
            status: 431,
            error: "InvalidParameterException",
            message: None,
        },
        Row {
            case: "oem data supplied neither inline nor from an Amazon S3 object".to_string(),
            body: ephemeris_body("no source", json!({"oem": {}}), &json!({})),
            status: 431,
            error: "InvalidParameterException",
            message: None,
        },
        Row {
            case: "a satelliteId naming no onboarding of this account".to_string(),
            body: ephemeris_body(
                "absent satellite",
                tle_inline(&[(VALID_FROM, VALID_TO)]),
                &json!({"satelliteId": ABSENT_UUID}),
            ),
            status: 434,
            error: "ResourceNotFoundException",
            message: Some(satellite_not_found_message(ABSENT_UUID)),
        },
    ];

    for row in rows {
        let before = stored_ephemerides(&engine);
        let reply = create_ephemeris(&router, row.body.clone()).await;
        assert_owed("at the bound", &row, &reply);
        assert_eq!(
            stored_ephemerides(&engine),
            before,
            "case at the bound / {}: a refused create stores no ephemeris, got {} on record",
            row.case,
            stored_ephemerides(&engine)
        );
    }
}

#[tokio::test]
async fn a_full_group_store_answers_each_create_with_the_limit_error_its_operation_models() {
    let (router, engine) = harness_with_engine();

    let mut index = 0;
    while stored_groups(&engine) < MAX_RECORDS_PER_STORE {
        index += 1;
        let reply = create_group(
            &router,
            json!({"endpointDetails": [uplink(&format!("filler-{index}"), PORT, RANGE)]}),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case filler-{index}: expected CreateDataflowEndpointGroup 200, got {} {}",
            reply.status, reply.body
        );
        assert!(
            index <= MAX_RECORDS_PER_STORE,
            "case filler-{index}: filling the store took more creates than the bound of \
             {MAX_RECORDS_PER_STORE} allows"
        );
    }

    // Both creates write the one store of this account's dataflow endpoint groups, and
    // each answers the limit error ITS operation models: 429 for the one that models
    // `ResourceLimitExceededException`, 402 for the V2 create that models
    // `ServiceQuotaExceededException` instead.
    let v1 = Row {
        case: "a well-formed CreateDataflowEndpointGroup".to_string(),
        body: json!({"endpointDetails": [uplink("one group too many", PORT, RANGE)]}),
        status: 429,
        error: "ResourceLimitExceededException",
        message: Some(group_limit_message()),
    };
    let before = stored_groups(&engine);
    let reply = create_group(&router, v1.body.clone()).await;
    assert_owed("at the bound", &v1, &reply);

    let v2 = Row {
        case: "a well-formed CreateDataflowEndpointGroupV2".to_string(),
        body: json!({"endpoints": [uplink("one V2 group too many", PORT, RANGE)]}),
        status: 402,
        error: "ServiceQuotaExceededException",
        message: Some(group_limit_message()),
    };
    let reply = create_group_v2(&router, v2.body.clone()).await;
    assert_owed("at the bound", &v2, &reply);

    assert_eq!(
        stored_groups(&engine),
        before,
        "case at the bound / a full group store: a refused create stores no group, got {} on \
         record",
        stored_groups(&engine)
    );
}
