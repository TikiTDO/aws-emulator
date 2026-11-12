// SPDX-License-Identifier: CC0-1.0

//! Reading a resource-based policy of the AWS Marketplace Catalog, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the boundary validator, the handler, the persisted state and
//! serialization — the same path a client takes.
//!
//! The rule under test: a resource-based policy is recorded per ENTITY resource ARN — the
//! kind every resource-policy operation is documented for — so a report of one answers the
//! document attached to that ARN, or the EMPTY policy document when none is attached, and
//! is refused with the modelled `ResourceNotFoundException` (HTTP 404) for an ARN that
//! identifies no entity resource of this catalog (including one naming its other resource
//! kind, a change set — see `resource_policy_entity_only.rs`). The catalog's own record of a
//! resource gates ATTACHING a policy, not reporting or clearing one, and nothing a read
//! answers makes a later read answer differently.
//!
//! The rows differ along the entity type, the policy document attached (shape and size),
//! the region and partition the ARN naming the resource is written with, and the lifecycle
//! point the read happens at (never attached, attached, cleared).

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

/// The document a resource carrying no resource-based policy reports: the empty JSON
/// policy document, which states no `Statement` and so grants what no attached policy
/// grants. `PutResourcePolicy` refuses it, so reading it back can only mean that nothing
/// is attached (see `resource_policy_lifecycle.rs`).
const EMPTY_POLICY_DOCUMENT: &str = "{}";

/// The product entity types `CreateProduct` creates, from `EntitySummary`'s per-type
/// summary members (`smithy_query(query="shape", shape="EntitySummary")`:
/// `AmiProductSummary`, `ContainerProductSummary`, `DataProductSummary`,
/// `MachineLearningProductSummary`, `SaaSProductSummary`), each with the suffix dropped.
const PRODUCT_ENTITY_TYPES: &[&str] = &[
    "AmiProduct",
    "ContainerProduct",
    "DataProduct",
    "MachineLearningProduct",
    "SaaSProduct",
];

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

/// The same resource, named by an ARN written for another partition and region: the
/// partition and region say WHERE a resource lives, not what it is, so both ARNs address
/// the one record.
fn rewritten_for_another_region(arn: &str) -> String {
    let resource = arn.splitn(6, ':').nth(5).unwrap_or_default();
    format!("arn:aws-cn:aws-marketplace:cn-north-1:{ACCOUNT}:{resource}")
}

/// An ARN of this catalog's form naming a resource no change set of this emulator created.
fn absent_arn(resource_type: &str, id: &str) -> String {
    format!("arn:aws:aws-marketplace:{REGION}:{ACCOUNT}:{CATALOG}/{resource_type}/{id}")
}

/// A policy document of `statements` statements, inside `ResourcePolicyJson`'s
/// `@length(min: 1, max: 10240)`. Every statement states its effect, its action and the
/// principal it is granted to, which is what makes the document a RESOURCE-based policy.
fn policy_document(statements: usize) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": (0..statements)
            .map(|index| json!({
                "Sid": format!("s{index}"),
                "Effect": if index % 2 == 0 { "Allow" } else { "Deny" },
                "Principal": {"AWS": format!("arn:aws:iam::{ACCOUNT}:root")},
                "Action": ["aws-marketplace:DescribeEntity"],
            }))
            .collect::<Vec<Value>>(),
    })
    .to_string()
}

/// A policy read reports the document that was attached and nothing else: before any
/// attachment, and after the attachment is cleared, the resource carries no document and
/// the read answers the empty policy document instead of one no caller attached.
///
/// The rows differ along the entity type, the number of statements in the attached
/// document, and the partition and region the ARN addressing the resource is written with.
#[tokio::test]
async fn a_policy_read_reports_only_what_was_attached() {
    let router = emulator();

    for (index, entity_type) in PRODUCT_ENTITY_TYPES.iter().enumerate() {
        let (entity_arn, _) = created_resources(&router, entity_type).await;

        {
            let arn = &entity_arn;
            let case = format!("{entity_type} {arn}");
            let id = resource_id_of(arn);

            let fresh = get_resource_policy(&router, arn).await;
            assert_eq!(
                (
                    fresh.status,
                    fresh.text("/Policy"),
                    fresh.content_type.clone(),
                ),
                (200, EMPTY_POLICY_DOCUMENT.to_string(), JSON.to_string()),
                "case {case:?}: a resource carrying no policy must report the empty policy \
                 document with 200 application/json, got {} {:?} with {:?}",
                fresh.status,
                fresh.content_type,
                fresh.body
            );

            // The rows' documents: one, two, three and many statements.
            for statements in [1usize, 2, 3, 24] {
                let document = policy_document(statements);
                let put = put_resource_policy(&router, arn, &document).await;
                assert_eq!(
                    (put.status, put.content_type.as_str()),
                    (200, JSON),
                    "case {case:?} with {statements} statements: PutResourcePolicy must \
                     answer 200 application/json, got {} {:?} with {:?}",
                    put.status,
                    put.content_type,
                    put.body
                );

                let read = get_resource_policy(&router, arn).await;
                assert_eq!(
                    (read.status, read.text("/Policy")),
                    (200, document.clone()),
                    "case {case:?} with {statements} statements: the read must report the \
                     document that was attached, got {} with {:?}",
                    read.status,
                    read.body
                );

                // The one record, addressed through an ARN written for another partition
                // and region: the resource is the same resource.
                let elsewhere = rewritten_for_another_region(arn);
                let read_elsewhere = get_resource_policy(&router, &elsewhere).await;
                assert_eq!(
                    (read_elsewhere.status, read_elsewhere.text("/Policy")),
                    (200, document.clone()),
                    "case {case:?} with {statements} statements: an ARN of another \
                     partition and region naming the same resource must report the same \
                     document, got {} with {:?}",
                    read_elsewhere.status,
                    read_elsewhere.body
                );
            }

            let cleared = delete_resource_policy(&router, arn).await;
            assert_eq!(
                (cleared.status, cleared.content_type.as_str(), &cleared.body),
                (200, JSON, &json!({})),
                "case {case:?}: DeleteResourcePolicy must answer 200 application/json with \
                 an empty object, got {} {:?} with {:?}",
                cleared.status,
                cleared.content_type,
                cleared.body
            );

            let after_clear = get_resource_policy(&router, arn).await;
            assert_eq!(
                (after_clear.status, after_clear.text("/Policy")),
                (200, EMPTY_POLICY_DOCUMENT.to_string()),
                "case {case:?} ({id}): a resource whose policy was cleared must report the \
                 empty policy document again, got {} with {:?}",
                after_clear.status,
                after_clear.body
            );

            // Clearing again asks for the end state the resource is already in, which is
            // not a refusal: the resource itself is still held.
            let cleared_again = delete_resource_policy(&router, arn).await;
            assert_eq!(
                cleared_again.status, 200,
                "case {case:?}: clearing an already absent policy must answer 200, got {} \
                 with {:?}",
                cleared_again.status, cleared_again.body
            );
        }

        // One row re-attaches after the clear, so the lifecycle is shown to be a cycle
        // rather than a one-way door.
        if index == 0 {
            let document = policy_document(3);
            let reattached = put_resource_policy(&router, &entity_arn, &document).await;
            assert_eq!(
                reattached.status, 200,
                "re-attaching after a clear must answer 200, got {} with {:?}",
                reattached.status, reattached.body
            );
            let read = get_resource_policy(&router, &entity_arn).await;
            assert_eq!(
                (read.status, read.text("/Policy")),
                (200, document),
                "a re-attached document must be reported, got {} with {:?}",
                read.status,
                read.body
            );
        }
    }
}

/// The catalog's own record of a resource gates ATTACHING a policy and nothing else: for
/// an ARN of this catalog naming an ENTITY the store holds no record of,
/// `PutResourcePolicy` is refused 404 `ResourceNotFoundException` naming that resource,
/// while reporting answers the empty policy document and clearing answers the same empty
/// 200 it answers for a resource that carries none. An ARN naming the catalog's other
/// resource KIND is a different matter: a change set is no entity resource, so all three
/// operations refuse it.
///
/// The rows differ along the resource kind and type the ARN names, and the id.
#[tokio::test]
async fn the_catalogs_record_gates_attaching_a_policy_and_not_reporting_one() {
    // (case, resource type segment, id, the sentence the refusal states, whether the report
    // and the clear are served for it) — ids of `ResourceId`'s `^[\w\-]+$` that nothing ever
    // created. Every sentence names an ENTITY: that is the resource kind these operations
    // are documented for, so it is what such a call asked for and did not find.
    let rows: Vec<(&str, &str, &str, String, bool)> = vec![
        (
            "an AmiProduct entity",
            "AmiProduct",
            "prod-b2c3d4e5f6g7h",
            "Requested entity 'prod-b2c3d4e5f6g7h' does not exist.".to_string(),
            true,
        ),
        (
            "a SaaSProduct entity",
            "SaaSProduct",
            "prod-c3d4e5f6g7h8i",
            "Requested entity 'prod-c3d4e5f6g7h8i' does not exist.".to_string(),
            true,
        ),
        (
            "an entity of a type nothing created",
            "Offer",
            "offer-1a2b3c4d5e6f7",
            "Requested entity 'offer-1a2b3c4d5e6f7' does not exist.".to_string(),
            true,
        ),
        (
            "a change set",
            CHANGE_SET_RESOURCE_TYPE,
            "cs-000000000009",
            "Requested entity 'cs-000000000009' does not exist.".to_string(),
            false,
        ),
    ];

    let router = emulator();
    // A resource that IS held, carrying a policy, so each answer below is shown to be
    // about the ARN rather than about the operation — and is shown to leave that policy
    // alone.
    let (held_arn, _) = created_resources(&router, "AmiProduct").await;
    let held_document = policy_document(2);
    let attached = put_resource_policy(&router, &held_arn, &held_document).await;
    assert_eq!(
        attached.status, 200,
        "attaching to the held resource must answer 200, got {} with {:?}",
        attached.status, attached.body
    );

    for (case, resource_type, id, missing, reads_served) in rows {
        let arn = absent_arn(resource_type, id);

        let refused = put_resource_policy(&router, &arn, &policy_document(1)).await;
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
            "case {case:?}: attaching a policy to a resource the catalog holds no record \
             of must be refused 404 ResourceNotFoundException naming it, got {} {:?} with \
             {:?}",
            refused.status,
            refused.error_code(),
            refused.body
        );

        // For an entity, reporting and clearing are not gated on that record: nothing is
        // attached, which the report says with the empty policy document and the clear with
        // its empty 200. For an ARN naming no entity resource at all they are refused like
        // the attach.
        for round in 0..2 {
            let read = get_resource_policy(&router, &arn).await;
            let cleared = delete_resource_policy(&router, &arn).await;
            if reads_served {
                assert_eq!(
                    (read.status, read.text("/Policy")),
                    (200, EMPTY_POLICY_DOCUMENT.to_string()),
                    "case {case:?} round {round}: the report must answer the empty policy \
                     document, got {} with {:?}",
                    read.status,
                    read.body
                );
                assert_eq!(
                    (cleared.status, &cleared.body),
                    (200, &json!({})),
                    "case {case:?} round {round}: the clear must answer an empty 200, got {} \
                     with {:?}",
                    cleared.status,
                    cleared.body
                );
            } else {
                for (operation, answer) in [("report", &read), ("clear", &cleared)] {
                    assert_eq!(
                        (
                            answer.status,
                            answer.error_code().to_string(),
                            answer.message()
                        ),
                        (
                            404,
                            "ResourceNotFoundException".to_string(),
                            missing.clone()
                        ),
                        "case {case:?} round {round}: the {operation} of an ARN naming no \
                         entity resource must be refused 404 ResourceNotFoundException \
                         naming the id asked for, got {} {:?} with {:?}",
                        answer.status,
                        answer.error_code(),
                        answer.body
                    );
                }
            }
        }

        // Neither answer made the resource exist, and the held resource's own policy is
        // exactly what was attached to it.
        let described = call(
            &router,
            "GET",
            &format!("/DescribeEntity?catalog={CATALOG}&entityId={id}"),
            None,
        )
        .await;
        assert_eq!(
            (described.status, described.error_code()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a policy call must not make the resource exist, got {} with \
             {:?}",
            described.status,
            described.body
        );
        let held = get_resource_policy(&router, &held_arn).await;
        assert_eq!(
            (held.status, held.text("/Policy")),
            (200, held_document.clone()),
            "case {case:?}: a call on another ARN must leave the held resource's policy \
             alone, got {} with {:?}",
            held.status,
            held.body
        );
    }
}

/// A policy read creates nothing: after `GetResourcePolicy` has reported the empty policy
/// document for an ARN the catalog holds no record of, that ARN still names no resource —
/// `DescribeEntity` and `PutResourcePolicy` answer exactly as they did before the read —
/// and for a held resource carrying no policy, attaching one after the read still works
/// and is what the next read reports.
#[tokio::test]
async fn a_policy_read_changes_nothing_a_later_caller_sees() {
    const ABSENT_ENTITY: &str = "prod-absent0entity0";

    let router = emulator();
    let absent = absent_arn("AmiProduct", ABSENT_ENTITY);

    // Three reads of the resource the catalog holds no record of, with the entity read
    // and the attach between them: each one answers the same, so no read materialised
    // the resource it was asked about or a policy for it.
    for round in 0..3 {
        let read = get_resource_policy(&router, &absent).await;
        assert_eq!(
            (read.status, read.text("/Policy")),
            (200, EMPTY_POLICY_DOCUMENT.to_string()),
            "round {round}: every read of a resource with no policy must answer the same \
             empty policy document, got {} with {:?}",
            read.status,
            read.body
        );

        let described = call(
            &router,
            "GET",
            &format!("/DescribeEntity?catalog={CATALOG}&entityId={ABSENT_ENTITY}"),
            None,
        )
        .await;
        assert_eq!(
            (described.status, described.error_code()),
            (404, "ResourceNotFoundException"),
            "round {round}: a policy read must not make the entity exist, got {} \
             with {:?}",
            described.status,
            described.body
        );

        let attach = put_resource_policy(&router, &absent, &policy_document(1)).await;
        assert_eq!(
            (attach.status, attach.error_code()),
            (404, "ResourceNotFoundException"),
            "round {round}: a policy read must not make the resource one a policy can be \
             attached to, got {} with {:?}",
            attach.status,
            attach.body
        );
    }

    // The held resource: the read before any attachment leaves it able to take a policy,
    // which the next read then reports.
    let (held_arn, _) = created_resources(&router, "SaaSProduct").await;
    let first = get_resource_policy(&router, &held_arn).await;
    assert_eq!(
        (first.status, first.text("/Policy")),
        (200, EMPTY_POLICY_DOCUMENT.to_string()),
        "a held resource carrying no policy must report the empty policy document, got {} \
         with {:?}",
        first.status,
        first.body
    );

    let document = policy_document(1);
    let attached = put_resource_policy(&router, &held_arn, &document).await;
    assert_eq!(
        attached.status, 200,
        "attaching after a read must answer 200, got {} with {:?}",
        attached.status, attached.body
    );
    for round in 0..3 {
        let read = get_resource_policy(&router, &held_arn).await;
        assert_eq!(
            (read.status, read.text("/Policy")),
            (200, document.clone()),
            "round {round}: every read must report the attached document, got {} with {:?}",
            read.status,
            read.body
        );
    }
}
