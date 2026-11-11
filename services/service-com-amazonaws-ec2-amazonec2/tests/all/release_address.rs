// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ReleaseAddress` and the `DescribeAddresses` read it is observed
//! through, driven over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=ReleaseAddress`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that the operation GIVES THE ONE ELASTIC IP ADDRESS THE REQUEST
//! NAMES BACK TO THE ADDRESS POOL — "after releasing an Elastic IP address, it is released
//! to the IP address pool" (`ReleaseAddress`) — so the allocation leaves this account's
//! address state and neither the allocation id nor the public address resolves to anything
//! afterwards, while a release the service refuses gives up nothing at all: an address that
//! still carries an association is in use ("you must disassociate the Elastic IP address
//! before you can release it. Otherwise, Amazon EC2 returns an error
//! (InvalidIPAddress.InUse)"), a network border group that is not the address's own names no
//! address of the account ("if you provide an incorrect network border group, you receive an
//! InvalidAddress.NotFound error" — `ReleaseAddressRequest$NetworkBorderGroup`), and a dry
//! run reports that the request would have succeeded without performing it.
//!
//! Every table row is a distinct input along what that generalises over — which member names
//! the address (the allocation id, or the deprecated `PublicIp`), whether the request also
//! names the border group the address is advertised from, whether the allocation carries
//! tags, whether the address is associated, and in what way the request is faulty — so the
//! one case a recording happens to contain is one row among many rather than the
//! specification.
//!
//! `AllocateAddress`'s own suite is [`super::elastic_ip_addresses`] and the association's is
//! [`super::elastic_ip_interface_associations`]; this file covers the RELEASE: what it takes
//! out of the account's state, what it reports, what it refuses, and what a dry run of it
//! leaves behind.

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{self, element, elements, error_code, error_message, member};
use super::wire::{param, Param};

/// The Region every call here is signed for. An Elastic IP address is a Region's resource,
/// and the network border group it is advertised from is named for that Region, so the
/// Region is read back off the allocation rather than spelled out by a row.
const REGION: &str = "us-west-2";

/// The image the one launch in this suite uses; `RunInstances` only needs one that resolves.
const AMI: &str = "ami-0123456789abcdef0";

/// EC2's answer to an authorized dry run, and the status it carries it on.
const DRY_RUN_CODE: &str = "DryRunOperation";
const DRY_RUN_STATUS: u16 = 412;
const DRY_RUN_MESSAGE: &str = "Request would have succeeded, but DryRun flag is set.";

/// An IPv4 address outside the block this emulator advertises Elastic IP addresses from, so
/// no allocation of the account can ever carry it: a documentation address (RFC 5737
/// TEST-NET-3), which is what a request naming an address of some other account looks like
/// from here.
const UNHELD_ADDRESS: &str = "203.0.113.17";

fn emulator() -> MultiServiceRouter {
    ec2_query::emulator().0
}

async fn api(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    ec2_query::call(router, REGION, action, params).await
}

/// One call that must answer HTTP 200, as its body.
async fn served(router: &MultiServiceRouter, case: &str, action: &str, params: &[Param]) -> String {
    let (status, body) = api(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One Elastic IP address the account holds: what an allocation reports about itself, which
/// is all a release needs to name it either documented way.
#[derive(Debug, Clone)]
struct Held {
    allocation_id: String,
    public_ip: String,
    /// The group the ALLOCATION reported, never a value this suite spells out.
    network_border_group: String,
}

/// Allocate one Elastic IP address, carrying `tags`, and answer what it reported.
async fn allocate(router: &MultiServiceRouter, case: &str, tags: &[(&str, &str)]) -> Held {
    let mut params = vec![param("Domain", "vpc")];
    if !tags.is_empty() {
        params.push(param("TagSpecification.1.ResourceType", "elastic-ip"));
        for (index, (key, value)) in tags.iter().enumerate() {
            params.push(param(
                &format!("TagSpecification.1.Tag.{}.Key", index + 1),
                key,
            ));
            params.push(param(
                &format!("TagSpecification.1.Tag.{}.Value", index + 1),
                value,
            ));
        }
    }
    let body = served(router, case, "AllocateAddress", &params).await;
    let reported = |name: &str| {
        element(&body, name).unwrap_or_else(|| {
            panic!("case {case:?}: AllocateAddress reported no <{name}>: {body}")
        })
    };
    Held {
        allocation_id: reported("allocationId"),
        public_ip: reported("publicIp"),
        network_border_group: reported("networkBorderGroup"),
    }
}

/// Whether an unnarrowed `DescribeAddresses` still reports `held`, asserting that the
/// allocation id and the public address are reported together or not at all.
async fn is_reported(router: &MultiServiceRouter, case: &str, held: &Held) -> bool {
    let body = served(router, case, "DescribeAddresses", &[]).await;
    let by_id = elements(&body, "allocationId").contains(&held.allocation_id);
    let by_address = elements(&body, "publicIp").contains(&held.public_ip);
    assert_eq!(
        by_id, by_address,
        "case {case:?}: the allocation {} and its address {} must be reported together, \
         got {by_id} and {by_address} in {body}",
        held.allocation_id, held.public_ip
    );
    by_id
}

/// Assert that a narrowed `DescribeAddresses` refuses to resolve `named`, with `code` and
/// `message` — the refusal a read of a released allocation answers.
async fn assert_unresolvable(
    router: &MultiServiceRouter,
    case: &str,
    member_name: &str,
    named: &str,
    code: &str,
    message: &str,
) {
    let params = [param(&format!("{member_name}.1"), named)];
    let (status, body) = api(router, "DescribeAddresses", &params).await;
    assert_eq!(
        status, 400,
        "case {case:?}: DescribeAddresses narrowed to {named} must be refused, got {status} \
         with body {body}"
    );
    let answered = error_code(&body);
    assert_eq!(
        answered, code,
        "case {case:?}: expected {code} for {named}, got {answered:?} in body {body}"
    );
    let answered = error_message(&body);
    assert_eq!(
        answered, message,
        "case {case:?}: expected the message {message:?} for {named}, got {answered:?}"
    );
}

/// The refusal one call answered: the status, the code and the message, read the one way
/// every row of this suite compares them.
async fn refusal(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[Param],
) -> (u16, String, String) {
    let (status, body) = api(router, action, params).await;
    assert_ne!(
        status, 200,
        "case {case:?}: {action} must be refused, got HTTP 200 with body {body}"
    );
    (status, error_code(&body), error_message(&body))
}

// ---------------------------------------------------------------------------
// The release itself
// ---------------------------------------------------------------------------

/// How a release names the address, and what the allocation carries while it is named that
/// way. "The allocation ID. This parameter is required." (`ReleaseAddressRequest$AllocationId`)
/// and "Deprecated." (`ReleaseAddressRequest$PublicIp`) are the two ways one address is
/// named, and the border group is part of naming it, so the rows vary which member carries
/// the name, whether the group is named beside it, and what the record holds.
struct ReleaseRow {
    case: &'static str,
    by_public_ip: bool,
    with_border_group: bool,
    tags: &'static [(&'static str, &'static str)],
}

const RELEASE_ROWS: &[ReleaseRow] = &[
    ReleaseRow {
        case: "named by its allocation id",
        by_public_ip: false,
        with_border_group: false,
        tags: &[],
    },
    ReleaseRow {
        case: "named by its public address, the deprecated spelling",
        by_public_ip: true,
        with_border_group: false,
        tags: &[],
    },
    ReleaseRow {
        case: "named by its allocation id and its own border group",
        by_public_ip: false,
        with_border_group: true,
        tags: &[],
    },
    ReleaseRow {
        case: "named by its public address and its own border group",
        by_public_ip: true,
        with_border_group: true,
        tags: &[],
    },
    ReleaseRow {
        case: "a tagged allocation named by its allocation id",
        by_public_ip: false,
        with_border_group: false,
        tags: &[("Name", "frontend"), ("owner", "team-a")],
    },
];

#[tokio::test]
async fn releasing_an_address_gives_the_allocation_back_to_the_pool() {
    for row in RELEASE_ROWS {
        let case = row.case;
        let router = emulator();
        let held = allocate(&router, case, row.tags).await;
        assert!(
            is_reported(&router, case, &held).await,
            "case {case:?}: the allocation must be held before it is released"
        );

        let mut params = if row.by_public_ip {
            vec![param("PublicIp", &held.public_ip)]
        } else {
            vec![param("AllocationId", &held.allocation_id)]
        };
        if row.with_border_group {
            params.push(param("NetworkBorderGroup", &held.network_border_group));
        }
        let body = served(&router, case, "ReleaseAddress", &params).await;

        // The envelope reports the release and nothing else: one `<return>true</return>`
        // beside the one `<requestId>` every ec2Query success carries.
        let flags = ec2_query::envelope_children(case, &body, "ReleaseAddress", "return");
        assert_eq!(
            flags,
            vec!["true".to_string()],
            "case {case:?}: expected one <return>true</return>, got {flags:?} in {body}"
        );
        let request_ids = ec2_query::envelope_children(case, &body, "ReleaseAddress", "requestId");
        assert_eq!(
            request_ids.len(),
            1,
            "case {case:?}: expected one <requestId>, got {request_ids:?} in {body}"
        );
        assert!(
            request_ids[0]
                .chars()
                .all(|c| c.is_ascii_hexdigit() || c == '-')
                && !request_ids[0].is_empty(),
            "case {case:?}: expected a request id, got {:?}",
            request_ids[0]
        );

        // The allocation has left the account's address state, so neither name resolves.
        assert!(
            !is_reported(&router, case, &held).await,
            "case {case:?}: {} must be gone once it is released",
            held.allocation_id
        );
        assert_unresolvable(
            &router,
            case,
            "AllocationId",
            &held.allocation_id,
            "InvalidAllocationID.NotFound",
            &format!(
                "The allocation ID '{}' does not exist",
                held.allocation_id
            ),
        )
        .await;
        assert_unresolvable(
            &router,
            case,
            "PublicIp",
            &held.public_ip,
            "InvalidAddress.NotFound",
            &format!(
                "The Elastic IP address '{}' does not exist or is not available.",
                held.public_ip
            ),
        )
        .await;

        // And a second release of the same address is refused rather than answered as a
        // no-op: a caller that releases twice has to be able to tell the release it
        // performed from one it did not.
        let (status, code, message) = refusal(&router, case, "ReleaseAddress", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second release must answer HTTP 400, got {status}"
        );
        let (expected_code, expected_message) = if row.by_public_ip {
            (
                "InvalidAddress.NotFound",
                format!(
                    "The Elastic IP address '{}' does not exist or is not available.",
                    held.public_ip
                ),
            )
        } else {
            (
                "InvalidAllocationID.NotFound",
                format!(
                    "The allocation ID '{}' does not exist",
                    held.allocation_id
                ),
            )
        };
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected {expected_code} from the second release, got {code:?}"
        );
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected the message {expected_message:?} from the second \
             release, got {message:?}"
        );
    }
}

/// Releasing one of several held allocations gives up exactly that one: the rule is about
/// the address the request names, so the rows vary which of three held allocations is
/// released and how it is named.
#[tokio::test]
async fn a_release_gives_up_only_the_address_the_request_names() {
    for (case, position, by_public_ip) in [
        ("the first of three, by allocation id", 0usize, false),
        ("the middle of three, by public address", 1usize, true),
        ("the last of three, by allocation id", 2usize, false),
    ] {
        let router = emulator();
        let mut held = Vec::new();
        for _ in 0..3 {
            held.push(allocate(&router, case, &[]).await);
        }
        let released = held.remove(position);

        let params = if by_public_ip {
            vec![param("PublicIp", &released.public_ip)]
        } else {
            vec![param("AllocationId", &released.allocation_id)]
        };
        served(&router, case, "ReleaseAddress", &params).await;

        assert!(
            !is_reported(&router, case, &released).await,
            "case {case:?}: the released {} must be gone",
            released.allocation_id
        );
        for kept in &held {
            assert!(
                is_reported(&router, case, kept).await,
                "case {case:?}: {} was not released and must still be held",
                kept.allocation_id
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The releases the service refuses, which release nothing
// ---------------------------------------------------------------------------

/// What makes a release faulty: each row is one fault, resolved against the one allocation
/// the account holds so the refusal can name it.
#[derive(Debug, Clone, Copy)]
enum Fault {
    /// Both members name the one address.
    BothMembers,
    /// Nothing names an address.
    NoMember,
    /// A value that is no allocation id at all.
    MalformedAllocationId(&'static str),
    /// A well-formed allocation id the account holds no address under.
    UnknownAllocationId(&'static str),
    /// A `PublicIp` that is not an IPv4 address.
    MalformedPublicIp(&'static str),
    /// An IPv4 address the account holds no allocation for.
    UnheldPublicIp(&'static str),
    /// A network border group that is not the address's own.
    ForeignBorderGroup(&'static str),
    /// A release that would have succeeded, asked as a dry run.
    AuthorizedDryRun,
}

/// The form fields one fault sends, against the held allocation.
fn fields(fault: Fault, held: &Held) -> Vec<Param> {
    match fault {
        Fault::BothMembers => vec![
            param("AllocationId", &held.allocation_id),
            param("PublicIp", &held.public_ip),
        ],
        Fault::NoMember => Vec::new(),
        Fault::MalformedAllocationId(value) | Fault::UnknownAllocationId(value) => {
            vec![param("AllocationId", value)]
        }
        Fault::MalformedPublicIp(value) | Fault::UnheldPublicIp(value) => {
            vec![param("PublicIp", value)]
        }
        Fault::ForeignBorderGroup(group) => vec![
            param("AllocationId", &held.allocation_id),
            param("NetworkBorderGroup", group),
        ],
        Fault::AuthorizedDryRun => vec![
            param("AllocationId", &held.allocation_id),
            param("DryRun", "true"),
        ],
    }
}

/// The status, code and message one fault must be refused with.
fn expected(fault: Fault, held: &Held) -> (u16, &'static str, String) {
    match fault {
        // The two refusals about the NAMING itself carry the sentence this service words
        // them with today, which is what the whole crate answers for these two classes
        // (`src/scaffolded.rs`'s `invalid_parameter_combination` and `missing_parameter`
        // callers): the row pins the class, the status and the code, and follows that one
        // wording rather than inventing a second one here.
        Fault::BothMembers => (
            400,
            "InvalidParameterCombination",
            "An Elastic IP address is named once, so allocationId and publicIp may not be \
             given together"
                .to_string(),
        ),
        Fault::NoMember => (
            400,
            "MissingParameter",
            "The request must contain the parameter allocationId".to_string(),
        ),
        Fault::MalformedAllocationId(value) => (
            400,
            "InvalidAllocationID.Malformed",
            format!("Invalid id: \"{value}\""),
        ),
        Fault::UnknownAllocationId(value) => (
            400,
            "InvalidAllocationID.NotFound",
            format!("The allocation ID '{value}' does not exist"),
        ),
        Fault::MalformedPublicIp(value) => (
            400,
            "InvalidParameterValue",
            format!("Invalid value '{value}' for publicIp. It must be an IPv4 address"),
        ),
        Fault::UnheldPublicIp(value) => (
            400,
            "InvalidAddress.NotFound",
            format!("The Elastic IP address '{value}' does not exist or is not available."),
        ),
        // The group is part of naming the address, so a group that is not the address's own
        // names no address of the account — and the refusal names the address the caller
        // holds, which is the one the allocation reported.
        Fault::ForeignBorderGroup(_) => (
            400,
            "InvalidAddress.NotFound",
            format!(
                "The Elastic IP address '{}' does not exist or is not available.",
                held.public_ip
            ),
        ),
        Fault::AuthorizedDryRun => (DRY_RUN_STATUS, DRY_RUN_CODE, DRY_RUN_MESSAGE.to_string()),
    }
}

const FAULT_ROWS: &[(&str, Fault)] = &[
    (
        "both allocationId and publicIp name the address",
        Fault::BothMembers,
    ),
    ("no member names an address at all", Fault::NoMember),
    (
        "an allocation id of a form no id has",
        Fault::MalformedAllocationId("eipalloc-not-an-id"),
    ),
    (
        "an allocation id whose hex digits are too few",
        Fault::MalformedAllocationId("eipalloc-0123456"),
    ),
    (
        "a well-formed long allocation id the account does not hold",
        Fault::UnknownAllocationId("eipalloc-0f1e2d3c4b5a69788"),
    ),
    (
        "a well-formed short allocation id the account does not hold",
        Fault::UnknownAllocationId("eipalloc-1a2b3c4d"),
    ),
    (
        "a publicIp that is not an address",
        Fault::MalformedPublicIp("not-an-address"),
    ),
    (
        "a publicIp with an octet out of range",
        Fault::MalformedPublicIp("54.300.1.1"),
    ),
    (
        "an IPv4 address the account holds no allocation for",
        Fault::UnheldPublicIp(UNHELD_ADDRESS),
    ),
    (
        "another Region's network border group",
        Fault::ForeignBorderGroup("eu-central-1"),
    ),
    (
        "a value that is no network border group",
        Fault::ForeignBorderGroup("not-a-border-group"),
    ),
    (
        "a dry run of a release that would have succeeded",
        Fault::AuthorizedDryRun,
    ),
];

#[tokio::test]
async fn a_release_the_service_refuses_gives_up_nothing() {
    let router = emulator();
    let held = allocate(&router, "the one allocation every fault is asked against", &[]).await;

    for (case, fault) in FAULT_ROWS {
        let params = fields(*fault, &held);
        let (expected_status, expected_code, expected_message) = expected(*fault, &held);
        let (status, code, message) = refusal(&router, case, "ReleaseAddress", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status}"
        );
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected {expected_code}, got {code:?}"
        );
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected the message {expected_message:?}, got {message:?}"
        );
        // Whatever the fault, the allocation the account holds is still held.
        assert!(
            is_reported(&router, case, &held).await,
            "case {case:?}: a refused release must leave {} allocated",
            held.allocation_id
        );
    }

    // And the allocation the refusals left alone can still be released.
    served(
        &router,
        "after every refusal",
        "ReleaseAddress",
        &[param("AllocationId", &held.allocation_id)],
    )
    .await;
    assert!(
        !is_reported(&router, "after every refusal", &held).await,
        "the allocation must be gone once it is really released"
    );
}

// ---------------------------------------------------------------------------
// An address that is still in use
// ---------------------------------------------------------------------------

/// "[Nondefault VPC] You must disassociate the Elastic IP address before you can release it.
/// Otherwise, Amazon EC2 returns an error (InvalidIPAddress.InUse)" (`ReleaseAddress`): the
/// rows vary which member names the address in the release that is refused, because the
/// association, not the naming, is what makes the address unreleasable.
#[tokio::test]
async fn an_associated_address_cannot_be_released_until_it_is_disassociated() {
    for (case, by_public_ip) in [
        ("an associated address named by its allocation id", false),
        ("an associated address named by its public address", true),
    ] {
        let router = emulator();
        let held = allocate(&router, case, &[]).await;

        let launched = served(
            &router,
            case,
            "RunInstances",
            &[
                param("ImageId", AMI),
                param("MinCount", "1"),
                param("MaxCount", "1"),
            ],
        )
        .await;
        let instance_id = element(&launched, "instanceId")
            .unwrap_or_else(|| panic!("case {case:?}: RunInstances reported no instance"));

        let associated = served(
            &router,
            case,
            "AssociateAddress",
            &[
                param("AllocationId", &held.allocation_id),
                param("InstanceId", &instance_id),
            ],
        )
        .await;
        let association_id = member(&associated, "associationId")
            .unwrap_or_else(|| panic!("case {case:?}: AssociateAddress reported no association"));

        let params = if by_public_ip {
            vec![param("PublicIp", &held.public_ip)]
        } else {
            vec![param("AllocationId", &held.allocation_id)]
        };
        let (status, code, message) = refusal(&router, case, "ReleaseAddress", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: releasing an associated address must answer HTTP 400, got {status}"
        );
        assert_eq!(
            code, "InvalidIPAddress.InUse",
            "case {case:?}: expected InvalidIPAddress.InUse, got {code:?}"
        );
        assert_eq!(
            message,
            format!("Address {} is in use.", held.public_ip),
            "case {case:?}: the refusal must name the address that is in use, got {message:?}"
        );
        assert!(
            is_reported(&router, case, &held).await,
            "case {case:?}: the refused release must leave {} allocated",
            held.allocation_id
        );

        // Ending the association is what makes the address releasable.
        served(
            &router,
            case,
            "DisassociateAddress",
            &[param("AssociationId", &association_id)],
        )
        .await;
        served(&router, case, "ReleaseAddress", &params).await;
        assert!(
            !is_reported(&router, case, &held).await,
            "case {case:?}: the disassociated address must be released"
        );
    }
}
