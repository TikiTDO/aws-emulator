// SPDX-License-Identifier: CC0-1.0

//! `ListActionExecutions`, over the wire the service actually speaks (`awsJson1_1`,
//! `POST /` with `X-Amz-Target: CodePipeline_20150709.ListActionExecutions`).
//!
//! The rule under test: an action execution exists exactly for an action a pipeline
//! execution has STARTED, so a pipeline reports one `InProgress` action execution per
//! action each of its executions begins with — the actions of its first stage at that
//! stage's lowest `runOrder` — and the listing reports those from the store, newest
//! execution first, paging over them with an opaque cursor. Nothing of a later
//! `runOrder` or stage has started yet, so no record for one is reported.
//!
//! Creating a pipeline RUNS it once, so every pipeline here carries its creation run's
//! records too; a case asserting about the execution it started names that execution
//! through the filter (`only`).
//!
//! Every action here is a `Custom`-owned source action, because such an action's work is
//! performed by a job worker that reports its outcome through the API rather than by the
//! clock: the records this suite asserts about therefore stay exactly as the start left
//! them, which is what lets it assert "started, no outcome yet" without racing the
//! engine. The worker lifecycle itself is the sibling suite
//! (`tests/all/custom_action_jobs.rs`).
//!
//! The harness comes from the sibling suite (`tests/all/pipeline_lifecycle.rs`), so
//! every assertion here goes through the same router the production emulator builds.

use serde_json::{json, Value};

use crate::pipeline_lifecycle::{creation_run, Emulator, ROLE_ARN};
use service_com_amazonaws_codepipeline_codepipeline_20150709::state::{
    encode_page_token, CURSOR_ACTION_EXECUTION, CURSOR_EXECUTION, CURSOR_PIPELINE,
};

/// A well-formed `PipelineExecutionId` (its modelled pattern is
/// `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`) that no start
/// ever mints, so it passes the boundary validator and is then looked up and missed.
const UNSTORED_EXECUTION_ID: &str = "00000000-0000-4000-8000-000000000000";

/// One `ActionDeclaration`, with the `runOrder` it declares (`None` for a declaration
/// that names none).
///
/// A `Custom`-owned source action: its work is performed by a job worker of this
/// account, so an execution that reaches it waits for that worker (`PollForJobs` ->
/// `AcknowledgeJob` -> `PutJobSuccessResult`) instead of being carried on by the clock.
/// That is what lets this suite assert "started, no outcome yet" without racing the
/// engine; the worker lifecycle itself is `tests/all/custom_action_jobs.rs`.
fn action(name: &str, run_order: Option<i64>) -> Value {
    let mut action = json!({
        "name": name,
        "actionTypeId": {
            "category": "Source",
            "owner": "Custom",
            "provider": "MySource",
            "version": "1",
        },
        "configuration": {"S3Bucket": "my-bucket", "S3ObjectKey": "app.zip"},
    });
    if let Some(run_order) = run_order {
        action["runOrder"] = json!(run_order);
    }
    action
}

/// One `StageDeclaration` carrying `actions`.
fn stage(name: &str, actions: Vec<Value>) -> Value {
    json!({"name": name, "actions": actions})
}

/// One `ActionDeclaration` for a stage that is NOT the pipeline's first: a `Source`
/// action can only appear in the first stage (`CreatePipeline` refuses one anywhere
/// else, which `validation_and_messages.rs` pins), so a later stage's action is of
/// another category.
fn later_action(name: &str) -> Value {
    json!({
        "name": name,
        "actionTypeId": {
            "category": "Build",
            "owner": "AWS",
            "provider": "CodeBuild",
            "version": "1",
        },
        "configuration": {"ProjectName": "my-project"},
    })
}

/// A `PipelineDeclaration` named `name` with `stages` — plus the acting stage a
/// declaration has to carry when `stages` declares no action outside the `Source`
/// category (`state::validate_declaration` refuses a pipeline that would do nothing
/// with the revision it fetched, and one carrying fewer than two stages).
///
/// Appending it changes nothing a case here asserts: every source action is
/// `Custom`-owned, so each execution waits at the source stage and no action of a later
/// stage has started.
fn pipeline(name: &str, stages: Vec<Value>) -> Value {
    let mut stages = stages;
    let acts = stages.iter().any(|stage| {
        stage["actions"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|action| action["actionTypeId"]["category"] != json!("Source"))
    });
    if !acts {
        stages.push(stage("Build", vec![later_action("Compile")]));
    }
    json!({
        "name": name,
        "roleArn": ROLE_ARN,
        "artifactStore": {"type": "S3", "location": "my-bucket"},
        "stages": stages,
    })
}

/// Create `declaration` and return the pipeline name it carries.
async fn create(emulator: &Emulator, declaration: &Value) -> String {
    emulator
        .ok("CreatePipeline", &json!({"pipeline": declaration}))
        .await;
    declaration["name"]
        .as_str()
        .expect("the declaration names the pipeline")
        .to_string()
}

/// Start an execution of `name` and return the id the start minted.
async fn start(emulator: &Emulator, name: &str) -> String {
    emulator
        .ok("StartPipelineExecution", &json!({"name": name}))
        .await["pipelineExecutionId"]
        .as_str()
        .expect("a started execution carries its id")
        .to_string()
}

/// A listing request for the action executions of ONE execution of `name`.
///
/// A pipeline always carries its creation run's action executions beside the ones a
/// case starts, so a case asserting about the execution it started names it through the
/// filter the model declares for exactly that.
fn only(name: &str, execution_id: &str) -> Value {
    json!({"pipelineName": name, "filter": {"pipelineExecutionId": execution_id}})
}

/// The `actionExecutionDetails` of a listing response.
fn details(page: &Value) -> Vec<Value> {
    page["actionExecutionDetails"]
        .as_array()
        .unwrap_or_else(|| panic!("a listing reports actionExecutionDetails, got {page}"))
        .clone()
}

/// The `(stageName, actionName)` of each reported record, in the order reported.
fn reported_actions(page: &Value) -> Vec<(String, String)> {
    details(page)
        .iter()
        .map(|detail| {
            (
                detail["stageName"].as_str().unwrap_or_default().to_string(),
                detail["actionName"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Every page of the listing for `request`, walked with `maxResults` per page: the
/// records in order, and how many pages it took.
async fn walk(emulator: &Emulator, request: &Value, max_results: i64) -> (Vec<Value>, usize) {
    let mut collected = Vec::new();
    let mut pages = 0;
    let mut next_token: Option<String> = None;
    loop {
        let mut page_request = request.clone();
        page_request["maxResults"] = json!(max_results);
        if let Some(token) = &next_token {
            page_request["nextToken"] = json!(token);
        }
        let page = emulator.ok("ListActionExecutions", &page_request).await;
        pages += 1;
        collected.extend(details(&page));
        match page["nextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
        assert!(
            pages < 50,
            "the listing issued a cursor for more than 50 pages of at most 50 records"
        );
    }
    (collected, pages)
}

/// Starting an execution records one action execution per action the executed pipeline
/// version begins with, and the listing reports exactly those.
///
/// The rows differ along what the rule generalises over: how many actions a stage
/// declares, which `runOrder`s they declare (including none at all, both published
/// bounds of `ActionRunOrder`'s `@range(min: 1, max: 999)`, and a lowest order that is
/// not 1), and how many stages the pipeline has.
#[tokio::test]
async fn an_execution_records_the_actions_it_starts() {
    /// One row: the case, the stages the pipeline declares, and the
    /// `(stage, action)` pairs the execution starts.
    type Case = (&'static str, Vec<Value>, Vec<(&'static str, &'static str)>);

    let cases: Vec<Case> = vec![
        (
            "one stage, one action",
            vec![stage("Source", vec![action("Src", None)])],
            vec![("Source", "Src")],
        ),
        (
            "three actions declaring no runOrder start together",
            vec![stage(
                "Source",
                vec![action("A", None), action("B", None), action("C", None)],
            )],
            vec![("Source", "A"), ("Source", "B"), ("Source", "C")],
        ),
        (
            "a higher runOrder has not started yet",
            vec![stage(
                "Source",
                vec![
                    action("First", Some(1)),
                    action("Also", Some(1)),
                    action("Later", Some(2)),
                ],
            )],
            vec![("Source", "First"), ("Source", "Also")],
        ),
        (
            "an action declaring no runOrder runs in order 1",
            vec![stage(
                "Source",
                vec![
                    action("Default", None),
                    action("Explicit", Some(1)),
                    action("Second", Some(2)),
                ],
            )],
            vec![("Source", "Default"), ("Source", "Explicit")],
        ),
        (
            "the lowest declared runOrder need not be 1",
            vec![stage(
                "Source",
                vec![action("Seventh", Some(7)), action("Ninth", Some(9))],
            )],
            vec![("Source", "Seventh")],
        ),
        (
            "ActionRunOrder's published bounds: 1 starts, 999 waits",
            vec![stage(
                "Source",
                vec![action("Min", Some(1)), action("Max", Some(999))],
            )],
            vec![("Source", "Min")],
        ),
        (
            "only the first stage has started",
            vec![
                stage("Source", vec![action("Src", None)]),
                stage("Build", vec![later_action("Compile")]),
                stage("Deploy", vec![later_action("Ship")]),
            ],
            vec![("Source", "Src")],
        ),
    ];

    for (case, stages, expected) in cases {
        let emulator = Emulator::new();
        let name = create(&emulator, &pipeline("demo", stages)).await;
        let execution_id = start(&emulator, &name).await;

        let page = emulator
            .ok("ListActionExecutions", &only(&name, &execution_id))
            .await;
        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(stage, action)| (stage.to_string(), action.to_string()))
            .collect();
        assert_eq!(
            reported_actions(&page),
            expected,
            "case {case:?}: unexpected action executions, got {page}"
        );
        assert!(
            page["nextToken"].is_null(),
            "case {case:?}: a page that was not truncated issues no cursor, got {page}"
        );

        let mut ids = Vec::new();
        for detail in details(&page) {
            assert_eq!(
                detail["pipelineExecutionId"].as_str(),
                Some(execution_id.as_str()),
                "case {case:?}: a record must name the execution that started it, got {detail}"
            );
            assert_eq!(
                detail["status"].as_str(),
                Some("InProgress"),
                "case {case:?}: a started action has no outcome yet, got {detail}"
            );
            assert_eq!(
                detail["pipelineVersion"].as_i64(),
                Some(1),
                "case {case:?}: the record names the version it ran, got {detail}"
            );
            assert!(
                detail["startTime"].is_number() && detail["lastUpdateTime"].is_number(),
                "case {case:?}: a record is timed, got {detail}"
            );
            assert!(
                detail["output"].is_null(),
                "case {case:?}: nothing ran the action, so it reports no output, got {detail}"
            );
            let id = detail["actionExecutionId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a record carries its id, got {detail}"))
                .to_string();
            assert!(
                !ids.contains(&id),
                "case {case:?}: two action executions share the id {id:?}"
            );
            ids.push(id);
        }
    }
}

/// An action execution exists only for an action one of the pipeline's executions has
/// started: a pipeline carries the records of its own creation run and of nothing else,
/// and a pipeline's records go with it when it is deleted, so a pipeline recreated under
/// the same name reports only its new creation run's.
#[tokio::test]
async fn a_pipeline_reports_the_records_of_its_own_executions_only() {
    let emulator = Emulator::new();
    let declaration = pipeline("demo", vec![stage("Source", vec![action("Src", None)])]);
    let name = create(&emulator, &declaration).await;

    let created = creation_run(&emulator, &name).await;
    let page = emulator
        .ok("ListActionExecutions", &json!({"pipelineName": name}))
        .await;
    assert_eq!(
        reported_actions(&page),
        vec![("Source".to_string(), "Src".to_string())],
        "the run the creation started reached the action, got {page}"
    );
    assert!(
        details(&page)
            .iter()
            .all(|detail| detail["pipelineExecutionId"] == json!(created)),
        "only the creation run has started an action, got {page}"
    );
    assert!(
        page["nextToken"].is_null(),
        "a page that was not truncated issues no cursor, got {page}"
    );

    let started = start(&emulator, &name).await;
    emulator.ok("DeletePipeline", &json!({"name": name})).await;
    create(&emulator, &declaration).await;
    let page = emulator
        .ok("ListActionExecutions", &json!({"pipelineName": name}))
        .await;
    let surviving: Vec<&str> = details(&page)
        .iter()
        .filter_map(|detail| {
            detail["pipelineExecutionId"]
                .as_str()
                .filter(|id| *id == created.as_str() || *id == started.as_str())
                .map(str::to_string)
        })
        .map(|_| "survivor")
        .collect();
    assert!(
        surviving.is_empty(),
        "a deleted pipeline's action executions must not survive it, got {page}"
    );
}

/// The listing reports the most recently started action executions first and pages
/// over them with a cursor: whatever the page size, walking every page reports each
/// record exactly once, in the same order the unpaginated listing reports them.
///
/// The rows are the page sizes, from one record per page up to more than the listing
/// holds — including both published bounds of `MaxResults`' `@range(min: 1, max: 100)`.
#[tokio::test]
async fn the_listing_pages_newest_first_over_every_record() {
    let emulator = Emulator::new();
    let name = create(
        &emulator,
        &pipeline(
            "demo",
            vec![stage(
                "Source",
                vec![action("A", None), action("B", None), action("C", None)],
            )],
        ),
    )
    .await;
    let created = creation_run(&emulator, &name).await;
    let older = start(&emulator, &name).await;
    let newer = start(&emulator, &name).await;

    let request = json!({"pipelineName": name});
    let whole = details(&emulator.ok("ListActionExecutions", &request).await);
    let order: Vec<&str> = whole
        .iter()
        .map(|detail| detail["pipelineExecutionId"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        order,
        vec![
            newer.as_str(),
            newer.as_str(),
            newer.as_str(),
            older.as_str(),
            older.as_str(),
            older.as_str(),
            created.as_str(),
            created.as_str(),
            created.as_str()
        ],
        "the most recently started execution's actions come first, got {whole:?}"
    );

    // (case, page size, pages the walk must take for the nine records held)
    let cases: [(&str, i64, usize); 5] = [
        ("MaxResults' published minimum", 1, 9),
        ("three per page", 3, 3),
        ("a size that does not divide the total", 4, 3),
        ("exactly the total", 9, 1),
        ("MaxResults' published maximum", 100, 1),
    ];
    for (case, max_results, expected_pages) in cases {
        let (walked, pages) = walk(&emulator, &request, max_results).await;
        assert_eq!(
            walked, whole,
            "case {case:?}: paging must report the same records in the same order"
        );
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: unexpected page count for a page size of {max_results}"
        );
    }
}

/// `MaxResults` is modelled `@range(min: 1, max: 100)`: both published bounds are
/// served, and one past either bound is refused at the boundary rather than clamped.
#[tokio::test]
async fn max_results_is_accepted_inside_its_modelled_range_and_refused_outside() {
    let emulator = Emulator::new();
    let name = create(
        &emulator,
        &pipeline(
            "demo",
            vec![stage("Source", vec![action("A", None), action("B", None)])],
        ),
    )
    .await;
    start(&emulator, &name).await;

    // (case, maxResults, expected status, expected awsJson error code — "" for success,
    //  records on the page). The pipeline holds four records: the two actions its
    //  creation run started, and the two the start below did.
    let cases: [(&str, i64, u16, &str, usize); 4] = [
        ("the minimum MaxResults allows", 1, 200, "", 1),
        ("the maximum MaxResults allows", 100, 200, "", 4),
        ("one below the minimum", 0, 400, "ValidationException", 0),
        ("one above the maximum", 101, 400, "ValidationException", 0),
    ];
    for (case, max_results, expected_status, expected_code, expected_records) in cases {
        let (status, response) = emulator
            .call(
                "ListActionExecutions",
                &json!({"pipelineName": name, "maxResults": max_results}),
            )
            .await;
        let code = response
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            (status, code),
            (expected_status, expected_code),
            "case {case:?}: unexpected answer {response}"
        );
        if expected_status == 200 {
            assert_eq!(
                details(&response).len(),
                expected_records,
                "case {case:?}: unexpected page size, got {response}"
            );
        }
    }
}

/// A `nextToken` this listing did not issue names no position in its order, so it is
/// refused with the error the model declares for it rather than served as a first page.
#[tokio::test]
async fn a_next_token_this_listing_never_issued_is_refused() {
    let emulator = Emulator::new();
    let name = create(
        &emulator,
        &pipeline("demo", vec![stage("Source", vec![action("Src", None)])]),
    )
    .await;
    let execution_id = start(&emulator, &name).await;

    // Inside `NextToken`'s modelled length (1..2048), so the boundary validator passes
    // each of these to the handler: arbitrary text, a cursor of another kind this
    // service's listings issue, a cursor of THIS kind naming something the order does
    // not hold, and both published length bounds.
    let accepted_by_the_validator: Vec<(&str, String)> = vec![
        ("not a cursor at all", "not-a-token".to_string()),
        (
            "a ListPipelines cursor naming this pipeline",
            encode_page_token(CURSOR_PIPELINE, &name),
        ),
        (
            "a ListPipelineExecutions cursor naming this execution",
            encode_page_token(CURSOR_EXECUTION, &execution_id),
        ),
        (
            "an action-execution cursor naming no stored record",
            encode_page_token(CURSOR_ACTION_EXECUTION, UNSTORED_EXECUTION_ID),
        ),
        ("NextToken's minimum length", "a".to_string()),
        ("NextToken's maximum length", "a".repeat(2048)),
    ];
    for (case, token) in accepted_by_the_validator {
        let (status, code) = emulator
            .expect_error(
                "ListActionExecutions",
                &json!({"pipelineName": name, "nextToken": token}),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidNextTokenException"),
            "case {case:?}: an unissued nextToken must be refused as the model declares"
        );
    }

    // One past either published length bound is refused at the boundary instead.
    for (case, token) in [
        ("one below NextToken's minimum length", String::new()),
        ("one above NextToken's maximum length", "a".repeat(2049)),
    ] {
        let (status, code) = emulator
            .expect_error(
                "ListActionExecutions",
                &json!({"pipelineName": name, "nextToken": token}),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a token outside the modelled length must be refused at the boundary"
        );
    }
}

/// A pipeline the store does not hold has no action executions to list: the listing
/// reports the modelled not-found rather than an empty page.
#[tokio::test]
async fn an_unknown_pipeline_is_not_found() {
    let emulator = Emulator::new();
    let name = create(
        &emulator,
        &pipeline("demo", vec![stage("Source", vec![action("Src", None)])]),
    )
    .await;
    start(&emulator, &name).await;
    create(
        &emulator,
        &pipeline("gone", vec![stage("Source", vec![action("Src", None)])]),
    )
    .await;
    emulator.ok("DeletePipeline", &json!({"name": "gone"})).await;

    for (case, pipeline_name) in [
        ("a name never created", "missing"),
        ("a pipeline that was deleted", "gone"),
        ("a stored name in different case", "DEMO"),
    ] {
        let (status, code) = emulator
            .expect_error(
                "ListActionExecutions",
                &json!({"pipelineName": pipeline_name}),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "PipelineNotFoundException"),
            "case {case:?}: an unknown pipeline must not be listed as empty"
        );
    }
}

/// The filter selects by pipeline execution, through either member that names one, and
/// an id it names must belong to the pipeline being listed.
#[tokio::test]
async fn the_filter_selects_the_execution_it_names() {
    let emulator = Emulator::new();
    let name = create(
        &emulator,
        &pipeline(
            "demo",
            vec![stage("Source", vec![action("A", None), action("B", None)])],
        ),
    )
    .await;
    let first = start(&emulator, &name).await;
    let second = start(&emulator, &name).await;
    let other = create(
        &emulator,
        &pipeline("other", vec![stage("Source", vec![action("Src", None)])]),
    )
    .await;
    let other_execution = start(&emulator, &other).await;

    // (case, the filter, the execution whose records must be reported)
    let selections: Vec<(&str, Value, &String)> = vec![
        (
            "pipelineExecutionId names the older execution",
            json!({"pipelineExecutionId": first}),
            &first,
        ),
        (
            "pipelineExecutionId names the newer execution",
            json!({"pipelineExecutionId": second}),
            &second,
        ),
        (
            "latestInPipelineExecution with startTimeRange Latest",
            json!({
                "latestInPipelineExecution": {
                    "pipelineExecutionId": first,
                    "startTimeRange": "Latest",
                }
            }),
            &first,
        ),
        (
            "latestInPipelineExecution with startTimeRange All",
            json!({
                "latestInPipelineExecution": {
                    "pipelineExecutionId": first,
                    "startTimeRange": "All",
                }
            }),
            &first,
        ),
        (
            "both members naming the same execution",
            json!({
                "pipelineExecutionId": second,
                "latestInPipelineExecution": {
                    "pipelineExecutionId": second,
                    "startTimeRange": "All",
                }
            }),
            &second,
        ),
    ];
    for (case, filter, expected_execution) in selections {
        let page = emulator
            .ok(
                "ListActionExecutions",
                &json!({"pipelineName": name, "filter": filter}),
            )
            .await;
        assert_eq!(
            reported_actions(&page),
            vec![
                ("Source".to_string(), "A".to_string()),
                ("Source".to_string(), "B".to_string())
            ],
            "case {case:?}: the named execution started both actions, got {page}"
        );
        for detail in details(&page) {
            assert_eq!(
                detail["pipelineExecutionId"].as_str(),
                Some(expected_execution.as_str()),
                "case {case:?}: only the named execution's records are selected, got {detail}"
            );
        }
    }

    // `PipelineExecutionNotFoundException` is documented for an execution that "cannot
    // be found, or an execution ID does not belong to the specified pipeline", and both
    // filter members that name an execution are checked the same way.
    type FilterMember = (&'static str, fn(&str) -> Value);
    let members: [FilterMember; 2] = [
        ("pipelineExecutionId", |id| {
            json!({"pipelineExecutionId": id})
        }),
        ("latestInPipelineExecution", |id| {
            json!({
                "latestInPipelineExecution": {
                    "pipelineExecutionId": id,
                    "startTimeRange": "Latest",
                }
            })
        }),
    ];
    for (id_case, id) in [
        ("an id no start ever minted", UNSTORED_EXECUTION_ID),
        ("another pipeline's execution", other_execution.as_str()),
    ] {
        for (member_case, build) in &members {
            let (status, code) = emulator
                .expect_error(
                    "ListActionExecutions",
                    &json!({"pipelineName": name, "filter": build(id)}),
                )
                .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "PipelineExecutionNotFoundException"),
                "case {id_case:?} via {member_case:?}: unexpected refusal"
            );
        }
    }
}

/// A record's `input` is the declaration the execution runs: the action's type id and
/// configuration, the role it assumes (its own when it declares one, else the
/// pipeline's), its Region (its own when it declares one, else the pipeline's), its
/// namespace and the input artifacts it consumes by name.
#[tokio::test]
async fn a_record_reports_the_input_its_declaration_configures() {
    // The Region the pipeline is created in, which an action declaring none runs in.
    const PIPELINE_REGION: &str = "eu-west-1";
    // A Region an action may be pinned to, different from its pipeline's.
    const ACTION_REGION: &str = "ap-southeast-2";
    // A second role, satisfying `RoleArn`'s modelled pattern, for the action-level
    // override.
    const ACTION_ROLE_ARN: &str = "arn:aws:iam::123456789012:role/action-role";

    let emulator = Emulator::new();
    let mut inherits = action("Inherits", None);
    // No `inputArtifacts`: this action is in the pipeline's FIRST stage, where no earlier
    // action outputs anything for it to consume, and `CreatePipeline` refuses a
    // declaration that names one anyway (`validation_and_messages.rs` pins that
    // refusal). What a record reports for declared input artifacts is covered by the
    // execution suite, which runs a pipeline whose later stage consumes its source's
    // output.
    inherits["namespace"] = json!("SourceVariables");
    let mut overrides = action("Overrides", None);
    overrides["roleArn"] = json!(ACTION_ROLE_ARN);
    overrides["region"] = json!(ACTION_REGION);

    let declaration = pipeline("demo", vec![stage("Source", vec![inherits, overrides])]);
    let (status, response) = emulator
        .call_in(
            PIPELINE_REGION,
            "CreatePipeline",
            &json!({"pipeline": declaration}),
        )
        .await;
    assert_eq!(
        status, 200,
        "the pipeline must be created in {PIPELINE_REGION}, got {response}"
    );
    emulator
        .ok("StartPipelineExecution", &json!({"name": "demo"}))
        .await;

    let page = emulator
        .ok("ListActionExecutions", &json!({"pipelineName": "demo"}))
        .await;
    let reported = details(&page);
    let by_action = |action_name: &str| -> Value {
        reported
            .iter()
            .find(|detail| detail["actionName"] == json!(action_name))
            .unwrap_or_else(|| panic!("{action_name} must be reported, got {page}"))
            .clone()
    };

    // (case, action, member of `input`, expected value)
    let expectations: Vec<(&str, &str, &str, Value)> = vec![
        (
            "the declared action type id",
            "Inherits",
            "actionTypeId",
            // On the wire an `ActionTypeId` carries the awsJson shape discriminator beside
            // the four members the shape declares, wherever this service serializes one.
            json!({
                "__type": "ActionTypeId",
                "category": "Source",
                "owner": "Custom",
                "provider": "MySource",
                "version": "1",
            }),
        ),
        (
            "the declared configuration",
            "Inherits",
            "configuration",
            json!({"S3Bucket": "my-bucket", "S3ObjectKey": "app.zip"}),
        ),
        (
            "a configuration with no variable reference resolves to itself",
            "Inherits",
            "resolvedConfiguration",
            json!({"S3Bucket": "my-bucket", "S3ObjectKey": "app.zip"}),
        ),
        (
            "the pipeline's role, which the action declares none of",
            "Inherits",
            "roleArn",
            json!(ROLE_ARN),
        ),
        (
            "the pipeline's Region, which the action declares none of",
            "Inherits",
            "region",
            json!(PIPELINE_REGION),
        ),
        (
            "the declared namespace",
            "Inherits",
            "namespace",
            json!("SourceVariables"),
        ),
        (
            "the action's own role overrides the pipeline's",
            "Overrides",
            "roleArn",
            json!(ACTION_ROLE_ARN),
        ),
        (
            "the action's own Region overrides the pipeline's",
            "Overrides",
            "region",
            json!(ACTION_REGION),
        ),
    ];
    for (case, action_name, member, expected) in expectations {
        let detail = by_action(action_name);
        assert_eq!(
            detail["input"][member], expected,
            "case {case:?}: unexpected input.{member} on {action_name}, got {detail}"
        );
    }

    // An action that declares no namespace and no input artifact reports neither.
    let overrides = by_action("Overrides");
    assert!(
        overrides["input"]["namespace"].is_null()
            && overrides["input"]["inputArtifacts"].is_null(),
        "an action declaring neither namespace nor input artifact reports neither, got {overrides}"
    );
}

/// `resolvedConfiguration` is the configuration with every reference this emulator can
/// resolve replaced: a pipeline variable's reference takes the value the execution
/// resolved for it, while a reference into an action's namespace — whose values are
/// output variables nothing here produces — leaves the member absent rather than
/// half-resolved.
#[tokio::test]
async fn resolved_configuration_substitutes_the_executions_pipeline_variables() {
    // (case, the configuration value declared, the resolvedConfiguration expected)
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "no reference at all resolves to itself",
            json!("plain-value"),
            json!({"Key": "plain-value"}),
        ),
        (
            "a pipeline-variable reference takes the resolved value",
            json!("#{variables.Env}"),
            json!({"Key": "staging"}),
        ),
        (
            "a reference inside surrounding text",
            json!("deploy-to-#{variables.Env}-now"),
            json!({"Key": "deploy-to-staging-now"}),
        ),
        (
            "the same reference twice",
            json!("#{variables.Env}/#{variables.Env}"),
            json!({"Key": "staging/staging"}),
        ),
        (
            "an action namespace's output variable cannot be resolved",
            json!("#{SourceVariables.CommitId}"),
            Value::Null,
        ),
        (
            "one resolvable and one unresolvable reference",
            json!("#{variables.Env}-#{SourceVariables.CommitId}"),
            Value::Null,
        ),
    ];

    for (case, configured, expected) in cases {
        let emulator = Emulator::new();
        let mut declared_action = action("Src", None);
        declared_action["configuration"] = json!({"Key": configured});
        let mut declaration = pipeline("demo", vec![stage("Source", vec![declared_action])]);
        declaration["variables"] = json!([{"name": "Env", "defaultValue": "staging"}]);

        let name = create(&emulator, &declaration).await;
        start(&emulator, &name).await;
        let page = emulator
            .ok("ListActionExecutions", &json!({"pipelineName": name}))
            .await;
        let detail = details(&page)
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: the started action is reported, got {page}"))
            .clone();
        assert_eq!(
            detail["input"]["resolvedConfiguration"], expected,
            "case {case:?}: unexpected resolvedConfiguration, got {detail}"
        );
        assert_eq!(
            detail["input"]["configuration"],
            json!({"Key": configured}),
            "case {case:?}: the declared configuration is reported as declared, got {detail}"
        );
    }
}

/// Substitution is ONE pass over the declared value: a reference is replaced by the
/// value the execution resolved for that variable, and what the replacement itself
/// contains is never re-scanned — so a chain of variables cannot expand, and a value
/// that looks like a reference is reported literally. A reference this emulator
/// cannot resolve still leaves the member absent.
#[tokio::test]
async fn resolved_configuration_substitutes_in_one_pass() {
    // (case, the pipeline's variable declarations, the configuration value declared,
    //  the resolvedConfiguration expected)
    let cases: Vec<(&str, Value, &str, Value)> = vec![
        (
            "a variable whose value is itself a reference is substituted literally",
            json!([
                {"name": "Env", "defaultValue": "#{variables.Other}"},
                {"name": "Other", "defaultValue": "prod"},
            ]),
            "#{variables.Env}",
            json!({"Key": "#{variables.Other}"}),
        ),
        (
            "a chain of three variables expands exactly one step",
            json!([
                {"name": "A", "defaultValue": "#{variables.B}"},
                {"name": "B", "defaultValue": "#{variables.C}"},
                {"name": "C", "defaultValue": "end"},
            ]),
            "#{variables.A}",
            json!({"Key": "#{variables.B}"}),
        ),
        (
            "the reverse declaration order expands exactly one step too",
            json!([
                {"name": "C", "defaultValue": "end"},
                {"name": "B", "defaultValue": "#{variables.C}"},
                {"name": "A", "defaultValue": "#{variables.B}"},
            ]),
            "#{variables.A}",
            json!({"Key": "#{variables.B}"}),
        ),
        (
            "an unterminated reference has no resolved form",
            json!([{"name": "Env", "defaultValue": "staging"}]),
            "#{variables.Env",
            Value::Null,
        ),
        (
            "a reference to a variable the pipeline does not declare has none either",
            json!([{"name": "Env", "defaultValue": "staging"}]),
            "#{variables.Missing}",
            Value::Null,
        ),
    ];

    for (case, variables, configured, expected) in cases {
        let emulator = Emulator::new();
        let mut declared_action = action("Src", None);
        declared_action["configuration"] = json!({"Key": configured});
        let mut declaration = pipeline("demo", vec![stage("Source", vec![declared_action])]);
        declaration["variables"] = variables;

        let name = create(&emulator, &declaration).await;
        start(&emulator, &name).await;
        let page = emulator
            .ok("ListActionExecutions", &json!({"pipelineName": name}))
            .await;
        let detail = details(&page)
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: the started action is reported, got {page}"))
            .clone();
        assert_eq!(
            detail["input"]["resolvedConfiguration"], expected,
            "case {case:?}: unexpected resolvedConfiguration, got {detail}"
        );
    }
}

/// Listing action executions reports the same thing twice and changes nothing a later
/// caller sees — it starts no action, no execution, and touches no stored pipeline.
#[tokio::test]
async fn listing_action_executions_changes_nothing() {
    let emulator = Emulator::new();
    let name = create(
        &emulator,
        &pipeline(
            "demo",
            vec![
                stage("Source", vec![action("A", None), action("B", None)]),
                stage("Build", vec![later_action("Compile")]),
            ],
        ),
    )
    .await;
    let execution_id = start(&emulator, &name).await;
    let request = json!({
        "pipelineName": name,
        "filter": {"pipelineExecutionId": execution_id},
    });

    let executions_before = emulator
        .ok("ListPipelineExecutions", &json!({"pipelineName": name}))
        .await;
    let state_before = emulator.ok("GetPipelineState", &json!({"name": name})).await;

    let first = emulator.ok("ListActionExecutions", &request).await;
    let second = emulator.ok("ListActionExecutions", &request).await;
    assert_eq!(
        first, second,
        "a read must report the same thing twice, got {first} then {second}"
    );

    let executions_after = emulator
        .ok("ListPipelineExecutions", &json!({"pipelineName": name}))
        .await;
    let state_after = emulator.ok("GetPipelineState", &json!({"name": name})).await;
    assert_eq!(
        executions_before, executions_after,
        "listing action executions must not change the executions a later caller sees"
    );
    assert_eq!(
        state_before, state_after,
        "listing action executions must not change the pipeline state a later caller sees"
    );
}
