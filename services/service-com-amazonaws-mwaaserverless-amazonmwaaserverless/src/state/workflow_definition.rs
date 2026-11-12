// SPDX-License-Identifier: CC0-1.0

//! The workflow definition a version SNAPSHOTS, and what that definition declares: the
//! schedule it runs on and the tasks it is made of.
//!
//! `DefinitionS3Location` is documented as the place the service "takes a snapshot of
//! the definition file at the time of workflow creation", so the definition a workflow
//! reports is the CONTENT read once, when the version was created, and frozen in that
//! version's [`crate::state::WorkflowSnapshot`]. Every later read answers from the
//! frozen copy: overwriting the Amazon S3 object changes nothing a caller reads back,
//! and no reporting operation ever goes to Amazon S3.
//!
//! The object is read through the emulator's own Amazon S3 service over the in-process
//! callback, which is the framework's documented seam for calling another emulated
//! service: nothing here opens a socket or leaves the host.
//!
//! That one read also establishes WHETHER the store serves an object at the address at
//! all ([`DefinitionRead`]), which is the fact an operation that freezes a configuration
//! holds the request to ([`crate::state::configuration_dependencies`]); what this module
//! answers is only what the read established, never what an operation does about it.

use crate::ops::types::{DefinitionS3Location, ScheduleConfiguration};

/// The most definition content this emulator snapshots for ONE workflow version.
///
/// The content is caller-supplied data held per version, so it needs a ceiling for the
/// same reason the version list itself has one
/// ([`crate::state::MAX_VERSIONS_PER_WORKFLOW`]): with that ceiling and
/// [`crate::state::MAX_WORKFLOWS`] it is what keeps the store finite. The number is the
/// emulator's own, not a published service limit, and is set generously for a YAML DAG
/// declaration; an object larger than this is left unsnapshotted, so the version reports
/// no definition content rather than this process holding an object of any size.
///
/// It is deliberately well past the length trait `GenericString` carries (1..255), which
/// is the shape `WorkflowDefinition` targets: the member is documented as "The workflow
/// definition content", and a definition file cut to 255 characters would report
/// something other than the content the version snapshotted. The whole snapshotted
/// content is reported, and this ceiling is what bounds it.
pub const MAX_DEFINITION_BYTES: usize = 16 * 1024;

/// The most parser events the schedule walk reads out of one definition.
///
/// Every event but the stream and document framing consumes at least one byte of the
/// content, so a document within [`MAX_DEFINITION_BYTES`] cannot reach this budget: it
/// is the backstop that keeps the walk's running time bounded whatever the parser does
/// with input it cannot make sense of. A document that reaches it was not read to the
/// end, so the walk stops there.
const MAX_DEFINITION_EVENTS: usize = 4 * MAX_DEFINITION_BYTES;

/// How deep into a definition the schedule walk follows a document.
///
/// A DAG-level declaration sits at the document root or one mapping in, so nothing this
/// reader looks for is deeper than a handful of levels; the bound is set well clear of
/// that and is what keeps the walk's own frame stack small however deeply a
/// caller-supplied document nests. A document nesting past it is not followed further.
const MAX_DEFINITION_NESTING: usize = 64;

/// The keys an Apache Airflow DAG declares its schedule under, in the order they are
/// looked for: `schedule` is the DAG parameter's current name and `schedule_interval`
/// the name it carried before, which Airflow still accepts.
const SCHEDULE_KEYS: &[&str] = &["schedule", "schedule_interval"];

/// The key a definition may nest its DAG-level declarations under, so a document that
/// wraps them in one mapping declares its schedule at `dag.schedule` rather than at the
/// document root.
const DAG_KEY: &str = "dag";

/// The key a definition declares its TASKS under, at the document root or inside its
/// [`DAG_KEY`] mapping: the definition "defines the workflow structure using supported
/// Amazon Web Services operators and tasks" and is written to "include task dependencies,
/// scheduling information, and operator configurations"
/// (`smithy_query(query="doc", shape="CreateWorkflowRequest",
/// member="DefinitionS3Location")`), and `tasks` is the key Apache Airflow's own DAG
/// vocabulary spells that collection with.
const TASKS_KEY: &str = "tasks";

/// The keys ONE task entry declares its own identifier under, in the order they are
/// looked for: Apache Airflow's `task_id` and the shorter `id` and `name` spellings a
/// declaration carries it under. A task entry that is the VALUE of a key in a `tasks`
/// MAPPING takes that key as its identifier, which an explicit declaration overrides.
const TASK_ID_KEYS: &[&str] = &["id", "task_id", "name"];

/// The keys a task entry names the operator that runs it under — what
/// `TaskInstanceSummary$OperatorName` ("The name of the Apache Airflow operator used for
/// this task instance") reports.
const TASK_OPERATOR_KEYS: &[&str] = &["operator", "operator_name", "type"];

/// The keys a task entry declares the tasks it waits for under ("include task
/// dependencies"): one task identifier, or a sequence of them. The relation is the
/// UPSTREAM one — the tasks that must finish before this one runs — which is the direction
/// each of these keys names.
const TASK_DEPENDENCY_KEYS: &[&str] = &["dependencies", "depends_on", "upstream"];

/// The most task declarations this reader takes out of ONE definition.
///
/// A run materialises one task instance per declared task, so this is what bounds that
/// map; with [`crate::state::MAX_RUNS_PER_WORKFLOW`] and [`crate::state::MAX_WORKFLOWS`]
/// it keeps the whole store finite. The number is the emulator's own ceiling, not a
/// published service limit, and is set well past the tasks a definition within
/// [`MAX_DEFINITION_BYTES`] realistically declares; a definition declaring more is one
/// this emulator does not run (see `crate::state::workflow_execution::ExecutionPlan`).
pub const MAX_DECLARED_TASKS: usize = 128;

/// Apache Airflow's cron PRESETS and the cron expression each one stands for, from the
/// preset table in Airflow's DAG scheduling documentation (`@hourly`, `@daily` /
/// `@midnight`, `@weekly`, `@monthly`, `@quarterly`, `@yearly` / `@annually`).
///
/// A preset is a shorthand for a cron expression, and `ScheduleConfiguration$CronExpression`
/// is documented as "a cron expression ... standard cron syntax", so a definition that
/// declares a preset reports the expression the preset stands for. The presets that stand
/// for NO cron schedule at all (`@once`, `@continuous`) are deliberately absent: a
/// workflow declaring one has no cron expression to report.
const CRON_PRESETS: &[(&str, &str)] = &[
    ("@hourly", "0 * * * *"),
    ("@daily", "0 0 * * *"),
    ("@midnight", "0 0 * * *"),
    ("@weekly", "0 0 * * 0"),
    ("@monthly", "0 0 1 * *"),
    ("@quarterly", "0 0 1 */3 *"),
    ("@yearly", "0 0 1 1 *"),
    ("@annually", "0 0 1 1 *"),
];

/// The field counts a cron expression is written with: five is standard cron syntax
/// (minute, hour, day-of-month, month, day-of-week), and EventBridge Scheduler — which
/// this service's schedules run on, per `ScheduleConfiguration`'s own docs — adds a
/// sixth, year, field.
const CRON_FIELD_COUNTS: &[usize] = &[5, 6];

/// The characters that give a cron FIELD its structure, between the values in it: the
/// list, range and step separators (`,`, `-`, `/`) and the nth-weekday qualifier (`#`) of
/// standard cron and of EventBridge Scheduler's dialect.
///
/// They only SEPARATE values; each position they mark out still carries a value of its
/// own ([`is_cron_field_value`]), so punctuation on its own is not a field.
const CRON_FIELD_SEPARATORS: &[char] = &[',', '-', '/', '#'];

/// The wildcards a cron field may carry where a value stands: every value of the field
/// (`*`) and no particular value (`?`, which EventBridge Scheduler takes in the day
/// fields).
///
/// A wildcard is a VALUE a position carries, like a number or a name, not structure: `*`
/// is a whole field on its own and `*/2` is a step over it.
const CRON_FIELD_WILDCARDS: &[&str] = &["*", "?"];

/// The names a cron month or day-of-week field may carry instead of a number: the twelve
/// three-letter month abbreviations and the seven day-of-week ones, as cron spells them.
const CRON_FIELD_NAMES: &[&str] = &[
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC", "SUN",
    "MON", "TUE", "WED", "THU", "FRI", "SAT",
];

/// The day-of-month qualifiers a cron field may carry on their own: the last day of the
/// month, and the weekday nearest a given day.
const CRON_FIELD_QUALIFIERS: &[&str] = &["L", "W", "LW"];

/// What one version froze out of the definition object: the content itself and the
/// schedule that content declares.
#[derive(Debug, Clone)]
pub struct DefinitionSnapshot {
    /// The definition content, exactly as the object held it when the version was
    /// created — what `WorkflowDefinition` reports.
    pub content: String,
    /// The schedule the content declares, or `None` when it declares none — what
    /// `ScheduleConfiguration` reports.
    pub schedule_configuration: Option<ScheduleConfiguration>,
}

/// An Amazon S3 client over the in-process callback into this same emulator.
///
/// `region` is the calling request's own signing region, so the definition object is
/// read where the caller's own Amazon S3 requests go. No network client is configured:
/// the callback routes in process, and this crate names no other service's crate.
fn s3_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The in-process transport routes on the request's own path, and a virtual-host
        // bucket name is not a host this emulator resolves, so the request addresses its
        // bucket in the path.
        .force_path_style(true)
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        // The transport is in-process: there is nothing to retry, time out or watch for
        // a stalled stream, and each of those would need a sleep implementation this
        // client is not given. The same goes for caching the identity: the credentials
        // are handed over directly.
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// What reading the definition object a new version would freeze ESTABLISHED about the
/// location that addresses it.
///
/// The read answers two questions at once, and they are different questions: WHETHER
/// Amazon S3 serves an object at that address at all — which is what decides whether the
/// configuration naming it can be frozen ([`crate::state::configuration_dependencies`]) —
/// and WHICH content the version freezes. Both come out of the one read, so no caller
/// reads the object twice to learn them.
#[derive(Debug, Clone)]
pub enum DefinitionRead {
    /// Amazon S3 answered about an object it HOLDS at that address, so the location is
    /// one this service can read: the definition the version freezes, or `None` when
    /// that object holds no definition content to report (it holds no bytes, is not
    /// text, or is past [`MAX_DEFINITION_BYTES`]), in which case the version reports no
    /// definition content and no schedule rather than content this service never read.
    Snapshotted(Option<DefinitionSnapshot>),
    /// Amazon S3 ANSWERED that the location serves no object this service can read: it
    /// holds nothing at that address, or it will not serve the read. The address is also
    /// this answer when it names no object anywhere — a blank bucket or key, or a bucket
    /// name Amazon S3 holds no bucket under — since no read of it could ever serve one.
    Inaccessible,
    /// Nothing was established about the location: there is no Amazon S3 to ask, or the
    /// call could not be dispatched. Nothing about the request is at fault.
    Unanswered,
}

impl DefinitionRead {
    /// The definition this version freezes, whatever the read established: `None` wherever
    /// there is no content to report, so a version is never given content this service
    /// did not read.
    #[must_use]
    pub fn snapshot(self) -> Option<DefinitionSnapshot> {
        match self {
            Self::Snapshotted(snapshot) => snapshot,
            Self::Inaccessible | Self::Unanswered => None,
        }
    }
}

/// Read the object `location` addresses ONCE, now, and answer what that read established.
///
/// The address is read EXACTLY as the request carried it — an Amazon S3 object key may
/// hold leading or trailing spaces, so nothing here trims one — which is what lets the
/// refusal a caller gets name the very address that was read.
///
/// `VersionId` is passed through when the location names one, because it selects which
/// object version is snapshotted ("If not specified, the latest version is used").
///
/// What the object store's REFUSAL establishes is read by the tree's one reading of it
/// ([`::service_common::s3_object_read::answered_refusal`]), so this service agrees with
/// every other about what an Amazon S3 answer MEANT; what this service then DOES about
/// each outcome is its own ([`crate::state::configuration_dependencies`]).
pub async fn read_definition(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    location: &DefinitionS3Location,
) -> DefinitionRead {
    let (bucket, object_key) = (location.bucket.as_str(), location.object_key.as_str());
    // An address that is not an address at all is checked BEFORE anything is asked of
    // the store, the way the shared object read checks it: a blank bucket or key, and a
    // value Amazon S3 holds no bucket under, name no object anywhere.
    if bucket.is_empty()
        || object_key.is_empty()
        || !::service_common::s3_bucket_name::is_well_formed(bucket)
    {
        return DefinitionRead::Inaccessible;
    }
    let answered = s3_client(ctx, region)
        .get_object()
        .bucket(bucket)
        .key(object_key)
        .set_version_id(location.version_id.clone())
        .send()
        .await;
    let object = match answered {
        Ok(object) => object,
        Err(error) => {
            return match ::service_common::s3_object_read::answered_refusal(&error) {
                // The store answered about that address: it holds nothing there, or it
                // will not serve the read.
                ::service_common::s3_object_read::ObjectRead::Absent
                | ::service_common::s3_object_read::ObjectRead::Denied => {
                    DefinitionRead::Inaccessible
                }
                // The store answered something else about an object it holds, so the
                // location is readable and there is simply nothing to snapshot.
                ::service_common::s3_object_read::ObjectRead::Served(_)
                | ::service_common::s3_object_read::ObjectRead::Unreadable => {
                    DefinitionRead::Snapshotted(None)
                }
                // Nothing answered at all.
                ::service_common::s3_object_read::ObjectRead::Unanswered => {
                    DefinitionRead::Unanswered
                }
            };
        }
    };
    // The ceiling is applied to the size Amazon S3 REPORTS, before a byte is buffered,
    // so an oversized object costs no allocation; the check after the body is read is
    // the backstop for an object whose reported size was wrong.
    if object
        .content_length()
        .is_some_and(|length| length > MAX_DEFINITION_BYTES as i64)
    {
        return DefinitionRead::Snapshotted(None);
    }
    let Ok(body) = object.body.collect().await else {
        // The store served the object and its bytes did not arrive in full, so nothing
        // was established about the object itself.
        return DefinitionRead::Unanswered;
    };
    let bytes = body.into_bytes();
    if bytes.len() > MAX_DEFINITION_BYTES {
        return DefinitionRead::Snapshotted(None);
    }
    let Ok(content) = String::from_utf8(bytes.to_vec()) else {
        return DefinitionRead::Snapshotted(None);
    };
    // `WorkflowDefinition` is modelled with a minimum length of 1, so an object holding
    // no bytes holds no definition content to report.
    if content.is_empty() {
        return DefinitionRead::Snapshotted(None);
    }
    let schedule_configuration = declared_schedule(&content);
    DefinitionRead::Snapshotted(Some(DefinitionSnapshot {
        content,
        schedule_configuration,
    }))
}

/// The schedule a definition DECLARES: the cron expression its DAG-level schedule key
/// carries ([`SCHEDULE_KEYS`], at the document root or inside its [`DAG_KEY`] mapping),
/// resolved through Airflow's preset shorthands ([`CRON_PRESETS`]).
///
/// `None` when the definition declares no schedule, or declares one that is not a cron
/// expression at all: `ScheduleConfiguration$CronExpression` is a cron expression in
/// standard cron syntax, so a value that is not one is not reported as one.
pub fn declared_schedule(definition: &str) -> Option<ScheduleConfiguration> {
    let declared = declared_schedule_value(definition)?;
    Some(ScheduleConfiguration {
        cron_expression: Some(cron_expression(&declared)?),
    })
}

/// The cron expression a declared schedule value stands for: a preset's expression, or
/// the value itself when it is written as a cron expression — as many fields as cron has
/// ([`CRON_FIELD_COUNTS`]), each written the way a cron field is ([`is_cron_field`]).
/// Prose that happens to have as many words as cron has fields is therefore not reported
/// as a cron expression.
///
/// The value is reported as the definition writes it, only trimmed of the surrounding
/// whitespace YAML does not consider part of a scalar.
fn cron_expression(declared: &str) -> Option<String> {
    let declared = declared.trim();
    if let Some((_, expression)) = CRON_PRESETS.iter().find(|(preset, _)| *preset == declared) {
        return Some((*expression).to_string());
    }
    let fields: Vec<&str> = declared.split_whitespace().collect();
    (CRON_FIELD_COUNTS.contains(&fields.len()) && fields.iter().copied().all(is_cron_field))
        .then(|| declared.to_string())
}

/// Whether `field` is written as ONE cron field: taking its structure
/// ([`CRON_FIELD_SEPARATORS`]) apart leaves one value ([`is_cron_field_value`]) in every
/// position that structure marks out.
///
/// So a field made only of separators is not one: it marks out positions and leaves every
/// one of them empty, which is no value the field could be read as.
fn is_cron_field(field: &str) -> bool {
    field
        .split(|character| CRON_FIELD_SEPARATORS.contains(&character))
        .all(is_cron_field_value)
}

/// Whether `token` is ONE value a cron field position may carry: a number, a wildcard
/// ([`CRON_FIELD_WILDCARDS`]), one of the day-of-month qualifiers
/// ([`CRON_FIELD_QUALIFIERS`]) or one of the month and day-of-week names
/// ([`CRON_FIELD_NAMES`]).
///
/// A position carrying nothing at all carries no value, so an empty token is not one.
fn is_cron_field_value(token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    token.chars().all(|character| character.is_ascii_digit())
        || CRON_FIELD_WILDCARDS
            .iter()
            .chain(CRON_FIELD_QUALIFIERS)
            .chain(CRON_FIELD_NAMES)
            .any(|name| token.eq_ignore_ascii_case(name))
}

/// The raw value the definition's DAG-level schedule key carries, read out of the FIRST
/// YAML document in `definition`.
///
/// Driven EVENT BY EVENT off the parser's own scanner, pulling one event at a time, and
/// never through the loader: the loader COMPOSES a document by descending once per
/// nesting level, so a document nesting as deep as the content allows would drive its
/// recursion as deep, and it resolves an ALIAS by copying the anchored node, which is how
/// a few hundred bytes of YAML become gigabytes. This walk does neither — it builds no
/// tree, recurses nowhere, and an alias costs it one event like any other node — so what
/// it needs is the three bounds it carries: [`MAX_DEFINITION_BYTES`] on the content,
/// [`MAX_DEFINITION_EVENTS`] on how many events are read, and
/// [`MAX_DEFINITION_NESTING`] on how deep the walk follows a document.
///
/// A document that runs past either bound, or that the parser cannot read to the end, is
/// not followed further: the walk stops there and answers with the DAG-level declaration
/// it had already read, because a declaration is decided by the first schedule key the
/// walk reaches and nothing past the stopping point can change it. YAML 1.2 is a
/// superset of JSON, so a definition written as JSON is read by the same walk.
fn declared_schedule_value(definition: &str) -> Option<String> {
    use ::yaml_rust2::parser::Event;

    let mut parser = ::yaml_rust2::parser::Parser::new_from_str(definition);
    let mut finder = ScheduleFinder::default();
    for _ in 0..MAX_DEFINITION_EVENTS {
        let Ok((event, _marker)) = parser.next_token() else {
            break;
        };
        if matches!(event, Event::StreamEnd) || !finder.accept(event) {
            break;
        }
    }
    finder.found
}

/// One open YAML collection, as the walks below track it.
#[derive(Debug)]
struct Frame {
    /// Whether this collection is a mapping, so its children alternate key and value.
    mapping: bool,
    /// Whether the next node in this mapping stands in the KEY position.
    expecting_key: bool,
    /// The key the node now being read sits under, for a mapping.
    key: Option<String>,
}

/// The collections a walk has OPEN at one point, outermost first, and the bookkeeping
/// every walk over a definition needs: which key the node now being read sits under, and
/// how deep the walk is.
///
/// Both walks below are driven off one of these, so the key-and-value stepping a mapping
/// needs — including the two positions a collection or an alias can occupy without being
/// a scalar — is written once.
#[derive(Debug, Default)]
struct OpenNodes {
    open: Vec<Frame>,
}

impl OpenNodes {
    /// How many collections are open.
    fn depth(&self) -> usize {
        self.open.len()
    }

    /// Forget every open collection: the walk has reached another document.
    fn clear(&mut self) {
        self.open.clear();
    }

    /// Open one collection, charging the node it occupies to the collection around it.
    ///
    /// `false` for a document that nests past [`MAX_DEFINITION_NESTING`], which is the
    /// point past which these readers stop following it.
    fn push(&mut self, mapping: bool) -> bool {
        if self.open.len() >= MAX_DEFINITION_NESTING {
            return false;
        }
        self.consume_node_slot();
        self.open.push(Frame {
            mapping,
            expecting_key: mapping,
            key: None,
        });
        true
    }

    /// Close the innermost collection and report the depth it occupied, so a walk can
    /// recognise the collection it was tracking.
    fn pop(&mut self) -> Option<usize> {
        let index = self.open.len().checked_sub(1)?;
        self.open.pop();
        Some(index)
    }

    /// Whether the scalar now being read stands in the KEY position of the enclosing
    /// mapping, so it names what comes next rather than being a value.
    fn expects_key(&self) -> bool {
        self.open
            .last()
            .is_some_and(|frame| frame.mapping && frame.expecting_key)
    }

    /// Record `key` as the enclosing mapping's current key.
    fn set_key(&mut self, key: String) {
        if let Some(frame) = self.open.last_mut() {
            frame.key = Some(key);
            frame.expecting_key = false;
        }
    }

    /// Step the enclosing mapping on to its next key: the node that stood in the value
    /// position has been read.
    fn consumed_value(&mut self) {
        if let Some(frame) = self.open.last_mut() {
            if frame.mapping {
                frame.expecting_key = true;
            }
        }
    }

    /// The key the node at `depth` sits under: the current key of the collection at that
    /// depth, and `None` when it is not a mapping or carries a key this reader cannot
    /// name.
    fn key_at(&self, depth: usize) -> Option<&str> {
        self.open.get(depth)?.key.as_deref()
    }

    /// Whether the collection at `depth` is a mapping, and `None` when nothing is open
    /// that deep.
    fn is_mapping_at(&self, depth: usize) -> Option<bool> {
        Some(self.open.get(depth)?.mapping)
    }

    /// The keys the node now being read sits under, outermost first, and `None` when any
    /// collection in the chain is not a mapping or carries a key this reader cannot name.
    fn key_path(&self) -> Option<Vec<&str>> {
        let mut keys = Vec::with_capacity(self.open.len());
        for frame in &self.open {
            if !frame.mapping {
                return None;
            }
            keys.push(frame.key.as_deref()?);
        }
        Some(keys)
    }

    /// Account for a node that occupies a position in the enclosing mapping without being
    /// a scalar — a nested collection, or an alias standing in for one — so its key and
    /// value positions stay in step.
    ///
    /// Either position can hold one: YAML allows a collection or an alias as a KEY, and
    /// such a key is one this reader cannot name, so the mapping's key is cleared and the
    /// node that follows is read as its value.
    fn consume_node_slot(&mut self) {
        if let Some(frame) = self.open.last_mut() {
            if !frame.mapping {
                return;
            }
            if frame.expecting_key {
                frame.key = None;
                frame.expecting_key = false;
            } else {
                frame.expecting_key = true;
            }
        }
    }
}

/// The walk that finds a definition's declared schedule: it tracks which key each scalar
/// sits under and keeps the first value at a DAG-level schedule path.
#[derive(Debug, Default)]
struct ScheduleFinder {
    /// The collections open at this point.
    nodes: OpenNodes,
    /// How many documents have begun. Only the first document's declarations count: a
    /// definition file is one DAG declaration.
    documents: usize,
    /// The first declared schedule value seen, if any.
    found: Option<String>,
}

impl ScheduleFinder {
    /// Take ONE parser event, and report whether the walk may go on.
    fn accept(&mut self, event: ::yaml_rust2::parser::Event) -> bool {
        use ::yaml_rust2::parser::Event;
        match event {
            Event::DocumentStart => {
                self.documents += 1;
                self.nodes.clear();
            }
            Event::MappingStart(..) | Event::SequenceStart(..) => {
                if !self.nodes.push(matches!(event, Event::MappingStart(..))) {
                    return false;
                }
            }
            Event::MappingEnd | Event::SequenceEnd => {
                self.nodes.pop();
            }
            Event::Scalar(value, ..) => {
                if self.nodes.expects_key() {
                    self.nodes.set_key(value);
                    return true;
                }
                let declares_schedule =
                    self.documents == 1 && self.found.is_none() && self.at_schedule_path();
                self.nodes.consumed_value();
                if declares_schedule {
                    self.found = Some(value);
                }
            }
            // An alias refers to an anchored node. Nothing here expands one, so it only
            // occupies the position it stands in.
            Event::Alias(..) => self.nodes.consume_node_slot(),
            _ => {}
        }
        true
    }

    /// Whether the node now being read is the value of a DAG-level schedule key: the
    /// open collections are all mappings, and their keys spell either `<schedule key>`
    /// at the document root or `dag.<schedule key>` one level in.
    fn at_schedule_path(&self) -> bool {
        match self.nodes.key_path().as_deref() {
            Some([key]) => SCHEDULE_KEYS.contains(key),
            Some([DAG_KEY, key]) => SCHEDULE_KEYS.contains(key),
            _ => false,
        }
    }
}

/// ONE task a definition declares, exactly as the definition writes it: nothing here
/// resolves a dependency or checks an identifier, which is the plan's business
/// (`crate::state::workflow_execution::ExecutionPlan`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskDeclaration {
    /// The task's own identifier ([`TASK_ID_KEYS`]), and `None` for an entry this reader
    /// finds no identifier in — which is therefore not a task it can run.
    pub task_id: Option<String>,
    /// The operator the task runs under ([`TASK_OPERATOR_KEYS`]), when it names one.
    pub operator_name: Option<String>,
    /// The identifiers of the tasks this one waits for ([`TASK_DEPENDENCY_KEYS`]), in the
    /// order the definition lists them.
    pub dependencies: Vec<String>,
}

/// What a definition declares about its tasks.
#[derive(Debug, Clone, Default)]
pub struct DeclaredTasks {
    /// The declarations read, in the order the definition writes them.
    pub tasks: Vec<TaskDeclaration>,
    /// Whether the definition declares MORE tasks than [`MAX_DECLARED_TASKS`], in which
    /// case the list above is not all of them.
    pub past_ceiling: bool,
}

/// The tasks the FIRST document in `definition` declares at a DAG-level [`TASKS_KEY`]
/// collection, written either as a sequence of task entries or as a mapping from task
/// identifier to task entry.
///
/// Driven event by event off the parser's own scanner under the same three bounds the
/// schedule walk carries ([`MAX_DEFINITION_BYTES`] on the content, [`MAX_DEFINITION_EVENTS`]
/// on how many events are read and [`MAX_DEFINITION_NESTING`] on how deep the walk
/// follows a document), plus [`MAX_DECLARED_TASKS`] on how many declarations it keeps: it
/// builds no tree, recurses nowhere, and an alias costs it one event like any other node.
/// A document the parser cannot read to the end is not followed further, so the walk
/// answers with the declarations it had already read.
pub fn declared_tasks(definition: &str) -> DeclaredTasks {
    use ::yaml_rust2::parser::Event;

    let mut parser = ::yaml_rust2::parser::Parser::new_from_str(definition);
    let mut finder = TaskFinder::default();
    for _ in 0..MAX_DEFINITION_EVENTS {
        let Ok((event, _marker)) = parser.next_token() else {
            break;
        };
        if matches!(event, Event::StreamEnd) || !finder.accept(event) {
            break;
        }
    }
    DeclaredTasks {
        tasks: finder.found,
        past_ceiling: finder.past_ceiling,
    }
}

/// Which part of a task entry the scalar now being read declares.
#[derive(Debug, Clone, Copy)]
enum TaskField {
    /// The task's identifier.
    Id,
    /// The operator the task runs under.
    Operator,
    /// One task this one waits for.
    Dependency,
}

/// The part of a task entry the key `key` declares, and `None` for a key this reader does
/// not read (an operator's own configuration, say).
fn task_field(key: &str) -> Option<TaskField> {
    if TASK_ID_KEYS.contains(&key) {
        Some(TaskField::Id)
    } else if TASK_OPERATOR_KEYS.contains(&key) {
        Some(TaskField::Operator)
    } else if TASK_DEPENDENCY_KEYS.contains(&key) {
        Some(TaskField::Dependency)
    } else {
        None
    }
}

/// The walk that finds a definition's declared tasks: it tracks the DAG-level `tasks`
/// collection, reads one declaration per entry in it, and keeps the identifier, operator
/// and dependencies each entry declares.
#[derive(Debug, Default)]
struct TaskFinder {
    /// The collections open at this point.
    nodes: OpenNodes,
    /// How many documents have begun. Only the first document's declarations count.
    documents: usize,
    /// The depth of the DAG-level `tasks` collection, once the walk is inside one.
    tasks_at: Option<usize>,
    /// The entry now being read, and the depth of its own collection.
    building: Option<(usize, TaskDeclaration)>,
    /// The declarations read so far.
    found: Vec<TaskDeclaration>,
    /// Whether the walk stopped at [`MAX_DECLARED_TASKS`].
    past_ceiling: bool,
}

impl TaskFinder {
    /// Take ONE parser event, and report whether the walk may go on: `false` once the
    /// document nests past [`MAX_DEFINITION_NESTING`] or declares more tasks than
    /// [`MAX_DECLARED_TASKS`].
    fn accept(&mut self, event: ::yaml_rust2::parser::Event) -> bool {
        use ::yaml_rust2::parser::Event;
        match event {
            Event::DocumentStart => {
                self.documents += 1;
                self.nodes.clear();
                self.tasks_at = None;
                self.building = None;
            }
            Event::MappingStart(..) | Event::SequenceStart(..) => {
                let mapping = matches!(event, Event::MappingStart(..));
                let container = self.documents == 1 && self.tasks_at.is_none() && self.at_tasks_path();
                // A task entry is a mapping whose parent IS the tasks collection. In a
                // mapping of tasks the key it sits under names it; in a sequence of them
                // the entry declares its own identifier.
                let entry = mapping && !container && self.entering_entry();
                let named_by_key = entry
                    .then(|| self.tasks_at.and_then(|at| self.nodes.key_at(at)))
                    .flatten()
                    .map(str::to_string);
                if !self.nodes.push(mapping) {
                    return false;
                }
                let depth = self.nodes.depth() - 1;
                if container {
                    self.tasks_at = Some(depth);
                } else if entry {
                    if self.found.len() >= MAX_DECLARED_TASKS {
                        self.past_ceiling = true;
                        return false;
                    }
                    self.building = Some((
                        depth,
                        TaskDeclaration {
                            task_id: named_by_key,
                            ..TaskDeclaration::default()
                        },
                    ));
                }
            }
            Event::MappingEnd | Event::SequenceEnd => {
                let Some(depth) = self.nodes.pop() else {
                    return true;
                };
                if self.building.as_ref().is_some_and(|(at, _)| *at == depth) {
                    if let Some((_, task)) = self.building.take() {
                        self.found.push(task);
                    }
                }
                if self.tasks_at == Some(depth) {
                    self.tasks_at = None;
                }
            }
            Event::Scalar(value, ..) => {
                if self.nodes.expects_key() {
                    self.nodes.set_key(value);
                    return true;
                }
                let declares = self.declared_field();
                self.nodes.consumed_value();
                if let (Some(field), Some((_, task))) = (declares, self.building.as_mut()) {
                    match field {
                        // An explicit identifier overrides the key the entry sits under.
                        TaskField::Id => task.task_id = Some(value),
                        TaskField::Operator => task.operator_name = Some(value),
                        TaskField::Dependency => task.dependencies.push(value),
                    }
                }
            }
            Event::Alias(..) => self.nodes.consume_node_slot(),
            _ => {}
        }
        true
    }

    /// Whether the collection now being opened is a DAG-level `tasks` collection: the
    /// open collections are all mappings, and their keys spell either `tasks` at the
    /// document root or `dag.tasks` one level in.
    fn at_tasks_path(&self) -> bool {
        match self.nodes.key_path().as_deref() {
            Some([key]) => *key == TASKS_KEY,
            Some([DAG_KEY, key]) => *key == TASKS_KEY,
            _ => false,
        }
    }

    /// Whether the collection now being opened is an ENTRY of the tasks collection: one
    /// is being tracked, nothing deeper inside it is being read, and this node is its
    /// direct child.
    fn entering_entry(&self) -> bool {
        self.building.is_none()
            && self
                .tasks_at
                .is_some_and(|at| self.nodes.depth() == at + 1)
    }

    /// Which part of the entry being read the scalar now being read declares: a scalar in
    /// the entry's own mapping, under a key this reader knows, or one in a sequence that
    /// entry's dependency key carries.
    fn declared_field(&self) -> Option<TaskField> {
        let (at, _) = self.building.as_ref()?;
        let key = self.nodes.key_at(*at)?;
        if self.nodes.depth() == at + 1 {
            return task_field(key);
        }
        if self.nodes.depth() == at + 2
            && self.nodes.is_mapping_at(at + 1) == Some(false)
            && TASK_DEPENDENCY_KEYS.contains(&key)
        {
            return Some(TaskField::Dependency);
        }
        None
    }
}

