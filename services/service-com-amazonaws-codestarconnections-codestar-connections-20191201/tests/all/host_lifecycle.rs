// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CreateHost` and the read that proves it, `GetHost`.
//!
//! Every call goes over the wire: a request is dispatched through the
//! [`MultiServiceRouter`] exactly as the emulator's front door does it (`POST /` +
//! `X-Amz-Target: CodeStar_connections_20191201.<Operation>`, awsJson1_0 body), so what
//! is asserted here is the service's HTTP behaviour and not a handler called directly.
//!
//! The rules pinned, each over a TABLE rather than one request:
//!
//! * `CreateHost` persists the host its request describes and answers a `HostArn` of the
//!   modeled form; a later `GetHost` on that ARN reports the stored host unchanged —
//!   over every modeled `ProviderType`, the published `HostName` / `Url` length bounds,
//!   with and without a `VpcConfiguration`, and over several tag sets;
//! * a host created through the API is `PENDING` and carries no status description, and
//!   the ARN it is keyed by carries the region the caller signed for;
//! * nothing about a host is keyed by its NAME: hosts created under one name get
//!   distinct ARNs and are independent resources;
//! * a `HostArn` naming no host — never created, or deleted since — is reported ABSENT
//!   (`ResourceNotFoundException`, at the status the recorded service answers) and
//!   nothing is written;
//! * a request outside a modeled constraint is refused and stores nothing;
//! * `GetHost` is a report: repeating it changes nothing a later caller sees.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codestarconnections_codestar_connections_20191201 as codestar;

use codestar::state::{
    ComAmazonawsCodestarconnectionsCodestarConnections20191201State as ServiceState,
    HOST_STATUS_PENDING,
};

/// The region the test signs for by default; the ARNs the service mints carry it.
const REGION: &str = "us-west-2";

/// The canonical emulator account id (`engine::emulator_account_id`).
const ACCOUNT_ID: &str = "000000000000";

/// awsJson1_0 target prefix, from `OPERATIONS.md` ("Wire protocol").
const TARGET_PREFIX: &str = "CodeStar_connections_20191201";

/// Every value the model's `ProviderType` enum accepts.
///
/// Referenced, not copied: this is the generated catalog
/// (`src/ops/types/shared_types.rs::PROVIDER_TYPE_VALUES`, from
/// `smithy_query(query="shape", shape="ProviderType")`), so the tables below widen with
/// the model instead of drifting from it.
const PROVIDER_TYPES: &[&str] = codestar::ops::types::shared_types::PROVIDER_TYPE_VALUES;

/// One emulator: the router the production front door builds, with this service mounted
/// on a single shared engine, plus a handle on that engine so a test can observe what was
/// persisted beside what the wire reports.
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

    /// Dispatch one awsJson1_0 call signed for `region` and return `(status, body)`.
    async fn call_in_region(&self, region: &str, operation: &str, body: Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("https://codestar-connections.us-west-2.amazonaws.com/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            // The SigV4 credential scope is what routes the request to this service and
            // what tells it which region to mint ARNs in.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/{region}\
                     /codestar-connections/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
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

    /// Dispatch one call signed for [`REGION`].
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        self.call_in_region(REGION, operation, body).await
    }

    /// `CreateHost`, asserting success, and return the minted ARN.
    async fn create_host(&self, case: &str, body: Value) -> String {
        let (status, response) = self.call("CreateHost", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateHost to answer 200, got {status} {response:?}"
        );
        response["HostArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no HostArn in {response:?}"))
            .to_string()
    }

    /// How many hosts the store holds.
    fn host_count(&self) -> usize {
        self.engine.state::<ServiceState>().lock().hosts.len()
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

    /// The `(status, status_message)` the store holds for `host_arn`.
    fn stored_status(&self, host_arn: &str) -> (String, Option<String>) {
        let state = self.engine.state::<ServiceState>();
        let store = state.lock();
        let host = store
            .hosts
            .get(host_arn)
            .unwrap_or_else(|| panic!("the created host {host_arn} is in the store"));
        (host.status.clone(), host.status_message.clone())
    }
}

/// The error code an awsJson error envelope carries (`__type`), reduced to the shape
/// NAME: the awsJson protocols allow the code to be written with or without its
/// namespace, and a client matches on the name.
fn error_code(response: &Value) -> String {
    let raw = response["__type"].as_str().unwrap_or_default();
    raw.rsplit_once('#')
        .map_or(raw, |(_, name)| name)
        .to_string()
}

/// The `Message` an awsJson error envelope carries.
fn error_message(response: &Value) -> String {
    response["Message"]
        .as_str()
        .or_else(|| response["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// A `Tags` wire list as a map, so a comparison does not depend on member order.
fn tag_map(tags: &Value) -> BTreeMap<String, String> {
    tags.as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|tag| {
                    (
                        tag["Key"].as_str().unwrap_or_default().to_string(),
                        tag["Value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Assert `arn` is a host ARN of the modeled `HostArn` form
/// `^arn:aws(-[\w]+)*:codestar-connections:.+:[0-9]{12}:host\/.+$`, in `region` and the
/// emulator's account, whose resource is `host/<non-empty id>`.
fn assert_host_arn(case: &str, arn: &str, region: &str) {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(
        fields.len(),
        6,
        "case {case:?}: a host ARN has six colon-separated fields, got {arn:?}"
    );
    assert_eq!(
        fields[0], "arn",
        "case {case:?}: wrong ARN prefix in {arn:?}"
    );
    // Every ARN this emulator mints is in the `aws` partition (`engine::emulator_arn`),
    // which is what the modeled `HostArn` pattern `^arn:aws(-[\w]+)*:…` allows.
    assert_eq!(
        fields[1], "aws",
        "case {case:?}: wrong partition field in {arn:?}"
    );
    assert_eq!(
        fields[2], "codestar-connections",
        "case {case:?}: wrong service field in {arn:?}"
    );
    assert_eq!(
        fields[3], region,
        "case {case:?}: the ARN must carry the signed region, got {arn:?}"
    );
    assert_eq!(
        fields[4], ACCOUNT_ID,
        "case {case:?}: wrong account field in {arn:?}"
    );
    let resource = fields[5];
    let id = resource
        .strip_prefix("host/")
        .unwrap_or_else(|| panic!("case {case:?}: resource must be host/<id>, got {resource:?}"));
    assert!(
        !id.is_empty(),
        "case {case:?}: the host id must not be empty in {arn:?}"
    );
}

/// One row of the create/read table: a host request and what it should read back as.
struct HostCase {
    case: &'static str,
    name: String,
    provider_type: &'static str,
    provider_endpoint: String,
    vpc_configuration: Option<Value>,
    tags: Vec<(&'static str, &'static str)>,
}

/// The table: one row per modeled `ProviderType`, plus the published `HostName`
/// (`len 1..64`) and `Url` (`len 1..512`) bounds, a `VpcConfiguration` with and without
/// its optional `TlsCertificate`, and tag sets of zero, one and three keys.
fn host_cases() -> Vec<HostCase> {
    let mut cases: Vec<HostCase> = PROVIDER_TYPES
        .iter()
        .enumerate()
        .map(|(index, provider_type)| HostCase {
            case: provider_type,
            name: format!("host-{provider_type}"),
            provider_type,
            provider_endpoint: format!("https://{provider_type}.{index}.example.internal"),
            vpc_configuration: None,
            tags: Vec::new(),
        })
        .collect();

    cases.push(HostCase {
        case: "shortest modeled name and endpoint",
        name: "h".to_string(),
        provider_type: "GitHubEnterpriseServer",
        provider_endpoint: "e".to_string(),
        vpc_configuration: None,
        tags: Vec::new(),
    });
    cases.push(HostCase {
        case: "longest modeled name and endpoint",
        name: "n".repeat(64),
        provider_type: "GitHubEnterpriseServer",
        // `Url` is `len 1..512`; this is exactly the published maximum.
        provider_endpoint: format!("https://{}", "e".repeat(512 - "https://".len())),
        vpc_configuration: None,
        tags: Vec::new(),
    });
    cases.push(HostCase {
        case: "vpc configuration, one subnet and one security group",
        name: "host-vpc-minimal".to_string(),
        provider_type: "GitHubEnterpriseServer",
        provider_endpoint: "https://ghe.vpc.example.internal".to_string(),
        // `VpcId` is `len 12..21`, pattern `^vpc-\w{8}(\w{9})?$`; `SubnetIds` and
        // `SecurityGroupIds` are `len 1..10`.
        vpc_configuration: Some(json!({
            "VpcId": "vpc-0a1b2c3d",
            "SubnetIds": ["subnet-0a1b2c3d"],
            "SecurityGroupIds": ["sg-0a1b2c3d"],
        })),
        tags: Vec::new(),
    });
    cases.push(HostCase {
        case: "vpc configuration with a TLS certificate and ten subnets",
        name: "host-vpc-full".to_string(),
        provider_type: "GitLabSelfManaged",
        provider_endpoint: "https://gitlab.vpc.example.internal".to_string(),
        // `SubnetIds` members are `^subnet-\w{8}(\w{9})?$` and `SecurityGroupIds`
        // members `^sg-\w{8}(\w{9})?$`, so each id carries exactly eight word
        // characters.
        vpc_configuration: Some(json!({
            "VpcId": "vpc-0a1b2c3d4e5f60718",
            "SubnetIds": (0..10).map(|n| format!("subnet-0a1b2c3{n}")).collect::<Vec<_>>(),
            "SecurityGroupIds": (0..10).map(|n| format!("sg-0a1b2c3{n}")).collect::<Vec<_>>(),
            "TlsCertificate": "-----BEGIN CERTIFICATE-----\nQUJD\n-----END CERTIFICATE-----",
        })),
        tags: Vec::new(),
    });
    cases.push(HostCase {
        case: "one tag",
        name: "host-one-tag".to_string(),
        provider_type: "GitHubEnterpriseServer",
        provider_endpoint: "https://ghe.one-tag.example.internal".to_string(),
        vpc_configuration: None,
        tags: vec![("env", "prod")],
    });
    cases.push(HostCase {
        case: "three tags",
        name: "host-three-tags".to_string(),
        provider_type: "GitHubEnterpriseServer",
        provider_endpoint: "https://ghe.three-tags.example.internal".to_string(),
        vpc_configuration: None,
        tags: vec![("env", "prod"), ("team", "infra"), ("owner", "platform")],
    });
    cases
}

/// The `CreateHost` request body for one row.
fn create_body(host: &HostCase) -> Value {
    let mut body = json!({
        "Name": host.name,
        "ProviderType": host.provider_type,
        "ProviderEndpoint": host.provider_endpoint,
    });
    if let Some(vpc) = &host.vpc_configuration {
        body["VpcConfiguration"] = vpc.clone();
    }
    if !host.tags.is_empty() {
        body["Tags"] = Value::Array(
            host.tags
                .iter()
                .map(|(key, value)| json!({ "Key": key, "Value": value }))
                .collect(),
        );
    }
    body
}

/// RULE: `CreateHost` persists the host its request describes under a newly minted
/// `HostArn` of the modeled form, and a later `GetHost` on that ARN reports every stored
/// member unchanged.
#[tokio::test]
async fn create_host_persists_the_request_and_get_host_reports_it() {
    let emulator = Emulator::new();

    for host in host_cases() {
        let case = host.case;
        let (status, created) = emulator.call("CreateHost", create_body(&host)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateHost to answer 200, got {status} {created:?}"
        );
        let host_arn = created["HostArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no HostArn in {created:?}"))
            .to_string();
        assert_host_arn(case, &host_arn, REGION);

        // The created host's tags are reported from state, and a host created with none
        // reports no tag list at all.
        let expected_tags: BTreeMap<String, String> = host
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        if expected_tags.is_empty() {
            assert!(
                created["Tags"].is_null(),
                "case {case:?}: a host created with no tags must report no Tags, got {created:?}"
            );
        } else {
            assert_eq!(
                tag_map(&created["Tags"]),
                expected_tags,
                "case {case:?}: CreateHost reported the wrong tags in {created:?}"
            );
        }

        // Round trip: the read reports what the create stored.
        let (status, read) = emulator
            .call("GetHost", json!({ "HostArn": host_arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetHost to answer 200, got {status} {read:?}"
        );
        assert_eq!(
            read["Name"].as_str(),
            Some(host.name.as_str()),
            "case {case:?}: GetHost reported the wrong Name, got {read:?}"
        );
        assert_eq!(
            read["ProviderType"].as_str(),
            Some(host.provider_type),
            "case {case:?}: GetHost reported the wrong ProviderType, got {read:?}"
        );
        assert_eq!(
            read["ProviderEndpoint"].as_str(),
            Some(host.provider_endpoint.as_str()),
            "case {case:?}: GetHost reported the wrong ProviderEndpoint, got {read:?}"
        );
        assert_eq!(
            read["Status"].as_str(),
            Some(HOST_STATUS_PENDING),
            "case {case:?}: a host created through the API is PENDING, got {read:?}"
        );
        match &host.vpc_configuration {
            None => assert!(
                read["VpcConfiguration"].is_null(),
                "case {case:?}: a host created without a VPC configuration must report \
                 none, got {read:?}"
            ),
            Some(vpc) => assert_eq!(
                &read["VpcConfiguration"],
                vpc,
                "case {case:?}: GetHost reported the wrong VpcConfiguration, got {read:?}"
            ),
        }

        // The tags are addressable by the host's ARN, like any resource this service
        // holds.
        let (status, listed) = emulator
            .call("ListTagsForResource", json!({ "ResourceArn": host_arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListTagsForResource to answer 200 for a host ARN, \
             got {status} {listed:?}"
        );
        assert_eq!(
            tag_map(&listed["Tags"]),
            expected_tags,
            "case {case:?}: the host's stored tags are wrong in {listed:?}"
        );
    }
}

/// RULE: the ARN a create mints carries the region the caller signed for, so it resolves
/// against the endpoint the client addressed.
#[tokio::test]
async fn a_minted_host_arn_carries_the_signed_region() {
    let emulator = Emulator::new();

    // Region codes spanning the commercial, European and Asia-Pacific naming plus a
    // China region: whichever one the caller signs for, the ARN carries THAT region (and
    // the `aws` partition this emulator mints in, which `assert_host_arn` checks).
    for region in ["us-east-1", "eu-west-1", "ap-southeast-4", "cn-north-1"] {
        let (status, created) = emulator
            .call_in_region(
                region,
                "CreateHost",
                json!({
                    "Name": format!("host-{region}"),
                    "ProviderType": "GitHubEnterpriseServer",
                    "ProviderEndpoint": format!("https://ghe.{region}.example.internal"),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected CreateHost to answer 200, got {status} {created:?}"
        );
        let host_arn = created["HostArn"].as_str().unwrap_or_default();
        assert_host_arn(region, host_arn, region);

        // The ARN the caller got back is the one the host is stored under.
        let (status, read) = emulator
            .call_in_region(region, "GetHost", json!({ "HostArn": host_arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {region:?}: the minted ARN must resolve, got {status} {read:?}"
        );
    }
}

/// RULE: a host created through the API is `PENDING` — a non-terminal status it leaves
/// only when the host is set up in the console — and carries no status description, which
/// the service reports only for a host whose VPC configuration is in flight.
#[tokio::test]
async fn a_host_created_through_the_api_is_pending_without_a_status_description() {
    let emulator = Emulator::new();

    for provider_type in PROVIDER_TYPES {
        let host_arn = emulator
            .create_host(
                provider_type,
                json!({
                    "Name": format!("pending-{provider_type}"),
                    "ProviderType": provider_type,
                    "ProviderEndpoint": "https://pending.example.internal",
                }),
            )
            .await;

        let (status, read) = emulator
            .call("GetHost", json!({ "HostArn": host_arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {provider_type:?}: expected GetHost to answer 200, got {status} {read:?}"
        );
        assert_eq!(
            read["Status"].as_str(),
            Some(HOST_STATUS_PENDING),
            "case {provider_type:?}: expected status PENDING, got {read:?}"
        );

        // `Host$StatusMessage` ("The status description for the host") describes a VPC
        // configuration being provisioned or torn down, and a host awaiting the console
        // setup step is in neither, so nothing is persisted for it either —
        // `host_status_descriptions.rs` pins what `ListHosts` reports.
        let (stored_status, stored_message) = emulator.stored_status(&host_arn);
        assert_eq!(
            stored_status, HOST_STATUS_PENDING,
            "case {provider_type:?}: expected the stored status PENDING, got \
             {stored_status:?}"
        );
        assert!(
            stored_message.is_none(),
            "case {provider_type:?}: a pending host carries no status description, got \
             {stored_message:?}"
        );
    }
}

/// RULE: nothing about a host is keyed by its name — the model constrains `HostName` to
/// `len 1..64` and nothing else — so creates that repeat a name mint distinct ARNs and
/// yield independent resources.
#[tokio::test]
async fn hosts_created_under_one_name_get_distinct_arns() {
    let emulator = Emulator::new();

    // Each row repeats one name a number of times; the name forms differ along what the
    // model allows (length, spaces, punctuation, case).
    let cases: [(&str, &str, usize); 4] = [
        ("plain name twice", "duplicate-host", 2),
        ("name with spaces, three times", "Duplicate Host", 3),
        ("single-character name, four times", "h", 4),
        ("longest modeled name, twice", "x", 2),
    ];

    for (case, name, repeats) in cases {
        let name = if case.starts_with("longest") {
            name.repeat(64)
        } else {
            name.to_string()
        };
        let mut arns: Vec<String> = Vec::new();
        for attempt in 0..repeats {
            let arn = emulator
                .create_host(
                    case,
                    json!({
                        "Name": name,
                        "ProviderType": "GitHubEnterpriseServer",
                        "ProviderEndpoint": format!("https://ghe-{attempt}.example.internal"),
                    }),
                )
                .await;
            assert!(
                !arns.contains(&arn),
                "case {case:?}: CreateHost reused the ARN {arn:?} (already got {arns:?})"
            );
            arns.push(arn);
        }

        // Each ARN resolves to its OWN host: same name, the endpoint of its own create.
        for (attempt, arn) in arns.iter().enumerate() {
            let (status, read) = emulator.call("GetHost", json!({ "HostArn": arn })).await;
            assert_eq!(
                status, 200,
                "case {case:?}: host {attempt} must resolve, got {status} {read:?}"
            );
            assert_eq!(
                read["Name"].as_str(),
                Some(name.as_str()),
                "case {case:?}: host {attempt} reported the wrong Name, got {read:?}"
            );
            assert_eq!(
                read["ProviderEndpoint"].as_str(),
                Some(format!("https://ghe-{attempt}.example.internal").as_str()),
                "case {case:?}: host {attempt} reported another host's endpoint, got {read:?}"
            );
        }

        // Deleting one leaves its namesakes alone.
        let (status, deleted) = emulator
            .call("DeleteHost", json!({ "HostArn": arns[0] }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteHost to answer 200, got {status} {deleted:?}"
        );
        for arn in &arns[1..] {
            let (status, read) = emulator.call("GetHost", json!({ "HostArn": arn })).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a namesake host was removed too, got {status} {read:?}"
            );
        }
    }
}

/// RULE: a `HostArn` that resolves to no host this service holds is reported ABSENT —
/// `ResourceNotFoundException`, with a message naming the ARN that was supplied — and the
/// failed read writes nothing.
#[tokio::test]
async fn a_host_arn_naming_no_host_is_reported_absent() {
    let emulator = Emulator::new();

    // One host exists, so the refusal comes from the lookup and not from an empty store.
    let kept = emulator
        .create_host(
            "kept",
            json!({
                "Name": "kept-host",
                "ProviderType": "GitHubEnterpriseServer",
                "ProviderEndpoint": "https://kept.example.internal",
            }),
        )
        .await;

    // A host created and then deleted: its ARN is as absent as one never minted, and the
    // ARN is never reused.
    let deleted = emulator
        .create_host(
            "deleted",
            json!({
                "Name": "doomed-host",
                "ProviderType": "GitHubEnterpriseServer",
                "ProviderEndpoint": "https://doomed.example.internal",
            }),
        )
        .await;
    let (status, response) = emulator
        .call("DeleteHost", json!({ "HostArn": deleted }))
        .await;
    assert_eq!(
        status, 200,
        "expected DeleteHost to answer 200, got {status} {response:?}"
    );

    // Every ARN below satisfies the modeled `HostArn` pattern but names no stored host.
    let unknown: Vec<(&str, String)> = vec![
        ("a deleted host", deleted.clone()),
        (
            "a uuid-shaped id",
            format!(
                "arn:aws:codestar-connections:{REGION}:{ACCOUNT_ID}\
                 :host/00000000-0000-0000-0000-000000000000"
            ),
        ),
        (
            "another region",
            format!("arn:aws:codestar-connections:eu-west-1:{ACCOUNT_ID}:host/some-host"),
        ),
        (
            "another partition",
            format!("arn:aws-cn:codestar-connections:cn-north-1:{ACCOUNT_ID}:host/some-host"),
        ),
        (
            "a deeper resource path",
            format!("arn:aws:codestar-connections:{REGION}:{ACCOUNT_ID}:host/kept-host/extra"),
        ),
    ];

    let before = emulator.host_arns();
    for (case, host_arn) in &unknown {
        let (status, response) = emulator.call("GetHost", json!({ "HostArn": host_arn })).await;
        // The model puts `@httpError 404` on `ResourceNotFoundException`, but the
        // recorded wire response for a host lookup that resolves to nothing is HTTP 400
        // with that same shape and message — the status a client of the real service
        // branches on; see `RESOURCE_NOT_FOUND_STATUS` in `src/state.rs`, the one status
        // every absent resource of this service is answered with.
        assert_eq!(
            status, 400,
            "case {case:?}: expected GetHost to refuse an absent host, got \
             {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: wrong error code in {response:?}"
        );
        assert!(
            error_message(&response).contains(host_arn.as_str()),
            "case {case:?}: the message must name the supplied ARN, got {:?}",
            error_message(&response)
        );
    }
    assert_eq!(
        emulator.host_arns(),
        before,
        "a refused GetHost changed the store"
    );

    // The surviving host still resolves.
    let (status, read) = emulator.call("GetHost", json!({ "HostArn": kept })).await;
    assert_eq!(
        status, 200,
        "the surviving host must still resolve, got {status} {read:?}"
    );
}

/// RULE: a `CreateHost` request outside a modeled constraint is refused before anything
/// is stored — `Name` is `len 1..64`, `ProviderEndpoint` is `len 1..512` and
/// `ProviderType` is a closed enum, so each published bound holds and one past it does
/// not.
#[tokio::test]
async fn a_create_host_request_outside_the_modeled_constraints_stores_nothing() {
    let emulator = Emulator::new();

    // (case, body, must be accepted)
    let cases: Vec<(String, Value, bool)> = vec![
        // `Name`: len 1..64.
        ("name len 0".to_string(), host_with_name(0), false),
        ("name len 1".to_string(), host_with_name(1), true),
        ("name len 64".to_string(), host_with_name(64), true),
        ("name len 65".to_string(), host_with_name(65), false),
        // `ProviderEndpoint`: len 1..512.
        ("endpoint len 0".to_string(), host_with_endpoint(0), false),
        ("endpoint len 1".to_string(), host_with_endpoint(1), true),
        ("endpoint len 512".to_string(), host_with_endpoint(512), true),
        ("endpoint len 513".to_string(), host_with_endpoint(513), false),
        // `ProviderType`: the closed modeled enum.
        (
            "provider type outside the enum".to_string(),
            json!({
                "Name": "bad-provider",
                "ProviderType": "GitHubEnterprise",
                "ProviderEndpoint": "https://ghe.example.internal",
            }),
            false,
        ),
        (
            "a required member missing".to_string(),
            json!({ "Name": "no-endpoint", "ProviderType": "GitHub" }),
            false,
        ),
    ];

    let mut accepted = 0usize;
    for (case, body, must_be_accepted) in cases {
        let before = emulator.host_count();
        let (status, response) = emulator.call("CreateHost", body).await;
        if must_be_accepted {
            accepted += 1;
            assert_eq!(
                status, 200,
                "case {case:?}: a request inside every modeled constraint was refused: \
                 {status} {response:?}"
            );
            assert_eq!(
                emulator.host_count(),
                before + 1,
                "case {case:?}: an accepted create stored no host"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a request outside a modeled constraint was accepted: \
                 {status} {response:?}"
            );
            assert_eq!(
                emulator.host_count(),
                before,
                "case {case:?}: a refused create still wrote to the store"
            );
        }
    }
    assert_eq!(
        emulator.host_count(),
        accepted,
        "the store must hold exactly the accepted hosts"
    );
}

/// A `CreateHost` body whose `Name` is `length` characters long.
fn host_with_name(length: usize) -> Value {
    json!({
        "Name": "n".repeat(length),
        "ProviderType": "GitHubEnterpriseServer",
        "ProviderEndpoint": "https://ghe.example.internal",
    })
}

/// A `CreateHost` body whose `ProviderEndpoint` is `length` characters long.
fn host_with_endpoint(length: usize) -> Value {
    json!({
        "Name": format!("endpoint-len-{length}"),
        "ProviderType": "GitHubEnterpriseServer",
        "ProviderEndpoint": "e".repeat(length),
    })
}

/// RULE: `GetHost` reports; it never changes what a later caller sees.
#[tokio::test]
async fn repeating_get_host_changes_nothing() {
    let emulator = Emulator::new();

    let host_arn = emulator
        .create_host(
            "stable",
            json!({
                "Name": "stable-host",
                "ProviderType": "GitHubEnterpriseServer",
                "ProviderEndpoint": "https://stable.example.internal",
                "Tags": [{ "Key": "env", "Value": "prod" }],
            }),
        )
        .await;

    let mut answers: Vec<Value> = Vec::new();
    for attempt in 0..4 {
        let (status, read) = emulator
            .call("GetHost", json!({ "HostArn": host_arn }))
            .await;
        assert_eq!(
            status, 200,
            "read {attempt}: expected GetHost to answer 200, got {status} {read:?}"
        );
        answers.push(read);
    }
    for (attempt, answer) in answers.iter().enumerate() {
        assert_eq!(
            answer, &answers[0],
            "read {attempt}: a repeated GetHost answered differently: {answer:?}"
        );
    }

    // The store is untouched by the reads: same hosts, same status, same tags.
    assert_eq!(
        emulator.host_arns(),
        vec![host_arn.clone()],
        "a GetHost changed which hosts exist"
    );
    assert_eq!(
        emulator.stored_status(&host_arn).0,
        HOST_STATUS_PENDING,
        "a GetHost moved the host's status"
    );
    let (status, listed) = emulator
        .call("ListTagsForResource", json!({ "ResourceArn": host_arn }))
        .await;
    assert_eq!(
        status, 200,
        "expected ListTagsForResource to answer 200, got {status} {listed:?}"
    );
    assert_eq!(
        tag_map(&listed["Tags"]),
        BTreeMap::from([("env".to_string(), "prod".to_string())]),
        "a GetHost changed the host's tags: {listed:?}"
    );
}
