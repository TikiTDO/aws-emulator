// SPDX-License-Identifier: CC0-1.0

//! Deleting an OpsItem, and how OpsCenter refuses an OpsItem id it holds no OpsItem under.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`call`], [`call_in_region`], [`error_code`],
//! [`error_message`], [`router`], [`router_with_engine`]) dispatches `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` through the multi-service router — and every
//! OpsItem is opened through `all/ops_items.rs`'s harness ([`create_ok`], [`minimal`]). No
//! assertion reads the state struct: what a delete did is read back through the operations
//! a client has.
//!
//! A delete is eventually consistent, so the record it marks is removed at a stored due
//! instant ([`SETTLES_AFTER`]). A case about what a settled delete leaves behind moves the
//! store to that instant ([`settle_deletes`]) rather than waiting for it, the way this
//! crate's other suites move a due instant on; the one case that lets the real clock settle
//! a delete polls for it ([`a_delete_settles_without_anything_asking_it_to`]).
//!
//! The suites are tables. The rows of the delete table differ along what the OpsItem was
//! when the delete reached it — what it carries, how many times it had been edited, what
//! status it is in, what hangs off it, which Region it lives in — rather than the single
//! OpsItem a recorded unit happened to delete; the rows of the refusal table differ along
//! the operation that cannot resolve the id and along the id itself.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssm_amazonssm::state::ops_items::OPS_ITEM_DELETE_SETTLES_AFTER;
use service_com_amazonaws_ssm_amazonssm::state::ComAmazonawsSsmAmazonssmState;
use smithy_server_runtime::MultiServiceRouter;

use crate::ops_items::{create_ok, minimal};
use crate::parameter_store::{
    call, call_in_region, error_code, error_message, router, router_with_engine, REGION,
};

/// The window a delete of an OpsItem settles in
/// (`crate::state::ops_items::OPS_ITEM_DELETE_SETTLES_AFTER`), which the service stores as a
/// due instant rather than sleeping for.
const SETTLES_AFTER: Duration = OPS_ITEM_DELETE_SETTLES_AFTER;

/// How long the one polling case waits for a delete to settle before it gives up.
///
/// Deliberately far longer than [`SETTLES_AFTER`], so a loaded host cannot fail a case that
/// an idle one passes: what the case asserts is that the delete settles on its own, not how
/// long it takes.
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);

/// How long that case waits between two looks while it polls.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The error the operations of this surface report for an OpsItem id they cannot resolve,
/// which is the exception the model declares for them (`smithy_query(query="operation",
/// operation="GetOpsItem")`: `OpsItemNotFoundException`, a client error, httpError 400).
const NOT_FOUND_CODE: &str = "OpsItemNotFoundException";

/// The HTTP status that refusal carries (the same `httpError`).
const NOT_FOUND_STATUS: u16 = 400;

/// The fixed text Systems Manager words that refusal with, ahead of the envelope its SDKs
/// read the service, the status, the error code and the request id out of.
const NOT_FOUND_BODY: &str = "OpsItem not found.";

/// The refusal an edit of an OpsItem whose delete has not settled yet carries, which is the
/// exception the model declares for `UpdateOpsItem` for that state
/// (`smithy_query(query="operation", operation="UpdateOpsItem")`:
/// `OpsItemConflictException`, "The specified OpsItem is in the process of being deleted").
const CONFLICT_CODE: &str = "OpsItemConflictException";

/// The service the envelope names, which is the service shape the request targets
/// (`AmazonSSM.<Operation>`, this service's `X-Amz-Target`).
const ENVELOPE_SERVICE: &str = "AmazonSSM";

/// Well-formed OpsItem ids nothing ever minted, spanning the modelled `OpsItemId` pattern
/// `^(oi)-[0-9a-f]{12}$` (`smithy_query(query="operation", operation="GetOpsItem")`): its
/// lowest and highest digits, and two in between.
const UNMINTED_IDS: &[&str] = &[
    "oi-000000000000",
    "oi-ffffffffffff",
    "oi-0123456789ab",
    "oi-deadbeef0123",
];

/// Settle every delete the store owes, as the clock will once [`SETTLES_AFTER`] has passed.
///
/// The removal is materialised from the stored due instant, so moving the instant the store
/// is asked about is what a case does instead of waiting for the window — nothing sleeps.
fn settle_deletes(engine: &Arc<engine::InMemoryEngine>) {
    engine
        .state::<ComAmazonawsSsmAmazonssmState>()
        .lock()
        .settle_ops_item_deletions_due(SystemTime::now() + SETTLES_AFTER);
}

/// `DeleteOpsItem` of `ops_item_id` at `region`'s endpoint, answering `(status, body)`.
async fn delete_in(router: &MultiServiceRouter, region: &str, ops_item_id: &str) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "DeleteOpsItem",
        &json!({ "OpsItemId": ops_item_id }),
    )
    .await
}

/// `DeleteOpsItem` at `region`, asserting it succeeded and reported no members of its own:
/// the modelled `DeleteOpsItemResponse` carries none, so what a delete did is read back
/// through `GetOpsItem` rather than out of its own response.
async fn delete_ok(router: &MultiServiceRouter, case: &str, region: &str, ops_item_id: &str) {
    let (status, response) = delete_in(router, region, ops_item_id).await;
    assert_eq!(
        status, 200,
        "case {case}: DeleteOpsItem {ops_item_id} must succeed, got {status} {response}"
    );
    assert_eq!(
        response.as_object().map(serde_json::Map::len),
        Some(0),
        "case {case}: DeleteOpsItem reports no members, got {response}"
    );
}

/// `GetOpsItem` of `ops_item_id` at `region`, answering `(status, body)`.
async fn get_in(router: &MultiServiceRouter, region: &str, ops_item_id: &str) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "GetOpsItem",
        &json!({ "OpsItemId": ops_item_id }),
    )
    .await
}

/// The request id the refusal's envelope names, after holding the whole message against the
/// wording the class carries: the fixed sentence, then the service, the status and the error
/// code the response itself carries, then the request id, then the closing field.
///
/// Everything but the request id is fixed, so the request id is what this hands back for the
/// caller to hold against another refusal's.
fn request_id_of(case: &str, status: u16, response: &Value) -> String {
    let message = error_message(response);
    let (body, envelope) = message.split_once(" (").unwrap_or_else(|| {
        panic!("case {case}: expected a sentence then an envelope, got {message:?}")
    });
    assert_eq!(
        body, NOT_FOUND_BODY,
        "case {case}: expected the sentence {NOT_FOUND_BODY:?}, got {body:?}"
    );
    let envelope = envelope.strip_suffix(')').unwrap_or_else(|| {
        panic!("case {case}: expected the envelope to be closed, got {message:?}")
    });
    let fields: Vec<&str> = envelope.split("; ").collect();
    // Field 3 is the request id, which is the serving call's own and so is held against its
    // form rather than against a value.
    let expected: Vec<String> = vec![
        format!("Service: {ENVELOPE_SERVICE}"),
        format!("Status Code: {status}"),
        format!("Error Code: {}", error_code(response)),
        String::new(),
        "Proxy: null".to_string(),
    ];
    assert_eq!(
        fields.len(),
        expected.len(),
        "case {case}: expected {} envelope fields, got {fields:?}",
        expected.len()
    );
    for (index, want) in expected.iter().enumerate() {
        if want.is_empty() {
            continue;
        }
        assert_eq!(
            fields[index], want,
            "case {case}: expected envelope field {index} to be {want:?}, got {:?}",
            fields[index]
        );
    }
    let request_id = fields[3]
        .strip_prefix("Request ID: ")
        .unwrap_or_else(|| panic!("case {case}: expected a Request ID field, got {fields:?}"));
    // A request id is minted per call as a hyphenated UUID, so the field carries one of that
    // form rather than anything of the request.
    assert_eq!(
        request_id.len(),
        36,
        "case {case}: expected a hyphenated UUID request id, got {request_id:?}"
    );
    assert!(
        request_id
            .chars()
            .all(|c| (c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) || c == '-'),
        "case {case}: expected a hyphenated UUID request id, got {request_id:?}"
    );
    request_id.to_string()
}

/// Assert that `response` is the refusal of an OpsItem id nothing is held under, answering
/// the request id its envelope named.
fn assert_not_found(case: &str, status: u16, response: &Value) -> String {
    assert_eq!(
        status, NOT_FOUND_STATUS,
        "case {case}: expected status {NOT_FOUND_STATUS}, got {status} {response}"
    );
    assert_eq!(
        error_code(response),
        NOT_FOUND_CODE,
        "case {case}: expected {NOT_FOUND_CODE}, got {response}"
    );
    request_id_of(case, status, response)
}

/// What is done to an OpsItem before the delete reaches it.
struct DeleteCase<'a> {
    case: &'a str,
    /// The Region the OpsItem is created in, deleted through and read back at.
    region: &'a str,
    /// The create's body.
    body: Value,
    /// The edits made before the delete, in order.
    edits: Vec<Value>,
    /// Whether a related item is associated with the OpsItem before the delete.
    related: bool,
}

/// Every form of OpsItem is gone once its delete has settled: the read of it takes the
/// not-found path, the related items held against it are gone with it, no edit of it is
/// served from the delete on, and a repeated delete is served the way the first was.
#[tokio::test]
async fn a_deleted_ops_item_is_no_longer_held() {
    let (router, engine) = router_with_engine();
    let mut data = minimal("An OpsItem carrying operational data");
    data["OperationalData"] = json!({
        "team": { "Value": "platform", "Type": "SearchableString" },
        "runbook": { "Value": "restart the agent", "Type": "String" }
    });
    let cases = vec![
        DeleteCase {
            case: "only the required members",
            region: REGION,
            body: minimal("An OpsItem nobody worked"),
            edits: Vec::new(),
            related: false,
        },
        DeleteCase {
            case: "edited twice before the delete",
            region: REGION,
            body: minimal("An OpsItem worked twice"),
            edits: vec![
                json!({ "Status": "InProgress" }),
                json!({ "Priority": 2, "Severity": "2" }),
            ],
            related: false,
        },
        DeleteCase {
            case: "resolved before the delete",
            region: REGION,
            body: minimal("An OpsItem worked to a close"),
            edits: vec![json!({ "Status": "Resolved" })],
            related: false,
        },
        DeleteCase {
            case: "carrying operational data",
            region: REGION,
            body: data,
            edits: Vec::new(),
            related: false,
        },
        DeleteCase {
            case: "carrying a related item",
            region: REGION,
            body: minimal("An OpsItem with a related resource"),
            edits: Vec::new(),
            related: true,
        },
        DeleteCase {
            case: "in another Region",
            region: "eu-central-1",
            body: minimal("An OpsItem of another Region"),
            edits: vec![json!({ "Status": "InProgress" })],
            related: true,
        },
    ];

    for DeleteCase {
        case,
        region,
        body,
        edits,
        related,
    } in cases
    {
        let (status, created) = call_in_region(&router, region, "CreateOpsItem", &body).await;
        assert_eq!(
            status, 200,
            "case {case}: CreateOpsItem must succeed, got {status} {created}"
        );
        let id = created["OpsItemId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: no OpsItemId in {created}"))
            .to_string();
        for (step, patch) in edits.iter().enumerate() {
            let mut edit = patch.clone();
            edit["OpsItemId"] = json!(id);
            let (status, response) = call_in_region(&router, region, "UpdateOpsItem", &edit).await;
            assert_eq!(
                status, 200,
                "case {case}: edit {} must succeed, got {status} {response}",
                step + 1
            );
        }
        if related {
            let (status, response) = call_in_region(
                &router,
                region,
                "AssociateOpsItemRelatedItem",
                &json!({
                    "OpsItemId": id,
                    "AssociationType": "RelatesTo",
                    "ResourceType": "AWS::SSM::Document",
                    "ResourceUri": format!("arn:aws:ssm:{region}:123456789012:document/AWS-RunShellScript"),
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: AssociateOpsItemRelatedItem must succeed, got {status} {response}"
            );
        }

        // The OpsItem is there to be deleted.
        let (status, response) = get_in(&router, region, &id).await;
        assert_eq!(
            status, 200,
            "case {case}: the OpsItem must be readable before its delete, got {status} \
             {response}"
        );

        delete_ok(&router, case, region, &id).await;

        // An edit of an OpsItem a delete has reached is never served: while the delete is
        // settling the OpsItem "is in the process of being deleted", and once it has
        // settled the id names no OpsItem at all.
        let (status, response) = call_in_region(
            &router,
            region,
            "UpdateOpsItem",
            &json!({ "OpsItemId": id, "Status": "Open" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: an edit of a deleted OpsItem is refused, got {status} {response}"
        );
        let refusal = error_code(&response);
        assert!(
            [CONFLICT_CODE, NOT_FOUND_CODE].contains(&refusal.as_str()),
            "case {case}: expected the edit to be refused as a conflict or as not found, got \
             {response}"
        );

        settle_deletes(&engine);

        let (status, response) = get_in(&router, region, &id).await;
        assert_not_found(case, status, &response);

        // What hung off the OpsItem is gone with it: an association is reached only through
        // the OpsItem it belongs to.
        let (status, listed) = call_in_region(
            &router,
            region,
            "ListOpsItemRelatedItems",
            &json!({ "OpsItemId": id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: ListOpsItemRelatedItems must succeed, got {status} {listed}"
        );
        assert_eq!(
            listed["Summaries"].as_array().map(Vec::len).unwrap_or(0),
            0,
            "case {case}: expected no related items of a deleted OpsItem, got {listed}"
        );

        // "This operation is idempotent. The system doesn't throw an exception if you
        // repeatedly call this operation for the same OpsItem" (`DeleteOpsItem`'s doc), and
        // a repeated delete brings the OpsItem back no more than the first one kept it.
        delete_ok(&router, case, region, &id).await;
        let (status, response) = get_in(&router, region, &id).await;
        assert_not_found(case, status, &response);

        // An edit of it, and an association against it, are refused the same way.
        for (operation, request) in [
            ("UpdateOpsItem", json!({ "OpsItemId": id, "Status": "Open" })),
            (
                "AssociateOpsItemRelatedItem",
                json!({
                    "OpsItemId": id,
                    "AssociationType": "RelatesTo",
                    "ResourceType": "AWS::SSM::Document",
                    "ResourceUri": format!("arn:aws:ssm:{region}:123456789012:document/AWS-RunPatchBaseline"),
                }),
            ),
        ] {
            let (status, response) = call_in_region(&router, region, operation, &request).await;
            assert_not_found(&format!("{case}: {operation}"), status, &response);
        }
    }
}

/// A delete of an id the Region holds no OpsItem under is served the same way as one that
/// reached a record — the operation models no not-found error — and marks nothing: the
/// OpsItem another Region holds under that id survives a delete aimed at this one.
#[tokio::test]
async fn a_delete_marks_nothing_outside_its_own_region() {
    let (router, engine) = router_with_engine();
    let created = create_ok(
        &router,
        "the OpsItem under test",
        &minimal("A regional OpsItem"),
    )
    .await;
    let held = created["OpsItemId"]
        .as_str()
        .unwrap_or_else(|| panic!("no OpsItemId in {created}"))
        .to_string();

    // The same id at every other Region's endpoint, and ids nothing ever minted at this
    // one: each delete is served, and the OpsItem this Region holds is untouched.
    for region in ["us-east-1", "eu-central-1", "ap-southeast-2"] {
        delete_ok(&router, region, region, &held).await;
    }
    for id in UNMINTED_IDS {
        delete_ok(&router, id, REGION, id).await;
    }
    settle_deletes(&engine);
    let (status, response) = get_in(&router, REGION, &held).await;
    assert_eq!(
        status, 200,
        "a delete aimed elsewhere leaves this Region's OpsItem readable, got {status} \
         {response}"
    );
    assert_eq!(
        response["OpsItem"]["OpsItemId"].as_str(),
        Some(held.as_str()),
        "expected the OpsItem the id names, got {response}"
    );

    // And once it is deleted through its OWN Region's endpoint, it is gone.
    delete_ok(&router, "its own Region", REGION, &held).await;
    settle_deletes(&engine);
    let (status, response) = get_in(&router, REGION, &held).await;
    assert_not_found("its own Region", status, &response);
}

/// A delete settles without anything asking it to: nothing but the clock moves the removal
/// on, so a client that only reads sees the OpsItem go.
///
/// The one case that waits for the real clock. It looks again until the read is refused
/// rather than waiting out the window, up to a deadline far longer than it.
#[tokio::test]
async fn a_delete_settles_without_anything_asking_it_to() {
    let router = router();
    let created = create_ok(
        &router,
        "an OpsItem to delete",
        &minimal("An OpsItem whose delete settles"),
    )
    .await;
    let id = created["OpsItemId"]
        .as_str()
        .unwrap_or_else(|| panic!("no OpsItemId in {created}"))
        .to_string();
    delete_ok(&router, "an OpsItem to delete", REGION, &id).await;

    let deadline = Instant::now() + SETTLE_DEADLINE;
    let (status, response) = loop {
        let (status, response) = get_in(&router, REGION, &id).await;
        if status != 200 {
            break (status, response);
        }
        assert!(
            Instant::now() < deadline,
            "expected the delete of {id} to settle and the read to be refused, still \
             answering {status} {response}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    };
    assert_not_found("an OpsItem to delete", status, &response);
}

/// One row of the refusal table: an operation of this surface that has to resolve an
/// OpsItem id, and the body it is called with for a given id.
type RefusalCase = (&'static str, fn(&str) -> Value);

/// Every operation that cannot resolve an OpsItem id refuses it the way Systems Manager
/// words that refusal: the fixed sentence, then the envelope naming the service, the status
/// and the error code the response carries and the request id of the serving call — which is
/// the only part that varies, and varies per call.
#[tokio::test]
async fn an_unresolvable_ops_item_id_is_refused_as_the_service_words_it() {
    let (router, engine) = router_with_engine();
    // One id that WAS held and whose delete has settled, beside ids nothing ever minted:
    // the refusal is the same however the id came to name nothing.
    let created = create_ok(&router, "a deleted OpsItem", &minimal("A deleted OpsItem")).await;
    let deleted = created["OpsItemId"]
        .as_str()
        .unwrap_or_else(|| panic!("no OpsItemId in {created}"))
        .to_string();
    delete_ok(&router, "a deleted OpsItem", REGION, &deleted).await;
    settle_deletes(&engine);

    // Each row is one operation of this surface that has to resolve an OpsItem id, with a
    // body that satisfies every other rule it holds a request to.
    let operations: &[RefusalCase] = &[
        ("GetOpsItem", |id| json!({ "OpsItemId": id })),
        ("UpdateOpsItem", |id| {
            json!({ "OpsItemId": id, "Status": "InProgress" })
        }),
        ("AssociateOpsItemRelatedItem", |id| {
            json!({
                "OpsItemId": id,
                "AssociationType": "RelatesTo",
                "ResourceType": "AWS::SSM::Document",
                "ResourceUri": "arn:aws:ssm:us-west-2:123456789012:document/AWS-RunShellScript",
            })
        }),
    ];
    let mut request_ids: Vec<String> = Vec::new();
    for (operation, body) in operations {
        for id in UNMINTED_IDS.iter().chain([deleted.as_str()].iter()) {
            let case = format!("{operation} of {id}");
            let (status, response) = call(&router, operation, &body(id)).await;
            request_ids.push(assert_not_found(&case, status, &response));
        }
    }

    // The request id is the serving call's own, so no two refusals carry one.
    let mut unique = request_ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        request_ids.len(),
        "expected one request id per refusal, got {request_ids:?}"
    );
}
