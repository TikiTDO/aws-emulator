// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for AppFabric's app-authorization chain and the ingestion
//! operations that depend on it, driven over the wire (restJson1) through the
//! same router the emulator boots.
//!
//! Each test states a RULE over a TABLE of inputs rather than one recorded call:
//!
//! * an app authorization is PERSISTED under its bundle, starts `PendingConnect`
//!   and is reported identically by `GetAppAuthorization` and
//!   `ListAppAuthorizations` for either identifier form;
//! * an `app` names an application the service serves: such a name is stored as
//!   the caller gave it, by every operation that names one;
//! * a bundle holds AT MOST ONE authorization per (app, tenant) pair, however
//!   many callers race for it;
//! * an ingestion exists only for a pair whose authorization has completed the
//!   connect handshake;
//! * `StopIngestion` moves `enabled` → `disabled` and refuses when there is no
//!   such move to make;
//! * `ListIngestionDestinations` reports exactly what was created, paging
//!   through every record once;
//! * a violation of a numeric `@range` bound names the member and the bound and
//!   withholds the submitted number;
//! * a not-found refusal states a CONSTANT message that never echoes the
//!   caller's identifier.
//!
//! The harness (the router, `Answer`, `encode`, `arn_of`, the app-bundle and
//! ingestion calls) is the one `app_bundle_ingestion_lifecycle` already builds.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_appfabric_fabricfrontendservice::state::{
    service_state, INGESTION_STATE_DISABLED, INGESTION_STATE_ENABLED, MAX_PAGE_SIZE,
};

use crate::app_bundle_ingestion_lifecycle::{
    arn_of, encode, unknown_bundle_arn, unknown_ingestion_arn, unknown_uuid, uuid_of, Answer,
    Emulator,
};

/// Applications the model's own documentation of the `app` member lists
/// (`smithy_query(query="doc", shape="CreateAppAuthorizationRequest",
/// member="app")`: "Valid values are: SLACK ASANA JIRA M365 M365AUDITLOGS ZOOM
/// ZENDESK OKTA GOOGLE DROPBOX SMARTSHEET CISCO"). The tables below walk entries
/// the issue did not cite rather than repeating one value.
const DOCUMENTED_APPS: &[&str] = &[
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

/// The `message` member of an error (or success) body, or `""`.
fn message(answer: &Answer) -> &str {
    answer
        .body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// One member of the `appAuthorization` a success body carries.
fn authorization_member<'a>(answer: &'a Answer, name: &str) -> &'a Value {
    answer
        .body
        .get("appAuthorization")
        .and_then(|authorization| authorization.get(name))
        .unwrap_or_else(|| panic!("expected appAuthorization.{name} in {}", answer.body))
}

/// The credential body one `authType` takes — the pairing the model's own
/// documentation of `credential` requires ("Specify credentials that match the
/// authorization type for your request").
fn credential_for(auth_type: &str) -> Value {
    if auth_type == "apiKey" {
        json!({"apiKeyCredential": {"apiKey": "placeholder-api-key"}})
    } else {
        json!({"oauth2Credential": {
            "clientId": "placeholder-client-id",
            "clientSecret": "placeholder-client-secret",
        }})
    }
}

/// What a connect of an authorization of this `authType` leaves it reading. The
/// handshake of an `apiKey` authorization completes with the call; an `oauth2`
/// one has its outcome owed to the clock, so the record still awaits it for the
/// span the handshake takes.
fn status_after_connect(auth_type: &str) -> &'static str {
    if auth_type == "apiKey" {
        "Connected"
    } else {
        "PendingConnect"
    }
}

/// `CreateAppAuthorization`, with no connect call after it.
async fn create_authorization(
    emulator: &Emulator,
    bundle: &str,
    app: &str,
    auth_type: &str,
    tenant: &str,
) -> Answer {
    emulator
        .call(
            "POST",
            &format!("/appbundles/{}/appauthorizations", encode(bundle)),
            Some(json!({
                "app": app,
                "authType": auth_type,
                "credential": credential_for(auth_type),
                "tenant": {"tenantIdentifier": tenant, "tenantDisplayName": format!("{tenant} display")},
            })),
        )
        .await
}

/// `ConnectAppAuthorization`, carrying the OAuth2 `authRequest` an `oauth2`
/// authorization requires and nothing for an `apiKey` one.
async fn connect_authorization(
    emulator: &Emulator,
    bundle: &str,
    authorization: &str,
    auth_type: &str,
) -> Answer {
    let body = if auth_type == "apiKey" {
        json!({})
    } else {
        json!({"authRequest": {
            "redirectUri": "https://example.com/oauth/callback",
            "code": "placeholder-authorization-code",
        }})
    };
    emulator
        .call(
            "POST",
            &format!(
                "/appbundles/{}/appauthorizations/{}/connect",
                encode(bundle),
                encode(authorization)
            ),
            Some(body),
        )
        .await
}

async fn get_authorization(emulator: &Emulator, bundle: &str, authorization: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!(
                "/appbundles/{}/appauthorizations/{}",
                encode(bundle),
                encode(authorization)
            ),
            None,
        )
        .await
}

async fn list_authorizations(emulator: &Emulator, bundle: &str, query: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("/appbundles/{}/appauthorizations{query}", encode(bundle)),
            None,
        )
        .await
}

/// Every `appAuthorizationArn` one page of `ListAppAuthorizations` reports.
fn listed_authorization_arns(answer: &Answer) -> Vec<String> {
    answer
        .body
        .get("appAuthorizationSummaryList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected appAuthorizationSummaryList in {}", answer.body))
        .iter()
        .map(|summary| {
            summary
                .get("appAuthorizationArn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("expected appAuthorizationArn in {summary}"))
                .to_string()
        })
        .collect()
}

/// `CreateIngestion` WITHOUT the harness's authorize-first helper, which is what
/// the dependency rule is about.
async fn create_ingestion_unauthorized(
    emulator: &Emulator,
    bundle: &str,
    app: &str,
    tenant: &str,
) -> Answer {
    emulator
        .call(
            "POST",
            &format!("/appbundles/{}/ingestions", encode(bundle)),
            Some(json!({"app": app, "tenantId": tenant, "ingestionType": "auditLog"})),
        )
        .await
}

pub(crate) async fn stop_ingestion(emulator: &Emulator, bundle: &str, ingestion: &str) -> Answer {
    emulator
        .call(
            "POST",
            &format!(
                "/appbundles/{}/ingestions/{}/stop",
                encode(bundle),
                encode(ingestion)
            ),
            None,
        )
        .await
}

pub(crate) async fn create_destination(
    emulator: &Emulator,
    bundle: &str,
    ingestion: &str,
    bucket: &str,
) -> Answer {
    emulator
        .call(
            "POST",
            &format!(
                "/appbundles/{}/ingestions/{}/ingestiondestinations",
                encode(bundle),
                encode(ingestion)
            ),
            Some(json!({
                "processingConfiguration": {"auditLog": {"schema": "raw", "format": "json"}},
                "destinationConfiguration": {"auditLog": {"destination": {"s3Bucket": {"bucketName": bucket}}}},
            })),
        )
        .await
}

async fn list_destinations(
    emulator: &Emulator,
    bundle: &str,
    ingestion: &str,
    query: &str,
) -> Answer {
    emulator
        .call(
            "GET",
            &format!(
                "/appbundles/{}/ingestions/{}/ingestiondestinations{query}",
                encode(bundle),
                encode(ingestion)
            ),
            None,
        )
        .await
}

/// A bundle holding one CONNECTED authorization for (`app`, `tenant`) and one
/// ingestion of that pair — the chain AppFabric requires (authorize → connect →
/// ingest), each step driven through its own operation.
pub(crate) async fn bundle_with_ingestion(
    emulator: &Emulator,
    app: &str,
    tenant: &str,
    case: &str,
) -> (String, String) {
    let bundle = arn_of(
        &emulator.create_app_bundle(json!({})).await.body,
        "appBundle",
    );
    let created = create_authorization(emulator, &bundle, app, "apiKey", tenant).await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: expected the authorization to be created, got {} with body {}",
        created.status, created.body
    );
    let authorization = authorization_member(&created, "appAuthorizationArn")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let connected = connect_authorization(emulator, &bundle, &authorization, "apiKey").await;
    assert_eq!(
        connected.status, 200,
        "case {case:?}: expected the authorization to connect, got {} with body {}",
        connected.status, connected.body
    );
    let ingestion = create_ingestion_unauthorized(emulator, &bundle, app, tenant).await;
    assert_eq!(
        ingestion.status, 201,
        "case {case:?}: expected the ingestion to be created, got {} with body {}",
        ingestion.status, ingestion.body
    );
    (bundle, arn_of(&ingestion.body, "ingestion"))
}

/// RULE: a created app authorization is PERSISTED under its bundle, starts in
/// the state the model documents as the initial one (`PendingConnect`), carries
/// the `admin` persona, and is reported by `GetAppAuthorization` — for either
/// `Identifier` form — and by `ListAppAuthorizations` exactly as the create call
/// returned it; `ConnectAppAuthorization` is the only thing that moves it out of
/// `PendingConnect`, to the outcome its own handshake reaches.
///
/// The table spans both `AuthType` values with their matching credential kinds
/// and applications the issue did not cite.
#[tokio::test]
async fn an_app_authorization_round_trips_and_starts_pending_connect() {
    let emulator = Emulator::new();
    let bundle = arn_of(
        &emulator.create_app_bundle(json!({})).await.body,
        "appBundle",
    );

    // (case, app, authType, tenant)
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        ("oauth2 for ZENDESK", "ZENDESK", "oauth2", "tenant-zendesk"),
        ("apiKey for CISCO", "CISCO", "apiKey", "tenant-cisco"),
        (
            "apiKey for M365AUDITLOGS",
            "M365AUDITLOGS",
            "apiKey",
            "tenant-m365",
        ),
        ("oauth2 for DROPBOX", "DROPBOX", "oauth2", "tenant-dropbox"),
        (
            "oauth2 for SMARTSHEET, same tenant id as another app",
            "SMARTSHEET",
            "oauth2",
            "tenant-cisco",
        ),
    ];

    let mut created_arns: Vec<String> = Vec::new();
    for (case, app, auth_type, tenant) in cases {
        let created = create_authorization(&emulator, &bundle, app, auth_type, tenant).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );
        assert_eq!(
            authorization_member(&created, "status"),
            "PendingConnect",
            "case {case:?}: expected a new authorization to be PendingConnect, got {}",
            created.body
        );
        assert_eq!(
            authorization_member(&created, "persona"),
            "admin",
            "case {case:?}: expected the admin persona, got {}",
            created.body
        );
        assert_eq!(
            authorization_member(&created, "app"),
            app,
            "case {case:?}: expected the app to be stored, got {}",
            created.body
        );
        assert_eq!(
            authorization_member(&created, "authType"),
            auth_type,
            "case {case:?}: expected the authType to be stored, got {}",
            created.body
        );
        assert_eq!(
            authorization_member(&created, "appBundleArn"),
            &json!(bundle),
            "case {case:?}: expected the parent bundle ARN, got {}",
            created.body
        );
        assert_eq!(
            authorization_member(&created, "tenant"),
            &json!({"tenantIdentifier": tenant, "tenantDisplayName": format!("{tenant} display")}),
            "case {case:?}: expected the tenant to be stored, got {}",
            created.body
        );
        // No credential material is ever reported back: both credential shapes
        // are @sensitive and the response shape has no member for them.
        let rendered = created.body.to_string();
        assert!(
            !rendered.contains("placeholder-api-key")
                && !rendered.contains("placeholder-client-secret"),
            "case {case:?}: the response must not carry credential material, got {rendered}"
        );

        let arn = authorization_member(&created, "appAuthorizationArn")
            .as_str()
            .unwrap_or_default()
            .to_string();
        for identifier in [arn.clone(), uuid_of(&arn)] {
            let read = get_authorization(&emulator, &bundle, &identifier).await;
            assert_eq!(
                read.status, 200,
                "case {case:?} read by {identifier:?}: expected HTTP 200, got {} with body {}",
                read.status, read.body
            );
            assert_eq!(
                read.body.get("appAuthorization"),
                created.body.get("appAuthorization"),
                "case {case:?} read by {identifier:?}: expected the stored record {}, got {}",
                created.body,
                read.body
            );
        }

        let connected = connect_authorization(&emulator, &bundle, &arn, auth_type).await;
        assert_eq!(
            connected.status, 200,
            "case {case:?}: expected the connect call to succeed, got {} with body {}",
            connected.status, connected.body
        );
        // The handshake of an `apiKey` authorization completes with the call; an
        // `oauth2` one has nothing this emulator can redeem with the
        // application, so its outcome is owed to the clock and the record still
        // awaits it here (`app_authorization_connect_validation` asserts the
        // outcome itself).
        let after_connect = status_after_connect(auth_type);
        assert_eq!(
            connected
                .body
                .get("appAuthorizationSummary")
                .and_then(|summary| summary.get("status")),
            Some(&json!(after_connect)),
            "case {case:?}: expected the summary to report {after_connect}, got {}",
            connected.body
        );
        let read = get_authorization(&emulator, &bundle, &arn).await;
        assert_eq!(
            read.body
                .get("appAuthorization")
                .and_then(|authorization| authorization.get("status")),
            Some(&json!(after_connect)),
            "case {case:?}: expected the connect's outcome to be persisted, got {}",
            read.body
        );

        created_arns.push(arn);
    }

    // Every authorization the table created is listed exactly once, in creation
    // order, and nothing else is.
    let listed = list_authorizations(&emulator, &bundle, "").await;
    assert_eq!(
        listed.status, 200,
        "expected HTTP 200 from ListAppAuthorizations, got {} with body {}",
        listed.status, listed.body
    );
    assert_eq!(
        listed_authorization_arns(&listed),
        created_arns,
        "expected the listing to report exactly what was created, got {}",
        listed.body
    );
}

/// RULE: an application name from the set the service serves is stored and
/// reported back as the request carried it, and what `CreateIngestion` requires
/// of such an (application, tenant) pair BEYOND the name is a CONNECTED app
/// authorization — which is what its refusal states, naming neither the
/// application nor the tenant.
///
/// The table spans every name the member's documentation lists, so no single
/// served value stands for the set. A name OUTSIDE the set is refused instead,
/// which `supported_application_names` pins.
#[tokio::test]
async fn a_served_application_name_is_accepted_and_persisted() {
    let apps: Vec<&str> = DOCUMENTED_APPS.to_vec();

    for (index, app) in apps.iter().enumerate() {
        let emulator = Emulator::new();
        let bundle = arn_of(
            &emulator.create_app_bundle(json!({})).await.body,
            "appBundle",
        );
        let tenant = format!("tenant-{index}");

        let created = create_authorization(&emulator, &bundle, app, "apiKey", &tenant).await;
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

        // PERSISTED: the bundle holds exactly that authorization and the reader
        // reports the same application name back.
        let arn = authorization_member(&created, "appAuthorizationArn")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let listed = list_authorizations(&emulator, &bundle, "").await;
        assert_eq!(
            listed_authorization_arns(&listed),
            vec![arn.clone()],
            "case {app:?}: expected the bundle to hold exactly the created authorization, got {}",
            listed.body
        );
        let read = get_authorization(&emulator, &bundle, &arn).await;
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

        // The ingestion of that pair is refused for the connect handshake the
        // authorization has not completed, never for the application's name.
        let refused = create_ingestion_unauthorized(&emulator, &bundle, app, &tenant).await;
        assert_eq!(
            refused.status, 400,
            "case {app:?}: expected HTTP 400 from CreateIngestion, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {app:?}: expected ValidationException from CreateIngestion, got {}",
            refused.body
        );
        assert_eq!(
            message(&refused),
            "The app authorization for the specified application and tenant was not connected. \
             Connect the app authorization for the application and tenant before creating an \
             ingestion.",
            "case {app:?}: expected the connect-the-authorization sentence, got {:?}",
            message(&refused)
        );
        assert!(
            !message(&refused).contains(app.trim()),
            "case {app:?}: the refusal must not echo the application name, got {:?}",
            message(&refused)
        );
    }
}

/// RULE: a bundle holds AT MOST ONE app authorization per (app, tenant) pair.
/// However many callers race for one pair, exactly one succeeds, every loser is
/// refused with a modelled client error (`ConflictException` 409 or
/// `ValidationException` 400), and the bundle afterwards holds exactly one
/// authorization for that pair — the winner's.
///
/// The table spans the number of racers, which is what the invariant has to hold
/// across; the runtime is multi-threaded so the calls genuinely overlap.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_creates_for_one_pair_yield_exactly_one_winner() {
    for racers in [2_usize, 3, 8, 16] {
        let emulator = Arc::new(Emulator::new());
        let bundle = arn_of(
            &emulator.create_app_bundle(json!({})).await.body,
            "appBundle",
        );

        let mut tasks = Vec::with_capacity(racers);
        for _ in 0..racers {
            let emulator = Arc::clone(&emulator);
            let bundle = bundle.clone();
            tasks.push(tokio::spawn(async move {
                create_authorization(&emulator, &bundle, "OKTA", "apiKey", "tenant-race").await
            }));
        }

        let mut winners: Vec<String> = Vec::new();
        for task in tasks {
            let answer = task.await.expect("a racer task never panics");
            if answer.status == 201 {
                winners.push(
                    authorization_member(&answer, "appAuthorizationArn")
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            } else {
                assert!(
                    (answer.status == 409 && answer.error_code() == "ConflictException")
                        || (answer.status == 400 && answer.error_code() == "ValidationException"),
                    "case {racers} racers: expected a loser to be refused 409 ConflictException \
                     or 400 ValidationException, got {} with body {}",
                    answer.status,
                    answer.body
                );
            }
        }

        assert_eq!(
            winners.len(),
            1,
            "case {racers} racers: expected exactly one winner, got {winners:?}"
        );
        let listed = list_authorizations(&emulator, &bundle, "").await;
        assert_eq!(
            listed_authorization_arns(&listed),
            winners,
            "case {racers} racers: expected the bundle to hold exactly the winner, got {}",
            listed.body
        );
    }
}

/// RULE: an ingestion exists only for an (app, tenant) pair whose app
/// authorization in the same bundle has completed the connect handshake. Any
/// other state of that dependency — none at all, one still `PendingConnect`, or
/// one connected for a DIFFERENT app or tenant — is refused with the modelled
/// `ValidationException` (HTTP 400) and leaves the bundle's ingestions empty.
#[tokio::test]
async fn an_ingestion_requires_a_connected_authorization_for_its_pair() {
    /// How far the authorization the bundle holds for a pair has got.
    #[derive(Debug, Clone, Copy)]
    enum Dependency {
        /// The bundle holds none.
        Missing,
        /// Created, so still `PendingConnect`.
        Created,
        /// Created and connected.
        Connected,
    }

    /// One row: the ingestion asked for, the authorization the bundle holds for
    /// some pair, and the status the ingestion call must answer with.
    #[derive(Debug, Clone, Copy)]
    struct Row {
        case: &'static str,
        app: &'static str,
        tenant: &'static str,
        held_app: &'static str,
        held_tenant: &'static str,
        held: Dependency,
        expected: u16,
    }

    let cases: Vec<Row> = vec![
        Row {
            case: "no authorization at all",
            app: "SLACK",
            tenant: "tenant-a",
            held_app: "SLACK",
            held_tenant: "tenant-a",
            held: Dependency::Missing,
            expected: 400,
        },
        Row {
            case: "authorization for the pair, not connected",
            app: "SLACK",
            tenant: "tenant-a",
            held_app: "SLACK",
            held_tenant: "tenant-a",
            held: Dependency::Created,
            expected: 400,
        },
        Row {
            case: "connected authorization for another tenant",
            app: "ASANA",
            tenant: "tenant-b",
            held_app: "ASANA",
            held_tenant: "tenant-other",
            held: Dependency::Connected,
            expected: 400,
        },
        Row {
            case: "connected authorization for another app",
            app: "JIRA",
            tenant: "tenant-c",
            held_app: "ZOOM",
            held_tenant: "tenant-c",
            held: Dependency::Connected,
            expected: 400,
        },
        Row {
            case: "authorization for the pair, not connected, another application",
            app: "GOOGLE",
            tenant: "tenant-d",
            held_app: "GOOGLE",
            held_tenant: "tenant-d",
            held: Dependency::Created,
            expected: 400,
        },
        Row {
            case: "connected authorization for the pair",
            app: "ZENDESK",
            tenant: "tenant-e",
            held_app: "ZENDESK",
            held_tenant: "tenant-e",
            held: Dependency::Connected,
            expected: 201,
        },
        Row {
            case: "connected authorization for the pair, another application",
            app: "SMARTSHEET",
            tenant: "tenant-f",
            held_app: "SMARTSHEET",
            held_tenant: "tenant-f",
            held: Dependency::Connected,
            expected: 201,
        },
    ];

    for row in cases {
        let Row {
            case,
            app,
            tenant,
            held_app,
            held_tenant,
            held,
            expected,
        } = row;
        let emulator = Emulator::new();
        let bundle = arn_of(
            &emulator.create_app_bundle(json!({})).await.body,
            "appBundle",
        );

        if !matches!(held, Dependency::Missing) {
            let created =
                create_authorization(&emulator, &bundle, held_app, "apiKey", held_tenant).await;
            assert_eq!(
                created.status, 201,
                "case {case:?}: the set-up authorization must be created, got {} with body {}",
                created.status, created.body
            );
            if matches!(held, Dependency::Connected) {
                let arn = authorization_member(&created, "appAuthorizationArn")
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let connected = connect_authorization(&emulator, &bundle, &arn, "apiKey").await;
                assert_eq!(
                    connected.status, 200,
                    "case {case:?}: the set-up authorization must connect, got {} with body {}",
                    connected.status, connected.body
                );
            }
        }

        let answer = create_ingestion_unauthorized(&emulator, &bundle, app, tenant).await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} with body {}",
            answer.status, answer.body
        );
        let listed = emulator.list_ingestions(&bundle, "").await;
        let ingestions = listed
            .body
            .get("ingestions")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected ingestions in {}", listed.body));
        if expected == 201 {
            let arn = arn_of(&answer.body, "ingestion");
            let read = emulator.get_ingestion(&bundle, &arn).await;
            assert_eq!(
                read.body.get("ingestion"),
                answer.body.get("ingestion"),
                "case {case:?}: expected the created ingestion to be readable back, got {}",
                read.body
            );
            assert_eq!(
                read.body
                    .get("ingestion")
                    .and_then(|ingestion| ingestion.get("state")),
                Some(&json!(INGESTION_STATE_DISABLED)),
                "case {case:?}: expected a new ingestion to be disabled, got {}",
                read.body
            );
            assert_eq!(
                ingestions.len(),
                1,
                "case {case:?}: expected exactly the created ingestion, got {}",
                listed.body
            );
        } else {
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {}",
                answer.body
            );
            assert!(
                ingestions.is_empty(),
                "case {case:?}: a refused CreateIngestion must persist nothing, got {}",
                listed.body
            );
        }
    }
}

/// Put the stored ingestion into one `IngestionState`.
///
/// The state is written straight into the SAME store the handlers read
/// (`state::service_state` on the engine this emulator was mounted on), so the
/// set-up is the real record rather than a double of one, and the call under
/// test is the only operation these rows exercise.
fn set_ingestion_state(
    engine: &engine::InMemoryEngine,
    bundle: &str,
    ingestion: &str,
    state: &str,
    case: &str,
) {
    let store = service_state(engine).unwrap_or_else(|| panic!("case {case:?}: no appfabric store"));
    let mut store = store.lock();
    let record = store
        .resolve_bundle_mut(bundle)
        .unwrap_or_else(|| panic!("case {case:?}: bundle {bundle} is not stored"))
        .resolve_ingestion_mut(ingestion)
        .unwrap_or_else(|| panic!("case {case:?}: ingestion {ingestion} is not stored"));
    record.state = state.to_string();
}

/// The `state` `GetIngestion` reports for one stored ingestion.
pub(crate) async fn ingestion_state(emulator: &Emulator, bundle: &str, ingestion: &str) -> String {
    let read = emulator.get_ingestion(bundle, ingestion).await;
    read.body
        .get("ingestion")
        .and_then(|ingestion| ingestion.get("state"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected ingestion.state in {}", read.body))
        .to_string()
}

/// RULE: `StopIngestion` moves an `enabled` ingestion to `disabled` and
/// persists the move, so every later read reports `disabled`; an ingestion that
/// is not `enabled` has no such move to make and is refused with the modelled
/// `ConflictException` (HTTP 409), unchanged.
///
/// The table spans both `IngestionState` values and both identifier forms.
#[tokio::test]
async fn stopping_an_ingestion_disables_it_and_refuses_when_there_is_nothing_to_stop() {
    // (case, state before, identifier form, expected status, expected code)
    let cases: Vec<(&str, &str, bool, u16, &str)> = vec![
        ("enabled, by ARN", INGESTION_STATE_ENABLED, true, 200, ""),
        (
            "enabled, by bare uuid",
            INGESTION_STATE_ENABLED,
            false,
            200,
            "",
        ),
        (
            "disabled, by ARN",
            INGESTION_STATE_DISABLED,
            true,
            409,
            "ConflictException",
        ),
        (
            "disabled, by bare uuid",
            INGESTION_STATE_DISABLED,
            false,
            409,
            "ConflictException",
        ),
    ];

    for (case, before, by_arn, expected, expected_code) in cases {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let emulator = Emulator::with_engine(&engine);
        let (bundle, ingestion) = bundle_with_ingestion(&emulator, "SLACK", "tenant-stop", case).await;
        if before != INGESTION_STATE_DISABLED {
            set_ingestion_state(&engine, &bundle, &ingestion, before, case);
        }
        let identifier = if by_arn {
            ingestion.clone()
        } else {
            uuid_of(&ingestion)
        };

        let answer = stop_ingestion(&emulator, &bundle, &identifier).await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            expected_code,
            "case {case:?}: expected error code {expected_code:?}, got {}",
            answer.body
        );
        let after = ingestion_state(&emulator, &bundle, &ingestion).await;
        assert_eq!(
            after, INGESTION_STATE_DISABLED,
            "case {case:?}: expected the stored state to be {INGESTION_STATE_DISABLED:?}, got {after:?}"
        );

        // A stop that found nothing to do is refused the same way the second
        // time: the state, not the call count, is what decides.
        let again = stop_ingestion(&emulator, &bundle, &identifier).await;
        assert_eq!(
            again.status, 409,
            "case {case:?}: expected a second stop to conflict, got {} with body {}",
            again.status, again.body
        );
    }
}

/// RULE: `StopIngestion` answers HTTP 404 `ResourceNotFoundException` for a
/// target the store does not hold, in either identifier form and at either level
/// of the chain, and changes nothing.
#[tokio::test]
async fn stopping_a_target_that_is_not_stored_is_not_found() {
    let emulator = Emulator::new();
    let (bundle, ingestion) =
        bundle_with_ingestion(&emulator, "ASANA", "tenant-missing", "set-up").await;
    let unknown = unknown_uuid();

    // (case, bundle identifier, ingestion identifier)
    let cases: Vec<(&str, String, String)> = vec![
        ("unknown ingestion uuid", bundle.clone(), unknown.clone()),
        (
            "unknown ingestion arn",
            bundle.clone(),
            unknown_ingestion_arn(&uuid_of(&bundle), &unknown),
        ),
        ("unknown bundle uuid", unknown.clone(), ingestion.clone()),
        (
            "unknown bundle arn",
            unknown_bundle_arn(&unknown),
            ingestion.clone(),
        ),
    ];

    for (case, bundle_identifier, ingestion_identifier) in cases {
        let answer = stop_ingestion(&emulator, &bundle_identifier, &ingestion_identifier).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {}",
            answer.body
        );
    }

    let state = ingestion_state(&emulator, &bundle, &ingestion).await;
    assert_eq!(
        state, INGESTION_STATE_DISABLED,
        "a refused stop must leave the stored ingestion as it was, got {state:?}"
    );
}

/// RULE: `ListIngestionDestinations` reports exactly the destinations stored
/// under the ingestion it names, and following its `nextToken` visits each one
/// exactly once whatever page size the caller asks for.
///
/// The table spans the number of stored destinations (none, one, several) and the
/// page size, including the model's own `maxResults` bounds.
#[tokio::test]
async fn listing_ingestion_destinations_pages_through_every_destination_once() {
    for total in [0_usize, 1, 3, 7] {
        let emulator = Emulator::new();
        let (bundle, ingestion) = bundle_with_ingestion(
            &emulator,
            "SLACK",
            &format!("tenant-list-{total}"),
            "set-up",
        )
        .await;

        let mut created: BTreeSet<String> = BTreeSet::new();
        for index in 0..total {
            let answer =
                create_destination(&emulator, &bundle, &ingestion, &format!("bucket-{index}"))
                    .await;
            assert_eq!(
                answer.status, 201,
                "case {total} destinations: expected HTTP 201, got {} with body {}",
                answer.status, answer.body
            );
            created.insert(arn_of(&answer.body, "ingestionDestination"));
        }

        for page_size in [None, Some(1_usize), Some(2), Some(MAX_PAGE_SIZE)] {
            let case = format!("{total} destinations, page size {page_size:?}");
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut query = match page_size {
                    Some(size) => format!("?maxResults={size}"),
                    None => String::new(),
                };
                if let Some(token) = token.as_deref() {
                    query.push(if query.is_empty() { '?' } else { '&' });
                    query.push_str(&format!("nextToken={}", encode(token)));
                }
                let answer = list_destinations(&emulator, &bundle, &ingestion, &query).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected HTTP 200, got {} with body {}",
                    answer.status, answer.body
                );
                let items = answer
                    .body
                    .get("ingestionDestinations")
                    .and_then(Value::as_array)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected ingestionDestinations in {}", answer.body)
                    });
                if let Some(size) = page_size {
                    assert!(
                        items.len() <= size,
                        "case {case:?}: a page must not exceed maxResults, got {} items",
                        items.len()
                    );
                }
                for item in items {
                    seen.push(
                        item.get("arn")
                            .and_then(Value::as_str)
                            .unwrap_or_else(|| panic!("case {case:?}: expected arn in {item}"))
                            .to_string(),
                    );
                }
                token = answer
                    .body
                    .get("nextToken")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                pages += 1;
                assert!(
                    pages <= total + 1,
                    "case {case:?}: pagination returned more pages than the {total} stored destinations"
                );
                if token.is_none() {
                    break;
                }
            }

            let unique: BTreeSet<String> = seen.iter().cloned().collect();
            assert_eq!(
                seen.len(),
                unique.len(),
                "case {case:?}: every destination must be listed once, got {seen:?}"
            );
            assert_eq!(
                unique, created,
                "case {case:?}: expected exactly the created destinations {created:?}, got {unique:?}"
            );
        }
    }
}

/// RULE: a READ of the ingestion-destination chain whose parent the store does
/// not hold is HTTP 404 `ResourceNotFoundException`, whichever level of the
/// chain is missing and in either identifier form.
#[tokio::test]
async fn listing_destinations_of_a_parent_that_is_not_stored_is_not_found() {
    let emulator = Emulator::new();
    let (bundle, _ingestion) =
        bundle_with_ingestion(&emulator, "ZOOM", "tenant-404", "set-up").await;
    let unknown = unknown_uuid();

    // (case, bundle identifier, ingestion identifier, expected message)
    let cases: Vec<(&str, String, String, &str)> = vec![
        (
            "unknown ingestion uuid",
            bundle.clone(),
            unknown.clone(),
            "The specified ingestion does not exist",
        ),
        (
            "unknown ingestion arn",
            bundle.clone(),
            unknown_ingestion_arn(&uuid_of(&bundle), &unknown),
            "The specified ingestion does not exist",
        ),
        (
            "unknown bundle uuid",
            unknown.clone(),
            unknown.clone(),
            "The app bundle ID was not found.",
        ),
        (
            "unknown bundle arn",
            unknown_bundle_arn(&unknown),
            unknown.clone(),
            "The app bundle ID was not found.",
        ),
    ];

    for (case, bundle_identifier, ingestion_identifier, expected) in cases {
        let answer =
            list_destinations(&emulator, &bundle_identifier, &ingestion_identifier, "").await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {}",
            answer.body
        );
        assert_eq!(
            message(&answer),
            expected,
            "case {case:?}: expected the message {expected:?}, got {:?}",
            message(&answer)
        );
    }
}

/// RULE: a violation of a numeric `@range` bound is refused with HTTP 400
/// `ValidationException` whose message names the member path and the violated
/// bound and WITHHOLDS the number that was submitted; a value inside the bound —
/// including each published bound itself — is accepted.
///
/// The table spans every listing that binds `maxResults` (`@range(min: 1, max:
/// 100)`), values past both published bounds, and the bounds themselves.
#[tokio::test]
async fn a_numeric_bound_violation_names_the_member_and_the_bound_but_not_the_value() {
    let emulator = Emulator::new();
    let (bundle, ingestion) =
        bundle_with_ingestion(&emulator, "JIRA", "tenant-bounds", "set-up").await;

    // Every operation whose `maxResults` the model bounds at 1..=100.
    let listings: Vec<(&str, String)> = vec![
        ("ListAppBundles", "/appbundles".to_string()),
        (
            "ListIngestions",
            format!("/appbundles/{}/ingestions", encode(&bundle)),
        ),
        (
            "ListAppAuthorizations",
            format!("/appbundles/{}/appauthorizations", encode(&bundle)),
        ),
        (
            "ListIngestionDestinations",
            format!(
                "/appbundles/{}/ingestions/{}/ingestiondestinations",
                encode(&bundle),
                encode(&ingestion)
            ),
        ),
    ];

    // (submitted value, the bound clause it violates); `0` and `-1` are below
    // the published minimum, the rest above the published maximum.
    let refused: Vec<(i64, String)> = vec![
        (101, "Member must have value less than or equal to 100".to_string()),
        (1_000, "Member must have value less than or equal to 100".to_string()),
        (
            i64::from(i32::MAX),
            "Member must have value less than or equal to 100".to_string(),
        ),
        (0, "Member must have value greater than or equal to 1".to_string()),
        (-1, "Member must have value greater than or equal to 1".to_string()),
    ];

    for (operation, path) in &listings {
        for (value, requirement) in &refused {
            let case = format!("{operation} maxResults={value}");
            let answer = emulator
                .call("GET", &format!("{path}?maxResults={value}"), None)
                .await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {}",
                answer.body
            );
            assert_eq!(
                message(&answer),
                format!(
                    "1 validation error detected: Value at 'maxResults' \
                     failed to satisfy constraint: {requirement}"
                ),
                "case {case:?}: wrong refusal message, got {:?}",
                message(&answer)
            );
            assert!(
                !message(&answer).contains(&format!("'{value}'")),
                "case {case:?}: the message must not echo the submitted value, got {:?}",
                message(&answer)
            );
        }

        // The published bounds themselves, and a value between them, are served.
        for value in [1, 50, 100] {
            let case = format!("{operation} maxResults={value}");
            let answer = emulator
                .call("GET", &format!("{path}?maxResults={value}"), None)
                .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200 inside the bound, got {} with body {}",
                answer.status, answer.body
            );
        }
    }
}

/// RULE: a not-found refusal states a CONSTANT message for its resource kind and
/// path — the reporting path reports the resource was not found, the mutation
/// path reports the identifier is not valid — and neither echoes the identifier
/// the caller supplied, so two refusals naming two different resources carry the
/// same sentence.
///
/// The table spans two independently created (then deleted) bundles, both
/// identifier forms, and an identifier no create call ever minted.
#[tokio::test]
async fn a_not_found_refusal_states_a_constant_message_that_never_echoes_the_identifier() {
    let emulator = Emulator::new();

    let mut identifiers: Vec<(String, String)> = Vec::new();
    for index in 0..2 {
        let arn = arn_of(
            &emulator.create_app_bundle(json!({})).await.body,
            "appBundle",
        );
        let deleted = emulator
            .call("DELETE", &format!("/appbundles/{}", encode(&arn)), None)
            .await;
        assert!(
            deleted.status < 300,
            "bundle {index}: expected the delete to succeed, got {} with body {}",
            deleted.status,
            deleted.body
        );
        identifiers.push((format!("deleted bundle {index} by arn"), arn.clone()));
        identifiers.push((format!("deleted bundle {index} by uuid"), uuid_of(&arn)));
    }
    identifiers.push(("never minted uuid".to_string(), unknown_uuid()));
    identifiers.push((
        "never minted arn".to_string(),
        unknown_bundle_arn(&unknown_uuid()),
    ));

    for (case, identifier) in &identifiers {
        // The REPORTING path.
        let read = emulator
            .call("GET", &format!("/appbundles/{}", encode(identifier)), None)
            .await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: expected HTTP 404 from GetAppBundle, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            read.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {}",
            read.body
        );
        assert_eq!(
            message(&read),
            "The app bundle ID was not found.",
            "case {case:?}: wrong read-path message, got {:?}",
            message(&read)
        );

        // The MUTATION path, which AppFabric words differently.
        let written = create_authorization(&emulator, identifier, "SLACK", "apiKey", "t").await;
        assert_eq!(
            written.status, 404,
            "case {case:?}: expected HTTP 404 from CreateAppAuthorization, got {} with body {}",
            written.status, written.body
        );
        assert_eq!(
            message(&written),
            "The specified app bundle ID is not valid",
            "case {case:?}: wrong mutation-path message, got {:?}",
            message(&written)
        );

        for (path, answer) in [("read", &read), ("mutation", &written)] {
            assert!(
                !message(answer).contains(identifier.as_str()),
                "case {case:?} ({path}): the message must not echo the identifier, got {:?}",
                message(answer)
            );
        }
    }

    // And the same for the ingestion level: one constant sentence, whichever
    // identifier was asked for.
    let bundle = arn_of(
        &emulator.create_app_bundle(json!({})).await.body,
        "appBundle",
    );
    for identifier in [
        unknown_uuid(),
        unknown_ingestion_arn(&uuid_of(&bundle), &unknown_uuid()),
    ] {
        let read = emulator.get_ingestion(&bundle, &identifier).await;
        assert_eq!(
            message(&read),
            "The specified ingestion does not exist",
            "case {identifier:?}: wrong ingestion read-path message, got {:?}",
            message(&read)
        );
        assert!(
            !message(&read).contains(identifier.as_str()),
            "case {identifier:?}: the message must not echo the identifier, got {:?}",
            message(&read)
        );
    }
}
