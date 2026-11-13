// SPDX-License-Identifier: CC0-1.0

//! The EC2 network a request places a MediaConnect elastic network interface in is resolved
//! against Amazon EC2 — the service that owns it — before any state is written.
//!
//! RULE: a request that places a MediaConnect-managed elastic network interface in a client
//! VPC names the subnet it goes in and the security groups it carries, which are EC2's
//! resources, so the request resolves every one of them against EC2 at call time; a request
//! naming an identifier EC2 answers no such resource for is refused with the modelled
//! `BadRequestException` (HTTP 400) in this service's validation envelope, naming the member
//! at fault and the answer EC2 gave, and stores NOTHING.
//!
//! The members that place one, from
//! `smithy_query(query="shape",shape="VpcRouterNetworkInterfaceConfiguration,VpcInterfaceRequest")`:
//! the router network interface's `Configuration.Vpc` (`SubnetId`, `SecurityGroupIds`, both
//! required, `SecurityGroupIds` with a modelled `length` 1..5), carried by
//! CreateRouterNetworkInterface and UpdateRouterNetworkInterface; and a flow's
//! `VpcInterfaces` entries (`VpcInterfaceRequest$SubnetId` / `$SecurityGroupIds`), carried by
//! CreateFlow and AddFlowVpcInterfaces — whose `VpcInterface$NetworkInterfaceIds` are "IDs of
//! the network interfaces created in customer's account by MediaConnect". A PUBLIC router
//! network interface configuration names no EC2 resource at all, so it resolves nothing.
//!
//! The other half of the rule: a refusal is GROUNDED in an answer. This service only gates on
//! the placement — it keeps the ids the request carried and reports no VPC or zone of its own
//! — so an emulator with no EC2 mounted learns nothing to refuse a request on and serves the
//! same call ([`nothing_is_refused_where_there_was_no_ec2_to_ask`], which is why the sibling
//! suites that build placements from synthetic ids are unaffected).
//!
//! EC2 is mounted beside MediaConnect on ONE engine ([`crate::flow_lifecycle::Emulator::with_ec2`])
//! and every subnet and security group an accepted row names exists only because EC2 minted
//! it, so the resolution under test runs over the same in-process callback as in production
//! and every refusal is EC2's own answer.
//!
//! The rows differ along what the rule generalises over: WHICH member carries the
//! unresolvable value, HOW MANY security groups the placement names (the modelled minimum and
//! maximum), WHERE in that list the unresolvable id sits, the FORM of the value (EC2's 17-hex
//! id form, its older 8-hex form, upper-case hex, an id of the wrong KIND that EC2 really
//! minted, and a value that is no EC2 id at all), WHICH Region the placement is resolved in,
//! and which of the four operations carries it.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};
use crate::flow_vpc_interfaces::{add_vpc_interfaces, vpc_interface_request};
// The reader over a flow's stored VPC interfaces already exists beside
// RemoveFlowVpcInterface's cases, so every suite reads them back through the same one.
use crate::remove_flow_vpc_interface::described_vpc_interfaces;
use crate::router_input_restart::{union, union_member};
use crate::router_network_interfaces::{
    batch_get_network_interfaces, create_network_interface, public_configuration, stored_count,
};

/// A second Region, so a row can assert WHICH Region a placement is resolved in: a subnet
/// belongs to one VPC, which belongs to one Region, so the same id resolves in one Region and
/// in no other.
const OTHER_REGION: &str = "us-west-2";

/// The EC2 error code for an identifier the account holds no subnet for, from EC2's error
/// catalogue — what the refusal relays.
const INVALID_SUBNET_ID_NOT_FOUND: &str = "InvalidSubnetID.NotFound";

/// The EC2 error code for an identifier the account holds no security group for.
const INVALID_GROUP_NOT_FOUND: &str = "InvalidGroup.NotFound";

/// The member path a refusal about a router network interface's subnet names.
const SUBNET_MEMBER: &str = "Configuration.Vpc.SubnetId";

/// The member path a refusal about a router network interface's security groups names.
const GROUP_MEMBER: &str = "Configuration.Vpc.SecurityGroupIds";

/// The modelled minimum and maximum number of security groups one
/// `VpcRouterNetworkInterfaceConfiguration` names (`SecurityGroupIds`, `@length` 1..5).
const MIN_SECURITY_GROUPS: usize = 1;
/// See [`MIN_SECURITY_GROUPS`].
const MAX_SECURITY_GROUPS: usize = 5;

/// MediaConnect mounted beside the OWNER of the subnets and security groups its placements
/// name, on one shared engine, plus the in-process callback the EC2 clients below are built
/// on — the very seam the handlers resolve a placement through.
struct Wired {
    emulator: Emulator,
    callback: ::smithy_server_runtime::EmbeddedEmulator,
}

/// One VPC of the account as EC2 minted it, in one Region.
struct Network {
    region: String,
    subnet_ids: Vec<String>,
    group_ids: Vec<String>,
}

impl Wired {
    fn new() -> Self {
        let (emulator, callback) = Emulator::with_ec2();
        Self { emulator, callback }
    }

    /// An EC2 client answering for `region` over this emulator's own in-process callback.
    fn ec2(&self, region: &str) -> ::aws_sdk_ec2::Client {
        ::aws_sdk_ec2::Client::from_conf(
            ::aws_sdk_ec2::Config::builder()
                .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
                .region(::aws_sdk_ec2::config::Region::new(region.to_string()))
                .endpoint_url(format!("https://ec2.{region}.amazonaws.com"))
                .credentials_provider(self.callback.aws_credentials_provider())
                .http_client(self.callback.http_client())
                .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
                .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
                .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
                .stalled_stream_protection(
                    ::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled(),
                )
                .build(),
        )
    }

    /// One VPC of the account in `region`, with `subnets` subnets and `groups` security
    /// groups in it — all minted by EC2's own operations, so every id below is one EC2
    /// handed out.
    async fn network(&self, region: &str, second_octet: u8, subnets: usize, groups: usize) -> Network {
        let ec2 = self.ec2(region);
        let vpc_id = ec2
            .create_vpc()
            .cidr_block(format!("10.{second_octet}.0.0/16"))
            .send()
            .await
            .expect("setup: EC2 must create the VPC")
            .vpc()
            .and_then(::aws_sdk_ec2::types::Vpc::vpc_id)
            .expect("setup: EC2 must report the VPC's id")
            .to_string();
        let mut subnet_ids = Vec::new();
        for index in 0..subnets {
            subnet_ids.push(
                ec2.create_subnet()
                    .vpc_id(&vpc_id)
                    .cidr_block(format!("10.{second_octet}.{}.0/24", index + 1))
                    .availability_zone(format!("{region}a"))
                    .send()
                    .await
                    .expect("setup: EC2 must create the subnet")
                    .subnet()
                    .and_then(::aws_sdk_ec2::types::Subnet::subnet_id)
                    .expect("setup: EC2 must report the subnet's id")
                    .to_string(),
            );
        }
        let mut group_ids = Vec::new();
        for index in 0..groups {
            group_ids.push(
                ec2.create_security_group()
                    .vpc_id(&vpc_id)
                    .group_name(format!("mediaconnect-{second_octet}-{index}"))
                    .description("a group named by a MediaConnect VPC placement")
                    .send()
                    .await
                    .expect("setup: EC2 must create the security group")
                    .group_id()
                    .expect("setup: EC2 must report the group's id")
                    .to_string(),
            );
        }
        Network {
            region: region.to_string(),
            subnet_ids,
            group_ids,
        }
    }
}

/// A VPC router network interface configuration naming `subnet` and `groups`.
fn vpc_configuration(subnet: &str, groups: &[String]) -> Value {
    union(
        "Vpc",
        "vpc",
        json!({ "securityGroupIds": groups, "subnetId": subnet }),
    )
}

/// The configuration the interface `arn` holds, read back through the implemented reader.
async fn stored_configuration(wired: &Wired, case: &str, arn: &str) -> Value {
    let (status, code, read) = batch_get_network_interfaces(&wired.emulator, &[arn]).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: expected 200 from the batch read, got {status} {code:?}: {read}"
    );
    read["routerNetworkInterfaces"][0]["configuration"].clone()
}

/// Assert `response` is this service's refusal for a placement EC2 resolved nothing for:
/// the modelled `BadRequestException`, in the validation envelope, naming `member` and the
/// EC2 code `code` that grounds it.
fn assert_placement_refusal(
    case: &str,
    status: u16,
    code: Option<&str>,
    response: &Value,
    member: &str,
    ec2_code: &str,
) {
    assert_eq!(
        (status, code),
        (400, Some("BadRequestException")),
        "case {case:?}: expected 400 BadRequestException, got {status} {code:?}: {response}"
    );
    let message = response["message"].as_str().unwrap_or_default();
    assert!(
        message.starts_with(mc::VALIDATION_FAILED_PREFIX),
        "case {case:?}: the refusal carries this service's validation envelope, got {message:?}"
    );
    assert!(
        message.contains(member),
        "case {case:?}: the refusal names the member at fault ({member}), got {message:?}"
    );
    assert!(
        message.contains(ec2_code),
        "case {case:?}: the refusal relays the answer EC2 gave ({ec2_code}), got {message:?}"
    );
}

/// Every unresolvable value a row below names, as `(form, the value)`.
///
/// `suffix` is what follows the kind prefix: EC2's 17-hex id form, its older 8-hex form,
/// upper-case hex (which no normalisation may fold), and — for a member the model puts no
/// pattern on — a value that is no EC2 id at all.
fn unresolvable_forms(prefix: &str) -> Vec<(String, String)> {
    vec![
        (
            "EC2's 17-hex id form".to_string(),
            format!("{prefix}-0f1e2d3c4b5a69780"),
        ),
        (
            "EC2's older 8-hex id form".to_string(),
            format!("{prefix}-1a2b3c4d"),
        ),
        (
            "UPPER-CASE hex, which no normalisation may fold".to_string(),
            format!("{prefix}-ABCDEF0123456789A"),
        ),
        (
            "a value that is no EC2 identifier at all".to_string(),
            "not-an-ec2-id".to_string(),
        ),
    ]
}

/// A create naming `configuration`, in the Region `region_name` when it names one.
async fn create_in_region(
    wired: &Wired,
    name: &str,
    configuration: Value,
    region_name: Option<&str>,
) -> (u16, Option<String>, Value) {
    let mut body = json!({ "name": name, "configuration": configuration });
    if let Some(region) = region_name {
        body["regionName"] = json!(region);
    }
    create_network_interface(&wired.emulator, body).await
}

#[tokio::test]
async fn a_router_network_interface_placement_is_resolved_against_ec2_before_it_is_stored() {
    let wired = Wired::new();
    let home = wired
        .network(REGION, 90, 2, MAX_SECURITY_GROUPS)
        .await;
    let second = wired.network(REGION, 91, 1, 1).await;
    let elsewhere = wired.network(OTHER_REGION, 92, 1, 1).await;

    // The accepted side: each configuration names what EC2 itself minted, so each is
    // accepted — and the interface reads back carrying exactly it.
    let accepted: Vec<(String, Value, Option<&str>)> = vec![
        (
            "a PUBLIC configuration, which names no EC2 resource".to_string(),
            public_configuration(2),
            None,
        ),
        (
            format!("vpc at the modelled minimum of {MIN_SECURITY_GROUPS} security group"),
            vpc_configuration(&home.subnet_ids[0], &home.group_ids[..MIN_SECURITY_GROUPS]),
            None,
        ),
        (
            format!("vpc at the modelled maximum of {MAX_SECURITY_GROUPS} security groups"),
            vpc_configuration(&home.subnet_ids[1], &home.group_ids),
            None,
        ),
        (
            "vpc in another VPC of the same account".to_string(),
            vpc_configuration(&second.subnet_ids[0], &second.group_ids),
            None,
        ),
        (
            "vpc in another Region's own subnet, named by RegionName".to_string(),
            vpc_configuration(&elsewhere.subnet_ids[0], &elsewhere.group_ids),
            Some(elsewhere.region.as_str()),
        ),
    ];

    for (case, configuration, region_name) in &accepted {
        let (status, code, created) =
            create_in_region(&wired, "rni-placed", configuration.clone(), *region_name).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: a create naming EC2's own resources must be accepted, got {status} \
             {code:?}: {created}"
        );
        let arn = created["routerNetworkInterface"]["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create reports no arn: {created}"))
            .to_string();
        let stored = stored_configuration(&wired, case, &arn).await;
        // Both variants are compared, so the PUBLIC row asserts its own payload rather than
        // the absence of a VPC one.
        for (member, json_name) in [("Public", "public"), ("Vpc", "vpc")] {
            assert_eq!(
                union_member(&stored, member, json_name),
                union_member(configuration, member, json_name),
                "case {case:?}: the read must report the configuration the create stored, got \
                 {stored}"
            );
        }
    }

    // The refusal side. One row per way a member names no resource of the account: each row
    // has exactly ONE unresolvable value, and states the member and the EC2 code the refusal
    // has to name.
    //
    // (case, the configuration, RegionName, the member at fault, EC2's error code)
    let mut rows: Vec<(String, Value, Option<&str>, &str, &str)> = Vec::new();
    for (form, unknown) in unresolvable_forms("subnet") {
        rows.push((
            format!("a subnet EC2 does not hold, in {form}"),
            vpc_configuration(&unknown, &home.group_ids[..MIN_SECURITY_GROUPS]),
            None,
            SUBNET_MEMBER,
            INVALID_SUBNET_ID_NOT_FOUND,
        ));
    }
    for (form, unknown) in unresolvable_forms("sg") {
        rows.push((
            format!("the only security group EC2 does not hold, in {form}"),
            vpc_configuration(&home.subnet_ids[0], ::std::slice::from_ref(&unknown)),
            None,
            GROUP_MEMBER,
            INVALID_GROUP_NOT_FOUND,
        ));
        rows.push((
            format!("the LAST of three security groups EC2 does not hold, in {form}"),
            vpc_configuration(
                &home.subnet_ids[0],
                &[
                    home.group_ids[0].clone(),
                    home.group_ids[1].clone(),
                    unknown.clone(),
                ],
            ),
            None,
            GROUP_MEMBER,
            INVALID_GROUP_NOT_FOUND,
        ));
        rows.push((
            format!("the FIRST of two security groups EC2 does not hold, in {form}"),
            vpc_configuration(
                &home.subnet_ids[0],
                &[unknown, home.group_ids[0].clone()],
            ),
            None,
            GROUP_MEMBER,
            INVALID_GROUP_NOT_FOUND,
        ));
    }
    // Ids of the WRONG KIND that EC2 really minted: neither member carries a modelled
    // pattern, so a security group id reaches the handler as a subnet and a subnet id as a
    // security group — and neither names the resource its member must name.
    rows.push((
        "a real security group id used as the subnet".to_string(),
        vpc_configuration(&home.group_ids[0], &home.group_ids[..MIN_SECURITY_GROUPS]),
        None,
        SUBNET_MEMBER,
        INVALID_SUBNET_ID_NOT_FOUND,
    ));
    rows.push((
        "a real subnet id used as a security group".to_string(),
        vpc_configuration(&home.subnet_ids[0], &[home.subnet_ids[0].clone()]),
        None,
        GROUP_MEMBER,
        INVALID_GROUP_NOT_FOUND,
    ));
    // The Region the placement is resolved in is the Region the interface is placed in: this
    // account's own subnet, named for a Region whose network does not hold it.
    rows.push((
        "this Region's own subnet, named for another Region".to_string(),
        vpc_configuration(&home.subnet_ids[0], &home.group_ids[..MIN_SECURITY_GROUPS]),
        Some(OTHER_REGION),
        SUBNET_MEMBER,
        INVALID_SUBNET_ID_NOT_FOUND,
    ));

    let held = stored_count(&wired.emulator);
    for (case, configuration, region_name, member, ec2_code) in &rows {
        let (status, code, answer) =
            create_in_region(&wired, "rni-refused", configuration.clone(), *region_name).await;
        assert_placement_refusal(case, status, code.as_deref(), &answer, member, ec2_code);
        assert_eq!(
            stored_count(&wired.emulator),
            held,
            "case {case:?}: a refused create must mint no router network interface"
        );
    }
}

/// Wait until the clock has settled the interface `arn` into ACTIVE, reading it back through
/// the implemented reader — the condition an update needs, rather than a span of time (an
/// update MOVES the interface's state, so it conflicts while a change is still in flight).
async fn settled_interface(wired: &Wired, arn: &str) {
    /// Generous: the transition is due one [`mc::TRANSITION_SPAN`] after the create, and this
    /// only bounds a loop that would otherwise never end.
    const DEADLINE: Duration = Duration::from_secs(120);
    /// How long the loop waits before looking again.
    const BETWEEN_READS: Duration = Duration::from_millis(25);

    let started = Instant::now();
    loop {
        let (status, code, read) = batch_get_network_interfaces(&wired.emulator, &[arn]).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "setup: expected 200 from the batch read, got {status} {code:?}: {read}"
        );
        if read["routerNetworkInterfaces"][0]["state"]
            == json!(mc::ROUTER_NETWORK_INTERFACE_STATE_ACTIVE)
        {
            return;
        }
        assert!(
            started.elapsed() < DEADLINE,
            "setup: the clock must settle the created interface into {}, last read {read}",
            mc::ROUTER_NETWORK_INTERFACE_STATE_ACTIVE
        );
        tokio::time::sleep(BETWEEN_READS).await;
    }
}

/// UpdateRouterNetworkInterface, which carries the same `Configuration` union the create
/// does (`smithy_query(query="operation",operation="UpdateRouterNetworkInterface")`:
/// `PUT /v1/routerNetworkInterface/{Arn}`).
async fn update_placement(
    wired: &Wired,
    arn: &str,
    configuration: Value,
) -> (u16, Option<String>, Value) {
    wired
        .emulator
        .send(
            "PUT",
            &format!("/v1/routerNetworkInterface/{arn}"),
            Some(json!({ "configuration": configuration })),
        )
        .await
}

#[tokio::test]
async fn an_updated_router_network_interface_placement_is_resolved_the_same_way() {
    let wired = Wired::new();
    let home = wired.network(REGION, 80, 2, 2).await;

    let (status, code, created) = create_in_region(
        &wired,
        "rni-updated",
        vpc_configuration(&home.subnet_ids[0], &home.group_ids[..1]),
        None,
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "setup: the interface under test must be created, got {status} {code:?}: {created}"
    );
    let arn = created["routerNetworkInterface"]["arn"]
        .as_str()
        .expect("setup: the create reports an arn")
        .to_string();
    settled_interface(&wired, &arn).await;

    let update = |configuration: Value| update_placement(&wired, &arn, configuration);

    // The accepted side: a replacement placement EC2 resolves is applied, and reads back.
    let replacement = vpc_configuration(&home.subnet_ids[1], &home.group_ids);
    let (status, code, applied) = update(replacement.clone()).await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case \"an update naming EC2's own resources\": expected 202, got {status} {code:?}: \
         {applied}"
    );
    let held = stored_configuration(&wired, "after the accepted update", &arn).await;
    assert_eq!(
        union_member(&held, "Vpc", "vpc"),
        union_member(&replacement, "Vpc", "vpc"),
        "case \"an update naming EC2's own resources\": the stored configuration must be the \
         one the update named, got {held}"
    );

    // The refusal side: the same rule, and the stored configuration survives untouched. The
    // accepted update above moved the interface into UPDATING, and an interface between two
    // states refuses an update whatever it carries, so the rows below wait for the clock to
    // settle it first.
    settled_interface(&wired, &arn).await;
    let rows: Vec<(String, Value, &str, &str)> = vec![
        (
            "a subnet EC2 does not hold".to_string(),
            vpc_configuration("subnet-0f1e2d3c4b5a69780", &home.group_ids[..1]),
            SUBNET_MEMBER,
            INVALID_SUBNET_ID_NOT_FOUND,
        ),
        (
            "a security group EC2 does not hold".to_string(),
            vpc_configuration(&home.subnet_ids[0], &["sg-1a2b3c4d".to_string()]),
            GROUP_MEMBER,
            INVALID_GROUP_NOT_FOUND,
        ),
        (
            "a value that is no EC2 identifier at all".to_string(),
            vpc_configuration("not-a-subnet", &["nope".to_string()]),
            SUBNET_MEMBER,
            INVALID_SUBNET_ID_NOT_FOUND,
        ),
    ];
    for (case, configuration, member, ec2_code) in &rows {
        let (status, code, answer) = update(configuration.clone()).await;
        assert_placement_refusal(case, status, code.as_deref(), &answer, member, ec2_code);
        assert_eq!(
            stored_configuration(&wired, case, &arn).await,
            held,
            "case {case:?}: a refused update must leave the stored configuration alone"
        );
    }
}

/// How many flows the store holds, so a row can assert that a refused create minted none.
fn flow_count(emulator: &Emulator) -> usize {
    emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>()
        .lock()
        .flows
        .len()
}

/// A flow body carrying `vpc_interfaces`, named `name`.
fn flow_body(name: &str, vpc_interfaces: Value) -> Value {
    json!({
        "name": name,
        "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" },
        "vpcInterfaces": vpc_interfaces,
    })
}

/// A subnet id and a security group id are values EC2 minted, and it mints none that is
/// empty, so a placement leaving either member empty names no resource of any account: that
/// is answerable from the REQUEST alone, so it is refused whether or not there is an EC2 to
/// ask — which is what this asserts, over an emulator with EC2 mounted and one without.
///
/// `group` is a security group id the rows that are not about the group list name; the rows
/// below never resolve, so it need only be a well-formed id.
async fn assert_empty_members_are_refused(emulator: &Emulator, mounted: &str, group: &str) {
    // (case, the create, the member the refusal names)
    let interfaces_before = stored_count(emulator);
    let flows_before = flow_count(emulator);

    let (status, code, answer) = create_network_interface(
        emulator,
        json!({
            "name": "rni-empty-subnet",
            "configuration": vpc_configuration("", &[group.to_string()]),
        }),
    )
    .await;
    let case = format!("a router network interface configuration with an empty SubnetId, {mounted}");
    assert_empty_member_refusal(&case, status, code.as_deref(), &answer, SUBNET_MEMBER);
    assert_eq!(
        stored_count(emulator),
        interfaces_before,
        "case {case:?}: a refused create must mint no router network interface"
    );

    for (case, vpc_interface, member) in [
        (
            "a flow VPC interface with an empty SubnetId",
            vpc_interface_request("vpc-empty-subnet", None, &[group], "", None),
            "VpcInterfaces[0].SubnetId",
        ),
        (
            "a flow VPC interface naming no security group",
            vpc_interface_request("vpc-no-groups", None, &[], &format!("subnet-{:017x}", 1), None),
            "VpcInterfaces[0].SecurityGroupIds",
        ),
    ] {
        let case = format!("{case}, {mounted}");
        let (status, code, answer) = emulator
            .create_flow(flow_body("flow-empty-member", json!([vpc_interface])))
            .await;
        assert_empty_member_refusal(&case, status, code.as_deref(), &answer, member);
        assert_eq!(
            flow_count(emulator),
            flows_before,
            "case {case:?}: a refused create must mint no flow"
        );
    }
}

/// Assert `response` is this service's refusal for a placement member that names nothing:
/// the modelled `BadRequestException`, in the validation envelope, naming `member`.
fn assert_empty_member_refusal(
    case: &str,
    status: u16,
    code: Option<&str>,
    response: &Value,
    member: &str,
) {
    assert_eq!(
        (status, code),
        (400, Some("BadRequestException")),
        "case {case:?}: expected 400 BadRequestException, got {status} {code:?}: {response}"
    );
    let message = response["message"].as_str().unwrap_or_default();
    assert!(
        message.starts_with(mc::VALIDATION_FAILED_PREFIX) && message.contains(member),
        "case {case:?}: the refusal names the member at fault ({member}) in this service's \
         validation envelope, got {message:?}"
    );
}

#[tokio::test]
async fn a_placement_member_that_names_nothing_is_refused_from_the_request_alone() {
    let wired = Wired::new();
    let home = wired.network(REGION, 60, 1, 1).await;
    assert_empty_members_are_refused(
        &wired.emulator,
        "with EC2 mounted",
        home.group_ids[0].as_str(),
    )
    .await;

    // The same refusals with no EC2 to ask: these name nothing whatever the account holds,
    // so unlike an unresolvable id they are not grounded in an answer.
    assert_empty_members_are_refused(
        &Emulator::new(),
        "with no EC2 mounted",
        "sg-0f1e2d3c4b5a69780",
    )
    .await;
}

#[tokio::test]
async fn a_flow_vpc_interface_placement_is_resolved_against_ec2_too() {
    let wired = Wired::new();
    let home = wired.network(REGION, 70, 2, 2).await;
    let unknown_subnet = "subnet-0f1e2d3c4b5a69780";
    let unknown_group = "sg-0f1e2d3c4b5a69780";

    // CreateFlow: a flow whose VPC interfaces name EC2's own resources is created, and the
    // interfaces read back out of the stored flow.
    let good = vpc_interface_request(
        "vpc-one",
        None,
        &[home.group_ids[0].as_str(), home.group_ids[1].as_str()],
        &home.subnet_ids[0],
        None,
    );
    let (status, code, created) = wired
        .emulator
        .create_flow(flow_body("flow-placed", json!([good.clone()])))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case \"CreateFlow naming EC2's own resources\": expected 201, got {status} {code:?}: \
         {created}"
    );
    let flow_arn = created["flow"]["flowArn"]
        .as_str()
        .expect("the create reports a flowArn")
        .to_string();
    let reported =
        described_vpc_interfaces(&wired.emulator, "CreateFlow naming EC2's own", &flow_arn).await;
    assert_eq!(
        (
            reported[0]["subnetId"].clone(),
            reported[0]["securityGroupIds"].clone()
        ),
        (
            json!(home.subnet_ids[0]),
            json!([home.group_ids[0], home.group_ids[1]])
        ),
        "case \"CreateFlow naming EC2's own resources\": the stored VPC interface must carry \
         the placement the request named, got {reported:?}"
    );

    // Every way one entry of a request can name no resource of the account, over both
    // operations that add a VPC interface to a flow: which member carries it, and where in a
    // multi-entry request that entry sits.
    //
    // (case, the VpcInterfaces list, the member at fault, EC2's error code)
    let rows: Vec<(String, Value, String, &str)> = vec![
        (
            "a subnet EC2 does not hold".to_string(),
            json!([vpc_interface_request(
                "vpc-bad-subnet",
                None,
                &[home.group_ids[0].as_str()],
                unknown_subnet,
                None,
            )]),
            "VpcInterfaces[0].SubnetId".to_string(),
            INVALID_SUBNET_ID_NOT_FOUND,
        ),
        (
            "a security group EC2 does not hold".to_string(),
            json!([vpc_interface_request(
                "vpc-bad-group",
                None,
                &[unknown_group],
                &home.subnet_ids[0],
                None,
            )]),
            "VpcInterfaces[0].SecurityGroupIds".to_string(),
            INVALID_GROUP_NOT_FOUND,
        ),
        (
            "the SECOND entry of a request naming a subnet EC2 does not hold".to_string(),
            json!([
                vpc_interface_request(
                    "vpc-good",
                    None,
                    &[home.group_ids[0].as_str()],
                    &home.subnet_ids[0],
                    None,
                ),
                vpc_interface_request(
                    "vpc-second-bad",
                    None,
                    &[home.group_ids[0].as_str()],
                    "not-an-ec2-id",
                    None,
                ),
            ]),
            "VpcInterfaces[1].SubnetId".to_string(),
            INVALID_SUBNET_ID_NOT_FOUND,
        ),
    ];

    let flows_held = flow_count(&wired.emulator);
    let interfaces_held =
        described_vpc_interfaces(&wired.emulator, "before the refusals", &flow_arn).await;
    for (case, vpc_interfaces, member, ec2_code) in &rows {
        // CreateFlow: refused, and no flow minted for it.
        let create_case = format!("CreateFlow with {case}");
        let (status, code, answer) = wired
            .emulator
            .create_flow(flow_body("flow-refused", vpc_interfaces.clone()))
            .await;
        assert_placement_refusal(
            &create_case,
            status,
            code.as_deref(),
            &answer,
            member,
            ec2_code,
        );
        assert_eq!(
            flow_count(&wired.emulator),
            flows_held,
            "case {create_case:?}: a refused create must mint no flow"
        );

        // AddFlowVpcInterfaces: refused the same way, and the flow keeps the VPC interfaces
        // it already had.
        let add_case = format!("AddFlowVpcInterfaces with {case}");
        let (status, code, answer) =
            add_vpc_interfaces(&wired.emulator, &flow_arn, vpc_interfaces.clone()).await;
        assert_placement_refusal(&add_case, status, code.as_deref(), &answer, member, ec2_code);
        assert_eq!(
            described_vpc_interfaces(&wired.emulator, &add_case, &flow_arn).await,
            interfaces_held,
            "case {add_case:?}: a refused add must leave the flow's VPC interfaces alone"
        );
    }

    // A placement naming more security groups than this service resolves is refused from the
    // request ALONE, before EC2 is asked: the list is caller-sized, and it is clamped before
    // it is cloned into the resolving call. (`VpcInterfaceRequest$SecurityGroupIds` carries no
    // modelled bound, which is the only member this ceiling ever decides.)
    let past_the_bound: Vec<String> = (0..=mc::MAX_PLACEMENT_SECURITY_GROUPS)
        .map(|index| format!("sg-{index:017x}"))
        .collect();
    let past_the_bound: Vec<&str> = past_the_bound.iter().map(String::as_str).collect();
    let (status, code, answer) = wired
        .emulator
        .create_flow(flow_body(
            "flow-too-many-groups",
            json!([vpc_interface_request(
                "vpc-too-many-groups",
                None,
                &past_the_bound,
                &home.subnet_ids[0],
                None,
            )]),
        ))
        .await;
    let case = format!(
        "one security group past the {} this service resolves",
        mc::MAX_PLACEMENT_SECURITY_GROUPS
    );
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "case {case:?}: expected 400 BadRequestException, got {status} {code:?}: {answer}"
    );
    let message = answer["message"].as_str().unwrap_or_default();
    assert!(
        message.starts_with(mc::VALIDATION_FAILED_PREFIX)
            && message.contains("VpcInterfaces[0].SecurityGroupIds"),
        "case {case:?}: the refusal names the member at fault in this service's validation \
         envelope, got {message:?}"
    );
    assert_eq!(
        flow_count(&wired.emulator),
        flows_held,
        "case {case:?}: a refused create must mint no flow"
    );

    // And an add naming EC2's own resources is still served after the refusals.
    let (status, code, added) = add_vpc_interfaces(
        &wired.emulator,
        &flow_arn,
        json!([vpc_interface_request(
            "vpc-two",
            None,
            &[home.group_ids[1].as_str()],
            &home.subnet_ids[1],
            None,
        )]),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case \"an add naming EC2's own resources after the refusals\": expected 201, got \
         {status} {code:?}: {added}"
    );
    assert_eq!(
        described_vpc_interfaces(&wired.emulator, "after the accepted add", &flow_arn)
            .await
            .len(),
        2,
        "case \"an add naming EC2's own resources after the refusals\": the flow must hold \
         both VPC interfaces"
    );
}

#[tokio::test]
async fn nothing_is_refused_where_there_was_no_ec2_to_ask() {
    // With no EC2 mounted this service learned nothing to refuse a placement on: it gates on
    // existence and keeps nothing of the network it resolved, so the same requests are
    // served rather than refused from silence.
    let emulator = Emulator::new();
    let configuration = vpc_configuration(
        "subnet-0f1e2d3c4b5a69780",
        &["sg-0f1e2d3c4b5a69780".to_string()],
    );

    let (status, code, created) = create_network_interface(
        &emulator,
        json!({ "name": "rni-no-ec2", "configuration": configuration.clone() }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case \"a create with no EC2 mounted\": expected 201, got {status} {code:?}: {created}"
    );
    assert_eq!(
        union_member(&created["routerNetworkInterface"]["configuration"], "Vpc", "vpc"),
        union_member(&configuration, "Vpc", "vpc"),
        "case \"a create with no EC2 mounted\": the configuration must be served as given, got \
         {created}"
    );

    let (status, code, flow) = emulator
        .create_flow(flow_body(
            "flow-no-ec2",
            json!([vpc_interface_request(
                "vpc-no-ec2",
                None,
                &["sg-0f1e2d3c4b5a69780"],
                "subnet-0f1e2d3c4b5a69780",
                None,
            )]),
        ))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case \"a flow create with no EC2 mounted\": expected 201, got {status} {code:?}: {flow}"
    );
}
