// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite-definition and suite-run lifecycle over the wire.
//!
//! Every case drives real HTTP requests through the same `MultiServiceRouter` the emulator
//! serves on a socket, so what is asserted here is the service's wire behaviour: the
//! response of a create is observable by a later read, a listing pages, a run's status
//! moves on the clock, and a delete makes the resource gone.
//!
//! The tables below span the classes the rules generalise over — every `Protocol` the model
//! enumerates, device lists of each permitted size, both `maxResults` bounds and one past
//! each, and every suite-run status the lifecycle reaches — rather than the one
//! configuration the bootstrap issue cited.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_iotdeviceadvisor_iotsenateservice as iotdeviceadvisor;
use smithy_server_runtime::MultiServiceRouter;

/// `SuiteDefinitionConfiguration$devicePermissionRoleArn` is `len 20..2048`; this is the
/// role every row below points at.
pub(crate) const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/DeviceAdvisorRole";

/// A thing ARN for a device under test.
pub(crate) fn thing_arn(n: usize) -> String {
    format!("arn:aws:iot:us-east-1:000000000000:thing/device-under-test-{n}")
}

/// The access key id a call presents unless the caller names another.
pub(crate) const DEFAULT_AKID: &str = "AKIDEXAMPLE";

/// The `Authorization` header a call presents: a SigV4 credential scope for `region`,
/// which is both what the framework resolves the region from and the signal the router
/// routes the request to this service by. Signed in scope only — authentication is off in
/// these suites, and signature verification is the framework's own concern.
fn authorization(akid: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={akid}/20260101/{region}/iotdeviceadvisor/\
         aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
    )
}

/// The emulator, assembled in-process with authentication off: this suite asserts
/// behaviour, and signature verification is the framework's own concern.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    /// The one shared engine the service's handlers write through, kept so a case can
    /// assert an invariant of the STORE itself (its occupancy bounds) and not only of the
    /// responses.
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

/// One response, reduced to what a client branches on.
pub(crate) struct Answer {
    pub(crate) status: u16,
    /// The parsed JSON body, or `Value::Null` for an empty one.
    pub(crate) body: Value,
    /// The restJson1 error code (`x-amzn-errortype`), when the answer carries one.
    pub(crate) error_code: Option<String>,
}

impl Answer {
    pub(crate) fn member(&self, name: &str) -> &Value {
        &self.body[name]
    }

    pub(crate) fn string(&self, name: &str) -> String {
        self.member(name)
            .as_str()
            .unwrap_or_else(|| panic!("member {name} is not a string in {}", self.body))
            .to_string()
    }
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(iotdeviceadvisor::EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// How many suite definitions and suite runs the shared store actually holds.
    fn stored_counts(&self) -> (usize, usize) {
        let state = iotdeviceadvisor::state::service_state(&*self.engine)
            .expect("the service's store is reachable from the shared engine");
        let store = state.lock();
        (store.definitions.len(), store.runs.len())
    }

    /// How many tag rows the store holds, split into (definition rows, run rows).
    fn stored_tag_counts(&self) -> (usize, usize) {
        let state = iotdeviceadvisor::state::service_state(&*self.engine)
            .expect("the service's store is reachable from the shared engine");
        let store = state.lock();
        let runs = store
            .tags
            .keys()
            .filter(|arn| arn.contains(":suiterun/"))
            .count();
        (store.tags.len() - runs, runs)
    }

    /// Put `count` FINISHED runs of `suite_definition_id` straight into the store, oldest
    /// first, each with a tag row, and hand back their ids in that order.
    ///
    /// A fixture, not a behaviour: the service serialises runs of one definition, so
    /// reaching the retention count over the wire would mean waiting out a transition
    /// span per run. What is under test is what the next real `StartSuiteRun` does with a
    /// store that is already at the limit.
    fn seed_finished_runs(&self, suite_definition_id: &str, count: usize) -> Vec<String> {
        let state = iotdeviceadvisor::state::service_state(&*self.engine)
            .expect("the service's store is reachable from the shared engine");
        let mut store = state.lock();
        let base = std::time::SystemTime::now() - Duration::from_secs(3600);
        (0..count)
            .map(|index| {
                let id = iotdeviceadvisor::state::new_uuid();
                let arn = iotdeviceadvisor::state::arn(
                    "us-east-1",
                    &format!("suiterun/{suite_definition_id}/{id}"),
                );
                let created_at = base + Duration::from_secs(index as u64);
                store.runs.insert(
                    id.clone(),
                    iotdeviceadvisor::state::SuiteRun {
                        id: id.clone(),
                        arn: arn.clone(),
                        suite_definition_id: suite_definition_id.to_string(),
                        suite_definition_version: "v1".to_string(),
                        configuration: Default::default(),
                        status: "STOPPED".to_string(),
                        created_at,
                        started_at: Some(created_at),
                        end_at: Some(created_at + Duration::from_secs(1)),
                        error_reason: None,
                        endpoint: iotdeviceadvisor::state::endpoint_for_region("us-east-1"),
                        next_transition_at: None,
                    },
                );
                store.tags.insert(
                    arn,
                    [("seeded".to_string(), index.to_string())]
                        .into_iter()
                        .collect(),
                );
                id
            })
            .collect()
    }

    /// Send one request, signed (in scope only — authentication is off) for `region`, so
    /// the handler resolves the same region a real SDK client would.
    pub(crate) async fn call(
        &self,
        method: &str,
        path_and_query: &str,
        region: &str,
        body: Option<Value>,
    ) -> Answer {
        self.call_as(DEFAULT_AKID, method, path_and_query, region, body)
            .await
    }

    /// The same request, presenting `akid` as the caller's access key id.
    ///
    /// Which caller made the call is observable — the service records it — so a suite that
    /// reads its own records back signs with an access key id no other suite uses rather
    /// than sharing [`DEFAULT_AKID`].
    pub(crate) async fn call_as(
        &self,
        akid: &str,
        method: &str,
        path_and_query: &str,
        region: &str,
        body: Option<Value>,
    ) -> Answer {
        let payload = body.map_or_else(bytes::Bytes::new, |value| {
            bytes::Bytes::from(serde_json::to_vec(&value).expect("the request body serializes"))
        });
        let request = http::Request::builder()
            .method(method)
            .uri(path_and_query)
            .header("host", format!("api.iotdeviceadvisor.{region}.amazonaws.com"))
            .header("content-type", "application/json")
            .header("authorization", authorization(akid, region))
            .body(payload)
            .expect("the request builds");

        self.dispatch(request).await
    }

    /// Send one request addressed the way the caller asks, rather than through the one
    /// endpoint form [`Emulator::call_as`] sends: `host` carries the RAW bytes of the
    /// `Host` header (`None` sends no `Host` header at all, as an HTTP/2 request does),
    /// and `authority` puts the endpoint on the request URI in absolute form, which is
    /// where an HTTP/2 or SDK-built request carries it.
    pub(crate) async fn call_addressed(
        &self,
        method: &str,
        path_and_query: &str,
        region: &str,
        host: Option<&[u8]>,
        authority: Option<&str>,
    ) -> Answer {
        self.call_addressed_with_body(method, path_and_query, region, host, authority, None)
            .await
    }

    /// The same request, carrying `body` as its JSON payload — what a POST addressed
    /// through a chosen endpoint needs, where [`Emulator::call_addressed`] sends none.
    pub(crate) async fn call_addressed_with_body(
        &self,
        method: &str,
        path_and_query: &str,
        region: &str,
        host: Option<&[u8]>,
        authority: Option<&str>,
        body: Option<Value>,
    ) -> Answer {
        let uri = match authority {
            Some(authority) => format!("https://{authority}{path_and_query}"),
            None => path_and_query.to_string(),
        };
        let mut request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .header("authorization", authorization(DEFAULT_AKID, region));
        if let Some(host) = host {
            request = request.header(
                http::header::HOST,
                http::HeaderValue::from_bytes(host).expect("the Host header value is sendable"),
            );
        }
        let payload = body.map_or_else(bytes::Bytes::new, |value| {
            bytes::Bytes::from(serde_json::to_vec(&value).expect("the request body serializes"))
        });
        let request = request.body(payload).expect("the request builds");

        self.dispatch(request).await
    }

    /// Serve one already-built request and reduce the response to what a client branches
    /// on.
    async fn dispatch(&self, request: http::Request<bytes::Bytes>) -> Answer {
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Answer {
            status,
            body,
            error_code,
        }
    }

    pub(crate) async fn create_definition(&self, configuration: &Value, region: &str) -> Answer {
        self.call(
            "POST",
            "/suiteDefinitions",
            region,
            Some(json!({ "suiteDefinitionConfiguration": configuration })),
        )
        .await
    }
}

/// The configuration rows the create/read round-trip is asserted over: one per `Protocol`
/// the model enumerates (source: the `Protocol` enum, re-exported as `PROTOCOL_VALUES`),
/// plus rows differing in device-list size (0, 1 and the modelled maximum of 2), in both
/// booleans, and in `rootGroup` form.
pub(crate) fn definition_rows() -> Vec<(String, Value)> {
    let mut rows: Vec<(String, Value)> = Vec::new();

    for (index, protocol) in iotdeviceadvisor::ops::types::PROTOCOL_VALUES
        .iter()
        .enumerate()
    {
        rows.push((
            format!("protocol {protocol}"),
            json!({
                "suiteDefinitionName": format!("protocol-suite-{index}"),
                "rootGroup": format!("group-{index}"),
                "devicePermissionRoleArn": ROLE_ARN,
                "protocol": protocol,
                "devices": [{ "thingArn": thing_arn(index) }],
            }),
        ));
    }

    rows.push((
        "no devices, no optional flags".to_string(),
        json!({
            "suiteDefinitionName": "minimal-suite",
            "rootGroup": "root-group-only",
            "devicePermissionRoleArn": ROLE_ARN,
        }),
    ));
    rows.push((
        "empty device list".to_string(),
        json!({
            "suiteDefinitionName": "empty-device-list",
            "rootGroup": "g",
            "devicePermissionRoleArn": ROLE_ARN,
            "devices": [],
        }),
    ));
    rows.push((
        // `DeviceUnderTestList` is `len 0..2`, so two devices is the modelled maximum.
        "two devices, long duration, certificate and role ARNs".to_string(),
        json!({
            "suiteDefinitionName": "two-device-suite",
            "rootGroup": "root/group/with/separators",
            "devicePermissionRoleArn": ROLE_ARN,
            "isLongDurationTest": true,
            "intendedForQualification": false,
            "devices": [
                {
                    "thingArn": thing_arn(7),
                    "certificateArn": "arn:aws:iot:us-east-1:000000000000:cert/aabbccddee",
                },
                { "deviceRoleArn": ROLE_ARN },
            ],
        }),
    ));
    rows.push((
        // The model: an empty `rootGroup` is legal exactly when the suite is intended for
        // qualification.
        "qualification suite with an empty rootGroup".to_string(),
        json!({
            "suiteDefinitionName": "qualification-suite",
            "rootGroup": "",
            "intendedForQualification": true,
            "devicePermissionRoleArn": ROLE_ARN,
            "devices": [{ "thingArn": thing_arn(1) }],
        }),
    ));
    rows.push((
        // `SuiteDefinitionName` is `len 1..256`: both published bounds.
        "shortest legal name".to_string(),
        json!({
            "suiteDefinitionName": "a",
            "rootGroup": "g",
            "devicePermissionRoleArn": ROLE_ARN,
        }),
    ));
    rows.push((
        "longest legal name".to_string(),
        json!({
            "suiteDefinitionName": "n".repeat(256),
            "rootGroup": "g",
            "devicePermissionRoleArn": ROLE_ARN,
        }),
    ));

    rows
}

/// `configuration` as the service STORES it, which is what a definition read answers.
///
/// A suite definition is stored RESOLVED: the optional members a caller left out come back
/// with the values the suite will actually run with (`devices` as an empty list,
/// `intendedForQualification` and `isLongDurationTest` as false, `protocol` as the service's
/// default), and a qualification suite that named an EMPTY `rootGroup` comes back with the
/// default qualification root group. The mirror of `state::resolved_configuration`, so a
/// round-trip assertion states the rule rather than repeating its values row by row.
pub(crate) fn resolved(configuration: &Value) -> Value {
    // The resolved protocol, written out rather than read from the service: plain MQTT 3.1.1
    // is the `Protocol` value a read of a definition stored without one answers with.
    const RESOLVED_PROTOCOL: &str = "MqttV3_1_1";

    let mut expected = configuration.clone();
    let object = expected
        .as_object_mut()
        .expect("a suite definition configuration is a JSON object");
    object.entry("devices").or_insert_with(|| json!([]));
    object
        .entry("intendedForQualification")
        .or_insert_with(|| json!(false));
    object
        .entry("isLongDurationTest")
        .or_insert_with(|| json!(false));
    object
        .entry("protocol")
        .or_insert_with(|| json!(RESOLVED_PROTOCOL));
    if object.get("rootGroup").and_then(Value::as_str) == Some("") {
        // The materialized default root group is taken from the service here because its
        // document SHAPE is the emulator's own choice and may be replaced; what the contract
        // fixes — that a qualification suite reads back a non-empty root group, stable across
        // reads and accepted back on an update — is pinned independently in
        // `tests/all/stored_configuration_and_tag_responses.rs`.
        object.insert(
            "rootGroup".to_string(),
            json!(iotdeviceadvisor::state::DEFAULT_QUALIFICATION_ROOT_GROUP),
        );
    }
    expected
}

/// A suite run configuration for `thing`, with the optional members the table varies.
pub(crate) fn run_configuration(
    thing: &str,
    selected: Option<Vec<&str>>,
    parallel: Option<bool>,
) -> Value {
    let mut configuration = json!({ "primaryDevice": { "thingArn": thing } });
    if let Some(selected) = selected {
        configuration["selectedTestList"] = json!(selected);
    }
    if let Some(parallel) = parallel {
        configuration["parallelRun"] = json!(parallel);
    }
    configuration
}

/// What a create mints is what every later read answers with, for every row of the table:
/// the id, the ARN, the first version label and the configuration as the service stored it.
#[tokio::test]
async fn a_created_definition_is_read_back_verbatim() {
    let emulator = Emulator::new();

    for (case, configuration) in definition_rows() {
        let created = emulator.create_definition(&configuration, "us-east-1").await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateSuiteDefinition, got {} {}",
            created.status, created.body
        );

        let id = created.string("suiteDefinitionId");
        assert!(
            (12..=36).contains(&id.len()),
            "case {case:?}: suiteDefinitionId must satisfy the modelled len 12..36, got {id:?}"
        );
        // The `UUID` shape AWS vends: 8-4-4-4-12 lowercase hex, version 4, variant 10x.
        let groups: Vec<&str> = id.split('-').collect();
        assert_eq!(
            groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12],
            "case {case:?}: suiteDefinitionId must be a UUID, got {id:?}"
        );
        assert!(
            groups
                .iter()
                .all(|group| group.bytes().all(|b| b.is_ascii_hexdigit()
                    && !b.is_ascii_uppercase())),
            "case {case:?}: a UUID is lowercase hex, got {id:?}"
        );
        assert!(
            groups[2].starts_with('4') && groups[3].starts_with(['8', '9', 'a', 'b']),
            "case {case:?}: expected a version-4, RFC 4122 variant UUID, got {id:?}"
        );
        assert_eq!(
            created.string("suiteDefinitionArn"),
            format!("arn:aws:iotdeviceadvisor:us-east-1:000000000000:suitedefinition/{id}"),
            "case {case:?}: unexpected suiteDefinitionArn"
        );
        assert_eq!(
            created.member("suiteDefinitionName"),
            &configuration["suiteDefinitionName"],
            "case {case:?}: suiteDefinitionName must come from the stored configuration"
        );
        assert!(
            created.member("createdAt").is_number(),
            "case {case:?}: expected a createdAt timestamp, got {}",
            created.member("createdAt")
        );

        let fetched = emulator
            .call("GET", &format!("/suiteDefinitions/{id}"), "us-east-1", None)
            .await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected HTTP 200 from GetSuiteDefinition, got {} {}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.member("suiteDefinitionConfiguration"),
            &resolved(&configuration),
            "case {case:?}: GetSuiteDefinition must answer with the stored configuration"
        );
        assert_eq!(
            fetched.string("suiteDefinitionId"),
            id,
            "case {case:?}: GetSuiteDefinition answered another id"
        );
        assert_eq!(
            fetched.string("latestVersion"),
            "v1",
            "case {case:?}: a freshly created definition has exactly one version"
        );
        assert_eq!(
            fetched.string("suiteDefinitionVersion"),
            "v1",
            "case {case:?}: an unqualified read answers the latest version"
        );
        assert_eq!(
            fetched.member("createdAt"),
            created.member("createdAt"),
            "case {case:?}: createdAt must be stable across reads"
        );
        assert!(
            fetched.member("lastModifiedAt").is_number(),
            "case {case:?}: expected a lastModifiedAt timestamp, got {}",
            fetched.member("lastModifiedAt")
        );

        // The version the create minted is addressable by name, and no other is.
        let by_version = emulator
            .call(
                "GET",
                &format!("/suiteDefinitions/{id}?suiteDefinitionVersion=v1"),
                "us-east-1",
                None,
            )
            .await;
        assert_eq!(
            by_version.member("suiteDefinitionConfiguration"),
            &resolved(&configuration),
            "case {case:?}: reading version v1 must answer the stored configuration"
        );
        let missing_version = emulator
            .call(
                "GET",
                &format!("/suiteDefinitions/{id}?suiteDefinitionVersion=v9"),
                "us-east-1",
                None,
            )
            .await;
        assert_eq!(
            (missing_version.status, missing_version.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: an unknown version must be a 404, got {} {}",
            missing_version.status,
            missing_version.body
        );
    }
}

/// The ARN region follows the caller's SigV4 scope, and the ids two creates mint differ.
#[tokio::test]
async fn ids_are_unique_and_arns_carry_the_request_region() {
    let emulator = Emulator::new();
    let configuration = json!({
        "suiteDefinitionName": "region-suite",
        "rootGroup": "g",
        "devicePermissionRoleArn": ROLE_ARN,
    });

    // Regions of different shapes, including the longest AWS publishes.
    for region in ["us-east-1", "eu-central-1", "ap-southeast-4", "il-central-1"] {
        let first = emulator.create_definition(&configuration, region).await;
        let second = emulator.create_definition(&configuration, region).await;
        let (first_id, second_id) = (
            first.string("suiteDefinitionId"),
            second.string("suiteDefinitionId"),
        );
        assert_ne!(
            first_id, second_id,
            "case {region:?}: two creates must mint two definitions"
        );
        assert_eq!(
            first.string("suiteDefinitionArn"),
            format!("arn:aws:iotdeviceadvisor:{region}:000000000000:suitedefinition/{first_id}"),
            "case {region:?}: the ARN must carry the request's region"
        );
    }
}

/// `clientToken` is an `@idempotencyToken`: a retry returns the first definition instead of
/// minting a second one.
#[tokio::test]
async fn a_retried_create_with_the_same_client_token_is_idempotent() {
    let emulator = Emulator::new();
    let body = json!({
        "suiteDefinitionConfiguration": {
            "suiteDefinitionName": "idempotent-suite",
            "rootGroup": "g",
            "devicePermissionRoleArn": ROLE_ARN,
        },
        "clientToken": "token-0001",
    });

    let first = emulator
        .call("POST", "/suiteDefinitions", "us-east-1", Some(body.clone()))
        .await;
    let retry = emulator
        .call("POST", "/suiteDefinitions", "us-east-1", Some(body))
        .await;

    assert_eq!(
        first.string("suiteDefinitionId"),
        retry.string("suiteDefinitionId"),
        "a retry on one clientToken must answer the definition the first call created"
    );
    assert_eq!(
        first.member("createdAt"),
        retry.member("createdAt"),
        "a retry must not re-stamp createdAt"
    );

    let listed = emulator
        .call("GET", "/suiteDefinitions", "us-east-1", None)
        .await;
    assert_eq!(
        listed.member("suiteDefinitionInformationList")
            .as_array()
            .map(Vec::len),
        Some(1),
        "a retried create must leave ONE definition, got {}",
        listed.body
    );

    // The same token offered for a DIFFERENT request is a client mistake, not a retry.
    let mut mutated = json!({
        "suiteDefinitionConfiguration": {
            "suiteDefinitionName": "idempotent-suite-renamed",
            "rootGroup": "g",
            "devicePermissionRoleArn": ROLE_ARN,
        },
        "clientToken": "token-0001",
    });
    let refused = emulator
        .call("POST", "/suiteDefinitions", "us-east-1", Some(mutated.clone()))
        .await;
    assert_eq!(
        (refused.status, refused.error_code.as_deref()),
        (400, Some("ValidationException")),
        "case \"token reused with a different configuration\": expected a 400 \
         ValidationException, got {} {}",
        refused.status,
        refused.body
    );

    // Including when only the tags differ.
    mutated["suiteDefinitionConfiguration"] = json!({
        "suiteDefinitionName": "idempotent-suite",
        "rootGroup": "g",
        "devicePermissionRoleArn": ROLE_ARN,
    });
    mutated["tags"] = json!({ "team": "iot" });
    let refused_tags = emulator
        .call("POST", "/suiteDefinitions", "us-east-1", Some(mutated))
        .await;
    assert_eq!(
        (refused_tags.status, refused_tags.error_code.as_deref()),
        (400, Some("ValidationException")),
        "case \"token reused with different tags\": expected a 400 ValidationException, \
         got {} {}",
        refused_tags.status,
        refused_tags.body
    );

    // A different token for the same configuration is a new definition, not a replay.
    let other_token = emulator
        .call(
            "POST",
            "/suiteDefinitions",
            "us-east-1",
            Some(json!({
                "suiteDefinitionConfiguration": {
                    "suiteDefinitionName": "idempotent-suite",
                    "rootGroup": "g",
                    "devicePermissionRoleArn": ROLE_ARN,
                },
                "clientToken": "token-0002",
            })),
        )
        .await;
    assert_eq!(
        other_token.status, 200,
        "case \"a second token\": expected HTTP 200, got {} {}",
        other_token.status, other_token.body
    );
    assert_ne!(
        other_token.string("suiteDefinitionId"),
        first.string("suiteDefinitionId"),
        "case \"a second token\": a different token must mint a different definition"
    );
}

/// `ListSuiteDefinitions` projects every stored definition and pages with a terminating
/// `nextToken`, for `maxResults` at each published bound and in between.
#[tokio::test]
async fn listing_projects_every_definition_and_pages_to_the_end() {
    let emulator = Emulator::new();

    let mut created: Vec<String> = Vec::new();
    for index in 0..7 {
        let configuration = json!({
            "suiteDefinitionName": format!("page-suite-{index}"),
            "rootGroup": format!("group-{index}"),
            "devicePermissionRoleArn": ROLE_ARN,
            "isLongDurationTest": index % 2 == 0,
            "intendedForQualification": index % 3 == 0,
            "devices": [{ "thingArn": thing_arn(index) }],
        });
        let created_definition = emulator.create_definition(&configuration, "us-east-1").await;
        assert_eq!(
            created_definition.status, 200,
            "setup: create {index} failed with {} {}",
            created_definition.status, created_definition.body
        );
        created.push(created_definition.string("suiteDefinitionId"));
    }
    created.sort();

    // `maxResults` is `range 1..50`: both published bounds, and a size between them.
    for page_size in [1, 2, 3, 50] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(token) => {
                    format!("/suiteDefinitions?maxResults={page_size}&nextToken={token}")
                }
                None => format!("/suiteDefinitions?maxResults={page_size}"),
            };
            let page = emulator.call("GET", &query, "us-east-1", None).await;
            assert_eq!(
                page.status, 200,
                "case maxResults={page_size}: expected HTTP 200, got {} {}",
                page.status, page.body
            );
            let entries = page
                .member("suiteDefinitionInformationList")
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                entries.len() <= page_size,
                "case maxResults={page_size}: a page held {} entries",
                entries.len()
            );
            for entry in entries {
                let id = entry["suiteDefinitionId"]
                    .as_str()
                    .expect("every entry carries a suiteDefinitionId")
                    .to_string();
                let index = created
                    .iter()
                    .position(|created| *created == id)
                    .unwrap_or_else(|| panic!("listing returned an unknown id {id:?}"));
                assert!(
                    entry["suiteDefinitionName"]
                        .as_str()
                        .is_some_and(|name| name.starts_with("page-suite-")),
                    "case maxResults={page_size}: entry {index} must carry the stored name, \
                     got {entry}"
                );
                assert!(
                    entry["createdAt"].is_number(),
                    "case maxResults={page_size}: entry {id} has no createdAt: {entry}"
                );
                assert!(
                    entry["defaultDevices"].is_array(),
                    "case maxResults={page_size}: defaultDevices must come from the \
                     configuration's devices, got {entry}"
                );
                seen.push(id);
            }
            pages += 1;
            assert!(
                pages <= 10,
                "case maxResults={page_size}: paging did not terminate"
            );
            match page.member("nextToken").as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        seen.sort();
        assert_eq!(
            seen, created,
            "case maxResults={page_size}: paging must yield every definition exactly once"
        );
        let expected_pages = created.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case maxResults={page_size}: expected {expected_pages} pages, got {pages}"
        );
    }

    // One past each published bound: the modelled `range 1..50` refuses both.
    for out_of_range in ["0", "51"] {
        let refused = emulator
            .call(
                "GET",
                &format!("/suiteDefinitions?maxResults={out_of_range}"),
                "us-east-1",
                None,
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case maxResults={out_of_range}: a value outside range 1..50 must be refused, \
             got {} {}",
            refused.status, refused.body
        );
    }
}

/// An empty `rootGroup` is legal exactly when the suite is intended for qualification —
/// the rule the model states in prose for `SuiteDefinitionConfiguration$rootGroup`.
#[tokio::test]
async fn an_empty_root_group_is_accepted_only_for_a_qualification_suite() {
    let emulator = Emulator::new();

    // (case, rootGroup, intendedForQualification, expected status)
    let rows: Vec<(&str, &str, Option<bool>, u16)> = vec![
        ("empty rootGroup, qualification", "", Some(true), 200),
        ("empty rootGroup, not qualification", "", Some(false), 400),
        ("empty rootGroup, flag absent", "", None, 400),
        ("non-empty rootGroup, qualification", "g", Some(true), 200),
        ("non-empty rootGroup, not qualification", "g", Some(false), 200),
        ("non-empty rootGroup, flag absent", "g", None, 200),
    ];

    for (case, root_group, qualification, expected) in rows {
        let mut configuration = json!({
            "suiteDefinitionName": format!("root-group-{case}"),
            "rootGroup": root_group,
            "devicePermissionRoleArn": ROLE_ARN,
        });
        if let Some(qualification) = qualification {
            configuration["intendedForQualification"] = json!(qualification);
        }
        let answer = emulator.create_definition(&configuration, "us-east-1").await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} {}",
            answer.status, answer.body
        );
        if expected == 400 {
            assert_eq!(
                answer.error_code.as_deref(),
                Some("ValidationException"),
                "case {case:?}: expected a ValidationException, got {}",
                answer.body
            );
        }
    }

    let long_root_group = "g".repeat(2048);
    let answer = emulator
        .create_definition(
            &json!({
                "suiteDefinitionName": "long-root-group",
                "rootGroup": long_root_group,
                "devicePermissionRoleArn": ROLE_ARN,
            }),
            "us-east-1",
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case \"rootGroup at the modelled maximum\": expected HTTP 200, got {} {}",
        answer.status, answer.body
    );
}

/// A read of something the store does not hold is a 404, and a malformed id is a 400 —
/// never a 501 and never a success.
#[tokio::test]
async fn unknown_and_malformed_ids_are_refused() {
    let emulator = Emulator::new();
    let known = emulator
        .create_definition(
            &json!({
                "suiteDefinitionName": "refusal-suite",
                "rootGroup": "g",
                "devicePermissionRoleArn": ROLE_ARN,
            }),
            "us-east-1",
        )
        .await;
    let known_id = known.string("suiteDefinitionId");

    // (case, path, expected status, expected error code)
    let rows: Vec<(String, String, u16, &str)> = vec![
        (
            "unknown definition".to_string(),
            "/suiteDefinitions/11111111-2222-3333-4444-555555555555".to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "unknown definition at the shortest legal id length".to_string(),
            "/suiteDefinitions/abcdefghijkl".to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "unknown run under a known definition".to_string(),
            format!("/suiteDefinitions/{known_id}/suiteRuns/11111111-2222-3333-4444-555555555555"),
            404,
            "ResourceNotFoundException",
        ),
        (
            // `UUID` is `len 12..36`: one character short of the lower bound.
            "id below the modelled minimum length".to_string(),
            "/suiteDefinitions/abcdefghijk".to_string(),
            400,
            "ValidationException",
        ),
        (
            // One character past the upper bound.
            "id above the modelled maximum length".to_string(),
            format!("/suiteDefinitions/{}", "a".repeat(37)),
            400,
            "ValidationException",
        ),
    ];

    for (case, path, expected_status, expected_code) in rows {
        let answer = emulator.call("GET", &path, "us-east-1", None).await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some(expected_code),
            "case {case:?}: expected error code {expected_code}, got {:?} {}",
            answer.error_code,
            answer.body
        );
    }
}

/// A suite run is minted, readable, stoppable, and its status moves only on the clock.
#[tokio::test]
async fn a_suite_run_is_persisted_and_its_status_moves_on_the_clock() {
    let emulator = Emulator::new();

    // The run configurations differ in the optional members and in list size; each row
    // carries the thing ARN its primary device is named by, so the endpoint the start call
    // reports can be checked against that device.
    let rows: Vec<(&str, String, Value)> = vec![
        (
            "primary device only",
            thing_arn(0),
            run_configuration(&thing_arn(0), None, None),
        ),
        (
            "empty selected test list",
            thing_arn(1),
            run_configuration(&thing_arn(1), Some(Vec::new()), Some(false)),
        ),
        (
            "several selected tests, parallel",
            thing_arn(2),
            run_configuration(
                &thing_arn(2),
                // `SelectedTestList` members are `UUID` (len 12..36).
                Some(vec![
                    "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
                    "11111111-2222-3333-4444-555555555555",
                    "testcasedef1",
                ]),
                Some(true),
            ),
        ),
    ];

    for (case, primary_thing_arn, configuration) in rows {
        let definition = emulator
            .create_definition(
                &json!({
                    "suiteDefinitionName": format!("run-suite-{case}"),
                    "rootGroup": "g",
                    "devicePermissionRoleArn": ROLE_ARN,
                    "devices": [{ "thingArn": thing_arn(0) }],
                }),
                "us-west-2",
            )
            .await;
        let definition_id = definition.string("suiteDefinitionId");

        let started = emulator
            .call(
                "POST",
                &format!("/suiteDefinitions/{definition_id}/suiteRuns"),
                "us-west-2",
                Some(json!({ "suiteRunConfiguration": configuration })),
            )
            .await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected HTTP 200 from StartSuiteRun, got {} {}",
            started.status, started.body
        );
        let run_id = started.string("suiteRunId");
        assert!(
            (12..=36).contains(&run_id.len()),
            "case {case:?}: suiteRunId must satisfy the modelled len 12..36, got {run_id:?}"
        );
        assert_eq!(
            started.string("suiteRunArn"),
            format!(
                "arn:aws:iotdeviceadvisor:us-west-2:000000000000:suiterun/{definition_id}/{run_id}"
            ),
            "case {case:?}: unexpected suiteRunArn"
        );
        let endpoint = started.string("endpoint");
        assert!(
            (45..=75).contains(&endpoint.len()),
            "case {case:?}: endpoint must satisfy the modelled len 45..75, got {endpoint:?} \
             ({} chars)",
            endpoint.len()
        );
        // The run names a primary device, so the host it reports is that DEVICE's host —
        // the one `GetEndpoint` vends over the wire for the same ARN — and not the
        // account-level host, which belongs to a caller that names no device.
        let vended = emulator
            .call(
                "GET",
                &format!(
                    "/endpoint?thingArn={}",
                    crate::endpoint::percent_encode(&primary_thing_arn)
                ),
                "us-west-2",
                None,
            )
            .await
            .string("endpoint");
        assert_eq!(
            endpoint, vended,
            "case {case:?}: the run must report the host GetEndpoint vends for its primary \
             device, got {endpoint:?} and {vended:?}"
        );
        let account_level = emulator
            .call("GET", "/endpoint", "us-west-2", None)
            .await
            .string("endpoint");
        assert_ne!(
            endpoint, account_level,
            "case {case:?}: a run naming a primary device must not report the \
             account-level host, got {endpoint:?}"
        );

        let run_path = format!("/suiteDefinitions/{definition_id}/suiteRuns/{run_id}");
        let pending = emulator.call("GET", &run_path, "us-west-2", None).await;
        assert_eq!(
            pending.status, 200,
            "case {case:?}: expected HTTP 200 from GetSuiteRun, got {} {}",
            pending.status, pending.body
        );
        assert_eq!(
            pending.string("status"),
            "PENDING",
            "case {case:?}: a run read straight after the start call is still PENDING"
        );
        assert_eq!(
            pending.member("suiteRunConfiguration"),
            &configuration,
            "case {case:?}: GetSuiteRun must answer with the stored suiteRunConfiguration"
        );
        assert_eq!(
            pending.string("suiteDefinitionVersion"),
            "v1",
            "case {case:?}: the run records the definition version it runs"
        );
        assert!(
            pending.member("endTime").is_null(),
            "case {case:?}: a non-terminal run has no endTime, got {}",
            pending.body
        );
        assert!(
            pending.member("testResult").is_null(),
            "case {case:?}: no test case ran, so there is no testResult to report, got {}",
            pending.body
        );

        // A second run while one is in progress is refused unless the configuration asks
        // for a parallel run; `parallel_runs` asserts that member's rule over its own
        // table, so here each row is only pinned to the answer ITS configuration earns.
        let second = emulator
            .call(
                "POST",
                &format!("/suiteDefinitions/{definition_id}/suiteRuns"),
                "us-west-2",
                Some(json!({ "suiteRunConfiguration": configuration })),
            )
            .await;
        if configuration["parallelRun"] == json!(true) {
            assert_eq!(
                second.status, 200,
                "case {case:?}: a second run asking for a parallel run, alongside one that \
                 also asked for one, must be accepted, got {} {}",
                second.status, second.body
            );
        } else {
            assert_eq!(
                (second.status, second.error_code.as_deref()),
                (400, Some("ConflictException")),
                "case {case:?}: a second non-parallel run while one is in progress must \
                 conflict, got {} {}",
                second.status,
                second.body
            );
        }

        // The clock, not the poll, moves the status: PENDING -> RUNNING -> ERROR, because
        // this emulator has no device test runner to execute the suite with.
        tokio::time::sleep(span() * 3 / 2).await;
        let running = emulator.call("GET", &run_path, "us-west-2", None).await;
        assert_eq!(
            running.string("status"),
            "RUNNING",
            "case {case:?}: one transition span after the start the run is RUNNING, got {}",
            running.body
        );
        assert!(
            running.member("startTime").is_number(),
            "case {case:?}: a run that left PENDING carries a startTime, got {}",
            running.body
        );

        tokio::time::sleep(span()).await;
        let terminal = emulator.call("GET", &run_path, "us-west-2", None).await;
        assert_eq!(
            terminal.string("status"),
            "ERROR",
            "case {case:?}: a run nothing could execute ends in the modelled failure \
             status, got {}",
            terminal.body
        );
        assert!(
            terminal.member("endTime").is_number(),
            "case {case:?}: a terminal run carries an endTime, got {}",
            terminal.body
        );
        assert_eq!(
            terminal.member("errorReason").as_str(),
            Some(iotdeviceadvisor::state::NO_TEST_RUNNER_REASON),
            "case {case:?}: the failure must say why nothing ran, got {}",
            terminal.body
        );
        assert!(
            terminal.member("testResult").is_null(),
            "case {case:?}: no test case ran, so no result is reported, got {}",
            terminal.body
        );

        // A terminal run has released the definition's slot.
        let next_run = emulator
            .call(
                "POST",
                &format!("/suiteDefinitions/{definition_id}/suiteRuns"),
                "us-west-2",
                Some(json!({ "suiteRunConfiguration": configuration })),
            )
            .await;
        assert_eq!(
            next_run.status, 200,
            "case {case:?}: once no run is in progress the next start is accepted, got {} {}",
            next_run.status, next_run.body
        );
        assert_ne!(
            next_run.string("suiteRunId"),
            run_id,
            "case {case:?}: a second start must mint a second run"
        );
    }
}

/// Stopping moves the stored run, a following read observes it, and a terminal run is
/// never resurrected.
#[tokio::test]
async fn stopping_a_run_is_observable_and_never_resurrects_a_terminal_run() {
    let emulator = Emulator::new();
    let definition = emulator
        .create_definition(
            &json!({
                "suiteDefinitionName": "stop-suite",
                "rootGroup": "g",
                "devicePermissionRoleArn": ROLE_ARN,
            }),
            "us-east-1",
        )
        .await;
    let definition_id = definition.string("suiteDefinitionId");
    let configuration = run_configuration(&thing_arn(3), None, None);

    let started = emulator
        .call(
            "POST",
            &format!("/suiteDefinitions/{definition_id}/suiteRuns"),
            "us-east-1",
            Some(json!({ "suiteRunConfiguration": configuration })),
        )
        .await;
    let run_id = started.string("suiteRunId");
    let run_path = format!("/suiteDefinitions/{definition_id}/suiteRuns/{run_id}");

    let stopped = emulator
        .call("POST", &format!("{run_path}/stop"), "us-east-1", None)
        .await;
    assert_eq!(
        stopped.status, 200,
        "case \"stop an in-progress run\": expected HTTP 200, got {} {}",
        stopped.status, stopped.body
    );

    let stopping = emulator.call("GET", &run_path, "us-east-1", None).await;
    assert_eq!(
        stopping.string("status"),
        "STOPPING",
        "case \"read straight after the stop\": expected STOPPING, got {}",
        stopping.body
    );

    // A run that is STOPPING is still in progress, and neither it nor this request asked
    // for a parallel run, so it still holds the definition: the stop must complete first.
    let while_stopping = emulator
        .call(
            "POST",
            &format!("/suiteDefinitions/{definition_id}/suiteRuns"),
            "us-east-1",
            Some(json!({ "suiteRunConfiguration": configuration })),
        )
        .await;
    assert_eq!(
        (while_stopping.status, while_stopping.error_code.as_deref()),
        (400, Some("ConflictException")),
        "case \"start while the previous run is STOPPING\": expected a 400 ConflictException, \
         got {} {}",
        while_stopping.status,
        while_stopping.body
    );

    tokio::time::sleep(span() * 3 / 2).await;
    let finished = emulator.call("GET", &run_path, "us-east-1", None).await;
    assert_eq!(
        finished.string("status"),
        "STOPPED",
        "case \"one span after the stop\": expected STOPPED, got {}",
        finished.body
    );
    let end_time = finished.member("endTime").clone();
    assert!(
        end_time.is_number(),
        "case \"a stopped run\": expected an endTime, got {}",
        finished.body
    );

    // Stopping again changes nothing: the run stays STOPPED with the endTime it reached.
    let restop = emulator
        .call("POST", &format!("{run_path}/stop"), "us-east-1", None)
        .await;
    assert_eq!(
        restop.status, 200,
        "case \"stop a terminal run\": expected HTTP 200, got {} {}",
        restop.status, restop.body
    );
    let after = emulator.call("GET", &run_path, "us-east-1", None).await;
    assert_eq!(
        (after.string("status"), after.member("endTime").clone()),
        ("STOPPED".to_string(), end_time),
        "case \"stop a terminal run\": a terminal run must not be resurrected, got {}",
        after.body
    );

    // Stopping a run nobody started is a 404.
    let unknown = emulator
        .call(
            "POST",
            &format!(
                "/suiteDefinitions/{definition_id}/suiteRuns/\
                 99999999-8888-7777-6666-555555555555/stop"
            ),
            "us-east-1",
            None,
        )
        .await;
    assert_eq!(
        (unknown.status, unknown.error_code.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case \"stop an unknown run\": expected a 404, got {} {}",
        unknown.status,
        unknown.body
    );
}

/// Starting a run on a definition the store does not hold is refused with the modelled
/// client-input error this operation declares (`smithy_query` lists no
/// `ResourceNotFoundException` for `StartSuiteRun`).
#[tokio::test]
async fn starting_a_run_on_an_unknown_definition_is_refused() {
    let emulator = Emulator::new();
    let configuration = run_configuration(&thing_arn(4), None, None);

    let rows: Vec<(&str, String)> = vec![
        (
            "unknown definition",
            "/suiteDefinitions/11111111-2222-3333-4444-555555555555/suiteRuns".to_string(),
        ),
        (
            "unknown definition at the shortest legal id length",
            "/suiteDefinitions/abcdefghijkl/suiteRuns".to_string(),
        ),
    ];

    for (case, path) in rows {
        let answer = emulator
            .call(
                "POST",
                &path,
                "us-east-1",
                Some(json!({ "suiteRunConfiguration": configuration })),
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected a 400 ValidationException, got {} {}",
            answer.status,
            answer.body
        );
    }

    // A version the definition never had is refused the same way.
    let definition = emulator
        .create_definition(
            &json!({
                "suiteDefinitionName": "version-suite",
                "rootGroup": "g",
                "devicePermissionRoleArn": ROLE_ARN,
            }),
            "us-east-1",
        )
        .await;
    let definition_id = definition.string("suiteDefinitionId");
    let answer = emulator
        .call(
            "POST",
            &format!("/suiteDefinitions/{definition_id}/suiteRuns"),
            "us-east-1",
            Some(json!({
                "suiteRunConfiguration": configuration,
                "suiteDefinitionVersion": "v7",
            })),
        )
        .await;
    assert_eq!(
        (answer.status, answer.error_code.as_deref()),
        (400, Some("ValidationException")),
        "case \"unknown suiteDefinitionVersion\": expected a 400 ValidationException, got {} {}",
        answer.status,
        answer.body
    );
}

/// A delete removes the definition and its runs, and is idempotent.
#[tokio::test]
async fn a_deleted_definition_and_its_runs_are_gone() {
    let emulator = Emulator::new();

    let mut ids = Vec::new();
    for index in 0..3 {
        let created = emulator
            .create_definition(
                &json!({
                    "suiteDefinitionName": format!("delete-suite-{index}"),
                    "rootGroup": "g",
                    "devicePermissionRoleArn": ROLE_ARN,
                }),
                "us-east-1",
            )
            .await;
        ids.push(created.string("suiteDefinitionId"));
    }

    let doomed = ids[1].clone();
    let started = emulator
        .call(
            "POST",
            &format!("/suiteDefinitions/{doomed}/suiteRuns"),
            "us-east-1",
            Some(json!({
                "suiteRunConfiguration": run_configuration(&thing_arn(5), None, None),
            })),
        )
        .await;
    let run_id = started.string("suiteRunId");

    let deleted = emulator
        .call("DELETE", &format!("/suiteDefinitions/{doomed}"), "us-east-1", None)
        .await;
    assert_eq!(
        deleted.status, 200,
        "case \"delete a definition\": expected HTTP 200, got {} {}",
        deleted.status, deleted.body
    );

    let gone = emulator
        .call("GET", &format!("/suiteDefinitions/{doomed}"), "us-east-1", None)
        .await;
    assert_eq!(
        (gone.status, gone.error_code.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case \"read a deleted definition\": expected a 404, got {} {}",
        gone.status,
        gone.body
    );

    let orphan = emulator
        .call(
            "GET",
            &format!("/suiteDefinitions/{doomed}/suiteRuns/{run_id}"),
            "us-east-1",
            None,
        )
        .await;
    assert_eq!(
        (orphan.status, orphan.error_code.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case \"read a run of a deleted definition\": expected a 404, got {} {}",
        orphan.status,
        orphan.body
    );

    let listed = emulator
        .call("GET", "/suiteDefinitions", "us-east-1", None)
        .await;
    let remaining: Vec<String> = listed
        .member("suiteDefinitionInformationList")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| entry["suiteDefinitionId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        !remaining.contains(&doomed),
        "case \"list after a delete\": the deleted definition is still listed: {}",
        listed.body
    );
    assert_eq!(
        remaining.len(),
        2,
        "case \"list after a delete\": the other definitions must survive, got {}",
        listed.body
    );

    // `DeleteSuiteDefinition` models no not-found error, so deleting again succeeds.
    let again = emulator
        .call("DELETE", &format!("/suiteDefinitions/{doomed}"), "us-east-1", None)
        .await;
    assert_eq!(
        again.status, 200,
        "case \"delete twice\": a delete of an absent definition still succeeds, got {} {}",
        again.status, again.body
    );
}

/// Tags given to a create are stored and echoed by the reads, over a table of tag-map
/// sizes (`TagMap` is `len 0..50`: no tags, one, and the modelled maximum).
#[tokio::test]
async fn tags_round_trip_through_the_reads() {
    let emulator = Emulator::new();

    let mut rows: Vec<(String, Value)> = vec![
        ("no tags".to_string(), json!({})),
        ("one tag".to_string(), json!({ "team": "iot" })),
        (
            "three tags".to_string(),
            json!({ "team": "iot", "env": "test", "owner": "device-advisor" }),
        ),
    ];
    let mut fifty = serde_json::Map::new();
    for index in 0..50 {
        fifty.insert(format!("key-{index}"), json!(format!("value-{index}")));
    }
    rows.push(("fifty tags".to_string(), Value::Object(fifty)));

    for (case, tags) in rows {
        let configuration = json!({
            "suiteDefinitionName": format!("tagged-suite-{case}"),
            "rootGroup": "g",
            "devicePermissionRoleArn": ROLE_ARN,
        });
        let created = emulator
            .call(
                "POST",
                "/suiteDefinitions",
                "us-east-1",
                Some(json!({
                    "suiteDefinitionConfiguration": configuration,
                    "tags": tags,
                })),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200, got {} {}",
            created.status, created.body
        );
        let id = created.string("suiteDefinitionId");

        let fetched = emulator
            .call("GET", &format!("/suiteDefinitions/{id}"), "us-east-1", None)
            .await;
        // `TagMap` is `len 0..50`, so an untagged resource reads as an EMPTY map, never as
        // an absent member.
        let expected = tags.clone();
        assert_eq!(
            fetched.member("tags"),
            &expected,
            "case {case:?}: GetSuiteDefinition must echo the stored tags, got {}",
            fetched.body
        );

        let started = emulator
            .call(
                "POST",
                &format!("/suiteDefinitions/{id}/suiteRuns"),
                "us-east-1",
                Some(json!({
                    "suiteRunConfiguration": run_configuration(&thing_arn(6), None, None),
                    "tags": tags,
                })),
            )
            .await;
        let run_id = started.string("suiteRunId");
        let run = emulator
            .call(
                "GET",
                &format!("/suiteDefinitions/{id}/suiteRuns/{run_id}"),
                "us-east-1",
                None,
            )
            .await;
        assert_eq!(
            run.member("tags"),
            &expected,
            "case {case:?}: GetSuiteRun must echo the stored tags, got {}",
            run.body
        );
    }
}

/// The store is bounded: a creation past the definition cap is refused, and a definition
/// run over and over retains only its most recent runs.
#[tokio::test]
async fn the_store_is_bounded_in_definitions_and_in_retained_runs() {
    let emulator = Emulator::new();
    let cap = iotdeviceadvisor::state::MAX_SUITE_DEFINITIONS;

    let configuration = |index: usize| {
        json!({
            "suiteDefinitionName": format!("bounded-suite-{index}"),
            "rootGroup": "g",
            "devicePermissionRoleArn": ROLE_ARN,
        })
    };

    let mut first_id = String::new();
    for index in 0..cap {
        let created = emulator
            .create_definition(&configuration(index), "us-east-1")
            .await;
        assert_eq!(
            created.status, 200,
            "case \"definition {index} of the cap\": expected HTTP 200, got {} {}",
            created.status, created.body
        );
        if index == 0 {
            first_id = created.string("suiteDefinitionId");
        }
    }

    let refused = emulator.create_definition(&configuration(cap), "us-east-1").await;
    assert_eq!(
        (refused.status, refused.error_code.as_deref()),
        (400, Some("ValidationException")),
        "case \"one definition past the cap\": expected a 400 ValidationException, got {} {}",
        refused.status,
        refused.body
    );

    // Deleting one makes room again: the cap bounds occupancy, it is not a lifetime quota.
    let deleted = emulator
        .call("DELETE", &format!("/suiteDefinitions/{first_id}"), "us-east-1", None)
        .await;
    assert_eq!(
        deleted.status, 200,
        "case \"delete to make room\": expected HTTP 200, got {} {}",
        deleted.status, deleted.body
    );
    let accepted = emulator.create_definition(&configuration(cap), "us-east-1").await;
    assert_eq!(
        accepted.status, 200,
        "case \"create after a delete\": expected HTTP 200, got {} {}",
        accepted.status, accepted.body
    );
    let (definitions, _) = emulator.stored_counts();
    assert_eq!(
        definitions, cap,
        "case \"store occupancy at the cap\": expected {cap} definitions, got {definitions}"
    );

    // Run retention: a definition whose runs have piled up keeps only the most recent
    // ones. The finished runs are seeded straight into the store (the per-definition
    // conflict rule serialises runs, so driving 51 of them over HTTP would mean waiting
    // out a transition span each time), and then ONE StartSuiteRun over the wire has to
    // bring occupancy back inside the bound. A fresh emulator, so the definition cap
    // exercised above does not stand in the way.
    let emulator = Emulator::new();
    let retained = iotdeviceadvisor::state::MAX_RUNS_PER_DEFINITION;
    let definition = emulator
        .create_definition(
            &json!({
                "suiteDefinitionName": "retention-suite",
                "rootGroup": "g",
                "devicePermissionRoleArn": ROLE_ARN,
            }),
            "us-east-1",
        )
        .await;
    assert_eq!(
        definition.status, 200,
        "setup: the retention definition must be created, got {} {}",
        definition.status, definition.body
    );
    let definition_id = definition.string("suiteDefinitionId");

    let seeded = emulator.seed_finished_runs(&definition_id, retained + 5);
    let (_, before) = emulator.stored_counts();
    assert_eq!(
        before,
        retained + 5,
        "setup: expected {} seeded runs in the store, got {before}",
        retained + 5
    );

    let started = emulator
        .call(
            "POST",
            &format!("/suiteDefinitions/{definition_id}/suiteRuns"),
            "us-east-1",
            Some(json!({
                "suiteRunConfiguration": run_configuration(&thing_arn(9), None, None),
            })),
        )
        .await;
    assert_eq!(
        started.status, 200,
        "case \"start a run past the retention count\": expected HTTP 200, got {} {}",
        started.status, started.body
    );
    let newest_id = started.string("suiteRunId");

    // The bound is on the STORE, not just on what a read answers.
    let (definitions, runs) = emulator.stored_counts();
    assert_eq!(
        definitions, 1,
        "case \"stored definitions in the retention case\": expected 1, got {definitions}"
    );
    assert_eq!(
        runs, retained,
        "case \"stored runs after a start past the retention count\": the store must retain \
         exactly {retained}, got {runs}"
    );
    let (_, tag_rows) = emulator.stored_tag_counts();
    assert_eq!(
        tag_rows,
        retained - 1,
        "case \"tag rows after retention\": a dropped run must take its tag row with it, so \
         only the {} surviving SEEDED runs keep one (the run just started carries no tags), \
         got {tag_rows}",
        retained - 1
    );

    // The oldest seeded run is aged out; the newest, and the one just started, are not.
    let oldest = emulator
        .call(
            "GET",
            &format!("/suiteDefinitions/{definition_id}/suiteRuns/{}", seeded[0]),
            "us-east-1",
            None,
        )
        .await;
    assert_eq!(
        (oldest.status, oldest.error_code.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case \"the oldest run past the retention count\": it must have been aged out, got {} {}",
        oldest.status,
        oldest.body
    );
    let newest_seeded = emulator
        .call(
            "GET",
            &format!(
                "/suiteDefinitions/{definition_id}/suiteRuns/{}",
                seeded.last().expect("runs were seeded")
            ),
            "us-east-1",
            None,
        )
        .await;
    assert_eq!(
        newest_seeded.status, 200,
        "case \"the newest seeded run\": it must still be readable, got {} {}",
        newest_seeded.status, newest_seeded.body
    );
    let newest = emulator
        .call(
            "GET",
            &format!("/suiteDefinitions/{definition_id}/suiteRuns/{newest_id}"),
            "us-east-1",
            None,
        )
        .await;
    assert_eq!(
        newest.status, 200,
        "case \"the run just started\": it must be readable, got {} {}",
        newest.status, newest.body
    );
}

/// The status-transition span this service promises callers, as the state module fixes it.
fn span() -> Duration {
    iotdeviceadvisor::state::STATUS_TRANSITION
}
