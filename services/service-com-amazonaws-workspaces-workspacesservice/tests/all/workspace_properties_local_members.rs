// SPDX-License-Identifier: CC0-1.0

//! The `WorkspaceProperties` member set this service parses, persists and reports, end to
//! end over the awsJson1_1 wire.
//!
//! Every case drives a real `MultiServiceRouter` with this service mounted on one shared
//! engine, so each assertion is about what a client would see.
//!
//! The rule under test, stated over the SET of property objects a client can submit rather
//! than over any one recorded request: wherever this service reports the properties a
//! request asked for — the `WorkspaceProperties` echoed in a `CreateWorkspaces`
//! `FailedRequests` entry, the one reported for a created WorkSpace in `PendingRequests`,
//! and the one `DescribeWorkspaces` reports for it later — EVERY member the request
//! submitted comes back unchanged. The member set is the whole declared one, which
//! includes `RecycleMode`: AWS carries that member while the published model snapshot does
//! not declare it, so this crate declares it itself (see
//! `smithy-model/LOCAL-ADDITIONS.md` and `src/ops/types/workspace_properties_types.rs`).
//!
//! The members the service fills in for the caller are the two switch-like properties a
//! WorkSpace always has one value of, an omitted `RunningMode` and an omitted
//! `RecycleMode`; that resolution has its own suite
//! (`create_workspaces_effective_properties`) and here only shapes what each case must
//! come back as.
//!
//! The last two tests pin the local declaration against the model itself: the snapshot's
//! own member list, which the local declaration must cover in full and which must still not
//! declare `RecycleMode`, and every shape the overlay restates, compared declaration for
//! declaration with the snapshot's. A refreshed snapshot that declares the member, adds one
//! the overlay does not restate, or changes a target, value set or constraint fails there
//! rather than silently narrowing what the service accepts or reports.

use serde_json::{json, Map, Value};
use service_com_amazonaws_workspaces_workspacesservice::ops::types::{
    A_G_A_MODE_FOR_WORK_SPACE_ENUM_VALUES, A_G_A_PREFERRED_PROTOCOL_FOR_WORK_SPACE_VALUES,
    COMPUTE_VALUES, OPERATING_SYSTEM_NAME_VALUES, PROTOCOL_VALUES, RUNNING_MODE_VALUES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::create_workspaces_validation::amazon_bundle_ids;
use crate::workspace_lifecycle::{
    call, describe_one, effective_properties, emulator_with_engine, register_directory, settle,
};

/// The published model snapshot this crate is generated from, embedded at compile time so
/// the local declaration is checked against the model rather than against a copy of it.
const SNAPSHOT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-workspaces-workspacesservice.json"
));

/// The overlay model the local `WorkspaceProperties` declaration is generated from,
/// embedded for the same reason: what it restates of the snapshot must still match it.
const OVERLAY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/local-additions.json"
));

/// The shape the local declaration is of.
const PROPERTIES_SHAPE: &str = "com.amazonaws.workspaces#WorkspaceProperties";
/// The shape the overlay adds, which the snapshot declares nowhere.
const RECYCLE_MODE_SHAPE: &str = "com.amazonaws.workspaces#RecycleMode";
/// The one restated shape the overlay deliberately declares without its members: codegen
/// resolves a STRUCTURE member through the Rust field's type (the modelled type in
/// `shared_types`), reading only the shape's type from the model.
const ACCELERATOR_SHAPE: &str = "com.amazonaws.workspaces#GlobalAcceleratorForWorkSpace";

/// The member this crate declares locally, which the snapshot does not.
const LOCAL_MEMBER: &str = "RecycleMode";

/// A directory this suite registers, so an element naming it is satisfiable. Matches the
/// model's `DirectoryId` pattern `^(d-[0-9a-f]{8,63}$)|(wsd-[0-9a-z]{8,63}$)$`.
const REGISTERED_DIRECTORY: &str = "d-0b0bca5caa";
/// A directory nobody registers, so an element naming it cannot be satisfied — which is
/// what puts its echo in `FailedRequests`.
const UNREGISTERED_DIRECTORY: &str = "wsd-0nowhere00";

/// One value for every member the properties shape declares.
///
/// The values come from the model: the enum members from the `*_VALUES` catalogues this
/// crate re-exports (`RunningMode`, `Compute`, `Protocol`, `OperatingSystemName` and the
/// two Global Accelerator enums), the auto-stop timeout in the 60-minute intervals
/// `WorkspaceProperties$RunningModeAutoStopTimeoutInMinutes` documents. `RecycleMode` is an
/// open string, so its value is simply one a request may carry.
fn every_member() -> Value {
    let auto_stop = RUNNING_MODE_VALUES
        .iter()
        .copied()
        .find(|mode| *mode == "AUTO_STOP")
        .expect("the model's RunningMode enum declares AUTO_STOP");
    json!({
        "RunningMode": auto_stop,
        "RunningModeAutoStopTimeoutInMinutes": 60,
        "RootVolumeSizeGib": 80,
        "UserVolumeSizeGib": 100,
        "ComputeTypeName": COMPUTE_VALUES[0],
        "Protocols": PROTOCOL_VALUES.to_vec(),
        "OperatingSystemName": OPERATING_SYSTEM_NAME_VALUES[0],
        "GlobalAccelerator": {
            "Mode": A_G_A_MODE_FOR_WORK_SPACE_ENUM_VALUES[0],
            "PreferredProtocol": A_G_A_PREFERRED_PROTOCOL_FOR_WORK_SPACE_VALUES[0],
        },
        "NestedVirtualizationEnabled": true,
        LOCAL_MEMBER: "DISABLED",
    })
}

/// The rows the rule is asserted over. They differ along what it generalises over: whether
/// the locally declared member is submitted at all, which value it carries, and how many of
/// the other members ride along.
fn property_rows() -> Vec<(&'static str, Value)> {
    let last_mode = RUNNING_MODE_VALUES
        .last()
        .expect("the model's RunningMode enum has values");
    vec![
        (
            "the local member alone",
            json!({ LOCAL_MEMBER: "DISABLED" }),
        ),
        (
            // Another value of the same member: nothing about it is fixed to one string.
            "the local member, another value",
            json!({ LOCAL_MEMBER: "ENABLED" }),
        ),
        (
            // It is an open string in the absence of a published value set, so a value
            // nothing publishes travels as sent rather than being normalised or refused.
            "the local member, an unpublished value",
            json!({ LOCAL_MEMBER: "disabled-for-now" }),
        ),
        (
            "the local member beside a submitted running mode",
            json!({ "RunningMode": last_mode, LOCAL_MEMBER: "ENABLED" }),
        ),
        ("every declared member", every_member()),
        (
            // No local member submitted: the service resolves it, like the running mode,
            // so what comes back carries its resolved value rather than nothing.
            "the modelled members only",
            json!({ "RunningMode": last_mode, "RootVolumeSizeGib": 80 }),
        ),
    ]
}

/// What a submitted property object must be reported as: itself, plus the members the
/// service resolves for a request that left them out.
fn expected(submitted: &Value) -> Value {
    effective_properties(Some(submitted))
}

/// One create element carrying `properties`.
fn element(directory_id: &str, user_name: &str, bundle_id: &str, properties: &Value) -> Value {
    json!({
        "DirectoryId": directory_id,
        "UserName": user_name,
        "BundleId": bundle_id,
        "WorkspaceProperties": properties,
    })
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

/// The `members` object of the snapshot's `WorkspaceProperties` shape.
fn snapshot_property_members() -> Map<String, Value> {
    model_shapes(SNAPSHOT)[PROPERTIES_SHAPE]["members"]
        .as_object()
        .unwrap_or_else(|| panic!("the snapshot must declare WorkspaceProperties with members"))
        .clone()
}

/// The `shapes` object of one model document.
fn model_shapes(model: &str) -> Value {
    let parsed: Value = serde_json::from_str(model).expect("a model document must be JSON");
    let shapes = parsed["shapes"]
        .as_object()
        .unwrap_or_else(|| panic!("a model document must carry a shapes object"))
        .clone();
    Value::Object(shapes)
}

/// `declaration` with every documentation trait dropped, recursively — and with a traits
/// object left empty by that dropped too, so a member the snapshot documents and nothing
/// else compares equal to the same member restated with no traits at all.
///
/// Documentation is prose: the overlay restates what codegen READS — targets, types, enum
/// values and constraint traits — so comparing declarations has to ignore the one trait
/// that carries no behaviour.
fn without_docs(declaration: &Value) -> Value {
    match declaration {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(key, _)| key.as_str() != "smithy.api#documentation")
                .map(|(key, value)| (key.clone(), without_docs(value)))
                .filter(|(key, value)| {
                    key.as_str() != "traits" || !value.as_object().is_some_and(Map::is_empty)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_docs).collect()),
        other => other.clone(),
    }
}

#[tokio::test]
async fn a_refused_element_echoes_every_property_member_it_submitted() {
    let (router, _engine) = emulator_with_engine();
    let bundles = amazon_bundle_ids(&router).await;
    let bundle = bundles[0].clone();

    for (case, submitted) in property_rows() {
        // The directory is what makes the element unsatisfiable, so the bundle is a real
        // catalog bundle: the refusal is about the resource, the echo about the request.
        let sent = element(UNREGISTERED_DIRECTORY, "probe", &bundle, &submitted);
        let (failed, pending) = submit_one(&router, &sent).await;
        assert!(
            pending.is_empty(),
            "case {case:?}: nothing may be created for a refused element, got {pending:?}"
        );
        assert_eq!(
            failed.len(),
            1,
            "case {case:?}: the unsatisfiable element gets its own entry, got {failed:?}"
        );
        let echoed = &failed[0]["WorkspaceRequest"]["WorkspaceProperties"];
        assert_eq!(
            echoed,
            &expected(&submitted),
            "case {case:?}: expected the echo to carry {:?}, got {echoed:?}",
            expected(&submitted)
        );
    }
}

#[tokio::test]
async fn a_created_workspace_keeps_every_property_member_it_was_created_with() {
    let (router, engine) = emulator_with_engine();
    let bundles = amazon_bundle_ids(&router).await;
    let bundle = bundles[bundles.len() / 2].clone();
    register_directory(&router, REGISTERED_DIRECTORY).await;

    for (index, (case, submitted)) in property_rows().into_iter().enumerate() {
        // One user per row: a directory user holds at most one WorkSpace.
        let user_name = format!("user{index}");
        let sent = element(REGISTERED_DIRECTORY, &user_name, &bundle, &submitted);
        let (failed, pending) = submit_one(&router, &sent).await;
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
            &pending[0]["WorkspaceProperties"],
            &expected(&submitted),
            "case {case:?}: expected PendingRequests to report {:?}, got {:?}",
            expected(&submitted),
            pending[0]
        );

        // Read back through the reader a client would use: the properties the create
        // stored are the properties a later describe reports.
        let workspace_id = pending[0]["WorkspaceId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a minted WorkspaceId, got {pending:?}"))
            .to_string();
        settle(&engine);
        let described = describe_one(&router, &workspace_id).await;
        assert_eq!(
            &described["WorkspaceProperties"],
            &expected(&submitted),
            "case {case:?}: expected DescribeWorkspaces to report {:?}, got {described:?}",
            expected(&submitted)
        );
    }
}

#[test]
fn the_overlay_restates_the_snapshot_exactly_but_for_the_member_it_adds() {
    // The overlay is what the local declaration is generated from, so anything it restates
    // of the snapshot — a member's target, a shape's type, an enum's values, a constraint
    // trait — must still BE what the snapshot says, or the type on the wire would be
    // narrower or wider than the model without any other test noticing.
    let snapshot = model_shapes(SNAPSHOT);
    let overlay = model_shapes(OVERLAY);
    let restated = overlay
        .as_object()
        .expect("the overlay carries a shapes object");

    for (shape_id, declared) in restated {
        if shape_id == RECYCLE_MODE_SHAPE {
            assert!(
                snapshot[shape_id].is_null(),
                "the snapshot now declares {shape_id}, so the local declaration should give \
                 way to the modelled shape"
            );
            continue;
        }
        let modelled = &snapshot[shape_id];
        assert!(
            !modelled.is_null(),
            "the overlay restates {shape_id}, which the snapshot no longer declares"
        );

        if shape_id == PROPERTIES_SHAPE {
            // The one shape that differs on purpose: the snapshot's members, restated as
            // the snapshot declares them, plus the member AWS carries and it does not.
            let declared_members = declared["members"]
                .as_object()
                .expect("the overlay declares the properties members");
            let modelled_members = modelled["members"]
                .as_object()
                .expect("the snapshot declares the properties members");
            for (member, modelled_member) in modelled_members {
                assert_eq!(
                    without_docs(&declared_members[member]),
                    without_docs(modelled_member),
                    "member {member} of {shape_id} must be restated as the snapshot declares \
                     it, got {:?}",
                    declared_members.get(member)
                );
            }
            let added: Vec<&String> = declared_members
                .keys()
                .filter(|member| !modelled_members.contains_key(*member))
                .collect();
            assert_eq!(
                added,
                vec![LOCAL_MEMBER],
                "{shape_id} may add {LOCAL_MEMBER} and nothing else, added {added:?}"
            );
            continue;
        }

        if shape_id == ACCELERATOR_SHAPE {
            // Declared without its members on purpose (see the constant): only the type is
            // read from the model, and the modelled Rust type carries the members.
            assert_eq!(
                declared["type"], modelled["type"],
                "{shape_id} must be restated with the type the snapshot gives it"
            );
            assert!(
                declared["members"]
                    .as_object()
                    .is_some_and(serde_json::Map::is_empty),
                "{shape_id} is restated for its type alone, so it must declare no members"
            );
            continue;
        }

        assert_eq!(
            without_docs(declared),
            without_docs(modelled),
            "{shape_id} must be restated exactly as the snapshot declares it"
        );
    }
}

#[tokio::test]
async fn the_declared_member_set_covers_the_snapshot_and_the_member_it_lacks() {
    // Half one: the local declaration covers the snapshot's own member list, and adds
    // exactly the member the snapshot does not declare.
    let snapshot_members = snapshot_property_members();
    assert!(
        !snapshot_members.contains_key(LOCAL_MEMBER),
        "the snapshot now declares {LOCAL_MEMBER}, so the local declaration should give way \
         to the modelled member; snapshot members {:?}",
        snapshot_members.keys().collect::<Vec<_>>()
    );
    let submitted = every_member();
    let submitted_members = submitted
        .as_object()
        .expect("the row is an object")
        .keys()
        .cloned()
        .collect::<Vec<String>>();
    for member in snapshot_members.keys() {
        assert!(
            submitted_members.contains(member),
            "the snapshot declares {member}, which this suite must drive through the wire; \
             submitted {submitted_members:?}"
        );
    }
    assert_eq!(
        submitted_members.len(),
        snapshot_members.len() + 1,
        "the declared set is the snapshot's members plus {LOCAL_MEMBER} only; submitted \
         {submitted_members:?}, snapshot {:?}",
        snapshot_members.keys().collect::<Vec<_>>()
    );

    // Half two: that whole set survives the trip, in the echo and in the stored WorkSpace.
    let (router, engine) = emulator_with_engine();
    let bundles = amazon_bundle_ids(&router).await;
    let bundle = bundles[bundles.len() - 1].clone();

    let sent = element(REGISTERED_DIRECTORY, "probe", &bundle, &submitted);
    let (failed, _) = submit_one(&router, &sent).await;
    assert_eq!(
        failed.len(),
        1,
        "the element is unsatisfiable before its directory is registered, got {failed:?}"
    );
    assert_eq!(
        &failed[0]["WorkspaceRequest"]["WorkspaceProperties"],
        &submitted,
        "the echo must carry every declared member as submitted, got {failed:?}"
    );

    register_directory(&router, REGISTERED_DIRECTORY).await;
    // The retry a client makes with the echo it got back asks for the same WorkSpace.
    let echoed = failed[0]["WorkspaceRequest"].clone();
    let (failed, pending) = submit_one(&router, &echoed).await;
    assert!(
        failed.is_empty(),
        "the retry of {echoed:?} must be satisfiable, got {failed:?}"
    );
    assert_eq!(
        pending.len(),
        1,
        "the retry creates the WorkSpace, got {pending:?}"
    );
    let workspace_id = pending[0]["WorkspaceId"]
        .as_str()
        .unwrap_or_else(|| panic!("a minted WorkspaceId, got {pending:?}"))
        .to_string();
    settle(&engine);
    let described = describe_one(&router, &workspace_id).await;
    assert_eq!(
        &described["WorkspaceProperties"], &submitted,
        "the stored WorkSpace must keep every declared member, got {described:?}"
    );
}
