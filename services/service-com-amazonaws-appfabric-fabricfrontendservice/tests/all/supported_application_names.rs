// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the application names this service serves, driven over
//! the wire (restJson1) through the same router the emulator boots.
//!
//! RULE: an operation whose request carries an `app` member serves only the
//! applications the member's own documentation lists; a name outside that set is
//! refused with the modelled `ValidationException` (HTTP 400) carrying the
//! sentence `app <app> is not supported`, naming `app` as the member at fault,
//! before anything is written — so a refused call leaves the bundle holding
//! exactly what it held. A name inside the set is accepted and stored as the
//! request carried it.
//!
//! The tables span the whole set rather than one recorded call: every served
//! name, and names outside it of each form a wrong one takes (the one the
//! recording carried, another vendor's product, a lower-case spelling of a
//! served name, a near miss of one, a served name with a trailing space, and a
//! name at the member's own `@length` ceiling), crossed with the two operations
//! that carry the member (`CreateAppAuthorization` and `CreateIngestion`).
//!
//! The harness (the router, `Answer`, `encode`, `arn_of`) is the one
//! `app_bundle_ingestion_lifecycle` already builds.

use serde_json::{json, Value};

use crate::app_bundle_ingestion_lifecycle::{arn_of, encode, Answer, Emulator};

/// The applications the service serves, from the value set the model's own
/// documentation of the member gives
/// (`smithy_query(query="doc",shape="CreateAppAuthorizationRequest",member="app")`:
/// "The name of the application. Valid values are: SLACK ASANA JIRA M365
/// M365AUDITLOGS ZOOM ZENDESK OKTA GOOGLE DROPBOX SMARTSHEET CISCO").
const SERVED_APPS: &[&str] = &[
    "SLACK",
    "ASANA",
    "JIRA",
    "M365",
    "M365AUDITLOGS",
    "ZOOM",
    "ZENDESK",
    "OKTA",
    "GOOGLE",
    "DROPBOX",
    "SMARTSHEET",
    "CISCO",
];

/// The tenant each row below authorizes, as the row's index makes it distinct.
fn tenant_of(index: usize) -> String {
    format!("tenant-app-name-{index}")
}

/// The `credential` an `apiKey` authorization takes, which is the pairing the
/// model's documentation of `credential` requires.
fn api_key_credential() -> Value {
    json!({"apiKeyCredential": {"apiKey": "placeholder-api-key"}})
}

/// A string member of a response body, or `""`.
fn member(answer: &Answer, name: &str) -> String {
    answer
        .body
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One member of the `appAuthorization` a success body carries.
fn authorization_member<'a>(answer: &'a Answer, name: &str) -> &'a Value {
    answer
        .body
        .get("appAuthorization")
        .and_then(|authorization| authorization.get(name))
        .unwrap_or_else(|| panic!("expected appAuthorization.{name} in {}", answer.body))
}

/// The member names a `ValidationException`'s `fieldList` faults.
fn faulted_fields(answer: &Answer) -> Vec<String> {
    answer
        .body
        .get("fieldList")
        .and_then(Value::as_array)
        .map(|fields| {
            fields
                .iter()
                .filter_map(|field| field.get("name"))
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// `CreateAppAuthorization` with no `clientToken`, so every call is a request of
/// its own rather than a retry.
async fn create_authorization(emulator: &Emulator, bundle: &str, app: &str, tenant: &str) -> Answer {
    emulator
        .call(
            "POST",
            &format!("/appbundles/{}/appauthorizations", encode(bundle)),
            Some(json!({
                "app": app,
                "authType": "apiKey",
                "credential": api_key_credential(),
                "tenant": {
                    "tenantIdentifier": tenant,
                    "tenantDisplayName": format!("{tenant} display"),
                },
            })),
        )
        .await
}

/// `CreateIngestion`, with no set-up of its own: a row that needs the
/// authorization chain satisfied creates it itself.
async fn create_ingestion(emulator: &Emulator, bundle: &str, app: &str, tenant: &str) -> Answer {
    emulator
        .call(
            "POST",
            &format!("/appbundles/{}/ingestions", encode(bundle)),
            Some(json!({"app": app, "tenantId": tenant, "ingestionType": "auditLog"})),
        )
        .await
}

/// The ARNs `ListAppAuthorizations` reports for `bundle`.
async fn held_authorizations(emulator: &Emulator, bundle: &str) -> Vec<String> {
    let listed = emulator
        .call(
            "GET",
            &format!("/appbundles/{}/appauthorizations", encode(bundle)),
            None,
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "ListAppAuthorizations must answer HTTP 200, got {} with body {}",
        listed.status, listed.body
    );
    listed
        .body
        .get("appAuthorizationSummaryList")
        .and_then(Value::as_array)
        .map(|summaries| {
            summaries
                .iter()
                .filter_map(|summary| summary.get("appAuthorizationArn"))
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The ARNs `ListIngestions` reports for `bundle`.
async fn held_ingestions(emulator: &Emulator, bundle: &str) -> Vec<String> {
    let listed = emulator.list_ingestions(bundle, "").await;
    assert_eq!(
        listed.status, 200,
        "ListIngestions must answer HTTP 200, got {} with body {}",
        listed.status, listed.body
    );
    listed
        .body
        .get("ingestions")
        .and_then(Value::as_array)
        .map(|summaries| {
            summaries
                .iter()
                .filter_map(|summary| summary.get("arn"))
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Application names OUTSIDE the served set, one of each form a wrong one takes.
/// The length-bound row is built here because it is not a literal.
fn unserved_apps() -> Vec<(&'static str, String)> {
    vec![
        ("the name the recording carried", "NOT_A_REAL_SUPPORTED_APP".to_string()),
        ("another vendor's product", "PAGERDUTY".to_string()),
        ("a lower-case spelling of a served name", "slack".to_string()),
        ("a near miss of a served name", "M365AUDITLOG".to_string()),
        ("a served name with a trailing space", "SLACK ".to_string()),
        // The member is `String255` (@length min 1 max 255), so a name of 255
        // characters passes the wire schema and reaches the service's own set.
        ("a name at the member's length ceiling", "A".repeat(255)),
    ]
}

/// An app bundle to hang a chain off, as its ARN.
async fn app_bundle(emulator: &Emulator) -> String {
    let created = emulator.create_app_bundle(json!({})).await;
    assert_eq!(
        created.status, 201,
        "the set-up app bundle must be created, got HTTP {} with body {}",
        created.status, created.body
    );
    arn_of(&created.body, "appBundle")
}

/// RULE: `CreateAppAuthorization` refuses an `app` outside the served set with
/// HTTP 400 `ValidationException`, the sentence `app <app> is not supported`
/// faulting the `app` member, and writes nothing — the bundle holds no
/// authorization afterwards, and a create for a served name still succeeds in
/// the same bundle.
#[tokio::test]
async fn create_app_authorization_refuses_an_unserved_application_name() {
    for (index, (case, app)) in unserved_apps().into_iter().enumerate() {
        let emulator = Emulator::new();
        let bundle = app_bundle(&emulator).await;
        let tenant = tenant_of(index);

        let refused = create_authorization(&emulator, &bundle, &app, &tenant).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?} (app {app:?}): expected HTTP 400, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?} (app {app:?}): expected ValidationException, got {} with body {}",
            refused.error_code(),
            refused.body
        );
        assert_eq!(
            member(&refused, "message"),
            format!("app {app} is not supported"),
            "case {case:?} (app {app:?}): expected the service's sentence, got {:?}",
            member(&refused, "message")
        );
        assert_eq!(
            member(&refused, "reason"),
            "fieldValidationFailed",
            "case {case:?} (app {app:?}): a member of the request is at fault, got {}",
            refused.body
        );
        assert_eq!(
            faulted_fields(&refused),
            vec!["app".to_string()],
            "case {case:?} (app {app:?}): expected the refusal to fault app, got {}",
            refused.body
        );

        // NOTHING was written: the bundle holds no authorization at all.
        assert_eq!(
            held_authorizations(&emulator, &bundle).await,
            Vec::<String>::new(),
            "case {case:?} (app {app:?}): a refused create must write no authorization"
        );

        // And the refusal is about the NAME, not about the bundle: a served
        // name is accepted in that very bundle.
        let accepted = create_authorization(&emulator, &bundle, "SLACK", &tenant).await;
        assert_eq!(
            accepted.status, 201,
            "case {case:?} (app {app:?}): a served name must still be accepted, got {} with \
             body {}",
            accepted.status, accepted.body
        );
        let arn = authorization_member(&accepted, "appAuthorizationArn")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            held_authorizations(&emulator, &bundle).await,
            vec![arn],
            "case {case:?} (app {app:?}): expected the bundle to hold exactly the accepted \
             authorization"
        );
    }
}

/// RULE, the same member on the other operation that carries it:
/// `CreateIngestion` refuses an `app` outside the served set with the same
/// status, code, sentence and faulted member — the name is judged before the
/// (application, tenant) pair's authorization is, since an unserved name could
/// never have one — and writes no ingestion.
#[tokio::test]
async fn create_ingestion_refuses_an_unserved_application_name() {
    for (index, (case, app)) in unserved_apps().into_iter().enumerate() {
        let emulator = Emulator::new();
        let bundle = app_bundle(&emulator).await;
        let tenant = tenant_of(index);

        let refused = create_ingestion(&emulator, &bundle, &app, &tenant).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?} (app {app:?}): expected HTTP 400, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?} (app {app:?}): expected ValidationException, got {} with body {}",
            refused.error_code(),
            refused.body
        );
        assert_eq!(
            member(&refused, "message"),
            format!("app {app} is not supported"),
            "case {case:?} (app {app:?}): expected the service's sentence, got {:?}",
            member(&refused, "message")
        );
        assert_eq!(
            faulted_fields(&refused),
            vec!["app".to_string()],
            "case {case:?} (app {app:?}): expected the refusal to fault app, got {}",
            refused.body
        );
        assert_eq!(
            held_ingestions(&emulator, &bundle).await,
            Vec::<String>::new(),
            "case {case:?} (app {app:?}): a refused create must write no ingestion"
        );
    }
}

/// RULE, the accepting half: every name the served set holds runs the chain —
/// `CreateAppAuthorization` stores it and `GetAppAuthorization` reports the same
/// name back, and once the pair is connected `CreateIngestion` of that pair
/// reports it too.
///
/// The table is the WHOLE set, so no single value stands for it.
#[tokio::test]
async fn every_served_application_name_is_accepted_and_round_trips() {
    for (index, app) in SERVED_APPS.iter().enumerate() {
        // A fresh store per row: a Region holds only one app bundle.
        let emulator = Emulator::new();
        let bundle = app_bundle(&emulator).await;
        let tenant = tenant_of(index);

        let created = create_authorization(&emulator, &bundle, app, &tenant).await;
        assert_eq!(
            created.status, 201,
            "case {app:?}: expected HTTP 201 from CreateAppAuthorization, got {} with body {}",
            created.status, created.body
        );
        assert_eq!(
            authorization_member(&created, "app"),
            &json!(app),
            "case {app:?}: expected the application name the request carried, got {}",
            created.body
        );
        let arn = authorization_member(&created, "appAuthorizationArn")
            .as_str()
            .unwrap_or_default()
            .to_string();

        // ROUND TRIP: the reader answers from the same stored record.
        let read = emulator
            .call(
                "GET",
                &format!(
                    "/appbundles/{}/appauthorizations/{}",
                    encode(&bundle),
                    encode(&arn)
                ),
                None,
            )
            .await;
        assert_eq!(
            read.status, 200,
            "case {app:?}: expected the authorization to be readable, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            authorization_member(&read, "app"),
            &json!(app),
            "case {app:?}: expected the stored application name on the read, got {}",
            read.body
        );

        // The connect handshake of an `apiKey` authorization completes with the
        // call, which is what the ingestion of that pair depends on.
        let connected = emulator
            .call(
                "POST",
                &format!(
                    "/appbundles/{}/appauthorizations/{}/connect",
                    encode(&bundle),
                    encode(&arn)
                ),
                Some(json!({})),
            )
            .await;
        assert_eq!(
            connected.status, 200,
            "case {app:?}: expected HTTP 200 from ConnectAppAuthorization, got {} with body {}",
            connected.status, connected.body
        );

        let ingestion = create_ingestion(&emulator, &bundle, app, &tenant).await;
        assert_eq!(
            ingestion.status, 201,
            "case {app:?}: expected HTTP 201 from CreateIngestion, got {} with body {}",
            ingestion.status, ingestion.body
        );
        assert_eq!(
            ingestion
                .body
                .get("ingestion")
                .and_then(|record| record.get("app")),
            Some(&json!(app)),
            "case {app:?}: expected the ingestion to report the application name, got {}",
            ingestion.body
        );
        assert_eq!(
            held_ingestions(&emulator, &bundle).await.len(),
            1,
            "case {app:?}: expected the bundle to hold exactly the created ingestion"
        );
    }
}
