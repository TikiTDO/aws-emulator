// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateLocalGatewayVirtualInterfaceGroup` and the
//! `DescribeLocalGatewayVirtualInterfaceGroups` that reads one back, driven over the wire
//! the service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST
//! whose body carries `Action=<Operation>`, answered with XML (README.md / OPERATIONS.md
//! "Wire protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rule under test is that the create COMPUTES its answer from the store: it writes a
//! virtual interface group ON a local gateway the account HOLDS, mints the group id, its ARN
//! and the owning account itself, keeps the local BGP configuration the request asked for,
//! holds no virtual interface until one is created in it, answers the non-terminal `pending`
//! configuration state the clock owes `available`, and refuses a request naming a gateway
//! the account does not hold with that resource's own `InvalidLocalGatewayID.NotFound` (and
//! an id that is not a local gateway id at all with its `.Malformed`). Every table row below
//! is a distinct input along what that generalises over — which gateway, in which Region,
//! which of the two local-ASN members (including neither), how many tags, which resource
//! type a tag specification names, each way a `LocalGatewayId` can fail to resolve, and
//! whether the request only asks about permission.

use std::time::{Duration, Instant};

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DRY_RUN_OPERATION,
    INVALID_LOCAL_GATEWAY_ID_MALFORMED, INVALID_LOCAL_GATEWAY_ID_NOT_FOUND,
    INVALID_PARAMETER_VALUE, LOCAL_GATEWAY_VIF_GROUP_ID_PREFIX,
    LOCAL_GATEWAY_VIF_GROUP_STATE_AVAILABLE, LOCAL_GATEWAY_VIF_GROUP_STATE_PENDING,
    LOCAL_GATEWAY_VIF_ID_PREFIX, LocalGatewaySpec, MISSING_PARAMETER, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, indexed};

/// The Region most calls here are signed for: a group's own ARN names the Region its
/// creating call was signed for, which is why one row drives [`OTHER_REGION`].
const REGION: &str = "us-east-1";

/// The second Region, so the ARN a created group carries is visibly the signed Region's
/// rather than a constant.
const OTHER_REGION: &str = "us-west-2";

/// The ec2Query action under test.
const CREATE: &str = "CreateLocalGatewayVirtualInterfaceGroup";

/// The reader this suite proves a created group round-trips through.
const DESCRIBE: &str = "DescribeLocalGatewayVirtualInterfaceGroups";

/// The action that puts a virtual interface IN a created group.
const CREATE_INTERFACE: &str = "CreateLocalGatewayVirtualInterface";

/// The XML element each group's id arrives in
/// (`LocalGatewayVirtualInterfaceGroup$LocalGatewayVirtualInterfaceGroupId`'s `xmlName`).
const ID_ELEMENT: &str = "localGatewayVirtualInterfaceGroupId";

/// The HTTP status EC2 answers `DryRunOperation` with: the request was refused because a
/// precondition the CALLER set holds, not because anything was wrong with it.
const DRY_RUN_STATUS: u16 = 412;

/// One form field.
fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// One `CreateLocalGatewayVirtualInterfaceGroup` call signed for `region`, as its status and
/// body.
async fn create_in(
    router: &MultiServiceRouter,
    region: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, region, CREATE, params).await
}

/// One `CreateLocalGatewayVirtualInterfaceGroup` call signed for [`REGION`].
async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    create_in(router, REGION, params).await
}

/// One `DescribeLocalGatewayVirtualInterfaceGroups` call, as its status and body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, DESCRIBE, params).await
}

/// The form fields for one `TagSpecification` naming `resource_type` and `tags`.
fn tag_specification(resource_type: &str, tags: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut fields = vec![param("TagSpecification.1.ResourceType", resource_type)];
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

/// The ARN of a local gateway virtual interface group, as
/// `LocalGatewayVirtualInterfaceGroup$LocalGatewayVirtualInterfaceGroupArn` carries it:
/// `arn:aws:ec2:<region>:<account>:local-gateway-virtual-interface-group/<id>`.
fn group_arn(region: &str, group_id: &str) -> String {
    format!(
        "arn:aws:ec2:{region}:{}:local-gateway-virtual-interface-group/{group_id}",
        engine::emulator_account_id()
    )
}

/// The ARN of the Outpost `name`, as `LocalGateway$OutpostArn` carries it.
fn outpost_arn(name: &str) -> String {
    format!(
        "arn:aws:outposts:{REGION}:{}:outpost/{name}",
        engine::emulator_account_id()
    )
}

/// The Outposts the seeded local gateways are part of. Nothing in the EC2 API this emulator
/// serves creates an Outpost, so a gateway is written straight through the store's own seam
/// — the one a gateway placed by an Outpost order would use.
fn gateway_outposts() -> Vec<&'static str> {
    vec![
        "op-00000000000000001",
        "op-00000000000000002",
        "op-00000000000000003",
    ]
}

/// Write the seeded local gateways into the engine's store and report the ids they were
/// given, in row order.
fn seed_gateways(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    gateway_outposts()
        .into_iter()
        .map(|outpost| {
            guard
                .insert_local_gateway(LocalGatewaySpec {
                    outpost_arn: outpost_arn(outpost),
                    state: "available".to_string(),
                    tags: Vec::new(),
                })
                .unwrap_or_else(|| {
                    panic!("case {outpost:?}: the store must take the seeded local gateway")
                })
        })
        .collect()
}

/// How many virtual interface groups the store holds.
fn stored_groups(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .local_gateway_virtual_interface_groups
        .len()
}

/// Assert that `body` carries `<member>value</member>`.
fn assert_member(case: &str, body: &str, member: &str, value: &str) {
    assert!(
        body.contains(&format!("<{member}>{value}</{member}>")),
        "case {case:?}: expected <{member}>{value}</{member}>, got {body}"
    );
}

/// The virtual interface ids one reported group carries: the `item` children of its
/// `localGatewayVirtualInterfaceIdSet`, which is how ec2Query flattens
/// `LocalGatewayVirtualInterfaceGroup$LocalGatewayVirtualInterfaceIds` onto the wire.
fn reported_interface_ids(body: &str) -> Vec<String> {
    let set = element(body, "localGatewayVirtualInterfaceIdSet").unwrap_or_default();
    elements(&set, "item")
}

/// Assert that `body` carries no `<member>` element at all.
fn assert_no_member(case: &str, body: &str, member: &str) {
    assert!(
        !body.contains(&format!("<{member}>")),
        "case {case:?}: expected no <{member}>, got {body}"
    );
}

// ---------------------------------------------------------------------------
// The create computes its answer from the store
// ---------------------------------------------------------------------------

/// One row of the create table: which seeded gateway the group is created on, in which
/// Region the call is signed, which of the two local-ASN members the request names (`None`
/// for a request that names neither), and the tags it asks for.
struct CreateRow {
    name: &'static str,
    gateway: usize,
    region: &'static str,
    /// "The Autonomous System Number(ASN) for the local Border Gateway Protocol (BGP)"
    /// (`LocalBgpAsn`).
    local_bgp_asn: Option<i32>,
    /// "The extended 32-bit ASN for the local BGP configuration" (`LocalBgpAsnExtended`).
    local_bgp_asn_extended: Option<i64>,
    tags: Vec<(&'static str, &'static str)>,
}

/// The creates, spanning what the rule generalises over: three gateways on three Outposts
/// (one of them used twice, so a gateway can hold more than one group), two Regions, the
/// 16-bit ASN member, the extended 32-bit one, a request that names neither, and none, one
/// and two tags.
fn create_rows() -> Vec<CreateRow> {
    vec![
        CreateRow {
            name: "a 16-bit local ASN, no tags",
            gateway: 0,
            region: REGION,
            local_bgp_asn: Some(64512),
            local_bgp_asn_extended: None,
            tags: Vec::new(),
        },
        CreateRow {
            name: "an extended local ASN, one tag",
            gateway: 0,
            region: REGION,
            local_bgp_asn: None,
            local_bgp_asn_extended: Some(4_200_000_000),
            tags: vec![("Name", "rack-a")],
        },
        CreateRow {
            name: "no ASN named, two tags",
            gateway: 1,
            region: REGION,
            local_bgp_asn: None,
            local_bgp_asn_extended: None,
            tags: vec![("Name", "rack-b"), ("Owner", "TeamB")],
        },
        CreateRow {
            name: "another Region, the lowest ASN",
            gateway: 2,
            region: OTHER_REGION,
            local_bgp_asn: Some(1),
            local_bgp_asn_extended: None,
            tags: vec![("Name", "rack-c")],
        },
    ]
}

/// The form fields one row sends, beside the gateway it names.
fn row_params(gateway_id: &str, row: &CreateRow) -> Vec<(String, String)> {
    let mut params = vec![param("LocalGatewayId", gateway_id)];
    if let Some(asn) = row.local_bgp_asn {
        params.push(param("LocalBgpAsn", &asn.to_string()));
    }
    if let Some(asn) = row.local_bgp_asn_extended {
        params.push(param("LocalBgpAsnExtended", &asn.to_string()));
    }
    if !row.tags.is_empty() {
        params.extend(tag_specification(
            "local-gateway-virtual-interface-group",
            &row.tags,
        ));
    }
    params
}

/// Each created group is answered as the store holds it — the minted id and ARN, the gateway
/// the request named, the emulator's account, the local BGP configuration asked for (and no
/// ASN member for a request that named none), the transient configuration state and every
/// tag — and reads back through `DescribeLocalGatewayVirtualInterfaceGroups` with the same
/// members.
#[tokio::test]
async fn each_created_group_is_stored_and_reads_back() {
    let (router, engine) = super::ec2_query::emulator();
    let gateways = seed_gateways(&engine);
    let mut minted: Vec<String> = Vec::new();

    for row in create_rows() {
        let case = row.name;
        let gateway_id = &gateways[row.gateway];

        let (status, body) = create_in(&router, row.region, &row_params(gateway_id, &row)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {CREATE}, got {status} with body {body}"
        );

        let group_id = element(&body, ID_ELEMENT)
            .unwrap_or_else(|| panic!("case {case:?}: no <{ID_ELEMENT}> in {body}"));
        assert!(
            group_id.starts_with(&format!("{LOCAL_GATEWAY_VIF_GROUP_ID_PREFIX}-")),
            "case {case:?}: expected an id minted with the group prefix, got {group_id:?}"
        );
        assert!(
            !minted.contains(&group_id),
            "case {case:?}: expected a fresh group id, got the already-minted {group_id:?}"
        );

        let mut expected: Vec<(&str, String)> = vec![
            (
                "localGatewayVirtualInterfaceGroupArn",
                group_arn(row.region, &group_id),
            ),
            ("localGatewayId", gateway_id.clone()),
            ("ownerId", engine::emulator_account_id().to_string()),
            (
                "configurationState",
                LOCAL_GATEWAY_VIF_GROUP_STATE_PENDING.to_string(),
            ),
        ];
        if let Some(asn) = row.local_bgp_asn {
            expected.push(("localBgpAsn", asn.to_string()));
        }
        if let Some(asn) = row.local_bgp_asn_extended {
            expected.push(("localBgpAsnExtended", asn.to_string()));
        }
        for (member, value) in &expected {
            assert_member(case, &body, member, value);
        }
        if row.local_bgp_asn.is_none() {
            assert_no_member(case, &body, "localBgpAsn");
        }
        if row.local_bgp_asn_extended.is_none() {
            assert_no_member(case, &body, "localBgpAsnExtended");
        }
        for (key, value) in &row.tags {
            assert_member(case, &body, "key", key);
            assert_member(case, &body, "value", value);
        }
        // A new group holds no virtual interface: one is put in it by a later
        // `CreateLocalGatewayVirtualInterface` naming it.
        assert!(
            reported_interface_ids(&body).is_empty(),
            "case {case:?}: expected a new group to hold no virtual interface, got {:?} from \
             {body}",
            reported_interface_ids(&body)
        );

        // The round trip: the group reads back through the describe with the same members, so
        // the create answered the record it stored rather than its own request.
        let (read_status, read_body) = describe(
            &router,
            &indexed(
                "LocalGatewayVirtualInterfaceGroupId",
                &[group_id.as_str()],
            ),
        )
        .await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: expected HTTP 200 from {DESCRIBE}, got {read_status} with body \
             {read_body}"
        );
        assert_eq!(
            elements(&read_body, ID_ELEMENT),
            vec![group_id.clone()],
            "case {case:?}: expected {DESCRIBE} to report exactly the created group, got \
             {read_body}"
        );
        for (member, value) in &expected {
            // The configuration state is the one member the clock may already have moved on,
            // so the round trip asserts every OTHER member of the stored record.
            if *member != "configurationState" {
                assert_member(case, &read_body, member, value);
            }
        }
        for (key, value) in &row.tags {
            assert_member(case, &read_body, "key", key);
            assert_member(case, &read_body, "value", value);
        }

        minted.push(group_id);
    }

    assert_eq!(
        stored_groups(&engine),
        create_rows().len(),
        "every created group must be stored once, got {}",
        stored_groups(&engine)
    );
}

/// A new group is answered in the non-terminal `pending` configuration state, and the clock
/// owes it `available` whether or not anybody calls again: the state is looked at again in a
/// loop until it has moved or a deadline far past [`TRANSITION_DELAY`] passes.
#[tokio::test]
async fn a_group_starts_pending_and_the_clock_owes_it_available() {
    let (router, engine) = super::ec2_query::emulator();
    let gateways = seed_gateways(&engine);

    let (status, body) = create(&router, &[param("LocalGatewayId", &gateways[0])]).await;
    assert_eq!(
        status, 200,
        "the clock: expected HTTP 200 from {CREATE}, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "configurationState").as_deref(),
        Some(LOCAL_GATEWAY_VIF_GROUP_STATE_PENDING),
        "the clock: a new group starts in the transient configuration state, got {body}"
    );
    let group_id =
        element(&body, ID_ELEMENT).unwrap_or_else(|| panic!("the clock: no <{ID_ELEMENT}> in {body}"));

    // Generous room: the move is owed one TRANSITION_DELAY after the create, and this waits
    // for the CONDITION rather than for a span of the clock.
    let deadline = Instant::now() + TRANSITION_DELAY * 40;
    let reported = loop {
        let (read_status, read_body) = describe(&router, &[]).await;
        assert_eq!(
            read_status, 200,
            "the clock: expected HTTP 200 from {DESCRIBE}, got {read_status} with body \
             {read_body}"
        );
        assert_eq!(
            elements(&read_body, ID_ELEMENT),
            vec![group_id.clone()],
            "the clock: expected the created group to be reported, got {read_body}"
        );
        let state = element(&read_body, "configurationState").unwrap_or_default();
        if state == LOCAL_GATEWAY_VIF_GROUP_STATE_AVAILABLE || Instant::now() >= deadline {
            break state;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert_eq!(
        reported, LOCAL_GATEWAY_VIF_GROUP_STATE_AVAILABLE,
        "the clock: the move owed one TRANSITION_DELAY after the create must have been \
         applied, got {reported:?}"
    );
}

/// Two creates naming the same gateway are two groups: each gets its own id and both are
/// reported, so the store is what the answer is computed from rather than one fixed record.
#[tokio::test]
async fn two_creates_on_one_gateway_make_two_groups() {
    let (router, engine) = super::ec2_query::emulator();
    let gateways = seed_gateways(&engine);
    let params = [param("LocalGatewayId", &gateways[0])];

    let mut ids = Vec::new();
    for attempt in 1..=2 {
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "create {attempt}: expected HTTP 200, got {status} with body {body}"
        );
        ids.push(
            element(&body, ID_ELEMENT)
                .unwrap_or_else(|| panic!("create {attempt}: no <{ID_ELEMENT}> in {body}")),
        );
    }
    assert_ne!(
        ids[0], ids[1],
        "two creates must mint two ids, got {ids:?} twice"
    );

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "both groups: expected HTTP 200 from {DESCRIBE}, got {status} with body {body}"
    );
    let mut reported = elements(&body, ID_ELEMENT);
    reported.sort();
    let mut expected = ids.clone();
    expected.sort();
    assert_eq!(
        reported, expected,
        "both groups: expected {expected:?} to be reported, got {reported:?}"
    );
    assert_eq!(
        stored_groups(&engine),
        2,
        "both groups: expected two stored groups, got {}",
        stored_groups(&engine)
    );
}

/// A created group is a group a virtual interface can really be created IN: the interface
/// create resolves the minted id, reads the local gateway and the local BGP ASN off the
/// STORED group, and the group then reports that interface — so the create backed its answer
/// with state rather than describing one.
#[tokio::test]
async fn a_created_group_takes_a_virtual_interface_and_reports_it() {
    let (router, engine) = super::ec2_query::emulator();
    let gateways = seed_gateways(&engine);
    let local_bgp_asn = 64515;

    let (status, body) = create(
        &router,
        &[
            param("LocalGatewayId", &gateways[0]),
            param("LocalBgpAsn", &local_bgp_asn.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the group: expected HTTP 200 from {CREATE}, got {status} with body {body}"
    );
    let group_id =
        element(&body, ID_ELEMENT).unwrap_or_else(|| panic!("the group: no <{ID_ELEMENT}> in {body}"));

    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        CREATE_INTERFACE,
        &[
            param("LocalGatewayVirtualInterfaceGroupId", &group_id),
            param("OutpostLagId", "ola-00000000000000001"),
            param("Vlan", "77"),
            param("LocalAddress", "10.0.1.1"),
            param("PeerAddress", "10.0.1.2"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the interface: expected HTTP 200 from {CREATE_INTERFACE}, got {status} with body {body}"
    );
    let interface_id = element(&body, "localGatewayVirtualInterfaceId")
        .unwrap_or_else(|| panic!("the interface: no <localGatewayVirtualInterfaceId> in {body}"));
    assert!(
        interface_id.starts_with(&format!("{LOCAL_GATEWAY_VIF_ID_PREFIX}-")),
        "the interface: expected an id minted with the interface prefix, got {interface_id:?}"
    );
    // The gateway and the local ASN are facts of the GROUP, so the interface reports the
    // ones the group was created with.
    assert_member("the interface", &body, "localGatewayId", &gateways[0]);
    assert_member(
        "the interface",
        &body,
        "localBgpAsn",
        &local_bgp_asn.to_string(),
    );

    let (status, body) = describe(
        &router,
        &indexed("LocalGatewayVirtualInterfaceGroupId", &[group_id.as_str()]),
    )
    .await;
    assert_eq!(
        status, 200,
        "the group again: expected HTTP 200 from {DESCRIBE}, got {status} with body {body}"
    );
    assert_eq!(
        reported_interface_ids(&body),
        vec![interface_id.clone()],
        "the group again: expected the group to report the interface created in it, got {body}"
    );
}

// ---------------------------------------------------------------------------
// A LocalGatewayId that resolves to no local gateway
// ---------------------------------------------------------------------------

/// A `LocalGatewayId` that resolves to no gateway of the account is refused and nothing is
/// written, and the CODE says HOW the value failed: a well-formed local gateway id the
/// account holds no gateway for is `InvalidLocalGatewayID.NotFound`, and a value that is no
/// local gateway id at all is `InvalidLocalGatewayID.Malformed`. The rows are each way a
/// value can fail to resolve: both published EC2 id suffix lengths with digits nothing was
/// created for, a suffix of neither length, an upper-case suffix, a non-hex suffix, the id of
/// a group on a gateway, another resource's id, and free text.
#[tokio::test]
async fn a_local_gateway_id_that_resolves_to_no_gateway_is_refused() {
    let prefix = ec2::state::LOCAL_GATEWAY_ID_PREFIX;
    let cases = [
        (
            "the long form",
            format!("{prefix}-0123456789abcdef0"),
            INVALID_LOCAL_GATEWAY_ID_NOT_FOUND,
        ),
        (
            "the short form",
            format!("{prefix}-00000000"),
            INVALID_LOCAL_GATEWAY_ID_NOT_FOUND,
        ),
        (
            "neither length",
            format!("{prefix}-0000000000000000"),
            INVALID_LOCAL_GATEWAY_ID_MALFORMED,
        ),
        (
            "an upper-case suffix",
            format!("{prefix}-0ABCDEF01"),
            INVALID_LOCAL_GATEWAY_ID_MALFORMED,
        ),
        (
            "a non-hex suffix",
            format!("{prefix}-zzzzzzzz"),
            INVALID_LOCAL_GATEWAY_ID_MALFORMED,
        ),
        (
            "a virtual interface group's own id",
            format!("{LOCAL_GATEWAY_VIF_GROUP_ID_PREFIX}-0123456789abcdef0"),
            INVALID_LOCAL_GATEWAY_ID_MALFORMED,
        ),
        (
            "another resource's id",
            "subnet-00000000000000000".to_string(),
            INVALID_LOCAL_GATEWAY_ID_MALFORMED,
        ),
        (
            "free text",
            "not-a-gateway-at-all".to_string(),
            INVALID_LOCAL_GATEWAY_ID_MALFORMED,
        ),
    ];

    for (case, local_gateway_id, expected_code) in cases {
        let (router, engine) = super::ec2_query::emulator();
        seed_gateways(&engine);

        let (status, body) = create(&router, &[param("LocalGatewayId", &local_gateway_id)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code:?}, got {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains(&format!("'{local_gateway_id}'")),
            "case {case:?}: expected the message to name {local_gateway_id:?}, got {message:?}"
        );
        assert_eq!(
            stored_groups(&engine),
            0,
            "case {case:?}: a refused create must write nothing, got {} groups",
            stored_groups(&engine)
        );
    }
}

/// A gateway the account DOES hold is what separates the refusals above from a success: the
/// same request that was refused for an unknown id succeeds once a gateway with that id is in
/// the store, so the answer is the store's rather than the request's.
#[tokio::test]
async fn the_same_id_is_refused_before_the_gateway_is_stored_and_served_after() {
    let (router, engine) = super::ec2_query::emulator();
    let unknown = format!("{}-0123456789abcdef0", ec2::state::LOCAL_GATEWAY_ID_PREFIX);

    let (status, body) = create(&router, &[param("LocalGatewayId", &unknown)]).await;
    assert_eq!(
        status, 400,
        "before: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        INVALID_LOCAL_GATEWAY_ID_NOT_FOUND,
        "before: expected the gateway's own NotFound, got {body}"
    );

    let stored = seed_gateways(&engine);
    let (status, body) = create(&router, &[param("LocalGatewayId", &stored[0])]).await;
    assert_eq!(
        status, 200,
        "after: expected HTTP 200, got {status} with body {body}"
    );
    assert_member("after", &body, "localGatewayId", &stored[0]);
}

/// A request that carries no `LocalGatewayId` names no gateway to create a group on, so it is
/// refused rather than answered with a group on nothing: a request that leaves the member out
/// is refused by the boundary validator before the handler runs, and one that sends it empty
/// is EC2's `MissingParameter`.
#[tokio::test]
async fn a_request_that_names_no_gateway_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed_gateways(&engine);

    for (case, params, expected_code) in [
        ("no LocalGatewayId at all", Vec::new(), None),
        (
            "an empty LocalGatewayId",
            vec![param("LocalGatewayId", "")],
            Some(MISSING_PARAMETER),
        ),
    ] {
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        if let Some(code) = expected_code {
            assert_eq!(
                error_code(&body),
                code,
                "case {case:?}: expected {code:?}, got {body}"
            );
        }
        assert_eq!(
            stored_groups(&engine),
            0,
            "case {case:?}: a refused create must write nothing, got {} groups",
            stored_groups(&engine)
        );
    }
}

/// An Autonomous System Number is a positive number, so a value that is not one configures no
/// BGP session at all and is refused — on either ASN member — and nothing is written.
#[tokio::test]
async fn an_asn_that_is_not_a_positive_number_is_refused() {
    for (member, value) in [
        ("LocalBgpAsn", "0"),
        ("LocalBgpAsn", "-1"),
        ("LocalBgpAsn", "-64512"),
        ("LocalBgpAsnExtended", "0"),
        ("LocalBgpAsnExtended", "-4200000000"),
    ] {
        let case = format!("{member}={value}");
        let (router, engine) = super::ec2_query::emulator();
        let gateways = seed_gateways(&engine);

        let (status, body) = create(
            &router,
            &[
                param("LocalGatewayId", &gateways[0]),
                param(member, value),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PARAMETER_VALUE,
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        assert_eq!(
            stored_groups(&engine),
            0,
            "case {case:?}: a refused create must write nothing, got {} groups",
            stored_groups(&engine)
        );
    }
}

/// A `TagSpecification` may only name the resource type this action creates: anything else is
/// refused and nothing is written. Rows: the resource type of the gateway the group hangs
/// off, the type of the virtual interfaces that go IN the group, the type of an association
/// made with it, and a type from another surface entirely.
#[tokio::test]
async fn a_tag_specification_naming_another_resource_type_is_refused() {
    for resource_type in [
        "local-gateway",
        "local-gateway-virtual-interface",
        "local-gateway-route-table-virtual-interface-group-association",
        "subnet",
    ] {
        let (router, engine) = super::ec2_query::emulator();
        let gateways = seed_gateways(&engine);

        let mut params = vec![param("LocalGatewayId", &gateways[0])];
        params.extend(tag_specification(resource_type, &[("Name", "rack")]));
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {resource_type:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PARAMETER_VALUE,
            "case {resource_type:?}: expected InvalidParameterValue, got {body}"
        );
        assert_eq!(
            stored_groups(&engine),
            0,
            "case {resource_type:?}: a refused create must write nothing, got {} groups",
            stored_groups(&engine)
        );
    }
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and writes
/// nothing, whether or not the gateway it names exists; `DryRun=false` is an ordinary
/// create.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_writing() {
    let (router, engine) = super::ec2_query::emulator();
    let gateways = seed_gateways(&engine);
    let unknown = format!("{}-0123456789abcdef0", ec2::state::LOCAL_GATEWAY_ID_PREFIX);

    // Rows: a gateway the store holds, and an id it does not — the flag asks whether the
    // caller is permitted, which is not a question about the gateway.
    for (case, local_gateway_id) in [
        ("a stored gateway", gateways[0].as_str()),
        ("an unknown gateway", unknown.as_str()),
    ] {
        let (status, body) = create(
            &router,
            &[
                param("LocalGatewayId", local_gateway_id),
                param("DryRun", "true"),
            ],
        )
        .await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            DRY_RUN_OPERATION,
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        assert_eq!(
            stored_groups(&engine),
            0,
            "case {case:?}: a dry run must write nothing, got {} groups",
            stored_groups(&engine)
        );
    }

    // `DryRun=false` is no dry run at all: the group is created.
    let (status, body) = create(
        &router,
        &[
            param("LocalGatewayId", &gateways[0]),
            param("DryRun", "false"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "DryRun=false: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        stored_groups(&engine),
        1,
        "DryRun=false: expected the group to be created, got {} groups",
        stored_groups(&engine)
    );
}

// ---------------------------------------------------------------------------
// Which refusal a request that fails in several ways at once gets
// ---------------------------------------------------------------------------

/// A request can fail in more than one way at once, and EC2 answers exactly one of them: the
/// request's own FORM is read before any permission is checked, and permission before
/// anything the request names is RESOLVED against the account's resources. Each row below
/// carries the same three defects — a `LocalGatewayId` that cannot resolve, a
/// `TagSpecification` naming another resource type, and `DryRun` — and drops one defect at a
/// time, so the code that comes back is the first refusal in that order rather than whichever
/// check happens to run last.
#[tokio::test]
async fn a_request_that_fails_several_ways_gets_the_first_refusal_in_order() {
    let unknown = format!("{}-0123456789abcdef0", ec2::state::LOCAL_GATEWAY_ID_PREFIX);
    let malformed = format!("{}-zzzzzzzz", ec2::state::LOCAL_GATEWAY_ID_PREFIX);

    // (case, LocalGatewayId, a TagSpecification naming another resource type, DryRun,
    //  expected status, expected code)
    let cases: [(&str, &str, bool, bool, u16, &str); 5] = [
        (
            "no id, a foreign tag type and a dry run",
            "",
            true,
            true,
            400,
            MISSING_PARAMETER,
        ),
        (
            "a malformed id, a foreign tag type and a dry run",
            malformed.as_str(),
            true,
            true,
            400,
            INVALID_LOCAL_GATEWAY_ID_MALFORMED,
        ),
        (
            "an unresolvable id, a foreign tag type and a dry run",
            unknown.as_str(),
            true,
            true,
            400,
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an unresolvable id and a dry run",
            unknown.as_str(),
            false,
            true,
            DRY_RUN_STATUS,
            DRY_RUN_OPERATION,
        ),
        (
            "an unresolvable id alone",
            unknown.as_str(),
            false,
            false,
            400,
            INVALID_LOCAL_GATEWAY_ID_NOT_FOUND,
        ),
    ];

    for (case, local_gateway_id, foreign_tag_type, dry_run, expected_status, expected_code) in cases
    {
        let (router, engine) = super::ec2_query::emulator();
        seed_gateways(&engine);

        let mut params = vec![param("LocalGatewayId", local_gateway_id)];
        if foreign_tag_type {
            params.extend(tag_specification("local-gateway", &[("Name", "rack")]));
        }
        if dry_run {
            params.push(param("DryRun", "true"));
        }

        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code:?}, got {body}"
        );
        assert_eq!(
            stored_groups(&engine),
            0,
            "case {case:?}: a refused create must write nothing, got {} groups",
            stored_groups(&engine)
        );
    }
}
