// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteVpnConnectionRoute`, read back through
//! `DescribeVpnConnections` and written through the `CreateVpnConnectionRoute` that puts the
//! route there in the first place, driven over `ec2Query` (README.md / OPERATIONS.md "Wire
//! protocol").
//!
//! The rule under test: a static route delete names an existing VPN connection of the
//! account and the destination block of one route it carries, and it takes THAT route out of
//! the connection's own route set — the route is answered in the non-terminal `deleting`
//! state with the clock owing its removal, and every other route of the connection is left
//! as it was. A request naming a connection the account does not hold, a block that is not a
//! CIDR block, a block the connection carries no route for, a member sent empty, or a
//! connection whose state no longer admits the change is refused with EC2's own code for
//! that refusal and writes nothing.
//!
//! Each table row is a distinct input along what that generalises over: which address family
//! the block names, whether its host bits are already clear, how large the block is, whether
//! the delete spells the block the way the create did, which `VpnState` the connection is
//! in, which form the id takes, and how many routes the connection holds. The recorded case
//! the issue was filed about — a well-formed `vpn-` id nothing holds — is ONE row of
//! [`a_connection_the_account_does_not_hold_is_refused_and_writes_nothing`].
//!
//! `CreateVpnConnection` is itself still a scaffold stub under its own issue, so the
//! connection a row acts on is seeded through the same store helper the create suite seeds
//! it with ([`super::vpn_connection_routes::seed`]).

use ec2::state::{
    INCORRECT_STATE, INVALID_PARAMETER_VALUE, INVALID_ROUTE_NOT_FOUND,
    INVALID_VPN_CONNECTION_ID, MISSING_PARAMETER, VPN_STATE_AVAILABLE, VPN_STATE_DELETED,
    VPN_STATE_DELETING, VPN_STATE_PENDING, VPN_STATIC_ROUTE_SOURCE_STATIC,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, error_message};
use super::vpn_connection_routes::{
    Route, described_routes, past_the_transition, route_params, seed, stored_routes,
};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// One call of `action` signed for the Region above.
async fn invoke(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// One `CreateVpnConnectionRoute` that must be answered, so a delete has a route to act on.
async fn create_route(
    case: &str,
    router: &MultiServiceRouter,
    vpn_connection_id: &str,
    destination_cidr_block: &str,
) {
    let (status, body) = invoke(
        router,
        "CreateVpnConnectionRoute",
        &route_params(vpn_connection_id, destination_cidr_block),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateVpnConnectionRoute for \
         {destination_cidr_block}, got {status} with body {body}"
    );
}

/// One `DeleteVpnConnectionRoute` naming `destination_cidr_block` on `vpn_connection_id`.
async fn delete_route(
    router: &MultiServiceRouter,
    vpn_connection_id: &str,
    destination_cidr_block: &str,
) -> (u16, String) {
    invoke(
        router,
        "DeleteVpnConnectionRoute",
        &route_params(vpn_connection_id, destination_cidr_block),
    )
    .await
}

/// The route triple a block in `state` is reported as, with the one `VpnStaticRouteSource`
/// the API publishes.
fn route(destination_cidr_block: &str, state: &str) -> Route {
    (
        destination_cidr_block.to_string(),
        VPN_STATIC_ROUTE_SOURCE_STATIC.to_string(),
        state.to_string(),
    )
}

/// One row of the delete table: the block the create wrote, the block the delete asks about,
/// and the block the connection ends up reporting as `deleting`.
struct DeleteCase {
    /// What makes this row a different input.
    case: &'static str,
    /// `DestinationCidrBlock` as the CREATE sent it.
    created: &'static str,
    /// `DestinationCidrBlock` as the DELETE sends it.
    requested: &'static str,
    /// The block the route carries, which is the canonical form of both of the above.
    stored: &'static str,
}

/// The blocks a delete is exercised with: both address families, a block whose host bits are
/// already clear and one whose are not, a single-address route, a whole-internet route, and
/// two rows where the delete spells the block differently from the create — the canonical
/// form is what identifies ONE route of a connection ("We modify the specified CIDR block to
/// its canonical form", `CreateVpcRequest$CidrBlock`).
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            case: "an IPv4 /24 deleted exactly as it was created",
            created: "192.168.100.0/24",
            requested: "192.168.100.0/24",
            stored: "192.168.100.0/24",
        },
        DeleteCase {
            case: "an IPv4 /24 whose delete sets the host bits the create left clear",
            created: "10.0.1.0/24",
            requested: "10.0.1.37/24",
            stored: "10.0.1.0/24",
        },
        DeleteCase {
            case: "a single-address IPv4 route",
            created: "203.0.113.7/32",
            requested: "203.0.113.7/32",
            stored: "203.0.113.7/32",
        },
        DeleteCase {
            case: "the whole IPv4 address space",
            created: "0.0.0.0/0",
            requested: "0.0.0.0/0",
            stored: "0.0.0.0/0",
        },
        DeleteCase {
            case: "an IPv6 /64",
            created: "2001:db8:abcd:1234::/64",
            requested: "2001:db8:abcd:1234::/64",
            stored: "2001:db8:abcd:1234::/64",
        },
        DeleteCase {
            case: "an IPv6 /32 whose delete sets the host bits the create left clear",
            created: "2001:db8::/32",
            requested: "2001:db8::9/32",
            stored: "2001:db8::/32",
        },
    ]
}

/// A block the connection never carries a route for, whichever family the rows name: the
/// companion route every table row below keeps so a refusal can be shown to write nothing.
const KEPT_ROUTE: &str = "172.31.0.0/16";

/// Assert `routes` is the route `stored` on its way out followed by the companion route
/// whose own deletion was never started.
///
/// The companion's state is compared against `deleting` rather than against the one its
/// create left it in: the move that create owed it falls due on the clock, so which of
/// `pending` and `available` it reports depends on when the row ran, while "its deletion was
/// never started" is what this write must leave true at any instant.
fn assert_one_route_is_on_its_way_out(case: &str, routes: &[Route], stored: &str) {
    assert_eq!(
        routes.len(),
        2,
        "case {case:?}: expected the connection to still hold both routes, got {routes:?}"
    );
    assert_eq!(
        routes[0],
        route(stored, VPN_STATE_DELETING),
        "case {case:?}: expected {stored} to be on its way out, got {routes:?}"
    );
    assert_eq!(
        routes[1].0, KEPT_ROUTE,
        "case {case:?}: expected {KEPT_ROUTE} to be the connection's other route, \
         got {routes:?}"
    );
    assert_ne!(
        routes[1].2, VPN_STATE_DELETING,
        "case {case:?}: a delete must not start another route's deletion, got {routes:?}"
    );
}

/// Assert the companion route is still the connection's one route and its deletion was
/// never started: what a REFUSED delete must leave behind, at any instant.
fn assert_the_kept_route_is_untouched(
    case: &str,
    engine: &engine::InMemoryEngine,
    vpn_connection_id: &str,
) {
    let routes = stored_routes(engine, vpn_connection_id);
    assert_eq!(
        routes.len(),
        1,
        "case {case:?}: a refused delete must write nothing, got {routes:?}"
    );
    assert_eq!(
        routes[0].0, KEPT_ROUTE,
        "case {case:?}: a refused delete must leave {KEPT_ROUTE} alone, got {routes:?}"
    );
    assert_ne!(
        routes[0].2, VPN_STATE_DELETING,
        "case {case:?}: a refused delete must not start the route's deletion, got {routes:?}"
    );
}

/// A delete moves the route it names into the transient `deleting` state, leaves every other
/// route of the connection alone, and the clock then owes the route's removal from the
/// connection — whether or not anybody asks again.
#[tokio::test]
async fn a_deleted_static_route_leaves_the_connection_once_its_removal_falls_due() {
    for DeleteCase {
        case,
        created,
        requested,
        stored,
    } in delete_cases()
    {
        let (router, engine) = super::ec2_query::emulator();
        let vpn_connection_id = seed(&engine, VPN_STATE_AVAILABLE);
        create_route(case, &router, &vpn_connection_id, created).await;
        create_route(case, &router, &vpn_connection_id, KEPT_ROUTE).await;

        let (status, body) = delete_route(&router, &vpn_connection_id, requested).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );

        // The route is on its way out and the other one is untouched: both are reported out
        // of the connection's own route set, so the store and the describe agree on them.
        let actual = stored_routes(&engine, &vpn_connection_id);
        assert_one_route_is_on_its_way_out(case, &actual, stored);
        let actual = described_routes(case, &router, &vpn_connection_id).await;
        assert_one_route_is_on_its_way_out(case, &actual, stored);

        // The removal the clock owed falls due on its own, at the span every transition of
        // this store takes; the route the delete named is then gone and the other one has
        // reached the state ITS own create owed it.
        tokio::time::sleep(past_the_transition()).await;
        let expected = vec![route(KEPT_ROUTE, VPN_STATE_AVAILABLE)];
        let actual = described_routes(case, &router, &vpn_connection_id).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: the clock owed the deleted route its removal, got {actual:?}"
        );
    }
}

/// One row of the refusal tables: what the request carries, and the EC2 code it must be
/// refused with.
struct RefusalCase {
    /// What makes this row a different input.
    case: &'static str,
    /// The member under test, as the request sends it.
    value: &'static str,
    /// The `<Code>` EC2 answers.
    code: &'static str,
}

/// A VPN connection id this account holds no connection for is refused with
/// `InvalidVpnConnectionId`, whichever form the id takes, and the message names the id the
/// request asked about. An empty id is a missing parameter, not a lookup that failed.
/// Nothing is written either way: the route the connection the account DOES hold carries is
/// untouched.
#[tokio::test]
async fn a_connection_the_account_does_not_hold_is_refused_and_writes_nothing() {
    // The first row is the id the recorded request named.
    let cases = [
        RefusalCase {
            case: "a long-form id nothing holds",
            value: "vpn-00000000000000002",
            code: INVALID_VPN_CONNECTION_ID,
        },
        RefusalCase {
            case: "a short-form id nothing holds",
            value: "vpn-0abcd123",
            code: INVALID_VPN_CONNECTION_ID,
        },
        RefusalCase {
            case: "an id of another resource kind",
            value: "vgw-0123456789abcdef0",
            code: INVALID_VPN_CONNECTION_ID,
        },
        RefusalCase {
            case: "not a resource id at all",
            value: "not-an-id",
            code: INVALID_VPN_CONNECTION_ID,
        },
        RefusalCase {
            case: "an empty id",
            value: "",
            code: MISSING_PARAMETER,
        },
    ];

    let (router, engine) = super::ec2_query::emulator();
    let held = seed(&engine, VPN_STATE_AVAILABLE);
    create_route("the connection the account holds", &router, &held, KEPT_ROUTE).await;
    for RefusalCase { case, value, code } in cases {
        let (status, body) = delete_route(&router, value, KEPT_ROUTE).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let actual = error_code(&body);
        assert_eq!(
            actual, code,
            "case {case:?}: expected the code {code}, got {actual} in {body}"
        );
        if code == INVALID_VPN_CONNECTION_ID {
            let expected = format!("The specified VpnConnection ({value}) does not exist");
            let actual = error_message(&body);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the message {expected:?}, got {actual:?}"
            );
        }
        assert_the_kept_route_is_untouched(case, &engine, &held);
    }
}

/// A `DestinationCidrBlock` that is not a CIDR block of either family is refused with
/// `InvalidParameterValue`, and an empty one is a missing parameter. Nothing is written.
#[tokio::test]
async fn a_destination_block_that_is_not_a_cidr_block_is_refused() {
    let cases = [
        RefusalCase {
            case: "an IPv4 prefix longer than the address",
            value: "10.0.0.0/33",
            code: INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "an address with no prefix length",
            value: "10.0.0.0",
            code: INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "a prefix length that is not a number",
            value: "10.0.0.0/abc",
            code: INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "a negative prefix length",
            value: "10.0.0.0/-1",
            code: INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "an IPv6 prefix longer than the address",
            value: "2001:db8::/129",
            code: INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "text that is not an address",
            value: "not-a-cidr",
            code: INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "an empty block",
            value: "",
            code: MISSING_PARAMETER,
        },
    ];

    let (router, engine) = super::ec2_query::emulator();
    let vpn_connection_id = seed(&engine, VPN_STATE_AVAILABLE);
    create_route("a block that is not a CIDR block", &router, &vpn_connection_id, KEPT_ROUTE)
        .await;
    for RefusalCase { case, value, code } in cases {
        let (status, body) = delete_route(&router, &vpn_connection_id, value).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let actual = error_code(&body);
        assert_eq!(
            actual, code,
            "case {case:?}: expected the code {code}, got {actual} in {body}"
        );
        assert_the_kept_route_is_untouched(case, &engine, &vpn_connection_id);
    }
}

/// A well-formed block the connection carries no route for is refused with
/// `InvalidRoute.NotFound` — a different answer from "no such connection", so a caller can
/// tell the two apart — and the route the connection does carry is left as it was. A block
/// ANOTHER connection of the account holds a route for is no different: a route belongs to
/// one connection.
#[tokio::test]
async fn a_block_the_connection_carries_no_route_for_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let vpn_connection_id = seed(&engine, VPN_STATE_AVAILABLE);
    let other = seed(&engine, VPN_STATE_AVAILABLE);
    create_route("the connection under test", &router, &vpn_connection_id, KEPT_ROUTE).await;
    create_route("another connection", &router, &other, "10.99.0.0/16").await;

    let cases = [
        RefusalCase {
            case: "an IPv4 block no route of this connection carries",
            value: "198.51.100.0/24",
            code: INVALID_ROUTE_NOT_FOUND,
        },
        RefusalCase {
            case: "a block another connection of the account holds a route for",
            value: "10.99.0.0/16",
            code: INVALID_ROUTE_NOT_FOUND,
        },
        RefusalCase {
            case: "a wider block than the one the connection holds",
            value: "172.0.0.0/8",
            code: INVALID_ROUTE_NOT_FOUND,
        },
        RefusalCase {
            case: "a narrower block than the one the connection holds",
            value: "172.31.1.0/24",
            code: INVALID_ROUTE_NOT_FOUND,
        },
        RefusalCase {
            case: "an IPv6 block no route of this connection carries",
            value: "2001:db8:dead:beef::/64",
            code: INVALID_ROUTE_NOT_FOUND,
        },
    ];
    for RefusalCase { case, value, code } in cases {
        let (status, body) = delete_route(&router, &vpn_connection_id, value).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let actual = error_code(&body);
        assert_eq!(
            actual, code,
            "case {case:?}: expected the code {code}, got {actual} in {body}"
        );
        assert_the_kept_route_is_untouched(case, &engine, &vpn_connection_id);
    }
}

/// Which `VpnState` a connection is in decides whether one of its static routes can be
/// deleted: a connection being brought up or in service admits the delete, one being or
/// already torn down does not — and that refusal writes nothing. The two non-terminal rows
/// are reached by seeding the connection in that state; the two terminal ones by the
/// `DeleteVpnConnection` that puts it on its way out, and by the clock that then owes it
/// `deleted`.
#[tokio::test]
async fn whether_a_route_can_be_deleted_follows_the_connections_state() {
    // Every member of the modelled `VpnState` enum
    // (`smithy_query(query="shape",shape="VpnState")`), with the status a route delete on a
    // connection in it answers.
    let cases = [
        (VPN_STATE_PENDING, false, 200, ""),
        (VPN_STATE_AVAILABLE, false, 200, ""),
        (VPN_STATE_DELETING, true, 400, INCORRECT_STATE),
        (VPN_STATE_DELETED, true, 400, INCORRECT_STATE),
    ];
    for (state, delete_the_connection, expected_status, expected_code) in cases {
        let case = format!("a {state} connection");
        let (router, engine) = super::ec2_query::emulator();
        // The connection starts in a state that admits the create, so every row has the
        // same route to delete; the two terminal rows are then taken out of that state by
        // the operation that does it.
        let seeded = if delete_the_connection {
            VPN_STATE_AVAILABLE
        } else {
            state
        };
        let vpn_connection_id = seed(&engine, seeded);
        create_route(&case, &router, &vpn_connection_id, KEPT_ROUTE).await;
        if delete_the_connection {
            let (status, body) = invoke(
                &router,
                "DeleteVpnConnection",
                &[("VpnConnectionId".to_string(), vpn_connection_id.clone())],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from DeleteVpnConnection, got {status} \
                 with body {body}"
            );
            if state == VPN_STATE_DELETED {
                // The connection's own move to `deleted` was owed from that write, at the
                // span every transition of this store takes.
                tokio::time::sleep(past_the_transition()).await;
            }
        }

        let (status, body) = delete_route(&router, &vpn_connection_id, KEPT_ROUTE).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let routes = stored_routes(&engine, &vpn_connection_id);
        if expected_status == 200 {
            assert_eq!(
                routes,
                vec![route(KEPT_ROUTE, VPN_STATE_DELETING)],
                "case {case:?}: expected the route to be on its way out, got {routes:?}"
            );
        } else {
            let actual = error_code(&body);
            assert_eq!(
                actual, expected_code,
                "case {case:?}: expected the code {expected_code}, got {actual} in {body}"
            );
            // The route the connection carried is untouched: the connection's own state
            // says nothing about what its routes have already been asked to do.
            assert_the_kept_route_is_untouched(&case, &engine, &vpn_connection_id);
        }
    }
}

/// A route of a connection is identified by its destination block, so repeating the delete —
/// with the same spelling or another spelling of the same block — reports the one deletion
/// already under way and does not push its removal any further out.
#[tokio::test]
async fn repeating_a_delete_reports_the_deletion_already_under_way() {
    let (router, engine) = super::ec2_query::emulator();
    let vpn_connection_id = seed(&engine, VPN_STATE_AVAILABLE);
    create_route("the route to delete", &router, &vpn_connection_id, "10.0.2.0/24").await;

    // The same block three ways: canonical, repeated verbatim, and with host bits set.
    for spelling in ["10.0.2.0/24", "10.0.2.0/24", "10.0.2.9/24"] {
        let case = format!("DestinationCidrBlock={spelling}");
        let (status, body) = delete_route(&router, &vpn_connection_id, spelling).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let actual = stored_routes(&engine, &vpn_connection_id);
        assert_eq!(
            actual,
            vec![route("10.0.2.0/24", VPN_STATE_DELETING)],
            "case {case:?}: expected the one route to be on its way out, got {actual:?}"
        );
    }

    // The removal was owed from the FIRST delete, so it falls due at the same instant
    // however many repeats followed.
    tokio::time::sleep(past_the_transition()).await;
    let actual = described_routes("after the repeats", &router, &vpn_connection_id).await;
    assert!(
        actual.is_empty(),
        "after the repeats: the clock owed the route its removal, got {actual:?}"
    );

    // Once the route is gone there is nothing left to delete, which is the refusal a block
    // the connection carries no route for is answered with.
    let (status, body) = delete_route(&router, &vpn_connection_id, "10.0.2.0/24").await;
    assert_eq!(
        status, 400,
        "a route already removed: expected HTTP 400, got {status} with body {body}"
    );
    let actual = error_code(&body);
    assert_eq!(
        actual, INVALID_ROUTE_NOT_FOUND,
        "a route already removed: expected {INVALID_ROUTE_NOT_FOUND}, got {actual} in {body}"
    );
}
