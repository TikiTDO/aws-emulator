// SPDX-License-Identifier: CC0-1.0

//! The CodePipeline webhook TRIGGER plane — the wire `ListWebhookItem$url` publishes —
//! end to end over the router: `PutWebhook` mints the URL, a `POST` to that URL starts
//! the target pipeline when the request satisfies the webhook's authentication and
//! filters, and starts nothing when it does not.
//!
//! Every assertion goes through the router, so nothing here can pass by reading a
//! handler's internals: the URL the control plane published is the address the POST is
//! sent to, and an execution is read back through `ListPipelineExecutions`.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix OPERATIONS.md records for this service.
const TARGET_PREFIX: &str = "CodePipeline_20150709";

/// A `roleArn` satisfying `RoleArn`'s modelled pattern.
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/service-role/cp";

/// The secret token the GITHUB_HMAC cases sign their payload with.
const SECRET_TOKEN: &str = "s3cr3t-github-token";

/// A secret no webhook here was defined with, which is what a payload signed by the wrong
/// sender proves.
const WRONG_SECRET: &str = "not-the-github-token";

/// The branch the pipeline's source action is configured for, which is what
/// `matchEquals`'s `{Branch}` placeholder resolves to.
const BRANCH: &str = "main";

/// The payload a provider sends for a push to that branch.
fn push_payload(branch: &str) -> String {
    json!({"ref": format!("refs/heads/{branch}"), "repository": {"name": "demo"}}).to_string()
}

/// That same payload padded to roughly `bytes` bytes, so a row can sit either side of the
/// bound the trigger plane reads a document within (`MAX_TRIGGER_PAYLOAD_BYTES`, 1 MiB):
/// past it no filter is evaluated, and the trigger starts nothing.
fn padded_payload(bytes: usize) -> String {
    json!({"ref": format!("refs/heads/{BRANCH}"), "pad": "x".repeat(bytes)}).to_string()
}

/// That same payload carrying `nodes` further nodes, so a row can sit either side of the
/// NODE bound the plane evaluates a document within (`MAX_PAYLOAD_NODES`, 4096) — the figure
/// that bounds what one JsonPath expression costs, which a byte count cannot.
fn wide_payload(nodes: usize) -> String {
    let items: Vec<Value> = (0..nodes).map(|index| json!(index)).collect();
    json!({"ref": format!("refs/heads/{BRANCH}"), "pad": items}).to_string()
}

/// That same payload with a chain nested `depth` deep, so a row can sit past the DEPTH bound
/// the plane evaluates a document within (`MAX_PAYLOAD_DEPTH`, 16).
fn deep_payload(depth: usize) -> String {
    let mut nested = json!("leaf");
    for _ in 0..depth {
        nested = json!({"next": nested});
    }
    json!({"ref": format!("refs/heads/{BRANCH}"), "pad": nested}).to_string()
}

/// One emulator: the shared engine, the service mounted on it, and the router that
/// dispatches to it — exactly how `core/router` assembles the production emulator.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        Self::served_at(None)
    }

    /// An emulator whose published base URL is `base` — what the data port's own URL is
    /// for a served emulator, and `None` for one served on no port at all.
    fn served_at(base: Option<&str>) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        if let Some(base) = base {
            router.set_url_base(
                smithy_server_runtime::EndpointUrl::parse(base)
                    .unwrap_or_else(|| panic!("{base} is an endpoint base")),
            );
        }
        router.register_service(
            service_com_amazonaws_codepipeline_codepipeline_20150709::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `operation` in `region`, optionally addressed to `host` — the endpoint the
    /// caller reached this emulator at.
    async fn call_in(
        &self,
        region: &str,
        host: Option<&str>,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/codepipeline/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
        );
        let mut request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("authorization", authorization);
        if let Some(host) = host {
            request = request.header("host", host);
        }
        let response = self
            .router
            .dispatch(
                request
                    .body(bytes::Bytes::from(body.to_string()))
                    .expect("a valid request"),
            )
            .await;
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

    async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, response) = self.call_in("us-east-1", None, operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {response}"
        );
        response
    }

    /// Send `method` to `url` — a URL this service PUBLISHED — with `headers` and `body`,
    /// the way a third party dials a webhook: no AWS credential of any kind.
    async fn dial(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, String)],
        body: &str,
    ) -> (u16, String) {
        let mut request = http::Request::builder()
            .method(method)
            .uri(url)
            .header("content-type", "application/json");
        for (name, value) in headers {
            request = request.header(*name, value.as_str());
        }
        let response = self
            .router
            .dispatch(
                request
                    .body(bytes::Bytes::from(body.to_string()))
                    .expect("a valid request"),
            )
            .await;
        let status = response.status().as_u16();
        let text = String::from_utf8_lossy(&response.into_body()).to_string();
        (status, text)
    }

    /// POST `body` to `url` with `headers`, which is the one call a trigger URL
    /// describes.
    async fn trigger(&self, url: &str, headers: &[(&str, String)], body: &str) -> (u16, String) {
        self.dial("POST", url, headers, body).await
    }

    /// Store a pipeline named `name` whose source action is `Custom`-owned (so an
    /// execution that reaches it waits for a job worker instead of being carried on by
    /// the clock) and configured for `BRANCH`.
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
                                "owner": "Custom",
                                "provider": "MySource",
                                "version": "1",
                            },
                            "configuration": {"Branch": BRANCH, "Repo": "demo"},
                            "outputArtifacts": [{"name": "SourceOut"}],
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

    /// Define `webhook` and answer the URL `PutWebhook` published for it.
    async fn put_webhook(&self, webhook: &Value) -> String {
        self.put_webhook_in("us-east-1", None, webhook).await
    }

    /// Define `webhook` through a request signed for `region` and addressed to `host`.
    async fn put_webhook_in(&self, region: &str, host: Option<&str>, webhook: &Value) -> String {
        let (status, put) = self
            .call_in(region, host, "PutWebhook", &json!({"webhook": webhook}))
            .await;
        assert_eq!(status, 200, "PutWebhook: expected HTTP 200, got {put}");
        put["webhook"]["url"]
            .as_str()
            .unwrap_or_else(|| panic!("PutWebhook reports a trigger URL, got {put}"))
            .to_string()
    }

    /// The webhook named `name`, as `ListWebhooks` reports it.
    async fn webhook(&self, name: &str) -> Value {
        let listed = self.ok("ListWebhooks", &json!({})).await;
        listed["webhooks"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|item| item["definition"]["name"] == json!(name))
            .cloned()
            .unwrap_or_else(|| panic!("ListWebhooks reports the webhook {name}, got {listed}"))
    }

    /// How many executions of `pipeline` carry the `Webhook` trigger type — the runs a
    /// POST to a trigger URL started, and nothing else.
    async fn webhook_runs(&self, pipeline: &str) -> usize {
        let listed = self
            .ok("ListPipelineExecutions", &json!({"pipelineName": pipeline}))
            .await;
        listed["pipelineExecutionSummaries"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|summary| summary["trigger"]["triggerType"] == json!("Webhook"))
            .count()
    }
}

/// A `WebhookDefinition` with the parts each table row varies.
fn definition(name: &str, authentication: &str, configuration: Value, filters: Value) -> Value {
    json!({
        "name": name,
        "targetPipeline": "demo",
        "targetAction": "Src",
        "filters": filters,
        "authentication": authentication,
        "authenticationConfiguration": configuration,
    })
}

/// A GITHUB_HMAC webhook filtered on the pushed ref.
fn github_definition(name: &str, filters: Value) -> Value {
    definition(
        name,
        "GITHUB_HMAC",
        json!({"SecretToken": SECRET_TOKEN}),
        filters,
    )
}

/// The signature header a provider sends for `body`, computed the way GitHub computes it:
/// the HMAC of the payload under the webhook's secret, written `<algorithm>=<hex>`.
fn signature(algorithm: &str, secret: &str, body: &str) -> (&'static str, String) {
    use ::hmac::Mac as _;
    match algorithm {
        "sha1" => {
            let mut mac =
                <::hmac::Hmac<::sha1::Sha1> as ::hmac::Mac>::new_from_slice(secret.as_bytes())
                    .expect("HMAC takes a key of any length");
            mac.update(body.as_bytes());
            (
                "x-hub-signature",
                format!("sha1={}", ::hex::encode(mac.finalize().into_bytes())),
            )
        }
        "sha256" => {
            let mut mac =
                <::hmac::Hmac<::sha2::Sha256> as ::hmac::Mac>::new_from_slice(secret.as_bytes())
                    .expect("HMAC takes a key of any length");
            mac.update(body.as_bytes());
            (
                "x-hub-signature-256",
                format!("sha256={}", ::hex::encode(mac.finalize().into_bytes())),
            )
        }
        other => panic!("{other} is not a digest a provider signs with"),
    }
}

/// `ListWebhookItem$url` is "a unique URL generated by CodePipeline. When a POST request
/// is made to this URL, the defined pipeline is started", so the URL has to name an
/// address THIS emulator answers on — never a host synthesized from the Region — and that
/// address has to serve the trigger.
///
/// The rows span what the addressing generalises over: the endpoint a caller reached this
/// emulator at (an address with a port, a loopback name, a bracketed IPv6 literal), a
/// caller that named no endpoint on an emulator published at its data port, one on an
/// emulator served on no port at all, and the Region the definition was signed for.
#[tokio::test]
async fn the_published_trigger_url_is_an_address_this_emulator_serves() {
    struct Row {
        case: &'static str,
        /// The base URL the emulator publishes, when it is served on a port.
        base: Option<&'static str>,
        /// The endpoint the defining request addressed.
        host: Option<&'static str>,
        region: &'static str,
        /// The `host:port` the published URL must carry.
        authority: &'static str,
    }
    let rows = [
        Row {
            case: "a caller on the loopback data port",
            base: None,
            host: Some("127.0.0.1:37915"),
            region: "us-west-2",
            authority: "127.0.0.1:37915",
        },
        Row {
            case: "a caller using a loopback name",
            base: None,
            host: Some("localhost:9297"),
            region: "us-east-1",
            authority: "localhost:9297",
        },
        Row {
            case: "a caller on an IPv6 loopback",
            base: None,
            host: Some("[::1]:9297"),
            region: "eu-west-2",
            authority: "[::1]:9297",
        },
        Row {
            case: "an in-process caller on a served emulator",
            base: Some("https://127.0.0.1:9443"),
            host: None,
            region: "ap-southeast-1",
            authority: "127.0.0.1:9443",
        },
        Row {
            case: "an emulator served on no port",
            base: None,
            host: None,
            region: "us-east-2",
            authority: "emulator.invalid",
        },
    ];
    for row in rows {
        let Row {
            case,
            base,
            host,
            region,
            authority,
        } = row;
        let emulator = Emulator::served_at(base);
        emulator.create_pipeline("demo").await;
        let url = emulator
            .put_webhook_in(
                region,
                host,
                &github_definition("hook", json!([{"jsonPath": "$.ref"}])),
            )
            .await;

        let expected_prefix =
            format!("https://{authority}/_aws/codepipeline-webhooks/{region}/trigger?t=");
        assert!(
            url.starts_with(&expected_prefix) && url.ends_with("&v=1"),
            "case {case:?}: expected a trigger URL under {expected_prefix:?}, got {url:?}"
        );
        for foreign in ["amazonaws.com", "webhooks.aws"] {
            assert!(
                !url.contains(foreign),
                "case {case:?}: the published URL names the host {foreign:?}, \
                 which this emulator does not serve: {url:?}"
            );
        }
        // The same URL comes back out of the listing: storing a reachable URL is not
        // enough if the projection re-derives one.
        let listed = emulator.webhook("hook").await;
        assert_eq!(
            listed["url"],
            json!(url),
            "case {case:?}: ListWebhooks reports another URL than PutWebhook published"
        );

        // And the address is SERVED: the POST a provider makes to it starts the pipeline.
        let payload = push_payload(BRANCH);
        let signed = signature("sha1", SECRET_TOKEN, &payload);
        let (status, answer) = emulator
            .trigger(&url, &[(signed.0, signed.1)], &payload)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the published URL answered {status} ({answer:?}) \
             to the POST it describes"
        );
        let runs = emulator.webhook_runs("demo").await;
        assert_eq!(
            runs, 1,
            "case {case:?}: expected the POST to start one Webhook-triggered execution, got {runs}"
        );
    }
}

/// "When a POST request is made to this URL, the defined pipeline is started as long as
/// the body of the post request satisfies the defined authentication and filtering" —
/// and "otherwise, the request is ignored", so a request that does not satisfy them is
/// acknowledged and starts nothing.
///
/// The rows span what that rule generalises over: each `WebhookAuthenticationType` the
/// model declares (`GITHUB_HMAC`, `IP`, `UNAUTHENTICATED`), each digest a provider signs
/// a payload with, a signature computed with the wrong secret, a malformed and an absent
/// signature, an address inside and outside an `AllowedIPRange`, a payload the filter
/// selects nothing from, a payload whose selected value differs, a payload either side of
/// each bound the plane reads and evaluates a document within (bytes, nodes, depth), a
/// filter either side of the descendant-segment bound, a `matchEquals` carrying the
/// documented `{Branch}` placeholder, five filters of which one fails, and a body that is
/// not a JSON document at all.
#[tokio::test]
async fn a_trigger_starts_the_pipeline_exactly_when_authentication_and_filters_are_satisfied() {
    struct Row {
        case: &'static str,
        webhook: Value,
        /// The headers the provider's POST carries, beside any signature.
        headers: Vec<(&'static str, String)>,
        /// Which digest to sign the payload under, and with which secret.
        signed: Option<(&'static str, &'static str)>,
        payload: String,
        /// Whether the POST starts an execution of the target pipeline.
        starts: bool,
    }
    let any_ref = json!([{"jsonPath": "$.ref"}]);
    let matching_ref = json!([{"jsonPath": "$.ref", "matchEquals": "refs/heads/main"}]);
    let rows = vec![
        Row {
            case: "GITHUB_HMAC, signed with the webhook's secret under sha1",
            webhook: github_definition("hook", matching_ref.clone()),
            headers: Vec::new(),
            signed: Some(("sha1", SECRET_TOKEN)),
            payload: push_payload(BRANCH),
            starts: true,
        },
        Row {
            case: "GITHUB_HMAC, signed with the webhook's secret under sha256",
            webhook: github_definition("hook", matching_ref.clone()),
            headers: Vec::new(),
            signed: Some(("sha256", SECRET_TOKEN)),
            payload: push_payload(BRANCH),
            starts: true,
        },
        Row {
            case: "GITHUB_HMAC, signed with the wrong secret",
            webhook: github_definition("hook", matching_ref.clone()),
            headers: Vec::new(),
            signed: Some(("sha1", WRONG_SECRET)),
            payload: push_payload(BRANCH),
            starts: false,
        },
        Row {
            case: "GITHUB_HMAC, carrying no signature at all",
            webhook: github_definition("hook", matching_ref.clone()),
            headers: Vec::new(),
            signed: None,
            payload: push_payload(BRANCH),
            starts: false,
        },
        Row {
            case: "GITHUB_HMAC, carrying a signature that is not a digest",
            webhook: github_definition("hook", matching_ref.clone()),
            headers: vec![("x-hub-signature", "sha1=not-hex".to_string())],
            signed: None,
            payload: push_payload(BRANCH),
            starts: false,
        },
        Row {
            case: "GITHUB_HMAC, signed over a payload the filter does not match",
            webhook: github_definition("hook", matching_ref.clone()),
            headers: Vec::new(),
            signed: Some(("sha1", SECRET_TOKEN)),
            payload: push_payload("dev"),
            starts: false,
        },
        Row {
            case: "GITHUB_HMAC, signed over a payload the filter selects nothing from",
            webhook: github_definition("hook", any_ref.clone()),
            headers: Vec::new(),
            signed: Some(("sha1", SECRET_TOKEN)),
            payload: json!({"repository": {"name": "demo"}}).to_string(),
            starts: false,
        },
        Row {
            case: "GITHUB_HMAC, signed over a body that is not a JSON document",
            webhook: github_definition("hook", any_ref.clone()),
            headers: Vec::new(),
            signed: Some(("sha1", SECRET_TOKEN)),
            payload: "not a document".to_string(),
            starts: false,
        },
        Row {
            // "All defined rules must pass": a definition stating no rule has none to
            // fail, so the body is not evaluated at all.
            case: "GITHUB_HMAC, no filters at all, over a body that is not a document",
            webhook: github_definition("hook", json!([])),
            headers: Vec::new(),
            signed: Some(("sha1", SECRET_TOKEN)),
            payload: "not a document".to_string(),
            starts: true,
        },
        Row {
            case: "GITHUB_HMAC, no filters at all, signed with the wrong secret",
            webhook: github_definition("hook", json!([])),
            headers: Vec::new(),
            signed: Some(("sha1", WRONG_SECRET)),
            payload: push_payload(BRANCH),
            starts: false,
        },
        Row {
            case: "GITHUB_HMAC, matchEquals resolving the target action's {Branch}",
            webhook: github_definition(
                "hook",
                json!([{"jsonPath": "$.ref", "matchEquals": "refs/heads/{Branch}"}]),
            ),
            headers: Vec::new(),
            signed: Some(("sha1", SECRET_TOKEN)),
            payload: push_payload(BRANCH),
            starts: true,
        },
        Row {
            case: "GITHUB_HMAC, matchEquals resolving {Branch} against another branch",
            webhook: github_definition(
                "hook",
                json!([{"jsonPath": "$.ref", "matchEquals": "refs/heads/{Branch}"}]),
            ),
            headers: Vec::new(),
            signed: Some(("sha1", SECRET_TOKEN)),
            payload: push_payload("dev"),
            starts: false,
        },
        Row {
            case: "GITHUB_HMAC, five filters that all pass",
            webhook: github_definition(
                "hook",
                json!([
                    {"jsonPath": "$.ref", "matchEquals": "refs/heads/main"},
                    {"jsonPath": "$.repository.name", "matchEquals": "demo"},
                    {"jsonPath": "$.ref"},
                    {"jsonPath": "$.repository"},
                    {"jsonPath": "$.repository.name", "matchEquals": "{Repo}"},
                ]),
            ),
            headers: Vec::new(),
            signed: Some(("sha1", SECRET_TOKEN)),
            payload: push_payload(BRANCH),
            starts: true,
        },
        Row {
            case: "GITHUB_HMAC, five filters of which one fails",
            webhook: github_definition(
                "hook",
                json!([
                    {"jsonPath": "$.ref", "matchEquals": "refs/heads/main"},
                    {"jsonPath": "$.repository.name", "matchEquals": "demo"},
                    {"jsonPath": "$.ref"},
                    {"jsonPath": "$.repository"},
                    {"jsonPath": "$.repository.name", "matchEquals": "other"},
                ]),
            ),
            headers: Vec::new(),
            signed: Some(("sha1", SECRET_TOKEN)),
            payload: push_payload(BRANCH),
            starts: false,
        },
        Row {
            case: "UNAUTHENTICATED, which has nothing to prove",
            webhook: definition("hook", "UNAUTHENTICATED", json!({}), matching_ref.clone()),
            headers: Vec::new(),
            signed: None,
            payload: push_payload(BRANCH),
            starts: true,
        },
        Row {
            case: "UNAUTHENTICATED, with a payload the filter does not match",
            webhook: definition("hook", "UNAUTHENTICATED", json!({}), matching_ref.clone()),
            headers: Vec::new(),
            signed: None,
            payload: push_payload("dev"),
            starts: false,
        },
        Row {
            case: "UNAUTHENTICATED, with a matching payload just inside the read bound",
            webhook: definition("hook", "UNAUTHENTICATED", json!({}), matching_ref.clone()),
            headers: Vec::new(),
            signed: None,
            payload: padded_payload(900 * 1024),
            starts: true,
        },
        Row {
            case: "UNAUTHENTICATED, with a matching payload past the read bound",
            webhook: definition("hook", "UNAUTHENTICATED", json!({}), matching_ref.clone()),
            headers: Vec::new(),
            signed: None,
            payload: padded_payload(1024 * 1024),
            starts: false,
        },
        Row {
            case: "UNAUTHENTICATED, with a matching payload inside the node bound",
            webhook: definition("hook", "UNAUTHENTICATED", json!({}), matching_ref.clone()),
            headers: Vec::new(),
            signed: None,
            payload: wide_payload(1_000),
            starts: true,
        },
        Row {
            case: "UNAUTHENTICATED, with a matching payload past the node bound",
            webhook: definition("hook", "UNAUTHENTICATED", json!({}), matching_ref.clone()),
            headers: Vec::new(),
            signed: None,
            payload: wide_payload(8_000),
            starts: false,
        },
        Row {
            case: "UNAUTHENTICATED, with a matching payload past the depth bound",
            webhook: definition("hook", "UNAUTHENTICATED", json!({}), matching_ref.clone()),
            headers: Vec::new(),
            signed: None,
            payload: deep_payload(24),
            starts: false,
        },
        Row {
            case: "UNAUTHENTICATED, with a filter inside the descendant-segment bound",
            webhook: definition(
                "hook",
                "UNAUTHENTICATED",
                json!({}),
                json!([{"jsonPath": "$..ref", "matchEquals": "refs/heads/main"}]),
            ),
            headers: Vec::new(),
            signed: None,
            payload: push_payload(BRANCH),
            starts: true,
        },
        Row {
            case: "UNAUTHENTICATED, with a filter past the descendant-segment bound",
            webhook: definition(
                "hook",
                "UNAUTHENTICATED",
                json!({}),
                json!([{"jsonPath": "$..*..*..*", "matchEquals": "refs/heads/main"}]),
            ),
            headers: Vec::new(),
            signed: None,
            payload: push_payload(BRANCH),
            starts: false,
        },
        Row {
            case: "IP, from an address inside the allowed range",
            webhook: definition(
                "hook",
                "IP",
                json!({"AllowedIPRange": "192.0.2.0/24"}),
                matching_ref.clone(),
            ),
            headers: vec![("x-forwarded-for", "192.0.2.17".to_string())],
            signed: None,
            payload: push_payload(BRANCH),
            starts: true,
        },
        Row {
            case: "IP, from an address outside the allowed range",
            webhook: definition(
                "hook",
                "IP",
                json!({"AllowedIPRange": "192.0.2.0/24"}),
                matching_ref.clone(),
            ),
            headers: vec![("x-forwarded-for", "198.51.100.17".to_string())],
            signed: None,
            payload: push_payload(BRANCH),
            starts: false,
        },
        Row {
            case: "IP, from a request that states no address",
            webhook: definition(
                "hook",
                "IP",
                json!({"AllowedIPRange": "192.0.2.0/24"}),
                matching_ref.clone(),
            ),
            headers: Vec::new(),
            signed: None,
            payload: push_payload(BRANCH),
            starts: false,
        },
        Row {
            case: "IP, where the range names one address",
            webhook: definition(
                "hook",
                "IP",
                json!({"AllowedIPRange": "192.0.2.17/32"}),
                matching_ref.clone(),
            ),
            headers: vec![("x-forwarded-for", "192.0.2.17, 203.0.113.9".to_string())],
            signed: None,
            payload: push_payload(BRANCH),
            starts: true,
        },
    ];
    for row in rows {
        let Row {
            case,
            webhook,
            mut headers,
            signed,
            payload,
            starts,
        } = row;
        let emulator = Emulator::new();
        emulator.create_pipeline("demo").await;
        let url = emulator.put_webhook(&webhook).await;
        if let Some((algorithm, secret)) = signed {
            let (name, value) = signature(algorithm, secret, &payload);
            headers.push((name, value));
        }

        let (status, answer) = emulator.trigger(&url, &headers, &payload).await;
        // Every delivered trigger is acknowledged: a provider is not told whether the
        // payload it signed satisfied the webhook.
        assert_eq!(
            status, 200,
            "case {case:?}: expected the trigger to be acknowledged with HTTP 200, \
             got {status} ({answer:?})"
        );

        let runs = emulator.webhook_runs("demo").await;
        let expected = usize::from(starts);
        assert_eq!(
            runs, expected,
            "case {case:?}: expected {expected} Webhook-triggered execution(s), got {runs}"
        );

        // "The date and time a webhook was last successfully triggered": recorded for a
        // trigger that started the pipeline, and absent while none has.
        let listed = emulator.webhook("hook").await;
        assert_eq!(
            listed.get("lastTriggered").is_some(),
            starts,
            "case {case:?}: lastTriggered should be present exactly when the trigger \
             started the pipeline, got {listed}"
        );
    }
}

/// The URL identifies ONE webhook of one account: a token no webhook carries names
/// nothing, a token addressed at another Region's trigger path names nothing, a deleted
/// webhook's URL serves nothing, and the only call the URL describes is a POST.
#[tokio::test]
async fn a_trigger_url_serves_only_the_webhook_it_names() {
    let emulator = Emulator::new();
    emulator.create_pipeline("demo").await;
    let url = emulator
        .put_webhook(&definition(
            "hook",
            "UNAUTHENTICATED",
            json!({}),
            json!([{"jsonPath": "$.ref"}]),
        ))
        .await;
    let payload = push_payload(BRANCH);

    // A token no webhook of this account carries.
    let unknown = url.replace("trigger?t=", "trigger?t=0-");
    let (status, answer) = emulator.trigger(&unknown, &[], &payload).await;
    assert_eq!(
        status, 404,
        "a URL whose token names no webhook: expected HTTP 404, got {status} ({answer:?})"
    );

    // The same token, addressed at another Region's trigger path: the pair names one
    // webhook, so the token alone is not a key.
    let elsewhere = url.replace(
        "/codepipeline-webhooks/us-east-1/",
        "/codepipeline-webhooks/eu-west-2/",
    );
    let (status, answer) = emulator.trigger(&elsewhere, &[], &payload).await;
    assert_eq!(
        status, 404,
        "a URL addressed at another Region: expected HTTP 404, got {status} ({answer:?})"
    );

    // A URL stating a form this plane does not publish is not one it minted.
    for version in ["&v=2", ""] {
        let other = format!("{}{version}", url.trim_end_matches("&v=1"));
        let (status, answer) = emulator.trigger(&other, &[], &payload).await;
        assert_eq!(
            status, 404,
            "a URL stating the version {version:?}: expected HTTP 404, \
             got {status} ({answer:?})"
        );
    }

    // Every method but POST: the published URL describes the one call a provider makes.
    for method in ["GET", "PUT", "DELETE"] {
        let (status, answer) = emulator.dial(method, &url, &[], &payload).await;
        assert_eq!(
            status, 405,
            "{method} to a trigger URL: expected HTTP 405, got {status} ({answer:?})"
        );
    }
    let runs = emulator.webhook_runs("demo").await;
    assert_eq!(
        runs, 0,
        "no request that named no webhook, and no method the URL does not describe, \
         may start an execution, got {runs}"
    );

    // The webhook's own POST does start one, which is what makes the refusals above
    // about the address rather than about the plane being absent.
    let (status, answer) = emulator.trigger(&url, &[], &payload).await;
    assert_eq!(
        status, 200,
        "the webhook's own trigger URL: expected HTTP 200, got {status} ({answer:?})"
    );
    let runs = emulator.webhook_runs("demo").await;
    assert_eq!(runs, 1, "expected one Webhook-triggered execution, got {runs}");

    // A webhook that no longer exists serves nothing, however many times the URL is
    // dialled.
    emulator
        .ok("DeleteWebhook", &json!({"name": "hook"}))
        .await;
    let (status, answer) = emulator.trigger(&url, &[], &payload).await;
    assert_eq!(
        status, 404,
        "a deleted webhook's URL: expected HTTP 404, got {status} ({answer:?})"
    );
    let runs = emulator.webhook_runs("demo").await;
    assert_eq!(
        runs, 1,
        "a deleted webhook's URL started another execution, got {runs} in all"
    );
}

/// A trigger is not a read: a POST that starts nothing leaves the store exactly as it
/// was, and one that starts an execution starts exactly one.
#[tokio::test]
async fn an_ignored_trigger_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    emulator.create_pipeline("demo").await;
    let url = emulator
        .put_webhook(&github_definition(
            "hook",
            json!([{"jsonPath": "$.ref", "matchEquals": "refs/heads/main"}]),
        ))
        .await;
    let payload = push_payload(BRANCH);
    let before = emulator.webhook("hook").await;

    // Three POSTs nothing can act on: a wrong secret, no signature, and a payload the
    // filter does not match.
    let wrong = signature("sha1", WRONG_SECRET, &payload);
    let other = push_payload("dev");
    let signed_other = signature("sha1", SECRET_TOKEN, &other);
    for (case, headers, body) in [
        ("a payload signed with the wrong secret", vec![wrong], &payload),
        ("a payload carrying no signature", Vec::new(), &payload),
        (
            "a signed payload the filter does not match",
            vec![signed_other],
            &other,
        ),
    ] {
        let (status, answer) = emulator.trigger(&url, &headers, body).await;
        assert_eq!(
            status, 200,
            "{case}: expected the trigger to be acknowledged with HTTP 200, \
             got {status} ({answer:?})"
        );
        let after = emulator.webhook("hook").await;
        assert_eq!(
            after, before,
            "{case}: the ignored trigger changed the stored webhook"
        );
        let runs = emulator.webhook_runs("demo").await;
        assert_eq!(runs, 0, "{case}: the ignored trigger started {runs} execution(s)");
    }

    // And the satisfied one starts exactly one execution.
    let signed = signature("sha256", SECRET_TOKEN, &payload);
    let (status, _) = emulator.trigger(&url, &[signed], &payload).await;
    assert_eq!(status, 200, "a satisfied trigger is acknowledged with HTTP 200");
    let runs = emulator.webhook_runs("demo").await;
    assert_eq!(runs, 1, "expected one Webhook-triggered execution, got {runs}");
}
