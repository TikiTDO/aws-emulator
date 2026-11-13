// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the `locale` an IPAM pool reports, driven over `ec2Query`
//! (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: every operation that reports an `IpamPool` carries the modelled
//! `locale` member for EVERY pool — the locale the pool was created with, and for a pool
//! created without one the default spelled out
//! (`ec2::state::IPAM_POOL_LOCALE_NONE`), rather than leaving the element out of the body.
//! So a client reading the member to decide where a pool allocates finds it on every pool,
//! from every reader: the create that mints the pool, the describe that lists it, and the
//! delete that retires it all report the same value, and a `locale` filter selects by the
//! value the body shows.
//!
//! Each table row is a distinct input along what the rule generalises over: whether the
//! create names a locale at all, which Region it names (the IPAM's home Region and another
//! operating Region of the same IPAM), which scope the pool is created in and which address
//! family it holds.

use ec2::state::{
    ADDRESS_FAMILY_IPV4, ADDRESS_FAMILY_IPV6, IPAM_POOL_LOCALE_NONE,
    IPAM_POOL_STATE_DELETE_IN_PROGRESS,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element};

/// The Region every call here is signed for, which is therefore the IPAM's home Region and
/// one of the Regions it operates in.
const REGION: &str = "us-east-1";

/// Another Region of the same partition, given to the IPAM as an operating Region so a pool
/// may name it as its locale: "operating Regions are Amazon Web Services Regions where the
/// IPAM is allowed to manage IP address CIDRs" (`Ipam$OperatingRegions`).
const OTHER_REGION: &str = "us-west-2";

/// One `(name, value)` request member.
fn p(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// One call of `action` signed for the Region above, asserted to have been served.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[(String, String)]) -> String {
    let (status, body) = call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {action} to answer HTTP 200, got {status} with body {body}"
    );
    body
}

/// The two default scopes of one IPAM created over the wire — "an IPAM contains two default
/// scopes" (`IpamScope`) — which is what a pool is created in.
struct SeededIpam {
    public_scope: String,
    private_scope: String,
}

/// Create one IPAM that operates in [`REGION`] and [`OTHER_REGION`], and answer the ids of
/// its two default scopes.
async fn seed_ipam(router: &MultiServiceRouter) -> SeededIpam {
    let case = "seeding the IPAM";
    let body = ok(
        case,
        router,
        "CreateIpam",
        &[p("OperatingRegion.1.RegionName", OTHER_REGION)],
    )
    .await;
    SeededIpam {
        public_scope: element(&body, "publicDefaultScopeId")
            .unwrap_or_else(|| panic!("case {case:?}: the IPAM reports a public scope, got {body}")),
        private_scope: element(&body, "privateDefaultScopeId").unwrap_or_else(|| {
            panic!("case {case:?}: the IPAM reports a private scope, got {body}")
        }),
    }
}

/// One row of the table: a create, and the `locale` every reader of the pool it mints must
/// report.
struct Row {
    /// What makes this row different from the ones beside it.
    case: &'static str,
    /// The `Locale` the create names, or `None` for a create that names none at all.
    locale: Option<&'static str>,
    /// Which of the IPAM's two default scopes the pool is created in.
    public: bool,
    /// The `AddressFamily` the pool holds.
    family: &'static str,
    /// The text the `<locale>` element must carry, in every response reporting this pool.
    expected: &'static str,
}

/// The rows: a create naming no locale, one naming the IPAM's home Region, one naming
/// another Region the IPAM operates in, and a localeless pool of the other address family in
/// the other scope.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an IPv4 pool in the private scope created with no locale",
            locale: None,
            public: false,
            family: ADDRESS_FAMILY_IPV4,
            expected: IPAM_POOL_LOCALE_NONE,
        },
        Row {
            case: "an IPv4 pool in the private scope whose locale is the IPAM's home Region",
            locale: Some(REGION),
            public: false,
            family: ADDRESS_FAMILY_IPV4,
            expected: REGION,
        },
        Row {
            case: "an IPv4 pool in the public scope whose locale is another operating Region",
            locale: Some(OTHER_REGION),
            public: true,
            family: ADDRESS_FAMILY_IPV4,
            expected: OTHER_REGION,
        },
        Row {
            case: "an IPv6 pool in the public scope created with no locale",
            locale: None,
            public: true,
            family: ADDRESS_FAMILY_IPV6,
            expected: IPAM_POOL_LOCALE_NONE,
        },
    ]
}

/// Create the row's pool and answer the `IpamPoolId` the service minted and the body the
/// create answered.
async fn create_pool(router: &MultiServiceRouter, ipam: &SeededIpam, row: &Row) -> (String, String) {
    let scope_id = if row.public {
        &ipam.public_scope
    } else {
        &ipam.private_scope
    };
    let mut params = vec![p("IpamScopeId", scope_id), p("AddressFamily", row.family)];
    if let Some(locale) = row.locale {
        params.push(p("Locale", locale));
    }
    let body = ok(row.case, router, "CreateIpamPool", &params).await;
    let pool_id = element(&body, "ipamPoolId")
        .unwrap_or_else(|| panic!("case {case:?}: the created pool reports an id, got {body}", case = row.case));
    (pool_id, body)
}

/// Every reader of a pool reports its `locale`: the one the create named, or the default
/// spelled out for a pool created without one.
#[tokio::test]
async fn every_reader_of_a_pool_reports_its_locale() {
    for row in rows() {
        let case = row.case;
        // A fresh emulator per row, so one row's pool cannot be what another row reads.
        let (router, _engine) = super::ec2_query::emulator();
        let ipam = seed_ipam(&router).await;
        let (pool_id, created) = create_pool(&router, &ipam, &row).await;
        let expected = row.expected;

        assert_eq!(
            element(&created, "locale").as_deref(),
            Some(expected),
            "case {case:?}: expected the created pool to report <locale>{expected}</locale>, \
             got {created}"
        );

        let described = ok(
            case,
            &router,
            "DescribeIpamPools",
            &[p("IpamPoolId.1", &pool_id)],
        )
        .await;
        assert_eq!(
            element(&described, "locale").as_deref(),
            Some(expected),
            "case {case:?}: expected the described pool {pool_id} to report \
             <locale>{expected}</locale>, got {described}"
        );

        // A filter selects by the value the body shows for the member, so the locale the
        // pool reports names the pool.
        let filtered = ok(
            case,
            &router,
            "DescribeIpamPools",
            &[
                p("Filter.1.Name", "locale"),
                p("Filter.1.Value.1", expected),
            ],
        )
        .await;
        assert_eq!(
            element(&filtered, "ipamPoolId").as_deref(),
            Some(pool_id.as_str()),
            "case {case:?}: expected the locale {expected:?} to select {pool_id}, got {filtered}"
        );

        let deleted = ok(
            case,
            &router,
            "DeleteIpamPool",
            &[p("IpamPoolId", &pool_id)],
        )
        .await;
        assert_eq!(
            element(&deleted, "state").as_deref(),
            Some(IPAM_POOL_STATE_DELETE_IN_PROGRESS),
            "case {case:?}: expected the delete to answer the pool on its way out, got {deleted}"
        );
        assert_eq!(
            element(&deleted, "locale").as_deref(),
            Some(expected),
            "case {case:?}: expected the deleted pool to report <locale>{expected}</locale>, \
             got {deleted}"
        );
    }
}

/// A locale no pool reports selects no pool, and the default spelling selects exactly the
/// pools created without a locale — so the member a pool reports is the member a filter
/// matches, pool by pool, rather than a value every pool answers to.
#[tokio::test]
async fn a_locale_filter_selects_the_pools_reporting_it() {
    let case = "pools spread over three locales";
    let (router, _engine) = super::ec2_query::emulator();
    let ipam = seed_ipam(&router).await;

    let mut by_locale: Vec<(&'static str, String)> = Vec::new();
    for row in rows() {
        let (pool_id, _) = create_pool(&router, &ipam, &row).await;
        by_locale.push((row.expected, pool_id));
    }

    // Every locale the pools report, plus one no pool reports.
    let wanted: Vec<&str> = vec![IPAM_POOL_LOCALE_NONE, REGION, OTHER_REGION, "eu-central-1"];
    for locale in wanted {
        let expected: Vec<String> = by_locale
            .iter()
            .filter(|(reported, _)| *reported == locale)
            .map(|(_, pool_id)| pool_id.clone())
            .collect();
        let body = ok(
            case,
            &router,
            "DescribeIpamPools",
            &[p("Filter.1.Name", "locale"), p("Filter.1.Value.1", locale)],
        )
        .await;
        let mut found = super::ec2_query::elements(&body, "ipamPoolId");
        found.sort();
        let mut wanted_ids = expected.clone();
        wanted_ids.sort();
        assert_eq!(
            found, wanted_ids,
            "case {case:?}: expected the locale {locale:?} to select {wanted_ids:?}, got {body}"
        );
    }
}
