// SPDX-License-Identifier: CC0-1.0

//! `UpdateIdentityProvider` over the wire, driven through the router exactly as an SDK
//! drives it (`POST /` with `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! Two rules are under test.
//!
//! A partial update: a member the request carries replaces what the stored provider
//! held, a member it leaves out keeps its stored value, and the provider's pool, name,
//! type and creation instant — none of them members of this input — stay as
//! `CreateIdentityProvider` stored them. The response is the STORED provider, so a later
//! `DescribeIdentityProvider` renders exactly the same resource.
//!
//! Endpoint resolution: an OIDC IdP is reached at four endpoints, and "Amazon Cognito
//! accepts the following elements when it can't discover endpoint URLs from
//! `oidc_issuer`: `attributes_url`, `authorize_url`, `jwks_uri`, `token_url`"
//! (`smithy_query(query="doc",shape="UpdateIdentityProviderRequest",member="ProviderDetails")`).
//! A submitted detail map that states all four resolves without reading anything from
//! the IdP; one that leaves any of them out has to be resolved by reading the issuer's
//! discovery document, which means looking its host up, and the emulator reaches no host
//! but its own — so the call is refused with a 400 `InvalidParameterException` naming
//! the host it could not reach, and the stored provider is left exactly as it was.
//!
//! Each table row names the case it asserts.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A request's SigV4 credential scope is what routes it to this service. The signature
/// is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::EmulatedService::new(engine),
    );
    router.set_request_authentication(false);
    router
}

/// Call `operation` with `body` and return its status and JSON response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "cognito-idp.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, payload)
}

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// The `__type` of an error body, which is the wire error code.
fn error_code(payload: &Value) -> &str {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// The `message` of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Create a pool and return its minted `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// A JSON object from key/value pairs, in the order given.
fn object(entries: &[(String, String)]) -> Value {
    let mut map = Map::new();
    for (key, value) in entries {
        map.insert(key.clone(), json!(value));
    }
    Value::Object(map)
}

/// Owned key/value pairs from borrowed ones, so a row can be written inline.
fn pairs(entries: &[(&str, &str)]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Owned strings from borrowed ones.
fn list(entries: &[&str]) -> Vec<String> {
    entries.iter().map(|entry| (*entry).to_string()).collect()
}

/// Every endpoint an OIDC detail map can state, beside the issuer and the client
/// credentials, for an IdP served from `host` — the full map the model's
/// `ProviderDetails` "create or update request" example carries.
fn oidc_details(host: &str) -> Vec<(String, String)> {
    vec![
        ("attributes_request_method".to_string(), "GET".to_string()),
        (
            "attributes_url".to_string(),
            format!("https://{host}/userInfo"),
        ),
        (
            "authorize_scopes".to_string(),
            "openid profile email".to_string(),
        ),
        (
            "authorize_url".to_string(),
            format!("https://{host}/authorize"),
        ),
        ("client_id".to_string(), "1example23456789".to_string()),
        (
            "client_secret".to_string(),
            "provider-app-client-secret".to_string(),
        ),
        (
            "jwks_uri".to_string(),
            format!("https://{host}/.well-known/jwks.json"),
        ),
        ("oidc_issuer".to_string(), format!("https://{host}")),
        ("token_url".to_string(), format!("https://{host}/token")),
    ]
}

/// `oidc_details` without `key`, so a row can drop exactly one endpoint.
fn oidc_details_without(host: &str, keys: &[&str]) -> Vec<(String, String)> {
    oidc_details(host)
        .into_iter()
        .filter(|(key, _)| !keys.contains(&key.as_str()))
        .collect()
}

/// A stored provider to update: what `CreateIdentityProvider` puts in the pool.
struct Stored {
    provider_name: String,
    provider_type: &'static str,
    details: Vec<(String, String)>,
    attribute_mapping: Vec<(String, String)>,
    idp_identifiers: Vec<String>,
    acr_mapping: Option<Vec<(String, String)>>,
}

impl Stored {
    /// Create this provider in `user_pool_id` and return the created provider.
    async fn create(&self, router: &MultiServiceRouter, user_pool_id: &str) -> Value {
        let mut body = Map::new();
        body.insert("UserPoolId".to_string(), json!(user_pool_id));
        body.insert("ProviderName".to_string(), json!(self.provider_name));
        body.insert("ProviderType".to_string(), json!(self.provider_type));
        body.insert("ProviderDetails".to_string(), object(&self.details));
        body.insert(
            "AttributeMapping".to_string(),
            object(&self.attribute_mapping),
        );
        body.insert("IdpIdentifiers".to_string(), json!(self.idp_identifiers));
        if let Some(mapping) = &self.acr_mapping {
            body.insert("AcrMapping".to_string(), object(mapping));
        }
        let created = ok(
            router,
            "CreateIdentityProvider",
            &Value::Object(body),
        )
        .await;
        created["IdentityProvider"].clone()
    }
}

/// One row of the partial-update table: the provider to store, and the members the
/// update submits. A member left `None` is left OUT of the update request, so the row
/// asserts the stored value survives it.
struct UpdateCase {
    case: &'static str,
    stored: Stored,
    details: Option<Vec<(String, String)>>,
    attribute_mapping: Option<Vec<(String, String)>>,
    idp_identifiers: Option<Vec<String>>,
    acr_mapping: Option<Vec<(String, String)>>,
}

impl UpdateCase {
    /// This row's `UpdateIdentityProvider` request for `user_pool_id`.
    fn request(&self, user_pool_id: &str) -> Value {
        let mut body = Map::new();
        body.insert("UserPoolId".to_string(), json!(user_pool_id));
        body.insert(
            "ProviderName".to_string(),
            json!(self.stored.provider_name),
        );
        if let Some(details) = &self.details {
            body.insert("ProviderDetails".to_string(), object(details));
        }
        if let Some(mapping) = &self.attribute_mapping {
            body.insert("AttributeMapping".to_string(), object(mapping));
        }
        if let Some(identifiers) = &self.idp_identifiers {
            body.insert("IdpIdentifiers".to_string(), json!(identifiers));
        }
        if let Some(mapping) = &self.acr_mapping {
            body.insert("AcrMapping".to_string(), object(mapping));
        }
        Value::Object(body)
    }

    /// What the updated provider must render for `ProviderDetails`: the submitted map,
    /// and the stored one when the update left the member out, either way completed by
    /// the entries the provider's type resolves
    /// ([`super::identity_provider_type_defaults`]).
    fn expected_details(&self) -> Value {
        super::identity_provider_type_defaults::resolved_details(
            self.stored.provider_type,
            &object(self.details.as_ref().unwrap_or(&self.stored.details)),
        )
    }

    /// Likewise `AttributeMapping`, completed by the mapping of the type's own subject
    /// claim.
    fn expected_attribute_mapping(&self) -> Value {
        super::identity_provider_type_defaults::resolved_attribute_mapping(
            self.stored.provider_type,
            &object(
                self.attribute_mapping
                    .as_ref()
                    .unwrap_or(&self.stored.attribute_mapping),
            ),
        )
    }

    /// Likewise `IdpIdentifiers`, whose order is the submitted one.
    fn expected_idp_identifiers(&self) -> Value {
        json!(self
            .idp_identifiers
            .as_ref()
            .unwrap_or(&self.stored.idp_identifiers))
    }

    /// Likewise `AcrMapping`, which stays absent on a provider that was created without
    /// one and that the update sets none for.
    fn expected_acr_mapping(&self) -> Option<Value> {
        self.acr_mapping
            .as_ref()
            .or(self.stored.acr_mapping.as_ref())
            .map(|mapping| object(mapping))
    }
}

/// The partial-update table. The rows differ along the member the update submits and
/// the one it leaves out, the provider's type (spanning
/// `smithy_query(query="shape",shape="IdentityProviderTypeType")`), and the size of
/// each submitted map and list — including the `IdpIdentifiers` maximum 50 and the
/// `AcrMapping` maximum 4 from
/// `smithy_query(query="operation",operation="UpdateIdentityProvider")`.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "an OIDC provider whose attribute mapping alone is submitted",
            stored: Stored {
                provider_name: "AuthExampleOIDC".to_string(),
                provider_type: "OIDC",
                details: oidc_details("auth.example.com"),
                attribute_mapping: pairs(&[("email", "email")]),
                idp_identifiers: list(&["auth.example.com", "AuthExample"]),
                acr_mapping: Some(pairs(&[("Level1", "urn:acr:1")])),
            },
            details: None,
            attribute_mapping: Some(pairs(&[
                ("email", "email"),
                ("given_name", "given_name"),
                ("username", "sub"),
            ])),
            idp_identifiers: None,
            acr_mapping: None,
        },
        UpdateCase {
            case: "an OIDC provider whose detail map is submitted whole, for another issuer",
            stored: Stored {
                provider_name: "MigratingOIDC".to_string(),
                provider_type: "OIDC",
                details: oidc_details("old.example.com"),
                attribute_mapping: pairs(&[("email", "email")]),
                idp_identifiers: list(&["old.example.com"]),
                acr_mapping: None,
            },
            details: Some(oidc_details("new.example.com")),
            attribute_mapping: None,
            idp_identifiers: None,
            acr_mapping: None,
        },
        UpdateCase {
            case: "a SAML provider whose identifier list is emptied",
            stored: Stored {
                provider_name: "CorpSAML".to_string(),
                provider_type: "SAML",
                details: pairs(&[
                    (
                        "MetadataFile",
                        "<EntityDescriptor entityID=\"https://saml.example.com\"/>",
                    ),
                    ("IDPSignout", "true"),
                ]),
                attribute_mapping: pairs(&[("email", "emailAddress")]),
                idp_identifiers: list(&["corp.example.com", "corp"]),
                acr_mapping: None,
            },
            details: None,
            attribute_mapping: None,
            idp_identifiers: Some(Vec::new()),
            acr_mapping: None,
        },
        UpdateCase {
            case: "a Google provider whose attribute mapping is emptied and detail map replaced",
            stored: Stored {
                provider_name: "Google".to_string(),
                provider_type: "Google",
                details: pairs(&[
                    ("client_id", "google-client-id"),
                    ("client_secret", "google-client-secret"),
                    ("authorize_scopes", "openid email profile"),
                ]),
                attribute_mapping: pairs(&[("email", "email"), ("username", "sub")]),
                idp_identifiers: list(&["google"]),
                acr_mapping: None,
            },
            details: Some(pairs(&[
                ("client_id", "rotated-google-client-id"),
                ("client_secret", "rotated-google-client-secret"),
                ("authorize_scopes", "openid email"),
            ])),
            attribute_mapping: Some(Vec::new()),
            idp_identifiers: None,
            acr_mapping: None,
        },
        UpdateCase {
            case: "a Facebook provider no member of which the update submits",
            stored: Stored {
                provider_name: "Facebook".to_string(),
                provider_type: "Facebook",
                details: pairs(&[
                    ("api_version", "v17.0"),
                    ("client_id", "1example23456789"),
                    ("client_secret", "provider-app-client-secret"),
                    ("authorize_scopes", "public_profile, email"),
                ]),
                attribute_mapping: pairs(&[("email", "email")]),
                idp_identifiers: list(&["facebook"]),
                acr_mapping: Some(Vec::new()),
            },
            details: None,
            attribute_mapping: None,
            idp_identifiers: None,
            acr_mapping: None,
        },
        UpdateCase {
            case: "a SignInWithApple provider whose ACR mapping is submitted at the maximum 4",
            stored: Stored {
                provider_name: "SignInWithApple".to_string(),
                provider_type: "SignInWithApple",
                details: pairs(&[
                    ("client_id", "com.example.service"),
                    ("team_id", "1EXAMPLE23"),
                    ("key_id", "2EXAMPLE34"),
                    ("private_key", ""),
                    ("authorize_scopes", "name email"),
                ]),
                attribute_mapping: Vec::new(),
                idp_identifiers: Vec::new(),
                acr_mapping: None,
            },
            details: None,
            attribute_mapping: None,
            idp_identifiers: None,
            acr_mapping: Some(pairs(&[
                ("Level1", "urn:acr:1"),
                ("Level2", "urn:acr:2"),
                ("Level3", "urn:acr:3"),
                ("Level4", "urn:acr:4"),
            ])),
        },
        UpdateCase {
            case: "a LoginWithAmazon provider whose identifiers are submitted at the maximum 50",
            stored: Stored {
                provider_name: "LoginWithAmazon".to_string(),
                provider_type: "LoginWithAmazon",
                details: pairs(&[
                    ("client_id", "amzn1.application-oa2-client.example"),
                    ("client_secret", "amazon-client-secret"),
                    ("authorize_scopes", "profile postal_code"),
                ]),
                attribute_mapping: pairs(&[("email", "email")]),
                idp_identifiers: list(&["amazon"]),
                acr_mapping: None,
            },
            details: None,
            attribute_mapping: None,
            idp_identifiers: Some((0..50).map(|index| format!("amazon-{index}")).collect()),
            acr_mapping: None,
        },
    ]
}

#[tokio::test]
async fn an_update_replaces_submitted_members_and_preserves_omitted_ones() {
    let router = router();
    let user_pool_id = pool(&router, "update-identity-provider-pool").await;

    for case in update_cases() {
        let label = case.case;
        let created = case.stored.create(&router, &user_pool_id).await;

        let updated = ok(
            &router,
            "UpdateIdentityProvider",
            &case.request(&user_pool_id),
        )
        .await;
        let provider = &updated["IdentityProvider"];

        assert_eq!(
            provider["ProviderDetails"],
            case.expected_details(),
            "case {label:?}: a submitted detail map replaces the stored one and an omitted one keeps it, got {provider}"
        );
        assert_eq!(
            provider["AttributeMapping"],
            case.expected_attribute_mapping(),
            "case {label:?}: a submitted attribute mapping replaces the stored one and an omitted one keeps it, got {provider}"
        );
        assert_eq!(
            provider["IdpIdentifiers"],
            case.expected_idp_identifiers(),
            "case {label:?}: a submitted identifier list replaces the stored one, in order, and an omitted one keeps it, got {provider}"
        );
        assert_eq!(
            provider.get("AcrMapping").cloned(),
            case.expected_acr_mapping(),
            "case {label:?}: a submitted ACR mapping replaces the stored one and an omitted one keeps it, got {provider}"
        );

        // The provider's identity and its creation instant are not members of this
        // input, so the update leaves them exactly as they were.
        for member in ["UserPoolId", "ProviderName", "ProviderType", "CreationDate"] {
            assert_eq!(
                provider[member], created[member],
                "case {label:?}: {member} is not a member of this input, so an update must leave it as created, got {provider}"
            );
        }

        // The provider was modified, so its last-modified instant is at or after the
        // instant it was created at.
        let created_at = created["CreationDate"].as_f64();
        let modified_at = provider["LastModifiedDate"].as_f64();
        assert!(
            modified_at.is_some_and(|modified| created_at.is_some_and(|created| modified >= created)),
            "case {label:?}: an updated provider reports a last-modified instant at or after its creation instant, got {provider}"
        );

        // The round trip: the reader renders the STORED provider, member for member.
        let read = ok(
            &router,
            "DescribeIdentityProvider",
            &json!({ "UserPoolId": user_pool_id, "ProviderName": case.stored.provider_name }),
        )
        .await;
        assert_eq!(
            read["IdentityProvider"], updated["IdentityProvider"],
            "case {label:?}: DescribeIdentityProvider must render what the update stored, got {read}"
        );
    }
}

/// One row of the OIDC endpoint-resolution table: the detail map the update submits,
/// and the message the refusal must carry — the issuer host a discovery read would have
/// to look up, or the keys the map has to state when it names no issuer at all.
struct ResolutionCase {
    case: &'static str,
    details: Vec<(String, String)>,
    expected_message: String,
}

/// The table. Its rows differ along WHICH endpoint the map leaves undiscovered — each of
/// the four in turn, two at once, and all of them — and along the form of the issuer the
/// endpoints would have to be discovered from: a bare origin, one with a path, one with
/// a port, one with nothing in it, and none at all.
fn resolution_cases() -> Vec<ResolutionCase> {
    let mut cases = Vec::new();

    // Each of the four endpoints in turn, with every other one stated: the map resolves
    // all but one, so the one it leaves out has to be discovered.
    for key in [
        "attributes_url",
        "authorize_url",
        "jwks_uri",
        "token_url",
    ] {
        cases.push(ResolutionCase {
            case: match key {
                "attributes_url" => "an OIDC map missing attributes_url",
                "authorize_url" => "an OIDC map missing authorize_url",
                "jwks_uri" => "an OIDC map missing jwks_uri",
                _ => "an OIDC map missing token_url",
            },
            details: oidc_details_without("idp.example.com", &[key]),
            expected_message: "idp.example.com".to_string(),
        });
    }

    cases.push(ResolutionCase {
        case: "an OIDC map missing two endpoints at once",
        details: oidc_details_without("two.example.com", &["jwks_uri", "token_url"]),
        expected_message: "two.example.com".to_string(),
    });
    cases.push(ResolutionCase {
        case: "an OIDC map stating nothing but its issuer, so every endpoint is discovered",
        details: pairs(&[("oidc_issuer", "https://discovery.example.com")]),
        expected_message: "discovery.example.com".to_string(),
    });
    cases.push(ResolutionCase {
        case: "an OIDC map whose issuer carries a path",
        details: pairs(&[("oidc_issuer", "https://tenant.example.com/oidc/v2")]),
        expected_message: "tenant.example.com".to_string(),
    });
    cases.push(ResolutionCase {
        case: "an OIDC map whose issuer carries a port, which a lookup of the host drops",
        details: pairs(&[("oidc_issuer", "https://ported.example.com:8443")]),
        expected_message: "ported.example.com".to_string(),
    });
    cases.push(ResolutionCase {
        case: "an OIDC map whose endpoint is present but empty, which states no endpoint",
        details: {
            let mut details = oidc_details_without("empty.example.com", &["token_url"]);
            details.push(("token_url".to_string(), String::new()));
            details
        },
        expected_message: "empty.example.com".to_string(),
    });

    // No issuer to discover from: the map has to state the endpoints itself, so the
    // refusal names the keys it is missing instead of a host.
    cases.push(ResolutionCase {
        case: "an OIDC map missing token_url that names no issuer",
        details: oidc_details_without("noissuer.example.com", &["token_url", "oidc_issuer"]),
        expected_message:
            "ProviderDetails must be provided with the following keys: [oidc_issuer, token_url]"
                .to_string(),
    });
    cases.push(ResolutionCase {
        case: "an OIDC map with no endpoint and no issuer at all",
        details: pairs(&[("client_id", "1example23456789")]),
        expected_message: "ProviderDetails must be provided with the following keys: \
             [attributes_url, authorize_url, jwks_uri, oidc_issuer, token_url]"
            .to_string(),
    });
    cases.push(ResolutionCase {
        case: "an OIDC map whose issuer is a bare host name, which is what a lookup takes",
        details: oidc_details_without("bare.example.com", &["token_url", "oidc_issuer"])
            .into_iter()
            .chain([("oidc_issuer".to_string(), "bare.example.com".to_string())])
            .collect(),
        expected_message: "bare.example.com".to_string(),
    });
    cases.push(ResolutionCase {
        case: "an OIDC map missing token_url whose issuer is present but empty",
        details: oidc_details_without("emptyissuer.example.com", &["token_url", "oidc_issuer"])
            .into_iter()
            .chain([("oidc_issuer".to_string(), String::new())])
            .collect(),
        expected_message:
            "ProviderDetails must be provided with the following keys: [oidc_issuer, token_url]"
                .to_string(),
    });

    cases
}

#[tokio::test]
async fn an_oidc_detail_map_whose_endpoints_cannot_be_resolved_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, "oidc-resolution-pool").await;

    let stored = Stored {
        provider_name: "AuthExampleOIDC".to_string(),
        provider_type: "OIDC",
        details: oidc_details("auth.example.com"),
        attribute_mapping: pairs(&[("email", "email")]),
        idp_identifiers: list(&["auth.example.com"]),
        acr_mapping: Some(pairs(&[("Level1", "urn:acr:1")])),
    };
    let created = stored.create(&router, &user_pool_id).await;

    for case in resolution_cases() {
        let label = case.case;
        let (status, payload) = call(
            &router,
            "UpdateIdentityProvider",
            &json!({
                "UserPoolId": user_pool_id,
                "ProviderName": stored.provider_name,
                "ProviderDetails": object(&case.details),
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {label:?}: a detail map an OIDC provider's endpoints cannot be resolved from is a 400 InvalidParameterException, got {payload}"
        );
        assert_eq!(
            error_message(&payload),
            case.expected_message,
            "case {label:?}: expected the refusal to report {:?}, got {payload}",
            case.expected_message
        );

        // A refused update wrote nothing: the stored provider is still the created one.
        let read = ok(
            &router,
            "DescribeIdentityProvider",
            &json!({ "UserPoolId": user_pool_id, "ProviderName": stored.provider_name }),
        )
        .await;
        assert_eq!(
            read["IdentityProvider"], created,
            "case {label:?}: a refused update must leave the stored provider exactly as it was, got {read}"
        );
    }
}

#[tokio::test]
async fn a_detail_map_that_resolves_every_endpoint_is_accepted() {
    let router = router();
    let user_pool_id = pool(&router, "oidc-resolved-pool").await;

    // An OIDC map stating all four endpoints needs nothing read from the IdP, whatever
    // its issuer, so the update stands. A provider type whose endpoints Amazon Cognito
    // does not discover from `oidc_issuer` — every other value of
    // `IdentityProviderTypeType` — is accepted with the detail keys its own type
    // documents, none of them an endpoint.
    for (label, provider_name, provider_type, details) in [
        (
            "an OIDC map stating all four endpoints",
            "AuthExampleOIDC",
            "OIDC",
            oidc_details("auth.example.com"),
        ),
        (
            "a SAML map carrying a metadata document and no endpoint",
            "CorpSAML",
            "SAML",
            pairs(&[("MetadataFile", "<EntityDescriptor/>"), ("IDPInit", "true")]),
        ),
        (
            "a Google map carrying only client credentials and scopes",
            "Google",
            "Google",
            pairs(&[
                ("client_id", "google-client-id"),
                ("client_secret", "google-client-secret"),
                ("authorize_scopes", "openid email profile"),
            ]),
        ),
        (
            "a SignInWithApple map carrying only its signing key material",
            "SignInWithApple",
            "SignInWithApple",
            pairs(&[
                ("client_id", "com.example.service"),
                ("team_id", "1EXAMPLE23"),
                ("key_id", "2EXAMPLE34"),
                ("authorize_scopes", "name email"),
            ]),
        ),
    ] {
        let stored = Stored {
            provider_name: provider_name.to_string(),
            provider_type,
            details: pairs(&[("client_id", "created-client-id")]),
            attribute_mapping: Vec::new(),
            idp_identifiers: Vec::new(),
            acr_mapping: None,
        };
        stored.create(&router, &user_pool_id).await;

        let updated = ok(
            &router,
            "UpdateIdentityProvider",
            &json!({
                "UserPoolId": user_pool_id,
                "ProviderName": provider_name,
                "ProviderDetails": object(&details),
            }),
        )
        .await;
        assert_eq!(
            updated["IdentityProvider"]["ProviderDetails"],
            super::identity_provider_type_defaults::resolved_details(
                provider_type,
                &object(&details)
            ),
            "case {label:?}: the submitted detail map must be what the provider now holds, got {updated}"
        );
    }
}

#[tokio::test]
async fn an_absent_pool_or_provider_name_is_a_400() {
    let router = router();
    let user_pool_id = pool(&router, "update-idp-notfound-pool").await;
    let stored = Stored {
        provider_name: "AuthExampleOIDC".to_string(),
        provider_type: "OIDC",
        details: oidc_details("auth.example.com"),
        attribute_mapping: Vec::new(),
        idp_identifiers: Vec::new(),
        acr_mapping: None,
    };
    stored.create(&router, &user_pool_id).await;

    // The provider rows span what the sentence's variable parts can be: a name of each
    // form `ProviderName` admits, over the pool the fixture created and over a pool name of
    // another Region's shape, so neither part is a value one row happens to carry.
    let mut cases: Vec<(String, Value, String)> = vec![(
        "an update of a provider in a pool that does not exist".to_string(),
        json!({
            "UserPoolId": "us-east-1_absent",
            "ProviderName": "AuthExampleOIDC",
            "AttributeMapping": { "email": "email" },
        }),
        "User pool us-east-1_absent does not exist.".to_string(),
    )];
    for absent in [
        "AbsentProvider",
        "SignInWithApple",
        "a",
        "Provider_with.every:char/",
    ] {
        cases.push((
            format!("an update of the provider name {absent:?}, which the pool does not hold"),
            json!({
                "UserPoolId": user_pool_id,
                "ProviderName": absent,
                "AttributeMapping": { "email": "email" },
            }),
            // The UPDATE path names the pool as the user pool it is.
            format!("Identity provider {absent} in User Pool {user_pool_id} does not exist."),
        ));
    }

    for (label, body, expected_message) in cases {
        let (status, payload) = call(&router, "UpdateIdentityProvider", &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ResourceNotFoundException"),
            "case {label:?}: must be a 400 ResourceNotFoundException, got {payload}"
        );
        assert_eq!(
            error_message(&payload),
            expected_message,
            "case {label:?}: expected the refusal to report {expected_message:?}, got {payload}"
        );
    }

    // The RESOLVE path words the same class its own way, so the two sentences cannot
    // collapse into one: the same absent provider read back names the pool as a tenant.
    for absent in ["AbsentProvider", "SignInWithApple"] {
        let (status, payload) = call(
            &router,
            "DescribeIdentityProvider",
            &json!({ "UserPoolId": user_pool_id, "ProviderName": absent }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ResourceNotFoundException"),
            "case a read of {absent:?}: must be a 400 ResourceNotFoundException, got {payload}"
        );
        let expected =
            format!("Identity provider {absent} for tenantId {user_pool_id} does not exist.");
        assert_eq!(
            error_message(&payload),
            expected,
            "case a read of {absent:?}: expected the resolve path's own sentence {expected:?}, \
             got {payload}"
        );
    }

    // Neither refusal is the scaffold's 501, and neither changed the stored provider.
    let read = ok(
        &router,
        "DescribeIdentityProvider",
        &json!({ "UserPoolId": user_pool_id, "ProviderName": stored.provider_name }),
    )
    .await;
    assert_eq!(
        read["IdentityProvider"]["ProviderDetails"],
        super::identity_provider_type_defaults::resolved_details(
            stored.provider_type,
            &object(&oidc_details("auth.example.com"))
        ),
        "a refused update of another pool or provider must leave this one as it was, got {read}"
    );
}
