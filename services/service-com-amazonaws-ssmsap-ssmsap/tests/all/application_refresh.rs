// SPDX-License-Identifier: CC0-1.0

//! `StartApplicationRefresh`, driven over the wire.
//!
//! Every case goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns), so the restJson1 routing, the boundary constraint walk, the handler and the
//! response serialization all run exactly as they do in the emulator binary. Nothing
//! reaches into `crate::state` to assert a behaviour: a refresh is only believed when a
//! later `GetApplication` / `GetOperation` / `ListOperations` / `ListDatabases` call
//! reports it back.
//!
//! RULE under test: a call that re-runs a registered application's DISCOVERY returns at
//! once with the `OperationId` of a persisted record of that work, and until the next
//! discovery result the clock owes it falls due the application reports the discovering
//! `ApplicationDiscoveryStatus` to every reader — while the `ApplicationStatus` of the SAP
//! system, which such a call neither starts nor stops, is left exactly as it was, and
//! what the registration derived (its components, its databases) is unchanged by the
//! re-discovery.
//!
//! The rows span what that rule generalises over: the application type, the forms the
//! modelled `ApplicationId` pattern admits, and the status the application is in when the
//! refresh starts (activated by its registration, started, stopped) — so the status the
//! refresh preserves is a different one in each row.

use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::{
    APPLICATION_DISCOVERY_STATUS_VALUES, APPLICATION_STATUS_VALUES,
};
use service_com_amazonaws_ssmsap_ssmsap::state::{
    APPLICATION_STATUS_ACTIVATED, APPLICATION_STATUS_DELETING, APPLICATION_STATUS_REGISTERING,
    APPLICATION_STATUS_STARTING, APPLICATION_STATUS_STOPPED, APPLICATION_STATUS_STOPPING,
    DISCOVERY_STATUS_DELETING, DISCOVERY_STATUS_REGISTERING, DISCOVERY_STATUS_SUCCESS,
    OPERATION_TYPE_START_APPLICATION_REFRESH, PendingChange, PendingTransition, TRANSITION_SPAN,
};

use crate::application_lifecycle::{
    is_operation_id, settle, valid_registration, Emulator, Reply, REGION,
};
use crate::validation_message_rendering::message;

/// The account every emulated ARN carries, from the core's single source of truth.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The modelled error code an operation already under way on an application is refused
/// with (`smithy_query(query="operation", operation="StartApplicationRefresh")`:
/// `ConflictException` `@httpError 409`).
const CONFLICT_EXCEPTION: &str = "ConflictException";

/// The modelled error code an `ApplicationId` naming no registered application is refused
/// with (`smithy_query(query="operation", operation="StartApplicationRefresh")`:
/// `ResourceNotFoundException` `@httpError 404`).
const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";

/// `OperationStatus$INPROGRESS`, as the model spells it.
const OPERATION_STATUS_INPROGRESS: &str = "INPROGRESS";

/// `OperationStatus$SUCCESS`, as the model spells it.
const OPERATION_STATUS_SUCCESS: &str = "SUCCESS";

/// What the application has been through by the time it is refreshed.
#[derive(Debug, Clone, Copy)]
enum Before {
    /// Nothing beyond its settled registration, so it is activated.
    Activated,
    /// A settled `StartApplication` on top of that.
    Started,
    /// A settled `StopApplication` on top of that, so the application is stopped.
    Stopped,
}

impl Before {
    /// The `ApplicationStatus` the application reports before the refresh — and, because
    /// a refresh re-runs discovery alone, after it as well.
    fn status(self) -> &'static str {
        match self {
            Before::Activated | Before::Started => APPLICATION_STATUS_ACTIVATED,
            Before::Stopped => APPLICATION_STATUS_STOPPED,
        }
    }
}

/// One application to refresh.
#[derive(Debug)]
struct Case {
    label: &'static str,
    /// The `ApplicationId`, spanning the forms the modelled `^[\w\d\.-]+$` admits.
    id: &'static str,
    /// An `ApplicationType` value (`smithy_query(query="shape",
    /// shape="ApplicationType")`: `HANA` | `SAP_ABAP`).
    app_type: &'static str,
    before: Before,
}

/// The rows: the status the application is in when the refresh starts, across both
/// application types, with ids of different forms.
const CASES: &[Case] = &[
    Case {
        label: "an activated HANA application",
        id: "HDB",
        app_type: "HANA",
        before: Before::Activated,
    },
    Case {
        label: "a started SAP_ABAP application with a dotted id",
        id: "abap.prod_01",
        app_type: "SAP_ABAP",
        before: Before::Started,
    },
    Case {
        label: "a stopped HANA application with a hyphenated id",
        id: "HDB-TEST",
        app_type: "HANA",
        before: Before::Stopped,
    },
    Case {
        label: "an application whose id is the shortest the pattern admits",
        id: "a",
        app_type: "SAP_ABAP",
        before: Before::Activated,
    },
];

impl Emulator {
    /// Register `case`'s application, bring it to the status `case.before` names, and
    /// leave no change outstanding.
    async fn register_for(&self, case: &Case) {
        let label = case.label;
        let registered = self
            .send(
                "/register-application",
                valid_registration(case.id, case.app_type),
            )
            .await;
        assert_eq!(
            registered.status, 200,
            "case {label:?}: expected 200 from RegisterApplication, got {registered:?}"
        );
        settle().await;
        let path = match case.before {
            Before::Activated => return,
            Before::Started => "/start-application",
            Before::Stopped => "/stop-application",
        };
        let moved = self.send(path, json!({"ApplicationId": case.id})).await;
        assert_eq!(
            moved.status, 200,
            "case {label:?}: expected 200 from {path}, got {moved:?}"
        );
        settle().await;
    }

    async fn refresh(&self, id: &str) -> Reply {
        self.send("/start-application-refresh", json!({"ApplicationId": id}))
            .await
    }

    async fn read_application(&self, id: &str) -> Reply {
        self.send("/get-application", json!({"ApplicationId": id}))
            .await
    }

    async fn refresh_record(&self, operation_id: &str) -> Reply {
        self.send("/get-operation", json!({"OperationId": operation_id}))
            .await
    }

    /// How many `START_APPLICATION_REFRESH` records this Region holds against
    /// `application_id`.
    ///
    /// `ListOperations` is still a scaffold stub in this crate, so a record the service
    /// filed for a call whose reply handed out no id (a refused refresh) is reachable
    /// nowhere over the wire — hence this ONE store read. Everything asserted about a
    /// record the service did hand an id for goes over the wire through `GetOperation`.
    fn refresh_record_count(&self, application_id: &str) -> usize {
        let mut count = 0;
        self.with_state(|state| {
            count = state
                .region(REGION)
                .operations
                .values()
                .filter(|record| {
                    record.op_type == OPERATION_TYPE_START_APPLICATION_REFRESH
                        && record.resource_id == application_id
                })
                .count();
        });
        count
    }

    /// The `DatabaseId`s `ListDatabases` reports for `id`, sorted.
    async fn listed_database_ids(&self, id: &str) -> Vec<String> {
        let reply = self
            .send("/list-databases", json!({"ApplicationId": id}))
            .await;
        let mut ids: Vec<String> = reply
            .member("Databases")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("DatabaseId").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        ids.sort();
        ids
    }
}

/// The `OperationId` a reply handed out.
fn operation_id(label: &str, reply: &Reply) -> String {
    reply
        .member("OperationId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {label:?}: expected an OperationId, got {reply:?}"))
        .to_string()
}

/// The `Components` an `Application` reports, sorted.
fn components(reply: &Reply) -> Vec<String> {
    let mut ids: Vec<String> = reply
        .app("Components")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids
}

/// The ARN the registration of `id` mints in [`REGION`].
fn expected_arn(id: &str) -> String {
    format!("arn:aws:ssm-sap:{REGION}:{ACCOUNT}:application/{id}")
}

#[tokio::test]
async fn a_refresh_rediscovers_the_application_and_files_a_record() {
    // The statuses a refresh reports are the model's own
    // (`smithy_query(query="shape", shape="ApplicationDiscoveryStatus,ApplicationStatus")`),
    // so no case can pass on a value the API does not define.
    assert!(
        APPLICATION_DISCOVERY_STATUS_VALUES.contains(&DISCOVERY_STATUS_REGISTERING)
            && APPLICATION_DISCOVERY_STATUS_VALUES.contains(&DISCOVERY_STATUS_SUCCESS),
        "the discovery statuses must be modelled values, got {DISCOVERY_STATUS_REGISTERING:?} \
         and {DISCOVERY_STATUS_SUCCESS:?}"
    );

    for case in CASES {
        let label = case.label;
        assert!(
            APPLICATION_STATUS_VALUES.contains(&case.before.status()),
            "case {label:?}: the preserved status must be a modelled value, got {:?}",
            case.before.status()
        );

        let emulator = Emulator::new();
        emulator.register_for(case).await;
        // A second application, refreshed by nothing, proves the work is scoped to the
        // application the call names.
        let untouched = format!("{}-keep", case.id);
        let other = emulator
            .send(
                "/register-application",
                valid_registration(&untouched, case.app_type),
            )
            .await;
        assert_eq!(
            other.status, 200,
            "case {label:?}: expected 200 registering the untouched application, got {other:?}"
        );
        settle().await;

        let before = emulator.read_application(case.id).await;
        assert_eq!(
            (
                before.status,
                before.app_str("Status"),
                before.app_str("DiscoveryStatus")
            ),
            (200, case.before.status(), DISCOVERY_STATUS_SUCCESS),
            "case {label:?}: the application starts out discovered and in its own status, \
             got {before:?}"
        );
        let components_before = components(&before);
        let databases_before = emulator.listed_database_ids(case.id).await;

        let refreshed = emulator.refresh(case.id).await;
        let started_at = Instant::now();
        assert_eq!(
            refreshed.status, 200,
            "case {label:?}: expected 200 from StartApplicationRefresh, got {refreshed:?}"
        );
        let record_id = operation_id(label, &refreshed);
        assert!(
            is_operation_id(&record_id),
            "case {label:?}: the OperationId must be the modelled UUID form, got {record_id:?}"
        );
        assert_eq!(
            emulator.refresh_record_count(case.id),
            1,
            "case {label:?}: one refresh files exactly one record"
        );

        // Everything a client can do while the re-discovery runs, before any assertion
        // gates on the clock.
        let during = emulator.read_application(case.id).await;
        let during_record = emulator.refresh_record(&record_id).await;
        let again = emulator.refresh(case.id).await;
        let neighbour = emulator.read_application(&untouched).await;

        // The transient assertions hold only while the clock still owes the next discovery
        // result: past that instant it was due whether or not anybody called.
        if started_at.elapsed() < TRANSITION_SPAN {
            assert_eq!(
                during.app_str("DiscoveryStatus"),
                DISCOVERY_STATUS_REGISTERING,
                "case {label:?}: a refreshing application is discovering, got {during:?}"
            );
            assert_eq!(
                during.app_str("Status"),
                case.before.status(),
                "case {label:?}: a refresh leaves the application's own status alone, got \
                 {during:?}"
            );
            assert_eq!(
                during.app_str("Arn"),
                expected_arn(case.id),
                "case {label:?}: the refreshing application keeps its ARN, got {during:?}"
            );
            assert_eq!(
                components(&during),
                components_before,
                "case {label:?}: a refresh re-discovers the same components, got {during:?}"
            );

            assert_eq!(
                (during_record.status, during_record.op_str("Status")),
                (200, OPERATION_STATUS_INPROGRESS),
                "case {label:?}: the refresh's record resolves and is running, got \
                 {during_record:?}"
            );
            assert!(
                during_record.op("EndTime").is_none(),
                "case {label:?}: an operation still running has no end time, got \
                 {during_record:?}"
            );

            assert_eq!(
                (again.status, again.code()),
                (409, CONFLICT_EXCEPTION),
                "case {label:?}: a second refresh is refused while the first runs, got {again:?}"
            );
            assert!(
                message(&again).contains(case.id),
                "case {label:?}: the refusal must name the application, got {:?}",
                message(&again)
            );

            assert_eq!(
                (
                    neighbour.app_str("Status"),
                    neighbour.app_str("DiscoveryStatus")
                ),
                (APPLICATION_STATUS_ACTIVATED, DISCOVERY_STATUS_SUCCESS),
                "case {label:?}: the application nothing refreshed is untouched, got \
                 {neighbour:?}"
            );
        }

        // Past the due instant the discovery has completed, without any further call
        // having been needed to make it so.
        settle().await;

        let after = emulator.read_application(case.id).await;
        assert_eq!(
            (
                after.status,
                after.app_str("DiscoveryStatus"),
                after.app_str("Status"),
            ),
            (200, DISCOVERY_STATUS_SUCCESS, case.before.status()),
            "case {label:?}: the completed refresh reports a discovered application still in \
             its own status, got {after:?}"
        );
        assert_eq!(
            components(&after),
            components_before,
            "case {label:?}: the re-discovery changed no component, got {after:?}"
        );
        assert_eq!(
            emulator.listed_database_ids(case.id).await,
            databases_before,
            "case {label:?}: the re-discovery changed no database"
        );

        let record = emulator.refresh_record(&record_id).await;
        assert_eq!(
            (
                record.status,
                record.op_str("Type"),
                record.op_str("Status"),
                record.op_str("ResourceId"),
                record.op_str("ResourceArn"),
            ),
            (
                200,
                OPERATION_TYPE_START_APPLICATION_REFRESH,
                OPERATION_STATUS_SUCCESS,
                case.id,
                expected_arn(case.id).as_str(),
            ),
            "case {label:?}: the record must describe the refresh it was filed for, got \
             {record:?}"
        );
        assert!(
            record.op("EndTime").is_some(),
            "case {label:?}: a completed operation reports when it ended, got {record:?}"
        );

        // A refresh asked once the previous one has completed is served, and files its own
        // record: re-discovery is work a client may repeat.
        let repeated = emulator.refresh(case.id).await;
        assert_eq!(
            repeated.status, 200,
            "case {label:?}: a refresh after the previous one completed is served, got \
             {repeated:?}"
        );
        let repeated_id = operation_id(label, &repeated);
        assert_ne!(
            repeated_id, record_id,
            "case {label:?}: each refresh files a record of its own, got {repeated_id:?}"
        );
        assert_eq!(
            emulator.refresh_record_count(case.id),
            2,
            "case {label:?}: the second refresh files a second record"
        );
        assert_eq!(
            emulator.refresh_record_count(&untouched),
            0,
            "case {label:?}: no record is filed against the application nothing refreshed"
        );
    }
}

#[tokio::test]
async fn a_refresh_of_an_unregistered_application_is_refused() {
    // The references span the modelled `ApplicationId` class (`@length(1, 60)`,
    // `@pattern ^[\w\d\.-]+$`): the shortest, the longest, every character class the
    // pattern admits, and a registered id spelled in another case.
    let registered = "HDB";
    let references = [
        "a".to_string(),
        "x".repeat(60),
        "No_Such.App-01".to_string(),
        registered.to_lowercase(),
    ];

    let emulator = Emulator::new();
    let created = emulator
        .send(
            "/register-application",
            valid_registration(registered, "HANA"),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "expected 200 from RegisterApplication, got {created:?}"
    );
    settle().await;

    for reference in &references {
        let reply = emulator.refresh(reference).await;
        assert_eq!(
            (reply.status, reply.code()),
            (404, RESOURCE_NOT_FOUND_EXCEPTION),
            "case {reference:?}: an unregistered application is a not-found refusal, got \
             {reply:?}"
        );
        assert_eq!(
            message(&reply),
            format!("Application {reference} doesn't exist."),
            "case {reference:?}: the refusal must name the reference the request carried, got \
             {reply:?}"
        );
        assert!(
            reply.member("OperationId").is_none(),
            "case {reference:?}: a refused refresh hands out no OperationId, got {reply:?}"
        );
        assert_eq!(
            emulator.refresh_record_count(reference),
            0,
            "case {reference:?}: a refused refresh files no record"
        );
        assert_eq!(
            emulator.refresh_record_count(registered),
            0,
            "case {reference:?}: nor against the application that is registered"
        );
    }

    // The registered application is still discovered: nothing the refusals did touched it.
    let unchanged = emulator.read_application(registered).await;
    assert_eq!(
        unchanged.app_str("DiscoveryStatus"),
        DISCOVERY_STATUS_SUCCESS,
        "a refused refresh leaves the registered application alone, got {unchanged:?}"
    );
}

/// One change the clock owes an application when a refresh is asked for: the transient
/// status it reports meanwhile, and what it was going to become.
struct Owed {
    label: &'static str,
    /// The `ApplicationStatus` the application reports while the change is outstanding.
    status: &'static str,
    /// The `ApplicationDiscoveryStatus` it reports meanwhile.
    discovery_status: &'static str,
    /// What the clock owes it, built against the due instant the case installs.
    change: fn() -> PendingChange,
}

/// The rows: every change this service's asynchronous calls leave outstanding — the next
/// status a registration, a start and a stop owe, and the removal a deregistration owes.
fn owed_changes() -> Vec<Owed> {
    vec![
        Owed {
            label: "a registration still running",
            status: APPLICATION_STATUS_REGISTERING,
            discovery_status: DISCOVERY_STATUS_REGISTERING,
            change: || PendingChange::Status {
                status: APPLICATION_STATUS_ACTIVATED.to_string(),
                discovery_status: DISCOVERY_STATUS_SUCCESS.to_string(),
            },
        },
        Owed {
            label: "a start still running",
            status: APPLICATION_STATUS_STARTING,
            discovery_status: DISCOVERY_STATUS_SUCCESS,
            change: || PendingChange::Status {
                status: APPLICATION_STATUS_ACTIVATED.to_string(),
                discovery_status: DISCOVERY_STATUS_SUCCESS.to_string(),
            },
        },
        Owed {
            label: "a stop still running",
            status: APPLICATION_STATUS_STOPPING,
            discovery_status: DISCOVERY_STATUS_SUCCESS,
            change: || PendingChange::Status {
                status: APPLICATION_STATUS_STOPPED.to_string(),
                discovery_status: DISCOVERY_STATUS_SUCCESS.to_string(),
            },
        },
        Owed {
            label: "a deregistration still running",
            status: APPLICATION_STATUS_DELETING,
            discovery_status: DISCOVERY_STATUS_DELETING,
            change: || PendingChange::Removal,
        },
    ]
}

/// How far out the owed change is placed: long enough that no machine this suite runs on
/// can reach the due instant between the two calls below, so the refusal is pinned by the
/// state the application is in rather than by how fast the test ran.
const FAR_FUTURE: Duration = Duration::from_secs(3_600);

#[tokio::test]
async fn a_refresh_is_refused_while_a_change_is_already_owed() {
    let id = "HDB";
    for owed in owed_changes() {
        let label = owed.label;
        let emulator = Emulator::new();
        let created = emulator
            .send("/register-application", valid_registration(id, "HANA"))
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected 200 from RegisterApplication, got {created:?}"
        );
        settle().await;
        // The registration's own change has fallen due, and the first request at or after
        // that instant is what materialises it: this read is that request, so the
        // application the edit below starts from is the DISCOVERED one every other caller
        // sees rather than one whose registration never settled.
        let discovered = emulator.read_application(id).await;
        assert_eq!(
            discovered.app_str("DiscoveryStatus"),
            DISCOVERY_STATUS_SUCCESS,
            "case {label:?}: the registration must have settled, got {discovered:?}"
        );

        // The store is edited rather than driven over the wire ONLY to place the due
        // instant: every asynchronous call of this service makes the change due one
        // `TRANSITION_SPAN` out, which no test can outrun deterministically. What is
        // installed is exactly what those calls install — the transient statuses and the
        // change the clock owes — and everything asserted about the refusal goes over the
        // wire.
        let due_at = SystemTime::now() + FAR_FUTURE;
        emulator.with_state(|state| {
            let application = state
                .region_entry(REGION)
                .applications
                .get_mut(id)
                .expect("the application registered just above");
            application.status = owed.status.to_string();
            application.discovery_status = owed.discovery_status.to_string();
            application.pending = Some(PendingTransition {
                due_at,
                change: (owed.change)(),
            });
        });

        let refused = emulator.refresh(id).await;
        assert_eq!(
            (refused.status, refused.code()),
            (409, CONFLICT_EXCEPTION),
            "case {label:?}: a refresh is refused while a change is owed, got {refused:?}"
        );
        let refusal = message(&refused);
        assert!(
            refusal.contains(id) && refusal.contains(owed.status),
            "case {label:?}: the refusal must name the application and the status it is in, \
             got {refusal:?}"
        );
        assert!(
            refused.member("OperationId").is_none(),
            "case {label:?}: a refused refresh hands out no OperationId, got {refused:?}"
        );
        assert_eq!(
            emulator.refresh_record_count(id),
            0,
            "case {label:?}: a refused refresh files no record"
        );

        // The refusal left the application exactly as it was: the refresh neither
        // re-discovered it nor moved what the clock still owes it.
        let after = emulator.read_application(id).await;
        assert_eq!(
            after.app_str("DiscoveryStatus"),
            owed.discovery_status,
            "case {label:?}: the refused refresh changed no discovery status, got {after:?}"
        );
        assert_eq!(
            after.app_str("Status"),
            owed.status,
            "case {label:?}: the refused refresh changed no status, got {after:?}"
        );
    }
}
