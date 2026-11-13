// SPDX-License-Identifier: CC0-1.0

//! What `DescribeNetworkInterfacePermissions` reports, end to end over the wire the service
//! actually speaks: `ec2Query`, an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DescribeNetworkInterfacePermissions`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! The rules under test:
//!
//! 1. the describe REPORTS THE STORE — the set it answers is the permissions the account
//!    holds, narrowed by the `NetworkInterfacePermissionIds` and `Filters` the request names
//!    and cut into `MaxResults`-sized pages, in `NetworkInterfacePermissionId` order. A
//!    fresh account holds none, so the empty set is one row of that rule rather than a fixed
//!    answer;
//! 2. it is a READ: no shape of request changes what a later caller sees, beyond the one
//!    clock-owed move below;
//! 3. a permission is recorded in the non-terminal `pending` state and the clock owes it
//!    `granted` one transition span later, so a caller that reads straight after the write
//!    still sees `pending` and a later one sees `granted`;
//! 4. an id the account holds no permission for is REFUSED
//!    (`InvalidNetworkInterfacePermissionID.NotFound`) rather than answered as an absent
//!    row, and something that is not an id of that kind at all is refused before the store
//!    is read (`.Malformed`);
//! 5. a `MaxResults` below the smallest page the model publishes is refused with EC2's own
//!    `InvalidMaxResults`, naming the value and the bound, while each published bound itself
//!    is served and a request that names none gets the documented 50-item default page;
//! 6. the request-controlled factors of the match are BOUNDED: a request AT each of this
//!    crate's ceilings is answered and one past it is refused;
//! 7. a permission is a grant over one interface, so it goes when that interface does.
//!
//! Each table row is a distinct input along what the rule generalises over — which grantee
//! and permission type a permission carries, which interface it is over, which filter name
//! and how many values narrow the report, which ids, which page size and which side of a
//! bound — so no row is the specification.
//!
//! The permissions themselves are SEEDED through the store's writer seam
//! ([`Ec2State::insert_network_interface_permission`]) rather than granted over the wire:
//! seeding is what lets one row per published `InterfacePermissionType` and one per grantee
//! KIND be a case, including the service grantee
//! (`NetworkInterfacePermission$AwsService`) that `CreateNetworkInterfacePermission` itself
//! does not record. What that operation records, and which grants it refuses, is the sibling
//! suite `network_interface_permission_grants.rs`. The interfaces the grants are over, and
//! the delete that takes one away, ARE driven over the wire, so the describe is proven to
//! report the same store the implemented mutations write.

use ec2::ops::types::INTERFACE_PERMISSION_TYPE_VALUES;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, INVALID_MAX_RESULTS, MAX_FILTER_VALUE_BYTES,
    MAX_FILTER_VALUES, MAX_FILTERS, MAX_RESOURCE_IDS, NETWORK_INTERFACE_PERMISSION_ID_PREFIX,
    NETWORK_INTERFACE_PERMISSION_STATE_GRANTED, NETWORK_INTERFACE_PERMISSION_STATE_PENDING,
    NetworkInterfacePermissionSpec,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, filters, indexed};
use super::volume_lifecycle::past_the_transition;
use super::wire::{Param, param};

/// The Region every call here is signed for: the account's default VPC and its default
/// subnets are seeded for the emulator's default Region, and the interfaces these grants are
/// over are created in one of them.
const REGION: &str = "us-east-1";

/// The page size a request that names no `MaxResults` gets: "If this parameter is not
/// specified, up to 50 results are returned by default"
/// (`DescribeNetworkInterfacePermissionsRequest$MaxResults`).
const DOCUMENTED_DEFAULT_PAGE: usize = 50;

/// The published bounds of this operation's page size
/// (`DescribeNetworkInterfacePermissionsMaxResults`, `range 5..255`, from
/// `smithy_query(query="operation",operation="DescribeNetworkInterfacePermissions")`).
const PUBLISHED_MIN_PAGE: i64 = 5;
const PUBLISHED_MAX_PAGE: i64 = 255;

/// The EC2 code for a request member whose value the service will not accept.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// One row of a table that expects a refusal: `(case, the parameters, the error code)`.
type RefusalRow = (&'static str, Vec<Param>, &'static str);

/// One row of the filter table: `(case, the filter parameters, the permission ids it must
/// select)`.
type FilterRow = (&'static str, Vec<Param>, Vec<String>);

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One `DescribeNetworkInterfacePermissions` call carrying `params`.
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(
        router,
        REGION,
        "DescribeNetworkInterfacePermissions",
        params,
    )
    .await
}

/// The permission ids a successful describe reports, in the order it reported them.
async fn described_ids(router: &MultiServiceRouter, case: &str, params: &[Param]) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    elements(&body, "networkInterfacePermissionId")
}

/// The `NextToken` a response carries, treating an element that is there but empty as no
/// token: "this value is null when there are no more items to return"
/// (`DescribeNetworkInterfacePermissionsResult$NextToken`).
fn token(body: &str) -> Option<String> {
    element(body, "nextToken").filter(|value| !value.is_empty())
}

/// The first default subnet of the account's default VPC — a subnet the store really holds,
/// so an interface created into it is one the service itself minted.
fn default_subnet(engine: &engine::InMemoryEngine) -> String {
    let state = engine.state::<Ec2State>();
    let store = state.lock();
    let vpc_id = store
        .default_vpc_id(REGION)
        .expect("a fresh account has its default VPC");
    store
        .subnets
        .iter()
        .find(|(_, subnet)| subnet.vpc_id.as_deref() == Some(vpc_id.as_str()))
        .map(|(subnet_id, _)| subnet_id.clone())
        .expect("the default VPC comes with its default subnets")
}

/// Create one network interface through the API and answer the id it was given.
async fn create_interface(router: &MultiServiceRouter, subnet_id: &str, description: &str) -> String {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "CreateNetworkInterface",
        &[
            param("SubnetId", subnet_id),
            param("Description", description),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateNetworkInterface must succeed, got {status} with body {body}"
    );
    element(&body, "networkInterfaceId").unwrap_or_default()
}

/// Seed one permission through the store's writer seam and answer the id it was given.
fn seed_permission(
    engine: &engine::InMemoryEngine,
    network_interface_id: &str,
    aws_account_id: Option<&str>,
    aws_service: Option<&str>,
    permission: &str,
) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_network_interface_permission(NetworkInterfacePermissionSpec {
            network_interface_id: network_interface_id.to_string(),
            aws_account_id: aws_account_id.map(str::to_string),
            aws_service: aws_service.map(str::to_string),
            permission: permission.to_string(),
        })
        .expect("the store holds the seeded permission")
}

/// What one seeded permission is: the grantee, the permission type and which of the two
/// seeded interfaces it is over.
///
/// The two permission types are the whole published catalog
/// (`INTERFACE_PERMISSION_TYPE_VALUES`, the `InterfacePermissionType` enum), and the
/// grantee is either an account or a service, which is the choice
/// `NetworkInterfacePermission$AwsAccountId` / `$AwsService` offer. The account ids and the
/// service principal are seeded FACTS of the fixture — nothing in the emulator derives
/// them.
struct PermissionFixture {
    interface: usize,
    aws_account_id: Option<&'static str>,
    aws_service: Option<&'static str>,
    permission: &'static str,
}

/// The seeded account: two interfaces with four grants over them, differing along grantee
/// kind, grantee value, permission type and which interface they are over.
struct Seeded {
    interface_ids: Vec<String>,
    permission_ids: Vec<String>,
}

impl Seeded {
    /// The permission ids in the order the store holds them, which is the order the describe
    /// reports and pages over.
    fn every_id(&self) -> Vec<String> {
        let mut ids = self.permission_ids.clone();
        ids.sort();
        ids
    }

    /// The id of the `index`th seeded permission.
    fn id(&self, index: usize) -> String {
        self.permission_ids[index].clone()
    }
}

async fn seed(router: &MultiServiceRouter, engine: &engine::InMemoryEngine) -> Seeded {
    let subnet_id = default_subnet(engine);
    let interface_ids = vec![
        create_interface(router, &subnet_id, "shared").await,
        create_interface(router, &subnet_id, "also shared").await,
    ];
    let fixtures = [
        PermissionFixture {
            interface: 0,
            aws_account_id: Some("111122223333"),
            aws_service: None,
            permission: INTERFACE_PERMISSION_TYPE_VALUES[1],
        },
        PermissionFixture {
            interface: 0,
            aws_account_id: Some("444455556666"),
            aws_service: None,
            permission: INTERFACE_PERMISSION_TYPE_VALUES[0],
        },
        PermissionFixture {
            interface: 1,
            aws_account_id: Some("111122223333"),
            aws_service: None,
            permission: INTERFACE_PERMISSION_TYPE_VALUES[0],
        },
        PermissionFixture {
            interface: 1,
            aws_account_id: None,
            aws_service: Some("elasticloadbalancing.amazonaws.com"),
            permission: INTERFACE_PERMISSION_TYPE_VALUES[1],
        },
    ];
    let permission_ids = fixtures
        .iter()
        .map(|fixture| {
            seed_permission(
                engine,
                &interface_ids[fixture.interface],
                fixture.aws_account_id,
                fixture.aws_service,
                fixture.permission,
            )
        })
        .collect();
    Seeded {
        interface_ids,
        permission_ids,
    }
}

// ---------------------------------------------------------------------------
// Rule 1: the store is what is reported
// ---------------------------------------------------------------------------

/// A fresh account has granted NOTHING over a network interface — it has no interface to
/// grant over — so the describe reports the empty set, and reporting it is a read: a second
/// call answers the same and the store is untouched.
#[tokio::test]
async fn a_fresh_account_reports_no_permissions_and_reading_changes_nothing() {
    let (router, engine) = emulator();

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "fresh account: expected HTTP 200, got {status} with body {body}"
    );
    let reported = elements(&body, "networkInterfacePermissionId");
    assert!(
        reported.is_empty(),
        "fresh account: expected no permissions, got {reported:?} from {body}"
    );
    assert_eq!(
        token(&body),
        None,
        "fresh account: an empty set has no further page, got {body}"
    );

    let second = described_ids(&router, "second read", &[]).await;
    assert_eq!(
        second, reported,
        "a read must not change what a later caller sees: {reported:?} then {second:?}"
    );
    assert!(
        engine
            .state::<Ec2State>()
            .lock()
            .network_interface_permissions
            .is_empty(),
        "describing permissions must not write one into the store"
    );
}

/// Every permission the store holds is reported, in id order, with every member it was
/// written with — and one written after a call shows up in the next one, so the answer is
/// computed from the store rather than fixed.
#[tokio::test]
async fn every_stored_permission_is_reported_with_its_members_and_a_later_write_is_visible() {
    let (router, engine) = emulator();
    let seeded = seed(&router, &engine).await;

    let reported = described_ids(&router, "every permission", &[]).await;
    assert_eq!(
        reported,
        seeded.every_id(),
        "every permission: expected the store's permissions in id order, got {reported:?}"
    );
    for id in &reported {
        assert!(
            id.starts_with(&format!("{NETWORK_INTERFACE_PERMISSION_ID_PREFIX}-")),
            "expected a {NETWORK_INTERFACE_PERMISSION_ID_PREFIX}- id, got {id:?}"
        );
    }

    // One permission read back whole: every member a writer set round-trips through the
    // describe, so the report cannot drop one.
    let (status, body) = describe(
        &router,
        &indexed("NetworkInterfacePermissionId", &[&seeded.id(3)]),
    )
    .await;
    assert_eq!(
        status, 200,
        "one named permission: expected HTTP 200, got {status} with body {body}"
    );
    for (member, expected) in [
        ("networkInterfacePermissionId", seeded.id(3)),
        ("networkInterfaceId", seeded.interface_ids[1].clone()),
        (
            "awsService",
            "elasticloadbalancing.amazonaws.com".to_string(),
        ),
        ("permission", INTERFACE_PERMISSION_TYPE_VALUES[1].to_string()),
        (
            "state",
            NETWORK_INTERFACE_PERMISSION_STATE_PENDING.to_string(),
        ),
    ] {
        let actual = element(&body, member);
        assert_eq!(
            actual.as_deref(),
            Some(expected.as_str()),
            "one named permission: expected {member} {expected:?}, got {actual:?} from {body}"
        );
    }
    assert_eq!(
        element(&body, "awsAccountId"),
        None,
        "a grant to a service names no account, got {body}"
    );

    // The later write is visible to the next call, over an interface created through the
    // API in the same store.
    let late_interface =
        create_interface(&router, &default_subnet(&engine), "granted later").await;
    let late = seed_permission(
        &engine,
        &late_interface,
        Some("777788889999"),
        None,
        INTERFACE_PERMISSION_TYPE_VALUES[0],
    );
    let after = described_ids(&router, "after a later write", &[]).await;
    let mut expected = seeded.every_id();
    expected.push(late.clone());
    expected.sort();
    assert_eq!(
        after, expected,
        "after a later write: expected {late:?} to be reported too, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: every shape of request is a read
// ---------------------------------------------------------------------------

/// Describing is a READ whatever the request asks for, so no shape of request may change
/// what a later caller sees. The rows differ along everything a request can name — nothing
/// at all, one id, every id, each kind of filter, a page smaller than the store, and the
/// refusals — and after each one the store is exactly what it was and a second identical
/// call answers identically.
#[tokio::test]
async fn no_shape_of_describe_changes_the_store_or_a_later_answer() {
    let (router, engine) = emulator();
    let seeded = seed(&router, &engine).await;
    let every: Vec<String> = seeded.every_id();
    let every_id: Vec<&str> = every.iter().map(String::as_str).collect();

    /// A well-formed permission id the account holds no permission for.
    const UNHELD_ID: &str = "eni-perm-000000000000000ff";

    let shapes: Vec<(&str, Vec<Param>)> = vec![
        ("the whole store", Vec::new()),
        (
            "one named id",
            indexed("NetworkInterfacePermissionId", &[every_id[0]]),
        ),
        (
            "every named id",
            indexed("NetworkInterfacePermissionId", &every_id),
        ),
        (
            "a filter that selects a subset",
            filters(&[(
                "network-interface-permission.network-interface-id",
                &[seeded.interface_ids[0].as_str()],
            )]),
        ),
        (
            "a filter that selects nothing",
            filters(&[(
                "network-interface-permission.aws-account-id",
                &["000000000000"],
            )]),
        ),
        (
            "two filters joined with an AND",
            filters(&[
                (
                    "network-interface-permission.aws-account-id",
                    &["111122223333"],
                ),
                (
                    "network-interface-permission.permission",
                    &[INTERFACE_PERMISSION_TYPE_VALUES[0]],
                ),
            ]),
        ),
        (
            "a page smaller than the store",
            vec![param("MaxResults", "5")],
        ),
        (
            "an id the account holds no permission for",
            indexed("NetworkInterfacePermissionId", &[UNHELD_ID]),
        ),
        (
            "an id that is not a permission id at all",
            indexed("NetworkInterfacePermissionId", &["not-an-id"]),
        ),
        (
            "a filter name the operation does not document",
            filters(&[("not-a-filter", &["anything"])]),
        ),
        (
            "a page size below the published minimum",
            vec![param("MaxResults", "1")],
        ),
        (
            "a NextToken this service did not mint",
            vec![param("NextToken", "not-a-token")],
        ),
    ];

    let snapshot = || {
        let state = engine.state::<Ec2State>();
        let store = state.lock();
        format!("{:?}", store.network_interface_permissions)
    };
    let before = snapshot();
    assert!(
        !every.is_empty(),
        "the invariant is asserted against a POPULATED store, which holds {} permissions",
        every.len()
    );

    for (case, params) in shapes {
        let (first_status, first_body) = describe(&router, &params).await;
        let (second_status, second_body) = describe(&router, &params).await;
        assert_eq!(
            first_status, second_status,
            "case {case:?}: expected the repeat to answer HTTP {first_status}, got \
             {second_status} with body {second_body}"
        );
        let first_ids = elements(&first_body, "networkInterfacePermissionId");
        let second_ids = elements(&second_body, "networkInterfacePermissionId");
        assert_eq!(
            first_ids, second_ids,
            "case {case:?}: expected the repeat to report {first_ids:?}, got {second_ids:?}"
        );
        assert_eq!(
            error_code(&first_body),
            error_code(&second_body),
            "case {case:?}: expected the repeat to carry the same error code, got {second_body}"
        );
    }
    // The clock-owed `granted` move is the one write a read may make, so what is asserted
    // here is that no request SHAPE wrote anything of its own: once the moves every seeded
    // permission was owed have been applied, two further reads leave the store exactly
    // where it is.
    past_the_transition().await;
    let _ = described_ids(&router, "after the transition", &[]).await;
    let settled = snapshot();
    assert!(
        settled.contains(NETWORK_INTERFACE_PERMISSION_STATE_GRANTED),
        "the clock owed every seeded permission its granted move, got {settled}"
    );
    for (case, params) in [
        ("the whole store", Vec::new()),
        (
            "one named id",
            indexed("NetworkInterfacePermissionId", &[every_id[0]]),
        ),
    ] {
        let _ = describe(&router, &params).await;
        assert_eq!(
            settled,
            snapshot(),
            "case {case:?}: a read after the clock's move must leave the store alone"
        );
    }
    assert_ne!(
        before, settled,
        "the seeded permissions start out pending, so the clock's move must be visible in \
         the store"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: the clock owes a pending permission its granted move
// ---------------------------------------------------------------------------

/// A permission is recorded in the non-terminal `pending` state — the
/// `NetworkInterfacePermissionStateCode` member a grant that is not in force yet carries —
/// and the clock owes it `granted` one transition span later, whether or not anybody calls
/// again. So a caller reading straight after the write sees `pending`, a later one sees
/// `granted`, and the move happens once.
#[tokio::test]
async fn a_pending_permission_reaches_granted_when_the_clock_owes_it() {
    let (router, engine) = emulator();
    let seeded = seed(&router, &engine).await;

    let states = |body: &str| elements(body, "state");
    let (status, before) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "before the transition: expected HTTP 200, got {status} with body {before}"
    );
    assert_eq!(
        states(&before),
        vec![NETWORK_INTERFACE_PERMISSION_STATE_PENDING.to_string(); seeded.permission_ids.len()],
        "before the transition: every permission must still be pending, got {before}"
    );

    past_the_transition().await;

    let (status, after) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "after the transition: expected HTTP 200, got {status} with body {after}"
    );
    assert_eq!(
        states(&after),
        vec![NETWORK_INTERFACE_PERMISSION_STATE_GRANTED.to_string(); seeded.permission_ids.len()],
        "after the transition: the clock owed every permission its granted move, got {after}"
    );

    let (status, again) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "a second read after the transition: expected HTTP 200, got {status} with body {again}"
    );
    assert_eq!(
        states(&again),
        states(&after),
        "the move happens once: expected {:?}, got {:?}",
        states(&after),
        states(&again)
    );
}

// ---------------------------------------------------------------------------
// Rule 1 (filters) and rule 4 (ids)
// ---------------------------------------------------------------------------

/// Every filter name the operation documents selects exactly the permissions that hold the
/// value asked for, several values are joined with an OR and several filters with an AND, and
/// a name the operation does not document is REFUSED rather than silently selecting nothing.
/// One row per documented name, from
/// `smithy_query(query="doc",shape="DescribeNetworkInterfacePermissionsRequest",member="Filters")`.
#[tokio::test]
async fn each_documented_filter_selects_the_permissions_that_hold_it() {
    let (router, engine) = emulator();
    let seeded = seed(&router, &engine).await;

    let rows: Vec<FilterRow> = vec![
        (
            "the permission id itself",
            filters(&[(
                "network-interface-permission.network-interface-permission-id",
                &[seeded.id(1).as_str()],
            )]),
            vec![seeded.id(1)],
        ),
        (
            "the interface the grant is over",
            filters(&[(
                "network-interface-permission.network-interface-id",
                &[seeded.interface_ids[0].as_str()],
            )]),
            vec![seeded.id(0), seeded.id(1)],
        ),
        (
            "the account the grant is to",
            filters(&[(
                "network-interface-permission.aws-account-id",
                &["111122223333"],
            )]),
            vec![seeded.id(0), seeded.id(2)],
        ),
        (
            "the service the grant is to",
            filters(&[(
                "network-interface-permission.aws-service",
                &["elasticloadbalancing.amazonaws.com"],
            )]),
            vec![seeded.id(3)],
        ),
        (
            "the permission type",
            filters(&[(
                "network-interface-permission.permission",
                &[INTERFACE_PERMISSION_TYPE_VALUES[0]],
            )]),
            vec![seeded.id(1), seeded.id(2)],
        ),
        (
            "the other permission type",
            filters(&[(
                "network-interface-permission.permission",
                &[INTERFACE_PERMISSION_TYPE_VALUES[1]],
            )]),
            vec![seeded.id(0), seeded.id(3)],
        ),
        (
            "two values of one filter, joined with an OR",
            filters(&[(
                "network-interface-permission.aws-account-id",
                &["111122223333", "444455556666"],
            )]),
            vec![seeded.id(0), seeded.id(1), seeded.id(2)],
        ),
        (
            "two filters, joined with an AND",
            filters(&[
                (
                    "network-interface-permission.aws-account-id",
                    &["111122223333"],
                ),
                (
                    "network-interface-permission.permission",
                    &[INTERFACE_PERMISSION_TYPE_VALUES[0]],
                ),
            ]),
            vec![seeded.id(2)],
        ),
        (
            "an EC2 wildcard over the account id",
            filters(&[(
                "network-interface-permission.aws-account-id",
                &["1111*"],
            )]),
            vec![seeded.id(0), seeded.id(2)],
        ),
        (
            "a value no permission holds",
            filters(&[(
                "network-interface-permission.aws-service",
                &["nothing.amazonaws.com"],
            )]),
            Vec::new(),
        ),
        (
            "a filter on a member a permission does not carry",
            filters(&[("network-interface-permission.aws-service", &["*"])]),
            vec![seeded.id(3)],
        ),
    ];

    for (case, params, expected) in rows {
        let mut wanted = expected.clone();
        wanted.sort();
        let reported = described_ids(&router, case, &params).await;
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected {wanted:?}, got {reported:?}"
        );
    }

    // A name this operation does not document is refused whether or not anything would have
    // matched it, so an unsupported filter can never read as "no results".
    for name in ["not-a-filter", "Network-Interface-Permission.Permission", "tag:Name"] {
        let (status, body) = describe(&router, &filters(&[(name, &["anything"])])).await;
        assert_eq!(
            status, 400,
            "filter {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PARAMETER_VALUE,
            "filter {name:?}: expected {INVALID_PARAMETER_VALUE}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The filter '{name}' is invalid"),
            "filter {name:?}: expected the refusal to name the filter, got {body}"
        );
    }
}

/// `NetworkInterfacePermissionIds` narrows the report to the ids named, repeats of one id
/// name one permission, a well-formed id the account holds no permission for is REFUSED
/// `InvalidNetworkInterfacePermissionID.NotFound`, something that is not an id of that kind
/// is refused `.Malformed` before the store is read, and an id list past this crate's
/// ceiling is refused too.
#[tokio::test]
async fn named_ids_narrow_the_report_and_an_id_naming_nothing_is_refused() {
    let (router, engine) = emulator();
    let seeded = seed(&router, &engine).await;

    let one = seeded.id(2);
    let two = seeded.id(0);
    for (case, ids, expected) in [
        ("one id", vec![one.as_str()], vec![one.clone()]),
        (
            "two ids",
            vec![one.as_str(), two.as_str()],
            vec![one.clone(), two.clone()],
        ),
        (
            "the same id twice",
            vec![one.as_str(), one.as_str()],
            vec![one.clone()],
        ),
    ] {
        let mut wanted = expected.clone();
        wanted.sort();
        let reported = described_ids(
            &router,
            case,
            &indexed("NetworkInterfacePermissionId", &ids),
        )
        .await;
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected {wanted:?}, got {reported:?}"
        );
    }

    let too_many: Vec<String> = (0..=MAX_RESOURCE_IDS)
        .map(|index| format!("eni-perm-{index:017x}"))
        .collect();
    let too_many_refs: Vec<&str> = too_many.iter().map(String::as_str).collect();
    let rows: Vec<RefusalRow> = vec![
        (
            "a well-formed id the account holds no permission for",
            indexed(
                "NetworkInterfacePermissionId",
                &["eni-perm-000000000000000ff"],
            ),
            "InvalidNetworkInterfacePermissionID.NotFound",
        ),
        (
            "an id of another kind",
            indexed("NetworkInterfacePermissionId", &["eni-00000000000000001"]),
            "InvalidNetworkInterfacePermissionID.Malformed",
        ),
        (
            "an id with too few digits",
            indexed("NetworkInterfacePermissionId", &["eni-perm-0000"]),
            "InvalidNetworkInterfacePermissionID.Malformed",
        ),
        (
            "an id with a digit outside hexadecimal",
            indexed(
                "NetworkInterfacePermissionId",
                &["eni-perm-00000000000000zzz"],
            ),
            "InvalidNetworkInterfacePermissionID.Malformed",
        ),
        (
            "one id past the ceiling on a resource-id list",
            indexed("NetworkInterfacePermissionId", &too_many_refs),
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a NextToken this service did not mint",
            vec![param("NextToken", "not-a-token")],
            "InvalidPaginationToken",
        ),
    ];
    for (case, params, expected_code) in rows {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // The ids a request names are resolved against the store, so the refusal names the id
    // the caller sent rather than a fixed one.
    let (_, body) = describe(
        &router,
        &indexed(
            "NetworkInterfacePermissionId",
            &["eni-perm-000000000000000ff"],
        ),
    )
    .await;
    assert_eq!(
        error_message(&body),
        "The NetworkInterfacePermission ID 'eni-perm-000000000000000ff' does not exist",
        "the refusal must name the id the request sent, got {body}"
    );
}

// ---------------------------------------------------------------------------
// Rule 5: paging
// ---------------------------------------------------------------------------

/// A request that names no `MaxResults` gets the documented 50-item default page with a
/// token, and the page after it holds the rest and carries none; a `MaxResults` smaller than
/// the store walks every permission once, in id order.
#[tokio::test]
async fn pages_walk_every_permission_once_and_a_request_naming_no_page_size_gets_fifty() {
    let (router, engine) = emulator();
    let interface_id = create_interface(&router, &default_subnet(&engine), "many grants").await;
    // Two more than the documented default, so the default page is cut and the page after it
    // is the remainder rather than empty.
    let held = DOCUMENTED_DEFAULT_PAGE + 2;
    let mut every: Vec<String> = (0..held)
        .map(|index| {
            seed_permission(
                &engine,
                &interface_id,
                Some("111122223333"),
                None,
                INTERFACE_PERMISSION_TYPE_VALUES[index % INTERFACE_PERMISSION_TYPE_VALUES.len()],
            )
        })
        .collect();
    every.sort();

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "the default page: expected HTTP 200, got {status} with body {body}"
    );
    let first = elements(&body, "networkInterfacePermissionId");
    assert_eq!(
        first.len(),
        DOCUMENTED_DEFAULT_PAGE,
        "the default page: expected {DOCUMENTED_DEFAULT_PAGE} permissions, got {}",
        first.len()
    );
    assert_eq!(
        first,
        every[..DOCUMENTED_DEFAULT_PAGE].to_vec(),
        "the default page: expected the first {DOCUMENTED_DEFAULT_PAGE} in id order, got \
         {first:?}"
    );
    let page_token = token(&body).unwrap_or_default();
    assert!(
        !page_token.is_empty(),
        "the default page: a further permission follows, so a token is due, got {body}"
    );

    let (status, body) = describe(&router, &[param("NextToken", &page_token)]).await;
    assert_eq!(
        status, 200,
        "the page after the default one: expected HTTP 200, got {status} with body {body}"
    );
    let rest = elements(&body, "networkInterfacePermissionId");
    assert_eq!(
        rest,
        every[DOCUMENTED_DEFAULT_PAGE..].to_vec(),
        "the page after the default one: expected the remainder, got {rest:?}"
    );
    assert_eq!(
        token(&body),
        None,
        "nothing follows the last page, so it carries no token, got {body}"
    );

    // A page size smaller than the store: the walk covers every permission once, in id
    // order, and terminates.
    for page_size in [PUBLISHED_MIN_PAGE, 7, PUBLISHED_MAX_PAGE] {
        let mut walked: Vec<String> = Vec::new();
        let mut carried: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params = vec![param("MaxResults", &page_size.to_string())];
            if let Some(cursor) = carried.clone() {
                params.push(param("NextToken", &cursor));
            }
            let case = format!("page size {page_size}");
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = elements(&body, "networkInterfacePermissionId");
            assert!(
                page.len() <= usize::try_from(page_size).unwrap_or(usize::MAX),
                "case {case:?}: expected at most {page_size} permissions, got {page:?}"
            );
            walked.extend(page);
            carried = token(&body);
            pages += 1;
            assert!(
                pages <= held + 1,
                "case {case:?}: paging must terminate, walked {walked:?}"
            );
            if carried.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, every,
            "case \"page size {page_size}\": expected every permission once in id order, got \
             {walked:?}"
        );
    }
}

/// `MaxResults` is served at each bound the model publishes and refused one past either —
/// and a value below the smallest page the operation serves carries EC2's own
/// `InvalidMaxResults`, naming the value the request sent and the bound it has to reach,
/// rather than a generic model-constraint code no EC2 client branches on.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused_with_ec2s_own_code() {
    let (router, engine) = emulator();
    let _seeded = seed(&router, &engine).await;

    for (case, page_size, expected_status) in [
        ("one below the published minimum", PUBLISHED_MIN_PAGE - 1, 400),
        ("far below the published minimum", 1, 400),
        ("the published minimum", PUBLISHED_MIN_PAGE, 200),
        ("the published maximum", PUBLISHED_MAX_PAGE, 200),
        ("one above the published maximum", PUBLISHED_MAX_PAGE + 1, 400),
    ] {
        let (status, body) = describe(&router, &[param("MaxResults", &page_size.to_string())]).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for MaxResults={page_size}, got \
             {status} with body {body}"
        );
        if expected_status != 400 {
            continue;
        }
        if page_size < PUBLISHED_MIN_PAGE {
            assert_eq!(
                error_code(&body),
                INVALID_MAX_RESULTS,
                "case {case:?}: expected {INVALID_MAX_RESULTS}, got {body}"
            );
            assert_eq!(
                error_message(&body),
                format!(
                    "Value ( {page_size} ) for parameter maxResults is invalid. Expecting a \
                     value greater than or equal to {PUBLISHED_MIN_PAGE}."
                ),
                "case {case:?}: expected the refusal to name the value and the bound, got \
                 {body}"
            );
        } else {
            assert!(
                error_message(&body).contains("maxResults"),
                "case {case:?}: expected the refusal to name the member, got {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 6: the request-controlled factors of the match are bounded
// ---------------------------------------------------------------------------

/// Matching cost is the PRODUCT of how many filters a request carries, how many values each
/// carries and how long a value is, and the model bounds none of them (`FilterList` and
/// `Filter$Values` carry no `@length`) — so a request AT each of this crate's ceilings is
/// answered while one PAST it is refused.
#[tokio::test]
async fn the_filtering_one_request_can_ask_for_is_bounded() {
    let (router, engine) = emulator();
    let seeded = seed(&router, &engine).await;
    let held = seeded.id(2);

    // A filter that every permission matches, repeated: the NUMBER of filters is what is
    // bounded here.
    let repeated = |count: usize| -> Vec<Param> {
        let entries: Vec<(&str, &[&str])> = (0..count)
            .map(|_| {
                (
                    "network-interface-permission.network-interface-permission-id",
                    &[][..],
                )
            })
            .collect();
        filters(&entries)
    };
    // One filter carrying `count` values, the last of which selects a permission the store
    // holds: the number of VALUES is what is bounded.
    let with_values = |count: usize| -> Vec<Param> {
        let mut values: Vec<String> = (0..count.saturating_sub(1))
            .map(|index| format!("eni-perm-{index:017x}"))
            .collect();
        values.push(held.clone());
        let refs: Vec<&str> = values.iter().map(String::as_str).collect();
        filters(&[(
            "network-interface-permission.network-interface-permission-id",
            &refs,
        )])
    };
    // One filter value of `bytes` characters: the LENGTH of a value is what is bounded.
    let value_of = |bytes: usize| -> Vec<Param> {
        let value = "a".repeat(bytes);
        filters(&[(
            "network-interface-permission.aws-account-id",
            &[value.as_str()],
        )])
    };

    for (case, params, expected_status) in [
        ("filters at the ceiling", repeated(MAX_FILTERS), 200),
        ("one filter past the ceiling", repeated(MAX_FILTERS + 1), 400),
        ("values at the ceiling", with_values(MAX_FILTER_VALUES), 200),
        (
            "one value past the ceiling",
            with_values(MAX_FILTER_VALUES + 1),
            400,
        ),
        (
            "a value at the byte ceiling",
            value_of(MAX_FILTER_VALUE_BYTES),
            200,
        ),
        (
            "a value one byte past the ceiling",
            value_of(MAX_FILTER_VALUE_BYTES + 1),
            400,
        ),
    ] {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                INVALID_PARAMETER_VALUE,
                "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 7: a grant does not outlive the interface it is over
// ---------------------------------------------------------------------------

/// A permission is a grant over ONE network interface, so deleting that interface through
/// the API takes its grants with it — and leaves every grant over another interface exactly
/// where it was.
#[tokio::test]
async fn the_grants_over_an_interface_go_when_that_interface_is_deleted() {
    let (router, engine) = emulator();
    let seeded = seed(&router, &engine).await;

    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "DeleteNetworkInterface",
        &[param("NetworkInterfaceId", &seeded.interface_ids[0])],
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteNetworkInterface must succeed, got {status} with body {body}"
    );

    let reported = described_ids(&router, "after the interface was deleted", &[]).await;
    let mut expected = vec![seeded.id(2), seeded.id(3)];
    expected.sort();
    assert_eq!(
        reported, expected,
        "after the interface was deleted: expected only the grants over the other interface, \
         got {reported:?}"
    );
    // And the ids of the grants that went now name nothing, rather than naming a record the
    // store no longer holds.
    let (status, body) = describe(
        &router,
        &indexed("NetworkInterfacePermissionId", &[&seeded.id(0)]),
    )
    .await;
    assert_eq!(
        status, 400,
        "a grant that went with its interface must name nothing, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidNetworkInterfacePermissionID.NotFound",
        "a grant that went with its interface must name nothing, got {body}"
    );
}
