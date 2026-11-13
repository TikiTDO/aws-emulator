// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what an exported API definition says about CONTENT: the media
//! types each operation negotiates, the API's TLS security policy, and the byte spelling
//! of the YAML serialization.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the `CreateRestApi`/`PutMethod`/
//! `PutIntegration`/`CreateDeployment` helpers and the raw `GetExport` call are this
//! crate's existing harness, reused from `rest_api_lifecycle`, `execute_api_data_plane`
//! and `stage_export`.
//!
//! The tests assert RULES over sets of inputs rather than one recorded export:
//!
//! * an exported operation states the media types its own records name — the content types
//!   keying its request models and its integration's request templates as `consumes`, the
//!   content types keying its method responses' models and its integration responses'
//!   templates as `produces` — so the rows differ along which of those four maps carries a
//!   content type, how many it carries, and whether two of them carry the same one;
//! * the exported document states the API's TLS security policy, whatever the record
//!   carries, so the rows span the create that sets none and each value the member
//!   declares;
//! * the YAML serialization is the same document as the JSON one, written as a bare
//!   mapping — no document start marker — with every string scalar double-quoted, so the
//!   rows span the kinds of scalar a definition carries (a version, a path, a status-code
//!   key, a boolean, a media type).

use serde_json::{json, Value};

use crate::execute_api_data_plane::{
    integration_response, method_response, put_integration, put_method,
};
use crate::rest_api_lifecycle::{create_api, resource_under, string_member, Gateway};
use crate::stage_export::export;
use crate::stage_request_validation::create_model;

/// The stage every case exports.
const STAGE: &str = "dev";

/// The resource path every case puts its method on.
const PATH: &str = "/pets";

/// The path part of the child resource every case also creates: a path template
/// parameter, so the exported document carries a `paths` key with flow indicators in it.
const PARAMETER_SEGMENT: &str = "{petId}";

/// The verb every content-type case puts on [`PATH`].
const VERB: &str = "GET";

/// The two dialects `exportType` accepts, from `GetExportRequest$exportType`: "'oas30'
/// for OpenAPI 3.0.x and 'swagger' for Swagger/OpenAPI 2.0".
const DIALECTS: &[&str] = &["swagger", "oas30"];

/// One row of the content-type table: the case, the `PutMethod` body, the
/// `PutIntegration` body, the `PutMethodResponse` body, the `PutIntegrationResponse`
/// body, and the `consumes` and `produces` lists the export must state (empty for a row
/// whose records name no content type, which states neither member).
type ContentTypeCase<'a> = (
    &'a str,
    Value,
    Value,
    Value,
    Value,
    &'a [&'a str],
    &'a [&'a str],
);

/// The model every row that keys a request or response model names. Created on the API so
/// the method's maps name a record of the API rather than a dangling model.
const MODEL: &str = "Pet";

/// Stand an API up with one `GET` method on [`PATH`] configured by a row's four bodies,
/// deployed to [`STAGE`]. Answers the API id.
async fn api_with_method(
    gateway: &Gateway,
    case: &str,
    method: Value,
    integration: Value,
    method_response_body: Value,
    integration_response_body: Value,
) -> String {
    let (api, root) = create_api(gateway, case).await;
    create_model(
        gateway,
        &api,
        MODEL,
        json!({
            "$schema": "http://json-schema.org/draft-04/schema#",
            "title": MODEL,
            "type": "object"
        }),
    )
    .await;
    let resource = resource_under(gateway, &api, &root, PATH.trim_start_matches('/')).await;
    put_method(gateway, &api, &resource, VERB, method).await;
    put_integration(gateway, &api, &resource, VERB, integration).await;
    method_response(gateway, &api, &resource, VERB, "200", method_response_body).await;
    integration_response(
        gateway,
        &api,
        &resource,
        VERB,
        "200",
        integration_response_body,
    )
    .await;
    // A second path whose template declares a parameter, on every API: the exported
    // document then carries a `paths` key carrying YAML flow indicators (`{`, `}`) inside
    // it, which the YAML case reads back through the round trip. Its method names no
    // content type of its own, so it is not what any content-type row asserts about.
    let child = resource_under(gateway, &api, &resource, PARAMETER_SEGMENT).await;
    put_method(
        gateway,
        &api,
        &child,
        VERB,
        json!({ "authorizationType": "NONE" }),
    )
    .await;
    put_integration(gateway, &api, &child, VERB, json!({ "type": "MOCK" })).await;
    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/deployments"),
            Some(json!({ "stageName": STAGE })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected 201 from CreateDeployment, got {} with {:?}",
        answer.status, answer.body
    );
    api
}

/// The exported operation object of [`PATH`]'s [`VERB`] in `dialect`.
async fn exported_operation(gateway: &Gateway, api: &str, dialect: &str, case: &str) -> Value {
    let answer = export(gateway, api, STAGE, dialect, None, &[]).await;
    let document = answer.json(case);
    document["paths"][PATH][&VERB.to_ascii_lowercase()].clone()
}

#[tokio::test]
async fn an_exported_operation_states_the_media_types_its_records_name() {
    // Rows differ along which stored map carries a content type (a request model, an
    // integration's request template, a method response's model, an integration
    // response's template), how many content types it carries, and whether two maps carry
    // the same one — which is one content type of the operation, not two.
    let cases: Vec<ContentTypeCase<'_>> = vec![
        (
            "records naming no content type",
            json!({ "authorizationType": "NONE" }),
            json!({ "type": "MOCK" }),
            json!({}),
            json!({}),
            &[],
            &[],
        ),
        (
            "an integration request template",
            json!({ "authorizationType": "NONE" }),
            json!({
                "type": "MOCK",
                "requestTemplates": { "application/json": "{\"statusCode\": 200}" }
            }),
            json!({}),
            json!({}),
            &["application/json"],
            &[],
        ),
        (
            "a request model",
            json!({
                "authorizationType": "NONE",
                "requestModels": { "application/json": MODEL }
            }),
            json!({ "type": "MOCK" }),
            json!({}),
            json!({}),
            &["application/json"],
            &[],
        ),
        (
            "an integration response template",
            json!({ "authorizationType": "NONE" }),
            json!({ "type": "MOCK" }),
            json!({}),
            json!({ "responseTemplates": { "text/html": "<html></html>" } }),
            &[],
            &["text/html"],
        ),
        (
            "a method response model",
            json!({ "authorizationType": "NONE" }),
            json!({ "type": "MOCK" }),
            json!({ "responseModels": { "application/json": MODEL } }),
            json!({}),
            &[],
            &["application/json"],
        ),
        (
            "several content types on both sides",
            json!({
                "authorizationType": "NONE",
                "requestModels": { "text/plain": MODEL }
            }),
            json!({
                "type": "MOCK",
                "requestTemplates": {
                    "application/json": "{\"statusCode\": 200}",
                    "application/xml": "<request/>"
                }
            }),
            json!({ "responseModels": { "application/json": MODEL } }),
            json!({ "responseTemplates": { "text/html": "<html></html>" } }),
            &["application/json", "application/xml", "text/plain"],
            &["application/json", "text/html"],
        ),
        (
            "one content type keyed by both a model and a template",
            json!({
                "authorizationType": "NONE",
                "requestModels": { "application/json": MODEL }
            }),
            json!({
                "type": "MOCK",
                "requestTemplates": { "application/json": "{\"statusCode\": 200}" }
            }),
            json!({ "responseModels": { "application/json": MODEL } }),
            json!({ "responseTemplates": { "application/json": "{}" } }),
            &["application/json"],
            &["application/json"],
        ),
    ];

    for (case, method, integration, response, integration_body, consumes, produces) in cases {
        let gateway = Gateway::new();
        let api = api_with_method(
            &gateway,
            case,
            method,
            integration,
            response,
            integration_body,
        )
        .await;

        // A 2.0 document states the content types on the operation itself.
        let operation = exported_operation(&gateway, &api, "swagger", case).await;
        for (member, expected) in [("consumes", consumes), ("produces", produces)] {
            let reported: Vec<String> = operation[member]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .map(|item| item.as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                reported, expected,
                "case {case}: expected {member} {expected:?}, got {:?}",
                operation[member]
            );
            // A method whose records name no content type states nothing, rather than an
            // empty list.
            if expected.is_empty() {
                assert!(
                    operation[member].is_null(),
                    "case {case}: expected no {member} member at all, got {:?}",
                    operation[member]
                );
            }
        }

        // `consumes` and `produces` are 2.0 members: a 3.0 document has neither, because a
        // content type is a key of a Media Type Object there.
        let oas30 = exported_operation(&gateway, &api, "oas30", case).await;
        for member in ["consumes", "produces"] {
            assert!(
                oas30[member].is_null(),
                "case {case}: a 3.0 document declares no {member}, got {:?}",
                oas30[member]
            );
        }
    }
}

#[tokio::test]
async fn an_export_states_the_tls_security_policy_its_api_carries() {
    // Rows span the create that sets no policy (the service picks one) and each value
    // `RestApi$securityPolicy` declares, read back from the record itself rather than
    // compared against a literal: the exported document states what the API carries.
    let cases: [(&str, Option<&str>); 3] = [
        ("a create that sets no security policy", None),
        ("a create that sets TLS_1_0", Some("TLS_1_0")),
        ("a create that sets TLS_1_2", Some("TLS_1_2")),
    ];
    /// The document key API Gateway exports the API's TLS policy under.
    const EXTENSION: &str = "x-amazon-apigateway-security-policy";

    for (case, policy) in cases {
        let gateway = Gateway::new();
        let mut request = json!({ "name": case });
        if let Some(policy) = policy {
            request["securityPolicy"] = Value::from(policy);
        }
        let created = gateway.call("POST", "/restapis", Some(request)).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from CreateRestApi, got {} with {:?}",
            created.status, created.body
        );
        let api = string_member(&created.body, "id", case);
        let root = string_member(&created.body, "rootResourceId", case);
        put_method(
            &gateway,
            &api,
            &root,
            VERB,
            json!({ "authorizationType": "NONE" }),
        )
        .await;
        put_integration(&gateway, &api, &root, VERB, json!({ "type": "MOCK" })).await;
        let deployed = gateway
            .call(
                "POST",
                &format!("/restapis/{api}/deployments"),
                Some(json!({ "stageName": STAGE })),
            )
            .await;
        assert_eq!(
            deployed.status, 201,
            "case {case}: expected 201 from CreateDeployment, got {} with {:?}",
            deployed.status, deployed.body
        );

        // What the record carries, through the reader a client reads it with.
        let read = gateway
            .call("GET", &format!("/restapis/{api}"), None)
            .await;
        let stored = string_member(&read.body, "securityPolicy", case);

        for dialect in DIALECTS {
            let case = format!("{case} exported as {dialect}");
            let document = export(&gateway, &api, STAGE, dialect, None, &[])
                .await
                .json(&case);
            assert_eq!(
                document[EXTENSION].as_str(),
                Some(stored.as_str()),
                "case {case}: expected the API's own policy {stored:?}, got {:?}",
                document[EXTENSION]
            );
        }
    }
}

/// One row of the YAML-spelling table: the case and the line the document must carry.
type YamlLineCase<'a> = (&'a str, &'a str);

/// `document` as the JSON value it spells, so a YAML export and a JSON export of one stage
/// can be compared as the same document.
///
/// `pub(crate)` for the sibling `stage_export_oas30_containers` suite, which compares the
/// serializations of a 3.0 export the same way: this crate reads a YAML export back
/// through ONE conversion.
pub(crate) fn yaml_as_json(document: &::yaml_rust2::yaml::Yaml) -> Value {
    use ::yaml_rust2::yaml::Yaml;
    match document {
        Yaml::Null | Yaml::BadValue => Value::Null,
        Yaml::Boolean(flag) => Value::from(*flag),
        Yaml::Integer(number) => Value::from(*number),
        Yaml::Real(text) => text
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map_or(Value::Null, Value::Number),
        Yaml::String(text) => Value::from(text.as_str()),
        Yaml::Array(items) => Value::Array(items.iter().map(yaml_as_json).collect()),
        Yaml::Hash(entries) => Value::Object(
            entries
                .iter()
                .map(|(key, value)| {
                    (
                        key.as_str().unwrap_or_default().to_string(),
                        yaml_as_json(value),
                    )
                })
                .collect(),
        ),
        Yaml::Alias(_) => Value::Null,
    }
}

#[tokio::test]
async fn the_yaml_export_is_a_bare_quoted_document_spelling_the_json_one() {
    let gateway = Gateway::new();
    let case = "yaml spelling";
    let api = api_with_method(
        &gateway,
        case,
        json!({
            "authorizationType": "NONE",
            "requestModels": { "application/json": MODEL },
            "requestParameters": { "method.request.querystring.page": false }
        }),
        json!({
            "type": "MOCK",
            "requestTemplates": { "application/json": "{\"statusCode\": 200}" }
        }),
        json!({
            "responseModels": { "application/json": MODEL },
            "responseParameters": { "method.response.header.012": false }
        }),
        json!({ "responseTemplates": { "text/html": "<html></html>" } }),
    )
    .await;

    let answer = export(&gateway, &api, STAGE, "swagger", Some("application/yaml"), &[]).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let text = String::from_utf8(answer.body.clone())
        .unwrap_or_else(|error| panic!("case {case}: expected a UTF-8 export, got {error}"));

    // No document start marker: the export is a bare mapping.
    assert!(
        !text.lines().any(|line| line.trim_end() == "---"),
        "case {case}: expected no document start marker, got {text}"
    );

    // Rows span the kinds of scalar and key a definition carries: a version that would
    // otherwise read as a number, a path, a media type in a sequence, a status-code key, a
    // response header named like an octal number, a path key declaring a parameter, the
    // security-policy extension's value, and a boolean — which stays a boolean.
    let lines: &[YamlLineCase<'_>] = &[
        ("the document version", "swagger: \"2.0\""),
        ("the stage base path", "basePath: \"/dev\""),
        ("a sequence item", "- \"application/json\""),
        ("a status code key", "\"200\":"),
        ("a numeric-looking header key", "\"012\":"),
        (
            "a path key declaring a parameter",
            "/pets/{petId}:",
        ),
        (
            "the security policy extension",
            "x-amazon-apigateway-security-policy: \"TLS_1_0\"",
        ),
        ("a boolean", "required: false"),
    ];
    for (line_case, expected) in lines {
        assert!(
            text.lines().any(|line| line.trim_start() == *expected),
            "case {case}/{line_case}: expected the line {expected:?}, got {text}"
        );
    }

    // A sequence's items sit at their KEY's indentation.
    assert!(
        text.contains("schemes:\n- \"https\"\n"),
        "case {case}: expected a sequence at its key's indentation, got {text}"
    );

    // Both serializations spell the SAME document.
    let documents = ::yaml_rust2::yaml::YamlLoader::load_from_str(&text)
        .unwrap_or_else(|error| panic!("case {case}: expected a YAML export, got {error} for {text}"));
    let parsed = documents
        .first()
        .unwrap_or_else(|| panic!("case {case}: expected one YAML document, got {text}"));
    let json = export(&gateway, &api, STAGE, "swagger", Some("application/json"), &[])
        .await
        .json(case);
    assert_eq!(
        yaml_as_json(parsed),
        json,
        "case {case}: the YAML and the JSON export must be one document, got {text}"
    );
}
