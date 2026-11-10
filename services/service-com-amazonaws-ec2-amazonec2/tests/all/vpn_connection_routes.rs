// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateVpnConnectionRoute`, read back through
//! `DescribeVpnConnections`, driven over `ec2Query` (README.md / OPERATIONS.md "Wire
//! protocol").
//!
//! The rule under test: a static route create names an existing VPN connection of the
//! account and a CIDR block, and it PUTS that block into the connection's own route set —
//! one route per destination block, reported as `Static`, answered in the non-terminal
//! `pending` state with the clock owing it `available`. A request naming a connection the
//! account does not hold, a block that is not a CIDR block, or a connection whose state no
//! longer admits a route is refused with EC2's own code for that refusal and writes
//! nothing.
//!
//! Each table row is a distinct input along what that generalises over: which address
//! family the block names, whether its host bits are already clear, how large the block is,
//! which `VpnState` the connection is in, which form the id takes, and how many routes the
//! connection already holds. The recorded case the issue was filed about — a well-formed
//! `vpn-` id nothing holds — is ONE row of
//! [`a_connection_the_account_does_not_hold_is_refused_and_writes_nothing`].

use std::time::Duration;

use ec2::ops::types::{Tag, VpnConnection, VpnConnectionOptions, VpnStaticRoute};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DRY_RUN_OPERATION, INCORRECT_STATE,
    INVALID_PARAMETER_VALUE, INVALID_VPN_CONNECTION_ID, MAX_FILTERS, MAX_FILTER_VALUES,
    MAX_FILTER_VALUE_BYTES, MAX_RESOURCE_IDS, MAX_VPN_STATIC_ROUTES, MISSING_PARAMETER,
    ROUTE_LIMIT_EXCEEDED, TRANSITION_DELAY, VPN_STATE_AVAILABLE, VPN_STATE_DELETED,
    VPN_STATE_DELETING, VPN_STATE_PENDING, VPN_STATIC_ROUTE_SOURCE_STATIC,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, filters, indexed};
use super::wire::{child, child_text, document};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// Long enough past [`TRANSITION_DELAY`] that a move the clock owes is certainly due.
pub(crate) fn past_the_transition() -> Duration {
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

/// The form fields of one `CreateVpnConnectionRoute` request — and of the
/// `DeleteVpnConnectionRoute` the delete suite sends, whose two members are the same two.
pub(crate) fn route_params(vpn_connection_id: &str, destination_cidr_block: &str) -> Vec<(String, String)> {
    vec![
        (
            "VpnConnectionId".to_string(),
            vpn_connection_id.to_string(),
        ),
        (
            "DestinationCidrBlock".to_string(),
            destination_cidr_block.to_string(),
        ),
    ]
}

/// A seeded VPN connection in `state`, carrying `routes`: the members
/// `CreateVpnConnection` reports for an `ipsec.1` connection to a virtual private gateway.
/// `CreateVpnConnection` is itself still a scaffold stub, so this suite seeds the store the
/// way the transit gateway suite seeds attachments.
fn connection(state: &str, routes: Vec<VpnStaticRoute>) -> VpnConnection {
    VpnConnection {
        customer_gateway_id: Some("cgw-0123456789abcdef0".to_string()),
        vpn_gateway_id: Some("vgw-0123456789abcdef0".to_string()),
        r#type: Some("ipsec.1".to_string()),
        category: Some("VPN".to_string()),
        state: Some(state.to_string()),
        customer_gateway_configuration: Some("<vpn_connection/>".to_string()),
        options: Some(VpnConnectionOptions {
            static_routes_only: Some(true),
            ..Default::default()
        }),
        routes: Some(routes),
        tags: Some(vec![Tag {
            key: Some("Name".to_string()),
            value: Some("branch-office".to_string()),
        }]),
        ..Default::default()
    }
}

/// Seed one connection in `state` with no static route, and answer its id.
///
/// `pub(crate)` so the envelope row in [`super::ec2_query_error_request_id`] seeds the
/// same connection this suite does rather than building a second one.
pub(crate) fn seed(engine: &engine::InMemoryEngine, state: &str) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_vpn_connection(connection(state, Vec::new()))
        .expect("the store holds the seeded VPN connection")
}

/// One request of the bound table: what makes it a different input, its form fields, and
/// the HTTP status the describe answers it with.
type BoundCase = (String, Vec<(String, String)>, u16);

/// One static route as this suite compares them: the destination block, the source and
/// the state, which is every member `VpnStaticRoute` has.
///
/// `pub(crate)` so the delete suite in [`super::delete_vpn_connection_route`] compares the
/// routes of a connection the way this one does rather than reading them a second way.
pub(crate) type Route = (String, String, String);

/// One reported connection: its `VpnConnectionId` and the routes it carries.
type ReportedConnection = (String, Vec<Route>);

/// The static routes the store holds for `vpn_connection_id` right now, as
/// (destination block, source, state) triples in the order they are stored.
pub(crate) fn stored_routes(
    engine: &engine::InMemoryEngine,
    vpn_connection_id: &str,
) -> Vec<Route> {
    engine
        .state::<Ec2State>()
        .lock()
        .vpn_connection(vpn_connection_id)
        .map(|record| {
            record
                .routes()
                .iter()
                .map(|route| {
                    (
                        route.destination_cidr_block.clone().unwrap_or_default(),
                        route.source.clone().unwrap_or_default(),
                        route.state.clone().unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every connection a `DescribeVpnConnections` body reports, by `vpnConnectionId`, with the
/// (destination block, source, state) of each static route it carries.
fn reported(case: &str, body: &str) -> Vec<ReportedConnection> {
    let parsed = document(case, body);
    let mut connections = Vec::new();
    for node in parsed.descendants() {
        if !node.is_element() {
            continue;
        }
        let Some(vpn_connection_id) = child_text(node, "vpnConnectionId") else {
            continue;
        };
        let routes = child(node, "routes")
            .map(|routes| {
                routes
                    .children()
                    .filter(|item| item.is_element() && item.tag_name().name() == "item")
                    .map(|item| {
                        (
                            child_text(item, "destinationCidrBlock").unwrap_or_default(),
                            child_text(item, "source").unwrap_or_default(),
                            child_text(item, "state").unwrap_or_default(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        connections.push((vpn_connection_id, routes));
    }
    connections
}

/// The routes `DescribeVpnConnections` reports for `vpn_connection_id`.
pub(crate) async fn described_routes(
    case: &str,
    router: &MultiServiceRouter,
    vpn_connection_id: &str,
) -> Vec<Route> {
    let params = vec![(
        "VpnConnectionId.1".to_string(),
        vpn_connection_id.to_string(),
    )];
    let (status, body) = invoke(router, "DescribeVpnConnections", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeVpnConnections expected HTTP 200, got {status} with body {body}"
    );
    reported(case, &body)
        .into_iter()
        .find(|(id, _)| id == vpn_connection_id)
        .map(|(_, routes)| routes)
        .unwrap_or_else(|| panic!("case {case:?}: no {vpn_connection_id} in {body}"))
}

/// One row of the create table: the block as the request spells it, and the block the
/// connection must end up holding.
struct RouteCase {
    /// What makes this row a different input.
    case: &'static str,
    /// `DestinationCidrBlock` as the request sends it.
    requested: &'static str,
    /// The block the route must carry afterwards.
    stored: &'static str,
}

/// The blocks a create is exercised with: both families, a block whose host bits are
/// already clear and one whose are not, a single-address route, a whole-internet route and
/// an IPv6 prefix. The canonical form is the one every CIDR member of this service stores
/// ("We modify the specified CIDR block to its canonical form",
/// `CreateVpcRequest$CidrBlock` / `CreateSubnetRequest$CidrBlock`).
fn route_cases() -> Vec<RouteCase> {
    vec![
        RouteCase {
            case: "an IPv4 /24 already in canonical form",
            requested: "192.168.100.0/24",
            stored: "192.168.100.0/24",
        },
        RouteCase {
            case: "an IPv4 /24 with its host bits set",
            requested: "10.0.1.5/24",
            stored: "10.0.1.0/24",
        },
        RouteCase {
            case: "a single-address IPv4 route",
            requested: "203.0.113.7/32",
            stored: "203.0.113.7/32",
        },
        RouteCase {
            case: "the whole IPv4 address space",
            requested: "0.0.0.0/0",
            stored: "0.0.0.0/0",
        },
        RouteCase {
            case: "an IPv6 /64",
            requested: "2001:db8:abcd:1234::/64",
            stored: "2001:db8:abcd:1234::/64",
        },
        RouteCase {
            case: "an IPv6 /32 with its host bits set",
            requested: "2001:db8::1/32",
            stored: "2001:db8::/32",
        },
    ]
}

/// A create puts the block into the connection's own route set, as a `Static` route in the
/// transient `pending` state, and `DescribeVpnConnections` reports exactly that. The clock
/// then owes the route `available`, whether or not anybody asks again.
#[tokio::test]
async fn a_created_static_route_is_stored_on_the_connection_and_reported_by_describe() {
    for RouteCase {
        case,
        requested,
        stored,
    } in route_cases()
    {
        let (router, engine) = super::ec2_query::emulator();
        let vpn_connection_id = seed(&engine, VPN_STATE_AVAILABLE);

        let (status, body) = invoke(
            &router,
            "CreateVpnConnectionRoute",
            &route_params(&vpn_connection_id, requested),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );

        let expected = vec![(
            stored.to_string(),
            VPN_STATIC_ROUTE_SOURCE_STATIC.to_string(),
            VPN_STATE_PENDING.to_string(),
        )];
        let actual = stored_routes(&engine, &vpn_connection_id);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the store to hold {expected:?}, got {actual:?}"
        );
        let actual = described_routes(case, &router, &vpn_connection_id).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected DescribeVpnConnections to report {expected:?}, \
             got {actual:?}"
        );

        // The move the clock owed falls due on its own, at the span every transition of
        // this store takes; the next call to reach the store materialises it.
        tokio::time::sleep(past_the_transition()).await;
        let expected = vec![(
            stored.to_string(),
            VPN_STATIC_ROUTE_SOURCE_STATIC.to_string(),
            VPN_STATE_AVAILABLE.to_string(),
        )];
        let actual = described_routes(case, &router, &vpn_connection_id).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: the clock owed this route its available state, got {actual:?}"
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
/// `InvalidVpnConnectionId`, whichever form the id takes — and an empty id is a missing
/// parameter, not a lookup that failed. Nothing is written either way: the connection the
/// account DOES hold is untouched.
#[tokio::test]
async fn a_connection_the_account_does_not_hold_is_refused_and_writes_nothing() {
    // The first row is the id the recorded request named.
    let cases = [
        RefusalCase {
            case: "a long-form id nothing holds",
            value: "vpn-00000000000000001",
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
    for RefusalCase { case, value, code } in cases {
        let (status, body) = invoke(
            &router,
            "CreateVpnConnectionRoute",
            &route_params(value, "192.168.0.0/24"),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let actual = error_code(&body);
        assert_eq!(
            actual, code,
            "case {case:?}: expected the code {code}, got {actual} in {body}"
        );
        let actual = stored_routes(&engine, &held);
        assert!(
            actual.is_empty(),
            "case {case:?}: a refused create must write nothing, got {actual:?}"
        );
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
    for RefusalCase { case, value, code } in cases {
        let (status, body) = invoke(
            &router,
            "CreateVpnConnectionRoute",
            &route_params(&vpn_connection_id, value),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let actual = error_code(&body);
        assert_eq!(
            actual, code,
            "case {case:?}: expected the code {code}, got {actual} in {body}"
        );
        let actual = stored_routes(&engine, &vpn_connection_id);
        assert!(
            actual.is_empty(),
            "case {case:?}: a refused create must write nothing, got {actual:?}"
        );
    }
}

/// Which `VpnState` a connection is in decides whether it can take a route: one being
/// brought up or in service can, one being or already torn down cannot — and that refusal
/// writes nothing.
#[tokio::test]
async fn whether_a_connection_can_take_a_route_follows_its_state() {
    // Every member of the modelled `VpnState` enum
    // (`smithy_query(query="shape",shape="VpnState")`), with the status its create answers.
    let cases = [
        (VPN_STATE_PENDING, 200, ""),
        (VPN_STATE_AVAILABLE, 200, ""),
        (VPN_STATE_DELETING, 400, INCORRECT_STATE),
        (VPN_STATE_DELETED, 400, INCORRECT_STATE),
    ];
    for (state, expected_status, expected_code) in cases {
        let case = format!("a {state} connection");
        let (router, engine) = super::ec2_query::emulator();
        let vpn_connection_id = seed(&engine, state);

        let (status, body) = invoke(
            &router,
            "CreateVpnConnectionRoute",
            &route_params(&vpn_connection_id, "172.16.0.0/16"),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let routes = stored_routes(&engine, &vpn_connection_id);
        if expected_status == 200 {
            assert_eq!(
                routes.len(),
                1,
                "case {case:?}: expected the route to be stored, got {routes:?}"
            );
        } else {
            let actual = error_code(&body);
            assert_eq!(
                actual, expected_code,
                "case {case:?}: expected the code {expected_code}, got {actual} in {body}"
            );
            assert!(
                routes.is_empty(),
                "case {case:?}: a refused create must write nothing, got {routes:?}"
            );
        }
    }
}

/// A route of a connection is identified by its destination block, so repeating the create
/// — with the same spelling or with another spelling of the same block — leaves ONE route
/// and does not push the move the clock already owes it any further out.
#[tokio::test]
async fn repeating_a_create_leaves_one_route() {
    let (router, engine) = super::ec2_query::emulator();
    let vpn_connection_id = seed(&engine, VPN_STATE_AVAILABLE);

    // The same block three ways: canonical, repeated verbatim, and with host bits set.
    for spelling in ["10.0.2.0/24", "10.0.2.0/24", "10.0.2.9/24"] {
        let case = format!("DestinationCidrBlock={spelling}");
        let (status, body) = invoke(
            &router,
            "CreateVpnConnectionRoute",
            &route_params(&vpn_connection_id, spelling),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let actual = stored_routes(&engine, &vpn_connection_id);
        assert_eq!(
            actual.len(),
            1,
            "case {case:?}: expected exactly one route, got {actual:?}"
        );
    }

    // The move was owed from the FIRST create, so it is due at the same instant however
    // many repeats followed.
    tokio::time::sleep(past_the_transition()).await;
    let actual = described_routes("after the repeats", &router, &vpn_connection_id).await;
    assert_eq!(
        actual,
        vec![(
            "10.0.2.0/24".to_string(),
            VPN_STATIC_ROUTE_SOURCE_STATIC.to_string(),
            VPN_STATE_AVAILABLE.to_string(),
        )],
        "the clock owed the one route its available state, got {actual:?}"
    );
}

/// The store holds a bounded number of routes per connection: a create for a block the
/// connection does not have is refused once that bound is reached, while a create for a
/// block it already holds adds none and so is still answered.
#[tokio::test]
async fn a_full_connection_refuses_a_new_block_but_still_answers_one_it_holds() {
    let (router, engine) = super::ec2_query::emulator();
    let held: Vec<VpnStaticRoute> = (0..MAX_VPN_STATIC_ROUTES)
        .map(|index| VpnStaticRoute {
            destination_cidr_block: Some(format!("10.{index}.0.0/24")),
            source: Some(VPN_STATIC_ROUTE_SOURCE_STATIC.to_string()),
            state: Some(VPN_STATE_AVAILABLE.to_string()),
        })
        .collect();
    let vpn_connection_id = engine
        .state::<Ec2State>()
        .lock()
        .insert_vpn_connection(connection(VPN_STATE_AVAILABLE, held))
        .expect("the store holds the seeded VPN connection");

    let (status, body) = invoke(
        &router,
        "CreateVpnConnectionRoute",
        &route_params(&vpn_connection_id, "192.168.250.0/24"),
    )
    .await;
    assert_eq!(
        status, 400,
        "a block beyond the ceiling: expected HTTP 400, got {status} with body {body}"
    );
    let actual = error_code(&body);
    assert_eq!(
        actual, ROUTE_LIMIT_EXCEEDED,
        "a block beyond the ceiling: expected {ROUTE_LIMIT_EXCEEDED}, got {actual} in {body}"
    );
    let actual = stored_routes(&engine, &vpn_connection_id).len();
    assert_eq!(
        actual, MAX_VPN_STATIC_ROUTES,
        "a refused create must write nothing, got {actual} routes"
    );

    let (status, body) = invoke(
        &router,
        "CreateVpnConnectionRoute",
        &route_params(&vpn_connection_id, "10.0.0.0/24"),
    )
    .await;
    assert_eq!(
        status, 200,
        "a block the connection already holds: expected HTTP 200, got {status} with {body}"
    );
    let actual = stored_routes(&engine, &vpn_connection_id).len();
    assert_eq!(
        actual, MAX_VPN_STATIC_ROUTES,
        "a block the connection already holds must not be stored twice, got {actual} routes"
    );
}

/// `DescribeVpnConnections` selects on what the connections carry: the route a create wrote
/// is matched by `route.destination-cidr-block`, an id names one connection, and a filter
/// name the operation does not document is refused rather than read as "no results".
#[tokio::test]
async fn describe_selects_connections_by_their_routes_and_ids() {
    let (router, engine) = super::ec2_query::emulator();
    let with_route = seed(&engine, VPN_STATE_AVAILABLE);
    let without_route = seed(&engine, VPN_STATE_AVAILABLE);

    let (status, body) = invoke(
        &router,
        "CreateVpnConnectionRoute",
        &route_params(&with_route, "198.51.100.0/24"),
    )
    .await;
    assert_eq!(
        status, 200,
        "the create: expected HTTP 200, got {status} with body {body}"
    );

    // Both connections are reported when nothing narrows the request.
    let (status, body) = invoke(&router, "DescribeVpnConnections", &[]).await;
    assert_eq!(status, 200, "describe all: got {status} with body {body}");
    let ids: Vec<String> = reported("describe all", &body)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(
        ids,
        vec![with_route.clone(), without_route.clone()],
        "describe all: expected both connections in id order, got {ids:?}"
    );

    // The route filter selects only the connection that has the block.
    let cases: Vec<(&str, &str, Vec<String>)> = vec![
        (
            "the block that was created",
            "198.51.100.0/24",
            vec![with_route.clone()],
        ),
        ("a block nothing holds", "198.51.100.128/25", Vec::new()),
        (
            "an EC2 wildcard over the block",
            "198.51.100.*",
            vec![with_route.clone()],
        ),
    ];
    for (case, value, expected) in cases {
        let params = filters(&[("route.destination-cidr-block", &[value])]);
        let (status, body) = invoke(&router, "DescribeVpnConnections", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let ids: Vec<String> = reported(case, &body)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(
            ids, expected,
            "case {case:?}: expected {expected:?}, got {ids:?}"
        );
    }

    // An id the account holds no connection for is an error, not an absent row.
    let params = vec![(
        "VpnConnectionId.1".to_string(),
        "vpn-00000000000000001".to_string(),
    )];
    let (status, body) = invoke(&router, "DescribeVpnConnections", &params).await;
    assert_eq!(status, 400, "an unknown id: got {status} with body {body}");
    let actual = error_code(&body);
    assert_eq!(
        actual, INVALID_VPN_CONNECTION_ID,
        "an unknown id: expected {INVALID_VPN_CONNECTION_ID}, got {actual} in {body}"
    );

    // A filter name the operation does not document is refused.
    let params = filters(&[("route.destination", &["198.51.100.0/24"])]);
    let (status, body) = invoke(&router, "DescribeVpnConnections", &params).await;
    assert_eq!(
        status, 400,
        "an undocumented filter: got {status} with body {body}"
    );
    let actual = error_code(&body);
    assert_eq!(
        actual, INVALID_PARAMETER_VALUE,
        "an undocumented filter: expected {INVALID_PARAMETER_VALUE}, got {actual} in {body}"
    );

    // An authorized dry run reports that the call WOULD have been permitted and reports no
    // connection: "If the request would succeed, but you do not have the required
    // permissions, the error response is DryRunOperation" (`DescribeVpnConnectionsRequest$
    // DryRun`), which EC2 answers with a precondition status rather than a result.
    let params = vec![("DryRun".to_string(), "true".to_string())];
    let (status, body) = invoke(&router, "DescribeVpnConnections", &params).await;
    assert_eq!(status, 412, "a dry run: got {status} with body {body}");
    let actual = error_code(&body);
    assert_eq!(
        actual, DRY_RUN_OPERATION,
        "a dry run: expected {DRY_RUN_OPERATION}, got {actual} in {body}"
    );
}

/// Every filter name `DescribeVpnConnections` documents is accepted and compares against
/// what the connection carries: a name whose attribute the connection has selects it, and
/// a documented name nothing is stored for selects nothing — a 200 with no row, never a
/// refusal and never every connection.
///
/// The names are the operation's own documented list
/// (`smithy_query(query="doc",shape="DescribeVpnConnectionsRequest",member="Filters")`),
/// every one of them, so a name the handler accepts without evaluating is visible here
/// rather than only in the handler. Two of them are stored for no connection of this
/// store: `transit-gateway-id` on a connection attached to a virtual private gateway, and
/// `bgp-asn`, which is a fact of the CUSTOMER GATEWAY — the modelled `VpnConnection` shape
/// carries no member for it, so the handler resolves the connection's `CustomerGatewayId` in
/// the customer gateway store, and this suite seeds no gateway there, which makes it an
/// attribute the connection carries no value for. The gateway that DOES carry one is seeded
/// by `tests/all/customer_gateways.rs::the_bgp_asn_filter_of_a_vpn_connection_resolves_its_
/// customer_gateway`.
#[tokio::test]
async fn describe_answers_every_documented_filter_name() {
    let (router, engine) = super::ec2_query::emulator();
    let vpn_connection_id = seed(&engine, VPN_STATE_AVAILABLE);
    let route = "203.0.113.0/24";
    let (status, body) = invoke(
        &router,
        "CreateVpnConnectionRoute",
        &route_params(&vpn_connection_id, route),
    )
    .await;
    assert_eq!(
        status, 200,
        "the create: expected HTTP 200, got {status} with body {body}"
    );

    // One row per documented name: the value the request sends, and whether the seeded
    // connection carries that attribute.
    let selected = vec![vpn_connection_id.clone()];
    let cases: Vec<(&str, &str, Vec<String>)> = vec![
        ("bgp-asn", "65000", Vec::new()),
        (
            "customer-gateway-configuration",
            "<vpn_connection/>",
            selected.clone(),
        ),
        (
            "customer-gateway-id",
            "cgw-0123456789abcdef0",
            selected.clone(),
        ),
        ("option.static-routes-only", "true", selected.clone()),
        ("route.destination-cidr-block", route, selected.clone()),
        ("state", VPN_STATE_AVAILABLE, selected.clone()),
        ("tag-key", "Name", selected.clone()),
        ("tag:Name", "branch-office", selected.clone()),
        ("transit-gateway-id", "tgw-0123456789abcdef0", Vec::new()),
        ("type", "ipsec.1", selected.clone()),
        ("vpn-connection-id", &vpn_connection_id, selected.clone()),
        ("vpn-gateway-id", "vgw-0123456789abcdef0", selected.clone()),
    ];

    for (name, value, expected) in cases {
        let case = format!("the filter {name}={value}");
        let params = filters(&[(name, &[value])]);
        let (status, body) = invoke(&router, "DescribeVpnConnections", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let ids: Vec<String> = reported(&case, &body)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(
            ids, expected,
            "case {case:?}: expected {expected:?}, got {ids:?}"
        );
    }
}

/// The request-controlled factors of the matching cost are capped, the way every sibling
/// describe of this crate caps them: more filters than `MAX_FILTERS`, more values in one
/// filter than `MAX_FILTER_VALUES`, a value longer than `MAX_FILTER_VALUE_BYTES` and more
/// ids than `MAX_RESOURCE_IDS` are each refused with `InvalidParameterValue`, and the bound
/// itself is accepted.
#[tokio::test]
async fn describe_caps_the_filter_and_id_factors_of_its_matching_cost() {
    let (router, engine) = super::ec2_query::emulator();
    let vpn_connection_id = seed(&engine, VPN_STATE_AVAILABLE);

    // One row per factor, each at its published bound and one past it.
    let at_the_bound = vec![("state", &[VPN_STATE_AVAILABLE][..]); MAX_FILTERS];
    let past_the_bound = vec![("state", &[VPN_STATE_AVAILABLE][..]); MAX_FILTERS + 1];
    let many_values: Vec<&str> = vec![VPN_STATE_AVAILABLE; MAX_FILTER_VALUES];
    let too_many_values: Vec<&str> = vec![VPN_STATE_AVAILABLE; MAX_FILTER_VALUES + 1];
    let long_value = "a".repeat(MAX_FILTER_VALUE_BYTES);
    let longer_value = "a".repeat(MAX_FILTER_VALUE_BYTES + 1);
    let cases: Vec<BoundCase> = vec![
        (
            format!("{MAX_FILTERS} filters"),
            filters(&at_the_bound),
            200,
        ),
        (
            format!("{} filters", MAX_FILTERS + 1),
            filters(&past_the_bound),
            400,
        ),
        (
            format!("{MAX_FILTER_VALUES} values in one filter"),
            filters(&[("state", &many_values)]),
            200,
        ),
        (
            format!("{} values in one filter", MAX_FILTER_VALUES + 1),
            filters(&[("state", &too_many_values)]),
            400,
        ),
        (
            format!("a {MAX_FILTER_VALUE_BYTES}-byte value"),
            filters(&[("state", &[long_value.as_str()])]),
            200,
        ),
        (
            format!("a {}-byte value", MAX_FILTER_VALUE_BYTES + 1),
            filters(&[("state", &[longer_value.as_str()])]),
            400,
        ),
        (
            format!("{MAX_RESOURCE_IDS} ids"),
            indexed(
                "VpnConnectionId",
                &vec![vpn_connection_id.as_str(); MAX_RESOURCE_IDS],
            ),
            200,
        ),
        (
            format!("{} ids", MAX_RESOURCE_IDS + 1),
            indexed(
                "VpnConnectionId",
                &vec![vpn_connection_id.as_str(); MAX_RESOURCE_IDS + 1],
            ),
            400,
        ),
    ];

    for (case, params, expected_status) in cases {
        let (status, body) = invoke(&router, "DescribeVpnConnections", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 400 {
            let actual = error_code(&body);
            assert_eq!(
                actual, INVALID_PARAMETER_VALUE,
                "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got {actual} in {body}"
            );
        }
    }
}
