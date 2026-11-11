// SPDX-License-Identifier: CC0-1.0

//! WHICH rule a refusal reports, over the wire (`POST /` with `X-Amz-Target:
//! AWSCognitoIdentityProviderService.<operation>`).
//!
//! A client branches on the error class and reads the message, so a refusal has
//! to report the rule the request actually broke rather than the next rule the
//! handler happened to reach first. The rules under test, each asserted over a
//! TABLE whose rows differ along what the rule generalises over:
//!
//! * a challenge response whose `Session` names no outstanding challenge is the
//!   `CodeMismatchException` both respond operations declare — "thrown if the
//!   provided code doesn't match what the server was expecting"
//!   (`smithy_query(query="operation",operation="AdminRespondToAuthChallenge")`)
//!   — in one sentence that reports the code and the session together and names
//!   neither value;
//! * a CREDENTIAL a request presents to be verified is checked against stored
//!   state rather than against its declared form, so a secret of any shape is
//!   answered by the grant's own refusal and never quoted back
//!   (`GetClientToken` "provides its client ID and secret");
//! * a limit definition whose class, attributes or attribute VALUES identify no
//!   limit this service publishes is a bad PARAMETER rather than a missing
//!   resource, in a sentence naming the class it was looked up under and the
//!   value that named nothing;
//! * `ConfirmDevice` registers the device's own credentials, so a request that
//!   supplies none is refused for THEM, before the device key it would store
//!   them against is resolved;
//! * a user pool is replicated only when it is encrypted under a
//!   customer-managed key — "you must use a multi-region key to enable
//!   multi-region replication for a user pool"
//!   (`smithy_query(query="doc",shape="KeyConfigurationType",member="KeyType")`)
//!   — and a Region a refused create stored no replica in names none;
//! * a `Credential` that is not a registration-response document at all is a
//!   bad parameter whatever the pool it would be registered against enables,
//!   while what the document SAYS is read once that pool is known to register
//!   passkeys — so the feature's own precondition is reported for a document
//!   that is one.
//!
//! The pools, app clients, users, secrets, access tokens and device keys are all
//! real: they come from `CreateUserPool`, `CreateUserPoolClient`,
//! `SetUserPoolMfaConfig`, `AdminCreateUser`, `AdminSetUserPassword` and
//! `InitiateAuth`. Each row names the case it asserts and every assertion
//! reports the actual value.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::state;

/// A request's SigV4 credential scope is what routes it to this service, and its
/// Region is the one a created pool lives in. The signature is never checked
/// here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// This service's regional endpoint host.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// The Region the calls above are signed for, so the Region every pool of this
/// suite is primary in.
const PRIMARY_REGION: &str = "us-east-1";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router.set_request_authentication(false);
    router
}

/// Call `operation` with `body` and return its status and JSON response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", ENDPOINT_HOST)
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

/// Call `operation` and assert it was refused with `code` and `message`.
async fn refused(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
    code: &str,
    message: &str,
) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        (status, error_code(&payload), error_message(&payload)),
        (400, code, message),
        "case {case:?}: expected a 400 {code} saying {message:?}, got {status} with {payload}"
    );
    payload
}

/// Create a pool whose request carries `extra` beside its name, and return its
/// minted `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str, extra: Value) -> String {
    let mut body = Map::new();
    body.insert("PoolName".to_string(), json!(name));
    if let Value::Object(members) = extra {
        for (member, value) in members {
            body.insert(member, value);
        }
    }
    let created = ok(router, "CreateUserPool", &Value::Object(body)).await;
    created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("a minted pool id, got {created}"))
        .to_string()
}

/// Create an app client of `user_pool_id` carrying `extra` beside its name, and
/// return its minted `ClientId`.
async fn client(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    name: &str,
    extra: Value,
) -> String {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("ClientName".to_string(), json!(name));
    if let Value::Object(members) = extra {
        for (member, value) in members {
            body.insert(member, value);
        }
    }
    let created = ok(router, "CreateUserPoolClient", &Value::Object(body)).await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .unwrap_or_else(|| panic!("a minted client id, got {created}"))
        .to_string()
}

/// Create `username` in `user_pool_id` with a permanent password, so a sign-in
/// of it is served with no challenge.
async fn confirmed_user(router: &MultiServiceRouter, user_pool_id: &str, username: &str) {
    ok(
        router,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    ok(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
}

/// Sign `username` in through `client_id` and return the whole
/// `AuthenticationResult`, which carries the access token and — on a pool that
/// remembers devices — the device key the sign-in minted.
async fn sign_in(router: &MultiServiceRouter, client_id: &str, username: &str) -> Value {
    let answered = ok(
        router,
        "InitiateAuth",
        &json!({
            "ClientId": client_id,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    answered["AuthenticationResult"].clone()
}

// ---------------------------------------------------------------------------
// A `Session` that names no outstanding challenge
// ---------------------------------------------------------------------------

/// The one sentence a challenge response answers a `Session` that names no
/// outstanding challenge with: it is no session of this request, which is the
/// same fact as a session started for another pool or app client.
const SESSION_NOT_FOR_THIS_REQUEST_MESSAGE: &str = "Invalid session for the user.";

#[tokio::test]
async fn every_session_that_names_no_outstanding_challenge_is_one_invalid_session() {
    let router = router();
    let user_pool_id = pool(&router, "session-pool", json!({})).await;
    let client_id = client(
        &router,
        &user_pool_id,
        "session-client",
        json!({ "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"] }),
    )
    .await;

    // The rows differ along the LENGTH and shape of the session presented,
    // across the modelled range of `SessionType` (length 20..4096,
    // `smithy_query(query="operation",operation="RespondToAuthChallenge")`):
    // its two bounds and a value between them, none of which this service ever
    // handed out.
    let sessions = [
        ("a session at the modelled minimum length", "a".repeat(20)),
        (
            "a session of a shape a token generator might produce",
            format!("AYABe{}", "Z".repeat(60)),
        ),
        ("a session at the modelled maximum length", "s".repeat(4096)),
    ];

    // Both operations that answer a challenge, each with the rest of its
    // request: the administrative one names the pool, the client-side one does
    // not.
    for (case, session) in &sessions {
        for operation in ["AdminRespondToAuthChallenge", "RespondToAuthChallenge"] {
            let case = format!("{operation} with {case}");
            let mut body = Map::new();
            if operation == "AdminRespondToAuthChallenge" {
                body.insert("UserPoolId".to_string(), json!(user_pool_id));
            }
            body.insert("ClientId".to_string(), json!(client_id));
            body.insert("ChallengeName".to_string(), json!("NEW_PASSWORD_REQUIRED"));
            body.insert("Session".to_string(), json!(session));
            body.insert(
                "ChallengeResponses".to_string(),
                json!({ "USERNAME": "nobody", "NEW_PASSWORD": GOOD_PASSWORD }),
            );
            let payload = refused(
                &router,
                &case,
                operation,
                &Value::Object(body),
                "NotAuthorizedException",
                SESSION_NOT_FOR_THIS_REQUEST_MESSAGE,
            )
            .await;
            // The sentence reports the pair, so it quotes neither half of it.
            assert!(
                !error_message(&payload).contains(session.as_str()),
                "case {case:?}: the refusal must not quote the session, got {payload}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// A presented app-client secret
// ---------------------------------------------------------------------------

/// The one sentence the machine-to-machine grant answers a pair it does not
/// authenticate with.
const INVALID_CLIENT_OR_SECRET: &str = "Invalid client or secret";

/// A `ClientSecretType`-shaped secret (pattern `^[\w+]+$`, length 24..64) the
/// app client below is created with.
const CLIENT_SECRET: &str = "m2msecret00000000000000001";

#[tokio::test]
async fn a_presented_client_secret_is_verified_rather_than_shape_checked() {
    let router = router();
    let user_pool_id = pool(&router, "secret-pool", json!({})).await;
    let client_id = client(
        &router,
        &user_pool_id,
        "secret-client",
        json!({
            "ClientSecret": CLIENT_SECRET,
            "ExplicitAuthFlows": ["ALLOW_CLIENT_TOKEN_AUTH"],
            "AllowedOAuthFlows": ["client_credentials"],
            "AllowedOAuthFlowsUserPoolClient": true,
        }),
    )
    .await;

    // The rows differ along WHICH declared rule the presented secret breaks —
    // its `@pattern`, each bound of its `@length`, and none of them — because
    // the grant verifies the value instead of judging its form.
    let secrets = [
        (
            "a secret outside the declared pattern",
            "garbage-secret-0000000000".to_string(),
        ),
        ("a secret under the declared minimum", "short1".to_string()),
        ("a secret over the declared maximum", "x".repeat(65)),
        ("the empty secret", String::new()),
        (
            "a well-formed secret no app client holds",
            "wrongsecret0000000000000000".to_string(),
        ),
    ];

    for (case, secret) in &secrets {
        let payload = refused(
            &router,
            case,
            "GetClientToken",
            &json!({ "ClientId": client_id, "Secret": secret }),
            "NotAuthorizedException",
            INVALID_CLIENT_OR_SECRET,
        )
        .await;
        // A credential a caller presented is never reflected: the refusal says
        // which pair was not accepted and nothing about what was sent.
        assert!(
            secret.is_empty() || !payload.to_string().contains(secret.as_str()),
            "case {case:?}: the refusal must not quote the presented secret, got {payload}"
        );
    }

    // The control: the stored secret IS authenticated, so the refusals above
    // come from verifying the value rather than from refusing the operation.
    let granted = ok(
        &router,
        "GetClientToken",
        &json!({ "ClientId": client_id, "Secret": CLIENT_SECRET }),
    )
    .await;
    assert!(
        granted["ClientAuthenticationResult"]["AccessToken"]
            .as_str()
            .is_some_and(|token| !token.is_empty()),
        "case \"the stored secret\": the grant must mint a token, got {granted}"
    );
}

// ---------------------------------------------------------------------------
// A limit definition that identifies no limit
// ---------------------------------------------------------------------------

/// What a refusal says for a category value the service publishes no limit of:
/// the class the definition looked it up under and the value itself.
fn unknown_category_message(category: &str) -> String {
    format!("Unknown API_CATEGORY category: {category}")
}

/// The `LimitDefinition` naming `category`'s request-rate limit.
fn limit_definition(category: &str) -> Value {
    json!({ "LimitClass": "API_CATEGORY", "Attributes": { "Category": category } })
}

#[tokio::test]
async fn a_definition_that_identifies_no_limit_is_a_bad_parameter() {
    let router = router();

    // The rows differ along HOW the value fails to name a limit: a plausible
    // category this service publishes none of, one differing from a published
    // name only in case or in a trailing space, the empty value, and the
    // attribute's own key sent as its value.
    let categories = [
        "UserDeletion",
        "userauthentication",
        "UserRead ",
        "",
        "Category",
    ];

    for category in categories {
        for operation in ["GetProvisionedLimit", "UpdateProvisionedLimit"] {
            let case = format!("{operation} for the category {category:?}");
            let mut body = Map::new();
            body.insert("LimitDefinition".to_string(), limit_definition(category));
            if operation == "UpdateProvisionedLimit" {
                body.insert("RequestedLimitValue".to_string(), json!(1_000));
            }
            refused(
                &router,
                &case,
                operation,
                &Value::Object(body),
                "InvalidParameterException",
                &unknown_category_message(category),
            )
            .await;
        }
    }

    // The control: a category this service DOES publish — one no recorded case
    // cited — is served, and the rate an update stores reads back, so the
    // refusals above are about the value rather than about the operation.
    // `UserRead` is one of this service's own catalogue of API categories
    // (`API_CATEGORIES` in `src/state.rs`).
    let read = ok(
        &router,
        "GetProvisionedLimit",
        &json!({ "LimitDefinition": limit_definition("UserRead") }),
    )
    .await;
    let free = read["Limit"]["FreeLimitValue"].as_i64();
    assert_eq!(
        read["Limit"]["ProvisionedLimitValue"].as_i64(),
        free,
        "case \"a published category\": an unadjusted limit reports its included rate, got {read}"
    );
    let provisioned = free.unwrap_or_default() + 80;
    ok(
        &router,
        "UpdateProvisionedLimit",
        &json!({
            "LimitDefinition": limit_definition("UserRead"),
            "RequestedLimitValue": provisioned,
        }),
    )
    .await;
    let read_back = ok(
        &router,
        "GetProvisionedLimit",
        &json!({ "LimitDefinition": limit_definition("UserRead") }),
    )
    .await;
    assert_eq!(
        read_back["Limit"]["ProvisionedLimitValue"].as_i64(),
        Some(provisioned),
        "case \"a published category\": the stored rate must read back, got {read_back}"
    );
}

// ---------------------------------------------------------------------------
// The device credentials a confirm registers
// ---------------------------------------------------------------------------

/// The one sentence `ConfirmDevice` answers a request that supplied no device
/// credentials with.
const NO_DEVICE_CREDENTIALS_MESSAGE: &str = "Invalid device credentials given, no credentials given";

/// The one sentence `ConfirmDevice` answers a `DeviceKey` it cannot confirm
/// with.
const INVALID_DEVICE_KEY_MESSAGE: &str = "Invalid device key given.";

/// A `DeviceKeyType`-shaped key (pattern `^[\w-]+_[0-9a-f-]+$`) no sign-in
/// minted.
const UNMINTED_DEVICE_KEY: &str = "us-east-1_00000000-0000-4000-8000-000000000000";

#[tokio::test]
async fn confirm_device_reports_the_credentials_it_was_given_none_of() {
    let router = router();
    let user_pool_id = pool(
        &router,
        "device-pool",
        json!({ "DeviceConfiguration": { "DeviceOnlyRememberedOnUserPrompt": false } }),
    )
    .await;
    let client_id = client(
        &router,
        &user_pool_id,
        "device-client",
        json!({ "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"] }),
    )
    .await;
    confirmed_user(&router, &user_pool_id, "alice").await;

    // The rows differ along HOW no credentials were supplied: the member left
    // out, the structure present but empty, and each of its two values present
    // but empty.
    let without_credentials = [
        ("no DeviceSecretVerifierConfig at all", None),
        ("an empty DeviceSecretVerifierConfig", Some(json!({}))),
        (
            "a verifier that is the empty string",
            Some(json!({ "PasswordVerifier": "" })),
        ),
        ("a salt that is the empty string", Some(json!({ "Salt": "" }))),
        (
            "both values present and empty",
            Some(json!({ "PasswordVerifier": "", "Salt": "" })),
        ),
    ];

    for (case, configuration) in &without_credentials {
        // The key the user's OWN sign-in minted, so the only thing wrong with
        // the request is the credentials it left out — and the key a sign-in
        // never minted, so the credentials are reported before the key either
        // way.
        let result = sign_in(&router, &client_id, "alice").await;
        let token = result["AccessToken"].as_str().expect("an access token");
        let minted = result["NewDeviceMetadata"]["DeviceKey"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a sign-in must mint a device key, got {result}"))
            .to_string();
        for (key_case, device_key) in [
            ("its own minted device key", minted.as_str()),
            ("a device key nothing minted", UNMINTED_DEVICE_KEY),
        ] {
            let case = format!("{case} and {key_case}");
            let mut body = Map::new();
            body.insert("AccessToken".to_string(), json!(token));
            body.insert("DeviceKey".to_string(), json!(device_key));
            if let Some(configuration) = configuration {
                body.insert(
                    "DeviceSecretVerifierConfig".to_string(),
                    configuration.clone(),
                );
            }
            refused(
                &router,
                &case,
                "ConfirmDevice",
                &Value::Object(body),
                "InvalidParameterException",
                NO_DEVICE_CREDENTIALS_MESSAGE,
            )
            .await;
        }
    }

    // With credentials supplied, the DEVICE KEY is what decides the call: a key
    // no sign-in of this user minted is refused for the key, and the key the
    // sign-in minted is confirmed and reads back. The rows differ along which
    // of the two SRP values the application generated.
    let with_credentials = [
        (
            "a verifier and its salt",
            json!({ "PasswordVerifier": "ZGV2aWNlLXZlcmlmaWVy", "Salt": "ZGV2aWNlLXNhbHQ" }),
        ),
        (
            "a verifier alone",
            json!({ "PasswordVerifier": "ZGV2aWNlLXZlcmlmaWVy" }),
        ),
        ("a salt alone", json!({ "Salt": "ZGV2aWNlLXNhbHQ" })),
    ];

    for (case, configuration) in &with_credentials {
        let result = sign_in(&router, &client_id, "alice").await;
        let token = result["AccessToken"].as_str().expect("an access token");
        let minted = result["NewDeviceMetadata"]["DeviceKey"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a sign-in must mint a device key, got {result}"))
            .to_string();
        refused(
            &router,
            &format!("{case} and a device key nothing minted"),
            "ConfirmDevice",
            &json!({
                "AccessToken": token,
                "DeviceKey": UNMINTED_DEVICE_KEY,
                "DeviceSecretVerifierConfig": configuration,
            }),
            "InvalidParameterException",
            INVALID_DEVICE_KEY_MESSAGE,
        )
        .await;
        ok(
            &router,
            "ConfirmDevice",
            &json!({
                "AccessToken": token,
                "DeviceKey": minted,
                "DeviceSecretVerifierConfig": configuration,
            }),
        )
        .await;
        // The round trip: the confirmed device is the one the reader reports.
        let read = ok(
            &router,
            "GetDevice",
            &json!({ "AccessToken": token, "DeviceKey": minted }),
        )
        .await;
        assert_eq!(
            read["Device"]["DeviceKey"].as_str(),
            Some(minted.as_str()),
            "case {case:?}: the reader must report the confirmed device, got {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// The key a replicated pool is encrypted under
// ---------------------------------------------------------------------------

/// The one sentence a pool that cannot be replicated for the key it is
/// encrypted under is refused with.
const CUSTOMER_MANAGED_KEY_REQUIRED: &str =
    "User pool needs a customer-managed KMS key for multi-region replication";

/// The one sentence a Region holding no replica of the pool is reported with.
const REPLICA_NOT_FOUND: &str = "Replica not found in the requested region";

/// A `KeyConfiguration.KmsKeyArn`-shaped ARN (`EncryptionKeyArnType`, len
/// 20..2048) standing for the multi-Region customer-managed key a replicated
/// pool is encrypted under.
const MULTI_REGION_KEY_ARN: &str = "arn:aws:kms:us-east-1:123456789012:key/multi-region-example";

/// The Region a replica of the pools below is asked for in: one of this
/// emulator's Region catalog that is not the pools' own.
const REPLICA_REGION: &str = "us-west-2";

#[tokio::test]
async fn a_pool_without_a_customer_managed_key_holds_no_replica_in_any_region() {
    let router = router();

    // The rows differ along the key the pool is encrypted under: none
    // configured, so the pool holds the service default, and the Amazon Web
    // Services owned key named outright.
    let without_a_customer_key = [
        ("a pool that configures no key at all", json!({})),
        (
            "a pool on the Amazon Web Services owned key",
            json!({ "KeyConfiguration": { "KeyType": "AWS_OWNED_KEY" } }),
        ),
    ];

    for (index, (case, extra)) in without_a_customer_key.iter().enumerate() {
        let user_pool_id = pool(&router, &format!("no-key-pool-{index}"), extra.clone()).await;
        refused(
            &router,
            case,
            "CreateUserPoolReplica",
            &json!({ "UserPoolId": user_pool_id, "RegionName": REPLICA_REGION }),
            "InvalidParameterException",
            CUSTOMER_MANAGED_KEY_REQUIRED,
        )
        .await;
        // The refused create stored nothing, so the pool reports only the
        // primary replica its own Region holds.
        let listed = ok(
            &router,
            "ListUserPoolReplicas",
            &json!({ "UserPoolId": user_pool_id }),
        )
        .await;
        let regions: Vec<String> = listed["UserPoolReplicas"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: the listing answers a list, got {listed}"))
            .iter()
            .filter_map(|replica| replica["RegionName"].as_str().map(str::to_string))
            .collect();
        assert_eq!(
            regions,
            vec![PRIMARY_REGION.to_string()],
            "case {case:?}: a refused create leaves the pool its primary replica alone, \
             got {regions:?}"
        );
        // And a Region holding no replica names none to the operations that
        // address one.
        refused(
            &router,
            case,
            "DeleteUserPoolReplica",
            &json!({ "UserPoolId": user_pool_id, "RegionName": REPLICA_REGION }),
            "ResourceNotFoundException",
            REPLICA_NOT_FOUND,
        )
        .await;
    }

    // The control: a pool whose own key is in use IS replicated, and the
    // replica reads back through the listing. The rows differ along how the
    // configuration says so: the key type named outright, and the ARN of the
    // key alone — "the ARN of the KMS key used for encryption. If not
    // specified, Amazon Web Services managed keys are used"
    // (`smithy_query(query="shape",shape="KeyConfigurationType")`), so naming
    // one is naming the key in use.
    for (index, (case, key_configuration)) in [
        (
            "a pool naming the customer-managed key type",
            json!({ "KeyType": "CUSTOMER_MANAGED_KEY", "KmsKeyArn": MULTI_REGION_KEY_ARN }),
        ),
        (
            "a pool naming only the key's ARN",
            json!({ "KmsKeyArn": MULTI_REGION_KEY_ARN }),
        ),
    ]
    .iter()
    .enumerate()
    {
        let user_pool_id = pool(
            &router,
            &format!("customer-key-pool-{index}"),
            json!({ "KeyConfiguration": key_configuration }),
        )
        .await;
        let created = ok(
            &router,
            "CreateUserPoolReplica",
            &json!({ "UserPoolId": user_pool_id, "RegionName": REPLICA_REGION }),
        )
        .await;
        assert_eq!(
            created["UserPoolReplica"]["RegionName"].as_str(),
            Some(REPLICA_REGION),
            "case {case:?}: the create answers the replica it stored, got {created}"
        );
        let listed = ok(
            &router,
            "ListUserPoolReplicas",
            &json!({ "UserPoolId": user_pool_id }),
        )
        .await;
        let regions: Vec<String> = listed["UserPoolReplicas"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: the listing answers a list, got {listed}"))
            .iter()
            .filter_map(|replica| replica["RegionName"].as_str().map(str::to_string))
            .collect();
        assert_eq!(
            regions,
            vec![PRIMARY_REGION.to_string(), REPLICA_REGION.to_string()],
            "case {case:?}: the replica must read back, got {regions:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A registration response this service cannot read
// ---------------------------------------------------------------------------

/// The one sentence a `Credential` this service cannot read as a registration
/// response is refused with.
const INVALID_CREDENTIAL_DATA: &str = "Credential data is not valid";

/// The one sentence a pool that has not enabled the passkey feature is reported
/// with.
const WEB_AUTHN_NOT_ENABLED: &str = "WebAuthn not enabled for this pool.";

/// The one sentence a challenge that names no answerable registration is
/// refused with.
const CHALLENGE_NOT_FOUND: &str = "The challenge does not exist or has expired";

/// The relying party the passkey pool below registers passkeys under.
const RELYING_PARTY: &str = "passkeys.example.com";

/// A `RegistrationResponseJSON` this service CAN read, answering `challenge`:
/// the response every malformed row below is a departure from (W3C Web
/// Authentication Level 2 §5.1 and §5.8.1).
fn readable_credential(challenge: &str) -> Value {
    json!({
        "id": "cmVmdXNhbC1ydWxlLWNyZWRlbnRpYWw",
        "rawId": "cmVmdXNhbC1ydWxlLWNyZWRlbnRpYWw",
        "type": "public-key",
        "authenticatorAttachment": "platform",
        "response": {
            "clientDataJSON": state::base64_url(
                json!({
                    "type": "webauthn.create",
                    "challenge": challenge,
                    "origin": format!("https://{RELYING_PARTY}"),
                })
                .to_string()
                .as_bytes(),
            ),
            "transports": ["internal"],
        },
    })
}

#[tokio::test]
async fn an_unreadable_registration_response_is_reported_before_the_pools_own_precondition() {
    let router = router();

    // Two pools: one that offers passkey sign-in and names a relying party, and
    // one that offers none at all — so the rows below span both sides of the
    // precondition the feature's own exception reports.
    let passkeys = pool(
        &router,
        "passkey-pool",
        json!({ "Policies": { "SignInPolicy": { "AllowedFirstAuthFactors": ["PASSWORD", "WEB_AUTHN"] } } }),
    )
    .await;
    ok(
        &router,
        "SetUserPoolMfaConfig",
        &json!({
            "UserPoolId": passkeys,
            "WebAuthnConfiguration": { "RelyingPartyId": RELYING_PARTY },
        }),
    )
    .await;
    let no_passkeys = pool(
        &router,
        "no-passkey-pool",
        json!({ "Policies": { "SignInPolicy": { "AllowedFirstAuthFactors": ["PASSWORD"] } } }),
    )
    .await;

    let mut tokens = Vec::new();
    for (pool_case, user_pool_id) in [
        ("a pool that offers passkeys", &passkeys),
        ("a pool that offers no passkeys", &no_passkeys),
    ] {
        let client_id = client(
            &router,
            user_pool_id,
            "passkey-client",
            json!({ "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"] }),
        )
        .await;
        confirmed_user(&router, user_pool_id, "alice").await;
        let result = sign_in(&router, &client_id, "alice").await;
        tokens.push((
            pool_case,
            result["AccessToken"]
                .as_str()
                .expect("an access token")
                .to_string(),
        ));
    }

    // A `RegistrationResponseJSON` is a JSON OBJECT, so a document of any other
    // kind carries no credential to register and is reported as the parameter it
    // is on EITHER pool — before the passkey feature is asked about at all. The
    // rows differ along the kind of document presented.
    let not_a_document: Vec<(&str, Value)> = vec![
        ("a credential that is a string", json!("public-key")),
        ("a credential that is a number", json!(1)),
        ("a credential that is a boolean", json!(true)),
        (
            "a credential that is a list of responses",
            json!([readable_credential("Y2hhbGxlbmdl")]),
        ),
    ];

    for (case, credential) in &not_a_document {
        for (pool_case, token) in &tokens {
            let case = format!("{case} on {pool_case}");
            refused(
                &router,
                &case,
                "CompleteWebAuthnRegistration",
                &json!({ "AccessToken": token, "Credential": credential }),
                "InvalidParameterException",
                INVALID_CREDENTIAL_DATA,
            )
            .await;
        }
    }

    // What the document SAYS is read once the pool is known to register passkeys
    // at all, so the same unreadable response is the parameter on the pool that
    // offers them and the feature's own precondition on the pool that does not.
    // The rows differ along WHAT about the document cannot be read: a missing
    // credential type, a missing credential id, a `clientDataJSON` that is not
    // the base64url of any JSON, and client data collected for the OTHER
    // ceremony.
    let unreadable: Vec<(&str, Value)> = vec![
        ("a credential reporting no type", {
            let mut credential = readable_credential("Y2hhbGxlbmdl");
            credential
                .as_object_mut()
                .expect("the response is an object")
                .remove("type");
            credential
        }),
        ("a credential carrying no id", {
            let mut credential = readable_credential("Y2hhbGxlbmdl");
            let members = credential
                .as_object_mut()
                .expect("the response is an object");
            members.remove("id");
            members.remove("rawId");
            credential
        }),
        ("client data that is not base64url JSON", {
            let mut credential = readable_credential("Y2hhbGxlbmdl");
            credential["response"]["clientDataJSON"] = json!("not base64url JSON");
            credential
        }),
        ("client data collected for the sign-in ceremony", {
            let mut credential = readable_credential("Y2hhbGxlbmdl");
            credential["response"]["clientDataJSON"] = json!(state::base64_url(
                json!({
                    "type": "webauthn.get",
                    "challenge": "Y2hhbGxlbmdl",
                    "origin": format!("https://{RELYING_PARTY}"),
                })
                .to_string()
                .as_bytes(),
            ));
            credential
        }),
    ];

    for (case, credential) in &unreadable {
        for (pool_case, token) in &tokens {
            let case = format!("{case} on {pool_case}");
            let (code, message) = if *pool_case == "a pool that offers passkeys" {
                ("InvalidParameterException", INVALID_CREDENTIAL_DATA)
            } else {
                ("WebAuthnNotEnabledException", WEB_AUTHN_NOT_ENABLED)
            };
            refused(
                &router,
                &case,
                "CompleteWebAuthnRegistration",
                &json!({ "AccessToken": token, "Credential": credential }),
                code,
                message,
            )
            .await;
        }
    }

    // A response the service CAN read is where the pool's own precondition is
    // reported: the pool that offers no passkeys reports the feature, and the
    // pool that offers them gets as far as the challenge the response answers.
    let readable = readable_credential("Y2hhbGxlbmdlLXRoYXQtd2FzLW5ldmVyLWhhbmRlZC1vdXQ");
    for (pool_case, token) in &tokens {
        let (code, message) = if *pool_case == "a pool that offers passkeys" {
            ("WebAuthnChallengeNotFoundException", CHALLENGE_NOT_FOUND)
        } else {
            ("WebAuthnNotEnabledException", WEB_AUTHN_NOT_ENABLED)
        };
        refused(
            &router,
            &format!("a readable response on {pool_case}"),
            "CompleteWebAuthnRegistration",
            &json!({ "AccessToken": token, "Credential": readable }),
            code,
            message,
        )
        .await;
    }
}
