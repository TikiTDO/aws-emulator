// SPDX-License-Identifier: CC0-1.0

//! The failure branch of the change-set lifecycle, end to end over HTTP.
//!
//! The rule under test: a change set is applied asynchronously, so what only applying can
//! judge — the members inside the opaque `Details`/`DetailsDocument` document the model
//! declares no shape for, and what the store already holds — does not refuse
//! `StartChangeSet`. Such a change set is accepted, runs, and then ends its run in the
//! terminal status `FAILED` with `FailureCode=CLIENT_ERROR`, reporting the defect of each
//! change in that change's `ErrorDetailList` and, when the defect belongs to the set
//! rather than to any one change, in `FailureDescription` — and it applies nothing.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::state::{
    CHANGE_SET_APPLYING, CHANGE_SET_PREPARING, ENTITY_VISIBILITY_VALUES, MAX_RESOURCE_TAGS,
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

/// How long after the start a change set has certainly ended its run: the two spans the
/// implementation publishes, plus a margin for a slow machine.
fn wait_for_the_end_of_the_run() {
    std::thread::sleep(CHANGE_SET_PREPARING + CHANGE_SET_APPLYING + Duration::from_millis(400));
}

/// `DateTimeISO8601`'s 20-character `....-..-..T..:..:..Z`, spelled out here rather than
/// through the crate's own predicate so a wrong implementation cannot make this pass.
fn is_iso8601(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 20
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':'
        && bytes[19] == b'Z'
        && [0..4, 5..7, 8..10, 11..13, 14..16, 17..19]
            .iter()
            .all(|range| bytes[range.clone()].iter().all(u8::is_ascii_digit))
}

/// `ErrorCodeString`'s `^[a-zA-Z_]+$`, with its `@length(min: 1, max: 72)`.
fn is_error_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 72
        && value.chars().all(|c| c.is_ascii_alphabetic() || c == '_')
}

/// `ExceptionMessageContent`'s `@length(min: 1, max: 2048)` and `^(.)+$`.
fn is_message(value: &str) -> bool {
    !value.is_empty() && value.chars().count() <= 2048 && !value.contains('\n')
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

async fn list_change_sets(router: &MultiServiceRouter) -> Answer {
    call(
        router,
        "POST",
        "/ListChangeSets",
        Some(json!({"Catalog": CATALOG})),
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

async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> Answer {
    call(
        router,
        "POST",
        "/ListTagsForResource",
        Some(json!({"ResourceArn": resource_arn})),
    )
    .await
}

/// Start `request`, wait for the clock to end its run, and answer the change set id and
/// the entity ids the change set named.
async fn run_change_set(router: &MultiServiceRouter, case: &str, request: Value) -> (String, Vec<String>) {
    let started = start_change_set(router, request).await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: a change set whose content only applying can judge must be \
         ACCEPTED, got {} with {:?}",
        started.status, started.body
    );
    let id = started.text("/ChangeSetId");

    // Nothing is reported as failed before the run has ended: the failure is the outcome
    // of applying the changes, not of accepting them.
    let running = describe_change_set(router, &id).await;
    assert!(
        ["PREPARING", "APPLYING"].contains(&running.text("/Status").as_str()),
        "case {case:?}: the change set must still be running, got {:?}",
        running.body
    );
    assert_eq!(
        running.body.pointer("/FailureCode"),
        None,
        "case {case:?}: a running change set reports no FailureCode, got {:?}",
        running.body
    );

    wait_for_the_end_of_the_run();
    let entity_ids = list_change_sets(router)
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
        .unwrap_or_default();
    (id, entity_ids)
}

/// A one-change `StartChangeSet` body creating an entity of `entity_type`, with the
/// details carried by `member` (`Details` as a JSON string, or `DetailsDocument`).
fn create_change(entity_type: &str, member: &str, details: Value) -> Value {
    let mut change =
        json!({"ChangeType": "CreateProduct", "Entity": {"Type": format!("{entity_type}@1.0")}});
    change[member] = if member == "Details" {
        json!(details.to_string())
    } else {
        details
    };
    json!({"Catalog": CATALOG, "ChangeSet": [change]})
}

/// Create an entity and answer its id and ARN, read back through `DescribeEntity`.
async fn created_entity(
    router: &MultiServiceRouter,
    entity_type: &str,
    details: Value,
    entity_tags: Value,
) -> (String, String) {
    let mut request = create_change(entity_type, "DetailsDocument", details);
    // `Change$EntityTags` is `@length(min: 1, max: 200)`, so a case that wants no tags
    // leaves the member out rather than sending an empty list.
    if entity_tags.as_array().is_some_and(|tags| !tags.is_empty()) {
        request["ChangeSet"][0]["EntityTags"] = entity_tags;
    }
    let (id, entity_ids) = run_change_set(router, "the entity the case acts on", request).await;
    let described = describe_change_set(router, &id).await;
    assert_eq!(
        described.text("/Status"),
        "SUCCEEDED",
        "the entity the case acts on must have been applied, got {:?}",
        described.body
    );
    let entity_id = entity_ids
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("change set {id} must name its entity"));
    let entity = describe_entity(router, &entity_id).await;
    assert_eq!(
        entity.status, 200,
        "the created entity must be readable, got {} with {:?}",
        entity.status, entity.body
    );
    (entity_id, entity.text("/EntityArn"))
}

/// Assert that change set `id` ended its run as a failure of its own changes: `FAILED`
/// with `FailureCode=CLIENT_ERROR`, an `EndTime`, the expected number of
/// `ErrorDetailList` entries on the expected changes and none on the rest, and no
/// `FailureDescription` — the failure is related to the changes.
async fn assert_failed_on_its_changes(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    expected: &[(usize, usize)],
) {
    let described = describe_change_set(router, id).await;
    assert_eq!(
        described.text("/Status"),
        "FAILED",
        "case {case:?}: a change set the catalog cannot apply must end its run FAILED, \
         got {:?}",
        described.body
    );
    assert_eq!(
        described.text("/FailureCode"),
        "CLIENT_ERROR",
        "case {case:?}: a failure in the request is reported as CLIENT_ERROR, got {:?}",
        described.body
    );
    assert!(
        is_iso8601(&described.text("/EndTime")),
        "case {case:?}: a change set in a terminal state carries an EndTime, got {:?}",
        described.body
    );
    assert_eq!(
        described.body.pointer("/FailureDescription"),
        None,
        "case {case:?}: a failure the changes carry is not described on the change set, \
         got {:?}",
        described.body
    );

    let summaries = described.list("/ChangeSet");
    for (index, summary) in summaries.iter().enumerate() {
        let expected_count = expected
            .iter()
            .find(|(at, _)| *at == index)
            .map(|(_, count)| *count);
        let Some(expected_count) = expected_count else {
            assert_eq!(
                summary.get("ErrorDetailList"),
                None,
                "case {case:?}: change {index} is applicable, so it carries no \
                 ErrorDetailList, got {summary:?}"
            );
            continue;
        };
        let errors = summary["ErrorDetailList"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            errors.len(),
            expected_count,
            "case {case:?}: change {index} must report one error per defect, got {summary:?}"
        );
        for error in &errors {
            let code = error["ErrorCode"].as_str().unwrap_or_default();
            let message = error["ErrorMessage"].as_str().unwrap_or_default();
            assert!(
                is_error_code(code),
                "case {case:?}: change {index} must name its error with an ErrorCode \
                 matching ^[a-zA-Z_]+$, got {error:?}"
            );
            assert!(
                is_message(message),
                "case {case:?}: change {index} must explain its error, got {error:?}"
            );
        }
    }

    // A read does not change what the next reader sees, and the list reports the same
    // outcome as the describe.
    let again = describe_change_set(router, id).await;
    assert_eq!(
        again.body, described.body,
        "case {case:?}: describing a failed change set twice must answer the same record"
    );
    let listed = list_change_sets(router)
        .await
        .list("/ChangeSetSummaryList")
        .into_iter()
        .find(|item| item["ChangeSetId"] == json!(id))
        .unwrap_or_else(|| panic!("case {case:?}: the failed change set must be listed"));
    assert_eq!(
        (listed["Status"].clone(), listed["FailureCode"].clone()),
        (json!("FAILED"), json!("CLIENT_ERROR")),
        "case {case:?}: the list must report the same status and failure, got {listed:?}"
    );
}

/// One row of [`a_change_set_whose_content_cannot_be_applied_fails_with_its_errors`]: the
/// case, the `StartChangeSet` body, and `(change index, number of errors)` for every
/// change applying is expected to find a defect in.
type FailureRow = (&'static str, Value, &'static [(usize, usize)]);

/// A change set whose changes carry content the catalog cannot read fails at the end of
/// its run, reporting the defect of each change in that change's `ErrorDetailList` — and
/// applies nothing.
///
/// The rows differ along which member is defective, how its value is wrong (a value
/// outside the published set, a value of the wrong JSON type, a null), which member of
/// the details carries it (`Details` or `DetailsDocument`), how many changes the set has,
/// which change is the defective one, how many defects one change has, and the `Intent`.
#[tokio::test]
async fn a_change_set_whose_content_cannot_be_applied_fails_with_its_errors() {
    // (case, request, (change index, number of errors) for every defective change)
    let rows: Vec<FailureRow> = vec![
        (
            "a Visibility outside the modelled values, details as a JSON string",
            create_change(
                "AmiProduct",
                "Details",
                json!({"ProductTitle": "titled", "Visibility": "Sideways"}),
            ),
            &[(0, 1)],
        ),
        (
            "a Visibility outside the modelled values, details as a document",
            create_change(
                "DataProduct",
                "DetailsDocument",
                json!({"ProductTitle": "titled", "Visibility": "Sideways"}),
            ),
            &[(0, 1)],
        ),
        (
            "a Visibility that is a number rather than one of the values",
            create_change("SaaSProduct", "DetailsDocument", json!({"Visibility": 7})),
            &[(0, 1)],
        ),
        (
            "a Visibility that is null",
            create_change("AmiProduct", "DetailsDocument", json!({"Visibility": null})),
            &[(0, 1)],
        ),
        (
            "a Visibility that is a list of the modelled values",
            create_change(
                "AmiProduct",
                "DetailsDocument",
                json!({"Visibility": ENTITY_VISIBILITY_VALUES}),
            ),
            &[(0, 1)],
        ),
        (
            "a ProductTitle that is an object rather than a string",
            create_change(
                "ContainerProduct",
                "DetailsDocument",
                json!({"ProductTitle": {"en": "titled"}}),
            ),
            &[(0, 1)],
        ),
        (
            "a Name that is a number rather than a string",
            create_change(
                "Offer",
                "DetailsDocument",
                json!({"Name": 42, "Description": "fine"}),
            ),
            &[(0, 1)],
        ),
        (
            "two defective members in one change",
            create_change(
                "AmiProduct",
                "Details",
                json!({"ProductTitle": 1, "Visibility": "Sideways"}),
            ),
            &[(0, 2)],
        ),
        (
            "the second of three changes is the defective one",
            json!({
                "Catalog": CATALOG,
                "ChangeSetName": "second-change-fails",
                "ChangeSet": [
                    {
                        "ChangeType": "CreateProduct",
                        "Entity": {"Type": "AmiProduct@1.0"},
                        "DetailsDocument": {"ProductTitle": "first"},
                    },
                    {
                        "ChangeType": "CreateProduct",
                        "Entity": {"Type": "DataProduct@1.0"},
                        "DetailsDocument": {"ProductTitle": "second", "Visibility": "Sideways"},
                    },
                    {
                        "ChangeType": "CreateProduct",
                        "Entity": {"Type": "SaaSProduct@1.0"},
                        "Details": "{\"ProductTitle\":\"third\"}",
                    },
                ],
            }),
            &[(1, 1)],
        ),
        (
            "every change of the set is defective",
            json!({
                "Catalog": CATALOG,
                "ChangeSet": [
                    {
                        "ChangeType": "CreateProduct",
                        "Entity": {"Type": "AmiProduct@1.0"},
                        "DetailsDocument": {"ProductTitle": true},
                    },
                    {
                        "ChangeType": "CreateProduct",
                        "Entity": {"Type": "AmiProduct@1.0"},
                        "DetailsDocument": {"Visibility": "Elsewhere"},
                    },
                ],
            }),
            &[(0, 1), (1, 1)],
        ),
        (
            "a VALIDATE change set, which is what reports the defect before it is applied",
            json!({
                "Catalog": CATALOG,
                "Intent": "VALIDATE",
                "ChangeSet": [{
                    "ChangeType": "AddRevisions",
                    "Entity": {"Type": "AmiProduct@1.0"},
                    "DetailsDocument": {"ProductTitle": ["titled"]},
                }],
            }),
            &[(0, 1)],
        ),
    ];

    for (case, request, expected) in rows {
        let router = emulator();
        let (id, entity_ids) = run_change_set(&router, case, request).await;
        assert_failed_on_its_changes(&router, case, &id, expected).await;

        // A change set that failed applied nothing: no entity it named exists.
        for entity_id in &entity_ids {
            let absent = describe_entity(&router, entity_id).await;
            assert_eq!(
                (absent.status, absent.error_code()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: a failed change set creates no entity, but {entity_id} \
                 answered {} {:?}",
                absent.status,
                absent.body
            );
        }
    }
}

/// The same rule's other side: details the catalog CAN read are applied, so a change set
/// carrying them succeeds and reports no failure. Every value the model's
/// `*VisibilityString` enums publish is such a value.
#[tokio::test]
async fn a_change_set_whose_content_can_be_applied_reports_no_failure() {
    let mut rows: Vec<(String, Value)> = ENTITY_VISIBILITY_VALUES
        .iter()
        .map(|visibility| {
            (
                format!("Visibility {visibility:?}"),
                json!({"ProductTitle": "titled", "Visibility": visibility}),
            )
        })
        .collect();
    rows.push((
        "no Visibility at all".to_string(),
        json!({"ProductTitle": "titled"}),
    ));
    rows.push((
        "members the catalog does not read".to_string(),
        json!({"Versions": [{"Name": "v1"}], "Regions": ["us-east-1"], "Count": 3}),
    ));

    for (case, details) in rows {
        let router = emulator();
        let (id, entity_ids) = run_change_set(
            &router,
            &case,
            create_change("AmiProduct", "DetailsDocument", details),
        )
        .await;
        let described = describe_change_set(&router, &id).await;
        assert_eq!(
            described.text("/Status"),
            "SUCCEEDED",
            "case {case:?}: details the catalog can read must be applied, got {:?}",
            described.body
        );
        assert_eq!(
            (
                described.body.pointer("/FailureCode"),
                described.body.pointer("/FailureDescription")
            ),
            (None, None),
            "case {case:?}: a succeeded change set reports no failure, got {:?}",
            described.body
        );
        for summary in described.list("/ChangeSet") {
            assert_eq!(
                summary.get("ErrorDetailList"),
                None,
                "case {case:?}: an applied change carries no error, got {summary:?}"
            );
        }
        let entity_id = entity_ids
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the change set must name its entity"));
        let entity = describe_entity(&router, &entity_id).await;
        assert_eq!(
            entity.status, 200,
            "case {case:?}: the applied entity must be readable, got {} with {:?}",
            entity.status, entity.body
        );
    }
}

/// A change set each of whose changes is applicable on its own but which contradicts
/// itself — two changes claiming a different value for the same ATTRIBUTE of one entity,
/// which are the members the catalog reads — fails with a `FailureDescription` instead,
/// since that failure belongs to no one change; and the entity it named is left exactly
/// as it was.
///
/// A member the catalog does not read is merged rather than claimed, so two changes
/// naming it is not a contradiction: the last one stands. The rows differ along which
/// attribute the changes claim, whether they agree, whether they claim the same attribute
/// at all, and whether the member is one the catalog reads.
#[tokio::test]
async fn a_change_set_that_contradicts_itself_fails_with_a_description() {
    // (case, entity type, the first change's details, the second change's details, fails)
    let rows: Vec<(&str, &str, Value, Value, bool)> = vec![
        (
            "the entity's name claimed twice with different values",
            "AmiProduct",
            json!({"ProductTitle": "left"}),
            json!({"ProductTitle": "right"}),
            true,
        ),
        (
            "the entity's visibility claimed twice, with a member agreed on beside it",
            "AmiProduct",
            json!({"Support": "email", "Visibility": "Public"}),
            json!({"Support": "email", "Visibility": "Limited"}),
            true,
        ),
        (
            "the other member an entity's name is read from",
            "Offer",
            json!({"Name": "left"}),
            json!({"Name": "right"}),
            true,
        ),
        (
            "the same attribute claimed twice with the same value",
            "AmiProduct",
            json!({"ProductTitle": "agreed"}),
            json!({"ProductTitle": "agreed"}),
            false,
        ),
        (
            "two different attributes of the one entity",
            "AmiProduct",
            json!({"ProductTitle": "titled"}),
            json!({"Visibility": "Public"}),
            false,
        ),
        (
            "a member the catalog does not read, given different values",
            "AmiProduct",
            json!({"Versions": [{"Name": "v1"}]}),
            json!({"Versions": [{"Name": "v2"}]}),
            false,
        ),
    ];

    for (case, entity_type, first, second, must_fail) in rows {
        let router = emulator();
        let (entity_id, _) = created_entity(
            &router,
            entity_type,
            json!({"Description": "created"}),
            json!([]),
        )
        .await;
        let request = json!({
            "Catalog": CATALOG,
            "ChangeSet": [
                {
                    "ChangeType": "UpdateInformation",
                    "Entity": {"Type": format!("{entity_type}@1.0"), "Identifier": entity_id},
                    "DetailsDocument": first,
                },
                {
                    "ChangeType": "UpdateDeliveryOptions",
                    "Entity": {"Type": format!("{entity_type}@1.0"), "Identifier": entity_id},
                    "DetailsDocument": second.clone(),
                },
            ],
        });
        let before = describe_entity(&router, &entity_id).await;
        let (id, _) = run_change_set(&router, case, request).await;
        let described = describe_change_set(&router, &id).await;

        if !must_fail {
            assert_eq!(
                described.text("/Status"),
                "SUCCEEDED",
                "case {case:?}: changes that do not contradict each other must be applied, \
                 got {:?}",
                described.body
            );
            assert_eq!(
                described.body.pointer("/FailureDescription"),
                None,
                "case {case:?}: an applied change set describes no failure, got {:?}",
                described.body
            );
            // And the merge the catalog did apply is the one the LAST change asked for.
            let applied = describe_entity(&router, &entity_id).await;
            for (member, value) in second.as_object().expect("the row's details are an object") {
                assert_eq!(
                    applied.body.pointer(&format!("/DetailsDocument/{member}")),
                    Some(value),
                    "case {case:?}: the last change to name {member} is the one that stands, \
                     got {:?}",
                    applied.body
                );
            }
            continue;
        }

        assert_eq!(
            described.text("/Status"),
            "FAILED",
            "case {case:?}: a change set that contradicts itself must fail, got {:?}",
            described.body
        );
        assert_eq!(
            described.text("/FailureCode"),
            "CLIENT_ERROR",
            "case {case:?}: the contradiction is in the request, got {:?}",
            described.body
        );
        assert!(
            is_message(&described.text("/FailureDescription")),
            "case {case:?}: a failure that belongs to no change must be described on the \
             change set, got {:?}",
            described.body
        );
        for summary in described.list("/ChangeSet") {
            assert_eq!(
                summary.get("ErrorDetailList"),
                None,
                "case {case:?}: each change is applicable on its own, so none carries an \
                 error, got {summary:?}"
            );
        }
        let after = describe_entity(&router, &entity_id).await;
        assert_eq!(
            after.body, before.body,
            "case {case:?}: a failed change set must leave the entity it named alone"
        );
    }
}

/// A change whose `EntityTags` would leave its entity holding more tags than one resource
/// may hold is a defect only applying can find — the bound is on what the STORE holds, not
/// on the request — so the change set fails with that change's `ErrorDetailList`, and the
/// entity keeps the tags it had.
///
/// The rows differ along how many keys the change adds and whether they are keys the
/// entity already holds.
#[tokio::test]
async fn a_change_whose_tags_pass_the_bound_fails_the_change_set() {
    // (case, the tags the second change set adds, how many keys of them are new)
    let rows: Vec<(&str, Value, usize)> = vec![
        (
            "re-tagging keys the entity already holds",
            json!([{"Key": "k0", "Value": "replaced"}, {"Key": "k1", "Value": "replaced"}]),
            0,
        ),
        (
            "one new key past the bound",
            json!([{"Key": "one-too-many", "Value": "v"}]),
            1,
        ),
        (
            "five new keys past the bound",
            json!((0..5)
                .map(|index| json!({"Key": format!("extra{index}"), "Value": "v"}))
                .collect::<Vec<Value>>()),
            5,
        ),
    ];

    for (case, tags, new_keys) in rows {
        let router = emulator();
        // `TagList` is `@length(min: 1, max: 200)` and a resource holds at most
        // `MAX_RESOURCE_TAGS` tags, so one creating change can fill the entity to the
        // bound — which is what makes the NEXT change's merge the thing that crosses it.
        let filling: Vec<Value> = (0..MAX_RESOURCE_TAGS)
            .map(|index| json!({"Key": format!("k{index}"), "Value": "v"}))
            .collect();
        let (entity_id, entity_arn) = created_entity(
            &router,
            "AmiProduct",
            json!({"ProductTitle": "tagged"}),
            json!(filling),
        )
        .await;
        let at_bound = list_tags(&router, &entity_arn).await;
        assert_eq!(
            at_bound.list("/Tags").len(),
            MAX_RESOURCE_TAGS,
            "case {case:?}: the entity must start out holding the whole bound, got {:?}",
            at_bound.body
        );

        let (id, _) = run_change_set(
            &router,
            case,
            json!({
                "Catalog": CATALOG,
                "ChangeSet": [{
                    "ChangeType": "UpdateInformation",
                    "Entity": {"Type": "AmiProduct@1.0", "Identifier": entity_id},
                    "DetailsDocument": {"Support": "email"},
                    "EntityTags": tags,
                }],
            }),
        )
        .await;
        let described = describe_change_set(&router, &id).await;

        if new_keys == 0 {
            assert_eq!(
                described.text("/Status"),
                "SUCCEEDED",
                "case {case:?}: a merge that stays inside the bound must be applied, got {:?}",
                described.body
            );
            let merged = list_tags(&router, &entity_arn).await;
            assert_eq!(
                merged.list("/Tags").len(),
                MAX_RESOURCE_TAGS,
                "case {case:?}: re-tagging a held key replaces its value, got {:?}",
                merged.body
            );
            continue;
        }

        assert_failed_on_its_changes(&router, case, &id, &[(0, 1)]).await;
        let errors = described.list("/ChangeSet/0/ErrorDetailList");
        let message = errors
            .first()
            .and_then(|error| error["ErrorMessage"].as_str())
            .unwrap_or_default();
        assert!(
            message.contains(&MAX_RESOURCE_TAGS.to_string())
                && message.contains(&(MAX_RESOURCE_TAGS + new_keys).to_string()),
            "case {case:?}: the error must name the bound and the count the merge reached, \
             got {message:?}"
        );
        let unchanged = list_tags(&router, &entity_arn).await;
        assert_eq!(
            unchanged.body, at_bound.body,
            "case {case:?}: a failed change set must leave the entity's tags alone"
        );
    }
}
