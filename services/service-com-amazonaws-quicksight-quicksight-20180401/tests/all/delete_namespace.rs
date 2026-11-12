// SPDX-License-Identifier: CC0-1.0

//! `DeleteNamespace`, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so what is asserted is what a QuickSight SDK client would see:
//! the route, the status, and a body the handler computed from the shared store.
//!
//! The rule under test: `DeleteNamespace` deletes the namespace `(AwsAccountId,
//! Namespace)` addresses together with the groups that namespace holds, as an
//! asynchronous process — the namespace reports the non-terminal `DELETING` status until
//! the clock makes its removal due and is gone from then on, freeing its name — while the
//! namespace every account already has cannot be deleted at all (an invalid value for
//! the parameter naming it) and a name the account holds no namespace under is not
//! found. So the rows below vary what the rule generalises over (the name's form and
//! length, how many groups the namespace holds, and the account) rather than re-running
//! one recorded call.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_quicksight_quicksight_20180401::state as qs_state;

use super::data_source_data_set_lifecycle::Harness;
use super::namespace_lifecycle::{create_namespace_in, describe_namespace_in, namespace_arn_in};

/// The account most rows use.
const ACCOUNT: &str = "000000000000";
/// A second, unrelated 12-digit account id, for the rows about account keying.
const OTHER_ACCOUNT: &str = "210987654321";
/// The region the test client signs for; it is the region in every ARN below.
const REGION: &str = "us-west-2";

/// `DeleteNamespace`: `DELETE /accounts/{AwsAccountId}/namespaces/{Namespace}`.
/// Source: smithy_query(query="operation", operation="DeleteNamespace").
async fn delete_namespace(harness: &Harness, account: &str, namespace: &str) -> (u16, Value) {
    harness
        .call(
            "DELETE",
            &format!("/accounts/{account}/namespaces/{namespace}"),
            None,
        )
        .await
}

/// `CreateGroup`: `POST /accounts/{AwsAccountId}/namespaces/{Namespace}/groups`.
/// Source: smithy_query(query="operation", operation="CreateGroup").
async fn create_group(
    harness: &Harness,
    account: &str,
    namespace: &str,
    group_name: &str,
) -> (u16, Value) {
    harness
        .call(
            "POST",
            &format!("/accounts/{account}/namespaces/{namespace}/groups"),
            Some(json!({ "GroupName": group_name })),
        )
        .await
}

/// `DescribeGroup`:
/// `GET /accounts/{AwsAccountId}/namespaces/{Namespace}/groups/{GroupName}`.
/// Source: smithy_query(query="operation", operation="DescribeGroup").
async fn describe_group(
    harness: &Harness,
    account: &str,
    namespace: &str,
    group_name: &str,
) -> (u16, Value) {
    harness
        .call(
            "GET",
            &format!("/accounts/{account}/namespaces/{namespace}/groups/{group_name}"),
            None,
        )
        .await
}

/// The instant the clock owes this namespace's removal, or `None` when no delete has
/// asked for one.
fn deletion_due(harness: &Harness, account: &str, namespace: &str) -> Option<SystemTime> {
    let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
    let store = state.lock();
    store
        .namespace(account, namespace)
        .and_then(|record| record.deletion_settles_at)
}

/// Bring the removal the clock owes this namespace forward, so a case can observe the
/// deletion completing without sleeping for the span.
fn settle_deletion(harness: &Harness, account: &str, namespace: &str) {
    let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
    let mut store = state.lock();
    let record = store
        .namespaces
        .get_mut(&(account.to_string(), namespace.to_string()))
        .unwrap_or_else(|| panic!("case {namespace:?}: the namespace being deleted is not stored"));
    record.deletion_settles_at = Some(SystemTime::now() - Duration::from_millis(1));
}

/// One row of the delete table.
struct Row {
    /// The case label every assertion names.
    case: &'static str,
    account: &'static str,
    /// The `Namespace` the create names and the delete addresses; the model admits
    /// len 0..64 over the pattern `^[a-zA-Z0-9._-]*$`.
    /// Source: smithy_query(query="operation", operation="DeleteNamespace").
    namespace: String,
    /// The groups the namespace holds when the delete runs — the users and groups that
    /// go with it.
    groups: &'static [&'static str],
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "empty-namespace",
            account: ACCOUNT,
            namespace: "sales".to_string(),
            groups: &[],
        },
        Row {
            case: "one-group",
            account: ACCOUNT,
            namespace: "finance.eu-west_2".to_string(),
            groups: &["analysts"],
        },
        Row {
            case: "several-groups",
            account: ACCOUNT,
            namespace: "n".to_string(),
            groups: &["analysts", "admins", "readers"],
        },
        Row {
            // The longest name the model admits: `Namespace` is len 0..64.
            case: "longest-name",
            account: ACCOUNT,
            namespace: "n".repeat(64),
            groups: &["analysts"],
        },
        Row {
            case: "other-account",
            account: OTHER_ACCOUNT,
            namespace: "sales".to_string(),
            groups: &["analysts", "admins"],
        },
    ]
}

#[tokio::test]
async fn delete_namespace_takes_the_namespace_and_its_groups() {
    let harness = Harness::new();

    for row in rows() {
        let case = row.case;
        let account = row.account;
        let namespace = row.namespace.as_str();

        let (status, body) = create_namespace_in(
            &harness,
            REGION,
            account,
            json!({ "Namespace": namespace, "IdentityStore": "QUICKSIGHT" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the namespace to delete was not created: {status} with {body}"
        );
        for group_name in row.groups {
            let (status, body) = create_group(&harness, account, namespace, group_name).await;
            assert_eq!(
                status, 201,
                "case {case:?}: group {group_name:?} was not created: {status} with {body}"
            );
        }

        let before = SystemTime::now();
        let (status, body) = delete_namespace(&harness, account, namespace).await;
        assert_eq!(
            status, 200,
            "case {case:?}: deleting a namespace the account holds expected 200, got \
             {status} with {body}"
        );
        assert!(
            body["RequestId"]
                .as_str()
                .is_some_and(|request_id| !request_id.is_empty()),
            "case {case:?}: the delete reported no RequestId: {body}"
        );

        // The delete is asynchronous, so the removal is owed at an instant after the
        // call rather than at it: a caller acting straight after the delete still sees
        // the namespace in its non-terminal status.
        let due = deletion_due(&harness, account, namespace).unwrap_or_else(|| {
            panic!("case {case:?}: the delete recorded no instant for the removal")
        });
        assert!(
            due > before,
            "case {case:?}: the deletion span must not be zero, due {due:?} for a call \
             at {before:?}"
        );
        let (status, body) = describe_namespace_in(&harness, REGION, account, namespace).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a namespace whose deletion is in flight is still \
             describable, got {status} with {body}"
        );
        let reported = &body["Namespace"];
        assert_eq!(
            reported["CreationStatus"],
            json!("DELETING"),
            "case {case:?}: a namespace being deleted reports DELETING: {body}"
        );
        assert_eq!(
            reported["Arn"],
            json!(namespace_arn_in(REGION, account, namespace)),
            "case {case:?}: the namespace being deleted keeps its Arn: {body}"
        );

        // The groups the namespace held go with it.
        for group_name in row.groups {
            let (status, body) = describe_group(&harness, account, namespace, group_name).await;
            assert_eq!(
                status, 404,
                "case {case:?}: group {group_name:?} went with its namespace, so \
                 describing it expected 404, got {status} with {body}"
            );
        }

        // Asking again for a deletion already in flight asks for nothing further.
        let (status, body) = delete_namespace(&harness, account, namespace).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated delete of the same namespace expected 200, got \
             {status} with {body}"
        );
        assert_eq!(
            deletion_due(&harness, account, namespace),
            Some(due),
            "case {case:?}: a repeated delete must not move the instant the removal is due"
        );

        // Once the clock owes the removal, the namespace is gone.
        settle_deletion(&harness, account, namespace);
        let (status, body) = describe_namespace_in(&harness, REGION, account, namespace).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a deleted namespace is not found, got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "case {case:?}: a name no namespace is held under is not found: {body}"
        );
        assert_eq!(
            body["Message"],
            json!(format!(
                "Namespace {namespace} not found in account {account}."
            )),
            "case {case:?}: the refusal must name the namespace and its account: {body}"
        );

        // So deleting it again finds nothing to delete, and its name is free to create
        // a namespace under once more.
        let (status, body) = delete_namespace(&harness, account, namespace).await;
        assert_eq!(
            status, 404,
            "case {case:?}: deleting an already deleted namespace expected 404, got \
             {status} with {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!("NAMESPACE"),
            "case {case:?}: the refusal is about a namespace: {body}"
        );

        let (status, body) = create_namespace_in(
            &harness,
            REGION,
            account,
            json!({ "Namespace": namespace, "IdentityStore": "QUICKSIGHT" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the deleted namespace's name is free again, so this create \
             expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["CreationStatus"],
            json!("CREATING"),
            "case {case:?}: the re-created namespace is a new one, still being created: \
             {body}"
        );
        // It is a new, empty namespace: nothing the deleted one held comes back.
        for group_name in row.groups {
            let (status, body) = describe_group(&harness, account, namespace, group_name).await;
            assert_eq!(
                status, 404,
                "case {case:?}: group {group_name:?} must not come back with the name, \
                 got {status} with {body}"
            );
        }
    }
}

#[tokio::test]
async fn deleting_a_namespace_leaves_the_others_alone() {
    let harness = Harness::new();
    let kept = "kept-namespace";
    let doomed = "doomed-namespace";

    for (case, account) in [("first-account", ACCOUNT), ("second-account", OTHER_ACCOUNT)] {
        for namespace in [kept, doomed] {
            let (status, body) = create_namespace_in(
                &harness,
                REGION,
                account,
                json!({ "Namespace": namespace, "IdentityStore": "QUICKSIGHT" }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: {namespace:?} was not created: {status} with {body}"
            );
            let (status, body) = create_group(&harness, account, namespace, "analysts").await;
            assert_eq!(
                status, 201,
                "case {case:?}: the group in {namespace:?} was not created: {status} \
                 with {body}"
            );
        }
    }

    // One namespace of one account is deleted, and the removal falls due.
    let (status, body) = delete_namespace(&harness, ACCOUNT, doomed).await;
    assert_eq!(
        status, 200,
        "deleting a namespace the account holds expected 200, got {status} with {body}"
    );
    settle_deletion(&harness, ACCOUNT, doomed);
    let (status, body) = describe_namespace_in(&harness, REGION, ACCOUNT, doomed).await;
    assert_eq!(
        status, 404,
        "the deleted namespace is gone, got {status} with {body}"
    );

    // Every other namespace, and every group in it, is untouched — including the
    // namespace of the same name in the other account.
    for (case, account, namespace) in [
        ("same-account-other-namespace", ACCOUNT, kept),
        ("other-account-same-namespace", OTHER_ACCOUNT, doomed),
        ("other-account-other-namespace", OTHER_ACCOUNT, kept),
    ] {
        let (status, body) = describe_namespace_in(&harness, REGION, account, namespace).await;
        assert_eq!(
            status, 200,
            "case {case:?}: this namespace was not the one deleted, got {status} with {body}"
        );
        assert_eq!(
            body["Namespace"]["Arn"],
            json!(namespace_arn_in(REGION, account, namespace)),
            "case {case:?}: the surviving namespace keeps its Arn: {body}"
        );
        let (status, body) = describe_group(&harness, account, namespace, "analysts").await;
        assert_eq!(
            status, 200,
            "case {case:?}: the groups of a namespace that was not deleted stay, got \
             {status} with {body}"
        );
    }
}

#[tokio::test]
async fn the_default_namespace_cannot_be_deleted() {
    let harness = Harness::new();

    // The namespace every account already has is not one a caller created and nothing
    // takes it away: the parameter naming it carries a value the operation does not
    // accept. Source: smithy_query(query="doc", shape="DescribeUserRequest",
    // member="Namespace") — "Currently, you should set this to default".
    for (case, account) in [
        ("untouched-account", ACCOUNT),
        ("another-untouched-account", OTHER_ACCOUNT),
        ("third-untouched-account", "123456789012"),
    ] {
        let (status, body) = delete_namespace(&harness, account, "default").await;
        assert_eq!(
            status, 400,
            "case {case:?}: deleting the default namespace expected 400, got {status} \
             with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("InvalidParameterValueException"),
            "case {case:?}: the namespace named is a value the operation does not \
             accept: {body}"
        );
        assert_eq!(
            body["Message"],
            json!("Cannot delete the default namespace."),
            "case {case:?}: the refusal must say which namespace cannot be deleted: {body}"
        );

        // And the refusal persisted nothing: the default namespace is still there,
        // ready to use.
        let (status, body) = describe_namespace_in(&harness, REGION, account, "default").await;
        assert_eq!(
            status, 200,
            "case {case:?}: the refused delete must leave the default namespace, got \
             {status} with {body}"
        );
        assert_eq!(
            body["Namespace"]["CreationStatus"],
            json!("CREATED"),
            "case {case:?}: the default namespace is still ready to use: {body}"
        );
    }

    // A group in the default namespace is untouched too.
    let (status, body) = create_group(&harness, ACCOUNT, "default", "analysts").await;
    assert_eq!(
        status, 201,
        "the group in the default namespace was not created: {status} with {body}"
    );
    let (status, body) = delete_namespace(&harness, ACCOUNT, "default").await;
    assert_eq!(
        status, 400,
        "deleting the default namespace expected 400, got {status} with {body}"
    );
    let (status, body) = describe_group(&harness, ACCOUNT, "default", "analysts").await;
    assert_eq!(
        status, 200,
        "a refused delete takes no group with it, got {status} with {body}"
    );
}

#[tokio::test]
async fn deleting_a_namespace_no_account_holds_is_not_found() {
    let harness = Harness::new();

    // The account holds one namespace, so the rows below are about the name rather than
    // about the account having none.
    let (status, body) = create_namespace_in(
        &harness,
        REGION,
        ACCOUNT,
        json!({ "Namespace": "sales", "IdentityStore": "QUICKSIGHT" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the namespace the rows need was not created: {status} with {body}"
    );

    let longest_name = "n".repeat(64);
    for (case, account, namespace) in [
        ("never-created", ACCOUNT, "no-such-namespace"),
        ("another-name", ACCOUNT, "sales.archive"),
        ("longest-name", ACCOUNT, longest_name.as_str()),
        ("other-account", OTHER_ACCOUNT, "sales"),
    ] {
        let (status, body) = delete_namespace(&harness, account, namespace).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a name the account holds no namespace under is not found, \
             got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "case {case:?}: nothing was there to delete: {body}"
        );
        assert_eq!(
            body["Message"],
            json!(format!(
                "Namespace {namespace} not found in account {account}."
            )),
            "case {case:?}: the refusal must name the namespace and its account: {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!("NAMESPACE"),
            "case {case:?}: the refusal is about a namespace: {body}"
        );
    }

    // The namespace the account does hold is still there: a refused delete persists
    // nothing.
    let (status, body) = describe_namespace_in(&harness, REGION, ACCOUNT, "sales").await;
    assert_eq!(
        status, 200,
        "the refused deletes must leave the namespace that exists, got {status} with {body}"
    );
}
