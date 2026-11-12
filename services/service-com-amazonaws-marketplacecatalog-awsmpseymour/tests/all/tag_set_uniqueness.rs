// SPDX-License-Identifier: CC0-1.0

//! A resource's tags are a MAP from key to value, end to end over HTTP.
//!
//! RULE: a tag key carries exactly one value, so whichever member a tag list arrives on,
//! what the catalog STORES is the set those pairs describe — a key named more than once in
//! one list contributes one tag whose value is the LAST pair given for it, a key the
//! resource already carries takes the incoming value instead of gaining a second entry, and
//! the per-resource tag bound therefore counts KEYS.
//!
//! The members a tag list arrives on are every one in the crate that carries tag pairs:
//! `StartChangeSet$ChangeSetTags` (the change set's own tags), each change's
//! `StartChangeSet$ChangeSet$EntityTags` (the tags applying the change puts on its entity)
//! and `TagResource$Tags`. `UntagResource$TagKeys` carries keys only, and removes every
//! entry holding one, so it cannot leave a key behind.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the boundary validator, the handler, the persisted state and
//! serialization — the same path a client takes. The rows differ along what the rule
//! generalises over: which member the repeated key arrives on, how many times it repeats,
//! where the repeats sit in the list, how many other keys travel with them, which resource
//! kind (entity or change set) holds the result, and which entity type.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::state::{
    CHANGE_SET_APPLYING, CHANGE_SET_PREPARING, MAX_RESOURCE_TAGS,
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

/// The restJson1 document media type: `application/json`, for answers and refusals alike.
const JSON: &str = "application/json";

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

/// What the emulator answered: the status, the JSON body and the content type.
struct Answer {
    status: u16,
    body: Value,
    content_type: String,
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

    /// The tags this answer reports, as `(key, value)` pairs in the order the list carries
    /// them — so a case can see a key reported twice rather than a sort hiding it.
    fn tag_entries(&self) -> Vec<(String, String)> {
        self.body
            .pointer("/Tags")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|tag| {
                (
                    tag["Key"].as_str().unwrap_or_default().to_string(),
                    tag["Value"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect()
    }

    /// Those same pairs sorted by key, which is the SET the resource carries once the rule
    /// holds.
    fn tag_pairs(&self) -> Vec<(String, String)> {
        let mut pairs = self.tag_entries();
        pairs.sort();
        pairs
    }

    /// The keys this answer reports, deduplicated — the same length as
    /// [`Answer::tag_entries`] exactly when no key is reported twice.
    fn tag_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.tag_entries().into_iter().map(|pair| pair.0).collect();
        keys.sort();
        keys.dedup();
        keys
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
        builder = builder.header("content-type", JSON);
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
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
    Answer {
        status,
        body,
        content_type,
    }
}

async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> Answer {
    call(
        router,
        "POST",
        "/ListTagsForResource",
        Some(json!({"ResourceArn": resource_arn})),
    )
    .await
}

async fn tag_resource(router: &MultiServiceRouter, resource_arn: &str, tags: Value) -> Answer {
    call(
        router,
        "POST",
        "/TagResource",
        Some(json!({"ResourceArn": resource_arn, "Tags": tags})),
    )
    .await
}

/// One `StartChangeSet` body creating an entity of `entity_type`, with the two tag members
/// the cases vary: the change set's own tags and the change's entity tags.
fn start_body(
    entity_type: &str,
    change_set_tags: Option<Value>,
    entity_tags: Option<Value>,
) -> Value {
    let mut change = json!({
        "ChangeType": "CreateProduct",
        "Entity": {"Type": format!("{entity_type}@1.0")},
        "Details": json!({"ProductTitle": entity_type}).to_string(),
    });
    if let Some(tags) = entity_tags {
        change["EntityTags"] = tags;
    }
    let mut body = json!({"Catalog": CATALOG, "ChangeSet": [change]});
    if let Some(tags) = change_set_tags {
        body["ChangeSetTags"] = tags;
    }
    body
}

/// The two resources one succeeded `CreateProduct` change set makes: the entity's ARN, read
/// back through `DescribeEntity`, and the change set's own, read back through
/// `DescribeChangeSet` — so every case addresses a resource the store really holds, named
/// by the ARN the service itself minted for it.
async fn created_resources(
    router: &MultiServiceRouter,
    entity_type: &str,
    change_set_tags: Option<Value>,
    entity_tags: Option<Value>,
) -> (String, String) {
    let started = call(
        router,
        "POST",
        "/StartChangeSet",
        Some(start_body(entity_type, change_set_tags, entity_tags)),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "StartChangeSet must answer 200, got {} with {:?}",
        started.status, started.body
    );
    let change_set_id = started.text("/ChangeSetId");
    wait_for_success();

    let described = call(
        router,
        "GET",
        &format!("/DescribeChangeSet?catalog={CATALOG}&changeSetId={change_set_id}"),
        None,
    )
    .await;
    assert_eq!(
        described.text("/Status"),
        "SUCCEEDED",
        "the change set must have succeeded before its entity is addressed, got {:?}",
        described.body
    );
    let change_set_arn = described.text("/ChangeSetArn");

    let listed = call(router, "POST", "/ListChangeSets", Some(json!({"Catalog": CATALOG}))).await;
    let entity_id = listed
        .body
        .pointer("/ChangeSetSummaryList")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|item| item["ChangeSetId"] == json!(change_set_id))
        .and_then(|item| item["EntityIdList"][0].as_str().map(str::to_string))
        .unwrap_or_else(|| {
            panic!("change set {change_set_id} must report its entity: {:?}", listed.body)
        });

    let entity = call(
        router,
        "GET",
        &format!("/DescribeEntity?catalog={CATALOG}&entityId={entity_id}"),
        None,
    )
    .await;
    assert_eq!(
        entity.status, 200,
        "DescribeEntity must report the created entity, got {} with {:?}",
        entity.status, entity.body
    );
    (entity.text("/EntityArn"), change_set_arn)
}

/// One row: the case, the entity type the change set creates, whether the tags land on the
/// ENTITY (`true`, through `EntityTags`) or on the change set (`false`, through
/// `ChangeSetTags`), the list submitted there, the set it must leave, the list a later
/// `TagResource` names, and the set THAT must leave.
type DuplicateRow = (
    &'static str,
    &'static str,
    bool,
    Value,
    &'static [(&'static str, &'static str)],
    Value,
    &'static [(&'static str, &'static str)],
);

/// A tag list naming one key more than once names ONE tag, whose value is the last pair
/// given for it — on every member a list arrives on, for either resource kind — and a
/// `TagResource` naming that key afterwards is the only value the resource then reports.
#[tokio::test]
async fn a_repeated_tag_key_is_stored_once_with_its_last_value() {
    // (case, entity type, on entity, submitted at start, stored set, retag, set after retag)
    let rows: [DuplicateRow; 6] = [
        (
            "ChangeSetTags naming one key twice",
            "DataProduct",
            false,
            json!([{"Key": "env", "Value": "a"}, {"Key": "env", "Value": "b"}]),
            &[("env", "b")],
            json!([{"Key": "env", "Value": "c"}]),
            &[("env", "c")],
        ),
        (
            "ChangeSetTags naming one key three times, interleaved with another key",
            "SaaSProduct",
            false,
            json!([
                {"Key": "team", "Value": "one"},
                {"Key": "env", "Value": "a"},
                {"Key": "team", "Value": "two"},
                {"Key": "team", "Value": "three"},
            ]),
            &[("env", "a"), ("team", "three")],
            json!([{"Key": "team", "Value": "final"}]),
            &[("env", "a"), ("team", "final")],
        ),
        (
            "ChangeSetTags repeating the LAST key of the list",
            "ContainerProduct",
            false,
            json!([
                {"Key": "wave", "Value": "one"},
                {"Key": "owner", "Value": "me"},
                {"Key": "owner", "Value": "you"},
            ]),
            &[("owner", "you"), ("wave", "one")],
            json!([{"Key": "owner", "Value": "us"}, {"Key": "wave", "Value": "two"}]),
            &[("owner", "us"), ("wave", "two")],
        ),
        (
            "EntityTags naming one key twice",
            "AmiProduct",
            true,
            json!([{"Key": "env", "Value": "a"}, {"Key": "env", "Value": "b"}]),
            &[("env", "b")],
            json!([{"Key": "env", "Value": "c"}]),
            &[("env", "c")],
        ),
        (
            "EntityTags repeating two different keys, with an empty value last",
            "MachineLearningProduct",
            true,
            json!([
                {"Key": "a", "Value": "1"},
                {"Key": "b", "Value": "1"},
                {"Key": "a", "Value": "2"},
                {"Key": "b", "Value": ""},
            ]),
            &[("a", "2"), ("b", "")],
            json!([{"Key": "a", "Value": "3"}, {"Key": "c", "Value": "9"}]),
            &[("a", "3"), ("b", ""), ("c", "9")],
        ),
        (
            "TagResource itself naming one key twice, on a change set carrying it already",
            "DataProduct",
            false,
            json!([{"Key": "k", "Value": "0"}]),
            &[("k", "0")],
            json!([
                {"Key": "k", "Value": "1"},
                {"Key": "other", "Value": "x"},
                {"Key": "k", "Value": "2"},
            ]),
            &[("k", "2"), ("other", "x")],
        ),
    ];

    for (case, entity_type, on_entity, submitted, stored, retag, after_retag) in rows {
        let router = emulator();
        let (change_set_tags, entity_tags) = if on_entity {
            (None, Some(submitted))
        } else {
            (Some(submitted), None)
        };
        let (entity_arn, change_set_arn) =
            created_resources(&router, entity_type, change_set_tags, entity_tags).await;
        let arn = if on_entity { &entity_arn } else { &change_set_arn };

        let expected_stored: Vec<(String, String)> = stored
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        let listed = list_tags(&router, arn).await;
        assert_eq!(
            (listed.status, listed.content_type.as_str()),
            (200, JSON),
            "case {case:?}: ListTagsForResource must answer 200 application/json, got {} {:?} \
             with {:?}",
            listed.status,
            listed.content_type,
            listed.body
        );
        assert_eq!(
            listed.tag_pairs(),
            expected_stored,
            "case {case:?}: the submitted list must be stored as one value per key, got {:?}",
            listed.tag_entries()
        );
        assert_eq!(
            listed.tag_entries().len(),
            listed.tag_keys().len(),
            "case {case:?}: no key may be reported twice, got {:?}",
            listed.tag_entries()
        );

        let tagged = tag_resource(&router, arn, retag).await;
        assert_eq!(
            (tagged.status, tagged.content_type.as_str()),
            (200, JSON),
            "case {case:?}: TagResource must answer 200 application/json, got {} {:?} with {:?}",
            tagged.status,
            tagged.content_type,
            tagged.body
        );

        let expected_after: Vec<(String, String)> = after_retag
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        let reread = list_tags(&router, arn).await;
        assert_eq!(
            reread.tag_pairs(),
            expected_after,
            "case {case:?}: every key TagResource named must report the value it wrote, got {:?}",
            reread.tag_entries()
        );
        assert_eq!(
            reread.tag_entries().len(),
            reread.tag_keys().len(),
            "case {case:?}: no key may be reported twice after TagResource, got {:?}",
            reread.tag_entries()
        );
    }
}

/// The per-resource tag bound counts the KEYS a resource holds, so a list whose repeated
/// keys collapse under the bound is stored, and the first key past it is refused.
#[tokio::test]
async fn the_resource_tag_bound_counts_keys() {
    let router = emulator();

    // `TagList` is `@length(min: 1, max: 200)`, so one list names at most that many pairs:
    // start the change set with a full list in which every key is named TWICE, which is
    // half the bound in keys.
    let keys = MAX_RESOURCE_TAGS / 2;
    let repeated: Vec<Value> = (0..keys)
        .flat_map(|index| {
            [
                json!({"Key": format!("k{index}"), "Value": "first"}),
                json!({"Key": format!("k{index}"), "Value": "last"}),
            ]
        })
        .collect();
    let (_, change_set_arn) =
        created_resources(&router, "AmiProduct", Some(json!(repeated)), None).await;

    let seeded = list_tags(&router, &change_set_arn).await;
    assert_eq!(
        seeded.tag_entries().len(),
        keys,
        "a list of {} pairs naming {keys} keys must be stored as {keys} tags, got {:?}",
        MAX_RESOURCE_TAGS,
        seeded.tag_entries()
    );
    assert!(
        seeded
            .tag_entries()
            .iter()
            .all(|(_, value)| value == "last"),
        "every stored key must carry the last value its list gave, got {:?}",
        seeded.tag_entries()
    );

    // The keys already held plus these reach the bound exactly, which is only true if the
    // accounting counts keys rather than the pairs the first list carried.
    let fresh: Vec<Value> = (keys..MAX_RESOURCE_TAGS)
        .map(|index| json!({"Key": format!("k{index}"), "Value": "v"}))
        .collect();
    let filled = tag_resource(&router, &change_set_arn, json!(fresh)).await;
    assert_eq!(
        filled.status, 200,
        "reaching the bound in KEYS must answer 200, got {} with {:?}",
        filled.status, filled.body
    );
    let at_bound = list_tags(&router, &change_set_arn).await;
    assert_eq!(
        at_bound.tag_entries().len(),
        MAX_RESOURCE_TAGS,
        "the resource must hold the whole bound in keys, got {} tags",
        at_bound.tag_entries().len()
    );

    // One key past the bound is refused, and a repeated key that is ALREADY held is not
    // past it: it is the same key, whatever the number of pairs naming it.
    let over = tag_resource(
        &router,
        &change_set_arn,
        json!([{"Key": "one-too-many", "Value": "v"}]),
    )
    .await;
    assert_eq!(
        (over.status, over.error_code()),
        (422, "ValidationException"),
        "one key past the bound must be refused 422 ValidationException, got {} {:?} with {:?}",
        over.status,
        over.error_code(),
        over.body
    );
    let held_again = tag_resource(
        &router,
        &change_set_arn,
        json!([{"Key": "k0", "Value": "one"}, {"Key": "k0", "Value": "two"}]),
    )
    .await;
    assert_eq!(
        held_again.status, 200,
        "re-tagging a key the resource holds must answer 200 at the bound, got {} with {:?}",
        held_again.status, held_again.body
    );
    let after = list_tags(&router, &change_set_arn).await;
    assert_eq!(
        after.tag_entries().len(),
        MAX_RESOURCE_TAGS,
        "re-tagging a held key must add no entry, got {} tags",
        after.tag_entries().len()
    );
    assert!(
        after
            .tag_entries()
            .contains(&("k0".to_string(), "two".to_string())),
        "the re-tagged key must report the last value written for it, got {:?}",
        after.tag_entries()
    );
}
