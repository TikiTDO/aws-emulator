// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateIpamPool`, driven over `ec2Query` (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rules under test:
//!
//! * a create MINTS the resource in the container the request names and stores it, taking
//!   the container's own facts (here: the IPAM, its Region and the scope type) from the
//!   stored container rather than from the request, and deriving everything else the
//!   service owns — the id, the ARN, the owning account, the resource's depth in its tree,
//!   and the non-terminal state the clock then owes the terminal one;
//! * a container id the account holds no container for is refused with that resource's own
//!   `NotFound` code, and something that is not an id of that kind at all with its
//!   `Malformed` code, whatever else the request carries;
//! * a member the documented rules of the resource do not admit — a netmask outside the
//!   address family's range, a minimum at or above the maximum, a default outside them, an
//!   advertisement flag on a family that has none, a public IP source in the private scope,
//!   a locale that is no Region — is refused and NOTHING is stored;
//! * one `ClientToken` names the one pool it minted: a replay answers that pool, and a
//!   replay asking for anything else is refused.
//!
//! Each table row is a distinct input along what those generalise over: which address
//! family the pool holds, which scope it is created in, which optional member the request
//! carries, which member is at fault, and how deep in a tree of pools it sits.

use std::time::Duration;

use ec2::ops::types::{AsnAssociation, ByoipCidr, IpamScope};
use ec2::state::{
    ADDRESS_FAMILY_IPV4, ADDRESS_FAMILY_IPV6, BYOIP_CIDR_STATE_PROVISIONED, ByoipCidrRecord,
    ComAmazonawsEc2Amazonec2State as Ec2State, IPAM_POOL_ID_PREFIX,
    IPAM_POOL_LOCALE_NONE, IPAM_POOL_PUBLIC_IP_SOURCE_BYOIP, IPAM_POOL_STATE_CREATE_COMPLETE,
    IPAM_POOL_STATE_CREATE_IN_PROGRESS, IPAM_SCOPE_TYPE_PRIVATE, IPAM_SCOPE_TYPE_PUBLIC,
    INVALID_IPAM_POOL_ID_MALFORMED, INVALID_IPAM_POOL_ID_NOT_FOUND,
    INVALID_IPAM_SCOPE_ID_MALFORMED, INVALID_IPAM_SCOPE_ID_NOT_FOUND, INVALID_PARAMETER_VALUE,
    RESOURCE_TYPE_IPAM_POOL, TOP_LEVEL_IPAM_POOL_DEPTH, TRANSITION_DELAY, canonical_byoip_cidr,
    ipam_resource_arn,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code};

/// The Region every call here is signed for, and the Region the seeded IPAM is in.
const REGION: &str = "us-east-1";

/// Long enough past [`TRANSITION_DELAY`] that a move the clock owes is certainly due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// One call of `action` signed for the Region above.
async fn invoke(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// One `(name, value)` request member.
fn p(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// The IPAM a suite seeds: the ARN of the IPAM itself and the ids of its two default
/// scopes, which is what a pool is created in.
struct SeededIpam {
    /// "The ARN of the IPAM" (`IpamScope$IpamArn`), which every pool in either scope
    /// reports.
    ipam_arn: String,
    /// The IPAM's default public scope.
    public_scope: String,
    /// The IPAM's default private scope.
    private_scope: String,
}

/// Seed one IPAM's two default scopes — "an IPAM contains two default scopes"
/// (`IpamScope`) — and answer what a request names them by.
///
/// `CreateIpam` and `CreateIpamScope` are still scaffolds, so the scopes are seeded through
/// the store's own writer the way every other suite in this crate seeds a resource whose
/// create is unwritten; that writer mints the scope id and ARN, so the ids here are the
/// ones a real create would have handed out.
fn seed_ipam(engine: &engine::InMemoryEngine) -> SeededIpam {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let ipam_id = guard.next_id("ipam");
    let ipam_arn = engine::emulator_arn("ec2", REGION, &format!("ipam/{ipam_id}"));
    let mut scope = |scope_type: &str| -> String {
        guard
            .insert_ipam_scope(IpamScope {
                ipam_arn: Some(ipam_arn.clone()),
                ipam_region: Some(REGION.to_string()),
                ipam_scope_type: Some(scope_type.to_string()),
                is_default: Some(true),
                // The `IpamScopeState` enum's settled member.
                state: Some("create-complete".to_string()),
                ..Default::default()
            })
            .expect("the store holds the seeded IPAM scope")
    };
    let public_scope = scope(IPAM_SCOPE_TYPE_PUBLIC);
    let private_scope = scope(IPAM_SCOPE_TYPE_PRIVATE);
    SeededIpam {
        ipam_arn,
        public_scope,
        private_scope,
    }
}

/// Create one pool of `family` in `scope_id` and answer the `IpamPoolId` the service
/// minted for it.
async fn create_pool(router: &MultiServiceRouter, scope_id: &str, family: &str) -> String {
    let (status, body) = invoke(
        router,
        "CreateIpamPool",
        &[p("IpamScopeId", scope_id), p("AddressFamily", family)],
    )
    .await;
    assert_eq!(
        status, 200,
        "creating an {family} pool in {scope_id}: got {status} with body {body}"
    );
    element(&body, "ipamPoolId")
        .unwrap_or_else(|| panic!("the created pool reports an id, got {body}"))
}

/// The ARN the store gave the seeded scope `scope_id`.
fn scope_arn(engine: &engine::InMemoryEngine, scope_id: &str) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .ipam_scopes
        .get(scope_id)
        .and_then(|scope| scope.ipam_scope_arn.clone())
        .unwrap_or_else(|| panic!("the seeded scope {scope_id} carries an ARN"))
}

/// How many pools the store holds, and what the scope says it holds.
fn pool_counts(engine: &engine::InMemoryEngine, scope_id: &str) -> (usize, i32) {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    let stored = guard.ipam_pools.len();
    let reported = guard
        .ipam_scopes
        .get(scope_id)
        .and_then(|scope| scope.pool_count)
        .unwrap_or(-1);
    (stored, reported)
}

// ---------------------------------------------------------------------------
// What a created pool reports
// ---------------------------------------------------------------------------

/// One row of the create table: a request, and what the pool it mints must report.
struct Created {
    /// What makes this row different from the ones beside it.
    case: &'static str,
    /// Which of the IPAM's two default scopes the pool is created in.
    public: bool,
    /// The `AddressFamily` the pool holds.
    family: &'static str,
    /// The request members beyond the scope and the family.
    extra: Vec<(String, String)>,
    /// `(element, text)` the answer must carry.
    expected: Vec<(&'static str, String)>,
    /// Elements the answer must NOT carry at all.
    absent: Vec<&'static str>,
}

/// A created pool reports what the request asked for and what the scope it was created in
/// decides, with every member the service owns derived rather than echoed.
#[tokio::test]
async fn a_created_pool_reports_the_request_and_its_scope() {
    let owned = |value: &str| value.to_string();
    let cases: Vec<Created> = vec![
        Created {
            case: "a top-level IPv4 pool in the private scope, with no optional member",
            public: false,
            family: ADDRESS_FAMILY_IPV4,
            extra: Vec::new(),
            expected: vec![
                ("addressFamily", owned(ADDRESS_FAMILY_IPV4)),
                ("ipamScopeType", owned(IPAM_SCOPE_TYPE_PRIVATE)),
                ("state", owned(IPAM_POOL_STATE_CREATE_IN_PROGRESS)),
                ("poolDepth", TOP_LEVEL_IPAM_POOL_DEPTH.to_string()),
                ("autoImport", owned("false")),
                // Nothing named a locale, which the pool reports as its default spelled
                // out rather than by leaving the member out.
                ("locale", owned(IPAM_POOL_LOCALE_NONE)),
            ],
            // A private-scope pool has no public IP source, an IPv4 pool no
            // advertisement flag, and nothing named a description.
            absent: vec!["publicIpSource", "publiclyAdvertisable", "description"],
        },
        Created {
            case: "an IPv4 pool in the public scope carrying every allocation netmask rule",
            public: true,
            family: ADDRESS_FAMILY_IPV4,
            extra: vec![
                p("Locale", REGION),
                p("Description", "production space"),
                p("AutoImport", "true"),
                p("AllocationMinNetmaskLength", "16"),
                p("AllocationMaxNetmaskLength", "28"),
                p("AllocationDefaultNetmaskLength", "24"),
            ],
            expected: vec![
                ("addressFamily", owned(ADDRESS_FAMILY_IPV4)),
                ("ipamScopeType", owned(IPAM_SCOPE_TYPE_PUBLIC)),
                ("locale", owned(REGION)),
                ("description", owned("production space")),
                ("autoImport", owned("true")),
                ("allocationMinNetmaskLength", owned("16")),
                ("allocationMaxNetmaskLength", owned("28")),
                ("allocationDefaultNetmaskLength", owned("24")),
                // "Default is byoip" for a pool in the public scope.
                ("publicIpSource", owned(IPAM_POOL_PUBLIC_IP_SOURCE_BYOIP)),
            ],
            absent: vec!["publiclyAdvertisable"],
        },
        Created {
            case: "an advertisable IPv6 BYOIP pool in the public scope",
            public: true,
            family: ADDRESS_FAMILY_IPV6,
            extra: vec![
                p("PublicIpSource", IPAM_POOL_PUBLIC_IP_SOURCE_BYOIP),
                p("PubliclyAdvertisable", "true"),
                p("AwsService", "ec2"),
                p("AllocationMinNetmaskLength", "48"),
                p("AllocationMaxNetmaskLength", "64"),
            ],
            expected: vec![
                ("addressFamily", owned(ADDRESS_FAMILY_IPV6)),
                ("publiclyAdvertisable", owned("true")),
                ("publicIpSource", owned(IPAM_POOL_PUBLIC_IP_SOURCE_BYOIP)),
                ("awsService", owned("ec2")),
                ("allocationMinNetmaskLength", owned("48")),
                ("allocationMaxNetmaskLength", owned("64")),
                ("locale", owned(IPAM_POOL_LOCALE_NONE)),
            ],
            absent: Vec::new(),
        },
        Created {
            case: "an Amazon-sourced IPv6 pool in the public scope",
            public: true,
            family: ADDRESS_FAMILY_IPV6,
            // The other member of the `IpamPoolPublicIpSource` catalog, which admits no
            // advertisement flag.
            extra: vec![p("PublicIpSource", "amazon")],
            expected: vec![
                ("addressFamily", owned(ADDRESS_FAMILY_IPV6)),
                ("publicIpSource", owned("amazon")),
            ],
            absent: vec!["publiclyAdvertisable"],
        },
        Created {
            case: "a pool whose allocations are required to carry two resource tags",
            public: false,
            family: ADDRESS_FAMILY_IPV4,
            extra: vec![
                p("AllocationResourceTag.1.Key", "Owner"),
                p("AllocationResourceTag.1.Value", "TeamA"),
                p("AllocationResourceTag.2.Key", "Stage"),
                p("AllocationResourceTag.2.Value", "prod"),
            ],
            expected: vec![("addressFamily", owned(ADDRESS_FAMILY_IPV4))],
            absent: vec!["publicIpSource"],
        },
        Created {
            case: "a pool tagged through TagSpecifications",
            public: false,
            family: ADDRESS_FAMILY_IPV4,
            extra: vec![
                p("TagSpecification.1.ResourceType", "ipam-pool"),
                p("TagSpecification.1.Tag.1.Key", "Name"),
                p("TagSpecification.1.Tag.1.Value", "development"),
            ],
            expected: vec![("addressFamily", owned(ADDRESS_FAMILY_IPV4))],
            absent: vec!["publicIpSource"],
        },
    ];

    for row in cases {
        let case = row.case;
        // Each row runs against its own freshly seeded IPAM, so one row's pool cannot
        // change what another row's scope reports.
        let (router, engine) = super::ec2_query::emulator();
        let ipam = seed_ipam(&engine);
        let scope_id = if row.public {
            ipam.public_scope.clone()
        } else {
            ipam.private_scope.clone()
        };
        let expected_scope_arn = scope_arn(&engine, &scope_id);

        let mut params = vec![p("IpamScopeId", &scope_id), p("AddressFamily", row.family)];
        params.extend(row.extra.clone());
        let (status, body) = invoke(&router, "CreateIpamPool", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );

        let pool_id = element(&body, "ipamPoolId").unwrap_or_default();
        assert!(
            pool_id.starts_with(&format!("{IPAM_POOL_ID_PREFIX}-")),
            "case {case:?}: expected an {IPAM_POOL_ID_PREFIX} id, got {pool_id:?} in {body}"
        );
        // The members the SERVICE owns: the ARN carries the minted id, the pool names the
        // scope it was created in and that scope's IPAM and Region, and the owner is this
        // emulator's account. An IPAM-family ARN carries an EMPTY Region segment — the
        // pool's Region is reported in `IpamPool$IpamRegion` — so the expected ARN is spelled
        // out here, independently of the writer that builds it, with the account segment
        // taken from the emulator rather than written as a literal.
        let expected_pool_arn = format!(
            "arn:aws:ec2::{account}:{RESOURCE_TYPE_IPAM_POOL}/{pool_id}",
            account = engine::emulator_account_id(),
        );
        assert_eq!(
            element(&body, "ipamPoolArn").as_deref(),
            Some(expected_pool_arn.as_str()),
            "case {case:?}: expected the pool's ARN to name {pool_id}, got {body}"
        );
        assert_eq!(
            element(&body, "ipamPoolArn").as_deref(),
            Some(ipam_resource_arn(RESOURCE_TYPE_IPAM_POOL, &pool_id).as_str()),
            "case {case:?}: expected the ARN the one writer of an IPAM ARN builds, got {body}"
        );
        assert_eq!(
            element(&body, "ipamScopeArn").as_deref(),
            Some(expected_scope_arn.as_str()),
            "case {case:?}: expected the scope it was created in, got {body}"
        );
        assert_eq!(
            element(&body, "ipamArn").as_deref(),
            Some(ipam.ipam_arn.as_str()),
            "case {case:?}: expected the scope's own IPAM, got {body}"
        );
        assert_eq!(
            element(&body, "ipamRegion").as_deref(),
            Some(REGION),
            "case {case:?}: expected the IPAM's Region, got {body}"
        );
        assert_eq!(
            element(&body, "ownerId").as_deref(),
            Some(engine::emulator_account_id()),
            "case {case:?}: expected this emulator's account, got {body}"
        );

        for (name, expected) in &row.expected {
            assert_eq!(
                element(&body, name).as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected <{name}> to be {expected:?}, got {body}"
            );
        }
        for name in &row.absent {
            assert!(
                element(&body, name).is_none(),
                "case {case:?}: expected no <{name}> at all, got {body}"
            );
        }

        // The pool was STORED, not just answered: the store holds it under the id the
        // answer reported, and the scope it was created in now counts it.
        let (stored, reported) = pool_counts(&engine, &scope_id);
        assert_eq!(
            stored, 1,
            "case {case:?}: expected the store to hold the one created pool, holds {stored}"
        );
        assert_eq!(
            reported, 1,
            "case {case:?}: expected the scope to report one pool, reports {reported}"
        );

        // The pool is answered in a non-terminal state, and the clock owes it the terminal
        // one whether or not anybody calls again.
        tokio::time::sleep(past_the_transition()).await;
        let settled = {
            let store = engine.state::<Ec2State>();
            let mut guard = store.lock();
            guard.materialize_due_transitions();
            guard
                .ipam_pools
                .get(&pool_id)
                .and_then(|pool| pool.state.clone())
                .unwrap_or_default()
        };
        assert_eq!(
            settled, IPAM_POOL_STATE_CREATE_COMPLETE,
            "case {case:?}: the clock owed the pool its {IPAM_POOL_STATE_CREATE_COMPLETE} state"
        );
    }
}

/// The tags and the allocation resource tags a request carries round-trip onto the pool.
#[tokio::test]
async fn a_pools_tags_and_allocation_resource_tags_round_trip() {
    let (router, engine) = super::ec2_query::emulator();
    let ipam = seed_ipam(&engine);

    let (status, body) = invoke(
        &router,
        "CreateIpamPool",
        &[
            p("IpamScopeId", &ipam.private_scope),
            p("AddressFamily", ADDRESS_FAMILY_IPV4),
            p("AllocationResourceTag.1.Key", "Owner"),
            p("AllocationResourceTag.1.Value", "TeamA"),
            p("AllocationResourceTag.2.Key", "Stage"),
            p("AllocationResourceTag.2.Value", "prod"),
        ],
    )
    .await;
    assert_eq!(status, 200, "allocation tags: got {status} with body {body}");
    assert_eq!(
        elements(&body, "key"),
        vec!["Owner".to_string(), "Stage".to_string()],
        "allocation tags: expected both required keys, got {body}"
    );
    assert_eq!(
        elements(&body, "value"),
        vec!["TeamA".to_string(), "prod".to_string()],
        "allocation tags: expected both required values, got {body}"
    );

    let (status, body) = invoke(
        &router,
        "CreateIpamPool",
        &[
            p("IpamScopeId", &ipam.private_scope),
            p("AddressFamily", ADDRESS_FAMILY_IPV4),
            p("TagSpecification.1.ResourceType", "ipam-pool"),
            p("TagSpecification.1.Tag.1.Key", "Name"),
            p("TagSpecification.1.Tag.1.Value", "development"),
        ],
    )
    .await;
    assert_eq!(status, 200, "pool tags: got {status} with body {body}");
    assert_eq!(
        elements(&body, "key"),
        vec!["Name".to_string()],
        "pool tags: expected the tag the request carried, got {body}"
    );
    assert_eq!(
        elements(&body, "value"),
        vec!["development".to_string()],
        "pool tags: expected the tag the request carried, got {body}"
    );
}

// ---------------------------------------------------------------------------
// What a create refuses
// ---------------------------------------------------------------------------

/// One row of the refusal table: what makes the row different from the ones beside it, the
/// request it sends, and the HTTP status and error code the service must refuse it with.
type Refusal = (&'static str, Vec<(String, String)>, u16, &'static str);

/// Every member of the request is checked against the store and against the pool's own
/// documented rules, and a refused create stores nothing.
#[tokio::test]
async fn a_create_is_refused_by_the_scope_and_the_pools_own_rules() {
    let (router, engine) = super::ec2_query::emulator();
    let ipam = seed_ipam(&engine);

    // A pool of each family in each scope, to be named as a SOURCE pool below.
    let private_ipv4 = create_pool(&router, &ipam.private_scope, ADDRESS_FAMILY_IPV4).await;
    let private_ipv6 = create_pool(&router, &ipam.private_scope, ADDRESS_FAMILY_IPV6).await;
    let public_ipv4 = create_pool(&router, &ipam.public_scope, ADDRESS_FAMILY_IPV4).await;
    let seeded_pools = engine.state::<Ec2State>().lock().ipam_pools.len();

    // Rows: one value of the request each rule refuses. Every id below is well formed, so
    // a `NotFound` row is about the STORE and a `Malformed` row about the request's form.
    let refusals: Vec<Refusal> = vec![
        (
            "a well-formed scope id nothing created (long id form)",
            vec![
                p("IpamScopeId", "ipam-scope-0000000000000000f"),
                p("AddressFamily", ADDRESS_FAMILY_IPV4),
            ],
            400,
            INVALID_IPAM_SCOPE_ID_NOT_FOUND,
        ),
        (
            "a well-formed scope id nothing created (short id form)",
            vec![
                p("IpamScopeId", "ipam-scope-12ab34cd"),
                p("AddressFamily", ADDRESS_FAMILY_IPV6),
            ],
            400,
            INVALID_IPAM_SCOPE_ID_NOT_FOUND,
        ),
        (
            "a scope id that is not an id at all",
            vec![
                p("IpamScopeId", "not-a-scope"),
                p("AddressFamily", ADDRESS_FAMILY_IPV4),
            ],
            400,
            INVALID_IPAM_SCOPE_ID_MALFORMED,
        ),
        (
            "a scope id whose suffix is not hexadecimal",
            vec![
                p("IpamScopeId", "ipam-scope-zzzzzzzz"),
                p("AddressFamily", ADDRESS_FAMILY_IPV4),
            ],
            400,
            INVALID_IPAM_SCOPE_ID_MALFORMED,
        ),
        (
            "an IPAM pool id where a scope id belongs",
            vec![
                p("IpamScopeId", &private_ipv4),
                p("AddressFamily", ADDRESS_FAMILY_IPV4),
            ],
            400,
            INVALID_IPAM_SCOPE_ID_MALFORMED,
        ),
        (
            "a netmask length past the IPv4 maximum",
            vec![
                p("IpamScopeId", &ipam.private_scope),
                p("AddressFamily", ADDRESS_FAMILY_IPV4),
                p("AllocationMinNetmaskLength", "33"),
            ],
            400,
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a default netmask length past the IPv4 maximum",
            vec![
                p("IpamScopeId", &ipam.private_scope),
                p("AddressFamily", ADDRESS_FAMILY_IPV4),
                p("AllocationDefaultNetmaskLength", "64"),
            ],
            400,
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a minimum netmask length at the maximum",
            vec![
                p("IpamScopeId", &ipam.private_scope),
                p("AddressFamily", ADDRESS_FAMILY_IPV4),
                p("AllocationMinNetmaskLength", "24"),
                p("AllocationMaxNetmaskLength", "24"),
            ],
            400,
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a default netmask length below the minimum",
            vec![
                p("IpamScopeId", &ipam.private_scope),
                p("AddressFamily", ADDRESS_FAMILY_IPV4),
                p("AllocationMinNetmaskLength", "20"),
                p("AllocationMaxNetmaskLength", "28"),
                p("AllocationDefaultNetmaskLength", "16"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "an advertisement flag on an IPv4 pool",
            vec![
                p("IpamScopeId", &ipam.public_scope),
                p("AddressFamily", ADDRESS_FAMILY_IPV4),
                p("PubliclyAdvertisable", "true"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "an advertisement flag on an Amazon-sourced IPv6 pool",
            vec![
                p("IpamScopeId", &ipam.public_scope),
                p("AddressFamily", ADDRESS_FAMILY_IPV6),
                p("PublicIpSource", "amazon"),
                p("PubliclyAdvertisable", "true"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "a public IP source on a pool in the private scope",
            vec![
                p("IpamScopeId", &ipam.private_scope),
                p("AddressFamily", ADDRESS_FAMILY_IPV6),
                p("PublicIpSource", IPAM_POOL_PUBLIC_IP_SOURCE_BYOIP),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "a locale that is no Region",
            vec![
                p("IpamScopeId", &ipam.private_scope),
                p("AddressFamily", ADDRESS_FAMILY_IPV4),
                p("Locale", "nowhere"),
            ],
            400,
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a source pool id that is not an id at all",
            vec![
                p("IpamScopeId", &ipam.private_scope),
                p("AddressFamily", ADDRESS_FAMILY_IPV4),
                p("SourceIpamPoolId", "not-a-pool"),
            ],
            400,
            INVALID_IPAM_POOL_ID_MALFORMED,
        ),
        (
            "a well-formed source pool id nothing created",
            vec![
                p("IpamScopeId", &ipam.private_scope),
                p("AddressFamily", ADDRESS_FAMILY_IPV4),
                p("SourceIpamPoolId", "ipam-pool-0000000000000000f"),
            ],
            400,
            INVALID_IPAM_POOL_ID_NOT_FOUND,
        ),
        (
            "a source pool in the other scope",
            vec![
                p("IpamScopeId", &ipam.private_scope),
                p("AddressFamily", ADDRESS_FAMILY_IPV4),
                p("SourceIpamPoolId", &public_ipv4),
            ],
            400,
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a source pool of the other address family",
            vec![
                p("IpamScopeId", &ipam.private_scope),
                p("AddressFamily", ADDRESS_FAMILY_IPV4),
                p("SourceIpamPoolId", &private_ipv6),
            ],
            400,
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a tag specification naming another resource type",
            vec![
                p("IpamScopeId", &ipam.private_scope),
                p("AddressFamily", ADDRESS_FAMILY_IPV4),
                p("TagSpecification.1.ResourceType", "vpc"),
                p("TagSpecification.1.Tag.1.Key", "Name"),
                p("TagSpecification.1.Tag.1.Value", "nope"),
            ],
            400,
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an allocation resource tag key given twice",
            vec![
                p("IpamScopeId", &ipam.private_scope),
                p("AddressFamily", ADDRESS_FAMILY_IPV4),
                p("AllocationResourceTag.1.Key", "Owner"),
                p("AllocationResourceTag.1.Value", "TeamA"),
                p("AllocationResourceTag.2.Key", "Owner"),
                p("AllocationResourceTag.2.Value", "TeamB"),
            ],
            400,
            INVALID_PARAMETER_VALUE,
        ),
        (
            // "A check for whether you have the required permissions for the action
            // without actually making the request" — the one row that is not a fault.
            "a dry run of a request that would have succeeded",
            vec![
                p("IpamScopeId", &ipam.private_scope),
                p("AddressFamily", ADDRESS_FAMILY_IPV4),
                p("DryRun", "true"),
            ],
            412,
            "DryRunOperation",
        ),
    ];

    for (case, params, expected_status, expected_code) in refusals {
        let (status, body) = invoke(&router, "CreateIpamPool", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        // A refused create writes nothing, so the store still holds only the pools seeded
        // above and the scopes still count them.
        let held = engine.state::<Ec2State>().lock().ipam_pools.len();
        assert_eq!(
            held, seeded_pools,
            "case {case:?}: a refused create must store no pool, store holds {held}"
        );
    }
}

// ---------------------------------------------------------------------------
// A pool within a pool
// ---------------------------------------------------------------------------

/// A pool created within a source pool sits one level deeper in its tree, and every level
/// takes the scope's facts from the same scope.
#[tokio::test]
async fn a_pool_within_a_source_pool_is_one_level_deeper() {
    let (router, engine) = super::ec2_query::emulator();
    let ipam = seed_ipam(&engine);

    let mut source: Option<String> = None;
    // Rows: the depth each successive create must report, which is what nesting
    // generalises over.
    for expected_depth in [TOP_LEVEL_IPAM_POOL_DEPTH, 2, 3] {
        let case = format!("a pool at depth {expected_depth}");
        let mut params = vec![
            p("IpamScopeId", &ipam.private_scope),
            p("AddressFamily", ADDRESS_FAMILY_IPV4),
        ];
        if let Some(source_pool_id) = source.as_deref() {
            params.push(p("SourceIpamPoolId", source_pool_id));
        }
        let (status, body) = invoke(&router, "CreateIpamPool", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "poolDepth").as_deref(),
            Some(expected_depth.to_string().as_str()),
            "case {case:?}: expected poolDepth {expected_depth}, got {body}"
        );
        assert_eq!(
            element(&body, "sourceIpamPoolId").as_deref(),
            source.as_deref(),
            "case {case:?}: expected the pool to name the source it was created in, got {body}"
        );
        source = element(&body, "ipamPoolId");
    }

    // Three pools in one scope, and the scope says so.
    let (stored, reported) = pool_counts(&engine, &ipam.private_scope);
    assert_eq!(
        stored, 3,
        "nesting: expected the store to hold three pools, holds {stored}"
    );
    assert_eq!(
        reported, 3,
        "nesting: expected the scope to report three pools, reports {reported}"
    );
}

// ---------------------------------------------------------------------------
// Idempotency
// ---------------------------------------------------------------------------

/// One `ClientToken` names the one pool it minted: a replay of the same request answers
/// that pool and creates nothing, and a replay asking for anything else is refused.
#[tokio::test]
async fn a_client_token_names_the_one_pool_it_minted() {
    let (router, engine) = super::ec2_query::emulator();
    let ipam = seed_ipam(&engine);
    let token = "f1a2b3c4-create-ipam-pool";

    let request = vec![
        p("IpamScopeId", &ipam.private_scope),
        p("AddressFamily", ADDRESS_FAMILY_IPV4),
        p("Description", "shared space"),
        p("ClientToken", token),
    ];
    let (status, body) = invoke(&router, "CreateIpamPool", &request).await;
    assert_eq!(status, 200, "first create: got {status} with body {body}");
    let pool_id = element(&body, "ipamPoolId").unwrap_or_default();

    let (status, replayed) = invoke(&router, "CreateIpamPool", &request).await;
    assert_eq!(status, 200, "replay: got {status} with body {replayed}");
    assert_eq!(
        element(&replayed, "ipamPoolId").as_deref(),
        Some(pool_id.as_str()),
        "replay: expected the pool the token minted, got {replayed}"
    );
    let held = engine.state::<Ec2State>().lock().ipam_pools.len();
    assert_eq!(
        held, 1,
        "replay: expected the one pool the token minted, store holds {held}"
    );

    // A replay reports the pool as it ACTUALLY stands: once the clock owes it the terminal
    // state, the replay answers that rather than the state the create left behind.
    tokio::time::sleep(past_the_transition()).await;
    let (status, settled) = invoke(&router, "CreateIpamPool", &request).await;
    assert_eq!(
        status, 200,
        "replay past the transition: got {status} with body {settled}"
    );
    assert_eq!(
        element(&settled, "state").as_deref(),
        Some(IPAM_POOL_STATE_CREATE_COMPLETE),
        "replay past the transition: expected the state the clock owed the pool, got {settled}"
    );

    // Rows: one member of the request changed under the SAME token — each asks for a pool
    // that is not the one the token names.
    let mismatches: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "another address family",
            vec![
                p("IpamScopeId", &ipam.private_scope),
                p("AddressFamily", ADDRESS_FAMILY_IPV6),
                p("Description", "shared space"),
                p("ClientToken", token),
            ],
        ),
        (
            "another description",
            vec![
                p("IpamScopeId", &ipam.private_scope),
                p("AddressFamily", ADDRESS_FAMILY_IPV4),
                p("Description", "something else"),
                p("ClientToken", token),
            ],
        ),
        (
            "another scope",
            vec![
                p("IpamScopeId", &ipam.public_scope),
                p("AddressFamily", ADDRESS_FAMILY_IPV4),
                p("Description", "shared space"),
                p("ClientToken", token),
            ],
        ),
        (
            "a netmask rule the first request did not carry",
            vec![
                p("IpamScopeId", &ipam.private_scope),
                p("AddressFamily", ADDRESS_FAMILY_IPV4),
                p("Description", "shared space"),
                p("AllocationMinNetmaskLength", "16"),
                p("ClientToken", token),
            ],
        ),
        (
            "a tag the first request did not carry",
            vec![
                p("IpamScopeId", &ipam.private_scope),
                p("AddressFamily", ADDRESS_FAMILY_IPV4),
                p("Description", "shared space"),
                p("TagSpecification.1.ResourceType", "ipam-pool"),
                p("TagSpecification.1.Tag.1.Key", "Name"),
                p("TagSpecification.1.Tag.1.Value", "development"),
                p("ClientToken", token),
            ],
        ),
    ];
    for (case, params) in mismatches {
        let (status, body) = invoke(&router, "CreateIpamPool", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "IdempotentParameterMismatch",
            "case {case:?}: expected IdempotentParameterMismatch, got {body}"
        );
        let held = engine.state::<Ec2State>().lock().ipam_pools.len();
        assert_eq!(
            held, 1,
            "case {case:?}: a refused replay must store no pool, store holds {held}"
        );
    }
}

// ---------------------------------------------------------------------------
// The created pool, read back through an implemented operation
// ---------------------------------------------------------------------------

/// A pool this call created is the pool the next operation to read one resolves: moving a
/// BYOIP range into it succeeds, which it can only do if the pool was stored with the
/// owner, the public scope type and the address family `MoveByoipCidrToIpam` checks the
/// range against.
#[tokio::test]
async fn a_created_pool_is_the_pool_a_later_reader_resolves() {
    let (router, engine) = super::ec2_query::emulator();
    let ipam = seed_ipam(&engine);
    let account = engine::emulator_account_id().to_string();

    let public_ipv4 = create_pool(&router, &ipam.public_scope, ADDRESS_FAMILY_IPV4).await;
    let private_ipv4 = create_pool(&router, &ipam.private_scope, ADDRESS_FAMILY_IPV4).await;

    let cidr = {
        let stored = engine
            .state::<Ec2State>()
            .lock()
            .insert_byoip_cidr(ByoipCidrRecord {
                byoip_cidr: ByoipCidr {
                    cidr: Some("192.0.2.0/24".to_string()),
                    state: Some(BYOIP_CIDR_STATE_PROVISIONED.to_string()),
                    network_border_group: Some(REGION.to_string()),
                    asn_associations: Some(vec![AsnAssociation {
                        asn: Some("64512".to_string()),
                        cidr: Some("192.0.2.0/24".to_string()),
                        state: Some("associated".to_string()),
                        status_message: None,
                    }]),
                    ..Default::default()
                },
                released_at: None,
                pending: None,
            });
        assert!(stored, "the store has room for the seeded BYOIP range");
        canonical_byoip_cidr("192.0.2.0/24").expect("192.0.2.0/24 is an address range")
    };

    // A pool in the private scope is no destination for a publicly routable range — which
    // is the created pool's own stored scope type being read back.
    let (status, body) = invoke(
        &router,
        "MoveByoipCidrToIpam",
        &[
            p("Cidr", &cidr),
            p("IpamPoolId", &private_ipv4),
            p("IpamPoolOwner", &account),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "a pool in the private scope: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        INVALID_PARAMETER_VALUE,
        "a pool in the private scope: expected InvalidParameterValue, got {body}"
    );

    let (status, body) = invoke(
        &router,
        "MoveByoipCidrToIpam",
        &[
            p("Cidr", &cidr),
            p("IpamPoolId", &public_ipv4),
            p("IpamPoolOwner", &account),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the move into the created pool: got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "ipamPoolId").as_deref(),
        Some(public_ipv4.as_str()),
        "the move: expected the range to name the created pool, got {body}"
    );
}
