// SPDX-License-Identifier: CC0-1.0

//! `ListDistributionTenants` with NO `AssociationFilter`, end to end over HTTP.
//!
//! The rule under test: `AssociationFilter` is an OPTIONAL input member of this paginated
//! listing (`smithy_query(query="operation",operation="ListDistributionTenants")` reports
//! every input member optional and the operation `@paginated` by `Marker`/`NextMarker`), so a
//! request carrying no filter element narrows by nothing and is served the account's whole
//! stored set — HTTP 200 with a `DistributionTenantList`, empty when the account holds no
//! tenant. The "exactly one association" rule is a rule about a filter element that IS
//! carried: it is refused only when the element names neither of its two members or both of
//! them.
//!
//! The rows differ along what that rule generalises over: whether the request carries a
//! filter element at all, which member it carries when it does, how many tenants the store
//! holds (none, one, several), and the page size the unfiltered walk pages at (each size from
//! the smallest to one past the stored set).
//!
//! The wire harness is [`crate::distribution_lifecycle`]'s, the request helpers are
//! [`crate::distribution_tenants_list`]'s and the fixture is created through
//! [`crate::distribution_tenant_lifecycle`]'s and [`crate::connection_groups`]'s helpers, so
//! every assertion travels the path a client takes.

use smithy_server_runtime::MultiServiceRouter;

use crate::connection_groups::create_connection_group;
use crate::distribution_lifecycle::{emulator, API_VERSION};
use crate::distribution_tenant_lifecycle::{
    create_distribution_tenant, escape, managed_certificate_request, multi_tenant_distribution,
};
use crate::distribution_tenants_list::{list_body, list_tenants, summaries, summary_ids};
use crate::response_headers_policy_lifecycle::{assert_absent, text};

// ---------------------------------------------------------------------------
// The fixture
// ---------------------------------------------------------------------------

/// What the fixture stored: two multi-tenant distributions, two connection groups, and the
/// tenants created across them.
#[derive(Debug, Clone)]
struct Fixture {
    /// The two multi-tenant distributions, in creation order.
    distributions: Vec<String>,
    /// The two connection groups, in creation order.
    groups: Vec<String>,
    /// One entry per created tenant: its id, its distribution and its connection group.
    tenants: Vec<Tenant>,
}

/// One created tenant, as the create persisted it.
#[derive(Debug, Clone)]
struct Tenant {
    id: String,
    distribution: String,
    group: String,
}

impl Fixture {
    /// The ids a listing under `distribution` and `group` must report, in the order this
    /// service reports them (tenant id order). `None` narrows by nothing, which is what an
    /// absent filter asks for.
    fn expected(&self, distribution: Option<&str>, group: Option<&str>) -> Vec<String> {
        let mut ids: Vec<String> = self
            .tenants
            .iter()
            .filter(|tenant| {
                distribution.is_none_or(|wanted| tenant.distribution == wanted)
                    && group.is_none_or(|wanted| tenant.group == wanted)
            })
            .map(|tenant| tenant.id.clone())
            .collect();
        ids.sort();
        ids
    }
}

/// How a row names the `AssociationFilter` it sends: `None` for a request carrying no filter
/// element at all, and otherwise the `DistributionId` and `ConnectionGroupId` the element
/// carries — the shape [`list_body`] takes.
type FilterOf<'a> = Option<(Option<&'a str>, Option<&'a str>)>;

/// Create one connection group named `name` over the wire and answer its minted id.
async fn connection_group(router: &MultiServiceRouter, name: &str) -> String {
    let body = format!(
        "<CreateConnectionGroupRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Name>{name}</Name><Enabled>true</Enabled></CreateConnectionGroupRequest>"
    );
    let answer = create_connection_group(router, &body).await;
    assert_eq!(
        answer.status, 201,
        "case {name}: expected HTTP 201 from CreateConnectionGroup, got {} with {}",
        answer.status, answer.raw
    );
    text(
        answer.root(),
        "Id",
        name,
        "the created connection group",
        &answer.raw,
    )
}

/// Create one tenant named `slug` under `distribution`, routed through `group`, and answer
/// what the create persisted.
async fn tenant(
    router: &MultiServiceRouter,
    slug: &str,
    distribution: &str,
    group: &str,
) -> Tenant {
    let body = format!(
        "<CreateDistributionTenantRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <DistributionId>{}</DistributionId><Name>{slug}</Name>\
         <Domains><member><Domain>{slug}.example.com</Domain></member></Domains>\
         <ConnectionGroupId>{}</ConnectionGroupId>\
         <Parameters><member><Name>tenantName</Name><Value>{slug}</Value></member></Parameters>\
         {}</CreateDistributionTenantRequest>",
        escape(distribution),
        escape(group),
        managed_certificate_request()
    );
    let answer = create_distribution_tenant(router, &body).await;
    assert_eq!(
        answer.status, 201,
        "case {slug}: expected HTTP 201 from CreateDistributionTenant, got {} with {}",
        answer.status, answer.raw
    );
    Tenant {
        id: text(answer.root(), "Id", slug, "the created tenant", &answer.raw),
        distribution: distribution.to_string(),
        group: group.to_string(),
    }
}

/// The two distributions and two connection groups every row below is served from, holding no
/// tenant yet.
async fn empty_fixture(router: &MultiServiceRouter) -> Fixture {
    Fixture {
        distributions: vec![
            multi_tenant_distribution(router, "ref-unfiltered-tenants-one").await,
            multi_tenant_distribution(router, "ref-unfiltered-tenants-two").await,
        ],
        groups: vec![
            connection_group(router, "unfiltered-tenants-group-one").await,
            connection_group(router, "unfiltered-tenants-group-two").await,
        ],
        tenants: Vec::new(),
    }
}

/// How the table spreads the tenants over the fixture's resources: the slug, which
/// distribution it is created under and which connection group it is routed through.
const SPREAD: [(&str, usize, usize); 4] = [
    ("unfiltered-a", 0, 0),
    ("unfiltered-b", 0, 1),
    ("unfiltered-c", 1, 0),
    ("unfiltered-d", 1, 1),
];

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

/// An unfiltered listing reports every tenant the account holds, at every stored-set size
/// from none upwards, and a filter element naming one member still narrows to that
/// resource's own tenants.
#[tokio::test]
async fn a_request_carrying_no_association_filter_lists_every_stored_tenant() {
    let router = emulator();
    let mut fixture = empty_fixture(&router).await;

    // Size zero: an account holding no tenant at all is served the empty page, not a
    // refusal — the listing's own output member, carrying nothing.
    let answer = list_tenants(&router, &list_body(None, None, None)).await;
    assert_eq!(
        answer.status, 200,
        "case no tenant stored: expected HTTP 200, got {} with {}",
        answer.status, answer.raw
    );
    assert!(
        summaries(&answer).is_empty(),
        "case no tenant stored: expected an empty DistributionTenantList, got {}",
        answer.raw
    );
    assert_absent(
        answer.root(),
        "NextMarker",
        "no tenant stored",
        "a listing with nothing left to resume from",
    );

    // One created tenant at a time: after each create the unfiltered listing reports every
    // tenant stored so far, and each single-member filter reports that resource's own.
    for (slug, distribution, group) in SPREAD {
        let distribution = fixture.distributions[distribution].clone();
        let group = fixture.groups[group].clone();
        let created = tenant(&router, slug, &distribution, &group).await;
        fixture.tenants.push(created);

        let rows: Vec<(String, FilterOf<'_>)> = vec![
            (
                format!("no filter element at all with {} stored", fixture.tenants.len()),
                None,
            ),
            (
                format!("the first distribution with {} stored", fixture.tenants.len()),
                Some((Some(&fixture.distributions[0]), None)),
            ),
            (
                format!("the second distribution with {} stored", fixture.tenants.len()),
                Some((Some(&fixture.distributions[1]), None)),
            ),
            (
                format!("the first connection group with {} stored", fixture.tenants.len()),
                Some((None, Some(&fixture.groups[0]))),
            ),
            (
                format!("the second connection group with {} stored", fixture.tenants.len()),
                Some((None, Some(&fixture.groups[1]))),
            ),
        ];
        for (case, filter) in rows {
            let answer = list_tenants(&router, &list_body(filter, None, None)).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected HTTP 200, got {} with {}",
                answer.status, answer.raw
            );
            let (distribution_filter, group_filter) = filter.unwrap_or((None, None));
            let expected = fixture.expected(distribution_filter, group_filter);
            let got = summary_ids(&answer);
            assert_eq!(
                got, expected,
                "case {case}: expected the tenants the filter names ({expected:?}), got {got:?} \
                 with {}",
                answer.raw
            );
        }
    }
}

/// The unfiltered listing pages: at every page size from the smallest to one past the stored
/// set, walking `Marker`/`NextMarker` reports every stored tenant exactly once, in one stable
/// order.
#[tokio::test]
async fn the_unfiltered_listing_pages_in_one_stable_order() {
    let router = emulator();
    let mut fixture = empty_fixture(&router).await;
    for (slug, distribution, group) in SPREAD {
        let distribution = fixture.distributions[distribution].clone();
        let group = fixture.groups[group].clone();
        let created = tenant(&router, slug, &distribution, &group).await;
        fixture.tenants.push(created);
    }
    let all = fixture.expected(None, None);

    for page_size in [1, 2, 3, 4, 5] {
        let mut walked: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        // One iteration per page, bounded by the stored set so a listing that never
        // terminates fails the case rather than hanging.
        for _ in 0..=all.len() {
            let answer =
                list_tenants(&router, &list_body(None, marker.as_deref(), Some(page_size))).await;
            assert_eq!(
                answer.status, 200,
                "case page size {page_size}: expected HTTP 200, got {} with {}",
                answer.status, answer.raw
            );
            let page = summary_ids(&answer);
            assert!(
                page.len() <= usize::try_from(page_size).expect("a positive page size"),
                "case page size {page_size}: a page served {} summaries, more than asked for, \
                 with {}",
                page.len(),
                answer.raw
            );
            walked.extend(page);
            match answer.root().text_at("NextMarker") {
                Some(next) => marker = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, all,
            "case page size {page_size}: expected every stored tenant exactly once in id \
             order ({all:?}), got {walked:?}"
        );
    }
}
