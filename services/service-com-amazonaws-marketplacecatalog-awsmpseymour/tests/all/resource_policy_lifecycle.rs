// SPDX-License-Identifier: CC0-1.0

//! The resource-policy lifecycle of the AWS Marketplace Catalog, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the boundary validator, the handler, the persisted state and
//! serialization — the same path a client takes.
//!
//! The rule under test: a resource-based policy is recorded per ENTITY resource ARN — the
//! resource kind every one of these operations is documented for — so reporting one and
//! clearing one are refused for an ARN that identifies no entity resource of this catalog
//! (a resource type the catalog does not have, its other resource KIND, or an id it holds a
//! record of under another type) and never for the mere absence of a stored policy or of
//! this catalog's own record of the resource. A report of a resource carrying no policy
//! answers 200 with the EMPTY policy document (a JSON object with no statements, which is
//! what no attached policy grants and what `PutResourcePolicy` refuses to store), and a
//! clear answers 200 with an empty body whether or not a document was attached.
//!
//! The rows differ along the resource kind the ARN names (entity or change set), the entity
//! type, whether this catalog holds a record of the resource, the id's form, the document
//! attached (shape and size), the partition and region the ARN is written with, and the
//! lifecycle point the call happens at (never attached, attached, cleared, re-attached).

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::state::{
    CHANGE_SET_APPLYING, CHANGE_SET_PREPARING, ENTITY_TYPES,
};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The catalog every operation's `Catalog` member documents as its fixed value.
const CATALOG: &str = "AWSMarketplace";

/// A SigV4 credential scope naming this service's signing name (`aws-marketplace`).
const SIGNED_FOR_US_EAST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The region [`SIGNED_FOR_US_EAST_1`] signs for, and so the region of the ARNs this
/// service mints for the resources these tests create.
const REGION: &str = "us-east-1";

/// The account every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The restJson1 document media type: `application/json`, for answers and refusals alike.
const JSON: &str = "application/json";

/// The resource type segment a change set's ARN carries, as this service mints it.
const CHANGE_SET_RESOURCE_TYPE: &str = "ChangeSet";

/// The document a resource carrying no resource-based policy reports: the empty JSON
/// policy document, which states no `Statement` and therefore grants nothing — exactly
/// what a resource with no policy attached grants. It is inside `ResourcePolicyJson`'s
/// `@length(min: 1, max: 10240)`, and `PutResourcePolicy` refuses it
/// (`a_statement_less_document_can_never_become_a_stored_policy`), so reading it back can
/// only mean "nothing is attached".
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
        .header("host", "catalog.marketplace.us-east-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_US_EAST_1);
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

async fn describe_entity(router: &MultiServiceRouter, entity_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/DescribeEntity?catalog={CATALOG}&entityId={entity_id}"),
        None,
    )
    .await
}

/// A resource ARN of this catalog's form, for the `resource_type`/`id` pair given.
fn catalog_arn(resource_type: &str, id: &str) -> String {
    format!("arn:aws:aws-marketplace:{REGION}:{ACCOUNT}:{CATALOG}/{resource_type}/{id}")
}

/// The same resource, named by an ARN written for another partition and region: the
/// partition and region say WHERE a resource lives, not what it is, so both ARNs address
/// the one resource.
fn rewritten_for_another_region(arn: &str) -> String {
    let resource = arn.splitn(6, ':').nth(5).unwrap_or_default();
    format!("arn:aws-cn:aws-marketplace:cn-north-1:{ACCOUNT}:{resource}")
}

/// The same ARN with its resource TYPE segment replaced, which is what names a different
/// resource — or no resource of this catalog at all.
fn with_resource_type(arn: &str, resource_type: &str) -> String {
    let id = arn.rsplit('/').next().unwrap_or_default();
    catalog_arn(resource_type, id)
}

/// The id portion of a resource ARN: the last `/`-separated segment, which is what a
/// refusal naming the resource names.
fn resource_id_of(arn: &str) -> String {
    arn.rsplit('/').next().unwrap_or(arn).to_string()
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

/// The two resources one `CreateProduct` change set makes: the entity's ARN and the change
/// set's own, each read back out of the service through its own describe, so a case
/// addresses a resource this catalog really holds under the ARN the service minted for it.
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

    let entity = describe_entity(router, &entity_id).await;
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

/// Reporting and clearing the policy of an ENTITY this catalog holds no record of are
/// served, not refused: the report answers the empty policy document, the clear answers an
/// empty 200, and neither call materialises the resource or a policy for it.
///
/// The rows span every entity type this catalog has ([`ENTITY_TYPES`], the source of the
/// per-type ARNs) crossed with four forms of `ResourceId`, and each row is also addressed
/// through an ARN written for another partition and region.
#[tokio::test]
async fn a_policy_report_of_a_resource_without_one_answers_the_empty_document() {
    // Ids of `ResourceId`'s `^[\w\-]+$` that no change set of this emulator created, each
    // also inside `DescribeEntity`'s `Identifier` pattern so the same id can be asked for
    // there: the shortest such form, the product form, a long hex form and a mixed-case
    // one.
    let ids = ["ab1", "prod-1", "prod-0a1b2c3d4e5f6a7b8c9d", "Ent-9x"];

    let router = emulator();
    let mut rows: Vec<(String, String)> = Vec::new();
    for entity_type in ENTITY_TYPES {
        for id in ids {
            rows.push((
                format!("{entity_type} {id}"),
                catalog_arn(entity_type, id),
            ));
        }
    }

    for (case, arn) in rows {
        for addressed in [arn.clone(), rewritten_for_another_region(&arn)] {
            let read = get_resource_policy(&router, &addressed).await;
            assert_eq!(
                (
                    read.status,
                    read.text("/Policy"),
                    read.content_type.clone(),
                ),
                (200, EMPTY_POLICY_DOCUMENT.to_string(), JSON.to_string()),
                "case {case:?} as {addressed:?}: a resource carrying no policy must report \
                 the empty policy document with 200 application/json, got {} {:?} with {:?}",
                read.status,
                read.content_type,
                read.body
            );
        }

        // Reading changed nothing a later caller sees: the next read answers identically,
        // the entity the ARN names still does not exist, and clearing the (absent) policy
        // is the same empty 200 — after which the read is unchanged again.
        let again = get_resource_policy(&router, &arn).await;
        assert_eq!(
            (again.status, again.text("/Policy")),
            (200, EMPTY_POLICY_DOCUMENT.to_string()),
            "case {case:?}: a repeated report must answer identically, got {} with {:?}",
            again.status,
            again.body
        );

        let id = resource_id_of(&arn);
        let described = describe_entity(&router, &id).await;
        assert_eq!(
            (described.status, described.error_code()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: reporting a policy must not make the resource exist, got {} \
             with {:?}",
            described.status,
            described.body
        );

        let cleared = delete_resource_policy(&router, &arn).await;
        assert_eq!(
            (cleared.status, cleared.content_type.as_str(), &cleared.body),
            (200, JSON, &json!({})),
            "case {case:?}: clearing the policy of a resource carrying none must answer \
             200 application/json with an empty object, got {} {:?} with {:?}",
            cleared.status,
            cleared.content_type,
            cleared.body
        );

        let after_clear = get_resource_policy(&router, &arn).await;
        assert_eq!(
            (after_clear.status, after_clear.text("/Policy")),
            (200, EMPTY_POLICY_DOCUMENT.to_string()),
            "case {case:?}: a cleared resource must still report the empty policy \
             document, got {} with {:?}",
            after_clear.status,
            after_clear.body
        );
    }
}

/// The whole lifecycle on an ENTITY this catalog created: the report before any
/// attachment is the empty document, an attached document is reported verbatim through
/// every ARN naming that resource, clearing answers an empty 200 and returns the report to
/// the empty document — and the cycle can run again.
///
/// The rows differ along the entity type and the number of statements in the attached
/// document.
#[tokio::test]
async fn the_policy_lifecycle_of_a_created_resource_round_trips() {
    let router = emulator();

    // Two of the catalog's entity types, one of them an entity type the recorded lifecycle
    // did not name.
    for entity_type in ["SaaSProduct", "ContainerProduct"] {
        let (entity_arn, _) = created_resources(&router, entity_type).await;

        {
            let arn = &entity_arn;
            let case = format!("{entity_type} {arn}");

            let fresh = get_resource_policy(&router, arn).await;
            assert_eq!(
                (fresh.status, fresh.text("/Policy")),
                (200, EMPTY_POLICY_DOCUMENT.to_string()),
                "case {case:?}: a created resource with nothing attached must report the \
                 empty policy document, got {} with {:?}",
                fresh.status,
                fresh.body
            );

            // The rows' documents: one, two, three and many statements.
            for statements in [1usize, 2, 3, 24] {
                let document = policy_document(statements);
                let put = put_resource_policy(&router, arn, &document).await;
                assert_eq!(
                    (put.status, put.content_type.as_str(), &put.body),
                    (200, JSON, &json!({})),
                    "case {case:?} with {statements} statements: PutResourcePolicy must \
                     answer 200 application/json with an empty object, got {} {:?} with {:?}",
                    put.status,
                    put.content_type,
                    put.body
                );

                for addressed in [arn.to_string(), rewritten_for_another_region(arn)] {
                    let read = get_resource_policy(&router, &addressed).await;
                    assert_eq!(
                        (read.status, read.text("/Policy")),
                        (200, document.clone()),
                        "case {case:?} with {statements} statements as {addressed:?}: the \
                         report must be the document that was attached, got {} with {:?}",
                        read.status,
                        read.body
                    );
                }
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
                "case {case:?}: a cleared resource must report the empty policy document \
                 again, got {} with {:?}",
                after_clear.status,
                after_clear.body
            );

            // Clearing again asks for the end state the resource is already in.
            let cleared_again = delete_resource_policy(&router, arn).await;
            assert_eq!(
                (cleared_again.status, &cleared_again.body),
                (200, &json!({})),
                "case {case:?}: clearing an already absent policy must answer the same \
                 empty 200, got {} with {:?}",
                cleared_again.status,
                cleared_again.body
            );

            // The lifecycle is a cycle, not a one-way door.
            let document = policy_document(3);
            let reattached = put_resource_policy(&router, arn, &document).await;
            assert_eq!(
                reattached.status, 200,
                "case {case:?}: re-attaching after a clear must answer 200, got {} with {:?}",
                reattached.status, reattached.body
            );
            let read = get_resource_policy(&router, arn).await;
            assert_eq!(
                (read.status, read.text("/Policy")),
                (200, document),
                "case {case:?}: a re-attached document must be what the report answers, \
                 got {} with {:?}",
                read.status,
                read.body
            );
        }
    }
}

/// An ARN that identifies no ENTITY resource of this catalog is refused 404
/// `ResourceNotFoundException` naming the id it asked for, by the report and the clear
/// alike: a resource type this catalog does not have, a resource of its other KIND (a
/// change set, whatever its id names), and an id this catalog holds a record of under
/// ANOTHER type.
///
/// The rows differ along why the ARN names no entity resource, and along the type segment's
/// form (a type of another service, a plural, a different case, a near-miss of a type this
/// catalog has, the change-set type).
#[tokio::test]
async fn an_arn_naming_no_resource_of_this_catalog_is_refused() {
    let router = emulator();
    // A resource the catalog DOES hold, so each refusal below is shown to be about the
    // ARN rather than about the operation — and is shown to leave its policy alone.
    let (held_arn, held_change_set_arn) = created_resources(&router, "AmiProduct").await;
    let held_document = policy_document(2);
    let attached = put_resource_policy(&router, &held_arn, &held_document).await;
    assert_eq!(
        attached.status, 200,
        "attaching to the held resource must answer 200, got {} with {:?}",
        attached.status, attached.body
    );

    // (case, an ARN of this catalog's form that names no resource of it). The type
    // segments are letters-only words `EntityType`'s `@pattern` admits that name none of
    // the catalog's own types ([`ENTITY_TYPES`] plus the change-set type).
    let mut rows: Vec<(String, String)> = vec![
        (
            "a type of another service".to_string(),
            catalog_arn("Bucket", "prod-1"),
        ),
        (
            "a plural of a type this catalog has".to_string(),
            catalog_arn("SaaSProducts", "prod-2"),
        ),
        (
            "a type differing only in case".to_string(),
            catalog_arn("amiproduct", "prod-3"),
        ),
        (
            "a near-miss of a type this catalog has".to_string(),
            catalog_arn("ContainerProductX", "prod-4"),
        ),
        (
            "a plural of the change set type".to_string(),
            catalog_arn("ChangeSets", "cs-000000000001"),
        ),
        // The two cross-KIND contradictions: the held entity's id addressed as a change
        // set, and the held change set's id addressed as an entity.
        (
            "the held entity id under the change set type".to_string(),
            with_resource_type(&held_arn, CHANGE_SET_RESOURCE_TYPE),
        ),
        (
            "the held change set id under an entity type".to_string(),
            with_resource_type(&held_change_set_arn, "AmiProduct"),
        ),
        // And the held change set under its OWN type: a change set is a resource of this
        // catalog, but not one of the kind these operations act on.
        (
            "the held change set itself".to_string(),
            held_change_set_arn.clone(),
        ),
    ];
    // The held entity's own id, addressed under every OTHER entity type: this catalog's
    // record of that id contradicts the ARN, so the ARN names no resource of it.
    for entity_type in ENTITY_TYPES {
        let other = with_resource_type(&held_arn, entity_type);
        if other != held_arn {
            rows.push((
                format!("the held id under the type {entity_type}"),
                other,
            ));
        }
    }

    for (case, arn) in rows {
        let id = resource_id_of(&arn);
        // Whatever kind of resource the ARN names, what a resource-policy operation asked
        // for and did not find is an ENTITY — the kind each of them is documented for.
        let missing = format!("Requested entity '{id}' does not exist.");

        for operation in ["GetResourcePolicy", "DeleteResourcePolicy"] {
            let refused = match operation {
                "GetResourcePolicy" => get_resource_policy(&router, &arn).await,
                _ => delete_resource_policy(&router, &arn).await,
            };
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
                "case {case:?}/{operation} on {arn:?}: an ARN naming no resource of this \
                 catalog must be refused 404 ResourceNotFoundException naming it, got {} \
                 {:?} with {:?}",
                refused.status,
                refused.error_code(),
                refused.body
            );
        }

        // Nothing the refused calls touched: the held resource still carries exactly the
        // document that was attached to it.
        let held = get_resource_policy(&router, &held_arn).await;
        assert_eq!(
            (held.status, held.text("/Policy")),
            (200, held_document.clone()),
            "case {case:?}: a refused call on another ARN must leave the held resource's \
             policy alone, got {} with {:?}",
            held.status,
            held.body
        );
    }
}

/// The empty policy document is what a resource carrying no policy reports, and it is a
/// document `PutResourcePolicy` refuses — so a report answering it can only mean that
/// nothing is attached, never that a caller stored it.
#[tokio::test]
async fn a_statement_less_document_can_never_become_a_stored_policy() {
    let router = emulator();
    let (entity_arn, _) = created_resources(&router, "DataProduct").await;

    // Documents stating no statement at all: the empty object a report answers, and the
    // two other ways of writing a document with no statements.
    for document in [
        EMPTY_POLICY_DOCUMENT,
        "{\"Statement\":[]}",
        "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
    ] {
        let refused = put_resource_policy(&router, &entity_arn, document).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (422, "ValidationException"),
            "case {document:?}: a document stating no statement must be refused 422 \
             ValidationException, got {} with {:?}",
            refused.status,
            refused.body
        );

        let read = get_resource_policy(&router, &entity_arn).await;
        assert_eq!(
            (read.status, read.text("/Policy")),
            (200, EMPTY_POLICY_DOCUMENT.to_string()),
            "case {document:?}: a refused document must not become the resource's policy, \
             got {} with {:?}",
            read.status,
            read.body
        );
    }
}
