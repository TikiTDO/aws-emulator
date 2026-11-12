// SPDX-License-Identifier: CC0-1.0

//! What `StartConfigurationChecks` does while the application it names already has work
//! under way on it, driven over the wire.
//!
//! Every case goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns), so the restJson1 routing, the boundary constraint validation, the handler and
//! the response serialization all run exactly as they do in the emulator binary. Nothing
//! reaches into `crate::state`: a run is only believed to exist when
//! `ListConfigurationCheckOperations` and `GetConfigurationCheckOperation` report it back.
//!
//! RULE under test: a call that starts asynchronous work against an application is
//! refused with the modelled `ConflictException` (HTTP 409) while a change the clock
//! still owes that application is outstanding, writes nothing while it refuses, and is
//! served once that change has fallen due — so no record is ever handed to a caller
//! under an application whose own state is still moving.
//!
//! The rows span what that rule generalises over: WHICH call left the change owed (a
//! registration still settling, a start, a stop, a refresh, a settings update, a
//! deregistration), the application type, the form of the `ApplicationId`, and how many
//! checks the refused request names.

use std::collections::BTreeSet;
use std::time::Instant;

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::CONFIGURATION_CHECK_TYPE_VALUES;
use service_com_amazonaws_ssmsap_ssmsap::state::TRANSITION_SPAN;

use crate::application_lifecycle::{settle, valid_registration, Emulator, Reply};
use crate::configuration_checks::{read_record, started, string_field};
use crate::validation_message_rendering::message;

/// The modelled error code an operation already under way on an application is refused
/// with (`smithy_query(query="operation", operation="StartConfigurationChecks")`:
/// `ConflictException` @httpError 409).
const CONFLICT_EXCEPTION: &str = "ConflictException";

/// The modelled error code a start against an application this Region has not registered
/// is answered with (same query: `ResourceNotFoundException` @httpError 404).
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// `ConfigurationCheckOperationListingMode$ALL_OPERATIONS`, as the model spells it — the
/// mode that reports EVERY run of an application, which is how a row proves a refused
/// start filed none.
const ALL_OPERATIONS: &str = "ALL_OPERATIONS";

/// How this service states the OTHER condition this operation refuses with the same
/// error code: the application's registration has not completed successfully, which is
/// what a registration still running has not done.
///
/// Spelled out here rather than taken from the crate, so the wording is pinned
/// independently of the one place the handler builds it in. It names nothing of the
/// request, which is what tells it apart from the in-flight refusal.
const NOT_REGISTERED_SUCCESSFULLY: &str =
    "The specified application has not been registered successfully";

/// The call that leaves a change the clock owes the application outstanding.
#[derive(Debug, Clone, Copy)]
enum InFlight {
    /// The registration itself has not settled yet.
    Registration,
    /// `StartApplication`.
    Start,
    /// `StopApplication`.
    Stop,
    /// `StartApplicationRefresh`.
    Refresh,
    /// `UpdateApplicationSettings`.
    SettingsUpdate,
    /// `DeregisterApplication`, after which the application stops existing altogether.
    Deregistration,
}

/// One row: an application with one call's work under way on it, and the
/// `StartConfigurationChecks` request made in that window.
#[derive(Debug)]
struct Case {
    label: &'static str,
    /// The `ApplicationId`, spanning the forms the modelled `^[\w\d\.-]+$` admits.
    id: &'static str,
    /// An `ApplicationType` value (`smithy_query(query="shape",
    /// shape="ApplicationType")`: `HANA` | `SAP_ABAP`).
    app_type: &'static str,
    in_flight: InFlight,
    /// How many checks of the model's own catalogue the start names; `None` leaves the
    /// `ConfigurationCheckIds` member absent, which asks for the whole catalogue.
    requested: Option<usize>,
}

const CASES: &[Case] = &[
    Case {
        label: "a registration that has not settled yet",
        id: "HDB-TEST",
        app_type: "HANA",
        in_flight: InFlight::Registration,
        requested: None,
    },
    Case {
        label: "a start still running",
        id: "a",
        app_type: "HANA",
        in_flight: InFlight::Start,
        requested: Some(1),
    },
    Case {
        label: "a stop still running",
        id: "abap.prod_01",
        app_type: "SAP_ABAP",
        in_flight: InFlight::Stop,
        requested: Some(2),
    },
    Case {
        label: "a refresh still running",
        id: "HDB_02",
        app_type: "SAP_ABAP",
        in_flight: InFlight::Refresh,
        requested: None,
    },
    Case {
        label: "a settings update still running",
        id: "hana-1",
        app_type: "HANA",
        in_flight: InFlight::SettingsUpdate,
        requested: Some(1),
    },
    Case {
        label: "a deregistration still running",
        id: "HDB.TEST-9",
        app_type: "HANA",
        in_flight: InFlight::Deregistration,
        requested: None,
    },
];

/// The checks `case`'s start asks for, read off the model's own catalogue so a change to
/// the `ConfigurationCheckType` enum keeps the rows testing the rule.
fn requested_checks(case: &Case) -> Vec<String> {
    let catalogue: Vec<String> = CONFIGURATION_CHECK_TYPE_VALUES
        .iter()
        .map(|check| (*check).to_string())
        .collect();
    match case.requested {
        None => catalogue,
        Some(count) => catalogue.into_iter().take(count).collect(),
    }
}

/// The `StartConfigurationChecks` body for `application`, carrying `case`'s check
/// selection.
fn start_request(case: &Case, application: &str) -> Value {
    let mut body = Map::new();
    body.insert("ApplicationId".into(), json!(application));
    if case.requested.is_some() {
        body.insert(
            "ConfigurationCheckIds".into(),
            json!(requested_checks(case)),
        );
    }
    Value::Object(body)
}

impl Emulator {
    /// Register one application, with no wait: the caller decides whether the
    /// registration is left outstanding.
    async fn register_one(&self, label: &str, id: &str, app_type: &str) {
        let reply = self
            .send("/register-application", valid_registration(id, app_type))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: expected 200 from RegisterApplication for {id:?}, got {reply:?}"
        );
    }

    /// Every run `ListConfigurationCheckOperations` reports for `application`, in every
    /// mode, as a set of `Id`s.
    async fn every_run(&self, application: &str) -> (Reply, BTreeSet<String>) {
        let reply = self
            .send(
                "/list-configuration-check-operations",
                json!({"ApplicationId": application, "ListMode": ALL_OPERATIONS}),
            )
            .await;
        let ids = reply
            .member("ConfigurationCheckOperations")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .map(|row| string_field(row, "Id"))
                    .collect::<BTreeSet<String>>()
            })
            .unwrap_or_default();
        (reply, ids)
    }
}

/// Make `case`'s call, leaving the change it owes the application outstanding.
async fn leave_work_in_flight(emulator: &Emulator, case: &Case) {
    let (path, body) = match case.in_flight {
        // The registration left outstanding by the caller is the work in flight.
        InFlight::Registration => return,
        InFlight::Start => ("/start-application", json!({"ApplicationId": case.id})),
        InFlight::Stop => ("/stop-application", json!({"ApplicationId": case.id})),
        InFlight::Refresh => (
            "/start-application-refresh",
            json!({"ApplicationId": case.id}),
        ),
        InFlight::SettingsUpdate => (
            "/update-application-settings",
            json!({
                "ApplicationId": case.id,
                "CredentialsToAddOrUpdate": [{
                    "DatabaseName": "TNT1",
                    "CredentialType": "ADMIN",
                    "SecretId": "sap/tnt1/admin",
                }],
            }),
        ),
        InFlight::Deregistration => (
            "/deregister-application",
            json!({"ApplicationId": case.id}),
        ),
    };
    let reply = emulator.send(path, body).await;
    assert_eq!(
        reply.status,
        200,
        "case {:?}: expected 200 from {path}, got {reply:?}",
        case.label
    );
}

/// RULE: a change the clock still owes an application is work already under way on it, so
/// `StartConfigurationChecks` is refused with the modelled `ConflictException` (HTTP 409)
/// until that work completes, files nothing while it refuses, and is served once the owed
/// change has fallen due — or answers the modelled 404 when the owed change was the
/// application's removal.
#[tokio::test]
async fn a_start_is_refused_while_the_application_owes_a_change() {
    for case in CASES {
        let label = case.label;
        let emulator = Emulator::new();

        // A second application with nothing in flight on it: the control the refusal must
        // leave alone. It is registered and settled first, so it is up whatever state the
        // row's own application is in.
        let free = format!("{}-free", case.id);
        emulator.register_one(label, &free, case.app_type).await;
        settle().await;

        emulator.register_one(label, case.id, case.app_type).await;
        if !matches!(case.in_flight, InFlight::Registration) {
            settle().await;
            leave_work_in_flight(&emulator, case).await;
        }
        let owed_at = Instant::now();

        // Everything a client can do in that window, before any assertion gates on the
        // clock.
        let refused = emulator.start_checks(start_request(case, case.id)).await;
        let sibling = emulator
            .send("/start-application", json!({"ApplicationId": case.id}))
            .await;
        let (listing, filed) = emulator.every_run(case.id).await;
        let on_the_free_one = emulator.start_checks(start_request(case, &free)).await;

        // The transient assertions hold only while the clock still owes the change: past
        // that instant it was due whether or not anybody called.
        if owed_at.elapsed() < TRANSITION_SPAN {
            assert_eq!(
                refused.status, 409,
                "case {label:?}: expected 409 while a change is owed, got {refused:?}"
            );
            assert_eq!(
                refused.code(),
                CONFLICT_EXCEPTION,
                "case {label:?}: expected {CONFLICT_EXCEPTION}, got {refused:?}"
            );
            // WHICH condition the refusal states depends on the work in flight: a
            // registration that has not settled has not been registered successfully
            // either, and that precondition is the one this operation reports first — in
            // the fixed wording that names nothing of the request. Any other work in
            // flight is an operation in progress on an application that IS registered, so
            // the refusal names it.
            let refusal = message(&refused);
            if matches!(case.in_flight, InFlight::Registration) {
                assert_eq!(
                    refusal, NOT_REGISTERED_SUCCESSFULLY,
                    "case {label:?}: expected the not-registered-successfully refusal, got \
                     {refusal:?}"
                );
            } else {
                assert!(
                    refusal.contains(case.id),
                    "case {label:?}: the refusal must name the application, got {refusal:?}"
                );
            }
            assert!(
                refused.member("ConfigurationCheckOperations").is_none(),
                "case {label:?}: a refusal reports no started operation, got {refused:?}"
            );

            // The sibling control call refuses the SAME condition in the SAME window, so
            // a client reads one answer for one state of the application.
            assert_eq!(
                (sibling.status, sibling.code()),
                (409, CONFLICT_EXCEPTION),
                "case {label:?}: StartApplication refuses the same window, got {sibling:?}"
            );

            // The refused start wrote nothing: no run exists under the application, so no
            // id was handed out that a later read could fail to resolve.
            assert_eq!(
                listing.status, 200,
                "case {label:?}: the listing still resolves, got {listing:?}"
            );
            assert!(
                filed.is_empty(),
                "case {label:?}: a refused start files no run, got {filed:?}"
            );

            // And the refusal is about THIS application only: the free one starts its
            // checks in the same window.
            assert_eq!(
                on_the_free_one.status, 200,
                "case {label:?}: an application owed nothing still starts checks, got \
                 {on_the_free_one:?}"
            );
            assert_eq!(
                started(&on_the_free_one).len(),
                requested_checks(case).len(),
                "case {label:?}: expected one operation per requested check on {free:?}, got \
                 {on_the_free_one:?}"
            );
        }

        // Once the owed change has fallen due the call is served — unless what fell due
        // was the application's removal, which leaves nothing to start checks against.
        settle().await;
        let after = emulator.start_checks(start_request(case, case.id)).await;
        if matches!(case.in_flight, InFlight::Deregistration) {
            assert_eq!(
                (after.status, after.code()),
                (404, RESOURCE_NOT_FOUND),
                "case {label:?}: a removed application is not found, got {after:?}"
            );
            let (gone, _) = emulator.every_run(case.id).await;
            assert_eq!(
                gone.status, 404,
                "case {label:?}: nothing is listed under a removed application, got {gone:?}"
            );
            continue;
        }

        assert_eq!(
            after.status, 200,
            "case {label:?}: expected the start to be served once the owed change fell due, \
             got {after:?}"
        );
        let expected = requested_checks(case);
        let operations = started(&after);
        assert_eq!(
            operations.len(),
            expected.len(),
            "case {label:?}: expected one operation per requested check, got {after:?}"
        );
        // Every id the 200 handed back names a record that exists, read back through the
        // reader of that record.
        let (_, now_filed) = emulator.every_run(case.id).await;
        for operation in &operations {
            let id = string_field(operation, "Id");
            let check = string_field(operation, "ConfigurationCheckId");
            assert!(
                expected.contains(&check),
                "case {label:?}: a started run names a requested check, got {operation:?}"
            );
            assert!(
                now_filed.contains(&id),
                "case {label:?}: the listing reports the run {id:?}, got {now_filed:?}"
            );
            let record = emulator.get_check(&id).await;
            assert_eq!(
                record.status, 200,
                "case {label:?}: the id handed back must resolve, got {record:?}"
            );
            assert_eq!(
                (
                    string_field(&read_record(&record), "Id"),
                    string_field(&read_record(&record), "ConfigurationCheckId"),
                ),
                (id.clone(), check.clone()),
                "case {label:?}: the record must be the run that was started, got {record:?}"
            );
        }
    }
}
