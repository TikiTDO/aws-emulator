// SPDX-License-Identifier: CC0-1.0

//! The DOMAIN's own REST data plane, driven over the real wire path: a request addressed to
//! the endpoint host `DescribeDomain` published, through the multi-service router, into the
//! mounted service. What is asserted is what a client sees.
//!
//! Three rules, each over a SET of inputs rather than one recorded call:
//!
//! * the endpoint a domain publishes SERVES the OpenSearch REST surface, and every status
//!   and value it answers is computed from the domain record and the indexes it holds —
//!   over a table of routes, under both addressings a client on this machine can use, with
//!   a path the surface does not serve answered the way the engine answers one it has no
//!   handler for;
//! * an index TEMPLATE gives every index created under a name its patterns match the
//!   mappings it carries, so a write that auto-creates such an index is judged by them —
//!   over a table of patterns and documents;
//! * fine-grained access control is the authorization BOUNDARY on a domain that has it
//!   switched on: a principal that is neither the master user nor bound by a role mapping
//!   is refused `403 security_exception` on every route, and binding its role by a role
//!   mapping makes the same requests serve.
//!
//! The session, the role and its policies live in the STS and IAM stores of the SAME shared
//! engine — the stores those services' own handlers write — so the principal a decision is
//! taken for is the one `sts:GetCallerIdentity` answers for the key the request signed
//! with. The control-plane harness comes from [`super::domain_lifecycle`]; this suite adds
//! no second copy of it.

use std::collections::BTreeMap;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::ingest::{
    self, DocumentWrite,
};
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;

use super::data_source_registry::created_domain;
use super::domain_lifecycle::{Answer, Emulator, DEFAULT_ACCESS_KEY_ID, REGION};

/// The role the unmapped session below is assumed from — the position a Firehose delivery
/// role is in when nothing has bound it to a fine-grained-access-control role.
const DELIVERY_ROLE: &str = "firehose-delivery";

/// The session name that session's credential carries.
const SESSION: &str = "delivery-session";

/// The access key id of that session's credentials.
pub(crate) const SESSION_KEY_ID: &str = "ASIAEXAMPLEDELIVERY1";

/// How long the seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: i32 = 3_600;

/// The ARN of the IAM role the seeded session speaks for — the identity a role mapping
/// names as a `backend_role`.
pub(crate) fn delivery_role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/{DELIVERY_ROLE}",
        ::engine::emulator_account_id()
    )
}

/// The principal the default access key speaks for: the IAM user this emulator's own
/// long-term key belongs to, which is what a domain naming it as its master user grants
/// everything to.
fn master_user_arn() -> String {
    ::service_common::caller_principal::ambient_user_arn()
}

/// The ARN of the SESSION that credential speaks for — the principal
/// `sts:GetCallerIdentity` reports for it, which is the name the cluster knows the caller
/// by.
pub(crate) fn delivery_session_arn() -> String {
    format!(
        "arn:aws:sts::{}:assumed-role/{DELIVERY_ROLE}/{SESSION}",
        ::engine::emulator_account_id()
    )
}

/// Record the assumed-role session [`SESSION_KEY_ID`] speaks for, as `sts:AssumeRole`
/// records one.
pub(crate) fn given_session(emulator: &Emulator) {
    let now = SystemTime::now();
    let account = ::engine::emulator_account_id();
    let role_arn = delivery_role_arn();
    let vend = sts::NewSession {
        role_arn: Some(role_arn.clone()),
        ..sts::NewSession::new(
            sts::Identity {
                user_id: sts::role_id(&role_arn),
                account: account.to_string(),
                arn: delivery_session_arn(),
            },
            sts::SessionKind::AssumedRole,
            SESSION_SECONDS,
        )
    };
    let store = sts::service_state(&**emulator.engine()).expect("the STS store");
    store
        .lock()
        .insert(vend.into_session(SESSION_KEY_ID, now))
        .expect("the session is recorded");
}

/// Give the delivery role an inline policy allowing this service's data-plane actions on
/// every domain — the IAM grant a Firehose delivery role carries, so what is left to
/// decide is fine-grained access control alone.
pub(crate) fn given_data_plane_policy(emulator: &Emulator) {
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": "es:ESHttp*", "Resource": "*" }]
    })
    .to_string();
    let store = iam::service_state(&**emulator.engine()).expect("the IAM store");
    store.lock().role_inline_policies.insert(
        iam::name_key(DELIVERY_ROLE),
        BTreeMap::from([(
            iam::name_key("inline"),
            iam::InlinePolicy {
                policy_name: "inline".to_string(),
                policy_document: document,
            },
        )]),
    );
}

/// The endpoint host a domain published, read out of the store the create wrote it to.
pub(crate) fn endpoint_of(emulator: &Emulator, domain: &str) -> String {
    let store = emulator.store();
    let guard = store.lock();
    guard
        .domains
        .get(domain)
        .unwrap_or_else(|| panic!("expected the store to hold domain {domain}"))
        .endpoint
        .clone()
}

/// The ARN a domain published.
fn arn_of(emulator: &Emulator, domain: &str) -> String {
    let store = emulator.store();
    let guard = store.lock();
    guard
        .domains
        .get(domain)
        .unwrap_or_else(|| panic!("expected the store to hold domain {domain}"))
        .arn
        .clone()
}

/// One REST request to the HOST a domain published — the addressing
/// `DomainStatus$Endpoint` publishes.
pub(crate) async fn rest(
    emulator: &Emulator,
    access_key_id: &str,
    host: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    dispatch_rest(emulator, access_key_id, host, method, path, body).await
}

/// The same request under the PREFIXED path addressing, on the emulator's own endpoint —
/// the one a client that cannot resolve the published host uses.
pub(crate) async fn rest_prefixed(
    emulator: &Emulator,
    domain: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    dispatch_rest(
        emulator,
        DEFAULT_ACCESS_KEY_ID,
        "es.us-east-1.amazonaws.com",
        method,
        &format!("/_aws/es/{domain}{path}"),
        body,
    )
    .await
}

/// Build and answer one signed data-plane request.
pub(crate) async fn dispatch_rest(
    emulator: &Emulator,
    access_key_id: &str,
    host: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    rest_payload(emulator, access_key_id, host, method, path, payload).await
}

/// The same request carrying a body that is not one JSON document — the newline-delimited
/// stream the bulk API takes, say — as the bytes the client sent.
pub(crate) async fn rest_payload(
    emulator: &Emulator,
    access_key_id: &str,
    host: &str,
    method: &str,
    path: &str,
    payload: String,
) -> Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", host)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/es/aws4_request, \
                 SignedHeaders=host, Signature=0"
            ),
        )
        .header("content-type", "application/json")
        .body(Bytes::from(payload))
        .expect("the test request is well formed");
    emulator.dispatch(request).await
}

/// The `error.type` a refused data-plane answer carries.
pub(crate) fn error_type(answer: &Answer) -> String {
    answer.body["error"]["type"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// RULE: the endpoint a domain publishes serves the OpenSearch REST surface, and every
/// value it answers is computed from the domain record and the indexes it holds.
///
/// The rows span the surface: the cluster banner and the cluster health, an index's
/// create and read, a document write (first and second), a document read, a count, a
/// search that matches and one that does not, an index the domain does not hold, and a
/// path the surface serves no handler for.
#[tokio::test]
async fn the_published_endpoint_serves_the_rest_surface_out_of_the_domain_record() {
    let emulator = Emulator::new();
    created_domain(&emulator, "search-target").await;
    let host = endpoint_of(&emulator, "search-target");
    let account = ::engine::emulator_account_id();

    // The cluster the endpoint answers for IS this domain: its name and its engine
    // version come from the record the create wrote.
    let banner = rest(&emulator, DEFAULT_ACCESS_KEY_ID, &host, "GET", "/", None).await;
    assert_eq!(
        banner.status, 200,
        "case \"GET /\": expected 200, got {} with {:?}",
        banner.status, banner.body
    );
    assert_eq!(
        banner.body["cluster_name"].as_str(),
        Some(format!("{account}:search-target").as_str()),
        "case \"GET /\": expected the domain's cluster name, got {:?}",
        banner.body
    );
    assert_eq!(
        banner.body["version"]["number"].as_str(),
        Some(
            format!(
                "{}.0",
                super::domain_lifecycle::default_engine_release()
            )
            .as_str()
        ),
        "case \"GET /\": expected the version the domain's EngineVersion names, got {:?}",
        banner.body
    );

    // The health is the cluster's own: one data node, as the default cluster
    // configuration carries, and no index yet, so nothing is unassigned.
    let health = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/_cluster/health",
        None,
    )
    .await;
    assert_eq!(
        health.status, 200,
        "case \"GET /_cluster/health\": expected 200, got {} with {:?}",
        health.status, health.body
    );
    assert_eq!(
        health.body["number_of_data_nodes"].as_i64(),
        Some(1),
        "case \"GET /_cluster/health\": expected the domain's node count, got {:?}",
        health.body
    );
    assert_eq!(
        health.body["status"].as_str(),
        Some("green"),
        "case \"GET /_cluster/health\": expected green with no index held, got {:?}",
        health.body
    );

    // An index created over the endpoint is readable back through it, carrying the
    // mappings the create named.
    let created = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "PUT",
        "/catalog",
        Some(json!({
            "settings": { "number_of_shards": 2, "number_of_replicas": 1 },
            "mappings": { "properties": { "title": { "type": "keyword" } } }
        })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case \"PUT /catalog\": expected 200, got {} with {:?}",
        created.status, created.body
    );
    assert_eq!(
        created.body["acknowledged"].as_bool(),
        Some(true),
        "case \"PUT /catalog\": expected the create to be acknowledged, got {:?}",
        created.body
    );
    let read = rest(&emulator, DEFAULT_ACCESS_KEY_ID, &host, "GET", "/catalog", None).await;
    assert_eq!(
        read.body["catalog"]["mappings"]["properties"]["title"]["type"].as_str(),
        Some("keyword"),
        "case \"GET /catalog\": expected the mappings the create named, got {:?}",
        read.body
    );
    assert_eq!(
        read.body["catalog"]["settings"]["index"]["provided_name"].as_str(),
        Some("catalog"),
        "case \"GET /catalog\": expected the index's own name in its settings, got {:?}",
        read.body
    );

    // The health now accounts for that index's shards: two primaries, and two replicas
    // no second node can hold, which is what makes such a cluster yellow.
    let with_index = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/_cluster/health",
        None,
    )
    .await;
    assert_eq!(
        (
            with_index.body["active_primary_shards"].as_i64(),
            with_index.body["unassigned_shards"].as_i64(),
            with_index.body["status"].as_str(),
        ),
        (Some(2), Some(2), Some("yellow")),
        "case \"GET /_cluster/health with one index\": expected the index's shards, got {:?}",
        with_index.body
    );

    // The health of ONE index is the health of that index, and an index the domain does
    // not hold is the engine's own not-found rather than a whole-cluster answer.
    struct HealthRow {
        case: &'static str,
        path: &'static str,
        status: u16,
        /// The `status`/`timed_out` pair the answer must report, for a row that is served.
        reported: Option<(&'static str, bool)>,
    }
    let health_rows: &[HealthRow] = &[
        HealthRow {
            case: "the health of the one index",
            path: "/_cluster/health/catalog",
            status: 200,
            reported: Some(("yellow", false)),
        },
        HealthRow {
            case: "the health of an index the domain does not hold",
            path: "/_cluster/health/absent",
            status: 404,
            reported: None,
        },
        HealthRow {
            case: "a wait for a status the cluster has not reached",
            path: "/_cluster/health?wait_for_status=green",
            status: 200,
            reported: Some(("yellow", true)),
        },
        HealthRow {
            case: "a wait for the status it has",
            path: "/_cluster/health?wait_for_status=yellow",
            status: 200,
            reported: Some(("yellow", false)),
        },
        HealthRow {
            case: "a path under the health route the surface serves no handler for",
            path: "/_cluster/health/catalog/extra",
            status: 400,
            reported: None,
        },
    ];
    for row in health_rows {
        let answered = rest(&emulator, DEFAULT_ACCESS_KEY_ID, &host, "GET", row.path, None).await;
        assert_eq!(
            answered.status, row.status,
            "case {:?}: expected {}, got {} with {:?}",
            row.case, row.status, answered.status, answered.body
        );
        if let Some((status, timed_out)) = row.reported {
            assert_eq!(
                (
                    answered.body["status"].as_str(),
                    answered.body["timed_out"].as_bool()
                ),
                (Some(status), Some(timed_out)),
                "case {:?}: expected {status}/{timed_out}, got {:?}",
                row.case,
                answered.body
            );
        }
    }

    // A document written through the endpoint is created, then replaced by a second
    // write to the same id, and reads back as it was sent.
    let first = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "PUT",
        "/catalog/_doc/one",
        Some(json!({ "title": "first" })),
    )
    .await;
    assert_eq!(
        (first.status, first.body["result"].as_str(), first.body["_version"].as_i64()),
        (201, Some("created"), Some(1)),
        "case \"PUT /catalog/_doc/one\": expected a created document, got {:?}",
        first.body
    );
    let second = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "PUT",
        "/catalog/_doc/one",
        Some(json!({ "title": "second" })),
    )
    .await;
    assert_eq!(
        (
            second.status,
            second.body["result"].as_str(),
            second.body["_version"].as_i64()
        ),
        (200, Some("updated"), Some(2)),
        "case \"a second write to one id\": expected an updated document, got {:?}",
        second.body
    );
    let document = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/catalog/_doc/one",
        None,
    )
    .await;
    assert_eq!(
        document.body["_source"],
        json!({ "title": "second" }),
        "case \"GET /catalog/_doc/one\": expected the document as sent, got {:?}",
        document.body
    );

    // A count and a search report the documents the index holds, and the query decides
    // which: the matching term selects the document and another selects none.
    let count = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/catalog/_count",
        None,
    )
    .await;
    assert_eq!(
        count.body["count"].as_i64(),
        Some(1),
        "case \"GET /catalog/_count\": expected one document, got {:?}",
        count.body
    );
    // (the case, the query, the hits it selects)
    let searches: &[(&str, Value, i64)] = &[
        ("no query at all", json!({}), 1),
        ("a matching term", json!({ "query": { "term": { "title": "second" } } }), 1),
        ("a term matching nothing", json!({ "query": { "term": { "title": "first" } } }), 0),
        ("a matching ids clause", json!({ "query": { "ids": { "values": ["one"] } } }), 1),
    ];
    for (case, query, expected) in searches {
        let answered = rest(
            &emulator,
            DEFAULT_ACCESS_KEY_ID,
            &host,
            "POST",
            "/catalog/_search",
            Some(query.clone()),
        )
        .await;
        assert_eq!(
            answered.body["hits"]["total"]["value"].as_i64(),
            Some(*expected),
            "case {case:?}: expected {expected} hits, got {:?}",
            answered.body
        );
    }

    // And the reads above changed nothing a later caller sees.
    let after = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/catalog/_count",
        None,
    )
    .await;
    assert_eq!(
        after.body["count"].as_i64(),
        Some(1),
        "case \"a count after the reads\": expected the reads to have changed nothing, got {:?}",
        after.body
    );

    // An index the domain does not hold is the engine's own not-found, and a path the
    // surface serves no handler for says exactly that rather than answering 2xx.
    let missing = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/absent/_count",
        None,
    )
    .await;
    assert_eq!(
        (missing.status, error_type(&missing)),
        (404, "index_not_found_exception".to_string()),
        "case \"a count on an index the domain does not hold\": got {:?}",
        missing.body
    );
    let unhandled = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/_nodes/hot_threads",
        None,
    )
    .await;
    assert_eq!(
        unhandled.status, 400,
        "case \"a path the surface serves no handler for\": expected 400, got {} with {:?}",
        unhandled.status, unhandled.body
    );
    assert!(
        unhandled.body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("no handler found for uri"),
        "case \"a path the surface serves no handler for\": expected the engine's wording, \
         got {:?}",
        unhandled.body
    );

    // The PREFIXED addressing reaches the same domain, so a client that cannot resolve
    // the published host is answered identically.
    let prefixed = rest_prefixed(&emulator, "search-target", "GET", "/catalog/_count", None).await;
    assert_eq!(
        (prefixed.status, prefixed.body["count"].as_i64()),
        (200, Some(1)),
        "case \"the prefixed addressing\": expected the same answer as the published host, \
         got {:?}",
        prefixed.body
    );
    // And the domain's NAME is not the only thing it is addressed by: its published host
    // names it under that addressing too.
    let by_host = rest_prefixed(&emulator, &host, "GET", "/catalog/_count", None).await;
    assert_eq!(
        (by_host.status, by_host.body["count"].as_i64()),
        (200, Some(1)),
        "case \"the prefixed addressing naming the host\": got {:?}",
        by_host.body
    );
}

/// RULE: every bound the data plane declares is enforced on the wire, and a request that
/// breaks one is refused rather than served — and a write that may not replace is refused
/// when the document is already there.
///
/// The rows span the bounds and refusals the surface states: a body past the byte bound, a
/// body that is not the JSON the handler expects, a page past the result window, more
/// index patterns than one request may name, a known route reached with a method it does
/// not take, and the create-only write.
#[tokio::test]
async fn every_bound_the_data_plane_declares_refuses_rather_than_serving() {
    let emulator = Emulator::new();
    created_domain(&emulator, "bounded").await;
    let host = endpoint_of(&emulator, "bounded");

    let created = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "PUT",
        "/catalog/_doc/one",
        Some(json!({ "title": "first" })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case \"the fixture document\": expected it to be created, got {} with {:?}",
        created.status, created.body
    );

    struct Row {
        case: &'static str,
        method: &'static str,
        path: &'static str,
        body: Option<Value>,
        status: u16,
        /// The `error.type` the refusal must carry, where the refusal has one.
        error_type: Option<&'static str>,
    }
    // A body past the 1 MiB the surface takes, built as a query value so the whole request
    // is still well-formed JSON.
    let oversized = json!({ "query": { "term": { "title": "x".repeat(1024 * 1024 + 16) } } });
    let rows: &[Row] = &[
        Row {
            case: "a body past the byte bound",
            method: "POST",
            path: "/catalog/_search",
            body: Some(oversized),
            status: 413,
            error_type: Some("illegal_argument_exception"),
        },
        Row {
            case: "a page past the result window",
            method: "POST",
            path: "/catalog/_search",
            body: Some(json!({ "from": 9_999, "size": 100 })),
            status: 400,
            error_type: Some("illegal_argument_exception"),
        },
        Row {
            case: "a create body that is not a JSON object",
            method: "PUT",
            path: "/other",
            body: Some(json!([1, 2])),
            status: 400,
            error_type: Some("parse_exception"),
        },
        Row {
            case: "a known route reached with a method it does not take",
            method: "POST",
            path: "/_cluster/health",
            body: None,
            status: 405,
            error_type: None,
        },
        Row {
            case: "a create of a document the index already holds",
            method: "PUT",
            path: "/catalog/_create/one",
            body: Some(json!({ "title": "again" })),
            status: 409,
            error_type: Some("version_conflict_engine_exception"),
        },
        Row {
            case: "a _doc write asking for a create of a document already there",
            method: "PUT",
            path: "/catalog/_doc/one?op_type=create",
            body: Some(json!({ "title": "again" })),
            status: 409,
            error_type: Some("version_conflict_engine_exception"),
        },
        Row {
            case: "a create of a document the index does not hold",
            method: "PUT",
            path: "/catalog/_create/two",
            body: Some(json!({ "title": "second" })),
            status: 201,
            error_type: None,
        },
    ];
    for row in rows {
        let answered = rest(
            &emulator,
            DEFAULT_ACCESS_KEY_ID,
            &host,
            row.method,
            row.path,
            row.body.clone(),
        )
        .await;
        assert_eq!(
            answered.status, row.status,
            "case {:?}: expected {}, got {} with {:?}",
            row.case, row.status, answered.status, answered.body
        );
        if let Some(expected) = row.error_type {
            assert_eq!(
                error_type(&answered),
                expected,
                "case {:?}: expected the engine's {expected}, got {:?}",
                row.case,
                answered.body
            );
        }
    }

    // The refused create left the document exactly as it was.
    let unchanged = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/catalog/_doc/one",
        None,
    )
    .await;
    assert_eq!(
        (
            unchanged.body["_source"].clone(),
            unchanged.body["_version"].as_i64()
        ),
        (json!({ "title": "first" }), Some(1)),
        "case \"the document a refused create addressed\": expected it unchanged, got {:?}",
        unchanged.body
    );

    // An index named twice is scanned ONCE, so a repeated pattern cannot multiply the
    // count — the two documents the index holds are two however often it is named.
    let repeated = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/catalog,catalog,catalog/_count",
        None,
    )
    .await;
    assert_eq!(
        (repeated.status, repeated.body["count"].as_i64()),
        (200, Some(2)),
        "case \"an index named three times\": expected it counted once, got {:?}",
        repeated.body
    );

    // And a request naming more index patterns than one may is refused rather than
    // resolving them.
    let crowded = (0..101)
        .map(|ordinal| format!("catalog-{ordinal}"))
        .collect::<Vec<String>>()
        .join(",");
    let over = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        &format!("/{crowded}/_count"),
        None,
    )
    .await;
    assert_eq!(
        (over.status, error_type(&over)),
        (400, "illegal_argument_exception".to_string()),
        "case \"more index patterns than one request may name\": got {:?}",
        over.body
    );
}

/// RULE: an index template gives every index CREATED under a name its patterns match the
/// mappings it carries, so a write that auto-creates such an index is judged by those
/// mappings — and a name no pattern matches is created with none.
///
/// The rows span what the rule generalises over: an index name a pattern matches and one
/// it does not, a document the mappings accept and one they refuse, and the two ways an
/// index comes into being (an explicit create and a write that auto-creates it).
#[tokio::test]
async fn an_index_template_gives_every_matching_index_it_creates_its_mappings() {
    let emulator = Emulator::new();
    created_domain(&emulator, "templated").await;
    let host = endpoint_of(&emulator, "templated");
    let arn = arn_of(&emulator, "templated");

    let written = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "PUT",
        "/_index_template/strict-events",
        Some(json!({
            "index_patterns": ["events-*"],
            "priority": 10,
            "template": {
                "mappings": {
                    "dynamic": "strict",
                    "properties": { "count": { "type": "integer" } }
                }
            }
        })),
    )
    .await;
    assert_eq!(
        (written.status, written.body["acknowledged"].as_bool()),
        (200, Some(true)),
        "case \"PUT /_index_template/strict-events\": got {:?}",
        written.body
    );
    let read_back = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/_index_template/strict-events",
        None,
    )
    .await;
    assert_eq!(
        read_back.body["index_templates"][0]["index_template"]["index_patterns"][0].as_str(),
        Some("events-*"),
        "case \"GET /_index_template/strict-events\": expected the template as written, got {:?}",
        read_back.body
    );

    // (the case, the index name, the document, whether the cluster takes it)
    let rows: &[(&str, &str, Value, bool)] = &[
        (
            "a matching index, a document the mappings map",
            "events-2026-01-01",
            json!({ "count": 1 }),
            true,
        ),
        (
            "a matching index, a document carrying a field they do not",
            "events-2026-01-02",
            json!({ "count": 1, "extra": "x" }),
            false,
        ),
        (
            "a matching index, a value the mapped type cannot parse",
            "events-2026-01-03",
            json!({ "count": "not-a-number" }),
            false,
        ),
        (
            "an index no pattern matches",
            "audits-2026-01-01",
            json!({ "count": 1, "extra": "x" }),
            true,
        ),
    ];
    for (case, index, document, taken) in rows {
        // The write goes in as a PRODUCER's does — the one data-plane write path — so the
        // rule is shown where a delivery stream's records land, not only over the wire.
        let answered = ingest::index_document(
            &**emulator.engine(),
            &DocumentWrite {
                domain: &arn,
                index_name: index,
                document_id: Some("only"),
                document: document.to_string().as_bytes(),
                principal_arn: &delivery_role_arn(),
                now: SystemTime::now(),
            },
        );
        assert_eq!(
            answered.is_ok(),
            *taken,
            "case {case:?}: expected taken={taken}, got {answered:?}"
        );
    }

    // The auto-created index carries the template's mappings, which is WHY the documents
    // above were judged by them, and the document that fit is the one it holds.
    let described = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/events-2026-01-01",
        None,
    )
    .await;
    assert_eq!(
        described.body["events-2026-01-01"]["mappings"]["dynamic"].as_str(),
        Some("strict"),
        "case \"the auto-created index\": expected the template's mappings, got {:?}",
        described.body
    );
    let counted = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/events-*/_count",
        None,
    )
    .await;
    assert_eq!(
        counted.body["count"].as_i64(),
        Some(1),
        "case \"the documents the matching indexes hold\": expected only the one that fit, \
         got {:?}",
        counted.body
    );

    // An explicit create under a matching name is given the template's mappings too, and
    // what the create itself names wins over them.
    let explicit = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "PUT",
        "/events-explicit",
        Some(json!({ "mappings": { "properties": { "label": { "type": "keyword" } } } })),
    )
    .await;
    assert_eq!(
        explicit.status, 200,
        "case \"an explicit create under a matching name\": got {:?}",
        explicit.body
    );
    let explicit_read = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/events-explicit",
        None,
    )
    .await;
    assert_eq!(
        (
            explicit_read.body["events-explicit"]["mappings"]["dynamic"].as_str(),
            explicit_read.body["events-explicit"]["mappings"]["properties"]["label"]["type"]
                .as_str(),
            explicit_read.body["events-explicit"]["mappings"]["properties"]["count"]["type"]
                .as_str(),
        ),
        (Some("strict"), Some("keyword"), Some("integer")),
        "case \"an explicit create under a matching name\": expected the template's mappings \
         with the create's own merged onto them, got {:?}",
        explicit_read.body
    );

    // A template that applies to no index name at all is refused rather than stored.
    let patternless = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "PUT",
        "/_index_template/no-patterns",
        Some(json!({ "template": { "mappings": {} } })),
    )
    .await;
    assert_eq!(
        (patternless.status, error_type(&patternless)),
        (400, "illegal_argument_exception".to_string()),
        "case \"a template naming no index pattern\": got {:?}",
        patternless.body
    );
}

/// RULE: on a domain with fine-grained access control switched on, a principal that is
/// neither the master user nor bound by a role mapping may do NOTHING — every route is
/// refused `403 security_exception`, whatever IAM allows — and binding its role by a role
/// mapping makes the same requests serve.
///
/// The rows span the surface the boundary covers: the cluster banner, the cluster health,
/// an index create, a document write, a count and a search.
#[tokio::test]
async fn fine_grained_access_control_decides_every_request_to_the_domain() {
    let emulator = Emulator::new();
    let created = emulator
        .create(json!({
            "DomainName": "guarded",
            "EncryptionAtRestOptions": { "Enabled": true },
            "NodeToNodeEncryptionOptions": { "Enabled": true },
            "DomainEndpointOptions": { "EnforceHTTPS": true },
            "AdvancedSecurityOptions": {
                "Enabled": true,
                "InternalUserDatabaseEnabled": false,
                "MasterUserOptions": { "MasterUserARN": master_user_arn() }
            }
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "case \"CreateDomain with fine-grained access control\": got {} with {:?}",
        created.status, created.body
    );
    let host = endpoint_of(&emulator, "guarded");
    let arn = arn_of(&emulator, "guarded");
    given_session(&emulator);
    given_data_plane_policy(&emulator);

    // The MASTER user is granted everything, so the index the rows below address exists.
    let seeded = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "PUT",
        "/catalog",
        Some(json!({ "mappings": { "properties": { "title": { "type": "keyword" } } } })),
    )
    .await;
    assert_eq!(
        seeded.status, 200,
        "case \"the master user creating an index\": got {} with {:?}",
        seeded.status, seeded.body
    );

    // (the case, the method, the path, the body)
    let routes: &[(&str, &str, &str, Option<Value>)] = &[
        ("the cluster banner", "GET", "/", None),
        ("the cluster health", "GET", "/_cluster/health", None),
        ("an index create", "PUT", "/other", None),
        (
            "a document write",
            "PUT",
            "/catalog/_doc/one",
            Some(json!({ "title": "a" })),
        ),
        ("a count", "GET", "/catalog/_count", None),
        ("a search", "POST", "/catalog/_search", Some(json!({}))),
    ];

    // An UNMAPPED principal: IAM allows every one of these actions, and the cluster
    // refuses each of them.
    for (case, method, path, body) in routes {
        let answered = rest(&emulator, SESSION_KEY_ID, &host, method, path, body.clone()).await;
        assert_eq!(
            (answered.status, error_type(&answered)),
            (403, "security_exception".to_string()),
            "case {case:?} unmapped: expected the cluster's refusal, got {} with {:?}",
            answered.status,
            answered.body
        );
        assert!(
            answered.body["error"]["reason"]
                .as_str()
                .unwrap_or_default()
                .contains("no permissions for ["),
            "case {case:?} unmapped: expected the security plugin's wording, got {:?}",
            answered.body
        );
    }

    // And a document a producer delivers as that same role lands nowhere either: the
    // write is REFUSED, which is what sends a delivery stream's record to its backup.
    let refused = ingest::index_document(
        &**emulator.engine(),
        &DocumentWrite {
            domain: &arn,
            index_name: "catalog",
            document_id: Some("delivered"),
            document: br#"{"title":"delivered"}"#,
            principal_arn: &delivery_role_arn(),
            now: SystemTime::now(),
        },
    );
    assert!(
        matches!(
            &refused,
            Err(ingest::WriteRefusal::Rejected { error_type, .. })
                if *error_type == ingest::SECURITY_EXCEPTION
        ),
        "case \"a producer's delivery as the unmapped role\": expected a security refusal, \
         got {refused:?}"
    );

    // The security API is the master user's alone: the unmapped principal cannot grant
    // itself a mapping.
    let self_granted = rest(
        &emulator,
        SESSION_KEY_ID,
        &host,
        "PUT",
        "/_plugins/_security/api/rolesmapping/all_access",
        Some(json!({ "backend_roles": [delivery_role_arn()] })),
    )
    .await;
    assert_eq!(
        (self_granted.status, error_type(&self_granted)),
        (403, "security_exception".to_string()),
        "case \"the unmapped principal writing a role mapping\": got {} with {:?}",
        self_granted.status,
        self_granted.body
    );

    // The MASTER user binds the role, which is the one change that makes the same
    // requests serve.
    let mapped = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "PUT",
        "/_plugins/_security/api/rolesmapping/all_access",
        Some(json!({ "backend_roles": [delivery_role_arn()], "users": [], "hosts": [] })),
    )
    .await;
    assert_eq!(
        mapped.status, 201,
        "case \"the master user binding the role\": got {} with {:?}",
        mapped.status, mapped.body
    );
    for (case, method, path, body) in routes {
        let answered = rest(&emulator, SESSION_KEY_ID, &host, method, path, body.clone()).await;
        assert!(
            (200..300).contains(&answered.status),
            "case {case:?} mapped: expected the request to serve, got {} with {:?}",
            answered.status,
            answered.body
        );
    }
    // And the producer's delivery lands now too.
    let delivered = ingest::index_document(
        &**emulator.engine(),
        &DocumentWrite {
            domain: &arn,
            index_name: "catalog",
            document_id: Some("delivered"),
            document: br#"{"title":"delivered"}"#,
            principal_arn: &delivery_role_arn(),
            now: SystemTime::now(),
        },
    );
    assert!(
        delivered.is_ok(),
        "case \"a producer's delivery as the mapped role\": expected it to land, got \
         {delivered:?}"
    );

    // A domain WITHOUT fine-grained access control has no security plugin at all, so the
    // same path is a path it serves no handler for rather than a refusal.
    created_domain(&emulator, "unguarded").await;
    let open_host = endpoint_of(&emulator, "unguarded");
    let no_plugin = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &open_host,
        "GET",
        "/_plugins/_security/api/rolesmapping/all_access",
        None,
    )
    .await;
    assert_eq!(
        no_plugin.status, 400,
        "case \"the security API on a domain without fine-grained access control\": \
         expected the engine's no-handler answer, got {} with {:?}",
        no_plugin.status, no_plugin.body
    );
    // …and that domain serves the unmapped principal, because nothing switched the
    // boundary on for it.
    let open = rest(
        &emulator,
        SESSION_KEY_ID,
        &open_host,
        "GET",
        "/_cluster/health",
        None,
    )
    .await;
    assert_eq!(
        open.status, 200,
        "case \"a domain without the boundary\": expected it to serve, got {} with {:?}",
        open.status, open.body
    );
}

/// RULE: the security plugin answers `authinfo` for the AUTHENTICATED user whatever
/// permissions that user holds — it is the route a client checks its own identity with
/// before it asks for anything — and what it reports is read from the request's principal
/// and the domain's own security configuration: the user, the backend roles its
/// credential speaks for, and the roles the mappings bind it to.
///
/// The rows span who can ask: the master user, a principal no mapping binds — which is
/// refused every other route while this one answers it — and that same principal once a
/// mapping binds it.
#[tokio::test]
async fn the_security_plugin_tells_every_authenticated_caller_who_it_is() {
    let emulator = Emulator::new();
    let created = emulator
        .create(json!({
            "DomainName": "who-am-i",
            "EncryptionAtRestOptions": { "Enabled": true },
            "NodeToNodeEncryptionOptions": { "Enabled": true },
            "DomainEndpointOptions": { "EnforceHTTPS": true },
            "AdvancedSecurityOptions": {
                "Enabled": true,
                "InternalUserDatabaseEnabled": false,
                "MasterUserOptions": { "MasterUserARN": master_user_arn() }
            }
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "case \"CreateDomain with fine-grained access control\": got {} with {:?}",
        created.status, created.body
    );
    let host = endpoint_of(&emulator, "who-am-i");
    given_session(&emulator);
    given_data_plane_policy(&emulator);

    /// The route under test.
    const AUTHINFO: &str = "/_plugins/_security/authinfo";

    // The principal no mapping binds is refused every route the boundary covers — and
    // answered this one, which is what lets a client confirm its credential works at all
    // before it is told what it may do with it.
    let refused = rest(
        &emulator,
        SESSION_KEY_ID,
        &host,
        "POST",
        "/catalog/_search",
        Some(json!({})),
    )
    .await;
    assert_eq!(
        (refused.status, error_type(&refused)),
        (403, "security_exception".to_string()),
        "case \"an unmapped principal searching\": got {} with {:?}",
        refused.status,
        refused.body
    );

    // What the plugin tells each caller it is.
    struct Told {
        case: &'static str,
        key: &'static str,
        user: String,
        backend_roles: Vec<String>,
        roles: Vec<String>,
    }
    let rows: Vec<Told> = vec![
        Told {
            case: "the master user",
            key: DEFAULT_ACCESS_KEY_ID,
            user: master_user_arn(),
            // A long-term IAM credential speaks for the user itself, so it presents no
            // backend role beside its own name.
            backend_roles: Vec::new(),
            roles: vec!["all_access".to_string()],
        },
        Told {
            case: "a principal no mapping binds",
            key: SESSION_KEY_ID,
            user: delivery_session_arn(),
            // An assumed-role session presents the ROLE it is a session of, which is the
            // identity a role mapping names.
            backend_roles: vec![delivery_role_arn()],
            roles: Vec::new(),
        },
    ];
    for Told {
        case,
        key,
        user,
        backend_roles,
        roles,
    } in &rows
    {
        let answered = rest(&emulator, key, &host, "GET", AUTHINFO, None).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected the plugin to answer who the caller is, got {} with \
             {:?}",
            answered.status, answered.body
        );
        assert_eq!(
            answered.body["user_name"].as_str(),
            Some(user.as_str()),
            "case {case:?}: expected the user {user:?}, got {:?}",
            answered.body
        );
        assert_eq!(
            answered.body["user"].as_str(),
            Some(
                format!(
                    "User [name={user}, backend_roles=[{}], requestedTenant=null]",
                    backend_roles.join(", ")
                )
                .as_str()
            ),
            "case {case:?}: expected the user description the cluster states, got {:?}",
            answered.body
        );
        assert_eq!(
            answered.body["backend_roles"],
            json!(backend_roles),
            "case {case:?}: expected the backend roles {backend_roles:?}, got {:?}",
            answered.body
        );
        assert_eq!(
            answered.body["roles"],
            json!(roles),
            "case {case:?}: expected the roles {roles:?}, got {:?}",
            answered.body
        );
    }

    // Binding the role to a security role is what `roles` reports next: what the plugin
    // tells a caller it holds is the mappings as they stand.
    let mapped = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "PUT",
        "/_plugins/_security/api/rolesmapping/readall",
        Some(json!({ "backend_roles": [delivery_role_arn()] })),
    )
    .await;
    assert_eq!(
        mapped.status, 201,
        "case \"the master user binding the role\": got {} with {:?}",
        mapped.status, mapped.body
    );
    let bound = rest(&emulator, SESSION_KEY_ID, &host, "GET", AUTHINFO, None).await;
    assert_eq!(
        (bound.status, bound.body["roles"].clone()),
        (200, json!(["readall"])),
        "case \"a principal a mapping binds\": expected the role it was bound to, got {:?}",
        bound.body
    );
    assert_eq!(
        bound.body["backend_roles"],
        json!([delivery_role_arn()]),
        "case \"a principal a mapping binds\": expected its backend role unchanged, got \
         {:?}",
        bound.body
    );

    // The route takes a GET and nothing else, and on a domain the boundary is not
    // switched on for there is no security plugin to ask at all.
    let written = rest(&emulator, DEFAULT_ACCESS_KEY_ID, &host, "PUT", AUTHINFO, None).await;
    assert_eq!(
        written.status, 405,
        "case \"a method the route does not take\": expected the engine's refusal, got {} \
         with {:?}",
        written.status, written.body
    );
}
