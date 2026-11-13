// SPDX-License-Identifier: CC0-1.0

//! The Lambda Core network connector lifecycle, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test is the one this control plane states for its only
//! resource: a connector's members are whatever the create, or a later configuration
//! change, stored, every operation computes its answer from that record, and "each
//! configuration update creates a new immutable version".
//!
//! The harness below (router, request helpers, the clock wait) is `pub(crate)` so the
//! sibling suite `timestamp_and_state_reason_wire.rs` drives the emulator the same way
//! rather than keeping a second copy of it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_lambdacore_lambdacoreapiservice::state::{
    ComAmazonawsLambdacoreLambdacoreapiserviceState, LIFECYCLE_TRANSITION, MAX_IDEMPOTENCY_TOKENS,
};
use service_com_amazonaws_lambdacore_lambdacoreapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The URI prefix every operation of this service is bound under.
pub(crate) const PREFIX: &str = "/2026-04-04/network-connectors";

/// A SigV4 credential scope naming this service's signing name (`lambda`). It is what
/// routes a request here, and its region is what a connector ARN is built for.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The region [`SIGNED_FOR_EU_WEST_1`] signs for.
const REGION: &str = "eu-west-1";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`), spelled out
/// here so a wrong account in the implementation fails.
const ACCOUNT: &str = "000000000000";

/// The unqualified ARN the service must build for the connector whose `Id` is `id`: the
/// resource segment of a connector's ARN is the Id the service assigned it, so `Arn` and
/// `Id` of one answer always name the same resource.
pub(crate) fn expected_arn(id: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:network-connector:{id}")
}

/// One emulator: a router carrying only this service, over one shared engine.
pub(crate) fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// The same emulator, with the engine it was built on, for a case that has to read or
/// arrange the store directly.
pub(crate) fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// What the emulator answered: the HTTP status and the JSON body.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// True when the answer is the modelled error `code`.
    fn is_error(&self, status: u16, code: &str) -> bool {
        self.status == status && self.error_code().ends_with(code)
    }
}

pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "lambda.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

pub(crate) async fn create(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", PREFIX, Some(request)).await
}

pub(crate) async fn get(router: &MultiServiceRouter, identifier: &str) -> Answer {
    call(router, "GET", &format!("{PREFIX}/{identifier}"), None).await
}

pub(crate) async fn list(router: &MultiServiceRouter, query: &str) -> Answer {
    call(router, "GET", &format!("{PREFIX}{query}"), None).await
}

pub(crate) async fn update(router: &MultiServiceRouter, identifier: &str, request: Value) -> Answer {
    call(
        router,
        "PUT",
        &format!("{PREFIX}/{identifier}"),
        Some(request),
    )
    .await
}

pub(crate) async fn delete(router: &MultiServiceRouter, identifier: &str) -> Answer {
    call(router, "DELETE", &format!("{PREFIX}/{identifier}"), None).await
}

pub(crate) async fn list_versions(
    router: &MultiServiceRouter,
    identifier: &str,
    query: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("{PREFIX}/{identifier}/versions{query}"),
        None,
    )
    .await
}

pub(crate) fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

pub(crate) fn text(value: &Value, pointer: &str) -> String {
    at(value, pointer).as_str().unwrap_or_default().to_string()
}

/// Assert that `answer` is the modelled 409 carrying exactly `message`.
///
/// Part of the shared harness so every suite that asserts a `ResourceConflictException`'s
/// wording checks the status, the error code and the whole sentence the same way.
pub(crate) fn assert_conflict(case: &str, answer: &Answer, message: &str) {
    assert_eq!(
        answer.status, 409,
        "case {case:?}: expected HTTP 409, got {} with {:?}",
        answer.status, answer.body
    );
    let code = text(&answer.body, "/__type");
    assert!(
        code.ends_with("ResourceConflictException"),
        "case {case:?}: expected ResourceConflictException, got {code:?} with {:?}",
        answer.body
    );
    assert_eq!(
        // `ResourceConflictException`'s message member is modelled `message`.
        text(&answer.body, "/message"),
        message,
        "case {case:?}: expected the refusal to state {message:?}, got {:?}",
        at(&answer.body, "/message")
    );
}

/// The names `ListNetworkConnectors` reported, in the order it reported them.
fn listed_names(answer: &Answer) -> Vec<String> {
    answer.body["NetworkConnectors"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "ListNetworkConnectors must return a NetworkConnectors list: {:?}",
                answer.body
            )
        })
        .iter()
        .map(|summary| summary["Name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The version numbers `ListNetworkConnectorVersions` reported, in order.
fn listed_versions(answer: &Answer) -> Vec<i64> {
    answer.body["NetworkConnectorVersions"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "ListNetworkConnectorVersions must return a NetworkConnectorVersions list: {:?}",
                answer.body
            )
        })
        .iter()
        .map(|summary| summary["Version"].as_i64().unwrap_or_default())
        .collect()
}

/// Wait out the span the clock owes an asynchronous transition, then a margin. Nothing is
/// called here: the transition is owed by the clock, not by a request.
pub(crate) fn wait_for_the_clock() {
    std::thread::sleep(LIFECYCLE_TRANSITION + Duration::from_millis(250));
}

/// A `VpcEgressConfiguration` built from its four modelled members, so a case can vary one
/// at a time. The member names and value sets come from
/// `NetworkConnectorVpcEgressConfiguration` (SubnetIds 1..16, SecurityGroupIds 0..16,
/// NetworkProtocol one of `IPv4` / `DualStack`, AssociatedComputeResourceTypes exactly one
/// of `MicroVm` / `WebFunction`).
pub(crate) fn vpc_egress(
    subnets: &[&str],
    security_groups: &[&str],
    protocol: Option<&str>,
    compute: Option<&str>,
) -> Value {
    let mut configuration = json!({ "SubnetIds": subnets, "SecurityGroupIds": security_groups });
    if let Some(protocol) = protocol {
        configuration["NetworkProtocol"] = json!(protocol);
    }
    if let Some(compute) = compute {
        configuration["AssociatedComputeResourceTypes"] = json!([compute]);
    }
    json!({ "VpcEgressConfiguration": configuration })
}

/// The simplest configuration a create can carry: one subnet, no security groups.
pub(crate) fn minimal_configuration() -> Value {
    vpc_egress(&["subnet-0123456789abcdef0"], &[], None, None)
}

/// An `OperatorRole` `NetworkConnectorRoleArn`'s expression admits.
///
/// A create carries one because the service requires it for the connector type a
/// `VpcEgressConfiguration` selects: the role is what "Lambda assumes to manage elastic
/// network interfaces in your VPC".
pub(crate) const OPERATOR_ROLE: &str = "arn:aws:iam::000000000000:role/LambdaNetworkConnectorOperator";

/// The simplest create request the service accepts: a name, the minimal configuration and
/// the operator role that connector type requires.
pub(crate) fn minimal_create(name: &str) -> Value {
    json!({
        "Name": name,
        "Configuration": minimal_configuration(),
        "OperatorRole": OPERATOR_ROLE,
    })
}

/// Assert that every member `expected` carries is reported identically under `pointer` of
/// `observed` — a member the request left out is not asserted, because an absent optional
/// member is not the same claim as a present one.
fn assert_configuration(case: &str, observed: &Value, pointer: &str, expected: &Value) {
    let expected_members = expected["VpcEgressConfiguration"]
        .as_object()
        .expect("each configuration under test is a VpcEgressConfiguration");
    for (member, value) in expected_members {
        let seen = at(observed, &format!("{pointer}/VpcEgressConfiguration/{member}"));
        assert_eq!(
            &seen, value,
            "case {case:?}: configuration member {member:?} was stored as {value:?} but \
             {pointer} reports {seen:?}"
        );
    }
}

/// `CreateNetworkConnector` persists what it was given, allocates the identity the model
/// requires, and `GetNetworkConnector` reports that record.
///
/// The rows differ along what the rule generalises over — how many subnets, how many
/// security groups, which network protocol, which compute resource type, whether an
/// operator role or tags are present at all — so no row's shape can be special-cased.
#[tokio::test]
async fn a_created_connector_is_reported_from_what_was_stored() {
    let operator_role = "arn:aws:iam::000000000000:role/LambdaNetworkConnectorOperator";
    let cases = vec![
        (
            "only the members a create cannot omit",
            json!({
                "Name": "minimal",
                "Configuration": minimal_configuration(),
                "OperatorRole": operator_role,
            }),
        ),
        (
            "several subnets and security groups, IPv4, MicroVm",
            json!({
                "Name": "many-subnets",
                "Configuration": vpc_egress(
                    &[
                        "subnet-0123456789abcdef0",
                        "subnet-0fedcba9876543210",
                        "subnet-00112233445566778",
                    ],
                    &["sg-0123456789abcdef0", "sg-0fedcba9876543210"],
                    Some("IPv4"),
                    Some("MicroVm"),
                ),
                "OperatorRole": operator_role,
            }),
        ),
        (
            "dual-stack networking for a web function, with tags",
            json!({
                "Name": "dual_stack-WebFunction",
                "Configuration": vpc_egress(
                    &["subnet-0123456789abcdef0"],
                    &["sg-0123456789abcdef0"],
                    Some("DualStack"),
                    Some("WebFunction"),
                ),
                "Tags": { "env": "test", "team": "networking" },
                "OperatorRole": operator_role,
            }),
        ),
        (
            "no security groups at all, and an operator role",
            json!({
                "Name": "no-security-groups",
                "Configuration": vpc_egress(&["subnet-0123456789abcdef0"], &[], Some("IPv4"), None),
                "OperatorRole": operator_role,
            }),
        ),
    ];

    let router = emulator();
    for (case, request) in &cases {
        let created = create(&router, request.clone()).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: expected HTTP 202, got {} with {:?}",
            created.status, created.body
        );

        let name = text(request, "/Name");
        let id = text(&created.body, "/Id");
        assert!(
            !id.is_empty() && id.len() <= 140,
            "case {case:?}: the service allocates a NetworkConnectorId of 1..140 \
             characters, got {id:?}"
        );
        let arn = expected_arn(&id);
        assert_eq!(
            text(&created.body, "/Arn"),
            arn,
            "case {case:?}: the ARN must name the connector's Id, the signed region and the \
             emulator account, got {:?}",
            at(&created.body, "/Arn")
        );
        assert_eq!(
            text(&created.body, "/Name"),
            name,
            "case {case:?}: CreateNetworkConnector must report the stored name, got {:?}",
            at(&created.body, "/Name")
        );
        assert_eq!(
            at(&created.body, "/Version"),
            json!(1),
            "case {case:?}: \"a newly created connector starts at version 1\", got {:?}",
            at(&created.body, "/Version")
        );
        assert_eq!(
            text(&created.body, "/VersionArn"),
            format!("{arn}:1"),
            "case {case:?}: the qualified ARN is the unqualified one plus the version \
             suffix, got {:?}",
            at(&created.body, "/VersionArn")
        );
        assert_eq!(
            at(&created.body, "/State"),
            json!("PENDING"),
            "case {case:?}: \"the network connector starts in PENDING state\", got {:?}",
            at(&created.body, "/State")
        );
        assert_configuration(case, &created.body, "/Configuration", &request["Configuration"]);
        assert_eq!(
            at(&created.body, "/OperatorRole"),
            at(request, "/OperatorRole"),
            "case {case:?}: CreateNetworkConnector must report the stored OperatorRole"
        );

        // The round trip: the create's answer is not the source of truth, the store is.
        let fetched = get(&router, &name).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected HTTP 200 from GetNetworkConnector, got {} with {:?}",
            fetched.status, fetched.body
        );
        for member in ["/Arn", "/Name", "/Id"] {
            assert_eq!(
                at(&fetched.body, member),
                at(&created.body, member),
                "case {case:?}: GetNetworkConnector must report the same {member} the create \
                 persisted"
            );
        }
        assert_configuration(case, &fetched.body, "/Configuration", &request["Configuration"]);
        assert_eq!(
            at(&fetched.body, "/OperatorRole"),
            at(request, "/OperatorRole"),
            "case {case:?}: GetNetworkConnector must report the stored OperatorRole"
        );
        assert_eq!(
            at(&fetched.body, "/LatestVersion"),
            json!(1),
            "case {case:?}: the latest version of a just-created connector is 1, got {:?}",
            at(&fetched.body, "/LatestVersion")
        );
        assert_eq!(
            text(&fetched.body, "/LatestVersionArn"),
            format!("{arn}:1"),
            "case {case:?}: LatestVersionArn is the qualified ARN of version 1, got {:?}",
            at(&fetched.body, "/LatestVersionArn")
        );
        assert!(
            !text(&fetched.body, "/LastModified").is_empty()
                || at(&fetched.body, "/LastModified").is_number(),
            "case {case:?}: GetNetworkConnector must report a LastModified, got {:?}",
            at(&fetched.body, "/LastModified")
        );
        assert_eq!(
            at(&fetched.body, "/LastUpdateStatus"),
            Value::Null,
            "case {case:?}: a connector that was never updated has no LastUpdateStatus, got {:?}",
            at(&fetched.body, "/LastUpdateStatus")
        );
    }

    // Every created connector appears in the list, each with the type its configuration
    // determines.
    let listed = list(&router, "").await;
    let mut expected_names: Vec<String> = cases
        .iter()
        .map(|(_, request)| text(request, "/Name"))
        .collect();
    expected_names.sort();
    assert_eq!(
        listed_names(&listed),
        expected_names,
        "ListNetworkConnectors must report every stored connector, got {:?}",
        listed.body
    );
    for (index, _) in cases.iter().enumerate() {
        assert_eq!(
            at(&listed.body, &format!("/NetworkConnectors/{index}/Type")),
            json!("VPC_EGRESS"),
            "a VpcEgressConfiguration makes the connector type VPC_EGRESS, got {:?}",
            at(&listed.body, &format!("/NetworkConnectors/{index}/Type"))
        );
        assert_eq!(
            at(&listed.body, &format!("/NetworkConnectors/{index}/LatestVersion")),
            json!(1),
            "each summary reports the connector's latest version, got {:?}",
            at(&listed.body, &format!("/NetworkConnectors/{index}/LatestVersion"))
        );
    }
}

/// A connector is not usable the instant the create returns: it reports `PENDING` to a
/// caller acting straight away, and the clock owes it `ACTIVE` without any further call.
#[tokio::test]
async fn a_new_connector_is_pending_until_the_clock_owes_it_active() {
    let router = emulator();
    let started = Instant::now();
    let created = create(&router, minimal_create("provisioning")).await;
    assert_eq!(
        at(&created.body, "/State"),
        json!("PENDING"),
        "CreateNetworkConnector must answer with the transient state, got {:?}",
        at(&created.body, "/State")
    );

    let immediately = get(&router, "provisioning").await;
    if started.elapsed() < LIFECYCLE_TRANSITION {
        assert_eq!(
            at(&immediately.body, "/State"),
            json!("PENDING"),
            "a caller acting inside the transition span still sees PENDING, got {:?}",
            at(&immediately.body, "/State")
        );
        // An update needs an ACTIVE connector, and a delete one that is ACTIVE or FAILED.
        let refused = update(
            &router,
            "provisioning",
            json!({ "Configuration": vpc_egress(&["subnet-0fedcba9876543210"], &[], None, None) }),
        )
        .await;
        assert!(
            refused.is_error(409, "ResourceConflictException"),
            "updating a connector that is not ACTIVE is refused with 409 \
             ResourceConflictException, got {} {:?}",
            refused.status,
            refused.body
        );
    }

    wait_for_the_clock();

    let settled = get(&router, "provisioning").await;
    assert_eq!(
        at(&settled.body, "/State"),
        json!("ACTIVE"),
        "once the transition span has passed the connector is ACTIVE, got {:?}",
        at(&settled.body, "/State")
    );
    let listed = list(&router, "").await;
    assert_eq!(
        at(&listed.body, "/NetworkConnectors/0/State"),
        json!("ACTIVE"),
        "ListNetworkConnectors reports the same settled state, got {:?}",
        listed.body
    );
    let versions = list_versions(&router, "provisioning", "").await;
    assert_eq!(
        at(&versions.body, "/NetworkConnectorVersions/0/State"),
        json!("ACTIVE"),
        "the connector's one version settles with it, got {:?}",
        versions.body
    );
}

/// `@idempotencyToken`: a replayed `ClientToken` answers with the connector that token
/// created, while a fresh token naming a name already in use is a conflict.
#[tokio::test]
async fn a_replayed_client_token_returns_the_same_connector() {
    let router = emulator();
    let request = json!({
        "Name": "idempotent",
        "Configuration": minimal_configuration(),
        "OperatorRole": OPERATOR_ROLE,
        "ClientToken": "token-one",
    });

    let first = create(&router, request.clone()).await;
    assert_eq!(
        first.status, 202,
        "the first create must succeed, got {} with {:?}",
        first.status, first.body
    );

    for case in ["the identical request", "the same token, other members unchanged"] {
        let replay = create(&router, request.clone()).await;
        assert_eq!(
            replay.status, 202,
            "case {case:?}: a replay succeeds, got {} with {:?}",
            replay.status, replay.body
        );
        for member in ["/Arn", "/Id", "/Version", "/VersionArn"] {
            assert_eq!(
                at(&replay.body, member),
                at(&first.body, member),
                "case {case:?}: a replayed ClientToken must answer with the first \
                 connector's {member}"
            );
        }
    }

    let conflicting = create(
        &router,
        json!({
            "Name": "idempotent",
            "Configuration": minimal_configuration(),
            "OperatorRole": OPERATOR_ROLE,
            "ClientToken": "token-two",
        }),
    )
    .await;
    assert!(
        conflicting.is_error(409, "ResourceConflictException"),
        "a second connector cannot take a name already in use: expected 409 \
         ResourceConflictException, got {} {:?}",
        conflicting.status,
        conflicting.body
    );

    let listed = list(&router, "").await;
    assert_eq!(
        listed_names(&listed),
        vec!["idempotent".to_string()],
        "neither the replay nor the refused create may add a second connector, got {:?}",
        listed.body
    );
}

/// "Each configuration update creates a new immutable version" — and "a no-op update
/// returns the current version number".
///
/// The rows differ along which member the change touches (subnets, security groups,
/// protocol, compute type, operator role) and how large it is, plus the two rows that ask
/// for what is already stored.
#[tokio::test]
async fn each_configuration_change_appends_a_version_and_a_no_op_does_not() {
    let base = vpc_egress(
        &["subnet-0123456789abcdef0"],
        &["sg-0123456789abcdef0"],
        Some("IPv4"),
        Some("MicroVm"),
    );
    let role_one = "arn:aws:iam::000000000000:role/OperatorOne";
    let role_two = "arn:aws:iam::000000000000:role/OperatorTwo";

    // (case, request, does it change the stored configuration?)
    let cases = vec![
        (
            "more subnets",
            json!({ "Configuration": vpc_egress(
                &["subnet-0123456789abcdef0", "subnet-0fedcba9876543210"],
                &["sg-0123456789abcdef0"],
                Some("IPv4"),
                Some("MicroVm"),
            ) }),
            true,
        ),
        (
            "another security group",
            json!({ "Configuration": vpc_egress(
                &["subnet-0123456789abcdef0", "subnet-0fedcba9876543210"],
                &["sg-0123456789abcdef0", "sg-0fedcba9876543210"],
                Some("IPv4"),
                Some("MicroVm"),
            ) }),
            true,
        ),
        (
            "a different network protocol",
            json!({ "Configuration": vpc_egress(
                &["subnet-0123456789abcdef0", "subnet-0fedcba9876543210"],
                &["sg-0123456789abcdef0", "sg-0fedcba9876543210"],
                Some("DualStack"),
                Some("MicroVm"),
            ) }),
            true,
        ),
        (
            "a different compute resource type",
            json!({ "Configuration": vpc_egress(
                &["subnet-0123456789abcdef0", "subnet-0fedcba9876543210"],
                &["sg-0123456789abcdef0", "sg-0fedcba9876543210"],
                Some("DualStack"),
                Some("WebFunction"),
            ) }),
            true,
        ),
        (
            "the operator role alone",
            json!({ "OperatorRole": role_two }),
            true,
        ),
        (
            "the operator role it already carries",
            json!({ "OperatorRole": role_two }),
            false,
        ),
        (
            "the configuration it already carries",
            json!({ "Configuration": vpc_egress(
                &["subnet-0123456789abcdef0", "subnet-0fedcba9876543210"],
                &["sg-0123456789abcdef0", "sg-0fedcba9876543210"],
                Some("DualStack"),
                Some("WebFunction"),
            ) }),
            false,
        ),
    ];

    let router = emulator();
    let created = create(
        &router,
        json!({
            "Name": "versioned",
            "Configuration": base,
            "OperatorRole": role_one,
        }),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "the create must succeed, got {} with {:?}",
        created.status, created.body
    );
    let arn = expected_arn(&text(&created.body, "/Id"));
    // An update needs an ACTIVE connector, which the clock owes the new one.
    wait_for_the_clock();

    let mut expected_version = 1_i64;
    let mut expected_configuration = base.clone();
    for (case, request, changes) in &cases {
        let updated = update(&router, "versioned", request.clone()).await;
        assert_eq!(
            updated.status, 202,
            "case {case:?}: expected HTTP 202 from UpdateNetworkConnector, got {} with {:?}",
            updated.status, updated.body
        );
        if *changes {
            expected_version += 1;
            if let Some(configuration) = request.get("Configuration") {
                expected_configuration = configuration.clone();
            }
        }
        assert_eq!(
            at(&updated.body, "/Version"),
            json!(expected_version),
            "case {case:?}: a configuration change increments the version and a no-op \
             returns the current one, got {:?}",
            at(&updated.body, "/Version")
        );
        assert_eq!(
            text(&updated.body, "/VersionArn"),
            format!("{arn}:{expected_version}"),
            "case {case:?}: the new version's qualified ARN carries its version suffix, \
             got {:?}",
            at(&updated.body, "/VersionArn")
        );
        assert_eq!(
            at(&updated.body, "/State"),
            json!("ACTIVE"),
            "case {case:?}: \"the connector remains in ACTIVE state during the update\", \
             got {:?}",
            at(&updated.body, "/State")
        );
        if *changes {
            assert_eq!(
                at(&updated.body, "/LastUpdateStatus"),
                json!("InProgress"),
                "case {case:?}: LastUpdateStatus is InProgress immediately after an update, \
                 got {:?}",
                at(&updated.body, "/LastUpdateStatus")
            );
        }

        // The store, not the answer, is what a later caller reads.
        let fetched = get(&router, "versioned").await;
        assert_eq!(
            at(&fetched.body, "/LatestVersion"),
            json!(expected_version),
            "case {case:?}: GetNetworkConnector must report the bumped latest version, got {:?}",
            at(&fetched.body, "/LatestVersion")
        );
        assert_configuration(case, &fetched.body, "/Configuration", &expected_configuration);
        assert_eq!(
            text(&fetched.body, "/OperatorRole"),
            if expected_version >= 6 { role_two } else { role_one },
            "case {case:?}: an update replaces only the members it carries, got {:?}",
            at(&fetched.body, "/OperatorRole")
        );

        let versions = list_versions(&router, "versioned", "").await;
        assert_eq!(
            listed_versions(&versions),
            (1..=expected_version).collect::<Vec<i64>>(),
            "case {case:?}: every persisted version is listed once, got {:?}",
            versions.body
        );
        let version_arns: Vec<String> = versions.body["NetworkConnectorVersions"]
            .as_array()
            .expect("a versions list")
            .iter()
            .map(|summary| summary["VersionArn"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            version_arns,
            (1..=expected_version)
                .map(|version| format!("{arn}:{version}"))
                .collect::<Vec<String>>(),
            "case {case:?}: each version carries its own qualified ARN, got {version_arns:?}"
        );
    }

    // The update's own asynchronous work is owed by the clock too.
    wait_for_the_clock();
    let settled = get(&router, "versioned").await;
    assert_eq!(
        at(&settled.body, "/LastUpdateStatus"),
        json!("Successful"),
        "LastUpdateStatus \"transitions through InProgress to Successful\", got {:?}",
        at(&settled.body, "/LastUpdateStatus")
    );

    // A version-qualified identifier reads one historical version, not the connector.
    for version in 1..=expected_version {
        let qualified = get(&router, &format!("{arn}:{version}")).await;
        assert_eq!(
            qualified.status, 200,
            "version {version}: expected HTTP 200, got {} with {:?}",
            qualified.status, qualified.body
        );
        assert_eq!(
            at(&qualified.body, "/Version"),
            json!(version),
            "version {version}: a qualified read reports the version it was asked for, got {:?}",
            at(&qualified.body, "/Version")
        );
        assert_eq!(
            text(&qualified.body, "/VersionArn"),
            format!("{arn}:{version}"),
            "version {version}: VersionArn is populated on a qualified read, got {:?}",
            at(&qualified.body, "/VersionArn")
        );
        assert_eq!(
            at(&qualified.body, "/LatestVersionArn"),
            Value::Null,
            "version {version}: LatestVersionArn is \"returned only on a standard \
             (unqualified) request\", got {:?}",
            at(&qualified.body, "/LatestVersionArn")
        );
    }
    let first_version = get(&router, &format!("{arn}:1")).await;
    assert_configuration("the first version", &first_version.body, "/Configuration", &base);
}

/// Every form `NetworkConnectorIdentifier` accepts resolves to the same record, and a form
/// that names nothing is refused with the modelled 404.
#[tokio::test]
async fn an_identifier_resolves_by_name_id_and_arn() {
    let router = emulator();
    let created = create(&router, minimal_create("resolvable")).await;
    let id = text(&created.body, "/Id");
    let arn = expected_arn(&id);

    let found = vec![
        ("the name", "resolvable".to_string()),
        ("the id", id.clone()),
        ("the unqualified ARN", arn.clone()),
        ("the version-qualified ARN", format!("{arn}:1")),
    ];
    for (case, identifier) in found {
        let answer = get(&router, &identifier).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: {identifier} must resolve, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            text(&answer.body, "/Id"),
            id,
            "case {case:?}: every identifier form names the same connector"
        );
    }

    let missing = vec![
        ("an unknown name", "no-such-connector".to_string()),
        ("an unknown id", "nc-ffffffffffffffff".to_string()),
        ("an unknown ARN", expected_arn("no-such-connector")),
        ("a version this connector has not got", format!("{arn}:7")),
    ];
    for (case, identifier) in missing {
        let answer = get(&router, &identifier).await;
        assert!(
            answer.is_error(404, "ResourceNotFoundException"),
            "case {case:?}: {identifier} names nothing, so expected 404 \
             ResourceNotFoundException, got {} {:?}",
            answer.status,
            answer.body
        );
    }

    // The two operations that document an unqualified identifier refuse a qualified one.
    let qualified = format!("{arn}:1");
    let refused_update = update(
        &router,
        &qualified,
        json!({ "Configuration": vpc_egress(&["subnet-0fedcba9876543210"], &[], None, None) }),
    )
    .await;
    assert!(
        refused_update.is_error(400, "InvalidParameterValueException"),
        "a version is immutable, so a qualified update is refused with 400 \
         InvalidParameterValueException, got {} {:?}",
        refused_update.status,
        refused_update.body
    );
    let refused_list = list_versions(&router, &qualified, "").await;
    assert!(
        refused_list.is_error(400, "InvalidParameterValueException"),
        "ListNetworkConnectorVersions documents an unqualified identifier, so a qualified \
         one is refused with 400 InvalidParameterValueException, got {} {:?}",
        refused_list.status,
        refused_list.body
    );
}

/// `ListNetworkConnectors` pages a stable total order: every connector appears exactly
/// once across the pages, `NextMarker` is present only while more remain, and the `State`
/// filter reports only the connectors in that state.
#[tokio::test]
async fn listing_pages_without_duplicates_and_filters_by_state() {
    let router = emulator();
    let names = [
        "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf",
    ];
    for name in names {
        let created = create(&router, minimal_create(name)).await;
        assert_eq!(
            created.status, 202,
            "creating {name} must succeed, got {} with {:?}",
            created.status, created.body
        );
    }
    let mut sorted: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
    sorted.sort();

    // Every page size from one item to more than there are connectors.
    for max_items in [1_usize, 2, 3, names.len(), names.len() + 5] {
        let mut seen: Vec<String> = Vec::new();
        let mut query = format!("?MaxItems={max_items}");
        loop {
            let page = list(&router, &query).await;
            assert_eq!(
                page.status, 200,
                "case MaxItems={max_items}: expected HTTP 200, got {} with {:?}",
                page.status, page.body
            );
            let page_names = listed_names(&page);
            assert!(
                page_names.len() <= max_items,
                "case MaxItems={max_items}: a page may not exceed the requested size, got {:?}",
                page_names
            );
            seen.extend(page_names);
            match page.body["NextMarker"].as_str() {
                Some(marker) => query = format!("?MaxItems={max_items}&Marker={marker}"),
                None => break,
            }
        }
        assert_eq!(
            seen, sorted,
            "case MaxItems={max_items}: paging must report every connector exactly once, in \
             one stable order, got {seen:?}"
        );
    }

    wait_for_the_clock();
    for (case, state, expected) in [
        ("the state they all settled in", "ACTIVE", sorted.clone()),
        ("a state none of them is in", "DELETING", Vec::new()),
        ("the state they left behind", "PENDING", Vec::new()),
    ] {
        let filtered = list(&router, &format!("?State={state}")).await;
        assert_eq!(
            listed_names(&filtered),
            expected,
            "case {case:?}: the State={state} filter must report only connectors in that \
             state, got {:?}",
            filtered.body
        );
        assert_eq!(
            at(&filtered.body, "/NextMarker"),
            Value::Null,
            "case {case:?}: one page holds every match, so no NextMarker is reported, got {:?}",
            at(&filtered.body, "/NextMarker")
        );
    }
}

/// `DeleteNetworkConnector` removes the connector and all of its versions, and a
/// version-qualified delete removes one historical version and leaves the rest.
#[tokio::test]
async fn deleting_removes_the_connector_and_its_versions() {
    let router = emulator();
    let created = create(&router, minimal_create("doomed")).await;
    let id = text(&created.body, "/Id");
    let arn = expected_arn(&id);
    wait_for_the_clock();

    // Three versions, so there is a historical version to remove on its own.
    for subnets in [
        vec!["subnet-0123456789abcdef0", "subnet-0fedcba9876543210"],
        vec!["subnet-00112233445566778"],
    ] {
        let updated = update(
            &router,
            "doomed",
            json!({ "Configuration": vpc_egress(&subnets, &[], None, None) }),
        )
        .await;
        assert_eq!(
            updated.status, 202,
            "each update must succeed, got {} with {:?}",
            updated.status, updated.body
        );
    }

    let latest = delete(&router, &format!("{arn}:3")).await;
    assert!(
        latest.is_error(409, "ResourceConflictException"),
        "\"you can't delete the latest version on its own\": expected 409 \
         ResourceConflictException, got {} {:?}",
        latest.status,
        latest.body
    );

    let historical = delete(&router, &format!("{arn}:1")).await;
    assert_eq!(
        historical.status, 202,
        "deleting a historical version must succeed, got {} with {:?}",
        historical.status, historical.body
    );
    assert_eq!(
        at(&historical.body, "/Version"),
        json!(1),
        "the answer reports the version it removed, got {:?}",
        at(&historical.body, "/Version")
    );
    assert_eq!(
        text(&historical.body, "/Id"),
        id,
        "the answer echoes the connector's persisted id, got {:?}",
        at(&historical.body, "/Id")
    );
    let remaining = list_versions(&router, "doomed", "").await;
    assert_eq!(
        listed_versions(&remaining),
        vec![2, 3],
        "\"the connector and its remaining versions are unaffected\", got {:?}",
        remaining.body
    );
    let gone = get(&router, &format!("{arn}:1")).await;
    assert!(
        gone.is_error(404, "ResourceNotFoundException"),
        "the removed version no longer resolves: expected 404, got {} {:?}",
        gone.status,
        gone.body
    );
    let connector = get(&router, "doomed").await;
    assert_eq!(
        connector.status, 200,
        "the connector itself survives a version delete, got {} with {:?}",
        connector.status, connector.body
    );

    // The whole connector: DELETING first, forgotten once the clock says so.
    let deleted = delete(&router, "doomed").await;
    assert_eq!(
        deleted.status, 202,
        "expected HTTP 202 from DeleteNetworkConnector, got {} with {:?}",
        deleted.status, deleted.body
    );
    for (member, expected) in [("/Arn", json!(arn)), ("/Name", json!("doomed")), ("/Id", json!(id))]
    {
        assert_eq!(
            at(&deleted.body, member),
            expected,
            "the delete echoes the persisted {member}, got {:?}",
            at(&deleted.body, member)
        );
    }
    assert_eq!(
        at(&deleted.body, "/State"),
        json!("DELETING"),
        "\"the connector transitions to DELETING state\", got {:?}",
        at(&deleted.body, "/State")
    );

    wait_for_the_clock();
    let after = get(&router, "doomed").await;
    assert!(
        after.is_error(404, "ResourceNotFoundException"),
        "\"after deletion completes, subsequent calls to GetNetworkConnector return \
         ResourceNotFoundException\", got {} {:?}",
        after.status,
        after.body
    );
    let listed = list(&router, "").await;
    assert_eq!(
        listed_names(&listed),
        Vec::<String>::new(),
        "a deleted connector is absent from ListNetworkConnectors, got {:?}",
        listed.body
    );
    // The idempotent success covers a connector this account still HAS, whatever state its
    // own deletion left it in; once the deletion has COMPLETED the resource is gone, so the
    // identifier names no connector of this account and Region and the service answers what
    // `ResourceNotFoundException` is documented for — "the specified network connector does
    // not exist".
    let again = delete(&router, "doomed").await;
    assert!(
        again.is_error(404, "ResourceNotFoundException"),
        "a delete of a connector whose deletion completed names nothing, got {} with {:?}",
        again.status,
        again.body
    );
    assert_eq!(
        at(&again.body, "/Message"),
        json!("Network connector not found: doomed"),
        "the refusal echoes the identifier the request carried, got {:?}",
        at(&again.body, "/Message")
    );

    // And the name is free again, with a fresh identity.
    let recreated = create(&router, minimal_create("doomed")).await;
    assert_eq!(
        recreated.status, 202,
        "the name a deleted connector held is free again, got {} with {:?}",
        recreated.status, recreated.body
    );
    assert_ne!(
        text(&recreated.body, "/Id"),
        id,
        "a new connector gets its own id, got {:?}",
        at(&recreated.body, "/Id")
    );
}

/// `ListNetworkConnectorVersions` pages and filters the same way, and refuses a marker it
/// never issued.
#[tokio::test]
async fn version_listing_pages_and_filters() {
    let router = emulator();
    create(&router, minimal_create("paged")).await;
    wait_for_the_clock();
    for subnets in [
        vec!["subnet-0fedcba9876543210"],
        vec!["subnet-00112233445566778"],
        vec!["subnet-0123456789abcdef0", "subnet-0fedcba9876543210"],
    ] {
        let updated = update(
            &router,
            "paged",
            json!({ "Configuration": vpc_egress(&subnets, &[], None, None) }),
        )
        .await;
        assert_eq!(
            updated.status, 202,
            "each update must succeed, got {} with {:?}",
            updated.status, updated.body
        );
    }

    for max_items in [1_i64, 2, 4, 9] {
        let mut seen: Vec<i64> = Vec::new();
        let mut query = format!("?MaxItems={max_items}");
        loop {
            let page = list_versions(&router, "paged", &query).await;
            assert_eq!(
                page.status, 200,
                "case MaxItems={max_items}: expected HTTP 200, got {} with {:?}",
                page.status, page.body
            );
            let versions = listed_versions(&page);
            assert!(
                i64::try_from(versions.len()).expect("a page fits in i64") <= max_items,
                "case MaxItems={max_items}: a page may not exceed the requested size, got \
                 {versions:?}"
            );
            seen.extend(versions);
            match page.body["NextMarker"].as_str() {
                Some(marker) => query = format!("?MaxItems={max_items}&Marker={marker}"),
                None => break,
            }
        }
        assert_eq!(
            seen,
            vec![1, 2, 3, 4],
            "case MaxItems={max_items}: every version is listed exactly once, in version \
             order, got {seen:?}"
        );
    }

    wait_for_the_clock();
    let active = list_versions(&router, "paged", "?State=ACTIVE").await;
    assert_eq!(
        listed_versions(&active),
        vec![1, 2, 3, 4],
        "every settled version is ACTIVE, got {:?}",
        active.body
    );
    let pending = list_versions(&router, "paged", "?State=PENDING").await;
    assert_eq!(
        listed_versions(&pending),
        Vec::<i64>::new(),
        "no version is still PENDING once the clock has settled them, got {:?}",
        pending.body
    );

    for marker in ["not-a-version", "0", "-3"] {
        let refused = list_versions(&router, "paged", &format!("?Marker={marker}")).await;
        assert!(
            refused.is_error(400, "InvalidParameterValueException"),
            "marker {marker:?} was never issued by this operation, so expected 400 \
             InvalidParameterValueException, got {} {:?}",
            refused.status,
            refused.body
        );
    }

    let unknown = list_versions(&router, "no-such-connector", "").await;
    assert!(
        unknown.is_error(404, "ResourceNotFoundException"),
        "listing the versions of a connector that does not exist answers the modelled 404, \
         got {} {:?}",
        unknown.status,
        unknown.body
    );
}

/// The client tokens the store remembers are bounded: a caller presenting a fresh token on
/// every call cannot grow that map without end, and the tokens still in it replay.
#[tokio::test]
async fn remembered_client_tokens_stay_bounded() {
    let (router, engine) = emulator_with_engine();
    let created = create(
        &router,
        json!({
            "Name": "bounded",
            "Configuration": minimal_configuration(),
            "OperatorRole": OPERATOR_ROLE,
            "ClientToken": "token-create",
        }),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "the create must succeed, got {} with {:?}",
        created.status, created.body
    );
    wait_for_the_clock();

    // A no-op update appends no version and consumes no quota, so the token map is the
    // only thing a caller could grow by repeating it with a new token each time.
    let repeats = MAX_IDEMPOTENCY_TOKENS + 50;
    let mut last_token = String::new();
    for attempt in 0..repeats {
        last_token = format!("token-{attempt}");
        let answer = update(
            &router,
            "bounded",
            json!({ "Configuration": minimal_configuration(), "ClientToken": last_token }),
        )
        .await;
        assert_eq!(
            answer.status, 202,
            "attempt {attempt}: a no-op update must succeed, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            at(&answer.body, "/Version"),
            json!(1),
            "attempt {attempt}: \"a no-op update returns the current version number\", got {:?}",
            at(&answer.body, "/Version")
        );
    }

    let remembered = engine
        .state::<ComAmazonawsLambdacoreLambdacoreapiserviceState>()
        .lock()
        .idempotency
        .len();
    assert!(
        remembered <= MAX_IDEMPOTENCY_TOKENS,
        "after {repeats} calls each carrying a fresh ClientToken the store must remember at \
         most {MAX_IDEMPOTENCY_TOKENS} of them, got {remembered}"
    );

    // The most recent token is one of those kept, and it still replays the request it was
    // first presented with.
    let replay = update(
        &router,
        "bounded",
        json!({
            "Configuration": minimal_configuration(),
            "ClientToken": last_token,
        }),
    )
    .await;
    assert_eq!(
        at(&replay.body, "/Version"),
        json!(1),
        "a token the store still remembers replays its own outcome rather than appending a \
         version, got {:?}",
        at(&replay.body, "/Version")
    );
    let versions = list_versions(&router, "bounded", "").await;
    assert_eq!(
        listed_versions(&versions),
        vec![1],
        "no repeat of a no-op update may append a version, got {:?}",
        versions.body
    );
}
