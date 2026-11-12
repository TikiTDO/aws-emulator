// SPDX-License-Identifier: CC0-1.0

//! Per-element validation of `CreateWorkspaces`, end to end over the awsJson1_1 wire.
//!
//! Every case drives a real `MultiServiceRouter` with this service mounted on one shared
//! engine, so each assertion is about what a client would see.
//!
//! The rule under test, stated over the SET of batches a client can submit rather than
//! over any one recorded request: `CreateWorkspaces` judges every element of its batch on
//! its own against this service's persisted state, creates nothing for an element it
//! cannot satisfy, and reports that element as its own `FailedRequests` entry under a 200
//! — the submitted request echoed verbatim beside the per-item `ErrorType` code naming
//! what could not be resolved (`ResourceNotFound.Directory` /
//! `ResourceNotFound.Bundle`), never a top-level error and never a minted WorkSpace.
//!
//! The directory half of that state is written by `RegisterWorkspaceDirectory`, which is
//! implemented in the same change and asserted here too: registering is what makes a
//! `DirectoryId` resolvable, and the registration is read back through the create it
//! enables. (`DescribeWorkspaceDirectories`, the operation that would report the
//! directory itself, is still a scaffold stub — the `WorkspaceDirectory` shape it answers
//! with is large enough to be its own issue — so the registration's own state is read
//! through this service's store, the way the lifecycle suite reads the clock.)
//!
//! "Verbatim" is over the members this service DECLARES: a request member it declares
//! nowhere is dropped at the boundary, before any handler sees it, so no echo can carry
//! it. `WorkspaceProperties` carries `RunningMode`,
//! `RunningModeAutoStopTimeoutInMinutes`, `RootVolumeSizeGib`, `UserVolumeSizeGib`,
//! `ComputeTypeName`, `Protocols`, `OperatingSystemName`, `GlobalAccelerator`,
//! `NestedVirtualizationEnabled` and `RecycleMode` (the member AWS answers with that the
//! published snapshot does not declare — see `smithy-model/LOCAL-ADDITIONS.md`), and the
//! cases below stay inside that set. Two members are not echoed exactly as submitted:
//!
//! * `Tags`: a recorded AWS echo carries the list even for a request that sent none, so
//!   an absent `Tags` comes back as `[]`;
//! * `WorkspaceProperties`: the echo carries the EFFECTIVE property set, so a property
//!   the service resolved for the caller (an omitted `RunningMode` or `RecycleMode`) is
//!   echoed with the value the WorkSpace would have been created with. That rule is
//!   asserted over its own table in `create_workspaces_effective_properties`; here it
//!   only shapes what the echo of each case must be.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_workspaces_workspacesservice::state::{
    service_state, DirectoryRegistrationState,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::workspace_lifecycle::{
    call, effective_properties, emulator_with_engine, register_directory, settle,
};

/// What one element of a submitted batch must produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The element is satisfiable: a WorkSpace is created and reported in
    /// `PendingRequests`.
    Created,
    /// The element is unsatisfiable: it is reported in `FailedRequests` under this
    /// per-item `ErrorType` code, and nothing is persisted for it.
    Refused(&'static str),
}

/// The `BundleId`s `DescribeWorkspaceBundles` reports for the Amazon-provided catalog.
///
/// The bundles a create may name are the ones the STORE holds, so the cases take their
/// bundle ids from the operation a client would list them with rather than from a
/// hand-written list.
/// `pub(crate)` because `tests/all.rs` compiles every suite into ONE test crate, so the
/// suites beside this one list the catalog through it rather than copying the call.
pub(crate) async fn amazon_bundle_ids(router: &MultiServiceRouter) -> Vec<String> {
    let filters = json!({ "Owner": "AMAZON" });
    let (status, body) = call(router, "DescribeWorkspaceBundles", &filters).await;
    assert_eq!(
        status, 200,
        "DescribeWorkspaceBundles must answer 200, body {body:?}"
    );
    body["Bundles"]
        .as_array()
        .unwrap_or_else(|| panic!("Bundles must be a list, got {body:?}"))
        .iter()
        .map(|bundle| {
            bundle["BundleId"]
                .as_str()
                .unwrap_or_else(|| panic!("every bundle carries a BundleId, got {bundle:?}"))
                .to_string()
        })
        .collect()
}

/// What the `FailedRequests` entry for `submitted` must echo.
///
/// The submitted request, member for member — with one normalisation: `Tags` comes back
/// as a list even for a request that sent none, which is what a recorded AWS echo
/// carries, so an element without tags is echoed with the empty list.
fn expected_echo(submitted: &Value) -> Value {
    let mut echo = submitted.clone();
    if echo.get("Tags").is_none() {
        echo["Tags"] = json!([]);
    }
    // The effective property set: the submitted properties with the ones the service
    // resolves for the caller filled in. See `create_workspaces_effective_properties`.
    echo["WorkspaceProperties"] = effective_properties(echo.get("WorkspaceProperties"));
    echo
}

/// Submit `elements` as one `CreateWorkspaces` batch and return `(failed, pending)`.
async fn create_batch(router: &MultiServiceRouter, elements: &[Value]) -> (Vec<Value>, Vec<Value>) {
    let request = json!({ "Workspaces": elements });
    let (status, body) = call(router, "CreateWorkspaces", &request).await;
    assert_eq!(
        status, 200,
        "case {elements:?}: a per-element refusal is still HTTP 200, body {body:?}"
    );
    let failed = body["FailedRequests"]
        .as_array()
        .unwrap_or_else(|| panic!("case {elements:?}: FailedRequests must be a list, got {body:?}"))
        .clone();
    let pending = body["PendingRequests"]
        .as_array()
        .unwrap_or_else(|| panic!("case {elements:?}: PendingRequests must be a list, got {body:?}"))
        .clone();
    (failed, pending)
}

/// The `UserName`s `DescribeWorkspaces` reports, in creation order.
async fn described_user_names(router: &MultiServiceRouter) -> Vec<String> {
    let (status, body) = call(router, "DescribeWorkspaces", &json!({})).await;
    assert_eq!(
        status, 200,
        "DescribeWorkspaces must answer 200, body {body:?}"
    );
    body["Workspaces"]
        .as_array()
        .unwrap_or_else(|| panic!("Workspaces must be a list, got {body:?}"))
        .iter()
        .map(|workspace| {
            workspace["UserName"]
                .as_str()
                .unwrap_or_else(|| panic!("every WorkSpace carries a UserName, got {workspace:?}"))
                .to_string()
        })
        .collect()
}

/// `DirectoryId` pattern from the model: `^(d-[0-9a-f]{8,63}$)|(wsd-[0-9a-z]{8,63}$)$`.
/// Checked by hand so the suite pulls in no regex engine.
fn is_modelled_directory_id(candidate: &str) -> bool {
    if let Some(suffix) = candidate.strip_prefix("wsd-") {
        return (8..=63).contains(&suffix.len())
            && suffix
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase());
    }
    let Some(suffix) = candidate.strip_prefix("d-") else {
        return false;
    };
    (8..=63).contains(&suffix.len()) && suffix.chars().all(|c| c.is_ascii_hexdigit())
}

/// The registration state this service's store holds for `directory_id`.
fn stored_directory_state(
    engine: &Arc<engine::InMemoryEngine>,
    directory_id: &str,
) -> Option<DirectoryRegistrationState> {
    let state = service_state(&**engine).expect("the WorkSpaces state store");
    let reported = state
        .lock()
        .directory(directory_id)
        .map(|record| record.state);
    reported
}

#[tokio::test]
async fn every_batch_element_is_judged_on_its_own_against_persisted_state() {
    let (router, engine) = emulator_with_engine();
    let bundles = amazon_bundle_ids(&router).await;
    assert!(
        bundles.len() >= 3,
        "the Amazon-provided catalog must offer at least three bundles, got {bundles:?}"
    );
    // Three catalog bundles, taken from different places in the reported catalog rather
    // than only its first entry.
    let (first_bundle, middle_bundle, last_bundle) = (
        bundles[0].clone(),
        bundles[bundles.len() / 2].clone(),
        bundles[bundles.len() - 1].clone(),
    );
    // Both id forms the model's `DirectoryId` allows, registered through the wire.
    for directory_id in ["d-aaaaaaaa11", "wsd-abcdef0123"] {
        register_directory(&router, directory_id).await;
    }

    // Each row is one element of a SINGLE submitted batch, and what it must produce. The
    // rows differ along what the rule generalises over: which resource is unresolvable
    // (directory, bundle, both), the id form, the optional members carried (tags,
    // `WorkspaceProperties`, `WorkspaceName`), and the per-element refusals that need no
    // resource lookup at all. (The effective-property rule the echoed
    // `WorkspaceProperties` follows has its own suite:
    // `create_workspaces_effective_properties`.)
    let cases: Vec<(Value, Expected)> = vec![
        (
            json!({
                "DirectoryId": "d-aaaaaaaa11",
                "UserName": "alice",
                "BundleId": first_bundle,
                "Tags": [{ "Key": "probe", "Value": "yes" }],
                "WorkspaceProperties": {
                    "RunningMode": "ALWAYS_ON",
                    "RootVolumeSizeGib": 80,
                    "UserVolumeSizeGib": 100,
                    "ComputeTypeName": "STANDARD",
                    "Protocols": ["PCOIP"],
                },
            }),
            Expected::Created,
        ),
        (
            // A directory nobody registered: unsatisfiable, however good the bundle is.
            json!({
                "DirectoryId": "d-ffffffff99",
                "UserName": "bob",
                "BundleId": first_bundle,
                "Tags": [{ "Key": "probe", "Value": "yes" }],
                "WorkspaceProperties": {
                    "RunningMode": "AUTO_STOP",
                    "RunningModeAutoStopTimeoutInMinutes": 60,
                    "ComputeTypeName": "PERFORMANCE",
                },
            }),
            Expected::Refused("ResourceNotFound.Directory"),
        ),
        (
            // A bundle the store does not hold, in a directory that is registered.
            json!({
                "DirectoryId": "d-aaaaaaaa11",
                "UserName": "carol",
                "BundleId": "wsb-00000000",
            }),
            Expected::Refused("ResourceNotFound.Bundle"),
        ),
        (
            // Neither resolves: the entry has two failures to report and one
            // `ErrorCode`, so it reports the first of them in the model's own member
            // order (`WorkspaceRequest` declares `DirectoryId` before `BundleId`).
            json!({
                "DirectoryId": "wsd-unregistered1",
                "UserName": "dan",
                "BundleId": "wsb-00000000",
            }),
            Expected::Refused("ResourceNotFound.Directory"),
        ),
        (
            // The same directory user as the first row (the model's `UserName` doc says
            // the name is not case-sensitive), judged against the WorkSpace this very
            // batch created for it.
            json!({
                "DirectoryId": "d-aaaaaaaa11",
                "UserName": "ALICE",
                "BundleId": middle_bundle,
            }),
            Expected::Refused("ResourceExists.WorkspaceUser"),
        ),
        (
            // A member combination the model forbids: `WorkspaceName` is required when
            // `UserName` is the reserved `[UNDEFINED]`.
            json!({
                "DirectoryId": "d-aaaaaaaa11",
                "UserName": "[UNDEFINED]",
                "BundleId": middle_bundle,
            }),
            Expected::Refused("InvalidParameterValues"),
        ),
        (
            // The same user-decoupled form, satisfied.
            json!({
                "DirectoryId": "d-aaaaaaaa11",
                "UserName": "[UNDEFINED]",
                "BundleId": middle_bundle,
                "WorkspaceName": "pool_station_1",
            }),
            Expected::Created,
        ),
        (
            // The other registered directory, and the last bundle of the catalog.
            json!({
                "DirectoryId": "wsd-abcdef0123",
                "UserName": "erin",
                "BundleId": last_bundle,
            }),
            Expected::Created,
        ),
    ];

    let elements: Vec<Value> = cases.iter().map(|(element, _)| element.clone()).collect();
    let (failed, pending) = create_batch(&router, &elements).await;

    let expected_failures: Vec<(&Value, &str)> = cases
        .iter()
        .filter_map(|(element, expected)| match expected {
            Expected::Refused(code) => Some((element, *code)),
            Expected::Created => None,
        })
        .collect();
    let expected_created: Vec<&Value> = cases
        .iter()
        .filter(|(_, expected)| *expected == Expected::Created)
        .map(|(element, _)| element)
        .collect();

    assert_eq!(
        failed.len(),
        expected_failures.len(),
        "every unsatisfiable element gets its own FailedRequests entry, got {failed:?}"
    );
    assert_eq!(
        pending.len(),
        expected_created.len(),
        "only satisfiable elements may be created, got {pending:?}"
    );

    // The failures come back in the order the elements were submitted, each echoing its
    // own request verbatim — tags and `WorkspaceProperties` included.
    for (reported, (submitted, code)) in failed.iter().zip(expected_failures.iter()) {
        assert_eq!(
            reported["ErrorCode"].as_str(),
            Some(*code),
            "case {submitted:?}: expected ErrorCode {code}, got {reported:?}"
        );
        assert!(
            reported["ErrorMessage"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {submitted:?}: a failure carries an ErrorMessage, got {reported:?}"
        );
        assert_eq!(
            &reported["WorkspaceRequest"],
            &expected_echo(submitted),
            "case {submitted:?}: the failure echoes the submitted request, got {reported:?}"
        );
    }

    for (reported, submitted) in pending.iter().zip(expected_created.iter()) {
        for member in ["DirectoryId", "UserName", "BundleId"] {
            assert_eq!(
                &reported[member], &submitted[member],
                "case {submitted:?}: {member} must be persisted as supplied, got {reported:?}"
            );
        }
        assert_eq!(
            reported["State"].as_str(),
            Some("PENDING"),
            "case {submitted:?}: a WorkSpace being created is PENDING, got {reported:?}"
        );
    }

    // And the store holds exactly the satisfiable elements: a refused element persisted
    // nothing a later describe can see.
    settle(&engine);
    let described = described_user_names(&router).await;
    let expected_users: Vec<String> = expected_created
        .iter()
        .map(|element| element["UserName"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        described, expected_users,
        "DescribeWorkspaces must report exactly the created WorkSpaces, got {described:?}"
    );
}

#[tokio::test]
async fn a_refused_element_is_reported_whatever_its_place_in_the_batch() {
    // The same unsatisfiable element is submitted alone, first, in the middle and last of
    // a batch of satisfiable ones: the judgment is the element's own, so neither the
    // batch's size nor the element's position changes it, and the satisfiable elements
    // beside it are still created.
    let refusals: [(Value, &str); 2] = [
        (
            json!({
                "DirectoryId": "d-cccccccc33",
                "UserName": "nobody",
                "BundleId": "BUNDLE",
            }),
            "ResourceNotFound.Directory",
        ),
        (
            json!({
                "DirectoryId": "d-bbbbbbbb22",
                "UserName": "nobody",
                "BundleId": "wsb-99999999",
            }),
            "ResourceNotFound.Bundle",
        ),
    ];

    for (template, code) in refusals {
        for (companions, position) in [(0usize, 0usize), (2, 0), (2, 1), (2, 2)] {
            let (router, engine) = emulator_with_engine();
            let bundles = amazon_bundle_ids(&router).await;
            let bundle = bundles[0].clone();
            register_directory(&router, "d-bbbbbbbb22").await;

            // The template's bundle placeholder stands for "a bundle the store holds".
            let mut refusing = template.clone();
            if refusing["BundleId"] == json!("BUNDLE") {
                refusing["BundleId"] = json!(bundle);
            }

            let mut elements: Vec<Value> = (0..companions)
                .map(|index| {
                    json!({
                        "DirectoryId": "d-bbbbbbbb22",
                        "UserName": format!("user{index}"),
                        "BundleId": bundle,
                    })
                })
                .collect();
            elements.insert(position, refusing.clone());
            let case = format!("{code} at {position} of {}", elements.len());

            let (failed, pending) = create_batch(&router, &elements).await;
            assert_eq!(
                failed.len(),
                1,
                "case {case}: exactly the unsatisfiable element fails, got {failed:?}"
            );
            assert_eq!(
                failed[0]["ErrorCode"].as_str(),
                Some(code),
                "case {case}: expected ErrorCode {code}, got {failed:?}"
            );
            assert_eq!(
                &failed[0]["WorkspaceRequest"],
                &expected_echo(&refusing),
                "case {case}: the failure echoes the submitted request, got {failed:?}"
            );
            assert_eq!(
                pending.len(),
                companions,
                "case {case}: every satisfiable element beside it is still created, got {pending:?}"
            );

            settle(&engine);
            let described = described_user_names(&router).await;
            assert_eq!(
                described.len(),
                companions,
                "case {case}: a refused element persists nothing, got {described:?}"
            );
            assert!(
                !described.iter().any(|user| user == "nobody"),
                "case {case}: the refused element's user must hold no WorkSpace, got {described:?}"
            );
        }
    }
}

#[tokio::test]
async fn registering_a_directory_is_what_makes_it_resolvable() {
    let (router, engine) = emulator_with_engine();
    let bundles = amazon_bundle_ids(&router).await;
    let bundle = bundles[0].clone();
    let directory_id = "d-1234abcd56";
    let request = json!({
        "DirectoryId": directory_id,
        "UserName": "alice",
        "BundleId": bundle,
    });

    // Before the registration the create cannot be satisfied.
    let (failed, pending) = create_batch(&router, std::slice::from_ref(&request)).await;
    assert_eq!(
        failed[0]["ErrorCode"].as_str(),
        Some("ResourceNotFound.Directory"),
        "an unregistered directory is reported as ResourceNotFound.Directory, got {failed:?}"
    );
    assert!(
        pending.is_empty(),
        "nothing may be created in an unregistered directory, got {pending:?}"
    );

    // Registering it answers the transient state the clock owes the settled one from.
    let (status, body) = call(
        &router,
        "RegisterWorkspaceDirectory",
        &json!({ "DirectoryId": directory_id, "SubnetIds": ["subnet-11111111"], "Tenancy": "SHARED" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "RegisterWorkspaceDirectory must answer 200, body {body:?}"
    );
    assert_eq!(
        body["DirectoryId"].as_str(),
        Some(directory_id),
        "the registration reports the directory it registered, body {body:?}"
    );
    assert_eq!(
        body["State"].as_str(),
        Some("REGISTERING"),
        "registration is asynchronous, so it answers REGISTERING, body {body:?}"
    );
    assert_eq!(
        stored_directory_state(&engine, directory_id),
        Some(DirectoryRegistrationState::Registering),
        "the stored registration is REGISTERING while the clock owes it"
    );

    // The same request is satisfiable now, and the created WorkSpace reads back in the
    // directory that was registered.
    let (failed, pending) = create_batch(&router, std::slice::from_ref(&request)).await;
    assert!(
        failed.is_empty(),
        "a registered directory must not fail a create, got {failed:?}"
    );
    assert_eq!(
        pending[0]["DirectoryId"].as_str(),
        Some(directory_id),
        "the created WorkSpace names the registered directory, got {pending:?}"
    );

    settle(&engine);
    assert_eq!(
        stored_directory_state(&engine, directory_id),
        Some(DirectoryRegistrationState::Registered),
        "the registration settles to REGISTERED"
    );
    let (status, described) = call(
        &router,
        "DescribeWorkspaces",
        &json!({ "DirectoryId": directory_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeWorkspaces must answer 200, body {described:?}"
    );
    assert_eq!(
        described["Workspaces"].as_array().map(Vec::len),
        Some(1),
        "the WorkSpace is reported in the directory it was created in, got {described:?}"
    );

    // Registering the same directory twice is refused.
    let (status, body) = call(
        &router,
        "RegisterWorkspaceDirectory",
        &json!({ "DirectoryId": directory_id }),
    )
    .await;
    assert_eq!(
        status, 400,
        "registering an already registered directory is refused, body {body:?}"
    );
    assert!(
        body["__type"]
            .as_str()
            .unwrap_or_default()
            .ends_with("ResourceAlreadyExistsException"),
        "expected ResourceAlreadyExistsException, got {body:?}"
    );

    // A registration that names no DirectoryId gets a minted `wsd-` id, which is itself
    // resolvable by a create.
    let (status, body) = call(
        &router,
        "RegisterWorkspaceDirectory",
        &json!({ "WorkspaceDirectoryName": "pool-directory", "WorkspaceType": "POOLS" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a registration without a DirectoryId must answer 200, body {body:?}"
    );
    let minted = body["DirectoryId"]
        .as_str()
        .unwrap_or_else(|| panic!("a minted DirectoryId, got {body:?}"))
        .to_string();
    assert!(
        is_modelled_directory_id(&minted),
        "the minted id must match the model's DirectoryId pattern, got {minted:?}"
    );
    assert_ne!(
        minted, directory_id,
        "the minted id must name a directory of its own, got {minted:?}"
    );
    let (failed, pending) = create_batch(
        &router,
        &[json!({
            "DirectoryId": minted,
            "UserName": "frank",
            "BundleId": bundle,
        })],
    )
    .await;
    assert!(
        failed.is_empty(),
        "case {minted}: the minted directory must be resolvable, got {failed:?}"
    );
    assert_eq!(
        pending.len(),
        1,
        "case {minted}: the create is satisfiable, got {pending:?}"
    );

    // And a registration with neither a DirectoryId nor a name identifies nothing.
    let (status, body) = call(&router, "RegisterWorkspaceDirectory", &json!({})).await;
    assert_eq!(
        status, 400,
        "a registration naming neither a directory nor a name is refused, body {body:?}"
    );
    assert!(
        body["__type"]
            .as_str()
            .unwrap_or_default()
            .ends_with("InvalidParameterValuesException"),
        "expected InvalidParameterValuesException, got {body:?}"
    );
}
