// SPDX-License-Identifier: CC0-1.0

//! The AWS Marketplace Catalog tagging and resource-policy surface, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the boundary validator, the handler, the persisted state and
//! serialization — the same path a client takes.
//!
//! The rule under test is the one every operation addressed by a `ResourceArn` follows:
//! the ARN is checked against the modelled `ResourceARN` syntax, then authorization is
//! evaluated on it BEFORE any lookup (an ARN naming no resource of this catalog in this
//! account is refused `AccessDeniedException`, since no policy of this catalog can grant
//! an action on it), then existence (`ResourceNotFoundException` naming the resource the
//! ARN asked for), and only then does the operation read or write the stored record. The
//! tags and the policy an operation reports are the ones the store holds and nothing else.
//!
//! The existence step binds an operation to the catalog's own record of the resource — the
//! tagging surface throughout, and `PutResourcePolicy` — while reporting and clearing a
//! resource POLICY are addressed by the ARN alone (`resource_policy_lifecycle.rs`), since
//! a policy is recorded per ARN rather than on that record. Which resource KINDS an
//! operation acts on at all is its own: the tagging surface acts on an entity or a change
//! set, the resource-policy surface on an entity alone
//! (`resource_policy_entity_only.rs`).
//!
//! The rows differ along what that rule generalises over: which operation, which resource
//! kind (entity or change set), which entity type, how many tags, which keys, and which
//! policy document.

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

/// A SigV4 credential scope naming this service's signing name (`aws-marketplace`). It is
/// what routes a request here, and its region is what the ARNs this service mints name.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The region [`SIGNED_FOR_EU_WEST_1`] signs for.
const REGION: &str = "eu-west-1";

/// The account every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The `ResourceARN` `@pattern` the model declares for every `ResourceArn` member
/// (`smithy_query(query="operation", operation="ListTagsForResource")`), written out here
/// so a wrong implementation cannot make this suite pass by reporting its own pattern.
const RESOURCE_ARN_PATTERN: &str =
    r"^arn:[\w+=/,.@-]+:aws-marketplace:[\w+=/,.@-]*:[0-9]+:[\w+=,.@-]+(/[\w+=,.@-]+)*$";

/// The restJson1 document media type: `application/json`, for both answers and refusals.
const JSON: &str = "application/json";

/// The smallest document `PutResourcePolicy` accepts: a resource-based policy document
/// (the member is documented as "the policy document to set; formatted in JSON") inside
/// `ResourcePolicyJson`'s `@length(min: 1, max: 10240)` — one statement stating its effect,
/// its action and the principal it is granted to. It is a policy a case ATTACHES, never one
/// the service invents for a resource that carries none.
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

/// One row of the tagging table: the case, the entity type the case creates, whether it
/// addresses the ENTITY (`true`) or the change set that created it (`false`), the tags
/// added, the keys untagged, and the tags that must be left.
type TagRow = (
    &'static str,
    &'static str,
    bool,
    Value,
    Value,
    &'static [(&'static str, &'static str)],
);

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

/// The two resources one `CreateProduct` change set makes: the entity's ARN, read back
/// through `DescribeEntity`, and the change set's own, read back through
/// `DescribeChangeSet` — so every case below addresses a resource the store really holds,
/// named by the ARN the service itself minted for it.
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

/// `TagResource` / `UntagResource` write the one copy of a resource's tags that
/// `ListTagsForResource` reports, for every resource a resource ARN names: a key is
/// carried once, re-tagging it replaces its value, and untagging a key the resource does
/// not carry leaves the set alone.
///
/// The rows differ along the resource kind and entity type, the number of tags, and which
/// keys are removed.
#[tokio::test]
async fn tags_are_reported_from_the_stored_record() {
    // (case, entity type, resource kind, tags added, keys untagged, tags left)
    let rows: [TagRow; 5] = [
        (
            "one tag on an AmiProduct entity",
            "AmiProduct",
            true,
            json!([{"Key": "env", "Value": "test"}]),
            json!(["env"]),
            &[],
        ),
        (
            "three tags on a SaaSProduct entity, one removed",
            "SaaSProduct",
            true,
            json!([
                {"Key": "env", "Value": "prod"},
                {"Key": "team", "Value": "catalog"},
                {"Key": "cost.centre", "Value": "4711"},
            ]),
            json!(["team"]),
            &[("cost.centre", "4711"), ("env", "prod")],
        ),
        (
            "tags on a ContainerProduct entity, a key that was never added removed",
            "ContainerProduct",
            true,
            json!([{"Key": "owner", "Value": "me"}]),
            json!(["absent-key"]),
            &[("owner", "me")],
        ),
        (
            "tags on the change set itself",
            "DataProduct",
            false,
            json!([
                {"Key": "wave", "Value": "one"},
                {"Key": "wave-2", "Value": "two"},
            ]),
            json!(["wave"]),
            &[("wave-2", "two")],
        ),
        (
            "every tag removed at once",
            "MachineLearningProduct",
            true,
            json!([
                {"Key": "a", "Value": "1"},
                {"Key": "b", "Value": "2"},
            ]),
            json!(["a", "b"]),
            &[],
        ),
    ];

    for (case, entity_type, on_entity, added, removed, left) in rows {
        let router = emulator();
        let (entity_arn, change_set_arn) = created_resources(&router, entity_type).await;
        let arn = if on_entity { &entity_arn } else { &change_set_arn };

        // A resource that carries no tags reports no `Tags` member at all: `TagList` is
        // `@length(min: 1 …)`, which an empty list does not satisfy.
        let before = list_tags(&router, arn).await;
        assert_eq!(
            (before.status, before.content_type.as_str(), before.tag_pairs().len()),
            (200, JSON, 0),
            "case {case:?}: an untagged resource must report 200 application/json and no tags, \
             got {} {:?} with {:?}",
            before.status,
            before.content_type,
            before.body
        );
        assert_eq!(
            before.text("/ResourceArn"),
            *arn,
            "case {case:?}: ListTagsForResource must report the stored ARN, got {:?}",
            before.body
        );

        let tagged = tag_resource(&router, arn, added.clone()).await;
        assert_eq!(
            (tagged.status, tagged.content_type.as_str()),
            (200, JSON),
            "case {case:?}: TagResource must answer 200 application/json, got {} {:?} with {:?}",
            tagged.status,
            tagged.content_type,
            tagged.body
        );

        let expected_added: Vec<(String, String)> = {
            let mut pairs: Vec<(String, String)> = added
                .as_array()
                .expect("the case's tags are a list")
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
        };
        let listed = list_tags(&router, arn).await;
        assert_eq!(
            listed.tag_pairs(),
            expected_added,
            "case {case:?}: ListTagsForResource must report exactly the tags TagResource \
             stored, got {:?}",
            listed.body
        );

        // Re-tagging a key the resource already carries replaces its value rather than
        // adding a second entry: a tag is one value per key.
        let first_key = expected_added[0].0.clone();
        let retagged = tag_resource(
            &router,
            arn,
            json!([{"Key": first_key.clone(), "Value": "replaced"}]),
        )
        .await;
        assert_eq!(
            retagged.status, 200,
            "case {case:?}: re-tagging must answer 200, got {} with {:?}",
            retagged.status, retagged.body
        );
        let after_retag = list_tags(&router, arn).await;
        let mut expected_retag = expected_added.clone();
        for pair in &mut expected_retag {
            if pair.0 == first_key {
                pair.1 = "replaced".to_string();
            }
        }
        assert_eq!(
            after_retag.tag_pairs(),
            expected_retag,
            "case {case:?}: re-tagging {first_key:?} must replace its value and add no \
             entry, got {:?}",
            after_retag.body
        );

        // Put the original value back, so the removal rows below read as written.
        let restored = tag_resource(&router, arn, added).await;
        assert_eq!(
            restored.status, 200,
            "case {case:?}: restoring the tags must answer 200, got {} with {:?}",
            restored.status, restored.body
        );

        let untagged = untag_resource(&router, arn, removed.clone()).await;
        assert_eq!(
            (untagged.status, untagged.content_type.as_str()),
            (200, JSON),
            "case {case:?}: UntagResource must answer 200 application/json, got {} {:?} \
             with {:?}",
            untagged.status,
            untagged.content_type,
            untagged.body
        );
        let expected_left: Vec<(String, String)> = left
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        let after_untag = list_tags(&router, arn).await;
        assert_eq!(
            after_untag.tag_pairs(),
            expected_left,
            "case {case:?}: untagging {removed:?} must leave exactly {expected_left:?}, \
             got {:?}",
            after_untag.body
        );

        // Untagging is an end state, not a deletion of an entry: naming the same keys
        // again changes nothing and is not an error.
        let again = untag_resource(&router, arn, removed).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: untagging twice must answer 200, got {} with {:?}",
            again.status, again.body
        );
        let settled = list_tags(&router, arn).await;
        assert_eq!(
            settled.tag_pairs(),
            expected_left,
            "case {case:?}: untagging twice must leave the same set, got {:?}",
            settled.body
        );
    }
}

/// The resource-policy operations report what the store holds for the ENTITY resource they
/// are documented for: `GetResourcePolicy` answers the attached document,
/// `PutResourcePolicy` replaces it and `DeleteResourcePolicy` leaves the resource with none
/// — after which there is no policy to report, exactly as for a resource that never had
/// one. The change set that created that entity is a resource of the OTHER kind, which the
/// same three operations refuse.
///
/// The rows differ along the policy document put: its shape and its size.
#[tokio::test]
async fn a_resource_policy_is_reported_from_what_was_attached() {
    let policies: [(&str, String); 4] = [
        (
            "the smallest policy document",
            MINIMAL_POLICY_DOCUMENT.to_string(),
        ),
        (
            "a one-statement policy",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"AWS": format!("arn:aws:iam::{ACCOUNT}:root")},
                    "Action": ["aws-marketplace:DescribeEntity"],
                }],
            })
            .to_string(),
        ),
        (
            "a two-statement policy",
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Principal": "*",
                        "Action": ["aws-marketplace:DescribeEntity"],
                    },
                    {
                        "Effect": "Deny",
                        "Principal": {"AWS": format!("arn:aws:iam::{ACCOUNT}:root")},
                        "Action": ["aws-marketplace:StartChangeSet"],
                    },
                ],
            })
            .to_string(),
        ),
        (
            // Still inside `ResourcePolicyJson`'s `@length(max: 10240)`.
            "a policy with a long statement list",
            json!({
                "Version": "2012-10-17",
                "Statement": (0..40)
                    .map(|index| json!({
                        "Sid": format!("s{index}"),
                        "Effect": "Allow",
                        "Principal": "*",
                        "Action": ["aws-marketplace:DescribeEntity"],
                    }))
                    .collect::<Vec<Value>>(),
            })
            .to_string(),
        ),
    ];

    let router = emulator();
    let (entity_arn, change_set_arn) = created_resources(&router, "AmiProduct").await;

    {
        let arn = &entity_arn;
        // A resource the catalog holds that carries no policy has no attached document to
        // report: `Policy` targets `ResourcePolicyJson` (`@length(min: 1)`), so what the
        // read answers is the EMPTY policy document, which grants what no attached policy
        // grants.
        let fresh = get_resource_policy(&router, arn).await;
        assert_eq!(
            (
                fresh.status,
                fresh.text("/Policy"),
                fresh.content_type.clone(),
            ),
            (200, EMPTY_POLICY_DOCUMENT.to_string(), JSON.to_string()),
            "case {arn:?}: a resource with no policy attached must report the empty policy \
             document with 200 application/json, got {} {:?} with {:?}",
            fresh.status,
            fresh.content_type,
            fresh.body
        );

        for (case, policy) in &policies {
            let put = put_resource_policy(&router, arn, policy).await;
            assert_eq!(
                (put.status, put.content_type.as_str()),
                (200, JSON),
                "case {case:?} on {arn:?}: PutResourcePolicy must answer 200 \
                 application/json, got {} {:?} with {:?}",
                put.status,
                put.content_type,
                put.body
            );

            let read = get_resource_policy(&router, arn).await;
            assert_eq!(
                read.text("/Policy"),
                *policy,
                "case {case:?} on {arn:?}: GetResourcePolicy must report the document that \
                 was attached, got {:?}",
                read.body
            );
        }

        // Deleting answers an EMPTY JSON object: `DeleteResourcePolicyResponse` has no
        // members.
        let deleted = delete_resource_policy(&router, arn).await;
        assert_eq!(
            (deleted.status, deleted.content_type.as_str(), &deleted.body),
            (200, JSON, &json!({})),
            "case {arn:?}: DeleteResourcePolicy must answer 200 application/json with an \
             empty object, got {} {:?} with {:?}",
            deleted.status,
            deleted.content_type,
            deleted.body
        );

        let after_delete = get_resource_policy(&router, arn).await;
        assert_eq!(
            (after_delete.status, after_delete.text("/Policy")),
            (200, EMPTY_POLICY_DOCUMENT.to_string()),
            "case {arn:?}: a resource whose policy was deleted must read back as one with \
             no policy attached, got {} with {:?}",
            after_delete.status,
            after_delete.body
        );

        // Deleting again asks for the same end state, which the resource is already in.
        let deleted_again = delete_resource_policy(&router, arn).await;
        assert_eq!(
            deleted_again.status, 200,
            "case {arn:?}: deleting an absent policy must answer 200, got {} with {:?}",
            deleted_again.status, deleted_again.body
        );
    }

    // The change set that created that entity is a resource of the catalog's other kind,
    // which no resource-policy operation is documented for: each refuses it as naming no
    // entity resource, and nothing is stored under it (see
    // `resource_policy_entity_only.rs` for the whole table).
    let change_set_id = change_set_arn
        .rsplit('/')
        .next()
        .unwrap_or(&change_set_arn)
        .to_string();
    for (operation, refused) in [
        (
            "PutResourcePolicy",
            put_resource_policy(&router, &change_set_arn, MINIMAL_POLICY_DOCUMENT).await,
        ),
        (
            "GetResourcePolicy",
            get_resource_policy(&router, &change_set_arn).await,
        ),
        (
            "DeleteResourcePolicy",
            delete_resource_policy(&router, &change_set_arn).await,
        ),
    ] {
        assert_eq!(
            (
                refused.status,
                refused.error_code().to_string(),
                refused.message(),
            ),
            (
                404,
                "ResourceNotFoundException".to_string(),
                format!("Requested entity '{change_set_id}' does not exist."),
            ),
            "case {operation} on {change_set_arn:?}: a change set names no entity resource \
             and must be refused 404 ResourceNotFoundException naming the id asked for, got \
             {} {:?} with {:?}",
            refused.status,
            refused.error_code(),
            refused.body
        );
    }

    // A policy that is not a JSON document is refused with the modelled client error
    // rather than stored: `Policy` is "the policy document to set; formatted in JSON".
    for not_json in ["not a policy", "{", "[1,2", "\"unterminated"] {
        let refused = put_resource_policy(&router, &entity_arn, not_json).await;
        assert_eq!(
            (
                refused.status,
                refused.error_code(),
                refused.content_type.as_str(),
                refused.errortype.as_str(),
            ),
            (422, "ValidationException", JSON, "ValidationException"),
            "case {not_json:?}: a policy that is not JSON must be refused 422 \
             ValidationException as application/json, got {} {:?} with {:?}",
            refused.status,
            refused.error_code(),
            refused.body
        );
    }
}

/// Attaching a policy binds to the catalog's own record: `PutResourcePolicy` refuses an
/// ARN of this catalog naming a resource the store holds no record of with the modelled
/// `ResourceNotFoundException`, naming the resource that was asked for — while reporting
/// and clearing a policy, which are addressed by the ARN alone, answer the resource's
/// (absent) policy instead.
///
/// The rows differ along the entity type and id named by an ARN the store holds no record
/// for.
#[tokio::test]
async fn attaching_a_policy_refuses_a_resource_the_catalog_does_not_hold() {
    // (case, entity type, entity id) — ids of `ResourceId`'s `^[\w\-]+$` that no change
    // set in this emulator ever created.
    let rows: [(&str, &str, &str); 3] = [
        ("an AmiProduct id", "AmiProduct", "prod-b2c3d4e5f6g7h"),
        ("a SaaSProduct id", "SaaSProduct", "prod-c3d4e5f6g7h8i"),
        ("an id of another entity type", "Offer", "offer-1a2b3c4d5e6f7"),
    ];

    let router = emulator();
    for (case, entity_type, entity_id) in rows {
        let arn =
            format!("arn:aws:aws-marketplace:{REGION}:{ACCOUNT}:{CATALOG}/{entity_type}/{entity_id}");

        let attached = put_resource_policy(&router, &arn, MINIMAL_POLICY_DOCUMENT).await;
        assert_eq!(
            (
                attached.status,
                attached.error_code(),
                attached.content_type.as_str(),
                attached.errortype.as_str(),
                attached.message(),
            ),
            (
                404,
                "ResourceNotFoundException",
                JSON,
                "ResourceNotFoundException",
                format!("Requested entity '{entity_id}' does not exist."),
            ),
            "case {case:?}: PutResourcePolicy must refuse an entity the catalog does not \
             hold with 404 ResourceNotFoundException naming it, got {} {:?} with {:?}",
            attached.status,
            attached.error_code(),
            attached.body
        );

        let read = get_resource_policy(&router, &arn).await;
        assert_eq!(
            (read.status, read.text("/Policy")),
            (200, EMPTY_POLICY_DOCUMENT.to_string()),
            "case {case:?}: GetResourcePolicy must report the empty policy document for an \
             ARN nothing is attached to, got {} with {:?}",
            read.status,
            read.body
        );

        let cleared = delete_resource_policy(&router, &arn).await;
        assert_eq!(
            (cleared.status, cleared.content_type.as_str(), &cleared.body),
            (200, JSON, &json!({})),
            "case {case:?}: DeleteResourcePolicy must answer an empty 200 for an ARN \
             nothing is attached to, got {} {:?} with {:?}",
            cleared.status,
            cleared.content_type,
            cleared.body
        );
    }
}

/// Every operation addressed by a `ResourceArn` classifies a refusal the same way, in
/// this order: a value outside the modelled `ResourceARN` syntax is a validation failure
/// naming the value and that syntax; a well-formed ARN naming no resource of this catalog
/// in this account is `AccessDeniedException`, because authorization is evaluated on the
/// ARN before anything is looked up; and an ARN of this catalog's own form naming a
/// resource the store holds no record for is `ResourceNotFoundException` naming that
/// resource — from every operation whose answer IS that record: the tagging surface's read
/// (`ListTagsForResource`) as much as its writes, and `PutResourcePolicy`. Reporting and
/// clearing a resource POLICY are addressed by the ARN alone, so that last step does not
/// apply to them and they answer the resource's (absent) policy instead.
///
/// The rows differ along the operation, its HTTP binding (query-bound or body-bound ARN)
/// and the class of ARN.
#[tokio::test]
async fn a_resource_arn_is_refused_by_syntax_then_authorization_then_existence() {
    /// An entity id of this catalog's form that no change set ever created.
    const ABSENT_ENTITY: &str = "prod-absent0entity0";

    let router = emulator();
    // One real resource, so a case that must NOT be refused is visible beside the ones
    // that must be, carrying a policy and a tag so every operation of the table has
    // something of its own to report for it.
    let (entity_arn, _) = created_resources(&router, "AmiProduct").await;
    let attached = put_resource_policy(&router, &entity_arn, MINIMAL_POLICY_DOCUMENT).await;
    assert_eq!(
        attached.status, 200,
        "attaching a policy to the stored resource must answer 200, got {} with {:?}",
        attached.status, attached.body
    );

    let absent = format!(
        "arn:aws:aws-marketplace:{REGION}:{ACCOUNT}:{CATALOG}/AmiProduct/{ABSENT_ENTITY}"
    );
    let malformed = [
        // The recorded instance: an ARN that stops at the service segment.
        "arn:aws:aws-marketplace",
        // No `arn:` prefix at all.
        "sim2real-behav-1b187cbd960a",
        // An empty account segment, which the pattern requires to be `[0-9]+`.
        "arn:aws:aws-marketplace:us-east-1::AWSMarketplace/AmiProduct/prod-1",
        // Another service's namespace, which the pattern pins to `aws-marketplace`.
        "arn:aws:s3:us-east-1:000000000000:bucket",
    ];
    let unauthorized = [
        // Well-formed, but the resource portion is not one this catalog names.
        format!("arn:aws:aws-marketplace:{REGION}:{ACCOUNT}:sim2real-behav-1b187cbd960a"),
        // A catalog-shaped resource in another account.
        format!("arn:aws:aws-marketplace:{REGION}:210987654321:{CATALOG}/AmiProduct/prod-1"),
        // This account, but another catalog.
        format!("arn:aws:aws-marketplace:{REGION}:{ACCOUNT}:OtherCatalog/AmiProduct/prod-1"),
        // Too few resource segments to name a resource of this catalog.
        format!("arn:aws:aws-marketplace:{REGION}:{ACCOUNT}:{CATALOG}/AmiProduct"),
    ];

    // Every operation the rule covers, not only the operations the recording exercised.
    for operation in [
        "ListTagsForResource",
        "TagResource",
        "UntagResource",
        "PutResourcePolicy",
        "GetResourcePolicy",
        "DeleteResourcePolicy",
    ] {
        let invoke = |arn: String| {
            let router = &router;
            async move {
                match operation {
                    "ListTagsForResource" => list_tags(router, &arn).await,
                    "TagResource" => {
                        tag_resource(router, &arn, json!([{"Key": "env", "Value": "test"}])).await
                    }
                    "UntagResource" => untag_resource(router, &arn, json!(["env"])).await,
                    "GetResourcePolicy" => get_resource_policy(router, &arn).await,
                    "PutResourcePolicy" => {
                        put_resource_policy(router, &arn, MINIMAL_POLICY_DOCUMENT).await
                    }
                    _ => delete_resource_policy(router, &arn).await,
                }
            }
        };

        for arn in malformed {
            let refused = invoke(arn.to_string()).await;
            // `ResourceArn` is judged by the HANDLER (`skip_validation` on each of these
            // inputs), so a value outside the syntax the member publishes carries the
            // modelled `ValidationException` — its code, its 422 and its `Message`.
            assert_eq!(
                (
                    refused.error_code(),
                    refused.content_type.as_str(),
                    refused.errortype.as_str(),
                ),
                ("ValidationException", JSON, "ValidationException"),
                "case {operation} {arn:?}: a malformed resource ARN must be refused \
                 ValidationException as application/json, got {} {:?} with {:?}",
                refused.status,
                refused.error_code(),
                refused.body
            );
            assert_eq!(
                refused.status, 422,
                "case {operation} {arn:?}: a malformed resource ARN must be refused with \
                 the modelled ValidationException's own status, got {}",
                refused.status
            );
            assert_eq!(
                refused.message(),
                format!(
                    "[Requested resource arn '{arn}' is invalid. It should match with \
                     {RESOURCE_ARN_PATTERN}.]"
                ),
                "case {operation} {arn:?}: the refusal must name the rejected ARN and the \
                 modelled pattern, got {:?}",
                refused.message()
            );
        }

        for arn in &unauthorized {
            let refused = invoke(arn.clone()).await;
            assert_eq!(
                (
                    refused.status,
                    refused.error_code(),
                    refused.content_type.as_str(),
                    refused.errortype.as_str(),
                ),
                (403, "AccessDeniedException", JSON, "AccessDeniedException"),
                "case {operation} {arn:?}: an ARN naming no resource of this catalog must \
                 be refused 403 AccessDeniedException as application/json, got {} {:?} \
                 with {:?}",
                refused.status,
                refused.error_code(),
                refused.body
            );
        }

        let answered = invoke(absent.clone()).await;
        // The existence step belongs to the operations whose answer IS the catalog's
        // record of the resource; the policy report and clear are addressed by the ARN
        // alone, so for them an ARN the store holds no record of is served.
        if matches!(operation, "GetResourcePolicy" | "DeleteResourcePolicy") {
            assert_eq!(
                (answered.status, answered.content_type.as_str()),
                (200, JSON),
                "case {operation} {absent:?}: a policy report or clear is addressed by the \
                 ARN alone and must be served 200 application/json, got {} {:?} with {:?}",
                answered.status,
                answered.content_type,
                answered.body
            );
        } else {
            assert_eq!(
                (
                    answered.status,
                    answered.error_code(),
                    answered.content_type.as_str(),
                    answered.errortype.as_str(),
                    answered.message(),
                ),
                (
                    404,
                    "ResourceNotFoundException",
                    JSON,
                    "ResourceNotFoundException",
                    format!("Requested entity '{ABSENT_ENTITY}' does not exist."),
                ),
                "case {operation} {absent:?}: an ARN of this catalog naming a resource the \
                 store does not hold must be refused 404 ResourceNotFoundException naming \
                 it, got {} {:?} with {:?}",
                answered.status,
                answered.error_code(),
                answered.body
            );
        }

        // The same operation against the resource that DOES exist is not refused at all,
        // which is what makes the refusals above about the ARN and not about the
        // operation.
        let served = invoke(entity_arn.clone()).await;
        assert_eq!(
            (served.status, served.content_type.as_str()),
            (200, JSON),
            "case {operation}: the stored resource must be served 200 application/json, \
             got {} {:?} with {:?}",
            served.status,
            served.content_type,
            served.body
        );
    }
}

/// A resource holds at most `TagList`'s maximum number of tags, so a `TagResource` whose
/// merge would pass that bound is refused with the modelled client error and leaves the
/// stored set alone, while a merge that exactly reaches it is stored.
#[tokio::test]
async fn tagging_past_the_modelled_bound_is_refused() {
    let router = emulator();
    let (entity_arn, _) = created_resources(&router, "AmiProduct").await;

    // `TagList` is `@length(min: 1, max: 200)`, so one request can name at most that
    // many: fill the resource in two halves, then push one over.
    let half = MAX_RESOURCE_TAGS / 2;
    for batch in 0..2 {
        let tags: Vec<Value> = (0..half)
            .map(|index| json!({"Key": format!("k{}", batch * half + index), "Value": "v"}))
            .collect();
        let filled = tag_resource(&router, &entity_arn, json!(tags)).await;
        assert_eq!(
            filled.status, 200,
            "batch {batch}: filling to the bound must answer 200, got {} with {:?}",
            filled.status, filled.body
        );
    }
    let at_bound = list_tags(&router, &entity_arn).await;
    assert_eq!(
        at_bound.tag_pairs().len(),
        MAX_RESOURCE_TAGS,
        "a resource must hold the whole bound, got {} tags",
        at_bound.tag_pairs().len()
    );

    let over = tag_resource(
        &router,
        &entity_arn,
        json!([{"Key": "one-too-many", "Value": "v"}]),
    )
    .await;
    assert_eq!(
        (over.status, over.error_code()),
        (422, "ValidationException"),
        "one tag past the bound must be refused 422 ValidationException, got {} {:?} with {:?}",
        over.status,
        over.error_code(),
        over.body
    );
    let unchanged = list_tags(&router, &entity_arn).await;
    assert_eq!(
        unchanged.tag_pairs(),
        at_bound.tag_pairs(),
        "a refused TagResource must leave the stored tags alone, got {} tags",
        unchanged.tag_pairs().len()
    );
}
