// SPDX-License-Identifier: CC0-1.0

//! The bounds this service enforces: the quotas on how much one caller may store, the
//! documented per-member size constraints the model does not express, and the checks that
//! refuse a request reaching past either.

use super::*;

/// The documented maximum length of a tag key, in characters: "Constraints: Maximum
/// length of 128 characters" on `Tag$Key`
/// (`smithy_query query="doc" shape="Tag" member="Key"`) — a constraint the modelled
/// `length 0..10000000` does not express, so it is checked here.
pub const MAX_TAG_KEY_CHARS: usize = 128;
/// The documented maximum length of a tag value, in characters: "Constraints: Maximum
/// length of 256 characters" on `Tag$Value`
/// (`smithy_query query="doc" shape="Tag" member="Value"`).
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// How many tags one resource may carry: AWS's published tagging limit of 50 tags per
/// resource, which applies to every taggable resource. Asking for more is refused with
/// the operation's modelled `ServiceQuotaExceededException` (HTTP 402) and stores
/// nothing.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// How many global networks one account may hold here, and how many sites one global
/// network may hold.
///
/// Network Manager publishes an account-level quota for both, adjustable per account, and
/// the model carries neither number, so these two are CHOSEN: their purpose is that a
/// caller cannot grow the emulator's store without bound. Reaching one is answered with
/// the operation's modelled `ServiceQuotaExceededException` (HTTP 402) and stores nothing,
/// exactly as exceeding a real quota does.
pub const MAX_GLOBAL_NETWORKS: usize = 100;
/// See [`MAX_GLOBAL_NETWORKS`].
pub const MAX_SITES_PER_GLOBAL_NETWORK: usize = 100;
/// How many links one site may hold. CHOSEN for the same reason as
/// [`MAX_SITES_PER_GLOBAL_NETWORK`]: the model carries no number and the published quota
/// is per-account adjustable, so this one only bounds the store.
pub const MAX_LINKS_PER_SITE: usize = 100;
/// How many devices one global network may hold. CHOSEN for the same reason as
/// [`MAX_SITES_PER_GLOBAL_NETWORK`]: the model carries no number and the published quota
/// is per-account adjustable, so this one only bounds the store.
pub const MAX_DEVICES_PER_GLOBAL_NETWORK: usize = 100;
/// How many connections one global network may hold. CHOSEN for the same reason as
/// [`MAX_SITES_PER_GLOBAL_NETWORK`]: the model carries no number and the published quota
/// is per-account adjustable, so this one only bounds the store.
pub const MAX_CONNECTIONS_PER_GLOBAL_NETWORK: usize = 100;

/// The documented maximum length, in characters, of a device's `Vendor`, `Model` and
/// `SerialNumber`: "Constraints: Maximum length of 128 characters" on each of the three
/// (`smithy_query query="doc" shape="CreateDeviceRequest" member="Vendor"`, the same
/// sentence for `Model` and `SerialNumber`, and for the same three members of
/// `UpdateDeviceRequest`) — a bound the modelled `ConstrainedString` (len 0..256) does not
/// express, so it is checked by [`device_descriptor_problem`] instead.
pub const MAX_DEVICE_DESCRIPTOR_CHARS: usize = 128;

/// Check one free-form device descriptor against the constraint
/// [`MAX_DEVICE_DESCRIPTOR_CHARS`] documents.
///
/// Answers `None` when `value` satisfies it, otherwise the member-level reason the refusal
/// reports. Shared by every operation that takes those descriptors, so a value a create
/// refuses an update refuses too — the same seam [`link_descriptor_problem`] gives a link.
pub fn device_descriptor_problem(value: &str) -> Option<String> {
    let max = MAX_DEVICE_DESCRIPTOR_CHARS;
    let length = value.chars().count();
    (length > max).then(|| format!("must be at most {max} characters, but is {length}"))
}

/// The documented maximum length, in characters, of a link's `Type` and `Provider`, and
/// the characters the documentation forbids in them: "Constraints: Maximum length of 128
/// characters. Cannot include the following characters: | \ ^" (`smithy_query query="doc"
/// shape="CreateLinkRequest" member="Provider"`, the same sentence for `Type` and for
/// both members of `UpdateLinkRequest`) — neither of which the modelled
/// `ConstrainedString` (len 0..256, pattern `^[\s\S]*$`) expresses, so they are checked
/// by [`link_descriptor_problem`] instead.
pub const MAX_LINK_DESCRIPTOR_CHARS: usize = 128;
/// See [`MAX_LINK_DESCRIPTOR_CHARS`].
pub const FORBIDDEN_LINK_DESCRIPTOR_CHARS: [char; 3] = ['|', '\\', '^'];

/// Check one free-form link descriptor against the two constraints
/// [`MAX_LINK_DESCRIPTOR_CHARS`] documents.
///
/// Answers `None` when `value` satisfies both, otherwise the member-level reason the
/// refusal reports. Shared by every operation that takes those descriptors, so a value a
/// create refuses an update refuses too.
pub fn link_descriptor_problem(value: &str) -> Option<String> {
    let max = MAX_LINK_DESCRIPTOR_CHARS;
    let length = value.chars().count();
    if length > max {
        return Some(format!("must be at most {max} characters, but is {length}"));
    }
    value
        .chars()
        .find(|candidate| FORBIDDEN_LINK_DESCRIPTOR_CHARS.contains(candidate))
        .map(|found| {
            format!("must not include any of the characters | \\ ^, but includes {found:?}")
        })
}

/// How many core networks one global network may hold.
///
/// Unlike the three bounds above this one is the SERVICE's number, not a chosen bound:
/// Cloud WAN gives a global network a single core network ("Each global network can have
/// one core network", AWS Cloud WAN documentation), so a second create against the same
/// global network is answered with the operation's modelled
/// `ServiceQuotaExceededException` (HTTP 402) and stores nothing.
pub const MAX_CORE_NETWORKS_PER_GLOBAL_NETWORK: usize = 1;

/// How many attachments one core network may hold here.
///
/// CHOSEN, like [`MAX_SITES_PER_GLOBAL_NETWORK`]: Cloud WAN publishes a per-account
/// adjustable quota for attachments per core network and the model carries no number, so
/// this bound exists only so that a repeated create cannot grow the store without end.
/// No attachment create models a `ServiceQuotaExceededException` (`smithy_query
/// query="operation"
/// operation="CreateVpcAttachment,CreateConnectAttachment,CreateSiteToSiteVpnAttachment"`),
/// so reaching it is refused with the modelled `ValidationException` (HTTP 400) carrying
/// [`VALIDATION_REASON_OTHER`] — the request is invalid against stored state, no single
/// member is at fault — and stores nothing.
pub const MAX_ATTACHMENTS_PER_CORE_NETWORK: usize = 100;

/// How many peerings one core network may hold here.
///
/// CHOSEN, like [`MAX_ATTACHMENTS_PER_CORE_NETWORK`], and refused the same way:
/// `CreateTransitGatewayPeering` models no `ServiceQuotaExceededException` (`smithy_query
/// query="operation" operation="CreateTransitGatewayPeering"`), so a create past this cap
/// answers the modelled `ValidationException` (HTTP 400) carrying
/// [`VALIDATION_REASON_OTHER`] and stores nothing.
pub const MAX_PEERINGS_PER_CORE_NETWORK: usize = 100;

/// How many managed prefix lists one core network may be associated with here.
///
/// CHOSEN, like [`MAX_ATTACHMENTS_PER_CORE_NETWORK`]: Cloud WAN publishes no number for it
/// and the model carries none, so this bound exists only so that repeated creates cannot
/// grow the store without end. `CreateCoreNetworkPrefixListAssociation` DOES model a
/// `ServiceQuotaExceededException` (`smithy_query query="operation"
/// operation="CreateCoreNetworkPrefixListAssociation"`), so reaching it is answered with
/// that modelled error (HTTP 402) and stores nothing.
pub const MAX_PREFIX_LIST_ASSOCIATIONS_PER_CORE_NETWORK: usize = 100;

/// How many customer gateways one global network may be associated with here.
///
/// CHOSEN, like [`MAX_PREFIX_LIST_ASSOCIATIONS_PER_CORE_NETWORK`] and for the same reason:
/// no published number bounds it and the model carries none, so this bound exists only so
/// that repeated associates — each naming a customer gateway of Amazon VPC, of which this
/// service holds no inventory — cannot grow the store without end.
/// `AssociateCustomerGateway` DOES model a `ServiceQuotaExceededException` (`smithy_query
/// query="operation" operation="AssociateCustomerGateway"`), so reaching it is answered
/// with that modelled error (HTTP 402) and stores nothing.
pub const MAX_CUSTOMER_GATEWAY_ASSOCIATIONS_PER_GLOBAL_NETWORK: usize = 100;

/// How many transit gateways one global network may hold a registration for here.
///
/// CHOSEN, like [`MAX_CUSTOMER_GATEWAY_ASSOCIATIONS_PER_GLOBAL_NETWORK`] and for the same
/// reason: no published number bounds it and the model carries none, so this bound exists
/// only so that repeated registers — each naming a transit gateway of Amazon VPC, of which
/// this service holds no inventory — cannot grow the store without end.
/// `RegisterTransitGateway` models no `ServiceQuotaExceededException` (`smithy_query
/// query="operation" operation="RegisterTransitGateway"`), so reaching this bound is
/// refused with the modelled `ValidationException` (HTTP 400) naming the member that asked,
/// and stores nothing.
pub const MAX_TRANSIT_GATEWAY_REGISTRATIONS_PER_GLOBAL_NETWORK: usize = 100;

/// How many transit gateway Connect peers one global network may hold an association for
/// here.
///
/// CHOSEN, like [`MAX_CUSTOMER_GATEWAY_ASSOCIATIONS_PER_GLOBAL_NETWORK`] and for the same
/// reason: no published number bounds it and the model carries none, so this bound exists
/// only so that repeated associates — each naming a Connect peer of a transit gateway of
/// Amazon VPC, of which this service holds no inventory — cannot grow the store without
/// end. `AssociateTransitGatewayConnectPeer` DOES model a `ServiceQuotaExceededException`
/// (`smithy_query query="operation" operation="AssociateTransitGatewayConnectPeer"`), so
/// reaching it is answered with that modelled error (HTTP 402) and stores nothing.
pub const MAX_TRANSIT_GATEWAY_CONNECT_PEER_ASSOCIATIONS_PER_GLOBAL_NETWORK: usize = 100;

/// How many Connect peers one Connect attachment may hold here, and how many inside CIDR
/// blocks one Connect peer may peer over.
///
/// Both are CHOSEN, like [`MAX_ATTACHMENTS_PER_CORE_NETWORK`] and for the same reason: the
/// model carries neither number — `ConstrainedStringList`, which `InsideCidrBlocks`
/// targets, constrains its member string only (`smithy_query shape=ConstrainedStringList`)
/// — and `CreateConnectPeer` models no `ServiceQuotaExceededException` (`smithy_query
/// query="operation" operation="CreateConnectPeer"`), so their only purpose is that a
/// repeated create cannot grow the store, or one peer's list, without end. Reaching either
/// is refused with the modelled `ValidationException` (HTTP 400) naming the member that
/// asked, and stores nothing.
pub const MAX_CONNECT_PEERS_PER_ATTACHMENT: usize = 100;
/// See [`MAX_CONNECT_PEERS_PER_ATTACHMENT`].
pub const MAX_INSIDE_CIDR_BLOCKS_PER_CONNECT_PEER: usize = 10;

/// Check the `InsideCidrBlocks` list a Connect peer create carries.
///
/// The LENGTH is judged first, against [`MAX_INSIDE_CIDR_BLOCKS_PER_CONNECT_PEER`], so no
/// per-entry work is spent on a list already too long to be accepted. Then no block may
/// repeat: a peer peers over a block once or not at all, so a repeated entry describes
/// nothing further. What the blocks CONTAIN is not judged here — "The inside IP addresses
/// used for BGP peering" (`smithy_query query="doc" shape="CreateConnectPeerRequest"
/// member="InsideCidrBlocks"`) is an opaque `ConstrainedString` in the model — so the
/// stored list is the one the request carried.
pub fn check_inside_cidr_blocks(
    member: &str,
    requested: &[String],
) -> Result<(), ValidationException> {
    if requested.len() > MAX_INSIDE_CIDR_BLOCKS_PER_CONNECT_PEER {
        return Err(field_rejection(
            member,
            VALIDATION_REASON_OTHER,
            format!(
                "a Connect peer peers over at most {MAX_INSIDE_CIDR_BLOCKS_PER_CONNECT_PEER} inside CIDR blocks, but {} were named",
                requested.len()
            ),
        ));
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for block in requested {
        if !seen.insert(block.as_str()) {
            return Err(field_rejection(
                member,
                VALIDATION_REASON_OTHER,
                format!("inside CIDR block {block} is named more than once"),
            ));
        }
    }
    Ok(())
}
