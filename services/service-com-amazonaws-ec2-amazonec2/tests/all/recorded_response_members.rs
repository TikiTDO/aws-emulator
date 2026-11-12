// SPDX-License-Identifier: CC0-1.0

//! The response elements the live EC2 wire renders that this checkout's Smithy model does
//! not declare, plus the two state facts the same reads depend on — driven over the wire
//! the service actually speaks (`ec2Query`, through the crate's shared [`super::ec2_query`]
//! harness).
//!
//! The rules under test:
//!
//! 1. A REPORTED SHAPE CARRIES THE ELEMENTS THE LIVE WIRE RENDERS ON IT, whichever
//!    operation reports the shape and however many of the shape one response carries —
//!    `dhcpOptionsSource` on a VPC, naming the scope that supplied its DHCP option set, and
//!    `verifiedAccessInstanceTransitGatewayAttachmentSet` on a Verified Access instance,
//!    present and empty for an instance holding no transit-gateway attachment. Neither is
//!    in the model's member list for its shape (`com.amazonaws.ec2#Vpc`,
//!    `com.amazonaws.ec2#VerifiedAccessInstance`), so the recording is the authority for
//!    both; each is still DERIVED from the record reported, which rule 2 pins by taking the
//!    derivation away.
//! 2. THE SCOPE IS READ OFF THE RECORD: a VPC the store holds no DHCP option set
//!    association for reports no `dhcpOptionsSource`, there being no scope to name.
//! 3. A VPC IS `available` ONCE THE CREATE THAT MADE IT HAS RETURNED: the create answers
//!    the transient `pending` the VPC was minted in, and every call after it reports
//!    `available` — a VPC is provisioned inside the call that creates it.
//! 4. A POOL REPORTS THE DESCRIPTION IT HOLDS: "A description of the address pool"
//!    (`PublicIpv4Pool$Description`) is reported for every pool, carrying the text the pool
//!    was created with and the EMPTY text for a pool created without one.
//! 5. THE READ AND THE MUTATION ANSWER THEIR OWN NOT-FOUND CODES for the same absent pool:
//!    `DescribePublicIpv4Pools` refuses `InvalidPublicIpv4PoolID.NotFound` where
//!    `DeletePublicIpv4Pool` refuses `InvalidPublicIpv4Pool.NotFound`, both under the one
//!    message naming the id the request sent.
//!
//! Each table row is a distinct input along what the rules generalise over — which
//! operation reports the shape, how many of the shape the response carries, whether the
//! record carries the value the element is derived from, what the create asked for — so no
//! single recorded call is the specification.

use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, PublicIpv4PoolSpec};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, emulator, error_code, error_message, indexed};
use super::wire::member;

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The scope `dhcpOptionsSource` names for a DHCP option set associated with the VPC
/// itself, which is where every set this store associates is associated.
const DHCP_OPTIONS_SOURCE_VPC: &str = "vpc";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// One call that must succeed, answering its body.
async fn ok(router: &MultiServiceRouter, case: &str, action: &str, params: &Params) -> String {
    let (status, body) = call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// RULE 1, on the VPC side: every VPC a response reports carries the scope its DHCP option
/// set comes from, whichever operation reports it and however many VPCs the response
/// carries.
#[tokio::test]
async fn every_reported_vpc_carries_the_scope_of_its_dhcp_option_set() {
    let (router, _engine) = emulator();

    // Rows: the operation reporting the VPC, and how many VPCs its response carries. They
    // span the three positions the shape is reported in — a member of a create's result, a
    // one-item list, and a list of several — and both kinds of VPC the store holds: the
    // account's ambient default VPC and the ones these creates mint.
    let created = ok(
        &router,
        "CreateVpc",
        "CreateVpc",
        &vec![p("CidrBlock", "10.0.0.0/16")],
    )
    .await;
    let created_id = element(&created, "vpcId").expect("the create reports the VPC it minted");
    let dual = ok(
        &router,
        "CreateVpc with an IPv6 block",
        "CreateVpc",
        &vec![
            p("CidrBlock", "10.1.0.0/16"),
            p("AmazonProvidedIpv6CidrBlock", "true"),
        ],
    )
    .await;

    let one = ok(
        &router,
        "DescribeVpcs naming one VPC",
        "DescribeVpcs",
        &indexed("VpcId", &[&created_id]),
    )
    .await;
    // Every VPC of the Region: the two creates above beside the account's default VPC.
    let all = ok(
        &router,
        "DescribeVpcs naming none",
        "DescribeVpcs",
        &Vec::new(),
    )
    .await;

    let rows: [(&str, &str, usize); 4] = [
        ("CreateVpc", &created, 1),
        ("CreateVpc with an IPv6 block", &dual, 1),
        ("DescribeVpcs naming one VPC", &one, 1),
        ("DescribeVpcs naming none", &all, 3),
    ];
    for (case, body, expected_vpcs) in rows {
        let ids = elements(body, "vpcId");
        assert_eq!(
            ids.len(),
            expected_vpcs,
            "case {case:?}: expected {expected_vpcs} reported VPC(s), got {ids:?} in {body}"
        );
        let sources = elements(body, "dhcpOptionsSource");
        assert_eq!(
            sources,
            vec![DHCP_OPTIONS_SOURCE_VPC.to_string(); expected_vpcs],
            "case {case:?}: expected one dhcpOptionsSource {DHCP_OPTIONS_SOURCE_VPC:?} per \
             reported VPC, got {sources:?} in {body}"
        );
        // The scope names the association the VPC reports, so there is exactly one of each.
        let ids = elements(body, "dhcpOptionsId");
        assert_eq!(
            ids.len(),
            sources.len(),
            "case {case:?}: expected a scope for each reported option set, got {ids:?} \
             against {sources:?} in {body}"
        );
    }
}

/// RULE 2: the scope is read off the record, not written onto every VPC — a VPC the store
/// holds no option set association for reports no scope at all.
#[tokio::test]
async fn a_vpc_with_no_option_set_association_reports_no_scope() {
    let (router, engine) = emulator();
    let created = ok(
        &router,
        "a VPC whose association is dropped",
        "CreateVpc",
        &vec![p("CidrBlock", "10.0.0.0/16")],
    )
    .await;
    let vpc_id = element(&created, "vpcId").expect("the create reports the VPC it minted");

    // Take the association away in the store, which is the only thing the element is
    // derived from.
    {
        let state = engine.state::<Ec2State>();
        let mut store = state.lock();
        let vpc = store
            .vpcs
            .get_mut(&vpc_id)
            .expect("the store holds the VPC the create minted");
        vpc.dhcp_options_id = None;
    }

    let described = ok(
        &router,
        "a VPC whose association is dropped",
        "DescribeVpcs",
        &indexed("VpcId", &[&vpc_id]),
    )
    .await;
    assert_eq!(
        element(&described, "dhcpOptionsId"),
        None,
        "case \"a VPC whose association is dropped\": the association is gone, got {described}"
    );
    assert_eq!(
        element(&described, "dhcpOptionsSource"),
        None,
        "case \"a VPC whose association is dropped\": expected no scope for a VPC that has no \
         option set, got {described}"
    );
}

/// RULE 1, on the Verified Access side: every instance a response reports carries its
/// transit-gateway attachment set, present and empty for an instance holding none —
/// whichever operation reports the instance.
#[tokio::test]
async fn every_reported_verified_access_instance_carries_its_attachment_set() {
    const ATTACHMENT_SET: &str = "verifiedAccessInstanceTransitGatewayAttachmentSet";

    let (router, _engine) = emulator();
    let created = ok(
        &router,
        "CreateVerifiedAccessInstance",
        "CreateVerifiedAccessInstance",
        &vec![p("Description", "the sales applications")],
    )
    .await;
    let instance_id =
        element(&created, "verifiedAccessInstanceId").expect("the create reports the instance");
    let second = ok(
        &router,
        "a second instance",
        "CreateVerifiedAccessInstance",
        &Vec::new(),
    )
    .await;
    assert_eq!(
        elements(&second, ATTACHMENT_SET).len(),
        1,
        "case \"a second instance\": expected the attachment set on it too, got {second}"
    );

    let modified = ok(
        &router,
        "ModifyVerifiedAccessInstance",
        "ModifyVerifiedAccessInstance",
        &vec![
            p("VerifiedAccessInstanceId", instance_id.clone()),
            p("Description", "the support applications"),
        ],
    )
    .await;
    let one = ok(
        &router,
        "DescribeVerifiedAccessInstances naming one",
        "DescribeVerifiedAccessInstances",
        &indexed("VerifiedAccessInstanceId", &[&instance_id]),
    )
    .await;
    let both = ok(
        &router,
        "DescribeVerifiedAccessInstances naming none",
        "DescribeVerifiedAccessInstances",
        &Vec::new(),
    )
    .await;

    // Rows: the operation reporting the instance, and how many instances its response
    // carries — a member of a create's result, of a modify's result, a one-item list and a
    // list of two.
    let rows: [(&str, &str, usize); 4] = [
        ("CreateVerifiedAccessInstance", &created, 1),
        ("ModifyVerifiedAccessInstance", &modified, 1),
        ("DescribeVerifiedAccessInstances naming one", &one, 1),
        ("DescribeVerifiedAccessInstances naming none", &both, 2),
    ];
    for (case, body, expected_instances) in rows {
        let ids = elements(body, "verifiedAccessInstanceId");
        assert_eq!(
            ids.len(),
            expected_instances,
            "case {case:?}: expected {expected_instances} reported instance(s), got {ids:?} in \
             {body}"
        );
        let sets = elements(body, ATTACHMENT_SET);
        assert_eq!(
            sets,
            vec![String::new(); expected_instances],
            "case {case:?}: expected one empty {ATTACHMENT_SET} per reported instance, got \
             {sets:?} in {body}"
        );
    }
}

/// RULE 3: the create answers the transient state the VPC was minted in, and every call
/// after it reports `available`.
#[tokio::test]
async fn a_vpc_is_available_once_the_create_that_made_it_has_returned() {
    // Rows: the two creates that mint a VPC, and the shapes a `CreateVpc` can take — an
    // IPv4 block alone, one with an IPv6 block beside it, and one carrying a tag.
    type Row = (&'static str, &'static str, Params);
    let rows: Vec<Row> = vec![
        (
            "an IPv4 block alone",
            "CreateVpc",
            vec![p("CidrBlock", "10.0.0.0/16")],
        ),
        (
            "an IPv6 block beside it",
            "CreateVpc",
            vec![
                p("CidrBlock", "10.1.0.0/16"),
                p("AmazonProvidedIpv6CidrBlock", "true"),
            ],
        ),
        (
            "a tag on the VPC",
            "CreateVpc",
            vec![
                p("CidrBlock", "10.2.0.0/16"),
                p("TagSpecification.1.ResourceType", "vpc"),
                p("TagSpecification.1.Tag.1.Key", "Name"),
                p("TagSpecification.1.Tag.1.Value", "web"),
            ],
        ),
    ];

    for (case, action, params) in rows {
        // One emulator per row: each row is an independent input, not a sequence.
        let (router, _engine) = emulator();
        let created = ok(&router, case, action, &params).await;
        // The VPC's OWN state, read through the tree: an association inside it carries a
        // `<state>` of its own.
        assert_eq!(
            member(case, &created, "state"),
            "pending",
            "case {case:?}: the create answers the state the VPC was minted in, got {created}"
        );
        let vpc_id = element(&created, "vpcId").expect("the create reports the VPC it minted");

        // Every call after the create: the state is a property of the clock, so the second
        // read reports exactly what the first did.
        for nth in 0..2 {
            let described = ok(&router, case, "DescribeVpcs", &indexed("VpcId", &[&vpc_id])).await;
            assert_eq!(
                member(case, &described, "state"),
                "available",
                "case {case:?}: read {nth}: a VPC is available once its create has returned, \
                 got {described}"
            );
        }
    }
}

/// RULE 4: a pool reports the description it holds — the text it was created with, and the
/// empty text for a pool created without one.
#[tokio::test]
async fn every_reported_pool_carries_the_description_it_holds() {
    // Rows differing along what the description can be: absent from the create (which is
    // every `CreatePublicIpv4Pool`, the request carrying no description member), empty text,
    // one word, and text with a space and a character outside ASCII.
    let seeded: Vec<(&str, Option<&str>, &str)> = vec![
        ("no description at all", None, ""),
        ("an empty description", Some(""), ""),
        ("one word", Some("byoip"), "byoip"),
        (
            "text with a space and one outside ASCII",
            Some("the Équipe range"),
            "the Équipe range",
        ),
    ];

    for (case, description, expected) in seeded {
        let (router, engine) = emulator();
        let pool_id = {
            let state = engine.state::<Ec2State>();
            let mut store = state.lock();
            store
                .insert_public_ipv4_pool(PublicIpv4PoolSpec {
                    description: description.map(str::to_string),
                    network_border_group: REGION.to_string(),
                    tags: Vec::new(),
                    cidrs: vec!["81.3.69.0/24".to_string()],
                })
                .expect("the store has room for the seeded pool")
        };
        let described = ok(
            &router,
            case,
            "DescribePublicIpv4Pools",
            &indexed("PoolId", &[&pool_id]),
        )
        .await;
        assert_eq!(
            element(&described, "description").as_deref(),
            Some(expected),
            "case {case:?}: expected the description {expected:?} on the reported pool, got \
             {described}"
        );
    }

    // And the pool the create under test mints: the request carries no description member,
    // so the pool holds the empty one and the describe still reports the element.
    let (router, _engine) = emulator();
    let created = ok(
        &router,
        "CreatePublicIpv4Pool",
        "CreatePublicIpv4Pool",
        &Vec::new(),
    )
    .await;
    let pool_id = element(&created, "poolId").expect("the create reports the pool it minted");
    let described = ok(
        &router,
        "CreatePublicIpv4Pool",
        "DescribePublicIpv4Pools",
        &indexed("PoolId", &[&pool_id]),
    )
    .await;
    assert_eq!(
        element(&described, "description").as_deref(),
        Some(""),
        "case \"CreatePublicIpv4Pool\": expected an empty description on a pool created \
         without one, got {described}"
    );
}

/// RULE 5: the READ of the pools and a request that ACTS on one answer DIFFERENT not-found
/// codes for the same absent pool, under the one message both carry.
#[tokio::test]
async fn the_read_and_the_delete_answer_their_own_not_found_codes() {
    /// The code `DescribePublicIpv4Pools` refuses an absent pool with.
    const READ_CODE: &str = "InvalidPublicIpv4PoolID.NotFound";
    /// The code `DeletePublicIpv4Pool` refuses the same absent pool with.
    const MUTATION_CODE: &str = "InvalidPublicIpv4Pool.NotFound";

    // Rows: one id of each shape a caller could send — the 17-hex-digit form EC2 mints, a
    // short legacy-length one, an id of another pool family, and a value that is no pool id
    // at all — so the codes are about the pool not being the account's rather than about the
    // id's shape. The last row is a pool the account DID hold and deleted, so "absent"
    // covers both the never-held and the just-removed id.
    let unheld: [(&str, &str); 4] = [
        (
            "a well-formed id nothing holds",
            "ipv4pool-ec2-00000000000000000",
        ),
        ("a short id", "ipv4pool-ec2-1a2b3c4d"),
        ("an IPv6 pool id", "ipv6pool-ec2-0abcdef1234567890"),
        ("a value that is no pool id", "not-a-pool"),
    ];

    let (router, _engine) = emulator();
    // A pool the account DOES hold, deleted below, so the read's code is also what a pool
    // that existed and went answers.
    let held = element(
        &ok(
            &router,
            "a deleted pool",
            "CreatePublicIpv4Pool",
            &Vec::new(),
        )
        .await,
        "poolId",
    )
    .expect("the create reports the pool it minted");
    ok(
        &router,
        "a deleted pool",
        "DeletePublicIpv4Pool",
        &vec![p("PoolId", held.clone())],
    )
    .await;

    let mut rows: Vec<(&str, String)> = unheld
        .iter()
        .map(|(case, pool_id)| (*case, (*pool_id).to_string()))
        .collect();
    rows.push(("a deleted pool", held));

    for (case, pool_id) in rows {
        let expected_message = format!("The pool ID '{pool_id}' does not exist.");
        for (action, expected_code) in [
            ("DescribePublicIpv4Pools", READ_CODE),
            ("DeletePublicIpv4Pool", MUTATION_CODE),
        ] {
            let params = match action {
                "DescribePublicIpv4Pools" => indexed("PoolId", &[&pool_id]),
                _ => vec![p("PoolId", pool_id.clone())],
            };
            let (status, body) = call(&router, REGION, action, &params).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 from {action}, got {status} with body {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, expected_code,
                "case {case:?}: expected {action} to refuse {expected_code}, got {code:?} in \
                 {body}"
            );
            let message = error_message(&body);
            assert_eq!(
                message, expected_message,
                "case {case:?}: expected {action} to answer {expected_message:?}, got \
                 {message:?}"
            );
        }
    }
}
