// SPDX-License-Identifier: CC0-1.0

//! AppSync's error contract, end to end over HTTP: which failures take which status,
//! which modelled error shape, and which text.
//!
//! Four rules are asserted here, each over a TABLE that spans the class it
//! generalises over rather than the single request that exposed it:
//!
//! 1. an `apiId` longer than the 26 characters an AppSync API id has names no API, so
//!    the call is a bad request (400 `BadRequestException`) naming the rejected id;
//!    a shorter id that simply names nothing is a not-found (404) — asserted on every
//!    operation that resolves an `apiId`, over ids from 1 to 64 characters;
//! 2. a member the model rules out outright — absent though required, or outside the
//!    member's modelled `@range` — is AppSync's own `BadRequestException` (400)
//!    naming that member, and the whole modelled range when it has one; asserted on
//!    every `@range` member this service's model declares, on each published bound
//!    and one past each, and on absent members at the top level and nested;
//! 3. a value outside an enumerated type's value set is reported by member path and
//!    permitted value set, WITHOUT echoing the rejected value, while every other
//!    constraint class keeps its echo;
//! 4. a `nextToken` a list operation cannot resume from takes AppSync's fixed
//!    invalid-token text, while a token that does name a page still resumes.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization, the boundary validator, the handlers
//! and the shared engine together.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "eu-west-1";

/// The number of characters an AppSync API id has; `state::API_ID_LEN` is the
/// service's own copy, and an id of exactly this length is well formed.
const API_ID_LEN: usize = 26;

/// `AuthenticationType`'s whole value set, in the order the model declares it.
/// Source: `smithy_query(query="shape", shape="AuthenticationType")`.
const AUTHENTICATION_TYPES: &[&str] = &[
    "API_KEY",
    "AWS_IAM",
    "AMAZON_COGNITO_USER_POOLS",
    "OPENID_CONNECT",
    "AWS_LAMBDA",
];

/// One answer from the emulator: the parts a client branches on.
///
/// Shared with the other suites that assert this service's error contract, so every
/// one of them reads the status, the error shape and the message the same way.
#[derive(Debug)]
pub(crate) struct Answer {
    pub(crate) status: u16,
    /// The `x-amzn-errortype` header restJson1 resolves the error shape from.
    pub(crate) error_type: String,
    /// The error body's `message`, or the empty string when there is none.
    pub(crate) message: String,
    pub(crate) body: Value,
}

/// A router with only this service mounted, over a fresh engine.
pub(crate) fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one request and return what the emulator answered.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(&value).expect("the request body serializes"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://appsync.{REGION}.amazonaws.com{path}"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/appsync/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(payload)
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Answer {
        status,
        error_type,
        message,
        body,
    }
}

/// A `CreateGraphqlApi` body with `name` and the required members set.
fn create_graphql_api_body(name: &str) -> Value {
    json!({
        "name": name,
        "authenticationType": "API_KEY",
        "queryDepthLimit": 0,
        "resolverCountLimit": 0,
        "xrayEnabled": false,
    })
}

/// Create one GraphQL API and return the `apiId` the service minted for it.
pub(crate) async fn create_api(router: &MultiServiceRouter, name: &str) -> String {
    let answer = call(
        router,
        "POST",
        "/v1/apis",
        Some(create_graphql_api_body(name)),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case create {name:?}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body["graphqlApi"]["apiId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A `CreateApi` (Event API) body whose auth-mode lists hold `auth_types`, with the
/// required `eventConfig` members present.
fn create_event_api_body(name: &str, connection: &str, publish: &str, subscribe: &str) -> Value {
    json!({
        "name": name,
        "eventConfig": {
            "authProviders": [{"authType": "API_KEY"}],
            "connectionAuthModes": [{"authType": connection}],
            "defaultPublishAuthModes": [{"authType": publish}],
            "defaultSubscribeAuthModes": [{"authType": subscribe}],
        },
    })
}

/// Rule 1: the id's SHAPE decides bad-request from not-found, on every operation that
/// resolves an `apiId`.
///
/// The rows differ along the one thing the rule reads — the id's length — from a
/// single character to well past the id length, and each is sent to all four
/// `apiId`-resolving operations.
#[tokio::test]
async fn an_overlong_api_id_is_a_bad_request_and_a_short_absent_one_is_not_found() {
    let router = router();

    // (label, id). Well formed: length <= API_ID_LEN. Malformed: longer.
    let well_formed = [
        ("one character".to_string(), "a".to_string()),
        (
            "21 characters outside the id alphabet".to_string(),
            "nonexistent0000000000".to_string(),
        ),
        ("one short of the id length".to_string(), "b".repeat(API_ID_LEN - 1)),
        ("exactly the id length".to_string(), "c".repeat(API_ID_LEN)),
    ];
    let malformed = [
        ("one past the id length".to_string(), "d".repeat(API_ID_LEN + 1)),
        (
            "32 characters".to_string(),
            "f3a91c07e6b5d0a48c2e1976badcafe0".to_string(),
        ),
        ("64 characters".to_string(), "e".repeat(64)),
    ];

    for (label, id) in well_formed.iter().chain(malformed.iter()) {
        let malformed_id = id.chars().count() > API_ID_LEN;
        let calls = [
            ("GetGraphqlApi", "GET", format!("/v1/apis/{id}"), None),
            ("DeleteGraphqlApi", "DELETE", format!("/v1/apis/{id}"), None),
            (
                "ListApiKeys",
                "GET",
                format!("/v1/apis/{id}/apikeys"),
                None,
            ),
            (
                "CreateApiKey",
                "POST",
                format!("/v1/apis/{id}/apikeys"),
                Some(json!({})),
            ),
        ];
        for (operation, method, path, body) in calls {
            let case = format!("{operation} with an apiId of {label}");
            let answer = call(&router, method, &path, body).await;
            if malformed_id {
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: expected status 400, got {:?}",
                    answer
                );
                assert_eq!(
                    answer.error_type, "BadRequestException",
                    "case {case:?}: expected error type BadRequestException, got {:?}",
                    answer
                );
                assert_eq!(
                    answer.message,
                    format!("Invalid API ID {id}"),
                    "case {case:?}: expected the refusal to name the rejected id, got {:?}",
                    answer
                );
            } else {
                assert_eq!(
                    answer.status, 404,
                    "case {case:?}: expected status 404, got {:?}",
                    answer
                );
                assert_eq!(
                    answer.error_type, "NotFoundException",
                    "case {case:?}: expected error type NotFoundException, got {:?}",
                    answer
                );
                assert_eq!(
                    answer.message, "API not found.",
                    "case {case:?}: expected AppSync's fixed not-found text, got {:?}",
                    answer
                );
            }
        }
    }
}

/// An id the service actually minted is served, not refused: the same four operations
/// answer 200 for it, so the rule above refuses only ids that cannot name an API.
#[tokio::test]
async fn a_minted_api_id_is_served_by_every_operation_that_resolves_one() {
    let router = router();
    let api_id = create_api(&router, "Api_minted_id").await;
    assert_eq!(
        api_id.chars().count(),
        API_ID_LEN,
        "case minted apiId: expected {API_ID_LEN} characters, got {api_id:?}"
    );

    let read = call(&router, "GET", &format!("/v1/apis/{api_id}"), None).await;
    assert_eq!(
        read.status, 200,
        "case GetGraphqlApi on a minted id: expected 200, got {read:?}"
    );
    assert_eq!(
        read.body["graphqlApi"]["apiId"].as_str(),
        Some(api_id.as_str()),
        "case GetGraphqlApi on a minted id: expected the stored api, got {read:?}"
    );

    let created_key = call(
        &router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        created_key.status, 200,
        "case CreateApiKey on a minted id: expected 200, got {created_key:?}"
    );

    let keys = call(&router, "GET", &format!("/v1/apis/{api_id}/apikeys"), None).await;
    assert_eq!(
        keys.status, 200,
        "case ListApiKeys on a minted id: expected 200, got {keys:?}"
    );

    let deleted = call(&router, "DELETE", &format!("/v1/apis/{api_id}"), None).await;
    assert_eq!(
        deleted.status, 200,
        "case DeleteGraphqlApi on a minted id: expected 200, got {deleted:?}"
    );
}

/// Rule 2: a value outside a member's modelled `@range` is AppSync's own
/// `BadRequestException`, stating the limit and the whole modelled range.
///
/// One row per `@range` member the model declares, each tested on both published
/// bounds (accepted) and on one value past each (refused). Source for the bounds:
/// `smithy_query(query="shape", shape="MaxResults,MaxBatchSize,QueryDepthLimit,
/// ResolverCountLimit")`; `range` is the refusal's rendering of those bounds, which
/// groups thousands, and `limit` is the label AppSync names the limit by.
#[tokio::test]
async fn a_value_outside_a_modelled_range_names_the_limit_and_the_range() {
    let router = router();
    let api_id = create_api(&router, "Api_range_rows").await;

    // One row per `@range` member: where the value goes, the label the refusal names
    // it by, and the bounds as the refusal renders them.
    struct RangeCase {
        member: &'static str,
        limit: &'static str,
        range: &'static str,
        min: i64,
        max: i64,
        carrier: Carrier,
    }
    // Where the row's value rides: a `CreateGraphqlApi` body member, or a list
    // operation's `maxResults` query parameter.
    enum Carrier {
        Body { name: &'static str },
        Query { base: String },
    }
    let rows = vec![
        RangeCase {
            member: "queryDepthLimit",
            limit: "queryDepthLimit",
            range: "0 to 75",
            min: 0,
            max: 75,
            carrier: Carrier::Body {
                name: "Api_depth",
            },
        },
        RangeCase {
            member: "resolverCountLimit",
            // AppSync names this limit `resolverLimit`, and groups the thousands in
            // its bound; `service::REFUSAL_LABELS` holds that vocabulary.
            limit: "resolverLimit",
            range: "0 to 10,000",
            min: 0,
            max: 10_000,
            carrier: Carrier::Body {
                name: "Api_resolvers",
            },
        },
        RangeCase {
            member: "maxResults",
            limit: "maxResults",
            range: "0 to 25",
            min: 0,
            max: 25,
            carrier: Carrier::Query {
                base: "/v1/apis".to_string(),
            },
        },
        RangeCase {
            member: "maxResults",
            limit: "maxResults",
            range: "0 to 25",
            min: 0,
            max: 25,
            carrier: Carrier::Query {
                base: format!("/v1/apis/{api_id}/apikeys"),
            },
        },
    ];

    for row in rows {
        // The same row at four values: each published bound, and one past each.
        for value in [row.min, row.max, row.min - 1, row.max + 1] {
            let (method, path, body) = match &row.carrier {
                Carrier::Body { name } => {
                    let mut body = create_graphql_api_body(name);
                    body[row.member] = json!(value);
                    ("POST", "/v1/apis".to_string(), Some(body))
                }
                Carrier::Query { base } => (
                    "GET",
                    format!("{base}?{}={value}", row.member),
                    None,
                ),
            };
            let within = (row.min..=row.max).contains(&value);
            let case = format!("{} at {value} ({method} {path})", row.member);
            let answer = call(&router, method, &path, body).await;
            if within {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected a published bound to be accepted, got {answer:?}"
                );
                continue;
            }
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected status 400, got {answer:?}"
            );
            assert_eq!(
                answer.error_type, "BadRequestException",
                "case {case:?}: expected error type BadRequestException, got {answer:?}"
            );
            assert_eq!(
                answer.message,
                format!("Invalid {} given. Must fall within range {}", row.limit, row.range),
                "case {case:?}: expected the limit and its modelled range, got {answer:?}"
            );
        }
    }
}

/// Rule 2b: a required member absent from the request is refused the same way — the
/// service's own `BadRequestException` naming that member.
///
/// The rows span a top-level member and a member nested inside a structure, on two
/// operations; the refusal names the member itself, never the path it sits on.
#[tokio::test]
async fn an_absent_required_member_is_named_by_appsyncs_own_refusal() {
    let router = router();

    // (label, method, path, body, the name the refusal must use for the member).
    let rows: Vec<(&str, &str, &str, Value, &str)> = vec![
        (
            "CreateGraphqlApi without authenticationType",
            "POST",
            "/v1/apis",
            json!({"name": "Api_no_auth"}),
            "authenticationType",
        ),
        (
            "CreateGraphqlApi with an incomplete enhancedMetricsConfig",
            "POST",
            "/v1/apis",
            {
                let mut body = create_graphql_api_body("Api_metrics");
                // `EnhancedMetricsConfig` requires all three behaviours
                // (`smithy_query(query="shape", shape="EnhancedMetricsConfig")`).
                body["enhancedMetricsConfig"] = json!({
                    "resolverLevelMetricsBehavior": "FULL_REQUEST_RESOLVER_METRICS",
                    "operationLevelMetricsConfig": "ENABLED",
                });
                body
            },
            "dataSourceLevelMetricsBehavior",
        ),
        (
            "CreateApi without eventConfig",
            "POST",
            "/v2/apis",
            json!({"name": "EventApi no config"}),
            "eventConfig",
        ),
        (
            "CreateGraphqlApi without operationLevelMetricsConfig",
            "POST",
            "/v1/apis",
            {
                let mut body = create_graphql_api_body("Api_operation_metrics");
                body["enhancedMetricsConfig"] = json!({
                    "resolverLevelMetricsBehavior": "FULL_REQUEST_RESOLVER_METRICS",
                    "dataSourceLevelMetricsBehavior": "FULL_REQUEST_DATA_SOURCE_METRICS",
                });
                body
            },
            // AppSync's own label for `operationLevelMetricsConfig` is short of the
            // member's plural; `service::REFUSAL_LABELS` holds that vocabulary.
            "operationLevelMetricConfig",
        ),
    ];

    for (label, method, path, body, named) in rows {
        let answer = call(&router, method, path, Some(body)).await;
        assert_eq!(
            answer.status, 400,
            "case {label:?}: expected status 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_type, "BadRequestException",
            "case {label:?}: expected error type BadRequestException, got {answer:?}"
        );
        assert_eq!(
            answer.message,
            format!("Invalid {named} given."),
            "case {label:?}: expected the refusal to name {named:?}, got {answer:?}"
        );
    }
}

/// Rule 3: an enum-value clause names the member path and the permitted values and
/// withholds the rejected value — on each offending member, however many there are.
#[tokio::test]
async fn an_enum_clause_names_the_value_set_and_withholds_the_rejected_value() {
    let router = router();

    // One row per request that should be refused, with the member paths its
    // clauses must flag.
    struct EnumCase {
        label: &'static str,
        method: &'static str,
        path: &'static str,
        body: Value,
        paths: Vec<&'static str>,
    }
    let rows = vec![
        EnumCase {
            label: "one offending auth-mode list",
            method: "POST",
            path: "/v2/apis",
            body: create_event_api_body("EventApi one", "INVALID_AUTH", "API_KEY", "API_KEY"),
            paths: vec!["eventConfig.connectionAuthModes.1.member.authType"],
        },
        EnumCase {
            label: "all three offending auth-mode lists",
            method: "POST",
            path: "/v2/apis",
            body: create_event_api_body(
                "EventApi three",
                "INVALID_AUTH",
                "INVALID_AUTH",
                "INVALID_AUTH",
            ),
            paths: vec![
                "eventConfig.connectionAuthModes.1.member.authType",
                "eventConfig.defaultPublishAuthModes.1.member.authType",
                "eventConfig.defaultSubscribeAuthModes.1.member.authType",
            ],
        },
        EnumCase {
            label: "an offending authenticationType on another operation",
            method: "POST",
            path: "/v1/apis",
            body: {
                let mut body = create_graphql_api_body("Api_bad_auth");
                body["authenticationType"] = json!("INVALID_AUTH");
                body
            },
            paths: vec!["authenticationType"],
        },
    ];

    for case in rows {
        let EnumCase {
            label,
            method,
            path,
            body,
            paths,
        } = case;
        let answer = call(&router, method, path, Some(body)).await;
        assert_eq!(
            answer.status, 400,
            "case {label:?}: expected status 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_type, "ValidationException",
            "case {label:?}: expected error type ValidationException, got {answer:?}"
        );
        let expected_count = paths.len();
        let plural = if expected_count == 1 { "error" } else { "errors" };
        assert!(
            answer
                .message
                .starts_with(&format!("{expected_count} validation {plural} detected: ")),
            "case {label:?}: expected one clause per offending member, got {answer:?}"
        );
        assert!(
            !answer.message.contains("INVALID_AUTH"),
            "case {label:?}: expected the rejected value to be withheld, got {answer:?}"
        );
        for path in &paths {
            assert!(
                answer.message.contains(&format!(
                    "Value at '{path}' failed to satisfy constraint: Member must satisfy enum \
                     value set: ["
                )),
                "case {label:?}: expected a clause for {path:?}, got {answer:?}"
            );
        }
        for value in AUTHENTICATION_TYPES {
            assert!(
                answer.message.contains(value),
                "case {label:?}: expected the permitted value {value:?} to be listed, \
                 got {answer:?}"
            );
        }
    }
}

/// Withholding the value is the ENUM clause's rule, not every clause's: a `@pattern`
/// violation still echoes what the caller sent, so a client can see which value was
/// rejected.
///
/// Asserted on a member whose refusal this SERVICE words — `CreateDomainName`'s
/// `certificateArn` — rather than on the Event API's `name`: the v2 `Api` surface answers
/// its own `name` with the framework's constraint error, whose clauses for that member
/// quote no value at all (`crate::event_api_name_form`).
#[tokio::test]
async fn a_pattern_clause_still_echoes_the_rejected_value() {
    let router = router();
    // `CreateDomainNameRequest$certificateArn` targets `CertificateArn`, whose `@pattern`
    // admits only an ACM or IAM certificate ARN
    // (`smithy_query(query="shape", shape="CertificateArn")`).
    let rejected = "arn:aws:s3:::held/certificate/one";
    let answer = call(
        &router,
        "POST",
        "/v1/domainnames",
        Some(json!({
            "domainName": "echoed.example.com",
            "certificateArn": rejected,
        })),
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "case an ARN outside CertificateArn's pattern: expected status 400, got {answer:?}"
    );
    assert!(
        answer
            .message
            .contains(&format!("Value '{rejected}' at 'certificateArn'")),
        "case an ARN outside CertificateArn's pattern: expected the echoed value, \
         got {answer:?}"
    );
}

/// Rule 4: a `nextToken` that names no page of the list takes AppSync's fixed
/// invalid-token text, on every list operation; a token the list itself handed back
/// still resumes.
#[tokio::test]
async fn an_unusable_pagination_token_takes_appsyncs_fixed_text() {
    let router = router();
    let api_id = create_api(&router, "Api_tokens_one").await;
    create_api(&router, "Api_tokens_two").await;
    for _ in 0..2 {
        let created = call(
            &router,
            "POST",
            &format!("/v1/apis/{api_id}/apikeys"),
            Some(json!({})),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case seed an API key: expected 200, got {created:?}"
        );
    }

    // The rows differ along the list operation and the token's form.
    let lists = [
        ("ListGraphqlApis", "/v1/apis".to_string()),
        ("ListApiKeys", format!("/v1/apis/{api_id}/apikeys")),
    ];
    let tokens = ["bogus", "0", "zzzzzzzzzzzzzzzzzzzzzzzzzz", "da2-nosuchkey"];
    for (operation, base) in &lists {
        for token in tokens {
            let case = format!("{operation} resuming from {token:?}");
            let answer = call(&router, "GET", &format!("{base}?nextToken={token}"), None).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected status 400, got {answer:?}"
            );
            assert_eq!(
                answer.error_type, "BadRequestException",
                "case {case:?}: expected error type BadRequestException, got {answer:?}"
            );
            assert_eq!(
                answer.message, "Invalid pagination token given.",
                "case {case:?}: expected AppSync's fixed invalid-token text, got {answer:?}"
            );
        }

        // A token the list handed back still resumes, so the refusal above is about
        // tokens that name no page rather than about paging at all.
        let first = call(&router, "GET", &format!("{base}?maxResults=1"), None).await;
        assert_eq!(
            first.status, 200,
            "case {operation} first page: expected 200, got {first:?}"
        );
        let next = first.body["nextToken"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !next.is_empty(),
            "case {operation} first page: expected a nextToken, got {first:?}"
        );
        let resumed = call(
            &router,
            "GET",
            &format!("{base}?maxResults=1&nextToken={next}"),
            None,
        )
        .await;
        assert_eq!(
            resumed.status, 200,
            "case {operation} resuming from its own token: expected 200, got {resumed:?}"
        );
    }
}
