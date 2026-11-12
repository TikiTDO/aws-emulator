// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetIpamDiscoveredRoutes`, driven over the wire the service actually
//! speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. the routes a resource discovery reports for a Region are DERIVED from the store: one
//!    discovered route per BGP ANNOUNCEMENT of the account's own BYOIP address ranges — a range
//!    advertised from a network border group of that Region, announced by each ASN whose
//!    association the range still holds, and by none it keeps no record of — with every member
//!    read off the stored range, and the two advertisement enums mapped onto each other by the
//!    sentence they share (`unicast`/`regional`, `anycast`/`global`);
//! 2. the report is of the Region asked about, through a discovery that OPERATES in it: a range
//!    advertised from another Region's border group is no route of this one, a Local Zone group's
//!    routes are its parent Region's, and a Region the discovery does not operate in has no
//!    discovered route rather than another Region's;
//! 3. `Filters` narrow the report by any member of a reported route (and a filter name the
//!    operation does not resolve is refused rather than read as "no results"), and
//!    `MaxResults`/`NextToken` page it in (prefix, ASN) order behind a token that only resumes
//!    the report it was minted for;
//! 4. a request naming something that is not a resource discovery id, a well-formed id the
//!    account holds no discovery for, an empty `ResourceRegion`, a page size outside the modelled
//!    range or a `NextToken` this service did not issue is refused with EC2's own code; `DryRun`
//!    on a request that would have succeeded reports that; and reading the report changes nothing
//!    a later caller sees.
//!
//! Each table row is a distinct input along what those rules generalise over — the prefix and its
//! address family, which network border group the range is advertised from, which
//! `ByoipCidrState` it sits in, how its advertisement method is spelled, whether it is in a public
//! address pool or an IPAM pool, how many ASNs announce it and whether an association is one the
//! range still holds, which Region is asked about and whether the discovery operates in it, which
//! member a filter names and in what form, and how large a page is asked for — so no row is the
//! specification.

use service_com_amazonaws_ec2_amazonec2::ops::types::{
    AsnAssociation, ByoipCidr, IPAM_BYOIP_ADVERTISEMENT_TYPE_VALUES, IPAM_BYOIP_CIDR_STATE_VALUES,
};
use service_com_amazonaws_ec2_amazonec2::state::{
    ByoipCidrRecord, ComAmazonawsEc2Amazonec2State as Ec2State, ASN_ASSOCIATION_STATE_ASSOCIATED,
    ASN_ASSOCIATION_STATE_DISASSOCIATED, ASN_ASSOCIATION_STATE_FAILED_ASSOCIATION,
    BYOIP_CIDR_STATE_ADVERTISED, BYOIP_CIDR_STATE_PENDING_PROVISION, BYOIP_CIDR_STATE_PROVISIONED,
    BYOIP_CIDR_STATE_PROVISIONED_NOT_PUBLICLY_ADVERTISABLE, DRY_RUN_OPERATION,
    INVALID_IPAM_RESOURCE_DISCOVERY_ID_MALFORMED, INVALID_IPAM_RESOURCE_DISCOVERY_ID_NOT_FOUND,
    INVALID_MAX_RESULTS, INVALID_PAGINATION_TOKEN, INVALID_PARAMETER_VALUE,
    LOCAL_ZONE_NETWORK_BORDER_GROUPS, MISSING_PARAMETER,
};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query;
use super::wire::{child, child_text, document, Param};

/// The Region every call here is signed for, and the home Region of the IPAM it creates. It is
/// the parent Region of two of the documented Local Zone network border groups
/// ([`LOCAL_ZONE_NETWORK_BORDER_GROUPS`]), which the border-group rows need.
const REGION: &str = "us-west-2";

/// A second Region of the same partition, which the discovery under test also operates in.
const OTHER_REGION: &str = "us-east-1";

/// A Region of the same partition no discovery here operates in.
const UNMONITORED_REGION: &str = "ap-southeast-1";

/// The action under test.
const ACTION: &str = "GetIpamDiscoveredRoutes";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// The address family the IPAM pool here holds, as `AddressFamily` spells it.
const FAMILY_IPV4: &str = "ipv4";

/// A public IPv4 pool id of the account, as EC2 spells one, for the row whose range the store
/// holds in such a pool.
const PUBLIC_POOL: &str = "ipv4pool-ec2-0123456789abcdef0";

/// One form field: `("ResourceRegion", "us-west-2")` as the call helpers want it.
fn p(key: &str, value: impl Into<String>) -> Param {
    (key.to_string(), value.into())
}

/// One call of `action` signed for `region`.
async fn api(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[Param],
) -> (u16, String) {
    ec2_query::call(router, region, action, params).await
}

/// One call of `action` signed for [`REGION`] that must succeed, as the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = api(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// Create one IPAM operating in `regions` beside its home Region, and answer the id of the
/// default resource discovery that came up with it ("the default resource discovery is the
/// resource discovery automatically created when you create an IPAM" —
/// `IpamResourceDiscovery$IsDefault`) beside its default public scope.
async fn default_discovery(
    case: &str,
    router: &MultiServiceRouter,
    regions: &[&str],
) -> (String, String) {
    let params: Vec<Param> = regions
        .iter()
        .enumerate()
        .map(|(index, region)| p(&format!("OperatingRegion.{}.RegionName", index + 1), *region))
        .collect();
    let body = ok(case, router, "CreateIpam", &params).await;
    let discovery = ec2_query::element(&body, "defaultResourceDiscoveryId").unwrap_or_else(|| {
        panic!("case {case:?}: CreateIpam must report the default resource discovery: {body}")
    });
    let scope = ec2_query::element(&body, "publicDefaultScopeId").unwrap_or_else(|| {
        panic!("case {case:?}: CreateIpam must report its default public scope: {body}")
    });
    (discovery, scope)
}

/// Create one IPv4 pool in `scope_id` and answer the `IpamPoolId` the service minted.
async fn create_pool(case: &str, router: &MultiServiceRouter, scope_id: &str) -> String {
    let body = ok(
        case,
        router,
        "CreateIpamPool",
        &[p("IpamScopeId", scope_id), p("AddressFamily", FAMILY_IPV4)],
    )
    .await;
    ec2_query::element(&body, "ipamPoolId")
        .unwrap_or_else(|| panic!("case {case:?}: the created pool must report an id: {body}"))
}

/// One BYOIP address range the store is seeded with.
///
/// Seeded rather than provisioned over the wire because no API path of this emulator reaches
/// several of the inputs the rules generalise over: `ProvisionIpamByoasn` is still a scaffold
/// stub, so an ASN association cannot be made through it, and a range's `ByoipCidrState` is the
/// clock's to move. The provision path IS driven, through `ProvisionByoipCidr`, by
/// [`a_provisioned_range_is_discovered_as_a_route_of_its_border_groups_region`] below.
struct SeededRoute {
    /// The prefix the account holds, in canonical form.
    cidr: &'static str,
    /// The network border group the range is advertised from.
    group: &'static str,
    /// The `ByoipCidrState` it sits in.
    state: &'static str,
    /// How `ByoipCidr$AdvertisementType` spells its advertisement method, when it has one.
    advertisement_type: Option<&'static str>,
    /// The public address pool it is in, when the store holds it in one.
    pool_id: Option<&'static str>,
    /// Whether the IPAM pool the fixture creates manages it.
    in_ipam_pool: bool,
    /// Its ASN associations, each with the `AsnAssociationState` it is in.
    asn_associations: &'static [(&'static str, &'static str)],
}

/// Seed one BYOIP address range of the account, managed by `ipam_pool_id` when the row says so.
fn seed_route(
    engine: &engine::InMemoryEngine,
    route: &SeededRoute,
    ipam_pool_id: Option<&str>,
) {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let stored = ByoipCidrRecord {
        byoip_cidr: ByoipCidr {
            cidr: Some(route.cidr.to_string()),
            state: Some(route.state.to_string()),
            advertisement_type: route.advertisement_type.map(str::to_string),
            network_border_group: Some(route.group.to_string()),
            pool_id: route.pool_id.map(str::to_string),
            ipam_pool_id: ipam_pool_id.map(str::to_string),
            asn_associations: Some(
                route
                    .asn_associations
                    .iter()
                    .map(|(asn, state)| AsnAssociation {
                        asn: Some((*asn).to_string()),
                        cidr: Some(route.cidr.to_string()),
                        state: Some((*state).to_string()),
                        status_message: None,
                    })
                    .collect(),
            ),
            ..Default::default()
        },
        released_at: None,
        pending: None,
    };
    assert!(
        guard.insert_byoip_cidr(stored),
        "the store must hold the seeded address range {}",
        route.cidr
    );
}

/// The ranges the account holds: two in the signing Region's own border group, one in a Local
/// Zone group of that Region, one IPv6 range, one whose only ASN association the range no longer
/// holds, and one advertised from ANOTHER Region's group.
const SEEDED_ROUTES: [SeededRoute; 6] = [
    SeededRoute {
        cidr: "192.0.2.0/24",
        group: REGION,
        state: BYOIP_CIDR_STATE_ADVERTISED,
        advertisement_type: Some("unicast"),
        pool_id: None,
        in_ipam_pool: true,
        asn_associations: &[("64512", ASN_ASSOCIATION_STATE_ASSOCIATED)],
    },
    SeededRoute {
        cidr: "198.51.100.0/22",
        group: REGION,
        state: BYOIP_CIDR_STATE_PROVISIONED,
        advertisement_type: Some("anycast"),
        pool_id: Some(PUBLIC_POOL),
        in_ipam_pool: false,
        asn_associations: &[
            ("64520", ASN_ASSOCIATION_STATE_ASSOCIATED),
            ("64513", ASN_ASSOCIATION_STATE_ASSOCIATED),
        ],
    },
    SeededRoute {
        cidr: "203.0.113.0/24",
        group: LOCAL_ZONE_NETWORK_BORDER_GROUPS[1],
        state: BYOIP_CIDR_STATE_PENDING_PROVISION,
        advertisement_type: None,
        pool_id: None,
        in_ipam_pool: false,
        asn_associations: &[],
    },
    SeededRoute {
        cidr: "203.0.115.0/24",
        group: REGION,
        state: BYOIP_CIDR_STATE_ADVERTISED,
        advertisement_type: Some("unicast"),
        pool_id: None,
        in_ipam_pool: false,
        // Neither association is one the range still holds, so neither announces it.
        asn_associations: &[
            ("64540", ASN_ASSOCIATION_STATE_DISASSOCIATED),
            ("64541", ASN_ASSOCIATION_STATE_FAILED_ASSOCIATION),
        ],
    },
    SeededRoute {
        cidr: "2600:1f14:abc::/48",
        group: REGION,
        state: BYOIP_CIDR_STATE_PROVISIONED_NOT_PUBLICLY_ADVERTISABLE,
        advertisement_type: None,
        pool_id: None,
        in_ipam_pool: false,
        asn_associations: &[("64530", ASN_ASSOCIATION_STATE_ASSOCIATED)],
    },
    SeededRoute {
        cidr: "203.0.114.0/24",
        group: OTHER_REGION,
        state: BYOIP_CIDR_STATE_ADVERTISED,
        advertisement_type: Some("anycast"),
        pool_id: None,
        in_ipam_pool: false,
        asn_associations: &[("64550", ASN_ASSOCIATION_STATE_ASSOCIATED)],
    },
];

/// One announcement the report of [`REGION`] is expected to carry, in the order it pages them.
struct Expected {
    /// What this row is called in a failure message.
    case: &'static str,
    /// The prefix announced.
    cidr: &'static str,
    /// The ASN announcing it, or `None` for a range holding no ASN association.
    asn: Option<&'static str>,
    /// The `IpamByoipCidrState` the route reports.
    state: &'static str,
    /// The `IpamByoipAdvertisementType` the route reports, when the range has a method.
    advertisement_type: Option<&'static str>,
    /// The network border group the route reports.
    group: &'static str,
    /// The public address pool the route reports, when the range is in one.
    pool_id: Option<&'static str>,
    /// Whether the IPAM pool the fixture creates is reported as managing it.
    in_ipam_pool: bool,
}

/// The announcements of [`REGION`]'s routes, in (prefix, ASN) order.
const EXPECTED: [Expected; 6] = [
    Expected {
        case: "an advertised range announced by one ASN, managed by an IPAM pool",
        cidr: "192.0.2.0/24",
        asn: Some("64512"),
        state: BYOIP_CIDR_STATE_ADVERTISED,
        advertisement_type: Some("regional"),
        group: REGION,
        pool_id: None,
        in_ipam_pool: true,
    },
    Expected {
        case: "the first ASN announcing a range two ASNs announce",
        cidr: "198.51.100.0/22",
        asn: Some("64513"),
        state: BYOIP_CIDR_STATE_PROVISIONED,
        advertisement_type: Some("global"),
        group: REGION,
        pool_id: Some(PUBLIC_POOL),
        in_ipam_pool: false,
    },
    Expected {
        case: "the second ASN announcing that range",
        cidr: "198.51.100.0/22",
        asn: Some("64520"),
        state: BYOIP_CIDR_STATE_PROVISIONED,
        advertisement_type: Some("global"),
        group: REGION,
        pool_id: Some(PUBLIC_POOL),
        in_ipam_pool: false,
    },
    Expected {
        case: "a range in a Local Zone group of the Region, announced by no ASN on record",
        cidr: "203.0.113.0/24",
        asn: None,
        state: BYOIP_CIDR_STATE_PENDING_PROVISION,
        advertisement_type: None,
        group: LOCAL_ZONE_NETWORK_BORDER_GROUPS[1],
        pool_id: None,
        in_ipam_pool: false,
    },
    Expected {
        case: "a range whose ASN associations it no longer holds",
        cidr: "203.0.115.0/24",
        asn: None,
        state: BYOIP_CIDR_STATE_ADVERTISED,
        advertisement_type: Some("regional"),
        group: REGION,
        pool_id: None,
        in_ipam_pool: false,
    },
    Expected {
        case: "an IPv6 range that is not publicly advertisable",
        cidr: "2600:1f14:abc::/48",
        asn: Some("64530"),
        state: BYOIP_CIDR_STATE_PROVISIONED_NOT_PUBLICLY_ADVERTISABLE,
        advertisement_type: None,
        group: REGION,
        pool_id: None,
        in_ipam_pool: false,
    },
];

/// The members of one discovered route the assertions below read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Reported {
    cidr: String,
    asn: Option<String>,
    discovery_id: Option<String>,
    resource_region: Option<String>,
    resource_owner_id: Option<String>,
    state: Option<String>,
    advertisement_type: Option<String>,
    group: Option<String>,
    pool_id: Option<String>,
    ipam_pool_id: Option<String>,
    sample_time: Option<String>,
}

/// Every route of one `GetIpamDiscoveredRoutes` response, in the order it reported them.
///
/// Read through the XML parser rather than by substring, so a member of the envelope is never
/// mistaken for one of a route's own.
fn reported(case: &str, body: &str) -> Vec<Reported> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .filter(|node| child(*node, "cidr").is_some())
        .map(|item| Reported {
            cidr: child_text(item, "cidr").unwrap_or_default(),
            asn: child_text(item, "asn"),
            discovery_id: child_text(item, "ipamResourceDiscoveryId"),
            resource_region: child_text(item, "resourceRegion"),
            resource_owner_id: child_text(item, "resourceOwnerId"),
            state: child_text(item, "state"),
            advertisement_type: child_text(item, "advertisementType"),
            group: child_text(item, "networkBorderGroup"),
            pool_id: child_text(item, "poolId"),
            ipam_pool_id: child_text(item, "ipamPoolId"),
            sample_time: child_text(item, "sampleTime"),
        })
        .collect()
}

/// One row of the Region table below: the case, the discovery the report is asked through, the
/// Region it is asked about, and the announcements it must carry.
type RegionRow<'ids> = (&'static str, &'ids str, &'static str, Vec<(String, String)>);

/// The announcement one reported route is of, as the page order names it.
fn announcement(route: &Reported) -> (String, String) {
    (
        route.cidr.clone(),
        route.asn.clone().unwrap_or_default(),
    )
}

/// The announcements one report carried, as `(prefix, ASN)` pairs.
fn announcements(case: &str, body: &str) -> Vec<(String, String)> {
    reported(case, body).iter().map(announcement).collect()
}

/// One report request for `region` through `discovery`, with `extra` beside it.
fn report_params(discovery: &str, region: &str, extra: Vec<Param>) -> Vec<Param> {
    let mut params = vec![
        p("IpamResourceDiscoveryId", discovery),
        p("ResourceRegion", region),
    ];
    params.extend(extra);
    params
}

/// What a suite drives: the router, the engine whose store holds the seeded ranges, the default
/// resource discovery of an IPAM operating in two Regions, and the IPAM pool one range is in.
struct Fixture {
    router: MultiServiceRouter,
    discovery: String,
    ipam_pool: String,
}

/// The emulator with the IPAM, its pool and every seeded range in place.
async fn fixture(case: &str) -> Fixture {
    let (router, engine) = ec2_query::emulator();
    let (discovery, scope) = default_discovery(case, &router, &[OTHER_REGION]).await;
    let ipam_pool = create_pool(case, &router, &scope).await;
    for route in &SEEDED_ROUTES {
        let pool = route.in_ipam_pool.then_some(ipam_pool.as_str());
        seed_route(&engine, route, pool);
    }
    Fixture {
        router,
        discovery,
        ipam_pool,
    }
}

// ---------------------------------------------------------------------------
// What the report names
// ---------------------------------------------------------------------------

/// Rule 1: one discovered route per announcement of the Region's ranges, every member read off
/// the stored range.
///
/// The rows differ along what that generalises over: the prefix and its address family, how many
/// ASNs announce it, whether an association is one the range still holds, the `ByoipCidrState` it
/// sits in, how its advertisement method is spelled, which border group it is advertised from, and
/// whether it is in a public address pool or an IPAM pool.
#[tokio::test]
async fn every_announcement_of_the_regions_ranges_is_reported_from_the_store() {
    let case = "the whole report";
    let fixture = fixture(case).await;
    let body = ok(
        case,
        &fixture.router,
        ACTION,
        &report_params(&fixture.discovery, REGION, Vec::new()),
    )
    .await;

    let routes = reported(case, &body);
    let expected: Vec<(String, String)> = EXPECTED
        .iter()
        .map(|row| (row.cidr.to_string(), row.asn.unwrap_or_default().to_string()))
        .collect();
    let got: Vec<(String, String)> = routes.iter().map(announcement).collect();
    assert_eq!(
        got, expected,
        "case {case:?}: expected one route per announcement of the Region's ranges, in \
         (prefix, ASN) order: got {got:?} in {body}"
    );

    let owner = engine::emulator_account_id().to_string();
    for (row, route) in EXPECTED.iter().zip(&routes) {
        let row_case = row.case;
        assert_eq!(
            route.asn.as_deref(),
            row.asn,
            "case {row_case:?}: expected the originating ASN {:?}, got {:?}",
            row.asn,
            route.asn
        );
        assert_eq!(
            route.state.as_deref(),
            Some(row.state),
            "case {row_case:?}: expected the range's stored state {:?}, got {:?}",
            row.state,
            route.state
        );
        assert!(
            IPAM_BYOIP_CIDR_STATE_VALUES.contains(&row.state),
            "case {row_case:?}: the state {:?} must be a member of the modelled \
             IpamByoipCidrState catalog {IPAM_BYOIP_CIDR_STATE_VALUES:?}",
            row.state
        );
        assert_eq!(
            route.advertisement_type.as_deref(),
            row.advertisement_type,
            "case {row_case:?}: expected the advertisement type {:?}, got {:?}",
            row.advertisement_type,
            route.advertisement_type
        );
        if let Some(advertisement_type) = row.advertisement_type {
            assert!(
                IPAM_BYOIP_ADVERTISEMENT_TYPE_VALUES.contains(&advertisement_type),
                "case {row_case:?}: the advertisement type {advertisement_type:?} must be a \
                 member of the modelled IpamByoipAdvertisementType catalog \
                 {IPAM_BYOIP_ADVERTISEMENT_TYPE_VALUES:?}"
            );
        }
        assert_eq!(
            route.group.as_deref(),
            Some(row.group),
            "case {row_case:?}: expected the stored network border group {:?}, got {:?}",
            row.group,
            route.group
        );
        assert_eq!(
            route.pool_id.as_deref(),
            row.pool_id,
            "case {row_case:?}: expected the stored public address pool {:?}, got {:?}",
            row.pool_id,
            route.pool_id
        );
        let expected_ipam_pool = row.in_ipam_pool.then_some(fixture.ipam_pool.as_str());
        assert_eq!(
            route.ipam_pool_id.as_deref(),
            expected_ipam_pool,
            "case {row_case:?}: expected the managing IPAM pool {expected_ipam_pool:?}, got {:?}",
            route.ipam_pool_id
        );
        assert_eq!(
            route.discovery_id.as_deref(),
            Some(fixture.discovery.as_str()),
            "case {row_case:?}: expected the discovery {:?} the report was asked through, got {:?}",
            fixture.discovery,
            route.discovery_id
        );
        assert_eq!(
            route.resource_region.as_deref(),
            Some(REGION),
            "case {row_case:?}: expected the Region {REGION:?} the report is of, got {:?}",
            route.resource_region
        );
        assert_eq!(
            route.resource_owner_id.as_deref(),
            Some(owner.as_str()),
            "case {row_case:?}: expected the owning account {owner:?}, got {:?}",
            route.resource_owner_id
        );
        let sampled = route
            .sample_time
            .as_deref()
            .and_then(ec2_query::as_timestamp);
        assert!(
            sampled.is_some(),
            "case {row_case:?}: expected an ISO 8601 sample time, got {:?}",
            route.sample_time
        );
    }

    // The ranges the report derives its routes from are the ranges the account holds: one store,
    // two reads.
    let described = ok(
        case,
        &fixture.router,
        "DescribeByoipCidrs",
        &[p("MaxResults", "100")],
    )
    .await;
    for row in &EXPECTED {
        let held = ec2_query::elements(&described, "cidr");
        assert!(
            held.iter().any(|cidr| cidr == row.cidr),
            "case {:?}: DescribeByoipCidrs must report the range {:?} the route is of, got {held:?}",
            row.case,
            row.cidr
        );
    }
}

/// Rule 1, through a real write: a range brought by `ProvisionByoipCidr` is discovered as a route
/// of the Region its border group belongs to, carrying what that provision stored.
///
/// The rows differ along what the border-group rule generalises over: a provision that names no
/// group at all (so the range is advertised from the signing Region) and one that names a Local
/// Zone group of that Region.
#[tokio::test]
async fn a_provisioned_range_is_discovered_as_a_route_of_its_border_groups_region() {
    let case = "provisioned ranges";
    let (router, _engine) = ec2_query::emulator();
    let (discovery, _scope) = default_discovery(case, &router, &[]).await;

    // (case, the Cidr provisioned, the NetworkBorderGroup asked for, the group expected), in the
    // (prefix, ASN) order the report pages them.
    let rows: [(&str, &str, Option<&str>, &str); 2] = [
        (
            "a provision naming a Local Zone group of the Region",
            "45.33.0.0/16",
            Some(LOCAL_ZONE_NETWORK_BORDER_GROUPS[1]),
            LOCAL_ZONE_NETWORK_BORDER_GROUPS[1],
        ),
        (
            "a provision naming no network border group",
            "81.2.69.0/24",
            None,
            REGION,
        ),
    ];

    let mut provisioned: Vec<(&str, &str, String)> = Vec::new();
    for (row, cidr, group, expected_group) in rows {
        let mut params = vec![p("Cidr", cidr)];
        if let Some(group) = group {
            params.push(p("NetworkBorderGroup", group));
        }
        let body = ok(row, &router, "ProvisionByoipCidr", &params).await;
        let pool_id = ec2_query::element(&body, "poolId").unwrap_or_else(|| {
            panic!("case {row:?}: a provision must mint an address pool: {body}")
        });
        provisioned.push((row, expected_group, pool_id));
    }

    let body = ok(
        case,
        &router,
        ACTION,
        &report_params(&discovery, REGION, Vec::new()),
    )
    .await;
    let routes = reported(case, &body);
    assert_eq!(
        routes.len(),
        rows.len(),
        "case {case:?}: expected one route per provisioned range, got {routes:?} in {body}"
    );
    for ((row, expected_group, pool_id), route) in provisioned.iter().zip(&routes) {
        assert_eq!(
            route.state.as_deref(),
            Some(BYOIP_CIDR_STATE_PENDING_PROVISION),
            "case {row:?}: a range the clock has not provisioned yet is discovered in the state \
             the provision left it in, got {:?}",
            route.state
        );
        assert_eq!(
            route.group.as_deref(),
            Some(*expected_group),
            "case {row:?}: expected the border group {expected_group:?} the provision stored, \
             got {:?}",
            route.group
        );
        assert_eq!(
            route.pool_id.as_deref(),
            Some(pool_id.as_str()),
            "case {row:?}: expected the address pool {pool_id:?} the provision minted, got {:?}",
            route.pool_id
        );
        assert_eq!(
            route.ipam_pool_id, None,
            "case {row:?}: a provisioned range no IPAM manages is in no IPAM pool, got {:?}",
            route.ipam_pool_id
        );
        assert_eq!(
            route.asn, None,
            "case {row:?}: a range holding no ASN association is announced by no ASN on record, \
             got {:?}",
            route.asn
        );
    }
}

/// Rule 2: the report is of the Region asked about, through a discovery that operates in it.
///
/// The rows differ along what that generalises over: which Region is asked about, whether the
/// discovery operates in it, and whether the Region holds ranges of its own.
#[tokio::test]
async fn the_report_is_of_a_region_the_discovery_operates_in() {
    let case = "a narrow discovery and a wide one";
    let fixture = fixture(case).await;
    // A second IPAM whose default discovery operates only in its home Region.
    let (home_only, _scope) = default_discovery(case, &fixture.router, &[]).await;

    let region_routes: Vec<(String, String)> = EXPECTED
        .iter()
        .map(|row| (row.cidr.to_string(), row.asn.unwrap_or_default().to_string()))
        .collect();

    // (case, the discovery asked through, the Region asked about, the announcements expected)
    let rows: [RegionRow<'_>; 5] = [
        (
            "the home Region of a discovery operating in two",
            &fixture.discovery,
            REGION,
            region_routes.clone(),
        ),
        (
            "the second operating Region, which holds one range",
            &fixture.discovery,
            OTHER_REGION,
            vec![("203.0.114.0/24".to_string(), "64550".to_string())],
        ),
        (
            "a Region that discovery does not operate in",
            &fixture.discovery,
            UNMONITORED_REGION,
            Vec::new(),
        ),
        (
            "the home Region of a discovery operating only there",
            &home_only,
            REGION,
            region_routes.clone(),
        ),
        (
            "a Region the narrow discovery does not operate in, holding a range",
            &home_only,
            OTHER_REGION,
            Vec::new(),
        ),
    ];

    for (row, discovery, region, expected) in rows {
        let body = ok(
            row,
            &fixture.router,
            ACTION,
            &report_params(discovery, region, Vec::new()),
        )
        .await;
        assert_eq!(
            announcements(row, &body),
            expected,
            "case {row:?}: expected the announcements {expected:?} in {body}"
        );
        // A report answered in one page carries no token, whether or not it reported anything.
        let token = ec2_query::member(&body, "nextToken");
        assert_eq!(
            token, None,
            "case {row:?}: expected no nextToken on the only page, got {token:?} in {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Narrowing the report and paging it
// ---------------------------------------------------------------------------

/// Rule 3, first half: a filter selects the routes whose member it names matches, and a name this
/// operation does not resolve is refused.
///
/// The rows differ along what that generalises over: which member the filter names, whether the
/// value is exact, carries EC2's `*` wildcard or is one of several, and whether it selects some
/// routes or none.
#[tokio::test]
async fn filters_narrow_the_report_and_an_unresolvable_filter_name_is_refused() {
    let case = "filters over the report";
    let fixture = fixture(case).await;
    let owner = engine::emulator_account_id().to_string();

    // (case, filter name, filter values, the prefixes expected)
    let rows: [(&str, &str, Vec<String>, Vec<&str>); 11] = [
        (
            "one prefix by its exact value",
            "cidr",
            vec!["192.0.2.0/24".to_string()],
            vec!["192.0.2.0/24"],
        ),
        (
            "a wildcard over the prefix",
            "cidr",
            vec!["203.0.*".to_string()],
            vec!["203.0.113.0/24", "203.0.115.0/24"],
        ),
        (
            "two prefixes, joined with an OR",
            "cidr",
            vec!["192.0.2.0/24".to_string(), "2600:1f14:abc::/48".to_string()],
            vec!["192.0.2.0/24", "2600:1f14:abc::/48"],
        ),
        (
            "a prefix the account does not hold",
            "cidr",
            vec!["198.18.0.0/24".to_string()],
            Vec::new(),
        ),
        (
            "the state the clock has not finished",
            "state",
            vec![BYOIP_CIDR_STATE_PENDING_PROVISION.to_string()],
            vec!["203.0.113.0/24"],
        ),
        (
            "the advertisement type of a range advertised from one location",
            "advertisement-type",
            vec!["regional".to_string()],
            vec!["192.0.2.0/24", "203.0.115.0/24"],
        ),
        (
            "the originating ASN of one announcement",
            "asn",
            vec!["64520".to_string()],
            vec!["198.51.100.0/22"],
        ),
        (
            "the Local Zone border group of the Region",
            "network-border-group",
            vec![LOCAL_ZONE_NETWORK_BORDER_GROUPS[1].to_string()],
            vec!["203.0.113.0/24"],
        ),
        (
            "the public address pool one range is in",
            "pool-id",
            vec![PUBLIC_POOL.to_string()],
            vec!["198.51.100.0/22", "198.51.100.0/22"],
        ),
        (
            "the account every route is owned by",
            "resource-owner-id",
            vec![owner.clone()],
            vec![
                "192.0.2.0/24",
                "198.51.100.0/22",
                "198.51.100.0/22",
                "203.0.113.0/24",
                "203.0.115.0/24",
                "2600:1f14:abc::/48",
            ],
        ),
        (
            "a wildcard over the sample time",
            "sample-time",
            vec!["*T*".to_string()],
            vec![
                "192.0.2.0/24",
                "198.51.100.0/22",
                "198.51.100.0/22",
                "203.0.113.0/24",
                "203.0.115.0/24",
                "2600:1f14:abc::/48",
            ],
        ),
    ];

    for (row, name, values, expected) in rows {
        let mut extra = vec![p("Filter.1.Name", name)];
        for (index, value) in values.iter().enumerate() {
            extra.push(p(&format!("Filter.1.Value.{}", index + 1), value.clone()));
        }
        let body = ok(
            row,
            &fixture.router,
            ACTION,
            &report_params(&fixture.discovery, REGION, extra),
        )
        .await;
        let got: Vec<String> = reported(row, &body)
            .into_iter()
            .map(|route| route.cidr)
            .collect();
        let expected: Vec<String> = expected.iter().map(|cidr| (*cidr).to_string()).collect();
        assert_eq!(
            got, expected,
            "case {row:?}: expected the prefixes {expected:?}, got {got:?} in {body}"
        );
    }

    // The IPAM pool filter names the pool the fixture created, so it is read off the fixture
    // rather than written as a literal.
    let row = "the IPAM pool managing one range";
    let body = ok(
        row,
        &fixture.router,
        ACTION,
        &report_params(
            &fixture.discovery,
            REGION,
            vec![
                p("Filter.1.Name", "ipam-pool-id"),
                p("Filter.1.Value.1", fixture.ipam_pool.clone()),
            ],
        ),
    )
    .await;
    let got: Vec<String> = reported(row, &body)
        .into_iter()
        .map(|route| route.cidr)
        .collect();
    assert_eq!(
        got,
        vec!["192.0.2.0/24".to_string()],
        "case {row:?}: expected the one range the IPAM pool manages, got {got:?} in {body}"
    );

    // Two filters are joined with an AND, so one that holds and one that cannot select nothing
    // together.
    let row = "an AND of two filters";
    let body = ok(
        row,
        &fixture.router,
        ACTION,
        &report_params(
            &fixture.discovery,
            REGION,
            vec![
                p("Filter.1.Name", "resource-region"),
                p("Filter.1.Value.1", REGION),
                p("Filter.2.Name", "state"),
                p("Filter.2.Value.1", "deprovisioned"),
            ],
        ),
    )
    .await;
    assert!(
        reported(row, &body).is_empty(),
        "case {row:?}: the AND of two filters selects nothing, got {body}"
    );

    // A filter name the operation does not resolve is refused — including the `tag:` set an EC2
    // describe of a TAGGABLE resource accepts, which a discovered route has no member for.
    for (row, name) in [
        ("a filter name nothing answers", "discovered-route"),
        ("a tag filter on a resource carrying no tags", "tag-key"),
        ("a tag filter naming a key", "tag:Name"),
    ] {
        let (status, body) = api(
            &fixture.router,
            REGION,
            ACTION,
            &report_params(
                &fixture.discovery,
                REGION,
                vec![p("Filter.1.Name", name), p("Filter.1.Value.1", "anything")],
            ),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {row:?}: expected HTTP 400 for the filter {name:?}, got {status}: {body}"
        );
        assert_eq!(
            ec2_query::error_code(&body),
            INVALID_PARAMETER_VALUE,
            "case {row:?}: expected {INVALID_PARAMETER_VALUE} for the filter {name:?}, got {body}"
        );
        assert!(
            ec2_query::error_message(&body).contains(name),
            "case {row:?}: expected the refusal to name the filter {name:?}, got {}",
            ec2_query::error_message(&body)
        );
    }
}

/// Rule 3, second half: `MaxResults` pages the report in (prefix, ASN) order, the walk ends, and
/// a token only resumes the report it was minted for.
///
/// The rows differ along what that generalises over: the page size asked for, at each published
/// bound of the modelled range and between them, beside a request naming none at all.
#[tokio::test]
async fn max_results_pages_the_report_and_a_token_resumes_only_its_own_report() {
    let case = "paging the report";
    let fixture = fixture(case).await;
    let whole: Vec<(String, String)> = EXPECTED
        .iter()
        .map(|row| (row.cidr.to_string(), row.asn.unwrap_or_default().to_string()))
        .collect();

    // The model's own range for the member is 5..1000
    // (`smithy_query(query="operation",operation="GetIpamDiscoveredRoutes")`), so 5 is the
    // smallest page a request may ask for and 1000 the largest.
    for size in [5_usize, 6, 999, 1000] {
        let row = format!("MaxResults={size}");
        let mut walked: Vec<(String, String)> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut extra = vec![p("MaxResults", size.to_string())];
            if let Some(next) = token.clone() {
                extra.push(p("NextToken", next));
            }
            let body = ok(
                &row,
                &fixture.router,
                ACTION,
                &report_params(&fixture.discovery, REGION, extra),
            )
            .await;
            let page = announcements(&row, &body);
            assert!(
                page.len() <= size,
                "case {row:?}: expected at most {size} routes in a page, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= whole.len() + 1,
                "case {row:?}: the walk must end, got {pages} pages for {walked:?}"
            );
            token = ec2_query::member(&body, "nextToken").filter(|value| !value.is_empty());
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, whole,
            "case {row:?}: expected the whole report once, in (prefix, ASN) order, got {walked:?}"
        );
    }

    // A request naming no page size is not paged at all: the whole report in one answer, with no
    // token to follow.
    let row = "no MaxResults at all";
    let body = ok(
        row,
        &fixture.router,
        ACTION,
        &report_params(&fixture.discovery, REGION, Vec::new()),
    )
    .await;
    assert_eq!(
        announcements(row, &body),
        whole,
        "case {row:?}: expected the whole report in one answer, got {body}"
    );
    assert_eq!(
        ec2_query::member(&body, "nextToken"),
        None,
        "case {row:?}: an unpaged report carries no token, got {body}"
    );

    // A token is minted for ONE report: the discovery it was asked through and the Region it was
    // asked about. Presented to any other report it is refused rather than resumed.
    let row = "a token from the home Region's walk";
    let first = ok(
        row,
        &fixture.router,
        ACTION,
        &report_params(&fixture.discovery, REGION, vec![p("MaxResults", "5")]),
    )
    .await;
    let token = ec2_query::member(&first, "nextToken")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| panic!("case {row:?}: a short page must carry a token: {first}"));
    let (other_discovery, _scope) = default_discovery(row, &fixture.router, &[OTHER_REGION]).await;
    for (inner, discovery, region) in [
        ("the same discovery, another Region", &fixture.discovery, OTHER_REGION),
        ("another discovery, the same Region", &other_discovery, REGION),
    ] {
        let (status, body) = api(
            &fixture.router,
            REGION,
            ACTION,
            &report_params(discovery, region, vec![p("NextToken", token.clone())]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {inner:?}: expected HTTP 400 for a token from another report, got {status}: \
             {body}"
        );
        assert_eq!(
            ec2_query::error_code(&body),
            INVALID_PAGINATION_TOKEN,
            "case {inner:?}: expected {INVALID_PAGINATION_TOKEN}, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// What the operation refuses
// ---------------------------------------------------------------------------

/// One row of the refusal table below: the request member at fault and the refusal it is owed.
struct Refusal<'ids> {
    /// The row's case label, named by every assertion about it.
    case: &'static str,
    /// The `IpamResourceDiscoveryId` this row sends.
    discovery_id: &'ids str,
    /// The `ResourceRegion` this row sends.
    region: &'static str,
    /// Whatever this row sends beside those two.
    extra: Vec<Param>,
    /// The HTTP status the refusal carries.
    status: u16,
    /// The EC2 error code the refusal carries.
    code: &'static str,
}

/// Rule 4: a request whose discovery id, Region, page size or `NextToken` the service cannot
/// resolve is refused with EC2's own code, a `DryRun` of a request that would have succeeded
/// reports that, and reading the report changes nothing a later caller sees.
///
/// The rows differ along what that generalises over: which member is at fault and in what way —
/// an id of the wrong form, an id of a NEIGHBOURING resource of the same prefix, a well-formed id
/// naming nothing, a Region naming nothing at all, a page size one past each published bound, and
/// a token this service never issued.
#[tokio::test]
async fn an_unresolvable_request_is_refused_and_reading_the_report_changes_nothing() {
    let case = "refusals";
    let fixture = fixture(case).await;
    let before = ok(
        case,
        &fixture.router,
        ACTION,
        &report_params(&fixture.discovery, REGION, Vec::new()),
    )
    .await;

    let rows: [Refusal<'_>; 8] = [
        Refusal {
            case: "not an id of any resource",
            discovery_id: "not-an-id",
            region: REGION,
            extra: Vec::new(),
            status: 400,
            code: INVALID_IPAM_RESOURCE_DISCOVERY_ID_MALFORMED,
        },
        Refusal {
            case: "an id of the neighbouring association resource",
            discovery_id: "ipam-res-disco-assoc-0123456789abcdef0",
            region: REGION,
            extra: Vec::new(),
            status: 400,
            code: INVALID_IPAM_RESOURCE_DISCOVERY_ID_MALFORMED,
        },
        Refusal {
            case: "a well-formed long id naming no discovery",
            discovery_id: "ipam-res-disco-0123456789abcdef0",
            region: REGION,
            extra: Vec::new(),
            status: 400,
            code: INVALID_IPAM_RESOURCE_DISCOVERY_ID_NOT_FOUND,
        },
        Refusal {
            case: "a well-formed short id naming no discovery",
            discovery_id: "ipam-res-disco-01234567",
            region: REGION,
            extra: Vec::new(),
            status: 400,
            code: INVALID_IPAM_RESOURCE_DISCOVERY_ID_NOT_FOUND,
        },
        Refusal {
            case: "a Region naming nothing at all",
            discovery_id: fixture.discovery.as_str(),
            region: "",
            extra: Vec::new(),
            status: 400,
            code: MISSING_PARAMETER,
        },
        Refusal {
            case: "a page size one below the modelled minimum",
            discovery_id: fixture.discovery.as_str(),
            region: REGION,
            extra: vec![p("MaxResults", "4")],
            status: 400,
            code: INVALID_MAX_RESULTS,
        },
        Refusal {
            case: "a page size one above the modelled maximum",
            discovery_id: fixture.discovery.as_str(),
            region: REGION,
            extra: vec![p("MaxResults", "1001")],
            status: 400,
            // Above the modelled `IpamMaxResults` range the boundary validator refuses the member
            // itself, before this operation's handler runs; only the MINIMUM carries EC2's own
            // `InvalidMaxResults` wording (`crate::service`'s refusal override).
            code: "ValidationError",
        },
        Refusal {
            case: "a token this service did not issue",
            discovery_id: fixture.discovery.as_str(),
            region: REGION,
            extra: vec![p("NextToken", "not-a-token")],
            status: 400,
            code: INVALID_PAGINATION_TOKEN,
        },
    ];

    for row in rows {
        let Refusal {
            case: row_case,
            discovery_id,
            region,
            extra,
            status: expected_status,
            code: expected_code,
        } = row;
        let (status, body) = api(
            &fixture.router,
            REGION,
            ACTION,
            &report_params(discovery_id, region, extra),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {row_case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            ec2_query::error_code(&body),
            expected_code,
            "case {row_case:?}: expected {expected_code}, got {body}"
        );
    }

    // "If you have the required permissions, the error response is DryRunOperation": the request
    // would have succeeded, so that is what it is told.
    let (status, body) = api(
        &fixture.router,
        REGION,
        ACTION,
        &report_params(&fixture.discovery, REGION, vec![p("DryRun", "true")]),
    )
    .await;
    assert_eq!(
        status, DRY_RUN_STATUS,
        "case {case:?}: expected HTTP {DRY_RUN_STATUS} from a dry run, got {status} with body \
         {body}"
    );
    assert_eq!(
        ec2_query::error_code(&body),
        DRY_RUN_OPERATION,
        "case {case:?}: expected {DRY_RUN_OPERATION} from a dry run, got {body}"
    );

    // The report is a READ: every refusal, dry run and earlier read above left the routes exactly
    // as they were. The sample time is the instant each sample was taken, so it is the one member
    // two reads are not expected to agree on.
    let after = ok(
        case,
        &fixture.router,
        ACTION,
        &report_params(&fixture.discovery, REGION, Vec::new()),
    )
    .await;
    let strip = |case: &str, body: &str| -> Vec<Reported> {
        reported(case, body)
            .into_iter()
            .map(|mut route| {
                route.sample_time = None;
                route
            })
            .collect()
    };
    assert_eq!(
        strip(case, &after),
        strip(case, &before),
        "case {case:?}: reading the report must change nothing a later caller sees, got {after}"
    );
}
