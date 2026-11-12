// SPDX-License-Identifier: CC0-1.0

//! Listing the State Manager associations an account holds, end to end: `ListAssociations`
//! reporting the associations `CreateAssociation` persisted, narrowed by the filters the
//! request carries and served one page at a time.
//!
//! RULE: a listing reports the associations the store holds for the Region the request was
//! signed for — each projected from the version in force — admitting an association only
//! when EVERY filter of the request admits it, and serving them in the store's own
//! ascending association-id order in pages of at most `MaxResults`, where the `NextToken` is
//! the id the next page starts at and a token shaped like no association id is
//! `InvalidNextToken`.
//!
//! Every case goes through the front door a client uses, over the dispatch
//! [`crate::state_manager_associations`] publishes, so this suite shares one calling
//! convention and one reading of the store with the create and update suites.
//!
//! The suites are tables. Each row is one input along the axis the rule generalises over —
//! which filter key narrows the listing, which value it carries, how many filters narrow
//! together, how big a page is, which Region the listing is taken in, and which shapes a
//! resume token can take — rather than the single listing the recorded units happened to
//! take.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::state_manager_associations::{
    call, call_in_region, created, described, error_code, service, without_overview, AGENT, NODE,
    OTHER_NODE, OTHER_REGION, REGION, SHELL,
};

/// `ListAssociations` with `body`, signed for `region`.
async fn list_in_region(
    router: &MultiServiceRouter,
    region: &str,
    body: &Value,
) -> (u16, Value) {
    call_in_region(router, region, "ListAssociations", body).await
}

/// The `Associations` a listing in `region` must report.
async fn listed_in_region(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    body: &Value,
) -> Value {
    let (status, parsed) = list_in_region(router, region, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListAssociations, got {status} with {parsed:?}"
    );
    parsed
}

/// The whole `ListAssociations` response of a listing in [`REGION`].
async fn listed(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    listed_in_region(router, case, REGION, body).await
}

/// The `AssociationId`s a listing response reports, in the order it reported them.
fn ids(response: &Value) -> Vec<String> {
    response["Associations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|association| {
            association["AssociationId"]
                .as_str()
                .unwrap_or("<unset>")
                .to_string()
        })
        .collect()
}

/// The refusal message of a failure body.
fn error_message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or("<no message>")
        .to_string()
}

/// One association the fixture creates: the label a row names it by, and the create that
/// persists it.
struct Fixture {
    label: &'static str,
    request: Value,
}

/// The associations every filter row is read against: one per form a create can take along
/// the axes the filters narrow on — the document, the node a create named the deprecated
/// way, the association name, and the targeting form (a tag, either resource-group target
/// key spelling, and node ids).
fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            label: "the agent document with no targets",
            request: json!({"Name": AGENT}),
        },
        Fixture {
            label: "the agent document on a node named the deprecated way",
            request: json!({"Name": AGENT, "InstanceId": NODE}),
        },
        Fixture {
            label: "the agent document on the other node, named the same way",
            request: json!({"Name": AGENT, "InstanceId": OTHER_NODE}),
        },
        Fixture {
            label: "the shell document on tag targets, with an association name",
            request: json!({
                "Name": SHELL,
                "Targets": [{"Key": "tag:Env", "Values": ["prod"]}],
                "Parameters": {"commands": ["echo one"]},
                "AssociationName": "nightly-shell",
                "ScheduleExpression": "rate(12 hours)",
            }),
        },
        Fixture {
            label: "the shell document on a resource group, named the colon way",
            request: json!({
                "Name": SHELL,
                "Targets": [{"Key": "resource-groups:Name", "Values": ["web-tier"]}],
                "Parameters": {"commands": ["echo two"]},
                "AssociationName": "web-tier-shell",
            }),
        },
        Fixture {
            label: "the shell document on a resource group, named the plain way",
            request: json!({
                "Name": SHELL,
                "Targets": [{"Key": "ResourceGroup", "Values": ["db-tier"]}],
                "Parameters": {"commands": ["echo three"]},
            }),
        },
        Fixture {
            label: "the shell document targeting a node by id",
            request: json!({
                "Name": SHELL,
                "Targets": [{"Key": "InstanceIds", "Values": [NODE]}],
                "Parameters": {"commands": ["echo four"]},
            }),
        },
    ]
}

/// Create every fixture in [`REGION`], answering the `(label, association id)` pairs and
/// the association each one reports — plus the id of one association created in
/// [`OTHER_REGION`], which no listing in `REGION` may report.
async fn with_fixtures(
    router: &MultiServiceRouter,
) -> (Vec<(&'static str, String)>, Vec<Value>, String) {
    let mut created_ids = Vec::new();
    let mut descriptions = Vec::new();
    for fixture in fixtures() {
        let description = created(router, fixture.label, &fixture.request).await;
        let id = description["AssociationId"]
            .as_str()
            .unwrap_or("<unset>")
            .to_string();
        created_ids.push((fixture.label, id));
        descriptions.push(description);
    }
    // An association is reached through the endpoint it was created at, so one created in
    // another Region is held under that Region and is listed only there.
    let (status, parsed) =
        call_in_region(router, OTHER_REGION, "CreateAssociation", &json!({"Name": AGENT})).await;
    assert_eq!(
        status, 200,
        "case \"the other Region's association\": expected HTTP 200 from CreateAssociation, got \
         {status} with {parsed:?}"
    );
    let elsewhere = parsed["AssociationDescription"]["AssociationId"]
        .as_str()
        .unwrap_or("<unset>")
        .to_string();
    (created_ids, descriptions, elsewhere)
}

/// One filter row: the filters a listing carries and the fixtures it must report.
struct FilterRow {
    case: &'static str,
    filters: Value,
    /// The labels of the fixtures the listing admits, in no particular order.
    admitted: Vec<&'static str>,
}

/// The filter table: one row per key the modelled `AssociationFilterKey` enum carries
/// (`crate::ops::types::ASSOCIATION_FILTER_KEY_VALUES`, via
/// `state::associations::is_association_filter_key`), each on a value that admits something
/// and a value that admits nothing, plus the rows where two filters narrow together.
fn filter_rows() -> Vec<FilterRow> {
    vec![
        FilterRow {
            case: "no filter at all lists every association of the Region",
            filters: json!([]),
            admitted: vec![
                "the agent document with no targets",
                "the agent document on a node named the deprecated way",
                "the agent document on the other node, named the same way",
                "the shell document on tag targets, with an association name",
                "the shell document on a resource group, named the colon way",
                "the shell document on a resource group, named the plain way",
                "the shell document targeting a node by id",
            ],
        },
        FilterRow {
            case: "Name names the agent document",
            filters: json!([{"key": "Name", "value": AGENT}]),
            admitted: vec![
                "the agent document with no targets",
                "the agent document on a node named the deprecated way",
                "the agent document on the other node, named the same way",
            ],
        },
        FilterRow {
            case: "Name names the shell document",
            filters: json!([{"key": "Name", "value": SHELL}]),
            admitted: vec![
                "the shell document on tag targets, with an association name",
                "the shell document on a resource group, named the colon way",
                "the shell document on a resource group, named the plain way",
                "the shell document targeting a node by id",
            ],
        },
        FilterRow {
            case: "Name names a document nothing is associated with",
            filters: json!([{"key": "Name", "value": "AWS-RunPowerShellScript"}]),
            admitted: vec![],
        },
        FilterRow {
            // "Filtering associations using the InstanceID attribute only returns legacy
            // associations created using the InstanceID attribute. Associations targeting
            // the managed node that are part of the Target Attributes ResourceGroup or Tags
            // aren't returned" — so the association that merely TARGETS that node is not
            // admitted.
            case: "InstanceId names the node a create named the deprecated way",
            filters: json!([{"key": "InstanceId", "value": NODE}]),
            admitted: vec!["the agent document on a node named the deprecated way"],
        },
        FilterRow {
            case: "InstanceId names the other node",
            filters: json!([{"key": "InstanceId", "value": OTHER_NODE}]),
            admitted: vec!["the agent document on the other node, named the same way"],
        },
        FilterRow {
            case: "InstanceId names a node no create named",
            filters: json!([{"key": "InstanceId", "value": "i-0f1e2d3c4b5a69780"}]),
            admitted: vec![],
        },
        FilterRow {
            case: "AssociationName names one association",
            filters: json!([{"key": "AssociationName", "value": "nightly-shell"}]),
            admitted: vec!["the shell document on tag targets, with an association name"],
        },
        FilterRow {
            case: "AssociationName names an association nothing carries",
            filters: json!([{"key": "AssociationName", "value": "weekly-shell"}]),
            admitted: vec![],
        },
        FilterRow {
            // The modelled `StatusName` values an association can report
            // (`crate::ops::types::ASSOCIATION_STATUS_NAME_VALUES`): an association nothing
            // has run is `Pending`.
            case: "AssociationStatusName names the status every association reports",
            filters: json!([{"key": "AssociationStatusName", "value": "Pending"}]),
            admitted: vec![
                "the agent document with no targets",
                "the agent document on a node named the deprecated way",
                "the agent document on the other node, named the same way",
                "the shell document on tag targets, with an association name",
                "the shell document on a resource group, named the colon way",
                "the shell document on a resource group, named the plain way",
                "the shell document targeting a node by id",
            ],
        },
        FilterRow {
            case: "AssociationStatusName names a status no association reports",
            filters: json!([{"key": "AssociationStatusName", "value": "Success"}]),
            admitted: vec![],
        },
        FilterRow {
            case: "AssociationStatusName names the failure status",
            filters: json!([{"key": "AssociationStatusName", "value": "Failed"}]),
            admitted: vec![],
        },
        FilterRow {
            case: "ResourceGroupName names a group targeted the colon way",
            filters: json!([{"key": "ResourceGroupName", "value": "web-tier"}]),
            admitted: vec!["the shell document on a resource group, named the colon way"],
        },
        FilterRow {
            case: "ResourceGroupName names a group targeted the plain way",
            filters: json!([{"key": "ResourceGroupName", "value": "db-tier"}]),
            admitted: vec!["the shell document on a resource group, named the plain way"],
        },
        FilterRow {
            case: "ResourceGroupName names a group nothing targets",
            filters: json!([{"key": "ResourceGroupName", "value": "api-tier"}]),
            admitted: vec![],
        },
        FilterRow {
            // Running an association is the SSM Agent's own poll from a managed node and
            // happens on no node here, so no association has a last-execution date either
            // side of the one a filter names.
            case: "LastExecutedBefore names an instant after every create",
            filters: json!([{"key": "LastExecutedBefore", "value": "2030-01-01T00:00:00Z"}]),
            admitted: vec![],
        },
        FilterRow {
            case: "LastExecutedAfter names an instant before every create",
            filters: json!([{"key": "LastExecutedAfter", "value": "2000-01-01T00:00:00Z"}]),
            admitted: vec![],
        },
        FilterRow {
            // No create or update of an association names a cloud connector, so no
            // association carries a connector id to compare.
            case: "CloudConnectorId names a connector no association carries",
            filters: json!([{"key": "CloudConnectorId", "value": "connector-one"}]),
            admitted: vec![],
        },
        FilterRow {
            case: "two filters narrow together",
            filters: json!([
                {"key": "Name", "value": SHELL},
                {"key": "AssociationName", "value": "nightly-shell"},
            ]),
            admitted: vec!["the shell document on tag targets, with an association name"],
        },
        FilterRow {
            case: "two filters that cannot both hold admit nothing",
            filters: json!([
                {"key": "Name", "value": AGENT},
                {"key": "AssociationName", "value": "nightly-shell"},
            ]),
            admitted: vec![],
        },
        FilterRow {
            case: "three filters narrow together",
            filters: json!([
                {"key": "Name", "value": AGENT},
                {"key": "InstanceId", "value": NODE},
                {"key": "AssociationStatusName", "value": "Pending"},
            ]),
            admitted: vec!["the agent document on a node named the deprecated way"],
        },
    ]
}

/// A listing reports the Region's associations that every filter of the request admits.
#[tokio::test]
async fn a_listing_reports_the_associations_its_filters_admit() {
    let (router, _engine) = service();
    let (created_ids, _descriptions, elsewhere) = with_fixtures(&router).await;

    for row in filter_rows() {
        let mut request = json!({});
        if !row.filters.as_array().is_some_and(|list| list.is_empty()) {
            request["AssociationFilterList"] = row.filters.clone();
        }
        let response = listed(&router, row.case, &request).await;
        let reported: BTreeSet<String> = ids(&response).into_iter().collect();
        let expected: BTreeSet<String> = created_ids
            .iter()
            .filter(|(label, _)| row.admitted.contains(label))
            .map(|(_, id)| id.clone())
            .collect();
        assert_eq!(
            reported, expected,
            "case {:?}: expected the associations {:?}, got {reported:?} from {response:?}",
            row.case, row.admitted
        );
        assert!(
            !reported.contains(&elsewhere),
            "case {:?}: an association created in another Region is not listed here, got \
             {reported:?}",
            row.case
        );
    }

    // The other Region lists its own association and none of this Region's.
    let response = listed_in_region(
        &router,
        "the other Region's listing",
        OTHER_REGION,
        &json!({}),
    )
    .await;
    assert_eq!(
        ids(&response),
        vec![elsewhere.clone()],
        "case \"the other Region's listing\": expected only the association created there, got \
         {response:?}"
    );
}

/// Filtering by an association's own id reports exactly that association, whichever
/// association it is.
#[tokio::test]
async fn filtering_by_an_association_id_reports_that_one_association() {
    let (router, _engine) = service();
    let (created_ids, _descriptions, _elsewhere) = with_fixtures(&router).await;

    for (label, id) in &created_ids {
        let response = listed(
            &router,
            label,
            &json!({"AssociationFilterList": [{"key": "AssociationId", "value": id}]}),
        )
        .await;
        let reported = ids(&response);
        assert_eq!(
            reported,
            vec![id.clone()],
            "case {label:?}: expected only the association {id}, got {reported:?}"
        );
    }
}

/// Every member a listing reports comes from the same place the describe of that
/// association reads it, so the two never disagree.
#[tokio::test]
async fn a_listed_association_agrees_with_its_describe() {
    let (router, _engine) = service();
    let (created_ids, _descriptions, _elsewhere) = with_fixtures(&router).await;

    let response = listed(&router, "the whole listing", &json!({})).await;
    let associations = response["Associations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        associations.len(),
        created_ids.len(),
        "case \"the whole listing\": expected {} associations, got {response:?}",
        created_ids.len()
    );

    // The members the `Association` shape shares with `AssociationDescription`
    // (`smithy_query(query="shape",shape="Association")`).
    let shared = [
        "Name",
        "InstanceId",
        "AssociationId",
        "AssociationVersion",
        "DocumentVersion",
        "Targets",
        "LastExecutionDate",
        "ScheduleExpression",
        "AssociationName",
        "ScheduleOffset",
        "Duration",
        "TargetMaps",
    ];
    for association in &associations {
        let id = association["AssociationId"].as_str().unwrap_or("<unset>");
        let case = format!("the listed association {id}");
        let description = described(&router, &case, &json!({"AssociationId": id})).await;
        for member in shared {
            assert_eq!(
                association[member], description[member],
                "case {case:?}: expected {member} to be {:?}, got {:?}",
                description[member], association[member]
            );
        }
        // An association nothing has run reports its own progress and no execution.
        assert_eq!(
            association["Overview"]["Status"],
            json!("Pending"),
            "case {case:?}: an association nothing has run is Pending, got {association:?}"
        );
        let detailed = association["Overview"]["DetailedStatus"].clone();
        assert!(
            detailed.is_null() || detailed == json!("Creating"),
            "case {case:?}: expected the creating detail while the create settles or none after \
             it, got {detailed:?}"
        );
        assert!(
            association["Overview"]["AssociationStatusAggregatedCount"].is_null()
                && association["LastExecutionDate"].is_null(),
            "case {case:?}: an association nothing has run reports no execution, got \
             {association:?}"
        );
        // The listing carries only the members its own shape declares.
        let carried: BTreeSet<&str> = association
            .as_object()
            .map(|members| members.keys().map(String::as_str).collect())
            .unwrap_or_default();
        let modelled: BTreeSet<&str> = shared
            .iter()
            .copied()
            .chain(["Overview"])
            .collect::<BTreeSet<&str>>();
        assert!(
            carried.is_subset(&modelled),
            "case {case:?}: expected only the members of the Association shape, got {carried:?}"
        );
    }
}

/// A listing is a read: taking it twice reports the same associations, and a later describe
/// still reports every one of them.
#[tokio::test]
async fn a_listing_changes_nothing_a_later_caller_sees() {
    let (router, _engine) = service();
    let (created_ids, descriptions, _elsewhere) = with_fixtures(&router).await;

    let first = listed(&router, "the first listing", &json!({})).await;
    let second = listed(&router, "the second listing", &json!({})).await;
    assert_eq!(
        ids(&first),
        ids(&second),
        "case \"two listings\": expected the same associations twice, got {first:?} then \
         {second:?}"
    );

    for ((label, id), description) in created_ids.iter().zip(&descriptions) {
        let read = described(&router, label, &json!({"AssociationId": id})).await;
        assert_eq!(
            without_overview(&read),
            without_overview(description),
            "case {label:?}: a listing left the association as the create persisted it, got \
             {read:?}"
        );
    }
}

/// A listing pages: every page holds at most `MaxResults` associations, the `NextToken` is
/// the id the next page starts at, and walking the pages reports every association exactly
/// once, in the store's own ascending order.
#[tokio::test]
async fn paging_a_listing_reports_every_association_exactly_once() {
    let (router, _engine) = service();
    let (created_ids, _descriptions, _elsewhere) = with_fixtures(&router).await;
    let held = created_ids.len();

    let mut whole = created_ids
        .iter()
        .map(|(_, id)| id.clone())
        .collect::<Vec<String>>();
    whole.sort();

    // The page sizes span the modelled range (`ListAssociationsRequest$MaxResults`, 1..50):
    // the minimum, sizes that do and do not divide the held count, the count itself, the
    // maximum, and the request that names no `MaxResults` at all.
    let sizes: &[Option<usize>] = &[
        Some(1),
        Some(2),
        Some(3),
        Some(held),
        Some(50),
        None,
    ];
    for size in sizes {
        let case = format!("a page size of {size:?}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({});
            if let Some(size) = size {
                request["MaxResults"] = json!(size);
            }
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let response = listed(&router, &case, &request).await;
            let page = ids(&response);
            let bound = size.unwrap_or(held);
            assert!(
                page.len() <= bound,
                "case {case:?}: expected at most {bound} associations in a page, got {page:?}"
            );
            if let Some(resumed) = &token {
                assert_eq!(
                    page.first().map(String::as_str),
                    Some(resumed.as_str()),
                    "case {case:?}: expected the page to start at the token {resumed}, got \
                     {page:?}"
                );
            }
            walked.extend(page.iter().cloned());
            token = response["NextToken"].as_str().map(str::to_string);
            pages += 1;
            assert!(
                pages <= held + 1,
                "case {case:?}: expected the walk to finish within {} pages, got {pages}",
                held + 1
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, whole,
            "case {case:?}: expected the walk to report every association once in ascending id \
             order, got {walked:?}"
        );
        let expected_pages = held.div_ceil(size.unwrap_or(held));
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, got {pages}"
        );
    }
}

/// A `NextToken` shaped like no association id was not issued here, so it is
/// `InvalidNextToken` rather than a page.
#[tokio::test]
async fn a_token_this_listing_never_issued_is_refused() {
    let (router, _engine) = service();
    let (created_ids, _descriptions, _elsewhere) = with_fixtures(&router).await;

    // Each row is a token outside the modelled `AssociationId` pattern
    // `^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`: a
    // word, an id of the right groups but the wrong widths, one group short, one group too
    // many, and a group that is not hexadecimal.
    let tokens = [
        ("a word", "next-page"),
        ("the groups too short", "0123456-0000-0000-0000-000000000000"),
        ("one group short", "01234567-0000-0000-000000000000"),
        (
            "one group too many",
            "01234567-0000-0000-0000-000000000000-0000",
        ),
        ("a group that is not hexadecimal", "0123456g-0000-0000-0000-000000000000"),
    ];
    for (case, token) in tokens {
        let (status, parsed) = list_in_region(&router, REGION, &json!({"NextToken": token})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a token this listing never issued, got \
             {status} with {parsed:?}"
        );
        let code = error_code(&parsed);
        assert_eq!(
            code, "InvalidNextToken",
            "case {case:?}: expected InvalidNextToken, got {code} with {parsed:?}"
        );
        let message = error_message(&parsed);
        assert_eq!(
            message, "The NextToken value is not valid",
            "case {case:?}: expected the service's own sentence, got {message:?}"
        );
    }

    // A token of the modelled form names a position in the store's own order, so a listing
    // resumed from one reports the associations at or after it and nothing before it.
    for (label, id) in &created_ids {
        let response = listed(&router, label, &json!({"NextToken": id})).await;
        let reported = ids(&response);
        assert!(
            reported.iter().all(|listed| listed >= id),
            "case {label:?}: expected a resumed listing to report only ids at or after {id}, got \
             {reported:?}"
        );
        assert!(
            reported.contains(id),
            "case {label:?}: expected a resumed listing to start at {id}, got {reported:?}"
        );
    }
}

/// A filter key outside the modelled enum, and a page size outside the modelled range, are
/// refused rather than served.
#[tokio::test]
async fn a_request_outside_the_model_is_refused() {
    let (router, _engine) = service();
    let (_created_ids, _descriptions, _elsewhere) = with_fixtures(&router).await;

    // (case, the request, what makes it malformed)
    let rows: &[(&str, Value)] = &[
        (
            "a filter key the enum does not carry",
            json!({"AssociationFilterList": [{"key": "DocumentVersion", "value": "1"}]}),
        ),
        (
            "a filter key of the right name in the wrong case",
            json!({"AssociationFilterList": [{"key": "name", "value": AGENT}]}),
        ),
        (
            "a page size below the modelled minimum",
            json!({"MaxResults": 0}),
        ),
        (
            "a page size above the modelled maximum",
            json!({"MaxResults": 51}),
        ),
    ];
    for (case, request) in rows {
        let (status, parsed) = list_in_region(&router, REGION, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a request outside the model, got {status} with \
             {parsed:?}"
        );
    }
}

/// What an update and a delete do to the listing: the listing reports the version in force,
/// and an association a delete removed is listed no more.
#[tokio::test]
async fn the_listing_follows_an_update_and_a_delete() {
    let (router, _engine) = service();
    let agent = created(
        &router,
        "the association that is updated",
        &json!({"Name": AGENT, "ScheduleExpression": "rate(30 minutes)"}),
    )
    .await;
    let shell = created(
        &router,
        "the association that is deleted",
        &json!({"Name": SHELL, "Parameters": {"commands": ["echo one"]}}),
    )
    .await;
    let agent_id = agent["AssociationId"].as_str().unwrap_or("<unset>").to_string();
    let shell_id = shell["AssociationId"].as_str().unwrap_or("<unset>").to_string();

    // The version in force is what the listing reports, so an update moves it.
    let (status, parsed) = call(
        &router,
        "UpdateAssociation",
        &json!({"AssociationId": agent_id, "ScheduleExpression": "rate(2 hours)"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the update\": expected HTTP 200 from UpdateAssociation, got {status} with \
         {parsed:?}"
    );

    let response = listed(&router, "after the update", &json!({})).await;
    let associations = response["Associations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let updated = associations
        .iter()
        .find(|association| association["AssociationId"] == json!(agent_id))
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        updated["AssociationVersion"],
        json!("2"),
        "case \"after the update\": expected the version in force, got {updated:?}"
    );
    assert_eq!(
        updated["ScheduleExpression"],
        json!("rate(2 hours)"),
        "case \"after the update\": expected the schedule the update supplied, got {updated:?}"
    );

    let (status, parsed) = call(
        &router,
        "DeleteAssociation",
        &json!({"AssociationId": shell_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the delete\": expected HTTP 200 from DeleteAssociation, got {status} with \
         {parsed:?}"
    );

    let response = listed(&router, "after the delete", &json!({})).await;
    let reported = ids(&response);
    assert_eq!(
        reported,
        vec![agent_id.clone()],
        "case \"after the delete\": expected only the association that was not deleted, got \
         {reported:?}"
    );
}
