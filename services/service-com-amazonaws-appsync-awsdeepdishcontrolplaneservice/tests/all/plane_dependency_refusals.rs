// SPDX-License-Identifier: CC0-1.0

//! What one GraphQL API's resources still use of each other, end to end over HTTP: the
//! deletes AppSync refuses while a reference is live, the deletes it serves once the
//! reference is gone, and the `UpdateResolver` that restates a resolver.
//!
//! The rules asserted here are stated over CLASSES of input rather than over the one
//! request that exposed the defect:
//!
//! 1. a data source a FUNCTION of its API still resolves against cannot be deleted: the
//!    call is a 400 `BadRequestException` listing the ids of every function that names
//!    it, in creation order, and the data source is left in place — whatever the number
//!    of referring functions, while a data source no function names is deletable even
//!    when a resolver resolves against it;
//! 2. a function a RESOLVER of its API still invokes (a step of its `pipelineConfig`)
//!    cannot be deleted: the call is a 400 `BadRequestException` naming the type and the
//!    field that address the referring resolver, and the function is left in place —
//!    whatever type and field the resolver is attached to;
//! 3. a delete whose reference is gone is served: detaching the resolver, then deleting
//!    the function, then the data source, each answers 200 with an empty body and leaves
//!    the resource unreadable afterwards, so a client's teardown runs to the end;
//! 4. an update RESTATES the resolver from the members the request carries and answers
//!    the stored record: the kind it names (`UNIT` where it names none), its pipeline,
//!    its templates verbatim and its `code` — so a member the update leaves out is gone
//!    from the resolver, and `GetResolver` answers the same body;
//! 5. an update describing a definition AppSync could not run, or addressing a field that
//!    carries no resolver, is refused and changes nothing.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization, the boundary validator, the handlers
//! and the shared engine together. The harness (`call`, `create_api`, `router`, `Answer`)
//! is the error-contract suite's and the data-source and function helpers are the
//! function suite's — reused rather than copied.

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

use super::error_contract::{call, create_api, router, Answer};
// The data sources and functions under test are created through the function suite's own
// helpers rather than through second copies of them.
use super::functions::{create_data_source, create_function, Row, RUNTIME};

/// The name of the data source the functions under test resolve against.
const DATA_SOURCE: &str = "ds_dependencies";

/// A second data source of the same API, which no function names.
const OTHER_DATA_SOURCE: &str = "ds_unreferenced";

/// The type the resolvers under test are attached to, and a second one, so a rule stated
/// over the referring resolver is not stated over one type.
const TYPES: &[&str] = &["Query", "Mutation"];

/// How many functions name one data source, in the rows of rule 1: none, one, and
/// several — the list in the refusal grows with them.
const REFERENCE_COUNTS: &[usize] = &[0, 1, 2, 3];

/// Create `count` functions resolving against `data_source` and return the ids the
/// creates minted, in the order they were created.
async fn create_functions(
    router: &MultiServiceRouter,
    api_id: &str,
    data_source: &str,
    count: usize,
) -> Vec<String> {
    let mut ids = Vec::new();
    for index in 0..count {
        let row = Row {
            request_template: Some("{\"version\": \"2018-05-29\"}"),
            function_version: Some(super::functions::FUNCTION_VERSION),
            ..Row::bare("a VTL function naming the data source", "fn_dependency")
        };
        let answer = create_function(router, api_id, data_source, &row).await;
        assert_eq!(
            answer.status, 200,
            "case create function {index}: expected 200, got {answer:?}"
        );
        ids.push(
            answer.body["functionConfiguration"]["functionId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
    ids
}

/// Send one `DeleteDataSource` for `name`.
async fn delete_data_source(router: &MultiServiceRouter, api_id: &str, name: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/v1/apis/{api_id}/datasources/{name}"),
        None,
    )
    .await
}

/// Send one `DeleteFunction` for `function_id`.
async fn delete_function(router: &MultiServiceRouter, api_id: &str, function_id: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/v1/apis/{api_id}/functions/{function_id}"),
        None,
    )
    .await
}

/// Send one `CreateResolver` with `body` on the type `type_name`.
async fn create_resolver(
    router: &MultiServiceRouter,
    api_id: &str,
    type_name: &str,
    body: Value,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/{type_name}/resolvers"),
        Some(body),
    )
    .await
}

/// Send one `UpdateResolver` with `body` for the resolver on `type_name`/`field`.
async fn update_resolver(
    router: &MultiServiceRouter,
    api_id: &str,
    type_name: &str,
    field: &str,
    body: Value,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/{type_name}/resolvers/{field}"),
        Some(body),
    )
    .await
}

/// Read one resolver back by the type and the field it is attached to.
async fn get_resolver(
    router: &MultiServiceRouter,
    api_id: &str,
    type_name: &str,
    field: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/apis/{api_id}/types/{type_name}/resolvers/{field}"),
        None,
    )
    .await
}

/// Send one `DeleteResolver` for the resolver on `type_name`/`field`.
async fn delete_resolver(
    router: &MultiServiceRouter,
    api_id: &str,
    type_name: &str,
    field: &str,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/v1/apis/{api_id}/types/{type_name}/resolvers/{field}"),
        None,
    )
    .await
}

/// A `PIPELINE` resolver body invoking `function_ids`.
fn pipeline_body(field: &str, function_ids: &[String]) -> Value {
    json!({
        "fieldName": field,
        "kind": state::PIPELINE_KIND,
        "pipelineConfig": {"functions": function_ids},
        "requestMappingTemplate": "{\"version\": \"2018-05-29\"}",
        "responseMappingTemplate": "$util.toJson($ctx.result)",
    })
}

/// Assert `answer` is the empty 200 a delete that removed the resource answers with:
/// `DeleteDataSourceResponse`, `DeleteFunctionResponse` and `DeleteResolverResponse`
/// declare no members at all, so the body is an empty document.
fn assert_empty_ok(case: &str, answer: &Answer) {
    assert_eq!(
        (answer.status, &answer.body),
        (200, &json!({})),
        "case {case:?}: expected an empty 200, got {answer:?}"
    );
}

/// Rule 1: a data source a function still resolves against is not deleted, and the
/// refusal lists the functions that name it.
#[tokio::test]
async fn a_data_source_a_function_resolves_against_is_not_deleted() {
    for count in REFERENCE_COUNTS {
        let router = router();
        let api_id = create_api(&router, &format!("deps-ds-{count}")).await;
        create_data_source(&router, &api_id, DATA_SOURCE).await;
        let function_ids = create_functions(&router, &api_id, DATA_SOURCE, *count).await;

        let answer = delete_data_source(&router, &api_id, DATA_SOURCE).await;
        if function_ids.is_empty() {
            // Nothing names the data source, so there is nothing in the way.
            assert_empty_ok("a data source no function names", &answer);
            continue;
        }

        let ids: Vec<&str> = function_ids.iter().map(String::as_str).collect();
        let expected = state::data_source_in_use_message(&ids);
        assert_eq!(
            (
                answer.status,
                answer.error_type.as_str(),
                answer.message.as_str()
            ),
            (400, "BadRequestException", expected.as_str()),
            "case a data source {count} function(s) name: expected 400 BadRequestException \
             {expected:?}, got {answer:?}"
        );

        // The refusal left the data source in place: it is still readable, so the
        // caller's next call sees exactly what it saw before the delete.
        let read = call(
            &router,
            "GET",
            &format!("/v1/apis/{api_id}/datasources/{DATA_SOURCE}"),
            None,
        )
        .await;
        assert_eq!(
            (read.status, read.body["dataSource"]["name"].as_str()),
            (200, Some(DATA_SOURCE)),
            "case a data source {count} function(s) name: expected it still readable, got {read:?}"
        );
    }
}

/// Rule 1, the other side: what blocks a delete is a FUNCTION naming the data source,
/// so a data source only a resolver resolves against is deletable, and a function naming
/// one data source does not block the delete of another.
#[tokio::test]
async fn only_a_function_reference_blocks_a_data_source_delete() {
    let router = router();
    let api_id = create_api(&router, "deps-ds-scope").await;
    create_data_source(&router, &api_id, DATA_SOURCE).await;
    create_data_source(&router, &api_id, OTHER_DATA_SOURCE).await;
    create_functions(&router, &api_id, DATA_SOURCE, 1).await;

    // A UNIT resolver resolving against the second data source: a resolver reference is
    // not what AppSync's refusal names, and the data source is deleted.
    let answer = create_resolver(
        &router,
        &api_id,
        TYPES[0],
        json!({
            "fieldName": "unitField",
            "kind": state::UNIT_KIND,
            "dataSourceName": OTHER_DATA_SOURCE,
            "requestMappingTemplate": "{\"version\": \"2018-05-29\"}",
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case attach a UNIT resolver to the second data source: expected 200, got {answer:?}"
    );

    let answer = delete_data_source(&router, &api_id, OTHER_DATA_SOURCE).await;
    assert_empty_ok("a data source only a resolver resolves against", &answer);

    // The first data source is still blocked, so the rule is about the reference and not
    // about the API holding any function at all.
    let answer = delete_data_source(&router, &api_id, DATA_SOURCE).await;
    assert_eq!(
        answer.status, 400,
        "case the data source its function names: expected 400, got {answer:?}"
    );
}

/// Rule 2: a function a resolver invokes is not deleted, and the refusal names the type
/// and the field that address that resolver.
#[tokio::test]
async fn a_function_a_resolver_invokes_is_not_deleted() {
    // The rows differ along the type and the field the referring resolver is attached
    // to, which are the two variable parts of AppSync's sentence.
    let cases: &[(&str, &str)] = &[
        (TYPES[0], "getItem"),
        (TYPES[0], "listItems"),
        (TYPES[1], "putItem"),
    ];

    for (type_name, field) in cases {
        let router = router();
        let api_id = create_api(&router, &format!("deps-fn-{type_name}-{field}")).await;
        create_data_source(&router, &api_id, DATA_SOURCE).await;
        let function_ids = create_functions(&router, &api_id, DATA_SOURCE, 1).await;

        let answer = create_resolver(
            &router,
            &api_id,
            type_name,
            pipeline_body(field, &function_ids),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case a pipeline on {type_name}/{field}: expected 200, got {answer:?}"
        );

        let answer = delete_function(&router, &api_id, &function_ids[0]).await;
        let expected = state::function_in_use_message(type_name, field);
        assert_eq!(
            (
                answer.status,
                answer.error_type.as_str(),
                answer.message.as_str()
            ),
            (400, "BadRequestException", expected.as_str()),
            "case a function the resolver on {type_name}/{field} invokes: expected 400 \
             BadRequestException {expected:?}, got {answer:?}"
        );

        // The refusal left the function in place.
        let read = call(
            &router,
            "GET",
            &format!("/v1/apis/{api_id}/functions/{}", function_ids[0]),
            None,
        )
        .await;
        assert_eq!(
            (
                read.status,
                read.body["functionConfiguration"]["functionId"].as_str()
            ),
            (200, Some(function_ids[0].as_str())),
            "case a function the resolver on {type_name}/{field} invokes: expected it still \
             readable, got {read:?}"
        );

        // A function no resolver invokes is deleted, so what blocks the delete is the
        // reference and not the API holding any resolver at all.
        let unused = create_functions(&router, &api_id, DATA_SOURCE, 1).await;
        let answer = delete_function(&router, &api_id, &unused[0]).await;
        assert_empty_ok("a function no resolver invokes", &answer);
    }
}

/// Rule 3: once the reference is gone each delete is served, so a teardown runs to the
/// end — detach the resolver, delete the function, delete the data source.
#[tokio::test]
async fn a_teardown_runs_to_the_end_once_each_reference_is_gone() {
    let router = router();
    let api_id = create_api(&router, "deps-teardown").await;
    create_data_source(&router, &api_id, DATA_SOURCE).await;
    let function_ids = create_functions(&router, &api_id, DATA_SOURCE, 2).await;
    let field = "getItem";
    let answer = create_resolver(
        &router,
        &api_id,
        TYPES[0],
        pipeline_body(field, &function_ids),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case attach the pipeline resolver: expected 200, got {answer:?}"
    );

    let answer = delete_resolver(&router, &api_id, TYPES[0], field).await;
    assert_empty_ok("detach the resolver", &answer);

    // The resolver is gone, so the field carries none.
    let read = get_resolver(&router, &api_id, TYPES[0], field).await;
    assert_eq!(
        (read.status, read.message.as_str()),
        (404, state::RESOLVER_NOT_FOUND_MESSAGE),
        "case read the detached resolver: expected 404 {:?}, got {read:?}",
        state::RESOLVER_NOT_FOUND_MESSAGE
    );

    // Nothing invokes the functions now, so each is deleted and unreadable afterwards.
    for function_id in &function_ids {
        let answer = delete_function(&router, &api_id, function_id).await;
        assert_empty_ok(&format!("delete the function {function_id}"), &answer);
        let read = call(
            &router,
            "GET",
            &format!("/v1/apis/{api_id}/functions/{function_id}"),
            None,
        )
        .await;
        assert_eq!(
            read.status, 404,
            "case read the deleted function {function_id}: expected 404, got {read:?}"
        );
    }

    // And nothing names the data source now, so it is deleted too.
    let answer = delete_data_source(&router, &api_id, DATA_SOURCE).await;
    assert_empty_ok("delete the data source", &answer);
    let read = call(
        &router,
        "GET",
        &format!("/v1/apis/{api_id}/datasources/{DATA_SOURCE}"),
        None,
    )
    .await;
    assert_eq!(
        read.status, 404,
        "case read the deleted data source: expected 404, got {read:?}"
    );

    // A second delete of either finds nothing left to delete.
    let answer = delete_data_source(&router, &api_id, DATA_SOURCE).await;
    assert_eq!(
        answer.status, 404,
        "case delete the data source twice: expected 404, got {answer:?}"
    );
    let answer = delete_function(&router, &api_id, &function_ids[0]).await;
    assert_eq!(
        answer.status, 404,
        "case delete the function twice: expected 404, got {answer:?}"
    );
}

/// One row of the update table: the resolver a `CreateResolver` attaches, and the
/// `UpdateResolver` body that restates it.
#[derive(Debug)]
struct UpdateRow {
    /// What the row exercises, named in every assertion of it.
    label: &'static str,
    field: &'static str,
    /// Whether the create attaches a `PIPELINE` resolver (a `UNIT` one otherwise).
    created_pipeline: bool,
    /// Whether the update asks for a `PIPELINE` resolver.
    updated_pipeline: bool,
    request_template: Option<&'static str>,
    response_template: Option<&'static str>,
    code: Option<&'static str>,
    /// Whether the update names the kind at all: a request that names none asks for a
    /// `UNIT` resolver, "the default resolver type" (`Resolver$kind`).
    names_kind: bool,
}

/// The update table: every form an update can restate a resolver in.
///
/// The rows differ along the kind the resolver was of and the kind it is updated to (so
/// both directions of the UNIT/PIPELINE transition are exercised), along what the update
/// carries (templates, `code`) and along whether it names a kind at all.
fn update_rows() -> Vec<UpdateRow> {
    vec![
        UpdateRow {
            label: "a UNIT resolver updated to a PIPELINE one",
            field: "unitToPipeline",
            created_pipeline: false,
            updated_pipeline: true,
            request_template: Some("{\"version\": \"2018-05-29\"}"),
            response_template: Some("$util.toJson($ctx.result)"),
            code: None,
            names_kind: true,
        },
        UpdateRow {
            label: "a PIPELINE resolver updated in place",
            field: "pipelineInPlace",
            created_pipeline: true,
            updated_pipeline: true,
            request_template: Some("## updated héllo 世界 🎉 \"quoted\" \\backslash\\ Ω≈ç√"),
            response_template: Some("$util.toJson($ctx.result) ## ✅"),
            code: None,
            names_kind: true,
        },
        UpdateRow {
            label: "a PIPELINE resolver updated to a UNIT one",
            field: "pipelineToUnit",
            created_pipeline: true,
            updated_pipeline: false,
            request_template: Some("{\"version\": \"2018-05-29\", \"payload\": {}}"),
            response_template: None,
            code: None,
            names_kind: true,
        },
        UpdateRow {
            label: "a UNIT resolver updated with code and no kind named",
            field: "unitWithCode",
            created_pipeline: false,
            updated_pipeline: false,
            request_template: None,
            response_template: None,
            code: Some("export function request(ctx) { return { payload: ctx.args }; }"),
            names_kind: false,
        },
    ]
}

/// The `UpdateResolver` body `row` asks for.
fn update_body(row: &UpdateRow, function_ids: &[String]) -> Value {
    let mut body = json!({});
    let object = body.as_object_mut().expect("the body is an object");
    if row.names_kind {
        let kind = if row.updated_pipeline {
            state::PIPELINE_KIND
        } else {
            state::UNIT_KIND
        };
        object.insert("kind".to_string(), json!(kind));
    }
    if row.updated_pipeline {
        object.insert(
            "pipelineConfig".to_string(),
            json!({"functions": function_ids}),
        );
    } else {
        object.insert("dataSourceName".to_string(), json!(DATA_SOURCE));
    }
    if let Some(template) = row.request_template {
        object.insert("requestMappingTemplate".to_string(), json!(template));
    }
    if let Some(template) = row.response_template {
        object.insert("responseMappingTemplate".to_string(), json!(template));
    }
    if let Some(code) = row.code {
        object.insert("code".to_string(), json!(code));
        // "When code is used, the runtime is required" (`Resolver$code`).
        object.insert(
            "runtime".to_string(),
            json!({"name": RUNTIME.0, "runtimeVersion": RUNTIME.1}),
        );
    }
    body
}

/// Assert `resolver` is the resolver `row`'s update asked for.
fn assert_updated(row: &UpdateRow, resolver: &Value, function_ids: &[String], source: &str) {
    let case = format!("{} ({source})", row.label);
    let expected_kind = if row.updated_pipeline {
        state::PIPELINE_KIND
    } else {
        state::UNIT_KIND
    };
    assert_eq!(
        resolver["kind"].as_str(),
        Some(expected_kind),
        "case {case:?}: expected kind {expected_kind:?}, got {resolver:?}"
    );
    if row.updated_pipeline {
        let functions: Vec<&str> = resolver["pipelineConfig"]["functions"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| item.as_str().unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default();
        let expected: Vec<&str> = function_ids.iter().map(String::as_str).collect();
        assert_eq!(
            functions, expected,
            "case {case:?}: expected the pipeline {expected:?}, got {resolver:?}"
        );
        // A PIPELINE resolver has no data source of its own, so a member the update left
        // out is absent from the answer.
        assert!(
            resolver["dataSourceName"].is_null(),
            "case {case:?}: expected no dataSourceName, got {resolver:?}"
        );
    } else {
        assert_eq!(
            resolver["dataSourceName"].as_str(),
            Some(DATA_SOURCE),
            "case {case:?}: expected the data source {DATA_SOURCE:?}, got {resolver:?}"
        );
        assert!(
            resolver["pipelineConfig"].is_null(),
            "case {case:?}: expected no pipelineConfig, got {resolver:?}"
        );
    }
    assert_eq!(
        resolver["requestMappingTemplate"].as_str(),
        row.request_template,
        "case {case:?}: expected the request template the update carried, got {resolver:?}"
    );
    assert_eq!(
        resolver["responseMappingTemplate"].as_str(),
        row.response_template,
        "case {case:?}: expected the response template the update carried, got {resolver:?}"
    );
    assert_eq!(
        resolver["code"].as_str(),
        row.code,
        "case {case:?}: expected the code the update carried, got {resolver:?}"
    );
}

/// Rule 4: an update restates the resolver and answers the stored record, which
/// `GetResolver` answers with afterwards.
#[tokio::test]
async fn an_update_restates_the_resolver_and_is_read_back() {
    let router = router();
    let api_id = create_api(&router, "deps-update").await;
    create_data_source(&router, &api_id, DATA_SOURCE).await;
    let function_ids = create_functions(&router, &api_id, DATA_SOURCE, 1).await;

    for row in update_rows() {
        // The resolver the update changes: attached as the row says, with templates the
        // update must replace.
        let mut created = json!({
            "fieldName": row.field,
            "requestMappingTemplate": "{\"version\": \"2018-05-29\", \"created\": true}",
            "responseMappingTemplate": "$util.toJson($ctx.prev.result)",
        });
        let object = created.as_object_mut().expect("the body is an object");
        if row.created_pipeline {
            object.insert("kind".to_string(), json!(state::PIPELINE_KIND));
            object.insert(
                "pipelineConfig".to_string(),
                json!({"functions": function_ids}),
            );
        } else {
            object.insert("dataSourceName".to_string(), json!(DATA_SOURCE));
        }
        let answer = create_resolver(&router, &api_id, TYPES[0], created).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected the create to answer 200, got {answer:?}",
            row.label
        );

        let answer = update_resolver(
            &router,
            &api_id,
            TYPES[0],
            row.field,
            update_body(&row, &function_ids),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected the update to answer 200, got {answer:?}",
            row.label
        );
        assert_updated(&row, &answer.body["resolver"], &function_ids, "the update");

        // The ARN is the one the create named: an update changes the resolver's
        // definition, not which field it is attached to.
        let read = get_resolver(&router, &api_id, TYPES[0], row.field).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected the read to answer 200, got {read:?}",
            row.label
        );
        assert_updated(&row, &read.body["resolver"], &function_ids, "the read");
        assert_eq!(
            read.body["resolver"]["resolverArn"],
            answer.body["resolver"]["resolverArn"],
            "case {:?}: expected the stored ARN to be the one the update answered, got {read:?}",
            row.label
        );
    }
}

/// Rule 5: an update AppSync could not run, and one addressing a field that carries no
/// resolver, are refused and change nothing.
#[tokio::test]
async fn an_update_appsync_cannot_run_is_refused_and_changes_nothing() {
    let router = router();
    let api_id = create_api(&router, "deps-update-refusals").await;
    create_data_source(&router, &api_id, DATA_SOURCE).await;
    let field = "refused";
    let answer = create_resolver(
        &router,
        &api_id,
        TYPES[0],
        json!({
            "fieldName": field,
            "dataSourceName": DATA_SOURCE,
            "requestMappingTemplate": "{\"version\": \"2018-05-29\"}",
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case attach the resolver the refusals leave alone: expected 200, got {answer:?}"
    );
    let stored = answer.body["resolver"].clone();

    // The rows differ along which member of the definition the update got wrong.
    let cases: &[(&str, Value, &str)] = &[
        (
            "a PIPELINE update carrying no pipelineConfig",
            json!({"kind": state::PIPELINE_KIND}),
            state::PIPELINE_CONFIG_REQUIRED_MESSAGE,
        ),
        (
            "a UNIT update carrying a pipelineConfig",
            json!({
                "kind": state::UNIT_KIND,
                "dataSourceName": DATA_SOURCE,
                "pipelineConfig": {"functions": []},
            }),
            state::PIPELINE_CONFIG_NOT_ALLOWED_MESSAGE,
        ),
        (
            "a UNIT update naming no data source",
            json!({"kind": state::UNIT_KIND}),
            state::DATA_SOURCE_NAME_MISSING_MESSAGE,
        ),
        (
            "an update carrying code without a runtime",
            json!({
                "dataSourceName": DATA_SOURCE,
                "code": "export function request(ctx) { return {}; }",
            }),
            "Invalid runtime given. A runtime is required when code is specified.",
        ),
        (
            "an update carrying an empty request template",
            json!({"dataSourceName": DATA_SOURCE, "requestMappingTemplate": ""}),
            "Invalid requestMappingTemplate given. Must be between 1 and 65536 characters long.",
        ),
    ];

    for (case, body, message) in cases {
        let answer = update_resolver(&router, &api_id, TYPES[0], field, body.clone()).await;
        assert_eq!(
            (
                answer.status,
                answer.error_type.as_str(),
                answer.message.as_str()
            ),
            (400, "BadRequestException", *message),
            "case {case:?}: expected 400 BadRequestException {message:?}, got {answer:?}"
        );
        let read = get_resolver(&router, &api_id, TYPES[0], field).await;
        assert_eq!(
            (read.status, &read.body["resolver"]),
            (200, &stored),
            "case {case:?}: expected the stored resolver unchanged, got {read:?}"
        );
    }

    // A field that carries no resolver is not updated into one: the update attaches
    // nothing, the way only a create attaches a resolver.
    let answer = update_resolver(
        &router,
        &api_id,
        TYPES[1],
        field,
        json!({"dataSourceName": DATA_SOURCE}),
    )
    .await;
    assert_eq!(
        (
            answer.status,
            answer.error_type.as_str(),
            answer.message.as_str()
        ),
        (404, "NotFoundException", state::RESOLVER_NOT_FOUND_MESSAGE),
        "case an update of a field carrying no resolver: expected 404 NotFoundException {:?}, \
         got {answer:?}",
        state::RESOLVER_NOT_FOUND_MESSAGE
    );
    let read = get_resolver(&router, &api_id, TYPES[1], field).await;
    assert_eq!(
        read.status, 404,
        "case an update of a field carrying no resolver: expected the field still to carry \
         none, got {read:?}"
    );
}
