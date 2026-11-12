// SPDX-License-Identifier: CC0-1.0

//! `DeleteResourcePolicy` of the AWS Marketplace Catalog, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the boundary validator, the handler, the persisted state and
//! serialization — the same path a client takes.
//!
//! The rule under test: a resource-based policy is recorded per ENTITY resource ARN — the
//! kind this operation is documented for — so clearing one asks for an end state (no policy
//! attached) and answers 200 with an empty document whether or not one was attached, for
//! any ARN of this catalog naming an entity. What is refused with the modelled
//! `ResourceNotFoundException` (HTTP 404), naming the id asked for, is an ARN that
//! identifies no entity resource of this catalog: a resource type this catalog does not
//! have, a resource of its other KIND (a change set, which only the tagging surface is
//! documented for — see `resource_policy_entity_only.rs`), or an id this catalog holds a
//! record of under ANOTHER entity type, since the type is part of what the ARN names. An id
//! this catalog simply holds no record of is not such an ARN — the catalog's own records are
//! the resources this emulator's change sets created, not every resource an account holds —
//! and a refused call changes nothing another caller can see.
//!
//! The rows differ along the resource kind the ARN names (entity or change set), the
//! entity type, whether the id is held at all, whether it is held under ANOTHER type or
//! kind, and whether the held resource carried a policy when it was cleared.

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

/// The smallest document `PutResourcePolicy` accepts: one statement inside
/// `ResourcePolicyJson`'s `@length(min: 1, max: 10240)`
/// (`smithy_query(query="operation", operation="PutResourcePolicy")`). A case ATTACHES it;
/// the service never invents one for a resource that carries none.
const MINIMAL_POLICY_DOCUMENT: &str = "{\"Version\":\"2012-10-17\",\"Statement\":\
     [{\"Effect\":\"Allow\",\"Principal\":\"*\",\"Action\":\"aws-marketplace:DescribeEntity\"}]}";

/// The document a resource carrying no resource-based policy reports: the empty JSON
/// policy document, which states no `Statement` and so grants what no attached policy
/// grants. `PutResourcePolicy` refuses it, so reading it back can only mean that nothing
/// is attached (see `resource_policy_lifecycle.rs`).
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

/// The id portion of a resource ARN: the last `/`-separated segment of its resource field,
/// which is what a refusal naming the resource names.
fn resource_id_of(arn: &str) -> String {
    arn.rsplit('/').next().unwrap_or(arn).to_string()
}

/// An ARN of this catalog's own form, naming `id` under `resource_type`.
fn catalog_arn(resource_type: &str, id: &str) -> String {
    format!("arn:aws:aws-marketplace:{REGION}:{ACCOUNT}:{CATALOG}/{resource_type}/{id}")
}

/// The sentence this service states a missing entity in, naming the entity that was asked
/// for — the one form every "does not exist" refusal of this service follows.
fn entity_missing(id: &str) -> String {
    format!("Requested entity '{id}' does not exist.")
}


/// An ARN of this catalog's form that identifies no ENTITY resource of this catalog is
/// refused by `DeleteResourcePolicy` with 404 `ResourceNotFoundException` naming the id
/// asked for — an ARN whose ID IS held, but under another entity type, an ARN naming the
/// catalog's other resource kind (a change set), and an ARN whose type this catalog does
/// not have at all — and such a refusal leaves the policy of a resource the catalog does
/// hold exactly as it was. An ARN whose id this catalog merely holds no record of is NOT
/// refused: the clear asks for an end state that ARN is already in.
///
/// The rows differ along the resource kind, the resource type, and whether the id is held
/// at all, held under another entity type, or held as the other kind of resource.
#[tokio::test]
async fn delete_resource_policy_refuses_an_arn_naming_no_entity_of_this_catalog() {
    let router = emulator();
    let (entity_arn, change_set_arn) = created_resources(&router, "AmiProduct").await;
    let held_entity_id = resource_id_of(&entity_arn);
    let held_change_set_id = resource_id_of(&change_set_arn);

    // A resource that IS held, carrying a policy, so each answer below is shown to be
    // about the ARN rather than about the operation — and to leave that policy alone.
    let attached = put_resource_policy(&router, &entity_arn, MINIMAL_POLICY_DOCUMENT).await;
    assert_eq!(
        attached.status, 200,
        "attaching a policy to the held entity must answer 200, got {} with {:?}",
        attached.status, attached.body
    );

    // (case, the ARN, the sentence the refusal must state)
    let refused_rows: Vec<(&str, String, String)> = vec![
        (
            "the held entity's id under another entity type",
            catalog_arn("SaaSProduct", &held_entity_id),
            entity_missing(&held_entity_id),
        ),
        (
            "the held entity's id as a change set",
            catalog_arn(CHANGE_SET_RESOURCE_TYPE, &held_entity_id),
            entity_missing(&held_entity_id),
        ),
        (
            "the held change set itself",
            change_set_arn.clone(),
            entity_missing(&held_change_set_id),
        ),
        (
            "a change set id nothing started",
            catalog_arn(CHANGE_SET_RESOURCE_TYPE, "cs-000000000009"),
            entity_missing("cs-000000000009"),
        ),
        (
            "the held change set's id as an entity",
            catalog_arn("AmiProduct", &held_change_set_id),
            entity_missing(&held_change_set_id),
        ),
        (
            "a resource type this catalog does not have",
            catalog_arn("Bucket", "prod-d4e5f6g7h8i9j"),
            entity_missing("prod-d4e5f6g7h8i9j"),
        ),
        (
            "a plural of a resource type this catalog has",
            catalog_arn("ChangeSets", "cs-000000000009"),
            entity_missing("cs-000000000009"),
        ),
    ];

    for (case, arn, missing) in refused_rows {
        let refused = delete_resource_policy(&router, &arn).await;
        assert_eq!(
            (
                refused.status,
                refused.error_code(),
                refused.content_type.as_str(),
                refused.errortype.as_str(),
                refused.message(),
            ),
            (
                404,
                "ResourceNotFoundException",
                JSON,
                "ResourceNotFoundException",
                missing.clone(),
            ),
            "case {case:?}: DeleteResourcePolicy must refuse {arn:?} with 404 \
             ResourceNotFoundException naming the resource asked for, got {} {:?} with {:?}",
            refused.status,
            refused.error_code(),
            refused.body
        );

        // The refusal changed nothing: repeating it is refused identically, and the held
        // resource still carries the document that was attached to it.
        let again = delete_resource_policy(&router, &arn).await;
        assert_eq!(
            (again.status, again.error_code(), again.message()),
            (404, "ResourceNotFoundException", missing),
            "case {case:?}: a repeated DeleteResourcePolicy must be refused identically, \
             got {} {:?} with {:?}",
            again.status,
            again.error_code(),
            again.body
        );

        let held = get_resource_policy(&router, &entity_arn).await;
        assert_eq!(
            (held.status, held.text("/Policy")),
            (200, MINIMAL_POLICY_DOCUMENT.to_string()),
            "case {case:?}: the refused call must leave the held resource's policy as it \
             was, got {} with {:?}",
            held.status,
            held.body
        );
    }

    // The ARNs this catalog simply holds no record of: the clear asks for the end state
    // they are already in, so it is served, and it still leaves the held resource's own
    // policy alone.
    let served_rows: Vec<(&str, String)> = vec![
        (
            "an AmiProduct id nothing created",
            catalog_arn("AmiProduct", "prod-b2c3d4e5f6g7h"),
        ),
        (
            "a SaaSProduct id nothing created",
            catalog_arn("SaaSProduct", "prod-c3d4e5f6g7h8i"),
        ),
        (
            "an id of an entity type nothing created",
            catalog_arn("Offer", "offer-1a2b3c4d5e6f7"),
        ),
    ];

    for (case, arn) in served_rows {
        for round in 0..2 {
            let cleared = delete_resource_policy(&router, &arn).await;
            assert_eq!(
                (cleared.status, cleared.content_type.as_str(), &cleared.body),
                (200, JSON, &json!({})),
                "case {case:?} round {round}: clearing the policy of an ARN this catalog \
                 holds no record of must answer 200 application/json with an empty \
                 document, got {} {:?} with {:?}",
                cleared.status,
                cleared.content_type,
                cleared.body
            );
        }

        let held = get_resource_policy(&router, &entity_arn).await;
        assert_eq!(
            (held.status, held.text("/Policy")),
            (200, MINIMAL_POLICY_DOCUMENT.to_string()),
            "case {case:?}: a clear on another ARN must leave the held resource's policy \
             as it was, got {} with {:?}",
            held.status,
            held.body
        );
    }
}

/// For an ENTITY the catalog DOES hold, `DeleteResourcePolicy` answers 200 with an empty
/// document and leaves the resource carrying no policy — the end state it asks for — so a
/// resource that carried none is the same success as one that carried a document, and the
/// resource itself survives the call.
///
/// The rows differ along the entity type, and along whether a policy was attached before
/// the clear.
#[tokio::test]
async fn delete_resource_policy_clears_the_policy_of_a_resource_the_catalog_holds() {
    let router = emulator();
    let (ami_entity_arn, _) = created_resources(&router, "AmiProduct").await;
    let (saas_entity_arn, _) = created_resources(&router, "SaaSProduct").await;

    // (case, the held resource's ARN, whether a policy is attached before the clear)
    let rows: Vec<(&str, &str, bool)> = vec![
        ("an AmiProduct entity carrying a policy", &ami_entity_arn, true),
        ("an AmiProduct entity carrying none", &ami_entity_arn, false),
        ("a SaaSProduct entity carrying a policy", &saas_entity_arn, true),
        ("a SaaSProduct entity carrying none", &saas_entity_arn, false),
    ];

    for (case, arn, attach_first) in rows {
        if attach_first {
            let attached = put_resource_policy(&router, arn, MINIMAL_POLICY_DOCUMENT).await;
            assert_eq!(
                attached.status, 200,
                "case {case:?}: attaching must answer 200, got {} with {:?}",
                attached.status, attached.body
            );
        }

        let cleared = delete_resource_policy(&router, arn).await;
        assert_eq!(
            (cleared.status, cleared.content_type.as_str(), &cleared.body),
            (200, JSON, &json!({})),
            "case {case:?}: DeleteResourcePolicy must answer 200 application/json with an \
             empty document, got {} {:?} with {:?}",
            cleared.status,
            cleared.content_type,
            cleared.body
        );

        // The end state: the resource is still held — the read reaches the handler and
        // reports the resource's policy, which is now the EMPTY policy document — so
        // clearing again is the same success.
        let read = get_resource_policy(&router, arn).await;
        assert_eq!(
            (read.status, read.text("/Policy")),
            (200, EMPTY_POLICY_DOCUMENT.to_string()),
            "case {case:?}: after the clear the resource must carry no policy and report \
             the empty policy document, got {} {:?} with {:?}",
            read.status,
            read.error_code(),
            read.body
        );

        let cleared_again = delete_resource_policy(&router, arn).await;
        assert_eq!(
            (
                cleared_again.status,
                cleared_again.content_type.as_str(),
                &cleared_again.body
            ),
            (200, JSON, &json!({})),
            "case {case:?}: clearing an already absent policy must answer 200 with an \
             empty document, got {} {:?} with {:?}",
            cleared_again.status,
            cleared_again.content_type,
            cleared_again.body
        );

        // And the resource is still attachable, so the clear removed the policy and not
        // the resource's record.
        let reattached = put_resource_policy(&router, arn, MINIMAL_POLICY_DOCUMENT).await;
        assert_eq!(
            reattached.status, 200,
            "case {case:?}: the resource must still be held after a clear, got {} with {:?}",
            reattached.status, reattached.body
        );
        let final_clear = delete_resource_policy(&router, arn).await;
        assert_eq!(
            final_clear.status, 200,
            "case {case:?}: clearing the re-attached policy must answer 200, got {} with {:?}",
            final_clear.status, final_clear.body
        );
    }
}
