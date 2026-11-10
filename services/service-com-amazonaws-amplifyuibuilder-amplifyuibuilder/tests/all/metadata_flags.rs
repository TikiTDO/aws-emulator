// SPDX-License-Identifier: CC0-1.0

//! The UI-builder metadata of an app environment: what `PutMetadataFlag` stores and
//! `GetMetadata` reports.
//!
//! Two rules are under test:
//!
//! 1. `PutMetadataFlag` stores the value of ONE feature of the app environment it
//!    addresses, and `GetMetadata` reports what was stored: the value a caller wrote comes
//!    back verbatim, writing one feature leaves the others alone, and the metadata of two
//!    app environments are independent.
//! 2. `featureName` is a plain `@httpLabel` string, so the model cannot say which names
//!    exist: a name outside the catalogue the service holds is refused with HTTP 400 (and,
//!    like every refusal of this service, names no error type), and the refused write
//!    stores nothing.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::state::{
    METADATA_ENVIRONMENT_QUOTA, METADATA_FEATURES, METADATA_FEATURE_DEFAULT,
};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

const APP_ID: &str = "d1234abcd";
const ENVIRONMENT: &str = "staging";

/// The response header restJson1 resolves an error shape from. It must be absent from
/// every refusal this service answers.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
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

    /// The `features` map a `GetMetadata` answer reports.
    fn features(&self) -> Value {
        self.body["features"].clone()
    }
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
    new_value: &str,
) -> Answer {
    let uri = format!("/app/{app_id}/environment/{environment}/metadata/features/{feature}");
    call(router, "PUT", &uri, Some(json!({"newValue": new_value}))).await
}

async fn get_metadata(router: &MultiServiceRouter, app_id: &str, environment: &str) -> Answer {
    let uri = format!("/app/{app_id}/environment/{environment}/metadata");
    call(router, "GET", &uri, None).await
}

/// Before anything is written, `GetMetadata` reports every feature of the catalogue at its
/// starting value — the map is `@required`, so it is reported rather than omitted.
#[tokio::test]
async fn metadata_reports_every_feature_before_anything_is_written() {
    let router = emulator();
    for (case, app_id, environment) in [
        ("the addressed app environment", APP_ID, ENVIRONMENT),
        ("another environment of that app", APP_ID, "production"),
        ("an app nothing has touched", "d9999zzzz", ENVIRONMENT),
    ] {
        let answer = get_metadata(&router, app_id, environment).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: GetMetadata must answer HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        let features = answer.features();
        for feature in METADATA_FEATURES {
            assert_eq!(
                features[feature],
                json!(METADATA_FEATURE_DEFAULT),
                "case {case:?}: feature {feature:?} must start at {METADATA_FEATURE_DEFAULT:?}, \
                 got {features:?}"
            );
        }
        assert_eq!(
            features.as_object().map(serde_json::Map::len),
            Some(METADATA_FEATURES.len()),
            "case {case:?}: GetMetadata must report exactly the catalogue, got {features:?}"
        );
    }
}

/// Rule 1: a write to a known feature succeeds and is what the next `GetMetadata` reports,
/// for every feature of the catalogue and whatever value the caller wrote; the features it
/// did not name do not move.
#[tokio::test]
async fn a_written_feature_is_read_back_through_get_metadata() {
    // Rows differ along the value written — the two flag spellings, their uppercase forms,
    // a value that is not a flag at all, and an empty one — because
    // `PutMetadataFlagBody$newValue` is a plain `@required` String, not an enum.
    let values = ["true", "false", "TRUE", "not-a-boolean", ""];

    let router = emulator();
    // The value every feature is expected to report, updated as the loop writes: the whole
    // map is compared after each write, so a write that moved a feature it did not name
    // fails here.
    let mut expected: std::collections::BTreeMap<&str, String> = METADATA_FEATURES
        .iter()
        .map(|feature| (*feature, METADATA_FEATURE_DEFAULT.to_string()))
        .collect();

    for feature in METADATA_FEATURES {
        for value in values {
            let written = put_flag(&router, APP_ID, ENVIRONMENT, feature, value).await;
            assert_eq!(
                written.status, 200,
                "case {feature:?}={value:?}: PutMetadataFlag must answer HTTP 200, got {} with \
                 {:?}",
                written.status, written.body
            );
            expected.insert(feature, value.to_string());

            let features = get_metadata(&router, APP_ID, ENVIRONMENT).await.features();
            for (name, want) in &expected {
                assert_eq!(
                    features[*name],
                    json!(want),
                    "case {feature:?}={value:?}: {name:?} must report {want:?}, got {features:?}"
                );
            }
        }
    }

    // The metadata of two app environments are independent: neither write above reached
    // another one.
    for (case, app_id, environment) in [
        ("another environment of the same app", APP_ID, "production"),
        ("another app, the same environment", "d9999zzzz", ENVIRONMENT),
    ] {
        let features = get_metadata(&router, app_id, environment).await.features();
        for feature in METADATA_FEATURES {
            assert_eq!(
                features[feature],
                json!(METADATA_FEATURE_DEFAULT),
                "case {case:?}: {feature:?} must still be at its starting value, got \
                 {features:?}"
            );
        }
    }

    // A write addressed to another app environment lands there and only there.
    put_flag(&router, "d9999zzzz", ENVIRONMENT, METADATA_FEATURES[0], "true").await;
    assert_eq!(
        get_metadata(&router, "d9999zzzz", ENVIRONMENT).await.features()[METADATA_FEATURES[0]],
        json!("true"),
        "a write must land in the app environment it addressed"
    );
    assert_eq!(
        get_metadata(&router, APP_ID, "production").await.features()[METADATA_FEATURES[0]],
        json!(METADATA_FEATURE_DEFAULT),
        "a write must not reach another app environment"
    );
}

/// Rule 2: a `featureName` outside the catalogue is refused with HTTP 400, names no error
/// type, and stores nothing.
#[tokio::test]
async fn an_unknown_feature_name_is_refused() {
    let router = emulator();
    // Rows differ in HOW the name misses the catalogue: unrelated, a case variant of a
    // known name, a known name with extra characters, a near-miss spelling, and a value
    // that looks like a path segment.
    let unknown = [
        ("an unrelated name", "someUnknownFeature"),
        ("the uppercase form of a known name", "AutoGenerateForms"),
        ("the lowercase form of a known name", "autogenerateforms"),
        ("a known name with a suffix", "autoGenerateFormsV2"),
        ("a known name with a prefix", "xautoGenerateForms"),
        ("a near-miss spelling", "autoGenerateForm"),
        ("a number", "42"),
    ];

    for (case, feature) in unknown {
        let refused = put_flag(&router, APP_ID, ENVIRONMENT, feature, "true").await;
        assert_eq!(
            refused.status, 400,
            "case {case:?} ({feature:?}): an unknown feature must be refused with HTTP 400, \
             got {} with {:?}",
            refused.status, refused.body
        );
        assert!(
            !refused.names_an_error_type(),
            "case {case:?} ({feature:?}): a refusal must name no error type, got header {:?} \
             and body {:?}",
            refused.error_type_header,
            refused.body
        );
        assert!(
            refused.body["message"]
                .as_str()
                .or_else(|| refused.body["Message"].as_str())
                .is_some_and(|message| message.contains(feature)),
            "case {case:?} ({feature:?}): the refusal must name the feature it rejected, got \
             {:?}",
            refused.body
        );

        let features = get_metadata(&router, APP_ID, ENVIRONMENT).await.features();
        assert_eq!(
            features.get(feature),
            None,
            "case {case:?} ({feature:?}): a refused write must store nothing, got {features:?}"
        );
        assert_eq!(
            features.as_object().map(serde_json::Map::len),
            Some(METADATA_FEATURES.len()),
            "case {case:?} ({feature:?}): the reported catalogue must not grow, got {features:?}"
        );
    }
}

/// The metadata store is bounded: `appId` and `environmentName` come from the request, so
/// past the emulator's own bound a write that would START another app environment's
/// metadata is refused (HTTP 400, the `InvalidParameterException` this operation's model
/// declares — see `tests/all/emulator_store_bound_refusals.rs` for the rule), while a write
/// to an app environment the store already holds still succeeds.
#[tokio::test]
async fn the_metadata_store_is_bounded_by_its_quota() {
    let router = emulator();
    let feature = METADATA_FEATURES[0];

    // Fill the store to its bound, one app environment per request.
    for index in 0..METADATA_ENVIRONMENT_QUOTA {
        let answer = put_flag(&router, &format!("app{index}"), ENVIRONMENT, feature, "true").await;
        assert_eq!(
            answer.status, 200,
            "case app environment {index}: a write within the bound must succeed, got {} with \
             {:?}",
            answer.status, answer.body
        );
    }

    // One more app environment is refused, however it differs from the ones held.
    for (case, app_id, environment) in [
        ("a fresh appId", "app-over-the-bound", ENVIRONMENT),
        ("a fresh environmentName", "app0", "another-environment"),
    ] {
        let refused = put_flag(&router, app_id, environment, feature, "true").await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a write past the bound must be refused with HTTP 400, got {} with \
             {:?}",
            refused.status, refused.body
        );
        assert!(
            !refused.names_an_error_type(),
            "case {case:?}: a refusal must name no error type, got header {:?} and body {:?}",
            refused.error_type_header,
            refused.body
        );
        let features = get_metadata(&router, app_id, environment).await.features();
        assert_eq!(
            features[feature],
            json!(METADATA_FEATURE_DEFAULT),
            "case {case:?}: a refused write must store nothing, got {features:?}"
        );
    }

    // An app environment the store already holds still accepts writes, on every feature of
    // the catalogue.
    for held in METADATA_FEATURES {
        let answer = put_flag(&router, "app0", ENVIRONMENT, held, "false").await;
        assert_eq!(
            answer.status, 200,
            "case {held:?}: a write to an app environment already held must succeed, got {} \
             with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            get_metadata(&router, "app0", ENVIRONMENT).await.features()[held],
            json!("false"),
            "case {held:?}: the write must be read back"
        );
    }
}
