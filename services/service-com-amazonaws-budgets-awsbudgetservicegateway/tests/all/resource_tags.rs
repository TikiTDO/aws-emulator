// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for this service's tag plane — `TagResource` and
//! `ListTagsForResource` — driven over the wire through the same `MultiServiceRouter`
//! the production router serves from.
//!
//! The RULES asserted here:
//!
//! 1. `TagResource` writes the supplied tags onto the resource its `ResourceARN` names,
//!    and `ListTagsForResource` reports exactly that set back. Both a budget and a budget
//!    action are such a resource ("a budget or budget action resource").
//! 2. A resource holds at most one tag per key ("each key must be unique for the
//!    resource"), so a supplied key the resource already carries RE-VALUES that tag and a
//!    key it does not carry is added — over the tags created with the resource as well as
//!    over tags a previous call wrote. A tag set naming one key twice states no value for
//!    it and is refused.
//! 3. The ARN is the whole identity of the resource: a well-formed budgets ARN naming a
//!    budget or action this service does not hold locates nothing, and a value that is
//!    not a budgets resource ARN is an invalid input value. Neither writes anything, and
//!    neither touches a resource the ARN nearly named.
//! 4. The tag set a resource holds is bounded by `ResourceTagList`'s own `@length`
//!    maximum, so a write that would take a resource past it is refused and changes
//!    nothing, while one that only re-values keys it already holds is accepted at the
//!    maximum.
//! 5. `ListTagsForResource` is a pure read: it reports the empty set for a resource
//!    nobody tagged, repeats its answer, and creates nothing.
//!
//! Each table spans its class rather than one request: the rows differ along the resource
//! kind (budget, budget action), the budget name's form, the number of tags (none, one,
//! several, the published maximum), and the key and value lengths at each published bound
//! and one past it.
//!
//! The harness (`router`, `call`, `is_error`, `caller_account`) and the budget/action
//! setup are the sibling suites', so every suite in this target drives the service
//! identically.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The ARN namespace and the tag ceiling are the service's own constants, so a table
/// built from them cannot drift from the code under test.
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    ARN_SERVICE, MAX_TAGS_PER_RESOURCE,
};

use super::budget_action_execution::{
    create_body, email_subscriber, iam_definition, seed_action, seed_budget,
};
use super::budget_lifecycle::{caller_account, call, is_error, router};

/// The ARN of a budget. AWS names a budget
/// `arn:{partition}:budgets::{account}:budget/{BudgetName}` — a global service, so the
/// region field is empty — and that layout is why `BudgetName`'s modelled `@pattern`
/// forbids ':' and the '/action/' substring inside a name.
/// `pub(crate)` so the sibling read-side suite names a budget's ARN the same way.
pub(crate) fn budget_arn(account_id: &str, budget_name: &str) -> String {
    format!("arn:aws:{ARN_SERVICE}::{account_id}:budget/{budget_name}")
}

/// The ARN of a budget action, which is named beneath the budget it acts on:
/// `…:budget/{BudgetName}/action/{ActionId}`.
pub(crate) fn action_arn(account_id: &str, budget_name: &str, action_id: &str) -> String {
    format!(
        "{}/action/{action_id}",
        budget_arn(account_id, budget_name)
    )
}

/// One `TagResource` call.
async fn tag(router: &MultiServiceRouter, resource_arn: &str, tags: &[(String, String)]) -> (u16, Value) {
    call(
        router,
        "TagResource",
        &json!({"ResourceARN": resource_arn, "ResourceTags": tag_list(tags)}),
    )
    .await
}

/// One `ListTagsForResource` call.
pub(crate) async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> (u16, Value) {
    call(
        router,
        "ListTagsForResource",
        &json!({"ResourceARN": resource_arn}),
    )
    .await
}

/// `(Key, Value)` pairs as the wire `ResourceTagList`.
pub(crate) fn tag_list(tags: &[(String, String)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect(),
    )
}

/// The `ResourceTags` an answer reports, as `(Key, Value)` pairs in the order they were
/// reported.
pub(crate) fn reported(body: &Value) -> Vec<(String, String)> {
    body["ResourceTags"]
        .as_array()
        .map(|tags| {
            tags.iter()
                .map(|tag| {
                    (
                        tag["Key"].as_str().unwrap_or_default().to_string(),
                        tag["Value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A `(Key, Value)` pair, so a table row reads as the tag it is.
pub(crate) fn pair(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// `n` distinct tags, for the rows that vary the SIZE of a tag set.
pub(crate) fn many_tags(n: usize) -> Vec<(String, String)> {
    (0..n)
        .map(|index| pair(&format!("key-{index}"), &format!("value-{index}")))
        .collect()
}

/// The forms a budget name takes, each accepted by `BudgetName`'s modelled `@pattern`
/// and each sitting inside the ARN's resource field, where the name is read back out of.
const NAME_FORMS: [(&str, &str); 3] = [
    ("plain", "monthly-cost"),
    ("holding a slash and spaces", "team/platform budget"),
    ("non-ascii", "予算"),
];

/// One row of the tag-set table: a case name and the tags one `TagResource` call supplies.
pub(crate) struct Row {
    pub(crate) case: String,
    pub(crate) tags: Vec<(String, String)>,
}

/// The tag-set table. Its rows differ along what the rule generalises over: the number of
/// tags (none, one, several, the `ResourceTagList` maximum), the key length at each
/// published `@length` bound of `ResourceTagKey` (1 and 128), the value length at each
/// published bound of `ResourceTagValue` (0 and 256), and non-ascii content.
///
/// `pub(crate)` so the sibling read-side suite asserts the create path over the SAME tag
/// sets rather than a second copy of this table.
pub(crate) fn tag_rows() -> Vec<Row> {
    vec![
        Row {
            case: "no tags at all".to_string(),
            tags: Vec::new(),
        },
        Row {
            // The case the issue recorded.
            case: "one tag".to_string(),
            tags: vec![pair("owner", "platform")],
        },
        Row {
            case: "several tags".to_string(),
            tags: vec![
                pair("owner", "platform"),
                pair("env", "prod"),
                pair("cost-center", "4815"),
            ],
        },
        Row {
            case: "a key at the published length minimum".to_string(),
            tags: vec![pair("k", "v")],
        },
        Row {
            case: "a key at the published length maximum".to_string(),
            tags: vec![pair(&"k".repeat(128), "v")],
        },
        Row {
            case: "a value at the published length minimum, which is empty".to_string(),
            tags: vec![pair("empty", "")],
        },
        Row {
            case: "a value at the published length maximum".to_string(),
            tags: vec![pair("long", &"v".repeat(256))],
        },
        Row {
            case: "non-ascii key and value".to_string(),
            tags: vec![pair("チーム", "予算-2026")],
        },
        Row {
            case: format!("the published maximum of {MAX_TAGS_PER_RESOURCE} tags in one call"),
            tags: many_tags(MAX_TAGS_PER_RESOURCE),
        },
    ]
}

/// RULE 1: the supplied tags are written onto the resource the `ResourceARN` names, and
/// the read reports exactly that set.
///
/// Every row of the tag-set table is run against BOTH kinds of taggable resource — a
/// budget and a budget action of that budget — and under each form a budget name takes,
/// so the rule holds over the tag set, the resource kind and the name the ARN carries.
#[tokio::test]
async fn tagging_writes_the_supplied_set_and_a_read_reports_it() {
    let router = router();
    let account_id = caller_account();

    for (name_case, form) in NAME_FORMS {
        for (index, row) in tag_rows().into_iter().enumerate() {
            let budget_name = format!("{form}-{index}");
            seed_budget(&router, &account_id, &budget_name).await;
            let action_id = seed_action(&router, &account_id, &budget_name).await;

            for (kind, arn) in [
                ("budget", budget_arn(&account_id, &budget_name)),
                ("budget action", action_arn(&account_id, &budget_name, &action_id)),
            ] {
                let case = format!("{} on a {kind} named {name_case}", row.case);
                let (status, body) = tag(&router, &arn, &row.tags).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected TagResource to answer 200, got {status} {body}"
                );

                let (status, body) = list_tags(&router, &arn).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected ListTagsForResource to answer 200, got \
                     {status} {body}"
                );
                assert_eq!(
                    reported(&body),
                    row.tags,
                    "case {case:?}: expected the read to report the tags that were written"
                );
            }
        }
    }
}

/// RULE 2: a resource holds at most one tag per key, so a supplied key it already carries
/// takes the new value in place — adding no tag — and a key it does not carry is added.
///
/// The steps are a sequence of `TagResource` calls over one resource, with the whole tag
/// set asserted after each: re-valuing a key, adding a key, an empty supply, a call doing
/// both at once, and a repeat of a call already applied. It is run twice: over a budget
/// whose tags came from `CreateBudget` (so the tags created WITH a resource take part in
/// the merge) and over an action tagged from empty.
#[tokio::test]
async fn a_key_the_resource_already_holds_is_revalued_in_place() {
    let router = router();
    let account_id = caller_account();
    let budget_name = "merged";

    // A budget created WITH tags, and an action of it created without any.
    let created = vec![pair("owner", "platform"), pair("env", "dev")];
    let (status, body) = call(
        &router,
        "CreateBudget",
        &json!({
            "AccountId": &account_id,
            "Budget": {
                "BudgetName": budget_name,
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "100", "Unit": "USD"}
            },
            "ResourceTags": tag_list(&created)
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBudget to answer 200, got {status} {body}"
    );
    let action_id = seed_action(&router, &account_id, budget_name).await;

    for (kind, arn, start) in [
        (
            "a budget carrying the tags it was created with",
            budget_arn(&account_id, budget_name),
            created.clone(),
        ),
        (
            "an action with no tags yet",
            action_arn(&account_id, budget_name, &action_id),
            Vec::new(),
        ),
    ] {
        // What the resource holds before the first step.
        let (status, body) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: expected the initial read to answer 200, got {status} {body}"
        );
        assert_eq!(
            reported(&body),
            start,
            "case {kind:?}: expected the resource to start from the tags it was created with"
        );

        let mut held = start;
        for (step, supplied) in [
            ("the first tag written by a call", vec![pair("owner", "fin-ops")]),
            ("a key the resource does not carry", vec![pair("team", "cost")]),
            ("an empty tag set", Vec::new()),
            (
                "a re-value and an add in one call",
                vec![pair("team", "finance"), pair("cost-center", "4815")],
            ),
            (
                "a call already applied, repeated",
                vec![pair("team", "finance")],
            ),
        ] {
            let case = format!("{step} on {kind}");
            let (status, body) = tag(&router, &arn, &supplied).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected TagResource to answer 200, got {status} {body}"
            );
            // The expected set, derived from the rule rather than written out per step:
            // a supplied key already held is re-valued where it is, and one that is not
            // held is appended.
            for (key, value) in supplied {
                match held.iter_mut().find(|(existing, _)| existing == &key) {
                    Some(existing) => existing.1 = value,
                    None => held.push((key, value)),
                }
            }

            let (status, body) = list_tags(&router, &arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected ListTagsForResource to answer 200, got {status} {body}"
            );
            assert_eq!(
                reported(&body),
                held,
                "case {case:?}: expected one tag per key, with a known key re-valued in place"
            );
        }
    }
}

/// RULE 2, the refusal half: one call cannot give a key two values, because a resource
/// holds at most one tag per key. The request is refused with `InvalidParameterException`
/// and writes nothing — not even the keys it named once.
///
/// The rows differ along where the repeat sits (adjacent, separated, the only two tags)
/// and on whether the repeated key is one the resource already holds.
#[tokio::test]
async fn a_tag_set_naming_one_key_twice_is_refused_and_writes_nothing() {
    let router = router();
    let account_id = caller_account();
    let budget_name = "duplicates";
    seed_budget(&router, &account_id, budget_name).await;
    let arn = budget_arn(&account_id, budget_name);

    let held = vec![pair("owner", "platform")];
    let (status, body) = tag(&router, &arn, &held).await;
    assert_eq!(
        status, 200,
        "setup: expected the first TagResource to answer 200, got {status} {body}"
    );

    for (case, supplied) in [
        (
            "the same key twice, adjacent",
            vec![pair("env", "dev"), pair("env", "prod")],
        ),
        (
            "the same key twice, separated by another",
            vec![pair("env", "dev"), pair("team", "cost"), pair("env", "prod")],
        ),
        (
            "a key the resource already holds, given twice",
            vec![pair("owner", "a"), pair("owner", "b")],
        ),
    ] {
        let (status, body) = tag(&router, &arn, &supplied).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the duplicate key to be refused, got {status} {body}"
        );
        assert!(
            is_error(&body, "InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {body}"
        );

        let (_, body) = list_tags(&router, &arn).await;
        assert_eq!(
            reported(&body),
            held,
            "case {case:?}: expected the refused call to write nothing"
        );
    }
}

/// RULE 3: the ARN is the whole identity of the resource.
///
/// A well-formed budgets ARN naming a resource this service does not hold is a
/// `NotFoundException`; a value that is not a budgets resource ARN at all is an
/// `InvalidParameterException`. Both halves are asserted for the write AND for the read,
/// and neither leaves a mark on the resources the ARNs nearly named.
///
/// The not-found rows differ along which part of the identity is wrong (the
/// budget name, the action id, the budget an action id belongs to); the invalid rows along
/// how the ARN departs from the form (not an ARN, another service's ARN, a region where a
/// global service has none, an account that is not 12 digits, no `budget/` collection, an
/// empty name, an empty action id).
#[tokio::test]
async fn an_arn_that_names_no_resource_of_this_service_is_refused() {
    let router = router();
    let owner = caller_account();

    seed_budget(&router, &owner, "owning").await;
    seed_budget(&router, &owner, "sibling").await;
    let owned_action = seed_action(&router, &owner, "owning").await;
    let sibling_action = seed_action(&router, &owner, "sibling").await;

    // Each stored resource carries one tag, so a refusal that wrote something would show.
    let held = vec![pair("owner", "platform")];
    for arn in [
        budget_arn(&owner, "owning"),
        budget_arn(&owner, "sibling"),
        action_arn(&owner, "owning", &owned_action),
        action_arn(&owner, "sibling", &sibling_action),
    ] {
        let (status, body) = tag(&router, &arn, &held).await;
        assert_eq!(
            status, 200,
            "setup: expected TagResource({arn}) to answer 200, got {status} {body}"
        );
    }

    let absent_action = "11111111-2222-3333-4444-555555555555";
    // The account field is part of the identity too, but an ARN naming an account that
    // is not the caller's is not a lookup at all: it is refused by the one account gate
    // before the store is consulted, so those rows live in `account_scoping.rs`. Every
    // row here therefore carries the caller's own account and differs in the rest of the
    // identity.
    let not_found = [
        ("a budget name nobody created", budget_arn(&owner, "never-created")),
        (
            "a stored budget name in another case",
            budget_arn(&owner, "OWNING"),
        ),
        ("an action id no action carries", action_arn(&owner, "owning", absent_action)),
        (
            "an action id that belongs to another budget",
            action_arn(&owner, "owning", &sibling_action),
        ),
        (
            "an action under a budget nobody created",
            action_arn(&owner, "never-created", &owned_action),
        ),
    ];
    for (case, arn) in &not_found {
        for (operation, (status, body)) in [
            ("TagResource", tag(&router, arn, &[pair("team", "cost")]).await),
            ("ListTagsForResource", list_tags(&router, arn).await),
        ] {
            assert_eq!(
                status, 400,
                "case {case:?} via {operation}: expected the call to be refused, got \
                 {status} {body}"
            );
            assert!(
                is_error(&body, "NotFoundException"),
                "case {case:?} via {operation}: expected NotFoundException, got {body}"
            );
        }
    }

    let invalid = [
        ("a value that is not an ARN", "not-an-arn".to_string()),
        (
            "another service's ARN",
            format!("arn:aws:sns::{owner}:budget/owning"),
        ),
        (
            "a region, where a global service's ARN has none",
            format!("arn:aws:{ARN_SERVICE}:us-east-1:{owner}:budget/owning"),
        ),
        (
            "an account that is not 12 digits",
            format!("arn:aws:{ARN_SERVICE}::12345:budget/owning"),
        ),
        (
            "an account that is not digits at all",
            format!("arn:aws:{ARN_SERVICE}::not-an-acct:budget/owning"),
        ),
        (
            "no partition",
            format!("arn::{ARN_SERVICE}::{owner}:budget/owning"),
        ),
        (
            "a collection this service does not name",
            format!("arn:aws:{ARN_SERVICE}::{owner}:billing-view/owning"),
        ),
        (
            "no resource at all",
            format!("arn:aws:{ARN_SERVICE}::{owner}:"),
        ),
        (
            "an empty budget name",
            format!("arn:aws:{ARN_SERVICE}::{owner}:budget/"),
        ),
        (
            "an empty action id",
            format!("arn:aws:{ARN_SERVICE}::{owner}:budget/owning/action/"),
        ),
        (
            "too few ARN fields",
            format!("arn:aws:{ARN_SERVICE}:{owner}:budget/owning"),
        ),
    ];
    for (case, arn) in &invalid {
        for (operation, (status, body)) in [
            ("TagResource", tag(&router, arn, &[pair("team", "cost")]).await),
            ("ListTagsForResource", list_tags(&router, arn).await),
        ] {
            assert_eq!(
                status, 400,
                "case {case:?} via {operation}: expected the call to be refused, got \
                 {status} {body}"
            );
            assert!(
                is_error(&body, "InvalidParameterException"),
                "case {case:?} via {operation}: expected InvalidParameterException, got {body}"
            );
        }
    }

    // Not one of those refusals touched a resource that really exists.
    for arn in [
        budget_arn(&owner, "owning"),
        budget_arn(&owner, "sibling"),
        action_arn(&owner, "owning", &owned_action),
        action_arn(&owner, "sibling", &sibling_action),
    ] {
        let (status, body) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {arn:?}: expected the resource to stay readable, got {status} {body}"
        );
        assert_eq!(
            reported(&body),
            held,
            "case {arn:?}: expected the refusals to leave this resource's tags alone"
        );
    }
}

/// RULE 1, the isolation half: a resource's tags are its own. Tagging a budget leaves its
/// own actions and its sibling budgets untouched.
#[tokio::test]
async fn tags_belong_to_the_one_resource_the_arn_names() {
    let router = router();
    let owner = caller_account();

    seed_budget(&router, &owner, "owning").await;
    seed_budget(&router, &owner, "sibling").await;
    let first = seed_action(&router, &owner, "owning").await;
    let second = seed_action(&router, &owner, "owning").await;

    let targets = [
        ("the budget", budget_arn(&owner, "owning")),
        ("its sibling budget", budget_arn(&owner, "sibling")),
        ("its first action", action_arn(&owner, "owning", &first)),
        ("its second action", action_arn(&owner, "owning", &second)),
    ];

    // Each target gets a tag set of its own, written one target at a time.
    for (index, (case, arn)) in targets.iter().enumerate() {
        let (status, body) = tag(&router, arn, &[pair("target", &index.to_string())]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected TagResource to answer 200, got {status} {body}"
        );
    }

    for (index, (case, arn)) in targets.iter().enumerate() {
        let (status, body) = list_tags(&router, arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListTagsForResource to answer 200, got {status} {body}"
        );
        assert_eq!(
            reported(&body),
            vec![pair("target", &index.to_string())],
            "case {case:?}: expected each resource to report only its own tags"
        );
    }
}

/// RULE 4: the tag set a resource holds is bounded by `ResourceTagList`'s published
/// `@length` maximum. A resource at that maximum still accepts a write that only
/// re-values keys it already holds (the count does not grow), and refuses one that would
/// add a key past it with `ServiceQuotaExceededException`, changing nothing.
///
/// The rows run the ceiling from one past it, from exactly at it, and with a supply that
/// mixes known and new keys.
#[tokio::test]
async fn a_write_past_the_published_tag_maximum_is_refused() {
    let router = router();
    let account_id = caller_account();
    let budget_name = "at-the-ceiling";
    seed_budget(&router, &account_id, budget_name).await;
    let arn = budget_arn(&account_id, budget_name);

    // Exactly the maximum, in one call: accepted.
    let full = many_tags(MAX_TAGS_PER_RESOURCE);
    let (status, body) = tag(&router, &arn, &full).await;
    assert_eq!(
        status, 200,
        "case \"exactly the maximum\": expected TagResource to answer 200, got {status} {body}"
    );
    let (_, body) = list_tags(&router, &arn).await;
    assert_eq!(
        reported(&body).len(),
        MAX_TAGS_PER_RESOURCE,
        "case \"exactly the maximum\": expected the resource to hold the whole set"
    );

    for (case, supplied) in [
        ("one key past the maximum", vec![pair("one-too-many", "x")]),
        (
            "a known key together with a new one",
            vec![pair("key-0", "revalued"), pair("one-too-many", "x")],
        ),
        ("several keys past the maximum", many_tags(MAX_TAGS_PER_RESOURCE + 5)[MAX_TAGS_PER_RESOURCE..].to_vec()),
    ] {
        let (status, body) = tag(&router, &arn, &supplied).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the write to be refused, got {status} {body}"
        );
        assert!(
            is_error(&body, "ServiceQuotaExceededException"),
            "case {case:?}: expected ServiceQuotaExceededException, got {body}"
        );
        let (_, body) = list_tags(&router, &arn).await;
        assert_eq!(
            reported(&body),
            full,
            "case {case:?}: expected the refused write to change nothing"
        );
    }

    // A write that only re-values keys the resource already holds keeps it at the
    // maximum, so it is accepted.
    let revalued: Vec<(String, String)> = full
        .iter()
        .map(|(key, _)| pair(key, "revalued"))
        .collect();
    let (status, body) = tag(&router, &arn, &revalued).await;
    assert_eq!(
        status, 200,
        "case \"re-valuing every key at the maximum\": expected 200, got {status} {body}"
    );
    let (_, body) = list_tags(&router, &arn).await;
    assert_eq!(
        reported(&body),
        revalued,
        "case \"re-valuing every key at the maximum\": expected every value to be replaced"
    );
}

/// RULE 4, the modelled half: a value outside a constraint the MODEL declares is refused
/// at request validation, before this handler runs — one tag past `ResourceTagList`'s
/// maximum, a key past `ResourceTagKey`'s, a value past `ResourceTagValue`'s, and a key
/// shorter than `ResourceTagKey`'s minimum — and nothing is written.
#[tokio::test]
async fn a_tag_outside_a_modelled_constraint_is_refused_at_validation() {
    let router = router();
    let account_id = caller_account();
    let budget_name = "validated";
    seed_budget(&router, &account_id, budget_name).await;
    let arn = budget_arn(&account_id, budget_name);

    for (case, supplied) in [
        (
            "one tag past the published list maximum",
            many_tags(MAX_TAGS_PER_RESOURCE + 1),
        ),
        (
            "a key one past its published length maximum",
            vec![pair(&"k".repeat(129), "v")],
        ),
        (
            "a key shorter than its published length minimum",
            vec![pair("", "v")],
        ),
        (
            "a value one past its published length maximum",
            vec![pair("long", &"v".repeat(257))],
        ),
    ] {
        let (status, body) = tag(&router, &arn, &supplied).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected request validation to refuse the call, got {status} {body}"
        );
        assert!(
            is_error(&body, "ValidationException"),
            "case {case:?}: expected ValidationException, got {body}"
        );
        let (_, body) = list_tags(&router, &arn).await;
        assert!(
            reported(&body).is_empty(),
            "case {case:?}: expected the refused call to write nothing, got {body}"
        );
    }
}

/// RULE 5: `ListTagsForResource` reports what the store holds and changes nothing — a
/// resource nobody tagged reports NO tags (HTTP 200 without the member, which is not the
/// same answer as "no such resource"), repeated reads agree, and a read neither creates a
/// resource nor a tag.
///
/// The rows are both kinds of taggable resource, each read before any tag is written,
/// twice in a row, and again after a write.
#[tokio::test]
async fn reading_tags_reports_the_empty_set_and_changes_nothing() {
    let router = router();
    let account_id = caller_account();
    let budget_name = "untagged";
    seed_budget(&router, &account_id, budget_name).await;
    let action_id = seed_action(&router, &account_id, budget_name).await;

    for (kind, arn) in [
        ("budget", budget_arn(&account_id, budget_name)),
        (
            "budget action",
            action_arn(&account_id, budget_name, &action_id),
        ),
    ] {
        for attempt in 0..2 {
            let (status, body) = list_tags(&router, &arn).await;
            assert_eq!(
                status, 200,
                "case {kind:?} read {attempt}: expected 200 for an untagged resource, got \
                 {status} {body}"
            );
            // An optional collection a read has nothing to report for is left out, not
            // reported empty (`tests/all/recorded_response_members.rs`), and the resource
            // is still located: the answer is 200 rather than a `NotFoundException`.
            assert!(
                body.get("ResourceTags").is_none(),
                "case {kind:?} read {attempt}: expected no ResourceTags member for an \
                 untagged resource, got {body}"
            );
            assert!(
                reported(&body).is_empty(),
                "case {kind:?} read {attempt}: expected no tags, got {body}"
            );
        }

        let written = vec![pair("owner", "platform")];
        let (status, body) = tag(&router, &arn, &written).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: expected TagResource to answer 200, got {status} {body}"
        );
        for attempt in 0..2 {
            let (status, body) = list_tags(&router, &arn).await;
            assert_eq!(
                status, 200,
                "case {kind:?} re-read {attempt}: expected 200, got {status} {body}"
            );
            assert_eq!(
                reported(&body),
                written,
                "case {kind:?} re-read {attempt}: expected the same answer every time"
            );
        }
    }

    // A read never minted the resource it failed to find: the budget the reads above
    // never named still does not exist.
    let (status, body) = list_tags(&router, &budget_arn(&account_id, "never-read")).await;
    assert_eq!(
        status, 400,
        "expected a read of an absent budget to stay absent, got {status} {body}"
    );
    assert!(
        is_error(&body, "NotFoundException"),
        "expected NotFoundException, got {body}"
    );
}

/// An action created WITH `ResourceTags` reports them: the tags created with a resource
/// and the tags a later call writes are one set, read through one operation.
#[tokio::test]
async fn an_action_created_with_tags_reports_them() {
    let router = router();
    let account_id = caller_account();
    let budget_name = "tagged-at-creation";
    seed_budget(&router, &account_id, budget_name).await;

    let created = vec![pair("team", "platform"), pair("env", "dev")];
    let (status, body) = call(
        &router,
        "CreateBudgetAction",
        &create_body(
            &account_id,
            budget_name,
            "APPLY_IAM_POLICY",
            &iam_definition(),
            "AUTOMATIC",
            "FORECASTED",
            &json!({"ActionThresholdValue": 80.0, "ActionThresholdType": "PERCENTAGE"}),
            &json!([email_subscriber(0)]),
            Some(tag_list(&created)),
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBudgetAction to answer 200, got {status} {body}"
    );
    let action_id = body["ActionId"].as_str().unwrap_or_default().to_string();
    let arn = action_arn(&account_id, budget_name, &action_id);

    let (status, body) = list_tags(&router, &arn).await;
    assert_eq!(
        status, 200,
        "expected ListTagsForResource to answer 200, got {status} {body}"
    );
    assert_eq!(
        reported(&body),
        created,
        "expected the tags the action was created with to be reported"
    );

    let (status, body) = tag(&router, &arn, &[pair("env", "prod")]).await;
    assert_eq!(
        status, 200,
        "expected TagResource to answer 200, got {status} {body}"
    );
    let (_, body) = list_tags(&router, &arn).await;
    assert_eq!(
        reported(&body),
        vec![pair("team", "platform"), pair("env", "prod")],
        "expected the later write to re-value the key the create had already set"
    );
}
