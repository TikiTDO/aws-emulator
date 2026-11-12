// SPDX-License-Identifier: CC0-1.0

//! The WorkSpace lifecycle, end to end over the awsJson1_1 wire.
//!
//! Every case drives a real `MultiServiceRouter` with this service mounted on one shared
//! engine, so each assertion is about what a client would see: the request is serialized,
//! routed, validated, handled and serialized back.
//!
//! The rules under test, all of them about WorkSpaces this emulator persisted rather than
//! about any one recorded request:
//!
//! * `CreateWorkspaces` mints a `WorkspaceId` matching the model's
//!   `^ws-[0-9a-z]{8,63}$` and persists every member the request supplied.
//! * `DescribeWorkspaces` reports stored WorkSpaces, filtered by the single filter the
//!   request names, paginated over a stable creation order.
//! * `StartWorkspaces` / `StopWorkspaces` / `TerminateWorkspaces` move the persisted
//!   state, and report anything they cannot change per item in `FailedRequests` under a
//!   200, never as a top-level error.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_workspaces_workspacesservice::ops::types::WorkspaceRequest;
use service_com_amazonaws_workspaces_workspacesservice::state::{
    service_state, DEFAULT_RECYCLE_MODE, DEFAULT_RUNNING_MODE, ERROR_CODE_WORKSPACE_INVALID_STATE,
    ERROR_CODE_WORKSPACE_NOT_FOUND, MAX_STORED_WORKSPACES, MESSAGE_WORKSPACE_NOT_FOUND,
    TRANSITION_DELAY,
};
use service_com_amazonaws_workspaces_workspacesservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A router with only this service mounted, together with the engine it shares, so a
/// case that needs a pre-populated store can reach the same state the handlers see.
///
/// `pub(crate)` because `tests/all.rs` compiles every suite into ONE test crate, so the
/// suites beside this one share this harness rather than copying it.
pub(crate) fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suite asserts handler behaviour, not signing, so no authenticator runs.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}


/// Register `directory_id` through the wire, the way a client would, so a create
/// request naming it can resolve it.
///
/// `CreateWorkspaces` refuses an element whose `DirectoryId` is not registered with
/// WorkSpaces, so every case below that creates a WorkSpace registers its directory
/// first. The subnet ids match the model's `SubnetId` pattern
/// `^(subnet-([0-9a-f]{8}|[0-9a-f]{17}))$`.
///
/// `pub(crate)`: see [`emulator_with_engine`].
pub(crate) async fn register_directory(router: &MultiServiceRouter, directory_id: &str) {
    let request = json!({
        "DirectoryId": directory_id,
        "SubnetIds": ["subnet-11111111", "subnet-22222222"],
    });
    let (status, body) = call(router, "RegisterWorkspaceDirectory", &request).await;
    assert_eq!(
        status, 200,
        "case {directory_id}: RegisterWorkspaceDirectory must answer 200, body {body:?}"
    );
    assert_eq!(
        body["DirectoryId"].as_str(),
        Some(directory_id),
        "case {directory_id}: the registration reports the directory it registered, body {body:?}"
    );
}

/// Call one operation on `router` and return `(status, body)`.
///
/// The `Authorization` credential scope is what routes the request: it names the signing
/// name (`workspaces`), which is the signal `resolve_route_key` reads first and the only
/// one that agrees with this service's identity.
///
/// `pub(crate)`: see [`emulator_with_engine`].
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = smithy_server_runtime::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "workspaces.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("WorkspacesService.{operation}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIAEMULATOREXAMPLE/20260101/us-east-1/workspaces/aws4_request, SignedHeaders=host, Signature=00",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("serialize the request body"),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Materialise the lifecycle transitions the clock owes, by naming the instant to
/// settle against rather than sleeping until it arrives.
///
/// Asking the store about a future instant is deterministic, so the suite asserts the
/// rule instead of racing a wall clock on a loaded runner. The handlers themselves
/// always settle against the real clock.
///
/// `pub(crate)`: see [`emulator_with_engine`].
pub(crate) fn settle(engine: &Arc<engine::InMemoryEngine>) {
    let state = service_state(&**engine).expect("the WorkSpaces state store");
    state
        .lock()
        .settle_at(Instant::now() + TRANSITION_DELAY + Duration::from_millis(1));
}

/// The EFFECTIVE `WorkspaceProperties` a submitted property object is reported as,
/// wherever this service reports the properties a request asked for: every member the
/// request submitted, unchanged, plus the value the service resolves for a switch-like
/// property the request left out (`RunningMode`, `RecycleMode`).
///
/// `pub(crate)` because every suite that submits a create asserts against the same
/// resolution: see [`emulator_with_engine`].
pub(crate) fn effective_properties(submitted: Option<&Value>) -> Value {
    let mut properties = submitted.cloned().unwrap_or_else(|| json!({}));
    if properties.get("RunningMode").is_none() {
        properties["RunningMode"] = json!(DEFAULT_RUNNING_MODE);
    }
    if properties.get("RecycleMode").is_none() {
        properties["RecycleMode"] = json!(DEFAULT_RECYCLE_MODE);
    }
    properties
}

/// Create one WorkSpace and return its id, asserting the call was accepted.
///
/// `pub(crate)`: see [`emulator_with_engine`].
pub(crate) async fn create_one(router: &MultiServiceRouter, request: &Value) -> String {
    let (status, body) = call(router, "CreateWorkspaces", &json!({ "Workspaces": [request] })).await;
    assert_eq!(
        status, 200,
        "case {request:?}: CreateWorkspaces must answer 200, body {body:?}"
    );
    assert_eq!(
        body["FailedRequests"].as_array().map(Vec::len),
        Some(0),
        "case {request:?}: CreateWorkspaces must report no failures, body {body:?}"
    );
    let pending = body["PendingRequests"]
        .as_array()
        .unwrap_or_else(|| panic!("case {request:?}: PendingRequests must be a list, got {body:?}"));
    assert_eq!(
        pending.len(),
        1,
        "case {request:?}: expected one pending request, got {pending:?}"
    );
    pending[0]["WorkspaceId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {request:?}: a minted WorkspaceId, got {pending:?}"))
        .to_string()
}

/// Describe with `filters` and return the `WorkspaceId`s in the order reported.
async fn describe_ids(router: &MultiServiceRouter, filters: &Value) -> Vec<String> {
    let (status, body) = call(router, "DescribeWorkspaces", filters).await;
    assert_eq!(
        status, 200,
        "case {filters:?}: DescribeWorkspaces must answer 200, body {body:?}"
    );
    reported_ids(filters, &body)
}

fn reported_ids(case: &Value, body: &Value) -> Vec<String> {
    body["Workspaces"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: Workspaces must be a list, got {body:?}"))
        .iter()
        .map(|workspace| {
            workspace["WorkspaceId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: every Workspace carries an id, got {body:?}"))
                .to_string()
        })
        .collect()
}

/// The one described WorkSpace matching `workspace_id`.
///
/// `pub(crate)`: see [`emulator_with_engine`] — the suites beside this one read a
/// WorkSpace back through this rather than copying the call.
pub(crate) async fn describe_one(router: &MultiServiceRouter, workspace_id: &str) -> Value {
    let filters = json!({ "WorkspaceIds": [workspace_id] });
    let (status, body) = call(router, "DescribeWorkspaces", &filters).await;
    assert_eq!(
        status, 200,
        "case {workspace_id}: DescribeWorkspaces must answer 200, body {body:?}"
    );
    let workspaces = body["Workspaces"]
        .as_array()
        .unwrap_or_else(|| panic!("case {workspace_id}: Workspaces must be a list, got {body:?}"));
    assert_eq!(
        workspaces.len(),
        1,
        "case {workspace_id}: expected exactly that WorkSpace, got {workspaces:?}"
    );
    workspaces[0].clone()
}

/// `WorkspaceId` pattern from the model: `Workspace$WorkspaceId` is
/// `^ws-[0-9a-z]{8,63}$`. Checked by hand so the suite pulls in no regex engine.
fn is_modelled_workspace_id(candidate: &str) -> bool {
    let Some(suffix) = candidate.strip_prefix("ws-") else {
        return false;
    };
    (8..=63).contains(&suffix.len())
        && suffix
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase())
}

/// The `ErrorCode`/`ErrorMessage`/`WorkspaceId` triples of a batch response.
///
/// `pub(crate)`: see [`emulator_with_engine`].
pub(crate) fn failed_requests(case: &str, body: &Value) -> Vec<(String, String, String)> {
    body["FailedRequests"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: FailedRequests must be a list, got {body:?}"))
        .iter()
        .map(|failure| {
            (
                failure["WorkspaceId"].as_str().unwrap_or("").to_string(),
                failure["ErrorCode"].as_str().unwrap_or("").to_string(),
                failure["ErrorMessage"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

/// Every create request a row of the round-trip table sends. The rows differ along the
/// optional-member space `WorkspaceRequest` declares: nothing optional, the encryption
/// members, `WorkspaceProperties`, tags, an IPv6 address, and the user-decoupled form
/// (`UserName` = the reserved `[UNDEFINED]`, which the model says requires
/// `WorkspaceName`).
fn create_request_table() -> Vec<Value> {
    vec![
        json!({
            "DirectoryId": "d-1234567890",
            "UserName": "alice",
            "BundleId": "wsb-amznstandard",
        }),
        json!({
            "DirectoryId": "d-1234567890",
            "UserName": "bob",
            "BundleId": "wsb-amznvalue",
            "VolumeEncryptionKey": "arn:aws:kms:us-east-1:123456789012:key/11111111-2222-3333-4444-555555555555",
            "UserVolumeEncryptionEnabled": true,
            "RootVolumeEncryptionEnabled": false,
        }),
        json!({
            // RunningMode / ComputeTypeName / Protocols values come from the model's
            // RunningMode, Compute and Protocol enums.
            "DirectoryId": "d-00000000ab",
            "UserName": "carol",
            "BundleId": "wsb-amznperformance",
            "WorkspaceProperties": {
                "RunningMode": "AUTO_STOP",
                "RunningModeAutoStopTimeoutInMinutes": 60,
                "RootVolumeSizeGib": 80,
                "UserVolumeSizeGib": 50,
                "ComputeTypeName": "PERFORMANCE",
                "Protocols": ["PCOIP"],
            },
        }),
        json!({
            "DirectoryId": "wsd-abcdef0123",
            "UserName": "DAVE",
            "BundleId": "wsb-amznstandard",
            "Tags": [{ "Key": "team", "Value": "research" }, { "Key": "cost-center" }],
            "Ipv6Address": "2001:db8::1",
        }),
        json!({
            "DirectoryId": "d-1234567890",
            "UserName": "[UNDEFINED]",
            "BundleId": "wsb-amznstandard",
            "WorkspaceName": "shared_station_1",
        }),
    ]
}

#[tokio::test]
async fn created_workspaces_are_persisted_and_described_back() {
    let (router, engine) = emulator_with_engine();
    for directory_id in ["d-1234567890", "d-00000000ab", "wsd-abcdef0123"] {
        register_directory(&router, directory_id).await;
    }
    let mut created: Vec<(Value, String)> = Vec::new();

    for request in create_request_table() {
        let (status, body) =
            call(&router, "CreateWorkspaces", &json!({ "Workspaces": [request] })).await;
        assert_eq!(
            status, 200,
            "case {request:?}: CreateWorkspaces must answer 200, body {body:?}"
        );
        let pending = body["PendingRequests"]
            .as_array()
            .unwrap_or_else(|| panic!("case {request:?}: PendingRequests list, got {body:?}"));
        assert_eq!(
            pending.len(),
            1,
            "case {request:?}: expected one pending request, got {pending:?}"
        );
        let workspace_id = pending[0]["WorkspaceId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {request:?}: a minted WorkspaceId, got {pending:?}"))
            .to_string();
        assert!(
            is_modelled_workspace_id(&workspace_id),
            "case {request:?}: minted id must match ^ws-[0-9a-z]{{8,63}}$, got {workspace_id:?}"
        );
        assert_eq!(
            pending[0]["State"].as_str(),
            Some("PENDING"),
            "case {request:?}: a WorkSpace being created is PENDING, got {pending:?}"
        );
        created.push((request, workspace_id));
    }

    let minted: std::collections::BTreeSet<&String> =
        created.iter().map(|(_, id)| id).collect();
    assert_eq!(
        minted.len(),
        created.len(),
        "every minted WorkspaceId must be distinct, got {created:?}"
    );

    settle(&engine);

    for (request, workspace_id) in &created {
        let described = describe_one(&router, workspace_id).await;
        // `Workspace` carries a member of the same name for every `WorkspaceRequest`
        // member except `Tags` (the WorkSpace shape has none; tags are read through
        // DescribeTags, which is its own gap issue), so every supplied value must come
        // back unchanged.
        for (member, supplied) in request.as_object().expect("a request object") {
            if member == "Tags" {
                continue;
            }
            // `WorkspaceProperties` is the one member reported as the EFFECTIVE set
            // rather than exactly as supplied: see [`effective_properties`].
            let expected = if member == "WorkspaceProperties" {
                effective_properties(Some(supplied))
            } else {
                supplied.clone()
            };
            assert_eq!(
                &described[member], &expected,
                "case {workspace_id}: {member} must be reported as supplied, got {described:?}"
            );
        }
        assert_eq!(
            described["State"].as_str(),
            Some("AVAILABLE"),
            "case {workspace_id}: a created WorkSpace settles to AVAILABLE, got {described:?}"
        );
    }

    // No filter: every stored WorkSpace, in creation order.
    let all = describe_ids(&router, &json!({})).await;
    let expected: Vec<String> = created.iter().map(|(_, id)| id.clone()).collect();
    assert_eq!(
        all, expected,
        "DescribeWorkspaces with no filter reports every stored WorkSpace in creation order"
    );
}

#[tokio::test]
async fn lifecycle_transitions_are_persisted_through_describe() {
    let (router, engine) = emulator_with_engine();
    register_directory(&router, "d-1234567890").await;
    let workspace_id = create_one(
        &router,
        &json!({
            "DirectoryId": "d-1234567890",
            "UserName": "alice",
            "BundleId": "wsb-amznstandard",
        }),
    )
    .await;

    // (operation, request member, the transient state the call returns in, the state the
    // clock owes). Create is already under way from the call above.
    let transitions: Vec<(&str, &str, &str, &str)> = vec![
        ("", "", "PENDING", "AVAILABLE"),
        ("StopWorkspaces", "StopWorkspaceRequests", "STOPPING", "STOPPED"),
        ("StartWorkspaces", "StartWorkspaceRequests", "STARTING", "AVAILABLE"),
    ];

    for (operation, member, transient, settled) in transitions {
        if !operation.is_empty() {
            let (status, body) = call(
                &router,
                operation,
                &json!({ member: [{ "WorkspaceId": workspace_id }] }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {operation}: must answer 200, body {body:?}"
            );
            assert_eq!(
                failed_requests(operation, &body),
                Vec::new(),
                "case {operation}: a stored WorkSpace in the right state must not fail"
            );
        }

        let described = describe_one(&router, &workspace_id).await;
        assert_eq!(
            described["State"].as_str(),
            Some(transient),
            "case {operation}: the call returns while the WorkSpace is still {transient}, got {described:?}"
        );

        settle(&engine);
        let described = describe_one(&router, &workspace_id).await;
        assert_eq!(
            described["State"].as_str(),
            Some(settled),
            "case {operation}: the WorkSpace settles to {settled}, got {described:?}"
        );
    }

    // Terminate: TERMINATING while it runs, then gone from the store.
    let (status, body) = call(
        &router,
        "TerminateWorkspaces",
        &json!({ "TerminateWorkspaceRequests": [{ "WorkspaceId": workspace_id }] }),
    )
    .await;
    assert_eq!(status, 200, "TerminateWorkspaces must answer 200, body {body:?}");
    assert_eq!(
        failed_requests("TerminateWorkspaces", &body),
        Vec::new(),
        "terminating a stored WorkSpace must not fail"
    );
    let described = describe_one(&router, &workspace_id).await;
    assert_eq!(
        described["State"].as_str(),
        Some("TERMINATING"),
        "TerminateWorkspaces returns while the WorkSpace is still TERMINATING, got {described:?}"
    );

    settle(&engine);
    assert_eq!(
        describe_ids(&router, &json!({})).await,
        Vec::<String>::new(),
        "a terminated WorkSpace is no longer reported"
    );
}

#[tokio::test]
async fn unknown_and_wrong_state_ids_are_reported_per_item() {
    let (router, engine) = emulator_with_engine();
    register_directory(&router, "d-1234567890").await;
    let available = create_one(
        &router,
        &json!({
            "DirectoryId": "d-1234567890",
            "UserName": "alice",
            "BundleId": "wsb-amznstandard",
        }),
    )
    .await;
    let pending = create_one(
        &router,
        &json!({
            "DirectoryId": "d-1234567890",
            "UserName": "bob",
            "BundleId": "wsb-amznstandard",
        }),
    )
    .await;
    settle(&engine);
    // `bob` is AVAILABLE now; put it back into a transient state so a wrong-state row
    // has something to aim at.
    let (_, _) = call(
        &router,
        "StopWorkspaces",
        &json!({ "StopWorkspaceRequests": [{ "WorkspaceId": pending }] }),
    )
    .await;

    // (operation, request member, the id to send, the expected per-item ErrorCode,
    // the expected ErrorMessage where the service fixes it — `None` means the message
    // is built from the request and must name the WorkSpace).
    let not_found_message = Some(MESSAGE_WORKSPACE_NOT_FOUND);
    let cases: Vec<(&str, &str, &str, &str, Option<&str>)> = vec![
        // An id no WorkSpace carries is a per-item failure, not a top-level error.
        (
            "StopWorkspaces",
            "StopWorkspaceRequests",
            "ws-00000000",
            ERROR_CODE_WORKSPACE_NOT_FOUND,
            not_found_message,
        ),
        (
            "StartWorkspaces",
            "StartWorkspaceRequests",
            "ws-00000000",
            ERROR_CODE_WORKSPACE_NOT_FOUND,
            not_found_message,
        ),
        (
            "TerminateWorkspaces",
            "TerminateWorkspaceRequests",
            "ws-00000000",
            ERROR_CODE_WORKSPACE_NOT_FOUND,
            not_found_message,
        ),
        // A state that forbids the change is a per-item failure too: an AVAILABLE
        // WorkSpace cannot be started, and a STOPPING one cannot be stopped... it is
        // already stopping, so aim the wrong-state row at starting it instead.
        (
            "StartWorkspaces",
            "StartWorkspaceRequests",
            &available,
            ERROR_CODE_WORKSPACE_INVALID_STATE,
            None,
        ),
        (
            "StartWorkspaces",
            "StartWorkspaceRequests",
            &pending,
            ERROR_CODE_WORKSPACE_INVALID_STATE,
            None,
        ),
    ];

    for (operation, member, workspace_id, expected_code, expected_message) in cases {
        let case = format!("{operation}({workspace_id})");
        let (status, body) = call(
            &router,
            operation,
            &json!({ member: [{ "WorkspaceId": workspace_id }] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: a per-item failure is still HTTP 200, body {body:?}"
        );
        let failures = failed_requests(&case, &body);
        assert_eq!(
            failures.len(),
            1,
            "case {case}: expected exactly one FailedRequests entry, got {failures:?}"
        );
        let (reported_id, code, message) = &failures[0];
        assert_eq!(
            reported_id, workspace_id,
            "case {case}: the failure names the WorkSpace it was asked about, got {failures:?}"
        );
        assert_eq!(
            code, expected_code,
            "case {case}: expected ErrorCode {expected_code}, got {failures:?}"
        );
        match expected_message {
            Some(expected) => assert_eq!(
                message, expected,
                "case {case}: expected the fixed ErrorMessage {expected:?}, got {message:?}"
            ),
            None => assert!(
                message.contains(workspace_id),
                "case {case}: the message names the WorkSpace, got {message:?}"
            ),
        }
    }
}

#[tokio::test]
async fn create_reports_per_item_failures_without_a_top_level_error() {
    let (router, engine) = emulator_with_engine();
    register_directory(&router, "d-1234567890").await;
    let taken = json!({
        "DirectoryId": "d-1234567890",
        "UserName": "Alice",
        "BundleId": "wsb-amznstandard",
    });
    let first = create_one(&router, &taken).await;
    settle(&engine);

    // (the request, the expected ErrorCode). A directory user holds at most one
    // WorkSpace, and the model's `UserName` doc states the user name is not
    // case-sensitive, so a differently-cased name is the same user. A user-decoupled
    // request (`UserName` = `[UNDEFINED]`) must carry `WorkspaceName`. The codes are
    // the per-item `ErrorType` vocabulary, not exception shape names.
    let cases: Vec<(Value, &str)> = vec![
        (taken.clone(), "ResourceExists.WorkspaceUser"),
        (
            json!({
                "DirectoryId": "d-1234567890",
                "UserName": "alice",
                "BundleId": "wsb-amznvalue",
            }),
            "ResourceExists.WorkspaceUser",
        ),
        (
            json!({
                "DirectoryId": "d-1234567890",
                "UserName": "[UNDEFINED]",
                "BundleId": "wsb-amznstandard",
            }),
            "InvalidParameterValues",
        ),
    ];

    for (request, expected_code) in cases {
        // The echo carries the submitted request, with `Tags` reported as a list even
        // for a request that sent none and `WorkspaceProperties` reported as the
        // effective property set (see `create_workspaces_effective_properties`); none of
        // these cases submits any property, so the effective set is what the service
        // resolves on its own.
        let mut echoed = request.clone();
        echoed["Tags"] = json!([]);
        echoed["WorkspaceProperties"] = effective_properties(None);
        let (status, body) =
            call(&router, "CreateWorkspaces", &json!({ "Workspaces": [request] })).await;
        assert_eq!(
            status, 200,
            "case {request:?}: a per-item create failure is still HTTP 200, body {body:?}"
        );
        assert_eq!(
            body["PendingRequests"].as_array().map(Vec::len),
            Some(0),
            "case {request:?}: nothing may be created, got {body:?}"
        );
        let failures = body["FailedRequests"]
            .as_array()
            .unwrap_or_else(|| panic!("case {request:?}: FailedRequests list, got {body:?}"));
        assert_eq!(
            failures.len(),
            1,
            "case {request:?}: expected one FailedRequests entry, got {failures:?}"
        );
        assert_eq!(
            failures[0]["ErrorCode"].as_str(),
            Some(expected_code),
            "case {request:?}: expected ErrorCode {expected_code}, got {failures:?}"
        );
        assert_eq!(
            &failures[0]["WorkspaceRequest"], &echoed,
            "case {request:?}: the failure echoes the request it refused, got {failures:?}"
        );
    }

    // Nothing was created beyond the first WorkSpace.
    assert_eq!(
        describe_ids(&router, &json!({})).await,
        vec![first],
        "a refused create must not persist a WorkSpace"
    );
}

#[tokio::test]
async fn describe_filters_select_only_matching_workspaces() {
    let (router, engine) = emulator_with_engine();
    for directory_id in ["d-1234567890", "wsd-abcdef0123"] {
        register_directory(&router, directory_id).await;
    }
    let alice = create_one(
        &router,
        &json!({
            "DirectoryId": "d-1234567890",
            "UserName": "alice",
            "BundleId": "wsb-amznstandard",
            "WorkspaceName": "alice_station",
        }),
    )
    .await;
    let bob = create_one(
        &router,
        &json!({
            "DirectoryId": "d-1234567890",
            "UserName": "bob",
            "BundleId": "wsb-amznvalue",
        }),
    )
    .await;
    let carol = create_one(
        &router,
        &json!({
            "DirectoryId": "wsd-abcdef0123",
            "UserName": "carol",
            "BundleId": "wsb-amznstandard",
        }),
    )
    .await;
    settle(&engine);

    // (filter, the ids it must report, in creation order).
    let cases: Vec<(Value, Vec<String>)> = vec![
        (json!({}), vec![alice.clone(), bob.clone(), carol.clone()]),
        (
            json!({ "WorkspaceIds": [alice.clone(), carol.clone()] }),
            vec![alice.clone(), carol.clone()],
        ),
        // An id that names nothing filters to nothing; it is not an error.
        (json!({ "WorkspaceIds": ["ws-00000000"] }), vec![]),
        (
            json!({ "WorkspaceIds": [bob.clone(), "ws-00000000"] }),
            vec![bob.clone()],
        ),
        (
            json!({ "DirectoryId": "d-1234567890" }),
            vec![alice.clone(), bob.clone()],
        ),
        (
            json!({ "DirectoryId": "wsd-abcdef0123" }),
            vec![carol.clone()],
        ),
        (json!({ "DirectoryId": "d-9999999999" }), vec![]),
        // UserName narrows a DirectoryId filter, and is not case-sensitive.
        (
            json!({ "DirectoryId": "d-1234567890", "UserName": "bob" }),
            vec![bob.clone()],
        ),
        (
            json!({ "DirectoryId": "d-1234567890", "UserName": "BOB" }),
            vec![bob.clone()],
        ),
        (
            json!({ "BundleId": "wsb-amznstandard" }),
            vec![alice.clone(), carol.clone()],
        ),
        (json!({ "BundleId": "wsb-00000000" }), vec![]),
        (
            json!({ "WorkspaceName": "alice_station" }),
            vec![alice.clone()],
        ),
        (json!({ "WorkspaceName": "nobodys_station" }), vec![]),
    ];

    for (filters, expected) in cases {
        let reported = describe_ids(&router, &filters).await;
        assert_eq!(
            reported, expected,
            "case {filters:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn describe_paginates_over_a_stable_creation_order() {
    let (router, engine) = emulator_with_engine();
    register_directory(&router, "d-1234567890").await;
    let mut expected = Vec::new();
    for user in ["u1", "u2", "u3", "u4", "u5"] {
        expected.push(
            create_one(
                &router,
                &json!({
                    "DirectoryId": "d-1234567890",
                    "UserName": user,
                    "BundleId": "wsb-amznstandard",
                }),
            )
            .await,
        );
    }
    settle(&engine);

    // Every page size the model allows at or below the stored count, plus the bound
    // itself (`Limit` is `range 1..25`) and a size that divides the count exactly.
    for limit in [1usize, 2, 3, 5, 25] {
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut filters = json!({ "Limit": limit });
            if let Some(token) = &next_token {
                filters["NextToken"] = json!(token);
            }
            let (status, body) = call(&router, "DescribeWorkspaces", &filters).await;
            assert_eq!(
                status, 200,
                "case limit={limit}: DescribeWorkspaces must answer 200, body {body:?}"
            );
            let page = reported_ids(&filters, &body);
            assert!(
                page.len() <= limit,
                "case limit={limit}: a page may not exceed Limit, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case limit={limit}: pagination must terminate, walked {walked:?}"
            );
            match body["NextToken"].as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, expected,
            "case limit={limit}: paging must walk every WorkSpace once, in creation order"
        );
    }

    // The documented shape of the first page: Limit items plus a NextToken, and the
    // follow-up call resumes exactly where it ended.
    let (_, first) = call(&router, "DescribeWorkspaces", &json!({ "Limit": 2 })).await;
    assert_eq!(
        reported_ids(&json!({ "Limit": 2 }), &first),
        expected[..2].to_vec(),
        "the first page of two is the first two WorkSpaces, got {first:?}"
    );
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a first page of two out of five carries a NextToken, got {first:?}"));
    let resumed = json!({ "Limit": 3, "NextToken": token });
    let (_, second) = call(&router, "DescribeWorkspaces", &resumed).await;
    assert_eq!(
        reported_ids(&resumed, &second),
        expected[2..].to_vec(),
        "the follow-up page resumes where the previous one ended, got {second:?}"
    );
    assert!(
        second["NextToken"].is_null(),
        "the last page carries no NextToken, got {second:?}"
    );
}

#[tokio::test]
async fn the_workspace_store_refuses_to_grow_past_its_bound() {
    let (router, engine) = emulator_with_engine();
    register_directory(&router, "d-1234567890").await;
    // Fill the store to its bound through the same state API the handler uses, so the
    // case costs one wire call rather than MAX_STORED_WORKSPACES of them.
    {
        let state = service_state(&*engine).expect("the WorkSpaces state store");
        let mut state = state.lock();
        for index in 0..MAX_STORED_WORKSPACES {
            let request = WorkspaceRequest {
                directory_id: "d-1234567890".to_string(),
                user_name: format!("user{index}"),
                bundle_id: "wsb-amznstandard".to_string(),
                ..WorkspaceRequest::default()
            };
            state
                .create_workspace(&request)
                .unwrap_or_else(|failure| panic!("case {index}: filling must succeed, got {failure:?}"));
        }
        assert_eq!(
            state.workspace_count(),
            MAX_STORED_WORKSPACES,
            "the store must hold exactly its bound before the refused create"
        );
    }

    let request = json!({
        "DirectoryId": "d-1234567890",
        "UserName": "one-too-many",
        "BundleId": "wsb-amznstandard",
    });
    let (status, body) =
        call(&router, "CreateWorkspaces", &json!({ "Workspaces": [request] })).await;
    assert_eq!(
        status, 200,
        "a create refused for a limit is still a per-item failure under 200, body {body:?}"
    );
    assert_eq!(
        body["PendingRequests"].as_array().map(Vec::len),
        Some(0),
        "nothing may be created past the bound, got {body:?}"
    );
    let failures = body["FailedRequests"]
        .as_array()
        .unwrap_or_else(|| panic!("FailedRequests must be a list, got {body:?}"));
    assert_eq!(
        failures.len(),
        1,
        "expected one FailedRequests entry, got {failures:?}"
    );
    assert_eq!(
        failures[0]["ErrorCode"].as_str(),
        Some("ResourceLimitExceeded"),
        "an exceeded limit is reported as the per-item ResourceLimitExceeded code, got {failures:?}"
    );

    let state = service_state(&*engine).expect("the WorkSpaces state store");
    assert_eq!(
        state.lock().workspace_count(),
        MAX_STORED_WORKSPACES,
        "a refused create must not grow the store"
    );
}

#[tokio::test]
async fn describe_refuses_filter_combinations_the_service_forbids() {
    let (router, engine) = emulator_with_engine();
    register_directory(&router, "d-1234567890").await;
    create_one(
        &router,
        &json!({
            "DirectoryId": "d-1234567890",
            "UserName": "alice",
            "BundleId": "wsb-amznstandard",
            "WorkspaceName": "alice_station",
        }),
    )
    .await;
    settle(&engine);

    // Each row names a combination the model's own docs forbid: WorkspaceIds,
    // DirectoryId, BundleId and WorkspaceName each say "You cannot combine this
    // parameter with any other filter", UserName says "You must specify this parameter
    // with DirectoryId", and a NextToken this service never minted names no page.
    let cases: Vec<Value> = vec![
        json!({ "WorkspaceIds": ["ws-00000000"], "DirectoryId": "d-1234567890" }),
        json!({ "WorkspaceIds": ["ws-00000000"], "BundleId": "wsb-amznstandard" }),
        json!({ "DirectoryId": "d-1234567890", "BundleId": "wsb-amznstandard" }),
        json!({ "BundleId": "wsb-amznstandard", "WorkspaceName": "alice_station" }),
        json!({ "DirectoryId": "d-1234567890", "WorkspaceName": "alice_station" }),
        json!({ "UserName": "alice" }),
        json!({ "NextToken": "not-a-token" }),
    ];

    for filters in cases {
        let (status, body) = call(&router, "DescribeWorkspaces", &filters).await;
        assert_eq!(
            status, 400,
            "case {filters:?}: the request must be refused, body {body:?}"
        );
        let kind = body["__type"].as_str().unwrap_or_default();
        assert!(
            kind.ends_with("InvalidParameterValuesException"),
            "case {filters:?}: expected InvalidParameterValuesException, got {body:?}"
        );
    }
}

#[tokio::test]
async fn paging_resumes_after_the_last_reported_workspace_when_the_store_changes() {
    // The invariant: a page resumes after the WorkSpace the token names, so a
    // WorkSpace removed from an EARLIER position between two pages shifts nothing
    // and no later WorkSpace is skipped. Each row removes a different number of
    // earlier WorkSpaces at a different page size.
    let cases: [(usize, usize); 4] = [
        // (page size, how many of the already-reported WorkSpaces to terminate)
        (2, 1),
        (2, 2),
        (1, 1),
        (3, 3),
    ];

    for (limit, terminated) in cases {
        let (router, engine) = emulator_with_engine();
        register_directory(&router, "d-1234567890").await;
        let mut created = Vec::new();
        for index in 0..6 {
            created.push(
                create_one(
                    &router,
                    &json!({
                        "DirectoryId": "d-1234567890",
                        "UserName": format!("user{index}"),
                        "BundleId": "wsb-amznstandard",
                    }),
                )
                .await,
            );
        }
        settle(&engine);

        let first_filters = json!({ "Limit": limit });
        let (_, first) = call(&router, "DescribeWorkspaces", &first_filters).await;
        let first_page = reported_ids(&first_filters, &first);
        assert_eq!(
            first_page,
            created[..limit].to_vec(),
            "case limit={limit} terminated={terminated}: the first page is the oldest {limit}, got {first:?}"
        );
        let token = first["NextToken"].as_str().unwrap_or_else(|| {
            panic!("case limit={limit} terminated={terminated}: a first page of {limit} out of 6 carries a NextToken, got {first:?}")
        });

        // Terminate WorkSpaces the first page already reported, and let the clock
        // complete the termination so they leave the store entirely.
        let requests: Vec<Value> = created[..terminated]
            .iter()
            .map(|workspace_id| json!({ "WorkspaceId": workspace_id }))
            .collect();
        let (status, body) = call(
            &router,
            "TerminateWorkspaces",
            &json!({ "TerminateWorkspaceRequests": requests }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case limit={limit} terminated={terminated}: TerminateWorkspaces must answer 200, body {body:?}"
        );
        settle(&engine);

        // Resuming must report the WorkSpaces after the first page, unaffected by
        // the removals behind it.
        let resumed = json!({ "Limit": limit, "NextToken": token });
        let walked = {
            let mut walked: Vec<String> = Vec::new();
            let mut filters = resumed.clone();
            loop {
                let (status, body) = call(&router, "DescribeWorkspaces", &filters).await;
                assert_eq!(
                    status, 200,
                    "case limit={limit} terminated={terminated}: DescribeWorkspaces must answer 200, body {body:?}"
                );
                walked.extend(reported_ids(&filters, &body));
                match body["NextToken"].as_str() {
                    Some(next) => filters["NextToken"] = json!(next),
                    None => break walked,
                }
            }
        };
        assert_eq!(
            walked,
            created[limit..].to_vec(),
            "case limit={limit} terminated={terminated}: resuming must report every later WorkSpace once, got {walked:?}"
        );
    }
}
