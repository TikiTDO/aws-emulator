// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-workspaces-workspacesservice service.
//!
//! This module owns the WorkSpace lifecycle: the persisted record, the ordering
//! pagination depends on, and the four transitions `CreateWorkspaces`,
//! `StartWorkspaces`, `StopWorkspaces` and `TerminateWorkspaces` request. The
//! transitions live here rather than in one of the operation files because all
//! four batch operations apply the same rules to the same records.
//!
//! It also owns the IP access control groups `CreateIpGroup` mints,
//! `AuthorizeIpRules`, `RevokeIpRules`, `UpdateRulesOfIpGroup` and
//! `DeleteIpGroup` change and `DescribeIpGroups`
//! reports: the registry keyed by `GroupId`, the name index
//! that makes a group name unique across the account, and the creation order
//! `DescribeIpGroups` paginates over.
//!
//! It owns the connection-alias lifecycle on the same terms: one record per
//! alias, the creation order `DescribeConnectionAliases` paginates over, and the
//! transition `DeleteConnectionAlias` starts. `CreateConnectionAlias` starts
//! none: WorkSpaces answers a describe issued straight after it with the alias
//! already `CREATED` (see [`ConnectionAliasLifecycleState`]).
//!
//! It also owns the account-link records the six account-link operations share
//! (`CreateAccountLinkInvitation`, `AcceptAccountLinkInvitation`,
//! `RejectAccountLinkInvitation`, `DeleteAccountLinkInvitation`,
//! `GetAccountLink` and `ListAccountLinks`): one store, one status transition
//! table, so the writers and the readers cannot disagree about a link.
//!
//! It also owns the account-wide Bring Your Own License (BYOL) configuration
//! ([`AccountByolConfig`]) that `ModifyAccount` changes and `DescribeAccount`
//! reports: account-scoped rather than per-WorkSpace, so it is one value on the
//! state struct rather than a map.
//!
//! It also owns the WorkSpace BUNDLE store `DescribeWorkspaceBundles` reports:
//! the Amazon-provided catalog every account can create WorkSpaces from, which
//! exists before any caller arrives and so is seeded when the store is built,
//! plus the bundles an account owns.
//!
//! It also owns the DIRECTORY REGISTRY `RegisterWorkspaceDirectory` writes: the
//! directories this account has registered with WorkSpaces, which is what a
//! `CreateWorkspaces` request's `DirectoryId` resolves against. Unlike the
//! bundle catalog it starts empty — a directory exists in WorkSpaces only once
//! it has been registered.
//!
//! The tag set each of those resources carries is reached through the `tags`
//! submodule: `CreateTags`, `DescribeTags` and `DeleteTags` address a resource by
//! its `ResourceId` whatever its kind, so resolving an id to ONE tag set — the
//! very list the create that minted the resource wrote — belongs beside the
//! records rather than in any one handler.

mod tags;

pub use tags::{TagRefusal, MESSAGE_TAG_RESOURCE_NOT_FOUND};

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::IpAddr;
use std::time::{Duration, Instant, SystemTime};

use crate::ops::types::{
    AccountLink, ComputeType, ConnectionAlias, ConnectionAliasAssociation, IpRuleItem, RootStorage,
    Tag, UserStorage, Workspace, WorkspaceBundle, WorkspaceProperties, WorkspaceRequest,
    WorkspacesIpGroup, COMPUTE_VALUES,
};

/// How long a lifecycle transition the clock owes stays in its transient state.
///
/// `CreateWorkspaces`, `StartWorkspaces`, `StopWorkspaces` and
/// `TerminateWorkspaces` are documented as asynchronous: each returns while the
/// WorkSpace is still in a transient state (`PENDING`, `STARTING`, `STOPPING`,
/// `TERMINATING`). `DeleteConnectionAlias` is the same: the model's
/// `ConnectionAliasState` carries the transient `DELETING` beside the settled
/// `CREATED`. The span is deliberately non-zero so
/// a caller that describes a resource straight after the call still observes that
/// transient state, and short enough that a test settles quickly. It is a CHOSEN
/// span, not a duration AWS documents.
pub const TRANSITION_DELAY: Duration = Duration::from_millis(300);

/// The reserved `UserName` that marks a user-decoupled WorkSpace.
///
/// From the model's `WorkspaceRequest$UserName` doc: "The reserved keyword,
/// `[UNDEFINED]`, is used when creating user-decoupled WorkSpaces", and
/// `WorkspaceRequest$WorkspaceName`: "`WorkspaceName` is required if `UserName`
/// is `[UNDEFINED]` for user-decoupled WorkSpaces."
pub const USER_DECOUPLED_USER_NAME: &str = "[UNDEFINED]";

/// The page size a describe uses when the caller sends no `Limit`.
///
/// The model caps both `DescribeWorkspacesRequest$Limit` and
/// `DescribeConnectionAliasesRequest$Limit` at 25 (`range 1..25`), so a request
/// without one cannot be served a larger page either.
pub const DEFAULT_DESCRIBE_PAGE_SIZE: usize = 25;

// Per-item `ErrorCode` values: the per-item failure vocabulary.
//
// The batch operations (`CreateWorkspaces`, `StartWorkspaces`,
// `StopWorkspaces`, `TerminateWorkspaces`, and the reboot/rebuild/restore
// family once implemented) report a per-request failure INSIDE a 200 response,
// in `FailedCreateWorkspaceRequest$ErrorCode` / `FailedWorkspaceChangeRequest$ErrorCode`.
// Both members target `com.amazonaws.workspaces#ErrorType`, a free-form string
// with no enum: a per-item code, NOT the name of a modelled exception shape.
// An exception shape name can only travel as a top-level error (and these
// operations model no errors at all), so putting one in a per-item `ErrorCode`
// names nothing a client can branch on.
//
// The vocabulary AWS answers with is the dotted `<Condition>[.<Resource>]`
// form: the condition, spelled as the matching exception shape is but without
// its `Exception` suffix, qualified with the resource the item is about when
// the item names one, and left unqualified when the failure is about the
// request's own members or an account-wide quota. `ResourceNotFound.Workspace`
// (an unknown `WorkspaceId`), `ResourceNotFound.Directory` and
// `ResourceNotFound.Bundle` are recorded values; the rest follow the same form.
//
// This is ONE vocabulary for the whole service: the codes below are shared by
// every batch operation that can report a given condition, so two operations
// cannot report one condition two ways.

/// Per-item `ErrorType` code for a WorkSpace id that names no stored WorkSpace.
///
/// The recorded value: `StartWorkspaces`, `StopWorkspaces` and
/// `TerminateWorkspaces` all answer an unknown `WorkspaceId` with this code and
/// the fixed message [`MESSAGE_WORKSPACE_NOT_FOUND`].
pub const ERROR_CODE_WORKSPACE_NOT_FOUND: &str = "ResourceNotFound.Workspace";
/// Per-item `ErrorType` code for a WorkSpace whose current state forbids the
/// change. Follows the same form as the recorded code above; AWS publishes no
/// spelling for it.
pub const ERROR_CODE_WORKSPACE_INVALID_STATE: &str = "InvalidResourceState.Workspace";

/// The per-item `ErrorMessage` paired with [`ERROR_CODE_WORKSPACE_NOT_FOUND`].
///
/// A FIXED sentence rather than one built from the request: AWS answers the same
/// text for every unknown `WorkspaceId`, and the recorded responses carry this
/// message beside an id that appears nowhere in it.
pub const MESSAGE_WORKSPACE_NOT_FOUND: &str = "The specified WorkSpace could not be found.";

/// The name of the modelled error shape a bundle-name collision is reported
/// with, for a handler to lift into that TOP-LEVEL error.
///
/// Distinct from the per-item codes here: the bundle operations model their
/// errors, so a refusal there travels as an exception, not inside a 200 body,
/// and an exception shape name is the correct wire value for it.
pub const MODELED_ERROR_ALREADY_EXISTS: &str = "ResourceAlreadyExistsException";
/// The name of the modelled error shape an exceeded bundle bound is reported
/// with. See [`MODELED_ERROR_ALREADY_EXISTS`].
pub const MODELED_ERROR_LIMIT_EXCEEDED: &str = "ResourceLimitExceededException";

/// The most tags one WorkSpaces resource carries.
///
/// From the model's `CreateIpGroupRequest$Tags` doc: "The tags. Each WorkSpaces
/// resource can have a maximum of 50 tags."
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// Per-item `ErrorType` code for a create request naming a directory that is not
/// registered with WorkSpaces.
///
/// `FailedCreateWorkspaceRequest$ErrorCode` targets the model's `ErrorType`
/// ("The error code that is returned if the WorkSpace cannot be created") — a
/// per-item code VOCABULARY, not one of this service's exception shape names.
/// The model spells its other per-item code lists the same `Category.Resource`
/// way (`ASSOCIATION_ERROR_CODE_VALUES` in `ops::types` is
/// `DeploymentError.WorkspaceUnreachable`, `ValidationError.InsufficientMemory`,
/// …). `ResourceNotFound.Directory` and `ResourceNotFound.Bundle` are the codes
/// AWS answers an unresolvable directory and an unresolvable bundle with; the
/// three below them follow the same vocabulary for the refusals this store can
/// also report per element, none of which AWS publishes a spelling for.
pub const ERROR_CODE_DIRECTORY_NOT_FOUND: &str = "ResourceNotFound.Directory";
/// Per-item `ErrorType` code for a create request naming a bundle that is not stored.
pub const ERROR_CODE_BUNDLE_NOT_FOUND: &str = "ResourceNotFound.Bundle";
/// Per-item `ErrorType` code for a create request whose directory user already
/// holds a WorkSpace.
pub const ERROR_CODE_WORKSPACE_EXISTS: &str = "ResourceExists.WorkspaceUser";
/// Per-item `ErrorType` code for a request whose member combination is invalid.
/// No resource is missing, so the code names no resource. Shared by the create
/// and the change operations (a change item that names no `WorkspaceId`).
pub const ERROR_CODE_INVALID_PARAMETERS: &str = "InvalidParameterValues";
/// Per-item `ErrorType` code for a request the store has no room for. About an
/// account-wide quota, so it names no resource.
pub const ERROR_CODE_LIMIT_EXCEEDED: &str = "ResourceLimitExceeded";

/// The `ErrorMessage` reported beside [`ERROR_CODE_DIRECTORY_NOT_FOUND`].
///
/// AWS publishes no wording for this member; this is the text a recorded AWS
/// response carried, kept because it describes the refusal. What is load-bearing
/// is the CODE beside it.
pub const MESSAGE_DIRECTORY_NOT_FOUND: &str =
    "The specified directory could not be found in the specified region.";
/// The `ErrorMessage` reported beside [`ERROR_CODE_BUNDLE_NOT_FOUND`]. See
/// [`MESSAGE_DIRECTORY_NOT_FOUND`].
pub const MESSAGE_BUNDLE_NOT_FOUND: &str =
    "The specified bundle could not be found in the specified region.";

/// The `RunningMode` a WorkSpace is created with when the request names none.
///
/// `WorkspaceProperties$RunningMode` targets the model's `RunningMode` enum
/// (`AUTO_STOP`, `ALWAYS_ON`, `MANUAL`) and is OPTIONAL, so a create request may
/// omit it — but the created WorkSpace still runs in one mode, so the service
/// resolves the omission to a default. `ALWAYS_ON` is the value a recorded AWS
/// response reports for a request that carried no `WorkspaceProperties` at all,
/// and it is the only one of the three that needs no further member (`AUTO_STOP`
/// is paired with a timeout and `MANUAL` is allow-list gated per this
/// operation's doc: "The MANUAL running mode value is only supported by Amazon
/// WorkSpaces Core. Contact your account team to be allow-listed").
pub const DEFAULT_RUNNING_MODE: &str = "ALWAYS_ON";

/// The `RecycleMode` a WorkSpace is created with when the request names none.
///
/// `WorkspaceProperties$RecycleMode` is the member AWS answers with that this
/// crate declares itself (see `smithy-model/LOCAL-ADDITIONS.md`): it is optional
/// like every other property, so a create request may omit it — and a recorded
/// AWS response reports `DISABLED` for a request that carried no
/// `WorkspaceProperties` at all, so the service resolves the omission the same
/// way it resolves the running mode. `DISABLED` is also the OFF value of the
/// two-valued vocabulary this service spells everywhere else it reports a
/// switch (`DedicatedTenancySupportResultEnum`,
/// `CertificateBasedAuthStatusEnum` and `AGAModeForWorkSpaceEnum` are all
/// `DISABLED`/`ENABLED`), so a WorkSpace nobody asked to recycle reports the
/// off value rather than nothing.
pub const DEFAULT_RECYCLE_MODE: &str = "DISABLED";

/// The EFFECTIVE property set for a create request's `WorkspaceProperties`.
///
/// One resolution, used wherever this service reports a WorkSpace's properties:
/// what it persists for an accepted request, and what it echoes back for a
/// refused one. Resolving means filling in what the service decides for the
/// caller — the two switch-like properties a WorkSpace always has one value of,
/// [`DEFAULT_RUNNING_MODE`] for an omitted `RunningMode` and
/// [`DEFAULT_RECYCLE_MODE`] for an omitted `RecycleMode`. Every other member is
/// left exactly as submitted: a member the request did not carry stays absent
/// rather than being invented, because nothing in this emulator derives a volume
/// size, a compute type or a protocol for a WorkSpace. The result is never
/// absent, so a client that resubmits an echoed `WorkspaceRequest` resubmits the
/// properties the WorkSpace would have had.
pub fn effective_workspace_properties(
    requested: Option<&WorkspaceProperties>,
) -> WorkspaceProperties {
    let mut properties = requested.cloned().unwrap_or_default();
    if properties.running_mode.is_none() {
        properties.running_mode = Some(DEFAULT_RUNNING_MODE.to_string());
    }
    if properties.recycle_mode.is_none() {
        properties.recycle_mode = Some(DEFAULT_RECYCLE_MODE.to_string());
    }
    properties
}

/// The most WorkSpace directories this emulator keeps registered at once.
///
/// AWS caps the directories an account registers with a service quota and
/// reports an exceeded cap as `ResourceLimitExceededException`, which
/// `RegisterWorkspaceDirectory` models. The NUMBER is this emulator's own bound
/// on an in-memory store, and sits far above anything a test or client script
/// registers.
pub const MAX_REGISTERED_DIRECTORIES: usize = 50;

/// The most WorkSpaces this emulator stores at once.
///
/// AWS caps WorkSpaces with an adjustable service quota and reports an exceeded
/// cap as `ResourceLimitExceededException` ("Your resource limits have been
/// exceeded"), which `CreateWorkspaces` models. The NUMBER here is this
/// emulator's own bound, not an AWS quota: it keeps an in-memory store that only
/// a caller's terminate ever shrinks from growing without end, and sits far above
/// anything a test or a client script creates.
pub const MAX_STORED_WORKSPACES: usize = 2_000;

/// The most IP access control groups this emulator stores at once.
///
/// AWS caps IP access control groups with a service quota and reports an exceeded
/// cap as `ResourceLimitExceededException` ("Your resource limits have been
/// exceeded"), which `CreateIpGroup` models. The NUMBER here is this emulator's
/// own bound, not an AWS quota: it keeps an in-memory registry that only a
/// caller's delete ever shrinks from growing without end, and sits far above
/// anything a test or a client script creates.
pub const MAX_STORED_IP_GROUPS: usize = 1_000;

/// The most rules one IP access control group holds.
///
/// This service's model gives `IpRuleList` no length trait, so nothing refuses a
/// caller-sized rule list before it is persisted. AWS publishes a
/// rules-per-IP-access-control-group quota of 10 CIDR rules, and another AWS
/// service spells the same `IpRuleList` as `len 1..100`; the NUMBER here is this
/// emulator's own bound, set at the larger of the two so it cannot refuse a list
/// AWS would accept, and small enough that a stored group's SIZE is bounded
/// rather than caller-chosen.
pub const MAX_RULES_PER_IP_GROUP: usize = 100;

/// The longest free-text value this emulator stores for an IP access control
/// group: its `GroupName`, its `GroupDesc` and a rule's `ruleDesc`.
///
/// The model gives `IpGroupName`, `IpGroupDesc` and `IpRuleDesc` no length trait
/// either, so this is the emulator's own bound on what one record can retain. It
/// is the 255 characters the model's own description-shaped member carries
/// (`Tag$Value`, `len 0..255`), which is at or above every length AWS documents
/// for these members, so a value AWS accepts is not refused here.
pub const MAX_IP_GROUP_TEXT_CHARS: usize = 255;

/// The message a request naming an IP access control group this service does not
/// hold is refused with, beside `ResourceNotFoundException` (HTTP 400).
///
/// A FIXED sentence chosen by the ERROR CLASS rather than built from the request:
/// a recorded WorkSpaces response carries this text for a `GroupId` that appears
/// nowhere in it. The identifier the caller sent travels in the error shape's own
/// `ResourceId` member ("The ID of the resource that could not be found"), which
/// is where this API carries resource identity. Shared by every operation that
/// resolves a `GroupId`, so one condition is not reported two ways.
pub const MESSAGE_IP_GROUP_NOT_FOUND: &str = "Ip Group Id is not found.";

/// The message `CreateIpGroup` refuses a `GroupName` another stored group already
/// holds with, beside `ResourceAlreadyExistsException` (HTTP 400).
///
/// The sentence is fixed text up to and including the final separator, and its one
/// variable part is the name the request asked for, verbatim. The id of the group
/// that holds the name is deliberately NOT in it: the refusal names what the
/// caller sent, not a value this emulator generated.
pub fn message_ip_group_name_taken(group_name: &str) -> String {
    format!(
        "Encountered an exception while creating IP Group - group already exists for given group name - {group_name}"
    )
}

/// A per-item batch failure: the wire `ErrorCode` and `ErrorMessage` a
/// `FailedCreateWorkspaceRequest` / `FailedWorkspaceChangeRequest` carries.
///
/// Also the (code, message) pair the bundle store reports a refusal with, where
/// the code is a `MODELED_ERROR_*` shape name so a handler can lift it into the
/// top-level error that code names.
pub type ItemFailure = (&'static str, String);

/// The `ENABLED` value of the model's `DedicatedTenancySupportResultEnum`
/// (`ENABLED | DISABLED`), which also spells the SINGLE value of the input enum
/// `DedicatedTenancySupportEnum` that `ModifyAccount` accepts.
pub const BYOL_SUPPORT_ENABLED: &str = "ENABLED";

/// The `DISABLED` value of `DedicatedTenancySupportResultEnum`.
pub const BYOL_SUPPORT_DISABLED: &str = "DISABLED";

/// The account-wide Bring Your Own License (BYOL) configuration.
///
/// `ModifyAccount` ("Modifies the configuration of Bring Your Own License (BYOL)
/// for the specified account") is the only writer, and `DescribeAccount`
/// ("Retrieves a list that describes the configuration of Bring Your Own License
/// (BYOL) for the specified account") the reader. The fields are private so the
/// only way to change them is [`ComAmazonawsWorkspacesWorkspacesserviceState::modify_byol`],
/// which is where the rules about which combinations may be stored live.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AccountByolConfig {
    /// Whether BYOL is turned on for this account.
    enabled: bool,
    /// The management network CIDR block, once one has been configured.
    management_cidr_range: Option<String>,
}

impl AccountByolConfig {
    /// The `DedicatedTenancySupportResultEnum` value this configuration reports.
    ///
    /// An account nobody has modified has BYOL off, so it reports `DISABLED`
    /// rather than reporting nothing.
    pub fn dedicated_tenancy_support(&self) -> &'static str {
        if self.enabled {
            BYOL_SUPPORT_ENABLED
        } else {
            BYOL_SUPPORT_DISABLED
        }
    }

    /// The configured management network CIDR block, or `None` while no
    /// `ModifyAccount` call has set one.
    pub fn management_cidr_range(&self) -> Option<&str> {
        self.management_cidr_range.as_deref()
    }

    /// Whether BYOL is turned on.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// A description of the BYOL status, for the `Message` member both
    /// `DescribeAccount` ("The text message to describe the status of BYOL") and
    /// `ModifyAccount` ("The text message to describe the status of BYOL
    /// modification") carry.
    ///
    /// Computed from the stored configuration. AWS documents no wording for
    /// either member, so this TEXT is this emulator's own; what is load-bearing
    /// is that it describes the configuration this store holds.
    pub fn status_message(&self) -> String {
        match (self.enabled, self.management_cidr_range.as_deref()) {
            (true, Some(range)) => format!(
                "BYOL is enabled for this account; the management network interface uses the CIDR range {range}."
            ),
            (true, None) => "BYOL is enabled for this account.".to_string(),
            (false, Some(range)) => format!(
                "BYOL is not enabled for this account; the management network CIDR range {range} is reserved for it."
            ),
            (false, None) => "BYOL is not enabled for this account.".to_string(),
        }
    }
}

/// The `Owner` a bundle Amazon Web Services provides carries.
///
/// From `WorkspaceBundle$Owner`: "This is the account identifier of the owner,
/// or `AMAZON` if the bundle is provided by Amazon Web Services", and
/// `DescribeWorkspaceBundlesRequest$Owner`: "To describe the bundles provided by
/// Amazon Web Services, specify `AMAZON`."
pub const AMAZON_BUNDLE_OWNER: &str = "AMAZON";

/// The `WorkspaceBundleState` of a bundle WorkSpaces can be created from (one of
/// `WORKSPACE_BUNDLE_STATE_VALUES` in `ops::types`).
pub const BUNDLE_STATE_AVAILABLE: &str = "AVAILABLE";

/// The `BundleType` of an ordinary bundle, as opposed to the `STANDBY` type a
/// standby WorkSpace uses (one of `BUNDLE_TYPE_VALUES` in `ops::types`).
pub const BUNDLE_TYPE_REGULAR: &str = "REGULAR";

/// The page size `DescribeWorkspaceBundles` reports.
///
/// `DescribeWorkspaceBundlesRequest` models no `Limit` member, so the caller
/// cannot ask for a size and this number is the emulator's OWN choice. It is the
/// 25 the rest of this service's surface bounds a page at: every `Limit` member
/// is `range 1..25` and `BundleIdList` is `length 1..25`.
pub const DESCRIBE_BUNDLE_PAGE_SIZE: usize = 25;

/// The root volume size, in GiB, every bundle in the Amazon-provided catalog
/// carries.
///
/// AWS publishes its bundles with an 80 GB root volume paired with a 10, 50 or
/// 100 GB user volume, which is where this number and
/// [`catalog_user_capacity_gib`]'s three come from.
const CATALOG_ROOT_CAPACITY_GIB: u32 = 80;

/// The most bundles this emulator stores at once.
///
/// AWS caps the bundles an account owns with a service quota and reports an
/// exceeded cap as `ResourceLimitExceededException`, which
/// `CreateWorkspaceBundle` models. The NUMBER is this emulator's own bound on an
/// in-memory store, and sits far above the Amazon-provided catalog plus anything
/// a test or client script creates.
pub const MAX_STORED_BUNDLES: usize = 500;

/// The operational state of a stored WorkSpace.
///
/// Every variant renders to a value of the model's `WorkspaceState` enum (see
/// `WORKSPACE_STATE_VALUES` in `ops::types`); only the states this slice of the
/// lifecycle can reach are modelled here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceLifecycleState {
    /// The WorkSpace is being created.
    Pending,
    /// The WorkSpace is running.
    Available,
    /// The WorkSpace is starting up.
    Starting,
    /// The WorkSpace is being stopped.
    Stopping,
    /// The WorkSpace is stopped.
    Stopped,
    /// The WorkSpace is being deleted.
    Terminating,
    /// The WorkSpace has been deleted; it is dropped from the store.
    Terminated,
}

impl WorkspaceLifecycleState {
    /// The `WorkspaceState` value this state serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Available => "AVAILABLE",
            Self::Starting => "STARTING",
            Self::Stopping => "STOPPING",
            Self::Stopped => "STOPPED",
            Self::Terminating => "TERMINATING",
            Self::Terminated => "TERMINATED",
        }
    }
}

/// A lifecycle transition the clock owes: the state to move to, and when it is due.
#[derive(Debug, Clone, Copy)]
struct PendingTransition {
    to: WorkspaceLifecycleState,
    due: Instant,
}

/// One persisted WorkSpace.
#[derive(Debug, Clone)]
pub struct WorkspaceRecord {
    /// Creation sequence number. The only ordering `DescribeWorkspaces`
    /// paginates over, so a page boundary is stable across calls.
    pub seq: u64,
    pub workspace_id: String,
    pub directory_id: String,
    pub user_name: String,
    pub bundle_id: String,
    pub workspace_name: Option<String>,
    pub computer_name: Option<String>,
    pub ip_address: Option<String>,
    pub ipv6_address: Option<String>,
    pub state: WorkspaceLifecycleState,
    pub subnet_id: Option<String>,
    pub volume_encryption_key: Option<String>,
    pub user_volume_encryption_enabled: Option<bool>,
    pub root_volume_encryption_enabled: Option<bool>,
    pub workspace_properties: Option<WorkspaceProperties>,
    pub tags: Vec<Tag>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub created: SystemTime,
    pending: Option<PendingTransition>,
}

impl WorkspaceRecord {
    /// Enter `transient` now and owe `settled` once [`TRANSITION_DELAY`] elapses.
    fn begin(&mut self, transient: WorkspaceLifecycleState, settled: WorkspaceLifecycleState) {
        self.state = transient;
        self.pending = Some(PendingTransition {
            to: settled,
            due: Instant::now() + TRANSITION_DELAY,
        });
    }

    /// Apply the owed transition if `now` has reached its due instant.
    fn settle(&mut self, now: Instant) {
        let Some(pending) = self.pending else {
            return;
        };
        if now < pending.due {
            return;
        }
        self.state = pending.to;
        self.pending = None;
    }

    /// This record as the modelled `Workspace` structure.
    ///
    /// `SubnetId`, `IpAddress`, `ModificationStates`, `RelatedWorkspaces`,
    /// `DataReplicationSettings` and `StandbyWorkspacesProperties` stay absent:
    /// nothing in this emulator models a directory's subnets, an address
    /// allocation, a modification, a standby WorkSpace or data replication, so
    /// there is no state to report them from.
    pub fn to_workspace(&self) -> Workspace {
        Workspace {
            workspace_id: Some(self.workspace_id.clone()),
            directory_id: Some(self.directory_id.clone()),
            user_name: Some(self.user_name.clone()),
            ip_address: self.ip_address.clone(),
            ipv6_address: self.ipv6_address.clone(),
            state: Some(self.state.as_wire().to_string()),
            bundle_id: Some(self.bundle_id.clone()),
            subnet_id: self.subnet_id.clone(),
            error_message: self.error_message.clone(),
            error_code: self.error_code.clone(),
            computer_name: self.computer_name.clone(),
            volume_encryption_key: self.volume_encryption_key.clone(),
            user_volume_encryption_enabled: self.user_volume_encryption_enabled,
            root_volume_encryption_enabled: self.root_volume_encryption_enabled,
            workspace_name: self.workspace_name.clone(),
            workspace_properties: self.workspace_properties.clone(),
            modification_states: None,
            related_workspaces: None,
            data_replication_settings: None,
            standby_workspaces_properties: None,
        }
    }

    /// The directory slot this WorkSpace occupies.
    ///
    /// A directory user has at most one WorkSpace; the model's
    /// `WorkspaceRequest$UserName` doc states the user name "is not
    /// case-sensitive", so the comparison folds case. A user-decoupled WorkSpace
    /// (`UserName` = `[UNDEFINED]`) is identified by its `WorkspaceName` instead.
    fn occupancy_key(&self) -> String {
        occupancy_key(
            &self.directory_id,
            &self.user_name,
            self.workspace_name.as_deref(),
        )
    }
}

/// The directory slot a create request would occupy. See
/// [`WorkspaceRecord::occupancy_key`].
fn occupancy_key(directory_id: &str, user_name: &str, workspace_name: Option<&str>) -> String {
    if user_name == USER_DECOUPLED_USER_NAME {
        format!("{directory_id}\u{1f}name\u{1f}{}", workspace_name.unwrap_or(""))
    } else {
        format!(
            "{directory_id}\u{1f}user\u{1f}{}",
            user_name.to_lowercase()
        )
    }
}

/// One persisted IP access control group.
#[derive(Debug, Clone)]
pub struct IpGroupRecord {
    /// Creation sequence number. The only ordering `DescribeIpGroups`
    /// paginates over, so a page boundary is stable across calls.
    pub seq: u64,
    pub group_id: String,
    pub group_name: String,
    pub group_desc: Option<String>,
    pub user_rules: Vec<IpRuleItem>,
    pub tags: Vec<Tag>,
}

impl IpGroupRecord {
    /// This record as the modelled `WorkspacesIpGroup` structure.
    ///
    /// A group holding no rules reports NO `userRules` member rather than an
    /// empty list: that is what a recorded WorkSpaces `DescribeIpGroups`
    /// response carries for a group whose rule set is empty, and the member is
    /// optional in the model, so "no rules" and "an empty list of rules" are
    /// the same state and are reported the same way however the group reached
    /// it (created without rules, or had its rules replaced by none).
    pub fn to_ip_group(&self) -> WorkspacesIpGroup {
        WorkspacesIpGroup {
            group_id: Some(self.group_id.clone()),
            group_name: Some(self.group_name.clone()),
            group_desc: self.group_desc.clone(),
            user_rules: (!self.user_rules.is_empty()).then(|| self.user_rules.clone()),
        }
    }
}

/// The registration state of a stored WorkSpace directory.
///
/// Both variants render to a value of the model's `WorkspaceDirectoryState`
/// (`WORKSPACE_DIRECTORY_STATE_VALUES` in `ops::types`). Registration is the
/// only directory lifecycle this emulator stores, so `DEREGISTERING`,
/// `DEREGISTERED` and `ERROR` are not reachable: `DeregisterWorkspaceDirectory`
/// is still a scaffold stub, and nothing here fails a registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectoryRegistrationState {
    /// The directory is being registered.
    Registering,
    /// The directory is registered and WorkSpaces can be created in it.
    Registered,
}

impl DirectoryRegistrationState {
    /// The `WorkspaceDirectoryState` value this state serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Registering => "REGISTERING",
            Self::Registered => "REGISTERED",
        }
    }
}

/// The most connection aliases this emulator stores at once.
///
/// AWS caps connection aliases with an adjustable service quota and reports an
/// exceeded cap as `ResourceLimitExceededException` ("Your resource limits have
/// been exceeded"), which `CreateConnectionAlias` models. The NUMBER here is this
/// emulator's own bound, not an AWS quota: it keeps an in-memory store that only
/// a caller's delete ever shrinks from growing without end, and sits far above
/// anything a test or a client script creates.
pub const MAX_STORED_CONNECTION_ALIASES: usize = 500;

/// The `AssociationStatus` of a connection alias associated with no directory.
///
/// One of the model's `AssociationStatus` values (see
/// `ASSOCIATION_STATUS_VALUES` in `ops::types`), and the only one that describes
/// an alias no directory association names: the others each describe an
/// association that exists, is being made or is being undone.
pub const ASSOCIATION_STATUS_NOT_ASSOCIATED: &str = "NOT_ASSOCIATED";

/// The message a request naming a connection alias this service does not hold is
/// refused with, beside `ResourceNotFoundException` (HTTP 400).
///
/// A FIXED sentence chosen by the ERROR CLASS: it interpolates no request or state
/// value, and the alias the caller named travels in the error shape's own
/// `ResourceId` member instead. The apostrophe is U+2019, as the recorded
/// WorkSpaces response spells it.
pub const MESSAGE_CONNECTION_ALIAS_NOT_FOUND: &str = "The connection alias couldn\u{2019}t be found. \
     Make sure that you have the correct connection alias, and then try again.";

/// The operational state of a stored connection alias.
///
/// `Creating`, `Created` and `Deleting` each render to a value of the model's
/// `ConnectionAliasState` enum (see `CONNECTION_ALIAS_STATE_VALUES` in
/// `ops::types`). That enum has no "deleted" value, because a deleted alias is
/// not reported at all: `Removed` is the terminal state the clock moves a
/// deleting alias to, and the record is dropped from the store as it is reached.
///
/// A created alias is stored `Created`, not `Creating`: `CreateConnectionAlias`
/// reports no `State` at all (its result carries only `AliasId`), and a
/// `DescribeConnectionAliases` issued straight after it reports the alias
/// `CREATED` — WorkSpaces hands the alias back ready, so no clock-owed
/// transition is due for a create. `Creating` is kept because the model carries
/// the value and [`Self::as_wire`] stays total over the enum; the only transient
/// state this store enters is `Deleting`, which `DeleteConnectionAlias` starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionAliasLifecycleState {
    /// The alias is being created.
    Creating,
    /// The alias is ready to be associated with a directory.
    Created,
    /// The alias is being deleted.
    Deleting,
    /// The alias has been deleted; it is dropped from the store.
    Removed,
}

impl ConnectionAliasLifecycleState {
    /// The `ConnectionAliasState` value this state serializes as, or `None` for
    /// the terminal state the model has no value for.
    pub fn as_wire(self) -> Option<&'static str> {
        match self {
            Self::Creating => Some("CREATING"),
            Self::Created => Some("CREATED"),
            Self::Deleting => Some("DELETING"),
            Self::Removed => None,
        }
    }
}

/// A registration the clock owes: the state to move to, and when it is due.
#[derive(Debug, Clone, Copy)]
struct PendingRegistration {
    to: DirectoryRegistrationState,
    due: Instant,
}

/// One registered WorkSpace directory.
///
/// This is the resource a `WorkspaceRequest`'s `DirectoryId` resolves against:
/// the model points that member at "the Directory Service directory for the
/// WorkSpace. You can use DescribeWorkspaceDirectories to list the available
/// directories", so a WorkSpace can only be created in a directory this account
/// has registered.
#[derive(Debug, Clone)]
pub struct WorkspaceDirectoryRecord {
    /// Registration sequence number: the stable order
    /// [`ComAmazonawsWorkspacesWorkspacesserviceState::ordered_directories`]
    /// reports, and what a minted `wsd-` id is derived from.
    pub seq: u64,
    pub directory_id: String,
    pub state: DirectoryRegistrationState,
    pub subnet_ids: Vec<String>,
    /// One of the model's `Tenancy` values, when the request named one.
    pub tenancy: Option<String>,
    /// One of the model's `WorkspaceType` values, when the request named one.
    pub workspace_type: Option<String>,
    /// One of the model's `UserIdentityType` values, when the request named one.
    pub user_identity_type: Option<String>,
    pub directory_name: Option<String>,
    pub directory_description: Option<String>,
    pub self_service_enabled: Option<bool>,
    pub tags: Vec<Tag>,
    pub registered: SystemTime,
    pending: Option<PendingRegistration>,
}

impl WorkspaceDirectoryRecord {
    /// Enter `transient` now and owe `settled` once [`TRANSITION_DELAY`] elapses.
    fn begin(&mut self, transient: DirectoryRegistrationState, settled: DirectoryRegistrationState) {
        self.state = transient;
        self.pending = Some(PendingRegistration {
            to: settled,
            due: Instant::now() + TRANSITION_DELAY,
        });
    }

    /// Apply the owed transition if `now` has reached its due instant.
    fn settle(&mut self, now: Instant) {
        let Some(pending) = self.pending else {
            return;
        };
        if now < pending.due {
            return;
        }
        self.state = pending.to;
        self.pending = None;
    }
}

/// What a directory registration carries: the members
/// `RegisterWorkspaceDirectory` takes that this store holds.
///
/// `DirectoryId` is OPTIONAL in the model; a request that omits it registers a
/// directory WorkSpaces itself identifies, and the store mints the `wsd-` id for
/// it (`DirectoryId` is `^(d-[0-9a-f]{8,63}$)|(wsd-[0-9a-z]{8,63}$)$`).
#[derive(Debug, Clone, Default)]
pub struct NewDirectory {
    pub directory_id: Option<String>,
    pub subnet_ids: Vec<String>,
    pub tenancy: Option<String>,
    pub workspace_type: Option<String>,
    pub user_identity_type: Option<String>,
    pub directory_name: Option<String>,
    pub directory_description: Option<String>,
    pub self_service_enabled: Option<bool>,
    pub tags: Vec<Tag>,
}

/// Why a directory registration was refused, as the modelled error the handler
/// answers with.
///
/// A registration is a single resource, not a batch element, so a refusal is a
/// top-level modelled error rather than an [`ItemFailure`].
#[derive(Debug, Clone)]
pub enum DirectoryRefusal {
    /// The `DirectoryId` names a directory that is already registered —
    /// `ResourceAlreadyExistsException`.
    AlreadyRegistered(String),
    /// The store has no room for another registration —
    /// `ResourceLimitExceededException`.
    LimitExceeded(String),
}

impl DirectoryRefusal {
    /// The refusal's message, for the modelled error's `Message` member.
    pub fn message(&self) -> &str {
        match self {
            Self::AlreadyRegistered(message) | Self::LimitExceeded(message) => message,
        }
    }
}

/// A connection-alias transition the clock owes: the state to move to, and when.
#[derive(Debug, Clone, Copy)]
struct PendingAliasTransition {
    to: ConnectionAliasLifecycleState,
    due: Instant,
}

/// One persisted connection alias.
#[derive(Debug, Clone)]
pub struct ConnectionAliasRecord {
    /// Creation sequence number. The only ordering
    /// `DescribeConnectionAliases` paginates over, so a page boundary is stable
    /// across calls.
    pub seq: u64,
    pub alias_id: String,
    /// The connection string exactly as the caller spelled it.
    pub connection_string: String,
    pub owner_account_id: String,
    pub state: ConnectionAliasLifecycleState,
    /// The tags `CreateConnectionAlias` associated with the alias.
    pub tags: Vec<Tag>,
    pub created: SystemTime,
    pending: Option<PendingAliasTransition>,
}

impl ConnectionAliasRecord {
    /// Enter `transient` now and owe `settled` once [`TRANSITION_DELAY`] elapses.
    fn begin(
        &mut self,
        transient: ConnectionAliasLifecycleState,
        settled: ConnectionAliasLifecycleState,
    ) {
        self.state = transient;
        self.pending = Some(PendingAliasTransition {
            to: settled,
            due: Instant::now() + TRANSITION_DELAY,
        });
    }

    /// Apply the owed transition if `now` has reached its due instant.
    fn settle(&mut self, now: Instant) {
        let Some(pending) = self.pending else {
            return;
        };
        if now < pending.due {
            return;
        }
        self.state = pending.to;
        self.pending = None;
    }

    /// This record as the modelled `ConnectionAlias` structure.
    pub fn to_connection_alias(&self) -> ConnectionAlias {
        ConnectionAlias {
            connection_string: Some(self.connection_string.clone()),
            alias_id: Some(self.alias_id.clone()),
            state: self.state.as_wire().map(str::to_string),
            owner_account_id: Some(self.owner_account_id.clone()),
            associations: Some(self.to_associations()),
        }
    }

    /// The alias's `Associations` list, as a describe reports it.
    ///
    /// The member is always reported and the list is never empty:
    /// `ConnectionAliasAssociationList` is `len 1..25`, so an alias holding no
    /// directory association cannot be described with an empty list, and
    /// WorkSpaces describes such an alias with exactly one entry carrying
    /// `AssociationStatus` [`ASSOCIATION_STATUS_NOT_ASSOCIATED`].
    ///
    /// `AssociatedAccountId`, `ResourceId` and `ConnectionIdentifier` are the
    /// members that name an association's directory, the account that made it
    /// and its DNS identifier: they stay absent while there is no association to
    /// name one. `AssociateConnectionAlias` is the only operation that
    /// associates an alias with a directory and is still a scaffold stub, so
    /// every stored alias is in exactly that position.
    fn to_associations(&self) -> Vec<ConnectionAliasAssociation> {
        vec![ConnectionAliasAssociation {
            association_status: Some(ASSOCIATION_STATUS_NOT_ASSOCIATED.to_string()),
            associated_account_id: None,
            resource_id: None,
            connection_identifier: None,
        }]
    }
}

/// The page size `ListAccountLinks` reports when the caller sends no
/// `MaxResults`.
///
/// The model caps `ListAccountLinksRequest$MaxResults` at 25 (`range 1..25`), so
/// a request without one cannot be served a larger page either.
pub const DEFAULT_ACCOUNT_LINK_PAGE_SIZE: usize = 25;

/// The message an account-link request naming an account that does not exist in
/// the caller's organization is refused with.
///
/// AWS publishes no wording for `AccessDeniedException`; this is the text a
/// recorded WorkSpaces response carried, kept because it describes the refusal.
/// What is load-bearing is the error SHAPE (`AccessDeniedException`, HTTP 400)
/// and that nothing is persisted — not this sentence.
pub const ACCOUNT_LINK_ACCESS_DENIED_MESSAGE: &str =
    "Please ensure both accounts to be linked exist and belong to the same parent AWS Organization.";

/// The message every account-link `ValidationException` carries.
///
/// The account-link API states a refusal by its ERROR CLASS, not by the request:
/// recorded responses show three different operations refusing two different
/// members (`CreateAccountLinkInvitation$TargetAccountId`, `GetAccountLink$LinkId`,
/// `DeleteAccountLinkInvitation$LinkId`), each with a value of its own, all
/// answering ONE sentence that names neither the member, nor the value, nor the
/// constraint. What is load-bearing is that the text is the same for every such
/// request — this particular sentence is the one a recording carried.
pub const ACCOUNT_LINK_INVALID_REQUEST_MESSAGE: &str = "The request is invalid.";

/// The message every account-link `ResourceNotFoundException` carries.
///
/// Request-independent for the same reason: two different link ids produce one
/// identical sentence in the recordings. The identifier the caller sent is
/// reported in the shape's own `ResourceId` member ("The ID of the resource that
/// could not be found"), which is where this API carries resource identity —
/// never in the prose.
pub const ACCOUNT_LINK_NOT_FOUND_MESSAGE: &str = "Requested AccountLink could not be found.";

/// The message a `ConflictException` carries when the target account already
/// holds a link or an invitation.
///
/// The sentence the model itself documents for `ConflictException` ("The
/// TargetAccountId is already linked or invited."), and request-independent as
/// every account-link message is.
pub const ACCOUNT_LINK_ALREADY_LINKED_MESSAGE: &str =
    "The TargetAccountId is already linked or invited.";

/// The message a `ConflictException` carries when the stored link's current
/// status forbids the change asked for — answering an invitation that was already
/// answered, or deleting a link that is established.
///
/// AWS publishes no wording for this case, so what is load-bearing is the error
/// SHAPE and that the text is a property of the class rather than of the request:
/// it names neither the link the caller asked about nor the status it is in.
pub const ACCOUNT_LINK_STATUS_CONFLICT_MESSAGE: &str =
    "The account link's current status does not allow this change.";

/// The most account links this emulator stores at once.
///
/// This is the emulator's OWN bound on an in-memory store, not an AWS quota: a
/// caller that keeps inviting fresh target accounts would otherwise grow the
/// store without end. It sits far above anything a test or a client script
/// creates.
pub const MAX_STORED_ACCOUNT_LINKS: usize = 500;

/// The status of a stored account link.
///
/// Every variant renders to a value of the model's `AccountLinkStatusEnum` (see
/// `ACCOUNT_LINK_STATUS_ENUM_VALUES` in `ops::types`), and every value of that
/// enum has a variant here, so a `LinkStatusFilter` entry the model accepts can
/// always be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountLinkStatus {
    /// The link is established.
    Linked,
    /// Establishing the link failed.
    LinkingFailed,
    /// No link exists — the status a deleted invitation reports.
    LinkNotFound,
    /// The invitation is waiting for the target account to accept it.
    PendingAcceptanceByTargetAccount,
    /// The target account rejected the invitation.
    Rejected,
}

impl AccountLinkStatus {
    /// The `AccountLinkStatusEnum` value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Linked => "LINKED",
            Self::LinkingFailed => "LINKING_FAILED",
            Self::LinkNotFound => "LINK_NOT_FOUND",
            Self::PendingAcceptanceByTargetAccount => "PENDING_ACCEPTANCE_BY_TARGET_ACCOUNT",
            Self::Rejected => "REJECTED",
        }
    }

    /// The status a wire `AccountLinkStatusEnum` value names, or `None` when the
    /// value is not one the model declares.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "LINKED" => Some(Self::Linked),
            "LINKING_FAILED" => Some(Self::LinkingFailed),
            "LINK_NOT_FOUND" => Some(Self::LinkNotFound),
            "PENDING_ACCEPTANCE_BY_TARGET_ACCOUNT" => Some(Self::PendingAcceptanceByTargetAccount),
            "REJECTED" => Some(Self::Rejected),
            _ => None,
        }
    }

    /// Whether a link in this status holds its target account.
    ///
    /// `ConflictException` is documented as "The `TargetAccountId` is already
    /// linked or invited", so exactly the invited and the linked statuses hold
    /// the account; a rejected or failed link leaves it free to be invited again.
    fn holds_target_account(self) -> bool {
        matches!(self, Self::Linked | Self::PendingAcceptanceByTargetAccount)
    }
}

/// One persisted account link.
#[derive(Debug, Clone)]
pub struct AccountLinkRecord {
    /// Creation sequence number: the only ordering `ListAccountLinks`
    /// paginates over, so a page boundary is stable across calls.
    pub seq: u64,
    pub account_link_id: String,
    pub source_account_id: String,
    pub target_account_id: String,
    pub status: AccountLinkStatus,
    /// The `ClientToken` the creating request carried, which is what makes
    /// `CreateAccountLinkInvitation` idempotent.
    pub client_token: Option<String>,
}

impl AccountLinkRecord {
    /// This record as the modelled `AccountLink` structure.
    pub fn to_account_link(&self) -> AccountLink {
        AccountLink {
            account_link_id: Some(self.account_link_id.clone()),
            account_link_status: Some(self.status.as_wire().to_string()),
            source_account_id: Some(self.source_account_id.clone()),
            target_account_id: Some(self.target_account_id.clone()),
        }
    }

    /// The account on the other side of this link from `account_id`'s point of
    /// view, which is what `GetAccountLink`'s `LinkedAccountId` names.
    pub fn linked_account_id(&self, account_id: &str) -> &str {
        if self.source_account_id == account_id {
            &self.target_account_id
        } else {
            &self.source_account_id
        }
    }
}

/// Why an IP access control group could not be created.
///
/// Each variant names one of `CreateIpGroup`'s own modelled errors; the handler
/// turns it into that error, so the decision lives next to the state it is made
/// from and the wording next to the wire.
#[derive(Debug, Clone)]
pub enum IpGroupCreateError {
    /// A stored group already carries the requested name. Answered as
    /// `ResourceAlreadyExistsException`, whose message names the requested name
    /// and not the holding group's id (see [`message_ip_group_name_taken`]).
    NameTaken,
    /// The registry already holds [`MAX_STORED_IP_GROUPS`] groups. Answered as
    /// `ResourceLimitExceededException`.
    LimitExceeded,
}

/// Why a submitted `IpRuleList` was refused before any of it was persisted.
///
/// The model constrains neither `IpRuleList`'s length nor `IpRuleDesc`'s, so
/// every rule here is this emulator's own bound (see [`MAX_RULES_PER_IP_GROUP`]
/// and [`MAX_IP_GROUP_TEXT_CHARS`]) or the `ipRule` format the model documents.
/// Each variant carries the message of the `InvalidParameterValuesException`
/// ("One or more parameter values are not valid") the handler answers with, so
/// the decision lives next to the state it is made from and every operation
/// that takes a rule list answers the same refusal the same way.
#[derive(Debug, Clone)]
pub enum IpRuleListRefusal {
    /// The list holds more than [`MAX_RULES_PER_IP_GROUP`] rules; this is how
    /// many it holds.
    TooManyRules(usize),
    /// A rule's `ipRule` is not an IP address range in CIDR notation (see
    /// [`is_ip_range`]); this is the value sent.
    NotAnIpRange(String),
    /// A rule's `ruleDesc` is longer than [`MAX_IP_GROUP_TEXT_CHARS`]; this is
    /// how long it is.
    RuleDescTooLong(usize),
}

impl IpRuleListRefusal {
    /// The refusal's message, for the modelled error's `Message` member.
    pub fn message(&self) -> String {
        match self {
            Self::TooManyRules(count) => format!(
                "UserRules holds {count} rules; an IP access control group can hold at most {}.",
                MAX_RULES_PER_IP_GROUP
            ),
            Self::NotAnIpRange(ip_rule) => format!(
                "UserRules holds ipRule '{ip_rule}', which is not an IP address range in CIDR notation."
            ),
            Self::RuleDescTooLong(length) => format!(
                "UserRules ruleDesc is {length} characters long; at most {} are accepted.",
                MAX_IP_GROUP_TEXT_CHARS
            ),
        }
    }
}

/// Why rules could not be added to an IP access control group.
///
/// Each variant names one of `AuthorizeIpRules`' own modelled errors, so the
/// decision lives next to the state it is made from and the wording next to the
/// wire.
#[derive(Debug, Clone)]
pub enum IpRuleAuthorizeRefusal {
    /// No stored group carries the requested `GroupId` — answered as
    /// `ResourceNotFoundException`.
    NoSuchGroup,
    /// Adding the submitted rules would leave the group holding more than
    /// [`MAX_RULES_PER_IP_GROUP`] rules; this is how many it would hold.
    /// Answered as `ResourceLimitExceededException`.
    LimitExceeded(usize),
}

/// Check a submitted `UserRules` list before any of it is persisted.
///
/// Shared by every operation that takes an `IpRuleList` for an IP access
/// control group, so one list is accepted — or refused — identically whichever
/// operation it arrives on.
pub fn validate_ip_rules(user_rules: &[IpRuleItem]) -> Result<(), IpRuleListRefusal> {
    if user_rules.len() > MAX_RULES_PER_IP_GROUP {
        return Err(IpRuleListRefusal::TooManyRules(user_rules.len()));
    }
    for rule in user_rules {
        // `IpRuleItem$ipRule`: "The IP address range, in CIDR notation." A value
        // that is not an IP address range is refused rather than persisted as a
        // rule that could never match a client address.
        if let Some(ip_rule) = rule.ip_rule.as_deref() {
            if !is_ip_range(ip_rule) {
                return Err(IpRuleListRefusal::NotAnIpRange(ip_rule.to_string()));
            }
        }
        if let Some(rule_desc) = rule.rule_desc.as_deref() {
            let length = rule_desc.chars().count();
            if length > MAX_IP_GROUP_TEXT_CHARS {
                return Err(IpRuleListRefusal::RuleDescTooLong(length));
            }
        }
    }
    Ok(())
}

/// Whether `value` is an IP address range in CIDR notation, or a single address.
///
/// The model's `IpRuleItem$ipRule` doc says "The IP address range, in CIDR
/// notation", and constrains neither the family nor the prefix length, so both
/// IPv4 and IPv6 are accepted, as is a bare address (the range holding just that
/// address). Nothing is normalised: the value a caller sent is what
/// `DescribeIpGroups` reports back.
///
/// The ADDRESS is parsed by `std`'s own `IpAddr` parser rather than by hand; all
/// this adds is the prefix length, which is valid up to its family's address
/// width (32 bits for IPv4, 128 for IPv6). No crate is pulled in for those two
/// numbers.
pub fn is_ip_range(value: &str) -> bool {
    match value.split_once('/') {
        Some((address, prefix)) => {
            let Ok(address) = address.parse::<IpAddr>() else {
                return false;
            };
            let width = if address.is_ipv4() { 32u8 } else { 128u8 };
            prefix.parse::<u8>().is_ok_and(|bits| bits <= width)
        }
        // A bare address is the range holding just itself.
        None => value.parse::<IpAddr>().is_ok(),
    }
}

/// Whether `value` is a well-formed AWS account id.
///
/// The model's `AwsAccount` is `^\d{12}$` (see
/// `smithy_query(query="shape",shape="AwsAccount")`), which is exactly twelve
/// ASCII digits.
fn is_aws_account_id(value: &str) -> bool {
    value.len() == 12 && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// The key a connection string occupies in the store.
///
/// A fully qualified domain name is case-insensitive (RFC 4343), so two spellings
/// that differ only in case name one connection string; the record keeps the
/// spelling the caller sent.
fn connection_string_key(connection_string: &str) -> String {
    connection_string.to_lowercase()
}

/// Why a connection alias could not be created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionAliasCreateError {
    /// A stored alias already holds this connection string.
    AlreadyExists {
        /// The alias that holds it.
        alias_id: String,
    },
    /// The store already holds [`MAX_STORED_CONNECTION_ALIASES`] aliases.
    LimitExceeded,
}

/// One persisted WorkSpace bundle.
///
/// Volume sizes are held as GiB numbers and rendered into the modelled
/// `RootStorage$Capacity` / `UserStorage$Capacity` strings on the way out, so the
/// store never holds a value it cannot render.
#[derive(Debug, Clone)]
pub struct WorkspaceBundleRecord {
    /// Sequence number: the stable order [`ComAmazonawsWorkspacesWorkspacesserviceState::ordered_bundles`]
    /// reports and pagination resumes after.
    pub seq: u64,
    pub bundle_id: String,
    pub name: String,
    pub description: String,
    pub image_id: String,
    /// One of the model's `Compute` enum values (`COMPUTE_VALUES` in `ops::types`).
    pub compute_name: String,
    pub root_capacity_gib: u32,
    pub user_capacity_gib: u32,
    /// `AMAZON` for a bundle this service provides, otherwise the owning
    /// account id.
    pub owner: String,
    /// One of the model's `WorkspaceBundleState` values.
    pub state: &'static str,
    /// One of the model's `BundleType` values.
    pub bundle_type: &'static str,
    pub created: SystemTime,
    pub last_updated: SystemTime,
}

impl WorkspaceBundleRecord {
    /// This record as the modelled `WorkspaceBundle` structure.
    pub fn to_workspace_bundle(&self) -> WorkspaceBundle {
        WorkspaceBundle {
            bundle_id: Some(self.bundle_id.clone()),
            name: Some(self.name.clone()),
            owner: Some(self.owner.clone()),
            description: Some(self.description.clone()),
            image_id: Some(self.image_id.clone()),
            root_storage: Some(RootStorage {
                capacity: self.root_capacity_gib.to_string(),
            }),
            user_storage: Some(UserStorage {
                capacity: self.user_capacity_gib.to_string(),
            }),
            compute_type: Some(ComputeType {
                name: Some(self.compute_name.clone()),
            }),
            last_updated_time: Some(::aws_smithy_types::DateTime::from(self.last_updated)),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.created)),
            state: Some(self.state.to_string()),
            bundle_type: Some(self.bundle_type.to_string()),
        }
    }
}

/// Why an account-link request could not be served.
///
/// The six account-link operations model the same error shapes, so the store
/// reports the KIND of refusal and each handler lifts it into its own modelled
/// error enum.
#[derive(Debug, Clone)]
pub enum AccountLinkFailure {
    /// The caller may not link the account the request names, because that
    /// account does not exist in the caller's organization:
    /// `AccessDeniedException`.
    AccessDenied(String),
    /// The request is invalid: `ValidationException`.
    Validation(String),
    /// The link's current state forbids the change, or the target account is
    /// already linked or invited: `ConflictException`.
    Conflict(String),
    /// No stored link answers to the identifier: `ResourceNotFoundException`.
    /// Carries the identifier for that shape's `ResourceId` member.
    NotFound {
        resource_id: String,
        message: String,
    },
}

impl AccountLinkFailure {
    /// The `AccessDeniedException` for a target account that exists in no
    /// organization this caller is part of.
    fn access_denied() -> Self {
        Self::AccessDenied(ACCOUNT_LINK_ACCESS_DENIED_MESSAGE.to_string())
    }

    /// The `ResourceNotFoundException` for a link identifier that names no
    /// stored link.
    ///
    /// The identifier travels in `resource_id`, which the handler renders into
    /// the shape's `ResourceId` member; the message is the class's fixed
    /// [`ACCOUNT_LINK_NOT_FOUND_MESSAGE`], so two requests naming two unknown
    /// links are refused with one identical sentence.
    fn not_found(resource_id: &str) -> Self {
        Self::NotFound {
            resource_id: resource_id.to_string(),
            message: ACCOUNT_LINK_NOT_FOUND_MESSAGE.to_string(),
        }
    }

    /// The `ValidationException` for an account-link request this service
    /// refuses as invalid, worded as the class's fixed
    /// [`ACCOUNT_LINK_INVALID_REQUEST_MESSAGE`].
    fn invalid() -> Self {
        Self::Validation(ACCOUNT_LINK_INVALID_REQUEST_MESSAGE.to_string())
    }
}

/// The `BundleId` / `WorkspaceImageId` fragment that identifies the
/// Amazon-provided bundle of `compute`.
///
/// `BundleId` is `^wsb-[0-9a-z]{8,63}$` and `WorkspaceImageId` is
/// `^wsi-[0-9a-z]{9,63}$`; every `Compute` value is upper-case letters, digits
/// and underscores, so lower-casing it and dropping the underscores leaves the
/// `[0-9a-z]` the patterns allow, and the `amzn` prefix below carries the
/// shortest value (`VALUE`) past the nine characters the image id needs.
fn catalog_slug(compute: &str) -> String {
    format!("amzn{}", compute.to_lowercase().replace('_', ""))
}

/// The user volume size, in GiB, the Amazon-provided bundle of `compute` carries.
///
/// AWS publishes its bundles with the root volume [`CATALOG_ROOT_CAPACITY_GIB`]
/// paired with a 10, 50 or 100 GB user volume. WHICH of those three a given
/// compute type ships with is this emulator's own assignment — a function of the
/// compute type alone, so a bundle reports the same size on every read.
fn catalog_user_capacity_gib(compute: &str) -> u32 {
    match compute {
        "VALUE" => 10,
        "STANDARD" | "PERFORMANCE" => 50,
        _ => 100,
    }
}

/// Per-service state for com-amazonaws-workspaces-workspacesservice. A handler reaches it through [`service_state`].
///
/// The first three maps below are one store with two indexes over it, so they
/// are PRIVATE and every insert and removal goes through [`Self::insert_record`]
/// / [`Self::remove_record`]: an index that outlived the record it points at
/// would make a directory slot permanently unavailable. Readers use
/// [`Self::workspace`], [`Self::workspace_count`] and
/// [`Self::ordered_workspaces`].
///
/// The IP access control group maps are the same shape, and private for the same
/// reason: a name index that outlived its group would make that group name
/// permanently unusable. They are written only by [`Self::create_ip_group`],
/// [`Self::authorize_ip_group_rules`], [`Self::revoke_ip_group_rules`],
/// [`Self::replace_ip_group_rules`] and [`Self::delete_ip_group`], and read
/// through [`Self::ip_group`], [`Self::ip_group_count`] and
/// [`Self::ordered_ip_groups`].
///
/// The connection-alias maps are the same arrangement for a further resource,
/// with [`Self::insert_alias_record`] / [`Self::remove_alias_record`] as the only
/// writers and [`Self::connection_alias`], [`Self::connection_alias_count`] and
/// [`Self::ordered_connection_aliases`] as the readers.
///
/// The bundle store below is the same shape: one map plus an index by
/// `BundleId`, both written only through [`Self::insert_bundle`]. A freshly
/// built store already holds the Amazon-provided catalog (see [`Self::default`]),
/// because those bundles exist in every account before any caller arrives —
/// `DescribeWorkspaceBundles` reports them, it does not create them.
///
/// The directory registry, by contrast, starts EMPTY: a directory exists in
/// WorkSpaces only once `RegisterWorkspaceDirectory` has registered it, which is
/// why a `CreateWorkspaces` request naming an unregistered `DirectoryId` cannot
/// be satisfied.
#[derive(Debug)]
pub struct ComAmazonawsWorkspacesWorkspacesserviceState {
    /// Every stored WorkSpace, keyed by its `WorkspaceId`. Never larger than
    /// [`MAX_STORED_WORKSPACES`].
    workspaces: HashMap<String, WorkspaceRecord>,
    /// Which `WorkspaceId` holds each occupied directory slot (see
    /// [`WorkspaceRecord::occupancy_key`]), so deciding whether a slot is taken
    /// costs one lookup rather than a scan of the whole store under the shared
    /// lock.
    occupancy: HashMap<String, String>,
    /// `WorkspaceId` by creation sequence: the stable order
    /// [`Self::ordered_workspaces`] reports, held sorted so no call has to sort
    /// the store.
    order: BTreeMap<u64, String>,
    /// Monotonic counter behind both the minted `WorkspaceId` and the creation
    /// order [`Self::ordered_workspaces`] reports.
    pub next_workspace_seq: u64,
    /// Every stored IP access control group, keyed by its `GroupId`. Never
    /// larger than [`MAX_STORED_IP_GROUPS`].
    ip_groups: HashMap<String, IpGroupRecord>,
    /// Which `GroupId` holds each taken group name: a group name is unique
    /// across the account, so this index is what makes two creates on one name
    /// leave exactly one group behind.
    ip_group_names: HashMap<String, String>,
    /// `GroupId` by creation sequence: the stable order
    /// [`Self::ordered_ip_groups`] reports.
    ip_group_order: BTreeMap<u64, String>,
    /// Monotonic counter behind both the minted `GroupId` and the creation order
    /// [`Self::ordered_ip_groups`] reports.
    next_ip_group_seq: u64,
    /// Every stored connection alias, keyed by its `AliasId`. Never larger than
    /// [`MAX_STORED_CONNECTION_ALIASES`].
    connection_aliases: HashMap<String, ConnectionAliasRecord>,
    /// Which `AliasId` holds each taken connection string (see
    /// [`connection_string_key`]), so deciding whether a connection string is
    /// taken costs one lookup rather than a scan of the whole store under the
    /// shared lock.
    connection_strings: HashMap<String, String>,
    /// `AliasId` by creation sequence: the stable order
    /// [`Self::ordered_connection_aliases`] reports, held sorted so no call has
    /// to sort the store.
    alias_order: BTreeMap<u64, String>,
    /// Monotonic counter behind both the minted `AliasId` and the creation order
    /// [`Self::ordered_connection_aliases`] reports.
    pub next_connection_alias_seq: u64,
    /// Every stored account link by creation sequence: the stable order
    /// [`Self::ordered_account_links`] reports, held sorted so no call has to
    /// sort the store. Never larger than [`MAX_STORED_ACCOUNT_LINKS`].
    account_links: BTreeMap<u64, AccountLinkRecord>,
    /// Which creation sequence each `AccountLinkId` names, so a lookup by id
    /// costs one map hit rather than a scan. Kept in step with
    /// `account_links` by [`Self::insert_account_link`] /
    /// [`Self::remove_account_link`].
    account_link_ids: HashMap<String, u64>,
    /// Monotonic counter behind both the minted `AccountLinkId` and the
    /// creation order [`Self::ordered_account_links`] reports.
    next_account_link_seq: u64,
    /// The accounts that EXIST in this emulator's AWS Organization, held sorted
    /// so [`Self::organization_accounts`] reports a stable order.
    ///
    /// An account link can only be created between accounts that exist and
    /// belong to one parent organization, so this is the set
    /// [`Self::create_account_link_invitation`] resolves a `TargetAccountId`
    /// against — a well-formed id that names no account here is refused rather
    /// than linked. A fresh store holds exactly the emulator's own account (see
    /// [`Self::default`]): accounts are created by AWS Organizations, not by
    /// WorkSpaces, so nothing in THIS service's API adds one and
    /// [`Self::register_organization_account`] — this service's own tests' way
    /// to state that a second account exists — is its only writer.
    organization_accounts: BTreeSet<String>,
    /// The account-wide BYOL configuration. Account-scoped, so one value rather
    /// than a map: `ModifyAccount` writes it through [`Self::modify_byol`] and
    /// `DescribeAccount` reads it through [`Self::byol`].
    byol: AccountByolConfig,
    /// Every stored bundle by sequence number: the stable order
    /// [`Self::ordered_bundles`] reports and `DescribeWorkspaceBundles`
    /// paginates over. Never larger than [`MAX_STORED_BUNDLES`].
    bundles: BTreeMap<u64, WorkspaceBundleRecord>,
    /// Which sequence number each `BundleId` names, so looking a bundle up by id
    /// costs one lookup rather than a scan.
    bundle_ids: HashMap<String, u64>,
    /// Monotonic counter behind both the minted `BundleId` and the order
    /// [`Self::ordered_bundles`] reports.
    next_bundle_seq: u64,
    /// Every directory this account has registered, keyed by its `DirectoryId`.
    /// Never larger than [`MAX_REGISTERED_DIRECTORIES`]. A directory is REACHED
    /// through [`Self::directory`] and written only by
    /// [`Self::register_directory`]: it is the resource `CreateWorkspaces`
    /// resolves a request's `DirectoryId` against, so nothing else may add one.
    directories: HashMap<String, WorkspaceDirectoryRecord>,
    /// Monotonic counter behind both a minted `wsd-` `DirectoryId` and the
    /// registration order [`Self::ordered_directories`] reports.
    next_directory_seq: u64,
}

impl Default for ComAmazonawsWorkspacesWorkspacesserviceState {
    /// An empty WorkSpace store that already holds the Amazon-provided bundle
    /// catalog.
    ///
    /// The catalog is seeded HERE, when the engine builds this service's state,
    /// rather than by the first `DescribeWorkspaceBundles` call: those bundles
    /// are AWS's own and are describable in a brand-new account, so reporting
    /// them must not be a write. Its `CreationTime` is the instant the store was
    /// built, which is stable for the life of the emulator.
    ///
    /// BYOL starts off, which is what an account nobody has modified reports.
    ///
    /// The organization holds exactly one account — this emulator's own — because
    /// that is the only account a fresh emulator knows exists; an account link
    /// can therefore name no other until one is registered.
    fn default() -> Self {
        let mut state = Self {
            workspaces: HashMap::new(),
            occupancy: HashMap::new(),
            order: BTreeMap::new(),
            next_workspace_seq: 0,
            ip_groups: HashMap::new(),
            ip_group_names: HashMap::new(),
            ip_group_order: BTreeMap::new(),
            next_ip_group_seq: 0,
            connection_aliases: HashMap::new(),
            connection_strings: HashMap::new(),
            alias_order: BTreeMap::new(),
            next_connection_alias_seq: 0,
            account_links: BTreeMap::new(),
            account_link_ids: HashMap::new(),
            next_account_link_seq: 0,
            organization_accounts: BTreeSet::from([::engine::emulator_account_id().to_string()]),
            byol: AccountByolConfig::default(),
            bundles: BTreeMap::new(),
            bundle_ids: HashMap::new(),
            next_bundle_seq: 0,
            directories: HashMap::new(),
            next_directory_seq: 0,
        };
        state.seed_amazon_bundle_catalog(SystemTime::now());
        state
    }
}

impl ComAmazonawsWorkspacesWorkspacesserviceState {
    /// Materialise every lifecycle transition the clock already owes — of a
    /// WorkSpace, of a connection alias and of a directory registration alike —
    /// and drop the WorkSpaces whose termination and the connection aliases
    /// whose deletion have completed.
    ///
    /// Due at the same instant whether or not anybody called, so a reporting
    /// operation may run it.
    pub fn settle(&mut self) {
        self.settle_at(Instant::now());
    }

    /// [`Self::settle`] against an explicit clock reading.
    ///
    /// Handlers call `settle()`, which reads the real clock; this exists so a
    /// test can state the instant it is asking about instead of sleeping.
    pub fn settle_at(&mut self, now: Instant) {
        for record in self.workspaces.values_mut() {
            record.settle(now);
        }
        for record in self.directories.values_mut() {
            record.settle(now);
        }
        let completed: Vec<String> = self
            .workspaces
            .values()
            .filter(|record| record.state == WorkspaceLifecycleState::Terminated)
            .map(|record| record.workspace_id.clone())
            .collect();
        for workspace_id in completed {
            self.remove_record(&workspace_id);
        }

        for record in self.connection_aliases.values_mut() {
            record.settle(now);
        }
        let deleted: Vec<String> = self
            .connection_aliases
            .values()
            .filter(|record| record.state == ConnectionAliasLifecycleState::Removed)
            .map(|record| record.alias_id.clone())
            .collect();
        for alias_id in deleted {
            self.remove_alias_record(&alias_id);
        }
    }

    /// Every stored WorkSpace in creation order.
    pub fn ordered_workspaces(&self) -> Vec<&WorkspaceRecord> {
        self.order
            .values()
            .filter_map(|workspace_id| self.workspaces.get(workspace_id))
            .collect()
    }

    /// How many WorkSpaces are stored.
    pub fn workspace_count(&self) -> usize {
        self.workspaces.len()
    }

    /// The stored WorkSpace `workspace_id` names, if any.
    pub fn workspace(&self, workspace_id: &str) -> Option<&WorkspaceRecord> {
        self.workspaces.get(workspace_id)
    }

    /// The registered directory `directory_id` names, if any.
    ///
    /// A directory is reported from the instant `RegisterWorkspaceDirectory`
    /// registered it, whether the registration has settled to `REGISTERED` or is
    /// still `REGISTERING`: registration completing is a transition the clock
    /// owes, and AWS documents no rule that refuses a create while it is under
    /// way.
    pub fn directory(&self, directory_id: &str) -> Option<&WorkspaceDirectoryRecord> {
        self.directories.get(directory_id)
    }

    /// How many directories are registered.
    pub fn directory_count(&self) -> usize {
        self.directories.len()
    }

    /// Every registered directory in registration order.
    pub fn ordered_directories(&self) -> Vec<&WorkspaceDirectoryRecord> {
        let mut directories: Vec<&WorkspaceDirectoryRecord> = self.directories.values().collect();
        directories.sort_by_key(|record| record.seq);
        directories
    }

    /// Register one directory, or report why it could not be registered.
    ///
    /// `RegisterWorkspaceDirectory` is the only caller. Registration is
    /// documented as asynchronous ("returns before the WorkSpace directory is
    /// registered"), so the stored record starts `REGISTERING` and the clock owes
    /// it `REGISTERED`. A request that names no `DirectoryId` gets one minted
    /// from the registration sequence number, which cannot repeat within a run.
    pub fn register_directory(
        &mut self,
        request: &NewDirectory,
    ) -> Result<WorkspaceDirectoryRecord, DirectoryRefusal> {
        if let Some(directory_id) = request.directory_id.as_deref() {
            if self.directories.contains_key(directory_id) {
                return Err(DirectoryRefusal::AlreadyRegistered(format!(
                    "The directory {directory_id} is already registered with Amazon WorkSpaces."
                )));
            }
        }
        if self.directories.len() >= MAX_REGISTERED_DIRECTORIES {
            return Err(DirectoryRefusal::LimitExceeded(format!(
                "Your resource limits have been exceeded: at most {MAX_REGISTERED_DIRECTORIES} directories can be registered."
            )));
        }

        let seq = self.next_directory_seq();
        let directory_id = request
            .directory_id
            .clone()
            .unwrap_or_else(|| format!("wsd-{seq:08x}"));
        let mut record = WorkspaceDirectoryRecord {
            seq,
            directory_id: directory_id.clone(),
            state: DirectoryRegistrationState::Registering,
            subnet_ids: request.subnet_ids.clone(),
            tenancy: request.tenancy.clone(),
            workspace_type: request.workspace_type.clone(),
            user_identity_type: request.user_identity_type.clone(),
            directory_name: request.directory_name.clone(),
            directory_description: request.directory_description.clone(),
            self_service_enabled: request.self_service_enabled,
            tags: request.tags.clone(),
            registered: SystemTime::now(),
            pending: None,
        };
        record.begin(
            DirectoryRegistrationState::Registering,
            DirectoryRegistrationState::Registered,
        );
        self.directories.insert(directory_id, record.clone());
        Ok(record)
    }

    /// The next registration sequence number. Strictly increasing and never
    /// reset, as [`Self::next_seq`] is for WorkSpaces.
    fn next_directory_seq(&mut self) -> u64 {
        self.next_directory_seq += 1;
        self.next_directory_seq
    }

    /// The account-wide BYOL configuration `DescribeAccount` reports.
    pub fn byol(&self) -> &AccountByolConfig {
        &self.byol
    }

    /// Apply a `ModifyAccount` change to the BYOL configuration and report what
    /// is now stored.
    ///
    /// Every member the request omits LEAVES THE STORED VALUE ALONE — a
    /// `ModifyAccount` request carries only the parts of the configuration it
    /// changes. So `turn_on_byol` is "the request asked for BYOL to be ENABLED",
    /// not "the request's support value": `false` means the request named no
    /// support value and the stored flag is untouched, never that BYOL is turned
    /// off. Nothing here can turn BYOL off, which matches the model's input enum
    /// `DedicatedTenancySupportEnum` having the single value `ENABLED`. Which
    /// combinations may be stored at all is decided before this is called.
    pub fn modify_byol(
        &mut self,
        turn_on_byol: bool,
        management_cidr_range: Option<&str>,
    ) -> AccountByolConfig {
        if turn_on_byol {
            self.byol.enabled = true;
        }
        if let Some(range) = management_cidr_range {
            self.byol.management_cidr_range = Some(range.to_string());
        }
        self.byol.clone()
    }

    /// Store `record` and point both indexes at it.
    fn insert_record(&mut self, record: WorkspaceRecord) {
        self.occupancy
            .insert(record.occupancy_key(), record.workspace_id.clone());
        self.order.insert(record.seq, record.workspace_id.clone());
        self.workspaces.insert(record.workspace_id.clone(), record);
    }

    /// Drop the record `workspace_id` names and every index entry for it.
    fn remove_record(&mut self, workspace_id: &str) {
        if let Some(record) = self.workspaces.remove(workspace_id) {
            self.occupancy.remove(&record.occupancy_key());
            self.order.remove(&record.seq);
        }
    }

    /// Create one WorkSpace from `request`, or report why it could not be created.
    ///
    /// Every element of a `CreateWorkspaces` batch is judged here on its own, and
    /// a request this refuses persists NOTHING. The order of the checks is the
    /// order the refusals are reported in: the request's member combination
    /// first (it can be judged without touching the store), then the resources it
    /// names — the directory, then the bundle — then the directory slot it would
    /// occupy, then the store's own bound.
    ///
    /// Each refusal names the resource that actually failed to resolve: an
    /// unknown `BundleId` is reported as [`ERROR_CODE_BUNDLE_NOT_FOUND`] whatever
    /// else the element carries, and [`ERROR_CODE_DIRECTORY_NOT_FOUND`] is
    /// reported only for an unknown `DirectoryId`. An element whose directory AND
    /// bundle are both unresolvable has two failures to report in one entry; it
    /// is reported against the FIRST of them in the model's own member order
    /// (`WorkspaceRequest` declares `DirectoryId` before `BundleId`), so which
    /// one a client is told about does not depend on the order the lookups
    /// happen to run in.
    ///
    /// The returned record is a snapshot of what was persisted, so the caller can
    /// serialize it into `PendingRequests` without holding the map borrow.
    pub fn create_workspace(
        &mut self,
        request: &WorkspaceRequest,
    ) -> Result<WorkspaceRecord, ItemFailure> {
        if request.user_name == USER_DECOUPLED_USER_NAME && request.workspace_name.is_none() {
            return Err((
                ERROR_CODE_INVALID_PARAMETERS,
                format!(
                    "WorkspaceName is required when UserName is {USER_DECOUPLED_USER_NAME}."
                ),
            ));
        }

        // A WorkSpace lives in a directory registered with WorkSpaces: the model
        // points `WorkspaceRequest$DirectoryId` at "the Directory Service
        // directory for the WorkSpace. You can use DescribeWorkspaceDirectories
        // to list the available directories", so an id naming nothing registered
        // names no directory this account can create in.
        if !self.directories.contains_key(&request.directory_id) {
            return Err((
                ERROR_CODE_DIRECTORY_NOT_FOUND,
                MESSAGE_DIRECTORY_NOT_FOUND.to_string(),
            ));
        }

        // ... and it is created FROM a bundle: `WorkspaceRequest$BundleId` is
        // "the identifier of the bundle for the WorkSpace. You can use
        // DescribeWorkspaceBundles to list the available bundles", which is this
        // store's bundles — the Amazon-provided catalog plus the ones this
        // account created. An element whose directory resolved and whose bundle
        // did not is reported against the BUNDLE.
        if !self.bundle_ids.contains_key(&request.bundle_id) {
            return Err((
                ERROR_CODE_BUNDLE_NOT_FOUND,
                MESSAGE_BUNDLE_NOT_FOUND.to_string(),
            ));
        }

        let wanted = occupancy_key(
            &request.directory_id,
            &request.user_name,
            request.workspace_name.as_deref(),
        );
        if let Some(existing) = self
            .occupancy
            .get(&wanted)
            .and_then(|workspace_id| self.workspaces.get(workspace_id))
        {
            return Err((
                ERROR_CODE_WORKSPACE_EXISTS,
                format!(
                    "WorkSpace {} already exists for user {} in directory {}.",
                    existing.workspace_id, existing.user_name, existing.directory_id
                ),
            ));
        }

        // The store is bounded: refuse rather than grow without end.
        if self.workspaces.len() >= MAX_STORED_WORKSPACES {
            return Err((
                ERROR_CODE_LIMIT_EXCEEDED,
                format!(
                    "Your resource limits have been exceeded: at most {MAX_STORED_WORKSPACES} WorkSpaces can exist."
                ),
            ));
        }

        let seq = self.next_seq();
        let workspace_id = format!("ws-{seq:08x}");
        let mut record = WorkspaceRecord {
            seq,
            // The OS-visible name AWS assigns; its documented format "varies",
            // so it is derived from the minted id to stay stable across reads.
            computer_name: Some(format!("WSAMZN-{seq:08X}")),
            workspace_id: workspace_id.clone(),
            directory_id: request.directory_id.clone(),
            user_name: request.user_name.clone(),
            bundle_id: request.bundle_id.clone(),
            workspace_name: request.workspace_name.clone(),
            ip_address: None,
            ipv6_address: request.ipv6_address.clone(),
            state: WorkspaceLifecycleState::Pending,
            subnet_id: None,
            volume_encryption_key: request.volume_encryption_key.clone(),
            user_volume_encryption_enabled: request.user_volume_encryption_enabled,
            root_volume_encryption_enabled: request.root_volume_encryption_enabled,
            // The EFFECTIVE property set, resolved once here: every later read
            // of this WorkSpace reports the same properties the create resolved.
            workspace_properties: Some(effective_workspace_properties(
                request.workspace_properties.as_ref(),
            )),
            tags: request.tags.clone().unwrap_or_default(),
            error_code: None,
            error_message: None,
            created: SystemTime::now(),
            pending: None,
        };
        // Creation is asynchronous: PENDING now, AVAILABLE once the clock owes it.
        record.begin(
            WorkspaceLifecycleState::Pending,
            WorkspaceLifecycleState::Available,
        );
        self.insert_record(record.clone());
        Ok(record)
    }

    /// Start the WorkSpace named by `workspace_id`.
    ///
    /// `StartWorkspaces` documents that a WorkSpace can only be started from
    /// `STOPPED`; a start already under way is accepted as a no-op.
    pub fn request_start(&mut self, workspace_id: &str) -> Result<(), ItemFailure> {
        let record = self.record_mut(workspace_id)?;
        match record.state {
            WorkspaceLifecycleState::Stopped => {
                record.begin(
                    WorkspaceLifecycleState::Starting,
                    WorkspaceLifecycleState::Available,
                );
                Ok(())
            }
            WorkspaceLifecycleState::Starting => Ok(()),
            other => Err(invalid_state(workspace_id, other, "started")),
        }
    }

    /// Stop the WorkSpace named by `workspace_id`.
    ///
    /// A running WorkSpace stops; one already stopped or stopping is accepted as
    /// a no-op. Any other state (a create or start still under way, a
    /// termination) cannot be stopped.
    pub fn request_stop(&mut self, workspace_id: &str) -> Result<(), ItemFailure> {
        let record = self.record_mut(workspace_id)?;
        match record.state {
            WorkspaceLifecycleState::Available => {
                record.begin(
                    WorkspaceLifecycleState::Stopping,
                    WorkspaceLifecycleState::Stopped,
                );
                Ok(())
            }
            WorkspaceLifecycleState::Stopping | WorkspaceLifecycleState::Stopped => Ok(()),
            other => Err(invalid_state(workspace_id, other, "stopped")),
        }
    }

    /// Terminate the WorkSpace named by `workspace_id`.
    ///
    /// Terminating is allowed from every state this emulator stores; the record
    /// enters `TERMINATING` and [`Self::settle`] drops it once the clock owes it.
    pub fn request_terminate(&mut self, workspace_id: &str) -> Result<(), ItemFailure> {
        let record = self.record_mut(workspace_id)?;
        if record.state != WorkspaceLifecycleState::Terminating {
            record.begin(
                WorkspaceLifecycleState::Terminating,
                WorkspaceLifecycleState::Terminated,
            );
        }
        Ok(())
    }

    fn record_mut(&mut self, workspace_id: &str) -> Result<&mut WorkspaceRecord, ItemFailure> {
        self.workspaces
            .get_mut(workspace_id)
            .ok_or_else(not_found)
    }

    /// The next creation sequence number.
    ///
    /// Strictly increasing and never reset, and the minted `WorkspaceId` is a
    /// function of it alone, so no sequence number — and no id — can repeat
    /// within one run of the emulator.
    fn next_seq(&mut self) -> u64 {
        self.next_workspace_seq += 1;
        self.next_workspace_seq
    }

    /// Every stored IP access control group in creation order.
    pub fn ordered_ip_groups(&self) -> Vec<&IpGroupRecord> {
        self.ip_group_order
            .values()
            .filter_map(|group_id| self.ip_groups.get(group_id))
            .collect()
    }

    /// The stored IP access control group `group_id` names, if any.
    pub fn ip_group(&self, group_id: &str) -> Option<&IpGroupRecord> {
        self.ip_groups.get(group_id)
    }

    /// How many IP access control groups are stored.
    pub fn ip_group_count(&self) -> usize {
        self.ip_groups.len()
    }

    /// Create one IP access control group, or report why it could not be created.
    ///
    /// The name index is consulted and written under the same lock as the
    /// registry, so of any number of creates racing on one name exactly one is
    /// accepted and every other is told the name is taken.
    ///
    /// The returned record is a snapshot of what was persisted, so the caller
    /// can answer from it without holding the map borrow.
    pub fn create_ip_group(
        &mut self,
        group_name: &str,
        group_desc: Option<String>,
        user_rules: Vec<IpRuleItem>,
        tags: Vec<Tag>,
    ) -> Result<IpGroupRecord, IpGroupCreateError> {
        if self.ip_group_names.contains_key(group_name) {
            return Err(IpGroupCreateError::NameTaken);
        }
        // The registry is bounded: refuse rather than grow without end.
        if self.ip_groups.len() >= MAX_STORED_IP_GROUPS {
            return Err(IpGroupCreateError::LimitExceeded);
        }

        self.next_ip_group_seq += 1;
        let seq = self.next_ip_group_seq;
        // `IpGroupId` is `^wsipg-[0-9a-z]{8,63}$`: eight lowercase hex digits of
        // the sequence number, which is strictly increasing and never reset, so
        // no two groups in one run of the emulator can share an id.
        let group_id = format!("wsipg-{seq:08x}");
        let record = IpGroupRecord {
            seq,
            group_id: group_id.clone(),
            group_name: group_name.to_string(),
            group_desc,
            user_rules,
            tags,
        };
        self.ip_group_names
            .insert(record.group_name.clone(), group_id.clone());
        self.ip_group_order.insert(seq, group_id.clone());
        self.ip_groups.insert(group_id, record.clone());
        Ok(record)
    }

    /// Replace the rule set of the IP access control group `group_id` names.
    ///
    /// `UpdateRulesOfIpGroup` "Replaces the current rules of the specified IP
    /// access control group with the specified rules": the submitted list is
    /// written WHOLE, so nothing of the previous list survives — including when
    /// the submitted list is empty, which leaves the group holding no rules at
    /// all rather than its previous ones.
    ///
    /// The returned record is a snapshot of what was persisted, so the caller
    /// can answer from it without holding the map borrow. `None` means no
    /// stored group carries that id.
    pub fn replace_ip_group_rules(
        &mut self,
        group_id: &str,
        user_rules: Vec<IpRuleItem>,
    ) -> Option<IpGroupRecord> {
        let record = self.ip_groups.get_mut(group_id)?;
        record.user_rules = user_rules;
        Some(record.clone())
    }

    /// Add the submitted rules to the IP access control group `group_id` names.
    ///
    /// `AuthorizeIpRules` "Adds one or more rules to the specified IP access
    /// control group", so the group keeps the rules it already held: the
    /// submitted ones are added to them rather than replacing them.
    ///
    /// A rule is identified by its address range (`IpRuleItem$ipRule`, "The IP
    /// address range, in CIDR notation") and a group holds at most ONE rule per
    /// range, so authorizing a range the group already holds re-authorizes it
    /// — the submitted `ruleDesc` becomes the stored one — rather than storing
    /// the range twice. The rules just authorized are the most recently
    /// authorized ones and are reported FIRST, in the order submitted,
    /// followed by the rules the group already held in the order it held them.
    ///
    /// Nothing is written when the merged set would hold more rules than this
    /// emulator stores, so a refused authorize leaves the group as it was.
    pub fn authorize_ip_group_rules(
        &mut self,
        group_id: &str,
        user_rules: Vec<IpRuleItem>,
    ) -> Result<IpGroupRecord, IpRuleAuthorizeRefusal> {
        let record = self
            .ip_groups
            .get_mut(group_id)
            .ok_or(IpRuleAuthorizeRefusal::NoSuchGroup)?;

        let mut seen: BTreeSet<Option<String>> = BTreeSet::new();
        let mut merged: Vec<IpRuleItem> = Vec::with_capacity(user_rules.len() + record.user_rules.len());
        for rule in user_rules
            .into_iter()
            .chain(record.user_rules.iter().cloned())
        {
            if seen.insert(rule.ip_rule.clone()) {
                merged.push(rule);
            }
        }
        if merged.len() > MAX_RULES_PER_IP_GROUP {
            return Err(IpRuleAuthorizeRefusal::LimitExceeded(merged.len()));
        }

        record.user_rules = merged;
        Ok(record.clone())
    }

    /// Remove the rules naming the address ranges in `ip_rules` from the IP
    /// access control group `group_id` names.
    ///
    /// `RevokeIpRules` "Removes one or more rules from the specified IP access
    /// control group", and a rule is identified by its address range, so every
    /// rule the request does not name is kept, in the order the group held it.
    /// A range the group does not hold removes nothing: the group ends in the
    /// state the caller asked for either way, and the operation models no error
    /// for a rule a group does not hold (its `ResourceNotFoundException` is
    /// about the GROUP). `None` means no stored group carries that id.
    pub fn revoke_ip_group_rules(
        &mut self,
        group_id: &str,
        ip_rules: &[String],
    ) -> Option<IpGroupRecord> {
        let record = self.ip_groups.get_mut(group_id)?;
        let revoked: BTreeSet<&str> = ip_rules.iter().map(String::as_str).collect();
        record
            .user_rules
            // A rule carrying no `ipRule` names no range, so no revoke can name
            // it either: it is kept.
            .retain(|rule| {
                rule.ip_rule
                    .as_deref()
                    .is_none_or(|ip_rule| !revoked.contains(ip_rule))
            });
        Some(record.clone())
    }

    /// Delete the IP access control group `group_id` names.
    ///
    /// An IP access control group carries no lifecycle state in the model, so
    /// the deletion is complete when this returns: the group is dropped from
    /// the registry together with every index entry for it, which releases the
    /// name it held for a later create. `None` means no stored group carries
    /// that id.
    pub fn delete_ip_group(&mut self, group_id: &str) -> Option<IpGroupRecord> {
        let record = self.ip_groups.remove(group_id)?;
        self.ip_group_names.remove(&record.group_name);
        self.ip_group_order.remove(&record.seq);
        Some(record)
    }

    /// Every stored connection alias in creation order.
    pub fn ordered_connection_aliases(&self) -> Vec<&ConnectionAliasRecord> {
        self.alias_order
            .values()
            .filter_map(|alias_id| self.connection_aliases.get(alias_id))
            .collect()
    }

    /// How many connection aliases are stored.
    pub fn connection_alias_count(&self) -> usize {
        self.connection_aliases.len()
    }

    /// The stored connection alias `alias_id` names, if any.
    pub fn connection_alias(&self, alias_id: &str) -> Option<&ConnectionAliasRecord> {
        self.connection_aliases.get(alias_id)
    }

    /// Create one connection alias for `connection_string`, or report why it
    /// could not be created.
    ///
    /// The returned record is a snapshot of what was persisted, so the caller
    /// can read the minted `AliasId` off it without holding the map borrow.
    pub fn create_connection_alias(
        &mut self,
        connection_string: &str,
        tags: Vec<Tag>,
    ) -> Result<ConnectionAliasRecord, ConnectionAliasCreateError> {
        // `CreateConnectionAliasRequest$ConnectionString`: "After you create a
        // connection string, it is always associated to your Amazon Web Services
        // account" — so a second alias cannot take a connection string a stored
        // alias already holds.
        let wanted = connection_string_key(connection_string);
        if let Some(alias_id) = self.connection_strings.get(&wanted) {
            return Err(ConnectionAliasCreateError::AlreadyExists {
                alias_id: alias_id.clone(),
            });
        }

        // The store is bounded: refuse rather than grow without end.
        if self.connection_aliases.len() >= MAX_STORED_CONNECTION_ALIASES {
            return Err(ConnectionAliasCreateError::LimitExceeded);
        }

        let seq = self.next_alias_seq();
        let record = ConnectionAliasRecord {
            seq,
            // `ConnectionAliasId` is `^wsca-[0-9a-z]{8,63}$`; the sequence number
            // alone decides the id, so no two aliases can share one.
            alias_id: format!("wsca-{seq:08x}"),
            connection_string: connection_string.to_string(),
            owner_account_id: ::engine::emulator_account_id().to_string(),
            // The alias is handed back ready: a describe issued straight after
            // the create reports it CREATED, so the store owes the clock no
            // creation transition (see `ConnectionAliasLifecycleState`).
            state: ConnectionAliasLifecycleState::Created,
            tags,
            created: SystemTime::now(),
            pending: None,
        };
        self.insert_alias_record(record.clone());
        Ok(record)
    }

    /// Delete the connection alias named by `alias_id`.
    ///
    /// The alias enters `DELETING` and [`Self::settle`] drops it, together with
    /// its hold on the connection string, once the clock owes it. Deleting is
    /// accepted from every state this emulator stores, and a delete already
    /// under way is accepted as a no-op; `None` means no stored alias carries
    /// that id.
    pub fn request_delete_connection_alias(
        &mut self,
        alias_id: &str,
    ) -> Option<ConnectionAliasRecord> {
        let record = self.connection_aliases.get_mut(alias_id)?;
        if record.state != ConnectionAliasLifecycleState::Deleting {
            record.begin(
                ConnectionAliasLifecycleState::Deleting,
                ConnectionAliasLifecycleState::Removed,
            );
        }
        Some(record.clone())
    }

    /// Store `record` and point both alias indexes at it.
    fn insert_alias_record(&mut self, record: ConnectionAliasRecord) {
        self.connection_strings.insert(
            connection_string_key(&record.connection_string),
            record.alias_id.clone(),
        );
        self.alias_order.insert(record.seq, record.alias_id.clone());
        self.connection_aliases
            .insert(record.alias_id.clone(), record);
    }

    /// Drop the alias `alias_id` names and every index entry for it.
    fn remove_alias_record(&mut self, alias_id: &str) {
        if let Some(record) = self.connection_aliases.remove(alias_id) {
            self.connection_strings
                .remove(&connection_string_key(&record.connection_string));
            self.alias_order.remove(&record.seq);
        }
    }

    /// The next connection-alias creation sequence number.
    ///
    /// Strictly increasing and never reset, and the minted `AliasId` is a
    /// function of it alone, so no sequence number — and no id — can repeat
    /// within one run of the emulator.
    fn next_alias_seq(&mut self) -> u64 {
        self.next_connection_alias_seq += 1;
        self.next_connection_alias_seq
    }

    // --- Account links ---------------------------------------------------

    /// Every stored account link in creation order.
    pub fn ordered_account_links(&self) -> Vec<&AccountLinkRecord> {
        self.account_links.values().collect()
    }

    /// How many account links are stored.
    pub fn account_link_count(&self) -> usize {
        self.account_links.len()
    }

    /// The stored link `account_link_id` names, if any.
    pub fn account_link(&self, account_link_id: &str) -> Option<&AccountLinkRecord> {
        self.account_link_ids
            .get(account_link_id)
            .and_then(|seq| self.account_links.get(seq))
    }

    /// The stored link between `account_id` and `linked_account_id`.
    ///
    /// This is what `GetAccountLink`'s `LinkedAccountId` names: the account on
    /// the other side of the link from the caller's. The earliest created
    /// match is reported, so the answer does not depend on iteration order.
    pub fn account_link_by_linked_account(
        &self,
        account_id: &str,
        linked_account_id: &str,
    ) -> Option<&AccountLinkRecord> {
        self.account_links.values().find(|record| {
            (record.source_account_id == account_id || record.target_account_id == account_id)
                && record.linked_account_id(account_id) == linked_account_id
        })
    }

    /// Every account known to exist in this emulator's organization, in
    /// ascending id order.
    pub fn organization_accounts(&self) -> Vec<&str> {
        self.organization_accounts
            .iter()
            .map(String::as_str)
            .collect()
    }

    /// Whether `account_id` names an account that exists in this emulator's
    /// organization, and so is an account a link may name.
    pub fn is_organization_account(&self, account_id: &str) -> bool {
        self.organization_accounts.contains(account_id)
    }

    /// Record that `account_id` exists in this emulator's organization.
    ///
    /// Answers whether the set changed; an id that is not a well-formed AWS
    /// account id (the model's `AwsAccount` is `^\d{12}$`) names no account and
    /// is not added. WorkSpaces does not create accounts, so no handler in this
    /// crate calls this: it is how this service's own tests state that a second
    /// account exists, the way `RegisterWorkspaceDirectory` states that a
    /// directory does.
    pub fn register_organization_account(&mut self, account_id: &str) -> bool {
        if !is_aws_account_id(account_id) {
            return false;
        }
        self.organization_accounts.insert(account_id.to_string())
    }

    /// Invite `target_account_id` to link with `source_account_id`.
    ///
    /// The invitation is persisted awaiting the target account's own accept or
    /// reject call, which is the status `CreateAccountLinkInvitation` reports.
    ///
    /// A link is only creatable between accounts that EXIST and belong to one
    /// parent organization, so a `TargetAccountId` that names no account in
    /// [`Self::organization_accounts`] is refused with
    /// [`AccountLinkFailure::AccessDenied`] and persists nothing — a
    /// well-formed id is not by itself grounds for a link.
    pub fn create_account_link_invitation(
        &mut self,
        source_account_id: &str,
        target_account_id: &str,
        client_token: Option<&str>,
    ) -> Result<AccountLinkRecord, AccountLinkFailure> {
        // `ValidationException`: "You either haven't provided a TargetAccountId
        // or are using the same value for TargetAccountId and SourceAccountId."
        // (A missing `TargetAccountId` is refused before a handler runs — the
        // member is `required`.)
        if target_account_id == source_account_id {
            return Err(AccountLinkFailure::invalid());
        }

        // Both accounts have to exist and share one parent organization. The
        // target is resolved against the accounts this emulator knows exist, so
        // a well-formed id that names none of them is refused BEFORE anything is
        // stored, rather than linked on the strength of its format alone.
        if !self.is_organization_account(target_account_id)
            || !self.is_organization_account(source_account_id)
        {
            return Err(AccountLinkFailure::access_denied());
        }

        // `ClientToken` is "a string of up to 64 ASCII characters that Amazon
        // WorkSpaces uses to ensure idempotent creation", so a repeat of the
        // same token from the same source account reports the link that token
        // already created instead of creating a second one.
        if let Some(token) = client_token {
            if let Some(existing) = self.account_links.values().find(|record| {
                record.source_account_id == source_account_id
                    && record.client_token.as_deref() == Some(token)
            }) {
                if existing.target_account_id == target_account_id {
                    return Ok(existing.clone());
                }
                // A token already spent on another target is an invalid
                // request, stated as the class's one sentence: the token the
                // caller sent, and the target it was spent on, stay out of it.
                return Err(AccountLinkFailure::invalid());
            }
        }

        // `ConflictException`: "The TargetAccountId is already linked or invited."
        if self.account_links.values().any(|record| {
            record.source_account_id == source_account_id
                && record.target_account_id == target_account_id
                && record.status.holds_target_account()
        }) {
            return Err(AccountLinkFailure::Conflict(
                ACCOUNT_LINK_ALREADY_LINKED_MESSAGE.to_string(),
            ));
        }

        // The store is bounded: refuse rather than grow without end.
        if self.account_links.len() >= MAX_STORED_ACCOUNT_LINKS {
            return Err(AccountLinkFailure::Conflict(format!(
                "At most {MAX_STORED_ACCOUNT_LINKS} account links can exist."
            )));
        }

        let seq = self.mint_account_link_seq();
        let record = AccountLinkRecord {
            seq,
            // `LinkId` is `^link-.{8,24}$`; a fixed-width 16-hex rendering of
            // the sequence number sits inside that, and is a function of the
            // sequence alone, so no id repeats within one run.
            account_link_id: format!("link-{seq:016x}"),
            source_account_id: source_account_id.to_string(),
            target_account_id: target_account_id.to_string(),
            // The invitation waits for the target account to answer it; this is
            // not a transition the clock owes, so nothing settles it.
            status: AccountLinkStatus::PendingAcceptanceByTargetAccount,
            client_token: client_token.map(str::to_string),
        };
        self.insert_account_link(record.clone());
        Ok(record)
    }

    /// Accept the invitation `account_link_id` names: the link becomes `LINKED`.
    pub fn accept_account_link_invitation(
        &mut self,
        account_link_id: &str,
    ) -> Result<AccountLinkRecord, AccountLinkFailure> {
        self.answer_invitation(account_link_id, AccountLinkStatus::Linked)
    }

    /// Reject the invitation `account_link_id` names: the link becomes `REJECTED`.
    pub fn reject_account_link_invitation(
        &mut self,
        account_link_id: &str,
    ) -> Result<AccountLinkRecord, AccountLinkFailure> {
        self.answer_invitation(account_link_id, AccountLinkStatus::Rejected)
    }

    /// Delete the invitation `account_link_id` names.
    ///
    /// Only an invitation is deletable — the operation is documented as deleting
    /// "the account link invitation" — so an established link is refused as a
    /// conflict. The removed record is reported with `LINK_NOT_FOUND`, the only
    /// value of the model's status enum that describes a link that no longer
    /// exists.
    pub fn delete_account_link_invitation(
        &mut self,
        account_link_id: &str,
    ) -> Result<AccountLinkRecord, AccountLinkFailure> {
        let record = self.account_link_mut(account_link_id)?;
        if record.status == AccountLinkStatus::Linked {
            return Err(AccountLinkFailure::Conflict(
                ACCOUNT_LINK_STATUS_CONFLICT_MESSAGE.to_string(),
            ));
        }
        let mut removed = record.clone();
        self.remove_account_link(account_link_id);
        removed.status = AccountLinkStatus::LinkNotFound;
        Ok(removed)
    }

    /// Move an invitation still awaiting the target account into `answered`.
    ///
    /// A link that already carries an answer is refused rather than answered a
    /// second time: accepting or rejecting is the target account's one-time
    /// response to an invitation.
    fn answer_invitation(
        &mut self,
        account_link_id: &str,
        answered: AccountLinkStatus,
    ) -> Result<AccountLinkRecord, AccountLinkFailure> {
        let record = self.account_link_mut(account_link_id)?;
        if record.status != AccountLinkStatus::PendingAcceptanceByTargetAccount {
            return Err(AccountLinkFailure::Conflict(
                ACCOUNT_LINK_STATUS_CONFLICT_MESSAGE.to_string(),
            ));
        }
        record.status = answered;
        Ok(record.clone())
    }

    fn account_link_mut(
        &mut self,
        account_link_id: &str,
    ) -> Result<&mut AccountLinkRecord, AccountLinkFailure> {
        let seq = *self
            .account_link_ids
            .get(account_link_id)
            .ok_or_else(|| AccountLinkFailure::not_found(account_link_id))?;
        self.account_links
            .get_mut(&seq)
            .ok_or_else(|| AccountLinkFailure::not_found(account_link_id))
    }

    /// Store `record` and point the id index at it.
    fn insert_account_link(&mut self, record: AccountLinkRecord) {
        self.account_link_ids
            .insert(record.account_link_id.clone(), record.seq);
        self.account_links.insert(record.seq, record);
    }

    /// Drop the record `account_link_id` names and its index entry.
    fn remove_account_link(&mut self, account_link_id: &str) {
        if let Some(seq) = self.account_link_ids.remove(account_link_id) {
            self.account_links.remove(&seq);
        }
    }

    /// The next account-link creation sequence number. Strictly increasing and
    /// never reset, so no minted `AccountLinkId` repeats within one run.
    fn mint_account_link_seq(&mut self) -> u64 {
        self.next_account_link_seq += 1;
        self.next_account_link_seq
    }

    /// Every stored bundle in the order [`Self::ordered_bundles`] documents: the
    /// Amazon-provided catalog first, then the bundles this account created, in
    /// creation order.
    pub fn ordered_bundles(&self) -> Vec<&WorkspaceBundleRecord> {
        self.bundles.values().collect()
    }

    /// The stored bundle `bundle_id` names, if any.
    pub fn bundle(&self, bundle_id: &str) -> Option<&WorkspaceBundleRecord> {
        self.bundle_ids
            .get(bundle_id)
            .and_then(|seq| self.bundles.get(seq))
    }

    /// How many bundles are stored, the Amazon-provided catalog included.
    pub fn bundle_count(&self) -> usize {
        self.bundles.len()
    }

    /// Create one bundle owned by this account, or report why it could not be
    /// created.
    ///
    /// The writer side of the bundle store: `CreateWorkspaceBundle` is the
    /// operation that calls it (it is still a scaffold stub, so this service's
    /// own test suite is its only caller today). The returned record is a
    /// snapshot of what was persisted.
    pub fn create_bundle(
        &mut self,
        request: &NewBundle,
    ) -> Result<WorkspaceBundleRecord, ItemFailure> {
        let account = ::engine::emulator_account_id();
        if let Some(existing) = self
            .bundles
            .values()
            .find(|record| record.owner == account && record.name == request.name)
        {
            return Err((
                MODELED_ERROR_ALREADY_EXISTS,
                format!(
                    "Bundle {} already exists with the name {}.",
                    existing.bundle_id, existing.name
                ),
            ));
        }
        if self.bundles.len() >= MAX_STORED_BUNDLES {
            return Err((
                MODELED_ERROR_LIMIT_EXCEEDED,
                format!(
                    "Your resource limits have been exceeded: at most {MAX_STORED_BUNDLES} bundles can exist."
                ),
            ));
        }

        let seq = self.next_bundle_seq();
        let now = SystemTime::now();
        let record = WorkspaceBundleRecord {
            seq,
            bundle_id: format!("wsb-{seq:08x}"),
            name: request.name.clone(),
            description: request.description.clone(),
            image_id: request.image_id.clone(),
            compute_name: request.compute_name.clone(),
            root_capacity_gib: request.root_capacity_gib,
            user_capacity_gib: request.user_capacity_gib,
            owner: account.to_string(),
            state: BUNDLE_STATE_AVAILABLE,
            bundle_type: BUNDLE_TYPE_REGULAR,
            created: now,
            last_updated: now,
        };
        self.insert_bundle(record.clone());
        Ok(record)
    }

    /// Seed the Amazon-provided bundle catalog, one bundle per compute type.
    ///
    /// The catalog's MEMBERSHIP comes from the model: `COMPUTE_VALUES` in
    /// `ops::types` is the `Compute` enum, and AWS offers a bundle for each of
    /// those compute types. The volume sizes come from
    /// [`CATALOG_ROOT_CAPACITY_GIB`] and [`catalog_user_capacity_gib`], and the
    /// ids are a function of the compute type (see [`catalog_slug`]) so they are
    /// the same on every read and in every run.
    fn seed_amazon_bundle_catalog(&mut self, created: SystemTime) {
        for compute in COMPUTE_VALUES {
            let slug = catalog_slug(compute);
            let user_capacity_gib = catalog_user_capacity_gib(compute);
            let seq = self.next_bundle_seq();
            self.insert_bundle(WorkspaceBundleRecord {
                seq,
                bundle_id: format!("wsb-{slug}"),
                name: format!("Amazon {compute}"),
                description: format!(
                    "Amazon-provided WorkSpaces bundle: {compute} compute, a {CATALOG_ROOT_CAPACITY_GIB} GiB root volume and a {user_capacity_gib} GiB user volume."
                ),
                image_id: format!("wsi-{slug}"),
                compute_name: (*compute).to_string(),
                root_capacity_gib: CATALOG_ROOT_CAPACITY_GIB,
                user_capacity_gib,
                owner: AMAZON_BUNDLE_OWNER.to_string(),
                state: BUNDLE_STATE_AVAILABLE,
                bundle_type: BUNDLE_TYPE_REGULAR,
                created,
                last_updated: created,
            });
        }
    }

    /// Store `record` and point the `BundleId` index at it.
    fn insert_bundle(&mut self, record: WorkspaceBundleRecord) {
        self.bundle_ids.insert(record.bundle_id.clone(), record.seq);
        self.bundles.insert(record.seq, record);
    }

    /// The next bundle sequence number. Strictly increasing and never reset, as
    /// [`Self::next_seq`] is for WorkSpaces.
    fn next_bundle_seq(&mut self) -> u64 {
        self.next_bundle_seq += 1;
        self.next_bundle_seq
    }
}

/// What a bundle this account owns is created from: the members
/// `CreateWorkspaceBundle` takes, with the modelled `Capacity` strings already
/// parsed into the GiB numbers the store holds.
#[derive(Debug, Clone)]
pub struct NewBundle {
    pub name: String,
    pub description: String,
    pub image_id: String,
    pub compute_name: String,
    pub root_capacity_gib: u32,
    pub user_capacity_gib: u32,
}

/// The per-item failure for a WorkSpace id that names no stored WorkSpace.
///
/// The message is the service's FIXED sentence ([`MESSAGE_WORKSPACE_NOT_FOUND`]) and
/// so names no id: which id was asked about is already reported in the failure's
/// own `WorkspaceId` member.
fn not_found() -> ItemFailure {
    (
        ERROR_CODE_WORKSPACE_NOT_FOUND,
        MESSAGE_WORKSPACE_NOT_FOUND.to_string(),
    )
}

/// The per-item failure for a WorkSpace whose state forbids `action`.
fn invalid_state(
    workspace_id: &str,
    state: WorkspaceLifecycleState,
    action: &str,
) -> ItemFailure {
    (
        ERROR_CODE_WORKSPACE_INVALID_STATE,
        format!(
            "The WorkSpace {workspace_id} is in state {} and cannot be {action}.",
            state.as_wire()
        ),
    )
}

/// Encode "resume after this creation sequence number" as a `NextToken`.
///
/// The token names the LAST record a page reported, by the creation sequence
/// that record carries, rather than an offset into the list: a record removed
/// between two pages then shifts nothing, so a paginating client cannot skip an
/// item. The encoding is a fixed-width lowercase-hex rendering of that sequence
/// number — deterministic, so resuming needs no stored token, and 16 characters
/// long, inside the model's `PaginationToken` length of 1..2048. EVERY paginated
/// reader in this service shares it, each over its own creation sequence, so one
/// token format covers the service.
pub fn encode_page_token(after_seq: u64) -> String {
    format!("{after_seq:016x}")
}

/// Decode a `NextToken` produced by [`encode_page_token`], or `None` when the
/// token was not produced by it.
pub fn decode_page_token(token: &str) -> Option<u64> {
    // Fixed width is the load-bearing check; `from_str_radix` rejects anything
    // that is not hex, and the lowercase class rejects an upper-case spelling
    // this service never mints.
    if token.len() != 16 || !token.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    u64::from_str_radix(token, 16).ok()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsWorkspacesWorkspacesserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsWorkspacesWorkspacesserviceState>)
}
