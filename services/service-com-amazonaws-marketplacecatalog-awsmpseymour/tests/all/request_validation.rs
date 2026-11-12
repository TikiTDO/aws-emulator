// SPDX-License-Identifier: CC0-1.0

//! How this service refuses a REQUEST, end to end over HTTP — the one grammar every
//! operation of the AWS Marketplace Catalog states a bad request in, and the rules that
//! sit behind the members it accepts.
//!
//! Five rules are under test, each asserted over a SET of inputs rather than one:
//!
//! 1. A member outside the syntax the service publishes for it is refused with ONE
//!    bracketed fault built from the request — the member's label as words, the value it
//!    rejected and the syntax it required — whatever else in the same request is also
//!    malformed, and with the modelled `ValidationException` (422).
//! 2. An `EntityType` is checked against the entity types this catalog HAS, not only
//!    against the characters its `@pattern` admits, so a letters-only value naming no
//!    type is refused rather than answered as an empty page.
//! 3. An `Entity$Type` may address its type at a schema version (`<Type>@<version>`),
//!    which is the form every real caller sends: such a change passes shape validation
//!    and is judged by the rules behind it (the unique entity/change-type pair, then the
//!    entity lookup), and the entity it creates is of the type the reference names.
//! 4. A batch read degrades PER ENTRY: a malformed id and a missing one are reported in
//!    `Errors` under the id that was asked for, with the code a single read of that id
//!    would have raised, and the call still serves the entries beside them.
//! 5. An action on a resource ARN this catalog cannot authorize is refused with the
//!    authorization-failure sentence AWS states every such refusal in: the authenticated
//!    principal, the IAM action and the resource ID the request named.
//! 6. A `MaxResults` outside the range its model declares is refused as a client error by
//!    every paginated operation — never served, never coerced into a legal page size, and
//!    never a panic or a 5xx.

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

/// A SigV4 credential scope naming this service's signing name (`aws-marketplace`).
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The region [`SIGNED_FOR_EU_WEST_1`] signs for.
const REGION: &str = "eu-west-1";

/// The syntax the service publishes for a catalog NAME, written out here rather than
/// read from the crate so a wrong implementation cannot make this suite pass by quoting
/// its own pattern. It is WIDER in characters and narrower in length than the modelled
/// `Catalog` `@pattern` (`^[a-zA-Z]+$`, `@length 1..64`), which is why the model's
/// pattern cannot be what a caller is told to satisfy.
const CATALOG_PATTERN: &str =
    r"^(?![.-])(?!.*[.-]{2})(?![0-9]+\.[0-9.]+$)[a-zA-Z0-9.-]{1,47}(?<![.-])$";

/// The syntax the service publishes for an `Entity$Identifier`: an entity id optionally
/// at a revision, or the `$<ChangeName>.Entity.Identifier` reference one change uses to
/// name the entity another change of the same change set creates. The published syntax
/// states no length; the modelled `Identifier` `@length(min: 1, max: 255)` is a
/// requirement beside it.
const ENTITY_IDENTIFIER_PATTERN: &str =
    r"(^[a-zA-Z0-9][.a-zA-Z0-9/-]+[a-zA-Z0-9](@\d+)?$)|(^\$[a-zA-Z]{1,72}\.Entity\.Identifier$)";

/// The syntax the service publishes for a `ChangeSetName`: the modelled `@pattern` with
/// the modelled `@length(min: 1, max: 100)` folded into it.
const CHANGE_SET_NAME_PATTERN: &str = r"^[\w\s+=.:@-]{1,100}$";

/// The syntax the service publishes for a `ClientRequestToken`: the modelled `@pattern`
/// with the modelled `@length(min: 1, max: 64)` folded into it.
const CLIENT_REQUEST_TOKEN_PATTERN: &str = r"^[!-~]{1,64}$";

/// The syntax the service publishes for an `AssessmentIdentifier`: the two forms its
/// member's doc documents ("the assessment ID … or the full assessment ARN"), written as
/// two separately anchored alternatives joined by `|` with no account field in the ARN
/// branch — the service's own string, not the shape's modelled `@pattern`.
const ASSESSMENT_IDENTIFIER_PATTERN: &str = concat!(
    r"(^assessment-[a-zA-Z0-9]+$)|(^arn:[a-zA-Z0-9-]+:[a-zA-Z0-9-]+:[a-zA-Z0-9-]*::",
    r"[a-zA-Z0-9-]+/Assessment/assessment-[a-zA-Z0-9]+$)"
);

/// The `EntityId` `@pattern` (`smithy_query(query="shape", shape="EntityId")`).
const ENTITY_ID_PATTERN: &str = "^[a-zA-Z0-9][.a-zA-Z0-9/-]+[a-zA-Z0-9]$";

/// A resource-based policy document `PutResourcePolicy` accepts, so a row below is refused
/// for the ARN it names rather than for the document it carries.
const RESOURCE_POLICY_DOCUMENT: &str = "{\"Version\":\"2012-10-17\",\"Statement\":\
     [{\"Effect\":\"Allow\",\"Principal\":\"*\",\"Action\":\"aws-marketplace:DescribeEntity\"}]}";

/// The entity types this catalog has, written out here from the model rather than read
/// from the crate so a wrong implementation cannot make this suite pass by quoting its own
/// set. It is the UNION of the two places the model names them: the valid values
/// `ListEntitiesRequest$EntityType` enumerates in prose
/// (`smithy_query(query="doc", shape="ListEntitiesRequest", member="EntityType")`), and
/// `EntitySummary`'s per-type summary members with the `Summary` suffix dropped
/// (`smithy_query(query="shape", shape="EntitySummary")`, the same set the
/// `EntityTypeFilters` union names with `Filters` dropped), which adds
/// `MachineLearningProduct` and `ExpressPrivateOfferConfiguration`.
const MODELLED_ENTITY_TYPES: [&str; 15] = [
    "AmiProduct",
    "Audience",
    "BrandingSettings",
    "ContainerProduct",
    "DataProduct",
    "Experience",
    "ExpressPrivateOfferConfiguration",
    "MachineLearningProduct",
    "Offer",
    "OfferSet",
    "ProcurementPolicy",
    "ResaleAuthorization",
    "SaaSProduct",
    "Seller",
    "Solution",
];

/// How long after the start a change set has certainly reached `SUCCEEDED`.
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

    /// The fault this answer carries: `ValidationException$Message` and its siblings are
    /// all spelled `Message` on this service's errors.
    fn message(&self) -> String {
        self.text("/Message")
    }

    /// The keys of the JSON object at `pointer`, sorted.
    fn keys(&self, pointer: &str) -> Vec<String> {
        let mut keys: Vec<String> = self
            .body
            .pointer(pointer)
            .and_then(Value::as_object)
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default();
        keys.sort();
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

/// One `StartChangeSet` body, with every member the cases below vary.
fn start_change_set_body(changes: Value, extra: &[(&str, Value)]) -> Value {
    let mut request = json!({"Catalog": CATALOG, "ChangeSet": changes});
    for (member, value) in extra {
        request[*member] = value.clone();
    }
    request
}

/// One change creating an entity of `entity_type`, which carries its schema version as
/// `Change$Entity$Type` publishes it.
fn create_change(entity_type: &str) -> Value {
    json!({
        "ChangeType": "CreateProduct",
        "Entity": {"Type": entity_type},
        "Details": json!({"ProductTitle": "request-validation"}).to_string(),
    })
}

/// RULE 1: a member outside the syntax the service publishes for it is refused with ONE
/// bracketed fault naming the member as words, the value and that syntax — and with the
/// modelled `ValidationException`, whose `httpError` is 422.
///
/// The rows span the class the rule generalises over: which operation, which member,
/// whether the member is bound to the query string or the body, whether it sits at the
/// top level or inside a nested list element, and which published syntax it failed.
#[tokio::test]
async fn a_member_outside_its_published_syntax_is_refused_with_one_bracketed_fault() {
    let router = emulator();

    // (case, method, uri, body, expected Message)
    let rows: Vec<(&str, &str, String, Option<Value>, String)> = vec![
        (
            "CancelChangeSet: a query-bound catalog with an underscore",
            "PATCH",
            "/CancelChangeSet?catalog=AWS_Marketplace&changeSetId=sim2realabcdef012345".to_string(),
            None,
            format!(
                "[Requested catalog 'AWS_Marketplace' is invalid. \
                 It should match with {CATALOG_PATTERN}.]"
            ),
        ),
        (
            "DescribeChangeSet: a query-bound catalog with an underscore",
            "GET",
            "/DescribeChangeSet?catalog=AWS_Marketplace&changeSetId=abc123".to_string(),
            None,
            format!(
                "[Requested catalog 'AWS_Marketplace' is invalid. \
                 It should match with {CATALOG_PATTERN}.]"
            ),
        ),
        (
            "DescribeEntity: a query-bound catalog ending in a hyphen",
            "GET",
            "/DescribeEntity?catalog=AWSMarketplace-&entityId=prod-abcdef".to_string(),
            None,
            format!(
                "[Requested catalog 'AWSMarketplace-' is invalid. \
                 It should match with {CATALOG_PATTERN}.]"
            ),
        ),
        (
            "ListAssessments: a body-bound catalog with a space",
            "POST",
            "/ListAssessments".to_string(),
            Some(json!({"Catalog": "AWS Marketplace"})),
            format!(
                "[Requested catalog 'AWS Marketplace' is invalid. \
                 It should match with {CATALOG_PATTERN}.]"
            ),
        ),
        (
            "ListEntities: a catalog that is a bare version number",
            "POST",
            "/ListEntities".to_string(),
            Some(json!({"Catalog": "1.0", "EntityType": "AmiProduct"})),
            format!(
                "[Requested catalog '1.0' is invalid. It should match with {CATALOG_PATTERN}.]"
            ),
        ),
        (
            "StartChangeSet: a catalog with two adjacent separators",
            "POST",
            "/StartChangeSet".to_string(),
            Some(start_change_set_body(
                json!([create_change("AmiProduct@1.0")]),
                &[("Catalog", json!("AWS--Marketplace"))],
            )),
            format!(
                "[Requested catalog 'AWS--Marketplace' is invalid. \
                 It should match with {CATALOG_PATTERN}.]"
            ),
        ),
        (
            "DescribeAssessment: an identifier of neither published form",
            "POST",
            "/DescribeAssessment".to_string(),
            Some(json!({
                "Catalog": CATALOG,
                "AssessmentIdentifier": "not-a-valid-identifier",
            })),
            format!(
                "[Requested assessment identifier 'not-a-valid-identifier' is invalid. \
                 It should match with {ASSESSMENT_IDENTIFIER_PATTERN}.]"
            ),
        ),
        (
            "ListChangeSets: a page token outside the token syntax",
            "POST",
            "/ListChangeSets".to_string(),
            Some(json!({"Catalog": CATALOG, "NextToken": "not a valid token !!"})),
            "[Requested page token is invalid.]".to_string(),
        ),
        (
            "ListEntities: a page token this service never issued",
            "POST",
            "/ListEntities".to_string(),
            Some(json!({
                "Catalog": CATALOG,
                "EntityType": "AmiProduct",
                "NextToken": "CS-0",
            })),
            "[Requested page token is invalid.]".to_string(),
        ),
        (
            "StartChangeSet: a change set name outside its syntax",
            "POST",
            "/StartChangeSet".to_string(),
            Some(start_change_set_body(
                json!([create_change("DataProduct@1.0")]),
                &[("ChangeSetName", json!("bad#name#xc36cd3d01e3b"))],
            )),
            format!(
                "[Requested change set name 'bad#name#xc36cd3d01e3b' is invalid. \
                 It should match with {CHANGE_SET_NAME_PATTERN}.]"
            ),
        ),
        (
            "StartChangeSet: a change set name past its published length",
            "POST",
            "/StartChangeSet".to_string(),
            Some(start_change_set_body(
                json!([create_change("DataProduct@1.0")]),
                &[("ChangeSetName", json!("n".repeat(101)))],
            )),
            format!(
                "[Requested change set name '{}' is invalid. \
                 It should match with {CHANGE_SET_NAME_PATTERN}.]",
                "n".repeat(101)
            ),
        ),
        (
            "StartChangeSet: a change set name of letters outside ASCII",
            "POST",
            "/StartChangeSet".to_string(),
            Some(start_change_set_body(
                json!([create_change("DataProduct@1.0")]),
                &[("ChangeSetName", json!("ユーザー"))],
            )),
            format!(
                "[Requested change set name 'ユーザー' is invalid. \
                 It should match with {CHANGE_SET_NAME_PATTERN}.]"
            ),
        ),
        (
            "StartChangeSet: a client request token with a space",
            "POST",
            "/StartChangeSet".to_string(),
            Some(start_change_set_body(
                json!([create_change("DataProduct@1.0")]),
                &[("ClientRequestToken", json!("has a space in it"))],
            )),
            format!(
                "[Requested client request token 'has a space in it' is invalid. \
                 It should match with {CLIENT_REQUEST_TOKEN_PATTERN}.]"
            ),
        ),
        (
            "StartChangeSet: a client request token past its published length",
            "POST",
            "/StartChangeSet".to_string(),
            Some(start_change_set_body(
                json!([create_change("DataProduct@1.0")]),
                &[("ClientRequestToken", json!("t".repeat(65)))],
            )),
            format!(
                "[Requested client request token '{}' is invalid. \
                 It should match with {CLIENT_REQUEST_TOKEN_PATTERN}.]",
                "t".repeat(65)
            ),
        ),
        (
            "StartChangeSet: an entity identifier inside a list element",
            "POST",
            "/StartChangeSet".to_string(),
            Some(start_change_set_body(
                json!([{
                    "ChangeType": "AddRevisions",
                    "Entity": {"Identifier": "bad#identifier", "Type": "DataProduct@1.0"},
                    "Details": "{}",
                }]),
                &[],
            )),
            format!(
                "[Requested entity identifier 'bad#identifier' is invalid. \
                 It should match with {ENTITY_IDENTIFIER_PATTERN}.]"
            ),
        ),
        (
            "StartChangeSet: an entity identifier whose revision is not a number",
            "POST",
            "/StartChangeSet".to_string(),
            Some(start_change_set_body(
                json!([{
                    "ChangeType": "AddRevisions",
                    "Entity": {"Identifier": "prod-abc123@v1", "Type": "DataProduct@1.0"},
                    "Details": "{}",
                }]),
                &[],
            )),
            format!(
                "[Requested entity identifier 'prod-abc123@v1' is invalid. \
                 It should match with {ENTITY_IDENTIFIER_PATTERN}.]"
            ),
        ),
        (
            "ListEntities: an entity type this catalog has no such type for",
            "POST",
            "/ListEntities".to_string(),
            Some(json!({"Catalog": CATALOG, "EntityType": "Ami1Product"})),
            "[Requested entity type 'Ami1Product' is invalid.]".to_string(),
        ),
    ];

    for (case, method, uri, body, expected) in rows {
        let answer = call(&router, method, &uri, body).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (422, "ValidationException"),
            "case {case:?}: a member outside its published syntax must answer the modelled \
             422 ValidationException, got {} {:?} with {:?}",
            answer.status,
            answer.error_code(),
            answer.body
        );
        assert_eq!(
            answer.message(),
            expected,
            "case {case:?}: the fault must be the one bracketed sentence built from the \
             request, got {:?}",
            answer.message()
        );
        // The service says everything it has to say in `Message`; the modelled field list
        // stays absent, as its own recorded refusals show.
        assert_eq!(
            answer.body.pointer("/ValidationExceptionFieldList"),
            None,
            "case {case:?}: this service reports a refusal in Message alone, got {:?}",
            answer.body
        );
    }
}

/// RULE 1 (the fault COUNT): a request with several malformed members is answered with
/// exactly ONE fault, not one per member.
///
/// The rows differ along how many members are malformed and which ones, so the rule is
/// asserted on more than the two-fault case the recording showed.
#[tokio::test]
async fn exactly_one_fault_is_reported_however_many_members_are_malformed() {
    let router = emulator();

    let malformed_change = json!({
        "ChangeType": "AddRevisions",
        "Entity": {"Identifier": "bad#identifier", "Type": "DataProduct@1.0"},
        "Details": "{}",
    });

    // (case, body)
    let rows: Vec<(&str, Value)> = vec![
        (
            "a bad name and a bad identifier",
            start_change_set_body(
                json!([malformed_change.clone()]),
                &[("ChangeSetName", json!("bad#name"))],
            ),
        ),
        (
            "a bad name, a bad token and a bad identifier",
            start_change_set_body(
                json!([malformed_change.clone()]),
                &[
                    ("ChangeSetName", json!("bad#name")),
                    ("ClientRequestToken", json!("has a space in it")),
                ],
            ),
        ),
        (
            "two change elements, each with a bad identifier",
            start_change_set_body(
                json!([malformed_change.clone(), malformed_change.clone()]),
                &[],
            ),
        ),
        (
            "a bad catalog on top of everything else",
            start_change_set_body(
                json!([malformed_change.clone()]),
                &[
                    ("Catalog", json!("AWS_Marketplace")),
                    ("ChangeSetName", json!("bad#name")),
                    ("ClientRequestToken", json!("has a space in it")),
                ],
            ),
        ),
    ];

    for (case, body) in rows {
        let answer = call(&router, "POST", "/StartChangeSet", Some(body)).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (422, "ValidationException"),
            "case {case:?}: must answer the modelled 422 ValidationException, got {} {:?} \
             with {:?}",
            answer.status,
            answer.error_code(),
            answer.body
        );
        let message = answer.message();
        assert_eq!(
            message.matches("Requested").count(),
            1,
            "case {case:?}: exactly one fault is reported however many members are \
             malformed, got {message:?}"
        );
        assert!(
            message.starts_with('[') && message.ends_with(']'),
            "case {case:?}: the fault is ONE bracketed sentence, got {message:?}"
        );
    }
}

/// RULE 2: `EntityType` is checked against the entity types this catalog HAS. A value
/// naming one of them is listed (an empty page while nothing of that type exists); a
/// letters-only value naming none is refused.
///
/// The accepted rows are the whole modelled set — including the types the recording never
/// named, so a set missing one of them fails here — and the refused rows span the ways a
/// value can miss it: an invented word, a near miss in case, a plural, a type with a digit
/// in it and one past the published length.
#[tokio::test]
async fn an_entity_type_outside_the_catalogs_own_set_is_refused() {
    let router = emulator();

    for entity_type in MODELLED_ENTITY_TYPES {
        let answer = call(
            &router,
            "POST",
            "/ListEntities",
            Some(json!({"Catalog": CATALOG, "EntityType": entity_type})),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {entity_type:?}: a type this catalog has must be listed, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body.pointer("/EntitySummaryList"),
            Some(&json!([])),
            "case {entity_type:?}: an empty page is reported as a present, empty list, \
             got {:?}",
            answer.body
        );
    }

    let too_long = "A".repeat(256);
    let refused = [
        ("an invented type", "Frobnicator"),
        ("a longer invented type", "NotARealMarketplaceProductType"),
        ("a real type in the wrong case", "amiproduct"),
        ("a real type pluralised", "AmiProducts"),
        ("a real type with a digit in it", "Ami1Product"),
        ("a type past the published length", too_long.as_str()),
    ];
    for (case, entity_type) in refused {
        let answer = call(
            &router,
            "POST",
            "/ListEntities",
            Some(json!({"Catalog": CATALOG, "EntityType": entity_type})),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (422, "ValidationException"),
            "case {case:?}: {entity_type:?} names no entity type and must be refused \
             before any listing, got {} {:?} with {:?}",
            answer.status,
            answer.error_code(),
            answer.body
        );
        assert_eq!(
            answer.message(),
            format!("[Requested entity type '{entity_type}' is invalid.]"),
            "case {case:?}: the fault names the value and quotes no syntax, got {:?}",
            answer.message()
        );
    }
}

/// RULE 3: an `Entity$Type` at a schema version passes shape validation and reaches the
/// rules behind it, and the entity such a change creates is of the type the reference
/// names — never of the versioned reference.
///
/// The rows differ along the type, the version form and the rule reached: the entity
/// lookup (a 404 naming the entity id inside the identifier), the unique entity/change
/// type pair (a 422, reached whether or not the entity exists), and a create that
/// succeeds and reads back.
#[tokio::test]
async fn a_versioned_entity_type_reaches_the_rules_behind_it() {
    // (case, versioned reference, the type it names)
    let rows = [
        ("a dotted version", "DataProduct@1.0", "DataProduct"),
        ("a bare major version", "AmiProduct@2", "AmiProduct"),
        (
            "a type the recording never named",
            "OfferSet@1.0",
            "OfferSet",
        ),
        ("a three-part version", "SaaSProduct@1.2.3", "SaaSProduct"),
    ];

    for (case, reference, entity_type) in rows {
        let router = emulator();

        // The entity lookup: a well-formed identifier naming no entity is the modelled
        // 404, and it names the entity id inside the identifier — not the revision it
        // was addressed at.
        let absent = call(
            &router,
            "POST",
            "/StartChangeSet",
            Some(start_change_set_body(
                json!([{
                    "ChangeType": "AddRevisions",
                    "Entity": {"Identifier": "prod-abc123@1", "Type": reference},
                    "Details": json!({"DataSetArn": "data-set-arn"}).to_string(),
                }]),
                &[],
            )),
        )
        .await;
        assert_eq!(
            (absent.status, absent.error_code(), absent.message()),
            (
                404,
                "ResourceNotFoundException",
                "Requested entity 'prod-abc123' does not exist.".to_string()
            ),
            "case {case:?}: a versioned entity type must reach the entity lookup, got {} \
             {:?} with {:?}",
            absent.status,
            absent.error_code(),
            absent.body
        );

        // The uniqueness rule is a rule about the REQUEST, so it is reached before the
        // entity is resolved: a change set that contradicts itself is refused whether or
        // not the entity it names exists.
        let contradicting = call(
            &router,
            "POST",
            "/StartChangeSet",
            Some(start_change_set_body(
                json!([
                    {
                        "ChangeType": "AddRevisions",
                        "Entity": {"Identifier": "prod-abc123@1", "Type": reference},
                        "Details": "{}",
                    },
                    {
                        "ChangeType": "AddRevisions",
                        "Entity": {"Identifier": "prod-abc123@1", "Type": reference},
                        "Details": "{}",
                    },
                ]),
                &[],
            )),
        )
        .await;
        assert_eq!(
            (
                contradicting.status,
                contradicting.error_code(),
                contradicting.message()
            ),
            (
                422,
                "ValidationException",
                "[Requested changes contain multiple changes for the same change type \
                 'AddRevisions' on entity 'prod-abc123'. Entity - change type pairs \
                 should be unique.]"
                    .to_string()
            ),
            "case {case:?}: the repeated (entity, change type) pair must be refused \
             before the entity is looked up, got {} {:?} with {:?}",
            contradicting.status,
            contradicting.error_code(),
            contradicting.body
        );

        // A create with that reference succeeds, and the entity it leaves is of the type
        // the reference names.
        let created = call(
            &router,
            "POST",
            "/StartChangeSet",
            Some(start_change_set_body(json!([create_change(reference)]), &[])),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: a versioned entity type must be accepted, got {} with {:?}",
            created.status, created.body
        );
        wait_for_success();
        let listed = call(
            &router,
            "POST",
            "/ListEntities",
            Some(json!({"Catalog": CATALOG, "EntityType": entity_type})),
        )
        .await;
        let summaries = listed
            .body
            .pointer("/EntitySummaryList")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            summaries.len(),
            1,
            "case {case:?}: the created entity must be listed under {entity_type:?}, \
             got {:?}",
            listed.body
        );
        let entity_id = summaries[0]["EntityId"].as_str().unwrap_or_default();
        let described = call(
            &router,
            "GET",
            &format!("/DescribeEntity?catalog={CATALOG}&entityId={entity_id}"),
            None,
        )
        .await;
        assert_eq!(
            (described.status, described.text("/EntityType")),
            (200, entity_type.to_string()),
            "case {case:?}: the stored entity is of the type the reference names, got {} \
             with {:?}",
            described.status,
            described.body
        );
        assert!(
            described
                .text("/EntityArn")
                .ends_with(&format!("{CATALOG}/{entity_type}/{entity_id}")),
            "case {case:?}: the entity's ARN names its type, not the version it was \
             addressed at, got {:?}",
            described.body
        );

        // The unique entity/change-type rule: the same change twice against that entity
        // is the request contradicting itself, which is the modelled 422.
        let repeated = call(
            &router,
            "POST",
            "/StartChangeSet",
            Some(start_change_set_body(
                json!([
                    {
                        "ChangeType": "AddRevisions",
                        "Entity": {"Identifier": format!("{entity_id}@1"), "Type": reference},
                        "Details": "{}",
                    },
                    {
                        "ChangeType": "AddRevisions",
                        "Entity": {"Identifier": format!("{entity_id}@1"), "Type": reference},
                        "Details": "{}",
                    },
                ]),
                &[],
            )),
        )
        .await;
        assert_eq!(
            (repeated.status, repeated.error_code()),
            (422, "ValidationException"),
            "case {case:?}: two changes of one change type against one entity must be \
             refused 422 ValidationException, got {} {:?} with {:?}",
            repeated.status,
            repeated.error_code(),
            repeated.body
        );
        assert!(
            repeated.message().contains("AddRevisions")
                && repeated.message().contains(entity_id)
                && repeated.message().contains("should be unique"),
            "case {case:?}: the fault must name the repeated change type and the entity, \
             got {:?}",
            repeated.message()
        );
    }

    // A reference whose TYPE part names no entity type, and one whose version part is
    // not a version, are still refused — accepting the versioned form widens the syntax,
    // it does not drop the check.
    let router = emulator();
    for reference in ["Frobnicator@1.0", "DataProduct@", "DataProduct@1..0", "@1.0"] {
        let answer = call(
            &router,
            "POST",
            "/StartChangeSet",
            Some(start_change_set_body(
                json!([create_change(reference)]),
                &[],
            )),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code(), answer.message()),
            (
                422,
                "ValidationException",
                format!("[Requested entity type '{reference}' is invalid.]")
            ),
            "case {reference:?}: a reference naming no entity type must be refused, got {} \
             {:?} with {:?}",
            answer.status,
            answer.error_code(),
            answer.body
        );
    }
}

/// RULE 4: a batch read reports each entry's own failure in `Errors`, keyed by the id
/// that was asked for — whether that entry named a missing entity or an id that is no
/// entity id at all — and serves the rest of the batch, so one bad entry costs the caller
/// nothing but that entry.
///
/// The rows differ along the mix: a missing id alone, two of them, a malformed id alone,
/// both together, and both together beside an entity the catalog holds.
#[tokio::test]
async fn a_mixed_batch_reports_each_entry_under_its_own_id() {
    let router = emulator();

    // One entity that really exists, so a partial answer is visible beside the failures.
    let created = call(
        &router,
        "POST",
        "/StartChangeSet",
        Some(start_change_set_body(
            json!([create_change("AmiProduct@1.0")]),
            &[],
        )),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "StartChangeSet must answer 200, got {} with {:?}",
        created.status, created.body
    );
    wait_for_success();
    let listed = call(
        &router,
        "POST",
        "/ListEntities",
        Some(json!({"Catalog": CATALOG, "EntityType": "AmiProduct"})),
    )
    .await;
    let held = listed
        .body
        .pointer("/EntitySummaryList/0/EntityId")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| panic!("the applied change set must leave one entity: {:?}", listed.body));

    /// A well-formed entity id no change set ever created.
    const MISSING: &str = "prod-abc123";
    /// A second one, so a row can carry two.
    const MISSING_TWO: &str = "prod-abc124";
    /// An id that is no entity id at all, reported under the value that was sent.
    const MALFORMED: &str = "bad entity id!";

    /// One row of the batch table: the case, the ids the batch asks for, the ids that
    /// must land in `EntityDetails`, and the `(id, ErrorCode)` pairs that must land in
    /// `Errors`.
    type BatchRow = (
        &'static str,
        Vec<String>,
        Vec<String>,
        Vec<(&'static str, &'static str)>,
    );

    // (case, requested ids, ids expected in EntityDetails, (id, code) expected in Errors)
    let rows: Vec<BatchRow> = vec![
        (
            "a missing id alone",
            vec![MISSING.to_string()],
            vec![],
            vec![(MISSING, "ResourceNotFoundException")],
        ),
        (
            "two missing ids",
            vec![MISSING.to_string(), MISSING_TWO.to_string()],
            vec![],
            vec![
                (MISSING, "ResourceNotFoundException"),
                (MISSING_TWO, "ResourceNotFoundException"),
            ],
        ),
        (
            "a held entity beside a missing one",
            vec![held.clone(), MISSING.to_string()],
            vec![held.clone()],
            vec![(MISSING, "ResourceNotFoundException")],
        ),
        (
            "every entry served",
            vec![held.clone()],
            vec![held.clone()],
            vec![],
        ),
        (
            "a malformed id alone",
            vec![MALFORMED.to_string()],
            vec![],
            vec![(MALFORMED, "ValidationException")],
        ),
        (
            "a missing id and a malformed one",
            vec![MISSING.to_string(), MALFORMED.to_string()],
            vec![],
            vec![
                (MISSING, "ResourceNotFoundException"),
                (MALFORMED, "ValidationException"),
            ],
        ),
        (
            "a held entity beside both failures",
            vec![held.clone(), MISSING.to_string(), MALFORMED.to_string()],
            vec![held.clone()],
            vec![
                (MISSING, "ResourceNotFoundException"),
                (MALFORMED, "ValidationException"),
            ],
        ),
    ];

    for (case, requested, served, failed) in rows {
        let answer = batch_read(&router, &requested).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a batch read answers 200 whatever its entries resolve to, \
             got {} with {:?}",
            answer.status,
            answer.body
        );
        let mut expected_served = served.clone();
        expected_served.sort();
        assert_eq!(
            answer.keys("/EntityDetails"),
            expected_served,
            "case {case:?}: EntityDetails holds exactly the entities the catalog has, \
             got {:?}",
            answer.body
        );
        let mut expected_failed: Vec<String> =
            failed.iter().map(|(id, _)| (*id).to_string()).collect();
        expected_failed.sort();
        assert_eq!(
            answer.keys("/Errors"),
            expected_failed,
            "case {case:?}: Errors is keyed by the exact requested id, got {:?}",
            answer.body
        );
        for (entity_id, code) in &failed {
            assert_eq!(
                answer.text(&format!("/Errors/{entity_id}/ErrorCode")),
                *code,
                "case {case:?}: entry {entity_id:?} must carry the code a single read of \
                 it would raise, got {:?}",
                answer.body
            );
            let message = answer.text(&format!("/Errors/{entity_id}/ErrorMessage"));
            let expected = if *code == "ValidationException" {
                format!(
                    "Requested entity id '{entity_id}' is invalid. \
                     It should match with {ENTITY_ID_PATTERN}."
                )
            } else {
                format!("Requested entity '{entity_id}' does not exist.")
            };
            assert_eq!(
                message, expected,
                "case {case:?}: entry {entity_id:?}'s message must be built from the \
                 request, got {message:?}"
            );
        }
    }
}

/// The batch read of `entity_ids`, every entry naming the one catalog this service has.
async fn batch_read(router: &MultiServiceRouter, entity_ids: &[String]) -> Answer {
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

/// RULE 5: an action on a resource ARN this catalog cannot authorize is refused with the
/// authorization-failure sentence AWS states every such refusal in — the authenticated
/// principal, the IAM action and the resource ID the ARN named.
///
/// The rows span every operation addressed by a `ResourceArn` (so the rule is asserted
/// past the two the recording exercised) and the classes of ARN the catalog cannot
/// authorize: another account, another catalog, and a resource portion that names no
/// resource of this catalog at all.
#[tokio::test]
async fn an_unauthorized_action_names_the_caller_principal_and_the_resource_id() {
    let router = emulator();

    // The principal this emulator's callers authenticate as, spelled out as
    // `sts:GetCallerIdentity` answers it: the one IAM user of the emulator's own account.
    let caller = format!(
        "arn:aws:iam::{}:user/emulator",
        ::engine::emulator_account_id()
    );
    let account = ::engine::emulator_account_id();

    // (case, resource ARN, the resource ID it names)
    let arns: Vec<(&str, String, &str)> = vec![
        (
            "a catalog-shaped resource in another account",
            format!("arn:aws:aws-marketplace:{REGION}:111111111111:{CATALOG}/AmiProduct/sim2real-behav-abc123"),
            "sim2real-behav-abc123",
        ),
        (
            "this account, but another catalog",
            format!("arn:aws:aws-marketplace:{REGION}:{account}:OtherCatalog/AmiProduct/prod-abcdef"),
            "prod-abcdef",
        ),
        (
            "a resource portion with no catalog path at all",
            format!("arn:aws:aws-marketplace:{REGION}:{account}:sim2real-behav-1b187cbd960a"),
            "sim2real-behav-1b187cbd960a",
        ),
        (
            "a change set of another account",
            format!("arn:aws:aws-marketplace:{REGION}:111111111111:{CATALOG}/ChangeSet/cs-000000000001"),
            "cs-000000000001",
        ),
    ];

    for (case, arn, resource_id) in arns {
        for (operation, method, uri, body) in [
            (
                "ListTagsForResource",
                "POST",
                "/ListTagsForResource".to_string(),
                Some(json!({"ResourceArn": arn})),
            ),
            (
                "TagResource",
                "POST",
                "/TagResource".to_string(),
                Some(json!({"ResourceArn": arn, "Tags": [{"Key": "k", "Value": "v"}]})),
            ),
            (
                "UntagResource",
                "POST",
                "/UntagResource".to_string(),
                Some(json!({"ResourceArn": arn, "TagKeys": ["k"]})),
            ),
            (
                "PutResourcePolicy",
                "POST",
                "/PutResourcePolicy".to_string(),
                Some(json!({"ResourceArn": arn, "Policy": RESOURCE_POLICY_DOCUMENT})),
            ),
            (
                "GetResourcePolicy",
                "GET",
                format!("/GetResourcePolicy?resourceArn={arn}"),
                None,
            ),
            (
                "DeleteResourcePolicy",
                "DELETE",
                format!("/DeleteResourcePolicy?resourceArn={arn}"),
                None,
            ),
        ] {
            let answer = call(&router, method, &uri, body).await;
            assert_eq!(
                (answer.status, answer.error_code()),
                (403, "AccessDeniedException"),
                "case {case:?}/{operation}: an ARN this catalog cannot authorize must be \
                 refused 403 AccessDeniedException, got {} {:?} with {:?}",
                answer.status,
                answer.error_code(),
                answer.body
            );
            assert_eq!(
                answer.message(),
                format!(
                    "User: {caller} is not authorized to perform: \
                     aws-marketplace:{operation} on resource: {resource_id} because no \
                     resource-based policy allows the aws-marketplace:{operation} action"
                ),
                "case {case:?}/{operation}: the refusal must name the denied principal, \
                 the IAM action and the resource ID, got {:?}",
                answer.message()
            );
        }
    }
}

/// RULE 6: every paginated operation refuses a `MaxResults` outside the range its model
/// declares as a client error — it is never served, never coerced into a legal page size
/// and never a 5xx — while the published bounds themselves are served.
///
/// The rows are the four paginated operations (whose ranges differ: 1..=50, 1..=20 and
/// 1..=100 twice) crossed with the ways a page size can leave the range: zero, negative,
/// and one past the published maximum.
#[tokio::test]
async fn a_page_size_outside_the_modelled_range_is_refused_as_a_client_error() {
    let router = emulator();

    // (operation, uri, a body with MaxResults filled in, the published maximum) — the
    // maxima come from the model's own range traits
    // (`smithy_query(query="operation", operation="ListEntities,ListChangeSets,…")`).
    let operations: Vec<(&str, &str, Value, i64)> = vec![
        (
            "ListEntities",
            "/ListEntities",
            json!({"Catalog": CATALOG, "EntityType": "AmiProduct"}),
            50,
        ),
        ("ListChangeSets", "/ListChangeSets", json!({"Catalog": CATALOG}), 20),
        ("ListAssessments", "/ListAssessments", json!({"Catalog": CATALOG}), 100),
        (
            "DescribeAssessment",
            "/DescribeAssessment",
            json!({"Catalog": CATALOG, "AssessmentIdentifier": "assessment-1"}),
            100,
        ),
    ];

    for (operation, uri, template, maximum) in operations {
        for requested in [0, -1, maximum + 1, i64::from(i32::MAX)] {
            let mut body = template.clone();
            body["MaxResults"] = json!(requested);
            let answer = call(&router, "POST", uri, Some(body)).await;
            assert!(
                (400..500).contains(&answer.status),
                "case {operation}/MaxResults={requested}: a page size outside the modelled \
                 range must be refused as a client error, got {} with {:?}",
                answer.status,
                answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {operation}/MaxResults={requested}: must answer ValidationException, \
                 got {:?}",
                answer.body
            );
        }

        // Both published bounds are served — which is what makes the refusals above about
        // the range and not about the member.
        for requested in [1, maximum] {
            let mut body = template.clone();
            body["MaxResults"] = json!(requested);
            let answer = call(&router, "POST", uri, Some(body)).await;
            let expected = if operation == "DescribeAssessment" {
                // The identifier is well formed and names no assessment, so this one
                // reaches the lookup rather than a page.
                404
            } else {
                200
            };
            assert_eq!(
                answer.status, expected,
                "case {operation}/MaxResults={requested}: a published bound must be \
                 served, got {} with {:?}",
                answer.status,
                answer.body
            );
        }
    }
}
