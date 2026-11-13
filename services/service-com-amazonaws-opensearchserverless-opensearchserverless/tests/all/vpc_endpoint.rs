// SPDX-License-Identifier: CC0-1.0

//! OpenSearch Serverless-managed interface VPC endpoints end to end over the wire: the
//! create that stores one and the batch read that answers it by id.
//!
//! Every case drives the real request path through [`crate::collection_lifecycle::call`] —
//! `MultiServiceRouter::dispatch` with `POST /`, `X-Amz-Target:
//! OpenSearchServerless.<Operation>` and an `awsJson1_0` body — so the boundary validator,
//! the protocol codec and the handler all take part, exactly as they do for an SDK client.
//!
//! The rule under test: `CreateVpcEndpoint` mints one `VpcEndpointId`, persists the VPC,
//! the subnets and the security groups the request named, and reports that stored record —
//! in the non-terminal `PENDING` status the endpoint settles out of on its own — so a
//! later `BatchGetVpcEndpoint` answers the same endpoint, a name already taken is refused
//! and a replayed `clientToken` answers the endpoint its first call created. The tables
//! span what that rule generalises over: the subnet count and both modelled `SubnetId`
//! widths, the presence and count of `securityGroupIds` up to its published maximum, the
//! `VpcEndpointName` length at both published bounds and one past each, and the `VpcId`
//! suffix length.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};

use crate::collection_lifecycle::{at, call, emulator, text, CLIENT_ERROR};

/// The `VpcEndpointStatus` values the model declares
/// (`smithy_query(query="shape", shape="VpcEndpointStatus")` -> `values`).
const VPC_ENDPOINT_STATUS_VALUES: &[&str] = &["ACTIVE", "DELETING", "FAILED", "PENDING"];

/// `VpcEndpointStatus$PENDING` — "Pending VPCEndpoint resource".
pub(crate) const PENDING: &str = "PENDING";

/// `VpcEndpointStatus$ACTIVE` — "VPCEndpoint resource is ready to use".
pub(crate) const ACTIVE: &str = "ACTIVE";

/// The prefix `VpcEndpointId`'s own `^vpce-[0-9a-z]*$` fixes.
const ID_PREFIX: &str = "vpce-";

/// Long enough for the `PENDING -> ACTIVE` transition `CreateVpcEndpoint` starts to fall
/// due; the emulator's span is 500 ms and nothing sleeps for it inside a handler.
pub(crate) const SETTLE_WAIT: Duration = Duration::from_millis(700);

/// True when `id` satisfies `VpcEndpointId`'s modelled `^vpce-[0-9a-z]*$` and its
/// `@length(min: 1, max: 255)`, with a non-empty suffix — a minted id names something.
pub(crate) fn is_vpc_endpoint_id(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix(ID_PREFIX) else {
        return false;
    };
    (1..=255).contains(&id.len())
        && !suffix.is_empty()
        && suffix
            .chars()
            .all(|character| character.is_ascii_digit() || character.is_ascii_lowercase())
}

/// Create the VPC endpoint `request` describes, asserting it was accepted, and answer its
/// `createVpcEndpointDetail`.
pub(crate) async fn create_endpoint(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    request: Value,
) -> Value {
    let (status, code, answer) = call(router, "CreateVpcEndpoint", request).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200 from CreateVpcEndpoint, got {status} {code} with {answer}"
    );
    at(&answer, "createVpcEndpointDetail").clone()
}

/// The whole `BatchGetVpcEndpoint` answer for `ids`.
pub(crate) async fn read_endpoints(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    ids: &[&str],
) -> Value {
    let (status, code, answer) = call(router, "BatchGetVpcEndpoint", json!({ "ids": ids })).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200 from BatchGetVpcEndpoint, got {status} {code} with {answer}"
    );
    answer
}

/// The single `VpcEndpointDetail` `BatchGetVpcEndpoint` reports for `id`.
pub(crate) async fn read_one(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    id: &str,
) -> Value {
    let answer = read_endpoints(router, case, &[id]).await;
    let details = at(&answer, "vpcEndpointDetails")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        details.len(),
        1,
        "case {case}: expected BatchGetVpcEndpoint({id}) to report exactly one detail, got {answer}"
    );
    details.into_iter().next().unwrap_or(Value::Null)
}

/// Poll `BatchGetVpcEndpoint` until `id` reports `ACTIVE`, answering its detail.
///
/// The `PENDING -> ACTIVE` move is owed by the clock, so this waits for the wall clock
/// rather than for a call: no handler sleeps for it.
pub(crate) async fn await_active(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    id: &str,
) -> Value {
    for _ in 0..10 {
        let detail = read_one(router, case, id).await;
        if text(&detail, "status") == ACTIVE {
            return detail;
        }
        tokio::time::sleep(SETTLE_WAIT / 5).await;
    }
    panic!("case {case}: the endpoint {id} never reached {ACTIVE}");
}

/// The list of strings at `path` of a VPC endpoint detail, or an empty list when the member
/// is absent.
pub(crate) fn ids(value: &Value, path: &str) -> Vec<String> {
    at(value, path)
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Assert that `detail` reports the identity and the inputs of the endpoint `request`
/// created, whatever status it is in.
pub(crate) fn assert_members(case: &str, id: &str, request: &Value, detail: &Value) {
    assert_eq!(
        text(detail, "id"),
        id,
        "case {case}: expected the detail of {id}, got {detail}"
    );
    assert_eq!(
        text(detail, "name"),
        text(request, "name"),
        "case {case}: expected the requested name, got {detail}"
    );
    assert_eq!(
        text(detail, "vpcId"),
        text(request, "vpcId"),
        "case {case}: expected the requested vpcId, got {detail}"
    );
    assert_eq!(
        at(detail, "subnetIds"),
        at(request, "subnetIds"),
        "case {case}: expected the requested subnetIds in request order, got {detail}"
    );
    // `securityGroupIds` is optional on the request and `@length(min: 1)` on the wire, so
    // an endpoint created without any reports none rather than an empty list.
    match request.get("securityGroupIds") {
        Some(groups) => assert_eq!(
            at(detail, "securityGroupIds"),
            groups,
            "case {case}: expected the requested securityGroupIds, got {detail}"
        ),
        None => assert_eq!(
            at(detail, "securityGroupIds"),
            &Value::Null,
            "case {case}: expected no securityGroupIds, got {detail}"
        ),
    }
    let status = text(detail, "status");
    assert!(
        VPC_ENDPOINT_STATUS_VALUES.contains(&status.as_str()),
        "case {case}: expected a modelled VpcEndpointStatus, got {status:?} in {detail}"
    );
    // Nothing in this slice fails an endpoint, so a stored record carries no failure.
    assert_eq!(
        at(detail, "failureCode"),
        &Value::Null,
        "case {case}: expected no failureCode, got {detail}"
    );
    assert_eq!(
        at(detail, "failureMessage"),
        &Value::Null,
        "case {case}: expected no failureMessage, got {detail}"
    );
}

// ---------------------------------------------------------------------------
// The create/read round trip, over a table that spans the subnet count, both modelled
// `SubnetId` widths, the security-group count up to the published maximum, and the
// `VpcEndpointName` length at both published bounds.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_created_vpc_endpoint_is_what_the_batch_read_reports() {
    let router = emulator();

    // `SubnetId` is `^subnet-([0-9a-f]{8}|[0-9a-f]{17})$` and `SecurityGroupId` is
    // `^[\w+\-]+$` (smithy_query shapes `SubnetIds` / `SecurityGroupIds`); `SubnetIds` is
    // `@length(min: 1)` and `SecurityGroupIds` `@length(min: 1, max: 5)`;
    // `VpcEndpointName` is `@length(min: 3, max: 32)` with `^[a-z][a-z0-9-]+$`. Row 1
    // names one subnet of the short width and no security groups, so it pins that an
    // endpoint created without any reports none; row 2 names three subnets of the long
    // width and one group; row 3 names the published maximum of five groups; rows 4 and 5
    // sit on the name bounds, 3 and 32 characters.
    let longest_name = format!("a{}", "b".repeat(31));
    let cases: Vec<(String, Value)> = vec![
        (
            "one short-width subnet, no security groups".to_string(),
            json!({
                "name": "first-endpoint",
                "vpcId": "vpc-1a2b3c4d",
                "subnetIds": ["subnet-1a2b3c4d"],
            }),
        ),
        (
            "three long-width subnets, one security group".to_string(),
            json!({
                "name": "second-endpoint",
                "vpcId": "vpc-0123456789abcdef0",
                "subnetIds": [
                    "subnet-0123456789abcdef0",
                    "subnet-abcdef01234567890",
                    "subnet-00000000000000001",
                ],
                "securityGroupIds": ["sg-0123456789abcdef0"],
            }),
        ),
        (
            "two subnets, the maximum of five security groups".to_string(),
            json!({
                "name": "third-endpoint",
                "vpcId": "vpc-9f8e7d6c",
                "subnetIds": ["subnet-11111111", "subnet-22222222"],
                "securityGroupIds": ["sg-1", "sg-2", "sg-3", "sg-4", "sg+5"],
            }),
        ),
        (
            "a name at the minimum length of three".to_string(),
            json!({
                "name": "abc",
                "vpcId": "vpc-",
                "subnetIds": ["subnet-deadbeef"],
                "securityGroupIds": ["sg-deadbeef"],
            }),
        ),
        (
            "a name at the maximum length of 32".to_string(),
            json!({
                "name": longest_name,
                "vpcId": "vpc-00000000000000000000000",
                "subnetIds": ["subnet-cafebabe", "subnet-cafebabecafebabe0"],
            }),
        ),
    ];

    let mut ids: BTreeSet<String> = BTreeSet::new();
    for (case, request) in &cases {
        let detail = create_endpoint(&router, case, request.clone()).await;
        let id = text(&detail, "id");
        assert!(
            is_vpc_endpoint_id(&id),
            "case {case}: expected an id matching ^vpce-[0-9a-z]*$, got {id:?}"
        );
        assert_eq!(
            text(&detail, "name"),
            text(request, "name"),
            "case {case}: expected the create to report the requested name, got {detail}"
        );
        assert_eq!(
            text(&detail, "status"),
            PENDING,
            "case {case}: expected the create to report {PENDING}, got {detail}"
        );
        assert!(
            ids.insert(id.clone()),
            "case {case}: expected a fresh id, got {id:?} a second time"
        );

        // The read straight after the create still observes the transient status, so the
        // span the endpoint settles over is never zero.
        let pending = read_one(&router, case, &id).await;
        assert_eq!(
            text(&pending, "status"),
            PENDING,
            "case {case}: expected the read straight after the create to report {PENDING}, got {pending}"
        );
        assert_members(case, &id, request, &pending);
        let created_date = at(&pending, "createdDate").as_i64().unwrap_or_default();
        assert!(
            created_date > 0,
            "case {case}: expected an epoch-millisecond createdDate, got {pending}"
        );

        // The endpoint reaches ACTIVE on its own, and nothing else about it changes.
        let active = await_active(&router, case, &id).await;
        assert_members(case, &id, request, &active);
        assert_eq!(
            at(&active, "createdDate").as_i64().unwrap_or_default(),
            created_date,
            "case {case}: expected a stable createdDate across reads, got {active}"
        );
    }

    // Every endpoint created above is still readable in one batch, in request order.
    let created: Vec<&str> = ids.iter().map(String::as_str).collect();
    let answer = read_endpoints(&router, "every created endpoint", &created).await;
    let reported: Vec<String> = at(&answer, "vpcEndpointDetails")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|detail| text(detail, "id"))
        .collect();
    assert_eq!(
        reported, created,
        "case every created endpoint: expected each requested id answered in request order, got {answer}"
    );
}

// ---------------------------------------------------------------------------
// A name already taken, over two requests that differ in everything else.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_vpc_endpoint_name_already_taken_is_refused() {
    let router = emulator();

    let first = json!({
        "name": "taken-name",
        "vpcId": "vpc-1a2b3c4d",
        "subnetIds": ["subnet-1a2b3c4d"],
    });
    let detail = create_endpoint(&router, "the first create", first.clone()).await;
    let id = text(&detail, "id");

    // The second request carries the same name and nothing else in common: the name alone
    // identifies one endpoint in the account.
    let cases: Vec<(&str, Value)> = vec![
        ("the identical request", first.clone()),
        (
            "another VPC, subnets and security groups",
            json!({
                "name": "taken-name",
                "vpcId": "vpc-9f8e7d6c",
                "subnetIds": ["subnet-99999999", "subnet-88888888"],
                "securityGroupIds": ["sg-other"],
            }),
        ),
    ];
    for (case, request) in cases {
        let (status, code, answer) = call(&router, "CreateVpcEndpoint", request).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ConflictException"),
            "case {case}: expected {CLIENT_ERROR} ConflictException, got {status} {code} with {answer}"
        );
        assert!(
            text(&answer, "message").contains("taken-name"),
            "case {case}: expected the refusal to name the name the request carried, got {answer}"
        );
    }

    // The refused creates stored nothing: the name still resolves to the first endpoint.
    let detail = read_one(&router, "after the refusals", &id).await;
    assert_eq!(
        text(&detail, "name"),
        "taken-name",
        "case after the refusals: expected the first endpoint to still hold the name, got {detail}"
    );

    // A free name is still created, so the refusal is about the name and not the account.
    let other = create_endpoint(
        &router,
        "a free name",
        json!({
            "name": "free-name",
            "vpcId": "vpc-1a2b3c4d",
            "subnetIds": ["subnet-1a2b3c4d"],
        }),
    )
    .await;
    assert_ne!(
        text(&other, "id"),
        id,
        "case a free name: expected a second endpoint with its own id, got {other}"
    );
}

// ---------------------------------------------------------------------------
// The `@idempotencyToken` replay: a spent token answers the endpoint its first call
// created, and the same token on another name is another endpoint.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_client_token_answers_the_endpoint_its_first_call_created() {
    let router = emulator();

    let request = json!({
        "name": "token-endpoint",
        "vpcId": "vpc-1a2b3c4d",
        "subnetIds": ["subnet-1a2b3c4d"],
        "clientToken": "a-client-token",
    });
    let first = create_endpoint(&router, "the first call", request.clone()).await;
    let id = text(&first, "id");

    // The retry answers the SAME endpoint rather than conflicting on the name or minting a
    // second id.
    let retry = create_endpoint(&router, "the retry", request.clone()).await;
    assert_eq!(
        text(&retry, "id"),
        id,
        "case the retry: expected the endpoint the first call created, got {retry}"
    );

    // The same token spent on another name is a request for another endpoint, which is
    // created on its own terms.
    let other = create_endpoint(
        &router,
        "the token on another name",
        json!({
            "name": "other-endpoint",
            "vpcId": "vpc-1a2b3c4d",
            "subnetIds": ["subnet-22222222"],
            "clientToken": "a-client-token",
        }),
    )
    .await;
    let other_id = text(&other, "id");
    assert_ne!(
        other_id, id,
        "case the token on another name: expected a second endpoint, got {other}"
    );

    let answer = read_endpoints(&router, "both endpoints", &[&id, &other_id]).await;
    let names: Vec<String> = at(&answer, "vpcEndpointDetails")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|detail| text(detail, "name"))
        .collect();
    assert_eq!(
        names,
        vec!["token-endpoint".to_string(), "other-endpoint".to_string()],
        "case both endpoints: expected exactly the two created endpoints, got {answer}"
    );
}

// ---------------------------------------------------------------------------
// An id that names no endpoint, beside ids that do.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn batch_get_reports_the_not_found_error_detail_for_an_unknown_id() {
    let router = emulator();

    let mut ids = Vec::new();
    for name in ["read-one", "read-two"] {
        let detail = create_endpoint(
            &router,
            name,
            json!({
                "name": name,
                "vpcId": "vpc-1a2b3c4d",
                "subnetIds": ["subnet-1a2b3c4d"],
            }),
        )
        .await;
        ids.push(text(&detail, "id"));
    }

    // Ids of the modelled shape that no endpoint carries: one never minted, and one whose
    // suffix is empty — both are well-formed `VpcEndpointId`s, so they reach the handler.
    let unknown = ["vpce-00000000000000000", "vpce-"];
    let requested: Vec<&str> = vec![ids[0].as_str(), unknown[0], ids[1].as_str(), unknown[1]];
    let answer = read_endpoints(&router, "two known and two unknown ids", &requested).await;

    let reported: Vec<String> = at(&answer, "vpcEndpointDetails")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|detail| text(detail, "id"))
        .collect();
    assert_eq!(
        reported,
        vec![ids[0].clone(), ids[1].clone()],
        "case two known and two unknown ids: expected only the stored endpoints, in request order, got {answer}"
    );

    let errors = at(&answer, "vpcEndpointErrorDetails")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        errors.len(),
        unknown.len(),
        "case two known and two unknown ids: expected one error detail per unknown id, got {answer}"
    );
    for (error, id) in errors.iter().zip(unknown) {
        assert_eq!(
            text(error, "id"),
            id,
            "case the error detail of {id}: expected the id the request carried, got {error}"
        );
        assert_eq!(
            text(error, "errorCode"),
            "NOT_FOUND",
            "case the error detail of {id}: expected the NOT_FOUND error code, got {error}"
        );
        assert_eq!(
            text(error, "errorMessage"),
            "The specified VpcEndpoint is not found.",
            "case the error detail of {id}: expected the not-found message, got {error}"
        );
    }
}

// ---------------------------------------------------------------------------
// The modelled constraints on the create input, at each published bound and one past it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_create_outside_the_modelled_constraints_is_refused() {
    let router = emulator();

    // One past each published bound of `CreateVpcEndpointRequest`: `name`
    // `@length(min: 3, max: 32)` with `^[a-z][a-z0-9-]+$`, `vpcId` `^vpc-[0-9a-z]*$`,
    // `subnetIds` `@length(min: 1)` over `^subnet-([0-9a-f]{8}|[0-9a-f]{17})$`,
    // `securityGroupIds` `@length(min: 1, max: 5)` — the accepted bounds themselves are
    // the rows of the round-trip table above.
    let too_long_name = format!("a{}", "b".repeat(32));
    let cases: Vec<(String, Value)> = vec![
        (
            "a name one character under the minimum".to_string(),
            json!({ "name": "ab", "vpcId": "vpc-1a2b3c4d", "subnetIds": ["subnet-1a2b3c4d"] }),
        ),
        (
            "a name one character over the maximum".to_string(),
            json!({
                "name": too_long_name,
                "vpcId": "vpc-1a2b3c4d",
                "subnetIds": ["subnet-1a2b3c4d"],
            }),
        ),
        (
            "a name outside the modelled pattern".to_string(),
            json!({
                "name": "Upper-Case",
                "vpcId": "vpc-1a2b3c4d",
                "subnetIds": ["subnet-1a2b3c4d"],
            }),
        ),
        (
            "no name at all".to_string(),
            json!({ "vpcId": "vpc-1a2b3c4d", "subnetIds": ["subnet-1a2b3c4d"] }),
        ),
        (
            "a vpcId outside the modelled pattern".to_string(),
            json!({
                "name": "bad-vpc",
                "vpcId": "vpc_1a2b3c4d",
                "subnetIds": ["subnet-1a2b3c4d"],
            }),
        ),
        (
            "no vpcId at all".to_string(),
            json!({ "name": "no-vpc", "subnetIds": ["subnet-1a2b3c4d"] }),
        ),
        (
            "one subnet under the minimum of one".to_string(),
            json!({ "name": "no-subnets", "vpcId": "vpc-1a2b3c4d", "subnetIds": [] }),
        ),
        (
            "no subnetIds at all".to_string(),
            json!({ "name": "no-subnet-member", "vpcId": "vpc-1a2b3c4d" }),
        ),
        (
            "a subnet id of neither modelled width".to_string(),
            json!({
                "name": "bad-subnet",
                "vpcId": "vpc-1a2b3c4d",
                "subnetIds": ["subnet-123"],
            }),
        ),
        (
            "one security group over the maximum of five".to_string(),
            json!({
                "name": "too-many-groups",
                "vpcId": "vpc-1a2b3c4d",
                "subnetIds": ["subnet-1a2b3c4d"],
                "securityGroupIds": ["sg-1", "sg-2", "sg-3", "sg-4", "sg-5", "sg-6"],
            }),
        ),
        (
            "one security group under the minimum of one".to_string(),
            json!({
                "name": "no-groups",
                "vpcId": "vpc-1a2b3c4d",
                "subnetIds": ["subnet-1a2b3c4d"],
                "securityGroupIds": [],
            }),
        ),
    ];

    for (case, request) in cases {
        let (status, code, answer) = call(&router, "CreateVpcEndpoint", request).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ValidationException"),
            "case {case}: expected {CLIENT_ERROR} ValidationException, got {status} {code} with {answer}"
        );
    }

    // Nothing above was stored: a refused create leaves the account without endpoints.
    let answer = read_endpoints(
        &router,
        "after every refusal",
        &["vpce-00000000000000000"],
    )
    .await;
    assert!(
        at(&answer, "vpcEndpointDetails")
            .as_array()
            .is_some_and(Vec::is_empty),
        "case after every refusal: expected no endpoint to have been stored, got {answer}"
    );
}
