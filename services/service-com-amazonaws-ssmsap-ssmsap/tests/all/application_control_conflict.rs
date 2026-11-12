// SPDX-License-Identifier: CC0-1.0

//! The conflict gate every control call of this service shares, driven over the wire.
//!
//! Every case goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns), so the restJson1 routing, the boundary constraint validation, the handler and
//! the response serialization all run exactly as they do in the emulator binary. Nothing
//! reaches into `crate::state`: the status a refusal words is the one `GetApplication`
//! reports in the same window.
//!
//! RULE under test: a change the clock still owes an application is an operation already
//! under way on it, so EVERY call that controls that application — `StartApplication`,
//! `StopApplication`, `StartApplicationRefresh`, `UpdateApplicationSettings`,
//! `StartConfigurationChecks` — is refused with the modelled `ConflictException` (HTTP
//! 409) while it is outstanding, in the one wording this service words that condition
//! with, and is served against an application that owes none.
//!
//! Two calls answer a REGISTRATION still running on their own terms instead, because what
//! the registration owes is not an operation that call has to wait for:
//! `StopApplication` is SERVED (stopping an SAP application is no change to what the
//! registration is establishing), and `StartApplicationRefresh` and
//! `StartConfigurationChecks` refuse the precondition that application has not met — it
//! has not been registered successfully — in the fixed wording that names nothing of the
//! request. Both are part of the same rule: what a control call does is decided by the
//! state of the application, not by the order calls arrive in.
//!
//! The rows span what the rule generalises over: WHICH control call runs into the gate,
//! which call left the change owed (a registration still settling, a start, a stop, a
//! refresh, a settings update, a deregistration), the application type and the form of
//! the `ApplicationId`.

use std::time::Instant;

use serde_json::{json, Value};
use service_com_amazonaws_ssmsap_ssmsap::state::{
    APPLICATION_STATUS_ACTIVATED, APPLICATION_STATUS_DELETING, APPLICATION_STATUS_REGISTERING,
    APPLICATION_STATUS_STARTING, APPLICATION_STATUS_STOPPING, TRANSITION_SPAN,
};

use crate::application_lifecycle::{settle, valid_registration, Emulator, Reply};
use crate::validation_message_rendering::message;

/// The modelled error code a call against an application that already has work under way
/// on it is refused with (`smithy_query(query="operation",
/// operation="StartApplication")`: `ConflictException` @httpError 409; the other four
/// control calls model the same error).
const CONFLICT_EXCEPTION: &str = "ConflictException";

/// One call that controls a registered application: the operations that file
/// asynchronous work against one application, which is what the gate guards.
#[derive(Debug, Clone, Copy)]
enum Control {
    Start,
    Stop,
    Refresh,
    SettingsUpdate,
    ConfigurationChecks,
}

/// Every control call, so a row drives the gate through all of them.
const CONTROLS: &[Control] = &[
    Control::Start,
    Control::Stop,
    Control::Refresh,
    Control::SettingsUpdate,
    Control::ConfigurationChecks,
];

impl Control {
    /// The restJson1 path the model binds this operation to.
    fn path(self) -> &'static str {
        match self {
            Self::Start => "/start-application",
            Self::Stop => "/stop-application",
            Self::Refresh => "/start-application-refresh",
            Self::SettingsUpdate => "/update-application-settings",
            Self::ConfigurationChecks => "/start-configuration-checks",
        }
    }

    /// The smallest body this call accepts for `application`: `UpdateApplicationSettings`
    /// needs a setting to change, every other control call names only the application.
    fn request(self, application: &str) -> Value {
        match self {
            Self::SettingsUpdate => json!({
                "ApplicationId": application,
                "CredentialsToAddOrUpdate": [{
                    "DatabaseName": "TNT1",
                    "CredentialType": "ADMIN",
                    "SecretId": "sap/tnt1/admin",
                }],
            }),
            _ => json!({"ApplicationId": application}),
        }
    }

    /// How this service words the refusal of this call against an application that is in
    /// `status` with work already under way on it.
    ///
    /// Spelled out here rather than taken from the crate, so the wording is pinned
    /// independently of the one place the handlers build it in.
    fn refusal(self, application: &str, status: &str) -> String {
        match self {
            Self::Start => format!(
                "Application {application} is {status} and cannot be started until that completes"
            ),
            Self::Stop => format!(
                "Application {application} is {status} and cannot be stopped until that completes"
            ),
            Self::Refresh => format!(
                "Application {application} is {status} and cannot be refreshed until that completes"
            ),
            Self::SettingsUpdate => format!(
                "Application {application} has an operation in progress and its settings \
                 cannot be updated until that operation completes"
            ),
            Self::ConfigurationChecks => format!(
                "Application {application} has an operation in progress and configuration \
                 checks cannot be started until that operation completes"
            ),
        }
    }

    /// What this call answers against an application whose REGISTRATION is still running.
    ///
    /// A registration that has not settled is not an operation every control call has to
    /// wait for: it is the application not having been registered successfully yet, which
    /// the two calls whose precondition that is state in their own fixed wording, while a
    /// stop is served and the other two refuse the work in flight as they do for any
    /// other call.
    fn against_a_running_registration(self, application: &str, status: &str) -> Expected {
        match self {
            Self::Stop => Expected::Served,
            Self::Refresh | Self::ConfigurationChecks => {
                Expected::Refused(NOT_REGISTERED_SUCCESSFULLY.to_string())
            }
            Self::Start | Self::SettingsUpdate => {
                Expected::Refused(self.refusal(application, status))
            }
        }
    }
}

/// How this service states that the application a call names has not been registered
/// successfully: entirely fixed text, naming nothing of the request, which is what tells
/// it apart from the refusal of work in flight.
///
/// Spelled out here rather than taken from the crate, for the same reason
/// [`Control::refusal`] is.
const NOT_REGISTERED_SUCCESSFULLY: &str =
    "The specified application has not been registered successfully";

/// What a control call answers in the window a change is owed.
#[derive(Debug)]
enum Expected {
    /// HTTP 200: the call does not wait for what the application owes.
    Served,
    /// HTTP 409 `ConflictException` carrying this message.
    Refused(String),
}

/// The call that leaves a change the clock owes the application outstanding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InFlight {
    /// The registration itself has not settled yet.
    Registration,
    Start,
    Stop,
    Refresh,
    SettingsUpdate,
    /// `DeregisterApplication`, after which the application stops existing altogether.
    Deregistration,
}

impl InFlight {
    /// The request that leaves this call's change owed, and `None` for the registration,
    /// which the caller simply does not wait out.
    fn request(self, application: &str) -> Option<(&'static str, Value)> {
        match self {
            Self::Registration => None,
            Self::Start => Some((Control::Start.path(), Control::Start.request(application))),
            Self::Stop => Some((Control::Stop.path(), Control::Stop.request(application))),
            Self::Refresh => Some((
                Control::Refresh.path(),
                Control::Refresh.request(application),
            )),
            Self::SettingsUpdate => Some((
                Control::SettingsUpdate.path(),
                Control::SettingsUpdate.request(application),
            )),
            Self::Deregistration => Some((
                "/deregister-application",
                json!({"ApplicationId": application}),
            )),
        }
    }

    /// The `ApplicationStatus` the application reports while this call's change is owed:
    /// the transient status the call moved it to, or the one it already held for a call
    /// that moves no status (`state::APPLICATION_STATUS_*`).
    fn status(self) -> &'static str {
        match self {
            Self::Registration => APPLICATION_STATUS_REGISTERING,
            Self::Start => APPLICATION_STATUS_STARTING,
            Self::Stop => APPLICATION_STATUS_STOPPING,
            Self::Refresh | Self::SettingsUpdate => APPLICATION_STATUS_ACTIVATED,
            Self::Deregistration => APPLICATION_STATUS_DELETING,
        }
    }

    /// What `control` answers against an application with THIS call's change owed.
    ///
    /// Every call that runs into work in flight on a registered application is refused in
    /// the one wording this service words that condition with; a registration still
    /// running is the application not yet being registered successfully, which two of them
    /// answer on their own terms ([`Control::against_a_running_registration`]).
    fn expected(self, control: Control, application: &str) -> Expected {
        match self {
            Self::Registration => {
                control.against_a_running_registration(application, self.status())
            }
            _ => Expected::Refused(control.refusal(application, self.status())),
        }
    }
}

/// One row: an application with one call's work under way on it.
#[derive(Debug)]
struct Case {
    label: &'static str,
    /// The `ApplicationId`, spanning the forms the modelled `^[\w\d\.-]+$` admits.
    id: &'static str,
    /// An `ApplicationType` value (`smithy_query(query="shape",
    /// shape="ApplicationType")`: `HANA` | `SAP_ABAP`).
    app_type: &'static str,
    in_flight: InFlight,
}

const CASES: &[Case] = &[
    Case {
        label: "a registration that has not settled yet",
        id: "HDB-TEST",
        app_type: "HANA",
        in_flight: InFlight::Registration,
    },
    Case {
        label: "a start still running",
        id: "a",
        app_type: "HANA",
        in_flight: InFlight::Start,
    },
    Case {
        label: "a stop still running",
        id: "abap.prod_01",
        app_type: "SAP_ABAP",
        in_flight: InFlight::Stop,
    },
    Case {
        label: "a refresh still running",
        id: "HDB_02",
        app_type: "SAP_ABAP",
        in_flight: InFlight::Refresh,
    },
    Case {
        label: "a settings update still running",
        id: "hana-1",
        app_type: "HANA",
        in_flight: InFlight::SettingsUpdate,
    },
    Case {
        label: "a deregistration still running",
        id: "HDB.TEST-9",
        app_type: "HANA",
        in_flight: InFlight::Deregistration,
    },
];

/// Register one application, with no wait: the caller decides whether the registration is
/// left outstanding.
async fn register(emulator: &Emulator, label: &str, id: &str, app_type: &str) {
    let reply = emulator
        .send("/register-application", valid_registration(id, app_type))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {label:?}: expected 200 from RegisterApplication for {id:?}, got {reply:?}"
    );
}

/// The `Status` `GetApplication` reports for `id` right now.
async fn reported_status(emulator: &Emulator, id: &str) -> (Reply, String) {
    let reply = emulator
        .send("/get-application", json!({"ApplicationId": id}))
        .await;
    let status = reply.app_str("Status").to_string();
    (reply, status)
}

/// RULE: a change the clock still owes an application is an operation already under way on
/// it, so every call that controls that application is refused with the modelled
/// `ConflictException` (HTTP 409) in the one wording this service words that condition
/// with — and an application that owes no change is controlled by each of them.
#[tokio::test]
async fn every_control_call_refuses_an_application_that_owes_a_change() {
    for case in CASES {
        let label = case.label;
        let emulator = Emulator::new();

        // One settled application per control call: the side of the rule the gate must
        // leave alone, registered and waited out before the row's own application owes
        // anything.
        let idle: Vec<String> = (0..CONTROLS.len())
            .map(|index| format!("{}-idle-{index}", case.id))
            .collect();
        for application in &idle {
            register(&emulator, label, application, case.app_type).await;
        }
        settle().await;

        register(&emulator, label, case.id, case.app_type).await;
        if case.in_flight != InFlight::Registration {
            settle().await;
            let (path, body) = case
                .in_flight
                .request(case.id)
                .expect("only a registration is left outstanding by the caller");
            let reply = emulator.send(path, body).await;
            assert_eq!(
                reply.status, 200,
                "case {label:?}: expected 200 from {path}, got {reply:?}"
            );
        }
        let owed_at = Instant::now();

        // Everything a client can do in that window, before any assertion gates on the
        // clock.
        let (read, status) = reported_status(&emulator, case.id).await;
        let mut refused = Vec::new();
        let mut served = Vec::new();
        for (control, application) in CONTROLS.iter().zip(&idle) {
            refused.push(
                emulator
                    .send(control.path(), control.request(case.id))
                    .await,
            );
            served.push(emulator.send(control.path(), control.request(application)).await);
        }

        // The transient assertions hold only while the clock still owes the change: past
        // that instant it was due whether or not anybody called.
        if owed_at.elapsed() >= TRANSITION_SPAN {
            continue;
        }

        assert_eq!(
            (read.status, status.as_str()),
            (200, case.in_flight.status()),
            "case {label:?}: expected the application to report {:?} while the change is \
             owed, got {read:?}",
            case.in_flight.status()
        );
        for ((control, answered), served) in CONTROLS.iter().zip(&refused).zip(&served) {
            match case.in_flight.expected(*control, case.id) {
                Expected::Served => {
                    assert_eq!(
                        answered.status, 200,
                        "case {label:?}: expected 200 from {} while the registration is \
                         still running, got {answered:?}",
                        control.path()
                    );
                    assert!(
                        answered.member("OperationId").is_some()
                            || answered.member("ConfigurationCheckOperations").is_some(),
                        "case {label:?}: a served {} reports the work it filed, got \
                         {answered:?}",
                        control.path()
                    );
                }
                Expected::Refused(expected) => {
                    assert_eq!(
                        (answered.status, answered.code()),
                        (409, CONFLICT_EXCEPTION),
                        "case {label:?}: expected 409 {CONFLICT_EXCEPTION} from {} while a \
                         change is owed, got {answered:?}",
                        control.path()
                    );
                    assert_eq!(
                        message(answered),
                        expected,
                        "case {label:?}: expected {} to word the refusal as this service \
                         words it, got {:?}",
                        control.path(),
                        message(answered)
                    );
                    assert!(
                        answered.member("OperationId").is_none()
                            && answered.member("ConfigurationCheckOperations").is_none(),
                        "case {label:?}: a refusal from {} reports no filed work, got \
                         {answered:?}",
                        control.path()
                    );
                }
            }

            // The same call against an application that owes nothing is served, so the
            // gate refuses the condition and not the operation.
            assert_eq!(
                served.status, 200,
                "case {label:?}: expected 200 from {} against an idle application, got \
                 {served:?}",
                control.path()
            );
        }
    }
}
