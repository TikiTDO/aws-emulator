// SPDX-License-Identifier: CC0-1.0

//! `UpdateDomainName` and `DeleteDomainName`, end to end over HTTP.
//!
//! The rules asserted here, each over CLASSES of input rather than over one request:
//!
//! 1. an update REPLACES the settable configuration of the custom domain the path
//!    addresses — the one member its input carries, `description` — and answers the
//!    `DomainNameConfig` it stored, which is the body `GetDomainName` and `ListDomainNames`
//!    report next; everything the input does not address (the certificate, the endpoint and
//!    hosted zone AppSync issued, the tags) is kept;
//! 2. a delete removes the domain, so every later read reports it absent and the account's
//!    other domains are untouched;
//! 3. both operations address a domain the account HOLDS, so one it does not — never
//!    created, or already deleted — is refused with the modelled `NotFoundException`
//!    (HTTP 404), and a refused call changes nothing.
//!
//! The rows span the FORM of the domain name (every shape the modelled `DomainName` pattern
//! admits, up to its maximum length), the form of the new `description` (absent, empty, a
//! multi-line and multi-byte value, and the modelled maximum length), whether the domain
//! carried a description before the update, and whether it carries tags — so neither rule
//! can be keyed to one kind of domain or one kind of value.
//!
//! The harness (`call`, `router`, `Answer`) is the source-API association suite's — reused
//! rather than copied.

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::source_api_association::{call, router, Answer};

/// `CreateDomainName`'s and `ListDomainNames`' shared path. Source: the `http` binding of
/// `CreateDomainName`, `smithy_query(query="operation", operation="CreateDomainName")`.
const DOMAIN_NAMES_PATH: &str = "/v1/domainnames";

/// The largest `domainName` the model admits: `DomainName` is `@length` 1..253. Source:
/// `smithy_query(query="operation", operation="UpdateDomainName")`.
const DOMAIN_NAME_MAX_LEN: usize = 253;

/// The largest `description` the model admits: `Description` is `@length` 0..255. Source:
/// the `description` member of `UpdateDomainNameRequest`
/// (`smithy_query(query="operation", operation="UpdateDomainName")`).
const DESCRIPTION_MAX_LEN: usize = 255;

/// A domain name of the modelled maximum length whose first label absorbs the padding,
/// which the modelled pattern admits.
fn max_length_domain(tail: &str) -> String {
    let label_len = DOMAIN_NAME_MAX_LEN - tail.len() - 1;
    format!("{}.{tail}", "a".repeat(label_len))
}

/// A description of exactly `length` characters, built from multi-byte and newline-bearing
/// text so the row exercises content as well as size. `chars`, because the model measures
/// `@length` in characters.
fn description(length: usize) -> String {
    let mut text = String::new();
    while text.chars().count() < length {
        text.push_str("the orders domain — 注文\n");
    }
    text.chars().take(length).collect()
}

/// `GET /v1/domainnames/{domainName}`.
async fn get_domain(router: &MultiServiceRouter, domain: &str) -> Answer {
    call(router, "GET", &format!("{DOMAIN_NAMES_PATH}/{domain}"), None).await
}

/// `POST /v1/domainnames/{domainName}`, carrying `body`.
async fn update_domain(router: &MultiServiceRouter, domain: &str, body: Value) -> Answer {
    call(
        router,
        "POST",
        &format!("{DOMAIN_NAMES_PATH}/{domain}"),
        Some(body),
    )
    .await
}

/// `DELETE /v1/domainnames/{domainName}`.
async fn delete_domain(router: &MultiServiceRouter, domain: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("{DOMAIN_NAMES_PATH}/{domain}"),
        None,
    )
    .await
}

/// Create one custom domain and answer the `DomainNameConfig` the create stored.
async fn create_domain(
    router: &MultiServiceRouter,
    domain: &str,
    description: Option<&str>,
    tags: Option<Value>,
) -> Value {
    let mut body = json!({
        "domainName": domain,
        "certificateArn": format!(
            "arn:aws:acm:us-east-1:000000000000:certificate/{}",
            domain.len()
        ),
    });
    if let Some(description) = description {
        body["description"] = json!(description);
    }
    if let Some(tags) = tags {
        body["tags"] = tags;
    }
    let answer = call(router, "POST", DOMAIN_NAMES_PATH, Some(body)).await;
    assert_eq!(
        answer.status, 200,
        "case seed {domain:?}: expected CreateDomainName 200, got {answer:?}"
    );
    answer.body["domainNameConfig"].clone()
}

/// One row of the update table.
struct UpdateCase {
    label: &'static str,
    /// The tail the domain name is built from, so each row addresses its own domain.
    domain: String,
    /// The description the domain is CREATED with.
    before: Option<String>,
    /// The `description` the update carries, or `None` for an update that omits the member.
    after: Option<String>,
    /// The tags the domain is created with.
    tags: Option<Value>,
}

/// The update table: the rows differ along the form of the domain name, the description the
/// domain held before, the description the update carries, and whether the domain is
/// tagged.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            label: "a two-label domain gaining a description",
            domain: "u0.example".to_string(),
            before: None,
            after: Some("the orders domain".to_string()),
            tags: None,
        },
        UpdateCase {
            label: "a three-label domain replacing its description",
            domain: "api.u1.example.com".to_string(),
            before: Some("the first description".to_string()),
            after: Some(description(64)),
            tags: Some(json!({"stage": "beta"})),
        },
        UpdateCase {
            label: "a hyphenated, digit-bearing domain losing its description",
            domain: "api-7.u2-x.example.com".to_string(),
            before: Some("the description the update drops".to_string()),
            // An update that omits the member REPLACES the stored configuration with one
            // that carries no description.
            after: None,
            tags: Some(json!({"team": "orders", "stage": "prod"})),
        },
        UpdateCase {
            label: "a wildcard domain taking the empty description",
            domain: "*.u3.example.com".to_string(),
            before: Some("not empty".to_string()),
            after: Some(String::new()),
            tags: None,
        },
        UpdateCase {
            label: "a prefixed wildcard domain taking the modelled maximum description",
            domain: "*dev.u4.example.com".to_string(),
            before: None,
            after: Some(description(DESCRIPTION_MAX_LEN)),
            tags: Some(json!({"stage": "dev"})),
        },
        UpdateCase {
            label: "a domain of the modelled maximum length",
            domain: max_length_domain("u5.example.com"),
            before: Some("the longest domain".to_string()),
            after: Some(description(1)),
            tags: None,
        },
    ]
}

/// RULE 1: an update replaces the domain's `description` with the one the request carries —
/// and with none when the request carries none — keeps everything its input does not
/// address, and answers the configuration the read reports next.
#[tokio::test]
async fn an_update_replaces_the_description_and_is_what_the_read_reports() {
    let router = router();

    for case in update_cases() {
        let UpdateCase {
            label,
            domain,
            before,
            after,
            tags,
        } = case;
        let created = create_domain(&router, &domain, before.as_deref(), tags.clone()).await;

        let mut body = json!({});
        if let Some(after) = &after {
            body["description"] = json!(after);
        }
        let updated = update_domain(&router, &domain, body).await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected UpdateDomainName 200, got {updated:?}"
        );
        let config = &updated.body["domainNameConfig"];
        let expected_description = after
            .as_ref()
            .map_or(Value::Null, |after| json!(after));
        assert_eq!(
            config["description"], expected_description,
            "case {label:?}: expected the description the update carried, got {config:?}"
        );

        // Everything the update's input does not address is kept, member for member.
        for member in [
            "domainName",
            "certificateArn",
            "appsyncDomainName",
            "hostedZoneId",
            "domainNameArn",
            "tags",
        ] {
            assert_eq!(
                config[member], created[member],
                "case {label:?}: expected {member} to survive the update as {:?}, got {:?}",
                created[member], config[member]
            );
        }

        // The answer is the STORED record: the read reports the same configuration.
        let read = get_domain(&router, &domain).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: expected GetDomainName 200 after the update, got {read:?}"
        );
        assert_eq!(
            read.body["domainNameConfig"], *config,
            "case {label:?}: expected the read to report what the update stored, got {read:?}"
        );
    }
}

/// RULE 2: a delete removes the domain, so every later read reports it absent, and the
/// account's other domains are untouched.
#[tokio::test]
async fn a_delete_removes_the_domain_and_leaves_the_others() {
    let router = router();

    let cases = update_cases();
    for case in &cases {
        create_domain(
            &router,
            &case.domain,
            case.before.as_deref(),
            case.tags.clone(),
        )
        .await;
    }

    let mut remaining: Vec<String> = cases.iter().map(|case| case.domain.clone()).collect();
    for case in &cases {
        let removed = delete_domain(&router, &case.domain).await;
        assert_eq!(
            removed.status, 200,
            "case {:?}: expected DeleteDomainName 200, got {removed:?}",
            case.label
        );

        let read = get_domain(&router, &case.domain).await;
        assert_eq!(
            (read.status, read.error_type.as_str()),
            (404, "NotFoundException"),
            "case {:?}: expected the deleted domain to read as absent, got {read:?}",
            case.label
        );

        remaining.retain(|held| held != &case.domain);
        let listed = call(&router, "GET", DOMAIN_NAMES_PATH, None).await;
        let names: Vec<String> = listed.body["domainNameConfigs"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry["domainName"].as_str())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let mut expected = remaining.clone();
        expected.sort();
        let mut reported = names.clone();
        reported.sort();
        assert_eq!(
            reported, expected,
            "case {:?}: expected the listing to hold the other domains, got {names:?}",
            case.label
        );
    }
}

/// RULE 3: both operations address a domain the account HOLDS, so one it does not — never
/// created, or already deleted — is refused with `NotFoundException` (404), and the refused
/// call changes nothing.
#[tokio::test]
async fn a_domain_the_account_does_not_hold_is_refused_as_absent() {
    let router = router();

    let held = "held.example.com";
    create_domain(&router, held, Some("the one domain"), None).await;

    // The rows differ along the FORM of the absent name — every shape the modelled pattern
    // admits — so the refusal cannot be keyed to one spelling.
    let absent: &[(&str, String)] = &[
        ("a two-label name", "absent.example".to_string()),
        ("a three-label name", "api.absent.example.com".to_string()),
        (
            "a hyphenated, digit-bearing name",
            "api-7.absent-9.example.com".to_string(),
        ),
        ("a wildcard name", "*.absent.example.com".to_string()),
        (
            "a prefixed wildcard name",
            "*dev.absent.example.com".to_string(),
        ),
        (
            "a name of the modelled maximum length",
            max_length_domain("absent.example.com"),
        ),
    ];
    for (label, domain) in absent {
        // Each operation states the condition in ITS own wording: AppSync words it bare on
        // `UpdateDomainName` (as on `GetDomainName`) and punctuated on `DeleteDomainName`
        // (as on the association calls), which `state`'s two constants are the one copy of.
        for (operation, expected, answer) in [
            (
                "UpdateDomainName",
                state::DOMAIN_NAME_NOT_FOUND_READ_MESSAGE,
                update_domain(&router, domain, json!({"description": "no such domain"})).await,
            ),
            (
                "DeleteDomainName",
                state::DOMAIN_NAME_NOT_FOUND_MESSAGE,
                delete_domain(&router, domain).await,
            ),
        ] {
            assert_eq!(
                (answer.status, answer.error_type.as_str()),
                (404, "NotFoundException"),
                "case {label:?} / {operation}: expected 404 NotFoundException, got {answer:?}"
            );
            assert_eq!(
                answer.body["message"],
                json!(expected),
                "case {label:?} / {operation}: expected AppSync's absent-domain sentence \
                 {expected:?}, got {answer:?}"
            );
        }
    }

    // A second delete of a domain this account DID hold is the same refusal: the delete is
    // not idempotent, because the resource is gone.
    let removed = delete_domain(&router, held).await;
    assert_eq!(
        removed.status, 200,
        "case \"the held domain\": expected DeleteDomainName 200, got {removed:?}"
    );
    let again = delete_domain(&router, held).await;
    assert_eq!(
        (again.status, again.error_type.as_str()),
        (404, "NotFoundException"),
        "case \"a repeated delete\": expected 404 NotFoundException, got {again:?}"
    );

    // Nothing the refused calls named was created.
    let listed = call(&router, "GET", DOMAIN_NAMES_PATH, None).await;
    assert_eq!(
        listed.body["domainNameConfigs"],
        json!([]),
        "case \"a refused call writes nothing\": expected an empty listing, got {listed:?}"
    );
}

/// RULE: an update whose `description` is outside the modelled `@length` is refused with
/// the service's `BadRequestException` (400) before anything is stored, exactly as the
/// create refuses one.
#[tokio::test]
async fn a_description_outside_the_modelled_length_is_refused() {
    let router = router();

    let domain = "bounded.example.com";
    let created = create_domain(&router, domain, Some("the first description"), None).await;

    // The published bound, and one character past it.
    for (label, length, status) in [
        ("the modelled maximum", DESCRIPTION_MAX_LEN, 200),
        ("one character past the maximum", DESCRIPTION_MAX_LEN + 1, 400),
    ] {
        let text = description(length);
        let answer = update_domain(&router, domain, json!({"description": text})).await;
        assert_eq!(
            answer.status, status,
            "case {label:?}: expected status {status}, got {answer:?}"
        );
        if status == 400 {
            assert_eq!(
                answer.error_type, "BadRequestException",
                "case {label:?}: expected the modelled BadRequestException, got {answer:?}"
            );
            // The refused update stored nothing: the description is still the accepted one.
            let read = get_domain(&router, domain).await;
            assert_eq!(
                read.body["domainNameConfig"]["description"],
                json!(description(DESCRIPTION_MAX_LEN)),
                "case {label:?}: expected the refused update to store nothing, got {read:?}"
            );
            assert_eq!(
                read.body["domainNameConfig"]["certificateArn"], created["certificateArn"],
                "case {label:?}: expected the certificate untouched, got {read:?}"
            );
        }
    }
}
