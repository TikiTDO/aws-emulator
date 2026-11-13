// SPDX-License-Identifier: CC0-1.0

//! The flow lifecycle, end to end over HTTP: CreateFlow -> DescribeFlow -> ListFlows ->
//! StartFlow -> StopFlow -> DeleteFlow.
//!
//! Every case drives the real request path (routing, restJson1 deserialization,
//! constraint validation, handler, serialization) through the same
//! `MultiServiceRouter` the emulator's front door uses, so what these tests assert is
//! what a MediaConnect client sees on the wire.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Every ARN CreateFlow mints must satisfy the modelled `FlowArn` constraint
/// (`smithy_query(query="shape",shape="FlowArn")`:
/// `^arn:(aws[a-zA-Z-]*):mediaconnect:[a-z0-9-]+:[0-9]{12}:flow:[a-zA-Z0-9-]+:[a-zA-Z0-9_-]+$`),
/// because DescribeFlow / StartFlow / StopFlow / DeleteFlow take it as an `@httpLabel`
/// and the framework validates the member against that pattern.
///
/// The check runs through the crate's own encoding of the constraint — the same
/// function ListFlows validates a pagination token with — and then re-asserts, field by
/// field, the parts the pattern fixes, so a bug in that function cannot hide here.
fn assert_minted_flow_arn(case: &str, arn: &str, name: &str) {
    assert!(
        service_com_amazonaws_mediaconnect_mediaconnect::state::is_mintable_flow_arn(arn),
        "case {case:?}: minted FlowArn must satisfy the modelled pattern, got {arn:?}"
    );
    let segments: Vec<&str> = arn.split(':').collect();
    assert_eq!(
        segments.len(),
        8,
        "case {case:?}: a flow ARN has 8 colon-separated segments, got {arn:?}"
    );
    assert_eq!(
        (segments[0], segments[2], segments[3], segments[5]),
        ("arn", "mediaconnect", REGION, "flow"),
        "case {case:?}: ARN prefix, service, Region and resource type, got {arn:?}"
    );
    assert!(
        segments[1].starts_with("aws"),
        "case {case:?}: the partition is an aws partition, got {arn:?}"
    );
    assert!(
        segments[4].len() == 12 && segments[4].bytes().all(|b| b.is_ascii_digit()),
        "case {case:?}: the account id is 12 digits, got {arn:?}"
    );
    assert_eq!(
        segments[7], name,
        "case {case:?}: the last segment is the flow name, got {arn:?}"
    );
}

/// The Region whose SigV4 credential scope [`Emulator::send`] signs every request for, and
/// so the Region every ARN below is minted in.
///
/// `pub(crate)` because the other suites assert the same Region in the ARNs they read
/// back.
pub(crate) const REGION: &str = "us-east-1";

/// One emulator: a router with only this service mounted on one shared engine.
///
/// Other suites in this test crate drive the same front door through it (see
/// `tests/all/flow_outputs.rs`, `tests/all/update_flow.rs` and
/// `tests/all/flow_entitlements.rs`, `tests/all/flow_identity_and_description.rs` and
/// `tests/all/gateway_lifecycle.rs`), so there is one harness rather than one per file.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    /// The same engine the mounted service holds, so a suite that needs a particular
    /// store contents can set it up without paying a request per record.
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not the subject here; the requests below carry a scope, not a
        // real signature.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_mediaconnect_mediaconnect::EmulatedService::new(engine.clone()),
        );
        Self { router, engine }
    }

    /// As [`Emulator::new`], with Amazon EC2 mounted beside this service on the SAME engine,
    /// and the in-process callback a suite drives EC2's own operations through.
    ///
    /// A VPC placement names EC2's resources — the subnet an elastic network interface goes
    /// in and the security groups it carries — so the suite covering it needs the owner of
    /// those resources answering beside this service, over the very callback the handlers
    /// resolve a placement through.
    pub(crate) fn with_ec2() -> (Self, ::smithy_server_runtime::EmbeddedEmulator) {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_mediaconnect_mediaconnect::EmulatedService::new(engine.clone()),
        );
        router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
            engine.clone(),
        ));
        let callback = ::smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
        (Self { router, engine }, callback)
    }

    /// Put `count` flows straight into the store, the way CreateFlow stores one.
    ///
    /// The store ceiling is a bound on what is STORED, so the suite that asserts it
    /// needs a full store rather than thousands of round trips: the calls that matter
    /// (the one that fills the last slot, the one past the bound) still go over the wire.
    fn seed_flows(&self, count: usize) {
        use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

        let state = self
            .engine
            .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
        let mut store = state.lock();
        for _ in 0..count {
            let suffix = store.next_suffix();
            let name = format!("seeded-flow-{suffix}");
            let flow_arn = engine::emulator_arn(
                mc::SERVICE_NAMESPACE,
                REGION,
                &mc::flow_resource_fragment(suffix, &name),
            );
            store.flows.insert(
                flow_arn.clone(),
                mc::StoredFlow {
                    flow_arn,
                    name: name.clone(),
                    description: mc::derived_flow_description(&name, &name, None),
                    availability_zone: format!("{REGION}a"),
                    status: mc::STATUS_STANDBY.to_string(),
                    pending: None,
                    egress_ip: Some(mc::assigned_egress_ip(suffix)),
                    flow_size: None,
                    maintenance: None,
                    source_failover_config: None,
                    source_monitoring_config: None,
                    ndi_config: None,
                    encoding_config: None,
                    sources: vec![mc::StoredSource {
                        name,
                        ingest_ip: Some(mc::assigned_ingest_ip(suffix)),
                        ingest_port: Some(mc::assigned_ingest_port(suffix)),
                        ..mc::StoredSource::default()
                    }],
                    outputs: Vec::new(),
                    entitlements: Vec::new(),
                    media_streams: Vec::new(),
                    vpc_interfaces: Vec::new(),
                    tags: BTreeMap::new(),
                    created_at: SystemTime::now(),
                },
            );
        }
    }

    /// Send one request and return `(status, error code, body)`. The error code is the
    /// restJson1 `x-amzn-errortype` header, absent on success.
    ///
    /// The request is signed for [`REGION`]; a suite that needs another signing Region
    /// calls [`Self::send_signed_for`].
    pub(crate) async fn send(
        &self,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (u16, Option<String>, Value) {
        self.send_signed_for(REGION, method, uri, body).await
    }

    /// The same request [`Self::send`] builds, but with its SigV4 credential scope naming
    /// `region`.
    ///
    /// The credential scope is where
    /// `service_com_amazonaws_mediaconnect_mediaconnect::state::request_region` reads "the
    /// current region" from, so this is what lets a suite assert the Region a create derives
    /// for a request that names none — distinguished from the fallback
    /// `state::DEFAULT_REGION`, which [`REGION`] happens to equal.
    pub(crate) async fn send_signed_for(
        &self,
        region: &str,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (u16, Option<String>, Value) {
        let payload = body
            .map(|value| bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")))
            .unwrap_or_default();
        let authorization = format!(
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLEEXAMPLE/20260101/{region}/mediaconnect/aws4_request, \
             SignedHeaders=host, Signature=deadbeef"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .header("host", "mediaconnect.us-east-1.amazonaws.com")
            .header("authorization", authorization)
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, code, parsed)
    }

    pub(crate) async fn create_flow(&self, body: Value) -> (u16, Option<String>, Value) {
        self.send("POST", "/v1/flows", Some(body)).await
    }

    pub(crate) async fn describe_flow(&self, flow_arn: &str) -> (u16, Option<String>, Value) {
        self.send("GET", &format!("/v1/flows/{flow_arn}"), None).await
    }

    pub(crate) async fn list_flows(&self, query: &str) -> (u16, Option<String>, Value) {
        self.send("GET", &format!("/v1/flows{query}"), None).await
    }
}

/// The `Flows` member of a ListFlows response, as `(FlowArn, Name, Status, SourceType)`.
fn listed(body: &Value) -> Vec<(String, String, String, String)> {
    body["flows"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|flow| {
            (
                flow["flowArn"].as_str().unwrap_or_default().to_string(),
                flow["name"].as_str().unwrap_or_default().to_string(),
                flow["status"].as_str().unwrap_or_default().to_string(),
                flow["sourceType"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// One row of the CreateFlow table: a request body, and what the class rule says the
/// stored flow must look like for it.
struct CreateCase {
    case: &'static str,
    body: Value,
    sources: usize,
    outputs: usize,
    source_type: &'static str,
    availability_zone: &'static str,
}

fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            case: "one rtp source, no outputs, no availability zone",
            body: json!({
                "name": "flow-rtp",
                "source": { "name": "src1", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
            }),
            sources: 1,
            outputs: 0,
            source_type: "OWNED",
            availability_zone: "us-east-1a",
        },
        CreateCase {
            case: "srt-listener source with an explicit ingest port and one output",
            body: json!({
                "name": "flow_srt",
                "availabilityZone": "us-east-1c",
                "source": {
                    "name": "srt-in",
                    "protocol": "srt-listener",
                    "whitelistCidr": "0.0.0.0/0",
                    "ingestPort": 9001,
                    "minLatency": 120
                },
                "outputs": [
                    { "name": "out-a", "protocol": "zixi-push", "destination": "198.51.100.9", "port": 2088 }
                ]
            }),
            sources: 1,
            outputs: 1,
            source_type: "OWNED",
            availability_zone: "us-east-1c",
        },
        CreateCase {
            case: "failover pair of sources with two outputs",
            body: json!({
                "name": "flow-failover",
                "sources": [
                    { "name": "primary", "protocol": "zixi-push", "whitelistCidr": "10.1.0.0/16", "maxBitrate": 80_000_000 },
                    { "name": "secondary", "protocol": "zixi-push", "whitelistCidr": "10.2.0.0/16", "maxLatency": 2000 }
                ],
                "sourceFailoverConfig": { "state": "ENABLED", "failoverMode": "FAILOVER", "recoveryWindow": 3000 },
                "outputs": [
                    { "name": "out-1", "protocol": "rtp-fec", "destination": "203.0.113.10", "port": 5000 },
                    { "name": "out-2", "protocol": "rist", "destination": "203.0.113.11", "port": 5002, "outputStatus": "DISABLED" }
                ]
            }),
            sources: 2,
            outputs: 2,
            source_type: "OWNED",
            availability_zone: "us-east-1a",
        },
        CreateCase {
            case: "entitled source has no ingest endpoint of its own",
            body: json!({
                "name": "flow-entitled",
                "source": {
                    "name": "from-partner",
                    "entitlementArn": "arn:aws:mediaconnect:us-east-1:000000000000:entitlement:1-0000000f:shared"
                }
            }),
            sources: 1,
            outputs: 0,
            source_type: "ENTITLED",
            availability_zone: "us-east-1a",
        },
        CreateCase {
            case: "source named only by the flow, three outputs, LARGE flow size",
            body: json!({
                "name": "FLOW-Large_4",
                "flowSize": "LARGE",
                "source": { "protocol": "udp", "whitelistCidr": "192.0.2.0/24", "maxBitrate": 50_000_000 },
                "outputs": [
                    { "protocol": "udp", "destination": "203.0.113.20", "port": 6000 },
                    { "protocol": "udp", "destination": "203.0.113.21", "port": 6002 },
                    { "name": "named-out", "protocol": "srt-caller", "destination": "203.0.113.22", "port": 6004, "streamId": "s-1" }
                ]
            }),
            sources: 1,
            outputs: 3,
            source_type: "OWNED",
            availability_zone: "us-east-1a",
        },
    ]
}

/// CreateFlow materialises each requested source and output into a stored record with
/// its own ARN, and the response is that record — the same bytes DescribeFlow reports.
#[tokio::test]
async fn create_flow_persists_the_flow_it_returns() {
    use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

    let emulator = Emulator::new();

    for CreateCase {
        case,
        body,
        sources,
        outputs,
        source_type,
        availability_zone,
    } in create_cases()
    {
        let requested_name = body["name"].as_str().expect("the case names the flow");
        let (status, code, created) = emulator.create_flow(body.clone()).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: expected 201 with no error, got {status} {code:?}: {created}"
        );

        let flow = &created["flow"];
        let flow_arn = flow["flowArn"].as_str().unwrap_or_default().to_string();
        assert_minted_flow_arn(case, &flow_arn, requested_name);
        assert_eq!(
            flow["name"].as_str(),
            Some(requested_name),
            "case {case:?}: stored Name, got {:?}",
            flow["name"]
        );
        assert_eq!(
            flow["status"].as_str(),
            Some("STANDBY"),
            "case {case:?}: a new flow stands by, got {:?}",
            flow["status"]
        );
        assert_eq!(
            flow["availabilityZone"].as_str(),
            Some(availability_zone),
            "case {case:?}: AvailabilityZone, got {:?}",
            flow["availabilityZone"]
        );

        // Every source is materialised with its own ARN, and `Flow.Source` is the first
        // of `Flow.Sources`.
        let stored_sources = flow["sources"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            stored_sources.len(),
            sources,
            "case {case:?}: one Source per requested source, got {stored_sources:?}"
        );
        assert_eq!(
            flow["source"], stored_sources[0],
            "case {case:?}: Flow.Source is the first of Flow.Sources, got {:?}",
            flow["source"]
        );
        let mut source_arns: Vec<&str> = stored_sources
            .iter()
            .map(|source| source["sourceArn"].as_str().unwrap_or_default())
            .collect();
        source_arns.sort_unstable();
        source_arns.dedup();
        assert_eq!(
            source_arns.len(),
            sources,
            "case {case:?}: each source has its own SourceArn, got {source_arns:?}"
        );
        let requested_sources: Vec<Value> = body["sources"].as_array().cloned().unwrap_or_else(|| {
            if body["source"].is_object() {
                vec![body["source"].clone()]
            } else {
                Vec::new()
            }
        });
        for (index, source) in stored_sources.iter().enumerate() {
            let entitled = source["entitlementArn"].is_string();
            assert_eq!(
                source["ingestIp"].is_string(),
                !entitled,
                "case {case:?}: an owned source gets an ingest address and an entitled one \
                 does not, got {source:?}"
            );
            assert_eq!(
                source["ingestPort"].is_number(),
                !entitled,
                "case {case:?}: an owned source gets an ingest port and an entitled one \
                 does not, got {source:?}"
            );
            // An explicit `IngestPort` is kept verbatim; a source that asked for none is
            // assigned one from the service's pool (`crate::state::assigned_ingest_port`:
            // 5000 up to the top of the 16-bit port space) — except on a Zixi protocol,
            // whose port the model fixes ("if the protocol of the source is Zixi, the port
            // must be set to 2088", `UpdateFlowSourceRequest$IngestPort`), asserted over
            // the whole `Protocol` catalog in `crate::flow_source_bridge_and_ingest_port`.
            if !entitled {
                let port = source["ingestPort"].as_i64().unwrap_or_default();
                let requested = requested_sources[index]["ingestPort"].as_i64();
                let zixi = requested_sources[index]["protocol"]
                    .as_str()
                    .is_some_and(mc::is_zixi_protocol);
                match (requested, zixi) {
                    (_, true) => assert_eq!(
                        port,
                        i64::from(mc::ZIXI_INGEST_PORT),
                        "case {case:?}: source {index} is a Zixi source, so it listens on \
                         the port the model fixes, got {port}"
                    ),
                    (Some(requested), false) => assert_eq!(
                        port, requested,
                        "case {case:?}: source {index} keeps the requested IngestPort, got \
                         {port}"
                    ),
                    (None, false) => assert!(
                        (5_000..=65_535).contains(&port),
                        "case {case:?}: source {index} is assigned a port from the pool, got \
                         {port}"
                    ),
                }
            }
            assert!(
                source["sourceArn"]
                    .as_str()
                    .unwrap_or_default()
                    .ends_with(source["name"].as_str().unwrap_or("?")),
                "case {case:?}: a SourceArn ends with the source's name, got {source:?}"
            );
        }

        // Every output is materialised with its own ARN and a resolved OutputStatus.
        let stored_outputs = flow["outputs"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            stored_outputs.len(),
            outputs,
            "case {case:?}: one Output per requested output, got {stored_outputs:?}"
        );
        let mut output_arns: Vec<&str> = stored_outputs
            .iter()
            .map(|output| output["outputArn"].as_str().unwrap_or_default())
            .collect();
        output_arns.sort_unstable();
        output_arns.dedup();
        assert_eq!(
            output_arns.len(),
            outputs,
            "case {case:?}: each output has its own OutputArn, got {output_arns:?}"
        );
        let requested_outputs = body["outputs"].as_array().cloned().unwrap_or_default();
        for (index, output) in stored_outputs.iter().enumerate() {
            // "If you don't specify the outputStatus field in your request, MediaConnect
            // sets it to ENABLED" (AddOutputRequest$OutputStatus).
            let expected_status = requested_outputs[index]["outputStatus"]
                .as_str()
                .unwrap_or("ENABLED");
            assert_eq!(
                output["outputStatus"].as_str(),
                Some(expected_status),
                "case {case:?}: output {index} status, got {output:?}"
            );
            assert_eq!(
                output["transport"]["protocol"],
                requested_outputs[index]["protocol"],
                "case {case:?}: output {index} transport protocol, got {output:?}"
            );
        }

        // The round trip: DescribeFlow reports the stored record, byte for byte.
        let (status, code, described) = emulator.describe_flow(&flow_arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: describe expected 200, got {status} {code:?}: {described}"
        );
        assert_eq!(
            described["flow"], created["flow"],
            "case {case:?}: DescribeFlow must report exactly the created flow, got {:?}",
            described["flow"]
        );

        // And ListFlows summarises the same record.
        let (status, _, list) = emulator.list_flows("").await;
        assert_eq!(status, 200, "case {case:?}: list expected 200, got {status}");
        let summary = listed(&list)
            .into_iter()
            .find(|(arn, ..)| arn == &flow_arn)
            .unwrap_or_else(|| panic!("case {case:?}: the new flow must be listed, got {list}"));
        assert_eq!(
            summary,
            (
                flow_arn.clone(),
                requested_name.to_string(),
                "STANDBY".to_string(),
                source_type.to_string()
            ),
            "case {case:?}: the listed summary must agree with the stored flow"
        );
    }
}

/// StartFlow and StopFlow move the PERSISTED status, through the transient status the
/// clock settles: the change is visible to DescribeFlow, and DeleteFlow removes the flow
/// so DescribeFlow afterwards is a 404 and ListFlows no longer lists it.
#[tokio::test]
async fn start_stop_and_delete_move_the_persisted_status() {
    let emulator = Emulator::new();
    let (status, _, created) = emulator
        .create_flow(json!({
            "name": "lifecycle-flow",
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
        }))
        .await;
    assert_eq!(status, 201, "create expected 201, got {status}: {created}");
    let flow_arn = created["flow"]["flowArn"]
        .as_str()
        .expect("the created flow carries an ARN")
        .to_string();

    // A transition the clock owes is not instantaneous: the caller sees the transient
    // status first, then the settled one. `STARTING -> ACTIVE`, then
    // `STOPPING -> STANDBY`.
    for (case, method, transient, settled) in [
        ("start", "start", "STARTING", "ACTIVE"),
        ("stop", "stop", "STOPPING", "STANDBY"),
    ] {
        let (status, code, body) = emulator
            .send("POST", &format!("/v1/flows/{method}/{flow_arn}"), None)
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: expected 202, got {status} {code:?}: {body}"
        );
        assert_eq!(
            (body["flowArn"].as_str(), body["status"].as_str()),
            (Some(flow_arn.as_str()), Some(transient)),
            "case {case:?}: the response names the flow and its transient status, got {body}"
        );

        let (_, _, described) = emulator.describe_flow(&flow_arn).await;
        assert_eq!(
            described["flow"]["status"].as_str(),
            Some(transient),
            "case {case:?}: the transient status is persisted, got {:?}",
            described["flow"]["status"]
        );

        // Wait out the transition the clock owes, then the settled status is reported.
        tokio::time::sleep(Duration::from_millis(900)).await;
        let (_, _, described) = emulator.describe_flow(&flow_arn).await;
        assert_eq!(
            described["flow"]["status"].as_str(),
            Some(settled),
            "case {case:?}: the clock settles the status, got {:?}",
            described["flow"]["status"]
        );
        let summary = listed(&emulator.list_flows("").await.2);
        assert!(
            summary
                .iter()
                .any(|(arn, _, listed_status, _)| arn == &flow_arn && listed_status == settled),
            "case {case:?}: ListFlows reports the settled status, got {summary:?}"
        );
    }

    let (status, code, deleted) = emulator
        .send("DELETE", &format!("/v1/flows/{flow_arn}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "delete expected 202, got {status} {code:?}: {deleted}"
    );
    assert_eq!(
        (deleted["flowArn"].as_str(), deleted["status"].as_str()),
        (Some(flow_arn.as_str()), Some("DELETING")),
        "delete reports the flow and DELETING, got {deleted}"
    );

    let (status, code, body) = emulator.describe_flow(&flow_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (404, Some("NotFoundException")),
        "describe after delete expected 404 NotFoundException, got {status} {code:?}: {body}"
    );
    let (_, _, list) = emulator.list_flows("").await;
    assert!(
        listed(&list).is_empty(),
        "a deleted flow must not be listed, got {list}"
    );
}

/// ListFlows pages the persisted flows: whatever `MaxResults` is, the pages partition
/// every stored flow exactly once, in one stable order, and only the last page omits
/// `NextToken`.
#[tokio::test]
async fn list_flows_pages_every_stored_flow_exactly_once() {
    let emulator = Emulator::new();
    let total = 7;
    for index in 0..total {
        let (status, _, body) = emulator
            .create_flow(json!({
                "name": format!("page-flow-{index}"),
                "source": { "name": format!("src-{index}"), "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
            }))
            .await;
        assert_eq!(status, 201, "flow {index} expected 201, got {status}: {body}");
    }

    let (_, _, all) = emulator.list_flows("?maxResults=1000").await;
    let expected: Vec<String> = listed(&all).into_iter().map(|(arn, ..)| arn).collect();
    assert_eq!(
        expected.len(),
        total,
        "every created flow is listed, got {expected:?}"
    );

    // The page size spans the modelled `MaxResults` range (1..1000): a page smaller than
    // the set, one that divides it, one that equals it, and the maximum.
    for page_size in [1_usize, 2, 3, 7, 1000] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
                None => format!("?maxResults={page_size}"),
            };
            let (status, code, body) = emulator.list_flows(&query).await;
            assert_eq!(
                (status, code.as_deref()),
                (200, None),
                "page size {page_size}: expected 200, got {status} {code:?}: {body}"
            );
            let page: Vec<String> = listed(&body).into_iter().map(|(arn, ..)| arn).collect();
            token = body["nextToken"].as_str().map(ToString::to_string);
            if token.is_some() {
                assert_eq!(
                    page.len(),
                    page_size,
                    "page size {page_size}: a page before the last one is full, got {page:?}"
                );
            }
            seen.extend(page);
            pages += 1;
            if token.is_none() {
                break;
            }
            assert!(
                pages <= total + 1,
                "page size {page_size}: paging did not terminate after {pages} pages"
            );
        }
        assert_eq!(
            seen, expected,
            "page size {page_size}: the pages must partition every stored flow once, in order"
        );
    }

    // A token stays usable when the flow it names is deleted between pages: it is the
    // ARN the listing RESUMES AT, so the key it carries need not still exist.
    let (_, _, first_page) = emulator.list_flows("?maxResults=3").await;
    let token = first_page["nextToken"]
        .as_str()
        .expect("a page smaller than the set carries a token")
        .to_string();
    let (status, _, _) = emulator
        .send("DELETE", &format!("/v1/flows/{token}"), None)
        .await;
    assert_eq!(
        status, 202,
        "deleting the flow the token names expected 202, got {status}"
    );
    let (status, code, resumed) = emulator
        .list_flows(&format!("?maxResults=1000&nextToken={token}"))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "resuming at a deleted flow's ARN expected 200, got {status} {code:?}: {resumed}"
    );
    let resumed_arns: Vec<String> = listed(&resumed).into_iter().map(|(arn, ..)| arn).collect();
    assert_eq!(
        resumed_arns,
        expected[4..].to_vec(),
        "resuming at a deleted flow's ARN lists the flows after it, got {resumed_arns:?}"
    );
}

/// One refused request: the URI (and body) to send, and the status and error code the
/// service answers with.
struct RefusalCase {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    status: u16,
    code: &'static str,
}

/// Every flow operation refuses an unknown flow with `NotFoundException` (404) — whether
/// the label is an ARN this service could have minted or not an ARN at all, because the
/// label is an identifier the store resolves. CreateFlow refuses a request that breaks a
/// rule of the class with `BadRequestException` (400). Never a 501 and never a success.
#[tokio::test]
async fn refusals_name_the_modelled_error() {
    let emulator = Emulator::new();
    let (status, _, created) = emulator
        .create_flow(json!({
            "name": "taken-name",
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
        }))
        .await;
    assert_eq!(status, 201, "create expected 201, got {status}: {created}");

    // Well-formed per the modelled pattern, but no such flow.
    let missing = "arn:aws:mediaconnect:us-east-1:000000000000:flow:1-deadbeef:missing";
    // Not an ARN at all: it names no flow either, so it resolves to no flow.
    let malformed = "not-an-arn";

    let cases = vec![
        RefusalCase {
            case: "describe an unknown flow",
            method: "GET",
            uri: format!("/v1/flows/{missing}"),
            body: None,
            status: 404,
            code: "NotFoundException",
        },
        RefusalCase {
            case: "start an unknown flow",
            method: "POST",
            uri: format!("/v1/flows/start/{missing}"),
            body: None,
            status: 404,
            code: "NotFoundException",
        },
        RefusalCase {
            case: "stop an unknown flow",
            method: "POST",
            uri: format!("/v1/flows/stop/{missing}"),
            body: None,
            status: 404,
            code: "NotFoundException",
        },
        RefusalCase {
            case: "delete an unknown flow",
            method: "DELETE",
            uri: format!("/v1/flows/{missing}"),
            body: None,
            status: 404,
            code: "NotFoundException",
        },
        RefusalCase {
            case: "describe a label that is not an ARN",
            method: "GET",
            uri: format!("/v1/flows/{malformed}"),
            body: None,
            status: 404,
            code: "NotFoundException",
        },
        RefusalCase {
            case: "delete a label that is not an ARN",
            method: "DELETE",
            uri: format!("/v1/flows/{malformed}"),
            body: None,
            status: 404,
            code: "NotFoundException",
        },
        RefusalCase {
            case: "create a flow with no source",
            method: "POST",
            uri: "/v1/flows".to_string(),
            body: Some(json!({ "name": "sourceless" })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "create a flow with an empty source list",
            method: "POST",
            uri: "/v1/flows".to_string(),
            body: Some(json!({ "name": "sourceless-list", "sources": [] })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "create a flow whose name cannot be an ARN segment",
            method: "POST",
            uri: "/v1/flows".to_string(),
            body: Some(json!({
                "name": "bad name/here",
                "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
            })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "create a flow with no name at all",
            method: "POST",
            uri: "/v1/flows".to_string(),
            body: Some(json!({
                "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
            })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "list with MaxResults outside the modelled range",
            method: "GET",
            uri: "/v1/flows?maxResults=0".to_string(),
            body: None,
            status: 400,
            code: "BadRequestException",
        },
        // A `NextToken` is the flow ARN the listing resumes at, so a token the service
        // could not have minted is refused. The rows below are the ways a token can fail
        // the modelled `FlowArn` form: not an ARN at all, another service's ARN, an
        // account id that is not 12 digits, and a flow name outside `[a-zA-Z0-9_-]+`.
        RefusalCase {
            case: "list with a nextToken that is not an ARN",
            method: "GET",
            uri: "/v1/flows?nextToken=not-a-token".to_string(),
            body: None,
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "list with a nextToken naming another service",
            method: "GET",
            uri: "/v1/flows?nextToken=arn:aws:medialive:us-east-1:000000000000:flow:1-abcdef12:f"
                .to_string(),
            body: None,
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "list with a nextToken whose account id is too short",
            method: "GET",
            uri: "/v1/flows?nextToken=arn:aws:mediaconnect:us-east-1:00000000000:flow:1-abcdef12:f"
                .to_string(),
            body: None,
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "list with a nextToken whose flow name is not an ARN segment",
            method: "GET",
            uri: "/v1/flows?nextToken=arn:aws:mediaconnect:us-east-1:000000000000:flow:1-abcdef12:"
                .to_string(),
            body: None,
            status: 400,
            code: "BadRequestException",
        },
    ];

    for RefusalCase {
        case,
        method,
        uri,
        body,
        status: expected_status,
        code: expected_code,
    } in cases
    {
        let (status, code, response) = emulator.send(method, &uri, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, Some(expected_code)),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {code:?}: \
             {response}"
        );
        assert_eq!(
            response["__type"].as_str(),
            Some(expected_code),
            "case {case:?}: the error envelope names the shape, got {response}"
        );
        assert!(
            response["message"].as_str().is_some_and(|m| !m.is_empty()),
            "case {case:?}: the refusal carries a message, got {response}"
        );
    }

    // Not one refusal above was about the flow that already exists: a flow name is not
    // a reservation, so creating a second flow with the same name succeeds (the ARN's
    // generated id keeps the two apart). The rule is asserted over a table of names in
    // `tests/all/flow_identity_and_description.rs`; here it only has to not be refused.
    let (status, code, body) = emulator
        .create_flow(json!({
            "name": "taken-name",
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "reusing a live flow's name expected 201, got {status} {code:?}: {body}"
    );
}

/// The flow store is BOUNDED: CreateFlow refuses once the store already holds
/// [`service_com_amazonaws_mediaconnect_mediaconnect::state::MAX_FLOWS`] flows, with the
/// modelled `CreateFlow420Exception` (HTTP 420) and without storing anything, while the
/// flows that were created are still readable. Deleting one makes room for exactly one
/// more, so the bound is a ceiling on what is stored, not a counter of calls.
#[tokio::test]
async fn create_flow_refuses_past_the_store_ceiling() {
    const CEILING: usize = service_com_amazonaws_mediaconnect_mediaconnect::state::MAX_FLOWS;

    let emulator = Emulator::new();
    // One slot short of the ceiling, set up directly; the calls at the boundary below
    // go through the real request path.
    emulator.seed_flows(CEILING - 1);
    let (status, code, body) = emulator
        .create_flow(json!({
            "name": "bound-flow-last",
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "the create that fills the last slot expected 201, got {status} {code:?}: {body}"
    );
    let last_arn = body["flow"]["flowArn"]
        .as_str()
        .expect("the created flow carries its ARN")
        .to_string();

    // One past the ceiling: refused with the error the model declares for a flow this
    // service will not create.
    let (status, code, body) = emulator
        .create_flow(json!({
            "name": "bound-flow-overflow",
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (420, Some("CreateFlow420Exception")),
        "at the ceiling, create expected 420 CreateFlow420Exception, got {status} {code:?}: \
         {body}"
    );
    assert!(
        body["message"].as_str().is_some_and(|m| !m.is_empty()),
        "at the ceiling, the refusal carries a message, got {body}"
    );

    // Nothing was stored by the refused call, and what was created is still readable.
    let (status, code, described) = emulator.describe_flow(&last_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "after the refusal the stored flows are still readable, got {status} {code:?}: \
         {described}"
    );
    let (status, _, listed_after) = emulator.list_flows("?maxResults=1").await;
    assert_eq!(
        status, 200,
        "after the refusal ListFlows still answers, got {status}: {listed_after}"
    );

    // Deleting one flow makes room for exactly one more: the bound counts what is
    // stored, not how many calls were made.
    let (status, _, _) = emulator
        .send("DELETE", &format!("/v1/flows/{last_arn}"), None)
        .await;
    assert_eq!(status, 202, "delete expected 202, got {status}");
    let (status, code, body) = emulator
        .create_flow(json!({
            "name": "bound-flow-after-delete",
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "after a delete, create expected 201, got {status} {code:?}: {body}"
    );
    let (status, code, body) = emulator
        .create_flow(json!({
            "name": "bound-flow-overflow-again",
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (420, Some("CreateFlow420Exception")),
        "back at the ceiling, create expected 420 CreateFlow420Exception, got {status} \
         {code:?}: {body}"
    );
}

/// Every collection ONE CreateFlow request can fill is bounded by
/// [`service_com_amazonaws_mediaconnect_mediaconnect::state::MAX_FLOW_COLLECTION_ENTRIES`]:
/// a request at the bound is stored, one entry past it is refused with the modelled
/// `BadRequestException` (400). The table's rows are those collections and the tag map, so
/// the rule is asserted over the whole class rather than over `Outputs` alone.
///
/// `Sources` is NOT one of them: a flow's source count is bounded by its own failover
/// state instead ("if the state is inactive, the flow can have only one source. If the
/// state is active, the flow can have one or two sources", `FailoverConfig$State`), the
/// subject of `crate::flow_source_failover_bound`.
#[tokio::test]
async fn create_flow_bounds_every_request_sized_collection() {
    const LIMIT: usize =
        service_com_amazonaws_mediaconnect_mediaconnect::state::MAX_FLOW_COLLECTION_ENTRIES;

    /// The request body for `count` entries in the collection named by `member`.
    fn body(name: &str, member: &str, count: usize) -> Value {
        let mut request = json!({
            "name": name,
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
        });
        match member {
            "Outputs" => {
                request["outputs"] = (0..count)
                    .map(|index| {
                        json!({
                            "name": format!("out-{index}"),
                            "protocol": "udp",
                            "destination": "203.0.113.10",
                            "port": 5000 + i32::try_from(index).unwrap_or(0) * 2
                        })
                    })
                    .collect();
            }
            "Entitlements" => {
                request["entitlements"] = (0..count)
                    .map(|index| {
                        json!({
                            "name": format!("ent-{index}"),
                            "subscribers": ["111122223333"]
                        })
                    })
                    .collect();
            }
            "MediaStreams" => {
                request["mediaStreams"] = (0..count)
                    .map(|index| {
                        json!({
                            "mediaStreamId": index,
                            "mediaStreamName": format!("stream-{index}"),
                            "mediaStreamType": "video"
                        })
                    })
                    .collect();
            }
            "VpcInterfaces" => {
                request["vpcInterfaces"] = (0..count)
                    .map(|index| {
                        json!({
                            "name": format!("vpc-{index}"),
                            "roleArn": "arn:aws:iam::111122223333:role/MediaConnect",
                            "securityGroupIds": ["sg-0123456789abcdef0"],
                            "subnetId": "subnet-0123456789abcdef0"
                        })
                    })
                    .collect();
            }
            "FlowTags" => {
                request["flowTags"] = (0..count)
                    .map(|index| (format!("tag-{index}"), json!("value")))
                    .collect::<serde_json::Map<String, Value>>()
                    .into();
            }
            other => panic!("unknown collection {other:?}"),
        }
        request
    }

    let emulator = Emulator::new();
    // Every collection a CreateFlow request can fill, from the operation's input shape:
    // `smithy_query(query="operation",operation="CreateFlow")`.
    for member in [
        "Outputs",
        "Entitlements",
        "MediaStreams",
        "VpcInterfaces",
        "FlowTags",
    ] {
        // At the bound: accepted, and the stored record carries the entries.
        let at_bound = format!("at-bound-{}", member.to_lowercase());
        let (status, code, created) = emulator
            .create_flow(body(&at_bound, member, LIMIT))
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {member:?} at the bound ({LIMIT}): expected 201, got {status} {code:?}: \
             {created}"
        );

        // One entry past it: refused, and nothing is stored under that name.
        let past_bound = format!("past-bound-{}", member.to_lowercase());
        let (status, code, refused) = emulator
            .create_flow(body(&past_bound, member, LIMIT + 1))
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {member:?} one past the bound ({}): expected 400 BadRequestException, got \
             {status} {code:?}: {refused}",
            LIMIT + 1
        );
        let (_, _, all) = emulator.list_flows("?maxResults=1000").await;
        assert!(
            !listed(&all).iter().any(|(_, name, ..)| name == &past_bound),
            "case {member:?}: a refused request stores nothing, got {all}"
        );
    }
}
