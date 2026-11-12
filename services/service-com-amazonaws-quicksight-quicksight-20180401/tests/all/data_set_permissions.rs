// SPDX-License-Identifier: CC0-1.0

//! A data set's resource permissions: granting, revoking and reading them back.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so what is asserted is what a QuickSight SDK client would see:
//! the route, the status, and a body the handler computed from the shared store.
//!
//! The rule under test: a resource's permission list holds ONE entry per principal, so
//! an update is applied per principal — a grant adds its actions to that principal's
//! entry (creating it when the principal holds nothing yet), a revoke takes its actions
//! away and drops the entry once the principal holds none — and a read reports the
//! stored list of the data set the request identifies by `(AwsAccountId, DataSetId)`.
//! So the rows below vary what that rule generalises over: how many principals and
//! actions an update names, whether the principal is new or already granted, whether a
//! revoke takes part or all of what a principal holds, whether one call both grants and
//! revokes, the size of the resulting list, and the account.

use serde_json::{json, Value};

use super::data_source_data_set_lifecycle::Harness;

/// The account most rows use.
const ACCOUNT: &str = "000000000000";
/// A second, unrelated 12-digit account id, for the rows about account keying.
const OTHER_ACCOUNT: &str = "210987654321";
/// The data set every row grants on.
const DATA_SET: &str = "perm-dset";

/// A QuickSight principal: `ResourcePermission$Principal` documents the ARN of a
/// QuickSight user or group.
fn user_arn(account: &str, user: &str) -> String {
    format!("arn:aws:quicksight:us-west-2:{account}:user/default/{user}")
}

/// The actions a data set's owner holds. `ResourcePermission$Actions` targets
/// `ActionList`, a list of plain IAM action name strings — the model constrains no
/// catalog of them, and the service stores whatever the caller grants — so these are
/// the data set actions QuickSight's own owner grant names.
const OWNER_ACTIONS: [&str; 6] = [
    "quicksight:DescribeDataSet",
    "quicksight:DescribeDataSetPermissions",
    "quicksight:PassDataSet",
    "quicksight:UpdateDataSet",
    "quicksight:UpdateDataSetPermissions",
    "quicksight:DeleteDataSet",
];

/// The actions a data set's viewer holds: reading it and nothing more.
const VIEWER_ACTIONS: [&str; 3] = [
    "quicksight:DescribeDataSet",
    "quicksight:DescribeDataSetPermissions",
    "quicksight:PassDataSet",
];

/// One `ResourcePermission` as the wire carries it.
fn permission(principal: &str, actions: &[&str]) -> Value {
    json!({ "Principal": principal, "Actions": actions })
}

/// Create the data source one fixture data set reads, and answer its ARN. The id is
/// derived from the data set's, so a case that creates several data sets does not
/// create the same data source twice.
async fn data_source_fixture(harness: &Harness, account: &str, data_set_id: &str) -> String {
    let (status, body) = harness
        .call(
            "POST",
            &format!("/accounts/{account}/data-sources"),
            Some(json!({
                "DataSourceId": format!("src-{data_set_id}"),
                "Name": "fixture source",
                "Type": "ATHENA",
            })),
        )
        .await;
    assert_eq!(
        status, 202,
        "fixture data source: create expected 202, got {status} with {body}"
    );
    body["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("fixture data source: create returned no Arn: {body}"))
        .to_string()
}

/// Create one data set, optionally with a permission list, and answer its ARN.
async fn create_data_set(
    harness: &Harness,
    account: &str,
    data_set_id: &str,
    permissions: Option<Value>,
) -> String {
    let data_source_arn = data_source_fixture(harness, account, data_set_id).await;
    let mut body = json!({
        "DataSetId": data_set_id,
        "Name": "permissions fixture",
        "ImportMode": "DIRECT_QUERY",
        "PhysicalTableMap": {
            "orders-table": {
                "RelationalTable": {
                    "DataSourceArn": data_source_arn,
                    "Schema": "public",
                    "Name": "orders",
                    "InputColumns": [{ "Name": "order_id", "Type": "INTEGER" }],
                }
            }
        },
    });
    if let Some(permissions) = permissions {
        body["Permissions"] = permissions;
    }
    let (status, response) = harness
        .call("POST", &format!("/accounts/{account}/data-sets"), Some(body))
        .await;
    assert_eq!(
        status, 200,
        "fixture {data_set_id}: create expected 200, got {status} with {response}"
    );
    response["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("fixture {data_set_id}: create returned no Arn: {response}"))
        .to_string()
}

/// One `DescribeDataSetPermissions` call.
async fn describe_permissions(harness: &Harness, account: &str, data_set_id: &str) -> (u16, Value) {
    harness
        .call(
            "GET",
            &format!("/accounts/{account}/data-sets/{data_set_id}/permissions"),
            None,
        )
        .await
}

/// One `UpdateDataSetPermissions` call carrying the grants and revokes given.
async fn update_permissions(
    harness: &Harness,
    account: &str,
    data_set_id: &str,
    grant: Option<Value>,
    revoke: Option<Value>,
) -> (u16, Value) {
    let mut body = json!({});
    if let Some(grant) = grant {
        body["GrantPermissions"] = grant;
    }
    if let Some(revoke) = revoke {
        body["RevokePermissions"] = revoke;
    }
    harness
        .call(
            "POST",
            &format!("/accounts/{account}/data-sets/{data_set_id}/permissions"),
            Some(body),
        )
        .await
}

/// The permission list a response reports, as `(principal, actions)` pairs in the
/// order it reported them.
fn permissions_of(body: &Value) -> Vec<(String, Vec<String>)> {
    body["Permissions"]
        .as_array()
        .unwrap_or_else(|| panic!("the response carries no Permissions list: {body}"))
        .iter()
        .map(|entry| {
            let principal = entry["Principal"]
                .as_str()
                .unwrap_or_else(|| panic!("an entry carries no Principal: {body}"))
                .to_string();
            let actions = entry["Actions"]
                .as_array()
                .unwrap_or_else(|| panic!("an entry carries no Actions: {body}"))
                .iter()
                .map(|action| {
                    action
                        .as_str()
                        .unwrap_or_else(|| panic!("an action is not a string: {body}"))
                        .to_string()
                })
                .collect();
            (principal, actions)
        })
        .collect()
}

/// One expected permission list as borrowed test data: `(principal, actions)` pairs.
type WantList<'a> = Vec<(&'a str, &'a [&'a str])>;

/// One row of the create-and-read table: the case, the data set id, the `Permissions`
/// the create carries, and the list the read must report.
type CreateCase<'a> = (&'static str, &'static str, Option<Value>, WantList<'a>);

/// One row of a round-trip table: the case, what the step grants, what it revokes, and
/// the list the read must report after it.
type StepCase<'a> = (&'static str, Option<Value>, Option<Value>, WantList<'a>);

/// One row of the keying table: the case, the account and data set id read, and the
/// list that read must report.
type KeyingCase<'a> = (&'static str, &'static str, &'static str, WantList<'a>);

/// The expected shape of [`permissions_of`], built from borrowed test data.
fn expected(entries: &[(&str, &[&str])]) -> Vec<(String, Vec<String>)> {
    entries
        .iter()
        .map(|(principal, actions)| {
            (
                (*principal).to_string(),
                actions.iter().map(|action| (*action).to_string()).collect(),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_read_reports_the_permission_list_the_create_persisted() {
    let harness = Harness::new();
    let analyst = user_arn(ACCOUNT, "analyst");
    let auditor = user_arn(ACCOUNT, "auditor");

    // Rows: a data set created with no list, with one entry, and with two entries
    // holding different numbers of actions.
    let cases: [CreateCase; 3] = [
        ("no permissions", "dset-none", None, vec![]),
        (
            "one owner",
            "dset-one",
            Some(json!([permission(&analyst, &OWNER_ACTIONS)])),
            vec![(analyst.as_str(), OWNER_ACTIONS.as_slice())],
        ),
        (
            "an owner and a viewer",
            "dset-two",
            Some(json!([
                permission(&analyst, &OWNER_ACTIONS),
                permission(&auditor, &VIEWER_ACTIONS),
            ])),
            vec![
                (analyst.as_str(), OWNER_ACTIONS.as_slice()),
                (auditor.as_str(), VIEWER_ACTIONS.as_slice()),
            ],
        ),
    ];

    for (case, data_set_id, permissions, want) in cases {
        let arn = create_data_set(&harness, ACCOUNT, data_set_id, permissions).await;
        let (status, body) = describe_permissions(&harness, ACCOUNT, data_set_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        let actual = permissions_of(&body);
        let want = expected(&want);
        assert_eq!(
            actual, want,
            "case {case:?}: expected {want:?}, got {actual:?} in {body}"
        );
        assert_eq!(
            body["DataSetId"],
            json!(data_set_id),
            "case {case:?}: expected the data set's id, got {body}"
        );
        assert_eq!(
            body["DataSetArn"],
            json!(arn),
            "case {case:?}: expected the data set's ARN {arn}, got {body}"
        );
        assert!(
            body["RequestId"].as_str().is_some_and(|id| !id.is_empty()),
            "case {case:?}: expected a RequestId, got {body}"
        );
    }
}

#[tokio::test]
async fn an_update_grants_and_revokes_per_principal() {
    let analyst = user_arn(ACCOUNT, "analyst");
    let auditor = user_arn(ACCOUNT, "auditor");
    // One extra action, so a row can grant to a principal that already holds some.
    const EXTRA: &str = "quicksight:UpdateDataSet";

    // One row: the case name, what the call grants, what it revokes, and the list the
    // data set is left holding. Each row starts from the same fixture — the analyst an
    // owner, the auditor a viewer — so the rows are independent of each other.
    type Case = (
        &'static str,
        Option<Value>,
        Option<Value>,
        Vec<(String, Vec<String>)>,
    );
    let owner = (analyst.clone(), OWNER_ACTIONS.map(str::to_string).to_vec());
    let viewer = (auditor.clone(), VIEWER_ACTIONS.map(str::to_string).to_vec());
    let stranger = user_arn(ACCOUNT, "stranger");

    let cases: Vec<Case> = vec![
        (
            "granting a principal that holds nothing adds an entry",
            Some(json!([permission(&stranger, &VIEWER_ACTIONS)])),
            None,
            vec![
                owner.clone(),
                viewer.clone(),
                (stranger.clone(), VIEWER_ACTIONS.map(str::to_string).to_vec()),
            ],
        ),
        (
            "granting a principal that already holds some adds only the new actions",
            Some(json!([permission(&auditor, &[VIEWER_ACTIONS[0], EXTRA])])),
            None,
            vec![
                owner.clone(),
                (
                    auditor.clone(),
                    VIEWER_ACTIONS
                        .iter()
                        .chain([EXTRA].iter())
                        .map(|action| (*action).to_string())
                        .collect(),
                ),
            ],
        ),
        (
            "granting only actions a principal already holds changes nothing",
            Some(json!([permission(&analyst, &OWNER_ACTIONS)])),
            None,
            vec![owner.clone(), viewer.clone()],
        ),
        (
            "revoking part of what a principal holds leaves the rest",
            None,
            Some(json!([permission(&analyst, &[OWNER_ACTIONS[5]])])),
            vec![
                (
                    analyst.clone(),
                    OWNER_ACTIONS[..5].iter().map(|a| a.to_string()).collect(),
                ),
                viewer.clone(),
            ],
        ),
        (
            "revoking everything a principal holds drops its entry",
            None,
            Some(json!([permission(&auditor, &VIEWER_ACTIONS)])),
            vec![owner.clone()],
        ),
        (
            "revoking an action a principal does not hold changes nothing",
            None,
            Some(json!([permission(&auditor, &[OWNER_ACTIONS[5]])])),
            vec![owner.clone(), viewer.clone()],
        ),
        (
            "revoking from a principal that holds nothing changes nothing",
            None,
            Some(json!([permission(&stranger, &VIEWER_ACTIONS)])),
            vec![owner.clone(), viewer.clone()],
        ),
        (
            "one call grants one principal and revokes another",
            Some(json!([permission(&stranger, &VIEWER_ACTIONS)])),
            Some(json!([permission(&auditor, &VIEWER_ACTIONS)])),
            vec![
                owner.clone(),
                (stranger.clone(), VIEWER_ACTIONS.map(str::to_string).to_vec()),
            ],
        ),
        (
            "one call revoking and granting the same principal leaves what it granted",
            Some(json!([permission(&auditor, &[EXTRA])])),
            Some(json!([permission(&auditor, &VIEWER_ACTIONS)])),
            vec![owner.clone(), (auditor.clone(), vec![EXTRA.to_string()])],
        ),
        (
            "two principals granted in one call",
            Some(json!([
                permission(&stranger, &VIEWER_ACTIONS),
                permission(&analyst, &[EXTRA]),
            ])),
            None,
            vec![
                owner.clone(),
                viewer.clone(),
                (stranger.clone(), VIEWER_ACTIONS.map(str::to_string).to_vec()),
            ],
        ),
        (
            "a call that neither grants nor revokes changes nothing",
            None,
            None,
            vec![owner.clone(), viewer.clone()],
        ),
    ];

    for (case, grant, revoke, want) in cases {
        let harness = Harness::new();
        let arn = create_data_set(
            &harness,
            ACCOUNT,
            DATA_SET,
            Some(json!([
                permission(&analyst, &OWNER_ACTIONS),
                permission(&auditor, &VIEWER_ACTIONS),
            ])),
        )
        .await;

        let (status, body) = update_permissions(&harness, ACCOUNT, DATA_SET, grant, revoke).await;
        assert_eq!(
            status, 200,
            "case {case:?}: update expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["DataSetArn"],
            json!(arn),
            "case {case:?}: update expected the data set's ARN {arn}, got {body}"
        );
        assert_eq!(
            body["DataSetId"],
            json!(DATA_SET),
            "case {case:?}: update expected the data set's id, got {body}"
        );

        let (read_status, read) = describe_permissions(&harness, ACCOUNT, DATA_SET).await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: read expected 200, got {read_status} with {read}"
        );
        let actual = permissions_of(&read);
        assert_eq!(
            actual, want,
            "case {case:?}: expected {want:?}, got {actual:?} in {read}"
        );
    }
}

#[tokio::test]
async fn an_update_is_visible_to_every_later_read_and_a_read_changes_nothing() {
    let harness = Harness::new();
    let analyst = user_arn(ACCOUNT, "analyst");
    create_data_set(&harness, ACCOUNT, DATA_SET, None).await;

    // Rows: the list after each step of a grant/revoke round trip, read twice per step
    // so a read that consumed or moved anything would show up here.
    let steps: [StepCase; 4] = [
        ("before any grant", None, None, vec![]),
        (
            "after granting the owner",
            Some(json!([permission(&analyst, &OWNER_ACTIONS)])),
            None,
            vec![(analyst.as_str(), OWNER_ACTIONS.as_slice())],
        ),
        (
            "after revoking one action",
            None,
            Some(json!([permission(&analyst, &[OWNER_ACTIONS[5]])])),
            vec![(analyst.as_str(), &OWNER_ACTIONS[..5])],
        ),
        (
            "after revoking the rest",
            None,
            Some(json!([permission(&analyst, &OWNER_ACTIONS[..5])])),
            vec![],
        ),
    ];

    for (case, grant, revoke, want) in steps {
        if grant.is_some() || revoke.is_some() {
            let (status, body) =
                update_permissions(&harness, ACCOUNT, DATA_SET, grant, revoke).await;
            assert_eq!(
                status, 200,
                "case {case:?}: update expected 200, got {status} with {body}"
            );
        }
        let want = expected(&want);
        for round in 0..2 {
            let (status, body) = describe_permissions(&harness, ACCOUNT, DATA_SET).await;
            assert_eq!(
                status, 200,
                "case {case:?} read {round}: expected 200, got {status} with {body}"
            );
            let actual = permissions_of(&body);
            assert_eq!(
                actual, want,
                "case {case:?} read {round}: expected {want:?}, got {actual:?} in {body}"
            );
        }
    }
}

#[tokio::test]
async fn permissions_are_held_per_account_and_per_data_set() {
    let harness = Harness::new();
    let analyst = user_arn(ACCOUNT, "analyst");
    let other = user_arn(OTHER_ACCOUNT, "analyst");
    create_data_set(&harness, ACCOUNT, DATA_SET, None).await;
    create_data_set(&harness, ACCOUNT, "perm-dset-sibling", None).await;
    create_data_set(&harness, OTHER_ACCOUNT, DATA_SET, None).await;

    let (status, body) = update_permissions(
        &harness,
        ACCOUNT,
        DATA_SET,
        Some(json!([permission(&analyst, &OWNER_ACTIONS)])),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "the grant expected 200, got {status} with {body}"
    );
    let (status, body) = update_permissions(
        &harness,
        OTHER_ACCOUNT,
        DATA_SET,
        Some(json!([permission(&other, &VIEWER_ACTIONS)])),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "the other account's grant expected 200, got {status} with {body}"
    );

    // Rows: the granted data set, its sibling in the same account, and the data set of
    // the same id in another account.
    let cases: [KeyingCase; 3] = [
        (
            "the granted data set",
            ACCOUNT,
            DATA_SET,
            vec![(analyst.as_str(), OWNER_ACTIONS.as_slice())],
        ),
        (
            "a sibling data set of the same account",
            ACCOUNT,
            "perm-dset-sibling",
            vec![],
        ),
        (
            "the same data set id in another account",
            OTHER_ACCOUNT,
            DATA_SET,
            vec![(other.as_str(), VIEWER_ACTIONS.as_slice())],
        ),
    ];

    for (case, account, data_set_id, want) in cases {
        let (status, body) = describe_permissions(&harness, account, data_set_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        let actual = permissions_of(&body);
        let want = expected(&want);
        assert_eq!(
            actual, want,
            "case {case:?}: expected {want:?}, got {actual:?} in {body}"
        );
    }
}

#[tokio::test]
async fn a_data_set_holds_at_most_the_modelled_number_of_grants() {
    let harness = Harness::new();
    create_data_set(&harness, ACCOUNT, DATA_SET, None).await;

    // The published bound: `ResourcePermissionList` is `length 1..64`, so 64 principals
    // fit and the 65th does not.
    const MAX: usize = 64;
    let principals: Vec<String> = (0..=MAX)
        .map(|index| user_arn(ACCOUNT, &format!("user-{index:03}")))
        .collect();

    let grant: Vec<Value> = principals[..MAX]
        .iter()
        .map(|principal| permission(principal, &VIEWER_ACTIONS))
        .collect();
    let (status, body) =
        update_permissions(&harness, ACCOUNT, DATA_SET, Some(json!(grant)), None).await;
    assert_eq!(
        status, 200,
        "case at the bound: expected 200, got {status} with {body}"
    );
    let (status, read) = describe_permissions(&harness, ACCOUNT, DATA_SET).await;
    assert_eq!(
        status, 200,
        "case at the bound: read expected 200, got {status} with {read}"
    );
    let actual = permissions_of(&read);
    assert_eq!(
        actual.len(),
        MAX,
        "case at the bound: expected {MAX} entries, got {:?} in {read}",
        actual.len()
    );

    // One past the bound.
    let (status, body) = update_permissions(
        &harness,
        ACCOUNT,
        DATA_SET,
        Some(json!([permission(&principals[MAX], &VIEWER_ACTIONS)])),
        None,
    )
    .await;
    assert_eq!(
        status, 400,
        "case past the bound: expected 400, got {status} with {body}"
    );
    assert_eq!(
        body["__type"],
        json!("InvalidParameterValueException"),
        "case past the bound: expected InvalidParameterValueException, got {body}"
    );
    let (status, read) = describe_permissions(&harness, ACCOUNT, DATA_SET).await;
    assert_eq!(
        status, 200,
        "case past the bound: read expected 200, got {status} with {read}"
    );
    let actual = permissions_of(&read);
    assert_eq!(
        actual.len(),
        MAX,
        "case past the bound: the refused grant was stored anyway: {read}"
    );
}

#[tokio::test]
async fn a_data_set_that_does_not_exist_is_not_found() {
    let harness = Harness::new();
    let analyst = user_arn(ACCOUNT, "analyst");
    create_data_set(&harness, ACCOUNT, DATA_SET, None).await;

    // Rows: a read and an update of an id nothing created, of a deleted data set, and
    // of an account that holds nothing.
    let (status, body) = harness
        .call(
            "DELETE",
            &format!("/accounts/{ACCOUNT}/data-sets/{DATA_SET}"),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "the fixture delete expected 200, got {status} with {body}"
    );

    let cases: [(&str, &str, &str); 3] = [
        ("an id nothing created", ACCOUNT, "perm-dset-absent"),
        ("a deleted data set", ACCOUNT, DATA_SET),
        ("an account holding nothing", "123456789012", DATA_SET),
    ];

    for (case, account, data_set_id) in cases {
        let (status, body) = describe_permissions(&harness, account, data_set_id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: read expected 404, got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "case {case:?}: read expected ResourceNotFoundException, got {body}"
        );

        let (status, body) = update_permissions(
            &harness,
            account,
            data_set_id,
            Some(json!([permission(&analyst, &VIEWER_ACTIONS)])),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: update expected 404, got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "case {case:?}: update expected ResourceNotFoundException, got {body}"
        );
    }
}
