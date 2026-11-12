// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateVpc`, driven over the wire the service actually speaks:
//! `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=CreateVpc`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//! ec2Query is selected by the `ec2` signing name in the request's SigV4 credential scope,
//! so every call here carries one.
//!
//! The rule under test is that the operation CREATES THE RESOURCE THE REQUEST DESCRIBES:
//! the VPC it stores carries the requested IPv4 CIDR block in canonical form, the
//! requested tenancy, the tags of a `vpc` tag specification and the IPv6 association the
//! request asked for; it is answered in the non-terminal `pending` state the clock then
//! owes `available`; and every member of it reads back through `DescribeVpcs`. Every table
//! row is a distinct input along what that generalises over — the CIDR value and its block
//! size, the tenancy, the tag count, the IPv6 request form — so the one case a recording
//! happens to contain is one row among many rather than the specification.
//!
//! What `DescribeVpcs` selects and pages over is its own suite's subject
//! ([`super::describe_vpcs`]); this suite reads it only to prove the round trip.

use std::collections::BTreeSet;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{elements, error_code};
use super::wire::{
    has_ipv6_association, ipv4_association, ipv6_association, is_long_id, member, param, vpc_tags,
    Param,
};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The IPv4 CIDR block of the default VPC a fresh account already has
/// ([`ec2::state::DEFAULT_VPC_CIDR_BLOCK`]), which every describe below reports beside the
/// VPCs the test creates.
const DEFAULT_VPC_CIDR_BLOCK: &str = ec2::state::DEFAULT_VPC_CIDR_BLOCK;

/// One row of a refusal table: the case, the request's form fields, and the EC2 error code
/// the service answers.
type RefusalCase = (&'static str, Vec<Param>, &'static str);

/// One row of a table that only varies the request: the case and its form fields.
type RequestCase = (&'static str, Vec<Param>);

/// One row of the tag table: the case and the tags the request assigns.
type TagCase = (&'static str, Vec<(&'static str, &'static str)>);

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it. The store is
/// reached only through the wire here, so the engine handle is not needed.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One `CreateVpc` call carrying `params`, as its HTTP status and XML body.
async fn create_vpc(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "CreateVpc", params).await
}

/// One `DescribeVpcs` call carrying `params`, as its HTTP status and XML body.
async fn describe_vpcs(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeVpcs", params).await
}

/// `CreateVpc` with just a CIDR block, requiring success, and the body it answered.
async fn create_ok(case: &str, router: &MultiServiceRouter, cidr: &str) -> String {
    let (status, body) = create_vpc(router, &[param("CidrBlock", cidr)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateVpc, got {status} with body {body}"
    );
    body
}

/// `DescribeVpcs` requiring success, and the body it answered.
async fn describe_ok(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    let (status, body) = describe_vpcs(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeVpcs, got {status} with body {body}"
    );
    body
}

// ---------------------------------------------------------------------------
// The CIDR block
// ---------------------------------------------------------------------------

/// The rule: `CreateVpc` stores the requested IPv4 CIDR block in its canonical form (the
/// host bits cleared, the prefix length kept) and every member of the VPC it answers reads
/// back through `DescribeVpcs`.
///
/// The rows differ in the block size and in how many host bits the request set; the
/// model's own example (`100.68.0.18/18` -> `100.68.0.0/18`) is one of them, and both
/// published bounds of the block size are covered.
#[tokio::test]
async fn create_vpc_canonicalises_the_cidr_block_and_round_trips_through_describe_vpcs() {
    let router = emulator();
    // (case, requested CIDR, the canonical form AWS stores)
    let cases = [
        ("the model's own example", "100.68.0.18/18", "100.68.0.0/18"),
        ("already canonical", "10.0.0.0/16", "10.0.0.0/16"),
        ("largest allowed block", "172.16.0.9/16", "172.16.0.0/16"),
        ("host bits mid-address", "172.20.35.200/20", "172.20.32.0/20"),
        ("a /24", "192.168.7.42/24", "192.168.7.0/24"),
        (
            "smallest allowed block",
            "192.168.100.14/28",
            "192.168.100.0/28",
        ),
    ];

    let mut ids = Vec::new();
    let mut dhcp_option_sets = BTreeSet::new();
    for (case, requested, canonical) in cases {
        let body = create_ok(case, &router, requested).await;

        let vpc_id = member(case, &body, "vpcId");
        assert!(
            is_long_id("vpc", &vpc_id),
            "case {case:?}: expected a vpc-<17 hex> id, got {vpc_id:?}"
        );
        assert_eq!(
            member(case, &body, "cidrBlock"),
            canonical,
            "case {case:?}: expected the canonical CIDR {canonical:?}, got {body}"
        );
        assert_eq!(
            member(case, &body, "state"),
            "pending",
            "case {case:?}: a new VPC is answered pending, got {body}"
        );
        assert_eq!(
            member(case, &body, "ownerId"),
            engine::emulator_account_id(),
            "case {case:?}: expected the emulator account as owner, got {body}"
        );
        assert_eq!(
            member(case, &body, "isDefault"),
            "false",
            "case {case:?}: only CreateDefaultVpc creates the default VPC, got {body}"
        );
        assert_eq!(
            member(case, &body, "instanceTenancy"),
            "default",
            "case {case:?}: tenancy defaults to default, got {body}"
        );
        assert_eq!(
            member(case, &body, "internetGatewayBlockMode"),
            "off",
            "case {case:?}: VPC Block Public Access is off until it is turned on, got {body}"
        );
        let dhcp_options_id = member(case, &body, "dhcpOptionsId");
        assert!(
            is_long_id("dopt", &dhcp_options_id),
            "case {case:?}: expected a dopt-<17 hex> option set, got {dhcp_options_id:?}"
        );
        dhcp_option_sets.insert(dhcp_options_id);

        // The primary IPv4 block is associated as soon as the VPC exists.
        assert_eq!(
            ipv4_association(case, &body, "cidrBlock"),
            canonical,
            "case {case:?}: the association carries the canonical CIDR, got {body}"
        );
        assert_eq!(
            ipv4_association(case, &body, "state"),
            "associated",
            "case {case:?}: the primary block is associated at once, got {body}"
        );
        let association_id = ipv4_association(case, &body, "associationId");
        assert!(
            is_long_id("vpc-cidr-assoc", &association_id),
            "case {case:?}: expected a vpc-cidr-assoc-<17 hex> id, got {association_id:?}"
        );
        assert!(
            !has_ipv6_association(&body),
            "case {case:?}: no IPv6 block was requested, got {body}"
        );

        // The round trip: the same VPC, read back through DescribeVpcs.
        let read = describe_ok(case, &router, &[param("VpcId.1", &vpc_id)]).await;
        assert_eq!(
            elements(&read, "vpcId"),
            vec![vpc_id.clone()],
            "case {case:?}: expected exactly the created VPC, got {read}"
        );
        assert_eq!(
            member(case, &read, "cidrBlock"),
            canonical,
            "case {case:?}: the stored CIDR must survive the round trip, got {read}"
        );
        assert_eq!(
            ipv4_association(case, &read, "associationId"),
            association_id,
            "case {case:?}: the association id must survive the round trip, got {read}"
        );
        assert_eq!(
            member(case, &read, "dhcpOptionsId"),
            member(case, &body, "dhcpOptionsId"),
            "case {case:?}: the option set must survive the round trip, got {read}"
        );
        ids.push(vpc_id);
    }

    let distinct: BTreeSet<&String> = ids.iter().collect();
    assert_eq!(
        distinct.len(),
        ids.len(),
        "every VPC gets its own id, got {ids:?}"
    );
    assert_eq!(
        dhcp_option_sets.len(),
        1,
        "every VPC of one Region inherits that Region's one default DHCP option set, got \
         {dhcp_option_sets:?}"
    );

    // Every created VPC is in the store, beside the default VPC the account already had.
    let read = describe_ok("all", &router, &[]).await;
    let reported = elements(&read, "vpcId");
    for id in &ids {
        assert!(
            reported.contains(id),
            "the created VPC {id:?} must be in the store, got {read}"
        );
    }
    assert_eq!(
        reported.len(),
        ids.len() + 1,
        "the store holds the created VPCs and the account's default VPC, got {read}"
    );
    assert!(
        elements(&read, "cidrBlock").contains(&DEFAULT_VPC_CIDR_BLOCK.to_string()),
        "the account's default VPC is untouched by a create, got {read}"
    );
}

/// The rule: a requested IPv4 CIDR block outside the published VPC block size (a /16
/// through a /28), or one that is not an IPv4 CIDR block at all, is refused with
/// `InvalidParameterValue`, and a request with no block at all with `MissingParameter` —
/// and either way nothing is stored.
///
/// The rows cross each published bound by one and by a lot, and cover the ways a value can
/// fail to be an IPv4 CIDR block.
#[tokio::test]
async fn a_cidr_block_outside_the_published_block_size_is_refused_and_stores_nothing() {
    let router = emulator();
    // (case, the CidrBlock value, the EC2 error code). A block whose SIZE is outside the
    // published range is a RANGE violation — `InvalidVpc.Range`, which tells a client to
    // retry with a smaller block — while a value that is not an IPv4 CIDR block at all could
    // not be read, which is `InvalidParameterValue`.
    let cases: [(&str, Option<&str>, &str); 10] = [
        ("one past the /16 bound", Some("10.0.0.0/15"), "InvalidVpc.Range"),
        ("far past the /16 bound", Some("10.0.0.0/8"), "InvalidVpc.Range"),
        ("one past the /28 bound", Some("10.0.0.0/29"), "InvalidVpc.Range"),
        ("a single address", Some("10.0.0.0/32"), "InvalidVpc.Range"),
        ("no prefix length", Some("10.0.0.0"), "InvalidParameterValue"),
        (
            "prefix length off the scale",
            Some("10.0.0.0/33"),
            "InvalidParameterValue",
        ),
        (
            "not an address at all",
            Some("ten.zero.zero.zero/16"),
            "InvalidParameterValue",
        ),
        ("an IPv6 block", Some("2600:1f00::/56"), "InvalidParameterValue"),
        ("an empty value", Some(""), "MissingParameter"),
        ("no CidrBlock member", None, "MissingParameter"),
    ];

    for (case, value, expected) in cases {
        let params: Vec<Param> = value
            .map(|value| vec![param("CidrBlock", value)])
            .unwrap_or_default();
        let (status, body) = create_vpc(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected the code {expected:?}, got {body}"
        );
    }

    let read = describe_ok("after the refusals", &router, &[]).await;
    assert_eq!(
        elements(&read, "cidrBlock"),
        vec![
            DEFAULT_VPC_CIDR_BLOCK.to_string(),
            DEFAULT_VPC_CIDR_BLOCK.to_string()
        ],
        "a refused CreateVpc must store nothing, so only the account's default VPC — with \
         its primary block reported on the VPC and on its association — is there: {read}"
    );

    // The published bounds themselves are accepted — the refusal is of what lies outside.
    for (case, cidr) in [
        ("the /16 bound", "10.0.0.0/16"),
        ("the /28 bound", "10.1.0.0/28"),
    ] {
        let body = create_ok(case, &router, cidr).await;
        assert_eq!(
            member(case, &body, "cidrBlock"),
            cidr,
            "case {case:?}: a block at the published bound is accepted, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Tenancy, tags, dry run
// ---------------------------------------------------------------------------

/// The rule: a VPC's instance tenancy is the requested value when the request names
/// `default` or `dedicated` and `default` when it names none; `host`, which the parameter's
/// own documentation excludes, is refused with `InvalidParameterValue`.
#[tokio::test]
async fn instance_tenancy_is_echoed_defaulted_or_refused() {
    let router = emulator();
    // (case, the requested tenancy, the stored tenancy)
    let accepted = [
        ("no tenancy named", None, "default"),
        ("default named", Some("default"), "default"),
        ("dedicated named", Some("dedicated"), "dedicated"),
    ];
    for (index, (case, requested, expected)) in accepted.into_iter().enumerate() {
        let cidr = format!("10.{index}.0.0/16");
        let mut params = vec![param("CidrBlock", &cidr)];
        if let Some(requested) = requested {
            params.push(param("InstanceTenancy", requested));
        }
        let (status, body) = create_vpc(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            member(case, &body, "instanceTenancy"),
            expected,
            "case {case:?}: expected the tenancy {expected:?}, got {body}"
        );
        // And it survives the read back.
        let vpc_id = member(case, &body, "vpcId");
        let read = describe_ok(case, &router, &[param("VpcId.1", &vpc_id)]).await;
        assert_eq!(
            member(case, &read, "instanceTenancy"),
            expected,
            "case {case:?}: the tenancy must survive the round trip, got {read}"
        );
    }

    let (status, body) = create_vpc(
        &router,
        &[
            param("CidrBlock", "10.9.0.0/16"),
            param("InstanceTenancy", "host"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"host tenancy\": expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "case \"host tenancy\": the host value cannot be used with this parameter, got {body}"
    );
}

/// The rule: the tags of a `vpc` tag specification are assigned to the VPC and are readable
/// back on it; a tag specification naming any other resource type, or none at all, is
/// refused.
///
/// The rows differ in the number of tags and in the values they carry (including an empty
/// value, which is a tag value like any other).
#[tokio::test]
async fn vpc_tag_specifications_are_assigned_and_other_resource_types_are_refused() {
    let router = emulator();
    // (case, the tags to assign)
    let cases: [TagCase; 4] = [
        ("no tags", vec![]),
        ("one tag", vec![("Name", "alpha")]),
        (
            "three tags",
            vec![("Name", "beta"), ("Tier", "web"), ("Owner", "TeamA")],
        ),
        ("an empty tag value", vec![("Name", "")]),
    ];

    for (index, (case, tags)) in cases.into_iter().enumerate() {
        let cidr = format!("10.{index}.0.0/16");
        let mut params = vec![param("CidrBlock", &cidr)];
        if !tags.is_empty() {
            params.push(param("TagSpecification.1.ResourceType", "vpc"));
            for (position, (key, value)) in tags.iter().enumerate() {
                let index = position + 1;
                params.push(param(&format!("TagSpecification.1.Tag.{index}.Key"), key));
                params.push(param(
                    &format!("TagSpecification.1.Tag.{index}.Value"),
                    value,
                ));
            }
        }
        let (status, body) = create_vpc(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let vpc_id = member(case, &body, "vpcId");

        let read = describe_ok(case, &router, &[param("VpcId.1", &vpc_id)]).await;
        let read_tags = vpc_tags(case, &read);
        let expected: Vec<(String, String)> = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            read_tags, expected,
            "case {case:?}: expected the tags {expected:?} on the VPC, got {read}"
        );
    }

    // A resource type other than the VPC being created, and a specification with none.
    let refused: [RefusalCase; 2] = [
        (
            "a subnet tag specification",
            vec![
                param("CidrBlock", "10.50.0.0/16"),
                param("TagSpecification.1.ResourceType", "subnet"),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "nope"),
            ],
            "InvalidParameterValue",
        ),
        (
            "a tag specification with no resource type",
            vec![
                param("CidrBlock", "10.51.0.0/16"),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "nope"),
            ],
            "MissingParameter",
        ),
    ];
    for (case, params, expected) in refused {
        let (status, body) = create_vpc(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected the code {expected:?}, got {body}"
        );
    }
}

/// The rule: the caller-chosen tag payload a `CreateVpc` stores is bounded — at most the
/// documented number of tags per resource, each key and value within its documented
/// length, each key given once and none carrying the reserved prefix — and a request past
/// any of those bounds is refused with nothing stored.
///
/// The bounds come from EC2's own documentation, held on the crate's constants:
/// [`ec2::state::MAX_TAGS_PER_RESOURCE`] ("Each resource can have a maximum of 50 tags"),
/// [`ec2::state::MAX_TAG_KEY_CHARS`] / [`ec2::state::MAX_TAG_VALUE_CHARS`] (a key accepts
/// 127 and a value 256 Unicode characters) and [`ec2::state::RESERVED_TAG_PREFIX`]. Each
/// numeric bound is exercised AT the published value (accepted) and one past it (refused).
#[tokio::test]
async fn the_tag_payload_a_create_stores_is_bounded() {
    let router = emulator();
    let before = elements(&describe_ok("before", &router, &[]).await, "vpcId");

    /// The form fields for `count` tags whose keys are distinct, with `value` on each.
    fn tag_params(count: usize, key_prefix: &str, value: &str) -> Vec<Param> {
        let mut params = vec![param("TagSpecification.1.ResourceType", "vpc")];
        for index in 1..=count {
            params.push(param(
                &format!("TagSpecification.1.Tag.{index}.Key"),
                &format!("{key_prefix}{index}"),
            ));
            params.push(param(
                &format!("TagSpecification.1.Tag.{index}.Value"),
                value,
            ));
        }
        params
    }

    let max_tags = ec2::state::MAX_TAGS_PER_RESOURCE;
    let max_key = ec2::state::MAX_TAG_KEY_CHARS;
    let max_value = ec2::state::MAX_TAG_VALUE_CHARS;

    // AT each published bound: accepted, and the payload reads back on the VPC.
    let accepted: [(&str, &str, Vec<Param>); 3] = [
        (
            "the maximum number of tags",
            "10.80.0.0/16",
            tag_params(max_tags, "key", "v"),
        ),
        (
            "a key of the maximum length",
            "10.81.0.0/16",
            vec![
                param("TagSpecification.1.ResourceType", "vpc"),
                param("TagSpecification.1.Tag.1.Key", &"k".repeat(max_key)),
                param("TagSpecification.1.Tag.1.Value", "v"),
            ],
        ),
        (
            "a value of the maximum length",
            "10.82.0.0/16",
            vec![
                param("TagSpecification.1.ResourceType", "vpc"),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", &"v".repeat(max_value)),
            ],
        ),
    ];
    for (case, cidr, tags) in accepted {
        let mut params = vec![param("CidrBlock", cidr)];
        params.extend(tags);
        let (status, body) = create_vpc(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a payload at the published bound is accepted, \
             got {status} with body {body}"
        );
        let vpc_id = member(case, &body, "vpcId");
        let read = describe_ok(case, &router, &[param("VpcId.1", &vpc_id)]).await;
        let read_tags = vpc_tags(case, &read);
        assert_eq!(
            read_tags,
            vpc_tags(case, &body),
            "case {case:?}: the stored tags must read back unchanged, got {read}"
        );
        assert!(
            !read_tags.is_empty(),
            "case {case:?}: expected the tags to be stored, got {read}"
        );
    }

    // One step past each bound, plus the two malformed keys: refused, nothing stored.
    let refused: [RefusalCase; 5] = [
        (
            "one tag past the maximum",
            {
                let mut params = vec![param("CidrBlock", "10.90.0.0/16")];
                params.extend(tag_params(max_tags + 1, "key", "v"));
                params
            },
            "TagLimitExceeded",
        ),
        (
            "a key one character past the maximum",
            vec![
                param("CidrBlock", "10.91.0.0/16"),
                param("TagSpecification.1.ResourceType", "vpc"),
                param("TagSpecification.1.Tag.1.Key", &"k".repeat(max_key + 1)),
                param("TagSpecification.1.Tag.1.Value", "v"),
            ],
            "InvalidParameterValue",
        ),
        (
            "a value one character past the maximum",
            vec![
                param("CidrBlock", "10.92.0.0/16"),
                param("TagSpecification.1.ResourceType", "vpc"),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param(
                    "TagSpecification.1.Tag.1.Value",
                    &"v".repeat(max_value + 1),
                ),
            ],
            "InvalidParameterValue",
        ),
        (
            "the same key twice",
            vec![
                param("CidrBlock", "10.93.0.0/16"),
                param("TagSpecification.1.ResourceType", "vpc"),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "first"),
                param("TagSpecification.1.Tag.2.Key", "Name"),
                param("TagSpecification.1.Tag.2.Value", "second"),
            ],
            "InvalidParameterValue",
        ),
        (
            "a key carrying the reserved prefix",
            vec![
                param("CidrBlock", "10.94.0.0/16"),
                param("TagSpecification.1.ResourceType", "vpc"),
                param(
                    "TagSpecification.1.Tag.1.Key",
                    &format!("{}reserved", ec2::state::RESERVED_TAG_PREFIX),
                ),
                param("TagSpecification.1.Tag.1.Value", "v"),
            ],
            "InvalidParameterValue",
        ),
    ];
    let accepted_count = elements(&describe_ok("accepted", &router, &[]).await, "vpcId").len();
    for (case, params, expected) in refused {
        let (status, body) = create_vpc(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected the code {expected:?}, got {body}"
        );
    }
    let after = elements(&describe_ok("after the refusals", &router, &[]).await, "vpcId");
    assert_eq!(
        after.len(),
        accepted_count,
        "a refused CreateVpc must store nothing: expected the {accepted_count} VPCs the \
         accepted rows left, got {after:?} (before the suite: {before:?})"
    );
}

/// The rule: the caller-chosen bytes a create retains are charged against the store's one
/// byte budget, so a record count is never the only thing bounding memory. The rows differ
/// in which caller-chosen strings the request carries.
#[tokio::test]
async fn the_store_charges_the_caller_chosen_bytes_a_create_retains() {
    use service_com_amazonaws_ec2_amazonec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;

    let (router, engine) = super::ec2_query::emulator();

    // (case, the members beside CidrBlock, the caller-chosen bytes the record retains)
    let cases: [(&str, &str, Vec<Param>, usize); 3] = [
        ("no caller strings", "10.60.0.0/16", vec![], 0),
        (
            "two tags",
            "10.61.0.0/16",
            vec![
                param("TagSpecification.1.ResourceType", "vpc"),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "alpha"),
                param("TagSpecification.1.Tag.2.Key", "Tier"),
                param("TagSpecification.1.Tag.2.Value", "web"),
            ],
            "Name".len() + "alpha".len() + "Tier".len() + "web".len(),
        ),
        (
            "a tag and an IPv6 association",
            "10.62.0.0/16",
            vec![
                param("TagSpecification.1.ResourceType", "vpc"),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "beta"),
                param("Ipv6CidrBlock", "2001:db8:1234:1a00::/56"),
                param("Ipv6Pool", "ipv6pool-ec2-0123456789abcdef0"),
            ],
            "Name".len()
                + "beta".len()
                + "2001:db8:1234:1a00::/56".len()
                + "ipv6pool-ec2-0123456789abcdef0".len(),
        ),
    ];

    for (case, cidr, extra, expected) in cases {
        let before = engine.state::<Ec2State>().lock().stored_bytes();
        let mut params = vec![param("CidrBlock", cidr)];
        params.extend(extra);
        let (status, body) = create_vpc(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let after = engine.state::<Ec2State>().lock().stored_bytes();
        assert_eq!(
            after - before,
            expected,
            "case {case:?}: expected the store to charge {expected} caller-chosen bytes, \
             it went from {before} to {after}"
        );
    }
}

/// The rule: `DryRun` on an action that would have succeeded is answered with
/// `DryRunOperation` (HTTP 412) and changes nothing.
#[tokio::test]
async fn a_dry_run_answers_dry_run_operation_and_changes_nothing() {
    let router = emulator();
    let before = elements(&describe_ok("before", &router, &[]).await, "vpcId");

    // (case, the members beside DryRun)
    let cases: [RequestCase; 3] = [
        ("a CIDR block", vec![param("CidrBlock", "10.1.0.0/16")]),
        (
            "a CIDR block and tags",
            vec![
                param("CidrBlock", "10.2.0.0/16"),
                param("TagSpecification.1.ResourceType", "vpc"),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "alpha"),
            ],
        ),
        (
            "a CIDR block and an Amazon-provided IPv6 block",
            vec![
                param("CidrBlock", "10.3.0.0/16"),
                param("AmazonProvidedIpv6CidrBlock", "true"),
            ],
        ),
    ];
    for (case, members) in cases {
        let mut params = members;
        params.push(param("DryRun", "true"));
        let (status, body) = create_vpc(&router, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: a dry run answers HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );
    }

    let after = elements(&describe_ok("after the dry runs", &router, &[]).await, "vpcId");
    assert_eq!(
        after, before,
        "a dry run must create no VPC, got {after:?} where {before:?} was there"
    );
}

// ---------------------------------------------------------------------------
// The clock-owed transition
// ---------------------------------------------------------------------------

/// The rule: `CreateVpc` answers the non-terminal state the VPC was minted in, and every
/// call that follows the create reports it `available` — a VPC is provisioned inside the
/// call that creates it, so the move is owed from the instant that call returns
/// (`VPC_PROVISIONING_DELAY`) and no second call is told `pending`. The IPv6 CIDR block
/// association created with it moves `associating` -> `associated` on that same instant.
/// Nothing the caller does drives either move: the clock owes them, and a read only
/// materialises what was already due.
#[tokio::test]
async fn a_vpc_becomes_available_once_the_clock_owes_the_transition() {
    let router = emulator();
    // Rows: a VPC with only its IPv4 block, and one that also carries an IPv6 block, whose
    // association has a state of its own to move.
    let plain = create_ok("IPv4 only", &router, "10.0.0.0/16").await;
    let (status, dual) = create_vpc(
        &router,
        &[
            param("CidrBlock", "10.1.0.0/16"),
            param("AmazonProvidedIpv6CidrBlock", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"with IPv6\": expected HTTP 200, got {status} with body {dual}"
    );

    for (case, body) in [("IPv4 only", &plain), ("with IPv6", &dual)] {
        assert_eq!(
            member(case, body, "state"),
            "pending",
            "case {case:?}: a caller acting straight after CreateVpc still sees pending, \
             got {body}"
        );
    }
    assert_eq!(
        ipv6_association("with IPv6", &dual, "state"),
        "associating",
        "case \"with IPv6\": the IPv6 association comes up too, got {dual}"
    );
    let plain_id = member("IPv4 only", &plain, "vpcId");
    let dual_id = member("with IPv6", &dual, "vpcId");

    // Read again: the state is a property of the clock, not of being read — the move was
    // owed from the instant the create returned, so the first call after it reports the
    // settled state and so does every later one.
    for (case, vpc_id) in [("IPv4 only", &plain_id), ("with IPv6", &dual_id)] {
        for nth in 0..2 {
            let read = describe_ok(case, &router, &[param("VpcId.1", vpc_id)]).await;
            assert_eq!(
                member(case, &read, "state"),
                "available",
                "case {case:?}: read {nth}: a VPC becomes available on its own once the \
                 transition is due, got {read}"
            );
        }
    }
    let read = describe_ok("with IPv6", &router, &[param("VpcId.1", &dual_id)]).await;
    assert_eq!(
        ipv6_association("with IPv6", &read, "state"),
        "associated",
        "case \"with IPv6\": the IPv6 association is associated once due, got {read}"
    );
}

// ---------------------------------------------------------------------------
// The IPv6 block
// ---------------------------------------------------------------------------

/// The rule: an IPv6 CIDR block is associated with the VPC in the form the request asked
/// for it — Amazon-provided means a `/56` the service picks out of Amazon's own space, a
/// caller-specified block means that block canonicalised, out of the pool the request
/// named — and each combination of the IPv6 members that the members' own documentation
/// excludes is refused with `InvalidParameterCombination`.
#[tokio::test]
async fn an_ipv6_block_is_associated_as_requested_and_excluded_combinations_are_refused() {
    let router = emulator();

    // Amazon-provided: the caller "cannot specify the range of IP addresses, or the size of
    // the CIDR block", so the block is minted with the documented /56 prefix length.
    let (status, body) = create_vpc(
        &router,
        &[
            param("CidrBlock", "10.0.0.0/16"),
            param("AmazonProvidedIpv6CidrBlock", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"amazon-provided\": expected HTTP 200, got {status} with body {body}"
    );
    let minted = ipv6_association("amazon-provided", &body, "ipv6CidrBlock");
    assert!(
        minted.ends_with("/56"),
        "case \"amazon-provided\": the documented prefix length is /56, got {minted:?}"
    );
    // A minted block is already canonical: reading it back through the crate's own CIDR
    // rule leaves it unchanged (which also proves it IS an IPv6 CIDR block).
    assert_eq!(
        ec2::state::canonical_ipv6_cidr(&minted),
        Ok(minted.clone()),
        "case \"amazon-provided\": a minted block is canonical, got {minted:?}"
    );
    assert_eq!(
        ipv6_association("amazon-provided", &body, "ipSource"),
        "amazon",
        "case \"amazon-provided\": the block comes from Amazon, got {body}"
    );
    assert_eq!(
        ipv6_association("amazon-provided", &body, "ipv6AddressAttribute"),
        "public",
        "case \"amazon-provided\": an Amazon-provided block is public, got {body}"
    );
    assert_eq!(
        ipv6_association("amazon-provided", &body, "networkBorderGroup"),
        REGION,
        "case \"amazon-provided\": advertised from the request's Region, got {body}"
    );
    let association_id = ipv6_association("amazon-provided", &body, "associationId");
    assert!(
        is_long_id("vpc-cidr-assoc", &association_id),
        "case \"amazon-provided\": expected a vpc-cidr-assoc-<17 hex> id, got {association_id:?}"
    );
    // And the association reads back through the describe.
    let vpc_id = member("amazon-provided", &body, "vpcId");
    let read = describe_ok("amazon-provided", &router, &[param("VpcId.1", &vpc_id)]).await;
    assert_eq!(
        ipv6_association("amazon-provided", &read, "ipv6CidrBlock"),
        minted,
        "case \"amazon-provided\": the block must survive the round trip, got {read}"
    );

    // A caller-specified block out of a named pool, canonicalised the same way the IPv4
    // block is. The rows differ in how many host bits the request set.
    let pool = "ipv6pool-ec2-0123456789abcdef0";
    let specified = [
        (
            "already canonical",
            "2001:db8:1234:1a00::/56",
            "2001:db8:1234:1a00::/56",
        ),
        (
            "host bits set",
            "2001:db8:1234:1aff:ffff::/56",
            "2001:db8:1234:1a00::/56",
        ),
        ("a /48", "2001:db8:5678::/48", "2001:db8:5678::/48"),
    ];
    for (index, (case, requested, canonical)) in specified.into_iter().enumerate() {
        let cidr = format!("10.{}.0.0/16", index + 10);
        let (status, body) = create_vpc(
            &router,
            &[
                param("CidrBlock", &cidr),
                param("Ipv6CidrBlock", requested),
                param("Ipv6Pool", pool),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            ipv6_association(case, &body, "ipv6CidrBlock"),
            canonical,
            "case {case:?}: expected the canonical block {canonical:?}, got {body}"
        );
        assert_eq!(
            ipv6_association(case, &body, "ipv6Pool"),
            pool,
            "case {case:?}: the association names the pool it came from, got {body}"
        );
        assert_eq!(
            ipv6_association(case, &body, "ipSource"),
            "byoip",
            "case {case:?}: a caller-specified block is brought to AWS, got {body}"
        );
    }

    // The combinations the members' own documentation excludes.
    // (case, the members beside CidrBlock, the EC2 error code)
    let refused: [RefusalCase; 5] = [
        (
            "a range with an Amazon-provided block",
            vec![
                param("AmazonProvidedIpv6CidrBlock", "true"),
                param("Ipv6CidrBlock", "2001:db8:1234:1a00::/56"),
                param("Ipv6Pool", pool),
            ],
            "InvalidParameterCombination",
        ),
        (
            "a pool with an Amazon-provided block",
            vec![
                param("AmazonProvidedIpv6CidrBlock", "true"),
                param("Ipv6Pool", pool),
            ],
            "InvalidParameterCombination",
        ),
        (
            "a block with no pool",
            vec![param("Ipv6CidrBlock", "2001:db8:1234:1a00::/56")],
            "InvalidParameterCombination",
        ),
        (
            "a border group without an Amazon-provided block",
            vec![param(
                "Ipv6CidrBlockNetworkBorderGroup",
                "us-east-1-wl1-bos-wlz-1",
            )],
            "InvalidParameterCombination",
        ),
        (
            "a block that is not an IPv6 CIDR",
            vec![
                param("Ipv6CidrBlock", "2001:db8:1234:1a00/56"),
                param("Ipv6Pool", pool),
            ],
            "InvalidParameterValue",
        ),
    ];
    for (case, members, expected) in refused {
        let mut params = vec![param("CidrBlock", "10.100.0.0/16")];
        params.extend(members);
        let (status, body) = create_vpc(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected the code {expected:?}, got {body}"
        );
    }
}

/// The rule: a request this emulator holds no state to serve is answered with the
/// scaffold's 501 `NotImplemented` — never with a success it could not have computed.
/// There is no IPAM pool and no address pool to allocate out of, and no VPC Encryption
/// Control resource to create.
#[tokio::test]
async fn a_cidr_this_emulator_cannot_allocate_answers_not_implemented() {
    let router = emulator();
    let before = elements(&describe_ok("before", &router, &[]).await, "vpcId");

    // (case, the members beside CidrBlock)
    let cases: [RequestCase; 6] = [
        (
            "an IPv4 IPAM pool",
            vec![param("Ipv4IpamPoolId", "ipam-pool-0123456789abcdef0")],
        ),
        (
            "an IPv6 IPAM pool",
            vec![param("Ipv6IpamPoolId", "ipam-pool-0123456789abcdef0")],
        ),
        (
            "an IPv4 netmask length",
            vec![param("Ipv4NetmaskLength", "16")],
        ),
        (
            "an IPv6 netmask length",
            vec![param("Ipv6NetmaskLength", "56")],
        ),
        (
            "an IPv6 pool with no block",
            vec![param("Ipv6Pool", "ipv6pool-ec2-0123456789abcdef0")],
        ),
        (
            "an encryption control configuration",
            vec![param("VpcEncryptionControl.Mode", "enforce")],
        ),
    ];
    for (case, members) in cases {
        let mut params = vec![param("CidrBlock", "10.0.0.0/16")];
        params.extend(members);
        let (status, body) = create_vpc(&router, &params).await;
        assert_eq!(
            status, 501,
            "case {case:?}: expected HTTP 501, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotImplemented",
            "case {case:?}: expected NotImplemented, got {body}"
        );
    }

    let after = elements(&describe_ok("after the refusals", &router, &[]).await, "vpcId");
    assert_eq!(
        after, before,
        "a request answered with a 501 must store nothing, got {after:?} where {before:?} \
         was there"
    );
}
