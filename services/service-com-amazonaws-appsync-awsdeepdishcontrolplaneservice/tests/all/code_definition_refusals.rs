// SPDX-License-Identifier: CC0-1.0

//! The APPSYNC_JS `code` a resolver or a function is defined by, end to end over HTTP:
//! what a call that would store code the runtime cannot read answers, and what the API's
//! inventory holds afterwards.
//!
//! The rules asserted here are stated over CLASSES of input rather than over the one
//! request that exposed the defect:
//!
//! 1. a call that would store `code` that does not compile is refused: 400
//!    `BadRequestException` with the `CODE_ERROR` reason, AppSync's fixed
//!    code-errors sentence and a `detail.codeErrors` list carrying one entry per fault,
//!    each with its `errorType`, the compiler's own diagnostic as its `value` and a
//!    `location`; asserted over every operation that stores code (`CreateFunction`,
//!    `UpdateFunction`, `CreateResolver`, `UpdateResolver`) and over several ways for a
//!    definition not to compile;
//! 2. a refused call stores NOTHING: the function is not in the API's inventory, the
//!    field carries no resolver, and an update leaves the stored definition as it was;
//! 3. code the compiler reads to the end is stored and read back verbatim — including
//!    code importing the `util` helpers AppSync's own resolvers import, code carrying
//!    non-ASCII text, and code defining both handlers;
//! 4. the inventory a listing reports is exactly the definitions that were accepted,
//!    each under its own id, with every member the model projects;
//! 5. the compile is BOUNDED: a definition that nests as deeply as the modelled maximum
//!    length of `code` allows is answered with that same refusal, because the compile runs
//!    under the engine's own memory, stack, clock and step ceilings.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization, the boundary validator, the handlers
//! and the shared engine together. The harness (`call`, `create_api`, `router`, `Answer`)
//! is the error-contract suite's.

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

use super::error_contract::{call, create_api, router, Answer};
// The data source every definition under test resolves against is created through the
// function suite's own helper rather than a second copy of it.
use super::functions::{create_data_source, RUNTIME};

/// The data source the definitions under test resolve against.
const DATA_SOURCE: &str = "ds_code";

/// The type the resolvers under test are attached to.
const TYPE: &str = "Query";

/// Code the compiler reads to the end, in the forms a real APPSYNC_JS definition takes:
/// the `util` helpers AppSync's own resolvers import
/// (`@aws-appsync/utils`, the one specifier the APPSYNC_JS runtime resolves), both
/// handlers, non-ASCII text in a literal, and the smallest handler there is.
const COMPILING_CODE: &[(&str, &str)] = &[
    ("the smallest handler", "export function request(ctx) { return {}; }"),
    (
        "both handlers",
        "export function request(ctx) { return { payload: ctx.args }; }\n\
         export function response(ctx) { return ctx.result; }\n",
    ),
    (
        "a handler importing the util helpers",
        "import { util } from '@aws-appsync/utils';\n\
         export function request(ctx) { return { id: util.autoId() }; }\n",
    ),
    (
        "a handler whose literals carry non-ASCII text",
        "export function request(ctx) {\n  // héllo 世界 🎉\n  return { greeting: 'Ω≈ç√' };\n}\n",
    ),
];

/// Code the engine cannot read to the end, in several forms, each with the position the
/// compile stops at: the line and the column just past the code's last character, counted
/// from one, which is as far as a compile that could not read the code got. (The engine
/// states no position of its own for a module it could not compile, so this is the one the
/// diagnostic carries — see `crate::state::code_definition`.)
const BROKEN_CODE: &[(&str, &str, i64, i64)] = &[
    (
        "a handler missing its closing brace",
        "export function request(ctx) {\n  return {\n    payload: ctx.args,\n",
        // Three lines, the last of them terminated: the next character would start line 4.
        4,
        1,
    ),
    (
        "an unterminated string literal",
        "export function request(ctx) { return { payload: 'unterminated }; }",
        1,
        68,
    ),
    (
        "a stray parenthesis",
        "export function request(ctx) { return (((( }",
        1,
        45,
    ),
    (
        "text that is not JavaScript at all",
        "this is not javascript at all",
        1,
        30,
    ),
];

/// A `CreateFunction` / `UpdateFunction` body defining the function `name` by `code`.
fn function_body(name: &str, code: &str) -> Value {
    json!({
        "name": name,
        "dataSourceName": DATA_SOURCE,
        "description": "a function defined by code",
        "code": code,
        "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
    })
}

/// A `CreateResolver` / `UpdateResolver` body defining the resolver on `field` by `code`.
fn resolver_body(field: &str, code: &str) -> Value {
    json!({
        "fieldName": field,
        "dataSourceName": DATA_SOURCE,
        "code": code,
        "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
    })
}

/// Send one `CreateFunction` defining `name` by `code`.
async fn create_function(
    router: &MultiServiceRouter,
    api_id: &str,
    name: &str,
    code: &str,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/functions"),
        Some(function_body(name, code)),
    )
    .await
}

/// Send one `CreateResolver` defining the resolver on `field` by `code`.
async fn create_resolver(
    router: &MultiServiceRouter,
    api_id: &str,
    field: &str,
    code: &str,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/{TYPE}/resolvers"),
        Some(resolver_body(field, code)),
    )
    .await
}

/// List the functions of `api_id`.
async fn list_functions(router: &MultiServiceRouter, api_id: &str) -> Answer {
    call(router, "GET", &format!("/v1/apis/{api_id}/functions"), None).await
}

/// Assert `answer` is the refusal code the engine could not read takes, and — where the
/// row states one — that it is reported at `position`, the line and column the compile
/// stopped at.
fn assert_code_error(case: &str, answer: &Answer, position: Option<(i64, i64)>) {
    assert_eq!(
        (
            answer.status,
            answer.error_type.as_str(),
            answer.message.as_str(),
            answer.body["reason"].as_str(),
        ),
        (
            400,
            "BadRequestException",
            state::CODE_ERRORS_MESSAGE,
            Some(state::CODE_ERROR_REASON),
        ),
        "case {case:?}: expected 400 BadRequestException {:?} with the {:?} reason, got \
         {answer:?}",
        state::CODE_ERRORS_MESSAGE,
        state::CODE_ERROR_REASON
    );

    let faults = answer.body["detail"]["codeErrors"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !faults.is_empty(),
        "case {case:?}: expected at least one code error in the detail, got {answer:?}"
    );
    for fault in &faults {
        assert_eq!(
            fault["errorType"].as_str(),
            Some(state::PARSE_ERROR),
            "case {case:?}: expected the parse errorType, got {fault:?}"
        );

        // The diagnostic is one line, worded the way a compiler words one: the unit the
        // code is compiled under, the position in it, and the sentence — and the position
        // it names is the one the `location` carries, so the two cannot disagree about
        // where in the code the compile stopped.
        let line = fault["location"]["line"].as_i64().unwrap_or_default();
        let column = fault["location"]["column"].as_i64().unwrap_or_default();
        let value = fault["value"].as_str().unwrap_or_default();
        if let Some((expected_line, expected_column)) = position {
            assert_eq!(
                (line, column),
                (expected_line, expected_column),
                "case {case:?}: expected the compile to stop at line {expected_line} column \
                 {expected_column}, got {fault:?}"
            );
        }
        let stated = format!("code.js({line},{column}): error");
        assert!(
            value.starts_with(&stated) && value.ends_with('\n'),
            "case {case:?}: expected a diagnostic stating {stated:?}, got {value:?}"
        );
        assert!(
            line >= 1 && column >= 1,
            "case {case:?}: expected the position the compile stopped at, got {fault:?}"
        );
        assert_eq!(
            fault["location"]["span"].as_i64(),
            Some(0),
            "case {case:?}: expected the span of a diagnostic about a position, got {fault:?}"
        );
    }
}

/// How one row of the nesting table builds its code: a construct repeated to the depth the
/// row asks for.
type NestedCode = fn(usize) -> String;

/// Rule 5 (the bound): a definition whose `code` nests as deeply as the modelled maximum
/// length allows is ANSWERED — either stored, where the code reads, or refused with the
/// code-error detail — because the compile is held to the engine's own memory, stack,
/// clock and step ceilings rather than to the host's.
///
/// The rows differ along the construct that nests and along the depth, up to the deepest
/// one `Code`'s modelled `@length` (1..32768, from
/// `smithy_query(query="shape", shape="Code")`) can describe: one character per level.
#[tokio::test]
async fn code_nested_as_deeply_as_the_modelled_length_allows_is_answered() {
    let router = router();
    let api_id = create_api(&router, "code-nested").await;
    create_data_source(&router, &api_id, DATA_SOURCE).await;

    let forms: &[(&str, NestedCode)] = &[
        ("nested parentheses", |depth| "(".repeat(depth)),
        ("nested array literals", |depth| "[".repeat(depth)),
        ("nested blocks", |depth| "{".repeat(depth)),
        ("stacked prefix operators", |depth| {
            format!("{}x", "!".repeat(depth - 1))
        }),
    ];

    let mut stored = 0;
    for (form, build) in forms {
        for depth in [2_usize, 1_000, 32_768] {
            let code = build(depth);
            let case = format!("{form} {depth} deep");
            let answer = create_function(&router, &api_id, "fn_nested", &code).await;
            if answer.status == 200 {
                // Code the engine read to the end is stored, however deep it nests.
                stored += 1;
                continue;
            }
            assert_code_error(&case, &answer, None);
        }
    }

    // The emulator served every one of them, and its inventory holds exactly the ones it
    // accepted — so a pathological definition neither stops the service nor leaves
    // anything behind.
    let listed = list_functions(&router, &api_id).await;
    let functions = listed.body["functions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        functions.len(),
        stored,
        "case the inventory after the nested definitions: expected the {stored} that were \
         accepted, got {listed:?}"
    );
}

/// Rules 1 and 2: a create whose `code` does not compile is refused with the code-error
/// detail and stores nothing.
#[tokio::test]
async fn a_create_whose_code_does_not_compile_is_refused_and_stores_nothing() {
    for (index, (case, code, line, column)) in BROKEN_CODE.iter().enumerate() {
        let router = router();
        let api_id = create_api(&router, &format!("code-create-{index}")).await;
        create_data_source(&router, &api_id, DATA_SOURCE).await;

        let answer = create_function(&router, &api_id, "fn_bad", code).await;
        assert_code_error(
            &format!("CreateFunction with {case}"),
            &answer,
            Some((*line, *column)),
        );

        // Nothing was stored for the refused create, so the API's inventory is empty.
        let listed = list_functions(&router, &api_id).await;
        let functions = listed.body["functions"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            functions.is_empty(),
            "case CreateFunction with {case:?}: expected no function in the inventory, got \
             {listed:?}"
        );

        let field = "badCode";
        let answer = create_resolver(&router, &api_id, field, code).await;
        assert_code_error(
            &format!("CreateResolver with {case}"),
            &answer,
            Some((*line, *column)),
        );

        // And the field carries no resolver.
        let read = call(
            &router,
            "GET",
            &format!("/v1/apis/{api_id}/types/{TYPE}/resolvers/{field}"),
            None,
        )
        .await;
        assert_eq!(
            read.status, 404,
            "case CreateResolver with {case:?}: expected the field to carry no resolver, got \
             {read:?}"
        );
    }
}

/// Rules 1 and 2: an update whose `code` does not compile is refused the same way and
/// leaves the stored definition exactly as it was.
#[tokio::test]
async fn an_update_whose_code_does_not_compile_is_refused_and_changes_nothing() {
    let router = router();
    let api_id = create_api(&router, "code-update").await;
    create_data_source(&router, &api_id, DATA_SOURCE).await;

    let good = COMPILING_CODE[0].1;
    let created = create_function(&router, &api_id, "fn_ok", good).await;
    assert_eq!(
        created.status, 200,
        "case create the function the updates leave alone: expected 200, got {created:?}"
    );
    let function_id = created.body["functionConfiguration"]["functionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let field = "okCode";
    let attached = create_resolver(&router, &api_id, field, good).await;
    assert_eq!(
        attached.status, 200,
        "case attach the resolver the updates leave alone: expected 200, got {attached:?}"
    );

    for (case, code, line, column) in BROKEN_CODE {
        let answer = call(
            &router,
            "POST",
            &format!("/v1/apis/{api_id}/functions/{function_id}"),
            Some(function_body("fn_ok", code)),
        )
        .await;
        assert_code_error(
            &format!("UpdateFunction with {case}"),
            &answer,
            Some((*line, *column)),
        );
        let read = call(
            &router,
            "GET",
            &format!("/v1/apis/{api_id}/functions/{function_id}"),
            None,
        )
        .await;
        assert_eq!(
            (
                read.status,
                read.body["functionConfiguration"]["code"].as_str()
            ),
            (200, Some(good)),
            "case UpdateFunction with {case:?}: expected the stored code unchanged, got {read:?}"
        );

        let answer = call(
            &router,
            "POST",
            &format!("/v1/apis/{api_id}/types/{TYPE}/resolvers/{field}"),
            Some(resolver_body(field, code)),
        )
        .await;
        assert_code_error(
            &format!("UpdateResolver with {case}"),
            &answer,
            Some((*line, *column)),
        );
        let read = call(
            &router,
            "GET",
            &format!("/v1/apis/{api_id}/types/{TYPE}/resolvers/{field}"),
            None,
        )
        .await;
        assert_eq!(
            (read.status, read.body["resolver"]["code"].as_str()),
            (200, Some(good)),
            "case UpdateResolver with {case:?}: expected the stored code unchanged, got {read:?}"
        );
    }
}

/// Rule 3: code the compiler reads to the end is stored and read back verbatim.
#[tokio::test]
async fn code_that_compiles_is_stored_and_read_back() {
    let router = router();
    let api_id = create_api(&router, "code-accepted").await;
    create_data_source(&router, &api_id, DATA_SOURCE).await;

    for (index, (case, code)) in COMPILING_CODE.iter().enumerate() {
        let answer = create_function(&router, &api_id, "fn_ok", code).await;
        assert_eq!(
            (
                answer.status,
                answer.body["functionConfiguration"]["code"].as_str()
            ),
            (200, Some(*code)),
            "case CreateFunction with {case:?}: expected 200 echoing the code, got {answer:?}"
        );
        let function_id = answer.body["functionConfiguration"]["functionId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let read = call(
            &router,
            "GET",
            &format!("/v1/apis/{api_id}/functions/{function_id}"),
            None,
        )
        .await;
        assert_eq!(
            (
                read.status,
                read.body["functionConfiguration"]["code"].as_str()
            ),
            (200, Some(*code)),
            "case CreateFunction with {case:?}: expected the stored code read back, got {read:?}"
        );

        let field = format!("okCode{index}");
        let answer = create_resolver(&router, &api_id, &field, code).await;
        assert_eq!(
            (answer.status, answer.body["resolver"]["code"].as_str()),
            (200, Some(*code)),
            "case CreateResolver with {case:?}: expected 200 echoing the code, got {answer:?}"
        );
        let read = call(
            &router,
            "GET",
            &format!("/v1/apis/{api_id}/types/{TYPE}/resolvers/{field}"),
            None,
        )
        .await;
        assert_eq!(
            (read.status, read.body["resolver"]["code"].as_str()),
            (200, Some(*code)),
            "case CreateResolver with {case:?}: expected the stored code read back, got {read:?}"
        );
    }
}

/// Rule 4: the inventory holds exactly the functions that were accepted, each under its
/// own id, with every member the model projects.
#[tokio::test]
async fn the_inventory_holds_exactly_the_functions_that_were_accepted() {
    let router = router();
    let api_id = create_api(&router, "code-inventory").await;
    create_data_source(&router, &api_id, DATA_SOURCE).await;

    // Two refused creates and one accepted one, in the order a client would make them.
    for (case, code, line, column) in BROKEN_CODE.iter().take(2) {
        let answer = create_function(&router, &api_id, "fn_bad", code).await;
        assert_code_error(
            &format!("CreateFunction with {case}"),
            &answer,
            Some((*line, *column)),
        );
    }
    let accepted = create_function(&router, &api_id, "fn_ok", COMPILING_CODE[1].1).await;
    assert_eq!(
        accepted.status, 200,
        "case the accepted create: expected 200, got {accepted:?}"
    );
    let function_id = accepted.body["functionConfiguration"]["functionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let listed = list_functions(&router, &api_id).await;
    let functions = listed.body["functions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        functions.len(),
        1,
        "case the inventory after two refusals and one accepted create: expected exactly the \
         accepted function, got {listed:?}"
    );
    let listed_function = &functions[0];
    assert_eq!(
        (
            listed_function["functionId"].as_str(),
            listed_function["name"].as_str(),
            listed_function["code"].as_str(),
            listed_function["dataSourceName"].as_str(),
            listed_function["description"].as_str(),
        ),
        (
            Some(function_id.as_str()),
            Some("fn_ok"),
            Some(COMPILING_CODE[1].1),
            Some(DATA_SOURCE),
            Some("a function defined by code"),
        ),
        "case the inventory after two refusals and one accepted create: expected the accepted \
         function's own projection, got {listed_function:?}"
    );
    assert_eq!(
        listed_function, &accepted.body["functionConfiguration"],
        "case the inventory after two refusals and one accepted create: expected the listing to \
         report what the create answered, got {listed_function:?}"
    );
}
