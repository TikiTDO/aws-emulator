// SPDX-License-Identifier: CC0-1.0

//! What the two entity READ paths answer for the SAME entity id, end to end over HTTP:
//! `DescribeEntity` and `BatchDescribeEntities`.
//!
//! RULE: an entity id is judged by the syntax this service publishes for one — the
//! `EntityId` shape's `@pattern`, inside that shape's `@length`
//! (`smithy_query(query="shape", shape="EntityId")`) — and not by the `@pattern` of the
//! wider shape a particular member happens to target, so both read paths reach the same
//! verdict about the same id: an id outside that syntax is the modelled
//! `ValidationException`, refused before any lookup; an id inside it reaches the lookup,
//! which answers the modelled `ResourceNotFoundException` for an entity the catalog does
//! not hold and the entity's own record for one it does. A batch read states that same
//! verdict per entry, under the id that was asked for, in the same error code and the
//! same sentence — the brackets this service wraps a top-level `ValidationException`
//! `Message` in being the only difference.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! The rows differ along what the rule generalises over: the characters an id carries
//! (letters, digits, hyphens, dots, slashes, an underscore), where it carries them
//! (first, middle, last), its length (two characters, three, the published bound, one
//! past it and far past it), and whether the catalog holds it.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::state::{
    CHANGE_SET_APPLYING, CHANGE_SET_PREPARING,
};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The catalog every operation's `Catalog` member documents as its fixed value.
const CATALOG: &str = "AWSMarketplace";

/// A SigV4 credential scope naming this service's signing name (`aws-marketplace`), which
/// is what routes a request here.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The syntax this service publishes for an entity id: the `EntityId` shape's `@pattern`
/// (`smithy_query(query="shape", shape="EntityId")`), quoted verbatim in a refusal.
/// Written out here rather than read from the crate, so an implementation quoting its own
/// syntax cannot make this suite pass.
const ENTITY_ID_PATTERN: &str = "^[a-zA-Z0-9][.a-zA-Z0-9/-]+[a-zA-Z0-9]$";

/// How long an entity id may be: the `@length(min: 1, max: 255)` the `EntityId` shape
/// carries — and the `Errors` / `EntityDetails` map keys with it, both of which target
/// that shape (`smithy_query(query="shape", shape="EntityId,Errors,EntityDetails")`).
const ENTITY_ID_MAX: usize = 255;

/// How long after the start a change set has certainly reached `SUCCEEDED`: the two spans
/// the implementation publishes, plus a margin for a slow machine.
fn wait_for_success() {
    std::thread::sleep(CHANGE_SET_PREPARING + CHANGE_SET_APPLYING + Duration::from_millis(400));
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        let raw = self.body["__type"].as_str().unwrap_or_default();
        raw.rsplit('#').next().unwrap_or(raw)
    }

    /// The string at `pointer`, or the empty string.
    fn text(&self, pointer: &str) -> String {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The fault this answer carries: every error of this service spells it `Message`.
    fn message(&self) -> String {
        self.text("/Message")
    }

    /// Whether the JSON object at `pointer` holds `key`.
    fn holds(&self, pointer: &str, key: &str) -> bool {
        self.body
            .pointer(pointer)
            .and_then(Value::as_object)
            .is_some_and(|map| map.contains_key(key))
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "catalog.marketplace.eu-west-1.amazonaws.com")
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

/// `entity_id` as one token of a JSON pointer: `~` and `/` carry meaning in a pointer,
/// so an id holding either is escaped (RFC 6901) before it addresses its own entry.
fn pointer_token(entity_id: &str) -> String {
    entity_id.replace('~', "~0").replace('/', "~1")
}

/// The single entity read.
async fn describe_entity(router: &MultiServiceRouter, entity_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/DescribeEntity?catalog={CATALOG}&entityId={entity_id}"),
        None,
    )
    .await
}

/// The batch read of `entity_ids`, every entry naming the one catalog this service has.
async fn batch_describe_entities(router: &MultiServiceRouter, entity_ids: &[&str]) -> Answer {
    let entries: Vec<Value> = entity_ids
        .iter()
        .map(|entity_id| json!({"Catalog": CATALOG, "EntityId": entity_id}))
        .collect();
    call(
        router,
        "POST",
        "/BatchDescribeEntities",
        Some(json!({"EntityRequestList": entries})),
    )
    .await
}

/// Create one entity through a change set and answer the id the catalog ends up holding.
async fn create_entity(router: &MultiServiceRouter, title: &str) -> String {
    let started = call(
        router,
        "POST",
        "/StartChangeSet",
        Some(json!({
            "Catalog": CATALOG,
            "ChangeSet": [{
                "ChangeType": "CreateProduct",
                "Entity": {"Type": "AmiProduct@1.0"},
                "Details": json!({"ProductTitle": title}).to_string(),
            }],
        })),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "StartChangeSet must answer 200, got {} with {:?}",
        started.status, started.body
    );
    wait_for_success();

    let listed = call(
        router,
        "POST",
        "/ListEntities",
        Some(json!({"Catalog": CATALOG, "EntityType": "AmiProduct"})),
    )
    .await;
    listed
        .body
        .pointer("/EntitySummaryList")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item["Name"] == json!(title))
                .and_then(|item| item["EntityId"].as_str().map(str::to_string))
        })
        .unwrap_or_else(|| {
            panic!(
                "the applied change set must leave a readable entity, got {:?}",
                listed.body
            )
        })
}

/// What the rule requires of an id: the catalog holds it, it is well formed and the
/// catalog does not hold it, or it is no entity id at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Held,
    Missing,
    Malformed,
}

/// RULE: both read paths judge an entity id by the syntax the service publishes, so for
/// the same id they reach the same verdict — and a batch read states it per entry, in the
/// error code and the sentence a single read of that id answers with.
#[tokio::test]
async fn both_read_paths_judge_an_entity_id_by_the_published_syntax() {
    let router = emulator();
    let held = create_entity(&router, "entity-read-agreement").await;

    let at_bound = "a".repeat(ENTITY_ID_MAX);
    let past_bound = "a".repeat(ENTITY_ID_MAX + 1);
    let far_past_bound = "a".repeat(1024);

    // (case, entity id, the verdict the published syntax and the store require)
    let rows: Vec<(String, &str, Verdict)> = vec![
        ("an entity the catalog holds".to_string(), &held, Verdict::Held),
        (
            "the shortest well-formed id".to_string(),
            "a-b",
            Verdict::Missing,
        ),
        (
            "a well-formed product id the catalog does not hold".to_string(),
            "prod-abcdefghijklmnop01",
            Verdict::Missing,
        ),
        (
            // `.` and `/` are inside the published syntax and outside `ResourceId`'s
            // `^[\w\-]+$`, the pattern the single read's member targets.
            "dots and slashes in the middle".to_string(),
            "a0.b-c/d1",
            Verdict::Missing,
        ),
        (
            format!("at the published length bound ({ENTITY_ID_MAX})"),
            &at_bound,
            Verdict::Missing,
        ),
        (
            "one character: shorter than the syntax's three".to_string(),
            "a",
            Verdict::Malformed,
        ),
        (
            "two characters: the middle class needs at least one".to_string(),
            "ab",
            Verdict::Malformed,
        ),
        (
            // The other direction: a character `ResourceId` admits and the published
            // syntax does not.
            "an underscore".to_string(),
            "prod_abcdef",
            Verdict::Malformed,
        ),
        (
            "a leading hyphen".to_string(),
            "-prod-abcdef",
            Verdict::Malformed,
        ),
        (
            "a trailing dot".to_string(),
            "prod-abcdef.",
            Verdict::Malformed,
        ),
        (
            format!("one past the published length bound ({})", ENTITY_ID_MAX + 1),
            &past_bound,
            Verdict::Malformed,
        ),
        (
            "far past the published length bound (1024)".to_string(),
            &far_past_bound,
            Verdict::Malformed,
        ),
    ];

    for (case, entity_id, verdict) in &rows {
        let single = describe_entity(&router, entity_id).await;
        let batch = batch_describe_entities(&router, &[entity_id]).await;
        assert_eq!(
            batch.status, 200,
            "case {case:?}: a batch read answers 200 whatever its entries resolve to, got \
             {} with {:?}",
            batch.status, batch.body
        );

        match verdict {
            Verdict::Held => {
                assert_eq!(
                    (single.status, single.error_code()),
                    (200, ""),
                    "case {case:?}: an entity the catalog holds must be served, got {} {:?} \
                     with {:?}",
                    single.status,
                    single.error_code(),
                    single.body
                );
                assert!(
                    batch.holds("/EntityDetails", entity_id)
                        && !batch.holds("/Errors", entity_id),
                    "case {case:?}: the batch read must serve the same entity under its own \
                     id and report no error for it, got {:?}",
                    batch.body
                );
                // The record is the SAME record, member for member, however it is read.
                for member in ["EntityType", "EntityArn", "EntityIdentifier", "LastModifiedDate"]
                {
                    let from_single = single.text(&format!("/{member}"));
                    let from_batch = batch.text(&format!(
                        "/EntityDetails/{}/{member}",
                        pointer_token(entity_id)
                    ));
                    assert_eq!(
                        from_batch, from_single,
                        "case {case:?}: the batch read must report the {member} the single \
                         read reports, got {from_batch:?} beside {from_single:?}"
                    );
                }
            }
            Verdict::Missing => {
                assert_eq!(
                    (single.status, single.error_code()),
                    (404, "ResourceNotFoundException"),
                    "case {case:?}: an id inside the published syntax reaches the lookup, \
                     which answers the modelled 404, got {} {:?} with {:?}",
                    single.status,
                    single.error_code(),
                    single.body
                );
                assert_eq!(
                    single.message(),
                    format!("Requested entity '{entity_id}' does not exist."),
                    "case {case:?}: the 404 names the entity asked for, got {:?}",
                    single.message()
                );
            }
            Verdict::Malformed => {
                assert_eq!(
                    (single.status, single.error_code()),
                    (422, "ValidationException"),
                    "case {case:?}: an id outside the published syntax is the modelled 422, \
                     refused before any lookup, got {} {:?} with {:?}",
                    single.status,
                    single.error_code(),
                    single.body
                );
                assert_eq!(
                    single.message(),
                    format!(
                        "[Requested entity id '{entity_id}' is invalid. It should match with \
                         {ENTITY_ID_PATTERN}.]"
                    ),
                    "case {case:?}: the refusal names the id and the syntax it had to \
                     satisfy, got {:?}",
                    single.message()
                );
            }
        }

        if *verdict == Verdict::Held {
            continue;
        }

        // The agreement itself: what the batch read reports for the entry is what the
        // single read of the same id answered — its error code, and its sentence, which a
        // top-level `Message` carries bracketed and an entry carries on its own.
        let expected_code = single.error_code().to_string();
        let expected_message = single
            .message()
            .strip_prefix('[')
            .and_then(|message| message.strip_suffix(']'))
            .unwrap_or(&single.message())
            .to_string();
        let entry = format!("/Errors/{}", pointer_token(entity_id));
        let reported_code = batch.text(&format!("{entry}/ErrorCode"));
        assert_eq!(
            reported_code, expected_code,
            "case {case:?}: the entry must carry the code a single read of it answers, got \
             {reported_code:?} in {:?}",
            batch.body
        );
        let reported_message = batch.text(&format!("{entry}/ErrorMessage"));
        assert_eq!(
            reported_message, expected_message,
            "case {case:?}: the entry must carry the sentence a single read of it answers, \
             got {reported_message:?}"
        );
        assert!(
            !batch.holds("/EntityDetails", entity_id),
            "case {case:?}: an entry that resolved to no entity must not be served, got \
             {:?}",
            batch.body
        );
    }

    // One call carrying every id of the table at once — the published maximum for
    // `EntityRequestList` is 20 (`@length(min: 1, max: 20)`), so the whole class fits in
    // one batch. Each entry lands under its own id, whatever the entries beside it are.
    let requested: Vec<&str> = rows.iter().map(|(_, entity_id, _)| *entity_id).collect();
    assert!(
        requested.len() <= 20,
        "the table must fit one batch, got {} entries",
        requested.len()
    );
    let batch = batch_describe_entities(&router, &requested).await;
    assert_eq!(
        batch.status, 200,
        "a mixed batch answers 200, got {} with {:?}",
        batch.status, batch.body
    );
    for (case, entity_id, verdict) in &rows {
        let served = batch.holds("/EntityDetails", entity_id);
        let failed = batch.holds("/Errors", entity_id);
        assert_eq!(
            (served, failed),
            (*verdict == Verdict::Held, *verdict != Verdict::Held),
            "case {case:?}: an entry is reported in exactly the map its verdict names, got \
             served={served} failed={failed} in {:?}",
            batch.body
        );
        let expected_code = match verdict {
            Verdict::Held => continue,
            Verdict::Missing => "ResourceNotFoundException",
            Verdict::Malformed => "ValidationException",
        };
        let reported_code = batch.text(&format!(
            "/Errors/{}/ErrorCode",
            pointer_token(entity_id)
        ));
        assert_eq!(
            reported_code, expected_code,
            "case {case:?}: the entry must carry the code a single read of it answers, got \
             {reported_code:?} in {:?}",
            batch.body
        );
    }
}
