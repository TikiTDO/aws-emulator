// SPDX-License-Identifier: CC0-1.0

//! `DescribePrefixLists` end-to-end: the Amazon Web Services service prefix lists a
//! Region publishes, and how a caller narrows and pages them.
//!
//! Every call goes over the wire the service actually speaks, through the shared
//! `super::ec2_query` harness: an `application/x-www-form-urlencoded` ec2Query POST
//! dispatched through the same `MultiServiceRouter` the emulator serves with, carrying
//! the Region in the SigV4 `Credential=` scope, which is where the service reads the
//! Region it is being asked about.
//!
//! The rules under test:
//!
//! 1. A Region reports one prefix list per service that has one, named
//!    `com.amazonaws.<region>.<service>`, each carrying an opaque `pl-` id and at least
//!    one IP address range. The id and the ranges are derived from the Region and the
//!    service, so they are STABLE across reads and DISTINCT between the lists of one
//!    Region.
//! 2. `PrefixListIds` reports the lists it names; an id that names none of the Region's
//!    prefix lists is refused with `InvalidPrefixListID.NotFound`.
//! 3. `Filters` (`prefix-list-id`, `prefix-list-name`) are ANDed with one another and
//!    ORed over each filter's values, with EC2's `*` and `?` wildcards; a filter the
//!    operation does not document is refused with `InvalidParameterValue`.
//! 4. `MaxResults` and `NextToken` page the matching lists: each page holds at most
//!    `MaxResults` of them, the walk reports every matching list exactly once, and the
//!    last page carries no token. A token this service did not mint is refused with
//!    `InvalidPaginationToken`, and a page size below 1 or above 1,000 is refused.
//! 5. `DryRun` performs nothing and answers 412 `DryRunOperation`.
//! 6. The describe is a read: repeating it answers the same prefix lists.
//!
//! Prefix list ORDER within a response is deliberately not asserted — EC2 says the order
//! of elements in a response might vary — so every comparison here is over a sorted set.
//! What the pagination test does rely on is that the order is STABLE across calls, which
//! is what makes a cursor token see each list exactly once.

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, filters, indexed, unescaped};

/// The `Filters` form fields for entries whose values are built at run time (a derived id
/// or name a previous call reported), on top of the harness's `filters` builder.
fn owned_filters(entries: &[(&str, Vec<String>)]) -> Vec<(String, String)> {
    let borrowed: Vec<(&str, Vec<&str>)> = entries
        .iter()
        .map(|(name, values)| (*name, values.iter().map(String::as_str).collect()))
        .collect();
    let rows: Vec<(&str, &[&str])> = borrowed
        .iter()
        .map(|(name, values)| (*name, values.as_slice()))
        .collect();
    filters(&rows)
}

/// The services a Region publishes an Amazon Web Services service prefix list for, as
/// `DescribePrefixLists` reports them.
///
/// The same catalog the handler derives from, and from the same source: the Amazon S3 and
/// Amazon DynamoDB service family, which is what "available Amazon Web Services services
/// in a prefix list format" (`DescribePrefixLists`) names — Amazon DynamoDB, Amazon S3 and
/// Amazon S3 Express One Zone — each with the IPv6 list AWS publishes beside its IPv4 one,
/// under the `ipv6` label the Amazon VPC User Guide's "AWS-managed prefix lists" page
/// gives it. Written out here so the handler's own catalog cannot drift from it unnoticed.
const SERVICES: [&str; 6] = [
    "dynamodb",
    "ipv6.dynamodb",
    "ipv6.s3",
    "ipv6.s3express",
    "s3",
    "s3express",
];

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it. This
/// operation reports facts of a Region and stores nothing, so the engine handle it hands
/// back is not needed here.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One `DescribePrefixLists` call signed for `region`, with `params` as extra form
/// fields. Answers the HTTP status and the response body.
async fn describe(
    router: &MultiServiceRouter,
    region: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, region, "DescribePrefixLists", params).await
}

/// One prefix list of a `DescribePrefixLists` response.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PrefixList {
    name: String,
    id: String,
    cidrs: Vec<String>,
}

/// Every prefix list in an ec2Query `DescribePrefixLists` body, sorted by name.
///
/// Each list carries exactly one `<prefixListId>`, one `<prefixListName>` and one
/// `<cidrSet>`, so the three element streams are read in document order and zipped —
/// which avoids splitting the `<prefixListSet>` items by hand, the `<cidrSet>` inside
/// each one being a nested list of `<item>`s itself.
fn prefix_lists(case: &str, body: &str) -> Vec<PrefixList> {
    let ids = elements(body, "prefixListId");
    let names = elements(body, "prefixListName");
    let cidr_sets = elements(body, "cidrSet");
    assert_eq!(
        ids.len(),
        names.len(),
        "case {case:?}: every prefix list carries an id and a name, got {body}"
    );
    assert_eq!(
        ids.len(),
        cidr_sets.len(),
        "case {case:?}: every prefix list carries a cidrSet, got {body}"
    );

    let mut lists: Vec<PrefixList> = ids
        .into_iter()
        .zip(names)
        .zip(cidr_sets)
        .map(|((id, name), cidr_set)| PrefixList {
            name,
            id,
            cidrs: cidr_set
                .split("<item")
                .skip(1)
                .map(|rest| {
                    let after_tag = rest.split_once('>').map(|(_, rest)| rest).unwrap_or(rest);
                    after_tag.split("</item>").next().unwrap_or_default().to_string()
                })
                .collect(),
        })
        .collect();
    lists.sort();
    lists
}

/// The prefix list names in a response, sorted.
fn list_names(case: &str, body: &str) -> Vec<String> {
    prefix_lists(case, body)
        .into_iter()
        .map(|list| list.name)
        .collect()
}

/// The names of every prefix list Region `region` publishes, sorted.
fn expected_names(region: &str) -> Vec<String> {
    let mut names: Vec<String> = SERVICES
        .iter()
        .map(|service| format!("com.amazonaws.{region}.{service}"))
        .collect();
    names.sort();
    names
}

/// Is `id` shaped like a prefix list id — `pl-` followed by lowercase hexadecimal
/// digits, the form AWS mints?
fn id_is_well_formed(id: &str) -> bool {
    id.strip_prefix("pl-").is_some_and(|digits| {
        !digits.is_empty()
            && digits
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// Is `cidr` a block of the ranges this emulator derives a prefix list's ranges from: a
/// `/21` of `198.18.0.0/15`, aligned on its own prefix length, or a `/48` of the IPv6
/// documentation range `2001:db8::/32`?
fn cidr_is_well_formed(cidr: &str) -> bool {
    let Some((address, length)) = cidr.split_once('/') else {
        return false;
    };
    if address.contains(':') {
        return length == "48" && address.starts_with("2001:db8:");
    }
    let octets: Vec<u32> = address
        .split('.')
        .filter_map(|octet| octet.parse::<u32>().ok())
        .filter(|octet| *octet <= 255)
        .collect();
    if octets.len() != 4 || length != "21" {
        return false;
    }
    octets[0] == 198
        && (octets[1] == 18 || octets[1] == 19)
        && octets[2].is_multiple_of(8)
        && octets[3] == 0
}

/// One row of a filter table: the case name, the `Filters` the request carries (each a
/// name and its values), and the prefix list names expected back. The values are owned
/// because most rows are built from the Region's own derived names and ids, which is what
/// the [`owned_filters`] builder above takes.
type FilterCase<'a> = (&'a str, Vec<(&'a str, Vec<String>)>, Vec<String>);

// ---------------------------------------------------------------------------
// Rule 1: a Region reports its own service prefix lists
// ---------------------------------------------------------------------------

/// Each Region reports one prefix list per service of the catalog, named for the Region
/// and the service, with an opaque id and at least one IP address range.
///
/// The rows differ in the SHAPE of the Region name the lists are derived from: a plain
/// direction, a compound direction, a `central` Region, and three-component Regions of
/// the GovCloud and China partitions.
#[tokio::test]
async fn each_region_reports_its_own_service_prefix_lists() {
    let router = emulator();
    let regions = [
        "us-east-1",
        "us-west-2",
        "eu-central-1",
        "ap-northeast-1",
        "us-gov-west-1",
        "cn-northwest-1",
    ];

    for region in regions {
        let (status, body) = describe(&router, region, &[]).await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected 200, got {status} {body}"
        );
        let lists = prefix_lists(region, &body);
        assert_eq!(
            list_names(region, &body),
            expected_names(region),
            "case {region:?}: expected one prefix list per service, got {body}"
        );
        for list in &lists {
            assert!(
                id_is_well_formed(&list.id),
                "case {region:?}: expected a pl- prefix list id, got {:?}",
                list.id
            );
            assert!(
                !list.cidrs.is_empty(),
                "case {region:?}: a prefix list reports the service's IP address range, got {list:?}"
            );
            for cidr in &list.cidrs {
                assert!(
                    cidr_is_well_formed(cidr),
                    "case {region:?}: expected an IPv4 block, got {cidr:?} in {list:?}"
                );
            }
        }

        // Two lists of one Region are two different lists: they share neither an id nor
        // a range, so a caller can tell them apart and route to each.
        let ids: Vec<&String> = lists.iter().map(|list| &list.id).collect();
        let cidrs: Vec<&String> = lists.iter().flat_map(|list| &list.cidrs).collect();
        for (index, id) in ids.iter().enumerate() {
            assert!(
                !ids[index + 1..].contains(id),
                "case {region:?}: two prefix lists share the id {id:?}, got {lists:?}"
            );
        }
        for (index, cidr) in cidrs.iter().enumerate() {
            assert!(
                !cidrs[index + 1..].contains(cidr),
                "case {region:?}: two prefix lists share the range {cidr:?}, got {lists:?}"
            );
        }
    }
}

/// A prefix list's id and ranges are the same on every read: a client that stored the id
/// of a Region's list finds the same list under it later.
#[tokio::test]
async fn a_prefix_list_keeps_its_id_and_ranges_across_reads() {
    let router = emulator();
    for region in ["us-east-1", "eu-west-1"] {
        let (_, first) = describe(&router, region, &[]).await;
        let (status, second) = describe(&router, region, &[]).await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected 200, got {status} {second}"
        );
        assert_eq!(
            prefix_lists(region, &second),
            prefix_lists(region, &first),
            "case {region:?}: a read must report the same ids and ranges"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: PrefixListIds
// ---------------------------------------------------------------------------

/// A list named by `PrefixListIds` is reported; naming none reports them all.
///
/// The rows differ in how many ids the request names, and the ids themselves come from a
/// previous unfiltered read rather than being written out here, since AWS mints them.
#[tokio::test]
async fn prefix_list_ids_select_the_lists_they_name() {
    let router = emulator();
    let region = "us-west-2";
    let (_, body) = describe(&router, region, &[]).await;
    let published = prefix_lists("catalog", &body);
    assert_eq!(
        published.len(),
        SERVICES.len(),
        "case \"catalog\": expected one list per service, got {body}"
    );
    let first = &published[0];
    let second = &published[1];

    let cases: [(&str, Vec<&str>, Vec<String>); 4] = [
        (
            "no ids",
            Vec::new(),
            published.iter().map(|list| list.name.clone()).collect(),
        ),
        (
            "one id",
            vec![first.id.as_str()],
            vec![first.name.clone()],
        ),
        (
            "the other id",
            vec![second.id.as_str()],
            vec![second.name.clone()],
        ),
        (
            "both ids",
            vec![first.id.as_str(), second.id.as_str()],
            vec![first.name.clone(), second.name.clone()],
        ),
    ];

    for (case, ids, expected) in cases {
        let (status, body) = describe(&router, region, &indexed("PrefixListId", &ids)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let mut expected = expected;
        expected.sort();
        assert_eq!(
            list_names(case, &body),
            expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
    }
}

/// A `PrefixListIds` entry that names none of the Region's prefix lists is refused with
/// `InvalidPrefixListID.NotFound` naming it — not answered with an empty list.
///
/// The rows differ in how the id is wrong: a well-formed id nothing has, an id of
/// another Region's list, something that is not a prefix list id at all, and an unknown
/// id beside a known one.
#[tokio::test]
async fn an_unknown_prefix_list_id_is_refused() {
    let router = emulator();
    let region = "us-east-1";
    let (_, catalog) = describe(&router, region, &[]).await;
    let known = prefix_lists("catalog", &catalog)[0].id.clone();
    let (_, elsewhere) = describe(&router, "eu-west-1", &[]).await;
    let other_region = prefix_lists("elsewhere", &elsewhere)[0].id.clone();

    let cases: [(&str, Vec<&str>, &str); 4] = [
        ("unknown id", vec!["pl-00000000"], "pl-00000000"),
        (
            "another Region's list",
            vec![other_region.as_str()],
            other_region.as_str(),
        ),
        ("not an id at all", vec!["com.amazonaws.s3"], "com.amazonaws.s3"),
        (
            "an unknown id beside a known one",
            vec![known.as_str(), "pl-ffffffff"],
            "pl-ffffffff",
        ),
    ];

    for (case, ids, offending) in cases {
        let (status, body) = describe(&router, region, &indexed("PrefixListId", &ids)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some("InvalidPrefixListID.NotFound"),
            "case {case:?}: expected InvalidPrefixListID.NotFound, got {body}"
        );
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert!(
            message.contains(offending),
            "case {case:?}: the message must name {offending:?}, got {message:?}"
        );
        assert!(
            element(&body, "prefixListSet").is_none(),
            "case {case:?}: a refused request reports no prefix lists, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: Filters
// ---------------------------------------------------------------------------

/// Filters are ANDed with one another, each filter's values are ORed, and a value may
/// carry EC2's `*` / `?` wildcards.
///
/// The rows walk both filters the operation documents, each filter's value forms (exact,
/// multi-value, wildcarded), a value that matches nothing, and two filters at once —
/// matching and contradictory.
#[tokio::test]
async fn filters_are_anded_and_their_values_ored() {
    let router = emulator();
    let region = "eu-west-1";
    let (_, catalog) = describe(&router, region, &[]).await;
    let published = prefix_lists("catalog", &catalog);
    let dynamodb = format!("com.amazonaws.{region}.dynamodb");
    let s3 = format!("com.amazonaws.{region}.s3");
    let ipv6_s3 = format!("com.amazonaws.{region}.ipv6.s3");
    let all = expected_names(region);
    let dynamodb_id = published
        .iter()
        .find(|list| list.name == dynamodb)
        .map(|list| list.id.clone())
        .expect("the Region publishes a DynamoDB prefix list");

    let cases: Vec<FilterCase> = vec![
        (
            "prefix-list-name exact",
            vec![("prefix-list-name", vec![s3.clone()])],
            vec![s3.clone()],
        ),
        (
            "prefix-list-name two values are ORed",
            vec![("prefix-list-name", vec![s3.clone(), dynamodb.clone()])],
            vec![dynamodb.clone(), s3.clone()],
        ),
        (
            "prefix-list-name trailing *",
            vec![("prefix-list-name", vec!["com.amazonaws.*".to_string()])],
            all.clone(),
        ),
        (
            // Both the service's own list and the IPv6 list published beside it end in
            // the service's label, so a leading `*` selects the pair.
            "prefix-list-name leading *",
            vec![("prefix-list-name", vec!["*.s3".to_string()])],
            vec![ipv6_s3.clone(), s3.clone()],
        ),
        (
            "prefix-list-name ? is one character",
            vec![("prefix-list-name", vec!["*.s?".to_string()])],
            vec![ipv6_s3.clone(), s3.clone()],
        ),
        (
            "prefix-list-name ? needs a character",
            vec![("prefix-list-name", vec!["*.s3?".to_string()])],
            Vec::new(),
        ),
        (
            "prefix-list-name of another Region",
            vec![(
                "prefix-list-name",
                vec!["com.amazonaws.us-east-1.s3".to_string()],
            )],
            Vec::new(),
        ),
        (
            "prefix-list-name empty value",
            vec![("prefix-list-name", vec![String::new()])],
            Vec::new(),
        ),
        (
            // A filter with no value at all has nothing for the attribute to match, so
            // it selects nothing rather than everything — the same answer the other
            // describes of this service give a valueless filter.
            "prefix-list-name with no values",
            vec![("prefix-list-name", Vec::new())],
            Vec::new(),
        ),
        (
            "prefix-list-id exact",
            vec![("prefix-list-id", vec![dynamodb_id.clone()])],
            vec![dynamodb.clone()],
        ),
        (
            "prefix-list-id wildcard",
            vec![("prefix-list-id", vec!["pl-*".to_string()])],
            all.clone(),
        ),
        (
            "prefix-list-id no match",
            vec![("prefix-list-id", vec!["pl-00000000".to_string()])],
            Vec::new(),
        ),
        (
            "two filters are ANDed",
            vec![
                ("prefix-list-id", vec![dynamodb_id.clone()]),
                ("prefix-list-name", vec![dynamodb.clone()]),
            ],
            vec![dynamodb.clone()],
        ),
        (
            "contradictory filters match nothing",
            vec![
                ("prefix-list-id", vec![dynamodb_id.clone()]),
                ("prefix-list-name", vec![s3.clone()]),
            ],
            Vec::new(),
        ),
    ];

    for (case, entries, expected) in cases {
        let (status, body) = describe(&router, region, &owned_filters(&entries)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let mut expected = expected;
        expected.sort();
        assert_eq!(
            list_names(case, &body),
            expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
    }
}

/// A filter name this operation does not document is refused with
/// `InvalidParameterValue` naming it, rather than silently ignored.
///
/// The rows differ in how the name is wrong: a near-miss of a real filter, a
/// member-cased name, a filter of the MANAGED prefix list describe (`owner-id`), a tag
/// filter this operation documents no family for, and no name at all.
#[tokio::test]
async fn an_unknown_filter_name_is_refused() {
    let router = emulator();
    let cases: [(&str, &str); 5] = [
        ("plural near-miss", "prefix-list-names"),
        ("member casing", "PrefixListName"),
        ("another describe's filter", "owner-id"),
        ("a tag filter", "tag:Name"),
        ("empty name", ""),
    ];

    for (case, name) in cases {
        let (status, body) = describe(&router, "us-east-1", &filters(&[(name, &["anything"])])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some("InvalidParameterValue"),
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert!(
            message.contains(&format!("'{name}'")),
            "case {case:?}: the message must name the filter {name:?}, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: MaxResults and NextToken
// ---------------------------------------------------------------------------

/// A `NextToken` walk reports every prefix list exactly once, each page holds at most
/// `MaxResults` of them, and the last page carries no token.
///
/// The rows differ in the page size: one list at a time, exactly as many as there are,
/// and more than there are.
#[tokio::test]
async fn max_results_and_next_token_page_the_prefix_lists() {
    let router = emulator();
    let region = "ap-southeast-2";
    let expected = expected_names(region);

    for page_size in [1usize, SERVICES.len(), SERVICES.len() + 5] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut params = vec![("MaxResults".to_string(), page_size.to_string())];
            if let Some(token) = &token {
                params.push(("NextToken".to_string(), token.clone()));
            }
            let (status, body) = describe(&router, region, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 on page {}, got {status} {body}",
                pages + 1
            );
            let page = list_names(&case, &body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most {page_size}, got {page:?}"
            );
            seen.extend(page);
            pages += 1;
            token = element(&body, "nextToken");
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: the walk must terminate, saw {seen:?}"
            );
            if token.is_none() {
                break;
            }
        }
        seen.sort();
        assert_eq!(
            seen, expected,
            "case {case:?}: the walk must report every prefix list exactly once"
        );
        let full_pages = expected.len().div_ceil(page_size);
        assert_eq!(
            pages, full_pages,
            "case {case:?}: expected {full_pages} pages, took {pages}"
        );
    }
}

/// A `NextToken` this service did not mint is refused with `InvalidPaginationToken`,
/// rather than read as a position.
///
/// The rows differ in how the token is wrong: not hex at all, hex that decodes to
/// something else, a bare prefix list name, and an empty token.
#[tokio::test]
async fn a_forged_next_token_is_refused() {
    let router = emulator();
    let cases: [(&str, &str); 4] = [
        ("not hex", "not-a-token"),
        ("hex of other text", "636f6d2e616d617a6f6e617773"),
        ("a bare name", "com.amazonaws.us-east-1.s3"),
        ("empty", ""),
    ];

    for (case, token) in cases {
        let (status, body) = describe(
            &router,
            "us-east-1",
            &[("NextToken".to_string(), token.to_string())],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some("InvalidPaginationToken"),
            "case {case:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

/// A page size of none is not a page; every other size the model leaves unbounded above
/// is accepted, the largest page this service serves and anything past it included.
#[tokio::test]
async fn a_page_size_outside_the_served_range_is_refused() {
    let router = emulator();
    for (case, max_results) in [("zero", "0"), ("negative", "-1")] {
        let (status, body) = describe(
            &router,
            "us-east-1",
            &[("MaxResults".to_string(), max_results.to_string())],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some("InvalidParameterValue"),
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
    }

    for (case, max_results) in [
        ("the smallest page", "1"),
        ("the largest page", "1000"),
        ("one past the largest page", "1001"),
    ] {
        let (status, body) = describe(
            &router,
            "us-east-1",
            &[("MaxResults".to_string(), max_results.to_string())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        assert!(
            !list_names(case, &body).is_empty(),
            "case {case:?}: expected a page of prefix lists, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: DryRun
// ---------------------------------------------------------------------------

/// A `DryRun` request performs nothing and is answered 412 `DryRunOperation`, while
/// `DryRun=false` is not a dry run and is performed.
#[tokio::test]
async fn a_dry_run_is_refused_with_dry_run_operation() {
    let router = emulator();
    let region = "us-east-1";
    for case in ["DryRun alone", "DryRun beside a filter"] {
        let mut params = vec![("DryRun".to_string(), "true".to_string())];
        if case.contains("filter") {
            params.extend(filters(&[("prefix-list-name", &["com.amazonaws.us-east-1.s3"])]));
        }
        let (status, body) = describe(&router, region, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected 412, got {status} {body}"
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some("DryRunOperation"),
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        assert!(
            element(&body, "prefixListSet").is_none(),
            "case {case:?}: a dry run reports no prefix lists, got {body}"
        );
    }

    let (status, body) = describe(
        &router,
        region,
        &[("DryRun".to_string(), "false".to_string())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"DryRun=false\": expected 200, got {status} {body}"
    );
    assert_eq!(
        list_names("DryRun=false", &body),
        expected_names(region),
        "case \"DryRun=false\": expected every prefix list of the Region, got {body}"
    );
}

// ---------------------------------------------------------------------------
// Rule 6: the describe is a read
// ---------------------------------------------------------------------------

/// Describing prefix lists changes nothing a later caller sees: repeated calls —
/// including ones that select, filter, page, fail or dry-run in between — answer the
/// same lists.
#[tokio::test]
async fn describing_prefix_lists_does_not_change_what_a_later_caller_sees() {
    let router = emulator();
    let region = "us-east-1";

    let (_, first) = describe(&router, region, &[]).await;
    assert_eq!(
        list_names("first", &first),
        expected_names(region),
        "case \"first\": expected every prefix list of the Region, got {first}"
    );
    let known = prefix_lists("first", &first)[0].id.clone();

    // A selective read, a filtered read, a paged read, a refused read and a dry run, all
    // between the two full reads.
    let _ = describe(&router, region, &indexed("PrefixListId", &[known.as_str()])).await;
    let _ = describe(
        &router,
        region,
        &filters(&[("prefix-list-name", &["com.amazonaws.*"])]),
    )
    .await;
    let _ = describe(
        &router,
        region,
        &[("MaxResults".to_string(), "1".to_string())],
    )
    .await;
    let _ = describe(&router, region, &indexed("PrefixListId", &["pl-00000000"])).await;
    let _ = describe(
        &router,
        region,
        &[("DryRun".to_string(), "true".to_string())],
    )
    .await;
    // …and a read in another Region, which must not leak into this one.
    let _ = describe(&router, "eu-west-1", &[]).await;

    let (status, second) = describe(&router, region, &[]).await;
    assert_eq!(
        status, 200,
        "case \"second\": expected 200, got {status} {second}"
    );
    assert_eq!(
        prefix_lists("second", &second),
        prefix_lists("first", &first),
        "case \"second\": a read must not change what a later caller sees"
    );
}

/// A Region name of a shape none of the rows above used — a single component, with no
/// direction or number — still reports one prefix list per catalog service, named for
/// it: the name is built from the Region, not matched against a list of known Regions.
#[tokio::test]
async fn a_region_of_any_shape_reports_one_list_per_service() {
    let router = emulator();
    let (status, body) = describe(&router, "local", &[]).await;
    assert_eq!(status, 200, "case \"local\": expected 200, got {status} {body}");
    assert_eq!(
        list_names("local", &body),
        expected_names("local"),
        "case \"local\": expected one prefix list per service, got {body}"
    );
}


