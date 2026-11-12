// SPDX-License-Identifier: CC0-1.0

//! The migrate-user user-pool Lambda trigger: what a pool whose `LambdaConfig` names a
//! `UserMigration` function does with a username it does not hold.
//!
//! The rule under test: that trigger "can create user profiles when users sign in or attempt
//! to reset their password with credentials that don't exist yet"
//! (`LambdaConfigType$UserMigration`), so a username such a pool does not hold is not an
//! unknown user yet — the operation INVOKES the configured function, with the
//! `triggerSource` of the stage that asked (`UserMigration_ForgotPassword` for a password
//! reset, `UserMigration_Authentication` for a sign-in) and the password that stage
//! presented, and creates the user from the `userAttributes` and `finalUserStatus` the
//! function answered with before deciding anything. A function that RETURNS NO PROFILE found
//! no such user, so the operation refuses the username with its own
//! `UserNotFoundException` and its own not-found sentence. A function that RAISES is the
//! trigger's own failure, and the refusal carries the exception that function raised: the
//! reset reports the username as a user it could not resolve
//! (`UserNotFoundException`), while a sign-in reports the trigger that failed
//! (`UserLambdaValidationException`).
//!
//! The suites here:
//!
//! 1. a table over what the rule generalises over — which stage asked, how the function
//!    answered, which status it asks for, which attributes it returns, the metadata the
//!    request assigned and the pool's Region — asserting the event the function was handed,
//!    the answer the operation gave and the user that reads back afterwards;
//! 2. the pool that configures NO migrate-user function, which invokes nothing;
//! 3. the round trip a migrated reset completes: the code the migration's reset issued is
//!    answered, and the user then signs in with the password that reset set.
//!
//! The function is a stand-in for AWS Lambda that serves the invocation this service issues
//! out of a legacy directory of its own, and records every event it was handed — so what the
//! assertions read is what Cognito really sent and really did with the answer.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_types::body::SdkBody;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, ErasedServiceDefinition, MultiServiceRouter, OperationContext,
    Service,
};

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// This service's SigV4 signing name, the 4th field of a credential scope.
const SIGNING_NAME: &str = "cognito-idp";

/// The local name of this service's Smithy shape, which is the `X-Amz-Target` prefix a
/// client writes.
const TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// The emulator's own endpoint, which is what an unsigned client is pointed at.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// The credential scope of a request addressed to AWS Lambda, which is what the trigger
/// invocation this service issues carries.
const LAMBDA_SCOPE: &str = "/lambda/aws4_request";

/// The password a legacy user signs in with, satisfying a default pool's password policy.
const LEGACY_PASSWORD: &str = "L3gacy!Passw0rd";

/// The password a migrated user's reset sets, distinct from the one above.
const RESET_PASSWORD: &str = "Res3t!Passw0rd";

/// The auth flows every app client below admits.
const SIGN_IN_FLOWS: [&str; 3] = [
    "ALLOW_USER_PASSWORD_AUTH",
    "ALLOW_ADMIN_USER_PASSWORD_AUTH",
    "ALLOW_REFRESH_TOKEN_AUTH",
];

/// The shared engine every router in one test mounts.
fn engine() -> Arc<engine::InMemoryEngine> {
    Arc::new(engine::InMemoryEngine::new())
}

/// One emulator with this service and the stand-in Lambda mounted over `engine`, and the
/// log of events the stand-in was handed.
fn router(
    engine: &Arc<engine::InMemoryEngine>,
    directory: HashMap<String, Answered>,
) -> (MultiServiceRouter, Arc<Mutex<Vec<Value>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(engine)));
    router.set_request_authentication(false);
    router.register_service(LegacyDirectoryLambda {
        inner: cognito_idp::EmulatedService::new(Arc::clone(engine)),
        invocation: MigrateUserFunction {
            directory,
            seen: Arc::clone(&seen),
        },
    });
    (router, seen)
}

/// Dispatch `operation` with `body`, signed for `region` when one is given and unsigned
/// otherwise, and return the status and JSON response body.
async fn dispatch(
    router: &MultiServiceRouter,
    region: Option<&str>,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let host = match region {
        Some(region) => format!("{SIGNING_NAME}.{region}.amazonaws.com"),
        None => EMULATOR_HOST.to_string(),
    };
    let mut request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"));
    if let Some(region) = region {
        request = request.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{region}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=0000"
            ),
        );
    }
    let response = router
        .dispatch(
            request
                .body(Bytes::from(body.to_string()))
                .expect("the request is well formed"),
        )
        .await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let payload = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, payload)
}

/// Call `operation` signed for `region`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, region: &str, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(router, Some(region), operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// The wire error code of an error body (`__type`), without its namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `message` member of a body.
fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A Lambda function ARN in `region`, inside `ArnType`'s pattern and length.
fn function_arn(region: &str, name: &str) -> String {
    format!("arn:aws:lambda:{region}:000000000000:function:{name}")
}

/// Create a pool in `region` with `extra` merged into its configuration, and return its
/// minted id.
async fn pool(router: &MultiServiceRouter, region: &str, pool_name: &str, extra: Value) -> String {
    let mut body = json!({ "PoolName": pool_name });
    if let Value::Object(members) = extra {
        for (member, value) in members {
            body[member] = value;
        }
    }
    let created = ok(router, region, "CreateUserPool", &body).await;
    created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPool must mint an id, got {created}"))
        .to_string()
}

/// Create an app client of `user_pool_id` admitting [`SIGN_IN_FLOWS`], and return its
/// minted id.
async fn app_client(router: &MultiServiceRouter, region: &str, user_pool_id: &str) -> String {
    let created = ok(
        router,
        region,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "app",
            "ExplicitAuthFlows": SIGN_IN_FLOWS,
        }),
    )
    .await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPoolClient must mint a client id, got {created}"))
        .to_string()
}

/// A stand-in for AWS Lambda that serves the migrate-user invocation this service issues.
struct LegacyDirectoryLambda {
    /// The model-derived half of a `Service` is not an author seam, so this stand-in
    /// borrows this service's own. It is never used for dispatch: every request this
    /// stand-in claims is served by the invocation below.
    inner: cognito_idp::EmulatedService<engine::InMemoryEngine>,
    invocation: MigrateUserFunction,
}

impl Service for LegacyDirectoryLambda {
    fn definition(&self) -> &dyn ErasedServiceDefinition {
        self.inner.definition()
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        parts
            .headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains(LAMBDA_SCOPE))
    }

    fn override_operation(&self, _req: &RtRequest) -> Option<&dyn ErasedOperation> {
        Some(&self.invocation)
    }
}

/// How the stand-in function answers one username: the three things a migrate-user function
/// can do with the event it is handed.
#[derive(Clone, Debug)]
enum Answered {
    /// It RETURNS the profile to migrate the user from.
    Profile {
        attributes: Value,
        final_status: Option<String>,
    },
    /// It RETURNS, having populated no profile at all: it found no such user.
    NoProfile,
    /// It RAISES, so Lambda reports the invocation as a function error carrying the
    /// exception the function raised.
    Raised,
}

/// The migrate-user function the stand-in runs: it answers the event it was handed the way
/// its legacy directory says this username is answered.
struct MigrateUserFunction {
    /// The legacy directory: a username and how the function answers for it.
    directory: HashMap<String, Answered>,
    /// Every event this function was handed, in the order it was handed them.
    seen: Arc<Mutex<Vec<Value>>>,
}

/// The exception a declining stand-in function raises for `username`.
fn raised_for(username: &str) -> String {
    format!("{username} is not in the legacy directory")
}

impl ErasedOperation for MigrateUserFunction {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        let event: Value = req
            .body()
            .bytes()
            .and_then(|bytes| serde_json::from_slice(bytes).ok())
            .unwrap_or(Value::Null);
        Box::pin(async move {
            self.seen
                .lock()
                .expect("the event log is not poisoned")
                .push(event.clone());
            let username = event
                .get("userName")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let returns = |answered: Value| {
                http::Response::builder()
                    .status(200)
                    .body(SdkBody::from(answered.to_string()))
            };
            let answer = match self.directory.get(&username) {
                Some(Answered::Profile {
                    attributes,
                    final_status,
                }) => {
                    let mut answered = event.clone();
                    answered["response"]["userAttributes"] = attributes.clone();
                    if let Some(final_status) = final_status {
                        answered["response"]["finalUserStatus"] = json!(final_status);
                    }
                    returns(answered)
                }
                Some(Answered::NoProfile) => returns(event.clone()),
                // Lambda signals a function that raised with this header and the error
                // document as the payload.
                Some(Answered::Raised) | None => http::Response::builder()
                    .status(200)
                    .header("x-amz-function-error", "Unhandled")
                    .body(SdkBody::from(
                        json!({
                            "errorType": "UserNotFoundError",
                            "errorMessage": raised_for(&username),
                        })
                        .to_string(),
                    )),
            };
            RtResponse::try_from(answer.expect("the stand-in answer is well formed"))
                .expect("the stand-in answer is a runtime response")
        })
    }

    /// Served unsigned: the stand-in answers whatever reached it, so no auth scheme of the
    /// borrowed definition is resolved for it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        Some(&[])
    }
}

/// Which stage asks a pool to migrate a username it does not hold.
#[derive(Clone, Copy, Debug)]
enum Stage {
    /// A self-service password reset, by `ForgotPassword`.
    ForgotPassword,
    /// A password sign-in, by `InitiateAuth` with `USER_PASSWORD_AUTH`.
    SignIn,
    /// An administrative password sign-in, by `AdminInitiateAuth`.
    AdminSignIn,
}

impl Stage {
    /// The `triggerSource` an event of this stage must carry, as Amazon Cognito documents
    /// the migrate-user trigger's sources.
    fn trigger_source(self) -> &'static str {
        match self {
            Self::ForgotPassword => "UserMigration_ForgotPassword",
            Self::SignIn | Self::AdminSignIn => "UserMigration_Authentication",
        }
    }

    /// Whether this stage presents a password, which is the member the event carries it in:
    /// "Amazon Cognito doesn't send this value in a request for the
    /// `UserMigration_ForgotPassword` trigger source".
    fn presents_a_password(self) -> bool {
        matches!(self, Self::SignIn | Self::AdminSignIn)
    }

    /// How this stage reports a username nothing migrated: the not-found sentence the
    /// operation answers for a user that does not exist.
    fn not_found_message(self) -> &'static str {
        match self {
            // `ForgotPassword` resolves the user from the `Username` and the `ClientId`
            // together, since it models no `UserPoolId`.
            Self::ForgotPassword => "Username/client id combination not found.",
            Self::SignIn | Self::AdminSignIn => "User does not exist.",
        }
    }

    /// The operation this stage calls, and its request body.
    fn request(self, user_pool_id: &str, client_id: &str, username: &str, metadata: Value) -> (&'static str, Value) {
        match self {
            Self::ForgotPassword => (
                "ForgotPassword",
                json!({
                    "ClientId": client_id,
                    "Username": username,
                    "ClientMetadata": metadata,
                }),
            ),
            Self::SignIn => (
                "InitiateAuth",
                json!({
                    "ClientId": client_id,
                    "AuthFlow": "USER_PASSWORD_AUTH",
                    "AuthParameters": { "USERNAME": username, "PASSWORD": LEGACY_PASSWORD },
                    "ClientMetadata": metadata,
                }),
            ),
            Self::AdminSignIn => (
                "AdminInitiateAuth",
                json!({
                    "UserPoolId": user_pool_id,
                    "ClientId": client_id,
                    "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                    "AuthParameters": { "USERNAME": username, "PASSWORD": LEGACY_PASSWORD },
                    "ClientMetadata": metadata,
                }),
            ),
        }
    }
}

/// How the row's function answers for the username the row submits.
#[derive(Clone, Copy, Debug)]
enum Answers {
    /// It returns the row's attributes and status.
    Profile,
    /// It returns, with no profile at all.
    NoProfile,
    /// It raises.
    Raised,
}

/// One row of the migration table.
struct Row {
    case: &'static str,
    region: &'static str,
    stage: Stage,
    /// How the migrate-user function answers for this row's username.
    answers: Answers,
    /// The `userAttributes` the function answers with, for a row whose function returns a
    /// profile.
    attributes: &'static [(&'static str, &'static str)],
    /// The `finalUserStatus` it asks for, or none at all.
    final_status: Option<&'static str>,
    /// The `ClientMetadata` the request assigns.
    metadata: &'static [(&'static str, &'static str)],
    /// The HTTP status and error code the operation answers with; an empty code is a
    /// success.
    expected: (u16, &'static str),
    /// The `UserStatus` the migrated user must read back as, or `None` where no user may
    /// exist afterwards.
    expected_status: Option<&'static str>,
}

/// A `ClientMetadata`/`userAttributes` pair list as a JSON object.
fn members(pairs: &[(&str, &str)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), json!(value)))
            .collect(),
    )
}

#[tokio::test]
async fn an_absent_username_is_resolved_through_the_pools_migrate_user_function() {
    let rows: [Row; 11] = [
        Row {
            case: "a reset of a legacy user the function confirms",
            region: "us-east-1",
            stage: Stage::ForgotPassword,
            answers: Answers::Profile,
            attributes: &[("email", "legacy@example.test"), ("email_verified", "true")],
            final_status: Some("CONFIRMED"),
            metadata: &[],
            expected: (200, ""),
            expected_status: Some("CONFIRMED"),
        },
        Row {
            case: "a reset of a legacy user the function names no status for",
            region: "eu-west-1",
            stage: Stage::ForgotPassword,
            answers: Answers::Profile,
            attributes: &[("email", "legacy@example.test"), ("email_verified", "true")],
            final_status: None,
            metadata: &[("source", "migration")],
            expected: (200, ""),
            expected_status: Some("RESET_REQUIRED"),
        },
        Row {
            case: "a reset whose function returns no profile",
            region: "us-east-1",
            stage: Stage::ForgotPassword,
            answers: Answers::NoProfile,
            attributes: &[],
            final_status: None,
            metadata: &[],
            expected: (400, "UserNotFoundException"),
            expected_status: None,
        },
        Row {
            case: "a reset whose function raises",
            region: "us-east-1",
            stage: Stage::ForgotPassword,
            answers: Answers::Raised,
            attributes: &[],
            final_status: None,
            metadata: &[],
            expected: (400, "UserNotFoundException"),
            expected_status: None,
        },
        Row {
            case: "a sign-in of a legacy user the function confirms",
            region: "us-east-1",
            stage: Stage::SignIn,
            answers: Answers::Profile,
            attributes: &[("email", "legacy@example.test"), ("email_verified", "true")],
            final_status: Some("CONFIRMED"),
            metadata: &[("tenant", "legacy-directory")],
            expected: (200, ""),
            expected_status: Some("CONFIRMED"),
        },
        Row {
            case: "a sign-in of a legacy user the function wants reset first",
            region: "ap-southeast-2",
            stage: Stage::SignIn,
            answers: Answers::Profile,
            attributes: &[("email", "legacy@example.test")],
            final_status: Some("RESET_REQUIRED"),
            metadata: &[],
            expected: (400, "PasswordResetRequiredException"),
            expected_status: Some("RESET_REQUIRED"),
        },
        Row {
            case: "a sign-in whose function returns no profile",
            region: "us-east-1",
            stage: Stage::SignIn,
            answers: Answers::NoProfile,
            attributes: &[],
            final_status: None,
            metadata: &[],
            expected: (400, "UserNotFoundException"),
            expected_status: None,
        },
        Row {
            case: "a sign-in whose function raises",
            region: "eu-west-1",
            stage: Stage::SignIn,
            answers: Answers::Raised,
            attributes: &[],
            final_status: None,
            metadata: &[("tenant", "legacy-directory")],
            expected: (400, "UserLambdaValidationException"),
            expected_status: None,
        },
        Row {
            case: "an administrative sign-in of a legacy user the function confirms",
            region: "us-west-2",
            stage: Stage::AdminSignIn,
            answers: Answers::Profile,
            attributes: &[
                ("email", "legacy@example.test"),
                ("email_verified", "true"),
                ("given_name", "Ada"),
                ("family_name", "Lovelace"),
            ],
            final_status: Some("CONFIRMED"),
            metadata: &[("source", "console")],
            expected: (200, ""),
            expected_status: Some("CONFIRMED"),
        },
        Row {
            case: "an administrative sign-in whose function returns one attribute only",
            region: "us-east-1",
            stage: Stage::AdminSignIn,
            answers: Answers::Profile,
            attributes: &[("email", "legacy@example.test")],
            final_status: Some("CONFIRMED"),
            metadata: &[],
            expected: (200, ""),
            expected_status: Some("CONFIRMED"),
        },
        Row {
            case: "an administrative sign-in whose function raises",
            region: "us-east-1",
            stage: Stage::AdminSignIn,
            answers: Answers::Raised,
            attributes: &[],
            final_status: None,
            metadata: &[],
            expected: (400, "UserLambdaValidationException"),
            expected_status: None,
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let Row {
            case,
            region,
            stage,
            answers,
            attributes,
            final_status,
            metadata,
            expected,
            expected_status,
        } = row;
        // One username per row, answered the way this row's function answers.
        let username = format!("legacy{index}");
        let answered = match answers {
            Answers::Profile => Answered::Profile {
                attributes: members(attributes),
                final_status: final_status.map(ToString::to_string),
            },
            Answers::NoProfile => Answered::NoProfile,
            Answers::Raised => Answered::Raised,
        };
        let directory = HashMap::from([(username.clone(), answered)]);
        let engine = engine();
        let (router, seen) = router(&engine, directory);
        let trigger = function_arn(region, "migrate-user");
        let user_pool_id = pool(
            &router,
            region,
            &format!("migrating{index}"),
            json!({ "LambdaConfig": { "UserMigration": trigger } }),
        )
        .await;
        let client_id = app_client(&router, region, &user_pool_id).await;

        let (operation, body) = stage.request(
            &user_pool_id,
            &client_id,
            &username,
            members(metadata),
        );
        let (status, payload) = dispatch(&router, None, operation, &body).await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            expected,
            "case {case:?}: expected HTTP {} {:?} from {operation}, got {status} with {payload}",
            expected.0,
            expected.1
        );
        // What the refusal SAYS is what tells the caller which of the two happened: a
        // function that returned no profile found no such user, so the operation's own
        // not-found sentence is the answer, while a function that RAISED is the trigger's
        // own failure and carries the exception that function raised.
        let expected_message = match answers {
            Answers::Profile => None,
            Answers::NoProfile => Some(stage.not_found_message().to_string()),
            Answers::Raised => Some(format!(
                "UserMigration failed with error {}.",
                raised_for(&username)
            )),
        };
        if let Some(expected_message) = expected_message {
            assert_eq!(
                message(&payload),
                expected_message,
                "case {case:?}: expected {expected_message:?}, got {payload}"
            );
        }

        // The function was invoked, exactly once, with the event of the stage that asked.
        let events = seen.lock().expect("the event log is not poisoned").clone();
        assert_eq!(
            events.len(),
            1,
            "case {case:?}: expected one invocation of the migrate-user function, got \
             {events:?}"
        );
        let event = &events[0];
        assert_eq!(
            event["triggerSource"].as_str().unwrap_or_default(),
            stage.trigger_source(),
            "case {case:?}: expected the stage's own trigger source, got {event}"
        );
        assert_eq!(
            event["userName"].as_str().unwrap_or_default(),
            username,
            "case {case:?}: expected the event to name the submitted username, got {event}"
        );
        assert_eq!(
            event["userPoolId"].as_str().unwrap_or_default(),
            user_pool_id,
            "case {case:?}: expected the event to name the pool the trigger is configured \
             on, got {event}"
        );
        assert_eq!(
            event["region"].as_str().unwrap_or_default(),
            region,
            "case {case:?}: expected the event to name the pool's Region, got {event}"
        );
        assert_eq!(
            event["version"].as_str().unwrap_or_default(),
            "1",
            "case {case:?}: expected the trigger's one event version, got {event}"
        );
        let presented = event["request"].get("password").and_then(Value::as_str);
        assert_eq!(
            presented,
            stage.presents_a_password().then_some(LEGACY_PASSWORD),
            "case {case:?}: expected the event to carry the presented password only for a \
             sign-in, got {event}"
        );
        let expected_metadata = (!metadata.is_empty()).then(|| members(metadata));
        assert_eq!(
            event["request"].get("clientMetadata").cloned(),
            expected_metadata,
            "case {case:?}: expected the metadata the request assigned, got {event}"
        );
        assert_eq!(
            event["request"].get("validationData").cloned(),
            expected_metadata,
            "case {case:?}: expected the request's metadata as validation data, got {event}"
        );
        // The event names no stored user, so it carries none of a stored user's attributes.
        assert!(
            event["request"].get("userAttributes").is_none(),
            "case {case:?}: a user that does not exist yet has no attributes to report, got \
             {event}"
        );

        // What the pool holds afterwards: the migrated user, read back through
        // `AdminGetUser`, or no user at all for a username the function declined.
        let (read_status, read_back) = dispatch(
            &router,
            Some(region),
            "AdminGetUser",
            &json!({ "UserPoolId": user_pool_id, "Username": username }),
        )
        .await;
        match expected_status {
            Some(expected_status) => {
                assert_eq!(
                    read_status, 200,
                    "case {case:?}: the migrated user must read back, got {read_status} with \
                     {read_back}"
                );
                assert_eq!(
                    read_back["UserStatus"].as_str().unwrap_or_default(),
                    expected_status,
                    "case {case:?}: expected the status the function asked for, got \
                     {read_back}"
                );
                assert_eq!(
                    read_back["Username"].as_str().unwrap_or_default(),
                    username,
                    "case {case:?}: expected the migrated user to answer to the submitted \
                     username, got {read_back}"
                );
                let read_attributes: HashMap<String, String> = read_back["UserAttributes"]
                    .as_array()
                    .map(|list| {
                        list.iter()
                            .map(|attribute| {
                                (
                                    attribute["Name"].as_str().unwrap_or_default().to_string(),
                                    attribute["Value"].as_str().unwrap_or_default().to_string(),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                for (name, value) in attributes {
                    assert_eq!(
                        read_attributes.get(*name).map(String::as_str),
                        Some(*value),
                        "case {case:?}: expected the attribute the function returned, got \
                         {read_back}"
                    );
                }
                // `sub` is the pool's to mint, so the migrated user holds one and it is not
                // any value the function returned.
                let sub = read_attributes.get("sub").cloned().unwrap_or_default();
                assert!(
                    !sub.is_empty() && !attributes.iter().any(|(_, value)| *value == sub),
                    "case {case:?}: expected a sub the pool minted, got {read_back}"
                );
            }
            None => {
                assert_eq!(
                    (read_status, error_code(&read_back).as_str()),
                    (400, "UserNotFoundException"),
                    "case {case:?}: a declined username names no user, got {read_status} with \
                     {read_back}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_pool_that_configures_no_migrate_user_function_invokes_nothing() {
    let engine = engine();
    // The directory HOLDS this username: only the missing configuration can keep it from
    // being migrated.
    let mut directory = HashMap::new();
    directory.insert(
        "legacy".to_string(),
        Answered::Profile {
            attributes: members(&[("email", "legacy@example.test"), ("email_verified", "true")]),
            final_status: Some("CONFIRMED".to_string()),
        },
    );
    let (router, seen) = router(&engine, directory);
    let region = "us-east-1";
    let user_pool_id = pool(&router, region, "unconfigured", json!({})).await;
    let client_id = app_client(&router, region, &user_pool_id).await;

    // (case, the operation, its body, the expected error code and message)
    let calls = [
        (
            "ForgotPassword",
            json!({ "ClientId": client_id, "Username": "legacy" }),
            "Username/client id combination not found.",
        ),
        (
            "InitiateAuth",
            json!({
                "ClientId": client_id,
                "AuthFlow": "USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": "legacy", "PASSWORD": LEGACY_PASSWORD },
            }),
            "User does not exist.",
        ),
        (
            "AdminInitiateAuth",
            json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": "legacy", "PASSWORD": LEGACY_PASSWORD },
            }),
            "User does not exist.",
        ),
    ];
    for (operation, body, expected_message) in calls {
        let (status, payload) = dispatch(&router, None, operation, &body).await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (400, "UserNotFoundException"),
            "case {operation:?}: a pool configuring no migrate-user function refuses the \
             username, got {status} with {payload}"
        );
        assert_eq!(
            message(&payload),
            expected_message,
            "case {operation:?}: expected the operation's own not-found sentence, got {payload}"
        );
    }
    let events = seen.lock().expect("the event log is not poisoned").clone();
    assert!(
        events.is_empty(),
        "a pool that configures no migrate-user function invokes nothing, got {events:?}"
    );
}

#[tokio::test]
async fn a_migrated_reset_can_be_confirmed_and_signed_in_with() {
    let engine = engine();
    let mut directory = HashMap::new();
    directory.insert(
        "legacy".to_string(),
        Answered::Profile {
            attributes: members(&[("email", "legacy@example.test"), ("email_verified", "true")]),
            // No status at all, so the migrated user owes a password: the reset it is
            // migrated by is what sets one.
            final_status: None,
        },
    );
    let (router, _seen) = router(&engine, directory);
    let region = "us-east-1";
    let trigger = function_arn(region, "migrate-user");
    let user_pool_id = pool(
        &router,
        region,
        "round-trip",
        json!({ "LambdaConfig": { "UserMigration": trigger } }),
    )
    .await;
    let client_id = app_client(&router, region, &user_pool_id).await;

    let started = ok(
        &router,
        region,
        "ForgotPassword",
        &json!({ "ClientId": client_id, "Username": "legacy" }),
    )
    .await;
    assert_eq!(
        started["CodeDeliveryDetails"]["Destination"]
            .as_str()
            .unwrap_or_default(),
        "l***@e***",
        "the reset reports the destination the migrated profile's verified email masks to, \
         got {started}"
    );

    // The code the reset issued, read out of this service's own state: the real service
    // delivers it out of band, so no response carries it.
    let code = {
        let service_state =
            cognito_idp::state::service_state(&*engine).expect("this service's own state");
        let guard = service_state.lock();
        guard
            .pool(&user_pool_id)
            .and_then(|pool| pool.user("legacy"))
            .and_then(|user| {
                user.outstanding_code(&cognito_idp::state::CodePurpose::PasswordReset)
            })
            .map(|code| code.code.clone())
            .expect("the migrated user holds the reset code the migration's reset issued")
    };
    ok(
        &router,
        region,
        "ConfirmForgotPassword",
        &json!({
            "ClientId": client_id,
            "Username": "legacy",
            "ConfirmationCode": code,
            "Password": RESET_PASSWORD,
        }),
    )
    .await;

    // The migrated user now holds the password that reset set, and signing in with it mints
    // tokens — which is the whole point of migrating it.
    let signed_in = ok(
        &router,
        region,
        "InitiateAuth",
        &json!({
            "ClientId": client_id,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "legacy", "PASSWORD": RESET_PASSWORD },
        }),
    )
    .await;
    assert!(
        signed_in["AuthenticationResult"]["AccessToken"]
            .as_str()
            .is_some_and(|token| !token.is_empty()),
        "the migrated user signs in with the password its reset set, got {signed_in}"
    );
    let read_back = ok(
        &router,
        region,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": "legacy" }),
    )
    .await;
    assert_eq!(
        read_back["UserStatus"].as_str().unwrap_or_default(),
        "CONFIRMED",
        "a migrated user that answered its reset owes no further one, got {read_back}"
    );
}
