// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-backupgateway-backuponpremises-v20210101 service.
//!
//! The resource this store owns is the **gateway**: `CreateGateway` mints one and
//! persists it here, and every other gateway operation resolves it out of this map by
//! ARN. Nothing is kept per request and nothing is recomputed from the request body —
//! the ARN a create returns is read back out of [`StoredGateway`] after the insert, so
//! a response can only describe a record that is really in the store.
//!
//! The `ActivationKey` a create carries is REDEEMED against the activation registry this
//! store owns ([`PendingActivation`]): the activation handshake a deployed appliance makes
//! issues the key ([`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::issue_activation_key`],
//! reached over the wire through `crate::service`), and the create that activates that
//! appliance consumes its entry
//! ([`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::redeem_activation_key`]), so
//! a key is redeemable exactly once and a key nothing issued is redeemable never. The
//! redeemed key is kept ON the record it created ([`StoredGateway::activation_key`]),
//! because it identifies the appliance the gateway stands for.
//!
//! The record carries the members of the later slices too (hypervisor association,
//! maintenance window, bandwidth schedule), so those operations extend this one struct
//! instead of opening a second map beside it and splitting one gateway across two
//! copies. They reuse the MODELLED shapes
//! ([`MaintenanceStartTime`], [`BandwidthRateLimitInterval`]) rather than mirroring
//! them, so a member cannot drift between what is stored and what is serialised.
//!
//! The gateway's MAINTENANCE WINDOW ([`StoredGateway::maintenance_start_time`]) is part of
//! every gateway record from the moment the gateway is activated: the create gives it the
//! default window ([`default_maintenance_start_time`]), so the field is NOT optional and a
//! read of any gateway always reports one. `PutMaintenanceStartTime` is what CHANGES it
//! ([`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::set_maintenance_start_time`]),
//! and `GetGateway` reports whatever that field holds: a window is one resource, so a put
//! REPLACES it whole rather than merging into it, and every reader of the record agrees on
//! the window the last write left.
//!
//! The second resource this store owns is the **hypervisor** ([`StoredHypervisor`]),
//! keyed by its own ARN: `GetHypervisor` resolves it out of that map and reports the
//! record, `ListHypervisors` pages over that map in ARN order, and the state either one
//! reports is derived from the record and the clock
//! ([`StoredHypervisor::state_as_of`]) so a read never moves it. `UpdateHypervisor` is
//! what changes its metadata in place
//! ([`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::update_hypervisor`]),
//! applying only the members the request carried, so every later read reports the record
//! the last write left and no second copy of a hypervisor exists to drift from it.
//! The ARN minted for a record is its ONLY identity: `Host`, `Name` and the rest are
//! metadata the record carries, so any number of records may hold the same host (one
//! appliance re-registered, or configured several times over) and no import or update is
//! refused because another record already carries the value it asks for.
//!
//! The third resource this store owns is the **virtual machine**
//! ([`StoredVirtualMachine`]), keyed by its own ARN. A virtual machine is not minted by a
//! request — no operation of this model creates one; it is part of the INVENTORY of the
//! hypervisor it runs on, and the service event that materialises that inventory is a
//! METADATA SYNC of the hypervisor
//! ([`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::start_metadata_sync`]),
//! which records every machine the hypervisor reports through
//! [`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::record_virtual_machine`] —
//! the one way a record gets here, and only for a hypervisor this store holds. A read
//! reports a machine once the sync that discovered it has fallen due
//! ([`StoredVirtualMachine::discovered_as_of`]), so `ListVirtualMachines` lists nothing
//! for a hypervisor whose metadata has never been synced, and nothing more than its
//! previous inventory while a sync is still in flight. `GetVirtualMachine` resolves ONE of
//! those records by its own ARN and reports it in full
//! ([`StoredVirtualMachine::details`]), including the VMware tags the inventory attached
//! to it, which no listing carries. A machine is as taggable as the other two resources:
//! the tagging operations resolve their one `ResourceARN` member against every map this
//! store owns ([`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::resource_tags`]),
//! so the AWS tags a client writes on a machine ([`StoredVirtualMachine::tags`], which
//! are not the VMware tags its inventory reported) live on the record beside the rest of
//! it, and a resource a read of this service resolves is one the tagging operations
//! resolve too.
//!
//! WHICH machines an inventory holds is this emulator's own answer ([`synced_inventory`]):
//! it stands in for the on-premises hypervisor as much as for the service, and has no
//! VMware host to enumerate, so a sync reports a bounded set of machines derived from the
//! WHOLE identity of the hypervisor record — the complete ARN it is keyed under, every
//! segment of it, and not the resource-id segment alone, which is minted per region and so
//! repeats across regions. Every sync of one record reports the same machines — which is
//! what makes a re-sync idempotent — and no two records share one.
//!
//! Every listing pages through one helper ([`page_of_where`], handing back a [`Page`]),
//! so a page boundary and the token that encodes it mean the same thing whichever
//! resource is being listed, filtered ([`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::page_of_virtual_machines`])
//! or not. A token identifies the whole QUERY it resumes and not merely where the last
//! page stopped ([`encode_page_token`]): the boundary, plus the filters the listing
//! applied beside its page controls, so a page cut from one query is never resumed by
//! another — whose matching records may lie entirely below that boundary, which would
//! report an empty page no client could tell from a last one. A `NextToken` is RESOLVED
//! against this store rather than parsed
//! ([`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::resume_after`]): only a
//! boundary this store really minted, for that resource type, in the region the listing
//! reports and under the filters the request presenting it carries, resumes a listing,
//! and every other string is refused with the one sentence
//! [`PAGE_TOKEN_INVALID_MESSAGE`] states — a boundary of
//! the right shape that nothing was ever stored under would bound the range at a key the
//! whole listing sorts above, so accepting it would hand a client page one for ever.
//! Every resource of this service is REGIONAL — its region is a segment of the
//! ARN it is minted and keyed under — so every listing reports only the records of the
//! region the listing request itself was addressed to ([`arn_is_in_region`], applied
//! before the page is cut), whichever resource it lists: one store serves every region a
//! client addresses, and a listing is an account-and-region listing, never a listing of
//! every region at once. That scope is not the listings' alone: an operation that resolves
//! ONE ARN reaches only the records of the region ITS request was addressed to
//! ([`arn_is_in_request_region`]), so an ARN of another region names no resource that
//! endpoint holds and is refused as the missing resource it is, with nothing read and
//! nothing written — a region never reports, writes or deletes a record another region's
//! listing owns.
//!
//! A hypervisor also carries its PROPERTY MAPPINGS ([`StoredPropertyMappings`]) — the
//! VMware-to-Amazon tag mappings and the IAM role they are applied with. They live on
//! the hypervisor record rather than in a map beside it, because that is the resource
//! they belong to: `GetHypervisorPropertyMappings` reports the mappings of the record its
//! `ServerArn` resolves to, and a hypervisor that carries none has no mappings resource
//! to report ([`MissingResource::PropertyMappings`]) and no metadata sync it can start
//! ([`MAPPINGS_NOT_CONFIGURED`]).
//! `PutHypervisorPropertyMappings` is what writes them
//! ([`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::set_property_mappings`]),
//! and only for a hypervisor this store HOLDS
//! ([`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::hypervisor`]) that a GATEWAY
//! is associated with
//! ([`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::gateway_for_hypervisor`]):
//! an ARN naming no record is refused as a missing hypervisor like any other
//! ([`MissingResource::Hypervisor`]), and for a record this store does hold, the appliance
//! that serves it is what applies the mappings, so with none associated the request is
//! refused ([`GATEWAY_OUT_OF_DATE`]) and nothing is written.
//!
//! Every resource a REQUEST of this API creates — a gateway and a hypervisor — carries the
//! AWS-assigned system tags beside the client's own: who created it
//! ([`CREATED_BY_TAG_KEY`], read off the signed request through [`request_created_by`])
//! and when ([`CREATED_DATE_TAG_KEY`]). They are stamped when the record is minted and a
//! tag read reports them AHEAD of the client's tags ([`reported_tags`]); they live in the
//! reserved key namespace ([`is_reserved_tag_key`]), so a client can neither write nor
//! remove one, and the client's own tags stay the map the tagging operations merge into
//! and remove from.
//!
//! The gateway's APPLIANCE SOFTWARE UPDATE ([`StoredGateway::software_update`]) is
//! written by `UpdateGatewaySoftwareNow` and by nothing else
//! ([`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::update_gateway_software_now`]):
//! the trigger is immediate but the install is not, so the record keeps the version the
//! update installs beside the instant the install falls due, and the version a read
//! reports is derived from that instant and the clock
//! ([`StoredGateway::software_version_as_of`]) the way a hypervisor's state is — so a read
//! never installs anything and every reader agrees on which version the appliance runs.
//!
//! A hypervisor also carries the METADATA SYNC last started on it
//! ([`StoredHypervisor::metadata_sync_started`]):
//! `StartVirtualMachinesMetadataSync` persists the instant it was requested at — for a
//! hypervisor whose property mappings are configured, which is the sync's PRECONDITION
//! ([`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::property_mappings_configured`])
//! — and the
//! `SyncMetadataStatus` and times a read reports are derived from that instant and the
//! clock ([`StoredHypervisor::latest_metadata_sync_status_as_of`]) the way the
//! hypervisor's own state is, so a read never moves a sync along and every reader agrees
//! on where it has got to. That one sync is also what records the hypervisor's
//! virtual-machine inventory, dated at the instant the sync falls due — so the machines a
//! listing reports and the `SUCCEEDED` a read of the hypervisor reports become true
//! together, and a sync started before the previous one fell due supersedes it on both
//! halves of that fact at once.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound;
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    BandwidthRateLimitInterval, Gateway, GatewayDetails, Hypervisor, HypervisorDetails,
    MaintenanceStartTime, Tag, VirtualMachine, VirtualMachineDetails,
};

/// The modelled mapping shape a stored property mapping is kept as, re-exported so the
/// record, the handlers and a test all name ONE type
/// (`smithy_query(query="shape",shape="VmwareToAwsTagMapping")`: `VmwareCategory`,
/// `VmwareTagName`, `AwsTagKey`, `AwsTagValue`, all required) instead of mirroring it.
pub use crate::ops::types::VmwareToAwsTagMapping;

/// The modelled VMware tag a stored virtual machine's tags are kept as, re-exported for
/// the same reason as [`VmwareToAwsTagMapping`]
/// (`smithy_query(query="shape",shape="VmwareTag")`: `VmwareCategory`, `VmwareTagName`,
/// `VmwareTagDescription`, none required).
pub use crate::ops::types::VmwareTag;

/// This service's ARN namespace (`smithy_query(query="service_info")`:
/// `awsService.arnNamespace`), and the second-to-last ARN segment every gateway ARN
/// carries.
pub const ARN_SERVICE: &str = "backup-gateway";

/// The ARN resource type of a gateway: `gateway/<id>`.
pub const GATEWAY_RESOURCE_TYPE: &str = "gateway";

/// The id prefix of a gateway this store minted: the `bgw-` of
/// `gateway/bgw-0000000000000000`.
///
/// One spelling for the mint and for the question "is this an ARN this store minted?"
/// ([`minted_resource_id`], [`minted_resource_sequence`]), so the two cannot drift.
const GATEWAY_ID_PREFIX: &str = "bgw-";

/// How many hex digits the body of a minted gateway id carries: the sixteen of
/// `bgw-0000000000000000`.
///
/// Beside the prefix, because the prefix and the width are the two halves of ONE
/// identifier form, and that form belongs to the resource kind — see
/// [`MintedResource::id_body_digits`].
const GATEWAY_ID_DIGITS: usize = 16;

/// The ARN resource type of a hypervisor: `hypervisor/<id>`.
///
/// The hypervisor is the second resource this service's ARNs name
/// (`smithy_query(query="shape",shape="GetHypervisorInput")`: `HypervisorArn` targets
/// `ServerArn`), and the tagging operations take any of this service's resources in the one `ResourceARN`
/// member — so they have to tell them apart. See [`MissingResource`].
pub const HYPERVISOR_RESOURCE_TYPE: &str = "hypervisor";

/// The id prefix of a hypervisor this store minted, for the same reason as
/// [`GATEWAY_ID_PREFIX`]: the `hype-` of `hypervisor/hype-1a2b3c4d`, which is the prefix
/// a hypervisor identity of this service carries on the wire.
const HYPERVISOR_ID_PREFIX: &str = "hype-";

/// How many hex digits the body of a minted hypervisor id carries: the eight of
/// `hype-1a2b3c4d`, the width a hypervisor identity of this service is served with.
///
/// Narrower than [`GATEWAY_ID_DIGITS`] because the width goes with the kind, not with the
/// store — see [`MintedResource::id_body_digits`].
const HYPERVISOR_ID_DIGITS: usize = 8;

/// The ARN resource type of a virtual machine: `vm/<id>`.
///
/// The third resource this service's ARNs name, spelled as the modelled
/// `VirtualMachine$ResourceArn` doc spells it
/// (`smithy_query(query="shape",shape="VirtualMachine")`: "For example,
/// `arn:aws:backup-gateway:us-west-1:0000000000000:vm/vm-0000ABCDEFGIJKL`").
pub const VIRTUAL_MACHINE_RESOURCE_TYPE: &str = "vm";

/// The id prefix of a virtual machine this store minted, for the same reason as
/// [`GATEWAY_ID_PREFIX`].
const VIRTUAL_MACHINE_ID_PREFIX: &str = "vm-";

/// How many hex digits the body of a minted virtual-machine id carries, for the same
/// reason as [`GATEWAY_ID_DIGITS`]: sixteen, a body of the same order as the one the
/// modelled `VirtualMachine$ResourceArn` doc illustrates (`vm/vm-0000ABCDEFGIJKL`,
/// thirteen characters behind the prefix), which is the only thing said about a machine
/// id's spelling anywhere.
const VIRTUAL_MACHINE_ID_DIGITS: usize = 16;

/// `ErrorCode` for a well-formed hypervisor ARN that names no hypervisor, and the
/// sentence that states it. See [`MissingResource`].
///
/// It is `ResourceNotFoundException$ErrorCode` for an operation that models that shape,
/// and `ValidationException$ErrorCode` — "a description of what caused the validation
/// error" — for one that does not: `AssociateGatewayToServer` models only
/// `ConflictException` beside this service's common errors
/// (`smithy_query(query="operation",operation="AssociateGatewayToServer")`), so the
/// condition is named by the same code whichever modelled shape carries it.
pub const HYPERVISOR_NOT_FOUND: &str = "HypervisorNotFound";

/// See [`HYPERVISOR_NOT_FOUND`].
pub const HYPERVISOR_NOT_FOUND_MESSAGE: &str = "The hypervisor is not found.";

/// `ErrorCode` for a well-formed gateway ARN that names no gateway, and the sentence that
/// states it. See [`MissingResource::Gateway`].
///
/// Read like [`HYPERVISOR_NOT_FOUND`]: it is `ResourceNotFoundException$ErrorCode` — "a
/// description of which resource wasn't found" — for an operation that models that shape,
/// and the same code inside whatever other modelled shape an operation that does not model
/// it has to carry the condition in, so one condition is named one way whichever shape
/// carries it.
pub const GATEWAY_NOT_FOUND: &str = "GatewayNotFound";

/// See [`GATEWAY_NOT_FOUND`]: the sentence AWS states for that refusal, entirely fixed
/// text. No part of it comes from the request — the ARN that resolved to nothing is a
/// request value, so it is not echoed back.
pub const GATEWAY_NOT_FOUND_MESSAGE: &str = "The specified gateway was not found.";

/// `ErrorCode` for a well-formed virtual-machine ARN that names no virtual machine this
/// service reports, and the sentence that states it. See
/// [`MissingResource::VirtualMachine`].
///
/// The third resource type this service resolves ([`VIRTUAL_MACHINE_RESOURCE_TYPE`]), named
/// in `ResourceNotFoundException$ErrorCode` the way [`HYPERVISOR_NOT_FOUND`] and
/// [`GATEWAY_NOT_FOUND`] name their own, so a client can tell which of the three resolved
/// to nothing.
pub const VIRTUAL_MACHINE_NOT_FOUND: &str = "VirtualMachineNotFound";

/// See [`VIRTUAL_MACHINE_NOT_FOUND`]: the sentence AWS states for that refusal, entirely
/// fixed text with no part taken from the request.
pub const VIRTUAL_MACHINE_NOT_FOUND_MESSAGE: &str = "The virtual machine is not found.";

/// `ValidationException$ErrorCode` for a request whose parameters this service will not act
/// on, and the sentence that states it.
///
/// `TestHypervisorConfiguration` "tests your hypervisor configuration to validate that
/// backup gateway can connect with the hypervisor and its resources"
/// (`smithy_query(query="operation",operation="TestHypervisorConfiguration")`): the whole
/// request is the configuration under test, so a `GatewayArn` that names no gateway makes
/// the REQUEST invalid rather than naming a resource the caller asked to read — AWS
/// answers it with the common `ValidationException` and this code rather than resolving the
/// ARN. `ValidationException$ErrorCode` is documented "A description of what caused the
/// validation error" (`smithy_query(query="shape",shape="ValidationException")`).
pub const INVALID_PARAMETER: &str = "InvalidParameter";

/// See [`INVALID_PARAMETER`]: the sentence AWS states for that refusal, entirely fixed
/// text — it names no member and echoes no value, so nothing of the request reaches it.
pub const INVALID_PARAMETER_MESSAGE: &str = "The specified request contains invalid parameters.";

/// `ConflictException$ErrorCode` for associating a gateway that is already associated
/// with a DIFFERENT server, and the sentence that states it.
///
/// A gateway backs up the virtual machines of ONE hypervisor — `GatewayDetails` carries a
/// single `HypervisorId` (`smithy_query(query="shape",shape="GatewayDetails")`) — so
/// moving an associated gateway to another server is the modelled "the operation cannot
/// proceed because it is not supported", whose `ErrorCode` member is REQUIRED and
/// documented as "a description of why the operation is not supported". The gateway has
/// to be disassociated first (`DisassociateGatewayFromServer`).
pub const GATEWAY_ALREADY_ASSOCIATED: &str = "GatewayAlreadyAssociated";

/// See [`GATEWAY_ALREADY_ASSOCIATED`]: the sentence the refusal states, naming the server
/// the gateway is associated with — read off the stored link, so the caller learns which
/// server it has to disassociate the gateway from. AWS publishes no wording for the
/// member, so this sentence is an example; the requirement is the shape, its status and
/// the `ErrorCode`.
pub fn gateway_already_associated_message(server_arn: &str) -> String {
    format!("The gateway is already associated with the server '{server_arn}'.")
}

/// `ConflictException$ErrorCode` for disassociating a gateway that is associated with NO
/// server, and the sentence that states it.
///
/// `DisassociateGatewayFromServer` "disassociates a backup gateway from the specified
/// server" — the server is not a member of its input, so the server it names is the one the
/// gateway's stored link points at
/// (`smithy_query(query="operation",operation="DisassociateGatewayFromServer")`). With no
/// link there is no server the request names, and the operation is NOT `@idempotent` where
/// `DeleteGateway` is, so repeating it is not declared to succeed: it is the modelled "the
/// operation cannot proceed because it is not supported", whose `ErrorCode` member is
/// REQUIRED and documented as "a description of why the operation is not supported". The
/// code names the condition the way [`GATEWAY_ALREADY_ASSOCIATED`] names its own.
pub const GATEWAY_NOT_ASSOCIATED: &str = "GatewayNotAssociated";

/// See [`GATEWAY_NOT_ASSOCIATED`]: the sentence the refusal states, phrased like
/// [`gateway_already_associated_message`] so the two halves of one link are worded alike.
/// AWS publishes no wording for the member, so this sentence is an example; the
/// requirement is the shape, its status and the `ErrorCode`.
pub const GATEWAY_NOT_ASSOCIATED_MESSAGE: &str = "The gateway is not associated with a server.";

/// `ResourceNotFoundException$ErrorCode` for a hypervisor that really exists but carries
/// no property mappings, so there is no mappings resource to report. See
/// [`MissingResource::PropertyMappings`].
///
/// `ErrorCode` is documented as "A description of which resource wasn't found"
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`), and the resource
/// that is missing here is the hypervisor's property MAPPINGS, not the hypervisor — so the
/// code names them, the way [`HYPERVISOR_NOT_FOUND`] names the hypervisor.
pub const MAPPINGS_NOT_FOUND: &str = "HypervisorPropertyMappingsNotFound";

/// See [`MAPPINGS_NOT_FOUND`]: the sentence AWS states for that refusal, entirely fixed
/// text — including its number disagreement, which is part of the fixed string — with no
/// part taken from the request. The wording is a REQUIREMENT of the error class and not
/// this crate's choice, so `tests/all/fixed_refusal_sentences.rs` pins it written out,
/// rather than read off this constant.
pub const MAPPINGS_NOT_FOUND_MESSAGE: &str = "The hypervisor property mappings is not found.";

/// `AccessDeniedException$ErrorCode` for a metadata sync asked for a `HypervisorArn` this
/// store holds no property mappings for.
///
/// A metadata sync has a PRECONDITION: the hypervisor's property mappings
/// ([`StoredPropertyMappings`], what `PutHypervisorPropertyMappings` sets) have to be
/// configured before the sync is started, and a `HypervisorArn` this store holds none
/// under — because the record carries no mappings, or because there is no record at all —
/// is refused rather than synced. The refusal is a PERMISSION matter, the modelled
/// `AccessDeniedException`, so it says nothing about whether the ARN names anything.
///
/// `AccessDeniedException$ErrorCode` is REQUIRED and documented as "A description of why
/// you have insufficient permissions"
/// (`smithy_query(query="shape",shape="AccessDeniedException")`). Both halves — the shape
/// and this `ErrorCode`, answered for an ARN naming no hypervisor at all — are what the
/// recorded unit `backup-gateway.start-metadata-sync-state-transition` pins
/// (`issues/things-that-now-work/20261023T110543Z-backup-gateway-startvirtualmachinesmetadatasync-stub.md`,
/// whose check asserts an `AccessDeniedException` with this code for a nonexistent
/// `HypervisorArn`, before any resource-existence lookup), so neither the value nor the
/// answer being a refusal of that kind is this emulator's choice — and the gate firing
/// before the existence lookup is why one ARN naming nothing and one naming an
/// unconfigured record take the same answer.
pub const MAPPINGS_NOT_CONFIGURED: &str = "MappingsNotConfigured";

/// See [`MAPPINGS_NOT_CONFIGURED`]: the sentence AWS is recorded stating for that refusal,
/// entirely fixed text with no part taken from the request — no trailing period included.
/// It names the condition the caller has to remedy (the hypervisor's property mappings),
/// not the generic permissions wording `AccessDeniedException`'s own documentation carries;
/// `tests/all/fixed_refusal_sentences.rs` pins it written out, rather than read off this
/// constant.
pub const MAPPINGS_NOT_CONFIGURED_MESSAGE: &str =
    "HypervisorPropertyMappings is not configured for the Hypervisor";

/// The status this service answers the [`MAPPINGS_NOT_CONFIGURED`] refusal with.
///
/// NOT the status `AccessDeniedException`'s `@httpError` declares (403): AWS is recorded
/// answering this precondition refusal 400, so the handler overrides the modelled status
/// exactly as [`MissingResource::status_override`] does for a missing hypervisor — and the
/// choice is stated here, beside this service's other refusal statuses, rather than in the
/// handler.
pub const MAPPINGS_NOT_CONFIGURED_STATUS: u16 = 400;

/// `ValidationException$ErrorCode` for a request that only the backup gateway serving a
/// hypervisor can carry out, made for a hypervisor NO gateway is associated with.
///
/// Property mappings are applied to the hypervisor's entities BY the gateway appliance
/// that serves it — `PutHypervisorPropertyMappings` "sets the property mappings for the
/// specified hypervisor", the "relationship of entity properties available from the
/// hypervisor to the properties available in Amazon Web Services"
/// (`smithy_query(query="operation",operation="PutHypervisorPropertyMappings")`), and a
/// gateway is associated with a hypervisor by `AssociateGatewayToServer` — so with no
/// such gateway there is no appliance that can apply them, which is what AWS is recorded
/// refusing with `ValidationException` and [`GATEWAY_OUT_OF_DATE_MESSAGE`].
///
/// It says nothing about whether the hypervisor EXISTS: this is the refusal for a record
/// the service holds and no appliance serves, so an ARN naming no record is resolved and
/// refused as a missing hypervisor ([`HYPERVISOR_NOT_FOUND`]) before an appliance is looked
/// for at all.
///
/// `ValidationException$ErrorCode` is documented as "A description of what caused the
/// validation error" (`smithy_query(query="shape",shape="ValidationException")`) and AWS
/// publishes no vocabulary for that member, so the value is the one AWS is recorded
/// sending — its LEADING SPACE included, which is part of the token and not a transcription
/// slip, so a client keyed on the recorded code matches.
pub const GATEWAY_OUT_OF_DATE: &str = " OutdatedGateway";

/// See [`GATEWAY_OUT_OF_DATE`]: the sentence AWS is recorded stating for that refusal
/// (`issues/.../backup-gateway-puthypervisorpropertymappings-stub.md`), kept in one place
/// so every operation the gateway has to carry out refuses the same way.
pub const GATEWAY_OUT_OF_DATE_MESSAGE: &str = "The specified gateway is out of date.";

/// `ValidationException$ErrorCode` for a maintenance window that names a day of the week
/// AND a day of the month, and the sentence that states it.
///
/// A maintenance start time is one window: "This is your gateway's weekly maintenance
/// start time including the day and time of the week. … Can be weekly or monthly"
/// (`smithy_query(query="shape",shape="MaintenanceStartTime")`) — weekly OR monthly, so
/// the two day members cannot hold together and a request carrying both states no window
/// the service can keep. The model declares each member on its own, so the boundary
/// validator cannot express the exclusion and the handler enforces it.
///
/// `ValidationException$ErrorCode` is documented as "A description of what caused the
/// validation error" (`smithy_query(query="shape",shape="ValidationException")`) and AWS
/// publishes no vocabulary for that member, so this value names the condition the way
/// [`HYPERVISOR_NOT_FOUND`] names its own.
pub const MAINTENANCE_START_TIME_INVALID: &str = "InvalidMaintenanceStartTime";

/// See [`MAINTENANCE_START_TIME_INVALID`]. AWS publishes no wording for this refusal, so
/// the sentence is an example; the requirement is the error shape, the status and the
/// `ErrorCode`, and the sentence names the two members that cannot hold together so the
/// caller can fix the request from it alone.
pub const MAINTENANCE_START_TIME_INVALID_MESSAGE: &str =
    "A maintenance start time is weekly or monthly: DayOfWeek and DayOfMonth cannot both \
     be specified.";

/// `ValidationException$ErrorCode` for a bandwidth rate-limit schedule whose intervals
/// cannot hold together, and the sentences that state each way they fail to.
///
/// A schedule is what ONE gateway appliance applies: "A bandwidth rate limit schedule
/// consists of one or more bandwidth rate limit intervals. A bandwidth rate limit interval
/// defines a period of time on one or more days of the week, during which bandwidth rate
/// limits are specified for uploading"
/// (`smithy_query(query="shape",shape="BandwidthRateLimitInterval")`), and an interval is
/// an intra-day window — "The bandwidth rate limit interval ends at the end of the minute.
/// To end an interval at the end of an hour, use the value `59`" (the `EndMinuteOfHour`
/// doc) — so no minute of any day of the week may be covered by two rate-limit
/// specifications, and no interval may end before it starts. The model declares each
/// member on its own, so the boundary validator cannot express either, and
/// [`bandwidth_rate_limit_schedule_violation`] enforces both.
///
/// `ValidationException$ErrorCode` is documented as "A description of what caused the
/// validation error" (`smithy_query(query="shape",shape="ValidationException")`) and AWS
/// publishes no vocabulary for that member, so this value names the condition the way
/// [`MAINTENANCE_START_TIME_INVALID`] names its own.
pub const BANDWIDTH_RATE_LIMIT_SCHEDULE_INVALID: &str = "InvalidBandwidthRateLimitSchedule";

/// The minute of the day a `BandwidthRateLimitInterval` hour/minute pair names, so two
/// endpoints of a window are compared as one number. `HourOfDay` is `@range(min: 0, max:
/// 23)` and `MinuteOfHour` `@range(min: 0, max: 59)`, both refused at the boundary before a
/// handler runs, so the product cannot leave the day.
fn minute_of_day(hour_of_day: i32, minute_of_hour: i32) -> i32 {
    hour_of_day * 60 + minute_of_hour
}

/// The lowest day of the week on which `one` and `other` both specify a rate limit for the
/// same minute, and the first and last minute of the day they share, or `None` when the
/// two intervals can be applied together.
///
/// Both windows are INCLUSIVE of their end minute (the `EndMinuteOfHour` doc: the interval
/// "ends at the end of the minute"), so two windows clash as soon as one's start is not
/// past the other's end.
fn overlapping_day_of_week(
    one: &BandwidthRateLimitInterval,
    other: &BandwidthRateLimitInterval,
) -> Option<(i32, i32, i32)> {
    let one_start = minute_of_day(one.start_hour_of_day, one.start_minute_of_hour);
    let one_end = minute_of_day(one.end_hour_of_day, one.end_minute_of_hour);
    let other_start = minute_of_day(other.start_hour_of_day, other.start_minute_of_hour);
    let other_end = minute_of_day(other.end_hour_of_day, other.end_minute_of_hour);
    if one_start > other_end || other_start > one_end {
        return None;
    }
    let day = one
        .days_of_week
        .iter()
        .copied()
        .filter(|day| other.days_of_week.contains(day))
        .min()?;
    Some((day, one_start.max(other_start), one_end.min(other_end)))
}

/// Why the bandwidth rate-limit schedule `intervals` states cannot be applied, as the
/// sentence a [`BANDWIDTH_RATE_LIMIT_SCHEDULE_INVALID`] refusal carries — or `None` when
/// every interval of it holds together and the schedule can be persisted.
///
/// Three ways a schedule fails, all one rule — a minute of a day of the week carries at
/// most one rate limit, and an interval is a window inside one day:
///
/// * an interval whose end precedes its start, which is no window of a single day;
/// * an interval listing a day of the week twice, which states the same minutes twice;
/// * two intervals sharing a day whose windows overlap, which an appliance cannot apply
///   at once.
///
/// Every member of every interval is already inside the constraints the model declares by
/// the time a handler can call this, so what is left is how the intervals stand to one
/// another. AWS publishes no wording for these refusals, so each sentence below is an
/// example: the requirement is the error shape, its status and the `ErrorCode`, and the
/// sentence names the intervals and the values that clash — taken from the request — so the
/// caller can fix it from the refusal alone. An interval is named by its place in the
/// list, counted as [`crate::service::element_member_path`] counts a list's elements —
/// from 1 — so this service answers "which element did you refuse?" in one numbering
/// wherever the refusal is worded.
pub fn bandwidth_rate_limit_schedule_violation(
    intervals: &[BandwidthRateLimitInterval],
) -> Option<String> {
    // Each interval on its own first: an interval that is not a window of one day has no
    // meaningful overlap with anything else.
    for (index, interval) in intervals.iter().enumerate() {
        // As in `validate_tag_mappings`: the position names the interval that was refused.
        let position = index + 1;
        if minute_of_day(interval.end_hour_of_day, interval.end_minute_of_hour)
            < minute_of_day(interval.start_hour_of_day, interval.start_minute_of_hour)
        {
            return Some(format!(
                "The bandwidth rate limit interval at position {position} ends at \
                 {:02}:{:02}, before it starts at {:02}:{:02}: a bandwidth rate limit \
                 interval is a period of time within one day, so EndHourOfDay and \
                 EndMinuteOfHour cannot precede StartHourOfDay and StartMinuteOfHour.",
                interval.end_hour_of_day,
                interval.end_minute_of_hour,
                interval.start_hour_of_day,
                interval.start_minute_of_hour,
            ));
        }
        for (day_index, day) in interval.days_of_week.iter().enumerate() {
            if interval.days_of_week[..day_index].contains(day) {
                return Some(format!(
                    "The bandwidth rate limit interval at position {position} lists day \
                     {day} of the week more than once: DaysOfWeek names each day of the \
                     week the interval applies to at most once."
                ));
            }
        }
    }

    // Then every pair, in the order the request listed them, so the same schedule is
    // always refused for the same pair.
    for (index, interval) in intervals.iter().enumerate() {
        for (other_index, other) in intervals.iter().enumerate().skip(index + 1) {
            if let Some((day, from, through)) = overlapping_day_of_week(interval, other) {
                // Both intervals named in the same numbering a single-interval refusal uses.
                let position = index + 1;
                let other_position = other_index + 1;
                return Some(format!(
                    "The bandwidth rate limit intervals at positions {position} and \
                     {other_position} both apply on day {day} of the week, from {:02}:{:02} \
                     through {:02}:{:02}: bandwidth rate limit intervals cannot overlap.",
                    from / 60,
                    from % 60,
                    through / 60,
                    through % 60,
                ));
            }
        }
    }

    None
}

/// `ValidationException$ErrorCode` for a `CreateGateway` whose `ActivationKey` names no
/// activation this service has issued and not yet redeemed.
///
/// An activation key is MINTED by the activation handshake a deployed appliance makes
/// ([`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::issue_activation_key`]) and
/// redeemed by the create that activates that appliance, so a key no handshake issued —
/// or one a create has already redeemed — names no activation to redeem. `CreateGateway`
/// declares exactly one client error (`ValidationException`, HTTP 400:
/// `smithy_query(query="operation",operation="CreateGateway")`), so that is the shape the
/// refusal takes, and this `ErrorCode` names the condition inside it.
pub const ACTIVATION_KEY_NOT_FOUND: &str = "ActivationKeyNotFound";

/// See [`ACTIVATION_KEY_NOT_FOUND`]: the sentence AWS is recorded stating for that refusal
/// (`issues/.../backup-gateway-negative-paths-succeed-or-return-the.md`), kept in one place
/// so every refusal of an unredeemable key is worded the same way.
pub const ACTIVATION_KEY_NOT_FOUND_MESSAGE: &str = "The specified activation key was not found.";

/// How many hyphen-separated groups an issued activation key carries, and how many
/// symbols each group holds.
///
/// The form a gateway appliance prints: the sibling storage-gateway model records
/// `29AV1-3OFV9-VVIUB-NKT0I-LRO6V` as the `ActivationKey` of its `ActivateGateway`
/// example (`smithy_query(query="operation",operation="ActivateGateway",
/// model_path="services/service-com-amazonaws-storagegateway-storagegateway-20130630/\
/// smithy-model/com-amazonaws-storagegateway-storagegateway-20130630.json")`). Five groups
/// of five keeps every issued key inside this model's own `ActivationKey` constraints
/// (len 1..50, pattern `^[0-9a-zA-Z\-]+$`), so a key this service issues is a key its own
/// boundary validator accepts.
const ACTIVATION_KEY_GROUPS: usize = 5;

/// See [`ACTIVATION_KEY_GROUPS`].
const ACTIVATION_KEY_GROUP_SYMBOLS: usize = 5;

/// The region a minted ARN falls back to when the request carries no SigV4 credential
/// scope to read one from (an unsigned request on an emulator with authentication off).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The longest region segment an ARN of this service may carry.
///
/// A region reaches a mint CALLER-SUPPLIED — off the credential scope the request was
/// signed with, which the signing format bounds only to non-empty — while every ARN this
/// service takes as INPUT is bounded by the model: 180 characters for `GatewayArn` and 500
/// for `ServerArn`/`ResourceArn` (`smithy_query(query="shape",shape="GatewayArn,
/// ServerArn")`). So the region segment is bounded here, where every ARN of this service is
/// minted and matched, and no record is ever keyed under an ARN its own API would refuse.
/// An EMULATOR bound, well past the longest region name AWS publishes and far enough inside
/// the tightest modelled maximum that every resource type's ARN fits it.
pub const MAX_ARN_REGION_LENGTH: usize = 64;

/// The version the emulated gateway appliance reports as `SoftwareVersion`.
///
/// `GatewayDetails$SoftwareVersion` targets the `Name` shape, whose modelled pattern is
/// `^[a-zA-Z0-9-]*$` (`smithy_query(query="shape",shape="GatewayDetails")`), so a dotted
/// version is not a legal value for this member — the appliance version is rendered with
/// hyphens.
pub const GATEWAY_SOFTWARE_VERSION: &str = "2-0-0";

/// The version an appliance runs once `UpdateGatewaySoftwareNow` has installed the update
/// it triggers — the newest release of the emulated appliance software.
///
/// CHOSEN, like [`GATEWAY_SOFTWARE_VERSION`], and rendered the same way, because AWS
/// publishes no version list for the appliance; what matters is that it DIFFERS from the
/// version a gateway is activated with, so an install is observable, and that it is the
/// newest one — an appliance already running it has nothing left to install.
pub const GATEWAY_UPDATED_SOFTWARE_VERSION: &str = "2-0-1";

/// How long the appliance software update `UpdateGatewaySoftwareNow` triggers takes to
/// install.
///
/// A CHOSEN span, not a documented one: AWS states only that the request triggers the
/// update immediately, answers 200 at once, and that "it might take some time for the
/// update to complete" (`smithy_query(query="operation",operation="UpdateGatewaySoftwareNow")`).
/// It is non-zero so a caller that reads straight after the trigger still sees the version
/// the appliance was running, and short enough that a caller which waits sees the new one.
/// Nothing sleeps for it — the due instant is stored and every read compares the clock
/// against it ([`StoredGateway::software_version_as_of`]).
pub const GATEWAY_SOFTWARE_UPDATE_INSTALLS_AFTER: Duration = Duration::from_millis(500);

/// `MaintenanceStartTime$DayOfWeek` of the window a gateway is activated with.
///
/// An appliance has a weekly maintenance window for as long as it exists:
/// `GatewayDetails$MaintenanceStartTime` "Returns your gateway's weekly maintenance start
/// time including the day and time of the week"
/// (`smithy_query(query="shape",shape="GatewayDetails")`), and `PutMaintenanceStartTime`
/// "Set the maintenance start time for a gateway"
/// (`smithy_query(query="operation",operation="PutMaintenanceStartTime")`) — it overrides
/// the window the appliance already has rather than giving it its first one. So the window
/// is assigned when the gateway is activated ([`default_maintenance_start_time`]) and a
/// read reports one for a gateway no put has ever touched.
///
/// WHICH window that is, is CHOSEN — like [`GATEWAY_SOFTWARE_VERSION`], AWS publishes no
/// value for it. It is a WEEKLY window, the form the member's own documentation names (the
/// shape can hold a weekly or a monthly one, so a default has to pick a form), and it falls
/// in the quiet hours of the week, which is what an appliance maintenance window is for.
/// `DayOfWeek` is "an ordinal number between 0 and 6 … where 0 represents Sunday", so this
/// is Sunday, and every default member below is inside its published range
/// (`DayOfWeek` 0..6, `HourOfDay` 0..23, `MinuteOfHour` 0..59).
pub const DEFAULT_MAINTENANCE_DAY_OF_WEEK: i32 = 0;

/// `MaintenanceStartTime$HourOfDay` of the window a gateway is activated with. See
/// [`DEFAULT_MAINTENANCE_DAY_OF_WEEK`].
pub const DEFAULT_MAINTENANCE_HOUR_OF_DAY: i32 = 3;

/// `MaintenanceStartTime$MinuteOfHour` of the window a gateway is activated with. See
/// [`DEFAULT_MAINTENANCE_DAY_OF_WEEK`].
pub const DEFAULT_MAINTENANCE_MINUTE_OF_HOUR: i32 = 0;

/// The maintenance window a gateway is activated with, as the MODELLED shape — the one
/// [`StoredGateway::maintenance_start_time`] holds until a put replaces it.
///
/// `DayOfMonth` is absent because the window is weekly: the two day members are
/// alternatives (see [`MAINTENANCE_START_TIME_INVALID`]), so a weekly window carries
/// `DayOfWeek` and nothing else. See [`DEFAULT_MAINTENANCE_DAY_OF_WEEK`] for where the
/// values come from.
pub fn default_maintenance_start_time() -> MaintenanceStartTime {
    MaintenanceStartTime {
        day_of_month: None,
        day_of_week: Some(DEFAULT_MAINTENANCE_DAY_OF_WEEK),
        hour_of_day: DEFAULT_MAINTENANCE_HOUR_OF_DAY,
        minute_of_hour: DEFAULT_MAINTENANCE_MINUTE_OF_HOUR,
    }
}

/// The only `GatewayType` the model declares (`GATEWAY_TYPE_VALUES`).
pub const BACKUP_VM: &str = "BACKUP_VM";

/// `HypervisorState` right after a hypervisor configuration is imported, before the
/// service has reached the host.
///
/// One of the four values the modelled enum declares — `PENDING`, `ONLINE`, `OFFLINE`,
/// `ERROR` (`smithy_query(query="shape",shape="HypervisorState")`), whose documented
/// order the `HypervisorDetails$State` doc restates.
pub const HYPERVISOR_PENDING: &str = "PENDING";

/// `HypervisorState` once the service has reached the hypervisor host. See
/// [`HYPERVISOR_PENDING`].
pub const HYPERVISOR_ONLINE: &str = "ONLINE";

/// How long a freshly imported hypervisor stays [`HYPERVISOR_PENDING`] before it is
/// [`HYPERVISOR_ONLINE`].
///
/// A CHOSEN span, not a documented one: AWS states no duration for the activation an
/// import starts, only that a hypervisor's state passes through `PENDING`. It is
/// non-zero so a caller that reads straight after an import still observes `PENDING`,
/// and short enough that a caller which waits sees `ONLINE`. Nothing sleeps for it — the
/// due instant is stored and every read compares the clock against it
/// ([`StoredHypervisor::state_as_of`]).
pub const HYPERVISOR_PENDING_FOR: Duration = Duration::from_millis(500);

/// `SyncMetadataStatus` the moment a metadata sync has been asked for: the sync request
/// exists, and the service has not started reading the hypervisor's inventory yet.
///
/// One of the five values the modelled enum declares — `CREATED`, `RUNNING`, `FAILED`,
/// `PARTIALLY_FAILED`, `SUCCEEDED`
/// (`smithy_query(query="shape",shape="SyncMetadataStatus")`).
pub const SYNC_METADATA_CREATED: &str = "CREATED";

/// `SyncMetadataStatus` while the sync is reading the hypervisor's inventory. See
/// [`SYNC_METADATA_CREATED`].
pub const SYNC_METADATA_RUNNING: &str = "RUNNING";

/// `SyncMetadataStatus` once the sync has finished. See [`SYNC_METADATA_CREATED`].
pub const SYNC_METADATA_SUCCEEDED: &str = "SUCCEEDED";

/// How long after the start call a metadata sync stays [`SYNC_METADATA_CREATED`] before
/// it is [`SYNC_METADATA_RUNNING`].
///
/// A CHOSEN span, not a documented one: AWS states no duration for a metadata sync, only
/// the statuses one passes through. It is non-zero so a caller that reads straight after
/// the start call still observes the sync it just requested as `CREATED`. Nothing sleeps
/// for it — the instant the sync started at is stored and every read compares the clock
/// against it ([`StoredHypervisor::latest_metadata_sync_status_as_of`]).
pub const METADATA_SYNC_RUNNING_AFTER: Duration = Duration::from_millis(250);

/// How long after the start call a metadata sync takes to be
/// [`SYNC_METADATA_SUCCEEDED`]. A CHOSEN span, for the same reason as
/// [`METADATA_SYNC_RUNNING_AFTER`], and the instant the sync's success is dated at.
pub const METADATA_SYNC_SUCCEEDS_AFTER: Duration = Duration::from_millis(500);

/// `LatestMetadataSyncStatusMessage` for each status a sync this service started passes
/// through — "the most recent status for the indicated metadata sync"
/// (`smithy_query(query="shape",shape="HypervisorDetails",member="LatestMetadataSyncStatusMessage")`),
/// stated in words beside the `SyncMetadataStatus` value. AWS publishes no wording for
/// the member, so these sentences are examples; the requirement is that the message
/// describes the status the same record reports.
pub const METADATA_SYNC_CREATED_MESSAGE: &str = "The metadata sync request was created.";

/// See [`METADATA_SYNC_CREATED_MESSAGE`].
pub const METADATA_SYNC_RUNNING_MESSAGE: &str = "The metadata sync is running.";

/// See [`METADATA_SYNC_CREATED_MESSAGE`].
pub const METADATA_SYNC_SUCCEEDED_MESSAGE: &str = "The metadata sync succeeded.";

/// The largest number of gateways this emulator keeps.
///
/// A ceiling, not an AWS quota: `CreateGateway` is the one operation that grows resident
/// memory, one record per call, for the life of the process, so the store is bounded and
/// a create past the ceiling is refused rather than served. The same bound the other
/// services in this workspace use for a caller-driven store.
pub const MAX_STORED_GATEWAYS: usize = 1_000;

/// The largest number of issued-but-unredeemed activation keys this emulator keeps.
///
/// The activation handshake is unsigned — it is what a deployed appliance makes BEFORE
/// anybody holds a gateway — so the registry it writes to is bounded here: once it is
/// full, issuing a key drops the key that has been pending longest
/// ([`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::issue_activation_key`]),
/// which keeps resident memory bounded without letting one caller's handshakes stop
/// another appliance from activating.
pub const MAX_PENDING_ACTIVATION_KEYS: usize = 1_000;

/// The largest number of hypervisors this emulator keeps, for the same reason as
/// [`MAX_STORED_GATEWAYS`]: `ImportHypervisorConfiguration` grows resident memory one
/// record per call, so the store is bounded and an import past the ceiling is refused
/// rather than served.
pub const MAX_STORED_HYPERVISORS: usize = 1_000;

/// The largest number of virtual machines this emulator keeps, for the same reason as
/// [`MAX_STORED_GATEWAYS`]: the inventory a hypervisor reports grows resident memory one
/// record per machine, so the store is bounded and a record past the ceiling is not kept.
pub const MAX_STORED_VIRTUAL_MACHINES: usize = 10_000;

/// The largest number of (resource kind, region) mint sequences this emulator keeps.
///
/// A mint sequence is a `u64` per kind per region a record has been minted in, and the
/// region a record is minted in comes off the request — so the registry is bounded here
/// for the same reason the stores above are. The ceiling is the stores' own ceilings
/// together, because a pair with a record stored under it must keep its own sequence:
/// once the registry reaches it, the ENTRIES of the pairs this store holds no record for
/// are dropped, and the sequence each had reached is folded into that kind's floor
/// ([`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::bound_mint_sequences`]), so
/// bounding the registry frees entries without un-spending an identity: a later mint for a
/// dropped pair resumes at or above every sequence that was dropped, never back at zero.
pub const MAX_MINT_SEQUENCES: usize =
    MAX_STORED_GATEWAYS + MAX_STORED_HYPERVISORS + MAX_STORED_VIRTUAL_MACHINES;

/// How many virtual machines a hypervisor reports in the inventory a metadata sync reads.
///
/// A CHOSEN count, as [`METADATA_SYNC_SUCCEEDS_AFTER`] is a chosen span: AWS states no
/// number — an inventory holds whatever the on-premises hypervisor holds. It is more than
/// one, so a listing over one hypervisor's inventory really has to page and filter, and
/// small, so syncing every hypervisor the store can hold stays well inside
/// [`MAX_STORED_VIRTUAL_MACHINES`]. See [`synced_inventory`].
pub const SYNCED_VIRTUAL_MACHINES: usize = 3;

/// The longest `VirtualMachine$Name` the model accepts — `@length(min: 1, max: 100)`
/// (`smithy_query(query="shape",shape="VirtualMachine")`) — which the name
/// [`synced_inventory`] reports for a machine is kept inside.
pub const MAX_VIRTUAL_MACHINE_NAME: usize = 100;

/// How many hex digits of a hypervisor record's identity FINGERPRINT the name of a machine
/// in its inventory carries ([`hypervisor_fingerprint`], [`synced_machine_name`]).
///
/// A fingerprint and not the ARN itself because `VirtualMachine$Name` is
/// `@length(min: 1, max: 100)` with pattern `^[a-zA-Z0-9-]*$`
/// (`smithy_query(query="shape",shape="VirtualMachine")`): an ARN of this service is at
/// least 50 characters (`ResourceArn`'s own `@length`) and separates its segments with
/// `:`, which that pattern admits nowhere, so no name can spell one out — while joining
/// the segments with a character the pattern does admit is not reversible, because the
/// segments contain that character themselves. 16 hex digits are 64 bits of the digest,
/// which keeps the name far inside [`MAX_VIRTUAL_MACHINE_NAME`] however long the ARN is.
const HYPERVISOR_FINGERPRINT_DIGITS: usize = 16;

/// The inventory folder the `VirtualMachine$Path` of a discovered machine sits in: a path
/// "is the path of the virtual machine" in its hypervisor's inventory
/// (`smithy_query(query="shape",shape="VirtualMachine",member="Path")`), which on a VMware
/// hypervisor is a datacenter and a folder under it.
const SYNCED_INVENTORY_FOLDER: &str = "/Datacenter/vm";

/// The page size a list operation of this service uses when a request names no
/// `MaxResults`, and the largest page it will return whatever `MaxResults` asks for.
///
/// The model declares neither (`MaxResults` has a minimum of 1 and no maximum), so these
/// are the emulator's own server-side bounds: without them one tiny request would clone
/// and serialise the whole store, so the response a caller can ask for is capped and the
/// rest is reached through `NextToken`.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// See [`DEFAULT_PAGE_SIZE`].
pub const MAX_PAGE_SIZE: usize = 100;

/// The page size a listing serves for the `MaxResults` a request named:
/// [`DEFAULT_PAGE_SIZE`] when it named none, and otherwise the size it asked for, floored
/// at one record and capped at [`MAX_PAGE_SIZE`].
///
/// Every list operation of this service pages by this one rule, so it is computed here —
/// beside the two bounds it reads — rather than restated by each listing handler: nothing
/// about it is resource-specific, and the paging core ([`Page`]) already relies on it, its
/// doc recording that the `limit` a page is cut with is the caller's `MaxResults` ALREADY
/// clamped. This is what clamps it.
///
/// The floor is belt-and-braces: `MaxResults` is modelled with a minimum of 1 and no
/// maximum, so a smaller value is refused at the boundary before a handler runs, and a
/// page of no records — which would carry a token on for ever without handing out a
/// record — cannot be asked for.
pub fn page_limit(max_results: Option<i32>) -> usize {
    max_results.map_or(DEFAULT_PAGE_SIZE, |max| {
        (max.max(1) as usize).min(MAX_PAGE_SIZE)
    })
}

/// The whole `ValidationException$Message` every paginated listing of this service answers
/// a `NextToken` it never issued with.
///
/// An unresolvable pagination token is a condition class of its own, NOT a violation of a
/// modelled constraint on the token member: `NextToken` is `@length(1, 1000)` with
/// `@pattern ^.+$` (`smithy_query(query="operation",operation="ListGateways")`), so a token
/// that satisfies those traits is a well-formed value whose PROVENANCE — whether a previous
/// response of that same listing minted it ([`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::resume_after`])
/// — is what fails. AWS states that condition in one FIXED sentence: no part of it is taken
/// from the request or from state, so the token is never echoed, nor is the member path or
/// the expression it satisfied. The sentence is recorded behaviour (the behavioral unit
/// `backup-gateway.list-gateways-returns-list-shape` pins it); the modelled half is the
/// error shape and its `@httpError 400`.
///
/// Kept here, in one place, because all three listings resolve a token through the one
/// helper and so refuse an unissued one identically.
pub const PAGE_TOKEN_INVALID_MESSAGE: &str = "The pagination token is invalid and cannot be processed.";

/// How many tags one resource can carry.
///
/// From the modelled doc of `CreateGatewayInput$Tags`: "A list of up to 50 tags to
/// assign to the gateway. Each tag is a key-value pair." The `Tags` list itself carries
/// no `@length` trait, so this limit is not refused at the boundary and the handlers
/// enforce it.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The prefix an Amazon Web Services tag key must not start with.
///
/// From the modelled doc of `Tag$Key`: "The key can't start with `aws:`" — the prefix AWS
/// reserves for the tags it generates itself. The `TagKey` `@pattern` admits it, so the
/// rule is not refused at the boundary and [`validate_tag_pair`] enforces it for every
/// member that targets that shape. Matched case-insensitively, as the reservation is.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` lies in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace — the keys AWS
/// assigns itself and a client may not write ([`validate_tag_pair`]) or remove.
///
/// Matched case-insensitively, as the reservation is, so the one rule covers every
/// spelling of the prefix.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.to_ascii_lowercase()
        .starts_with(RESERVED_TAG_KEY_PREFIX)
}

/// The AWS-assigned tag key naming the identity that created a resource.
///
/// A resource created through this API carries the provenance AWS stamps on it, and the
/// recorded wire of a tag read reports it ahead of the client's own tags
/// ([`reported_tags`]). It is in the reserved namespace
/// ([`RESERVED_TAG_KEY_PREFIX`]), so no client can write or remove it.
pub const CREATED_BY_TAG_KEY: &str = "aws:createdBy";

/// The AWS-assigned tag key naming WHEN a resource was created — the record's own
/// creation instant, the companion of [`CREATED_BY_TAG_KEY`].
pub const CREATED_DATE_TAG_KEY: &str = "aws:createdDate";

/// The identity a request speaks for, as [`CREATED_BY_TAG_KEY`]'s value names it.
///
/// The colon-separated fields are the kind of principal the request was signed by, the
/// credential it presented and the name that credential speaks for — the three facts a
/// serving service can read off a signed request, resolved through the one rule every
/// service of this emulator resolves a caller with
/// (`::service_common::management_events::recorded_caller`), so two services naming one
/// caller name it alike. A temporary credential speaks for a session whose name this
/// service cannot resolve, and an unsigned request presents no credential at all: the
/// fields that cannot be resolved are left off rather than guessed.
///
/// Read off the SIGNED REQUEST and never off its body: the creator of a resource is who
/// called, not anything the caller asked for. Every field comes from the credential scope
/// the caller itself presented, so the value discloses nothing to that caller it did not
/// send; no secret is read or reported — a key id cannot sign a request.
pub fn request_created_by(parts: &::http::request::Parts) -> String {
    let caller = ::service_common::management_events::recorded_caller(parts);
    match (caller.access_key_id, caller.user_name) {
        (Some(key), Some(name)) => format!("{}:{key}:{name}", caller.principal_type),
        (Some(key), None) => format!("{}:{key}", caller.principal_type),
        (None, _) => caller.principal_type.to_string(),
    }
}

/// WHO created a resource and WHEN: the provenance of the call that minted a record, and
/// the two facts the AWS-assigned system tags report ([`system_tag_list`]).
///
/// One value rather than two parameters, because a creation has one creator and one
/// instant: every writer of this store that mints a resource takes this, so neither fact
/// can be supplied without the other.
#[derive(Debug, Clone)]
pub struct Creation {
    /// The identity the creating request was signed by ([`request_created_by`]).
    pub by: String,
    /// The instant the record is created at — what its `created_time` keeps.
    pub at: SystemTime,
}

/// One activation this service has ISSUED and no create has redeemed yet.
///
/// Keyed in the store by the activation key itself: the key is what a create presents, so
/// it is the identity of the pending activation, and a redeemed key is REMOVED rather than
/// flagged — the activation it named is spent and names nothing to redeem again.
#[derive(Debug, Clone)]
pub struct PendingActivation {
    /// The region the handshake asked for the key in, normalised through [`arn_region`].
    ///
    /// An activation key is issued for ONE region — which is what the handshake's
    /// `activationRegion` names — so a create addressed to another region's endpoint
    /// presents a key that region's service never issued.
    pub region: String,
    /// When the handshake issued the key: the order
    /// [`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::issue_activation_key`]
    /// drops pending keys in once the registry is full.
    pub issued_at: SystemTime,
}

/// One persisted gateway.
///
/// Keyed in the store by [`StoredGateway::arn`]; `gateway_id` is the ARN's resource
/// suffix, kept beside it so a later slice can resolve a gateway by id without
/// re-parsing the ARN.
#[derive(Debug, Clone)]
pub struct StoredGateway {
    /// The gateway's ARN — the store's key and the identity every operation takes.
    pub arn: String,
    /// The generated id in the ARN's `gateway/<id>` suffix.
    pub gateway_id: String,
    /// `GatewayDisplayName`, as last set by create or `UpdateGatewayInformation`.
    pub display_name: String,
    /// `GatewayType`; the model declares only [`BACKUP_VM`].
    pub gateway_type: String,
    /// The activation key the gateway was created with — the one its create REDEEMED
    /// ([`PendingActivation`]). Not part of any response shape — kept because it
    /// identifies the appliance this record stands for.
    pub activation_key: String,
    /// The hypervisor this gateway is associated with, as
    /// [`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::associate_gateway_to_server`]
    /// last set it. `None` means "not associated".
    pub hypervisor_arn: Option<String>,
    /// `HypervisorId` — the `hypervisor/<id>` suffix of [`Self::hypervisor_arn`], absent
    /// until the gateway is associated with a hypervisor.
    pub hypervisor_id: Option<String>,
    /// `LastSeenTime` — when the appliance last communicated with the cloud. Set when
    /// the record is minted and moved only by a call that really contacts the gateway;
    /// a read never advances it.
    pub last_seen_time: SystemTime,
    /// When this record was created.
    pub created_time: SystemTime,
    /// The identity that created this resource, as [`CREATED_BY_TAG_KEY`] names it
    /// ([`request_created_by`]) — read off the request that created the record and kept
    /// on it, so every later read of its tags reports the same creator.
    pub created_by: String,
    /// `SoftwareVersion` of the appliance: the version it is running now, moved only by
    /// an appliance software update whose install has fallen due
    /// ([`Self::software_version_as_of`]).
    pub software_version: String,
    /// The appliance software update `UpdateGatewaySoftwareNow` last triggered, absent
    /// until one is triggered and again once its install has been folded onto
    /// [`Self::software_version`]. `None` means "no update is installing".
    pub software_update: Option<GatewaySoftwareUpdate>,
    /// `VpcEndpoint`, set only for a gateway activated through a VPC endpoint.
    pub vpc_endpoint: Option<String>,
    /// `MaintenanceStartTime` — the appliance's weekly or monthly maintenance window.
    ///
    /// NOT optional: a gateway has a window from the moment it is activated
    /// ([`default_maintenance_start_time`]), and `PutMaintenanceStartTime` replaces it, so
    /// "no window" is not a state a gateway of this service can be in.
    pub maintenance_start_time: MaintenanceStartTime,
    /// The bandwidth rate-limit schedule, empty until
    /// `PutBandwidthRateLimitSchedule` sets one.
    pub bandwidth_rate_limit_intervals: Vec<BandwidthRateLimitInterval>,
    /// The resource's tags, by key. A map, not a list: `TagResource` merges by key and
    /// the same key set twice is one tag.
    pub tags: BTreeMap<String, String>,
}

/// An appliance software update of one gateway, as `UpdateGatewaySoftwareNow` triggered
/// it.
///
/// Kept ON the gateway record rather than in a map beside it: the update is a fact about
/// that appliance, so every reader of the record agrees on which version it is installing
/// and when that install falls due.
#[derive(Debug, Clone)]
pub struct GatewaySoftwareUpdate {
    /// The version the appliance runs once this install is done.
    pub to_version: String,
    /// When the install falls due. Until then the appliance is still running the version
    /// it was running when the update was triggered.
    pub installed_at: SystemTime,
}

impl StoredGateway {
    /// The version the appliance is running at `now`: the version an update whose install
    /// has fallen due left there, and otherwise the one on the record.
    ///
    /// Derived, never written: a read of a gateway reports the version the clock says the
    /// appliance runs without moving an install along, so every reader of one record
    /// agrees and reading twice reports the same thing.
    pub fn software_version_as_of(&self, now: SystemTime) -> String {
        match &self.software_update {
            Some(update) if now >= update.installed_at => update.to_version.clone(),
            _ => self.software_version.clone(),
        }
    }

    /// Fold an install the clock has ALREADY made due onto [`Self::software_version`], so
    /// the version on the record is the one the appliance runs and the update still held
    /// beside it is one still to come.
    ///
    /// Reached only from a MUTATING operation
    /// ([`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::update_gateway_software_now`]);
    /// a read derives the version instead ([`Self::software_version_as_of`]) and writes
    /// nothing.
    fn settle_software_update(&mut self, now: SystemTime) {
        if self
            .software_update
            .as_ref()
            .is_some_and(|update| now >= update.installed_at)
        {
            self.software_version = self.software_version_as_of(now);
            self.software_update = None;
        }
    }

    /// This record as the `GatewayDetails` shape `GetGateway` returns, as of `now`.
    ///
    /// `now` is what decides the `SoftwareVersion` reported: an install the clock has made
    /// due is in effect for every reader at the same instant
    /// ([`Self::software_version_as_of`]).
    ///
    /// `NextUpdateAvailabilityTime` and `DeprecationDate` stay absent: this service
    /// schedules no appliance update of its own — an update happens when a caller asks for
    /// one — and no deprecation date is set for an emulated gateway.
    pub fn details_as_of(&self, now: SystemTime) -> GatewayDetails {
        GatewayDetails {
            gateway_arn: Some(self.arn.clone()),
            gateway_display_name: Some(self.display_name.clone()),
            gateway_type: Some(self.gateway_type.clone()),
            hypervisor_id: self.hypervisor_id.clone(),
            last_seen_time: Some(date_time(self.last_seen_time)),
            maintenance_start_time: Some(self.maintenance_start_time.clone()),
            next_update_availability_time: None,
            vpc_endpoint: self.vpc_endpoint.clone(),
            deprecation_date: None,
            software_version: Some(self.software_version_as_of(now)),
        }
    }

    /// This record as the `Gateway` summary shape `ListGateways` returns.
    pub fn summary(&self) -> Gateway {
        Gateway {
            gateway_arn: Some(self.arn.clone()),
            gateway_display_name: Some(self.display_name.clone()),
            gateway_type: Some(self.gateway_type.clone()),
            hypervisor_id: self.hypervisor_id.clone(),
            last_seen_time: Some(date_time(self.last_seen_time)),
        }
    }

    /// Every tag a read of this resource reports: the AWS-assigned system tags, then the
    /// client's own in key order ([`reported_tags`]).
    pub fn tag_list(&self) -> Vec<Tag> {
        reported_tags(&self.created_by, self.created_time, &self.tags)
    }
}

/// The `SyncMetadataStatus` a sync that started at `started` is in at `now`, and the
/// sentence that states it.
///
/// The one place the spans of a sync are turned into a status, so a report of the status,
/// of its message and of the successful-sync time are all read off the same boundaries.
fn sync_status_as_of(started: SystemTime, now: SystemTime) -> (&'static str, &'static str) {
    if now >= started + METADATA_SYNC_SUCCEEDS_AFTER {
        (SYNC_METADATA_SUCCEEDED, METADATA_SYNC_SUCCEEDED_MESSAGE)
    } else if now >= started + METADATA_SYNC_RUNNING_AFTER {
        (SYNC_METADATA_RUNNING, METADATA_SYNC_RUNNING_MESSAGE)
    } else {
        (SYNC_METADATA_CREATED, METADATA_SYNC_CREATED_MESSAGE)
    }
}

/// A stored tag map as the modelled `Tags` list, in key order. One rendering for every
/// resource this service tags.
fn tag_list(tags: &BTreeMap<String, String>) -> Vec<Tag> {
    tags.iter()
        .map(|(key, value)| Tag {
            key: key.clone(),
            value: value.clone(),
        })
        .collect()
}

/// Every tag a read reports for a resource this service CREATED for a caller: the two
/// AWS-assigned system tags first ([`system_tag_list`]), then the client's own in key
/// order ([`tag_list`]).
///
/// The two namespaces are kept apart in the record — the client's tags are the map
/// `TagResource` merges into and `UntagResource` removes from, and the system pair is
/// derived from the record's own provenance — so a client can neither write nor remove an
/// AWS-assigned key ([`is_reserved_tag_key`]), and the system pair cannot be pushed out of
/// its leading position by a client key that happens to sort below `aws:`.
fn reported_tags(
    created_by: &str,
    created_time: SystemTime,
    tags: &BTreeMap<String, String>,
) -> Vec<Tag> {
    system_tag_list(created_by, created_time)
        .into_iter()
        .chain(tag_list(tags))
        .collect()
}

/// The AWS-assigned tags a resource created through this API carries: who created it
/// ([`CREATED_BY_TAG_KEY`]) and when ([`CREATED_DATE_TAG_KEY`]).
///
/// Both are facts about the call that minted the record, so neither is derived from the
/// request body and neither moves afterwards: a later read of one resource reports the
/// creator and the creation instant the record has always had.
fn system_tag_list(created_by: &str, created_time: SystemTime) -> Vec<Tag> {
    vec![
        Tag {
            key: CREATED_BY_TAG_KEY.to_string(),
            value: created_by.to_string(),
        },
        Tag {
            key: CREATED_DATE_TAG_KEY.to_string(),
            value: created_date(created_time),
        },
    ]
}

/// A creation instant as [`CREATED_DATE_TAG_KEY`]'s value spells it: a tag value is a
/// string, so the instant is rendered as an ISO-8601 `date-time` (RFC 3339, as
/// `aws_smithy_types` writes a `DateTime`), truncated to WHOLE SECONDS — the granularity a
/// creation date is reported at.
///
/// An instant built from whole seconds has no rendering a formatter can refuse, and this
/// runs on a request path, so the unreachable arm answers the empty string rather than
/// panicking and costing every caller the store.
fn created_date(created_time: SystemTime) -> String {
    // Through the crate's one `SystemTime` -> `DateTime` conversion, then truncated: `secs`
    // is the whole-second part of that instant.
    ::aws_smithy_types::DateTime::from_secs(date_time(created_time).secs())
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// One persisted hypervisor.
///
/// Keyed in the store by [`StoredHypervisor::arn`], the identity every hypervisor
/// operation takes (`GetHypervisor`, `ListHypervisors`, `UpdateHypervisor`,
/// `DeleteHypervisor` all name or return a `ServerArn`). The members are exactly the
/// ones `HypervisorDetails` reports
/// (`smithy_query(query="shape",shape="HypervisorDetails")`), so a read serialises the
/// record rather than recomputing anything from the request — the hypervisor's
/// credentials are NOT among them and are not kept here.
#[derive(Debug, Clone)]
pub struct StoredHypervisor {
    /// The hypervisor's ARN — the store's key. The generated id is its
    /// `hypervisor/<id>` suffix and is not kept beside it: one source of truth for the
    /// identity.
    pub arn: String,
    /// `HypervisorDetails$Name`.
    pub name: String,
    /// `HypervisorDetails$Host` — an IP address or an FQDN.
    pub host: String,
    /// `HypervisorDetails$KmsKeyArn`, absent unless the import named one.
    pub kms_key_arn: Option<String>,
    /// `HypervisorDetails$LogGroupArn`, absent until `UpdateHypervisor` sets one.
    pub log_group_arn: Option<String>,
    /// `HypervisorDetails$State` as last PERSISTED; what a read reports is
    /// [`StoredHypervisor::state_as_of`], which also accounts for the activation the
    /// clock has already made due.
    pub state: String,
    /// When this record's [`HYPERVISOR_PENDING`] state becomes [`HYPERVISOR_ONLINE`],
    /// or `None` for a record whose state no longer moves on its own.
    pub online_due: Option<SystemTime>,
    /// `LastSuccessfulMetadataSyncTime`, absent until a metadata sync has succeeded.
    pub last_successful_metadata_sync_time: Option<SystemTime>,
    /// `LatestMetadataSyncStatus` — a `SyncMetadataStatus` value — absent until a
    /// metadata sync has been started.
    pub latest_metadata_sync_status: Option<String>,
    /// `LatestMetadataSyncStatusMessage`.
    pub latest_metadata_sync_status_message: Option<String>,
    /// When this record was created.
    pub created_time: SystemTime,
    /// The identity that created this resource — the same fact
    /// [`StoredGateway::created_by`] keeps, read off the request that imported the record
    /// ([`request_created_by`]).
    pub created_by: String,
    /// The resource's tags, by key — the same representation [`StoredGateway::tags`]
    /// uses, so the tagging operations treat both resources alike.
    pub tags: BTreeMap<String, String>,
    /// When the metadata sync last started for this hypervisor was REQUESTED, and `None`
    /// for a record no sync has been started on.
    ///
    /// `StartVirtualMachinesMetadataSync` stores this instant and nothing else about the
    /// sync's progress: the clock alone carries the sync from
    /// [`SYNC_METADATA_CREATED`] through [`SYNC_METADATA_RUNNING`] to
    /// [`SYNC_METADATA_SUCCEEDED`], exactly as `online_due` carries the hypervisor's own
    /// state, so what every reader reports is a function of this record and the current
    /// time and no read has to poll for a transition to happen.
    pub metadata_sync_started: Option<SystemTime>,
    /// The hypervisor's property mappings, absent until they have been SET for it.
    ///
    /// An import never mints mappings: `PutHypervisorPropertyMappings` is the operation
    /// that writes them ([`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::set_property_mappings`]),
    /// and `GetHypervisorPropertyMappings` reports whatever this field holds. Absent is a
    /// meaningful value and not an empty mapping set: a hypervisor with no mappings has
    /// no mappings resource to report ([`MissingResource::PropertyMappings`]), where one
    /// whose mappings are an empty list has.
    pub property_mappings: Option<StoredPropertyMappings>,
}

impl StoredHypervisor {
    /// The `HypervisorState` this hypervisor is in at `now`.
    ///
    /// An import leaves the hypervisor [`HYPERVISOR_PENDING`] with the instant its
    /// activation falls due; the clock alone moves it to [`HYPERVISOR_ONLINE`], so the
    /// state a read reports is a function of the record and the current time and is the
    /// same for every reader — no call has to poll for the transition to happen, and a
    /// read does not cause it.
    pub fn state_as_of(&self, now: SystemTime) -> &str {
        if self.state == HYPERVISOR_PENDING && self.online_due.is_some_and(|due| now >= due) {
            HYPERVISOR_ONLINE
        } else {
            &self.state
        }
    }

    /// The `LatestMetadataSyncStatus` this record reports at `now`.
    ///
    /// For a record with a sync in flight ([`Self::metadata_sync_started`]) the status is
    /// the one the clock has reached since that instant; for every other record it is
    /// whatever was last PERSISTED, so a status no sync of this service started (a
    /// `FAILED` or `PARTIALLY_FAILED` one, say) is reported as stored.
    pub fn latest_metadata_sync_status_as_of(&self, now: SystemTime) -> Option<String> {
        match self.metadata_sync_started {
            Some(started) => Some(sync_status_as_of(started, now).0.to_string()),
            None => self.latest_metadata_sync_status.clone(),
        }
    }

    /// The `LatestMetadataSyncStatusMessage` this record reports at `now` — the sentence
    /// of the status [`Self::latest_metadata_sync_status_as_of`] reports, so the two
    /// cannot disagree.
    pub fn latest_metadata_sync_status_message_as_of(&self, now: SystemTime) -> Option<String> {
        match self.metadata_sync_started {
            Some(started) => Some(sync_status_as_of(started, now).1.to_string()),
            None => self.latest_metadata_sync_status_message.clone(),
        }
    }

    /// The `LastSuccessfulMetadataSyncTime` this record reports at `now` — "the time when
    /// the most recent successful sync of metadata occurred".
    ///
    /// A sync in flight dates its success at the instant it falls due
    /// ([`METADATA_SYNC_SUCCEEDS_AFTER`] after it started), so the time a read reports is
    /// stable however often it is read, and until that instant the previous successful
    /// sync's time stands: a sync that has not finished has not succeeded.
    pub fn last_successful_metadata_sync_time_as_of(&self, now: SystemTime) -> Option<SystemTime> {
        match self.metadata_sync_started {
            Some(started) if now >= started + METADATA_SYNC_SUCCEEDS_AFTER => {
                Some(started + METADATA_SYNC_SUCCEEDS_AFTER)
            }
            _ => self.last_successful_metadata_sync_time,
        }
    }

    /// This record as the `HypervisorDetails` shape `GetHypervisor` returns, as of `now`.
    pub fn details_as_of(&self, now: SystemTime) -> HypervisorDetails {
        HypervisorDetails {
            host: Some(self.host.clone()),
            hypervisor_arn: Some(self.arn.clone()),
            kms_key_arn: self.kms_key_arn.clone(),
            name: Some(self.name.clone()),
            log_group_arn: self.log_group_arn.clone(),
            state: Some(self.state_as_of(now).to_string()),
            last_successful_metadata_sync_time: self
                .last_successful_metadata_sync_time_as_of(now)
                .map(date_time),
            latest_metadata_sync_status_message: self
                .latest_metadata_sync_status_message_as_of(now),
            latest_metadata_sync_status: self.latest_metadata_sync_status_as_of(now),
        }
    }

    /// This record as the `Hypervisor` summary shape `ListHypervisors` returns, as of
    /// `now`.
    ///
    /// The summary is the subset of [`Self::details_as_of`] the `Hypervisor` shape
    /// declares (`smithy_query(query="shape",shape="Hypervisor")`: `Host`,
    /// `HypervisorArn`, `KmsKeyArn`, `Name`, `State`) — read off the same record and the
    /// same clock, so a listing and a get cannot disagree about one hypervisor.
    pub fn summary_as_of(&self, now: SystemTime) -> Hypervisor {
        Hypervisor {
            host: Some(self.host.clone()),
            hypervisor_arn: Some(self.arn.clone()),
            kms_key_arn: self.kms_key_arn.clone(),
            name: Some(self.name.clone()),
            state: Some(self.state_as_of(now).to_string()),
        }
    }

    /// Every tag a read of this resource reports: the AWS-assigned system tags, then the
    /// client's own in key order ([`reported_tags`]) — the same rendering
    /// [`StoredGateway::tag_list`] answers with, so the two resource types report their
    /// tags alike.
    pub fn tag_list(&self) -> Vec<Tag> {
        reported_tags(&self.created_by, self.created_time, &self.tags)
    }
}

/// One persisted virtual machine.
///
/// Keyed in the store by [`StoredVirtualMachine::arn`] — the `ResourceArn` the modelled
/// `VirtualMachine` reports and `GetVirtualMachine` takes. The members are exactly the
/// ones `VirtualMachine` reports (`smithy_query(query="shape",shape="VirtualMachine")`)
/// plus the ARN of the hypervisor the machine runs on: a virtual machine belongs to its
/// hypervisor (`VirtualMachine$HypervisorId` is "the ID of the virtual machine's
/// hypervisor"), which is what `ListVirtualMachines` filters on, and the `HypervisorId` a
/// read reports is that ARN's resource id rather than a second copy of it.
#[derive(Debug, Clone)]
pub struct StoredVirtualMachine {
    /// The machine's ARN — the store's key.
    pub arn: String,
    /// `VirtualMachine$Name`.
    pub name: String,
    /// `VirtualMachine$HostName`.
    pub host_name: String,
    /// `VirtualMachine$Path` — where the machine sits in the hypervisor's inventory, and
    /// the machine's identity ON that hypervisor: see
    /// [`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::record_virtual_machine`].
    pub path: String,
    /// The ARN of the hypervisor this machine runs on — a key of the `hypervisors` map.
    pub hypervisor_arn: String,
    /// `VirtualMachine$LastBackupDate`, absent for a machine that has never been backed
    /// up.
    pub last_backup_date: Option<SystemTime>,
    /// The VMware tags attached to this machine, in the order its hypervisor's inventory
    /// reported them.
    ///
    /// `VirtualMachineDetails$VmwareTags` — the one member
    /// `VirtualMachineDetails` carries that the `VirtualMachine` summary does not
    /// (`smithy_query(query="shape",shape="VirtualMachineDetails")`), so only a read of a
    /// single machine reports them. A tag is attached to a machine on the hypervisor, not
    /// through this API, so it reaches the store on the same out-of-band inventory channel
    /// as the rest of the record ([`VirtualMachineInventory`]), and is kept as the
    /// MODELLED [`VmwareTag`] shape so a member cannot drift between what is stored and
    /// what is serialised. Empty for a machine whose inventory reported none.
    pub vmware_tags: Vec<VmwareTag>,
    /// The resource's AWS tags, by key — the same representation [`StoredGateway::tags`]
    /// and [`StoredHypervisor::tags`] use, so the tagging operations treat every resource
    /// of this service alike.
    ///
    /// Distinct from [`Self::vmware_tags`], which are the tags the machine carries ON its
    /// hypervisor and reach the store with the rest of the inventory: these are the tags
    /// `TagResource` writes through this API, and no inventory report sets or clears them.
    pub tags: BTreeMap<String, String>,
    /// When the sync that discovered this machine REPORTED it — the instant that sync
    /// fell due, which is also the instant it is reported as having succeeded at. Kept
    /// across a re-sync of the same machine ONCE it has fallen due, so re-reporting an
    /// unchanged inventory is visible as the one record it is; a machine whose reporting
    /// sync was superseded before that instant is dated by the sync that supersedes it
    /// instead
    /// ([`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::start_metadata_sync`]),
    /// because the superseded sync never succeeds. No member of the modelled
    /// `VirtualMachine` carries it: it is the record's own provenance, and what a read
    /// compares the clock against to decide whether the machine is part of the inventory
    /// yet ([`Self::discovered_as_of`]).
    pub discovered_time: SystemTime,
}

impl StoredVirtualMachine {
    /// Whether a read AT `now` reports this machine as part of its hypervisor's inventory.
    ///
    /// A sync has read the inventory once it falls due, so a machine it reported is part
    /// of that inventory from [`Self::discovered_time`] on — the same instant the
    /// hypervisor's own [`StoredHypervisor::latest_metadata_sync_status_as_of`] turns
    /// [`SYNC_METADATA_SUCCEEDED`]. What a listing reports is therefore a function of the
    /// records and the clock, exactly as a hypervisor's state is: no read moves a sync
    /// along, and a caller listing straight after starting the first sync of a hypervisor
    /// still sees the inventory it had before — none.
    pub fn discovered_as_of(&self, now: SystemTime) -> bool {
        now >= self.discovered_time
    }

    /// The persisted AWS tags as the modelled `Tags` list, in key order.
    ///
    /// No AWS-assigned system tag ([`system_tag_list`]) among them, unlike the two
    /// resource types a REQUEST creates: no operation of this model creates a virtual
    /// machine, so no caller created this record and there is no creating identity to
    /// report. It reaches the service as part of the inventory a metadata sync reads off
    /// the hypervisor it runs on ([`StoredVirtualMachine::discovered_time`]).
    pub fn tag_list(&self) -> Vec<Tag> {
        tag_list(&self.tags)
    }

    /// This record as the `VirtualMachine` shape `ListVirtualMachines` returns.
    ///
    /// `HypervisorId` is read off the stored hypervisor ARN
    /// ([`arn_resource_id`]) — the `hypervisor/<id>` suffix — so the id a listing reports
    /// and the hypervisor the record belongs to cannot drift apart.
    pub fn summary(&self) -> VirtualMachine {
        VirtualMachine {
            host_name: Some(self.host_name.clone()),
            hypervisor_id: arn_resource_id(&self.hypervisor_arn).map(str::to_string),
            name: Some(self.name.clone()),
            path: Some(self.path.clone()),
            resource_arn: Some(self.arn.clone()),
            last_backup_date: self.last_backup_date.map(date_time),
        }
    }

    /// This record as the `VirtualMachineDetails` shape `GetVirtualMachine` returns.
    ///
    /// Every member the `VirtualMachine` summary also carries is taken FROM that summary,
    /// so a read of one machine and a listing that holds it cannot disagree about it.
    /// `VmwareTags` is the one member `VirtualMachineDetails` adds
    /// (`smithy_query(query="shape",shape="VirtualMachineDetails")`): the tags the record
    /// holds, in the order its inventory reported them, and an EMPTY list — not an absent
    /// member — for a machine that carries none, because the store knows that it has none
    /// where an absent member would say it does not know.
    pub fn details(&self) -> VirtualMachineDetails {
        let summary = self.summary();
        VirtualMachineDetails {
            host_name: summary.host_name,
            hypervisor_id: summary.hypervisor_id,
            name: summary.name,
            path: summary.path,
            resource_arn: summary.resource_arn,
            last_backup_date: summary.last_backup_date,
            vmware_tags: Some(self.vmware_tags.clone()),
        }
    }
}

/// What a hypervisor's inventory says about one virtual machine: the members a read of a
/// machine reports, as the sync that discovered it found them.
///
/// Not a request shape — no operation of this model carries one. It is the argument of
/// [`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::record_virtual_machine`],
/// the channel a hypervisor's inventory reaches this service through when its metadata is
/// synced ([`synced_inventory`]).
#[derive(Debug, Clone, Default)]
pub struct VirtualMachineInventory {
    /// `VirtualMachine$Name`.
    pub name: String,
    /// `VirtualMachine$HostName`.
    pub host_name: String,
    /// `VirtualMachine$Path`.
    pub path: String,
    /// `VirtualMachine$LastBackupDate`, absent for a machine never backed up.
    pub last_backup_date: Option<SystemTime>,
    /// `VirtualMachineDetails$VmwareTags` — the VMware tags attached to the machine on
    /// its hypervisor, empty for a machine that carries none.
    pub vmware_tags: Vec<VmwareTag>,
}

/// The inventory the hypervisor stored under `hypervisor_arn` reports when its metadata is
/// synced, in the order the sync reads it.
///
/// This emulator stands in for the on-premises hypervisor as much as for the service, and
/// it has no VMware host to enumerate — so the inventory is derived from the IDENTITY of
/// the hypervisor record itself: the WHOLE ARN this store minted for it, which no request
/// chose. Whole, because the ARN is a record's only identity and every segment of it
/// carries part of that identity: the resource id behind it is minted per (kind, region)
/// ([`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::mint_resource`]), so the
/// first hypervisor of every region shares one, and a derivation over that segment alone
/// would publish one set of machines for all of them. Two consequences, both of them the
/// point: every sync of one hypervisor reports exactly the machines the previous sync did,
/// so a re-sync re-finds each machine by its [`StoredVirtualMachine::path`] and updates the
/// record it already has rather than minting a second one; and no two hypervisors —
/// whichever regions they were imported in — report the same machine.
///
/// `LastBackupDate` is absent on every machine: a virtual machine is backed up by a backup
/// plan outside this model, and no operation here records one, so a machine this service
/// has only just discovered has never been backed up as far as it knows.
/// `VmwareTags` is empty for the same kind of reason — a VMware tag is attached to a
/// machine ON the hypervisor, and the hypervisor this emulator stands in for attaches
/// none.
pub fn synced_inventory(hypervisor_arn: &str) -> Vec<VirtualMachineInventory> {
    (1..=SYNCED_VIRTUAL_MACHINES)
        .map(|ordinal| {
            let name = synced_machine_name(hypervisor_arn, ordinal);
            VirtualMachineInventory {
                path: format!("{SYNCED_INVENTORY_FOLDER}/{name}"),
                // `VirtualMachine$HostName` targets the same `Name` shape as the machine's
                // own name, whose pattern `^[a-zA-Z0-9-]*$` admits no dots — so no
                // qualified DNS name can be spelled there, and the emulated guest answers
                // to its own name.
                host_name: name.clone(),
                name,
                last_backup_date: None,
                vmware_tags: Vec::new(),
            }
        })
        .collect()
}

/// The `VirtualMachine$Name` a sync reports for the `ordinal`-th machine of the hypervisor
/// stored under `hypervisor_arn`.
///
/// Derived from the WHOLE ARN: the fingerprint of it ([`hypervisor_fingerprint`]) is what
/// makes the name distinct for every distinct record, and the resource-id segment is kept
/// in front of that fingerprint because it is the part of the identity a client sees
/// elsewhere (`VirtualMachine$HypervisorId`), which is what makes a machine's name
/// readable as a machine OF that hypervisor.
///
/// Every character comes from that resource id, which an ARN segment restricts to
/// `[a-zA-Z-0-9]`, from the hex fingerprint, or from the ordinal — exactly what the
/// member's `^[a-zA-Z0-9-]*$` pattern accepts — and the id, the only unbounded part, is
/// clipped so the name stays inside [`MAX_VIRTUAL_MACHINE_NAME`] however long the ARN is.
/// The clip never touches the fingerprint, so clipping cannot bring two records' names
/// back together.
fn synced_machine_name(hypervisor_arn: &str, ordinal: usize) -> String {
    // A record of this store is always keyed by an ARN minted here, so it always has a
    // resource id; the resource type stands in for anything else, keeping the reported
    // name inside the modelled pattern whatever the ARN spells.
    let hypervisor_id = arn_resource_id(hypervisor_arn).unwrap_or(HYPERVISOR_RESOURCE_TYPE);
    let suffix = format!("-{}-vm-{ordinal}", hypervisor_fingerprint(hypervisor_arn));
    let head: String = hypervisor_id
        .chars()
        .take(MAX_VIRTUAL_MACHINE_NAME.saturating_sub(suffix.len()))
        .collect();
    format!("{head}{suffix}")
}

/// The fingerprint of the identity of the hypervisor record stored under `hypervisor_arn`:
/// the first [`HYPERVISOR_FINGERPRINT_DIGITS`] hex digits of the SHA-256 digest of the
/// whole ARN.
///
/// A pure function of that one string, so the inventory [`synced_inventory`] derives from it
/// is the same at every sync of one record (what makes a re-sync idempotent) and differs
/// for any two records, which differ in at least one ARN segment — including two records of
/// two regions, whose resource ids are minted from per-region sequences and so collide.
fn hypervisor_fingerprint(hypervisor_arn: &str) -> String {
    let digest = Sha256::digest(hypervisor_arn.as_bytes());
    hex::encode(&digest[..HYPERVISOR_FINGERPRINT_DIGITS / 2])
}

/// The property mappings SET for one hypervisor.
///
/// The members are exactly the ones `GetHypervisorPropertyMappings` reports beside the
/// `HypervisorArn` (`smithy_query(query="operation",
/// operation="GetHypervisorPropertyMappings")`: `IamRoleArn`, `VmwareToAwsTagMappings`),
/// kept as the MODELLED [`VmwareToAwsTagMapping`] shape so a member cannot drift between
/// what is stored and what is serialised, and in the order they were set — a list, not a
/// set, is what the model declares.
#[derive(Debug, Clone)]
pub struct StoredPropertyMappings {
    /// `IamRoleArn` — the role the mappings are applied with.
    pub iam_role_arn: String,
    /// `VmwareToAwsTagMappings`, in the order they were set.
    pub vmware_to_aws_tag_mappings: Vec<VmwareToAwsTagMapping>,
}

/// What an import asks for a hypervisor to be created with: the members
/// `ImportHypervisorConfiguration` takes that a later read reports
/// (`smithy_query(query="operation",operation="ImportHypervisorConfiguration")`). Its
/// `Username` / `Password` are `@sensitive` credentials no response member carries, so
/// they are deliberately absent here and never persisted.
#[derive(Debug, Clone, Default)]
pub struct HypervisorConfiguration {
    /// `Name` — required by the import's input.
    pub name: String,
    /// `Host` — required by the import's input.
    pub host: String,
    /// `KmsKeyArn`, optional.
    pub kms_key_arn: Option<String>,
    /// `Tags` — "the tags of the hypervisor configuration to import" — by key, as
    /// [`tag_map`] validated them.
    pub tags: BTreeMap<String, String>,
}

/// What an update asks a stored hypervisor to BECOME: the members `UpdateHypervisor`
/// takes that a later read reports
/// (`smithy_query(query="operation",operation="UpdateHypervisor")`).
///
/// Every member is optional on the wire and optional here, and `None` means "the request
/// did not carry it": an update applies only what it states and leaves the rest of the
/// record exactly as the previous write left it.
///
/// Its `Username` / `Password` are `@sensitive` credentials no response member carries,
/// so — as in [`HypervisorConfiguration`] — they are deliberately absent here and never
/// persisted.
#[derive(Debug, Clone, Default)]
pub struct HypervisorUpdate {
    /// `Name`, or `None` to leave the stored name alone.
    pub name: Option<String>,
    /// `Host`, or `None` to leave the stored host alone.
    pub host: Option<String>,
    /// `LogGroupArn`, or `None` to leave the stored log group alone.
    ///
    /// The member's pattern is `^$|^arn:…:log-group:…:\*$`, so the EMPTY string is a
    /// value the model accepts and the only way a caller can state "no log group": it
    /// detaches whatever the record carried, which is why an empty value is not the same
    /// as an absent one.
    pub log_group_arn: Option<String>,
}

/// One page of a list operation: the records it returns and the token that resumes
/// after them, present only while more records remain.
///
/// One shape for every paginated listing of this service, so a page boundary means the
/// same thing whichever resource is being listed.
#[derive(Debug, Default)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_token: Option<String>,
}

/// The kinds of resource this store MINTS an ARN for: the three resource types of this
/// service's ARNs, as the `gateway`, `hypervisor` and `vm` of
/// `arn:aws:backup-gateway:<region>:<account>:<type>/<id>`.
///
/// A kind rather than the resource-type string wherever the store has to pick the mint
/// sequence or the id prefix that goes with it, so the mapping is exhaustive at compile
/// time and no caller can ask for a type this store does not mint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MintedResource {
    Gateway,
    Hypervisor,
    VirtualMachine,
}

impl MintedResource {
    /// The kind `resource_type` names, or `None` for a resource type this store mints no
    /// ARN for.
    fn for_resource_type(resource_type: &str) -> Option<Self> {
        match resource_type {
            GATEWAY_RESOURCE_TYPE => Some(Self::Gateway),
            HYPERVISOR_RESOURCE_TYPE => Some(Self::Hypervisor),
            VIRTUAL_MACHINE_RESOURCE_TYPE => Some(Self::VirtualMachine),
            _ => None,
        }
    }

    /// The resource-type segment of an ARN of this kind.
    fn resource_type(self) -> &'static str {
        match self {
            Self::Gateway => GATEWAY_RESOURCE_TYPE,
            Self::Hypervisor => HYPERVISOR_RESOURCE_TYPE,
            Self::VirtualMachine => VIRTUAL_MACHINE_RESOURCE_TYPE,
        }
    }

    /// The id prefix this store mints an id of this kind under.
    fn id_prefix(self) -> &'static str {
        match self {
            Self::Gateway => GATEWAY_ID_PREFIX,
            Self::Hypervisor => HYPERVISOR_ID_PREFIX,
            Self::VirtualMachine => VIRTUAL_MACHINE_ID_PREFIX,
        }
    }

    /// How many hex digits the BODY of an id of this kind carries — the eight of
    /// `hype-1a2b3c4d`, the sixteen of `bgw-0000000000000000`.
    ///
    /// A property of the KIND, beside its prefix: each resource this service names has
    /// its own identifier form on the wire, so a kind's width is asked of the kind rather
    /// than being one number every mint of this store shares. A sequence too large for
    /// that width simply renders wider, so the width pads an id, it never truncates one
    /// and no two sequences of a kind can collide.
    fn id_body_digits(self) -> usize {
        match self {
            Self::Gateway => GATEWAY_ID_DIGITS,
            Self::Hypervisor => HYPERVISOR_ID_DIGITS,
            Self::VirtualMachine => VIRTUAL_MACHINE_ID_DIGITS,
        }
    }
}

/// Per-service state for com-amazonaws-backupgateway-backuponpremises-v20210101. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsBackupgatewayBackuponpremisesV20210101State {
    /// Gateways by ARN. A `BTreeMap` because `ListGateways` paginates over it: the key
    /// order is total and stable, so a page boundary means the same thing on every call
    /// and a token can name the key it resumes after.
    gateways: BTreeMap<String, StoredGateway>,
    /// The next sequence to mint for each (resource kind, region) pair — the number behind
    /// the `<kind>/<id>` ARN suffix.
    ///
    /// Monotonic per PAIR, so every minted ARN is unique within a run and the sequence a
    /// test sees is reproducible — a pair this registry holds no entry for starts from its
    /// kind's `minted_sequence_floors` entry, so dropping an entry to bound the registry
    /// does not restart one. Per pair rather than one counter per kind because every
    /// resource of this service is REGIONAL while one store serves every region a client
    /// addresses: a sequence is minted in exactly one region, so a counter scoped to the
    /// region is what lets
    /// [`Self::is_minted_resource_arn`] tell an ARN a region's own listing handed out from
    /// one another region's listing did — a global counter answers "minted somewhere",
    /// which is not the question a page boundary asks. Bounded by
    /// [`MAX_MINT_SEQUENCES`](crate::state::MAX_MINT_SEQUENCES).
    next_resource_ids: BTreeMap<(MintedResource, String), u64>,
    /// The lowest sequence a (kind, region) pair with NO entry in `next_resource_ids` may
    /// mint next, per kind: the high-water mark of the sequences
    /// [`Self::bound_mint_sequences`] has dropped.
    ///
    /// Bounding the registry may drop a pair's entry, but it must not un-spend the
    /// identities that entry stands for — an id this store handed out is handed out once in
    /// a run — so a dropped sequence is folded in here and a pair with no entry mints, and
    /// counts as having minted ([`Self::next_resource_id`]), from this floor upward. Per
    /// KIND rather than per pair because a per-pair record is the very thing the bound
    /// drops: one floor at or above every sequence dropped for that kind in ANY region is
    /// above what the dropped pair itself reached, which is what a region needs to resume
    /// minting without repeating itself. At most one `u64` per kind, so the whole registry
    /// stays inside [`MAX_MINT_SEQUENCES`](crate::state::MAX_MINT_SEQUENCES).
    minted_sequence_floors: BTreeMap<MintedResource, u64>,
    /// The activation keys this service has issued and no create has redeemed, by key.
    ///
    /// The registry `CreateGateway` resolves its `ActivationKey` against: the activation
    /// handshake of a deployed appliance writes it ([`Self::issue_activation_key`]) and the
    /// create that activates that appliance consumes its entry
    /// ([`Self::redeem_activation_key`]), so a key is redeemable exactly once and a key
    /// nothing issued is redeemable never.
    ///
    /// A `BTreeMap` so the pending set has a total, stable order — the order the oldest
    /// entry is found in when the registry is full.
    pending_activations: BTreeMap<String, PendingActivation>,
    /// Monotonic counter behind an issued activation key, so every key this service hands
    /// out is unique within a run and the sequence is reproducible — the same reason
    /// `next_resource_ids` sits behind the `<kind>/<id>` ARN suffix.
    next_activation_id: u64,
    /// Hypervisors by ARN. A `BTreeMap` for the same reason as `gateways`: the key order
    /// is total and stable, and it is the order `ListHypervisors` reports them in —
    /// "ordered by their Amazon Resource Names (ARNs)"
    /// (`smithy_query(query="operation",operation="ListHypervisors")`) — so a page
    /// boundary means the same thing on every call and a token can name the key it
    /// resumes after.
    hypervisors: BTreeMap<String, StoredHypervisor>,
    /// Virtual machines by ARN. A `BTreeMap` for the same reason as `hypervisors`: the
    /// key order is total and stable, and it is the order `ListVirtualMachines` reports
    /// them in — "a list of your VirtualMachine objects, ordered by their Amazon Resource
    /// Names (ARNs)" (`smithy_query(query="operation",operation="ListVirtualMachines")`).
    ///
    /// One map for every hypervisor's machines, not one per hypervisor: the listing's
    /// `HypervisorArn` is a FILTER on an otherwise whole-account listing (the member is
    /// optional), so both listings page over the same order.
    virtual_machines: BTreeMap<String, StoredVirtualMachine>,
}

impl ComAmazonawsBackupgatewayBackuponpremisesV20210101State {
    /// Issue an activation key for `region`, persist it as pending, and hand it back.
    ///
    /// This is the service half of the activation handshake a deployed appliance makes
    /// before anybody creates a gateway: the key is MINTED here — never taken from a
    /// request — and `CreateGateway` serves only a key it finds in this registry, so the
    /// create really redeems an activation instead of accepting whatever string it was
    /// handed.
    ///
    /// The key is minted from this store's own counter, rendered as the hyphen-separated
    /// groups an appliance prints ([`ACTIVATION_KEY_GROUPS`]) — the same way every ARN
    /// suffix of this store is minted, so a key is unique within a run and the sequence a
    /// test sees is reproducible. What makes a key redeemable is this REGISTRY, not how the
    /// key is spelled, so nothing rests on it being unguessable.
    ///
    /// Once the registry holds [`MAX_PENDING_ACTIVATION_KEYS`], the key pending LONGEST is
    /// dropped to make room: the handshake is unsigned, so the bound cannot be a refusal
    /// that would let one caller stop every other appliance from activating.
    pub fn issue_activation_key(&mut self, region: &str, now: SystemTime) -> String {
        while self.pending_activations.len() >= MAX_PENDING_ACTIVATION_KEYS {
            let Some(oldest) = self
                .pending_activations
                .iter()
                .min_by(|(left_key, left), (right_key, right)| {
                    (left.issued_at, left_key.as_str()).cmp(&(right.issued_at, right_key.as_str()))
                })
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.pending_activations.remove(&oldest);
        }
        let key = minted_activation_key(self.next_activation_id);
        self.next_activation_id += 1;
        self.pending_activations.insert(
            key.clone(),
            PendingActivation {
                region: arn_region(region),
                issued_at: now,
            },
        );
        key
    }

    /// Whether `key` names an activation this service issued for `region` and no create has
    /// redeemed — read-only, so the caller can refuse before anything is written.
    pub fn activation_key_is_redeemable(&self, key: &str, region: &str) -> bool {
        self.pending_activations
            .get(key)
            .is_some_and(|pending| pending.region == arn_region(region))
    }

    /// Redeem `key` for `region`, removing the activation it names.
    ///
    /// `false` — nothing removed — for a key this service never issued, one already
    /// redeemed, or one issued for another region: the caller refuses, and the registry is
    /// left as it was.
    pub fn redeem_activation_key(&mut self, key: &str, region: &str) -> bool {
        if !self.activation_key_is_redeemable(key, region) {
            return false;
        }
        self.pending_activations.remove(key).is_some()
    }

    /// How many issued activation keys are still pending redemption.
    pub fn pending_activation_count(&self) -> usize {
        self.pending_activations.len()
    }

    /// Mint and persist a gateway, and hand back the record as it is now STORED —
    /// read out of the map after the insert, so the ARN a caller gets is the key the
    /// store holds rather than anything derived from the request.
    pub fn create_gateway(
        &mut self,
        region: &str,
        activation_key: &str,
        display_name: &str,
        gateway_type: &str,
        tags: BTreeMap<String, String>,
        creation: Creation,
    ) -> StoredGateway {
        let (gateway_id, arn) = self.mint_resource(MintedResource::Gateway, region);
        let record = StoredGateway {
            arn: arn.clone(),
            gateway_id,
            display_name: display_name.to_string(),
            gateway_type: gateway_type.to_string(),
            activation_key: activation_key.to_string(),
            hypervisor_arn: None,
            hypervisor_id: None,
            last_seen_time: creation.at,
            created_time: creation.at,
            created_by: creation.by,
            software_version: GATEWAY_SOFTWARE_VERSION.to_string(),
            software_update: None,
            vpc_endpoint: None,
            // Activating the appliance is what gives it its maintenance window, so the
            // record carries one before any put: see `default_maintenance_start_time`.
            maintenance_start_time: default_maintenance_start_time(),
            bandwidth_rate_limit_intervals: Vec::new(),
            tags,
        };
        // `or_insert` hands back a reference INTO the map, so the record returned is
        // the one the store now holds; the id counter makes the key fresh, so this
        // always inserts.
        self.gateways.entry(arn).or_insert(record).clone()
    }

    /// The gateway stored under `arn`, if any.
    pub fn gateway(&self, arn: &str) -> Option<&StoredGateway> {
        self.gateways.get(arn)
    }

    /// The gateway stored under `arn`, for in-place mutation.
    pub fn gateway_mut(&mut self, arn: &str) -> Option<&mut StoredGateway> {
        self.gateways.get_mut(arn)
    }

    /// Remove the gateway stored under `arn`, returning the record that was removed.
    pub fn remove_gateway(&mut self, arn: &str) -> Option<StoredGateway> {
        self.gateways.remove(arn)
    }

    /// How many gateways are stored.
    pub fn gateway_count(&self) -> usize {
        self.gateways.len()
    }

    /// One page of the gateways of `region`, in key order.
    ///
    /// `region` is the region the listing request was addressed to, and only the gateways
    /// minted in it are part of the listing ([`page_of_in_region`]): a gateway is a
    /// regional resource, so one of another region is no more part of this listing than a
    /// gateway of another account would be.
    ///
    /// `after` is the ARN the previous page ended on (resolved from a `NextToken`), and
    /// the page starts at the next key AFTER it — so a record created, or deleted,
    /// between two pages changes what the second page holds without ever repeating or
    /// skipping a record that stayed put. `limit` is the caller's `MaxResults` already
    /// resolved by [`page_limit`], so no request can make one response carry the whole
    /// store.
    pub fn page_of_gateways(
        &self,
        region: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Page<StoredGateway> {
        page_of_in_region(&self.gateways, region, after, limit)
    }

    /// Set the bandwidth rate-limit schedule of the gateway stored under `arn`, and hand
    /// back the record as it is now STORED — read out of the map after the write, so what
    /// a caller gets is what the store holds.
    ///
    /// `None` — nothing written — when this store holds no gateway under `arn`.
    ///
    /// The intervals REPLACE whatever that gateway carried: the request states the whole
    /// schedule the gateway is to have, so setting the same schedule again leaves the
    /// record exactly as one call left it, no interval of an earlier call survives a later
    /// one, and an empty list is a schedule of no intervals — the state a gateway that has
    /// never had one is in, which is "no bandwidth rate limiting in effect". The stored
    /// ORDER is the order the request listed them in, which is the order a read reports.
    /// Nothing else on the record is touched: setting a schedule is not a maintenance
    /// window and does not move the gateway's other members.
    ///
    /// Whether the intervals can HOLD TOGETHER is settled before this is reached — see
    /// [`bandwidth_rate_limit_schedule_violation`], the refusal the handler answers with —
    /// so this writes the schedule it is given.
    pub fn set_bandwidth_rate_limit_schedule(
        &mut self,
        arn: &str,
        intervals: Vec<BandwidthRateLimitInterval>,
    ) -> Option<StoredGateway> {
        self.gateways.get_mut(arn)?.bandwidth_rate_limit_intervals = intervals;
        self.gateways.get(arn).cloned()
    }

    /// Trigger the appliance software update of the gateway stored under `arn`, and hand
    /// back the record as it is now STORED — read out of the map after the write, so the
    /// ARN a caller gets is the key the store holds.
    ///
    /// `None` — nothing written — when this store holds no gateway under `arn`.
    ///
    /// The update is triggered at once and INSTALLS when
    /// [`GATEWAY_SOFTWARE_UPDATE_INSTALLS_AFTER`] has passed, which is when a read starts
    /// reporting [`GATEWAY_UPDATED_SOFTWARE_VERSION`]; nothing sleeps for that instant, it
    /// is stored on the record. An install the clock has already made due is folded onto
    /// the record first, so the version an appliance runs only ever moves forward.
    ///
    /// An appliance already running the newest version has nothing to install, and one
    /// already installing it is installing it: in both cases the trigger leaves the record
    /// exactly as the call before it left it, so the same request twice is one update and
    /// a second call neither restarts nor delays the install the first one started.
    /// Nothing else on the record is touched — an update is not an association, a window,
    /// a schedule or a tagging call.
    pub fn update_gateway_software_now(
        &mut self,
        arn: &str,
        now: SystemTime,
    ) -> Option<StoredGateway> {
        let gateway = self.gateways.get_mut(arn)?;
        gateway.settle_software_update(now);
        if gateway.software_update.is_none()
            && gateway.software_version != GATEWAY_UPDATED_SOFTWARE_VERSION
        {
            gateway.software_update = Some(GatewaySoftwareUpdate {
                to_version: GATEWAY_UPDATED_SOFTWARE_VERSION.to_string(),
                installed_at: now + GATEWAY_SOFTWARE_UPDATE_INSTALLS_AFTER,
            });
        }
        self.gateways.get(arn).cloned()
    }

    /// Set the maintenance start time of the gateway stored under `arn`, and hand back the
    /// record as it is now STORED — read out of the map after the write, so what a caller
    /// gets is what the store holds.
    ///
    /// `None` — nothing written — when this store holds no gateway under `arn`.
    ///
    /// The window REPLACES whatever that gateway carried — the window the activation
    /// assigned it ([`default_maintenance_start_time`]) or the one an earlier put wrote:
    /// the request states the window the gateway is to have, so a day the request does not
    /// carry is not on the record afterwards either (a monthly window does not keep the
    /// `DayOfWeek` a weekly one left behind), and setting the same window again leaves the
    /// record exactly as one call left it. Nothing else on the record is touched — setting
    /// the window neither moves the gateway's association nor touches its schedule or tags.
    pub fn set_maintenance_start_time(
        &mut self,
        arn: &str,
        window: MaintenanceStartTime,
    ) -> Option<StoredGateway> {
        self.gateways.get_mut(arn)?.maintenance_start_time = window;
        self.gateways.get(arn).cloned()
    }

    /// Associate the gateway stored under `gateway_arn` with the hypervisor stored under
    /// `server_arn`, and hand back the gateway as it is now STORED — cloned off the record
    /// in the map after the write, so the ARN a caller gets is the key the store holds.
    ///
    /// Both ARNs have to name a record this store really holds IN `region`, the region the
    /// request was addressed to: an association is a link between two existing resources of
    /// one regional endpoint, so an ARN naming a gateway or a server this service does not
    /// have — or one it holds in ANOTHER region, which this endpoint resolves no more than
    /// a record it never held ([`arn_is_in_region`]) — is refused and NOTHING is written
    /// ([`AssociationRefusal::NoSuchGateway`], [`AssociationRefusal::NoSuchServer`]). The
    /// gateway is resolved first, so a request that names neither is answered for the
    /// gateway — one answer, not one that depends on which map was consulted first; the
    /// region scope is part of resolving each end, so it does not change that order.
    ///
    /// A gateway backs up the machines of ONE hypervisor, so a gateway already associated
    /// with a different server is refused too
    /// ([`AssociationRefusal::AlreadyAssociated`], carrying the server it is associated
    /// with). Associating it with the server it is ALREADY associated with is the same
    /// link again: it succeeds and changes nothing, so a retried request answers as the
    /// first one did.
    ///
    /// The `HypervisorId` the gateway then reports is read off the stored hypervisor's own
    /// ARN ([`arn_resource_id`]), so the id a read reports and the hypervisor the gateway
    /// is linked to cannot drift apart.
    pub fn associate_gateway_to_server(
        &mut self,
        gateway_arn: &str,
        server_arn: &str,
        region: &str,
    ) -> Result<StoredGateway, AssociationRefusal> {
        if !(arn_is_in_region(gateway_arn, region) && self.gateways.contains_key(gateway_arn)) {
            return Err(AssociationRefusal::NoSuchGateway);
        }
        let server = self
            .hypervisors
            .get(server_arn)
            .filter(|_| arn_is_in_region(server_arn, region))
            .ok_or(AssociationRefusal::NoSuchServer)?;
        // The link is to the record's own ARN — the key this store holds the hypervisor
        // under — and not to the string the request spelled. Both are read before the
        // gateway is borrowed mutably, which is why they are cloned here.
        let server_arn = server.arn.clone();
        let server_id = arn_resource_id(&server_arn).map(str::to_string);
        // Present: the same `&mut self` borrow found it above, so nothing could have
        // removed it in between.
        let Some(gateway) = self.gateways.get_mut(gateway_arn) else {
            return Err(AssociationRefusal::NoSuchGateway);
        };
        if let Some(current) = &gateway.hypervisor_arn {
            if *current != server_arn {
                return Err(AssociationRefusal::AlreadyAssociated(current.clone()));
            }
        }
        gateway.hypervisor_arn = Some(server_arn);
        gateway.hypervisor_id = server_id;
        Ok(gateway.clone())
    }

    /// Clear the association of the gateway stored under `gateway_arn`, and hand back the
    /// gateway as it is now STORED — cloned off the record in the map after the write, so
    /// the ARN a caller gets is the key the store holds.
    ///
    /// The inverse of [`Self::associate_gateway_to_server`] and the only way a link it
    /// wrote is removed: the link is a fact about the GATEWAY record, so both the server's
    /// ARN and the `HypervisorId` read off it are dropped together and every later read of
    /// that gateway (`GetGateway`, `ListGateways`) reports no server. The HYPERVISOR record
    /// is not touched — it exists independently of the gateways that back it up, and it may
    /// not be in this store at all, since a gateway keeps naming a server that was deleted.
    ///
    /// Refused, with nothing written, for a `gateway_arn` this store holds no record under
    /// ([`DisassociationRefusal::NoSuchGateway`]) and for a gateway associated with no
    /// server ([`DisassociationRefusal::NotAssociated`]): there is no link to remove, and
    /// no server the request could be naming.
    pub fn disassociate_gateway_from_server(
        &mut self,
        gateway_arn: &str,
    ) -> Result<StoredGateway, DisassociationRefusal> {
        let gateway = self
            .gateways
            .get_mut(gateway_arn)
            .ok_or(DisassociationRefusal::NoSuchGateway)?;
        if gateway.hypervisor_arn.is_none() {
            return Err(DisassociationRefusal::NotAssociated);
        }
        gateway.hypervisor_arn = None;
        gateway.hypervisor_id = None;
        Ok(gateway.clone())
    }

    /// Persist a hypervisor for `configuration` and hand back the record as it is now
    /// STORED — read out of the map after the insert, so the ARN that comes back is the
    /// key the store holds rather than a string built from the request.
    ///
    /// The record is minted [`HYPERVISOR_PENDING`] with the instant its activation falls
    /// due ([`HYPERVISOR_PENDING_FOR`] after `now`): on AWS an imported hypervisor is not
    /// reachable the moment the import returns, and the state a read reports is derived
    /// from that stored instant rather than advanced by anything a caller does.
    ///
    /// `ImportHypervisorConfiguration` is the operation that performs this: its handler
    /// resolves the host, bounds the store and then calls here, so the record a caller
    /// gets an ARN for is the one this map holds.
    pub fn import_hypervisor(
        &mut self,
        region: &str,
        configuration: HypervisorConfiguration,
        creation: Creation,
    ) -> StoredHypervisor {
        let (_, arn) = self.mint_resource(MintedResource::Hypervisor, region);
        let record = StoredHypervisor {
            arn: arn.clone(),
            name: configuration.name,
            host: configuration.host,
            kms_key_arn: configuration.kms_key_arn,
            log_group_arn: None,
            state: HYPERVISOR_PENDING.to_string(),
            online_due: Some(creation.at + HYPERVISOR_PENDING_FOR),
            last_successful_metadata_sync_time: None,
            latest_metadata_sync_status: None,
            latest_metadata_sync_status_message: None,
            created_time: creation.at,
            created_by: creation.by,
            tags: configuration.tags,
            // An import starts no metadata sync: `StartVirtualMachinesMetadataSync` is
            // the operation that does, for a hypervisor that already exists.
            metadata_sync_started: None,
            // An import sets no property mappings: they are SET for a hypervisor that
            // already exists, so a freshly imported record carries none.
            property_mappings: None,
        };
        // `or_insert` hands back a reference INTO the map, so the record returned is the
        // one the store now holds; the id counter makes the key fresh, so this always
        // inserts.
        self.hypervisors.entry(arn).or_insert(record).clone()
    }

    /// The hypervisor stored under `arn`, if any.
    ///
    /// The ONE lookup every hypervisor operation resolves its `ServerArn` through: an ARN
    /// with no record behind it is absent here, whether it was never imported or has been
    /// deleted, which is the same answer the real service gives for both.
    pub fn hypervisor(&self, arn: &str) -> Option<&StoredHypervisor> {
        self.hypervisors.get(arn)
    }

    /// The hypervisor stored under `arn`, for in-place mutation.
    pub fn hypervisor_mut(&mut self, arn: &str) -> Option<&mut StoredHypervisor> {
        self.hypervisors.get_mut(arn)
    }

    /// Remove the hypervisor stored under `arn`, returning the record that was removed.
    ///
    /// The machines of its inventory go with it: a [`StoredVirtualMachine`] is a machine
    /// ON a hypervisor, so once the hypervisor is gone the service holds no inventory for
    /// it and `ListVirtualMachines` reports none — rather than machines whose
    /// `HypervisorId` names nothing.
    ///
    /// The ASSOCIATION a gateway holds to it does NOT go with it: the link is a fact about
    /// the GATEWAY ([`Self::associate_gateway_to_server`]), which stays its own resource,
    /// and `DisassociateGatewayFromServer` is the operation that clears it. So a gateway
    /// still names the server it was associated with, and a read of that gateway still
    /// reports the id it recorded.
    ///
    /// That leftover link resolves no hypervisor: the record is gone from this map, so
    /// [`Self::hypervisor`] answers `None` for its ARN and every operation that resolves a
    /// hypervisor refuses it as a missing one ([`MissingResource::Hypervisor`]) — the same
    /// answer an ARN that was never imported takes, so which other resources still point at
    /// a deleted record never decides how a request naming it is refused.
    pub fn remove_hypervisor(&mut self, arn: &str) -> Option<StoredHypervisor> {
        let removed = self.hypervisors.remove(arn)?;
        self.virtual_machines
            .retain(|_, machine| machine.hypervisor_arn != arn);
        Some(removed)
    }

    /// How many hypervisors are stored.
    pub fn hypervisor_count(&self) -> usize {
        self.hypervisors.len()
    }

    /// One page of the hypervisors of `region`, in ARN order — the order `ListHypervisors`
    /// reports them in. Same contract as [`Self::page_of_gateways`], whose implementation
    /// it shares, region scope included.
    pub fn page_of_hypervisors(
        &self,
        region: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Page<StoredHypervisor> {
        page_of_in_region(&self.hypervisors, region, after, limit)
    }

    /// Every hypervisor this store holds whose `Host` is `host`, in ARN order.
    ///
    /// A hypervisor is keyed by the ARN minted for it and by nothing else: `Host` is
    /// metadata the record carries, not an identity, so any number of records may hold one
    /// host and this hands back all of them. A lookup, never a constraint — no writer of
    /// this store consults it to decide whether a record may exist.
    pub fn hypervisors_for_host(&self, host: &str) -> Vec<&StoredHypervisor> {
        self.hypervisors
            .values()
            .filter(|hypervisor| hypervisor.host == host)
            .collect()
    }

    /// Apply `update` to the hypervisor stored under `arn`, and hand back the record as it
    /// is now STORED — read out of the map after the write, so what a caller gets is what
    /// the store holds.
    ///
    /// Only the members `update` STATES are written: a member the request did not carry
    /// leaves the stored one as it was, rather than clearing it, exactly as
    /// [`Self::set_maintenance_start_time`] replaces only the resource its request states.
    /// Nothing else on the record moves — an update is not an import and not a sync, so it
    /// neither re-dates the record nor touches its state, tags, mappings or sync progress.
    ///
    /// `None` — nothing written — when `arn` names no hypervisor this store holds (never
    /// imported, or already deleted). That is the ONLY condition an update is refused on:
    /// a record is identified by its ARN alone, so the VALUES the update carries decide
    /// nothing about whether it may be written, and a host another record already holds is
    /// written like any other.
    pub fn update_hypervisor(
        &mut self,
        arn: &str,
        update: HypervisorUpdate,
    ) -> Option<StoredHypervisor> {
        let record = self.hypervisors.get_mut(arn)?;
        if let Some(name) = update.name {
            record.name = name;
        }
        if let Some(host) = update.host {
            record.host = host;
        }
        if let Some(log_group_arn) = update.log_group_arn {
            // The empty string the member's pattern accepts means "no log group", so it is
            // stored as the absent value a read reports nothing for.
            record.log_group_arn = (!log_group_arn.is_empty()).then_some(log_group_arn);
        }
        // The record the store now holds, read back off the same borrow that wrote it:
        // nothing between the write and here could have removed it.
        Some(record.clone())
    }

    /// The gateway ASSOCIATED with the hypervisor `hypervisor_arn` names, if any.
    ///
    /// A gateway records the hypervisor it serves in [`StoredGateway::hypervisor_arn`], so
    /// the association is read off the gateway records rather than kept a second time on
    /// the hypervisor record. `None` for a hypervisor no gateway is associated with, which
    /// is the ONE question an operation only the serving gateway can carry out is gated on
    /// ([`GATEWAY_OUT_OF_DATE`]).
    ///
    /// This asks about the gateway records and nothing else: a link [`Self::remove_hypervisor`]
    /// left behind names an ARN this store holds no hypervisor under, so a `Some` here is no
    /// evidence that the record exists. Whether the store HOLDS the hypervisor is
    /// [`Self::hypervisor`]'s question, and a caller that resolves both asks that one first
    /// — an ARN naming no record is refused as a missing hypervisor
    /// ([`MissingResource::Hypervisor`]) rather than as a gateway that is out of date.
    pub fn gateway_for_hypervisor(&self, hypervisor_arn: &str) -> Option<&StoredGateway> {
        self.gateways
            .values()
            .find(|gateway| gateway.hypervisor_arn.as_deref() == Some(hypervisor_arn))
    }

    /// Set the property mappings of the hypervisor stored under `arn`, and hand back the
    /// record as it is now STORED — read out of the map after the write, so what a caller
    /// gets is what the store holds.
    ///
    /// `None` — nothing written — when this store holds no hypervisor under `arn`.
    ///
    /// The mappings REPLACE whatever that hypervisor carried: the request states the
    /// mappings the hypervisor is to have, so setting the same ones again leaves the
    /// record exactly as one call left it, and no mapping of an earlier call survives a
    /// later one. Nothing else on the record is touched — a put of mappings is not a
    /// metadata sync and does not move the hypervisor's state.
    pub fn set_property_mappings(
        &mut self,
        arn: &str,
        mappings: StoredPropertyMappings,
    ) -> Option<StoredHypervisor> {
        self.hypervisors.get_mut(arn)?.property_mappings = Some(mappings);
        self.hypervisors.get(arn).cloned()
    }

    /// Whether this store holds property mappings for the hypervisor `arn` names — the
    /// PRECONDITION of a metadata sync ([`MAPPINGS_NOT_CONFIGURED`]).
    ///
    /// `false` both for a record carrying no mappings and for an ARN naming no record at
    /// all: the question is whether the mappings a sync needs are configured, and neither
    /// case has them, which is why the one refusal covers both and reveals nothing about
    /// whether the resource exists. An EMPTY mapping list is configured — the mappings
    /// resource exists and holds no mapping ([`StoredPropertyMappings`]), which is what
    /// `GetHypervisorPropertyMappings` reports for it too.
    pub fn property_mappings_configured(&self, arn: &str) -> bool {
        self.hypervisors
            .get(arn)
            .is_some_and(|record| record.property_mappings.is_some())
    }

    /// Start a metadata sync on the hypervisor stored under `arn` at `now`, and hand back
    /// the record as it is now STORED — read out of the map after the write, so what a
    /// caller gets is what the store holds.
    ///
    /// `None` — nothing written — when this store holds no hypervisor under `arn`. Whether
    /// the sync may be started at all is settled BEFORE this is reached: its precondition
    /// is the hypervisor's property mappings
    /// ([`Self::property_mappings_configured`], the refusal
    /// [`MAPPINGS_NOT_CONFIGURED`] states), so a record reaching here has them and this
    /// writes the sync it is asked for.
    ///
    /// What is persisted is the instant the sync was REQUESTED, the status and message
    /// that instant puts the record in, and the time of a sync that had already
    /// succeeded; every later report of the new sync's progress is derived from that
    /// instant and the clock
    /// ([`StoredHypervisor::latest_metadata_sync_status_as_of`]), so no reader moves it.
    ///
    /// A sync is also what MATERIALISES the hypervisor's virtual-machine inventory: it
    /// records every machine the hypervisor reports ([`synced_inventory`]) through
    /// [`Self::record_virtual_machine`], dated at the instant the sync falls due — so the
    /// machines a listing reports and the [`SYNC_METADATA_SUCCEEDED`] a read of the
    /// hypervisor reports become true at the same instant, and a client that only has this
    /// API can reach a non-empty `ListVirtualMachines`. A machine the previous sync already
    /// reported keeps the record and the ARN it has, so a re-sync neither re-dates it nor
    /// mints a second machine for it.
    ///
    /// A sync that had NOT fallen due when this one was started is superseded by it, on
    /// both halves of that one fact: its success is not folded onto the record above, and
    /// the machines it had not published yet are published by THIS sync instead
    /// ([`StoredVirtualMachine::discovered_time`]) rather than at the abandoned sync's due
    /// instant. So the inventory a listing reports never gets ahead of the successful sync
    /// a read of the hypervisor reports.
    pub fn start_metadata_sync(&mut self, arn: &str, now: SystemTime) -> Option<StoredHypervisor> {
        let record = self.hypervisors.get_mut(arn)?;
        // A sync the clock had already finished is MATERIALISED before the new one
        // replaces the instant its progress is derived from: the time of the most recent
        // successful sync is a fact about a sync that really completed, so it never goes
        // backwards or disappears because another sync was started.
        record.last_successful_metadata_sync_time = record.last_successful_metadata_sync_time_as_of(now);
        record.metadata_sync_started = Some(now);
        let (status, message) = sync_status_as_of(now, now);
        record.latest_metadata_sync_status = Some(status.to_string());
        record.latest_metadata_sync_status_message = Some(message.to_string());
        // The machines are minted in the hypervisor's OWN region, read off the ARN this
        // store holds it under, so a machine and the hypervisor it runs on are regional
        // resources of one region.
        let region = arn_parts(arn)
            .map(|parts| parts.region.to_string())
            .unwrap_or_else(|| DEFAULT_REGION.to_string());
        let discovered = now + METADATA_SYNC_SUCCEEDS_AFTER;
        // The machines a superseded sync had not published yet are published by this one:
        // the sync they were dated by no longer succeeds at any instant, and a reader
        // decides whether a machine is part of the inventory with the same predicate this
        // writer asks it with ([`StoredVirtualMachine::discovered_as_of`]). A machine an
        // earlier sync already published keeps the instant it became visible at — that is
        // a fact about an inventory a reader could already see.
        for machine in self.virtual_machines.values_mut() {
            if machine.hypervisor_arn == arn && !machine.discovered_as_of(now) {
                machine.discovered_time = discovered;
            }
        }
        for machine in synced_inventory(arn) {
            self.record_virtual_machine(&region, arn, machine, discovered);
        }
        self.hypervisors.get(arn).cloned()
    }

    /// Record one virtual machine of `hypervisor_arn`'s inventory, and hand back the
    /// record as it is now STORED — read out of the map after the insert, so the ARN that
    /// comes back is the key the store holds.
    ///
    /// The ONE way a machine gets into this store: a machine is discovered on the
    /// hypervisor it runs on when that hypervisor's metadata is synced
    /// ([`Self::start_metadata_sync`], the only caller), and no operation of this model
    /// creates one, so nothing a client sends over the wire describes a machine. `now` is
    /// the instant the discovering sync REPORTS it, which a read compares the clock against
    /// ([`StoredVirtualMachine::discovered_as_of`]).
    ///
    /// A machine is identified on its hypervisor by its [`StoredVirtualMachine::path`] —
    /// where it sits in that hypervisor's inventory — so reporting the same path again
    /// UPDATES the record under the ARN the machine already has (keeping its
    /// `discovered_time`) rather than minting a second machine for it. Re-dating a record
    /// whose publication has not fallen due yet is the caller's business, and
    /// [`Self::start_metadata_sync`] is where it happens.
    ///
    /// `None` — nothing written — when this store holds no hypervisor under
    /// `hypervisor_arn` (a machine cannot be on a hypervisor the service does not have),
    /// or when the store already holds [`MAX_STORED_VIRTUAL_MACHINES`] machines.
    pub fn record_virtual_machine(
        &mut self,
        region: &str,
        hypervisor_arn: &str,
        inventory: VirtualMachineInventory,
        now: SystemTime,
    ) -> Option<StoredVirtualMachine> {
        if !self.hypervisors.contains_key(hypervisor_arn) {
            return None;
        }
        let known = self
            .virtual_machines
            .values()
            .find(|machine| {
                machine.hypervisor_arn == hypervisor_arn && machine.path == inventory.path
            })
            .map(|machine| {
                (
                    machine.arn.clone(),
                    machine.discovered_time,
                    machine.tags.clone(),
                )
            });
        // A machine's AWS tags are kept across a re-sync, exactly as its
        // `discovered_time` is: a tag is written through `TagResource` and belongs to the
        // resource, so re-reporting the hypervisor's inventory does not clear what a
        // client tagged the machine with.
        let (arn, discovered_time, tags) = match known {
            Some(known) => known,
            None => {
                if self.virtual_machines.len() >= MAX_STORED_VIRTUAL_MACHINES {
                    return None;
                }
                let (_, arn) = self.mint_resource(MintedResource::VirtualMachine, region);
                (arn, now, BTreeMap::new())
            }
        };
        let record = StoredVirtualMachine {
            arn: arn.clone(),
            name: inventory.name,
            host_name: inventory.host_name,
            path: inventory.path,
            hypervisor_arn: hypervisor_arn.to_string(),
            last_backup_date: inventory.last_backup_date,
            vmware_tags: inventory.vmware_tags,
            discovered_time,
            tags,
        };
        self.virtual_machines.insert(arn.clone(), record);
        // Read back out of the map, so what a caller gets is the record the store holds.
        self.virtual_machines.get(&arn).cloned()
    }

    /// The virtual machine stored under `arn`, if any — the record, whatever the clock has
    /// reached.
    pub fn virtual_machine(&self, arn: &str) -> Option<&StoredVirtualMachine> {
        self.virtual_machines.get(arn)
    }

    /// The virtual machine a read AT `now` resolves `arn` to: the record stored under it,
    /// once the sync that discovered it has reported it
    /// ([`StoredVirtualMachine::discovered_as_of`]).
    ///
    /// `None` — so `GetVirtualMachine` refuses the ARN — both for an ARN no machine is
    /// stored under and for a machine whose discovering sync has not fallen due yet: until
    /// then the machine is not part of any inventory this service holds, and the listing
    /// that would hand a client its ARN does not report it either.
    pub fn virtual_machine_as_of(
        &self,
        arn: &str,
        now: SystemTime,
    ) -> Option<&StoredVirtualMachine> {
        self.virtual_machine(arn)
            .filter(|machine| machine.discovered_as_of(now))
    }

    /// How many virtual machines are stored.
    pub fn virtual_machine_count(&self) -> usize {
        self.virtual_machines.len()
    }

    /// One page of the virtual machines of `region` that a read AT `now` reports, in ARN
    /// order — the order `ListVirtualMachines` reports them in — holding only the machines
    /// of `hypervisor_arn` when one is named.
    ///
    /// A machine of another region is not part of the listing at all, exactly as
    /// [`Self::page_of_gateways`] lists only the gateways of the region the request was
    /// addressed to. Nor is a machine whose discovering sync has not fallen due yet
    /// ([`StoredVirtualMachine::discovered_as_of`]), or one of another hypervisor: every
    /// filter is applied BEFORE the page is cut, so a page holds up to `limit` MATCHING
    /// machines and carries a token only while a matching machine remains past its
    /// boundary, and a filtered listing pages exactly like an unfiltered one. Same boundary
    /// contract as [`Self::page_of_gateways`], whose implementation it shares.
    ///
    /// `hypervisor_arn` is the one filter this listing takes beside its page controls, so
    /// it is also what the token this page carries is minted UNDER
    /// ([`encode_page_token`]): the boundary of a page of the machines of one hypervisor
    /// resumes the listing of that hypervisor and no other, because the machines matching
    /// another filter are not the records this page was cut from.
    pub fn page_of_virtual_machines(
        &self,
        region: &str,
        hypervisor_arn: Option<&str>,
        after: Option<&str>,
        limit: usize,
        now: SystemTime,
    ) -> Page<StoredVirtualMachine> {
        page_of_where(
            &self.virtual_machines,
            after,
            limit,
            &[hypervisor_arn],
            |arn, machine| {
                arn_is_in_region(arn, region)
                    && machine.discovered_as_of(now)
                    && hypervisor_arn.is_none_or(|filter| machine.hypervisor_arn == filter)
            },
        )
    }

    /// The ARN `arn` is stored under and the tags the resource it names carries, or
    /// `None` for an ARN this store holds no record for.
    ///
    /// The tagging operations take any of this service's resources in one `ResourceARN`
    /// member, so they resolve it through this one lookup rather than knowing which map
    /// holds which kind: the ARN comes back out of the store, so what a response echoes
    /// is the key the store holds. EVERY resource this store owns is reachable through it
    /// — gateway, hypervisor and virtual machine — so a resource a sibling read resolves
    /// is a resource the tagging operations resolve too, and `None` means the service
    /// reports no resource under that ARN at all.
    ///
    /// `now` is what decides a virtual machine, whose record is part of its hypervisor's
    /// inventory only from the instant the sync that discovered it fell due
    /// ([`StoredVirtualMachine::discovered_as_of`]) — the same lookup `GetVirtualMachine`
    /// and `ListVirtualMachines` resolve a machine through, so the operations cannot
    /// disagree about which machines exist. A read of tags, like every other read, writes
    /// nothing.
    pub fn resource_tags(&self, arn: &str, now: SystemTime) -> Option<(String, Vec<Tag>)> {
        if let Some(gateway) = self.gateways.get(arn) {
            return Some((gateway.arn.clone(), gateway.tag_list()));
        }
        if let Some(hypervisor) = self.hypervisors.get(arn) {
            return Some((hypervisor.arn.clone(), hypervisor.tag_list()));
        }
        let machine = self.virtual_machine_as_of(arn, now)?;
        Some((machine.arn.clone(), machine.tag_list()))
    }

    /// Merge `requested` into the tags of the resource `arn` names, and hand back the ARN
    /// the store holds it under.
    ///
    /// `None` for an ARN this store holds no record for; `Err` for tags the model's own
    /// traits cannot refuse ([`merged_tags`]), in which case NOTHING is written. Spans
    /// every resource type, `now` included, for the same reason as [`Self::resource_tags`].
    pub fn merge_resource_tags(
        &mut self,
        arn: &str,
        requested: &[Tag],
        now: SystemTime,
    ) -> Option<Result<String, String>> {
        let (resource_arn, tags) = self.taggable_mut(arn, now)?;
        Some(merged_tags(tags, requested).map(|merged| {
            *tags = merged;
            resource_arn
        }))
    }

    /// Remove the tags `keys` names from the resource `arn` names, and hand back the ARN
    /// the store holds it under.
    ///
    /// Removal is BY KEY and nothing else: a key the resource carries is dropped whatever
    /// its value, a key it does not carry leaves the tag set alone (so removing the same
    /// key twice, or one never set, is a no-op that still succeeds), and a repeated key in
    /// one request removes it once. `None` for an ARN this store holds no record for, and
    /// spans every resource type, `now` included, for the same reason as
    /// [`Self::resource_tags`].
    pub fn remove_resource_tags(
        &mut self,
        arn: &str,
        keys: &[String],
        now: SystemTime,
    ) -> Option<String> {
        let (resource_arn, tags) = self.taggable_mut(arn, now)?;
        for key in keys {
            tags.remove(key);
        }
        // The ARN comes back out of the store, so what the response echoes is the key the
        // store holds rather than the string the request spelled.
        Some(resource_arn)
    }

    /// The ARN `arn` is stored under and the tag map of the resource it names, to WRITE.
    ///
    /// The writing half of [`Self::resource_tags`], resolved over the same three maps and
    /// against the same clock, so a tag write reaches exactly the resources a tag read
    /// reports and neither operation has to know which map holds which kind.
    fn taggable_mut(
        &mut self,
        arn: &str,
        now: SystemTime,
    ) -> Option<(String, &mut BTreeMap<String, String>)> {
        if let Some(gateway) = self.gateways.get_mut(arn) {
            return Some((gateway.arn.clone(), &mut gateway.tags));
        }
        if let Some(hypervisor) = self.hypervisors.get_mut(arn) {
            return Some((hypervisor.arn.clone(), &mut hypervisor.tags));
        }
        let machine = self
            .virtual_machines
            .get_mut(arn)
            .filter(|machine| machine.discovered_as_of(now))?;
        Some((machine.arn.clone(), &mut machine.tags))
    }

    /// The page boundary a client-supplied `NextToken` resumes after, for the listing of
    /// `resource_type` records of `region` that applies `filters` — the `after` every
    /// `page_of_*` takes.
    ///
    /// `Some` only for a token THIS STORE REALLY ISSUED for THIS listing, which is a
    /// question about the store and not about the spelling of the token: the boundary a
    /// token encodes is a key a page of this listing ended on, so it has to be an ARN this
    /// store MINTED for that resource type IN THAT REGION
    /// ([`Self::is_minted_resource_arn`]) and one of the region this listing reports
    /// ([`arn_is_in_region`]). Everything else is `None` —
    /// not hex, not UTF-8, not an ARN of this service, an ARN naming another resource
    /// type, an ARN of another region's listing, and an ARN of the right shape that this
    /// store never minted — and the caller answers `None` with the modelled
    /// `ValidationException`, never by silently restarting the listing from the top: a
    /// client that mangled a token must learn that, not receive page one again. Shape
    /// alone cannot decide this, because an ARN of the right form sorting below every
    /// stored key would include the whole listing and hand back page one.
    ///
    /// `filters` are the filters the request presenting the token carries, in the order
    /// the listing applies them — the very filters a page of this listing mints its token
    /// under ([`encode_page_token`]) — so a token issued for a differently filtered
    /// listing of the same resource type is `None` too: the records it was cut from are
    /// not the records being listed, and resuming after its boundary would drop every
    /// matching record sorting below it and answer with a page no client can tell from a
    /// final one. A listing that takes no filter at all passes an empty slice.
    ///
    /// A boundary whose record has since been DELETED still resumes the listing: a minted
    /// ARN stays minted, so the page after it is the records that sort above it, exactly
    /// as [`page_of_where`] documents for a record deleted between two pages.
    pub fn resume_after(
        &self,
        token: &str,
        resource_type: &str,
        region: &str,
        filters: &[Option<&str>],
    ) -> Option<String> {
        let boundary = decode_page_token(token, resource_type, filters)?;
        (arn_is_in_region(&boundary, region)
            && self.is_minted_resource_arn(&boundary, resource_type))
        .then_some(boundary)
    }

    /// Whether `arn` is one THIS STORE minted for a `resource_type` record — the question
    /// [`Self::resume_after`] asks of a token's boundary.
    ///
    /// A minted ARN is the ARN of the sequence its id encodes, rendered the ONE way this
    /// store renders it ([`minted_resource_id`]) and below the sequence this store mints
    /// next IN THAT REGION — so an id of another spelling, of another account or
    /// partition, or of a sequence that region has not reached is not minted. The region
    /// is part of the question because a sequence is minted in exactly one region: a
    /// listing of one region never handed out a boundary of a record minted in another,
    /// even where that boundary's own region segment is this listing's.
    ///
    /// The mint sequence rather than the record map is what is consulted, because a record
    /// this store minted and has since removed was still minted — including after the
    /// registry's bound dropped that pair's entry, where the sequence a pair has minted is
    /// read off its kind's floor ([`Self::next_resource_id`]), which is at or above every
    /// sequence the pair reached. So a boundary this store issued stays accepted for as
    /// long as a client holds it; a region that minted nothing at all holds no record
    /// either, so a boundary the floor accepts for such a region resumes an empty listing.
    fn is_minted_resource_arn(&self, arn: &str, resource_type: &str) -> bool {
        let Some(resource) = MintedResource::for_resource_type(resource_type) else {
            return false;
        };
        let Some(parts) = arn_parts(arn) else {
            return false;
        };
        if parts.resource_type != resource_type {
            return false;
        }
        let Some(sequence) = minted_resource_sequence(parts.resource_id, resource) else {
            return false;
        };
        // Comparing against the ARN this store WOULD mint for that sequence in that region
        // is what settles the account and partition segments too, in one place rather than
        // segment by segment.
        sequence < self.next_resource_id(resource, parts.region)
            && arn == minted_resource_arn(resource, parts.region, sequence)
    }

    /// The sequence this store mints NEXT for a `resource` record of `region`, which is
    /// also the first sequence it has NOT minted there: a pair the registry holds an entry
    /// for answers from that entry, and one it does not from the kind's floor, which is at
    /// or above every sequence a dropped entry had reached.
    fn next_resource_id(&self, resource: MintedResource, region: &str) -> u64 {
        self.next_resource_ids
            .get(&(resource, arn_region(region)))
            .copied()
            .unwrap_or_else(|| self.minted_sequence_floor(resource))
    }

    /// The floor a `resource` pair with no registry entry mints from — zero until the
    /// registry's bound has dropped an entry of that kind. See `minted_sequence_floors`.
    fn minted_sequence_floor(&self, resource: MintedResource) -> u64 {
        self.minted_sequence_floors
            .get(&resource)
            .copied()
            .unwrap_or_default()
    }

    /// Mint the next id for a `resource` record of `region`, and the ARN the store keys
    /// such a record under: `(id, arn)`.
    ///
    /// The ONE place an ARN of this store is minted — all three resources go through it —
    /// so the form an ARN has, the sequence it comes from and the provenance check that
    /// resumes a page boundary ([`Self::is_minted_resource_arn`]) cannot drift apart.
    fn mint_resource(&mut self, resource: MintedResource, region: &str) -> (String, String) {
        self.bound_mint_sequences();
        let region = arn_region(region);
        // A pair with no entry starts at its kind's floor, not at zero: bounding the
        // registry drops entries, and an identity a dropped entry had already handed out
        // stays handed out.
        let floor = self.minted_sequence_floor(resource);
        let sequence = self
            .next_resource_ids
            .entry((resource, region.clone()))
            .or_insert(floor);
        let id = minted_resource_id(resource, *sequence);
        let arn = minted_resource_arn(resource, &region, *sequence);
        *sequence += 1;
        (id, arn)
    }

    /// Keep the mint registry inside [`MAX_MINT_SEQUENCES`] by dropping the ENTRIES of the
    /// (kind, region) pairs this store holds no record for — and folding the sequence each
    /// of them had reached into that kind's floor.
    ///
    /// A pair a record is stored under keeps its own entry. A pair whose entry is dropped
    /// keeps its identities SPENT: the floor it will mint from afterwards is at or above
    /// the sequence it had reached ([`Self::minted_sequence_floor`]), so no ARN this store
    /// has handed out is ever handed out again, and no boundary it has issued stops being
    /// one it issued. What the bound frees is the per-pair bookkeeping, which is the part
    /// that grows with the regions a caller addresses.
    fn bound_mint_sequences(&mut self) {
        if self.next_resource_ids.len() < MAX_MINT_SEQUENCES {
            return;
        }
        let stored = self.stored_mint_sequence_keys();
        let mut floors = ::std::mem::take(&mut self.minted_sequence_floors);
        self.next_resource_ids.retain(|key, next| {
            if stored.contains(key) {
                return true;
            }
            let floor = floors.entry(key.0).or_default();
            *floor = (*floor).max(*next);
            false
        });
        self.minted_sequence_floors = floors;
    }

    /// How many (kind, region) mint sequences the registry holds — the registry whose size
    /// [`MAX_MINT_SEQUENCES`] bounds, so a test can tell the bound really ran.
    pub fn mint_sequence_count(&self) -> usize {
        self.next_resource_ids.len()
    }

    /// The (kind, region) pairs this store holds at least one record under.
    fn stored_mint_sequence_keys(&self) -> BTreeSet<(MintedResource, String)> {
        let keyed = |kind: MintedResource, arn: &String| {
            arn_parts(arn).map(|parts| (kind, parts.region.to_string()))
        };
        self.gateways
            .keys()
            .filter_map(|arn| keyed(MintedResource::Gateway, arn))
            .chain(
                self.hypervisors
                    .keys()
                    .filter_map(|arn| keyed(MintedResource::Hypervisor, arn)),
            )
            .chain(
                self.virtual_machines
                    .keys()
                    .filter_map(|arn| keyed(MintedResource::VirtualMachine, arn)),
            )
            .collect()
    }
}

/// One page of the records of an ARN-keyed map that are stored in `region`, in key order
/// — the paging every list operation of this service does, written once.
///
/// `after` is the key the previous page ended on (resolved from a `NextToken`), and the
/// page starts at the next key AFTER it — so a record created, or deleted, between two
/// pages changes what the second page holds without ever repeating or skipping a record
/// that stayed put. `limit` is the caller's `MaxResults` already resolved by
/// [`page_limit`], so no request can make one response carry a whole
/// store. A record of another region is not part of the listing at all
/// ([`arn_is_in_region`]): the filter is applied before the page is cut, so a page holds
/// up to `limit` records OF THAT REGION and promises a successor only while one remains.
fn page_of_in_region<T: Clone>(
    records: &BTreeMap<String, T>,
    region: &str,
    after: Option<&str>,
    limit: usize,
) -> Page<T> {
    // No filters: a listing of a region's records of one kind is the whole listing, so
    // the only query identity its token needs is the boundary, whose own region segment
    // is what scopes it to this region.
    page_of_where(records, after, limit, &[], |arn, _| {
        arn_is_in_region(arn, region)
    })
}

/// One page of the records of an ARN-keyed map that `keep` accepts — judged on the ARN
/// the record is keyed under and the record itself — in key order.
///
/// The paging [`page_of_in_region`] does, with a predicate: the records `keep` rejects are
/// not part of the listing at all, so they neither fill a page nor make one promise a
/// successor. `page_of_in_region` is this with a predicate that keeps the records of one
/// region, so a further-filtered listing and a merely regional one cut their pages and
/// mint their tokens the same way.
///
/// `filters` are the REQUEST-supplied filters `keep` applies beside the region — the
/// values the listing was narrowed by, in the order it applies them — and the token this
/// page carries is minted under them ([`encode_page_token`]), so the page a token resumes
/// is a page of the same query. A listing narrowed by nothing but its region passes an
/// empty slice, the region being carried by the boundary ARN itself.
fn page_of_where<T: Clone>(
    records: &BTreeMap<String, T>,
    after: Option<&str>,
    limit: usize,
    filters: &[Option<&str>],
    keep: impl Fn(&str, &T) -> bool,
) -> Page<T> {
    let lower = match after {
        Some(key) => Bound::Excluded(key.to_string()),
        None => Bound::Unbounded,
    };
    let mut remaining = records
        .range((lower, Bound::Unbounded))
        .filter(|(arn, record)| keep(arn, record));
    let page: Vec<(&String, &T)> = remaining.by_ref().take(limit).collect();
    // A token only when a record really remains past this page: the final page must
    // not carry one, or a paginating client never stops.
    let next_token = remaining
        .next()
        .and_then(|_| page.last().map(|(key, _)| encode_page_token(key, filters)));
    Page {
        items: page.into_iter().map(|(_, record)| record.clone()).collect(),
        next_token,
    }
}

/// Check ONE Amazon Web Services tag key/value pair, wherever in this model a request
/// carries such a pair to be persisted — `key_at` and `value_at` are the validation paths
/// of the members the two halves arrived in, which the refusal names the way the boundary
/// validator names a member it refuses itself
/// ([`crate::service::element_member_path`]).
///
/// Two rules, both from the model's docs rather than its traits — so the boundary
/// validator cannot have refused them already, whichever member targets the `TagKey` and
/// `TagValue` shapes:
///
/// * a tag key "can't start with `aws:`" (that prefix is reserved for AWS-generated
///   tags), which `TagKey`'s `@pattern` allows;
/// * "Spaces are not allowed in tag values" (the `Tag` shape's own doc), which
///   `TagValue`'s `@pattern` of `^[^\x00]*$` admits. A space is read as the whole Unicode
///   whitespace family — deliberately wider than `U+0020`, since a tab or a line break
///   separates words exactly as a space does — where a KEY is allowed every separator its
///   pattern spells out as `\p{Z}`, so a key may carry spaces and a value may not.
///
/// `Err` carries the whole message the refusal states, in which case the caller writes
/// NOTHING.
pub fn validate_tag_pair(
    key: &str,
    value: &str,
    key_at: &str,
    value_at: &str,
) -> Result<(), String> {
    if is_reserved_tag_key(key) {
        return Err(crate::service::handler_constraint_violation(
            Some(key),
            key_at,
            &format!(
                "a tag key must not start with the reserved prefix \
                 '{RESERVED_TAG_KEY_PREFIX}'"
            ),
        ));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(crate::service::handler_constraint_violation(
            Some(value),
            value_at,
            "a tag value must not contain spaces",
        ));
    }
    Ok(())
}

/// The member a request carries resource tags in.
///
/// `smithy_query(query="shape",shape="CreateGatewayInput,ImportHypervisorConfigurationInput,TagResourceInput")`:
/// every input of this model that accepts tags names the member `Tags`, targeting the one
/// `Tags` list of `Tag`.
const TAGS_MEMBER: &str = "Tags";

/// The member a `PutHypervisorPropertyMappings` request carries its mappings in
/// (`smithy_query(query="shape",shape="PutHypervisorPropertyMappingsInput")`).
const TAG_MAPPINGS_MEMBER: &str = "VmwareToAwsTagMappings";

/// Check the AWS tag pairs a request's `VmwareToAwsTagMappings` carries, by the same
/// rules as every other AWS tag pair this service persists ([`validate_tag_pair`]).
///
/// `VmwareToAwsTagMapping$AwsTagKey` and `$AwsTagValue` target the very `TagKey` and
/// `TagValue` shapes `Tag$Key` and `Tag$Value` do, so a pair this service would refuse as
/// a resource tag is refused here too. The VMware half of a mapping (`VmwareCategory`,
/// `VmwareTagName`) names a tag of the hypervisor rather than an AWS tag, so these rules
/// do not reach it.
///
/// `Err` carries the message the refusal states, and nothing is written.
pub fn validate_tag_mappings(mappings: &[VmwareToAwsTagMapping]) -> Result<(), String> {
    for (index, mapping) in mappings.iter().enumerate() {
        // The element's place in the list, counted as a validation path counts it, so a
        // request carrying several mappings is told WHICH one was refused.
        let position = index + 1;
        validate_tag_pair(
            &mapping.aws_tag_key,
            &mapping.aws_tag_value,
            &crate::service::element_member_path(TAG_MAPPINGS_MEMBER, position, "AwsTagKey"),
            &crate::service::element_member_path(TAG_MAPPINGS_MEMBER, position, "AwsTagValue"),
        )?;
    }
    Ok(())
}

/// Check every AWS tag pair a request's `Tags` carries, by the rules
/// [`validate_tag_pair`] states.
///
/// Both rules are facts of the REQUEST alone — they are read off the pair and off nothing
/// else — so a handler answers them BEFORE it consults the store, exactly as
/// [`validate_tag_mappings`] is answered for the mapping member: a pair this service may
/// not persist is refused for being that pair, whether or not the `ResourceARN` beside it
/// names a record this store holds. Were the lookup first, one request would get two
/// different answers for the same pair depending on something the pair has nothing to do
/// with, and a caller that got both wrong would learn only about the ARN.
///
/// `Err` carries the message the refusal states, in which case NOTHING is written. The
/// pairs are checked in the order the request listed them, so one request is always
/// refused for its first offending pair.
pub fn validate_tags(requested: &[Tag]) -> Result<(), String> {
    for (index, tag) in requested.iter().enumerate() {
        // As in `validate_tag_mappings`: the position names the pair that was refused.
        let position = index + 1;
        validate_tag_pair(
            &tag.key,
            &tag.value,
            &crate::service::element_member_path(TAGS_MEMBER, position, "Key"),
            &crate::service::element_member_path(TAGS_MEMBER, position, "Value"),
        )?;
    }
    Ok(())
}

/// Merge a request's `Tags` into the tags a resource already carries, refusing what the
/// model's own traits cannot express.
///
/// Each requested pair satisfies the tag rules [`validate_tags`] states, and a
/// resource carries at most [`MAX_TAGS_PER_RESOURCE`] tags — the one rule of the three
/// that is about the resource rather than a single pair, and so the one reported at the
/// tag member itself rather than at a pair inside it, and the one a handler can only
/// answer once it has the resource in hand.
///
/// `Err` carries the message the refusal states. Merging is by key, so a key already
/// present is overwritten rather than duplicated.
pub fn merged_tags(
    existing: &BTreeMap<String, String>,
    requested: &[Tag],
) -> Result<BTreeMap<String, String>, String> {
    validate_tags(requested)?;
    let mut merged = existing.clone();
    for tag in requested {
        merged.insert(tag.key.clone(), tag.value.clone());
    }
    if merged.len() > MAX_TAGS_PER_RESOURCE {
        return Err(crate::service::handler_constraint_violation(
            Some(&merged.len().to_string()),
            &crate::service::member_path(TAGS_MEMBER),
            &format!("a resource must carry at most {MAX_TAGS_PER_RESOURCE} tags"),
        ));
    }
    Ok(merged)
}

/// A request's `Tags` as the stored tag map, for a resource that carries none yet.
/// Same rules as [`merged_tags`], which this delegates to.
pub fn tag_map(requested: &[Tag]) -> Result<BTreeMap<String, String>, String> {
    merged_tags(&BTreeMap::new(), requested)
}

/// The opaque pagination token for a page that ended on `arn`, of the listing narrowed by
/// `filters`.
///
/// The token identifies the QUERY it resumes and not merely where the last page stopped:
/// the page boundary (the last ARN returned) together with the filters the listing applied
/// beside its page controls ([`page_token_payload`]), hex-encoded so a client has nothing
/// to read in it and treats it as opaque, as the modelled `NextToken` requires. Encoding
/// that rather than storing a cursor server-side is what lets a token stay resolvable for
/// as long as the client holds it, with no per-caller state.
///
/// The filters belong in the token because a boundary means nothing without the query it
/// was cut from: the records matching a DIFFERENT filter may all sort below it, so
/// resuming that other listing after it would hand back no records and no successor — the
/// shape of a correct final page — and report a resource as empty while the store holds
/// its records. The page CONTROLS are not part of the identity: `MaxResults` is the
/// `pageSize` of the modelled `@paginated` trait, so a client may ask for a different page
/// size on every page and still walk one listing.
pub fn encode_page_token(arn: &str, filters: &[Option<&str>]) -> String {
    hex::encode(page_token_payload(arn, filters))
}

/// The byte that separates the fields inside a page token's decoded payload: the ASCII
/// unit separator, which no field can contain — a boundary is an ARN of the printable form
/// [`is_resource_arn`] accepts, and every member a listing of this service is narrowed by
/// is modelled with a `@pattern` of printable characters — so the fields read back exactly
/// as they were written.
const PAGE_TOKEN_FIELD_SEPARATOR: char = '\u{1f}';

/// What a page token encodes, before it is hex-encoded: the boundary ARN, then one field
/// per filter the listing applied, in the order it applies them.
///
/// Written once and read back through this very function ([`decode_page_token`] rebuilds
/// the payload the request presenting a token WOULD have been handed and compares), so no
/// token can be resumed by a query that would not have minted it. A listing narrowed by
/// nothing — one that takes no filter, or one whose request named none — encodes the bare
/// boundary, because that is the same listing of the same records: the whole listing of
/// that resource type in that region. Where one filter of several is absent its field is
/// EMPTY, which no present one can spell, every such member being modelled non-empty.
fn page_token_payload(arn: &str, filters: &[Option<&str>]) -> String {
    if filters.iter().all(Option::is_none) {
        return arn.to_string();
    }
    let mut payload = arn.to_string();
    for filter in filters {
        payload.push(PAGE_TOKEN_FIELD_SEPARATOR);
        payload.push_str(filter.unwrap_or_default());
    }
    payload
}

/// The ARN a `NextToken` SPELLS, for a listing of `resource_type` narrowed by `filters`:
/// the hex-decoded boundary, once the payload is the one [`encode_page_token`] mints for
/// those very filters and the boundary is an ARN of this service naming that resource
/// type.
///
/// `None` for not hex, not UTF-8, not an ARN of this service, an ARN naming another
/// resource type, or a payload minted for another query — a token one listing handed out
/// does not resume another, whether the other lists a different resource type, whose page
/// boundary is a key of a different map, or the same resource type under a different
/// filter, whose matching records may lie entirely on the far side of that boundary.
///
/// SHAPE ONLY, which is why this is private: whether this store ever ISSUED the token is
/// a question about the store, which only
/// [`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::resume_after`] can answer —
/// and that is the one route a listing resolves a token through, so no handler can accept
/// a boundary this store never minted.
fn decode_page_token(
    token: &str,
    resource_type: &str,
    filters: &[Option<&str>],
) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let boundary = decoded.split(PAGE_TOKEN_FIELD_SEPARATOR).next()?;
    (decoded == page_token_payload(boundary, filters) && is_resource_arn(boundary, resource_type))
        .then(|| boundary.to_string())
}

/// One resource id as this store mints them: the `resource` kind's id prefix followed by
/// `sequence` in that kind's hex width — the `hype-00000000` of
/// `hypervisor/hype-00000000`, the `bgw-0000000000000000` of
/// `gateway/bgw-0000000000000000`.
///
/// The prefix AND the width come from the kind ([`MintedResource::id_prefix`],
/// [`MintedResource::id_body_digits`]), so each resource is minted in the identifier form
/// that resource is served in rather than in one form shared across kinds.
///
/// The ONE rendering of a minted id — every mint of this store goes through it, and
/// [`minted_resource_sequence`] reads it back — so the form an id has and the form a
/// resumed page boundary is checked against cannot drift apart.
fn minted_resource_id(resource: MintedResource, sequence: u64) -> String {
    format!(
        "{}{sequence:0width$x}",
        resource.id_prefix(),
        width = resource.id_body_digits()
    )
}

/// The ARN this store mints for the `sequence`-th `resource` of `region`.
///
/// The ONE rendering of a minted ARN: the mint writes it
/// ([`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::mint_resource`]) and the
/// provenance check compares a token's boundary against it
/// ([`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::is_minted_resource_arn`]),
/// so what counts as minted is exactly what was minted.
fn minted_resource_arn(resource: MintedResource, region: &str, sequence: u64) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!(
            "{}/{}",
            resource.resource_type(),
            minted_resource_id(resource, sequence)
        ),
    )
}

/// The sequence a minted resource id encodes, or `None` for an id this store would not
/// have minted under `prefix`.
///
/// The inverse of [`minted_resource_id`], and strict about the spelling: the id has to be
/// EXACTLY what this store renders for that sequence, so an unpadded, over-padded or
/// upper-case spelling of the same number is not an id this store minted.
fn minted_resource_sequence(id: &str, resource: MintedResource) -> Option<u64> {
    let digits = id.strip_prefix(resource.id_prefix())?;
    let sequence = u64::from_str_radix(digits, 16).ok()?;
    (minted_resource_id(resource, sequence) == id).then_some(sequence)
}

/// The segments of an ARN of this service: `arn:(aws|aws-cn|aws-us-gov):backup-gateway:
/// <region>:<account>:<type>/<id>`.
struct ArnParts<'a> {
    region: &'a str,
    account: &'a str,
    resource_type: &'a str,
    resource_id: &'a str,
}

/// Split `arn` into its segments, or `None` for anything that is not an ARN of this
/// service at all (wrong field count, wrong literal or partition, another service, or no
/// `<type>/<id>` resource).
///
/// The ONE parse of this service's ARN form: [`is_resource_arn`], [`arn_resource_type`]
/// and [`arn_resource_id`] all read their answer off it, so the accepted form cannot be
/// changed in one copy only. It says nothing about whether each segment satisfies the
/// modelled pattern — that is `is_arn_segment`, which only the callers that need it ask.
fn arn_parts(arn: &str) -> Option<ArnParts<'_>> {
    let fields: Vec<&str> = arn.split(':').collect();
    let [literal, partition, service, region, account, resource] = fields.as_slice() else {
        return None;
    };
    if *literal != "arn" || !matches!(*partition, "aws" | "aws-cn" | "aws-us-gov") {
        return None;
    }
    if *service != ARN_SERVICE {
        return None;
    }
    let (resource_type, resource_id) = resource.split_once('/')?;
    Some(ArnParts {
        region,
        account,
        resource_type,
        resource_id,
    })
}

/// Whether `candidate` is an ARN of this service naming `resource_type`, in the form
/// the modelled `GatewayArn` and `ServerArn` patterns accept:
/// `arn:(aws|aws-cn|aws-us-gov):backup-gateway:<region>:<account>:<type>/<id>`, every
/// segment non-empty and drawn from `[a-zA-Z-0-9]`.
pub fn is_resource_arn(candidate: &str, resource_type: &str) -> bool {
    let Some(parts) = arn_parts(candidate) else {
        return false;
    };
    parts.resource_type == resource_type
        && [
            parts.region,
            parts.account,
            parts.resource_type,
            parts.resource_id,
        ]
        .into_iter()
        .all(is_arn_segment)
}

/// The resource-type segment of a backup-gateway ARN — the `gateway` of
/// `arn:aws:backup-gateway:<region>:<account>:gateway/<id>` — or `None` for anything
/// that is not an ARN of this service at all.
///
/// The one place an ARN is classified: the tagging operations take any of this
/// service's resources in a single `ResourceARN` member, so the resource they look the
/// ARN up as is read off the ARN rather than guessed from which store happens to hold
/// a record.
pub fn arn_resource_type(arn: &str) -> Option<&str> {
    arn_parts(arn).map(|parts| parts.resource_type)
}

/// The resource-id segment of a backup-gateway ARN — the `hype-…` of
/// `arn:aws:backup-gateway:<region>:<account>:hypervisor/hype-…` — or `None` for
/// anything that is not an ARN of this service at all.
///
/// The counterpart of [`arn_resource_type`]: an operation that reports a resource's bare
/// ID (`VirtualMachine$HypervisorId`) reads it off the ARN the store holds, so the id and
/// the record cannot name two different resources.
pub fn arn_resource_id(arn: &str) -> Option<&str> {
    arn_parts(arn).map(|parts| parts.resource_id)
}

/// Why an association between a gateway and a server could not be made.
///
/// Returned by
/// [`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::associate_gateway_to_server`],
/// which writes nothing on any of these arms. The store names the CONDITION and the
/// handler maps it to the modelled error its own operation declares, so the store does
/// not have to know which error shapes an operation carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssociationRefusal {
    /// The `GatewayArn` names no gateway this store holds in the region addressed.
    NoSuchGateway,
    /// The `ServerArn` names no hypervisor this store holds in the region addressed.
    NoSuchServer,
    /// The gateway is already associated with a DIFFERENT server, whose ARN this carries.
    AlreadyAssociated(String),
}

/// Why a gateway could not be disassociated from the server it backs up.
///
/// Returned by
/// [`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::disassociate_gateway_from_server`],
/// which writes nothing on either arm. As with [`AssociationRefusal`], the store names the
/// CONDITION and the handler maps it to the modelled error its own operation declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisassociationRefusal {
    /// The `GatewayArn` names no gateway this store holds.
    NoSuchGateway,
    /// The gateway is associated with no server, so there is no link to remove.
    NotAssociated,
}

/// How this service refuses a well-formed resource ARN that names no resource it holds.
///
/// Every such refusal is the modelled `ResourceNotFoundException`, answered at HTTP 400 —
/// the status AWS is recorded giving every client refusal of this service, whatever status
/// the error shape's `@httpError` declares — and carrying the `ErrorCode` that names WHICH
/// kind of resource was not found, paired with the fixed sentence AWS states for that kind.
/// All three parts of a refusal — the status, the code and the sentence ([`Self::message`])
/// — are read off this one value, so they cannot be chosen apart, and no handler composes a
/// sentence of its own for an ARN it could not resolve.
///
/// A sentence AWS states for one of these kinds carries NO part of the request: the ARN a
/// caller sent is a request value, not a part of the template, so only the arm for a
/// resource type this service does not name at all ([`Self::Other`]) words the condition
/// around the ARN that was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingResource {
    /// A `hypervisor/<id>` ARN with no hypervisor behind it.
    ///
    /// The store holds hypervisors ([`StoredHypervisor`]), so this arm is reached for a
    /// hypervisor ARN the store has no record under — never for one it does.
    Hypervisor,
    /// A hypervisor the store DOES hold, which carries no property mappings
    /// ([`StoredHypervisor::property_mappings`]) — so the resource that is missing is the
    /// mappings and not the hypervisor.
    ///
    /// Chosen by the operation that resolved the mappings, never by [`Self::for_arn`]:
    /// the ARN names a hypervisor in both cases, and only the record says which of the
    /// two is missing.
    PropertyMappings,
    /// A `gateway/<id>` ARN with no gateway behind it.
    ///
    /// Named directly by every operation that resolves a gateway, because the resource
    /// that is missing is then the gateway that operation was asked for, whatever resource
    /// type the ARN happens to spell; [`Self::for_arn`] reaches it for the tagging
    /// operations, which take any of this service's resources in one member.
    Gateway,
    /// A `vm/<id>` ARN naming no virtual machine this service reports, reached the same
    /// two ways [`Self::Gateway`] is.
    VirtualMachine,
    /// A resource type this service does not name at all — neither a gateway, nor a
    /// hypervisor, nor a virtual machine.
    ///
    /// Reached only through [`Self::for_arn`], for the tagging operations: they take any of
    /// this service's resources in ONE member, so an ARN of a type this service never
    /// mints reaches a handler and names no kind of resource whose refusal AWS states. It
    /// is the one arm that keeps the status its error shape declares and words the
    /// condition around the ARN, because there is no kind of resource to name instead.
    Other,
}

impl MissingResource {
    /// Which refusal `arn` takes, read off the resource type the ARN names.
    pub fn for_arn(arn: &str) -> Self {
        match arn_resource_type(arn) {
            Some(HYPERVISOR_RESOURCE_TYPE) => Self::Hypervisor,
            Some(GATEWAY_RESOURCE_TYPE) => Self::Gateway,
            Some(VIRTUAL_MACHINE_RESOURCE_TYPE) => Self::VirtualMachine,
            _ => Self::Other,
        }
    }

    /// `ResourceNotFoundException$ErrorCode` — "A description of which resource wasn't
    /// found" (`smithy_query(query="shape",shape="ResourceNotFoundException")`) — or
    /// `None` for a resource type this service states no code for.
    pub fn error_code(self) -> Option<&'static str> {
        match self {
            Self::Hypervisor => Some(HYPERVISOR_NOT_FOUND),
            Self::PropertyMappings => Some(MAPPINGS_NOT_FOUND),
            Self::Gateway => Some(GATEWAY_NOT_FOUND),
            Self::VirtualMachine => Some(VIRTUAL_MACHINE_NOT_FOUND),
            Self::Other => None,
        }
    }

    /// The sentence the refusal states.
    pub fn message(self, arn: &str) -> String {
        match self {
            Self::Hypervisor => HYPERVISOR_NOT_FOUND_MESSAGE.to_string(),
            Self::PropertyMappings => MAPPINGS_NOT_FOUND_MESSAGE.to_string(),
            Self::Gateway => GATEWAY_NOT_FOUND_MESSAGE.to_string(),
            Self::VirtualMachine => VIRTUAL_MACHINE_NOT_FOUND_MESSAGE.to_string(),
            Self::Other => resource_not_found_message(arn),
        }
    }

    /// The status this service answers the refusal with where it is NOT the one
    /// `ResourceNotFoundException`'s `@httpError` declares (404), and `None` where it is.
    ///
    /// Every kind of resource this service names is answered 400: that is the status AWS
    /// is recorded giving these refusals, and it is one answer for the condition rather
    /// than one per operation. Only [`Self::Other`] — an ARN of a resource type this
    /// service does not name, for which no refusal of AWS's is recorded — keeps the status
    /// the shape declares.
    pub fn status_override(self) -> Option<u16> {
        match self {
            Self::Hypervisor
            | Self::PropertyMappings
            | Self::Gateway
            | Self::VirtualMachine => Some(400),
            Self::Other => None,
        }
    }
}

/// The sentence this service states for an ARN of a resource type it does not name at all
/// and holds no record for, reached through [`MissingResource::message`] on its
/// [`MissingResource::Other`] arm.
///
/// Every resource type this service DOES name is worded by the fixed sentence AWS states
/// for that type ([`GATEWAY_NOT_FOUND_MESSAGE`], [`HYPERVISOR_NOT_FOUND_MESSAGE`],
/// [`VIRTUAL_MACHINE_NOT_FOUND_MESSAGE`]), which carries no part of the request; this one
/// names the ARN because there is no kind of resource to name instead.
pub fn resource_not_found_message(arn: &str) -> String {
    format!("The resource '{arn}' was not found.")
}

/// One `[a-zA-Z-0-9]+` ARN segment, as the modelled `GatewayArn` pattern spells it.
fn is_arn_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Whether `arn` names a resource of `region` — read off the region segment of the ARN
/// the store keys the record under, which is the region the request that minted it was
/// addressed to.
///
/// The question every listing of this service answers before it reports a record: a
/// resource is regional, so one minted in another region belongs to another region's
/// listing. `region` is normalised through [`arn_region`] exactly as a minted ARN's
/// segment was, so the two are compared in one spelling; anything that is not an ARN of
/// this service at all is in no region.
pub fn arn_is_in_region(arn: &str, region: &str) -> bool {
    arn_parts(arn).is_some_and(|parts| parts.region == arn_region(region))
}

/// Whether the request `parts` describes resolves `arn` AT ALL — the question every
/// operation of this service that resolves a single ARN answers before it reads or writes
/// the record it names.
///
/// A resource is regional, so the endpoint a request was addressed to
/// ([`request_region`]) decides which records that request can reach, exactly as it
/// decides which records a listing reports ([`arn_is_in_region`]): one store serves every
/// region a client addresses, so an ARN whose region segment names another region names
/// no resource of this endpoint — no more than one of another account would — and an
/// operation refuses it as the missing resource it is rather than reading or deleting a
/// record its own listings do not report.
pub fn arn_is_in_request_region(arn: &str, parts: &::http::request::Parts) -> bool {
    arn_is_in_region(arn, &request_region(parts))
}

/// The region to mint an ARN in, falling back to [`DEFAULT_REGION`] for anything that
/// would not satisfy the modelled ARN pattern's `[a-zA-Z-0-9]+` region segment or would not
/// fit inside a modelled ARN at all ([`MAX_ARN_REGION_LENGTH`]).
///
/// Both halves are the same question — whether an ARN built around this region is one the
/// operations that TAKE such an ARN can resolve — so the length is settled here beside the
/// character class rather than left to each minting path.
pub fn arn_region(region: &str) -> String {
    if is_arn_segment(region) && region.len() <= MAX_ARN_REGION_LENGTH {
        region.to_string()
    } else {
        DEFAULT_REGION.to_string()
    }
}

/// One activation key as the activation handshake prints it: `sequence`, rendered as
/// [`ACTIVATION_KEY_GROUPS`] groups of [`ACTIVATION_KEY_GROUP_SYMBOLS`] base-36 symbols.
///
/// The sequence is the store's own counter, as every ARN suffix this store mints is, so a
/// key is unique within a run and the sequence is reproducible. Nothing is taken from the
/// request: a key derived from what a client sent would let that client present a key the
/// service never issued.
fn minted_activation_key(sequence: u64) -> String {
    const SYMBOLS: &[u8; 36] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut remaining = sequence;
    let mut symbols = [b'0'; ACTIVATION_KEY_GROUPS * ACTIVATION_KEY_GROUP_SYMBOLS];
    // Least significant symbol last, so the rendering reads as a number and two keys minted
    // in a row differ in their final group.
    for symbol in symbols.iter_mut().rev() {
        let digit = usize::try_from(remaining % 36).unwrap_or_default();
        *symbol = SYMBOLS.get(digit).copied().unwrap_or(b'0');
        remaining /= 36;
    }
    symbols
        .chunks(ACTIVATION_KEY_GROUP_SYMBOLS)
        .map(|group| ::std::str::from_utf8(group).unwrap_or_default())
        .collect::<Vec<&str>>()
        .join("-")
}

/// The region this request was signed for, read from its SigV4 credential scope.
///
/// A resource is regional, and the region a client's ARN must carry is the one it
/// addressed the endpoint with — which on a signed request is the scope's third field.
/// An unsigned request (authentication off) carries none, so [`DEFAULT_REGION`] stands
/// in.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| arn_region(&scope.region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A stored instant as the modelled `Time` timestamp.
pub fn date_time(time: SystemTime) -> ::aws_smithy_types::DateTime {
    match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(since_epoch) => ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64()),
        Err(before_epoch) => {
            ::aws_smithy_types::DateTime::from_secs_f64(-before_epoch.duration().as_secs_f64())
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBackupgatewayBackuponpremisesV20210101State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBackupgatewayBackuponpremisesV20210101State>)
}
