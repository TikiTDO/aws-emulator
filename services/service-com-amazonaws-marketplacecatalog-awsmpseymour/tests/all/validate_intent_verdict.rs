// SPDX-License-Identifier: CC0-1.0

//! What `Intent` decides about a change set, end to end over HTTP.
//!
//! RULE: `Intent` decides whether the changes of a change set are APPLIED to the catalog,
//! never how the change set is JUDGED — `StartChangeSetRequest$Intent` is documented "The
//! default is APPLY. To test your request before applying changes to your entities, use
//! VALIDATE" (`smithy_query(query="doc", shape="StartChangeSetRequest", member="Intent")`)
//! — so validating a request reaches the same verdict applying the same request reaches:
//! the same `StartChangeSet` refusal, or the same terminal `Status`, `FailureCode`,
//! `FailureDescription` and per-change `ErrorDetailList` at the end of the run. The one
//! thing the intent does decide is the WRITE: a validation leaves the catalog's entities
//! exactly as it found them.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rows differ along what the rule generalises over: which rule decides
//! the verdict (the `StartChangeSet` refusal of a repeated entity/change-type pair, the
//! self-contradiction of the set, the per-entity tag ceiling, the per-entity details
//! ceiling, a details member the catalog cannot read, and a request with nothing wrong with
//! it), how two changes reach one entity (a bare entity id, an id at a revision, a
//! reference to another change's entity), whether that entity already exists or the change
//! set creates it, how many changes the set carries, and which change types and entity
//! types they use.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::state::{
    CHANGE_SET_APPLYING, CHANGE_SET_PREPARING, DETAILS_JSON_LIMIT, MAX_RESOURCE_TAGS,
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

/// The two `Intent` values, which is what every row is run under.
///
/// Source: `Intent` is an enum of exactly these (`smithy_query(query="shape",
/// shape="Intent")`).
const INTENTS: [&str; 2] = ["APPLY", "VALIDATE"];

/// The entity types the rows act on, which is also every type `product_entity_ids` counts
/// the catalog's entities over.
///
/// Source: the five product summaries `EntitySummary` carries
/// (`smithy_query(query="shape", shape="EntitySummary")`).
const PRODUCT_TYPES: [&str; 5] = [
    "AmiProduct",
    "SaaSProduct",
    "DataProduct",
    "ContainerProduct",
    "MachineLearningProduct",
];

/// How long after the start a change set has certainly ended its run: the two spans the
/// implementation publishes, plus a margin for a slow machine.
fn wait_for_the_end_of_the_run() {
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
    fn error_code(&self) -> String {
        let raw = self.body["__type"].as_str().unwrap_or_default();
        raw.rsplit('#').next().unwrap_or(raw).to_string()
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

    /// The array at `pointer`, or an empty one.
    fn list(&self, pointer: &str) -> Vec<Value> {
        self.body
            .pointer(pointer)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
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

/// `POST /StartChangeSet` with `changes` as the whole `ChangeSet`, under `intent`.
async fn start(router: &MultiServiceRouter, intent: &str, changes: Value) -> Answer {
    call(
        router,
        "POST",
        "/StartChangeSet",
        Some(json!({"Catalog": CATALOG, "Intent": intent, "ChangeSet": changes})),
    )
    .await
}

async fn describe_change_set(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/DescribeChangeSet?catalog={CATALOG}&changeSetId={id}"),
        None,
    )
    .await
}

/// Every entity id the catalog holds across [`PRODUCT_TYPES`], sorted — which is what a
/// validation may not change.
async fn product_entity_ids(router: &MultiServiceRouter) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for entity_type in PRODUCT_TYPES {
        let listed = call(
            router,
            "POST",
            "/ListEntities",
            Some(json!({"Catalog": CATALOG, "EntityType": entity_type})),
        )
        .await;
        for summary in listed.list("/EntitySummaryList") {
            ids.push(
                summary["EntityId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
    }
    ids.sort();
    ids
}

/// One change: a change type, the entity type it acts under, the identifier it names (none
/// for a change that creates an entity of its own), the details it submits, and the
/// `ChangeName` the other changes may name it by.
fn change(
    change_type: &str,
    entity_type: &str,
    identifier: Option<&str>,
    details: Value,
    change_name: Option<&str>,
) -> Value {
    let mut entity = json!({"Type": format!("{entity_type}@1.0")});
    if let Some(identifier) = identifier {
        entity["Identifier"] = json!(identifier);
    }
    let mut built = json!({
        "ChangeType": change_type,
        "Entity": entity,
        "DetailsDocument": details,
    });
    if let Some(name) = change_name {
        built["ChangeName"] = json!(name);
    }
    built
}

/// `MAX_RESOURCE_TAGS` tags, which is also `TagList`'s own `@length(max: 200)`, so one
/// change can carry exactly this many.
fn tags_filling_the_ceiling() -> Value {
    json!((0..MAX_RESOURCE_TAGS)
        .map(|index| json!({"Key": format!("key{index}"), "Value": "filling"}))
        .collect::<Vec<Value>>())
}

/// A details member whose value is long enough that two such changes merge past
/// `DETAILS_JSON_LIMIT` while each stays inside it on its own.
fn half_the_details_ceiling(member: &str) -> Value {
    json!({member: "f".repeat(DETAILS_JSON_LIMIT * 2 / 3)})
}

/// A message with every entity id in it replaced by a placeholder.
///
/// The entity a change acts on is named in the refusals and failures the catalog reports,
/// and the id of an entity a change set CREATES is minted per change set — so two runs of
/// one request name two different new entities while saying the same thing about them.
fn without_entity_ids(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut rest = message;
    while let Some(at) = rest.find("ent-") {
        out.push_str(&rest[..at]);
        out.push_str("<entity>");
        rest = &rest[at + "ent-".len()..];
        let tail = rest
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(rest.len());
        rest = &rest[tail..];
    }
    out.push_str(rest);
    out
}

/// Everything the emulator reported about one change set, with the entity ids taken out of
/// the messages: the `StartChangeSet` answer, and — when the change set was started — the
/// terminal state of its run.
#[derive(Debug, PartialEq, Eq)]
struct Verdict {
    start_status: u16,
    error_code: String,
    message: String,
    status: String,
    failure_code: String,
    failure_description: String,
    /// The `ErrorDetail` of each change, by change index: its code and its message.
    change_errors: Vec<Vec<(String, String)>>,
}

/// Run `changes` under `intent` on a catalog of its own and report the verdict, together
/// with the entity ids the catalog holds afterwards.
///
/// `existing` asks for one entity to be created and applied first, which the row's change
/// set then acts on.
async fn verdict_of(
    case: &str,
    intent: &str,
    existing: bool,
    build: fn(&str) -> Value,
) -> (Verdict, Vec<String>, Vec<String>) {
    let router = emulator();
    let mut fixture = String::new();
    if existing {
        let started = start(
            &router,
            "APPLY",
            json!([change(
                "CreateProduct",
                "AmiProduct",
                None,
                json!({"ProductTitle": "the entity the case acts on"}),
                None,
            )]),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {case:?} under {intent}: the fixture's create must answer 200, got {} \
             with {:?}",
            started.status, started.body
        );
        wait_for_the_end_of_the_run();
        fixture = product_entity_ids(&router)
            .await
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the fixture's entity must exist"));
    }
    let before = product_entity_ids(&router).await;

    let started = start(&router, intent, build(&fixture)).await;
    let mut verdict = Verdict {
        start_status: started.status,
        error_code: started.error_code(),
        message: without_entity_ids(&started.message()),
        status: String::new(),
        failure_code: String::new(),
        failure_description: String::new(),
        change_errors: Vec::new(),
    };
    if started.status != 200 {
        let after = product_entity_ids(&router).await;
        return (verdict, before, after);
    }

    wait_for_the_end_of_the_run();
    let described = describe_change_set(&router, &started.text("/ChangeSetId")).await;
    assert_eq!(
        described.status, 200,
        "case {case:?} under {intent}: DescribeChangeSet must answer 200, got {} with {:?}",
        described.status, described.body
    );
    verdict.status = described.text("/Status");
    verdict.failure_code = described.text("/FailureCode");
    verdict.failure_description = without_entity_ids(&described.text("/FailureDescription"));
    verdict.change_errors = described
        .list("/ChangeSet")
        .iter()
        .map(|summary| {
            summary["ErrorDetailList"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|detail| {
                    (
                        detail["ErrorCode"].as_str().unwrap_or_default().to_string(),
                        without_entity_ids(detail["ErrorMessage"].as_str().unwrap_or_default()),
                    )
                })
                .collect()
        })
        .collect();
    let after = product_entity_ids(&router).await;
    (verdict, before, after)
}

/// What a row expects of the verdict both intents must reach.
enum Expect {
    /// `StartChangeSet` refuses the request: this status and error code, with a message
    /// saying this.
    Refused {
        status: u16,
        code: &'static str,
        message_says: &'static str,
    },
    /// The change set is started and its run ends `FAILED` with `CLIENT_ERROR`;
    /// `description` says whether the failure belongs to the set rather than to one of its
    /// changes, and `errors` gives the error code each listed change reports — every change
    /// not listed reporting none.
    Failed {
        description: bool,
        errors: &'static [(usize, &'static str)],
    },
    /// The change set is started and its run ends `SUCCEEDED`, reporting no failure at all.
    Succeeded,
}

/// One row: the case, whether it needs an entity to exist first, how it builds its change
/// set from that entity's id, and the verdict both intents must reach.
struct Row {
    case: &'static str,
    existing: bool,
    build: fn(&str) -> Value,
    expect: Expect,
}

/// RULE: a change set is judged by its CONTENT, so `Intent=VALIDATE` reaches the verdict
/// `Intent=APPLY` reaches for the same request — the same refusal, or the same terminal
/// status, failure and per-change errors — and the only difference is that the validation
/// writes no entity.
#[tokio::test]
async fn a_validation_reaches_the_verdict_applying_the_same_request_reaches() {
    let rows: Vec<Row> = vec![
        Row {
            case: "a repeated change type reaching the entity the set creates, through a \
                   reference",
            existing: false,
            build: |_| {
                json!([
                    change(
                        "CreateProduct",
                        "AmiProduct",
                        None,
                        json!({"ProductTitle": "one"}),
                        Some("A"),
                    ),
                    change(
                        "CreateProduct",
                        "AmiProduct",
                        Some("$A.Entity.Identifier"),
                        json!({"ProductTitle": "one"}),
                        None,
                    ),
                ])
            },
            expect: Expect::Refused {
                status: 422,
                code: "ValidationException",
                message_says: "multiple changes for the same change type",
            },
        },
        Row {
            case: "a repeated change type reaching the entity the set creates, through two \
                   references",
            existing: false,
            build: |_| {
                json!([
                    change(
                        "CreateProduct",
                        "SaaSProduct",
                        None,
                        json!({"ProductTitle": "one"}),
                        Some("A"),
                    ),
                    change(
                        "AddRevisions",
                        "SaaSProduct",
                        Some("$A.Entity.Identifier"),
                        json!({"Support": "email"}),
                        Some("B"),
                    ),
                    change(
                        "AddRevisions",
                        "SaaSProduct",
                        Some("$B.Entity.Identifier"),
                        json!({"Support": "phone"}),
                        None,
                    ),
                ])
            },
            expect: Expect::Refused {
                status: 422,
                code: "ValidationException",
                message_says: "multiple changes for the same change type",
            },
        },
        Row {
            case: "a repeated change type reaching an existing entity, one change naming it \
                   and the other naming that change",
            existing: true,
            build: |entity| {
                json!([
                    change(
                        "AddRevisions",
                        "AmiProduct",
                        Some(entity),
                        json!({"Support": "email"}),
                        Some("A"),
                    ),
                    change(
                        "AddRevisions",
                        "AmiProduct",
                        Some("$A.Entity.Identifier"),
                        json!({"Support": "phone"}),
                        None,
                    ),
                ])
            },
            expect: Expect::Refused {
                status: 422,
                code: "ValidationException",
                message_says: "multiple changes for the same change type",
            },
        },
        Row {
            case: "a repeated change type naming an existing entity twice, once at a revision",
            existing: true,
            build: |entity| {
                json!([
                    change(
                        "UpdateInformation",
                        "AmiProduct",
                        Some(entity),
                        json!({"Support": "email"}),
                        None,
                    ),
                    change(
                        "UpdateInformation",
                        "AmiProduct",
                        Some(&format!("{entity}@1")),
                        json!({"Support": "phone"}),
                        None,
                    ),
                ])
            },
            expect: Expect::Refused {
                status: 422,
                code: "ValidationException",
                message_says: "multiple changes for the same change type",
            },
        },
        Row {
            case: "two changes claiming a different ProductTitle for the entity the set creates",
            existing: false,
            build: |_| {
                json!([
                    change(
                        "CreateProduct",
                        "AmiProduct",
                        None,
                        json!({"ProductTitle": "one"}),
                        Some("A"),
                    ),
                    change(
                        "AddRevisions",
                        "AmiProduct",
                        Some("$A.Entity.Identifier"),
                        json!({"ProductTitle": "two"}),
                        None,
                    ),
                ])
            },
            expect: Expect::Failed {
                description: true,
                errors: &[],
            },
        },
        Row {
            case: "two changes claiming a different Visibility for the entity the set creates",
            existing: false,
            build: |_| {
                json!([
                    change(
                        "CreateProduct",
                        "DataProduct",
                        None,
                        json!({"Visibility": "Draft"}),
                        Some("A"),
                    ),
                    change(
                        "UpdateInformation",
                        "DataProduct",
                        Some("$A.Entity.Identifier"),
                        json!({"Visibility": "Public"}),
                        None,
                    ),
                ])
            },
            expect: Expect::Failed {
                description: true,
                errors: &[],
            },
        },
        Row {
            case: "two changes claiming a different ProductTitle for an existing entity",
            existing: true,
            build: |entity| {
                json!([
                    change(
                        "AddRevisions",
                        "AmiProduct",
                        Some(entity),
                        json!({"ProductTitle": "one"}),
                        None,
                    ),
                    change(
                        "UpdateInformation",
                        "AmiProduct",
                        Some(entity),
                        json!({"ProductTitle": "two"}),
                        None,
                    ),
                ])
            },
            expect: Expect::Failed {
                description: true,
                errors: &[],
            },
        },
        Row {
            case: "two changes claiming a different Name for an existing entity",
            existing: true,
            build: |entity| {
                json!([
                    change(
                        "AddDeliveryOptions",
                        "AmiProduct",
                        Some(entity),
                        json!({"Name": "one"}),
                        None,
                    ),
                    change(
                        "ReleaseProduct",
                        "AmiProduct",
                        Some(&format!("{entity}@1")),
                        json!({"Name": "two"}),
                        None,
                    ),
                ])
            },
            expect: Expect::Failed {
                description: true,
                errors: &[],
            },
        },
        Row {
            case: "the tag ceiling crossed on the entity the set creates",
            existing: false,
            build: |_| {
                let mut creating = change(
                    "CreateProduct",
                    "ContainerProduct",
                    None,
                    json!({"ProductTitle": "tagged"}),
                    Some("A"),
                );
                creating["EntityTags"] = tags_filling_the_ceiling();
                let mut tagging = change(
                    "AddRevisions",
                    "ContainerProduct",
                    Some("$A.Entity.Identifier"),
                    json!({"Support": "email"}),
                    None,
                );
                tagging["EntityTags"] = json!([{"Key": "oneTooMany", "Value": "over"}]);
                json!([creating, tagging])
            },
            expect: Expect::Failed {
                description: false,
                errors: &[(1, "LIMIT_EXCEEDED")],
            },
        },
        Row {
            case: "the details ceiling crossed on the entity the set creates",
            existing: false,
            build: |_| {
                json!([
                    change(
                        "CreateProduct",
                        "MachineLearningProduct",
                        None,
                        half_the_details_ceiling("First"),
                        Some("A"),
                    ),
                    change(
                        "AddRevisions",
                        "MachineLearningProduct",
                        Some("$A.Entity.Identifier"),
                        half_the_details_ceiling("Second"),
                        None,
                    ),
                ])
            },
            expect: Expect::Failed {
                description: false,
                errors: &[(1, "LIMIT_EXCEEDED")],
            },
        },
        Row {
            case: "a details member the catalog cannot read, on the entity the set creates",
            existing: false,
            build: |_| {
                json!([change(
                    "CreateProduct",
                    "AmiProduct",
                    None,
                    json!({"ProductTitle": 7}),
                    None,
                )])
            },
            expect: Expect::Failed {
                description: false,
                errors: &[(0, "INVALID_VALUE")],
            },
        },
        Row {
            case: "one change creating an entity, with nothing wrong with it",
            existing: false,
            build: |_| {
                json!([change(
                    "CreateProduct",
                    "AmiProduct",
                    None,
                    json!({"ProductTitle": "sound"}),
                    None,
                )])
            },
            expect: Expect::Succeeded,
        },
        Row {
            case: "two change types reaching the entity the set creates, agreeing on its title",
            existing: false,
            build: |_| {
                json!([
                    change(
                        "CreateProduct",
                        "SaaSProduct",
                        None,
                        json!({"ProductTitle": "agreed"}),
                        Some("A"),
                    ),
                    change(
                        "AddRevisions",
                        "SaaSProduct",
                        Some("$A.Entity.Identifier"),
                        json!({"ProductTitle": "agreed", "Support": "email"}),
                        None,
                    ),
                ])
            },
            expect: Expect::Succeeded,
        },
        Row {
            case: "two creates, each making an entity of its own under the same title",
            existing: false,
            build: |_| {
                json!([
                    change(
                        "CreateProduct",
                        "AmiProduct",
                        None,
                        json!({"ProductTitle": "same"}),
                        None,
                    ),
                    change(
                        "CreateProduct",
                        "DataProduct",
                        None,
                        json!({"ProductTitle": "same"}),
                        None,
                    ),
                ])
            },
            expect: Expect::Succeeded,
        },
        Row {
            case: "one change against an existing entity, with nothing wrong with it",
            existing: true,
            build: |entity| {
                json!([change(
                    "AddRevisions",
                    "AmiProduct",
                    Some(entity),
                    json!({"Support": "email"}),
                    None,
                )])
            },
            expect: Expect::Succeeded,
        },
    ];

    for row in &rows {
        // Every row is run once per `Intent`, each on a catalog of its own, and the
        // verdict is read the same way whichever intent produced it.
        let mut runs: Vec<(&str, Verdict, Vec<String>, Vec<String>)> = Vec::new();
        for intent in INTENTS {
            let (verdict, before, after) =
                verdict_of(row.case, intent, row.existing, row.build).await;
            assert_expected(row.case, intent, &verdict, &row.expect);
            runs.push((intent, verdict, before, after));
        }

        let (first_intent, first_verdict, _, _) = &runs[0];
        for (intent, verdict, _, _) in &runs[1..] {
            assert_eq!(
                verdict, first_verdict,
                "case {case:?}: {intent} must reach the verdict {first_intent} reaches for \
                 the same request, got {verdict:?} against {first_verdict:?}",
                case = row.case,
            );
        }

        // The one difference the intent does make: a validation writes no entity, while
        // applying a request the catalog can apply writes the ones its changes create.
        for (intent, _, before, after) in &runs {
            if *intent == "VALIDATE" {
                assert_eq!(
                    after, before,
                    "case {case:?}: a validation must leave the catalog's entities as it \
                     found them, held {before:?} and then {after:?}",
                    case = row.case
                );
            } else if matches!(row.expect, Expect::Succeeded) && !row.existing {
                assert!(
                    after.len() > before.len(),
                    "case {case:?}: applying a request the catalog can apply must write the \
                     entities its changes create, held {before:?} and then {after:?}",
                    case = row.case
                );
            }
        }
    }
}

/// Assert one verdict is the one the row expects, so the test pins the verdict itself and
/// not only that the intents agree on it.
fn assert_expected(case: &str, intent: &str, verdict: &Verdict, expect: &Expect) {
    let case = format!("{case} under {intent}");
    let case = case.as_str();
    match expect {
        Expect::Refused {
            status,
            code,
            message_says,
        } => {
            assert_eq!(
                (verdict.start_status, verdict.error_code.as_str()),
                (*status, *code),
                "case {case:?}: expected StartChangeSet to refuse {status} {code}, got {verdict:?}"
            );
            assert!(
                verdict.message.contains(message_says),
                "case {case:?}: expected a refusal saying {message_says:?}, got {:?}",
                verdict.message
            );
        }
        Expect::Failed {
            description,
            errors,
        } => {
            assert_eq!(
                (
                    verdict.start_status,
                    verdict.status.as_str(),
                    verdict.failure_code.as_str()
                ),
                (200, "FAILED", "CLIENT_ERROR"),
                "case {case:?}: expected a started change set that ends FAILED with \
                 CLIENT_ERROR, got {verdict:?}"
            );
            assert_eq!(
                !verdict.failure_description.is_empty(),
                *description,
                "case {case:?}: expected FailureDescription present={description}, got {:?}",
                verdict.failure_description
            );
            for (index, held) in verdict.change_errors.iter().enumerate() {
                let expected: Vec<&str> = errors
                    .iter()
                    .filter(|(at, _)| *at == index)
                    .map(|(_, code)| *code)
                    .collect();
                let actual: Vec<&str> = held.iter().map(|(code, _)| code.as_str()).collect();
                assert_eq!(
                    actual, expected,
                    "case {case:?}: expected change {index} to report {expected:?}, got {held:?}"
                );
                for (_, message) in held {
                    assert!(
                        !message.is_empty() && message.chars().count() <= 2048,
                        "case {case:?}: every reported error must carry a message inside \
                         ExceptionMessageContent's 2048 characters, got {message:?}"
                    );
                }
            }
            assert!(
                errors
                    .iter()
                    .all(|(at, _)| *at < verdict.change_errors.len()),
                "case {case:?}: the row expects an error on a change the change set does not \
                 report, got {verdict:?}"
            );
        }
        Expect::Succeeded => {
            assert_eq!(
                (
                    verdict.start_status,
                    verdict.status.as_str(),
                    verdict.failure_code.as_str(),
                    verdict.failure_description.as_str()
                ),
                (200, "SUCCEEDED", "", ""),
                "case {case:?}: expected a started change set that ends SUCCEEDED with no \
                 failure, got {verdict:?}"
            );
            for (index, held) in verdict.change_errors.iter().enumerate() {
                assert!(
                    held.is_empty(),
                    "case {case:?}: change {index} of a succeeded change set reports no \
                     error, got {held:?}"
                );
            }
        }
    }
}
