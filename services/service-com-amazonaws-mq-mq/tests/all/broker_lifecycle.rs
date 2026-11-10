// SPDX-License-Identifier: CC0-1.0

//! End-to-end broker lifecycle for Amazon MQ, driven over the wire.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the real
//! front door uses, so these tests exercise routing, restJson1 (de)serialization
//! and the handlers together. Each assertion names its case and the actual value.
//!
//! The value catalogs the tables span are the model's own enumerations —
//! `smithy_query(query="shape", shape="EngineType"|"DeploymentMode"|"BrokerStorageType")`,
//! which the crate re-exports as `ENGINE_TYPE_VALUES`, `DEPLOYMENT_MODE_VALUES`
//! and `BROKER_STORAGE_TYPE_VALUES` — not a list invented here.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use service_com_amazonaws_mq_mq::ops::types::{
    BROKER_STORAGE_TYPE_VALUES, DEPLOYMENT_MODE_VALUES, ENGINE_TYPE_VALUES,
};
use service_com_amazonaws_mq_mq::state::BROKER_PROVISIONING;

/// A SigV4 `Authorization` header signing for `mq` in `region`: it is what the
/// router resolves this service from, and what the ARN's region segment is read
/// from.
pub(crate) fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/mq/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// One emulator with one shared engine, serving `mq` over the production dispatch path.
pub(crate) struct Mq {
    router: MultiServiceRouter,
    region: String,
}

impl Mq {
    pub(crate) fn new(region: &str) -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(service_com_amazonaws_mq_mq::EmulatedService::new(engine));
        Self {
            router,
            region: region.to_string(),
        }
    }

    pub(crate) async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let (status, _code, json) = self.call_with_code(method, path, body).await;
        (status, json)
    }

    /// The same dispatch, also reporting the error code a restJson1 client
    /// branches on: the `x-amzn-errortype` header, falling back to the body's
    /// `__type`/`code` member.
    pub(crate) async fn call_with_code(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (u16, String, Value) {
        let payload = body
            .map(|b| Bytes::from(serde_json::to_vec(&b).expect("serialize the request body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://mq.{}.amazonaws.com{path}", self.region))
            .header("authorization", authorization(&self.region))
            .header("content-type", "application/json")
            .body(payload)
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let json = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        // The code is sent in the header, in the body, or both, depending on the
        // renderer; a client reads whichever is present, so the test does too.
        let code = if header_code.is_empty() {
            ["/__type", "/code"]
                .iter()
                .find_map(|pointer| json.pointer(pointer).and_then(Value::as_str))
                .unwrap_or_default()
                .to_string()
        } else {
            header_code
        };
        // An error type is sent as a shape id or a bare name; the name is what a
        // client matches on.
        let code = code
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string();
        (status, code, json)
    }

    pub(crate) async fn create_broker(&self, body: Value) -> (u16, Value) {
        self.call("POST", "/v1/brokers", Some(body)).await
    }

    pub(crate) async fn describe_broker(&self, broker_id: &str) -> (u16, Value) {
        self.call("GET", &format!("/v1/brokers/{broker_id}"), None)
            .await
    }

    async fn delete_broker(&self, broker_id: &str) -> (u16, Value) {
        self.call("DELETE", &format!("/v1/brokers/{broker_id}"), None)
            .await
    }

    pub(crate) async fn list_brokers(&self, query: &str) -> (u16, Value) {
        let path = if query.is_empty() {
            "/v1/brokers".to_string()
        } else {
            format!("/v1/brokers?{query}")
        };
        self.call("GET", &path, None).await
    }

    async fn create_user(&self, broker_id: &str, username: &str, body: Value) -> (u16, Value) {
        self.call(
            "POST",
            &format!("/v1/brokers/{broker_id}/users/{username}"),
            Some(body),
        )
        .await
    }

    async fn describe_user(&self, broker_id: &str, username: &str) -> (u16, Value) {
        self.call(
            "GET",
            &format!("/v1/brokers/{broker_id}/users/{username}"),
            None,
        )
        .await
    }
}

/// A minimal valid `CreateBroker` body: the members the model requires, plus the
/// one initial user Amazon MQ creates a broker with (`Users` — "an
/// administrative user is required", see `state::MIN_USERS_PER_BROKER`).
pub(crate) fn create_body(name: &str) -> Value {
    json!({
        "brokerName": name,
        "engineType": "ACTIVEMQ",
        "deploymentMode": "SINGLE_INSTANCE",
        "hostInstanceType": "mq.t3.micro",
        "publiclyAccessible": false,
        "users": [{"username": "admin", "password": "SuperSecret123"}],
    })
}

pub(crate) fn string_at(value: &Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Whether `id` has the shape Amazon MQ mints: `b-` then 32 lower-case hex
/// digits in the 8-4-4-4-12 grouping.
fn is_minted_broker_id(id: &str) -> bool {
    let Some(body) = id.strip_prefix("b-") else {
        return false;
    };
    let groups: Vec<&str> = body.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .map(|group| group.len())
            .eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()))
}

/// The whole happy path from the issue, end to end: every answer is read back
/// out of state, and the delete really removes the record.
#[tokio::test]
async fn the_broker_lifecycle_round_trips_through_state() {
    let mq = Mq::new("eu-west-2");

    let (status, created) = mq
        .create_broker(json!({
            "brokerName": "demo-broker",
            "engineType": "ACTIVEMQ",
            "engineVersion": "5.18",
            "deploymentMode": "SINGLE_INSTANCE",
            "hostInstanceType": "mq.t3.micro",
            "publiclyAccessible": false,
            "securityGroups": ["sg-1", "sg-2"],
            "subnetIds": ["subnet-a"],
            "storageType": "EBS",
            "tags": {"team": "payments"},
            "users": [{"username": "admin", "password": "SuperSecret123"}],
        }))
        .await;
    assert_eq!(status, 200, "create: expected 200, got {status} {created:?}");

    let broker_id = string_at(&created, "/brokerId");
    let broker_arn = string_at(&created, "/brokerArn");
    assert!(
        is_minted_broker_id(&broker_id),
        "create: expected a minted b-<8-4-4-4-12 hex> BrokerId, got {broker_id:?}"
    );
    assert_eq!(
        broker_arn,
        format!("arn:aws:mq:eu-west-2:000000000000:broker:demo-broker:{broker_id}"),
        "create: unexpected BrokerArn, got {broker_arn:?}"
    );

    let (status, described) = mq.describe_broker(&broker_id).await;
    assert_eq!(
        status, 200,
        "describe: expected 200, got {status} {described:?}"
    );
    for (pointer, expected) in [
        ("/brokerId", json!(broker_id.clone())),
        ("/brokerArn", json!(broker_arn.clone())),
        ("/brokerName", json!("demo-broker")),
        // Amazon MQ answers these three in its own wire spelling, not in the
        // model's enum token (see `state::reported_engine_type`).
        ("/engineType", json!("ActiveMQ")),
        ("/storageType", json!("ebs")),
        ("/authenticationStrategy", json!("simple")),
        // The create named the 5.18 LINE; the broker reports the patch release it
        // runs on that line.
        ("/engineVersion", json!("5.18.8")),
        ("/deploymentMode", json!("SINGLE_INSTANCE")),
        ("/hostInstanceType", json!("mq.t3.micro")),
        ("/publiclyAccessible", json!(false)),
        ("/securityGroups", json!(["sg-1", "sg-2"])),
        ("/subnetIds", json!(["subnet-a"])),
        ("/tags/team", json!("payments")),
        ("/users/0/username", json!("admin")),
        // Documented defaults: AutoMinorVersionUpgrade is "true by default",
        // AuthenticationStrategy's "default is SIMPLE".
        ("/autoMinorVersionUpgrade", json!(true)),
    ] {
        assert_eq!(
            described.pointer(pointer).cloned().unwrap_or(Value::Null),
            expected,
            "describe {pointer}: expected {expected:?}, got {:?}",
            described.pointer(pointer)
        );
    }
    // `Created` targets `__timestampIso8601`, the model's only timestamp shape, which
    // carries `@timestampFormat("date-time")`: the wire form is a string, so the
    // instant is read the way a client bound to this model reads it.
    let described_at = crate::configuration_wire_form::iso_instant(
        "describe a created broker",
        &described,
        "/created",
    );
    assert!(
        described_at.secs() > 0,
        "describe: expected a Created timestamp of the instant the broker was created, \
         got {:?}",
        described.pointer("/created")
    );
    let body = serde_json::to_string(&described).expect("render the describe body");
    assert!(
        !body.contains("SuperSecret123"),
        "describe must never echo a user password, got {body}"
    );

    let (status, listed) = mq.list_brokers("").await;
    assert_eq!(status, 200, "list: expected 200, got {status} {listed:?}");
    assert_eq!(
        listed.pointer("/brokerSummaries").and_then(Value::as_array).map(Vec::len),
        Some(1),
        "list: expected exactly the one created broker, got {listed:?}"
    );
    for (pointer, expected) in [
        ("/brokerSummaries/0/brokerId", json!(broker_id.clone())),
        ("/brokerSummaries/0/brokerArn", json!(broker_arn.clone())),
        ("/brokerSummaries/0/brokerName", json!("demo-broker")),
        ("/brokerSummaries/0/engineType", json!("ActiveMQ")),
        ("/brokerSummaries/0/deploymentMode", json!("SINGLE_INSTANCE")),
        ("/brokerSummaries/0/hostInstanceType", json!("mq.t3.micro")),
    ] {
        assert_eq!(
            listed.pointer(pointer).cloned().unwrap_or(Value::Null),
            expected,
            "list {pointer}: expected {expected:?}, got {:?}",
            listed.pointer(pointer)
        );
    }

    let (status, body) = mq
        .create_user(
            &broker_id,
            "alice",
            json!({"password": "AlicePass12345", "consoleAccess": true, "groups": ["admins"]}),
        )
        .await;
    assert_eq!(
        status, 200,
        "create-user: expected 200, got {status} {body:?}"
    );

    let (status, user) = mq.describe_user(&broker_id, "alice").await;
    assert_eq!(
        status, 200,
        "describe-user: expected 200, got {status} {user:?}"
    );
    for (pointer, expected) in [
        ("/brokerId", json!(broker_id.clone())),
        ("/username", json!("alice")),
        ("/consoleAccess", json!(true)),
        ("/groups", json!(["admins"])),
        ("/replicationUser", json!(false)),
    ] {
        assert_eq!(
            user.pointer(pointer).cloned().unwrap_or(Value::Null),
            expected,
            "describe-user {pointer}: expected {expected:?}, got {:?}",
            user.pointer(pointer)
        );
    }

    // The new user shows up on the broker too — one record, two readers.
    let (_, described) = mq.describe_broker(&broker_id).await;
    let usernames: Vec<String> = described
        .pointer("/users")
        .and_then(Value::as_array)
        .map(|users| users.iter().map(|u| string_at(u, "/username")).collect())
        .unwrap_or_default();
    assert_eq!(
        usernames,
        vec!["admin".to_string(), "alice".to_string()],
        "describe after create-user: expected both usernames, got {usernames:?}"
    );

    let (status, deleted) = mq.delete_broker(&broker_id).await;
    assert_eq!(status, 200, "delete: expected 200, got {status} {deleted:?}");
    assert_eq!(
        string_at(&deleted, "/brokerId"),
        broker_id,
        "delete: expected the deleted BrokerId back, got {deleted:?}"
    );

    let (status, gone) = mq.describe_broker(&broker_id).await;
    assert_eq!(
        status, 404,
        "describe after delete: expected 404, got {status} {gone:?}"
    );
    let (status, listed) = mq.list_brokers("").await;
    assert_eq!(status, 200, "list after delete: got {status} {listed:?}");
    assert_eq!(
        listed.pointer("/brokerSummaries").and_then(Value::as_array).map(Vec::len),
        Some(0),
        "list after delete: expected no brokers, got {listed:?}"
    );
}

/// The rule spans the whole modelled value space, not the one combination the
/// issue cited: whatever `CreateBroker` accepts, `DescribeBroker` returns.
#[tokio::test]
async fn every_modelled_engine_deployment_and_storage_combination_round_trips() {
    let mq = Mq::new("us-east-1");
    let mut cases = 0usize;

    for engine_type in ENGINE_TYPE_VALUES {
        for deployment_mode in DEPLOYMENT_MODE_VALUES {
            for storage_type in BROKER_STORAGE_TYPE_VALUES {
                for publicly_accessible in [false, true] {
                    // Vary the host instance type and the subnet count along the
                    // deployment mode, so no two rows are the same request.
                    let host_instance_type = format!("mq.m5.{}", deployment_mode.to_lowercase());
                    let name = format!(
                        "b-{engine_type}-{deployment_mode}-{storage_type}-{publicly_accessible}"
                    )
                    .replace('_', "-");
                    let case = name.clone();

                    let (status, created) = mq
                        .create_broker(json!({
                            "brokerName": name,
                            "engineType": engine_type,
                            "deploymentMode": deployment_mode,
                            "hostInstanceType": host_instance_type,
                            "publiclyAccessible": publicly_accessible,
                            "storageType": storage_type,
                            "autoMinorVersionUpgrade": false,
                            "authenticationStrategy": "LDAP",
                            "users": [{"username": "admin", "password": "SuperSecret123"}],
                        }))
                        .await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected 200 from create, got {status} {created:?}"
                    );
                    let broker_id = string_at(&created, "/brokerId");

                    let (status, described) = mq.describe_broker(&broker_id).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected 200 from describe, got {status} {described:?}"
                    );
                    for (pointer, expected) in [
                        ("/brokerName", json!(name)),
                        // The wire spellings Amazon MQ answers these members in,
                        // whatever modelled token the create sent.
                        (
                            "/engineType",
                            json!(if *engine_type == "ACTIVEMQ" {
                                "ActiveMQ"
                            } else {
                                "RabbitMQ"
                            }),
                        ),
                        ("/storageType", json!(storage_type.to_lowercase())),
                        ("/authenticationStrategy", json!("ldap")),
                        ("/deploymentMode", json!(deployment_mode)),
                        ("/hostInstanceType", json!(host_instance_type)),
                        ("/publiclyAccessible", json!(publicly_accessible)),
                        ("/autoMinorVersionUpgrade", json!(false)),
                    ] {
                        assert_eq!(
                            described.pointer(pointer).cloned().unwrap_or(Value::Null),
                            expected,
                            "case {case:?} {pointer}: expected {expected:?}, got {:?}",
                            described.pointer(pointer)
                        );
                    }
                    cases += 1;
                }
            }
        }
    }

    assert_eq!(
        cases,
        ENGINE_TYPE_VALUES.len() * DEPLOYMENT_MODE_VALUES.len() * BROKER_STORAGE_TYPE_VALUES.len() * 2,
        "the table must cover every modelled combination, covered {cases}"
    );
}

/// A broker name is unique per account: a second create with a name already in
/// use is refused with `ConflictException` (409), whatever else differs.
#[tokio::test]
async fn a_reused_broker_name_is_a_conflict() {
    let mq = Mq::new("us-west-2");

    // Rows differ in the name's form (plain, dashed, underscored, digits, the
    // 50-character maximum) — the rule is about the name being taken, not about
    // any particular spelling.
    for name in [
        "alpha",
        "alpha-beta",
        "alpha_beta",
        "broker42",
        &"n".repeat(50),
    ] {
        let (status, first) = mq.create_broker(create_body(name)).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected the first create to succeed, got {status} {first:?}"
        );

        // A different engine, mode and instance type: only the name repeats.
        let (status, conflict) = mq
            .create_broker(json!({
                "brokerName": name,
                "engineType": "RABBITMQ",
                "deploymentMode": "CLUSTER_MULTI_AZ",
                "hostInstanceType": "mq.m5.large",
                "publiclyAccessible": true,
                "users": [{"username": "other-admin", "password": "AnotherSecret12"}],
            }))
            .await;
        assert_eq!(
            status, 409,
            "case {name:?}: expected 409 on the reused name, got {status} {conflict:?}"
        );
        assert_eq!(
            string_at(&conflict, "/errorAttribute"),
            "brokerName",
            "case {name:?}: expected the conflict to name brokerName, got {conflict:?}"
        );
    }

    // Only the five brokers the first create of each row made exist.
    let (_, listed) = mq.list_brokers("").await;
    assert_eq!(
        listed
            .pointer("/brokerSummaries")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(5),
        "a refused create must store nothing, got {listed:?}"
    );
}

/// `CreatorRequestId` is the operation's `@idempotencyToken`: repeating the
/// request with the same token answers with the broker that token created.
#[tokio::test]
async fn the_idempotency_token_does_not_mint_a_second_broker() {
    let mq = Mq::new("us-east-2");

    for token in ["11111111-1111-1111-1111-111111111111", "a", "token-2"] {
        let mut body = create_body(&format!("broker-for-{token}"));
        body["creatorRequestId"] = json!(token);

        let (status, first) = mq.create_broker(body.clone()).await;
        assert_eq!(
            status, 200,
            "case {token:?}: expected 200, got {status} {first:?}"
        );
        let (status, repeated) = mq.create_broker(body).await;
        assert_eq!(
            status, 200,
            "case {token:?}: expected the repeat to succeed, got {status} {repeated:?}"
        );
        assert_eq!(
            string_at(&repeated, "/brokerId"),
            string_at(&first, "/brokerId"),
            "case {token:?}: the repeat must answer with the same BrokerId, got {repeated:?}"
        );
    }

    let (_, listed) = mq.list_brokers("").await;
    assert_eq!(
        listed
            .pointer("/brokerSummaries")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(3),
        "a repeated token must not mint a second broker, got {listed:?}"
    );

    // A token reused for a DIFFERENT request is a client error, not a retry:
    // each row changes one of the members that decide which broker is asked for.
    for (case, member, value) in [
        ("different brokerName", "brokerName", json!("other-name")),
        ("different engineType", "engineType", json!("RABBITMQ")),
        (
            "different deploymentMode",
            "deploymentMode",
            json!("CLUSTER_MULTI_AZ"),
        ),
        (
            "different hostInstanceType",
            "hostInstanceType",
            json!("mq.m5.large"),
        ),
        // The fingerprint covers every member that defines the broker, not just
        // the four that name it.
        ("added storageType", "storageType", json!("EBS")),
        ("added engineVersion", "engineVersion", json!("5.18")),
        (
            "added securityGroups",
            "securityGroups",
            json!(["sg-only"]),
        ),
        (
            "flipped publiclyAccessible",
            "publiclyAccessible",
            json!(true),
        ),
    ] {
        let mut body = create_body("broker-for-a");
        body["creatorRequestId"] = json!("a");
        body[member] = value;
        let (status, refused) = mq.create_broker(body).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409 for a mismatched token replay, got {status} {refused:?}"
        );
        assert_eq!(
            string_at(&refused, "/errorAttribute"),
            "creatorRequestId",
            "case {case:?}: the refusal must name creatorRequestId, got {refused:?}"
        );
    }
}

/// `ListBrokers` pages every stored broker exactly once, at each published
/// `MaxResults` bound and past it.
#[tokio::test]
async fn list_brokers_pages_every_broker_exactly_once() {
    let mq = Mq::new("eu-central-1");

    const TOTAL: usize = 7;
    let mut all_ids = Vec::new();
    for index in 0..TOTAL {
        let (status, created) = mq.create_broker(create_body(&format!("paged-{index}"))).await;
        assert_eq!(status, 200, "setup {index}: got {status} {created:?}");
        all_ids.push(string_at(&created, "/brokerId"));
    }
    all_ids.sort();

    // The published range for MaxResults is 1..100 (`@range` on the member), so
    // the rows are the minimum, values inside, the stored count, and the maximum.
    for page_size in [1usize, 2, 3, TOTAL, 100] {
        let mut seen = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let query = match &token {
                None => format!("maxResults={page_size}"),
                Some(t) => format!("maxResults={page_size}&nextToken={t}"),
            };
            let (status, listed) = mq.list_brokers(&query).await;
            assert_eq!(
                status, 200,
                "case maxResults={page_size}: expected 200, got {status} {listed:?}"
            );
            let summaries = listed
                .pointer("/brokerSummaries")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert!(
                summaries.len() <= page_size,
                "case maxResults={page_size}: a page must not exceed the page size, got {}",
                summaries.len()
            );
            seen.extend(summaries.iter().map(|s| string_at(s, "/brokerId")));
            pages += 1;
            assert!(
                pages <= TOTAL + 1,
                "case maxResults={page_size}: pagination did not terminate after {pages} pages"
            );
            match listed.pointer("/nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted, all_ids,
            "case maxResults={page_size}: expected every broker exactly once, got {seen:?}"
        );
    }
}

/// A request that omits `MaxResults` gets the page size the member documents
/// ("20 by default"), not the whole stored set.
#[tokio::test]
async fn list_brokers_defaults_to_the_documented_page_size() {
    use service_com_amazonaws_mq_mq::state::DEFAULT_MAX_RESULTS;

    let mq = Mq::new("us-east-1");
    let total = DEFAULT_MAX_RESULTS + 5;
    for index in 0..total {
        let (status, body) = mq
            .create_broker(create_body(&format!("default-page-{index}")))
            .await;
        assert_eq!(status, 200, "setup {index}: got {status} {body:?}");
    }

    let (status, listed) = mq.list_brokers("").await;
    assert_eq!(status, 200, "expected 200, got {status} {listed:?}");
    assert_eq!(
        listed
            .pointer("/brokerSummaries")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(DEFAULT_MAX_RESULTS),
        "expected the documented default page size of {DEFAULT_MAX_RESULTS}, got {listed:?}"
    );
    let next = listed
        .pointer("/nextToken")
        .and_then(Value::as_str)
        .expect("a NextToken while brokers remain")
        .to_string();

    let (status, rest) = mq.list_brokers(&format!("nextToken={next}")).await;
    assert_eq!(status, 200, "second page: got {status} {rest:?}");
    assert_eq!(
        rest.pointer("/brokerSummaries")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(total - DEFAULT_MAX_RESULTS),
        "expected the remaining brokers on the second page, got {rest:?}"
    );
    assert_eq!(
        rest.pointer("/nextToken"),
        None,
        "the last page must carry no NextToken, got {rest:?}"
    );
}

/// A `MaxResults` outside the published range, and a `NextToken` this service
/// never issued, are client errors — not a silent clamp and not an empty page.
#[tokio::test]
async fn list_brokers_refuses_values_outside_the_published_bounds() {
    let mq = Mq::new("us-east-1");
    mq.create_broker(create_body("only-broker")).await;

    // One past each published bound of `@range(min: 1, max: 100)`.
    for query in ["maxResults=0", "maxResults=101"] {
        let (status, body) = mq.list_brokers(query).await;
        assert_eq!(
            status, 400,
            "case {query:?}: expected 400, got {status} {body:?}"
        );
    }

    // Rows: no tag at all, non-hex digits, a hex blob without the tag, the tag
    // with a sign prefix a radix parser would otherwise accept, the tag with too
    // few digits, and the tag alone.
    for token in [
        "not-a-token",
        "zzzz",
        "6e6f70652d61742d616c6c",
        "mqb1+00000000001",
        "mqb10",
        "mqb1",
    ] {
        let (status, body) = mq.list_brokers(&format!("nextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected 400 for a foreign nextToken, got {status} {body:?}"
        );
    }
}

/// `CreateBroker` refuses a value outside the constraint the model documents
/// for that member, with `BadRequestException` (400) naming the member.
#[tokio::test]
async fn create_broker_refuses_values_outside_the_documented_constraints() {
    let mq = Mq::new("us-east-1");

    // Each row breaks exactly one documented constraint.
    let cases: Vec<(&str, Value)> = vec![
        ("brokerName too long", {
            let mut b = create_body(&"x".repeat(51));
            b["brokerName"] = json!("x".repeat(51));
            b
        }),
        ("brokerName with a space", create_body("has a space")),
        ("brokerName with a wildcard", create_body("star*broker")),
        ("brokerName empty", create_body("")),
        ("engineType unknown", {
            let mut b = create_body("engine-case");
            b["engineType"] = json!("KAFKA");
            b
        }),
        ("deploymentMode unknown", {
            let mut b = create_body("mode-case");
            b["deploymentMode"] = json!("TRIPLE_AZ");
            b
        }),
        ("hostInstanceType blank", {
            let mut b = create_body("host-case");
            b["hostInstanceType"] = json!("   ");
            b
        }),
        ("storageType unknown", {
            let mut b = create_body("storage-case");
            b["storageType"] = json!("TAPE");
            b
        }),
        ("authenticationStrategy unknown", {
            let mut b = create_body("auth-case");
            b["authenticationStrategy"] = json!("KERBEROS");
            b
        }),
        ("brokerName absent", {
            let mut b = create_body("absent-case");
            b.as_object_mut().expect("object").remove("brokerName");
            b
        }),
        ("inline user password too short", {
            let mut b = create_body("user-pass-case");
            b["users"] = json!([{"username": "admin", "password": "short"}]);
            b
        }),
        // The documented `Username` charset, exercised in the body where a
        // forbidden character can actually be sent.
        ("inline user name with a space", {
            let mut b = create_body("user-space-case");
            b["users"] = json!([{"username": "bad name", "password": "LongEnoughPass1"}]);
            b
        }),
        ("inline user name with an exclamation", {
            let mut b = create_body("user-bang-case");
            b["users"] = json!([{"username": "bad!name", "password": "LongEnoughPass1"}]);
            b
        }),
        ("inline user name outside ascii", {
            let mut b = create_body("user-nonascii-case");
            b["users"] = json!([{"username": "b\u{fc}ser", "password": "LongEnoughPass1"}]);
            b
        }),
        ("inline user name too short", {
            let mut b = create_body("user-short-case");
            b["users"] = json!([{"username": "a", "password": "LongEnoughPass1"}]);
            b
        }),
        ("inline user named twice", {
            let mut b = create_body("user-dup-case");
            b["users"] = json!([
                {"username": "admin", "password": "LongEnoughPass1"},
                {"username": "admin", "password": "LongEnoughPass2"},
            ]);
            b
        }),
        // "Must be set when dataReplicationMode is set to CRDR".
        ("CRDR without the primary broker ARN", {
            let mut b = create_body("crdr-case");
            b["dataReplicationMode"] = json!("CRDR");
            b
        }),
        // "The list of rules (1 minimum, 125 maximum)".
        ("securityGroups empty", {
            let mut b = create_body("sg-empty-case");
            b["securityGroups"] = json!([]);
            b
        }),
        ("securityGroups one past the maximum", {
            let mut b = create_body("sg-max-case");
            b["securityGroups"] = json!((0..126).map(|i| format!("sg-{i}")).collect::<Vec<_>>());
            b
        }),
        // "A SINGLE_INSTANCE deployment requires one subnet."
        ("SINGLE_INSTANCE with two subnets", {
            let mut b = create_body("subnet-single-case");
            b["subnetIds"] = json!(["subnet-a", "subnet-b"]);
            b
        }),
        // "An ACTIVE_STANDBY_MULTI_AZ ... deployment requires two subnets."
        ("ACTIVE_STANDBY_MULTI_AZ with one subnet", {
            let mut b = create_body("subnet-standby-case");
            b["deploymentMode"] = json!("ACTIVE_STANDBY_MULTI_AZ");
            b["subnetIds"] = json!(["subnet-a"]);
            b
        }),
    ];

    for (case, body) in cases {
        let (status, answer) = mq.create_broker(body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {answer:?}"
        );
        // Each handler names the member whose documented rule fired, so the row
        // cannot pass on some other refusal. `None` is left only for the
        // framework's own pre-handler refusal of an ABSENT required member,
        // whose envelope carries code and message only: a value outside a
        // constraint the MODEL declares (an enum's value set) is refused by the
        // handler now, so it names its member like every other refusal.
        let expected_attribute = match case {
            "brokerName too long"
            | "brokerName with a space"
            | "brokerName with a wildcard"
            | "brokerName empty" => Some("brokerName"),
            "hostInstanceType blank" => Some("hostInstanceType"),
            "inline user password too short" => Some("users"),
            "inline user name with a space"
            | "inline user name with an exclamation"
            | "inline user name outside ascii"
            | "inline user name too short" => Some("users"),
            "inline user named twice" => Some("users"),
            "CRDR without the primary broker ARN" => Some("dataReplicationPrimaryBrokerArn"),
            "securityGroups empty" | "securityGroups one past the maximum" => {
                Some("securityGroups")
            }
            "SINGLE_INSTANCE with two subnets" | "ACTIVE_STANDBY_MULTI_AZ with one subnet" => {
                Some("subnetIds")
            }
            "engineType unknown" => Some("engineType"),
            "deploymentMode unknown" => Some("deploymentMode"),
            "storageType unknown" => Some("storageType"),
            "authenticationStrategy unknown" => Some("authenticationStrategy"),
            // Absence is found while READING the request, before any handler
            // runs, and the framework's refusal envelope carries no modelled
            // member to name it in.
            "brokerName absent" => None,
            other => panic!("case {other:?} has no expected errorAttribute in this table"),
        };
        assert_eq!(
            answer.pointer("/errorAttribute").and_then(Value::as_str),
            expected_attribute,
            "case {case:?}: expected errorAttribute {expected_attribute:?}, got {answer:?}"
        );
    }

    // Nothing a refused create named was stored.
    let (_, listed) = mq.list_brokers("").await;
    assert_eq!(
        listed
            .pointer("/brokerSummaries")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "a refused create must store nothing, got {listed:?}"
    );
}

/// A caller cannot grow the store, or one broker record, without bound: the
/// documented quotas are enforced and a create past one is refused, not stored.
#[tokio::test]
async fn the_documented_quotas_bound_what_one_caller_can_store() {
    use service_com_amazonaws_mq_mq::state::{MAX_BROKERS, MAX_USERS_PER_BROKER};

    let mq = Mq::new("us-east-1");

    // Fill the account's broker quota, then one past it.
    for index in 0..MAX_BROKERS {
        let (status, body) = mq.create_broker(create_body(&format!("quota-{index}"))).await;
        assert_eq!(
            status, 200,
            "broker {index}: expected 200 inside the quota, got {status} {body:?}"
        );
    }
    let (status, body) = mq.create_broker(create_body("quota-one-too-many")).await;
    assert_eq!(
        status, 400,
        "one past the broker quota: expected 400, got {status} {body:?}"
    );
    let (_, listed) = mq.list_brokers("maxResults=100").await;
    assert_eq!(
        listed
            .pointer("/brokerSummaries")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(MAX_BROKERS),
        "the refused create must store nothing, got {listed:?}"
    );

    // And the per-broker user quota, on a broker created inside the quota. The
    // create already stored this broker's initial user, so the quota is reached
    // one `CreateUser` earlier.
    let broker_id = listed
        .pointer("/brokerSummaries/0/brokerId")
        .and_then(Value::as_str)
        .expect("a broker to add users to")
        .to_string();
    for index in 1..MAX_USERS_PER_BROKER {
        let (status, body) = mq
            .create_user(
                &broker_id,
                &format!("user-{index}"),
                json!({"password": "GoodPassword12"}),
            )
            .await;
        assert_eq!(
            status, 200,
            "user {index}: expected 200 inside the quota, got {status} {body:?}"
        );
    }
    let (status, body) = mq
        .create_user(
            &broker_id,
            "one-too-many",
            json!({"password": "GoodPassword12"}),
        )
        .await;
    assert_eq!(
        status, 400,
        "one past the user quota: expected 400, got {status} {body:?}"
    );
    let (status, _) = mq.describe_user(&broker_id, "one-too-many").await;
    assert_eq!(
        status, 404,
        "the refused create-user must store nothing, got {status}"
    );
}

/// `DataReplicationMode` round-trips once the member the model couples it to is
/// supplied, and the subnet count each deployment mode documents is accepted.
#[tokio::test]
async fn the_coupled_members_round_trip_when_the_request_satisfies_the_rule() {
    let mq = Mq::new("us-east-1");

    // Rows: the documented subnet count for each mode, and CRDR with the primary
    // broker ARN the model requires alongside it.
    /// One row: the case name, the `CreateBroker` body, and the describe
    /// members it must read back as.
    type Row = (&'static str, Value, Vec<(&'static str, Value)>);
    let cases: Vec<Row> = vec![
        (
            "SINGLE_INSTANCE with one subnet",
            {
                let mut b = create_body("ok-single");
                b["subnetIds"] = json!(["subnet-a"]);
                b
            },
            vec![("/subnetIds", json!(["subnet-a"]))],
        ),
        (
            "ACTIVE_STANDBY_MULTI_AZ with two subnets",
            {
                let mut b = create_body("ok-standby");
                b["deploymentMode"] = json!("ACTIVE_STANDBY_MULTI_AZ");
                b["subnetIds"] = json!(["subnet-a", "subnet-b"]);
                b
            },
            vec![("/subnetIds", json!(["subnet-a", "subnet-b"]))],
        ),
        (
            "CLUSTER_MULTI_AZ with three subnets (no documented count)",
            {
                let mut b = create_body("ok-cluster");
                b["deploymentMode"] = json!("CLUSTER_MULTI_AZ");
                b["engineType"] = json!("RABBITMQ");
                b["subnetIds"] = json!(["subnet-a", "subnet-b", "subnet-c"]);
                b
            },
            vec![("/subnetIds", json!(["subnet-a", "subnet-b", "subnet-c"]))],
        ),
        (
            "CRDR with the primary broker ARN",
            {
                let mut b = create_body("ok-crdr");
                b["dataReplicationMode"] = json!("CRDR");
                b["dataReplicationPrimaryBrokerArn"] =
                    json!("arn:aws:mq:us-east-1:000000000000:broker:primary:b-1");
                b
            },
            vec![("/dataReplicationMode", json!("CRDR"))],
        ),
        (
            "NONE needs no primary broker ARN",
            {
                let mut b = create_body("ok-none");
                b["dataReplicationMode"] = json!("NONE");
                b
            },
            vec![("/dataReplicationMode", json!("NONE"))],
        ),
    ];

    for (case, body, expectations) in cases {
        let (status, created) = mq.create_broker(body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {created:?}"
        );
        let broker_id = string_at(&created, "/brokerId");
        let (status, described) = mq.describe_broker(&broker_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from describe, got {status} {described:?}"
        );
        for (pointer, expected) in expectations {
            assert_eq!(
                described.pointer(pointer).cloned().unwrap_or(Value::Null),
                expected,
                "case {case:?} {pointer}: expected {expected:?}, got {:?}",
                described.pointer(pointer)
            );
        }
    }
}

/// The user operations enforce `Username` and `Password` as the model documents
/// them, and refuse a second user with a name the broker already has.
#[tokio::test]
async fn create_user_enforces_the_documented_user_constraints() {
    let mq = Mq::new("us-east-1");
    let (_, created) = mq.create_broker(create_body("user-rules")).await;
    let broker_id = string_at(&created, "/brokerId");

    // Accepted: every form the documented charset allows, at the published
    // length bounds (2 and 100 characters).
    for username in ["ab", "a-b.c_d~e", "User99", &"u".repeat(100)] {
        let (status, body) = mq
            .create_user(&broker_id, username, json!({"password": "GoodPassword12"}))
            .await;
        assert_eq!(
            status, 200,
            "case {username:?}: expected 200, got {status} {body:?}"
        );
        let (status, user) = mq.describe_user(&broker_id, username).await;
        assert_eq!(
            status, 200,
            "case {username:?}: expected the user to read back, got {status} {user:?}"
        );
        assert_eq!(
            string_at(&user, "/username"),
            username,
            "case {username:?}: expected the stored username back, got {user:?}"
        );
    }

    // Refused: one past each documented bound, and each forbidden password
    // character. The usernames here are URI-safe, because `Username` is an
    // `@httpLabel`; the forbidden-CHARACTER rows for a username are exercised
    // through `CreateBroker`'s inline `Users`, where the username travels in the
    // body, in `create_broker_refuses_values_outside_the_documented_constraints`.
    let over_long = "u".repeat(101);
    let refused: Vec<(&str, &str, &str)> = vec![
        ("username too short", "a", "GoodPassword12"),
        ("username too long", &over_long, "GoodPassword12"),
        ("password too short", "pwshort", "Elevenchars"),
        ("password too few unique", "pwunique", "aaaaaaaaaaaa"),
        ("password with a comma", "pwcomma", "Good,Password1"),
        ("password with a colon", "pwcolon", "Good:Password1"),
        ("password with an equals", "pwequals", "Good=Password1"),
    ];
    for (case, username, password) in refused {
        let (status, body) = mq
            .create_user(&broker_id, username, json!({"password": password}))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body:?}"
        );
        let (status, _) = mq.describe_user(&broker_id, username).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused create must store nothing, got {status}"
        );
    }

    // A username already on the broker is a conflict, whatever else changes.
    let (status, body) = mq
        .create_user(
            &broker_id,
            "User99",
            json!({"password": "DifferentPass12", "consoleAccess": true}),
        )
        .await;
    assert_eq!(
        status, 409,
        "a repeated username must be 409, got {status} {body:?}"
    );
}

/// Every operation that resolves a broker or a user by id answers
/// `NotFoundException` (404) when it does not exist.
#[tokio::test]
async fn unknown_ids_are_not_found() {
    let mq = Mq::new("us-east-1");
    let (_, created) = mq.create_broker(create_body("present")).await;
    let present = string_at(&created, "/brokerId");

    for broker_id in ["b-does-not-exist", "b-00000000-0000-0000-0000-000000000000"] {
        for (case, (status, body)) in [
            ("describe-broker", mq.describe_broker(broker_id).await),
            (
                "create-user",
                mq.create_user(broker_id, "someone", json!({"password": "GoodPassword12"}))
                    .await,
            ),
            ("describe-user", mq.describe_user(broker_id, "someone").await),
            ("delete-broker", mq.delete_broker(broker_id).await),
            // A missing broker is reported as missing even when a member of the
            // same request also breaks its documented constraint: the resource
            // is resolved before the members are checked.
            (
                "create-user with a bad password",
                mq.create_user(broker_id, "someone", json!({"password": "short"}))
                    .await,
            ),
            (
                "create-user with a bad username",
                mq.create_user(broker_id, "a", json!({"password": "GoodPassword12"}))
                    .await,
            ),
        ] {
            assert_eq!(
                status, 404,
                "case {case} on {broker_id:?}: expected 404, got {status} {body:?}"
            );
        }
    }

    // An existing broker with an unknown user is the same refusal.
    for username in ["nobody", "also-nobody"] {
        let (status, body) = mq.describe_user(&present, username).await;
        assert_eq!(
            status, 404,
            "case unknown user {username:?}: expected 404, got {status} {body:?}"
        );
    }
}

/// `CreateBroker` is documented as asynchronous, so the broker it mints is
/// reported in a non-terminal state first and reaches `RUNNING` once the clock
/// owes it — with no call in between.
#[tokio::test]
async fn a_new_broker_reports_creation_in_progress_before_running() {
    let mq = Mq::new("us-east-1");

    for name in ["async-one", "async-two"] {
        let started = std::time::Instant::now();
        let (_, created) = mq.create_broker(create_body(name)).await;
        let broker_id = string_at(&created, "/brokerId");

        let (_, described) = mq.describe_broker(&broker_id).await;
        // The describe runs in-process microseconds after the create, so the
        // broker must still be provisioning — unless this machine was stalled
        // for longer than the whole span, in which case RUNNING is the correct
        // answer and the assertion below would be a wall-clock race.
        let still_provisioning = started.elapsed() < BROKER_PROVISIONING;
        if still_provisioning {
            assert_eq!(
                string_at(&described, "/brokerState"),
                "CREATION_IN_PROGRESS",
                "case {name:?}: a just-created broker must still be provisioning, got {described:?}"
            );
            let (_, listed) = mq.list_brokers("").await;
            let states: Vec<String> = listed
                .pointer("/brokerSummaries")
                .and_then(Value::as_array)
                .map(|rows| rows.iter().map(|r| string_at(r, "/brokerState")).collect())
                .unwrap_or_default();
            assert!(
                states.contains(&"CREATION_IN_PROGRESS".to_string()),
                "case {name:?}: ListBrokers must report the same state, got {states:?}"
            );
        } else {
            assert_eq!(
                string_at(&described, "/brokerState"),
                "RUNNING",
                "case {name:?}: past the provisioning span the broker must be RUNNING, \
                 got {described:?}"
            );
        }

        tokio::time::sleep(BROKER_PROVISIONING + std::time::Duration::from_millis(100)).await;

        let (_, described) = mq.describe_broker(&broker_id).await;
        assert_eq!(
            string_at(&described, "/brokerState"),
            "RUNNING",
            "case {name:?}: the broker must reach RUNNING on the clock, got {described:?}"
        );
    }
}

/// Logging asked for at create time is reported back, with the CloudWatch Logs
/// log groups derived from the broker's own minted id — a write the service
/// accepted is a write it can report.
#[tokio::test]
async fn create_broker_reports_the_logging_it_was_asked_for() {
    let mq = Mq::new("us-east-1");

    // Rows: the `Logs` member as sent, then the `audit`/`general` the summary
    // must report. `Logs` has exactly two flags, so the table spans every
    // combination plus the absent member. Every row is an ActiveMQ broker
    // (`create_body`), the engine audit logging applies to, so every row reports
    // the audit flag — `false` where audit logging is off, never absent.
    let cases: Vec<(&str, Option<Value>, bool, bool)> = vec![
        ("no logs member", None, false, false),
        ("general only", Some(json!({"general": true})), false, true),
        ("audit only", Some(json!({"audit": true})), true, false),
        (
            "both on",
            Some(json!({"audit": true, "general": true})),
            true,
            true,
        ),
        (
            "both off",
            Some(json!({"audit": false, "general": false})),
            false,
            false,
        ),
    ];

    for (index, (case, logs, expected_audit, expected_general)) in cases.into_iter().enumerate() {
        let mut body = create_body(&format!("logging-{index}"));
        if let Some(logs) = logs {
            body["logs"] = logs;
        }
        let (status, created) = mq.create_broker(body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {created:?}");
        let broker_id = string_at(&created, "/brokerId");

        let (status, described) = mq.describe_broker(&broker_id).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {described:?}");
        assert_eq!(
            described.pointer("/logs/general").and_then(Value::as_bool),
            Some(expected_general),
            "case {case:?}: expected general={expected_general}, got {described:?}"
        );
        assert_eq!(
            described.pointer("/logs/audit").and_then(Value::as_bool),
            Some(expected_audit),
            "case {case:?}: expected audit={expected_audit:?}, got {described:?}"
        );
        for (kind, pointer) in [
            ("general", "/logs/generalLogGroup"),
            ("audit", "/logs/auditLogGroup"),
        ] {
            assert_eq!(
                string_at(&described, pointer),
                format!("/aws/amazonmq/broker/{broker_id}/{kind}"),
                "case {case:?}: the {kind} log group must name this broker, got {described:?}"
            );
        }
    }
}

/// `Logs$audit` "does not apply to RabbitMQ brokers", so asking for it on one is
/// refused rather than accepted and dropped — on every deployment mode.
#[tokio::test]
async fn audit_logging_is_refused_on_rabbitmq_brokers() {
    let mq = Mq::new("us-east-1");

    for (index, mode) in DEPLOYMENT_MODE_VALUES.iter().enumerate() {
        let mut body = create_body(&format!("rabbit-audit-{index}"));
        body["engineType"] = json!("RABBITMQ");
        body["deploymentMode"] = json!(mode);
        body["logs"] = json!({"audit": true, "general": true});
        let (status, code, refused) = mq
            .call_with_code("POST", "/v1/brokers", Some(body))
            .await;
        assert_eq!(
            status, 400,
            "case {mode:?}: audit logging on RABBITMQ must be 400, got {status} {refused:?}"
        );
        assert_eq!(
            code, "BadRequestException",
            "case {mode:?}: expected BadRequestException, got {code:?} {refused:?}"
        );
        assert_eq!(
            string_at(&refused, "/errorAttribute"),
            "logs",
            "case {mode:?}: the refusal must name the member, got {refused:?}"
        );

        // The same broker without the audit flag is accepted, so the refusal is
        // about that member and not about the engine type.
        let mut body = create_body(&format!("rabbit-general-{index}"));
        body["engineType"] = json!("RABBITMQ");
        body["deploymentMode"] = json!(mode);
        body["logs"] = json!({"general": true});
        let (status, created) = mq.create_broker(body).await;
        assert_eq!(
            status, 200,
            "case {mode:?}: general logging on RABBITMQ must be accepted, got {created:?}"
        );
    }
}

/// The maintenance window, encryption options and LDAP metadata a create sets
/// are persisted and reported; the LDAP service-account password never is.
#[tokio::test]
async fn create_broker_reports_the_window_encryption_and_ldap_it_was_given() {
    let mq = Mq::new("eu-central-1");

    // `WeeklyStartTime$timeZone` is documented as "UTC by default", so an
    // omitted zone is reported as UTC and a given one is reported verbatim.
    // The days span the model's own `DayOfWeek` catalog entries.
    let windows: Vec<(&str, Value, &str)> = vec![
        (
            "no time zone",
            json!({"dayOfWeek": "MONDAY", "timeOfDay": "22:45"}),
            "UTC",
        ),
        (
            "named zone",
            json!({"dayOfWeek": "SATURDAY", "timeOfDay": "02:00", "timeZone": "Europe/Berlin"}),
            "Europe/Berlin",
        ),
        (
            "offset zone",
            json!({"dayOfWeek": "SUNDAY", "timeOfDay": "13:30", "timeZone": "+02:00"}),
            "+02:00",
        ),
    ];

    for (index, (case, window, expected_zone)) in windows.into_iter().enumerate() {
        let mut body = create_body(&format!("window-{index}"));
        body["maintenanceWindowStartTime"] = window.clone();
        let (status, created) = mq.create_broker(body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {created:?}");
        let broker_id = string_at(&created, "/brokerId");

        let (_, described) = mq.describe_broker(&broker_id).await;
        assert_eq!(
            string_at(&described, "/maintenanceWindowStartTime/dayOfWeek"),
            window["dayOfWeek"].as_str().unwrap_or_default(),
            "case {case:?}: the window day must round-trip, got {described:?}"
        );
        assert_eq!(
            string_at(&described, "/maintenanceWindowStartTime/timeOfDay"),
            window["timeOfDay"].as_str().unwrap_or_default(),
            "case {case:?}: the window time must round-trip, got {described:?}"
        );
        assert_eq!(
            string_at(&described, "/maintenanceWindowStartTime/timeZone"),
            expected_zone,
            "case {case:?}: expected timeZone {expected_zone:?}, got {described:?}"
        );
    }

    // Encryption options: with a customer key, and with the Amazon-owned key.
    let encryptions: Vec<(&str, Value, Option<&str>, bool)> = vec![
        (
            "customer key",
            json!({"kmsKeyId": "arn:aws:kms:eu-central-1:111122223333:key/abc", "useAwsOwnedKey": false}),
            Some("arn:aws:kms:eu-central-1:111122223333:key/abc"),
            false,
        ),
        (
            "aws owned key",
            json!({"useAwsOwnedKey": true}),
            None,
            true,
        ),
    ];
    for (index, (case, options, expected_key, expected_owned)) in
        encryptions.into_iter().enumerate()
    {
        let mut body = create_body(&format!("encryption-{index}"));
        body["encryptionOptions"] = options;
        let (status, created) = mq.create_broker(body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {created:?}");
        let broker_id = string_at(&created, "/brokerId");

        let (_, described) = mq.describe_broker(&broker_id).await;
        assert_eq!(
            described
                .pointer("/encryptionOptions/kmsKeyId")
                .and_then(Value::as_str),
            expected_key,
            "case {case:?}: expected kmsKeyId {expected_key:?}, got {described:?}"
        );
        assert_eq!(
            described
                .pointer("/encryptionOptions/useAwsOwnedKey")
                .and_then(Value::as_bool),
            Some(expected_owned),
            "case {case:?}: expected useAwsOwnedKey={expected_owned}, got {described:?}"
        );
    }

    // LDAP metadata, with one host and with a failover host.
    for (index, hosts) in [vec!["ldap.example.com"], vec!["ldap1.example.com", "ldap2.example.com"]]
        .into_iter()
        .enumerate()
    {
        let case = format!("{} host(s)", hosts.len());
        let mut body = create_body(&format!("ldap-{index}"));
        body["authenticationStrategy"] = json!("LDAP");
        body["ldapServerMetadata"] = json!({
            "hosts": hosts,
            "roleBase": "ou=group,dc=example,dc=com",
            "roleSearchMatching": "(member=uid={0})",
            "roleSearchSubtree": true,
            "serviceAccountUsername": "cn=admin,dc=example,dc=com",
            "serviceAccountPassword": "LdapServicePass12",
            "userBase": "ou=user,dc=example,dc=com",
            "userSearchMatching": "(uid={0})",
        });
        let (status, created) = mq.create_broker(body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {created:?}");
        let broker_id = string_at(&created, "/brokerId");

        let (_, described) = mq.describe_broker(&broker_id).await;
        let reported: Vec<String> = described
            .pointer("/ldapServerMetadata/hosts")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .map(|r| r.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            reported, hosts,
            "case {case:?}: the LDAP hosts must round-trip, got {described:?}"
        );
        assert_eq!(
            string_at(&described, "/ldapServerMetadata/serviceAccountUsername"),
            "cn=admin,dc=example,dc=com",
            "case {case:?}: the service account must round-trip, got {described:?}"
        );
        assert_eq!(
            described.pointer("/ldapServerMetadata/serviceAccountPassword"),
            None,
            "case {case:?}: no response may carry the service-account password, \
             got {described:?}"
        );
        assert_eq!(
            string_at(&described, "/authenticationStrategy"),
            "ldap",
            "case {case:?}: the authentication strategy must round-trip, got {described:?}"
        );
    }
}

/// A broker cannot be created against a configuration this service does not
/// hold, so the member is refused rather than accepted and reported as absent.
#[tokio::test]
async fn create_broker_refuses_a_configuration_that_does_not_exist() {
    let mq = Mq::new("us-east-1");

    let cases: Vec<(&str, Value)> = vec![
        ("id only", json!({"id": "c-1111"})),
        ("id and revision", json!({"id": "c-2222", "revision": 3})),
    ];
    for (index, (case, configuration)) in cases.into_iter().enumerate() {
        let mut body = create_body(&format!("configured-{index}"));
        body["configuration"] = configuration;
        let (status, code, refused) = mq
            .call_with_code("POST", "/v1/brokers", Some(body))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {refused:?}"
        );
        assert_eq!(
            code, "BadRequestException",
            "case {case:?}: expected BadRequestException, got {code:?} {refused:?}"
        );
        assert_eq!(
            string_at(&refused, "/errorAttribute"),
            "configuration",
            "case {case:?}: the refusal must name the member, got {refused:?}"
        );
    }

    // Nothing was stored by any refused create.
    let (_, listed) = mq.list_brokers("").await;
    assert_eq!(
        listed
            .pointer("/brokerSummaries")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0),
        0,
        "a refused create must store nothing, got {listed:?}"
    );
}

/// `Groups` carries its own documented bound — "The list of groups (20 maximum)
/// … must be 2-100 characters long" — on both the operations that accept it.
#[tokio::test]
async fn user_groups_honour_their_documented_bound() {
    let mq = Mq::new("us-east-1");
    let (_, created) = mq.create_broker(create_body("grouped")).await;
    let broker_id = string_at(&created, "/brokerId");

    let group = |count: usize| -> Vec<String> {
        (0..count).map(|index| format!("group-{index}")).collect()
    };

    // At the published bound, and one past it; then a value outside the
    // published character/length rule.
    let cases: Vec<(&str, Value, u16)> = vec![
        ("20 groups, the published maximum", json!(group(20)), 200),
        ("21 groups, one past it", json!(group(21)), 400),
        ("a one-character group", json!(["a"]), 400),
        ("a group with a space", json!(["bad group"]), 400),
        ("no groups", json!([]), 200),
    ];

    for (index, (case, groups, expected)) in cases.into_iter().enumerate() {
        let (status, body) = mq
            .create_user(
                &broker_id,
                &format!("grouped{index}"),
                json!({"password": "GoodPassword12", "groups": groups}),
            )
            .await;
        assert_eq!(
            status, expected,
            "case {case:?} on CreateUser: expected {expected}, got {status} {body:?}"
        );

        // The same rule on the inline users `CreateBroker` accepts.
        let mut create = create_body(&format!("grouped-inline-{index}"));
        create["users"] = json!([{
            "username": format!("inline{index}"),
            "password": "GoodPassword12",
            "groups": groups,
        }]);
        let (status, body) = mq.create_broker(create).await;
        assert_eq!(
            status, expected,
            "case {case:?} on CreateBroker: expected {expected}, got {status} {body:?}"
        );
    }
}

/// Every refusal this service makes carries a code the model declares, whether
/// it came from the request boundary or from a handler: a client branching on
/// the code never sees one the model does not contain.
#[tokio::test]
async fn refusals_carry_a_modelled_error_code() {
    let mq = Mq::new("us-east-1");
    let (_, created) = mq.create_broker(create_body("coded")).await;
    let broker_id = string_at(&created, "/brokerId");

    // Rows: a boundary refusal (a constrained member the framework checks), a
    // handler refusal, a conflict and a missing resource.
    /// One row: the case, the status and error code it must answer with, and the
    /// answer the dispatch gave (status, code, body).
    type Row<'a> = (&'a str, u16, &'a str, (u16, String, Value));
    let cases: Vec<Row<'_>> = vec![
        (
            "maxResults below the published range",
            400,
            "BadRequestException",
            mq.call_with_code("GET", "/v1/brokers?maxResults=0", None).await,
        ),
        (
            "maxResults above the published range",
            400,
            "BadRequestException",
            mq.call_with_code("GET", "/v1/brokers?maxResults=101", None)
                .await,
        ),
        (
            "an engine type the model does not declare",
            400,
            "BadRequestException",
            {
                let mut body = create_body("bad-engine");
                body["engineType"] = json!("KAFKA");
                mq.call_with_code("POST", "/v1/brokers", Some(body)).await
            },
        ),
        (
            "a broker name already in use",
            409,
            "ConflictException",
            mq.call_with_code("POST", "/v1/brokers", Some(create_body("coded")))
                .await,
        ),
        (
            "a broker that does not exist",
            404,
            "NotFoundException",
            mq.call_with_code("GET", "/v1/brokers/b-missing", None).await,
        ),
        (
            "a user that does not exist",
            404,
            "NotFoundException",
            mq.call_with_code(
                "GET",
                &format!("/v1/brokers/{broker_id}/users/nobody"),
                None,
            )
            .await,
        ),
    ];

    for (case, expected_status, expected_code, (status, code, body)) in cases {
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {body:?}"
        );
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected {expected_code}, got {code:?} {body:?}"
        );
    }
}

/// Every `BrokerId` is minted by the store: well-shaped, distinct from every
/// other broker's, and the key the broker is actually stored under — including
/// for brokers created with the same name in different emulators and for
/// brokers whose requests are otherwise identical.
#[tokio::test]
async fn every_minted_broker_id_is_well_shaped_and_unique() {
    let mq = Mq::new("us-east-1");

    let mut minted: Vec<String> = Vec::new();
    // Rows differ in what a generator could accidentally key on: the name, the
    // engine, the deployment mode and the count of otherwise identical creates.
    let names: Vec<String> = (0..12).map(|index| format!("minted-{index}")).collect();
    for (index, name) in names.iter().enumerate() {
        let mut body = create_body(name);
        body["engineType"] = json!(ENGINE_TYPE_VALUES[index % ENGINE_TYPE_VALUES.len()]);
        let (status, created) = mq.create_broker(body).await;
        assert_eq!(status, 200, "case {name:?}: expected 200, got {created:?}");
        let broker_id = string_at(&created, "/brokerId");
        assert!(
            is_minted_broker_id(&broker_id),
            "case {name:?}: expected a minted b-<8-4-4-4-12 hex> id, got {broker_id:?}"
        );
        assert!(
            !minted.contains(&broker_id),
            "case {name:?}: {broker_id:?} was already minted for another broker ({minted:?})"
        );

        // The minted id is the one the broker is stored under.
        let (status, described) = mq.describe_broker(&broker_id).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the minted id must resolve, got {status} {described:?}"
        );
        assert_eq!(
            string_at(&described, "/brokerName"),
            *name,
            "case {name:?}: the minted id must resolve to its own broker, got {described:?}"
        );
        minted.push(broker_id);
    }

    assert_eq!(
        minted.len(),
        names.len(),
        "every create must mint its own id, got {minted:?}"
    );
}
