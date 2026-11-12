// SPDX-License-Identifier: CC0-1.0

//! Groups: the create and the read that reports it, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so what is asserted is what a QuickSight SDK client would see:
//! the route, the status, and a body the handler computed from the shared store.
//!
//! The rule under test: a QuickSight group is identified by
//! `(AwsAccountId, Namespace, GroupName)` and belongs to a namespace the account holds;
//! `CreateGroup` persists it, answers HTTP 201 for the group it created and reports that
//! group — its ARN, its name, the description it was given and the `PrincipalId` the
//! service minted for it — and `DescribeGroup` answers from that same record. A name the
//! namespace already holds is refused as an existing resource, and a namespace the
//! account does not hold is not found. So the rows below vary what the rule generalises
//! over (the group name's form and length, whether a description is submitted and how
//! long it is, the namespace, and the account) rather than re-running one recorded call.

use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use serde_json::{json, Value};

use service_com_amazonaws_quicksight_quicksight_20180401::state as qs_state;

use super::data_source_data_set_lifecycle::Harness;
use super::namespace_lifecycle::create_namespace_in;

/// The account most rows use.
const ACCOUNT: &str = "000000000000";
/// A second, unrelated 12-digit account id, for the rows about account keying.
const OTHER_ACCOUNT: &str = "210987654321";
/// The region the harness signs for; it is the region in every ARN below.
const REGION: &str = "us-west-2";
/// The namespace every account already has, and the one a caller with no custom
/// namespace addresses.
/// Source: smithy_query(query="operation", operation="CreateGroup") — `Namespace` is a
/// required label.
const DEFAULT_NAMESPACE: &str = "default";
/// A namespace this suite creates, so a row can show the namespace is part of a group's
/// identity rather than a filter applied afterwards.
const CUSTOM_NAMESPACE: &str = "sales-ns";

/// The bytes a group name is percent-encoded over when it goes into the URI path label
/// `DescribeGroup` addresses. `GroupName` admits every character in `\u{20}..\u{FF}`
/// (pattern `^[\u0020-\u00FF]+$`), so a name can hold a space or a non-ASCII letter.
/// Source: smithy_query(query="shape", shape="GroupName").
const GROUP_NAME_LABEL: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'%')
    .add(b'/')
    .add(b'?')
    .add(b'#')
    .add(b'+');

/// The group name as the URI path label carries it.
///
/// `pub(crate)` because every suite that addresses a group by name needs it: the label
/// is percent-encoded over the bytes the pattern admits but a URI does not.
pub(crate) fn group_label(group_name: &str) -> String {
    utf8_percent_encode(group_name, GROUP_NAME_LABEL).to_string()
}

/// The ARN a group created under `(account, namespace, group_name)` in [`REGION`]
/// carries.
fn group_arn(account: &str, namespace: &str, group_name: &str) -> String {
    format!("arn:aws:quicksight:{REGION}:{account}:group/{namespace}/{group_name}")
}

/// `CreateGroup`: `POST /accounts/{AwsAccountId}/namespaces/{Namespace}/groups`.
/// Source: smithy_query(query="operation", operation="CreateGroup").
async fn create_group(
    harness: &Harness,
    account: &str,
    namespace: &str,
    body: Value,
) -> (u16, Value) {
    harness
        .call(
            "POST",
            &format!("/accounts/{account}/namespaces/{namespace}/groups"),
            Some(body),
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
            &format!(
                "/accounts/{account}/namespaces/{namespace}/groups/{}",
                group_label(group_name)
            ),
            None,
        )
        .await
}

/// Create the namespace the rows that do not use the default one put their groups in.
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
        "the namespace the group rows need was not created: {status} with {body}"
    );
}

/// One row of the create-then-read table.
struct Row {
    /// The case label every assertion names.
    case: &'static str,
    account: &'static str,
    namespace: &'static str,
    /// The `GroupName` the create names; the model admits len 1.. over the pattern
    /// `^[\u0020-\u00FF]+$`.
    /// Source: smithy_query(query="operation", operation="CreateGroup").
    group_name: String,
    /// The `Description` the create submits, if any; the model admits len 1..512.
    description: Option<String>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "plain-name-with-description",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            group_name: "Sales".to_string(),
            description: Some("Sales team".to_string()),
        },
        Row {
            // The pattern admits `\u0020`, so a name can hold spaces.
            case: "name-with-spaces-and-no-description",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            group_name: "Sales Management East".to_string(),
            description: None,
        },
        Row {
            case: "punctuated-name",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            group_name: "sales-eu_west.2:readers".to_string(),
            description: Some("Readers in eu-west-2".to_string()),
        },
        Row {
            // The pattern reaches `\u00FF`, so a name can hold a Latin-1 letter.
            case: "non-ascii-name",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            group_name: "Ventes-Café".to_string(),
            description: Some("Équipe des ventes".to_string()),
        },
        Row {
            // The shortest name and the shortest description the model admits.
            case: "single-character-name-and-description",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            group_name: "g".to_string(),
            description: Some("d".to_string()),
        },
        Row {
            // The longest description the model admits: `GroupDescription` is len 1..512.
            case: "long-name-and-longest-description",
            account: ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            group_name: "g".repeat(128),
            description: Some("d".repeat(512)),
        },
        Row {
            // Same name as the first row, in another namespace of the same account.
            case: "custom-namespace",
            account: ACCOUNT,
            namespace: CUSTOM_NAMESPACE,
            group_name: "Sales".to_string(),
            description: Some("Sales team of the sales namespace".to_string()),
        },
        Row {
            // Same name again, in another account's default namespace.
            case: "other-account",
            account: OTHER_ACCOUNT,
            namespace: DEFAULT_NAMESPACE,
            group_name: "Sales".to_string(),
            description: None,
        },
    ]
}

impl Row {
    fn body(&self) -> Value {
        let mut body = json!({ "GroupName": self.group_name });
        if let Some(description) = &self.description {
            body["Description"] = json!(description);
        }
        body
    }

    /// The `Description` a read of this group reports: the one submitted, or none at
    /// all when the create named none.
    fn reported_description(&self) -> Value {
        match &self.description {
            Some(description) => json!(description),
            None => Value::Null,
        }
    }
}

#[tokio::test]
async fn create_group_persists_it_and_describe_reports_it() {
    let harness = Harness::new();
    create_custom_namespace(&harness, ACCOUNT).await;

    for row in rows() {
        let case = row.case;
        let group_name = row.group_name.as_str();
        let namespace = row.namespace;
        let expected_arn = group_arn(row.account, namespace, group_name);

        let (status, body) = create_group(&harness, row.account, namespace, row.body()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: create expected 201, got {status} with body {body}"
        );
        let created = &body["Group"];
        assert_eq!(
            created["Arn"],
            json!(expected_arn),
            "case {case:?}: create reported the wrong Arn: {body}"
        );
        assert_eq!(
            created["GroupName"],
            json!(group_name),
            "case {case:?}: create reported the wrong GroupName: {body}"
        );
        assert_eq!(
            created["Description"],
            row.reported_description(),
            "case {case:?}: create reported the wrong Description: {body}"
        );
        let principal_id = created["PrincipalId"].as_str().unwrap_or_default().to_string();
        assert!(
            !principal_id.is_empty(),
            "case {case:?}: create reported no PrincipalId: {body}"
        );
        assert!(
            body["RequestId"]
                .as_str()
                .is_some_and(|request_id| !request_id.is_empty()),
            "case {case:?}: create reported no RequestId: {body}"
        );

        // The create wrote the group to the shared store, keyed by the account, the
        // namespace and the name, so the reads have a record to answer from.
        {
            let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
            let store = state.lock();
            let record = store
                .group(row.account, namespace, group_name)
                .unwrap_or_else(|| panic!("case {case:?}: the created group is not stored"));
            assert_eq!(
                record.arn, expected_arn,
                "case {case:?}: the stored group carries the wrong Arn"
            );
            assert_eq!(
                record.description, row.description,
                "case {case:?}: the submitted Description is not stored as given"
            );
            assert_eq!(
                record.principal_id, principal_id,
                "case {case:?}: the stored PrincipalId is not the one the create reported"
            );
        }

        // Two reads of the same group report the same thing: a read answers from the
        // stored record and changes nothing.
        for attempt in ["first-read", "second-read"] {
            let (status, body) = describe_group(&harness, row.account, namespace, group_name).await;
            assert_eq!(
                status, 200,
                "case {case:?}/{attempt}: describe expected 200, got {status} with {body}"
            );
            let reported = &body["Group"];
            assert_eq!(
                reported["Arn"],
                json!(expected_arn),
                "case {case:?}/{attempt}: describe reported the wrong Arn: {body}"
            );
            assert_eq!(
                reported["GroupName"],
                json!(group_name),
                "case {case:?}/{attempt}: describe reported the wrong GroupName: {body}"
            );
            assert_eq!(
                reported["Description"],
                row.reported_description(),
                "case {case:?}/{attempt}: describe reported the wrong Description: {body}"
            );
            assert_eq!(
                reported["PrincipalId"],
                json!(principal_id),
                "case {case:?}/{attempt}: the PrincipalId must not change between \
                 reads: {body}"
            );
        }

        // A second create of the same name in the same namespace is refused: the name
        // belongs to the group holding it.
        let (status, body) = create_group(&harness, row.account, namespace, row.body()).await;
        assert_eq!(
            status, 409,
            "case {case:?}: re-creating the same group expected 409, got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceExistsException"),
            "case {case:?}: a name already in use names an existing resource: {body}"
        );
        assert_eq!(
            body["Message"],
            json!(format!(
                "Group {group_name} already exists. AWS account id: {}, groupName: \
                 {group_name}. ",
                row.account
            )),
            "case {case:?}: the refusal must name the group and its account: {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!("GROUP"),
            "case {case:?}: the refusal is about a group: {body}"
        );

        // And the group the first create stored is untouched by the refusal.
        let (status, body) = describe_group(&harness, row.account, namespace, group_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the refused create must not disturb the group: {status} with {body}"
        );
        assert_eq!(
            body["Group"]["PrincipalId"],
            json!(principal_id),
            "case {case:?}: the stored group is still the one the first create minted: {body}"
        );
    }
}

#[tokio::test]
async fn group_identity_is_the_account_the_namespace_and_the_name() {
    let harness = Harness::new();
    create_custom_namespace(&harness, ACCOUNT).await;
    let shared = "Shared Group";

    // The same name in three places: two namespaces of one account, and another
    // account's default namespace.
    let places = [
        ("same-account-default-namespace", ACCOUNT, DEFAULT_NAMESPACE),
        ("same-account-custom-namespace", ACCOUNT, CUSTOM_NAMESPACE),
        ("other-account", OTHER_ACCOUNT, DEFAULT_NAMESPACE),
    ];

    let mut principal_ids = Vec::new();
    for (case, account, namespace) in places {
        let (status, body) = create_group(
            &harness,
            account,
            namespace,
            json!({ "GroupName": shared, "Description": case }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: one group of that name per namespace, so this create \
             expected 201, got {status} with {body}"
        );
        assert_eq!(
            body["Group"]["Arn"],
            json!(group_arn(account, namespace, shared)),
            "case {case:?}: the ARN must carry the owning account and namespace: {body}"
        );
        principal_ids.push(
            body["Group"]["PrincipalId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }

    for (index, (case, account, namespace)) in places.iter().enumerate() {
        let (status, body) = describe_group(&harness, account, namespace, shared).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["Group"]["Description"],
            json!(case),
            "case {case:?}: describe must report that namespace's own group: {body}"
        );
        assert_eq!(
            body["Group"]["PrincipalId"],
            json!(principal_ids[index]),
            "case {case:?}: each group has its own PrincipalId: {body}"
        );
    }

    // Each of the three is a group of its own, so no two share a handle.
    for (left, right) in [(0usize, 1usize), (0, 2), (1, 2)] {
        assert_ne!(
            principal_ids[left], principal_ids[right],
            "case {:?}/{:?}: two distinct groups must not share a PrincipalId",
            places[left].0, places[right].0
        );
    }

    // A name no group in that namespace is held under is not found.
    for (case, account, namespace, group_name) in [
        (
            "unused-name",
            ACCOUNT,
            DEFAULT_NAMESPACE,
            "No Such Group".to_string(),
        ),
        (
            "name-used-in-another-namespace-only",
            ACCOUNT,
            CUSTOM_NAMESPACE,
            "Sales".to_string(),
        ),
        (
            "account-that-created-nothing",
            "123456789012",
            DEFAULT_NAMESPACE,
            shared.to_string(),
        ),
    ] {
        let (status, body) = describe_group(&harness, account, namespace, &group_name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a group that was never created cannot be described, got \
             {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "case {case:?}: a name no group is held under is not found: {body}"
        );
        assert_eq!(
            body["Message"],
            json!(format!(
                "Group {group_name} not found in namespace {namespace} in account {account}."
            )),
            "case {case:?}: the refusal must name what was not found: {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!("GROUP"),
            "case {case:?}: the refusal is about a group: {body}"
        );
    }
}

#[tokio::test]
async fn a_group_needs_a_namespace_the_account_holds() {
    let harness = Harness::new();

    for (case, account, namespace) in [
        ("never-created", ACCOUNT, "no-such-namespace"),
        // A namespace created under one account is no namespace of another.
        ("created-under-another-account", OTHER_ACCOUNT, CUSTOM_NAMESPACE),
    ] {
        if case == "created-under-another-account" {
            create_custom_namespace(&harness, ACCOUNT).await;
        }
        let (status, body) =
            create_group(&harness, account, namespace, json!({ "GroupName": "Sales" })).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a group needs a namespace to live in, got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "case {case:?}: a namespace the account does not hold is not found: {body}"
        );
        assert_eq!(
            body["Message"],
            json!(format!("Namespace {namespace} not found in account {account}.")),
            "case {case:?}: the refusal must name the namespace: {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!("NAMESPACE"),
            "case {case:?}: the refusal is about a namespace: {body}"
        );

        // Nothing was stored for the refused create.
        let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
        let store = state.lock();
        assert!(
            store.group(account, namespace, "Sales").is_none(),
            "case {case:?}: a refused create must persist nothing"
        );
    }
}
