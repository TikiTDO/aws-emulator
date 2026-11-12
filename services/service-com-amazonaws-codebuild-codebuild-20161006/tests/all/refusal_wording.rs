// SPDX-License-Identifier: CC0-1.0

//! Cover for the WORDING of CodeBuild's refusals, driven over the wire the router
//! serves: `POST /` with `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! An error message is part of the contract — clients, CLIs and SDK wrappers surface
//! and match on it — so for each refusal class below the service has ONE sentence,
//! stated exactly as CodeBuild states it: its fixed text verbatim, and as variable
//! parts only what CodeBuild itself interpolates (a source type taken from the stored
//! project). Nothing about the request that CodeBuild does not name may reach the
//! message: no echoed value, no member path, no ARN.
//!
//! Each table spans the class the rule is about rather than one recorded request: every
//! form of an absent buildspec, every capacity member, the model's whole `SourceType`
//! catalogue, several operations that take a sort order, and several unresolvable
//! sandbox identifiers.

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::ops::types::{
    SORT_ORDER_TYPE_VALUES, SOURCE_TYPE_VALUES,
};
use service_com_amazonaws_codebuild_codebuild_20161006::state;
use smithy_server_runtime::MultiServiceRouter;

use super::project_build_lifecycle::{
    call, create_project_body, emulator, error_type, source_of, REGION,
};

/// The `message` an error body carries.
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// Call `operation`, assert it was refused with HTTP 400 and `code`, and hand back the
/// message it stated.
async fn refusal(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: Value,
    code: &str,
) -> String {
    let (status, answer) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400 from {operation}, got {status} with {answer}"
    );
    assert_eq!(
        error_type(&answer),
        code,
        "case {case:?}: expected {code} from {operation}, got {answer}"
    );
    message(&answer)
}

/// Create a project named `name` whose source is `source`, asserting it succeeded.
async fn create_project_with_source(router: &MultiServiceRouter, name: &str, source: Value) {
    let (status, body) = call(
        router,
        "CreateProject",
        create_project_body(name, json!({"source": source})),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateProject {name:?}: expected 200, got {status} with {body}"
    );
}

/// RULE: a source that declares it has no input source code must carry its buildspec
/// inline, and CodeBuild refuses the combination with 400 `InvalidInputException` worded
/// as `Invalid input: a buildspec is required for source type <type>` — the whole
/// sentence fixed, including the `Invalid input: ` prefix it puts on this class of
/// request-shape error, with the source type taken from the source under check as its
/// only variable part. The rule holds wherever a project's source is set, so the rows
/// drive `CreateProject` and `UpdateProject` alike, over every form of "no inline
/// buildspec" the model names ("If this value is not provided or is set to an empty
/// string") plus one that declares no commands either.
#[tokio::test]
async fn the_buildspec_refusal_is_worded_as_codebuild_words_it() {
    let router = emulator();

    // The source type is the one `ProjectSource$type` value documented as "The project
    // does not have input source code" (`state::NO_SOURCE_TYPE`, one of the model's
    // `SOURCE_TYPE_VALUES`), which is also what CodeBuild interpolates.
    let expected = format!(
        "Invalid input: a buildspec is required for source type {}",
        state::NO_SOURCE_TYPE
    );

    // (case, the source the request sets)
    let sources: [(&str, Value); 3] = [
        ("buildspec omitted", json!({"type": state::NO_SOURCE_TYPE})),
        (
            "buildspec empty",
            json!({"type": state::NO_SOURCE_TYPE, "buildspec": ""}),
        ),
        (
            "buildspec all whitespace",
            json!({"type": state::NO_SOURCE_TYPE, "buildspec": "  \n\t "}),
        ),
    ];

    for (index, (case, source)) in sources.into_iter().enumerate() {
        let created = format!("spec-create-{index}");
        let stated = refusal(
            &router,
            &format!("CreateProject, {case}"),
            "CreateProject",
            create_project_body(&created, json!({"source": source.clone()})),
            "InvalidInputException",
        )
        .await;
        assert_eq!(
            stated, expected,
            "case {case:?} on CreateProject: expected CodeBuild's buildspec sentence, \
             got {stated:?}"
        );

        // The same rule on an UPDATE that empties the buildspec of a stored project.
        let updated = format!("spec-update-{index}");
        create_project_with_source(
            &router,
            &updated,
            json!({"type": state::NO_SOURCE_TYPE, "buildspec": "version: 0.2"}),
        )
        .await;
        let stated = refusal(
            &router,
            &format!("UpdateProject, {case}"),
            "UpdateProject",
            json!({"name": updated, "source": source}),
            "InvalidInputException",
        )
        .await;
        assert_eq!(
            stated, expected,
            "case {case:?} on UpdateProject: expected CodeBuild's buildspec sentence, \
             got {stated:?}"
        );
    }

    // The control: a source that carries a buildspec inline is accepted.
    create_project_with_source(
        &router,
        "spec-accepted",
        json!({"type": state::NO_SOURCE_TYPE, "buildspec": "version: 0.2"}),
    )
    .await;
}

/// RULE: a `FleetCapacity` below the floor of one machine is refused with 400
/// `InvalidInputException` worded `Capacity must be greater or equal to 1` — wholly
/// fixed text that names neither the member that carried the value nor the value
/// itself, so the sentence is the same for every capacity member and every request.
#[tokio::test]
async fn the_fleet_capacity_refusal_is_one_fixed_sentence() {
    let router = emulator();

    // (case, the capacity members merged into an otherwise valid request)
    let cases: [(&str, Value); 6] = [
        ("baseCapacity zero", json!({"baseCapacity": 0})),
        ("baseCapacity negative", json!({"baseCapacity": -7})),
        (
            "baseCapacity far below the floor",
            json!({"baseCapacity": -4096}),
        ),
        (
            "maxCapacity zero",
            json!({"baseCapacity": 2, "scalingConfiguration": {"maxCapacity": 0}}),
        ),
        (
            "maxCapacity negative",
            json!({"baseCapacity": 2, "scalingConfiguration": {"maxCapacity": -1}}),
        ),
        (
            "both capacity members below the floor",
            json!({"baseCapacity": 0, "scalingConfiguration": {"maxCapacity": -3}}),
        ),
    ];

    for (index, (case, members)) in cases.into_iter().enumerate() {
        let name = format!("wording-fleet-{index}");
        let mut body = json!({
            "name": name,
            "environmentType": "LINUX_CONTAINER",
            "computeType": "BUILD_GENERAL1_SMALL",
        });
        if let (Value::Object(target), Value::Object(extra)) = (&mut body, members) {
            target.extend(extra);
        }
        let stated = refusal(
            &router,
            case,
            "CreateFleet",
            body,
            "InvalidInputException",
        )
        .await;
        assert_eq!(
            stated, "Capacity must be greater or equal to 1",
            "case {case:?}: expected CodeBuild's capacity sentence, got {stated:?}"
        );
        for named in ["baseCapacity", "maxCapacity", &name] {
            assert!(
                !stated.contains(named),
                "case {case:?}: the sentence must not name {named:?}, got {stated:?}"
            );
        }
    }
}

/// RULE: a webhook is refused when the project's source type cannot take the build type
/// the request asks for — a repository-event webhook needs a provider that sends
/// repository events, and a runner webhook is "only available for NO_SOURCE source type
/// projects" — and CodeBuild words that refusal `Invalid build type provided for <type>
/// type webhook`: fixed text whose only variable part is the project's STORED source
/// type, with nothing of the request in it and no ARN.
///
/// The table walks `SourceType`'s whole modelled value list
/// (`ops::types::SOURCE_TYPE_VALUES`), so it covers the source types the issue never
/// cited as well as the one it did.
#[tokio::test]
async fn the_webhook_build_type_refusal_is_worded_as_codebuild_words_it() {
    let router = emulator();

    // Acting on the provider's half of a webhook needs an access token for it, which is
    // its own refusal class (see `tests/all/webhook_access_token.rs`): hold one for every
    // `ServerType` so each row below is decided by the build-type pairing under test.
    for server_type in state::WEBHOOK_SOURCE_TYPES {
        let (status, body) = call(
            &router,
            "ImportSourceCredentials",
            json!({
                "serverType": server_type,
                "authType": "PERSONAL_ACCESS_TOKEN",
                "token": "an-access-token",
                "shouldOverwrite": true,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "ImportSourceCredentials ({server_type}): expected 200, got {status} with {body}"
        );
    }

    for source_type in SOURCE_TYPE_VALUES {
        let takes_repository_events = state::WEBHOOK_SOURCE_TYPES.contains(source_type);
        let expected = format!("Invalid build type provided for {source_type} type webhook");

        // A repository-event webhook (the request names no build type, so it is the
        // default one) against this source type.
        let name = format!("hook-default-{}", source_type.to_lowercase());
        create_project_with_source(&router, &name, source_of(source_type)).await;
        let case = format!("default build type on {source_type}");
        if takes_repository_events {
            let (status, body) = call(&router, "CreateWebhook", json!({"projectName": name})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a source that sends repository events takes a webhook, \
                 got {status} with {body}"
            );
        } else {
            let stated = refusal(
                &router,
                &case,
                "CreateWebhook",
                json!({"projectName": name}),
                "InvalidInputException",
            )
            .await;
            assert_eq!(
                stated, expected,
                "case {case:?}: expected CodeBuild's build-type sentence, got {stated:?}"
            );
        }

        // A RUNNER webhook against this source type: the mirror of the same pairing, so
        // it answers with the same sentence for every source type but the one it is
        // available for.
        let name = format!("hook-runner-{}", source_type.to_lowercase());
        create_project_with_source(&router, &name, source_of(source_type)).await;
        let case = format!("{} on {source_type}", state::RUNNER_WEBHOOK_BUILD_TYPE);
        let request = json!({
            "projectName": name,
            "buildType": state::RUNNER_WEBHOOK_BUILD_TYPE,
        });
        if *source_type == state::RUNNER_WEBHOOK_SOURCE_TYPE {
            let (status, body) = call(&router, "CreateWebhook", request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a runner webhook's own source type takes it, got {status} \
                 with {body}"
            );
        } else {
            let stated = refusal(
                &router,
                &case,
                "CreateWebhook",
                request,
                "InvalidInputException",
            )
            .await;
            assert_eq!(
                stated, expected,
                "case {case:?}: expected CodeBuild's build-type sentence, got {stated:?}"
            );
            assert!(
                !stated.contains("arn:") && !stated.contains(&name),
                "case {case:?}: the sentence must name neither the project nor its ARN, \
                 got {stated:?}"
            );
        }

        // The rule holds wherever a webhook's build type is SET, so the same pairing is
        // refused with the same sentence on an UPDATE of a webhook the project already
        // has: the build type each source type cannot take is the one the create above
        // accepted for the other side of the pairing.
        if !takes_repository_events && *source_type != state::RUNNER_WEBHOOK_SOURCE_TYPE {
            // A source that takes no webhook at all has none to update.
            continue;
        }
        let (held, update_to) = if takes_repository_events {
            (
                format!("hook-default-{}", source_type.to_lowercase()),
                state::RUNNER_WEBHOOK_BUILD_TYPE,
            )
        } else {
            (
                format!("hook-runner-{}", source_type.to_lowercase()),
                "BUILD",
            )
        };
        let case = format!("UpdateWebhook to {update_to} on {source_type}");
        let stated = refusal(
            &router,
            &case,
            "UpdateWebhook",
            json!({"projectName": held, "buildType": update_to}),
            "InvalidInputException",
        )
        .await;
        assert_eq!(
            stated, expected,
            "case {case:?}: expected CodeBuild's build-type sentence, got {stated:?}"
        );
    }
}

/// RULE: a sort order outside the modelled `SortOrderType` value set is refused with 400
/// `InvalidInputException` worded `Invalid sort order specified` — wholly fixed text
/// that quotes neither the rejected value, the member path nor the permitted set — on
/// every operation whose input takes a sort order. Every OTHER constraint violation
/// still states the member and the constraint it failed.
#[tokio::test]
async fn an_unmodelled_sort_order_is_refused_in_codebuilds_own_words() {
    let router = emulator();
    create_project_with_source(
        &router,
        "sorted-proj",
        json!({"type": state::NO_SOURCE_TYPE, "buildspec": "version: 0.2"}),
    )
    .await;
    let report_arn = format!(
        "arn:aws:codebuild:{REGION}:{}:report/sorted-report:1",
        engine::EMULATOR_ACCOUNT_ID
    );

    // Operations whose input models a `sortOrder`, with the other members each one
    // requires. The last one is still a scaffolded handler, which is the point: the
    // refusal belongs to the sort order, not to what the operation goes on to do.
    let operations: [(&str, Value); 6] = [
        ("ListBuildsForProject", json!({"projectName": "sorted-proj"})),
        ("ListBuilds", json!({})),
        ("ListProjects", json!({})),
        ("ListSandboxes", json!({})),
        ("ListReportGroups", json!({})),
        ("DescribeCodeCoverages", json!({"reportArn": report_arn})),
    ];

    // Values outside the model's value set: a token of the right shape, the right value
    // in the wrong case, the empty string, and one with trailing space.
    let rejected = ["NOT_A_VALID_ORDER", "ascending", "", "ASCENDING "];

    for (operation, base) in &operations {
        for value in rejected {
            let mut body = base.clone();
            if let Value::Object(members) = &mut body {
                members.insert("sortOrder".to_string(), json!(value));
            }
            let case = format!("{operation} with sortOrder {value:?}");
            let stated = refusal(
                &router,
                &case,
                operation,
                body,
                "InvalidInputException",
            )
            .await;
            assert_eq!(
                stated, "Invalid sort order specified",
                "case {case:?}: expected CodeBuild's sort-order sentence, got {stated:?}"
            );
        }

        // Every value the model DOES declare is accepted by the same operation — the
        // refusal is about the value set, not about the member being present.
        for value in SORT_ORDER_TYPE_VALUES {
            let mut body = base.clone();
            if let Value::Object(members) = &mut body {
                members.insert("sortOrder".to_string(), json!(value));
            }
            let (status, answer) = call(&router, operation, body).await;
            assert_ne!(
                status, 400,
                "{operation} with sortOrder {value:?}: a modelled value must not be \
                 refused, got {answer}"
            );
        }
    }

    // A violation of ANY other member still states the member and the constraint: the
    // sort-order sentence replaces the constraint envelope for that one member only.
    let stated = refusal(
        &router,
        "a maxResults past its modelled range",
        "DescribeCodeCoverages",
        json!({"reportArn": report_arn, "maxResults": 101}),
        "InvalidInputException",
    )
    .await;
    assert_eq!(
        stated,
        "1 validation error detected: Value '101' at 'maxResults' failed to satisfy \
         constraint: Member must have value less than or equal to 100",
        "a non-sort-order violation must still state its member, got {stated:?}"
    );
}

/// RULE: a request that has to resolve an EXISTING webhook of a project carrying none is
/// refused with 400 `ResourceNotFoundException` worded `Webhook does not exist for this
/// project` — wholly fixed text that names neither the project nor its ARN, so the
/// sentence is the same for every way a project came to have no webhook. Removing one is
/// not such a request: a webhook that is not there is already removed, so that call is
/// served instead of refused.
///
/// The table spans every source type a project can carry a webhook on — the providers
/// that send repository events (`state::WEBHOOK_SOURCE_TYPES`) plus the runner webhook's
/// own source type (`state::RUNNER_WEBHOOK_SOURCE_TYPE`) — each in both forms of absence
/// (never connected, and connected then disconnected).
#[tokio::test]
async fn an_absent_webhook_is_refused_in_codebuilds_own_words() {
    let router = emulator();

    // Acting on the provider's half of a webhook needs an access token for it, which is
    // its own refusal class (see `tests/all/webhook_access_token.rs`): hold one for every
    // `ServerType` so every row below is decided by the absent webhook under test.
    for server_type in state::WEBHOOK_SOURCE_TYPES {
        let (status, body) = call(
            &router,
            "ImportSourceCredentials",
            json!({
                "serverType": server_type,
                "authType": "PERSONAL_ACCESS_TOKEN",
                "token": "an-access-token",
                "shouldOverwrite": true,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "ImportSourceCredentials ({server_type}): expected 200, got {status} with {body}"
        );
    }

    let mut source_types: Vec<&str> = state::WEBHOOK_SOURCE_TYPES.to_vec();
    source_types.push(state::RUNNER_WEBHOOK_SOURCE_TYPE);

    for source_type in source_types {
        // The only webhook a runner project takes is a runner one; every other source
        // type takes the default, repository-event webhook.
        let build_type = (source_type == state::RUNNER_WEBHOOK_SOURCE_TYPE)
            .then_some(state::RUNNER_WEBHOOK_BUILD_TYPE);

        for (form, connect_first) in [("never connected", false), ("disconnected", true)] {
            let project = format!("hook-gone-{}-{form}", source_type.to_lowercase()).replace(' ', "-");
            create_project_with_source(&router, &project, source_of(source_type)).await;

            if connect_first {
                let mut request = json!({"projectName": project});
                if let (Value::Object(members), Some(build_type)) = (&mut request, build_type) {
                    members.insert("buildType".to_string(), json!(build_type));
                }
                let (status, body) = call(&router, "CreateWebhook", request).await;
                assert_eq!(
                    status, 200,
                    "CreateWebhook on a {source_type} project: expected 200, got {status} \
                     with {body}"
                );
                let (status, body) =
                    call(&router, "DeleteWebhook", json!({"projectName": project})).await;
                assert_eq!(
                    status, 200,
                    "DeleteWebhook on a {source_type} project: expected 200, got {status} \
                     with {body}"
                );
            }

            // The operation that CHANGES an existing webhook is the one with nothing to
            // act on; the one that REMOVES it is served (see
            // `tests/all/webhook_delete_is_idempotent.rs`), so only the update is
            // asserted on here.
            let case = format!("UpdateWebhook on a {source_type} project, {form}");
            let stated = refusal(
                &router,
                &case,
                "UpdateWebhook",
                json!({"projectName": project, "rotateSecret": true}),
                "ResourceNotFoundException",
            )
            .await;
            assert_eq!(
                stated, state::WEBHOOK_NOT_FOUND_MESSAGE,
                "case {case:?}: expected CodeBuild's absent-webhook sentence, got {stated:?}"
            );
            assert!(
                !stated.contains("arn:") && !stated.contains(&project),
                "case {case:?}: the sentence must name neither the project nor its ARN, \
                 got {stated:?}"
            );
        }
    }
}

/// RULE: a `sandboxId` that resolves to no sandbox of this account is refused with 400
/// `ResourceNotFoundException` worded `Sandbox does not exist` — wholly fixed text that
/// names no identifier, so no ARN for a resource that does not exist is disclosed and
/// the sentence is the same on every operation that ACTS ON a sandbox.
/// (`StartSandboxConnection`, which opens a session into one, classifies an id it cannot
/// resolve as invalid input instead — see `tests/all/sandbox_connection.rs`. A value that
/// is no sandbox identifier at all is refused ahead of the lookup by every operation whose
/// member takes "a sandboxId or sandboxArn" — see
/// `tests/all/identifier_shape_refusals.rs`.)
#[tokio::test]
async fn an_absent_sandbox_is_refused_in_codebuilds_own_words() {
    let router = emulator();

    // Identifiers of the documented `<project-name>:<uuid>` shape that name nothing, the
    // ARN form of one of them, and one whose second half is not a uuid.
    let absent = [
        "sandbox-proj:3f3ec2b3-57b1-4a16-9a6a-4a4d2b9c9a11".to_string(),
        "another-proj:00000000-0000-8000-8000-000000000000".to_string(),
        state::sandbox_arn(REGION, "sandbox-proj:1"),
        "sandbox-proj:0001".to_string(),
    ];

    for identifier in absent {
        // (operation, the request that addresses the sandbox)
        let requests: [(&str, Value); 2] = [
            (
                "StartCommandExecution",
                json!({"sandboxId": identifier, "command": "echo hi"}),
            ),
            ("StopSandbox", json!({"id": identifier})),
        ];
        for (operation, body) in requests {
            let case = format!("{operation} for {identifier:?}");
            let stated = refusal(
                &router,
                &case,
                operation,
                body,
                "ResourceNotFoundException",
            )
            .await;
            assert_eq!(
                stated, "Sandbox does not exist",
                "case {case:?}: expected CodeBuild's sandbox sentence, got {stated:?}"
            );
            assert!(
                !stated.contains("arn:") && !stated.contains(&identifier),
                "case {case:?}: the sentence must name no identifier, got {stated:?}"
            );
        }
    }
}
