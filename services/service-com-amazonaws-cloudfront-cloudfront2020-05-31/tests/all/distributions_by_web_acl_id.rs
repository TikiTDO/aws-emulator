// SPDX-License-Identifier: CC0-1.0

//! `ListDistributionsByWebACLId`, end to end over HTTP.
//!
//! The rule under test: a distribution is listed under a web ACL identifier exactly when
//! its STORED `DistributionConfig$WebACLId` equals that identifier — however the
//! association got there, the configuration a create submitted or a later association
//! call — and the sentinel `null` lists the distributions associated with no web ACL at
//! all ("if you specify "null" for the ID, the request returns a list of the distributions
//! that aren't associated with a web ACL"). A web ACL lives in WAF, which this service
//! cannot resolve, so the identifier is COMPARED and never validated: one no distribution
//! carries lists nothing at HTTP 200, whatever form it takes. The page contract is the one
//! every CloudFront distribution listing states: `MaxItems` is the page size and is
//! echoed, `Marker` is echoed and resumes after the id the previous page ended at,
//! `NextMarker` and `IsTruncated` arrive exactly when matching distributions remain, and
//! `Quantity` states the length of `Items`.
//!
//! The rows below span what that rule generalises over: WHICH form the identifier takes
//! (the `wafv2` ARN in two scopes and a second partition, a WAF Classic ARN, a bare WAF
//! Classic id), HOW the association got onto the distribution (named in the create's
//! configuration, set by `AssociateDistributionWebACL`, moved from one web ACL to
//! another), how many distributions share one web ACL (so a page can cut inside a matching
//! set), distributions associated with no web ACL at all, and a well-formed identifier no
//! distribution carries.
//!
//! The identifier catalog is [`crate::web_acl_association::WEB_ACLS`], whose source is the
//! model's own documentation of `DistributionConfig$WebACLId`; the wire harness (the
//! router, the request helpers and the element scanner) is the one
//! [`crate::distribution_lifecycle`] builds.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    MAX_ITEMS, MAX_ITEMS_INVALID_MESSAGE, WEB_ACL_ID_NONE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, create_distribution, emulator, get_distribution, is_distribution_id, list_distributions,
    Answer, Node, API_VERSION,
};
use crate::web_acl_association::{associate_web_acl, WEB_ACLS};

/// One distribution under test: the web ACL its create names in its configuration, and the
/// web ACL a later `AssociateDistributionWebACL` call puts on it. Each index is into
/// [`WEB_ACLS`]; `None` is "no web ACL".
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    in_config: Option<usize>,
    associated: Option<usize>,
}

impl Row {
    /// The web ACL this distribution ends up associated with: an association call writes the
    /// same one member a create's configuration does, so the later write is what the store
    /// holds. Computed from the row rather than read back from the handler.
    fn web_acl(&self) -> Option<usize> {
        self.associated.or(self.in_config)
    }
}

/// The index in [`WEB_ACLS`] no row below ever associates, so a well-formed identifier
/// nothing carries is covered.
const UNUSED_WEB_ACL: usize = 4;

const ROWS: &[Row] = &[
    Row {
        case: "the create's configuration names the wafv2 global web ACL",
        in_config: Some(0),
        associated: None,
    },
    Row {
        case: "a second distribution created under the same wafv2 global web ACL",
        in_config: Some(0),
        associated: None,
    },
    Row {
        case: "a third distribution created under the same wafv2 global web ACL",
        in_config: Some(0),
        associated: None,
    },
    Row {
        case: "the create's configuration names the wafv2 global web ACL of another account",
        in_config: Some(1),
        associated: None,
    },
    Row {
        case: "the create's configuration names a web ACL in another partition",
        in_config: Some(2),
        associated: None,
    },
    Row {
        case: "the create's configuration names a WAF Classic ARN",
        in_config: Some(3),
        associated: None,
    },
    Row {
        case: "created with no web ACL and left that way",
        in_config: None,
        associated: None,
    },
    Row {
        case: "a second distribution created with no web ACL",
        in_config: None,
        associated: None,
    },
    Row {
        case: "created with no web ACL, then associated with another account's global one",
        in_config: None,
        associated: Some(1),
    },
    Row {
        case: "created under the wafv2 global web ACL, then moved to a WAF Classic ARN",
        in_config: Some(0),
        associated: Some(3),
    },
];

/// The identifier of the web ACL at `index`.
fn web_acl_id(index: usize) -> &'static str {
    WEB_ACLS[index].1
}

/// `WebACLId` travels as ONE `@httpLabel` path segment, so the separators inside an ARN are
/// percent-encoded exactly as an SDK client encodes them.
///
/// `pub(crate)` so a sibling suite whose own identifier travels as one path label (a trust
/// store addressed by its ARN, say) encodes it through this one helper rather than carrying a
/// second copy of the mapping.
pub(crate) fn encode_label(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '/' => "%2F".to_string(),
            ':' => "%3A".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// `GET /2020-05-31/distributionsByWebACLId/{WebACLId}` plus the query the caller pages
/// with — the route the model binds this operation to.
async fn list_by_web_acl(router: &MultiServiceRouter, web_acl_id: &str, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!(
            "/{API_VERSION}/distributionsByWebACLId/{}{query}",
            encode_label(web_acl_id)
        ),
        None,
        None,
    )
    .await
}

/// The distribution ids of a `DistributionList` answer, in document order.
fn listed_ids(list: &Node) -> Vec<String> {
    list.child("Items")
        .map(|items| {
            items
                .children_named("DistributionSummary")
                .into_iter()
                .map(|summary| summary.text_at("Id").unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The `DistributionConfig` a row submits: one origin, one default cache behaviour and the
/// web ACL the row's create names, if any.
fn config_xml(caller_reference: &str, row: &Row) -> String {
    let web_acl = match row.in_config {
        Some(index) => format!("<WebACLId>{}</WebACLId>", web_acl_id(index)),
        None => String::new(),
    };
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items>\
         <Origin><Id>primary</Id><DomainName>a.example.com</DomainName></Origin>\
         </Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy></DefaultCacheBehavior>\
         <Comment>{caller_reference}</Comment>\
         <Enabled>true</Enabled>{web_acl}\
         </DistributionConfig>"
    )
}

/// Create every row, applying the association call the row asks for, and report the
/// `(row, id)` pairs.
async fn seed(router: &MultiServiceRouter) -> Vec<(&'static Row, String)> {
    let mut created = Vec::new();
    for (position, row) in ROWS.iter().enumerate() {
        let answer = create_distribution(router, &config_xml(&format!("ref-{position}"), row)).await;
        assert_eq!(
            answer.status, 201,
            "case {}: expected HTTP 201 from CreateDistribution, got {} with {}",
            row.case, answer.status, answer.raw
        );
        let id = answer.root().text_at("Id").unwrap_or_default().to_string();
        assert!(
            is_distribution_id(&id),
            "case {}: expected a minted distribution id, got {id:?}",
            row.case
        );
        if let Some(index) = row.associated {
            let associated = associate_web_acl(router, &id, web_acl_id(index), None).await;
            assert_eq!(
                associated.status, 200,
                "case {}: expected HTTP 200 from AssociateDistributionWebACL, got {} with {}",
                row.case, associated.status, associated.raw
            );
        }
        created.push((row, id));
    }
    created
}

/// The ids this suite expects under the web ACL at `index`, in id order.
fn expected_ids(created: &[(&'static Row, String)], index: Option<usize>) -> Vec<String> {
    let mut ids: Vec<String> = created
        .iter()
        .filter(|(row, _)| row.web_acl() == index)
        .map(|(_, id)| id.clone())
        .collect();
    ids.sort();
    ids
}

/// Walk every page of one web ACL, returning the ids in the order they were served.
async fn walk(router: &MultiServiceRouter, web_acl: &str, page_size: Option<i32>) -> Vec<String> {
    let label = match page_size {
        None => format!("{web_acl}/default"),
        Some(size) => format!("{web_acl}/MaxItems={size}"),
    };
    let mut collected: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    // At most one page per distribution plus one, so a listing that never stops truncating
    // fails here rather than looping forever.
    for page in 0..=ROWS.len() + 1 {
        let query = match (page_size, marker.as_deref()) {
            (None, None) => String::new(),
            (None, Some(value)) => format!("?Marker={value}"),
            (Some(size), None) => format!("?MaxItems={size}"),
            (Some(size), Some(value)) => format!("?MaxItems={size}&Marker={value}"),
        };
        let answer = list_by_web_acl(router, web_acl, &query).await;
        assert_eq!(
            answer.status, 200,
            "case {label} page {page}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        let list = answer.root();
        // "Marker: the value you provided for the Marker request parameter."
        assert_eq!(
            list.text_at("Marker"),
            Some(marker.as_deref().unwrap_or_default()),
            "case {label} page {page}: expected the Marker the request carried, got {:?}",
            list.text_at("Marker")
        );
        // "MaxItems: the value you provided for the MaxItems request parameter."
        assert_eq!(
            list.text_at("MaxItems"),
            Some(page_size.unwrap_or(MAX_ITEMS).to_string().as_str()),
            "case {label} page {page}: expected MaxItems to echo the page size, got {:?}",
            list.text_at("MaxItems")
        );
        let ids = listed_ids(list);
        assert_eq!(
            list.text_at("Quantity"),
            Some(ids.len().to_string().as_str()),
            "case {label} page {page}: expected Quantity to state the summaries served ({}), \
             got {:?}",
            ids.len(),
            list.text_at("Quantity")
        );
        if let Some(size) = page_size {
            assert!(
                i32::try_from(ids.len()).unwrap_or(i32::MAX) <= size.min(MAX_ITEMS),
                "case {label} page {page}: expected at most {} summaries, got {}",
                size.min(MAX_ITEMS),
                ids.len()
            );
        }
        collected.extend(ids.iter().cloned());

        let next_marker = list.text_at("NextMarker").map(str::to_string);
        let is_truncated = list.text_at("IsTruncated").unwrap_or_default().to_string();
        match next_marker {
            Some(next) => {
                assert_eq!(
                    is_truncated, "true",
                    "case {label} page {page}: a NextMarker means IsTruncated true, got \
                     {is_truncated:?}"
                );
                assert_eq!(
                    ids.last().map(String::as_str),
                    Some(next.as_str()),
                    "case {label} page {page}: expected NextMarker to be the id this page ended \
                     at, got {next:?}"
                );
                marker = Some(next);
            }
            None => {
                assert_eq!(
                    is_truncated, "false",
                    "case {label} page {page}: no NextMarker means IsTruncated false, got \
                     {is_truncated:?}"
                );
                return collected;
            }
        }
    }
    panic!("case {label}: the listing never stopped truncating");
}

/// Every distribution id `ListDistributions` reports, in document order.
async fn listed_ids_of_distributions(router: &MultiServiceRouter) -> Vec<String> {
    let answer = list_distributions(router, "").await;
    assert_eq!(
        answer.status, 200,
        "case the unfiltered listing: expected HTTP 200, got {} with {}",
        answer.status, answer.raw
    );
    listed_ids(answer.root())
}

/// What a read of the distribution reports under `DistributionConfig/WebACLId`.
async fn stored_web_acl(router: &MultiServiceRouter, id: &str, case: &str) -> String {
    let fetched = get_distribution(router, id).await;
    assert_eq!(
        fetched.status, 200,
        "case {case}: expected HTTP 200 from GetDistribution for {id}, got {} with {}",
        fetched.status, fetched.raw
    );
    fetched
        .root()
        .text_at("DistributionConfig/WebACLId")
        .unwrap_or_default()
        .to_string()
}

/// Each web ACL lists exactly the distributions whose stored configuration names it, once
/// each and in id order — and every id it lists is a distribution the store holds whose
/// configuration really carries that identifier.
#[tokio::test]
async fn each_web_acl_lists_the_distributions_associated_with_it() {
    let router = emulator();
    let created = seed(&router).await;
    let all_ids = listed_ids_of_distributions(&router).await;

    for (index, (case, web_acl)) in WEB_ACLS.iter().enumerate() {
        let expected = expected_ids(&created, Some(index));
        let answer = list_by_web_acl(&router, web_acl, "").await;
        assert_eq!(
            answer.status, 200,
            "case web ACL {case}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        let ids = listed_ids(answer.root());
        assert_eq!(
            ids, expected,
            "case web ACL {case}: expected the distributions associated with it, in id order, got \
             {ids:?}"
        );

        for id in &ids {
            assert!(
                all_ids.contains(id),
                "case web ACL {case}: expected {id} to be a distribution ListDistributions \
                 reports, got {all_ids:?}"
            );
            let stored = stored_web_acl(&router, id, case).await;
            assert_eq!(
                stored, *web_acl,
                "case web ACL {case}: expected the stored configuration of {id} to name it, got \
                 {stored:?}"
            );
        }
    }
}

/// The sentinel `null` lists exactly the distributions associated with no web ACL, and
/// those distributions store no identifier at all.
#[tokio::test]
async fn the_null_sentinel_lists_the_unassociated_distributions() {
    let router = emulator();
    let created = seed(&router).await;

    let expected = expected_ids(&created, None);
    assert!(
        !expected.is_empty(),
        "case the null sentinel: the table must cover a distribution with no web ACL, got \
         {expected:?}"
    );
    let answer = list_by_web_acl(&router, WEB_ACL_ID_NONE, "").await;
    assert_eq!(
        answer.status, 200,
        "case the null sentinel: expected HTTP 200, got {} with {}",
        answer.status, answer.raw
    );
    let ids = listed_ids(answer.root());
    assert_eq!(
        ids, expected,
        "case the null sentinel: expected the distributions associated with no web ACL, in id \
         order, got {ids:?}"
    );
    for id in &ids {
        let stored = stored_web_acl(&router, id, "the null sentinel").await;
        assert!(
            stored.is_empty(),
            "case the null sentinel: expected {id} to store no web ACL, got {stored:?}"
        );
    }
}

/// Walking the pages at ANY admissible page size enumerates one web ACL's distributions
/// exactly once, in id order — the sentinel included.
#[tokio::test]
async fn walking_the_pages_enumerates_each_web_acl_exactly_once() {
    let router = emulator();
    let created = seed(&router).await;

    let mut selectors: Vec<(Option<usize>, String)> = (0..WEB_ACLS.len())
        .map(|index| (Some(index), web_acl_id(index).to_string()))
        .collect();
    selectors.push((None, WEB_ACL_ID_NONE.to_string()));

    for (index, selector) in selectors {
        let expected = expected_ids(&created, index);
        for page_size in [None, Some(1), Some(2), Some(3), Some(MAX_ITEMS), Some(i32::MAX)] {
            let collected = walk(&router, &selector, page_size).await;
            assert_eq!(
                collected, expected,
                "case {selector} page size {page_size:?}: expected every distribution associated \
                 with it exactly once in id order, got {collected:?}"
            );
        }
    }
}

/// An identifier no distribution carries lists NOTHING at HTTP 200 — never the whole store,
/// and never a refusal: a web ACL is a WAF resource this service cannot resolve, so a
/// well-formed identifier nothing carries and a malformed one answer the same way.
#[tokio::test]
async fn an_identifier_nothing_carries_lists_nothing() {
    let router = emulator();
    let created = seed(&router).await;
    assert!(
        expected_ids(&created, Some(UNUSED_WEB_ACL)).is_empty(),
        "the table must leave one catalogued web ACL unassociated"
    );

    let known = web_acl_id(0);
    let absent: Vec<(&str, String)> = vec![
        (
            "a catalogued identifier no distribution carries",
            web_acl_id(UNUSED_WEB_ACL).to_string(),
        ),
        ("a known identifier in upper case", known.to_uppercase()),
        (
            "a known identifier with its trailing id changed",
            known.replace("EXAMPLE11111", "EXAMPLE99999"),
        ),
        ("a token that is no ARN and no id", "not-a-web-acl".to_string()),
        ("an ARN whose segments are empty", "arn:aws:wafv2:::".to_string()),
        ("a bare word", "NULL".to_string()),
    ];

    for (case, candidate) in absent {
        let answer = list_by_web_acl(&router, &candidate, "").await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200 for {candidate:?}, got {} with {}",
            answer.status, answer.raw
        );
        let list = answer.root();
        assert!(
            listed_ids(list).is_empty(),
            "case {case}: expected no distribution summary for {candidate:?}, got {:?}",
            listed_ids(list)
        );
        assert_eq!(
            list.text_at("Quantity"),
            Some("0"),
            "case {case}: expected Quantity 0, got {:?}",
            list.text_at("Quantity")
        );
        // A `DistributionList` that lists nothing carries no `Items` element at all.
        assert!(
            list.child("Items").is_none(),
            "case {case}: expected no Items element, got {}",
            answer.raw
        );
        assert_eq!(
            list.text_at("IsTruncated"),
            Some("false"),
            "case {case}: expected IsTruncated false, got {:?}",
            list.text_at("IsTruncated")
        );
        assert!(
            list.text_at("NextMarker").is_none(),
            "case {case}: expected no NextMarker, got {:?}",
            list.text_at("NextMarker")
        );
    }
}

/// A page size that names no page, and a marker this service never issued, are refused as
/// invalid arguments — the same constraint the unfiltered listing states.
#[tokio::test]
async fn an_unusable_page_size_or_marker_is_refused() {
    let router = emulator();
    seed(&router).await;

    let refusals: &[(&str, &str, bool)] = &[
        ("?MaxItems=-7", "a negative page size", true),
        ("?MaxItems=not-a-number", "a page size that is no integer", true),
        ("?Marker=not-an-id", "a marker that is no distribution id", false),
    ];
    for selector in [web_acl_id(0), WEB_ACL_ID_NONE] {
        for (query, description, states_the_page_size) in refusals {
            let answer = list_by_web_acl(&router, selector, query).await;
            assert_eq!(
                answer.status, 400,
                "case {description} under {selector}: expected HTTP 400, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "InvalidArgument",
                "case {description} under {selector}: expected InvalidArgument, got {}",
                answer.raw
            );
            if *states_the_page_size {
                let message = answer.root().find_text("Message").unwrap_or_default();
                assert_eq!(
                    message, MAX_ITEMS_INVALID_MESSAGE,
                    "case {description} under {selector}: expected the page-size constraint, got \
                     {message:?}"
                );
            }
        }
    }
}

/// The listing REPORTS: asking twice answers the same thing, and the unfiltered listing is
/// untouched by it.
#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    let created = seed(&router).await;
    let before = listed_ids_of_distributions(&router).await;

    for (index, selector) in (0..WEB_ACLS.len())
        .map(|index| (Some(index), web_acl_id(index).to_string()))
        .chain(std::iter::once((None, WEB_ACL_ID_NONE.to_string())))
    {
        let expected = expected_ids(&created, index);
        for attempt in 0..2 {
            let ids = listed_ids(list_by_web_acl(&router, &selector, "").await.root());
            assert_eq!(
                ids, expected,
                "case {selector} attempt {attempt}: expected the same page every time, got {ids:?}"
            );
        }
    }

    let after = listed_ids_of_distributions(&router).await;
    assert_eq!(
        after, before,
        "case the unfiltered listing: expected it to be unchanged by the filtered reads, got \
         {after:?}"
    );
}
