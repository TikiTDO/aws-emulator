// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sagemakerruntime-amazonsagemakerruntime service.
//!
//! This service is SageMaker's inference DATA plane: the endpoints it invokes are owned
//! by the SageMaker control plane, so an endpoint is resolved in that service's store
//! (`service_com_amazonaws_sagemaker_sagemaker::state`) through the one shared engine,
//! never copied into a second map here. What this store holds is what the data plane
//! itself produces: the log of the invocations it served, which is the emulator's
//! counterpart of the per-endpoint invocation metrics the real service publishes, and the
//! queue of the asynchronous submissions it accepted.
//!
//! # One resolver for the three operations
//!
//! All three operations this service models (`InvokeEndpoint`, `InvokeEndpointAsync`,
//! `InvokeEndpointWithResponseStream`) answer the SAME question first — "does the account
//! host this endpoint, and does it serve traffic?" — so that question is asked in ONE
//! place, [`resolve_endpoint`], and each handler maps the answer onto its own modelled
//! errors.
//!
//! # Clock-owed transitions
//!
//! An endpoint's status is NOT advanced here: the `Creating -> InService` transition
//! belongs to the control-plane call that started it, and [`resolve_endpoint`] only
//! materialises what that clock already owed (the same write `DescribeEndpoint` makes).
//! The one transition this service's own calls owe is an accepted asynchronous
//! submission's outcome, due [`ASYNC_OUTCOME_DUE_AFTER`] the call that queued it — see
//! [`AsyncInvocation::settle`].

/// Whether IAM lets the caller perform the invocation it asks for — the decision every
/// route takes before any of the state below is read. Held beside this store rather than
/// in it: the documents it is taken from belong to the IAM service, and nothing of it is
/// written here.
pub mod authorization;

use std::collections::VecDeque;
use std::time::{Duration, SystemTime};

/// The SageMaker CONTROL PLANE, whose store owns every endpoint this data plane invokes.
use ::service_com_amazonaws_sagemaker_sagemaker as sagemaker;

/// The message a handler refuses with when the service is mounted on an engine that
/// holds no store for it. Unreachable in the assembled emulator (the router mounts
/// every service on the one shared `InMemoryEngine`); stated once so no handler
/// invents its own wording — and never by building a private engine, which would drop
/// the request's writes.
pub const NO_STATE: &str = "SageMaker Runtime state is unavailable: this service is not \
                            mounted on the shared in-memory engine";

/// The message a handler refuses with when the SageMaker CONTROL PLANE's store is not
/// reachable on this engine, so the endpoint a request names can be neither resolved
/// nor refused as missing.
pub const NO_CONTROL_PLANE_STATE: &str =
    "SageMaker's endpoint store is unavailable: the SageMaker control plane is not \
     mounted on the shared in-memory engine";

/// How many invocations the log below keeps.
///
/// The log is bounded so a client invoking an endpoint in a loop cannot grow this
/// service's state without end; the oldest entry is dropped when a new one arrives at
/// the cap.
pub const MAX_RECORDED_INVOCATIONS: usize = 1024;

/// How many accepted asynchronous submissions the queue below keeps, bounded for the
/// same reason as [`MAX_RECORDED_INVOCATIONS`].
pub const MAX_QUEUED_ASYNC_INVOCATIONS: usize = 1024;

/// How long after `InvokeEndpointAsync` accepted a submission its outcome is due.
///
/// A CHOSEN span, not a published one: the real wait depends on the queue, the instance
/// count and the container. It is deliberately short, but never zero, so a caller that
/// reads the record straight back still observes [`AsyncInvocationStatus::InProgress`] —
/// the status the real service reports for a submission it has only queued.
pub const ASYNC_OUTCOME_DUE_AFTER: Duration = Duration::from_millis(500);

/// Why an accepted asynchronous invocation ends in [`AsyncInvocationStatus::Failed`].
///
/// A SageMaker model is a CONTAINER IMAGE of customer code, which a generated emulator
/// cannot run, so the queued inference is never performed. The submission is still
/// ACCEPTED (HTTP 202 is the answer to queueing it, which this service really did), and
/// the run it cannot perform ends in the failure state with this reason rather than in a
/// `Completed` that claims an output object nothing wrote.
pub const NO_INFERENCE_CONTAINER: &str =
    "No inference was performed: a SageMaker model runs as a customer-provided container \
     image, which this emulator does not run, so no output object was written to the \
     endpoint's AsyncInferenceConfig S3OutputPath.";

/// The `EndpointStatus` members with no deployment behind the endpoint, so an invocation
/// has nothing to route to.
///
/// Taken from the modelled enum (`ENDPOINT_STATUS_VALUES` in the SageMaker crate's
/// `ops/types/shared_types.rs`): `Creating` is still provisioning, `Deleting` is coming
/// down, and `Failed`, `OutOfService` and `UpdateRollbackFailed` have no serving
/// deployment at all. The enum's remaining members — `InService`, `Updating`,
/// `SystemUpdating` and `RollingBack` — all have the previous or new deployment up, and
/// the real service keeps serving inference in each of them.
pub const NON_SERVING_ENDPOINT_STATUSES: &[&str] = &[
    "Creating",
    "Deleting",
    "Failed",
    "OutOfService",
    "UpdateRollbackFailed",
];

/// What the service did with one `InvokeEndpoint` call.
///
/// Every variant but [`InvocationOutcome::NoInferenceContainer`] is a refusal of the
/// REQUEST, decided from the control plane's own record of the endpoint. The last one is
/// the honest answer to a request this emulator cannot serve: a model container is
/// customer-provided code in a container image, which a generated emulator cannot run,
/// so no inference is performed and the operation's modelled `ModelError` says so rather
/// than a synthesised prediction body claiming work that never ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvocationOutcome {
    /// No endpoint of that name exists in the account.
    EndpointNotFound,
    /// The endpoint exists but is not `InService`, so it serves no traffic yet.
    EndpointNotInService,
    /// `TargetVariant` named a production variant the endpoint does not host.
    VariantNotFound,
    /// `EnableExplanations` was set on an endpoint carrying no explainer configuration.
    ExplanationsNotConfigured,
    /// `SessionId` named a stateful session that does not exist.
    SessionNotFound,
    /// The endpoint hosts no asynchronous-inference configuration, so there is no output
    /// path to queue a submission against.
    AsyncInferenceNotConfigured,
    /// The submission carried neither an `InputLocation` nor an inline body, so there is
    /// no input to queue.
    NoInput,
    /// The submission carried BOTH an `InputLocation` and an inline body, which the model
    /// documents as mutually exclusive, so which one to queue is undecidable.
    InputAmbiguous,
    /// The request's `Filename` is not a value the member's modelled pattern admits, so it
    /// cannot name an object under the endpoint's configured output path. (The
    /// `S3OutputPathExtension` is always usable: its own pattern admits every segment it
    /// can carry, and the spliced key is never resolved here.)
    OutputLocationInvalid,
    /// The request's payload is past the maximum its member's modelled `@length` declares.
    /// Reached once the request's addressing resolved for a payload the service streams to
    /// the model container, and with the input-source choice for one that is a source.
    PayloadOverMaximum,
    /// Everything about the request resolved; the submission was queued.
    Queued,
    /// Everything about the request resolved and the model behind the endpoint computed the
    /// inference this call answered with.
    Served,
    /// Everything about the request resolved; no container ran the inference.
    NoInferenceContainer,
}

impl InvocationOutcome {
    /// A stable short name for the outcome, for logs and test assertions.
    pub fn label(self) -> &'static str {
        match self {
            Self::EndpointNotFound => "EndpointNotFound",
            Self::EndpointNotInService => "EndpointNotInService",
            Self::VariantNotFound => "VariantNotFound",
            Self::ExplanationsNotConfigured => "ExplanationsNotConfigured",
            Self::SessionNotFound => "SessionNotFound",
            Self::AsyncInferenceNotConfigured => "AsyncInferenceNotConfigured",
            Self::NoInput => "NoInput",
            Self::InputAmbiguous => "InputAmbiguous",
            Self::OutputLocationInvalid => "OutputLocationInvalid",
            Self::PayloadOverMaximum => "PayloadOverMaximum",
            Self::Queued => "Queued",
            Self::Served => "Served",
            Self::NoInferenceContainer => "NoInferenceContainer",
        }
    }
}

/// One invocation this service served, as the log below replays it.
///
/// The request BODY and `CustomAttributes` are `@sensitive` in the model, so neither is
/// stored: the record keeps the body's SIZE, which is what a metric would report, and
/// the non-sensitive routing members of the request.
#[derive(Debug, Clone)]
pub struct EndpointInvocation {
    pub endpoint_name: String,
    /// The production variant the request TARGETED, or `None` when it named none —
    /// without `TargetVariant` the real service distributes the request across the
    /// endpoint's variants by their weights, and no container served this one, so no
    /// variant is recorded for it.
    pub target_variant: Option<String>,
    /// `Body`'s length in bytes; the body itself is `@sensitive` and is not stored.
    pub request_bytes: usize,
    pub content_type: Option<String>,
    pub accept: Option<String>,
    pub inference_id: Option<String>,
    pub target_model: Option<String>,
    pub session_id: Option<String>,
    pub outcome: InvocationOutcome,
    pub at: SystemTime,
}

/// The lifecycle of an accepted asynchronous submission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsyncInvocationStatus {
    /// Queued, and its outcome not yet due.
    InProgress,
    /// Its outcome was due and nothing performed the inference — see
    /// [`NO_INFERENCE_CONTAINER`].
    Failed,
}

/// One accepted `InvokeEndpointAsync` submission, as the queue below holds it.
///
/// `Body` and `CustomAttributes` are `@sensitive`, so an inline payload is kept only as
/// its SIZE and the custom attributes are not kept at all. The two locations are the
/// ones the response reported, stored so a later read answers the same strings.
#[derive(Debug, Clone)]
pub struct AsyncInvocation {
    pub inference_id: String,
    pub endpoint_name: String,
    pub input_location: Option<String>,
    /// The inline `Body`'s length in bytes, when the submission carried one.
    pub inline_input_bytes: Option<usize>,
    pub content_type: Option<String>,
    pub accept: Option<String>,
    pub output_location: String,
    pub failure_location: Option<String>,
    pub request_ttl_seconds: Option<i32>,
    pub invocation_timeout_seconds: Option<i32>,
    pub status: AsyncInvocationStatus,
    /// Why the invocation failed, once its outcome is due.
    pub failure_reason: Option<String>,
    pub submitted_at: SystemTime,
    /// When the queued invocation's outcome falls due — [`ASYNC_OUTCOME_DUE_AFTER`]
    /// after [`Self::submitted_at`].
    pub outcome_due_at: SystemTime,
}

impl AsyncInvocation {
    /// Materialise the outcome the clock already owed this submission.
    ///
    /// Owed at [`Self::outcome_due_at`] whether or not anybody read the record, so a
    /// reader makes no difference to the result. Answers whether this call moved it.
    pub fn settle(&mut self, now: SystemTime) -> bool {
        if self.status == AsyncInvocationStatus::InProgress && now >= self.outcome_due_at {
            self.status = AsyncInvocationStatus::Failed;
            self.failure_reason = Some(NO_INFERENCE_CONTAINER.to_string());
            return true;
        }
        false
    }
}

/// What the control plane records about the endpoint a request names, as a data-plane
/// handler reads it.
#[derive(Debug, Clone)]
pub struct EndpointFacts {
    pub arn: String,
    pub status: String,
    /// The names of the production variants the endpoint hosts, in the order the
    /// endpoint configuration listed them.
    pub production_variants: Vec<String>,
    /// Whether the endpoint carries an explainer configuration, which is what
    /// `EnableExplanations` overrides a parameter of.
    pub has_explainer_config: bool,
    /// The endpoint's asynchronous-inference output paths, when its endpoint
    /// configuration carries an `AsyncInferenceConfig`.
    pub async_output: Option<AsyncOutputPaths>,
    /// Whether this call materialised the `Creating` -> `InService` transition the clock
    /// already owed.
    pub settled: bool,
}

/// Where an endpoint's asynchronous inference results are written, as
/// `AsyncInferenceOutputConfig` configured them.
#[derive(Debug, Clone)]
pub struct AsyncOutputPaths {
    pub s3_output_path: Option<String>,
    pub s3_failure_path: Option<String>,
}

/// `SessionId`'s constant that asks for a stateful session to be created: "Create a
/// stateful session by specifying the value NEW_SESSION"
/// (`InvokeEndpointInput$SessionId`'s model doc). The member's own pattern
/// (`^(NEW_SESSION)$|^[a-zA-Z0-9](-*[a-zA-Z0-9])*$`) carries it as its first alternative,
/// so every other accepted value is a session ID. Only `InvokeEndpoint` models it:
/// `InvokeEndpointWithResponseStream$SessionId`'s pattern has no such alternative, which
/// is what [`NewSessionConstant`] says.
pub const NEW_SESSION: &str = "NEW_SESSION";

/// Whether the calling operation's `SessionId` member admits the [`NEW_SESSION`] constant.
///
/// A named choice rather than a bare flag, because the two call sites differ only in this:
/// `InvokeEndpointInput$SessionId`'s pattern carries `^(NEW_SESSION)$` as an alternative,
/// and `InvokeEndpointWithResponseStreamInput$SessionId`'s does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewSessionConstant {
    /// The member admits `NEW_SESSION`, so that value ASKS for a session rather than
    /// naming one.
    Admitted,
    /// The member does not model `NEW_SESSION`, so every accepted value names a session
    /// that must already exist.
    NotModelled,
}

/// Whether the request's `TargetVariant` names a production variant the endpoint hosts,
/// answering the refusal a handler should render when it does not.
///
/// `TargetVariant` "[s]pecif[ies] the production variant to send the inference request
/// to", so it has to name one of the variants the endpoint hosts. The rule is asked HERE,
/// beside [`resolve_endpoint`], because `InvokeEndpoint` and
/// `InvokeEndpointWithResponseStream` both bind the member and must not drift apart in
/// what they accept or in what they say about it.
pub fn check_target_variant(
    facts: &EndpointFacts,
    endpoint_name: &str,
    target_variant: Option<&str>,
) -> Option<(InvocationOutcome, String)> {
    let target = target_variant?;
    if facts
        .production_variants
        .iter()
        .any(|hosted| hosted == target)
    {
        return None;
    }
    Some((
        InvocationOutcome::VariantNotFound,
        format!(
            "Variant {target} not found for endpoint {endpoint_name}. The endpoint hosts: {}.",
            facts.production_variants.join(", ")
        ),
    ))
}

/// Whether the request's `SessionId` names a live stateful session, answering the refusal
/// a handler should render when it does not.
///
/// A stateful session is created BY THE MODEL: "When you create a session with a stateful
/// model, the model must create the session ID and set the expiration time"
/// (`InvokeEndpointInput$SessionId`'s model doc). So a session exists only once a container
/// has answered a [`NEW_SESSION`] request, and no container runs here: every id names a
/// session that does not exist. `new_session` says whether the calling operation's member
/// models the constant at all — where it does, that value asks for a session rather than
/// naming one, so it falls through to the inference step.
pub fn check_session_id(
    endpoint_name: &str,
    session_id: Option<&str>,
    new_session: NewSessionConstant,
) -> Option<(InvocationOutcome, String)> {
    let session_id = session_id?;
    if new_session == NewSessionConstant::Admitted && session_id == NEW_SESSION {
        return None;
    }
    Some((
        InvocationOutcome::SessionNotFound,
        format!("Session {session_id} not found for endpoint {endpoint_name}."),
    ))
}

/// The answer to "does the account host this endpoint?".
#[derive(Debug)]
pub enum EndpointLookup {
    /// The SageMaker control plane is not mounted on this engine, so the question cannot
    /// be answered either way.
    NoControlPlane,
    /// No endpoint record of that name: the account hosts no such endpoint.
    NotFound,
    /// The endpoint, as the control plane records it.
    Found(EndpointFacts),
}

/// Resolve `endpoint_name` in the SageMaker CONTROL PLANE's own endpoint store.
///
/// The endpoint is the control plane's resource, so it is read there and nowhere else:
/// this service keeps no second copy that could drift from `CreateEndpoint`'s. The one
/// write this makes is the `Creating` -> `InService` transition the clock already owed,
/// which was due at the stored instant whether or not anybody invoked the endpoint — the
/// same transition `DescribeEndpoint` materialises — and [`EndpointFacts::settled`]
/// reports it so the caller can record the state change.
pub fn resolve_endpoint<Eng: Send + Sync + 'static>(
    engine: &Eng,
    endpoint_name: &str,
    now: SystemTime,
) -> EndpointLookup {
    let Some(control_plane) = sagemaker::state::service_state(engine) else {
        return EndpointLookup::NoControlPlane;
    };
    let mut store = control_plane.lock();
    let Some(endpoint) = store.endpoints.get_mut(endpoint_name) else {
        return EndpointLookup::NotFound;
    };
    let settled = endpoint.settle_status(now);
    EndpointLookup::Found(EndpointFacts {
        arn: endpoint.arn.clone(),
        status: endpoint.status.clone(),
        production_variants: endpoint
            .production_variants
            .iter()
            .map(|variant| variant.variant_name.clone())
            .collect(),
        has_explainer_config: endpoint.explainer_config.is_some(),
        async_output: endpoint
            .async_inference_config
            .as_ref()
            .map(|config| AsyncOutputPaths {
                s3_output_path: config.output_config.s3_output_path.clone(),
                s3_failure_path: config.output_config.s3_failure_path.clone(),
            }),
        settled,
    })
}

/// The variants `endpoint_name` serves, each with the model deployed behind it and that
/// model's artifact, read out of the SageMaker CONTROL PLANE's own stores.
///
/// The endpoint, its configuration and the models behind it are all the control plane's
/// resources, so the reading of them lives there
/// (`sagemaker::state::endpoint_serving_variants`) and this is only the lock around it: this
/// service keeps no second copy of any of them.
pub fn serving_variants<Eng: Send + Sync + 'static>(
    engine: &Eng,
    endpoint_name: &str,
) -> Vec<sagemaker::state::ServingVariant> {
    let Some(control_plane) = sagemaker::state::service_state(engine) else {
        return Vec::new();
    };
    let store = control_plane.lock();
    sagemaker::state::endpoint_serving_variants(&store, endpoint_name)
}

/// Per-service state for com-amazonaws-sagemakerruntime-amazonsagemakerruntime. A handler reaches it through [`service_state`].
///
/// It holds no endpoint: an endpoint is the control plane's resource and is resolved
/// there (see [`resolve_endpoint`]). What is here is what this data plane itself
/// produced — the invocations it answered and the asynchronous submissions it queued.
#[derive(Debug, Default)]
pub struct ComAmazonawsSagemakerruntimeAmazonsagemakerruntimeState {
    /// Every invocation this service served, oldest first, capped at
    /// [`MAX_RECORDED_INVOCATIONS`].
    invocations: VecDeque<EndpointInvocation>,
    /// Every asynchronous submission this service accepted, oldest first, capped at
    /// [`MAX_QUEUED_ASYNC_INVOCATIONS`].
    async_invocations: VecDeque<AsyncInvocation>,
    /// How many asynchronous submissions this store has ever accepted.
    ///
    /// MONOTONIC, unlike the queue's length, which falls back as the cap evicts: it is
    /// what a generated `InferenceId` is mixed from (see
    /// [`Self::next_submission_sequence`]), so two submissions cannot share one even
    /// after eviction.
    async_submissions: u64,
    /// How many of the queue's LEADING submissions have already settled.
    ///
    /// The queue is oldest-first and every submission's outcome falls due the same span
    /// after it was accepted, so the settled ones are always a prefix: this is where
    /// [`Self::settle_async_invocations`] starts, which is what keeps the common case (a
    /// request arriving with nothing newly due) O(1) rather than a walk of the whole
    /// queue under the lock. It is decremented with every eviction, so it keeps naming
    /// the same submission.
    settled_prefix: usize,
}

impl ComAmazonawsSagemakerruntimeAmazonsagemakerruntimeState {
    /// Append `invocation` to the log, dropping the oldest entry at the cap.
    pub fn record_invocation(&mut self, invocation: EndpointInvocation) {
        while self.invocations.len() >= MAX_RECORDED_INVOCATIONS {
            self.invocations.pop_front();
        }
        self.invocations.push_back(invocation);
    }

    /// Every logged invocation, oldest first.
    pub fn invocations(&self) -> impl ExactSizeIterator<Item = &EndpointInvocation> {
        self.invocations.iter()
    }

    /// The logged invocations of one endpoint, oldest first.
    pub fn invocations_of<'log>(
        &'log self,
        endpoint_name: &'log str,
    ) -> impl Iterator<Item = &'log EndpointInvocation> {
        self.invocations
            .iter()
            .filter(move |invocation| invocation.endpoint_name == endpoint_name)
    }

    /// The most recently logged invocation.
    pub fn last_invocation(&self) -> Option<&EndpointInvocation> {
        self.invocations.back()
    }

    /// Queue an accepted asynchronous submission, dropping the oldest at the cap.
    ///
    /// Every submission's outcome falls due the same span after it was accepted, so
    /// pushing at the back keeps [`AsyncInvocation::outcome_due_at`] rising with position —
    /// the ordering [`Self::settle_async_invocations`] stops its walk on. A future outcome
    /// driven by a per-request value (a `RequestTTLSeconds` expiry, say) would break that,
    /// and would have to be settled by a walk of the whole queue.
    pub fn queue_async_invocation(&mut self, invocation: AsyncInvocation) {
        while self.async_invocations.len() >= MAX_QUEUED_ASYNC_INVOCATIONS {
            self.async_invocations.pop_front();
            // The evicted submission was part of the settled prefix unless the prefix was
            // already empty, so the index keeps naming the same record.
            self.settled_prefix = self.settled_prefix.saturating_sub(1);
        }
        self.async_invocations.push_back(invocation);
    }

    /// Take the next submission sequence number: MONOTONIC, unlike the queue's length,
    /// which falls back as the cap evicts.
    ///
    /// Taken and advanced in ONE locked call, so two submissions in the same instant
    /// cannot mix the same value into a generated `InferenceId`.
    pub fn next_submission_sequence(&mut self) -> u64 {
        self.async_submissions = self.async_submissions.saturating_add(1);
        self.async_submissions
    }

    /// Every queued asynchronous submission, oldest first.
    pub fn async_invocations(&self) -> impl ExactSizeIterator<Item = &AsyncInvocation> {
        self.async_invocations.iter()
    }

    /// The most recent submission made under `inference_id`.
    ///
    /// The id is the caller's to choose and the real service does not make it unique, so
    /// two submissions may carry one id; the latest is the one a reader asked for.
    pub fn async_invocation(&self, inference_id: &str) -> Option<&AsyncInvocation> {
        self.async_invocations
            .iter()
            .rev()
            .find(|invocation| invocation.inference_id == inference_id)
    }

    /// Materialise the outcome the clock already owed every queued submission.
    ///
    /// The queue is oldest-first and every submission's outcome falls due the SAME span
    /// after it was accepted ([`ASYNC_OUTCOME_DUE_AFTER`]), so `outcome_due_at` rises with
    /// insertion order and the settled submissions are a PREFIX of the queue. The walk
    /// therefore starts past that prefix ([`Self::settled_prefix`]) and stops at the first
    /// record whose outcome is not yet due — so a request that owes nothing touches no
    /// record at all, instead of scanning the whole queue under the lock.
    pub fn settle_async_invocations(&mut self, now: SystemTime) {
        let start = self.settled_prefix.min(self.async_invocations.len());
        let mut settled = start;
        for invocation in self.async_invocations.range_mut(start..) {
            if invocation.status == AsyncInvocationStatus::InProgress
                && now < invocation.outcome_due_at
            {
                break;
            }
            invocation.settle(now);
            settled += 1;
        }
        self.settled_prefix = settled;
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSagemakerruntimeAmazonsagemakerruntimeState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSagemakerruntimeAmazonsagemakerruntimeState>)
}
