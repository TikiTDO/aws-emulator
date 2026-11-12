// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ORDER in which CodeBuild resolves the resources a request
//! addresses, driven over the wire the router serves (`POST /` with
//! `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1) through the harness in
//! `project_build_lifecycle`.
//!
//! RULES under test:
//!
//! 1. A request addressed by a per-project resource id (`<projectName>:<uuid>`) resolves
//!    the PROJECT that id names first: with that project absent the refusal reports the
//!    project, whether or not a record is held under the id. An id that carries no uuid
//!    half names no project, so such a request is answered on the record it addresses.
//! 2. An operation that addresses a project's webhook resolves the project before the
//!    webhook: an absent project is reported with CodeBuild's fixed absent-project
//!    sentence, and only a project that IS held can report its webhook as the absent
//!    resource.
//! 3. `DeleteBuildBatch` resolves the id it is given before anything is deleted: an id
//!    that denotes no batch build of this account is refused with the one error the
//!    operation models, and nothing it holds is removed.
//!
//! Every refusal is read back against the store: what a refused call must not change is
//! asserted through the implemented readers.

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::state;

use crate::project_build_lifecycle::{
    batch_enabled, call, create_project, create_project_body, emulator, error_type, expected_arn,
    listed_ids, PAST_THE_SETTLE_SPAN, REGION,
};

/// A well-formed uuid half for a resource id this service never minted: every id it
/// mints derives its uuid from the project, the sequence number and the start instant,
/// so a uuid built from `nth` alone is never one of them.
fn synthetic_uuid(nth: usize) -> String {
    format!("00000000-0000-8000-8000-{nth:012}")
}

/// Start one build of `project` and hand back its id.
async fn start_build(router: &smithy_server_runtime::MultiServiceRouter, project: &str) -> String {
    let (status, body) = call(router, "StartBuild", json!({"projectName": project})).await;
    assert_eq!(
        status, 200,
        "StartBuild {project:?}: expected 200, got {status} with {body}"
    );
    body["build"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("StartBuild {project:?} must report an id, got {body}"))
        .to_string()
}

/// Start one batch build of `project` and hand back its id.
async fn start_build_batch(
    router: &smithy_server_runtime::MultiServiceRouter,
    project: &str,
) -> String {
    let (status, body) = call(router, "StartBuildBatch", json!({"projectName": project})).await;
    assert_eq!(
        status, 200,
        "StartBuildBatch {project:?}: expected 200, got {status} with {body}"
    );
    body["buildBatch"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("StartBuildBatch {project:?} must report an id, got {body}"))
        .to_string()
}

/// Delete `project`, asserting it succeeded — the builds and batch builds it started
/// survive it, which is what makes the ordering rows below decidable.
async fn delete_project(router: &smithy_server_runtime::MultiServiceRouter, project: &str) {
    let (status, body) = call(router, "DeleteProject", json!({"name": project})).await;
    assert_eq!(
        status, 200,
        "DeleteProject {project:?}: expected 200, got {status} with {body}"
    );
}

/// How the project a build id names came to be absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Absence {
    /// No project of that name was ever created, so no build was ever started under it
    /// either: the id is well formed and denotes nothing at all.
    NeverCreated,
    /// The project was created, a build of it started, and the project then deleted: the
    /// BUILD record is still held under the id, so only the order the preconditions are
    /// resolved in decides which resource the refusal names.
    DeletedUnderItsBuild,
}

/// RULE 1: `RetryBuild` resolves the project named by the build id's project half
/// before the build record, so a retry whose project is absent is refused on the
/// PROJECT — named by its ARN — and no build is minted.
///
/// The table spans how the project came to be absent (never created, deleted while its
/// build stayed) and the project-name forms the model's `ProjectName` pattern allows
/// (the two-character minimum, a long name, and names carrying `-` and `_`), with a
/// control row proving a retry whose project IS held still succeeds.
#[tokio::test]
async fn a_retry_whose_project_is_absent_is_refused_on_the_project() {
    // (project name, how it is absent) — the names span the pattern
    // `^[A-Za-z0-9][A-Za-z0-9\-_]{1,149}$` on `CreateProjectInput$name`
    // (smithy_query(query="shape",shape="ProjectName")): the shortest allowed name, a
    // long one, and one of each permitted punctuation.
    let cases: &[(&str, Absence)] = &[
        ("ab", Absence::NeverCreated),
        ("retry-gone-project", Absence::NeverCreated),
        ("retry_gone_project", Absence::NeverCreated),
        (
            "r0123456789012345678901234567890123456789012345678901234567890123456789",
            Absence::NeverCreated,
        ),
        ("ab", Absence::DeletedUnderItsBuild),
        ("retry-deleted-project", Absence::DeletedUnderItsBuild),
        ("retry_deleted_project", Absence::DeletedUnderItsBuild),
    ];

    for (nth, (project, absence)) in cases.iter().enumerate() {
        let case = format!("{project:?} {absence:?}");
        // A fresh emulator per row: a row's state must not depend on the one before.
        let router = emulator();
        // A live project with a build of its own, so every row also proves the refusal
        // is about the project the id names and not about the account holding none.
        create_project(&router, "retry-live", json!({})).await;
        let live_build = start_build(&router, "retry-live").await;

        let (requested, held_builds) = match absence {
            Absence::NeverCreated => (
                format!("{project}:{}", synthetic_uuid(nth + 1)),
                vec![live_build.clone()],
            ),
            Absence::DeletedUnderItsBuild => {
                create_project(&router, project, json!({})).await;
                let orphan = start_build(&router, project).await;
                delete_project(&router, project).await;
                (orphan.clone(), vec![live_build.clone(), orphan])
            }
        };

        let (status, body) = call(&router, "RetryBuild", json!({"id": requested})).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_type(&body),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {body}"
        );
        // Built here from the region the call is signed for and the emulator's account,
        // so the assertion pins the ARN the message must carry rather than echoing
        // whatever the handler produced.
        assert_eq!(
            body["message"],
            json!(format!(
                "Project cannot be found: {}",
                expected_arn(&format!("project/{project}"))
            )),
            "case {case}: the refusal must name the absent PROJECT by its ARN, got {body}"
        );
        assert!(
            !body["message"]
                .as_str()
                .unwrap_or_default()
                .contains(":build/"),
            "case {case}: the refusal must not point the caller at a build, got {body}"
        );

        // Nothing was minted: the account still holds exactly the builds that were
        // started, read back through the store the handlers write.
        let (status, listed) = call(&router, "ListBuilds", json!({})).await;
        assert_eq!(
            status, 200,
            "case {case}: ListBuilds after the refusal: expected 200, got {status} with {listed}"
        );
        let mut reported = listed_ids(&listed);
        reported.sort();
        let mut expected = held_builds;
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case}: a refused retry must persist no build, got {listed}"
        );
    }
}

/// RULE 1, the boundary: a value that carries no uuid half names no project, so the
/// request is answered on the record it addresses — the BUILD — and not on a project
/// the id does not name.
///
/// The table spans the forms such a value takes: a bare project name, a project name
/// with a second half that is not a uuid, and the build's ARN form.
#[tokio::test]
async fn an_id_that_names_no_project_is_answered_on_the_build() {
    let router = emulator();
    create_project(&router, "retry-boundary", json!({})).await;
    let held = start_build(&router, "retry-boundary").await;

    let cases: &[(&str, String)] = &[
        ("a bare project name", "retry-boundary".to_string()),
        (
            "a second half that is not a uuid",
            "retry-boundary:not-a-uuid".to_string(),
        ),
        (
            "a second half one hex digit short of a uuid",
            "retry-boundary:00000000-0000-8000-8000-00000000000".to_string(),
        ),
        (
            "the build's ARN form",
            expected_arn(&format!("build/{held}")),
        ),
    ];

    for (case, requested) in cases {
        let (status, body) = call(&router, "RetryBuild", json!({"id": requested})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_type(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["message"],
            json!(format!(
                "Build cannot be found: {}",
                expected_arn(&format!("build/{requested}"))
            )),
            "case {case:?}: the refusal must name the build the id addresses, got {body}"
        );
    }

    // The one build that was started is untouched, and nothing was minted beside it.
    let (_, listed) = call(&router, "ListBuilds", json!({})).await;
    assert_eq!(
        listed_ids(&listed),
        vec![held],
        "a refused retry must persist no build, got {listed}"
    );
}

/// A `CreateProject` request for a GITHUB-sourced project — the source type a
/// repository-event webhook can be created for.
fn github_project_body(name: &str) -> Value {
    create_project_body(
        name,
        json!({"source": {"type": "GITHUB", "location": "https://github.com/o/r.git"}}),
    )
}

/// Hold a GitHub access token, which is what CodeBuild carries a webhook call out
/// against the repository provider with (its own refusal class), so every row below is
/// decided by the precondition it is written for.
async fn import_github_credentials(router: &smithy_server_runtime::MultiServiceRouter) {
    let (status, body) = call(
        router,
        "ImportSourceCredentials",
        json!({
            "serverType": "GITHUB",
            "authType": "PERSONAL_ACCESS_TOKEN",
            "token": "an-access-token",
            "shouldOverwrite": true,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "ImportSourceCredentials: expected 200, got {status} with {body}"
    );
}

/// How one webhook operation's request body is formed for a project name — the member
/// every webhook operation addresses its project by.
type WebhookRequest = fn(&str) -> Value;

/// RULE 2: every operation that addresses a project's webhook resolves the project
/// first, so an absent project is refused with CodeBuild's fixed absent-project
/// sentence — which names neither the project nor its ARN — while a held project that
/// carries no webhook is answered by what that operation does with one that is not
/// there: created for `CreateWebhook`, removed-already for `DeleteWebhook` (nothing to
/// remove, so the empty answer), and refused with the absent-webhook sentence only by
/// the update, which has no existing record to change.
///
/// The table spans all three webhook operations, each against both forms of an absent
/// project (never created, and created then deleted) and against a held project with no
/// webhook, so the two separately actionable conditions are proved distinguishable.
#[tokio::test]
async fn a_webhook_operation_reports_an_absent_project_before_an_absent_webhook() {
    // (operation, the request that addresses the project's webhook)
    let operations: [(&str, WebhookRequest); 3] = [
        ("CreateWebhook", |project| json!({"projectName": project})),
        ("UpdateWebhook", |project| {
            json!({"projectName": project, "branchFilter": "^refs/heads/main$"})
        }),
        ("DeleteWebhook", |project| json!({"projectName": project})),
    ];

    for (operation, request) in operations {
        for form in ["never created", "deleted"] {
            let router = emulator();
            import_github_credentials(&router).await;
            let project = format!(
                "hook-absent-{}-{}",
                operation.to_lowercase(),
                form.replace(' ', "-")
            );
            if form == "deleted" {
                let (status, body) =
                    call(&router, "CreateProject", github_project_body(&project)).await;
                assert_eq!(
                    status, 200,
                    "CreateProject {project:?}: expected 200, got {status} with {body}"
                );
                delete_project(&router, &project).await;
            }

            let case = format!("{operation} on a project {form}");
            let (status, body) = call(&router, operation, request(&project)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} with {body}"
            );
            assert_eq!(
                error_type(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {body}"
            );
            assert_eq!(
                body["message"],
                json!(state::PROJECT_NOT_FOUND_MESSAGE),
                "case {case:?}: expected CodeBuild's absent-project sentence, got {body}"
            );
            let stated = body["message"].as_str().unwrap_or_default();
            assert!(
                !stated.contains("arn:") && !stated.contains(&project),
                "case {case:?}: the sentence names neither the project nor its ARN, got {body}"
            );
        }

        // The same operation against a project that IS held: the webhook is then the
        // resource the answer is about, so the two conditions never collapse into one.
        let router = emulator();
        import_github_credentials(&router).await;
        let project = format!("hook-held-{}", operation.to_lowercase());
        let (status, body) = call(&router, "CreateProject", github_project_body(&project)).await;
        assert_eq!(
            status, 200,
            "CreateProject {project:?}: expected 200, got {status} with {body}"
        );

        let case = format!("{operation} on a held project carrying no webhook");
        let (status, body) = call(&router, operation, request(&project)).await;
        match operation {
            "CreateWebhook" => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a held project takes a webhook, got {status} with {body}"
                );
                assert!(
                    body["webhook"]["payloadUrl"].is_string(),
                    "case {case:?}: the created webhook is reported, got {body}"
                );
            }
            // A delete has nothing to remove, which is the state it would leave behind
            // anyway: the empty answer, and the project left where it was.
            "DeleteWebhook" => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: removing a webhook that is not there is a no-op, got \
                     {status} with {body}"
                );
                assert!(
                    body["webhook"].is_null(),
                    "case {case:?}: the empty output models no members, got {body}"
                );
            }
            _ => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected 400, got {status} with {body}"
                );
                assert_eq!(
                    body["message"],
                    json!(state::WEBHOOK_NOT_FOUND_MESSAGE),
                    "case {case:?}: a held project reports the WEBHOOK as the absent \
                     resource, got {body}"
                );
            }
        }
    }
}

/// Which value `DeleteBuildBatch` is called with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BatchId {
    /// A well-formed id of a project this account holds, denoting no batch build.
    UnknownOfHeldProject,
    /// A well-formed id whose project half names no project either.
    UnknownOfAbsentProject,
    /// The id of a finished batch build that this call's predecessor already deleted.
    AlreadyDeleted,
    /// A value carrying no uuid half at all — a bare project name.
    NoUuidHalf,
    /// The ARN form of a batch build id rather than the id.
    ArnForm,
    /// The id of a BUILD this account holds, which denotes no batch build.
    BuildOfTheProject,
}

/// RULE 3: `DeleteBuildBatch` resolves the id before it deletes anything, so an id that
/// denotes no batch build of this account is refused with the single error the operation
/// models — in CodeBuild's own fixed words — and the batch builds and builds the account
/// does hold are left exactly where they were.
///
/// The table spans the ways an id can denote no batch build: unknown under a held
/// project, unknown under an absent one, already deleted, carrying no uuid half, given
/// in ARN form, and naming a build rather than a batch.
#[tokio::test]
async fn deleting_a_batch_build_id_that_denotes_nothing_is_refused() {
    let cases: &[BatchId] = &[
        BatchId::UnknownOfHeldProject,
        BatchId::UnknownOfAbsentProject,
        BatchId::AlreadyDeleted,
        BatchId::NoUuidHalf,
        BatchId::ArnForm,
        BatchId::BuildOfTheProject,
    ];

    for (nth, target) in cases.iter().enumerate() {
        // A fresh emulator per row, holding one finished batch build and one build, so
        // every row proves the refusal removed neither.
        let router = emulator();
        create_project(&router, "del-batch-kept", batch_enabled()).await;
        let kept_batch = start_build_batch(&router, "del-batch-kept").await;
        let kept_build = start_build(&router, "del-batch-kept").await;
        tokio::time::sleep(PAST_THE_SETTLE_SPAN).await;

        let requested = match target {
            BatchId::UnknownOfHeldProject => {
                format!("del-batch-kept:{}", synthetic_uuid(nth + 1))
            }
            BatchId::UnknownOfAbsentProject => {
                format!("del-batch-never-created:{}", synthetic_uuid(nth + 1))
            }
            BatchId::AlreadyDeleted => {
                let doomed = start_build_batch(&router, "del-batch-kept").await;
                tokio::time::sleep(PAST_THE_SETTLE_SPAN).await;
                let (status, body) =
                    call(&router, "DeleteBuildBatch", json!({"id": doomed.clone()})).await;
                assert_eq!(
                    status, 200,
                    "case {target:?}: the first delete: expected 200, got {status} with {body}"
                );
                assert_eq!(
                    body["statusCode"],
                    json!(state::BATCH_DELETED_STATUS_CODE),
                    "case {target:?}: the first delete reports the batch-level statusCode, \
                     got {body}"
                );
                doomed
            }
            BatchId::NoUuidHalf => "del-batch-kept".to_string(),
            BatchId::ArnForm => {
                format!(
                    "arn:aws:codebuild:{REGION}:{}:build-batch/{kept_batch}",
                    engine::EMULATOR_ACCOUNT_ID
                )
            }
            BatchId::BuildOfTheProject => kept_build.clone(),
        };

        let (status, body) = call(&router, "DeleteBuildBatch", json!({"id": requested})).await;
        assert_eq!(
            status, 400,
            "case {target:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_type(&body),
            "InvalidInputException",
            "case {target:?}: expected InvalidInputException, got {body}"
        );
        assert_eq!(
            body["message"],
            json!(state::INVALID_BUILD_BATCH_ID_MESSAGE),
            "case {target:?}: expected CodeBuild's invalid-batch-id sentence, got {body}"
        );
        assert!(
            body["statusCode"].is_null() && body["buildsDeleted"].is_null(),
            "case {target:?}: a refusal reports no delete outcome at all, got {body}"
        );

        // Nothing the account held was removed: both reads are computed from the store.
        let (status, after) = call(
            &router,
            "BatchGetBuildBatches",
            json!({"ids": [kept_batch.clone()]}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {target:?}: BatchGetBuildBatches: expected 200, got {status} with {after}"
        );
        assert_eq!(
            after["buildBatches"][0]["id"],
            json!(kept_batch),
            "case {target:?}: the batch build the call did not name is still held, got {after}"
        );
        let (status, builds) = call(&router, "BatchGetBuilds", json!({"ids": [&kept_build]})).await;
        assert_eq!(
            status, 200,
            "case {target:?}: BatchGetBuilds: expected 200, got {status} with {builds}"
        );
        assert_eq!(
            builds["builds"][0]["id"],
            json!(kept_build),
            "case {target:?}: the build the account held is still there, got {builds}"
        );
    }
}
