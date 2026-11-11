// SPDX-License-Identifier: CC0-1.0

//! A pipeline route addresses nested resources — the workspace, the pipeline
//! inside it and, for a read, a version of that pipeline — and the refusal names
//! the OUTERMOST one the request got wrong: the workspace before the pipeline, and
//! the pipeline before a version of it. Two operations addressing the same path
//! therefore state the same absence.
//!
//! The rows span the class rather than one recorded request: every verb a pipeline
//! path carries (`DescribePipeline`, `UpdatePipeline`, `DeletePipeline`,
//! `ListPipelines`), each depth of wrongness (workspace, pipeline, version, and
//! several at once), a workspace that never existed and one that existed and was
//! deleted, a workspace that holds no pipeline at all and one that holds another
//! pipeline, and workspace names at both ends of the modelled `WorkspaceName`
//! length (smithy_query query="operation" operation="DescribePipeline":
//! `workspaceName` len 1..64, pattern `^[a-zA-Z0-9_-]+$`; `pipelineVersion` is
//! bound to the `version` query string).
//!
//! The expected strings are written out BY HAND as an independent oracle: this
//! suite never calls the crate's message helpers, so a handler that drifts to a
//! template of its own cannot agree with itself.
//!
//! Everything is driven over HTTP through the same `MultiServiceRouter` the
//! emulator boots with, so every assertion is about the wire behaviour a client
//! sees.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, error_code, router};
use crate::hierarchies_etags_and_refusals::error_message;
use crate::support::ensure_workspace;

/// The workspace the fixture opens and keeps, holding one pipeline.
const HELD: &str = "depth-plant";

/// A workspace the fixture opens and leaves EMPTY, so a pipeline name that exists
/// elsewhere is absent in it.
const EMPTY: &str = "depth-plant-empty";

/// A workspace the fixture opens, fills, empties and then DELETES, so a route
/// under it is wrong about a workspace that once existed.
const REMOVED: &str = "depth-plant-removed";

/// The pipeline the fixture creates in [`HELD`] and in [`REMOVED`].
const PIPELINE: &str = "nightly-rollup";

/// A pipeline name nothing was created under.
const UNKNOWN_PIPELINE: &str = "no-such-pipeline";

/// Workspace names no workspace answers to, each inside the modelled
/// `WorkspaceName`: a hyphenated name, a mixed-case name with an underscore and
/// digits, one at the published minimum length of 1 and one at the published
/// maximum of 64.
const UNKNOWN_WORKSPACES: [&str; 4] = [
    "no-such-workspace",
    "Absent_Workspace_7",
    "x",
    "wwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwww",
];

/// Versions no write of the fixture's pipeline ever produced. `0` is inside the
/// modelled `Version` pattern (`^(0|([1-9]{1}\d*))$`) and below the first version
/// a write mints; the others are past the newest one.
const UNKNOWN_VERSIONS: [&str; 3] = ["0", "9", "1000000000"];

/// The resource KINDS this service words its absences with, as the wire reports
/// them.
const WORKSPACE_KIND: &str = "Workspace";
const PIPELINE_KIND: &str = "Pipeline";

/// One row: the case name, the HTTP method, the URI, the request body, and the
/// absence the refusal must state.
type Row = (String, &'static str, String, Option<Value>, String);

/// How this service states an absence, written out by hand.
fn absent(kind: &str, identifier: &str) -> String {
    format!("{kind} with ID {identifier} not found")
}

/// `GET|PUT|DELETE /workspaces/{workspaceName}/pipelines/{pipelineName}`.
fn pipeline_uri(workspace: &str, pipeline: &str) -> String {
    format!("/workspaces/{workspace}/pipelines/{pipeline}")
}

/// The same, naming one version of the pipeline on the `version` query string.
fn versioned_uri(workspace: &str, pipeline: &str, version: &str) -> String {
    format!("{}?version={version}", pipeline_uri(workspace, pipeline))
}

/// `CreatePipeline` in `workspace`, which the fixture opens first.
async fn create_pipeline(router: &MultiServiceRouter, workspace: &str, pipeline: &str) {
    ensure_workspace(router, workspace).await;
    let (status, body) = call(
        router,
        "POST",
        &format!("/workspaces/{workspace}/pipelines"),
        Some(json!({ "pipelineName": pipeline, "computations": [] })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreatePipeline {workspace}/{pipeline}: got {status} {body}"
    );
}

/// Two open workspaces — one holding [`PIPELINE`] at two versions, one empty —
/// and a third that held the same pipeline and was then emptied and deleted.
async fn fixture(router: &MultiServiceRouter) {
    create_pipeline(router, HELD, PIPELINE).await;
    // A second write, so the pipeline the rows address holds more than one version
    // and a version row is about a version a write could plausibly have produced.
    let (status, updated) = call(
        router,
        "PUT",
        &pipeline_uri(HELD, PIPELINE),
        Some(json!({ "description": "the second version" })),
    )
    .await;
    assert_eq!(
        status, 200,
        "UpdatePipeline {HELD}/{PIPELINE}: got {status} {updated}"
    );

    ensure_workspace(router, EMPTY).await;

    create_pipeline(router, REMOVED, PIPELINE).await;
    let (status, deleted) = call(router, "DELETE", &pipeline_uri(REMOVED, PIPELINE), None).await;
    assert_eq!(
        status, 200,
        "DeletePipeline {REMOVED}/{PIPELINE}: got {status} {deleted}"
    );
    let (status, gone) = call(router, "DELETE", &format!("/workspaces/{REMOVED}"), None).await;
    assert_eq!(
        status, 200,
        "DeleteWorkspace {REMOVED}: got {status} {gone}"
    );
}

/// Drive `rows` and assert each is the modelled 404 stating the absence the row
/// expects.
async fn assert_rows(router: &MultiServiceRouter, rows: Vec<Row>) {
    for (case, method, uri, body, expected) in rows {
        let (status, answered) = call(router, method, &uri, body).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answered}"
        );
        assert_eq!(
            error_message(&answered),
            expected,
            "case {case:?}: expected {expected:?}, got {answered}"
        );
    }
}

#[tokio::test]
async fn a_pipeline_route_refuses_the_outermost_resource_the_request_got_wrong() {
    let router = router();
    fixture(&router).await;

    let mut rows: Vec<Row> = Vec::new();

    // A workspace this store holds no record of is the outermost resource the
    // request got wrong, whatever the rest of the path says: the pipeline the held
    // workspace does hold, a pipeline nothing created, and a version no write
    // produced all leave the workspace the one that is absent. The four unknown
    // names rotate through the rows, so the expected message is built from the
    // identifier the row sent.
    for (at, workspace) in UNKNOWN_WORKSPACES.iter().enumerate() {
        rows.push((
            format!("DescribePipeline, unknown workspace {workspace:?}, a pipeline that exists elsewhere"),
            "GET",
            pipeline_uri(workspace, PIPELINE),
            None,
            absent(WORKSPACE_KIND, workspace),
        ));
        rows.push((
            format!("DescribePipeline, unknown workspace {workspace:?} and an unknown pipeline and version"),
            "GET",
            versioned_uri(workspace, UNKNOWN_PIPELINE, UNKNOWN_VERSIONS[at % UNKNOWN_VERSIONS.len()]),
            None,
            absent(WORKSPACE_KIND, workspace),
        ));
        rows.push((
            format!("UpdatePipeline, unknown workspace {workspace:?}"),
            "PUT",
            pipeline_uri(workspace, PIPELINE),
            Some(json!({ "description": "a pipeline in a workspace that is not there" })),
            absent(WORKSPACE_KIND, workspace),
        ));
        rows.push((
            format!("DeletePipeline, unknown workspace {workspace:?}"),
            "DELETE",
            pipeline_uri(workspace, PIPELINE),
            None,
            absent(WORKSPACE_KIND, workspace),
        ));
        rows.push((
            format!("ListPipelines, unknown workspace {workspace:?}"),
            "GET",
            format!("/workspaces/{workspace}/pipelines"),
            None,
            absent(WORKSPACE_KIND, workspace),
        ));
    }

    // A workspace that WAS held and has been deleted is absent in exactly the same
    // way as one that never existed: the registry is the one place that says which
    // workspaces exist.
    for (case, method, body) in [
        ("DescribePipeline", "GET", None),
        (
            "UpdatePipeline",
            "PUT",
            Some(json!({ "description": "a deleted workspace" })),
        ),
        ("DeletePipeline", "DELETE", None),
    ] {
        rows.push((
            format!("{case}, a workspace deleted after it held the pipeline"),
            method,
            pipeline_uri(REMOVED, PIPELINE),
            body,
            absent(WORKSPACE_KIND, REMOVED),
        ));
    }
    rows.push((
        "ListPipelines, a workspace deleted after it held the pipeline".to_string(),
        "GET",
        format!("/workspaces/{REMOVED}/pipelines"),
        None,
        absent(WORKSPACE_KIND, REMOVED),
    ));

    // With the workspace resolved, the pipeline inside it is the outermost resource
    // left to get wrong — a name nothing created, and a name created only in
    // ANOTHER workspace — and it stays the one named even when the version is wrong
    // too.
    for (workspace, pipeline, label) in [
        (HELD, UNKNOWN_PIPELINE, "a pipeline nothing created"),
        (EMPTY, PIPELINE, "a workspace that holds no pipeline at all"),
    ] {
        rows.push((
            format!("DescribePipeline, {label}"),
            "GET",
            pipeline_uri(workspace, pipeline),
            None,
            absent(PIPELINE_KIND, pipeline),
        ));
        rows.push((
            format!("DescribePipeline at a version, {label}"),
            "GET",
            versioned_uri(workspace, pipeline, UNKNOWN_VERSIONS[1]),
            None,
            absent(PIPELINE_KIND, pipeline),
        ));
        rows.push((
            format!("UpdatePipeline, {label}"),
            "PUT",
            pipeline_uri(workspace, pipeline),
            Some(json!({ "description": "a pipeline that is not there" })),
            absent(PIPELINE_KIND, pipeline),
        ));
        rows.push((
            format!("DeletePipeline, {label}"),
            "DELETE",
            pipeline_uri(workspace, pipeline),
            None,
            absent(PIPELINE_KIND, pipeline),
        ));
    }

    assert_rows(&router, rows).await;
}

#[tokio::test]
async fn two_reads_of_the_same_unknown_workspace_state_the_same_absence() {
    let router = router();
    fixture(&router).await;

    // The pipeline read and the pipeline listing address the same workspace, so a
    // workspace neither resolves is reported identically by both: same status, same
    // code, same message. A caller cannot be told the workspace is missing by one
    // read and the pipeline by the other.
    for workspace in UNKNOWN_WORKSPACES {
        let (read_status, read) = call(&router, "GET", &pipeline_uri(workspace, PIPELINE), None).await;
        let (list_status, listed) = call(
            &router,
            "GET",
            &format!("/workspaces/{workspace}/pipelines"),
            None,
        )
        .await;
        assert_eq!(
            read_status, list_status,
            "case {workspace:?}: DescribePipeline answered {read_status} {read} \
             and ListPipelines {list_status} {listed}"
        );
        assert_eq!(
            error_code(&read),
            error_code(&listed),
            "case {workspace:?}: DescribePipeline answered {read} and ListPipelines {listed}"
        );
        assert_eq!(
            error_message(&read),
            error_message(&listed),
            "case {workspace:?}: DescribePipeline answered {read} and ListPipelines {listed}"
        );
        assert_eq!(
            error_message(&read),
            absent(WORKSPACE_KIND, workspace),
            "case {workspace:?}: expected the workspace to be the absence, got {read}"
        );
    }
}

#[tokio::test]
async fn a_version_is_the_absence_only_once_the_workspace_and_the_pipeline_resolve() {
    let router = router();
    fixture(&router).await;

    for version in UNKNOWN_VERSIONS {
        // The workspace and the pipeline both resolve, so the version is the
        // innermost — and now outermost wrong — thing the request named: the refusal
        // is about the pipeline's versions and names the version that was asked for,
        // not the workspace.
        let (status, body) = call(
            &router,
            "GET",
            &versioned_uri(HELD, PIPELINE, version),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case version {version:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case version {version:?}: expected ResourceNotFoundException, got {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains(PIPELINE) && message.contains(version),
            "case version {version:?}: the refusal must name the pipeline and the \
             version asked for, got {message:?}"
        );
        assert_ne!(
            message,
            absent(WORKSPACE_KIND, HELD),
            "case version {version:?}: a resolved workspace must not be reported \
             absent, got {message:?}"
        );

        // The SAME version asked for under a workspace this store does not hold is
        // the workspace's absence instead: the outer resource is resolved first.
        for workspace in UNKNOWN_WORKSPACES {
            let (status, outer) = call(
                &router,
                "GET",
                &versioned_uri(workspace, PIPELINE, version),
                None,
            )
            .await;
            assert_eq!(
                status, 404,
                "case {workspace:?} at version {version:?}: expected 404, got {status} {outer}"
            );
            assert_eq!(
                error_message(&outer),
                absent(WORKSPACE_KIND, workspace),
                "case {workspace:?} at version {version:?}: expected the workspace \
                 to be the absence, got {outer}"
            );
        }
    }
}

#[tokio::test]
async fn resolving_outside_in_still_serves_every_pipeline_path_the_store_holds() {
    let router = router();
    fixture(&router).await;

    // The resolution refuses nothing that resolves: the same routes as the refusals
    // above, with every resource on them present — the pipeline at its latest
    // version, at each version a write produced, the listing of its workspace, and
    // the listing of the workspace that holds no pipeline.
    let served: [(&str, String); 4] = [
        ("DescribePipeline, the latest version", pipeline_uri(HELD, PIPELINE)),
        (
            "DescribePipeline, the version the create wrote",
            versioned_uri(HELD, PIPELINE, "1"),
        ),
        (
            "DescribePipeline, the version the update wrote",
            versioned_uri(HELD, PIPELINE, "2"),
        ),
        ("ListPipelines, the held workspace", format!("/workspaces/{HELD}/pipelines")),
    ];
    for (case, uri) in served {
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
    }

    // An open workspace that holds no pipeline is an empty page, not an absence:
    // the workspace resolves.
    let (status, empty) = call(
        &router,
        "GET",
        &format!("/workspaces/{EMPTY}/pipelines"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "ListPipelines on an empty workspace: got {status} {empty}"
    );
    assert_eq!(
        empty["pipelineSummaries"].as_array().map(Vec::len),
        Some(0),
        "ListPipelines on an empty workspace must report no pipelines, got {empty}"
    );
}
