// SPDX-License-Identifier: CC0-1.0

//! What a `WebhookDefinition` is held to, and what resolves the pipeline it names,
//! over the wire the service actually speaks (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: CodePipeline_20150709.<Operation>`).
//!
//! The rules under test, each asserted over a SET of inputs:
//!
//! * `PutWebhook` "defines a webhook and returns a unique webhook URL": it stores the
//!   definition it is sent and resolves NOTHING about the pipeline the definition
//!   names, so a webhook whose `targetPipeline` the account does not hold — because it
//!   does not exist yet, or because it was deleted while the webhook stayed — is
//!   stored, reported and listed like any other;
//! * `WebhookAuthConfiguration` documents which property each
//!   `WebhookAuthenticationType` uses, so a configuration that is not EXACTLY that one
//!   property is refused with `InvalidWebhookAuthenticationParametersException` naming
//!   the single permitted property, and nothing is stored;
//! * the operations that connect a webhook to the external tool DO resolve that
//!   reference — the connection they make and break starts the webhook's target
//!   pipeline — so a dangling one is refused with `ValidationException` naming the
//!   webhook and the pipeline its definition references;
//! * `DeleteWebhook` stops the account holding the webhook, "returns successfully when
//!   trying to delete a webhook that is already deleted", and a webhook re-created
//!   under the same name "will have a different URL".
//!
//! The harness is the one `pipeline_lifecycle` owns, so every assertion goes through
//! the same router the production emulator builds.

use serde_json::{json, Value};

use service_com_amazonaws_codepipeline_codepipeline_20150709::state::{
    authentication_configuration_single_property_message,
    webhook_dangling_target_pipeline_message, AUTHENTICATION_CONFIGURATION_NO_PROPERTY_MESSAGE,
    AUTHENTICATION_GITHUB_HMAC, AUTHENTICATION_IP, AUTHENTICATION_UNAUTHENTICATED,
    AUTH_PROPERTY_ALLOWED_IP_RANGE, AUTH_PROPERTY_SECRET_TOKEN, REDACTED_SECRET_TOKEN,
};

use crate::pipeline_lifecycle::{declaration, Emulator};

/// The operations that connect a stored webhook to the external tool.
const REGISTER: &str = "RegisterWebhookWithThirdParty";
const DEREGISTER: &str = "DeregisterWebhookWithThirdParty";

/// The status every error these operations model defaults to.
const REFUSAL_STATUS: u16 = 400;

/// The error `PutWebhook` models for an authentication configuration that does not
/// match the authentication type.
const BAD_AUTH_PARAMETERS: &str = "InvalidWebhookAuthenticationParametersException";

/// A `SecretToken` satisfying `WebhookAuthConfigurationSecretToken`'s modelled 1..100
/// length.
const SECRET_TOKEN: &str = "s3cr3t-github-token";

/// The pipeline every case that needs a held one targets.
const HELD_PIPELINE: &str = "held";

/// A `WebhookDefinition` with the parts the tables vary.
fn definition(
    name: &str,
    target_pipeline: &str,
    authentication: &str,
    authentication_configuration: Value,
) -> Value {
    json!({
        "name": name,
        "targetPipeline": target_pipeline,
        "targetAction": "Src",
        "filters": [{"jsonPath": "$.ref", "matchEquals": "refs/heads/main"}],
        "authentication": authentication,
        "authenticationConfiguration": authentication_configuration,
    })
}

/// The GitHub HMAC definition the third-party operations serve.
fn github_definition(name: &str, target_pipeline: &str) -> Value {
    definition(
        name,
        target_pipeline,
        AUTHENTICATION_GITHUB_HMAC,
        json!({AUTH_PROPERTY_SECRET_TOKEN: SECRET_TOKEN}),
    )
}

/// An emulator holding exactly one pipeline, [`HELD_PIPELINE`].
async fn emulator() -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok("CreatePipeline", &json!({"pipeline": declaration(HELD_PIPELINE)}))
        .await;
    emulator
}

/// Every webhook the account holds, in the order `ListWebhooks` reports them.
async fn listed_webhooks(emulator: &Emulator) -> Vec<Value> {
    let listed = emulator.ok("ListWebhooks", &json!({})).await;
    listed["webhooks"]
        .as_array()
        .unwrap_or_else(|| panic!("ListWebhooks reports a webhooks list, got {listed}"))
        .clone()
}

/// RULE: `PutWebhook` stores the definition it is sent and resolves nothing about the
/// pipeline it names.
///
/// The rows differ in how `targetPipeline` relates to the pipelines the account holds —
/// held, never created, a held name in another letter case, and a name at
/// `PipelineName`'s modelled maximum length (1..100) — and in whether the request
/// applies tags. Every row is accepted, reported with its generated ARN and URL, and
/// read back through `ListWebhooks`.
#[tokio::test]
async fn a_definition_is_stored_without_resolving_its_target_pipeline() {
    // (case, targetPipeline, tags the request applies)
    let rows: Vec<(&str, String, Option<Value>)> = vec![
        (
            "a pipeline the account holds",
            HELD_PIPELINE.to_string(),
            Some(json!([{"key": "team", "value": "build"}])),
        ),
        (
            "a name no pipeline carries",
            "never-created".to_string(),
            None,
        ),
        (
            "a held name in another letter case",
            HELD_PIPELINE.to_uppercase(),
            Some(json!([{"key": "env", "value": "prod"}, {"key": "owner", "value": "a"}])),
        ),
        (
            "a name at PipelineName's maximum length",
            "p".repeat(100),
            None,
        ),
    ];

    for (case, target_pipeline, tags) in rows {
        let emulator = emulator().await;
        let mut request = json!({"webhook": github_definition("hook", &target_pipeline)});
        if let Some(tags) = &tags {
            request["tags"] = tags.clone();
        }
        let put = emulator.ok("PutWebhook", &request).await;
        let item = &put["webhook"];

        assert_eq!(
            item["definition"]["targetPipeline"],
            json!(target_pipeline),
            "case {case:?}: the definition is stored as sent, got {put}"
        );
        // The secret the caller supplied is redacted, and the service-owned members
        // are minted.
        assert_eq!(
            item["definition"]["authenticationConfiguration"][AUTH_PROPERTY_SECRET_TOKEN],
            json!(REDACTED_SECRET_TOKEN),
            "case {case:?}: the secret token is redacted, got {put}"
        );
        assert_eq!(
            item["arn"],
            json!("arn:aws:codepipeline:us-east-1:000000000000:webhook:hook"),
            "case {case:?}: unexpected webhook ARN, got {put}"
        );
        assert!(
            item["url"].as_str().is_some_and(|url| url
                .starts_with(
                    "https://emulator.invalid/_aws/codepipeline-webhooks/us-east-1/trigger?t="
                )
                && url.ends_with("&v=1")),
            "case {case:?}: expected a trigger URL on an address this emulator serves \
             (the unserved base is what an in-process router publishes), got {put}"
        );
        // The tags member is always reported, as an empty list for a webhook carrying
        // none.
        assert_eq!(
            item.get("tags").cloned(),
            Some(tags.clone().unwrap_or_else(|| json!([]))),
            "case {case:?}: the tags applied are reported, got {put}"
        );

        // The round trip: the store holds exactly the webhook that was put.
        let listed = listed_webhooks(&emulator).await;
        assert_eq!(
            listed,
            vec![item.clone()],
            "case {case:?}: ListWebhooks reports the stored webhook, got {listed:?}"
        );

        // Defining a webhook created no pipeline: the account still holds just the one
        // it was given.
        let pipelines = emulator.ok("ListPipelines", &json!({})).await;
        let names: Vec<&str> = pipelines["pipelines"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: a pipelines list, got {pipelines}"))
            .iter()
            .filter_map(|pipeline| pipeline["name"].as_str())
            .collect();
        assert_eq!(
            names,
            vec![HELD_PIPELINE],
            "case {case:?}: defining a webhook creates no pipeline, got {names:?}"
        );
    }
}

/// RULE, continued: the reference stays as it was written when the pipeline it names
/// goes away — deleting a pipeline changes no webhook.
#[tokio::test]
async fn deleting_a_pipeline_leaves_the_webhooks_that_reference_it_untouched() {
    let emulator = emulator().await;
    let put = emulator
        .ok(
            "PutWebhook",
            &json!({"webhook": github_definition("hook", HELD_PIPELINE)}),
        )
        .await;

    emulator
        .ok("DeletePipeline", &json!({"name": HELD_PIPELINE}))
        .await;

    let listed = listed_webhooks(&emulator).await;
    assert_eq!(
        listed,
        vec![put["webhook"].clone()],
        "the webhook is reported exactly as it was stored, got {listed:?}"
    );
}

/// RULE: an `authenticationConfiguration` must be exactly the one property its
/// `WebhookAuthenticationType` uses, and the refusal names that property.
///
/// The rows are every authentication type the model declares, each against every
/// configuration the shape can carry: the permitted property alone, the other property
/// alone, both, and none. The accepted rows are the ones AWS documents — "For
/// GITHUB_HMAC, only the SecretToken property must be set", "For IP, only the
/// AllowedIPRange property must be set", and UNAUTHENTICATED, which uses neither.
#[tokio::test]
async fn an_authentication_configuration_must_be_exactly_the_property_its_type_uses() {
    let secret = json!({AUTH_PROPERTY_SECRET_TOKEN: SECRET_TOKEN});
    let range = json!({AUTH_PROPERTY_ALLOWED_IP_RANGE: "10.0.0.0/8"});
    let both = json!({
        AUTH_PROPERTY_SECRET_TOKEN: SECRET_TOKEN,
        AUTH_PROPERTY_ALLOWED_IP_RANGE: "10.0.0.0/8",
    });
    let empty = json!({});

    // (case, authentication, configuration, the message the refusal carries or None
    // when the configuration is the one the type uses)
    let rows: Vec<(&str, &str, Value, Option<String>)> = vec![
        (
            "GITHUB_HMAC with its secret token",
            AUTHENTICATION_GITHUB_HMAC,
            secret.clone(),
            None,
        ),
        (
            "GITHUB_HMAC with no property at all",
            AUTHENTICATION_GITHUB_HMAC,
            empty.clone(),
            Some(authentication_configuration_single_property_message(
                AUTH_PROPERTY_SECRET_TOKEN,
            )),
        ),
        (
            "GITHUB_HMAC with the other type's property",
            AUTHENTICATION_GITHUB_HMAC,
            range.clone(),
            Some(authentication_configuration_single_property_message(
                AUTH_PROPERTY_SECRET_TOKEN,
            )),
        ),
        (
            "GITHUB_HMAC with its property and one more",
            AUTHENTICATION_GITHUB_HMAC,
            both.clone(),
            Some(authentication_configuration_single_property_message(
                AUTH_PROPERTY_SECRET_TOKEN,
            )),
        ),
        (
            "IP with its allowed range",
            AUTHENTICATION_IP,
            range.clone(),
            None,
        ),
        (
            "IP with no property at all",
            AUTHENTICATION_IP,
            empty.clone(),
            Some(authentication_configuration_single_property_message(
                AUTH_PROPERTY_ALLOWED_IP_RANGE,
            )),
        ),
        (
            "IP with the other type's property",
            AUTHENTICATION_IP,
            secret.clone(),
            Some(authentication_configuration_single_property_message(
                AUTH_PROPERTY_ALLOWED_IP_RANGE,
            )),
        ),
        (
            "IP with its property and one more",
            AUTHENTICATION_IP,
            both.clone(),
            Some(authentication_configuration_single_property_message(
                AUTH_PROPERTY_ALLOWED_IP_RANGE,
            )),
        ),
        (
            "UNAUTHENTICATED with no property at all",
            AUTHENTICATION_UNAUTHENTICATED,
            empty.clone(),
            None,
        ),
        (
            "UNAUTHENTICATED carrying a secret token",
            AUTHENTICATION_UNAUTHENTICATED,
            secret.clone(),
            Some(AUTHENTICATION_CONFIGURATION_NO_PROPERTY_MESSAGE.to_string()),
        ),
        (
            "UNAUTHENTICATED carrying an allowed range",
            AUTHENTICATION_UNAUTHENTICATED,
            range.clone(),
            Some(AUTHENTICATION_CONFIGURATION_NO_PROPERTY_MESSAGE.to_string()),
        ),
        (
            "UNAUTHENTICATED carrying both",
            AUTHENTICATION_UNAUTHENTICATED,
            both.clone(),
            Some(AUTHENTICATION_CONFIGURATION_NO_PROPERTY_MESSAGE.to_string()),
        ),
    ];

    for (case, authentication, configuration, expected_message) in rows {
        let emulator = emulator().await;
        let request = json!({"webhook": definition(
            "hook",
            HELD_PIPELINE,
            authentication,
            configuration.clone(),
        )});

        let (status, response) = emulator.call("PutWebhook", &request).await;
        let Some(expected) = expected_message else {
            assert_eq!(
                status, 200,
                "case {case:?}: the configuration the type uses is accepted, got {status} \
                 with {response}"
            );
            assert_eq!(
                response["webhook"]["definition"]["authentication"],
                json!(authentication),
                "case {case:?}: the accepted configuration is stored, got {response}"
            );
            continue;
        };
        assert_eq!(
            (
                status,
                response["__type"].as_str().unwrap_or_default().to_string()
            ),
            (REFUSAL_STATUS, BAD_AUTH_PARAMETERS.to_string()),
            "case {case:?}: expected the refusal the model declares for {configuration}, \
             got {status} with {response}"
        );
        assert_eq!(
            response["message"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?}, got {response}"
        );
        // A refused definition stores nothing.
        let listed = listed_webhooks(&emulator).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: a refused PutWebhook must store no webhook, got {listed:?}"
        );
    }

    // The wording itself, written out here so a reword of the handler's text fails this
    // suite rather than following it.
    for property in [AUTH_PROPERTY_SECRET_TOKEN, AUTH_PROPERTY_ALLOWED_IP_RANGE] {
        assert_eq!(
            authentication_configuration_single_property_message(property),
            format!(
                "Optional['authenticationConfig' should contain only one property: \
                 '{property}']"
            ),
            "the refusal names the single permitted property"
        );
    }
}

/// RULE: the third-party operations resolve the webhook's `targetPipeline`, so a
/// dangling reference is refused with `ValidationException` naming the webhook and the
/// pipeline it references — however the reference came to be dangling.
#[tokio::test]
async fn connecting_a_webhook_resolves_the_pipeline_its_definition_references() {
    // (case, the webhook's targetPipeline, whether the held pipeline is deleted first)
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "a pipeline that was never created",
            "never-created".to_string(),
            false,
        ),
        (
            "a pipeline deleted after the webhook was defined",
            HELD_PIPELINE.to_string(),
            true,
        ),
    ];

    for (case, target_pipeline, delete_pipeline) in rows {
        let emulator = emulator().await;
        emulator
            .ok(
                "PutWebhook",
                &json!({"webhook": github_definition("hook", &target_pipeline)}),
            )
            .await;
        if delete_pipeline {
            emulator
                .ok("DeletePipeline", &json!({"name": HELD_PIPELINE}))
                .await;
        }

        let expected = webhook_dangling_target_pipeline_message("hook", &target_pipeline);
        for operation in [REGISTER, DEREGISTER] {
            let (status, response) = emulator
                .call(operation, &json!({"webhookName": "hook"}))
                .await;
            assert_eq!(
                (
                    status,
                    response["__type"].as_str().unwrap_or_default().to_string()
                ),
                (REFUSAL_STATUS, "ValidationException".to_string()),
                "case {case:?} through {operation}: a dangling reference is refused, got \
                 {status} with {response}"
            );
            assert_eq!(
                response["message"].as_str(),
                Some(expected.as_str()),
                "case {case:?} through {operation}: expected {expected:?}, got {response}"
            );
        }
        // The refusal stored nothing and removed nothing: the webhook is still there.
        let listed = listed_webhooks(&emulator).await;
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: the webhook itself is left where it is, got {listed:?}"
        );

        // The same calls are served once the account holds a pipeline of that name, so
        // what blocked them was the reference and nothing else about the webhook.
        emulator
            .ok(
                "CreatePipeline",
                &json!({"pipeline": declaration(&target_pipeline)}),
            )
            .await;
        for operation in [REGISTER, DEREGISTER] {
            let (status, response) = emulator
                .call(operation, &json!({"webhookName": "hook"}))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?} through {operation}: a resolvable reference is served, got \
                 {status} with {response}"
            );
        }
    }

    // The wording itself, written out here rather than taken from the handler.
    assert_eq!(
        webhook_dangling_target_pipeline_message("hook", "never-created"),
        "Webhook hook references a pipeline never-created which does not exist",
        "the refusal states the reference that could not be resolved"
    );
}

/// RULE: `DeleteWebhook` stops the account holding the webhook, is a success for a name
/// it does not hold, and leaves nothing for a webhook re-created under that name to
/// inherit — "it will have a different URL".
#[tokio::test]
async fn deleting_a_webhook_removes_it_and_is_idempotent() {
    let emulator = emulator().await;
    let names = ["first", "second.hook@1", "third_hook-x"];
    let mut urls: Vec<String> = Vec::new();
    for name in names {
        let put = emulator
            .ok(
                "PutWebhook",
                &json!({
                    "webhook": github_definition(name, HELD_PIPELINE),
                    "tags": [{"key": "team", "value": "build"}],
                }),
            )
            .await;
        urls.push(
            put["webhook"]["url"]
                .as_str()
                .unwrap_or_else(|| panic!("webhook {name}: a trigger URL, got {put}"))
                .to_string(),
        );
    }

    for (index, name) in names.iter().enumerate() {
        // Deleting one webhook removes exactly that one.
        emulator.ok("DeleteWebhook", &json!({"name": name})).await;
        let remaining: Vec<String> = listed_webhooks(&emulator)
            .await
            .iter()
            .filter_map(|item| item["definition"]["name"].as_str().map(str::to_string))
            .collect();
        assert_eq!(
            remaining,
            names[index + 1..]
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<String>>(),
            "webhook {name}: deleting one webhook removes exactly that one, got \
             {remaining:?}"
        );

        // Deleting it again is the same success: the state the call asks for already
        // holds.
        let (status, response) = emulator.call("DeleteWebhook", &json!({"name": name})).await;
        assert_eq!(
            status, 200,
            "webhook {name}: deleting an already deleted webhook is a success, got \
             {status} with {response}"
        );

        // Its tags went with it: the ARN it was addressed by now names no resource.
        let arn = format!("arn:aws:codepipeline:us-east-1:000000000000:webhook:{name}");
        let (status, code) = emulator
            .expect_error("ListTagsForResource", &json!({"resourceArn": arn}))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (REFUSAL_STATUS, "ResourceNotFoundException"),
            "webhook {name}: a deleted webhook's ARN names no resource"
        );

        // A webhook re-created under the same name is a NEW webhook: it carries a
        // different trigger URL and no tag of the deleted one.
        let recreated = emulator
            .ok(
                "PutWebhook",
                &json!({"webhook": github_definition(name, HELD_PIPELINE)}),
            )
            .await;
        assert_ne!(
            recreated["webhook"]["url"].as_str(),
            Some(urls[index].as_str()),
            "webhook {name}: a re-created webhook carries a different URL, got \
             {recreated}"
        );
        assert_eq!(
            recreated["webhook"]["tags"],
            json!([]),
            "webhook {name}: a re-created webhook inherits no tag, got {recreated}"
        );
        emulator.ok("DeleteWebhook", &json!({"name": name})).await;
    }

    assert!(
        listed_webhooks(&emulator).await.is_empty(),
        "every webhook was deleted"
    );
}
