// SPDX-License-Identifier: CC0-1.0

//! Which RESOURCE KIND the resource-policy operations of the AWS Marketplace Catalog act
//! on, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the boundary validator, the handler, the persisted state and
//! serialization — the same path a client takes.
//!
//! The rule under test: every resource-policy operation is documented for an ENTITY
//! resource ("attaches a resource-based policy to an entity", "a resource-based policy of
//! an entity that is identified by its resource ARN", and `ResourceArn` as "the ARN of the
//! entity resource"), so a resource ARN of this catalog naming a resource of any OTHER kind
//! it has — a change set, which only the tagging surface is documented for ("a resource
//! (either an entity or change set)") — names no resource for those operations to act on
//! and is refused with the modelled `ResourceNotFoundException` (HTTP 404), naming the id
//! the ARN asked for as the entity it is not. The tagging surface keeps acting on both
//! kinds.
//!
//! The rows differ along whether the change set the ARN names is one this emulator holds,
//! the form of the change set id, the partition and region the ARN is written with, and the
//! entity type of the change set's own entity.

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

/// A SigV4 credential scope naming this service's signing name (`aws-marketplace`). Its
/// region is the one the ARNs this service mints name.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The region [`SIGNED_FOR_EU_WEST_1`] signs for.
const REGION: &str = "eu-west-1";

/// The account every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The restJson1 document media type: `application/json`, for answers and refusals alike.
const JSON: &str = "application/json";

/// The resource type segment a change set's ARN carries, as this service mints it.
const CHANGE_SET_RESOURCE_TYPE: &str = "ChangeSet";

/// The smallest document `PutResourcePolicy` accepts: one statement stating its effect, its
/// action and the principal it is granted to, inside `ResourcePolicyJson`'s
/// `@length(min: 1, max: 10240)`
/// (`smithy_query(query="operation", operation="PutResourcePolicy")`).
const MINIMAL_POLICY_DOCUMENT: &str = "{\"Version\":\"2012-10-17\",\"Statement\":\
     [{\"Effect\":\"Allow\",\"Principal\":\"*\",\"Action\":\"aws-marketplace:DescribeEntity\"}]}";

/// The document a resource carrying no resource-based policy reports: the empty JSON policy
/// document, which states no `Statement` and so grants what no attached policy grants.
const EMPTY_POLICY_DOCUMENT: &str = "{}";

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

/// What the emulator answered: the status, the JSON body and the two headers a restJson1
/// client reads an error from.
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

    /// The `(key, value)` pairs of a `ListTagsForResource` answer, sorted.
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

async fn get_resource_policy(router: &MultiServiceRouter, resource_arn: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/GetResourcePolicy?resourceArn={resource_arn}"),
        None,
    )
    .await
}

async fn put_resource_policy(
    router: &MultiServiceRouter,
    resource_arn: &str,
    policy: &str,
) -> Answer {
    call(
        router,
        "POST",
        "/PutResourcePolicy",
        Some(json!({"ResourceArn": resource_arn, "Policy": policy})),
    )
    .await
}

async fn delete_resource_policy(router: &MultiServiceRouter, resource_arn: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/DeleteResourcePolicy?resourceArn={resource_arn}"),
        None,
    )
    .await
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

/// Every resource-policy operation, invoked against one ARN: the three the model binds a
/// `ResourceArn` of an entity resource to.
async fn policy_operations(router: &MultiServiceRouter, arn: &str) -> Vec<(&'static str, Answer)> {
    vec![
        (
            "PutResourcePolicy",
            put_resource_policy(router, arn, MINIMAL_POLICY_DOCUMENT).await,
        ),
        ("GetResourcePolicy", get_resource_policy(router, arn).await),
        (
            "DeleteResourcePolicy",
            delete_resource_policy(router, arn).await,
        ),
    ]
}

/// The two resources one `CreateProduct` change set makes: the entity's ARN and the change
/// set's own, each read back out of the service through its own describe, so every case
/// addresses a resource the store really holds under the ARN the service minted for it.
async fn created_resources(router: &MultiServiceRouter, entity_type: &str) -> (String, String) {
    let started = call(
        router,
        "POST",
        "/StartChangeSet",
        Some(json!({
            "Catalog": CATALOG,
            "ChangeSet": [{
                "ChangeType": "CreateProduct",
                "Entity": {"Type": format!("{entity_type}@1.0")},
                "Details": json!({"ProductTitle": entity_type}).to_string(),
            }],
        })),
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

    // The entity the change set created, as `ListChangeSets` reports it on the change
    // set's own summary (`EntityIdList`).
    let listed = call(
        router,
        "POST",
        "/ListChangeSets",
        Some(json!({"Catalog": CATALOG})),
    )
    .await;
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
            panic!(
                "change set {change_set_id} must report its entity: {:?}",
                listed.body
            )
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
        "the created entity must be readable, got {} with {:?}",
        entity.status, entity.body
    );
    let entity_arn = entity.text("/EntityArn");

    assert!(
        !entity_arn.is_empty() && !change_set_arn.is_empty(),
        "both resources must be named by an ARN the service minted, got {entity_arn:?} \
         and {change_set_arn:?}"
    );
    (entity_arn, change_set_arn)
}

/// An ARN of this catalog's own form, naming `id` under `resource_type`.
fn catalog_arn(resource_type: &str, id: &str) -> String {
    format!("arn:aws:aws-marketplace:{REGION}:{ACCOUNT}:{CATALOG}/{resource_type}/{id}")
}

/// The same resource, named by an ARN written for another partition and region: the
/// partition and region say WHERE a resource lives, not what it is.
fn rewritten_for_another_region(arn: &str) -> String {
    let resource = arn.splitn(6, ':').nth(5).unwrap_or_default();
    format!("arn:aws-cn:aws-marketplace:cn-north-1:{ACCOUNT}:{resource}")
}

/// The id portion of a resource ARN: the last `/`-separated segment of its resource field.
fn resource_id_of(arn: &str) -> String {
    arn.rsplit('/').next().unwrap_or(arn).to_string()
}

/// The sentence this service states a missing entity in, naming the id that was asked for —
/// the one form every "does not exist" refusal of this service follows.
fn entity_missing(id: &str) -> String {
    format!("Requested entity '{id}' does not exist.")
}

/// A resource ARN naming a CHANGE SET names no entity resource, so every resource-policy
/// operation refuses it with 404 `ResourceNotFoundException` naming the id it asked for,
/// attaches nothing and clears nothing — while the entity of that same change set is served
/// by all three.
///
/// The rows differ along whether the change set is one this emulator holds, the form of the
/// change set id, and the partition and region the ARN is written with.
#[tokio::test]
async fn the_resource_policy_operations_refuse_a_change_set_arn() {
    let router = emulator();
    let (entity_arn, change_set_arn) = created_resources(&router, "AmiProduct").await;
    let held_change_set_id = resource_id_of(&change_set_arn);
    let held_entity_id = resource_id_of(&entity_arn);

    // The entity of that change set carries a policy, so each refusal below is shown to be
    // about the ARN rather than about the operation — and to leave that policy alone.
    let attached = put_resource_policy(&router, &entity_arn, MINIMAL_POLICY_DOCUMENT).await;
    assert_eq!(
        attached.status, 200,
        "attaching a policy to the entity must answer 200, got {} with {:?}",
        attached.status, attached.body
    );

    // (case, an ARN naming a change set). The ids are `ResourceId`'s `^[\w\-]+$`: the one
    // this emulator minted, ids in other forms it never minted, and the HELD ENTITY's id
    // addressed as a change set.
    let rows: Vec<(String, String)> = vec![
        (
            "the change set this emulator holds".to_string(),
            change_set_arn.clone(),
        ),
        (
            "that same change set in another partition and region".to_string(),
            rewritten_for_another_region(&change_set_arn),
        ),
        (
            "a change set id nothing started".to_string(),
            catalog_arn(CHANGE_SET_RESOURCE_TYPE, "cs-000000000009"),
        ),
        (
            "the shortest change set id form".to_string(),
            catalog_arn(CHANGE_SET_RESOURCE_TYPE, "cs1"),
        ),
        (
            "a long change set id".to_string(),
            catalog_arn(CHANGE_SET_RESOURCE_TYPE, "cs-0a1b2c3d4e5f6a7b8c9d"),
        ),
        (
            "a mixed-case change set id".to_string(),
            catalog_arn(CHANGE_SET_RESOURCE_TYPE, "Cs-9X"),
        ),
        (
            "the held entity's id under the change set type".to_string(),
            catalog_arn(CHANGE_SET_RESOURCE_TYPE, &held_entity_id),
        ),
    ];

    for (case, arn) in rows {
        let missing = entity_missing(&resource_id_of(&arn));
        // Twice, so a refusal is shown to leave nothing behind that a second call reads
        // differently.
        for round in 0..2 {
            for (operation, refused) in policy_operations(&router, &arn).await {
                assert_eq!(
                    (
                        refused.status,
                        refused.error_code().to_string(),
                        refused.content_type.clone(),
                        refused.errortype.clone(),
                        refused.message(),
                    ),
                    (
                        404,
                        "ResourceNotFoundException".to_string(),
                        JSON.to_string(),
                        "ResourceNotFoundException".to_string(),
                        missing.clone(),
                    ),
                    "case {case:?}/{operation} round {round} on {arn:?}: an ARN naming a \
                     change set names no entity resource and must be refused 404 \
                     ResourceNotFoundException naming the id asked for, got {} {:?} with {:?}",
                    refused.status,
                    refused.error_code(),
                    refused.body
                );
            }
        }

        // Nothing the refused calls touched: the entity still carries exactly the document
        // that was attached to it.
        let held = get_resource_policy(&router, &entity_arn).await;
        assert_eq!(
            (held.status, held.text("/Policy")),
            (200, MINIMAL_POLICY_DOCUMENT.to_string()),
            "case {case:?}: a refused call on a change set ARN must leave the entity's \
             policy alone, got {} with {:?}",
            held.status,
            held.body
        );
    }

    // The change set itself survived every refusal and is still what the catalog holds it
    // as — a policy call neither created nor removed a resource.
    let described = call(
        &router,
        "GET",
        &format!("/DescribeChangeSet?catalog={CATALOG}&changeSetId={held_change_set_id}"),
        None,
    )
    .await;
    assert_eq!(
        (described.status, described.text("/ChangeSetArn")),
        (200, change_set_arn.clone()),
        "the change set must still be held after the refusals, got {} with {:?}",
        described.status,
        described.body
    );
}

/// The entity resource the policy operations ARE documented for is served by all three: the
/// report before any attachment is the empty document, an attached document is reported
/// back, and the clear returns the report to the empty document.
///
/// The rows differ along the entity type of the entity addressed.
#[tokio::test]
async fn the_resource_policy_operations_serve_the_entity_of_the_same_change_set() {
    let router = emulator();

    for entity_type in ["AmiProduct", "SaaSProduct"] {
        let (entity_arn, _) = created_resources(&router, entity_type).await;

        let fresh = get_resource_policy(&router, &entity_arn).await;
        assert_eq!(
            (fresh.status, fresh.text("/Policy")),
            (200, EMPTY_POLICY_DOCUMENT.to_string()),
            "case {entity_type:?}: an entity carrying no policy must report the empty \
             policy document, got {} with {:?}",
            fresh.status,
            fresh.body
        );

        let attached = put_resource_policy(&router, &entity_arn, MINIMAL_POLICY_DOCUMENT).await;
        assert_eq!(
            (attached.status, attached.content_type.as_str()),
            (200, JSON),
            "case {entity_type:?}: attaching to the entity must answer 200 \
             application/json, got {} {:?} with {:?}",
            attached.status,
            attached.content_type,
            attached.body
        );

        let read = get_resource_policy(&router, &entity_arn).await;
        assert_eq!(
            (read.status, read.text("/Policy")),
            (200, MINIMAL_POLICY_DOCUMENT.to_string()),
            "case {entity_type:?}: the report must be the document that was attached, got \
             {} with {:?}",
            read.status,
            read.body
        );

        let cleared = delete_resource_policy(&router, &entity_arn).await;
        assert_eq!(
            (cleared.status, &cleared.body),
            (200, &json!({})),
            "case {entity_type:?}: the clear must answer an empty 200, got {} with {:?}",
            cleared.status,
            cleared.body
        );

        let after_clear = get_resource_policy(&router, &entity_arn).await;
        assert_eq!(
            (after_clear.status, after_clear.text("/Policy")),
            (200, EMPTY_POLICY_DOCUMENT.to_string()),
            "case {entity_type:?}: a cleared entity must report the empty policy document \
             again, got {} with {:?}",
            after_clear.status,
            after_clear.body
        );
    }
}

/// The tagging surface is documented for "a resource (either an entity or change set)", so
/// it keeps acting on a change set ARN that the policy surface refuses: the tags written
/// through it are what the read reports back, for the change set as much as for its entity.
///
/// The rows differ along the resource kind the ARN names and the entity type of the change
/// set's own entity.
#[tokio::test]
async fn the_tagging_surface_still_acts_on_a_change_set() {
    let router = emulator();

    for entity_type in ["ContainerProduct", "DataProduct"] {
        let (entity_arn, change_set_arn) = created_resources(&router, entity_type).await;

        for (kind, arn) in [("entity", &entity_arn), ("change set", &change_set_arn)] {
            let case = format!("{entity_type} {kind}");
            let tagged = tag_resource(&router, arn, json!([{"Key": "env", "Value": "test"}])).await;
            assert_eq!(
                (tagged.status, tagged.content_type.as_str()),
                (200, JSON),
                "case {case:?}: TagResource must answer 200 application/json for either \
                 resource kind, got {} {:?} with {:?}",
                tagged.status,
                tagged.content_type,
                tagged.body
            );

            let listed = list_tags(&router, arn).await;
            assert_eq!(
                (listed.status, listed.tag_pairs()),
                (200, vec![("env".to_string(), "test".to_string())]),
                "case {case:?}: ListTagsForResource must report the tag that was written, \
                 got {} with {:?}",
                listed.status,
                listed.body
            );
            assert_eq!(
                listed.text("/ResourceArn"),
                **arn,
                "case {case:?}: ListTagsForResource must report the stored ARN, got {:?}",
                listed.body
            );
        }

        // And the policy surface still refuses that same change set ARN, which is what
        // makes the two surfaces' documented resource kinds visible side by side.
        let refused = get_resource_policy(&router, &change_set_arn).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (404, "ResourceNotFoundException"),
            "case {entity_type:?}: a tagged change set is still no entity resource for the \
             policy surface, got {} {:?} with {:?}",
            refused.status,
            refused.error_code(),
            refused.body
        );
    }
}
