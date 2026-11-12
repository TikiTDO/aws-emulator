// SPDX-License-Identifier: CC0-1.0

//! The CodePipeline webhook lifecycle — `PutWebhook` and the `ListWebhooks` that
//! reads it back — end to end over the wire the service actually speaks: `awsJson1_1`,
//! `POST /` with `X-Amz-Target: CodePipeline_20150709.<Operation>`.
//!
//! Every assertion goes through the router, so nothing here can pass by reading a
//! handler's internals: a response that is not computed from the shared store fails.
//!
//! The pipeline suite's client is private to its own module; this suite keeps its own
//! minimal one so the two stay independently editable.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix OPERATIONS.md records for this service.
const TARGET_PREFIX: &str = "CodePipeline_20150709";

/// A `roleArn` satisfying `RoleArn`'s modelled pattern
/// (`^arn:aws(-[\w]+)*:iam::[0-9]{12}:role/`).
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/service-role/cp";

/// The canonical emulator account id every ARN carries
/// (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";

/// The secret token the GITHUB_HMAC cases send. `PutWebhook` and `ListWebhooks` both
/// document "If a secret token was provided, it will be redacted in the response", so
/// no response may ever carry this string back.
const SECRET_TOKEN: &str = "s3cr3t-github-token";

/// What a redacted `SecretToken` renders as (`state::REDACTED_SECRET_TOKEN`).
const REDACTED: &str = "****";

/// One emulator: the shared engine, the service mounted on it, and the router that
/// dispatches to it — exactly how `core/router` assembles the production emulator.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_codepipeline_codepipeline_20150709::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `operation` with `body`, signed for `region` so the handler reads the
    /// region off the SigV4 credential scope the way a real SDK request carries it.
    async fn call_in(&self, region: &str, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/codepipeline/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("authorization", authorization)
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or_else(|e| {
                panic!("{operation}: response body is not JSON ({e}): {body:?}")
            })
        };
        (status, parsed)
    }

    /// Call `operation` in the default region.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_in("us-east-1", operation, body).await
    }

    async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {response}"
        );
        response
    }

    /// The HTTP status and the `__type` an error response carries (the awsJson error
    /// code clients match on).
    async fn expect_error(&self, operation: &str, body: &Value) -> (u16, String) {
        let (status, response) = self.call(operation, body).await;
        let code = response
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{operation}: expected an error body, got {response}"))
            .to_string();
        (status, code)
    }

    /// Store the smallest pipeline named `name` a webhook can target: a source stage,
    /// and the non-`Source` stage every declaration has to carry
    /// (`state::validate_declaration`).
    async fn create_pipeline(&self, name: &str) {
        self.ok(
            "CreatePipeline",
            &json!({"pipeline": {
                "name": name,
                "roleArn": ROLE_ARN,
                "artifactStore": {"type": "S3", "location": "my-bucket"},
                "stages": [
                    {
                        "name": "Source",
                        "actions": [{
                            "name": "Src",
                            "actionTypeId": {
                                "category": "Source",
                                "owner": "AWS",
                                "provider": "S3",
                                "version": "1",
                            },
                        }],
                    },
                    {
                        "name": "Build",
                        "actions": [{
                            "name": "Build0",
                            "actionTypeId": {
                                "category": "Build",
                                "owner": "AWS",
                                "provider": "CodeBuild",
                                "version": "1",
                            },
                        }],
                    },
                ],
            }}),
        )
        .await;
    }

    /// Every stored webhook, in the order `ListWebhooks` reports them.
    async fn list_webhooks(&self) -> Vec<Value> {
        let listed = self.ok("ListWebhooks", &json!({})).await;
        listed["webhooks"]
            .as_array()
            .unwrap_or_else(|| panic!("ListWebhooks reports a webhooks list, got {listed}"))
            .clone()
    }
}

/// A `WebhookDefinition` with the parts each table row varies.
fn definition(
    name: &str,
    target_pipeline: &str,
    authentication: &str,
    authentication_configuration: Value,
    filters: Value,
) -> Value {
    json!({
        "name": name,
        "targetPipeline": target_pipeline,
        "targetAction": "Src",
        "filters": filters,
        "authentication": authentication,
        "authenticationConfiguration": authentication_configuration,
    })
}

/// The simplest definition that is a webhook: GITHUB_HMAC with one filter.
fn github_definition(name: &str, target_pipeline: &str) -> Value {
    definition(
        name,
        target_pipeline,
        "GITHUB_HMAC",
        json!({"SecretToken": SECRET_TOKEN}),
        json!([{"jsonPath": "$.ref", "matchEquals": "refs/heads/{Branch}"}]),
    )
}

/// A webhook of every shape a definition can take is stored as sent, and the same
/// webhook comes back out of `ListWebhooks`: the rule is "PutWebhook stores the
/// definition and reports the stored webhook", so the table varies what a definition
/// can differ in — authentication type and configuration, filter count, whether a
/// filter carries `matchEquals`, and whether the request applies tags.
#[tokio::test]
async fn every_webhook_definition_round_trips_through_list_webhooks() {
    // The `authentication` values are the model's own enum
    // (`WEBHOOK_AUTHENTICATION_TYPE_VALUES` -> GITHUB_HMAC, IP, UNAUTHENTICATED), and
    // `WebhookFilters` is modelled `@length(min: 0, max: 5)`, so 0 and 5 are the
    // published bounds of the filter count.
    let five_filters: Vec<Value> = (0..5)
        .map(|index| json!({"jsonPath": format!("$.filter{index}"), "matchEquals": "x"}))
        .collect();
    let cases: Vec<(&str, Value, Option<Value>)> = vec![
        (
            "GITHUB_HMAC, one filter, no tags",
            github_definition("github-hook", "demo"),
            None,
        ),
        (
            "IP range, no filters at all (the modelled minimum)",
            definition(
                "ip-hook",
                "demo",
                "IP",
                json!({"AllowedIPRange": "10.0.0.0/8"}),
                json!([]),
            ),
            None,
        ),
        (
            "UNAUTHENTICATED, five filters (the modelled maximum)",
            definition(
                "open-hook",
                "demo",
                "UNAUTHENTICATED",
                json!({}),
                Value::Array(five_filters),
            ),
            None,
        ),
        (
            "GITHUB_HMAC, a filter with no matchEquals",
            definition(
                "no-match-hook",
                "demo",
                "GITHUB_HMAC",
                json!({"SecretToken": SECRET_TOKEN}),
                json!([{"jsonPath": "$.repository.full_name"}]),
            ),
            None,
        ),
        (
            "IPv6 range with tags",
            definition(
                "v6.hook@1",
                "demo",
                "IP",
                json!({"AllowedIPRange": "2001:db8::/32"}),
                json!([{"jsonPath": "$['action']", "matchEquals": "opened"}]),
            ),
            Some(json!([{"key": "team", "value": "build"}, {"key": "env", "value": ""}])),
        ),
    ];

    for (case, webhook, tags) in cases {
        let emulator = Emulator::new();
        emulator.create_pipeline("demo").await;
        let name = webhook["name"].as_str().expect("a name").to_string();

        let mut request = json!({"webhook": webhook});
        if let Some(tags) = &tags {
            request["tags"] = tags.clone();
        }
        let put = emulator.ok("PutWebhook", &request).await;
        let item = &put["webhook"];

        // The definition is reported as stored, with the secret token redacted.
        let mut expected_definition = webhook.clone();
        if expected_definition["authenticationConfiguration"]
            .get("SecretToken")
            .is_some()
        {
            expected_definition["authenticationConfiguration"]["SecretToken"] = json!(REDACTED);
        }
        assert_eq!(
            item["definition"], expected_definition,
            "case {case:?}: PutWebhook reports the definition as stored, got {put}"
        );
        assert_eq!(
            item["arn"],
            json!(format!(
                "arn:aws:codepipeline:us-east-1:{ACCOUNT_ID}:webhook:{name}"
            )),
            "case {case:?}: unexpected webhook ARN, got {put}"
        );
        let url = item["url"].as_str().unwrap_or_default();
        assert!(
            url.starts_with(
                "https://emulator.invalid/_aws/codepipeline-webhooks/us-east-1/trigger?t="
            ) && url.ends_with("&v=1"),
            "case {case:?}: expected a trigger URL on an address this emulator serves, \
             got {url:?}"
        );
        // The tags member is always reported — an empty list for a definition that
        // applied none.
        assert_eq!(
            item.get("tags").cloned(),
            Some(tags.clone().unwrap_or_else(|| json!([]))),
            "case {case:?}: PutWebhook reports the tags applied, got {put}"
        );
        // Nothing has triggered a freshly defined webhook, and no trigger has
        // failed, so none of those members is invented.
        for absent in ["lastTriggered", "errorCode", "errorMessage"] {
            assert!(
                item.get(absent).is_none(),
                "case {case:?}: {absent} is absent on a webhook that never fired, got {put}"
            );
        }

        // The round trip: the webhook the store holds is the one that was put.
        let listed = emulator.list_webhooks().await;
        assert_eq!(
            listed,
            vec![item.clone()],
            "case {case:?}: ListWebhooks reports the stored webhook, got {listed:?}"
        );

        // Listing is reporting only: a second read answers the same thing.
        let again = emulator.list_webhooks().await;
        assert_eq!(
            again, listed,
            "case {case:?}: ListWebhooks changed what a later caller sees"
        );

        // The secret the caller supplied never leaves the emulator.
        let rendered = format!("{put}{listed:?}");
        assert!(
            !rendered.contains(SECRET_TOKEN),
            "case {case:?}: a response carried the secret token back: {rendered}"
        );
    }
}

/// `WebhookFilterRule$jsonPath` is "a JsonPath expression that is applied to the
/// body/payload of the webhook", so a value that is not a JsonPath expression is
/// refused with `InvalidWebhookFilterPatternException` and one that is, is stored.
/// The rows vary the form of the expression, not just its text.
#[tokio::test]
async fn a_filter_pattern_is_accepted_only_when_it_is_a_jsonpath_expression() {
    // (case, jsonPath, the error code expected, or None when it is accepted)
    let cases: [(&str, &str, Option<&str>); 13] = [
        ("the root alone", "$", None),
        ("a member", "$.ref", None),
        ("a nested member", "$.pull_request.base.ref", None),
        ("a bracketed name", "$['action']", None),
        ("an array index", "$.commits[0].id", None),
        ("a wildcard", "$.commits[*].id", None),
        ("a descendant", "$..id", None),
        (
            "plain text, no expression at all",
            "not-a-json-path",
            Some("InvalidWebhookFilterPatternException"),
        ),
        (
            "a member path missing its root",
            "ref",
            Some("InvalidWebhookFilterPatternException"),
        ),
        (
            "a trailing dot with no member",
            "$.",
            Some("InvalidWebhookFilterPatternException"),
        ),
        (
            "an unclosed bracket",
            "$.commits[0",
            Some("InvalidWebhookFilterPatternException"),
        ),
        (
            "an unclosed quoted name",
            "$['action",
            Some("InvalidWebhookFilterPatternException"),
        ),
        (
            "a descendant with no selector",
            "$..",
            Some("InvalidWebhookFilterPatternException"),
        ),
    ];

    for (case, json_path, expected_error) in cases {
        let emulator = Emulator::new();
        emulator.create_pipeline("demo").await;
        let webhook = definition(
            "hook",
            "demo",
            "UNAUTHENTICATED",
            json!({}),
            json!([{"jsonPath": json_path, "matchEquals": "x"}]),
        );
        let request = json!({"webhook": webhook});

        match expected_error {
            None => {
                let put = emulator.ok("PutWebhook", &request).await;
                assert_eq!(
                    put["webhook"]["definition"]["filters"][0]["jsonPath"],
                    json!(json_path),
                    "case {case:?}: an accepted filter is stored as sent, got {put}"
                );
            }
            Some(expected) => {
                let (status, code) = emulator.expect_error("PutWebhook", &request).await;
                assert_eq!(
                    (status, code.as_str()),
                    (400, expected),
                    "case {case:?}: unexpected refusal for jsonPath {json_path:?}"
                );
                // A refused definition stores nothing.
                let listed = emulator.list_webhooks().await;
                assert!(
                    listed.is_empty(),
                    "case {case:?}: a refused PutWebhook must store no webhook, got {listed:?}"
                );
            }
        }
    }
}

/// `JsonPath` is modelled `@length(min: 1, max: 150)`, so what reaches the expression
/// parser is bounded by the model: the longest expression the model allows is stored,
/// and one character past the maximum is refused at the boundary before the handler
/// runs.
#[tokio::test]
async fn a_filter_pattern_is_bounded_by_the_modelled_length() {
    let emulator = Emulator::new();
    emulator.create_pipeline("demo").await;

    // "$." plus a member name, sized to the modelled maximum and one past it.
    let cases = [
        ("the longest jsonPath the model allows", 148, None),
        (
            "one character past the modelled maximum",
            149,
            Some("ValidationException"),
        ),
    ];
    for (case, member_length, expected_error) in cases {
        let json_path = format!("$.{}", "a".repeat(member_length));
        let request = json!({"webhook": definition(
            "hook",
            "demo",
            "UNAUTHENTICATED",
            json!({}),
            json!([{"jsonPath": json_path, "matchEquals": "x"}]),
        )});
        match expected_error {
            None => {
                let put = emulator.ok("PutWebhook", &request).await;
                assert_eq!(
                    put["webhook"]["definition"]["filters"][0]["jsonPath"],
                    json!(json_path),
                    "case {case:?}: a {}-character jsonPath is stored as sent, got {put}",
                    json_path.len()
                );
            }
            Some(expected) => {
                let (status, code) = emulator.expect_error("PutWebhook", &request).await;
                assert_eq!(
                    (status, code.as_str()),
                    (400, expected),
                    "case {case:?}: a {}-character jsonPath is past the modelled maximum",
                    json_path.len()
                );
            }
        }
    }
}

/// `WebhookAuthConfiguration` documents which property each authentication type
/// uses — "For GITHUB_HMAC, only the SecretToken property must be set", "For IP, only
/// the AllowedIPRange property must be set. This property must be set to a valid CIDR
/// range", and UNAUTHENTICATED "accepts all webhook trigger requests regardless of
/// origin" — so a configuration that does not match the type is refused with
/// `InvalidWebhookAuthenticationParametersException`.
#[tokio::test]
async fn authentication_parameters_must_match_the_authentication_type() {
    const REFUSED: Option<&str> = Some("InvalidWebhookAuthenticationParametersException");
    // (case, authentication, authenticationConfiguration, expected error or None)
    let cases: Vec<(&str, &str, Value, Option<&str>)> = vec![
        (
            "GITHUB_HMAC with its secret token",
            "GITHUB_HMAC",
            json!({"SecretToken": SECRET_TOKEN}),
            None,
        ),
        (
            "GITHUB_HMAC with no secret token",
            "GITHUB_HMAC",
            json!({}),
            REFUSED,
        ),
        (
            "GITHUB_HMAC also carrying an IP range",
            "GITHUB_HMAC",
            json!({"SecretToken": SECRET_TOKEN, "AllowedIPRange": "10.0.0.0/8"}),
            REFUSED,
        ),
        (
            "IP with an IPv4 CIDR range",
            "IP",
            json!({"AllowedIPRange": "10.0.0.0/8"}),
            None,
        ),
        // Each published bound of the IPv4 prefix length, and one past it.
        (
            "IP with the shortest IPv4 prefix",
            "IP",
            json!({"AllowedIPRange": "0.0.0.0/0"}),
            None,
        ),
        (
            "IP with the longest IPv4 prefix",
            "IP",
            json!({"AllowedIPRange": "192.0.2.1/32"}),
            None,
        ),
        (
            "IP with an IPv4 prefix past 32",
            "IP",
            json!({"AllowedIPRange": "192.0.2.1/33"}),
            REFUSED,
        ),
        // The same bounds for IPv6, whose prefix runs to 128.
        (
            "IP with the longest IPv6 prefix",
            "IP",
            json!({"AllowedIPRange": "2001:db8::1/128"}),
            None,
        ),
        (
            "IP with an IPv6 prefix past 128",
            "IP",
            json!({"AllowedIPRange": "2001:db8::1/129"}),
            REFUSED,
        ),
        (
            "IP with an address but no prefix",
            "IP",
            json!({"AllowedIPRange": "10.0.0.0"}),
            REFUSED,
        ),
        (
            "IP with something that is not an address",
            "IP",
            json!({"AllowedIPRange": "not-an-address/8"}),
            REFUSED,
        ),
        ("IP with no range at all", "IP", json!({}), REFUSED),
        (
            "IP also carrying a secret token",
            "IP",
            json!({"AllowedIPRange": "10.0.0.0/8", "SecretToken": SECRET_TOKEN}),
            REFUSED,
        ),
        (
            "UNAUTHENTICATED with neither property",
            "UNAUTHENTICATED",
            json!({}),
            None,
        ),
        (
            "UNAUTHENTICATED carrying a secret token",
            "UNAUTHENTICATED",
            json!({"SecretToken": SECRET_TOKEN}),
            REFUSED,
        ),
        (
            "UNAUTHENTICATED carrying an IP range",
            "UNAUTHENTICATED",
            json!({"AllowedIPRange": "10.0.0.0/8"}),
            REFUSED,
        ),
    ];

    for (case, authentication, configuration, expected_error) in cases {
        let emulator = Emulator::new();
        emulator.create_pipeline("demo").await;
        let request = json!({"webhook": definition(
            "hook",
            "demo",
            authentication,
            configuration.clone(),
            json!([{"jsonPath": "$.ref", "matchEquals": "main"}]),
        )});

        match expected_error {
            None => {
                let put = emulator.ok("PutWebhook", &request).await;
                assert_eq!(
                    put["webhook"]["definition"]["authentication"],
                    json!(authentication),
                    "case {case:?}: an accepted configuration is stored, got {put}"
                );
            }
            Some(expected) => {
                let (status, code) = emulator.expect_error("PutWebhook", &request).await;
                assert_eq!(
                    (status, code.as_str()),
                    (400, expected),
                    "case {case:?}: unexpected refusal for {configuration}"
                );
                let listed = emulator.list_webhooks().await;
                assert!(
                    listed.is_empty(),
                    "case {case:?}: a refused PutWebhook must store no webhook, got {listed:?}"
                );
            }
        }
    }
}

/// A webhook is defined over a pipeline NAME: `PutWebhook` stores the definition it is
/// sent whether or not the account holds that pipeline, so a webhook defined before its
/// pipeline exists — or outliving it — is stored and reported like any other. The rows
/// differ in how the name relates to a held pipeline: absent, differing only in letter
/// case, and held.
#[tokio::test]
async fn a_webhook_is_stored_whatever_its_target_pipeline_names() {
    let emulator = Emulator::new();
    emulator.create_pipeline("demo").await;

    for target in ["missing", "Demo", "demo"] {
        let put = emulator
            .ok(
                "PutWebhook",
                &json!({"webhook": github_definition("hook", target)}),
            )
            .await;
        assert_eq!(
            put["webhook"]["definition"]["targetPipeline"],
            json!(target),
            "target pipeline {target:?}: the definition is stored as sent, got {put}"
        );
        // And it is the webhook the store holds: the read path reports the same one.
        let listed = emulator.list_webhooks().await;
        assert_eq!(
            listed,
            vec![put["webhook"].clone()],
            "target pipeline {target:?}: ListWebhooks reports the stored webhook, got \
             {listed:?}"
        );
    }
}

/// Defining a webhook whose name already exists replaces its definition in place: the
/// URL and the ARN were minted once and keep identifying the webhook, and the store
/// holds one webhook rather than two.
#[tokio::test]
async fn redefining_a_webhook_keeps_its_url_and_arn() {
    let emulator = Emulator::new();
    emulator.create_pipeline("demo").await;
    emulator.create_pipeline("other").await;

    let first = emulator
        .ok(
            "PutWebhook",
            &json!({
                "webhook": github_definition("hook", "demo"),
                "tags": [{"key": "team", "value": "build"}],
            }),
        )
        .await;
    let url = first["webhook"]["url"].clone();
    let arn = first["webhook"]["arn"].clone();

    // Each row changes something about the definition; the identity must not move.
    let revisions: Vec<(&str, Value, Option<Value>)> = vec![
        (
            "a different target pipeline and authentication",
            definition(
                "hook",
                "other",
                "UNAUTHENTICATED",
                json!({}),
                json!([{"jsonPath": "$.action", "matchEquals": "opened"}]),
            ),
            None,
        ),
        (
            "a different filter set and new tags",
            definition(
                "hook",
                "demo",
                "IP",
                json!({"AllowedIPRange": "198.51.100.0/24"}),
                json!([]),
            ),
            Some(json!([{"key": "env", "value": "prod"}])),
        ),
    ];

    for (case, webhook, tags) in revisions {
        let mut request = json!({"webhook": webhook});
        if let Some(tags) = &tags {
            request["tags"] = tags.clone();
        }
        let put = emulator.ok("PutWebhook", &request).await;
        assert_eq!(
            (put["webhook"]["url"].clone(), put["webhook"]["arn"].clone()),
            (url.clone(), arn.clone()),
            "case {case:?}: redefining a webhook keeps its URL and ARN, got {put}"
        );
        assert_eq!(
            put["webhook"]["definition"], webhook,
            "case {case:?}: the new definition replaces the old one, got {put}"
        );
        // A redefinition states the whole webhook, tags included: the webhook carries
        // exactly the tags THIS request applied, and none when it applies none.
        let expected_tags = tags.unwrap_or_else(|| json!([]));
        assert_eq!(
            put["webhook"]["tags"], expected_tags,
            "case {case:?}: unexpected tags, got {put}"
        );

        let listed = emulator.list_webhooks().await;
        assert_eq!(
            listed,
            vec![put["webhook"].clone()],
            "case {case:?}: the store holds exactly the redefined webhook, got {listed:?}"
        );
    }
}

/// A tag set no resource can carry is refused with the error the model declares for
/// it, and nothing is stored.
#[tokio::test]
async fn a_tag_set_no_resource_can_carry_is_refused() {
    // The tag-count bound is AWS's documented "maximum number of tags per resource:
    // 50" (`state::MAX_TAGS_PER_RESOURCE`), tested at the bound and one past it.
    let fifty: Vec<Value> = (0..50)
        .map(|index| json!({"key": format!("k{index}"), "value": format!("v{index}")}))
        .collect();
    let fifty_one: Vec<Value> = (0..51)
        .map(|index| json!({"key": format!("k{index}"), "value": format!("v{index}")}))
        .collect();
    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        ("no tags at all", json!([]), None),
        (
            "fifty tags, the documented maximum",
            Value::Array(fifty),
            None,
        ),
        (
            "fifty-one tags, one past the maximum",
            Value::Array(fifty_one),
            Some("TooManyTagsException"),
        ),
        (
            "a key in the reserved aws: namespace",
            json!([{"key": "aws:created-by", "value": "me"}]),
            Some("InvalidTagsException"),
        ),
        (
            "the reserved namespace in another letter case",
            json!([{"key": "AWS:CreatedBy", "value": "me"}]),
            Some("InvalidTagsException"),
        ),
        (
            "the same key twice",
            json!([{"key": "team", "value": "build"}, {"key": "team", "value": "ship"}]),
            Some("InvalidTagsException"),
        ),
    ];

    for (case, tags, expected_error) in cases {
        let emulator = Emulator::new();
        emulator.create_pipeline("demo").await;
        let request = json!({"webhook": github_definition("hook", "demo"), "tags": tags});

        match expected_error {
            None => {
                let put = emulator.ok("PutWebhook", &request).await;
                assert_eq!(
                    put["webhook"].get("tags").cloned(),
                    Some(tags.clone()),
                    "case {case:?}: an accepted tag set is reported as applied, got {put}"
                );
            }
            Some(expected) => {
                let (status, code) = emulator.expect_error("PutWebhook", &request).await;
                assert_eq!(
                    (status, code.as_str()),
                    (400, expected),
                    "case {case:?}: unexpected refusal"
                );
                let listed = emulator.list_webhooks().await;
                assert!(
                    listed.is_empty(),
                    "case {case:?}: a refused PutWebhook must store no webhook, got {listed:?}"
                );
            }
        }
    }
}

/// `ListWebhooks` pages through the stored webhooks in name order: every webhook is
/// reported exactly once across the pages, each page is at most `MaxResults` long, and
/// the last page carries no token.
#[tokio::test]
async fn list_webhooks_pages_through_every_stored_webhook() {
    let emulator = Emulator::new();
    emulator.create_pipeline("demo").await;
    let names: Vec<String> = (0..7).map(|index| format!("hook-{index}")).collect();
    for name in &names {
        emulator
            .ok(
                "PutWebhook",
                &json!({"webhook": github_definition(name, "demo")}),
            )
            .await;
    }

    // `MaxResults` is modelled `@range(min: 1, max: 100)`; the rows span a page
    // smaller than, equal to and larger than the stored set, plus the request that
    // names no page size at all.
    for page_size in [Some(1), Some(3), Some(7), Some(100), None] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({});
            if let Some(page_size) = page_size {
                request["MaxResults"] = json!(page_size);
            }
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let page = emulator.ok("ListWebhooks", &request).await;
            let webhooks = page["webhooks"].as_array().expect("a webhooks list").clone();
            if let Some(limit) = page_size {
                assert!(
                    webhooks.len() <= limit as usize,
                    "case {page_size:?}: a page holds at most {limit} webhooks, got {}",
                    webhooks.len()
                );
            }
            for webhook in &webhooks {
                seen.push(
                    webhook["definition"]["name"]
                        .as_str()
                        .expect("a webhook name")
                        .to_string(),
                );
            }
            token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            assert!(
                pages <= names.len() + 1,
                "case {page_size:?}: paging did not terminate after {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, names,
            "case {page_size:?}: every webhook is reported once, in name order"
        );
        assert_eq!(
            seen.iter().collect::<BTreeSet<_>>().len(),
            names.len(),
            "case {page_size:?}: no webhook is reported twice, got {seen:?}"
        );
    }

    // A token this listing never issued is refused: not base64 at all, base64 of
    // text carrying no cursor kind ("demo"), and a cursor of another listing's kind
    // (base64 of "pipeline:demo").
    for token in ["not-a-token", "ZGVtbw==", "cGlwZWxpbmU6ZGVtbw=="] {
        let (status, code) = emulator
            .expect_error("ListWebhooks", &json!({"NextToken": token}))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidNextTokenException"),
            "token {token:?} was not issued by ListWebhooks"
        );
    }
}

/// The region the request is signed for is the one the webhook's ARN and trigger URL
/// carry, and an unsigned request falls back to the emulator's default region.
#[tokio::test]
async fn the_signed_region_is_the_one_in_the_arn_and_the_url() {
    for region in ["us-east-1", "eu-west-2", "ap-southeast-1"] {
        let emulator = Emulator::new();
        emulator.create_pipeline("demo").await;
        let (status, put) = emulator
            .call_in(
                region,
                "PutWebhook",
                &json!({"webhook": github_definition("hook", "demo")}),
            )
            .await;
        assert_eq!(
            status, 200,
            "region {region:?}: expected HTTP 200, got {put}"
        );
        assert_eq!(
            put["webhook"]["arn"],
            json!(format!(
                "arn:aws:codepipeline:{region}:{ACCOUNT_ID}:webhook:hook"
            )),
            "region {region:?}: the ARN carries the signed region, got {put}"
        );
        let url = put["webhook"]["url"].as_str().unwrap_or_default();
        assert!(
            url.starts_with(&format!(
                "https://emulator.invalid/_aws/codepipeline-webhooks/{region}/trigger?t="
            )),
            "region {region:?}: the trigger URL carries the signed region, got {url:?}"
        );
    }
}

/// Two webhooks never share a trigger URL: the URL is what a third party calls, so it
/// identifies exactly one webhook.
#[tokio::test]
async fn every_webhook_gets_its_own_trigger_url() {
    let emulator = Emulator::new();
    emulator.create_pipeline("demo").await;
    let mut urls = BTreeSet::new();
    for index in 0..16 {
        let name = format!("hook-{index}");
        let put = emulator
            .ok(
                "PutWebhook",
                &json!({"webhook": github_definition(&name, "demo")}),
            )
            .await;
        let url = put["webhook"]["url"]
            .as_str()
            .expect("a trigger URL")
            .to_string();
        assert!(
            urls.insert(url.clone()),
            "webhook {name}: trigger URL {url:?} was already used by another webhook"
        );
    }
}
