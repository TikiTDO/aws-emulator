// SPDX-License-Identifier: CC0-1.0

//! What this emulator answers when a request reaches a bound the EMULATOR holds one of
//! its own stores under.
//!
//! The RULE: such a refusal names an error shape the refusing operation's own model
//! declares for it — the declared shape whose documented condition covers "the store
//! cannot hold another one" — so the status a client classifies the answer by is one of
//! that operation's declared statuses. This service strips every error-type discriminator
//! from a refusal (`src/service/untyped_errors.rs`), so the status is all a client has,
//! and a shape the model does not declare for the operation is therefore not widened into
//! the channel to carry one of these refusals.
//!
//! The rows span every store this emulator bounds on its own account, and so both sides
//! of the rule:
//!
//! * `PutMetadataFlag`, `RefreshToken` and `ExchangeCodeForToken` declare no quota and no
//!   throttling error, so each refuses with the `InvalidParameterException` (400) it does
//!   declare — "an invalid or out-of-range value was supplied for the input parameter",
//!   which is what a request naming an app environment or a token session outside the
//!   range the store can hold supplies;
//! * `StartCodegenJob` declares `ThrottlingException` (429) and refuses with that;
//! * `CreateComponent` declares `ServiceQuotaExceededException` (402) and refuses with
//!   that.
//!
//! Each row also pins what the refusal may not carry: an error type (this service names
//! none) and anything the request supplied (the sentence is the only machine-readable name
//! the class has on an untyped refusal, so it quotes the bound and nothing per-caller).
//!
//! The declared statuses in the table below come from
//! `smithy_query(query="operation",operation="PutMetadataFlag,RefreshToken,ExchangeCodeForToken,StartCodegenJob,CreateComponent")`.
//!
//! Each suite in this crate carries its own request harness, so this one does too.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::state::{
    CODEGEN_JOB_QUOTA, COMPONENT_QUOTA, METADATA_ENVIRONMENT_QUOTA, METADATA_FEATURES,
    METADATA_FEATURE_DEFAULT, PROVIDER_TOKEN_SESSION_QUOTA,
};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The app and backend environment the rows fill a store under. `StartCodegenJob$appId`
/// targets `AppId`, which the model constrains `@pattern("^d[a-z0-9]+$")`, so the id is
/// spelled the way that operation admits.
const APP_ID: &str = "d1234abcd";
const ENVIRONMENT: &str = "staging";

/// An app the rows hold nothing under: where a request past the bound addresses another
/// app environment entirely.
const ANOTHER_APP_ID: &str = "d9999zzzz";

/// The only `TokenProviders` value the model declares.
const PROVIDER: &str = "figma";

/// The response header restJson1 resolves an error shape from. It must be absent from
/// every refusal this service answers.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// Which of this emulator's own stores a row is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bounded {
    /// The metadata flags of an app environment, written by `PutMetadataFlag`.
    Metadata,
    /// The live token sessions, as `RefreshToken` reaches them.
    TokenRefresh,
    /// The same store, as `ExchangeCodeForToken` reaches it.
    TokenExchange,
    /// The codegen jobs, started by `StartCodegenJob`.
    CodegenJob,
    /// The components, created by `CreateComponent`.
    Component,
}

/// One bound: the operation whose request reaches it, the statuses that operation's model
/// declares, the status the rule predicts for the refusal, and how many records the store
/// holds once it is at the bound.
struct Site {
    operation: &'static str,
    store: Bounded,
    declared: &'static [u16],
    refusal: u16,
    bound: usize,
}

const SITES: [Site; 5] = [
    Site {
        operation: "PutMetadataFlag",
        store: Bounded::Metadata,
        declared: &[400, 401],
        refusal: 400,
        bound: METADATA_ENVIRONMENT_QUOTA,
    },
    Site {
        operation: "RefreshToken",
        store: Bounded::TokenRefresh,
        declared: &[400],
        refusal: 400,
        bound: PROVIDER_TOKEN_SESSION_QUOTA,
    },
    Site {
        operation: "ExchangeCodeForToken",
        store: Bounded::TokenExchange,
        declared: &[400],
        refusal: 400,
        bound: PROVIDER_TOKEN_SESSION_QUOTA,
    },
    Site {
        operation: "StartCodegenJob",
        store: Bounded::CodegenJob,
        declared: &[400, 429, 500],
        refusal: 429,
        bound: CODEGEN_JOB_QUOTA,
    },
    Site {
        operation: "CreateComponent",
        store: Bounded::Component,
        declared: &[400, 402, 409, 500],
        refusal: 402,
        bound: COMPONENT_QUOTA,
    },
];

/// One request that reaches a bound: how it differs from what the store holds, and the
/// values it supplies — none of which the refusal may quote.
struct PastTheBound {
    case: &'static str,
    supplied: Vec<String>,
    answer: Answer,
}

/// What the emulator answered: the HTTP status, the JSON body, and the error-type header.
struct Answer {
    status: u16,
    body: Value,
    error_type_header: Option<String>,
}

impl Answer {
    /// Whether the answer names an error type anywhere a client would read one.
    fn names_an_error_type(&self) -> bool {
        self.error_type_header.is_some()
            || ["__type", "code", "Code"]
                .iter()
                .any(|member| self.body[*member].is_string())
    }

    /// The `message` the answer carries, in either spelling, or the empty string.
    fn message(&self) -> String {
        self.body["message"]
            .as_str()
            .or_else(|| self.body["Message"].as_str())
            .unwrap_or_default()
            .to_string()
    }
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "amplifyuibuilder.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_AMPLIFYUIBUILDER);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type_header = response
        .headers()
        .get(ERROR_TYPE_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer {
        status,
        body,
        error_type_header,
    }
}

async fn put_flag(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    feature: &str,
) -> Answer {
    let uri = format!("/app/{app_id}/environment/{environment}/metadata/features/{feature}");
    call(router, "PUT", &uri, Some(json!({"newValue": "true"}))).await
}

async fn get_metadata(router: &MultiServiceRouter, app_id: &str, environment: &str) -> Answer {
    let uri = format!("/app/{app_id}/environment/{environment}/metadata");
    call(router, "GET", &uri, None).await
}

async fn refresh(router: &MultiServiceRouter, token: &str, client_id: Option<&str>) -> Answer {
    let mut body = json!({"token": token});
    if let Some(client_id) = client_id {
        body["clientId"] = json!(client_id);
    }
    call(
        router,
        "POST",
        &format!("/tokens/{PROVIDER}/refresh"),
        Some(body),
    )
    .await
}

async fn exchange(router: &MultiServiceRouter, code: &str, client_id: Option<&str>) -> Answer {
    let mut body = json!({"code": code, "redirectUri": "https://app.example.com/cb"});
    if let Some(client_id) = client_id {
        body["clientId"] = json!(client_id);
    }
    call(router, "POST", &format!("/tokens/{PROVIDER}"), Some(body)).await
}

async fn start_codegen_job(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
) -> Answer {
    let uri = format!("/app/{app_id}/environment/{environment}/codegen-jobs");
    call(
        router,
        "POST",
        &uri,
        Some(json!({"renderConfig": {"react": {}}})),
    )
    .await
}

async fn create_component(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    name: &str,
) -> Answer {
    let uri = format!("/app/{app_id}/environment/{environment}/components");
    call(
        router,
        "POST",
        &uri,
        Some(json!({
            "name": name,
            "componentType": "View",
            "properties": {},
            "variants": [],
            "overrides": {},
            "bindingProperties": {},
        })),
    )
    .await
}

/// Fill the store this site bounds right up to its bound, through the operation that
/// writes it, and report the LAST request the bound admitted: the control that shows the
/// refusal below is not simply what the operation always answers.
async fn fill_to_bound(router: &MultiServiceRouter, site: &Site) -> Answer {
    let mut last = None;
    for index in 0..site.bound {
        let answer = match site.store {
            Bounded::Metadata => {
                put_flag(router, &format!("app{index}"), ENVIRONMENT, METADATA_FEATURES[0]).await
            }
            Bounded::TokenRefresh => refresh(router, &format!("figd_token_{index}"), None).await,
            Bounded::TokenExchange => exchange(router, &format!("code-{index}"), None).await,
            Bounded::CodegenJob => start_codegen_job(router, APP_ID, ENVIRONMENT).await,
            Bounded::Component => {
                create_component(router, APP_ID, ENVIRONMENT, &format!("Component{index}")).await
            }
        };
        assert_eq!(
            answer.status, 200,
            "case {:?} record {index}: a request within the bound must succeed, got {} with {:?}",
            site.operation, answer.status, answer.body
        );
        last = Some(answer);
    }
    last.expect("every bound of this service admits at least one record")
}

/// The requests that reach the bound, each naming something the store does not hold and
/// each differing from the others in HOW it differs from what the store holds.
async fn requests_past_bound(router: &MultiServiceRouter, site: &Site) -> Vec<PastTheBound> {
    let owned = |values: [&str; 2]| values.iter().map(|value| value.to_string()).collect();
    match site.store {
        Bounded::Metadata => vec![
            PastTheBound {
                case: "a fresh appId",
                supplied: owned(["app-over-the-bound", METADATA_FEATURES[0]]),
                answer: put_flag(
                    router,
                    "app-over-the-bound",
                    ENVIRONMENT,
                    METADATA_FEATURES[0],
                )
                .await,
            },
            PastTheBound {
                case: "a fresh environmentName under an appId the store holds",
                supplied: owned(["another-environment", METADATA_FEATURES[1]]),
                answer: put_flag(router, "app0", "another-environment", METADATA_FEATURES[1]).await,
            },
        ],
        Bounded::TokenRefresh => vec![
            PastTheBound {
                case: "a refresh token the store holds no session for",
                supplied: owned(["figd_token_over_the_bound", PROVIDER]),
                answer: refresh(router, "figd_token_over_the_bound", None).await,
            },
            PastTheBound {
                case: "a held refresh token presented by another client",
                supplied: owned(["figd_token_0", "another-client"]),
                answer: refresh(router, "figd_token_0", Some("another-client")).await,
            },
        ],
        Bounded::TokenExchange => vec![
            PastTheBound {
                case: "a fresh access code",
                supplied: owned(["code-over-the-bound", PROVIDER]),
                answer: exchange(router, "code-over-the-bound", None).await,
            },
            PastTheBound {
                case: "another fresh access code, for a client",
                supplied: owned(["code-over-the-bound-2", "a-client"]),
                answer: exchange(router, "code-over-the-bound-2", Some("a-client")).await,
            },
        ],
        Bounded::CodegenJob => vec![
            PastTheBound {
                case: "another job in the app environment that holds them",
                supplied: owned([APP_ID, ENVIRONMENT]),
                answer: start_codegen_job(router, APP_ID, ENVIRONMENT).await,
            },
            PastTheBound {
                case: "a job in an app environment holding none",
                supplied: owned([ANOTHER_APP_ID, "another-environment"]),
                answer: start_codegen_job(router, ANOTHER_APP_ID, "another-environment").await,
            },
        ],
        Bounded::Component => vec![
            PastTheBound {
                case: "another component in the app environment that holds them",
                supplied: owned(["ComponentOverTheBound", APP_ID]),
                answer: create_component(router, APP_ID, ENVIRONMENT, "ComponentOverTheBound")
                    .await,
            },
            PastTheBound {
                case: "a component in an app environment holding none",
                supplied: owned(["Component0", ANOTHER_APP_ID]),
                answer: create_component(router, ANOTHER_APP_ID, ENVIRONMENT, "Component0").await,
            },
        ],
    }
}

/// The request a store AT its bound still admits, where the operation has one: a write
/// that grows nothing. An exchange, a start and a create always add a record, so there is
/// none for those.
async fn still_admitted_at_bound(
    router: &MultiServiceRouter,
    site: &Site,
) -> Option<(&'static str, Answer)> {
    match site.store {
        Bounded::Metadata => Some((
            "a write to an app environment the store already holds",
            put_flag(router, "app0", ENVIRONMENT, METADATA_FEATURES[2]).await,
        )),
        Bounded::TokenRefresh => Some((
            "a refresh of a session the store already holds",
            refresh(router, "figd_token_0", None).await,
        )),
        Bounded::TokenExchange | Bounded::CodegenJob | Bounded::Component => None,
    }
}

/// The rule, on every store this emulator bounds on its own account: the refusal carries
/// the status the refusing operation's model declares for the shape it names, names no
/// error type, quotes the bound and nothing the request supplied — while the same request
/// within the bound is served.
#[tokio::test]
async fn an_emulator_store_bound_is_refused_with_a_status_the_operation_declares() {
    for site in &SITES {
        assert!(
            site.declared.contains(&site.refusal),
            "case {:?}: the status the rule predicts must be one the operation's model \
             declares, got {} outside {:?}",
            site.operation,
            site.refusal,
            site.declared
        );

        let router = emulator();
        let admitted = fill_to_bound(&router, site).await;
        assert_eq!(
            admitted.status, 200,
            "case {:?}: the last request within the bound must be served, got {} with {:?}",
            site.operation, admitted.status, admitted.body
        );

        let past = requests_past_bound(&router, site).await;
        assert_eq!(
            past.len(),
            2,
            "case {:?}: the table must drive both forms of request past the bound, got {}",
            site.operation,
            past.len()
        );
        for PastTheBound {
            case,
            supplied,
            answer,
        } in &past
        {
            let case = format!("{} past its bound: {case}", site.operation);
            assert_eq!(
                answer.status, site.refusal,
                "case {case:?}: expected HTTP {}, got {} with {:?}",
                site.refusal, answer.status, answer.body
            );
            assert!(
                site.declared.contains(&answer.status),
                "case {case:?}: expected a status the operation's model declares ({:?}), got {}",
                site.declared,
                answer.status
            );
            assert!(
                !answer.names_an_error_type(),
                "case {case:?}: expected a refusal naming no error type, got header {:?} and \
                 body {:?}",
                answer.error_type_header,
                answer.body
            );
            let message = answer.message();
            assert!(
                message.contains(&site.bound.to_string()),
                "case {case:?}: expected the refusal to quote the bound {}, got {message:?}",
                site.bound
            );
            for value in supplied {
                assert!(
                    !message.contains(value),
                    "case {case:?}: expected the refusal to quote nothing the request supplied, \
                     got {value:?} in {message:?}"
                );
            }
        }

        if let Some((case, answer)) = still_admitted_at_bound(&router, site).await {
            let case = format!("{} at its bound: {case}", site.operation);
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a request that grows the store by nothing must still be served, \
                 got {} with {:?}",
                answer.status, answer.body
            );
        }
    }
}

/// The refused write stores nothing: the app environment `PutMetadataFlag` was refused for
/// still reports every feature of the catalogue at its starting value, so the refusal is
/// not a write the emulator took and hid behind an error.
#[tokio::test]
async fn a_metadata_write_refused_for_the_bound_stores_nothing() {
    let router = emulator();
    let site = SITES
        .iter()
        .find(|site| site.store == Bounded::Metadata)
        .expect("the table holds the metadata bound");
    fill_to_bound(&router, site).await;

    for (case, app_id, environment) in [
        ("a fresh appId", "app-over-the-bound", ENVIRONMENT),
        ("a fresh environmentName", "app0", "another-environment"),
    ] {
        let refused = put_flag(&router, app_id, environment, METADATA_FEATURES[0]).await;
        assert_eq!(
            refused.status, site.refusal,
            "case {case:?}: expected HTTP {}, got {} with {:?}",
            site.refusal, refused.status, refused.body
        );

        let reported = get_metadata(&router, app_id, environment).await;
        assert_eq!(
            reported.status, 200,
            "case {case:?}: GetMetadata must still answer HTTP 200, got {} with {:?}",
            reported.status, reported.body
        );
        for feature in METADATA_FEATURES {
            assert_eq!(
                reported.body["features"][feature],
                json!(METADATA_FEATURE_DEFAULT),
                "case {case:?}: {feature:?} must still report {METADATA_FEATURE_DEFAULT:?}, got \
                 {:?}",
                reported.body["features"]
            );
        }
    }
}
