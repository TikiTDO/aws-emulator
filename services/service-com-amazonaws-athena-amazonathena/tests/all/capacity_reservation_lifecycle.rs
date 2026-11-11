// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for Athena's capacity-reservation slice: `CreateCapacityReservation`
//! registers a reservation, `GetCapacityReservation` reads it back, and
//! `UpdateCapacityReservation`, `CancelCapacityReservation` and
//! `DeleteCapacityReservation` move it along its lifecycle — all driven over the wire
//! (awsJson1_1, `x-amz-target: AmazonAthena.<Operation>`) through the same router the
//! emulator boots, so every assertion is about what an SDK client observes.
//!
//! Each test states a rule and asserts it over a TABLE of inputs rather than one recorded
//! request: the rows differ along whatever the rule generalises over — the requested number
//! of data processing units, the name's length and character set, the size of a tag list,
//! which operation is called, and how much time has passed since the call that started a
//! transition.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_athena_amazonathena::state::{
    self, CapacityReservationRecord, CANCELLED_CAPACITY_RESERVATION_RETENTION,
    CAPACITY_ALLOCATION, CAPACITY_RESERVATION_CANCELLATION, CAPACITY_RESERVATION_STATUS_ACTIVE,
    CAPACITY_RESERVATION_STATUS_CANCELLED, CAPACITY_RESERVATION_STATUS_CANCELLING,
    CAPACITY_RESERVATION_STATUS_PENDING, CAPACITY_RESERVATION_STATUS_UPDATE_PENDING,
    MAX_TAGS_PER_RESOURCE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, engine_and_router, error_code, message, router};

/// The wire error code every refusal of this slice must carry: the client error each of
/// its operations models (HTTP 400).
const INVALID_REQUEST: &str = "InvalidRequestException";

/// The smallest number of data processing units a request may ask for: the floor of the
/// modelled `TargetDpusInteger` range (4..), which the boundary validator enforces.
const MODELLED_FLOOR_TARGET_DPUS: i32 = 4;

/// The amount every test that is not about the amount itself asks for: inside the modelled
/// range and above its floor, so no row says anything about the floor by accident.
const SOME_TARGET_DPUS: i32 = 24;

/// A little past a span, so a test that waits for a transition the clock owes is not
/// racing the instant it is due at.
const PAST_DUE: Duration = Duration::from_millis(150);

/// The five operations of this slice that address ONE reservation by `Name`.
const OPERATIONS: [&str; 5] = [
    "CreateCapacityReservation",
    "GetCapacityReservation",
    "UpdateCapacityReservation",
    "CancelCapacityReservation",
    "DeleteCapacityReservation",
];

/// A valid reservation name of `length` characters, all of them inside the modelled
/// `CapacityReservationName` pattern (`^[a-zA-Z0-9._-]+$`).
fn name_of_length(length: usize) -> String {
    "aA0._-".chars().cycle().take(length).collect()
}

/// `GetCapacityReservation`'s view of one reservation: `(status_code, CapacityReservation)`.
async fn read_reservation(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    let (status, body) = call(router, "GetCapacityReservation", &json!({ "Name": name })).await;
    (status, body["CapacityReservation"].clone())
}

/// The `Status` the reservation `name` reports, or the refusal's error code.
async fn status_of(router: &MultiServiceRouter, name: &str) -> String {
    let (status, reservation) = read_reservation(router, name).await;
    if status != 200 {
        return format!("<{status}>");
    }
    reservation["Status"]
        .as_str()
        .unwrap_or("<no Status>")
        .to_string()
}

/// Create one reservation and assert it was accepted.
async fn create(router: &MultiServiceRouter, name: &str, target_dpus: i32) {
    let (status, body) = call(
        router,
        "CreateCapacityReservation",
        &json!({ "Name": name, "TargetDpus": target_dpus }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateCapacityReservation({target_dpus}) must be accepted, got \
         {status} with {body:?}"
    );
}

/// Wait out a span a transition the clock owes is due after.
fn wait_out(span: Duration) {
    std::thread::sleep(span + PAST_DUE);
}

/// One row of the create/read-back table.
struct CreateCase {
    case: &'static str,
    name: String,
    target_dpus: i32,
    /// The `Tags` member the create carries (`Value::Null` = absent), and the (key, value)
    /// pairs the stored record must then hold.
    tags: Value,
    stored_tags: Vec<(&'static str, &'static str)>,
}

/// The create table: the requested amount at the documented minimum and well above it, the
/// name at both ends of the modelled length, and a tag list of none, one and many.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            case: "the smallest amount the model admits, no tags",
            name: "modelled-floor".to_string(),
            target_dpus: MODELLED_FLOOR_TARGET_DPUS,
            tags: Value::Null,
            stored_tags: Vec::new(),
        },
        CreateCase {
            case: "an amount well above that floor",
            name: "above-the-floor".to_string(),
            target_dpus: SOME_TARGET_DPUS,
            tags: Value::Null,
            stored_tags: Vec::new(),
        },
        CreateCase {
            case: "a one-character name with one tag",
            name: name_of_length(1),
            target_dpus: 48,
            tags: json!([{ "Key": "team", "Value": "analytics" }]),
            stored_tags: vec![("team", "analytics")],
        },
        CreateCase {
            case: "the longest name the model admits, with two tags",
            name: name_of_length(128),
            target_dpus: 240,
            tags: json!([
                { "Key": "team", "Value": "analytics" },
                { "Key": "cost-centre", "Value": "4711" },
            ]),
            stored_tags: vec![("team", "analytics"), ("cost-centre", "4711")],
        },
        CreateCase {
            case: "a large requested amount with the most tags one resource holds",
            name: "large.reservation_1".to_string(),
            target_dpus: 10_000,
            tags: Value::Array(
                (0..MAX_TAGS_PER_RESOURCE)
                    .map(|index| json!({ "Key": format!("key-{index}"), "Value": "v" }))
                    .collect(),
            ),
            stored_tags: Vec::new(),
        },
    ]
}

/// A created reservation is the one a later read reports: the requested amount comes back
/// unchanged, with no data processing units allocated yet and the allocation of that amount
/// already requested.
#[tokio::test]
async fn a_created_reservation_is_read_back_by_get_capacity_reservation() {
    let (engine, router) = engine_and_router();

    for case in create_cases() {
        let CreateCase {
            case,
            name,
            target_dpus,
            tags,
            stored_tags,
        } = case;
        let mut request = json!({ "Name": name, "TargetDpus": target_dpus });
        if !tags.is_null() {
            request["Tags"] = tags;
        }
        let (status, created) = call(&router, "CreateCapacityReservation", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateCapacityReservation to answer 200, got {status} \
             with {created:?}"
        );

        let (status, read) = read_reservation(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetCapacityReservation to answer 200, got {status} \
             with {read:?}"
        );
        assert_eq!(
            read["Name"].as_str(),
            Some(name.as_str()),
            "case {case:?}: the read must report the name the create registered, got {read:?}"
        );
        assert_eq!(
            read["TargetDpus"].as_i64(),
            Some(i64::from(target_dpus)),
            "case {case:?}: the read must report the requested {target_dpus} data \
             processing units, got {read:?}"
        );
        assert_eq!(
            read["AllocatedDpus"].as_i64(),
            Some(0),
            "case {case:?}: a reservation that was just created has no data processing \
             units allocated, got {read:?}"
        );
        assert_eq!(
            read["Status"].as_str(),
            Some(CAPACITY_RESERVATION_STATUS_PENDING),
            "case {case:?}: a caller reading straight back must see \
             {CAPACITY_RESERVATION_STATUS_PENDING}, got {read:?}"
        );
        assert!(
            read["CreationTime"].as_f64().is_some(),
            "case {case:?}: the read must report a CreationTime, got {read:?}"
        );
        assert_eq!(
            read["LastAllocation"]["Status"].as_str(),
            Some("PENDING"),
            "case {case:?}: the allocation of the requested amount is in flight, got {read:?}"
        );
        assert_eq!(
            read["LastAllocation"]["RequestTime"].as_f64(),
            read["CreationTime"].as_f64(),
            "case {case:?}: the create is when that allocation was requested, got {read:?}"
        );
        assert!(
            read["LastAllocation"]["RequestCompletionTime"].is_null(),
            "case {case:?}: an allocation still in flight has not completed, got {read:?}"
        );
        assert!(
            read["LastSuccessfulAllocationTime"].is_null(),
            "case {case:?}: no allocation has succeeded yet, got {read:?}"
        );

        // `ListTagsForResource` is still a scaffold, so the tag list is asserted against
        // the record the write persisted rather than over the wire.
        if !stored_tags.is_empty() {
            let store = state::service_state(&*engine).expect("this service's state");
            let persisted: Vec<(String, String)> = store
                .lock()
                .capacity_reservations
                .reservation(&name, SystemTime::now())
                .expect("the reservation the create registered")
                .tags
                .iter()
                .map(|tag| {
                    (
                        tag.key.clone().unwrap_or_default(),
                        tag.value.clone().unwrap_or_default(),
                    )
                })
                .collect();
            let expected: Vec<(String, String)> = stored_tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect();
            assert_eq!(
                persisted, expected,
                "case {case:?}: the tags the create carried must be persisted in order"
            );
        }
    }
}

/// Every requested amount the modelled `TargetDpusInteger` range admits is accepted and
/// round-trips; one below its floor is refused, on the create and the update alike, and
/// changes nothing.
#[tokio::test]
async fn the_requested_amount_is_bounded_by_the_modelled_floor() {
    let (engine, router) = engine_and_router();

    // The reservation the update rows address, allocated so an update is not refused for
    // its status instead of its amount.
    create(&router, "updatable", SOME_TARGET_DPUS).await;
    wait_out(CAPACITY_ALLOCATION);

    // (case, TargetDpus, whether the request is inside the modelled range)
    let amounts: [(&str, i32, bool); 7] = [
        ("a negative amount", -4, false),
        ("no capacity at all", 0, false),
        ("one below the modelled floor", MODELLED_FLOOR_TARGET_DPUS - 1, false),
        ("the modelled floor", MODELLED_FLOOR_TARGET_DPUS, true),
        ("one above the modelled floor", MODELLED_FLOOR_TARGET_DPUS + 1, true),
        ("an amount well above the floor", SOME_TARGET_DPUS, true),
        ("a far larger amount", 10_000, true),
    ];

    for (case, target_dpus, inside) in amounts {
        let name = format!("floor{target_dpus}");
        let (status, body) = call(
            &router,
            "CreateCapacityReservation",
            &json!({ "Name": name, "TargetDpus": target_dpus }),
        )
        .await;
        if inside {
            assert_eq!(
                status, 200,
                "case {case:?}: {target_dpus} data processing units must be accepted, got \
                 {status} with {body:?}"
            );
            let (_, read) = read_reservation(&router, &name).await;
            assert_eq!(
                read["TargetDpus"].as_i64(),
                Some(i64::from(target_dpus)),
                "case {case:?}: the amount must round-trip through the create, got {read:?}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: {target_dpus} data processing units must be refused, got \
             {status} with {body:?}"
        );
        assert!(
            state::service_state(&*engine)
                .expect("this service's state")
                .lock()
                .capacity_reservations
                .reservation(&name, SystemTime::now())
                .is_none(),
            "case {case:?}: a refused create must register no reservation"
        );

        // The same floor on the update, which carries the same member.
        let (status, body) = call(
            &router,
            "UpdateCapacityReservation",
            &json!({ "Name": "updatable", "TargetDpus": target_dpus }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?} on UpdateCapacityReservation: {target_dpus} data processing \
             units must be refused, got {status} with {body:?}"
        );
        let (_, read) = read_reservation(&router, "updatable").await;
        assert_eq!(
            read["TargetDpus"].as_i64(),
            Some(i64::from(SOME_TARGET_DPUS)),
            "case {case:?}: a refused update must change no requested amount, got {read:?}"
        );
    }
}

/// A reservation is PENDING until its allocation is due and ACTIVE with the whole
/// requested amount allocated from then on — and reading is what materialises nothing: two
/// reads at the same point report the same thing.
#[tokio::test]
async fn a_reservation_is_active_when_its_allocation_is_due() {
    let router = router();

    let target_dpus = SOME_TARGET_DPUS + 8;
    create(&router, "allocating", target_dpus).await;

    let (_, pending) = read_reservation(&router, "allocating").await;
    assert_eq!(
        pending["Status"].as_str(),
        Some(CAPACITY_RESERVATION_STATUS_PENDING),
        "a caller reading straight back must still see \
         {CAPACITY_RESERVATION_STATUS_PENDING}, got {pending:?}"
    );

    wait_out(CAPACITY_ALLOCATION);
    for attempt in 0..2 {
        let (_, read) = read_reservation(&router, "allocating").await;
        assert_eq!(
            read["Status"].as_str(),
            Some(CAPACITY_RESERVATION_STATUS_ACTIVE),
            "read {attempt} after the allocation was due must report \
             {CAPACITY_RESERVATION_STATUS_ACTIVE}, got {read:?}"
        );
        assert_eq!(
            read["AllocatedDpus"].as_i64(),
            Some(i64::from(target_dpus)),
            "read {attempt}: the allocated amount must equal the requested one, got {read:?}"
        );
        assert_eq!(
            read["LastAllocation"]["Status"].as_str(),
            Some("SUCCEEDED"),
            "read {attempt}: the allocation completed, got {read:?}"
        );
        assert_eq!(
            read["LastAllocation"]["RequestCompletionTime"].as_f64(),
            read["LastSuccessfulAllocationTime"].as_f64(),
            "read {attempt}: the most recent allocation that succeeded is that one, got \
             {read:?}"
        );
    }
}

/// An update asks for a new amount: the requested amount changes at once, the allocated one
/// only when the new allocation is due, and the reservation reports UPDATE_PENDING in
/// between.
#[tokio::test]
async fn an_update_reallocates_the_reservation() {
    let router = router();

    // (case, the amount created with, the amount updated to)
    let cases: [(&str, i32, i32); 3] = [
        ("an increase", SOME_TARGET_DPUS, SOME_TARGET_DPUS * 4),
        ("a decrease to the modelled floor", 120, MODELLED_FLOOR_TARGET_DPUS),
        ("the same amount again", 64, 64),
    ];

    for (case, created_with, updated_to) in cases {
        let name = format!("updated-{created_with}-{updated_to}");
        create(&router, &name, created_with).await;
        wait_out(CAPACITY_ALLOCATION);

        let (status, body) = call(
            &router,
            "UpdateCapacityReservation",
            &json!({ "Name": name, "TargetDpus": updated_to }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the update must be accepted, got {status} with {body:?}"
        );

        let (_, updating) = read_reservation(&router, &name).await;
        assert_eq!(
            updating["Status"].as_str(),
            Some(CAPACITY_RESERVATION_STATUS_UPDATE_PENDING),
            "case {case:?}: a caller reading straight back must see \
             {CAPACITY_RESERVATION_STATUS_UPDATE_PENDING}, got {updating:?}"
        );
        assert_eq!(
            updating["TargetDpus"].as_i64(),
            Some(i64::from(updated_to)),
            "case {case:?}: the requested amount is the update's at once, got {updating:?}"
        );
        assert_eq!(
            updating["AllocatedDpus"].as_i64(),
            Some(i64::from(created_with)),
            "case {case:?}: the allocated amount is still the one the reservation holds, \
             got {updating:?}"
        );
        assert_eq!(
            updating["LastAllocation"]["Status"].as_str(),
            Some("PENDING"),
            "case {case:?}: the new allocation is in flight, got {updating:?}"
        );
        assert!(
            updating["LastSuccessfulAllocationTime"].as_f64().is_some(),
            "case {case:?}: the allocation the create made still succeeded, got {updating:?}"
        );

        wait_out(CAPACITY_ALLOCATION);
        let (_, updated) = read_reservation(&router, &name).await;
        assert_eq!(
            updated["Status"].as_str(),
            Some(CAPACITY_RESERVATION_STATUS_ACTIVE),
            "case {case:?}: the reservation is allocated again, got {updated:?}"
        );
        assert_eq!(
            updated["AllocatedDpus"].as_i64(),
            Some(i64::from(updated_to)),
            "case {case:?}: the allocated amount must reach the updated request, got \
             {updated:?}"
        );
    }
}

/// A cancellation takes the reservation through CANCELLING to CANCELLED, releases its
/// capacity, and only then can it be deleted — after which no operation reaches it.
#[tokio::test]
async fn a_cancelled_reservation_can_be_deleted_and_is_then_gone() {
    let router = router();

    create(&router, "cancelled", SOME_TARGET_DPUS).await;
    wait_out(CAPACITY_ALLOCATION);

    let (status, body) = call(
        &router,
        "CancelCapacityReservation",
        &json!({ "Name": "cancelled" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "cancelling an allocated reservation must be accepted, got {status} with {body:?}"
    );

    let (_, cancelling) = read_reservation(&router, "cancelled").await;
    assert_eq!(
        cancelling["Status"].as_str(),
        Some(CAPACITY_RESERVATION_STATUS_CANCELLING),
        "a caller reading straight back must see \
         {CAPACITY_RESERVATION_STATUS_CANCELLING}, got {cancelling:?}"
    );

    wait_out(CAPACITY_RESERVATION_CANCELLATION);
    let (_, cancelled) = read_reservation(&router, "cancelled").await;
    assert_eq!(
        cancelled["Status"].as_str(),
        Some(CAPACITY_RESERVATION_STATUS_CANCELLED),
        "the cancellation is complete once it is due, got {cancelled:?}"
    );
    assert_eq!(
        cancelled["AllocatedDpus"].as_i64(),
        Some(0),
        "a cancelled reservation holds no capacity, got {cancelled:?}"
    );
    assert_eq!(
        cancelled["TargetDpus"].as_i64(),
        Some(i64::from(SOME_TARGET_DPUS)),
        "a cancelled reservation can still be viewed for historical reference, got \
         {cancelled:?}"
    );

    let (status, body) = call(
        &router,
        "DeleteCapacityReservation",
        &json!({ "Name": "cancelled" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "deleting a cancelled reservation must be accepted, got {status} with {body:?}"
    );

    // "A deleted reservation ... can no longer be referenced", so every operation of the
    // slice now reports the name as absent — and the name is free for another create.
    for operation in OPERATIONS.iter().filter(|op| **op != OPERATIONS[0]) {
        let (status, body) = call(
            &router,
            operation,
            &json!({ "Name": "cancelled", "TargetDpus": SOME_TARGET_DPUS }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {operation:?}: after the delete, expected a refusal, got {status} with \
             {body:?}"
        );
        assert_eq!(
            message(&body),
            state::capacity_reservation_not_found("cancelled"),
            "case {operation:?}: after the delete, expected the miss this slice words, got \
             {:?}",
            message(&body)
        );
    }
    create(&router, "cancelled", SOME_TARGET_DPUS).await;
}

/// The message the slice refuses `operation` with when the reservation it addressed is in
/// `status`: a call that would have CHANGED the reservation states the state that refused
/// it, and a delete states the rule it needs instead — a reservation must be cancelled
/// before it can be deleted, whatever state it is in.
fn status_refusal(operation: &str, status: &str) -> String {
    if operation == "DeleteCapacityReservation" {
        state::capacity_reservation_not_cancelled()
    } else {
        state::capacity_reservation_wrong_state(status)
    }
}

/// The three operations of the slice a reservation's status can refuse.
const MUTATING_OPERATIONS: [&str; 3] = [
    "UpdateCapacityReservation",
    "CancelCapacityReservation",
    "DeleteCapacityReservation",
];

/// Every call the reservation's status does not allow is refused, naming that status, and
/// changes nothing: the status the registry holds is what decides, not the order the caller
/// tried things in.
#[tokio::test]
async fn a_call_the_status_does_not_allow_is_refused() {
    let router = router();

    // While the first allocation is in flight, nothing but a read is allowed.
    create(&router, "gated", SOME_TARGET_DPUS).await;
    for operation in MUTATING_OPERATIONS {
        let (status, body) = call(
            &router,
            operation,
            &json!({ "Name": "gated", "TargetDpus": SOME_TARGET_DPUS * 2 }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {operation:?} while {CAPACITY_RESERVATION_STATUS_PENDING}: expected a \
             refusal, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_REQUEST,
            "case {operation:?} while {CAPACITY_RESERVATION_STATUS_PENDING}: expected \
             {INVALID_REQUEST}, got {body:?}"
        );
        assert_eq!(
            message(&body),
            status_refusal(operation, CAPACITY_RESERVATION_STATUS_PENDING),
            "case {operation:?}: the refusal must state the status that refused it, got {:?}",
            message(&body)
        );
        assert_eq!(
            status_of(&router, "gated").await,
            CAPACITY_RESERVATION_STATUS_PENDING,
            "case {operation:?}: a refused call must leave the reservation as it was"
        );
    }

    // Allocated: a delete is still refused, since the reservation has not been cancelled.
    wait_out(CAPACITY_ALLOCATION);
    let (status, body) = call(
        &router,
        "DeleteCapacityReservation",
        &json!({ "Name": "gated" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a delete before the cancel must be refused, got {status} with {body:?}"
    );
    assert_eq!(
        message(&body),
        status_refusal(
            "DeleteCapacityReservation",
            CAPACITY_RESERVATION_STATUS_ACTIVE,
        ),
        "the refusal must state the status the reservation is in, got {:?}",
        message(&body)
    );

    // Being updated: the allocation in flight refuses another update and the cancel.
    let (status, _) = call(
        &router,
        "UpdateCapacityReservation",
        &json!({ "Name": "gated", "TargetDpus": SOME_TARGET_DPUS * 2 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the update of an allocated reservation is accepted"
    );
    for operation in MUTATING_OPERATIONS {
        let (status, body) = call(
            &router,
            operation,
            &json!({ "Name": "gated", "TargetDpus": SOME_TARGET_DPUS }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {operation:?} while {CAPACITY_RESERVATION_STATUS_UPDATE_PENDING}: \
             expected a refusal, got {status} with {body:?}"
        );
        assert_eq!(
            message(&body),
            status_refusal(operation, CAPACITY_RESERVATION_STATUS_UPDATE_PENDING),
            "case {operation:?}: the refusal must state the status that refused it, got {:?}",
            message(&body)
        );
    }

    // Cancelling: a second cancel, an update and a delete are all refused until the
    // cancellation is complete.
    wait_out(CAPACITY_ALLOCATION);
    let (status, _) = call(
        &router,
        "CancelCapacityReservation",
        &json!({ "Name": "gated" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the cancel of an allocated reservation is accepted"
    );
    for operation in MUTATING_OPERATIONS {
        let (status, body) = call(
            &router,
            operation,
            &json!({ "Name": "gated", "TargetDpus": SOME_TARGET_DPUS }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {operation:?} while {CAPACITY_RESERVATION_STATUS_CANCELLING}: expected a \
             refusal, got {status} with {body:?}"
        );
        assert_eq!(
            message(&body),
            status_refusal(operation, CAPACITY_RESERVATION_STATUS_CANCELLING),
            "case {operation:?}: the refusal must state the status that refused it, got {:?}",
            message(&body)
        );
    }

    // Cancelled: an update and a second cancel are still refused, the delete being the one
    // call this status allows.
    wait_out(CAPACITY_RESERVATION_CANCELLATION);
    for operation in ["UpdateCapacityReservation", "CancelCapacityReservation"] {
        let (status, body) = call(
            &router,
            operation,
            &json!({ "Name": "gated", "TargetDpus": SOME_TARGET_DPUS }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {operation:?} while {CAPACITY_RESERVATION_STATUS_CANCELLED}: expected a \
             refusal, got {status} with {body:?}"
        );
        assert_eq!(
            message(&body),
            status_refusal(operation, CAPACITY_RESERVATION_STATUS_CANCELLED),
            "case {operation:?}: the refusal must state the status that refused it, got {:?}",
            message(&body)
        );
    }
}

/// A name the registry does not hold is refused by every operation that addresses an
/// existing reservation, whatever else the account holds.
#[tokio::test]
async fn an_unregistered_name_is_refused() {
    let router = router();

    create(&router, "registered", SOME_TARGET_DPUS).await;

    // Names no create registered: another spelling, a prefix, a one-character name, and
    // the longest the model admits.
    let absent: [String; 4] = [
        "unregistered".to_string(),
        "registere".to_string(),
        name_of_length(1),
        name_of_length(128),
    ];

    for name in absent {
        for operation in OPERATIONS.iter().filter(|op| **op != OPERATIONS[0]) {
            let (status, body) = call(
                &router,
                operation,
                &json!({ "Name": name, "TargetDpus": SOME_TARGET_DPUS }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {name:?} on {operation}: expected a refusal, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                INVALID_REQUEST,
                "case {name:?} on {operation}: expected {INVALID_REQUEST}, got {body:?}"
            );
            assert_eq!(
                message(&body),
                state::capacity_reservation_not_found(&name),
                "case {name:?} on {operation}: every operation of the slice words the miss \
                 the same way"
            );
        }
    }
}

/// A reservation name is unique per account: a second create under a name the registry
/// already holds is refused and changes nothing.
#[tokio::test]
async fn a_name_the_registry_already_holds_is_refused() {
    let router = router();

    // (case, the amount the second create asks for)
    let cases: [(&str, i32); 2] = [
        ("the same amount again", SOME_TARGET_DPUS),
        ("another amount", SOME_TARGET_DPUS * 2),
    ];

    for (case, second_amount) in cases {
        let name = format!("taken-{second_amount}");
        create(&router, &name, SOME_TARGET_DPUS).await;

        let (status, body) = call(
            &router,
            "CreateCapacityReservation",
            &json!({ "Name": name, "TargetDpus": second_amount }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the second create must be refused, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_REQUEST,
            "case {case:?}: expected {INVALID_REQUEST}, got {body:?}"
        );
        let (_, read) = read_reservation(&router, &name).await;
        assert_eq!(
            read["TargetDpus"].as_i64(),
            Some(i64::from(SOME_TARGET_DPUS)),
            "case {case:?}: the refused create must leave the first reservation as it was, \
             got {read:?}"
        );
    }
}

/// The status, the allocated amount and the allocation report all follow from the stored
/// instants alone, at every point of the lifecycle — the invariant the wire tests above
/// sample at a few instants, asserted here across the whole span without waiting for it.
#[test]
fn the_reported_state_follows_the_clock() {
    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    let created = CapacityReservationRecord::created("clock".to_string(), 48, Vec::new(), base);

    // (case, how long after the create, the status, the allocated amount)
    let while_allocating: [(&str, Duration, &str, i32); 4] = [
        (
            "the instant of the create",
            Duration::ZERO,
            CAPACITY_RESERVATION_STATUS_PENDING,
            0,
        ),
        (
            "part way through the allocation",
            CAPACITY_ALLOCATION / 2,
            CAPACITY_RESERVATION_STATUS_PENDING,
            0,
        ),
        (
            "the instant the allocation is due",
            CAPACITY_ALLOCATION,
            CAPACITY_RESERVATION_STATUS_ACTIVE,
            48,
        ),
        (
            "long after the allocation is due",
            Duration::from_secs(86_400),
            CAPACITY_RESERVATION_STATUS_ACTIVE,
            48,
        ),
    ];
    for (case, elapsed, status, allocated) in while_allocating {
        let now = base + elapsed;
        let reported = created.to_capacity_reservation(now);
        assert_eq!(
            reported.status, status,
            "case {case:?}: expected status {status}, got {:?}",
            reported.status
        );
        assert_eq!(
            reported.allocated_dpus, allocated,
            "case {case:?}: expected {allocated} allocated, got {}",
            reported.allocated_dpus
        );
        assert_eq!(
            reported.last_successful_allocation_time.is_some(),
            status == CAPACITY_RESERVATION_STATUS_ACTIVE,
            "case {case:?}: an allocation has succeeded only once it is due"
        );
    }

    // An update asked for while the reservation is allocated: the requested amount changes
    // at once, the allocated one when the new allocation is due.
    let update_at = base + CAPACITY_ALLOCATION;
    let mut updated = created.clone();
    updated.request_allocation(96, update_at);
    let while_updating: [(&str, Duration, &str, i32, i32); 3] = [
        (
            "the instant of the update",
            Duration::ZERO,
            CAPACITY_RESERVATION_STATUS_UPDATE_PENDING,
            96,
            48,
        ),
        (
            "part way through the new allocation",
            CAPACITY_ALLOCATION / 2,
            CAPACITY_RESERVATION_STATUS_UPDATE_PENDING,
            96,
            48,
        ),
        (
            "the instant the new allocation is due",
            CAPACITY_ALLOCATION,
            CAPACITY_RESERVATION_STATUS_ACTIVE,
            96,
            96,
        ),
    ];
    for (case, elapsed, status, target, allocated) in while_updating {
        let reported = updated.to_capacity_reservation(update_at + elapsed);
        assert_eq!(
            reported.status, status,
            "case {case:?}: expected status {status}, got {:?}",
            reported.status
        );
        assert_eq!(
            reported.target_dpus, target,
            "case {case:?}: expected {target} requested, got {}",
            reported.target_dpus
        );
        assert_eq!(
            reported.allocated_dpus, allocated,
            "case {case:?}: expected {allocated} allocated, got {}",
            reported.allocated_dpus
        );
    }

    // A cancellation asked for while the reservation is allocated: CANCELLING until it is
    // complete, then CANCELLED with no capacity, and out of the account once the
    // documented retention has elapsed.
    let cancel_at = update_at + CAPACITY_ALLOCATION;
    let mut cancelled = updated.clone();
    cancelled.cancel(cancel_at);
    let while_cancelling: [(&str, Duration, &str, i32, bool); 5] = [
        (
            "the instant of the cancel",
            Duration::ZERO,
            CAPACITY_RESERVATION_STATUS_CANCELLING,
            96,
            false,
        ),
        (
            "part way through the cancellation",
            CAPACITY_RESERVATION_CANCELLATION / 2,
            CAPACITY_RESERVATION_STATUS_CANCELLING,
            96,
            false,
        ),
        (
            "the instant the cancellation is due",
            CAPACITY_RESERVATION_CANCELLATION,
            CAPACITY_RESERVATION_STATUS_CANCELLED,
            0,
            false,
        ),
        (
            "one second before the retention ends",
            CANCELLED_CAPACITY_RESERVATION_RETENTION - Duration::from_secs(1),
            CAPACITY_RESERVATION_STATUS_CANCELLED,
            0,
            false,
        ),
        (
            "the instant the retention ends",
            CANCELLED_CAPACITY_RESERVATION_RETENTION,
            CAPACITY_RESERVATION_STATUS_CANCELLED,
            0,
            true,
        ),
    ];
    for (case, elapsed, status, allocated, retired) in while_cancelling {
        let now = cancel_at + elapsed;
        let reported = cancelled.to_capacity_reservation(now);
        assert_eq!(
            reported.status, status,
            "case {case:?}: expected status {status}, got {:?}",
            reported.status
        );
        assert_eq!(
            reported.allocated_dpus, allocated,
            "case {case:?}: expected {allocated} allocated, got {}",
            reported.allocated_dpus
        );
        assert_eq!(
            cancelled.is_retired_at(now),
            retired,
            "case {case:?}: expected the account to hold it: {}",
            !retired
        );
    }
}

/// A cancelled reservation leaves the account once the documented retention has elapsed:
/// no read reaches it, and its name is free again.
#[tokio::test]
async fn a_cancelled_reservation_is_gone_once_its_retention_elapsed() {
    let (engine, router) = engine_and_router();

    // A reservation cancelled longer ago than the retention, seeded through the store: the
    // span is 45 days, which no test can wait out.
    let long_ago = SystemTime::now() - (CANCELLED_CAPACITY_RESERVATION_RETENTION * 2);
    let mut retired =
        CapacityReservationRecord::created("retired".to_string(), 64, Vec::new(), long_ago);
    retired.cancel(long_ago + CAPACITY_ALLOCATION);
    let store = state::service_state(&*engine).expect("this service's state");
    store.lock().capacity_reservations.insert(retired);

    for operation in OPERATIONS.iter().filter(|op| **op != OPERATIONS[0]) {
        let (status, body) = call(
            &router,
            operation,
            &json!({ "Name": "retired", "TargetDpus": SOME_TARGET_DPUS }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {operation:?}: a reservation past its retention is absent, got {status} \
             with {body:?}"
        );
        assert_eq!(
            message(&body),
            state::capacity_reservation_not_found("retired"),
            "case {operation:?}: expected the miss this slice words, got {:?}",
            message(&body)
        );
    }

    // The name is free: a create under it registers a NEW reservation, in the status a
    // create leaves behind.
    create(&router, "retired", SOME_TARGET_DPUS).await;
    let (_, read) = read_reservation(&router, "retired").await;
    assert_eq!(
        read["Status"].as_str(),
        Some(CAPACITY_RESERVATION_STATUS_PENDING),
        "the create must register a new reservation under the freed name, got {read:?}"
    );
    assert_eq!(
        read["TargetDpus"].as_i64(),
        Some(i64::from(SOME_TARGET_DPUS)),
        "the new reservation holds the new request, got {read:?}"
    );
}

/// The wording of each refusal class is fixed text with the reservation's state or name as
/// its only variable part — asserted against the sentences themselves, so a change to how
/// this service words them is caught here rather than tracking the implementation.
#[tokio::test]
async fn each_refusal_class_states_its_own_sentence() {
    let router = router();

    create(&router, "worded", SOME_TARGET_DPUS).await;

    // A call that would have CHANGED a reservation whose allocation is still in flight.
    let (_, body) = call(
        &router,
        "UpdateCapacityReservation",
        &json!({ "Name": "worded", "TargetDpus": SOME_TARGET_DPUS * 2 }),
    )
    .await;
    assert_eq!(
        message(&body),
        "Reservation cannot be modified when state is PENDING",
        "the update refusal must state that the reservation cannot be modified in the \
         state it is in, got {:?}",
        message(&body)
    );

    // A delete of a reservation that was not cancelled.
    let (_, body) = call(
        &router,
        "DeleteCapacityReservation",
        &json!({ "Name": "worded" }),
    )
    .await;
    assert_eq!(
        message(&body),
        "Reservation must be cancelled before it can be deleted",
        "the delete refusal must state the rule the delete needs, got {:?}",
        message(&body)
    );

    // A name the registry does not hold, which the sentence names.
    let (_, body) = call(
        &router,
        "GetCapacityReservation",
        &json!({ "Name": "never-created" }),
    )
    .await;
    assert_eq!(
        message(&body),
        "CapacityReservation never-created was not found.",
        "the miss must name the reservation the request addressed, got {:?}",
        message(&body)
    );

    // A name the registry already holds, which that sentence names too.
    let (_, body) = call(
        &router,
        "CreateCapacityReservation",
        &json!({ "Name": "worded", "TargetDpus": SOME_TARGET_DPUS }),
    )
    .await;
    assert_eq!(
        message(&body),
        "CapacityReservation worded already exists.",
        "the duplicate create must name the reservation the request addressed, got {:?}",
        message(&body)
    );
}

/// A tag list that names nothing taggable is refused, and the refused create registers
/// nothing: the rules are `Tag$Key`'s own (a key is required and unique per resource) plus
/// the documented 50 tags one Athena resource holds.
#[tokio::test]
async fn a_tag_list_that_names_nothing_taggable_is_refused() {
    let (engine, router) = engine_and_router();

    // (case, the Tags member sent, whether Athena accepts it)
    let cases: Vec<(&str, Value, bool)> = vec![
        ("one tag", json!([{ "Key": "team", "Value": "analytics" }]), true),
        (
            "a tag with no value, which is a tag of an empty value",
            json!([{ "Key": "team" }]),
            true,
        ),
        (
            "the most tags one resource holds",
            Value::Array(
                (0..MAX_TAGS_PER_RESOURCE)
                    .map(|index| json!({ "Key": format!("key-{index}"), "Value": "v" }))
                    .collect(),
            ),
            true,
        ),
        (
            "one tag more than that",
            Value::Array(
                (0..=MAX_TAGS_PER_RESOURCE)
                    .map(|index| json!({ "Key": format!("key-{index}"), "Value": "v" }))
                    .collect(),
            ),
            false,
        ),
        ("a tag with no key", json!([{ "Value": "analytics" }]), false),
        (
            "a key that appears twice",
            json!([
                { "Key": "team", "Value": "analytics" },
                { "Key": "team", "Value": "platform" },
            ]),
            false,
        ),
    ];

    for (index, (case, tags, accepted)) in cases.into_iter().enumerate() {
        let name = format!("tagged-{index}");
        let (status, body) = call(
            &router,
            "CreateCapacityReservation",
            &json!({ "Name": name, "TargetDpus": SOME_TARGET_DPUS, "Tags": tags }),
        )
        .await;
        let held = state::service_state(&*engine)
            .expect("this service's state")
            .lock()
            .capacity_reservations
            .reservation(&name, SystemTime::now())
            .is_some();
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: the create must be accepted, got {status} with {body:?}"
            );
            assert!(held, "case {case:?}: an accepted create must register it");
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: the create must be refused, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_REQUEST,
            "case {case:?}: expected {INVALID_REQUEST}, got {body:?}"
        );
        assert!(
            !held,
            "case {case:?}: a refused create must register no reservation"
        );
    }
}

/// A create past the number of reservations this emulator holds is refused, and the
/// reservations already held are untouched.
#[tokio::test]
async fn a_create_past_the_registry_bound_is_refused() {
    let (engine, router) = engine_and_router();

    // The registry filled through the store rather than through as many calls: the bound is
    // what is under test, not the create path that reaches it.
    let now = SystemTime::now();
    {
        let store = state::service_state(&*engine).expect("this service's state");
        let mut held = store.lock();
        for index in 0..state::MAX_CAPACITY_RESERVATIONS {
            held.capacity_reservations
                .insert(CapacityReservationRecord::created(
                    format!("held-{index}"),
                    SOME_TARGET_DPUS,
                    Vec::new(),
                    now,
                ));
        }
    }

    let (status, body) = call(
        &router,
        "CreateCapacityReservation",
        &json!({ "Name": "one-too-many", "TargetDpus": SOME_TARGET_DPUS }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a create past the bound must be refused, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        INVALID_REQUEST,
        "expected {INVALID_REQUEST}, got {body:?}"
    );
    assert!(
        message(&body).contains(&state::MAX_CAPACITY_RESERVATIONS.to_string()),
        "the refusal must state the bound it reached, got {:?}",
        message(&body)
    );

    let (status, read) = read_reservation(&router, "held-0").await;
    assert_eq!(
        status, 200,
        "a refused create must leave the reservations already held readable, got {read:?}"
    );
}
