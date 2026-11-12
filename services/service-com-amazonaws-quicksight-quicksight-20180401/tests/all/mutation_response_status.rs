// SPDX-License-Identifier: CC0-1.0

//! The HTTP status a QuickSight write answers with, and the lifecycle status it reports.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so what is asserted is what a QuickSight SDK client would see:
//! the route, the status, and a body the handler computed from the shared store.
//!
//! The rule under test: a QuickSight write chooses its own HTTP status through the
//! `Status` member its response binds with `@httpResponseCode` — 202 when the service
//! only ACCEPTS the work, with the resource left in the non-terminal lifecycle status
//! the response reports and the terminal one reached later by the clock and reported by
//! a read; 201 when the call CREATES the resource there and then; and the model's
//! default 200 for a write the service answers as a plain success. Because the member
//! carries the status, it is never a body member of its own. So the rows below vary what
//! the rule generalises over — the operation and its outcome class, the resource kind,
//! the account, the namespace, the region signed for, and which optional members the
//! request carries — rather than re-running one recorded call.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_quicksight_quicksight_20180401::state as qs_state;

use super::data_source_data_set_lifecycle::Harness;
use super::group_lifecycle::group_label;
use super::namespace_lifecycle::create_namespace_in;

/// The account most rows use.
const ACCOUNT: &str = "000000000000";
/// A second, unrelated 12-digit account id, for the rows about account keying.
const OTHER_ACCOUNT: &str = "210987654321";
/// The region the harness signs for unless a row says otherwise.
const REGION: &str = "us-west-2";
/// A second region, so a row can show what does and does not depend on it.
const OTHER_REGION: &str = "eu-central-1";
/// The namespace every QuickSight account already has.
const DEFAULT_NAMESPACE: &str = "default";
/// A namespace a row creates, so the group and assignment rows are not all in `default`.
const CUSTOM_NAMESPACE: &str = "status-ns";

/// Members of the model's `ResourceStatus` enum.
/// Source: `smithy_query(query="shape", shape="ResourceStatus")`.
const CREATION_IN_PROGRESS: &str = "CREATION_IN_PROGRESS";
const CREATION_SUCCESSFUL: &str = "CREATION_SUCCESSFUL";
const UPDATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";
const UPDATE_SUCCESSFUL: &str = "UPDATE_SUCCESSFUL";

async fn create_data_source(harness: &Harness, account: &str, body: Value) -> (u16, Value) {
    harness
        .call(
            "POST",
            &format!("/accounts/{account}/data-sources"),
            Some(body),
        )
        .await
}

async fn update_data_source(
    harness: &Harness,
    account: &str,
    id: &str,
    body: Value,
) -> (u16, Value) {
    harness
        .call(
            "PUT",
            &format!("/accounts/{account}/data-sources/{id}"),
            Some(body),
        )
        .await
}

async fn describe_data_source(harness: &Harness, account: &str, id: &str) -> (u16, Value) {
    harness
        .call("GET", &format!("/accounts/{account}/data-sources/{id}"), None)
        .await
}

async fn create_group(
    harness: &Harness,
    region: &str,
    account: &str,
    namespace: &str,
    body: Value,
) -> (u16, Value) {
    harness
        .call_in_region(
            region,
            "POST",
            &format!("/accounts/{account}/namespaces/{namespace}/groups"),
            Some(body),
        )
        .await
}

async fn create_assignment(
    harness: &Harness,
    region: &str,
    account: &str,
    namespace: &str,
    body: Value,
) -> (u16, Value) {
    harness
        .call_in_region(
            region,
            "POST",
            &format!("/accounts/{account}/namespaces/{namespace}/iam-policy-assignments"),
            Some(body),
        )
        .await
}

/// The ARN QuickSight reports for the assignment `name` of `account`: the account and
/// the name, with no region field between them.
fn assignment_arn(account: &str, name: &str) -> String {
    format!("arn:aws:quicksight::{account}:assignment/{name}")
}

/// Create the namespace the rows that are not in `default` address.
async fn create_custom_namespace(harness: &Harness, account: &str) {
    let (status, body) = create_namespace_in(
        harness,
        REGION,
        account,
        json!({ "Namespace": CUSTOM_NAMESPACE, "IdentityStore": "QUICKSIGHT" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the namespace the rows need was not created: {status} with {body}"
    );
}

/// `Status` is bound to the HTTP status, so no response of this class carries it as a
/// body member.
fn assert_status_is_not_a_body_member(case: &str, body: &Value) {
    assert_eq!(
        body["Status"],
        Value::Null,
        "case {case:?}: Status is the HTTP status, so it is not a body member: {body}"
    );
}

/// One row of the data-source table: the create body, the update body, and the id both
/// address.
struct SourceRow {
    case: &'static str,
    account: &'static str,
    id: &'static str,
    create: Value,
    update: Value,
}

/// The rows differ in the source kind (members of the model's `DataSourceType` enum),
/// in which optional members each call carries, in the length of `Name` at the model's
/// bounds (1..128), and in the owning account.
fn source_rows() -> Vec<SourceRow> {
    vec![
        SourceRow {
            case: "athena, no optional members",
            account: ACCOUNT,
            id: "status-athena",
            create: json!({ "DataSourceId": "status-athena", "Name": "a", "Type": "ATHENA" }),
            update: json!({ "Name": "a".repeat(128) }),
        },
        SourceRow {
            case: "s3, parameters replaced by the update",
            account: ACCOUNT,
            id: "status-s3",
            create: json!({
                "DataSourceId": "status-s3",
                "Name": "manifest",
                "Type": "S3",
                "DataSourceParameters": {
                    "S3Parameters": {
                        "ManifestFileLocation": { "Bucket": "b-1", "Key": "manifest.json" }
                    }
                },
            }),
            update: json!({
                "Name": "manifest",
                "DataSourceParameters": {
                    "S3Parameters": {
                        "ManifestFileLocation": { "Bucket": "b-2", "Key": "manifest.json" }
                    }
                },
            }),
        },
        SourceRow {
            case: "postgresql, ssl flipped and credentials submitted",
            account: ACCOUNT,
            id: "status-postgres",
            create: json!({
                "DataSourceId": "status-postgres",
                "Name": "warehouse",
                "Type": "POSTGRESQL",
                "DataSourceParameters": {
                    "PostgreSqlParameters": { "Host": "db", "Port": 5432, "Database": "sales" }
                },
                "SslProperties": { "DisableSsl": true },
            }),
            update: json!({
                "Name": "warehouse",
                "SslProperties": { "DisableSsl": false },
                "Credentials": {
                    "CredentialPair": { "Username": "analyst", "Password": "pw-17" }
                },
            }),
        },
        SourceRow {
            case: "snowflake, another account's resource",
            account: OTHER_ACCOUNT,
            id: "status-snowflake",
            create: json!({
                "DataSourceId": "status-snowflake",
                "Name": "snowy",
                "Type": "SNOWFLAKE",
                "Tags": [{ "Key": "team", "Value": "analytics" }],
            }),
            update: json!({ "Name": "s" }),
        },
    ]
}

#[tokio::test]
async fn an_accepted_data_source_write_answers_202_and_settles_through_the_read() {
    let harness = Harness::new();

    for row in source_rows() {
        let SourceRow {
            case,
            account,
            id,
            create,
            update,
        } = row;

        let (status, body) = create_data_source(&harness, account, create).await;
        assert_eq!(
            status, 202,
            "case {case:?}: the create is accepted, not completed, so it expected 202, \
             got {status} with {body}"
        );
        assert_eq!(
            body["CreationStatus"],
            json!(CREATION_IN_PROGRESS),
            "case {case:?}: expected CreationStatus {CREATION_IN_PROGRESS}, got {body}"
        );
        assert_status_is_not_a_body_member(case, &body);

        // The non-terminal status is observable: a caller reading the resource straight
        // after the create sees the state the create reported.
        let (status, body) = describe_data_source(&harness, account, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["DataSource"]["Status"],
            json!(CREATION_IN_PROGRESS),
            "case {case:?}: the stored resource is still being created: {body}"
        );
        // And the list reports the same status as the read, from the same record.
        let (status, listed) = harness
            .call("GET", &format!("/accounts/{account}/data-sources"), None)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: list expected 200, got {status} with {listed}"
        );
        let in_list = listed["DataSources"]
            .as_array()
            .and_then(|sources| {
                sources
                    .iter()
                    .find(|source| source["DataSourceId"] == json!(id))
                    .cloned()
            })
            .unwrap_or_else(|| panic!("case {case:?}: the created source is not listed: {listed}"));
        assert_eq!(
            in_list["Status"],
            json!(CREATION_IN_PROGRESS),
            "case {case:?}: the list must report the same status as the read: {listed}"
        );

        // Once the transition the clock owes has fallen due, the same read reports the
        // terminal status — no further call was needed to move it.
        harness.settle_data_source(account, id);
        let (status, body) = describe_data_source(&harness, account, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe after the span expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["DataSource"]["Status"],
            json!(CREATION_SUCCESSFUL),
            "case {case:?}: once due, the creation is successful: {body}"
        );

        // The update is the same class of write: accepted, non-terminal, settled later.
        let (status, body) = update_data_source(&harness, account, id, update).await;
        assert_eq!(
            status, 202,
            "case {case:?}: the update is accepted, not completed, so it expected 202, \
             got {status} with {body}"
        );
        assert_eq!(
            body["UpdateStatus"],
            json!(UPDATE_IN_PROGRESS),
            "case {case:?}: expected UpdateStatus {UPDATE_IN_PROGRESS}, got {body}"
        );
        assert_status_is_not_a_body_member(case, &body);

        let (_, body) = describe_data_source(&harness, account, id).await;
        assert_eq!(
            body["DataSource"]["Status"],
            json!(UPDATE_IN_PROGRESS),
            "case {case:?}: the stored resource is still being updated: {body}"
        );
        harness.settle_data_source(account, id);
        let (_, body) = describe_data_source(&harness, account, id).await;
        assert_eq!(
            body["DataSource"]["Status"],
            json!(UPDATE_SUCCESSFUL),
            "case {case:?}: once due, the update is successful: {body}"
        );
    }
}

#[tokio::test]
async fn a_created_group_answers_201() {
    let harness = Harness::new();
    create_custom_namespace(&harness, ACCOUNT).await;

    // Rows: the name's form and length (`GroupName` admits len 1.. over
    // `^[\u0020-\u00FF]+$`), whether a description is carried, the namespace, the
    // account, and the region the call is signed for.
    let rows: Vec<(&str, &str, &str, &str, Value)> = vec![
        (
            "plain name with a description",
            REGION,
            ACCOUNT,
            DEFAULT_NAMESPACE,
            json!({ "GroupName": "analysts", "Description": "the analysts" }),
        ),
        (
            "the shortest name the model admits",
            REGION,
            ACCOUNT,
            DEFAULT_NAMESPACE,
            json!({ "GroupName": "g" }),
        ),
        (
            "a name holding a space, in a namespace the account created",
            REGION,
            ACCOUNT,
            CUSTOM_NAMESPACE,
            json!({ "GroupName": "Field Ops" }),
        ),
        (
            "another account, signed for another region",
            OTHER_REGION,
            OTHER_ACCOUNT,
            DEFAULT_NAMESPACE,
            json!({ "GroupName": "Ventes-Café", "Description": "d".repeat(512) }),
        ),
    ];

    for (case, region, account, namespace, body) in rows {
        let group_name = body["GroupName"].as_str().expect("a name").to_string();
        let (status, response) = create_group(&harness, region, account, namespace, body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: the group is created by this call, so it expected 201, got \
             {status} with {response}"
        );
        assert_eq!(
            response["Group"]["GroupName"],
            json!(group_name),
            "case {case:?}: the create must report the group it stored: {response}"
        );
        assert_status_is_not_a_body_member(case, &response);

        // Reading the group back is not a creation, so it is a plain success.
        let (status, response) = harness
            .call_in_region(
                region,
                "GET",
                &format!(
                    "/accounts/{account}/namespaces/{namespace}/groups/{}",
                    group_label(&group_name)
                ),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the read of a created group expected 200, got {status} with \
             {response}"
        );
    }
}

#[tokio::test]
async fn a_created_iam_policy_assignment_answers_201_with_its_own_arn() {
    let harness = Harness::new();
    create_custom_namespace(&harness, ACCOUNT).await;

    // Rows: every member of the model's `AssignmentStatus` enum, with and without a
    // policy ARN and an identity map, in two namespaces, two accounts and two regions.
    // Source: `smithy_query(query="shape", shape="AssignmentStatus")`.
    let rows: Vec<(&str, &str, &str, &str, Value)> = vec![
        (
            "enabled, with a policy and identities",
            REGION,
            ACCOUNT,
            DEFAULT_NAMESPACE,
            json!({
                "AssignmentName": "enabled-rule",
                "AssignmentStatus": "ENABLED",
                "PolicyArn": "arn:aws:iam::aws:policy/ReadOnlyAccess",
                "Identities": { "user": ["analyst"] },
            }),
        ),
        (
            "draft, with nothing optional",
            REGION,
            ACCOUNT,
            DEFAULT_NAMESPACE,
            json!({ "AssignmentName": "draft-rule", "AssignmentStatus": "DRAFT" }),
        ),
        (
            "disabled, in a namespace the account created",
            REGION,
            ACCOUNT,
            CUSTOM_NAMESPACE,
            json!({
                "AssignmentName": "disabled.rule=1",
                "AssignmentStatus": "DISABLED",
                "PolicyArn": "arn:aws:iam::aws:policy/job-function/ViewOnlyAccess",
            }),
        ),
        (
            "another account, signed for another region",
            OTHER_REGION,
            OTHER_ACCOUNT,
            DEFAULT_NAMESPACE,
            json!({ "AssignmentName": "elsewhere-rule", "AssignmentStatus": "ENABLED" }),
        ),
    ];

    for (case, region, account, namespace, body) in rows {
        let name = body["AssignmentName"].as_str().expect("a name").to_string();
        let (status, response) =
            create_assignment(&harness, region, account, namespace, body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: the assignment is created by this call, so it expected 201, \
             got {status} with {response}"
        );
        // The assignment's ARN names the account that owns it and the name that
        // identifies it there, and carries no region field — so the same assignment has
        // the same ARN whichever region the caller signed for.
        assert_eq!(
            response["Arn"],
            json!(assignment_arn(account, &name)),
            "case {case:?}: the create must report the assignment's own ARN: {response}"
        );
        assert_eq!(
            response["AssignmentName"],
            json!(name),
            "case {case:?}: the create must report the name it stored: {response}"
        );
        assert_status_is_not_a_body_member(case, &response);

        // The update of that same assignment is a plain success: it creates nothing.
        let (status, response) = harness
            .call_in_region(
                region,
                "PUT",
                &format!(
                    "/accounts/{account}/namespaces/{namespace}/iam-policy-assignments/{name}"
                ),
                Some(json!({ "AssignmentStatus": "DISABLED" })),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: an update creates nothing, so it expected 200, got {status} \
             with {response}"
        );
    }
}

#[tokio::test]
async fn a_write_the_service_completes_at_once_still_answers_200() {
    let harness = Harness::new();
    // The rule is about what the service's response states, not "every write is 2xx
    // something new": these writes neither create a resource nor leave work in flight,
    // so each is the plain success the model's default status describes.
    let (status, body) = create_data_source(
        &harness,
        ACCOUNT,
        json!({ "DataSourceId": "plain-source", "Name": "src", "Type": "ATHENA" }),
    )
    .await;
    assert_eq!(status, 202, "fixture create: got {status} with {body}");
    let (status, body) = create_assignment(
        &harness,
        REGION,
        ACCOUNT,
        DEFAULT_NAMESPACE,
        json!({ "AssignmentName": "plain-rule", "AssignmentStatus": "DRAFT" }),
    )
    .await;
    assert_eq!(status, 201, "fixture create: got {status} with {body}");

    // A folder create: the service answers it as a plain success, so the fix must not
    // have turned every create into a 201.
    let (status, body) = harness
        .call(
            "POST",
            &format!("/accounts/{ACCOUNT}/folders/plain-folder"),
            Some(json!({ "Name": "a folder" })),
        )
        .await;
    assert_eq!(
        status, 200,
        "creating a folder expected 200, got {status} with {body}"
    );

    for (case, method, path) in [
        (
            "DeleteDataSource",
            "DELETE",
            format!("/accounts/{ACCOUNT}/data-sources/plain-source"),
        ),
        (
            "DeleteIAMPolicyAssignment",
            "DELETE",
            format!("/accounts/{ACCOUNT}/namespace/{DEFAULT_NAMESPACE}/iam-policy-assignments/plain-rule"),
        ),
    ] {
        let (status, body) = harness.call(method, &path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        assert_status_is_not_a_body_member(case, &body);
    }
}

/// The transition a data-source write leaves owed is a real, non-zero span, and the
/// stored record — not a later call — is what decides which side of it a read is on.
#[test]
fn the_data_source_transition_is_due_within_the_chosen_span() {
    let span = qs_state::DATA_SOURCE_LIFECYCLE_SPAN;
    assert!(
        span >= Duration::from_secs(2) && span <= Duration::from_secs(5),
        "the transition span must be between 2 and 5 seconds, got {span:?}"
    );

    let accepted_at = SystemTime::now();
    let record = qs_state::DataSourceRecord {
        arn: "arn:aws:quicksight:us-west-2:000000000000:datasource/span".to_string(),
        data_source_id: "span".to_string(),
        name: "span".to_string(),
        r#type: "ATHENA".to_string(),
        status: CREATION_IN_PROGRESS.to_string(),
        settled_status: CREATION_SUCCESSFUL.to_string(),
        status_settles_at: accepted_at + span,
        created_time: qs_state::now_timestamp(accepted_at),
        last_updated_time: qs_state::now_timestamp(accepted_at),
        data_source_parameters: None,
        vpc_connection_properties: None,
        ssl_properties: None,
        permissions: Vec::new(),
        tags: Vec::new(),
        error_info: None,
    };
    assert_eq!(
        record.status_at(accepted_at),
        CREATION_IN_PROGRESS,
        "at the instant the write was accepted the transition is not yet due"
    );
    assert_eq!(
        record.status_at(record.status_settles_at),
        CREATION_SUCCESSFUL,
        "once due, the record reports the terminal status"
    );
    assert!(
        record.status_settles_at > accepted_at,
        "the span must move the due instant forward"
    );
}
