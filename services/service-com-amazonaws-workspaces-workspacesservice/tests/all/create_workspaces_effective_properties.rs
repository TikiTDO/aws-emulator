// SPDX-License-Identifier: CC0-1.0

//! The EFFECTIVE `WorkspaceProperties` of a `CreateWorkspaces` element, end to end over
//! the awsJson1_1 wire, and which resource a per-element refusal names.
//!
//! Every case drives a real `MultiServiceRouter` with this service mounted on one shared
//! engine, so each assertion is about what a client would see.
//!
//! The two rules under test, stated over the SET of elements a client can submit rather
//! than over any one recorded request:
//!
//! 1. Wherever this service reports the properties of a WorkSpace a request asked for —
//!    the `WorkspaceProperties` echoed in a `FailedRequests` entry, the one reported for a
//!    created WorkSpace in `PendingRequests`, and the one `DescribeWorkspaces` reports for
//!    it later — it reports the EFFECTIVE property set: every property the request
//!    submitted, unchanged, plus the value the service resolves for a property the request
//!    left to it (an omitted `RunningMode` becomes [`DEFAULT_RUNNING_MODE`], an omitted
//!    `RecycleMode` becomes `DEFAULT_RECYCLE_MODE`). It is never absent, so a client that
//!    retries a partial batch by resubmitting an echoed `WorkspaceRequest` asks for the
//!    same WorkSpace.
//! 2. A per-element refusal names the resource that actually failed to resolve:
//!    `ResourceNotFound.Bundle` for a `BundleId` the store does not hold and
//!    `ResourceNotFound.Directory` for a `DirectoryId` no `RegisterWorkspaceDirectory`
//!    registered, each beside the fixed sentence AWS answers for that class. An element
//!    with BOTH unresolvable has two failures and one `ErrorCode`, and reports the first
//!    in the model's own member order (`WorkspaceRequest` declares `DirectoryId` before
//!    `BundleId`).
//!
//! The property values the rows carry come from the model: the `RunningMode` values from
//! `RUNNING_MODE_VALUES` (the `RunningMode` enum), the compute names from
//! `COMPUTE_VALUES`, the protocols from `PROTOCOL_VALUES` and the operating-system names
//! from `OPERATING_SYSTEM_NAME_VALUES`, all re-exported by this crate's `ops::types`.

use serde_json::{json, Value};
use service_com_amazonaws_workspaces_workspacesservice::ops::types::{
    A_G_A_MODE_FOR_WORK_SPACE_ENUM_VALUES, A_G_A_PREFERRED_PROTOCOL_FOR_WORK_SPACE_VALUES,
    COMPUTE_VALUES, OPERATING_SYSTEM_NAME_VALUES, PROTOCOL_VALUES, RUNNING_MODE_VALUES,
};
use service_com_amazonaws_workspaces_workspacesservice::state::DEFAULT_RUNNING_MODE;
use smithy_server_runtime::MultiServiceRouter;

use crate::create_workspaces_validation::amazon_bundle_ids;
use crate::workspace_lifecycle::{
    call, describe_one, effective_properties, emulator_with_engine, register_directory, settle,
};

/// A directory this suite registers, so an element naming it is satisfiable. Matches the
/// model's `DirectoryId` pattern `^(d-[0-9a-f]{8,63}$)|(wsd-[0-9a-z]{8,63}$)$`.
const REGISTERED_DIRECTORY: &str = "d-0ff1ce0b0b";
/// A directory nobody registers, so an element naming it cannot be satisfied.
const UNREGISTERED_DIRECTORY: &str = "wsd-0unregistered";

/// The submitted `WorkspaceProperties` of one row, and the effective set it must be
/// reported as.
///
/// The rows differ along what rule 1 generalises over: whether the member is submitted at
/// all, which of the enum's running modes it names, how many of the other properties ride
/// along, and the types those properties have (enum, integer, boolean, list, structure).
fn property_table() -> Vec<(&'static str, Option<Value>, Value)> {
    // Catalogued values, taken from the model's own enums rather than written by hand.
    let running_modes = RUNNING_MODE_VALUES;
    assert!(
        running_modes.contains(&DEFAULT_RUNNING_MODE),
        "the default running mode must be one the model's RunningMode enum declares, got \
         {running_modes:?}"
    );
    let compute = COMPUTE_VALUES
        .last()
        .expect("the model's Compute enum has values");
    let operating_system = OPERATING_SYSTEM_NAME_VALUES
        .first()
        .expect("the model's OperatingSystemName enum has values");
    let every_protocol: Vec<&str> = PROTOCOL_VALUES.to_vec();
    // `GlobalAccelerator` is a nested structure: its required `Mode` and its optional
    // `PreferredProtocol` both come from the model's own enums.
    let accelerator_mode = A_G_A_MODE_FOR_WORK_SPACE_ENUM_VALUES
        .first()
        .expect("the model's AGAModeForWorkSpaceEnum has values");
    let preferred_protocol = A_G_A_PREFERRED_PROTOCOL_FOR_WORK_SPACE_VALUES
        .last()
        .expect("the model's AGAPreferredProtocolForWorkSpace has values");

    let mut rows: Vec<(&'static str, Option<Value>, Value)> = vec![
        (
            // No `WorkspaceProperties` at all: the whole set is resolved for the caller.
            "absent",
            None,
            json!({ "RunningMode": DEFAULT_RUNNING_MODE }),
        ),
        (
            // Present but empty: the same resolution, from an object rather than nothing.
            "empty",
            Some(json!({})),
            json!({ "RunningMode": DEFAULT_RUNNING_MODE }),
        ),
        (
            // Properties without a running mode: what IS submitted is kept, and only the
            // running mode is resolved.
            "no running mode, volumes only",
            Some(json!({ "RootVolumeSizeGib": 175, "UserVolumeSizeGib": 100 })),
            json!({
                "RootVolumeSizeGib": 175,
                "UserVolumeSizeGib": 100,
                "RunningMode": DEFAULT_RUNNING_MODE,
            }),
        ),
        (
            // The same, over the members with the other modelled types: a boolean, a
            // list with every value its enum declares, a nested structure, two enums.
            "no running mode, every other member",
            Some(json!({
                "NestedVirtualizationEnabled": true,
                "Protocols": every_protocol,
                "GlobalAccelerator": {
                    "Mode": accelerator_mode,
                    "PreferredProtocol": preferred_protocol,
                },
                "ComputeTypeName": compute,
                "OperatingSystemName": operating_system,
            })),
            json!({
                "NestedVirtualizationEnabled": true,
                "Protocols": every_protocol,
                "GlobalAccelerator": {
                    "Mode": accelerator_mode,
                    "PreferredProtocol": preferred_protocol,
                },
                "ComputeTypeName": compute,
                "OperatingSystemName": operating_system,
                "RunningMode": DEFAULT_RUNNING_MODE,
            }),
        ),
    ];

    // One row per running mode the model declares — including the ones this issue's
    // recording never carried: a submitted running mode is reported as submitted, so
    // nothing is defaulted over it.
    for mode in running_modes {
        let case: &'static str = match *mode {
            "ALWAYS_ON" => "running mode ALWAYS_ON",
            "AUTO_STOP" => "running mode AUTO_STOP",
            "MANUAL" => "running mode MANUAL",
            other => {
                panic!("the model gained a RunningMode value this table does not cover: {other}")
            }
        };
        let mut submitted = json!({
            "RunningMode": mode,
            "ComputeTypeName": COMPUTE_VALUES[0],
        });
        if *mode == "AUTO_STOP" {
            // The member the model pairs with this mode: "The time after a user logs off
            // when WorkSpaces are automatically stopped. Configured in 60-minute
            // intervals."
            submitted["RunningModeAutoStopTimeoutInMinutes"] = json!(120);
        }
        rows.push((case, Some(submitted.clone()), submitted));
    }
    // The rows above write out the properties they expect member by member; the ones the
    // service resolves for a row that left them out are filled in here, in one place, by
    // the same resolution every suite asserts against.
    rows.into_iter()
        .map(|(case, submitted, effective)| (case, submitted, effective_properties(Some(&effective))))
        .collect()
}

/// Submit `element` as a one-element `CreateWorkspaces` batch and return
/// `(failed, pending)`.
async fn submit_one(router: &MultiServiceRouter, element: &Value) -> (Vec<Value>, Vec<Value>) {
    let (status, body) = call(
        router,
        "CreateWorkspaces",
        &json!({ "Workspaces": [element] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {element:?}: CreateWorkspaces must answer 200, body {body:?}"
    );
    let failed = body["FailedRequests"]
        .as_array()
        .unwrap_or_else(|| panic!("case {element:?}: FailedRequests must be a list, got {body:?}"))
        .clone();
    let pending = body["PendingRequests"]
        .as_array()
        .unwrap_or_else(|| panic!("case {element:?}: PendingRequests must be a list, got {body:?}"))
        .clone();
    (failed, pending)
}

/// One create element, with `properties` carried only when the row submits any.
fn element(
    directory_id: &str,
    user_name: &str,
    bundle_id: &str,
    properties: &Option<Value>,
) -> Value {
    let mut element = json!({
        "DirectoryId": directory_id,
        "UserName": user_name,
        "BundleId": bundle_id,
    });
    if let Some(properties) = properties {
        element["WorkspaceProperties"] = properties.clone();
    }
    element
}

/// The refused echo carries the effective property set, over every row of
/// [`property_table`].
#[tokio::test]
async fn a_refused_element_echoes_its_effective_workspace_properties() {
    let (router, _engine) = emulator_with_engine();
    let bundles = amazon_bundle_ids(&router).await;
    let bundle = bundles[0].clone();

    for (case, submitted, effective) in property_table() {
        // The directory is what makes this element unsatisfiable, so the bundle is a real
        // catalog bundle: the refusal is about the resource, the echo about the request.
        let submitted_element = element(UNREGISTERED_DIRECTORY, "probe", &bundle, &submitted);
        let (failed, pending) = submit_one(&router, &submitted_element).await;
        assert_eq!(
            failed.len(),
            1,
            "case {case:?}: the unsatisfiable element gets its own entry, got {failed:?}"
        );
        assert!(
            pending.is_empty(),
            "case {case:?}: nothing may be created for a refused element, got {pending:?}"
        );
        assert_eq!(
            failed[0]["ErrorCode"].as_str(),
            Some("ResourceNotFound.Directory"),
            "case {case:?}: expected ErrorCode ResourceNotFound.Directory, got {failed:?}"
        );
        assert_eq!(
            &failed[0]["WorkspaceRequest"]["WorkspaceProperties"],
            &effective,
            "case {case:?}: expected the effective properties {effective:?}, got {:?}",
            failed[0]["WorkspaceRequest"]
        );
        // And the rest of the echo is still the request that was sent.
        for member in ["DirectoryId", "UserName", "BundleId"] {
            assert_eq!(
                &failed[0]["WorkspaceRequest"][member], &submitted_element[member],
                "case {case:?}: {member} must be echoed as submitted, got {failed:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_created_workspace_reports_the_same_effective_properties_the_echo_carries() {
    let (router, engine) = emulator_with_engine();
    let bundles = amazon_bundle_ids(&router).await;
    let bundle = bundles[bundles.len() / 2].clone();
    register_directory(&router, REGISTERED_DIRECTORY).await;

    for (index, (case, submitted, effective)) in property_table().into_iter().enumerate() {
        // One user per row: a directory user holds at most one WorkSpace.
        let user_name = format!("user{index}");
        let submitted_element = element(REGISTERED_DIRECTORY, &user_name, &bundle, &submitted);
        let (failed, pending) = submit_one(&router, &submitted_element).await;
        assert!(
            failed.is_empty(),
            "case {case:?}: a satisfiable element must not fail, got {failed:?}"
        );
        assert_eq!(
            pending.len(),
            1,
            "case {case:?}: the element must be created, got {pending:?}"
        );
        assert_eq!(
            &pending[0]["WorkspaceProperties"], &effective,
            "case {case:?}: expected the effective properties {effective:?} in PendingRequests, \
             got {:?}",
            pending[0]
        );

        // Read back through the reader a client would use: the properties the create
        // resolved are the properties the store holds.
        let workspace_id = pending[0]["WorkspaceId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a minted WorkspaceId, got {pending:?}"))
            .to_string();
        settle(&engine);
        let described = describe_one(&router, &workspace_id).await;
        assert_eq!(
            &described["WorkspaceProperties"], &effective,
            "case {case:?}: expected DescribeWorkspaces to report {effective:?}, got {described:?}"
        );
    }
}

#[tokio::test]
async fn resubmitting_an_echoed_request_asks_for_the_same_workspace() {
    // The reason the echo must be complete: a client retries a partial batch by sending
    // the `WorkspaceRequest`s it got back. Whatever the row submitted, the retry must
    // produce a WorkSpace with the properties of the echo it came from.
    for (case, submitted, effective) in property_table() {
        let (router, engine) = emulator_with_engine();
        let bundles = amazon_bundle_ids(&router).await;
        let bundle = bundles[bundles.len() - 1].clone();

        let (failed, _) = submit_one(
            &router,
            &element(REGISTERED_DIRECTORY, "retried", &bundle, &submitted),
        )
        .await;
        assert_eq!(
            failed.len(),
            1,
            "case {case:?}: the element is unsatisfiable before the directory exists, got \
             {failed:?}"
        );
        let echoed = failed[0]["WorkspaceRequest"].clone();

        // The remediation the ErrorCode points at, then the retry: the echoed request,
        // resubmitted unchanged.
        register_directory(&router, REGISTERED_DIRECTORY).await;
        let (failed, pending) = submit_one(&router, &echoed).await;
        assert!(
            failed.is_empty(),
            "case {case:?}: the retry of {echoed:?} must be satisfiable, got {failed:?}"
        );
        assert_eq!(
            pending.len(),
            1,
            "case {case:?}: the retry creates the WorkSpace, got {pending:?}"
        );
        let workspace_id = pending[0]["WorkspaceId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a minted WorkspaceId, got {pending:?}"))
            .to_string();

        settle(&engine);
        let described = describe_one(&router, &workspace_id).await;
        assert_eq!(
            &described["WorkspaceProperties"], &effective,
            "case {case:?}: the retried WorkSpace must carry the echoed properties \
             {effective:?}, got {described:?}"
        );
    }
}

/// The `ErrorMessage` AWS answers beside each per-resource code: fixed text for the error
/// CLASS, interpolating nothing from the request, as a recorded response carries it beside
/// an id that appears nowhere in it.
fn expected_message(code: &str) -> &'static str {
    match code {
        "ResourceNotFound.Bundle" => "The specified bundle could not be found in the specified region.",
        "ResourceNotFound.Directory" => {
            "The specified directory could not be found in the specified region."
        }
        other => panic!("no recorded message for the per-item code {other}"),
    }
}

#[tokio::test]
async fn a_refusal_names_the_resource_that_failed_to_resolve() {
    let (router, _engine) = emulator_with_engine();
    let bundles = amazon_bundle_ids(&router).await;
    register_directory(&router, REGISTERED_DIRECTORY).await;
    let held_bundle = bundles[0].clone();
    let other_held_bundle = bundles[bundles.len() - 1].clone();

    // `BundleId` ids the store does not hold, at both ends of the model's pattern
    // `^wsb-[0-9a-z]{8,63}$`: the shortest suffix it allows (8) and a long one (63).
    let shortest_unheld = "wsb-00000000".to_string();
    let longest_unheld = format!("wsb-{}", "z".repeat(63));

    // (case, DirectoryId, BundleId, expected ErrorCode or None when satisfiable)
    let cases: Vec<(&str, &str, &str, Option<&str>)> = vec![
        (
            "both resolve",
            REGISTERED_DIRECTORY,
            &held_bundle,
            None,
        ),
        (
            "both resolve, another catalog bundle",
            REGISTERED_DIRECTORY,
            &other_held_bundle,
            None,
        ),
        (
            "bundle unheld, shortest id",
            REGISTERED_DIRECTORY,
            &shortest_unheld,
            Some("ResourceNotFound.Bundle"),
        ),
        (
            "bundle unheld, longest id",
            REGISTERED_DIRECTORY,
            &longest_unheld,
            Some("ResourceNotFound.Bundle"),
        ),
        (
            "directory unregistered, bundle held",
            UNREGISTERED_DIRECTORY,
            &held_bundle,
            Some("ResourceNotFound.Directory"),
        ),
        (
            // Two failures, one `ErrorCode`: the first in the model's member order.
            "neither resolves",
            UNREGISTERED_DIRECTORY,
            &shortest_unheld,
            Some("ResourceNotFound.Directory"),
        ),
    ];

    for (index, (case, directory_id, bundle_id, expected)) in cases.iter().enumerate() {
        let user_name = format!("attributed{index}");
        let submitted = element(directory_id, &user_name, bundle_id, &None);
        let (failed, pending) = submit_one(&router, &submitted).await;
        match expected {
            None => {
                assert!(
                    failed.is_empty(),
                    "case {case:?}: both references resolve, so nothing may fail, got {failed:?}"
                );
                assert_eq!(
                    pending.len(),
                    1,
                    "case {case:?}: the element must be created, got {pending:?}"
                );
            }
            Some(code) => {
                assert!(
                    pending.is_empty(),
                    "case {case:?}: nothing may be created, got {pending:?}"
                );
                assert_eq!(
                    failed.len(),
                    1,
                    "case {case:?}: exactly one entry reports the element, got {failed:?}"
                );
                assert_eq!(
                    failed[0]["ErrorCode"].as_str(),
                    Some(*code),
                    "case {case:?}: expected ErrorCode {code}, got {failed:?}"
                );
                assert_eq!(
                    failed[0]["ErrorMessage"].as_str(),
                    Some(expected_message(code)),
                    "case {case:?}: expected the fixed message for {code}, got {failed:?}"
                );
            }
        }
    }
}
