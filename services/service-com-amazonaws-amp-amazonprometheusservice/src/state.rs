// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-amp-amazonprometheusservice service.
//!
//! Besides the state struct itself this module holds the few derivations every
//! workspace handler needs (the ARN, the Prometheus endpoint, the request's region,
//! a fresh workspace id), so `src/ops/*.rs` share ONE copy of each rule.

use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId, ShapeType, TraitMap};
use aws_smithy_schema_server::constraints::{
    validate, LengthTrait, PatternTrait, Violation, ViolationKind,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// `WorkspaceStatusCode` values a workspace moves through here. Each is an entry of
/// the modelled `com.amazonaws.amp#WorkspaceStatusCode` enum, whose own
/// documentation states the lifecycle rules this crate enforces:
/// `CREATING` — "Workspace is being created. Deletion is disallowed until status is
/// ACTIVE."; `ACTIVE` — "Workspace has been created and is usable."; `UPDATING` —
/// "Workspace is being updated. Updates are allowed only when status is ACTIVE.";
/// `DELETING` — "Workspace is being deleted. Deletions are allowed only when status is
/// ACTIVE." (`crate::ops::types::WORKSPACE_STATUS_CODE_VALUES` carries the full list.)
///
/// The same three literals are entries of `com.amazonaws.amp#AnomalyDetectorStatusCode`
/// too (`crate::ops::types::ANOMALY_DETECTOR_STATUS_CODE_VALUES`), so an anomaly
/// detector's lifecycle reports them from these same constants rather than from a
/// second copy of the strings. `com.amazonaws.amp#ScraperStatusCode`
/// (`crate::ops::types::SCRAPER_STATUS_CODE_VALUES`) spells them the same way and
/// states the same rules for a scraper: `ACTIVE` — "Scraper has been created and is
/// usable."; `CREATING` and `UPDATING` — "Deletion is disallowed until status is
/// ACTIVE."; `DELETING` — "Scraper is being deleted. Deletions are allowed only when
/// status is ACTIVE." — so a scraper's lifecycle reads them from here too.
pub const STATUS_CREATING: &str = "CREATING";
/// See [`STATUS_CREATING`].
pub const STATUS_ACTIVE: &str = "ACTIVE";
/// See [`STATUS_CREATING`].
pub const STATUS_DELETING: &str = "DELETING";
/// See [`STATUS_CREATING`]. The transient status a resource whose UPDATE has been
/// accepted reports until the clock settles it back to [`STATUS_ACTIVE`]: an entry of
/// `com.amazonaws.amp#ScraperStatusCode` documented "Scraper is being updated. Deletion
/// is disallowed until status is ACTIVE.", and of `com.amazonaws.amp#WorkspaceStatusCode`
/// documented "Workspace is being updated. Updates are allowed only when status is
/// ACTIVE."
pub const STATUS_UPDATING: &str = "UPDATING";

/// `WorkspaceConfigurationStatusCode` values a workspace CONFIGURATION moves through
/// here. Each is an entry of the modelled
/// `com.amazonaws.amp#WorkspaceConfigurationStatusCode` enum, whose own documentation
/// states the rule this crate enforces: `UPDATING` — "Workspace configuration is being
/// updated. Update is disallowed until workspace configuration is ACTIVE and workspace
/// status is ACTIVE."; `ACTIVE` — "Workspace configuration has been updated. Update is
/// disallowed until workspace configuration is ACTIVE and workspace status is ACTIVE."
/// The enum's third entry, `UPDATE_FAILED`, is never reported: nothing in this emulator
/// fails a configuration update, so claiming that status would be a status for work that
/// never ran. (`crate::ops::types::WORKSPACE_CONFIGURATION_STATUS_CODE_VALUES` carries
/// the full list.)
pub const CONFIG_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`CONFIG_STATUS_ACTIVE`].
pub const CONFIG_STATUS_UPDATING: &str = "UPDATING";

/// The retention period a workspace's configuration carries before any
/// `UpdateWorkspaceConfiguration` sets one.
///
/// DOCUMENTED: Amazon Managed Service for Prometheus publishes "Data retention period"
/// as a workspace quota with a default value of 150 days, so a workspace that nobody has
/// configured still retains metrics for that long and `DescribeWorkspaceConfiguration`
/// reports it. `WorkspaceConfigurationDescription$retentionPeriodInDays` is modelled
/// `@range(min: 1)`, which this value satisfies.
pub const DEFAULT_RETENTION_PERIOD_IN_DAYS: i32 = 150;

/// The out-of-order sample window a workspace's configuration carries before any
/// `UpdateWorkspaceConfiguration` sets one.
///
/// Amazon Managed Service for Prometheus rejects out-of-order samples unless a window
/// is configured for them, so an unconfigured workspace's window is zero seconds —
/// which is also the published lower bound of the modelled `@range(min: 0)` on
/// `WorkspaceConfigurationDescription$outOfOrderTimeWindowInSeconds`. The member is
/// reported rather than omitted because the workspace HAS a window from the moment it
/// exists; a caller comparing it before and after an update reads a number either way.
pub const DEFAULT_OUT_OF_ORDER_TIME_WINDOW_IN_SECONDS: i32 = 0;

/// The rule-evaluation query offset a workspace's configuration carries before any
/// `UpdateWorkspaceConfiguration` sets one.
///
/// `UpdateWorkspaceConfigurationRequest$ruleQueryOffsetInSeconds` documents what the
/// member buys — "Specifies the duration in seconds to offset rule evaluation queries
/// into the past. This allows ingested samples to be available before rule evaluation."
/// — so a workspace nobody has configured offsets them by nothing, the published lower
/// bound of the modelled `@range(min: 0)`. Reported for the same reason as
/// [`DEFAULT_OUT_OF_ORDER_TIME_WINDOW_IN_SECONDS`].
pub const DEFAULT_RULE_QUERY_OFFSET_IN_SECONDS: i32 = 0;

/// How many label sets one workspace's configuration holds before
/// `UpdateWorkspaceConfiguration` refuses with the modelled
/// `ServiceQuotaExceededException`.
///
/// This emulator models neither Service Quotas nor an account's adjustments, so — like
/// [`WORKSPACE_QUOTA`] — this is the emulator's OWN bound on the store rather than a
/// claim about any account's limit. It is what stops one request's `limitsPerLabelSet`
/// list growing a workspace's stored configuration without end; an update REPLACES the
/// stored list, so nothing accumulates beyond this bound.
pub const LABEL_SET_LIMITS_PER_WORKSPACE: usize = 1_000;

/// The Prometheus metric-name label. `LimitsPerLabelSet$labelSet` documents the rule
/// that goes with it: "If the label name is metric name label `__name__`, then the
/// metric part of the name must conform to the following pattern:
/// `[a-zA-Z_:][a-zA-Z0-9_:]*`" — so for this label name the VALUE is a metric name and
/// is held to that pattern. Validated by `UpdateWorkspaceConfiguration`, because
/// `LabelValue` is modelled with a length bound only.
pub const METRIC_NAME_LABEL: &str = "__name__";

/// `LoggingConfigurationStatusCode` values a rules-and-alerting logging configuration
/// moves through here. Each is an entry of the modelled
/// `com.amazonaws.amp#LoggingConfigurationStatusCode` enum, whose own documentation
/// states the lifecycle rule this crate enforces: `CREATING` — "Logging configuration
/// is being created. Update/Deletion is disallowed until logging configuration is
/// ACTIVE and workspace status is ACTIVE."; `ACTIVE` — "Logging configuration has been
/// created/updated."; `UPDATING` — "Logging configuration is being updated.
/// Update/Deletion is disallowed until logging configuration is ACTIVE and workspace
/// status is ACTIVE."; `DELETING` — "Logging configuration is being deleting.
/// Update/Deletion is disallowed until logging configuration is ACTIVE and workspace
/// status is ACTIVE." (`crate::ops::types::LOGGING_CONFIGURATION_STATUS_CODE_VALUES`
/// carries the full list, including the two failure codes nothing here reaches: this
/// emulator runs no vended-log delivery that could fail.)
pub const LOGGING_STATUS_CREATING: &str = "CREATING";
/// See [`LOGGING_STATUS_CREATING`].
pub const LOGGING_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`LOGGING_STATUS_CREATING`].
pub const LOGGING_STATUS_UPDATING: &str = "UPDATING";
/// See [`LOGGING_STATUS_CREATING`]. The transient status a configuration whose DELETION
/// has been accepted reports until the clock removes the record.
pub const LOGGING_STATUS_DELETING: &str = "DELETING";

/// The `resourceType` the modelled `ResourceNotFoundException` reports for a workspace's
/// rules-and-alerting logging configuration. The member is a free-form `String`
/// documented as "Type of the resource affected", so the emulator reports this
/// resource's name in the service's `AWS::APS::` namespace, the way
/// [`WORKSPACE_RESOURCE_TYPE`] does for the workspace itself.
pub const LOGGING_CONFIGURATION_RESOURCE_TYPE: &str = "AWS::APS::LoggingConfiguration";

/// `WorkspacePolicyStatusCode` values a workspace's resource-based policy moves
/// through here. Each is an entry of the modelled
/// `com.amazonaws.amp#WorkspacePolicyStatusCode` enum, whose own documentation states
/// the lifecycle rule this crate enforces — `CREATING`: "Resource-based Policy is
/// being created. Update/Deletion is disallowed until Resource-based Policy is ACTIVE
/// and Workspace is ACTIVE.", and `ACTIVE` / `UPDATING` / `DELETING` repeat that same
/// sentence. (`crate::ops::types::WORKSPACE_POLICY_STATUS_CODE_VALUES` carries the
/// full list.)
pub const POLICY_STATUS_CREATING: &str = "CREATING";
/// See [`POLICY_STATUS_CREATING`].
pub const POLICY_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`POLICY_STATUS_CREATING`].
pub const POLICY_STATUS_UPDATING: &str = "UPDATING";
/// See [`POLICY_STATUS_CREATING`].
pub const POLICY_STATUS_DELETING: &str = "DELETING";

/// The revision id a caller may name on the first put for a workspace that holds no
/// resource-based policy: `PutResourcePolicyRequest$revisionId` documents "For the
/// first PUT request on a workspace that doesn't have an existing resource policy, you
/// can specify `NO_POLICY` as the revision ID."
pub const NO_POLICY_REVISION_ID: &str = "NO_POLICY";

/// The `resourceType` the modelled `ResourceNotFoundException` / `ConflictException`
/// report for a workspace's resource-based policy. The member is a free-form `String`
/// documented as "Type of the resource affected", so the emulator reports the
/// resource's AWS type name.
pub const POLICY_RESOURCE_TYPE: &str = "AWS::APS::ResourcePolicy";

/// How long a workspace stays in a transient status before the clock owes the next
/// transition (`CREATING` -> `ACTIVE`, `DELETING` -> removed, a workspace
/// configuration's `UPDATING` -> `ACTIVE`), and the same for a resource-based policy
/// and for a scraper (`CREATING`/`UPDATING` -> `ACTIVE`, `DELETING` -> removed).
///
/// CHOSEN, not documented: Amazon Managed Service for Prometheus publishes no
/// duration for any of these transitions, so this is the emulator's own span — long
/// enough that a caller acting straight after
/// `CreateWorkspace`/`DeleteWorkspace`/`UpdateWorkspaceConfiguration` still observes
/// the transient status and the state errors that go with it, short enough
/// that a workflow does not wait on it. Nothing sleeps for it: the due instant is
/// stored and applied by [`ComAmazonawsAmpAmazonprometheusserviceState::materialize_due`].
pub const TRANSITION_DELAY: Duration = Duration::from_millis(500);

/// `RuleGroupsNamespaceStatusCode` values a rule groups namespace moves through here.
/// Each is an entry of the modelled `com.amazonaws.amp#RuleGroupsNamespaceStatusCode`
/// enum, whose own documentation states the lifecycle rules this crate enforces:
/// `CREATING` — "Namespace is being created. Update/Deletion is disallowed until
/// namespace is ACTIVE and workspace status is ACTIVE."; `ACTIVE` — "Namespace has been
/// created/updated. …"; `UPDATING` — "Namespace is being updated. …"; `DELETING` —
/// "Namespace is being deleting. …". (`crate::ops::types::RULE_GROUPS_NAMESPACE_STATUS_CODE_VALUES`
/// carries the full list.) They are spelled separately from the `WorkspaceStatusCode`
/// constants above because they belong to a different modelled enum.
pub const NAMESPACE_STATUS_CREATING: &str = "CREATING";
/// See [`NAMESPACE_STATUS_CREATING`].
pub const NAMESPACE_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`NAMESPACE_STATUS_CREATING`].
pub const NAMESPACE_STATUS_UPDATING: &str = "UPDATING";
/// See [`NAMESPACE_STATUS_CREATING`].
pub const NAMESPACE_STATUS_DELETING: &str = "DELETING";

/// The `resourceType` the modelled `ResourceNotFoundException` / `ConflictException`
/// report for a workspace. The member is a free-form `String` documented as "Type of
/// the resource affected", so the emulator reports the resource's AWS type name.
pub const WORKSPACE_RESOURCE_TYPE: &str = "AWS::APS::Workspace";

/// The alert manager definition a workspace may hold: its record, its statuses and
/// the rule that decides whether a `data` blob is one. Re-exported, so a handler
/// names `crate::state::…` for it exactly as it does for a workspace.
pub mod alert_manager;
pub use alert_manager::*;

/// The time series a workspace holds: what its Prometheus data plane writes and what an
/// instant query reads back. Re-exported for the same reason the alert manager definition
/// is.
pub mod metrics;
pub use metrics::*;

/// Reading a Prometheus remote-write request — the wire format the data plane receives.
/// Named rather than re-exported: a caller says `crate::state::remote_write::…`, so the
/// wire format's own message names stay beside it.
pub mod remote_write;

/// The PromQL an instant query carries, parsed and evaluated against the samples a
/// workspace holds. Named rather than re-exported for the same reason
/// [`remote_write`] is.
pub mod promql;

/// The `resourceType` those same exceptions report for a rule groups namespace. The
/// name is the one the model itself gives the resource: `RuleGroupsNamespaceArn`
/// carries `@arnReference(type: "AWS::APS::RuleGroupsNamespace")`.
pub const RULE_GROUPS_NAMESPACE_RESOURCE_TYPE: &str = "AWS::APS::RuleGroupsNamespace";

/// `QueryLoggingConfigurationStatusCode` values a query logging configuration moves
/// through here. Each is an entry of the modelled
/// `com.amazonaws.amp#QueryLoggingConfigurationStatusCode` enum, whose own
/// documentation names them: `CREATING` — "Query logging configuration is being
/// created."; `ACTIVE` — "Query logging configuration is active."; `UPDATING` — "Query
/// logging configuration is being updated."; `DELETING` — "Query logging configuration
/// is being deleted." (`crate::ops::types::QUERY_LOGGING_CONFIGURATION_STATUS_CODE_VALUES`
/// carries the full list, including the two failure codes nothing here reaches: this
/// emulator runs no vended-log delivery that could fail.)
pub const QUERY_LOGGING_STATUS_CREATING: &str = "CREATING";
/// See [`QUERY_LOGGING_STATUS_CREATING`].
pub const QUERY_LOGGING_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`QUERY_LOGGING_STATUS_CREATING`].
pub const QUERY_LOGGING_STATUS_UPDATING: &str = "UPDATING";
/// See [`QUERY_LOGGING_STATUS_CREATING`].
pub const QUERY_LOGGING_STATUS_DELETING: &str = "DELETING";

/// The `resourceType` the modelled `ResourceNotFoundException` / `ConflictException`
/// report for a query logging configuration — the resource's AWS type name, as
/// [`WORKSPACE_RESOURCE_TYPE`] is for a workspace.
pub const QUERY_LOGGING_RESOURCE_TYPE: &str = "AWS::APS::QueryLoggingConfiguration";

/// `ScraperLoggingConfigurationStatusCode` values a SCRAPER's logging configuration
/// moves through here. Each is an entry of the modelled
/// `com.amazonaws.amp#ScraperLoggingConfigurationStatusCode` enum
/// (`crate::ops::types::SCRAPER_LOGGING_CONFIGURATION_STATUS_CODE_VALUES` carries the
/// full list, including the two failure codes nothing here reaches: this emulator runs
/// no vended-log delivery that could fail). The lifecycle is the one a query logging
/// configuration follows — a configuration whose deletion has been accepted reports
/// `DELETING` until the clock drops the record
/// ([`ComAmazonawsAmpAmazonprometheusserviceState::materialize_due`]), and any other
/// pending transition settles to `ACTIVE`.
pub const SCRAPER_LOGGING_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`SCRAPER_LOGGING_STATUS_ACTIVE`].
pub const SCRAPER_LOGGING_STATUS_DELETING: &str = "DELETING";
/// See [`SCRAPER_LOGGING_STATUS_ACTIVE`]. The transient status a configuration whose
/// UPDATE has been accepted reports until the clock settles it back to
/// [`SCRAPER_LOGGING_STATUS_ACTIVE`]: an entry of
/// `com.amazonaws.amp#ScraperLoggingConfigurationStatusCode` documented "Scraper logging
/// configuration is being updated."
pub const SCRAPER_LOGGING_STATUS_UPDATING: &str = "UPDATING";

/// The `resourceType` the modelled `ResourceNotFoundException` / `ConflictException`
/// report for a scraper's logging configuration — the resource's AWS type name, as
/// [`QUERY_LOGGING_RESOURCE_TYPE`] is for a workspace's.
pub const SCRAPER_LOGGING_RESOURCE_TYPE: &str = "AWS::APS::ScraperLoggingConfiguration";

/// The `message` the modelled `ResourceNotFoundException` reports for a scraper that
/// holds no logging configuration — one CONSTANT sentence naming the kind of resource
/// that is missing, for the same reason [`QUERY_LOGGING_NOT_FOUND_MESSAGE`] is: the
/// scraper the request named is reported in `resourceId` instead.
pub const SCRAPER_LOGGING_NOT_FOUND_MESSAGE: &str = "Scraper logging configuration not found.";

/// The `message` the modelled `ResourceNotFoundException` reports for a workspace this
/// account does not hold.
///
/// One CONSTANT sentence, exactly as [`QUERY_LOGGING_NOT_FOUND_MESSAGE`] is for a
/// configuration: `ResourceNotFoundException` models `resourceId` (@required) beside
/// `message`, so the identifier of the missing resource has its own home and the
/// sentence carries nothing from the request.
///
/// It is the sentence of every workspace-addressed operation RECORDED answering it —
/// `DeleteWorkspace` among them — but not of ALL of them: the recorded text is each
/// OPERATION's own, and `DescribeWorkspace` answers
/// [`DESCRIBE_WORKSPACE_NOT_FOUND_MESSAGE`] instead. An operation with no recording of
/// its own answers this one, so a caller still reads one value across the operations that
/// share it.
pub const WORKSPACE_NOT_FOUND_MESSAGE: &str = "Workspace not found.";

/// The `message` `DescribeWorkspace` reports for a workspace this account does not hold:
/// [`WORKSPACE_NOT_FOUND_MESSAGE`] without the trailing period.
///
/// RECORDED BEHAVIOUR: in the recorded delete lifecycle, the `DescribeWorkspace` that
/// finds a deleted workspace gone is refused "Workspace not found" while the
/// `DeleteWorkspace` on that same gone id is refused "Workspace not found." — so this
/// service does not have ONE workspace not-found sentence, it has one per operation, and
/// each answers the text recorded for it. Constant and id-free like every other refusal
/// sentence here: the id asked for is reported in `resourceId`.
pub const DESCRIBE_WORKSPACE_NOT_FOUND_MESSAGE: &str = "Workspace not found";

/// The `message` the ARN-addressed `/tags/{resourceArn}` operations report for a
/// `resourceArn` that resolves to nothing — RECORDED BEHAVIOUR, like
/// [`RESOURCE_NOT_FOUND_CODE`] beside it. Constant and ARN-free for the same reason
/// [`WORKSPACE_NOT_FOUND_MESSAGE`] is: nothing of the request is echoed back into it.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The wire error code the ARN-addressed `/tags/{resourceArn}` operations answer for a
/// `resourceArn` that resolves to no live resource.
///
/// RECORDED BEHAVIOUR: those three operations model `ResourceNotFoundException`
/// (@httpError 404) — `smithy_query(query="operation",operation="TagResource,UntagResource,ListTagsForResource")`
/// — but the recorded real-aps response for an ARN that names nothing carries the code
/// `NotFoundException` at that same 404, whatever ARN type it names. A client branches on
/// the CODE, so that is what this service answers; the code is not a shape the model
/// declares, which is why it is raised through [`arn_not_found`] and the synthetic
/// [`crate::scaffolded::NotFoundException`] rather than through an operation's modelled
/// error enum.
///
/// The operations addressed by RESOURCE ID rather than by ARN keep the modelled
/// `ResourceNotFoundException`, which is what they are recorded answering (a
/// `DescribeAnomalyDetector` for a detector that is gone, for one).
pub const RESOURCE_NOT_FOUND_CODE: &str = "NotFoundException";

/// The `message` the modelled `ResourceNotFoundException` reports for a rule groups
/// namespace a workspace does not hold. Constant and id-free for the same reason
/// [`WORKSPACE_NOT_FOUND_MESSAGE`] is; the namespace name is reported in `resourceId`.
pub const RULE_GROUPS_NAMESPACE_NOT_FOUND_MESSAGE: &str = "Rule groups namespace not found.";

/// The `message` the modelled `ResourceNotFoundException` reports for a workspace that
/// holds no rules-and-alerting logging configuration — constant and id-free for the same
/// reason [`QUERY_LOGGING_NOT_FOUND_MESSAGE`] is.
///
/// RECORDED BEHAVIOUR: the recorded `DescribeLoggingConfiguration` and
/// `UpdateLoggingConfiguration` on a workspace whose configuration has been deleted are
/// both refused "Workspace logging configuration not found." — the sentence names the
/// resource FAMILY as the service names it, a logging configuration OF a workspace, which
/// is the same sentence the query logging configuration's own absence is recorded
/// answering.
pub const LOGGING_NOT_FOUND_MESSAGE: &str = "Workspace logging configuration not found.";

/// The `message` the modelled `ValidationException` reports when the workspace's
/// rules-and-alerting logging configuration is asked to be CREATED while the one the
/// workspace holds is being deleted.
///
/// A record whose removal the clock already owes is still held, so there is nothing to
/// create yet, and it is not a record a create can be answered from either: it is about
/// to be dropped. Constant and id-free, like every other refusal sentence here — the
/// workspace a caller named is already in its own request — and named for the resource it
/// speaks about, as the neighbouring refusals of each configuration are.
pub const LOGGING_CONFIGURATION_DELETING_MESSAGE: &str =
    "The logging configuration is being deleted; create it again once the deletion has \
     completed.";

/// [`LOGGING_CONFIGURATION_DELETING_MESSAGE`] for the workspace's QUERY logging
/// configuration, which is a different resource with its own lifecycle.
pub const QUERY_LOGGING_CONFIGURATION_DELETING_MESSAGE: &str =
    "The query logging configuration is being deleted; create it again once the deletion \
     has completed.";

/// The `message` the modelled `ValidationException` reports when one of a workspace's
/// logging configurations is asked to be CREATED while the WORKSPACE itself is being
/// deleted.
///
/// `WorkspaceStatusCode$DELETING` is "Workspace is being deleted", and
/// [`RegionalState::materialize_due`] drops every configuration a removed workspace
/// holds along with it — so a create in that window could only be accepted and then
/// leave the workspace holding nothing at all. One sentence for both single-valued
/// logging children, neither of which models a `ConflictException` to refuse with;
/// constant and id-free, like every other refusal sentence here, because the workspace
/// a caller named is already in its own request.
pub const WORKSPACE_DELETING_MESSAGE: &str =
    "The workspace is being deleted; it cannot take new logging configurations.";

/// The `message` the modelled `ResourceNotFoundException` reports for a workspace that
/// holds no resource-based policy — constant and id-free for the same reason
/// [`WORKSPACE_NOT_FOUND_MESSAGE`] is; the workspace is reported in `resourceId`.
///
/// RECORDED BEHAVIOUR: the recorded `DescribeResourcePolicy` and repeat
/// `DeleteResourcePolicy` on a workspace whose policy has been deleted are both refused
/// "Workspace resource policy not found." — the noun phrase is the service's own for this
/// family, a resource policy OF a workspace.
pub const POLICY_NOT_FOUND_MESSAGE: &str = "Workspace resource policy not found.";

/// The `message` the modelled `ResourceNotFoundException` reports for a workspace that
/// holds no query logging configuration.
///
/// One CONSTANT sentence, as Amazon Managed Service for Prometheus answers it: it names
/// the kind of resource that is missing and carries nothing from the request, which
/// `resourceId` reports instead. The three operations that look a configuration up share
/// it, so they cannot drift apart.
pub const QUERY_LOGGING_NOT_FOUND_MESSAGE: &str = "Workspace logging configuration not found.";

/// How many workspaces ONE REGION's store holds before `CreateWorkspace` refuses with
/// the modelled `ServiceQuotaExceededException`.
///
/// Amazon Managed Service for Prometheus publishes an adjustable quota on workspaces
/// per account per Region, and this emulator models neither Service Quotas nor an
/// account's adjustments — so this is the emulator's OWN bound on the store, not a
/// claim about any account's limit. It is counted per Region because the published quota
/// is, and because the workspaces of one Region are all a caller signed for it can
/// reach. It sits far above any workflow an emulator serves, and it is what stops the
/// state growing without end: every record it bounds is released by `DeleteWorkspace`,
/// which also forgets that workspace's client tokens, so that map cannot outgrow it
/// either.
pub const WORKSPACE_QUOTA: usize = 1_000;

/// How this service partitions its state by Region, what each Region's store holds, and
/// what bounds the partitioning. Re-exported, so a handler names `crate::state::…` for a
/// Region's store exactly as it does for a workspace.
pub mod regions;
pub use regions::*;

/// How many rule groups namespaces ONE workspace holds before
/// `CreateRuleGroupsNamespace` refuses with the modelled
/// `ServiceQuotaExceededException`.
///
/// Amazon Managed Service for Prometheus publishes an adjustable quota on rule groups
/// namespaces per workspace, and this emulator models neither Service Quotas nor an
/// account's adjustments — so this is the emulator's OWN bound on the store, not a
/// claim about any account's limit. It is what stops one workspace's namespace map
/// growing without end; the map itself is owned by the workspace record, so deleting
/// the workspace releases every namespace in it.
pub const RULE_GROUPS_NAMESPACES_PER_WORKSPACE: usize = 1_000;

/// The largest page `ListWorkspaces` serves, and the page size it uses when the request
/// names no `maxResults`: `ListWorkspacesRequest$maxResults` is modelled
/// `@range(min: 1, max: 1000)` and documented "The maximum number of workspaces to
/// return per request. The default is 100." The handler clamps into `1..=MAX` itself
/// rather than trusting the boundary validator, so a caller cannot choose the amount of
/// work one request does.
pub const WORKSPACE_PAGE_LIMIT: i32 = 1_000;
/// See [`WORKSPACE_PAGE_LIMIT`].
pub const WORKSPACE_PAGE_DEFAULT: i32 = 100;

/// The largest page `ListScrapers` serves, and the page size it uses when the request
/// names no `maxResults`: `ListScrapersRequest$maxResults` is modelled
/// `@range(min: 1, max: 1000)` and documented "The maximum number of scrapers to return
/// in one ListScrapers operation. The range is 1-1000. If you omit this parameter, the
/// default of 100 is used." Clamped by the handler rather than trusted from the request,
/// exactly as [`WORKSPACE_PAGE_LIMIT`] is.
pub const SCRAPER_PAGE_LIMIT: i32 = 1_000;
/// See [`SCRAPER_PAGE_LIMIT`].
pub const SCRAPER_PAGE_DEFAULT: i32 = 100;

/// The largest page `ListRuleGroupsNamespaces` serves, and the page size it uses when
/// the request names no `maxResults`: `ListRuleGroupsNamespacesRequest$maxResults` is
/// modelled `@range(min: 1, max: 1000)` and documented "The maximum number of results to
/// return. The default is 100." Clamped by the handler rather than trusted from the
/// request, exactly as [`WORKSPACE_PAGE_LIMIT`] is.
pub const RULE_GROUPS_NAMESPACE_PAGE_LIMIT: i32 = 1_000;
/// See [`RULE_GROUPS_NAMESPACE_PAGE_LIMIT`].
pub const RULE_GROUPS_NAMESPACE_PAGE_DEFAULT: i32 = 100;

/// The largest page `ListAnomalyDetectors` serves, and the page size it uses when the
/// request names no `maxResults`.
///
/// The limit is the published one: `ListAnomalyDetectorsRequest$maxResults` is modelled
/// `@range(min: 1, max: 1000)` and documented "The maximum number of results to return
/// in a single call. Valid range is 1 to 1000." The member publishes NO default, so the
/// page size for a request that omits it is this emulator's own choice, taken to be the
/// one every other list operation of this service documents ([`WORKSPACE_PAGE_DEFAULT`],
/// [`SCRAPER_PAGE_DEFAULT`], [`RULE_GROUPS_NAMESPACE_PAGE_DEFAULT`]) so one listing does
/// not bound a page differently from the next. Clamped by the handler rather than trusted
/// from the request, exactly as [`WORKSPACE_PAGE_LIMIT`] is.
pub const ANOMALY_DETECTOR_PAGE_LIMIT: i32 = 1_000;
/// See [`ANOMALY_DETECTOR_PAGE_LIMIT`].
pub const ANOMALY_DETECTOR_PAGE_DEFAULT: i32 = 100;

/// Cut ONE page out of the records a listing has already selected, and report the
/// `nextToken` that page owes.
///
/// Every list operation of this service pages its results the same way, so the rule
/// lives here ONCE instead of in each handler:
///
/// * `next_token` is the cursor a previous page of the SAME listing emitted — the key of
///   its last entry — so this page starts strictly AFTER that key. It is a filter over
///   the records already selected by the caller, never a key into the store, so a token
///   from another listing can only drop entries from this page.
/// * `max_results` is clamped into `1..=limit` here rather than trusted from the
///   request, so the amount of work one call does is bounded by this service and not
///   chosen by its caller; `default` is the page size a request that names no
///   `max_results` gets. Each listing passes its own published pair (for example
///   [`WORKSPACE_PAGE_DEFAULT`] / [`WORKSPACE_PAGE_LIMIT`]), because the pair is a fact
///   about that operation's modelled member.
/// * The token is emitted ONLY when a further page exists — "a token indicating that
///   there are more results to retrieve" — never on the last page.
///
/// `records` is the selected list in the order the listing serves it, and is truncated
/// in place to the page; `key` reads the member that listing orders and pages by.
pub fn take_page<T>(
    records: &mut Vec<T>,
    key: impl Fn(&T) -> &str,
    next_token: Option<&str>,
    max_results: Option<i32>,
    default: i32,
    limit: i32,
) -> Option<String> {
    if let Some(cursor) = next_token {
        records.retain(|record| key(record) > cursor);
    }

    let page_size = max_results.map_or(default, |max| max.clamp(1, limit)) as usize;
    let has_more = records.len() > page_size;
    records.truncate(page_size);

    has_more
        .then(|| records.last().map(|record| key(record).to_string()))
        .flatten()
}

/// The filter keys `ListScrapers` honours, as `ListScrapersRequest$filters` documents
/// them: "Keys include `status`, `sourceArn`, `destinationArn`, and `alias`."
///
/// A key that is not one of these four is not a filter this service defines, and it is
/// IGNORED rather than refused: the member documents the keys it includes without saying
/// that any other is rejected, so refusing one would invent a refusal. The `@httpQuery`
/// members (`nextToken`, `maxResults`) never reach the map at all — the
/// `@httpQueryParams` binding in `crate::ops::list_scrapers` leaves them out, as the
/// trait requires.
pub const SCRAPER_FILTER_STATUS: &str = "status";
/// See [`SCRAPER_FILTER_STATUS`].
pub const SCRAPER_FILTER_SOURCE_ARN: &str = "sourceArn";
/// See [`SCRAPER_FILTER_STATUS`].
pub const SCRAPER_FILTER_DESTINATION_ARN: &str = "destinationArn";
/// See [`SCRAPER_FILTER_STATUS`].
pub const SCRAPER_FILTER_ALIAS: &str = "alias";

/// The `resourceType` the modelled `ResourceNotFoundException` / `ConflictException`
/// report for a scraper. The name is the one the model itself gives the resource:
/// `ScraperArn` carries `@arnReference(type: "AWS::APS::Scraper")`.
pub const SCRAPER_RESOURCE_TYPE: &str = "AWS::APS::Scraper";

/// The `message` the modelled `ResourceNotFoundException` carries for a scraper this
/// account does not hold. Constant and id-free for the same reason
/// [`WORKSPACE_NOT_FOUND_MESSAGE`] is: `ResourceNotFoundException` models `resourceId`
/// beside `message`, so the id of the missing scraper has its own home there.
pub const SCRAPER_NOT_FOUND_MESSAGE: &str = "Scraper not found.";

/// The `resourceType` the modelled `ResourceNotFoundException` / `ConflictException` /
/// `ServiceQuotaExceededException` report for an anomaly detector — the resource's AWS
/// type name, as [`WORKSPACE_RESOURCE_TYPE`] is for a workspace.
pub const ANOMALY_DETECTOR_RESOURCE_TYPE: &str = "AWS::APS::AnomalyDetector";

/// The `message` the modelled `ResourceNotFoundException` carries for an anomaly
/// detector that does not resolve.
///
/// The id itself goes in that exception's `resourceId` member — which is what the
/// member is for — so the message is the fixed sentence the service answers with,
/// rather than a second copy of the id.
pub const ANOMALY_DETECTOR_NOT_FOUND_MESSAGE: &str = "Anomaly detector not found.";

/// How many anomaly detectors one workspace holds before an operation that would bring
/// another into existence — `CreateAnomalyDetector`, or `PutAnomalyDetector` addressing an
/// id the workspace holds nothing for — refuses with the modelled
/// `ServiceQuotaExceededException`.
///
/// Amazon Managed Service for Prometheus publishes no fixed number here, and this
/// emulator models neither Service Quotas nor an account's adjustments, so this is the
/// emulator's OWN bound on the store — the role [`WORKSPACE_QUOTA`] plays for
/// workspaces. It is what stops a workspace's detector map (and the client-token map
/// the same deletion releases) growing without end.
pub const ANOMALY_DETECTORS_PER_WORKSPACE_QUOTA: usize = 1_000;

/// The `quotaCode` the modelled `ServiceQuotaExceededException` reports when
/// [`ANOMALY_DETECTORS_PER_WORKSPACE_QUOTA`] is reached.
///
/// Named for what it bounds: a real quota code is an identifier Service Quotas issues,
/// which this emulator does not model and so must not invent one of. Shared by both
/// operations that can reach the bound, so they report the same code.
pub const ANOMALY_DETECTOR_QUOTA_CODE: &str = "AnomalyDetectorsPerWorkspace";

/// The `message` the modelled `ServiceQuotaExceededException` carries when a workspace
/// already holds [`ANOMALY_DETECTORS_PER_WORKSPACE_QUOTA`] anomaly detectors — one
/// sentence for both operations that can reach that bound.
pub fn anomaly_detector_quota_message() -> String {
    format!(
        "The workspace already holds the maximum of \
         {ANOMALY_DETECTORS_PER_WORKSPACE_QUOTA} anomaly detectors"
    )
}

/// The `message` the modelled `ConflictException` carries when a workspace whose status
/// does not allow new children is asked to take an anomaly detector — one sentence for
/// both operations that can bring one into existence.
pub fn workspace_refuses_new_detector_message(workspace_status: &str) -> String {
    format!("The workspace is {workspace_status}; it cannot take new anomaly detectors")
}

/// The `message` the modelled `ConflictException` carries when the status a resource is
/// CURRENTLY in forbids the call that names it.
///
/// RECORDED BEHAVIOUR: a `DeleteWorkspace` on a workspace that is already `DELETING` is
/// refused "Can't delete workspace in non-ACTIVE state. Current status is DELETING" —
/// fixed text up to ONE variable part, the resource's current status code read from this
/// store (and no trailing period). Every status enum this service models states the rule
/// the same way ("Deletions are allowed only when status is ACTIVE", "Updates are allowed
/// only when status is ACTIVE"), so every refusal that turns on the status of the resource
/// the call ACTS ON is worded with this one template and a caller reads one sentence for
/// the whole class: `action` is what the call does to it ("delete", "update") and
/// `resource` is that resource as the service names it in prose.
///
/// A refusal that turns on the status of the PARENT a resource lives in, or on two
/// resources' statuses at once, says something else and is worded by its own handler: the
/// template carries one status and names one resource.
pub fn status_forbids_call_message(action: &str, resource: &str, current_status: &str) -> String {
    format!(
        "Can't {action} {resource} in non-{STATUS_ACTIVE} state. Current status is \
         {current_status}"
    )
}

/// How many scrapers ONE REGION's store holds before `CreateScraper` refuses with the
/// modelled `ServiceQuotaExceededException`.
///
/// Amazon Managed Service for Prometheus publishes an adjustable quota on scrapers per
/// account per Region, and this emulator models neither Service Quotas nor an account's
/// adjustments — so this is the emulator's OWN bound on the store, the role
/// [`WORKSPACE_QUOTA`] plays for workspaces, and counted per Region as that quota and the
/// published one both are. It is what stops the scraper map (and the create-token map the
/// same deletion releases) growing without end.
pub const SCRAPERS_PER_ACCOUNT_QUOTA: usize = 1_000;

/// The PromQL aggregation operators a Random Cut Forest query may be wrapped by.
///
/// Source: `RandomCutForestConfiguration$query`, which states "Random Cut Forest
/// queries must be wrapped by a supported PromQL aggregation operator" and then lists
/// them — "Supported PromQL aggregation operators: `avg`, `count`, `group`, `max`,
/// `min`, `quantile`, `stddev`, `stdvar`, and `sum`". Sorted, so a refusal names them
/// in one stable order.
pub const RANDOM_CUT_FOREST_AGGREGATION_OPERATORS: &[&str] = &[
    "avg", "count", "group", "max", "min", "quantile", "stddev", "stdvar", "sum",
];

/// The `message` the modelled `ValidationException` carries for a `randomCutForest` query
/// no supported aggregation operator wraps.
///
/// RECORDED BEHAVIOUR: a syntactically valid but unwrapped query is refused 400 with
/// "Unsupported PromQL expression for RandomCutForest, expression's root must be a
/// supported aggregation operator" — the WHOLE sentence is fixed: it echoes neither the
/// rejected expression nor the member path nor the operator list. This is the service
/// judging what the request MEANS, not a member failing a constraint the model declares,
/// so it is raised through [`request_validation_failed`] (no `fieldList`) rather than
/// through the member template [`member_validation_failed`] composes.
pub const RANDOM_CUT_FOREST_QUERY_UNSUPPORTED_MESSAGE: &str =
    "Unsupported PromQL expression for RandomCutForest, expression's root must be a \
     supported aggregation operator";

/// This service's ARN namespace (`OPERATIONS.md`: "ARN namespace: `aps`").
const ARN_SERVICE: &str = "aps";

/// Region used when a request carries no SigV4 credential scope to read one from —
/// the region the emulator's own generated client config defaults to.
const DEFAULT_REGION: &str = "us-east-1";

/// One stored resource-based policy: the members `DescribeResourcePolicy` reports,
/// plus the bookkeeping its lifecycle and its optimistic lock need. It lives ON the
/// workspace it is attached to (`/workspaces/{workspaceId}/policy`), so deleting the
/// workspace releases the policy with it.
#[derive(Debug, Clone)]
pub struct ResourcePolicy {
    /// The JSON policy document exactly as it was put — the service stores the
    /// caller's document and reports it back.
    pub policy_document: String,
    /// One of [`POLICY_STATUS_CREATING`], [`POLICY_STATUS_ACTIVE`],
    /// [`POLICY_STATUS_UPDATING`], [`POLICY_STATUS_DELETING`].
    pub status_code: String,
    /// The revision this document is at: minted by [`mint_revision_id`] on every put,
    /// and what a later put or delete names to take the optimistic lock.
    pub revision_id: String,
    /// When the clock owes this policy's next lifecycle transition, if one is pending:
    /// `CREATING`/`UPDATING` become `ACTIVE`, `DELETING` is removed.
    pub transition_due: Option<SystemTime>,
    /// The `clientToken` of the `PutResourcePolicy` call that produced this revision,
    /// which is what makes that call idempotent per its `@idempotencyToken`: replaying
    /// it answers with this revision instead of minting a second one.
    pub client_token: Option<String>,
    /// The `clientToken` of the `DeleteResourcePolicy` call that started a pending
    /// deletion, so replaying that exact call stays idempotent rather than conflicting
    /// with the deletion it already started.
    pub delete_token: Option<String>,
}

/// One stored workspace: every member `WorkspaceDescription` / `WorkspaceSummary`
/// report, plus the bookkeeping the lifecycle needs.
#[derive(Debug, Clone)]
pub struct Workspace {
    pub workspace_id: String,
    pub arn: String,
    /// `None` when the workspace carries no alias (none given, or one that was all
    /// blank space — see [`normalize_alias`]).
    ///
    /// This is the PUBLISHED alias: what every reader reports. A rename that has been
    /// accepted but has not settled yet is held in [`Workspace::pending_alias`] and
    /// lands here only when the workspace is settled again.
    pub alias: Option<String>,
    /// The alias an accepted-but-unsettled rename will publish, or `None` when no
    /// rename is in flight. The inner `Option` is the alias itself, so a rename that
    /// leaves the workspace with none is held here the same way one that sets a value
    /// is.
    ///
    /// A rename is accepted with no status to report (modelled 204, `Unit` output), so
    /// a reader has no signal that an alias it can see is not in effect yet: the new
    /// alias and the settled status therefore become visible in the SAME step, applied
    /// by [`ComAmazonawsAmpAmazonprometheusserviceState::materialize_due`] when the
    /// owed settling comes due. Until then every reader reports the alias the
    /// workspace had before the rename.
    pub pending_alias: Option<Option<String>>,
    /// One of [`STATUS_CREATING`], [`STATUS_ACTIVE`], [`STATUS_UPDATING`],
    /// [`STATUS_DELETING`].
    pub status_code: String,
    pub created_at: SystemTime,
    /// Sorted so a listing and a describe report the tags in one stable order.
    pub tags: BTreeMap<String, String>,
    pub kms_key_arn: Option<String>,
    /// The anomaly detectors this workspace holds, keyed by `anomalyDetectorId`. A
    /// detector is a CHILD of its workspace
    /// (`/workspaces/{workspaceId}/anomalydetectors`), so it lives in the workspace's
    /// own record: removing the workspace removes its detectors with it, and no second
    /// map can disagree about which workspace a detector belongs to.
    pub anomaly_detectors: HashMap<String, AnomalyDetector>,
    /// `clientToken -> anomalyDetectorId`, which is what makes `CreateAnomalyDetector`
    /// idempotent per its `@idempotencyToken`.
    pub anomaly_detector_tokens: HashMap<String, String>,
    /// When the clock owes this workspace's next lifecycle transition, if one is
    /// pending: `CREATING`/`UPDATING` become `ACTIVE`, `DELETING` is removed.
    pub transition_due: Option<SystemTime>,
    /// The `clientToken` of the `DeleteWorkspace` call that started a pending
    /// deletion, so replaying that exact call stays idempotent rather than
    /// conflicting with the deletion it already started.
    pub delete_token: Option<String>,
    /// The `clientToken` of the `UpdateWorkspaceAlias` call whose update is still in
    /// flight, which is what makes that `@idempotent` operation idempotent per its
    /// `@idempotencyToken`: replaying it is accepted again instead of conflicting with
    /// the update it already started. Forgotten the moment the update settles (see
    /// [`ComAmazonawsAmpAmazonprometheusserviceState::materialize_due`]), so a later
    /// call reusing the token is a new request.
    pub update_token: Option<String>,
    /// This workspace's alert manager definition, of which it holds at most one:
    /// `CreateAlertManagerDefinition` "creates the alert manager definition in a
    /// workspace" and refuses a second one, so the record hangs off the workspace and
    /// is released with it.
    pub alert_manager_definition: Option<AlertManagerDefinition>,
    /// What `DescribeWorkspaceConfiguration` reports and `UpdateWorkspaceConfiguration`
    /// writes: the workspace's label set limits, retention period and sample windows.
    pub configuration: WorkspaceConfiguration,
    /// This workspace's rules-and-alerting logging configuration, when it has one.
    ///
    /// `/workspaces/{workspaceId}/logging` names at most ONE such configuration per
    /// workspace — `LoggingConfigurationMetadata` is a single structure keyed by nothing
    /// but the workspace — so it lives ON the workspace record: a workspace that goes
    /// away takes its logging configuration with it.
    pub logging_configuration: Option<LoggingConfiguration>,
    /// The resource-based policy attached to this workspace, if one is attached:
    /// `PutResourcePolicy` creates or replaces it, `DescribeResourcePolicy` reads it
    /// and `DeleteResourcePolicy` removes it.
    pub policy: Option<ResourcePolicy>,
    /// This workspace's rule groups namespaces, keyed by `RuleGroupsNamespaceName`
    /// (unique within a workspace, which is what `CreateRuleGroupsNamespace` refuses a
    /// second time). They hang off the workspace record because the namespace is a
    /// child of the workspace: dropping the workspace releases every one of them.
    /// Sorted, so a listing reports one stable order.
    pub rule_groups_namespaces: BTreeMap<String, RuleGroupsNamespace>,
    /// The time series written to this workspace's Prometheus data plane, which is what
    /// an instant query against it reads back. They are the workspace's OWN data, so they
    /// live on its record: deleting the workspace releases them with everything else it
    /// holds. See [`metrics`].
    pub metrics: IngestedMetrics,
}

/// One stored rule groups namespace: every member `RuleGroupsNamespaceDescription` /
/// `RuleGroupsNamespaceSummary` report, plus the bookkeeping the lifecycle needs.
#[derive(Debug, Clone)]
pub struct RuleGroupsNamespace {
    pub name: String,
    pub arn: String,
    /// The rules file the namespace holds, exactly as the request carried it:
    /// `RuleGroupsNamespaceData` is a blob, so the bytes are stored unaltered and read
    /// back unaltered.
    pub data: Vec<u8>,
    /// One of [`NAMESPACE_STATUS_CREATING`], [`NAMESPACE_STATUS_ACTIVE`],
    /// [`NAMESPACE_STATUS_UPDATING`], [`NAMESPACE_STATUS_DELETING`].
    pub status_code: String,
    /// `RuleGroupsNamespaceStatus$statusReason` — "The reason for the failure, if any",
    /// so it is absent while nothing failed.
    pub status_reason: Option<String>,
    pub created_at: SystemTime,
    /// When the namespace was "most recently changed"
    /// (`RuleGroupsNamespaceDescription$modifiedAt`): the creation instant until a
    /// later call changes the record.
    pub modified_at: SystemTime,
    /// Sorted so a listing and a describe report the tags in one stable order.
    pub tags: BTreeMap<String, String>,
    /// When the clock owes this namespace's next lifecycle transition, if one is
    /// pending: `CREATING`/`UPDATING` become `ACTIVE`, `DELETING` is removed.
    pub transition_due: Option<SystemTime>,
    /// The `clientToken` of the call that created this namespace, which is what makes
    /// `CreateRuleGroupsNamespace` idempotent per its `@idempotencyToken`.
    pub create_token: Option<String>,
    /// The `clientToken` of the `PutRuleGroupsNamespace` update still in flight, which
    /// is what makes that `@idempotent` operation idempotent per its
    /// `@idempotencyToken`: replaying it answers with the update it already started
    /// instead of starting a second one. Forgotten the moment the update settles (see
    /// [`materialize_namespaces_due`]), so a later call reusing the token is a new
    /// request.
    pub update_token: Option<String>,
    /// The `clientToken` of the `DeleteRuleGroupsNamespace` call that started a pending
    /// deletion, so replaying that exact call stays idempotent rather than conflicting
    /// with the deletion it already started.
    pub delete_token: Option<String>,
}

/// One entry of a configuration's `limitsPerLabelSet`: a label set and the active time
/// series limit enforced for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelSetLimit {
    /// The label set, `labelName -> labelValue`. Sorted, so a describe reports one
    /// stable order whatever order the request's `LabelSet` map was iterated in.
    pub label_set: BTreeMap<String, String>,
    /// `LimitsPerLabelSetEntry$maxSeries`, absent when the request set none.
    pub max_series: Option<i64>,
}

/// One workspace's stored configuration — the record
/// `WorkspaceConfigurationDescription` reports.
#[derive(Debug, Clone)]
pub struct WorkspaceConfiguration {
    /// One of [`CONFIG_STATUS_ACTIVE`], [`CONFIG_STATUS_UPDATING`].
    pub status_code: String,
    /// `WorkspaceConfigurationStatus$statusReason` — "The reason for the current
    /// status, if a reason is available". No status this emulator reports has one.
    pub status_reason: Option<String>,
    /// The label sets with an enforced active time series limit, in the order the
    /// update that set them listed them.
    pub limits_per_label_set: Vec<LabelSetLimit>,
    /// Always present: a workspace retains metrics for some period from the moment it
    /// exists ([`DEFAULT_RETENTION_PERIOD_IN_DAYS`] until an update changes it).
    pub retention_period_in_days: i32,
    /// Always present, for the same reason:
    /// [`DEFAULT_OUT_OF_ORDER_TIME_WINDOW_IN_SECONDS`] until an update changes it.
    pub out_of_order_time_window_in_seconds: i32,
    /// Always present, for the same reason:
    /// [`DEFAULT_RULE_QUERY_OFFSET_IN_SECONDS`] until an update changes it.
    pub rule_query_offset_in_seconds: i32,
    /// When the clock owes this configuration's move out of `UPDATING` back to
    /// `ACTIVE`, if an update is pending.
    pub status_due: Option<SystemTime>,
    /// The `clientToken` of the `UpdateWorkspaceConfiguration` call that started the
    /// update now pending, so replaying that exact call answers with the update it
    /// already started instead of starting a second one. Forgotten when that update
    /// settles, so it never speaks for a configuration that is no longer updating.
    pub update_token: Option<String>,
}

impl Default for WorkspaceConfiguration {
    /// The configuration a freshly created workspace carries: nothing is updating, no
    /// label set limit is enforced, metrics are retained for the service's default
    /// period, and neither sample window is offset.
    fn default() -> Self {
        Self {
            status_code: CONFIG_STATUS_ACTIVE.to_string(),
            status_reason: None,
            limits_per_label_set: Vec::new(),
            retention_period_in_days: DEFAULT_RETENTION_PERIOD_IN_DAYS,
            out_of_order_time_window_in_seconds: DEFAULT_OUT_OF_ORDER_TIME_WINDOW_IN_SECONDS,
            rule_query_offset_in_seconds: DEFAULT_RULE_QUERY_OFFSET_IN_SECONDS,
            status_due: None,
            update_token: None,
        }
    }
}

/// One stored rules-and-alerting logging configuration: every member
/// `LoggingConfigurationMetadata` reports, plus the bookkeeping its lifecycle needs.
#[derive(Debug, Clone)]
pub struct LoggingConfiguration {
    /// The CloudWatch log group the vended log data is published to.
    pub log_group_arn: String,
    /// One of [`LOGGING_STATUS_CREATING`], [`LOGGING_STATUS_ACTIVE`],
    /// [`LOGGING_STATUS_UPDATING`], [`LOGGING_STATUS_DELETING`].
    pub status_code: String,
    /// `LoggingConfigurationStatus$statusReason` — "If failed, the reason for the
    /// failure", so `None` while nothing has failed.
    pub status_reason: Option<String>,
    pub created_at: SystemTime,
    /// When the configuration was most recently CHANGED, i.e. by the call that created
    /// it or last updated what it points at — not by the clock moving its status on.
    pub modified_at: SystemTime,
    /// When the clock owes this configuration's next transition, if one is pending:
    /// `CREATING`/`UPDATING` become `ACTIVE`, `DELETING` is removed.
    pub transition_due: Option<SystemTime>,
    /// The `clientToken` of the call that created it, never overwritten — so replaying
    /// that exact create still answers from the stored record after later updates, which
    /// is what `@idempotencyToken` asks for.
    pub client_token: Option<String>,
    /// The `clientToken` of the call that last UPDATED it. Replaying that exact token is
    /// the SAME request, so `UpdateLoggingConfiguration` answers from the stored record
    /// instead of applying the change twice. Kept apart from [`Self::client_token`] for
    /// the same reason `QueryLoggingConfiguration` keeps its create token apart.
    pub update_token: Option<String>,
}

impl LoggingConfiguration {
    /// Apply the transition the clock already owes this configuration at `now`, and
    /// report whether the workspace must stop holding it.
    ///
    /// The due instant was recorded by the call that started the transition, so it is
    /// owed whether or not anybody calls again. A configuration being DELETED is GONE
    /// once its due instant has passed — hence the `true` — and every other transient
    /// status this emulator puts a configuration in resolves to `ACTIVE`.
    fn materialize_due(&mut self, now: SystemTime) -> bool {
        if self.transition_due.is_none_or(|due| now < due) {
            return false;
        }
        self.transition_due = None;
        if self.status_code == LOGGING_STATUS_DELETING {
            return true;
        }
        self.status_code = LOGGING_STATUS_ACTIVE.to_string();
        false
    }
}

/// One destination of a query logging configuration: the `LoggingDestination` members
/// the service persists (`cloudWatchLogs.logGroupArn` and `filters.qspThreshold`), held
/// as plain data so the wire shapes are built from the STORED record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryLoggingDestination {
    /// `CloudWatchLogDestination$logGroupArn` — the log group the vended log data is
    /// published to.
    pub log_group_arn: String,
    /// `LoggingFilter$qspThreshold` — the Query Samples Processed threshold above which
    /// a query is logged (`@range(min: 0)`).
    pub qsp_threshold: i64,
}

/// One workspace's query logging configuration: every member
/// `QueryLoggingConfigurationMetadata` reports, plus the lifecycle bookkeeping.
///
/// A workspace holds at most one, which is why the store keys them by `workspaceId`
/// (`DescribeQueryLoggingConfiguration` takes nothing else to find one by).
#[derive(Debug, Clone)]
pub struct QueryLoggingConfiguration {
    pub workspace_id: String,
    /// One of [`QUERY_LOGGING_STATUS_CREATING`], [`QUERY_LOGGING_STATUS_ACTIVE`],
    /// [`QUERY_LOGGING_STATUS_UPDATING`], [`QUERY_LOGGING_STATUS_DELETING`].
    pub status_code: String,
    /// `QueryLoggingConfigurationStatus$statusReason` — "If there is a failure, the
    /// reason for the failure", so it stays `None` while nothing failed.
    pub status_reason: Option<String>,
    /// `LoggingDestinations` is modelled `len 1..1`, so this holds exactly one entry.
    pub destinations: Vec<QueryLoggingDestination>,
    pub created_at: SystemTime,
    pub modified_at: SystemTime,
    /// When the clock owes this configuration's next transition, if one is pending:
    /// `CREATING`/`UPDATING` become `ACTIVE`, `DELETING` is removed.
    pub transition_due: Option<SystemTime>,
    /// The `clientToken` of the `CreateQueryLoggingConfiguration` call that created
    /// this configuration, never overwritten — so replaying that exact create still
    /// answers from the stored record after later updates, which is what
    /// `@idempotencyToken` asks for.
    pub create_token: Option<String>,
    /// The `clientToken` of the call that last UPDATED this configuration. Replaying
    /// that exact token is the SAME update, so `UpdateQueryLoggingConfiguration` answers
    /// from the stored record instead of applying the change twice. Kept apart from
    /// [`Self::create_token`] so a replay is only ever matched against the token the
    /// SAME operation recorded: a token another operation used names a different
    /// request, which this one has never carried out.
    pub update_token: Option<String>,
}

/// One scraper's logging configuration: every member
/// `DescribeScraperLoggingConfigurationResponse` reports, plus the lifecycle
/// bookkeeping.
///
/// A scraper holds at most one, which is why the store keys them by `scraperId`
/// (`DescribeScraperLoggingConfiguration` takes nothing else to find one by), exactly as
/// [`QueryLoggingConfiguration`] is keyed by the workspace that holds it.
#[derive(Debug, Clone)]
pub struct ScraperLoggingConfiguration {
    pub scraper_id: String,
    /// One entry of the modelled `ScraperLoggingConfigurationStatusCode` enum — see
    /// [`SCRAPER_LOGGING_STATUS_ACTIVE`].
    pub status_code: String,
    /// `ScraperLoggingConfigurationStatus$statusReason` — the reason for a failure, so
    /// it stays `None` while nothing failed.
    pub status_reason: Option<String>,
    /// `loggingDestination` (@required) — the destination the scraper's logs go to.
    pub logging_destination: crate::ops::types::ScraperLoggingDestination,
    /// `scraperComponents` is modelled `len 1.., @uniqueItems`, so a stored
    /// configuration holds at least one component.
    pub scraper_components: Vec<crate::ops::types::ScraperComponent>,
    /// `modifiedAt` (@required) — when the record itself last changed.
    pub modified_at: SystemTime,
    /// When the clock owes this configuration's next transition, if one is pending:
    /// `DELETING` is removed, any other transient status becomes `ACTIVE`.
    pub transition_due: Option<SystemTime>,
    /// The `clientToken` of the call that last CHANGED this configuration (the start of
    /// a deletion). Replaying that exact call is the SAME request, so a handler answers
    /// from the stored record instead of applying the change twice.
    pub client_token: Option<String>,
}

/// One stored scraper: every member `ScraperSummary` reports.
///
/// `CreateScraper` puts one record here per accepted create, and `ListScrapers` reports
/// the scrapers this store HOLDS, so the listing is computed from state rather than
/// announced — an account that has never created one lists the empty page AWS lists for
/// it. `DeleteScraper` moves the record it names to `DELETING` and lets the clock
/// drop it ([`ComAmazonawsAmpAmazonprometheusserviceState::materialize_due`]);
/// `UpdateScraper` replaces the members it carries and moves the record to `UPDATING`
/// until that same clock settles it.
///
/// Every member of `ScraperDescription` lives here too ([`scraper_description`]), so a
/// describe reports the record rather than a second copy of it.
#[derive(Debug, Clone)]
pub struct Scraper {
    pub scraper_id: String,
    pub arn: String,
    /// `None` when the scraper carries no alias; `ScraperSummary$alias` is optional.
    pub alias: Option<String>,
    pub role_arn: String,
    /// One entry of the modelled `ScraperStatusCode` enum
    /// (`crate::ops::types::SCRAPER_STATUS_CODE_VALUES`).
    pub status_code: String,
    /// "If there is a failure, the reason for the failure"
    /// (`ScraperSummary$statusReason`), so `None` while nothing has failed.
    pub status_reason: Option<String>,
    pub created_at: SystemTime,
    pub last_modified_at: SystemTime,
    pub tags: BTreeMap<String, String>,
    /// "The Amazon EKS cluster from which the scraper collects metrics."
    pub source: crate::ops::types::Source,
    /// "The destination where the scraper sends metrics."
    pub destination: crate::ops::types::Destination,
    pub role_configuration: Option<crate::ops::types::RoleConfiguration>,
    /// `ScraperSummary$exporters` is modelled `len ..1`; empty when none is configured.
    pub exporters: Vec<crate::ops::types::ExporterConfiguration>,
    /// "The configuration in use by the scraper."
    /// (`ScraperDescription$scrapeConfiguration`, @required) — the base-64 encoded YAML
    /// the caller last supplied, stored exactly as given.
    pub scrape_configuration: crate::ops::types::ScrapeConfiguration,
    /// When the clock owes this scraper's next lifecycle transition, if one is pending:
    /// `CREATING`/`UPDATING` become `ACTIVE`, `DELETING` is removed.
    pub transition_due: Option<SystemTime>,
    /// The `clientToken` of the `DeleteScraper` call that started a pending deletion, so
    /// replaying that exact call stays idempotent rather than conflicting with the
    /// deletion it already started.
    pub delete_token: Option<String>,
    /// The `clientToken` of the `UpdateScraper` call that started a pending update, for
    /// the same reason [`Scraper::delete_token`] records a deletion's: replaying that
    /// exact call is the SAME request, so it answers from the record that call wrote
    /// instead of applying the change twice. Cleared once the update settles, since the
    /// token then stands for no pending change.
    pub update_token: Option<String>,
}

/// One stored anomaly detector: every member `AnomalyDetectorDescription` /
/// `AnomalyDetectorSummary` report, plus the bookkeeping its lifecycle needs.
#[derive(Debug, Clone)]
pub struct AnomalyDetector {
    pub anomaly_detector_id: String,
    /// The workspace this detector belongs to — the `ws-` segment of its ARN.
    pub workspace_id: String,
    pub arn: String,
    /// `AnomalyDetectorDescription$alias` is **required**, so a detector always has one:
    /// the `alias` its `CreateAnomalyDetector` was given, or — for a detector brought into
    /// existence by `PutAnomalyDetector`, whose request carries no `alias` member — the
    /// `anomalyDetectorId` that call addressed it by, the only name its caller gave it.
    pub alias: String,
    pub evaluation_interval_in_seconds: i32,
    pub missing_data_action: Option<crate::ops::types::AnomalyDetectorMissingDataAction>,
    pub configuration: crate::ops::types::AnomalyDetectorConfiguration,
    /// Sorted, so a describe reports the labels in one stable order.
    pub labels: BTreeMap<String, String>,
    /// One of [`STATUS_CREATING`], [`STATUS_ACTIVE`], [`STATUS_UPDATING`],
    /// [`STATUS_DELETING`].
    pub status_code: String,
    pub created_at: SystemTime,
    pub modified_at: SystemTime,
    /// Sorted so a describe reports the tags in one stable order.
    pub tags: BTreeMap<String, String>,
    /// When the clock owes this detector's next lifecycle transition, if one is
    /// pending: `CREATING` becomes `ACTIVE`, `UPDATING` becomes `ACTIVE`, `DELETING`
    /// is removed.
    pub transition_due: Option<SystemTime>,
    /// The `clientToken` of the change STILL PENDING on this detector — the update a
    /// `PutAnomalyDetector` applied, or the creation it performed — for the same reason
    /// [`Scraper::update_token`] records a scraper's: replaying that exact call
    /// is the SAME request, so it answers from the record that call wrote instead of
    /// applying the change twice. Cleared once the change settles, since the token then
    /// stands for no pending change.
    pub update_token: Option<String>,
}

/// The transitions the clock owes inside ONE Region's store, and the ARN-addressed
/// lookups over its records. The store itself — and what bounds how many of them there
/// are — lives in [`regions`].
impl RegionalState {
    /// Apply every lifecycle transition the clock already owes at `now` in this Region.
    ///
    /// Driven by [`ComAmazonawsAmpAmazonprometheusserviceState::materialize_due`], which
    /// every handler calls first; see there for why a reporting operation may write this.
    pub fn materialize_due(&mut self, now: SystemTime) {
        let mut deleted: Vec<String> = Vec::new();
        for workspace in self.workspaces.values_mut() {
            // The alert manager definition's own pending transition, owed on the same
            // terms: the record settles itself and says when it is gone.
            if workspace
                .alert_manager_definition
                .as_mut()
                .is_some_and(|definition| definition.settle_due(now))
            {
                workspace.alert_manager_definition = None;
            }

            // A detector's own pending transitions, owed on the same terms. A workspace
            // removed below takes its detectors with it, so this only has to settle the
            // detectors of a workspace that stays.
            let mut deleted_detectors: Vec<String> = Vec::new();
            for detector in workspace.anomaly_detectors.values_mut() {
                let Some(detector_due) = detector.transition_due else {
                    continue;
                };
                if now < detector_due {
                    continue;
                }
                detector.transition_due = None;
                if detector.status_code == STATUS_DELETING {
                    deleted_detectors.push(detector.anomaly_detector_id.clone());
                } else {
                    // Only the lifecycle status moves here — which `status` reports —
                    // so `modifiedAt` keeps the instant the record itself last changed.
                    detector.status_code = STATUS_ACTIVE.to_string();
                    // The token identified the update that just settled, so it stops
                    // standing for one: a later call carrying it is a new request.
                    detector.update_token = None;
                }
            }
            for detector_id in deleted_detectors {
                workspace.anomaly_detectors.remove(&detector_id);
                workspace
                    .anomaly_detector_tokens
                    .retain(|_, id| id != &detector_id);
            }

            if workspace.configuration.status_due.is_some_and(|due| now >= due) {
                workspace.configuration.status_due = None;
                workspace.configuration.status_code = CONFIG_STATUS_ACTIVE.to_string();
                workspace.configuration.status_reason = None;
                // The token identified the update that just settled, so it stops
                // standing for one: a later call carrying it is a new request.
                workspace.configuration.update_token = None;
            }
            // A rules-and-alerting logging configuration settles on its own clock, and
            // the removal `DeleteLoggingConfiguration` owes leaves the workspace holding
            // none at all.
            if workspace
                .logging_configuration
                .as_mut()
                .is_some_and(|logging| logging.materialize_due(now))
            {
                workspace.logging_configuration = None;
            }
            // The attached policy's transition is owed on its own clock, independently
            // of the workspace's: a policy put on an active workspace becomes ACTIVE
            // without the workspace moving at all.
            let policy_removed = match workspace.policy.as_mut() {
                Some(policy) if policy.transition_due.is_some_and(|due| now >= due) => {
                    policy.transition_due = None;
                    if policy.status_code == POLICY_STATUS_DELETING {
                        true
                    } else {
                        policy.status_code = POLICY_STATUS_ACTIVE.to_string();
                        false
                    }
                }
                _ => false,
            };
            if policy_removed {
                workspace.policy = None;
            }
            materialize_namespaces_due(&mut workspace.rule_groups_namespaces, now);
            let Some(due) = workspace.transition_due else {
                continue;
            };
            if now < due {
                continue;
            }
            workspace.transition_due = None;
            if workspace.status_code == STATUS_DELETING {
                // Dropping the workspace drops its logging configuration with it.
                deleted.push(workspace.workspace_id.clone());
            } else {
                workspace.status_code = STATUS_ACTIVE.to_string();
                // The settling is the one step that publishes what the update changed:
                // the alias an accepted rename is holding becomes visible here,
                // together with the status it settles to, so no reader ever sees the
                // new alias on a workspace that is not yet settled.
                if let Some(pending) = workspace.pending_alias.take() {
                    workspace.alias = pending;
                }
                // The token identified the update that just settled, so it stops
                // standing for one: a later call carrying it is a new request.
                workspace.update_token = None;
            }
        }
        for workspace_id in deleted {
            self.workspaces.remove(&workspace_id);
            self.client_tokens.retain(|_, id| id != &workspace_id);
            // A query logging configuration belongs to its workspace, so it goes when
            // the workspace does.
            self.query_logging.remove(&workspace_id);
        }

        let mut dropped: Vec<String> = Vec::new();
        for configuration in self.query_logging.values_mut() {
            let Some(due) = configuration.transition_due else {
                continue;
            };
            if now < due {
                continue;
            }
            configuration.transition_due = None;
            if configuration.status_code == QUERY_LOGGING_STATUS_DELETING {
                dropped.push(configuration.workspace_id.clone());
            } else {
                configuration.status_code = QUERY_LOGGING_STATUS_ACTIVE.to_string();
            }
        }
        for workspace_id in dropped {
            self.query_logging.remove(&workspace_id);
        }

        // A scraper is an account-level resource, so its pending transition is owed on
        // its own clock: one being deleted is GONE once its due instant has passed, and
        // one in any other transient status settles to `ACTIVE`.
        let mut removed_scrapers: Vec<String> = Vec::new();
        self.scrapers.retain(|_, scraper| match scraper.transition_due {
            Some(due) if now >= due => {
                scraper.transition_due = None;
                if scraper.status_code == STATUS_DELETING {
                    removed_scrapers.push(scraper.scraper_id.clone());
                    false
                } else {
                    scraper.status_code = STATUS_ACTIVE.to_string();
                    // The token identified the change that just settled, so it stops
                    // standing for one: a later call carrying it is a new request.
                    scraper.update_token = None;
                    true
                }
            }
            _ => true,
        });
        for scraper_id in removed_scrapers {
            // A scraper's logging configuration belongs to that scraper, so it goes
            // when the scraper does.
            self.scraper_logging.remove(&scraper_id);
            // So does the create token that named it: with the scraper gone there is no
            // record for a replay of that create to answer from.
            self.scraper_tokens.retain(|_, id| id != &scraper_id);
        }

        // A scraper's logging configuration settles on its own clock too, on the same
        // terms a workspace's query logging configuration does.
        let mut dropped_scraper_logging: Vec<String> = Vec::new();
        for configuration in self.scraper_logging.values_mut() {
            let Some(due) = configuration.transition_due else {
                continue;
            };
            if now < due {
                continue;
            }
            configuration.transition_due = None;
            if configuration.status_code == SCRAPER_LOGGING_STATUS_DELETING {
                dropped_scraper_logging.push(configuration.scraper_id.clone());
            } else {
                configuration.status_code = SCRAPER_LOGGING_STATUS_ACTIVE.to_string();
            }
        }
        for scraper_id in dropped_scraper_logging {
            self.scraper_logging.remove(&scraper_id);
        }
    }

    /// The taggable resource the ARN `resource_arn` names, when THIS REGION's store holds
    /// one.
    ///
    /// `TagResource` enumerates the kinds there are — "The only resources that can be
    /// tagged are rule groups namespaces, scrapers, and workspaces" — and an anomaly
    /// detector is one as well: `CreateAnomalyDetector` accepts `tags` and every
    /// detector reader reports them, while `PutAnomalyDetector` carries no `tags` at
    /// all, so the ARN-addressed tagging operations are the only way a detector's tags
    /// ever change — exactly as they are for a rule groups namespace, whose
    /// `PutRuleGroupsNamespace` carries none either.
    ///
    /// Resolved by looking at the stored records, so there is no index that can
    /// disagree with them about which resource an ARN names, miss a kind, or outlive
    /// the resource it named. The comparison is exact and every stored ARN carries the
    /// Region of the store holding it, so an ARN naming another Region resolves to
    /// nothing here — which is what a caller addressing this Region's endpoint is told.
    pub fn taggable_of_arn(&self, resource_arn: &str) -> Option<TaggedResource> {
        for (workspace_id, workspace) in &self.workspaces {
            if workspace.arn == resource_arn {
                return Some(TaggedResource::Workspace(workspace_id.clone()));
            }
            if let Some(namespace) = workspace
                .rule_groups_namespaces
                .values()
                .find(|namespace| namespace.arn == resource_arn)
            {
                return Some(TaggedResource::RuleGroupsNamespace(
                    workspace_id.clone(),
                    namespace.name.clone(),
                ));
            }
            if let Some(detector) = workspace
                .anomaly_detectors
                .values()
                .find(|detector| detector.arn == resource_arn)
            {
                return Some(TaggedResource::AnomalyDetector(
                    workspace_id.clone(),
                    detector.anomaly_detector_id.clone(),
                ));
            }
        }
        self.scrapers
            .values()
            .find(|scraper| scraper.arn == resource_arn)
            .map(|scraper| TaggedResource::Scraper(scraper.scraper_id.clone()))
    }

    /// The refusal a scraper `destination` earns when the reference it carries does not
    /// resolve — `None` when it does, or when it names a resource no local state speaks
    /// for.
    ///
    /// A destination is a REFERENCE to another resource, and
    /// `ResourceNotFoundException` is modelled on every operation that stores one for
    /// exactly that: "The request references a resources that doesn't exist." The
    /// reference this service can resolve is a workspace of its own in the account the
    /// emulator serves, so one that names no workspace this store holds does not resolve
    /// — reported the way every other unresolved workspace is, with the constant sentence
    /// in `message`, the id in `resourceId` and the parent's AWS type in `resourceType`.
    /// A destination in another account (which `roleConfiguration` exists to enable) or in
    /// another service names a resource this store holds nothing about, so it is taken as
    /// given.
    pub fn unresolved_destination_workspace(
        &self,
        destination: &crate::ops::types::Destination,
    ) -> Option<crate::ops::types::ResourceNotFoundException> {
        let workspace_arn = destination_workspace_arn(destination)?;
        if !names_this_account(workspace_arn) {
            return None;
        }
        if matches!(
            self.taggable_of_arn(workspace_arn),
            Some(TaggedResource::Workspace(_))
        ) {
            return None;
        }
        Some(workspace_not_found(workspace_id_of_arn(workspace_arn)))
    }

    /// The tag map the resource `resource_arn` names CARRIES, for a reader to report —
    /// `None` when the ARN names no resource this store holds.
    pub fn tags_of_arn(&self, resource_arn: &str) -> Option<&BTreeMap<String, String>> {
        match self.taggable_of_arn(resource_arn)? {
            TaggedResource::Workspace(workspace_id) => {
                self.workspaces.get(&workspace_id).map(|w| &w.tags)
            }
            TaggedResource::RuleGroupsNamespace(workspace_id, name) => self
                .workspaces
                .get(&workspace_id)?
                .rule_groups_namespaces
                .get(&name)
                .map(|namespace| &namespace.tags),
            TaggedResource::AnomalyDetector(workspace_id, anomaly_detector_id) => self
                .workspaces
                .get(&workspace_id)?
                .anomaly_detectors
                .get(&anomaly_detector_id)
                .map(|detector| &detector.tags),
            TaggedResource::Scraper(scraper_id) => {
                self.scrapers.get(&scraper_id).map(|scraper| &scraper.tags)
            }
        }
    }

    /// The same tag map, for a writer to merge into or remove from — so `TagResource`
    /// and `UntagResource` change the record every reader reports, whatever kind of
    /// resource the ARN names.
    pub fn tags_of_arn_mut(&mut self, resource_arn: &str) -> Option<&mut BTreeMap<String, String>> {
        match self.taggable_of_arn(resource_arn)? {
            TaggedResource::Workspace(workspace_id) => {
                self.workspaces.get_mut(&workspace_id).map(|w| &mut w.tags)
            }
            TaggedResource::RuleGroupsNamespace(workspace_id, name) => self
                .workspaces
                .get_mut(&workspace_id)?
                .rule_groups_namespaces
                .get_mut(&name)
                .map(|namespace| &mut namespace.tags),
            TaggedResource::AnomalyDetector(workspace_id, anomaly_detector_id) => self
                .workspaces
                .get_mut(&workspace_id)?
                .anomaly_detectors
                .get_mut(&anomaly_detector_id)
                .map(|detector| &mut detector.tags),
            TaggedResource::Scraper(scraper_id) => self
                .scrapers
                .get_mut(&scraper_id)
                .map(|scraper| &mut scraper.tags),
        }
    }
}

/// One resource the `/tags/{resourceArn}` operations can address, as the store reaches
/// it: a child resource is named by its parent workspace too, because that is where the
/// record lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    /// A workspace, by `workspaceId`.
    Workspace(String),
    /// A rule groups namespace, by its workspace's `workspaceId` and its own name.
    RuleGroupsNamespace(String, String),
    /// An anomaly detector, by its workspace's `workspaceId` and its own id.
    AnomalyDetector(String, String),
    /// A scraper, by `scraperId`.
    Scraper(String),
}

/// Whether a workspace and its rules-and-alerting logging configuration are in the state
/// the modelled `LoggingConfigurationStatusCode` requires before an update or a deletion
/// may act on that configuration.
///
/// Every entry of that enum documents the same rule: "Update/Deletion is disallowed until
/// logging configuration is ACTIVE and workspace status is ACTIVE." Takes the two statuses
/// rather than the workspace, so a caller that has already answered the missing-resource
/// case reports the pair it refused on.
pub fn logging_mutation_allowed(workspace_status: &str, logging_status: &str) -> bool {
    workspace_status == STATUS_ACTIVE && logging_status == LOGGING_STATUS_ACTIVE
}

/// Whether a workspace and its attached policy are in the state the modelled
/// `WorkspacePolicyStatusCode` requires before a resource-policy put or delete may act.
///
/// Every entry of that enum documents the same rule: "Update/Deletion is disallowed
/// until Resource-based Policy is ACTIVE and Workspace is ACTIVE." A workspace with no
/// policy attached yet has nothing to wait for, so only the workspace's own status
/// gates the first put.
pub fn policy_mutation_allowed(workspace: &Workspace) -> bool {
    workspace.status_code == STATUS_ACTIVE
        && workspace
            .policy
            .as_ref()
            .is_none_or(|policy| policy.status_code == POLICY_STATUS_ACTIVE)
}

/// Whether a request naming `revision_id` may act on the policy a workspace currently
/// holds — the optimistic lock `PutResourcePolicyRequest$revisionId` and
/// `DeleteResourcePolicyRequest$revisionId` describe.
///
/// "If you don't specify a revision ID, the policy is updated regardless of its current
/// revision", so an absent id always matches. Otherwise the id must name the policy's
/// current revision, and on a workspace that holds no policy the only id that matches
/// is [`NO_POLICY_REVISION_ID`]: "For the first PUT request on a workspace that doesn't
/// have an existing resource policy, you can specify `NO_POLICY` as the revision ID."
///
/// A request whose `revisionId` does not match is refused through
/// [`stale_revision_rejection`].
pub fn revision_matches(policy: Option<&ResourcePolicy>, revision_id: Option<&str>) -> bool {
    match (policy, revision_id) {
        (_, None) => true,
        (None, Some(revision_id)) => revision_id == NO_POLICY_REVISION_ID,
        (Some(policy), Some(revision_id)) => revision_id == policy.revision_id,
    }
}

/// Apply every rule-groups-namespace transition the clock owes at `now` inside one
/// workspace: a namespace whose due instant has passed becomes `ACTIVE`, or is removed
/// when it was being deleted. Called from [`RegionalState::materialize_due`], for the same
/// reason — the transition was owed at an instant recorded when the call that started
/// it ran, whether or not anybody calls again.
fn materialize_namespaces_due(
    namespaces: &mut BTreeMap<String, RuleGroupsNamespace>,
    now: SystemTime,
) {
    namespaces.retain(|_, namespace| match namespace.transition_due {
        Some(due) if now >= due => {
            namespace.transition_due = None;
            if namespace.status_code == NAMESPACE_STATUS_DELETING {
                false
            } else {
                namespace.status_code = NAMESPACE_STATUS_ACTIVE.to_string();
                // The token identified the update that just settled, so it stops
                // standing for one: a later call carrying it is a new request.
                namespace.update_token = None;
                true
            }
        }
        _ => true,
    });
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAmpAmazonprometheusserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAmpAmazonprometheusserviceState>)
}

/// The region this request was signed for, defaulting to [`DEFAULT_REGION`] when it
/// carries no SigV4 credential scope (an anonymous or host-routed request).
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// Record a call that CHANGED this service's state, so an emulator run is auditable the
/// way CloudTrail makes a real one: the operation, the resource it changed and the
/// access-key id the REQUEST presented.
///
/// EVERY handler of this crate calls this on each path where it writes to the store, and
/// only there: a reporting operation changes nothing to account for, and a path that
/// answers from the stored record without writing — an idempotency replay, a refusal —
/// has nothing to record either. So the stream holds one record per completed change,
/// and an operator reading it sees every change the run made rather than the subset of
/// handlers that happened to be written with the call.
///
/// A resource the service gives no ARN of its own — a logging or query logging
/// configuration, an alert manager definition, a resource-based policy — is recorded
/// under the ARN of the workspace or scraper it hangs off, which is the resource the
/// change is addressed to.
///
/// Carries request metadata ONLY — `emulator_server::extract_akid` reads the id out of
/// the presented `Authorization` header. The emulator's own credentials, the caller's
/// secrets and the stored document itself stay out of it, so the record names what
/// happened without reproducing what was sent.
pub fn record_state_change(operation: &str, resource: &str, parts: &::http::request::Parts) {
    tracing::info!(
        operation,
        resource,
        access_key_id = %::emulator_server::extract_akid(&parts.headers),
        "aps state change"
    );
}

/// A workspace ARN: `arn:aws:aps:<region>:<account>:workspace/<workspaceId>`, as
/// `WorkspaceArn`'s pattern `^arn:aws[-a-z]*:aps:[-a-z0-9]+:[0-9]{12}:workspace/.+$`
/// requires. The account segment comes from the engine, never from a literal.
pub fn workspace_arn(region: &str, workspace_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("workspace/{workspace_id}"))
}

/// An anomaly detector ARN:
/// `arn:aws:aps:<region>:<account>:anomalydetector/<workspaceId>/<anomalyDetectorId>`,
/// as `AnomalyDetectorArn`'s pattern
/// `^arn:aws[-a-z]*:aps:[-a-z0-9]+:[0-9]{12}:anomalydetector/ws-.+/ad-.+$` requires.
/// The account segment comes from the engine, never from a literal.
pub fn anomaly_detector_arn(region: &str, workspace_id: &str, anomaly_detector_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("anomalydetector/{workspace_id}/{anomaly_detector_id}"),
    )
}

/// A rule groups namespace ARN, in the form
/// `RuleGroupsNamespaceDescription$arn` documents: "For example,
/// `arn:aws:aps:<region>:123456789012:rulegroupsnamespace/ws-example1-1234-abcd-5678-ef90abcd1234/rulesfile1`".
/// The account segment comes from the engine, never from a literal.
pub fn rule_groups_namespace_arn(region: &str, workspace_id: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("rulegroupsnamespace/{workspace_id}/{name}"),
    )
}

/// A scraper ARN, in the form `ScraperDescription$arn` documents: "For example,
/// `arn:aws:aps:<region>:123456798012:scraper/s-example1-1234-abcd-5678-ef9012abcd34`".
/// The account segment comes from the engine, never from a literal.
pub fn scraper_arn(region: &str, scraper_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("scraper/{scraper_id}"))
}

/// The ARN of the IAM role one scraper collects through.
///
/// `CreateScraper` documents that the service makes it: "An IAM role will be created for
/// you that Amazon Managed Service for Prometheus uses to access the metrics in your
/// source." So a scraper's `roleArn` — `@required` on `ScraperDescription`, and documented
/// "the IAM role that provides permissions for the scraper to discover and collect metrics
/// on your behalf" with the example
/// `arn:aws:iam::123456789012:role/service-role/AmazonGrafanaServiceRole-12example` — is
/// the service-linked role Amazon Managed Service for Prometheus creates for a scraper,
/// under the path it reserves for that role. One role per scraper, so the name carries
/// the scraper's id; the account segment comes from the engine, and IAM is global, so the
/// region segment is empty, as `IamRoleArn`'s pattern
/// (`^arn:aws[-a-z]*:iam::[0-9]{12}:role/.+$`) requires.
pub fn scraper_role_arn(scraper_id: &str) -> String {
    ::engine::emulator_arn(
        "iam",
        "",
        &format!("role/{SCRAPER_ROLE_PATH}/{SCRAPER_ROLE_NAME_PREFIX}_{scraper_id}"),
    )
}

/// The IAM path the scraper's service-linked role sits under, and the name that role
/// carries before the scraper's own id — see [`scraper_role_arn`].
const SCRAPER_ROLE_PATH: &str = "aws-service-role/scraper.aps.amazonaws.com";
/// See [`SCRAPER_ROLE_PATH`].
const SCRAPER_ROLE_NAME_PREFIX: &str = "AWSServiceRoleForAmazonPrometheusScraper";

/// The workspace id a workspace ARN names: the resource segment of
/// `arn:aws:aps:<region>:<account>:workspace/<workspaceId>`, which is what the modelled
/// `ResourceNotFoundException$resourceId` reports for a reference that does not resolve.
/// An ARN carrying no resource segment is reported as it came, so the refusal still names
/// what the caller sent.
pub fn workspace_id_of_arn(workspace_arn: &str) -> &str {
    workspace_arn
        .split('/')
        .next_back()
        .filter(|id| !id.is_empty())
        .unwrap_or(workspace_arn)
}

/// Whether `arn` names a resource in the account this emulator serves, so this store is
/// the one that would hold it. A reference to any other account names a resource no local
/// state can speak for, which is why resolution is attempted only for this one.
///
/// The account is the fifth colon-separated segment of an ARN
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`).
pub fn names_this_account(arn: &str) -> bool {
    arn.split(':').nth(4) == Some(::engine::emulator_account_id())
}

/// The URL a workspace's Prometheus API is served at, in the form
/// `WorkspaceDescription$prometheusEndpoint` documents: "For example,
/// `https://aps-workspaces.<region>.amazonaws.com/workspaces/ws-.../api/v1/`".
///
/// The HOST is the endpoint the REQUEST reached this service on, never a host composed
/// from a literal: the member is "The Prometheus endpoint available for this workspace",
/// so it has to be an address the caller that read it can reach — a Prometheus agent's
/// remote_write target, a Grafana datasource and an SDK caller all follow it. The path is
/// the documented one ([`workspace_prometheus_path`]), which is also the path space this
/// service serves that API under, so following the announced URL lands on the served
/// route.
pub fn prometheus_endpoint(parts: &::http::request::Parts, workspace_id: &str) -> String {
    format!(
        "{}://{}{}",
        request_scheme(parts),
        request_authority(parts),
        workspace_prometheus_path(workspace_id)
    )
}

/// The scheme the request reached this service on: the one its URI names when it carries
/// an absolute URI, and otherwise `https`, which is the only scheme the emulator's
/// listener serves.
fn request_scheme(parts: &::http::request::Parts) -> &str {
    parts.uri.scheme_str().unwrap_or("https")
}

/// The authority the request was addressed to: its `Host` header, or the authority of an
/// absolute request URI.
///
/// A request that names neither (neither HTTP/1.1 nor HTTP/2 permits one, but the type
/// allows it) is answered for the address the emulator's own listener serves on, so the
/// announced endpoint is still one a caller can reach.
fn request_authority(parts: &::http::request::Parts) -> String {
    parts
        .headers
        .get(::http::header::HOST)
        .and_then(|host| host.to_str().ok())
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .map(ToString::to_string)
        .or_else(|| parts.uri.authority().map(::http::uri::Authority::to_string))
        .unwrap_or_else(|| {
            format!(
                "{}:{}",
                ::emulator_server::DEFAULT_BIND_ADDRESS,
                ::emulator_server::data_port_from_env()
            )
        })
}

/// A fresh workspace id of the shape the service mints: `ws-` followed by a UUID,
/// which `WorkspaceId`'s pattern (`[0-9A-Za-z][-.0-9A-Z_a-z]*`, length 1..64)
/// accepts and `WorkspaceDescription$workspaceId` shows ("For example,
/// `ws-example1-1234-abcd-5678-ef90abcd1234`").
pub fn mint_workspace_id(counter: &mut u64) -> String {
    mint_id(b"workspace", "ws", counter)
}

/// A fresh anomaly detector id of the shape the service mints: `ad-` followed by a
/// UUID, which `AnomalyDetectorId`'s pattern (`^ad-[0-9A-Za-z][-.0-9A-Z_a-z]*$`,
/// length 1..64) requires. Minted exactly as [`mint_workspace_id`] is, off the same
/// store counter, so two resources never share an id's input.
pub fn mint_anomaly_detector_id(counter: &mut u64) -> String {
    mint_id(b"anomalydetector", "ad", counter)
}

/// A fresh scraper id of the shape the service mints: `s-` followed by a UUID, which
/// `ScraperId`'s pattern (`^[0-9A-Za-z][-.0-9A-Z_a-z]*$`, length 1..64) accepts and
/// `ScraperDescription$scraperId` shows ("For example,
/// `s-example1-1234-abcd-5678-ef9012abcd34`"). Minted exactly as [`mint_workspace_id`]
/// is, off the same store counter, so two resources never share an id's input.
pub fn mint_scraper_id(counter: &mut u64) -> String {
    mint_id(b"scraper", "s", counter)
}

/// A fresh resource-policy revision id: the opaque, server-minted value
/// `PutResourcePolicyResponse$revisionId` carries and a later put or delete names to
/// take the optimistic lock. Every put mints a new one, so no two revisions of a
/// policy — or of two policies — share an id. A revision names no resource kind, so it
/// is the bare UUID [`mint_uuid`] mints rather than a prefixed id.
pub fn mint_revision_id(counter: &mut u64) -> String {
    mint_uuid(b"resource-policy-revision", counter)
}

/// `<prefix>-<uuid>` for one resource `domain`: the shared body of
/// [`mint_workspace_id`] and [`mint_anomaly_detector_id`], so both kinds of id are
/// minted by ONE rule.
fn mint_id(domain: &[u8], prefix: &str, counter: &mut u64) -> String {
    format!("{prefix}-{}", mint_uuid(domain, counter))
}

/// A fresh UUID for one of this store's minted identifiers, in the 8-4-4-4-12 hex form.
///
/// It is a digest of `domain` (so identifiers of different kinds never collide) and
/// this store's id counter (unique within the process), folded with a per-process seed
/// (so two emulator processes do not mint the same ids) and hashed so the identifier
/// discloses neither. It takes the counter rather than `&mut self` so a handler holding
/// a stored record borrowed can still mint — which is what lets it mint only AFTER
/// every refusal has been answered.
fn mint_uuid(domain: &[u8], counter: &mut u64) -> String {
    *counter += 1;
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(counter.to_be_bytes());
    hasher.update(process_seed().to_be_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    let hex = hex::encode(digest);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// A seed drawn once per process, so ids minted by two emulator processes from the same
/// counter still differ.
fn process_seed() -> u128 {
    static SEED: OnceLock<u128> = OnceLock::new();
    *SEED.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or_default()
    })
}

/// The alias to STORE for a given `alias` member.
///
/// `CreateWorkspace$alias` and `UpdateWorkspaceAlias$alias` both document that
/// "blank spaces at the beginning or end of the alias that you specify will be
/// trimmed from the value used", so the stored value is the trimmed one. An alias
/// that is nothing but blank space trims to the empty string, which `WorkspaceAlias`
/// (length 1..100) cannot hold, so it leaves the workspace with no alias at all.
pub fn normalize_alias(alias: Option<&str>) -> Option<String> {
    alias
        .map(str::trim)
        .filter(|trimmed| !trimmed.is_empty())
        .map(ToString::to_string)
}

/// How many tags one resource holds. `TagMap` is modelled `len ..50` — which bounds a
/// single request — and 50 is also the service's documented per-resource tag limit, so
/// `TagResource` enforces it on the ACCUMULATED map. That is both the fidelity rule and
/// what stops a stored tag map growing without end as a caller repeats the operation
/// with fresh keys.
pub const TAGS_PER_RESOURCE_LIMIT: usize = 50;

/// The tag-key prefix the service reserves: `TagResource$tags` documents "Keys must
/// not begin with aws:".
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// The requirement a `tags` member fails when it carries a reserved key, worded as
/// [`constraint_requirement`] words a modelled constraint, so a rule the service
/// enforces itself reads like one the model declares.
pub fn reserved_tag_key_requirement() -> String {
    format!("Member must not contain a key beginning with {RESERVED_TAG_KEY_PREFIX}")
}

/// The refusal the three ARN-addressed `/tags/{resourceArn}` operations share for a
/// `resourceArn` that resolves to no live resource: [`RESOURCE_NOT_FOUND_CODE`] at 404,
/// carrying [`RESOURCE_NOT_FOUND_MESSAGE`].
///
/// ONE composition for all three, so `TagResource`, `UntagResource` and
/// `ListTagsForResource` cannot drift apart, and generic over the operation's modelled
/// error enum so each returns it through its own error channel.
pub fn arn_not_found<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::not_found(RESOURCE_NOT_FOUND_MESSAGE)
}

/// The refusal every operation addressed by `/workspaces/{workspaceId}` shares for a
/// `workspaceId` that resolves to no workspace this account holds in the Region the
/// request was signed for: the modelled `ResourceNotFoundException` carrying
/// [`WORKSPACE_NOT_FOUND_MESSAGE`], the unresolved id in `resourceId` and the parent's
/// AWS type name ([`WORKSPACE_RESOURCE_TYPE`]) in `resourceType`.
///
/// ONE composition for all of them, for the reason [`arn_not_found`] is one for the three
/// ARN-addressed tagging operations: the `resourceType` reported, and what `resourceId`
/// names, are decided here once, so no two handlers can drift apart. It hands back the
/// modelled SHAPE rather than an operation's error channel, because each operation wraps
/// it in its own modelled error enum and answers it through its own channel.
pub fn workspace_not_found(workspace_id: &str) -> crate::ops::types::ResourceNotFoundException {
    workspace_not_found_worded(WORKSPACE_NOT_FOUND_MESSAGE, workspace_id)
}

/// [`workspace_not_found`] under an operation's OWN sentence, for the one operation whose
/// recorded wording differs — `DescribeWorkspace`, which answers
/// [`DESCRIBE_WORKSPACE_NOT_FOUND_MESSAGE`]. The two members that IDENTIFY the missing
/// workspace are still composed in one place, whichever sentence reports it.
pub fn workspace_not_found_worded(
    message: &str,
    workspace_id: &str,
) -> crate::ops::types::ResourceNotFoundException {
    crate::ops::types::ResourceNotFoundException {
        message: message.to_string(),
        resource_id: workspace_id.to_string(),
        resource_type: WORKSPACE_RESOURCE_TYPE.to_string(),
    }
}

/// How a `revisionId` that does not name the policy's current revision is refused: the
/// modelled `ValidationException` (400) carrying the service's own fixed sentence for the
/// failed precondition ([`stale_revision_message`]).
///
/// RECORDED BEHAVIOUR: `PutResourcePolicy` and `DeleteResourcePolicy` model BOTH
/// `ValidationException` (@httpError 400) and `ConflictException` (@httpError 409), and a
/// recorded real-aps put carrying a revision the policy is no longer at is refused
/// `ValidationException`. The two codes are not interchangeable to a caller: a 400
/// `ValidationException` says "the value you sent is wrong — re-read the revision and
/// send the fresh one", while a 409 `ConflictException` reads as a transient
/// resource-state conflict that SDK and hand-written retry wrappers replay unchanged. The
/// optimistic lock is a MEMBER the caller got wrong, so it is refused as one.
///
/// `ConflictException` stays what it is for: the resource being in a status that forbids
/// the call at all ("Update/Deletion is disallowed until Resource-based Policy is ACTIVE
/// and Workspace is ACTIVE"), which no value in the request can fix.
///
/// `action` is what the refused call was going to do to the policy — see
/// [`POLICY_UPDATE_ACTION`] and [`POLICY_DELETE_ACTION`].
pub fn stale_revision_rejection(action: &str) -> crate::ops::types::ValidationException {
    request_validation_failed(&stale_revision_message(action))
}

/// What a `PutResourcePolicy` holding the lock was going to do to the workspace's
/// resource policy, as [`stale_revision_message`] names it.
pub const POLICY_UPDATE_ACTION: &str = "update";
/// [`POLICY_UPDATE_ACTION`] for `DeleteResourcePolicy`.
pub const POLICY_DELETE_ACTION: &str = "delete";

/// The sentence the service answers when the optimistic lock on a workspace's resource
/// policy fails.
///
/// RECORDED BEHAVIOUR: a `PutResourcePolicy` naming a revision the policy is no longer at
/// is refused "Can't conditionally update a workspace resource policy with a wrong
/// revisionId." — fixed text with no variable parts: neither the revision the caller sent
/// nor the one the policy is at appears in it. The only part that moves is the ACTION the
/// refused call named, since the same lock guards the conditional delete; the sentence
/// names what the call was going to do, as it does for the put it was recorded on.
pub fn stale_revision_message(action: &str) -> String {
    format!("Can't conditionally {action} a workspace resource policy with a wrong revisionId.")
}

/// The reserved tag key a request carries, if any — the lowest one, so the refusal
/// names the same key however the request's map was iterated.
pub fn reserved_tag_key<'a, I: IntoIterator<Item = &'a String>>(keys: I) -> Option<&'a str> {
    keys.into_iter()
        .map(String::as_str)
        .filter(|key| key.starts_with(RESERVED_TAG_KEY_PREFIX))
        .min()
}

/// The label set limits to STORE for a request's `limitsPerLabelSet`.
///
/// `LabelSet` is a map on the wire, so each entry's labels are kept sorted; the list
/// itself keeps the request's order, which is the order a later describe reports.
pub fn store_label_set_limits(limits: &[crate::ops::types::LimitsPerLabelSet]) -> Vec<LabelSetLimit> {
    limits
        .iter()
        .map(|entry| LabelSetLimit {
            label_set: entry
                .label_set
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
            max_series: entry.limits.max_series,
        })
        .collect()
}

/// The stored label set limits as the `LimitsPerLabelSetList` the wire shapes carry.
/// Always a list, empty when no label set limit is enforced, so a reader sees `[]`
/// rather than an absent member — the same choice [`tag_map`] makes for `TagMap`.
pub fn label_set_limit_list(
    limits: &[LabelSetLimit],
) -> Vec<crate::ops::types::LimitsPerLabelSet> {
    limits
        .iter()
        .map(|entry| crate::ops::types::LimitsPerLabelSet {
            label_set: entry
                .label_set
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
            limits: crate::ops::types::LimitsPerLabelSetEntry {
                max_series: entry.max_series,
            },
        })
        .collect()
}

/// The stored configuration's status, as the modelled `LoggingConfigurationStatus` the
/// logging operations answer with — so every one of them reports it the same way.
pub fn logging_configuration_status(
    logging: &LoggingConfiguration,
) -> crate::ops::types::LoggingConfigurationStatus {
    crate::ops::types::LoggingConfigurationStatus {
        status_code: logging.status_code.clone(),
        status_reason: logging.status_reason.clone(),
    }
}

/// The stored tags as the `TagMap` the wire shapes carry. Always a map, empty when
/// the resource has no tags, so a reader sees `{}` rather than an absent member.
pub fn tag_map(tags: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    string_map(tags, false)
}

/// The stored metric labels as the `PrometheusMetricLabelMap` the wire shapes carry.
/// ABSENT when the resource carries no labels — the member is optional and a detector
/// created without labels has none, so there is nothing to report.
pub fn label_map(labels: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    string_map(labels, true)
}

/// One stored string map as the wire map shape, so [`tag_map`] and [`label_map`] share
/// ONE projection and differ only in `empty_is_absent`: whether an empty map is
/// reported as `{}` or left out of the response altogether.
fn string_map(
    map: &BTreeMap<String, String>,
    empty_is_absent: bool,
) -> Option<HashMap<String, String>> {
    if empty_is_absent && map.is_empty() {
        return None;
    }
    Some(
        map.iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )
}

/// One stored instant as the wire timestamp a response carries: the instant truncated to
/// whole MILLISECONDS.
///
/// Every modelled `Timestamp` member of this service targets `smithy.api#Timestamp` with
/// no `@timestampFormat` override, so restJson1 writes it as epoch seconds with a
/// fractional part — a JSON number. RECORDED BEHAVIOUR: the instants real aps publishes
/// carry at most millisecond resolution, so the clock's finer remainder is no part of the
/// value a caller receives; an `f64` holds today's epoch seconds to about a quarter of a
/// microsecond anyway, so a nanosecond remainder would not survive a client's round trip
/// of the number it was sent.
///
/// ONE derivation for every instant this service reports, so no two paths publish the same
/// stored instant at two different resolutions.
pub fn wire_instant(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let millis = match at.duration_since(UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_millis()).unwrap_or(i64::MAX),
        // An instant BEFORE the epoch, reported as the milliseconds it precedes the epoch
        // by: truncated towards the epoch, as the forward direction is truncated towards
        // it too.
        Err(before) => i64::try_from(before.duration().as_millis())
            .map_or(i64::MIN, |millis| -millis),
    };
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// The `statusReason` a status structure reports while NOTHING has failed: the empty
/// reason.
///
/// RECORDED BEHAVIOUR: the member is "The reason for the failure, if any" and the model
/// leaves open whether it is emitted when there has been no failure. Every recorded read
/// of a rule groups namespace or an alert manager definition carries `statusReason`
/// PRESENT and empty beside `statusCode`, so a client that reads the member
/// unconditionally — the member is modelled, and a deserializer that distinguishes absent
/// from empty branches on it — finds a value there rather than a missing key.
pub const NO_FAILURE_STATUS_REASON: &str = "";

/// The `statusReason` a read path publishes for a stored record: the reason the record
/// holds, or [`NO_FAILURE_STATUS_REASON`] while it holds none. Never absent, which is why
/// it answers a `String` and each caller does the one `Some` wrap the member needs.
///
/// ONE derivation, so a resource's description and its summary cannot report a stored
/// reason two different ways.
pub fn published_status_reason(stored: Option<&str>) -> String {
    stored.unwrap_or(NO_FAILURE_STATUS_REASON).to_string()
}

/// The operator an expression APPLIES outermost, when it leads with one: the identifier
/// the expression STARTS with, immediately followed by its argument list (`sum(x)`) or by
/// an aggregation modifier (`sum by (job) (x)`, `sum without (job) (x)`) — the form
/// PromQL's aggregation syntax takes, `<aggr-op> [by|without (<labels>)] (<expression>)`.
///
/// This is how [`RANDOM_CUT_FOREST_AGGREGATION_OPERATORS`] is checked: a Random Cut
/// Forest query must be WRAPPED by one of those aggregation operators, so the whole
/// expression has to BE a call to one. `None` when the expression does not lead with a
/// call at all (a bare selector, a parenthesised or arithmetic expression), which is
/// exactly the shape that is not wrapped.
pub fn leading_call_operator(expression: &str) -> Option<&str> {
    let trimmed = expression.trim_start();
    let end = trimmed
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(trimmed.len());
    let (identifier, rest) = trimmed.split_at(end);
    // An identifier starts with a letter or `_`, so a leading digit is a number and not
    // a call at all.
    if identifier.is_empty() || identifier.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let rest = rest.trim_start();
    let applies = rest.starts_with('(')
        || ["by", "without"].iter().any(|modifier| {
            rest.strip_prefix(modifier)
                .is_some_and(|tail| tail.starts_with('(') || tail.starts_with(char::is_whitespace))
        });
    applies.then_some(identifier)
}

/// Refuse an anomaly detector configuration the model accepts but the service does not.
///
/// `AnomalyDetectorConfiguration` is a union whose only member is `randomCutForest`, and
/// `RandomCutForestConfiguration$query` requires the query be "wrapped by a supported
/// PromQL aggregation operator" — so the whole expression must BE a call to one of
/// [`RANDOM_CUT_FOREST_AGGREGATION_OPERATORS`]. Shared, because the rule is the
/// configuration's own and every operation that stores one applies it.
///
/// The refusal is the service's own fixed sentence for this class
/// ([`RANDOM_CUT_FOREST_QUERY_UNSUPPORTED_MESSAGE`]): the expression is a well-formed
/// value of a member that satisfies every constraint the model declares, and what the
/// service rejects is the EXPRESSION it denotes — not a member constraint, so not the
/// member template.
pub fn check_random_cut_forest_configuration(
    configuration: &crate::ops::types::AnomalyDetectorConfiguration,
) -> Result<(), crate::ops::types::ValidationException> {
    let crate::ops::types::AnomalyDetectorConfiguration::randomCutForest(random_cut_forest) =
        configuration;
    let operators = RANDOM_CUT_FOREST_AGGREGATION_OPERATORS;
    let wrapping = leading_call_operator(&random_cut_forest.query);
    if wrapping.is_some_and(|operator| operators.contains(&operator)) {
        return Ok(());
    }
    Err(request_validation_failed(
        RANDOM_CUT_FOREST_QUERY_UNSUPPORTED_MESSAGE,
    ))
}

/// The query logging destinations to STORE for a request's `destinations` member: the
/// two members `LoggingDestination` carries, kept as plain data.
pub fn stored_query_logging_destinations(
    destinations: &[crate::ops::types::LoggingDestination],
) -> Vec<QueryLoggingDestination> {
    destinations
        .iter()
        .map(|destination| QueryLoggingDestination {
            log_group_arn: destination.cloud_watch_logs.log_group_arn.clone(),
            qsp_threshold: destination.filters.qsp_threshold,
        })
        .collect()
}

/// The `LoggingDestinations` a query logging reader reports, rebuilt from the STORED
/// record rather than from whatever the request carried.
pub fn wire_query_logging_destinations(
    destinations: &[QueryLoggingDestination],
) -> Vec<crate::ops::types::LoggingDestination> {
    destinations
        .iter()
        .map(|destination| crate::ops::types::LoggingDestination {
            cloud_watch_logs: crate::ops::types::CloudWatchLogDestination {
                log_group_arn: destination.log_group_arn.clone(),
            },
            filters: crate::ops::types::LoggingFilter {
                qsp_threshold: destination.qsp_threshold,
            },
        })
        .collect()
}

/// The `QueryLoggingConfigurationStatus` a stored configuration reports — the one
/// derivation `CreateQueryLoggingConfiguration`, `UpdateQueryLoggingConfiguration` and
/// `DescribeQueryLoggingConfiguration` all answer with.
pub fn query_logging_status(
    configuration: &QueryLoggingConfiguration,
) -> crate::ops::types::QueryLoggingConfigurationStatus {
    crate::ops::types::QueryLoggingConfigurationStatus {
        status_code: configuration.status_code.clone(),
        status_reason: configuration.status_reason.clone(),
    }
}

/// The `QueryLoggingConfigurationMetadata` a stored configuration reports.
pub fn query_logging_metadata(
    configuration: &QueryLoggingConfiguration,
) -> crate::ops::types::QueryLoggingConfigurationMetadata {
    crate::ops::types::QueryLoggingConfigurationMetadata {
        status: query_logging_status(configuration),
        workspace: configuration.workspace_id.clone(),
        destinations: wire_query_logging_destinations(&configuration.destinations),
        created_at: wire_instant(configuration.created_at),
        modified_at: wire_instant(configuration.modified_at),
    }
}

/// The `ScraperLoggingConfigurationStatus` a stored scraper logging configuration
/// reports — the one derivation every reader and writer of that configuration answers
/// with, exactly as [`query_logging_status`] is for a workspace's query logging
/// configuration.
pub fn scraper_logging_status(
    configuration: &ScraperLoggingConfiguration,
) -> crate::ops::types::ScraperLoggingConfigurationStatus {
    crate::ops::types::ScraperLoggingConfigurationStatus {
        status_code: configuration.status_code.clone(),
        status_reason: configuration.status_reason.clone(),
    }
}

// =============================================================================
// The media type this service answers in
// =============================================================================

/// The media type every aps response announces.
///
/// RECORDED BEHAVIOUR: the model declares `restJson1`, whose serializer stamps
/// [`JSON_CONTENT_TYPE`] on a JSON body, but every recorded real-aps response — a
/// success and a failure alike, on 200s, 400s and 404s — carries
/// `application/x-amz-json-1.1`, which is what a client content-negotiating or
/// protocol-asserting on the response reads. The body is the same JSON either way, so
/// nothing but the announced type changes. Applied in
/// [`crate::service::EmulatedService`], which is the one place this service's responses
/// pass through.
pub const RESPONSE_CONTENT_TYPE: &str = "application/x-amz-json-1.1";

/// The media type the `restJson1` serializer stamps on a JSON body — the one value
/// [`RESPONSE_CONTENT_TYPE`] replaces. A response whose body is NOT that JSON envelope
/// (a `@httpPayload` blob's `application/octet-stream`, a string payload's
/// `text/plain`) announces its own type and is left alone.
pub const JSON_CONTENT_TYPE: &str = "application/json";

// =============================================================================
// Validation: how this service states a refused request
// =============================================================================

/// The `reason` every aps `ValidationException` carries.
///
/// RECORDED BEHAVIOUR, like [`RESPONSE_CONTENT_TYPE`]: `ValidationException$reason` is
/// @required and targets `ValidationExceptionReason`, whose declared value set is
/// `UNKNOWN_OPERATION` / `CANNOT_PARSE` / `FIELD_VALIDATION_FAILED` / `OTHER`
/// (`crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES`) — but every recorded
/// real-aps 400, across four operations in four fixtures, reports `Validation error`,
/// and `reason` is the member a client branches on. The shape is a STRING with a
/// declared value set rather than a closed enum type — which is why the member is a
/// `String` here and why a value outside that set still deserializes — so answering the
/// value the service actually answers costs a client nothing.
///
/// One value for every refusal this service raises, whether a member is at fault or the
/// request is wrong as a whole: that is the one value observed, and a client cannot read
/// two vocabularies for one error shape. WHICH member is at fault is reported in
/// `fieldList`, which is absent exactly when no single member is.
pub const VALIDATION_REASON: &str = "Validation error";

/// `ValidationException`'s own documentation, which is what a refusal naming no member
/// can say about itself: "The input fails to satisfy the constraints specified by an
/// Amazon Web Services service."
const VALIDATION_EXCEPTION_DOC: &str =
    "The input fails to satisfy the constraints specified by an Amazon Web Services service.";

/// This service's modelled 400 for the members a request got wrong.
///
/// ONE composition, shared by the request boundary and by every handler that refuses a
/// rule the model cannot express, so a caller reads one form for both: `fieldList`
/// carries a `{name, message}` entry per refused member, `reason` is
/// [`VALIDATION_REASON`], and `message` names the first refused member and the
/// requirement it failed — `Invalid <member>: <requirement>`. The submitted VALUE is
/// never echoed: the caller named the member, and aps keeps its error text free of
/// caller input.
pub fn validation_failed(
    fields: Vec<crate::ops::types::ValidationExceptionField>,
) -> crate::ops::types::ValidationException {
    let message = fields.first().map_or_else(
        || VALIDATION_EXCEPTION_DOC.to_string(),
        |field| format!("Invalid {}: {}", field.name, field.message),
    );
    crate::ops::types::ValidationException {
        message,
        reason: VALIDATION_REASON.to_string(),
        field_list: Some(fields),
    }
}

/// [`validation_failed`] for a single refused member: the member's name as the wire
/// names it, and the requirement it failed.
pub fn member_validation_failed(
    name: &str,
    requirement: &str,
) -> crate::ops::types::ValidationException {
    validation_failed(vec![crate::ops::types::ValidationExceptionField {
        name: name.to_string(),
        message: requirement.to_string(),
    }])
}

/// This service's modelled 400 for a request that is wrong as a WHOLE rather than at one
/// member: there is no per-field breakdown to report, so `fieldList` is absent, and
/// `message` is one constant sentence carrying nothing from the request.
pub fn request_validation_failed(message: &str) -> crate::ops::types::ValidationException {
    crate::ops::types::ValidationException {
        message: message.to_string(),
        reason: VALIDATION_REASON.to_string(),
        field_list: None,
    }
}

/// [`validation_failed`] for the constraint violations the framework's own walk found at
/// the request boundary — one `fieldList` entry per refused MEMBER, in the order the
/// walk visits them, so a member that violates two constraints is reported once (by the
/// first requirement it failed) rather than twice.
pub fn constraint_validation_failed(
    violations: &[Violation],
) -> crate::ops::types::ValidationException {
    let mut fields: Vec<crate::ops::types::ValidationExceptionField> = Vec::new();
    for violation in violations {
        if fields.iter().any(|field| field.name == violation.path) {
            continue;
        }
        fields.push(crate::ops::types::ValidationExceptionField {
            name: violation.path.clone(),
            message: constraint_requirement(&violation.kind),
        });
    }
    validation_failed(fields)
}

/// The requirement a violated constraint states, as this service states it.
///
/// The wording is the framework's own vocabulary for each constraint class, with ONE
/// difference: a `@pattern` is stated [`unanchored`], because the anchors are how the
/// model says "the whole value", not part of what the caller has to match.
///
/// MIGRATION NOTE, as `services/common/src/validation.rs` asks of anything that inlines
/// these rules again: the requirement half of the sentence and the `@pattern`
/// anchor-stripping are the SAME for every service, and that module stages them for one
/// migration of its own (`grep -rln 'fn unanchored' services/*/src` lists the crates
/// carrying a copy — this crate is now one of them). What stays this service's own is the
/// DIALECT around it: `Invalid <member>: <requirement>` with one `fieldList` entry per
/// member ([`validation_failed`]). When that migration lands, these two functions are to
/// be deleted and called from the common module, not copied further.
pub fn constraint_requirement(kind: &ViolationKind) -> String {
    match kind {
        ViolationKind::MissingRequiredMember => "Member must not be null".to_string(),
        ViolationKind::MinLength(min) => {
            format!("Member must have length greater than or equal to {min}")
        }
        ViolationKind::MaxLength(max) => {
            format!("Member must have length less than or equal to {max}")
        }
        ViolationKind::MinValue(min) => {
            format!("Member must have value greater than or equal to {min}")
        }
        ViolationKind::MaxValue(max) => {
            format!("Member must have value less than or equal to {max}")
        }
        ViolationKind::Pattern(pattern) => format!(
            "Member must satisfy regular expression pattern: {}",
            unanchored(pattern)
        ),
        ViolationKind::EnumValue(allowed) => {
            format!("Member must satisfy enum value set: [{}]", allowed.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        // `ViolationKind` is `#[non_exhaustive]`: a constraint class the framework adds
        // later is still reported as a refused member rather than going unworded.
        _ => VALIDATION_EXCEPTION_DOC.to_string(),
    }
}

/// A `@pattern` as a message states it: without the `^` and `$` anchors.
///
/// Rendering only — never a decision about whether a value is acceptable, which is the
/// framework's constraint walk over the model's own pattern. A trailing `$` is kept when
/// it is escaped (`\$`, the literal dollar sign), since that one is a character the
/// pattern admits rather than an anchor.
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(head) if !head.ends_with('\\') => head,
        _ => body,
    }
}

// =============================================================================
// `kmsKeyArn`: the member whose modelled `@pattern` the published model over-states
// =============================================================================
//
// `CreateWorkspace$kmsKeyArn` targets `KmsKeyArn`
// (`smithy_query(query="shape", shape="KmsKeyArn")`): `@length(20..2048)` plus
// `@pattern(^arn:aws[-a-z]*:kms:[-a-z0-9]+:[0-9]{12}:key/[-a-f0-9]+$)`. That pattern's
// key-id segment is `[-a-f0-9]+` — lowercase hex and dashes — but the recorded live
// refusal for a malformed `kmsKeyArn` quotes the requirement the service ACTUALLY
// applies, `arn:aws[-a-z]*:kms:[-a-z0-9]+:[0-9]{12}:key/.+`, whose key-id segment is
// unconstrained. That message is built FROM the request (it names the member and the
// constraint the value failed), so the CONSTRAINT it reports is the service's own; under
// the published pattern this emulator refuses key ARNs AWS accepts — a multi-region
// `key/mrk-<hex>` CMK among them — and reports a requirement no aps response states.
//
// `CreateWorkspaceInput` therefore carries `skip_validation = ["kmsKeyArn"]` in
// `src/ops/types/create_workspace_types.rs`, and this module re-declares the member's
// constraints with the key-id segment as the service reads it. The published model in
// `smithy-model/` is left exactly as shipped.
//
// NOTHING about constraint checking is reimplemented here: the constraints are
// re-declared in the framework's own form (a [`TraitMap`] of
// [`LengthTrait`]/[`PatternTrait`]) hung on a member schema of the same name, and the
// check is [`validate`] — the same walk the request boundary runs — so a refusal made
// here reads exactly like one made there, down to the requirement
// [`constraint_requirement`] renders for it.

/// This service's Smithy namespace, for the member schema below.
const NAMESPACE: &str = "com.amazonaws.amp";

/// `KmsKeyArn`'s pattern with its key-id segment as the service applies and reports it:
/// `key/.+` rather than the published `key/[-a-f0-9]+`. See the section comment above for
/// where that reading comes from.
pub const KMS_KEY_ARN_PATTERN: &str = r"^arn:aws[-a-z]*:kms:[-a-z0-9]+:[0-9]{12}:key/.+$";

/// `kmsKeyArn`: the member's modelled `@length(20..2048)` plus [`KMS_KEY_ARN_PATTERN`].
/// The length bound is the model's as shipped — only the pattern is re-read.
static KMS_KEY_ARN_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(20), Some(2048))));
    traits.insert(Box::new(PatternTrait::new(KMS_KEY_ARN_PATTERN)));
    traits
});

/// The `kmsKeyArn` member schema the walk reads: its name gives the violation path — so
/// the refusal names `kmsKeyArn`, exactly as a boundary refusal would — and its trait map
/// carries the constraints.
static M_KMS_KEY_ARN: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.amp#CheckedMember$kmsKeyArn",
        NAMESPACE,
        "kmsKeyArn",
    ),
    ShapeType::String,
    "kmsKeyArn",
    0,
)
.with_traits(&KMS_KEY_ARN_TRAITS);

/// The root of the shadow input the walk is driven over. Its member list is empty on
/// purpose: the walk visits what [`CheckedString`] WRITES, and a root struct schema
/// carries no member name, so it contributes no path segment. The shape id is deliberately
/// one the model does not declare, so it cannot collide with a modelled shape.
static CHECKED_MEMBER_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "com.amazonaws.amp#CheckedMember",
        NAMESPACE,
        "CheckedMember",
    ),
    ShapeType::Structure,
    &[],
);

/// One string written for the validating walk the way the codec writes a string member,
/// against the member schema whose re-declared constraints the walk is to apply.
struct CheckedString<'a> {
    member: &'static Schema,
    value: &'a str,
}

impl SerializableStruct for CheckedString<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(self.member, self.value)
    }
}

/// Every constraint `member` declares that `value` violates, found by the framework's own
/// walk — so a check made here reads exactly like one made at the request boundary.
fn member_violations(member: &'static Schema, value: &str) -> Vec<Violation> {
    validate(&CheckedString { member, value }, &CHECKED_MEMBER_SCHEMA)
}

/// Refuse a `kmsKeyArn` that violates the member's modelled length bound or the pattern
/// the service applies, or pass it through.
///
/// The refusal is composed by [`constraint_validation_failed`] — the one composition the
/// request boundary and every other aps handler refuse through — so a caller reads the
/// same `reason`, the same single `fieldList` entry naming the member and the requirement
/// it failed, and the same `Invalid kmsKeyArn: <requirement>` message, whichever side made
/// the decision.
pub fn check_kms_key_arn(kms_key_arn: &str) -> Result<(), crate::ops::types::ValidationException> {
    let violations = member_violations(&M_KMS_KEY_ARN, kms_key_arn);
    if violations.is_empty() {
        return Ok(());
    }
    Err(constraint_validation_failed(&violations))
}

// =============================================================================
// Map KEYS: the constraints no request boundary applies
// =============================================================================
//
// The framework's constraint walk captures a map entry's key as the path segment for that
// entry's VALUE and never checks it against the key shape's own constraints
// (`vendor/smithy-schema-server/src/constraints.rs`, `write_leaf`). So every `@length` and
// `@pattern` a map's KEY shape declares has to be applied by the handler that accepts the
// map; nothing else in the request walk will. A key outside them is otherwise stored and
// reported back as persisted, which tells a caller its mislabelled resource succeeded.
//
// Each key shape is re-declared ONCE below as a [`MapKeyShape`], and every handler that
// accepts a map of that shape applies it through [`check_map_keys`], so the refusal reads
// like a boundary refusal: the member named, the requirement
// [`constraint_requirement`] words for the constraint at fault, and nothing from the
// request echoed.

/// One map KEY shape's modelled constraints, re-declared so the handler that accepts the
/// map can apply them.
pub struct MapKeyShape {
    /// The key shape's modelled `@length` minimum, in characters (how the framework's
    /// walk measures a string), or `None` when the shape declares none.
    min_length: Option<i64>,
    /// The key shape's modelled `@length` maximum, in characters, or `None`.
    max_length: Option<i64>,
    /// The key shape's modelled `@pattern`, exactly as the model declares it: this is what
    /// a refusal reports as the requirement, whatever form the decision below takes.
    pattern: &'static str,
    /// Whether a key satisfies `pattern`.
    satisfies: fn(&str) -> bool,
}

impl MapKeyShape {
    /// Which of the key shape's constraints `key` fails, if any — length before pattern,
    /// so a key failing both is reported once, by the bound it misses.
    fn violated_by(&self, key: &str) -> Option<ViolationKind> {
        let length = i64::try_from(key.chars().count()).unwrap_or(i64::MAX);
        if let Some(min) = self.min_length.filter(|min| length < *min) {
            return Some(ViolationKind::MinLength(min));
        }
        if let Some(max) = self.max_length.filter(|max| length > *max) {
            return Some(ViolationKind::MaxLength(max));
        }
        if !(self.satisfies)(key) {
            return Some(ViolationKind::Pattern(self.pattern.to_string()));
        }
        None
    }
}

/// Refuse a map whose KEYS violate the key shape's modelled constraints, or pass it
/// through.
///
/// The refusal names the MAP member (`labels`, `tags`, `limitsPerLabelSet`): a key is not
/// a member of its own, so there is no path below the map to report it at, and it is
/// composed by [`member_validation_failed`] like every other aps member refusal.
pub fn check_map_keys<'a, I: IntoIterator<Item = &'a String>>(
    member: &str,
    keys: I,
    key_shape: &MapKeyShape,
) -> Result<(), crate::ops::types::ValidationException> {
    // Sorted, so a map carrying two bad keys is refused the same way however the request's
    // map was iterated.
    let mut keys: Vec<&str> = keys.into_iter().map(String::as_str).collect();
    keys.sort_unstable();
    match keys
        .into_iter()
        .find_map(|key| key_shape.violated_by(key))
    {
        Some(kind) => Err(member_validation_failed(
            member,
            &constraint_requirement(&kind),
        )),
        None => Ok(()),
    }
}

/// `PrometheusMetricLabelKey`, the key of `PrometheusMetricLabelMap`
/// (`smithy_query(query="shape", shape="PrometheusMetricLabelKey")`): `@length(1..7168)`
/// and `@pattern(^(?!__)[a-zA-Z_][a-zA-Z0-9_]*$)` — a Prometheus label name that must not
/// begin with the reserved double-underscore prefix.
pub const PROMETHEUS_METRIC_LABEL_KEY: MapKeyShape = MapKeyShape {
    min_length: Some(1),
    max_length: Some(7168),
    pattern: r"^(?!__)[a-zA-Z_][a-zA-Z0-9_]*$",
    satisfies: is_prometheus_metric_label_key,
};

/// `LabelName`, the key of `LabelSet`
/// (`smithy_query(query="shape", shape="LabelName")`): `@length(min: 1)` and
/// `@pattern(^[a-zA-Z_][a-zA-Z0-9_]*$)`, the same Prometheus label name without the
/// reserved-prefix clause — `__name__` is a label a label set may constrain.
pub const LABEL_NAME: MapKeyShape = MapKeyShape {
    min_length: Some(1),
    max_length: None,
    pattern: r"^[a-zA-Z_][a-zA-Z0-9_]*$",
    satisfies: is_label_name,
};

/// `TagKey`, the key of `TagMap` (`smithy_query(query="shape", shape="TagKey")`):
/// `@length(1..128)` and [`TAG_KEY_PATTERN`].
pub const TAG_KEY: MapKeyShape = MapKeyShape {
    min_length: Some(1),
    max_length: Some(128),
    pattern: TAG_KEY_PATTERN,
    satisfies: satisfies_tag_key_pattern,
};

/// `TagKey`'s modelled `@pattern`, as the model declares it.
const TAG_KEY_PATTERN: &str = r"^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$";

/// `TAG_KEY_PATTERN` hung on a member schema, so the decision below is the framework's
/// own walk and the Unicode classes the pattern names mean there exactly what they mean at
/// the request boundary.
static TAG_KEY_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(PatternTrait::new(TAG_KEY_PATTERN)));
    traits
});

/// The member schema carrying [`TAG_KEY_TRAITS`]. Its name is never reported: the refusal
/// names the MAP member the key belongs to, which [`check_map_keys`] is given.
static M_TAG_KEY: Schema = Schema::new_member(
    ShapeId::from_static("com.amazonaws.amp#CheckedMember$tagKey", NAMESPACE, "tagKey"),
    ShapeType::String,
    "tagKey",
    0,
)
.with_traits(&TAG_KEY_TRAITS);

/// Whether `key` satisfies [`TAG_KEY_PATTERN`], decided by the framework's constraint
/// walk rather than by character predicates: the pattern names Unicode general categories,
/// which only a pattern engine reads the same way the boundary does.
fn satisfies_tag_key_pattern(key: &str) -> bool {
    member_violations(&M_TAG_KEY, key).is_empty()
}

/// Whether `key` satisfies `^[a-zA-Z_][a-zA-Z0-9_]*$` — two character classes, so it is
/// decided with character predicates rather than a pattern engine.
fn is_label_name(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Whether `key` satisfies `^(?!__)[a-zA-Z_][a-zA-Z0-9_]*$`: a label name whose first two
/// characters are not both underscores.
///
/// Decided here rather than by the framework's walk because that walk compiles a
/// `@pattern` with the `regex` crate, which rejects a negative lookahead and treats a
/// pattern it cannot compile as matching — so hanging this one on a member schema would
/// refuse nothing. The two clauses are each a character test.
fn is_prometheus_metric_label_key(key: &str) -> bool {
    !key.starts_with(RESERVED_LABEL_NAME_PREFIX) && is_label_name(key)
}

/// The label-name prefix `PrometheusMetricLabelKey`'s pattern excludes, and which
/// Prometheus reserves for its own label names ([`METRIC_NAME_LABEL`] is one of them).
pub const RESERVED_LABEL_NAME_PREFIX: &str = "__";

// =============================================================================
// Rule groups namespaces
// =============================================================================

/// The `RuleGroupsNamespaceStatus` a stored namespace reports.
///
/// ONE derivation for every path that publishes a namespace's status, so the describe, the
/// listing and any later reader cannot drift apart: the status code the record holds, and
/// the reason it holds ([`published_status_reason`], which reports the service's
/// no-failure reason while nothing has failed).
pub fn rule_groups_namespace_status(
    namespace: &RuleGroupsNamespace,
) -> crate::ops::types::RuleGroupsNamespaceStatus {
    crate::ops::types::RuleGroupsNamespaceStatus {
        status_code: namespace.status_code.clone(),
        status_reason: Some(published_status_reason(namespace.status_reason.as_deref())),
    }
}

/// The stored namespace as the `RuleGroupsNamespaceDescription` `DescribeRuleGroupsNamespace`
/// reports — every member read back off the persisted record, including the rules file it
/// holds, byte for byte as it was written.
pub fn rule_groups_namespace_description(
    namespace: &RuleGroupsNamespace,
) -> crate::ops::types::RuleGroupsNamespaceDescription {
    crate::ops::types::RuleGroupsNamespaceDescription {
        arn: namespace.arn.clone(),
        name: namespace.name.clone(),
        status: rule_groups_namespace_status(namespace),
        data: namespace.data.clone(),
        created_at: wire_instant(namespace.created_at),
        modified_at: wire_instant(namespace.modified_at),
        tags: tag_map(&namespace.tags),
    }
}

/// The stored namespace as the `RuleGroupsNamespaceSummary` `ListRuleGroupsNamespaces`
/// reports — the same persisted record [`rule_groups_namespace_description`] reports,
/// through the same status and the same instants, minus the rules file the summary shape
/// does not carry.
pub fn rule_groups_namespace_summary(
    namespace: &RuleGroupsNamespace,
) -> crate::ops::types::RuleGroupsNamespaceSummary {
    crate::ops::types::RuleGroupsNamespaceSummary {
        arn: namespace.arn.clone(),
        name: namespace.name.clone(),
        status: rule_groups_namespace_status(namespace),
        created_at: wire_instant(namespace.created_at),
        modified_at: wire_instant(namespace.modified_at),
        tags: tag_map(&namespace.tags),
    }
}

/// The bound BOTH operations that carry a rules file state in their own documentation:
/// "The combined length of a rule group namespace and a rule group name cannot exceed
/// 721 UTF-8 bytes."
///
/// It is an `<important>` callout on `CreateRuleGroupsNamespace` and on
/// `PutRuleGroupsNamespace` alike
/// (`smithy_query(query="doc",shape="CreateRuleGroupsNamespace")`), so it is a
/// constraint of the REQUEST those two carry a rules file in — the namespace name is one
/// member and the rule group names live in the `data` blob — and a request past it is
/// refused rather than stored.
pub const RULE_GROUP_NAME_COMBINED_MAX_UTF8_BYTES: usize = 721;

/// The mapping key a Prometheus rules file carries its rule groups under, and the key
/// each group carries its name under: the file is
/// `groups:\n  - name: <group>\n    rules: [...]`, the structure
/// `RuleGroupsNamespaceData` links to as "the rule groups namespace structure".
const RULE_GROUPS_KEY: &str = "groups";
/// See [`RULE_GROUPS_KEY`].
const RULE_GROUP_NAME_KEY: &str = "name";

/// How many parse events of a caller-supplied rules file are read while looking for its
/// rule group names. The reader is driven event by event off an iterative state machine,
/// so this bounds the WORK one request can ask of it (nothing recurses over the
/// document): a file longer than this is read as far as this bound and no further.
const MAX_RULES_FILE_EVENTS: usize = 1 << 20;

/// How deeply the reader follows a caller-supplied rules file. A rule group name sits
/// exactly three collections deep (`groups` -> item -> `name`), so a document that nests
/// further carries nothing this reader has to track.
const MAX_RULES_FILE_DEPTH: usize = 32;

/// Whether `data` declares a rule group whose name, combined with `namespace_name`,
/// exceeds [`RULE_GROUP_NAME_COMBINED_MAX_UTF8_BYTES`].
///
/// Both lengths are counted in UTF-8 BYTES, as the published bound states — a multibyte
/// name reaches it in fewer characters.
pub fn rule_group_name_bound_exceeded(namespace_name: &str, data: &[u8]) -> bool {
    rule_group_names(data).iter().any(|group| {
        namespace_name.len() + group.len() > RULE_GROUP_NAME_COMBINED_MAX_UTF8_BYTES
    })
}

/// The requirement a request past that bound failed, worded as the service documents the
/// bound itself — carrying nothing from the request, like every other aps refusal.
pub fn rule_group_name_bound_requirement() -> String {
    format!(
        "The combined length of a rule group namespace and a rule group name cannot \
         exceed {RULE_GROUP_NAME_COMBINED_MAX_UTF8_BYTES} UTF-8 bytes"
    )
}

/// One open collection of the rules file being read.
struct RulesFrame {
    /// A sequence keys its children by position, a mapping by name.
    sequence: bool,
    /// The mapping key this collection is the value of, if any.
    key: Option<String>,
    /// For a mapping, the key read but not yet matched with its value.
    pending_key: Option<String>,
}

/// The rule group names a rules file declares: the `name` of each item of the top-level
/// `groups` sequence of its FIRST YAML document, which is where a Prometheus rule groups
/// file carries them.
///
/// Read with `yaml-rust2` — the YAML 1.2 implementation the sibling apigateway,
/// cloudformation and greengrassv2 crates already read caller-supplied documents with —
/// driven EVENT BY EVENT rather than loaded into a tree, so nothing recurses over a
/// caller-supplied document and an alias (the one way a YAML body amplifies itself) is
/// not expanded.
///
/// A blob that is not UTF-8, not YAML, or carries no top-level `groups` sequence declares
/// no rule group name here: `RuleGroupsNamespaceData` is a blob, stored byte for byte
/// whatever it holds, so the bound is measured against the names this reader can actually
/// SEE and nothing else about the file is judged.
fn rule_group_names(data: &[u8]) -> Vec<String> {
    use ::yaml_rust2::parser::Event;

    let Ok(text) = ::std::str::from_utf8(data) else {
        return Vec::new();
    };
    let mut names: Vec<String> = Vec::new();
    let mut frames: Vec<RulesFrame> = Vec::new();
    let mut parser = ::yaml_rust2::parser::Parser::new_from_str(text);

    for _ in 0..MAX_RULES_FILE_EVENTS {
        // Not a YAML document (or no longer one): there is nothing further to measure.
        let Ok((event, _marker)) = parser.next_token() else {
            break;
        };
        match event {
            Event::Scalar(value, ..) => {
                // The first scalar of a mapping pair is the KEY, not a value.
                if let Some(frame) = frames.last_mut() {
                    if !frame.sequence && frame.pending_key.is_none() {
                        frame.pending_key = Some(value);
                        continue;
                    }
                }
                let key = take_pending_key(&mut frames);
                if key.as_deref() == Some(RULE_GROUP_NAME_KEY) && is_rule_group_item(&frames) {
                    names.push(value);
                }
            }
            Event::SequenceStart(..) | Event::MappingStart(..) => {
                let sequence = matches!(event, Event::SequenceStart(..));
                let key = take_pending_key(&mut frames);
                if frames.len() >= MAX_RULES_FILE_DEPTH {
                    break;
                }
                frames.push(RulesFrame {
                    sequence,
                    key,
                    pending_key: None,
                });
            }
            Event::SequenceEnd | Event::MappingEnd => {
                frames.pop();
            }
            // An alias copies an anchored node, which is how a YAML body amplifies
            // itself. Nothing here expands one, so the document is read no further.
            Event::Alias(_) => break,
            // A rules file is ONE document; a second one in the same body is not it.
            Event::DocumentEnd | Event::StreamEnd => break,
            _ => {}
        }
    }
    names
}

/// The mapping key the node about to be read is the value of, consumed so it speaks for
/// that node only: a sequence item and the document's root node have none.
fn take_pending_key(frames: &mut [RulesFrame]) -> Option<String> {
    frames.last_mut().and_then(|frame| frame.pending_key.take())
}

/// Whether the open collections are exactly `groups` -> one item: the mapping at the
/// document's root, the sequence its `groups` key carries, and one mapping item of that
/// sequence. A `name` scalar THERE is a rule group's name.
fn is_rule_group_item(frames: &[RulesFrame]) -> bool {
    let [root, groups, item] = frames else {
        return false;
    };
    !root.sequence
        && root.key.is_none()
        && groups.sequence
        && groups.key.as_deref() == Some(RULE_GROUPS_KEY)
        && !item.sequence
        && item.key.is_none()
}

// =============================================================================
// Scrapers
// =============================================================================

/// The stored scraper as the `ScraperSummary` `ListScrapers` reports — every member read
/// back off the persisted record.
pub fn scraper_summary(scraper: &Scraper) -> crate::ops::types::ScraperSummary {
    crate::ops::types::ScraperSummary {
        scraper_id: scraper.scraper_id.clone(),
        arn: scraper.arn.clone(),
        alias: scraper.alias.clone(),
        role_arn: scraper.role_arn.clone(),
        status: crate::ops::types::ScraperStatus {
            status_code: scraper.status_code.clone(),
        },
        status_reason: scraper.status_reason.clone(),
        created_at: wire_instant(scraper.created_at),
        last_modified_at: wire_instant(scraper.last_modified_at),
        tags: tag_map(&scraper.tags),
        source: scraper.source.clone(),
        destination: scraper.destination.clone(),
        role_configuration: scraper.role_configuration.clone(),
        exporters: Some(scraper.exporters.clone()),
    }
}

/// The stored scraper as the `ScraperDescription` `DescribeScraper` reports — the same
/// persisted record [`scraper_summary`] reports, plus the three members the description
/// carries and the summary does not (`scrapeConfiguration`, and `source` and
/// `destination` in their described form).
pub fn scraper_description(scraper: &Scraper) -> crate::ops::types::ScraperDescription {
    crate::ops::types::ScraperDescription {
        scraper_id: scraper.scraper_id.clone(),
        arn: scraper.arn.clone(),
        alias: scraper.alias.clone(),
        role_arn: scraper.role_arn.clone(),
        status: crate::ops::types::ScraperStatus {
            status_code: scraper.status_code.clone(),
        },
        status_reason: scraper.status_reason.clone(),
        created_at: wire_instant(scraper.created_at),
        last_modified_at: wire_instant(scraper.last_modified_at),
        tags: tag_map(&scraper.tags),
        scrape_configuration: scraper.scrape_configuration.clone(),
        source: scraper.source.clone(),
        destination: scraper.destination.clone(),
        role_configuration: scraper.role_configuration.clone(),
        exporters: Some(scraper.exporters.clone()),
    }
}

/// The ARN of the resource a scraper reads from, for the `sourceArn` filter.
///
/// `Source` is a union: `eksConfiguration` names the EKS cluster by ARN, and
/// `vpcConfiguration` models no ARN at all — it names security groups and subnets — so a
/// VPC source has no `sourceArn` to match and never satisfies that filter.
pub fn scraper_source_arn(source: &crate::ops::types::Source) -> Option<&str> {
    match source {
        crate::ops::types::Source::eksConfiguration(eks) => Some(eks.cluster_arn.as_str()),
        crate::ops::types::Source::vpcConfiguration(_) => None,
    }
}

/// The ARN of the resource a scraper writes to, for the `destinationArn` filter: the
/// workspace of an `ampConfiguration`, or the dataset of a `cloudWatchConfiguration`.
pub fn scraper_destination_arn(destination: &crate::ops::types::Destination) -> Option<&str> {
    match destination {
        crate::ops::types::Destination::ampConfiguration(amp) => {
            Some(amp.workspace_arn.as_str())
        }
        crate::ops::types::Destination::cloudWatchConfiguration(cloud_watch) => {
            Some(cloud_watch.dataset_arn.as_str())
        }
    }
}

/// The WORKSPACE ARN a destination names, when it names one.
///
/// `Destination` is a union: `ampConfiguration` sends the metrics to an Amazon Managed
/// Service for Prometheus workspace, named by ARN — a resource of THIS service, which this
/// store can resolve — while `cloudWatchConfiguration` names a CloudWatch dataset, a
/// resource of another service this store holds nothing about. That is the difference from
/// [`scraper_destination_arn`], which reports whichever ARN the destination carries because
/// the `destinationArn` filter matches on the stored value rather than resolving it.
pub fn destination_workspace_arn(destination: &crate::ops::types::Destination) -> Option<&str> {
    match destination {
        crate::ops::types::Destination::ampConfiguration(amp) => Some(amp.workspace_arn.as_str()),
        crate::ops::types::Destination::cloudWatchConfiguration(_) => None,
    }
}

/// Whether `scraper` satisfies every filter in `filters`.
///
/// `ListScrapersRequest$filters` documents the rule: "Filters on the same key are `OR`'d
/// together, and filters on different keys are `AND`'d together." The keys honoured are
/// the four the member documents ([`SCRAPER_FILTER_STATUS`] and friends); any other key
/// is not a filter this service defines and is ignored.
pub fn scraper_matches_filters(
    scraper: &Scraper,
    filters: &HashMap<String, Vec<String>>,
) -> bool {
    filters.iter().all(|(key, wanted)| {
        let held: Option<&str> = match key.as_str() {
            SCRAPER_FILTER_STATUS => Some(scraper.status_code.as_str()),
            SCRAPER_FILTER_ALIAS => scraper.alias.as_deref(),
            SCRAPER_FILTER_SOURCE_ARN => scraper_source_arn(&scraper.source),
            SCRAPER_FILTER_DESTINATION_ARN => scraper_destination_arn(&scraper.destination),
            // Not one of the documented filter keys: nothing to filter on.
            _ => return true,
        };
        held.is_some_and(|held| wanted.iter().any(|value| value == held))
    })
}

