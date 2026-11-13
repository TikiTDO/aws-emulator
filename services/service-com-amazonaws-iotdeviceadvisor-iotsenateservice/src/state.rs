// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iotdeviceadvisor-iotsenateservice service.
//!
//! IoT Core Device Advisor keeps two resources: a **suite definition** (a versioned test
//! suite configuration) and a **suite run** of one definition version. Both live here, in
//! the single per-service store behind the engine's state lock, so a value a create mints
//! is observable by every later read in the process.
//!
//! Besides the records this module owns the handful of rules every handler shares: the ARN
//! and endpoint shapes, the version-string sequence, and the clock-owed suite-run status
//! transitions ([`SuiteRun::materialize_due`]).

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::ops::types::{DeviceUnderTest, SuiteDefinitionConfiguration, SuiteRunConfiguration};

// WHERE an endpoint is ANNOUNCED — which of the addresses below a caller is actually
// handed, so the device it points at reaches something — lives in
// `state/endpoint_addressing.rs`.
pub mod endpoint_addressing;

pub use endpoint_addressing::announced_endpoint;

/// The region an ARN or endpoint falls back to when the request carries no SigV4 scope.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace, as the Smithy model's `aws.api#service` trait gives it
/// (`arnNamespace: iotdeviceadvisor`).
pub const ARN_NAMESPACE: &str = "iotdeviceadvisor";

/// How long a suite run spends in each non-terminal status before the clock owes it the
/// next one. A chosen span (AWS documents no duration for it): inside the 250–750 ms
/// window the emulator contract fixes, so a caller acting straight after `StartSuiteRun`
/// still observes `PENDING`, and never zero.
pub const STATUS_TRANSITION: Duration = Duration::from_millis(500);

/// Why a suite run cannot reach a test-result status in this emulator.
///
/// Device Advisor drives real MQTT test cases against a physical device over the IoT data
/// plane; this emulator has no device and no test runner, so a run is accepted, tracked,
/// and then ends in the modelled `ERROR` status carrying this reason — never in `PASS` /
/// `FAIL` / `PASS_WITH_WARNINGS`, which would report an outcome for tests nothing ran.
pub const NO_TEST_RUNNER_REASON: &str =
    "Device Advisor test execution is not available in this emulator: no device test runner \
     is attached, so the suite run could not be executed against the primary device.";

/// Suite-run statuses from which no further transition is owed.
///
/// The `SuiteRunStatus` values that are not `PENDING`, `RUNNING` or `STOPPING`; a run in
/// one of these keeps its status and its `endTime` forever. The single definition of
/// "terminal": [`SuiteRun::materialize_due`] consults it rather than listing the statuses
/// a second time.
pub const TERMINAL_STATUSES: &[&str] = &[
    "CANCELED",
    "ERROR",
    "FAIL",
    "PASS",
    "PASS_WITH_WARNINGS",
    "STOPPED",
];

/// True when `status` is one no clock-owed transition leaves.
pub fn is_terminal(status: &str) -> bool {
    TERMINAL_STATUSES.contains(&status)
}

/// The suite-run statuses that report a SUCCESSFUL run of the suite.
///
/// The `SuiteRunStatus` values a run reaches by executing its suite and passing it, as
/// against the ones that report it failing (`ERROR`, `FAIL`) or never finishing
/// (`CANCELED`, `STOPPED`, and the three non-terminal statuses). The single definition of
/// "successful", which is what decides whether a run has a qualification report
/// ([`ComAmazonawsIotdeviceadvisorIotsenateserviceState::qualification_report`]).
///
/// Nothing executes a suite in this emulator ([`NO_TEST_RUNNER_REASON`]), so no run of it
/// reaches one of these; the rule still belongs here rather than in the reader, so a run
/// that can succeed needs no second decision about what its success means.
pub const SUCCESSFUL_STATUSES: &[&str] = &["PASS", "PASS_WITH_WARNINGS"];

/// True when `status` reports a run that executed its suite and passed.
pub fn is_successful(status: &str) -> bool {
    SUCCESSFUL_STATUSES.contains(&status)
}

/// How many suite definitions this account's store holds at once.
///
/// An OCCUPANCY BOUND this emulator chooses, not a claim about a published AWS quota: the
/// store is in memory and every `CreateSuiteDefinition` adds a record, so without a cap a
/// caller could grow it without limit. `CreateSuiteDefinition` refuses past it with the
/// modelled `ValidationException` (the only client-input error it declares), the way a
/// real service refuses a creation past its account quota.
pub const MAX_SUITE_DEFINITIONS: usize = 100;

/// How many configuration version LABELS the store retains per suite definition.
///
/// The same occupancy bound as [`MAX_RUNS_PER_DEFINITION`], and again a RETENTION policy
/// rather than a refusal: every `UpdateSuiteDefinition` hands out one more version label,
/// so without a bound a caller could grow one definition's record without limit.
/// [`SuiteDefinition::record_version`] drops the LOWEST-ordinal labels once a definition
/// would hold more than this, so the most recent versions — the ones a read or a run names
/// — stay addressable, and an aged-out version reads as the modelled "no such version"
/// exactly as a version that never existed does.
pub const MAX_VERSIONS_PER_DEFINITION: usize = 50;

/// How many suite runs the store retains per definition.
///
/// The same occupancy bound for runs, as a RETENTION policy rather than a refusal: a
/// definition can be run any number of times, so `StartSuiteRun` drops the oldest runs of
/// that definition (and their tag rows) that are no longer in progress once it would
/// exceed this count, exactly as a real service ages out old run history. A run still in
/// progress is never dropped.
pub const MAX_RUNS_PER_DEFINITION: usize = 50;

/// How many runs of one definition may be in progress AT ONCE when they all ask to run in
/// parallel.
///
/// A run whose configuration does not ask for a parallel run holds its definition alone
/// (see [`SuiteRun::allows_parallel_runs`]), so without a cap only the parallel case is
/// unbounded: a caller could hold any number of in-progress runs of one definition open,
/// and retention may never drop a run that is in progress. An OCCUPANCY BOUND this
/// emulator chooses, not a claim about a published AWS quota, in the same spirit as
/// [`MAX_SUITE_DEFINITIONS`]: `StartSuiteRun` refuses past it with the modelled
/// concurrency error it already answers a non-parallel clash with.
pub const MAX_PARALLEL_RUNS_PER_DEFINITION: usize = 10;

/// How many tags one resource carries at once.
///
/// `TagMap` is `len 0..50` (`smithy_query(query="shape",shape="TagMap")`), so a resource
/// cannot end up holding more than 50 tags: `TagResource` refuses a merge that would leave
/// more than this with the modelled client-input error, the way a real service refuses a
/// tagging call past the per-resource tag limit.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The HTTP status a successful TAG MUTATION answers with.
///
/// Both tag mutations leave their success code defaulted in the model (`@http` on
/// `TagResource` and on `UntagResource` carries no `code:`), so Smithy's 200 default is
/// not a statement about this service: the success status is a behavioural fact, and the
/// recorded AWS responses are what state it. A tag mutation writes only the tag set of the
/// resource its `resourceArn` names and reports nothing back about it — `TagResourceResponse`
/// and `UntagResourceResponse` declare no members — so there is no entity for a caller to
/// parse and the answer is 204 No Content. A 200 there promises a body the response does not
/// carry, and every client path gated on the no-content status (conditional body parsing,
/// caches and proxies that key on it) takes the wrong branch. The two operations share the
/// `/tags/{resourceArn}` route and this one status, so the rule lives here rather than twice
/// in the handlers.
///
/// This does NOT extend to every operation whose output declares no members: `DeleteSuiteDefinition`
/// reports nothing either and keeps the defaulted 200 the recordings answer it with.
pub const TAG_MUTATION_SUCCESS_STATUS: u16 = 204;

/// The `protocol` a stored suite definition configuration carries when the caller named none.
///
/// `SuiteDefinitionConfiguration$protocol` is an optional `Protocol`
/// (`smithy_query(query="shape",shape="Protocol")`: `MqttV3_1_1`, `MqttV5`,
/// `MqttV3_1_1_OverWebSocket`, `MqttV5_OverWebSocket`), documented as "the MQTT protocol
/// that is configured in the suite definition" — a suite always runs over ONE of them, so
/// the service resolves the member when it stores the configuration rather than leaving the
/// suite with no protocol. The resolved value is plain MQTT 3.1.1, which is what the recorded
/// reads of a definition stored without a protocol answer with.
pub const DEFAULT_PROTOCOL: &str = "MqttV3_1_1";

/// The `rootGroup` a QUALIFICATION suite that named none is stored with.
///
/// The model's prose for `SuiteDefinitionConfiguration$rootGroup`: "If `rootGroup` is empty,
/// and `intendedForQualification` is set to true, all the qualification tests are included,
/// and the configuration is default." The empty string is therefore an INSTRUCTION to the
/// service and not the value it keeps: it stores the default qualification root group, so
/// every later read answers a non-empty one.
///
/// A Device Advisor root group is a document naming the test groups a run executes. This
/// emulator attaches no device test runner ([`NO_TEST_RUNNER_REASON`]) and so holds no
/// test-case catalog to enumerate; what it materialises is the single default qualification
/// group with the default configuration the prose names, in that document shape — non-empty,
/// and well inside `RootGroup`'s `len 0..2048`.
///
/// Its internal structure is EMULATOR-CHOSEN: what the service contract fixes is that a
/// qualification suite stores a non-empty root group and reads it back unchanged, not the
/// document's shape, so a later change that gains a real test-group catalog can replace this
/// value without breaking anything a caller may rely on.
pub const DEFAULT_QUALIFICATION_ROOT_GROUP: &str = concat!(
    r#"{"configuration":{},"tests":[{"name":"Device Advisor qualification tests","#,
    r#""configuration":{},"test":{"id":"qualification"}}]}"#
);

/// One of this service's two taggable resources, as a `resourceArn` names it.
///
/// `ListTagsForResource`/`TagResource`/`UntagResource` all document the same thing about
/// their `resourceArn`: "This can be SuiteDefinition ARN or SuiteRun ARN".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    /// `…:suitedefinition/<suiteDefinitionId>`, as `CreateSuiteDefinition` mints it.
    SuiteDefinition(String),
    /// `…:suiterun/<suiteDefinitionId>/<suiteRunId>`, as `StartSuiteRun` mints it.
    SuiteRun {
        suite_definition_id: String,
        suite_run_id: String,
    },
}

/// The six fields an ARN is written in:
/// `arn:<partition>:<service>:<region>:<account>:<resource>`.
///
/// `resource` is the TAIL of the value it was read from, so a `:` inside it stays in it.
struct ArnFields<'a> {
    partition: &'a str,
    service: &'a str,
    region: &'a str,
    account: &'a str,
    resource: &'a str,
}

/// Split `arn` into [`ArnFields`], or `None` when it does not open with `arn:` or does not
/// carry all six fields.
///
/// The ONE place this module splits an ARN, so the two readers of one — [`ArnScope::of`],
/// which keeps the scope fields, and [`parse_resource_arn`], which checks the resource
/// portion — cannot disagree about where an ARN's fields are. Neither emptiness nor the
/// service is judged here: each caller applies the rule it needs.
fn arn_fields(arn: &str) -> Option<ArnFields<'_>> {
    // `splitn(6, ..)` so a `:` inside the resource portion stays in it.
    let mut fields = arn.splitn(6, ':');
    let (
        Some("arn"),
        Some(partition),
        Some(service),
        Some(region),
        Some(account),
        Some(resource),
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return None;
    };
    Some(ArnFields {
        partition,
        service,
        region,
        account,
        resource,
    })
}

/// Parse `arn` as the ARN of a taggable IoT Device Advisor resource.
///
/// The tagging operations address a resource ONLY by ARN, and the model constrains
/// `AmazonResourceName` by length alone, so the ARN's shape is checked here:
/// `arn:<partition>:iotdeviceadvisor:<region>:<account>:` followed by one of the two
/// resource forms this service mints. `Err` carries why, for the modelled client-input
/// error — a value that is not one of this service's ARNs is a bad request, never a
/// missing resource, so it is refused before anything is looked up.
pub fn parse_resource_arn(arn: &str) -> Result<TaggedResource, String> {
    // The value is request-supplied and goes into the refusal text, so it is stated through
    // [`quoted_value`]: quoted, so no newline inside it can forge a second line of the
    // message, and bounded, so it cannot carry the message past what `Message` allows.
    let malformed = || {
        let shown = quoted_value(arn);
        bounded_message(format!(
            "resourceArn {shown} is not an IoT Device Advisor suite definition or suite run ARN"
        ))
    };
    let Some(fields) = arn_fields(arn) else {
        return Err(malformed());
    };
    if fields.partition.is_empty() || fields.account.is_empty() || fields.service != ARN_NAMESPACE
    {
        return Err(malformed());
    }
    let mut parts = fields.resource.split('/');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("suitedefinition"), Some(id), None, None) if !id.is_empty() => {
            Ok(TaggedResource::SuiteDefinition(id.to_string()))
        }
        (Some("suiterun"), Some(definition), Some(run), None)
            if !definition.is_empty() && !run.is_empty() =>
        {
            Ok(TaggedResource::SuiteRun {
                suite_definition_id: definition.to_string(),
                suite_run_id: run.to_string(),
            })
        }
        _ => Err(malformed()),
    }
}

/// The smallest step a write stamp advances by when the clock has not visibly moved.
///
/// Timestamps leave this service as epoch seconds in an f64, whose mantissa resolves an
/// epoch instant to about a microsecond: that is the finest difference a client can
/// observe, so it is the step [`SuiteDefinition::write_stamp`] uses to keep successive
/// write stamps strictly increasing.
const WRITE_STAMP_TICK: Duration = Duration::from_micros(1);

/// A stored suite definition: its identity, its ONE live configuration, the version labels
/// it has handed out, and its timestamps.
///
/// Device Advisor keeps exactly one live configuration per `suiteDefinitionId`: an update
/// hands out a new version label and REPLACES that configuration, so the versions are
/// pointers into the definition's history rather than snapshots of it. Hence a label SET
/// beside a single [`configuration`](Self::configuration), not a label -> configuration map.
#[derive(Debug, Clone)]
pub struct SuiteDefinition {
    /// The minted `suiteDefinitionId` (a UUID, so inside the modelled 12..36 length).
    pub id: String,
    /// `arn:aws:iotdeviceadvisor:<region>:<account>:suitedefinition/<id>`.
    pub arn: String,
    /// The version recorded last — the one an unqualified read echoes. Tracked explicitly
    /// rather than derived from [`versions`](Self::versions), whose members order
    /// lexicographically (`v10` before `v2`).
    pub latest_version: String,
    /// Every version label this definition has handed out and still accepts on a read: a
    /// pointer set, holding no configuration of its own.
    pub versions: BTreeSet<String>,
    /// The ONE live configuration, as the last create or update left it. Every read of
    /// this definition — version-qualified or not — answers with this one, since a
    /// definition keeps no per-version snapshots.
    pub configuration: SuiteDefinitionConfiguration,
    pub created_at: SystemTime,
    pub last_modified_at: SystemTime,
}

impl SuiteDefinition {
    /// True when `version` is a label this definition has handed out and still retains —
    /// the one check a version-qualified read or run makes before serving the live
    /// configuration. See [`MAX_VERSIONS_PER_DEFINITION`] for what "still retains" drops.
    pub fn holds_version(&self, version: &str) -> bool {
        self.versions.contains(version)
    }

    /// `suiteDefinitionName` as the STORED live configuration holds it.
    pub fn name(&self) -> &str {
        self.configuration.suite_definition_name.as_str()
    }

    /// Make `configuration` this definition's live one under a NEW version label, and hand
    /// that label back.
    ///
    /// The label set grows; the configuration is REPLACED, because a definition has exactly
    /// one live configuration and no per-version snapshots. The next ordinal is one past the
    /// highest this definition holds — not `versions.len()`, which would collide with a
    /// surviving label once retention has dropped an older one. BOTH timestamps move to the
    /// write instant: for the one live record `createdAt` behaves as a last-write stamp,
    /// just as `lastModifiedAt` does.
    /// The configuration is stored RESOLVED ([`resolved_configuration`]), exactly as a
    /// create stores the one it was given.
    pub fn record_version(
        &mut self,
        configuration: SuiteDefinitionConfiguration,
        now: SystemTime,
    ) -> String {
        let configuration = resolved_configuration(configuration);
        let highest = self
            .versions
            .iter()
            .filter_map(|label| version_ordinal(label))
            .max()
            .unwrap_or(0);
        let label = version_label(highest + 1);
        self.versions.insert(label.clone());
        self.latest_version = label.clone();
        self.configuration = configuration;
        let stamp = self.write_stamp(now);
        self.created_at = stamp;
        self.last_modified_at = stamp;
        self.prune_versions(MAX_VERSIONS_PER_DEFINITION);
        label
    }

    /// The instant to stamp a write with: `now`, never at or before the stamp this record
    /// already carries.
    ///
    /// A write stamp is what orders a definition's writes, so two writes must not read as
    /// one: when the clock has not visibly advanced since the last write (a coarse clock,
    /// or two calls inside the same tick) the stamp moves on by [`WRITE_STAMP_TICK`]
    /// instead of standing still.
    fn write_stamp(&self, now: SystemTime) -> SystemTime {
        let previous = self.last_modified_at.max(self.created_at);
        now.max(previous + WRITE_STAMP_TICK)
    }

    /// Retain at most `keep` version labels, dropping the lowest-ordinal (oldest) ones.
    ///
    /// See [`MAX_VERSIONS_PER_DEFINITION`]. The current latest version is never dropped:
    /// it carries the highest ordinal, and `keep` is at least one.
    fn prune_versions(&mut self, keep: usize) {
        let keep = keep.max(1);
        if self.versions.len() <= keep {
            return;
        }
        // Oldest first by ORDINAL, not by the labels' lexicographic order (`v10` < `v2`).
        let mut labels: Vec<(usize, String)> = self
            .versions
            .iter()
            .map(|label| (version_ordinal(label).unwrap_or(0), label.clone()))
            .collect();
        labels.sort();
        let drop_count = self.versions.len() - keep;
        for (_, label) in labels.into_iter().take(drop_count) {
            self.versions.remove(&label);
        }
    }
}

/// A stored suite run of one definition version.
#[derive(Debug, Clone)]
pub struct SuiteRun {
    /// The minted `suiteRunId` (a UUID).
    pub id: String,
    /// `arn:aws:iotdeviceadvisor:<region>:<account>:suiterun/<definition id>/<run id>`,
    /// minted in the scope the parent definition's own ARN records (see [`ArnScope`]).
    pub arn: String,
    pub suite_definition_id: String,
    pub suite_definition_version: String,
    /// The `suiteRunConfiguration` the start call carried, returned verbatim by reads.
    pub configuration: SuiteRunConfiguration,
    /// One of `SuiteRunStatus`.
    pub status: String,
    pub created_at: SystemTime,
    /// Set when the run leaves `PENDING`, by whichever path takes it out of the queue
    /// (the clock-owed start, or a caller's stop); absent only while it is still queued.
    pub started_at: Option<SystemTime>,
    /// Set only once the status is terminal.
    pub end_at: Option<SystemTime>,
    pub error_reason: Option<String>,
    /// The Device Advisor endpoint this run's primary device connects to, as
    /// [`endpoint_for`] derives it from that device.
    pub endpoint: String,
    /// The instant the next status transition becomes due, while one is owed.
    pub next_transition_at: Option<SystemTime>,
}

impl SuiteRun {
    /// Apply every status transition the clock has ALREADY made due at `now`.
    ///
    /// The whole lifecycle is clock-driven, so each reader materialises it before
    /// answering and nothing has to sleep: `PENDING` -> `RUNNING` -> `ERROR` for a run the
    /// emulator cannot execute (see [`NO_TEST_RUNNER_REASON`]), and `STOPPING` -> `STOPPED`
    /// for one a caller stopped. A transition that is not due yet is left alone, and a
    /// terminal run is never touched again.
    pub fn materialize_due(&mut self, now: SystemTime) {
        while let Some(due) = self.next_transition_at {
            if is_terminal(&self.status) {
                // Nothing is owed from a terminal status: drop any stale due instant.
                self.next_transition_at = None;
                return;
            }
            if due > now {
                return;
            }
            match self.status.as_str() {
                "PENDING" => {
                    self.status = "RUNNING".to_string();
                    self.leave_pending(due);
                    self.next_transition_at = Some(due + STATUS_TRANSITION);
                }
                "RUNNING" => {
                    self.status = "ERROR".to_string();
                    self.error_reason = Some(NO_TEST_RUNNER_REASON.to_string());
                    self.end_at = Some(due);
                    self.next_transition_at = None;
                }
                "STOPPING" => {
                    self.status = "STOPPED".to_string();
                    self.end_at = Some(due);
                    self.next_transition_at = None;
                }
                _ => {
                    // Unreachable: `TERMINAL_STATUSES` above plus these three arms cover
                    // every `SuiteRunStatus`. Clear the due instant rather than loop.
                    self.next_transition_at = None;
                    return;
                }
            }
        }
    }

    /// True while this run still occupies its definition: any status that is not terminal,
    /// so `STOPPING` still counts — a run the caller asked to stop is in progress until
    /// the stop completes.
    ///
    /// This is what `StartSuiteRun` weighs a concurrent start against, and what retention
    /// never drops.
    pub fn is_in_progress(&self) -> bool {
        !is_terminal(&self.status)
    }

    /// True when this run was started asking to run in parallel with others, so it does
    /// not hold its definition alone. See [`asks_for_parallel_run`].
    pub fn allows_parallel_runs(&self) -> bool {
        asks_for_parallel_run(&self.configuration)
    }

    /// True when a stop would move this run: it has not been asked to stop yet and has
    /// not finished. `STOPPING` and every terminal status answer false, so stopping twice
    /// neither restarts the stop clock nor resurrects a finished run.
    pub fn is_stoppable(&self) -> bool {
        matches!(self.status.as_str(), "PENDING" | "RUNNING")
    }

    /// Stamp `at` as the instant this run stopped being queued, unless it is stamped.
    ///
    /// The ONE writer of [`SuiteRun::started_at`], so the stamp belongs to having left
    /// `PENDING` rather than to one particular way of leaving it: every path out of the
    /// queued status goes through here. A run that already carries a stamp keeps it — a
    /// run starts once, and a later transition is not a second start.
    fn leave_pending(&mut self, at: SystemTime) {
        if self.started_at.is_none() {
            self.started_at = Some(at);
        }
    }

    /// Ask this run to stop, and report whether that moved it.
    ///
    /// A stoppable run goes to `STOPPING`, from which the clock owes it `STOPPED` with an
    /// `endTime`; because it has left `PENDING` it also carries its start stamp from here
    /// on, so the end instant it reaches is reported against a start instant rather than
    /// against nothing. A run that is already stopping or has finished is left exactly as
    /// it is (see [`SuiteRun::is_stoppable`]), keeping both the stamp and the status it
    /// already reached.
    pub fn begin_stop(&mut self, now: SystemTime) -> bool {
        if !self.is_stoppable() {
            return false;
        }
        self.leave_pending(now);
        self.status = "STOPPING".to_string();
        self.next_transition_at = Some(now + STATUS_TRANSITION);
        true
    }
}

/// Per-service state for com-amazonaws-iotdeviceadvisor-iotsenateservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsIotdeviceadvisorIotsenateserviceState {
    /// suiteDefinitionId -> definition. A `BTreeMap` so every listing, and the paging
    /// token built from it, has one deterministic order.
    pub definitions: BTreeMap<String, SuiteDefinition>,
    /// suiteRunId -> run.
    pub runs: BTreeMap<String, SuiteRun>,
    /// resourceArn -> tags. The ONE place tags live: a definition's and a run's tags are
    /// both keyed by their ARN here, so the tagging operations and the reads that echo
    /// tags cannot disagree.
    pub tags: BTreeMap<String, BTreeMap<String, String>>,
    /// `clientToken` -> what that token already created, so a retried
    /// `CreateSuiteDefinition` returns the first definition instead of minting a second,
    /// and the same token offered for a DIFFERENT request is refused. One entry per
    /// stored definition at most — a token is recorded only alongside a new definition
    /// and dropped with it — so [`MAX_SUITE_DEFINITIONS`] bounds this map too.
    pub create_definition_tokens: BTreeMap<String, AcceptedCreateRequest>,
}

/// What one `clientToken` was accepted for: the definition it created, and a fingerprint
/// of the request that created it.
///
/// An `@idempotencyToken` identifies a RETRY of one request, not "whatever the caller
/// sends next under the same name": a token offered again with different parameters is a
/// client mistake, not a retry, so the fingerprint is kept to tell the two apart.
#[derive(Debug, Clone)]
pub struct AcceptedCreateRequest {
    pub suite_definition_id: String,
    /// See [`create_request_fingerprint`].
    pub fingerprint: String,
}

/// A fingerprint of a `CreateSuiteDefinition` request: everything the stored definition is
/// built from — the configuration, the tags, and the region the ARN is minted in.
///
/// Derived member by member from the already-normalised values (the tags as a sorted
/// map), each written as a length-prefixed field so no value can impersonate another
/// through a separator, and so the fingerprint is defined by the DATA rather than by a
/// formatter whose output is not a stability contract.
pub fn create_request_fingerprint(
    region: &str,
    configuration: &SuiteDefinitionConfiguration,
    tags: &BTreeMap<String, String>,
) -> String {
    /// One length-prefixed field: `<len>:<value>;`.
    fn field(value: &str, into: &mut String) {
        use ::std::fmt::Write;
        // `write!` into the buffer we already own: no per-field allocation.
        let _ = write!(into, "{}:{value};", value.len());
    }

    let mut out = String::new();
    field(region, &mut out);
    field(&configuration.suite_definition_name, &mut out);
    field(&configuration.root_group, &mut out);
    field(&configuration.device_permission_role_arn, &mut out);
    field(configuration.protocol.as_deref().unwrap_or(""), &mut out);
    field(
        match configuration.intended_for_qualification {
            Some(true) => "qualification=true",
            Some(false) => "qualification=false",
            None => "qualification=unset",
        },
        &mut out,
    );
    field(
        match configuration.is_long_duration_test {
            Some(true) => "longDuration=true",
            Some(false) => "longDuration=false",
            None => "longDuration=unset",
        },
        &mut out,
    );
    let devices = configuration.devices.as_deref().unwrap_or_default();
    field(&devices.len().to_string(), &mut out);
    for device in devices {
        field(device.thing_arn.as_deref().unwrap_or(""), &mut out);
        field(device.certificate_arn.as_deref().unwrap_or(""), &mut out);
        field(device.device_role_arn.as_deref().unwrap_or(""), &mut out);
    }
    field(&tags.len().to_string(), &mut out);
    for (key, value) in tags {
        field(key, &mut out);
        field(value, &mut out);
    }
    out
}

impl ComAmazonawsIotdeviceadvisorIotsenateserviceState {
    /// Materialise every clock-owed run transition, then hand back the run named by
    /// `(definition id, run id)`.
    ///
    /// A read goes through here: it writes only what the clock already made due at `now`
    /// — the same state a caller would have seen had nobody called at all — and only to
    /// the run the caller named. A run of ANOTHER definition is matched out first, so no
    /// operation ever touches a resource it was not asked about.
    pub fn run_mut(
        &mut self,
        suite_definition_id: &str,
        suite_run_id: &str,
        now: SystemTime,
    ) -> Option<&mut SuiteRun> {
        let run = self.runs.get_mut(suite_run_id)?;
        if run.suite_definition_id != suite_definition_id {
            return None;
        }
        run.materialize_due(now);
        Some(run)
    }

    /// Every run OF ONE DEFINITION, with its clock-owed transitions materialised.
    ///
    /// The definition filter runs before the materialisation, so a call about one
    /// definition never advances another definition's runs.
    pub fn runs_of_mut(
        &mut self,
        suite_definition_id: &str,
        now: SystemTime,
    ) -> impl Iterator<Item = &mut SuiteRun> {
        let suite_definition_id = suite_definition_id.to_string();
        self.runs.values_mut().filter_map(move |run| {
            if run.suite_definition_id != suite_definition_id {
                return None;
            }
            run.materialize_due(now);
            Some(run)
        })
    }

    /// Remove `keys` from the tags recorded for `arn` and report how many the call
    /// actually removed.
    ///
    /// The counterpart of [`merge_tags`](Self::merge_tags). A key the resource does not
    /// carry is simply absent afterwards too, so removing it is not an error and needs no
    /// refusal. An ARN left with no tags keeps NO row at all, exactly as `merge_tags`
    /// keeps none for an empty merge: [`tags_of`](Self::tags_of) must not be able to tell
    /// "untagged down to nothing" from "never tagged".
    pub fn remove_tags<'a>(&mut self, arn: &str, keys: impl IntoIterator<Item = &'a str>) -> usize {
        let Some(tags) = self.tags.get_mut(arn) else {
            return 0;
        };
        let removed = keys
            .into_iter()
            .filter(|key| tags.remove(*key).is_some())
            .count();
        if tags.is_empty() {
            self.tags.remove(arn);
        }
        removed
    }

    /// The tags recorded for `arn` — an EMPTY MAP when it carries none.
    ///
    /// `TagMap` is `len 0..50`, so "no tags" is a value the member can hold: every read
    /// that reports a resource's tags serves that empty map rather than dropping the
    /// member, so a caller never has to tell `null` from `{}` to learn a resource is
    /// untagged. Whether a row exists at all is an artefact of how this store prunes empty
    /// rows ([`remove_tags`](Self::remove_tags)), and is not observable.
    pub fn tags_of(&self, arn: &str) -> std::collections::HashMap<String, String> {
        self.tags
            .get(arn)
            .into_iter()
            .flatten()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// True when this store holds the resource `resource` names, UNDER THE ARN it was
    /// named by.
    ///
    /// The ARN identifies the resource whole: an ARN carrying the right id but another
    /// partition, region or account names a resource of that other scope, which this
    /// store does not hold, so it is not found rather than silently treated as the local
    /// one. A pure predicate — a tag read never advances anything.
    pub fn holds_tagged_resource(&self, arn: &str, resource: &TaggedResource) -> bool {
        match resource {
            TaggedResource::SuiteDefinition(id) => self
                .definitions
                .get(id)
                .is_some_and(|definition| definition.arn == arn),
            TaggedResource::SuiteRun {
                suite_definition_id,
                suite_run_id,
            } => self.runs.get(suite_run_id).is_some_and(|run| {
                run.arn == arn && run.suite_definition_id == *suite_definition_id
            }),
        }
    }

    /// Add `tags` to `arn`'s row, overwriting the value of every key it already carries
    /// and leaving the rest in place, and report how many keys the call actually WROTE.
    ///
    /// `Ok` counts the named keys the row did not carry with that value already — the ones
    /// this call created or whose value it replaced — so `Ok(0)` is a merge that left the
    /// stored tags exactly as they were, whether because it named no key at all (`TagMap`
    /// is `len 0..50`, so an empty map is a legal request) or because every key it named
    /// already carried the value it asked for. The counterpart of
    /// [`remove_tags`](Self::remove_tags), which reports the same way, and what lets a
    /// caller's audit record state the no-op rather than claim a change.
    ///
    /// `Err` is how many tags the merge would have left, when that is past `limit`: the
    /// caller refuses with its own modelled error and the row is left untouched, so a
    /// refused tagging call changes nothing. An empty row is dropped rather than kept, so
    /// a resource with no tags has no row in either direction.
    pub fn merge_tags(
        &mut self,
        arn: &str,
        tags: BTreeMap<String, String>,
        limit: usize,
    ) -> Result<usize, usize> {
        let existing = self.tags.get(arn);
        let would_hold = existing.map_or(tags.len(), |existing| {
            existing.len() + tags.keys().filter(|key| !existing.contains_key(*key)).count()
        });
        if would_hold > limit {
            return Err(would_hold);
        }
        let written = tags
            .iter()
            .filter(|(key, value)| {
                existing.is_none_or(|existing| existing.get(key.as_str()) != Some(*value))
            })
            .count();
        if written == 0 {
            // Nothing to write: the row — including the absence of one — is left as it is,
            // so an empty merge onto an untagged resource creates no row for it.
            return Ok(0);
        }
        let row = self.tags.entry(arn.to_string()).or_default();
        row.extend(tags);
        Ok(written)
    }

    /// Retain at most `keep` runs of `suite_definition_id`, dropping the OLDEST ones that
    /// have FINISHED, with their tag rows.
    ///
    /// The store's occupancy bound for runs (see [`MAX_RUNS_PER_DEFINITION`]). Liveness is
    /// judged on the CURRENT status, so the clock-owed transitions are materialised first:
    /// the lifecycle only advances when something touches the run, so a run nobody has
    /// read still carries the status it was stored with and would otherwise look in
    /// progress forever — leaving the bound with nothing it is allowed to drop.
    ///
    /// A run that is still in progress — including one that is `STOPPING` — is never
    /// dropped, so a caller can always read back the run it just started or stopped. That
    /// cannot defeat the bound: the number of runs of one definition that can be in
    /// progress at once is itself capped ([`MAX_PARALLEL_RUNS_PER_DEFINITION`]), so at
    /// most that many runs are ever skipped and the remaining candidates take the count
    /// down to `keep`.
    pub fn prune_runs(&mut self, suite_definition_id: &str, keep: usize, now: SystemTime) {
        let mut total = 0usize;
        let mut droppable: Vec<(SystemTime, String, String)> = Vec::new();
        for run in self.runs.values_mut() {
            if run.suite_definition_id != suite_definition_id {
                continue;
            }
            run.materialize_due(now);
            total += 1;
            if !run.is_in_progress() {
                droppable.push((run.created_at, run.id.clone(), run.arn.clone()));
            }
        }
        if total <= keep {
            return;
        }
        // Oldest first: retention keeps the most recent runs.
        droppable.sort();
        for (_, id, arn) in droppable.into_iter().take(total - keep) {
            self.runs.remove(&id);
            self.tags.remove(&arn);
        }
    }

    /// The QUALIFICATION REPORT of the suite run `(suite_definition_id, suite_run_id)`
    /// names, when that run has one: its definition and the run itself.
    ///
    /// The ONE rule for whether a report exists, and the only place it is stated. A
    /// qualification report is the report of a SUCCESSFUL qualifying test suite run, so
    /// all three of these hold for a run that has one:
    ///
    /// * the run belongs to the definition the caller named, since crossing one
    ///   definition's id with another's run names no report, just as it names no run;
    /// * the definition it ran is INTENDED FOR QUALIFICATION, since the report is the
    ///   qualification report of a qualifying suite; and
    /// * the run SUCCEEDED ([`is_successful`]), so a run still in progress, one a caller
    ///   stopped, and one that failed or errored have none.
    ///
    /// No run of THIS emulator reaches a successful status: nothing executes the suite
    /// (see [`NO_TEST_RUNNER_REASON`]), so a run ends in `ERROR`, and the operation that
    /// reads this therefore answers its modelled not-found refusal for every run the
    /// emulator can produce. The rule is the SERVICE's, not the lifecycle's: a report
    /// appears the moment a run can succeed, with nothing here to change.
    ///
    /// A pure read: the clock-owed transitions are NOT materialised here, so a caller that
    /// wants the report of a run whose next transition has just become due materialises it
    /// first ([`Self::run_mut`]) and then asks.
    pub fn qualification_report(
        &self,
        suite_definition_id: &str,
        suite_run_id: &str,
    ) -> Option<(&SuiteDefinition, &SuiteRun)> {
        let run = self.runs.get(suite_run_id)?;
        if run.suite_definition_id != suite_definition_id {
            return None;
        }
        let definition = self.definitions.get(suite_definition_id)?;
        if definition.configuration.intended_for_qualification != Some(true) {
            return None;
        }
        is_successful(&run.status).then_some((definition, run))
    }
}

/// The longest `message` any error this service answers with may carry.
///
/// Every error shape in this service's model holds its text in a `message` member targeting
/// `Message`, which is `len 1..2048` (`smithy_query(query="shape",shape="Message")`), so a
/// longer one is a body the model says cannot exist. Nothing downstream checks it: the
/// framework validates a request's INPUT against the model and serializes an error's members
/// as they are given, so the bound holds where the message is BUILT — here and in
/// [`bounded_message`], which every refusal text in this crate goes through.
pub const MAX_MESSAGE_LENGTH: usize = 2048;

/// How many characters of a request-supplied value a refusal states.
///
/// A refusal names the value it refused so the caller can see what was wrong with it, which a
/// leading run of characters serves; every ARN-typed member here is itself as long as a whole
/// message may be (`AmazonResourceName` is `len 20..2048`), so stating one WHOLE would leave
/// no room for the sentence around it. The ceiling is well past the longest ARN this service
/// mints — a suite run's, whose resource portion carries two ids — so a value of a form this
/// service itself produces is always stated in full, and only a longer one is cut.
const MAX_ECHOED_VALUE: usize = 256;

/// What marks a value a refusal stated only the leading characters of.
const ELISION: &str = "...";

/// `value` as a refusal message states it PLAINLY: itself when short, its leading
/// [`MAX_ECHOED_VALUE`] characters followed by [`ELISION`] when longer.
///
/// Rendering only — every caller keeps the ORIGINAL value for its own checks and reaches here
/// solely to interpolate, because a value cut down before a check is not the value the caller
/// sent. The cut falls between characters, so a multi-byte character is never split.
pub fn echoed_value(value: &str) -> String {
    let mut shown: String = value.chars().take(MAX_ECHOED_VALUE).collect();
    if shown.len() < value.len() {
        shown.push_str(ELISION);
    }
    shown
}

/// `value` as a refusal message states it QUOTED: the same bounded rendering as
/// [`echoed_value`], wrapped in quotes with anything unprintable inside it escaped, so a
/// value carrying a newline cannot forge a second line of the message.
///
/// The bound is on the QUOTED rendering rather than on the value: an escape stands for one
/// character with several, so a value of [`MAX_ECHOED_VALUE`] characters can quote to many
/// times that, and it is what goes on the wire that has to fit. A character is therefore
/// stated whole or not at all — never cut in the middle of its escape — and [`ELISION`] goes
/// INSIDE the quotes, so a cut rendering cannot read as a value that ended there.
pub fn quoted_value(value: &str) -> String {
    let mut shown = String::with_capacity(MAX_ECHOED_VALUE);
    shown.push('"');
    let mut room = MAX_ECHOED_VALUE;
    let mut cut = false;
    for character in value.chars() {
        let escaped: String = character.escape_debug().collect();
        let taken = escaped.chars().count();
        if taken > room {
            cut = true;
            break;
        }
        room -= taken;
        shown.push_str(&escaped);
    }
    if cut {
        shown.push_str(ELISION);
    }
    shown.push('"');
    shown
}

/// `message` as it goes on the wire: itself when it already fits [`MAX_MESSAGE_LENGTH`]
/// characters — the maximum the member every error of this service carries its text in
/// declares — else its longest prefix that leaves room for [`ELISION`], cut between
/// characters, followed by that marker.
///
/// The ceiling on a stated value is what keeps a refusal both readable and WHOLE — every
/// sentence this crate builds from one is far inside the bound, so no fixed text is ever
/// lost. This is where the bound itself holds, for any message however its parts render, and
/// the marker is what keeps a message that did have to be cut from reading as a complete
/// sentence that ended mid-word.
pub fn bounded_message(message: String) -> String {
    if message.chars().count() <= MAX_MESSAGE_LENGTH {
        return message;
    }
    let kept = MAX_MESSAGE_LENGTH - ELISION.chars().count();
    let mut bounded: String = message.chars().take(kept).collect();
    bounded.push_str(ELISION);
    bounded
}

/// The message a refusal carries when the resource the request NAMED does not exist.
///
/// Device Advisor builds this text from the request: the template is
/// `Unable to find resource with <member>: <value>`, where `<member>` is the input member
/// that identified the missing resource and `<value>` is the value the caller sent. One
/// definition for the whole service, so every lookup that cannot find what it was asked
/// for words it the same way, whichever error shape the operation models for it.
pub fn not_found_message(member: &str, value: &str) -> String {
    bounded_message(format!(
        "Unable to find resource with {member}: {}",
        echoed_value(value)
    ))
}

/// The message a `SuiteDefinitionConfiguration` with an empty `rootGroup` is refused with
/// when the suite is not intended for qualification.
///
/// Device Advisor builds it from the violated rule, with its `Invalid Request: ` prefix;
/// see [`root_group_violation`] for the rule itself.
pub const EMPTY_ROOT_GROUP_MESSAGE: &str =
    "Invalid Request: rootGroup cannot be empty for non-qualification test suite";

/// The upper bound the `MaxResults` shape's `range` trait puts on `maxResults`
/// (`range 1..50`), which is also the page size a caller who names none gets: one page
/// then holds everything unless the caller asks for fewer.
pub const MAX_RESULTS_MAX: i32 = 50;

/// The message a listing refuses a `nextToken` it cannot have issued with.
///
/// One definition for the whole service, so every listing words that refusal the same way,
/// whichever error shape the operation models for it.
pub const INVALID_PAGE_TOKEN_MESSAGE: &str = "Invalid nextToken";

/// Where one listing call starts, and how many entries the page it answers may carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRequest {
    /// The resource id the page resumes AT — one the listing holds — or `None` to start
    /// from the beginning of the listing's order.
    pub resume_at: Option<String>,
    /// The greatest number of entries the page may carry. Never zero, so a paging loop
    /// always makes progress.
    pub page_size: usize,
}

/// Decide the [`PageRequest`] the paging members of a listing request ask for.
///
/// The paging rule belongs to the SERVICE, not to one listing, so every paginated
/// operation here asks this rather than restating it: an absent or empty `nextToken` starts
/// from the beginning (`Token` is `len 0..2000`, and an empty token carries no position, so
/// it is the same request as naming none), any other token must name a position in the very
/// set this call is listing, and an absent `maxResults` means one page holds everything, so
/// it defaults to [`MAX_RESULTS_MAX`].
///
/// `positions` is that set: the ids of the entries this call's own filters select, in the
/// listing's order — exactly the values this listing hands a `nextToken` out for. The token
/// is checked AGAINST them rather than against the shape of an id, because the set of tokens
/// a listing can issue is the set of positions it holds: a value of the right shape that
/// names none of them is one no page of this listing ever ended before, so using it as the
/// page's lower bound would answer a short, a truncated or an empty listing — a loss a
/// caller cannot tell from "this is the last page" — instead of refusing the token. That is
/// what refuses a forged, stale, hand-edited or cross-listing token (an id the SIBLING
/// listing hands out, or one belonging to a differently filtered set, both of the same
/// shape) rather than silently dropping records below it.
///
/// `Err` carries the message for the refusal — the caller lifts it into the modelled
/// client-input error its own operation declares, which is how one rule serves listings
/// with different error enums.
pub fn page_request<'a>(
    next_token: Option<&str>,
    max_results: Option<i32>,
    positions: impl IntoIterator<Item = &'a str>,
) -> Result<PageRequest, String> {
    let resume_at = match next_token {
        None | Some("") => None,
        Some(token) => {
            if !positions.into_iter().any(|position| position == token) {
                return Err(INVALID_PAGE_TOKEN_MESSAGE.to_string());
            }
            Some(token.to_string())
        }
    };

    // The boundary validator has already refused a `maxResults` outside `range 1..50`;
    // `max(1)` only keeps a paging loop terminating if it ever stops doing so.
    let page_size = usize::try_from(max_results.unwrap_or(MAX_RESULTS_MAX))
        .unwrap_or(1)
        .max(1);

    Ok(PageRequest {
        resume_at,
        page_size,
    })
}

/// The one rule the model states in prose for `SuiteDefinitionConfiguration$rootGroup`
/// and cannot express as a constraint: "if intendedForQualification is set to true,
/// rootGroup can be an empty string. If intendedForQualification is false, rootGroup
/// cannot be an empty string."
///
/// `Some(message)` names the violation for the modelled `ValidationException` with the
/// template AWS answers it with ([`EMPTY_ROOT_GROUP_MESSAGE`]); `None` means the
/// configuration satisfies the rule. The rule belongs to the SHAPE, so every operation
/// that accepts a `SuiteDefinitionConfiguration` asks here rather than restating it.
pub fn root_group_violation(configuration: &SuiteDefinitionConfiguration) -> Option<String> {
    (configuration.root_group.is_empty() && configuration.intended_for_qualification != Some(true))
        .then(|| EMPTY_ROOT_GROUP_MESSAGE.to_string())
}

/// `configuration` as the service STORES it: every optional member resolved to the value
/// the suite will actually run with.
///
/// A suite definition read answers the stored configuration, and the store holds a
/// RESOLVED one: a caller that named no device list, no long-duration flag, no
/// qualification flag and no protocol has not left those undecided — the suite runs against
/// no devices, as a short-duration, non-qualification suite, over the default MQTT protocol
/// ([`DEFAULT_PROTOCOL`]) — and a qualification suite that named an empty `rootGroup` has
/// asked for the default qualification root group ([`DEFAULT_QUALIFICATION_ROOT_GROUP`]),
/// which is what gets stored. So a read echoes resolved values rather than the caller's
/// omissions.
///
/// Idempotent, and it never overrides a value the caller did send. The required members
/// (`suiteDefinitionName`, `devicePermissionRoleArn`, and a non-empty `rootGroup`) are the
/// caller's and are left alone; the prose rules on the shape are checked BEFORE this
/// ([`definition_configuration_violation`]), so an empty `rootGroup` that reaches here
/// belongs to a qualification suite.
///
/// The two optional booleans are resolved alike because they are the same kind of member on
/// the same shape — an absent one has always behaved as `false` everywhere in this crate
/// (`intendedForQualification` decides whether a run has a qualification report and whether
/// an empty `rootGroup` is legal, and both read `Some(true)` only), so resolving it changes
/// no decision, only what a read reports.
pub fn resolved_configuration(
    mut configuration: SuiteDefinitionConfiguration,
) -> SuiteDefinitionConfiguration {
    configuration.devices = Some(configuration.devices.unwrap_or_default());
    configuration.intended_for_qualification =
        Some(configuration.intended_for_qualification.unwrap_or(false));
    configuration.is_long_duration_test = Some(configuration.is_long_duration_test.unwrap_or(false));
    configuration.protocol = Some(
        configuration
            .protocol
            .unwrap_or_else(|| DEFAULT_PROTOCOL.to_string()),
    );
    if configuration.root_group.is_empty() {
        configuration.root_group = DEFAULT_QUALIFICATION_ROOT_GROUP.to_string();
    }
    configuration
}

/// The version string for the `n`-th configuration a definition holds (1-based).
///
/// `SuiteDefinitionVersion` is `len 2..255`, so a bare ordinal (`"1"`) is not a legal
/// value for the member; the sequence is `v1`, `v2`, … and the same strings are what
/// `GetSuiteDefinition` accepts back on its `suiteDefinitionVersion` query.
pub fn version_label(n: usize) -> String {
    format!("v{n}")
}

/// The ordinal inside a label [`version_label`] produced, or `None` for any other string.
///
/// The inverse of [`version_label`], and the only place the label's shape is decoded:
/// ordering versions by ordinal is what keeps `v10` after `v2`, which their keys'
/// lexicographic order does not.
pub fn version_ordinal(label: &str) -> Option<usize> {
    label.strip_prefix('v')?.parse().ok()
}

/// Record one state-changing call of this service in the emulator's audit trail.
///
/// Every mutating operation here emits exactly one event on the `emulator::audit` target:
/// who presented which access key id, in which region, which operation, which resource,
/// and how the call ended (the subscriber stamps the time). That is what makes a
/// destructive call — `DeleteSuiteDefinition` cascading to a definition's runs, or
/// `StopSuiteRun` moving a run — attributable after the fact.
///
/// An ATTEMPTED mutation is recorded too, with the outcome that refused it (an update of
/// an id the store does not hold, say): `resource` is then the ARN the request NAMED, not
/// a claim that such a resource is stored. A handler reaches this through [`AuditedCall`]
/// rather than calling it directly, because that is what makes the record happen on the
/// paths that return early as well as on the one that completes.
///
/// Request-derived values are written with `Debug`, so a newline inside one cannot forge a
/// second record. Nothing secret is written: the presented ACCESS KEY ID identifies the
/// caller and is not a secret, and no secret key, session token or emulator credential is
/// read here.
pub fn audit(
    operation: &str,
    parts: &::http::request::Parts,
    region: &str,
    resource: &str,
    outcome: &str,
) {
    ::tracing::info!(
        target: "emulator::audit",
        operation = %operation,
        access_key_id = ?::emulator_server::extract_akid(&parts.headers),
        region = ?region,
        resource = ?resource,
        outcome = %outcome,
        "state-changing iotdeviceadvisor request"
    );
}

/// The outcome a record carries for a call that returned without naming one.
///
/// Only a call that got as far as changing something has an outcome of its own to report,
/// so a record left with this is a call that ended in a refusal whose error did not go
/// through [`AuditedCall::refused`]: the attempt is still recorded rather than dropped.
const UNREPORTED_OUTCOME: &str = "refused";

/// The one audit record a mutating call owes [`audit`], written when the handler returns —
/// WHICHEVER way it returns.
///
/// [`audit`] writes its record where it is called, so a handler that returns an error
/// before reaching it leaves none and the ATTEMPTED mutation the rule above promises goes
/// unrecorded. A handler opens this value with the resource the request NAMES before it
/// does any work, says how the call ended ([`AuditedCall::ended`]) or what refused it
/// ([`AuditedCall::refused`]), and the record is written when the value goes out of scope
/// — so "exactly one record per mutating call" holds on every path out of the handler,
/// the early returns and the ones `?` takes included.
#[derive(Debug)]
pub struct AuditedCall<'a> {
    operation: &'static str,
    parts: &'a ::http::request::Parts,
    region: String,
    resource: String,
    outcome: String,
}

impl<'a> AuditedCall<'a> {
    /// Open the record of an ATTEMPT to mutate `resource` through `operation`.
    ///
    /// `resource` is the ARN the request NAMED, which the store need not hold; a call that
    /// mints its resource names the collection it creates into instead
    /// ([`definition_collection_arn`]) until [`AuditedCall::acted_on`] replaces it with the
    /// ARN the call resolved.
    pub fn attempt(
        operation: &'static str,
        parts: &'a ::http::request::Parts,
        region: &str,
        resource: &str,
    ) -> Self {
        Self {
            operation,
            parts,
            region: region.to_string(),
            resource: resource.to_string(),
            outcome: UNREPORTED_OUTCOME.to_string(),
        }
    }

    /// Name the resource the call turned out to act on: the ARN a create minted, or the
    /// stored ARN a lookup resolved for the id the request named.
    pub fn acted_on(&mut self, resource: &str) {
        self.resource = resource.to_string();
    }

    /// How a call that was NOT refused ended — what it changed, or the no-op it was.
    pub fn ended(&mut self, outcome: impl Into<String>) {
        self.outcome = outcome.into();
    }

    /// Refuse the call with `error`: the record keeps the wire error code the caller is
    /// answered with, and the error is handed back for the handler to return.
    ///
    /// The code is read off the error rather than named a second time here, so a record of
    /// a refusal cannot drift from the refusal the client saw.
    pub fn refused<E: ::smithy_server_runtime::OperationError>(&mut self, error: E) -> E {
        self.outcome = format!("refused-{}", error.shape_name());
        error
    }
}

impl Drop for AuditedCall<'_> {
    fn drop(&mut self) {
        audit(
            self.operation,
            self.parts,
            &self.region,
            &self.resource,
            &self.outcome,
        );
    }
}

/// The resource an audit record names for a call that names no resource of its own: this
/// account's suite-definition COLLECTION in `region`.
///
/// `CreateSuiteDefinition` identifies no definition — the one it mints does not exist
/// until the call succeeds — so a refused create names the resource TYPE it creates into,
/// an ARN that carries no id and so is never one a stored definition answers to.
pub fn definition_collection_arn(region: &str) -> String {
    arn(region, "suitedefinition")
}

/// A freshly minted `UUID`-shaped id: a random v4 UUID, 36 characters, inside the
/// modelled 12..36 length.
///
/// The generator is the `uuid` crate's, reached through `emulator_telemetry`'s `random()`
/// constructor — `uuid::Uuid::new_v4().as_hyphenated()` verbatim
/// (`vendor/emulator-telemetry/src/types.rs`) — because `emulator-telemetry` is already
/// one of this crate's dependencies, so no new entry in the workspace lockfile is needed
/// for it. Only the generator is borrowed: the newtype's telemetry meaning is dropped on
/// the next line and nothing about the UUID's layout or entropy is relied on here. The
/// same borrowing is what `service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend`
/// does for its ids.
pub fn new_uuid() -> String {
    ::emulator_telemetry::BatchId::random().as_str().to_string()
}

/// The region to stamp into ARNs and endpoints: the SigV4 credential scope's region when
/// the request is signed and that region has the published shape, else [`DEFAULT_REGION`].
///
/// The scope region is caller-supplied, and it is interpolated into ARNs and into the
/// endpoint host, where `:` or an extra dotted label would produce a structurally
/// well-formed but fabricated value AWS never mints. AWS resolves the region from the
/// endpoint it served and only honours a region it publishes, so a region that does not
/// LOOK like a published one is replaced by the default instead of being reflected back.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| has_published_region_shape(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// True when `region` has the shape every AWS region identifier has:
/// `<2+ lowercase letters>(-<lowercase letters>)+-<digit>`, e.g. `us-east-1`,
/// `ap-southeast-4`, `il-central-1`, `us-gov-west-1`.
///
/// A shape test, not an allowlist of the regions that exist today: a new region must keep
/// working without an emulator change, while a value carrying ARN- or host-structural
/// characters (`:`, `.`, `/`, whitespace, upper case) is refused.
pub fn has_published_region_shape(region: &str) -> bool {
    let mut parts = region.split('-');
    let Some(first) = parts.next() else {
        return false;
    };
    if first.len() < 2 || !first.bytes().all(|b| b.is_ascii_lowercase()) {
        return false;
    }
    let rest: Vec<&str> = parts.collect();
    // At least one middle label plus the trailing number.
    let Some((number, middle)) = rest.split_last() else {
        return false;
    };
    if middle.is_empty() {
        return false;
    }
    middle
        .iter()
        .all(|label| !label.is_empty() && label.bytes().all(|b| b.is_ascii_lowercase()))
        && !number.is_empty()
        && number.bytes().all(|b| b.is_ascii_digit())
}

/// This service's ARN for `resource` in `region`.
pub fn arn(region: &str, resource: &str) -> String {
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, resource)
}

/// The scope an ARN names its resource in: the `arn:<partition>:<service>:<region>:<account>:`
/// fields that precede the resource portion.
///
/// A resource this store already holds records the scope it was minted in inside its own
/// ARN, so a call that mints a CHILD of that resource takes the scope off the PARENT's
/// stored ARN ([`ArnScope::arn`]) rather than off the request that happened to make the
/// call. Parent and child then agree about the scope an ARN carries — the one
/// [`ComAmazonawsIotdeviceadvisorIotsenateserviceState::holds_tagged_resource`] reads out
/// of it — so a single scope addresses both, and every regional value the child reports
/// (its ARN and its endpoint, [`ArnScope::region`]) names that one scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArnScope<'a> {
    /// `arn:<partition>:<service>:<region>:<account>:` verbatim, including the separator
    /// that closes the account field, so an ARN in this scope is this followed by a
    /// resource portion.
    prefix: &'a str,
    /// The region field of those five.
    region: &'a str,
}

impl<'a> ArnScope<'a> {
    /// The scope `arn` names its resource in, or `None` when `arn` does not carry the six
    /// ARN fields with a partition, service, region and account — there is then no scope to
    /// inherit, and the caller falls back to the request's own.
    pub fn of(arn: &'a str) -> Option<Self> {
        let fields = arn_fields(arn)?;
        if fields.partition.is_empty()
            || fields.service.is_empty()
            || fields.region.is_empty()
            || fields.account.is_empty()
        {
            return None;
        }
        // The resource portion is the TAIL of `arn`, so what precedes it is the scope,
        // separator included.
        let prefix = &arn[..arn.len() - fields.resource.len()];
        Some(Self {
            prefix,
            region: fields.region,
        })
    }

    /// The region this scope is in — what a resource of this scope reports its endpoint in.
    pub fn region(&self) -> &'a str {
        self.region
    }

    /// `resource` addressed in THIS scope: the scope's partition, service, region and
    /// account, with the resource portion the caller names.
    pub fn arn(&self, resource: &str) -> String {
        format!("{}{resource}", self.prefix)
    }
}

/// The modelled `Endpoint` length bounds (`smithy_query(query="shape",shape="Endpoint")`
/// gives `len 45..75`), so every host this module builds can be checked against them.
pub const MIN_ENDPOINT_LENGTH: usize = 45;
/// See [`MIN_ENDPOINT_LENGTH`].
pub const MAX_ENDPOINT_LENGTH: usize = 75;

/// Everything a Device Advisor host carries besides its label and its region.
const ENDPOINT_FIXED_LENGTH: usize = ".deviceadvisor.iot.".len() + ".amazonaws.com".len();

/// One Device Advisor host: a single label under this service's endpoint domain.
///
/// Both endpoints Device Advisor vends — the account-level one and a device-level one —
/// have this shape, so the LABEL is the only thing that tells them apart.
fn endpoint_host(label: &str, region: &str) -> String {
    format!("{label}.deviceadvisor.iot.{region}.amazonaws.com")
}

/// The region a host whose label is `label_length` characters long can carry while staying
/// inside [`MAX_ENDPOINT_LENGTH`]: the caller's region, or [`DEFAULT_REGION`] when the
/// caller's would push the host past it.
///
/// Every region AWS publishes fits; a longer one falls back rather than producing an
/// out-of-range `Endpoint` (the ARN still carries the caller's own region).
fn host_region(label_length: usize, region: &str) -> &str {
    if label_length + ENDPOINT_FIXED_LENGTH + region.len() > MAX_ENDPOINT_LENGTH {
        DEFAULT_REGION
    } else {
        region
    }
}

/// The subject an ACCOUNT-LEVEL endpoint's label is derived from: the account and the
/// region, and nothing a caller names.
const ACCOUNT_ENDPOINT_SUBJECT: &str = "account-endpoint";

/// The ACCOUNT-LEVEL Device Advisor endpoint for this account in `region`.
///
/// Device Advisor vends one account-scoped host, and this is the host reported to a caller
/// that names NO device under test. Its label is an OPAQUE one minted for the account and
/// region ([`endpoint_label`]) — the same form a device-level host's label has — so it is
/// derived here rather than stored, and the same account and region always answer the same
/// host.
///
/// An operation that REPORTS an endpoint reaches it through [`announced_endpoint`] rather
/// than calling this directly, so no two operations can report different hosts for one
/// device and none announces a host the caller cannot reach.
pub fn endpoint_for_region(region: &str) -> String {
    let region = host_region(ENDPOINT_LABEL_LENGTH, region);
    let label = endpoint_label(
        ACCOUNT_ENDPOINT_SUBJECT,
        &[::emulator_server::emulator_account_id(), region],
    );
    endpoint_host(&label, region)
}

/// The device a request names, exactly as it names it.
///
/// Device Advisor lets a caller name the device under test by its thing, by its
/// certificate, or by the role it assumes, wherever a device appears: on a `GetEndpoint`
/// request's own members, and inside the `DeviceUnderTest` a suite definition or a suite
/// run configuration carries. A `GetEndpoint` request that names NONE of them is asking
/// for the account-level endpoint instead.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DeviceIdentity<'a> {
    pub thing_arn: Option<&'a str>,
    pub certificate_arn: Option<&'a str>,
    pub device_role_arn: Option<&'a str>,
}

impl<'a> DeviceIdentity<'a> {
    /// The identity a modelled `DeviceUnderTest` names, borrowed from it.
    pub fn of(device: &'a DeviceUnderTest) -> Self {
        Self {
            thing_arn: device.thing_arn.as_deref(),
            certificate_arn: device.certificate_arn.as_deref(),
            device_role_arn: device.device_role_arn.as_deref(),
        }
    }

    /// True when the request named no device at all.
    pub fn names_no_device(&self) -> bool {
        self.thing_arn.is_none() && self.certificate_arn.is_none() && self.device_role_arn.is_none()
    }

    /// Every ARN-typed member of a device identity: its wire name, the value this identity
    /// carries for it, and the AWS service whose ARN that member names — a thing and a
    /// certificate are IoT resources, and the device role is the IAM role the device
    /// assumes. The one table for the crate, so every operation that accepts a device
    /// checks the same members against the same services.
    fn arn_members(&self) -> [(&'static str, Option<&'a str>, &'static str); 3] {
        [
            ("thingArn", self.thing_arn, IOT_RESOURCE_SERVICE),
            ("certificateArn", self.certificate_arn, IOT_RESOURCE_SERVICE),
            ("deviceRoleArn", self.device_role_arn, DEVICE_ROLE_SERVICE),
        ]
    }
}

/// The AWS service whose ARNs name a thing and a certificate: both are IoT resources.
pub const IOT_RESOURCE_SERVICE: &str = "iot";

/// The AWS service whose ARNs name the role a device assumes, and the role Device Advisor
/// assumes to reach the devices under test: both are IAM roles.
pub const DEVICE_ROLE_SERVICE: &str = "iam";

/// How a device identity has to name its device, as the member that carries it documents.
///
/// The requirement is stated per member: a suite run's primary device "requires a thing ARN
/// or a certificate ARN" (`SuiteRunConfiguration$primaryDevice`), a device under test of a
/// suite definition takes "a thing ARN, certificate ARN or device role ARN"
/// (`DeviceUnderTest`), and a `GetEndpoint` request that names no device is asking for the
/// account-level endpoint, so naming none is legal there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceNaming {
    /// The device is named by its thing or by its certificate.
    ThingOrCertificate,
    /// The device is named by its thing, its certificate or the role it assumes.
    AnyDeviceArn,
    /// Naming no device at all is legal for this member.
    Optional,
}

impl DeviceNaming {
    /// What this member requires, worded for the refusal that names it.
    fn requirement(self) -> &'static str {
        match self {
            Self::ThingOrCertificate => "a thing ARN or a certificate ARN",
            Self::AnyDeviceArn => "a thing ARN, certificate ARN or device role ARN",
            Self::Optional => "nothing",
        }
    }

    /// True when `device` names the device the way this member requires.
    fn satisfied_by(self, device: &DeviceIdentity<'_>) -> bool {
        match self {
            Self::ThingOrCertificate => {
                device.thing_arn.is_some() || device.certificate_arn.is_some()
            }
            Self::AnyDeviceArn => !device.names_no_device(),
            Self::Optional => true,
        }
    }
}

/// The violation an ARN-typed member commits when its value is not an ARN of the service
/// that member names, worded from the request: the member's path and the form it must have.
///
/// The modelled length bounds (`AmazonResourceName` is `len 20..2048`) are enforced before
/// a handler runs; the ARN FORM is not modelled, so it is checked here and refused with the
/// operation's own client-input error. A legal value is as long as a legal `message`, so what
/// the refusal STATES of it is bounded on the way out too ([`bounded_message`]).
pub fn arn_member_violation(member: &str, value: &str, service: &str) -> Option<String> {
    (!is_arn_of_service(value, service)).then(|| {
        // The CHECK is on the value the caller sent; only what the message STATES of it is
        // bounded ([`quoted_value`]), so the refusal cannot run past what `Message` allows.
        let shown = quoted_value(value);
        bounded_message(format!(
            "{member} must be an Amazon Resource Name of the form \
             arn:<partition>:{service}:<region>:<account>:<resource>, got {shown}"
        ))
    })
}

/// The violation the device identity at `path` commits, if any.
///
/// Two rules, both properties of the device identity rather than of one operation: every
/// ARN it carries has to BE an Amazon Resource Name of the service that member names, and
/// the identity has to NAME a device the way `naming` requires. `path` is where the
/// identity sits in the request (empty for one carried by the input's own members), so the
/// message names the member the caller actually sent.
pub fn device_violation(
    path: &str,
    device: &DeviceIdentity<'_>,
    naming: DeviceNaming,
) -> Option<String> {
    for (member, value, service) in device.arn_members() {
        if let Some(value) = value {
            let member = if path.is_empty() {
                member.to_string()
            } else {
                format!("{path}.{member}")
            };
            if let Some(violation) = arn_member_violation(&member, value, service) {
                return Some(violation);
            }
        }
    }
    (!naming.satisfied_by(device)).then(|| {
        let subject = if path.is_empty() {
            "the device under test"
        } else {
            path
        };
        format!("{subject} requires {}", naming.requirement())
    })
}

/// True when a run configuration asks for a run that executes in parallel with others.
///
/// `SuiteRunConfiguration$parallelRun` is the input that governs this (the model: "TRUE if
/// multiple test suites run in parallel"), and it is optional: a caller who sends nothing
/// has not asked for a parallel run, so an absent member reads the same as `false`.
pub fn asks_for_parallel_run(configuration: &SuiteRunConfiguration) -> bool {
    configuration.parallel_run.unwrap_or(false)
}

/// Every rule the model states in prose for a `SuiteRunConfiguration`, which no modelled
/// constraint can express: the run's primary device has to name a device, and by an ARN of
/// the service the member naming it belongs to.
///
/// The rule belongs to the SHAPE, so every operation that accepts a run configuration asks
/// here rather than restating it.
pub fn run_configuration_violation(configuration: &SuiteRunConfiguration) -> Option<String> {
    device_violation(
        "suiteRunConfiguration.primaryDevice",
        &DeviceIdentity::of(&configuration.primary_device),
        DeviceNaming::ThingOrCertificate,
    )
}

/// Every rule the model states in prose for a `SuiteDefinitionConfiguration`: the
/// `rootGroup` rule ([`root_group_violation`]) and the ARNs the configuration points at —
/// the IAM role Device Advisor assumes for the devices, and each device under test, which
/// the shape documents as needing a thing, a certificate or a device role ARN.
///
/// The rule belongs to the SHAPE, so every operation that accepts a definition
/// configuration asks here rather than restating it.
pub fn definition_configuration_violation(
    configuration: &SuiteDefinitionConfiguration,
) -> Option<String> {
    if let Some(violation) = root_group_violation(configuration) {
        return Some(violation);
    }
    if let Some(violation) = arn_member_violation(
        "suiteDefinitionConfiguration.devicePermissionRoleArn",
        &configuration.device_permission_role_arn,
        DEVICE_ROLE_SERVICE,
    ) {
        return Some(violation);
    }
    configuration
        .devices
        .iter()
        .flatten()
        .enumerate()
        .find_map(|(index, device)| {
            device_violation(
                &format!("suiteDefinitionConfiguration.devices[{index}]"),
                &DeviceIdentity::of(device),
                DeviceNaming::AnyDeviceArn,
            )
        })
}

/// Length of the single DNS label a Device Advisor host carries, account-level and
/// device-level alike.
///
/// The exact length is this emulator's choice — what a caller depends on is that the label
/// is one stable, opaque, lowercase-alphanumeric DNS label (the form of the AWS IoT
/// Developer Guide's account-specific endpoint prefix, e.g. `a3qexamplesffut`), and that
/// the host stays inside the modelled `Endpoint` bounds for every region AWS publishes.
pub const ENDPOINT_LABEL_LENGTH: usize = 14;

/// The alphabet an endpoint label's FIRST character is drawn from: a hostname label is read
/// case-insensitively and AWS writes these prefixes in lowercase, and starting with a
/// letter keeps the label in the preferred form of RFC 1035 §2.3.1.
const ENDPOINT_LABEL_LEADING_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz";

/// The alphabet the rest of an endpoint label is drawn from.
const ENDPOINT_LABEL_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// The OPAQUE label a Device Advisor host carries for `subject` — the kind of endpoint it
/// is — and the `fields` that endpoint belongs to.
///
/// Every host this service vends carries one such label, so a client is handed an address
/// and never an AWS identifier it could read an account, a thing or a role out of. It is
/// DERIVED (the SHA-256 of the domain-separated, length-prefixed subject and fields, mapped
/// onto the label alphabet) rather than drawn at random or copied out of an identifier:
/// asking twice for one endpoint answers the same label, two endpoints get two labels, and
/// nothing is stored per endpoint. Truncating a digest to a single label makes a collision
/// between two subjects a probability rather than an impossibility; nothing treats the label
/// as a unique key. The same derivation the `com-amazonaws-iot-awsiotservice` crate uses for
/// an account's IoT endpoint prefix.
fn endpoint_label(subject: &str, fields: &[&str]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"iotdeviceadvisor/");
    hasher.update(subject.as_bytes());
    hasher.update(b"/");
    // Every field length-prefixed, so no two different subjects can feed the digest the
    // same bytes.
    for field in fields {
        hasher.update(field.len().to_string().as_bytes());
        hasher.update(b":");
        hasher.update(field.as_bytes());
        hasher.update(b";");
    }
    let digest = hasher.finalize();
    digest
        .iter()
        .take(ENDPOINT_LABEL_LENGTH)
        .enumerate()
        .map(|(position, byte)| {
            let alphabet = if position == 0 {
                ENDPOINT_LABEL_LEADING_ALPHABET
            } else {
                ENDPOINT_LABEL_ALPHABET
            };
            char::from(alphabet[usize::from(*byte) % alphabet.len()])
        })
        .collect()
}

/// The subject a DEVICE-LEVEL endpoint's label is derived from: the account, the region and
/// the ARNs the request names the device by.
const DEVICE_ENDPOINT_SUBJECT: &str = "device-endpoint";

/// The DEVICE-LEVEL Device Advisor endpoint for the device `device` names, in `region`.
///
/// Device Advisor vends an endpoint that is unique per device beside the account-level one
/// (AWS IoT Core Device Advisor developer guide, "Device Advisor endpoints"), so the label
/// is DERIVED — from the account, the region the host carries, and the ARNs the request
/// names — by the one [`endpoint_label`] derivation every host this service vends carries:
/// asking twice for one device answers the same host, and two devices get two hosts, with
/// nothing stored per device.
///
/// This service holds no device registry — only the devices a suite definition or a suite
/// run points at — so two ARNs naming one real device (a thing and that thing's
/// certificate) are two identities here, each with its own stable host, rather than one.
pub fn endpoint_for_device(region: &str, device: &DeviceIdentity<'_>) -> String {
    let region = host_region(ENDPOINT_LABEL_LENGTH, region);
    let label = endpoint_label(
        DEVICE_ENDPOINT_SUBJECT,
        &[
            ::emulator_server::emulator_account_id(),
            region,
            device.thing_arn.unwrap_or(""),
            device.certificate_arn.unwrap_or(""),
            device.device_role_arn.unwrap_or(""),
        ],
    );
    endpoint_host(&label, region)
}

/// The Device Advisor host AWS ITSELF vends for the device `device` names, in `region`:
/// the ONE derivation behind every endpoint this service reports.
///
/// A caller or a run that names no device is vended the account-level host
/// ([`endpoint_for_region`]); one that names a device — by its thing, its certificate or the
/// role it assumes — is vended that device's own host ([`endpoint_for_device`]). Both are
/// a function of the account, the region and the identity alone, with nothing stored and
/// nothing per call, so the host a suite run reports for its primary device and the host
/// `GetEndpoint` vends for that same device are the same string.
///
/// This is the host a client that resolves AWS's own names is told; what an operation
/// ANNOUNCES to a client that reached THIS emulator is the address it can dial
/// ([`announced_endpoint`]), because a host nothing here answers on is no endpoint at all.
pub fn endpoint_for(region: &str, device: &DeviceIdentity<'_>) -> String {
    if device.names_no_device() {
        endpoint_for_region(region)
    } else {
        endpoint_for_device(region, device)
    }
}

/// True when `value` is an Amazon Resource Name of `service`.
///
/// The form every AWS ARN has — `arn:<partition>:<service>:<region>:<account>:<resource>`
/// with a non-empty partition, service and resource — with the service field equal to
/// `service`. The region and account fields may be empty, as they are in a global
/// service's ARN (an IAM role's, say).
pub fn is_arn_of_service(value: &str, service: &str) -> bool {
    let mut fields = value.splitn(6, ':');
    fields.next() == Some("arn")
        && fields.next().is_some_and(|partition| !partition.is_empty())
        && fields.next() == Some(service)
        // The region and the account, either of which a global service's ARN leaves empty.
        && fields.next().is_some()
        && fields.next().is_some()
        && fields.next().is_some_and(|resource| !resource.is_empty())
}

// ---------------------------------------------------------------------------
// The qualification report's download URL
//
// `GetSuiteRunReport` reports a DOWNLOAD URL for the report a successful qualifying
// run leaves. AWS announces that document from object storage, which this emulator
// does not have, so what is announced here is a LOCATOR built from the report's own
// subject on the endpoint the caller reached. Nothing in this service serves it, and
// nothing reads it back: the report AWS compiles is AWS's, and a document of this
// emulator's own making would not be it.

/// The path a qualification report's download URL carries, under which the report of one
/// `(suite definition, suite run)` is addressed.
const QUALIFICATION_REPORT_PATH: &str = "/qualificationReports";

/// The endpoint a report URL names when the request carries none, so a URL this service
/// announces always names an endpoint rather than an empty authority.
const LOOPBACK_HOST: &str = "127.0.0.1";

/// The endpoint the request reached this emulator on, as a report URL names it: the
/// request's own `Host`, else the authority its URI carries, else [`LOOPBACK_HOST`].
///
/// Read from the request rather than built from the model's endpoint template, because
/// the URL has to resolve for whoever was handed it: a client addressing the emulator by
/// socket is handed that socket, and a literal AWS endpoint here would send a following
/// client to the real service. The value is the CALLER's, and it is interpolated into a
/// URL this service announces, so only a bare authority is honoured.
///
/// Each source is tested on its own, in that order, and the FIRST one carrying a bare
/// authority is what the URL names: a `Host` this service cannot honour leaves the
/// authority the URI carries still to be read, so the chain is the documented one for
/// every unusable `Host` rather than only for one that is not even UTF-8.
///
/// The same rule, with the same two sources and the same filter, as
/// `service-com-amazonaws-artifact-artifact`'s `document_url_host`: both services resolve
/// the caller's own endpoint for a URL they announce, and this note is here so the two
/// cannot drift apart unnoticed if either changes.
pub fn report_url_host(parts: &::http::request::Parts) -> String {
    [
        parts
            .headers
            .get(::http::header::HOST)
            .and_then(|value| value.to_str().ok()),
        // The URI's AUTHORITY, not its host: a request that carries its endpoint on the
        // URI carries the port there too, and a URL announced without the port would not
        // resolve back to the socket the caller reached.
        parts.uri.authority().map(::http::uri::Authority::as_str),
    ]
    .into_iter()
    .flatten()
    .find(|candidate| is_bare_authority(candidate))
    .unwrap_or(LOOPBACK_HOST)
    .to_string()
}

/// True when `value` is a bare authority: a host with an optional port and nothing else.
///
/// A path, query, fragment, userinfo or space makes it something other than an endpoint,
/// so a URL built around it would not resolve back to this emulator.
fn is_bare_authority(value: &str) -> bool {
    !value.is_empty()
        && !value.contains(['/', '?', '#', '@', '\\', ' ', '\t'])
        && value.parse::<::http::uri::Authority>().is_ok()
}

/// The download URL of one suite run's qualification report, addressed to `host`.
///
/// Derived from the report's own subject: the definition that was run and the run that
/// produced the report, so every read of one report announces the same URL rather than a
/// freshly invented one, and the URL names the record it can be traced back to. The ids
/// are the UUIDs this service mints, which carry no character the path shape would have to
/// escape. AWS announces the document from object storage, which this emulator does not
/// have; what it announces instead is this locator on the endpoint the caller reached,
/// never a literal AWS endpoint (which would send a following client to the real service).
pub fn qualification_report_url(
    host: &str,
    definition: &SuiteDefinition,
    run: &SuiteRun,
) -> String {
    format!(
        "https://{host}{QUALIFICATION_REPORT_PATH}/{}/{}",
        definition.id, run.id
    )
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIotdeviceadvisorIotsenateserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIotdeviceadvisorIotsenateserviceState>)
}
