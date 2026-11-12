// SPDX-License-Identifier: CC0-1.0

//! The length of a details value, end to end over HTTP.
//!
//! The rule under test: `Json` — the shape of every member that carries a details value
//! as a string, on the way in (`Change$Details`) and on the way out
//! (`DescribeEntityResponse$Details`, `ChangeSummary$Details`) — is
//! `@length(min: 2, max: 16384)`, and `JsonDocumentType` carries the SAME details with no
//! bound of its own. So a details value is judged against that length wherever it is
//! ACCEPTED, whichever member carried it: a change submitting more is refused, and a
//! change set whose merge would leave an entity reporting more fails at the end of its
//! run and applies nothing. What a reader is handed is therefore always inside the length
//! its own modelled shape declares.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::state::{
    CHANGE_SET_APPLYING, CHANGE_SET_PREPARING, DETAILS_JSON_LIMIT,
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

/// `Json`'s modelled `@length(min)`: the shortest details value is the empty object.
const DETAILS_JSON_MINIMUM: usize = 2;

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
    fn error_code(&self) -> &str {
        let raw = self.body["__type"].as_str().unwrap_or_default();
        raw.rsplit('#').next().unwrap_or(raw)
    }

    /// The fault's sentence, whichever casing the answer spells the member with: the
    /// modelled `ValidationException$Message`, or the `message` the centralized boundary
    /// walk answers a modelled constraint with.
    fn message(&self) -> String {
        for pointer in ["/Message", "/message"] {
            if let Some(text) = self.body.pointer(pointer).and_then(Value::as_str) {
                return text.to_string();
            }
        }
        String::new()
    }

    /// The string at `pointer`, or the empty string.
    fn text(&self, pointer: &str) -> String {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
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

async fn start_change_set(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/StartChangeSet", Some(request)).await
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

async fn describe_entity(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/DescribeEntity?catalog={CATALOG}&entityId={id}"),
        None,
    )
    .await
}

async fn list_change_sets(router: &MultiServiceRouter) -> Answer {
    call(
        router,
        "POST",
        "/ListChangeSets",
        Some(json!({"Catalog": CATALOG})),
    )
    .await
}

/// The entity ids change set `id` named, read back through `ListChangeSets`'
/// `EntityIdList`.
async fn entity_ids_of(router: &MultiServiceRouter, id: &str) -> Vec<String> {
    list_change_sets(router)
        .await
        .list("/ChangeSetSummaryList")
        .into_iter()
        .find(|item| item["ChangeSetId"] == json!(id))
        .map(|item| {
            item["EntityIdList"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// A one-change `StartChangeSet` body, with the details carried by `member` (`Details` as
/// a JSON string, or `DetailsDocument` as a document) and acting on `identifier` — an
/// entity the change updates, or a new one when it is absent.
fn change_set(
    change_type: &str,
    member: &str,
    details: &Value,
    identifier: Option<&str>,
) -> Value {
    let mut entity = json!({"Type": "AmiProduct@1.0"});
    if let Some(identifier) = identifier {
        entity["Identifier"] = json!(identifier);
    }
    let mut change = json!({"ChangeType": change_type, "Entity": entity});
    change[member] = if member == "Details" {
        json!(details.to_string())
    } else {
        details.clone()
    };
    json!({"Catalog": CATALOG, "ChangeSet": [change]})
}

/// A flat details object of `count` distinct members under `prefix`, padded so its `Json`
/// string is EXACTLY `target` characters.
fn details_of_length(prefix: &str, count: usize, target: usize) -> Value {
    let mut members = Map::new();
    for index in 0..count {
        members.insert(format!("{prefix}m{index}"), json!("v".repeat(30)));
    }
    let pad = format!("{prefix}Pad");
    members.insert(pad.clone(), json!(""));
    let base = Value::Object(members.clone()).to_string().len();
    assert!(
        base <= target,
        "a case asking for {target} characters cannot carry {count} members, \
         which already take {base}"
    );
    members.insert(pad, json!("p".repeat(target - base)));
    let details = Value::Object(members);
    let built = details.to_string().len();
    assert_eq!(
        built, target,
        "the case must submit exactly {target} characters, built {built}"
    );
    details
}

/// A details object carrying `options` delivery options — each with an id of the width
/// the catalog mints when `with_ids`, and none when not — padded so its `Json` string is
/// EXACTLY `target` characters.
fn details_with_options(options: usize, with_ids: bool, target: usize) -> Value {
    let mut members = Map::new();
    let list: Vec<Value> = (0..options)
        .map(|index| {
            // The member names inside one delivery option come from the catalog's own
            // change documents, which the model declares no shape for; what matters to
            // the length rule is that an option the catalog has to identify is an object
            // carrying no `Id`.
            let mut option = json!({"Details": {"AmiDeliveryOptionDetails": {}}});
            if with_ids {
                option["Id"] = json!(format!("do-{index:012x}"));
            }
            option
        })
        .collect();
    members.insert("DeliveryOptions".to_string(), json!(list));
    members.insert("Pad".to_string(), json!(""));
    let base = Value::Object(members.clone()).to_string().len();
    assert!(
        base <= target,
        "a case asking for {target} characters cannot carry {options} delivery options, \
         which already take {base}"
    );
    members.insert("Pad".to_string(), json!("p".repeat(target - base)));
    Value::Object(members)
}

/// Assert that `details` is reportable as the modelled `Json` string: inside its
/// `@length`, and matching its `^[\s]*\{[\s\S]*\}[\s]*$`.
fn assert_reportable(case: &str, details: &str) {
    assert!(
        details.len() <= DETAILS_JSON_LIMIT && details.len() >= DETAILS_JSON_MINIMUM,
        "case {case:?}: a reported Details must be {DETAILS_JSON_MINIMUM}..\
         {DETAILS_JSON_LIMIT} characters, got {}",
        details.len()
    );
    assert!(
        details.trim().starts_with('{') && details.trim().ends_with('}'),
        "case {case:?}: a reported Details must be a JSON object, got {:?}",
        details.chars().take(32).collect::<String>()
    );
}

/// One row of [`the_length_json_declares_bounds_the_details_a_change_may_submit`]: the
/// case, the member carrying the details, the details, and the status the submit is
/// answered with.
type SubmitRow = (&'static str, &'static str, Value, u16);

/// A change submitting details inside `Json`'s modelled length is applied and reported
/// back; one submitting more is refused and applies nothing.
///
/// The rows differ along which member carries the details (`Details` as the modelled
/// string, `DetailsDocument` as the document the model bounds in no way), how the value
/// gets its size (one long member value, many flat members, a list of nested objects),
/// and where it sits relative to the published bound (the minimum, just inside, exactly
/// on it, one character past, far past).
#[tokio::test]
async fn the_length_json_declares_bounds_the_details_a_change_may_submit() {
    let rows: Vec<SubmitRow> = vec![
        (
            "the modelled minimum: an empty details object",
            "DetailsDocument",
            json!({}),
            200,
        ),
        (
            "many flat members, one character inside the bound",
            "DetailsDocument",
            details_of_length("a", 300, DETAILS_JSON_LIMIT - 1),
            200,
        ),
        (
            "one long member value, exactly on the bound",
            "DetailsDocument",
            details_of_length("b", 0, DETAILS_JSON_LIMIT),
            200,
        ),
        (
            "a list of nested objects inside the bound",
            "DetailsDocument",
            json!({"Rows": (0..50)
                .map(|index| json!({"Text": "x".repeat(100), "At": index}))
                .collect::<Vec<Value>>()}),
            200,
        ),
        (
            "one long member value, one character past the bound",
            "DetailsDocument",
            details_of_length("c", 0, DETAILS_JSON_LIMIT + 1),
            422,
        ),
        (
            "a thousand flat members, far past the bound",
            "DetailsDocument",
            details_of_length("d", 1_000, 48_000),
            422,
        ),
        (
            "a list of nested objects past the bound",
            "DetailsDocument",
            json!({"Rows": (0..300)
                .map(|index| json!({"Text": "x".repeat(100), "At": index}))
                .collect::<Vec<Value>>()}),
            422,
        ),
        (
            "the same value as the modelled Details string, exactly on the bound",
            "Details",
            details_of_length("e", 0, DETAILS_JSON_LIMIT),
            200,
        ),
        // Past `Json`'s own `@length`, the modelled string member is refused by the
        // centralized boundary walk before the handler runs (`skip_validation` on
        // `Change` names only `ChangeName`), which answers its own
        // `ValidationException`.
        (
            "the modelled Details string, one character past the bound",
            "Details",
            details_of_length("f", 0, DETAILS_JSON_LIMIT + 1),
            400,
        ),
    ];

    for (case, member, details, expected_status) in rows {
        let router = emulator();
        let submitted = details.to_string();
        let answer = start_change_set(
            &router,
            change_set("CreateProduct", member, &details, None),
        )
        .await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?} ({} characters in {member}): expected {expected_status}, got {} \
             with {:?}",
            submitted.len(),
            answer.status,
            answer.body
        );

        if expected_status != 200 {
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: a details value past the modelled length must be refused \
                 as a ValidationException, got {:?}",
                answer.body
            );
            if expected_status == 422 {
                let message = answer.message();
                assert!(
                    message.contains(member)
                        && message.contains(&DETAILS_JSON_LIMIT.to_string())
                        && message.contains(&submitted.len().to_string()),
                    "case {case:?}: the refusal must name the member, the bound and the \
                     length it sent, got {message:?}"
                );
            }
            // A refused submit starts nothing, so there is no change set and no entity
            // for a later read to find.
            let listed = list_change_sets(&router).await;
            assert!(
                listed.list("/ChangeSetSummaryList").is_empty(),
                "case {case:?}: a refused submit must start no change set, got {:?}",
                listed.body
            );
            continue;
        }

        let id = answer.text("/ChangeSetId");
        wait_for_the_end_of_the_run();
        let described = describe_change_set(&router, &id).await;
        assert_eq!(
            described.text("/Status"),
            "SUCCEEDED",
            "case {case:?}: details inside the modelled length must be applied, got {:?}",
            described.body
        );

        let entity_id = entity_ids_of(&router, &id)
            .await
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the change set must name its entity"));
        let entity = describe_entity(&router, &entity_id).await;
        assert_eq!(
            entity.status, 200,
            "case {case:?}: the created entity must be readable, got {} with {:?}",
            entity.status, entity.body
        );
        let reported = entity.text("/Details");
        assert_reportable(case, &reported);
        assert_eq!(
            serde_json::from_str::<Value>(&reported).ok(),
            Some(details.clone()),
            "case {case:?}: the entity must report the details the change submitted, got \
             {reported:?}"
        );
        assert_eq!(
            entity.body.pointer("/DetailsDocument"),
            Some(&details),
            "case {case:?}: the document must carry the same details as the string, got \
             {:?}",
            entity.body
        );

        // The change set reports what the change SUBMITTED, in the same `Json` member,
        // so it is bounded by the same length.
        let summary = described.list("/ChangeSet");
        let change_details = summary
            .first()
            .and_then(|change| change.get("Details"))
            .and_then(Value::as_str)
            .unwrap_or("{}");
        assert_reportable(case, change_details);
    }
}

/// A change set whose merge would leave an entity reporting more than `Json`'s length
/// fails at the end of its run, naming the bound and the length the merge reached, and
/// leaves the entity's details as they were — so successive change sets can never grow a
/// reported `Details` past the bound.
///
/// The rows differ along how many steps they take, how large each step is, and which step
/// crosses the bound. The expected outcome of each step is derived from the rule itself —
/// the merge the step describes is serialized here and compared with the bound — rather
/// than written down per row.
#[tokio::test]
async fn a_merge_past_the_bound_fails_the_change_set_and_leaves_the_entity() {
    let rows: Vec<(&str, Vec<usize>)> = vec![
        (
            "three eight-kilobyte steps, the last past the bound",
            vec![8_000, 8_000, 8_000],
        ),
        (
            "six four-kilobyte steps, the last two past the bound",
            vec![4_000; 6],
        ),
        (
            "a first step exactly on the bound, then one small member",
            vec![DETAILS_JSON_LIMIT, 64],
        ),
    ];

    for (case, steps) in rows {
        let router = emulator();
        // What the entity holds, carried forward across the steps that were applied.
        let mut held: Map<String, Value> = Map::new();
        let mut entity_id = String::new();

        for (step, target) in steps.iter().copied().enumerate() {
            let case = format!("{case}, step {step}");
            // Each step submits members of its OWN, so every step that is applied GROWS
            // the stored object rather than replacing what the last one left.
            let details = details_of_length(&format!("s{step}"), 0, target);
            let mut merged = held.clone();
            for (key, value) in details.as_object().expect("the step builds an object") {
                merged.insert(key.clone(), value.clone());
            }
            let merged_length = Value::Object(merged.clone()).to_string().len();
            let fits = merged_length <= DETAILS_JSON_LIMIT;

            let request = if step == 0 {
                change_set("CreateProduct", "DetailsDocument", &details, None)
            } else {
                change_set(
                    "UpdateInformation",
                    "DetailsDocument",
                    &details,
                    Some(&entity_id),
                )
            };
            let started = start_change_set(&router, request).await;
            assert_eq!(
                started.status, 200,
                "case {case:?}: a step inside the submitted length must be accepted, got \
                 {} with {:?}",
                started.status, started.body
            );
            let id = started.text("/ChangeSetId");
            wait_for_the_end_of_the_run();
            let described = describe_change_set(&router, &id).await;

            if step == 0 {
                entity_id = entity_ids_of(&router, &id)
                    .await
                    .first()
                    .cloned()
                    .unwrap_or_else(|| panic!("case {case:?}: the step must name its entity"));
            }

            if fits {
                assert_eq!(
                    described.text("/Status"),
                    "SUCCEEDED",
                    "case {case:?}: a merge of {merged_length} characters is inside the \
                     bound and must be applied, got {:?}",
                    described.body
                );
                held = merged;
            } else {
                assert_eq!(
                    (described.text("/Status"), described.text("/FailureCode")),
                    ("FAILED".to_string(), "CLIENT_ERROR".to_string()),
                    "case {case:?}: a merge of {merged_length} characters passes the bound \
                     and must fail the change set, got {:?}",
                    described.body
                );
                let errors = described.list("/ChangeSet/0/ErrorDetailList");
                let error = errors.first().cloned().unwrap_or(Value::Null);
                assert_eq!(
                    error["ErrorCode"],
                    json!("LIMIT_EXCEEDED"),
                    "case {case:?}: the change must report the bound it crossed, got \
                     {error:?}"
                );
                let message = error["ErrorMessage"].as_str().unwrap_or_default();
                assert!(
                    message.contains(&DETAILS_JSON_LIMIT.to_string())
                        && message.contains(&merged_length.to_string())
                        && message.contains(&entity_id),
                    "case {case:?}: the error must name the bound, the length the merge \
                     reached and the entity, got {message:?}"
                );
            }

            let entity = describe_entity(&router, &entity_id).await;
            assert_eq!(
                entity.status, 200,
                "case {case:?}: the entity must stay readable, got {} with {:?}",
                entity.status, entity.body
            );
            let reported = entity.text("/Details");
            assert_reportable(&case, &reported);
            assert_eq!(
                serde_json::from_str::<Value>(&reported).ok(),
                Some(Value::Object(held.clone())),
                "case {case:?}: the entity must report exactly what the applied steps \
                 merged, got {} characters",
                reported.len()
            );
        }
    }
}

/// The bound is judged on the merge EVERY change of the change set would leave, so two
/// changes against one entity are judged on their combined result: a change set whose
/// first change adds a delivery option the catalog has to identify and whose second fills
/// the entity to just under the bound is failed by the second change, because the id the
/// first change's option still needs is part of what the entity would then report.
#[tokio::test]
async fn two_changes_against_one_entity_are_judged_on_their_combined_merge() {
    let router = emulator();

    // An entity to act on, so both changes of the next change set name the same one.
    let created = start_change_set(
        &router,
        change_set(
            "CreateProduct",
            "DetailsDocument",
            &json!({"ProductTitle": "sized"}),
            None,
        ),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the entity the case acts on must be created, got {} with {:?}",
        created.status, created.body
    );
    wait_for_the_end_of_the_run();
    let entity_id = entity_ids_of(&router, &created.text("/ChangeSetId"))
        .await
        .first()
        .cloned()
        .expect("the creating change set must name its entity");
    let before = describe_entity(&router, &entity_id).await;
    let held: Map<String, Value> = serde_json::from_str(&before.text("/Details"))
        .ok()
        .and_then(|value: Value| value.as_object().cloned())
        .expect("the created entity must report its details as a JSON object");

    // The merge the two changes describe: what the entity holds, the option the first
    // change adds (which carries no id, so the catalog has to assign one), and the
    // members the second change adds — padded so the merge itself lands just UNDER the
    // bound and only the id it still owes takes it past.
    let option = json!({"DeliveryOptions": [{"Details": {"AmiDeliveryOptionDetails": {}}}]});
    let mut merged = held.clone();
    for (key, value) in option.as_object().expect("the option builds an object") {
        merged.insert(key.clone(), value.clone());
    }
    let target = DETAILS_JSON_LIMIT - 10;
    merged.insert("Filler".to_string(), json!(""));
    let base = Value::Object(merged.clone()).to_string().len();
    assert!(
        base <= target,
        "the case must be able to pad the merge to {target} characters, which already \
         takes {base}"
    );
    let filler = "p".repeat(target - base);
    merged.insert("Filler".to_string(), json!(filler.clone()));
    let merged_length = Value::Object(merged).to_string().len();
    assert_eq!(
        merged_length, target,
        "the merge must land on {target} characters, built {merged_length}"
    );

    let started = start_change_set(
        &router,
        json!({
            "Catalog": CATALOG,
            "ChangeSet": [
                {
                    "ChangeType": "AddDeliveryOptions",
                    "Entity": {"Type": "AmiProduct@1.0", "Identifier": entity_id},
                    "DetailsDocument": option,
                },
                {
                    "ChangeType": "UpdateInformation",
                    "Entity": {"Type": "AmiProduct@1.0", "Identifier": entity_id},
                    "DetailsDocument": {"Filler": filler},
                },
            ],
        }),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "each change submits a value inside the bound, so the change set must be \
         accepted, got {} with {:?}",
        started.status, started.body
    );
    wait_for_the_end_of_the_run();
    let described = describe_change_set(&router, &started.text("/ChangeSetId")).await;
    assert_eq!(
        (described.text("/Status"), described.text("/FailureCode")),
        ("FAILED".to_string(), "CLIENT_ERROR".to_string()),
        "the combined merge plus the id the catalog owes passes the bound, so the change \
         set must fail, got {:?}",
        described.body
    );
    assert_eq!(
        described.body.pointer("/ChangeSet/0/ErrorDetailList"),
        None,
        "the first change is applicable on its own merge, so it reports no error, got {:?}",
        described.body
    );
    let error = described
        .list("/ChangeSet/1/ErrorDetailList")
        .first()
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        error["ErrorCode"],
        json!("LIMIT_EXCEEDED"),
        "the change that crosses the bound must report it, got {error:?}"
    );
    let message = error["ErrorMessage"].as_str().unwrap_or_default();
    assert!(
        message.contains(&DETAILS_JSON_LIMIT.to_string())
            && message.contains(&entity_id)
            && message
                .split_whitespace()
                .filter_map(|word| word.parse::<usize>().ok())
                .any(|number| number > DETAILS_JSON_LIMIT),
        "the error must name the bound, the entity and a length past the bound, got \
         {message:?}"
    );

    let after = describe_entity(&router, &entity_id).await;
    assert_eq!(
        after.body, before.body,
        "a failed change set must leave the entity exactly as it was"
    );
    assert_reportable("two changes against one entity", &after.text("/Details"));
}

/// The ids the catalog assigns to the delivery options a change submits are part of what
/// the entity then reports, so they count toward the bound: a change whose submitted
/// details sit exactly on the bound and whose options still need ids would leave the
/// entity reporting more than `Json` admits, and fails — while the same change with the
/// ids already supplied is applied and reported at the bound.
///
/// The rows differ along how many options the change carries and whether the catalog has
/// to mint their ids.
#[tokio::test]
async fn the_ids_the_catalog_assigns_count_toward_the_bound() {
    // (case, how many delivery options, whether the change supplies their ids)
    let rows: Vec<(&str, usize, bool)> = vec![
        ("one option the catalog has to identify", 1, false),
        ("five options the catalog has to identify", 5, false),
        ("five options that already carry their ids", 5, true),
    ];

    for (case, options, with_ids) in rows {
        let router = emulator();
        let details = details_with_options(options, with_ids, DETAILS_JSON_LIMIT);
        let submitted = details.to_string();
        assert_eq!(
            submitted.len(),
            DETAILS_JSON_LIMIT,
            "case {case:?}: the case must submit a value exactly on the bound, got {}",
            submitted.len()
        );

        let started = start_change_set(
            &router,
            change_set("CreateProduct", "DetailsDocument", &details, None),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: a submitted value on the bound must be accepted, got {} with \
             {:?}",
            started.status, started.body
        );
        let id = started.text("/ChangeSetId");
        wait_for_the_end_of_the_run();
        let described = describe_change_set(&router, &id).await;
        let entity_id = entity_ids_of(&router, &id)
            .await
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the change set must name its entity"));
        let entity = describe_entity(&router, &entity_id).await;

        if with_ids {
            assert_eq!(
                described.text("/Status"),
                "SUCCEEDED",
                "case {case:?}: options that need no id leave the value on the bound, so \
                 the change set must be applied, got {:?}",
                described.body
            );
            assert_eq!(
                entity.status, 200,
                "case {case:?}: the created entity must be readable, got {} with {:?}",
                entity.status, entity.body
            );
            assert_reportable(case, &entity.text("/Details"));
            continue;
        }

        assert_eq!(
            (described.text("/Status"), described.text("/FailureCode")),
            ("FAILED".to_string(), "CLIENT_ERROR".to_string()),
            "case {case:?}: the ids the catalog would assign take the value past the \
             bound, so the change set must fail, got {:?}",
            described.body
        );
        assert_eq!(
            (entity.status, entity.error_code()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a failed change set creates no entity, got {} with {:?}",
            entity.status,
            entity.body
        );
    }
}
