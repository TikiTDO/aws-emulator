// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `BatchGetWorkflows`: the batch reader that "returns a list of
//! resource metadata for a given list of workflow names".
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a batch read answers one entry per REQUESTED name, in the request's
//! own order and keeping its repeats — the stored workflow's projection under `Workflows`,
//! the name itself under `MissingWorkflows` when this account holds no workflow under it —
//! and the projection it serves for a name is the same one the single-workflow read
//! (`GetWorkflow`) serves, graph included when the batch asked for one. Because both output
//! members target a list the model bounds `length 1..25`
//! (`smithy_query shape=BatchGetWorkflowsResponse`), neither is reported as an empty list:
//! the member whose list would be empty is omitted.
//!
//! The suites are tables: the rows differ along what the rule generalises over — how many
//! names the batch carries (one, several, the modelled maximum), whether each resolves,
//! which order they are in, whether a name repeats, and the form and length of the name.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, names_in, ok};

/// The largest batch the model accepts, and also the largest answer either output member
/// can carry: "len 1..25" on `WorkflowNames` and on `Workflows`
/// (`smithy_query shape=WorkflowNames,Workflows`).
const MAX_BATCH: usize = 25;

/// The longest `Name` the model accepts: "len 1..255" on `NameString`
/// (`smithy_query operation=GetWorkflow`).
const MAX_NAME_LENGTH: usize = 255;

/// The role ARN the job a trigger action runs is registered with.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/sim2real-glue-job-role";

/// Create a workflow.
async fn create(router: &MultiServiceRouter, body: &Value) {
    ok(router, "CreateWorkflow", body).await;
}

/// The `Workflow` structure `GetWorkflow` reports for `name`, which is the projection a
/// batch read of the same name must serve.
async fn get(router: &MultiServiceRouter, request: &Value) -> Value {
    let body = ok(router, "GetWorkflow", request).await;
    body.get("Workflow")
        .cloned()
        .unwrap_or_else(|| panic!("GetWorkflow must report a Workflow, got {body}"))
}

/// Read `names` in one batch, optionally asking for the graph, and answer the status and
/// the parsed body.
async fn batch_call(
    router: &MultiServiceRouter,
    names: &[String],
    include_graph: Option<bool>,
) -> (u16, Value) {
    let mut request = json!({ "Names": names });
    if let Some(include_graph) = include_graph {
        request["IncludeGraph"] = json!(include_graph);
    }
    call(router, "BatchGetWorkflows", &request).await
}

/// The same, for a batch that must succeed.
async fn batch(
    router: &MultiServiceRouter,
    names: &[String],
    include_graph: Option<bool>,
) -> Value {
    let (status, body) = batch_call(router, names, include_graph).await;
    assert_eq!(
        status, 200,
        "BatchGetWorkflows over {names:?} must answer 200, got {status} with {body}"
    );
    body
}

/// The elements of an output list member, read as the empty list when the member is absent
/// — which is how a batch reports "none of these", since the model bounds both of its
/// output lists `length 1..25` and so admits no empty one.
fn optional_list(body: &Value, member: &str) -> Vec<Value> {
    match body.get(member) {
        None | Some(Value::Null) => Vec::new(),
        Some(value) => value
            .as_array()
            .unwrap_or_else(|| panic!("{member} must be a list, got {value}"))
            .clone(),
    }
}

/// The `MissingWorkflows` names a batch reported, in the order it reported them.
fn missing_names(body: &Value) -> Vec<String> {
    optional_list(body, "MissingWorkflows")
        .iter()
        .map(|name| {
            name.as_str()
                .unwrap_or_else(|| panic!("MissingWorkflows holds strings, got {name}"))
                .to_string()
        })
        .collect()
}

/// The `Workflows` names a batch reported, in the order it reported them.
fn found_names(body: &Value) -> Vec<String> {
    if body.get("Workflows").is_none_or(Value::is_null) {
        return Vec::new();
    }
    names_in(body, "Workflows")
}

/// The workflows every row below reads over, created once per suite: three plainly named
/// ones carrying different members, a name at the modelled length bound, a name carrying
/// whitespace, and enough numbered ones to fill a batch at the modelled maximum.
async fn stored_workflows(router: &MultiServiceRouter) -> Vec<String> {
    let long_name = "w".repeat(MAX_NAME_LENGTH);
    let definitions = vec![
        json!({ "Name": "alpha", "Description": "the first workflow" }),
        json!({ "Name": "beta", "DefaultRunProperties": { "env": "staging" } }),
        json!({ "Name": "gamma", "MaxConcurrentRuns": 4 }),
        json!({ "Name": long_name }),
        json!({ "Name": "batch read\ttwo" }),
    ];
    let mut names = Vec::new();
    for definition in &definitions {
        create(router, definition).await;
        names.push(definition["Name"].as_str().expect("a Name").to_string());
    }
    for index in 0..MAX_BATCH {
        let name = format!("bulk-{index:02}");
        create(router, &json!({ "Name": name })).await;
        names.push(name);
    }
    names
}

/// One row of the batch table: the names the request carries, and what the answer must
/// report for them.
struct BatchCase {
    /// What this row varies, named in every assertion it drives.
    case: &'static str,
    /// The `Names` the batch asks for, in request order and with its repeats.
    names: Vec<String>,
    /// The names that must come back under `Workflows`, in order.
    found: Vec<String>,
    /// The names that must come back under `MissingWorkflows`, in order.
    missing: Vec<String>,
}

/// The rows every suite below drives.
fn batch_cases() -> Vec<BatchCase> {
    let long_name = "w".repeat(MAX_NAME_LENGTH);
    let whitespace_name = "batch read\ttwo".to_string();
    let bulk: Vec<String> = (0..MAX_BATCH).map(|index| format!("bulk-{index:02}")).collect();
    let mut bulk_with_misses = bulk[..MAX_BATCH - 3].to_vec();
    bulk_with_misses.extend(["gone-one".to_string(), "gone-two".to_string(), "gone-three".to_string()]);

    vec![
        BatchCase {
            case: "the smallest batch, one stored name",
            names: vec!["alpha".to_string()],
            found: vec!["alpha".to_string()],
            missing: Vec::new(),
        },
        BatchCase {
            case: "the smallest batch, one name nothing is stored under",
            names: vec!["no-such-workflow".to_string()],
            found: Vec::new(),
            missing: vec!["no-such-workflow".to_string()],
        },
        BatchCase {
            case: "three stored names",
            names: vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()],
            found: vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()],
            missing: Vec::new(),
        },
        BatchCase {
            case: "the same three names in the reverse order",
            names: vec!["gamma".to_string(), "beta".to_string(), "alpha".to_string()],
            found: vec!["gamma".to_string(), "beta".to_string(), "alpha".to_string()],
            missing: Vec::new(),
        },
        BatchCase {
            case: "stored and missing names interleaved",
            names: vec![
                "missing-first".to_string(),
                "alpha".to_string(),
                "missing-second".to_string(),
                "gamma".to_string(),
            ],
            found: vec!["alpha".to_string(), "gamma".to_string()],
            missing: vec!["missing-first".to_string(), "missing-second".to_string()],
        },
        BatchCase {
            case: "every name missing",
            names: vec!["ghost-one".to_string(), "ghost-two".to_string()],
            found: Vec::new(),
            missing: vec!["ghost-one".to_string(), "ghost-two".to_string()],
        },
        BatchCase {
            case: "one stored name repeated three times",
            names: vec!["beta".to_string(), "beta".to_string(), "beta".to_string()],
            found: vec!["beta".to_string(), "beta".to_string(), "beta".to_string()],
            missing: Vec::new(),
        },
        BatchCase {
            case: "one missing name repeated twice, beside a stored one",
            names: vec![
                "absent".to_string(),
                "alpha".to_string(),
                "absent".to_string(),
            ],
            found: vec!["alpha".to_string()],
            missing: vec!["absent".to_string(), "absent".to_string()],
        },
        BatchCase {
            case: "a name at the modelled length bound",
            names: vec![long_name.clone(), "alpha".to_string()],
            found: vec![long_name, "alpha".to_string()],
            missing: Vec::new(),
        },
        BatchCase {
            case: "a name carrying a space and a tab",
            names: vec![whitespace_name.clone()],
            found: vec![whitespace_name],
            missing: Vec::new(),
        },
        BatchCase {
            case: "a batch at the modelled maximum, every name stored",
            names: bulk.clone(),
            found: bulk,
            missing: Vec::new(),
        },
        BatchCase {
            case: "a batch at the modelled maximum, three names missing",
            names: bulk_with_misses.clone(),
            found: bulk_with_misses[..MAX_BATCH - 3].to_vec(),
            missing: bulk_with_misses[MAX_BATCH - 3..].to_vec(),
        },
    ]
}

// ---------------------------------------------------------------------------
// One entry per requested name
// ---------------------------------------------------------------------------

/// A batch read answers one entry per requested name — the stored projection under
/// `Workflows`, the name under `MissingWorkflows` — in the request's order and keeping its
/// repeats, and omits the member whose list would be empty.
#[tokio::test]
async fn a_batch_answers_one_entry_per_requested_name() {
    let router = emulator();
    stored_workflows(&router).await;

    for case in batch_cases() {
        let body = batch(&router, &case.names, None).await;

        assert_eq!(
            found_names(&body),
            case.found,
            "case {}: expected Workflows for {:?}, got {body}",
            case.case,
            case.found
        );
        assert_eq!(
            missing_names(&body),
            case.missing,
            "case {}: expected MissingWorkflows {:?}, got {body}",
            case.case,
            case.missing
        );

        // Every requested name is accounted for exactly once, so the two lists together
        // are the request.
        assert_eq!(
            found_names(&body).len() + missing_names(&body).len(),
            case.names.len(),
            "case {}: every requested name must be accounted for, got {body}",
            case.case
        );

        // Neither member is reported as an empty list: the model bounds both output lists
        // `length 1..25`, so "none of these" is an omitted member.
        if case.found.is_empty() {
            assert!(
                body.get("Workflows").is_none_or(Value::is_null),
                "case {}: a batch that found nothing reports no Workflows, got {body}",
                case.case
            );
        }
        if case.missing.is_empty() {
            assert!(
                body.get("MissingWorkflows").is_none_or(Value::is_null),
                "case {}: a batch that missed nothing reports no MissingWorkflows, got {body}",
                case.case
            );
        }
    }
}

/// The projection a batch serves for a name is the one the single-workflow read serves for
/// it, member for member — so a client reads the same workflow whichever reader it uses.
#[tokio::test]
async fn a_batch_serves_the_single_read_s_projection() {
    let router = emulator();
    stored_workflows(&router).await;

    for case in batch_cases() {
        let body = batch(&router, &case.names, None).await;
        let projections = optional_list(&body, "Workflows");
        assert_eq!(
            projections.len(),
            case.found.len(),
            "case {}: expected one projection per found name, got {body}",
            case.case
        );

        for (name, projection) in case.found.iter().zip(projections.iter()) {
            let single = get(&router, &json!({ "Name": name })).await;
            assert_eq!(
                projection, &single,
                "case {}: the batch projection of {name:?} must equal GetWorkflow's, got \
                 {projection} against {single}",
                case.case
            );
        }
    }
}

/// `IncludeGraph` governs the whole batch: a batch that asked for no graph is answered
/// without one for any workflow, and a batch that asked gets the graph of each — the same
/// graph the single read reports.
#[tokio::test]
async fn include_graph_governs_the_whole_batch() {
    let router = emulator();
    stored_workflows(&router).await;

    // One workflow with a component attached, so an asked-for graph has a node in it and
    // "present but empty" is not the only thing the assertion can see.
    ok(
        &router,
        "CreateJob",
        &json!({
            "Name": "batch-read-job",
            "Role": ROLE_ARN,
            "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
        }),
    )
    .await;
    create(&router, &json!({ "Name": "attached" })).await;
    ok(
        &router,
        "CreateTrigger",
        &json!({
            "Name": "batch-read-trigger",
            "WorkflowName": "attached",
            "Type": "ON_DEMAND",
            "Actions": [{ "JobName": "batch-read-job" }],
        }),
    )
    .await;

    let names: Vec<String> = vec![
        "attached".to_string(),
        "alpha".to_string(),
        "beta".to_string(),
    ];

    for (case, include_graph) in [
        ("no IncludeGraph member", None),
        ("IncludeGraph false", Some(false)),
    ] {
        let body = batch(&router, &names, include_graph).await;
        for projection in optional_list(&body, "Workflows") {
            assert!(
                projection.get("Graph").is_none_or(Value::is_null),
                "case {case}: expected no Graph, got {projection}"
            );
        }
    }

    let body = batch(&router, &names, Some(true)).await;
    let projections = optional_list(&body, "Workflows");
    assert_eq!(
        projections.len(),
        names.len(),
        "case IncludeGraph true: expected one projection per name, got {body}"
    );
    for (name, projection) in names.iter().zip(projections.iter()) {
        let single = get(&router, &json!({ "Name": name, "IncludeGraph": true })).await;
        assert_eq!(
            projection.get("Graph"),
            single.get("Graph"),
            "case IncludeGraph true for {name:?}: the batch must report the single read's \
             graph, got {projection} against {single}"
        );
        let graph = projection
            .get("Graph")
            .unwrap_or_else(|| panic!("case IncludeGraph true for {name:?}: expected a Graph, got {projection}"));
        let nodes = graph
            .get("Nodes")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case IncludeGraph true for {name:?}: Graph.Nodes must be a list, got {graph}"));
        let expected_nodes = usize::from(name == "attached") * 2;
        assert_eq!(
            nodes.len(),
            expected_nodes,
            "case IncludeGraph true for {name:?}: expected {expected_nodes} nodes, got {graph}"
        );
    }
}

/// A batch read reports the workflow's last execution, and reports the same run the single
/// read does — so the batch reader is not a projection that drops what a run produced.
#[tokio::test]
async fn a_batch_reports_the_last_execution() {
    let router = emulator();
    ok(
        &router,
        "CreateJob",
        &json!({
            "Name": "batch-last-run-job",
            "Role": ROLE_ARN,
            "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
        }),
    )
    .await;
    create(&router, &json!({ "Name": "never-run" })).await;
    create(&router, &json!({ "Name": "has-run" })).await;
    ok(
        &router,
        "CreateTrigger",
        &json!({
            "Name": "batch-last-run-trigger",
            "WorkflowName": "has-run",
            "Type": "ON_DEMAND",
            "Actions": [{ "JobName": "batch-last-run-job" }],
        }),
    )
    .await;
    let started = ok(&router, "StartWorkflowRun", &json!({ "Name": "has-run" })).await;
    let run_id = started
        .get("RunId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("StartWorkflowRun must answer a RunId, got {started}"))
        .to_string();

    let names = vec!["never-run".to_string(), "has-run".to_string()];
    let body = batch(&router, &names, None).await;
    let projections = optional_list(&body, "Workflows");
    assert_eq!(
        projections.len(),
        2,
        "case a run and a workflow without one: expected two projections, got {body}"
    );

    assert!(
        projections[0].get("LastRun").is_none_or(Value::is_null),
        "case never run: a workflow with no execution reports no LastRun, got {}",
        projections[0]
    );

    let last_run = projections[1]
        .get("LastRun")
        .unwrap_or_else(|| panic!("case has run: expected a LastRun, got {}", projections[1]));
    assert_eq!(
        last_run.get("WorkflowRunId").and_then(Value::as_str),
        Some(run_id.as_str()),
        "case has run: expected the started run {run_id}, got {last_run}"
    );
    let single = get(&router, &json!({ "Name": "has-run" })).await;
    assert_eq!(
        last_run.get("WorkflowRunId"),
        single.get("LastRun").and_then(|run| run.get("WorkflowRunId")),
        "case has run: the batch and the single read must report the same last run, got \
         {last_run} against {single}"
    );
}

/// A batch read changes nothing a later caller sees: the same batch answers identically,
/// and the single read of each name still answers what it did before the batch ran.
#[tokio::test]
async fn a_batch_read_changes_nothing() {
    let router = emulator();
    stored_workflows(&router).await;

    for case in batch_cases() {
        let mut before = Vec::new();
        for name in &case.found {
            before.push(get(&router, &json!({ "Name": name })).await);
        }

        let first = batch(&router, &case.names, None).await;
        let second = batch(&router, &case.names, None).await;
        assert_eq!(
            first, second,
            "case {}: a batch read must be repeatable, got {second}",
            case.case
        );

        for (name, before) in case.found.iter().zip(before.iter()) {
            let after = get(&router, &json!({ "Name": name })).await;
            assert_eq!(
                &after, before,
                "case {}: the batch must leave {name:?} as it was, got {after}",
                case.case
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The modelled batch bound
// ---------------------------------------------------------------------------

/// `Names` is bounded `length 1..25`: a batch at the bound is served, and one on either
/// side of it — empty, or one name too many — is refused at the request boundary with the
/// constraint validator's `ValidationException` and 400.
#[tokio::test]
async fn a_batch_outside_the_modelled_bound_is_refused() {
    let router = emulator();
    stored_workflows(&router).await;

    let at_bound: Vec<String> = (0..MAX_BATCH).map(|index| format!("bulk-{index:02}")).collect();
    let body = batch(&router, &at_bound, None).await;
    assert_eq!(
        found_names(&body).len(),
        MAX_BATCH,
        "case a batch of {MAX_BATCH}: expected one projection per name, got {body}"
    );

    let past_bound: Vec<String> = (0..=MAX_BATCH).map(|index| format!("bulk-{index:02}")).collect();
    for (case, names) in [
        ("an empty batch", Vec::new()),
        ("one name past the modelled maximum", past_bound),
    ] {
        let (status, body) = batch_call(&router, &names, None).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {} names, got {status} with {body}",
            names.len()
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {case}: expected the boundary's ValidationException, got {} in {body}",
            error_code(&body)
        );
    }

    // `Names` is required, so a request that carries none is refused the same way.
    let (status, body) = call(&router, "BatchGetWorkflows", &json!({})).await;
    assert_eq!(
        status, 400,
        "case no Names member: expected 400, got {status} with {body}"
    );
    assert!(
        error_code(&body).ends_with("ValidationException"),
        "case no Names member: expected the boundary's ValidationException, got {} in {body}",
        error_code(&body)
    );
}
