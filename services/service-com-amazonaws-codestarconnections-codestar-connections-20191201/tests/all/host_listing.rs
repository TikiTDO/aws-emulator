// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListHosts`, and for the `CreateHost` that puts hosts in the
//! store it pages over.
//!
//! Every call goes over the wire: a request is dispatched through the
//! [`MultiServiceRouter`] exactly as the emulator's front door does it (`POST /` +
//! `X-Amz-Target: CodeStar_connections_20191201.<Operation>`, awsJson1_0 body), so what
//! is asserted here is the service's HTTP behaviour and not a handler called directly.
//!
//! Nothing is seeded into the store by hand: a host gets there through `CreateHost` and
//! is read back through `ListHosts`, so these tests pin the round trip rather than the
//! shape of a record someone wrote into state.
//!
//! The rules pinned, each over a TABLE rather than one request:
//!
//! * `ListHosts` reports every host the store holds, rebuilt from the stored record —
//!   for every modeled `ProviderType`, both published `Name` length bounds, with and
//!   without a VPC configuration, with and without tags;
//! * paging with any page size inside the modeled `MaxResults` range yields each host
//!   exactly once in a stable (creation) order, and emits a token only when a further
//!   page exists;
//! * a `MaxResults` outside the modeled range is refused, and a `NextToken` this service
//!   did not issue is refused as a bad request rather than read as the first page;
//! * a host removed by `DeleteHost` stops being listed, and an empty store lists nothing;
//! * a tag operation names any resource this service HOLDS, so a host created here is
//!   taggable exactly as a connection is;
//! * listing changes nothing a later reader sees.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codestarconnections_codestar_connections_20191201 as codestar;

/// The region the test signs for; the ARNs the service mints must carry it.
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

/// The status a host created through the API carries, from `CreateHost`'s own doc ("A
/// host created through the CLI or the SDK is in `PENDING` status by default").
///
/// Referenced from the service's own catalog rather than spelled out here.
const CREATED_HOST_STATUS: &str = codestar::state::HOST_STATUS_PENDING;

/// One emulator: the router the production front door builds, with this service mounted
/// on a single shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The tests sign nothing, so no authenticator may be registered.
        router.set_request_authentication(false);
        router.register_service(codestar::EmulatedService::new(engine));
        Self { router }
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

    /// Dispatch one signed awsJson1_0 call to `operation` under the short target prefix.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        self.call_target(&format!("{TARGET_PREFIX}.{operation}"), body, true)
            .await
    }

    /// `CreateHost`, asserting success, and return the minted ARN.
    async fn create_host(&self, case: &str, body: Value) -> String {
        let (status, response) = self.call("CreateHost", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateHost to answer 200, got {status} {response:?}"
        );
        let arn = response["HostArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no HostArn in {response:?}"))
            .to_string();
        assert_host_arn(case, &arn);
        arn
    }

    /// `DeleteHost`, asserting success.
    async fn delete_host(&self, case: &str, host_arn: &str) {
        let (status, response) = self.call("DeleteHost", json!({ "HostArn": host_arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteHost to answer 200, got {status} {response:?}"
        );
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

/// Assert `arn` is a host ARN of the modeled `HostArn` form
/// `^arn:aws(-[\w]+)*:codestar-connections:.+:[0-9]{12}:host\/.+$`, in this request's
/// region and account, whose resource is `host/<uuid>`.
fn assert_host_arn(case: &str, arn: &str) {
    let prefix = format!("arn:aws:codestar-connections:{REGION}:{ACCOUNT_ID}:host/");
    assert!(
        arn.starts_with(&prefix),
        "case {case:?}: expected a host ARN under {prefix:?}, got {arn:?}"
    );
    let id = &arn[prefix.len()..];
    let hyphens: Vec<usize> = id
        .char_indices()
        .filter(|(_, c)| *c == '-')
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        (id.len(), hyphens.as_slice()),
        (36, [8usize, 13, 18, 23].as_slice()),
        "case {case:?}: expected a hyphenated UUID host id, got {id:?}"
    );
    assert!(
        id.chars().all(|c| c == '-' || c.is_ascii_hexdigit()),
        "case {case:?}: expected a hexadecimal host id, got {id:?}"
    );
}

/// One `ListHosts` page: its `Hosts` entries and its `NextToken`, asserting a 200 and
/// that the page respects the requested size.
async fn list_page(
    emulator: &Emulator,
    case: &str,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (Vec<Value>, Option<String>) {
    let mut body = Map::new();
    if let Some(max) = max_results {
        body.insert("MaxResults".to_string(), json!(max));
    }
    if let Some(token) = next_token {
        body.insert("NextToken".to_string(), json!(token));
    }
    let (status, response) = emulator.call("ListHosts", Value::Object(body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListHosts to answer 200, got {status} {response:?}"
    );
    let hosts = response["Hosts"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no Hosts array in {response:?}"))
        .clone();
    // `MaxResults` 0 is the modeled default, i.e. "no page size requested".
    if let Some(max) = max_results.filter(|max| *max > 0) {
        assert!(
            hosts.len() as i64 <= max,
            "case {case:?}: page of {} exceeds MaxResults {max}",
            hosts.len()
        );
    }
    (
        hosts,
        response["NextToken"].as_str().map(ToString::to_string),
    )
}

/// Every host, read by paging with `max_results` and following `NextToken` to the end.
async fn list_every_host(emulator: &Emulator, case: &str, max_results: Option<i64>) -> Vec<Value> {
    let mut hosts = Vec::new();
    let mut next_token: Option<String> = None;
    // A page must make progress, so the loop cannot need more rounds than hosts.
    for _ in 0..64 {
        let (page, token) = list_page(emulator, case, max_results, next_token.as_deref()).await;
        hosts.extend(page);
        match token {
            Some(token) => next_token = Some(token),
            None => return hosts,
        }
    }
    panic!("case {case:?}: ListHosts never stopped emitting a NextToken");
}

/// The `HostArn` of each listed entry, in the order they were listed.
fn listed_arns(case: &str, hosts: &[Value]) -> Vec<String> {
    hosts
        .iter()
        .map(|host| {
            host["HostArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no HostArn in {host:?}"))
                .to_string()
        })
        .collect()
}

/// One host to create: the members `CreateHost` accepts, and what `ListHosts` must then
/// report back.
struct HostRow {
    case: String,
    name: String,
    provider_type: &'static str,
    provider_endpoint: String,
    vpc_configuration: Option<Value>,
    tags: Option<Value>,
}

impl HostRow {
    /// The `CreateHost` request body for this row.
    fn request(&self) -> Value {
        let mut body = json!({
            "Name": self.name,
            "ProviderType": self.provider_type,
            "ProviderEndpoint": self.provider_endpoint,
        });
        if let Some(vpc) = &self.vpc_configuration {
            body["VpcConfiguration"] = vpc.clone();
        }
        if let Some(tags) = &self.tags {
            body["Tags"] = tags.clone();
        }
        body
    }
}

/// The table every "a created host is reported as created" assertion runs over: each
/// modeled `ProviderType`, both published `Name` length bounds (`len 1..64`), an endpoint
/// with and without a port and path, and a VPC configuration (with and without the
/// optional `TlsCertificate`) on half the rows. Bounds and member names come from
/// `smithy_query(query="operation", operation="CreateHost")` and
/// `smithy_query(query="shape", shape="VpcConfiguration")`.
fn host_rows() -> Vec<HostRow> {
    let mut rows = Vec::new();
    for (index, provider) in PROVIDER_TYPES.iter().enumerate() {
        rows.push(HostRow {
            case: format!("{provider}/minimal"),
            // The modeled `HostName` minimum length is 1.
            name: "h".to_string(),
            provider_type: provider,
            provider_endpoint: format!("https://ghes-{index}.example.net"),
            vpc_configuration: None,
            tags: None,
        });
        rows.push(HostRow {
            case: format!("{provider}/vpc+tags"),
            // The modeled `HostName` maximum length is 64.
            name: format!("{provider}{}", "x".repeat(64 - provider.len())),
            provider_type: provider,
            provider_endpoint: format!("https://ghes-{index}.example.net:8443/api/v3"),
            vpc_configuration: Some(json!({
                "VpcId": "vpc-0a1b2c3d",
                "SubnetIds": ["subnet-0a1b2c3d", "subnet-1b2c3d4e"],
                "SecurityGroupIds": ["sg-0a1b2c3d"],
                // `TlsCertificate` is optional; one row carries it and the other does not.
                "TlsCertificate": if index % 2 == 0 { Value::Null } else { json!("-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----") },
            })),
            tags: Some(json!([
                { "Key": "owner", "Value": "platform" },
                { "Key": "tier", "Value": "build" },
            ])),
        });
    }
    // A `TlsCertificate` of `null` is not a member at all.
    for row in &mut rows {
        if let Some(vpc) = row.vpc_configuration.as_mut() {
            if vpc["TlsCertificate"].is_null() {
                vpc.as_object_mut()
                    .expect("the VPC configuration is an object")
                    .remove("TlsCertificate");
            }
        }
    }
    rows
}

/// Assert one listed entry reports exactly what the row was created with.
fn assert_listed_host(row: &HostRow, host_arn: &str, host: &Value) {
    let case = &row.case;
    assert_eq!(
        host["HostArn"].as_str(),
        Some(host_arn),
        "case {case:?}: wrong HostArn in {host:?}"
    );
    assert_eq!(
        host["Name"].as_str(),
        Some(row.name.as_str()),
        "case {case:?}: wrong Name in {host:?}"
    );
    assert_eq!(
        host["ProviderType"].as_str(),
        Some(row.provider_type),
        "case {case:?}: wrong ProviderType in {host:?}"
    );
    assert_eq!(
        host["ProviderEndpoint"].as_str(),
        Some(row.provider_endpoint.as_str()),
        "case {case:?}: wrong ProviderEndpoint in {host:?}"
    );
    assert_eq!(
        host["Status"].as_str(),
        Some(CREATED_HOST_STATUS),
        "case {case:?}: wrong Status in {host:?}"
    );
    match &row.vpc_configuration {
        Some(vpc) => assert_eq!(
            &host["VpcConfiguration"], vpc,
            "case {case:?}: wrong VpcConfiguration in {host:?}"
        ),
        None => assert!(
            host.get("VpcConfiguration").is_none_or(Value::is_null),
            "case {case:?}: a host created without a VpcConfiguration reported one: {host:?}"
        ),
    }
    // A created host awaits the console setup step, which is not one of the VPC-config
    // transitional statuses `Host$StatusMessage` describes, so the entry reports no
    // `StatusMessage` member at all. `host_status_descriptions.rs` pins the rule over
    // every documented status.
    assert!(
        host.get("StatusMessage").is_none_or(Value::is_null),
        "case {case:?}: a created host reported a StatusMessage in {host:?}"
    );
}

/// RULE: `ListHosts` reports every host the service holds, each rebuilt from the stored
/// record — name, provider type, endpoint, VPC configuration and status — in creation
/// order.
#[tokio::test]
async fn every_created_host_is_reported_by_list_from_state() {
    let emulator = Emulator::new();
    let rows = host_rows();
    assert!(
        rows.len() >= 2 * PROVIDER_TYPES.len(),
        "the ProviderType catalog must drive this table, got {} rows",
        rows.len()
    );

    let mut created = Vec::new();
    for row in &rows {
        created.push(emulator.create_host(&row.case, row.request()).await);
    }

    let listed = list_every_host(&emulator, "every host", None).await;
    assert_eq!(
        listed_arns("every host", &listed),
        created,
        "expected every created host exactly once in creation order, got {listed:?}"
    );
    for ((row, host_arn), host) in rows.iter().zip(&created).zip(&listed) {
        assert_listed_host(row, host_arn, host);
    }
}

/// RULE: repeated creates mint distinct hosts, so two hosts that differ in nothing a
/// caller supplied are still two rows in the listing.
#[tokio::test]
async fn repeated_creates_are_distinct_hosts_in_the_listing() {
    let emulator = Emulator::new();
    let request = json!({
        "Name": "twin",
        "ProviderType": "GitHubEnterpriseServer",
        "ProviderEndpoint": "https://ghes.example.net",
    });

    let mut created = Vec::new();
    for attempt in 0..3 {
        created.push(
            emulator
                .create_host(&format!("twin-{attempt}"), request.clone())
                .await,
        );
    }
    let unique: BTreeSet<&String> = created.iter().collect();
    assert_eq!(
        unique.len(),
        created.len(),
        "expected {} distinct host ARNs, got {created:?}",
        created.len()
    );

    let listed = list_every_host(&emulator, "twins", None).await;
    assert_eq!(
        listed_arns("twins", &listed),
        created,
        "expected each identically-created host listed once, got {listed:?}"
    );
}

/// RULE: paging with any page size the model accepts yields every host exactly once in a
/// stable order, and a `NextToken` is emitted only when a further page exists.
#[tokio::test]
async fn paging_yields_every_host_exactly_once_for_every_page_size() {
    let emulator = Emulator::new();

    // An empty store is the zero row of this rule: nothing to list, and so no token.
    let (page, token) = list_page(&emulator, "empty store", None, None).await;
    assert!(
        page.is_empty() && token.is_none(),
        "case \"empty store\": expected no hosts and no NextToken, got {page:?} / {token:?}"
    );

    let mut created = Vec::new();
    for index in 0..7 {
        let case = format!("host-{index}");
        created.push(
            emulator
                .create_host(
                    &case,
                    json!({
                        "Name": case,
                        "ProviderType": PROVIDER_TYPES[index % PROVIDER_TYPES.len()],
                        "ProviderEndpoint": format!("https://ghes-{index}.example.net"),
                    }),
                )
                .await,
        );
    }

    // `MaxResults` is modeled `range 0..100`: both published bounds, sizes that divide
    // the set and sizes that do not, plus an omitted member (which arrives as the
    // modeled default 0, i.e. "no page size requested").
    for max_results in [None, Some(0), Some(1), Some(2), Some(3), Some(7), Some(100)] {
        let case = format!("MaxResults={max_results:?}");
        let listed = list_every_host(&emulator, &case, max_results).await;
        assert_eq!(
            listed_arns(&case, &listed),
            created,
            "case {case:?}: expected every host exactly once in creation order, got {listed:?}"
        );
    }

    // The last page of an exactly-divided walk carries no token: 7 hosts at 7 per page
    // is one full page and nothing after it.
    let (page, token) = list_page(&emulator, "one full page", Some(7), None).await;
    assert_eq!(
        (page.len(), token.clone()),
        (7, None),
        "case \"one full page\": expected the whole set and no NextToken, got {token:?}"
    );
}

/// RULE: a `MaxResults` outside the modeled `range 0..100` is refused; every value
/// inside it is accepted.
#[tokio::test]
async fn max_results_outside_the_modeled_range_is_refused() {
    let emulator = Emulator::new();
    for index in 0..3 {
        emulator
            .create_host(
                &format!("host-{index}"),
                json!({
                    "Name": format!("host-{index}"),
                    "ProviderType": "GitHub",
                    "ProviderEndpoint": "https://github.com",
                }),
            )
            .await;
    }

    // Both published bounds and a value between them.
    for max_results in [0i64, 1, 50, 100] {
        let case = format!("MaxResults={max_results}");
        let (status, response) = emulator
            .call("ListHosts", json!({ "MaxResults": max_results }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a MaxResults inside the modeled range must be accepted, \
             got {status} {response:?}"
        );
    }

    // One past each published bound, and a value far outside it.
    for max_results in [-1i64, 101, 1000] {
        let case = format!("MaxResults={max_results}");
        let (status, response) = emulator
            .call("ListHosts", json!({ "MaxResults": max_results }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a MaxResults outside the modeled range must be refused, \
             got {status} {response:?}"
        );
    }
}

/// RULE: a `NextToken` this service did not issue is refused as a bad request, not
/// silently treated as the first page.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    emulator
        .create_host(
            "seed",
            json!({
                "Name": "seed",
                "ProviderType": "GitHub",
                "ProviderEndpoint": "https://github.com",
            }),
        )
        .await;

    for token in [
        "not-a-token",
        "0",
        "zzzzzzzzzzzzzzzz",
        " 000000000000000",
        "00000000000000000",
    ] {
        let case = format!("NextToken={token:?}");
        let (status, response) = emulator
            .call("ListHosts", json!({ "NextToken": token }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 for a forged NextToken, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: wrong error code in {response:?}"
        );
        assert!(
            response.get("Hosts").is_none(),
            "case {case:?}: a refused request must report no hosts: {response:?}"
        );
    }

    // The token this service DID issue is accepted, so the refusal above is about the
    // token's provenance and not about sending one at all.
    emulator
        .create_host(
            "second",
            json!({
                "Name": "second",
                "ProviderType": "GitHub",
                "ProviderEndpoint": "https://github.com",
            }),
        )
        .await;
    let (_, token) = list_page(&emulator, "issued token", Some(1), None).await;
    let token = token.expect("a second host means a further page, so a token is issued");
    let (page, _) = list_page(&emulator, "issued token", Some(1), Some(&token)).await;
    assert_eq!(
        page.len(),
        1,
        "case \"issued token\": expected the second page to carry the second host, got {page:?}"
    );
}

/// RULE: `ListHosts` reports the hosts the store holds NOW — a host `DeleteHost` removed
/// is not listed, whichever position it held, and a store emptied of hosts lists none.
#[tokio::test]
async fn a_deleted_host_is_no_longer_listed() {
    for deleted_index in 0..3usize {
        let case = format!("delete-index-{deleted_index}");
        let emulator = Emulator::new();

        let mut created = Vec::new();
        for index in 0..3 {
            created.push(
                emulator
                    .create_host(
                        &case,
                        json!({
                            "Name": format!("host-{index}"),
                            "ProviderType": "GitHubEnterpriseServer",
                            "ProviderEndpoint": format!("https://ghes-{index}.example.net"),
                        }),
                    )
                    .await,
            );
        }

        emulator.delete_host(&case, &created[deleted_index]).await;

        let expected: Vec<String> = created
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != deleted_index)
            .map(|(_, arn)| arn.clone())
            .collect();
        let listed = list_every_host(&emulator, &case, None).await;
        assert_eq!(
            listed_arns(&case, &listed),
            expected,
            "case {case:?}: expected the surviving hosts in creation order, got {listed:?}"
        );

        // Emptying the store leaves an empty list and no token, not the last page again.
        for arn in &expected {
            emulator.delete_host(&case, arn).await;
        }
        let (page, token) = list_page(&emulator, &case, None, None).await;
        assert!(
            page.is_empty() && token.is_none(),
            "case {case:?}: expected no hosts and no NextToken, got {page:?} / {token:?}"
        );
    }
}

/// RULE: an RPC target names its operation in the segment after its LAST dot, whatever
/// prefix stands in front of it — so every published form of this service's
/// `X-Amz-Target` prefix reaches the operation's handler, signed or not, and a prefix
/// naming another service reaches none of them.
#[tokio::test]
async fn every_published_target_prefix_form_reaches_the_handler() {
    // Each prefix form a client may write for this service: the single-identifier short
    // form, the fully qualified Coral name an SDK sends (the form the dispatch issue
    // `20261023T122922Z-codestar-connections-implemented-connection-operations-answer-501-notimplemented-instead-of.md`
    // records), the absolute shape id, and one namespace label.
    let prefixes = [
        ("the short prefix", TARGET_PREFIX),
        (
            "the Coral service name an SDK sends",
            "com.amazonaws.codestar.connections.CodeStar_connections_20191201",
        ),
        (
            "the absolute shape id",
            "com.amazonaws.codestarconnections.CodeStar_connections_20191201",
        ),
        (
            "one namespace label",
            "codestarconnections.CodeStar_connections_20191201",
        ),
    ];

    for (form, prefix) in prefixes {
        for signed in [true, false] {
            let case = format!("{form}/signed={signed}");
            let emulator = Emulator::new();

            let (status, response) = emulator
                .call_target(
                    &format!("{prefix}.CreateHost"),
                    json!({
                        "Name": "routed",
                        "ProviderType": "GitHubEnterpriseServer",
                        "ProviderEndpoint": "https://ghes.example.net",
                    }),
                    signed,
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected CreateHost to reach its handler, got {status} \
                 {response:?}"
            );
            let created = response["HostArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no HostArn in {response:?}"))
                .to_string();

            let (status, response) = emulator
                .call_target(&format!("{prefix}.ListHosts"), json!({}), signed)
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected ListHosts to reach its handler, got {status} \
                 {response:?}"
            );
            let listed = response["Hosts"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: no Hosts array in {response:?}"))
                .clone();
            assert_eq!(
                listed_arns(&case, &listed),
                vec![created],
                "case {case:?}: expected the host created under this prefix to be listed, \
                 got {listed:?}"
            );
        }
    }

    // A prefix naming ANOTHER service is not served here, however this service's own
    // operations are named: the operation is resolved only for the service the prefix
    // names.
    for target in [
        "CodeConnections_20231201.ListHosts",
        "com.amazonaws.codeconnections.CodeConnections_20231201.ListHosts",
    ] {
        let emulator = Emulator::new();
        let (status, response) = emulator.call_target(target, json!({}), false).await;
        assert_ne!(
            status, 200,
            "case {target:?}: a target naming another service must not be served here, \
             got {status} {response:?}"
        );
    }
}

/// RULE: the tag operations apply to any resource this service HOLDS, keyed by its ARN —
/// a host as much as a connection — and refuse an ARN naming no stored resource.
#[tokio::test]
async fn tag_operations_apply_to_every_resource_the_service_holds() {
    let emulator = Emulator::new();

    // One resource of each type this service persists, each created through its own API.
    let host = emulator
        .create_host(
            "host",
            json!({
                "Name": "tagged-host",
                "ProviderType": "GitHubEnterpriseServer",
                "ProviderEndpoint": "https://ghes.example.net",
            }),
        )
        .await;
    let (status, response) = emulator
        .call(
            "CreateConnection",
            json!({ "ConnectionName": "tagged-connection", "ProviderType": "GitHub" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"connection\": expected CreateConnection to answer 200, got {status} {response:?}"
    );
    let connection = response["ConnectionArn"]
        .as_str()
        .expect("CreateConnection reports a ConnectionArn")
        .to_string();

    for (case, resource_arn) in [("host", &host), ("connection", &connection)] {
        let (status, response) = emulator
            .call(
                "TagResource",
                json!({
                    "ResourceArn": resource_arn,
                    "Tags": [{ "Key": "owner", "Value": "platform" }],
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected TagResource to tag a stored resource, got {status} \
             {response:?}"
        );

        let (status, response) = emulator
            .call("ListTagsForResource", json!({ "ResourceArn": resource_arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListTagsForResource to answer 200, got {status} \
             {response:?}"
        );
        assert_eq!(
            response["Tags"],
            json!([{ "Key": "owner", "Value": "platform" }]),
            "case {case:?}: wrong Tags in {response:?}"
        );

        let (status, response) = emulator
            .call(
                "UntagResource",
                json!({ "ResourceArn": resource_arn, "TagKeys": ["owner"] }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UntagResource to answer 200, got {status} {response:?}"
        );
        let (_, response) = emulator
            .call("ListTagsForResource", json!({ "ResourceArn": resource_arn }))
            .await;
        assert_eq!(
            response["Tags"],
            json!([]),
            "case {case:?}: the removed tag outlived UntagResource: {response:?}"
        );
    }

    // An ARN naming no stored resource is still refused, for either resource type: the
    // rule is "a resource this service holds", not "an ARN of the right shape".
    let absent_host = format!("arn:aws:codestar-connections:{REGION}:{ACCOUNT_ID}:host/absent");
    let absent_connection =
        format!("arn:aws:codestar-connections:{REGION}:{ACCOUNT_ID}:connection/absent");
    for (case, resource_arn) in [("absent host", absent_host), ("absent", absent_connection)] {
        for (operation, body) in [
            (
                "TagResource",
                json!({
                    "ResourceArn": resource_arn,
                    "Tags": [{ "Key": "owner", "Value": "platform" }],
                }),
            ),
            (
                "UntagResource",
                json!({ "ResourceArn": resource_arn, "TagKeys": ["owner"] }),
            ),
            (
                "ListTagsForResource",
                json!({ "ResourceArn": resource_arn }),
            ),
        ] {
            let case = format!("{case}/{operation}");
            let (status, response) = emulator.call(operation, body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the modeled not-found refusal, got {status} \
                 {response:?}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {case:?}: wrong error code in {response:?}"
            );
        }
    }
}

/// RULE: listing is a read — it changes nothing a later caller sees, however it is paged.
#[tokio::test]
async fn listing_does_not_change_what_a_later_reader_sees() {
    let emulator = Emulator::new();
    for index in 0..4 {
        emulator
            .create_host(
                &format!("host-{index}"),
                json!({
                    "Name": format!("host-{index}"),
                    "ProviderType": PROVIDER_TYPES[index % PROVIDER_TYPES.len()],
                    "ProviderEndpoint": format!("https://ghes-{index}.example.net"),
                }),
            )
            .await;
    }

    let first = list_every_host(&emulator, "first read", None).await;
    // Page through it twice at the smallest page size, then read it whole again: the
    // answer must be byte-for-byte what the first read gave.
    for case in ["page walk 1", "page walk 2"] {
        let paged = list_every_host(&emulator, case, Some(1)).await;
        assert_eq!(
            paged, first,
            "case {case:?}: paging reported a different set than the whole read"
        );
    }
    let last = list_every_host(&emulator, "last read", None).await;
    assert_eq!(
        last, first,
        "case \"last read\": reads must not change what a later reader sees"
    );
}
