// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `AcceptCapacityReservationBillingOwnership`, driven over the wire the
//! service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose
//! body carries `Action=AcceptCapacityReservationBillingOwnership`, answered with XML
//! (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: accepting the billing ownership of a shared Capacity Reservation
//! ANSWERS the billing assignment request the store holds for that reservation — the one sent
//! TO the calling account — by moving it from `pending` to `accepted` as of the call, and
//! reports `Return=true`; a value outside the Capacity Reservation id form EC2 issues is
//! refused `InvalidCapacityReservationId.Malformed` before any lookup, an id this account was
//! sent no request for is reported `InvalidCapacityReservationId.NotFound`, a request that is
//! not `pending` is refused `IncorrectState` naming the status it is in, and every refusal —
//! and every dry run — leaves the store exactly as it was.
//!
//! Each table's rows differ along what the rule generalises over: which request of several is
//! answered, what the answered request carries (a status message, the reservation's details,
//! neither, both), which side of the assignment the calling account is on, which id form was
//! sent, and which status the stored request is in — the modelled catalog's own members
//! (`crate::ops::types::CAPACITY_RESERVATION_BILLING_REQUEST_STATUS_VALUES`), so no single
//! recorded request is the specification. The answered request is read back through
//! `DescribeCapacityReservationBillingRequests` over the wire, so the assignment is proven by
//! the store rather than by the answer.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ec2::ops::types::{
    CAPACITY_RESERVATION_BILLING_REQUEST_STATUS_VALUES, CapacityReservationInfo,
};
use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use ec2::state::{
    CALLER_ROLE_UNUSED_RESERVATION_BILLING_OWNER,
    CAPACITY_RESERVATION_BILLING_REQUEST_STATUS_ACCEPTED,
    CAPACITY_RESERVATION_BILLING_REQUEST_STATUS_PENDING, CapacityReservationBillingRequestSpec,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, element, error_code, error_message, indexed};

/// The Region every call here is signed for. A billing assignment is between two accounts, so
/// the Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// The Capacity Reservation owner that sends a request to the calling account.
const OTHER_OWNER: &str = "210987654321";

/// A consumer account that is NOT the calling one, for the requests this account cannot
/// answer. `AccountID` is modelled `@length(12, 12)` with the pattern `^[0-9]{12}$`
/// (`smithy_query(query="shape",shape="AccountID")`), which both ids satisfy.
const OTHER_CONSUMER: &str = "345678901234";

/// A well-formed Capacity Reservation id of the form the mint issues, that the store holds no
/// billing request under.
const ABSENT_RESERVATION: &str = "cr-0fedcba987654321f";

/// EC2's answer to an authorized dry run, and the status it carries it on.
const DRY_RUN_CODE: &str = "DryRunOperation";
const DRY_RUN_STATUS: u16 = 412;
const DRY_RUN_MESSAGE: &str = "Request would have succeeded, but DryRun flag is set.";

/// The two halves of `InvalidCapacityReservationId` this operation refuses an id it cannot
/// answer with.
const MALFORMED_CODE: &str = "InvalidCapacityReservationId.Malformed";
const NOT_FOUND_CODE: &str = "InvalidCapacityReservationId.NotFound";

/// The code EC2 answers when the resource a request names exists but is not in a state that
/// admits the request.
const INCORRECT_STATE_CODE: &str = "IncorrectState";

/// The account every call in this suite is made by — the emulator's own, which is the consumer
/// account each stored request is compared against.
fn this_account() -> String {
    engine::emulator_account_id().to_string()
}

/// What one seeded billing request is created with.
///
/// Either side may be the calling account (`None`) or another account, so one table row can
/// put the caller on the consumer side, on the owner side, or on neither.
#[derive(Debug, Clone, Copy)]
struct Seed {
    /// The Capacity Reservation owner that initiated the request; `None` is the calling
    /// account.
    requested_by: Option<&'static str>,
    /// The consumer account the request was sent to; `None` is the calling account.
    sent_to: Option<&'static str>,
    /// The request's status, a member of `CapacityReservationBillingRequestStatus`.
    status: &'static str,
    /// "Information about the status", or `None` for a request that carries none.
    status_message: Option<&'static str>,
    /// The reservation's instance type, zone, zone id and tenancy, or `None` for a request
    /// that carries no `CapacityReservationInfo` at all.
    info: Option<(&'static str, &'static str, &'static str, &'static str)>,
}

impl Seed {
    /// A `pending` request another owner sent to the calling account, carrying nothing
    /// optional — the request this operation answers.
    const fn pending_to_this_account() -> Self {
        Self {
            requested_by: Some(OTHER_OWNER),
            sent_to: None,
            status: CAPACITY_RESERVATION_BILLING_REQUEST_STATUS_PENDING,
            status_message: None,
            info: None,
        }
    }

    /// The same request in `status` instead of `pending`.
    const fn in_status(status: &'static str) -> Self {
        Self {
            status,
            ..Self::pending_to_this_account()
        }
    }

    /// A `pending` request sent to ANOTHER consumer account, initiated by `requested_by`.
    const fn pending_to_another_consumer(requested_by: Option<&'static str>) -> Self {
        Self {
            requested_by,
            sent_to: Some(OTHER_CONSUMER),
            ..Self::pending_to_this_account()
        }
    }

    /// This seed's `CapacityReservationInfo`, as the store holds it.
    fn capacity_reservation_info(&self) -> Option<CapacityReservationInfo> {
        self.info
            .map(|(instance_type, zone, zone_id, tenancy)| CapacityReservationInfo {
                instance_type: Some(instance_type.to_string()),
                availability_zone: Some(zone.to_string()),
                availability_zone_id: Some(zone_id.to_string()),
                tenancy: Some(tenancy.to_string()),
            })
    }
}

/// An account id one side of a seeded request names: the calling account for `None`.
fn party(account: Option<&str>) -> String {
    account.map_or_else(this_account, str::to_string)
}

/// Write `seeds` into the engine's store, the way an operation assigning the billing of a
/// reservation's unused capacity would, and answer the reservation id each was stored under
/// beside the instant it was last updated at.
fn seed(engine: &engine::InMemoryEngine, seeds: &[Seed]) -> (Vec<String>, Vec<SystemTime>) {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let mut ids = Vec::new();
    let mut updated_at = Vec::new();
    for (index, seed) in seeds.iter().enumerate() {
        let capacity_reservation_id = guard.next_id("cr");
        // A fixed instant well in the past, so an answer written by the service is told apart
        // from the seeded one by its timestamp alone.
        let last_update_time = UNIX_EPOCH + Duration::from_secs(1_700_000_000 + index as u64);
        let stored = guard.insert_capacity_reservation_billing_request(
            CapacityReservationBillingRequestSpec {
                capacity_reservation_id: capacity_reservation_id.clone(),
                requested_by: party(seed.requested_by),
                unused_reservation_billing_owner_id: party(seed.sent_to),
                last_update_time,
                status: seed.status,
                status_message: seed.status_message.map(str::to_string),
                capacity_reservation_info: seed.capacity_reservation_info(),
            },
        );
        assert!(
            stored,
            "row {index}: the store must hold the seeded billing request"
        );
        ids.push(capacity_reservation_id);
        updated_at.push(last_update_time);
    }
    drop(guard);
    (ids, updated_at)
}

/// One `AcceptCapacityReservationBillingOwnership` call, as its HTTP status and XML body.
async fn accept(
    router: &MultiServiceRouter,
    capacity_reservation_id: &str,
    dry_run: bool,
) -> (u16, String) {
    let mut params = vec![(
        "CapacityReservationId".to_string(),
        capacity_reservation_id.to_string(),
    )];
    if dry_run {
        params.push(("DryRun".to_string(), "true".to_string()));
    }
    super::ec2_query::call(
        router,
        REGION,
        "AcceptCapacityReservationBillingOwnership",
        &params,
    )
    .await
}

/// What `DescribeCapacityReservationBillingRequests` reports for one reservation to the
/// consumer side of the assignment: its status, its `LastUpdateTime`, and its status message
/// when it carries one.
async fn described(
    router: &MultiServiceRouter,
    case: &str,
    capacity_reservation_id: &str,
) -> (String, String, Option<String>) {
    let mut params = vec![(
        "Role".to_string(),
        CALLER_ROLE_UNUSED_RESERVATION_BILLING_OWNER.to_string(),
    )];
    params.extend(indexed("CapacityReservationId", &[capacity_reservation_id]));
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "DescribeCapacityReservationBillingRequests",
        &params,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: describing must answer HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "capacityReservationId").as_deref(),
        Some(capacity_reservation_id),
        "case {case:?}: expected the request for {capacity_reservation_id} to be reported, got \
         {body}"
    );
    (
        element(&body, "status").unwrap_or_default(),
        element(&body, "lastUpdateTime").unwrap_or_default(),
        element(&body, "statusMessage"),
    )
}

/// The status the store holds for one reservation's billing request, read straight out of the
/// engine — for the assertions that need to know only that NOTHING was written.
fn stored_status(engine: &engine::InMemoryEngine, capacity_reservation_id: &str) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .capacity_reservation_billing_requests
        .get(capacity_reservation_id)
        .and_then(|request| request.status.clone())
}

// ---------------------------------------------------------------------------
// A pending request sent to this account is accepted
// ---------------------------------------------------------------------------

/// Accepting moves the request this account was sent to `accepted`, as of the call, and the
/// describe reports it — whichever of the stored requests is answered and whatever that
/// request carries.
#[tokio::test]
async fn a_pending_request_sent_to_this_account_is_accepted_and_reported_accepted() {
    // The rows differ along which request of the store is answered and what that request
    // carries: nothing optional, a status message, the reservation's details, and both.
    let rows: [(&str, Vec<Seed>, usize); 4] = [
        (
            "the only request, carrying nothing optional",
            vec![Seed::pending_to_this_account()],
            0,
        ),
        (
            "the first of three, carrying a status message",
            vec![
                Seed {
                    status_message: Some("Waiting on the consumer account"),
                    ..Seed::pending_to_this_account()
                },
                Seed::pending_to_this_account(),
                Seed::in_status("rejected"),
            ],
            0,
        ),
        (
            "the last of three, carrying the reservation's details",
            vec![
                Seed::in_status("expired"),
                Seed::pending_to_this_account(),
                Seed {
                    info: Some(("m5.large", "us-east-1a", "use1-az1", "default")),
                    ..Seed::pending_to_this_account()
                },
            ],
            2,
        ),
        (
            "one of two, carrying both",
            vec![
                Seed {
                    requested_by: Some(OTHER_CONSUMER),
                    status_message: Some("Please take the billing"),
                    info: Some(("c5.xlarge", "eu-west-1b", "euw1-az2", "dedicated")),
                    ..Seed::pending_to_this_account()
                },
                Seed::pending_to_this_account(),
            ],
            0,
        ),
    ];

    for (case, seeds, target) in rows {
        let (router, engine) = super::ec2_query::emulator();
        let (ids, seeded_at) = seed(&engine, &seeds);
        let id = ids[target].clone();

        let (status, body) = accept(&router, &id, false).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "return").as_deref(),
            Some("true"),
            "case {case:?}: expected <return>true</return>, got {body}"
        );

        let (reported_status, reported_time, reported_message) =
            described(&router, case, &id).await;
        assert_eq!(
            reported_status, CAPACITY_RESERVATION_BILLING_REQUEST_STATUS_ACCEPTED,
            "case {case:?}: expected the request to be reported accepted, got \
             {reported_status:?}"
        );
        // "When the request was last updated": the answer was given now, so the instant the
        // describe reports is the one the answer was written at, not the seeded one.
        let reported_seconds = as_timestamp(&reported_time).unwrap_or_else(|| {
            panic!("case {case:?}: expected an ISO 8601 lastUpdateTime, got {reported_time:?}")
        });
        let seeded_seconds = seeded_at[target]
            .duration_since(UNIX_EPOCH)
            .expect("the seeded instant is after the epoch")
            .as_secs_f64();
        assert!(
            reported_seconds > seeded_seconds,
            "case {case:?}: expected lastUpdateTime to be moved past {seeded_seconds} by the \
             answer, got {reported_seconds}"
        );
        // An accepted request's status says all there is: whatever message the pending request
        // carried is not reported against the new status.
        assert_eq!(
            reported_message, None,
            "case {case:?}: an accepted request must report no status message, got \
             {reported_message:?}"
        );

        // Answering one request leaves every other request of the store exactly as it was.
        for (index, seeded) in seeds.iter().enumerate() {
            if index == target {
                continue;
            }
            let held = stored_status(&engine, &ids[index]);
            assert_eq!(
                held.as_deref(),
                Some(seeded.status),
                "case {case:?}: row {index} must still be {:?}, got {held:?}",
                seeded.status
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The ids this operation cannot answer
// ---------------------------------------------------------------------------

/// A value outside the Capacity Reservation id form EC2 issues is refused as malformed before
/// anything is looked up — under `DryRun` too, there being nothing to look up — and nothing is
/// written.
#[tokio::test]
async fn a_value_outside_the_capacity_reservation_id_form_is_refused_malformed() {
    // Every row is a value EC2 could not have ISSUED as a Capacity Reservation id: the
    // all-zero placeholder the mint counting from one can never produce, the short id form,
    // a long id whose leading digit is above zero, hexadecimal suffixes of the wrong length,
    // a non-hexadecimal digit, upper-case hex, another resource's prefix, no suffix at all,
    // and a well-formed id with a character appended.
    let malformed = [
        "cr-00000000000000000",
        "cr-0abcdef12",
        "cr-1234567890abcdef1",
        "cr-0abcdef1234567",
        "cr-0abcdefg123456789",
        "cr-0ABCDEF123456789A",
        "cfr-0123456789abcdef1",
        "cr-",
        "cr-0123456789abcdef12",
    ];

    for dry_run in [false, true] {
        for id in malformed {
            let case = format!("{id:?} with DryRun={dry_run}");
            let (router, engine) = super::ec2_query::emulator();
            let (ids, _) = seed(&engine, &[Seed::pending_to_this_account()]);

            let (status, body) = accept(&router, id, dry_run).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                MALFORMED_CODE,
                "case {case:?}: expected {MALFORMED_CODE}, got {body}"
            );
            assert_eq!(
                error_message(&body),
                format!("Capacity Reservation ID '{id}' is malformed"),
                "case {case:?}: unexpected message in {body}"
            );
            let held = stored_status(&engine, &ids[0]);
            assert_eq!(
                held.as_deref(),
                Some(CAPACITY_RESERVATION_BILLING_REQUEST_STATUS_PENDING),
                "case {case:?}: a refused request must write nothing, got {held:?}"
            );
        }
    }
}

/// A well-formed id this account was sent no billing request for is reported not found —
/// whether the store holds no request under it at all, or holds one sent to ANOTHER consumer
/// account, a request this account is not the consumer of being none of its to answer.
#[tokio::test]
async fn an_id_this_account_was_sent_no_request_for_is_reported_not_found() {
    // The rows differ along why the id resolves to no request this account was sent: the
    // store holds none under it, it holds one this account itself initiated towards another
    // consumer, and it holds one between two other accounts entirely.
    let rows: [(&str, Option<Seed>); 3] = [
        ("an id the store holds no request under", None),
        (
            "a request this account initiated towards another consumer",
            Some(Seed::pending_to_another_consumer(None)),
        ),
        (
            "a request between two other accounts",
            Some(Seed::pending_to_another_consumer(Some(OTHER_OWNER))),
        ),
    ];

    for (case, held_request) in rows {
        let (router, engine) = super::ec2_query::emulator();
        let id = match held_request {
            None => ABSENT_RESERVATION.to_string(),
            Some(seeded) => seed(&engine, &[seeded]).0[0].clone(),
        };

        let (status, body) = accept(&router, &id, false).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            NOT_FOUND_CODE,
            "case {case:?}: expected {NOT_FOUND_CODE}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The capacity-reservation ID '{id}' does not exist"),
            "case {case:?}: unexpected message in {body}"
        );
        if held_request.is_some() {
            let held = stored_status(&engine, &id);
            assert_eq!(
                held.as_deref(),
                Some(CAPACITY_RESERVATION_BILLING_REQUEST_STATUS_PENDING),
                "case {case:?}: a refused request must write nothing, got {held:?}"
            );
        }
    }
}

/// A request that is not `pending` carries no assignment to take on, so it is refused for the
/// status it IS in — every other member of the modelled catalog — and nothing is written. A
/// second accept of a request this account already accepted is refused for the same reason.
#[tokio::test]
async fn a_request_that_is_not_pending_cannot_be_accepted() {
    // Every member of `CapacityReservationBillingRequestStatus` other than `pending`
    // (`crate::ops::types::CAPACITY_RESERVATION_BILLING_REQUEST_STATUS_VALUES`), so a member
    // the model adds later is covered too.
    let statuses: Vec<&'static str> = CAPACITY_RESERVATION_BILLING_REQUEST_STATUS_VALUES
        .iter()
        .copied()
        .filter(|status| *status != CAPACITY_RESERVATION_BILLING_REQUEST_STATUS_PENDING)
        .collect();
    assert!(
        statuses.len() >= 4,
        "the catalog must carry more than its pending member, got \
         {CAPACITY_RESERVATION_BILLING_REQUEST_STATUS_VALUES:?}"
    );

    for status in statuses {
        let case = format!("a request in {status:?}");
        let (router, engine) = super::ec2_query::emulator();
        let (ids, _) = seed(&engine, &[Seed::in_status(status)]);

        let (http_status, body) = accept(&router, &ids[0], false).await;
        assert_eq!(
            http_status, 400,
            "case {case:?}: expected HTTP 400, got {http_status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INCORRECT_STATE_CODE,
            "case {case:?}: expected {INCORRECT_STATE_CODE}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!(
                "Capacity Reservation billing request '{}' is in the '{status}' state.",
                ids[0]
            ),
            "case {case:?}: unexpected message in {body}"
        );
        let held = stored_status(&engine, &ids[0]);
        assert_eq!(
            held.as_deref(),
            Some(status),
            "case {case:?}: a refused request must write nothing, got {held:?}"
        );
    }

    // Accepting is not idempotent, for that same reason: once answered, the request is no
    // longer pending.
    let (router, engine) = super::ec2_query::emulator();
    let (ids, _) = seed(&engine, &[Seed::pending_to_this_account()]);
    let (first, first_body) = accept(&router, &ids[0], false).await;
    assert_eq!(
        first, 200,
        "the first accept must succeed, got {first} with body {first_body}"
    );
    let (second, second_body) = accept(&router, &ids[0], false).await;
    assert_eq!(
        second, 400,
        "a second accept must be refused, got {second} with body {second_body}"
    );
    assert_eq!(
        error_code(&second_body),
        INCORRECT_STATE_CODE,
        "a second accept must be refused for the state, got {second_body}"
    );
    assert_eq!(
        error_message(&second_body),
        format!(
            "Capacity Reservation billing request '{}' is in the '{}' state.",
            ids[0], CAPACITY_RESERVATION_BILLING_REQUEST_STATUS_ACCEPTED
        ),
        "a second accept must name the status the request is in, got {second_body}"
    );
    let held = stored_status(&engine, &ids[0]);
    assert_eq!(
        held.as_deref(),
        Some(CAPACITY_RESERVATION_BILLING_REQUEST_STATUS_ACCEPTED),
        "a refused second accept must leave the accepted assignment as it was, got {held:?}"
    );
}

// ---------------------------------------------------------------------------
// A dry run writes nothing
// ---------------------------------------------------------------------------

/// A dry run of a request that would have succeeded is answered `DryRunOperation` and the
/// assignment is NOT written — the same call without the flag then succeeds.
#[tokio::test]
async fn a_dry_run_is_answered_before_the_assignment_is_written() {
    let (router, engine) = super::ec2_query::emulator();
    let (ids, _) = seed(
        &engine,
        &[
            Seed::pending_to_this_account(),
            Seed {
                info: Some(("r5.2xlarge", "us-west-2a", "usw2-az1", "default")),
                ..Seed::pending_to_this_account()
            },
        ],
    );

    for id in &ids {
        let case = format!("a dry run of {id}");
        let (status, body) = accept(&router, id, true).await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            DRY_RUN_CODE,
            "case {case:?}: expected {DRY_RUN_CODE}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            DRY_RUN_MESSAGE,
            "case {case:?}: unexpected message in {body}"
        );
        let (reported_status, _, _) = described(&router, &case, id).await;
        assert_eq!(
            reported_status, CAPACITY_RESERVATION_BILLING_REQUEST_STATUS_PENDING,
            "case {case:?}: a dry run must leave the request pending, got {reported_status:?}"
        );
    }

    // And the same call without the flag is the one that writes the assignment.
    for id in &ids {
        let case = format!("accepting {id} for real");
        let (status, body) = accept(&router, id, false).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let (reported_status, _, _) = described(&router, &case, id).await;
        assert_eq!(
            reported_status, CAPACITY_RESERVATION_BILLING_REQUEST_STATUS_ACCEPTED,
            "case {case:?}: expected the request to be reported accepted, got \
             {reported_status:?}"
        );
    }
}
