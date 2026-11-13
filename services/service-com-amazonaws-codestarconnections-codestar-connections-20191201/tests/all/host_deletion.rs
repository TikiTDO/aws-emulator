// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteHost`.
//!
//! Every call goes over the wire: a request is dispatched through the
//! [`MultiServiceRouter`] exactly as the emulator's front door does it (`POST /` +
//! `X-Amz-Target: <prefix>.DeleteHost`, awsJson1_0 body), so what is asserted here is
//! the service's HTTP behaviour and not a handler called directly.
//!
//! The rules pinned, each over a TABLE rather than one request:
//!
//! * a `HostArn` naming a host the store holds deletes that host and nothing else, for
//!   every status the host may be deletable in;
//! * a `HostArn` naming no host is refused as ABSENT and writes nothing, for every form
//!   of a well-shaped ARN that resolves to nothing;
//! * a host the service holds but may not delete yet — its VPC configuration in flight,
//!   or connections still associated with it — is refused as UNAVAILABLE and stays;
//! * a target whose prefix names another service deletes nothing here.
//!
//! Hosts are SEEDED into the shared engine's store rather than created through the API,
//! so each row starts from exactly the status it is about — including the two
//! transitional VPC states no create produces. That is the same store the handler reads
//! — the engine is shared, as `state_roundtrip.rs` pins — so a seeded host is observed
//! through the wire exactly as a created one is; `host_lifecycle.rs` and
//! `host_listing.rs` cover the hosts `CreateHost` itself mints.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codestarconnections_codestar_connections_20191201 as codestar;

use codestar::state::{
    ComAmazonawsCodestarconnectionsCodestarConnections20191201State as ServiceState, StoredHost,
    ARN_NAMESPACE, HOST_STATUS_AVAILABLE, HOST_STATUS_DELETE_BLOCKED, HOST_STATUS_VALUES,
};

/// The region the test signs for; the ARNs it builds and the service resolves carry it.
const REGION: &str = "us-west-2";

/// The canonical emulator account id (`engine::emulator_account_id`).
const ACCOUNT_ID: &str = "000000000000";

/// The awsJson1_0 target prefix built from the service shape's LOCAL name, from
/// `OPERATIONS.md` ("Wire protocol") — the form the framework's own routing key resolves
/// today, so every call here goes through it.
const SHORT_TARGET_PREFIX: &str = "CodeStar_connections_20191201";

/// One emulator: the router the production front door builds, with this service mounted
/// on a single shared engine, plus a handle on that engine.
///
/// The existing `connection_lifecycle.rs` harness does not expose the engine, and these
/// tests need it to seed the host store, so this one keeps both halves.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The tests sign nothing, so no authenticator may be registered.
        router.set_request_authentication(false);
        router.register_service(codestar::EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Dispatch one awsJson1_0 call under `target`, optionally carrying a SigV4
    /// credential scope, and return `(status, parsed body)`.
    async fn call_target(&self, target: &str, body: Value, signed: bool) -> (u16, Value) {
        let mut request = ::http::Request::builder()
            .method("POST")
            .uri("https://codestar-connections.us-west-2.amazonaws.com/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", target);
        if signed {
            // The SigV4 credential scope is what routes the request to this service and
            // what tells it which region to mint ARNs in.
            request = request.header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/{REGION}\
                     /codestar-connections/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            );
        }
        let request = request
            .body(::bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("the request builds");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Dispatch one signed call to `operation` under the short target prefix.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        self.call_target(&format!("{SHORT_TARGET_PREFIX}.{operation}"), body, true)
            .await
    }

    /// `CreateConnection`, asserting success, and return the minted ARN.
    async fn create_connection(&self, case: &str, body: Value) -> String {
        let (status, response) = self.call("CreateConnection", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateConnection to answer 200, got {status} {response:?}"
        );
        response["ConnectionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no ConnectionArn in {response:?}"))
            .to_string()
    }

    /// Put one host into the shared store, as `CreateHost` will once it is written, and
    /// return its ARN.
    fn seed_host(&self, id: &str, status: &str, vpc: Option<VpcSeed>) -> String {
        let host_arn = host_arn(id);
        let state = self.engine.state::<ServiceState>();
        let mut store = state.lock();
        // Creation order comes from the store's own counter, exactly as `CreateHost`
        // takes it, so two seeded hosts never share a sequence.
        let sequence = store.take_sequence();
        let record = StoredHost {
            name: format!("host-{id}"),
            host_arn: host_arn.clone(),
            provider_type: "GitHubEnterpriseServer".to_string(),
            provider_endpoint: "https://ghe.example.internal".to_string(),
            vpc_configuration: vpc.map(|seed| codestar::ops::types::VpcConfiguration {
                vpc_id: seed.vpc_id.to_string(),
                subnet_ids: seed.subnet_ids.iter().map(ToString::to_string).collect(),
                security_group_ids: seed
                    .security_group_ids
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                tls_certificate: None,
            }),
            status: status.to_string(),
            status_message: None,
            sequence,
        };
        store.hosts.insert(host_arn.clone(), record);
        host_arn
    }

    /// Attach one tag to a stored resource, the way `TagResource` merges one.
    fn seed_tag(&self, resource_arn: &str, key: &str, value: &str) {
        self.engine
            .state::<ServiceState>()
            .lock()
            .merge_tags(
                resource_arn,
                &[codestar::ops::types::Tag {
                    key: key.to_string(),
                    value: value.to_string(),
                }],
            );
    }

    /// Overwrite a stored host's status, as the service itself will when the host's VPC
    /// configuration finishes provisioning.
    fn set_host_status(&self, host_arn: &str, status: &str) {
        let state = self.engine.state::<ServiceState>();
        let mut store = state.lock();
        let host = store
            .hosts
            .get_mut(host_arn)
            .unwrap_or_else(|| panic!("the seeded host {host_arn} is in the store"));
        host.status = status.to_string();
    }

    /// Whether the store holds a host under `host_arn`.
    fn holds_host(&self, host_arn: &str) -> bool {
        self.engine
            .state::<ServiceState>()
            .lock()
            .hosts
            .contains_key(host_arn)
    }

    /// Every host ARN the store holds.
    fn host_arns(&self) -> Vec<String> {
        self.engine
            .state::<ServiceState>()
            .lock()
            .hosts
            .keys()
            .cloned()
            .collect()
    }

    /// The tags the store holds for `resource_arn`, as key/value pairs.
    fn tags_of(&self, resource_arn: &str) -> Vec<(String, String)> {
        self.engine
            .state::<ServiceState>()
            .lock()
            .tag_list(resource_arn)
            .into_iter()
            .map(|tag| (tag.key, tag.value))
            .collect()
    }
}

/// One `VpcConfiguration` to seed, in the modeled forms (`VpcId` `^vpc-\w{8}(\w{9})?$`,
/// one to ten subnets and security groups).
#[derive(Clone, Copy)]
struct VpcSeed {
    vpc_id: &'static str,
    subnet_ids: &'static [&'static str],
    security_group_ids: &'static [&'static str],
}

/// A host ARN of the modeled `HostArn` form
/// `^arn:aws(-[\w]+)*:codestar-connections:.+:[0-9]{12}:host\/.+$`, in this request's
/// region and account.
fn host_arn(id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, REGION, &format!("host/{id}"))
}

/// The error code an awsJson error envelope carries (`__type`), reduced to the shape
/// NAME: the awsJson protocols allow the code to be written with or without its
/// namespace, and a client matches on the name.
fn error_code(response: &Value) -> String {
    let raw = response["__type"].as_str().unwrap_or_default();
    raw.rsplit_once('#').map_or(raw, |(_, name)| name).to_string()
}

/// The `Message` an awsJson error envelope carries.
fn error_message(response: &Value) -> String {
    response["Message"]
        .as_str()
        .or_else(|| response["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// Every status [`HOST_STATUS_VALUES`] names that is not one of the two
/// [`HOST_STATUS_DELETE_BLOCKED`] states — derived from the catalog, so the table widens
/// with it instead of drifting from it.
fn deletable_statuses() -> Vec<&'static str> {
    HOST_STATUS_VALUES
        .iter()
        .copied()
        .filter(|status| !HOST_STATUS_DELETE_BLOCKED.contains(status))
        .collect()
}

/// RULE: `DeleteHost` removes the host its `HostArn` names — and only that host, with
/// the tags keyed by its ARN — and answers an empty 200; a host in any status but the two
/// transitional VPC states is deletable, with or without a VPC configuration.
#[tokio::test]
async fn deleting_a_stored_host_removes_that_host_and_its_tags() {
    const VPC: VpcSeed = VpcSeed {
        vpc_id: "vpc-0a1b2c3d",
        subnet_ids: &["subnet-0a1b2c3d", "subnet-1b2c3d4e"],
        security_group_ids: &["sg-0a1b2c3d"],
    };

    // The rows differ along the status the host is in and whether it carries a VPC
    // configuration — the two things a host record can differ in that this rule has to
    // hold across.
    let mut rows: Vec<(String, &'static str, Option<VpcSeed>)> = Vec::new();
    for status in deletable_statuses() {
        rows.push((format!("{status}/no-vpc"), status, None));
        rows.push((format!("{status}/vpc"), status, Some(VPC)));
    }
    assert!(
        rows.len() >= 6,
        "the status catalog must drive this table, got {} rows",
        rows.len()
    );

    for (case, status, vpc) in rows {
        let emulator = Emulator::new();
        // A sibling host and a sibling connection: the delete must leave both alone.
        let other = emulator.seed_host("sibling-host", status, vpc);
        emulator.seed_tag(&other, "owner", "sibling");
        let target = emulator.seed_host("deleted-host", status, vpc);
        emulator.seed_tag(&target, "owner", "platform");
        emulator.seed_tag(&target, "tier", "build");

        let (status_code, response) = emulator
            .call("DeleteHost", json!({ "HostArn": target }))
            .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected DeleteHost to answer 200, got {status_code} {response:?}"
        );
        assert_eq!(
            response,
            json!({}),
            "case {case:?}: DeleteHost models an empty output, got {response:?}"
        );
        assert!(
            !emulator.holds_host(&target),
            "case {case:?}: the host is still in the store after DeleteHost"
        );
        assert_eq!(
            emulator.tags_of(&target),
            Vec::<(String, String)>::new(),
            "case {case:?}: the deleted host's tags outlived it: {:?}",
            emulator.tags_of(&target)
        );
        assert_eq!(
            emulator.host_arns(),
            vec![other.clone()],
            "case {case:?}: DeleteHost must leave every other host in place"
        );
        assert_eq!(
            emulator.tags_of(&other),
            vec![("owner".to_string(), "sibling".to_string())],
            "case {case:?}: DeleteHost must leave every other resource's tags in place"
        );

        // Deleting it again is the ABSENT answer, not a second success: the record is
        // gone, so there is nothing left to delete.
        let (status_code, response) = emulator
            .call("DeleteHost", json!({ "HostArn": target }))
            .await;
        assert_eq!(
            status_code, 400,
            "case {case:?}: expected 400 for a host already deleted, got {status_code} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: wrong error code in {response:?}"
        );
    }
}

/// RULE: a `HostArn` that resolves to no host this service holds is refused as absent —
/// the modeled `ResourceNotFoundException`, at the status the live service answers, with
/// a message naming the ARN that was supplied — and writes nothing.
#[tokio::test]
async fn an_arn_naming_no_host_is_refused_as_absent_and_writes_nothing() {
    let emulator = Emulator::new();
    // One host and one connection exist, so "absent" is about the ARN resolving to
    // nothing and not about an empty store.
    let kept = emulator.seed_host("kept-host", HOST_STATUS_AVAILABLE, None);
    let connection = emulator
        .create_connection(
            "setup",
            json!({ "ConnectionName": "kept-connection", "ProviderType": "GitHub" }),
        )
        .await;

    // Every row is a well-shaped `HostArn` (the modeled pattern) that names no host:
    // the recorded all-zero id, another region, another account, another partition, a
    // name-shaped id, a deeper resource path. An ARN of another KIND of resource does
    // not satisfy the modeled pattern at all and is refused before the handler — see
    // `a_host_arn_outside_the_modeled_pattern_is_refused_as_invalid`.
    let rows: Vec<(&str, String)> = vec![
        (
            "the recorded all-zero host id",
            format!(
                "arn:aws:codestar-connections:{REGION}:{ACCOUNT_ID}:host/00000000-0000-0000-0000-000000000000"
            ),
        ),
        (
            "another region",
            format!("arn:aws:codestar-connections:eu-central-1:{ACCOUNT_ID}:host/kept-host"),
        ),
        (
            "another account",
            format!("arn:aws:codestar-connections:{REGION}:111122223333:host/kept-host"),
        ),
        (
            "another partition",
            format!("arn:aws-us-gov:codestar-connections:{REGION}:{ACCOUNT_ID}:host/kept-host"),
        ),
        ("a name-shaped id", host_arn("no-such-host")),
        ("a deeper resource path", host_arn("kept-host/extra")),
    ];

    for (case, arn) in rows {
        let (status, response) = emulator.call("DeleteHost", json!({ "HostArn": arn })).await;
        // The model puts `@httpError 404` on `ResourceNotFoundException`, but the
        // recorded wire answer of the live service for this call is HTTP 400 (corpus
        // `simparity-wire-aws-v1`, fixture
        // `codestar-connections-host-error-paths#task#3`), which is the status a client
        // branches on; see `RESOURCE_NOT_FOUND_STATUS` in `src/state.rs`.
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for an ARN naming no host, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: wrong error code in {response:?}"
        );
        // The message is built from the request: it names the ARN that was supplied.
        assert_eq!(
            error_message(&response),
            format!("No host exists with the ARN: {arn}"),
            "case {case:?}: wrong message in {response:?}"
        );
        assert_eq!(
            emulator.host_arns(),
            vec![kept.clone()],
            "case {case:?}: a refused DeleteHost must leave the store untouched"
        );
    }

    // The connection a host ARN did not name is still there, too.
    let (status, response) = emulator
        .call("GetConnection", json!({ "ConnectionArn": connection }))
        .await;
    assert_eq!(
        status, 200,
        "a refused DeleteHost must not touch the connection store, got {status} {response:?}"
    );
}

/// RULE: a host whose VPC configuration is in flight (`VPC_CONFIG_INITIALIZING` or
/// `VPC_CONFIG_DELETING`) is refused as UNAVAILABLE — it is there, so the refusal is not
/// an absence — and is deletable once its status leaves those states.
#[tokio::test]
async fn a_host_whose_vpc_configuration_is_in_flight_is_refused_until_it_settles() {
    for status in HOST_STATUS_DELETE_BLOCKED {
        let emulator = Emulator::new();
        let host = emulator.seed_host("vpc-host", status, None);

        let (code, response) = emulator
            .call("DeleteHost", json!({ "HostArn": host }))
            .await;
        assert_eq!(
            code, 400,
            "case {status:?}: expected 400 for a host in a transitional VPC state, \
             got {code} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceUnavailableException",
            "case {status:?}: wrong error code in {response:?}"
        );
        assert!(
            error_message(&response).contains(status),
            "case {status:?}: the refusal must report the status it refused on, \
             got {response:?}"
        );
        assert!(
            emulator.holds_host(&host),
            "case {status:?}: a refused DeleteHost must leave the host in the store"
        );

        // Once the VPC work is over, the same call succeeds: the refusal was about the
        // host's state and nothing else.
        emulator.set_host_status(&host, HOST_STATUS_AVAILABLE);
        let (code, response) = emulator
            .call("DeleteHost", json!({ "HostArn": host }))
            .await;
        assert_eq!(
            code, 200,
            "case {status:?}: expected 200 once the host left {status}, got {code} {response:?}"
        );
        assert!(
            !emulator.holds_host(&host),
            "case {status:?}: the host is still in the store after a successful DeleteHost"
        );
    }
}

/// RULE: a host still named by a connection cannot be deleted — "before you delete a
/// host, all connections associated to the host must be deleted" — whatever the number of
/// connections, and the refusal words the host it names without reporting how many;
/// connections naming ANOTHER host never block it.
#[tokio::test]
async fn a_host_is_not_deletable_while_connections_are_associated_with_it() {
    // (case, connections attached to the host under test, connections attached to the
    // other host)
    let rows = [
        ("one attached", 1, 0),
        ("two attached", 2, 0),
        ("three attached", 3, 0),
        ("none attached, three on another host", 0, 3),
        ("one attached, two on another host", 1, 2),
    ];

    for (case, attached, elsewhere) in rows {
        let emulator = Emulator::new();
        let host = emulator.seed_host("busy-host", HOST_STATUS_AVAILABLE, None);
        let other_host = emulator.seed_host("other-host", HOST_STATUS_AVAILABLE, None);

        let mut on_host = Vec::new();
        for index in 0..attached {
            on_host.push(
                emulator
                    .create_connection(
                        case,
                        json!({
                            "ConnectionName": format!("attached-{index}"),
                            "ProviderType": "GitHubEnterpriseServer",
                            "HostArn": host,
                        }),
                    )
                    .await,
            );
        }
        for index in 0..elsewhere {
            emulator
                .create_connection(
                    case,
                    json!({
                        "ConnectionName": format!("elsewhere-{index}"),
                        "ProviderType": "GitHubEnterpriseServer",
                        "HostArn": other_host,
                    }),
                )
                .await;
        }

        if attached > 0 {
            let (code, response) = emulator
                .call("DeleteHost", json!({ "HostArn": host }))
                .await;
            assert_eq!(
                code, 400,
                "case {case:?}: expected 400 while connections are attached, \
                 got {code} {response:?}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceUnavailableException",
                "case {case:?}: wrong error code in {response:?}"
            );
            // The refusal is the fixed template with the `HostArn` the request supplied
            // between its two halves, and it reports no count: the SAME message whether
            // one connection or three still hold the host.
            assert_eq!(
                error_message(&response),
                format!("There exist connections for host {host}, host cannot be deleted"),
                "case {case:?}: wrong refusal message in {response:?}"
            );
            assert!(
                emulator.holds_host(&host),
                "case {case:?}: a refused DeleteHost must leave the host in the store"
            );

            // Clear the association the way a caller has to, one connection at a time:
            // only the LAST one unblocks the delete.
            for (cleared, connection) in on_host.iter().enumerate() {
                let (code, response) = emulator
                    .call("DeleteConnection", json!({ "ConnectionArn": connection }))
                    .await;
                assert_eq!(
                    code, 200,
                    "case {case:?}: expected DeleteConnection to answer 200, \
                     got {code} {response:?}"
                );
                let remaining = attached - cleared - 1;
                let (code, response) = emulator
                    .call("DeleteHost", json!({ "HostArn": host }))
                    .await;
                let expected = if remaining == 0 { 200 } else { 400 };
                assert_eq!(
                    code, expected,
                    "case {case:?}: with {remaining} connection(s) left, expected \
                     {expected}, got {code} {response:?}"
                );
            }
        } else {
            let (code, response) = emulator
                .call("DeleteHost", json!({ "HostArn": host }))
                .await;
            assert_eq!(
                code, 200,
                "case {case:?}: connections on another host must not block this delete, \
                 got {code} {response:?}"
            );
        }

        assert!(
            !emulator.holds_host(&host),
            "case {case:?}: the host must be gone once nothing is associated with it"
        );
        // The other host, and the connections that name it, are untouched.
        assert!(
            emulator.holds_host(&other_host),
            "case {case:?}: DeleteHost must leave every other host in place"
        );
    }
}

/// RULE: a `HostArn` outside the modeled constraints is refused by the boundary
/// validator — HTTP 400 `ValidationException` — rather than read as an absent host, and
/// nothing is deleted.
#[tokio::test]
async fn a_host_arn_outside_the_modeled_pattern_is_refused_as_invalid() {
    let emulator = Emulator::new();
    let kept = emulator.seed_host("kept-host", HOST_STATUS_AVAILABLE, None);

    // `HostArn` is modeled `len 0..256`, `pattern
    // ^arn:aws(-[\w]+)*:codestar-connections:.+:[0-9]{12}:host\/.+$`: an empty value, a
    // non-ARN, another service's ARN, a non-host resource, an 11-digit account, an
    // empty resource id, and one past the published length maximum.
    let rows: Vec<(&str, Value)> = vec![
        ("the member absent", json!({})),
        ("an empty HostArn", json!({ "HostArn": "" })),
        ("not an ARN at all", json!({ "HostArn": "kept-host" })),
        (
            "another service's ARN",
            json!({ "HostArn": format!("arn:aws:codepipeline:{REGION}:{ACCOUNT_ID}:host/kept-host") }),
        ),
        (
            "a connection resource, not a host",
            json!({ "HostArn": format!("arn:aws:codestar-connections:{REGION}:{ACCOUNT_ID}:connection/kept") }),
        ),
        (
            "an 11-digit account id",
            json!({ "HostArn": format!("arn:aws:codestar-connections:{REGION}:00000000000:host/kept-host") }),
        ),
        (
            "an empty host id",
            json!({ "HostArn": format!("arn:aws:codestar-connections:{REGION}:{ACCOUNT_ID}:host/") }),
        ),
        (
            "one character past the length maximum",
            json!({ "HostArn": format!(
                "arn:aws:codestar-connections:{REGION}:{ACCOUNT_ID}:host/{}",
                "h".repeat(257)
            ) }),
        ),
    ];

    for (case, body) in rows {
        let (status, response) = emulator.call("DeleteHost", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for an input the model refuses, \
             got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: wrong error code in {response:?}"
        );
        assert_eq!(
            emulator.host_arns(),
            vec![kept.clone()],
            "case {case:?}: a refused DeleteHost must leave the store untouched"
        );
    }
}

/// RULE (the routing half this operation can observe): a target whose prefix names
/// ANOTHER service is not served by this one, so nothing is deleted here for it.
///
/// This service's own SDK prefix is the dotted Coral name
/// (`com.amazonaws.codestar.connections.CodeStar_connections_20191201`). The framework
/// resolves an awsJson routing key at the FIRST dot, so that prefix once reached no
/// modeled operation at all; the service's routing seams now read the target through
/// `service_common::rpc_target` (see `src/service.rs`), so both prefix forms reach the
/// operation named after the LAST dot. The calls in this suite use the short prefix,
/// which the framework routes on its own; `sync_blocker_summary.rs` pins both forms.
#[tokio::test]
async fn a_target_naming_another_service_does_not_delete_anything_here() {
    // (case, target) — the sibling CodeConnections service publishes the same operation
    // names under its own shape id. Only a DOTTED prefix is covered: for a
    // single-identifier prefix the framework's own routing key IS the operation name, so
    // `AnythingAtAll.DeleteHost` is routed by the framework before any service seam sees
    // it, which is pre-existing behaviour this change does not touch.
    let rows = [
        (
            "another service's dotted prefix",
            "com.amazonaws.codeconnections.CodeConnections_20231201.DeleteHost",
        ),
        (
            "a dotted prefix merely ending in this shape name's characters",
            "com.amazonaws.codestar.connections.NotCodeStar_connections_20191201.DeleteHost",
        ),
    ];

    for (case, target) in rows {
        let emulator = Emulator::new();
        let host = emulator.seed_host("routed-host", HOST_STATUS_AVAILABLE, None);

        let (status, response) = emulator
            .call_target(target, json!({ "HostArn": host }), true)
            .await;
        assert_ne!(
            status, 200,
            "case {case:?}: a target naming another service must not be served here, \
             got {status} {response:?}"
        );
        assert!(
            emulator.holds_host(&host),
            "case {case:?}: nothing may be deleted for a target this service does not own"
        );
    }
}
