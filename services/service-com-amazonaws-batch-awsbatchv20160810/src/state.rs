// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-batch-awsbatchv20160810 service.
//!
//! Every implemented operation computes its answer from the records below: the
//! compute environments, job queues, job definition revisions, consumable
//! resources and jobs this service has been asked to create, keyed the way AWS
//! Batch keys them (by name, with an ARN index beside it so anything the model
//! documents as "name or ARN" resolves both ways).
//!
//! # The job lifecycle is driven by the CLOCK, never by a read
//!
//! A job AWS Batch accepts is returned in a non-terminal status (`SUBMITTED`),
//! so the moves out of it are owed by the clock: each one falls due
//! [`JOB_PHASE_MILLIS`] after the call (or the transition) that started it, and
//! [`ComAmazonawsBatchAwsbatchv20160810State::materialize_due`] only materialises
//! what is already due. A reporting operation (`DescribeJobs`, `ListJobs`, the
//! `Describe*` readers) calls it and therefore never changes what a later caller
//! sees: two reads at the same instant answer identically, and the transient
//! status is observable because the span is never zero.
//!
//! # Everything a caller can fill is BOUNDED
//!
//! Every map below is filled by a request anyone can repeat, so each one has a maximum
//! ([`limits`]): a create past the cap answers the modelled `ClientException`, and the
//! jobs store evicts the jobs that have been terminal longest before it refuses, so the
//! store cannot grow for the emulator's lifetime.
//!
//! A job's container is NOT run: this emulator has no container runtime, and the
//! engine contract (`vendor/engine/src/lib.rs`) forbids reporting a status,
//! duration or exit code for work nothing ran. `RUNNABLE` is where a job waits to be
//! placed on compute capacity, so what happens next is decided by the compute
//! environments behind the job's queue ([`ComAmazonawsBatchAwsbatchv20160810State::job_is_placeable`]):
//! a queue with no capacity of its own to offer — one whose environments are all
//! `UNMANAGED`, `DISABLED` or not created yet — leaves the job RESTING in `RUNNABLE`,
//! where only a caller ends it (`CancelJob`, `TerminateJob`, or the deletion of its
//! queue), while a queue whose MANAGED environment can hold it PLACES it one span later
//! and the placement RECORDS the job attempt it starts — one [`JobAttempt`] carrying the
//! ARN of the task the job was placed as — and then ends in the modelled failure state with
//! [`NO_CONTAINER_RUNTIME_REASON`], which is what that attempt ends with too. Either way no
//! `startedAt`, `stoppedAt` or `exitCode` is invented for a container nothing
//! ran, and no job is ever advanced to `STARTING` or `RUNNING`.
//! Everything around it — accepting, resolving, scheduling, placing, listing, cancelling,
//! terminating — is real.
//!
//! An ARRAY job's parent runs no container of its own, so it is never placed: its status
//! is what its children add up to
//! ([`ComAmazonawsBatchAwsbatchv20160810State::advance_array_parents`]).
//!
//! A SERVICE job's work is not run either, for the same reason: nothing here runs
//! SageMaker AI, so a service job `SubmitServiceJob` accepted walks the same
//! clock-owed chain (`SUBMITTED` -> `RUNNABLE`) and then ends in the modelled failure
//! state with [`NO_SERVICE_JOB_RUNTIME_REASON`], with no `scheduledAt`, `startedAt`,
//! attempt or capacity usage invented for it. A service job the service environments of
//! its queue publish no room for is never placed at all, so it RESTS in `RUNNABLE` until
//! a later call publishes the capacity it asked for
//! ([`ComAmazonawsBatchAwsbatchv20160810State::rearm_resting_service_jobs`]).
//!
//! # Every job state CHANGE is published to EventBridge
//!
//! AWS Batch notifies the account's default EventBridge bus as a job moves through
//! its statuses, which is the documented, polling-free way to observe a job. An event
//! reports a TRANSITION of a job the service already holds, so the initial status a
//! submit stores is not one: a job's event stream begins at its FIRST move out of that
//! initial status ("Events aren't created for the initial job submission" — the User
//! Guide's "Job state change events"). So every write of a job's status that MOVES it —
//! the moves the clock materialises, and the terminal status a cancel, a terminate or a
//! queue deletion writes — records a [`JobStateChange`] here, and the operation that
//! caused it drains the queue and publishes the changes once this store's lock is
//! released ([`publish_job_state_changes`]).

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

mod array_jobs;
mod placement;

pub use array_jobs::*;
pub use placement::*;

use crate::ops::types::{
    AttemptContainerDetail, AttemptDetail, CapacityLimit, ClientException, ComputeEnvironmentOrder,
    ComputeResource,
    ConsumableResourceProperties, ContainerDetail, ContainerOverrides, ContainerProperties,
    EcsProperties, EcsSettings, EksConfiguration, FairsharePolicy, JobDefinition, JobDependency,
    JobStateTimeLimitAction, JobTimeout, KeyValuePair, NodeProperties,
    QuotaShareCapacityLimit, QuotaSharePolicy, QuotaSharePreemptionConfiguration,
    QuotaShareResourceSharingConfiguration, ResourceRequirement, RetryStrategy, ServerException,
    ServiceEnvironmentOrder, ServiceJobPreemptionConfiguration, ServiceJobRetryStrategy,
    ServiceJobTimeout, ShareAttributes, UpdatePolicy,
};

/// This service's ARN namespace and sigv4 signing name (`OPERATIONS.md`).
pub const ARN_SERVICE: &str = "batch";

/// The region used when a request carries no SigV4 credential scope to read one
/// from. Every emulator defaults to `us-east-1`.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long one clock-owed transition takes: a compute environment or job queue
/// leaving `CREATING`, a compute environment or job queue leaving `UPDATING`, and
/// each move a submitted job makes.
///
/// A span chosen by this emulator (AWS documents no duration for it), inside the
/// 250–750 ms band the project fixes for a transition the clock owes. It is never
/// zero, so a caller that reads straight after a create still observes `CREATING`,
/// and a caller that terminates straight after a submit still observes `SUBMITTED`.
pub const JOB_PHASE_MILLIS: i64 = 500;

/// What this emulator will store for one caller, and the answer past it.
///
/// Every map in [`ComAmazonawsBatchAwsbatchv20160810State`] is filled by requests an
/// unauthenticated caller can repeat, so each one carries a maximum: a create past the
/// cap answers the modelled `ClientException` (HTTP 400), the way AWS Batch answers a
/// request past a service quota, and the jobs store additionally evicts the jobs that
/// have been terminal longest (nothing a later call can move again), so a long-running
/// emulator cannot accumulate records without bound. The indexes beside those maps are
/// bounded by them: the due index and the dependency index hold only entries for jobs the
/// store still holds, and an entry goes when either end of it does.
pub mod limits {
    /// AWS Batch service quota "Compute environments per Region": 50 (adjustable).
    pub const MAX_COMPUTE_ENVIRONMENTS: usize = 50;

    /// AWS Batch service quota "Job queues per Region": 50 (adjustable).
    pub const MAX_JOB_QUEUES: usize = 50;

    /// Service environments kept at once. An EMULATOR bound (AWS publishes no quota for
    /// this resource): `CreateServiceEnvironment` fills the store and anyone can repeat
    /// it, so a create past this answers the modelled `ClientException`. Sized like the
    /// other per-Region, name-keyed Batch resources above.
    pub const MAX_SERVICE_ENVIRONMENTS: usize = 50;

    /// Dependencies one submit may name. AWS Batch documents it on `SubmitJob$dependsOn`:
    /// "A job can depend upon a maximum of 20 jobs."
    pub const MAX_JOB_DEPENDENCIES: usize = 20;

    /// Jobs one bulk `CancelJobs`/`TerminateJobs` request may name, from both operations'
    /// own docs: "An array of up to 50 Batch job IDs of the jobs to cancel" /
    /// "...to terminate". Both members target a plain string list, so the bound is applied
    /// in the handlers.
    pub const MAX_BULK_JOBS: usize = 50;

    /// Jobs kept at once. An emulator bound, as above: a submit that would exceed it
    /// first evicts the jobs that have been terminal longest, and is refused only when
    /// there is nothing evictable left.
    pub const MAX_JOBS: usize = 500;

    /// Consumable resources kept at once. An EMULATOR-CHOSEN ceiling, not an AWS quota:
    /// `CreateConsumableResource` is a request anyone can repeat, so the store answers the
    /// modelled `ClientException` past this point rather than growing without bound.
    pub const MAX_CONSUMABLE_RESOURCES: usize = 500;

    /// How many `UpdateConsumableResource` client tokens this store remembers at once, so
    /// an identical repeat of one resolves to the update it already applied
    /// ([`super::ComAmazonawsBatchAwsbatchv20160810State::consumable_resource_update_for_token`]).
    /// An EMULATOR bound: the cache is a replay index anyone can fill, so the oldest entry
    /// is dropped past this point rather than letting the store grow without bound.
    pub const MAX_CONSUMABLE_RESOURCE_UPDATE_TOKENS: usize = 500;

    /// Quota shares kept at once. An EMULATOR bound (AWS publishes no quota for this
    /// resource): `CreateQuotaShare` fills the store and anyone can repeat it, so a create
    /// past this answers the modelled `ClientException`. Sized like the other per-Region
    /// Batch resources above.
    pub const MAX_QUOTA_SHARES: usize = 50;

    /// Service jobs kept at once, the same emulator bound as [`MAX_JOBS`] and with the
    /// same eviction-before-refusal rule
    /// ([`super::ComAmazonawsBatchAwsbatchv20160810State::insert_service_job`]).
    pub const MAX_SERVICE_JOBS: usize = 500;

    /// How many bytes of `SubmitServiceJob$serviceRequestPayload` this service stores for
    /// one service job. An EMULATOR bound (AWS publishes none for the member): the payload
    /// is a caller-supplied string kept on the record and read once to derive what capacity
    /// the job asks for ([`super::requested_capacity`]), so a submit past this answers the
    /// modelled `ClientException` rather than letting one request decide how much is stored
    /// and parsed. 1 MiB, which is far more than a SageMaker training request needs.
    pub const MAX_SERVICE_REQUEST_PAYLOAD_BYTES: usize = 1024 * 1024;

    /// How many instance groups of one service job's request are weighed against a service
    /// environment's capacity ([`super::requested_capacity`]). An EMULATOR bound: the
    /// request chooses how many groups it names, so the derived demand is capped here
    /// instead, which is what keeps the admission check constant-sized per record.
    pub const MAX_REQUESTED_INSTANCE_GROUPS: usize = 16;

    /// How many `capacityLimits` entries ONE service environment publishes. An EMULATOR
    /// bound (AWS publishes no quota and the model declares no length): the list is filled
    /// by `CreateServiceEnvironment`/`UpdateServiceEnvironment` and is walked whenever a
    /// service job is weighed against the environment, so a request past this answers the
    /// modelled `ClientException`.
    pub const MAX_SERVICE_ENVIRONMENT_CAPACITY_LIMITS: usize = 20;

    /// How many `serviceEnvironmentOrder` entries ONE job queue carries, bounded for the
    /// same reason as the capacity limits above: the list is caller-filled and is walked
    /// when a service job of the queue is placed.
    pub const MAX_SERVICE_ENVIRONMENT_ORDER_ENTRIES: usize = 10;

    /// Scheduling policies kept at once. An EMULATOR bound (AWS publishes no quota for
    /// this resource): `CreateSchedulingPolicy` fills the store and anyone can repeat it,
    /// so a create past this answers the modelled `ClientException`. Sized like the other
    /// per-Region, name-keyed Batch resources above.
    pub const MAX_SCHEDULING_POLICIES: usize = 50;

    /// How many tags ONE resource carries at most.
    ///
    /// The model's own bound: every tag map on this service — `TagResource$tags`,
    /// `ListTagsForResource$tags`, `CreateJobQueue$tags`, … — targets
    /// `com.amazonaws.batch#TagrisTagsMap`, declared `@length(min: 1, max: 50)`
    /// (`smithy_query(query="shape", shape="TagrisTagsMap")`). A resource holding more
    /// than this could not be reported through that shape at all, so a `TagResource`
    /// whose merge would take a resource past it is refused instead.
    pub const MAX_TAGS_PER_RESOURCE: usize = 50;

    /// Job ids ONE `DescribeJobs` call may name. AWS documents it on the member:
    /// `DescribeJobsRequest$jobs` is "A list of up to 100 job IDs"
    /// (`smithy_query(query="doc", shape="DescribeJobsRequest", member="jobs")`); the
    /// model types it as a plain `StringList`, so the request validator admits any length
    /// and the handler applies the bound.
    pub const MAX_DESCRIBE_JOBS: usize = 100;
}

/// What AWS Batch answers a `DescribeJobs` naming more than
/// [`limits::MAX_DESCRIBE_JOBS`] job ids with, inside its frontend envelope.
///
/// The spacing around the maximum is AWS's own (the recorded detail reads
/// `Maximum number of jobs supported is  100 .`): the number is interpolated into a
/// sentence that already carries the separators.
pub fn describe_jobs_bound_message() -> String {
    format!(
        "Maximum number of jobs supported is  {} .",
        limits::MAX_DESCRIBE_JOBS
    )
}

/// The `ClientException` message a request past one of [`limits`] answers with.
pub fn quota_message(resource: &str, maximum: usize) -> String {
    format!("Too many {resource}: this emulator stores at most {maximum}")
}

/// The detail the tagging layer refuses a request whose RESULTANT tag set would leave one
/// resource holding more than [`limits::MAX_TAGS_PER_RESOURCE`] user tags with.
///
/// One fixed sentence for the one rule, carrying the maximum and nothing from the request:
/// the layer states the ceiling it holds a resource to, not how many entries the call
/// carried. It answers BOTH ways that ceiling is reached — a request whose own `tags` map
/// is longer than the declared length of `TagrisTagsMap`, and a merge into tags a resource
/// already holds that would take it past the ceiling — so the two cannot drift into two
/// sentences.
pub fn tag_set_limit_message() -> String {
    format!(
        "The resultant tag set must not have more than {} user tags.",
        limits::MAX_TAGS_PER_RESOURCE
    )
}

/// The longest `TagKey` the model admits: `com.amazonaws.batch#TagKey` is
/// `@length(min: 1, max: 128)` (`smithy_query(query="shape", shape="TagrisTagsMap")`).
pub const MAX_TAG_KEY_LEN: usize = 128;

/// The longest `TagValue` the model admits, from the same shape:
/// `@length(min: 0, max: 256)` — so an EMPTY tag value is legal.
pub const MAX_TAG_VALUE_LEN: usize = 256;

/// Whether every entry of `tags` fits the bounds the model declares for a tag KEY and a
/// tag VALUE; the refusal detail naming the first entry that does not otherwise.
///
/// The boundary walk enforces the tag MAP's own length (at most
/// [`limits::MAX_TAGS_PER_RESOURCE`] entries in one request) but not the bounds of the
/// key and value shapes INSIDE it, so the service enforces those rather than storing a
/// key or value it could never report back through `TagrisTagsMap` again. The entries are
/// examined in key order, so one request always answers with the same entry.
pub fn validate_tags(tags: &HashMap<String, String>) -> Result<(), String> {
    let ordered: BTreeMap<&str, &str> = tags
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    for (key, value) in ordered {
        validate_tag_key(key)?;
        let length = value.chars().count();
        if length > MAX_TAG_VALUE_LEN {
            return Err(format!(
                "The value of tag key {key} is {length} characters: a tag value is at most \
                 {MAX_TAG_VALUE_LEN} characters"
            ));
        }
    }
    Ok(())
}

/// [`validate_tags`] for the keys alone, which is what `UntagResource` carries; examined
/// in the order the request listed them.
pub fn validate_tag_keys(keys: &[String]) -> Result<(), String> {
    keys.iter().try_for_each(|key| validate_tag_key(key))
}

/// Whether one tag key fits `TagKey`'s declared bounds.
fn validate_tag_key(key: &str) -> Result<(), String> {
    let length = key.chars().count();
    if length == 0 || length > MAX_TAG_KEY_LEN {
        return Err(format!(
            "The tag key {key} is {length} characters: a tag key is 1 to {MAX_TAG_KEY_LEN} \
             characters"
        ));
    }
    Ok(())
}

/// What `DescribeQuotaShare` answers for a `quotaShareArn` that is not a quota-share ARN
/// at all — the recorded detail, which states the pattern requirement without echoing the
/// value or spelling the pattern out.
pub const QUOTA_SHARE_ARN_NOT_VALID: &str =
    "Quota Share arn should match a valid Quota Share arn pattern";

/// The rule a scheduling-policy identifier that is not a scheduling-policy ARN at all
/// ([`parse_scheduling_policy_arn`]) broke: a valid pattern governs it. The pattern itself
/// is not spelled out, so the response publishes no shape a client could then encode.
pub const SCHEDULING_POLICY_ARN_NOT_VALID: &str =
    "Scheduling policy arn should match a valid Scheduling policy arn pattern";

/// That refusal as a scheduling-policy operation answers it: the rule above, then the
/// identifier the request submitted, verbatim.
///
/// The offending value is what makes the refusal actionable — `DescribeSchedulingPolicies`
/// takes a LIST of identifiers, so a refusal that states the rule alone leaves the caller
/// unable to tell which entry broke it — and it is the sentence's only variable part.
///
/// Answered BARE: Batch's frontend envelope ([`client_exception_enveloped`]) belongs to the
/// refusal classes AWS is recorded to wrap, and this identifier-shape class is not one of
/// them, so the message carries the sentence alone and no request id of its own (the
/// response still reports one in the header AWS carries it in,
/// `crate::service::request_id_header`).
///
/// The two EVIDENCED parts are the rule text and that the value refused comes from the
/// request; the `": "` joining them is this crate's choice, because the recorded refusal of
/// this class is asserted by shape rather than by value. A later recording that pins the
/// exact form changes this one expression and nothing else.
pub fn scheduling_policy_arn_not_valid(reference: &str) -> ClientException {
    client_exception(format!("{SCHEDULING_POLICY_ARN_NOT_VALID}: {reference}"))
}

/// What `DescribeSchedulingPolicies` answers for a request whose `arns` names nothing.
///
/// The member is `@required` (`smithy_query(query="operation",
/// operation="DescribeSchedulingPolicies")`) because it carries the identity of what to
/// describe — unlike the optional filter list of `DescribeComputeEnvironments`,
/// `DescribeJobQueues` and `DescribeServiceEnvironments`, where naming nothing means
/// "every one of them". A list with no entries therefore identifies nothing, and is
/// refused instead of answered with an empty result.
pub const SCHEDULING_POLICY_ARNS_REQUIRED: &str =
    "arns must name at least one scheduling policy";

/// What `DeleteSchedulingPolicy` answers for a policy a job queue still names:
/// `DeleteSchedulingPolicy`'s own doc — "You can't delete a scheduling policy that's used
/// in any job queues."
pub const SCHEDULING_POLICY_IN_USE: &str =
    "Cannot delete a scheduling policy that's used in any job queues.";

/// What AWS Batch answers when a `Create*` names an object it already holds: the bare
/// detail `CreateComputeEnvironment` is recorded to state for a duplicate name, with no
/// resource name interpolated into it.
pub const OBJECT_ALREADY_EXISTS: &str = "Object already exists";

/// The HTTP status AWS Batch answers a NAME COLLISION with.
///
/// The model gives `ClientException` `httpError` 400, which is the status a validation
/// refusal carries; a `Create*` that names an object the service already holds is a
/// CONFLICT, and the frontend answers the same error shape with HTTP 409 — recorded for a
/// duplicate `CreateSchedulingPolicy`. The status is the only thing that differs, so each
/// collision refusal is built from [`OBJECT_ALREADY_EXISTS`] exactly as before and carried
/// on `crate::scaffolded::Scaffolded::modeled_with_status`.
pub const CONFLICT_STATUS: u16 = 409;

/// What `CreateComputeEnvironment` answers when an `UNMANAGED` environment is created
/// with Fargate compute resources — the recorded detail, stated as the rule rather than
/// naming the environment.
///
/// `computeResources` is "details of the compute resources MANAGED by the compute
/// environment", and Fargate capacity exists only where Batch manages it, so the pair
/// `UNMANAGED` + a Fargate `CRType` describes nothing this service can create.
pub const UNMANAGED_FARGATE_NOT_CREATABLE: &str =
    "Cannot create an UNMANAGED Fargate Compute Environment.";

/// The `CRType` values whose compute resources run on Fargate, from the model's own
/// `com.amazonaws.batch#CRType` enum (`EC2 | SPOT | FARGATE | FARGATE_SPOT |
/// ECS_MANAGED_INSTANCES`), of which these two are the Fargate pair.
pub const FARGATE_COMPUTE_TYPES: &[&str] = &["FARGATE", "FARGATE_SPOT"];

/// Whether a compute resource of this `CRType` runs on Fargate
/// ([`FARGATE_COMPUTE_TYPES`]).
pub fn is_fargate_compute_type(compute_type: &str) -> bool {
    FARGATE_COMPUTE_TYPES.contains(&compute_type)
}

/// What `CreateJobQueue` answers when its `computeEnvironmentOrder` names a compute
/// environment it cannot attach — the bare detail AWS is recorded to state, which names
/// the requirement rather than the environment.
pub const COMPUTE_ENVIRONMENTS_NOT_ATTACHABLE: &str =
    "Compute Environments must be created and valid before attaching them to a job queue";

/// What `CreateJobQueue`/`UpdateJobQueue` answer when a `serviceEnvironmentOrder` entry
/// names a service environment they cannot attach — the recorded detail, which (like
/// [`COMPUTE_ENVIRONMENTS_NOT_ATTACHABLE`], its compute-environment twin) states the
/// precondition instead of naming the entry, so the one refusal covers an environment
/// that was never created and one that is not valid.
pub const SERVICE_ENVIRONMENTS_NOT_ATTACHABLE: &str =
    "Service environments must be created and valid before attaching them to a job queue.";

/// What `DescribeJobDefinitions` answers for a `nextToken` it did not issue — the bare
/// detail AWS is recorded to state.
pub const NEXT_TOKEN_NOT_PROCESSABLE: &str = "Error processing next token";

/// What a `Delete*` answers for a resource whose state still accepts work: the fixed
/// sentence for the rule — the KIND of resource that cannot be deleted while enabled, and
/// the state to set it to first.
///
/// AWS states the RULE here and nothing from the request: no name, no ARN and no state
/// value read off the record, because a resource that is not [`STATE_DISABLED`] is by the
/// model's own `CEState`/`JQState` enums exactly an enabled one. Recorded in this form for
/// `DeleteComputeEnvironment`, and answered by every delete of this service that requires
/// the same precondition (`DeleteJobQueue`, `DeleteServiceEnvironment`), each naming its
/// own kind.
pub fn delete_enabled_message(kind: &str) -> String {
    format!("Cannot delete an enabled {kind}, set the state to {STATE_DISABLED} first.")
}

/// What `DeleteQuotaShare` answers for a share whose state still accepts submissions.
///
/// The same RULE as [`delete_enabled_message`] — a resource that still accepts work must be
/// disabled before it can be deleted — in the sentence the quota-share API states it with,
/// which is the one AWS is recorded to answer here. It carries nothing from the request:
/// a share that is not [`STATE_DISABLED`] is by the model's own `QuotaShareState` enum
/// exactly an enabled one.
pub const QUOTA_SHARE_MUST_BE_DISABLED: &str = "Quota Share must be DISABLED before deleting.";

/// What `UpdateComputeEnvironment` answers for a request carrying `computeResources` for
/// an environment Batch does not manage: the fixed sentence for the rule, naming the
/// environment by the ARN it is stored under.
///
/// `computeResources` is "details of the compute resources MANAGED by the compute
/// environment", so an environment whose type is not [`CE_TYPE_MANAGED`] has none to
/// update. The
/// one value the sentence carries is the resource's ARN — the identity AWS names a
/// compute environment by in a refusal, whichever of name or ARN the request used to
/// reach it.
pub fn unmanaged_compute_resources_message(compute_environment_arn: &str) -> String {
    format!(
        "Cannot update, compute resources not supported on unmanaged compute environment \
         {compute_environment_arn}."
    )
}

/// What `UpdateServiceJob` answers for a service job that has already completed — the
/// bare detail AWS is recorded to state, which names the precondition rather than the
/// job, so the one refusal covers either terminal `ServiceJobStatus`
/// ([`is_terminal_service_job_status`]).
pub const SERVICE_JOB_COMPLETED_NOT_UPDATABLE: &str =
    "UpdateServiceJob cannot be called on a service job which is completed.";

/// `statusReason` of a job the deletion of its job queue ended.
///
/// `DeleteJobQueue`'s own doc: "All jobs in the queue are eventually terminated when you
/// delete a job queue."
pub const JOB_QUEUE_DELETED_REASON: &str = "The job queue this job was submitted to was deleted";

/// `statusReason` of a service job the deletion of its quota share ended.
///
/// `DeleteQuotaShare`'s own doc: "All jobs in the share are eventually terminated when you
/// delete a quota share." Worded as [`JOB_QUEUE_DELETED_REASON`] is, naming the resource
/// whose removal ended the job.
pub const QUOTA_SHARE_DELETED_REASON: &str =
    "The quota share this job was submitted to was deleted";

/// `statusReason` of a service job whose work nothing here dispatched.
///
/// A service job's work runs on another service (`ServiceJobType` is
/// `SAGEMAKER_TRAINING`), which this emulator does not run and the engine contract
/// (`vendor/engine/src/lib.rs`) forbids pretending to: so the run the caller asked for
/// ends in the modelled failure state saying exactly that, and no `scheduledAt`,
/// `startedAt`, attempt or capacity usage is invented for it.
pub const NO_SERVICE_JOB_RUNTIME_REASON: &str =
    "This emulator does not run SageMaker AI, so the service job was never dispatched";

/// `statusReason` of a job released by a dependency that failed.
pub const DEPENDENT_JOB_FAILED_REASON: &str = "Dependent Job failed";

/// `statusReason` of a job none of whose dependencies the service can resolve any more.
///
/// `SubmitJob` refuses a `dependsOn` entry naming a job that does not exist, and eviction
/// keeps a dependency a live job still waits on, so nothing reachable through the API
/// produces this. It exists so a job that COULD not be released can still reach a
/// terminal status instead of occupying the store for ever.
pub const DEPENDENCY_UNRESOLVABLE_REASON: &str =
    "A job this job depends on is no longer known to the service";

/// Message of the `ServerException` a handler answers when the engine does not
/// hand back this service's store.
pub const UNREACHABLE_STATE: &str = "the Batch emulator could not reach its service state";

// Status/state literals. The values come from the model's own enums
// (`C_E_STATUS_VALUES`, `J_Q_STATUS_VALUES`, `JOB_STATUS_VALUES`, `CEState`/`JQState`).
/// `CEStatus`/`JQStatus` value a resource carries while its creation is pending.
pub const STATUS_CREATING: &str = "CREATING";
/// `CEStatus`/`JQStatus` value a resource carries while an update to it is pending
/// (`UpdateComputeEnvironment`, `UpdateJobQueue`); the move back to [`STATUS_VALID`]
/// is owed by the clock, like the one out of [`STATUS_CREATING`].
pub const STATUS_UPDATING: &str = "UPDATING";
/// `CEStatus`/`JQStatus` value of a created resource.
pub const STATUS_VALID: &str = "VALID";
/// `CEStatus`/`JQStatus` value a resource carries while the deletion a `Delete*` call
/// started is still owed by the clock.
pub const STATUS_DELETING: &str = "DELETING";
/// `CEState`/`JQState` default: a resource is created enabled unless asked otherwise.
pub const STATE_ENABLED: &str = "ENABLED";
/// `CEState`/`JQState` of a resource that accepts no new work, and the state a resource
/// must carry before it can be deleted (`DeleteComputeEnvironment`: "you must set its
/// state to DISABLED"; `DeleteJobQueue`: submissions must first be disabled with
/// `UpdateJobQueue`). `JQState`'s own doc: "If the job queue state is `DISABLED`, new
/// jobs can't be added to the queue, but jobs already in the queue can finish."
pub const STATE_DISABLED: &str = "DISABLED";
/// `JobDefinition$status` of a revision `RegisterJobDefinition` created and nothing has
/// deregistered.
pub const STATUS_ACTIVE: &str = "ACTIVE";

/// `JobDefinition$status` of a deregistered revision. `DescribeJobDefinitions`'
/// `status` filter is matched against stored status values, and
/// `DeregisterJobDefinition` is what sets this one.
pub const STATUS_INACTIVE: &str = "INACTIVE";

/// `OrchestrationType` of a job definition whose work runs on Amazon ECS — the default,
/// per the `containerOrchestrationType` doc ("The valid values are `ECS` (default) or
/// `EKS`").
pub const ORCHESTRATION_TYPE_ECS: &str = "ECS";
/// `OrchestrationType` of a job definition whose work runs on Amazon EKS.
pub const ORCHESTRATION_TYPE_EKS: &str = "EKS";

/// `PlatformCapability` for jobs on Amazon EC2 — the default when the request names none
/// (`platformCapabilities` doc: "If no value is specified, it defaults to EC2").
pub const PLATFORM_CAPABILITY_EC2: &str = "EC2";
/// `PlatformCapability` for jobs on Fargate.
pub const PLATFORM_CAPABILITY_FARGATE: &str = "FARGATE";
/// `PlatformCapability` for jobs on Amazon ECS Managed Instances.
pub const PLATFORM_CAPABILITY_MANAGED_INSTANCES: &str = "MANAGED_INSTANCES";

/// The longest `jobDefinitionName` the service accepts, from the member's doc: "It can be
/// up to 128 letters long. It can contain uppercase and lowercase letters, numbers,
/// hyphens (-), and underscores (_)." The model types the member as a plain `String`, so
/// the framework cannot refuse a bad one.
pub const MAX_JOB_DEFINITION_NAME_LEN: usize = 128;

/// The longest `serviceEnvironmentName` the service accepts, from the member's doc: "The
/// name for the service environment. It can be up to 128 characters long and can contain
/// letters, numbers, hyphens (-), and underscores (_)." The model types the member as a
/// plain `String`, so the framework cannot refuse a bad one.
pub const MAX_SERVICE_ENVIRONMENT_NAME_LEN: usize = 128;

/// The longest `quotaShareName` the service accepts, from the member's doc: "The name of
/// the quota share. It can be up to 128 characters long. It can contain uppercase and
/// lowercase letters, numbers, hyphens (-), and underscores (_)." The model types the
/// member as a plain `String`, so the framework cannot refuse a bad one.
pub const MAX_QUOTA_SHARE_NAME_LEN: usize = 128;

/// The longest `jobName` the service accepts, from the member's doc (`SubmitJob$jobName`,
/// `SubmitServiceJob$jobName`): "It can be up to 128 characters long. It can contain
/// uppercase and lowercase letters, numbers, hyphens (-), and underscores (_)." The model
/// types the member as a plain `String`, so the framework cannot refuse a bad one.
pub const MAX_JOB_NAME_LEN: usize = 128;

/// The fewest retry attempts this service accepts, from the member docs of BOTH retry
/// strategies — `RetryStrategy$attempts` ("The number of times to move a job to the
/// `RUNNABLE` status. You can specify between 1 and 10 attempts.") and
/// `ServiceJobRetryStrategy$attempts` ("You can specify between 1 and 10 attempts.").
/// Both model the member as a plain `Integer`, so the bound is applied in the handlers.
pub const MIN_RETRY_ATTEMPTS: i32 = 1;
/// The most retry attempts this service accepts; see [`MIN_RETRY_ATTEMPTS`].
pub const MAX_RETRY_ATTEMPTS: i32 = 10;

/// The `ResourceType` whose `ResourceRequirement$value` is a vCPU count.
pub const RESOURCE_TYPE_VCPU: &str = "VCPU";
/// The `ResourceType` whose `ResourceRequirement$value` is a memory hard limit in MiB.
pub const RESOURCE_TYPE_MEMORY: &str = "MEMORY";

/// The vCPU/memory pairs Fargate supports, as `ResourceRequirement$value`'s own doc
/// enumerates them under `type="VCPU"`: one entry per supported vCPU count ("The supported
/// values are 0.25, 0.5, 1, 2, 4, 8, 16, and 32"), carrying the `MEMORY` values that doc
/// lists for it, in MiB.
///
/// Source: `smithy_query(query="doc", shape="ResourceRequirement", member="value")`. The
/// member is a plain `String` on both sides, so neither the values nor the pairing can be
/// enforced at the boundary.
const FARGATE_VCPU_MEMORY: &[(f64, &[i64])] = &[
    (0.25, &[512, 1024, 2048]),
    (0.5, &[1024, 2048, 3072, 4096]),
    (1.0, &[2048, 3072, 4096, 5120, 6144, 7168, 8192]),
    (
        2.0,
        &[
            4096, 5120, 6144, 7168, 8192, 9216, 10240, 11264, 12288, 13312, 14336, 15360, 16384,
        ],
    ),
    (
        4.0,
        &[
            8192, 9216, 10240, 11264, 12288, 13312, 14336, 15360, 16384, 17408, 18432, 19456,
            20480, 21504, 22528, 23552, 24576, 25600, 26624, 27648, 28672, 29696, 30720,
        ],
    ),
    (
        8.0,
        &[
            16384, 20480, 24576, 28672, 32768, 36864, 40960, 45056, 49152, 53248, 57344, 61440,
        ],
    ),
    (
        16.0,
        &[
            32768, 40960, 49152, 57344, 65536, 73728, 81920, 90112, 98304, 106496, 114688, 122880,
        ],
    ),
    (32.0, &[61440, 122880, 249856]),
];

/// The fewest results `ListServiceJobs$maxResults` accepts, from the member's doc: "This
/// value can be between 1 and 100." The model types it as a plain `Integer`, so the bound
/// is applied in the handler.
pub const MIN_LIST_SERVICE_JOBS_RESULTS: i32 = 1;
/// The most results `ListServiceJobs$maxResults` accepts; see
/// [`MIN_LIST_SERVICE_JOBS_RESULTS`].
pub const MAX_LIST_SERVICE_JOBS_RESULTS: i32 = 100;

/// The fewest results `ListConsumableResources$maxResults` accepts, from the member's doc:
/// "This value can be between 1 and 100." The model types it as a plain `Integer`, so the
/// bound is applied in the handler.
pub const MIN_LIST_CONSUMABLE_RESOURCES_RESULTS: i32 = 1;
/// The most results `ListConsumableResources$maxResults` accepts; see
/// [`MIN_LIST_CONSUMABLE_RESOURCES_RESULTS`]. Also the page size of a request that names
/// none, from the same doc: "If this parameter isn't used, then `ListConsumableResources`
/// returns up to 100 results and a `nextToken` value if applicable."
pub const MAX_LIST_CONSUMABLE_RESOURCES_RESULTS: i32 = 100;

/// The fewest results `ListJobsByConsumableResource$maxResults` accepts, from the member's
/// doc: "This value can be between 1 and 100." The model types it as a plain `Integer`, so
/// the bound is applied in the handler.
pub const MIN_LIST_JOBS_BY_CONSUMABLE_RESOURCE_RESULTS: i32 = 1;
/// The most results `ListJobsByConsumableResource$maxResults` accepts; see
/// [`MIN_LIST_JOBS_BY_CONSUMABLE_RESOURCE_RESULTS`]. Also the page size of a request that
/// names none, from the same doc: "If this parameter isn't used, then
/// `ListJobsByConsumableResource` returns up to 100 results and a `nextToken` value if
/// applicable."
pub const MAX_LIST_JOBS_BY_CONSUMABLE_RESOURCE_RESULTS: i32 = 100;

/// The most `ListJobsByConsumableResource$filters` entries this emulator applies to one
/// request, and the most values any one of them may carry.
///
/// EMULATOR-CHOSEN ceilings, not AWS quotas (the model constrains the list's length
/// nowhere and the member's doc states no limit): every entry costs one selection pass
/// over the jobs selected so far, and every value in it costs one comparison per job, so
/// these two are what bound the work — entries x values x stored jobs — one request can
/// ask this handler for. The operation documents two filter names, so a request past
/// either ceiling is answering nothing a caller could mean; it is refused with the
/// modelled `ClientException` before any pass runs, rather than silently trimmed, which
/// would answer a narrowing the caller did not get.
pub const MAX_LIST_JOBS_BY_CONSUMABLE_RESOURCE_FILTERS: usize = 10;
/// See [`MAX_LIST_JOBS_BY_CONSUMABLE_RESOURCE_FILTERS`].
pub const MAX_LIST_JOBS_BY_CONSUMABLE_RESOURCE_FILTER_VALUES: usize = 100;

/// The fewest results `ListQuotaShares$maxResults` accepts, from the member's doc: "This
/// value can be between 1 and 100." The model types it as a plain `Integer`, so the bound
/// is applied in the handler.
pub const MIN_LIST_QUOTA_SHARES_RESULTS: i32 = 1;
/// The most results `ListQuotaShares$maxResults` accepts; see
/// [`MIN_LIST_QUOTA_SHARES_RESULTS`]. Also the page size of a request that names none, from
/// the same doc: "If this parameter isn't used, `ListQuotaShares` returns up to 100 results
/// and a `nextToken` value if applicable."
pub const MAX_LIST_QUOTA_SHARES_RESULTS: i32 = 100;

/// The fewest results `ListSchedulingPolicies$maxResults` accepts, from the member's doc:
/// "This value can be between 1 and 100." The model types it as a plain `Integer`, so the
/// bound is applied in the handler.
pub const MIN_LIST_SCHEDULING_POLICIES_RESULTS: i32 = 1;
/// The most results `ListSchedulingPolicies$maxResults` accepts; see
/// [`MIN_LIST_SCHEDULING_POLICIES_RESULTS`]. Also the page size of a request that names
/// none, from the same doc: "If this parameter isn't used, `ListSchedulingPolicies`
/// returns up to 100 results and a `nextToken` value if applicable."
pub const MAX_LIST_SCHEDULING_POLICIES_RESULTS: i32 = 100;

/// The longest scheduling policy `name` the service accepts, from the member's doc: "The
/// name of the fair-share scheduling policy. It can be up to 128 letters long. It can
/// contain uppercase and lowercase letters, numbers, hyphens (-), and underscores (_)."
/// The model types the member as a plain `String`, so the framework cannot refuse a bad
/// one.
pub const MAX_SCHEDULING_POLICY_NAME_LEN: usize = 128;

/// The bounds of `FairsharePolicy$shareDecaySeconds`, from the member's own doc: "A value
/// of zero (0) indicates the default minimum time window (600 seconds). The maximum
/// supported value is 604800 (1 week)." The model types the member as a plain `Integer`,
/// so the rule is applied by every operation that accepts the policy
/// ([`validate_fairshare_policy`]).
pub const MIN_SHARE_DECAY_SECONDS: i32 = 0;
/// See [`MIN_SHARE_DECAY_SECONDS`].
pub const MAX_SHARE_DECAY_SECONDS: i32 = 604_800;

/// The bounds of `FairsharePolicy$computeReservation`, from the member's own doc: "The
/// minimum value is 0 and the maximum value is 99."
pub const MIN_COMPUTE_RESERVATION: i32 = 0;
/// See [`MIN_COMPUTE_RESERVATION`].
pub const MAX_COMPUTE_RESERVATION: i32 = 99;

/// The bounds of `ShareAttributes$weightFactor`, from the member's own doc: "The smallest
/// supported value is 0.0001, and the largest supported value is 999.9999."
pub const MIN_WEIGHT_FACTOR: f32 = 0.0001;
/// See [`MIN_WEIGHT_FACTOR`].
pub const MAX_WEIGHT_FACTOR: f32 = 999.9999;

/// The `ShareAttributes$weightFactor` of an entry that carries none, from the member's own
/// doc: "The weight factor for the share identifier. The default value is 1.0."
pub const DEFAULT_WEIGHT_FACTOR: f32 = 1.0;

/// The longest `ShareAttributes$shareIdentifier` the service accepts, from the member's
/// own doc: "The string is limited to 255 alphanumeric characters, and can be followed by
/// an asterisk (*)."
pub const MAX_SHARE_IDENTIFIER_LEN: usize = 255;

/// The most share identifiers one fair-share policy may carry, from the
/// `ShareAttributes$shareIdentifier` doc ("There can be no more than 500 share identifiers
/// active in a job queue"), which `CreateJobQueue$schedulingPolicyArn` repeats ("Jobs
/// queues with a fair-share scheduling policy can have a maximum of 500 active share
/// identifiers"). The model constrains the list's length nowhere, so a longer
/// `shareDistribution` is refused here — before anything walks it
/// ([`validate_fairshare_policy`]), which is also what bounds the payload one stored
/// policy retains.
pub const MAX_SHARE_DISTRIBUTION_ENTRIES: usize = 500;

/// The lowest `schedulingPriority` the service accepts, from the member's doc: "The
/// minimum supported value is 0 and the maximum supported value is 9999."
pub const MIN_SCHEDULING_PRIORITY: i32 = 0;
/// The highest `schedulingPriority` the service accepts; see [`MIN_SCHEDULING_PRIORITY`].
pub const MAX_SCHEDULING_PRIORITY: i32 = 9999;

/// The shortest job timeout the service accepts, in seconds, from the `timeout` member's
/// doc: "The minimum value for the timeout is 60 seconds."
pub const MIN_ATTEMPT_DURATION_SECONDS: i32 = 60;

/// The bounds of `JobStateTimeLimitAction$maxTimeSeconds`, from the member's own doc:
/// "The minimum value is 600 (10 minutes) and the maximum value is 86,400 (24 hours)."
/// The model types the member as a plain `Integer`, so the framework admits anything and
/// the rule is applied by every operation that accepts the list
/// ([`validate_job_state_time_limit_actions`]).
pub const MIN_JOB_STATE_TIME_LIMIT_SECONDS: i32 = 600;
/// See [`MIN_JOB_STATE_TIME_LIMIT_SECONDS`].
pub const MAX_JOB_STATE_TIME_LIMIT_SECONDS: i32 = 86_400;

/// The most job definitions `DescribeJobDefinitions` accepts in one request, from the
/// `jobDefinitions` member's doc: "A list of up to 100 job definitions."
pub const MAX_DESCRIBE_FILTER_ENTRIES: usize = 100;

/// The page-size bounds `DescribeJobDefinitions` accepts, from the `maxResults` member's
/// doc: "This value can be between 1 and 100. If this parameter isn't used, then
/// `DescribeJobDefinitions` returns up to 100 results".
pub const MIN_DESCRIBE_PAGE_SIZE: i32 = 1;
/// See [`MIN_DESCRIBE_PAGE_SIZE`]; also the page size used when the caller names no
/// `maxResults`.
pub const MAX_DESCRIBE_PAGE_SIZE: i32 = 100;

/// The most job-definition revisions this emulator keeps.
///
/// An EMULATOR-CHOSEN ceiling, not an AWS quota: the store is in memory and every
/// `RegisterJobDefinition` adds to it, so the registry answers the modelled
/// `ClientException` past this point rather than growing without bound. AWS enforces a
/// per-account job-definition quota of its own.
pub const MAX_JOB_DEFINITIONS: usize = 5_000;

/// `ConsumableResource$resourceType` of a resource that is handed back when a job that
/// held it completes — the default, from the `resourceType` member's own doc:
/// "`REPLENISHABLE` (default)".
pub const RESOURCE_TYPE_REPLENISHABLE: &str = "REPLENISHABLE";

/// `ConsumableResource$resourceType` of a resource a completed job does NOT hand back,
/// from the same doc's list.
pub const RESOURCE_TYPE_NON_REPLENISHABLE: &str = "NON_REPLENISHABLE";

/// Every `resourceType` a consumable resource may carry, from the `resourceType` member's
/// doc ("Can be one of: `REPLENISHABLE` (default), `NON_REPLENISHABLE`"). The model types
/// the member as a plain `String`, so the framework admits anything and the handler
/// applies this list.
pub const CONSUMABLE_RESOURCE_TYPES: [&str; 2] =
    [RESOURCE_TYPE_REPLENISHABLE, RESOURCE_TYPE_NON_REPLENISHABLE];

/// `UpdateConsumableResource$operation` that makes the request's `quantity` the new total,
/// from the member's own doc: "`SET` Sets the quantity of the resource to the value
/// specified by the `quantity` parameter."
pub const UPDATE_OPERATION_SET: &str = "SET";

/// `UpdateConsumableResource$operation` that adds the request's `quantity` to the total,
/// from the same doc: "`ADD` Increases the quantity of the resource by the value specified
/// by the `quantity` parameter."
pub const UPDATE_OPERATION_ADD: &str = "ADD";

/// `UpdateConsumableResource$operation` that takes the request's `quantity` off the total,
/// from the same doc: "`REMOVE` Reduces the quantity of the resource by the value specified
/// by the `quantity` parameter."
pub const UPDATE_OPERATION_REMOVE: &str = "REMOVE";

/// Every `operation` an `UpdateConsumableResource` may name, from that member's doc ("Must
/// be one of: SET, ADD, REMOVE"). The model types the member as a plain `String`, so the
/// framework admits anything and the handler applies this list — the same arrangement as
/// [`CONSUMABLE_RESOURCE_TYPES`].
pub const CONSUMABLE_RESOURCE_UPDATE_OPERATIONS: [&str; 3] =
    [UPDATE_OPERATION_SET, UPDATE_OPERATION_ADD, UPDATE_OPERATION_REMOVE];

/// How long an `UpdateConsumableResource$clientToken` stays the key of the update it
/// applied, in milliseconds.
///
/// The member's own doc states the duration: "A clientToken is valid for 8 hours." Past it
/// the token keys nothing, so the same token names a new request.
pub const CLIENT_TOKEN_VALIDITY_MILLIS: i64 = 8 * 60 * 60 * 1_000;

/// `CEType` value whose compute resources AWS Batch manages.
pub const CE_TYPE_MANAGED: &str = "MANAGED";

/// `JobStatus` of a job the service has accepted but not yet scheduled.
pub const JOB_SUBMITTED: &str = "SUBMITTED";
/// `JobStatus` of a job waiting on the jobs it depends on.
pub const JOB_PENDING: &str = "PENDING";
/// `JobStatus` of a job the scheduler may place.
pub const JOB_RUNNABLE: &str = "RUNNABLE";
/// `JobStatus` of a job whose container is being started.
pub const JOB_STARTING: &str = "STARTING";
/// `JobStatus` of a job whose container is running.
pub const JOB_RUNNING: &str = "RUNNING";
/// Terminal `JobStatus`: the job's container exited successfully.
pub const JOB_SUCCEEDED: &str = "SUCCEEDED";
/// Terminal `JobStatus`: the job did not complete successfully.
pub const JOB_FAILED: &str = "FAILED";

/// How many `RUNNABLE` jobs `GetJobQueueSnapshot` lists, from the operation's own doc:
/// "The first 100 RUNNABLE jobs in the job queue are listed in order of dispatch."
pub const MAX_FRONT_OF_QUEUE_JOBS: usize = 100;

// `ServiceJobStatus` values. A separate enum from `JobStatus` in the model
// (`smithy_query(query="shape", shape="ServiceJobStatus")`: SUBMITTED, PENDING,
// RUNNABLE, SCHEDULED, STARTING, RUNNING, SUCCEEDED, FAILED), so a service job's
// statuses are named — and tested for terminality — on their own.
/// `ServiceJobStatus` of a service job the service has accepted but not yet placed —
/// the status `SubmitServiceJob` hands a fresh service job back in.
pub const SERVICE_JOB_SUBMITTED: &str = "SUBMITTED";
/// `ServiceJobStatus` of a service job the scheduler may place.
pub const SERVICE_JOB_RUNNABLE: &str = "RUNNABLE";
/// `ServiceJobStatus` of a service job whose work is running, and the status
/// `ListServiceJobs` selects when a request names none (`ListServiceJobs$jobStatus`: "If
/// you don't specify a status, only RUNNING jobs are returned").
pub const SERVICE_JOB_RUNNING: &str = "RUNNING";
/// Terminal `ServiceJobStatus`: the service job completed successfully.
pub const SERVICE_JOB_SUCCEEDED: &str = "SUCCEEDED";
/// Terminal `ServiceJobStatus`: the service job did not complete successfully.
pub const SERVICE_JOB_FAILED: &str = "FAILED";

/// `JobQueueType` of a queue that runs service jobs, which is the type
/// `SubmitServiceJob$jobQueue` documents ("The job queue must have the type
/// `SAGEMAKER_TRAINING`"). One of the model's `JobQueueType` values
/// (`smithy_query(query="shape", shape="JobQueueType")`).
pub const JOB_QUEUE_TYPE_SAGEMAKER_TRAINING: &str = "SAGEMAKER_TRAINING";

/// The `source` of every event AWS Batch publishes.
///
/// EventBridge names the publishing service in an event's `source` as
/// `aws.<service>`, and the "AWS Batch events" reference states Batch's own:
/// "Events are emitted on a best-effort basis ... with a `source` of
/// `aws.batch`".
pub const EVENT_SOURCE: &str = "aws.batch";

/// The `detail-type` of the event Batch publishes when a job changes state
/// ("AWS Batch events": the job state change event's detail type is
/// `Batch Job State Change`).
pub const JOB_STATE_CHANGE_DETAIL_TYPE: &str = "Batch Job State Change";

/// The event bus Batch publishes onto.
///
/// "Events generated by Amazon Web Services services go to your account's
/// default event bus" (EventBridge's event-bus reference), so a Batch event
/// lands on `default` and nowhere else.
pub const DEFAULT_EVENT_BUS: &str = "default";

/// How many unpublished job state changes this store holds.
///
/// This store's OWN bound, not a limit AWS publishes: a change is queued as it
/// happens and drained by the next call that reaches the publisher, so without a
/// bound a run that submits jobs and never calls again would grow the heap. It is
/// a ring — the oldest unpublished change is dropped before a new one is queued —
/// which is faithful to "events are emitted on a best-effort basis", and it is
/// far above any realistic burst (each job contributes at most three changes, and
/// the jobs store itself is capped at [`limits::MAX_JOBS`]).
const MAX_PENDING_EVENTS: usize = 4 * limits::MAX_JOBS;

/// One job state change waiting to be published to EventBridge.
///
/// The `detail` is rendered WHEN THE CHANGE HAPPENS (see
/// [`job_state_change_detail`]), so the event reports the job as it was at that
/// transition rather than as it is when the queue is drained.
#[derive(Debug, Clone)]
pub struct JobStateChange {
    /// The job's ARN, which the event envelope carries in `resources`.
    pub job_arn: String,
    /// The `JobStatus` the job moved INTO, kept beside the detail so a caller
    /// (and a test) can see which transition this record is for without parsing.
    pub status: String,
    /// WHEN the job moved, in epoch milliseconds: the event's `time`, so the
    /// envelope reports the transition's instant rather than the instant the queue
    /// was drained (which a reader can materialise much later).
    pub at: i64,
    /// The rendered `detail` of the event: the job's own description.
    pub detail: ::serde_json::Value,
}

/// The `detail` of a `Batch Job State Change` event: the job as this store holds
/// it, in the member names the `JobDetail` shape documents.
///
/// "The `detail` section of the event contains the job's details, in the same
/// form as the `DescribeJobs` API returns them" ("AWS Batch events"), so every
/// member below is read from the stored record — the SAME record `DescribeJobs`
/// projects — and nothing that is not stored is invented: `attempts` reports the
/// attempts the record holds (empty for a job nothing has placed, as the sample
/// event shows it), and `startedAt`, `stoppedAt`, `statusReason` and the optional
/// members appear only once the record carries them.
pub fn job_state_change_detail(job: &Job) -> ::serde_json::Value {
    use ::serde_json::{json, Map, Value};

    let mut detail = Map::new();
    detail.insert("jobArn".to_string(), json!(job.job_arn));
    detail.insert("jobName".to_string(), json!(job.job_name));
    detail.insert("jobId".to_string(), json!(job.job_id));
    detail.insert("jobQueue".to_string(), json!(job.job_queue_arn));
    detail.insert("jobDefinition".to_string(), json!(job.job_definition_arn));
    detail.insert("status".to_string(), json!(job.status));
    if let Some(reason) = &job.status_reason {
        detail.insert("statusReason".to_string(), json!(reason));
    }
    detail.insert("createdAt".to_string(), json!(job.created_at));
    if let Some(at) = job.started_at {
        detail.insert("startedAt".to_string(), json!(at));
    }
    if let Some(at) = job.stopped_at {
        detail.insert("stoppedAt".to_string(), json!(at));
    }
    // One entry per attempt the record holds, present and empty for a job no placement
    // has started an attempt for (the "AWS Batch events" sample event shows
    // `"attempts": []`).
    detail.insert(
        "attempts".to_string(),
        Value::Array(job.attempts.iter().map(attempt_event_detail).collect()),
    );
    detail.insert(
        "dependsOn".to_string(),
        Value::Array(
            job.depends_on
                .iter()
                .flatten()
                .map(|dependency| {
                    let mut entry = Map::new();
                    if let Some(id) = &dependency.job_id {
                        entry.insert("jobId".to_string(), json!(id));
                    }
                    if let Some(kind) = &dependency.r#type {
                        entry.insert("type".to_string(), json!(kind));
                    }
                    Value::Object(entry)
                })
                .collect(),
        ),
    );
    detail.insert(
        "parameters".to_string(),
        json!(job.parameters.clone().unwrap_or_default()),
    );
    if let Some(container) = &job.container {
        detail.insert("container".to_string(), container_event_detail(container));
    }
    if let Some(identifier) = &job.share_identifier {
        detail.insert("shareIdentifier".to_string(), json!(identifier));
    }
    if let Some(priority) = job.scheduling_priority {
        detail.insert("schedulingPriority".to_string(), json!(priority));
    }
    if let Some(timeout) = &job.timeout {
        if let Some(seconds) = timeout.attempt_duration_seconds {
            detail.insert(
                "timeout".to_string(),
                json!({ "attemptDurationSeconds": seconds }),
            );
        }
    }
    if let Some(strategy) = &job.retry_strategy {
        let mut rendered = Map::new();
        if let Some(attempts) = strategy.attempts {
            rendered.insert("attempts".to_string(), json!(attempts));
        }
        detail.insert("retryStrategy".to_string(), Value::Object(rendered));
    }
    if let Some(capabilities) = &job.platform_capabilities {
        detail.insert("platformCapabilities".to_string(), json!(capabilities));
    }
    if let Some(propagate) = job.propagate_tags {
        detail.insert("propagateTags".to_string(), json!(propagate));
    }
    if let Some(tags) = &job.tags {
        detail.insert("tags".to_string(), json!(tags));
    }
    // The array properties the RECORD holds: a child's own index, a parent's size. The
    // parent's `statusSummary` is counted from the children's records and so is not a
    // member of the job this detail is about; it is reported by the readers that can see
    // them (`DescribeJobs`, `ListJobs`).
    if let Some(index) = job.array_index {
        detail.insert("arrayProperties".to_string(), json!({ "index": index }));
    } else if let Some(size) = job.array_size {
        detail.insert("arrayProperties".to_string(), json!({ "size": size }));
    }
    detail.insert("isCancelled".to_string(), json!(job.is_cancelled));
    detail.insert("isTerminated".to_string(), json!(job.is_terminated));
    Value::Object(detail)
}

/// The `container` member of a job state change's detail: the stored effective
/// container, in the member names `ContainerDetail` documents.
///
/// Only the members this store actually holds are rendered — the effective
/// container the definition and the request's overrides add up to. Nothing a run
/// would have produced (an `exitCode`, a `logStreamName`, a `taskArn`, a network
/// interface) is invented, because no container ran; `reason` appears only
/// because `finish_job` records the modelled failure reason on it.
fn container_event_detail(container: &ContainerDetail) -> ::serde_json::Value {
    use ::serde_json::{json, Map, Value};

    let mut rendered = Map::new();
    if let Some(image) = &container.image {
        rendered.insert("image".to_string(), json!(image));
    }
    if let Some(vcpus) = container.vcpus {
        rendered.insert("vcpus".to_string(), json!(vcpus));
    }
    if let Some(memory) = container.memory {
        rendered.insert("memory".to_string(), json!(memory));
    }
    if let Some(command) = &container.command {
        rendered.insert("command".to_string(), json!(command));
    }
    if let Some(role) = &container.job_role_arn {
        rendered.insert("jobRoleArn".to_string(), json!(role));
    }
    if let Some(role) = &container.execution_role_arn {
        rendered.insert("executionRoleArn".to_string(), json!(role));
    }
    if let Some(user) = &container.user {
        rendered.insert("user".to_string(), json!(user));
    }
    if let Some(privileged) = container.privileged {
        rendered.insert("privileged".to_string(), json!(privileged));
    }
    if let Some(readonly) = container.readonly_root_filesystem {
        rendered.insert("readonlyRootFilesystem".to_string(), json!(readonly));
    }
    if let Some(reason) = &container.reason {
        rendered.insert("reason".to_string(), json!(reason));
    }
    if let Some(environment) = &container.environment {
        rendered.insert(
            "environment".to_string(),
            Value::Array(
                environment
                    .iter()
                    .map(|pair| {
                        let mut entry = Map::new();
                        if let Some(name) = &pair.name {
                            entry.insert("name".to_string(), json!(name));
                        }
                        if let Some(value) = &pair.value {
                            entry.insert("value".to_string(), json!(value));
                        }
                        Value::Object(entry)
                    })
                    .collect(),
            ),
        );
    }
    if let Some(requirements) = &container.resource_requirements {
        rendered.insert(
            "resourceRequirements".to_string(),
            Value::Array(
                requirements
                    .iter()
                    .map(|requirement| {
                        json!({ "type": requirement.r#type, "value": requirement.value })
                    })
                    .collect(),
            ),
        );
    }
    Value::Object(rendered)
}

/// A digest of the members ONE request carries, so an identical repeat of a `clientToken`
/// can be told apart from the same token carrying a different payload — the comparison
/// every `clientToken` on this service documents ("two requests with identical payloads and
/// clientTokens ... are considered the same request").
///
/// SHA-256 over `parts` (the `sha2` crate, hex-encoded with `hex`, as [`new_uuid`] does),
/// each separated by a NUL so two members cannot run together into the same bytes. The
/// caller renders its OWN members, in a fixed order, and orders any map's entries before
/// rendering them: the digest must depend on what the request says, never on how it
/// happened to be iterated. One definition, so two operations' token paths cannot drift
/// apart on the layout.
pub fn request_members_digest<I>(parts: I) -> String
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_ref().as_bytes());
        hasher.update([0_u8]);
    }
    hex::encode(hasher.finalize())
}

/// A fresh identifier in the shape AWS Batch gives a job id and a compute environment
/// `uuid`: a version-4 UUID, `8-4-4-4-12` lowercase hex with the version and variant bits
/// RFC 4122 §4.4 fixes.
///
/// The 128 bits come from three sources hashed together with SHA-256 (the `sha2` crate)
/// and hex-encoded (the `hex` crate): the OS-seeded randomness a `RandomState` draws, the
/// system clock in nanoseconds, and `sequence` — a counter the store hands out, so two
/// calls in the same nanosecond cannot collide. An id names a resource for correlation; a
/// credential would come from `ctx.credential_store()` instead, never from here.
pub fn new_uuid(sequence: u64) -> String {
    use sha2::{Digest, Sha256};
    use std::collections::hash_map::RandomState;
    use std::hash::BuildHasher;

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let mut hasher = Sha256::new();
    hasher.update(RandomState::new().hash_one(sequence).to_le_bytes());
    hasher.update(nanos.to_le_bytes());
    hasher.update(sequence.to_le_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 4122 §4.4: the version nibble is 4, and the variant's two top bits are `10`.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Epoch milliseconds, the unit every `createdAt`/`startedAt`/`stoppedAt` member
/// of this service carries.
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
}

/// The region this request was signed for, which is the region of every ARN the
/// request mints. Falls back to [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// `arn:aws:batch:<region>:<account>:compute-environment/<name>`.
pub fn compute_environment_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("compute-environment/{name}"))
}

/// `arn:aws:batch:<region>:<account>:job-queue/<name>`.
pub fn job_queue_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("job-queue/{name}"))
}

/// `arn:aws:batch:<region>:<account>:job-definition/<name>:<revision>`.
pub fn job_definition_arn(region: &str, name: &str, revision: i32) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("job-definition/{name}:{revision}"),
    )
}

/// `arn:aws:batch:<region>:<account>:job/<job id>`.
pub fn job_arn(region: &str, job_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("job/{job_id}"))
}

/// `arn:aws:batch:<region>:<account>:consumable-resource/<name>`.
pub fn consumable_resource_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("consumable-resource/{name}"))
}

/// `arn:aws:batch:<region>:<account>:service-environment/<name>`.
pub fn service_environment_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("service-environment/{name}"),
    )
}

/// `arn:aws:batch:<region>:<account>:job-queue/<queue>/quota-share/<name>` — a quota
/// share is named under the job queue it is associated with, which is the ARN pattern
/// `DescribeQuotaShare` matches a `quotaShareArn` against.
pub fn quota_share_arn(region: &str, job_queue_name: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("job-queue/{job_queue_name}/quota-share/{name}"),
    )
}

/// The `(job queue name, quota share name)` a well-formed `quotaShareArn` names, or
/// `None` when the value carries no `job-queue/<queue>/quota-share/<share>` path — which
/// is a DIFFERENT refusal from an ARN of the right shape naming nothing stored.
pub fn parse_quota_share_arn(value: &str) -> Option<(&str, &str)> {
    let resource = arn_resource_id(value, "job-queue")?;
    let (queue, share) = resource.split_once("/quota-share/")?;
    (!queue.is_empty() && !share.is_empty() && !share.contains('/')).then_some((queue, share))
}

/// `arn:aws:batch:<region>:<account>:scheduling-policy/<name>` — the format
/// `CreateSchedulingPolicy$arn` documents ("The format is
/// aws:Partition:batch:Region:Account:scheduling-policy/Name").
pub fn scheduling_policy_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("scheduling-policy/{name}"))
}

/// The policy NAME a well-formed scheduling-policy ARN names, or `None` when `value` is
/// not a scheduling-policy ARN at all — which is a DIFFERENT refusal from an ARN of the
/// right shape naming nothing stored (that one is reported as absent, not refused).
///
/// Every scheduling-policy member is documented as an ARN and only as an ARN
/// (`DescribeSchedulingPolicies$arns` is "a list of up to 100 scheduling policy Amazon
/// Resource Name (ARN) entries"; `DeleteSchedulingPolicy$arn` and
/// `UpdateSchedulingPolicy$arn` are "the Amazon Resource Name (ARN) of the scheduling
/// policy"), unlike the "name or ARN" members every other resource kind takes.
pub fn parse_scheduling_policy_arn(value: &str) -> Option<&str> {
    let name = arn_resource_id(value, "scheduling-policy")?;
    (!name.is_empty() && !name.contains('/')).then_some(name)
}

/// `arn:aws:batch:<region>:<account>:service-job/<job id>` — the ARN a service job
/// carries, the resource kind AWS Batch names a service job with.
pub fn service_job_arn(region: &str, job_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("service-job/{job_id}"))
}

/// The resource part of an ARN (everything after the account field), or `None`
/// when `value` is not an ARN at all.
fn arn_resource(value: &str) -> Option<&str> {
    if !value.starts_with("arn:") {
        return None;
    }
    value.splitn(6, ':').nth(5)
}

/// The resource id an ARN of this `kind` names (`compute-environment`,
/// `job-queue`, `job-definition`, `job`, `service-environment`), or `None` when `value`
/// is not an ARN of that kind. A plain name is passed through by the callers below,
/// which is how every "name or ARN" member resolves both ways.
fn arn_resource_id<'a>(value: &'a str, kind: &str) -> Option<&'a str> {
    arn_resource(value)?.strip_prefix(kind)?.strip_prefix('/')
}

/// What a Batch resource ARN names: which of this service's stores holds that
/// resource, and the key it is held under.
///
/// Parsed ONCE, by [`tagged_resource`], so every tagging operation
/// (`TagResource`, `UntagResource`, `ListTagsForResource`) covers exactly the same
/// resource kinds — every kind whose `Create*`/`Register*`/`Submit*` accepts a `tags`
/// map, which is every record in this store that carries one.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TaggedResource {
    ComputeEnvironment(String),
    JobQueue(String),
    /// Keyed by the ARN itself, the way [`ComAmazonawsBatchAwsbatchv20160810State::quota_shares`] keys a share.
    QuotaShare(String),
    /// The `<name>:<revision>` reference the ARN carries.
    JobDefinition(String),
    Job(String),
    ServiceJob(String),
    ConsumableResource(String),
    SchedulingPolicy(String),
    ServiceEnvironment(String),
}

/// The resource an ARN names, or `None` when `value` is not an ARN of a Batch resource
/// kind this service holds.
///
/// Only the ARN form resolves: `resourceArn` is documented as "The Amazon Resource Name
/// (ARN) of the resource", so a bare name names nothing here (unlike the "name or ARN"
/// members every other operation takes).
fn tagged_resource(value: &str) -> Option<TaggedResource> {
    // A quota share's ARN is nested under its job queue's, so it is recognised before
    // the `job-queue/<name>` form it starts with.
    if parse_quota_share_arn(value).is_some() {
        return Some(TaggedResource::QuotaShare(value.to_string()));
    }
    let (kind, id) = arn_resource(value)?.split_once('/')?;
    if id.is_empty() {
        return None;
    }
    let id = id.to_string();
    match kind {
        "compute-environment" => Some(TaggedResource::ComputeEnvironment(id)),
        "job-queue" => Some(TaggedResource::JobQueue(id)),
        "job-definition" => Some(TaggedResource::JobDefinition(id)),
        "job" => Some(TaggedResource::Job(id)),
        "service-job" => Some(TaggedResource::ServiceJob(id)),
        "consumable-resource" => Some(TaggedResource::ConsumableResource(id)),
        "scheduling-policy" => Some(TaggedResource::SchedulingPolicy(id)),
        "service-environment" => Some(TaggedResource::ServiceEnvironment(id)),
        _ => None,
    }
}

/// The arm list both tag accessors share, written ONCE.
///
/// [`ComAmazonawsBatchAwsbatchv20160810State::resource_tags`] and
/// [`ComAmazonawsBatchAwsbatchv20160810State::resource_tags_mut`] differ only in the map
/// lookup (`get` or `get_mut`) and in how they borrow the record's `tags`, so the list of
/// kinds lives here: a kind added to [`TaggedResource`] is added once and both accessors
/// gain it together, which is what keeps a read and the write that follows it on the same
/// record.
macro_rules! resource_tags_of {
    ($state:expr, $arn:expr, $get:ident $(, $mutability:tt)?) => {
        match tagged_resource($arn)? {
            TaggedResource::ComputeEnvironment(name) => $state
                .compute_environments
                .$get(&name)
                .filter(|record| record.arn == $arn)
                .map(|record| & $($mutability)? record.tags),
            TaggedResource::JobQueue(name) => $state
                .job_queues
                .$get(&name)
                .filter(|record| record.arn == $arn)
                .map(|record| & $($mutability)? record.tags),
            TaggedResource::QuotaShare(key) => $state
                .quota_shares
                .$get(&key)
                .map(|record| & $($mutability)? record.tags),
            TaggedResource::JobDefinition(reference) => {
                let key = $state.job_definition_key(&reference)?;
                $state
                    .job_definitions
                    .$get(&key)
                    .filter(|record| record.job_definition_arn == $arn)
                    .map(|record| & $($mutability)? record.tags)
            }
            TaggedResource::Job(id) => $state
                .jobs
                .$get(&id)
                .filter(|record| record.job_arn == $arn)
                .map(|record| & $($mutability)? record.tags),
            TaggedResource::ServiceJob(id) => $state
                .service_jobs
                .$get(&id)
                .filter(|record| record.job_arn == $arn)
                .map(|record| & $($mutability)? record.tags),
            TaggedResource::ConsumableResource(name) => $state
                .consumable_resources
                .$get(&name)
                .filter(|record| record.arn == $arn)
                .map(|record| & $($mutability)? record.tags),
            TaggedResource::SchedulingPolicy(name) => $state
                .scheduling_policies
                .$get(&name)
                .filter(|record| record.arn == $arn)
                .map(|record| & $($mutability)? record.tags),
            TaggedResource::ServiceEnvironment(name) => $state
                .service_environments
                .$get(&name)
                .filter(|record| record.arn == $arn)
                .map(|record| & $($mutability)? record.tags),
        }
    };
}

/// A compute environment, exactly as `CreateComputeEnvironment` stored it.
#[derive(Debug, Clone)]
pub struct ComputeEnvironment {
    pub name: String,
    pub arn: String,
    pub ce_type: String,
    pub state: String,
    pub status: String,
    pub status_reason: Option<String>,
    /// Set while the `CREATING` -> `VALID` or `UPDATING` -> `VALID` move is still
    /// owed by the clock.
    pub status_due_at: Option<i64>,
    /// The environment's unique identifier, minted once at creation.
    pub uuid: String,
    pub unmanagedv_cpus: Option<i32>,
    pub compute_resources: Option<ComputeResource>,
    /// The infrastructure update policy the last `UpdateComputeEnvironment` set, which
    /// `DescribeComputeEnvironments` reports back; `None` until one sets it (nothing
    /// else carries this member — `CreateComputeEnvironment` has no `updatePolicy`).
    pub update_policy: Option<UpdatePolicy>,
    pub service_role: Option<String>,
    pub eks_configuration: Option<EksConfiguration>,
    pub ecs_settings: Option<EcsSettings>,
    pub context: Option<String>,
    pub tags: Option<HashMap<String, String>>,
}

/// A service environment, exactly as `CreateServiceEnvironment` stored it.
///
/// The name is the environment's identity and the ARN is minted once beside it, so every
/// later operation reports the same one. AWS Batch returns a service environment READY:
/// the `DescribeServiceEnvironments` that follows the create reports it `VALID`, so there
/// is no transient status here and nothing about a fresh environment is owed by the clock.
/// [`ServiceEnvironment::status_due_at`] carries the one move a LATER call owes: the
/// `DELETING` span `DeleteServiceEnvironment` writes, at the end of which the record stops
/// existing.
#[derive(Debug, Clone)]
pub struct ServiceEnvironment {
    pub name: String,
    pub arn: String,
    /// `ServiceEnvironmentType` (the model's only value today is `SAGEMAKER_TRAINING`).
    pub environment_type: String,
    /// `ServiceEnvironmentState`: `ENABLED` unless the create asked for `DISABLED`.
    pub state: String,
    /// `ServiceEnvironmentStatus`.
    pub status: String,
    /// Set while a status move is still owed by the clock: the `DELETING` span a delete
    /// wrote. `None` for a fresh environment, which is already `VALID`.
    pub status_due_at: Option<i64>,
    pub capacity_limits: Vec<CapacityLimit>,
    pub tags: Option<HashMap<String, String>>,
}

/// A scheduling policy, exactly as `CreateSchedulingPolicy` stored it.
///
/// The name is the policy's identity and the ARN is minted once beside it
/// ([`scheduling_policy_arn`]), so every later operation reports the same one. AWS Batch
/// returns a scheduling policy ready — it carries no status member at all — so nothing
/// about it is owed by the clock.
///
/// A policy carries a fair-share policy OR a quota-share policy, never both: "Only one of
/// fairsharePolicy or quotaSharePolicy can be set. Once set, this policy type cannot be
/// removed or changed" (`CreateSchedulingPolicy$fairsharePolicy`).
#[derive(Debug, Clone)]
pub struct SchedulingPolicy {
    pub name: String,
    pub arn: String,
    pub fairshare_policy: Option<FairsharePolicy>,
    pub quota_share_policy: Option<QuotaSharePolicy>,
    pub tags: Option<HashMap<String, String>>,
}

/// A job queue, exactly as `CreateJobQueue` stored it. Its
/// `compute_environment_order` holds the ARN each entry resolved to, so a queue
/// created by compute environment NAME reports the environment's ARN the way AWS
/// Batch does.
#[derive(Debug, Clone)]
pub struct JobQueue {
    pub name: String,
    pub arn: String,
    pub state: String,
    pub status: String,
    pub status_reason: Option<String>,
    /// Set while the `CREATING` -> `VALID` or `UPDATING` -> `VALID` move is still owed by
    /// the clock.
    pub status_due_at: Option<i64>,
    pub priority: i32,
    pub scheduling_policy_arn: Option<String>,
    pub compute_environment_order: Vec<ComputeEnvironmentOrder>,
    pub service_environment_order: Option<Vec<ServiceEnvironmentOrder>>,
    pub job_queue_type: Option<String>,
    pub tags: Option<HashMap<String, String>>,
    pub job_state_time_limit_actions: Option<Vec<JobStateTimeLimitAction>>,
}

/// A quota share, exactly as `CreateQuotaShare` stored it.
///
/// Keyed by its ARN, which is the only identifier `DescribeQuotaShare` takes
/// (`quotaShareArn`), and nested under the job queue the share is associated with — what
/// that ARN names ([`quota_share_arn`]). AWS Batch returns a fresh quota share in the
/// non-terminal `CREATING` status, so the move to `VALID` is owed by the clock
/// ([`QuotaShare::status_due_at`]).
#[derive(Debug, Clone)]
pub struct QuotaShare {
    pub name: String,
    pub arn: String,
    /// The ARN of the job queue the create's `jobQueue` resolved to (by name or by ARN).
    pub job_queue_arn: String,
    pub capacity_limits: Vec<QuotaShareCapacityLimit>,
    pub resource_sharing_configuration: QuotaShareResourceSharingConfiguration,
    pub preemption_configuration: QuotaSharePreemptionConfiguration,
    /// `QuotaShareState`: `ENABLED` unless the create asked for `DISABLED` ("The default
    /// state is `ENABLED`").
    pub state: String,
    /// `QuotaShareStatus`.
    pub status: String,
    /// Set while a status move is still owed by the clock: the `CREATING` -> `VALID` move
    /// a create left, the `UPDATING` -> `VALID` one an update left, or the `DELETING` span
    /// a delete wrote, at the end of which the record stops existing.
    pub status_due_at: Option<i64>,
    pub tags: Option<HashMap<String, String>>,
}

/// A submitted job: the request's own members, the definition it resolved to, and
/// the effective container the definition and the overrides add up to.
#[derive(Debug, Clone)]
pub struct Job {
    pub job_id: String,
    pub job_arn: String,
    pub job_name: String,
    pub job_queue_arn: String,
    pub job_definition_arn: String,
    pub status: String,
    pub status_reason: Option<String>,
    pub created_at: i64,
    /// `startedAt`: "when the job transitioned from the `STARTING` state to the `RUNNING`
    /// state" (`JobDetail$startedAt`). Nothing here places a job on compute capacity, so
    /// no job ever starts and this stays `None` — the member is then ABSENT from every
    /// projection, rather than carrying an instant nothing ran at.
    pub started_at: Option<i64>,
    /// `stoppedAt`: "when the job transitioned from the `RUNNING` state to a terminal
    /// state" (`JobDetail$stoppedAt`). A job that never ran never made that transition, so
    /// it reports none even once it is terminal; [`Self::ended_at`] is what records WHEN
    /// it became terminal.
    pub stopped_at: Option<i64>,
    /// When this job reached a terminal `JobStatus`, in epoch milliseconds, whatever took
    /// it there.
    ///
    /// Internal bookkeeping, not a modelled member: it is what releases the jobs waiting
    /// on this one and what orders the terminal jobs the store evicts first. It is NOT
    /// `stoppedAt`, which AWS Batch documents as the `RUNNING` -> terminal transition and
    /// so leaves absent for a job that never ran.
    pub ended_at: Option<i64>,
    /// When the next clock-owed transition falls due; `None` while the job waits
    /// on a dependency, and once it is terminal.
    pub next_phase_due_at: Option<i64>,
    /// When this job reached its CURRENT position in the `RUNNABLE` ordering of its job
    /// queue, in epoch milliseconds, or `None` while it is not in that ordering.
    ///
    /// `GetJobQueueSnapshot` reports it as `earliestTimeAtPosition` ("when the job
    /// transitioned to its current position in the job queue"), so it is written when the
    /// job ENTERS `RUNNABLE` and again whenever a job ahead of it in the same queue leaves
    /// the ordering — which is the only thing that moves a job forward here.
    pub at_position_since: Option<i64>,
    pub is_cancelled: bool,
    pub is_terminated: bool,
    /// The `reason` of a `CancelJob` that arrived while this job was `PENDING`, i.e.
    /// still waiting on the jobs it depends on.
    ///
    /// `CancelJob`'s own doc defers that one case — "A PENDING job is cancelled after all
    /// dependency jobs are completed. Therefore, it might take longer than expected to
    /// cancel a job in PENDING status." — so the request is RECORDED here and
    /// materialised by the clock, when
    /// [`ComAmazonawsBatchAwsbatchv20160810State::advance_job`] next finds the
    /// dependencies complete. `None` for every other job.
    pub cancel_requested: Option<String>,
    pub parameters: Option<HashMap<String, String>>,
    pub container: Option<ContainerDetail>,
    /// The job attempts this job has had, oldest first — one per time the scheduler
    /// PLACED its container on compute capacity ([`Self::attempts`] is what
    /// `JobDetail$attempts`, "a list of job attempts that are associated with this job",
    /// is projected from).
    ///
    /// Empty for a job nothing has placed yet, which is why the projection reports no
    /// attempt for it. Bounded by the placements a job can have, which the lifecycle
    /// decides and no request can repeat.
    pub attempts: Vec<JobAttempt>,
    pub node_properties: Option<NodeProperties>,
    pub retry_strategy: Option<RetryStrategy>,
    pub timeout: Option<JobTimeout>,
    pub depends_on: Option<Vec<JobDependency>>,
    pub share_identifier: Option<String>,
    pub scheduling_priority: Option<i32>,
    pub propagate_tags: Option<bool>,
    pub platform_capabilities: Option<Vec<String>>,
    pub consumable_resource_properties: Option<ConsumableResourceProperties>,
    pub tags: Option<HashMap<String, String>>,
    /// `ArrayPropertiesDetail$size` — "The size of the array job. This parameter is
    /// returned for parent array jobs": how many children the submit that created this
    /// job spawned for it. `None` for every job that is not an array parent, which is what
    /// tells a parent apart from a plain job.
    pub array_size: Option<i32>,
    /// `ArrayPropertiesDetail$index` — "The job index within the array that's associated
    /// with this job. This parameter is returned for array job children". `None` for every
    /// job that is not a child of an array parent.
    pub array_index: Option<i32>,
    /// The id of the array parent that spawned this child, `None` for every other job.
    ///
    /// Internal bookkeeping, not a modelled member: it is what a child's own records are
    /// aggregated into ([`ComAmazonawsBatchAwsbatchv20160810State::array_status_summary`])
    /// and what keeps a child from being evicted while its parent is still live.
    pub array_parent_id: Option<String>,
    /// When this job's CURRENT status was written, in epoch milliseconds.
    ///
    /// Internal bookkeeping: it is what an array parent reports as its summary's
    /// `statusSummaryLastUpdatedAt` — the latest instant one of its children last moved —
    /// so that instant is read off the records the summary counts instead of being taken
    /// from the clock when somebody reads it.
    pub status_since: i64,
}

/// A consumable resource, exactly as `CreateConsumableResource` stored it.
///
/// AWS Batch hands a created consumable resource back ready — the create response carries
/// no status and `DescribeConsumableResource` models none — so this record owes the clock
/// no transition.
#[derive(Debug, Clone)]
pub struct ConsumableResource {
    pub name: String,
    pub arn: String,
    /// `totalQuantity`: how much of the resource exists. Non-negative (the member's doc:
    /// "Must be non-negative"), and 0 for a create that named none.
    pub total_quantity: i64,
    /// `inUseQuantity`: how much of the resource the jobs holding it have taken. 0 for a
    /// resource nothing has taken yet; the operations that reserve and release quantity
    /// maintain it.
    pub in_use_quantity: i64,
    /// One of [`CONSUMABLE_RESOURCE_TYPES`].
    pub resource_type: String,
    /// `createdAt`: the instant this record was stored, in epoch milliseconds.
    pub created_at: i64,
    pub tags: Option<HashMap<String, String>>,
}

impl ConsumableResource {
    /// `availableQuantity` — "The amount of the consumable resource that is currently
    /// available to use": what is left of [`Self::total_quantity`] once the quantity in
    /// use is taken off. Derived on every read, never stored, so it cannot drift from the
    /// two quantities it is computed from.
    pub fn available_quantity(&self) -> i64 {
        self.total_quantity.saturating_sub(self.in_use_quantity).max(0)
    }
}

/// One `UpdateConsumableResource` a `clientToken` already applied, kept so an identical
/// repeat of that request answers what the first one answered and applies nothing further.
///
/// The member's doc: "If this parameter is specified and two update requests with
/// identical payloads and clientTokens are received, these requests are considered the
/// same request. Both requests will succeed, but the update will only happen once." So the
/// response the first request computed is what is remembered, not just the fact of it.
#[derive(Debug, Clone)]
pub struct ConsumableResourceUpdate {
    /// A digest of the request that applied this update, so a token repeated with a
    /// DIFFERENT payload can be told apart from the identical repeat the doc describes.
    pub request_digest: String,
    /// The name of the resource the update was applied to — the response's
    /// `consumableResourceName`.
    pub consumable_resource_name: String,
    /// That resource's ARN — the response's `consumableResourceArn`.
    pub consumable_resource_arn: String,
    /// The `totalQuantity` the update left the resource at — the response's own member.
    pub total_quantity: i64,
    /// When the update was applied, in epoch milliseconds: what
    /// [`CLIENT_TOKEN_VALIDITY_MILLIS`] is measured from.
    pub applied_at: i64,
}

/// Whether a `JobStatus` is one a job never leaves.
pub fn is_terminal_status(status: &str) -> bool {
    matches!(status, JOB_SUCCEEDED | JOB_FAILED)
}

/// Whether `job` still REQUIRES the consumable resources its
/// `consumableResourceProperties` name.
///
/// A requirement lasts exactly as long as the job can still consume the resource: a job in
/// a terminal `JobStatus` ([`is_terminal_status`]) will never be placed again, so it holds
/// nothing and requires nothing, whatever took it there (the clock, `CancelJob`,
/// `TerminateJob`, the deletion of its queue). `ListJobsByConsumableResource` — whose
/// result set is "Batch jobs that require a specific consumable resource" — therefore
/// stops reporting a job from the instant it becomes terminal, and does so under every
/// filter, the status filters among them.
///
/// The job KEEPS its `consumableResourceProperties`: they are part of what was submitted,
/// so `DescribeJobs` still replays them. What ends is the requirement, not the record.
pub fn requires_consumable_resources(job: &Job) -> bool {
    !is_terminal_status(&job.status)
}

/// A service job, as `SubmitServiceJob` accepted it: the record `DescribeServiceJob`
/// reports and `TerminateServiceJob` ends.
///
/// A service job's work is dispatched to another service (`ServiceJobType` is
/// `SAGEMAKER_TRAINING`), which this emulator does not run, so no attempt, capacity
/// usage or preemption is ever invented for one: those members stay absent.
#[derive(Debug, Clone, Default)]
pub struct ServiceJob {
    pub job_id: String,
    pub job_arn: String,
    pub job_name: String,
    /// The ARN of the queue the job was submitted to, which is what
    /// `DescribeServiceJob$jobQueue` reports ("The ARN of the job queue").
    pub job_queue_arn: String,
    pub service_job_type: String,
    pub status: String,
    pub status_reason: Option<String>,
    pub created_at: i64,
    pub scheduled_at: Option<i64>,
    pub started_at: Option<i64>,
    pub stopped_at: Option<i64>,
    /// When the next clock-owed transition falls due, and `None` once the job owes none
    /// (it reached a terminal status, or it was seeded in one of the modelled statuses
    /// this emulator never moves a job into).
    pub next_phase_due_at: Option<i64>,
    /// Whether a `TerminateServiceJob` is what ended this job.
    pub is_terminated: bool,
    pub share_identifier: Option<String>,
    pub quota_share_name: Option<String>,
    pub scheduling_priority: Option<i32>,
    pub service_request_payload: Option<String>,
    /// What capacity this job asks for, derived from `service_request_payload` ONCE, when
    /// the submit was accepted ([`requested_capacity`]): what the scheduler weighs against
    /// the capacity the queue's service environments publish, so no later call re-reads the
    /// payload. Empty for a request that states no capacity this service can measure.
    pub requested_capacity: Vec<InstanceDemand>,
    pub retry_strategy: Option<ServiceJobRetryStrategy>,
    pub timeout_config: Option<ServiceJobTimeout>,
    pub preemption_configuration: Option<ServiceJobPreemptionConfiguration>,
    pub tags: Option<HashMap<String, String>>,
    /// The `clientToken` the `SubmitServiceJob` that minted this job carried, if any:
    /// what makes a repeat of that submit resolve to THIS record instead of minting a
    /// second one ("This token is used to ensure idempotency of requests").
    pub client_token: Option<String>,
    /// A digest of the submit request that minted this job, so a `clientToken` repeated
    /// with a DIFFERENT payload can be told apart from the identical replay the member's
    /// doc describes ("two submit requests with identical payloads and `clientToken`s").
    /// `None` for a record no submit request minted.
    pub request_digest: Option<String>,
}

/// Whether a `ServiceJobStatus` is one a service job never leaves.
pub fn is_terminal_service_job_status(status: &str) -> bool {
    matches!(status, SERVICE_JOB_SUCCEEDED | SERVICE_JOB_FAILED)
}

/// The `CapacityLimit$capacityUnit` that counts INSTANCES whatever their type.
///
/// The member's own doc: "For `SAGEMAKER_TRAINING` jobs in a quota management enabled
/// service environment, specify the instance type (for example, `ml.m5.large`). Otherwise,
/// use `NUM_INSTANCES`" — so a limit's unit is either an instance type or this.
pub const CAPACITY_UNIT_NUM_INSTANCES: &str = "NUM_INSTANCES";

/// Whether a caller-filled list this service STORES and later walks fits the maximum it is
/// kept to; the refusal detail naming the member, what it holds and the maximum otherwise.
///
/// Every list below is walked on a path a later request drives (a service job weighed
/// against the capacity its queue's environments publish), so each one is kept to a maximum
/// rather than letting one request decide how much work a later one costs.
pub fn validate_stored_list_length(
    member: &str,
    held: usize,
    maximum: usize,
) -> Result<(), String> {
    if held > maximum {
        return Err(format!(
            "{member} names {held} entries: at most {maximum} are stored"
        ));
    }
    Ok(())
}

/// Whether `payload` fits [`limits::MAX_SERVICE_REQUEST_PAYLOAD_BYTES`], the refusal detail
/// naming its size and the maximum otherwise.
pub fn validate_service_request_payload(payload: &str) -> Result<(), String> {
    if payload.len() > limits::MAX_SERVICE_REQUEST_PAYLOAD_BYTES {
        return Err(format!(
            "serviceRequestPayload is {} bytes: at most {} are stored",
            payload.len(),
            limits::MAX_SERVICE_REQUEST_PAYLOAD_BYTES
        ));
    }
    Ok(())
}

/// How many instances of one kind a service job's request asks for: the instance type the
/// group wants (when the request states one) and how many instances of it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstanceDemand {
    pub instance_type: Option<String>,
    pub instances: i32,
}

/// How much capacity ONE service job asks for: one entry per instance group its request
/// names, derived ONCE when the submit is accepted and kept on the record
/// ([`ServiceJob::requested_capacity`]), so no later call re-reads the payload.
///
/// `SubmitServiceJob$serviceRequestPayload` is "the request, in JSON, for the service that
/// the `SubmitServiceJob` operation is queueing", and for a `SAGEMAKER_TRAINING` job that
/// request is a SageMaker training request: the instances it needs are its `ResourceConfig`
/// (`InstanceType`/`InstanceCount`, and an `InstanceGroups` entry per group of a
/// heterogeneous cluster). A payload that states no `ResourceConfig` — or that is not an
/// object at all — asks for no capacity this service can measure, which is the EMPTY
/// demand: nothing to weigh against a limit.
///
/// At most [`limits::MAX_REQUESTED_INSTANCE_GROUPS`] groups are read, so the work and the
/// list this returns are bounded by a constant rather than by how many groups one request
/// names.
pub fn requested_capacity(payload: &str) -> Vec<InstanceDemand> {
    let Ok(request) = ::serde_json::from_str::<::serde_json::Value>(payload) else {
        return Vec::new();
    };
    let Some(resources) = request.get("ResourceConfig") else {
        return Vec::new();
    };
    let mut requested: Vec<InstanceDemand> = resources
        .get("InstanceGroups")
        .and_then(|groups| groups.as_array())
        .map(|groups| {
            groups
                .iter()
                .filter_map(instance_demand)
                .take(limits::MAX_REQUESTED_INSTANCE_GROUPS)
                .collect()
        })
        .unwrap_or_default();
    if requested.len() < limits::MAX_REQUESTED_INSTANCE_GROUPS {
        requested.extend(instance_demand(resources));
    }
    requested
}

/// The instances one `ResourceConfig` (or one of its `InstanceGroups` entries) asks for;
/// `None` when it states no `InstanceCount`.
fn instance_demand(resources: &::serde_json::Value) -> Option<InstanceDemand> {
    let instances = i32::try_from(resources.get("InstanceCount")?.as_i64()?).ok()?;
    let instance_type = resources
        .get("InstanceType")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    Some(InstanceDemand {
        instance_type,
        instances,
    })
}

/// Whether the capacity `limits` publish admits everything `requested` asks for
/// ([`requested_capacity`]).
///
/// A limit admits an entry when it is measured in the entry's own instance type and its
/// `maxCapacity` covers that entry, or it is measured in [`CAPACITY_UNIT_NUM_INSTANCES`]
/// and its `maxCapacity` covers the instances the whole request needs. A limit that states
/// no unit or no maximum publishes no bound, so it refuses nothing, and so does an
/// environment that publishes no limits at all; limits that publish nothing about an
/// entry's instance type cannot admit it.
///
/// The limits are folded into one maximum per unit FIRST, so the answer costs one pass over
/// each of the two lists rather than a pass over the limits per requested entry, and both
/// lists are themselves bounded ([`limits::MAX_SERVICE_ENVIRONMENT_CAPACITY_LIMITS`],
/// [`limits::MAX_REQUESTED_INSTANCE_GROUPS`]).
pub fn capacity_admits(limits: &[CapacityLimit], requested: &[InstanceDemand]) -> bool {
    if limits.is_empty() || requested.is_empty() {
        return true;
    }
    // The largest maximum published per unit, and whether any limit publishes no bound at
    // all (no unit or no maximum), which admits anything.
    let mut unbounded = false;
    let mut by_unit: BTreeMap<&str, i32> = BTreeMap::new();
    for limit in limits {
        match (limit.capacity_unit.as_deref(), limit.max_capacity) {
            (Some(unit), Some(maximum)) => {
                let held = by_unit.entry(unit).or_insert(maximum);
                *held = (*held).max(maximum);
            }
            _ => unbounded = true,
        }
    }
    if unbounded {
        return true;
    }
    // Saturating, because the counts come from a caller's payload: a request naming many
    // huge instance groups must not overflow the sum it is weighed with.
    let total: i32 = requested
        .iter()
        .fold(0_i32, |running, demand| {
            running.saturating_add(demand.instances)
        });
    let instances_published = by_unit.get(CAPACITY_UNIT_NUM_INSTANCES).copied();
    requested.iter().all(|demand| {
        instances_published.is_some_and(|maximum| maximum >= total)
            || demand.instance_type.as_deref().is_some_and(|wanted| {
                by_unit
                    .get(wanted)
                    .is_some_and(|maximum| *maximum >= demand.instances)
            })
    })
}

/// What a job's dependencies allow right now.
enum DependencyOutcome {
    /// At least one dependency has not finished.
    Waiting,
    /// A dependency names a job the store does not hold, so the job can never be released.
    Unresolvable,
    /// Every dependency succeeded; the value is the instant the last one stopped.
    Satisfied(i64),
    /// A dependency failed; the value is the instant it stopped.
    Failed(i64),
}

/// What a `CancelJob` call did to the job it named.
///
/// Every variant is a SUCCESS for the operation — AWS Batch answers `CancelJob` 200
/// "even if no job is cancelled" — so this reports what happened to the job rather than
/// whether the call is allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelOutcome {
    /// The store holds no job with that id, so there was nothing to cancel.
    Unknown,
    /// The job had not progressed past `RUNNABLE`: it is now `FAILED` and `isCancelled`.
    Cancelled,
    /// The job is `PENDING` on the jobs it depends on, so the cancel is recorded and
    /// falls due when they complete (see [`Job::cancel_requested`]).
    Deferred,
    /// The job reached `STARTING` or `RUNNING`, which `CancelJob` does not cancel: it is
    /// left running, for `TerminateJob` to end.
    NotCancelled,
    /// The job is already in a terminal status, and is left exactly as it is.
    AlreadyTerminal,
}

/// What materialising one owed transition did to a job.
enum Advanced {
    /// The job moved and is not terminal: it may owe another transition at this instant.
    Moved,
    /// The job reached a terminal status: the jobs waiting on it may now move.
    Terminal,
}

/// How many request ids this process has minted, so two refusals answered in the same
/// nanosecond still name two different requests (see [`new_uuid`]).
static MINTED_REQUEST_IDS: AtomicU64 = AtomicU64::new(0);

/// The id of the request a refusal is answering, which Batch's frontend names in the
/// envelope it wraps a `ClientException` in ([`client_exception_message`]).
///
/// A request id is minted by the SERVICE, per call — nothing in the request carries one —
/// so this emulator mints it the way it mints every other identifier ([`new_uuid`]): a
/// fresh version-4 UUID, distinct for every refusal for the emulator's lifetime. It is a
/// per-run value and is never derived from the request.
pub fn new_request_id() -> String {
    new_uuid(MINTED_REQUEST_IDS.fetch_add(1, Ordering::Relaxed))
}

/// How Batch words the `ClientException`s its frontend WRAPS: the detail of what was
/// wrong, inside `Error executing request, Exception : <detail>, RequestId: <id>`, where
/// the id is the one the service gave the call being refused.
///
/// Only the refusals AWS is recorded to wrap are worded this way; see
/// [`client_exception`] for the bare form and [`client_exception_enveloped`] for the
/// wrapped one.
pub fn client_exception_message(detail: &str, request_id: &str) -> String {
    format!("Error executing request, Exception : {detail}, RequestId: {request_id}")
}

/// The request id a message worded by [`client_exception_message`] states, or `None` for a
/// message that is not in that envelope.
///
/// The one reader of the envelope's shape, beside the one writer above, so the id a
/// response STATES can be reported in the header AWS carries a response's request id in
/// (`crate::service::request_id_header`) without a second copy of the format.
pub fn enveloped_request_id(message: &str) -> Option<&str> {
    let (head, request_id) = message.rsplit_once(", RequestId: ")?;
    head.starts_with("Error executing request, Exception : ")
        .then_some(request_id)
        .filter(|id| !id.is_empty())
}

/// The segment the validating walk spells a step from a collection into one of its
/// elements with (`aws_smithy_schema_server::constraints::path`): the step itself, and the
/// segment before it, name a POSITION inside that collection rather than a member of the
/// shape.
const COLLECTION_PATH_STEP: &str = "member";

/// The member a dotted violation path names: its last segment that is a member of a shape
/// rather than a position inside a collection, so a value inside a list or a map is
/// reported against the member that list or map arrived on.
fn violation_member(path: &str) -> &str {
    let mut segments: Vec<&str> = path.split('.').collect();
    while let Some(last) = segments.pop() {
        if last == COLLECTION_PATH_STEP {
            // What precedes a collection step is the POSITION inside that collection —
            // an element's index, or a key the caller chose — so it is dropped with it:
            // the rule fired on the member, and a refusal states the member.
            segments.pop();
            continue;
        }
        if last.is_empty() {
            continue;
        }
        return last;
    }
    path
}

/// A lowerCamel member name as the English noun phrase a Batch refusal opens with: the
/// name split into words at its humps, each word lowercased, and the first letter of the
/// phrase capitalised (`idleResourceAssignmentStrategy` becomes
/// `Idle resource assignment strategy`).
///
/// A run of capitals is one word (`EC2` stays `ec2`), because a hump only starts a word
/// where a lowercase letter or a digit precedes it, or where a lowercase letter follows
/// it.
fn member_phrase(member: &str) -> String {
    let chars: Vec<char> = member.chars().collect();
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    for (index, c) in chars.iter().enumerate() {
        let previous = index.checked_sub(1).and_then(|before| chars.get(before));
        let starts_word = c.is_uppercase()
            && !word.is_empty()
            && (previous.is_some_and(|p| p.is_lowercase() || p.is_ascii_digit())
                || chars.get(index + 1).is_some_and(|next| next.is_lowercase()));
        if starts_word {
            words.push(std::mem::take(&mut word));
        }
        word.extend(c.to_lowercase());
    }
    if !word.is_empty() {
        words.push(word);
    }
    let phrase = words.join(" ");
    let mut letters = phrase.chars();
    match letters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + letters.as_str(),
        None => phrase,
    }
}

/// How AWS Batch words the refusal of a value outside the closed set an enum member
/// declares: the member named as a noun phrase ([`member_phrase`]), then the set the
/// service enforced.
///
/// The rejected value is NOT echoed and the path the request nested the member at is not
/// reported: Batch names the rule that fired and the set it holds the member to, which is
/// what the caller has to satisfy. Recorded in exactly this form for a `quotaSharePolicy`
/// whose `idleResourceAssignmentStrategy` is outside its one-member enum, and raised for
/// every enum member of this service by the refusal seam (`crate::service`), since the
/// model types each of them as a plain `String` whose membership the boundary validator
/// checks before any handler runs.
pub fn enum_membership_detail<S: AsRef<str>>(member_path: &str, allowed: &[S]) -> String {
    let values: Vec<&str> = allowed.iter().map(AsRef::as_ref).collect();
    format!(
        "{} must be one of [{}].",
        member_phrase(violation_member(member_path)),
        values.join(", ")
    )
}

/// How AWS Batch words the refusal of a value outside a closed set the SERVICE holds a
/// member to: the member as the request spells it, then the set it enforced.
///
/// The rejected value is not echoed, for the same reason [`enum_membership_detail`] does
/// not echo it. The two differ in LAYER, and AWS words them differently: a member the
/// model declares an enum for is refused by the boundary validator, in the phrasing above,
/// while a member the model types as a plain `String` reaches a handler that applies the
/// set itself and is recorded with this phrasing — observed for
/// `CreateConsumableResource$resourceType`, and raised for every closed set a handler of
/// this service applies ([`CONSUMABLE_RESOURCE_TYPES`],
/// [`CONSUMABLE_RESOURCE_UPDATE_OPERATIONS`]).
pub fn closed_set_detail<S: AsRef<str>>(member: &str, allowed: &[S]) -> String {
    let values: Vec<&str> = allowed.iter().map(AsRef::as_ref).collect();
    format!(
        "{member} needs to be one of the following values: [{}]",
        values.join(", ")
    )
}

/// How AWS Batch words the refusal of a NAME that does not match the pattern it holds that
/// member to: the member as the request spells it, then that a valid pattern governs it.
///
/// Neither the pattern, nor the length or the charset it admits, is stated: the service
/// names WHICH member was malformed and that a pattern governs it, so the response does
/// not publish a bound a client could then encode. Recorded in exactly this form for
/// `CreateQuotaShare$quotaShareName`, and answered for every name member this service
/// checks itself ([`is_valid_resource_name`]) — the model types each of them as a plain
/// `String` with no `@pattern` for the boundary validator to enforce.
pub fn name_pattern_detail(member: &str) -> String {
    format!("{member} should match a valid pattern.")
}

/// The sentence AWS Batch's resource resolver answers when an identifier names no stored
/// resource: the ARN the identifier resolved to, then ` does not exist`.
///
/// The ARN comes from the REQUEST ([`resolved_arn`]) — the resource is not stored, so it
/// has no ARN of its own — and nothing else is stated: the identifier the caller gave is
/// what fills the sentence. Recorded in exactly this form for `UpdateComputeEnvironment`,
/// `DescribeConsumableResource` and `DescribeServiceJob`; the resolvers that terminate the
/// same sentence with a period take [`does_not_exist_sentence`] instead.
pub fn does_not_exist_message(arn: &str) -> String {
    format!("{arn} does not exist")
}

/// [`does_not_exist_message`] as the resolvers that TERMINATE the sentence word it: the
/// same sentence, closed with a period.
///
/// The job-queue resolver (`UpdateJobQueue`), the quota-share one (`DescribeQuotaShare`),
/// the scheduling-policy one (`UpdateSchedulingPolicy`) and the queue reference a
/// `CreateQuotaShare` only refers to are each recorded ending the sentence this way, so the
/// period is built here once instead of at every call site. What fills the sentence is
/// still the call site's: the resolvers that act ON a resource name the ARN they resolved
/// it to ([`resolved_arn`]), while a call that only REFERS to one echoes the reference the
/// request gave.
pub fn does_not_exist_sentence(arn: &str) -> String {
    format!("{}.", does_not_exist_message(arn))
}

/// A [`does_not_exist_message`] refusal: the modelled `ClientException` (HTTP 400) for an
/// identifier that resolves to no stored resource.
pub fn does_not_exist_exception(arn: &str) -> ClientException {
    client_exception(does_not_exist_message(arn))
}

/// The ARN a "name or ARN" identifier names, whether or not anything is stored under it:
/// the value itself when it already is an ARN, and otherwise the ARN `mint` gives that
/// name in this request's region and account (`compute_environment_arn`, `job_queue_arn`,
/// … — the same builders a create uses).
///
/// This is what a not-found refusal reports ([`does_not_exist_message`]): AWS names the
/// ARN it resolved the identifier to, and for an identifier naming nothing that ARN can
/// only be computed from the request.
pub fn resolved_arn(name_or_arn: &str, mint: impl FnOnce(&str) -> String) -> String {
    if name_or_arn.starts_with("arn:") {
        return name_or_arn.to_string();
    }
    mint(name_or_arn)
}

/// Whether `value` is a name AWS Batch accepts for a resource it keys by name: a
/// non-empty string of at most `max_len` letters, numbers, hyphens and underscores.
///
/// Every Batch resource whose name member documents that charset shares this rule
/// (`jobDefinitionName`, `serviceEnvironmentName`, …), and the model types each of them
/// as a plain `String`, so the framework admits anything and the rule is applied in the
/// handler.
pub fn is_valid_resource_name(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.chars().count() <= max_len
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Whether `value` is a `jobDefinitionName` the service accepts: up to
/// [`MAX_JOB_DEFINITION_NAME_LEN`] letters, numbers, hyphens and underscores
/// (the member's own doc). See [`is_valid_resource_name`].
pub fn is_valid_job_definition_name(value: &str) -> bool {
    is_valid_resource_name(value, MAX_JOB_DEFINITION_NAME_LEN)
}

/// Whether `value` is a `jobName` the service accepts: up to [`MAX_JOB_NAME_LEN`]
/// letters, numbers, hyphens and underscores (the member's own doc). See
/// [`is_valid_resource_name`].
pub fn is_valid_job_name(value: &str) -> bool {
    is_valid_resource_name(value, MAX_JOB_NAME_LEN)
}

/// Whether `value` is a `serviceEnvironmentName` the service accepts: up to
/// [`MAX_SERVICE_ENVIRONMENT_NAME_LEN`] letters, numbers, hyphens and underscores
/// (the member's own doc). See [`is_valid_resource_name`].
pub fn is_valid_service_environment_name(value: &str) -> bool {
    is_valid_resource_name(value, MAX_SERVICE_ENVIRONMENT_NAME_LEN)
}

/// Encode a `DescribeJobDefinitions` cursor: the `(name, revision)` key of the
/// first item of the NEXT page.
///
/// Hex-encoded so the token is opaque to the caller, as the member's doc asks
/// ("Treat this token as an opaque identifier").
pub fn encode_page_token(name: &str, revision: i32) -> String {
    hex::encode(format!("{name}:{revision}"))
}

/// Decode a cursor produced by [`encode_page_token`]; `None` when the token was
/// not produced by this service.
///
/// A job-definition name holds no `:` ([`is_valid_job_definition_name`]), so the
/// LAST `:` separates the name from the revision unambiguously.
pub fn decode_page_token(token: &str) -> Option<(String, i32)> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let (name, revision) = decoded.rsplit_once(':')?;
    Some((name.to_string(), revision.parse().ok()?))
}

/// The execution role of EVERY container-properties object a job definition
/// carries, one entry per object, in declaration order: the top-level
/// `containerProperties`, the `container` and the `ecsProperties` tasks of each
/// node range of a multi-node definition, and each `ecsProperties` task (which is
/// where an ECS task carries the role for its containers).
///
/// One traversal, so a rule about the execution role — "For jobs that run on
/// Fargate resources, you must provide an execution role" — is applied to every
/// place a job definition can carry container properties rather than only to the
/// top-level member.
pub fn execution_roles_of<'a>(
    container_properties: Option<&'a ContainerProperties>,
    node_properties: Option<&'a NodeProperties>,
    ecs_properties: Option<&'a EcsProperties>,
) -> Vec<Option<&'a str>> {
    let mut roles = Vec::new();
    if let Some(container) = container_properties {
        roles.push(container.execution_role_arn.as_deref());
    }
    if let Some(node) = node_properties {
        for range in &node.node_range_properties {
            if let Some(container) = range.container.as_ref() {
                roles.push(container.execution_role_arn.as_deref());
            }
            for task in range
                .ecs_properties
                .as_ref()
                .map(|ecs| ecs.task_properties.as_slice())
                .unwrap_or_default()
            {
                roles.push(task.execution_role_arn.as_deref());
            }
        }
    }
    if let Some(ecs) = ecs_properties {
        for task in &ecs.task_properties {
            roles.push(task.execution_role_arn.as_deref());
        }
    }
    roles
}

/// The `resourceRequirements` list of EVERY container-properties object a job definition
/// carries, one entry per object, in declaration order: the top-level
/// `containerProperties`, the `container` of each node range of a multi-node definition,
/// and each container of each `ecsProperties` task (its own or a node range's).
///
/// One traversal, the twin of [`execution_roles_of`], so a rule about what a container may
/// reserve is applied to every place a job definition can carry container properties
/// rather than only to the top-level member.
pub fn resource_requirements_of<'a>(
    container_properties: Option<&'a ContainerProperties>,
    node_properties: Option<&'a NodeProperties>,
    ecs_properties: Option<&'a EcsProperties>,
) -> Vec<&'a [ResourceRequirement]> {
    let mut requirements: Vec<&'a [ResourceRequirement]> = Vec::new();
    let mut push = |list: Option<&'a Vec<ResourceRequirement>>| {
        if let Some(list) = list {
            requirements.push(list.as_slice());
        }
    };
    if let Some(container) = container_properties {
        push(container.resource_requirements.as_ref());
    }
    if let Some(node) = node_properties {
        for range in &node.node_range_properties {
            if let Some(container) = range.container.as_ref() {
                push(container.resource_requirements.as_ref());
            }
            for task in range
                .ecs_properties
                .as_ref()
                .map(|ecs| ecs.task_properties.as_slice())
                .unwrap_or_default()
            {
                for container in &task.containers {
                    push(container.resource_requirements.as_ref());
                }
            }
        }
    }
    if let Some(ecs) = ecs_properties {
        for task in &ecs.task_properties {
            for container in &task.containers {
                push(container.resource_requirements.as_ref());
            }
        }
    }
    requirements
}

/// The reason `requirements` is not a Fargate-supported reservation, or `None` when it is.
///
/// `ResourceRequirement$value`'s own doc pairs the two: "For jobs that are running on
/// Fargate resources, then `value` must match one of the supported values and the `MEMORY`
/// values must be one of the values supported for that `VCPU` value" ([`FARGATE_VCPU_MEMORY`]).
/// A list that states neither resource states no pair to check; one that states only one of
/// them is incomplete, which is a different rule ("you must specify at least one vCPU", "you
/// must specify at least 4 MiB of memory") and not this function's.
pub fn fargate_reservation_refusal(requirements: &[ResourceRequirement]) -> Option<String> {
    let stated = |resource: &str| {
        requirements
            .iter()
            .find(|requirement| requirement.r#type == resource)
            .map(|requirement| requirement.value.as_str())
    };
    let (Some(vcpu), Some(memory)) = (
        stated(RESOURCE_TYPE_VCPU),
        stated(RESOURCE_TYPE_MEMORY),
    ) else {
        return None;
    };
    let admitted = vcpu
        .parse::<f64>()
        .ok()
        .zip(memory.parse::<i64>().ok())
        .is_some_and(|(vcpu, memory)| {
            FARGATE_VCPU_MEMORY.iter().any(|(supported, memories)| {
                (*supported - vcpu).abs() < f64::EPSILON && memories.contains(&memory)
            })
        });
    (!admitted).then(|| {
        // The detail AWS is recorded to state for a pair Fargate does not support, worded
        // from the reservation the request stated: the two values and their units.
        format!("Fargate resource requirements ({vcpu} vCPU, {memory} MiB) not valid.")
    })
}

/// `environment` with every pair whose value is the EMPTY STRING dropped.
///
/// A `KeyValuePair` carrying an empty value says nothing about the variable it names, so
/// AWS Batch is recorded to store no such variable: a read of the stored revision cannot
/// tell the name apart from one the request never mentioned. The model constrains
/// `KeyValuePair$value` not at all, so this is applied where the revision is WRITTEN.
fn without_empty_values(environment: &mut Option<Vec<KeyValuePair>>) {
    let Some(pairs) = environment.as_mut() else {
        return;
    };
    pairs.retain(|pair| pair.value.as_deref() != Some(""));
}

/// Drop every empty-valued environment pair from EVERY container-properties object a job
/// definition carries ([`without_empty_values`]), in the same traversal
/// [`execution_roles_of`] and [`resource_requirements_of`] walk.
///
/// Only the objects whose `environment` is a `KeyValuePair` list are walked: an Amazon EKS
/// pod states its variables as `EksContainerEnvironmentVariable`, a shape of its own whose
/// `value` AWS does not document this behaviour for.
pub fn normalize_environments(
    container_properties: Option<&mut ContainerProperties>,
    node_properties: Option<&mut NodeProperties>,
    ecs_properties: Option<&mut EcsProperties>,
) {
    if let Some(container) = container_properties {
        without_empty_values(&mut container.environment);
    }
    if let Some(node) = node_properties {
        for range in &mut node.node_range_properties {
            if let Some(container) = range.container.as_mut() {
                without_empty_values(&mut container.environment);
            }
            if let Some(ecs) = range.ecs_properties.as_mut() {
                for task in &mut ecs.task_properties {
                    for container in &mut task.containers {
                        without_empty_values(&mut container.environment);
                    }
                }
            }
        }
    }
    if let Some(ecs) = ecs_properties {
        for task in &mut ecs.task_properties {
            for container in &mut task.containers {
                without_empty_values(&mut container.environment);
            }
        }
    }
}

/// Per-service state for com-amazonaws-batch-awsbatchv20160810. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsBatchAwsbatchv20160810State {
    /// Compute environments by name.
    pub compute_environments: HashMap<String, ComputeEnvironment>,
    /// Compute environment ARN -> name, so `name or ARN` members resolve both ways.
    pub compute_environments_by_arn: HashMap<String, String>,
    /// Job queues by name.
    pub job_queues: HashMap<String, JobQueue>,
    /// Job queue ARN -> name.
    pub job_queues_by_arn: HashMap<String, String>,
    /// Service environments by name. Bounded by [`limits::MAX_SERVICE_ENVIRONMENTS`].
    pub service_environments: HashMap<String, ServiceEnvironment>,
    /// Service environment ARN -> name, so `name or ARN` members resolve both ways.
    pub service_environments_by_arn: HashMap<String, String>,
    /// Scheduling policies by name — the name `CreateSchedulingPolicy` keys them by.
    /// Bounded by [`limits::MAX_SCHEDULING_POLICIES`].
    pub scheduling_policies: HashMap<String, SchedulingPolicy>,
    /// Scheduling policy ARN -> name, so a policy named by ARN (the only form
    /// `DescribeSchedulingPolicies`, `UpdateSchedulingPolicy` and `DeleteSchedulingPolicy`
    /// document) resolves to its record.
    pub scheduling_policies_by_arn: HashMap<String, String>,
    /// Registered job-definition revisions, keyed by `(jobDefinitionName, revision)` —
    /// the describe and pagination order. Bounded by [`MAX_JOB_DEFINITIONS`].
    pub job_definitions: BTreeMap<(String, i32), JobDefinition>,
    /// Jobs by job id.
    pub jobs: HashMap<String, Job>,
    /// Consumable resources by name — the name `CreateConsumableResource` must keep
    /// unique ("The name of the consumable resource. Must be unique."). Bounded by
    /// [`limits::MAX_CONSUMABLE_RESOURCES`].
    pub consumable_resources: HashMap<String, ConsumableResource>,
    /// Consumable resource ARN -> name, so `name or ARN` members resolve both ways.
    pub consumable_resources_by_arn: HashMap<String, String>,
    /// The `UpdateConsumableResource` client tokens this store still remembers, keyed by
    /// the token, each with the update it applied
    /// ([`Self::consumable_resource_update_for_token`]). A token is valid for
    /// [`CLIENT_TOKEN_VALIDITY_MILLIS`] and the index is bounded by
    /// [`limits::MAX_CONSUMABLE_RESOURCE_UPDATE_TOKENS`].
    consumable_resource_updates: HashMap<String, ConsumableResourceUpdate>,
    /// Quota shares by ARN — the only identifier `DescribeQuotaShare` takes. Bounded by
    /// [`limits::MAX_QUOTA_SHARES`], and a share goes when its own `DeleteQuotaShare`
    /// falls due, or when the job queue it hangs off does.
    pub quota_shares: BTreeMap<String, QuotaShare>,
    /// Job ids per queue ARN, in submission order, backing `ListJobs`.
    pub jobs_by_queue: HashMap<String, Vec<String>>,
    /// Service jobs by service job id, the records `DescribeServiceJob` reports and
    /// `TerminateServiceJob` ends. Bounded by [`limits::MAX_SERVICE_JOBS`].
    pub service_jobs: HashMap<String, ServiceJob>,
    /// The jobs with a clock-owed transition still pending, ordered by the instant it
    /// falls due, so [`ComAmazonawsBatchAwsbatchv20160810State::materialize_due`] visits
    /// the jobs that are actually due instead of re-scanning the store.
    jobs_due: BTreeSet<(i64, String)>,
    /// Job ids waiting on a job, keyed by the job id they wait on: when a job reaches a
    /// terminal status, only the jobs that depend on it are re-examined.
    dependents: HashMap<String, Vec<String>>,
    /// How many identifiers this store has minted, so two ids are distinct even when the
    /// clock reports the same nanosecond (see [`new_uuid`]).
    minted_ids: u64,
    /// The job state changes this store has recorded and nothing has published to
    /// EventBridge yet, oldest first.
    ///
    /// A transition is recorded where it HAPPENS — under this store's lock, with no
    /// network call — and drained by the operation that caused it once the lock is
    /// released (see [`Self::take_pending_events`] and [`publish_job_state_changes`]).
    /// A ring bounded by [`MAX_PENDING_EVENTS`].
    pending_events: VecDeque<JobStateChange>,
    /// Whether a clock pump ([`drive_job_clock`]) is already waiting on this store's
    /// due instants, so a second submit does not start a second one.
    clock_pump_running: bool,
    /// Which claim the running pump holds, so a pump releases its OWN claim and never
    /// a later pump's (see [`Self::release_clock_pump`]).
    clock_pump_generation: u64,
    /// The running clock pump's task handle, kept so the pump it replaces is aborted
    /// rather than merely forgotten (see [`Self::set_clock_pump`]).
    clock_pump: Option<::tokio::task::JoinHandle<()>>,
}

impl ComAmazonawsBatchAwsbatchv20160810State {
    /// A fresh identifier for a resource this store is about to hold: the UUID shape AWS
    /// Batch uses, over this store's own counter (see [`new_uuid`]).
    pub fn new_uuid(&mut self) -> String {
        self.minted_ids = self.minted_ids.wrapping_add(1);
        new_uuid(self.minted_ids)
    }
    /// The compute environment `name_or_arn` names, resolved by name or by ARN.
    pub fn compute_environment(&self, name_or_arn: &str) -> Option<&ComputeEnvironment> {
        let name = self.compute_environment_name(name_or_arn)?;
        self.compute_environments.get(&name)
    }

    /// The NAME of the compute environment `name_or_arn` names.
    pub fn compute_environment_name(&self, name_or_arn: &str) -> Option<String> {
        if let Some(name) = self.compute_environments_by_arn.get(name_or_arn) {
            return Some(name.clone());
        }
        if self.compute_environments.contains_key(name_or_arn) {
            return Some(name_or_arn.to_string());
        }
        let id = arn_resource_id(name_or_arn, "compute-environment")?;
        self.compute_environments
            .contains_key(id)
            .then(|| id.to_string())
    }

    /// The job queue `name_or_arn` names, resolved by name or by ARN.
    pub fn job_queue(&self, name_or_arn: &str) -> Option<&JobQueue> {
        let name = self.job_queue_name(name_or_arn)?;
        self.job_queues.get(&name)
    }

    /// The NAME of the job queue `name_or_arn` names.
    pub fn job_queue_name(&self, name_or_arn: &str) -> Option<String> {
        if let Some(name) = self.job_queues_by_arn.get(name_or_arn) {
            return Some(name.clone());
        }
        if self.job_queues.contains_key(name_or_arn) {
            return Some(name_or_arn.to_string());
        }
        let id = arn_resource_id(name_or_arn, "job-queue")?;
        self.job_queues.contains_key(id).then(|| id.to_string())
    }

    /// The names of the job queues whose `computeEnvironmentOrder` still names the
    /// compute environment `arn`.
    ///
    /// `DeleteComputeEnvironment`'s own doc requires the environment to be disassociated
    /// "from any job queues with the UpdateJobQueue API operation" before it can be
    /// deleted, so the delete answers the modelled refusal while this is non-empty.
    pub fn queues_using_compute_environment(&self, arn: &str) -> Vec<String> {
        let mut names: Vec<String> = self
            .job_queues
            .values()
            .filter(|queue| {
                queue
                    .compute_environment_order
                    .iter()
                    .any(|entry| entry.compute_environment == arn)
            })
            .map(|queue| queue.name.clone())
            .collect();
        names.sort();
        names
    }

    /// The names of the job queues whose `serviceEnvironmentOrder` still names the service
    /// environment `arn`.
    ///
    /// `DeleteServiceEnvironment`'s own doc requires the environment to be disassociated
    /// "from any job queues with the UpdateJobQueue API operation" before it can be
    /// deleted, so the delete answers the modelled refusal while this is non-empty — the
    /// compute-environment rule above, on the other order a queue carries.
    pub fn queues_using_service_environment(&self, arn: &str) -> Vec<String> {
        let mut names: Vec<String> = self
            .job_queues
            .values()
            .filter(|queue| {
                queue
                    .service_environment_order
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .any(|entry| entry.service_environment == arn)
            })
            .map(|queue| queue.name.clone())
            .collect();
        names.sort();
        names
    }

    /// The consumable resource `name_or_arn` names, resolved by name or by ARN — the two
    /// forms every `consumableResource` member documents ("The name or ARN of the
    /// consumable resource").
    pub fn consumable_resource(&self, name_or_arn: &str) -> Option<&ConsumableResource> {
        let name = self.consumable_resource_name(name_or_arn)?;
        self.consumable_resources.get(&name)
    }

    /// The NAME of the consumable resource `name_or_arn` names.
    pub fn consumable_resource_name(&self, name_or_arn: &str) -> Option<String> {
        if let Some(name) = self.consumable_resources_by_arn.get(name_or_arn) {
            return Some(name.clone());
        }
        if self.consumable_resources.contains_key(name_or_arn) {
            return Some(name_or_arn.to_string());
        }
        let id = arn_resource_id(name_or_arn, "consumable-resource")?;
        self.consumable_resources
            .contains_key(id)
            .then(|| id.to_string())
    }

    /// The `UpdateConsumableResource` the `clientToken` `token` already applied, while that
    /// token is still valid at `now` ([`CLIENT_TOKEN_VALIDITY_MILLIS`]).
    ///
    /// What makes an identical repeat of an update "the same request" rather than a second
    /// one: the repeat answers the response this record holds and applies nothing.
    pub fn consumable_resource_update_for_token(
        &self,
        token: &str,
        now: i64,
    ) -> Option<&ConsumableResourceUpdate> {
        self.consumable_resource_updates
            .get(token)
            .filter(|update| now.saturating_sub(update.applied_at) < CLIENT_TOKEN_VALIDITY_MILLIS)
    }

    /// Remember that `token` applied `update`, so an identical repeat of that request
    /// resolves to it.
    ///
    /// Tokens whose validity has run out at `update.applied_at` are forgotten first, and a
    /// cache still at [`limits::MAX_CONSUMABLE_RESOURCE_UPDATE_TOKENS`] after that drops the
    /// token applied longest ago: the index is a replay cache, so losing its oldest entry
    /// costs a repeat its shortcut and never changes a stored quantity.
    pub fn remember_consumable_resource_update(
        &mut self,
        token: &str,
        update: ConsumableResourceUpdate,
    ) {
        let now = update.applied_at;
        self.consumable_resource_updates
            .retain(|_, kept| now.saturating_sub(kept.applied_at) < CLIENT_TOKEN_VALIDITY_MILLIS);
        while self.consumable_resource_updates.len()
            >= limits::MAX_CONSUMABLE_RESOURCE_UPDATE_TOKENS
        {
            let Some(oldest) = self
                .consumable_resource_updates
                .iter()
                .min_by(|(left_key, left), (right_key, right)| {
                    (left.applied_at, left_key.as_str())
                        .cmp(&(right.applied_at, right_key.as_str()))
                })
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.consumable_resource_updates.remove(&oldest);
        }
        self.consumable_resource_updates
            .insert(token.to_string(), update);
    }

    /// The service environment `name_or_arn` names, resolved by name or by ARN.
    pub fn service_environment(&self, name_or_arn: &str) -> Option<&ServiceEnvironment> {
        let name = self.service_environment_name(name_or_arn)?;
        self.service_environments.get(&name)
    }

    /// The NAME of the service environment `name_or_arn` names.
    pub fn service_environment_name(&self, name_or_arn: &str) -> Option<String> {
        if let Some(name) = self.service_environments_by_arn.get(name_or_arn) {
            return Some(name.clone());
        }
        if self.service_environments.contains_key(name_or_arn) {
            return Some(name_or_arn.to_string());
        }
        let id = arn_resource_id(name_or_arn, "service-environment")?;
        self.service_environments
            .contains_key(id)
            .then(|| id.to_string())
    }

    /// The scheduling policy `name_or_arn` names, resolved by ARN (the form every
    /// scheduling-policy member documents) or by plain name.
    pub fn scheduling_policy(&self, name_or_arn: &str) -> Option<&SchedulingPolicy> {
        let name = self.scheduling_policy_name(name_or_arn)?;
        self.scheduling_policies.get(&name)
    }

    /// The NAME of the scheduling policy `name_or_arn` names.
    pub fn scheduling_policy_name(&self, name_or_arn: &str) -> Option<String> {
        if let Some(name) = self.scheduling_policies_by_arn.get(name_or_arn) {
            return Some(name.clone());
        }
        if self.scheduling_policies.contains_key(name_or_arn) {
            return Some(name_or_arn.to_string());
        }
        let id = arn_resource_id(name_or_arn, "scheduling-policy")?;
        self.scheduling_policies
            .contains_key(id)
            .then(|| id.to_string())
    }

    /// The tag map of the resource `arn` names, or `None` when this service holds no
    /// such resource.
    ///
    /// The ONE place a tagging operation resolves a `resourceArn`, so `TagResource`,
    /// `UntagResource` and `ListTagsForResource` all cover the same resource kinds and
    /// all read the same stored map the `Describe*` readers report. The ARN must be the
    /// one the resource was minted with — the same region, account and id — because a
    /// different one names a different resource.
    pub fn resource_tags(&self, arn: &str) -> Option<&Option<HashMap<String, String>>> {
        resource_tags_of!(self, arn, get)
    }

    /// [`Self::resource_tags`], mutably: the slot a tagging WRITE updates, resolved and
    /// reached in one step so the write lands on the record the read reported.
    pub fn resource_tags_mut(&mut self, arn: &str) -> Option<&mut Option<HashMap<String, String>>> {
        resource_tags_of!(self, arn, get_mut, mut)
    }

    /// `entries` with every `computeEnvironment` resolved against the stored compute
    /// environments and rewritten as that environment's ARN; the refusal detail of the
    /// first entry that resolves to nothing otherwise.
    ///
    /// The normalisation rule is shared by every operation that accepts the list
    /// (`CreateJobQueue`, `UpdateJobQueue`), so a queue reports the ARN whichever form
    /// named the environment.
    ///
    /// An environment this service holds nothing for, and one whose deletion the clock
    /// already owes (so it can never be valid again), are the SAME refusal, and AWS states
    /// it as the requirement rather than as the entry it was given:
    /// [`COMPUTE_ENVIRONMENTS_NOT_ATTACHABLE`].
    pub fn resolve_compute_environment_order(
        &self,
        entries: &[ComputeEnvironmentOrder],
    ) -> Result<Vec<ComputeEnvironmentOrder>, String> {
        entries
            .iter()
            .map(|entry| {
                let environment = self
                    .compute_environment(&entry.compute_environment)
                    .filter(|environment| environment.status != STATUS_DELETING)
                    .ok_or_else(|| COMPUTE_ENVIRONMENTS_NOT_ATTACHABLE.to_string())?;
                Ok(ComputeEnvironmentOrder {
                    order: entry.order,
                    compute_environment: environment.arn.clone(),
                })
            })
            .collect()
    }

    /// `entries` with every `serviceEnvironment` resolved against the stored service
    /// environments and rewritten as that environment's ARN; see
    /// [`Self::resolve_compute_environment_order`].
    ///
    /// An entry naming an environment this service holds nothing for, and one naming an
    /// environment whose deletion the clock already owes (so it can never be valid
    /// again), are the SAME refusal, and AWS states it as the precondition rather than as
    /// the entry it was given: [`SERVICE_ENVIRONMENTS_NOT_ATTACHABLE`].
    pub fn resolve_service_environment_order(
        &self,
        entries: &[ServiceEnvironmentOrder],
    ) -> Result<Vec<ServiceEnvironmentOrder>, String> {
        entries
            .iter()
            .map(|entry| {
                let environment = self
                    .service_environment(&entry.service_environment)
                    .filter(|environment| environment.status != STATUS_DELETING)
                    .ok_or_else(|| SERVICE_ENVIRONMENTS_NOT_ATTACHABLE.to_string())?;
                Ok(ServiceEnvironmentOrder {
                    order: entry.order,
                    service_environment: environment.arn.clone(),
                })
            })
            .collect()
    }

    /// The revision `RegisterJobDefinition` gives the next registration of `name`: one
    /// past the highest revision stored for it, and 1 for a name nothing is stored for. A
    /// revision is never reused, so deregistering the highest one does not hand its
    /// number out again.
    pub fn next_revision(&self, name: &str) -> i32 {
        self.revisions_of(name)
            .max()
            .map_or(1, |highest| highest.saturating_add(1))
    }

    /// Every stored revision number of `name`, ascending.
    pub fn revisions_of<'a>(&'a self, name: &'a str) -> impl Iterator<Item = i32> + 'a {
        self.job_definitions
            .keys()
            .filter(move |(stored, _)| stored == name)
            .map(|(_, revision)| *revision)
    }

    /// The job-definition revision a `name`, `name:revision` or ARN reference names (with
    /// or without the revision, the forms `SubmitJob$jobDefinition` documents): the exact
    /// revision when one is given, else "the latest active revision".
    pub fn job_definition(&self, reference: &str) -> Option<&JobDefinition> {
        let (name, revision) = parse_job_definition_reference(reference)?;
        match revision {
            Some(revision) => self.job_definitions.get(&(name.to_string(), revision)),
            None => self
                .revisions_of(name)
                .filter_map(|revision| self.job_definitions.get(&(name.to_string(), revision)))
                .filter(|definition| {
                    definition.status.as_deref() == Some(STATUS_ACTIVE)
                })
                .max_by_key(|definition| definition.revision),
        }
    }

    /// The `(name, revision)` key of the STORED revision a reference names, whatever
    /// its status.
    ///
    /// The forms are the ones a "name and revision (`name:revision`) or full ARN"
    /// member documents; a reference carrying no revision names the definition's
    /// LATEST revision, which is the highest revision stored for the name. Unlike
    /// [`Self::job_definition`] — which resolves a reference a job is to RUN under, so
    /// a bare name there means the latest ACTIVE revision — this resolves a reference
    /// to a revision as a record, so an already-`INACTIVE` revision still answers to
    /// it and `DeregisterJobDefinition` stays idempotent.
    ///
    /// `None` when no stored revision answers to the reference.
    pub fn job_definition_key(&self, reference: &str) -> Option<(String, i32)> {
        let (name, revision) = parse_job_definition_reference(reference)?;
        match revision {
            Some(revision) => {
                let key = (name.to_string(), revision);
                self.job_definitions.contains_key(&key).then_some(key)
            }
            None => self
                .revisions_of(name)
                .max()
                .map(|revision| (name.to_string(), revision)),
        }
    }

    /// The STORED revision a reference names, mutably: [`Self::job_definition_key`]
    /// followed by the lookup it proved, so a caller that must WRITE the record (the
    /// status `DeregisterJobDefinition` sets) resolves and mutates in one step
    /// instead of resolving a key and then performing a second lookup that could
    /// miss.
    pub fn job_definition_mut(&mut self, reference: &str) -> Option<&mut JobDefinition> {
        let key = self.job_definition_key(reference)?;
        self.job_definitions.get_mut(&key)
    }

    /// Materialise every transition the clock already owes, and nothing else.
    ///
    /// Safe for a reporting operation to call: it writes only what was due at
    /// `now` whether or not anybody called, so a later caller sees exactly what
    /// this caller saw. That includes the LAST step of a deletion a `Delete*` call
    /// already started: a resource it moved to `DELETING` stops existing when that move
    /// falls due, which was owed at the same instant even if nobody had called.
    ///
    /// The work one call does is bounded. The compute environment, job queue, service
    /// environment and quota share stores are capped by [`limits::MAX_COMPUTE_ENVIRONMENTS`],
    /// [`limits::MAX_JOB_QUEUES`], [`limits::MAX_SERVICE_ENVIRONMENTS`] and
    /// [`limits::MAX_QUOTA_SHARES`],
    /// and the jobs are reached through the due index rather than by scanning: a call
    /// materialises the transitions that have actually fallen due — each of which happens
    /// once in a job's life — plus the jobs waiting on a job that just finished.
    pub fn materialize_due(&mut self, now: i64) {
        // A resource the clock owes a move either settles into `VALID` — whether it is
        // leaving `CREATING` (a create) or `UPDATING` (an update) — or, when a `Delete*`
        // call put it into `DELETING`, stops existing: the same clock-owed transition,
        // so they are materialised in the same pass.
        let mut deleted_environments: Vec<String> = Vec::new();
        // The earliest instant at which a compute environment settled into a status that
        // can hold a job, in THIS pass: the jobs resting in its queues are owed their
        // placement from that instant ([`Self::rearm_resting_jobs`]), never from the
        // instant somebody happened to call.
        let mut capacity_at: Option<i64> = None;
        for environment in self.compute_environments.values_mut() {
            if let Some(due) = environment.status_due_at.filter(|due| *due <= now) {
                environment.status_due_at = None;
                if environment.status == STATUS_DELETING {
                    deleted_environments.push(environment.name.clone());
                } else {
                    environment.status = STATUS_VALID.to_string();
                    if environment.ce_type == CE_TYPE_MANAGED
                        && environment.state == STATE_ENABLED
                    {
                        capacity_at = Some(capacity_at.map_or(due, |earliest: i64| earliest.min(due)));
                    }
                }
            }
        }
        for name in deleted_environments {
            if let Some(removed) = self.compute_environments.remove(&name) {
                self.compute_environments_by_arn.remove(&removed.arn);
            }
        }
        if let Some(at) = capacity_at {
            self.rearm_resting_jobs(at);
        }
        // A queue leaves its transient status (`CREATING` after a create, `UPDATING`
        // after an update) for `VALID` once the span the clock owes has elapsed, and
        // stops existing when the deletion a `DeleteJobQueue` started falls due.
        let mut deleted_queues: Vec<String> = Vec::new();
        for queue in self.job_queues.values_mut() {
            if queue.status_due_at.is_some_and(|due| due <= now) {
                queue.status_due_at = None;
                if queue.status == STATUS_DELETING {
                    deleted_queues.push(queue.name.clone());
                } else {
                    queue.status = STATUS_VALID.to_string();
                }
            }
        }
        for name in deleted_queues {
            if let Some(removed) = self.job_queues.remove(&name) {
                self.job_queues_by_arn.remove(&removed.arn);
                // "All jobs in the queue are eventually terminated when you delete a job
                // queue" (`DeleteJobQueue`): the queue's removal is when that falls due.
                for id in self.jobs_by_queue.remove(&removed.arn).unwrap_or_default() {
                    let live = self
                        .jobs
                        .get(&id)
                        .is_some_and(|job| !is_terminal_status(&job.status));
                    if live {
                        if let Some(job) = self.jobs.get_mut(&id) {
                            job.is_terminated = true;
                        }
                        self.finish_job(&id, JOB_FAILED, Some(JOB_QUEUE_DELETED_REASON), now);
                    }
                }
                // The quota shares that hang off the queue go with it: their ARNs are
                // nested under one nothing resolves any more.
                self.quota_shares
                    .retain(|_, share| share.job_queue_arn != removed.arn);
            }
        }
        // A service environment is stored already `VALID` (AWS returns it ready), so the
        // only move the clock owes one is the removal a delete accepted: the record stops
        // existing when the `DELETING` span it wrote falls due. The status is checked, not
        // assumed, so a transition added here later cannot be mistaken for a removal.
        let mut deleted_service_environments: Vec<String> = Vec::new();
        for environment in self.service_environments.values_mut() {
            if environment.status_due_at.is_some_and(|due| due <= now)
                && environment.status == STATUS_DELETING
            {
                environment.status_due_at = None;
                deleted_service_environments.push(environment.name.clone());
            }
        }
        for name in deleted_service_environments {
            if let Some(removed) = self.service_environments.remove(&name) {
                self.service_environments_by_arn.remove(&removed.arn);
                // A queue's `serviceEnvironmentOrder` must not keep an entry naming a
                // record nothing resolves any more, the way a deleted queue's quota shares
                // go with it above.
                for queue in self.job_queues.values_mut() {
                    if let Some(order) = queue.service_environment_order.as_mut() {
                        order.retain(|entry| entry.service_environment != removed.arn);
                    }
                }
            }
        }
        // A share leaves its transient status (`CREATING` after a create, `UPDATING` after
        // an update) for `VALID` once the span the clock owes has elapsed, and stops
        // existing when the removal a `DeleteQuotaShare` started falls due — the way a
        // queue's and an environment's do above.
        let mut deleted_shares: Vec<String> = Vec::new();
        for share in self.quota_shares.values_mut() {
            if share.status_due_at.is_some_and(|due| due <= now) {
                share.status_due_at = None;
                if share.status == STATUS_DELETING {
                    deleted_shares.push(share.arn.clone());
                } else {
                    share.status = STATUS_VALID.to_string();
                }
            }
        }
        for key in deleted_shares {
            if let Some(removed) = self.quota_shares.remove(&key) {
                // "All jobs in the share are eventually terminated when you delete a quota
                // share" (`DeleteQuotaShare`): the share's removal is when that falls due.
                // A job names its share by name, so the share it belongs to is the one of
                // that name on the queue the job was submitted to.
                let carried: Vec<String> = self
                    .service_jobs
                    .values()
                    .filter(|job| {
                        job.job_queue_arn == removed.job_queue_arn
                            && job.quota_share_name.as_deref() == Some(removed.name.as_str())
                    })
                    .map(|job| job.job_id.clone())
                    .collect();
                for id in carried {
                    self.terminate_service_job(&id, QUOTA_SHARE_DELETED_REASON, now);
                }
            }
        }
        self.advance_jobs(now);
        // An array parent's status is what its children add up to, so it is recomputed
        // after they have moved and never before.
        self.advance_array_parents(now);
        self.advance_service_jobs(now);
    }

    /// Advance the service jobs the clock owes a transition at `now`.
    ///
    /// A service job the service accepted is `SUBMITTED`, and one span later the scheduler
    /// has looked at it: a job whose requested capacity the service environments of its
    /// queue publish room for becomes `RUNNABLE` and is then owed its next move, while one
    /// that CANNOT be placed becomes `RUNNABLE` and RESTS there — it owes nothing, because
    /// no capacity is coming until a later call publishes it
    /// ([`Self::service_job_is_placeable`], [`Self::rearm_resting_service_jobs`]).
    ///
    /// A placeable job's work is dispatched to another service, which nothing here runs, so
    /// its chain ends one span later in `FAILED` with [`NO_SERVICE_JOB_RUNTIME_REASON`]. A
    /// record therefore owes at most two moves, and the store is capped by
    /// [`limits::MAX_SERVICE_JOBS`], so the work one call does is bounded. Nothing is
    /// invented on the way: a job that was never dispatched gets no `scheduledAt`,
    /// `startedAt` or attempt.
    fn advance_service_jobs(&mut self, now: i64) {
        // Whether a job can be placed is read from the queue and its service environments,
        // so it is decided for every job the clock owes a move BEFORE anything is written.
        let unplaceable: BTreeSet<String> = self
            .service_jobs
            .values()
            .filter(|job| {
                job.next_phase_due_at.is_some_and(|due| due <= now)
                    && !self.service_job_is_placeable(job)
            })
            .map(|job| job.job_id.clone())
            .collect();
        for job in self.service_jobs.values_mut() {
            while let Some(due) = job.next_phase_due_at.filter(|due| *due <= now) {
                let placeable = !unplaceable.contains(&job.job_id);
                match job.status.as_str() {
                    SERVICE_JOB_SUBMITTED => {
                        job.status = SERVICE_JOB_RUNNABLE.to_string();
                        job.next_phase_due_at =
                            placeable.then_some(due + JOB_PHASE_MILLIS);
                    }
                    // A job that cannot be placed stays where it is: `RUNNABLE` is where a
                    // service job waits for capacity, and nothing terminal may be reported
                    // for work the scheduler never handed on.
                    SERVICE_JOB_RUNNABLE if !placeable => job.next_phase_due_at = None,
                    SERVICE_JOB_RUNNABLE => {
                        job.status = SERVICE_JOB_FAILED.to_string();
                        job.status_reason = Some(NO_SERVICE_JOB_RUNTIME_REASON.to_string());
                        job.stopped_at = Some(due);
                        job.next_phase_due_at = None;
                    }
                    // Every other modelled status is one this emulator never moves a
                    // record into, so nothing is owed from it.
                    _ => job.next_phase_due_at = None,
                }
            }
        }
    }

    /// Whether the service environments behind `job`'s queue publish room for the capacity
    /// its request asks for ([`ServiceJob::requested_capacity`], [`capacity_admits`]).
    ///
    /// A service job is placed into ONE of the service environments its queue orders, so it
    /// is placeable as soon as any of them admits the whole request. A demand nothing here
    /// can measure (a payload stating no `ResourceConfig`), a queue or an environment this
    /// store no longer holds, and an environment that publishes no capacity limits all
    /// state no bound, so none of them holds a job back.
    ///
    /// The demand was derived when the submit was accepted and is read off the record, so
    /// this costs one walk of two bounded lists
    /// ([`limits::MAX_SERVICE_ENVIRONMENT_ORDER_ENTRIES`],
    /// [`limits::MAX_SERVICE_ENVIRONMENT_CAPACITY_LIMITS`]) and never re-reads a payload.
    fn service_job_is_placeable(&self, job: &ServiceJob) -> bool {
        if job.requested_capacity.is_empty() {
            return true;
        }
        let Some(queue) = self.job_queue(&job.job_queue_arn) else {
            return true;
        };
        let Some(order) = queue
            .service_environment_order
            .as_ref()
            .filter(|order| !order.is_empty())
        else {
            return true;
        };
        order.iter().any(|entry| {
            self.service_environment(&entry.service_environment)
                .is_none_or(|environment| {
                    capacity_admits(&environment.capacity_limits, &job.requested_capacity)
                })
        })
    }

    /// Owe the next move again to every service job resting in `RUNNABLE` that the capacity
    /// a call has just published now admits ([`Self::service_job_is_placeable`]).
    ///
    /// A resting job owes nothing while its queue's service environments cannot hold it, so
    /// the call that publishes the capacity it needs — one that raises an environment's own
    /// `capacityLimits` (`UpdateServiceEnvironment`), or one that gives the queue a
    /// different `serviceEnvironmentOrder` (`UpdateJobQueue`) — is what makes its next move
    /// due, one span later, like every other transition this service owes.
    pub fn rearm_resting_service_jobs(&mut self, now: i64) {
        let resting: Vec<String> = self
            .service_jobs
            .values()
            .filter(|job| {
                job.status == SERVICE_JOB_RUNNABLE
                    && job.next_phase_due_at.is_none()
                    && self.service_job_is_placeable(job)
            })
            .map(|job| job.job_id.clone())
            .collect();
        for id in resting {
            if let Some(job) = self.service_jobs.get_mut(&id) {
                job.next_phase_due_at = Some(now + JOB_PHASE_MILLIS);
            }
        }
    }

    /// Advance the jobs the clock owes a transition at `now`.
    ///
    /// Seeded from the due index, so no job that is not due is even looked at. Each pop
    /// materialises at most ONE transition; a job is re-queued only when it really moved,
    /// and a job waiting on a dependency is queued only when that dependency reaches a
    /// terminal status. The total work is therefore proportional to the transitions that
    /// fell due, not to the size of the store.
    fn advance_jobs(&mut self, now: i64) {
        let mut work: VecDeque<String> = self
            .jobs_due
            .iter()
            .take_while(|(due, _)| *due <= now)
            .map(|(_, id)| id.clone())
            .collect();
        while let Some(id) = work.pop_front() {
            match self.advance_job(&id, now) {
                None => {}
                Some(Advanced::Terminal) => {
                    if let Some(waiting) = self.dependents.get(&id) {
                        work.extend(waiting.iter().cloned());
                    }
                }
                Some(Advanced::Moved) => work.push_back(id),
            }
        }
    }

    /// Materialise the ONE transition `id` owes at `now`: `None` when nothing moved,
    /// else whether the job is now in a terminal status.
    fn advance_job(&mut self, id: &str, now: i64) -> Option<Advanced> {
        let job = self.jobs.get(id)?;
        if is_terminal_status(&job.status) {
            return None;
        }
        // An array PARENT walks no chain of its own: it runs no container, it is never
        // placed, and its status is what its children add up to
        // ([`Self::advance_array_parents`], which runs once the children of this pass have
        // moved). So the clock owes the parent record nothing.
        if job.array_size.is_some() {
            self.set_job_due(id, None);
            return None;
        }
        // What a PENDING job owes is decided by the jobs it depends on, recomputed from
        // their stop instants on every look, so the answer never depends on WHEN it was
        // looked at: released once they have all succeeded, failed when one failed, and
        // out of the due index again while any of them is still running. Its due instant
        // is only a wake-up hint, set by `finish_job` when a dependency finishes.
        if job.status == JOB_PENDING {
            // A `CancelJob` that arrived while the job was PENDING falls due with the
            // jobs it waits on, not with the call: whatever they completed WITH, the
            // job then ends FAILED carrying the caller's reason (`CancelJob`'s doc).
            if job.cancel_requested.is_some() {
                return match self.dependency_outcome(id, now) {
                    DependencyOutcome::Waiting => {
                        self.set_job_due(id, None);
                        None
                    }
                    // Nothing the API exposes reaches this (see
                    // `DEPENDENCY_UNRESOLVABLE_REASON`); the cancel the caller asked for
                    // is what ends the job, so it is what the job records.
                    DependencyOutcome::Unresolvable => {
                        self.finish_cancelled(id, now);
                        Some(Advanced::Terminal)
                    }
                    DependencyOutcome::Failed(at) | DependencyOutcome::Satisfied(at) => {
                        self.finish_cancelled(id, at);
                        Some(Advanced::Terminal)
                    }
                };
            }
            return match self.dependency_outcome(id, now) {
                DependencyOutcome::Waiting => {
                    self.set_job_due(id, None);
                    None
                }
                DependencyOutcome::Unresolvable => {
                    self.finish_job(id, JOB_FAILED, Some(DEPENDENCY_UNRESOLVABLE_REASON), now);
                    Some(Advanced::Terminal)
                }
                DependencyOutcome::Failed(at) => {
                    self.finish_job(id, JOB_FAILED, Some(DEPENDENT_JOB_FAILED_REASON), at);
                    Some(Advanced::Terminal)
                }
                DependencyOutcome::Satisfied(at) => {
                    // The job may be placed one span after the last dependency stopped,
                    // and then waits for the scheduler like every other released job.
                    let released = at + JOB_PHASE_MILLIS;
                    if released <= now {
                        self.set_job_status(id, JOB_RUNNABLE, released);
                        self.arm_placement(id, released);
                        Some(Advanced::Moved)
                    } else {
                        self.set_job_due(id, Some(released));
                        None
                    }
                }
            };
        }
        match job.next_phase_due_at {
            Some(due) if due <= now => {
                let waits = job.depends_on.as_ref().is_some_and(|on| !on.is_empty());
                match job.status.clone().as_str() {
                    JOB_SUBMITTED if waits => {
                        self.set_job_status(id, JOB_PENDING, due);
                        self.set_job_due(id, None);
                        Some(Advanced::Moved)
                    }
                    JOB_SUBMITTED => {
                        self.set_job_status(id, JOB_RUNNABLE, due);
                        // `RUNNABLE` is where a job waits for the scheduler to place it:
                        // the placement is owed one span later by the queue's own compute
                        // capacity, and a queue with none to offer leaves the job resting
                        // here ([`Self::arm_placement`]).
                        self.arm_placement(id, due);
                        Some(Advanced::Moved)
                    }
                    // A job resting in `RUNNABLE` owes nothing unless its queue's compute
                    // capacity placed it, which is what this due instant says happened.
                    JOB_RUNNABLE => {
                        if self.place_job(id) {
                            // Placed, so the attempt that placement started is recorded
                            // with the task it was placed as — and nothing here can RUN
                            // that task's container, so the placed job ends in the modelled
                            // failure state saying exactly that, which is what the attempt
                            // ends with too (see [`NO_CONTAINER_RUNTIME_REASON`]).
                            self.finish_job(id, JOB_FAILED, Some(NO_CONTAINER_RUNTIME_REASON), due);
                            Some(Advanced::Terminal)
                        } else {
                            self.set_job_due(id, None);
                            None
                        }
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Persist a submitted job, bounded by [`limits::MAX_JOBS`].
    ///
    /// Makes room first by evicting the jobs that have been terminal longest — a terminal
    /// job is one no later call can move again — and answers `Err(message)`, for the
    /// modelled `ClientException`, when every job in a full store is still live. ONE place
    /// decides that bound for every job this store holds
    /// ([`Self::make_room_for_jobs`]).
    pub fn insert_job(&mut self, job: Job) -> Result<(), String> {
        self.make_room_for_jobs(1)?;
        let id = job.job_id.clone();
        let queue_arn = job.job_queue_arn.clone();
        let due = job.next_phase_due_at;
        // An edge only for a dependency the store actually holds: a `dependsOn` id is
        // request-supplied, and `SubmitJob` refuses one that names no job, so an entry for
        // an absent id would be an index key nothing could ever remove.
        let dependencies: Vec<String> = job
            .depends_on
            .iter()
            .flatten()
            .filter_map(|dependency| dependency.job_id.clone())
            .filter(|dependency_id| self.jobs.contains_key(dependency_id))
            .collect();
        for dependency_id in dependencies {
            self.dependents.entry(dependency_id).or_default().push(id.clone());
        }
        self.jobs.insert(id.clone(), job);
        self.jobs_by_queue
            .entry(queue_arn)
            .or_default()
            .push(id.clone());
        if let Some(due) = due {
            self.jobs_due.insert((due, id.clone()));
        }
        // A job's INITIAL status is not a state change: a job state change event reports
        // a job the service already holds MOVING, so nothing is queued for the status a
        // submit stores. This job's stream starts at its first move out of it, which
        // `set_job_status`/`finish_job` record.
        Ok(())
    }

    /// Queue the job state change `id` has just taken at `at`, for publication to
    /// EventBridge.
    ///
    /// Called from the ONE place each status MOVE is written
    /// ([`Self::set_job_status`], [`Self::finish_job`]), so every transition a job
    /// makes — clock-owed or caller-driven — is recorded exactly once and none can
    /// be published twice. The status a job is CREATED in is not a transition and is
    /// never recorded here (`Self::insert_job`). The detail is rendered here, while the
    /// record still holds the status this change is about.
    fn record_job_state_change(&mut self, id: &str, at: i64) {
        let Some(job) = self.jobs.get(id) else {
            return;
        };
        let change = JobStateChange {
            job_arn: job.job_arn.clone(),
            status: job.status.clone(),
            at,
            detail: job_state_change_detail(job),
        };
        while self.pending_events.len() >= MAX_PENDING_EVENTS {
            self.pending_events.pop_front();
        }
        self.pending_events.push_back(change);
    }

    /// Take at most `limit` of the job state changes not yet published, oldest first.
    ///
    /// The caller publishes what it is handed (see [`publish_job_state_changes`]);
    /// taking the records out under the lock is what stops two concurrent requests
    /// from publishing the same change twice. The LIMIT is what keeps one drain from
    /// multiplying into an unbounded number of in-process dispatches: what it leaves
    /// behind keeps the clock pump awake (see [`Self::next_job_due_or_release_pump`])
    /// and is published by the next wake-up or call.
    pub fn take_pending_events(&mut self, limit: usize) -> Vec<JobStateChange> {
        let taking = limit.min(self.pending_events.len());
        self.pending_events.drain(..taking).collect()
    }

    /// The earliest instant at which this store owes a job transition, if any.
    ///
    /// Read from the due index, so it costs one lookup rather than a scan.
    pub fn next_job_due_at(&self) -> Option<i64> {
        self.jobs_due.iter().next().map(|(due, _)| *due)
    }

    /// What the clock pump waits for next, or `None` — having RELEASED its claim —
    /// when the store owes nothing.
    ///
    /// Observing "nothing is due" and standing down happen in ONE critical section,
    /// which is what makes the hand-over race-free: a `SubmitJob` holds this same
    /// lock to store its job (and so to add its due instant) BEFORE it tries to
    /// claim the pump, so either this call sees that due instant and the running
    /// pump keeps it, or the submit's claim finds the flag already cleared and
    /// starts a pump of its own. No job is left with an unpublished transition
    /// because a pump was standing down while it arrived.
    /// A change already recorded but not yet published keeps the pump awake too: one
    /// drain publishes at most [`MAX_PUBLISHED_PER_DRAIN`] of them, so the rest are
    /// owed another wake-up (`now`, i.e. as soon as the pump's minimum tick allows)
    /// rather than the next call.
    pub fn next_job_due_or_release_pump(&mut self, now: i64, generation: u64) -> Option<i64> {
        if !self.pending_events.is_empty() {
            return Some(now);
        }
        match self.next_job_due_at() {
            Some(due) => Some(due),
            None => {
                self.release_clock_pump(generation);
                None
            }
        }
    }

    /// Claim the single clock pump for this store: the claim's GENERATION when the
    /// caller may start one, `None` when one is already running.
    ///
    /// The generation is what makes a release safe: a pump releases its own claim
    /// and never a later pump's (see [`Self::release_clock_pump`]).
    pub fn start_clock_pump(&mut self) -> Option<u64> {
        if self.clock_pump_running {
            return None;
        }
        self.clock_pump_running = true;
        self.clock_pump_generation = self.clock_pump_generation.wrapping_add(1);
        Some(self.clock_pump_generation)
    }

    /// Release the claim `generation` took — and ONLY that one.
    ///
    /// A pump that ends on the wake-up cap must not clear a claim a pump started
    /// since is holding, or two pumps would run against one store; comparing the
    /// generation is what keeps "at most one pump per store" true on that path too.
    pub fn release_clock_pump(&mut self, generation: u64) {
        if self.clock_pump_generation == generation {
            self.clock_pump_running = false;
        }
    }

    /// Keep the running pump's task handle, aborting the one it replaces.
    ///
    /// The replaced handle is a pump that has already ended — by standing down, or
    /// on its wake-up cap — so the abort is normally a no-op; it is what guarantees
    /// the invariant anyway, that no earlier pump task is still running beside the
    /// one that now holds the claim.
    pub fn set_clock_pump(&mut self, handle: ::tokio::task::JoinHandle<()>) {
        if let Some(previous) = self.clock_pump.replace(handle) {
            previous.abort();
        }
    }

    /// Persist a service job, bounded by [`limits::MAX_SERVICE_JOBS`].
    ///
    /// Makes room first by evicting the service jobs that have been terminal longest — a
    /// terminal service job is one no later call can move again — and answers
    /// `Err(message)`, for the modelled `ClientException`, when every service job in a
    /// full store is still live.
    pub fn insert_service_job(&mut self, job: ServiceJob) -> Result<(), String> {
        if self.service_jobs.len() >= limits::MAX_SERVICE_JOBS {
            self.evict_terminal_service_jobs(
                self.service_jobs.len() + 1 - limits::MAX_SERVICE_JOBS,
            );
        }
        if self.service_jobs.len() >= limits::MAX_SERVICE_JOBS {
            return Err(quota_message("service jobs", limits::MAX_SERVICE_JOBS));
        }
        self.service_jobs.insert(job.job_id.clone(), job);
        Ok(())
    }

    /// The service job a `clientToken` already minted, if this store holds one.
    ///
    /// `SubmitServiceJob`'s `clientToken` "is used to ensure idempotency of requests", so
    /// a submit resolves its token here before minting anything. Scans the service jobs,
    /// which [`limits::MAX_SERVICE_JOBS`] bounds, so no second index can drift out of
    /// step with the records it would point at.
    pub fn service_job_by_client_token(&self, token: &str) -> Option<&ServiceJob> {
        self.service_jobs
            .values()
            .find(|job| job.client_token.as_deref() == Some(token))
    }

    /// Drop up to `wanted` of the service jobs that have been terminal longest.
    fn evict_terminal_service_jobs(&mut self, wanted: usize) {
        let mut evictable: Vec<(i64, String)> = self
            .service_jobs
            .values()
            .filter(|job| is_terminal_service_job_status(&job.status))
            .map(|job| (job.stopped_at.unwrap_or(job.created_at), job.job_id.clone()))
            .collect();
        evictable.sort();
        for (_, id) in evictable.into_iter().take(wanted) {
            self.service_jobs.remove(&id);
        }
    }

    /// Terminate the service job `id`, if the store holds one that can still be ended.
    ///
    /// A service job that has not reached a terminal `ServiceJobStatus` ends `FAILED` at
    /// `now`, carrying `reason` as its `statusReason` and `isTerminated` — the two members
    /// `DescribeServiceJob` documents for exactly this (`reason`: "A message to attach to
    /// the service job that explains the reason for terminating it. This message is
    /// returned by DescribeServiceJob operations on the service job."; `isTerminated`:
    /// "Indicates whether the service job has been terminated."). A service job already in
    /// a terminal status is left exactly as it is, so the first reason to end it is the one
    /// it keeps, and an `id` the store holds no service job for changes nothing at all.
    pub fn terminate_service_job(&mut self, id: &str, reason: &str, now: i64) {
        let Some(job) = self.service_jobs.get_mut(id) else {
            return;
        };
        if is_terminal_service_job_status(&job.status) {
            return;
        }
        job.status = SERVICE_JOB_FAILED.to_string();
        job.status_reason = Some(reason.to_string());
        job.stopped_at = Some(now);
        job.is_terminated = true;
        // Terminal: the clock owes the job nothing more.
        job.next_phase_due_at = None;
    }

    /// Terminate or cancel `id`, and answer whether the job exists.
    ///
    /// AWS Batch documents for `TerminateJob`: jobs in `SUBMITTED`, `PENDING` or
    /// `RUNNABLE` are cancelled, jobs that have progressed to `STARTING` or `RUNNING` are
    /// terminated; either way the job ends `FAILED` carrying `reason`. `STARTING` and
    /// `RUNNING` are modelled but unreachable here (nothing places a job, so a job rests
    /// in `RUNNABLE` until a caller ends it), and the rule is kept for both so the
    /// distinction holds the day a runtime exists. A job already in a terminal status is
    /// left exactly as it is.
    pub fn terminate_job(&mut self, id: &str, reason: &str, now: i64) -> bool {
        let Some(job) = self.jobs.get(id) else {
            return false;
        };
        if is_terminal_status(&job.status) {
            return true;
        }
        // "When you try to cancel an array parent job in PENDING, Batch attempts to cancel
        // all child jobs" (`CancelJob`): an ending a caller asks of an array PARENT is
        // asked of every child it spawned, whichever call asks it. A child has no children
        // of its own, so this goes one level deep.
        for child_id in self.array_child_ids(id) {
            self.terminate_job(&child_id, reason, now);
        }
        let running = self
            .jobs
            .get(id)
            .is_some_and(|job| matches!(job.status.as_str(), JOB_STARTING | JOB_RUNNING));
        if let Some(job) = self.jobs.get_mut(id) {
            job.is_terminated = running;
            job.is_cancelled = !running;
        }
        self.finish_job(id, JOB_FAILED, Some(reason), now);
        true
    }

    /// Cancel `id` with `reason`, and answer what that did to the job.
    ///
    /// AWS Batch documents for `CancelJob`: a job in `SUBMITTED`, `PENDING` or `RUNNABLE`
    /// is cancelled and ends `FAILED`; a job that progressed to `STARTING` or `RUNNING`
    /// "aren't cancelled" and must be terminated instead, "However, the API operation
    /// still succeeds, even if no job is cancelled". A job already in a terminal status is
    /// left exactly as it is, and a `PENDING` job's cancel is deferred to the clock (see
    /// [`Job::cancel_requested`]). Which of those happened is the answer, so the caller
    /// never has to re-derive it; `CancelJob` itself succeeds for all of them.
    pub fn cancel_job(&mut self, id: &str, reason: &str, now: i64) -> CancelOutcome {
        let Some(job) = self.jobs.get(id) else {
            return CancelOutcome::Unknown;
        };
        if is_terminal_status(&job.status) {
            return CancelOutcome::AlreadyTerminal;
        }
        // The children of an array parent are cancelled with it, as above.
        for child_id in self.array_child_ids(id) {
            self.cancel_job(&child_id, reason, now);
        }
        let Some(job) = self.jobs.get(id) else {
            return CancelOutcome::Unknown;
        };
        match job.status.as_str() {
            // A `PENDING` job is cancelled from the caller's point of view straight away
            // — AWS is recorded to report `isCancelled` on it while it is still `PENDING`
            // — but it only LEAVES `PENDING` once the jobs it waits on are done, so the
            // terminal status is the clock's to materialise (see [`Job::cancel_requested`]).
            JOB_PENDING => {
                if let Some(job) = self.jobs.get_mut(id) {
                    job.is_cancelled = true;
                    job.cancel_requested = Some(reason.to_string());
                }
                CancelOutcome::Deferred
            }
            JOB_SUBMITTED | JOB_RUNNABLE => {
                if let Some(job) = self.jobs.get_mut(id) {
                    job.is_cancelled = true;
                }
                self.finish_job(id, JOB_FAILED, Some(reason), now);
                CancelOutcome::Cancelled
            }
            // STARTING and RUNNING: cancelling does not touch them.
            _ => CancelOutcome::NotCancelled,
        }
    }

    /// Materialise the cancel [`Job::cancel_requested`] recorded for `id`: the job ends
    /// `FAILED` at `stopped_at` carrying the reason the cancel named.
    fn finish_cancelled(&mut self, id: &str, stopped_at: i64) {
        let reason = self
            .jobs
            .get(id)
            .and_then(|job| job.cancel_requested.clone())
            .unwrap_or_default();
        if let Some(job) = self.jobs.get_mut(id) {
            job.is_cancelled = true;
        }
        self.finish_job(id, JOB_FAILED, Some(&reason), stopped_at);
    }

    /// Set `id`'s status at `at`, leaving what the clock owes it alone.
    ///
    /// A job ENTERING `RUNNABLE` joins its queue's `RUNNABLE` ordering at `at`, which is
    /// what `GetJobQueueSnapshot` reports as its `earliestTimeAtPosition`
    /// ([`Job::at_position_since`]).
    fn set_job_status(&mut self, id: &str, status: &str, at: i64) {
        if let Some(job) = self.jobs.get_mut(id) {
            let entering_queue = status == JOB_RUNNABLE && job.status != JOB_RUNNABLE;
            job.status = status.to_string();
            job.status_since = at;
            if entering_queue {
                job.at_position_since = Some(at);
            }
        }
        self.record_job_state_change(id, at);
    }

    /// Whether the queue `arn` dispatches its `RUNNABLE` jobs by job priority before
    /// submission time, i.e. whether a FAIR-SHARE scheduling policy is attached to it.
    ///
    /// `JobDetail$schedulingPriority`: "This only affects jobs in job queues with a
    /// fair-share policy. Jobs with a higher scheduling priority are scheduled before jobs
    /// with a lower scheduling priority." A scheduling policy carries a fair-share policy
    /// or a quota-share policy and never both ([`SchedulingPolicy`]), and a quota share's
    /// only documented `idleResourceAssignmentStrategy` is `FIFO`, so priority orders the
    /// queues whose attached policy is NOT a quota-share one; a queue with no policy
    /// attached at all is first-in-first-out.
    fn queue_orders_by_priority(&self, arn: &str) -> bool {
        self.job_queue(arn)
            .and_then(|queue| queue.scheduling_policy_arn.as_deref())
            .and_then(|policy_arn| self.scheduling_policy(policy_arn))
            .is_some_and(|policy| policy.quota_share_policy.is_none())
    }

    /// The key a job is ordered by in its queue's `RUNNABLE` ordering: the job's own
    /// priority, NEGATED so the higher priority dispatches first, and nothing else.
    ///
    /// Submission order is what decides between the jobs of ONE priority — and so between
    /// the jobs of one share, which carry one priority unless a submit overrode it — and
    /// that order needs no key at all: [`Self::jobs_by_queue`] holds a queue's jobs in
    /// submission order and the sort below is stable, so equal keys come out as submitted,
    /// whatever resolution the clock reports. `GetJobQueueSnapshot`'s `frontOfQueue` doc:
    /// "For first-in-first-out (FIFO) job queues, jobs are ordered based on their
    /// submission time."
    ///
    /// The priority is read only for a queue that dispatches by it
    /// ([`Self::queue_orders_by_priority`]); every other queue is first-in-first-out. A job
    /// that names no `schedulingPriority` is ordered at the lowest the member documents
    /// ("The minimum supported value is 0", [`MIN_SCHEDULING_PRIORITY`]).
    fn queue_order_key(job: &Job, by_priority: bool) -> i32 {
        if by_priority {
            job.scheduling_priority
                .unwrap_or(MIN_SCHEDULING_PRIORITY)
                .saturating_neg()
        } else {
            0
        }
    }

    /// The `RUNNABLE` jobs of the queue `arn`, in the order the queue dispatches them.
    pub fn runnable_jobs_of_queue(&self, arn: &str) -> Vec<&Job> {
        let by_priority = self.queue_orders_by_priority(arn);
        let mut queued: Vec<&Job> = self
            .jobs_by_queue
            .get(arn)
            .into_iter()
            .flatten()
            .filter_map(|id| self.jobs.get(id))
            .filter(|job| job.status == JOB_RUNNABLE)
            .collect();
        // STABLE, so the submission order `jobs_by_queue` is kept in survives every tie.
        queued.sort_by_key(|job| Self::queue_order_key(job, by_priority));
        queued
    }

    /// Move every `RUNNABLE` job BEHIND `departed` in its queue to its new position at
    /// `at`: a job leaving the ordering is the only thing that advances the jobs after it,
    /// and each of those is now at a position it reached at this instant.
    ///
    /// "Behind" is read off the queue's own dispatch ordering, whatever decides it, so the
    /// jobs that move forward are the same ones the next snapshot reports after `departed`.
    fn advance_queue_positions(&mut self, departed: &str, at: i64) {
        let Some(gone) = self.jobs.get(departed) else {
            return;
        };
        let queue_arn = gone.job_queue_arn.clone();
        let ordered = self.runnable_jobs_of_queue(&queue_arn);
        let Some(position) = ordered.iter().position(|job| job.job_id == departed) else {
            return;
        };
        let behind: Vec<String> = ordered[position + 1..]
            .iter()
            .map(|job| job.job_id.clone())
            .collect();
        for id in behind {
            if let Some(job) = self.jobs.get_mut(&id) {
                job.at_position_since = Some(at);
            }
        }
    }

    /// Point `id`'s clock-owed transition at `due`, or clear it, keeping the due index in
    /// step so [`Self::advance_jobs`] stays proportional to what is due.
    fn set_job_due(&mut self, id: &str, due: Option<i64>) {
        let Some(job) = self.jobs.get_mut(id) else {
            return;
        };
        let previous = job.next_phase_due_at;
        job.next_phase_due_at = due;
        if let Some(previous) = previous {
            self.jobs_due.remove(&(previous, id.to_string()));
        }
        if let Some(due) = due {
            self.jobs_due.insert((due, id.to_string()));
        }
    }

    /// Put `id` in a terminal `status` at `ended_at` with `reason` as its
    /// `statusReason` (and its container's `reason`), and clear what the clock owed it.
    ///
    /// `reason` is `None` where nothing is known to report: a status AWS Batch sends with
    /// no `statusReason` leaves the member absent rather than carrying prose this emulator
    /// authored.
    ///
    /// `stoppedAt` is written only for a job that had actually STARTED: the member is
    /// documented as the `RUNNING` -> terminal transition, so a job nothing ran reports
    /// none ([`Job::stopped_at`]). [`Job::ended_at`] records the instant either way.
    ///
    /// Every route into a terminal status goes through here — the clock and `TerminateJob`
    /// alike — so this is where the jobs WAITING on `id` are woken: each one that is still
    /// waiting gets this instant as its due instant, which puts it back in the due index,
    /// and the next `materialize_due` recomputes what its dependencies now allow. One
    /// release path, both callers.
    fn finish_job(&mut self, id: &str, status: &str, reason: Option<&str>, ended_at: i64) {
        self.set_job_due(id, None);
        // A job leaving the queue's `RUNNABLE` ordering moves the jobs behind it forward,
        // so each of those reaches its new position at this instant. Read BEFORE the
        // status is overwritten, which is what tells us the job was in that ordering.
        let was_queued = self
            .jobs
            .get(id)
            .is_some_and(|job| job.status == JOB_RUNNABLE);
        if was_queued {
            self.advance_queue_positions(id, ended_at);
        }
        if let Some(job) = self.jobs.get_mut(id) {
            job.status = status.to_string();
            job.status_since = ended_at;
            job.status_reason = reason.map(str::to_string);
            job.ended_at = Some(ended_at);
            job.at_position_since = None;
            if job.started_at.is_some() {
                job.stopped_at = Some(ended_at);
            }
            if let Some(container) = job.container.as_mut() {
                container.reason = reason.map(str::to_string);
            }
            // An attempt still open when the job ends ends WITH it, carrying the same
            // reason: whatever took the job terminal ended the attempt it was running
            // ([`end_open_attempt`]).
            end_open_attempt(job, reason, ended_at);
        }
        self.record_job_state_change(id, ended_at);
        let waiting = self.dependents.get(id).cloned().unwrap_or_default();
        for dependent in waiting {
            let still_waiting = self.jobs.get(&dependent).is_some_and(|job| {
                job.status == JOB_PENDING && job.next_phase_due_at.is_none()
            });
            if still_waiting {
                self.set_job_due(&dependent, Some(ended_at));
            }
        }
    }

    /// Drop up to `wanted` of the jobs that have been terminal longest, with every index
    /// entry that named them. A job a live job still depends on is kept, so a dependency
    /// never vanishes from under a dependent that is still waiting for it, and so is the
    /// child of an array parent that is still live, whose status the parent's own summary
    /// is counted from ([`Self::array_status_summary`]).
    fn evict_terminal_jobs(&mut self, wanted: usize) {
        let mut evictable: Vec<(i64, String)> = self
            .jobs
            .values()
            .filter(|job| {
                is_terminal_status(&job.status)
                    && !self.has_live_dependent(&job.job_id)
                    && !self.is_child_of_live_array_parent(job)
            })
            .map(|job| (job.ended_at.unwrap_or(job.created_at), job.job_id.clone()))
            .collect();
        evictable.sort();
        for (_, id) in evictable.into_iter().take(wanted) {
            self.remove_job(&id);
        }
    }

    /// Whether a job that is not terminal still waits on `id`.
    fn has_live_dependent(&self, id: &str) -> bool {
        self.dependents.get(id).is_some_and(|waiting| {
            waiting.iter().any(|dependent| {
                self.jobs
                    .get(dependent)
                    .is_some_and(|job| !is_terminal_status(&job.status))
            })
        })
    }

    /// Forget `id` and every index entry that named it.
    fn remove_job(&mut self, id: &str) {
        let Some(job) = self.jobs.remove(id) else {
            return;
        };
        if let Some(due) = job.next_phase_due_at {
            self.jobs_due.remove(&(due, id.to_string()));
        }
        if let Some(ids) = self.jobs_by_queue.get_mut(&job.job_queue_arn) {
            ids.retain(|candidate| candidate != id);
        }
        self.dependents.remove(id);
        for dependency in job.depends_on.iter().flatten() {
            if let Some(dependency_id) = dependency.job_id.as_deref() {
                if let Some(waiting) = self.dependents.get_mut(dependency_id) {
                    waiting.retain(|candidate| candidate != id);
                    // The key goes with its last edge, so the index holds nothing for a
                    // job that is gone.
                    if waiting.is_empty() {
                        self.dependents.remove(dependency_id);
                    }
                }
            }
        }
    }

    /// What the jobs `id` depends on allow right now.
    fn dependency_outcome(&self, id: &str, now: i64) -> DependencyOutcome {
        let Some(job) = self.jobs.get(id) else {
            return DependencyOutcome::Waiting;
        };
        let mut latest = job.created_at;
        for dependency in job.depends_on.iter().flatten() {
            let Some(dependency_id) = dependency.job_id.as_deref() else {
                continue;
            };
            let Some(dependency) = self.jobs.get(dependency_id) else {
                return DependencyOutcome::Unresolvable;
            };
            let stopped = dependency.ended_at.unwrap_or(now);
            match dependency.status.as_str() {
                JOB_SUCCEEDED => latest = latest.max(stopped),
                JOB_FAILED => return DependencyOutcome::Failed(stopped),
                _ => return DependencyOutcome::Waiting,
            }
        }
        DependencyOutcome::Satisfied(latest)
    }
}

/// The job definition NAME a reference carries: the name part of an ARN
/// (`.../job-definition/<name>:<revision>`) or of a `<name>:<revision>` short form, and
/// the reference itself when it is a bare name.
pub fn job_definition_reference_name(reference: &str) -> &str {
    let short = arn_resource_id(reference, "job-definition").unwrap_or(reference);
    short.rsplit_once(':').map_or(short, |(name, _)| name)
}

/// Refuse a `jobStateTimeLimitActions` list that names a `maxTimeSeconds` outside the
/// documented bounds; answers the refusal detail of the first offending entry.
///
/// Shared by every operation that accepts the list (`CreateJobQueue`, `UpdateJobQueue`),
/// so one rule covers all of them.
pub fn validate_job_state_time_limit_actions(
    actions: &[JobStateTimeLimitAction],
) -> Result<(), String> {
    for action in actions {
        if !(MIN_JOB_STATE_TIME_LIMIT_SECONDS..=MAX_JOB_STATE_TIME_LIMIT_SECONDS)
            .contains(&action.max_time_seconds)
        {
            return Err(format!(
                "jobStateTimeLimitActions maxTimeSeconds must be between {MIN_JOB_STATE_TIME_LIMIT_SECONDS} and {MAX_JOB_STATE_TIME_LIMIT_SECONDS}, got {}",
                action.max_time_seconds
            ));
        }
    }
    Ok(())
}

/// Whether `value` is a scheduling policy `name` the service accepts: up to
/// [`MAX_SCHEDULING_POLICY_NAME_LEN`] letters, numbers, hyphens and underscores
/// (the member's own doc). See [`is_valid_resource_name`].
pub fn is_valid_scheduling_policy_name(value: &str) -> bool {
    is_valid_resource_name(value, MAX_SCHEDULING_POLICY_NAME_LEN)
}

/// Whether `value` is the ARN of a scheduling policy this endpoint serves.
///
/// The ARN is `arn:<partition>:batch:<region>:<account>:scheduling-policy/<name>`, the
/// form `SchedulingPolicyDetail$arn` documents, and it is served only when it is the one
/// this service mints for that name in `region` ([`scheduling_policy_arn`]): a policy in
/// another region, account or partition is not this endpoint's to describe. An ARN of
/// another resource kind, or of no kind, names no policy at all.
pub fn is_scheduling_policy_arn(region: &str, value: &str) -> bool {
    parse_scheduling_policy_arn(value).is_some_and(|name| {
        is_valid_scheduling_policy_name(name) && value == scheduling_policy_arn(region, name)
    })
}

/// Whether `value` can name a scheduling policy this endpoint serves.
///
/// Either shape a scheduling-policy identifier has: the ARN
/// ([`is_scheduling_policy_arn`]) or the `<name>` inside it. Anything else — an ARN of
/// another resource kind or of no kind, an empty identifier, a value outside the charset
/// the `name` member documents — names no policy at all, which is a DIFFERENT answer from
/// an identifier of the right shape naming nothing stored (that one is reported as absent,
/// or is already deleted); the refusal is [`scheduling_policy_arn_not_valid`].
///
/// The WRITERS (`DeleteSchedulingPolicy`, `UpdateSchedulingPolicy`) resolve both shapes,
/// the way every other identifier in this service resolves: their `arn` member is
/// documented as an ARN, but no recorded response refuses a bare name there, and this
/// service resolves "name or ARN" everywhere a refusal is not recorded.
/// `DescribeSchedulingPolicies` is the exception: its `arns` is "a list of up to 100
/// scheduling policy Amazon Resource Name (ARN) entries" AND a describe naming a policy by
/// name is recorded as refused, so that reader checks [`is_scheduling_policy_arn`] instead
/// of this.
pub fn is_scheduling_policy_reference(region: &str, value: &str) -> bool {
    is_scheduling_policy_arn(region, value) || is_valid_scheduling_policy_name(value)
}

/// Every `QuotaShareIdleResourceAssignmentStrategy` the model declares
/// (`smithy_query(query="shape", shape="QuotaShareIdleResourceAssignmentStrategy")`).
/// The generated input types carry the member as a plain `String`, so the value is
/// checked here by every operation that accepts a quota-share policy.
pub const QUOTA_SHARE_IDLE_RESOURCE_ASSIGNMENT_STRATEGIES: [&str; 1] = ["FIFO"];

/// The member a quota-share policy's idle-resource assignment strategy arrives on, which
/// is the member an enum-membership refusal of it names ([`enum_membership_detail`]).
pub const IDLE_RESOURCE_ASSIGNMENT_STRATEGY_MEMBER: &str = "idleResourceAssignmentStrategy";

/// The name of the member a fair-share scheduling policy is carried on, which is the name
/// AWS Batch uses for that policy KIND in the refusal of a type change
/// ([`policy_kind_change_message`]).
pub const FAIRSHARE_POLICY_KIND: &str = "fairsharePolicy";

/// The name of the member a quota-share scheduling policy is carried on — the other kind
/// a scheduling policy can have ([`FAIRSHARE_POLICY_KIND`]).
pub const QUOTA_SHARE_POLICY_KIND: &str = "quotaSharePolicy";

/// What an update that would change a scheduling policy's KIND is refused with: the kind
/// the stored policy has, the kind the request asked for, and what to do instead.
///
/// "Once set during creation, a fairsharePolicy cannot be removed or changed to a
/// quotaSharePolicy" (`UpdateSchedulingPolicy$fairsharePolicy`), and the mirror sentence on
/// `$quotaSharePolicy`: the kind a policy was created with is the kind it keeps, and the
/// only way to the other kind is a new policy. The refusal is built from the request, so it
/// states WHICH direction was refused rather than the rule in the abstract.
pub fn policy_kind_change_message(from: &str, to: &str) -> String {
    format!(
        "Cannot change scheduling policy type from {from} to {to}. Create a new scheduling \
         policy instead."
    )
}

/// The detail AWS Batch refuses a scheduling policy carrying BOTH kinds with: the fixed
/// sentence for the rule its two members' docs state ("Only one of fairsharePolicy or
/// quotaSharePolicy can be set"), naming the two members and nothing from the request.
pub const POLICY_KINDS_EXCLUSIVE: &str =
    "A scheduling policy can have either fairsharePolicy or quotaSharePolicy, but not both.";

/// The refusal when a request carries a policy of neither kind or of both, or a policy
/// outside the bounds its own members document.
///
/// "Only one of fairsharePolicy or quotaSharePolicy can be set" — the doc of
/// `CreateSchedulingPolicy$fairsharePolicy` and `$quotaSharePolicy`. Shared by every
/// operation that accepts the pair (`CreateSchedulingPolicy`, `UpdateSchedulingPolicy`).
///
/// Each refusal carries the envelope AWS is recorded to word IT with, chosen where it is
/// raised: the exclusivity rule and an enum-membership refusal are the frontend's, so both
/// carry its envelope ([`client_exception_enveloped`]), while the fair-share bounds state
/// their detail alone ([`client_exception`]).
pub fn validate_policy_kinds(
    fairshare: Option<&FairsharePolicy>,
    quota_share: Option<&QuotaSharePolicy>,
) -> Result<(), ClientException> {
    if fairshare.is_some() && quota_share.is_some() {
        return Err(client_exception_enveloped(POLICY_KINDS_EXCLUSIVE));
    }
    if let Some(quota_share) = quota_share {
        validate_quota_share_policy(quota_share)?;
    }
    if let Some(fairshare) = fairshare {
        validate_fairshare_policy(fairshare).map_err(client_exception)?;
    }
    Ok(())
}

/// Refuse a `quotaSharePolicy` whose `idleResourceAssignmentStrategy` is not one the
/// model declares ([`QUOTA_SHARE_IDLE_RESOURCE_ASSIGNMENT_STRATEGIES`]), with the sentence
/// and the frontend envelope every enum-membership refusal of this service carries
/// ([`enum_membership_detail`]).
///
/// A value sent on the WIRE is refused by the boundary validator before any handler runs —
/// the member is an enum in the model, so the generated schema carries its value set — and
/// the refusal seam (`crate::service`) words that one from the same builder. This check is
/// what refuses a policy reaching the rule from anywhere else.
pub fn validate_quota_share_policy(policy: &QuotaSharePolicy) -> Result<(), ClientException> {
    if !QUOTA_SHARE_IDLE_RESOURCE_ASSIGNMENT_STRATEGIES
        .contains(&policy.idle_resource_assignment_strategy.as_str())
    {
        return Err(client_exception_enveloped(enum_membership_detail(
            IDLE_RESOURCE_ASSIGNMENT_STRATEGY_MEMBER,
            &QUOTA_SHARE_IDLE_RESOURCE_ASSIGNMENT_STRATEGIES,
        )));
    }
    Ok(())
}

/// Refuse a `fairsharePolicy` outside the bounds its members document:
/// `shareDecaySeconds` ([`MAX_SHARE_DECAY_SECONDS`]), `computeReservation`
/// ([`MAX_COMPUTE_RESERVATION`]), each `shareIdentifier`'s charset and length
/// ([`MAX_SHARE_IDENTIFIER_LEN`]), each `weightFactor` ([`MIN_WEIGHT_FACTOR`]), and the
/// no-overlap rule between share identifiers.
///
/// The model types every one of these members as a plain `Integer`/`Float`/`String`, so
/// the framework admits anything and the rule is applied by every operation that accepts
/// the policy (`CreateSchedulingPolicy`, `UpdateSchedulingPolicy`).
pub fn validate_fairshare_policy(policy: &FairsharePolicy) -> Result<(), String> {
    if let Some(decay) = policy.share_decay_seconds {
        if !(MIN_SHARE_DECAY_SECONDS..=MAX_SHARE_DECAY_SECONDS).contains(&decay) {
            return Err(format!(
                "fairsharePolicy shareDecaySeconds must be between {MIN_SHARE_DECAY_SECONDS} and {MAX_SHARE_DECAY_SECONDS}, got {decay}"
            ));
        }
    }
    if let Some(reservation) = policy.compute_reservation {
        if !(MIN_COMPUTE_RESERVATION..=MAX_COMPUTE_RESERVATION).contains(&reservation) {
            return Err(format!(
                "fairsharePolicy computeReservation must be between {MIN_COMPUTE_RESERVATION} and {MAX_COMPUTE_RESERVATION}, got {reservation}"
            ));
        }
    }
    let distribution = policy.share_distribution.as_deref().unwrap_or_default();
    // The ENTRY COUNT is bounded before any work proportional to it: the list comes
    // straight from the request, so a longer one is refused rather than walked. The same
    // sentence every other stored list is kept to ([`validate_stored_list_length`]), so the
    // wording cannot drift between them.
    validate_stored_list_length(
        "fairsharePolicy shareDistribution",
        distribution.len(),
        MAX_SHARE_DISTRIBUTION_ENTRIES,
    )?;
    for share in distribution {
        if !is_valid_share_identifier(&share.share_identifier) {
            return Err(format!(
                "shareIdentifier {} must be 1 to {MAX_SHARE_IDENTIFIER_LEN} alphanumeric characters, optionally followed by an asterisk (*)",
                share.share_identifier
            ));
        }
        if let Some(weight) = share.weight_factor {
            if !(MIN_WEIGHT_FACTOR..=MAX_WEIGHT_FACTOR).contains(&weight) {
                return Err(format!(
                    "weightFactor of shareIdentifier {} must be between {MIN_WEIGHT_FACTOR} and {MAX_WEIGHT_FACTOR}, got {weight}",
                    share.share_identifier
                ));
            }
        }
    }
    if let Some((left, right)) = overlapping_share_identifiers(distribution) {
        return Err(format!(
            "The list of share identifiers in a fair-share policy can't overlap: {left} and {right}"
        ));
    }
    Ok(())
}

/// Whether `value` is a `shareIdentifier` the service accepts: at most
/// [`MAX_SHARE_IDENTIFIER_LEN`] alphanumeric characters, optionally followed by a single
/// asterisk which makes the entry a PREFIX (the member's own doc).
pub fn is_valid_share_identifier(value: &str) -> bool {
    let prefix = value.strip_suffix('*').unwrap_or(value);
    !prefix.is_empty()
        && prefix.chars().count() <= MAX_SHARE_IDENTIFIER_LEN
        && prefix.chars().all(|c| c.is_ascii_alphanumeric())
}

/// A pair of overlapping share identifiers in `distribution`, or `None`.
///
/// Two entries overlap when one selects everything the other does: the same identifier
/// twice, or a prefix entry (`<prefix>*`) and any entry that starts with that prefix —
/// the example the `shareIdentifier` doc gives ("you can't have one that specifies a
/// shareIdentifier of UserA* and another that specifies a shareIdentifier of UserA1").
///
/// SORTED rather than pairwise, so the cost is `n log n` comparisons and not `n²`: in
/// identifier order every entry that starts with a given prefix follows it contiguously,
/// so an overlapping pair always leaves an ADJACENT overlapping pair behind — if `a` is a
/// prefix of `c` then every `b` between them has `a` as a prefix too, and an overlap
/// between two different identifiers is only ever an earlier PREFIX entry against a later
/// one. Checking neighbours therefore finds an overlap exactly when one exists.
fn overlapping_share_identifiers(distribution: &[ShareAttributes]) -> Option<(String, String)> {
    // The identifier an entry selects on: the string without the `*` that makes it a
    // prefix. The full string breaks ties so the answer does not depend on input order.
    let sort_key = |value: &str| {
        (
            value.strip_suffix('*').unwrap_or(value).to_string(),
            value.to_string(),
        )
    };
    let mut sorted: Vec<&str> = distribution
        .iter()
        .map(|share| share.share_identifier.as_str())
        .collect();
    sorted.sort_unstable_by_key(|value| sort_key(value));
    sorted.windows(2).find_map(|pair| {
        share_identifiers_overlap(pair[0], pair[1])
            .then(|| (pair[0].to_string(), pair[1].to_string()))
    })
}

/// Whether two `shareIdentifier` entries select a common share identifier.
fn share_identifiers_overlap(left: &str, right: &str) -> bool {
    match (left.strip_suffix('*'), right.strip_suffix('*')) {
        // Two prefixes overlap when either contains the other.
        (Some(left), Some(right)) => left.starts_with(right) || right.starts_with(left),
        // A prefix and an exact identifier overlap when the identifier matches it.
        (Some(prefix), None) => right.starts_with(prefix),
        (None, Some(prefix)) => left.starts_with(prefix),
        // Two exact identifiers overlap only when they are the same.
        (None, None) => left == right,
    }
}

/// The two forms a job-definition reference takes, parsed once: the NAME it carries
/// and the revision when it names one.
///
/// The syntax is what every `jobDefinition` member documents — a bare name, a
/// `<name>:<revision>` short form, or a full ARN of either shape — so the
/// resolvers above differ only in what they SELECT (the latest active revision, or
/// the latest stored one) rather than each re-parsing the reference. `None` when a
/// revision is present but is not a number, which no stored revision answers to.
fn parse_job_definition_reference(reference: &str) -> Option<(&str, Option<i32>)> {
    let short = arn_resource_id(reference, "job-definition").unwrap_or(reference);
    match short.rsplit_once(':') {
        Some((name, revision)) => Some((name, Some(revision.parse().ok()?))),
        None => Some((short, None)),
    }
}

/// The tag map a `Describe*` reader reports for a record whose `tags` are `tags`.
///
/// AWS Batch's tag plane MATERIALISES the map: a resource created with no tags is
/// described with `tags` present and EMPTY, never with the member missing (the same map
/// `ListTagsForResource` answers for it). A typed client therefore reads
/// `detail.tags()` without a null guard, so every reader in this service reports the map
/// through here rather than passing the stored `Option` straight out.
pub fn materialized_tags(tags: &Option<HashMap<String, String>>) -> Option<HashMap<String, String>> {
    Some(tags.clone().unwrap_or_default())
}

/// The `fairsharePolicy` `DescribeSchedulingPolicies` reports for a policy whose stored
/// policies are `fairshare` and `quota_share`.
///
/// AWS Batch materialises a scheduling policy's fair-share policy the way it materialises
/// a tag map: a FAIR-SHARE policy is described with all three of its members present,
/// zero-valued where the create left them out and with each distribution entry carrying
/// its `weightFactor`, and a policy created with NEITHER kind is described carrying the
/// zero-valued fair-share policy. Nothing is invented by it: each value is the member's own
/// documented default (zero for the decay, "A value of zero (0) indicates the default
/// minimum time window"; zero for the reservation, "The minimum value is 0"; `1.0` for a
/// weight factor, "The default value is 1.0"), and a policy that shares nothing
/// distributes nothing.
///
/// A QUOTA-SHARE policy carries no fair-share policy at all: "Only one of fairsharePolicy
/// or quotaSharePolicy can be set" (`CreateSchedulingPolicy$fairsharePolicy`), so adding a
/// zero-valued one would describe a policy of both kinds — one this service's own
/// [`validate_policy_kinds`] refuses, and one no caller could round-trip back into a
/// create or an update.
pub fn materialized_fairshare_policy(
    fairshare: Option<&FairsharePolicy>,
    quota_share: Option<&QuotaSharePolicy>,
) -> Option<FairsharePolicy> {
    if fairshare.is_none() && quota_share.is_some() {
        return None;
    }
    Some(FairsharePolicy {
        share_decay_seconds: Some(
            fairshare
                .and_then(|policy| policy.share_decay_seconds)
                .unwrap_or(0),
        ),
        compute_reservation: Some(
            fairshare
                .and_then(|policy| policy.compute_reservation)
                .unwrap_or(0),
        ),
        share_distribution: Some(
            fairshare
                .and_then(|policy| policy.share_distribution.clone())
                .unwrap_or_default()
                .into_iter()
                .map(|entry| ShareAttributes {
                    share_identifier: entry.share_identifier,
                    weight_factor: Some(entry.weight_factor.unwrap_or(DEFAULT_WEIGHT_FACTOR)),
                })
                .collect(),
        ),
    })
}

/// A `ClientException` whose message is `detail` exactly as the code raising it states
/// it — the 400 an unknown, duplicate, invalid or over-quota request answers with.
///
/// AWS Batch does NOT wrap every refusal in its frontend envelope: the recorded
/// refusals of `CreateJobQueue` ([`COMPUTE_ENVIRONMENTS_NOT_ATTACHABLE`]),
/// `CreateComputeEnvironment` ([`OBJECT_ALREADY_EXISTS`]) and `DescribeJobDefinitions`
/// ([`NEXT_TOKEN_NOT_PROCESSABLE`]) carry the detail alone. So the envelope is chosen
/// per refusal, at the site that raises it ([`client_exception_enveloped`]), rather than
/// applied to all of them here.
pub fn client_exception(detail: impl AsRef<str>) -> ClientException {
    ClientException {
        message: Some(detail.as_ref().to_string()),
    }
}

/// A `ClientException` whose message is `detail` inside Batch's frontend envelope
/// ([`client_exception_message`]) — for the refusals AWS is recorded to wrap, such as
/// `RegisterJobDefinition`'s missing properties object and the quota-share refusals.
pub fn client_exception_enveloped(detail: impl AsRef<str>) -> ClientException {
    ClientException {
        message: Some(client_exception_message(detail.as_ref(), &new_request_id())),
    }
}

/// The `ServerException` for an engine that did not hand back this service's store.
pub fn unreachable_state() -> ServerException {
    ServerException {
        message: Some(UNREACHABLE_STATE.to_string()),
    }
}

/// A `ClientException` for a reference that resolves to no resource, worded the way the
/// Batch service layer words a not-found: `<Kind> <reference> not found.`
///
/// This is the OTHER wording a Batch `ClientException` carries. A refusal the frontend
/// raises is wrapped by [`client_exception_message`]; a lookup the service itself performs
/// and misses answers this bare, request-derived sentence naming the kind of resource and
/// the reference the caller gave — the observed wording of, for instance,
/// `SubmitServiceJob` against a `jobQueue` nothing answers to. The rule is the sentence,
/// not any one reference: the caller's own text is what fills it.
pub fn not_found_exception(kind: &str, reference: &str) -> ClientException {
    ClientException {
        message: Some(format!("{kind} {reference} not found.")),
    }
}

/// The detail a tagging call's `resourceArn` is refused with when the ARN is well formed
/// and names no resource.
///
/// One fixed sentence for the one condition, carrying nothing from the request — the twin
/// of [`BAD_RESOURCE_ARN_MESSAGE`]: the tag layer states WHICH of its two refusals this is
/// and echoes neither the ARN nor the kind of resource it would have named, where a miss
/// the SERVICE layer finds names the reference it was given ([`not_found_exception`]).
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The refusal a tagging operation answers a well-formed `resourceArn` that resolves to no
/// stored resource with.
///
/// The tagging endpoint classifies its own refusals, and the two it can raise about a
/// `resourceArn` are different failures: a value that is not an ARN at all is a malformed
/// request ([`malformed_resource_arn`]), while a well-formed ARN naming nothing is a
/// MISSING resource, which that layer answers under
/// [`crate::scaffolded::TAGGING_NOT_FOUND_CODE`] over
/// [`RESOURCE_NOT_FOUND_MESSAGE`] — so a caller tearing a resource down can tell "already
/// gone" from "that is not an ARN" by the code alone.
///
/// Shared by `ListTagsForResource`, `TagResource` and `UntagResource` so one endpoint
/// answers one refusal, and generic over the operation's modeled error enum so each one
/// returns it through its own error channel.
pub fn unknown_resource_arn<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::tagging_not_found(RESOURCE_NOT_FOUND_MESSAGE)
}

/// The refusal a tagging operation answers a request whose resultant tag set would exceed
/// the ceiling with ([`tag_set_limit_message`]).
///
/// Raised under the tagging endpoint's own code, as the boundary validator's refusal of a
/// `tags` map longer than `TagrisTagsMap` declares is (`crate::service`): one rule, one
/// code and one sentence, whether the request alone breaks the ceiling or the merge into
/// what the resource already holds does.
pub fn tag_set_limit_exceeded<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::tagging_refusal(tag_set_limit_message())
}

/// The detail a tagging call's `resourceArn` is refused with when the value is not an ARN
/// at all.
///
/// One fixed sentence for the one condition, carrying nothing from the request: the tag
/// frontend reads the value AS an ARN before it resolves anything, so there is no
/// resource, kind or reference for it to name yet.
pub const BAD_RESOURCE_ARN_MESSAGE: &str = "Invalid input resource arn: bad syntax";

/// The refusal a tagging operation answers a `resourceArn` that is not an ARN with, and
/// `None` for a value that IS one (whether or not it names a resource this service holds).
///
/// Resolving a tagging call's `resourceArn` is the tag frontend's work, not this
/// service's, so a value of the wrong SHAPE is refused there, under that frontend's code
/// ([`crate::scaffolded::TAGGING_REFUSAL_CODE`], HTTP 400) — the same code it refuses a
/// tag MAP outside its declared bounds with — and never reaches this service's lookup. An
/// ARN the lookup then misses is that layer's not-found instead ([`unknown_resource_arn`]):
/// a well-formed reference to a resource that is not there.
///
/// Shared by `ListTagsForResource`, `TagResource` and `UntagResource` so one endpoint
/// answers one refusal, and generic over the operation's modeled error enum so each one
/// returns it through its own error channel.
pub fn malformed_resource_arn<E>(resource_arn: &str) -> Option<crate::scaffolded::Scaffolded<E>> {
    let is_arn = arn_resource(resource_arn).is_some_and(|resource| !resource.is_empty());
    (!is_arn).then(|| crate::scaffolded::Scaffolded::tagging_refusal(BAD_RESOURCE_ARN_MESSAGE))
}

/// The container a job actually gets: the definition's `containerProperties` with
/// the submit request's `containerOverrides` applied over it.
///
/// `command`, `vcpus`, `memory` and `instanceType` are replaced by the override
/// when it carries one; `environment` and `resourceRequirements` are merged
/// entry-wise (an override entry replaces the definition's entry of the same
/// name/type and is otherwise added), which is what AWS Batch documents for them.
pub fn effective_container(
    properties: Option<&ContainerProperties>,
    overrides: Option<&ContainerOverrides>,
) -> Option<ContainerDetail> {
    let properties = properties?;
    let mut detail = ContainerDetail {
        image: properties.image.clone(),
        vcpus: properties.vcpus,
        memory: properties.memory,
        command: properties.command.clone(),
        job_role_arn: properties.job_role_arn.clone(),
        execution_role_arn: properties.execution_role_arn.clone(),
        volumes: properties.volumes.clone(),
        environment: properties.environment.clone(),
        mount_points: properties.mount_points.clone(),
        readonly_root_filesystem: properties.readonly_root_filesystem,
        ulimits: properties.ulimits.clone(),
        privileged: properties.privileged,
        user: properties.user.clone(),
        instance_type: properties.instance_type.clone(),
        resource_requirements: properties.resource_requirements.clone(),
        linux_parameters: properties.linux_parameters.clone(),
        log_configuration: properties.log_configuration.clone(),
        secrets: properties.secrets.clone(),
        network_configuration: properties.network_configuration.clone(),
        fargate_platform_configuration: properties.fargate_platform_configuration.clone(),
        ephemeral_storage: properties.ephemeral_storage.clone(),
        runtime_platform: properties.runtime_platform.clone(),
        repository_credentials: properties.repository_credentials.clone(),
        enable_execute_command: properties.enable_execute_command,
        ..ContainerDetail::default()
    };
    if let Some(overrides) = overrides {
        if let Some(command) = overrides.command.as_ref().filter(|it| !it.is_empty()) {
            detail.command = Some(command.clone());
        }
        if let Some(vcpus) = overrides.vcpus {
            detail.vcpus = Some(vcpus);
        }
        if let Some(memory) = overrides.memory {
            detail.memory = Some(memory);
        }
        if let Some(instance_type) = overrides.instance_type.as_ref() {
            detail.instance_type = Some(instance_type.clone());
        }
        if let Some(environment) = overrides.environment.as_deref() {
            detail.environment = Some(merged_environment(
                detail.environment.as_deref(),
                environment,
            ));
        }
        if let Some(requirements) = overrides.resource_requirements.as_deref() {
            detail.resource_requirements = Some(merged_requirements(
                detail.resource_requirements.as_deref(),
                requirements,
            ));
        }
    }
    Some(detail)
}

/// `base` with every `overrides` entry replacing the entry of the same name, or
/// appended when there is none.
fn merged_environment(base: Option<&[KeyValuePair]>, overrides: &[KeyValuePair]) -> Vec<KeyValuePair> {
    let mut merged = base.unwrap_or_default().to_vec();
    for pair in overrides {
        match merged.iter_mut().find(|entry| entry.name == pair.name) {
            Some(entry) => entry.value = pair.value.clone(),
            None => merged.push(pair.clone()),
        }
    }
    merged
}

/// `base` with every `overrides` requirement replacing the requirement of the
/// same type, or appended when there is none.
fn merged_requirements(
    base: Option<&[ResourceRequirement]>,
    overrides: &[ResourceRequirement],
) -> Vec<ResourceRequirement> {
    let mut merged = base.unwrap_or_default().to_vec();
    for requirement in overrides {
        match merged
            .iter_mut()
            .find(|entry| entry.r#type == requirement.r#type)
        {
            Some(entry) => entry.value = requirement.value.clone(),
            None => merged.push(requirement.clone()),
        }
    }
    merged
}

/// The job definition's parameter defaults with the submit request's parameters
/// applied over them, as AWS Batch documents for `SubmitJob$parameters`.
pub fn merged_parameters(
    defaults: Option<&HashMap<String, String>>,
    overrides: Option<&HashMap<String, String>>,
) -> Option<HashMap<String, String>> {
    if defaults.is_none() && overrides.is_none() {
        return None;
    }
    let mut merged = defaults.cloned().unwrap_or_default();
    if let Some(overrides) = overrides {
        for (key, value) in overrides {
            merged.insert(key.clone(), value.clone());
        }
    }
    Some(merged)
}

/// Whether `candidate` matches a listing filter `value` the way the `JOB_NAME` filter is
/// documented: case-insensitively, and matching any candidate that begins with the text
/// before a trailing `*` ("If the value ends with an asterisk (*), the filter matches any
/// job name that begins with the string before the '*'").
///
/// Shared by every listing operation that documents that filter, so the two spell one
/// rule once.
pub fn matches_name_filter(candidate: &str, value: &str) -> bool {
    match value.strip_suffix('*') {
        // Compared byte-wise rather than through two lowercased copies: a job name is
        // ASCII (`^[A-Za-z0-9][A-Za-z0-9_-]*$`), and `is_char_boundary` keeps the slice
        // valid for anything else.
        Some(prefix) => {
            candidate.len() >= prefix.len()
                && candidate.is_char_boundary(prefix.len())
                && candidate[..prefix.len()].eq_ignore_ascii_case(prefix)
        }
        None => candidate.eq_ignore_ascii_case(value),
    }
}

/// The instants a `BEFORE_CREATED_AT` or `AFTER_CREATED_AT` filter's `values` name, from
/// the filter's own doc: each is "a string representation of the number of milliseconds
/// since 00:00:00 UTC (midnight) on January 1, 1970".
///
/// A value that is not one is the message a `ClientException` carries, built from the
/// filter name and the value the request gave.
pub fn created_at_filter_bounds(name: &str, values: &[String]) -> Result<Vec<i64>, String> {
    values
        .iter()
        .map(|value| {
            value.parse::<i64>().map_err(|_| {
                format!(
                    "The {name} filter value {value} is not a number of milliseconds since the epoch"
                )
            })
        })
        .collect()
}

/// Whether a record created at `created_at` passes the created-at filter `name` with
/// `bounds`: `BEFORE_CREATED_AT` keeps what was created no later than a bound,
/// `AFTER_CREATED_AT` what was created no earlier than one.
pub fn matches_created_at_filter(name: &str, created_at: i64, bounds: &[i64]) -> bool {
    if name == "BEFORE_CREATED_AT" {
        bounds.iter().any(|bound| created_at <= *bound)
    } else {
        bounds.iter().any(|bound| created_at >= *bound)
    }
}

/// One page of `items`, plus the `nextToken` that resumes after it.
///
/// The token is the index the next page starts at, so a caller that pages to the
/// end sees every item exactly once. An unparseable or out-of-range token is an
/// error the caller gets as a `ClientException`, worded the way AWS Batch words an
/// unusable `nextToken` ([`NEXT_TOKEN_NOT_PROCESSABLE`]): the detail states that the token
/// could not be processed and echoes nothing of the token itself.
pub fn paginate<T>(
    items: Vec<T>,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Result<(Vec<T>, Option<String>), String> {
    let total = items.len();
    let start = match next_token {
        Some(token) => token
            .parse::<usize>()
            .ok()
            .filter(|start| *start <= total)
            .ok_or_else(|| NEXT_TOKEN_NOT_PROCESSABLE.to_string())?,
        None => 0,
    };
    let limit = max_results
        .filter(|requested| *requested > 0)
        .and_then(|requested| usize::try_from(requested).ok())
        .unwrap_or(total);
    let end = start.saturating_add(limit).min(total);
    let page: Vec<T> = items.into_iter().skip(start).take(end - start).collect();
    let token = (end < total).then(|| end.to_string());
    Ok((page, token))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBatchAwsbatchv20160810State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBatchAwsbatchv20160810State>)
}

/// How many entries one `PutEvents` request carries.
///
/// EventBridge models `PutEvents$Entries` as `length 1..10`, so a drain that
/// holds more changes than that is published in several requests rather than one
/// the service would refuse.
const MAX_PUT_EVENTS_ENTRIES: usize = 10;

/// How many job state changes ONE drain publishes.
///
/// This module's OWN bound. The queue itself is bounded ([`MAX_PENDING_EVENTS`]),
/// but one call draining all of it would dispatch that many changes over
/// `MAX_PENDING_EVENTS / MAX_PUT_EVENTS_ENTRIES` in-process requests, each of
/// which fans out to a rule's targets: the product is what this caps. What a drain
/// leaves behind keeps the clock pump awake
/// ([`ComAmazonawsBatchAwsbatchv20160810State::next_job_due_or_release_pump`]), so
/// nothing is dropped — it is published a tick later. The figure is several times
/// the changes a realistic burst produces (a job contributes at most three).
const MAX_PUBLISHED_PER_DRAIN: usize = 50;

/// Publish every job state change this store has recorded onto the account's
/// default EventBridge bus, and leave the queue empty.
///
/// This is Batch's side of the documented, polling-free way to observe a job:
/// "AWS Batch sends job state change events to EventBridge" with
/// `source` [`EVENT_SOURCE`] and `detail-type`
/// [`JOB_STATE_CHANGE_DETAIL_TYPE`] ("AWS Batch events"), so a rule on the
/// default bus matching that pair reaches its targets. EventBridge is reached the
/// way any emulated service reaches another: through the in-process callback
/// ([`smithy_server_runtime::OperationContext::callback`]), so this crate names
/// no other service crate.
///
/// Called by a handler AFTER it has released this store's lock, so no lock is
/// held across the call. Nothing about the answer changes what the caller is
/// told: publication is "on a best-effort basis" ("AWS Batch events"), and an
/// emulator with no EventBridge mounted has nowhere to publish, so a failure is
/// logged and the operation's own result stands.
pub async fn publish_job_state_changes<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &Eng,
    region: &str,
) {
    publish_pending(ctx.callback(), engine, region).await;
}

/// [`publish_job_state_changes`] over the callback alone, so the clock pump
/// ([`drive_job_clock`]) — which outlives the request that started it and has no
/// `OperationContext` — publishes through exactly the same path a handler does.
async fn publish_pending<Eng: Send + Sync + 'static>(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    engine: &Eng,
    region: &str,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    // The guard is taken and dropped inside this statement: the records are moved
    // out here so nothing is published twice, and no lock is held across an await.
    let changes = state.lock().take_pending_events(MAX_PUBLISHED_PER_DRAIN);
    if changes.is_empty() {
        return;
    }
    let config = ::aws_sdk_eventbridge::Config::builder()
        .behavior_version(::aws_sdk_eventbridge::config::BehaviorVersion::latest())
        .http_client(callback.http_client())
        .credentials_provider(callback.aws_credentials_provider())
        .region(::aws_sdk_eventbridge::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://events.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or
        // watch for a stalled stream, and each of those would need a sleep
        // implementation this client is not given. The same goes for caching the
        // identity: the credentials are handed over directly.
        .retry_config(::aws_sdk_eventbridge::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_eventbridge::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_eventbridge::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_eventbridge::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    let client = ::aws_sdk_eventbridge::Client::from_conf(config);
    for window in changes.chunks(MAX_PUT_EVENTS_ENTRIES) {
        let mut request = client.put_events();
        for change in window {
            let entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
                .event_bus_name(DEFAULT_EVENT_BUS)
                .source(EVENT_SOURCE)
                .detail_type(JOB_STATE_CHANGE_DETAIL_TYPE)
                // The event's `time` is WHEN the job moved, not when this queue was
                // drained: `PutEventsRequestEntry$Time` is "the time stamp of the
                // event, per RFC3339. If no time stamp is provided, the time stamp
                // of the PutEvents call is used."
                .time(::aws_sdk_eventbridge::primitives::DateTime::from_millis(
                    change.at,
                ))
                // "resources: the job's ARN" — the event names the resource it is
                // about, which is what a rule filtering on `resources` reads.
                .resources(&change.job_arn)
                .detail(change.detail.to_string())
                .build();
            request = request.entries(entry);
        }
        if let Err(error) = request.send().await {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a Batch job state change could not be published to EventBridge"
            );
        }
    }
}

/// The shortest one wake-up of the clock pump waits.
///
/// A due instant already in the past is materialised on the NEXT wake-up, not on
/// this one's instant, so the pump always hands the runtime back and a wake-up
/// that materialises nothing cannot turn the loop into a spin. It is far below
/// [`JOB_PHASE_MILLIS`], so it never delays a transition noticeably.
const MIN_CLOCK_WAIT_MILLIS: i64 = 5;

/// The longest one wake-up of the clock pump waits before it looks at the store
/// again.
///
/// Every transition this service owes falls due [`JOB_PHASE_MILLIS`] after the one
/// before it, so a wait is never near this value; it is a backstop, so a due
/// instant far in the future cannot park the pump indefinitely.
const MAX_CLOCK_WAIT_MILLIS: i64 = 5_000;

/// How many wake-ups one pump makes before it stands down.
///
/// A backstop, not a budget: the transitions the store owes are already finite
/// (each job makes at most a handful, and the store is capped by
/// [`limits::MAX_JOBS`]), and every wake-up either materialises a transition or
/// clears the due instant that woke it. The cap guarantees termination even if a
/// later change left a due instant that materialising does not clear, and the next
/// submit starts a fresh pump.
const MAX_CLOCK_WAKE_UPS: usize = 10_000;

/// Publish the job state changes the CLOCK owes, as they fall due, with no further
/// call from anybody.
///
/// AWS Batch notifies EventBridge as a job moves, which is the whole point of the
/// event path: a client that reacts to job completion through EventBridge makes no
/// further Batch call, so a transition published only by the next reader would
/// never be published at all. This starts ONE in-process task that waits for the
/// earliest due instant the store holds, materialises what is then due
/// ([`ComAmazonawsBatchAwsbatchv20160810State::materialize_due`], the same call
/// every operation makes) and publishes what that produced, until nothing is owed.
///
/// It does not change WHEN a transition happens: `materialize_due` applies only
/// what the stored due instant has already made due, so a caller reading before
/// that instant still sees the transient status, and two reads at the same instant
/// still answer identically. The task waits for the clock so that no request ever
/// has to.
///
/// At most one pump runs per store (the store carries the flag), it is bounded by
/// [`MAX_CLOCK_WAKE_UPS`] wake-ups, and it exits as soon as the store owes nothing.
/// With no async runtime to spawn onto there is no pump, and the transitions are
/// materialised and published by the next operation instead.
pub fn drive_job_clock<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &::std::sync::Arc<Eng>,
    region: &str,
) {
    let Some(state) = service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let Some(generation) = state.lock().start_clock_pump() else {
        return;
    };
    let callback = ctx.callback().clone();
    let engine = ::std::sync::Arc::clone(engine);
    let region = region.to_string();
    let pumped = ::engine::ServiceState::clone(&state);
    let handle = runtime.spawn(async move {
        // `true` only if the wake-up cap below is what ended the loop; a pump that
        // ran out of work has already released its claim inside the one critical
        // section that observed it (`next_job_due_or_release_pump`).
        let mut capped = true;
        for _ in 0..MAX_CLOCK_WAKE_UPS {
            let Some(due) = state
                .lock()
                .next_job_due_or_release_pump(now_millis(), generation)
            else {
                capped = false;
                break;
            };
            // Every wake-up waits, even for an instant already past: that is what
            // keeps the backstop above from ever costing a CPU-bound spin, and it
            // hands the runtime back between wake-ups.
            let wait = (due - now_millis()).clamp(MIN_CLOCK_WAIT_MILLIS, MAX_CLOCK_WAIT_MILLIS);
            ::tokio::time::sleep(::std::time::Duration::from_millis(
                u64::try_from(wait).unwrap_or(1),
            ))
            .await;
            state.lock().materialize_due(now_millis());
            publish_pending(&callback, &*engine, &region).await;
        }
        if capped {
            // Only THIS pump's claim, so a pump started since is left holding its own.
            state.lock().release_clock_pump(generation);
        }
    });
    // The handle is kept so the pump it replaces is aborted rather than merely
    // forgotten: that is what guarantees no earlier pump task runs beside the one
    // holding the claim.
    pumped.lock().set_clock_pump(handle);
}
