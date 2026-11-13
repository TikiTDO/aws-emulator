// SPDX-License-Identifier: CC0-1.0

//! The Run Command slice: a command a caller sent, and the one invocation it holds per
//! managed node the send resolved.
//!
//! `SendCommand` writes a [`CommandRecord`] here, `GetCommandInvocation` reports the
//! [`CommandInvocationRecord`] one of its nodes holds and `ListCommands` reports the
//! commands a Region holds, narrowed by the criteria a request names
//! ([`CommandRecord::matches_filter`]), so what a send persisted is what a later read
//! reports. A command is keyed by `(region, command id)`: a command is sent to
//! the Systems Manager endpoint of one Region and identified there by the id this service
//! minted for it.
//!
//! "An invocation is a copy of a command sent to a specific managed node. … if a user
//! runs `SendCommand` against three managed nodes, then a command invocation is created
//! for each requested managed node ID" (`CommandInvocation`), so the invocations live ON
//! the command record, keyed by node id, rather than in a second store that could hold a
//! command's invocations after the command itself was dropped.
//!
//! Every invocation is minted `Pending` — "the command hasn't been sent to the managed
//! node" (`CommandInvocation$StatusDetails`) — and stays there: delivering a command is
//! the SSM Agent's own poll from the node, and this emulator runs no agent on any node,
//! so nothing picks an invocation up. Nothing therefore reports an exit code, output or
//! an execution window for a step that never ran; the command's counts are COUNTED from
//! the invocations the store holds ([`CommandRecord::completed_count`] and its siblings)
//! rather than asserted.
//!
//! The one write a read makes is the drop the clock already owes: Run Command retains a
//! command's history for [`COMMAND_HISTORY_RETENTION`], after which the command is gone
//! whether or not anybody called.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AlarmConfiguration, CloudWatchOutputConfig, Command, CommandInvocation, DocumentParameter,
    NotificationConfig, Target,
};
use super::ManagedNodeRecord;

/// The `CommandStatus` and `CommandInvocationStatus` value a command and its invocations
/// are minted with: "Pending: The command hasn't been sent to any managed nodes"
/// (`Command$StatusDetails`). From the modelled enums
/// (`crate::ops::types::COMMAND_STATUS_VALUES` and `COMMAND_INVOCATION_STATUS_VALUES`).
pub const COMMAND_STATUS_PENDING: &str = "Pending";

/// The `CommandInvocationStatus` values that are terminal, as `Command$StatusDetails`
/// lists them ("Success", "Delivery Timed Out", "Execution Timed Out", "Failed",
/// "Cancelled" — the last three spelled as the modelled enum spells them). What
/// [`CommandRecord::completed_count`] counts: "the number of targets for which the
/// command invocation reached a terminal state" (`Command$CompletedCount`).
pub const TERMINAL_INVOCATION_STATUSES: &[&str] = &["Success", "Cancelled", "Failed", "TimedOut"];

/// The `CommandInvocationStatus` values that count as errors: "the number of targets for
/// which the status is Failed or Execution Timed Out" (`Command$ErrorCount`).
pub const ERROR_INVOCATION_STATUSES: &[&str] = &["Failed", "TimedOut"];

/// The `StatusDetails` of an invocation the command never reached: "Delivery Timed Out:
/// The command wasn't delivered to the managed node before the delivery timeout expired"
/// (`CommandInvocation$StatusDetails`). What `Command$DeliveryTimedOutCount` counts.
pub const DELIVERY_TIMED_OUT_STATUS_DETAILS: &str = "Delivery Timed Out";

/// The `MaxConcurrency` a send that names none runs with: "The default value is 50"
/// (`SendCommandRequest$MaxConcurrency`).
pub const DEFAULT_MAX_CONCURRENCY: &str = "50";

/// The `MaxErrors` a send that names none runs with: "The default value is 0"
/// (`SendCommandRequest$MaxErrors`).
pub const DEFAULT_MAX_ERRORS: &str = "0";

/// The delivery timeout a send that names none runs with, in seconds: the Systems Manager
/// User Guide's "Understanding command timeout values" gives the `TimeoutSeconds` default
/// as 3,600 seconds, within the modelled 30..2592000 range.
pub const DEFAULT_TIMEOUT_SECONDS: i32 = 3600;

/// How long a command stays readable after it was requested: the Systems Manager User
/// Guide's "Run Command reports command history for 30 days" — after that the command and
/// its invocations are no longer reported.
pub const COMMAND_HISTORY_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// How many commands this store keeps at once, oldest evicted first.
///
/// The retention window above is measured in days, so it alone leaves the store growing
/// with one record per send for as long as the emulator runs. This is the emulator's own
/// guard — not an AWS quota — the same shape as the document store's total guard: past it
/// the oldest command's history is dropped early, so a caller sending commands in a loop
/// cannot grow this store without bound.
pub const MAX_COMMANDS_RETAINED: usize = 1_000;

/// The `ResponseCode` reported for an invocation that has run no step yet: a response code
/// is a step's own exit code, and an invocation nothing has started has none, which this
/// service reports as `-1` rather than as the zero that means "the step succeeded"
/// (`GetCommandInvocationResult$ResponseCode`).
pub const NO_RESPONSE_CODE: i32 = -1;

/// The target `Key` that names managed nodes by their node ids, and the value that names
/// every node of the Region: `Key=InstanceIds,Values=instance-id-1,…` and
/// `Key=InstanceIds,Values=*` (the `Target` doc's Run Command formats).
pub const TARGET_KEY_INSTANCE_IDS: &str = "InstanceIds";

/// The target `Key` that names every instance of the Region: `Key=AWS::EC2::Instance,
/// Values=*` (same doc).
pub const TARGET_KEY_EC2_INSTANCE: &str = "AWS::EC2::Instance";

/// The target `Values` entry that selects every node the key can select (same doc).
pub const TARGET_VALUE_ALL: &str = "*";

/// The target `Key` that names a resource group by name: `Key=resource-groups:Name,
/// Values=resource-group-name` (same doc).
pub const TARGET_KEY_RESOURCE_GROUP_NAME: &str = "resource-groups:Name";

/// The same selection spelled the way the Automation and Change Manager formats spell it:
/// `Key=ResourceGroup,Values=resource-group-name` (same doc).
pub const TARGET_KEY_RESOURCE_GROUP: &str = "ResourceGroup";

/// The target keys a send may name, in the `Target` doc's own order: the node-id forms,
/// the tag forms ([`super::TAG_FILTER_KEY_PREFIX`] and [`super::TAG_KEY_FILTER_KEY`]) and
/// the resource-group forms. What a refusal of an unsupported key lists.
pub fn supported_target_keys() -> Vec<&'static str> {
    vec![
        TARGET_KEY_INSTANCE_IDS,
        TARGET_KEY_EC2_INSTANCE,
        "tag:<tag-key>",
        super::TAG_KEY_FILTER_KEY,
        TARGET_KEY_RESOURCE_GROUP_NAME,
        TARGET_KEY_RESOURCE_GROUP,
    ]
}

/// One invocation of a command: the copy of it that one managed node holds.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct CommandInvocationRecord {
    /// The node this invocation was created for, and the key it is held under.
    pub instance_id: String,
    /// "The fully qualified host name of the managed node"
    /// (`CommandInvocation$InstanceName`), as the node itself reported it; `None` for a
    /// node that has reported none.
    pub instance_name: Option<String>,
    /// A `CommandInvocationStatus` value.
    pub status: String,
    /// The detailed status, in the spelling `CommandInvocation$StatusDetails` documents.
    pub status_details: String,
}

impl CommandInvocationRecord {
    /// The invocation a send creates for `node`: nothing has been delivered to the node
    /// yet, so it is `Pending`.
    pub fn pending(instance_id: impl Into<String>, instance_name: Option<String>) -> Self {
        Self {
            instance_id: instance_id.into(),
            instance_name,
            status: COMMAND_STATUS_PENDING.to_string(),
            status_details: COMMAND_STATUS_PENDING.to_string(),
        }
    }
}

/// A command as this store holds it: the request as Systems Manager received it, and the
/// invocations it created.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct CommandRecord {
    /// "A unique identifier for this command" (`Command$CommandId`), and part of the
    /// store key.
    pub command_id: String,
    /// The Region the command was sent to; the other part of its store key.
    pub region: String,
    /// The document the command runs, by the name the document is held under — never the
    /// ARN spelling a request may have named it by, since `Command$DocumentName` is a
    /// document NAME.
    pub document_name: String,
    /// The `DocumentVersion` the send resolved.
    pub document_version: String,
    /// The names of the document's steps, as the content declared them when the command
    /// was sent: what `GetCommandInvocation`'s `PluginName` names one of.
    pub step_names: Vec<String>,
    pub comment: Option<String>,
    /// The parameter values the send supplied, ordered so two reads agree.
    pub parameters: BTreeMap<String, Vec<String>>,
    /// The node ids the send named, exactly as it named them.
    pub instance_ids: Vec<String>,
    /// The search criteria the send named.
    pub targets: Vec<Target>,
    pub output_s3_bucket_name: Option<String>,
    pub output_s3_key_prefix: Option<String>,
    pub max_concurrency: String,
    pub max_errors: String,
    /// The delivery timeout the send ran with, in seconds.
    pub timeout_seconds: i32,
    pub service_role: Option<String>,
    pub notification_config: Option<NotificationConfig>,
    pub cloud_watch_output_config: Option<CloudWatchOutputConfig>,
    pub alarm_configuration: Option<AlarmConfiguration>,
    /// When the command was requested (`Command$RequestedDateTime`).
    pub requested_at: SystemTime,
    /// A `CommandStatus` value.
    pub status: String,
    /// The detailed status, in the spelling `Command$StatusDetails` documents.
    pub status_details: String,
    /// One invocation per node the send resolved, keyed by node id so a read of one
    /// node's invocation is a lookup and a listing walks a stable order.
    pub invocations: BTreeMap<String, CommandInvocationRecord>,
}

impl CommandRecord {
    /// When the command expires: "`ExpiresAfter` is calculated based on the total timeout
    /// for the overall command" (`Command$ExpiresAfter`), which is the delivery timeout
    /// the send ran with, measured from the request.
    pub fn expires_after(&self) -> SystemTime {
        self.requested_at + Duration::from_secs(self.timeout_seconds.max(0) as u64)
    }

    /// When this command stops being reported ([`COMMAND_HISTORY_RETENTION`]).
    pub fn retained_until(&self) -> SystemTime {
        self.requested_at + COMMAND_HISTORY_RETENTION
    }

    /// "The number of targets for the command" (`Command$TargetCount`): the nodes the
    /// send resolved, which is what it created an invocation for.
    pub fn target_count(&self) -> i32 {
        i32::try_from(self.invocations.len()).unwrap_or(i32::MAX)
    }

    /// "The number of targets for which the command invocation reached a terminal state"
    /// (`Command$CompletedCount`).
    pub fn completed_count(&self) -> i32 {
        self.count_invocations(|invocation| {
            TERMINAL_INVOCATION_STATUSES.contains(&invocation.status.as_str())
        })
    }

    /// "The number of targets for which the status is Failed or Execution Timed Out"
    /// (`Command$ErrorCount`).
    pub fn error_count(&self) -> i32 {
        self.count_invocations(|invocation| {
            ERROR_INVOCATION_STATUSES.contains(&invocation.status.as_str())
        })
    }

    /// "The number of targets for which the status is Delivery Timed Out"
    /// (`Command$DeliveryTimedOutCount`).
    pub fn delivery_timed_out_count(&self) -> i32 {
        self.count_invocations(|invocation| {
            invocation.status_details == DELIVERY_TIMED_OUT_STATUS_DETAILS
        })
    }

    fn count_invocations(&self, mut matches: impl FnMut(&CommandInvocationRecord) -> bool) -> i32 {
        i32::try_from(
            self.invocations
                .values()
                .filter(|invocation| matches(invocation))
                .count(),
        )
        .unwrap_or(i32::MAX)
    }

    /// The invocation `instance_id` holds of this command, or `None` when the command
    /// created none for that node.
    pub fn invocation(&self, instance_id: &str) -> Option<&CommandInvocationRecord> {
        self.invocations.get(instance_id)
    }

    /// Whether a `Status` filter value selects this command.
    ///
    /// A command carries two readings of its status: the modelled `CommandStatus`, and the
    /// detailed status `Command$StatusDetails` spells in words ("Delivery Timed Out"). The
    /// documented filter values cover both readings and carry no spaces
    /// ([`LIST_COMMANDS_STATUS_FILTER_VALUES`]), so a value selects this command when it
    /// names either of them.
    pub fn matches_status_filter(&self, value: &str) -> bool {
        self.status == value || self.status_details.replace(' ', "") == value
    }

    /// Whether the filter `key`=`value` selects this command.
    ///
    /// Each key narrows by one property of the command: when it was requested, the status
    /// it holds, whether that status is terminal, or the document it runs. A key this
    /// service does not know selects nothing — its caller refuses such a key before
    /// anything is asked about the history.
    pub fn matches_filter(&self, key: &str, value: &str) -> bool {
        match key {
            COMMAND_FILTER_KEY_INVOKED_AFTER => {
                parse_filter_instant(value).is_some_and(|at| self.requested_at >= at)
            }
            COMMAND_FILTER_KEY_INVOKED_BEFORE => {
                parse_filter_instant(value).is_some_and(|at| self.requested_at <= at)
            }
            COMMAND_FILTER_KEY_STATUS => self.matches_status_filter(value),
            // "Executing … currently still running" is every status that is not terminal;
            // "Complete … already completed" is every status that is.
            COMMAND_FILTER_KEY_EXECUTION_STAGE => match value {
                EXECUTION_STAGE_EXECUTING => !is_terminal_status(&self.status),
                EXECUTION_STAGE_COMPLETE => is_terminal_status(&self.status),
                _ => false,
            },
            COMMAND_FILTER_KEY_DOCUMENT_NAME => self.document_name == value,
            _ => false,
        }
    }

    /// The modelled `Command` for this command: every member either echoed from the
    /// request as it was received, resolved by the send, or counted from the invocations
    /// the store holds. A member the request left unset is left unset here too rather
    /// than invented.
    pub fn to_command(&self) -> Command {
        Command {
            command_id: Some(self.command_id.clone()),
            document_name: Some(self.document_name.clone()),
            document_version: Some(self.document_version.clone()),
            comment: self.comment.clone(),
            expires_after: Some(::aws_smithy_types::DateTime::from(self.expires_after())),
            parameters: (!self.parameters.is_empty()).then(|| {
                self.parameters
                    .iter()
                    .map(|(name, values)| (name.clone(), values.clone()))
                    .collect()
            }),
            instance_ids: (!self.instance_ids.is_empty()).then(|| self.instance_ids.clone()),
            targets: (!self.targets.is_empty()).then(|| self.targets.clone()),
            requested_date_time: Some(::aws_smithy_types::DateTime::from(self.requested_at)),
            status: Some(self.status.clone()),
            status_details: Some(self.status_details.clone()),
            // "(Deprecated) You can no longer specify this parameter. The system ignores
            // it" (`SendCommandRequest$OutputS3Region`), so nothing is stored for it and
            // nothing is reported.
            output_s3_region: None,
            output_s3_bucket_name: self.output_s3_bucket_name.clone(),
            output_s3_key_prefix: self.output_s3_key_prefix.clone(),
            max_concurrency: Some(self.max_concurrency.clone()),
            max_errors: Some(self.max_errors.clone()),
            target_count: self.target_count(),
            completed_count: self.completed_count(),
            error_count: self.error_count(),
            delivery_timed_out_count: self.delivery_timed_out_count(),
            service_role: self.service_role.clone(),
            notification_config: self.notification_config.clone(),
            cloud_watch_output_config: self.cloud_watch_output_config.clone(),
            timeout_seconds: Some(self.timeout_seconds),
            alarm_configuration: self.alarm_configuration.clone(),
            // "The CloudWatch alarm that was invoked by the command"
            // (`Command$TriggeredAlarms`): no alarm has been evaluated for a command
            // nothing has delivered, so none was invoked.
            triggered_alarms: None,
        }
    }

    /// The modelled `CommandInvocation` for one node's invocation of this command — the
    /// command's own members plus that invocation's status.
    pub fn to_command_invocation(&self, invocation: &CommandInvocationRecord) -> CommandInvocation {
        CommandInvocation {
            command_id: Some(self.command_id.clone()),
            instance_id: Some(invocation.instance_id.clone()),
            instance_name: invocation.instance_name.clone(),
            comment: self.comment.clone(),
            document_name: Some(self.document_name.clone()),
            document_version: Some(self.document_version.clone()),
            requested_date_time: Some(::aws_smithy_types::DateTime::from(self.requested_at)),
            status: Some(invocation.status.clone()),
            status_details: Some(invocation.status_details.clone()),
            // The trace output, the output URLs and the per-step results come from a step
            // that ran on the node; nothing has been delivered, so there are none.
            trace_output: None,
            standard_output_url: None,
            standard_error_url: None,
            command_plugins: None,
            service_role: self.service_role.clone(),
            notification_config: self.notification_config.clone(),
            cloud_watch_output_config: self.cloud_watch_output_config.clone(),
        }
    }
}

/// The ids of the nodes `region` holds that one target selects, or `Err(key)` when the
/// target's key is not one a command targets nodes by ([`supported_target_keys`]).
///
/// A target is CRITERIA, not an identity: "an array of search criteria that targets
/// managed nodes using a Key,Value combination that you specify" (`SendCommand`'s
/// `Targets`), so criteria that describe no registered node select none rather than
/// refusing the send. The values of one target UNION (the `Target$Values` doc: "if you
/// specified `tag:ServerRole`, you could specify `value:WebServer` to run a command on
/// instances that include EC2 tags of `ServerRole,WebServer`"), and `*` selects every
/// node the key can select.
///
/// Matching a node against a key and its values is the one node matcher this service
/// already has ([`ManagedNodeRecord::matches_filter`]), so the node-id and tag forms are
/// not matched a second way here.
pub fn nodes_matching_target<'a>(
    store: &'a super::ComAmazonawsSsmAmazonssmState,
    region: &'a str,
    target: &Target,
) -> Result<Vec<&'a str>, String> {
    let key = target.key.as_deref().unwrap_or_default();
    let values: &[String] = target.values.as_deref().unwrap_or_default();
    let every_node = values.iter().any(|value| value == TARGET_VALUE_ALL);
    let matched = |filter_key: &'static str, ec2_only: bool| -> Vec<&'a str> {
        store
            .managed_nodes_in_region(region)
            .filter(move |(_, node)| {
                !ec2_only
                    || node.properties.resource_type.as_deref()
                        == Some(super::RESOURCE_TYPE_EC2_INSTANCE)
            })
            .filter(move |(_, node)| {
                every_node || node.matches_filter(filter_key, values, super::OPERATOR_EQUAL)
            })
            .map(|(id, _)| id)
            .collect()
    };

    match key {
        // The node-id form selects by identity; `AWS::EC2::Instance` is that same
        // selection narrowed to the nodes that ARE EC2 instances.
        TARGET_KEY_INSTANCE_IDS => Ok(matched(TARGET_KEY_INSTANCE_IDS, false)),
        TARGET_KEY_EC2_INSTANCE => Ok(matched(TARGET_KEY_INSTANCE_IDS, true)),
        // A resource group is a resource of another service, and nothing registers one in
        // this emulator, so a group names no node reachable here and the target selects
        // none — what the store holds, rather than a refusal.
        TARGET_KEY_RESOURCE_GROUP_NAME | TARGET_KEY_RESOURCE_GROUP => Ok(Vec::new()),
        super::TAG_KEY_FILTER_KEY => Ok(matched(super::TAG_KEY_FILTER_KEY, false)),
        _ => match key.strip_prefix(super::TAG_FILTER_KEY_PREFIX) {
            Some(name) if !name.is_empty() => Ok(store
                .managed_nodes_in_region(region)
                .filter(|(_, node)| {
                    node.tags
                        .get(name)
                        .is_some_and(|held| values.iter().any(|wanted| wanted == held))
                })
                .map(|(id, _)| id)
                .collect()),
            _ => Err(key.to_string()),
        },
    }
}

/// Why the parameter values a send supplied are not the ones the document declares, or
/// `None` when they are.
///
/// "You must specify values for all required parameters in the Amazon Web Services
/// Systems Manager document (SSM document). You can only supply values to parameters
/// defined in the SSM document" (`InvalidParameters`). A declared parameter is REQUIRED
/// when the document declares no default for it: the default is the value the document
/// runs with when the send names none, so a parameter that has one needs no value from
/// the caller and a parameter that has none cannot be left out.
pub fn parameter_rejection(
    document_name: &str,
    declared: &[DocumentParameter],
    supplied: &BTreeMap<String, Vec<String>>,
) -> Option<String> {
    for name in supplied.keys() {
        if !declared
            .iter()
            .any(|parameter| parameter.name.as_deref() == Some(name.as_str()))
        {
            return Some(format!(
                "Document {document_name} doesn't support a parameter named {name}"
            ));
        }
    }
    let missing: Vec<&str> = declared
        .iter()
        .filter(|parameter| parameter.default_value.is_none())
        .filter_map(|parameter| parameter.name.as_deref())
        .filter(|name| !supplied.contains_key(*name))
        .collect();
    (!missing.is_empty()).then(|| format!("Missing required parameters: {}", missing.join(", ")))
}

/// Why the document cannot run on a node the send NAMED, or `None` when it can.
///
/// "The document doesn't support the platform type of the given managed node IDs. For
/// example, you sent an document for a Windows managed node to a Linux node"
/// (`UnsupportedPlatformType`), so the platforms a document's own content restricts it to
/// ([`super::ParsedDocumentContent::platform_types`]) have to include the platform the
/// node reported. A node that has reported no platform restricts nothing.
pub fn unsupported_platform(
    document_name: &str,
    platform_types: &[String],
    node: &ManagedNodeRecord,
) -> Option<String> {
    let platform = node.properties.platform_type.as_deref()?;
    if platform_types.is_empty() || platform_types.iter().any(|held| held == platform) {
        return None;
    }
    Some(format!(
        "Document {document_name} doesn't support platform type {platform}"
    ))
}

/// The `CommandFilterKey` that narrows the history by when a command was requested:
/// "InvokedAfter: Specify a timestamp to limit your results … occurring July 7, 2021, and
/// later" (`CommandFilter$value`). From the modelled enum
/// (`crate::ops::types::COMMAND_FILTER_KEY_VALUES`).
pub const COMMAND_FILTER_KEY_INVOKED_AFTER: &str = "InvokedAfter";

/// The same narrowing from the other side: "InvokedBefore: Specify a timestamp to limit
/// your results … from before July 7, 2021" (same doc).
pub const COMMAND_FILTER_KEY_INVOKED_BEFORE: &str = "InvokedBefore";

/// The `CommandFilterKey` that narrows the history by a command's status: "Specify a valid
/// command status to see a list of all command executions with that status" (same doc).
pub const COMMAND_FILTER_KEY_STATUS: &str = "Status";

/// The `CommandFilterKey` that narrows the history by whether a command is still running
/// (same doc), whose values are [`EXECUTION_STAGE_VALUES`].
pub const COMMAND_FILTER_KEY_EXECUTION_STAGE: &str = "ExecutionStage";

/// The `CommandFilterKey` that narrows the history by the document a command runs:
/// "Specify name of the … document … for which you want to see command execution results"
/// (same doc).
pub const COMMAND_FILTER_KEY_DOCUMENT_NAME: &str = "DocumentName";

/// Whether `key` is a filter key a command history read accepts: the modelled
/// `CommandFilterKey` enum is that whole set, so nothing else names a property of a
/// command to narrow by.
pub fn is_command_filter_key(key: &str) -> bool {
    crate::ops::types::COMMAND_FILTER_KEY_VALUES.contains(&key)
}

/// The values a `Status` filter of a COMMAND history read may name, in the order
/// `CommandFilter$value` lists them ("The status values you can specify for
/// `ListCommands` are …") — a wider set than the modelled `CommandStatus` enum, because
/// the detailed status a command reports in words is also named here, spelled without its
/// spaces ([`CommandRecord::matches_status_filter`]).
pub const LIST_COMMANDS_STATUS_FILTER_VALUES: &[&str] = &[
    "Pending",
    "InProgress",
    "Success",
    "Cancelled",
    "Failed",
    "TimedOut",
    "AccessDenied",
    "DeliveryTimedOut",
    "ExecutionTimedOut",
    "Incomplete",
    "NoInstancesInTag",
    "LimitExceeded",
];

/// The `ExecutionStage` value that selects the commands still running: "Executing:
/// Returns a list of command executions that are currently still running"
/// (`CommandFilter$value`).
pub const EXECUTION_STAGE_EXECUTING: &str = "Executing";

/// The `ExecutionStage` value that selects the commands that are done: "Complete: Returns
/// a list of command executions that have already completed" (same doc).
pub const EXECUTION_STAGE_COMPLETE: &str = "Complete";

/// The values an `ExecutionStage` filter may name (same doc). What a refusal of another
/// value lists.
pub const EXECUTION_STAGE_VALUES: &[&str] = &[EXECUTION_STAGE_EXECUTING, EXECUTION_STAGE_COMPLETE];

/// Whether `status` is one of the terminal states, which a command and one of its
/// invocations reach by the same names: `Command$StatusDetails` and
/// `CommandInvocation$StatusDetails` both end at "Success", "Cancelled", "Failed" and the
/// timed-out states, so this reads the one set ([`TERMINAL_INVOCATION_STATUSES`]).
pub fn is_terminal_status(status: &str) -> bool {
    TERMINAL_INVOCATION_STATUSES.contains(&status)
}

/// Whether `id` is shaped like a `CommandId`: the modelled length is exactly 36, and this
/// service mints the hyphenated UUID form ([`super::ComAmazonawsSsmAmazonssmState::mint_command_id`]),
/// read by the one reader of that form this crate has.
pub fn is_command_id(id: &str) -> bool {
    super::is_activation_id(id)
}

/// The bounds the modelled `CommandMaxResults` carries (range 1..50).
pub const MIN_COMMAND_PAGE_SIZE: i32 = 1;

/// See [`MIN_COMMAND_PAGE_SIZE`]. It is also the page size a request that names no
/// `MaxResults` gets, the way every other read of this service serves one.
pub const MAX_COMMAND_PAGE_SIZE: i32 = 50;

/// The page size a command history read serves, or the refusal message for a `MaxResults`
/// outside the modelled range — which the protocol's boundary validator already refuses,
/// so this is a backstop rather than the first line of defence.
pub fn command_page_size(max_results: Option<i32>) -> Result<usize, String> {
    super::page_size(max_results, MIN_COMMAND_PAGE_SIZE, MAX_COMMAND_PAGE_SIZE)
}

/// The instant a timestamp filter value names, or `None` when the value names none.
///
/// "Specify a timestamp to limit your results. For example, specify
/// `2024-07-07T00:00:00Z`" (`CommandFilter$value`) — the ISO 8601 spelling
/// `::aws_smithy_types::date_time::Format::DateTime` reads, which is the one the service
/// renders `RequestedDateTime` in.
pub fn parse_filter_instant(value: &str) -> Option<SystemTime> {
    let parsed = ::aws_smithy_types::DateTime::from_str(
        value,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    SystemTime::try_from(parsed).ok()
}

/// Why `value` is not a value the filter key `key` may carry, or `None` when it is one.
/// `position` is the filter's place in the request's `Filters` list, counted from one.
///
/// A filter value has to be a legal value of what it filters: a timestamp for the two
/// instant keys, one of the status values the operation documents for a `Status` filter,
/// and one of the two stages for an `ExecutionStage` filter. `DocumentName` filters a
/// document name, which constrains nothing here — a name no document carries selects no
/// command rather than refusing the read.
///
/// The sentence is worded the way a request-validation refusal of this API is: the value,
/// the member it was carried in — spelled as the emulator's own boundary validator spells a
/// list member's path — and the constraint it failed, as the page-size refusal of every read
/// of this service already states one (`super::page_size`). The enum clause lists the values
/// the operation documents; the timestamp clause is this emulator's own wording of what
/// `CommandFilter$value` documents the two instant keys as taking ("Specify a timestamp …
/// For example, specify 2024-07-07T00:00:00Z"), which no model constraint states.
pub fn command_filter_value_rejection(position: usize, key: &str, value: &str) -> Option<String> {
    let failed = |constraint: String| {
        Some(format!(
            "Value '{value}' at 'filters.{position}.member.value' failed to satisfy \
             constraint: {constraint}"
        ))
    };
    let enumerated =
        |values: &[&str]| format!("Member must satisfy enum value set: [{}]", values.join(", "));
    match key {
        COMMAND_FILTER_KEY_INVOKED_AFTER | COMMAND_FILTER_KEY_INVOKED_BEFORE
            if parse_filter_instant(value).is_none() =>
        {
            failed(
                "Member must be a timestamp, for example 2024-07-07T00:00:00Z".to_string(),
            )
        }
        COMMAND_FILTER_KEY_STATUS if !LIST_COMMANDS_STATUS_FILTER_VALUES.contains(&value) => {
            failed(enumerated(LIST_COMMANDS_STATUS_FILTER_VALUES))
        }
        COMMAND_FILTER_KEY_EXECUTION_STAGE if !EXECUTION_STAGE_VALUES.contains(&value) => {
            failed(enumerated(EXECUTION_STAGE_VALUES))
        }
        _ => None,
    }
}

/// How this service states that a node id cannot be named beside a `Pending` status
/// filter: the sentence `ListCommandsRequest$InstanceId` states the rule in, verbatim —
/// "You can't specify a managed node ID in the same command that you specify Status =
/// Pending. This is because the command hasn't reached the managed node yet." It carries
/// nothing of the request, because the rule is about the two criteria and not about which
/// node or which filter was named.
pub const NODE_ID_WITH_PENDING_STATUS: &str =
    "You can't specify a managed node ID in the same command that you specify Status = \
     Pending. This is because the command hasn't reached the managed node yet.";

/// How this service states that the managed nodes a request NAMED cannot take the call.
///
/// `InvalidInstanceId` is what every operation that RESOLVES a node id answers with when
/// the ids name no node it can act on: "SSM Agent isn't registered with the SSM endpoint
/// … The managed node isn't in a valid state". The sentence carries no interpolated value
/// — it states the refusal for the set of ids the request named, whatever that set was,
/// so neither the ids nor the account are repeated into it.
pub const NODES_NOT_IN_A_VALID_STATE: &str = "Instances not in a valid state for account";
