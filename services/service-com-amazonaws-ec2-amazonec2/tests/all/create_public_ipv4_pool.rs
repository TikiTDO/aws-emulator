// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreatePublicIpv4Pool`, driven over the wire the service actually
//! speaks: `ec2Query`, an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=<Operation>`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//! The round trip is read back through `DescribePublicIpv4Pools`, whose own rules the
//! sibling `public_ipv4_pools` suite covers.
//!
//! The rules under test:
//!
//! 1. THE CALL MINTS AN EMPTY POOL AND PERSISTS IT: "Creates a public IPv4 address pool"
//!    (`CreatePublicIpv4Pool`) under an `ipv4pool-ec2-…` id the SERVICE mints — the request
//!    carries no id member — advertised from a network border group the signed Region
//!    advertises addresses from, carrying the tags the request asked for, and holding NO
//!    address range (a CIDR reaches a pool through `ProvisionPublicIpv4PoolCidr`), so both
//!    of its counts are zero until one does.
//! 2. EVERY CREATE MINTS ITS OWN POOL: however many pools an account creates, no two share
//!    an id and the store reports every one of them.
//! 3. WHAT IT REFUSES: a `NetworkBorderGroup` the Region does not advertise addresses from
//!    is HTTP 400 `InvalidParameterValue`; a `TagSpecification` outside the documented tag
//!    rules is refused with the code those rules carry; and a `DryRun` is answered from the
//!    permission check alone (HTTP 412 `DryRunOperation`) — in every case with no pool
//!    created.
//!
//! Each table row is a distinct input along what the rules generalise over — how many tags
//! and of what shape, which border group, how many pools, which rule a refused request
//! breaks — so no single recorded call is the specification.

use ec2::state::{MAX_TAGS_PER_RESOURCE, PUBLIC_IPV4_POOL_RESOURCE_TYPE};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, filters, indexed};

/// The Region every call here is signed for, and so the network border group a pool created
/// without one is advertised from.
const REGION: &str = "us-west-2";

/// A Local Zone network border group of that Region — the other reading of "The
/// Availability Zone (AZ) or Local Zone (LZ) network border group"
/// (`CreatePublicIpv4PoolRequest$NetworkBorderGroup`), taken from the Local Zone groups
/// `crate::state::LOCAL_ZONE_NETWORK_BORDER_GROUPS` lists for this Region.
const LOCAL_ZONE_GROUP: &str = "us-west-2-lax-1";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// One row of the create table: `(case, the parameters, the border group it must be
/// advertised from, the tags that must round-trip)`.
type CreateRow = (&'static str, Params, &'static str, Vec<(&'static str, &'static str)>);

/// One row of a table that expects a refusal: `(case, the parameters, the status, the error
/// code)`.
type RefusalRow = (&'static str, Params, u16, &'static str);

/// The emulator the tests drive.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One `CreatePublicIpv4Pool` call carrying `params`.
async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "CreatePublicIpv4Pool", params).await
}

/// One `DescribePublicIpv4Pools` call carrying `params`.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribePublicIpv4Pools", params).await
}

/// A `PoolId` is `ipv4pool-ec2-` followed by 8 or 17 lowercase hexadecimal digits.
///
/// Spelled out here ON PURPOSE rather than calling the service's own id reader: this is the
/// test's INDEPENDENT statement of the form EC2 mints an `Ipv4PoolEc2Id` in, so a change to
/// the production helper cannot make the assertion agree with it by construction.
fn is_pool_id(candidate: &str) -> bool {
    let Some(suffix) = candidate.strip_prefix("ipv4pool-ec2-") else {
        return false;
    };
    matches!(suffix.len(), 8 | 17)
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// Create one pool and answer the id it was minted under, failing the case when the call did
/// not succeed or answered something that is no pool id.
async fn create_one(router: &MultiServiceRouter, case: &str, params: &[(String, String)]) -> String {
    let (status, body) = create(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreatePublicIpv4Pool, got {status} with body \
         {body}"
    );
    let pool_id = element(&body, "poolId").unwrap_or_default();
    assert!(
        is_pool_id(&pool_id),
        "case {case:?}: expected an ipv4pool-ec2- pool id, got {pool_id:?} in {body}"
    );
    pool_id
}

/// The pool ids a successful describe reports, in the order it reported them.
async fn described_ids(router: &MultiServiceRouter, case: &str, params: &[(String, String)]) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribePublicIpv4Pools, got {status} with body \
         {body}"
    );
    elements(&body, "poolId")
}

/// The form parameters for one `TagSpecification` over `tags`.
fn tag_specification(resource_type: &str, tags: &[(&str, &str)]) -> Params {
    let mut params = vec![p("TagSpecification.1.ResourceType", resource_type)];
    for (index, (key, value)) in tags.iter().enumerate() {
        params.push(p(&format!("TagSpecification.1.Tag.{}.Key", index + 1), *key));
        params.push(p(
            &format!("TagSpecification.1.Tag.{}.Value", index + 1),
            *value,
        ));
    }
    params
}

/// RULE 1: whatever the request carries, the pool's id is the service's, its border group is
/// one the Region advertises from, its tags are the request's, and it holds no address
/// range — as the describe reports the STORED record back.
#[tokio::test]
async fn create_public_ipv4_pool_persists_an_empty_pool_the_describe_reports() {
    // Rows differing along what a successful create can vary: how many tags it carries and
    // of what shape (none, one, three, a value with a space, a non-ASCII value), and which
    // border group it names — none at all, the Region's own, and one of the Region's Local
    // Zone groups.
    let rows: Vec<CreateRow> = vec![
        ("no parameters at all", Vec::new(), REGION, Vec::new()),
        (
            "the Region's own network border group",
            vec![p("NetworkBorderGroup", REGION)],
            REGION,
            Vec::new(),
        ),
        (
            "a Local Zone network border group of the Region",
            vec![p("NetworkBorderGroup", LOCAL_ZONE_GROUP)],
            LOCAL_ZONE_GROUP,
            Vec::new(),
        ),
        (
            "one tag",
            tag_specification(PUBLIC_IPV4_POOL_RESOURCE_TYPE, &[("Name", "byoip")]),
            REGION,
            vec![("Name", "byoip")],
        ),
        (
            "three tags, one value carrying a space",
            tag_specification(
                PUBLIC_IPV4_POOL_RESOURCE_TYPE,
                &[("Name", "byoip"), ("Owner", "Team A"), ("Tier", "edge")],
            ),
            REGION,
            vec![("Name", "byoip"), ("Owner", "Team A"), ("Tier", "edge")],
        ),
        (
            "a tag value outside ASCII, beside a border group",
            {
                let mut params = vec![p("NetworkBorderGroup", REGION)];
                params.extend(tag_specification(
                    PUBLIC_IPV4_POOL_RESOURCE_TYPE,
                    &[("Owner", "Équipe")],
                ));
                params
            },
            REGION,
            vec![("Owner", "Équipe")],
        ),
    ];

    for (case, params, expected_group, expected_tags) in rows {
        // One emulator per row: the rows are independent inputs, not a sequence, so "exactly
        // this pool is stored" is an assertion about the row.
        let router = emulator();
        let pool_id = create_one(&router, case, &params).await;

        // The round trip: the record a later caller sees is the pool this create minted.
        let (status, described) = describe(&router, &indexed("PoolId", &[&pool_id])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribePublicIpv4Pools, got {status} with \
             body {described}"
        );
        assert_eq!(
            elements(&described, "poolId"),
            vec![pool_id.clone()],
            "case {case:?}: expected exactly the created pool, got {described}"
        );
        assert_eq!(
            element(&described, "networkBorderGroup").as_deref(),
            Some(expected_group),
            "case {case:?}: expected the pool to be advertised from {expected_group}, got \
             {described}"
        );
        // A pool is the container for the CIDRs a caller brings to AWS, and this call
        // provisions none, so it holds no range and counts no address.
        for (member, expected) in [("totalAddressCount", "0"), ("totalAvailableAddressCount", "0")]
        {
            assert_eq!(
                element(&described, member).as_deref(),
                Some(expected),
                "case {case:?}: expected {member} {expected} on a freshly created pool, got \
                 {described}"
            );
        }
        // A pool created without a description holds the EMPTY one, which the describe
        // reports as a present element: `CreatePublicIpv4Pool` carries no description member
        // for a caller to set.
        assert_eq!(
            element(&described, "description").as_deref(),
            Some(""),
            "case {case:?}: expected an empty description on a freshly created pool, got \
             {described}"
        );
        for absent in ["firstAddress", "lastAddress", "addressCount"] {
            assert_eq!(
                element(&described, absent),
                None,
                "case {case:?}: expected no {absent} on a freshly created pool, got {described}"
            );
        }

        let reported_keys = elements(&described, "key");
        let expected_keys: Vec<String> = expected_tags
            .iter()
            .map(|(key, _)| (*key).to_string())
            .collect();
        assert_eq!(
            reported_keys, expected_keys,
            "case {case:?}: expected the stored tag keys, got {described}"
        );
        let reported_values = elements(&described, "value");
        let expected_values: Vec<String> = expected_tags
            .iter()
            .map(|(_, value)| (*value).to_string())
            .collect();
        assert_eq!(
            reported_values, expected_values,
            "case {case:?}: expected the stored tag values, got {described}"
        );

        // The tags are STORED, not echoed: a filter over them selects the pool, which it
        // could only do off the record the store holds.
        for (key, value) in &expected_tags {
            let selected = described_ids(&router, case, &filters(&[(&format!("tag:{key}"), &[value])])).await;
            assert_eq!(
                selected,
                vec![pool_id.clone()],
                "case {case:?}: expected the stored tag {key}={value} to select the pool, got \
                 {selected:?}"
            );
        }
    }
}

/// RULE 2, as an INVARIANT over a count: however many pools one account creates, each gets
/// its own id and the store holds every one of them.
#[tokio::test]
async fn every_create_mints_its_own_pool() {
    // A bounded number of creates, well above the single pool a recorded call makes and
    // above one page of the largest page size the model publishes (`PoolMaxResults` is
    // `range 1..10`), so both the mint and the paging over the result are exercised past
    // any single-row case.
    const POOLS: usize = 23;

    let router = emulator();
    let mut minted: Vec<String> = Vec::new();
    for nth in 0..POOLS {
        let case = format!("pool {nth}");
        let pool_id = create_one(
            &router,
            &case,
            &tag_specification(
                PUBLIC_IPV4_POOL_RESOURCE_TYPE,
                &[("Name", "byoip"), ("Index", &nth.to_string())],
            ),
        )
        .await;
        assert!(
            !minted.contains(&pool_id),
            "case {case:?}: pool id {pool_id} was minted twice"
        );
        minted.push(pool_id);
    }
    let mut expected = minted.clone();
    expected.sort();

    // Every pool the account created is in the store, reported in pool-id order over as many
    // pages as the page size needs. The loop is BOUNDED by the number of pages a page of 10
    // needs for `POOLS` pools, plus one: a describe that answered a token with no pool on it
    // must fail this test rather than spin in it.
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..=(POOLS / 10 + 1) {
        let mut params = vec![p("MaxResults", "10")];
        if let Some(next) = token.as_deref() {
            params.push(p("NextToken", next));
        }
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 200,
            "case \"every pool\": expected HTTP 200, got {status} with body {body}"
        );
        let reported = elements(&body, "poolId");
        assert!(
            !reported.is_empty(),
            "case \"every pool\": page {page} carried no pool, got {body}"
        );
        seen.extend(reported);
        token = element(&body, "nextToken");
        if token.is_none() {
            break;
        }
    }
    assert!(
        token.is_none(),
        "case \"every pool\": paging must end within the pages {POOLS} pools need, \
         got {seen:?}"
    );
    assert_eq!(
        seen, expected,
        "case \"every pool\": expected every created pool once, in id order, got {seen:?}"
    );
}

/// RULE 3: every create this service cannot satisfy is refused with EC2's own code, and a
/// refusal creates nothing.
#[tokio::test]
async fn create_public_ipv4_pool_refuses_what_it_cannot_satisfy() {
    // One tag past the documented quota, built from the quota the service publishes
    // (`crate::state::MAX_TAGS_PER_RESOURCE`) rather than from a number written here.
    let mut too_many_tags = vec![p(
        "TagSpecification.1.ResourceType",
        PUBLIC_IPV4_POOL_RESOURCE_TYPE,
    )];
    for nth in 0..=MAX_TAGS_PER_RESOURCE {
        too_many_tags.push(p(
            &format!("TagSpecification.1.Tag.{}.Key", nth + 1),
            format!("k{nth}"),
        ));
        too_many_tags.push(p(
            &format!("TagSpecification.1.Tag.{}.Value", nth + 1),
            format!("v{nth}"),
        ));
    }

    // Rows differing along WHICH rule the request breaks: the border group in each of its
    // wrong forms, the resource type a tag specification names, a missing resource type,
    // each tag rule, and the dry run in both of its readings.
    let rows: Vec<RefusalRow> = vec![
        (
            "another Region's network border group",
            vec![p("NetworkBorderGroup", "eu-central-1")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a Local Zone group of another Region",
            vec![p("NetworkBorderGroup", "us-east-1-dfw-2")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a Local Zone group this Region does not advertise from",
            vec![p("NetworkBorderGroup", "us-west-2-zzz-9")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a border group that is no location at all",
            vec![p("NetworkBorderGroup", "banana")],
            400,
            "InvalidParameterValue",
        ),
        (
            "tagging a resource this call does not create",
            tag_specification("elastic-ip", &[("Name", "nope")]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a tag specification naming no resource type",
            vec![
                p("TagSpecification.1.Tag.1.Key", "Name"),
                p("TagSpecification.1.Tag.1.Value", "byoip"),
            ],
            400,
            "MissingParameter",
        ),
        (
            "a tag key in the reserved namespace",
            tag_specification(
                PUBLIC_IPV4_POOL_RESOURCE_TYPE,
                &[("aws:created-by", "me")],
            ),
            400,
            "InvalidParameterValue",
        ),
        (
            "an empty tag key",
            tag_specification(PUBLIC_IPV4_POOL_RESOURCE_TYPE, &[("", "byoip")]),
            400,
            "InvalidParameterValue",
        ),
        (
            "one tag key given twice",
            tag_specification(
                PUBLIC_IPV4_POOL_RESOURCE_TYPE,
                &[("Name", "byoip"), ("Name", "again")],
            ),
            400,
            "InvalidParameterValue",
        ),
        (
            "one tag past the documented quota",
            too_many_tags,
            400,
            "TagLimitExceeded",
        ),
        (
            "a dry run that would have succeeded",
            vec![p("DryRun", "true")],
            412,
            "DryRunOperation",
        ),
        (
            "a dry run of a request that would have been refused",
            vec![p("DryRun", "true"), p("NetworkBorderGroup", "eu-central-1")],
            400,
            "InvalidParameterValue",
        ),
    ];

    for (case, params, expected_status, expected_code) in rows {
        let router = emulator();
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        // A refused create stores nothing: the account holds no pool at all.
        let reported = described_ids(&router, case, &[]).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: expected a refusal to create no pool, got {reported:?}"
        );
    }
}
