// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeletePublicIpv4Pool`, driven over the wire the service actually
//! speaks: `ec2Query`, an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=<Operation>`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//! Every delete is read back through `DescribePublicIpv4Pools`, whose own rules the sibling
//! `public_ipv4_pools` suite covers.
//!
//! The rules under test:
//!
//! 1. THE DELETE REMOVES EXACTLY THE POOL IT NAMES: "Delete a public IPv4 pool"
//!    (`DeletePublicIpv4Pool`) answers `ReturnValue` true, the id stops resolving for the
//!    describe, every other pool the account holds is untouched, and a second delete of the
//!    same id is a refusal — the store is what decides, not the call count.
//! 2. `NetworkBorderGroup` NARROWS THE LOOKUP: "The Availability Zone (AZ) or Local Zone
//!    (LZ) network border group that the resource ... is in"
//!    (`DeletePublicIpv4PoolRequest$NetworkBorderGroup`) — a request naming the group the
//!    pool is advertised from deletes it, a request naming another group finds no pool there
//!    and deletes nothing, and a request naming none deletes the pool wherever it is
//!    advertised from.
//! 3. WHAT IT REFUSES, with nothing deleted: a pool id the account holds no pool for is HTTP
//!    400 `InvalidPublicIpv4Pool.NotFound` naming the id the request sent, a `PoolId` that
//!    arrived empty is `MissingParameter` (and one the request left out entirely is refused
//!    by the request walk before the handler runs), a pool that still holds an address range
//!    is `DependencyViolation` (the range leaves through `DeprovisionPublicIpv4PoolCidr`
//!    first), and a `DryRun` is answered from the permission check alone (HTTP 412
//!    `DryRunOperation`).
//!
//! Each table row is a distinct input along what those rules generalise over — how many
//! pools the account holds and which of them is deleted, which network border group the pool
//! is advertised from and which the request names, how many ranges a pool holds and how
//! large they are, and which rule a refused request breaks — so no single recorded call is
//! the specification.
//!
//! A pool holding address ranges is seeded straight into the store through the store's own
//! insert, the way the sibling `public_ipv4_pools` suite seeds its multi-range rows: one
//! BYOIP provision creates a pool with one range and `ProvisionPublicIpv4PoolCidr` is still a
//! scaffold, so a pool holding two ranges has no API path. The ranges seeded are the blocks
//! RFC 5737 (`192.0.2.0/24`, `198.51.100.0/24`, `203.0.113.0/24`) and RFC 2544
//! (`198.18.0.0/15`) set aside for documentation and benchmarking, so no row names address
//! space that belongs to anybody.
//!
//! Each refusal's MESSAGE is checked only for the substitution the service makes — the value
//! the request sent, named back — never for wording this emulator has no rule for.

use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, PublicIpv4PoolSpec};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, indexed, unescaped};

/// The Region every call here is signed for, and so the network border group a pool created
/// without one is advertised from.
const REGION: &str = "us-west-2";

/// A Local Zone network border group of that Region — the other reading of "The Availability
/// Zone (AZ) or Local Zone (LZ) network border group", taken from the Local Zone groups
/// `ec2::state::LOCAL_ZONE_NETWORK_BORDER_GROUPS` lists for this Region.
const LOCAL_ZONE_GROUP: &str = "us-west-2-lax-1";

/// EC2's error code for a public IPv4 pool id the account holds no pool for, as a request
/// that ACTS on the pool is refused with.
const NOT_FOUND: &str = "InvalidPublicIpv4Pool.NotFound";

/// The code the READ of the pools answers for that same absent pool: the two paths of this
/// resource carry different codes for it, so a delete read back through
/// `DescribePublicIpv4Pools` is refused under this one.
const READ_NOT_FOUND: &str = "InvalidPublicIpv4PoolID.NotFound";

/// EC2's error code for a member the operation needs that reached the handler empty.
const MISSING_PARAMETER: &str = "MissingParameter";

/// The code the request walk answers a REQUIRED member the request left out entirely with,
/// before any handler of this service runs — a different failure from a member that arrived
/// present but empty, which is the handler's own `MissingParameter`.
const MALFORMED_INPUT: &str = "MalformedInput";

/// EC2's error code for a delete the resource's own dependencies forbid.
const DEPENDENCY_VIOLATION: &str = "DependencyViolation";

/// EC2's error code for a `DryRun` request that would otherwise have been served.
const DRY_RUN_OPERATION: &str = "DryRunOperation";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// The emulator the tests drive, with the engine the seeded rows write through.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One `DeletePublicIpv4Pool` call carrying `params`.
async fn delete(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DeletePublicIpv4Pool", params).await
}

/// One `CreatePublicIpv4Pool` call carrying `params`.
async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "CreatePublicIpv4Pool", params).await
}

/// One `DescribePublicIpv4Pools` call carrying `params`.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribePublicIpv4Pools", params).await
}

/// One `DeprovisionPublicIpv4PoolCidr` call for `cidr` out of `pool_id`.
async fn deprovision(router: &MultiServiceRouter, pool_id: &str, cidr: &str) -> (u16, String) {
    call(
        router,
        REGION,
        "DeprovisionPublicIpv4PoolCidr",
        &[p("PoolId", pool_id), p("Cidr", cidr)],
    )
    .await
}

/// Create one pool advertised from `border_group` (the Region's own group when `None`) and
/// answer the id the SERVICE minted for it.
async fn create_one(
    router: &MultiServiceRouter,
    case: &str,
    border_group: Option<&str>,
) -> String {
    let params: Params = border_group
        .map(|group| vec![p("NetworkBorderGroup", group)])
        .unwrap_or_default();
    let (status, body) = create(router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreatePublicIpv4Pool, got {status} with body \
         {body}"
    );
    let pool_id = element(&body, "poolId").unwrap_or_default();
    assert!(
        pool_id.starts_with("ipv4pool-ec2-"),
        "case {case:?}: expected a minted public IPv4 pool id, got {pool_id:?} in {body}"
    );
    pool_id
}

/// Write one pool holding `cidrs`, advertised from `border_group`, straight into the store
/// and answer the id minted for it.
///
/// The store's own insert is what mints the id, canonicalises the ranges and charges the
/// bytes, so a seeded pool is a pool exactly as a provision would have created it.
fn seed_pool(
    engine: &engine::InMemoryEngine,
    cidrs: &[&str],
    border_group: &str,
) -> String {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    guard
        .insert_public_ipv4_pool(PublicIpv4PoolSpec {
            description: None,
            network_border_group: border_group.to_string(),
            tags: Vec::new(),
            cidrs: cidrs.iter().map(|cidr| (*cidr).to_string()).collect(),
        })
        .expect("the store has room for the seeded pool")
}

/// Whether the store still holds a pool under `pool_id`, read straight from the store so it
/// reports what a later caller would be told without a call of its own.
fn stored(engine: &engine::InMemoryEngine, pool_id: &str) -> bool {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard.public_ipv4_pools.contains_key(pool_id)
}

/// The pool ids a successful describe of the whole account reports, in the order it reported
/// them.
async fn described_ids(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, body) = describe(router, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribePublicIpv4Pools, got {status} with body \
         {body}"
    );
    elements(&body, "poolId")
}

/// Assert that `pool_id` no longer resolves for the describe that reads it by id.
async fn assert_gone(router: &MultiServiceRouter, case: &str, pool_id: &str) {
    let (status, body) = describe(router, &indexed("PoolId", &[pool_id])).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a deleted pool id to stop resolving, got {status} with body \
         {body}"
    );
    assert_eq!(
        error_code(&body),
        READ_NOT_FOUND,
        "case {case:?}: expected {READ_NOT_FOUND} for a deleted pool, got {body}"
    );
}

/// One successful-delete row: `(case, how many pools the account holds, the index of the one
/// deleted, the border group the request names)`.
type DeleteRow = (&'static str, usize, usize, Option<&'static str>);

/// RULE 1 and RULE 2: the named pool is the pool that goes, whatever else the account holds
/// and whichever reading of the border group member the request takes.
#[tokio::test]
async fn delete_public_ipv4_pool_removes_exactly_the_pool_it_names() {
    // Rows differing along how many pools the account holds, WHICH of them is deleted (the
    // only one, the first, a middle one, the last), and what the request says about the
    // network border group: nothing, the Region's own group, and a Local Zone group of the
    // Region the pool is advertised from.
    let rows: Vec<DeleteRow> = vec![
        ("the only pool, no border group named", 1, 0, None),
        ("the first of three", 3, 0, None),
        ("a middle one of five", 5, 2, None),
        ("the last of four", 4, 3, None),
        ("the Region's own border group named", 2, 1, Some(REGION)),
    ];

    for (case, pools, victim, named_group) in rows {
        // One emulator per row: the rows are independent inputs, not a sequence, so "exactly
        // this pool went" is an assertion about the row.
        let (router, engine) = emulator();
        let mut minted: Vec<String> = Vec::new();
        for nth in 0..pools {
            minted.push(create_one(&router, &format!("{case} / pool {nth}"), None).await);
        }
        let pool_id = minted[victim].clone();

        let mut params = vec![p("PoolId", pool_id.as_str())];
        if let Some(group) = named_group {
            params.push(p("NetworkBorderGroup", group));
        }
        let (status, body) = delete(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeletePublicIpv4Pool, got {status} with \
             body {body}"
        );
        assert_eq!(
            element(&body, "returnValue").as_deref(),
            Some("true"),
            "case {case:?}: expected the delete to report its result, got {body}"
        );

        // The round trip: the id stops resolving, the store no longer holds the record, and
        // every other pool the account created is still there, in id order.
        assert_gone(&router, case, &pool_id).await;
        assert!(
            !stored(&engine, &pool_id),
            "case {case:?}: the store must not hold the deleted pool {pool_id}"
        );
        let mut expected: Vec<String> = minted
            .iter()
            .filter(|held| *held != &pool_id)
            .cloned()
            .collect();
        expected.sort();
        let reported = described_ids(&router, case).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected every other pool to survive the delete, got {reported:?}"
        );

        // RULE 1, the second half: the id is gone, so deleting it again is refused rather
        // than answered as a second success.
        let (status, body) = delete(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a second delete of {pool_id} to be refused, got {status} \
             with body {body}"
        );
        assert_eq!(
            error_code(&body),
            NOT_FOUND,
            "case {case:?}: expected {NOT_FOUND} on the second delete, got {body}"
        );
    }
}

/// RULE 2: the border group the request names decides whether the pool is found at all — a
/// pool advertised from a Local Zone group is deleted when that group is named, and left
/// alone when another group is.
#[tokio::test]
async fn the_network_border_group_narrows_which_pool_the_delete_finds() {
    // Rows differing along WHERE the pool is advertised from and WHAT the request names:
    // every pairing of the Region's own group and one of its Local Zone groups, plus the
    // request that names neither.
    let rows: Vec<(&str, &str, Option<&str>, bool)> = vec![
        ("Region pool, no group named", REGION, None, true),
        ("Region pool, its own group named", REGION, Some(REGION), true),
        (
            "Region pool, a Local Zone group named",
            REGION,
            Some(LOCAL_ZONE_GROUP),
            false,
        ),
        (
            "Local Zone pool, no group named",
            LOCAL_ZONE_GROUP,
            None,
            true,
        ),
        (
            "Local Zone pool, its own group named",
            LOCAL_ZONE_GROUP,
            Some(LOCAL_ZONE_GROUP),
            true,
        ),
        (
            "Local Zone pool, the Region's group named",
            LOCAL_ZONE_GROUP,
            Some(REGION),
            false,
        ),
        (
            "Local Zone pool, another Region's group named",
            LOCAL_ZONE_GROUP,
            Some("us-east-1"),
            false,
        ),
    ];

    for (case, advertised_from, named_group, expect_deleted) in rows {
        let (router, engine) = emulator();
        let pool_id = create_one(&router, case, Some(advertised_from)).await;

        let mut params = vec![p("PoolId", pool_id.as_str())];
        if let Some(group) = named_group {
            params.push(p("NetworkBorderGroup", group));
        }
        let (status, body) = delete(&router, &params).await;
        if expect_deleted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from DeletePublicIpv4Pool, got {status} with \
                 body {body}"
            );
            assert!(
                !stored(&engine, &pool_id),
                "case {case:?}: the store must not hold the deleted pool {pool_id}"
            );
            assert_gone(&router, case, &pool_id).await;
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected a pool advertised from {advertised_from} not to be \
                 found under {named_group:?}, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                NOT_FOUND,
                "case {case:?}: expected {NOT_FOUND}, got {body}"
            );
            assert!(
                stored(&engine, &pool_id),
                "case {case:?}: a refused delete must leave the pool {pool_id} in the store"
            );
            let reported = described_ids(&router, case).await;
            assert_eq!(
                reported,
                vec![pool_id.clone()],
                "case {case:?}: expected the pool to survive, got {reported:?}"
            );
        }
    }
}

/// RULE 3: a pool that still holds an address range is refused until the range leaves it, and
/// the delete succeeds once `DeprovisionPublicIpv4PoolCidr` has taken every range out.
#[tokio::test]
async fn a_pool_that_still_holds_a_range_is_refused_until_the_range_leaves() {
    // Rows differing along how many ranges a pool holds and how large they are: one most
    // specific block, one much larger block, and three blocks at once — so the refusal is not
    // keyed to a single range.
    let rows: Vec<(&str, Vec<&str>)> = vec![
        ("one /24", vec!["192.0.2.0/24"]),
        ("one /15", vec!["198.18.0.0/15"]),
        (
            "three ranges",
            vec!["192.0.2.0/24", "198.51.100.0/24", "203.0.113.0/24"],
        ),
    ];

    for (case, cidrs) in rows {
        let (router, engine) = emulator();
        let pool_id = seed_pool(&engine, &cidrs, REGION);

        // While the pool holds a range, the delete is refused and the pool stays.
        for (nth, cidr) in cidrs.iter().enumerate() {
            let (status, body) = delete(&router, &[p("PoolId", pool_id.as_str())]).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected a pool holding {} range(s) to be refused, got {status} \
                 with body {body}",
                cidrs.len() - nth
            );
            assert_eq!(
                error_code(&body),
                DEPENDENCY_VIOLATION,
                "case {case:?}: expected {DEPENDENCY_VIOLATION} while a range is provisioned, \
                 got {body}"
            );
            let message = unescaped(&element(&body, "Message").unwrap_or_default());
            assert!(
                message.contains(&pool_id),
                "case {case:?}: the refusal names the pool the request sent, got {message:?}"
            );
            assert!(
                stored(&engine, &pool_id),
                "case {case:?}: a refused delete must leave the pool {pool_id} in the store"
            );

            // One range leaves through the operation that owns that move.
            let (status, body) = deprovision(&router, &pool_id, cidr).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from DeprovisionPublicIpv4PoolCidr for \
                 {cidr}, got {status} with body {body}"
            );
        }

        // The pool holds nothing now, so the delete is served and the id stops resolving.
        let (status, body) = delete(&router, &[p("PoolId", pool_id.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 once every range has left, got {status} with body \
             {body}"
        );
        assert_eq!(
            element(&body, "returnValue").as_deref(),
            Some("true"),
            "case {case:?}: expected the delete to report its result, got {body}"
        );
        assert!(
            !stored(&engine, &pool_id),
            "case {case:?}: the store must not hold the deleted pool {pool_id}"
        );
        assert_gone(&router, case, &pool_id).await;
    }
}

/// RULE 3: every delete this service cannot serve is refused with EC2's own code, and a
/// refusal deletes nothing.
#[tokio::test]
async fn delete_public_ipv4_pool_refuses_what_it_cannot_serve() {
    // Rows differing along WHICH rule the request breaks: a well-formed pool id of each
    // published id length that names no pool, a value that is no pool id at all, the absent
    // and the empty `PoolId`, and the dry run in both of its readings. The `{pool}` in a row's
    // parameters is replaced by the id of the pool the account really holds, so a row can
    // name it without knowing the minted value.
    let rows: Vec<(&str, Params, u16, &str)> = vec![
        (
            "a 17-digit pool id naming no pool",
            vec![p("PoolId", "ipv4pool-ec2-00000000000000000")],
            400,
            NOT_FOUND,
        ),
        (
            "an 8-digit pool id naming no pool",
            vec![p("PoolId", "ipv4pool-ec2-0abcdef1")],
            400,
            NOT_FOUND,
        ),
        (
            "a value that is no pool id at all",
            vec![p("PoolId", "not-a-pool")],
            400,
            NOT_FOUND,
        ),
        ("no PoolId at all", Vec::new(), 400, MALFORMED_INPUT),
        (
            "an empty PoolId",
            vec![p("PoolId", "")],
            400,
            MISSING_PARAMETER,
        ),
        (
            "a dry run that would have succeeded",
            vec![p("PoolId", "{pool}"), p("DryRun", "true")],
            412,
            DRY_RUN_OPERATION,
        ),
        (
            "a dry run of a pool the account does not hold",
            vec![
                p("PoolId", "ipv4pool-ec2-00000000000000000"),
                p("DryRun", "true"),
            ],
            412,
            DRY_RUN_OPERATION,
        ),
    ];

    for (case, params, expected_status, expected_code) in rows {
        let (router, engine) = emulator();
        // One pool the account really holds, so "a refusal deletes nothing" has something to
        // be true of.
        let pool_id = create_one(&router, case, None).await;
        let params: Params = params
            .into_iter()
            .map(|(key, value)| {
                if value == "{pool}" {
                    (key, pool_id.clone())
                } else {
                    (key, value)
                }
            })
            .collect();

        let (status, body) = delete(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        if expected_code == NOT_FOUND {
            let sent = params
                .iter()
                .find(|(key, _)| key == "PoolId")
                .map(|(_, value)| value.clone())
                .unwrap_or_default();
            let message = unescaped(&element(&body, "Message").unwrap_or_default());
            assert!(
                message.contains(&sent),
                "case {case:?}: the refusal names the id the request sent, got {message:?}"
            );
        }
        // Nothing was deleted: the account still holds exactly the pool it created.
        assert!(
            stored(&engine, &pool_id),
            "case {case:?}: a refused delete must leave the pool {pool_id} in the store"
        );
        let reported = described_ids(&router, case).await;
        assert_eq!(
            reported,
            vec![pool_id.clone()],
            "case {case:?}: expected the refusal to delete no pool, got {reported:?}"
        );
    }
}
