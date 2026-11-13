// SPDX-License-Identifier: CC0-1.0

//! The Event API (`Api`) create-and-read path, end to end over HTTP.
//!
//! Three rules, each asserted over a TABLE that spans the class it generalises over
//! rather than one request:
//!
//! * a create mints the resource's identity (an `apiId`, its ARN and the endpoint
//!   hostnames it is served on), persists everything the request carried, and answers
//!   from that stored record — so the reader reports the same body;
//! * an authorization mode is valid only where the API's own authorization providers
//!   set its type up, and a provider configured through a member of its own needs that
//!   member; a create that breaks either is refused with `BadRequestException` (400)
//!   and persists nothing;
//! * an `Api` is a resource of its own: the reader resolves an `Api` id only, so an id
//!   that names a GraphQL API — or nothing at all — is a not-found, while an id longer
//!   than an AppSync API id is a bad request.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization, the handlers and the shared engine
//! together.
//!
//! The harness (`call`, `router`, `Answer`, `create_api`) is the error-contract suite's.

use serde_json::{json, Value};

use crate::error_contract::{call, create_api, router, Answer};
use crate::graphql_api_lifecycle::REGION;

/// The account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The number of characters an AppSync API id has; an id of exactly this length is
/// well formed, and `state::API_ID_LEN` is the service's own copy.
const API_ID_LEN: usize = 26;

/// The fixed text AppSync answers for an API that is absent.
const API_NOT_FOUND: &str = "API not found.";

/// `AuthenticationType`'s whole value set, in the order the model declares it.
/// Source: `smithy_query(query="shape", shape="AuthenticationType")`.
pub(crate) const AUTHENTICATION_TYPES: &[&str] = &[
    "API_KEY",
    "AWS_IAM",
    "AMAZON_COGNITO_USER_POOLS",
    "OPENID_CONNECT",
    "AWS_LAMBDA",
];

/// One `authProviders` entry for `auth_type`, carrying the configuration member that
/// type is set up through.
///
/// Source: the `AuthProvider` shape — `cognitoConfig`, `openIDConnectConfig` and
/// `lambdaAuthorizerConfig`, with the members each of those shapes requires
/// (`smithy_query(query="closure", operation="CreateApi")`). `API_KEY` and `AWS_IAM`
/// are configured by nothing, so their provider carries only the type.
fn provider(auth_type: &str) -> Value {
    match auth_type {
        "AMAZON_COGNITO_USER_POOLS" => json!({
            "authType": auth_type,
            "cognitoConfig": {"userPoolId": "pool_events", "awsRegion": REGION},
        }),
        "OPENID_CONNECT" => json!({
            "authType": auth_type,
            "openIDConnectConfig": {"issuer": "https://issuer.example.com/"},
        }),
        "AWS_LAMBDA" => json!({
            "authType": auth_type,
            "lambdaAuthorizerConfig": {
                "authorizerUri":
                    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:events_authorizer"),
            },
        }),
        _ => json!({"authType": auth_type}),
    }
}

/// The `authProviders` list for every type in `auth_types`.
fn providers(auth_types: &[&str]) -> Value {
    Value::Array(auth_types.iter().map(|ty| provider(ty)).collect())
}

/// An auth-mode list naming every type in `auth_types`.
fn modes(auth_types: &[&str]) -> Value {
    Value::Array(
        auth_types
            .iter()
            .map(|ty| json!({"authType": ty}))
            .collect(),
    )
}

/// One row of the create table: the `CreateApi` request to send.
struct CreateRow {
    label: &'static str,
    name: String,
    owner_contact: Option<&'static str>,
    tags: Option<Value>,
    event_config: Value,
}

impl CreateRow {
    /// This row as a `CreateApi` request body.
    fn body(&self) -> Value {
        let mut body = json!({"name": self.name, "eventConfig": self.event_config});
        if let Some(contact) = self.owner_contact {
            body["ownerContact"] = json!(contact);
        }
        if let Some(tags) = &self.tags {
            body["tags"] = tags.clone();
        }
        body
    }

    /// The tag map the created API must report: the pairs the request carried, or the
    /// empty map for a request that carried no `tags` member at all.
    fn expected_tags(&self) -> Value {
        self.tags.clone().unwrap_or_else(|| json!({}))
    }
}

/// An `eventConfig` whose providers set up `auth_types` and whose three auth-mode
/// lists name `connection`, `publish` and `subscribe` respectively.
pub(crate) fn event_config(
    auth_types: &[&str],
    connection: &[&str],
    publish: &[&str],
    subscribe: &[&str],
    log_config: Option<Value>,
) -> Value {
    let mut config = json!({
        "authProviders": providers(auth_types),
        "connectionAuthModes": modes(connection),
        "defaultPublishAuthModes": modes(publish),
        "defaultSubscribeAuthModes": modes(subscribe),
    });
    if let Some(log_config) = log_config {
        config["logConfig"] = log_config;
    }
    config
}

/// The create table: the rows differ along what the rule generalises over — which
/// authorization types the API sets up, how many of them, which of them each mode
/// list draws on, whether the configuration carries a `logConfig`, whether the request
/// carries `tags` and `ownerContact`, and the span of `ApiName`'s own length bounds
/// (`@length` 1..50, `smithy_query(query="operation", operation="CreateApi")`).
fn create_rows() -> Vec<CreateRow> {
    vec![
        CreateRow {
            label: "one provider, no tags, no ownerContact",
            name: "Events".to_string(),
            owner_contact: None,
            tags: None,
            event_config: event_config(
                &["API_KEY"],
                &["API_KEY"],
                &["API_KEY"],
                &["API_KEY"],
                None,
            ),
        },
        CreateRow {
            label: "an IAM-authorized API with tags and an ownerContact",
            name: "Events_iam-api".to_string(),
            owner_contact: Some("platform.team"),
            tags: Some(json!({"team": "events", "tier": "gold"})),
            event_config: event_config(
                &["AWS_IAM"],
                &["AWS_IAM"],
                &["AWS_IAM"],
                &["AWS_IAM"],
                None,
            ),
        },
        CreateRow {
            label: "every modelled authorization type, each mode list on a different one",
            name: "Events all providers".to_string(),
            owner_contact: Some("owner.one"),
            tags: Some(json!({"purpose": "fanout"})),
            event_config: event_config(
                AUTHENTICATION_TYPES,
                &["API_KEY", "AWS_IAM"],
                &["AMAZON_COGNITO_USER_POOLS"],
                &["OPENID_CONNECT", "AWS_LAMBDA"],
                // `EventLogConfig` requires both members
                // (`smithy_query(query="shape", shape="EventLogConfig")`); `ALL` is
                // one of `EventLogLevel`'s declared values.
                Some(json!({
                    "logLevel": "ALL",
                    "cloudWatchLogsRoleArn":
                        format!("arn:aws:iam::{ACCOUNT}:role/service-role/appsync-events"),
                })),
            ),
        },
        CreateRow {
            label: "a single-character name on a Cognito-authorized API",
            name: "E".to_string(),
            owner_contact: None,
            tags: Some(json!({"team": "events"})),
            event_config: event_config(
                &["AMAZON_COGNITO_USER_POOLS"],
                &["AMAZON_COGNITO_USER_POOLS"],
                &["AMAZON_COGNITO_USER_POOLS"],
                &["AMAZON_COGNITO_USER_POOLS"],
                Some(json!({
                    "logLevel": "NONE",
                    "cloudWatchLogsRoleArn":
                        format!("arn:aws:iam::{ACCOUNT}:role/service-role/appsync-quiet"),
                })),
            ),
        },
        CreateRow {
            // `ApiName`'s published maximum length.
            label: "a name of exactly the modelled maximum length",
            name: "N".repeat(50),
            owner_contact: None,
            tags: None,
            event_config: event_config(
                &["OPENID_CONNECT", "AWS_LAMBDA"],
                &["AWS_LAMBDA"],
                &["OPENID_CONNECT"],
                &["OPENID_CONNECT", "AWS_LAMBDA"],
                None,
            ),
        },
    ]
}

/// Create one Event API and return the answer.
pub(crate) async fn create_event_api(
    router: &smithy_server_runtime::MultiServiceRouter,
    body: Value,
) -> Answer {
    call(router, "POST", "/v2/apis", Some(body)).await
}

/// The address the core binds a resource listener on by default, and so the host an Event
/// API's two endpoints are published on.
pub(crate) const LOOPBACK_HOST: &str = "127.0.0.1";

/// Rule 1: a create mints the API's identity, persists what the request carried, and
/// answers from the stored record — which is the record the reader reports.
#[tokio::test]
async fn a_created_event_api_is_answered_from_the_record_the_reader_reports() {
    let router = router();

    for row in create_rows() {
        let case = row.label;
        let created = create_event_api(&router, row.body()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateApi 200, got {created:?}"
        );
        let api = created.body["api"].clone();

        let api_id = api["apiId"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            api_id.chars().count(),
            API_ID_LEN,
            "case {case:?}: expected a minted apiId of {API_ID_LEN} characters, got {api:?}"
        );
        assert_eq!(
            api["name"].as_str(),
            Some(row.name.as_str()),
            "case {case:?}: expected the requested name back, got {api:?}"
        );
        assert_eq!(
            api["apiArn"].as_str(),
            Some(format!("arn:aws:appsync:{REGION}:{ACCOUNT}:apis/{api_id}").as_str()),
            "case {case:?}: expected the API's own ARN, got {api:?}"
        );
        assert_eq!(
            api["ownerContact"].as_str(),
            row.owner_contact,
            "case {case:?}: expected the requested ownerContact back, got {api:?}"
        );
        assert_eq!(
            api["tags"],
            row.expected_tags(),
            "case {case:?}: expected the requested tags back, got {api:?}"
        );
        assert_eq!(
            api["xrayEnabled"].as_bool(),
            Some(false),
            "case {case:?}: a create turns on no X-Ray tracing, got {api:?}"
        );
        assert!(
            api["wafWebAclArn"].is_null(),
            "case {case:?}: a fresh API is associated with no web ACL, got {api:?}"
        );
        assert!(
            api["created"].as_f64().is_some_and(|epoch| epoch > 0.0),
            "case {case:?}: expected a creation timestamp, got {api:?}"
        );

        // The DNS records publish the two endpoints the API is SERVED on — "an HTTP and a
        // real-time endpoint" (`Api$dns`) — as the `host:port` of each endpoint's own
        // socket on this emulator's interface, because the whole purpose of the value is
        // that a client dials it and no client on this machine resolves an AWS hostname.
        // The two endpoints are distinct, and both are reached at the address the core
        // bound.
        let http = api["dns"]["HTTP"].as_str().unwrap_or_default().to_string();
        let realtime = api["dns"]["REALTIME"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        for (endpoint, published) in [("HTTP", &http), ("REALTIME", &realtime)] {
            let (host, port) = published
                .rsplit_once(':')
                .unwrap_or_else(|| panic!("case {case:?}: expected a host:port, got {api:?}"));
            assert_eq!(
                host, LOOPBACK_HOST,
                "case {case:?}: expected dns.{endpoint} on the address the core binds, \
                 got {api:?}"
            );
            assert!(
                port.parse::<u16>().is_ok_and(|port| port > 0),
                "case {case:?}: expected dns.{endpoint} to name the port its socket landed \
                 on, got {api:?}"
            );
        }
        assert_ne!(
            http, realtime,
            "case {case:?}: expected the two endpoints on sockets of their own, got {api:?}"
        );

        // The configuration comes back as it was sent: every provider and every
        // auth-mode list, in order.
        let requested = &row.event_config;
        let reported = &api["eventConfig"];
        assert_eq!(
            auth_types(&reported["authProviders"]),
            auth_types(&requested["authProviders"]),
            "case {case:?}: expected the configured providers back in order, got {api:?}"
        );
        for member in [
            "connectionAuthModes",
            "defaultPublishAuthModes",
            "defaultSubscribeAuthModes",
        ] {
            assert_eq!(
                auth_types(&reported[member]),
                auth_types(&requested[member]),
                "case {case:?}: expected {member} back in order, got {api:?}"
            );
        }
        assert_eq!(
            reported["logConfig"], requested["logConfig"],
            "case {case:?}: expected the logConfig back, got {api:?}"
        );
        for provider in reported["authProviders"].as_array().unwrap_or(&Vec::new()) {
            let requested_provider = requested["authProviders"]
                .as_array()
                .and_then(|list| {
                    list.iter()
                        .find(|candidate| candidate["authType"] == provider["authType"])
                })
                .cloned()
                .unwrap_or(Value::Null);
            for member in ["cognitoConfig", "openIDConnectConfig", "lambdaAuthorizerConfig"] {
                if requested_provider[member].is_null() {
                    continue;
                }
                for (key, value) in requested_provider[member]
                    .as_object()
                    .cloned()
                    .unwrap_or_default()
                {
                    assert_eq!(
                        provider[member][&key], value,
                        "case {case:?}: expected {member}.{key} back, got {provider:?}"
                    );
                }
            }
        }

        // The reader answers the stored record, so it reports exactly this body.
        let read = call(&router, "GET", &format!("/v2/apis/{api_id}"), None).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected GetApi 200, got {read:?}"
        );
        assert_eq!(
            read.body["api"], api,
            "case {case:?}: expected GetApi to answer the created API {api:?}, got {read:?}"
        );
    }
}

/// The `authType` of every entry of an `authProviders` or auth-mode list, in order.
fn auth_types(list: &Value) -> Vec<String> {
    list.as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry["authType"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Two creates are two APIs: the identity is minted per call, never derived from the
/// request, so the same body twice yields two independently readable APIs.
#[tokio::test]
async fn an_identical_request_creates_a_second_independent_api() {
    let router = router();
    let body = event_config(
        &["API_KEY"],
        &["API_KEY"],
        &["API_KEY"],
        &["API_KEY"],
        None,
    );
    let request = json!({"name": "Events twin", "eventConfig": body});

    let first = create_event_api(&router, request.clone()).await;
    let second = create_event_api(&router, request.clone()).await;
    for (case, answer) in [("the first create", &first), ("the second create", &second)] {
        assert_eq!(
            answer.status, 200,
            "case {case}: expected CreateApi 200, got {answer:?}"
        );
    }
    let first_id = first.body["api"]["apiId"].as_str().unwrap_or_default();
    let second_id = second.body["api"]["apiId"].as_str().unwrap_or_default();
    assert_ne!(
        first_id, second_id,
        "case two identical creates: expected two minted ids, got {first_id:?} and {second_id:?}"
    );
    assert_ne!(
        first.body["api"]["dns"], second.body["api"]["dns"],
        "case two identical creates: expected each API on its own endpoints, got {:?} and {:?}",
        first.body["api"]["dns"], second.body["api"]["dns"]
    );
    for (case, id, expected) in [
        ("the first API", first_id, &first.body["api"]),
        ("the second API", second_id, &second.body["api"]),
    ] {
        let read = call(&router, "GET", &format!("/v2/apis/{id}"), None).await;
        assert_eq!(
            read.body["api"], *expected,
            "case {case}: expected the API to read back as created, got {read:?}"
        );
    }
}

/// Rule 2a: an auth-mode list is "a list of valid authorization modes" — a mode whose
/// type the API's own providers do not set up is refused, on every one of the three
/// lists, and the refused create persists nothing.
///
/// The refusal is AppSync's own fixed sentence naming the CALL that list authorizes
/// (`state::invalid_event_config_auth_message`), so it states neither the member nor the
/// type the request named.
#[tokio::test]
async fn an_auth_mode_outside_the_configured_providers_is_refused() {
    let router = router();

    // The rows differ along the member that carries the offending mode — with the call
    // its refusal names, as the model's three lists map onto AppSync's sentence — and
    // along the type it names: every modelled type the one configured provider does not
    // set up.
    for (member, call) in [
        ("connectionAuthModes", "connect"),
        ("defaultPublishAuthModes", "publish"),
        ("defaultSubscribeAuthModes", "subscribe"),
    ] {
        for unconfigured in AUTHENTICATION_TYPES.iter().filter(|ty| **ty != "API_KEY") {
            let case = format!("{member} naming {unconfigured}");
            let mut config = event_config(
                &["API_KEY"],
                &["API_KEY"],
                &["API_KEY"],
                &["API_KEY"],
                None,
            );
            config[member] = modes(&["API_KEY", unconfigured]);
            let answer =
                create_event_api(&router, json!({"name": "Events_refused", "eventConfig": config}))
                    .await;
            assert_eq!(
                answer.status, 400,
                "case {case}: expected status 400, got {answer:?}"
            );
            assert_eq!(
                answer.error_type, "BadRequestException",
                "case {case}: expected error type BadRequestException, got {answer:?}"
            );
            assert_eq!(
                answer.message,
                format!("Event API configuration {call} auth contains an invalid auth type."),
                "case {case}: expected the sentence naming the {call} call, got {answer:?}"
            );
            assert!(
                !answer.message.contains(member) && !answer.message.contains(*unconfigured),
                "case {case}: expected the sentence to state nothing of the request, \
                 got {answer:?}"
            );
            assert!(
                answer.body["api"].is_null(),
                "case {case}: a refused create answers no API, got {answer:?}"
            );
        }
    }

    // The refusals wrote nothing, so a well-formed create still succeeds and the API
    // it creates is the only one readable.
    let accepted = create_event_api(
        &router,
        json!({
            "name": "Events_accepted",
            "eventConfig": event_config(
                &["API_KEY"], &["API_KEY"], &["API_KEY"], &["API_KEY"], None,
            ),
        }),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case a well-formed create after the refusals: expected 200, got {accepted:?}"
    );
}

/// Rule 2b: a provider whose authorization type is set up through a member of its own
/// needs that member — and is accepted once it carries it.
#[tokio::test]
async fn a_provider_without_the_configuration_its_type_needs_is_refused() {
    let router = router();

    for auth_type in AUTHENTICATION_TYPES {
        // The bare provider: the type alone, with no configuration member.
        let bare = json!([{"authType": auth_type}]);
        let mut config = event_config(
            &[auth_type],
            &[auth_type],
            &[auth_type],
            &[auth_type],
            None,
        );
        config["authProviders"] = bare;
        let answer = create_event_api(
            &router,
            json!({"name": "Events_bare", "eventConfig": config}),
        )
        .await;

        // `API_KEY` and `AWS_IAM` are configured by nothing, so the bare provider is
        // the whole configuration for them; every other type needs its own member.
        let needs_config = !matches!(*auth_type, "API_KEY" | "AWS_IAM");
        if needs_config {
            assert_eq!(
                answer.status, 400,
                "case {auth_type} without its configuration: expected status 400, got {answer:?}"
            );
            assert_eq!(
                answer.error_type, "BadRequestException",
                "case {auth_type} without its configuration: expected BadRequestException, \
                 got {answer:?}"
            );
        } else {
            assert_eq!(
                answer.status, 200,
                "case {auth_type} needs no configuration: expected status 200, got {answer:?}"
            );
        }

        // The same type, configured: accepted, and the configuration is reported back.
        let configured = create_event_api(
            &router,
            json!({
                "name": "Events_configured",
                "eventConfig": event_config(
                    &[auth_type], &[auth_type], &[auth_type], &[auth_type], None,
                ),
            }),
        )
        .await;
        assert_eq!(
            configured.status, 200,
            "case {auth_type} configured: expected status 200, got {configured:?}"
        );
        assert_eq!(
            auth_types(&configured.body["api"]["eventConfig"]["authProviders"]),
            vec![auth_type.to_string()],
            "case {auth_type} configured: expected the provider back, got {configured:?}"
        );
    }

    // An OIDC provider is identified by its issuer, which has to be an absolute URL.
    let mut config = event_config(
        &["OPENID_CONNECT"],
        &["OPENID_CONNECT"],
        &["OPENID_CONNECT"],
        &["OPENID_CONNECT"],
        None,
    );
    config["authProviders"] =
        json!([{"authType": "OPENID_CONNECT", "openIDConnectConfig": {"issuer": "issuer"}}]);
    let answer = create_event_api(
        &router,
        json!({"name": "Events_issuer", "eventConfig": config}),
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "case an issuer that is not a URL: expected status 400, got {answer:?}"
    );
    assert_eq!(
        answer.error_type, "BadRequestException",
        "case an issuer that is not a URL: expected BadRequestException, got {answer:?}"
    );

    // An authorization type identifies ONE provider, so the API cannot set it up twice.
    let mut config = event_config(
        &["API_KEY"],
        &["API_KEY"],
        &["API_KEY"],
        &["API_KEY"],
        None,
    );
    config["authProviders"] = providers(&["API_KEY", "API_KEY"]);
    let answer = create_event_api(
        &router,
        json!({"name": "Events_twice", "eventConfig": config}),
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "case one type set up twice: expected status 400, got {answer:?}"
    );
    assert_eq!(
        answer.error_type, "BadRequestException",
        "case one type set up twice: expected BadRequestException, got {answer:?}"
    );
}

/// Rule 3: the reader resolves an `Api` id and nothing else — the id's SHAPE decides
/// bad-request from not-found, and an id that names a GraphQL API names no `Api`.
#[tokio::test]
async fn the_reader_resolves_an_event_api_id_only() {
    let router = router();

    let created = create_event_api(
        &router,
        json!({
            "name": "Events_reader",
            "eventConfig": event_config(
                &["API_KEY"], &["API_KEY"], &["API_KEY"], &["API_KEY"], None,
            ),
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case seed an Event API: expected 200, got {created:?}"
    );
    let event_api_id = created.body["api"]["apiId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // A GraphQL API is created by the other create operation, so its id names no `Api`.
    let graphql_api_id = create_api(&router, "Api_graphql_reader").await;

    // (label, id, expected status). The rows span the id's length from one character
    // to past the API-id length, plus the two ids this engine really holds.
    let rows: Vec<(String, String, u16)> = vec![
        ("the Event API's own id".to_string(), event_api_id.clone(), 200),
        ("a GraphQL API's id".to_string(), graphql_api_id, 404),
        ("one character".to_string(), "a".to_string(), 404),
        (
            "an id of the full length nothing created".to_string(),
            "b".repeat(API_ID_LEN),
            404,
        ),
        (
            "one past the API-id length".to_string(),
            "c".repeat(API_ID_LEN + 1),
            400,
        ),
        ("64 characters".to_string(), "d".repeat(64), 400),
    ];

    for (case, id, expected) in rows {
        let answer = call(&router, "GET", &format!("/v2/apis/{id}"), None).await;
        assert_eq!(
            answer.status, expected,
            "case {case}: expected status {expected}, got {answer:?}"
        );
        match expected {
            200 => assert_eq!(
                answer.body["api"]["apiId"].as_str(),
                Some(event_api_id.as_str()),
                "case {case}: expected the stored API, got {answer:?}"
            ),
            404 => {
                assert_eq!(
                    answer.error_type, "NotFoundException",
                    "case {case}: expected error type NotFoundException, got {answer:?}"
                );
                assert_eq!(
                    answer.message, API_NOT_FOUND,
                    "case {case}: expected AppSync's absent-API text, got {answer:?}"
                );
            }
            _ => {
                assert_eq!(
                    answer.error_type, "BadRequestException",
                    "case {case}: expected error type BadRequestException, got {answer:?}"
                );
                assert!(
                    answer.message.contains(&id),
                    "case {case}: expected the refusal to name the rejected id, got {answer:?}"
                );
            }
        }
    }
}
