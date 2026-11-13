// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the CREATE-time half of `ListTagsForResource`, driven over the
//! wire through the same `MultiServiceRouter` the production router serves from.
//!
//! `resource_tags.rs` asserts the tags a `TagResource` call writes being read back. A
//! resource's tags have a second source — the create that made it — and a lifetime, and
//! those are what this suite asserts:
//!
//! 1. The tags a resource is CREATED with are the tags its ARN reports, for a budget
//!    (`CreateBudget$ResourceTags`) and for a budget action
//!    (`CreateBudgetAction$ResourceTags`) alike, since both are "a budget or budget action
//!    resource", and a resource created with no `ResourceTags` member reports no tags.
//!    Asserted over the sibling suite's own tag-set table, so one table spans the class
//!    for the write path and the create path alike.
//! 2. That round trip holds over the whole range of tag COUNTS a resource can hold, not
//!    only the counts a table happens to name.
//! 3. A budget name the modelled `@pattern` admits is carried unchanged inside the ARN, so
//!    the budget its ARN resolves to is the one that was created.
//! 4. The tags belong to the resource: once the budget that owns them is gone, neither its
//!    own ARN nor its action's reports anything, and a budget created again under the same
//!    name starts from the tags THAT create supplied.
//!
//! The harness is shared with the sibling suites — `router`, `call`, `is_error` and
//! `caller_account` from the lifecycle suite, the `CreateBudgetAction` body builder from
//! the action suite, and the ARN, tag-list and tag-table helpers from `resource_tags.rs` —
//! so every suite in this target drives the service the same way.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

/// The tag ceiling is the service's own constant, so a table built from it cannot drift
/// from the code under test.
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::MAX_TAGS_PER_RESOURCE;

use super::budget_action_execution::{create_body, email_subscriber, iam_definition};
use super::budget_lifecycle::{caller_account, call, is_error, router};
use super::resource_tags::{
    action_arn, budget_arn, list_tags, many_tags, pair, reported, tag_list, tag_rows,
};

/// Create `budget_name` with `tags` when the row supplies them, and fail the test if the
/// setup call did not take.
///
/// `None` is the budget created with no `ResourceTags` MEMBER at all, which is a different
/// request from one carrying the empty list.
async fn seed_tagged_budget(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    tags: Option<&[(String, String)]>,
) {
    let mut request = json!({
        "AccountId": account_id,
        "Budget": {
            "BudgetName": budget_name,
            "TimeUnit": "MONTHLY",
            "BudgetType": "COST",
            "BudgetLimit": {"Amount": "100", "Unit": "USD"}
        }
    });
    if let Some(tags) = tags {
        request["ResourceTags"] = tag_list(tags);
    }
    let (status, body) = call(router, "CreateBudget", &request).await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBudget({budget_name}) to answer 200, got {status} {body}"
    );
}

/// Create one action on `budget_name` with `tags` when the row supplies them, and answer
/// the `ActionId` the service minted for it.
async fn seed_tagged_action(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    tags: Option<&[(String, String)]>,
) -> String {
    let request = create_body(
        account_id,
        budget_name,
        "APPLY_IAM_POLICY",
        &iam_definition(),
        "MANUAL",
        "ACTUAL",
        &json!({"ActionThresholdValue": 100.0, "ActionThresholdType": "PERCENTAGE"}),
        &json!([email_subscriber(0)]),
        tags.map(tag_list),
    );
    let (status, body) = call(router, "CreateBudgetAction", &request).await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBudgetAction({budget_name}) to answer 200, got {status} {body}"
    );
    body["ActionId"].as_str().unwrap_or_default().to_string()
}

/// The key an ACTION's copy of a row's tag carries: the budget's key behind a prefix,
/// trimmed to `ResourceTagKey`'s published 128-character maximum
/// (`smithy_query(query="shape",shape="ResourceTagKey")`), so a row sitting AT that
/// maximum is not pushed past it.
fn action_key(key: &str) -> String {
    const PREFIX: &str = "action-";
    const MAX_KEY: usize = 128;
    let kept: String = key.chars().take(MAX_KEY - PREFIX.len()).collect();
    format!("{PREFIX}{kept}")
}

/// The tags the ACTION of a row is created with: the row's tags under [`action_key`], so a
/// handler reporting the owning budget's tags for an action's ARN (or the other way round)
/// fails the row rather than passing it.
fn action_tags(tags: &[(String, String)]) -> Vec<(String, String)> {
    tags.iter()
        .map(|(key, value)| pair(&action_key(key), value))
        .collect()
}

/// One create-time row: the case name, and the `ResourceTags` the create supplies
/// (`None` for a create carrying no such member).
type CreateRow = (String, Option<Vec<(String, String)>>);

/// The create-time rows: the sibling suite's tag-set table, plus the one case a create has
/// and a `TagResource` call does not — a create carrying no `ResourceTags` member at all,
/// where the member is optional.
fn create_rows() -> Vec<CreateRow> {
    let mut rows = vec![("no ResourceTags member at all".to_string(), None)];
    rows.extend(
        tag_rows()
            .into_iter()
            .map(|row| (row.case, Some(row.tags))),
    );
    rows
}

/// RULE 1: the tags a resource is created with are the tags its ARN reports, for either
/// kind of taggable resource, and a resource created with none reports no tags at all.
#[tokio::test]
async fn the_tags_a_resource_is_created_with_are_reported_for_its_arn() {
    let router = router();
    let account_id = caller_account();

    for (index, (case, tags)) in create_rows().into_iter().enumerate() {
        let budget_name = format!("created-{index}");
        seed_tagged_budget(&router, &account_id, &budget_name, tags.as_deref()).await;
        // The action's own tags differ from its budget's, so the two cannot be confused.
        let on_action = action_tags(tags.as_deref().unwrap_or_default());
        let action_id =
            seed_tagged_action(&router, &account_id, &budget_name, Some(&on_action)).await;

        let on_budget = tags.unwrap_or_default();
        for (kind, arn, expected) in [
            (
                "budget",
                budget_arn(&account_id, &budget_name),
                on_budget.clone(),
            ),
            (
                "budget action",
                action_arn(&account_id, &budget_name, &action_id),
                on_action.clone(),
            ),
        ] {
            let (status, body) = list_tags(&router, &arn).await;
            assert_eq!(
                status, 200,
                "case {case:?} on a {kind}: expected 200, got {status} {body}"
            );
            assert_eq!(
                reported(&body),
                expected,
                "case {case:?} on a {kind}: expected the tags it was created with, got {body}"
            );
            // The member is reported only for a resource that HOLDS tags: an optional
            // collection with nothing to report is left out
            // (`tests/all/recorded_response_members.rs`).
            assert_eq!(
                body["ResourceTags"].is_array(),
                !expected.is_empty(),
                "case {case:?} on a {kind}: expected the ResourceTags member only for a \
                 resource holding tags, got {body}"
            );
        }
    }
}

/// RULE 2: the round trip holds over the whole range of tag counts one resource can hold.
/// The counts walk one by one up to a dozen and then by larger steps, so the rule is
/// asserted across the range and at the published bounds without an unbounded case count.
#[tokio::test]
async fn every_tag_count_reads_back_the_tags_it_was_created_with() {
    let router = router();
    let account_id = caller_account();

    let counts: Vec<usize> = (0..=12)
        .chain([25, 99, 100, 199, MAX_TAGS_PER_RESOURCE])
        .collect();
    for count in counts {
        let tags = many_tags(count);
        let budget_name = format!("count-{count:03}");
        seed_tagged_budget(&router, &account_id, &budget_name, Some(&tags)).await;

        let (status, body) = list_tags(&router, &budget_arn(&account_id, &budget_name)).await;
        assert_eq!(
            status, 200,
            "case {count} tags: expected 200, got {status} {body}"
        );
        assert_eq!(
            body["ResourceTags"].as_array().map(Vec::len).unwrap_or(0),
            count,
            "case {count} tags: expected {count} tags back, got {body}"
        );
        assert_eq!(
            reported(&body),
            tags,
            "case {count} tags: expected the tags it was created with, got {body}"
        );
    }
}

/// RULE 3: a budget name `BudgetName`'s modelled `@pattern` admits is carried unchanged
/// inside the ARN, so the budget the ARN resolves to is the one that was created.
///
/// The rows differ along the form of the name: slashes, spaces, non-ascii characters, the
/// word `action` where it is not the `/action/` separator, and each published `@length`
/// bound (`@length(1, 100)`).
#[tokio::test]
async fn a_budget_name_the_pattern_allows_is_resolved_out_of_its_arn() {
    let router = router();
    let account_id = caller_account();

    for (case, name) in [
        ("a plain name", "monthly-cost".to_string()),
        (
            "a slash that is not the action separator",
            "team/platform".to_string(),
        ),
        ("several slashes", "team/platform/finops".to_string()),
        ("the word action with one slash", "action/".to_string()),
        ("spaces", "monthly cost budget".to_string()),
        ("non-ascii characters", "予算-2026".to_string()),
        (
            "one character, the published length minimum",
            "x".to_string(),
        ),
        ("the published length maximum", "y".repeat(100)),
    ] {
        let tags = vec![pair("case", case)];
        seed_tagged_budget(&router, &account_id, &name, Some(&tags)).await;

        let (status, body) = list_tags(&router, &budget_arn(&account_id, &name)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            reported(&body),
            tags,
            "case {case:?}: expected the tags that name was created with, got {body}"
        );
    }
}

/// RULE 4: the tags belong to the resource. Once the budget that owns them is deleted,
/// neither its own ARN nor its action's reports anything — and a budget created again under
/// the same name starts from the tags THAT create supplied.
#[tokio::test]
async fn the_tags_of_a_deleted_budget_are_no_longer_reported() {
    let router = router();
    let account_id = caller_account();
    let first = vec![pair("owner", "platform")];
    seed_tagged_budget(&router, &account_id, "transient", Some(&first)).await;
    let action_id = seed_tagged_action(&router, &account_id, "transient", Some(&first)).await;

    let (status, body) = call(
        &router,
        "DeleteBudget",
        &json!({"AccountId": &account_id, "BudgetName": "transient"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected DeleteBudget to answer 200, got {status} {body}"
    );

    for (case, arn) in [
        ("the deleted budget", budget_arn(&account_id, "transient")),
        (
            "an action of the deleted budget",
            action_arn(&account_id, "transient", &action_id),
        ),
    ] {
        let (status, body) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the refusal at 400, got {status} {body}"
        );
        assert!(
            is_error(&body, "NotFoundException"),
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    let second = vec![pair("env", "prod")];
    seed_tagged_budget(&router, &account_id, "transient", Some(&second)).await;
    let (status, body) = list_tags(&router, &budget_arn(&account_id, "transient")).await;
    assert_eq!(
        status, 200,
        "case the re-created budget: expected 200, got {status} {body}"
    );
    assert_eq!(
        reported(&body),
        second,
        "case the re-created budget: expected the tags the second create supplied, got {body}"
    );
}
