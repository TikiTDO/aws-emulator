// SPDX-License-Identifier: CC0-1.0

//! How an ERROR of this service reaches the caller, over the wire (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: a refusal of the CALLER is answered with HTTP 400 and names its
//! error shape in the `x-amzn-errortype` response header, as the shape's simple name
//! followed by a colon — whatever `@httpError` the model annotates that shape with. The
//! model declares 404 for `ResourceNotFoundException` and `UserNotFoundException`, 403 for
//! `NotAuthorizedException` and 400 for `InvalidParameterException` and
//! `GroupExistsException`
//! (`smithy_query(query="shape",shape="ResourceNotFoundException,UserNotFoundException,NotAuthorizedException,GroupExistsException")`),
//! and this endpoint answers one status for all of them, with the class carried in the
//! body's `__type` and in the header an AWS SDK resolves the exception type from first.
//!
//! The rows span what the rule generalises over, so no row is the rule: the four error
//! CLASSES (and so the three modelled statuses) a refusal of this surface is raised with,
//! the resource FAMILY the lookup resolved nothing for (app client, app-client secret,
//! group, resource server, identity provider, user pool, user), the LAYER the refusal was
//! raised at (a handler's own modelled error, and the framework's refusal of a value
//! outside a modelled constraint, which no handler sees), and the KIND of refusal (a
//! resource that does not exist, a caller that is not authorized, a duplicate create, a
//! malformed request). A SUCCESS is the control: it keeps its status and names no error
//! shape.
//!
//! Every assertion names its case and reports the value it saw.
//!
//! The harness below (the router, the one call, and the pieces of an answer this rule is
//! read from) is `pub(crate)` so a sibling suite asserting the SAME rule over another
//! part of this surface drives it through this one, rather than through a second copy.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service. The signature is
/// never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The response header an AWS SDK resolves the concrete exception type from.
pub(crate) const ERRORTYPE: &str = "x-amzn-errortype";

/// The status this endpoint answers every refusal of the caller with.
pub(crate) const REFUSED: u16 = 400;

/// A pool id nothing created, in the form `UserPoolIdType`'s pattern demands
/// (`^[\w-]+_[0-9a-zA-Z]+$`, from
/// `smithy_query(query="operation",operation="ListUserPoolClients")`).
pub(crate) const UNMINTED_POOL: &str = "us-east-1_notapool1";

/// An app client id nothing minted, in the form `ClientIdType`'s pattern demands
/// (`^[\w+]+$`, from `smithy_query(query="operation",operation="DescribeUserPoolClient")`).
pub(crate) const UNMINTED_CLIENT: &str = "notanappclient1234567890ab";

/// One emulator with just this service mounted.
pub(crate) fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router.set_request_authentication(false);
    router
}

/// What one call answered: its HTTP status, its `x-amzn-errortype` header and its JSON
/// body.
pub(crate) struct Answered {
    pub(crate) status: u16,
    pub(crate) errortype: Option<String>,
    pub(crate) payload: Value,
}

/// Call `operation` with `body`.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> Answered {
    let (status, headers, payload) = answered_with_headers(router, operation, body).await;
    let errortype = headers
        .get(ERRORTYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    Answered {
        status,
        errortype,
        payload,
    }
}

/// Call `operation` with `body`, answering the status, EVERY response header and the JSON
/// body — for a rule about what a response reports beside its body (the id it names the
/// request it answers by, `crate::response_request_id`).
pub(crate) async fn answered_with_headers(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, http::HeaderMap, Value) {
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
    let headers = response.headers().clone();
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
    (status, headers, payload)
}

/// Call `operation`, requiring success, and return its response body.
pub(crate) async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let answered = call(router, operation, body).await;
    assert_eq!(
        answered.status, 200,
        "{operation} must succeed, got {} with {}",
        answered.status, answered.payload
    );
    answered.payload
}

/// The `__type` of an error body, which is the wire error code.
pub(crate) fn error_code(payload: &Value) -> &str {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// One row: a call this service refuses, and the error class it refuses it with.
struct Refusal {
    case: &'static str,
    operation: &'static str,
    body: Value,
    class: &'static str,
}

#[tokio::test]
async fn every_refusal_is_a_400_naming_its_error_shape_in_the_errortype_header() {
    let router = router();

    // One live pool with one app client, one group, one resource server and one user, so
    // every row below is refused for the reason it names rather than for a pool that was
    // never there.
    let pool = ok(&router, "CreateUserPool", &json!({ "PoolName": "error-wire" })).await;
    let user_pool_id = pool["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    ok(
        &router,
        "CreateUserPoolClient",
        &json!({ "UserPoolId": &user_pool_id, "ClientName": "error-wire-client" }),
    )
    .await;
    ok(
        &router,
        "CreateGroup",
        &json!({ "UserPoolId": &user_pool_id, "GroupName": "readers" }),
    )
    .await;
    ok(
        &router,
        "AdminCreateUser",
        &json!({ "UserPoolId": &user_pool_id, "Username": "alice" }),
    )
    .await;

    let rows = vec![
        // A resource the lookup resolves nothing for, over the families this endpoint
        // addresses: the model gives this class 404, the wire gives it 400.
        Refusal {
            case: "ListUserPoolClients on a pool nothing created",
            operation: "ListUserPoolClients",
            body: json!({ "UserPoolId": UNMINTED_POOL, "MaxResults": 10 }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "CreateUserPoolClient in a pool nothing created",
            operation: "CreateUserPoolClient",
            body: json!({ "UserPoolId": UNMINTED_POOL, "ClientName": "nowhere" }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "AddUserPoolClientSecret for an app client the pool does not hold",
            operation: "AddUserPoolClientSecret",
            body: json!({ "UserPoolId": &user_pool_id, "ClientId": UNMINTED_CLIENT }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "CreateGroup in a pool nothing created",
            operation: "CreateGroup",
            body: json!({ "UserPoolId": UNMINTED_POOL, "GroupName": "nowhere" }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "GetGroup for a name the pool does not hold",
            operation: "GetGroup",
            body: json!({ "UserPoolId": &user_pool_id, "GroupName": "writers" }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "ListUsersInGroup for a group the pool does not hold",
            operation: "ListUsersInGroup",
            body: json!({ "UserPoolId": &user_pool_id, "GroupName": "writers" }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "CreateResourceServer in a pool nothing created",
            operation: "CreateResourceServer",
            body: json!({
                "UserPoolId": UNMINTED_POOL,
                "Identifier": "https://api.example.com",
                "Name": "nowhere",
            }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "DescribeResourceServer for an identifier the pool does not hold",
            operation: "DescribeResourceServer",
            body: json!({
                "UserPoolId": &user_pool_id,
                "Identifier": "https://api.example.com",
            }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "ListResourceServers on a pool nothing created",
            operation: "ListResourceServers",
            body: json!({ "UserPoolId": UNMINTED_POOL, "MaxResults": 10 }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "DescribeIdentityProvider for a provider the pool does not hold",
            operation: "DescribeIdentityProvider",
            body: json!({ "UserPoolId": &user_pool_id, "ProviderName": "Google" }),
            class: "ResourceNotFoundException",
        },
        // A user the pool does not hold is its own class, also modelled at 404.
        Refusal {
            case: "AdminGetUser for a user the pool does not hold",
            operation: "AdminGetUser",
            body: json!({ "UserPoolId": &user_pool_id, "Username": "nobody" }),
            class: "UserNotFoundException",
        },
        // A caller this endpoint does not authorize: modelled at 403.
        Refusal {
            case: "GetUser on an access token naming no signed-in user",
            operation: "GetUser",
            body: json!({ "AccessToken": "not.a.token" }),
            class: "NotAuthorizedException",
        },
        // A duplicate create: modelled at 400 already, so only the header is new.
        Refusal {
            case: "CreateGroup for a name the pool already holds",
            operation: "CreateGroup",
            body: json!({ "UserPoolId": &user_pool_id, "GroupName": "readers" }),
            class: "GroupExistsException",
        },
        // A value outside a modelled constraint, refused before any handler runs: the one
        // error no handler of this service raises, so it reaches the caller through the
        // framework's own refusal rendering.
        Refusal {
            case: "CreateGroup with a name shorter than its modelled minimum length",
            operation: "CreateGroup",
            body: json!({ "UserPoolId": &user_pool_id, "GroupName": "" }),
            class: "InvalidParameterException",
        },
        Refusal {
            case: "GetGroup with no group name at all, which the model requires",
            operation: "GetGroup",
            body: json!({ "UserPoolId": &user_pool_id }),
            class: "InvalidParameterException",
        },
    ];

    for Refusal {
        case,
        operation,
        body,
        class,
    } in rows
    {
        let answered = call(&router, operation, &body).await;
        assert_eq!(
            answered.status, REFUSED,
            "case {case:?}: a refusal of the caller is HTTP {REFUSED}, got {} with {}",
            answered.status, answered.payload
        );
        assert_eq!(
            error_code(&answered.payload),
            class,
            "case {case:?}: expected the body to name {class}, got {}",
            answered.payload
        );
        let expected = format!("{class}:");
        assert_eq!(
            answered.errortype.as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected {ERRORTYPE} {expected:?}, got {:?}",
            answered.errortype
        );
    }
}

/// The control: a call this service SERVES keeps its own status and names no error shape,
/// so the header is a statement about a refusal rather than one every response carries.
#[tokio::test]
async fn a_served_call_keeps_its_status_and_names_no_error_shape() {
    let router = router();
    let pool = ok(
        &router,
        "CreateUserPool",
        &json!({ "PoolName": "error-wire-served" }),
    )
    .await;
    let user_pool_id = pool["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    ok(
        &router,
        "CreateGroup",
        &json!({ "UserPoolId": &user_pool_id, "GroupName": "readers" }),
    )
    .await;

    // (case, operation, body) — a create, a read of what it stored, and a listing.
    let rows = vec![
        (
            "a group read back by name",
            "GetGroup",
            json!({ "UserPoolId": &user_pool_id, "GroupName": "readers" }),
        ),
        (
            "the pool's groups listed",
            "ListGroups",
            json!({ "UserPoolId": &user_pool_id }),
        ),
        (
            "an app client created in the pool",
            "CreateUserPoolClient",
            json!({ "UserPoolId": &user_pool_id, "ClientName": "served" }),
        ),
    ];

    for (case, operation, body) in rows {
        let answered = call(&router, operation, &body).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {}",
            answered.status, answered.payload
        );
        assert_eq!(
            answered.errortype, None,
            "case {case:?}: a served call names no error shape, got {:?}",
            answered.errortype
        );
    }
}
