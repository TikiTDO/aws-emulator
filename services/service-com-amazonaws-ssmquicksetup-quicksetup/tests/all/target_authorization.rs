// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the request-authorization stage that fronts this service's
//! ARN-labelled routes.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rule under test: EVERY route that carries a resource ARN as a path label — the
//! manager's own `/configurationManager/{ManagerArn}`, the tag plane's
//! `/tags/{ResourceArn}`, whose label is any taggable resource's ARN, and
//! `/configurationDefinition/{ManagerArn}/{Id}`, whose label is the addressed definition's
//! PARENT — is authorized against that ARN before the request reaches the service, and an
//! ARN that names no resource the endpoint serves is refused there: HTTP 401, the wire
//! error code `UnauthorizedException`, and the one fixed body member `message` =
//! `Unauthorized`. Four consequences are what the rows below span:
//!
//! * it precedes the modelled constraint walk, so a label outside its own `@pattern` —
//!   and a label no `@pattern` bounds at all — earns the same refusal rather than a
//!   `ValidationException`;
//! * it precedes the handler, so an unauthorized target earns it rather than the handler's
//!   modelled `ResourceNotFoundException`, whatever else the request carries (a body the
//!   handler would refuse, a tag key the service reserves);
//! * it fronts the ARN-labelled routes only: `GetConfiguration`'s label is a configuration
//!   id rather than an ARN, so that route names no ARN to authorize;
//! * it judges a ROUTE's label only, so an ARN carried in the request BODY is resolved by
//!   the handler, in this service's own modelled shapes.
//!
//! The ARNs below are examples of "an ARN this endpoint serves no resource for"; nothing
//! about any one of them is part of the rule, which is why the rows differ in every part
//! an ARN has (Region, account, uuid, case, form) and include values that are not ARNs at
//! all.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::{
    QUICK_SETUP_TYPE_CATALOG, TRANSITION_SPAN,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for.
const REGION: &str = "us-west-2";

/// The status the authorization stage refuses on.
const UNAUTHORIZED_STATUS: u16 = 401;

/// The wire error code it carries — a shape this service's model does not declare, which
/// is what tells a client the refusal came from in front of the service.
const UNAUTHORIZED_CODE: &str = "UnauthorizedException";

/// The whole sentence it states, for every request it refuses.
const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ssmquicksetup_quicksetup::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `ssm-quicksetup` in `region`, the signal the router routes on.
fn request(
    method: &str,
    uri: &str,
    region: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("ssm-quicksetup.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDQUICKSETUP/20260101/{region}/ssm-quicksetup/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call in `region`. An empty body reads as `Value::Null`.
async fn call_in(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    region: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, region, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// [`call_in`], in [`REGION`].
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    call_in(router, method, uri, REGION, body).await
}

/// The error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.ssmquicksetup#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The message a refusal carried, read through either casing restJson1 may render an
/// error structure's single message member under — the point of several rows below being
/// that the stage uses the lowercase one and the service's own shapes do not.
fn error_message(body: &Value) -> String {
    ["Message", "message"]
        .iter()
        .filter_map(|key| body.get(*key))
        .filter_map(Value::as_str)
        .next()
        .unwrap_or("<no message>")
        .to_string()
}

/// Percent-encode a value for use as one `@httpLabel` path segment, which is what an SDK
/// does: a non-greedy label may not carry a raw `/`.
fn label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            ' ' => "%20".to_string(),
            '?' => "%3F".to_string(),
            '#' => "%23".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// Every operation the stage fronts: the three bound to `/configurationManager/
/// {ManagerArn}`, whose label is the manager's own ARN, the three bound to
/// `/tags/{ResourceArn}`, whose label is any taggable resource's ARN, and the one bound to
/// `/configurationDefinition/{ManagerArn}/{Id}`, whose label is the ARN of the manager
/// that OWNS the definition it addresses (`smithy_query(query="digest")`).
const ARN_LABELLED_OPERATIONS: &[&str] = &[
    "GetConfigurationManager",
    "UpdateConfigurationManager",
    "DeleteConfigurationManager",
    "UpdateConfigurationDefinition",
    "ListTagsForResource",
    "TagResource",
    "UntagResource",
];

/// Drive one operation of either list against `arn` in `region`, on the route and with
/// the payload the model binds for it (`OPERATIONS.md`).
async fn call_naming_in(
    router: &MultiServiceRouter,
    operation: &str,
    arn: &str,
    definition_id: &str,
    region: &str,
) -> (u16, Value) {
    let manager = format!("/configurationManager/{}", label(arn));
    let tags = format!("/tags/{}", label(arn));
    match operation {
        "GetConfigurationManager" => call_in(router, "GET", &manager, region, None).await,
        "DeleteConfigurationManager" => call_in(router, "DELETE", &manager, region, None).await,
        "UpdateConfigurationManager" => {
            call_in(
                router,
                "PUT",
                &manager,
                region,
                Some(&json!({ "Name": "renamed" })),
            )
            .await
        }
        "UpdateConfigurationDefinition" => {
            call_in(
                router,
                "PUT",
                &format!(
                    "/configurationDefinition/{}/{}",
                    label(arn),
                    label(definition_id)
                ),
                region,
                Some(&json!({ "TypeVersion": "LATEST" })),
            )
            .await
        }
        "ListTagsForResource" => call_in(router, "GET", &tags, region, None).await,
        "TagResource" => {
            call_in(
                router,
                "PUT",
                &tags,
                region,
                Some(&json!({ "Tags": { "Env": "prod" } })),
            )
            .await
        }
        "UntagResource" => {
            call_in(
                router,
                "DELETE",
                &format!("{tags}?tagKeys=Env"),
                region,
                None,
            )
            .await
        }
        other => panic!("no route is wired for operation {other:?}"),
    }
}

/// [`call_naming_in`], in [`REGION`].
async fn call_naming(
    router: &MultiServiceRouter,
    operation: &str,
    arn: &str,
    definition_id: &str,
) -> (u16, Value) {
    call_naming_in(router, operation, arn, definition_id, REGION).await
}

/// Assert one authorization refusal: the status, the wire error code, and the whole fixed
/// sentence under the lowercase `message` member.
fn assert_unauthorized(case: &str, status: u16, answer: &Value) {
    assert_eq!(
        status, UNAUTHORIZED_STATUS,
        "case {case:?}: expected {UNAUTHORIZED_STATUS}, got {status} / {answer:?}"
    );
    let code = error_code(answer);
    assert_eq!(
        code, UNAUTHORIZED_CODE,
        "case {case:?}: expected {UNAUTHORIZED_CODE}, got {code:?} in {answer:?}"
    );
    let message = error_message(answer);
    assert_eq!(
        message, UNAUTHORIZED_MESSAGE,
        "case {case:?}: expected {UNAUTHORIZED_MESSAGE:?}, got {message:?}"
    );
    // The sentence travels under the lowercase member the stage names, not under the
    // `Message` this service's own modelled errors carry.
    assert_eq!(
        answer.get("message").and_then(Value::as_str),
        Some(UNAUTHORIZED_MESSAGE),
        "case {case:?}: expected the sentence under the lowercase `message`, got {answer:?}"
    );
    assert_eq!(
        answer.get("Message"),
        None,
        "case {case:?}: the stage carries no `Message` member, got {answer:?}"
    );
}

/// Create one configuration manager, answering its ARN.
async fn create_manager(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    // `Type` must name a configuration type this service publishes; no case here is about
    // the type, so every create uses the catalog's first entry. Its parameters name
    // somewhere to deploy to and the Region to deploy into, and provide every parameter
    // the table documented for that type marks `(Required)`, which every accepted
    // definition must.
    let type_ = QUICK_SETUP_TYPE_CATALOG[0].type_;
    let body = json!({
        "Name": name,
        "ConfigurationDefinitions": [{
            "Type": type_,
            "Parameters": super::type_parameter_fixtures::with_required_parameters(
                type_,
                json!({
                    "TargetOrganizationalUnits": "ou-1",
                    "TargetRegions": "us-east-1",
                }),
            ),
        }],
    });
    let (status, answer) = call(router, "POST", "/configurationManager", Some(&body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateConfigurationManager must succeed, got {status} / {answer:?}"
    );
    answer
        .get("ManagerArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a ManagerArn, got {answer:?}"))
        .to_string()
}

/// The id of the one definition a manager was created with.
async fn definition_id(router: &MultiServiceRouter, case: &str, manager_arn: &str) -> String {
    let (status, answer) = call(
        router,
        "GET",
        &format!("/configurationManager/{}", label(manager_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the live manager must be readable, got {status} / {answer:?}"
    );
    answer["ConfigurationDefinitions"]
        .as_array()
        .and_then(|definitions| definitions.first())
        .and_then(|definition| definition.get("Id"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a definition with an Id, got {answer:?}"))
        .to_string()
}

/// ARNs that name no resource this endpoint serves, each satisfying the modelled
/// `ManagerArn` pattern
/// `^arn:aws:ssm-quicksetup:([^:]+):(\d{12}):configuration-manager/<uuid>$`
/// (`smithy_query(query="shape",shape="GetConfigurationManagerInput")`). The rows differ
/// in Region, account id and uuid — including its hex case, which that pattern admits —
/// because none of the three is part of the rule.
const UNSERVED_ARNS: &[(&str, &str)] = &[
    (
        "a well-formed ARN in the signing Region, the all-zero account",
        "arn:aws:ssm-quicksetup:us-west-2:000000000000:configuration-manager/11111111-2222-3333-4444-555555555555",
    ),
    (
        "a well-formed ARN of another Region and another account",
        "arn:aws:ssm-quicksetup:eu-central-1:123456789012:configuration-manager/0f0e0d0c-0b0a-0908-0706-050403020100",
    ),
    (
        "an upper-case hex uuid",
        "arn:aws:ssm-quicksetup:ap-southeast-4:987654321098:configuration-manager/ABCDEF01-2345-6789-ABCD-EF0123456789",
    ),
    (
        "a mixed-case hex uuid in a third Region",
        "arn:aws:ssm-quicksetup:sa-east-1:111122223333:configuration-manager/aBcDeF01-2345-6789-aBcD-eF0123456789",
    ),
];

/// Label values the modelled `ManagerArn` pattern does NOT admit, which the stage judges
/// all the same because it runs ahead of the constraint walk — and which the tag routes'
/// `ResourceArn` (no `@pattern`, no `@length`) admits to the stage in any case.
const UNSERVED_LABELS: &[(&str, &str)] = &[
    ("not an ARN at all", "not-an-arn"),
    (
        "an ARN of another vendor",
        "arn:aws:ssm:us-west-2:123456789012:document/Example",
    ),
    (
        "this service's ARN namespace with no uuid after the resource type",
        "arn:aws:ssm-quicksetup:us-west-2:123456789012:configuration-manager/",
    ),
    (
        "a uuid one character short",
        "arn:aws:ssm-quicksetup:us-west-2:123456789012:configuration-manager/11111111-2222-3333-4444-55555555555",
    ),
    (
        "an account id that is not twelve digits",
        "arn:aws:ssm-quicksetup:us-west-2:12345:configuration-manager/11111111-2222-3333-4444-555555555555",
    ),
];

/// A definition id for the rows whose manager is not served: the target is judged first,
/// so the id only has to be a path segment.
const SOME_DEFINITION_ID: &str = "cfg-1";

/// Every route the stage fronts refuses a target the endpoint serves no resource for with
/// the authorization refusal — whatever form the value takes, including forms the
/// modelled label pattern would have refused — while a route whose label is no ARN at all
/// still resolves its own identifier.
#[tokio::test]
async fn an_unserved_target_is_refused_by_the_authorization_stage() {
    let router = emulator();
    // A manager that exists, so each refusal below is about the ARN that was sent rather
    // than about an empty store.
    let live = create_manager(&router, "a live manager", "front-door").await;

    for (target_case, target) in UNSERVED_ARNS.iter().chain(UNSERVED_LABELS) {
        for operation in ARN_LABELLED_OPERATIONS {
            let case = format!("{operation} on {target_case}");
            let (status, answer) =
                call_naming(&router, operation, target, SOME_DEFINITION_ID).await;
            assert_unauthorized(&case, status, &answer);
        }
    }

    // The other edge of the rule: a route whose label is no ARN keeps resolving that
    // label itself. `GetConfiguration`'s label is a configuration id, so an id of the
    // form this service mints that names no stored configuration is the handler's own
    // modelled refusal — and one outside that form, like one outside the label's
    // `@pattern`, is a 400 about the request's input, which only a stage in front of the
    // walk could preempt.
    let (status, answer) = call(
        &router,
        "GET",
        &format!("/getConfiguration/{}", "0".repeat(32)),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "a configuration id naming nothing stays the modelled 404, got {status} / {answer:?}"
    );
    let code = error_code(&answer);
    assert_eq!(
        code, "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {code:?} in {answer:?}"
    );
    let (status, answer) = call(
        &router,
        "GET",
        &format!("/getConfiguration/{}", label("not a configuration id")),
        None,
    )
    .await;
    assert_eq!(
        status, 400,
        "a configuration id outside its modelled pattern is the walk's 400, got {status} / \
         {answer:?}"
    );
    let code = error_code(&answer);
    assert_eq!(
        code, "ValidationException",
        "expected ValidationException, got {code:?} in {answer:?}"
    );

    // The live manager is still readable, so the refusals above were about the targets
    // they named and nothing else.
    let (status, answer) = call(
        &router,
        "GET",
        &format!("/configurationManager/{}", label(&live)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "the live manager must still be readable, got {status} / {answer:?}"
    );
}

/// The ARN of a manager that DID exist and whose teardown has come due is a target the
/// endpoint no longer serves, so every route the stage fronts refuses it the same way —
/// the stage reads the store the clock owes, not the one the delete left behind.
#[tokio::test]
async fn the_arn_of_a_torn_down_manager_is_refused_by_the_authorization_stage() {
    let router = emulator();
    let doomed = create_manager(&router, "a manager about to be deleted", "doomed").await;
    let id = definition_id(&router, "a manager about to be deleted", &doomed).await;

    // Positive control: while the manager is live, the very same calls are served by the
    // handlers behind the stage. The delete is left out of this loop only because it is
    // what the next call makes: it is driven below, and then again once the teardown is
    // due.
    for operation in ARN_LABELLED_OPERATIONS
        .iter()
        .filter(|operation| **operation != "DeleteConfigurationManager")
    {
        let case = format!("{operation} on a live manager");
        let (status, answer) = call_naming(&router, operation, &doomed, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a served target must reach the handler, got {status} / {answer:?}"
        );
    }

    let (status, answer) = call(
        &router,
        "DELETE",
        &format!("/configurationManager/{}", label(&doomed)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteConfigurationManager must be accepted, got {status} / {answer:?}"
    );
    // The delete starts an asynchronous teardown, so the manager leaves the store only
    // once that teardown comes due.
    tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(400)).await;

    for operation in ARN_LABELLED_OPERATIONS {
        let case = format!("{operation} on the ARN of a torn-down manager");
        let (status, answer) = call_naming(&router, operation, &doomed, &id).await;
        assert_unauthorized(&case, status, &answer);
    }
}

/// A manager belongs to the Region its ARN was minted in, so its own ARN is a target no
/// OTHER Region's endpoint serves — and every route the stage fronts refuses it there
/// while serving it in its own Region.
#[tokio::test]
async fn an_arn_of_another_regions_endpoint_is_refused_by_the_authorization_stage() {
    let router = emulator();
    let home = create_manager(&router, "a manager of the signing Region", "regional").await;
    let id = definition_id(&router, "a manager of the signing Region", &home).await;

    // Three other endpoints, so the refusal is not about one Region's name.
    for foreign in ["us-east-1", "eu-west-3", "ap-northeast-1"] {
        for operation in ARN_LABELLED_OPERATIONS {
            let case = format!("{operation} through the {foreign} endpoint");
            let (status, answer) =
                call_naming_in(&router, operation, &home, &id, foreign).await;
            assert_unauthorized(&case, status, &answer);
        }
    }

    // Its own endpoint still serves it, so the refusals above were about the endpoint
    // they were sent to.
    let (status, answer) = call(
        &router,
        "GET",
        &format!("/configurationManager/{}", label(&home)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "the manager's own endpoint must serve it, got {status} / {answer:?}"
    );
}

/// The stage runs ahead of everything the request SAYS: a request whose body or query the
/// service itself would refuse, and one naming a resource-level rule the service enforces,
/// are refused as unauthorized when their target is one the endpoint does not serve — and
/// the same request against a served target reaches the refusal the service owns.
#[tokio::test]
async fn the_stage_precedes_every_refusal_the_service_itself_states() {
    let router = emulator();
    let live = create_manager(&router, "a live manager", "ordering").await;
    let id = definition_id(&router, "a live manager", &live).await;
    let unserved = UNSERVED_ARNS[0].1;

    // (case, method, path builder, body, the status and code a SERVED target earns)
    struct Row {
        case: &'static str,
        method: &'static str,
        path: fn(&str, &str) -> String,
        body: Option<Value>,
        served_status: u16,
        served_code: &'static str,
    }
    let rows = vec![
        Row {
            // The manager's own route, with a `Name` outside its modelled pattern: the
            // walk refuses it, but only once the target has been authorized.
            case: "UpdateConfigurationManager with a Name outside its modelled pattern",
            method: "PUT",
            path: |arn, _| format!("/configurationManager/{}", label(arn)),
            body: Some(json!({ "Name": "renamed!!" })),
            served_status: 400,
            served_code: "ValidationException",
        },
        Row {
            // `TagsMap` is `@required`, so a body without it is refused by the walk — but
            // only once the target has been authorized.
            case: "TagResource with no Tags member at all",
            method: "PUT",
            path: |arn, _| format!("/tags/{}", label(arn)),
            body: Some(json!({})),
            served_status: 400,
            served_code: "ValidationException",
        },
        Row {
            // A tag key under the AWS-reserved prefix is the service's own refusal.
            case: "TagResource with a reserved tag key",
            method: "PUT",
            path: |arn, _| format!("/tags/{}", label(arn)),
            body: Some(json!({ "Tags": { "aws:cloudformation:stack": "s" } })),
            served_status: 400,
            served_code: "ValidationException",
        },
        Row {
            case: "UntagResource with a reserved tag key",
            method: "DELETE",
            path: |arn, _| format!("/tags/{}?tagKeys=aws:reserved", label(arn)),
            body: None,
            served_status: 400,
            served_code: "ValidationException",
        },
        Row {
            // The definition id is a label of its own, and the manager this one names
            // holds no definition with it: the service's modelled not-found.
            case: "UpdateConfigurationDefinition naming no stored definition",
            method: "PUT",
            path: |arn, _| {
                format!(
                    "/configurationDefinition/{}/{}",
                    label(arn),
                    label("zzzzzzzz")
                )
            },
            body: Some(json!({ "TypeVersion": "LATEST" })),
            served_status: 404,
            served_code: "ResourceNotFoundException",
        },
        Row {
            // A `TypeVersion` outside its modelled pattern is the walk's refusal.
            case: "UpdateConfigurationDefinition with a TypeVersion outside its pattern",
            method: "PUT",
            path: |arn, id| {
                format!(
                    "/configurationDefinition/{}/{}",
                    label(arn),
                    label(id)
                )
            },
            body: Some(json!({ "TypeVersion": "not-a-version" })),
            served_status: 400,
            served_code: "ValidationException",
        },
    ];

    for row in &rows {
        let (status, answer) = call(
            &router,
            row.method,
            &(row.path)(unserved, SOME_DEFINITION_ID),
            row.body.as_ref(),
        )
        .await;
        assert_unauthorized(
            &format!("{} on an unserved target", row.case),
            status,
            &answer,
        );

        let (status, answer) = call(
            &router,
            row.method,
            &(row.path)(&live, &id),
            row.body.as_ref(),
        )
        .await;
        assert_eq!(
            status, row.served_status,
            "case {:?} on a served target: expected {}, got {status} / {answer:?}",
            row.case, row.served_status
        );
        let code = error_code(&answer);
        assert_eq!(
            code, row.served_code,
            "case {:?} on a served target: expected {}, got {code:?} in {answer:?}",
            row.case, row.served_code
        );
    }
}

/// An ARN the request carries in its BODY is no route label, so no route is authorized
/// against it: `ListConfigurations` resolves it in the handler and answers this service's
/// own modelled `ResourceNotFoundException`, with the not-found sentence built from the
/// ARN the request supplied.
#[tokio::test]
async fn an_arn_carried_in_the_body_is_still_the_services_own_not_found() {
    let router = emulator();
    // A manager that exists, so each refusal below is about the ARN that was sent.
    let live = create_manager(&router, "a live manager", "body-arn").await;

    // Only the pattern-valid rows: `ListConfigurationsInput.ManagerArn` carries the
    // modelled `ManagerArn` pattern, so a value outside it is the walk's own 400 — there
    // being no route label to authorize, nothing runs ahead of that walk here.
    for (target_case, target) in UNSERVED_ARNS {
        let (status, answer) = call(
            &router,
            "POST",
            "/listConfigurations",
            Some(&json!({ "ManagerArn": target })),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {target_case:?}: a body ARN must stay the modelled 404, got {status} / {answer:?}"
        );
        let code = error_code(&answer);
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {target_case:?}: expected ResourceNotFoundException, got {code:?} in {answer:?}"
        );
        let message = error_message(&answer);
        let expected = format!("Quick setup manager with arn: {target} could not be found.");
        assert_eq!(
            message, expected,
            "case {target_case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // The live manager's configurations are still listable through the same route.
    let (status, answer) = call(
        &router,
        "POST",
        "/listConfigurations",
        Some(&json!({ "ManagerArn": live })),
    )
    .await;
    assert_eq!(
        status, 200,
        "the live manager must be listable, got {status} / {answer:?}"
    );
}
