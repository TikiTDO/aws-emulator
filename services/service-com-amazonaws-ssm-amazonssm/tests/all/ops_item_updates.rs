// SPDX-License-Identifier: CC0-1.0

//! Editing an OpsItem end to end: `UpdateOpsItem` writes what the request names,
//! `GetOpsItem` reports it, and `ListOpsItemEvents` reports the trail the edits left.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`call`], [`call_in_region`], [`error_code`], [`router`])
//! dispatches `POST /` with `X-Amz-Target: AmazonSSM.<Operation>` through the
//! multi-service router — and every OpsItem an edit is made to is opened through
//! `all/ops_items.rs`'s harness ([`create_ok`], [`get_ok`], [`minimal`]). Nothing reaches
//! into the state struct, so what an edit is read back as is what it persisted.
//!
//! The suites are tables. Each row is one input along the axis the rule under test
//! generalises over — which members the edit names, which status it moves the OpsItem to,
//! which operational-data keys it adds and removes, how many edits it takes, the Region —
//! rather than the single edit a recorded unit happened to make.

use serde_json::{json, Value};
use service_com_amazonaws_ssm_amazonssm::state::ops_item_events::{
    written_detail, DETAIL_TYPE_UPDATE,
};
use service_com_amazonaws_ssm_amazonssm::state::ops_items::MAX_OPERATIONAL_DATA_KEYS as MAX_DATA_KEYS;
use smithy_server_runtime::MultiServiceRouter;

use crate::ops_items::{create_ok, get_ok, minimal, ops_item_arn};
use crate::parameter_store::{call, call_in_region, error_code, router, ACCOUNT, REGION};

/// Every value the modelled `OpsItemStatus` enum declares (`smithy_query(query="shape",
/// shape="OpsItemStatus")`), and so every status an edit may move an OpsItem to.
const OPS_ITEM_STATUSES: &[&str] = &[
    "Open",
    "InProgress",
    "Resolved",
    "Pending",
    "TimedOut",
    "Cancelling",
    "Cancelled",
    "Failed",
    "CompletedWithSuccess",
    "CompletedWithFailure",
    "Scheduled",
    "RunbookInProgress",
    "PendingChangeCalendarOverride",
    "ChangeCalendarOverrideApproved",
    "ChangeCalendarOverrideRejected",
    "PendingApproval",
    "Approved",
    "Revoked",
    "Rejected",
    "Closed",
];

/// The `OpsItem` members every edit writes whatever it named: "each time the OpsItem is
/// edited the version number increments by one" (`OpsItem$Version`), beside who last
/// updated it and when (`OpsItem$LastModifiedBy`, `OpsItem$LastModifiedTime`).
const EDIT_STAMPS: &[&str] = &["Version", "LastModifiedTime", "LastModifiedBy"];

/// "The value has a maximum size of 20 KB" (`UpdateOpsItem`'s `OperationalData` doc).
const MAX_DATA_VALUE: usize = 20 * 1024;

/// The `Source` every OpsItem event of this service carries.
const EVENT_SOURCE: &str = "aws.ssm";

/// `UpdateOpsItem` with `body`, answering `(status, body)`.
async fn update(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "UpdateOpsItem", body).await
}

/// `UpdateOpsItem`, asserting it succeeded and reported no members of its own — the
/// modelled `UpdateOpsItemResponse` carries none, so an edit is read back through
/// `GetOpsItem` rather than out of its own response.
async fn update_ok(router: &MultiServiceRouter, case: &str, body: &Value) {
    let (status, response) = update(router, body).await;
    assert_eq!(
        status, 200,
        "case {case}: UpdateOpsItem {body} must succeed, got {status} {response}"
    );
    assert_eq!(
        response.as_object().map(serde_json::Map::len),
        Some(0),
        "case {case}: UpdateOpsItem reports no members, got {response}"
    );
}

/// The OpsItem id a create reported.
fn ops_item_id(created: &Value) -> String {
    created["OpsItemId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateOpsItem reports an OpsItemId, got {created}"))
        .to_string()
}

/// A create naming every optional member, so an edit of any one of them changes a value
/// the OpsItem already carries rather than filling a hole. The type is
/// `/aws/changerequest`, which is the type the four workflow times are reported for.
fn fully_populated(title: &str) -> Value {
    json!({
        "Title": title,
        "Description": format!("what {title} is about"),
        "Source": "emulator-tests",
        "OpsItemType": "/aws/changerequest",
        "Priority": 3,
        "Category": "Performance",
        "Severity": "3",
        "Notifications": [{ "Arn": format!("arn:aws:sns:{REGION}:{ACCOUNT}:ops-alerts") }],
        "RelatedOpsItems": [{ "OpsItemId": "oi-0000000000ab" }],
        "OperationalData": {
            "team": { "Value": "platform", "Type": "SearchableString" },
            "runbook": { "Value": "restart the agent", "Type": "String" }
        },
        "ActualStartTime": 1_700_000_000,
        "ActualEndTime": 1_700_003_600,
        "PlannedStartTime": 1_700_000_000,
        "PlannedEndTime": 1_700_007_200
    })
}

/// Whether two reported values are the one value, comparing numbers numerically: a
/// timestamp travels as epoch seconds, which JSON may carry as either an integer or a
/// fraction of one.
fn same(left: &Value, right: &Value) -> bool {
    match (left.as_f64(), right.as_f64()) {
        (Some(left), Some(right)) => left == right,
        _ => left == right,
    }
}

/// A member of a reported OpsItem as a number — its timestamps, for the cases that order
/// two of them.
fn number(item: &Value, member: &str) -> f64 {
    item[member]
        .as_f64()
        .unwrap_or_else(|| panic!("{member} is reported as a number, got {item}"))
}

/// The `Version` token a read reported, as the number it carries.
///
/// `OpsItem$Version` is a String, and what the service mints for it is a token that grows
/// with every write of the record, so two versions of one OpsItem are ordered by comparing
/// the numbers they spell rather than by counting edits.
fn version(item: &Value, case: &str) -> u64 {
    item["Version"]
        .as_str()
        .and_then(|token| token.parse().ok())
        .unwrap_or_else(|| panic!("case {case}: Version is reported as a number, got {item}"))
}

/// The events `ListOpsItemEvents` reports about ONE OpsItem, newest first.
async fn events_of(router: &MultiServiceRouter, case: &str, ops_item_id: &str) -> Vec<Value> {
    let request = json!({
        "Filters": [{ "Key": "OpsItemId", "Values": [ops_item_id], "Operator": "Equal" }]
    });
    let (status, body) = call(router, "ListOpsItemEvents", &request).await;
    assert_eq!(
        status, 200,
        "case {case}: ListOpsItemEvents {request} must succeed, got {status} {body}"
    );
    body["Summaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: ListOpsItemEvents reports Summaries, got {body}"))
        .clone()
}

/// One row of the edit table: the members the edit names, and the members the OpsItem must
/// report differently afterwards.
struct EditCase<'a> {
    case: &'a str,
    patch: Value,
    expected: Value,
}

/// An edit writes the members it names and leaves every other member as the OpsItem
/// already carried it, whichever members it names — and every edit advances the version
/// and the last-modified stamp.
#[tokio::test]
async fn an_edit_writes_the_members_it_names_and_leaves_the_rest() {
    let router = router();
    let cases = vec![
        EditCase {
            case: "the status alone",
            patch: json!({ "Status": "InProgress" }),
            expected: json!({ "Status": "InProgress" }),
        },
        EditCase {
            case: "the title alone",
            patch: json!({ "Title": "A disk filled up" }),
            expected: json!({ "Title": "A disk filled up" }),
        },
        EditCase {
            case: "the description alone",
            patch: json!({ "Description": "The root volume is at 97 percent." }),
            expected: json!({ "Description": "The root volume is at 97 percent." }),
        },
        EditCase {
            case: "the priority alone",
            patch: json!({ "Priority": 1 }),
            expected: json!({ "Priority": 1 }),
        },
        EditCase {
            case: "the category alone",
            patch: json!({ "Category": "Cost" }),
            expected: json!({ "Category": "Cost" }),
        },
        EditCase {
            case: "the severity alone",
            patch: json!({ "Severity": "1" }),
            expected: json!({ "Severity": "1" }),
        },
        EditCase {
            case: "the notifications, replaced",
            patch: json!({
                "Notifications": [{ "Arn": format!("arn:aws:sns:{REGION}:{ACCOUNT}:page-the-oncall") }]
            }),
            expected: json!({
                "Notifications": [{ "Arn": format!("arn:aws:sns:{REGION}:{ACCOUNT}:page-the-oncall") }]
            }),
        },
        EditCase {
            case: "the notifications, cleared",
            patch: json!({ "Notifications": [] }),
            expected: json!({ "Notifications": [] }),
        },
        EditCase {
            case: "the related OpsItems",
            patch: json!({
                "RelatedOpsItems": [
                    { "OpsItemId": "oi-00000000cafe" },
                    { "OpsItemId": "oi-00000000beef" }
                ]
            }),
            expected: json!({
                "RelatedOpsItems": [
                    { "OpsItemId": "oi-00000000cafe" },
                    { "OpsItemId": "oi-00000000beef" }
                ]
            }),
        },
        EditCase {
            case: "one workflow time",
            patch: json!({ "ActualEndTime": 1_700_009_999_i64 }),
            expected: json!({ "ActualEndTime": 1_700_009_999_i64 }),
        },
        EditCase {
            case: "all four workflow times",
            patch: json!({
                "ActualStartTime": 1_700_100_000_i64,
                "ActualEndTime": 1_700_103_600_i64,
                "PlannedStartTime": 1_700_100_000_i64,
                "PlannedEndTime": 1_700_107_200_i64
            }),
            expected: json!({
                "ActualStartTime": 1_700_100_000_i64,
                "ActualEndTime": 1_700_103_600_i64,
                "PlannedStartTime": 1_700_100_000_i64,
                "PlannedEndTime": 1_700_107_200_i64
            }),
        },
        EditCase {
            case: "an operational-data key",
            patch: json!({
                "OperationalData": { "team": { "Value": "networking", "Type": "String" } }
            }),
            expected: json!({
                "OperationalData": {
                    "team": { "Value": "networking", "Type": "String" },
                    "runbook": { "Value": "restart the agent", "Type": "String" }
                }
            }),
        },
        EditCase {
            case: "several members at once",
            patch: json!({
                "Status": "Resolved",
                "Title": "The disk was resized",
                "Priority": 5,
                "Severity": "4"
            }),
            expected: json!({
                "Status": "Resolved",
                "Title": "The disk was resized",
                "Priority": 5,
                "Severity": "4"
            }),
        },
        EditCase {
            case: "no member beyond the id",
            patch: json!({}),
            expected: json!({}),
        },
    ];

    for EditCase {
        case,
        patch,
        expected,
    } in cases
    {
        let created = create_ok(&router, case, &fully_populated(case)).await;
        let id = ops_item_id(&created);
        let before = get_ok(&router, case, &id).await;

        let mut body = patch.clone();
        body["OpsItemId"] = json!(id);
        update_ok(&router, case, &body).await;
        let after = get_ok(&router, case, &id).await;

        let expected_members = expected
            .as_object()
            .unwrap_or_else(|| panic!("case {case}: the row's expectation is an object"));
        for (member, want) in expected_members {
            assert!(
                same(&after[member], want),
                "case {case}: expected {member} to be {want}, got {}",
                after[member]
            );
        }
        for member in before
            .as_object()
            .unwrap_or_else(|| panic!("case {case}: GetOpsItem reports an OpsItem, got {before}"))
            .keys()
        {
            if expected_members.contains_key(member) || EDIT_STAMPS.contains(&member.as_str()) {
                continue;
            }
            assert_eq!(
                before[member], after[member],
                "case {case}: {member} is not what the edit named, so it must survive it: \
                 was {}, now {}",
                before[member], after[member]
            );
        }
        assert!(
            version(&after, case) > version(&before, case),
            "case {case}: one edit must leave a version past the one it replaced, was {}, now {}",
            before["Version"],
            after["Version"]
        );
        assert!(
            number(&after, "LastModifiedTime") >= number(&before, "LastModifiedTime"),
            "case {case}: the edit's instant is not before the create's, got {} after {}",
            after["LastModifiedTime"],
            before["LastModifiedTime"]
        );
        assert_eq!(
            after["LastModifiedBy"], before["CreatedBy"],
            "case {case}: the edit names the principal that made it, got {}",
            after["LastModifiedBy"]
        );
    }
}

/// Every status the model declares is a status an edit moves an OpsItem to, and a later
/// read reports it: the status an OpsItem carries is the one the last edit wrote.
#[tokio::test]
async fn every_modelled_status_is_an_edit_the_service_persists() {
    let router = router();
    for status in OPS_ITEM_STATUSES {
        let created = create_ok(
            &router,
            status,
            &minimal(&format!("An OpsItem moving to {status}")),
        )
        .await;
        let id = ops_item_id(&created);
        let before = get_ok(&router, status, &id).await;
        assert_eq!(
            before["Status"].as_str(),
            Some("Open"),
            "case {status}: a created OpsItem is open, got {}",
            before["Status"]
        );

        update_ok(
            &router,
            status,
            &json!({ "OpsItemId": id, "Status": status }),
        )
        .await;
        let after = get_ok(&router, status, &id).await;
        assert_eq!(
            after["Status"].as_str(),
            Some(*status),
            "case {status}: expected the status the edit named, got {}",
            after["Status"]
        );
        assert!(
            version(&after, status) > version(&before, status),
            "case {status}: the status edit must advance the version past {}, got {}",
            before["Version"],
            after["Version"]
        );
    }
}

/// A status the model does not declare is refused rather than persisted, whatever it looks
/// like: the status an OpsItem carries is one of the modelled values.
#[tokio::test]
async fn a_status_the_model_does_not_declare_is_refused() {
    let router = router();
    // Values no `OpsItemStatus` member spells: a plausible neighbour of one, a differently
    // cased one, and a value of another enum entirely.
    for status in ["Resolving", "inprogress", "OPEN", "Done", "CompletedWithWarning"] {
        let created = create_ok(&router, status, &minimal(&format!("Not {status}"))).await;
        let id = ops_item_id(&created);
        let before = get_ok(&router, status, &id).await;
        let (code, response) = update(&router, &json!({ "OpsItemId": id, "Status": status })).await;
        assert_ne!(
            code, 200,
            "case {status}: an unmodelled status is refused, got {code} {response}"
        );
        let read = get_ok(&router, status, &id).await;
        assert_eq!(
            read["Status"].as_str(),
            Some("Open"),
            "case {status}: a refused edit writes nothing, got {read}"
        );
        assert_eq!(
            version(&read, status),
            version(&before, status),
            "case {status}: a refused edit is no edit, so the version stands at {}, got {read}",
            before["Version"]
        );
    }
}

/// The `Version` an OpsItem reports is a token the service re-derives at every write of the
/// record: a read either side of an edit reports two tokens and the later one is the
/// greater, whatever members the edit named — while a read on its own moves nothing.
#[tokio::test]
async fn the_version_token_is_re_derived_at_every_write() {
    let router = router();
    // Each row is one edit along the axis the rule generalises over: which member it
    // writes, how many it writes at once, an edit that only removes operational data, an
    // edit that writes a member back to the value the OpsItem already holds, and an edit
    // that names no member at all.
    let edits = [
        json!({ "Status": "InProgress" }),
        json!({ "Title": "The same OpsItem, retitled" }),
        json!({
            "OperationalData": { "pager": { "Value": "page the on-call", "Type": "String" } }
        }),
        json!({ "OperationalDataToDelete": ["pager"] }),
        json!({ "Priority": 1, "Category": "Recovery", "Severity": "1" }),
        json!({ "Status": "InProgress" }),
        json!({}),
    ];
    let created = create_ok(
        &router,
        "a versioned OpsItem",
        &fully_populated("A versioned OpsItem"),
    )
    .await;
    let id = ops_item_id(&created);
    let mut previous = version(&get_ok(&router, "the create", &id).await, "the create");
    for (step, patch) in edits.iter().enumerate() {
        let case = format!("edit {} naming {patch}", step + 1);
        let mut body = patch.clone();
        body["OpsItemId"] = json!(id);
        update_ok(&router, &case, &body).await;
        let reported = version(&get_ok(&router, &case, &id).await, &case);
        assert!(
            reported > previous,
            "case {case}: expected a version token past {previous}, got {reported}"
        );
        previous = reported;
    }
    // Reading an OpsItem is not a write of it, so the token a read reports stands until the
    // next edit.
    for attempt in 1..=3 {
        let case = format!("read {attempt} with no edit between");
        let reported = version(&get_ok(&router, &case, &id).await, &case);
        assert_eq!(
            reported, previous,
            "case {case}: expected the version token to stand at {previous}, got {reported}"
        );
    }
}

/// Consecutive edits each advance the version past the one before it and each leave the
/// OpsItem in the status that edit named, however many of them there are.
#[tokio::test]
async fn consecutive_edits_advance_the_version_once_each() {
    let router = router();
    let created = create_ok(
        &router,
        "a worked OpsItem",
        &minimal("An OpsItem worked to completion"),
    )
    .await;
    let id = ops_item_id(&created);
    let mut previous = version(&get_ok(&router, "the create", &id).await, "the create");
    // A walk through the statuses a work item passes through, each one an edit.
    for (step, status) in ["InProgress", "Pending", "InProgress", "Resolved", "Closed"]
        .iter()
        .enumerate()
    {
        let case = format!("edit {} to {status}", step + 1);
        update_ok(&router, &case, &json!({ "OpsItemId": id, "Status": status })).await;
        let read = get_ok(&router, &case, &id).await;
        assert_eq!(
            read["Status"].as_str(),
            Some(*status),
            "case {case}: expected the status this edit named, got {}",
            read["Status"]
        );
        let reported = version(&read, &case);
        assert!(
            reported > previous,
            "case {case}: expected a version past {previous}, got {reported}"
        );
        previous = reported;
    }
}

/// One row of the operational-data table: what the OpsItem holds, what the edit adds, what
/// it deletes, and the map the OpsItem must hold afterwards.
struct DataCase<'a> {
    case: &'a str,
    held: Value,
    additions: Value,
    deletions: Vec<&'a str>,
    expected: Value,
}

/// An edit of the `OperationalData` map is a patch of it: the keys the edit supplies are
/// added or overwritten, the keys it names for deletion are removed, and every other key
/// the OpsItem holds is left alone.
#[tokio::test]
async fn an_edit_patches_the_operational_data_map_key_by_key() {
    let router = router();
    let held = json!({
        "team": { "Value": "platform", "Type": "SearchableString" },
        "runbook": { "Value": "restart the agent", "Type": "String" }
    });
    let cases = vec![
        DataCase {
            case: "a new key is added",
            held: held.clone(),
            additions: json!({ "ticket": { "Value": "T-1234", "Type": "String" } }),
            deletions: Vec::new(),
            expected: json!({
                "team": { "Value": "platform", "Type": "SearchableString" },
                "runbook": { "Value": "restart the agent", "Type": "String" },
                "ticket": { "Value": "T-1234", "Type": "String" }
            }),
        },
        DataCase {
            case: "an existing key is overwritten",
            held: held.clone(),
            additions: json!({ "team": { "Value": "networking", "Type": "String" } }),
            deletions: Vec::new(),
            expected: json!({
                "team": { "Value": "networking", "Type": "String" },
                "runbook": { "Value": "restart the agent", "Type": "String" }
            }),
        },
        DataCase {
            case: "one key is removed",
            held: held.clone(),
            additions: json!({}),
            deletions: vec!["runbook"],
            expected: json!({ "team": { "Value": "platform", "Type": "SearchableString" } }),
        },
        DataCase {
            case: "every key is removed",
            held: held.clone(),
            additions: json!({}),
            deletions: vec!["team", "runbook"],
            expected: json!({}),
        },
        DataCase {
            case: "one key removed and another added",
            held: held.clone(),
            additions: json!({ "owner": { "Value": "the on-call", "Type": "String" } }),
            deletions: vec!["runbook"],
            expected: json!({
                "team": { "Value": "platform", "Type": "SearchableString" },
                "owner": { "Value": "the on-call", "Type": "String" }
            }),
        },
        DataCase {
            case: "a key both removed and supplied keeps the supplied value",
            held: held.clone(),
            additions: json!({ "team": { "Value": "storage", "Type": "String" } }),
            deletions: vec!["team"],
            expected: json!({
                "team": { "Value": "storage", "Type": "String" },
                "runbook": { "Value": "restart the agent", "Type": "String" }
            }),
        },
        DataCase {
            case: "removing a key the map never held changes nothing",
            held: held.clone(),
            additions: json!({}),
            deletions: vec!["nobody-wrote-this"],
            expected: held.clone(),
        },
        DataCase {
            case: "an edit naming neither leaves the map",
            held: held.clone(),
            additions: json!({}),
            deletions: Vec::new(),
            expected: held.clone(),
        },
        DataCase {
            case: "a key the service defines may be added",
            held: json!({}),
            additions: json!({
                "/aws/resources": {
                    "Value": "[{\"arn\":\"arn:aws:s3:::a-bucket\"}]",
                    "Type": "SearchableString"
                }
            }),
            deletions: Vec::new(),
            expected: json!({
                "/aws/resources": {
                    "Value": "[{\"arn\":\"arn:aws:s3:::a-bucket\"}]",
                    "Type": "SearchableString"
                }
            }),
        },
        DataCase {
            case: "a value of exactly the published size limit",
            held: json!({}),
            additions: json!({ "payload": { "Value": "p".repeat(MAX_DATA_VALUE), "Type": "String" } }),
            deletions: Vec::new(),
            expected: json!({ "payload": { "Value": "p".repeat(MAX_DATA_VALUE), "Type": "String" } }),
        },
    ];

    for DataCase {
        case,
        held,
        additions,
        deletions,
        expected,
    } in cases
    {
        let mut create = minimal(case);
        create["OperationalData"] = held;
        let created = create_ok(&router, case, &create).await;
        let id = ops_item_id(&created);

        let mut body = json!({ "OpsItemId": id });
        if !additions.as_object().is_some_and(serde_json::Map::is_empty) {
            body["OperationalData"] = additions;
        }
        if !deletions.is_empty() {
            body["OperationalDataToDelete"] = json!(deletions);
        }
        update_ok(&router, case, &body).await;

        let read = get_ok(&router, case, &id).await;
        assert_eq!(
            read["OperationalData"], expected,
            "case {case}: expected the patched map {expected}, got {}",
            read["OperationalData"]
        );
    }
}

/// Each edit appends exactly one event to the OpsItem's trail, carrying what that edit left
/// persisted; an OpsItem nobody has edited has no events, and one OpsItem's edits are not
/// in another's trail.
#[tokio::test]
async fn each_edit_appends_one_event_to_the_trail() {
    let router = router();
    let edited = ops_item_id(
        &create_ok(&router, "the edited OpsItem", &minimal("An OpsItem to work")).await,
    );
    let untouched = ops_item_id(
        &create_ok(&router, "the untouched OpsItem", &minimal("An OpsItem nobody works")).await,
    );

    // Each row is one more edit on the same OpsItem, so the trail is checked at every
    // length rather than only at its last.
    let statuses = ["InProgress", "Resolved", "Open", "Closed"];
    // The version token each edit left on the OpsItem, which is what the event that edit
    // appended records: read back through `GetOpsItem` rather than counted, because the
    // token is the service's to mint.
    let mut versions: Vec<u64> = Vec::new();
    for (step, status) in statuses.iter().enumerate() {
        let case = format!("after {} edit(s)", step + 1);
        update_ok(
            &router,
            &case,
            &json!({ "OpsItemId": edited, "Status": status }),
        )
        .await;
        versions.push(version(&get_ok(&router, &case, &edited).await, &case));

        let trail = events_of(&router, &case, &edited).await;
        assert_eq!(
            trail.len(),
            step + 1,
            "case {case}: expected one event per edit, got {trail:?}"
        );
        // Newest first, so the trail reads as the reverse of the edits made.
        for (age, event) in trail.iter().enumerate() {
            let version = versions[step - age];
            let wrote = statuses[step - age];
            assert_eq!(
                event["Detail"].as_str(),
                Some(written_detail(version, wrote).as_str()),
                "case {case}: event {age} back records version {version} and status {wrote}, \
                 got {event}"
            );
            assert_eq!(
                event["OpsItemId"].as_str(),
                Some(edited.as_str()),
                "case {case}: an event names the OpsItem it is about, got {event}"
            );
            assert_eq!(
                event["Source"].as_str(),
                Some(EVENT_SOURCE),
                "case {case}: expected source {EVENT_SOURCE}, got {event}"
            );
            assert_eq!(
                event["DetailType"].as_str(),
                Some(DETAIL_TYPE_UPDATE),
                "case {case}: expected detail type {DETAIL_TYPE_UPDATE}, got {event}"
            );
            assert!(
                event["CreatedBy"]["Arn"]
                    .as_str()
                    .is_some_and(|arn| arn.starts_with("arn:")),
                "case {case}: an event names the identity that made the edit, got {event}"
            );
        }
        assert!(
            events_of(&router, &case, &untouched).await.is_empty(),
            "case {case}: an OpsItem nobody edited has no events"
        );
    }
}

/// One OpsItem's operational-data map is bounded, and the bound is about the map an edit
/// leaves behind rather than the keys one edit names: keys accumulate across edits, an edit
/// that leaves the map at the bound is served, one that would take it past the bound is
/// refused and writes nothing, and removing a key makes room for another.
#[tokio::test]
async fn one_ops_items_operational_data_map_is_bounded() {
    let router = router();
    let case = "a map filled one edit at a time";
    let created = create_ok(&router, case, &minimal("An OpsItem that collects data")).await;
    let id = ops_item_id(&created);

    /// The operational-data entry for the key numbered `index`.
    fn entry(index: usize) -> (String, Value) {
        (
            format!("key-{index}"),
            json!({ "Value": format!("value {index}"), "Type": "String" }),
        )
    }

    // The bound is reached by accumulation over several edits, which is the growth one
    // request's body cannot show.
    let per_edit = MAX_DATA_KEYS / 4;
    let mut added = 0;
    while added < MAX_DATA_KEYS {
        let upto = (added + per_edit).min(MAX_DATA_KEYS);
        let data: serde_json::Map<String, Value> = (added..upto).map(entry).collect();
        update_ok(
            &router,
            case,
            &json!({ "OpsItemId": id, "OperationalData": data }),
        )
        .await;
        added = upto;
    }
    let read = get_ok(&router, case, &id).await;
    assert_eq!(
        read["OperationalData"].as_object().map(serde_json::Map::len),
        Some(MAX_DATA_KEYS),
        "case {case}: expected {MAX_DATA_KEYS} keys to accumulate, got {}",
        read["OperationalData"]
    );

    let (one_too_many_key, one_too_many_value) = entry(MAX_DATA_KEYS);
    let past_the_bound = json!({
        "OpsItemId": id,
        "OperationalData": { one_too_many_key.clone(): one_too_many_value.clone() }
    });
    let (status, response) = update(&router, &past_the_bound).await;
    assert_eq!(
        status, 400,
        "case {case}: a key past the bound is refused, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "OpsItemInvalidParameterException",
        "case {case}: expected OpsItemInvalidParameterException, got {response}"
    );
    assert_eq!(
        response["ParameterNames"],
        json!(["OperationalData"]),
        "case {case}: expected the refusal to name OperationalData, got {response}"
    );
    assert_eq!(
        get_ok(&router, case, &id).await,
        read,
        "case {case}: the refused edit writes nothing"
    );

    // The bound is on the map the edit leaves behind, so removing a key makes room.
    update_ok(
        &router,
        case,
        &json!({
            "OpsItemId": id,
            "OperationalData": { one_too_many_key.clone(): one_too_many_value },
            "OperationalDataToDelete": [entry(0).0]
        }),
    )
    .await;
    let after = get_ok(&router, case, &id).await;
    assert_eq!(
        after["OperationalData"].as_object().map(serde_json::Map::len),
        Some(MAX_DATA_KEYS),
        "case {case}: expected the map to stay at the bound, got {}",
        after["OperationalData"]
    );
    assert!(
        after["OperationalData"][&one_too_many_key].is_object()
            && after["OperationalData"][entry(0).0].is_null(),
        "case {case}: expected the removed key gone and the added key held, got {}",
        after["OperationalData"]
    );
}

/// One row of the refusal table: the edit, and the error code it must be refused with.
struct RefusalCase<'a> {
    case: &'a str,
    body: Value,
    code: &'a str,
}

/// An edit the service cannot serve is refused with the modelled error for it, and writes
/// nothing: the OpsItem is left at the version and the members it already carried.
#[tokio::test]
async fn unservable_edits_are_refused_and_write_nothing() {
    let router = router();
    let created = create_ok(&router, "the OpsItem under test", &minimal("An OpsItem to edit")).await;
    let held = ops_item_id(&created);
    let other = ops_item_id(
        &create_ok(&router, "another OpsItem", &minimal("Another OpsItem entirely")).await,
    );
    let before = get_ok(&router, "the OpsItem under test", &held).await;

    let cases = vec![
        RefusalCase {
            case: "an id this Region holds no OpsItem under",
            body: json!({ "OpsItemId": "oi-0000deadbeef", "Status": "InProgress" }),
            code: "OpsItemNotFoundException",
        },
        RefusalCase {
            case: "an ARN of another account",
            body: json!({
                "OpsItemId": held,
                "OpsItemArn": format!("arn:aws:ssm:{REGION}:111122223333:opsitem/{held}"),
                "Status": "InProgress"
            }),
            code: "OpsItemAccessDeniedException",
        },
        RefusalCase {
            case: "an ARN of another OpsItem of this account",
            body: json!({
                "OpsItemId": held,
                "OpsItemArn": ops_item_arn(REGION, &other),
                "Status": "InProgress"
            }),
            code: "OpsItemNotFoundException",
        },
        // "Severity options range from 1 to 4" (`OpsItem$Severity`): one past each end of
        // that range, and a value that is not a number at all.
        RefusalCase {
            case: "a severity past the documented range",
            body: json!({ "OpsItemId": held, "Severity": "5" }),
            code: "OpsItemInvalidParameterException",
        },
        RefusalCase {
            case: "a severity below the documented range",
            body: json!({ "OpsItemId": held, "Severity": "0" }),
            code: "OpsItemInvalidParameterException",
        },
        RefusalCase {
            case: "a severity outside the documented options",
            body: json!({ "OpsItemId": held, "Severity": "critical" }),
            code: "OpsItemInvalidParameterException",
        },
        // "Category options include: Availability, Cost, Performance, Recovery, Security"
        // (`OpsItem$Category`): a neighbour of one of them, and a differently cased one.
        RefusalCase {
            case: "a category outside the documented options",
            body: json!({ "OpsItemId": held, "Category": "Capacity" }),
            code: "OpsItemInvalidParameterException",
        },
        RefusalCase {
            case: "a documented category, differently cased",
            body: json!({ "OpsItemId": held, "Category": "security" }),
            code: "OpsItemInvalidParameterException",
        },
        RefusalCase {
            case: "an operational-data key under a reserved prefix",
            body: json!({
                "OpsItemId": held,
                "OperationalData": { "aws-owned": { "Value": "whatever", "Type": "String" } }
            }),
            code: "OpsItemInvalidParameterException",
        },
        RefusalCase {
            case: "an operational-data value one byte past the size limit",
            body: json!({
                "OpsItemId": held,
                "OperationalData": {
                    "payload": { "Value": "p".repeat(MAX_DATA_VALUE + 1), "Type": "String" }
                }
            }),
            code: "OpsItemInvalidParameterException",
        },
    ];

    for RefusalCase { case, body, code } in cases {
        let (status, response) = update(&router, &body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected a client error, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            code,
            "case {case}: expected {code}, got {response}"
        );
    }

    let after = get_ok(&router, "the OpsItem under test", &held).await;
    assert_eq!(
        before, after,
        "a refused edit writes nothing: expected {before}, got {after}"
    );
}

/// An OpsItem is edited only through the endpoint of the Region it was created in: the same
/// id at another Region's endpoint names no OpsItem, and the one that holds it is untouched.
#[tokio::test]
async fn an_ops_item_is_edited_only_through_its_own_region() {
    let router = router();
    let created = create_ok(&router, "the OpsItem under test", &minimal("A regional OpsItem")).await;
    let held = ops_item_id(&created);
    let before = get_ok(&router, "the OpsItem under test", &held).await;
    for region in ["us-east-1", "eu-central-1", "ap-southeast-2"] {
        let (status, response) = call_in_region(
            &router,
            region,
            "UpdateOpsItem",
            &json!({ "OpsItemId": held, "Status": "InProgress" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {region}: expected the edit to be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "OpsItemNotFoundException",
            "case {region}: expected OpsItemNotFoundException, got {response}"
        );
    }
    let read = get_ok(&router, "the OpsItem under test", &held).await;
    assert_eq!(
        read["Status"].as_str(),
        Some("Open"),
        "an edit refused at another Region's endpoint leaves this one untouched, got {read}"
    );
    assert_eq!(
        version(&read, "the OpsItem under test"),
        version(&before, "the OpsItem under test"),
        "an edit refused at another Region's endpoint is no edit, got {read}"
    );
}

/// The `/aws/dedup` value of an OpsItem that is still open is that work item's: an edit
/// that would point a second OpsItem at it is refused and told which OpsItem holds it,
/// while an edit that keeps its own dedup string is served — and once the holder is no
/// longer open, the dedup string is free.
#[tokio::test]
async fn an_edit_cannot_claim_the_dedup_string_of_an_open_ops_item() {
    let router = router();
    /// The `OperationalData` patch that points an OpsItem at the work item `dedup` names.
    fn dedup_data(dedup: &str) -> Value {
        json!({
            "/aws/dedup": {
                "Value": format!("{{\"dedupString\": \"{dedup}\"}}"),
                "Type": "SearchableString"
            }
        })
    }

    for dedup in ["disk-full", "node-unreachable/worker-7", "a dedup string with spaces"] {
        let mut first = minimal(&format!("The work item for {dedup}"));
        first["OperationalData"] = dedup_data(dedup);
        let holder = ops_item_id(&create_ok(&router, dedup, &first).await);
        let claimant = ops_item_id(
            &create_ok(&router, dedup, &minimal(&format!("A second report of {dedup}"))).await,
        );

        let (status, response) = update(
            &router,
            &json!({ "OpsItemId": claimant, "OperationalData": dedup_data(dedup) }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {dedup}: expected the claim to be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "OpsItemAlreadyExistsException",
            "case {dedup}: expected OpsItemAlreadyExistsException, got {response}"
        );
        assert_eq!(
            response["OpsItemId"].as_str(),
            Some(holder.as_str()),
            "case {dedup}: expected the refusal to name the OpsItem that holds it, got {response}"
        );

        // The holder keeps its own dedup string across an edit of its own.
        update_ok(
            &router,
            dedup,
            &json!({ "OpsItemId": holder, "Status": "InProgress", "Priority": 1 }),
        )
        .await;

        // Resolving the holder finishes that work, so the dedup string is free again.
        update_ok(
            &router,
            dedup,
            &json!({ "OpsItemId": holder, "Status": "Resolved" }),
        )
        .await;
        update_ok(
            &router,
            dedup,
            &json!({ "OpsItemId": claimant, "OperationalData": dedup_data(dedup) }),
        )
        .await;
        let read = get_ok(&router, dedup, &claimant).await;
        assert_eq!(
            read["OperationalData"]["/aws/dedup"]["Value"].as_str(),
            Some(format!("{{\"dedupString\": \"{dedup}\"}}").as_str()),
            "case {dedup}: expected the dedup data to round-trip, got {read}"
        );
    }
}
