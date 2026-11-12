// SPDX-License-Identifier: CC0-1.0

//! IAM policy assignments: the create, the read, the update and the delete, over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so what is asserted is what a QuickSight SDK client would see:
//! the route, the status, and a body the handler computed from the shared store.
//!
//! The rule under test: an IAM policy assignment's name identifies it inside its
//! account — the model states that assignment names are unique per Amazon Web Services
//! account, so a name the account already holds is refused whichever namespace the
//! create names, while the namespace it was created in is the address every later call
//! must present. The create persists the submitted status, policy ARN and identities
//! under a server-minted `AssignmentId`, the update applies only the optional members
//! its request carries (replacing a submitted `Identities` map whole), the delete
//! removes the record, and every read answers from that stored record. So the rows
//! below vary what the rule generalises over — the status, the presence of a policy
//! ARN, the shape of the identity map, the name's form, the namespace, the account and
//! the number of callers racing to create — rather than re-running one recorded call.

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{json, Value};

use service_com_amazonaws_quicksight_quicksight_20180401::state as qs_state;

use super::data_source_data_set_lifecycle::Harness;

/// The account most rows use.
const ACCOUNT: &str = "000000000000";
/// A second, unrelated 12-digit account id, for the rows about account keying.
const OTHER_ACCOUNT: &str = "210987654321";
/// The namespace every QuickSight account has.
const DEFAULT_NAMESPACE: &str = "default";
/// A second namespace, for the rows about namespace keying.
const OTHER_NAMESPACE: &str = "marketing-eu";

/// Every value the model's `AssignmentStatus` enum admits. Source:
/// `smithy_query(query="shape", shape="AssignmentStatus")` — `ENABLED | DRAFT |
/// DISABLED`.
const ASSIGNMENT_STATUSES: [&str; 3] = ["ENABLED", "DRAFT", "DISABLED"];

/// An IAM policy ARN a request can name. The member is a free-form `Arn`, so what
/// matters is that it is stored and reported verbatim.
const READ_ONLY_POLICY: &str = "arn:aws:iam::aws:policy/ReadOnlyAccess";
const VIEW_ONLY_POLICY: &str = "arn:aws:iam::aws:policy/job-function/ViewOnlyAccess";

async fn create_assignment(
    harness: &Harness,
    account: &str,
    namespace: &str,
    body: Value,
) -> (u16, Value) {
    harness
        .call(
            "POST",
            &format!("/accounts/{account}/namespaces/{namespace}/iam-policy-assignments"),
            Some(body),
        )
        .await
}

async fn describe_assignment(
    harness: &Harness,
    account: &str,
    namespace: &str,
    name: &str,
) -> (u16, Value) {
    harness
        .call(
            "GET",
            &format!("/accounts/{account}/namespaces/{namespace}/iam-policy-assignments/{name}"),
            None,
        )
        .await
}

async fn update_assignment(
    harness: &Harness,
    account: &str,
    namespace: &str,
    name: &str,
    body: Value,
) -> (u16, Value) {
    harness
        .call(
            "PUT",
            &format!("/accounts/{account}/namespaces/{namespace}/iam-policy-assignments/{name}"),
            Some(body),
        )
        .await
}

/// `DeleteIAMPolicyAssignment` is bound to `/namespace/` (singular), not
/// `/namespaces/`. Source: `smithy_query(query="operation",
/// operation="DeleteIAMPolicyAssignment")`.
async fn delete_assignment(
    harness: &Harness,
    account: &str,
    namespace: &str,
    name: &str,
) -> (u16, Value) {
    harness
        .call(
            "DELETE",
            &format!("/accounts/{account}/namespace/{namespace}/iam-policy-assignments/{name}"),
            None,
        )
        .await
}

/// What the service reports for an assignment name the namespace does not hold.
fn not_found_message(name: &str) -> Value {
    json!(format!("Cannot find an assignment named {name}."))
}

/// One row of the create-then-read table.
struct Row {
    /// The case label every assertion names.
    case: &'static str,
    account: &'static str,
    namespace: &'static str,
    assignment_name: &'static str,
    assignment_status: &'static str,
    policy_arn: Option<&'static str>,
    /// The `IdentityMap` the create submits and every later read must report back.
    identities: Option<Value>,
}

impl Row {
    /// The request body this row submits.
    fn body(&self) -> Value {
        let mut body = json!({
            "AssignmentName": self.assignment_name,
            "AssignmentStatus": self.assignment_status,
        });
        if let Some(policy_arn) = self.policy_arn {
            body["PolicyArn"] = json!(policy_arn);
        }
        if let Some(identities) = &self.identities {
            body["Identities"] = identities.clone();
        }
        body
    }

    /// The `PolicyArn` a read must report: the submitted one, or nothing at all.
    fn expected_policy_arn(&self) -> Value {
        match self.policy_arn {
            Some(policy_arn) => json!(policy_arn),
            None => Value::Null,
        }
    }

    /// The `Identities` a read must report: the submitted map, or nothing at all.
    fn expected_identities(&self) -> Value {
        self.identities.clone().unwrap_or(Value::Null)
    }
}

/// The rows: each varies what the rule generalises over, not just the resource's name.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "enabled-with-one-user",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            assignment_name: "analyst-readonly",
            assignment_status: "ENABLED",
            policy_arn: Some(READ_ONLY_POLICY),
            identities: Some(json!({ "user": ["analyst"] })),
        },
        Row {
            // Both optional members absent: a draft rule that names no policy and no
            // identity yet is a well-formed assignment.
            case: "draft-without-policy-or-identities",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            assignment_name: "draft.rule",
            assignment_status: "DRAFT",
            policy_arn: None,
            identities: None,
        },
        Row {
            // The name exercises the punctuation the model's pattern admits
            // (`[0-9a-zA-Z-_.:=+@]`), and the row sits in a second namespace.
            case: "disabled-with-groups-in-another-namespace",
            account: ACCOUNT,
            namespace: OTHER_NAMESPACE,
            assignment_name: "Rule_2:tier=gold+eu@team",
            assignment_status: "DISABLED",
            policy_arn: Some(VIEW_ONLY_POLICY),
            identities: Some(json!({ "group": ["analysts", "engineers"] })),
        },
        Row {
            // Both identity kinds at once, under another account, with the shortest
            // name the model's pattern admits (2 characters).
            case: "both-identity-kinds-other-account",
            account: OTHER_ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            assignment_name: "ab",
            assignment_status: "ENABLED",
            policy_arn: Some(READ_ONLY_POLICY),
            identities: Some(json!({ "user": ["first", "second"], "group": ["auditors"] })),
        },
        Row {
            // An identity key with no names under it: the map is present and empty of
            // identities, which is not the same as an absent map.
            case: "identity-key-with-no-names",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            assignment_name: "empty-identity-list",
            assignment_status: "DRAFT",
            policy_arn: Some(READ_ONLY_POLICY),
            identities: Some(json!({ "user": [] })),
        },
    ]
}

#[tokio::test]
async fn create_persists_the_assignment_and_describe_reports_it() {
    let harness = Harness::new();
    let mut assignment_ids: HashSet<String> = HashSet::new();

    let table = rows();
    for status in ASSIGNMENT_STATUSES {
        assert!(
            table.iter().any(|row| row.assignment_status == status),
            "the table must span every modelled AssignmentStatus; {status:?} is missing"
        );
    }

    for row in &table {
        let case = row.case;
        let (status, body) =
            create_assignment(&harness, row.account, row.namespace, row.body()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: create expected 201, got {status} with body {body}"
        );
        assert_eq!(
            body["Arn"],
            json!(format!(
                "arn:aws:quicksight::{}:assignment/{}",
                row.account, row.assignment_name
            )),
            "case {case:?}: create must report the assignment's own ARN: {body}"
        );
        assert_eq!(
            body["AssignmentName"],
            json!(row.assignment_name),
            "case {case:?}: create reported the wrong AssignmentName: {body}"
        );
        assert_eq!(
            body["AssignmentStatus"],
            json!(row.assignment_status),
            "case {case:?}: create must report the submitted AssignmentStatus: {body}"
        );
        assert_eq!(
            body["PolicyArn"],
            row.expected_policy_arn(),
            "case {case:?}: create must report the submitted PolicyArn: {body}"
        );
        assert_eq!(
            body["Identities"],
            row.expected_identities(),
            "case {case:?}: create must report the submitted Identities: {body}"
        );
        let assignment_id = body["AssignmentId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !assignment_id.is_empty(),
            "case {case:?}: create minted no AssignmentId: {body}"
        );
        assert!(
            assignment_ids.insert(assignment_id.clone()),
            "case {case:?}: the AssignmentId {assignment_id:?} was already minted for \
             another assignment"
        );
        assert!(
            body["RequestId"]
                .as_str()
                .is_some_and(|request_id| !request_id.is_empty()),
            "case {case:?}: create reported no RequestId: {body}"
        );

        // The read answers from the stored record: everything the create was given,
        // under the id the create minted and the account that owns it.
        let (status, body) =
            describe_assignment(&harness, row.account, row.namespace, row.assignment_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe expected 200, got {status} with body {body}"
        );
        let assignment = &body["IAMPolicyAssignment"];
        assert_eq!(
            assignment["AwsAccountId"],
            json!(row.account),
            "case {case:?}: describe reported the wrong AwsAccountId: {body}"
        );
        assert_eq!(
            assignment["AssignmentName"],
            json!(row.assignment_name),
            "case {case:?}: describe reported the wrong AssignmentName: {body}"
        );
        assert_eq!(
            assignment["AssignmentId"],
            json!(assignment_id),
            "case {case:?}: the AssignmentId must be the one the create minted: {body}"
        );
        assert_eq!(
            assignment["AssignmentStatus"],
            json!(row.assignment_status),
            "case {case:?}: describe lost the submitted AssignmentStatus: {body}"
        );
        assert_eq!(
            assignment["PolicyArn"],
            row.expected_policy_arn(),
            "case {case:?}: describe lost the submitted PolicyArn: {body}"
        );
        assert_eq!(
            assignment["Identities"],
            row.expected_identities(),
            "case {case:?}: describe lost the submitted Identities: {body}"
        );

        // A second create of a name the account already holds is a conflict, whatever
        // it submits, and leaves the stored assignment as it was.
        let (status, body) = create_assignment(
            &harness,
            row.account,
            row.namespace,
            json!({
                "AssignmentName": row.assignment_name,
                "AssignmentStatus": "DISABLED",
                "PolicyArn": VIEW_ONLY_POLICY,
            }),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: re-creating the same assignment expected 409, got {status} \
             with {body}"
        );
        assert_eq!(
            body["Message"],
            json!(format!(
                "An assignment named {} already exists.",
                row.assignment_name
            )),
            "case {case:?}: the conflict must name the assignment: {body}"
        );
        let (status, body) =
            describe_assignment(&harness, row.account, row.namespace, row.assignment_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe after the conflict expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["IAMPolicyAssignment"]["AssignmentStatus"],
            json!(row.assignment_status),
            "case {case:?}: the refused create must not have overwritten the record: {body}"
        );
    }
}

#[tokio::test]
async fn an_assignment_name_is_unique_per_account_whatever_namespace_claims_it() {
    let harness = Harness::new();
    let shared_name = "shared-rule";
    let other_account_policy = "arn:aws:iam::aws:policy/AdministratorAccess";

    // The name is free in each account, so each account's create of it succeeds; a
    // second create of the same name inside ONE account is refused whichever namespace
    // it names, so it cannot overwrite the rule that holds the name. Source: the
    // operation's model documentation — "Assignment names are unique per Amazon Web
    // Services account. To avoid overwriting rules in other namespaces, use assignment
    // names that are unique."
    for (case, account, namespace, policy_arn, expected_status) in [
        (
            "first-account-claims-the-name",
            ACCOUNT,
            DEFAULT_NAMESPACE,
            READ_ONLY_POLICY,
            201u16,
        ),
        (
            "same-account-same-namespace-again",
            ACCOUNT,
            DEFAULT_NAMESPACE,
            VIEW_ONLY_POLICY,
            409,
        ),
        (
            "same-account-another-namespace",
            ACCOUNT,
            OTHER_NAMESPACE,
            VIEW_ONLY_POLICY,
            409,
        ),
        (
            "another-account-claims-the-name",
            OTHER_ACCOUNT,
            DEFAULT_NAMESPACE,
            other_account_policy,
            201,
        ),
        (
            "another-account-another-namespace-again",
            OTHER_ACCOUNT,
            OTHER_NAMESPACE,
            READ_ONLY_POLICY,
            409,
        ),
    ] {
        let (status, body) = create_assignment(
            &harness,
            account,
            namespace,
            json!({
                "AssignmentName": shared_name,
                "AssignmentStatus": "ENABLED",
                "PolicyArn": policy_arn,
            }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: create expected {expected_status}, got {status} with {body}"
        );
        if status == 409 {
            assert_eq!(
                body["Message"],
                json!(format!("An assignment named {shared_name} already exists.")),
                "case {case:?}: the conflict must name the assignment: {body}"
            );
        }
    }

    // Each account's own assignment is the one its read reports, and it is reachable
    // only through the namespace that created it: the refused creates left nothing in
    // the other namespace and overwrote nothing in this one.
    for (case, account, namespace, expected_status, expected_policy) in [
        (
            "first-account-creating-namespace",
            ACCOUNT,
            DEFAULT_NAMESPACE,
            200u16,
            READ_ONLY_POLICY,
        ),
        (
            "first-account-other-namespace",
            ACCOUNT,
            OTHER_NAMESPACE,
            404,
            READ_ONLY_POLICY,
        ),
        (
            "second-account-creating-namespace",
            OTHER_ACCOUNT,
            DEFAULT_NAMESPACE,
            200,
            other_account_policy,
        ),
    ] {
        let (status, body) = describe_assignment(&harness, account, namespace, shared_name).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: describe expected {expected_status}, got {status} with {body}"
        );
        if status == 200 {
            assert_eq!(
                body["IAMPolicyAssignment"]["PolicyArn"],
                json!(expected_policy),
                "case {case:?}: describe must report that account's own assignment: {body}"
            );
        } else {
            assert_eq!(
                body["Message"],
                not_found_message(shared_name),
                "case {case:?}: the refusal must name the assignment: {body}"
            );
        }
    }

    // An account that created nothing, and a name nobody created, are both absent;
    // and so is an update or a delete addressed to the wrong namespace.
    for (case, account, namespace, name) in [
        ("unknown-account", "123456789012", DEFAULT_NAMESPACE, shared_name),
        ("unknown-name", ACCOUNT, DEFAULT_NAMESPACE, "never-created"),
    ] {
        let (status, body) = describe_assignment(&harness, account, namespace, name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            body["Message"],
            not_found_message(name),
            "case {case:?}: the refusal must name the assignment: {body}"
        );
    }

    let (status, body) = update_assignment(
        &harness,
        ACCOUNT,
        OTHER_NAMESPACE,
        shared_name,
        json!({ "AssignmentStatus": "DISABLED" }),
    )
    .await;
    assert_eq!(
        status, 404,
        "an update addressed to a namespace that does not hold the assignment expected \
         404, got {status} with {body}"
    );
    let (status, body) = delete_assignment(&harness, ACCOUNT, OTHER_NAMESPACE, shared_name).await;
    assert_eq!(
        status, 404,
        "a delete addressed to a namespace that does not hold the assignment expected \
         404, got {status} with {body}"
    );
    let (status, body) = describe_assignment(&harness, ACCOUNT, DEFAULT_NAMESPACE, shared_name).await;
    assert_eq!(
        status, 200,
        "neither refusal may have touched the stored assignment, got {status} with {body}"
    );
    assert_eq!(
        body["IAMPolicyAssignment"]["AssignmentStatus"],
        json!("ENABLED"),
        "the assignment keeps the status its create gave it: {body}"
    );
}

#[tokio::test]
async fn update_applies_only_the_members_the_request_carries() {
    let harness = Harness::new();
    let name = "update-target";
    let (status, body) = create_assignment(
        &harness,
        ACCOUNT,
        DEFAULT_NAMESPACE,
        json!({
            "AssignmentName": name,
            "AssignmentStatus": "ENABLED",
            "PolicyArn": READ_ONLY_POLICY,
            "Identities": { "user": ["analyst"] },
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "the fixture create expected 201, got {status} with {body}"
    );
    let assignment_id = body["AssignmentId"].clone();

    // Each row submits one subset of the optional members and states the whole record
    // that must follow: what the row names changes, everything else stays.
    let cases: Vec<(&str, Value, &str, Value, Value)> = vec![
        (
            "status-only",
            json!({ "AssignmentStatus": "DISABLED" }),
            "DISABLED",
            json!(READ_ONLY_POLICY),
            json!({ "user": ["analyst"] }),
        ),
        (
            "policy-only",
            json!({ "PolicyArn": VIEW_ONLY_POLICY }),
            "DISABLED",
            json!(VIEW_ONLY_POLICY),
            json!({ "user": ["analyst"] }),
        ),
        (
            // A submitted map replaces the stored one whole, so the `user` key the
            // create set is gone rather than merged with.
            "identities-replace-the-stored-map",
            json!({ "Identities": { "group": ["auditors"] } }),
            "DISABLED",
            json!(VIEW_ONLY_POLICY),
            json!({ "group": ["auditors"] }),
        ),
        (
            "every-member-at-once",
            json!({
                "AssignmentStatus": "DRAFT",
                "PolicyArn": READ_ONLY_POLICY,
                "Identities": { "user": ["first", "second"] },
            }),
            "DRAFT",
            json!(READ_ONLY_POLICY),
            json!({ "user": ["first", "second"] }),
        ),
        (
            // Nothing named: an update that carries no optional member changes nothing.
            "no-member-at-all",
            json!({}),
            "DRAFT",
            json!(READ_ONLY_POLICY),
            json!({ "user": ["first", "second"] }),
        ),
    ];

    for (case, request, expected_status, expected_policy, expected_identities) in cases {
        let (status, body) =
            update_assignment(&harness, ACCOUNT, DEFAULT_NAMESPACE, name, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: update expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["AssignmentStatus"],
            json!(expected_status),
            "case {case:?}: update reported the wrong AssignmentStatus: {body}"
        );
        assert_eq!(
            body["AssignmentId"], assignment_id,
            "case {case:?}: an update must not re-mint the AssignmentId: {body}"
        );

        let (status, body) =
            describe_assignment(&harness, ACCOUNT, DEFAULT_NAMESPACE, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe expected 200, got {status} with {body}"
        );
        let assignment = &body["IAMPolicyAssignment"];
        assert_eq!(
            assignment["AssignmentStatus"],
            json!(expected_status),
            "case {case:?}: describe reported the wrong AssignmentStatus: {body}"
        );
        assert_eq!(
            assignment["PolicyArn"], expected_policy,
            "case {case:?}: describe reported the wrong PolicyArn: {body}"
        );
        assert_eq!(
            assignment["Identities"], expected_identities,
            "case {case:?}: describe reported the wrong Identities: {body}"
        );
        assert_eq!(
            assignment["AssignmentId"], assignment_id,
            "case {case:?}: the AssignmentId must survive an update: {body}"
        );
    }

    // A name the namespace does not hold has nothing to update.
    let (status, body) = update_assignment(
        &harness,
        ACCOUNT,
        DEFAULT_NAMESPACE,
        "never-created",
        json!({ "AssignmentStatus": "ENABLED" }),
    )
    .await;
    assert_eq!(
        status, 404,
        "updating an absent assignment expected 404, got {status} with {body}"
    );
    assert_eq!(
        body["Message"],
        not_found_message("never-created"),
        "the refusal must name the assignment: {body}"
    );
}

#[tokio::test]
async fn an_assignment_moves_between_every_pair_of_modelled_statuses() {
    let harness = Harness::new();

    // The transition class is every ordered pair of the modelled statuses, the
    // no-op pairs included.
    for from in ASSIGNMENT_STATUSES {
        for to in ASSIGNMENT_STATUSES {
            let case = format!("{from}-to-{to}");
            let name = format!("transition-{from}-{to}");
            let (status, body) = create_assignment(
                &harness,
                ACCOUNT,
                DEFAULT_NAMESPACE,
                json!({
                    "AssignmentName": name,
                    "AssignmentStatus": from,
                    "PolicyArn": READ_ONLY_POLICY,
                }),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: create expected 201, got {status} with {body}"
            );
            assert_eq!(
                body["AssignmentStatus"],
                json!(from),
                "case {case:?}: create must report the status it was given: {body}"
            );

            let (status, body) = update_assignment(
                &harness,
                ACCOUNT,
                DEFAULT_NAMESPACE,
                &name,
                json!({ "AssignmentStatus": to }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: update expected 200, got {status} with {body}"
            );

            let (status, body) =
                describe_assignment(&harness, ACCOUNT, DEFAULT_NAMESPACE, &name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: describe expected 200, got {status} with {body}"
            );
            assert_eq!(
                body["IAMPolicyAssignment"]["AssignmentStatus"],
                json!(to),
                "case {case:?}: describe must report the status the update set: {body}"
            );
        }
    }
}

#[tokio::test]
async fn delete_removes_the_assignment_and_leaves_its_siblings() {
    let harness = Harness::new();
    let deleted = "doomed-rule";
    let kept = "surviving-rule";

    for name in [deleted, kept] {
        let (status, body) = create_assignment(
            &harness,
            ACCOUNT,
            DEFAULT_NAMESPACE,
            json!({
                "AssignmentName": name,
                "AssignmentStatus": "ENABLED",
                "PolicyArn": READ_ONLY_POLICY,
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {name:?}: the fixture create expected 201, got {status} with {body}"
        );
    }
    // An assignment in another namespace of the same account, under its own name: the
    // delete below addresses one namespace and must not reach it.
    let elsewhere = "other-namespace-rule";
    let (status, body) = create_assignment(
        &harness,
        ACCOUNT,
        OTHER_NAMESPACE,
        json!({ "AssignmentName": elsewhere, "AssignmentStatus": "DRAFT" }),
    )
    .await;
    assert_eq!(
        status, 201,
        "the namespace fixture create expected 201, got {status} with {body}"
    );

    let (status, body) = delete_assignment(&harness, ACCOUNT, DEFAULT_NAMESPACE, deleted).await;
    assert_eq!(
        status, 200,
        "delete expected 200, got {status} with body {body}"
    );
    assert_eq!(
        body["AssignmentName"],
        json!(deleted),
        "delete must report the name it removed: {body}"
    );

    // What was deleted is gone; what was not is untouched.
    for (case, namespace, name, expected_status) in [
        ("deleted", DEFAULT_NAMESPACE, deleted, 404u16),
        ("sibling-in-the-same-namespace", DEFAULT_NAMESPACE, kept, 200),
        ("sibling-in-another-namespace", OTHER_NAMESPACE, elsewhere, 200),
    ] {
        let (status, body) = describe_assignment(&harness, ACCOUNT, namespace, name).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} with {body}"
        );
        if expected_status == 404 {
            assert_eq!(
                body["Message"],
                not_found_message(name),
                "case {case:?}: the refusal must name the assignment: {body}"
            );
        }
    }

    // A second delete, and an update, have nothing left to act on.
    let (status, body) = delete_assignment(&harness, ACCOUNT, DEFAULT_NAMESPACE, deleted).await;
    assert_eq!(
        status, 404,
        "deleting an absent assignment expected 404, got {status} with {body}"
    );
    assert_eq!(
        body["Message"],
        not_found_message(deleted),
        "the refusal must name the assignment: {body}"
    );
    let (status, body) = update_assignment(
        &harness,
        ACCOUNT,
        DEFAULT_NAMESPACE,
        deleted,
        json!({ "AssignmentStatus": "ENABLED" }),
    )
    .await;
    assert_eq!(
        status, 404,
        "updating a deleted assignment expected 404, got {status} with {body}"
    );

    // The create path is free again: the deleted name can be assigned anew — in any
    // namespace of the account, since nothing holds the name any more.
    let (status, body) = create_assignment(
        &harness,
        ACCOUNT,
        OTHER_NAMESPACE,
        json!({ "AssignmentName": deleted, "AssignmentStatus": "DISABLED" }),
    )
    .await;
    assert_eq!(
        status, 201,
        "re-creating a deleted assignment expected 201, got {status} with {body}"
    );
    assert_eq!(
        body["AssignmentStatus"],
        json!("DISABLED"),
        "the new assignment carries the status this create was given: {body}"
    );
    let (status, body) = describe_assignment(&harness, ACCOUNT, OTHER_NAMESPACE, deleted).await;
    assert_eq!(
        status, 200,
        "the re-created assignment is read through the namespace that created it, got \
         {status} with {body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_creates_of_distinct_names_all_commit_and_read_back() {
    // Creates are serialized by the store, so simultaneous creates of names nothing
    // holds all commit: the number of callers racing is what this rule generalises
    // over. Each row stands up its own emulator, so what it asserts about the store
    // depends on that row's racers alone.
    for (case, callers) in [
        ("two-racers", 2usize),
        ("six-racers", 6),
        ("eleven-racers", 11),
    ] {
        let harness = Arc::new(Harness::new());
        let mut racers = Vec::with_capacity(callers);
        for index in 0..callers {
            let handle = Arc::clone(&harness);
            let name = format!("{case}-{index}");
            racers.push(tokio::spawn(async move {
                let (status, body) = create_assignment(
                    &handle,
                    ACCOUNT,
                    DEFAULT_NAMESPACE,
                    json!({
                        "AssignmentName": name,
                        "AssignmentStatus": "DRAFT",
                        "PolicyArn": READ_ONLY_POLICY,
                    }),
                )
                .await;
                (name, status, body)
            }));
        }

        let mut committed: Vec<(String, String)> = Vec::with_capacity(callers);
        for racer in racers {
            let (name, status, body) = racer.await.expect("a racing create panicked");
            assert_eq!(
                status, 201,
                "case {case:?}: the racer that named {name:?} expected 201, got {status} \
                 with {body}"
            );
            let assignment_id = body["AssignmentId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the racer that named {name:?} minted no AssignmentId: {body}")
                })
                .to_string();
            committed.push((name, assignment_id));
        }

        let distinct: HashSet<&String> = committed.iter().map(|(_, id)| id).collect();
        assert_eq!(
            distinct.len(),
            callers,
            "case {case:?}: {callers} committed creates must hold {callers} distinct \
             AssignmentIds, got {committed:?}"
        );

        // Every racer's assignment is readable, and reads back ITS record rather than
        // another racer's.
        for (name, assignment_id) in &committed {
            let (status, body) =
                describe_assignment(&harness, ACCOUNT, DEFAULT_NAMESPACE, name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {name:?} must read back, got {status} with {body}"
            );
            let assignment = &body["IAMPolicyAssignment"];
            assert_eq!(
                assignment["AssignmentId"],
                json!(assignment_id),
                "case {case:?}: {name:?} reads back another create's record: {body}"
            );
            assert_eq!(
                assignment["AssignmentStatus"],
                json!("DRAFT"),
                "case {case:?}: {name:?} lost the submitted AssignmentStatus: {body}"
            );
            assert_eq!(
                assignment["PolicyArn"],
                json!(READ_ONLY_POLICY),
                "case {case:?}: {name:?} lost the submitted PolicyArn: {body}"
            );
        }

        // The account holds one record per committed create and nothing besides.
        let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
        let held = state.lock().iam_policy_assignment_count(ACCOUNT);
        assert_eq!(
            held, callers,
            "case {case:?}: the account must hold one record per committed create, it \
             holds {held}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_creates_of_one_name_commit_exactly_one_assignment() {
    // However many callers race for ONE name, the store is serialized: one of them
    // commits and every other is refused the name, so the account holds exactly one
    // assignment under it and the refused racers leave nothing behind. Each row stands
    // up its own emulator, so the count it asserts is that row's alone.
    for (case, callers, assignment_status) in [
        ("two-racers", 2usize, "DRAFT"),
        ("six-racers", 6, "ENABLED"),
        ("eleven-racers", 11, "DISABLED"),
    ] {
        let harness = Arc::new(Harness::new());
        let name = format!("contended-{case}");
        let mut racers = Vec::with_capacity(callers);
        for _ in 0..callers {
            let handle = Arc::clone(&harness);
            let name = name.clone();
            racers.push(tokio::spawn(async move {
                create_assignment(
                    &handle,
                    ACCOUNT,
                    DEFAULT_NAMESPACE,
                    json!({
                        "AssignmentName": name,
                        "AssignmentStatus": assignment_status,
                        "PolicyArn": READ_ONLY_POLICY,
                    }),
                )
                .await
            }));
        }

        let mut winners: Vec<String> = Vec::new();
        for racer in racers {
            let (status, body) = racer.await.expect("a racing create panicked");
            match status {
                201 => winners.push(
                    body["AssignmentId"]
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: the committing create minted no AssignmentId: {body}")
                        })
                        .to_string(),
                ),
                // The name was already claimed by the racer that reached the store
                // first, so this one is refused the modelled conflict.
                409 => assert_eq!(
                    body["Message"],
                    json!(format!("An assignment named {name} already exists.")),
                    "case {case:?}: a refused racer must name the assignment: {body}"
                ),
                other => panic!(
                    "case {case:?}: a racing create answered {other}, which is neither a \
                     commit nor a modelled refusal: {body}"
                ),
            }
        }
        assert_eq!(
            winners.len(),
            1,
            "case {case:?}: exactly one of {callers} racers for one name may commit, got \
             {winners:?}"
        );

        // What the name holds is the committing racer's record, whole: the refused
        // racers wrote nothing over it.
        let (status, body) = describe_assignment(&harness, ACCOUNT, DEFAULT_NAMESPACE, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the committed assignment must read back, got {status} with {body}"
        );
        assert_eq!(
            body["IAMPolicyAssignment"]["AssignmentId"],
            json!(winners[0]),
            "case {case:?}: the stored assignment must be the one that committed: {body}"
        );
        assert_eq!(
            body["IAMPolicyAssignment"]["AssignmentStatus"],
            json!(assignment_status),
            "case {case:?}: the stored assignment lost the submitted AssignmentStatus: {body}"
        );

        let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
        let store = state.lock();
        let record = store
            .iam_policy_assignment(ACCOUNT, DEFAULT_NAMESPACE, &name)
            .unwrap_or_else(|| panic!("case {case:?}: the committed assignment is not stored"));
        assert_eq!(
            record.assignment_id, winners[0],
            "case {case:?}: the store holds a record no create reported"
        );
        // The refused racers left nothing behind: one commit, one record in the whole
        // account.
        let held = store.iam_policy_assignment_count(ACCOUNT);
        assert_eq!(
            held, 1,
            "case {case:?}: {callers} racers for one name must leave the account holding \
             one assignment, it holds {held}"
        );
    }
}
