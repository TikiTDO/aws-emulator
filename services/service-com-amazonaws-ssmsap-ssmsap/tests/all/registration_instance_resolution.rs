// SPDX-License-Identifier: CC0-1.0

//! What a registration does about the Amazon EC2 instances it names: it STORES them and
//! refuses nothing synchronously — and the DISCOVERY it then runs on them is what decides
//! the outcome it settles as.
//!
//! RULE under test: `RegisterApplication` is asynchronous and accepts the Amazon EC2
//! instance ids it is given on their modelled form alone
//! (`smithy_query(query="shape", shape="InstanceId")`: `^i-[\w\d]{8}$|^i-[\w\d]{17}$`) —
//! it refuses nothing for an id the service that owns instances holds no record of, so
//! such a registration is SERVED, the application it creates is readable, and the ids it
//! named are the ids the stored record reports. What that owner answers decides the
//! asynchronous OUTCOME instead: the discovery of the registration runs a Systems Manager
//! document on the instance, so an instance the owner reports running is discovered
//! (`ACTIVATED`/`SUCCESS`) and one it holds no record of, or will not read as an instance
//! id at all, is a discovery that could not be carried out
//! (`FAILED`/`REGISTRATION_FAILED`). This service invents no instance state of its own
//! either way.
//!
//! WHY the CALL is served rather than refused, as the modelled
//! `ResourceNotFoundException` would make it easy to read: the recorded behaviour of this
//! service registers applications against instance ids nothing ever launched, and the
//! call succeeds — the
//! behavioural units `ssm-sap.deregister-application-removes-resource`,
//! `ssm-sap.get-application-missing-identifier-rejected`,
//! `ssm-sap.get-application-nonexistent-id-validation-error` and
//! `ssm-sap.get-operation-invalid-uuid-format-rejected` each register on a freshly
//! generated id of exactly that kind. This suite is the guardrail for it: a later change
//! that starts refusing such a registration fails here, beside the rule, rather than only
//! against the corpus.
//!
//! The rows span what the rule generalises over: both lengths the modelled id form
//! admits, ids the owning service would answer as unknown and ids it would not read as an
//! instance id at all, an id that owner really does hold, which member names it
//! (`Instances`, a declared component's `Ec2InstanceId`) — and whether that owner is
//! mounted beside this service at all, since what this service can CONCLUDE about an
//! instance is only what that owner answered.

use serde_json::{json, Value};

use crate::application_lifecycle::{settle, valid_registration};
use crate::ec2_instance_shutdown::{registration, settle_wait, Emulator};

/// Instance ids the owner of an instance holds nothing for, spanning the answers it has
/// for one.
///
/// The first two are well formed — the two lengths EC2 has issued resource ids in — and
/// name no instance, since nothing in this emulator launches an all-`f` counter value. The
/// rest are outside the form EC2 reads an instance id in at all (a long form whose leading
/// digit is above zero, the all-zero placeholder, a body that is not hexadecimal) while
/// still inside the `InstanceId` pattern this service's own model declares, so each one
/// reaches the handler.
const INSTANCES_NOBODY_HOLDS: &[(&str, &str)] = &[
    (
        "a long-form id nothing was launched for",
        "i-0ffffffffffffffff",
    ),
    ("a short-form id nothing was launched for", "i-ffffffff"),
    (
        "a long-form id whose leading digit is above zero",
        "i-1234567890abcdef0",
    ),
    ("the all-zero placeholder id", "i-00000000000000000"),
    ("a body that is not hexadecimal", "i-0zzzzzzzzzzzzzzzz"),
];

/// A string member of the `Operation` the registration filed, read back through
/// `GetOperation` — so what an accepted registration stored is read out of persisted state
/// rather than out of the registering call's own answer.
async fn recorded_property(
    emulator: &Emulator,
    case: &str,
    operation_id: &str,
    property: &str,
) -> String {
    let reply = emulator
        .ssm_sap("/get-operation", json!({"OperationId": operation_id}))
        .await;
    reply
        .get("Operation")
        .and_then(|operation| operation.get("Properties"))
        .and_then(|properties| properties.get(property))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the operation to report {property}, got {reply}")
        })
        .to_string()
}

/// The `OperationId` a registration answered with.
fn operation_id(case: &str, reply: &Value) -> String {
    reply
        .get("OperationId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an OperationId, got {reply}"))
        .to_string()
}

/// RULE: with the owner of the instances mounted beside this service, a registration
/// naming an instance that owner holds nothing for is still served — the instance it
/// named is what the stored record reports — and what that registration SETTLES as is
/// what the discovery on that instance could do.
#[tokio::test]
async fn a_registration_is_served_whatever_the_owner_of_the_instance_holds() {
    let emulator = Emulator::new();
    // The control: an id that owner really does hold, so the rows differ in what EC2
    // would answer and in nothing else.
    let launched = emulator.launch_running(1).await[0].clone();
    let mut rows: Vec<(String, String, &str, &str)> = INSTANCES_NOBODY_HOLDS
        .iter()
        .map(|(label, instance_id)| {
            (
                (*label).to_string(),
                (*instance_id).to_string(),
                "FAILED",
                "REGISTRATION_FAILED",
            )
        })
        .collect();
    rows.push((
        "an id the owner does hold".to_string(),
        launched,
        "ACTIVATED",
        "SUCCESS",
    ));

    for (index, (label, instance_id, status, discovery_status)) in rows.iter().enumerate() {
        let application_id = format!("stored-instance-{index}");
        let reply = emulator
            .ssm_sap(
                "/register-application",
                registration(&application_id, instance_id, &[]),
            )
            .await;

        // The registration is under way, as an asynchronous call's answer reports it:
        // nothing about the instance is refused here, and no outcome is reported yet.
        assert_eq!(
            reply.get("Application").and_then(|app| app.get("Status")),
            Some(&json!("REGISTERING")),
            "case {label:?}: expected a REGISTERING application, got {reply}"
        );

        let recorded = recorded_property(
            &emulator,
            label,
            &operation_id(label, &reply),
            "Instances",
        )
        .await;
        assert_eq!(
            &recorded, instance_id,
            "case {label:?}: expected the record to report the instance the registration \
             named, got {recorded:?}"
        );

        // What the clock owes the registration is the outcome of its discovery on that
        // instance, which is what the owner of the instance answered for.
        tokio::time::sleep(settle_wait()).await;
        let fetched = emulator
            .ssm_sap(
                "/get-application",
                json!({"ApplicationId": &application_id}),
            )
            .await;
        let application = fetched.get("Application");
        assert_eq!(
            (
                application.and_then(|app| app.get("Status")),
                application.and_then(|app| app.get("DiscoveryStatus")),
            ),
            (Some(&json!(status)), Some(&json!(discovery_status))),
            "case {label:?}: expected {status}/{discovery_status}, got {fetched}"
        );
    }
}

/// RULE: the same holds for the instance a DECLARED COMPONENT names, which is the other
/// place a registration associates an instance with the application.
#[tokio::test]
async fn a_component_on_an_instance_the_owner_holds_nothing_for_is_stored_too() {
    let emulator = Emulator::new();
    let system_instance = emulator.launch_running(1).await[0].clone();

    for (index, (label, instance_id)) in INSTANCES_NOBODY_HOLDS.iter().enumerate() {
        let case = format!("{label} as a component's Ec2InstanceId");
        let application_id = format!("stored-component-{index}");
        let reply = emulator
            .ssm_sap(
                "/register-application",
                registration(
                    &application_id,
                    &system_instance,
                    &[("WD", instance_id)],
                ),
            )
            .await;

        // The operation record names each declared component by its id and the instance
        // the declaration put it on, so the instance the component named is in there.
        let recorded = recorded_property(
            &emulator,
            &case,
            &operation_id(&case, &reply),
            "ComponentsInfo",
        )
        .await;
        assert!(
            recorded.contains(instance_id),
            "case {case:?}: expected the record to report the component's instance \
             {instance_id:?}, got {recorded:?}"
        );

        // The component is one of the application's own once the discovery the
        // registration runs has produced it, reported by the listing that walks them.
        tokio::time::sleep(settle_wait()).await;
        let components = emulator
            .ssm_sap(
                "/list-components",
                json!({"ApplicationId": &application_id}),
            )
            .await;
        let listed = components
            .get("Components")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            listed
                .iter()
                .any(|component| component.get("ComponentId") == Some(&json!("HDB-WD"))),
            "case {case:?}: expected the declared component to be listed, got {components}"
        );
    }
}

/// RULE: with no owner of the instances mounted beside this service there is nothing to
/// answer for one, so this service concludes nothing about it: the same registrations are
/// served and settle as discovered, which is what lets a caller that never launched an
/// instance register against an id of the modelled form.
#[tokio::test]
async fn a_registration_is_served_where_the_owner_of_the_instance_is_not_mounted() {
    let emulator = crate::application_lifecycle::Emulator::new();

    for (index, (label, instance_id)) in INSTANCES_NOBODY_HOLDS.iter().enumerate() {
        let application_id = format!("no-owner-{index}");
        let mut body = valid_registration(&application_id, "HANA");
        body["Instances"] = json!([instance_id]);

        let reply = emulator.send("/register-application", body).await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: expected 200 from RegisterApplication, got {reply:?}"
        );
        settle().await;

        let fetched = emulator
            .send(
                "/get-application",
                json!({"ApplicationId": &application_id}),
            )
            .await;
        assert_eq!(
            fetched.app("Id").and_then(Value::as_str),
            Some(application_id.as_str()),
            "case {label:?}: expected the registration to be readable, got {fetched:?}"
        );
        assert_eq!(
            fetched.app_str("Status"),
            "ACTIVATED",
            "case {label:?}: expected an ACTIVATED application, got {fetched:?}"
        );
    }
}
