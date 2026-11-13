// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ModifyVpcTenancy`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=ModifyVpcTenancy`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: the call WRITES the requested instance tenancy onto the VPC the
//! request names, so every later reader of that VPC reports it, and answers `return` true;
//! nothing else about that VPC, and no other VPC, is touched. A request naming a VPC the
//! account does not hold, leaving a parameter out, or probing with `DryRun` is refused with
//! the code and status EC2 answers for it, and writes nothing at all.
//!
//! Every table row is a distinct input along what that generalises over — the tenancy the
//! VPC already carries, the size of its block, whether it carries tags, whether the test
//! created it at all, and how many VPCs the account holds beside it — so the one case a
//! recording happens to contain is one row among many rather than the specification. The
//! tenancy asked for is read from the catalog the `VpcTenancy` enum publishes
//! (`smithy_query(query="shape",shape="VpcTenancy")` -> `["default"]`, as
//! [`ec2::ops::types::VPC_TENANCY_VALUES`] holds it) rather than written out here, so the
//! table follows the model.
//!
//! What `DescribeVpcs` selects and pages over is its own suite's subject
//! ([`super::describe_vpcs`]); this suite reads it only to prove the write.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{error_code, error_message, filters};
use super::wire::{member, param, vpc_tags, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The members of a reported VPC this suite compares BEFORE and AFTER a modify: everything
/// the operation must leave exactly as it was.
///
/// The VPC's own `state` is deliberately not among them: a created VPC is answered `pending`
/// and the clock owes it `available`, so it moves on its own and a comparison of it would be
/// a comparison of how fast the host ran rather than of what this operation wrote.
const UNTOUCHED_MEMBERS: [&str; 4] = ["cidrBlock", "dhcpOptionsId", "isDefault", "ownerId"];

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it. The store is
/// reached only through the wire here, so the engine handle is not needed.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// A well-formed VPC id this account holds no VPC for: the long form's seventeen hexadecimal
/// digits, so what a refusal is asserted for is a resource that is not there rather than a
/// value that could not be read.
fn unissued_vpc_id() -> String {
    "vpc-0123456789abcdef1".to_string()
}

/// The `DescribeVpcs` report of one VPC, narrowed to it by id.
async fn described(case: &str, router: &MultiServiceRouter, vpc_id: &str) -> String {
    ok(case, router, "DescribeVpcs", &[param("VpcId.1", vpc_id)]).await
}

/// The instance tenancy `DescribeVpcs` reports for `vpc_id` — the value every later reader of
/// that VPC sees.
async fn described_tenancy(case: &str, router: &MultiServiceRouter, vpc_id: &str) -> String {
    let body = described(case, router, vpc_id).await;
    member(case, &body, "instanceTenancy")
}

/// The [`UNTOUCHED_MEMBERS`] and the tags `DescribeVpcs` reports for `vpc_id`, as one value a
/// before/after comparison can be made of.
async fn untouched(
    case: &str,
    router: &MultiServiceRouter,
    vpc_id: &str,
) -> (Vec<(String, String)>, Vec<(String, String)>) {
    let body = described(case, router, vpc_id).await;
    let members = UNTOUCHED_MEMBERS
        .iter()
        .map(|name| ((*name).to_string(), member(case, &body, name)))
        .collect();
    (members, vpc_tags(case, &body))
}

/// One `CreateVpc` whose VPC the rule is then asserted on: the case, the form fields the
/// create carries, and the tenancy that create stores.
struct Fixture {
    /// What makes this row a different input.
    case: &'static str,
    /// The `CreateVpc` form fields.
    create: Vec<Param>,
    /// The tenancy the VPC carries before the modify.
    created_tenancy: &'static str,
}

/// The VPC shapes the rule is asserted on. The rows differ in the tenancy the VPC already
/// carries (including the one the modify asks for, so the write is also asked of a VPC
/// already in the requested state), in the size of its CIDR block, and in whether it carries
/// tags the modify must leave alone.
fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            case: "a dedicated VPC, the largest block",
            create: vec![
                param("CidrBlock", "10.0.0.0/16"),
                param("InstanceTenancy", "dedicated"),
            ],
            created_tenancy: "dedicated",
        },
        Fixture {
            case: "a dedicated VPC, the smallest block",
            create: vec![
                param("CidrBlock", "10.1.0.0/28"),
                param("InstanceTenancy", "dedicated"),
            ],
            created_tenancy: "dedicated",
        },
        Fixture {
            case: "a dedicated VPC carrying tags",
            create: vec![
                param("CidrBlock", "10.2.0.0/20"),
                param("InstanceTenancy", "dedicated"),
                param("TagSpecification.1.ResourceType", "vpc"),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "alpha"),
                param("TagSpecification.1.Tag.2.Key", "Tier"),
                param("TagSpecification.1.Tag.2.Value", "web"),
            ],
            created_tenancy: "dedicated",
        },
        Fixture {
            case: "a VPC already at the tenancy the request asks for",
            create: vec![
                param("CidrBlock", "10.3.0.0/16"),
                param("InstanceTenancy", "default"),
            ],
            created_tenancy: "default",
        },
        Fixture {
            case: "a VPC created with no tenancy named",
            create: vec![param("CidrBlock", "10.4.0.0/24")],
            created_tenancy: "default",
        },
    ]
}

// ---------------------------------------------------------------------------
// The write
// ---------------------------------------------------------------------------

/// The rule: `ModifyVpcTenancy` writes the requested tenancy onto the VPC the request names
/// and answers `return` true, so every later reader of that VPC reports the tenancy this call
/// accepted; asking again for a tenancy the VPC already has is served the same way (the
/// request asks for a state, and the VPC ends up in it); and no other member of the VPC, nor
/// any tag it carries, changes.
#[tokio::test]
async fn modify_vpc_tenancy_writes_the_requested_tenancy_and_every_reader_reports_it() {
    let router = emulator();
    // The tenancies this operation may write are the catalog the model publishes — "you can
    // change the instance tenancy attribute of a VPC to default only" is why there is exactly
    // one — so the table is driven by it rather than by a value written here.
    assert_eq!(
        ec2::ops::types::VPC_TENANCY_VALUES,
        ["default"],
        "the published tenancy catalog must be the one the VpcTenancy enum declares, got {:?}",
        ec2::ops::types::VPC_TENANCY_VALUES
    );

    for requested in ec2::ops::types::VPC_TENANCY_VALUES {
        for fixture in fixtures() {
            let case = fixture.case;
            let created = ok(case, &router, "CreateVpc", &fixture.create).await;
            let vpc_id = member(case, &created, "vpcId");
            assert_eq!(
                member(case, &created, "instanceTenancy"),
                fixture.created_tenancy,
                "case {case:?}: the create must store the tenancy it was asked for, got {created}"
            );
            let before = untouched(case, &router, &vpc_id).await;

            // Asked TWICE: the second call is the same request about a VPC that is already in
            // the requested state, which is served rather than refused.
            for attempt in 1..=2 {
                let (status, body) = call(
                    &router,
                    "ModifyVpcTenancy",
                    &[param("VpcId", &vpc_id), param("InstanceTenancy", requested)],
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?} attempt {attempt}: expected HTTP 200, got {status} with \
                     body {body}"
                );
                assert_eq!(
                    super::ec2_query::member(&body, "return").as_deref(),
                    Some("true"),
                    "case {case:?} attempt {attempt}: the request succeeded, so it returns \
                     true, got {body}"
                );
                let reported = described_tenancy(case, &router, &vpc_id).await;
                assert_eq!(
                    reported, *requested,
                    "case {case:?} attempt {attempt}: expected the tenancy {requested:?} to be \
                     reported, got {reported:?}"
                );
            }

            let after = untouched(case, &router, &vpc_id).await;
            assert_eq!(
                after, before,
                "case {case:?}: only the tenancy may change, expected {before:?}, got {after:?}"
            );
        }
    }
}

/// The rule, on the VPC the request does NOT name: a modify writes the tenancy of exactly one
/// VPC, so every other VPC the account holds keeps the tenancy it had.
///
/// The rows differ in which of the account's VPCs is modified and in how the others came to
/// exist — two the test created with `dedicated` tenancy, and the default VPC a fresh account
/// already has, which the test created nothing for.
#[tokio::test]
async fn modify_vpc_tenancy_changes_only_the_vpc_the_request_names() {
    let router = emulator();
    let case = "three dedicated VPCs beside the account's default VPC";

    // The default VPC a fresh account already holds, read through the `is-default` filter
    // `DescribeVpcs` documents: a VPC this test created nothing for.
    let defaults = ok(
        case,
        &router,
        "DescribeVpcs",
        &filters(&[("is-default", &["true"])]),
    )
    .await;
    let default_vpc = member(case, &defaults, "vpcId");
    let default_tenancy = member(case, &defaults, "instanceTenancy");

    let mut created = Vec::new();
    for index in 0..3u8 {
        let cidr = format!("10.{}.0.0/16", 10 + index);
        let body = ok(
            case,
            &router,
            "CreateVpc",
            &[
                param("CidrBlock", &cidr),
                param("InstanceTenancy", "dedicated"),
            ],
        )
        .await;
        created.push(member(case, &body, "vpcId"));
    }

    for modified in 0..created.len() {
        let (status, body) = call(
            &router,
            "ModifyVpcTenancy",
            &[
                param("VpcId", &created[modified]),
                param("InstanceTenancy", "default"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: modifying {} must succeed, got {status} with body {body}",
            created[modified]
        );

        // The VPCs modified so far report `default`; the ones not yet named still report the
        // tenancy their create stored.
        for (index, vpc_id) in created.iter().enumerate() {
            let expected = if index <= modified {
                "default"
            } else {
                "dedicated"
            };
            let reported = described_tenancy(case, &router, vpc_id).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: after modifying VPC {modified}, VPC {index} must report \
                 {expected:?}, got {reported:?}"
            );
        }
        // And the VPC no request here names is untouched.
        let reported = described_tenancy(case, &router, &default_vpc).await;
        assert_eq!(
            reported, default_tenancy,
            "case {case:?}: the default VPC must keep the tenancy {default_tenancy:?}, got \
             {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The refusals
// ---------------------------------------------------------------------------

/// One request the operation refuses: the case, the form fields, and the code, status and
/// message EC2 answers it with.
struct Refusal {
    /// What makes this row a different input.
    case: &'static str,
    /// The form fields the row sends.
    params: Vec<Param>,
    /// The `<Code>` the answer must carry.
    code: &'static str,
    /// The HTTP status that code is answered at.
    status: u16,
    /// The `<Message>` the answer must carry.
    message: String,
}

/// The rule: a request this operation cannot serve is answered with the code, status and
/// sentence EC2 words for it — a VPC the account does not hold with that resource's own
/// not-found code, a parameter carrying no value with `MissingParameter`, and a permission
/// probe with `DryRunOperation` — and the VPC it named keeps the tenancy it had.
///
/// The rows differ in WHAT cannot be served: a resource that is not there, a parameter sent
/// empty, a request that would otherwise have succeeded, and the same probe about a resource
/// that is not there. A request leaving a required parameter out altogether is refused before
/// the handler runs, so it is asserted below the table, where only the refusal itself and the
/// absence of a write are this operation's to own.
#[tokio::test]
async fn modify_vpc_tenancy_refuses_what_it_cannot_serve_and_writes_nothing() {
    let router = emulator();
    let setup = "a dedicated VPC to refuse requests about";
    let created = ok(
        setup,
        &router,
        "CreateVpc",
        &[
            param("CidrBlock", "10.30.0.0/16"),
            param("InstanceTenancy", "dedicated"),
        ],
    )
    .await;
    let vpc_id = member(setup, &created, "vpcId");
    let unissued = unissued_vpc_id();

    let refusals = vec![
        Refusal {
            case: "a well-formed VPC id the account holds no VPC for",
            params: vec![
                param("VpcId", &unissued),
                param("InstanceTenancy", "default"),
            ],
            code: ec2::state::INVALID_VPC_ID_NOT_FOUND,
            status: 400,
            message: format!("The vpc ID '{unissued}' does not exist"),
        },
        Refusal {
            case: "a VpcId sent empty",
            params: vec![param("VpcId", ""), param("InstanceTenancy", "default")],
            code: "MissingParameter",
            status: 400,
            message: "The request must contain the parameter vpcId".to_string(),
        },
        // The permission probe: "checks whether you have the required permissions for the
        // action, without actually making the request" (`DryRun`) — so a request that would
        // otherwise have succeeded is answered `DryRunOperation` and the tenancy is NOT
        // written, which the read below the table asserts for every row alike.
        Refusal {
            case: "a DryRun of a request that would otherwise have succeeded",
            params: vec![
                param("VpcId", &vpc_id),
                param("InstanceTenancy", "default"),
                param("DryRun", "true"),
            ],
            code: "DryRunOperation",
            status: 412,
            message: "Request would have succeeded, but DryRun flag is set.".to_string(),
        },
        // The same probe about a VPC that is not there: the question is about the CALLER, so
        // it is answered before the resource is resolved, exactly as this service answers a
        // dry run of every other request naming a resource it holds none of.
        Refusal {
            case: "a DryRun naming a VPC the account does not hold",
            params: vec![
                param("VpcId", &unissued),
                param("InstanceTenancy", "default"),
                param("DryRun", "true"),
            ],
            code: "DryRunOperation",
            status: 412,
            message: "Request would have succeeded, but DryRun flag is set.".to_string(),
        },
    ];

    for refusal in refusals {
        let case = refusal.case;
        let (status, body) = call(&router, "ModifyVpcTenancy", &refusal.params).await;
        assert_eq!(
            status, refusal.status,
            "case {case:?}: expected HTTP {}, got {status} with body {body}",
            refusal.status
        );
        assert_eq!(
            error_code(&body),
            refusal.code,
            "case {case:?}: expected the code {:?}, got {body}",
            refusal.code
        );
        assert_eq!(
            error_message(&body),
            refusal.message,
            "case {case:?}: expected the message {:?}, got {body}",
            refusal.message
        );
        // Nothing was written: the VPC every row either names or leaves alone still carries
        // the tenancy its create stored.
        let reported = described_tenancy(case, &router, &vpc_id).await;
        assert_eq!(
            reported, "dedicated",
            "case {case:?}: a refused request writes nothing, so the tenancy is still \
             \"dedicated\", got {reported:?}"
        );
    }

    // A request that leaves a `@required` member out entirely is refused before the handler
    // runs, so WHICH code it carries is the framework boundary's answer rather than this
    // operation's: what this operation owes is that such a request is refused at a 4xx and
    // that nothing is written.
    for (case, params) in [
        ("no VpcId at all", vec![param("InstanceTenancy", "default")]),
        ("no InstanceTenancy at all", vec![param("VpcId", &vpc_id)]),
        ("neither required parameter", vec![]),
    ] {
        let (status, body) = call(&router, "ModifyVpcTenancy", &params).await;
        assert!(
            (400..500).contains(&status),
            "case {case:?}: expected a 4xx refusal, got {status} with body {body}"
        );
        assert!(
            !error_code(&body).is_empty(),
            "case {case:?}: a refusal carries an error code, got {body}"
        );
        let reported = described_tenancy(case, &router, &vpc_id).await;
        assert_eq!(
            reported, "dedicated",
            "case {case:?}: a refused request writes nothing, so the tenancy is still \
             \"dedicated\", got {reported:?}"
        );
    }
}

/// The rule: the only tenancy this operation writes is one the published catalog names
/// ([`ec2::ops::types::VPC_TENANCY_VALUES`]) — "you cannot change the instance tenancy
/// attribute to dedicated" — so a request naming any other value is refused and the VPC keeps
/// the tenancy it had.
///
/// The rows are the tenancies EC2 words elsewhere that this catalog excludes. WHICH code this
/// service answers for a value outside a modelled catalog it has no recorded wording for is
/// the subject of its own refusal-wording issue, so what is pinned here is that the request is
/// refused at a 4xx and that nothing is written — never a 200 and never the tenancy changing.
#[tokio::test]
async fn modify_vpc_tenancy_refuses_a_tenancy_outside_the_published_catalog() {
    let router = emulator();
    let setup = "a dedicated VPC to ask an unpublished tenancy of";
    let created = ok(
        setup,
        &router,
        "CreateVpc",
        &[
            param("CidrBlock", "10.40.0.0/16"),
            param("InstanceTenancy", "dedicated"),
        ],
    )
    .await;
    let vpc_id = member(setup, &created, "vpcId");

    // The two tenancies the rest of this model publishes for a placement — `Tenancy`'s
    // `[default, dedicated, host]` (`smithy_query(query="shape",shape="Tenancy")`) — minus the
    // one `VpcTenancy` carries, and the empty value, which names no tenancy at all.
    for tenancy in ["dedicated", "host", ""] {
        let case = format!("the tenancy {tenancy:?}, which VpcTenancy does not publish");
        assert!(
            !ec2::ops::types::VPC_TENANCY_VALUES.contains(&tenancy),
            "case {case:?}: the row must name a tenancy outside the published catalog {:?}",
            ec2::ops::types::VPC_TENANCY_VALUES
        );
        let (status, body) = call(
            &router,
            "ModifyVpcTenancy",
            &[param("VpcId", &vpc_id), param("InstanceTenancy", tenancy)],
        )
        .await;
        assert!(
            (400..500).contains(&status),
            "case {case:?}: expected a 4xx refusal, got {status} with body {body}"
        );
        assert!(
            !error_code(&body).is_empty(),
            "case {case:?}: a refusal carries an error code, got {body}"
        );
        let reported = described_tenancy(&case, &router, &vpc_id).await;
        assert_eq!(
            reported, "dedicated",
            "case {case:?}: a refused request writes nothing, so the tenancy is still \
             \"dedicated\", got {reported:?}"
        );
    }
}
