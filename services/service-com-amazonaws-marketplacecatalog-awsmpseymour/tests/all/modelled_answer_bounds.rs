// SPDX-License-Identifier: CC0-1.0

//! How long a fault this service builds around a SUBMITTED value may be, end to end over
//! HTTP.
//!
//! RULE: a message the emulator reports satisfies the modelled `@length` of the member
//! that carries it, so a sentence built around a submitted value quotes that value cut to
//! the room the sentence leaves inside that bound — the whole sentence always reported,
//! the value as far as it fits. The members that carry one are
//! `ExceptionMessageContent` for every modelled error's `Message` and
//! `BatchDescribeErrorMessageContent` for a batch entry's `ErrorMessage`
//! (`smithy_query(query="shape",
//! shape="ExceptionMessageContent,BatchDescribeErrorMessageContent")`: both
//! `@length(min: 1, max: 2048)`).
//!
//! The rule is the one this crate already states for the other values it quotes back (a
//! submitted details value in an `ErrorDetail$ErrorMessage`, a submitted scope value in a
//! `ControlError$Scope` entry); what it reaches here is every member whose boundary
//! validation this service replaces with a predicate of its own — those arrive at
//! whatever length they were sent — and `AssessmentIdentifier`, which its own
//! `@length(max: 2048)` lets fill a whole message on its own.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! The rows differ along what the rule generalises over: which member carried the value
//! (nine of them, across six operations), which operation carried that member, whether
//! the fault is a top-level `Message` or one entry of a batch answer, and how far past
//! the bound the submitted value is (one character, twice it, ten times it).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The catalog every operation's `Catalog` member documents as its fixed value.
const CATALOG: &str = "AWSMarketplace";

/// A SigV4 credential scope naming this service's signing name (`aws-marketplace`), which
/// is what routes a request here.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// How long a message may be: the `@length(min: 1, max: 2048)` of
/// `ExceptionMessageContent` and of `BatchDescribeErrorMessageContent`. Written out here
/// rather than read from the crate so an implementation quoting its own bound cannot make
/// this suite pass.
const MESSAGE_MAX: usize = 2048;

/// How much of the submitted value a fault must still name, however much of it the
/// message had room for: enough to see WHICH value was refused.
const NAMED_HEAD: usize = 255;

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

/// The start of `value` a fault has to name whatever else it does.
fn head(value: &str) -> String {
    value.chars().take(NAMED_HEAD).collect()
}

/// Assert `message` is a fault this service may report for `value`: inside the modelled
/// length of the member carrying it, non-empty, and still naming the value it refused.
fn assert_inside_the_message_bound(case: &str, message: &str, value: &str) {
    let length = message.chars().count();
    assert!(
        (1..=MESSAGE_MAX).contains(&length),
        "case {case:?}: the message must stay inside its modelled {MESSAGE_MAX} \
         characters, got {length}"
    );
    assert!(
        message.contains(&head(value)),
        "case {case:?}: the message must still name the value it refused, got {message:?}"
    );
}

/// One `StartChangeSet` change, with `members` merged over a change the catalog accepts —
/// so a row that malforms ONE member is refused for that member and nothing else.
fn change(members: Value) -> Value {
    let mut change = json!({
        "ChangeType": "CreateProduct",
        "Entity": {"Type": "AmiProduct@1.0"},
        "Details": json!({"ProductTitle": "modelled-answer-bounds"}).to_string(),
    });
    if let (Value::Object(target), Value::Object(extra)) = (&mut change, &members) {
        for (member, carried) in extra {
            target.insert(member.clone(), carried.clone());
        }
    }
    change
}

/// Every call that carries `value` in a member this service judges with a predicate of
/// its own — so the whole submitted value reaches the fault that quotes it — as
/// (member, method, uri, body).
fn faults_quoting(value: &str) -> Vec<(&'static str, &'static str, String, Option<Value>)> {
    vec![
        (
            "DescribeEntity EntityId",
            "GET",
            format!("/DescribeEntity?catalog={CATALOG}&entityId={value}"),
            None,
        ),
        (
            "ListEntities Catalog",
            "POST",
            "/ListEntities".to_string(),
            Some(json!({"Catalog": value, "EntityType": "AmiProduct"})),
        ),
        (
            "ListEntities EntityType",
            "POST",
            "/ListEntities".to_string(),
            Some(json!({"Catalog": CATALOG, "EntityType": value})),
        ),
        (
            "StartChangeSet ChangeSetName",
            "POST",
            "/StartChangeSet".to_string(),
            Some(json!({
                "Catalog": CATALOG,
                "ChangeSet": [change(json!({}))],
                "ChangeSetName": value,
            })),
        ),
        (
            "StartChangeSet ClientRequestToken",
            "POST",
            "/StartChangeSet".to_string(),
            Some(json!({
                "Catalog": CATALOG,
                "ChangeSet": [change(json!({}))],
                "ClientRequestToken": value,
            })),
        ),
        (
            "StartChangeSet ChangeSet.Entity.Type",
            "POST",
            "/StartChangeSet".to_string(),
            Some(json!({
                "Catalog": CATALOG,
                "ChangeSet": [change(json!({"Entity": {"Type": value}}))],
            })),
        ),
        (
            "StartChangeSet ChangeSet.Entity.Identifier",
            "POST",
            "/StartChangeSet".to_string(),
            Some(json!({
                "Catalog": CATALOG,
                "ChangeSet": [change(json!({
                    "ChangeType": "UpdateInformation",
                    "Entity": {"Type": "AmiProduct@1.0", "Identifier": value},
                }))],
            })),
        ),
        (
            "StartChangeSet ChangeSet.ChangeName",
            "POST",
            "/StartChangeSet".to_string(),
            Some(json!({
                "Catalog": CATALOG,
                "ChangeSet": [change(json!({"ChangeName": value}))],
            })),
        ),
        (
            "DescribeAssessment AssessmentIdentifier",
            "POST",
            "/DescribeAssessment".to_string(),
            Some(json!({"Catalog": CATALOG, "AssessmentIdentifier": value})),
        ),
        (
            "StartChangeSet ChangeSet.ChangeType",
            "POST",
            "/StartChangeSet".to_string(),
            Some(json!({
                "Catalog": CATALOG,
                "ChangeSet": [change(json!({"ChangeType": value}))],
            })),
        ),
        (
            "ListEntities Sort.SortBy",
            "POST",
            "/ListEntities".to_string(),
            Some(json!({
                "Catalog": CATALOG,
                "EntityType": "AmiProduct",
                "Sort": {"SortBy": value},
            })),
        ),
        (
            "ListTagsForResource ResourceArn",
            "POST",
            "/ListTagsForResource".to_string(),
            Some(json!({"ResourceArn": value})),
        ),
    ]
}

/// RULE: a top-level fault that quotes a submitted value stays inside the length its
/// `Message` declares, and still names the value it refused — for every member whose
/// published syntax this service judges itself, so no value a caller can send takes the
/// fault out of contract.
#[tokio::test]
async fn a_top_level_fault_quoting_a_submitted_value_stays_inside_its_message_length() {
    let router = emulator();

    for size in [MESSAGE_MAX + 1, MESSAGE_MAX * 2, MESSAGE_MAX * 10] {
        let value = "a".repeat(size);
        for (member, method, uri, body) in faults_quoting(&value) {
            let case = format!("{member} of {size} characters");
            let answer = call(&router, method, &uri, body).await;
            assert_eq!(
                (answer.status, answer.error_code()),
                (422, "ValidationException"),
                "case {case:?}: a value outside the published syntax must be the modelled \
                 422, got {} {:?} with {:?}",
                answer.status,
                answer.error_code(),
                answer.body
            );
            assert_inside_the_message_bound(&case, &answer.message(), &value);
        }
    }
}

/// RULE, for the fault a batch read reports per entry: a `BatchDescribeErrorDetail`
/// quotes the entry's own submitted id, so that `ErrorMessage` is held to the same bound
/// as a top-level `Message` — the call still answering 200 and reporting the entry under
/// the id that was asked for.
///
/// The rows differ along the size of the submitted id, and the last one carries a second
/// entry beside it, so the bound is asserted on an entry of a mixed batch too.
#[tokio::test]
async fn a_batch_entry_quoting_its_submitted_id_stays_inside_its_message_length() {
    let router = emulator();
    /// A well-formed id no change set created, for the entry beside the long one.
    const MISSING: &str = "prod-answerbounds01";

    for (size, beside) in [
        (MESSAGE_MAX + 1, false),
        (MESSAGE_MAX * 2, false),
        (MESSAGE_MAX * 10, true),
    ] {
        let entity_id = "a".repeat(size);
        let case = format!("an entry of {size} characters");
        let mut entries = vec![json!({"Catalog": CATALOG, "EntityId": entity_id})];
        if beside {
            entries.push(json!({"Catalog": CATALOG, "EntityId": MISSING}));
        }
        let answer = call(
            &router,
            "POST",
            "/BatchDescribeEntities",
            Some(json!({"EntityRequestList": entries})),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a batch read answers 200 whatever its entries resolve to, got \
             {} with {:?}",
            answer.status, answer.body
        );
        let entry = format!("/Errors/{entity_id}");
        assert_eq!(
            answer.text(&format!("{entry}/ErrorCode")),
            "ValidationException",
            "case {case:?}: an id past the modelled length is reported as the malformed \
             id it is, under the id that was asked for, got {:?}",
            answer.body
        );
        assert_inside_the_message_bound(
            &case,
            &answer.text(&format!("{entry}/ErrorMessage")),
            &entity_id,
        );
        if beside {
            assert_eq!(
                answer.text(&format!("/Errors/{MISSING}/ErrorCode")),
                "ResourceNotFoundException",
                "case {case:?}: the entry beside it must still be reported on its own \
                 terms, got {:?}",
                answer.body
            );
        }
    }
}
