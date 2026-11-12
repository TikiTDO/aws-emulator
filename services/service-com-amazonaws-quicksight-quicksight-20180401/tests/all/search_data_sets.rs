// SPDX-License-Identifier: CC0-1.0

//! `SearchDataSets`: finding an account's stored data sets through one filter.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so what is asserted is what a QuickSight SDK client would see:
//! the route, the status, and a body the handler computed from the shared store.
//!
//! The rule under test: a search reports the summaries of the calling account's stored
//! data sets that satisfy its one filter, where the filter's `Name` chooses what is
//! compared — the display name, or a principal ARN in the data set's permissions at
//! the access level the attribute names — and its `Operator` chooses how: `StringLike`
//! by substring (the model allows it for the name attribute only) or `StringEquals` in
//! full. So the rows below vary the inputs that rule generalises over: the attribute,
//! the operator, where in the name the value falls, how many data sets match, the
//! access each principal holds, the page size and the account.

use serde_json::{json, Value};

use super::data_source_data_set_lifecycle::Harness;

/// The account most rows use.
const ACCOUNT: &str = "000000000000";
/// A second, unrelated 12-digit account id, for the rows about account keying.
const OTHER_ACCOUNT: &str = "210987654321";

/// A user ARN used as a grantee. QuickSight principals are user or group ARNs.
fn user_arn(account: &str, user: &str) -> String {
    format!("arn:aws:quicksight:us-west-2:{account}:user/default/{user}")
}

/// The actions QuickSight grants a data set's owner: reading it plus the ones that
/// change it, its permissions or its existence.
fn owner_actions() -> Value {
    json!([
        "quicksight:DescribeDataSet",
        "quicksight:DescribeDataSetPermissions",
        "quicksight:PassDataSet",
        "quicksight:UpdateDataSet",
        "quicksight:UpdateDataSetPermissions",
        "quicksight:DeleteDataSet",
    ])
}

/// The actions QuickSight grants a data set's viewer: reading it and nothing more.
fn viewer_actions() -> Value {
    json!([
        "quicksight:DescribeDataSet",
        "quicksight:DescribeDataSetPermissions",
        "quicksight:PassDataSet",
    ])
}

/// Create the data source every fixture data set reads, and answer its ARN.
async fn data_source_fixture(harness: &Harness, account: &str) -> String {
    let (status, body) = harness
        .call(
            "POST",
            &format!("/accounts/{account}/data-sources"),
            Some(json!({
                "DataSourceId": "search-fixture-source",
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

/// A one-entry `PhysicalTableMap` reading `data_source_arn`.
fn relational_table_map(data_source_arn: &str) -> Value {
    json!({
        "orders-table": {
            "RelationalTable": {
                "DataSourceArn": data_source_arn,
                "Schema": "public",
                "Name": "orders",
                "InputColumns": [{ "Name": "order_id", "Type": "INTEGER" }],
            }
        }
    })
}

/// Create one data set, optionally with permissions, and assert the create succeeded.
async fn create_data_set(
    harness: &Harness,
    account: &str,
    data_source_arn: &str,
    data_set_id: &str,
    name: &str,
    permissions: Option<Value>,
) {
    let mut body = json!({
        "DataSetId": data_set_id,
        "Name": name,
        "ImportMode": "DIRECT_QUERY",
        "PhysicalTableMap": relational_table_map(data_source_arn),
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
}

/// One `SearchDataSets` call.
async fn search(harness: &Harness, account: &str, body: Value) -> (u16, Value) {
    harness
        .call(
            "POST",
            &format!("/accounts/{account}/search/data-sets"),
            Some(body),
        )
        .await
}

/// A search body carrying the one filter `(name, operator, value)`.
fn one_filter(name: &str, operator: &str, value: &str) -> Value {
    json!({ "Filters": [{ "Name": name, "Operator": operator, "Value": value }] })
}

/// The `DataSetId`s the response reports, in the order it reported them.
fn summary_ids(body: &Value) -> Vec<String> {
    body["DataSetSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("the response carries no DataSetSummaries list: {body}"))
        .iter()
        .map(|summary| {
            summary["DataSetId"]
                .as_str()
                .unwrap_or_else(|| panic!("a summary carries no DataSetId: {body}"))
                .to_string()
        })
        .collect()
}

/// Four data sets whose names differ in the parts a substring search can land on.
async fn name_fixture(harness: &Harness) -> String {
    let arn = data_source_fixture(harness, ACCOUNT).await;
    for (id, name) in [
        ("dset-a-regional", "Sales by Region"),
        ("dset-b-eu", "Sales EU"),
        ("dset-c-marketing", "Marketing Spend"),
        ("dset-d-logistics", "Freight"),
    ] {
        create_data_set(harness, ACCOUNT, &arn, id, name, None).await;
    }
    arn
}

#[tokio::test]
async fn a_name_filter_reports_the_data_sets_whose_name_the_value_matches() {
    let harness = Harness::new();
    name_fixture(&harness).await;

    // Rows: the operator, and where in the stored name the value falls — the whole
    // name, its start, its middle, its end, a value no name holds, and (for
    // StringEquals) a value that is only a substring of one.
    let cases: [(&str, &str, &str, Vec<&str>); 8] = [
        (
            "StringLike on the whole name",
            "StringLike",
            "Sales by Region",
            vec!["dset-a-regional"],
        ),
        (
            "StringLike on a leading substring matching two",
            "StringLike",
            "Sales",
            vec!["dset-a-regional", "dset-b-eu"],
        ),
        (
            "StringLike on a substring in the middle",
            "StringLike",
            "by",
            vec!["dset-a-regional"],
        ),
        (
            "StringLike on a trailing substring",
            "StringLike",
            "Spend",
            vec!["dset-c-marketing"],
        ),
        (
            "StringLike on a one-character substring",
            "StringLike",
            "F",
            vec!["dset-d-logistics"],
        ),
        (
            "StringLike on a value no name holds",
            "StringLike",
            "Logistics",
            vec![],
        ),
        (
            "StringEquals on the whole name",
            "StringEquals",
            "Sales EU",
            vec!["dset-b-eu"],
        ),
        (
            "StringEquals on a substring only",
            "StringEquals",
            "Sales",
            vec![],
        ),
    ];

    for (case, operator, value, expected) in cases {
        let (status, body) = search(
            &harness,
            ACCOUNT,
            one_filter("DATASET_NAME", operator, value),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        let actual = summary_ids(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?} in {body}"
        );
        assert!(
            body["RequestId"].as_str().is_some_and(|id| !id.is_empty()),
            "case {case:?}: expected a RequestId, got {body}"
        );
    }
}

#[tokio::test]
async fn a_matched_summary_reports_the_stored_data_set() {
    let harness = Harness::new();
    name_fixture(&harness).await;

    let (status, body) = search(
        &harness,
        ACCOUNT,
        one_filter("DATASET_NAME", "StringEquals", "Marketing Spend"),
    )
    .await;
    assert_eq!(
        status, 200,
        "search expected 200, got {status} with {body}"
    );
    let summary = &body["DataSetSummaries"][0];
    let (describe_status, described) = harness
        .call(
            "GET",
            &format!("/accounts/{ACCOUNT}/data-sets/dset-c-marketing"),
            None,
        )
        .await;
    assert_eq!(
        describe_status, 200,
        "describe expected 200, got {describe_status} with {described}"
    );
    let data_set = &described["DataSet"];
    // Rows: the members a summary and the stored data set share.
    for member in ["Arn", "DataSetId", "Name", "ImportMode", "CreatedTime"] {
        assert_eq!(
            summary[member], data_set[member],
            "case {member:?}: the summary disagrees with the stored data set: {summary} vs {data_set}"
        );
    }
}

#[tokio::test]
async fn an_ownership_filter_reports_the_data_sets_the_principal_can_reach() {
    let harness = Harness::new();
    let arn = data_source_fixture(&harness, ACCOUNT).await;
    let analyst = user_arn(ACCOUNT, "analyst");
    let auditor = user_arn(ACCOUNT, "auditor");
    let stranger = user_arn(ACCOUNT, "stranger");

    // Rows of fixture: the analyst sole owner; the analyst one of two owners; the
    // analyst a viewer only; and a data set nobody is granted.
    create_data_set(
        &harness,
        ACCOUNT,
        &arn,
        "dset-a-sole",
        "Sole",
        Some(json!([{ "Principal": analyst, "Actions": owner_actions() }])),
    )
    .await;
    create_data_set(
        &harness,
        ACCOUNT,
        &arn,
        "dset-b-shared",
        "Shared",
        Some(json!([
            { "Principal": analyst, "Actions": owner_actions() },
            { "Principal": auditor, "Actions": owner_actions() },
        ])),
    )
    .await;
    create_data_set(
        &harness,
        ACCOUNT,
        &arn,
        "dset-c-viewed",
        "Viewed",
        Some(json!([
            { "Principal": auditor, "Actions": owner_actions() },
            { "Principal": analyst, "Actions": viewer_actions() },
        ])),
    )
    .await;
    create_data_set(&harness, ACCOUNT, &arn, "dset-d-ungranted", "Ungranted", None).await;

    // Rows: each ownership attribute against a principal who owns, one who only
    // views, and one with no grant at all.
    let cases: [(&str, &str, &str, Vec<&str>); 9] = [
        (
            "owner, a principal owning two",
            "QUICKSIGHT_OWNER",
            &analyst,
            vec!["dset-a-sole", "dset-b-shared"],
        ),
        (
            "direct owner, the same principal",
            "DIRECT_QUICKSIGHT_OWNER",
            &analyst,
            vec!["dset-a-sole", "dset-b-shared"],
        ),
        (
            "owner, a principal owning two others",
            "QUICKSIGHT_OWNER",
            &auditor,
            vec!["dset-b-shared", "dset-c-viewed"],
        ),
        (
            "owner, a principal that only views",
            "QUICKSIGHT_OWNER",
            &stranger,
            vec![],
        ),
        (
            "viewer or owner, a principal that views one and owns two",
            "QUICKSIGHT_VIEWER_OR_OWNER",
            &analyst,
            vec!["dset-a-sole", "dset-b-shared", "dset-c-viewed"],
        ),
        (
            "direct viewer or owner, the same principal",
            "DIRECT_QUICKSIGHT_VIEWER_OR_OWNER",
            &analyst,
            vec!["dset-a-sole", "dset-b-shared", "dset-c-viewed"],
        ),
        (
            "sole owner, a principal sharing one of its two",
            "DIRECT_QUICKSIGHT_SOLE_OWNER",
            &analyst,
            vec!["dset-a-sole"],
        ),
        (
            "sole owner, a principal owning one alone",
            "DIRECT_QUICKSIGHT_SOLE_OWNER",
            &auditor,
            vec!["dset-c-viewed"],
        ),
        (
            "viewer or owner, a principal with no grant",
            "DIRECT_QUICKSIGHT_VIEWER_OR_OWNER",
            &stranger,
            vec![],
        ),
    ];

    for (case, attribute, principal, expected) in cases {
        let (status, body) = search(
            &harness,
            ACCOUNT,
            one_filter(attribute, "StringEquals", principal),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        let actual = summary_ids(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?} in {body}"
        );
    }
}

#[tokio::test]
async fn a_search_pages_through_its_matches() {
    let harness = Harness::new();
    let arn = data_source_fixture(&harness, ACCOUNT).await;
    for index in 0..5 {
        create_data_set(
            &harness,
            ACCOUNT,
            &arn,
            &format!("dset-{index}"),
            &format!("Sales {index}"),
            None,
        )
        .await;
    }
    // One data set the filter excludes, so paging is over the matches rather than
    // over the account's data sets.
    create_data_set(&harness, ACCOUNT, &arn, "dset-other", "Freight", None).await;

    // One row of the paging table: the case name, the `MaxResults` it asks for, and
    // the pages it expects, each a list of `DataSetId`s.
    type PagingCase = (&'static str, Option<i64>, Vec<Vec<&'static str>>);

    // Rows: a page size below, at and above the number of matches, and one absent.
    let cases: [PagingCase; 4] = [
        (
            "two at a time",
            Some(2),
            vec![
                vec!["dset-0", "dset-1"],
                vec!["dset-2", "dset-3"],
                vec!["dset-4"],
            ],
        ),
        (
            "one at a time",
            Some(1),
            vec![
                vec!["dset-0"],
                vec!["dset-1"],
                vec!["dset-2"],
                vec!["dset-3"],
                vec!["dset-4"],
            ],
        ),
        (
            "exactly the match count",
            Some(5),
            vec![vec!["dset-0", "dset-1", "dset-2", "dset-3", "dset-4"]],
        ),
        (
            "no page size",
            None,
            vec![vec!["dset-0", "dset-1", "dset-2", "dset-3", "dset-4"]],
        ),
    ];

    for (case, max_results, expected_pages) in cases {
        let mut next_token: Option<String> = None;
        let mut pages: Vec<Vec<String>> = Vec::new();
        loop {
            let mut body = one_filter("DATASET_NAME", "StringLike", "Sales");
            if let Some(max_results) = max_results {
                body["MaxResults"] = json!(max_results);
            }
            if let Some(token) = &next_token {
                body["NextToken"] = json!(token);
            }
            let (status, response) = search(&harness, ACCOUNT, body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} with {response}"
            );
            pages.push(summary_ids(&response));
            next_token = response["NextToken"].as_str().map(str::to_string);
            if next_token.is_none() {
                break;
            }
            assert!(
                pages.len() <= expected_pages.len(),
                "case {case:?}: expected {} page(s), got {pages:?}",
                expected_pages.len()
            );
        }
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected pages {expected_pages:?}, got {pages:?}"
        );
    }
}

#[tokio::test]
async fn a_search_sees_only_the_calling_accounts_data_sets() {
    let harness = Harness::new();
    let arn = data_source_fixture(&harness, ACCOUNT).await;
    let other_arn = data_source_fixture(&harness, OTHER_ACCOUNT).await;
    create_data_set(&harness, ACCOUNT, &arn, "dset-shared-id", "Sales", None).await;
    create_data_set(
        &harness,
        OTHER_ACCOUNT,
        &other_arn,
        "dset-shared-id",
        "Sales",
        None,
    )
    .await;
    create_data_set(
        &harness,
        OTHER_ACCOUNT,
        &other_arn,
        "dset-only-there",
        "Sales elsewhere",
        None,
    )
    .await;

    // Rows: the same filter asked of each account, and of an account holding nothing.
    let cases: [(&str, &str, Vec<&str>); 3] = [
        ("the first account", ACCOUNT, vec!["dset-shared-id"]),
        (
            "the second account",
            OTHER_ACCOUNT,
            vec!["dset-only-there", "dset-shared-id"],
        ),
        ("an account holding nothing", "123456789012", vec![]),
    ];
    for (case, account, expected) in cases {
        let (status, body) = search(
            &harness,
            account,
            one_filter("DATASET_NAME", "StringLike", "Sales"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        let actual = summary_ids(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?} in {body}"
        );
    }
}

#[tokio::test]
async fn a_filter_the_model_does_not_allow_is_refused() {
    let harness = Harness::new();
    name_fixture(&harness).await;

    // Rows: StringLike against each attribute that holds a principal ARN (the model
    // allows that operator for the name attribute only).
    let cases: [(&str, Value); 5] = [
        (
            "StringLike on QUICKSIGHT_OWNER",
            one_filter("QUICKSIGHT_OWNER", "StringLike", "analyst"),
        ),
        (
            "StringLike on DIRECT_QUICKSIGHT_OWNER",
            one_filter("DIRECT_QUICKSIGHT_OWNER", "StringLike", "analyst"),
        ),
        (
            "StringLike on QUICKSIGHT_VIEWER_OR_OWNER",
            one_filter("QUICKSIGHT_VIEWER_OR_OWNER", "StringLike", "analyst"),
        ),
        (
            "StringLike on DIRECT_QUICKSIGHT_VIEWER_OR_OWNER",
            one_filter("DIRECT_QUICKSIGHT_VIEWER_OR_OWNER", "StringLike", "analyst"),
        ),
        (
            "StringLike on DIRECT_QUICKSIGHT_SOLE_OWNER",
            one_filter("DIRECT_QUICKSIGHT_SOLE_OWNER", "StringLike", "analyst"),
        ),
    ];

    for (case, body) in cases {
        let (status, response) = search(&harness, ACCOUNT, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {response}"
        );
        assert_eq!(
            response["__type"],
            json!("InvalidParameterValueException"),
            "case {case:?}: expected InvalidParameterValueException, got {response}"
        );
    }
}

#[tokio::test]
async fn a_filter_value_outside_the_models_enums_is_refused() {
    let harness = Harness::new();
    name_fixture(&harness).await;

    // Rows: a value outside each of the filter's two enum catalogs, and an empty
    // one. The modelled constraint is refused before the handler runs.
    let cases: [(&str, Value); 4] = [
        (
            "an operator outside FilterOperator",
            one_filter("DATASET_NAME", "StringContains", "Sales"),
        ),
        (
            "an attribute outside DataSetFilterAttribute",
            one_filter("QUICKSIGHT_USER", "StringEquals", "analyst"),
        ),
        (
            "an empty attribute name",
            one_filter("", "StringEquals", "analyst"),
        ),
        (
            "an empty operator",
            one_filter("DATASET_NAME", "", "Sales"),
        ),
    ];

    for (case, body) in cases {
        let (status, response) = search(&harness, ACCOUNT, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {response}"
        );
        assert_eq!(
            response["__type"],
            json!("ValidationException"),
            "case {case:?}: expected ValidationException, got {response}"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let harness = Harness::new();
    name_fixture(&harness).await;

    // Rows: an opaque-looking token, this service's prefix with a non-hex body, a
    // prefix with an odd-length body, and an empty one.
    for token in ["not-a-token", "qs1-zzzz", "qs1-abc", ""] {
        let mut body = one_filter("DATASET_NAME", "StringLike", "Sales");
        body["NextToken"] = json!(token);
        let (status, response) = search(&harness, ACCOUNT, body).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected 400, got {status} with {response}"
        );
        assert_eq!(
            response["__type"],
            json!("InvalidNextTokenException"),
            "case {token:?}: expected InvalidNextTokenException, got {response}"
        );
    }
}

#[tokio::test]
async fn searching_changes_nothing_a_later_caller_sees() {
    let harness = Harness::new();
    name_fixture(&harness).await;

    // Rows: the same search run twice, then the list the search reads from, so a
    // search that consumed or moved anything would show up here.
    let mut bodies: Vec<Vec<String>> = Vec::new();
    for round in 0..2 {
        let (status, body) = search(
            &harness,
            ACCOUNT,
            one_filter("DATASET_NAME", "StringLike", "Sales"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case round {round}: expected 200, got {status} with {body}"
        );
        bodies.push(summary_ids(&body));
    }
    assert_eq!(
        bodies[0], bodies[1],
        "a repeated search answered differently: {:?} then {:?}",
        bodies[0], bodies[1]
    );

    let (status, listed) = harness
        .call("GET", &format!("/accounts/{ACCOUNT}/data-sets"), None)
        .await;
    assert_eq!(
        status, 200,
        "case list after search: expected 200, got {status} with {listed}"
    );
    let listed_ids = summary_ids(&listed);
    assert_eq!(
        listed_ids,
        vec![
            "dset-a-regional".to_string(),
            "dset-b-eu".to_string(),
            "dset-c-marketing".to_string(),
            "dset-d-logistics".to_string(),
        ],
        "case list after search: the searches changed what the list reports: {listed}"
    );
}
