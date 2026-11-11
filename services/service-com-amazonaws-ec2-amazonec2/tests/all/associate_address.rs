// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `AssociateAddress`, driven over the wire the service actually
//! speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. `AssociateAddress` BINDS an Elastic IP address the account holds to one of the
//!    account's instances and PERSISTS that binding: the service mints the association id,
//!    `DescribeAddresses` reports it with the instance, the instance's primary network
//!    interface — the one an instance carries a public address through — and the instance's
//!    private address, and the instance reports the Elastic IP address as its public address
//!    through `DescribeInstances` — however the request named the address and whether or not
//!    it named the private address or the reassociation flag;
//! 2. the binding is IDEMPOTENT ("this is an idempotent operation. If you perform the
//!    operation more than once, Amazon EC2 doesn't return an error") and it MOVES on
//!    request: an address associated again elsewhere leaves the resource it was on, and an
//!    instance given a second address gives up the first, "which remains allocated to your
//!    account";
//! 3. a request naming an address, an instance or a network interface the account does not
//!    hold, naming neither or both of a pair the model says are exclusive, naming a private
//!    address the instance does not have, or asking to remap an address with
//!    `AllowReassociation=false`, is refused with EC2's own error code and changes NOTHING
//!    — and `DryRun` on a request that would have succeeded changes nothing either.
//!
//! Each table row is a distinct input along what the rule generalises over — which member
//! names the address, which flags are set, which id form is named, which resource kind is
//! unknown — so no row is the specification.

use std::collections::BTreeSet;

use service_com_amazonaws_ec2_amazonec2::state::ELASTIC_IP_ASSOCIATION_ID_PREFIX;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query;

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The image every launch here uses; `RunInstances` only needs one that resolves.
const AMI: &str = "ami-0123456789abcdef0";

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

fn emulator() -> MultiServiceRouter {
    ec2_query::emulator().0
}

async fn api(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    ec2_query::call(router, REGION, action, params).await
}

/// Allocate one Elastic IP address and answer `(allocation id, public ip)`.
async fn allocate(router: &MultiServiceRouter) -> (String, String) {
    let (status, body) = api(router, "AllocateAddress", &[]).await;
    assert_eq!(
        status, 200,
        "AllocateAddress must succeed, got {status} with body {body}"
    );
    let allocation_id = ec2_query::element(&body, "allocationId")
        .unwrap_or_else(|| panic!("AllocateAddress must mint an allocation id: {body}"));
    let public_ip = ec2_query::element(&body, "publicIp")
        .unwrap_or_else(|| panic!("AllocateAddress must mint an address: {body}"));
    (allocation_id, public_ip)
}

/// Launch one instance and answer `(instance id, its primary private address)`.
async fn launch(router: &MultiServiceRouter) -> (String, String) {
    let params = vec![p("ImageId", AMI), p("MinCount", "1"), p("MaxCount", "1")];
    let (status, body) = api(router, "RunInstances", &params).await;
    assert_eq!(
        status, 200,
        "RunInstances must succeed, got {status} with body {body}"
    );
    let instance_id = ec2_query::element(&body, "instanceId")
        .unwrap_or_else(|| panic!("RunInstances must report the instance it minted: {body}"));
    let private_ip = ec2_query::element(&body, "privateIpAddress")
        .unwrap_or_else(|| panic!("RunInstances must report the instance's private ip: {body}"));
    (instance_id, private_ip)
}

/// One `AssociateAddress` call carrying `params`, as its HTTP status and body.
async fn associate(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    api(router, "AssociateAddress", params).await
}

/// One `AssociateAddress` call that must succeed, as the association id it answered.
async fn associated(router: &MultiServiceRouter, case: &str, params: &[(String, String)]) -> String {
    let (status, body) = associate(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from AssociateAddress, got {status} with body {body}"
    );
    let association_id = ec2_query::element(&body, "associationId").unwrap_or_default();
    assert!(
        is_association_id(&association_id),
        "case {case:?}: expected an {ELASTIC_IP_ASSOCIATION_ID_PREFIX}- association id, \
         got {association_id:?}"
    );
    association_id
}

/// An association id is [`ELASTIC_IP_ASSOCIATION_ID_PREFIX`] followed by 8 or 17 lowercase
/// hexadecimal digits — the short and the long EC2 resource id form.
fn is_association_id(candidate: &str) -> bool {
    candidate
        .strip_prefix(&format!("{ELASTIC_IP_ASSOCIATION_ID_PREFIX}-"))
        .is_some_and(|suffix| {
            matches!(suffix.len(), 8 | 17)
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
}

/// The members of one stored address the assertions below read.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AddressRow {
    allocation_id: Option<String>,
    public_ip: Option<String>,
    association_id: Option<String>,
    instance_id: Option<String>,
    private_ip_address: Option<String>,
    network_interface_id: Option<String>,
}

/// The one address `allocation_id` names, as `DescribeAddresses` reports it.
async fn stored_address(
    router: &MultiServiceRouter,
    case: &str,
    allocation_id: &str,
) -> AddressRow {
    let params = vec![p("AllocationId.1", allocation_id)];
    let (status, body) = api(router, "DescribeAddresses", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeAddresses must report {allocation_id}, \
         got {status} with body {body}"
    );
    AddressRow {
        allocation_id: ec2_query::element(&body, "allocationId"),
        public_ip: ec2_query::element(&body, "publicIp"),
        association_id: ec2_query::element(&body, "associationId"),
        instance_id: ec2_query::element(&body, "instanceId"),
        private_ip_address: ec2_query::element(&body, "privateIpAddress"),
        network_interface_id: ec2_query::element(&body, "networkInterfaceId"),
    }
}

/// The public address `DescribeInstances` reports for `instance_id` (`Instance$ipAddress`).
async fn instance_public_ip(
    router: &MultiServiceRouter,
    case: &str,
    instance_id: &str,
) -> Option<String> {
    let params = vec![p("InstanceId.1", instance_id)];
    let (status, body) = api(router, "DescribeInstances", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInstances must report {instance_id}, \
         got {status} with body {body}"
    );
    ec2_query::element(&body, "ipAddress")
}

/// The id of the instance's PRIMARY network interface, the one its launch created — what an
/// association named with an `InstanceId` binds the Elastic IP address to, read through the
/// harness every suite that needs it shares.
async fn primary_interface(router: &MultiServiceRouter, case: &str, instance_id: &str) -> String {
    ec2_query::primary_network_interface(router, REGION, case, instance_id).await
}

/// One row of the association table: which members name the one address and the one
/// instance this call binds together.
struct AssociateRow {
    case: &'static str,
    /// Name the address by its public address — `PublicIp`, the deprecated member — rather
    /// than by its allocation id.
    by_public_ip: bool,
    /// Name the instance's primary private address explicitly, rather than letting the
    /// default pick it.
    name_private_ip: bool,
    allow_reassociation: Option<bool>,
}

/// Rule 1: the association is minted, persisted and reported from BOTH sides, for every
/// way a request can name one address and one instance.
#[tokio::test]
async fn associating_an_address_with_an_instance_persists_the_binding() {
    let rows = [
        AssociateRow {
            case: "the address named by its allocation id",
            by_public_ip: false,
            name_private_ip: false,
            allow_reassociation: None,
        },
        AssociateRow {
            case: "the address named by its public address",
            by_public_ip: true,
            name_private_ip: false,
            allow_reassociation: None,
        },
        AssociateRow {
            case: "the instance's primary private address named explicitly",
            by_public_ip: false,
            name_private_ip: true,
            allow_reassociation: None,
        },
        AssociateRow {
            case: "reassociation allowed explicitly",
            by_public_ip: false,
            name_private_ip: false,
            allow_reassociation: Some(true),
        },
        AssociateRow {
            case: "reassociation refused explicitly, on an address nothing holds",
            by_public_ip: true,
            name_private_ip: true,
            allow_reassociation: Some(false),
        },
    ];

    let router = emulator();
    let mut association_ids: BTreeSet<String> = BTreeSet::new();
    for row in &rows {
        let case = row.case;
        let (allocation_id, public_ip) = allocate(&router).await;
        let (instance_id, private_ip) = launch(&router).await;

        let mut params = Vec::new();
        if row.by_public_ip {
            params.push(p("PublicIp", &public_ip));
        } else {
            params.push(p("AllocationId", &allocation_id));
        }
        params.push(p("InstanceId", &instance_id));
        if row.name_private_ip {
            params.push(p("PrivateIpAddress", &private_ip));
        }
        if let Some(allow) = row.allow_reassociation {
            params.push(p("AllowReassociation", allow.to_string()));
        }

        let association_id = associated(&router, case, &params).await;
        assert!(
            association_ids.insert(association_id.clone()),
            "case {case:?}: every association gets its own id, but {association_id} \
             was handed out twice"
        );

        let network_interface_id = primary_interface(&router, case, &instance_id).await;
        let stored = stored_address(&router, case, &allocation_id).await;
        assert_eq!(
            stored,
            AddressRow {
                allocation_id: Some(allocation_id.clone()),
                public_ip: Some(public_ip.clone()),
                association_id: Some(association_id.clone()),
                instance_id: Some(instance_id.clone()),
                private_ip_address: Some(private_ip.clone()),
                // An instance carries an Elastic IP address through its primary network
                // interface, so that is the interface the association names.
                network_interface_id: Some(network_interface_id),
            },
            "case {case:?}: DescribeAddresses must report the stored association, \
             got {stored:?}"
        );

        let reported = instance_public_ip(&router, case, &instance_id).await;
        assert_eq!(
            reported,
            Some(public_ip.clone()),
            "case {case:?}: the instance must report the Elastic IP address as its \
             public address, got {reported:?}"
        );
    }
}

/// Rule 2a: the operation is idempotent — repeating it answers the same association id and
/// leaves the stored record exactly as it was.
#[tokio::test]
async fn repeating_the_same_association_changes_nothing() {
    let router = emulator();
    let (allocation_id, public_ip) = allocate(&router).await;
    let (instance_id, private_ip) = launch(&router).await;

    // Rows: the same binding asked for in each of the ways a request can spell it. Every
    // one of them names the same address, the same instance and the same private address,
    // so none of them is a new association.
    let repeats: [(&str, Params); 4] = [
        (
            "the first call",
            vec![p("AllocationId", &allocation_id), p("InstanceId", &instance_id)],
        ),
        (
            "the same call again",
            vec![p("AllocationId", &allocation_id), p("InstanceId", &instance_id)],
        ),
        (
            "the same binding with the private address named",
            vec![
                p("AllocationId", &allocation_id),
                p("InstanceId", &instance_id),
                p("PrivateIpAddress", &private_ip),
            ],
        ),
        (
            "the same binding by the address's public address, reassociation refused",
            vec![
                p("PublicIp", &public_ip),
                p("InstanceId", &instance_id),
                p("AllowReassociation", "false"),
            ],
        ),
    ];

    let mut first: Option<String> = None;
    let mut first_record: Option<AddressRow> = None;
    for (case, params) in &repeats {
        let association_id = associated(&router, case, params).await;
        let stored = stored_address(&router, case, &allocation_id).await;
        match (&first, &first_record) {
            (Some(expected), Some(expected_record)) => {
                assert_eq!(
                    &association_id, expected,
                    "case {case:?}: an association that already holds keeps its id, \
                     got {association_id}"
                );
                assert_eq!(
                    &stored, expected_record,
                    "case {case:?}: the stored address must be unchanged, got {stored:?}"
                );
            }
            _ => {
                first = Some(association_id);
                first_record = Some(stored);
            }
        }
    }
}

/// Rule 2b: an address moves to the resource that last asked for it, and an instance holds
/// at most one address — the one it gives up stays allocated.
#[tokio::test]
async fn an_address_moves_and_the_one_it_displaces_stays_allocated() {
    let router = emulator();
    let (first_allocation, first_ip) = allocate(&router).await;
    let (second_allocation, second_ip) = allocate(&router).await;
    let (left_instance, left_private_ip) = launch(&router).await;
    let (right_instance, right_private_ip) = launch(&router).await;

    let case = "the address is first associated with the left instance";
    let first_association = associated(
        &router,
        case,
        &[
            p("AllocationId", &first_allocation),
            p("InstanceId", &left_instance),
        ],
    )
    .await;

    // Moving the SAME address to another instance: the left instance gives it up, the
    // right one reports it, and the move is a new association rather than the old one.
    let case = "the same address is then associated with the right instance";
    let moved_association = associated(
        &router,
        case,
        &[
            p("AllocationId", &first_allocation),
            p("InstanceId", &right_instance),
        ],
    )
    .await;
    assert_ne!(
        moved_association, first_association,
        "case {case:?}: a remap is a new association, so it must not reuse \
         {first_association}"
    );
    let stored = stored_address(&router, case, &first_allocation).await;
    let right_interface = primary_interface(&router, case, &right_instance).await;
    assert_eq!(
        stored,
        AddressRow {
            allocation_id: Some(first_allocation.clone()),
            public_ip: Some(first_ip.clone()),
            association_id: Some(moved_association.clone()),
            instance_id: Some(right_instance.clone()),
            private_ip_address: Some(right_private_ip.clone()),
            network_interface_id: Some(right_interface.clone()),
        },
        "case {case:?}: the address must now be reported on the right instance, \
         got {stored:?}"
    );
    let left_reported = instance_public_ip(&router, case, &left_instance).await;
    assert_eq!(
        left_reported, None,
        "case {case:?}: the left instance must give up the address, got {left_reported:?}"
    );
    let right_reported = instance_public_ip(&router, case, &right_instance).await;
    assert_eq!(
        right_reported,
        Some(first_ip.clone()),
        "case {case:?}: the right instance must report the address, got {right_reported:?}"
    );

    // A SECOND address on the same instance: "the existing address is disassociated from
    // the instance, but remains allocated to your account".
    let case = "a second address is associated with the instance that holds the first";
    let second_association = associated(
        &router,
        case,
        &[
            p("AllocationId", &second_allocation),
            p("InstanceId", &right_instance),
        ],
    )
    .await;
    let displaced = stored_address(&router, case, &first_allocation).await;
    assert_eq!(
        displaced,
        AddressRow {
            // Still the account's address, with its public address, and nothing else.
            allocation_id: Some(first_allocation.clone()),
            public_ip: Some(first_ip.clone()),
            association_id: None,
            instance_id: None,
            private_ip_address: None,
            network_interface_id: None,
        },
        "case {case:?}: the displaced address must stay allocated and unassociated, \
         got {displaced:?}"
    );
    let stored = stored_address(&router, case, &second_allocation).await;
    assert_eq!(
        stored,
        AddressRow {
            allocation_id: Some(second_allocation.clone()),
            public_ip: Some(second_ip.clone()),
            association_id: Some(second_association),
            instance_id: Some(right_instance.clone()),
            private_ip_address: Some(right_private_ip.clone()),
            network_interface_id: Some(right_interface),
        },
        "case {case:?}: the second address must be the one the instance holds, \
         got {stored:?}"
    );
    let right_reported = instance_public_ip(&router, case, &right_instance).await;
    assert_eq!(
        right_reported,
        Some(second_ip.clone()),
        "case {case:?}: the instance must report the address it now holds, \
         got {right_reported:?}"
    );
    // The left instance never got an address back.
    let left_reported = instance_public_ip(&router, case, &left_instance).await;
    assert_eq!(
        left_reported, None,
        "case {case:?}: the left instance must still report no public address, \
         got {left_reported:?}"
    );
    assert!(
        !left_private_ip.is_empty() && !right_private_ip.is_empty(),
        "both instances must have been given a primary private address, \
         got {left_private_ip:?} and {right_private_ip:?}"
    );
}

/// Rule 3: every refusal, with the code the real service answers, and nothing stored.
#[tokio::test]
async fn a_request_it_cannot_serve_is_refused_and_stores_nothing() {
    let router = emulator();
    let (free_allocation, free_ip) = allocate(&router).await;
    let (held_allocation, held_ip) = allocate(&router).await;
    let (holder_instance, _holder_private_ip) = launch(&router).await;
    let (other_instance, other_private_ip) = launch(&router).await;
    let (leaving_instance, _leaving_private_ip) = launch(&router).await;

    // One address is already associated, so the reassociation row below has something to
    // refuse, and one instance is on its way out.
    let held_association = associated(
        &router,
        "the address the reassociation row names",
        &[
            p("AllocationId", &held_allocation),
            p("InstanceId", &holder_instance),
        ],
    )
    .await;
    let (status, body) = api(
        &router,
        "TerminateInstances",
        &[p("InstanceId.1", &leaving_instance)],
    )
    .await;
    assert_eq!(
        status, 200,
        "TerminateInstances must succeed, got {status} with body {body}"
    );

    // Rows: `(case, parameters, status, error code)`. Each row differs along what the
    // refusal generalises over — which member is missing, which pair is given together,
    // which id form is named, which kind of resource is unknown.
    let rows: Vec<(&str, Params, u16, &str)> = vec![
        (
            "an allocation id of the long form the account holds no address for",
            vec![
                p("AllocationId", "eipalloc-0123456789abcdef0"),
                p("InstanceId", &other_instance),
            ],
            400,
            "InvalidAllocationID.NotFound",
        ),
        (
            "an allocation id of the short form the account holds no address for",
            vec![
                p("AllocationId", "eipalloc-0a1b2c3d"),
                p("InstanceId", &other_instance),
            ],
            400,
            "InvalidAllocationID.NotFound",
        ),
        (
            "an allocation id one digit short of the short form",
            vec![
                p("AllocationId", "eipalloc-0a1b2c3"),
                p("InstanceId", &other_instance),
            ],
            400,
            "InvalidAllocationID.Malformed",
        ),
        (
            "an allocation id one digit short of the long form",
            vec![
                p("AllocationId", "eipalloc-0123456789abcdef"),
                p("InstanceId", &other_instance),
            ],
            400,
            "InvalidAllocationID.Malformed",
        ),
        (
            "an allocation id with an upper-case hexadecimal digit",
            vec![
                p("AllocationId", "eipalloc-0A1B2C3D"),
                p("InstanceId", &other_instance),
            ],
            400,
            "InvalidAllocationID.Malformed",
        ),
        (
            "something that is not an allocation id at all",
            vec![
                p("AllocationId", "the-first-one"),
                p("InstanceId", &other_instance),
            ],
            400,
            "InvalidAllocationID.Malformed",
        ),
        (
            "no address named at all",
            vec![p("InstanceId", &other_instance)],
            400,
            "MissingParameter",
        ),
        (
            "the address named twice, by allocation id and by public address",
            vec![
                p("AllocationId", &free_allocation),
                p("PublicIp", "54.0.0.1"),
                p("InstanceId", &other_instance),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "a public address the account holds no allocation for",
            vec![
                p("PublicIp", "203.0.113.9"),
                p("InstanceId", &other_instance),
            ],
            400,
            "InvalidAddress.NotFound",
        ),
        (
            "a publicIp that is not an IPv4 address",
            vec![
                p("PublicIp", "ten.zero.zero.one"),
                p("InstanceId", &other_instance),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "no resource to associate the address with",
            vec![p("AllocationId", &free_allocation)],
            400,
            "MissingParameter",
        ),
        (
            "both an instance and a network interface",
            vec![
                p("AllocationId", &free_allocation),
                p("InstanceId", &other_instance),
                p("NetworkInterfaceId", "eni-0123456789abcdef0"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "an instance id the account holds no instance for",
            vec![
                p("AllocationId", &free_allocation),
                p("InstanceId", "i-0aaaaaaaaaaaaaaaa"),
            ],
            400,
            "InvalidInstanceID.NotFound",
        ),
        (
            "an instance id whose leading digit is above zero",
            vec![
                p("AllocationId", &free_allocation),
                p("InstanceId", "i-1234567890abcdef0"),
            ],
            400,
            "InvalidInstanceID.Malformed",
        ),
        (
            "a network interface id of the long form",
            vec![
                p("AllocationId", &free_allocation),
                p("NetworkInterfaceId", "eni-0123456789abcdef0"),
            ],
            400,
            "InvalidNetworkInterfaceID.NotFound",
        ),
        (
            "the all-zero network interface id, which is well formed",
            vec![
                p("AllocationId", &free_allocation),
                p("NetworkInterfaceId", "eni-00000000000000000"),
            ],
            400,
            "InvalidNetworkInterfaceID.NotFound",
        ),
        (
            "a network interface id of the short form",
            vec![
                p("AllocationId", &free_allocation),
                p("NetworkInterfaceId", "eni-0a1b2c3d"),
            ],
            400,
            "InvalidNetworkInterfaceID.NotFound",
        ),
        (
            "a network interface id one digit short of the short form",
            vec![
                p("AllocationId", &free_allocation),
                p("NetworkInterfaceId", "eni-0a1b2c3"),
            ],
            400,
            "InvalidNetworkInterfaceId.Malformed",
        ),
        (
            "an id of another resource kind in networkInterfaceId",
            vec![
                p("AllocationId", &free_allocation),
                p("NetworkInterfaceId", "subnet-0123456789abcdef0"),
            ],
            400,
            "InvalidNetworkInterfaceId.Malformed",
        ),
        (
            "a private address the instance does not have",
            vec![
                p("AllocationId", &free_allocation),
                p("InstanceId", &other_instance),
                p("PrivateIpAddress", "10.99.99.99"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "an address held elsewhere, with reassociation refused",
            vec![
                p("AllocationId", &held_allocation),
                p("InstanceId", &other_instance),
                p("AllowReassociation", "false"),
            ],
            400,
            "Resource.AlreadyAssociated",
        ),
        (
            "an instance that is on its way out",
            vec![
                p("AllocationId", &free_allocation),
                p("InstanceId", &leaving_instance),
            ],
            400,
            "IncorrectInstanceState",
        ),
        (
            "a request that would have succeeded, under DryRun",
            vec![
                p("AllocationId", &free_allocation),
                p("InstanceId", &other_instance),
                p("DryRun", "true"),
            ],
            412,
            "DryRunOperation",
        ),
    ];

    // The public address the instance every refused row names holds BEFORE any of them:
    // the one its own launch assigned it out of Amazon's pool, the default subnet it was
    // launched into mapping one on launch (`Subnet$MapPublicIpOnLaunch`). No refusal may
    // change it.
    let before_refusals = instance_public_ip(&router, "before the refusals", &other_instance).await;

    for (case, params, expected_status, expected_code) in &rows {
        let (status, body) = associate(&router, params).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let code = ec2_query::error_code(&body);
        assert_eq!(
            code, *expected_code,
            "case {case:?}: expected the error code {expected_code}, got {code:?} \
             with body {body}"
        );
    }

    // Every refusal above left both stores exactly as the setup had them.
    let case = "after every refusal";
    let free = stored_address(&router, case, &free_allocation).await;
    assert_eq!(
        free.association_id, None,
        "case {case:?}: the free address must still be unassociated, got {free:?}"
    );
    assert_eq!(
        free.instance_id, None,
        "case {case:?}: the free address must still name no instance, got {free:?}"
    );
    let held = stored_address(&router, case, &held_allocation).await;
    assert_eq!(
        held.association_id,
        Some(held_association),
        "case {case:?}: the held address must keep the association it had, got {held:?}"
    );
    assert_eq!(
        held.instance_id,
        Some(holder_instance.clone()),
        "case {case:?}: the held address must still be on its own instance, got {held:?}"
    );
    // No refused request associated anything: the instance every one of them named holds
    // exactly the address it held before, which is neither Elastic IP.
    let reported = instance_public_ip(&router, case, &other_instance).await;
    assert_eq!(
        reported, before_refusals,
        "case {case:?}: the instance every refused request named must hold the address it \
         held before them ({before_refusals:?}), got {reported:?}"
    );
    assert!(
        reported.as_deref() != Some(free_ip.as_str())
            && reported.as_deref() != Some(held_ip.as_str()),
        "case {case:?}: the instance every refused request named must hold neither Elastic \
         IP ({free_ip:?} or {held_ip:?}), got {reported:?}"
    );
    assert!(
        !other_private_ip.is_empty(),
        "the instance every refused request named must have a private address, \
         got {other_private_ip:?}"
    );
}
