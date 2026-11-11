// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListTrafficPolicyVersions`.
//!
//! The rule under test: the operation REPORTS every version the traffic policy
//! its `Id` names holds at the moment of the call, read from the service's
//! state in numerical order by version number, as a page that starts at the
//! requested `TrafficPolicyVersionMarker`, carries at most `MaxItems` versions,
//! echoes that `maxitems` back and carries in `TrafficPolicyVersionMarker` the
//! version number the next page starts at — so the pages concatenate to the
//! policy's whole version set exactly once, each listed version reports the
//! same `TrafficPolicy` `GetTrafficPolicy` reports for it, a policy the account
//! does not hold is the modelled `NoSuchTrafficPolicy`, and nothing a caller
//! reads changes what a later caller sees.
//!
//! The tables therefore vary the axes the rule generalises over rather than
//! renaming a policy: the POPULATION of versions (one, then one more at a
//! time), the record type and document form each version carries (so a listing
//! that reported one version's document for another could not pass), whether a
//! version has a comment and how long it is, the PAGE SIZE (every size from one
//! to one past the population, plus the page bound and one past it) and the
//! MARKER (every version number in the listing, one past the highest, the
//! published range bounds, and values that are no version number at all).
//!
//! Every test drives the service through the real router, using the SAME
//! harness the other traffic-policy suites use, so what is asserted is the
//! HTTP/restXml behaviour a Route 53 client sees.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::ops::list_traffic_policy_versions::DEFAULT_PAGE_SIZE;
use service_com_amazonaws_route53_awsdnsv20130401::ops::types::R_R_TYPE_VALUES;
use service_com_amazonaws_route53_awsdnsv20130401::state::{
    FIRST_TRAFFIC_POLICY_VERSION, MAX_TRAFFIC_POLICY_VERSION,
};

use crate::hosted_zone_records::{call, router, Reply};
use crate::traffic_policies::{
    create_policy, document_with_start_endpoint, document_with_start_rule, get_policy,
    policy_count, policy_of, PolicyView,
};
use crate::traffic_policy_not_found_envelope::assert_no_such_traffic_policy;
use crate::traffic_policy_versions::create_version;

/// `TrafficPolicyComment` is `len 0..1024`
/// (smithy_query(query="shape",shape="TrafficPolicyComment")).
const COMMENT_MAX_LEN: usize = 1024;

/// Send one `ListTrafficPolicyVersions` request for the policy `id` names, with
/// `query` as its query string.
///
/// `pub(crate)` so the suite asserting a policy record outlives its versions
/// lists them through the SAME request rather than growing a second copy of it.
pub(crate) async fn list_versions(router: &MultiServiceRouter, id: &str, query: &str) -> Reply {
    call(
        router,
        "GET",
        &format!("/2013-04-01/trafficpolicies/{id}/versions{query}"),
        "",
    )
    .await
}

/// List with `query`, asserting the call succeeded.
async fn listing(router: &MultiServiceRouter, case: &str, id: &str, query: &str) -> Reply {
    let reply = list_versions(router, id, query).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from ListTrafficPolicyVersions, got {} with body {}",
        reply.status, reply.body
    );
    reply
}

/// One listed `TrafficPolicy`, by the members every one of them carries.
///
/// `Comment` is optional per version, so it is asserted on its own
/// ([`comments_of`]) and through [`get_policy`]: aligning an optional element by
/// index across a page would read one version's comment as another's.
#[derive(Debug, Clone, PartialEq, Eq)]
struct VersionView {
    id: String,
    version: String,
    name: String,
    record_type: String,
    document: String,
}

/// Every version a listing reports, in the order it reports them.
///
/// `Id`, `Version`, `Name`, `Type` and `Document` are all required members of
/// `TrafficPolicy`, so a reply that reports one version whole reports as many
/// of each as it has versions; anything else is a malformed listing rather than
/// a different order.
fn versions_of(reply: &Reply, case: &str) -> Vec<VersionView> {
    let ids = reply.all("TrafficPolicy/Id");
    let versions = reply.all("TrafficPolicy/Version");
    let names = reply.all("TrafficPolicy/Name");
    let types = reply.all("TrafficPolicy/Type");
    let documents = reply.all("TrafficPolicy/Document");
    let widths = (
        versions.len(),
        names.len(),
        types.len(),
        documents.len(),
    );
    assert_eq!(
        widths,
        (ids.len(), ids.len(), ids.len(), ids.len()),
        "case {case:?}: every listed version must be reported whole, got {widths:?} beside \
         {} id(s); body {}",
        ids.len(),
        reply.body
    );
    (0..ids.len())
        .map(|at| VersionView {
            id: ids[at].to_string(),
            version: versions[at].to_string(),
            name: names[at].to_string(),
            record_type: types[at].to_string(),
            document: documents[at].to_string(),
        })
        .collect()
}

/// The comments a listing reports, in document order.
fn comments_of(reply: &Reply) -> Vec<String> {
    reply
        .all("TrafficPolicy/Comment")
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// The `TrafficPolicyVersionMarker` of a page: `None` once the listing has
/// ended, where there is no version left to resume at.
fn next_marker(reply: &Reply) -> Option<String> {
    reply
        .first("TrafficPolicyVersionMarker")
        .filter(|marker| !marker.is_empty())
        .map(str::to_string)
}

/// Whether a page says more versions remain.
fn truncated(reply: &Reply) -> bool {
    reply.first("IsTruncated") == Some("true")
}

/// One row of the version table: the version to create, and the `Type` the
/// document it carries names.
struct VersionRow {
    case: &'static str,
    document: String,
    comment: Option<String>,
    expect_type: &'static str,
}

/// The versions created AFTER the policy's first one. Each row differs in the
/// record type its document names, the document's entry-point form, whether it
/// carries a comment and how long that comment is.
fn later_versions() -> Vec<VersionRow> {
    vec![
        VersionRow {
            case: "a version keeping the first version's record type",
            document: document_with_start_endpoint("A", "192.0.2.2"),
            comment: Some("one endpoint, moved".to_string()),
            expect_type: "A",
        },
        VersionRow {
            case: "a version of another record type, with no comment",
            document: document_with_start_endpoint("AAAA", "2001:db8::2"),
            comment: None,
            expect_type: "AAAA",
        },
        VersionRow {
            case: "a version whose entry point is a rule over two endpoints",
            document: document_with_start_rule("CNAME", "a.example.com", "b.example.com"),
            comment: Some("primary with a backup".to_string()),
            expect_type: "CNAME",
        },
        VersionRow {
            case: "a version whose document carries XML special characters",
            document: document_with_start_endpoint("TXT", "v=spf1 <include:example.com> ~all"),
            comment: Some("a&b<c>d\"e'f".to_string()),
            expect_type: "TXT",
        },
        VersionRow {
            case: "a version with a comment at the 1024-character maximum",
            document: document_with_start_rule(
                "SRV",
                "1 1 80 a.example.com",
                "1 1 80 b.example.com",
            ),
            comment: Some("c".repeat(COMMENT_MAX_LEN)),
            expect_type: "SRV",
        },
        VersionRow {
            case: "a version with an empty comment, which the 0 minimum allows",
            document: document_with_start_endpoint("MX", "10 mail.example.com"),
            comment: Some(String::new()),
            expect_type: "MX",
        },
    ]
}

/// What a listing must report for one version: the required members, and the
/// comment it was created with.
#[derive(Debug, Clone)]
struct Expected {
    view: VersionView,
    comment: Option<String>,
}

/// The comments of `expected` that a listing reports at all: an absent comment
/// has no element, and an empty one carries no text.
fn expected_comments(expected: &[Expected]) -> Vec<String> {
    expected
        .iter()
        .filter_map(|version| version.comment.clone())
        .filter(|comment| !comment.is_empty())
        .collect()
}

/// The policy's first version, created with the policy itself.
async fn first_version(router: &MultiServiceRouter, name: &str) -> Expected {
    let document = document_with_start_endpoint("A", "192.0.2.1");
    let comment = "the first version";
    let created = create_policy(router, name, &document, Some(comment)).await;
    assert_eq!(
        created.status, 201,
        "expected 201 from CreateTrafficPolicy for {name:?}, got {} with body {}",
        created.status, created.body
    );
    let policy = policy_of(&created, name);
    Expected {
        view: VersionView {
            id: policy.id,
            version: FIRST_TRAFFIC_POLICY_VERSION.to_string(),
            name: policy.name,
            record_type: policy.record_type,
            document: policy.document,
        },
        comment: Some(comment.to_string()),
    }
}

/// Add the version `row` describes to the policy `id` names, and return what
/// every listing of that policy must then report for it.
async fn add_version(
    router: &MultiServiceRouter,
    held: &Expected,
    row: &VersionRow,
    version: i32,
) -> Expected {
    let created = create_version(router, &held.view.id, &row.document, row.comment.as_deref()).await;
    assert_eq!(
        created.status, 201,
        "case {:?}: expected 201 from CreateTrafficPolicyVersion, got {} with body {}",
        row.case, created.status, created.body
    );
    let answered = policy_of(&created, row.case);
    assert_eq!(
        answered.version,
        version.to_string(),
        "case {:?}: expected Version {version}, got {:?}",
        row.case,
        answered.version
    );
    Expected {
        view: VersionView {
            id: held.view.id.clone(),
            version: version.to_string(),
            name: held.view.name.clone(),
            record_type: row.expect_type.to_string(),
            document: row.document.clone(),
        },
        comment: row.comment.clone(),
    }
}

/// Create a policy with every version of [`later_versions`] added to it, and
/// return what a listing of it must report, in numerical version order.
async fn populate(router: &MultiServiceRouter, name: &str) -> Vec<Expected> {
    let first = first_version(router, name).await;
    let mut expected = vec![first.clone()];
    for (index, row) in later_versions().iter().enumerate() {
        let version = FIRST_TRAFFIC_POLICY_VERSION + 1 + i32::try_from(index).unwrap_or(0);
        expected.push(add_version(router, &first, row, version).await);
    }
    expected
}

#[tokio::test]
async fn the_listing_reports_every_version_the_policy_holds() {
    // The population is the varying axis: one version, then one more after
    // every row — so the listing is asserted against a set that grows by a
    // version of a different record type, document form and comment each time,
    // in the numerical order the versions are numbered in.
    let router = router();

    // Every record type the table spans is one the model's own `RRType` enum
    // names, so no row asserts a type a traffic policy version could not carry.
    for row in &later_versions() {
        assert!(
            R_R_TYPE_VALUES.contains(&row.expect_type),
            "case {:?}: {} is not a modelled RRType",
            row.case,
            row.expect_type
        );
    }

    let first = first_version(&router, "version-listing").await;
    let mut expected = vec![first.clone()];

    for (index, row) in later_versions().iter().enumerate() {
        let case = format!(
            "{} version(s) held, before creating {}",
            expected.len(),
            row.case
        );
        let reply = listing(&router, &case, &first.view.id, "").await;
        let listed = versions_of(&reply, &case);
        let wanted: Vec<VersionView> = expected.iter().map(|held| held.view.clone()).collect();
        assert_eq!(
            listed, wanted,
            "case {case:?}: the listing must report every version the policy holds, in \
             numerical order by version number; body {}",
            reply.body
        );
        assert_eq!(
            comments_of(&reply),
            expected_comments(&expected),
            "case {case:?}: each listed version must carry the comment it was created with; \
             body {}",
            reply.body
        );
        assert!(
            !truncated(&reply),
            "case {case:?}: a listing that fits one page must not be truncated; body {}",
            reply.body
        );
        assert_eq!(
            next_marker(&reply),
            None,
            "case {case:?}: an untruncated listing names no version to resume at; body {}",
            reply.body
        );

        // Every listed version is cross-checked against what GetTrafficPolicy
        // reports for the same (Id, Version), so the listing is proved against
        // another reader of the same state rather than against itself.
        for held in &expected {
            let read = get_policy(&router, &held.view.id, &held.view.version).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: version {} must be readable, got {} with body {}",
                held.view.version, read.status, read.body
            );
            let got = policy_of(&read, &case);
            let wanted = PolicyView {
                id: held.view.id.clone(),
                version: held.view.version.clone(),
                name: held.view.name.clone(),
                record_type: held.view.record_type.clone(),
                document: held.view.document.clone(),
                comment: held.comment.clone().filter(|text| !text.is_empty()),
            };
            assert_eq!(
                got, wanted,
                "case {case:?}: the listed version must be what GetTrafficPolicy reports, \
                 got {got:?}; body {}",
                read.body
            );
        }

        let version = FIRST_TRAFFIC_POLICY_VERSION + 1 + i32::try_from(index).unwrap_or(0);
        expected.push(add_version(&router, &first, row, version).await);
    }

    // The last row's version is listed too, so the final set is asserted as
    // well as every set on the way to it.
    let case = "every version created";
    let reply = listing(&router, case, &first.view.id, "").await;
    let listed = versions_of(&reply, case);
    let wanted: Vec<VersionView> = expected.iter().map(|held| held.view.clone()).collect();
    assert_eq!(
        listed, wanted,
        "case {case:?}: the listing must report the whole version set; body {}",
        reply.body
    );
}

#[tokio::test]
async fn version_paging_covers_the_listing_exactly_once() {
    // The rule as an invariant: a page holds at most MaxItems versions and
    // TrafficPolicyVersionMarker is the version number the next page starts at,
    // so the pages concatenate to the full listing for EVERY page size. The
    // page size is the varying axis.
    let router = router();
    let created = populate(&router, "version-paging").await;
    let id = created[0].view.id.clone();
    let all: Vec<VersionView> = created.iter().map(|held| held.view.clone()).collect();

    let unpaged = listing(&router, "the whole listing", &id, "").await;
    assert_eq!(
        versions_of(&unpaged, "the whole listing"),
        all,
        "the whole listing must report every created version; body {}",
        unpaged.body
    );

    for page_size in 1..=all.len() + 1 {
        let mut walked: Vec<VersionView> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            let case = format!("maxitems={page_size}, marker={marker:?}");
            let query = match &marker {
                Some(resume) => format!("?maxitems={page_size}&trafficpolicyversion={resume}"),
                None => format!("?maxitems={page_size}"),
            };
            let page = listing(&router, &case, &id, &query).await;
            assert_eq!(
                page.first("MaxItems"),
                Some(page_size.to_string()).as_deref(),
                "case {case:?}: the response must echo the request's maxitems; body {}",
                page.body
            );
            let listed = versions_of(&page, &case);
            assert!(
                listed.len() <= page_size,
                "case {case:?}: a page held {} version(s); body {}",
                listed.len(),
                page.body
            );
            walked.extend(listed);
            pages += 1;
            assert!(
                pages <= all.len() + 2,
                "case {case:?}: paging did not terminate"
            );
            if !truncated(&page) {
                assert_eq!(
                    next_marker(&page),
                    None,
                    "case {case:?}: a final page names no version to resume at; body {}",
                    page.body
                );
                break;
            }
            let resume = next_marker(&page).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a truncated page must name the version the next page \
                     starts at; body {}",
                    page.body
                )
            });
            assert_eq!(
                Some(resume.as_str()),
                all.get(walked.len()).map(|held| held.version.as_str()),
                "case {case:?}: the marker must be the version number the next page starts \
                 at; body {}",
                page.body
            );
            marker = Some(resume);
        }
        assert_eq!(
            walked, all,
            "case maxitems={page_size}: paging must cover the listing exactly once"
        );
    }
}

#[tokio::test]
async fn the_listing_echoes_the_page_size_it_was_asked_for() {
    // `MaxItems` in the response is "the value that you specified for the
    // maxitems parameter", so it is echoed whatever the caller sent — while the
    // page itself never exceeds the per-policy version ceiling, which no
    // listing can be longer than. The rows are sizes below the population, that
    // bound, and one past it.
    let router = router();
    let created = populate(&router, "version-page-size").await;
    let id = created[0].view.id.clone();
    let page_bound = MAX_TRAFFIC_POLICY_VERSION as usize;

    for requested in [1, 2, page_bound, page_bound + 1] {
        let case = format!("maxitems={requested}");
        let page = listing(&router, &case, &id, &format!("?maxitems={requested}")).await;
        let echoed = page.first("MaxItems").unwrap_or("");
        assert_eq!(
            echoed,
            requested.to_string(),
            "case {case:?}: expected the requested maxitems echoed, got {echoed:?}; body {}",
            page.body
        );
        let listed = versions_of(&page, &case);
        let wanted = requested.min(page_bound).min(created.len());
        assert_eq!(
            listed.len(),
            wanted,
            "case {case:?}: expected {wanted} version(s), got {}; body {}",
            listed.len(),
            page.body
        );
        assert_eq!(
            truncated(&page),
            listed.len() < created.len(),
            "case {case:?}: IsTruncated must say whether more versions remain; body {}",
            page.body
        );
    }

    // With no maxitems at all the whole version set comes back in one page,
    // and the required `MaxItems` reports the default page size the listing
    // applied instead of a value the request never sent.
    let case = "no maxitems";
    let whole = listing(&router, case, &id, "").await;
    assert_eq!(
        versions_of(&whole, case).len(),
        created.len(),
        "case {case:?}: a request without maxitems must list every version; body {}",
        whole.body
    );
    let default_page_size = DEFAULT_PAGE_SIZE.to_string();
    assert_eq!(
        whole.first("MaxItems"),
        Some(default_page_size.as_str()),
        "case {case:?}: expected the default page size {default_page_size:?} in MaxItems, got \
         {:?}; body {}",
        whole.first("MaxItems"),
        whole.body
    );
    assert!(
        created.len() < DEFAULT_PAGE_SIZE as usize,
        "case {case:?}: the row assumes a population inside one default page, got {}",
        created.len()
    );
}

#[tokio::test]
async fn a_marker_resumes_the_listing_at_the_version_it_names() {
    // The marker is the version number "of the first version that Route 53
    // will return", and the listing runs in numerical order by that number, so
    // a listing resumed at any version reports exactly the tail from it. Every
    // version number is a row, plus one past the highest and the top of the
    // published range, where nothing is left to report.
    let router = router();
    let created = populate(&router, "version-marker").await;
    let id = created[0].view.id.clone();
    let all: Vec<VersionView> = created.iter().map(|held| held.view.clone()).collect();

    for (at, held) in all.iter().enumerate() {
        let case = format!("trafficpolicyversion={}", held.version);
        let page = listing(
            &router,
            &case,
            &id,
            &format!("?trafficpolicyversion={}", held.version),
        )
        .await;
        assert_eq!(
            versions_of(&page, &case),
            all[at..].to_vec(),
            "case {case:?}: the listing must resume at the version the marker names; body {}",
            page.body
        );
    }

    // A version number past the highest the policy holds leaves nothing to
    // report: an empty, untruncated page rather than a page from elsewhere.
    let past = all.len() + 1;
    for marker in [past.to_string(), MAX_TRAFFIC_POLICY_VERSION.to_string()] {
        let case = format!("trafficpolicyversion={marker}");
        let page = listing(
            &router,
            &case,
            &id,
            &format!("?trafficpolicyversion={marker}"),
        )
        .await;
        assert_eq!(
            versions_of(&page, &case),
            Vec::new(),
            "case {case:?}: no version is at or past that number; body {}",
            page.body
        );
        assert!(
            !truncated(&page),
            "case {case:?}: an empty page must not be truncated; body {}",
            page.body
        );
    }
}

#[tokio::test]
async fn a_marker_that_is_no_version_number_is_refused() {
    // The marker positions the listing at a version, so a value that is no
    // version number of the published range (`TrafficPolicyVersion` is
    // `range 1..1000`) positions nothing: the request is refused rather than
    // answered from somewhere else. The rows are a non-numeric value, the
    // number below the range's minimum, a negative one, a numeric literal that
    // is not an integer, and one past the range's maximum.
    let router = router();
    let created = populate(&router, "version-marker-refusals").await;
    let id = created[0].view.id.clone();

    for marker in ["abc", "0", "-1", "1e3", "1001"] {
        let page = list_versions(&router, &id, &format!("?trafficpolicyversion={marker}")).await;
        assert_eq!(
            page.status, 400,
            "case trafficpolicyversion={marker:?}: expected 400, got {} with body {}",
            page.status, page.body
        );
        assert_eq!(
            page.error_code(),
            "InvalidInput",
            "case trafficpolicyversion={marker:?}: expected the modelled InvalidInput, got {:?}; \
             body {}",
            page.error_code(),
            page.body
        );
        assert!(
            page.first("Error/Message").unwrap_or("").contains(marker),
            "case trafficpolicyversion={marker:?}: the refusal must name the marker it could \
             not place; body {}",
            page.body
        );
    }

    // An empty marker is what a first request sends, so it lists from the
    // policy's first version rather than being refused.
    let case = "an empty marker";
    let page = listing(&router, case, &id, "?trafficpolicyversion=").await;
    assert_eq!(
        versions_of(&page, case).len(),
        created.len(),
        "case {case:?}: an empty marker must list from the first version; body {}",
        page.body
    );
}

#[tokio::test]
async fn versions_are_listed_per_policy_and_a_missing_policy_is_refused() {
    // The listing is of ONE policy's versions, so two policies list their own
    // versions independently, and an id the account holds no policy under has
    // no versions to list at all.
    let router = router();
    let one = populate(&router, "listed-policy-one").await;
    let two = first_version(&router, "listed-policy-two").await;

    let case = "the policy with several versions";
    let listed_one = versions_of(&listing(&router, case, &one[0].view.id, "").await, case);
    assert_eq!(
        listed_one,
        one.iter().map(|held| held.view.clone()).collect::<Vec<_>>(),
        "case {case:?}: a policy's listing must report its own versions"
    );

    let case = "the policy with one version";
    let reply = listing(&router, case, &two.view.id, "").await;
    let listed_two = versions_of(&reply, case);
    assert_eq!(
        listed_two,
        vec![two.view.clone()],
        "case {case:?}: a policy holding one version lists exactly it; body {}",
        reply.body
    );

    // Several forms of an id no policy carries, including one that differs from
    // a held id only in its last character.
    let mut near_miss = one[0].view.id.clone();
    near_miss.pop();
    near_miss.push(if one[0].view.id.ends_with('0') { '1' } else { '0' });
    let rows = [
        (
            "an id in the minted form that no policy carries",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        ("an id one character off a held one", near_miss),
        ("an id that is not in the minted form at all", "x".to_string()),
    ];

    for (case, unknown) in &rows {
        let reply = list_versions(&router, unknown, "").await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404 for an id that names no policy, got {} with \
             body {}",
            reply.status, reply.body
        );
        // Route 53 reports a traffic policy it does not hold by its code alone,
        // so every id above is refused with the same body: `Type` and `Code`
        // and no `Message` element.
        assert_no_such_traffic_policy(&reply, case);
    }
}

#[tokio::test]
async fn listing_the_versions_leaves_the_policy_exactly_as_it_was() {
    // The rule is a report: repeating it answers the same listing, creates and
    // consumes nothing, and the version set it reports still moves with a later
    // create — which the numerical order puts at the END of the listing.
    let router = router();
    let created = populate(&router, "version-listing-is-a-read").await;
    let id = created[0].view.id.clone();
    let all: Vec<VersionView> = created.iter().map(|held| held.view.clone()).collect();
    let policies_before = policy_count(&router).await;

    for attempt in 1..=3 {
        let case = format!("listing {attempt}");
        let reply = listing(&router, &case, &id, "").await;
        assert_eq!(
            versions_of(&reply, &case),
            all,
            "case {case:?}: a read must report what the first read did"
        );
        let counted = policy_count(&router).await;
        assert_eq!(
            counted, policies_before,
            "case {case:?}: listing must not change how many policies the account holds"
        );
    }

    // No read consumed a version number: the next create is still the version
    // one past the highest the policy held before the reads.
    let next = FIRST_TRAFFIC_POLICY_VERSION + i32::try_from(all.len()).unwrap_or(0);
    let added = add_version(
        &router,
        &created[0],
        &VersionRow {
            case: "a version created after the listing",
            document: document_with_start_endpoint("A", "192.0.2.9"),
            comment: None,
            expect_type: "A",
        },
        next,
    )
    .await;

    let case = "after a later create";
    let reply = listing(&router, case, &id, "").await;
    let mut expected = all.clone();
    expected.push(added.view);
    assert_eq!(
        versions_of(&reply, case),
        expected,
        "case {case:?}: the listing must follow the state, with the newest version last; \
         body {}",
        reply.body
    );
}
