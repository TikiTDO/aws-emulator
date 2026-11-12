// SPDX-License-Identifier: CC0-1.0

//! The modelled `@length` of a member this service judges with a predicate of its own,
//! end to end over HTTP.
//!
//! One rule is under test: a member whose boundary validation this service replaces with
//! a hand-written predicate is still bounded by that member's modelled `@length`, so a
//! value past the bound is refused as malformed — before any lookup, so it is never
//! answered as missing, never stored, and never reported back inside a shape whose
//! `@length` forbids it.
//!
//! The rows span the members the rule reaches (`Entity$Type`, `Entity$Identifier`,
//! `EntityRequest$EntityId`, `DescribeAssessmentRequest$AssessmentIdentifier`), the forms
//! each one accepts (a bare value, a type at a schema version, an id at a revision, an id
//! and the full ARN), and for every one of them the published bound, one character past
//! it, and a value far past it.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The catalog every operation's `Catalog` member documents as its fixed value.
const CATALOG: &str = "AWSMarketplace";

/// A SigV4 credential scope naming this service's signing name (`aws-marketplace`).
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// How long a member naming an entity, its type or a revision of one may be: the
/// `@length(min: 1, max: 255)` the model puts on `EntityType`, `Identifier`, `EntityId`
/// and `ResourceId` alike
/// (`smithy_query(query="shape", shape="Entity,Identifier,EntityType,EntityId")`).
/// Written out here rather than read from the crate so a wrong implementation cannot make
/// this suite pass by quoting its own bound.
const ENTITY_MEMBER_MAX: usize = 255;

/// How long an `AssessmentIdentifier` may be: that shape's
/// `@length(min: 1, max: 2048)` (`smithy_query(query="shape",
/// shape="AssessmentIdentifier")`).
const ASSESSMENT_IDENTIFIER_MAX: usize = 2048;

/// How long any `Message` of this service may be: `ExceptionMessageContent`'s
/// `@length(min: 1, max: 2048)` (`smithy_query(query="shape",
/// shape="ExceptionMessageContent")`), the shape every modelled error's `Message`
/// targets — so a refusal quoting a value longer than that cannot carry it whole.
const EXCEPTION_MESSAGE_MAX: usize = 2048;

/// The syntax the service publishes for an `Entity$Identifier`: an entity id optionally
/// at a revision, or the `$<ChangeName>.Entity.Identifier` reference one change uses to
/// name the entity another change of the same change set creates. The published syntax
/// states no length — it is the one this service's recorded refusals quote — so the
/// modelled `@length` is a requirement beside it, not inside it.
const ENTITY_IDENTIFIER_PATTERN: &str =
    r"(^[a-zA-Z0-9][.a-zA-Z0-9/-]+[a-zA-Z0-9](@\d+)?$)|(^\$[a-zA-Z]{1,72}\.Entity\.Identifier$)";

/// The syntax the service publishes for an entity id: the modelled `EntityId`
/// `@pattern`, quoted verbatim in a refusal.
const ENTITY_ID_PATTERN: &str = "^[a-zA-Z0-9][.a-zA-Z0-9/-]+[a-zA-Z0-9]$";

/// The syntax the service publishes for an `AssessmentIdentifier`: the two forms the
/// member's doc documents, as two separately anchored alternatives joined by `|` with no
/// account field in the ARN branch — the service's own string, not the modelled `@pattern`.
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

/// An entity type reference of exactly `length` characters: a type this catalog has, at a
/// schema version of digits, which is the versioned form every real caller sends.
fn entity_type_reference(entity_type: &str, length: usize) -> String {
    let head = format!("{entity_type}@");
    format!("{head}{}", "1".repeat(length - head.chars().count()))
}

/// An entity id of exactly `length` characters, inside the published entity-id syntax.
fn entity_id(length: usize) -> String {
    "a".repeat(length)
}

/// An assessment id of exactly `length` characters: the `assessment-` prefix and a run of
/// alphanumerics, which is the id form `AssessmentIdentifier` accepts.
fn assessment_id(length: usize) -> String {
    let head = "assessment-";
    format!("{head}{}", "b".repeat(length - head.chars().count()))
}

/// An assessment ARN of exactly `length` characters, naming an assessment of this catalog
/// in this service's own ARN namespace — the other form `AssessmentIdentifier` accepts.
fn assessment_arn(length: usize) -> String {
    let head = format!("arn:aws:aws-marketplace:eu-west-1::{CATALOG}/Assessment/assessment-");
    format!("{head}{}", "c".repeat(length - head.chars().count()))
}

/// One `StartChangeSet` body carrying a single change for `entity`.
fn start_change_set_body(entity: Value) -> Value {
    json!({
        "Catalog": CATALOG,
        "ChangeSet": [{
            "ChangeType": "CreateProduct",
            "Entity": entity,
            "Details": json!({"ProductTitle": "modelled-member-lengths"}).to_string(),
        }],
    })
}

/// One row: the case, the request, and the answer the rule requires — status, wire error
/// code (empty for a success) and the exact fault, where the row states one.
type Row = (
    String,
    &'static str,
    String,
    Option<Value>,
    u16,
    &'static str,
    Option<String>,
);

/// RULE: a member this service judges itself is bounded by the member's modelled
/// `@length`, so a value past the bound is refused as malformed (422
/// `ValidationException`) rather than accepted, stored or reported missing — while a value
/// AT the bound is accepted and judged by the rules behind it.
#[tokio::test]
async fn a_hand_judged_member_is_bounded_by_its_modelled_length() {
    let router = emulator();

    let mut rows: Vec<Row> = Vec::new();

    // `Entity$Type` — a type this catalog has, at a schema version. The version is what
    // can grow, so the bound is on the whole reference.
    for (label, entity_type, length) in [
        ("at the bound", "AmiProduct", ENTITY_MEMBER_MAX),
        // An entity type the issue did not cite, to pin the rule on the member rather
        // than on one type.
        ("at the bound", "DataProduct", ENTITY_MEMBER_MAX),
        ("one past the bound", "AmiProduct", ENTITY_MEMBER_MAX + 1),
        ("far past the bound", "AmiProduct", 1011),
    ] {
        let reference = entity_type_reference(entity_type, length);
        let refused = length > ENTITY_MEMBER_MAX;
        rows.push((
            format!("StartChangeSet / Entity.Type {entity_type} {label} ({length})"),
            "POST",
            "/StartChangeSet".to_string(),
            Some(start_change_set_body(json!({"Type": reference.clone()}))),
            if refused { 422 } else { 200 },
            if refused { "ValidationException" } else { "" },
            refused.then(|| format!("[Requested entity type '{reference}' is invalid.]")),
        ));
    }

    // `Entity$Identifier` — an entity id, on its own or at a revision. The identifier and
    // the revision share the member's budget, so the bound is on the whole value; an
    // identifier inside it reaches the lookup and is reported MISSING instead.
    for (label, identifier, length) in [
        ("at the bound", entity_id(ENTITY_MEMBER_MAX), ENTITY_MEMBER_MAX),
        (
            "at the bound, at a revision",
            format!("{}@12", entity_id(ENTITY_MEMBER_MAX - 3)),
            ENTITY_MEMBER_MAX,
        ),
        (
            "one past the bound",
            entity_id(ENTITY_MEMBER_MAX + 1),
            ENTITY_MEMBER_MAX + 1,
        ),
        (
            "one past the bound, at a revision",
            format!("{}@12", entity_id(ENTITY_MEMBER_MAX - 2)),
            ENTITY_MEMBER_MAX + 1,
        ),
        ("far past the bound", entity_id(300), 300),
    ] {
        assert_eq!(
            identifier.chars().count(),
            length,
            "case {label:?}: the row's identifier must be {length} characters long, got {}",
            identifier.chars().count()
        );
        let refused = length > ENTITY_MEMBER_MAX;
        let named = identifier
            .split('@')
            .next()
            .unwrap_or(&identifier)
            .to_string();
        rows.push((
            format!("StartChangeSet / Entity.Identifier {label} ({length})"),
            "POST",
            "/StartChangeSet".to_string(),
            Some(start_change_set_body(
                json!({"Type": "AmiProduct@1.0", "Identifier": identifier.clone()}),
            )),
            if refused { 422 } else { 404 },
            if refused {
                "ValidationException"
            } else {
                "ResourceNotFoundException"
            },
            Some(if refused {
                format!(
                    "[Requested entity identifier '{identifier}' is invalid. \
                     It should match with {ENTITY_IDENTIFIER_PATTERN}.]"
                )
            } else {
                format!("Requested entity '{named}' does not exist.")
            }),
        ));
    }

    // `DescribeEntityRequest$EntityId` — an id at the bound is well formed, so it is
    // reported missing rather than refused.
    rows.push((
        format!("DescribeEntity / entityId at the bound ({ENTITY_MEMBER_MAX})"),
        "GET",
        format!(
            "/DescribeEntity?catalog={CATALOG}&entityId={}",
            entity_id(ENTITY_MEMBER_MAX)
        ),
        None,
        404,
        "ResourceNotFoundException",
        Some(format!(
            "Requested entity '{}' does not exist.",
            entity_id(ENTITY_MEMBER_MAX)
        )),
    ));

    // `DescribeAssessmentRequest$AssessmentIdentifier` — a member of the same kind with a
    // DIFFERENT modelled bound and two accepted forms, so the rule is the member's own
    // `@length` and not one constant.
    for (label, identifier, length) in [
        (
            "an id at the bound",
            assessment_id(ASSESSMENT_IDENTIFIER_MAX),
            ASSESSMENT_IDENTIFIER_MAX,
        ),
        (
            "an ARN at the bound",
            assessment_arn(ASSESSMENT_IDENTIFIER_MAX),
            ASSESSMENT_IDENTIFIER_MAX,
        ),
        (
            "an id one past the bound",
            assessment_id(ASSESSMENT_IDENTIFIER_MAX + 1),
            ASSESSMENT_IDENTIFIER_MAX + 1,
        ),
        (
            "an ARN one past the bound",
            assessment_arn(ASSESSMENT_IDENTIFIER_MAX + 1),
            ASSESSMENT_IDENTIFIER_MAX + 1,
        ),
        (
            "an id far past the bound",
            assessment_id(4096),
            4096,
        ),
    ] {
        assert_eq!(
            identifier.chars().count(),
            length,
            "case {label:?}: the row's identifier must be {length} characters long, got {}",
            identifier.chars().count()
        );
        let refused = length > ASSESSMENT_IDENTIFIER_MAX;
        let named = identifier
            .rsplit('/')
            .next()
            .unwrap_or(&identifier)
            .to_string();
        rows.push((
            format!("DescribeAssessment / {label} ({length})"),
            "POST",
            "/DescribeAssessment".to_string(),
            Some(json!({"Catalog": CATALOG, "AssessmentIdentifier": identifier.clone()})),
            if refused { 422 } else { 404 },
            if refused {
                "ValidationException"
            } else {
                "ResourceNotFoundException"
            },
            // A sentence quoting the whole identifier is longer than a `Message` may
            // be, so what the answer carries is that sentence cut to fit.
            Some(if refused {
                format!(
                    "[Requested assessment identifier '{identifier}' is invalid. \
                     It should match with {ASSESSMENT_IDENTIFIER_PATTERN}.]"
                )
            } else {
                format!("Requested assessment '{named}' does not exist.")
            }),
        ));
    }

    for (case, method, uri, body, status, code, expected) in rows {
        let answer = call(&router, method, &uri, body).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (status, code),
            "case {case:?}: a value past its member's modelled length is refused as \
             malformed and one inside it is judged by the rules behind the member, got \
             {} {:?} with {:?}",
            answer.status,
            answer.error_code(),
            answer.body
        );
        if let Some(expected) = expected {
            assert_message(&case, &answer, &expected);
        }
    }
}

/// Assert `answer` carries `sentence` — cut to fit when the whole sentence is longer than
/// a `Message` may be.
///
/// A sentence built around a submitted value can be longer than
/// [`EXCEPTION_MESSAGE_MAX`] for a value its own member admits, and no `Message` can
/// carry that: what the answer must carry is the same sentence, with the quoted value
/// cut, inside the bound.
fn assert_message(case: &str, answer: &Answer, sentence: &str) {
    let actual = answer.message();
    let length = actual.chars().count();
    assert!(
        (1..=EXCEPTION_MESSAGE_MAX).contains(&length),
        "case {case:?}: the Message must stay inside its modelled \
         {EXCEPTION_MESSAGE_MAX} characters, got {length}"
    );
    if sentence.chars().count() <= EXCEPTION_MESSAGE_MAX {
        assert_eq!(
            actual, sentence,
            "case {case:?}: the answer must carry the sentence built from the request, got \
             {actual:?}"
        );
        return;
    }
    // The cut falls at the quoted value, far past this prefix, so what the answer
    // carries begins with what the whole sentence begins with.
    let shared: String = actual.chars().take(512).collect();
    assert!(
        sentence.starts_with(&shared),
        "case {case:?}: the answer must carry that same sentence, cut to fit, got {actual:?}"
    );
}

/// RULE, on the reporting side: what a change set serves back is what it was started
/// with, and a value past the member's modelled length never gets that far — so every
/// `ChangeSummary$Entity$Type` a read reports is inside the `@length` of the shape that
/// carries it.
#[tokio::test]
async fn a_change_set_reports_no_entity_type_past_the_modelled_length() {
    let router = emulator();

    // The longest reference the member can hold round-trips verbatim.
    let accepted = entity_type_reference("AmiProduct", ENTITY_MEMBER_MAX);
    let started = call(
        &router,
        "POST",
        "/StartChangeSet",
        Some(start_change_set_body(json!({"Type": accepted.clone()}))),
    )
    .await;
    assert_eq!(
        (started.status, started.error_code()),
        (200, ""),
        "a reference at the modelled bound must be accepted, got {} {:?} with {:?}",
        started.status,
        started.error_code(),
        started.body
    );
    let change_set_id = started.text("/ChangeSetId");

    let described = call(
        &router,
        "GET",
        &format!("/DescribeChangeSet?catalog={CATALOG}&changeSetId={change_set_id}"),
        None,
    )
    .await;
    assert_eq!(
        described.status, 200,
        "the started change set must be readable, got {} with {:?}",
        described.status, described.body
    );
    let reported = described.text("/ChangeSet/0/Entity/Type");
    assert_eq!(
        reported, accepted,
        "DescribeChangeSet must report the reference the change set was started with, got \
         {reported:?}"
    );
    assert!(
        reported.chars().count() <= ENTITY_MEMBER_MAX,
        "case {change_set_id:?}: a reported Entity.Type must be within the modelled {} \
         characters, got {} characters",
        ENTITY_MEMBER_MAX,
        reported.chars().count()
    );

    // One character more is refused, so no change set carrying it exists to report it.
    let refused_type = entity_type_reference("AmiProduct", ENTITY_MEMBER_MAX + 1);
    let refused = call(
        &router,
        "POST",
        "/StartChangeSet",
        Some(start_change_set_body(json!({"Type": refused_type.clone()}))),
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (422, "ValidationException"),
        "a reference past the modelled bound must be refused, got {} {:?} with {:?}",
        refused.status,
        refused.error_code(),
        refused.body
    );
    let listed = call(
        &router,
        "POST",
        "/ListChangeSets",
        Some(json!({"Catalog": CATALOG})),
    )
    .await;
    let summaries = listed
        .body
        .pointer("/ChangeSetSummaryList")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        summaries.len(),
        1,
        "only the accepted change set may exist, got {summaries:?}"
    );
}

/// RULE, per batch entry: a batch read judges each `EntityRequest$EntityId` itself, so an
/// id past the modelled length is reported as the malformed id it is, under the id that
/// was asked for, beside the entries next to it — and the sentence it carries stays inside
/// the length `BatchDescribeErrorMessageContent` declares however long that id was.
#[tokio::test]
async fn a_batch_read_bounds_each_entity_id_by_its_modelled_length() {
    let router = emulator();

    let inside = entity_id(ENTITY_MEMBER_MAX);
    let past = entity_id(ENTITY_MEMBER_MAX + 1);
    let far_past = entity_id(1024);
    let answer = call(
        &router,
        "POST",
        "/BatchDescribeEntities",
        Some(json!({
            "EntityRequestList": [
                {"Catalog": CATALOG, "EntityId": inside},
                {"Catalog": CATALOG, "EntityId": past},
                {"Catalog": CATALOG, "EntityId": far_past},
            ]
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "a batch read answers 200 whatever its entries resolve to, got {} with {:?}",
        answer.status, answer.body
    );

    // (the id asked for, the error code its entry must carry, its message)
    let expected = [
        (
            &inside,
            "ResourceNotFoundException",
            format!("Requested entity '{inside}' does not exist."),
        ),
        (
            &past,
            "ValidationException",
            format!(
                "Requested entity id '{past}' is invalid. It should match with \
                 {ENTITY_ID_PATTERN}."
            ),
        ),
        (
            &far_past,
            "ValidationException",
            format!(
                "Requested entity id '{far_past}' is invalid. It should match with \
                 {ENTITY_ID_PATTERN}."
            ),
        ),
    ];
    for (asked, code, message) in expected {
        let case = format!("entry of {} characters", asked.chars().count());
        let reported_code = answer.text(&format!("/Errors/{asked}/ErrorCode"));
        assert_eq!(
            reported_code, code,
            "case {case:?}: the entry must carry the code a single read of it would raise, \
             got {reported_code:?} in {:?}",
            answer.body
        );
        let reported_message = answer.text(&format!("/Errors/{asked}/ErrorMessage"));
        let length = reported_message.chars().count();
        assert!(
            (1..=EXCEPTION_MESSAGE_MAX).contains(&length),
            "case {case:?}: the entry's ErrorMessage must stay inside its modelled \
             {EXCEPTION_MESSAGE_MAX} characters, got {length}"
        );
        assert_eq!(
            reported_message, message,
            "case {case:?}: the entry's message must be built from the request, got \
             {reported_message:?}"
        );
    }
}
