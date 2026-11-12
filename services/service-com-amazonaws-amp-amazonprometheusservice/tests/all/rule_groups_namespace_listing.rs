// SPDX-License-Identifier: CC0-1.0

//! `ListRuleGroupsNamespaces`, end to end over HTTP.
//!
//! The rule under test: the listing reports the rule groups namespaces the NAMED
//! workspace holds, read off the persisted records, ordered by name, narrowed to the
//! names that BEGIN with the request's `name` when one is given, and served one page of
//! at most `maxResults` entries (default 100) with `nextToken` as the cursor to the next
//! page.
//!
//! Every suite spans the inputs that rule generalises over — the prefix (absent, a whole
//! name, a proper prefix, the same letters in the other case, one that matches nothing),
//! the page size (each published bound of `maxResults` and one past it, plus the
//! documented default), the number of stored namespaces, and the workspace a namespace
//! belongs to — rather than one recorded request's values.
//!
//! The request harness (`Amp`, `Reply`) and the create/describe helpers are the ones
//! `workspace_lifecycle` and `rule_groups_namespace_lifecycle` already build.

use crate::rule_groups_namespace_lifecycle::{create_body, create_namespace, describe_namespace};
use crate::workspace_lifecycle::{encode_label, tags_of, text, wait_for_due_transition, Amp, Reply};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    NAMESPACE_STATUS_ACTIVE, NAMESPACE_STATUS_CREATING, NO_FAILURE_STATUS_REASON,
    RULE_GROUPS_NAMESPACE_PAGE_DEFAULT, RULE_GROUPS_NAMESPACE_PAGE_LIMIT,
};
use std::collections::BTreeMap;

/// `GET /workspaces/{workspaceId}/rulegroupsnamespaces` — ListRuleGroupsNamespaces, with
/// the `@httpQuery` members the request names.
pub(crate) async fn list_namespaces(
    amp: &Amp,
    region: &str,
    workspace_id: &str,
    query: &[(&str, String)],
) -> Reply {
    let mut uri = format!("/workspaces/{workspace_id}/rulegroupsnamespaces");
    if !query.is_empty() {
        let encoded: Vec<String> = query
            .iter()
            .map(|(key, value)| format!("{key}={}", encode_label(value)))
            .collect();
        uri.push('?');
        uri.push_str(&encoded.join("&"));
    }
    amp.call("GET", &uri, region, "").await
}

/// The `name` of every summary on one page, in the order the page carries them.
pub(crate) fn names_of(case: &str, reply: &Reply) -> Vec<String> {
    let list = reply.body["ruleGroupsNamespaces"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: ruleGroupsNamespaces must be a list, got {reply:?}")
        });
    list.iter().map(|entry| text(&entry["name"])).collect()
}

/// The `nextToken` a page carries, `None` when it carries none.
pub(crate) fn next_token(reply: &Reply) -> Option<String> {
    reply.body["nextToken"]
        .as_str()
        .map(|token| token.to_string())
}

/// Walk every page the listing serves for one query, returning the names in page order
/// and how many pages it took.
async fn walk_pages(
    amp: &Amp,
    region: &str,
    workspace_id: &str,
    case: &str,
    base_query: &[(&str, String)],
) -> (Vec<String>, usize) {
    let mut collected: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut query = base_query.to_vec();
        if let Some(cursor) = token.as_ref() {
            query.push(("nextToken", cursor.clone()));
        }
        let page = list_namespaces(amp, region, workspace_id, &query).await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: page {pages} must answer 200, got {page:?}"
        );
        collected.extend(names_of(case, &page));
        pages += 1;
        token = next_token(&page);
        if token.is_none() {
            break;
        }
        assert!(
            pages < 1_000,
            "case {case:?}: the listing kept emitting a nextToken after {pages} pages"
        );
    }
    (collected, pages)
}

/// The names seeded for the prefix-filter table, and the one order the listing reports
/// them in (by name, the order the store keys them in).
///
/// They span what a prefix can distinguish: names where one is a prefix of another, the
/// same letters in the other case, and punctuation the modelled
/// `RuleGroupsNamespaceName` pattern `[0-9A-Za-z][-.0-9A-Z_a-z]*` allows.
const SEEDED_NAMES: &[&str] = &[
    "alphabet",
    "alpha",
    "rules2",
    "b",
    "Beta",
    "alpha.rules",
    "z",
    "beta",
    "rules1",
];

/// `SEEDED_NAMES` in the order the listing must report them: by name.
const ORDERED_NAMES: &[&str] = &[
    "Beta",
    "alpha",
    "alpha.rules",
    "alphabet",
    "b",
    "beta",
    "rules1",
    "rules2",
    "z",
];

/// Create every name of [`SEEDED_NAMES`] in a fresh ACTIVE workspace, returning its id.
async fn seeded_workspace(amp: &Amp, region: &str, alias: &str) -> String {
    let workspace_id = amp.active_workspace(region, alias).await;
    for name in SEEDED_NAMES {
        let created = create_namespace(
            amp,
            region,
            &workspace_id,
            &create_body(
                name,
                format!("groups: [{{name: {name}}}]").as_bytes(),
                &[("owner".to_string(), (*name).to_string())],
            ),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "seeding {name:?}: the create must answer 202, got {created:?}"
        );
    }
    workspace_id
}

/// The documented prefix filter, one row per thing a prefix can distinguish. The expected
/// subset of each row is written out rather than computed, so the test states the rule
/// instead of restating the implementation.
#[tokio::test]
async fn the_name_filter_keeps_only_the_names_that_begin_with_it() {
    let amp = Amp::new();
    let region = "us-west-2";
    let workspace_id = seeded_workspace(&amp, region, "listing-host").await;

    let rows: Vec<(&str, Option<&str>, Vec<&str>)> = vec![
        ("no filter lists every namespace", None, ORDERED_NAMES.to_vec()),
        (
            "a prefix shared by three names",
            Some("alpha"),
            vec!["alpha", "alpha.rules", "alphabet"],
        ),
        ("a longer prefix narrows to one", Some("alphab"), vec!["alphabet"]),
        ("a whole name is still a prefix of itself", Some("alphabet"), vec!["alphabet"]),
        (
            "a prefix that is also a whole name, and a prefix of another",
            Some("b"),
            vec!["b", "beta"],
        ),
        ("the filter is case-sensitive: upper", Some("B"), vec!["Beta"]),
        ("the filter is case-sensitive: lower", Some("be"), vec!["beta"]),
        ("a prefix spanning punctuation", Some("alpha."), vec!["alpha.rules"]),
        ("a digit-bearing prefix", Some("rules"), vec!["rules1", "rules2"]),
        ("a prefix matching the last name", Some("z"), vec!["z"]),
        ("a prefix nothing begins with", Some("nosuch"), vec![]),
        ("a prefix that differs only in case", Some("Z"), vec![]),
        (
            "a prefix one character past a stored name",
            Some("alphabets"),
            vec![],
        ),
    ];

    for (case, prefix, expected) in rows {
        let query: Vec<(&str, String)> = prefix
            .map(|value| vec![("name", value.to_string())])
            .unwrap_or_default();
        let reply = list_namespaces(&amp, region, &workspace_id, &query).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the listing must answer 200, got {reply:?}"
        );
        assert_eq!(
            names_of(case, &reply),
            expected,
            "case {case:?}: expected {expected:?}, got {reply:?}"
        );
        assert_eq!(
            next_token(&reply),
            None,
            "case {case:?}: one page holds every match, so no nextToken is owed, got {reply:?}"
        );
    }
}

/// Every summary member is read off the persisted record: the ARN and tags the create
/// minted, the status the clock owes, and the timestamps `DescribeRuleGroupsNamespace`
/// reports for the same namespace.
#[tokio::test]
async fn each_summary_reports_the_persisted_record() {
    let amp = Amp::new();
    let region = "eu-central-1";
    let workspace_id = seeded_workspace(&amp, region, "summary-host").await;

    // Straight after the creates the namespaces are still in their transient status, and
    // the listing reports that rather than a status nothing reached yet.
    let fresh = list_namespaces(&amp, region, &workspace_id, &[]).await;
    for entry in fresh.body["ruleGroupsNamespaces"]
        .as_array()
        .expect("a list of summaries")
    {
        let name = text(&entry["name"]);
        assert_eq!(
            text(&entry["status"]["statusCode"]),
            NAMESPACE_STATUS_CREATING,
            "case {name:?}: a fresh namespace lists as CREATING, got {entry:?}"
        );
    }

    wait_for_due_transition();

    let listed = list_namespaces(&amp, region, &workspace_id, &[]).await;
    let summaries = listed.body["ruleGroupsNamespaces"]
        .as_array()
        .expect("a list of summaries")
        .clone();
    assert_eq!(
        summaries.len(),
        ORDERED_NAMES.len(),
        "the listing must report every stored namespace, got {listed:?}"
    );

    for entry in &summaries {
        let name = text(&entry["name"]);
        let described = describe_namespace(&amp, region, &workspace_id, &name).await;
        let record = &described.body["ruleGroupsNamespace"];
        assert_eq!(
            text(&entry["arn"]),
            text(&record["arn"]),
            "case {name:?}: the summary's arn must be the stored one, got {entry:?}"
        );
        assert_eq!(
            text(&entry["status"]["statusCode"]),
            NAMESPACE_STATUS_ACTIVE,
            "case {name:?}: the clock owes the move to ACTIVE, got {entry:?}"
        );
        assert_eq!(
            entry["status"]["statusReason"].as_str(),
            Some(NO_FAILURE_STATUS_REASON),
            "case {name:?}: nothing failed, so the summary reports the empty reason the \
             service sends, got {entry:?}"
        );
        assert_eq!(
            entry["status"]["statusReason"].as_str(),
            record["status"]["statusReason"].as_str(),
            "case {name:?}: the summary's statusReason must be the one the describe \
             reports, got {entry:?}"
        );
        assert_eq!(
            entry["createdAt"].as_f64(),
            record["createdAt"].as_f64(),
            "case {name:?}: createdAt must be the stored instant, got {entry:?}"
        );
        assert_eq!(
            entry["modifiedAt"].as_f64(),
            record["modifiedAt"].as_f64(),
            "case {name:?}: modifiedAt must be the stored instant, got {entry:?}"
        );
        assert_eq!(
            tags_of(&entry["tags"]),
            BTreeMap::from([("owner".to_string(), name.clone())]),
            "case {name:?}: the summary's tags must be the stored ones, got {entry:?}"
        );
    }
}

/// `maxResults` bounds one page and `nextToken` carries the walk to the next: every page
/// size concatenates to the same single ordered list, and only a page with more behind it
/// carries a token. One row per page size, including each published bound of the modelled
/// `@range(min: 1, max: 1000)`.
#[tokio::test]
async fn pagination_walks_the_same_ordered_list_whatever_the_page_size() {
    let amp = Amp::new();
    let region = "ap-south-1";
    let workspace_id = seeded_workspace(&amp, region, "paging-host").await;
    let total = ORDERED_NAMES.len();

    for page_size in [1i32, 2, 3, 8, 9, 10, RULE_GROUPS_NAMESPACE_PAGE_LIMIT] {
        let case = format!("maxResults {page_size}");
        let (names, pages) = walk_pages(
            &amp,
            region,
            &workspace_id,
            &case,
            &[("maxResults", page_size.to_string())],
        )
        .await;
        assert_eq!(
            names,
            ORDERED_NAMES.to_vec(),
            "case {case:?}: the walk must report every namespace once, in name order, \
             got {names:?}"
        );
        let expected_pages = total.div_ceil(page_size.clamp(1, i32::MAX) as usize).max(1);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: {total} namespaces at {page_size} per page is {expected_pages} \
             pages, walked {pages}"
        );
    }

    // The filter holds across the page boundary too: "The other parameters must be the
    // same as the initial call."
    let (filtered, pages) = walk_pages(
        &amp,
        region,
        &workspace_id,
        "filtered, one per page",
        &[("maxResults", "1".to_string()), ("name", "alpha".to_string())],
    )
    .await;
    assert_eq!(
        filtered,
        vec!["alpha", "alpha.rules", "alphabet"],
        "case \"filtered, one per page\": the prefix must hold on every page, got {filtered:?}"
    );
    assert_eq!(
        pages, 3,
        "case \"filtered, one per page\": three matches at one per page is three pages, \
         walked {pages}"
    );

    // A page that holds every remaining entry owes no token.
    let whole = list_namespaces(
        &amp,
        region,
        &workspace_id,
        &[("maxResults", total.to_string())],
    )
    .await;
    assert_eq!(
        next_token(&whole),
        None,
        "case \"maxResults equals the total\": the last page carries no nextToken, got {whole:?}"
    );
}

/// The documented default page size: "The maximum number of results to return. The
/// default is 100." A request naming no `maxResults` gets that many and a token for the
/// rest; naming the published maximum gets them all.
#[tokio::test]
async fn an_omitted_max_results_serves_the_documented_default_page() {
    let amp = Amp::new();
    let region = "us-east-2";
    let workspace_id = amp.active_workspace(region, "default-page-host").await;

    let total = RULE_GROUPS_NAMESPACE_PAGE_DEFAULT as usize + 5;
    let mut expected: Vec<String> = Vec::new();
    for index in 0..total {
        let name = format!("ns{index:03}");
        let created = create_namespace(
            &amp,
            region,
            &workspace_id,
            &create_body(&name, b"groups: []", &[]),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "seeding {name:?}: the create must answer 202, got {created:?}"
        );
        expected.push(name);
    }
    expected.sort();

    let first = list_namespaces(&amp, region, &workspace_id, &[]).await;
    assert_eq!(
        first.status, 200,
        "case \"no maxResults\": the listing must answer 200, got {first:?}"
    );
    let first_names = names_of("no maxResults", &first);
    assert_eq!(
        first_names.len(),
        RULE_GROUPS_NAMESPACE_PAGE_DEFAULT as usize,
        "case \"no maxResults\": the default page holds {RULE_GROUPS_NAMESPACE_PAGE_DEFAULT} \
         entries, got {}",
        first_names.len()
    );
    assert_eq!(
        next_token(&first).as_deref(),
        first_names.last().map(|name| name.as_str()),
        "case \"no maxResults\": the token is the cursor past the last entry, got {first:?}"
    );

    let (walked, pages) = walk_pages(&amp, region, &workspace_id, "no maxResults", &[]).await;
    assert_eq!(
        walked, expected,
        "case \"no maxResults\": the walk must report all {total} namespaces in name order"
    );
    assert_eq!(
        pages, 2,
        "case \"no maxResults\": {total} namespaces at {RULE_GROUPS_NAMESPACE_PAGE_DEFAULT} \
         per page is two pages, walked {pages}"
    );

    let whole = list_namespaces(
        &amp,
        region,
        &workspace_id,
        &[("maxResults", RULE_GROUPS_NAMESPACE_PAGE_LIMIT.to_string())],
    )
    .await;
    assert_eq!(
        names_of("maxResults at the published maximum", &whole),
        expected,
        "case \"maxResults at the published maximum\": one page holds all {total}, got {whole:?}"
    );
}

/// `maxResults` is modelled `@range(min: 1, max: 1000)`: a value past either published
/// bound is refused with the modelled `ValidationException` (400) rather than served.
#[tokio::test]
async fn a_max_results_past_a_published_bound_is_refused() {
    let amp = Amp::new();
    let region = "eu-west-1";
    let workspace_id = seeded_workspace(&amp, region, "bounds-host").await;

    for (case, value) in [
        ("one below the published minimum", 0i32),
        ("negative", -1),
        (
            "one past the published maximum",
            RULE_GROUPS_NAMESPACE_PAGE_LIMIT + 1,
        ),
    ] {
        let refused = list_namespaces(
            &amp,
            region,
            &workspace_id,
            &[("maxResults", value.to_string())],
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: maxResults {value} must be refused with 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {refused:?}"
        );
    }

    // Each published bound itself is accepted.
    for (case, value) in [
        ("the published minimum", 1i32),
        ("the published maximum", RULE_GROUPS_NAMESPACE_PAGE_LIMIT),
    ] {
        let served = list_namespaces(
            &amp,
            region,
            &workspace_id,
            &[("maxResults", value.to_string())],
        )
        .await;
        assert_eq!(
            served.status, 200,
            "case {case:?}: maxResults {value} is inside the modelled range, got {served:?}"
        );
    }
}

/// A listing is a read: it reports what the store holds and changes nothing a later
/// caller sees. It is also scoped to ONE workspace, and a workspace the account does not
/// hold is the modelled `ResourceNotFoundException`.
#[tokio::test]
async fn the_listing_is_scoped_to_its_workspace_and_changes_nothing() {
    let amp = Amp::new();
    let region = "us-west-2";
    let first = amp.active_workspace(region, "first").await;
    let second = amp.active_workspace(region, "second").await;

    // A workspace that holds nothing lists the empty page, not an error.
    let empty = list_namespaces(&amp, region, &first, &[]).await;
    assert_eq!(
        empty.status, 200,
        "case \"empty workspace\": the listing must answer 200, got {empty:?}"
    );
    assert_eq!(
        names_of("empty workspace", &empty),
        Vec::<String>::new(),
        "case \"empty workspace\": nothing is stored, so the page is empty, got {empty:?}"
    );
    assert_eq!(
        next_token(&empty),
        None,
        "case \"empty workspace\": an empty page owes no nextToken, got {empty:?}"
    );

    for (case, workspace_id, names) in [
        ("first workspace", &first, vec!["only-here", "shared"]),
        ("second workspace", &second, vec!["shared"]),
    ] {
        for name in &names {
            let created = create_namespace(
                &amp,
                region,
                workspace_id,
                &create_body(name, b"groups: []", &[]),
            )
            .await;
            assert_eq!(
                created.status, 202,
                "case {case:?}: seeding {name:?} must answer 202, got {created:?}"
            );
        }
        let listed = list_namespaces(&amp, region, workspace_id, &[]).await;
        assert_eq!(
            names_of(case, &listed),
            names,
            "case {case:?}: the listing reports only this workspace's namespaces, got {listed:?}"
        );
    }

    // Reading twice reports the same page, and a describe afterwards still finds every
    // namespace: nothing was consumed by the read.
    let repeated = list_namespaces(&amp, region, &first, &[]).await;
    assert_eq!(
        names_of("read twice", &repeated),
        vec!["only-here", "shared"],
        "case \"read twice\": a listing must not change what a later caller sees, got {repeated:?}"
    );
    for name in ["only-here", "shared"] {
        let described = describe_namespace(&amp, region, &first, name).await;
        assert_eq!(
            described.status, 200,
            "case {name:?}: the namespace must survive the listing, got {described:?}"
        );
    }

    // A workspace id of the shape the service mints that no workspace holds.
    let absent = list_namespaces(
        &amp,
        region,
        "ws-00000000-0000-0000-0000-000000000000",
        &[],
    )
    .await;
    assert_eq!(
        absent.status, 404,
        "case \"absent workspace\": expected 404, got {absent:?}"
    );
    assert_eq!(
        absent.error_code(),
        "ResourceNotFoundException",
        "case \"absent workspace\": expected ResourceNotFoundException, got {absent:?}"
    );
}
