// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHEN an operation nested under an app bundle judges the
//! rules it owns over the request BODY, driven over the wire (restJson1) through
//! the same router the emulator boots.
//!
//! RULE: the app bundle an operation's URI names is the SCOPE the call is made
//! in, so that identifier is resolved FIRST — an identifier naming nothing
//! stored is refused as the app bundle (HTTP 404 `ResourceNotFoundException`)
//! whatever else the request carries — and a rule the handler owns over the
//! request's own members is judged only inside a bundle that does resolve, where
//! it is the refusal that operation models for it (HTTP 400
//! `ValidationException`).
//!
//! The table spans the whole class rather than one recorded call: every
//! implemented operation on this chain that owns such a rule (the
//! `credential`/`authType` pairing on `CreateAppAuthorization` for each of the
//! model's two `AuthType` values and on `UpdateAppAuthorization`, the
//! connected-authorization dependency `CreateIngestion` owns, and the
//! supported-application rule both creates own over `app`), crossed with the
//! application names each row class takes and with four identifier forms that
//! name no stored bundle.
//!
//! The harness (the router, `encode`, `arn_of`) is the one
//! `app_bundle_ingestion_lifecycle` already builds.

use serde_json::{json, Value};

use crate::app_bundle_ingestion_lifecycle::{
    arn_of, encode, unknown_bundle_arn, unknown_uuid, uuid_of, Answer, Emulator,
};

/// The sentence a write into a bundle the store does not hold is refused with —
/// see `app_bundle_refusal_wording`, which is where that wording is pinned.
const BUNDLE_NOT_VALID_MESSAGE: &str = "The specified app bundle ID is not valid";

/// Application names the rows below carry that the service DOES serve, from the
/// set the member's own documentation gives ("Valid values are: SLACK ASANA JIRA
/// M365 M365AUDITLOGS ZOOM ZENDESK OKTA GOOGLE DROPBOX SMARTSHEET CISCO" —
/// `smithy_query(query="doc",shape="CreateAppAuthorizationRequest",member="app")`).
const APPS: &[&str] = &["SLACK", "M365AUDITLOGS"];

/// Names OUTSIDE that set, for the rows whose fault is the application name
/// itself: another vendor's product, a lower-case spelling of a served name and
/// a near miss of one.
const UNSUPPORTED_APPS: &[&str] = &["PAGERDUTY", "slack", "M365AUDITLOG"];

/// The sentence `CreateIngestion` refuses an (application, tenant) pair whose
/// app bundle holds no app authorization for it with — see
/// `ingestion_authorization_refusal_wording`, which is where that wording is
/// pinned.
const AUTHORIZATION_NOT_FOUND_MESSAGE: &str =
    "The app authorization for the specified application and tenant was not found. \
     Create an app authorization for the application and tenant before creating an ingestion.";

/// An application the documented set above DOES hold, for the rows whose fault
/// is the credential rather than the state of the store. It is NOT one of
/// `APPS`, so the authorization the set-up stores for it is not the one an
/// ingestion row asks about.
const SUPPORTED_APP: &str = "ASANA";

/// The tenant every row below authorizes.
const TENANT: &str = "tenant-scope-order";

/// The API key the `apiKeyCredential` arm of the `Credential` union carries.
const API_KEY: &str = "secret-api-key-4b2e";

/// The OAuth2 client id and secret the `oauth2Credential` arm carries.
const CLIENT_ID: &str = "secret-client-id-11a7";
const CLIENT_SECRET: &str = "secret-client-secret-6f03";

/// The credential kind one `authType` requires, as the model documents the
/// pairing on `credential` ("Specify credentials that match the authorization
/// type for your request. For example, if the authorization type for your
/// request is OAuth2 (oauth2), then you should provide only the OAuth2
/// credentials") over the model's closed `AuthType` enum (`apiKey` | `oauth2`).
fn required_credential_kind(auth_type: &str) -> &'static str {
    match auth_type {
        "apiKey" => "apiKeyCredential",
        "oauth2" => "oauth2Credential",
        other => panic!("not a modelled AuthType: {other:?}"),
    }
}

/// A `Credential` union body of one kind.
fn credential_of(kind: &str) -> Value {
    match kind {
        "apiKeyCredential" => json!({"apiKeyCredential": {"apiKey": API_KEY}}),
        "oauth2Credential" => json!({"oauth2Credential": {
            "clientId": CLIENT_ID,
            "clientSecret": CLIENT_SECRET,
        }}),
        other => panic!("not a modelled Credential member: {other:?}"),
    }
}

/// The credential kind that is NOT the one `authType` requires — the fault the
/// credential rows below carry.
fn mismatched_credential(auth_type: &str) -> Value {
    match required_credential_kind(auth_type) {
        "apiKeyCredential" => credential_of("oauth2Credential"),
        _ => credential_of("apiKeyCredential"),
    }
}

/// A `message` or other string member of a response body, or `""`.
fn member(answer: &Answer, name: &str) -> String {
    answer
        .body
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One request that breaks exactly ONE rule its handler owns over the request's
/// own members, and nothing else: every other member is well-formed and every
/// parent but the bundle (where a row names one) resolves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Faulty {
    /// `CreateAppAuthorization` whose `credential` is not the kind the
    /// `authType` it names uses.
    CreateAuthorizationCredential(&'static str),
    /// `CreateIngestion` for an (application, tenant) pair the bundle holds no
    /// app authorization for — the state-dependent rule that handler owns.
    CreateIngestionWithoutAuthorization(&'static str),
    /// `UpdateAppAuthorization` whose `credential` is not the kind the STORED
    /// authorization's `authType` uses.
    UpdateAuthorizationCredential(&'static str),
    /// `CreateAppAuthorization` whose `app` names no application the service
    /// serves.
    CreateAuthorizationApp(&'static str),
    /// `CreateIngestion` whose `app` names no application the service serves.
    CreateIngestionApp(&'static str),
}

impl Faulty {
    fn method(self) -> &'static str {
        match self {
            Self::UpdateAuthorizationCredential(_) => "PATCH",
            _ => "POST",
        }
    }

    /// The request URI, under `bundle`, reaching `authorization` for the row
    /// that names an existing authorization.
    fn uri(self, bundle: &str, authorization: &str) -> String {
        match self {
            Self::CreateAuthorizationCredential(_) | Self::CreateAuthorizationApp(_) => {
                format!("/appbundles/{}/appauthorizations", encode(bundle))
            }
            Self::CreateIngestionWithoutAuthorization(_) | Self::CreateIngestionApp(_) => {
                format!("/appbundles/{}/ingestions", encode(bundle))
            }
            Self::UpdateAuthorizationCredential(_) => format!(
                "/appbundles/{}/appauthorizations/{}",
                encode(bundle),
                encode(authorization)
            ),
        }
    }

    fn body(self) -> Value {
        match self {
            Self::CreateAuthorizationCredential(auth_type) => json!({
                "app": SUPPORTED_APP,
                "authType": auth_type,
                "credential": mismatched_credential(auth_type),
                "tenant": {"tenantIdentifier": TENANT, "tenantDisplayName": "scope order"},
            }),
            Self::CreateIngestionWithoutAuthorization(app) | Self::CreateIngestionApp(app) => {
                json!({
                    "app": app,
                    "tenantId": TENANT,
                    "ingestionType": "auditLog",
                })
            }
            Self::CreateAuthorizationApp(app) => json!({
                "app": app,
                "authType": "apiKey",
                "credential": credential_of(required_credential_kind("apiKey")),
                "tenant": {"tenantIdentifier": TENANT, "tenantDisplayName": "scope order"},
            }),
            Self::UpdateAuthorizationCredential(stored_auth_type) => {
                json!({"credential": mismatched_credential(stored_auth_type)})
            }
        }
    }

    /// The sentence the operation refuses this body with INSIDE a bundle that
    /// resolves: the credential kind the authorization type requires for the
    /// pairing rule, and the value-free precondition sentence for the ingestion
    /// whose pair is not authorized.
    fn expected_validation_message(self) -> String {
        match self {
            Self::CreateIngestionWithoutAuthorization(_) => {
                AUTHORIZATION_NOT_FOUND_MESSAGE.to_string()
            }
            Self::CreateAuthorizationApp(app) | Self::CreateIngestionApp(app) => {
                format!("app {app} is not supported")
            }
            Self::CreateAuthorizationCredential(auth_type)
            | Self::UpdateAuthorizationCredential(auth_type) => format!(
                "credential must be a {} for authType {auth_type}",
                required_credential_kind(auth_type)
            ),
        }
    }

    /// The member the refusal names in its `fieldList`, where a member of the
    /// request is what is at fault; a refusal the STORE decides faults none.
    fn expected_field(self) -> Option<&'static str> {
        match self {
            Self::CreateIngestionWithoutAuthorization(_) => None,
            Self::CreateAuthorizationCredential(_) | Self::UpdateAuthorizationCredential(_) => {
                Some("credential")
            }
            Self::CreateAuthorizationApp(_) | Self::CreateIngestionApp(_) => Some("app"),
        }
    }
}

/// Every faulty request the table spans: the credential pairing for each of the
/// model's two `AuthType` values plus the update that judges it against the
/// stored record, an unauthorized ingestion for each served application name,
/// and each create for an application name the service does not serve.
fn faulty_requests(stored_auth_type: &'static str) -> Vec<Faulty> {
    let mut rows = Vec::new();
    for app in APPS {
        rows.push(Faulty::CreateIngestionWithoutAuthorization(app));
    }
    for app in UNSUPPORTED_APPS {
        rows.push(Faulty::CreateAuthorizationApp(app));
        rows.push(Faulty::CreateIngestionApp(app));
    }
    for auth_type in ["apiKey", "oauth2"] {
        rows.push(Faulty::CreateAuthorizationCredential(auth_type));
    }
    rows.push(Faulty::UpdateAuthorizationCredential(stored_auth_type));
    rows
}

/// An app bundle to hang the chain off, as its ARN.
async fn app_bundle(emulator: &Emulator) -> String {
    let created = emulator.create_app_bundle(json!({})).await;
    assert_eq!(
        created.status, 201,
        "the set-up app bundle must be created, got HTTP {} with body {}",
        created.status, created.body
    );
    arn_of(&created.body, "appBundle")
}

/// One stored app authorization of `auth_type` under `bundle`, as its ARN: what
/// the `UpdateAppAuthorization` row judges its credential against.
async fn stored_authorization(emulator: &Emulator, bundle: &str, auth_type: &str) -> String {
    let created = emulator
        .call(
            "POST",
            &format!("/appbundles/{}/appauthorizations", encode(bundle)),
            Some(json!({
                "app": SUPPORTED_APP,
                "authType": auth_type,
                "credential": credential_of(required_credential_kind(auth_type)),
                "tenant": {"tenantIdentifier": TENANT, "tenantDisplayName": "scope order"},
            })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "the set-up app authorization must be created, got HTTP {} with body {}",
        created.status, created.body
    );
    let authorization = created
        .body
        .get("appAuthorization")
        .unwrap_or_else(|| panic!("expected appAuthorization in {}", created.body))
        .get("appAuthorizationArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !authorization.is_empty(),
        "expected an appAuthorizationArn in {}",
        created.body
    );
    authorization
}

/// How many app authorizations `bundle` holds.
async fn held_authorizations(emulator: &Emulator, bundle: &str) -> usize {
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
        .get("appAuthorizations")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

/// How many ingestions `bundle` holds.
async fn held_ingestions(emulator: &Emulator, bundle: &str) -> usize {
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
        .map_or(0, Vec::len)
}

/// RULE: a request whose URI names an app bundle the store does not hold is
/// refused as THAT app bundle — HTTP 404 `ResourceNotFoundException`, the
/// mutation-path sentence, `resourceType` `AppBundle` and the supplied
/// identifier in `resourceId` — even when the body also breaks a rule the
/// handler owns, and no refusal mentions the body's fault.
#[tokio::test]
async fn an_unknown_app_bundle_is_refused_before_any_handler_owned_body_rule() {
    let emulator = Emulator::new();

    // A bundle that really existed and is now gone, which is the other way an
    // identifier comes to name nothing.
    let deleted = app_bundle(&emulator).await;
    let removed = emulator.delete_app_bundle(&deleted).await;
    assert!(
        removed.status < 300,
        "expected the set-up bundle delete to succeed, got {} with body {}",
        removed.status,
        removed.body
    );

    let identifiers = [
        ("a uuid no create minted", unknown_uuid()),
        ("an arn no create minted", unknown_bundle_arn(&unknown_uuid())),
        ("a deleted bundle by arn", deleted.clone()),
        ("a deleted bundle by uuid", uuid_of(&deleted)),
    ];
    // Every nested identifier a row needs is a uuid no create ever minted: the
    // bundle is the outer parent of each of these paths, so it is the level that
    // refuses.
    let child = unknown_uuid();

    for (identifier_case, identifier) in &identifiers {
        for faulty in faulty_requests("apiKey") {
            let case = format!("{faulty:?} under {identifier_case}");
            let answer = emulator
                .call(
                    faulty.method(),
                    &faulty.uri(identifier, &child),
                    Some(faulty.body()),
                )
                .await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected HTTP 404 for a bundle the store does not hold, got {} \
                 with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {} with body {}",
                answer.error_code(),
                answer.body
            );
            assert_eq!(
                member(&answer, "message"),
                BUNDLE_NOT_VALID_MESSAGE,
                "case {case:?}: expected the app-bundle mutation wording, got {:?}",
                member(&answer, "message")
            );
            assert_eq!(
                member(&answer, "resourceType"),
                "AppBundle",
                "case {case:?}: expected resourceType AppBundle, got {:?}",
                member(&answer, "resourceType")
            );
            assert_eq!(
                member(&answer, "resourceId"),
                identifier.as_str(),
                "case {case:?}: expected the supplied identifier in resourceId, got {:?}",
                member(&answer, "resourceId")
            );
            // The body's fault is never what such a call is refused for.
            let body_fault = faulty.expected_validation_message();
            assert!(
                !member(&answer, "message").contains(&body_fault),
                "case {case:?}: the app-bundle refusal must not report the body fault \
                 {body_fault:?}, got {:?}",
                member(&answer, "message")
            );
        }
    }
}

/// RULE, the other half: inside a bundle that DOES resolve, each of those same
/// bodies is refused by the rule its handler owns — HTTP 400
/// `ValidationException`, naming the member at fault and the value that was
/// wrong — and the refused call writes nothing.
#[tokio::test]
async fn inside_a_resolving_app_bundle_the_body_rule_is_what_refuses() {
    for stored_auth_type in ["apiKey", "oauth2"] {
        let emulator = Emulator::new();
        let bundle = app_bundle(&emulator).await;
        let authorization = stored_authorization(&emulator, &bundle, stored_auth_type).await;
        let authorizations_before = held_authorizations(&emulator, &bundle).await;
        let ingestions_before = held_ingestions(&emulator, &bundle).await;

        for faulty in faulty_requests(stored_auth_type) {
            let case = format!("{faulty:?} under a stored bundle, stored authType \
                                {stored_auth_type}");
            let answer = emulator
                .call(
                    faulty.method(),
                    &faulty.uri(&bundle, &authorization),
                    Some(faulty.body()),
                )
                .await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400 from the rule the handler owns, got {} with \
                 body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {} with body {}",
                answer.error_code(),
                answer.body
            );
            assert_eq!(
                member(&answer, "message"),
                faulty.expected_validation_message(),
                "case {case:?}: expected the modelled sentence {:?}, got {:?}",
                faulty.expected_validation_message(),
                member(&answer, "message")
            );
            let field = answer
                .body
                .get("fieldList")
                .and_then(Value::as_array)
                .and_then(|list| list.first())
                .and_then(|entry| entry.get("name"))
                .and_then(Value::as_str)
                .map(str::to_string);
            assert_eq!(
                field.as_deref(),
                faulty.expected_field(),
                "case {case:?}: expected the refusal to name {:?}, got {field:?} in body {}",
                faulty.expected_field(),
                answer.body
            );
        }

        // Nothing any of the refusals above touched was written: the bundle
        // still holds exactly what the set-up created.
        assert_eq!(
            held_authorizations(&emulator, &bundle).await,
            authorizations_before,
            "stored authType {stored_auth_type}: a refused call must write no app authorization"
        );
        assert_eq!(
            held_ingestions(&emulator, &bundle).await,
            ingestions_before,
            "stored authType {stored_auth_type}: a refused call must write no ingestion"
        );
        // The stored authorization still reports the credential kind it was
        // created with, so no refused update half-applied.
        let read = emulator
            .call(
                "GET",
                &format!(
                    "/appbundles/{}/appauthorizations/{}",
                    encode(&bundle),
                    encode(&authorization)
                ),
                None,
            )
            .await;
        assert_eq!(
            read.status, 200,
            "stored authType {stored_auth_type}: the stored authorization must survive, got {} \
             with body {}",
            read.status, read.body
        );
        let reported_auth_type = read
            .body
            .get("appAuthorization")
            .and_then(|authorization| authorization.get("authType"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            reported_auth_type, stored_auth_type,
            "stored authType {stored_auth_type}: expected the stored authType, got \
             {reported_auth_type:?}"
        );
    }
}
