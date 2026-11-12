// SPDX-License-Identifier: CC0-1.0

//! `StopApplication$IncludeEc2InstanceShutdown` really does shut the instances down.
//!
//! The rule under test: a parameter the model documents as acting on a resource of
//! ANOTHER service has to act on it there — "if set to `True`, the StopApplication
//! operation will shut down the associated Amazon EC2 instance in addition to the
//! application" (`smithy_query(query="doc",shape="StopApplicationInput",
//! member="IncludeEc2InstanceShutdown")`) — so the stop is observable where an
//! instance's state lives, `DescribeInstances`, and not only in this service's own
//! operation record. The instances associated with an application are the ones its
//! registration named: the `Instances` the SAP system runs on and the `Ec2InstanceId`
//! of each declared component.
//!
//! That rule spans two services, so both are mounted on ONE engine — through the harness
//! every suite of this target drives (`crate::application_lifecycle::Emulator::with_ec2`):
//! Systems Manager for SAP is driven over the wire the way a client drives it, and EC2 —
//! which owns the instances — through the AWS SDK over the same in-process callback the
//! handler's own shutdown uses.
//!
//! The rows span the class the rule generalises over: the flag set, unset and omitted;
//! an application with one associated instance and one with several; one instance named
//! twice by a single registration; an instance EC2 does not hold beside one it does; and
//! a shutdown asked for twice. An instance of the account that no registration named is
//! the control: it must still be running at the end of every case.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_ssmsap_ssmsap::state::TRANSITION_SPAN;
use smithy_server_runtime::EmbeddedEmulator;

use crate::application_lifecycle::REGION;

/// An AMI id a launch names; nothing here asserts anything about the image.
const AMI: &str = "ami-00112233445566778";

/// An instance id no launch in this emulator ever mints (EC2 issues `i-` and 17
/// hexadecimal digits of a counter that starts at one), so EC2 holds no such instance
/// while it satisfies the `Ec2InstanceId` pattern a registration requires.
///
/// It is named as a declared COMPONENT's instance wherever a suite needs an application
/// whose work on an instance EC2 refuses: the instance the SAP system itself runs on is
/// the one the registration's own discovery has to reach
/// (`tests/all/registration_instance_resolution.rs` pins what naming it there does), so a
/// registration that has to settle as discovered names an instance EC2 holds there.
pub(crate) const UNHELD_INSTANCE: &str = "i-0ffffffffffffffff";

/// How long a launch is given to reach `running`: the EC2 emulator moves an instance out
/// of `pending` on its own clock, a fraction of a second after the launch.
const LAUNCH_BUDGET: Duration = Duration::from_secs(10);

/// Comfortably longer than [`TRANSITION_SPAN`], so a change the clock owes an
/// application — and the one EC2 owes an instance it has just begun stopping — has come
/// due. Nothing in the emulator sleeps; this is the test waiting for the clock.
pub(crate) fn settle_wait() -> Duration {
    TRANSITION_SPAN + Duration::from_millis(400)
}

/// Systems Manager for SAP and EC2 over one shared engine.
///
/// The SAP half is the harness every suite of this target drives
/// (`crate::application_lifecycle::Emulator`), mounted with EC2 beside it; the EC2 half is
/// the AWS SDK over that harness's own in-process callback, which is the seam the
/// handler's own shutdown reaches EC2 through.
///
/// `pub(crate)` because the two suites whose rule spans both services share it: the one
/// here, about what the shutdown does to an instance, and
/// `tests/all/operation_event_resources.rs`, about what the operation reports for it.
pub(crate) struct Emulator {
    sap: crate::application_lifecycle::Emulator,
    callback: EmbeddedEmulator,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let (sap, callback) = crate::application_lifecycle::Emulator::with_ec2();
        Self { sap, callback }
    }

    /// One Systems Manager for SAP call, required to answer 200, as its response body.
    pub(crate) async fn ssm_sap(&self, path: &str, body: Value) -> Value {
        let reply = self.sap.send(path, body).await;
        assert_eq!(
            reply.status, 200,
            "case {path}: expected HTTP 200, got {reply:?}"
        );
        reply.body
    }

    /// One Systems Manager for SAP call as the wire answered it, WHATEVER its status.
    ///
    /// `pub(crate)` for the suite whose rule is about what this service refuses once the
    /// owner of an instance has answered for it
    /// (`tests/all/registration_outcome.rs`): it reads the status and the error of a
    /// reply, which [`Self::ssm_sap`] asserts away.
    pub(crate) async fn reply(
        &self,
        path: &str,
        body: Value,
    ) -> crate::application_lifecycle::Reply {
        self.sap.send(path, body).await
    }

    /// An EC2 client for [`REGION`] over the in-process callback — the same seam the
    /// handler's own shutdown reaches EC2 through.
    pub(crate) fn ec2(&self) -> ::aws_sdk_ec2::Client {
        let config = ::aws_sdk_ec2::Config::builder()
            .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
            .region(::aws_sdk_ec2::config::Region::new(REGION.to_string()))
            .endpoint_url(format!("https://ec2.{REGION}.amazonaws.com"))
            .credentials_provider(self.callback.aws_credentials_provider())
            .http_client(self.callback.http_client())
            .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
            .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                ::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        ::aws_sdk_ec2::Client::from_conf(config)
    }

    /// Launch `count` instances through EC2 itself and answer their ids, once every one
    /// of them is `running`.
    pub(crate) async fn launch_running(&self, count: i32) -> Vec<String> {
        let launched = self
            .ec2()
            .run_instances()
            .image_id(AMI)
            .min_count(count)
            .max_count(count)
            .send()
            .await
            .unwrap_or_else(|error| panic!("setup: EC2 must launch {count} instances: {error:?}"));
        let ids: Vec<String> = launched
            .instances()
            .iter()
            .filter_map(|instance| instance.instance_id().map(str::to_string))
            .collect();
        assert_eq!(
            ids.len(),
            usize::try_from(count).expect("a non-negative count"),
            "setup: expected {count} launched instance ids, got {ids:?}"
        );

        let deadline = Instant::now() + LAUNCH_BUDGET;
        loop {
            let states = self.instance_states(&ids).await;
            if states.values().all(|state| state == "running") {
                return ids;
            }
            assert!(
                Instant::now() < deadline,
                "setup: the launched instances must reach running, their states are {states:?}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// The `InstanceState$Name` EC2 reports for each of `ids`.
    pub(crate) async fn instance_states(&self, ids: &[String]) -> BTreeMap<String, String> {
        let described = self
            .ec2()
            .describe_instances()
            .set_instance_ids(Some(ids.to_vec()))
            .send()
            .await
            .unwrap_or_else(|error| panic!("EC2 must describe {ids:?}: {error:?}"));
        let states: BTreeMap<String, String> = described
            .reservations()
            .iter()
            .flat_map(::aws_sdk_ec2::types::Reservation::instances)
            .filter_map(|instance| {
                let id = instance.instance_id()?.to_string();
                let state = instance.state()?.name()?.as_str().to_string();
                Some((id, state))
            })
            .collect();
        assert_eq!(
            states.len(),
            ids.len(),
            "EC2 must report a state for each of {ids:?}, got {states:?}"
        );
        states
    }
}

/// The `RegisterApplication` body for a HANA application running on `instance`, with one
/// declared component per `(ComponentType, Ec2InstanceId)` pair in `components`.
///
/// Built on the registration builder every suite of this target shares, so the members a
/// HANA registration conditionally requires are the ones that builder supplies.
pub(crate) fn registration(id: &str, instance: &str, components: &[(&str, &str)]) -> Value {
    let mut body = crate::application_lifecycle::valid_registration(id, "HANA");
    body["Instances"] = json!([instance]);
    if !components.is_empty() {
        body["ComponentsInfo"] = Value::Array(
            components
                .iter()
                .map(|(component_type, ec2_instance_id)| {
                    json!({
                        "ComponentType": component_type,
                        // The `Sid` of the registration the shared builder writes, so a
                        // component belongs to this application's own SAP system.
                        "Sid": "HDB",
                        "Ec2InstanceId": ec2_instance_id,
                    })
                })
                .collect(),
        );
    }
    body
}

/// The `Status` of the application `id`.
async fn application_status(emulator: &Emulator, id: &str) -> String {
    let reply = emulator
        .ssm_sap("/get-application", json!({"ApplicationId": id}))
        .await;
    reply["Application"]["Status"]
        .as_str()
        .unwrap_or_else(|| panic!("case {id}: expected an Application Status, got {reply}"))
        .to_string()
}

/// The `Status` of the operation `operation_id`.
async fn operation_status(emulator: &Emulator, operation_id: &str) -> String {
    let reply = emulator
        .ssm_sap("/get-operation", json!({"OperationId": operation_id}))
        .await;
    reply["Operation"]["Status"]
        .as_str()
        .unwrap_or_else(|| panic!("case {operation_id}: expected an Operation Status, got {reply}"))
        .to_string()
}

/// Register `id` on `instance` with `components` and wait until it is `ACTIVATED`.
pub(crate) async fn register_activated(
    emulator: &Emulator,
    id: &str,
    instance: &str,
    components: &[(&str, &str)],
) {
    emulator
        .ssm_sap(
            "/register-application",
            registration(id, instance, components),
        )
        .await;
    tokio::time::sleep(settle_wait()).await;
    let status = application_status(emulator, id).await;
    assert_eq!(
        status, "ACTIVATED",
        "setup {id}: a settled registration is ACTIVATED, got {status}"
    );
}

/// Stop `id`, carrying `IncludeEc2InstanceShutdown` exactly when `flag` is `Some`, and
/// answer the `OperationId` the call filed.
pub(crate) async fn stop(emulator: &Emulator, id: &str, flag: Option<bool>) -> String {
    let mut body = json!({"ApplicationId": id});
    if let Some(shutdown) = flag {
        body["IncludeEc2InstanceShutdown"] = json!(shutdown);
    }
    let reply = emulator.ssm_sap("/stop-application", body).await;
    reply["OperationId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {id}: expected an OperationId, got {reply}"))
        .to_string()
}

/// One row of the shutdown table.
#[derive(Debug)]
struct Row {
    /// The application id the row registers.
    id: &'static str,
    /// Components declared on instances of their OWN, each launched for this row: one
    /// `ComponentType` per entry.
    component_types: &'static [&'static str],
    /// A component declared on the SAME instance the SAP system runs on, so one
    /// registration names that id twice.
    repeats_instance: bool,
    /// The `IncludeEc2InstanceShutdown` the stop carries; `None` omits the member.
    flag: Option<bool>,
    /// Whether the row's instances must be shut down.
    shuts_down: bool,
}

/// The flag drives the EC2 instances of the application it was asked for, whatever the
/// registration associated with it — and leaves every other instance of the account, and
/// every instance of an application stopped without the flag, running.
#[tokio::test]
async fn the_flag_shuts_down_the_instances_the_registration_associated() {
    // Rows: one associated instance and several; a component sharing the SAP system's
    // own instance, so an id is named twice; and the flag set to false and omitted
    // altogether, which shut nothing down.
    let rows: &[Row] = &[
        Row {
            id: "one-instance",
            component_types: &[],
            repeats_instance: false,
            flag: Some(true),
            shuts_down: true,
        },
        Row {
            id: "three-instances",
            component_types: &["WD", "ASCS"],
            repeats_instance: false,
            flag: Some(true),
            shuts_down: true,
        },
        Row {
            id: "instance-named-twice",
            component_types: &[],
            repeats_instance: true,
            flag: Some(true),
            shuts_down: true,
        },
        Row {
            id: "flag-false",
            component_types: &["ERS"],
            repeats_instance: false,
            flag: Some(false),
            shuts_down: false,
        },
        Row {
            id: "flag-omitted",
            component_types: &[],
            repeats_instance: false,
            flag: None,
            shuts_down: false,
        },
    ];

    let emulator = Emulator::new();
    // The control: an instance of the account no registration names. Nothing may stop it.
    let bystander = emulator.launch_running(1).await;

    for row in rows {
        let case = format!("{row:?}");
        let launched = emulator
            .launch_running(1 + i32::try_from(row.component_types.len()).expect("a small count"))
            .await;
        let system_instance = launched[0].clone();
        let mut components: Vec<(&str, &str)> = row
            .component_types
            .iter()
            .zip(launched.iter().skip(1))
            .map(|(component_type, instance)| (*component_type, instance.as_str()))
            .collect();
        if row.repeats_instance {
            components.push(("WD", system_instance.as_str()));
        }
        register_activated(&emulator, row.id, &system_instance, &components).await;

        let before = emulator.instance_states(&launched).await;
        assert!(
            before.values().all(|state| state == "running"),
            "case {case}: the registration must leave the instances running, got {before:?}"
        );

        let operation_id = stop(&emulator, row.id, row.flag).await;

        // The shutdown is asked for as part of the stop, so EC2 has already begun it by
        // the time the call answers — while the application itself is still STOPPING.
        let during_application = application_status(&emulator, row.id).await;
        assert_eq!(
            during_application, "STOPPING",
            "case {case}: the application must be STOPPING, got {during_application}"
        );
        let during = emulator.instance_states(&launched).await;
        let expected_during = if row.shuts_down { "stopping" } else { "running" };
        assert!(
            during.values().all(|state| state == expected_during),
            "case {case}: expected every instance {expected_during}, got {during:?}"
        );

        tokio::time::sleep(settle_wait()).await;

        let settled_application = application_status(&emulator, row.id).await;
        assert_eq!(
            settled_application, "STOPPED",
            "case {case}: the application must be STOPPED, got {settled_application}"
        );
        // The record the call filed is unchanged by the shutdown: this service reports
        // the work it did on its own resource.
        let settled_operation = operation_status(&emulator, &operation_id).await;
        assert_eq!(
            settled_operation, "SUCCESS",
            "case {case}: the stop's operation must be SUCCESS, got {settled_operation}"
        );
        let after = emulator.instance_states(&launched).await;
        let expected_after = if row.shuts_down { "stopped" } else { "running" };
        assert!(
            after.values().all(|state| state == expected_after),
            "case {case}: expected every instance {expected_after}, got {after:?}"
        );

        let control = emulator.instance_states(&bystander).await;
        assert!(
            control.values().all(|state| state == "running"),
            "case {case}: an instance no registration named must keep running, got {control:?}"
        );
    }
}

/// Each associated instance is shut down on its own: an id EC2 no longer holds does not
/// keep the application's other instances running, the application's own stop stands
/// whatever EC2 answers, and asking for the shutdown again leaves an instance already
/// stopped exactly where it is.
#[tokio::test]
async fn an_instance_ec2_does_not_hold_does_not_hold_back_the_others() {
    let emulator = Emulator::new();
    let held = emulator.launch_running(1).await;
    let system_instance = held[0].clone();

    // The SAP system is registered on an instance EC2 holds, so its discovery completes,
    // and a declared component on one EC2 holds no record of — which is the instance the
    // shutdown then cannot carry out.
    let id = "partly-held";
    register_activated(
        &emulator,
        id,
        &system_instance,
        &[("WD", UNHELD_INSTANCE)],
    )
    .await;

    let case = "an application whose component instance EC2 does not hold";
    let operation_id = stop(&emulator, id, Some(true)).await;
    let during = emulator.instance_states(&held).await;
    assert_eq!(
        during.get(&system_instance).map(String::as_str),
        Some("stopping"),
        "case {case}: the instance EC2 does hold must be stopping, got {during:?}"
    );

    tokio::time::sleep(settle_wait()).await;
    let status = application_status(&emulator, id).await;
    assert_eq!(
        status, "STOPPED",
        "case {case}: the application's own stop stands, got {status}"
    );
    let operation = operation_status(&emulator, &operation_id).await;
    assert_eq!(
        operation, "SUCCESS",
        "case {case}: the stop's operation must be SUCCESS, got {operation}"
    );
    let after = emulator.instance_states(&held).await;
    assert_eq!(
        after.get(&system_instance).map(String::as_str),
        Some("stopped"),
        "case {case}: the instance EC2 does hold must be stopped, got {after:?}"
    );

    // Asked for a second time, with the instance already stopped: the call succeeds and
    // the instance stays where it is.
    let case = "a shutdown asked for twice";
    let again = stop(&emulator, id, Some(true)).await;
    let repeated = emulator.instance_states(&held).await;
    assert_eq!(
        repeated.get(&system_instance).map(String::as_str),
        Some("stopped"),
        "case {case}: an instance already stopped stays stopped, got {repeated:?}"
    );
    tokio::time::sleep(settle_wait()).await;
    let operation = operation_status(&emulator, &again).await;
    assert_eq!(
        operation, "SUCCESS",
        "case {case}: the second stop's operation must be SUCCESS, got {operation}"
    );
}
