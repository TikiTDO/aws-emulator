// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteLocalGatewayRouteTable` and the
//! `DescribeLocalGatewayRouteTables` that reads the deleted table back, driven over the wire
//! the service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST
//! whose body carries `Action=<Operation>`, answered with XML (README.md / OPERATIONS.md
//! "Wire protocol").
//!
//! The rule under test: a delete ANSWERS THE STORED RECORD the request names, moved into the
//! non-terminal `deleting` state with the clock owing it the terminal `deleted` — so a caller
//! reading straight after the call still sees the delete in progress, and
//! `DescribeLocalGatewayRouteTables` reports the very state and members the delete answered
//! with; and a delete the service cannot carry out is refused with that class's own code and
//! HTTP status while every stored record is left exactly where it was — a value that is not a
//! route table id at all before anything is looked up, a well-formed id the account holds no
//! table for, a parameter the request did not carry, a request that only asks about
//! permission, a table an association still holds, and a table a delete already took down.
//!
//! Each row of the success table is a distinct input along what that rule generalises over:
//! which mode the table was created in (both members of the model's own catalog, and a create
//! that named none), which Region the calls are signed for, how many tags the table carries,
//! whether the delete arrives while the table is still `pending` or once the clock has made
//! it `available`, and whether the table holds static routes. The refusal rows are one per way
//! a named route table id can fail to resolve, one per way the id can be absent, one per kind
//! of association that holds a table, and one per dry run.

use std::time::{Duration, Instant};

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DEPENDENCY_VIOLATION, DRY_RUN_OPERATION,
    INCORRECT_STATE, INVALID_LOCAL_GATEWAY_ROUTE_TABLE_ID_MALFORMED,
    INVALID_LOCAL_GATEWAY_ROUTE_TABLE_ID_NOT_FOUND,
    LOCAL_GATEWAY_ROUTE_TABLE_MODE_DIRECT_VPC_ROUTING, LOCAL_GATEWAY_ROUTE_TABLE_STATE_AVAILABLE,
    LOCAL_GATEWAY_ROUTE_TABLE_STATE_DELETED, LOCAL_GATEWAY_ROUTE_TABLE_STATE_DELETING,
    LOCAL_GATEWAY_ROUTE_TABLE_STATE_PENDING, LocalGatewaySpec, MISSING_PARAMETER,
    RESOURCE_TYPE_LOCAL_GATEWAY_ROUTE_TABLE, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, indexed};

/// The Region most calls here are signed for: a route table's ARN names the Region its
/// creating call was signed for, which is why one row drives [`OTHER_REGION`].
const REGION: &str = "us-east-1";

/// The second Region, so what a delete answers is visibly the stored table's rather than a
/// constant of this suite.
const OTHER_REGION: &str = "us-west-2";

/// The ec2Query action under test.
const DELETE: &str = "DeleteLocalGatewayRouteTable";

/// The create this suite puts the tables it deletes into the store through.
const CREATE: &str = "CreateLocalGatewayRouteTable";

/// The reader this suite proves a deleted table is still reported through.
const DESCRIBE: &str = "DescribeLocalGatewayRouteTables";

/// The XML element each route table's id arrives in
/// (`LocalGatewayRouteTable$LocalGatewayRouteTableId`'s `xmlName`).
const ID_ELEMENT: &str = "localGatewayRouteTableId";

/// The HTTP status EC2 answers `DryRunOperation` with: the request was refused because a
/// precondition the CALLER set holds, not because anything was wrong with it.
const DRY_RUN_STATUS: u16 = 412;

/// One form field.
fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// One `action` call signed for `region`, as its HTTP status and XML body.
async fn invoke_in(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, region, action, params).await
}

/// One `action` call signed for [`REGION`].
async fn invoke(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    invoke_in(router, REGION, action, params).await
}

/// The same call, requiring HTTP 200, and the body it answered.
async fn invoke_ok(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[(String, String)],
) -> String {
    let (status, body) = invoke_in(router, region, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// The one `LocalGatewayRouteTableId=` request member a delete takes.
fn table_param(route_table_id: &str) -> Vec<(String, String)> {
    vec![param("LocalGatewayRouteTableId", route_table_id)]
}

/// The ARN of the Outpost `name`, as `LocalGateway$OutpostArn` carries it.
fn outpost_arn(name: &str) -> String {
    format!(
        "arn:aws:outposts:{REGION}:{}:outpost/{name}",
        engine::emulator_account_id()
    )
}

/// The ARN of a local gateway route table, as
/// `LocalGatewayRouteTable$LocalGatewayRouteTableArn` carries it:
/// `arn:aws:ec2:<region>:<account>:local-gateway-route-table/<id>`.
fn route_table_arn(region: &str, route_table_id: &str) -> String {
    format!(
        "arn:aws:ec2:{region}:{}:local-gateway-route-table/{route_table_id}",
        engine::emulator_account_id()
    )
}

/// One local gateway on the Outpost `outpost`, written straight through the store's own seam:
/// nothing in the EC2 API this emulator serves creates an Outpost, so a gateway is placed the
/// way an Outpost order would place it.
fn seed_gateway(engine: &engine::InMemoryEngine, outpost: &str) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_local_gateway(LocalGatewaySpec {
            outpost_arn: outpost_arn(outpost),
            state: "available".to_string(),
            tags: Vec::new(),
        })
        .unwrap_or_else(|| panic!("the store must take the local gateway on {outpost:?}"))
}

/// The form fields for one `TagSpecification` naming the resource type a create of a route
/// table may tag and `tags`.
fn tag_specification(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut fields = vec![param(
        "TagSpecification.1.ResourceType",
        RESOURCE_TYPE_LOCAL_GATEWAY_ROUTE_TABLE,
    )];
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        fields.push(param(&format!("TagSpecification.1.Tag.{position}.Key"), key));
        fields.push(param(
            &format!("TagSpecification.1.Tag.{position}.Value"),
            value,
        ));
    }
    fields
}

/// One route table created on `gateway_id` through the wire, in `region`, with `mode` (`None`
/// for a request that names none) and `tags`: the id the create minted.
async fn create_table(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    gateway_id: &str,
    mode: Option<&str>,
    tags: &[(&str, &str)],
) -> String {
    let mut params = vec![param("LocalGatewayId", gateway_id)];
    if let Some(mode) = mode {
        params.push(param("Mode", mode));
    }
    if !tags.is_empty() {
        params.extend(tag_specification(tags));
    }
    let body = invoke_ok(case, router, region, CREATE, &params).await;
    element(&body, ID_ELEMENT).unwrap_or_else(|| panic!("case {case:?}: no <{ID_ELEMENT}> in {body}"))
}

/// The state `DescribeLocalGatewayRouteTables` reports for `route_table_id` right now, with
/// the describe required to report that table and nothing else.
async fn described_state(case: &str, router: &MultiServiceRouter, route_table_id: &str) -> String {
    let body = invoke_ok(
        case,
        router,
        REGION,
        DESCRIBE,
        &indexed("LocalGatewayRouteTableId", &[route_table_id]),
    )
    .await;
    assert_eq!(
        elements(&body, ID_ELEMENT),
        vec![route_table_id.to_string()],
        "case {case:?}: expected {DESCRIBE} to report the table the request named, got {body}"
    );
    element(&body, "state").unwrap_or_default()
}

/// The state `DescribeLocalGatewayRouteTables` reports for `route_table_id` once it is
/// `wanted`: the state is looked at AGAIN in a loop until it has moved or a deadline far past
/// [`TRANSITION_DELAY`] passes, so the wait is for the CONDITION rather than for a span of the
/// clock.
async fn described_state_once(
    case: &str,
    router: &MultiServiceRouter,
    route_table_id: &str,
    wanted: &str,
) -> String {
    let deadline = Instant::now() + TRANSITION_DELAY * 40;
    loop {
        let state = described_state(case, router, route_table_id).await;
        if state == wanted || Instant::now() >= deadline {
            break state;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Every route table id the store holds, in the store's own stable order: a refused delete
/// removes none of them and adds none.
///
/// The ids rather than (id, state) pairs: the state of a stored table also moves on the CLOCK
/// (`pending` -> `available` one `TRANSITION_DELAY` after its create, whether or not anybody
/// calls), so a pair-wise comparison across a run of calls would fail for a move the clock
/// owed rather than for a write the refused delete made. What a refused delete must not do to
/// the state is checked by [`no_table_is_being_deleted`], which is the invariant this suite
/// means.
fn stored_table_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .local_gateway_route_tables
        .keys()
        .cloned()
        .collect()
}

/// Every stored route table that is in one of the two delete states: empty for a store no
/// delete has written.
fn tables_being_deleted(engine: &engine::InMemoryEngine) -> Vec<(String, String)> {
    engine
        .state::<Ec2State>()
        .lock()
        .local_gateway_route_tables
        .iter()
        .filter_map(|(route_table_id, record)| {
            let state = record.view.state.clone().unwrap_or_default();
            (state == LOCAL_GATEWAY_ROUTE_TABLE_STATE_DELETING
                || state == LOCAL_GATEWAY_ROUTE_TABLE_STATE_DELETED)
                .then_some((route_table_id.clone(), state))
        })
        .collect()
}

/// Assert that `body` carries `<member>value</member>`.
fn assert_member(case: &str, body: &str, member: &str, value: &str) {
    assert!(
        body.contains(&format!("<{member}>{value}</{member}>")),
        "case {case:?}: expected <{member}>{value}</{member}>, got {body}"
    );
}

/// The sentence EC2 words `IncorrectState` with for a route table in `state`.
fn incorrect_state_message(route_table_id: &str, state: &str) -> String {
    format!("{RESOURCE_TYPE_LOCAL_GATEWAY_ROUTE_TABLE} '{route_table_id}' is in the '{state}' state.")
}

// ---------------------------------------------------------------------------
// A delete answers the stored table in `deleting`, and the clock owes it `deleted`
// ---------------------------------------------------------------------------

/// One delete, as a row of the table.
struct Case {
    /// The case name every assertion of the row quotes.
    name: &'static str,
    /// The Region the row's calls are signed for.
    region: &'static str,
    /// The `Mode` the create named; `None` for a create that named none, whose table is in
    /// the mode the create writes by default.
    mode: Option<&'static str>,
    /// The tags the deleted table was created with.
    tags: Vec<(&'static str, &'static str)>,
    /// Whether the table is left to reach `available` before the delete arrives.
    deleted_once_available: bool,
    /// The destinations of the static routes the table holds when the delete arrives.
    routes: Vec<&'static str>,
}

/// One (key, value) tag pair, as the rows spell their tags.
fn tag(key: &'static str, value: &'static str) -> (&'static str, &'static str) {
    (key, value)
}

/// The deletes, spanning what the rule generalises over: both members of the model's own
/// `LocalGatewayRouteTableMode` catalog (`smithy_query(query="shape",
/// shape="LocalGatewayRouteTableMode")`: `direct-vpc-routing`, `coip`) and a create that named
/// none, two Regions, none / one / two tags, a table deleted while still `pending` and one
/// deleted once the clock has made it `available`, and a table holding no route beside one
/// holding two.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "coip, no tags, deleted while still pending, holding no route",
            region: REGION,
            mode: Some("coip"),
            tags: Vec::new(),
            deleted_once_available: false,
            routes: Vec::new(),
        },
        Case {
            name: "direct-vpc-routing, one tag, deleted once available, holding one route",
            region: REGION,
            mode: Some("direct-vpc-routing"),
            tags: vec![tag("Name", "rack-a")],
            deleted_once_available: true,
            routes: vec!["10.0.1.0/24"],
        },
        Case {
            name: "no mode named, two tags, deleted while pending, holding two routes",
            region: REGION,
            mode: None,
            tags: vec![tag("Name", "rack-b"), tag("Owner", "TeamB")],
            deleted_once_available: false,
            routes: vec!["10.0.2.0/24", "2600:1f00::/56"],
        },
        Case {
            name: "another Region, coip, one tag, deleted once available",
            region: OTHER_REGION,
            mode: Some("coip"),
            tags: vec![tag("Name", "rack-c")],
            deleted_once_available: true,
            routes: Vec::new(),
        },
    ]
}

/// A delete answers the table the request named as the STORE holds it, in the non-terminal
/// `deleting` state; the describe reports that same state and those same members; the clock
/// then owes the table the terminal `deleted` whether or not anybody calls again; the other
/// table of the same gateway is left where it was; and a second delete of a table a delete
/// already took down is refused for the state that table is in.
#[tokio::test]
async fn a_delete_answers_the_stored_table_in_deleting_and_the_clock_owes_it_deleted() {
    for case in cases() {
        let Case {
            name,
            region,
            mode,
            tags,
            deleted_once_available,
            routes,
        } = case;
        let (router, engine) = super::ec2_query::emulator();
        let gateway_id = seed_gateway(&engine, "op-00000000000000001");
        let outpost = outpost_arn("op-00000000000000001");
        let expected_mode = mode.unwrap_or(LOCAL_GATEWAY_ROUTE_TABLE_MODE_DIRECT_VPC_ROUTING);

        let route_table_id = create_table(name, &router, region, &gateway_id, mode, &tags).await;
        // A SECOND table of the same gateway, so each row shows the delete acting on the one
        // table the request named.
        let untouched = create_table(name, &router, region, &gateway_id, Some("coip"), &[]).await;

        // The static routes the row's table holds when the delete arrives, each aimed at a
        // virtual interface group of the same gateway: routes are part of the TABLE, so they
        // do not stand in the way of taking it down.
        if !routes.is_empty() {
            let group_body = invoke_ok(
                name,
                &router,
                region,
                "CreateLocalGatewayVirtualInterfaceGroup",
                &[param("LocalGatewayId", &gateway_id)],
            )
            .await;
            let group_id = element(&group_body, "localGatewayVirtualInterfaceGroupId")
                .unwrap_or_else(|| panic!("case {name:?}: no virtual interface group id in {group_body}"));
            for destination in &routes {
                invoke_ok(
                    name,
                    &router,
                    region,
                    "CreateLocalGatewayRoute",
                    &[
                        param("LocalGatewayRouteTableId", &route_table_id),
                        param("DestinationCidrBlock", destination),
                        param("LocalGatewayVirtualInterfaceGroupId", &group_id),
                    ],
                )
                .await;
            }
        }

        if deleted_once_available {
            let state = described_state_once(
                name,
                &router,
                &route_table_id,
                LOCAL_GATEWAY_ROUTE_TABLE_STATE_AVAILABLE,
            )
            .await;
            assert_eq!(
                state, LOCAL_GATEWAY_ROUTE_TABLE_STATE_AVAILABLE,
                "case {name:?}: the move the clock owed the created table must have landed \
                 before the delete, got {state:?}"
            );
        } else {
            let state = described_state(name, &router, &route_table_id).await;
            assert_eq!(
                state, LOCAL_GATEWAY_ROUTE_TABLE_STATE_PENDING,
                "case {name:?}: a table whose create has just answered is still in the \
                 transient state, got {state:?}"
            );
        }

        let (status, body) = invoke_in(&router, region, DELETE, &table_param(&route_table_id)).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 from {DELETE}, got {status} with body {body}"
        );

        // The answer is the STORED record, in the state a delete puts it into.
        assert_eq!(
            element(&body, ID_ELEMENT).as_deref(),
            Some(route_table_id.as_str()),
            "case {name:?}: expected the table the request named, got {body}"
        );
        for (member, value) in [
            (
                "state",
                LOCAL_GATEWAY_ROUTE_TABLE_STATE_DELETING.to_string(),
            ),
            (
                "localGatewayRouteTableArn",
                route_table_arn(region, &route_table_id),
            ),
            ("localGatewayId", gateway_id.clone()),
            ("outpostArn", outpost.clone()),
            ("ownerId", engine::emulator_account_id().to_string()),
            ("mode", expected_mode.to_string()),
        ] {
            assert_member(name, &body, member, &value);
        }
        for (key, value) in &tags {
            assert_member(name, &body, "key", key);
            assert_member(name, &body, "value", value);
        }

        // The round trip: the describe reports the very state and members the delete answered
        // with, so the delete wrote the store rather than only composing an answer.
        let read = invoke_ok(
            name,
            &router,
            REGION,
            DESCRIBE,
            &indexed("LocalGatewayRouteTableId", &[route_table_id.as_str()]),
        )
        .await;
        for (member, value) in [
            (
                "state",
                LOCAL_GATEWAY_ROUTE_TABLE_STATE_DELETING.to_string(),
            ),
            (
                "localGatewayRouteTableArn",
                route_table_arn(region, &route_table_id),
            ),
            ("localGatewayId", gateway_id.clone()),
            ("outpostArn", outpost.clone()),
            ("mode", expected_mode.to_string()),
        ] {
            assert_member(name, &read, member, &value);
        }

        // A second delete of a table a delete already took down has nothing left to take
        // down: it is refused for the state that table is in, in whichever of the two delete
        // states the clock has left it.
        let (second_status, second_body) =
            invoke_in(&router, region, DELETE, &table_param(&route_table_id)).await;
        assert_eq!(
            second_status, 400,
            "case {name:?}: a second delete must be refused, got {second_status} with body \
             {second_body}"
        );
        assert_eq!(
            error_code(&second_body),
            INCORRECT_STATE,
            "case {name:?}: a second delete must be refused for the table's state, got \
             {second_body}"
        );
        let refused_message = error_message(&second_body);
        assert!(
            [
                LOCAL_GATEWAY_ROUTE_TABLE_STATE_DELETING,
                LOCAL_GATEWAY_ROUTE_TABLE_STATE_DELETED,
            ]
            .iter()
            .any(|state| refused_message == incorrect_state_message(&route_table_id, state)),
            "case {name:?}: the refusal must name the table and the delete state it is in, \
             got {refused_message:?}"
        );

        // The move the clock owes the deleted table lands whether or not anybody calls again,
        // and the record is still reported while it happens.
        let settled = described_state_once(
            name,
            &router,
            &route_table_id,
            LOCAL_GATEWAY_ROUTE_TABLE_STATE_DELETED,
        )
        .await;
        assert_eq!(
            settled, LOCAL_GATEWAY_ROUTE_TABLE_STATE_DELETED,
            "case {name:?}: the state the clock owes a deleted table must have landed, got \
             {settled:?}"
        );

        // The other table of the same gateway was never in a delete state.
        let neighbour = described_state(name, &router, &untouched).await;
        assert!(
            neighbour != LOCAL_GATEWAY_ROUTE_TABLE_STATE_DELETING
                && neighbour != LOCAL_GATEWAY_ROUTE_TABLE_STATE_DELETED,
            "case {name:?}: the delete must leave the other table of the gateway alone, and \
             it is in {neighbour:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A delete the service cannot carry out is refused, and changes nothing
// ---------------------------------------------------------------------------

/// Every way the delete refuses a request is answered with that class's own code and HTTP
/// status — never with a 501 or an empty success — and no stored route table is touched: a
/// value that is not a route table id of this kind, a well-formed id the account holds no
/// table for, an id the request did not carry at all, a table a VPC association or a virtual
/// interface group association still holds, and a request that only asks about permission.
#[tokio::test]
async fn a_delete_it_cannot_carry_out_is_refused_and_changes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let gateway_id = seed_gateway(&engine, "op-00000000000000001");

    // One table per refusal that needs a stored table of its own, so no row can be satisfied
    // by another row's leftovers.
    let free = create_table("the dry run", &router, REGION, &gateway_id, Some("coip"), &[]).await;
    let vpc_held = create_table(
        "the VPC association",
        &router,
        REGION,
        &gateway_id,
        Some("direct-vpc-routing"),
        &[],
    )
    .await;
    let group_held = create_table(
        "the virtual interface group association",
        &router,
        REGION,
        &gateway_id,
        Some("coip"),
        &[],
    )
    .await;

    // The VPC association that holds a table: the VPC is created through the wire, because an
    // association is refused for a VPC the account does not hold.
    let vpc_body = invoke_ok(
        "the VPC association",
        &router,
        REGION,
        "CreateVpc",
        &[param("CidrBlock", "10.9.0.0/16")],
    )
    .await;
    let vpc_id =
        element(&vpc_body, "vpcId").unwrap_or_else(|| panic!("no <vpcId> in {vpc_body}"));
    let vpc_association_body = invoke_ok(
        "the VPC association",
        &router,
        REGION,
        "CreateLocalGatewayRouteTableVpcAssociation",
        &[
            param("LocalGatewayRouteTableId", &vpc_held),
            param("VpcId", &vpc_id),
        ],
    )
    .await;
    let vpc_association_id = element(&vpc_association_body, "localGatewayRouteTableVpcAssociationId")
        .unwrap_or_else(|| panic!("no association id in {vpc_association_body}"));

    // The virtual interface group association that holds the other table.
    let group_body = invoke_ok(
        "the virtual interface group association",
        &router,
        REGION,
        "CreateLocalGatewayVirtualInterfaceGroup",
        &[param("LocalGatewayId", &gateway_id)],
    )
    .await;
    let group_id = element(&group_body, "localGatewayVirtualInterfaceGroupId")
        .unwrap_or_else(|| panic!("no group id in {group_body}"));
    let group_association_body = invoke_ok(
        "the virtual interface group association",
        &router,
        REGION,
        "CreateLocalGatewayRouteTableVirtualInterfaceGroupAssociation",
        &[
            param("LocalGatewayRouteTableId", &group_held),
            param("LocalGatewayVirtualInterfaceGroupId", &group_id),
        ],
    )
    .await;
    let group_association_id = element(
        &group_association_body,
        "localGatewayRouteTableVirtualInterfaceGroupAssociationId",
    )
    .unwrap_or_else(|| panic!("no association id in {group_association_body}"));

    let before = stored_table_ids(&engine);

    // (case, the request's fields, the HTTP status, the code and the message expected)
    type Refusal = (String, Vec<(String, String)>, u16, &'static str, String);
    let mut rows: Vec<Refusal> = Vec::new();

    // One row per way a value can fail to be a local gateway route table id: a prefix that is
    // not this resource's (including the one the recorded corpus carries, which runs the
    // resource's words together), no suffix at all, a hex suffix of neither published length,
    // a suffix that is not lowercase hex, and another resource's id form.
    for id in [
        "lgwrtb-00000000000000000",
        "lgw-rtb-",
        "lgw-rtb-0123456",
        "lgw-rtb-0123456789abcdef",
        "lgw-rtb-0123456789ABCDEF0",
        "lgw-rtb-not-an-id",
        "rtb-0123456789abcdef0",
        "lgw-0123456789abcdef0",
        "not-a-route-table-id",
    ] {
        rows.push((
            format!("a value that is not a route table id: {id:?}"),
            table_param(id),
            400,
            INVALID_LOCAL_GATEWAY_ROUTE_TABLE_ID_MALFORMED,
            format!("The {RESOURCE_TYPE_LOCAL_GATEWAY_ROUTE_TABLE} ID '{id}' is malformed"),
        ));
    }
    // The request's own FORM is read before the dry run is answered, so a malformed id is
    // refused even when the caller only asked about permission.
    rows.push((
        "a dry run naming a value that is not a route table id".to_string(),
        {
            let mut params = table_param("lgw-rtb-not-an-id");
            params.push(param("DryRun", "true"));
            params
        },
        400,
        INVALID_LOCAL_GATEWAY_ROUTE_TABLE_ID_MALFORMED,
        format!(
            "The {RESOURCE_TYPE_LOCAL_GATEWAY_ROUTE_TABLE} ID 'lgw-rtb-not-an-id' is malformed"
        ),
    ));
    // One row per published id length that names no table of the account — including the
    // all-zero id a client reaches for as a placeholder, which no mint ever issues.
    for id in [
        "lgw-rtb-0123456789abcdef0",
        "lgw-rtb-0000000f",
        "lgw-rtb-00000000000000000",
    ] {
        rows.push((
            format!("a well-formed id no create ever minted: {id:?}"),
            table_param(id),
            400,
            INVALID_LOCAL_GATEWAY_ROUTE_TABLE_ID_NOT_FOUND,
            format!("The LocalGateway Route Table ID '{id}' does not exist"),
        ));
    }
    // The id is `@required` but modelled `clientOptional`, so an EMPTY member reaches the
    // handler, which refuses it as a parameter the request did not carry; a member left out
    // ALTOGETHER is a null the boundary validator refuses before the handler, with the
    // framework's own `MalformedInput`, as every other operation of this service answers it.
    rows.push((
        "an empty LocalGatewayRouteTableId".to_string(),
        table_param(""),
        400,
        MISSING_PARAMETER,
        "The request must contain the parameter localGatewayRouteTableId".to_string(),
    ));
    rows.push((
        "no LocalGatewayRouteTableId at all".to_string(),
        Vec::new(),
        400,
        "MalformedInput",
        "1 validation error detected: Value null at 'localGatewayRouteTableId' failed to \
         satisfy constraint: Member must not be null"
            .to_string(),
    ));
    // A table an association still holds: taking it away would leave the association pointing
    // at nothing, so the refusal names the association that holds it.
    rows.push((
        "a table a VPC association still holds".to_string(),
        table_param(&vpc_held),
        400,
        DEPENDENCY_VIOLATION,
        format!(
            "The local gateway route table '{vpc_held}' has a dependent object: the VPC \
             association '{vpc_association_id}'"
        ),
    ));
    rows.push((
        "a table a virtual interface group association still holds".to_string(),
        table_param(&group_held),
        400,
        DEPENDENCY_VIOLATION,
        format!(
            "The local gateway route table '{group_held}' has a dependent object: the virtual \
             interface group association '{group_association_id}'"
        ),
    ));
    // A dry run is a question about PERMISSION, answered before anything is resolved: a table
    // the delete would have taken down and a well-formed id naming none are answered alike.
    for (case, id) in [
        ("a dry run of a delete that would have succeeded", free.as_str()),
        (
            "a dry run naming a well-formed id no create minted",
            "lgw-rtb-0123456789abcdef0",
        ),
    ] {
        rows.push((
            case.to_string(),
            {
                let mut params = table_param(id);
                params.push(param("DryRun", "true"));
                params
            },
            DRY_RUN_STATUS,
            DRY_RUN_OPERATION,
            "Request would have succeeded, but DryRun flag is set.".to_string(),
        ));
    }

    for (case, params, expected_status, expected_code, expected_message) in &rows {
        let (status, body) = invoke(&router, DELETE, params).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, *expected_code,
            "case {case:?}: expected the code {expected_code:?}, got {code:?} in {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            &message, expected_message,
            "case {case:?}: expected the message {expected_message:?}, got {message:?}"
        );
    }

    // Not one stored table was taken down: every row above was refused before anything was
    // written, so the store holds the same tables and none of them is in a delete state.
    let after = stored_table_ids(&engine);
    assert_eq!(
        after, before,
        "a refused delete must leave every stored route table in the store: expected \
         {before:?}, got {after:?}"
    );
    let being_deleted = tables_being_deleted(&engine);
    assert!(
        being_deleted.is_empty(),
        "a refused delete must put no table into a delete state, and these are in one: \
         {being_deleted:?}"
    );

    // The table the dry run named is still there to be deleted, which is what makes that
    // refusal a report about permission rather than about the table.
    let (status, body) = invoke(&router, DELETE, &table_param(&free)).await;
    assert_eq!(
        status, 200,
        "the table a dry run named must still be deletable, got {status} with body {body}"
    );
    assert_member(
        "the dry run",
        &body,
        "state",
        LOCAL_GATEWAY_ROUTE_TABLE_STATE_DELETING,
    );
}
