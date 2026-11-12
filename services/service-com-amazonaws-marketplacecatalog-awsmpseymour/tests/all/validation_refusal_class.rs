// SPDX-License-Identifier: CC0-1.0

//! How this service refuses a member it judges against a rule of its OWN — a published
//! syntax, a published value set, or a modelled range it states in words — end to end
//! over HTTP.
//!
//! THE RULE: such a member, when the value is outside that rule, is refused with the
//! service's own modelled `ValidationException` — its code, its `@httpError(422)` and its
//! bracketed `Message`, which names the kind of member, quotes the submitted value and
//! then states the syntax, the bounds or the value set the service publishes for it. The
//! generic request-boundary refusal (the protocol's 400 with a lowercase `message` and no
//! `Message` member at all) is never that refusal.
//!
//! Every assertion is made over a TABLE spanning the class, not one recorded request: the
//! rows cross the kind of constraint (a range, a published syntax, a published set), the
//! operation (every paginated one, every one addressed by a resource ARN, both lists that
//! sort, the mutating entry point), the HTTP binding of the member (body-bound or
//! query-bound) and the way the value leaves the rule.
//!
//! Each sentence below is written out here rather than taken from the crate, so an
//! implementation that words a refusal its own way cannot make this suite pass.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The catalog every operation's `Catalog` member documents as its fixed value.
const CATALOG: &str = "AWSMarketplace";

/// A SigV4 credential scope naming this service's signing name (`aws-marketplace`),
/// which is what routes a request here.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The region [`SIGNED_FOR_EU_WEST_1`] signs for.
const REGION: &str = "eu-west-1";

/// The account the emulator mints its ARNs in.
const ACCOUNT: &str = "000000000000";

/// The modelled `ResourceARN` `@pattern`
/// (`smithy_query(query="shape", shape="ResourceARN")`), which is the syntax the service
/// quotes back for a resource ARN outside it.
const RESOURCE_ARN_PATTERN: &str =
    r"^arn:[\w+=/,.@-]+:aws-marketplace:[\w+=/,.@-]*:[0-9]+:[\w+=,.@-]+(/[\w+=,.@-]+)*$";

/// The syntax the service publishes for a `Change$Entity$Type`: an entity type at a schema
/// version. The modelled `EntityType` `@pattern` (`^[a-zA-Z]+$`) admits no separator at
/// all, so the model's pattern cannot be what a caller is told to satisfy here.
const ENTITY_TYPE_REFERENCE_PATTERN: &str = "^(.*)@(.*)$";

/// The syntax the service publishes for an `AssessmentIdentifier`: the two forms the
/// member's doc documents, as two separately anchored alternatives joined by `|` with no
/// account field in the ARN branch.
const ASSESSMENT_IDENTIFIER_PATTERN: &str = concat!(
    r"(^assessment-[a-zA-Z0-9]+$)|(^arn:[a-zA-Z0-9-]+:[a-zA-Z0-9-]+:[a-zA-Z0-9-]*::",
    r"[a-zA-Z0-9-]+/Assessment/assessment-[a-zA-Z0-9]+$)"
);

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

    /// The string at `pointer`, or the empty string when the member is absent.
    fn text(&self, pointer: &str) -> String {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
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

/// One row: the case, the request to make, and the exact `Message` the rule requires.
struct Row {
    case: String,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    message: String,
}

/// Assert the whole class on one row: the modelled `ValidationException`, its 422, the
/// sentence the service states, and no trace of the boundary's own refusal.
async fn assert_refused(router: &MultiServiceRouter, row: &Row) {
    let answer = call(router, row.method, &row.uri, row.body.clone()).await;
    let case = &row.case;
    assert_eq!(
        (answer.status, answer.error_code()),
        (422, "ValidationException"),
        "case {case:?}: a member outside the rule the service judges it by must carry the \
         modelled ValidationException and its 422, got {} {:?} with {:?}",
        answer.status,
        answer.error_code(),
        answer.body
    );
    assert_eq!(
        answer.text("/Message"),
        row.message,
        "case {case:?}: expected the service's own sentence, got {:?}",
        answer.body
    );
    // The boundary's aggregate arrives under a lowercase `message` and carries no
    // `Message` at all; a handler-stated refusal is the other way round.
    assert_eq!(
        answer.body.pointer("/message"),
        None,
        "case {case:?}: the refusal must be the service's, not the request boundary's, \
         got {:?}",
        answer.body
    );
    assert_eq!(
        answer.body.pointer("/ValidationExceptionFieldList"),
        None,
        "case {case:?}: this service says everything it has to say in Message, got {:?}",
        answer.body
    );
}

/// A one-change `StartChangeSet` body, with `members` merged over a change the catalog
/// accepts — so a row that malforms ONE member is refused for that member alone.
fn start_change_set_body(members: Value) -> Value {
    let mut change = json!({
        "ChangeType": "CreateProduct",
        "Entity": {"Type": "AmiProduct@1.0"},
        "Details": json!({"ProductTitle": "validation-refusal-class"}).to_string(),
    });
    if let (Value::Object(target), Value::Object(extra)) = (&mut change, &members) {
        for (member, carried) in extra {
            target.insert(member.clone(), carried.clone());
        }
    }
    json!({"Catalog": CATALOG, "ChangeSet": [change]})
}

/// Every paginated operation of this service and the maximum its own `MaxResults` `@range`
/// declares, from the model
/// (`smithy_query(query="operation", operation="ListChangeSets,ListEntities,\
/// ListAssessments,DescribeAssessment")`: 1..=20, 1..=50, 1..=100, 1..=100).
fn paginated() -> Vec<(&'static str, Value, i32)> {
    vec![
        ("ListChangeSets", json!({"Catalog": CATALOG}), 20),
        (
            "ListEntities",
            json!({"Catalog": CATALOG, "EntityType": "AmiProduct"}),
            50,
        ),
        ("ListAssessments", json!({"Catalog": CATALOG}), 100),
        (
            "DescribeAssessment",
            json!({"Catalog": CATALOG, "AssessmentIdentifier": "assessment-0ff0ff0ff"}),
            100,
        ),
    ]
}

/// RULE, for a range: a `MaxResults` outside the range its member declares is refused with
/// the modelled `ValidationException` and the out-of-bounds sentence, which quotes the
/// size asked for and the bounds that member publishes.
///
/// The rows cross every paginated operation of the service — whose declared maxima differ
/// (20, 50, 100 twice) — with the ways a page size leaves a range: both one past a bound,
/// zero, a negative, and the largest a 32-bit member can carry.
#[tokio::test]
async fn a_page_size_outside_its_modelled_range_is_the_services_own_422() {
    let router = emulator();

    for (operation, request, limit) in paginated() {
        for requested in [0, -1, limit + 1, i32::MAX] {
            let mut body = request.clone();
            body["MaxResults"] = json!(requested);
            assert_refused(
                &router,
                &Row {
                    case: format!("{operation} / MaxResults {requested}"),
                    method: "POST",
                    uri: format!("/{operation}"),
                    body: Some(body),
                    message: format!(
                        "[Provided max results value '{requested}' is out of bounds. \
                         It should be between 1 and {limit}.]"
                    ),
                },
            )
            .await;
        }
    }
}

/// A `MaxResults` INSIDE the declared range is not refused — the rule is about the values
/// outside it, so each published bound is served.
#[tokio::test]
async fn a_page_size_at_either_published_bound_is_served() {
    let router = emulator();

    for (operation, request, limit) in paginated() {
        for requested in [1, limit] {
            let mut body = request.clone();
            body["MaxResults"] = json!(requested);
            let answer = call(&router, "POST", &format!("/{operation}"), Some(body)).await;
            let case = format!("{operation} / MaxResults {requested}");
            assert_ne!(
                answer.status, 422,
                "case {case:?}: a page size inside the declared range must not be refused \
                 for its size, got {} with {:?}",
                answer.status, answer.body
            );
            assert!(
                answer.text("/Message").is_empty()
                    || !answer.text("/Message").contains("max results"),
                "case {case:?}: no page-size fault may be stated for a size inside the \
                 range, got {:?}",
                answer.text("/Message")
            );
        }
    }
}

/// RULE, for a published syntax: a resource ARN outside the syntax the member publishes is
/// refused with the modelled `ValidationException` and the sentence quoting the ARN and
/// that syntax — from EVERY operation this service addresses by a resource ARN, before it
/// decides whether the ARN names a resource of this catalog.
///
/// The rows cross all six such operations with the ways an ARN leaves the pattern: a
/// prefix that stops early, no `arn:` at all, an empty account where digits are required,
/// another service's namespace, a colon inside the resource portion, and a trailing empty
/// resource segment.
#[tokio::test]
async fn a_resource_arn_outside_its_published_syntax_is_the_services_own_422() {
    let router = emulator();

    let malformed: Vec<String> = vec![
        "arn:aws:aws-marketplace".to_string(),
        "catalog-entity-0f0f0f0f".to_string(),
        format!("arn:aws:aws-marketplace:{REGION}::{CATALOG}/AmiProduct/prod-1"),
        format!("arn:aws:s3:{REGION}:{ACCOUNT}:bucket/key"),
        format!("arn:aws:aws-marketplace:{REGION}:{ACCOUNT}:{CATALOG}:AmiProduct"),
        format!("arn:aws:aws-marketplace:{REGION}:{ACCOUNT}:{CATALOG}/AmiProduct/"),
    ];

    // (operation, method, uri builder, body builder) — the tagging surface and the
    // resource-policy surface, including the two operations that bind the ARN to the
    // query string.
    for arn in &malformed {
        let message = format!(
            "[Requested resource arn '{arn}' is invalid. It should match with \
             {RESOURCE_ARN_PATTERN}.]"
        );
        let rows = vec![
            Row {
                case: format!("ListTagsForResource / {arn}"),
                method: "POST",
                uri: "/ListTagsForResource".to_string(),
                body: Some(json!({"ResourceArn": arn})),
                message: message.clone(),
            },
            Row {
                case: format!("TagResource / {arn}"),
                method: "POST",
                uri: "/TagResource".to_string(),
                body: Some(json!({
                    "ResourceArn": arn,
                    "Tags": [{"Key": "env", "Value": "test"}],
                })),
                message: message.clone(),
            },
            Row {
                case: format!("UntagResource / {arn}"),
                method: "POST",
                uri: "/UntagResource".to_string(),
                body: Some(json!({"ResourceArn": arn, "TagKeys": ["env"]})),
                message: message.clone(),
            },
            Row {
                case: format!("GetResourcePolicy / {arn}"),
                method: "GET",
                uri: format!("/GetResourcePolicy?resourceArn={arn}"),
                body: None,
                message: message.clone(),
            },
            Row {
                case: format!("PutResourcePolicy / {arn}"),
                method: "POST",
                uri: "/PutResourcePolicy".to_string(),
                body: Some(json!({
                    "ResourceArn": arn,
                    "Policy": "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\
                        \"Allow\",\"Principal\":\"*\",\"Action\":\
                        \"aws-marketplace:DescribeEntity\"}]}",
                })),
                message: message.clone(),
            },
            Row {
                case: format!("DeleteResourcePolicy / {arn}"),
                method: "DELETE",
                uri: format!("/DeleteResourcePolicy?resourceArn={arn}"),
                body: None,
                message: message.clone(),
            },
        ];
        for row in &rows {
            assert_refused(&router, row).await;
        }
    }
}

/// RULE, for a published FORM: a `Change$Entity$Type` that is not written the way the
/// service publishes the member is refused with the sentence quoting that form, while one
/// of that form naming no type this catalog has is refused with the set-based sentence
/// instead — two different faults, as the two checks are different.
///
/// The rows cross the kinds of value: bare type names this catalog does have (so only the
/// missing version is at fault), a bare word that is no type either, and — on the other
/// side — values that carry the separator but name no type or no version.
#[tokio::test]
async fn an_entity_type_without_its_schema_version_is_the_services_own_422() {
    let router = emulator();

    for entity_type in ["AmiProduct", "SaaSProduct", "OfferSet", "Frobnicator"] {
        assert_refused(
            &router,
            &Row {
                case: format!("StartChangeSet / Entity.Type {entity_type}"),
                method: "POST",
                uri: "/StartChangeSet".to_string(),
                body: Some(start_change_set_body(
                    json!({"Entity": {"Type": entity_type}}),
                )),
                message: format!(
                    "[Requested entity type '{entity_type}' is invalid. It should match \
                     with {ENTITY_TYPE_REFERENCE_PATTERN}.]"
                ),
            },
        )
        .await;
    }

    // Of the published form, but naming no entity type of this catalog (or no version):
    // the set is what it failed, and no syntax is quoted for a set.
    for reference in ["Frobnicator@1.0", "amiproduct@1.0", "AmiProduct@", "@1.0"] {
        assert_refused(
            &router,
            &Row {
                case: format!("StartChangeSet / Entity.Type {reference}"),
                method: "POST",
                uri: "/StartChangeSet".to_string(),
                body: Some(start_change_set_body(json!({"Entity": {"Type": reference}}))),
                message: format!("[Requested entity type '{reference}' is invalid.]"),
            },
        )
        .await;
    }
}

/// RULE, for a published SET: a `Change$ChangeType` naming no change this catalog performs
/// is refused with the modelled `ValidationException` and the set-based sentence — whether
/// the value misses the set only, or also the characters the modelled `@pattern` admits,
/// because the set is what the member is judged by.
#[tokio::test]
async fn a_change_type_outside_the_catalogs_set_is_the_services_own_422() {
    let router = emulator();

    for change_type in [
        // Outside the modelled `ChangeType` pattern (`^[A-Z][\w]*$`) as well as the set.
        "updateInformation",
        "create product",
        "Create-Product",
        // Inside that pattern, outside the set.
        "Nonsense",
        "CreateProductV2",
        "Z",
    ] {
        assert_refused(
            &router,
            &Row {
                case: format!("StartChangeSet / ChangeType {change_type}"),
                method: "POST",
                uri: "/StartChangeSet".to_string(),
                body: Some(start_change_set_body(json!({"ChangeType": change_type}))),
                message: format!("[Requested change type '{change_type}' is invalid.]"),
            },
        )
        .await;
    }
}

/// RULE, for a set the MODEL does not publish: a `Sort$SortBy` the list does not order by
/// is refused with the modelled `ValidationException` and the sentence quoting the
/// attribute asked for, whose allowed-value slot renders the empty list — `Sort$SortBy`
/// targets a plain string with no enum constraint, so the service has no set to name.
///
/// The rows cross both lists that sort with the ways an attribute misses: an invented
/// name, the other list's own attribute, a case variant of a supported one, and a name
/// outside the characters the modelled `@pattern` admits.
#[tokio::test]
async fn a_sort_attribute_a_list_does_not_order_by_is_the_services_own_422() {
    let router = emulator();

    let lists = [
        (
            "ListEntities",
            json!({"Catalog": CATALOG, "EntityType": "AmiProduct"}),
            // `StartTime` is `ListChangeSets`' attribute, not this list's.
            ["NotARealSortField", "StartTime", "entityid", "Not1Real"],
        ),
        (
            "ListChangeSets",
            json!({"Catalog": CATALOG}),
            // `EntityId` is `ListEntities`' attribute, not this list's.
            ["NotARealSortField", "EntityId", "starttime", "Not1Real"],
        ),
    ];

    for (operation, request, attributes) in lists {
        for sort_by in attributes {
            let mut body = request.clone();
            body["Sort"] = json!({"SortBy": sort_by});
            assert_refused(
                &router,
                &Row {
                    case: format!("{operation} / Sort.SortBy {sort_by}"),
                    method: "POST",
                    uri: format!("/{operation}"),
                    body: Some(body),
                    message: format!(
                        "[Requested sort by '{sort_by}' is invalid. It should be one \
                         of '[]'.]"
                    ),
                },
            )
            .await;
        }
    }
}

/// RULE, for a published syntax with two alternatives: an `AssessmentIdentifier` of
/// neither form the service publishes is refused with the modelled `ValidationException`
/// and the sentence quoting the identifier and that syntax — the service's own spelling of
/// it, not the model's.
///
/// The rows span the ways an identifier misses both alternatives: the bare prefix, a word
/// of neither form, the id form with a character the alternative does not admit, and an
/// ARN whose resource portion is not an assessment of this catalog.
#[tokio::test]
async fn an_assessment_identifier_of_neither_published_form_is_the_services_own_422() {
    let router = emulator();

    for identifier in [
        "not-a-valid-identifier",
        "assessment-",
        "assessment-00_11",
        "arn:aws:aws-marketplace:eu-west-1::AWSMarketplace/Assessment/not-an-assessment",
    ] {
        assert_refused(
            &router,
            &Row {
                case: format!("DescribeAssessment / AssessmentIdentifier {identifier}"),
                method: "POST",
                uri: "/DescribeAssessment".to_string(),
                body: Some(json!({
                    "Catalog": CATALOG,
                    "AssessmentIdentifier": identifier,
                })),
                message: format!(
                    "[Requested assessment identifier '{identifier}' is invalid. \
                     It should match with {ASSESSMENT_IDENTIFIER_PATTERN}.]"
                ),
            },
        )
        .await;
    }
}
