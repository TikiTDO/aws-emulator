// SPDX-License-Identifier: CC0-1.0

//! What `GetSigningCertificate` answers for a user pool is a certificate the SAML
//! identity provider on the other side can actually use.
//!
//! The RULE under test: the operation "returns the signing certificate for SAML
//! 2.0 federation" of the pool it names, and the member it answers carries "the
//! x.509 certificate that signs SAML 2.0 authentication requests for your user
//! pool" over a certificate that is "valid for 10 years from the date of issue"
//! (`smithy_query(query="operation",operation="GetSigningCertificate")`). So the
//! member is the base64 of a DER X.509 certificate — the coding a SAML metadata
//! `X509Certificate` element carries — issued to the pool's own SAML
//! service-provider name, self-signed, usable for signing, and valid over ten
//! years from the pool's date of issue. It is issued WITH the pool and stored on
//! it, so repeated reads of one pool answer one certificate while two pools
//! answer different ones, and a `UserPoolId` no stored pool answers to is refused
//! rather than issued a certificate.
//!
//! This suite reads the member the way the operator who installs it at an
//! identity provider does — decode the base64, parse the DER, then read the
//! certificate's own fields — rather than matching on its text, so nothing but a
//! real certificate passes it.
//!
//! The rows differ along what the rule generalises over: the Region the pool
//! lives in (the request's credential scope), whether the pool federates with a
//! SAML identity provider at all, the pool's configuration and the length of its
//! name — plus repeated reads, because the certificate is issued once and stored.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;
use x509_cert::der::oid::AssociatedOid;
use x509_cert::der::{Decode, Encode};

use cognito_idp::state;
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router.set_request_authentication(false);
    router
}

/// Call `operation` in `region` with `body`, returning its status and JSON body.
///
/// The Region travels in the request's SigV4 credential scope, which is what
/// decides the Region a created pool lives in; the signature itself is never
/// checked here.
async fn call(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cognito-idp.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/cognito-idp/\
                 aws4_request, SignedHeaders=host;x-amz-date, Signature=0000"
            ),
        )
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
async fn ok(router: &MultiServiceRouter, region: &str, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, region, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} in {region} must succeed, got {status} with {payload}"
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
        .or_else(|| payload.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// One row: a pool to create, and whether it federates with a SAML identity
/// provider.
struct Case {
    case: &'static str,
    region: &'static str,
    pool_name: String,
    /// The members the create carries beside `PoolName`.
    configuration: Value,
    /// A SAML identity provider to create in the pool, when the row federates.
    saml_provider: Option<&'static str>,
}

/// The table. The rows span the Region the pool lives in, the pool's
/// configuration, the length of its `PoolName` (`PoolNameType`, len 1..128,
/// `smithy_query(query="operation",operation="CreateUserPool")`) and whether a
/// SAML identity provider is configured at all — the certificate belongs to the
/// POOL, so none of those may change what the rule requires of it.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "a default pool that federates with nothing",
            region: "us-east-1",
            pool_name: "signing-certificate-plain".to_string(),
            configuration: json!({}),
            saml_provider: None,
        },
        Case {
            case: "a pool with a SAML identity provider",
            region: "us-east-1",
            pool_name: "signing-certificate-saml".to_string(),
            configuration: json!({}),
            saml_provider: Some("CorpSAML"),
        },
        Case {
            case: "a configured pool with a SAML identity provider in another Region",
            region: "eu-west-1",
            pool_name: "signing-certificate-eu".to_string(),
            configuration: json!({
                "MfaConfiguration": "OPTIONAL",
                "AutoVerifiedAttributes": ["email"],
            }),
            saml_provider: Some("EuropeSAML"),
        },
        Case {
            case: "a pool whose name is 128 characters, the modelled maximum",
            region: "ap-southeast-2",
            pool_name: "s".repeat(128),
            saml_provider: None,
            configuration: json!({ "DeletionProtection": "ACTIVE" }),
        },
    ]
}

/// Create the row's pool and, when it federates, its SAML identity provider;
/// return the minted `UserPoolId` and the pool's reported creation date, in
/// epoch seconds.
async fn create_pool(router: &MultiServiceRouter, case: &Case) -> (String, f64) {
    let mut body = Map::new();
    body.insert("PoolName".to_string(), json!(case.pool_name));
    for (name, value) in case
        .configuration
        .as_object()
        .expect("the configuration is an object")
    {
        body.insert(name.clone(), value.clone());
    }
    let created = ok(
        router,
        case.region,
        "CreateUserPool",
        &Value::Object(body),
    )
    .await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {:?}: CreateUserPool must mint an id, got {created}", case.case))
        .to_string();
    let creation_date = created["UserPool"]["CreationDate"]
        .as_f64()
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: CreateUserPool must report a creation date, got {created}",
                case.case
            )
        });
    if let Some(provider_name) = case.saml_provider {
        ok(
            router,
            case.region,
            "CreateIdentityProvider",
            &json!({
                "UserPoolId": user_pool_id,
                "ProviderName": provider_name,
                "ProviderType": "SAML",
                "ProviderDetails": {
                    "MetadataFile": "<EntityDescriptor entityID=\"https://saml.example.com\"/>",
                },
            }),
        )
        .await;
    }
    (user_pool_id, creation_date)
}

/// Read the pool's signing certificate through the operation.
async fn signing_certificate(
    router: &MultiServiceRouter,
    region: &str,
    user_pool_id: &str,
) -> String {
    let answered = ok(
        router,
        region,
        "GetSigningCertificate",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await;
    answered["Certificate"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("GetSigningCertificate must answer a certificate, got {answered}")
        })
        .to_string()
}

/// The certificate extension of type `T`, decoded, or `None` when the
/// certificate carries none of that type.
fn extension<T>(certificate: &x509_cert::Certificate) -> Option<T>
where
    T: AssociatedOid + for<'der> Decode<'der>,
{
    certificate
        .tbs_certificate
        .extensions
        .as_ref()?
        .iter()
        .find(|extension| extension.extn_id == T::OID)
        .and_then(|extension| T::from_der(extension.extn_value.as_bytes()).ok())
}

/// Read `certificate` the way the operator installing it at an identity provider
/// does: the member is base64, with none of the armour or line breaks a PEM block
/// carries, and what it decodes to is a DER X.509 certificate.
fn parse(case: &str, certificate: &str) -> x509_cert::Certificate {
    assert!(
        !certificate.is_empty(),
        "case {case:?}: the certificate must not be empty"
    );
    assert!(
        !certificate.contains("BEGIN CERTIFICATE"),
        "case {case:?}: the member carries the base64 of the certificate rather than a PEM \
         block, got {certificate:?}"
    );
    assert!(
        !certificate.chars().any(char::is_whitespace),
        "case {case:?}: the base64 carries no whitespace, got {certificate:?}"
    );
    let der = aws_smithy_types::base64::decode(certificate).unwrap_or_else(|error| {
        panic!("case {case:?}: the member must be base64, got {certificate:?} ({error})")
    });
    x509_cert::Certificate::from_der(&der).unwrap_or_else(|error| {
        panic!("case {case:?}: the decoded member must be a DER X.509 certificate ({error})")
    })
}

/// Assert everything the rule requires of one pool's certificate, and return its
/// public key as the bytes two certificates are compared by.
fn assert_usable_signing_certificate(
    case: &str,
    user_pool_id: &str,
    creation_date: f64,
    certificate: &str,
) -> Vec<u8> {
    let parsed = parse(case, certificate);
    let tbs = &parsed.tbs_certificate;

    // The certificate is issued to the pool's own SAML service-provider name, so
    // the provider that holds it knows which pool it verifies.
    let expected_name = state::saml_service_provider_entity_id(user_pool_id);
    let subject = tbs.subject.to_string();
    assert!(
        subject.contains(&expected_name),
        "case {case:?}: expected the subject to name {expected_name:?}, got {subject:?}"
    );
    assert_eq!(
        subject,
        tbs.issuer.to_string(),
        "case {case:?}: a pool's signing certificate is self-signed, so its issuer is its own \
         subject, got issuer {:?}",
        tbs.issuer.to_string()
    );

    // An end-entity certificate that signs: it signs the pool's SAML messages and
    // certifies no other certificate.
    let key_usage: x509_cert::ext::pkix::KeyUsage = extension(&parsed).unwrap_or_else(|| {
        panic!(
            "case {case:?}: the certificate carries its key usages, got extensions {:?}",
            tbs.extensions.as_ref().map(|extensions| extensions
                .iter()
                .map(|extension| extension.extn_id.to_string())
                .collect::<Vec<_>>())
        )
    });
    assert!(
        key_usage.digital_signature(),
        "case {case:?}: a signing certificate signs, got {key_usage:?}"
    );
    let basic_constraints: Option<x509_cert::ext::pkix::BasicConstraints> = extension(&parsed);
    assert!(
        basic_constraints
            .as_ref()
            .is_none_or(|constraints| !constraints.ca),
        "case {case:?}: the certificate must not be a certificate authority, got \
         {basic_constraints:?}"
    );

    // Valid for ten years from the date of issue, which is the date the pool
    // reports being created on — to the whole second an X.509 validity period is
    // stated at, with a second of slack for the clock between the two reads.
    let bearing = |at: x509_cert::time::Time| at.to_unix_duration().as_secs() as f64;
    let not_before = bearing(tbs.validity.not_before);
    let not_after = bearing(tbs.validity.not_after);
    assert!(
        (not_before - creation_date).abs() <= 2.0,
        "case {case:?}: the certificate must be issued when the pool was created \
         ({creation_date}), got {not_before}"
    );
    let validity = state::SIGNING_CERTIFICATE_VALIDITY.as_secs() as f64;
    assert!(
        (not_after - not_before - validity).abs() <= 2.0,
        "case {case:?}: the certificate must be valid for {validity} seconds from its date of \
         issue, got {} ({not_before} to {not_after})",
        not_after - not_before
    );

    tbs.subject_public_key_info
        .to_der()
        .unwrap_or_else(|error| {
            panic!("case {case:?}: a parsed certificate re-encodes its public key ({error})")
        })
}

// ---------------------------------------------------------------------------
// RULE: every pool answers the base64 of a usable X.509 signing certificate,
// issued to that pool and valid for ten years from its date of issue.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_pool_answers_a_usable_signing_certificate() {
    let router = router();
    // Every certificate this suite reads, so no two pools are shown to answer the
    // same key material.
    let mut keys: BTreeSet<Vec<u8>> = BTreeSet::new();

    for case in cases() {
        let (user_pool_id, creation_date) = create_pool(&router, &case).await;
        let certificate = signing_certificate(&router, case.region, &user_pool_id).await;
        let key = assert_usable_signing_certificate(
            case.case,
            &user_pool_id,
            creation_date,
            &certificate,
        );
        assert!(
            keys.insert(key),
            "case {:?}: each pool is issued its own certificate, but this one repeats a key \
             another pool answered",
            case.case
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the certificate is issued with the pool and stored on it, so repeated
// reads answer the one certificate and reading it changes nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn repeated_reads_answer_the_one_issued_certificate() {
    let router = router();

    for case in cases() {
        let (user_pool_id, _) = create_pool(&router, &case).await;
        let first = signing_certificate(&router, case.region, &user_pool_id).await;
        // Read again, and once more after another pool of the same Region was
        // created, so neither polling this operation nor the service issuing
        // another pool's certificate moves this one.
        let second = signing_certificate(&router, case.region, &user_pool_id).await;
        let (other_pool, _) = create_pool(
            &router,
            &Case {
                case: "a pool created between two reads",
                region: case.region,
                pool_name: format!("neighbour-of-{}", case.region),
                configuration: json!({}),
                saml_provider: None,
            },
        )
        .await;
        let third = signing_certificate(&router, case.region, &user_pool_id).await;
        assert_eq!(
            first, second,
            "case {:?}: two reads of one pool must answer one certificate, got {first:?} then \
             {second:?}",
            case.case
        );
        assert_eq!(
            first, third,
            "case {:?}: a read after another pool was created must answer the same certificate, \
             got {first:?} then {third:?}",
            case.case
        );
        let neighbour = signing_certificate(&router, case.region, &other_pool).await;
        assert_ne!(
            first, neighbour,
            "case {:?}: another pool must answer its own certificate, got {neighbour:?}",
            case.case
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a `UserPoolId` no stored pool answers to is refused as an unresolved
// user pool, with the sentence this operation words for it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_absent_pool_is_refused_rather_than_issued_a_certificate() {
    let router = router();
    // One pool that DOES exist, so a refusal cannot be the store being empty.
    let live = cases().remove(0);
    let (live_pool, _) = create_pool(&router, &live).await;

    // Ids satisfying `UserPoolIdType`'s pattern `^[\w-]+_[0-9a-zA-Z]+$` that no
    // create minted, so the lookup is what fails rather than a constraint: two
    // Regions, a numeric suffix, an upper-case one and the shortest form the
    // pattern admits.
    let absent = [
        "us-east-1_absentpool",
        "eu-west-2_0000000000",
        "us-west-2_ZZZZZZZZZ",
        "a_0",
    ];
    for user_pool_id in absent {
        let (status, payload) = call(
            &router,
            live.region,
            "GetSigningCertificate",
            &json!({ "UserPoolId": user_pool_id }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ResourceNotFoundException"),
            "case {user_pool_id:?}: expected a 400 ResourceNotFoundException, got {status} with \
             {payload}"
        );
        let expected = format!("User pool {user_pool_id} does not exist.");
        let message = error_message(&payload);
        assert_eq!(
            message, expected,
            "case {user_pool_id:?}: expected {expected:?}, got {message:?}"
        );
    }

    // The stored pool still answers its certificate after every refusal.
    let certificate = signing_certificate(&router, live.region, &live_pool).await;
    parse("the stored pool after the refusals", &certificate);
}
