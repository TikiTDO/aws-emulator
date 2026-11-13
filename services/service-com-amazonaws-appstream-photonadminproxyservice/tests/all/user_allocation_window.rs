// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHAT decides whether a create of a user pool identity the pool
//! already holds is answered like the create that allocated it.
//!
//! RULE: the outcome of a create is decided by this service's own state and by the span the
//! emulator OWES the allocation it made — never by how many calls a caller made in between
//! or by how fast the host ran them. So a repeat issued while the allocation is still open
//! is answered the same empty success however much work precedes it, a repeat that asks for
//! a DIFFERENT user than the stored one is refused `ResourceAlreadyExistsException` even
//! while it is open, and the window closes on an instant the allocating create fixed,
//! whatever arrives in the meantime.
//!
//! The rows differ along what the rule generalises over: the number and the kind of calls
//! between the create and the repeat (none, one read, several reads, a write then a read, a
//! create of another identity), the attributes the identity carries, and which attribute a
//! conflicting repeat names.
//!
//! Every case drives the awsJson1_1 front door through the shared
//! [`Emulator`](crate::stack_fleet_lifecycle::Emulator) harness. Nothing here waits out a
//! span of its own choosing: the one case about the window CLOSING looks again until it
//! has, inside a generous deadline.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::refusal_messages::{error_message, refusal};
use crate::stack_fleet_lifecycle::{error_code, Emulator};

/// The only authentication type the user pool operations manage, which
/// `CreateUser.AuthenticationType` documents as "You must specify USERPOOL".
const USER_POOL: &str = "USERPOOL";

/// AWS's sentence for a username that is taken, read from the crate so this suite tracks
/// what the service states rather than a copy of it.
const ALREADY_EXISTS: &str =
    service_com_amazonaws_appstream_photonadminproxyservice::state::USER_ALREADY_EXISTS_MESSAGE;

/// How long the window-closing case looks again before it gives up. Generous: it bounds a
/// span the service owes, so it says nothing about how fast this host has to be.
const DEADLINE: Duration = Duration::from_secs(60);

/// How often that case looks again while it waits.
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// The `CreateUser` request for `user_name` carrying `extra` beside the identity.
fn create_request(user_name: &str, extra: &Value) -> Value {
    let mut request = json!({ "UserName": user_name, "AuthenticationType": USER_POOL });
    for (member, value) in extra.as_object().expect("the row carries members") {
        request[member] = value.clone();
    }
    request
}

/// Every user the pool reports.
async fn user_pool(emulator: &Emulator, case: &str) -> Vec<Value> {
    let payload = emulator
        .ok(
            case,
            "DescribeUsers",
            json!({ "AuthenticationType": USER_POOL }),
        )
        .await;
    payload
        .get("Users")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Every user the pool reports for `user_name` — one for an identity allocated once.
async fn reported(emulator: &Emulator, case: &str, user_name: &str) -> Vec<Value> {
    user_pool(emulator, case)
        .await
        .into_iter()
        .filter(|user| user.get("UserName").and_then(Value::as_str) == Some(user_name))
        .collect()
}

/// What a row does between the create that allocates the identity and the repeat of it.
///
/// Each variant is a different amount and kind of work, which is what the rule generalises
/// over: none of it decides the repeat's outcome.
#[derive(Debug, Clone, Copy)]
enum Between {
    /// Nothing at all: the repeat follows the create directly.
    Nothing,
    /// Reads of the pool, which report the identity back to the caller.
    Reads(usize),
    /// A write to the user, then a read of it.
    DisableThenRead,
    /// A create of a DIFFERENT identity, which allocates one of its own.
    AnotherCreate,
}

impl Between {
    async fn apply(self, emulator: &Emulator, case: &str, user_name: &str) {
        match self {
            Between::Nothing => {}
            Between::Reads(times) => {
                for _ in 0..times {
                    user_pool(emulator, case).await;
                }
            }
            Between::DisableThenRead => {
                emulator
                    .ok(
                        case,
                        "DisableUser",
                        json!({ "UserName": user_name, "AuthenticationType": USER_POOL }),
                    )
                    .await;
                user_pool(emulator, case).await;
            }
            Between::AnotherCreate => {
                emulator
                    .ok(
                        case,
                        "CreateUser",
                        create_request("another.identity@example.com", &json!({})),
                    )
                    .await;
            }
        }
    }
}

#[tokio::test]
async fn a_repeat_issued_while_the_allocation_is_open_is_answered_like_the_first() {
    // Rows: what the created identity carries, and what the caller does between the create
    // and the repeat of it. Whatever that work is, the repeat is answered the same empty
    // success and the identity is still allocated exactly once, exactly as it was.
    for (case, user_name, created, between) in [
        (
            "nothing in between",
            "straight.after@example.com",
            json!({ "FirstName": "Ada", "LastName": "Lovelace" }),
            Between::Nothing,
        ),
        (
            "one read in between",
            "read.back@example.com",
            json!({ "MessageAction": "SUPPRESS" }),
            Between::Reads(1),
        ),
        (
            "eight reads in between",
            "read.back.again@example.com",
            json!({ "FirstName": "Grace" }),
            Between::Reads(8),
        ),
        (
            "a write and a read in between",
            "disabled.then.repeated@example.com",
            json!({ "LastName": "Hopper" }),
            Between::DisableThenRead,
        ),
        (
            "another identity allocated in between",
            "ユーザー@example.com",
            json!({}),
            Between::AnotherCreate,
        ),
    ] {
        let emulator = Emulator::new();
        emulator
            .ok(case, "CreateUser", create_request(user_name, &created))
            .await;
        let allocated = reported(&emulator, case, user_name).await;
        assert_eq!(
            allocated.len(),
            1,
            "case {case:?}: the create must have allocated exactly the one identity, got \
             {allocated:?}"
        );

        between.apply(&emulator, case, user_name).await;
        let before = reported(&emulator, case, user_name).await;

        let answer = emulator
            .ok(case, "CreateUser", create_request(user_name, &created))
            .await;
        assert_eq!(
            answer,
            json!({}),
            "case {case:?}: a repeat of an open allocation answers the same empty result, got \
             {answer}"
        );

        let after = reported(&emulator, case, user_name).await;
        assert_eq!(
            after.len(),
            1,
            "case {case:?}: the identity must still be allocated exactly once, got {after:?}"
        );
        assert_eq!(
            after, before,
            "case {case:?}: the repeat must leave the stored user exactly as it was, got \
             {after:?}"
        );
    }
}

#[tokio::test]
async fn a_repeat_that_asks_for_a_different_user_is_refused_while_the_allocation_is_open() {
    // Rows: what the identity was created with, and the attribute the repeat names
    // differently — including one the stored record does not carry at all. A create that
    // asks for a user other than the one that is there claims a username that is taken,
    // whether or not the allocation is still open, so no row waits for anything.
    for (case, created, repeated) in [
        (
            "a different first name",
            json!({ "FirstName": "Ada", "LastName": "Lovelace" }),
            json!({ "FirstName": "Augusta", "LastName": "Lovelace" }),
        ),
        (
            "a different last name",
            json!({ "FirstName": "Ada", "LastName": "Lovelace" }),
            json!({ "LastName": "Byron" }),
        ),
        (
            "both names different",
            json!({ "FirstName": "Ada", "LastName": "Lovelace" }),
            json!({ "FirstName": "Augusta", "LastName": "Byron" }),
        ),
        (
            "a name the stored user does not carry",
            json!({}),
            json!({ "FirstName": "Augusta" }),
        ),
    ] {
        let emulator = Emulator::new();
        let user_name = "conflicting@example.com";
        emulator
            .ok(case, "CreateUser", create_request(user_name, &created))
            .await;
        let before = reported(&emulator, case, user_name).await;

        let message = refusal(
            &emulator,
            case,
            "CreateUser",
            create_request(user_name, &repeated),
            400,
            "ResourceAlreadyExistsException",
        )
        .await;
        assert_eq!(
            message, ALREADY_EXISTS,
            "case {case:?}: expected AWS's sentence {ALREADY_EXISTS:?}, got {message:?}"
        );
        assert!(
            !message.contains(user_name),
            "case {case:?}: the sentence states that the username is taken without echoing \
             it, got {message:?}"
        );

        let after = reported(&emulator, case, user_name).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused create must leave the stored user exactly as it was, \
             got {after:?}"
        );
    }
}

/// Look again until a create of the identity `request` names is refused as a username that
/// is taken, answering the message that refusal carried.
///
/// The allocation a create makes settles on an instant that create fixed, which is a span no
/// caller controls, so a suite that needs the identity ESTABLISHED waits for the refusal
/// rather than for a span of its own choosing. A repeat cannot move that instant, which is
/// what makes looking again with the create itself sound — and asserting it, since a run of
/// identical creates may not hold the allocation open.
///
/// THE ONE waiter for that span in these suites, so no row decides the outcome at an instant
/// of its own choosing.
pub(crate) async fn refused_once_the_allocation_settles(
    emulator: &Emulator,
    case: &str,
    request: &Value,
) -> String {
    let deadline = Instant::now() + DEADLINE;
    let mut issued = 0_usize;
    loop {
        let (status, payload) = emulator.call("CreateUser", request.clone()).await;
        issued += 1;
        if status != 200 {
            assert_eq!(
                status, 400,
                "case {case:?}: create {issued} must be answered 200 or refused 400, got \
                 {status} {payload}"
            );
            assert_eq!(
                error_code(&payload),
                "ResourceAlreadyExistsException",
                "case {case:?}: create {issued} must be refused as a username that is taken, \
                 got {payload}"
            );
            return error_message(&payload);
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the allocation never closed: {issued} identical creates were all \
             answered 200"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

#[tokio::test]
async fn the_allocation_window_closes_on_an_instant_a_later_create_cannot_move() {
    // A run of identical creates cannot hold the allocation open: the window is measured
    // from the create that allocated the identity, so looking again eventually finds it
    // closed — and once it is, every further create is refused.
    let case = "a run of identical creates";
    let emulator = Emulator::new();
    let user_name = "settles@example.com";
    let request = create_request(user_name, &json!({ "MessageAction": "SUPPRESS" }));
    emulator.ok(case, "CreateUser", request.clone()).await;

    let message = refused_once_the_allocation_settles(&emulator, case, &request).await;
    assert_eq!(
        message, ALREADY_EXISTS,
        "case {case:?}: expected AWS's sentence {ALREADY_EXISTS:?}, got {message:?}"
    );

    // However many creates were answered, the identity was allocated once.
    let allocated = reported(&emulator, case, user_name).await;
    assert_eq!(
        allocated.len(),
        1,
        "case {case:?}: the identity must be allocated exactly once, got {allocated:?}"
    );

    // And the window stays closed: a refused create does not reopen it.
    for round in 0..3 {
        let message = refusal(
            &emulator,
            case,
            "CreateUser",
            request.clone(),
            400,
            "ResourceAlreadyExistsException",
        )
        .await;
        assert_eq!(
            message, ALREADY_EXISTS,
            "case {case:?}: create {round} after the window closed must carry AWS's sentence \
             {ALREADY_EXISTS:?}, got {message:?}"
        );
    }
    let settled = reported(&emulator, case, user_name).await;
    assert_eq!(
        settled, allocated,
        "case {case:?}: the refused creates must leave the stored user exactly as it was, \
         got {settled:?}"
    );
}
