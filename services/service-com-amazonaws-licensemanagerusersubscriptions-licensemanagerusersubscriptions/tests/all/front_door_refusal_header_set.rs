// SPDX-License-Identifier: CC0-1.0

//! The error-type HEADER SET this service answers each refusal class under, read off the
//! raw response — driven over the real wire path through the router's `dispatch`, so every
//! call is deserialized, constraint-checked, handled and serialized exactly as a client's
//! would be.
//!
//! The rule under test: a refusal of this service names its error SHAPE in exactly TWO
//! places and no more — the `restJson1` error-type header (`x-amzn-errortype`), carried
//! ONCE, and the body's `__type` discriminator — and the two always name the same shape, so
//! a client that resolves the error class from the header and one that reads it off the body
//! cannot disagree. A success names no shape in either place. A second occurrence of the
//! header would fold, in any client that joins repeated header values, into a value naming
//! no error shape at all, which is why the set is asserted and not just its first value.
//!
//! Asserted over a TABLE spanning the refusal classes this service can answer rather than
//! one request. The rows differ along what the rule generalises over — WHICH class answered
//! (the front door's non-modelled authorization denial at HTTP 401, the tagging surface's
//! own non-modelled code, a handler's modelled `ValidationException`, the boundary
//! validator's, and a served request that names no shape at all), WHICH operation was asked
//! (the user plane, the identity-provider plane, the license-server plane, the tagging
//! surface), and WHICH way the request named what it acts on (an EC2 instance, an ARN of one
//! of this service's own resources, a (provider, product) pair). The denial rows also pin
//! the status and the fixed sentence that class carries, so the whole envelope of the
//! refusal is covered, not only its headers.
//!
//! The harness (router + engine + the account's directories and network) is the one in
//! [`super::identity_provider_subscriptions`]; only the rule is new here.

use serde_json::{json, Value};

// ONE wire harness per service: the dispatch plumbing and the fixtures come from the
// sibling suites rather than a second copy.
use super::identity_provider_subscriptions::{
    arn_prefix, as_str, dispatch, encoded_label, harness, provider_union, register, Harness,
    PRODUCTS,
};
use super::resource_resolution_refusals::held_instance;

/// The header a REST-JSON client resolves a failure's error shape from. Spelled out here
/// rather than read from the production constant, so a change to it fails a test instead of
/// silently changing what this suite accepts.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// The code this service's front door denies a request under, and the codes the other
/// refusal classes of the table carry.
const UNAUTHORIZED: &str = "UnauthorizedException";
const BAD_REQUEST: &str = "BadRequestException";
const VALIDATION: &str = "ValidationException";

/// The fixed sentence that travels with the denial.
const UNAUTHORIZED_MESSAGE: &str =
    "Unauthorized. Please check the request and access policy and try again.";

/// A directory of the account's fleet this suite registers a provider for.
const REGISTERED_DIRECTORY: &str = "d-0123456789";
/// A directory of the account's fleet this suite registers nothing for.
const UNREGISTERED_DIRECTORY: &str = "d-00000000ff";
/// A subnet of the account's own Amazon EC2 store, which a registration's `Settings` name.
const SUBNET: &str = "subnet-0123456789abcdef0";
/// A security group of that store, likewise.
const SECURITY_GROUP: &str = "sg-0123abcd";

/// One row: a request, the status it is answered with and the error-type header set it
/// leaves with.
struct Row {
    case: String,
    status: u16,
    /// Every occurrence of [`ERROR_TYPE_HEADER`] expected, in order.
    header_set: Vec<&'static str>,
    response: http::Response<bytes::Bytes>,
}

/// Every value `response` carries for [`ERROR_TYPE_HEADER`], in order — the SET a client
/// reading the raw headers observes, rather than the first occurrence alone.
fn error_type_header_set(response: &http::Response<bytes::Bytes>) -> Vec<String> {
    response
        .headers()
        .get_all(ERROR_TYPE_HEADER)
        .iter()
        .map(|value| {
            value
                .to_str()
                .expect("an error-type header value is textual")
                .to_string()
        })
        .collect()
}

/// The response body as JSON, or `Null` when there is none.
fn body_of(response: &http::Response<bytes::Bytes>) -> Value {
    if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    }
}

/// Assert `row` was answered with its status and its whole header set, and that the body's
/// own discriminator agrees with the header: a client reading one and a client reading the
/// other must not disagree about what was refused.
fn assert_row(row: &Row) {
    let Row {
        case,
        status,
        header_set,
        response,
    } = row;
    let body = body_of(response);
    assert_eq!(
        response.status().as_u16(),
        *status,
        "case {case:?}: expected HTTP {status}, got {} with body {body:?}",
        response.status().as_u16()
    );
    let actual = error_type_header_set(response);
    assert_eq!(
        actual, *header_set,
        "case {case:?}: expected the error-type header set {header_set:?}, got {actual:?} \
         with body {body:?}"
    );
    let named = body.get("__type").and_then(Value::as_str);
    assert_eq!(
        named,
        header_set.first().copied(),
        "case {case:?}: the body and the header must name the same shape, got body {named:?} \
         against header set {actual:?}"
    );
}

/// Register `directory_id` for `product` and answer the `IdentityProviderArn` minted.
async fn registered(h: &Harness, directory_id: &str, product: &str, case: &str) -> String {
    let reply = register(
        h,
        &json!({
            "IdentityProvider": provider_union(directory_id),
            "Product": product,
            "Settings": { "Subnets": [SUBNET], "SecurityGroupId": SECURITY_GROUP },
        }),
    )
    .await;
    as_str(
        &reply.ok(case)["IdentityProviderSummary"],
        "IdentityProviderArn",
        case,
    )
    .to_string()
}

/// One `POST` of `body` to `path`, as the raw response it was answered with.
async fn post(h: &Harness, path: &str, body: &Value) -> http::Response<bytes::Bytes> {
    dispatch(h, "POST", path, Some(body)).await
}

/// A denial row: the request is answered HTTP 401 and names the front door's own code once
/// in the header and once in the body.
fn denied(case: &str, response: http::Response<bytes::Bytes>) -> Row {
    Row {
        case: case.to_string(),
        status: 401,
        header_set: vec![UNAUTHORIZED],
        response,
    }
}

/// RULE: every refusal of this service names its error shape ONCE in the error-type header
/// and once in the body's discriminator, the two agreeing — on every surface that can refuse,
/// whichever way the request named what it acts on, and whichever refusal class answered;
/// a served request names no shape in either place.
#[tokio::test]
async fn a_refusal_names_its_error_shape_once_in_the_header_and_once_in_the_body() {
    let h = harness();
    // One real registration and one association, so the denials below are about what each
    // request names and not about an empty store.
    let provider_arn = registered(&h, REGISTERED_DIRECTORY, PRODUCTS[0], "the registration").await;
    let held = held_instance(0);

    // An instance id no instance of the account has, and ARNs of this service's own
    // resource kinds that state holds nothing for.
    let unheld_instance = "i-0123456789abcdef0";
    let unknown_association = format!("{}iu-000000009999", arn_prefix("instance-user"));
    let unknown_endpoint = format!("{}lse-000000009999", arn_prefix("license-server-endpoint"));
    let unknown_product_user = format!("{}pu-000000009999", arn_prefix("user"));
    // An ARN of this service's shape that resolves to no registration: the identity-provider
    // plane's own way of naming what a request acts on. ONE slash in the resource part, which
    // is what the member's `Arn` @pattern admits, so the denial is reached rather than a
    // constraint refusal.
    let unknown_provider = format!("{}idp-000000009999", arn_prefix("identity-provider"));

    let rows: Vec<Row> = vec![
        // The user plane, naming an EC2 instance the account does not hold.
        denied(
            "AssociateUser on an instance the account does not hold",
            post(
                &h,
                "/user/AssociateUser",
                &json!({
                    "Username": "someone",
                    "InstanceId": unheld_instance,
                    "IdentityProvider": provider_union(REGISTERED_DIRECTORY),
                }),
            )
            .await,
        ),
        denied(
            "DisassociateUser on an instance the account does not hold",
            post(
                &h,
                "/user/DisassociateUser",
                &json!({
                    "Username": "someone",
                    "InstanceId": unheld_instance,
                    "IdentityProvider": provider_union(REGISTERED_DIRECTORY),
                }),
            )
            .await,
        ),
        denied(
            "ListUserAssociations on an instance the account does not hold",
            post(
                &h,
                "/user/ListUserAssociations",
                &json!({
                    "InstanceId": unheld_instance,
                    "IdentityProvider": provider_union(REGISTERED_DIRECTORY),
                }),
            )
            .await,
        ),
        // The same plane, naming an association by the ARN that identifies one instead.
        denied(
            "DisassociateUser by an InstanceUserArn state holds nothing for",
            post(
                &h,
                "/user/DisassociateUser",
                &json!({ "InstanceUserArn": unknown_association }),
            )
            .await,
        ),
        // The subscription surface, through a directory nothing is registered for.
        denied(
            "ListProductSubscriptions through an unregistered identity provider",
            post(
                &h,
                "/user/ListProductSubscriptions",
                &json!({
                    "IdentityProvider": provider_union(UNREGISTERED_DIRECTORY),
                    "Product": PRODUCTS[0],
                }),
            )
            .await,
        ),
        denied(
            "StartProductSubscription through an unregistered identity provider",
            post(
                &h,
                "/user/StartProductSubscription",
                &json!({
                    "Username": "someone",
                    "IdentityProvider": provider_union(UNREGISTERED_DIRECTORY),
                    "Product": PRODUCTS[0],
                }),
            )
            .await,
        ),
        denied(
            "StopProductSubscription through an unregistered identity provider",
            post(
                &h,
                "/user/StopProductSubscription",
                &json!({
                    "Username": "someone",
                    "IdentityProvider": provider_union(UNREGISTERED_DIRECTORY),
                    "Product": PRODUCTS[0],
                }),
            )
            .await,
        ),
        denied(
            "StopProductSubscription by a ProductUserArn state holds nothing for",
            post(
                &h,
                "/user/StopProductSubscription",
                &json!({ "ProductUserArn": unknown_product_user }),
            )
            .await,
        ),
        // The identity-provider plane, by the pair and by the ARN.
        denied(
            "DeregisterIdentityProvider by an unregistered (provider, product) pair",
            post(
                &h,
                "/identity-provider/DeregisterIdentityProvider",
                &json!({
                    "IdentityProvider": provider_union(UNREGISTERED_DIRECTORY),
                    "Product": PRODUCTS[0],
                }),
            )
            .await,
        ),
        denied(
            "UpdateIdentityProviderSettings by an unregistered (provider, product) pair",
            post(
                &h,
                "/identity-provider/UpdateIdentityProviderSettings",
                &json!({
                    "IdentityProvider": provider_union(UNREGISTERED_DIRECTORY),
                    "Product": PRODUCTS[0],
                    "UpdateSettings": { "AddSubnets": [SUBNET], "RemoveSubnets": [] },
                }),
            )
            .await,
        ),
        // The license-server plane.
        denied(
            "DeleteLicenseServerEndpoint by an ARN state holds nothing for",
            post(
                &h,
                "/license-server/DeleteLicenseServerEndpoint",
                &json!({
                    "LicenseServerEndpointArn": unknown_endpoint,
                    "ServerType": "RDS_SAL",
                }),
            )
            .await,
        ),
        // THE OTHER CLASSES: a served request names no shape at all, and a refusal of
        // another class names its OWN code — once in each place, exactly as the denial does.
        Row {
            case: "ListIdentityProviders, served".to_string(),
            status: 200,
            header_set: Vec::new(),
            response: post(&h, "/identity-provider/ListIdentityProviders", &json!({})).await,
        },
        Row {
            case: "ListUserAssociations on an instance the account holds, served".to_string(),
            status: 200,
            header_set: Vec::new(),
            response: post(
                &h,
                "/user/ListUserAssociations",
                &json!({
                    "InstanceId": held,
                    "IdentityProvider": provider_union(REGISTERED_DIRECTORY),
                }),
            )
            .await,
        },
        Row {
            case: "ListTagsForResource by an ARN naming no resource".to_string(),
            status: 400,
            header_set: vec![BAD_REQUEST],
            response: dispatch(
                &h,
                "GET",
                &format!("/tags/{}", encoded_label(&unknown_association)),
                None,
            )
            .await,
        },
        Row {
            case: "DeregisterIdentityProvider by an IdentityProviderArn naming no \
                   registration"
                .to_string(),
            status: 400,
            header_set: vec![VALIDATION],
            response: post(
                &h,
                "/identity-provider/DeregisterIdentityProvider",
                &json!({ "IdentityProviderArn": unknown_provider }),
            )
            .await,
        },
        Row {
            case: "DeregisterIdentityProvider by an ARN outside the member's pattern"
                .to_string(),
            status: 400,
            header_set: vec![VALIDATION],
            response: post(
                &h,
                "/identity-provider/DeregisterIdentityProvider",
                &json!({ "IdentityProviderArn": "not-an-arn" }),
            )
            .await,
        },
        Row {
            case: "UpdateIdentityProviderSettings on a held registration, served".to_string(),
            status: 200,
            header_set: Vec::new(),
            response: post(
                &h,
                "/identity-provider/UpdateIdentityProviderSettings",
                &json!({
                    "IdentityProviderArn": provider_arn,
                    "UpdateSettings": { "AddSubnets": [], "RemoveSubnets": [] },
                }),
            )
            .await,
        },
    ];

    for row in &rows {
        assert_row(row);
        // The denial class also carries one fixed sentence, whatever it was asked about: the
        // envelope a client branches on is the status, the shape and that text together.
        if row.status == 401 {
            let body = body_of(&row.response);
            assert_eq!(
                body.get("message").and_then(Value::as_str),
                Some(UNAUTHORIZED_MESSAGE),
                "case {:?}: expected the fixed denial sentence, got {body:?}",
                row.case
            );
        }
    }
}
