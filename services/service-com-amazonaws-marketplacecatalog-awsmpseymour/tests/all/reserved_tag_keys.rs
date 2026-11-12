// SPDX-License-Identifier: CC0-1.0

//! The tag-key namespace Amazon Web Services keeps for itself, end to end over HTTP.
//!
//! RULE: a tag key inside the `aws:` namespace (`state::RESERVED_TAG_KEY_PREFIX`, matched
//! however it is cased) names a tag Amazon Web Services creates, which a caller can
//! neither create, edit nor remove — so EVERY request member of this service that carries
//! tag keys is refused with the modelled `ValidationException` (422) when one of its keys
//! lies inside it, and the request writes nothing: not the keys beside the offending one,
//! and not the resource the request would otherwise have created. A key merely CONTAINING
//! the prefix, or carrying `aws` without the colon, is outside the namespace and is stored
//! like any other.
//!
//! The members the rule covers are every one in the crate that accepts tag keys:
//! `TagResource$Tags`, `UntagResource$TagKeys`, `StartChangeSet$ChangeSetTags` and each
//! change's `StartChangeSet$ChangeSet$EntityTags`.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the boundary validator, the handler, the persisted state and
//! serialization — the same path a client takes. The rows differ along what the rule
//! generalises over: the casing of the prefix, what follows it, where the offending key
//! sits in the list, how many keys the request names, which member carries them, and which
//! resource kind (entity or change set) they would have landed on.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::state::{
    is_reserved_tag_key, CHANGE_SET_APPLYING, CHANGE_SET_PREPARING, RESERVED_TAG_KEY_PREFIX,
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

/// What the emulator answered: the status, the JSON body and the header a restJson1 client
/// reads an error code from.
struct Answer {
    status: u16,
    body: Value,
    content_type: String,
    errortype: String,
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

    /// The `Message` of a refusal, under either casing a protocol spells it.
    fn message(&self) -> String {
        let lower = self.text("/message");
        if lower.is_empty() {
            self.text("/Message")
        } else {
            lower
        }
    }

    /// The tags this answer reports, as `(key, value)` pairs sorted by key, so a case
    /// asserts the SET of tags without pinning the order a list happens to carry.
    fn tag_pairs(&self) -> Vec<(String, String)> {
        let mut pairs: Vec<(String, String)> = self
            .body
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
            .collect();
        pairs.sort();
        pairs
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
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    let content_type = header("content-type");
    let errortype = header("x-amzn-errortype");
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
        errortype,
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

async fn untag_resource(router: &MultiServiceRouter, resource_arn: &str, keys: Value) -> Answer {
    call(
        router,
        "POST",
        "/UntagResource",
        Some(json!({"ResourceArn": resource_arn, "TagKeys": keys})),
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

/// The change sets the store holds, by id — so a case can assert that a refused
/// `StartChangeSet` started nothing.
async fn change_set_ids(router: &MultiServiceRouter) -> Vec<String> {
    let listed = call(router, "POST", "/ListChangeSets", Some(json!({"Catalog": CATALOG}))).await;
    listed
        .body
        .pointer("/ChangeSetSummaryList")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item["ChangeSetId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The two resources one succeeded `CreateProduct` change set makes: the entity's ARN,
/// read back through `DescribeEntity`, and the change set's own, read back through
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
    let entity_arn = entity.text("/EntityArn");
    assert!(
        !entity_arn.is_empty() && !change_set_arn.is_empty(),
        "both resources must carry an ARN, got entity {entity_arn:?} and change set {change_set_arn:?}"
    );
    (entity_arn, change_set_arn)
}

/// The keys inside the reserved namespace, each built from the prefix the crate publishes
/// rather than re-spelled here.
///
/// Source for the namespace itself: AWS's tagging rules keep the `aws:` prefix for the
/// tags AWS applies, which a caller cannot create, edit or delete — `Tag$Key`'s own
/// `@pattern` (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, `smithy_query(query="shape",
/// shape="Tag")`) admits the colon, so the key syntax cannot state it. The rows vary the
/// casing of the prefix and what follows it, including entries the issue never cited.
fn reserved_keys() -> Vec<(&'static str, String)> {
    let prefix = RESERVED_TAG_KEY_PREFIX;
    let upper = prefix.to_uppercase();
    vec![
        ("the prefix with nothing after it", prefix.to_string()),
        ("a key AWS generates", format!("{prefix}createdBy")),
        (
            "a key another AWS service generates, with a second colon",
            format!("{prefix}cloudformation:stack-name"),
        ),
        ("the prefix in capitals", format!("{upper}CreatedBy")),
        (
            "the prefix in mixed case",
            format!("Aws{}servicecatalog", &prefix[3..]),
        ),
        (
            "a key whose remainder uses the whole published key character class",
            format!("{prefix}cost.centre/eu=1+2@3"),
        ),
    ]
}

/// Keys OUTSIDE the namespace, which the rule leaves alone: the prefix without its colon,
/// the prefix in the middle of a key, and a key that merely starts with those letters.
fn allowed_keys() -> Vec<(&'static str, String)> {
    let prefix = RESERVED_TAG_KEY_PREFIX;
    let bare = prefix.trim_end_matches(':').to_string();
    vec![
        ("the prefix letters without the colon", bare.clone()),
        ("a word starting with those letters", format!("{bare}ome-key")),
        ("the prefix inside the key", format!("my-{prefix}thing")),
        ("the prefix after a separator", format!("team/{prefix}owner")),
        ("an ordinary key", "cost.centre".to_string()),
    ]
}

/// RULE, for `TagResource`: a request naming a key inside the reserved namespace is
/// refused 422 `ValidationException` naming that key, and writes NOTHING — not even the
/// legal keys in the same request.
///
/// The rows cross the reserved keys with where the offending key sits in `Tags` (alone,
/// first, last) and with the resource kind it addressed (the entity, or the change set
/// that created it).
#[tokio::test]
async fn tag_resource_refuses_a_reserved_tag_key_and_writes_nothing() {
    for (case, key) in reserved_keys() {
        assert!(
            is_reserved_tag_key(&key),
            "case {case:?}: {key:?} must be a key of the reserved namespace"
        );
        // (how the request is built, whether the ENTITY or the change set is addressed)
        let placements: [(&str, Value, bool); 3] = [
            ("alone", json!([{"Key": key, "Value": "v"}]), true),
            (
                "first of three",
                json!([
                    {"Key": key, "Value": "v"},
                    {"Key": "env", "Value": "test"},
                    {"Key": "team", "Value": "catalog"},
                ]),
                true,
            ),
            (
                "last of two, on the change set",
                json!([{"Key": "env", "Value": "test"}, {"Key": key, "Value": "v"}]),
                false,
            ),
        ];

        for (placement, tags, on_entity) in placements {
            let router = emulator();
            let (entity_arn, change_set_arn) =
                created_resources(&router, "AmiProduct", None, None).await;
            let arn = if on_entity { &entity_arn } else { &change_set_arn };

            let refused = tag_resource(&router, arn, tags).await;
            assert_eq!(
                (
                    refused.status,
                    refused.error_code(),
                    refused.content_type.as_str(),
                    refused.errortype.as_str(),
                ),
                (422, "ValidationException", JSON, "ValidationException"),
                "case {case:?} ({placement}): a reserved tag key must be refused 422 \
                 ValidationException as application/json, got {} {:?} with {:?}",
                refused.status,
                refused.error_code(),
                refused.body
            );
            let message = refused.message();
            assert!(
                message.contains(&key) && message.contains(RESERVED_TAG_KEY_PREFIX),
                "case {case:?} ({placement}): the refusal must name the offending key and the \
                 reserved prefix, got {message:?}"
            );

            let after = list_tags(&router, arn).await;
            assert_eq!(
                after.tag_pairs(),
                Vec::<(String, String)>::new(),
                "case {case:?} ({placement}): a refused TagResource must write nothing, got {:?}",
                after.tag_pairs()
            );
        }
    }
}

/// RULE, for `UntagResource`: a key inside the reserved namespace names a tag AWS owns,
/// which a caller cannot remove — so the request is refused and the stored tags are left
/// exactly as they were, including the keys named beside it.
#[tokio::test]
async fn untag_resource_refuses_a_reserved_tag_key_and_removes_nothing() {
    for (case, key) in reserved_keys() {
        let router = emulator();
        let (entity_arn, _) = created_resources(&router, "SaaSProduct", None, None).await;
        let stored = tag_resource(
            &router,
            &entity_arn,
            json!([{"Key": "env", "Value": "test"}, {"Key": "team", "Value": "catalog"}]),
        )
        .await;
        assert_eq!(
            stored.status, 200,
            "case {case:?}: the legal tags must be stored first, got {} with {:?}",
            stored.status, stored.body
        );
        let before = list_tags(&router, &entity_arn).await;

        // The reserved key beside a key the resource DOES carry: the whole request is
        // refused, so the held key survives too.
        let refused = untag_resource(&router, &entity_arn, json!([key, "env"])).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (422, "ValidationException"),
            "case {case:?}: untagging a reserved key must be refused 422 ValidationException, \
             got {} {:?} with {:?}",
            refused.status,
            refused.error_code(),
            refused.body
        );
        assert!(
            refused.message().contains(&key),
            "case {case:?}: the refusal must name the offending key, got {:?}",
            refused.message()
        );

        let after = list_tags(&router, &entity_arn).await;
        assert_eq!(
            after.tag_pairs(),
            before.tag_pairs(),
            "case {case:?}: a refused UntagResource must remove nothing, got {:?} where {:?} \
             was stored",
            after.tag_pairs(),
            before.tag_pairs()
        );
    }
}

/// RULE, for the two tag members of `StartChangeSet`: a reserved key in `ChangeSetTags` or
/// in a change's `EntityTags` is refused 422 `ValidationException`, and no change set is
/// started at all — so neither the change set nor the entity its change describes comes
/// into being.
#[tokio::test]
async fn start_change_set_refuses_reserved_tags_and_starts_nothing() {
    for (case, key) in reserved_keys() {
        let tags = json!([{"Key": "env", "Value": "test"}, {"Key": key, "Value": "v"}]);
        // (member under test, change-set tags, entity tags)
        let rows: [(&str, Option<Value>, Option<Value>); 3] = [
            ("ChangeSetTags", Some(tags.clone()), None),
            ("ChangeSet.EntityTags", None, Some(tags.clone())),
            (
                "both members at once",
                Some(tags.clone()),
                Some(tags.clone()),
            ),
        ];

        for (member, change_set_tags, entity_tags) in rows {
            let router = emulator();
            let refused = call(
                &router,
                "POST",
                "/StartChangeSet",
                Some(start_body("ContainerProduct", change_set_tags, entity_tags)),
            )
            .await;
            assert_eq!(
                (
                    refused.status,
                    refused.error_code(),
                    refused.content_type.as_str(),
                ),
                (422, "ValidationException", JSON),
                "case {case:?} in {member}: a reserved tag key must be refused 422 \
                 ValidationException as application/json, got {} {:?} with {:?}",
                refused.status,
                refused.error_code(),
                refused.body
            );
            assert!(
                refused.message().contains(&key),
                "case {case:?} in {member}: the refusal must name the offending key, got {:?}",
                refused.message()
            );
            assert_eq!(
                change_set_ids(&router).await,
                Vec::<String>::new(),
                "case {case:?} in {member}: a refused StartChangeSet must start nothing"
            );
        }
    }
}

/// The other side of the rule: a key OUTSIDE the reserved namespace is stored by every
/// path that writes tags and read back through `ListTagsForResource`, and removed again by
/// `UntagResource` — so the refusal above is about the namespace and not about tagging.
///
/// The round trip goes through `StartChangeSet` (both tag members at once) and
/// `TagResource`, on both resource kinds.
#[tokio::test]
async fn a_key_outside_the_reserved_namespace_is_stored_by_every_tagging_path() {
    for (case, key) in allowed_keys() {
        assert!(
            !is_reserved_tag_key(&key),
            "case {case:?}: {key:?} must lie outside the reserved namespace"
        );
        let router = emulator();
        let at_start = json!([{"Key": key, "Value": "start"}]);
        let (entity_arn, change_set_arn) = created_resources(
            &router,
            "DataProduct",
            Some(at_start.clone()),
            Some(at_start.clone()),
        )
        .await;

        // Both members of the start carried the key, so both resources hold it.
        for (kind, arn) in [("entity", &entity_arn), ("change set", &change_set_arn)] {
            let listed = list_tags(&router, arn).await;
            assert_eq!(
                listed.tag_pairs(),
                vec![(key.clone(), "start".to_string())],
                "case {case:?}: the {kind} must report the tag the start put on it, got {:?}",
                listed.tag_pairs()
            );
        }

        // And `TagResource` edits that same stored copy.
        let tagged =
            tag_resource(&router, &entity_arn, json!([{"Key": key, "Value": "edited"}])).await;
        assert_eq!(
            tagged.status, 200,
            "case {case:?}: tagging with a key outside the namespace must answer 200, got {} \
             with {:?}",
            tagged.status, tagged.body
        );
        let edited = list_tags(&router, &entity_arn).await;
        assert_eq!(
            edited.tag_pairs(),
            vec![(key.clone(), "edited".to_string())],
            "case {case:?}: re-tagging the key must replace its value, got {:?}",
            edited.tag_pairs()
        );

        let untagged = untag_resource(&router, &entity_arn, json!([key])).await;
        assert_eq!(
            untagged.status, 200,
            "case {case:?}: untagging a key outside the namespace must answer 200, got {} with \
             {:?}",
            untagged.status, untagged.body
        );
        let emptied = list_tags(&router, &entity_arn).await;
        assert_eq!(
            emptied.tag_pairs(),
            Vec::<(String, String)>::new(),
            "case {case:?}: the key must be gone after untagging it, got {:?}",
            emptied.tag_pairs()
        );
    }
}
